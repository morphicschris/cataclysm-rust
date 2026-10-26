//! `Physics.c` as compiled into Cataclysm: integrating forces and torques into the position,
//! velocity and orientation of a ship, once per universe tick.
//!
//! Every floating-point step follows the disassembly of `physUpdateObjPosVelShip` (0x4dfdf0):
//! which products are stored to `float` locals and which stay in an FPU register decides the
//! low bits of the result (see `hwc_math::fpu`).

use hwc_math::fpu::{eq_or_unordered, ld, st};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::object::{sof, Ship, ISMOVING_MOVING, ISMOVING_ROTATING};
use crate::statics::ShipStaticInfo;
use crate::tactics::{self, MaxVelocityContext};
use crate::tunables::SimTweaks;

/// What the physics step needs from the universe.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct PhysContext {
    /// `universe.phystimeelapsed`.
    pub dt: f32,
    /// `universe.univUpdateCounter` (selects the frames that refresh collision rectangles).
    pub update_counter: u32,
    /// Single-player games skip the world-bounds clamp.
    pub single_player: bool,
    /// Half-extents of the world (`universe` size, 0x888c58..0x888c60).
    pub world_bounds: Vector,
    pub max_vel: MaxVelocityContext,
    /// `universe.totaltimeelapsed` (the field slow-down's stopwatch).
    pub now: f32,
}

/// Speed a ship flung by a field (`+0x550`) may keep (0x883664); the cap falls back to the
/// ship's own over the first half of the slow-down (0x883668).
const FLUNG_SPEED: f32 = 1400.0;
const FLUNG_RAMP: f64 = 0.5;

/// The integer "is any component non-zero" test the original makes on a vector's bit
/// patterns (so `-0.0` counts as non-zero).
fn bits_nonzero(v: Vector) -> bool {
    (v.x.to_bits() | v.y.to_bits() | v.z.to_bits()) != 0
}

/// `physApplyForceToObj` (0x4df890): adds `force` along one of the ship's axes (odd directions
/// are the negative axis). Ignored for ships that never take forces and for crazy ships.
pub fn apply_force(ship: &mut Ship, force: f32, direction: u16) {
    // (The single-player clamp check on +0x4c4 is not modelled.)
    if ship.dont_apply_force_ever != 0 || ship.flags & sof::CRAZY != 0 {
        return;
    }
    apply_force_to(&mut ship.posinfo.force, &ship.rotinfo.coordsys, force, direction);
}

/// The arithmetic of `physApplyForceToObj` on any object (missiles take it unconditionally).
pub fn apply_force_to(f: &mut Vector, coordsys: &Matrix, force: f32, direction: u16) {
    let col = match direction {
        0 | 1 => 0,
        2 | 3 => 1,
        4 | 5 => 2,
        _ => return,
    };
    let axis = coordsys.col(col);
    let s = if direction & 1 != 0 { -ld(force) } else { ld(force) };
    // x and y products are stored to float locals; z stays in a register.
    let fx = st(ld(axis.x) * s);
    let fy = st(ld(axis.y) * s);
    f.x = st(ld(fx) + ld(f.x));
    f.y = st(ld(fy) + ld(f.y));
    f.z = st(ld(axis.z) * s + ld(f.z));
}

/// `physApplyRotToObj` (0x4df9c0): adds a torque about one of the ship's axes.
pub fn apply_rot(ship: &mut Ship, torque: f32, direction: u16) {
    if ship.flags & sof::CRAZY != 0 {
        return;
    }
    let t = if direction & 1 != 0 { -torque } else { torque };
    let tq = &mut ship.rotinfo.torque;
    match direction {
        0 | 1 => tq.x += t,
        2 | 3 => tq.y += t,
        4 | 5 => tq.z += t,
        _ => {}
    }
}

/// Cataclysm's world-bounds velocity clamp (0x4dfcf0): a velocity component pointing further
/// out is zeroed once the ship is within 300 m of the world's edge on that axis.
pub fn clamp_to_world_bounds(ship: &mut Ship, bounds: Vector) {
    let margin = ld(300.0f32);
    let bx = ld(bounds.x) - margin; // kept in a register
    let by = ld(st(ld(bounds.y) - margin));
    let bz = ld(st(ld(bounds.z) - margin));
    let p = ship.posinfo.position;
    let v = &mut ship.posinfo.velocity;
    let clamp = |pos: f32, vel: &mut f32, b: f64| {
        let (pos, ve) = (ld(pos), ld(*vel));
        if (b < pos && ve > 0.0) || (-b > pos && ve < 0.0) {
            *vel = 0.0;
        }
    };
    clamp(p.x, &mut v.x, bx);
    clamp(p.y, &mut v.y, by);
    clamp(p.z, &mut v.z, bz);
}

/// Multiplies the coordinate system by a rotation of `rotspeed * dt` about one axis.
pub(crate) fn rotate(coordsys: &mut Matrix, rotspeed: f32, dt: f32, make: fn(f32, f32) -> Matrix) {
    let angle = ld(dt) * ld(rotspeed);
    let (s, c) = (st(fsin(angle)), st(fcos(angle)));
    *coordsys = coordsys.mul_mat(&make(c, s));
}

/// `physUpdateObjPosVelShip` (0x4dfdf0).
pub fn update_ship(ship: &mut Ship, stat: &ShipStaticInfo, tw: &SimTweaks, ctx: &PhysContext) {
    let dt = ld(ctx.dt);
    let h = &stat.header;
    let one_over_mass = ld(h.one_over_mass);
    let force = ship.posinfo.force;
    // a = F/m * t: x keeps F/m in a register, y and z store it first.
    let ax = st(one_over_mass * ld(force.x) * dt);
    let ay = st(ld(st(one_over_mass * ld(force.y))) * dt);
    let az = st(ld(st(one_over_mass * ld(force.z))) * dt);
    // (+0x550: a ship slowed by a field keeps its velocity; forces do not change it.)
    if !ship.disable.slowed {
        let v = &mut ship.posinfo.velocity;
        v.x += ax;
        v.y += ay;
        v.z += az;
    }

    if bits_nonzero(ship.posinfo.velocity) {
        ship.posinfo.is_moving = ISMOVING_MOVING;
        let mut cap = tactics::capped_speed(ship, stat, ctx.max_vel);
        if ship.disable.slowed {
            // A ship slowed by a field (`+0x550`, stopwatch `+0x560`) may go at 1400 m/s,
            // falling back to its own top speed over the first half of the slow-down.
            let own = cap;
            if own < FLUNG_SPEED {
                cap = FLUNG_SPEED;
            }
            let frac = ship.disable.slow_timer.fraction(ctx.now);
            let span = ld(FLUNG_SPEED) - ld(own);
            if frac < FLUNG_RAMP && span > 0.0 {
                cap = st((1.0 - ld(st(frac))) * span + ld(own));
            }
        }
        ship.posinfo.velocity.cap_sloppy(cap);
        if !ctx.single_player {
            clamp_to_world_bounds(ship, ctx.world_bounds);
        }
        let v = ship.posinfo.velocity;
        let p = &mut ship.posinfo.position;
        // d.y and d.z are stored before the additions; d.x is added from the register.
        let dy = st(dt * ld(v.y));
        let dz = st(dt * ld(v.z));
        p.x = st(dt * ld(v.x) + ld(p.x));
        p.y += dy;
        p.z += dz;
        // (Fog-of-war vision update not modelled.)
    } else {
        ship.posinfo.is_moving = 0;
    }

    // Crazy or disabled ships drift to a stop when nothing pushes them.
    let f = ship.flags;
    if f & sof::DEAD == 0
        && f & (sof::CRAZY | sof::DISABLED) != 0
        && ship.posinfo.is_moving != 0
        && !bits_nonzero(Vector::new(ax, ay, az))
    {
        let v = ship.posinfo.velocity;
        let (lo, hi) = (ld(tw.considered_still_lo), ld(tw.considered_still_hi));
        let still = |c: f32| ld(c) > lo && ld(c) < hi;
        if still(v.x) && still(v.y) && still(v.z) {
            ship.posinfo.is_moving = 0;
            ship.posinfo.velocity = Vector::ZERO;
        } else if !ship.disable.disabled && !ship.disable.slowed {
            let k = tw.ship_crazy_friction;
            let v = &mut ship.posinfo.velocity;
            v.x *= k;
            v.y *= k;
            v.z *= k;
        }
    }

    // Torque -> rotation speed.
    let tq = ship.rotinfo.torque;
    let zero = |x: f32| eq_or_unordered(ld(x), 0.0);
    if !(zero(tq.x) && zero(tq.y) && zero(tq.z)) {
        let i = h.one_over_moment_of_inertia;
        // x: the whole term stays in a register up to the addition; y and z store I*t and
        // then the term.
        let ry = st(ld(st(ld(i.y) * ld(tq.y))) * dt);
        let rz = st(ld(st(ld(i.z) * ld(tq.z))) * dt);
        let r = &mut ship.rotinfo.rotspeed;
        r.x = st(ld(i.x) * ld(tq.x) * dt + ld(r.x));
        r.y += ry;
        r.z += rz;
        r.cap_sloppy(h.max_rot);
    }

    let mut changed = false;
    let rs = ship.rotinfo.rotspeed;
    if !zero(rs.x) {
        rotate(&mut ship.rotinfo.coordsys, rs.x, ctx.dt, Matrix::rot_about_x);
        ship.posinfo.is_moving |= ISMOVING_ROTATING;
        changed = true;
    }
    if !zero(rs.y) {
        rotate(&mut ship.rotinfo.coordsys, rs.y, ctx.dt, Matrix::rot_about_y);
        ship.posinfo.is_moving |= ISMOVING_ROTATING;
        changed = true;
    }
    if !zero(rs.z) {
        rotate(&mut ship.rotinfo.coordsys, rs.z, ctx.dt, Matrix::rot_about_z);
        ship.posinfo.is_moving |= ISMOVING_ROTATING;
        changed = true;
    }

    if ship.flags & sof::IMPACTABLE != 0 || ship.flags2 & 2 != 0 {
        if changed {
            ship.coll.coll_offset = ship.rotinfo.coordsys.mul_vec(h.coll.coll_sphere_offset);
            if ctx.update_counter & 3 == 3 {
                crate::collision::update_coll_rect(ship, stat);
            }
        }
        ship.coll.coll_position = ship.coll.coll_offset + ship.posinfo.position;
        if changed && !stat.engine_nozzle_offset[0].is_zero_or_unordered() {
            ship.engine_offset = ship.rotinfo.coordsys.mul_vec(stat.engine_nozzle_offset[0]);
        }
        ship.engine_position = ship.engine_offset + ship.posinfo.position;
    }

    ship.posinfo.force = Vector::ZERO;
    ship.rotinfo.torque = Vector::ZERO;
}

/// `univUpdateObjRotInfo` (0x53b340): recomputes the collision and engine positions after the
/// coordinate system or position was set directly.
pub fn update_obj_rot_info(ship: &mut Ship, stat: &ShipStaticInfo) {
    if ship.flags & sof::IMPACTABLE == 0 {
        return;
    }
    ship.coll.coll_offset = ship.rotinfo.coordsys.mul_vec(stat.header.coll.coll_sphere_offset);
    ship.coll.coll_position = ship.posinfo.position + ship.coll.coll_offset;
    crate::collision::update_coll_rect(ship, stat);
    ship.engine_offset = ship.rotinfo.coordsys.mul_vec(stat.engine_nozzle_offset[0]);
    ship.engine_position = ship.posinfo.position + ship.engine_offset;
}

/// The `== 0.0` tests on the nozzle offset use the unordered-is-equal pattern.
trait ZeroOrUnordered {
    fn is_zero_or_unordered(&self) -> bool;
}

impl ZeroOrUnordered for Vector {
    fn is_zero_or_unordered(&self) -> bool {
        eq_or_unordered(ld(self.x), 0.0) && eq_or_unordered(ld(self.y), 0.0) && eq_or_unordered(ld(self.z), 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ShipId, ShipTable};
    use crate::statics::{ShipRace, ShipStaticInfo, ShipType};

    struct Case {
        pos: [f32; 3],
        vel: [f32; 3],
        force: [f32; 3],
        coordsys: [f32; 9],
        rotspeed: [f32; 3],
        torque: [f32; 3],
        health: f32,
        exp_pos: [u32; 3],
        exp_vel: [u32; 3],
        exp_rot: [u32; 3],
        /// Collision position, when the emulation got past the rotation (no FSIN in Ghidra's
        /// emulator).
        exp_coll: Option<[u32; 3]>,
    }

    /// Inputs and results of emulating `physUpdateObjPosVelShip` (0x4dfdf0) in cataclysm.exe
    /// (Ghidra p-code emulation; constructed ships, see the tools in the report). The
    /// emulator stops at the first FSIN, so for rotating ships only position, velocity and
    /// rotation speed are compared.
    const CASES: &[Case] = &[
        Case { pos: [f32::from_bits(0xc4d99e09), f32::from_bits(0xc522310a), f32::from_bits(0x454998f0)], vel: [f32::from_bits(0xc366f665), f32::from_bits(0xc1642080), f32::from_bits(0x420af286)], force: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f741a82), f32::from_bits(0x3e9a43f9), f32::from_bits(0x00000000), f32::from_bits(0x3e9a43f9), f32::from_bits(0xbf741a82), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], torque: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], health: f32::from_bits(0x42f00000), exp_pos: [0xc4db6bf6, 0xc5223f4c, 0x4549bbad], exp_vel: [0xc366f665, 0xc1642080, 0x420af286], exp_rot: [0x00000000, 0x00000000, 0x00000000], exp_coll: Some([0xc4db6bf6, 0xc5223f4c, 0x4549bbad]) },
        Case { pos: [f32::from_bits(0xc54c9339), f32::from_bits(0x4543d8d7), f32::from_bits(0x42924c47)], vel: [f32::from_bits(0x43496a8c), f32::from_bits(0xc33e468d), f32::from_bits(0xc2d638a5)], force: [f32::from_bits(0xc72141c4), f32::from_bits(0x46d9bfd6), f32::from_bits(0x45544d3a)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3e899ccf), f32::from_bits(0x3f76947b), f32::from_bits(0x00000000), f32::from_bits(0x3f76947b), f32::from_bits(0xbe899ccf), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0xbf170d59), f32::from_bits(0xbf1ddb14), f32::from_bits(0xbf568ddb)], torque: [f32::from_bits(0xc53b6ae3), f32::from_bits(0x441642ad), f32::from_bits(0xc41be5c6)], health: f32::from_bits(0x447a0000), exp_pos: [0xc54c1fcf, 0x454354a1, 0x4285cb31], exp_vel: [0x42e6d335, 0xc3043587, 0xc2c8115d], exp_rot: [0xc0200000, 0x40200000, 0xc0200000], exp_coll: None },
        Case { pos: [f32::from_bits(0xc5846818), f32::from_bits(0x45542de6), f32::from_bits(0x4596a6bf)], vel: [f32::from_bits(0x430fd1b5), f32::from_bits(0x4311e152), f32::from_bits(0x4351d8ba)], force: [f32::from_bits(0xc71e3b96), f32::from_bits(0xc5724281), f32::from_bits(0xc72452e3)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f755666), f32::from_bits(0xbe9239df), f32::from_bits(0x00000000), f32::from_bits(0xbe9239df), f32::from_bits(0xbf755666), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x3f1029b7), f32::from_bits(0x3d9b8a6c), f32::from_bits(0xbf497be7)], torque: [f32::from_bits(0xc40a1ab4), f32::from_bits(0x452d06af), f32::from_bits(0x44022291)], health: f32::from_bits(0x42f00000), exp_pos: [0xc5844a61, 0x4554b7b4, 0x4596e3da], exp_vel: [0x426db6a4, 0x4309ce09, 0x42f46a1b], exp_rot: [0xc0200000, 0x40200000, 0x400aea93], exp_coll: None },
        Case { pos: [f32::from_bits(0x4588843a), f32::from_bits(0xc58aa080), f32::from_bits(0x455670e5)], vel: [f32::from_bits(0x438e34b1), f32::from_bits(0x4316a28a), f32::from_bits(0x4385c294)], force: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf30e678), f32::from_bits(0x3f390bf7), f32::from_bits(0x00000000), f32::from_bits(0x3f390bf7), f32::from_bits(0x3f30e678), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], torque: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], health: f32::from_bits(0x447a0000), exp_pos: [0x4589126f, 0xc58a552f, 0x45577c6a], exp_vel: [0x438e34b1, 0x4316a28a, 0x4385c294], exp_rot: [0x00000000, 0x00000000, 0x00000000], exp_coll: Some([0x4589126f, 0xc58a552f, 0x45577c6a]) },
        Case { pos: [f32::from_bits(0x47c2ec00), f32::from_bits(0xc22781ce), f32::from_bits(0xc58feb6c)], vel: [f32::from_bits(0x43480000), f32::from_bits(0x4392ca47), f32::from_bits(0xc36542ea)], force: [f32::from_bits(0x473db2b7), f32::from_bits(0x471e3fd7), f32::from_bits(0x469316b6)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3ea8170b), f32::from_bits(0x3f71cf8d), f32::from_bits(0x00000000), f32::from_bits(0x3f71cf8d), f32::from_bits(0xbea8170b), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0xbf6359b9), f32::from_bits(0xbd69554d), f32::from_bits(0x3ec48ff1)], torque: [f32::from_bits(0xc57d015b), f32::from_bits(0xc4fc34c8), f32::from_bits(0xc57f1c01)], health: f32::from_bits(0x42f00000), exp_pos: [0x47c2ec00, 0xc1c8039c, 0xc5904a71], exp_vel: [0x00000000, 0x43870000, 0xc33e09a8], exp_rot: [0xc0200000, 0xc0200000, 0xc0200000], exp_coll: None },
        Case { pos: [f32::from_bits(0x4581c4c7), f32::from_bits(0x4584650e), f32::from_bits(0x451cb9a2)], vel: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], force: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f557464), f32::from_bits(0x3f0d5385), f32::from_bits(0x00000000), f32::from_bits(0x3f0d5385), f32::from_bits(0xbf557464), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0xbf7bcc2b), f32::from_bits(0xbf52dbd3), f32::from_bits(0x3f594333)], torque: [f32::from_bits(0xc571971a), f32::from_bits(0xc4892d3f), f32::from_bits(0xc370fd05)], health: f32::from_bits(0x447a0000), exp_pos: [0x4581c4c7, 0x4584650e, 0x451cb9a2], exp_vel: [0x00000000, 0x00000000, 0x00000000], exp_rot: [0xc0200000, 0xc0200000, 0xbf05441b], exp_coll: None },
        Case { pos: [f32::from_bits(0xc563bc09), f32::from_bits(0xc5150b8f), f32::from_bits(0x4529f48a)], vel: [f32::from_bits(0xc3786cb0), f32::from_bits(0xc3497365), f32::from_bits(0xc38e78be)], force: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f2b5345), f32::from_bits(0x3f3e3853), f32::from_bits(0x00000000), f32::from_bits(0x3f3e3853), f32::from_bits(0xbf2b5345), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], torque: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], health: f32::from_bits(0x42f00000), exp_pos: [0xc564b476, 0xc515d502, 0x4528e68a], exp_vel: [0xc3786cb0, 0xc3497365, 0xc3870000], exp_rot: [0x00000000, 0x00000000, 0x00000000], exp_coll: Some([0xc564b476, 0xc515d502, 0x4528e68a]) },
        Case { pos: [f32::from_bits(0xc2d1979b), f32::from_bits(0xc5111f1d), f32::from_bits(0x45540183)], vel: [f32::from_bits(0xc36e2aa5), f32::from_bits(0x42922491), f32::from_bits(0xc2f5f2dc)], force: [f32::from_bits(0xc6aecc9c), f32::from_bits(0xc59091de), f32::from_bits(0xc6f2e0eb)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f178437), f32::from_bits(0xbf4e58a2), f32::from_bits(0x00000000), f32::from_bits(0xbf4e58a2), f32::from_bits(0xbf178437), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x3e3c1499), f32::from_bits(0xbf5bc6a3), f32::from_bits(0xbf765e4f)], torque: [f32::from_bits(0xc5602bbe), f32::from_bits(0xc58024bc), f32::from_bits(0x456c672b)], health: f32::from_bits(0x447a0000), exp_pos: [0xc2f5308e, 0xc510dfae, 0x455345c5], exp_vel: [0xc38e63ce, 0x427dbbd6, 0xc33bbdf1], exp_rot: [0xc0200000, 0xc0200000, 0x40200000], exp_coll: None },
        Case { pos: [f32::from_bits(0x44df6b43), f32::from_bits(0x452e685c), f32::from_bits(0xc5512845)], vel: [f32::from_bits(0x4387675e), f32::from_bits(0xc28ad640), f32::from_bits(0xc1becd06)], force: [f32::from_bits(0xc49200e4), f32::from_bits(0x46988eb4), f32::from_bits(0x46d1ac49)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3d8257ca), f32::from_bits(0xbf7f7b23), f32::from_bits(0x00000000), f32::from_bits(0xbf7f7b23), f32::from_bits(0xbd8257ca), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0xbf1cacc9), f32::from_bits(0x3f28177a), f32::from_bits(0xbe133a55)], torque: [f32::from_bits(0xc51b9a73), f32::from_bits(0x4563b06c), f32::from_bits(0x4568808a)], health: f32::from_bits(0x42f00000), exp_pos: [0x44e18403, 0x452e4b9f, 0xc5510835], exp_vel: [0x43862fe5, 0xc1e5e44c, 0x42004032], exp_rot: [0xc0200000, 0x40200000, 0x40200000], exp_coll: None },
        Case { pos: [f32::from_bits(0xc4dca86f), f32::from_bits(0x45815648), f32::from_bits(0x452c6b25)], vel: [f32::from_bits(0x438d0c56), f32::from_bits(0x4318e9bc), f32::from_bits(0x4395f024)], force: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f47b7b7), f32::from_bits(0x3f202713), f32::from_bits(0x00000000), f32::from_bits(0x3f202713), f32::from_bits(0xbf47b7b7), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], torque: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x00000000)], health: f32::from_bits(0x447a0000), exp_pos: [0xc4da743e, 0x4581a2bd, 0x452d9705], exp_vel: [0x438d0c56, 0x4318e9bc, 0x4395f024], exp_rot: [0x00000000, 0x00000000, 0x00000000], exp_coll: Some([0xc4da743e, 0x4581a2bd, 0x452d9705]) },
        Case { pos: [f32::from_bits(0xc497d646), f32::from_bits(0x451a15f8), f32::from_bits(0x441aa53a)], vel: [f32::from_bits(0x42f84cf2), f32::from_bits(0xc2e4fae2), f32::from_bits(0xc37caa8e)], force: [f32::from_bits(0x4688d3ac), f32::from_bits(0xc70aa852), f32::from_bits(0xc65b6fc0)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f3cb655), f32::from_bits(0x3f2cfc13), f32::from_bits(0x00000000), f32::from_bits(0x3f2cfc13), f32::from_bits(0xbf3cb655), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x3f1027fe), f32::from_bits(0xbf66ea00), f32::from_bits(0x3f5271b0)], torque: [f32::from_bits(0x445e2981), f32::from_bits(0x44189311), f32::from_bits(0x44dd6005)], health: f32::from_bits(0x42f00000), exp_pos: [0xc4969500, 0x45195987, 0x44166d3a], exp_vel: [0x4320a330, 0xc33c70d0, 0xc3870000], exp_rot: [0x40200000, 0x40200000, 0x40200000], exp_coll: None },
        Case { pos: [f32::from_bits(0xc59556da), f32::from_bits(0x4509db30), f32::from_bits(0x44584d19)], vel: [f32::from_bits(0x42a759bd), f32::from_bits(0x4341e784), f32::from_bits(0xc3096fde)], force: [f32::from_bits(0xc7047153), f32::from_bits(0x47372bcb), f32::from_bits(0xc72bb027)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbeb34537), f32::from_bits(0x3f6fcb76), f32::from_bits(0x00000000), f32::from_bits(0x3f6fcb76), f32::from_bits(0x3eb34537), f32::from_bits(0x00000000)], rotspeed: [f32::from_bits(0x3d003b4f), f32::from_bits(0x3e7de127), f32::from_bits(0x3f690467)], torque: [f32::from_bits(0xc5812c28), f32::from_bits(0x44fcf551), f32::from_bits(0xc5839f15)], health: f32::from_bits(0x447a0000), exp_pos: [0xc5955055, 0x450afec8, 0x4454b915], exp_vel: [0x4150a078, 0x4391cc3c, 0xc3650104], exp_rot: [0xc0200000, 0x40200000, 0xc0200000], exp_coll: None },
    ];

    fn v(a: [f32; 3]) -> Vector {
        Vector::new(a[0], a[1], a[2])
    }

    fn bits(v: Vector) -> [u32; 3] {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
    }

    #[test]
    fn matches_emulated_binary() {
        let mut stat = ShipStaticInfo::new(ShipType::sSwarmer, String::new());
        stat.ship_race = ShipRace::Somtaaw;
        stat.header.one_over_mass = 1.0 / 30.0;
        stat.header.one_over_moment_of_inertia = Vector::new(1.0 / 7.0, 1.0 / 9.0, 1.0 / 11.0);
        stat.header.max_velocity = 450.0;
        stat.header.max_rot = 2.5;
        stat.header.coll.coll_sphere_offset = Vector::new(3.0, -1.5, 10.0);
        stat.health.max_health = 1000.0;
        let tw = SimTweaks::compiled();
        let _ = ShipTable::default();
        for (i, c) in CASES.iter().enumerate() {
            let mut s = Ship::new(ShipId(0), ShipType::sSwarmer, ShipRace::Somtaaw, 0);
            s.flags = sof::ROTATABLE | sof::IMPACTABLE;
            s.posinfo.position = v(c.pos);
            s.posinfo.velocity = v(c.vel);
            s.posinfo.force = v(c.force);
            s.rotinfo.coordsys = Matrix { m: c.coordsys };
            s.rotinfo.rotspeed = v(c.rotspeed);
            s.rotinfo.torque = v(c.torque);
            s.health = c.health;
            let ctx = PhysContext {
                dt: 0.0625,
                update_counter: 0,
                single_player: false,
                world_bounds: Vector::new(100_000.0, 100_000.0, 100_000.0),
                max_vel: MaxVelocityContext::default(),
                now: 0.0,
            };
            update_ship(&mut s, &stat, &tw, &ctx);
            assert_eq!(bits(s.posinfo.position), c.exp_pos, "case {i} position");
            assert_eq!(bits(s.posinfo.velocity), c.exp_vel, "case {i} velocity");
            assert_eq!(bits(s.rotinfo.rotspeed), c.exp_rot, "case {i} rotation speed");
            if let Some(coll) = c.exp_coll {
                assert_eq!(bits(s.coll.coll_position), coll, "case {i} collision position");
            }
        }
    }
}
