//! The heavy corvette's burst attack (`HeavyCorvette.c`, 0x5af820..0x5afd10): b/kHeavyCorvette.
//!
//! Given a special target, the corvette takes the middle of the targets as a point in space,
//! charges for `burstChargeTime` while it flies within `burstRange` of it and turns to face it,
//! then every gun fires one burst bullet at that point. A burst bullet flies at `burstSpeed` and
//! at the end of its life bursts: ships within `burstRadius` are pushed, rocked and hurt
//! ([`crate::damage::point_explosion_in_space`]). The guns then cool for `burstCoolDownTime`.
//!
//! As in Cataclysm, only the AI and the mission scripts order this: the player's special menu has
//! no item for it.

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::object::{Ship, ShipId};
use crate::ships::ShipSpec;
use crate::universe::Universe;

/// `BURST_*`, the burst states (+0x640).
pub mod state {
    pub const BEGIN: u32 = 0;
    pub const FLY_TO_FIRE_POINT: u32 = 1;
    pub const FIRE: u32 = 2;
}

/// `specialFlags` while the guns fire a burst (`SPECIAL_BurstFiring`).
pub const BURST_FIRING: u32 = 0x40;

/// `HeavyCorvetteSpec`.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HeavyCorvetteSpec {
    /// +0x640.
    pub burst_state: u32,
    /// +0x644.
    pub cooldown: bool,
    /// +0x648: where the burst goes.
    pub burst_fire_vector: Vector,
    /// +0x654: charge built up.
    pub burst_charge_state: f32,
    /// +0x658: time left to cool.
    pub burst_charge_state2: f32,
}

/// The ship's state; ships from a saved game older than this module get theirs here.
fn spec_mut(s: &mut Ship) -> &mut HeavyCorvetteSpec {
    if !matches!(s.spec, ShipSpec::HeavyCorvette(_)) {
        s.spec = ShipSpec::HeavyCorvette(HeavyCorvetteSpec::default());
    }
    match &mut s.spec {
        ShipSpec::HeavyCorvette(c) => c,
        _ => unreachable!(),
    }
}

/// `HeavyCorvetteInit` (0x5af870).
pub fn init(u: &mut Universe, id: ShipId) {
    u.ships.ship_mut(id).spec = ShipSpec::HeavyCorvette(HeavyCorvetteSpec::default());
}

/// `heavyCorvetteOrderChanged` (0x5afd10).
pub fn order_changed(u: &mut Universe, id: ShipId) {
    spec_mut(u.ships.ship_mut(id)).burst_state = state::BEGIN;
}

/// `HeavyCorvetteHouseKeep` (0x5afcc0).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let dt = u.phys_time_elapsed;
    let c = spec_mut(u.ships.ship_mut(id));
    if c.cooldown {
        c.burst_charge_state2 = st(ld(c.burst_charge_state2) - ld(dt));
        if c.burst_charge_state2 <= 0.0 {
            c.burst_charge_state = 0.0;
            c.cooldown = false;
        }
    }
}

/// `aitrackHeadingFunc(ship, heading, accuracy, accuracy, 0, AITRACKHEADING_IGNOREUPVEC)`.
fn track(u: &mut Universe, id: ShipId, point: Vector, accuracy: f32) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let p = s.posinfo.position;
    let mut heading = Vector::new(st(ld(point.x) - ld(p.x)), st(ld(point.y) - ld(p.y)), st(ld(point.z) - ld(p.z)));
    heading.normalize();
    crate::aitrack::heading(s, stat, &tw, dt, &mut heading, accuracy, accuracy, 0.0, 1)
}

/// `doBurstFire` (0x5afb10): true when any gun fired. The original works out a bullet life here
/// (+0x65c) that Cataclysm's bullets no longer read: they keep their gun's.
fn burst_fire(u: &mut Universe, id: ShipId) -> bool {
    let point = spec_mut(u.ships.ship_mut(id)).burst_fire_vector;
    track(u, id, point, 0.9999);
    u.ships.ship_mut(id).special_flags |= BURST_FIRING;
    let fired = crate::gun::shoot_guns_at_point(u, id, point);
    u.ships.ship_mut(id).special_flags &= !BURST_FIRING;
    if fired {
        let cool = u.tweaks.combat.burst_cool_down_time;
        let c = spec_mut(u.ships.ship_mut(id));
        c.cooldown = true;
        c.burst_charge_state2 = cool;
    }
    fired
}

/// `HeavyCorvetteSpecialTarget` (0x5af8a0). True when the special is over (no targets left).
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    if spec_mut(u.ships.ship_mut(id)).cooldown {
        let s = u.ships.ship_mut(id);
        if let Some(stat) = statics.ship(s.ship_type) {
            crate::aitrack::steady_ship(s, stat, &tw, dt);
        }
        return false;
    }
    if targets.is_empty() {
        return true;
    }
    match spec_mut(u.ships.ship_mut(id)).burst_state {
        state::BEGIN => {
            let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
            for t in targets.iter().filter_map(|&t| crate::target::get(u, t)) {
                x = st(ld(x) + ld(t.coll.coll_position.x));
                y = st(ld(y) + ld(t.coll.coll_position.y));
                z = st(ld(z) + ld(t.coll.coll_position.z));
            }
            let inv = st(1.0 / ld(targets.len() as f32));
            let point = Vector::new(st(ld(inv) * ld(x)), st(ld(inv) * ld(y)), st(ld(inv) * ld(z)));
            {
                use crate::battle_chatter::{attempt, bce, can_chatter_at_this_time};
                if u.ships.ship(id).player == 0 && can_chatter_at_this_time(u, bce::FIGHTER_BURST_ATTACK, Some(id)) {
                    let t = u.ships.ship(id).ship_type.index() as i32;
                    attempt(u, bce::FIGHTER_BURST_ATTACK, id, t);
                }
            }
            let c = spec_mut(u.ships.ship_mut(id));
            c.burst_fire_vector = point;
            c.burst_state = state::FLY_TO_FIRE_POINT;
        }
        state::FLY_TO_FIRE_POINT => {
            let charge_time = u.tweaks.combat.burst_charge_time;
            let range = u.tweaks.combat.burst_range;
            let c = spec_mut(u.ships.ship_mut(id));
            let charged = charge_time <= c.burst_charge_state;
            if !charged {
                c.burst_charge_state = st(ld(dt) + ld(c.burst_charge_state));
            }
            let point = c.burst_fire_vector;
            let aimed = track(u, id, point, 0.99);
            if !crate::aitrack::move_reached_destination(u.ships.ship(id), point, range) {
                crate::aiship::fly_to_point_avoiding_objs(u, id, Some(point), 4, 0.0, None);
                return false;
            }
            let s = u.ships.ship_mut(id);
            if let Some(stat) = statics.ship(s.ship_type) {
                crate::aitrack::zero_velocity(s, stat, &tw, dt);
            }
            if aimed && charged {
                spec_mut(u.ships.ship_mut(id)).burst_state = state::FIRE;
            }
        }
        _ => {
            if burst_fire(u, id) {
                spec_mut(u.ships.ship_mut(id)).burst_state = state::BEGIN;
            }
        }
    }
    false
}
