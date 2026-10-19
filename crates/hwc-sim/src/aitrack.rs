//! `AITrack.c`: the lowest AI layer. Given a desired velocity or heading it applies the thrust
//! and torque that bring the ship there. All functions are Cataclysm's builds (identical in
//! logic to HW1), with the floating-point evaluation order taken from the disassembly.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, ld, lt_or_unordered, st, Ext};
use hwc_math::Vector;

use crate::object::{rot, sof, trans, Ship};
use crate::physics::{apply_force, apply_rot};
use crate::statics::ShipStaticInfo;
use crate::tunables::SimTweaks;

/// `AITRACKHEADING_IGNOREUPVEC`.
pub const IGNORE_UP_VEC: u32 = 1;
/// `AITRACKHEADING_DONTROLL`.
pub const DONT_ROLL: u32 = 2;
/// `AITRACK_DONT_ZERO_ME`.
pub const DONT_ZERO_ME: u32 = 4;

/// `capNumber` on a register value, stored as the float argument of the physics call.
fn cap(value: Ext, limit: f32) -> f32 {
    if value > ld(limit) {
        limit
    } else {
        st(value)
    }
}

fn between(v: Ext, lo: f32, hi: f32) -> bool {
    v > ld(lo) && v < ld(hi)
}

/// `aitrackHeadingFunc` (0x418df0): turn towards `desired` (which it may flatten and normalise
/// in place when the ship cannot pitch). Returns true when on heading (and then stops the
/// rotation unless `DONT_ZERO_ME`).
#[allow(clippy::too_many_arguments)]
pub fn heading(
    ship: &mut Ship,
    stat: &ShipStaticInfo,
    _tw: &SimTweaks,
    dt: f32,
    desired: &mut Vector,
    accuracy: f32,
    up_accuracy: f32,
    sinbank: f32,
    flags: u32,
) -> bool {
    if ship.dont_rotate_ever {
        return true;
    }
    let z = |v: f32| eq_or_unordered(ld(v), 0.0);
    if z(desired.x) && z(desired.y) && z(desired.z) {
        return true;
    }
    if z(ship.nonstat.rot_strength[rot::ABOUT_Y_CCW as usize]) {
        desired.z = 0.0;
        desired.normalize();
    }
    let h = ship.heading();
    let dot = (ld(h.z) * ld(desired.z) + ld(h.y) * ld(desired.y)) + ld(h.x) * ld(desired.x);
    let up_z = ship.rotinfo.coordsys.m[2];
    if !(dot > -1.0) {
        apply_rot(ship, ship.nonstat.rot_strength[rot::ABOUT_X_CW as usize], rot::ABOUT_X_CW);
        return false;
    }
    if !lt_or_unordered(dot, ld(accuracy))
        && (flags & (IGNORE_UP_VEC | DONT_ROLL) != 0 || !lt_or_unordered(ld(up_z), ld(up_accuracy)))
    {
        if flags & DONT_ZERO_ME == 0 && ship.flags & sof::CRAZY == 0 {
            ship.rotinfo.rotspeed = Vector::ZERO;
        }
        return true;
    }

    let cs = ship.rotinfo.coordsys;
    let dis = cs.vec_mul(*desired);
    let uis = cs.vec_mul(Vector::new(0.0, 0.0, 1.0));
    let oot = ld(st(1.0 / ld(dt)));
    let moi = stat.header.moment_of_inertia;
    let ns = ship.nonstat;
    let mut yaw_closeness = 0.0f32;

    // Yaw (about x).
    if !z(dis.y) {
        let turn = ld(ns.turn_speed[0]);
        let desired_rot = if ld(dis.z) < 0.0 {
            if ld(dis.y) < 0.0 {
                turn
            } else {
                -turn
            }
        } else {
            -(ld(dis.y) * turn)
        };
        if !z(sinbank) {
            yaw_closeness = if ld(dis.z) < 0.0 {
                if ld(dis.y) < 0.0 {
                    sinbank
                } else {
                    -sinbank
                }
            } else {
                st(-(ld(dis.y) * ld(sinbank)))
            };
        }
        let rs = ld(ship.rotinfo.rotspeed.x);
        if rs < desired_rot {
            let t = (desired_rot - rs) * ld(moi.x) * oot;
            let t = cap(t, ns.rot_strength[0]);
            apply_rot(ship, t, rot::ABOUT_X_CCW);
        } else {
            let t = (rs - desired_rot) * ld(moi.x) * oot;
            let t = cap(t, ns.rot_strength[1]);
            apply_rot(ship, t, rot::ABOUT_X_CW);
        }
    }

    // Pitch (about y).
    if !z(dis.x) {
        let desired_rot = ld(dis.x) * ld(ns.turn_speed[1]);
        let rs = ld(ship.rotinfo.rotspeed.y);
        if rs < desired_rot {
            let t = cap((desired_rot - rs) * ld(moi.y) * oot, ns.rot_strength[2]);
            apply_rot(ship, t, rot::ABOUT_Y_CCW);
        } else {
            let t = cap((rs - desired_rot) * ld(moi.y) * oot, ns.rot_strength[3]);
            apply_rot(ship, t, rot::ABOUT_Y_CW);
        }
    }

    if flags & DONT_ROLL != 0 {
        return false;
    }

    // Roll (about z) towards the banked up vector.
    if !eq_or_unordered(ld(uis.y), ld(yaw_closeness)) {
        let desired_rot = (ld(uis.y) - ld(yaw_closeness)) * ld(ns.turn_speed[2]);
        let rs = ld(ship.rotinfo.rotspeed.z);
        if rs < desired_rot {
            let t = cap((desired_rot - rs) * ld(moi.z) * oot, ns.rot_strength[4]);
            apply_rot(ship, t, rot::ABOUT_Z_CCW);
        } else {
            let t = cap((rs - desired_rot) * ld(moi.z) * oot, ns.rot_strength[5]);
            apply_rot(ship, t, rot::ABOUT_Z_CW);
        }
    }
    false
}

/// `aitrackHeadingWithBankPitchFunc` (0x419210): heading tracking for ships that pitch while
/// turning or descending (frigates and bigger). `desired` need not be normalised.
#[allow(clippy::too_many_arguments)]
pub fn heading_with_bank_pitch(
    ship: &mut Ship,
    stat: &ShipStaticInfo,
    tw: &SimTweaks,
    dt: f32,
    desired: Vector,
    accuracy: f32,
    sinbank: f32,
    pitch_descend: f32,
    pitch_turn: f32,
    flags: u32,
) -> bool {
    if ship.dont_rotate_ever {
        return true;
    }
    let h = ship.heading();
    let (mut cx, mut cy) = (h.x, h.y);
    let cz = h.z;
    let up_z = ship.rotinfo.coordsys.m[2];
    let (mut dx, mut dy) = (desired.x, desired.y);

    let mut des_sqr = st(ld(dy) * ld(dy) + ld(dx) * ld(dx));
    let mut cur_sqr = ld(cy) * ld(cy) + ld(cx) * ld(cx);
    if eq_or_unordered(ld(des_sqr), 0.0) {
        dx = 1.0;
        des_sqr = 1.0;
    }
    if eq_or_unordered(cur_sqr, 0.0) {
        cx = 1.0;
        cur_sqr = 1.0;
    }
    let temp1 = st(1.0 / fsqrt(cur_sqr));
    let temp2 = 1.0 / fsqrt(ld(des_sqr));
    cx = st(ld(temp1) * ld(cx));
    cy = st(ld(temp1) * ld(cy));
    dx = st(temp2 * ld(dx));
    let dy_ext = temp2 * ld(dy);
    dy = st(dy_ext);
    let dot = dy_ext * ld(cy) + ld(dx) * ld(cx);

    if !lt_or_unordered(dot, ld(accuracy))
        && eq_or_unordered(ld(pitch_descend), 0.0)
        && between(ld(cz), tw.stabilize_z_lo, tw.stabilize_z_hi)
        && (flags & IGNORE_UP_VEC != 0 || !lt_or_unordered(ld(up_z), ld(accuracy)))
    {
        ship.rotinfo.rotspeed = Vector::ZERO;
        return true;
    }

    let abs_dot = if dot < 0.0 { -dot } else { dot };
    let mut pitch = (1.0 - abs_dot) * ld(pitch_turn);
    let pd = ld(pitch_descend);
    if pd < 0.0 {
        pitch = -pitch;
        if pd < pitch {
            pitch = pd;
        }
    } else if pd > pitch {
        pitch = pd;
    }

    // FPTAN is approximated by the double tangent.
    let tan = pitch.tan();
    let dz = st(tan);
    let t = 1.0 / fsqrt(tan * ld(dz) + 1.0);
    let mut des = Vector::new(st(t * ld(dx)), st(t * ld(dy)), st(ld(dz) * t));
    heading(ship, stat, tw, dt, &mut des, accuracy, accuracy, sinbank, flags)
}

/// `aitrackVelocityVectorGuidance` (0x419470): thrust towards a desired velocity.
pub fn velocity_vector(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32, desired: Vector) {
    let v = ship.posinfo.velocity;
    let ex = ld(desired.x) - ld(v.x);
    let ey = ld(desired.y) - ld(v.y);
    let ez = ld(desired.z) - ld(v.z);
    if lt_or_unordered((ez * ez + ey * ey) + ex * ex, ld(tw.error_velocity_squared)) {
        return;
    }
    let cs = ship.rotinfo.coordsys;
    let d = cs.vec_mul(desired);
    let s = cs.vec_mul(v);
    let mot = ld(st(ld(stat.header.mass) / ld(dt)));
    let caps = ship.nonstat.thrust_strength;
    let axis = |ship: &mut Ship, sv: f32, dv: f32, pos: u16, neg: u16| {
        let (sv, dv) = (ld(sv), ld(dv));
        if sv < dv {
            let f = cap((dv - sv) * mot, caps[pos as usize]);
            apply_force(ship, f, pos);
        } else if sv > dv {
            let f = cap((sv - dv) * mot, caps[neg as usize]);
            apply_force(ship, f, neg);
        }
    };
    axis(ship, s.x, d.x, trans::UP, trans::DOWN);
    axis(ship, s.y, d.y, trans::RIGHT, trans::LEFT);
    axis(ship, s.z, d.z, trans::FORWARD, trans::BACK);
}

/// `aitrackIsZeroVelocity`.
pub fn is_zero_velocity(ship: &Ship, tw: &SimTweaks) -> bool {
    let v = ship.posinfo.velocity;
    let (lo, hi) = (tw.still_velocity_lo, tw.still_velocity_hi);
    between(ld(v.x), lo, hi) && between(ld(v.y), lo, hi) && between(ld(v.z), lo, hi)
}

/// `aitrackZeroVelocity` (0x4197c0): thrust against the current drift. True when already still.
pub fn zero_velocity(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32) -> bool {
    if is_zero_velocity(ship, tw) {
        return true;
    }
    let s = ship.rotinfo.coordsys.vec_mul(ship.posinfo.velocity);
    let mot = ld(st(ld(stat.header.mass) / ld(dt)));
    let caps = ship.nonstat.thrust_strength;
    let axis = |ship: &mut Ship, sv: f32, pos: u16, neg: u16| {
        let t = ld(sv) * mot;
        if ld(sv) < 0.0 {
            let f = cap(-t, caps[pos as usize]);
            apply_force(ship, f, pos);
        } else {
            let f = cap(t, caps[neg as usize]);
            apply_force(ship, f, neg);
        }
    };
    axis(ship, s.x, trans::UP, trans::DOWN);
    axis(ship, s.y, trans::RIGHT, trans::LEFT);
    axis(ship, s.z, trans::FORWARD, trans::BACK);
    false
}

/// `aitrackForceZeroVelocity`.
pub fn force_zero_velocity(ship: &mut Ship) {
    if ship.flags & sof::CRAZY == 0 {
        ship.posinfo.velocity = Vector::ZERO;
    }
}

/// `aitrackForceSteadyShip` (0x419ce0).
pub fn force_steady_ship(ship: &mut Ship) {
    if ship.flags & sof::CRAZY == 0 {
        ship.posinfo.velocity = Vector::ZERO;
        ship.rotinfo.rotspeed = Vector::ZERO;
    }
}

/// `aitrackSteadyShipDriftOnly`: stop drifting; once still, zero the velocity exactly.
pub fn steady_ship_drift_only(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32) -> bool {
    if zero_velocity(ship, stat, tw, dt) {
        force_zero_velocity(ship);
        return true;
    }
    false
}

/// `aitrackIsStabilizeShip` (0x418350).
pub fn is_stabilize_ship(ship: &Ship, tw: &SimTweaks) -> bool {
    let m = &ship.rotinfo.coordsys.m;
    let (heading_z, up_z, right_z) = (m[8], m[2], m[5]);
    between(ld(heading_z), tw.stabilize_z_lo, tw.stabilize_z_hi)
        && between(ld(ship.rotinfo.rotspeed.x), tw.still_rot_lo, tw.still_rot_hi)
        && between(ld(right_z), tw.stabilize_z_lo, tw.stabilize_z_hi)
        && ld(up_z) > 0.0
}

/// `aitrackIsShipSteady` (0x419bd0).
pub fn is_ship_steady(ship: &Ship, tw: &SimTweaks) -> bool {
    let zero = is_zero_velocity(ship, tw);
    is_stabilize_ship(ship, tw) & zero
}

/// Torque `(1/t * I) * r`, negated for the counter-clockwise direction.
fn damp(oot: Ext, moi: f32, r: f32) -> Ext {
    oot * ld(moi) * ld(r)
}

/// `aitrackStabilizeGuidance` (0x418400): level the ship (no pitch, no roll, no yaw spin).
pub fn stabilize(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32) -> bool {
    let rx = ship.rotinfo.rotspeed.x;
    let mut ry = ship.rotinfo.rotspeed.y;
    let rz = ship.rotinfo.rotspeed.z;
    let m = ship.rotinfo.coordsys.m;
    let (heading_z, up_z, right_z) = (m[8], m[2], m[5]);
    let stabilized_pitch = between(ld(heading_z), tw.stabilize_z_lo, tw.stabilize_z_hi);
    if stabilized_pitch {
        ship.rotinfo.rotspeed.y = 0.0;
        ry = 0.0;
    }
    if between(ld(rx), tw.still_rot_lo, tw.still_rot_hi)
        && stabilized_pitch
        && between(ld(right_z), tw.stabilize_z_lo, tw.stabilize_z_hi)
        && ld(up_z) > 0.0
    {
        return true;
    }
    let oot = ld(st(1.0 / ld(dt)));
    let moi = stat.header.moment_of_inertia;
    let rs = ship.nonstat.rot_strength;

    let t = damp(oot, moi.x, rx);
    if ld(rx) > 0.0 {
        apply_rot(ship, cap(t, rs[1]), rot::ABOUT_X_CW);
    } else {
        apply_rot(ship, cap(-t, rs[0]), rot::ABOUT_X_CCW);
    }

    if stabilized_pitch {
        let t = damp(oot, moi.z, rz);
        if ld(rz) > 1.0 {
            apply_rot(ship, cap(t, rs[5]), rot::ABOUT_Z_CW);
        } else if ld(rz) < -1.0 {
            apply_rot(ship, cap(-t, rs[4]), rot::ABOUT_Z_CCW);
        } else if ld(right_z) > 0.0 {
            apply_rot(ship, st(ld(right_z) * ld(rs[4])), rot::ABOUT_Z_CCW);
        } else {
            apply_rot(ship, st(-ld(right_z) * ld(rs[5])), rot::ABOUT_Z_CW);
        }
        return false;
    }

    let t = damp(oot, moi.z, rz);
    if ld(rz) > 0.0 {
        apply_rot(ship, cap(t, rs[5]), rot::ABOUT_Z_CW);
    } else {
        apply_rot(ship, cap(-t, rs[4]), rot::ABOUT_Z_CCW);
    }
    let t = damp(oot, moi.y, ry);
    if ld(ry) > 1.0 {
        apply_rot(ship, cap(t, rs[3]), rot::ABOUT_Y_CW);
    } else if ld(ry) < -1.0 {
        apply_rot(ship, cap(-t, rs[2]), rot::ABOUT_Y_CCW);
    } else if ld(heading_z) < 0.0 {
        apply_rot(ship, st(-ld(heading_z) * ld(rs[2])), rot::ABOUT_Y_CCW);
    } else {
        apply_rot(ship, st(ld(heading_z) * ld(rs[3])), rot::ABOUT_Y_CW);
    }
    false
}

/// `aitrackSteadyShip` (0x419c80): level the ship and stop it.
pub fn steady_ship(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32) -> bool {
    let stabilized = stabilize(ship, stat, tw, dt);
    let zero_vel = zero_velocity(ship, stat, tw, dt);
    if stabilized && zero_vel {
        force_steady_ship(ship);
        return true;
    }
    false
}

/// `aitrackZeroRotationAnywhere` (0x418810): stop spinning in whatever orientation.
pub fn zero_rotation_anywhere(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, dt: f32) -> bool {
    let r = ship.rotinfo.rotspeed;
    if between(ld(r.x), tw.still_rot_lo, tw.still_rot_hi)
        && between(ld(r.y), tw.still_rot_lo, tw.still_rot_hi)
        && between(ld(r.z), tw.still_rot_lo, tw.still_rot_hi)
    {
        return true;
    }
    let oot = ld(st(1.0 / ld(dt)));
    let moi = stat.header.moment_of_inertia;
    let rs = ship.nonstat.rot_strength;
    for (i, (rv, m)) in [(r.x, moi.x), (r.y, moi.y), (r.z, moi.z)].into_iter().enumerate() {
        let t = damp(oot, m, rv);
        let (ccw, cw) = (2 * i as u16, 2 * i as u16 + 1);
        if ld(rv) > 0.0 {
            apply_rot(ship, cap(t, rs[cw as usize]), cw);
        } else {
            apply_rot(ship, cap(-t, rs[ccw as usize]), ccw);
        }
    }
    false
}

/// `aitrackHeadingAndUp` (0x418a90): turn so the heading and up vectors match the desired
/// ones. True when both are within `accuracy` (the rotation is then stopped).
pub fn heading_and_up(ship: &mut Ship, stat: &ShipStaticInfo, dt: f32, desired_heading: Vector, desired_up: Vector, accuracy: f32) -> bool {
    if ship.dont_rotate_ever {
        return true;
    }
    let h = ship.heading();
    let up = ship.up();
    let hd = (ld(h.x) * ld(desired_heading.x) + ld(h.y) * ld(desired_heading.y)) + ld(h.z) * ld(desired_heading.z);
    let ud = (ld(up.x) * ld(desired_up.x) + ld(up.y) * ld(desired_up.y)) + ld(up.z) * ld(desired_up.z);
    let rs = ship.nonstat.rot_strength;
    if hd <= -1.0 {
        apply_rot(ship, rs[1], rot::ABOUT_X_CW);
        return false;
    }
    if ud <= -1.0 {
        apply_rot(ship, rs[5], rot::ABOUT_Z_CW);
        return false;
    }
    if ld(accuracy) <= hd && ld(accuracy) <= ud {
        if ship.flags & sof::CRAZY == 0 {
            ship.rotinfo.rotspeed = Vector::ZERO;
        }
        return true;
    }
    let lh = ship.rotinfo.coordsys.vec_mul(desired_heading);
    let lu = ship.rotinfo.coordsys.vec_mul(desired_up);
    let inv_dt = 1.0 / ld(dt);
    let ts = ship.nonstat.turn_speed;
    let moi = stat.header.moment_of_inertia;
    if !eq_or_unordered(ld(lh.y), 0.0) {
        let f = if ld(lh.z) >= 0.0 {
            -(ld(lh.y) * ld(ts[0]))
        } else if ld(lh.y) >= 0.0 {
            -ld(ts[0])
        } else {
            ld(ts[0])
        };
        let f = ld(st(f));
        let cur = ld(ship.rotinfo.rotspeed.x);
        if f <= cur {
            let t = cap((cur - f) * ld(moi.x) * inv_dt, rs[1]);
            apply_rot(ship, t, rot::ABOUT_X_CW);
        } else {
            let t = cap((f - cur) * ld(moi.x) * inv_dt, rs[0]);
            apply_rot(ship, t, rot::ABOUT_X_CCW);
        }
    }
    if !eq_or_unordered(ld(lh.x), 0.0) {
        let f = ld(st(ld(lh.x) * ld(ts[1])));
        let cur = ld(ship.rotinfo.rotspeed.y);
        if f <= cur {
            let t = cap((cur - f) * ld(moi.y) * inv_dt, rs[3]);
            apply_rot(ship, t, rot::ABOUT_Y_CW);
        } else {
            let t = cap((f - cur) * ld(moi.y) * inv_dt, rs[2]);
            apply_rot(ship, t, rot::ABOUT_Y_CCW);
        }
    }
    if !eq_or_unordered(ld(lu.y), 0.0) {
        let v = ld(st(ld(lu.y) * ld(ts[2])));
        let cur = ld(ship.rotinfo.rotspeed.z);
        if cur < v {
            let t = cap((v - cur) * ld(moi.z) * inv_dt, rs[4]);
            apply_rot(ship, t, rot::ABOUT_Z_CCW);
        } else {
            let t = cap((cur - v) * ld(moi.z) * inv_dt, rs[5]);
            apply_rot(ship, t, rot::ABOUT_Z_CW);
        }
    }
    false
}

/// `aitrackZeroForwardVelocity` (0x419b10): stop moving along the heading. True when the
/// forward speed is within (-3, 3).
pub fn zero_forward_velocity(ship: &mut Ship, stat: &ShipStaticInfo, dt: f32) -> bool {
    let h = ship.heading();
    let v = ship.posinfo.velocity;
    let fwd = ld(st((ld(h.x) * ld(v.x) + ld(h.y) * ld(v.y)) + ld(h.z) * ld(v.z)));
    if fwd > -3.0 && fwd < 3.0 {
        return true;
    }
    let mot = ld(stat.header.mass) / ld(dt);
    let caps = ship.nonstat.thrust_strength;
    if fwd >= 0.0 {
        let f = cap(mot * fwd, caps[trans::BACK as usize]);
        apply_force(ship, f, trans::BACK);
    } else {
        let f = cap(-(mot * fwd), caps[trans::FORWARD as usize]);
        apply_force(ship, f, trans::FORWARD);
    }
    false
}

/// `FUN_00419e90`: within `dist` of `point` (squared distance compared).
pub fn within_distance(ship: &Ship, point: Vector, dist: f32) -> bool {
    let p = ship.posinfo.position;
    let dx = ld(p.x) - ld(point.x);
    let dy = ld(p.y) - ld(point.y);
    let dz = ld(p.z) - ld(point.z);
    (dx * dx + dy * dy) + dz * dz < ld(dist) * ld(dist)
}

/// `MoveReachedDestinationVariable` (0x419d60): within `bounds` on every axis (exclusive).
pub fn move_reached_destination(ship: &Ship, dest: Vector, bounds: f32) -> bool {
    let p = ship.posinfo.position;
    let neg = ld(-bounds);
    let dx = ld(dest.x) - ld(p.x);
    let dy = ld(st(ld(dest.y) - ld(p.y)));
    let dz = ld(st(ld(dest.z) - ld(p.z)));
    let b = ld(bounds);
    dx > neg && dx < b && dy > neg && dy < b && dz > neg && dz < b
}

/// `MoveLeftToGo` (0x419e00): the largest per-axis distance left (register value).
pub fn move_left_to_go(ship: &Ship, dest: Vector) -> Ext {
    let p = ship.posinfo.position;
    let abs = |v: Ext| if v < 0.0 { -v } else { v };
    let x = st(abs(ld(dest.x) - ld(p.x)));
    let y = st(abs(ld(dest.y) - ld(p.y)));
    let z = abs(ld(dest.z) - ld(p.z));
    let xy = if ld(x) > ld(y) { x } else { y };
    if ld(xy) > z {
        ld(xy)
    } else {
        z
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::ShipId;
    use crate::statics::{ShipRace, ShipType};
    use hwc_math::Matrix;

    struct Row {
        vel: [f32; 3],
        coordsys: [f32; 9],
        rotspeed: [f32; 3],
        thrust: [f32; 6],
        rot: [f32; 6],
        turn: [f32; 3],
        mass: f32,
        moi: [f32; 3],
        desired: [f32; 3],
        vel_target: [f32; 3],
        sinbank: f32,
        vv_force: [u32; 3],
        hd_torque: [u32; 3],
        hd_rotspeed: [u32; 3],
        hd_ret: u8,
        zv_force: [u32; 3],
        zv_ret: u8,
    }

    /// Results of emulating `aitrackVelocityVectorGuidance` (0x419470), `aitrackHeadingFunc`
    /// (0x418df0, accuracy 0.999, flags 0) and `aitrackZeroVelocity` (0x4197c0) in
    /// cataclysm.exe on constructed ships (compiled-in AITrack tweaks, dt = 1/16).
    const ROWS: &[Row] = &[
        Row { vel: [f32::from_bits(0xc2fc1264), f32::from_bits(0xc0228b22), f32::from_bits(0x42faf04c)], coordsys: [f32::from_bits(0x3d32a282), f32::from_bits(0xbd527419), f32::from_bits(0x3f7f6b00), f32::from_bits(0xbf432bd0), f32::from_bits(0xbf25a9ba), f32::from_bits(0x00000000), f32::from_bits(0x3f25494e), f32::from_bits(0xbf42ba37), f32::from_bits(0xbd8a05d0)], rotspeed: [f32::from_bits(0xbf2f6ba5), f32::from_bits(0x3e17200c), f32::from_bits(0x3eb00c0c)], thrust: [f32::from_bits(0x458be8d5), f32::from_bits(0x4592243f), f32::from_bits(0x45a000d8), f32::from_bits(0x462847cb), f32::from_bits(0x45ba9e50), f32::from_bits(0x45f6b084)], rot: [f32::from_bits(0x45030cbc), f32::from_bits(0x44fec768), f32::from_bits(0x44427015), f32::from_bits(0x44ae04d4), f32::from_bits(0x452baba3), f32::from_bits(0x4486775d)], turn: [f32::from_bits(0x3fa611dc), f32::from_bits(0x3f6bab25), f32::from_bits(0x3fab97f9)], mass: f32::from_bits(0x4338388a), moi: [f32::from_bits(0x42f2c57a), f32::from_bits(0x42c748f2), f32::from_bits(0x42bf2787)], desired: [f32::from_bits(0xbf69dd10), f32::from_bits(0xbe59b198), f32::from_bits(0xbeb18f84)], vel_target: [f32::from_bits(0xc3890787), f32::from_bits(0xc27f1c1e), f32::from_bits(0xc2d0142f)], sinbank: f32::from_bits(0x00000000),
              vv_force: [0xc60fd108, 0x453739aa, 0xc5812f18], hd_torque: [0xc4949ee6, 0xc4446390, 0x00000000], hd_rotspeed: [0xbf2f6ba5, 0x3e17200c, 0x3eb00c0c], hd_ret: 0, zv_force: [0x46395a69, 0x4526b46e, 0xc59e62e2], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x42b2da59), f32::from_bits(0xc10b4e82), f32::from_bits(0xc3052e77)], coordsys: [f32::from_bits(0xbd24039d), f32::from_bits(0xbd9021a2), f32::from_bits(0x3f7f28d0), f32::from_bits(0x3f5e814e), f32::from_bits(0xbefd330b), f32::from_bits(0x00000000), f32::from_bits(0x3efc5e35), f32::from_bits(0x3f5dc645), f32::from_bits(0x3da5d408)], rotspeed: [f32::from_bits(0x3e63e244), f32::from_bits(0x3f5251a4), f32::from_bits(0xbd1f487f)], thrust: [f32::from_bits(0x46d81bcb), f32::from_bits(0x46cfe235), f32::from_bits(0x46ac97b0), f32::from_bits(0x46c329ac), f32::from_bits(0x469eaf1e), f32::from_bits(0x4662f933)], rot: [f32::from_bits(0x443ee57d), f32::from_bits(0x443b7389), f32::from_bits(0x44ed779b), f32::from_bits(0x44a2e54f), f32::from_bits(0x45234f77), f32::from_bits(0x44399440)], turn: [f32::from_bits(0x3f8f6a05), f32::from_bits(0x401241f1), f32::from_bits(0x3fe9324b)], mass: f32::from_bits(0x424d55b2), moi: [f32::from_bits(0x42c0a55c), f32::from_bits(0x436d3724), f32::from_bits(0x4322a9f1)], desired: [f32::from_bits(0xbd4442b7), f32::from_bits(0xbe409e70), f32::from_bits(0x3f7b217a)], vel_target: [f32::from_bits(0xc165fe2e), f32::from_bits(0xc261b9ab), f32::from_bits(0x4393259d)], sinbank: f32::from_bits(0x3f000000),
              vv_force: [0xc6ea383a, 0xc5081350, 0x46ce35b9], hd_torque: [0xc43b7389, 0x44ed779b, 0x451a7ffb], hd_rotspeed: [0x3e63e244, 0x3f5251a4, 0xbd1f487f], hd_ret: 0, zv_force: [0xc6ea383a, 0xc5081350, 0x46ce35b9], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x41f788ac), f32::from_bits(0x42ea9a7b), f32::from_bits(0xc2c601e3)], coordsys: [f32::from_bits(0x3e9dc3c1), f32::from_bits(0x3e6907ff), f32::from_bits(0x3f6c7928), f32::from_bits(0x3f1815e7), f32::from_bits(0xbf4ded59), f32::from_bits(0xa4000000), f32::from_bits(0x3f3e3843), f32::from_bits(0x3f0c7c2b), f32::from_bits(0xbec4205f)], rotspeed: [f32::from_bits(0x3f2215ab), f32::from_bits(0xbcde4690), f32::from_bits(0xbf10c617)], thrust: [f32::from_bits(0x464f2b6d), f32::from_bits(0x45fc9f4c), f32::from_bits(0x46bdac47), f32::from_bits(0x46c3f81b), f32::from_bits(0x46bdba12), f32::from_bits(0x46e70d3e)], rot: [f32::from_bits(0x44c38be3), f32::from_bits(0x4495e0b9), f32::from_bits(0x43d37d65), f32::from_bits(0x44bc1fc0), f32::from_bits(0x45338d6c), f32::from_bits(0x443a644f)], turn: [f32::from_bits(0x3fb7ad04), f32::from_bits(0x3f9eec27), f32::from_bits(0x3fea481e)], mass: f32::from_bits(0x43aee6ab), moi: [f32::from_bits(0x4381acd0), f32::from_bits(0x4147e797), f32::from_bits(0x4388de2c)], desired: [f32::from_bits(0xbf794321), f32::from_bits(0x3de6834e), f32::from_bits(0x3e4af123)], vel_target: [f32::from_bits(0xc3920d55), f32::from_bits(0x420710f0), f32::from_bits(0x426dd295)], sinbank: f32::from_bits(0x00000000),
              vv_force: [0xc70017d6, 0x45d9aabc, 0x46b830b7], hd_torque: [0x44c38be3, 0xc189f93b, 0x451acdc2], hd_rotspeed: [0x3f2215ab, 0xbcde4690, 0xbf10c617], hd_ret: 0, zv_force: [0xc558a898, 0xc6ffcb7e, 0x46b830b7], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x42f6f29a), f32::from_bits(0xc2945aa4), f32::from_bits(0xc2ece851)], coordsys: [f32::from_bits(0x3e5f25d3), f32::from_bits(0xbe8df03e), f32::from_bits(0x3f6f8e65), f32::from_bits(0xbf494333), f32::from_bits(0xbf1e34f5), f32::from_bits(0xa4000000), f32::from_bits(0x3f140b70), f32::from_bits(0xbf3c55af), f32::from_bits(0xbeb48aa3)], rotspeed: [f32::from_bits(0xbdae3034), f32::from_bits(0x3f490d7d), f32::from_bits(0x3e6ea20f)], thrust: [f32::from_bits(0x46d46a40), f32::from_bits(0x46e828fe), f32::from_bits(0x45fec8f3), f32::from_bits(0x46e308e0), f32::from_bits(0x456bfddb), f32::from_bits(0x469f382a)], rot: [f32::from_bits(0x45006c55), f32::from_bits(0x44de2365), f32::from_bits(0x43fa2328), f32::from_bits(0x439a33e8), f32::from_bits(0x43a0d8de), f32::from_bits(0x4493c75f)], turn: [f32::from_bits(0x3f40f281), f32::from_bits(0x4017e30b), f32::from_bits(0x4029e698)], mass: f32::from_bits(0x430fe4a3), moi: [f32::from_bits(0x427ffac2), f32::from_bits(0x4328ce5f), f32::from_bits(0x425d5558)], desired: [f32::from_bits(0xbdcf850e), f32::from_bits(0xbf59bc06), f32::from_bits(0xbf041de0)], vel_target: [f32::from_bits(0xc1f32fec), f32::from_bits(0xc37f2857), f32::from_bits(0xc31ad302)], sinbank: f32::from_bits(0x3f000000),
              vv_force: [0xc6273802, 0x43d2d768, 0xc6e3a64c], hd_torque: [0xc3be035e, 0xc39a33e8, 0x43a0d8de], hd_rotspeed: [0xbdae3034, 0x3f490d7d, 0x3e6ea20f], hd_ret: 0, zv_force: [0xc63fba87, 0x45171364, 0x46feea34], zv_ret: 0 },
        Row { vel: [f32::from_bits(0xc31fb2d1), f32::from_bits(0xc31b7ecc), f32::from_bits(0xc2cf8894)], coordsys: [f32::from_bits(0xbc7e517e), f32::from_bits(0x3cdb9e6e), f32::from_bits(0x3f7fe08c), f32::from_bits(0xbf5d8b4d), f32::from_bits(0xbf00461e), f32::from_bits(0x22000000), f32::from_bits(0x3f00365b), f32::from_bits(0xbf5d7015), f32::from_bits(0x3cfdc679)], rotspeed: [f32::from_bits(0x3f42ae22), f32::from_bits(0x398c16cd), f32::from_bits(0x3ed0dce2)], thrust: [f32::from_bits(0x452b99c2), f32::from_bits(0x461abc02), f32::from_bits(0x46401a56), f32::from_bits(0x46dd6280), f32::from_bits(0x4650855f), f32::from_bits(0x46cbca11)], rot: [f32::from_bits(0x450fda12), f32::from_bits(0x44502f18), f32::from_bits(0x43ee34f4), f32::from_bits(0x45151f8f), f32::from_bits(0x44f7b2e2), f32::from_bits(0x435bfec2)], turn: [f32::from_bits(0x403554c1), f32::from_bits(0x3ff29f35), f32::from_bits(0x3f696c6f)], mass: f32::from_bits(0x4295196f), moi: [f32::from_bits(0x423c6e2f), f32::from_bits(0x4388615e), f32::from_bits(0x43940dff)], desired: [f32::from_bits(0x3f4b7e19), f32::from_bits(0xbe239a4f), f32::from_bits(0x3f15d8a6)], vel_target: [f32::from_bits(0x436e77c5), f32::from_bits(0xc23fb8d5), f32::from_bits(0x432f99e3)], sinbank: f32::from_bits(0x00000000),
              vv_force: [0x46f378be, 0x452a8f14, 0x45455b5c], hd_torque: [0x44353930, 0x43ee34f4, 0xc35bfec2], hd_rotspeed: [0x3f42ae22, 0x398c16cd, 0x3ed0dce2], hd_ret: 0, zv_force: [0x463261eb, 0x470fe3e2, 0x44f20701], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x428cb597), f32::from_bits(0x43068d6f), f32::from_bits(0xc2d15c49)], coordsys: [f32::from_bits(0x3d2a20ee), f32::from_bits(0x3e5eb9ad), f32::from_bits(0x3f79a512), f32::from_bits(0x3f7b74c8), f32::from_bits(0xbe40131f), f32::from_bits(0x00000000), f32::from_bits(0x3e3b4e73), f32::from_bits(0x3f7536ba), f32::from_bits(0xbe62bffd)], rotspeed: [f32::from_bits(0x3f47ef49), f32::from_bits(0x3ef3888c), f32::from_bits(0x3ebc9638)], thrust: [f32::from_bits(0x4524ad66), f32::from_bits(0x45bf8332), f32::from_bits(0x46c44128), f32::from_bits(0x465facba), f32::from_bits(0x46aaac27), f32::from_bits(0x46a9f03d)], rot: [f32::from_bits(0x439d697d), f32::from_bits(0x43b74c14), f32::from_bits(0x43e9ed7d), f32::from_bits(0x43ce50a8), f32::from_bits(0x443d07f7), f32::from_bits(0x44ec298e)], turn: [f32::from_bits(0x3f037967), f32::from_bits(0x40389820), f32::from_bits(0x40190e0f)], mass: f32::from_bits(0x437cd96a), moi: [f32::from_bits(0x42fadd4a), f32::from_bits(0x423f2f71), f32::from_bits(0x437766f8)], desired: [f32::from_bits(0xbc4629a2), f32::from_bits(0xbecee175), f32::from_bits(0x3f6a268e)], vel_target: [f32::from_bits(0xc06838ca), f32::from_bits(0xc2f27035), f32::from_bits(0x43893297)], sinbank: f32::from_bits(0x3f000000),
              vv_force: [0xc68c14f5, 0xc68952c8, 0x45e6d0f4], hd_torque: [0xc3b74c14, 0x43e9ed7d, 0x443d07f7], hd_rotspeed: [0x3f47ef49, 0x3ef3888c, 0x3ebc9638], hd_ret: 0, zv_force: [0xc68c14f5, 0xc68952c8, 0x45e6d0f4], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x40634bb8), f32::from_bits(0x43193752), f32::from_bits(0xc337bc59)], coordsys: [f32::from_bits(0x3e4b235f), f32::from_bits(0x3e6fc0f6), f32::from_bits(0x3f73a5df), f32::from_bits(0xbf43518c), f32::from_bits(0x3f257d3b), f32::from_bits(0x00000000), f32::from_bits(0xbf1d8118), f32::from_bits(0xbf39e4f5), f32::from_bits(0x3e9d1ec2)], rotspeed: [f32::from_bits(0xbf33ef67), f32::from_bits(0xbf197472), f32::from_bits(0x3f6a9f26)], thrust: [f32::from_bits(0x46093fc9), f32::from_bits(0x46a1bbc6), f32::from_bits(0x46545bbd), f32::from_bits(0x467351b5), f32::from_bits(0x45f7c23e), f32::from_bits(0x465508e4)], rot: [f32::from_bits(0x43e19df4), f32::from_bits(0x44b6fe73), f32::from_bits(0x450e416f), f32::from_bits(0x44ae7424), f32::from_bits(0x4487a53d), f32::from_bits(0x44fa672d)], turn: [f32::from_bits(0x3f96450a), f32::from_bits(0x3fb6142e), f32::from_bits(0x3fcf4d5b)], mass: f32::from_bits(0x4324b08b), moi: [f32::from_bits(0x42e60f1a), f32::from_bits(0x42d34a4b), f32::from_bits(0x42373c36)], desired: [f32::from_bits(0xbeefda30), f32::from_bits(0xbf606703), f32::from_bits(0xbde1e3c9)], vel_target: [f32::from_bits(0xc30c89d8), f32::from_bits(0xc3837c5c), f32::from_bits(0xc2045b78)], sinbank: f32::from_bits(0x00000000),
              vv_force: [0x4608a75a, 0xc6571cac, 0x4628a472], hd_torque: [0x43e19df4, 0x422f8e7d, 0x00000000], hd_rotspeed: [0xbf33ef67, 0xbf197472, 0x3f6a9f26], hd_ret: 0, zv_force: [0x4608a75a, 0xc6571cac, 0x4628a472], zv_ret: 0 },
        Row { vel: [f32::from_bits(0xc1a218d2), f32::from_bits(0x430afc53), f32::from_bits(0xc33ed0f0)], coordsys: [f32::from_bits(0x3e80974b), f32::from_bits(0xbe4fc40c), f32::from_bits(0x3f724a19), f32::from_bits(0xbf20dfba), f32::from_bits(0xbf472318), f32::from_bits(0x00000000), f32::from_bits(0x3f3c78d0), f32::from_bits(0xbf18420e), f32::from_bits(0xbea54f49)], rotspeed: [f32::from_bits(0x3e377ed0), f32::from_bits(0x3f20f003), f32::from_bits(0x3f7529fe)], thrust: [f32::from_bits(0x458a8d4a), f32::from_bits(0x460aa373), f32::from_bits(0x467d3064), f32::from_bits(0x4688c089), f32::from_bits(0x45b0eac1), f32::from_bits(0x46cbdfe1)], rot: [f32::from_bits(0x441812a6), f32::from_bits(0x44cc8d2e), f32::from_bits(0x44f4889c), f32::from_bits(0x4503d986), f32::from_bits(0x43a9b379), f32::from_bits(0x452a4e3d)], turn: [f32::from_bits(0x403cd883), f32::from_bits(0x3f42d923), f32::from_bits(0x402c2672)], mass: f32::from_bits(0x43887bd1), moi: [f32::from_bits(0x42951368), f32::from_bits(0x430f27a0), f32::from_bits(0x43827f9f)], desired: [f32::from_bits(0x3ea29e3d), f32::from_bits(0xbf0dd1de), f32::from_bits(0x3f4501f0)], vel_target: [f32::from_bits(0x42be916f), f32::from_bits(0xc32631f0), f32::from_bits(0x4366de45)], sinbank: f32::from_bits(0x3f000000),
              vv_force: [0xc5992ae0, 0xc683cf11, 0x4514053b], hd_torque: [0xc48079ad, 0x4324d68f, 0xc528be98], hd_rotspeed: [0x3e377ed0, 0x3f20f003, 0x3f7529fe], hd_ret: 0, zv_force: [0xc5992ae0, 0xc683cf11, 0x4514053b], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x41838bbb), f32::from_bits(0xc31e5dbd), f32::from_bits(0x4342acd1)], coordsys: [f32::from_bits(0x3e857b76), f32::from_bits(0xbd974d43), f32::from_bits(0x3f766c19), f32::from_bits(0x3e8b975a), f32::from_bits(0x3f764d42), f32::from_bits(0xa3800000), f32::from_bits(0xbf6d163f), f32::from_bits(0x3e865e62), f32::from_bits(0x3e8abcf9)], rotspeed: [f32::from_bits(0xbda9c534), f32::from_bits(0x3f51ceaa), f32::from_bits(0xbf31cacd)], thrust: [f32::from_bits(0x4631e74d), f32::from_bits(0x4635256a), f32::from_bits(0x46e28a2c), f32::from_bits(0x468ca615), f32::from_bits(0x45c9804f), f32::from_bits(0x4657ef1a)], rot: [f32::from_bits(0x450bd490), f32::from_bits(0x448391a5), f32::from_bits(0x44defb3d), f32::from_bits(0x447cbaec), f32::from_bits(0x45230925), f32::from_bits(0x44d16bae)], turn: [f32::from_bits(0x3fcfbd18), f32::from_bits(0x3fd04253), f32::from_bits(0x403a7eb7)], mass: f32::from_bits(0x416e6271), moi: [f32::from_bits(0x42d68561), f32::from_bits(0x42d8cdac), f32::from_bits(0x438620b9)], desired: [f32::from_bits(0xbf142dd1), f32::from_bits(0x3f4daab3), f32::from_bits(0x3e0f1e12)], vel_target: [f32::from_bits(0xc32da5b1), f32::from_bits(0x4371040a), f32::from_bits(0x4227b73d)], sinbank: f32::from_bits(0x00000000),
              vv_force: [0xc4880cb8, 0x46edde8e, 0xc6131177], hd_torque: [0xc48391a5, 0xc47cbaec, 0x45230925], hd_rotspeed: [0xbda9c534, 0x3f51ceaa, 0xbf31cacd], hd_ret: 0, zv_force: [0x45797bfa, 0x46e29f64, 0xc62a4b0d], zv_ret: 0 },
        Row { vel: [f32::from_bits(0x4342d60f), f32::from_bits(0xc1f2f58f), f32::from_bits(0xc2d4ac25)], coordsys: [f32::from_bits(0x3e5f5661), f32::from_bits(0x3d23b796), f32::from_bits(0x3f79a09b), f32::from_bits(0x3e389619), f32::from_bits(0xbf7bce74), f32::from_bits(0x23000000), f32::from_bits(0x3f7589c9), f32::from_bits(0x3e33fdcc), f32::from_bits(0xbe630e8d)], rotspeed: [f32::from_bits(0x3f2522d5), f32::from_bits(0xbf46982e), f32::from_bits(0x3f474920)], thrust: [f32::from_bits(0x46c13569), f32::from_bits(0x46180b41), f32::from_bits(0x466a8c4b), f32::from_bits(0x46cbcb31), f32::from_bits(0x461df42a), f32::from_bits(0x46c17cf4)], rot: [f32::from_bits(0x4535e49a), f32::from_bits(0x44b130cc), f32::from_bits(0x450e2297), f32::from_bits(0x4502e362), f32::from_bits(0x45238ae7), f32::from_bits(0x453a9361)], turn: [f32::from_bits(0x40277888), f32::from_bits(0x4033f356), f32::from_bits(0x400c554f)], mass: f32::from_bits(0x43ac0078), moi: [f32::from_bits(0x42fb2fdf), f32::from_bits(0x4389e26d), f32::from_bits(0x431a861c)], desired: [f32::from_bits(0x3f1004de), f32::from_bits(0x3f0916a6), f32::from_bits(0x3f213fc6)], vel_target: [f32::from_bits(0x4328c5b4), f32::from_bits(0x4320a68b), f32::from_bits(0x433cf6bc)], sinbank: f32::from_bits(0x3f000000),
              vv_force: [0xc6b42d6d, 0x46ae2b00, 0x46e74d5c], hd_torque: [0x446b0dbc, 0x450e2297, 0xc53a9361], hd_rotspeed: [0x3f2522d5, 0xbf46982e, 0x3f474920], hd_ret: 0, zv_force: [0xc6b42d6d, 0x46ae2b00, 0x46e74d5c], zv_ret: 0 },
    ];

    fn v(a: [f32; 3]) -> Vector {
        Vector::new(a[0], a[1], a[2])
    }

    fn bits(v: Vector) -> [u32; 3] {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
    }

    #[test]
    fn matches_emulated_binary() {
        let tw = SimTweaks::compiled();
        for (i, r) in ROWS.iter().enumerate() {
            let mut stat = ShipStaticInfo::new(ShipType::sAcolyte, String::new());
            stat.header.mass = r.mass;
            stat.header.moment_of_inertia = v(r.moi);
            let mut s = Ship::new(ShipId(0), ShipType::sAcolyte, ShipRace::Somtaaw, 0);
            s.flags = sof::ROTATABLE | sof::IMPACTABLE;
            s.posinfo.velocity = v(r.vel);
            s.rotinfo.coordsys = Matrix { m: r.coordsys };
            s.rotinfo.rotspeed = v(r.rotspeed);
            s.nonstat.thrust_strength = r.thrust;
            s.nonstat.rot_strength = r.rot;
            s.nonstat.turn_speed = r.turn;
            let base = s.clone();

            velocity_vector(&mut s, &stat, &tw, 0.0625, v(r.vel_target));
            assert_eq!(bits(s.posinfo.force), r.vv_force, "row {i} velocity guidance");

            let mut s = base.clone();
            let mut d = v(r.desired);
            let ret = heading(&mut s, &stat, &tw, 0.0625, &mut d, 0.999, 0.999, r.sinbank, 0);
            assert_eq!(ret as u8, r.hd_ret, "row {i} heading result");
            assert_eq!(bits(s.rotinfo.torque), r.hd_torque, "row {i} heading torque");
            assert_eq!(bits(s.rotinfo.rotspeed), r.hd_rotspeed, "row {i} heading rotspeed");

            let mut s = base.clone();
            let ret = zero_velocity(&mut s, &stat, &tw, 0.0625);
            assert_eq!(ret as u8, r.zv_ret, "row {i} zero velocity result");
            assert_eq!(bits(s.posinfo.force), r.zv_force, "row {i} zero velocity force");
        }
    }
}
