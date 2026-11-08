//! The defence manager (`AIDefenseMan.c` as Cataclysm has it): the mothership's sphere of
//! influence and the enemy ships leading into it (`aidCheckSphereOfInfluence`), escorts for the
//! resourcers (`aidSetupResourceDefenseTeams`, `aioCreateGuardShips`) and the defence of the
//! mothership (`aidMothershipDefense`): new dangerous ships and idle teams, then guards and every
//! team in range when the attackers outnumber them, go after the ships attacking it.
//!
//! The roving defence teams of an advanced player are in [`super::roving`]. Not yet ported:
//! guard teams recalled to the sphere of influence, distress calls from resourcers, and the HW1
//! races' cloak defence.

use hwc_math::fpu::{ld, st};

use super::attack::find_enemy_mothership;
use super::team::{self, add_move, move_type, AiMove, MoveKind, SAME_FORMATION};
use super::{ai, ai_mut, Manager};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// Feature bits (`DefenseFeatures`, +0x10).
pub mod feature {
    pub const RESOURCE_DEFENSE: u32 = 0x1;
    pub const ROVING_DEFENSE: u32 = 0x2;
    pub const SPHERE_OF_INFLUENCE: u32 = 0x4;
    pub const MOTHERSHIP_DEFENSE: u32 = 0x10;
    pub const GUARDS_DEFEND_MOTHERSHIP: u32 = 0x20;
    pub const ALL_DEFEND_MOTHERSHIP: u32 = 0x40;
    pub const CLOAK_DEFENSE: u32 = 0x100;
}

/// Most defence teams (`AIPLAYER_MAX_DEFENSE_TEAMS` as the resource escorts check it).
const MAX_DEFENSE_TEAMS: usize = 12;
/// The ratio of defenders to attackers that brings in more defenders (0x7409f8).
const DEFENDER_RATIO: f32 = f32::from_bits(0x3fb3_3333);
/// Range of the enemies near the mothership's attacker (0x740a14).
const ATTACKER_NEIGHBOURS: f32 = 30000.0;
/// Escort sizes for capital ships and others (0x740a34, 0x740a38).
const ESCORT_CAPITAL: i32 = 12;
const ESCORT_OTHER: i32 = 6;
/// Formation of a mothership defender's attack (0x740af0).
const DEFEND_ATTACK_FORMATION: i32 = 4;

/// The defence manager's part of `struct AIPlayer`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct DefenseState {
    /// +0x1cdc.. the defence teams.
    pub teams: Vec<usize>,
    /// +0x1cd4: the team defending the mothership.
    pub mothership_team: Option<usize>,
    /// +0x1cb0: the ships attacking the mothership.
    pub mothership_attackers: Vec<ShipId>,
    /// +0x1cc4 distress, +0x1cc8 threats, +0x1ccc ships to rescue: cleared every turn.
    pub distress: Vec<ShipId>,
    pub threats: Vec<ShipId>,
    pub rescue: Vec<ShipId>,
}

/// `aidInit` (0x403950): the features by level.
pub fn init(u: &mut Universe, p: usize) {
    let a = ai_mut(u, p);
    a.defense = DefenseState::default();
    a.defense_features = match a.level {
        1 => 0x131,
        2 => 0x246 | 0x131,
        _ => 0,
    };
}

/// `aidClearDistressSignal` (0x402870).
pub fn clear_distress_signal(u: &mut Universe, p: usize) {
    let d = &mut ai_mut(u, p).defense;
    d.distress.clear();
    d.threats.clear();
    d.rescue.clear();
}

/// `aidTeamDied`.
pub fn team_died(a: &mut super::AiPlayer, t: usize) {
    a.defense.teams.retain(|&x| x != t);
    if a.defense.mothership_team == Some(t) {
        a.defense.mothership_team = None;
    }
}

/// `aidShipDied`.
pub fn ship_died(a: &mut super::AiPlayer, id: ShipId) {
    let d = &mut a.defense;
    for l in [&mut d.mothership_attackers, &mut d.distress, &mut d.threats, &mut d.rescue] {
        l.retain(|&s| s != id);
    }
}

fn team_ships(u: &Universe, t: usize) -> Vec<ShipId> {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).map(|x| x.1.ships.clone()).unwrap_or_default()
}

/// Releases a team's ships to the new ships and destroys it.
fn release(u: &mut Universe, t: usize) {
    team::destroy(u, t);
}

/// `aidDefenseManager` (0x4035a0).
pub fn manager(u: &mut Universe, p: usize) {
    for t in ai(u, p).defense.teams.clone() {
        if team::team_is_done(u, t) {
            release(u, t);
        }
    }
    let f = ai(u, p).defense_features;
    if f & feature::RESOURCE_DEFENSE != 0 {
        if f & feature::ROVING_DEFENSE == 0 {
            setup_resource_defense_teams(u, p);
        } else {
            super::roving::manage(u, p);
        }
    }
    check_sphere_of_influence(u, p);
    if f & feature::MOTHERSHIP_DEFENSE != 0 {
        mothership_defense(u, p);
    }
}

/// `aidCheckSphereOfInfluence` (0x4028d0): the sphere's radius (a quarter of the squared
/// distance to the enemy mothership, renewed every sixteen seconds), and with the feature the
/// enemy ships leading into it become distress calls and threats.
fn check_sphere_of_influence(u: &mut Universe, p: usize) {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.posinfo.position) else { return };
    if ai(u, p).attack.sphere_of_influence == 0.0 || (u.total_time_elapsed as i64) & 15 == 0 {
        let Some(enemy) = find_enemy_mothership(u, p).and_then(|m| u.ships.get(m)).map(|s| s.posinfo.position) else { return };
        let d = crate::level::distance_squared(enemy, ms);
        ai_mut(u, p).attack.sphere_of_influence = st(d * 0.25);
    }
    if ai(u, p).defense_features & feature::SPHERE_OF_INFLUENCE != 0 {
        let radius2 = ai(u, p).attack.sphere_of_influence;
        let lead = find_lead_ships_in_sphere(u, p, ms, radius2);
        let d = &mut ai_mut(u, p).defense;
        for id in lead {
            if !d.distress.contains(&id) {
                d.distress.push(id);
            }
            if !d.threats.contains(&id) {
                d.threats.push(id);
            }
        }
    }
}

/// `aiuFindLeadShipInSphereOfInfluence` (0x41d050): in each enemy blob the player's ships are
/// in whose centre is inside the sphere, the first enemy ship inside it (a fighter only when
/// the blob has more than three enemies).
fn find_lead_ships_in_sphere(u: &Universe, p: usize, centre: hwc_math::Vector, radius2: f32) -> Vec<ShipId> {
    let mut out = Vec::new();
    for b in super::attack::enemy_blobs(u, p) {
        if !b.own || crate::level::distance_squared(b.centre, centre) >= f64::from(radius2) {
            continue;
        }
        let enemies: Vec<ShipId> = b.ships.iter().copied().filter(|&id| super::attack::is_enemy(u, p, id)).collect();
        let n = enemies.len();
        for &id in &enemies {
            let s = u.ships.ship(id);
            let fighter = u.statics.ship(s.ship_type).and_then(|st| st.ship_class) == Some(ShipClass::Fighter);
            if crate::level::distance_squared(s.posinfo.position, centre) < f64::from(radius2) && (!fighter || n > 3) {
                out.push(id);
                break;
            }
        }
    }
    out
}

/// `aidSetupResourceDefenseTeams` (0x402aa0): every resourcer or processor of the resource
/// manager no team guards gets an escort team (up to the most defence teams).
fn setup_resource_defense_teams(u: &mut Universe, p: usize) {
    for id in ai(u, p).resource_reserves.clone() {
        let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { continue };
        if !matches!(t, ShipType::sWorker | ShipType::sProcessor | ShipType::bWorker | ShipType::bProcessor | ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate | ShipType::tResourceCollector) {
            continue;
        }
        let guarded = u.ai.teams.iter().flatten().filter(|(o, _)| *o == p).any(|(_, tm)| tm.moves.iter().any(|m| matches!(&m.kind, MoveKind::GuardShips { ships } if ships.contains(&id))));
        if guarded || ai(u, p).defense.teams.len() >= MAX_DEFENSE_TEAMS {
            continue;
        }
        let team = super::team_create(u, p, Manager::Defense, "");
        ai_mut(u, p).defense.teams.push(team);
        create_guard_ships(u, p, team, vec![id]);
    }
}

/// `aioCreateGuardShips` (0x40e250): Acolytes (Somtaaw) or Interceptors (Beast) or what stands
/// in for them, a sphere formation, then they guard the ships.
fn create_guard_ships(u: &mut Universe, p: usize, t: usize, ships: Vec<ShipId>) {
    let capital = ships.iter().any(|&id| u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.is_capital_ship));
    let num = if capital { ESCORT_CAPITAL } else { ESCORT_OTHER };
    let acq = |u: &Universe, n: u8| ShipType::from_index(n.into()).is_some_and(|ty| super::research::ship_type_acquired(u, p, ty));
    match u.players[p].race {
        ShipRace::Somtaaw => super::attack::fancy_get_ships(u, t, 1, num, &[(0, 20), (5, 15), (3, 10), (1, 18), (4, 24), (17, 24)], true, 0),
        ShipRace::Beast => {
            let a = if acq(u, 0x22) { 0x22 } else { 0x23 };
            let b = if acq(u, 0x26) { 0x26 } else { 0x25 };
            let c = if acq(u, 0x34) { 0x34 } else { 0x24 };
            super::attack::fancy_get_ships(u, t, 0x23, num, &[(0x21, 20), (0x24, 15), (0x25, 10), (0x23, 18), (0x28, 24), (0x27, 24), (a, 18), (b, 24), (c, 24)], true, 0);
        }
        _ => {}
    }
    add_move(u, t, AiMove::new(MoveKind::Formation { formation: 5 }, SAME_FORMATION, 2, false, false));
    add_move(u, t, AiMove::new(MoveKind::GuardShips { ships }, SAME_FORMATION, 2, false, false));
    add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aiuIsShipDangerous` (0x41a990).
pub fn is_dangerous(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    match u.statics.ship(s.ship_type).and_then(|st| st.ship_class) {
        Some(ShipClass::HeavyCruiser | ShipClass::Carrier | ShipClass::Destroyer | ShipClass::Corvette | ShipClass::Fighter | ShipClass::MonsterShip | ShipClass::Micro) => true,
        Some(ShipClass::Frigate) => !matches!(s.ship_type, ShipType::bDFGFrigate | ShipType::tDFGFrigate),
        _ => false,
    }
}

/// `aiuMakeShipsOnlyDangerousToMothership` (0x41adf0): capital ships, frigates (not the
/// support frigates), corvettes when more than three (minelayers always), bombers when six or
/// more.
pub fn only_dangerous_to_mothership(u: &Universe, sel: &mut Vec<ShipId>) {
    let mut i = 0;
    while i < sel.len() {
        let n = sel.len();
        let s = u.ships.ship(sel[i]);
        let keep = match u.statics.ship(s.ship_type).and_then(|st| st.ship_class) {
            Some(ShipClass::HeavyCruiser | ShipClass::Carrier | ShipClass::Destroyer) => true,
            Some(ShipClass::Frigate) => !matches!(s.ship_type, ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate),
            Some(ShipClass::Corvette) => s.ship_type == ShipType::tMinelayerCorvette || n > 3,
            Some(ShipClass::Fighter) => matches!(s.ship_type, ShipType::kAttackBomber | ShipType::tAttackBomber) && n >= 6,
            _ => false,
        };
        if keep {
            i += 1;
        } else {
            // clRemoveTargetFromSelection: the last ship moves into the hole.
            sel.swap_remove(i);
        }
    }
}

/// `aiuEnemyShipsInMothershipBlob` (0x41a8d0).
fn enemy_ships_in_mothership_blob(u: &Universe, p: usize) -> Vec<ShipId> {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)) else { return Vec::new() };
    let Some(b) = ms.coll_my_blob.and_then(|b| u.blobs.get(b)) else { return Vec::new() };
    b.ships
        .iter()
        .copied()
        .filter(|&id| u.ships.get(id).is_some_and(|s| !u.players.get(s.player).is_some_and(|pl| pl.is_ally(p)) && s.player != p && !matches!(s.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1)))
        .collect()
}

/// `aitTeamIsIdle`: no move, or done, or getting ships.
fn team_is_idle(u: &Universe, t: usize) -> bool {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).is_none_or(|(_, tm)| tm.current.is_none_or(|c| matches!(tm.moves[c].move_type(), move_type::DONE | move_type::GET_SHIPS | move_type::FANCY_GET_SHIPS)))
}

/// `aitTeamIsGuarding`.
fn team_is_guarding(u: &Universe, t: usize) -> bool {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).is_some_and(|(_, tm)| tm.current.is_some_and(|c| matches!(tm.moves[c].move_type(), 1 | 0xb | 0xc | 0x1f)))
}

/// `aitTeamIsntDefendingMothership` (0x417420).
fn team_isnt_defending_mothership(u: &Universe, t: usize, attackers: &[ShipId]) -> bool {
    let Some((_, tm)) = u.ai.teams.get(t).and_then(|x| x.as_ref()) else { return false };
    let Some(c) = tm.current else { return false };
    match &tm.moves[c].kind {
        MoveKind::DefendMothership => false,
        MoveKind::Attack { targets } | MoveKind::FlankAttack { targets } if !targets.is_empty() && targets.iter().all(|x| attackers.contains(x)) => false,
        _ => true,
    }
}

/// `aitTeamIsInMothershipRange` (0x4174f0): the team's first ship is within 40 seconds (30 from
/// a carrier) of the mothership at the speed of its slowest ship.
fn team_is_in_mothership_range(u: &Universe, p: usize, ships: &[ShipId]) -> bool {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)) else { return false };
    let Some(first) = ships.first().and_then(|&id| u.ships.get(id)) else { return false };
    let mut slowest = f64::from(1e21f32);
    for s in ships.iter().filter_map(|&id| u.ships.get(id)) {
        let Some(stat) = u.statics.ship(s.ship_type) else { continue };
        let v = st(crate::tactics::ships_max_velocity(s, stat, u.max_velocity_context(s)) * ld(st(crate::tactics::speed_multiplier(s, stat))));
        if ld(v) < slowest {
            slowest = ld(v);
        }
    }
    let ms_is_mothership = u.statics.ship(ms.ship_type).and_then(|st| st.ship_class) == Some(ShipClass::Mothership);
    let seconds2 = if ms_is_mothership { 1600.0 } else { 900.0 };
    let d = crate::level::distance_squared(first.posinfo.position, ms.posinfo.position);
    ld(st(slowest * slowest * seconds2)) > d
}

/// Inserts a defend-the-mothership move into the team (`aimCreateDefendMothershipNoAdd` +
/// `aimInsertMove`).
fn defend(u: &mut Universe, t: usize) {
    team::insert_move(u, t, AiMove::new(MoveKind::DefendMothership, SAME_FORMATION, 2, true, true));
}

/// `aidMothershipDefense` (0x403130).
fn mothership_defense(u: &mut Universe, p: usize) {
    let mut attackers = enemy_ships_in_mothership_blob(u, p);
    if attackers.is_empty() {
        if let Some(rocker) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).and_then(|s| s.getting_rocked) {
            attackers = super::attack::find_nearby_enemy_ships(u, p, rocker, ATTACKER_NEIGHBOURS);
            attackers.push(rocker);
        }
    }
    let lone_fighter = attackers.len() == 1 && u.ships.get(attackers[0]).and_then(|s| u.statics.ship(s.ship_type)).and_then(|st| st.ship_class) == Some(ShipClass::Fighter);
    if attackers.is_empty() || lone_fighter {
        if lone_fighter {
            u.randy.random(hwc_math::random::RAN_AI_PLAYER);
        }
        ai_mut(u, p).defense.mothership_attackers.clear();
        if let Some(t) = ai(u, p).defense.mothership_team {
            release(u, t);
            ai_mut(u, p).defense.mothership_team = None;
        }
        return;
    }
    ai_mut(u, p).defense.mothership_attackers = attackers.clone();
    // Dangerous new ships join the mothership's defence team.
    for id in ai(u, p).new_ships.clone() {
        if !is_dangerous(u, id) {
            continue;
        }
        let t = match ai(u, p).defense.mothership_team {
            Some(t) => t,
            None => {
                let t = super::team_create(u, p, Manager::Defense, "");
                add_move(u, t, AiMove::new(MoveKind::DefendMothership, SAME_FORMATION, 2, true, false));
                add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
                let d = &mut ai_mut(u, p).defense;
                d.teams.push(t);
                d.mothership_team = Some(t);
                t
            }
        };
        ai_mut(u, p).new_ships.retain(|&s| s != id);
        super::team_add_ship(u, t, id);
    }
    let teams: Vec<usize> = u.ai.teams.iter().enumerate().filter_map(|(i, x)| x.as_ref().filter(|(o, tm)| *o == p && tm.manager != Manager::Script).map(|_| i)).collect();
    // Idle teams with dangerous ships defend.
    let mut defenders: Vec<ShipId> = Vec::new();
    for &t in &teams {
        let ships = team_ships(u, t);
        if team_is_idle(u, t) && ships.first().is_some_and(|&s| is_dangerous(u, s)) {
            defend(u, t);
            defenders.extend(ships);
        }
    }
    only_dangerous_to_mothership(u, &mut attackers);
    let f = ai(u, p).defense_features;
    let outnumbered = |defenders: &[ShipId], attackers: &[ShipId]| defenders.is_empty() || (defenders.len() as f32 / attackers.len() as f32) > DEFENDER_RATIO;
    if !attackers.is_empty() && f & feature::GUARDS_DEFEND_MOTHERSHIP != 0 && outnumbered(&defenders, &attackers) {
        for &t in &teams {
            let ships = team_ships(u, t);
            if team_is_guarding(u, t) && ships.first().is_some_and(|&s| is_dangerous(u, s)) {
                defend(u, t);
                defenders.extend(ships);
            }
        }
        if f & feature::ALL_DEFEND_MOTHERSHIP != 0 && outnumbered(&defenders, &attackers) {
            // aidAddAllShips: every team in range whose ships are not motherships or carriers.
            for &t in &teams {
                let ships = team_ships(u, t);
                let Some(&first) = ships.first() else { continue };
                let class = u.ships.get(first).and_then(|s| u.statics.ship(s.ship_type)).and_then(|st| st.ship_class);
                if matches!(class, Some(ShipClass::Mothership | ShipClass::Carrier)) || !is_dangerous(u, first) {
                    continue;
                }
                if team_isnt_defending_mothership(u, t, &attackers) && team_is_in_mothership_range(u, p, &ships) {
                    defend(u, t);
                }
            }
        }
    }
}

/// `aimProcessDefendMothership` (0x409660): the team attacks the mothership's attacker nearest
/// its first ship; done when none is left.
pub fn process_defend_mothership(u: &mut Universe, p: usize, t: usize) -> bool {
    let ships = team_ships(u, t);
    let Some(&first) = ships.first() else { return false };
    let Some(target) = nearest_mothership_attacker(u, p, first) else { return true };
    team::insert_attack_before(u, t, vec![target], DEFEND_ATTACK_FORMATION);
    false
}

/// `aiuFindNearestMothershipAttacker` (0x41af10): of the attackers dangerous to the mothership
/// (all of them when none is), the nearest.
fn nearest_mothership_attacker(u: &Universe, p: usize, from: ShipId) -> Option<ShipId> {
    let all: Vec<ShipId> = ai(u, p).defense.mothership_attackers.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0)).collect();
    if all.is_empty() {
        return None;
    }
    let mut sel = all.clone();
    only_dangerous_to_mothership(u, &mut sel);
    if sel.is_empty() {
        sel = all;
    }
    let pos = u.ships.get(from)?.posinfo.position;
    let mut best = None;
    let mut best_d = 1e21f32;
    for (i, &id) in sel.iter().enumerate() {
        let q = u.ships.ship(id).posinfo.position;
        let (dx, dy, dz) = (q.x - pos.x, q.y - pos.y, q.z - pos.z);
        let d = st(ld(dx) * ld(dx) + ld(dy) * ld(dy) + ld(dz) * ld(dz));
        if i == 0 || d < best_d {
            best_d = d;
            best = Some(id);
        }
    }
    best
}
