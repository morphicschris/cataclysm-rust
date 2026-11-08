//! The attack manager (`AIAttackMan.c` as Cataclysm has it): attack teams created in free slots
//! with an attack type drawn by its probability (`aiaProcessAttackTeams`,
//! `aiaGenerateAttackType`), teams built round the big ships the player was given
//! (`aiaDivideNewShips`), the attack variable that sends waiting teams in (`aiaTimeAttack`), and
//! the armada's targets (`aiaArmada`, `aiuFindArmadaTarget`). The orders (`AIOrders.c`) give the
//! teams their moves ([`super::team`]).
//!
//! Not yet ported: the target takeouts (types 4 and 5, which go by `statsBestShipsToUseToKillTarget`),
//! harass, capture and mine teams, reconnaissance, and the escort teams of the guard and huge
//! mothership takeouts; those attack types are not generated.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_AI_PLAYER;

use super::team::{self, add_move, AiMove, MoveKind, NO_FORMATION, SAME_FORMATION};
use super::{ai, ai_mut, AiPlayer, Manager};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// Attack slots (`AttackTeam[24]` at +0xc1c).
pub const ATTACK_SLOTS: usize = 24;
/// Attack types the manager draws (`AIA_NUM_ATTACK_TYPES` in a multiplayer game).
const MP_ATTACK_TYPES: u32 = 11;
/// Range of the nearby enemies a finished attack team goes after (0x46ea6000).
const NEARBY_ENEMY_RANGE: f32 = 30000.0;
/// Formation of the attacks given to a finished team (0x740a7c).
const CURRENT_TEAM_ATTACK_FORMATION: i32 = 4;

/// Feature bits (`AttackFeatures`, +0x0c).
pub mod feature {
    pub const RECON: u32 = 0x1;
    pub const ADVANCED_RECON: u32 = 0x2;
    pub const TAKEOUT_MOTHERSHIP_FAST: u32 = 0x10;
    pub const TAKEOUT_MOTHERSHIP_GUARD: u32 = 0x20;
    pub const TAKEOUT_MOTHERSHIP_BIG: u32 = 0x40;
    pub const TAKEOUT_MOTHERSHIP_HUGE: u32 = 0x80;
    pub const TAKEOUT_TARGET: u32 = 0x100;
    pub const FANCY_TAKEOUT_TARGET: u32 = 0x200;
    pub const FIGHTER_STRIKE: u32 = 0x1000;
    pub const CORVETTE_STRIKE: u32 = 0x2000;
    pub const FRIGATE_STRIKE: u32 = 0x4000;
    pub const HARASS: u32 = 0x1_0000;
    pub const CAPTURE: u32 = 0x2_0000;
    pub const MINE: u32 = 0x4_0000;
    pub const SPECIAL_DEFENSE: u32 = 0x10_0000;
    pub const SPECIAL_DEFENSE_2: u32 = 0x20_0000;
}

/// The chance (out of 256) of each attack type by level, as base and spread
/// (`aiaInit`: 0x740888.. and 0x740894..; type 9's at 0x8d3288 is zero in the executable).
/// In the order `aiaInit` draws them (type 10 before type 9).
const PROBABILITIES: [(usize, [i32; 3], [i32; 3]); 15] = [
    (0, [130; 3], [50; 3]),
    (1, [120; 3], [50; 3]),
    (2, [100; 3], [50; 3]),
    (3, [0, 0, 70], [0, 0, 40]),
    (4, [100; 3], [50; 3]),
    (5, [50; 3], [40; 3]),
    (6, [15; 3], [15; 3]),
    (7, [100; 3], [60; 3]),
    (8, [115; 3], [70; 3]),
    (10, [60; 3], [30; 3]),
    (9, [0; 3], [0; 3]),
    (11, [80; 3], [40; 3]),
    (12, [0, 0, 55], [0, 0, 45]),
    (13, [0, 0, 55], [0, 0, 35]),
    (14, [0, 0, 55], [0, 0, 35]),
];
/// Teams in the armada before it looks for targets, by level (0x7409f0).
const ARMADA_THRESHOLD: [u8; 3] = [0, 1, 2];

/// The attack manager's part of `struct AIPlayer`.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AttackState {
    /// +0xc84: the attack variable; teams waiting in a temporary guard go in when it is negative.
    pub var: String,
    /// +0x1cac: when the timed attack comes next.
    pub attack_time: f32,
    /// +0xcb4.. the chance of each attack type.
    pub probabilities: [i32; 15],
    /// +0xcf4.
    pub armada_threshold: u8,
    /// +0xcf8: the armada's targets.
    pub armada_targets: Vec<ShipId>,
    /// +0x1ca0: the player's ships see the armada's targets.
    pub armada_target_seen: bool,
    /// +0xc1c: the attack teams.
    pub teams: Vec<Option<usize>>,
    /// +0x1cd0 (the defence manager's): the square of the radius of the mothership's sphere of
    /// influence, a quarter of the squared distance to the enemy mothership.
    pub sphere_of_influence: f32,
    /// +0xc04.. (count +0xc18): the scouts' teams.
    #[serde(default, deserialize_with = "crate::save::added_in_15")]
    pub recon_teams: Vec<usize>,
    /// +0x1ca4: the scout sent to look at the armada's targets.
    #[serde(default, deserialize_with = "crate::save::added_in_15")]
    pub recon_target_team: Option<usize>,
}

fn rnd(u: &mut Universe) -> u32 {
    u.randy.random(RAN_AI_PLAYER)
}

/// Whether a type is the race's and loaded (`aiaInit`'s test).
fn race_has(u: &Universe, race: ShipRace, t: ShipType) -> bool {
    crate::research::race_types(race).contains(&t.index())
        && u.statics.ship(t).is_some_and(|st| st.info_needed && (st.ship_class != Some(ShipClass::Resource) || u.single_player))
}

/// `aiaInit` (0x402010): the attack variable, the time of the first timed attack, the chances
/// of the attack types and the features by level.
pub fn init(u: &mut Universe, p: usize) {
    let level = ai(u, p).level.min(2) as usize;
    let race = u.players[p].race;
    let v = rnd(u) % 3 + 2;
    let var = team::var_create(u, v as i32);
    let r = rnd(u);
    let attack_time = st(ld(r as f32) * ld(f32::from_bits(0x3448_0000)) + 200.0);
    let mut probabilities = [0i32; 15];
    for &(k, base, dev) in &PROBABILITIES {
        let r = rnd(u);
        probabilities[k] = (r % (dev[level] as u32 * 2 + 1)) as i32 + base[level] - dev[level];
    }
    let mut f = 0u32;
    if level == 2 {
        f |= feature::ADVANCED_RECON;
        let big = [ShipType::kHeavyCruiser, ShipType::tHeavyCruiser, ShipType::bHeavyCruiser, ShipType::sDreadnought];
        if big.iter().any(|&t| race_has(u, race, t)) {
            f |= feature::TAKEOUT_MOTHERSHIP_HUGE;
        }
        f |= feature::FANCY_TAKEOUT_TARGET | feature::FIGHTER_STRIKE | feature::CORVETTE_STRIKE | feature::HARASS;
        if race_has(u, race, ShipType::kSalCapCorvette) || race_has(u, race, ShipType::tSalCapCorvette) {
            f |= feature::CAPTURE;
        }
        if race_has(u, race, ShipType::tMinelayerCorvette) {
            f |= feature::MINE;
        }
        if race_has(u, race, ShipType::kCloakGenerator) || race_has(u, race, ShipType::tCloakGenerator) {
            f |= feature::SPECIAL_DEFENSE;
        }
        if race_has(u, race, ShipType::kGravWellGenerator) || race_has(u, race, ShipType::tGravWellGenerator) {
            f |= feature::SPECIAL_DEFENSE_2;
        }
    }
    if level != 0 {
        f |= feature::RECON | feature::TAKEOUT_MOTHERSHIP_FAST | feature::TAKEOUT_TARGET | feature::FRIGATE_STRIKE;
    }
    f |= feature::TAKEOUT_MOTHERSHIP_GUARD | feature::TAKEOUT_MOTHERSHIP_BIG;
    let dev = [0u32, 0, 20][level];
    let base = [0i32, 0, 25][level];
    let r = rnd(u);
    let chance = (r % (dev * 2 + 1)) as i32 + base - dev as i32;
    let r = rnd(u);
    if ((r % 101) as i32) < chance {
        f |= 0x8_0000;
    }
    let a = ai_mut(u, p);
    a.attack_features = f;
    a.attack = AttackState {
        var,
        attack_time,
        probabilities,
        armada_threshold: ARMADA_THRESHOLD[level],
        armada_targets: Vec::new(),
        armada_target_seen: false,
        teams: vec![None; ATTACK_SLOTS],
        sphere_of_influence: 0.0,
        recon_teams: Vec::new(),
        recon_target_team: None,
    };
}

/// `aiaTeamDied`: the team leaves its slot.
pub fn team_died(a: &mut AiPlayer, t: usize) {
    a.attack.recon_teams.retain(|&x| x != t);
    if a.attack.recon_target_team == Some(t) {
        a.attack.recon_target_team = None;
    }
    for s in &mut a.attack.teams {
        if *s == Some(t) {
            *s = None;
        }
    }
}

/// `aiaShipDied`: the ship leaves the armada's targets.
pub fn ship_died(a: &mut AiPlayer, id: ShipId) {
    a.attack.armada_targets.retain(|&s| s != id);
}

/// `aiuFindEnemyMothership` (0x41e8b0): the primary enemy's mothership (which may be gone).
pub fn find_enemy_mothership(u: &Universe, p: usize) -> Option<ShipId> {
    let enemy = match u.ai.players.get(p).and_then(|a| a.as_ref()) {
        Some(a) => a.primary_enemy,
        None => (0..u.base.players.len()).find(|&q| q != p)?,
    };
    u.base.players.get(enemy)?.mothership.filter(|&m| u.ships.get(m).is_some())
}

/// `aiaAttackManager` (0x401840).
pub fn manager(u: &mut Universe, p: usize) {
    for t in ai(u, p).attack.teams.clone().into_iter().flatten() {
        let empty = u.ai.teams.get(t).and_then(|x| x.as_ref()).is_none_or(|x| x.1.ships.is_empty());
        if team::team_is_done(u, t) && empty {
            team::destroy(u, t);
        }
    }
    let divided = !ai(u, p).new_ships.is_empty() && divide_new_ships(u, p);
    super::recon::more_scouts(u, p);
    if ai(u, p).attack_features != 0 && !divided {
        process_attack_teams(u, p);
    }
    time_attack(u, p);
}

/// `aiaProcessAttackTeams` (0x401a90): every free slot gets a new team (while few teams wait
/// for ships) with an attack type drawn by the chances; a team that has finished goes after
/// the enemies near it, or the enemy mothership.
fn process_attack_teams(u: &mut Universe, p: usize) {
    for i in 0..ATTACK_SLOTS {
        match ai(u, p).attack.teams[i] {
            None => {
                let waiting = team::count_teams_waiting_for_ships(u, p, Some(Manager::Attack));
                if !(waiting < 4 || (waiting < 10 && u.base.players[p].resource_units > 12000)) {
                    continue;
                }
                let t = super::team_create(u, p, Manager::Attack, "");
                ai_mut(u, p).attack.teams[i] = Some(t);
                let mut misses = 0;
                while misses <= 99 {
                    let k = rnd(u) % MP_ATTACK_TYPES;
                    let r = rnd(u);
                    if ((r & 0xff) as i32) < ai(u, p).attack.probabilities[k as usize] {
                        if generate_attack_type(u, p, t, k, false) {
                            break;
                        }
                    } else {
                        misses += 1;
                    }
                }
            }
            Some(t) => {
                if !team::team_is_done(u, t) {
                    continue;
                }
                let ships = u.ai.teams[t].as_ref().map(|x| x.1.ships.clone()).unwrap_or_default();
                let Some(&first) = ships.first() else {
                    team::destroy(u, t);
                    ai_mut(u, p).attack.teams[i] = None;
                    continue;
                };
                let mut near = find_nearby_enemy_ships(u, p, first, NEARBY_ENEMY_RANGE);
                near.retain(|&id| !is_enemy_mothership(u, p, id));
                if near.is_empty() {
                    if let Some(ms) = find_enemy_mothership(u, p) {
                        team::delete_all_moves(u, t);
                        takeout_targets_with_current_team(u, t, vec![ms]);
                    }
                } else {
                    team::delete_all_moves(u, t);
                    takeout_targets_with_current_team(u, t, near);
                }
            }
        }
    }
    armada(u, p);
}

/// `aiuShipIsntAnEnemyMothership`'s opposite.
fn is_enemy_mothership(u: &Universe, p: usize, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    u.base.players.get(s.player).is_some_and(|pl| pl.mothership == Some(id)) && !u.players[s.player].is_ally(p)
}

pub fn is_enemy(u: &Universe, p: usize, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.player != p && s.player < u.players.len() && !u.players[s.player].is_ally(p) && s.flags & sof::DEAD == 0)
}

/// `aiuFindNearbyEnemyShips` (0x41dbe0): the enemy ships within `range` of the ship.
pub fn find_nearby_enemy_ships(u: &Universe, p: usize, id: ShipId, range: f32) -> Vec<ShipId> {
    let Some(pos) = u.ships.get(id).map(|s| s.posinfo.position) else { return Vec::new() };
    let r2 = ld(range) * ld(range);
    u.ship_list
        .iter()
        .copied()
        .filter(|&s| s != id && is_enemy(u, p, s))
        .filter(|&s| u.ships.get(s).is_some_and(|x| x.flags & sof::HIDE == 0 && crate::level::distance_squared(x.posinfo.position, pos) < r2))
        .collect()
}

/// `aioCreateTakeoutTargetsWithCurrentTeam` / `...TargetWithCurrentTeam`.
fn takeout_targets_with_current_team(u: &mut Universe, t: usize, targets: Vec<ShipId>) {
    add_move(u, t, AiMove::new(MoveKind::Attack { targets }, CURRENT_TEAM_ATTACK_FORMATION, 2, true, false));
    add_move(u, t, done());
}

/// `aiaDivideNewShips` (0x401b90): big ships the player was given make a mothership takeout of
/// their own (huge with a Dreadnought and a Destroyer or a Beast Heavy Cruiser and Ion Array
/// Frigate, big with a Multi-Beam or Hive Frigate and a Ramming Frigate).
fn divide_new_ships(u: &mut Universe, p: usize) -> bool {
    let mut count = [0u32; crate::base::NUM_SHIP_TYPES];
    for &id in &ai(u, p).new_ships {
        if let Some(s) = u.ships.get(id) {
            count[s.ship_type.index()] += 1;
        }
    }
    let n = |t: ShipType| count[t.index()];
    let kind = if n(ShipType::sDreadnought) != 0 && n(ShipType::sDestroyer) != 0 {
        3
    } else if (n(ShipType::sMultiBeamFrigate) != 0 || n(ShipType::sHiveFrigate) != 0) && n(ShipType::sRammingFrigate) != 0 {
        2
    } else if n(ShipType::bHeavyCruiser) != 0 && n(ShipType::bIonArrayFrigate) != 0 {
        3
    } else {
        // (The HW1 races' combinations are for the single-player game.)
        return false;
    };
    let Some(slot) = ai(u, p).attack.teams.iter().position(Option::is_none) else { return false };
    let t = super::team_create(u, p, Manager::Attack, "");
    ai_mut(u, p).attack.teams[slot] = Some(t);
    if generate_attack_type(u, p, t, kind, true) {
        return true;
    }
    team::destroy(u, t);
    false
}

/// `aiaGenerateAttackType` (0x401480): the order for attack type `k` when the player has its
/// feature (and an enemy mothership for the takeouts). False when it was not given.
fn generate_attack_type(u: &mut Universe, p: usize, t: usize, k: u32, force: bool) -> bool {
    let f = ai(u, p).attack_features;
    let enemy_ms = find_enemy_mothership(u, p);
    match k {
        0 if f & feature::TAKEOUT_MOTHERSHIP_FAST != 0 && enemy_ms.is_some() => takeout_mothership_fast(u, p, t),
        1 if f & feature::TAKEOUT_MOTHERSHIP_GUARD != 0 && enemy_ms.is_some() => takeout_mothership_guard(u, p, t),
        2 if f & feature::TAKEOUT_MOTHERSHIP_BIG != 0 && enemy_ms.is_some() => takeout_mothership_big(u, p, t, force),
        3 if f & feature::TAKEOUT_MOTHERSHIP_HUGE != 0 => {
            let own_ms_class = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).and_then(|s| u.statics.ship(s.ship_type)).and_then(|s| s.ship_class);
            if !(own_ms_class == Some(ShipClass::Mothership) || force) || enemy_ms.is_none() {
                return false;
            }
            takeout_mothership_huge(u, p, t);
        }
        6 if f & feature::FIGHTER_STRIKE != 0 => fighter_strike(u, p, t),
        7 if f & feature::CORVETTE_STRIKE != 0 => corvette_strike(u, p, t),
        8 if f & feature::FRIGATE_STRIKE != 0 => frigate_strike(u, p, t),
        // The special defence teams are the HW1 races' (cloak and gravity well generators).
        13 => return f & feature::SPECIAL_DEFENSE != 0,
        14 => return f & feature::SPECIAL_DEFENSE_2 != 0,
        _ => return false,
    }
    true
}

// ---- the orders (AIOrders.c) -----------------------------------------------------------------

fn done() -> AiMove {
    AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false)
}

/// `aimCreateFancyGetShips` with the alternatives given as (type number, worth in tenths).
pub(super) fn fancy_get_ships(u: &mut Universe, t: usize, primary: u8, num: i32, alternatives: &[(u8, i32)], random: bool, priority: u32) {
    let ty = |n: u8| ShipType::from_index(n.into()).expect("ship type");
    let alternatives = alternatives.iter().map(|&(n, r)| (ty(n), r)).collect();
    add_move(
        u,
        t,
        AiMove::new(MoveKind::FancyGetShips { primary: ty(primary), num, alternatives, random_alternative: random, priority, var: None }, SAME_FORMATION, 2, true, false),
    );
}

/// The end every attack order has: the attack variable counted down, a temporary guard, the
/// armada and done.
fn guard_and_armada(u: &mut Universe, p: usize, t: usize, guard_formation: i32, armada_formation: i32, hyperspace: bool) {
    let var = ai(u, p).attack.var.clone();
    add_move(u, t, AiMove::new(MoveKind::VarDec { var }, SAME_FORMATION, 2, true, false));
    add_move(u, t, AiMove::new(MoveKind::TempGuard, guard_formation, 2, true, false));
    add_move(u, t, AiMove::new(MoveKind::Armada, armada_formation, 2, true, false));
    add_move(u, t, done());
    if hyperspace {
        if let Some(Some((_, tm))) = u.ai.teams.get_mut(t) {
            tm.flags |= team::team_flag::HYPERSPACE;
        }
    }
}

/// Whether the Beast player has acquired the type (`player->beastAcquired[type]`).
fn acquired(u: &Universe, p: usize, t: u8) -> bool {
    ShipType::from_index(t.into()).is_some_and(|ty| super::research::ship_type_acquired(u, p, ty))
}

/// `aioCreateTakeoutMothershipFast` (0x40ed50): three Multi-Beam Frigates (Somtaaw) or Ion Array
/// Frigates (Beast), or what stands in for them.
fn takeout_mothership_fast(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => fancy_get_ships(u, t, 7, 3, &[(7, 10), (7, 10), (7, 10), (7, 30), (10, 30), (25, 30), (8, 10), (8, 10), (4, 7), (5, 3), (3, 3)], true, 0),
        ShipRace::Beast => {
            let mb = acquired(u, p, 0x29);
            let (a, b, c, d) = if mb { (0x29, 0x29, 0x29, 0x29) } else { (0x2c, 0x2c, 0x2c, 0x2e) };
            let hive = if acquired(u, p, 0x2a) { 0x2a } else { 0x3b };
            let acv = if acquired(u, p, 0x26) { 0x26 } else { 0x25 };
            let alts = [
                (0x2c, 10),
                (0x2c, 10),
                (0x2c, 10),
                (0x2c, 30),
                (0x2e, 30),
                (0x2e, 30),
                (0x3b, 10),
                (0x3b, 10),
                (0x25, 7),
                (0x28, 3),
                (0x27, 3),
                (a, 10),
                (b, 10),
                (c, 10),
                (d, 30),
                (hive, 10),
                (hive, 10),
                (acv, 7),
            ];
            fancy_get_ships(u, t, 0x2c, 3, &alts, true, 0);
        }
        _ => {}
    }
    guard_and_armada(u, p, t, 1, 1, true);
}

/// `aioCreateTakeoutMothershipGuard` (0x40ffd0), the attacking team: three ships by the kind of
/// mothership the player has. (Its escort team is not ported yet.)
fn takeout_mothership_guard(u: &mut Universe, p: usize, t: usize) {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.ship_type) else { return };
    match ms {
        ShipType::sMothership => fancy_get_ships(u, t, 6, 3, &[(10, 30), (25, 30), (8, 30), (7, 10), (4, 7), (4, 7)], true, 0),
        ShipType::sCarrier => fancy_get_ships(u, t, 7, 3, &[(1, 30), (2, 30), (3, 10), (4, 7), (5, 7)], true, 0),
        ShipType::bMothership => {
            let hive = if acquired(u, p, 0x2a) { 0x2a } else { 0x3b };
            let mb = if acquired(u, p, 0x29) { 0x29 } else { 0x2c };
            let (a, b) = if acquired(u, p, 0x26) { (0x26, 0x26) } else { (0x27, 0x3a) };
            fancy_get_ships(u, t, 0x2c, 3, &[(0x3a, 30), (0x2e, 30), (0x3b, 10), (0x27, 7), (0x28, 7), (hive, 30), (mb, 10), (a, 7), (b, 7)], true, 0);
        }
        ShipType::bCarrier => {
            let hive = if acquired(u, p, 0x2a) { 0x2a } else { 0x3b };
            let mb = if acquired(u, p, 0x29) { 0x29 } else { 0x2c };
            let (a, b) = if acquired(u, p, 0x26) { (0x26, 0x26) } else { (0x27, 0x25) };
            fancy_get_ships(u, t, 0x25, 3, &[(0x26, 10), (0x27, 7), (0x28, 7), (hive, 30), (mb, 10), (a, 7), (b, 7)], true, 0);
        }
        _ => return,
    }
    guard_and_armada(u, p, t, 1, 4, false);
}

/// `aioCreateTakeoutMothershipBig` (0x40f590): two frigates with two more (asked for first),
/// heavier from a mothership than from a carrier.
fn takeout_mothership_big(u: &mut Universe, p: usize, t: usize, force: bool) {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.ship_type) else { return };
    let (first, second, heavy, other, ms_type, carrier) = match u.players[p].race {
        ShipRace::Somtaaw => (7u8, 6u8, 10u8, 0x19u8, ShipType::sMothership, ShipType::sCarrier),
        ShipRace::Beast => (0x2c, 0x27, 0x2e, 0x28, ShipType::bMothership, ShipType::bCarrier),
        _ => return,
    };
    if ms == ms_type || force {
        fancy_get_ships(u, t, first, 2, &[(other, 30), (heavy, 30)], true, 0);
        fancy_get_ships(u, t, second, 2, &[(heavy, 30), (first, 30)], true, 1);
    } else if ms == carrier {
        fancy_get_ships(u, t, first, 2, &[(second, 10)], true, 0);
        fancy_get_ships(u, t, second, 2, &[(first, 10)], true, 1);
    } else {
        return;
    }
    guard_and_armada(u, p, t, 1, 4, true);
}

/// `aioCreateTakeoutMothershipHuge` (0x40fc90), the attacking team: a Dreadnought or a Beast
/// Heavy Cruiser. (Its escort team is not ported yet.)
fn takeout_mothership_huge(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => fancy_get_ships(u, t, 10, 1, &[], false, 0),
        ShipRace::Beast => fancy_get_ships(u, t, 0x2e, 1, &[], false, 0),
        _ => {}
    }
    guard_and_armada(u, p, t, NO_FORMATION, NO_FORMATION, true);
}

/// `aioCreateFighterStrike` (0x40ee90): thirteen Acolytes or fifteen Beast Attack Bombers.
fn fighter_strike(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => fancy_get_ships(u, t, 1, 13, &[(1, 12), (0, 20), (4, 7), (4, 7), (5, 10), (3, 10), (5, 10), (2, 10), (2, 10)], true, 0),
        ShipRace::Beast => {
            let x = if acquired(u, p, 0x22) { 0x22 } else { 0x23 };
            let (a, b) = if acquired(u, p, 0x26) { (0x26, 0x26) } else { (0x27, 0x25) };
            let alts = [(0x23, 12), (0x27, 7), (0x3b, 7), (0x3a, 7), (0x21, 20), (0x25, 10), (0x25, 10), (0x28, 8), (0x24, 10), (x, 12), (a, 7), (b, 7)];
            fancy_get_ships(u, t, 0x25, 15, &alts, true, 0);
        }
        _ => {}
    }
    guard_and_armada(u, p, t, 0, 3, false);
}

/// `aioCreateCorvetteStrike` (0x40f0d0): nine ACVs or Beast Heavy Corvettes.
fn corvette_strike(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => fancy_get_ships(u, t, 4, 9, &[(1, 13), (4, 10), (4, 10), (4, 10), (4, 10), (5, 7), (3, 7)], true, 0),
        ShipRace::Beast => {
            let (a, b, c, d) = if acquired(u, p, 0x26) { (0x26, 0x26, 0x26, 0x26) } else { (0x28, 0x27, 0x25, 0x25) };
            let alts = [(0x23, 13), (0x28, 10), (0x28, 10), (0x28, 10), (0x28, 10), (0x27, 7), (0x25, 7), (0x22, 13), (a, 10), (b, 10), (c, 10), (d, 10)];
            fancy_get_ships(u, t, 0x28, 9, &alts, true, 0);
        }
        _ => {}
    }
    guard_and_armada(u, p, t, 2, 4, false);
}

/// `aioCreateFrigateStrike` (0x40f3a0): five Hive Frigates or Beast Ion Array Frigates.
fn frigate_strike(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => fancy_get_ships(u, t, 8, 5, &[(7, 10)], true, 0),
        ShipRace::Beast => {
            let mb = if acquired(u, p, 0x29) { 0x29 } else { 0x2c };
            let hive = if acquired(u, p, 0x2a) { 0x2a } else { 0x2c };
            fancy_get_ships(u, t, 0x2c, 5, &[(0x2c, 10), (mb, 10), (hive, 10)], true, 0);
        }
        _ => {}
    }
    guard_and_armada(u, p, t, 4, 3, true);
}

// ---- timing and the armada --------------------------------------------------------------------

/// `aiaTimeAttack` (0x401e00): a beginner attacks at once. Otherwise every so often (800 to
/// 1000 seconds) the attack variable goes to -1, sending the waiting teams in; once more teams
/// have counted it down it sinks by 100 a turn, and below -300 it starts again from 2 to 4.
/// With no RUs left to harvest the player attacks at once.
fn time_attack(u: &mut Universe, p: usize) {
    let level = ai(u, p).level;
    let var = ai(u, p).attack.var.clone();
    let value = match level {
        0 => -1,
        1 | 2 => {
            if ai(u, p).rus_in_world == 0 {
                for t in super::teams_with_flag(u, p, team::team_flag::TEMP_GUARDING) {
                    team::delete_moves_until(u, t, team::move_type::TEMP_GUARD);
                }
                -1
            } else if ai(u, p).attack.attack_time < u.total_time_elapsed {
                team::var_set(u, &var, -1);
                let r = rnd(u);
                let a = ai_mut(u, p);
                a.attack.attack_time = st(ld(r as f32) * ld(-f32::from_bits(0x3348_0000)) + 1000.0 + ld(a.attack.attack_time));
                return;
            } else {
                let v = team::var_get(u, &var);
                if v < -300 && ai(u, p).rus_in_world != 0 {
                    (rnd(u) % 3 + 2) as i32
                } else if v >= -1 {
                    return;
                } else {
                    for t in super::teams_with_flag(u, p, team::team_flag::TEMP_GUARDING) {
                        team::delete_moves_until(u, t, team::move_type::TEMP_GUARD);
                    }
                    v - 100
                }
            }
        }
        _ => return,
    };
    team::var_set(u, &var, value);
}

/// `aiaArmada` (0x401fa0): with more teams in the armada than the level's threshold and no
/// targets, the armada looks for some.
fn armada(u: &mut Universe, p: usize) {
    let n = team::num_teams_with_flag(u, p, team::team_flag::ARMADA);
    let a = &ai(u, p).attack;
    if usize::from(a.armada_threshold) >= n {
        return;
    }
    if a.armada_targets.is_empty() {
        let (targets, seen) = find_armada_target(u, p);
        let a = ai_mut(u, p);
        a.attack.armada_targets = targets;
        a.attack.armada_target_seen = seen;
    }
    // A scout goes to look at targets the player's ships do not see.
    let a = &ai(u, p).attack;
    if !a.armada_targets.is_empty() && !a.armada_target_seen && a.recon_target_team.is_none() {
        let targets = a.armada_targets.clone();
        let t = super::recon::send_recon(u, p, &targets);
        ai_mut(u, p).attack.recon_target_team = t;
    }
}

/// `aiuRateShip`'s value of a ship: a mothership 5500, a Somtaaw or Beast ship its cost, any
/// other 1. (Its strength is `statsGetOverallKillRating`, which is 0 for every ship: the game's
/// `statfile.bin` is HW1's, of the wrong size, and `statsLoadFightStats` clears the table.)
fn ship_value(u: &Universe, id: ShipId) -> i32 {
    let Some(s) = u.ships.get(id) else { return 0 };
    let Some(st) = u.statics.ship(s.ship_type) else { return 0 };
    if st.ship_class == Some(ShipClass::Mothership) || s.ship_type == ShipType::trFloatingCity {
        0x157c
    } else if matches!(st.ship_race, ShipRace::Somtaaw | ShipRace::Beast) {
        st.build_cost
    } else {
        1
    }
}

/// `aiuFindArmadaTarget` (0x41e8e0) over the blobs with visible enemy ships
/// (`aiuCreateBlobArrays`, `aiuRateBlobs`): the blob with the most of the primary enemy's value
/// (a third either way at random when the player has no ship there), unless the enemy's
/// mothership is in sight, which is always preferred (its strength being 0). Returns the
/// targets and whether the player's ships are among them.
/// A blob with visible enemy ships, rated for the player (`aiuCreateBlobArrays`,
/// `aiuRateBlobs`): its ships (less cloaked enemies the player cannot see), the value of the
/// primary enemy's ships in it, whether the player has ships there and whether a mothership is.
pub struct EnemyBlob {
    pub centre: hwc_math::Vector,
    pub radius: f32,
    pub ships: Vec<ShipId>,
    pub value: i32,
    pub own: bool,
    pub mothership: bool,
}

/// The enemy blobs (`aiuFillInArrays`, `aiuFillInAIBlobArrays`, `aiuRateBlobs`).
pub fn enemy_blobs(u: &Universe, p: usize) -> Vec<EnemyBlob> {
    let enemy = ai(u, p).primary_enemy;
    let mut blobs = Vec::new();
    for b in &u.blobs.blobs {
        let visible_enemy = b.ships.iter().any(|&id| {
            u.ships.get(id).is_some_and(|s| {
                s.player != p
                    && !(crate::cloak::is_cloaked(s) && !crate::visibility::visible_to_player(u, id, p))
                    && crate::fow::ship_visible_to_player(u, id, p)
                    && !matches!(s.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1)
            })
        });
        if !visible_enemy {
            continue;
        }
        let ships: Vec<ShipId> = b.ships.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player == p || !crate::cloak::is_cloaked(s) || crate::visibility::visible_to_player(u, id, p))).collect();
        let mut r = EnemyBlob { centre: b.centre, radius: b.radius, ships, value: 0, own: false, mothership: false };
        for &id in &r.ships {
            let s = u.ships.ship(id);
            let mut v = ship_value(u, id);
            if u.base.players.get(s.player).is_some_and(|pl| pl.mothership == Some(id)) {
                r.mothership = true;
                v = (f64::from(v) * 1.5) as i32;
            }
            if s.player == p {
                r.own = true;
            } else if s.player == enemy {
                r.value += v;
            }
        }
        blobs.push(r);
    }
    blobs
}

fn find_armada_target(u: &mut Universe, p: usize) -> (Vec<ShipId>, bool) {
    let blobs = enemy_blobs(u, p);
    let mut best: Option<usize> = None;
    let mut best_score = 0.0f32;
    let mut mothership_blob: Option<usize> = None;
    let single = blobs.len() == 1;
    for (i, b) in blobs.iter().enumerate() {
        if b.mothership && b.value >= 0x157d {
            mothership_blob = Some(i);
            if single {
                best = Some(i);
            }
            continue;
        }
        let mut score = b.value as f32;
        if !b.own {
            let r = rnd(u);
            let s = ld(score);
            score = st((s * f64::from(1.0f32 / 3.0) - s * -f64::from(1.0f32 / 3.0)) * ld(f32::from_bits(0x2f80_0000)) * ld(r as f32) + s * -f64::from(1.0f32 / 3.0) + s);
        }
        if best_score < score {
            best_score = score;
            best = Some(i);
        }
    }
    // The mothership blob's score is its value over a strength of 0: infinite.
    let mothership_score = if mothership_blob.is_some() { f32::INFINITY } else { 0.0 };
    if mothership_score < best_score * 1.5 {
        let b = &blobs[best.expect("best blob")];
        let targets = b.ships.iter().copied().filter(|&id| is_enemy(u, p, id)).collect();
        return (targets, b.own);
    }
    let enemy = ai(u, p).primary_enemy;
    let enemy_ms = u.base.players.get(enemy).and_then(|pl| pl.mothership).filter(|&m| u.ships.get(m).is_some_and(|s| s.flags & sof::DEAD == 0));
    if let (Some(ms), Some(i)) = (enemy_ms, mothership_blob) {
        return (vec![ms], blobs[i].own);
    }
    (Vec::new(), false)
}
