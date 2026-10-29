//! What a battle ping makes the fleet say when it is evaluated (`pingBattlePingEvaluate`
//! 0x4e4710): with ships on both sides `battlePingEvaluate` (0x424130), with no enemies left
//! `battlePingEvaluateNoEnemies` (0x423fa0), and with no friendlies nothing. A battle ping's
//! losses while it lives are counted by `battleAccountForLosses` (0x423e70).
//!
//! The pings themselves belong to the front end (the sensors' battle pings): it hands each
//! one here when it evaluates it. Everything is rolled on the battle chatter random stream,
//! so nothing here changes the game.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_BATTLE;
use hwc_math::Vector;

use crate::battle_chatter::{attempt_linked as say, bce, can_chatter_at_this_time as can, fleet_attempt as fleet, CLASS_DEPENDENCIES};
use crate::object::{sof, ShipId};
use crate::universe::Universe;

/// `battleping` (the ping's user data, +0x00..+0x28), less its ship list.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BattleStats {
    /// +0x00 `nEnemyShips`, +0x04 `nFriendlyShips`.
    pub enemies: i32,
    pub friendlies: i32,
    /// +0x08: the ping's radius.
    pub radius: f32,
    /// +0x10..+0x18: enemy firepower and value lost, and when.
    pub enemy_power_lost: f32,
    pub enemy_value_lost: f32,
    pub enemies_lost_at: f32,
    /// +0x1c..+0x24: the same for friendly ships.
    pub friendly_power_lost: f32,
    pub friendly_value_lost: f32,
    pub friendlies_lost_at: f32,
    /// +0x28: when it was last evaluated (it is evaluated at most every 8 s).
    pub evaluated_at: f32,
}

/// A ship lost from a battle ping, as `battleAccountForLosses` reads it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Loss {
    /// It was the local player's.
    pub friendly: bool,
    pub fire_power: f32,
    pub value: f32,
}

/// The loss a dying ship is to the battle pings it is in.
pub fn loss_of(u: &Universe, ship: ShipId) -> Option<Loss> {
    let s = u.ships.get(ship)?;
    let st = u.statics.ship(s.ship_type)?;
    Some(Loss { friendly: s.player == u.base.cur_player, fire_power: fire_power(u, s.ship_type), value: st.build_cost as f32 })
}

/// `battleAccountForLosses`: losses fade with the time since the last one
/// (`/ (now - then)^batExpLossDevalue`) before the new one is added.
pub fn account_for_losses(u: &Universe, b: &mut BattleStats, loss: Loss) {
    let now = u.total_time_elapsed;
    let exp = f64::from(u.statics.battle_chatter.exp_loss_devalue);
    let (power, value, at) = if loss.friendly {
        (&mut b.friendly_power_lost, &mut b.friendly_value_lost, &mut b.friendlies_lost_at)
    } else {
        (&mut b.enemy_power_lost, &mut b.enemy_value_lost, &mut b.enemies_lost_at)
    };
    let since = st(ld(now) - ld(*at));
    if *value != 0.0 && since != 0.0 {
        let d = f64::from(since).powf(exp);
        *power = (f64::from(*power) / d) as f32;
        *value = (f64::from(*value) / d) as f32;
    }
    *power = st(ld(loss.fire_power) + ld(*power));
    *value = st(ld(loss.value) + ld(*value));
    *at = now;
}

/// `gunShipFirePower` (0x488180): each gun's damage (its weapon's damage x `DamageMult`,
/// `suGunUpgradedFirePower`) per `FireTime` (1/16 s when zero). The heavy interceptor types
/// fire one gun at a time, at their `gunsFireTime`.
pub fn fire_power(u: &Universe, t: crate::statics::ShipType) -> f32 {
    let Some(guns) = u.statics.ship(t).and_then(|s| s.guns.as_ref()) else { return 0.0 };
    let interceptor = matches!(t.index(), 0x44 | 0x52 | 0x5d);
    let total = if interceptor { u.combat.get(t).and_then(|c| c.interceptor.as_ref()).map_or(0.0, |i| i.fire_time) } else { 0.0 };
    let mut power = 0.0f32;
    for g in guns {
        let fire = if g.fire_time == 0.0 { 0.0625 } else { g.fire_time };
        let w = crate::combat_statics::WEAPON_DAMAGE.get(g.start_upgrade_level.index()).map_or(0.0, |r| r[0]);
        let damage = ld(w) * ld(g.damage_mult);
        let this = if interceptor { ld(fire) / (guns.len() as f64 * ld(total)) * (damage / ld(fire)) } else { damage / ld(fire) };
        power = st(this + ld(power));
    }
    power
}

/// `battleShipOnMyTail` (0x423d00): `bad` is behind `me` within `batBogeyAspect` of its tail
/// and has `me` in its sights.
pub fn ship_on_my_tail(u: &Universe, me: ShipId, bad: ShipId) -> bool {
    let (Some(m), Some(b)) = (u.ships.get(me), u.ships.get(bad)) else { return false };
    let aspect = u.statics.battle_chatter.bogey_aspect;
    let d = b.posinfo.position - m.posinfo.position;
    let p = m.rotinfo.coordsys.transpose().mul_vec(d);
    if !(p.z < 0.0 && p.x.abs() + p.y.abs() < -(aspect * p.z)) {
        return false;
    }
    let q = b.rotinfo.coordsys.transpose().mul_vec(-d);
    q.z > 0.0 && q.x.abs() + q.y.abs() < aspect * q.z
}

fn pick(u: &mut Universe, ships: &[ShipId]) -> ShipId {
    let r = u.randy.random(RAN_BATTLE) & 0x7fff;
    ships[r as usize % ships.len()]
}

fn is_local(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.player == u.base.cur_player)
}

/// `battleMouthPieceFind`: up to `batMouthpieceTries` random ships of the ping, the first of
/// the local player's.
fn mouth_piece(u: &mut Universe, ships: &[ShipId]) -> Option<ShipId> {
    for _ in 0..u.statics.battle_chatter.mouthpiece_tries.max(0) {
        let m = pick(u, ships);
        if is_local(u, m) {
            return Some(m);
        }
    }
    None
}

/// `battleMouthPieceFindAnother`: the same, but `first` itself also ends the search (as in
/// the original, it may answer itself).
fn another(u: &mut Universe, first: ShipId, ships: &[ShipId]) -> Option<ShipId> {
    if ships.is_empty() {
        return None;
    }
    let tries = u.statics.battle_chatter.mouthpiece_tries;
    let mut m = pick(u, ships);
    let mut n = 1;
    while n <= tries {
        if is_local(u, m) || m == first {
            return Some(m);
        }
        m = pick(u, ships);
        n += 1;
    }
    None
}

fn type_of(u: &Universe, id: ShipId) -> i32 {
    u.ships.get(id).map_or(-1, |s| s.ship_type.index() as i32)
}

fn class_of(u: &Universe, id: ShipId) -> Option<usize> {
    let s = u.ships.get(id)?;
    u.statics.ship(s.ship_type)?.ship_class.map(|c| c.index())
}

fn group_of(u: &Universe, id: ShipId) -> u8 {
    u.speech_gate.as_ref().and_then(|g| g.groups.get(&id).copied()).unwrap_or(0)
}

/// The single-player mission (13) during which the winning reports stay quiet.
fn quiet_mission(u: &Universe) -> bool {
    u.single_player && u.base.options.mission == 13
}

/// `pingBattlePingEvaluate` on an evaluation: what the battle makes the fleet say.
pub fn evaluate(u: &mut Universe, centre: Vector, b: &mut BattleStats, ships: &[ShipId]) {
    if b.enemies == 0 {
        if b.friendlies != 0 {
            evaluate_no_enemies(u, b, ships);
        }
    } else if b.friendlies != 0 {
        evaluate_battle(u, centre, b, ships);
    }
}

/// `battlePingEvaluateNoEnemies`: friendly or allied fire is reported; otherwise, if enemies
/// were lost, the battle is won.
fn evaluate_no_enemies(u: &mut Universe, b: &BattleStats, ships: &[ShipId]) {
    let mut friendly_fire = false;
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        if class_of(u, id).is_none_or(|c| c == 0) || s.player != u.base.cur_player {
            continue;
        }
        let (bits, pos) = (s.chatter_bits, s.posinfo.position);
        let group = group_of(u, id);
        if bits & 1 != 0 {
            if group == 0 {
                fleet(u, None, bce::FRIENDLY_FIRE, -1, None);
            } else {
                fleet(u, None, bce::GROUP_FRIENDLY_FIRE, i32::from(group), None);
            }
            u.ships.ship_mut(id).chatter_bits &= !1;
            friendly_fire = true;
        }
        if bits & 4 != 0 {
            if group == 0 {
                fleet(u, None, bce::UNDER_ATTACK_FROM_ALLY, -1, Some(pos));
            } else {
                fleet(u, None, bce::GROUP_ATTACK_FROM_ALLY, i32::from(group), None);
            }
            u.ships.ship_mut(id).chatter_bits &= !4;
            friendly_fire = true;
        }
    }
    if quiet_mission(u) || friendly_fire || b.enemy_value_lost == 0.0 || ships.is_empty() {
        return;
    }
    let m = mouth_piece(u, ships);
    if fleet(u, None, bce::BATTLE_WON, -1, None).is_none() {
        if let Some(m) = m {
            let group = group_of(u, m);
            if group != 0 {
                let at = u.ships.get(m).map(|s| s.posinfo.position);
                fleet(u, None, bce::GROUP_VICTORY, i32::from(group), at);
            }
        }
    }
}

/// `battlePingEvaluate`: each of the local player's ships may speak of wingmen, tails, hits,
/// lost wingmen, damage and retreat; then the battle status number (friendly over enemy
/// standing, from firepower and value) picks a report.
fn evaluate_battle(u: &mut Universe, centre: Vector, b: &mut BattleStats, ships: &[ShipId]) {
    let now = u.total_time_elapsed;
    if st(ld(b.evaluated_at) + 8.0) > now {
        return;
    }
    b.evaluated_at = now;
    let cur = u.base.cur_player;
    if !ships.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.player == cur && s.special_flags2 & 0x10_0000 == 0)) {
        return;
    }
    let (mut friendly_power, mut friendly_value, mut enemy_power, mut enemy_value) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        if s.flags & sof::DEAD != 0 || class_of(u, id).is_none_or(|c| c == 0) {
            continue;
        }
        let (t, player) = (s.ship_type, s.player);
        let power = fire_power(u, t);
        let value = u.statics.ship(t).map_or(0, |st| st.build_cost) as f32;
        if player != cur {
            if crate::shockwave::is_ally(u, id, cur) {
                friendly_power = st(ld(power) + ld(friendly_power));
                friendly_value = st(ld(value) + ld(friendly_value));
            } else {
                enemy_power = st(ld(power) + ld(enemy_power));
                enemy_value = st(ld(value) + ld(enemy_value));
            }
            continue;
        }
        friendly_power = st(ld(power) + ld(friendly_power));
        friendly_value = st(ld(value) + ld(friendly_value));
        ship_events(u, centre, b, ships, id);
    }

    let tw = u.statics.battle_chatter.clone();
    let [sfp, sfv, sep, sev] = tw.standing;
    let friendly = st(ld(sfv) * ld(friendly_value) + ld(sfp) * ld(friendly_power));
    let enemy = st(ld(sev) * ld(enemy_value) + ld(sep) * ld(enemy_power));
    // Recent losses change the chance of the winning and losing reports for this evaluation
    // (Cataclysm sets them to the standing weight, or 1 with no value on that side).
    let saved: Vec<f64> = [bce::LOSING_BATTLE, bce::LOSING_BADLY, bce::WINNING_BATTLE, bce::KICKING_BUTT]
        .iter()
        .map(|&k| tw.events[k].random_weight)
        .collect();
    let statics = std::sync::Arc::make_mut(&mut u.statics);
    let t = &mut statics.battle_chatter;
    if b.friendly_value_lost != 0.0 {
        let w = if friendly_value == 0.0 { 1.0 } else { f64::from(sfv) };
        t.events[bce::LOSING_BATTLE].random_weight = w;
        t.events[bce::LOSING_BADLY].random_weight = w;
    }
    if b.enemy_value_lost != 0.0 {
        let w = if enemy_value == 0.0 { 1.0 } else { f64::from(sev) };
        t.events[bce::WINNING_BATTLE].random_weight = w;
        t.events[bce::KICKING_BUTT].random_weight = w;
    }
    let standing = if enemy > 0.0 { st(ld(friendly) / ld(enemy)) } else { -1.0 };
    report(u, &tw.bsn, standing, ships);
    // The saved weights go back through a float, as in the original.
    let t = &mut std::sync::Arc::make_mut(&mut u.statics).battle_chatter;
    for (k, w) in [bce::LOSING_BATTLE, bce::LOSING_BADLY, bce::WINNING_BATTLE, bce::KICKING_BUTT].into_iter().zip(saved) {
        t.events[k].random_weight = f64::from(w as f32);
    }
}

/// The per-ship part of `battlePingEvaluate` for one of the local player's ships.
fn ship_events(u: &mut Universe, centre: Vector, b: &BattleStats, ships: &[ShipId], id: ShipId) {
    let s = u.ships.ship(id);
    let evasive_wingman = if s.tactics == 0 { s.attack.wingman } else { None };
    let (fired_upon, firing_at_us, attacked) = (s.recently_fired_upon, s.firing_at_us, s.recently_attacked);
    let own_type = s.ship_type.index() as i32;
    if evasive_wingman.is_some() && can(u, bce::WINGMAN, Some(id)) {
        say(u, None, bce::WINGMAN, id, -1);
    }
    // Someone on this ship's tail: its wingman or another ship calls it, and it answers.
    if fired_upon != 0 && firing_at_us.is_some_and(|f| ship_on_my_tail(u, id, f)) {
        let h = if let Some(w) = evasive_wingman {
            if can(u, bce::LEADER_CHASED, Some(w)) {
                say(u, None, bce::LEADER_CHASED, w, -1)
            } else {
                None
            }
        } else {
            match another(u, id, ships) {
                Some(m) if can(u, bce::WINGMAN_CHASED, Some(m)) => say(u, None, bce::WINGMAN_CHASED, m, -1),
                _ => None,
            }
        };
        if h.is_some() && can(u, bce::WINGMAN_CHASED_RSP, Some(id)) {
            say(u, h, bce::WINGMAN_CHASED_RSP, id, own_type);
        }
    }
    // Hit hard just now (`BAT_ReallyRecentAttack`): someone answers.
    if attacked > 0x48 && can(u, bce::WINGMAN_HIT, Some(id)) {
        if let Some(h) = say(u, None, bce::WINGMAN_HIT, id, -1) {
            let m = match evasive_wingman {
                Some(w) => Some(w),
                None => another(u, id, ships),
            };
            if let Some(m) = m {
                if can(u, bce::WINGMAN_HIT_RSP, Some(m)) {
                    say(u, Some(h), bce::WINGMAN_HIT_RSP, m, type_of(u, m));
                }
            }
        }
    }
    if u.ships.ship(id).chatter_bits & 2 != 0 {
        if can(u, bce::WINGMAN_DIES, Some(id)) {
            say(u, None, bce::WINGMAN_DIES, id, own_type);
        }
        u.ships.ship_mut(id).chatter_bits &= !2;
    }
    // Damage in the red: the ship says so, or else fleet command names it.
    if u.ships.ship(id).chatter_bits & 8 != 0 {
        let reports = class_of(u, id).and_then(|c| CLASS_DEPENDENCIES.get(c)).is_some_and(|d| d & 2 != 0);
        if reports && u.ships.ship(id).special_flags2 & 0x10_0000 == 0 {
            let said = can(u, bce::SHIP_DAMAGED, Some(id)) && say(u, None, bce::SHIP_DAMAGED, id, own_type).is_some();
            if !said && can(u, bce::DAMAGED_SPECIFIC_SHIP, Some(id)) {
                let at = u.ships.get(id).map(|s| s.posinfo.position);
                fleet(u, None, bce::DAMAGED_SPECIFIC_SHIP, own_type, at);
            }
        }
        u.ships.ship_mut(id).chatter_bits &= !8;
    }
    // Moving out of the battle is a retreat.
    let dest = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).filter(|c| c.order == crate::command::order::MOVE).and_then(|c| {
        usize::try_from(c.waypoint_index).ok().and_then(|i| c.waypoints.get(i)).copied()
    });
    if let Some(d) = dest {
        let v = centre - d;
        let dist = (f64::from(v.x) * f64::from(v.x) + f64::from(v.y) * f64::from(v.y) + f64::from(v.z) * f64::from(v.z)).sqrt();
        if ld(u.statics.battle_chatter.retreat_modifier) * ld(b.radius) < dist {
            u.ships.ship_mut(id).retreating = true;
            if let Some(h) = fleet(u, None, bce::RETREAT, -1, None) {
                fleet(u, Some(h), bce::RETREAT_RSP, -1, None);
            }
        }
    }
}

/// The battle report for battle status number `bsn` (thresholds `t`: kicking butt, winning,
/// neutral, losing), said by fleet intelligence and answered by one of the local ships.
fn report(u: &mut Universe, t: &[f32; 4], bsn: f32, ships: &[ShipId]) {
    let Some(m) = mouth_piece(u, ships) else { return };
    let answer = |u: &mut Universe, h: u32, k: usize| {
        if let Some(m2) = another(u, m, ships) {
            say(u, Some(h), k, m2, type_of(u, m2));
        }
    };
    if bsn > t[0] {
        if quiet_mission(u) {
            return;
        }
        if let Some(h) = fleet(u, None, bce::START_BATTLE_ADVANTAGED, -1, None) {
            fleet(u, Some(h), bce::START_BATTLE_ADVANTAGED_RSP, -1, None);
        } else if let Some(h) = fleet(u, None, bce::KICKING_BUTT, -1, None) {
            answer(u, h, bce::KICKING_BUTT_RSP);
        }
    } else if bsn > t[1] {
        if let Some(h) = fleet(u, None, bce::START_BATTLE_ADVANTAGED, -1, None) {
            fleet(u, Some(h), bce::START_BATTLE_ADVANTAGED_RSP, -1, None);
        } else if let Some(h) = fleet(u, None, bce::WINNING_BATTLE, -1, None) {
            answer(u, h, bce::WINNING_BATTLE_RSP);
        } else if let Some(h) = fleet(u, None, bce::POSITIVE_BATTLE, -1, None) {
            say(u, Some(h), bce::POSITIVE_BATTLE_MORE, m, type_of(u, m));
        }
    } else if bsn > t[2] {
        if let Some(h) = fleet(u, None, bce::START_BATTLE_FAIR, -1, None) {
            fleet(u, Some(h), bce::START_BATTLE_FAIR_RSP, -1, None);
        } else if fleet(u, None, bce::NEUTRAL_BATTLE, -1, None).is_none() {
            fleet(u, None, bce::NEUTRAL_BATTLE_MORE, -1, None);
        }
    } else if bsn > t[3] {
        if mothership_under_attack(u, m, ships) {
            return;
        }
        if let Some(h) = fleet(u, None, bce::START_BATTLE_DISADVANT, -1, None) {
            fleet(u, Some(h), bce::START_BATTLE_DISADVANT_RSP, -1, None);
        } else if let Some(h) = fleet(u, None, bce::LOSING_BATTLE, -1, None) {
            answer(u, h, bce::LOSING_BATTLE_RSP);
        } else if fleet(u, None, bce::LOSING_BATTLE_MORE, -1, None).is_none() {
            let h = fleet(u, None, bce::NEGATIVE_BATTLE, -1, None);
            let m2 = another(u, m, ships);
            if let (Some(m2), Some(_)) = (m2, h) {
                if can(u, bce::NEGATIVE_BATTLE_MORE, Some(m2)) && say(u, None, bce::NEGATIVE_BATTLE_MORE, m2, type_of(u, m2)).is_none() {
                    fleet(u, None, bce::NEGATIVE_BATTLE_MORE2, -1, None);
                }
            }
        }
    } else if bsn > 0.0 {
        if mothership_under_attack(u, m, ships) {
            return;
        }
        if let Some(h) = fleet(u, None, bce::START_BATTLE_DISADVANT, -1, None) {
            fleet(u, Some(h), bce::START_BATTLE_DISADVANT_RSP, -1, None);
            return;
        }
        let h = match fleet(u, None, bce::LOSING_BADLY, -1, None) {
            Some(h) => Some(h),
            None => say(u, None, bce::LOSING_BADLY_MORE, m, type_of(u, m)),
        };
        if let Some(h) = h {
            answer(u, h, bce::LOSING_BADLY_RSP);
        }
    }
}

/// Losing with the mothership in the battle: it says it is under heavy fire and the mouth
/// piece answers (with the mothership's type, as in the original). Whether it was said.
fn mothership_under_attack(u: &mut Universe, m: ShipId, ships: &[ShipId]) -> bool {
    let cur = u.base.cur_player;
    let Some(ms) = u.base.players.get(cur).and_then(|p| p.mothership) else { return false };
    if !ships.contains(&ms) || !can(u, bce::MOTHERSHIP_LARGE_ATTACK, Some(ms)) {
        return false;
    }
    let ms_type = type_of(u, ms);
    let Some(h) = say(u, None, bce::MOTHERSHIP_LARGE_ATTACK, ms, ms_type) else { return false };
    if can(u, bce::MOTHERSHIP_LARGE_ATTACK_RSP, Some(m)) {
        say(u, Some(h), bce::MOTHERSHIP_LARGE_ATTACK_RSP, m, ms_type);
    }
    true
}

/// `battleShipDyingWithTimeToScream` (0x424dc0), as a ship dies (`univDeleteDeadShip` for a
/// plain death, and the effects' `deathCries`): a capital ship, frigate or resourcer cries
/// out and fleet command reports the loss; a corvette or fighter's cry is a lost wingman's.
/// With the camera far from everything (`closest >= 5000` on the main screen) the capital
/// cry is queued straight away, from nowhere.
pub fn ship_dying(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.special_flags2 & 0x10_0000 != 0 {
        return;
    }
    let (t, pos, local) = (s.ship_type.index() as i32, s.posinfo.position, s.player == u.chatter.local_player);
    match class_of(u, id) {
        Some(1 | 2 | 3 | 4 | 7) => {
            if !can(u, bce::CAPITAL_SHIP_DIES, Some(id)) {
                return;
            }
            if u.speech_gate.as_ref().is_some_and(|g| g.far_from_camera) {
                if local {
                    let e = u.statics.battle_chatter.events[bce::CAPITAL_SHIP_DIES].event;
                    crate::speech::queue_linked(u, None, e, t, None);
                }
                return;
            }
            if let Some(h) = say(u, None, bce::CAPITAL_SHIP_DIES, id, t) {
                fleet(u, Some(h), bce::SHIP_DIES_REPORT, t, Some(pos));
            }
        }
        Some(5 | 6 | 11) => {
            let cried = can(u, bce::CAPITAL_SHIP_DIES, Some(id)) && say(u, None, bce::CAPITAL_SHIP_DIES, id, t).is_some();
            if !cried && can(u, bce::WINGMAN_DIES, Some(id)) {
                say(u, None, bce::WINGMAN_DIES, id, t);
            }
        }
        _ => {}
    }
}

/// `battleLaunchWelcomeExchange` (0x424f10), when a ship of class `class` and type `t` is
/// built: fleet command announces a capital ship launch and welcomes it, or else just
/// names it (`0x200103d`).
pub fn launch_welcome(u: &mut Universe, class: usize, t: i32) {
    if CLASS_DEPENDENCIES.get(class).is_none_or(|d| d & 1 == 0) {
        return;
    }
    if let Some(h) = fleet(u, None, bce::CAP_SHIP_LAUNCHED, -1, None) {
        fleet(u, Some(h), bce::CAP_SHIP_WELCOMED, -1, None);
    } else {
        crate::speech::queue_linked(u, None, 0x200_103d, t, None);
    }
}
