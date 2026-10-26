//! `FlightMan.c`: the scripted flight manoeuvres fighters fly while attacking (turnarounds,
//! barrel rolls, whip strafes, evasive rolls and yo-yos, slaloms, sandwiches).
//!
//! A manoeuvre is started with [`init`] (which closes any running one) and advanced once per
//! frame with [`execute`] until it reports completion. Each manoeuvre keeps its progress in
//! [`FlightmanInfo`] (the original's `flightmanInfo` allocation) and the ship's
//! `flightmanState1/2` bytes. Constants come from `FlightMan.script`
//! ([`crate::combat_tweaks::FlightManTweaks`]); the accumulation of rotated angles follows the
//! binary (`total = dt * rotspeed + total` stored to a float, the comparison made on the
//! register value).

use hwc_math::fpu::{ld, lt_or_unordered, st};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::aiship::{self, flags as fly};
use crate::aitrack;
use crate::object::{trans, ShipId};
use crate::physics::{apply_force, apply_rot};
use crate::universe::Universe;

/// Flight manoeuvres (`FLIGHTMAN_*`).
pub mod kind {
    pub const NULL: u32 = 0;
    pub const DONOTHING: u32 = 1;
    pub const FLIPTURN: u32 = 2;
    pub const HARDBANK: u32 = 3;
    pub const IMMELMAN: u32 = 4;
    pub const SPLIT_S: u32 = 5;
    pub const CORKSCREW: u32 = 6;
    pub const ENDOVER: u32 = 7;
    pub const NO_AIP: u32 = 8;
    pub const BARREL_ROLL: u32 = 9;
    pub const WHIP_STRAFE: u32 = 10;
    pub const CELEB_FLIP: u32 = 11;
    pub const BARRELROLL_OUT: u32 = 12;
    pub const SWARMER_BARRELROLL: u32 = 13;
    pub const ROLL180: u32 = 14;
    pub const ROLLAWAY: u32 = 15;
    pub const SOFTBANK: u32 = 16;
    pub const SPLITS_EVASIVE: u32 = 17;
    pub const SLALOM: u32 = 18;
    pub const SIDESTEP: u32 = 19;
    pub const LOYOYO: u32 = 20;
    pub const HIYOYO: u32 = 21;
    pub const SPEEDBRAKE: u32 = 22;
    pub const SANDWICH: u32 = 23;
}

/// Manoeuvre families (`FLIGHTMAN_EVASIVE_FRONT` .. `FLIGHTMAN_AIP`).
pub mod family {
    pub const EVASIVE_FRONT: u32 = 1;
    pub const EVASIVE_BEHIND: u32 = 2;
    pub const EVASIVE_PURE: u32 = 3;
    pub const TURNAROUND: u32 = 4;
    pub const AIP: u32 = 5;
}

/// `TACTALK_*` bits of `tacticsTalk`.
pub mod tactalk {
    pub const FLIPTURN: u32 = 1;
    pub const BARRELROLL: u32 = 4;
}

/// Most slalom waypoints the info block holds.
const SLALOM_MAX_WAYPOINTS: usize = 15;

/// Progress of the running manoeuvre.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FlightmanInfo {
    FlipTurn { pitch: f32, roll: f32, finished: f32, roll_left: bool },
    Corkscrew { yaw: f32, roll: f32, increment: f32, roll_left: bool },
    Immelman { pitch: f32, roll: f32, inverted: bool, roll_left: bool },
    SplitS { pitch: f32, roll: f32, roll_left: bool, inverted: bool },
    /// Hard bank, soft bank and the evasive split-S share this layout (and their init).
    Bank { pitch: f32, roll: f32, roll_left: bool },
    Slalom { waypoints: Vec<Vector>, timestamp: f32 },
    Sandwich { waypoint: Vector, timestamp: f32 },
    /// Barrel roll, swarmer barrel roll, roll 180, barrel-roll-out and celebration flip.
    Roll { total: f32, roll_left: bool },
    Endover { total: f32, amount: f32, increment: f32, roll_left: bool },
    WhipStrafe { timestamp: f32 },
    RollAway { roll: f32, timestamp: f32, roll_left: bool },
    YoYo { pitch: f32, timestamp: f32 },
    SideStep { roll: f32, pitch: f32, yaw: f32, timestamp: f32, roll_left: bool, yaw_left: bool },
}

/// Turn about one axis by `limit` radians (positive `dir`: pitch up / roll right / yaw left).
/// Returns true once the accumulated angle has reached the limit.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Axis {
    PitchUp,
    PitchDown,
    RollRight,
    RollLeft,
    YawLeft,
    YawRight,
}

impl Axis {
    /// (rotation-speed component, rotation index, positive direction)
    fn parts(self) -> (usize, u16, bool) {
        match self {
            Axis::PitchUp => (1, 2, true),
            Axis::PitchDown => (1, 3, false),
            Axis::RollRight => (2, 4, true),
            Axis::RollLeft => (2, 5, false),
            Axis::YawLeft => (0, 0, true),
            Axis::YawRight => (0, 1, false),
        }
    }
}

fn comp(v: Vector, i: usize) -> f32 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

/// `flightmanPitchUp` and its inlined relatives.
fn turn(u: &mut Universe, id: ShipId, total: &mut f32, axis: Axis, limit: f32, max_speed: f32, accel: f32) -> bool {
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let (c, idx, positive) = axis.parts();
    let rs = comp(s.rotinfo.rotspeed, c);
    let v = ld(dt) * ld(rs) + ld(*total);
    *total = st(v);
    if positive {
        if !lt_or_unordered(v, ld(limit)) {
            return true;
        }
        if lt_or_unordered(ld(rs), ld(max_speed)) {
            let t = st(ld(accel) * ld(s.nonstat.rot_strength[idx as usize]));
            apply_rot(s, t, idx);
        }
    } else {
        if !(v > -ld(limit)) {
            return true;
        }
        if lt_or_unordered(-ld(max_speed), ld(rs)) {
            let t = st(ld(accel) * ld(s.nonstat.rot_strength[idx as usize]));
            apply_rot(s, t, idx);
        }
    }
    false
}

/// `flightmanStabilizePitch` / `Roll` / `Yaw` (component 1 / 2 / 0).
fn stabilize(u: &mut Universe, id: ShipId, c: usize) -> bool {
    let dt = u.phys_time_elapsed;
    let (lo, hi) = (u.tweaks.still_rot_lo, u.tweaks.still_rot_hi);
    let statics = u.statics.clone();
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return true };
    let moi = comp(stat.header.moment_of_inertia, c);
    let rs = comp(s.rotinfo.rotspeed, c);
    let (ccw, cw) = match c {
        0 => (0u16, 1u16),
        1 => (2, 3),
        _ => (4, 5),
    };
    if !(ld(rs) <= ld(hi)) {
        let v = (ld(rs) * ld(moi)) / ld(dt);
        let cap = s.nonstat.rot_strength[cw as usize];
        let t = if ld(cap) < v { cap } else { st(v) };
        apply_rot(s, t, cw);
        false
    } else if ld(rs) < ld(lo) {
        let v = -((ld(rs) * ld(moi)) / ld(dt));
        let cap = s.nonstat.rot_strength[ccw as usize];
        let t = if ld(cap) < v { cap } else { st(v) };
        apply_rot(s, t, ccw);
        false
    } else {
        true
    }
}

/// Thrust along `dir` at `k` times one of the ship's thrust strengths.
fn thrust(u: &mut Universe, id: ShipId, k: f32, strength_index: usize, dir: u16) {
    let s = u.ships.ship_mut(id);
    let f = st(ld(k) * ld(s.nonstat.thrust_strength[strength_index]));
    apply_force(s, f, dir);
}

fn forward(u: &mut Universe, id: ShipId, k: f32) {
    thrust(u, id, k, trans::FORWARD as usize, trans::FORWARD);
}

const PI: f32 = std::f32::consts::PI;
const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;
const TWO_PI: f32 = 2.0 * std::f32::consts::PI;
const FOUR_PI: f32 = 4.0 * std::f32::consts::PI;

/// `ranRandomFn(RAN_GAME)` for a negative flag word.
fn resolve_flags(u: &mut Universe, flags: i32) -> u32 {
    if flags < 0 {
        u.randy.random(hwc_math::random::RAN_GAME)
    } else {
        flags as u32
    }
}

/// `flightmanInitFunc` (0x479360): close the running manoeuvre (keeping `tacticsTalk`), then
/// set up `flightman`. `flags` is a manoeuvre parameter (-1: random); the sandwich takes the
/// target ship instead.
pub fn init(u: &mut Universe, id: ShipId, flightman: u32, flags: i32, target: Option<ShipId>) {
    {
        let s = u.ships.ship_mut(id);
        if s.flightman != kind::NULL {
            let talk = s.tactics_talk;
            s.flightman_info = None;
            s.tactics_talk = talk;
            s.flightman = kind::NULL;
            s.tactics_need_to_flightman = 0;
        }
        s.flightman = flightman;
        s.flightman_state1 = 0;
    }
    let now = u.total_time_elapsed;
    let info = match flightman {
        kind::FLIPTURN => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::FlipTurn { pitch: 0.0, roll: 0.0, finished: 0.0, roll_left: f & 1 != 0 })
        }
        kind::HARDBANK | kind::SOFTBANK | kind::SPLITS_EVASIVE => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::Bank { pitch: 0.0, roll: 0.0, roll_left: f & 1 != 0 })
        }
        kind::IMMELMAN => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::Immelman { pitch: 0.0, roll: 0.0, inverted: f & 1 != 0, roll_left: f & 2 != 0 })
        }
        kind::SPLIT_S => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::SplitS { pitch: 0.0, roll: 0.0, roll_left: f & 1 != 0, inverted: f & 2 != 0 })
        }
        kind::CORKSCREW => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::Corkscrew { yaw: 0.0, roll: 0.0, increment: 4.0, roll_left: f & 1 != 0 })
        }
        kind::ENDOVER => {
            let f = resolve_flags(u, flags);
            let r = u.randy.random(hwc_math::random::RAN_GAME);
            let deg = (f64::from((r & 3) as f32) - 1.5) * 60.0 + 540.0;
            let amount = st(deg * f64::from(0.017_453_292f32));
            Some(FlightmanInfo::Endover {
                total: 0.0,
                amount,
                increment: u.tweaks.combat.flightman.endover_maxrotspeed,
                roll_left: f & 1 != 0,
            })
        }
        kind::BARREL_ROLL | kind::SWARMER_BARRELROLL | kind::ROLL180 => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::Roll { total: 0.0, roll_left: f & 1 != 0 })
        }
        kind::CELEB_FLIP | kind::BARRELROLL_OUT => Some(FlightmanInfo::Roll { total: 0.0, roll_left: false }),
        kind::WHIP_STRAFE => Some(FlightmanInfo::WhipStrafe { timestamp: 0.0 }),
        kind::ROLLAWAY => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::RollAway { roll: 0.0, timestamp: 0.0, roll_left: f & 1 != 0 })
        }
        kind::SLALOM => Some(slalom_init(u, id, flags, now)),
        kind::SIDESTEP => {
            let f = resolve_flags(u, flags);
            Some(FlightmanInfo::SideStep {
                roll: 0.0,
                pitch: 0.0,
                yaw: 0.0,
                timestamp: 0.0,
                roll_left: f & 1 != 0,
                yaw_left: f & 2 != 0,
            })
        }
        kind::LOYOYO | kind::HIYOYO => Some(FlightmanInfo::YoYo { pitch: 0.0, timestamp: 0.0 }),
        kind::SANDWICH => target.map(|t| sandwich_init(u, id, t, now)),
        _ => None,
    };
    let s = u.ships.ship_mut(id);
    s.flightman_info = info;
    if !s.ship_is_attacking {
        s.ship_is_attacking = true;
    }
    if let Some(w) = s.attack.wingman {
        if let Some(ws) = u.ships.get_mut(w) {
            if !ws.ship_is_attacking {
                ws.ship_is_attacking = true;
            }
        }
    }
}

/// `flightmanClose` (0x479410).
pub fn close(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.flightman_info = None;
    s.flightman = kind::NULL;
    s.tactics_need_to_flightman = 0;
    s.tactics_talk = 0;
}

fn slalom_init(u: &mut Universe, id: ShipId, flags: i32, now: f32) -> FlightmanInfo {
    let f = resolve_flags(u, flags);
    let tw = &u.tweaks.combat.flightman;
    let s = u.ships.ship(id);
    let count = ((tw.slalom_randomiterations & f).wrapping_add(tw.slalom_baseiterations)) as i32;
    let mut right = f & 0x100 != 0;
    let h = s.heading();
    let r = s.right();
    let k = ld(tw.slalom_run_length);
    let fr = Vector::new(st(k * (ld(h.x) + ld(r.x))), st(k * (ld(h.y) + ld(r.y))), st(k * (ld(h.z) + ld(r.z))));
    let fl = Vector::new(st(k * (ld(h.x) - ld(r.x))), st(k * (ld(h.y) - ld(r.y))), st(k * (ld(h.z) - ld(r.z))));
    let mut pos = s.posinfo.position;
    let step = |p: Vector, right: bool| if right { p + fr } else { p + fl };
    pos = step(pos, right);
    let mut waypoints = vec![pos];
    let mut i = 1;
    while i < count && waypoints.len() < SLALOM_MAX_WAYPOINTS {
        right = !right;
        pos = step(pos, right);
        waypoints.push(pos);
        i += 1;
    }
    FlightmanInfo::Slalom { waypoints, timestamp: now }
}

fn sandwich_init(u: &mut Universe, id: ShipId, target: ShipId, now: f32) -> FlightmanInfo {
    let tw = &u.tweaks.combat.flightman;
    let s = u.ships.ship(id);
    let tp = u.ships.get(target).map_or(s.posinfo.position, |t| t.posinfo.position);
    let p = s.posinfo.position;
    let k = ld(tw.sandwich_pincher_scale);
    let diff = Vector::new(st(k * (ld(tp.x) - ld(p.x))), st(k * (ld(tp.y) - ld(p.y))), st(k * (ld(tp.z) - ld(p.z))));
    let mut angle = ld(tw.sandwich_angle);
    if s.flightman_state2 == 1 {
        angle = -angle;
    }
    let m = Matrix::rot_about_z(st(fcos(angle)), st(fsin(angle)));
    let w = m.mul_vec(diff);
    let waypoint = Vector::new(w.x + p.x, w.y + p.y, p.z + w.z);
    FlightmanInfo::Sandwich { waypoint, timestamp: now }
}

/// `flightmanExecute` (0x479460): one frame of the running manoeuvre; true (and the manoeuvre
/// closed) when it has finished.
pub fn execute(u: &mut Universe, id: ShipId) -> bool {
    let fm = u.ships.ship(id).flightman;
    if fm == kind::NULL {
        return true;
    }
    let done = match fm {
        kind::FLIPTURN => flip_turn(u, id),
        kind::HARDBANK => bank(u, id, false),
        kind::SOFTBANK => bank(u, id, true),
        kind::IMMELMAN => immelman(u, id),
        kind::SPLIT_S => split_s(u, id),
        kind::CORKSCREW => corkscrew(u, id),
        kind::ENDOVER => endover(u, id),
        kind::BARREL_ROLL | kind::SWARMER_BARRELROLL | kind::ROLL180 => roll(u, id, fm),
        kind::WHIP_STRAFE => whip_strafe(u, id),
        kind::CELEB_FLIP => celeb_flip(u, id),
        kind::BARRELROLL_OUT => barrel_roll_out(u, id),
        kind::ROLLAWAY => roll_away(u, id),
        kind::SPLITS_EVASIVE => split_s_evasive(u, id),
        kind::SLALOM => slalom(u, id),
        kind::SIDESTEP => side_step(u, id),
        kind::LOYOYO => yoyo(u, id, false),
        kind::HIYOYO => yoyo(u, id, true),
        kind::SANDWICH => sandwich(u, id),
        // DONOTHING, and the slots without a routine.
        _ => true,
    };
    if done {
        close(u, id);
    }
    done
}

fn info(u: &Universe, id: ShipId) -> FlightmanInfo {
    u.ships.ship(id).flightman_info.clone().expect("running manoeuvre has its info")
}

fn put(u: &mut Universe, id: ShipId, i: FlightmanInfo) {
    u.ships.ship_mut(id).flightman_info = Some(i);
}

fn state(u: &Universe, id: ShipId) -> u8 {
    u.ships.ship(id).flightman_state1
}

fn set_state(u: &mut Universe, id: ShipId, s: u8) {
    u.ships.ship_mut(id).flightman_state1 = s;
}

fn flip_turn(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::FlipTurn { mut pitch, mut roll, mut finished, roll_left } = info(u, id) else { return true };
    let mut done = false;
    match state(u, id) {
        0 => {
            if turn(u, id, &mut pitch, Axis::PitchDown, PI, t.flipturn_maxrotspeedflip, t.flipturn_flipaccelmodifier) {
                set_state(u, id, 1);
                finished = u.total_time_elapsed;
            }
        }
        1 => {
            stabilize(u, id, 1);
            forward(u, id, t.flipturn_afterflipthrustmod);
            if ld(u.total_time_elapsed) - ld(finished) > ld(t.flipturn_pauseafterfliptime) {
                set_state(u, id, 2);
            }
        }
        2 => {
            forward(u, id, t.flipturn_rollthrustmod);
            let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
            done = turn(u, id, &mut roll, axis, PI, t.flipturn_maxrollspeed, t.flipturn_rollaccelmodifier);
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::FlipTurn { pitch, roll, finished, roll_left });
    done
}

fn corkscrew(u: &mut Universe, id: ShipId) -> bool {
    let FlightmanInfo::Corkscrew { mut yaw, mut roll, mut increment, roll_left } = info(u, id) else { return true };
    let mut done = false;
    if state(u, id) == 0 {
        turn(u, id, &mut yaw, Axis::YawLeft, FOUR_PI, 2.0, 0.25);
        increment = st(ld(increment) - ld(0.05f32));
        done = turn(u, id, &mut roll, Axis::RollRight, TWO_PI, increment, 0.25);
    }
    put(u, id, FlightmanInfo::Corkscrew { yaw, roll, increment, roll_left });
    done
}

fn immelman(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Immelman { mut pitch, mut roll, inverted, roll_left } = info(u, id) else { return true };
    let mut done = false;
    match state(u, id) {
        0 => {
            let axis = if inverted { Axis::PitchDown } else { Axis::PitchUp };
            if turn(u, id, &mut pitch, axis, PI, t.immelman_maxrotspeedflip, t.immelman_accelmodifierflip) {
                set_state(u, id, 1);
            } else {
                forward(u, id, t.immelman_thrustmod);
            }
        }
        1 => {
            stabilize(u, id, 1);
            forward(u, id, t.immelman_rollthrustmod);
            let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
            done = turn(u, id, &mut roll, axis, PI, t.immelman_maxrollspeed, t.immelman_rollaccelmodifier);
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::Immelman { pitch, roll, inverted, roll_left });
    done
}

fn split_s(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::SplitS { mut pitch, mut roll, roll_left, inverted } = info(u, id) else { return true };
    let mut done = false;
    match state(u, id) {
        0 => {
            let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
            if turn(u, id, &mut roll, axis, PI, t.split_s_maxrotspeedroll, t.split_s_accelmodifierroll) {
                set_state(u, id, 1);
            }
        }
        1 => {
            stabilize(u, id, 2);
            let axis = if inverted { Axis::PitchDown } else { Axis::PitchUp };
            if turn(u, id, &mut pitch, axis, PI, t.split_s_maxrotspeedpitch, t.split_s_accelmodifierflip) {
                done = true;
            } else {
                forward(u, id, t.split_s_thrustmod);
            }
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::SplitS { pitch, roll, roll_left, inverted });
    done
}

/// Hard bank (soft: soft bank): roll 90 degrees, then pitch up 180 (soft: 90) with thrust.
fn bank(u: &mut Universe, id: ShipId, soft: bool) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Bank { mut pitch, mut roll, roll_left } = info(u, id) else { return true };
    let (thrust_k, acc_flip, max_pitch, acc_roll, max_roll, pitch_limit) = if soft {
        (t.softbank_thrustmod, t.softbank_accelmodifierflip, t.softbank_maxrotspeedpitch, t.softbank_accelmodifierroll, t.softbank_maxrotspeedroll, HALF_PI)
    } else {
        (t.hardbank_thrustmod, t.hardbank_accelmodifierflip, t.hardbank_maxrotspeedpitch, t.hardbank_accelmodifierroll, t.hardbank_maxrotspeedroll, PI)
    };
    let mut done = false;
    match state(u, id) {
        0 => {
            let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
            if turn(u, id, &mut roll, axis, HALF_PI, max_roll, acc_roll) {
                set_state(u, id, 1);
            }
        }
        1 => {
            stabilize(u, id, 2);
            if turn(u, id, &mut pitch, Axis::PitchUp, pitch_limit, max_pitch, acc_flip) {
                done = true;
            } else {
                forward(u, id, thrust_k);
            }
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::Bank { pitch, roll, roll_left });
    done
}

fn slalom(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Slalom { waypoints, mut timestamp } = info(u, id) else { return true };
    let st1 = state(u, id) as usize;
    let mut next = ld(u.total_time_elapsed) - ld(timestamp) > ld(t.slalom_maxwaypointtime);
    if !next {
        let wp = waypoints.get(st1).copied().unwrap_or_default();
        if aitrack::move_reached_destination(u.ships.ship(id), wp, t.slalom_reachdestinationrange) {
            next = true;
        } else {
            aiship::fly_to_point_avoiding_objs(u, id, Some(wp), fly::FAST_AS_POSSIBLE | fly::POINT_IN_DIRECTION_FLYING, t.slalom_minvelocity, None);
            return false;
        }
    }
    let _ = next;
    let ns = (st1 as u8).wrapping_add(1);
    set_state(u, id, ns);
    if ns as usize >= waypoints.len() {
        return true;
    }
    timestamp = u.total_time_elapsed;
    put(u, id, FlightmanInfo::Slalom { waypoints, timestamp });
    false
}

fn sandwich(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Sandwich { waypoint, timestamp } = info(u, id) else { return true };
    if ld(u.total_time_elapsed) - ld(timestamp) > ld(t.sandwich_maxtime) {
        return true;
    }
    if aitrack::move_reached_destination(u.ships.ship(id), waypoint, t.sandwich_reachdestinationrange) {
        return true;
    }
    aiship::fly_to_point_avoiding_objs(u, id, Some(waypoint), fly::FAST_AS_POSSIBLE | fly::POINT_IN_DIRECTION_FLYING, t.sandwich_minvelocity, None);
    false
}

fn roll(u: &mut Universe, id: ShipId, fm: u32) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Roll { mut total, roll_left } = info(u, id) else { return true };
    let (limit, max, acc) = match fm {
        kind::BARREL_ROLL => (TWO_PI, t.barrel_roll_maxrotspeedroll, t.barrel_roll_accelmodifierroll),
        kind::SWARMER_BARRELROLL => (FOUR_PI, t.swarmer_barrelroll_maxrotspeedroll, t.swarmer_barrelroll_accelmodifierroll),
        _ => (PI, t.roll180_maxrotspeedroll, t.roll180_accelmodifierroll),
    };
    let mut done = false;
    if state(u, id) == 0 {
        let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
        done = turn(u, id, &mut total, axis, limit, max, acc);
    }
    put(u, id, FlightmanInfo::Roll { total, roll_left });
    done
}

fn barrel_roll_out(u: &mut Universe, id: ShipId) -> bool {
    let FlightmanInfo::Roll { mut total, roll_left } = info(u, id) else { return true };
    let mut done = false;
    if state(u, id) == 0 {
        // Up thrust at half the forward strength.
        thrust(u, id, 0.5, trans::FORWARD as usize, trans::UP);
        done = turn(u, id, &mut total, Axis::RollLeft, PI, 1.7, 0.25);
    }
    put(u, id, FlightmanInfo::Roll { total, roll_left });
    done
}

fn celeb_flip(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Roll { mut total, roll_left } = info(u, id) else { return true };
    let mut done = false;
    if state(u, id) == 0 {
        done = turn(u, id, &mut total, Axis::PitchUp, TWO_PI, t.celeb_flip_maxrotspeedroll, t.celeb_flip_accelmodifierroll);
    }
    put(u, id, FlightmanInfo::Roll { total, roll_left });
    done
}

fn endover(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Endover { mut total, amount, mut increment, roll_left } = info(u, id) else { return true };
    let mut done = false;
    if state(u, id) == 0 {
        increment = st(ld(increment) - ld(0.2f32));
        let axis = if roll_left { Axis::YawLeft } else { Axis::YawRight };
        done = turn(u, id, &mut total, axis, amount, increment, t.endover_accelmodifier);
    }
    put(u, id, FlightmanInfo::Endover { total, amount, increment, roll_left });
    done
}

fn whip_strafe(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::WhipStrafe { mut timestamp } = info(u, id) else { return true };
    let mut done = false;
    match state(u, id) {
        0 => {
            let above = u.ships.ship(id).flightman_state2 & 1 != 0;
            let mut updown = Vector::new(0.0, 0.0, if above { -1.0 } else { 1.0 });
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(id);
            if let Some(stat) = statics.ship(s.ship_type) {
                let acc = t.whipstrafe_facedownup_accuracy;
                if aitrack::heading(s, stat, &tw, dt, &mut updown, acc, acc, stat.sinbank, aitrack::IGNORE_UP_VEC) {
                    s.flightman_state1 = 1;
                    timestamp = u.total_time_elapsed;
                }
            }
        }
        1 => {
            forward(u, id, t.whipstrafe_whipthrustmod);
            if ld(u.total_time_elapsed) - ld(timestamp) > ld(t.whipstrafe_whiptime) {
                set_state(u, id, 2);
            }
        }
        2 => done = true,
        _ => {}
    }
    put(u, id, FlightmanInfo::WhipStrafe { timestamp });
    done
}

fn roll_away(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::RollAway { mut roll, mut timestamp, roll_left } = info(u, id) else { return true };
    let up = |u: &mut Universe| thrust(u, id, t.rollaway_thrustmod, trans::UP as usize, trans::UP);
    let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
    let mut done = false;
    match state(u, id) {
        0 => {
            let r = turn(u, id, &mut roll, axis, PI, t.rollaway_maxrotspeedroll, t.rollaway_accelmodifierroll);
            up(u);
            if r {
                set_state(u, id, 1);
                timestamp = u.total_time_elapsed;
            }
        }
        1 => {
            stabilize(u, id, 2);
            up(u);
            if !(ld(u.total_time_elapsed) - ld(timestamp) <= ld(t.rollaway_thrustdowntime)) {
                set_state(u, id, 2);
            }
        }
        2 => {
            done = turn(u, id, &mut roll, axis, TWO_PI, t.rollaway_maxrotspeedroll, t.rollaway_accelmodifierroll);
            up(u);
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::RollAway { roll, timestamp, roll_left });
    done
}

fn split_s_evasive(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::Bank { mut pitch, mut roll, roll_left } = info(u, id) else { return true };
    let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
    let mut done = false;
    match state(u, id) {
        0 => {
            let r = turn(u, id, &mut roll, axis, HALF_PI, t.splitsevasive_maxrotspeedroll, t.splitsevasive_accelmodifierroll);
            forward(u, id, t.splitsevasive_thrustmod);
            if r {
                set_state(u, id, 1);
            }
        }
        1 => {
            stabilize(u, id, 2);
            forward(u, id, t.splitsevasive_thrustmod);
            if turn(u, id, &mut pitch, Axis::PitchUp, HALF_PI, t.splitsevasive_maxrotspeedpitch, t.splitsevasive_accelmodifierpitch) {
                set_state(u, id, 2);
            }
        }
        2 => {
            stabilize(u, id, 1);
            forward(u, id, t.splitsevasive_thrustmod);
            if turn(u, id, &mut roll, axis, PI, t.splitsevasive_maxrotspeedroll, t.splitsevasive_accelmodifierroll) {
                set_state(u, id, 3);
                pitch = 0.0;
            }
        }
        3 => {
            stabilize(u, id, 2);
            forward(u, id, t.splitsevasive_thrustmod);
            done = turn(u, id, &mut pitch, Axis::PitchUp, PI, t.splitsevasive_maxrotspeedpitch, t.splitsevasive_accelmodifierpitch);
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::Bank { pitch, roll, roll_left });
    done
}

/// High (`high`) and low yo-yo.
fn yoyo(u: &mut Universe, id: ShipId, high: bool) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::YoYo { mut pitch, mut timestamp } = info(u, id) else { return true };
    let (k, acc, max, first, second) = if high {
        (t.hiyoyo_thrustmod, t.hiyoyo_accelmodifierpitch, t.hiyoyo_maxrotspeedpitch, t.hiyoyo_goup_time, t.hiyoyo_godown_time)
    } else {
        (t.loyoyo_thrustmod, t.loyoyo_accelmodifierpitch, t.loyoyo_maxrotspeedpitch, t.loyoyo_godown_time, t.loyoyo_goup_time)
    };
    let (out, back) = if high { (Axis::PitchUp, Axis::PitchDown) } else { (Axis::PitchDown, Axis::PitchUp) };
    let now = u.total_time_elapsed;
    let mut done = false;
    match state(u, id) {
        0 => {
            forward(u, id, k[0]);
            if turn(u, id, &mut pitch, out, HALF_PI, max, acc) {
                set_state(u, id, 1);
                timestamp = now;
            }
        }
        1 => {
            stabilize(u, id, 1);
            forward(u, id, k[1]);
            if ld(now) - ld(timestamp) > ld(first) {
                set_state(u, id, 2);
                pitch = 0.0;
            }
        }
        2 => {
            forward(u, id, k[2]);
            if turn(u, id, &mut pitch, back, PI, max, acc) {
                set_state(u, id, 3);
                timestamp = now;
            }
        }
        3 => {
            stabilize(u, id, 1);
            forward(u, id, k[3]);
            if ld(now) - ld(timestamp) > ld(second) {
                set_state(u, id, 4);
                pitch = 0.0;
            }
        }
        4 => {
            forward(u, id, k[4]);
            done = turn(u, id, &mut pitch, out, HALF_PI, max, acc);
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::YoYo { pitch, timestamp });
    done
}

fn side_step(u: &mut Universe, id: ShipId) -> bool {
    let t = u.tweaks.combat.flightman.clone();
    let FlightmanInfo::SideStep { mut roll, mut pitch, mut yaw, mut timestamp, roll_left, yaw_left } = info(u, id) else {
        return true;
    };
    let now = u.total_time_elapsed;
    let mut done = false;
    match state(u, id) {
        0 => {
            let axis = if roll_left { Axis::RollLeft } else { Axis::RollRight };
            let r = turn(u, id, &mut roll, axis, t.sidestep_rollrotate, t.sidestep_maxrotspeedroll, t.sidestep_accelmodifierroll);
            if r {
                stabilize(u, id, 2);
            }
            let p = turn(u, id, &mut pitch, Axis::PitchUp, t.sidestep_pitchrotate, t.sidestep_maxrotspeedpitch, t.sidestep_accelmodifierpitch);
            forward(u, id, t.sidestep_thrustmodturn);
            if r & p {
                set_state(u, id, 2);
                timestamp = now;
            }
        }
        2 => {
            stabilize(u, id, 1);
            forward(u, id, t.sidestep_thrustmod);
            if ld(now) - ld(timestamp) > ld(t.sidestep_pausetime) {
                set_state(u, id, 3);
            }
        }
        3 => {
            let axis = if yaw_left { Axis::YawLeft } else { Axis::YawRight };
            if turn(u, id, &mut yaw, axis, PI, t.sidestep_maxrotspeedyaw, t.sidestep_accelmodifieryaw) {
                set_state(u, id, 4);
                timestamp = now;
            }
        }
        4 => {
            stabilize(u, id, 0);
            forward(u, id, t.sidestep_thrustmod);
            if ld(now) - ld(timestamp) > ld(t.sidestep_bootittime) {
                done = true;
            }
        }
        _ => {}
    }
    put(u, id, FlightmanInfo::SideStep { roll, pitch, yaw, timestamp, roll_left, yaw_left });
    done
}

/// `flightmanGetRandom` (0x4794f0): a manoeuvre of `family` drawn from the probabilities.
pub fn get_random(u: &mut Universe, prob: &crate::combat_statics::FlightManProb, family: u32) -> u32 {
    let pick = |u: &mut Universe, cum: &[u32], sum: u32, base: u32, fallthrough_one: bool| -> u32 {
        let r = u.randy.random(hwc_math::random::RAN_GAME) % sum;
        for (i, &c) in cum.iter().enumerate() {
            if r < c {
                return i as u32 + base;
            }
        }
        let _ = fallthrough_one;
        1
    };
    match family {
        family::EVASIVE_FRONT if prob.sum_evasive_front != 0 => pick(u, &prob.cum_evasive_front, prob.sum_evasive_front, 15, true),
        family::EVASIVE_BEHIND if prob.sum_evasive_behind != 0 => pick(u, &prob.cum_evasive_behind, prob.sum_evasive_behind, 15, true),
        family::EVASIVE_PURE if prob.sum_evasive_pure != 0 => pick(u, &prob.cum_evasive_pure, prob.sum_evasive_pure, 15, true),
        family::TURNAROUND if prob.sum_turnaround != 0 => pick(u, &prob.cum_turnaround, prob.sum_turnaround, 1, true),
        family::AIP if prob.sum_aip != 0 => pick(u, &prob.cum_aip, prob.sum_aip, 8, true),
        _ => kind::DONOTHING,
    }
}

/// `flightmanTestRandom` (0x479650): true with the probability of `flightman` in `family`.
pub fn test_random(u: &mut Universe, prob: &crate::combat_statics::FlightManProb, family: u32, flightman: u32) -> bool {
    let (sum, p) = match family {
        family::EVASIVE_FRONT => (prob.sum_evasive_front, prob.prob_evasive_front.get(flightman.wrapping_sub(15) as usize)),
        family::EVASIVE_BEHIND => (prob.sum_evasive_behind, prob.prob_evasive_behind.get(flightman.wrapping_sub(15) as usize)),
        family::EVASIVE_PURE => (prob.sum_evasive_pure, prob.prob_evasive_pure.get(flightman.wrapping_sub(15) as usize)),
        family::TURNAROUND => (prob.sum_turnaround, prob.prob_turnaround.get(flightman.wrapping_sub(1) as usize)),
        family::AIP => (prob.sum_aip, prob.prob_aip.get(flightman.wrapping_sub(8) as usize)),
        _ => return false,
    };
    if sum == 0 {
        return false;
    }
    let r = u.randy.random(hwc_math::random::RAN_GAME) % sum;
    p.is_some_and(|&p| r < p)
}
