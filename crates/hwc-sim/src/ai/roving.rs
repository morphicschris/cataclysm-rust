//! The roving defence of an advanced computer player (`aidDefenseManager` with the roving
//! feature, `aioCreateFastRovingDefense`, `aioCreateSlowRovingDefense`): patrols that fly
//! between the player's resourcers nearest the enemy (`aimProcessActivePatrol`,
//! `aiuGeneratePatrolPath`, `aimProcessPatrolMove`), answer distress calls and threats to the
//! sphere of influence (`aihFast/SlowDefenseDistressHandler`, `aiuRescueShipType`,
//! `aiuRescueShip`), go home to be topped up (`aihFast/SlowDefenseNumbersLowHandler`) and are
//! made again when wiped out. Fast patrols are fighters, slow ones frigates and corvettes; the
//! slow patrols' paths are only ever made of HW1's Advance Support Frigates, so in Cataclysm
//! they wait at home for distress calls.

use hwc_math::Vector;

use super::team::{self, AiMove, Event, Handler, MoveKind, SAME_FORMATION};
use super::{ai, ai_mut, Manager};
use crate::object::ShipId;
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// `MOVE_PATROLMOVE`, `MOVE_ACTIVEPATROL` and the patrol's own moves (`MOVE_MOVE` 7).
pub mod move_type {
    pub const PATROL_POINT: u32 = 7;
    pub const PATROL_MOVE: u32 = 0xb;
    pub const ACTIVE_PATROL: u32 = 0xc;
}

/// Team flags of the roving defence teams.
pub mod flag {
    pub const FAST: u32 = 0x1;
    pub const SLOW: u32 = 0x2;
}

/// `aidDefenseManager`: a big resourcer counts this (0x7409fc); with fewer than 0x740a04 teams
/// there is a team for each 0x740a0c of it, else for each 0x740a08.
const BIG_RESOURCER: u32 = 2;
const FEW_TEAMS: u32 = 3;
const PER_TEAM_FEW: u32 = 2;
const PER_TEAM_MANY: u32 = 4;
/// Numbers low (0x740a58, 0x740a60, per cent of the team).
const FAST_NUMBERS_LOW: i32 = 80;
const SLOW_NUMBERS_LOW: i32 = 50;
/// Turns between a slow patrol's looks at the distress calls (the fast one looks every turn).
const SLOW_INTERRUPT_DELAY: i32 = 3;
/// Home points (0x740b00, 0x740b18) and the home move's formation (0x740b04, 0x740b1c).
const FAST_HOME: f32 = 75000.0;
const SLOW_HOME: f32 = 7000.0;
const HOME_FORMATION: i32 = 1;
/// A threat's defenders within this (0x740b08, 0x740b24) are attacked with it, in formation
/// 0x740b10 / 0x740b2c when more than one, else 0x740b14 / 0x740b30; a slow patrol only goes
/// for threats within this (squared, 0x740b20) of it.
const THREAT_RANGE: f32 = 15000.0;
const FAST_FORMATION_MANY: i32 = 3;
const FAST_FORMATION_ONE: i32 = 5;
const SLOW_FORMATION_MANY: i32 = 1;
const SLOW_FORMATION_ONE: i32 = 5;
const SLOW_THREAT_SQ: f32 = 100_000_000.0;
/// `aiuRescueShip`: the attackers within this of the ship (0x740b8c); the formation by team and
/// attacker numbers (0x740b58..0x740b68).
const RESCUE_RANGE: f32 = 8000.0;
/// Kamikaze health of a fast patrol's ships and of rescuers (0x740a94, 0x740a98).
const KAMIKAZE_HEALTH: i32 = 45;
const RESCUE_KAMIKAZE_HEALTH: i32 = 25;
/// `aitCheckIfOtherDefTeamAnsweringSignalNeedsHelp`'s strength (0x740a30).
const HELP_STRENGTH: f32 = 1.2;
/// Patrol points are this short of the resourcers seen from the enemy mothership (0x451c4000).
const PATROL_STANDOFF: f32 = 2500.0;

fn ty(n: u8) -> ShipType {
    ShipType::from_index(n.into()).expect("ship type")
}

fn acquired(u: &Universe, p: usize, n: u8) -> bool {
    super::research::ship_type_acquired(u, p, ty(n))
}

fn fancy(primary: u8, num: i32, alternatives: &[(u8, i32)], wait: bool, remove: bool) -> AiMove {
    let alternatives = alternatives.iter().map(|&(n, r)| (ty(n), r)).collect();
    AiMove::new(MoveKind::FancyGetShips { primary: ty(primary), num, alternatives, random_alternative: true, priority: 0, var: None }, SAME_FORMATION, 2, wait, remove)
}

fn team_ships(u: &Universe, t: usize) -> Vec<ShipId> {
    u.ai.teams.get(t).and_then(|x| x.as_ref()).map(|x| x.1.ships.clone()).unwrap_or_default()
}

fn team_player(u: &Universe, t: usize) -> usize {
    u.ai.teams[t].as_ref().expect("team").0
}

/// `aidDefenseManager`'s roving part: a fast or slow team (alternately) while there are fewer
/// teams than the resourcers call for.
pub fn manage(u: &mut Universe, p: usize) {
    let mut weight = 0u32;
    for &id in &ai(u, p).resource_collectors {
        let Some(s) = u.ships.get(id) else { continue };
        match s.ship_type.index() {
            0xc | 0x10 | 0x2f | 0x33 | 0x3c | 0x3e | 0x4a | 0x4c => weight += BIG_RESOURCER,
            0xe | 0x31 | 0x48 | 0x58 | 0x59 => weight += 1,
            _ => {}
        }
    }
    let n = ai(u, p).defense.teams.len() as u32;
    let per = if n < FEW_TEAMS { PER_TEAM_FEW } else { PER_TEAM_MANY };
    if weight / per <= n {
        return;
    }
    let t = super::team_create(u, p, Manager::Defense, "");
    let fast = n & 1 == 0;
    if let Some(Some((_, tm))) = u.ai.teams.get_mut(t) {
        tm.flags |= if fast { flag::FAST } else { flag::SLOW };
    }
    ai_mut(u, p).defense.teams.push(t);
    if fast {
        create_fast(u, p, t);
    } else {
        create_slow(u, p, t);
    }
}

fn patrol_move(kind: u32, team_died: Handler, numbers: i32, numbers_low: Handler, delay: i32, distress: Handler) -> AiMove {
    let mut m = AiMove::new(MoveKind::ActivePatrol { kind }, 2, 2, true, false);
    m.team_died = Some(team_died);
    m.events.numbers_low = Some(Event { handler: numbers_low, once: false, fired: false, percent: numbers, base: 0, per_ship: false });
    m.events.getting_rocked = Some(Event { handler: Handler::GenericGettingRocked, once: false, fired: false, percent: 0, base: 0, per_ship: false });
    m.events.interrupt = Some(Event { handler: distress, once: false, fired: false, percent: delay, base: delay, per_ship: false });
    m
}

/// `aioCreateFastRovingDefense` (0x40e980): twelve fighters on an active patrol of the
/// resourcers.
pub fn create_fast(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => team::add_move(u, t, fancy(1, 12, &[(4, 14), (4, 14), (3, 14), (0, 20), (2, 15), (5, 15)], true, false)),
        ShipRace::Beast => {
            let (a, b) = if acquired(u, p, 0x26) { (0x26, 0x26) } else { (0x25, 0x27) };
            team::add_move(u, t, fancy(0x23, 12, &[(0x28, 14), (0x28, 14), (0x27, 14), (0x21, 20), (0x25, 15), (0x3b, 15), (a, 14), (b, 14)], true, false));
        }
        _ => {}
    }
    team::add_move(u, t, patrol_move(0, Handler::FastDefenseTeamDied, FAST_NUMBERS_LOW, Handler::FastDefenseNumbersLow, 0, Handler::FastDefenseDistress));
    team::add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aioCreateSlowRovingDefense` (0x40eb50): three frigates and corvettes on an active patrol of
/// the support frigates.
pub fn create_slow(u: &mut Universe, p: usize, t: usize) {
    match u.players[p].race {
        ShipRace::Somtaaw => team::add_move(u, t, fancy(7, 3, &[(4, 10), (3, 10), (5, 10), (0x11, 10), (6, 7), (10, 30), (0x19, 30), (8, 30)], true, false)),
        ShipRace::Beast => {
            let x26 = if acquired(u, p, 0x26) { 0x26 } else { 0x25 };
            let x34 = if acquired(u, p, 0x34) { 0x34 } else { 0x27 };
            let x30 = if acquired(u, p, 0x30) { 0x30 } else { 0x28 };
            let x2a = if acquired(u, p, 0x2a) { 0x2a } else { 0x2c };
            team::add_move(u, t, fancy(0x27, 3, &[(0x28, 10), (0x25, 10), (0x24, 10), (0x27, 10), (0x2c, 7), (0x2e, 30), (x26, 10), (x34, 10), (x30, 7), (x2a, 30)], true, false));
        }
        _ => {}
    }
    team::add_move(u, t, patrol_move(1, Handler::SlowDefenseTeamDied, SLOW_NUMBERS_LOW, Handler::SlowDefenseNumbersLow, SLOW_INTERRUPT_DELAY, Handler::SlowDefenseDistress));
    team::add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aiuFindClosestVulnerableGoodGuyShips` (0x41bed0): the player's resourcers (kind 0) or
/// Advance Support Frigates (kind 1) in the blobs it has ships in, nearest `from` first, as
/// many as there are such blobs.
fn closest_vulnerable(u: &Universe, p: usize, from: Vector, kind: u32) -> Vec<ShipId> {
    let blobs: Vec<&crate::blobs::Blob> = u.blobs.blobs.iter().filter(|b| b.ships.iter().any(|&s| u.ships.get(s).is_some_and(|x| x.player == p))).collect();
    let mut cands = Vec::new();
    for b in &blobs {
        for &id in &b.ships {
            let Some(s) = u.ships.get(id) else { continue };
            if s.player != p {
                continue;
            }
            let t = s.ship_type;
            let asf = matches!(t, ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate);
            let ok = if kind == 1 {
                asf
            } else {
                u.statics.ship(t).and_then(|st| st.ship_class) == Some(ShipClass::Resource) || asf || matches!(t, ShipType::kSalCapCorvette | ShipType::tSalCapCorvette)
            };
            if ok {
                cands.push(id);
            }
        }
    }
    let d = |id: ShipId| {
        let q = u.ships.ship(id).posinfo.position;
        let (x, y, z) = (q.x - from.x, q.y - from.y, q.z - from.z);
        x * x + y * y + z * z
    };
    let mut out = Vec::new();
    while out.len() < blobs.len() && !cands.is_empty() {
        let mut bi = 0;
        for i in 1..cands.len() {
            if d(cands[i]) < d(cands[bi]) {
                bi = i;
            }
        }
        out.push(cands.remove(bi));
    }
    out
}

/// `aiuGeneratePatrolPath` (0x41b430): points near the vulnerable ships no farther from the
/// enemy mothership than the player's own mothership is.
fn patrol_path(u: &mut Universe, p: usize, kind: u32) -> Vec<Vector> {
    let own = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.posinfo.position);
    let enemy = super::attack::find_enemy_mothership(u, p).and_then(|m| u.ships.get(m)).map(|s| s.posinfo.position);
    let (Some(own), Some(enemy)) = (own, enemy) else { return Vec::new() };
    let mut v = closest_vulnerable(u, p, enemy, if kind == 1 { 1 } else { 0 });
    let reach = {
        let (x, y, z) = (enemy.x - own.x, enemy.y - own.y, enemy.z - own.z);
        x * x + y * y + z * z
    };
    v.retain(|&id| {
        let q = u.ships.ship(id).posinfo.position;
        let (x, y, z) = (q.x - enemy.x, q.y - enemy.y, q.z - enemy.z);
        x * x + y * y + z * z <= reach
    });
    v.into_iter().map(|id| u.ships.ship(id).posinfo.position).collect::<Vec<_>>().into_iter().map(|q| super::harass::range_standoff_point(u, q, enemy, PATROL_STANDOFF)).collect()
}

/// `aimProcessActivePatrol` (0x409ba0): never done while the team has ships; a patrol of the
/// path goes before it.
pub fn process_active_patrol(u: &mut Universe, t: usize, c: usize) -> bool {
    let ships = team_ships(u, t);
    if ships.is_empty() {
        return true;
    }
    let p = team_player(u, t);
    let MoveKind::ActivePatrol { kind } = u.ai.teams[t].as_ref().expect("team").1.moves[c].kind else { return true };
    let path = patrol_path(u, p, kind);
    if path.is_empty() {
        return false;
    }
    let tm = &u.ai.teams[t].as_ref().expect("team").1;
    let mut m = AiMove::new(MoveKind::PatrolMove { path, start: 0 }, tm.moves[c].formation, 2, true, true);
    m.events = tm.moves[c].events.clone();
    m.team_died = tm.moves[c].team_died;
    team::add_move_before_and_make_current(u, t, m);
    false
}

/// `aimProcessPatrolMove` (0x409810): the first time a move to each point of the path goes
/// before it (from the start point); then it is done.
pub fn process_patrol_move(u: &mut Universe, t: usize, c: usize) -> bool {
    if team_ships(u, t).is_empty() {
        return true;
    }
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    if tm.moves[c].processing {
        return true;
    }
    let MoveKind::PatrolMove { path, start } = tm.moves[c].kind.clone() else { return true };
    let (formation, tactics, events, died) = (tm.moves[c].formation, tm.moves[c].tactics, tm.moves[c].events.clone(), tm.moves[c].team_died);
    let mut at = c;
    for &point in path.iter().skip(start) {
        let mut m = AiMove::new(MoveKind::PatrolPoint { destination: point }, formation, tactics, true, true);
        m.events = events.clone();
        m.team_died = died;
        tm.moves.insert(at, m);
        at += 1;
    }
    tm.moves[at].processing = true;
    if at != c {
        tm.current = Some(c);
    }
    false
}

/// Runs a roving defence handler.
pub fn handle(u: &mut Universe, t: usize, h: Handler) {
    let p = team_player(u, t);
    match h {
        Handler::FastDefenseTeamDied => {
            team::delete_all_moves(u, t);
            create_fast(u, p, t);
        }
        Handler::SlowDefenseTeamDied => {
            team::delete_all_moves(u, t);
            create_slow(u, p, t);
        }
        Handler::FastDefenseNumbersLow => numbers_low(u, p, t, true),
        Handler::SlowDefenseNumbersLow => numbers_low(u, p, t, false),
        Handler::FastDefenseDistress => distress(u, p, t, true),
        Handler::SlowDefenseDistress => distress(u, p, t, false),
        _ => {}
    }
}

/// `aihFast/SlowDefenseNumbersLowHandler` (0x406080, 0x4062e0): home, and the losses made up
/// (a new team's worth when none are left).
fn numbers_low(u: &mut Universe, p: usize, t: usize, fast: bool) {
    let home = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map_or(Vector::ZERO, |s| s.posinfo.position);
    let point = super::harass::range_standoff_point(u, home, Vector::ZERO, if fast { FAST_HOME } else { SLOW_HOME });
    let ships = team_ships(u, t);
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let Some(c) = tm.current else { return };
    let base = tm.moves[c].events.numbers_low.map_or(0, |e| e.base);
    tm.moves[c].processing = false;
    if let Some(e) = tm.moves[c].events.numbers_low.as_mut() {
        e.fired = false;
    }
    team::add_move_before_and_make_current(u, t, AiMove::new(MoveKind::Move { destination: point }, HOME_FORMATION, 2, false, true));
    let get = if ships.is_empty() {
        match (u.players[p].race, fast) {
            (ShipRace::Somtaaw, true) => Some(fancy(1, 12, &[(0, 20), (4, 14), (2, 15)], true, false)),
            (ShipRace::Beast, true) => {
                let x = if acquired(u, p, 0x26) { 0x26 } else { 0x25 };
                Some(fancy(0x23, 12, &[(0x28, 14), (0x21, 20), (0x25, 15), (x, 14)], true, false))
            }
            (ShipRace::Somtaaw, false) => Some(fancy(5, 3, &[(4, 12), (5, 7), (7, 30), (8, 30)], true, false)),
            (ShipRace::Beast, false) => {
                let x26 = if acquired(u, p, 0x26) { 0x26 } else { 0x25 };
                let x29 = if acquired(u, p, 0x29) { 0x29 } else { 0x2c };
                let x2a = if acquired(u, p, 0x2a) { 0x2a } else { 0x3b };
                Some(fancy(0x27, 3, &[(0x28, 12), (0x3b, 7), (0x2c, 30), (0x25, 30), (x26, 12), (x29, 30), (x2a, 30)], true, false))
            }
            _ => None,
        }
    } else {
        let kind = u.ships.get(ships[0]).map(|s| s.ship_type);
        let n = ships.len() as i32;
        let want = if n < base { (base - n) as i8 as i32 } else { 1 };
        kind.map(|k| AiMove::new(MoveKind::FancyGetShips { primary: k, num: want, alternatives: Vec::new(), random_alternative: false, priority: 0, var: None }, SAME_FORMATION, 2, true, true))
    };
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let c = tm.current.expect("current");
    if let Some(e) = tm.moves[c + 1].events.numbers_low.as_mut() {
        e.base = 0;
    }
    tm.moves[c + 1].processing = false;
    if let Some(g) = get {
        tm.moves.insert(c + 1, g);
    }
}

/// `aitCheckIfOtherDefTeamAnsweringSignalNeedsHelp` (0x417770): no other defence team is
/// attacking these targets, or those that are are no good at it.
fn others_need_help(u: &Universe, p: usize, t: usize, targets: &[ShipId]) -> bool {
    let mut theirs: Vec<ShipId> = Vec::new();
    for &o in &ai(u, p).defense.teams {
        if o == t {
            continue;
        }
        let Some((_, tm)) = u.ai.teams.get(o).and_then(|x| x.as_ref()) else { continue };
        let Some(c) = tm.current else { continue };
        let same = match &tm.moves[c].kind {
            MoveKind::Attack { targets: x } | MoveKind::AdvancedAttack { targets: x, .. } | MoveKind::FlankAttack { targets: x } | MoveKind::FlankAttackHyperspace { targets: x } => {
                x.len() == targets.len() && x.iter().all(|s| targets.contains(s))
            }
            _ => false,
        };
        if same {
            for &s in &tm.ships {
                if !theirs.contains(&s) {
                    theirs.push(s);
                }
            }
        }
    }
    if theirs.is_empty() {
        return true;
    }
    super::harass::not_good_at_killing_pub(u, &theirs, targets, HELP_STRENGTH)
}

/// An advanced attack on `targets` before the current move, with its events but neither the
/// numbers-low nor the distress handler.
fn insert_defence_attack(u: &mut Universe, p: usize, t: usize, targets: Vec<ShipId>, formation: i32, kamikaze: Option<i32>) {
    let tm = &mut u.ai.teams[t].as_mut().expect("team").1;
    let Some(c) = tm.current else { return };
    let mut m = AiMove::new(MoveKind::AdvancedAttack { targets, started: false }, formation, 2, true, true);
    m.events = tm.moves[c].events.clone();
    m.team_died = tm.moves[c].team_died;
    m.events.numbers_low = None;
    m.events.interrupt = None;
    tm.moves[c].processing = false;
    if let (Some(pct), true) = (kamikaze, ai(u, p).attack_features & 0x8_0000 != 0) {
        m.events.health_low = Some(Event { handler: Handler::KamikazeHealthLow, once: false, fired: false, percent: pct, base: 0, per_ship: true });
    }
    team::add_move_before_and_make_current(u, t, m);
}

/// `aiuRescueShip` (0x41a990): the dangerous enemies near the ship (and whoever is hitting it)
/// are attacked when no other defence team has it in hand.
fn rescue_ship(u: &mut Universe, p: usize, t: usize, ship: ShipId) -> bool {
    let mut enemies: Vec<ShipId> = super::attack::find_nearby_enemy_ships(u, p, ship, RESCUE_RANGE);
    enemies.retain(|&s| super::defense::is_dangerous(u, s));
    if let Some(r) = u.ships.get(ship).and_then(|s| s.getting_rocked) {
        enemies.push(r);
    }
    if enemies.is_empty() || !others_need_help(u, p, t, &enemies) {
        return false;
    }
    let ships = team_ships(u, t);
    let mut formation = if ships.len() < 4 { 1 } else { 5 };
    if 2 <= enemies.len() {
        formation = 4;
    }
    let lead_class = ships.first().and_then(|&s| u.ships.get(s)).and_then(|s| u.statics.ship(s.ship_type)).and_then(|s| s.ship_class);
    let kamikaze = matches!(lead_class, Some(ShipClass::Corvette | ShipClass::Fighter)).then_some(RESCUE_KAMIKAZE_HEALTH);
    insert_defence_attack(u, p, t, enemies, formation, kamikaze);
    true
}

/// `aiuRescueShipType` (0x41a8d0): the last ship of the type among those to rescue is rescued.
fn rescue_ship_type(u: &mut Universe, p: usize, t: usize, list: &[ShipId], kind: u8) -> bool {
    let Some(&last) = list.iter().filter(|&&s| u.ships.get(s).is_some_and(|x| x.ship_type == ty(kind))).last() else { return false };
    rescue_ship(u, p, t, last)
}

/// The processor type of a race (`aihFast/SlowDefenseDistressHandler`'s rescue by race).
fn processor(race: ShipRace) -> Option<u8> {
    match race {
        ShipRace::Kushan => Some(0x3c),
        ShipRace::Taiidan => Some(0x4a),
        ShipRace::Somtaaw => Some(0x10),
        ShipRace::Beast => Some(0x33),
        _ => None,
    }
}

/// `aihFastDefenseDistressHandler` (0x406890) / `aihSlowDefenseDistressHandler` (0x406b00): a
/// processor to rescue first, then the threats to the sphere of influence (a slow patrol only
/// near ones), then (fast) a resourcer to rescue.
fn distress(u: &mut Universe, p: usize, t: usize, fast: bool) {
    let ships = team_ships(u, t);
    if !fast && ships.is_empty() {
        return;
    }
    let rescue = ai(u, p).defense.rescue.clone();
    if !rescue.is_empty() {
        let race = ships.first().and_then(|&s| u.ships.get(s)).and_then(|s| u.statics.ship(s.ship_type)).map(|s| s.ship_race);
        if let Some(k) = race.and_then(processor) {
            if rescue_ship_type(u, p, t, &rescue, k) {
                return;
            }
        }
    }
    let threats = ai(u, p).defense.threats.clone();
    for &th in &threats {
        let mut targets: Vec<ShipId> = super::attack::find_nearby_enemy_ships(u, p, th, THREAT_RANGE);
        targets.retain(|&s| super::defense::is_dangerous(u, s));
        targets.push(th);
        let near = fast || {
            let a = u.ships.get(th).map(|s| s.posinfo.position);
            let b = ships.first().and_then(|&s| u.ships.get(s)).map(|s| s.posinfo.position);
            a.zip(b).is_some_and(|(a, b)| (crate::level::distance_squared(a, b) as f32) < SLOW_THREAT_SQ)
        };
        if others_need_help(u, p, t, &targets) && near {
            let formation = match (fast, 1 < targets.len()) {
                (true, true) => FAST_FORMATION_MANY,
                (true, false) => FAST_FORMATION_ONE,
                (false, true) => SLOW_FORMATION_MANY,
                (false, false) => SLOW_FORMATION_ONE,
            };
            let kamikaze = if fast {
                Some(KAMIKAZE_HEALTH)
            } else {
                let lead_class = ships.first().and_then(|&s| u.ships.get(s)).and_then(|s| u.statics.ship(s.ship_type)).and_then(|s| s.ship_class);
                (lead_class == Some(ShipClass::Corvette)).then_some(RESCUE_KAMIKAZE_HEALTH)
            };
            insert_defence_attack(u, p, t, targets, formation, kamikaze);
            return;
        }
    }
    if fast && !rescue.is_empty() {
        let k = match u.players[p].race {
            ShipRace::Taiidan => Some(0x58),
            ShipRace::Somtaaw => Some(0xe),
            ShipRace::Beast => Some(0x31),
            _ => None,
        };
        if let Some(k) = k {
            rescue_ship_type(u, p, t, &rescue, k);
        }
    }
}
