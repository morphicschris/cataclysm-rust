//! Target takeouts (attack types 4 and 5: `aioCreateTakeoutTarget`,
//! `aioCreateFancyTakeoutTarget`): the most dangerous enemy capital ship or frigate the player
//! knows of (`aiuUpdateKnowledgeOfEnemyShips`, `statsGetMostDangerousShipNonStatConstraints`
//! with `aiaPriorityShipsConstraints`) is attacked by ships picked or bought for it by the
//! Cataclysm stats tables (`statsBestShipsToUseToKillTarget`, `statsBestShipToBuyToKillShip`,
//! `statsNumShipsNeededToKillTarget`); the fancy form sends an escort team along when the
//! target has defenders (`statsBestShipToBuyToKillFleet`, `aimCreateGuardCooperatingTeam`, the
//! two teams waiting for each other on AI variables).

use hwc_math::random::RAN_AI_PLAYER;

use super::team::{self, add_move, var_create, AiMove, Event, Handler, MoveKind, SAME_FORMATION};
use super::{ai, ai_mut};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// `MOVE_VARSET`, `MOVE_VARWAIT`, `MOVE_VARDESTROY`, `MOVE_GUARDCOOPTEAM`.
pub mod move_type {
    pub const VAR_SET: u32 = 0x1a;
    pub const VAR_WAIT: u32 = 0x1d;
    pub const VAR_DESTROY: u32 = 0x1e;
    pub const GUARD_COOPERATING_TEAM: u32 = 0x1f;
}

/// The temporary guard and attack of a takeout (0x740a84, 0x740a88; the fancy one's 0x740a8c,
/// 0x740a90) and the target attack's formation.
const GUARD_FORMATION: i32 = 1;
const GUARD_TACTICS: i32 = 2;
const ATTACK_FORMATION: i32 = 1;
/// The escort is bought against the enemies this near the target (0x467a0000), at most 20.
const ESCORT_RANGE: f32 = 16000.0;
const ESCORT_MAX: i32 = 20;

/// `statsGetOverallKillRating`'s Cataclysm stand-in (0x88c200, five floats a type, the first
/// used), for the first 60 types; other types rate 1.
const RATING: [f32; 60] = [
    1.0, 3.0, 4.0, 4.0, 5.0, 6.0, 7.0, 9.0, 8.0, 1.0, 11.0, 1.0, 5.0, 1.0, 5.0, 1.0, 5.0, 3.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 2.0, 3.0, 2.0, 1.0, 3.0, 2.0, 4.0, 5.0, 5.0, 7.0, 6.0, 9.0, 8.0, 1.0, 10.0, 1.0, 11.0, 5.0, 7.0, 5.0, 1.0, 5.0,
    3.0, 2.0, 2.0, 2.0, 2.0, 2.0, 5.0, 4.0,
];
/// Whether a ship to buy against a type is known (0x88c110 for a Somtaaw target, the same table
/// three on for a Beast one).
const CAN_COUNTER: [u8; 63] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 0, 0, 0, 0, 1, 1, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 1, 1, 1, 0, 0, 0, 1, 1, 0, 0, 0, 0, 1, 1,
];
/// The type to buy against each type, for a Somtaaw and a Beast buyer (0x88cd54).
const COUNTER: [[u8; 2]; 60] = [
    [0, 33], [1, 40], [1, 35], [1, 35], [4, 59], [4, 39], [6, 58], [7, 46], [8, 39], [7, 39], [10, 46], [10, 46], [10, 44], [8, 44], [4, 58], [0, 35], [25, 44], [8, 59], [10, 46], [10, 46],
    [10, 46], [10, 46], [10, 46], [10, 46], [10, 46], [25, 46], [10, 46], [10, 46], [10, 46], [10, 46], [10, 46], [10, 46], [10, 46], [0, 33], [1, 40], [1, 35], [1, 35], [4, 37], [4, 59], [4, 39],
    [4, 40], [7, 44], [8, 59], [7, 39], [6, 44], [8, 44], [10, 46], [25, 44], [6, 58], [4, 58], [0, 59], [25, 44], [8, 59], [10, 46], [10, 46], [10, 46], [10, 46], [10, 46], [1, 35], [4, 37],
];
/// The first type of each race (0x87f3f0).
const FIRST_TYPE: [u8; 4] = [0, 33, 60, 74];
/// Ships of a class needed per unit of the class matrix (0x88cfa0), and the matrix by target
/// class and buyer class (0x88cfd0, `[target + buyer * 12]`).
const CLASS_MULTIPLIER: [i32; 12] = [1, 1, 1, 1, 2, 8, 10, 0, 0, 8, 0, 10];
const CLASS_MATRIX: [i32; 144] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 3, 3, 3, 3, 3, 3, 3, 1, 1, 3, 3, 1, 3, 3, 3, 3, 2, 2, 2, 1, 1, 2, 2, 3, 3, 3, 3, 3, 2, 2, 2, 1, 1, 2, 2, 3, 3, 3, 3, 3, 3, 1, 1, 1, 1, 2, 1, 3, 3, 3, 3, 3, 3, 3, 1, 1, 3, 1, 1, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 3, 1, 1, 3, 1, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 3, 3, 3, 3, 3, 0, 0, 0, 0, 0, 0,
];
/// `statsBestShipToBuyToKillFleet`'s choices by the buyer's race (0x88cf34..).
const FLEET_KILLERS: [&[u8]; 5] = [&[6, 7, 8, 10, 25], &[37, 39, 40, 44, 46, 58, 59], &[61, 65, 66, 69, 70, 73], &[75, 80, 83, 85, 86, 90, 91], &[94, 95]];

fn rnd(u: &mut Universe) -> u32 {
    u.randy.random(RAN_AI_PLAYER)
}

fn ty(n: u8) -> ShipType {
    ShipType::from_index(n.into()).expect("ship type")
}

fn race_index(r: ShipRace) -> usize {
    r as usize
}

fn race_of_type(u: &Universe, t: ShipType) -> usize {
    u.statics.ship(t).map_or(6, |s| race_index(s.ship_race))
}

fn class_index(u: &Universe, t: ShipType) -> usize {
    u.statics.ship(t).and_then(|s| s.ship_class).map_or(0, |c| c.index())
}

fn rating(t: ShipType) -> f32 {
    RATING.get(t.index()).copied().unwrap_or(1.0)
}

/// `aiuUpdateKnowledgeOfEnemyShips` (0x41f820), the primary enemy's part: every ship of the
/// primary enemy seen in a blob where the player has ships is remembered by type.
pub fn update_knowledge(u: &mut Universe, p: usize) {
    let enemy = ai(u, p).primary_enemy;
    let mut seen = Vec::new();
    for b in super::attack::enemy_blobs(u, p).into_iter().filter(|b| b.own) {
        for id in b.ships {
            let Some(s) = u.ships.get(id) else { continue };
            if matches!(s.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1 | ShipType::dSlipgateSmall | ShipType::dSlipgateMedium | ShipType::dSlipgateLarge) {
                continue;
            }
            if s.player == enemy {
                seen.push((s.ship_type.index(), id));
            }
        }
    }
    let a = ai_mut(u, p);
    if a.known_enemy_ships.len() < ShipType::COUNT {
        a.known_enemy_ships.resize(ShipType::COUNT, Vec::new());
    }
    for (t, id) in seen {
        if !a.known_enemy_ships[t].contains(&id) {
            a.known_enemy_ships[t].push(id);
        }
    }
}

/// `aitAnyTeamOfPlayerAttackingThisShip` (0x417d50).
fn any_team_attacking(u: &Universe, p: usize, id: ShipId) -> bool {
    u.ai.teams.iter().flatten().filter(|(q, _)| *q == p).any(|(_, tm)| {
        tm.moves.iter().any(|m| match &m.kind {
            MoveKind::Attack { targets } | MoveKind::AdvancedAttack { targets, .. } | MoveKind::FlankAttack { targets } => targets.contains(&id),
            _ => false,
        })
    })
}

/// `aiaPriorityShipsConstraints` (0x4011d0): a capital ship or a fighting frigate, not an enemy
/// mothership, that no team of the player is attacking.
fn priority(u: &Universe, p: usize, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    if super::attack::is_enemy_mothership_pub(u, p, id) {
        return false;
    }
    let ok = match u.statics.ship(s.ship_type).and_then(|st| st.ship_class) {
        Some(ShipClass::HeavyCruiser | ShipClass::Carrier | ShipClass::Destroyer | ShipClass::MonsterShip) => true,
        Some(ShipClass::Frigate) => !matches!(s.ship_type, ShipType::sProcessor | ShipType::bDFGFrigate | ShipType::bProcessor | ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate | ShipType::tDFGFrigate),
        _ => false,
    };
    ok && !any_team_attacking(u, p, id)
}

/// The target of types 4 and 5: the first known ship of each type, the most dangerous of those
/// that pass [`priority`] (`statsGetMostDangerousShipNonStatConstraints`).
pub fn find_target(u: &Universe, p: usize) -> Option<ShipId> {
    let a = ai(u, p);
    let mut best: Option<ShipId> = None;
    let mut best_r = -1.0f32;
    for list in &a.known_enemy_ships {
        let Some(&id) = list.first() else { continue };
        if !priority(u, p, id) {
            continue;
        }
        let t = u.ships.ship(id).ship_type;
        let r = if t.index() < 0x3c { rating(t) } else { 1.0 };
        if best_r < r {
            best_r = r;
            best = Some(id);
        }
    }
    best
}

/// The ratio of ratings `statsBestShipsToUseToKillTarget` sums.
fn ratio(u: &Universe, ship: ShipType, target: ShipType) -> f32 {
    let (rs, rt) = (race_of_type(u, ship), race_of_type(u, target));
    if rs <= 1 && rt <= 1 {
        rating(target) / rating(ship)
    } else {
        1.0
    }
}

/// `statsNumShipsNeededToKillTarget` (0x51ff10).
fn num_needed(u: &mut Universe, buy: ShipType, target: ShipType) -> i32 {
    let bc = class_index(u, buy);
    let k = if target.index() < 0x3c { CLASS_MATRIX[class_index(u, target) + bc * 12] } else { (rnd(u) % 9 + 1) as i32 };
    CLASS_MULTIPLIER[bc] * k
}

/// `statsBestShipsToUseToKillTarget` (0x51ffe0): ships of the new ships, the best against the
/// target first with as many of its type as the tables want, until their ratings add up to
/// one; whether that is enough.
fn best_ships_to_use(u: &mut Universe, new_ships: &[ShipId], target: ShipType) -> (Vec<ShipId>, bool) {
    if new_ships.is_empty() {
        return (Vec::new(), false);
    }
    let mut pool: Vec<ShipId> = new_ships.to_vec();
    let mut out = Vec::new();
    loop {
        let mut best = None;
        let mut best_r = -1.0f32;
        for &id in &pool {
            let r = ratio(u, u.ships.ship(id).ship_type, target);
            if best_r < r {
                best_r = r;
                best = Some(id);
            }
        }
        let Some(b) = best else { return (out, false) };
        let bt = u.ships.ship(b).ship_type;
        if race_of_type(u, bt) > 1 {
            return (Vec::new(), false);
        }
        let mut k = num_needed(u, bt, target);
        pool.retain(|&s| s != b);
        out.push(b);
        loop {
            k -= 1;
            if k <= 0 {
                break;
            }
            let Some(i) = pool.iter().position(|&s| u.ships.ship(s).ship_type == bt) else { break };
            out.push(pool.remove(i));
        }
        let sum: f32 = out.iter().fold(0.0f32, |s, &id| ratio(u, u.ships.ship(id).ship_type, target) + s);
        if 1.0 <= 1.0 / (1.0 / sum) {
            return (out, true);
        }
        if pool.is_empty() {
            return (out, false);
        }
    }
}

/// `statsBestShipToBuyToKillShip` (0x51fe10); `None` where the original has no answer (it
/// returns a null pointer there).
fn best_to_buy(u: &Universe, buyer_race: usize, target: ShipType) -> Option<ShipType> {
    let tr = race_of_type(u, target);
    let t = target.index();
    let c = match tr {
        0 => {
            if t > 0x3b {
                return Some(ty(1));
            }
            CAN_COUNTER[t]
        }
        1 => {
            if t > 0x3b {
                return Some(ty(0x23));
            }
            CAN_COUNTER[t + 3]
        }
        _ => return FIRST_TYPE.get(buyer_race).map(|&n| ty(n)),
    };
    if c == 0 {
        return None;
    }
    COUNTER.get(t).and_then(|r| r.get(buyer_race)).map(|&n| ty(n))
}

/// `statsBestShipToBuyToKillFleet` (0x51ff70).
fn best_to_buy_against_fleet(u: &mut Universe, race: usize) -> Option<ShipType> {
    let list = FLEET_KILLERS.get(race)?;
    let r = rnd(u);
    let i = if race == 4 { (r & 1) as usize } else { r as usize % list.len() };
    Some(ty(list[i]))
}

fn get_ships(u: &mut Universe, t: usize, ship_type: ShipType, num: i32) {
    add_move(u, t, AiMove::new(MoveKind::FancyGetShips { primary: ship_type, num, alternatives: Vec::new(), random_alternative: false, priority: 0, var: None }, SAME_FORMATION, 2, true, false));
}

/// The ships of a takeout: the best of the new ships when they are enough (or the player has no
/// fighting mothership), else ships to buy.
fn ships_for(u: &mut Universe, p: usize, t: usize, target: ShipId) {
    let target_type = u.ships.ship(target).ship_type;
    let new_ships = ai(u, p).new_ships.clone();
    let (best, enough) = best_ships_to_use(u, &new_ships, target_type);
    let ms = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| (s.flags, s.ship_type));
    let ms_race = ms.map(|m| race_of_type(u, m.1));
    if enough || ms.is_none() || ms.is_some_and(|m| m.0 & sof::DEAD != 0) || ms_race == Some(race_index(ShipRace::Traders)) {
        for id in best {
            ai_mut(u, p).new_ships.retain(|&s| s != id);
            super::team_add_ship(u, t, id);
        }
        return;
    }
    let race = ms_race.unwrap_or(0);
    let mut buy = best_to_buy(u, race, target_type);
    let mut n = match buy {
        Some(b) => num_needed(u, b, target_type),
        None => 0,
    };
    if n == 0 {
        let fallback = match race {
            0 => Some(7),
            1 => Some(0x2c),
            3 => Some(0x5b),
            4 => Some(0x5f),
            _ => None,
        };
        if let Some(f) = fallback {
            buy = Some(ty(f));
        }
        n = 1;
    }
    if let Some(b) = buy {
        get_ships(u, t, b, n);
    }
}

fn attack_var(u: &Universe, p: usize) -> String {
    ai(u, p).attack.var.clone()
}

/// `aioCreateTakeoutTarget` (0x4106f0).
pub fn takeout_target(u: &mut Universe, p: usize, t: usize, target: ShipId) {
    if race_of_type(u, u.ships.ship(target).ship_type) > 1 {
        return;
    }
    ships_for(u, p, t, target);
    let var = attack_var(u, p);
    add_move(u, t, AiMove::new(MoveKind::VarDec { var }, SAME_FORMATION, 2, true, false));
    add_move(u, t, AiMove::new(MoveKind::TempGuard, GUARD_FORMATION, GUARD_TACTICS, true, false));
    let mut m = AiMove::new(MoveKind::Attack { targets: vec![target] }, ATTACK_FORMATION, 2, true, false);
    m.events.getting_rocked = Some(Event { handler: Handler::GenericGettingRocked, once: true, fired: false, percent: 0, base: 0, per_ship: false });
    add_move(u, t, m);
    add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aioCreateFancyTakeoutTarget` (0x410890): the takeout, and an escort team when the target
/// has defenders; the two wait for each other before setting out.
pub fn fancy_takeout_target(u: &mut Universe, p: usize, t: usize, target: ShipId) {
    ships_for(u, p, t, target);
    let defenders = {
        let mut v = super::attack::find_nearby_enemy_ships(u, p, target, ESCORT_RANGE);
        v.retain(|&s| super::defense::is_dangerous(u, s));
        v
    };
    let mut escort = None;
    if !defenders.is_empty() {
        let race = race_index(u.players[p].race);
        let mut buy = best_to_buy_against_fleet(u, race);
        let mut n = buy.map_or(0, |b| CLASS_MULTIPLIER[class_index(u, b)] * 3);
        if n > ESCORT_MAX {
            buy = best_to_buy_against_fleet(u, race);
            n = buy.map_or(0, |b| CLASS_MULTIPLIER[class_index(u, b)] * 3).min(ESCORT_MAX);
        }
        if let (Some(b), true) = (buy, n != 0) {
            escort = Some((b, n));
        }
    }
    let var = attack_var(u, p);
    let Some((buy, n)) = escort else {
        add_move(u, t, AiMove::new(MoveKind::VarDec { var }, SAME_FORMATION, 2, true, false));
        add_move(u, t, AiMove::new(MoveKind::TempGuard, GUARD_FORMATION, GUARD_TACTICS, true, false));
        let mut m = AiMove::new(MoveKind::Attack { targets: vec![target] }, ATTACK_FORMATION, 2, true, false);
        m.events.getting_rocked = Some(Event { handler: Handler::GenericGettingRocked, once: true, fired: false, percent: 0, base: 0, per_ship: false });
        add_move(u, t, m);
        add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
        return;
    };
    let manager = u.ai.teams[t].as_ref().expect("team").1.manager;
    let e = super::team_create(u, p, manager, "");
    if let Some(Some((_, tm))) = u.ai.teams.get_mut(e) {
        tm.cooperating = Some(t);
    }
    let mine = var_create(u, 0);
    let theirs = var_create(u, 0);
    // The escort.
    get_ships(u, e, buy, n);
    add_move(u, e, AiMove::new(MoveKind::VarSet { var: theirs.clone(), value: 1 }, SAME_FORMATION, 2, false, false));
    let mut w = AiMove::new(MoveKind::VarWait { var: mine.clone(), value: 1 }, SAME_FORMATION, 2, true, false);
    w.team_died = Some(Handler::Patrol);
    add_move(u, e, w);
    add_move(u, e, AiMove::new(MoveKind::VarDestroy { var: mine.clone() }, SAME_FORMATION, 2, false, false));
    add_move(u, e, AiMove::new(MoveKind::GuardCooperatingTeam, SAME_FORMATION, 2, false, false));
    add_move(u, e, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
    // The takeout.
    add_move(u, t, AiMove::new(MoveKind::VarSet { var: mine, value: 1 }, SAME_FORMATION, 2, false, false));
    let mut w = AiMove::new(MoveKind::VarWait { var: theirs.clone(), value: 1 }, SAME_FORMATION, 2, true, false);
    w.team_died = Some(Handler::Patrol);
    add_move(u, t, w);
    add_move(u, t, AiMove::new(MoveKind::VarDestroy { var: theirs }, SAME_FORMATION, 2, false, false));
    add_move(u, t, AiMove::new(MoveKind::VarDec { var }, SAME_FORMATION, 2, true, false));
    add_move(u, t, AiMove::new(MoveKind::TempGuard, GUARD_FORMATION, GUARD_TACTICS, true, false));
    add_move(u, t, AiMove::new(MoveKind::Attack { targets: vec![target] }, ATTACK_FORMATION, 2, true, false));
    add_move(u, t, AiMove::new(MoveKind::Done, SAME_FORMATION, 2, false, false));
}

/// `aimProcessGuardCooperatingTeam` (0x40bfa0): the team guards its cooperating team's ships.
pub fn process_guard_cooperating_team(u: &mut Universe, t: usize) -> bool {
    let Some((_, tm)) = u.ai.teams.get(t).and_then(|x| x.as_ref()) else { return true };
    if tm.ships.is_empty() {
        return false;
    }
    let ships = tm.ships.clone();
    let theirs = tm.cooperating.and_then(|c| u.ai.teams.get(c)).and_then(|x| x.as_ref()).map(|x| x.1.ships.clone()).unwrap_or_default();
    if !theirs.is_empty() {
        let ok: Vec<ShipId> = ships.into_iter().filter(|&s| team::wrap_ok_pub(u, s)).collect();
        if !ok.is_empty() {
            crate::command::cl_protect(u, &ok, &theirs);
        }
    }
    true
}
