//! `AIShip.c`: flying a ship to a point while steering around nearby objects
//! (`aishipFlyToPointAvoidingObjsFunc`, 0x414620, as modified in Cataclysm).
//!
//! The fly-to code is a small state machine on `Ship::ai_state`:
//! 0. decide whether a capital ship does the "fancy descend" (set `ai_descend`);
//! 1. optionally turn towards the destination first (car turn for small ships);
//! 2. fly: desired velocity = (destination - position) * VELOCITY_SCALE_FACTOR, capped, plus a
//!    repulsion away from every object in the ship's collision blob that is too close, then let
//!    `AITrack` turn and thrust towards it.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, ld, st, Ext};
use hwc_math::Vector;

use crate::aitrack;
use crate::command::{attr, order};
use crate::object::{sof, special, Ship, ShipId};
use crate::physics::apply_force;
use crate::statics::{ShipClass, ShipStaticInfo};
use crate::tactics::{self, is_capital_ship_static_or_big, is_strike_craft};
use crate::universe::Universe;

/// `AISHIP_*` flags.
pub mod flags {
    pub const FIRST_POINT_IN_DIRECTION_FLYING: u32 = 1;
    pub const POINT_IN_DIRECTION_FLYING: u32 = 2;
    pub const FAST_AS_POSSIBLE: u32 = 4;
    pub const CAR_TURN: u32 = 8;
    pub const DONT_FLY_TO_OBSCURED_POINTS: u32 = 16;
    pub const RETURN_IMMED_IF_POINT_OBSCURED: u32 = 32;
    pub const IGNORE_FORMATION_OBSCURED_POINTS: u32 = 64;
    pub const IGNORE_DOCK_WITH_OBSCURED_POINTS: u32 = 128;
    pub const STOP_IF_VELOCITY_TOO_LOW: u32 = 256;
}
use flags::*;

/// `AISHIP_FLY_OBJECT_IN_WAY`.
pub const FLY_OBJECT_IN_WAY: u32 = 1;

/// Accuracy the turn-first state uses (0x3f75c28f).
const TURN_FIRST_ACCURACY: f32 = f32::from_bits(0x3f75_c28f);

/// `GetCollSizeInDirection` (0x413fa0): half the collision box's extent along `dir`
/// (register value).
pub fn coll_size_in_direction(ship: &Ship, stat: &ShipStaticInfo, dir: Vector) -> Ext {
    let m = &ship.rotinfo.coordsys.m;
    let (dx, dy, dz) = (ld(dir.x), ld(dir.y), ld(dir.z));
    let comp = |a: usize| {
        let v = (dz * ld(m[a + 2]) + dy * ld(m[a + 1])) + dx * ld(m[a]);
        if v < 0.0 {
            -v
        } else {
            v
        }
    };
    let (up, right, heading) = (comp(0), comp(3), comp(6));
    let c = &stat.header.coll;
    ((heading * ld(c.forward_length) + right * ld(c.right_length)) + up * ld(c.up_length)) * 0.5
}

/// What the avoidance loop needs to know about another object.
struct Neighbour {
    id: ShipId,
    flags: u32,
    coll_optimize_dist: f32,
    coll_position: Vector,
    avoid_size: f32,
    avoid_pad: f32,
    avoid_modifier: f32,
    right_of_way: i8,
    command: Option<crate::object::CommandId>,
    special_flags: u32,
    docking_ship: Option<ShipId>,
    player: usize,
    ship: Ship,
}

/// `aishipFlyToPointAvoidingObjsFunc` (0x414620). Returns [`FLY_OBJECT_IN_WAY`] when the
/// destination is obscured or the resulting speed is negligible.
pub fn fly_to_point_avoiding_objs(
    u: &mut Universe,
    id: ShipId,
    destination: Option<Vector>,
    fly_flags: u32,
    mut limit_vel: f32,
    with_vel: Option<Vector>,
) -> u32 {
    let statics = u.statics.clone();
    let dt = u.phys_time_elapsed;
    let tw = u.tweaks.clone();
    let ship = u.ships.ship(id).clone();
    let Some(stat) = statics.ship(ship.ship_type) else { return 0 };
    if ship.dont_apply_force_ever != 0 || stat.header.immobile {
        return 0;
    }
    // (Single-player +0x4c4 check not modelled.)
    let cmd = ship.command.and_then(|c| u.commands.get(c)).cloned();
    let mut avoid_scale = tw.avoid_obj_scale_factor;
    if is_strike_craft(ship.ship_type) && cmd.as_ref().is_some_and(|c| c.order == order::ATTACK) {
        avoid_scale = tw.combat_avoid_obj_scale_factor;
    }

    let mut state = ship.ai_state;
    if state > 2 {
        return 0;
    }
    if state == 0 {
        let s = u.ships.ship_mut(id);
        if !eq_or_unordered(ld(stat.pitchdescend), 0.0) {
            s.ai_descend = 0.0;
            if let Some(dest) = destination {
                let p = s.posinfo.position;
                let dx = ld(dest.x) - ld(p.x);
                let dy = ld(dest.y) - ld(p.y);
                let mut dz = ld(dest.z) - ld(p.z);
                if dz < 0.0 {
                    dz = -dz;
                }
                let zleft = st(dz);
                let r2 = st(dy * dy + dx * dx);
                if !(ld(zleft) < ld(tw.min_dist_for_fancy_descend)) {
                    let r = fsqrt(ld(r2));
                    if r * ld(tw.min_angle_for_fancy_descend) <= ld(zleft) {
                        let inv = st(1.0 / ld(zleft));
                        s.ai_descend = inv;
                        let k = ld(tw.min_dist_to_track_heading);
                        if k * k > ld(r2) {
                            s.ai_descend = -inv;
                        } else if r * ld(tw.max_angle_for_track_heading) < ld(zleft) {
                            s.ai_descend = -s.ai_descend;
                        }
                    }
                }
            }
        }
        s.ai_state = 1;
        state = 1;
    }
    if state == 1 {
        if fly_flags & (FIRST_POINT_IN_DIRECTION_FLYING | CAR_TURN) != 0 {
            let dest = destination.unwrap_or_default();
            let s = u.ships.ship_mut(id);
            let p = s.posinfo.position;
            let mut t = Vector::new(dest.x - p.x, dest.y - p.y, dest.z - p.z);
            t.normalize();
            let done = if eq_or_unordered(ld(stat.pitchdescend), 0.0) {
                aitrack::heading(s, stat, &tw, dt, &mut t, TURN_FIRST_ACCURACY, TURN_FIRST_ACCURACY, stat.sinbank, aitrack::IGNORE_UP_VEC)
            } else {
                ld(s.ai_descend) < 0.0
                    || aitrack::heading_with_bank_pitch(s, stat, &tw, dt, t, TURN_FIRST_ACCURACY, stat.sinbank, 0.0, stat.pitchturn, aitrack::IGNORE_UP_VEC)
            };
            if done {
                s.ai_state = 2;
                return 0;
            }
            if fly_flags & CAR_TURN != 0 && !is_capital_ship_static_or_big(stat) {
                let f = st(ld(tw.car_turn_power) * ld(s.nonstat.thrust_strength[4]));
                apply_force(s, f, 4);
            } else {
                aitrack::steady_ship_drift_only(s, stat, &tw, dt);
            }
            return 0;
        }
        u.ships.ship_mut(id).ai_state = 2;
    }

    // --- STATE_START_FLYING
    let ship = u.ships.ship(id).clone();
    let p = ship.posinfo.position;
    let mut target = match destination {
        Some(d) => {
            let mut t = Vector::new(d.x - p.x, d.y - p.y, d.z - p.z);
            if fly_flags & FAST_AS_POSSIBLE == 0 {
                let k = tw.velocity_scale_factor;
                t = Vector::new(t.x * k, t.y * k, t.z * k);
            }
            t
        }
        None => Vector::ZERO,
    };
    if ld(ship.speed_cap) > 0.0 {
        limit_vel = ship.speed_cap;
    }
    if eq_or_unordered(ld(limit_vel), 0.0) {
        if let Some(c) = &cmd {
            let is_leader = c.selection.first() == Some(&id);
            let sphere_like = c.formation.formation_type == 5 || c.formation.formation_type == 10;
            if c.order != order::DOCK
                && c.attributes & attr::FORMATION != 0
                && is_leader
                && (!sphere_like || (c.attributes & attr::PROTECTING == 0 && c.formation.enders == 0))
            {
                limit_vel = c.formation.travel_vel;
            }
        }
    }
    if ld(limit_vel) < 0.0 {
        target.cap_min(-limit_vel);
    }
    if let Some(w) = with_vel {
        target = target + w;
    }
    if ld(limit_vel) > 0.0 {
        target.cap(limit_vel);
    } else {
        let cap = tactics::capped_speed(&ship, stat, u.max_velocity_context(&ship));
        target.cap(cap);
    }
    let mut desired = target;
    let mut result = 0u32;

    let skip_avoid = u.temp_disable_avoiding
        || ship.special_flags2 & 0x40 != 0
        || ship.docking_ship.is_some()
        || ship.special_flags & 2 != 0;
    'avoid: {
        if skip_avoid {
            break 'avoid;
        }
        let ship_size = stat.header.coll.avoid_coll_sphere_size;
        let mut ship_pad = stat.header.coll.avoid_coll_sphere_pad;
        let mut ship_row_pad = ship_pad;
        let mut row_enabled = false;
        let docking_small = cmd.as_ref().is_some_and(|c| c.order == order::DOCK)
            && ship.base.dock_point != -1
            && matches!(stat.ship_class, Some(ShipClass::Fighter) | Some(ShipClass::Corvette));
        if !docking_small && stat.header.right_of_way > 0 {
            row_enabled = true;
            ship_row_pad = st(ld(ship_size) * ld(tw.avoid_obj_row_padding_scale));
        }
        let Some(blob_id) = ship.coll_my_blob else { break 'avoid };
        let Some(blob) = u.blobs.get(blob_id).cloned() else { break 'avoid };
        let mut obscure_add = 0.0f32;
        // Small ships do not look at small targets.
        let first_pass = if ld(stat.header.coll.original_coll_sphere_size) < 500.0 { 1 } else { 0 };
        for pass in first_pass..2 {
            let (list, max_possible) = if pass == 0 {
                let mp = st(ld(blob.max_small_target_size) * ld(tw.avoid_obj_padding_scale) + ld(ship_row_pad));
                (&blob.small_targets, mp)
            } else {
                let scale = if ld(blob.max_big_target_size) > 1000.0 {
                    tw.avoid_obj_padding_scale_big
                } else {
                    tw.avoid_obj_padding_scale
                };
                let mp = st(ld(scale) * ld(blob.max_big_target_size) + ld(ship_row_pad));
                (&blob.big_targets, mp)
            };
            if list.is_empty() {
                continue;
            }
            let mut index = 0usize;
            if pass == 0 {
                // Binary search for the first target that can be within reach.
                let (mut bottom, mut top) = (0i32, list.len() as i32 - 1);
                let mut mid = 0i32;
                while top >= bottom {
                    mid = (bottom + top) >> 1;
                    let other = u.ships.ship(list[mid as usize]);
                    if ld(ship.coll_optimize_dist) - ld(other.coll_optimize_dist) > ld(max_possible) {
                        bottom = mid + 1;
                    } else {
                        top = mid - 1;
                    }
                }
                index = mid as usize;
            }
            while index < list.len() {
                let n = neighbour(u, list[index]);
                index += 1;
                let dist_check = st(ld(n.coll_optimize_dist) - ld(ship.coll_optimize_dist));
                if ld(dist_check) > ld(max_possible) {
                    break;
                }
                let abs_check = if ld(dist_check) < 0.0 { -ld(dist_check) } else { ld(dist_check) };
                let mut max_consider = st(ld(ship_row_pad) + ld(n.avoid_pad));
                if abs_check > ld(max_consider) {
                    continue;
                }
                let rep = Vector::new(
                    ship.coll.coll_position.x - n.coll_position.x,
                    ship.coll.coll_position.y - n.coll_position.y,
                    ship.coll.coll_position.z - n.coll_position.z,
                );
                let within = |v: Vector, m: f32| {
                    let neg = -m;
                    let ok = |c: f32| !(ld(c) < ld(neg) || c.is_nan()) && !(ld(c) > ld(m));
                    ok(v.x) && ok(v.y) && ok(v.z)
                };
                let mut normal_check = !row_enabled;
                if row_enabled {
                    if !within(rep, max_consider) {
                        continue;
                    }
                    if stat.header.right_of_way > n.right_of_way {
                        // We have right of way: friendly ships in our way are asked to move
                        // aside instead, so we do not avoid them, except ships of our own
                        // command and enemies when told not to plow through.
                        if n.flags & sof::HIDE != 0 || n.special_flags & special::ROW_GETTING_OUT_OF_WAY != 0 {
                            continue;
                        }
                        let allied = u.players.get(n.player).is_some_and(|p| p.is_ally(ship.player));
                        if allied {
                            if n.command.is_some() && n.command == ship.command {
                                normal_check = true;
                            } else {
                                row_maybe_signal(u, &ship, n.id);
                                continue;
                            }
                        } else if ship.special_flags2 & 1 != 0 {
                            normal_check = true;
                        } else {
                            continue;
                        }
                    } else {
                        normal_check = true;
                    }
                    if normal_check {
                        max_consider = st(ld(ship_pad) + ld(n.avoid_pad));
                        if abs_check > ld(max_consider) {
                            continue;
                        }
                    }
                }
                debug_assert!(normal_check);
                if !within(rep, max_consider) {
                    continue;
                }
                if n.flags & sof::HIDE != 0 || n.flags & sof::IMPACTABLE == 0 || n.flags & sof::NOT_BUMPABLE != 0 || n.id == id {
                    continue;
                }
                let avoid_size = n.avoid_size;
                if cmd.is_some() && n.command == ship.command {
                    // Same formation: closer padding (and our own padding shrinks for the rest).
                    let k = ld(tw.avoid_obj_formation_padding_scale);
                    let tpad = k * ld(avoid_size);
                    ship_pad = st(ld(ship_size) * k);
                    max_consider = st(ld(ship_pad) + tpad);
                    if !within(rep, max_consider) {
                        continue;
                    }
                }
                if n.docking_ship == Some(id) {
                    continue;
                }
                // A salvager latching onto its target does not avoid it.
                if ship.special_flags & crate::base::special::SALVAGER != 0 && ship.base.salvage.target == Some(n.id) && ship.base.salvage.latching {
                    continue;
                }
                // Ships clamped to us (modules, latched workers, salvagers) are not avoided.
                if n.ship.flags & sof::CLAMPED != 0 && n.ship.base.clamp.as_ref().and_then(|c| c.host) == Some(id) {
                    continue;
                }
                // (Salvager, cloaking and slaved-ship exceptions are not modelled.)
                let mag2 = (ld(rep.z) * ld(rep.z) + ld(rep.y) * ld(rep.y)) + ld(rep.x) * ld(rep.x);
                if mag2 > ld(max_consider) * ld(max_consider) {
                    continue;
                }
                let mut dist = st(fsqrt(mag2));
                let mut rx = rep.x;
                if ld(dist) < ld(0.1f32) {
                    dist = 0.1;
                    rx = 0.1;
                }
                let inv = 1.0 / ld(dist);
                let norm = Vector::new(st(inv * ld(rx)), st(inv * ld(rep.y)), st(inv * ld(rep.z)));
                let other_stat = statics.ship(n.ship.ship_type).expect("neighbour statics");
                let other_size = st(coll_size_in_direction(&n.ship, other_stat, norm));
                let collide = coll_size_in_direction(&ship, stat, norm) + ld(other_size);
                let mut to_collision = ld(dist) - collide;
                if to_collision < 20.0 {
                    to_collision = 20.0;
                }
                let k = 1.0 / (to_collision * ld(avoid_scale));
                let mut repulse = Vector::new(st(k * ld(norm.x)), st(k * ld(norm.y)), st(k * ld(norm.z)));

                if fly_flags & DONT_FLY_TO_OBSCURED_POINTS != 0 {
                    let mut skip = false;
                    if fly_flags & IGNORE_FORMATION_OBSCURED_POINTS != 0
                        && ship.command.is_some()
                        && ship.command == n.command
                        && cmd.as_ref().is_some_and(|c| c.attributes & attr::FORMATION != 0)
                    {
                        skip = true;
                    }
                    if !skip {
                        let obscure = ld(tw.avoid_obj_obscuredpoint_scale) * ld(avoid_size) + ld(obscure_add);
                        obscure_add = st(ld(tw.avoid_spread_out_dist) + ld(obscure_add));
                        let base = destination.unwrap_or(ship.posinfo.position);
                        let d = Vector::new(
                            n.coll_position.x - base.x,
                            n.coll_position.y - base.y,
                            n.coll_position.z - base.z,
                        );
                        let dmag2 = st((ld(d.z) * ld(d.z) + ld(d.y) * ld(d.y)) + ld(d.x) * ld(d.x));
                        if ld(dmag2) < obscure * obscure {
                            let obscured = if eq_or_unordered(ld(n.avoid_modifier), 0.0) {
                                true
                            } else {
                                let dd = st(fsqrt(ld(dmag2)));
                                let inv = 1.0 / ld(dd);
                                let dn = Vector::new(st(ld(d.x) * inv), st(ld(d.y) * inv), st(ld(d.z) * inv));
                                coll_size_in_direction(&n.ship, other_stat, dn) > ld(dd)
                            };
                            if obscured {
                                if fly_flags & RETURN_IMMED_IF_POINT_OBSCURED != 0 {
                                    return FLY_OBJECT_IN_WAY;
                                }
                                result |= FLY_OBJECT_IN_WAY;
                            }
                        }
                    }
                }

                // Pass on the right when the repulsion opposes where we want to go.
                let test = st((ld(target.z) * ld(norm.z) + ld(target.y) * ld(norm.y)) + ld(target.x) * ld(norm.x));
                if ld(test) < 0.0 {
                    let tmag2 = (ld(target.z) * ld(target.z) + ld(target.y) * ld(target.y)) + ld(target.x) * ld(target.x);
                    if !(tmag2 < 900.0) && !(tmag2 * ld(tw.use_pass_right_angle) > ld(test) * ld(test)) {
                        let mut pr = Vector::new(st(ld(target.y) - 0.0), st(0.0 - ld(target.x)), 0.0);
                        pr.normalize();
                        let m = ld(tw.pass_right_factor) * ld(dist);
                        repulse = Vector::new(
                            st(ld(st(m * ld(pr.x))) + ld(repulse.x)),
                            st(ld(st(m * ld(pr.y))) + ld(repulse.y)),
                            st(m * ld(pr.z) + ld(repulse.z)),
                        );
                    }
                }
                desired = desired + repulse;
            }
        }
    }

    let s = u.ships.ship_mut(id);
    let still = |c: f32| ld(c) > ld(tw.neg_avoid_min_vel) && ld(c) < ld(tw.avoid_min_vel);
    if fly_flags & (STOP_IF_VELOCITY_TOO_LOW | DONT_FLY_TO_OBSCURED_POINTS) != 0
        && still(desired.x)
        && still(desired.y)
        && still(desired.z)
    {
        if fly_flags & RETURN_IMMED_IF_POINT_OBSCURED != 0 {
            return FLY_OBJECT_IN_WAY;
        }
        result |= FLY_OBJECT_IN_WAY;
        desired = Vector::ZERO;
    } else if fly_flags & POINT_IN_DIRECTION_FLYING != 0 {
        let pd = stat.pitchdescend;
        if eq_or_unordered(ld(pd), 0.0) {
            let mut head = desired.copy_and_normalize();
            aitrack::heading(s, stat, &tw, dt, &mut head, tw.flyship_heading_accuracy, tw.flyship_heading_accuracy, stat.sinbank, 0);
        } else {
            let pitch = match destination {
                Some(dest) if !eq_or_unordered(ld(s.ai_descend), 0.0) => {
                    let a = ld(s.ai_descend);
                    let a = if a < 0.0 { -a } else { a };
                    let zleft = st((ld(dest.z) - ld(s.posinfo.position.z)) * a);
                    st(pitch_to_descend_with(&tw, zleft, pd))
                }
                _ => 0.0,
            };
            let head = if destination.is_some() && ld(s.ai_descend) < 0.0 { s.heading() } else { desired };
            aitrack::heading_with_bank_pitch(s, stat, &tw, dt, head, tw.flyship_heading_accuracy, stat.sinbank, pitch, stat.pitchturn, 0);
        }
    }
    aitrack::velocity_vector(s, stat, &tw, dt, desired);
    result
}

/// `aishipGetTrajectory` (0x413ef0): from the ship's position to the target's collision
/// centre.
pub fn trajectory(ship: &Ship, target: &Ship) -> Vector {
    let t = target.coll.coll_position;
    let p = ship.posinfo.position;
    Vector::new(st(ld(t.x) - ld(p.x)), st(ld(t.y) - ld(p.y)), st(ld(t.z) - ld(p.z)))
}

/// `aishipGetTrajectoryWithVelPrediction` (0x413e20): the trajectory led by where the target
/// will be when a bullet of `speed` arrives (no lead at or below `below`).
pub fn trajectory_with_vel_prediction(ship: &Ship, target: &Ship, speed: f32, below: f32) -> Vector {
    let mut t = trajectory(ship, target);
    if ld(speed) > ld(below) {
        let k = fsqrt((ld(t.z) * ld(t.z) + ld(t.y) * ld(t.y)) + ld(t.x) * ld(t.x)) / ld(speed);
        let v = target.posinfo.velocity;
        let a = st(k * ld(v.x));
        let b = st(k * ld(v.y));
        let c = k * ld(v.z);
        t.x = st(ld(a) + ld(t.x));
        t.y = st(ld(b) + ld(t.y));
        t.z = st(c + ld(t.z));
    }
    t
}

/// `aishipGetTrajectoryWithAngleCorrection` (0x413f20): from where the ship will be in a
/// quarter of a second.
pub fn trajectory_with_angle_correction(ship: &Ship, target: &Ship) -> Vector {
    let p = ship.posinfo.position;
    let v = ship.posinfo.velocity;
    let q = ld(0.25f32);
    let px = ld(v.x) * q + ld(p.x);
    let py = st(ld(v.y) * q + ld(p.y));
    let pz = st(ld(v.z) * q + ld(p.z));
    let t = target.coll.coll_position;
    Vector::new(st(ld(t.x) - px), st(ld(t.y) - ld(py)), st(ld(t.z) - ld(pz)))
}

/// The distance `fmathSqrtDouble` gives for a trajectory (register value).
pub fn traj_dist(traj: Vector) -> Ext {
    fsqrt((ld(traj.z) * ld(traj.z) + ld(traj.y) * ld(traj.y)) + ld(traj.x) * ld(traj.x))
}

/// `RangeToTarget` (0x435720): distance to the target's surface (its rough collision sphere),
/// never negative.
pub fn range_to_target(target_stat: &ShipStaticInfo, traj: Vector) -> Ext {
    range_given_dist(target_stat, traj_dist(traj))
}

/// `RangeToTargetGivenDist` (0x435770) on an already computed distance.
pub fn range_given_dist(target_stat: &ShipStaticInfo, dist: Ext) -> Ext {
    let r = dist - ld(target_stat.header.coll.approx_coll_sphere_size);
    if r <= 0.0 || r.is_nan() {
        0.0
    } else {
        r
    }
}

/// `moveTo - moveFrom`, or the velocity when the ship has no move.
pub fn direction_of_travel(s: &Ship) -> Vector {
    if s.move_to.x == 0.0 && s.move_to.y == 0.0 && s.move_to.z == 0.0 {
        s.posinfo.velocity
    } else {
        Vector::new(st(ld(s.move_to.x) - ld(s.move_from.x)), st(ld(s.move_to.y) - ld(s.move_from.y)), st(ld(s.move_to.z) - ld(s.move_from.z)))
    }
}

/// The right-of-way part of the avoidance walk: a friendly ship in the way of a ship with right
/// of way is told to move aside when it is idle or halted and ahead of us.
fn row_maybe_signal(u: &mut Universe, me: &Ship, other: ShipId) {
    let Some(o) = u.ships.get(other) else { return };
    let their = o.command.and_then(|c| u.commands.get(c));
    if let Some(tc) = their {
        let mine_protecting = me.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.attributes & attr::PROTECTING != 0);
        if tc.attributes & attr::PROTECTING != 0 || mine_protecting {
            return;
        }
        if !matches!(tc.order, order::NULL | order::HALT | order::MILITARY_PARADE) {
            return;
        }
    }
    let dir = direction_of_travel(me);
    let to = Vector::new(
        st(ld(o.coll.coll_position.x) - ld(me.coll.coll_position.x)),
        st(ld(o.coll.coll_position.y) - ld(me.coll.coll_position.y)),
        st(ld(o.coll.coll_position.z) - ld(me.coll.coll_position.z)),
    );
    if !((ld(dir.x) * ld(to.x) + ld(dir.y) * ld(to.y)) + ld(dir.z) * ld(to.z) > 0.0) {
        return;
    }
    let mover = match their {
        Some(tc) if tc.attributes & attr::FORMATION != 0 => {
            let leader = tc.selection[0];
            if u.ships.ship(leader).special_flags & special::ROW_GETTING_OUT_OF_WAY != 0 {
                return;
            }
            leader
        }
        _ => other,
    };
    row_signal(u, mover, me, to, dir);
}

/// `rowSignalShipToGetOutOfWayOfMe` (0x414410): `id` will move aside (sideways when `me` flies
/// mostly up or down, otherwise up or down) and come back afterwards.
pub fn row_signal(u: &mut Universe, id: ShipId, me: &Ship, me_to_ship: Vector, me_dir: Vector) {
    let tw = u.tweaks.combat.clone();
    let mut avoid = tw.row_avoid_by[me.ship_type.index()];
    if avoid == 0.0 {
        let size = u.statics.ship(me.ship_type).map_or(0.0, |st| st.header.coll.avoid_coll_sphere_size);
        avoid = st(ld(tw.avoid_row_ship_by_scale) * ld(size) + ld(tw.avoid_row_ship_by_add));
    }
    let s = u.ships.ship_mut(id);
    s.dont_apply_force_ever |= 2;
    s.row_original_point = s.posinfo.position;
    s.special_flags |= special::ROW_GETTING_OUT_OF_WAY;
    s.row_state = 0;
    s.row_get_out_of_way = Some(me.id);
    let dir = me_dir.normalized();
    let ud = tw.row_going_mostly_updown_angle;
    let delta = if ud < dir.z || dir.z < -ud {
        let a = Vector::new(me_to_ship.x, me_to_ship.y, 0.0).normalized();
        let b = Vector::new(me_dir.x, me_dir.y, 0.0).normalized();
        let mut part = Vector::new(st(ld(b.y) - 0.0), st(0.0 - ld(b.x)), 0.0);
        if 0.0 < ld(a.y) * ld(b.x) - ld(a.x) * ld(b.y) {
            part = Vector::new(-part.x, -part.y, -part.z);
        }
        Vector::new(st(ld(part.x) * ld(avoid)), st(ld(part.y) * ld(avoid)), st(ld(part.z) * ld(avoid)))
    } else if -ld(me_to_ship.z) <= ld(tw.row_go_down_instead_of_up_ratio) * ld(avoid) {
        Vector::new(0.0, 0.0, avoid)
    } else {
        Vector::new(0.0, 0.0, -avoid)
    };
    let p = s.posinfo.position;
    s.row_out_of_way_point = Vector::new(st(ld(delta.x) + ld(p.x)), st(ld(delta.y) + ld(p.y)), st(ld(delta.z) + ld(p.z)));
}

/// `rowOriginalPointIsClear`: the ship with right of way has passed (it is moving away from us)
/// and is far enough from our original point.
fn row_original_point_is_clear(u: &Universe, id: ShipId) -> bool {
    let s = u.ships.ship(id);
    let Some(r) = s.row_get_out_of_way.and_then(|r| u.ships.get(r)) else { return true };
    let dir = direction_of_travel(r);
    let d = [
        ld(s.coll.coll_position.x) - ld(r.coll.coll_position.x),
        ld(s.coll.coll_position.y) - ld(r.coll.coll_position.y),
        ld(s.coll.coll_position.z) - ld(r.coll.coll_position.z),
    ];
    if 0.0 < (ld(dir.x) * d[0] + ld(dir.y) * d[1]) + ld(dir.z) * d[2] {
        return false;
    }
    let size = ld(u.statics.ship(r.ship_type).map_or(0.0, |st| st.header.coll.avoid_coll_sphere_size));
    let o = s.row_original_point;
    let e = [
        ld(r.coll.coll_position.x) - ld(o.x),
        ld(r.coll.coll_position.y) - ld(o.y),
        ld(r.coll.coll_position.z) - ld(o.z),
    ];
    !((e[0] * e[0] + e[1] * e[1]) + e[2] * e[2] < size * size)
}

/// `rowFlyShipOutOfWay` (0x414290): fly to the out-of-the-way point, wait there until the ship
/// with right of way has passed, then return. Meanwhile the ship takes no other forces (bit 1
/// of `dont_apply_force_ever`, cleared only while this runs).
pub fn row_fly_out_of_way(u: &mut Universe, id: ShipId) {
    const REACHED: f32 = 50.0;
    let state = u.ships.ship(id).row_state;
    u.ships.ship_mut(id).dont_apply_force_ever &= !2;
    match state {
        0 | 1 => {
            if u.ships.ship(id).row_get_out_of_way.is_none() || row_original_point_is_clear(u, id) {
                let s = u.ships.ship_mut(id);
                s.row_state = 2;
                s.dont_apply_force_ever |= 2;
                return;
            }
            if state == 0 {
                let p = u.ships.ship(id).row_out_of_way_point;
                if aitrack::move_reached_destination(u.ships.ship(id), p, REACHED) {
                    let s = u.ships.ship_mut(id);
                    s.row_state = 1;
                    s.dont_apply_force_ever |= 2;
                    return;
                }
                fly_to_point_avoiding_objs(u, id, Some(p), 0, 0.0, None);
            } else {
                fly_to_point_avoiding_objs(u, id, None, 0, 0.0, None);
            }
        }
        2 => {
            let o = u.ships.ship(id).row_original_point;
            let done = aitrack::move_reached_destination(u.ships.ship(id), o, REACHED)
                || fly_to_point_avoiding_objs(u, id, Some(o), flags::DONT_FLY_TO_OBSCURED_POINTS | flags::RETURN_IMMED_IF_POINT_OBSCURED, 0.0, None) & FLY_OBJECT_IN_WAY != 0;
            if done {
                row_remove_ship(u.ships.ship_mut(id));
                return;
            }
        }
        _ => {}
    }
    u.ships.ship_mut(id).dont_apply_force_ever |= 2;
}

/// `rowRemoveShipFromGettingOutOfWay`.
pub fn row_remove_ship(s: &mut Ship) {
    if s.special_flags & special::ROW_GETTING_OUT_OF_WAY != 0 {
        s.special_flags &= !special::ROW_GETTING_OUT_OF_WAY;
        s.dont_apply_force_ever &= !2;
        s.row_get_out_of_way = None;
    }
}

/// `getPitchToDescend` (0x413d00): pitch for the normalised height left to descend.
pub fn pitch_to_descend_with(tw: &crate::tunables::SimTweaks, zleft: f32, pd: f32) -> Ext {
    struct T<'a>(&'a crate::tunables::SimTweaks);
    let t = T(tw);
    let (p4, p3, p2) = (ld(t.0.descend_pitch4_dist), ld(t.0.descend_pitch3_dist), ld(t.0.descend_pitch2_dist));
    let (o4, o34, o23) = (
        ld(t.0.one_over_descend_pitch4_dist),
        ld(t.0.one_over_descend_3_minus_4_dist),
        ld(t.0.one_over_descend_2_minus_3_dist),
    );
    let pd = ld(pd);
    if ld(zleft) < 0.0 || zleft.is_nan() {
        let zl = ld(-zleft);
        if zl < 0.0 {
            return 0.0;
        }
        if zl < p4 {
            return o4 * zl * pd;
        }
        if zl < p3 {
            return (p3 - zl) * o34 * pd;
        }
        if zl < p2 {
            return -((zl - p3) * o23 * pd);
        }
        return -pd;
    }
    let zl = ld(zleft);
    if zl < p4 {
        return -(o4 * zl * pd);
    }
    if zl < p3 {
        return -((p3 - zl) * o34 * pd);
    }
    if zl < p2 {
        return (zl - p3) * o23 * pd;
    }
    pd
}

fn neighbour(u: &Universe, id: ShipId) -> Neighbour {
    let s = u.ships.ship(id);
    let stat = u.statics.ship(s.ship_type);
    let c = stat.map(|st| st.header.coll.clone());
    Neighbour {
        id,
        flags: s.flags,
        coll_optimize_dist: s.coll_optimize_dist,
        coll_position: s.coll.coll_position,
        avoid_size: c.as_ref().map_or(0.0, |c| c.avoid_coll_sphere_size),
        avoid_pad: c.as_ref().map_or(0.0, |c| c.avoid_coll_sphere_pad),
        avoid_modifier: c.as_ref().map_or(0.0, |c| c.avoid_coll_modifier),
        right_of_way: stat.map_or(0, |st| st.header.right_of_way),
        command: s.command,
        special_flags: s.special_flags,
        docking_ship: s.docking_ship,
        player: s.player,
        ship: s.clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::blobs::Blob;
    use crate::object::{sof, BlobId};
    use crate::statics::{ShipRace, ShipStaticInfo, ShipType, StaticWorld, Tweaks};
    use crate::tunables::SimTweaks;
    use hwc_math::Matrix;

    struct S {
        pos: [f32; 3],
        vel: [f32; 3],
        coordsys: [f32; 9],
        coll: [f32; 3],
        cod: f32,
        thrust: [f32; 6],
        rot: [f32; 6],
        turn: [f32; 3],
    }

    struct Row {
        a: S,
        b: S,
        b_avoid_modifier: f32,
        dest: [f32; 3],
        flags: u32,
        limit: f32,
        force: [u32; 3],
        torque: [u32; 3],
        ret: u32,
    }

    /// Emulation of `aishipFlyToPointAvoidingObjsFunc` (0x414620) in cataclysm.exe: a small ship
    /// in fly state 2 with one big neighbour (in the blob's big-target list), compiled-in
    /// AIShip tweaks, dt = 1/16; the force, torque and return value it leaves.
    const ROWS: &[Row] = &[
        Row { a: S { pos: [f32::from_bits(0xc3925108), f32::from_bits(0xc3688f89), f32::from_bits(0xc280985d)], vel: [f32::from_bits(0xc2568ee1), f32::from_bits(0x424e12f1), f32::from_bits(0xc28d0a1d)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf7f85d0), f32::from_bits(0x3d7a0146), f32::from_bits(0x00000000), f32::from_bits(0xbd7a0146), f32::from_bits(0xbf7f85d0), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc3918175), f32::from_bits(0xc36c31e5), f32::from_bits(0xc27fc38d)], cod: f32::from_bits(0x433319b2), thrust: [f32::from_bits(0x4586647c), f32::from_bits(0x460c2b00), f32::from_bits(0x44da1c84), f32::from_bits(0x44915dd0), f32::from_bits(0x460519a9), f32::from_bits(0x4583fc88)], rot: [f32::from_bits(0x43816628), f32::from_bits(0x43b61faf), f32::from_bits(0x43c3f538), f32::from_bits(0x445805c7), f32::from_bits(0x43862207), f32::from_bits(0x4388c53a)], turn: [f32::from_bits(0x3ffb2b16), f32::from_bits(0x3fef297d), f32::from_bits(0x40389951)] },
              b: S { pos: [f32::from_bits(0xc3788171), f32::from_bits(0xc39fa32f), f32::from_bits(0xc31d4fa6)], vel: [f32::from_bits(0x42877920), f32::from_bits(0xc1e73184), f32::from_bits(0xc26ce9ec)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf5b8917), f32::from_bits(0xbf03ae9a), f32::from_bits(0x00000000), f32::from_bits(0x3f03ae9a), f32::from_bits(0xbf5b8917), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc3739da8), f32::from_bits(0xc3a1fc4f), f32::from_bits(0xc31af5ee)], cod: f32::from_bits(0x43adabcf), thrust: [f32::from_bits(0x45c209a5), f32::from_bits(0x452badf5), f32::from_bits(0x45e66609), f32::from_bits(0x45d5b45e), f32::from_bits(0x451f67ec), f32::from_bits(0x45bf78aa)], rot: [f32::from_bits(0x4389ac5d), f32::from_bits(0x43052179), f32::from_bits(0x43b8676e), f32::from_bits(0x43b7bcb3), f32::from_bits(0x43d07767), f32::from_bits(0x441a5a4c)], turn: [f32::from_bits(0x4025c9a8), f32::from_bits(0x4039cfcf), f32::from_bits(0x403c3a9b)] },
              b_avoid_modifier: f32::from_bits(0x00000000), dest: [f32::from_bits(0x4526c674), f32::from_bits(0x44bc50cb), f32::from_bits(0xc4994d67)], flags: 0x3b, limit: f32::from_bits(0xc1a00000), force: [0x44b151bf, 0x458185b0, 0x4586647c], torque: [0x431cfaee, 0x425f6959, 0xc388c53a], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0xc342389c), f32::from_bits(0x4294808e), f32::from_bits(0x41dbee21)], vel: [f32::from_bits(0x42a86827), f32::from_bits(0x41a23928), f32::from_bits(0x421c4b42)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f10afa9), f32::from_bits(0xbf533110), f32::from_bits(0x00000000), f32::from_bits(0x3f533110), f32::from_bits(0x3f10afa9), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc3433aba), f32::from_bits(0x429cb43b), f32::from_bits(0x41e7e068)], cod: f32::from_bits(0x43c654ad), thrust: [f32::from_bits(0x4520f256), f32::from_bits(0x4590f334), f32::from_bits(0x45e762e0), f32::from_bits(0x458e6b4e), f32::from_bits(0x45e47a86), f32::from_bits(0x45abab7d)], rot: [f32::from_bits(0x43426e0a), f32::from_bits(0x433317bf), f32::from_bits(0x445d9962), f32::from_bits(0x43437495), f32::from_bits(0x437f492a), f32::from_bits(0x43b815d6)], turn: [f32::from_bits(0x40122750), f32::from_bits(0x4028a77c), f32::from_bits(0x3fd8383d)] },
              b: S { pos: [f32::from_bits(0xc3185668), f32::from_bits(0xc1ba6cf6), f32::from_bits(0xc238f49c)], vel: [f32::from_bits(0xbebdaedd), f32::from_bits(0x4266bff2), f32::from_bits(0xc2319948)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f3829ad), f32::from_bits(0x3f31d200), f32::from_bits(0x80000000), f32::from_bits(0xbf31d200), f32::from_bits(0x3f3829ad), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc31735d7), f32::from_bits(0xc1af8714), f32::from_bits(0xc24b6eb6)], cod: f32::from_bits(0x42333b5a), thrust: [f32::from_bits(0x4503772b), f32::from_bits(0x45d48836), f32::from_bits(0x45cc7ae8), f32::from_bits(0x460944f1), f32::from_bits(0x45df22a3), f32::from_bits(0x453de085)], rot: [f32::from_bits(0x44321991), f32::from_bits(0x445e01ab), f32::from_bits(0x43bd7a53), f32::from_bits(0x42d9e601), f32::from_bits(0x43c8137b), f32::from_bits(0x43a3020b)], turn: [f32::from_bits(0x3f70b668), f32::from_bits(0x3f1cdbd1), f32::from_bits(0x3f4fe7e0)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0x44f99d7b), f32::from_bits(0xc5241d20), f32::from_bits(0x4431cc94)], flags: 0x4, limit: f32::from_bits(0x00000000), force: [0xc3ad8dc0, 0xc60ff48c, 0x4520f256], torque: [0x00000000, 0x00000000, 0x00000000], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0xc3301889), f32::from_bits(0xc33802c3), f32::from_bits(0x43406fa0)], vel: [f32::from_bits(0x421bf298), f32::from_bits(0x409c0d15), f32::from_bits(0x428fe38a)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f14e9d9), f32::from_bits(0x3f503b86), f32::from_bits(0x80000000), f32::from_bits(0xbf503b86), f32::from_bits(0x3f14e9d9), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc32fdafe), f32::from_bits(0xc335d6fe), f32::from_bits(0x43400d2b)], cod: f32::from_bits(0x438f66d4), thrust: [f32::from_bits(0x4593f968), f32::from_bits(0x45015171), f32::from_bits(0x44f87aae), f32::from_bits(0x45e30789), f32::from_bits(0x45d97632), f32::from_bits(0x46057820)], rot: [f32::from_bits(0x4423068d), f32::from_bits(0x440cf2e4), f32::from_bits(0x4368f00d), f32::from_bits(0x4443a04e), f32::from_bits(0x43430c6b), f32::from_bits(0x43dbf30d)], turn: [f32::from_bits(0x3f5a00dc), f32::from_bits(0x3fb9d30a), f32::from_bits(0x3fa24d6d)] },
              b: S { pos: [f32::from_bits(0xc36066c7), f32::from_bits(0xc381f248), f32::from_bits(0x433599b1)], vel: [f32::from_bits(0xc16e5964), f32::from_bits(0x4228ded5), f32::from_bits(0x4104f945)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f7fb9a6), f32::from_bits(0xbd3dbce7), f32::from_bits(0x00000000), f32::from_bits(0x3d3dbce7), f32::from_bits(0x3f7fb9a6), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc35d9cfa), f32::from_bits(0xc380c557), f32::from_bits(0x4338df61)], cod: f32::from_bits(0x41530a46), thrust: [f32::from_bits(0x45b8ed91), f32::from_bits(0x45ff1911), f32::from_bits(0x453feacb), f32::from_bits(0x45239432), f32::from_bits(0x45eab7d5), f32::from_bits(0x45747118)], rot: [f32::from_bits(0x43d2835c), f32::from_bits(0x443c8706), f32::from_bits(0x43cba96a), f32::from_bits(0x438160ad), f32::from_bits(0x4387c0fb), f32::from_bits(0x43bc3ea2)], turn: [f32::from_bits(0x400fa8b5), f32::from_bits(0x3fb5d77c), f32::from_bits(0x401b5e84)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0xc52dc532), f32::from_bits(0xc4d33a0b), f32::from_bits(0x446b7f1f)], flags: 0x30, limit: f32::from_bits(0xc1a00000), force: [0xc58cc01f, 0x45b10657, 0x4593f968], torque: [0x00000000, 0x00000000, 0x00000000], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0xc1448d38), f32::from_bits(0xbdb918e1), f32::from_bits(0x4382a08e)], vel: [f32::from_bits(0xc06050b4), f32::from_bits(0xc22248a5), f32::from_bits(0xc0a7ceb5)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3d0f7226), f32::from_bits(0xbf7fd7cc), f32::from_bits(0x00000000), f32::from_bits(0x3f7fd7cc), f32::from_bits(0x3d0f7226), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc18a428b), f32::from_bits(0xbfa59443), f32::from_bits(0x4383ea4d)], cod: f32::from_bits(0x42f05d1e), thrust: [f32::from_bits(0x4601b23c), f32::from_bits(0x45b01959), f32::from_bits(0x45841cb6), f32::from_bits(0x45e18964), f32::from_bits(0x4533192c), f32::from_bits(0x45b68c7d)], rot: [f32::from_bits(0x44424d73), f32::from_bits(0x44052f7a), f32::from_bits(0x43b1e48a), f32::from_bits(0x4330cc82), f32::from_bits(0x440bd2dd), f32::from_bits(0x437a1162)], turn: [f32::from_bits(0x3fb76cb8), f32::from_bits(0x3fed94dc), f32::from_bits(0x40369391)] },
              b: S { pos: [f32::from_bits(0xc2837051), f32::from_bits(0xc29906f8), f32::from_bits(0x43155a6e)], vel: [f32::from_bits(0x420ab2c0), f32::from_bits(0x40775a31), f32::from_bits(0x428f5727)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f6eede8), f32::from_bits(0x3eb7d5a1), f32::from_bits(0x80000000), f32::from_bits(0xbeb7d5a1), f32::from_bits(0x3f6eede8), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc285af96), f32::from_bits(0xc2957bb0), f32::from_bits(0x4315e549)], cod: f32::from_bits(0x42b14ccf), thrust: [f32::from_bits(0x4585ecef), f32::from_bits(0x45ed1eee), f32::from_bits(0x4537a77b), f32::from_bits(0x45cfb518), f32::from_bits(0x44ad220a), f32::from_bits(0x4606f43a)], rot: [f32::from_bits(0x4421d071), f32::from_bits(0x43b37c09), f32::from_bits(0x445da407), f32::from_bits(0x44384d95), f32::from_bits(0x4315584e), f32::from_bits(0x43955083)], turn: [f32::from_bits(0x3fdd5761), f32::from_bits(0x403fb6e8), f32::from_bits(0x3f5a0b1f)] },
              b_avoid_modifier: f32::from_bits(0x00000000), dest: [f32::from_bits(0xc52a650e), f32::from_bits(0xc48c8a8e), f32::from_bits(0xc51a5e6a)], flags: 0x3b, limit: f32::from_bits(0x00000000), force: [0xc5be55d7, 0x45db015b, 0xc5b01959], torque: [0xc2e547e6, 0xc2b094ee, 0x439e2826], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0xc25a97ff), f32::from_bits(0x41b8c9bc), f32::from_bits(0xc20ef5a2)], vel: [f32::from_bits(0xc13fea26), f32::from_bits(0xc07f60e3), f32::from_bits(0xc08c78b3)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf6b236e), f32::from_bits(0x3eca71c4), f32::from_bits(0x00000000), f32::from_bits(0xbeca71c4), f32::from_bits(0xbf6b236e), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc24a904f), f32::from_bits(0x41d2f4cb), f32::from_bits(0xc205df4d)], cod: f32::from_bits(0x43078c17), thrust: [f32::from_bits(0x4512e0fb), f32::from_bits(0x44d6c2d0), f32::from_bits(0x45f602c9), f32::from_bits(0x44b5e38c), f32::from_bits(0x45ad9039), f32::from_bits(0x460157d8)], rot: [f32::from_bits(0x43b2e68e), f32::from_bits(0x43cda86b), f32::from_bits(0x43d93234), f32::from_bits(0x4455e424), f32::from_bits(0x4421d545), f32::from_bits(0x4440c512)], turn: [f32::from_bits(0x3f18dbb1), f32::from_bits(0x4014dc33), f32::from_bits(0x3fd04f47)] },
              b: S { pos: [f32::from_bits(0xc30cb7f8), f32::from_bits(0xc2b6af75), f32::from_bits(0x4213c91a)], vel: [f32::from_bits(0xc19da346), f32::from_bits(0x427d4705), f32::from_bits(0x423c41c1)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf0929c6), f32::from_bits(0xbf582741), f32::from_bits(0x00000000), f32::from_bits(0x3f582741), f32::from_bits(0xbf0929c6), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc30a5555), f32::from_bits(0xc2ae7c39), f32::from_bits(0x4210e91f)], cod: f32::from_bits(0x435e70f6), thrust: [f32::from_bits(0x45e370aa), f32::from_bits(0x4590a56f), f32::from_bits(0x4503241f), f32::from_bits(0x45b19d0c), f32::from_bits(0x45f90d7f), f32::from_bits(0x45f8e35a)], rot: [f32::from_bits(0x43482170), f32::from_bits(0x43ca6ed3), f32::from_bits(0x438d6ee6), f32::from_bits(0x43ba6706), f32::from_bits(0x4458e617), f32::from_bits(0x43da6f11)], turn: [f32::from_bits(0x3fa439ea), f32::from_bits(0x3f9737eb), f32::from_bits(0x3fcceae7)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0xc2f16ff0), f32::from_bits(0xc28eaf75), f32::from_bits(0x4263c91a)], flags: 0x3b, limit: f32::from_bits(0xc1a00000), force: [0x00000000, 0x00000000, 0x00000000], torque: [0x00000000, 0x00000000, 0x00000000], ret: 1 },
        Row { a: S { pos: [f32::from_bits(0xc3245652), f32::from_bits(0xc3100a2b), f32::from_bits(0x4373bc4e)], vel: [f32::from_bits(0x4264bb4b), f32::from_bits(0xc23b368b), f32::from_bits(0x425fab16)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3ed47f6b), f32::from_bits(0x3f68e905), f32::from_bits(0x80000000), f32::from_bits(0xbf68e905), f32::from_bits(0x3ed47f6b), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc3288e17), f32::from_bits(0xc3141840), f32::from_bits(0x4376bc01)], cod: f32::from_bits(0x4358c940), thrust: [f32::from_bits(0x4598bf91), f32::from_bits(0x456e99cb), f32::from_bits(0x45cc16fe), f32::from_bits(0x4597d5bf), f32::from_bits(0x44bb03cf), f32::from_bits(0x45b57df2)], rot: [f32::from_bits(0x43af777e), f32::from_bits(0x444254fb), f32::from_bits(0x4406cd6b), f32::from_bits(0x4433c2a1), f32::from_bits(0x43442510), f32::from_bits(0x44266a1e)], turn: [f32::from_bits(0x4001ad6d), f32::from_bits(0x40282573), f32::from_bits(0x402d76dd)] },
              b: S { pos: [f32::from_bits(0xc373ac40), f32::from_bits(0xc2a626c3), f32::from_bits(0x4315b357)], vel: [f32::from_bits(0x4124a222), f32::from_bits(0x41935913), f32::from_bits(0x4255a4b6)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f7f0f26), f32::from_bits(0x3daf6b78), f32::from_bits(0x80000000), f32::from_bits(0xbdaf6b78), f32::from_bits(0x3f7f0f26), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc3733940), f32::from_bits(0xc2a06a1d), f32::from_bits(0x4316a527)], cod: f32::from_bits(0x4358fea9), thrust: [f32::from_bits(0x45f533c2), f32::from_bits(0x460989ae), f32::from_bits(0x453ffeee), f32::from_bits(0x44c7d200), f32::from_bits(0x45b89b55), f32::from_bits(0x4508bd51)], rot: [f32::from_bits(0x445aed62), f32::from_bits(0x43efd635), f32::from_bits(0x4403d2f4), f32::from_bits(0x443c7906), f32::from_bits(0x4446f537), f32::from_bits(0x4355dadf)], turn: [f32::from_bits(0x3fd1142f), f32::from_bits(0x3f7378d5), f32::from_bits(0x3fd358c7)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0xc513cc3b), f32::from_bits(0x450460d4), f32::from_bits(0xc41b7db9)], flags: 0x74, limit: f32::from_bits(0x00000000), force: [0x44a8ab90, 0x45cd1622, 0x4598bf91], torque: [0x00000000, 0x00000000, 0x00000000], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0xc284e8db), f32::from_bits(0x436e80ff), f32::from_bits(0x43420679)], vel: [f32::from_bits(0xc279bb18), f32::from_bits(0xc2918673), f32::from_bits(0x428efb77)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f59dd07), f32::from_bits(0xbf066e31), f32::from_bits(0x00000000), f32::from_bits(0x3f066e31), f32::from_bits(0x3f59dd07), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc280c0e4), f32::from_bits(0x4370e23f), f32::from_bits(0x4342f0e8)], cod: f32::from_bits(0x430a5520), thrust: [f32::from_bits(0x45f5ac8d), f32::from_bits(0x45e3e198), f32::from_bits(0x44c41568), f32::from_bits(0x45ff921a), f32::from_bits(0x45be68f5), f32::from_bits(0x454f2bdd)], rot: [f32::from_bits(0x436adeb9), f32::from_bits(0x4332713b), f32::from_bits(0x443b4637), f32::from_bits(0x444293be), f32::from_bits(0x43bd1b01), f32::from_bits(0x4373fbf7)], turn: [f32::from_bits(0x3f2ee4c6), f32::from_bits(0x3fc47b74), f32::from_bits(0x3f9f4f12)] },
              b: S { pos: [f32::from_bits(0xc2d39f1b), f32::from_bits(0x436e801a), f32::from_bits(0x4369319b)], vel: [f32::from_bits(0xc2b338f3), f32::from_bits(0xc221d70a), f32::from_bits(0x4280789d)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3ecb956f), f32::from_bits(0xbf6ae471), f32::from_bits(0x00000000), f32::from_bits(0x3f6ae471), f32::from_bits(0x3ecb956f), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc2d58ac4), f32::from_bits(0x436ea492), f32::from_bits(0x4365cfe2)], cod: f32::from_bits(0x4169426f), thrust: [f32::from_bits(0x44c73c6e), f32::from_bits(0x450668e0), f32::from_bits(0x46096e0d), f32::from_bits(0x45f6b47b), f32::from_bits(0x45cf89d8), f32::from_bits(0x460011a6)], rot: [f32::from_bits(0x43b42be5), f32::from_bits(0x4395c4aa), f32::from_bits(0x4446f9cb), f32::from_bits(0x4412422a), f32::from_bits(0x4451ad7d), f32::from_bits(0x43c19c42)], turn: [f32::from_bits(0x3fc42d4a), f32::from_bits(0x4012db34), f32::from_bits(0x402d2fb8)] },
              b_avoid_modifier: f32::from_bits(0x00000000), dest: [f32::from_bits(0xc5018a3f), f32::from_bits(0xc493d711), f32::from_bits(0xc52a3d7d)], flags: 0x6, limit: f32::from_bits(0x00000000), force: [0x458db4c2, 0x45884d76, 0xc5e3e198], torque: [0xc25a9df8, 0xc30d5758, 0x430a0056], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0x420f0ec1), f32::from_bits(0x437e2e0f), f32::from_bits(0x4222d806)], vel: [f32::from_bits(0xc29de9f5), f32::from_bits(0x42a5b1e9), f32::from_bits(0x41ffeada)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3d7728a4), f32::from_bits(0x3f7f8894), f32::from_bits(0x80000000), f32::from_bits(0xbf7f8894), f32::from_bits(0x3d7728a4), f32::from_bits(0x00000000)], coll: [f32::from_bits(0x4208d20c), f32::from_bits(0x437ac8ea), f32::from_bits(0x42218dea)], cod: f32::from_bits(0x431248ec), thrust: [f32::from_bits(0x4532fa03), f32::from_bits(0x45c0d93c), f32::from_bits(0x45c4b356), f32::from_bits(0x4606eb78), f32::from_bits(0x4608af5b), f32::from_bits(0x4593bc4e)], rot: [f32::from_bits(0x44283d26), f32::from_bits(0x43c80713), f32::from_bits(0x43df61dd), f32::from_bits(0x440a6642), f32::from_bits(0x445d41aa), f32::from_bits(0x4305917c)], turn: [f32::from_bits(0x3fd427e4), f32::from_bits(0x3faba748), f32::from_bits(0x4020c691)] },
              b: S { pos: [f32::from_bits(0xc2424335), f32::from_bits(0x439608a7), f32::from_bits(0x42995ba7)], vel: [f32::from_bits(0x42b43608), f32::from_bits(0xc0b2d1ed), f32::from_bits(0xc2a817bc)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf0eae89), f32::from_bits(0x3f548d0c), f32::from_bits(0x00000000), f32::from_bits(0xbf548d0c), f32::from_bits(0xbf0eae89), f32::from_bits(0x00000000)], coll: [f32::from_bits(0xc23ec2ed), f32::from_bits(0x4396f018), f32::from_bits(0x429ee3d6)], cod: f32::from_bits(0x43395128), thrust: [f32::from_bits(0x45a89351), f32::from_bits(0x45df1715), f32::from_bits(0x4498679c), f32::from_bits(0x453685a0), f32::from_bits(0x45cb28e6), f32::from_bits(0x4609d458)], rot: [f32::from_bits(0x4455a7d5), f32::from_bits(0x43a2b698), f32::from_bits(0x445b6fb7), f32::from_bits(0x43a19af1), f32::from_bits(0x43e71b97), f32::from_bits(0x43630a59)], turn: [f32::from_bits(0x40006d1f), f32::from_bits(0x3fbd5a4e), f32::from_bits(0x400d9cf7)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0x452da8b0), f32::from_bits(0xc4f76337), f32::from_bits(0x44764439)], flags: 0x3b, limit: f32::from_bits(0x43160000), force: [0x45832f10, 0xc60b2199, 0xc5c0d93c], torque: [0x430498ee, 0xc1f51685, 0xc305917c], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0x4273e312), f32::from_bits(0xc30778b8), f32::from_bits(0xc38b6fa6)], vel: [f32::from_bits(0xc29f0941), f32::from_bits(0x429ff6c1), f32::from_bits(0xc1a18b09)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3e5f96e6), f32::from_bits(0x3f79d2aa), f32::from_bits(0x80000000), f32::from_bits(0xbf79d2aa), f32::from_bits(0x3e5f96e6), f32::from_bits(0x00000000)], coll: [f32::from_bits(0x427623bc), f32::from_bits(0xc30802e9), f32::from_bits(0xc3892b17)], cod: f32::from_bits(0x433af4ea), thrust: [f32::from_bits(0x45c8a2c6), f32::from_bits(0x45e075a8), f32::from_bits(0x46043c39), f32::from_bits(0x4513ae3d), f32::from_bits(0x45685349), f32::from_bits(0x46065e9d)], rot: [f32::from_bits(0x441238e7), f32::from_bits(0x43c709a7), f32::from_bits(0x44494711), f32::from_bits(0x4342fb74), f32::from_bits(0x43974f88), f32::from_bits(0x431b5055)], turn: [f32::from_bits(0x400b4950), f32::from_bits(0x3fdbc1ba), f32::from_bits(0x3fa16b12)] },
              b: S { pos: [f32::from_bits(0x4326556c), f32::from_bits(0xc2b145a9), f32::from_bits(0xc378d2b8)], vel: [f32::from_bits(0xc1418f74), f32::from_bits(0xc2455ee2), f32::from_bits(0x42275eff)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf7f901b), f32::from_bits(0xbd6f40c7), f32::from_bits(0x00000000), f32::from_bits(0x3d6f40c7), f32::from_bits(0xbf7f901b), f32::from_bits(0x00000000)], coll: [f32::from_bits(0x4325263f), f32::from_bits(0xc2a9e5f7), f32::from_bits(0xc37d602c)], cod: f32::from_bits(0x4362457f), thrust: [f32::from_bits(0x450b3b0d), f32::from_bits(0x44f266f9), f32::from_bits(0x45aad1d2), f32::from_bits(0x44d3fc16), f32::from_bits(0x44c73b9f), f32::from_bits(0x45018ca1)], rot: [f32::from_bits(0x43c89962), f32::from_bits(0x443f0f73), f32::from_bits(0x43a59c37), f32::from_bits(0x4345d439), f32::from_bits(0x443286d8), f32::from_bits(0x443c6d3f)], turn: [f32::from_bits(0x403f6189), f32::from_bits(0x3f1f72c1), f32::from_bits(0x3f69a0b2)] },
              b_avoid_modifier: f32::from_bits(0x3f800000), dest: [f32::from_bits(0xc3a8d1d2), f32::from_bits(0x452fca47), f32::from_bits(0xc220cee7)], flags: 0x13, limit: f32::from_bits(0xc1a00000), force: [0xc5817b93, 0xc4bac741, 0xc5e075a8], torque: [0x4127a966, 0xc1af65f7, 0xc106a6c7], ret: 0 },
        Row { a: S { pos: [f32::from_bits(0x436623d9), f32::from_bits(0xc32577c4), f32::from_bits(0xc18ccac4)], vel: [f32::from_bits(0x4281fdea), f32::from_bits(0x427c83c7), f32::from_bits(0x421d5256)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0xbf6189fb), f32::from_bits(0xbef23972), f32::from_bits(0x00000000), f32::from_bits(0x3ef23972), f32::from_bits(0xbf6189fb), f32::from_bits(0x00000000)], coll: [f32::from_bits(0x436b1999), f32::from_bits(0xc3246ae0), f32::from_bits(0xc1a0dbd4)], cod: f32::from_bits(0x42b1971f), thrust: [f32::from_bits(0x44b58c0b), f32::from_bits(0x460279f5), f32::from_bits(0x45ae17e0), f32::from_bits(0x45b5f379), f32::from_bits(0x45f96f7b), f32::from_bits(0x45cbf418)], rot: [f32::from_bits(0x43ceebc2), f32::from_bits(0x440e2211), f32::from_bits(0x4325bb96), f32::from_bits(0x439754fb), f32::from_bits(0x43bb6752), f32::from_bits(0x4437edd1)], turn: [f32::from_bits(0x4009d437), f32::from_bits(0x40009ded), f32::from_bits(0x3fbe6bfa)] },
              b: S { pos: [f32::from_bits(0x43897d8b), f32::from_bits(0xc3671578), f32::from_bits(0xc143bafd)], vel: [f32::from_bits(0xc1ed37a1), f32::from_bits(0xc1b8988f), f32::from_bits(0xc2b3f927)], coordsys: [f32::from_bits(0x00000000), f32::from_bits(0x00000000), f32::from_bits(0x3f800000), f32::from_bits(0x3f44d27b), f32::from_bits(0x3f23b2ae), f32::from_bits(0x80000000), f32::from_bits(0xbf23b2ae), f32::from_bits(0x3f44d27b), f32::from_bits(0x00000000)], coll: [f32::from_bits(0x438b897b), f32::from_bits(0xc363eeb9), f32::from_bits(0xc16bdde5)], cod: f32::from_bits(0x43477ae6), thrust: [f32::from_bits(0x45d9688e), f32::from_bits(0x452293f9), f32::from_bits(0x45709993), f32::from_bits(0x45b1ddfd), f32::from_bits(0x451c6598), f32::from_bits(0x46014f46)], rot: [f32::from_bits(0x42e656d1), f32::from_bits(0x444932ca), f32::from_bits(0x441ed55a), f32::from_bits(0x43d0f835), f32::from_bits(0x43a55801), f32::from_bits(0x4446a9ff)], turn: [f32::from_bits(0x4011d963), f32::from_bits(0x3f835c42), f32::from_bits(0x3fa59122)] },
              b_avoid_modifier: f32::from_bits(0x00000000), dest: [f32::from_bits(0xc50a322f), f32::from_bits(0x4417a9ca), f32::from_bits(0xc3edba7e)], flags: 0x3b, limit: f32::from_bits(0x00000000), force: [0xc5f9de12, 0x4542a54b, 0xc60279f5], torque: [0xc32c4945, 0xc1abb064, 0x4324f3ef], ret: 0 },
    ];

    fn v(a: [f32; 3]) -> Vector {
        Vector::new(a[0], a[1], a[2])
    }

    fn statics(b_avoid_modifier: f32) -> StaticWorld {
        let mut ships: Vec<Option<ShipStaticInfo>> = vec![None; ShipType::COUNT];
        let mut a = ShipStaticInfo::new(ShipType::sAcolyte, String::new());
        a.ship_race = ShipRace::Somtaaw;
        let h = &mut a.header;
        h.mass = 30.0;
        h.one_over_mass = st(1.0 / 30.0);
        h.moment_of_inertia = Vector::new(5.0, 6.0, 7.0);
        h.max_velocity = 500.0;
        h.coll.avoid_coll_sphere_size = 25.0;
        h.coll.avoid_coll_sphere_pad = 64.0;
        h.coll.avoid_coll_modifier = 1.0;
        h.coll.original_coll_sphere_size = 22.0;
        h.coll.up_length = 10.0;
        h.coll.right_length = 30.0;
        h.coll.forward_length = 40.0;
        h.right_of_way = 0;
        a.health.max_health = 100.0;
        a.sinbank = f32::from_bits(0x3f7d_70a4);
        a.pitchdescend = 0.0;
        a.is_capital_ship = false;
        let mut b = ShipStaticInfo::new(ShipType::sHiveFrigate, String::new());
        b.ship_race = ShipRace::Somtaaw;
        let h = &mut b.header;
        h.mass = 800.0;
        h.max_velocity = 200.0;
        h.coll.avoid_coll_sphere_size = 90.0;
        h.coll.avoid_coll_sphere_pad = 230.0;
        h.coll.avoid_coll_modifier = b_avoid_modifier;
        h.coll.original_coll_sphere_size = 600.0;
        h.coll.up_length = 60.0;
        h.coll.right_length = 80.0;
        h.coll.forward_length = 180.0;
        b.health.max_health = 100.0;
        ships[ShipType::sAcolyte.index()] = Some(a);
        ships[ShipType::sHiveFrigate.index()] = Some(b);
        StaticWorld {
            options: Default::default(),
            tweaks: Tweaks::default(),
            ships,
            missiles: Vec::new(),
            mines: Vec::new(),
            mine_max_diameter: 0.0,
            asteroids: Vec::new(),
            dust_clouds: Vec::new(),
            nebulae: Vec::new(),
            derelicts: Vec::new(),
            research: Default::default(),
            unit_caps: Default::default(),
            unit_cap_presets: BTreeMap::new(),
            team_colors: Default::default(),
            default_lod: None,
            default_mex: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn matches_emulated_binary() {
        for (i, r) in ROWS.iter().enumerate() {
            let mut u = Universe::new(Arc::new(statics(r.b_avoid_modifier)), SimTweaks::compiled(), &[ShipRace::Somtaaw]);
            let ida = u.add_ship(ShipType::sAcolyte, 0, Vector::ZERO, None).unwrap();
            let idb = u.add_ship(ShipType::sHiveFrigate, 0, Vector::ZERO, None).unwrap();
            for (id, s) in [(ida, &r.a), (idb, &r.b)] {
                let sh = u.ships.ship_mut(id);
                sh.flags = sof::ROTATABLE | sof::IMPACTABLE | sof::TARGETABLE;
                sh.posinfo.position = v(s.pos);
                sh.posinfo.velocity = v(s.vel);
                sh.rotinfo.coordsys = Matrix { m: s.coordsys };
                sh.coll.coll_position = v(s.coll);
                sh.coll_optimize_dist = s.cod;
                sh.coll_my_blob = Some(BlobId(0));
                sh.nonstat.thrust_strength = s.thrust;
                sh.nonstat.rot_strength = s.rot;
                sh.nonstat.turn_speed = s.turn;
                sh.health = 100.0;
                sh.ai_state = 2;
            }
            let mut blob = u.blobs.blobs.first().cloned().unwrap_or_else(|| Blob {
                centre: Vector::ZERO,
                radius: 0.0,
                volume: 0.0,
                one_over_volume: 0.0,
                total_mass: 0.0,
                sort_distance: 0.0,
                sqrt_sort_distance: 0.0,
                flags: 0,
                objects: Vec::new(),
                small_targets: Vec::new(),
                big_targets: Vec::new(),
                max_small_target_size: 0.0,
                max_big_target_size: 0.0,
                ships: Vec::new(),
                small_ships: Vec::new(),
                big_ships: Vec::new(),
                max_ship_size: 0.0,
                max_small_ship_size: 0.0,
                max_big_ship_size: 0.0,
                bullets: Vec::new(),
                missiles: Vec::new(),
                mines: Vec::new(),
            });
            blob.big_targets = vec![idb];
            blob.small_targets = Vec::new();
            blob.max_big_target_size = 600.0;
            blob.max_small_target_size = 0.0;
            u.blobs.blobs = vec![blob];
            let ret = fly_to_point_avoiding_objs(&mut u, ida, Some(v(r.dest)), r.flags, r.limit, None);
            let s = u.ships.ship(ida);
            let f = s.posinfo.force;
            let t = s.rotinfo.torque;
            assert_eq!(ret, r.ret, "row {i} result");
            assert_eq!([f.x.to_bits(), f.y.to_bits(), f.z.to_bits()], r.force, "row {i} force");
            assert_eq!([t.x.to_bits(), t.y.to_bits(), t.z.to_bits()], r.torque, "row {i} torque");
        }
    }
}
