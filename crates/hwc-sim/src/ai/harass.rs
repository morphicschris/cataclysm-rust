//! Harassment (`aioCreateHarass`, `aimProcessHarassAttack`, `aiuFindHarassTarget` and the
//! handlers `aihHarassNumbersLowHandler`, `aihHarassTeamDiedHandler`,
//! `aihGenericGettingRockedHandler`, `aihHarassFiringSingleShipHandler`,
//! `aihHarassDisengageSingleShipHandler`, `aihKamikazeHealthLowHandler`): a team of fighters
//! that goes out near the enemy mothership and picks off unarmed, undefended or fighter-prey
//! ships, fights back when hit by fewer ships than it has, goes home to be topped up when it has
//! lost too many, and is made again (three times in four) when it is wiped out.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::RAN_AI_PLAYER;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use super::team::{self, add_move, team_flag, AiMove, Event, Handler, MoveKind, SAME_FORMATION};
use super::{ai, ai_mut};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// `MOVE_HARASSATTACK`, `MOVE_ADVANCEDATTACK`.
pub mod move_type {
    pub const ADVANCED_ATTACK: u32 = 0x12;
    pub const HARASS_ATTACK: u32 = 0x15;
}

/// Team flags of a harass team.
pub mod flag {
    /// `aioCreateHarass`: the team harasses.
    pub const HARASS: u32 = 0x4;
    /// The team has died once, twice (`aihHarassTeamDiedHandler`).
    pub const DIED_ONCE: u32 = 0x20;
    pub const DIED_TWICE: u32 = 0x40;
    /// The team is attacking its harass target.
    pub const ATTACKING: u32 = 0x1000;
    pub const LOOKING: u32 = 0x8000;
}

/// The share of the team that may be lost before it goes home (0x740a44, per cent).
const NUMBERS_LOW: i32 = 40;
/// The fighters and corvettes of a kamikaze player ram when below this health (0x740a94).
const KAMIKAZE_HEALTH: i32 = 45;
/// `aihGenericGettingRockedHandler`: the team fights back only when it has this many times the
/// ships hitting it (0x740a9c), in this formation (0x740aa0).
const ROCKED_STRENGTH: f32 = 1.5;
const ROCKED_FORMATION: i32 = 1;
/// `aimProcessHarassAttack`: farther than this (squared, 0x740ab4) the team first flies to a
/// point this far short of the target (0x740ab8) in formation 0x740ac0; enemy defenders within
/// 0x740abc of the target are attacked first, in formation 0x740ac4 (one) or 0x740ac8 (more).
const APPROACH_SQ: f32 = 4_000_000.0;
const APPROACH_STANDOFF: f32 = 16000.0;
const APPROACH_FORMATION: i32 = 2;
const DEFENDERS_RANGE: f32 = 25000.0;
const ATTACK_FORMATION_ONE: i32 = 5;
const ATTACK_FORMATION_MORE: i32 = 3;
/// `aihHarassFiringSingleShipHandler`'s formation (0x740acc).
const FIRING_FORMATION: i32 = 5;
/// `aihHarassNumbersLowHandler`: home to this short of the mothership (0x740ad0), in formation
/// 0x740ad4.
const HOME_STANDOFF: f32 = 7500.0;
const HOME_FORMATION: i32 = 1;
/// `aioCreateHarass`: the standoff from the enemy mothership, and the move's formation (the
/// table entry at 0x8d32a0, 0 in the executable).
const OUT_STANDOFF: f32 = 25000.0;
const OUT_FORMATION: i32 = 0;
/// `aiuFindHarassTarget`: the targets' ranges and weights (0x740b88..0x740b9c).
const TARGET_STRENGTH: f32 = 2.0;
const TARGET_RANGE: f32 = 15000.0;
const UNDEFENDED_WEIGHT: f32 = 1.0;
const UNARMED_WEIGHT: f32 = 1.2;
const FIGHTER_PREY_WEIGHT: f32 = 0.8;
/// `aiuFindRangeStandoffPoint`'s spread: an angle about z (degrees, 0x740b48, 0x740b4c) and a
/// scale of the distance (0x740b50, 0x740b54).
const STANDOFF_ANGLE: (f32, f32) = (-10.0, 10.0);
const STANDOFF_SCALE: (f32, f32) = (0.95, 1.05);

fn rnd(u: &mut Universe) -> u32 {
    u.randy.random(RAN_AI_PLAYER)
}

fn ty(n: u8) -> ShipType {
    ShipType::from_index(n.into()).expect("ship type")
}

fn fancy(primary: u8, num: i32, alternatives: &[(u8, i32)], wait: bool, remove: bool) -> AiMove {
    let alternatives = alternatives.iter().map(|&(n, r)| (ty(n), r)).collect();
    AiMove::new(MoveKind::FancyGetShips { primary: ty(primary), num, alternatives, random_alternative: true, priority: 0, var: None }, SAME_FORMATION, 2, wait, remove)
}

fn event(handler: Handler, once: bool) -> Option<Event> {
    Some(Event { handler, once, fired: false, percent: 0, base: 0, per_ship: false })
}

/// `aiuFindRangeStandoffPoint` (0x41a300): a point on the way from `from` to `target`, `range`
/// short of it (scaled a little at random) and turned a little about z (the target itself when
/// it is within `range`).
pub fn range_standoff_point(u: &mut Universe, target: Vector, from: Vector, range: f32) -> Vector {
    let lo = st(ld(STANDOFF_ANGLE.0) * ld(0.017_453_292f32));
    let r = rnd(u);
    let angle = st((ld(STANDOFF_ANGLE.1) * ld(0.017_453_292f32) - ld(lo)) * ld(2.328_306_4e-10f32) * fild_u32(r) + ld(lo));
    let r = rnd(u);
    let scale = st((ld(STANDOFF_SCALE.1) - ld(STANDOFF_SCALE.0)) * ld(2.328_306_4e-10f32) * fild_u32(r) + ld(STANDOFF_SCALE.0));
    let v = [st(ld(target.x) - ld(from.x)), st(ld(target.y) - ld(from.y)), st(ld(target.z) - ld(from.z))];
    let len = fsqrt(ld(v[0]) * ld(v[0]) + ld(v[1]) * ld(v[1]) + ld(v[2]) * ld(v[2]));
    if len - ld(range) <= 0.0 {
        return target;
    }
    let want = st((len - ld(range)) * ld(scale));
    let k = ld(want) / len;
    let v = Vector::new(st(ld(v[0]) * k), st(ld(v[1]) * k), st(ld(v[2]) * k));
    let m = Matrix::rot_about_z(st(fcos(ld(angle))), st(fsin(ld(angle))));
    let w = m.mul_vec(v);
    Vector::new(st(ld(w.x) + ld(from.x)), st(ld(w.y) + ld(from.y)), st(ld(w.z) + ld(from.z)))
}

/// `aioCreateHarass` (0x40e750).
pub fn create(u: &mut Universe, p: usize, t: usize) {
    if let Some(Some((_, tm))) = u.ai.teams.get_mut(t) {
        tm.flags |= flag::HARASS;
    }
    let ms = super::recon::enemy_mothership_coords(u, p);
    let dest = range_standoff_point(u, ms, Vector::ZERO, OUT_STANDOFF);
    match u.players[p].race {
        ShipRace::Somtaaw => add_move(u, t, fancy(1, 15, &[(4, 14), (3, 14), (2, 10)], true, false)),
        ShipRace::Beast => {
            let x = if super::research::ship_type_acquired(u, p, ty(0x26)) { 0x26 } else { 0x25 };
            add_move(u, t, fancy(0x23, 15, &[(0x27, 14), (0x25, 14), (0x24, 10), (x, 14)], true, false));
        }
        _ => {}
    }
    let handlers = |m: &mut AiMove| {
        m.events.numbers_low = Some(Event { handler: Handler::HarassNumbersLow, once: true, fired: false, percent: NUMBERS_LOW, base: 0, per_ship: false });
        m.events.getting_rocked = event(Handler::GenericGettingRocked, false);
        m.team_died = Some(Handler::Harass);
    };
    let mut m = AiMove::new(MoveKind::Move { destination: dest }, OUT_FORMATION, 2, true, false);
    handlers(&mut m);
    add_move(u, t, m);
    let mut m = AiMove::new(MoveKind::HarassAttack { target: None }, SAME_FORMATION, 2, true, false);
    handlers(&mut m);
    add_move(u, t, m);
    add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aiaProcessSpecialTeams`' harass team, by its chance.
pub fn special_team(u: &mut Universe, p: usize) {
    if ai(u, p).attack.harass_team.is_some() || ai(u, p).attack_features & super::attack::feature::HARASS == 0 {
        return;
    }
    let r = rnd(u);
    if ((r & 0xff) as i32) < ai(u, p).attack.probabilities[10] {
        let t = super::team_create(u, p, super::Manager::Attack, "");
        ai_mut(u, p).attack.harass_team = Some(t);
        create(u, p, t);
    }
}

fn team_ships(u: &Universe, t: usize) -> Vec<ShipId> {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).map(|x| x.1.ships.clone()).unwrap_or_default()
}

fn team_player(u: &Universe, t: usize) -> usize {
    u.ai.teams[t].as_ref().expect("team").0
}

fn dist_sq(a: Vector, b: Vector) -> f32 {
    let (x, y, z) = (a.x - b.x, a.y - b.y, a.z - b.z);
    x * x + y * y + z * z
}

/// `aiuGetClosestShip` (0x41a7b0) as the original has it: the last ship nearer than the first.
fn closest_ship(u: &Universe, ships: &[ShipId], to: Vector) -> Option<ShipId> {
    let (&first, rest) = ships.split_first()?;
    let d0 = dist_sq(to, u.ships.get(first)?.posinfo.position);
    let mut best = first;
    for &s in rest {
        if u.ships.get(s).is_some_and(|x| dist_sq(to, x.posinfo.position) < d0) {
            best = s;
        }
    }
    Some(best)
}

fn class(u: &Universe, id: ShipId) -> Option<ShipClass> {
    u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).and_then(|s| s.ship_class)
}

/// The enemy blobs with some of the player's ships in them (`+0x2c`).
fn blobs_with_own_ships(u: &Universe, p: usize) -> Vec<super::attack::EnemyBlob> {
    super::attack::enemy_blobs(u, p).into_iter().filter(|b| b.own).collect()
}

fn allied(u: &Universe, p: usize, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.player == p || (s.player < u.players.len() && u.players[s.player].is_ally(p)))
}

/// `aiuFindUnarmedUndefendedEnemyShips` (0x41b6d0): the enemy ships of blobs where every
/// enemy is a resourcer, a non-combat ship or a salvage corvette (or a crystal or slipgate).
fn unarmed_undefended(u: &Universe, p: usize) -> Vec<ShipId> {
    let mut out = Vec::new();
    for b in blobs_with_own_ships(u, p) {
        let defended = b.ships.iter().any(|&id| {
            let s = u.ships.ship(id);
            !allied(u, p, id)
                && !matches!(class(u, id), Some(ShipClass::Resource | ShipClass::NonCombat))
                && !matches!(s.ship_type, ShipType::kSalCapCorvette | ShipType::tSalCapCorvette | ShipType::dCrystal0 | ShipType::dCrystal1 | ShipType::dSlipgateSmall | ShipType::dSlipgateMedium | ShipType::dSlipgateLarge)
        });
        if !defended {
            for &id in &b.ships {
                if !out.contains(&id) && super::attack::is_enemy(u, p, id) {
                    out.push(id);
                }
            }
        }
    }
    out
}

/// `aiuFindUnarmedEnemyShips` (0x41b7f0) and `aiuFindFighterVulnerableEnemyShips` (0x41b900).
fn prey(u: &Universe, p: usize, fighter_prey: bool) -> Vec<ShipId> {
    let mut out = Vec::new();
    for b in blobs_with_own_ships(u, p) {
        for &id in &b.ships {
            if allied(u, p, id) {
                continue;
            }
            let s = u.ships.ship(id);
            let t = s.ship_type;
            let ok = if fighter_prey {
                u.statics.ship(t).is_some_and(|st| st.is_capital_ship) && !matches!(t, ShipType::tDFGFrigate | ShipType::bDFGFrigate | ShipType::kGravWellGenerator | ShipType::tGravWellGenerator | ShipType::sMothership | ShipType::bMothership)
            } else {
                (matches!(class(u, id), Some(ShipClass::Resource | ShipClass::NonCombat)) || matches!(t, ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate | ShipType::kSalCapCorvette | ShipType::tSalCapCorvette))
                    && !matches!(t, ShipType::kGravWellGenerator | ShipType::tGravWellGenerator)
            };
            if ok {
                out.push(id);
            }
        }
    }
    out
}

/// `aiuFindNearbyDangerousEnemyShips` (0x41da90).
fn nearby_dangerous(u: &Universe, p: usize, id: ShipId, range: f32) -> Vec<ShipId> {
    let mut v = super::attack::find_nearby_enemy_ships(u, p, id, range);
    v.retain(|&s| super::defense::is_dangerous(u, s));
    v
}

/// `statsGetRelativeFleetStrengths` (0x4fa2b0) in Cataclysm: the ratio of the ship counts.
fn relative_strength(of: usize, against: usize) -> f64 {
    of as f64 / against as f64
}

/// `aiuFindBestUnarmedEnemyShip` (0x41c370) / `aiuFindBestFighterVulnerableEnemyShip`
/// (0x41c4d0): the nearest candidate the team outnumbers the defenders of two to one.
fn best_prey(u: &Universe, p: usize, team: &[ShipId], fighter_prey: bool) -> Option<ShipId> {
    let mut cands = prey(u, p, fighter_prey);
    let lead = u.ships.get(*team.first()?)?.posinfo.position;
    loop {
        let c = closest_ship(u, &cands, lead)?;
        let near = nearby_dangerous(u, p, c, TARGET_RANGE);
        if near.is_empty() || f64::from(TARGET_STRENGTH) <= relative_strength(team.len(), near.len()) {
            return Some(c);
        }
        cands.retain(|&s| s != c);
    }
}

/// `aiuFindHarassTarget` (0x41c630): the nearest undefended ship, unarmed ship or capital ship
/// (by distance weighted 1, 1.2 and 0.8), not a resourcer sitting at the enemy mothership.
fn find_target(u: &Universe, p: usize, team: &[ShipId]) -> Option<ShipId> {
    let lead = u.ships.get(*team.first()?)?.posinfo.position;
    let undefended = closest_ship(u, &unarmed_undefended(u, p), lead);
    let unarmed = best_prey(u, p, team, false);
    let fighter = best_prey(u, p, team, true);
    let enemy_ms = super::attack::find_enemy_mothership(u, p).and_then(|m| u.ships.get(m)).map(|s| s.posinfo.position);
    let score = |c: Option<ShipId>, w: f32| -> f32 {
        let Some(c) = c else { return 1e21 };
        let pos = u.ships.ship(c).posinfo.position;
        if let Some(m) = enemy_ms {
            if class(u, c) == Some(ShipClass::Resource) && dist_sq(pos, m) < 4500.0 {
                return 1e21;
            }
        }
        dist_sq(pos, lead) * w
    };
    let a = score(undefended, UNDEFENDED_WEIGHT);
    let b = score(unarmed, UNARMED_WEIGHT);
    let c = score(fighter, FIGHTER_PREY_WEIGHT);
    if a < b && a < c {
        return undefended;
    }
    if c <= b {
        fighter
    } else {
        unarmed
    }
}

/// An attack move (the advanced one for a team with feature 1) with the current move's
/// events, made current.
fn insert_attack(u: &mut Universe, t: usize, targets: Vec<ShipId>, formation: i32) -> Option<()> {
    let tm = &u.ai.teams.get(t)?.as_ref()?.1;
    let c = tm.current?;
    let advanced = tm.features & 1 != 0;
    let kind = if advanced { MoveKind::AdvancedAttack { targets, started: false } } else { MoveKind::Attack { targets } };
    let mut m = AiMove::new(kind, formation, 2, true, true);
    m.events = tm.moves[c].events.clone();
    m.team_died = tm.moves[c].team_died;
    let p = team_player(u, t);
    if ai(u, p).attack_features & 0x8_0000 != 0 {
        m.events.health_low = Some(Event { handler: Handler::KamikazeHealthLow, once: false, fired: false, percent: KAMIKAZE_HEALTH, base: 0, per_ship: true });
    }
    team::add_move_before_and_make_current(u, t, m);
    Some(())
}

/// `aimProcessHarassAttack` (0x408de0): never done while the team has ships; it finds a
/// target, flies near it, then attacks its defenders or it.
pub fn process_harass_attack(u: &mut Universe, t: usize, c: usize) -> bool {
    let p = team_player(u, t);
    let ships = team_ships(u, t);
    if ships.is_empty() {
        return true;
    }
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    if tm.moves[c].processing {
        return false;
    }
    tm.flags &= !flag::LOOKING;
    let Some(target) = find_target(u, p, &ships) else { return false };
    let lead = u.ships.ship(ships[0]).posinfo.position;
    let tpos = u.ships.ship(target).posinfo.position;
    if APPROACH_SQ < st(crate::level::distance_squared(lead, tpos)) {
        let point = range_standoff_point(u, tpos, lead, APPROACH_STANDOFF);
        let tm = &u.ai.teams[t].as_ref().expect("team").1;
        let mut m = AiMove::new(MoveKind::Move { destination: point }, APPROACH_FORMATION, 2, true, true);
        m.events = tm.moves[c].events.clone();
        m.team_died = tm.moves[c].team_died;
        team::add_move_before_and_make_current(u, t, m);
        return false;
    }
    u.ai.teams[t].as_mut().expect("team").1.flags |= flag::ATTACKING;
    let defenders = nearby_dangerous(u, p, target, DEFENDERS_RANGE);
    let n = defenders.len();
    if n == 0 {
        insert_attack(u, t, vec![target], ATTACK_FORMATION_ONE);
        let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
        let cur = tm.current.expect("current");
        tm.moves[cur].events.firing = event(Handler::HarassFiringSingleShip, true);
        if let MoveKind::HarassAttack { target: tg } = &mut tm.moves[cur + 1].kind {
            *tg = Some(target);
        }
        tm.moves[cur + 1].processing = false;
        return false;
    }
    let formation = if n < 2 { ATTACK_FORMATION_ONE } else { ATTACK_FORMATION_MORE };
    insert_attack(u, t, defenders, formation);
    if n == 1 {
        let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
        let cur = tm.current.expect("current");
        tm.moves[cur].events.firing = event(Handler::HarassFiringSingleShip, true);
    }
    false
}

/// `aimHarassAttackShipDied` (0x4079d0).
pub fn harass_target_died(u: &mut Universe, t: usize, c: usize, id: ShipId) {
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let MoveKind::HarassAttack { target } = &mut tm.moves[c].kind else { return };
    if *target != Some(id) {
        return;
    }
    *target = None;
    let mut reform = false;
    if let Some(f) = tm.moves[c].events.firing.as_mut() {
        if f.fired {
            f.fired = false;
            reform = !tm.ships.is_empty();
        }
    }
    tm.moves[c].processing = false;
    if reform {
        let ships = tm.ships.clone();
        team::wrap_formation_pub(u, &ships, APPROACH_FORMATION);
    }
}

/// `aimProcessAdvancedAttack` (0x408d40): `aiuAttack`, then wait until the ships stop
/// attacking.
pub fn process_advanced_attack(u: &mut Universe, t: usize, c: usize) -> bool {
    let ships = team_ships(u, t);
    if ships.is_empty() {
        return true;
    }
    let MoveKind::AdvancedAttack { targets, started } = u.ai.teams[t].as_ref().expect("team").1.moves[c].kind.clone() else { return true };
    let targets: Vec<ShipId> = targets.into_iter().filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0)).collect();
    if !started {
        if targets.is_empty() {
            return true;
        }
        attack(u, &ships, &targets);
        u.ai.teams[t].as_mut().expect("team").1.moves[c].kind = MoveKind::AdvancedAttack { targets, started: true };
        return false;
    }
    if !targets.is_empty() && !team::ships_no_longer_attacking_pub(u, &ships) {
        u.ai.teams[t].as_mut().expect("team").1.moves[c].kind = MoveKind::AdvancedAttack { targets, started: true };
        return false;
    }
    true
}

fn fit(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.flags & (sof::DEAD | sof::HYPERSPACE) == 0 && s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION == 0)
}

/// `aiuAttack` (0x41ea70): the ships that cannot attack guard those that can, which attack by
/// type.
fn attack(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let capable = crate::command::attack_capable(u, ships);
    if capable.is_empty() {
        return;
    }
    if capable.len() != ships.len() {
        let guards: Vec<ShipId> = ships.iter().copied().filter(|s| !capable.contains(s) && fit(u, *s)).collect();
        let guarded: Vec<ShipId> = capable.iter().copied().filter(|&s| fit(u, s)).collect();
        if !guards.is_empty() && !guarded.is_empty() {
            crate::command::cl_protect(u, &guards, &guarded);
        }
    }
    let first = u.ships.ship(capable[0]).ship_type;
    if capable.iter().all(|&s| u.ships.ship(s).ship_type == first) {
        team::wrap_attack(u, &capable, targets);
        return;
    }
    let mut left = capable;
    while let Some(&lead) = left.first() {
        let t = u.ships.ship(lead).ship_type;
        let group: Vec<ShipId> = left.iter().copied().filter(|&s| u.ships.ship(s).ship_type == t).collect();
        left.retain(|s| !group.contains(s));
        team::wrap_attack(u, &group, targets);
    }
}

/// Runs a handler (`aieExecute` calls them).
pub fn handle(u: &mut Universe, t: usize, h: Handler, rocker: Option<ShipId>) {
    match h {
        Handler::Reconnaissance => super::recon::team_died_handler(u, t),
        Handler::Harass => team_died(u, t),
        Handler::HarassNumbersLow => numbers_low(u, t),
        Handler::GenericGettingRocked => getting_rocked(u, t, rocker),
        Handler::HarassFiringSingleShip => {
            let ships = team_ships(u, t);
            team::wrap_formation_pub(u, &ships, FIRING_FORMATION);
            let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
            if let Some(c) = tm.current {
                tm.moves[c].events.disengage = event(Handler::HarassDisengageSingleShip, true);
            }
        }
        Handler::HarassDisengageSingleShip => {
            let ships = team_ships(u, t);
            let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
            let Some(c) = tm.current else { return };
            let formation = tm.moves[c].formation;
            if let Some(f) = tm.moves[c].events.firing.as_mut() {
                f.fired = false;
            }
            team::wrap_formation_pub(u, &ships, formation);
        }
        Handler::KamikazeHealthLow => kamikaze(u, t),
        Handler::Patrol => {
            if let Some(Some((_, tm))) = u.ai.teams.get_mut(t) {
                tm.flags |= team_flag::DESTROY;
            }
        }
    }
}

/// `aihHarassTeamDiedHandler` (0x406e40): made again three times in four (the third death
/// ends it).
fn team_died(u: &mut Universe, t: usize) {
    let p = team_player(u, t);
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    if tm.flags & flag::DIED_ONCE == 0 {
        tm.flags |= flag::DIED_ONCE;
    } else if tm.flags & flag::DIED_TWICE != 0 {
        tm.flags = (tm.flags & !(flag::DIED_ONCE | flag::DIED_TWICE)) | team_flag::DESTROY;
        return;
    } else {
        tm.flags |= flag::DIED_TWICE;
    }
    if rnd(u) & 3 != 0 {
        team::delete_all_moves(u, t);
        create(u, p, t);
    } else {
        u.ai.teams[t].as_mut().expect("team").1.flags |= team_flag::DESTROY;
    }
}

/// `aihHarassNumbersLowHandler` (0x405d50): the attacks are dropped and the team goes home to
/// be topped up, then out again.
fn numbers_low(u: &mut Universe, t: usize) {
    let p = team_player(u, t);
    let home = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map_or(Vector::ZERO, |s| s.posinfo.position);
    loop {
        let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
        let Some(c) = tm.current else { return };
        let ty = tm.moves[c].move_type();
        if ty != team::move_type::ATTACK && ty != move_type::ADVANCED_ATTACK {
            break;
        }
        if !tm.moves[c].remove {
            if c + 1 >= tm.moves.len() {
                return;
            }
            tm.current = Some(c + 1);
        } else {
            team::delete_current_move(u, t);
        }
    }
    let point = range_standoff_point(u, home, Vector::ZERO, HOME_STANDOFF);
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let c = tm.current.expect("current");
    tm.moves[c].processing = false;
    if let Some(n) = tm.moves[c].events.numbers_low.as_mut() {
        n.fired = false;
    }
    let mut m = AiMove::new(MoveKind::Move { destination: point }, HOME_FORMATION, 0, false, true);
    m.tactics = 0;
    team::add_move_before_and_make_current(u, t, m);
    let get = match u.players[p].race {
        ShipRace::Somtaaw => Some(fancy(1, 16, &[(4, 14), (2, 14)], true, true)),
        ShipRace::Beast => {
            let a = if super::research::ship_type_acquired(u, p, ty(0x22)) { 0x22 } else { 0x23 };
            let b = if super::research::ship_type_acquired(u, p, ty(0x26)) { 0x26 } else { 0x25 };
            Some(fancy(0x28, 16, &[(0x23, 14), (0x25, 14), (a, 14), (b, 14)], true, true))
        }
        _ => None,
    };
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    if let Some(g) = get {
        // Before the move that was current (the moves are now: home, it).
        let c = tm.current.expect("current");
        tm.moves.insert(c + 1, g);
    }
    tm.flags &= !flag::ATTACKING;
}

/// `aihGenericGettingRockedHandler` (0x4065b0): unless already attacking there, the team
/// attacks whoever hits it (and its formation) when it has half again as many ships.
fn getting_rocked(u: &mut Universe, t: usize, rocker: Option<ShipId>) {
    let Some(rocker) = rocker else { return };
    let ships = team_ships(u, t);
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let Some(c) = tm.current else { return };
    if let MoveKind::Attack { targets } = &tm.moves[c].kind {
        if let (Some(&first), Some(&tgt)) = (ships.first(), targets.first()) {
            let blob = |id: ShipId| u.ships.get(id).and_then(|s| s.coll_my_blob);
            if blob(first) == blob(tgt) {
                return;
            }
        }
    }
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    if let Some(e) = tm.moves[c].events.getting_rocked.as_mut() {
        e.fired = false;
    }
    let targets = ship_and_its_formation(u, rocker);
    if not_good_at_killing(u, &ships, &targets) {
        return;
    }
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    tm.moves[c].processing = false;
    let mut m = AiMove::new(MoveKind::Attack { targets }, ROCKED_FORMATION, 2, true, true);
    m.events = tm.moves[c].events.clone();
    m.team_died = tm.moves[c].team_died;
    team::add_move_before_and_make_current(u, t, m);
}

/// `getShipAndItsFormation` (0x4496f0): the ship, or its formation's ships.
fn ship_and_its_formation(u: &Universe, id: ShipId) -> Vec<ShipId> {
    if let Some(c) = u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)) {
        if c.attributes & crate::command::attr::FORMATION != 0 {
            return c.selection.clone();
        }
    }
    vec![id]
}

/// `aiuSelectionNotGoodAtKillingTheseTargets` (0x41e740): fighters against ships no good at
/// shooting fighters, or fewer than 1.5 ships for each target.
fn not_good_at_killing(u: &Universe, team: &[ShipId], targets: &[ShipId]) -> bool {
    let all_fighters = targets.iter().all(|&id| class(u, id) == Some(ShipClass::Fighter));
    let bad = |id: ShipId| {
        u.ships.get(id).is_some_and(|s| {
            matches!(
                s.ship_type,
                ShipType::bIonArrayFrigate | ShipType::bHeavyCruiser | ShipType::kHeavyCruiser | ShipType::kIonCannonFrigate | ShipType::kStandardDestroyer | ShipType::tHeavyCruiser | ShipType::tIonCannonFrigate | ShipType::tStandardDestroyer
            )
        })
    };
    if all_fighters && team.iter().all(|&id| bad(id)) {
        return true;
    }
    relative_strength(team.len(), targets.len()) < f64::from(ROCKED_STRENGTH)
}

/// `aihKamikazeHealthLowHandler` (0x405fe0): the badly hurt fighters and corvettes ram.
fn kamikaze(u: &mut Universe, t: usize) {
    let tm = &u.ai.teams[t].as_ref().expect("team").1;
    let Some(c) = tm.current else { return };
    let pct = tm.moves[c].events.health_low.as_ref().map_or(0, |e| e.percent);
    let ram: Vec<ShipId> = tm
        .ships
        .iter()
        .copied()
        .filter(|&id| {
            matches!(class(u, id), Some(ShipClass::Fighter | ShipClass::Corvette))
                && u.ships.get(id).zip(u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type))).is_some_and(|(s, st)| st_lt(s.health, pct, st.health.max_health))
                && fit(u, id)
        })
        .collect();
    if !ram.is_empty() {
        crate::player_orders::cl_kamikaze(u, &ram);
    }
}

/// `health * 100 < percent * max` as the original computes it.
fn st_lt(health: f32, percent: i32, max: f32) -> bool {
    st(ld(health) * 100.0) < st(ld(percent as f32) * ld(max))
}

/// `aieCheckHealthLow` (0x404090): a ship (or, not per ship, the team as a whole) below the
/// percentage of its health.
pub fn health_low(u: &Universe, ships: &[ShipId], percent: i32, per_ship: bool) -> bool {
    let (mut max, mut have) = (0f32, 0f32);
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        let Some(st_) = u.statics.ship(s.ship_type) else { continue };
        if per_ship {
            if st_lt(s.health, percent, st_.health.max_health) {
                return true;
            }
        } else {
            max = st(ld(max) + ld(st_.health.max_health));
            have = st(ld(have) + ld(s.health));
        }
    }
    !per_ship && st_lt(have, percent, max)
}
