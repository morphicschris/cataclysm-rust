//! Missile ships (`MissileDestroyer` 0x5b5db0..0x5b5f00, `P1MissileCorvette` 0x5b8bd0..0x5b8d70,
//! `CruiseMissile` 0x5aa9c0..): tMissileDestroyer, bMissileCorvette, p1MissileCorvette,
//! bCruiseMissile.
//!
//! * The missile destroyer attacks like any capital ship and fires its launchers with the
//!   default fire routine. Its special targeting fires volleys (every `MissileVolleyTime`): each
//!   launcher in range of the current target fires one missile at it and the targets take
//!   turns; the ship stops while firing, and when out of range it flies to the first target.
//!   Missiles regenerate one at a time (`MissileRegenerateTime`), but not until
//!   `MissileLagVolleyTime` after the last volley.
//! * The missile corvettes side-step attack; their volleys need no range check.
//! * The cruise missile flies at its target and, within 5 km, turns kamikaze.
//!
//! These are the only missile regeneration in the game: other launchers (the Acolytes' link
//! missiles) are only refilled by docking.

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::aiship::flags as fly;
use crate::object::{special, ShipId};
use crate::statics::{GunType, ShipStaticInfo};
use crate::universe::Universe;

use super::{Module, ShipSpec};

/// Volley states of the missile destroyer (+0x648).
pub mod volley {
    pub const BEGIN: i32 = 10;
    pub const NOT_BEGIN: i32 = 20;
}

/// Distance beyond which a cruise missile only flies at its target (5 km).
const CRUISE_KAMIKAZE_RANGE: f64 = 5000.0;
/// The cruise missile's speed limit in its approach (0xc3c80000).
const CRUISE_LIMIT_VEL: f32 = -400.0;

/// Per-ship state (missile destroyer +0x638..+0x64c; missile corvettes +0x640..+0x650 after
/// their side-step state, which is the generic [`crate::object::Ship::side_step`]).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MissileShipSpec {
    /// Last missile regeneration.
    pub last_regen: f32,
    /// Last volley.
    pub last_volley: f32,
    /// Last call of the special targeting (regeneration waits `MissileLagVolleyTime` after it).
    pub last_special: f32,
    /// The target the next missile goes to.
    pub cur_target: i32,
    /// Missile destroyer only: see [`volley`].
    pub volley_state: i32,
}

/// Per-type statics (`MissileDestroyerStaticInit` / `P1MissileCorvetteStaticInit`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MissileShipStatics {
    /// +0 / +0xc: the bullet ranges and 0.9 times the minimum ranges, per tactic.
    pub gun_range: [f32; 3],
    pub too_close_range: [f32; 3],
    /// +0x18 `MissileRegenerateTime`, +0x1c `MissileVolleyTime`, +0x20 `MissileLagVolleyTime`.
    pub regenerate_time: f32,
    pub volley_time: f32,
    pub lag_volley_time: f32,
}

impl MissileShipStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for (key, raw) in &stat.custom_entries {
            let v = crate::statics::cscan::scan_f32(raw);
            match key.as_str() {
                "MissileRegenerateTime" => s.regenerate_time = v.unwrap_or(s.regenerate_time),
                "MissileVolleyTime" => s.volley_time = v.unwrap_or(s.volley_time),
                "MissileLagVolleyTime" => s.lag_volley_time = v.unwrap_or(s.lag_volley_time),
                _ => {}
            }
        }
        for t in 0..3 {
            s.gun_range[t] = stat.bullet_range[t];
            s.too_close_range[t] = st(ld(stat.min_bullet_range[t]) * 0.9);
        }
        s
    }
}

fn module(u: &Universe, id: ShipId) -> Option<Module> {
    u.ships.get(id).map(|s| super::module_of(s.ship_type))
}

fn spec(u: &Universe, id: ShipId) -> Option<&MissileShipSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::MissileShip(m) => Some(m),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut MissileShipSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::MissileShip(m) => Some(m),
        _ => None,
    }
}

fn statics(u: &Universe, id: ShipId) -> MissileShipStatics {
    match super::type_statics(u, u.ships.ship(id).ship_type) {
        super::TypeStatics::MissileShip(s) => s.clone(),
        _ => MissileShipStatics::default(),
    }
}

/// `MissileDestroyerInit` (0x5b5e10) / `P1MissileCorvetteInit` (0x5b8c50); the cruise missile
/// has no state.
pub fn init(u: &mut Universe, id: ShipId) {
    let m = module(u, id);
    let Some(s) = u.ships.get_mut(id) else { return };
    match m {
        Some(Module::MissileDestroyer) => {
            s.spec = ShipSpec::MissileShip(MissileShipSpec { last_special: -100_000.0, volley_state: volley::BEGIN, ..Default::default() });
        }
        Some(Module::MissileCorvette) => {
            s.side_step = Default::default();
            s.spec = ShipSpec::MissileShip(MissileShipSpec { last_special: -100_000.0, ..Default::default() });
        }
        _ => {}
    }
}

/// `CruiseMissileAttack` (0x5aa9f0): fly at the target; within 5 km go kamikaze and close in
/// with `attackPassive`.
pub fn cruise_missile_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(t) = u.ships.get(target) else { return };
    let (tpos, tvel) = (t.posinfo.position, t.posinfo.velocity);
    let tstat = u.statics.ship(t.ship_type).expect("statics");
    let traj = crate::aiship::trajectory(u.ships.ship(id), t);
    let r = crate::aiship::range_to_target(tstat, traj);
    if CRUISE_KAMIKAZE_RANGE < r {
        let flags = fly::FIRST_POINT_IN_DIRECTION_FLYING | fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE;
        crate::aiship::fly_to_point_avoiding_objs(u, id, Some(tpos), flags, CRUISE_LIMIT_VEL, Some(tvel));
        return;
    }
    let s = u.ships.ship_mut(id);
    s.special_flags |= special::KAMIKAZE | special::BROKEN_FORMATION;
    // The attack routine's maximum distance is the ship's bullet range for its tactics.
    let max = u.statics.ship(s.ship_type).map_or(0.0, |st| st.bullet_range[s.tactics]);
    crate::attack::attack_passive(u, id, target, max, 0.0);
}

/// `P1MissileCorvetteAttack` (0x5b8c80): `attackSideStep`.
pub fn corvette_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if let Some(c) = u.combat.get(u.ships.ship(id).ship_type).cloned() {
        crate::attack::attack_side_step(u, id, target, &c);
    }
}

fn is_missile_launcher(u: &Universe, id: ShipId, gi: usize) -> bool {
    let t = u.ships.ship(id).ship_type;
    u.statics.ship(t).and_then(|s| s.guns.as_ref()).and_then(|g| g.get(gi)).is_some_and(|g| g.gun_type == Some(GunType::MissileLauncher))
}

/// Fires launcher `gi` at the current target and moves on to the next target.
fn volley_shot(u: &mut Universe, id: ShipId, gi: usize, targets: &[ShipId]) {
    let cur = spec(u, id).map_or(0, |s| s.cur_target) as usize;
    let t = targets[cur];
    if u.ships.get(t).is_some() {
        crate::gun::shoot_missile(u, id, gi, Some(t));
    }
    let s = spec_mut(u, id).expect("missile ship");
    s.cur_target += 1;
    if targets.len() as i32 <= s.cur_target {
        s.cur_target = 0;
    }
}

/// Starts a volley if `MissileVolleyTime` has passed since the last (both routines start so).
fn volley_due(u: &mut Universe, id: ShipId) -> bool {
    let now = u.total_time_elapsed;
    let vt = statics(u, id).volley_time;
    let s = spec_mut(u, id).expect("missile ship");
    s.last_special = now;
    if !(ld(vt) < ld(now) - ld(s.last_volley)) {
        return false;
    }
    s.last_volley = now;
    true
}

/// `MissileDestroyerSpecialTarget` (0x5b5f00) and `P1MissileCorvetteSpecialTarget` (0x5b8d70):
/// one volley at the targets. Done when there are no targets or (destroyer) every launcher in
/// range is empty, or (corvettes) every launcher is empty.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if spec(u, id).is_none() {
        return true;
    }
    let destroyer = module(u, id) == Some(Module::MissileDestroyer);
    if !volley_due(u, id) {
        return false;
    }
    let n = targets.len() as i32;
    if n == 0 {
        let s = spec_mut(u, id).expect("missile ship");
        s.cur_target = 0;
        if destroyer {
            s.volley_state = volley::BEGIN;
        }
        return true;
    }
    {
        let s = spec_mut(u, id).expect("missile ship");
        if destroyer && s.volley_state == volley::BEGIN {
            // (Battle chatter "volley attack" for the local player.)
            s.volley_state = volley::NOT_BEGIN;
        }
        if n <= s.cur_target {
            s.cur_target = 0;
        }
    }
    let guns = u.ships.ship(id).guns.len();
    let mut fired = false;
    let mut tried = false;
    for gi in 0..guns {
        if !is_missile_launcher(u, id, gi) {
            continue;
        }
        if destroyer {
            let cur = spec(u, id).expect("missile ship").cur_target as usize;
            let Some(t) = u.ships.get(targets[cur]) else { continue };
            let me = u.ships.ship(id).coll.coll_position;
            let tc = t.coll.coll_position;
            let traj = Vector::new(st(ld(me.x) - ld(tc.x)), st(ld(me.y) - ld(tc.y)), st(ld(me.z) - ld(tc.z)));
            let r = crate::aiship::range_to_target(u.statics.ship(t.ship_type).expect("statics"), traj);
            let br = u.statics.ship(u.ships.ship(id).ship_type).and_then(|s| s.guns.as_ref()).map_or(0.0, |g| g[gi].bullet_range);
            if !(r < ld(br) * ld(f32::from_bits(0x3f66_6666))) {
                continue;
            }
            tried = true;
        }
        if u.ships.ship(id).guns[gi].num_missiles > 0 {
            fired = true;
            volley_shot(u, id, gi, targets);
        }
    }
    if fired {
        if destroyer {
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(id);
            if let Some(stat) = statics.ship(s.ship_type) {
                crate::aitrack::zero_velocity(s, stat, &tw, dt);
            }
        }
        return false;
    }
    if !destroyer || tried {
        let s = spec_mut(u, id).expect("missile ship");
        s.cur_target = 0;
        if destroyer {
            s.volley_state = volley::BEGIN;
        }
        return true;
    }
    // Out of range: fly to the first target (done when the flight reports an obstacle, as the
    // original returns the flight's result).
    let Some(p) = u.ships.get(targets[0]).map(|t| t.posinfo.position) else { return true };
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(p), fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE, 0.0, None) != 0
}

/// `MissileDestroyerHousekeep` (0x5b5e40) / `P1MissileCorvetteHousekeep` (0x5b8cb0): one
/// missile back every `MissileRegenerateTime`, once `MissileLagVolleyTime` has passed since the
/// last volley order.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(s) = spec(u, id).cloned() else { return };
    let ms = statics(u, id);
    let now = ld(u.total_time_elapsed);
    if ld(ms.lag_volley_time) < now - ld(s.last_special) && ld(ms.regenerate_time) < now - ld(s.last_regen) {
        spec_mut(u, id).expect("missile ship").last_regen = u.total_time_elapsed;
        super::minelayer::regenerate_one(u, id, GunType::MissileLauncher);
    }
}
