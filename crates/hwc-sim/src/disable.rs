//! Disabling ships (the "GenericCapital" helpers of Cataclysm, 0x5ae410..0x5aea30): the ACV's
//! EMP (`ShipApplyEMP`), timed disabling (`ShipDisableForTime`, used for derelict-race ships in
//! multiplayer and KAS `ShipsSetHelpless`), the DFG/Nomad field slow-down
//! (`ShipApplyFieldSlowdown`), re-enabling (`ShipReEnable`) and the per-frame check
//! (`ShipDisableUpdate`), with the stopwatch helpers they use (0x5234e0..).

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::Vector;

use crate::object::{sof, Ship, ShipId};
use crate::universe::Universe;

/// The disable reason an EMP or a timed disable sets (`ship + 0x1dc + 9 * 4`).
pub const REASON_EMP: usize = 9;
/// Velocity damping of an EMP-shocked ship per frame (0x8ba4c8).
const EMP_VELOCITY_DAMPING: f32 = 0.9;

/// A stopwatch (`Stopwatch`: start time, duration, a marker and the real-time flag).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Stopwatch {
    pub start: f32,
    pub duration: f32,
    pub real_time: bool,
}

impl Stopwatch {
    /// `stopwatchStart`.
    pub fn start(&mut self, now: f32, duration: f32) {
        self.duration = duration;
        self.start = now;
        self.real_time = false;
    }

    /// `stopwatchExtend`: the time left plus `extra`, from now.
    pub fn extend(&mut self, now: f32, extra: f32) {
        self.duration = st((ld(self.duration) - (ld(now) - ld(self.start))) + ld(extra));
        self.start = now;
        self.real_time = false;
    }

    /// `stopwatchExpired`.
    pub fn expired(&self, now: f32) -> bool {
        !hwc_math::fpu::lt_or_unordered(ld(now) - ld(self.start), ld(self.duration))
    }

    /// `stopwatchRemaining` (register value).
    pub fn remaining(&self, now: f32) -> f64 {
        ld(self.duration) - (ld(now) - ld(self.start))
    }

    /// `stopwatchFraction` (register value; 0 for a non-positive duration).
    pub fn fraction(&self, now: f32) -> f64 {
        if self.duration <= 0.0 {
            return 0.0;
        }
        (ld(now) - ld(self.start)) / ld(self.duration)
    }
}

/// A ship's disable state (+0x528..+0x570).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct DisableState {
    /// +0x528: shocked by an EMP (velocity is damped while disabled).
    pub emp_shocked: bool,
    /// +0x52c: EMP charges taken.
    pub emp_count: i32,
    /// +0x530: disabled for a time.
    pub disabled: bool,
    /// +0x534.
    pub timer: Stopwatch,
    /// +0x544 / +0x548 / +0x54c: the `DISABLED` and `CRAZY` flags and the autostabilise flag
    /// from before, restored when re-enabled.
    pub saved_disabled: bool,
    pub saved_crazy: bool,
    pub saved_autostabilize: bool,
    /// +0x550: in a slow-down field, +0x554 the velocity then, +0x560 its timer.
    pub slowed: bool,
    pub slowed_velocity: Vector,
    pub slow_timer: Stopwatch,
}

/// The flicker effect a KAS-flagged ship shows after an EMP (+0x4d0..+0x4dc): its state (0 off,
/// 1 flickering, else ending), the next flicker time, the end time and the fixed delay.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Flicker {
    pub state: i32,
    pub next: f32,
    pub end: f32,
    pub delay: f32,
}

fn disable_now(u: &mut Universe, id: ShipId, duration: f32) {
    let now = u.total_time_elapsed;
    let s = u.ships.ship_mut(id);
    s.disable.disabled = true;
    crate::base::set_disable_reason(s, true, REASON_EMP);
    let f = s.flags;
    s.disable.saved_disabled = f & sof::DISABLED != 0;
    s.disable.saved_crazy = f & sof::CRAZY != 0;
    s.flags |= sof::DISABLED | sof::CRAZY;
    s.disable.saved_autostabilize = s.autostabilize_ship;
    s.disable.timer.start(now, duration);
    apply_careen_rotation(u, id);
}

/// `ApplyCareenRotationDirectly`: a random spin (the `*CRAZY*` ranges) capped at the type's
/// top rotation speed.
pub fn apply_careen_rotation(u: &mut Universe, id: ShipId) {
    let tw = u.tweaks.combat.clone();
    let dt = ld(u.phys_time_elapsed);
    let r: [f32; 3] = std::array::from_fn(|i| st(u.randy.frandom_between_ext(RAN_GAME, tw.crazy_rock[i][0], tw.crazy_rock[i][1]) * dt));
    let max_rot = u.statics.ship(u.ships.ship(id).ship_type).map_or(0.0, |s| s.header.max_rot);
    let s = u.ships.ship_mut(id);
    let rs = &mut s.rotinfo.rotspeed;
    rs.x = st(ld(r[0]) + ld(rs.x));
    rs.y = st(ld(r[1]) + ld(rs.y));
    rs.z = st(ld(r[2]) + ld(rs.z));
    rs.cap_sloppy(max_rot);
}

/// `ShipDisableForTime` (0x5ae6c0).
pub fn disable_for_time(u: &mut Universe, id: ShipId, duration: f32) {
    if !u.ships.ship(id).disable.disabled {
        disable_now(u, id, duration);
    } else {
        let now = u.total_time_elapsed;
        u.ships.ship_mut(id).disable.timer.extend(now, duration);
    }
}

/// `ShipApplyEMP` (0x5ae410): `charges` EMP hits (at most 10 count) disable the ship for
/// `charges * TimeShockedByEMP` seconds (extending a running disable). Ships being infected
/// by the Beast are immune; types without a shock time only count the hits (and, when KAS
/// flagged them, flicker).
pub fn apply_emp(u: &mut Universe, id: ShipId, charges: i32) {
    let statics = u.statics.clone();
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let n = if charges > 10 { 10 } else { charges };
    if s.beast.timer != 0.0 {
        return;
    }
    if 0.0 < stat.time_shocked_by_emp {
        if !s.disable.emp_shocked {
            u.ships.ship_mut(id).disable.emp_shocked = true;
            crate::ships::emp_shocked(u, id);
            crate::mad::close_guns(u, id);
        }
        let duration = st(ld(n as f32) * ld(stat.time_shocked_by_emp));
        if u.ships.ship(id).disable.disabled {
            let now = u.total_time_elapsed;
            let s = u.ships.ship_mut(id);
            s.disable.timer.extend(now, duration);
            s.disable.emp_count += n;
            return;
        }
        disable_now(u, id, duration);
        u.ships.ship_mut(id).disable.emp_count += n;
        return;
    }
    if u.ships.ship(id).special_flags2 & crate::base::special2::KAS_EMP_FLICKER != 0 {
        let now = u.total_time_elapsed;
        let needed = u.kas_naggarok_emp;
        let s = u.ships.ship_mut(id);
        s.disable.emp_count += charges;
        // Without the KAS variable the original reads the charge count's bits as a float.
        let frac = match needed {
            Some(v) => st(ld(s.disable.emp_count as u32 as f32) / ld(v as f32)),
            None => f32::from_bits(n as u32),
        };
        let frac = frac.clamp(0.0, 1.0);
        let left = st(1.0 - ld(frac));
        s.flicker.delay = left;
        let t = st(ld(left) + ld(now));
        s.flicker.state = 1;
        s.flicker.next = t;
        s.flicker.end = st(ld(t) + 1.0);
    }
}

/// `ShipApplyFieldSlowdown` (0x5ae630): inside the world a field slows the ship for
/// `duration` (extended while it stays in, at most `max`).
pub fn apply_field_slowdown(u: &mut Universe, id: ShipId, duration: f32, max: f32) {
    let now = u.total_time_elapsed;
    let bounds = u.world_bounds;
    let s = u.ships.ship_mut(id);
    if !point_in_universe(s.posinfo.position, bounds) {
        return;
    }
    if !s.disable.slowed {
        s.disable.slowed_velocity = s.posinfo.velocity;
        s.disable.slowed = true;
        s.disable.slow_timer.start(now, duration);
        return;
    }
    s.disable.slow_timer.extend(now, duration);
    if ld(max) < s.disable.slow_timer.remaining(now) {
        s.disable.slow_timer.start(now, max);
    }
}

/// `RammingFrigatePointInUniverse` (0x5bc8a0): inside the world bounds.
pub fn point_in_universe(p: Vector, bounds: Vector) -> bool {
    !(p.z < -bounds.z) && p.z <= bounds.z && -bounds.y <= p.y && p.y <= bounds.y && -bounds.x <= p.x && p.x <= bounds.x
}

fn re_enable_inner(s: &mut Ship) {
    crate::base::set_disable_reason(s, false, REASON_EMP);
    let keep = s.flags & !(sof::DISABLED | sof::CRAZY);
    let mut restored = 0;
    if s.disable.saved_disabled {
        restored |= sof::DISABLED;
    }
    if s.disable.saved_crazy {
        restored |= sof::CRAZY;
    }
    s.flags = keep | restored;
    s.autostabilize_ship = s.disable.saved_autostabilize;
    let v = s.posinfo.velocity;
    if v.x != 0.0 || v.y != 0.0 || v.z != 0.0 {
        s.autostabilize_ship = true;
    }
    s.disable.disabled = false;
    if s.disable.emp_shocked {
        s.disable.emp_shocked = false;
    }
}

/// `ShipReEnable` (0x5ae770).
pub fn re_enable(s: &mut Ship) {
    if s.disable.disabled {
        re_enable_inner(s);
    }
}

/// `ShipDisableUpdate` (0x5ae850): the slow-down ends when its timer runs out or the ship
/// leaves the world; a disabled ship drifts to a stop (EMP) until its timer runs out.
pub fn update(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let bounds = u.world_bounds;
    let s = u.ships.ship_mut(id);
    if s.disable.slowed {
        let inside = point_in_universe(s.posinfo.position, bounds);
        if !inside || s.disable.slow_timer.expired(now) {
            let v = s.posinfo.velocity;
            if v.x != 0.0 || v.y != 0.0 || v.z != 0.0 {
                s.autostabilize_ship = true;
            }
            s.disable.slowed = false;
        }
    }
    if s.disable.disabled {
        if !s.disable.timer.expired(now) {
            if s.disable.emp_shocked {
                let k = ld(EMP_VELOCITY_DAMPING);
                let v = &mut s.posinfo.velocity;
                v.x = st(k * ld(v.x));
                v.y = st(k * ld(v.y));
                v.z = st(k * ld(v.z));
            }
        } else {
            re_enable_inner(s);
        }
    }
}

/// The flicker update of `univShipThink`.
pub fn flicker_update(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    if s.flicker.state == 0 || !(s.flicker.next < now) || !(s.flicker.end < now) {
        return;
    }
    if s.flicker.state == 1 {
        let delay = s.flicker.delay;
        let next = if delay < 0.0 {
            let r = u.randy.random(RAN_GAME);
            st(f64::from(r) * ld(1.862_645_3e-10) + 0.4 + ld(now))
        } else {
            delay
        };
        let r = u.randy.random(RAN_GAME);
        let s = u.ships.ship_mut(id);
        s.flicker.next = next;
        s.flicker.end = st(f64::from(r) * ld(3.492_459_7e-10) + 1.5 + ld(next));
    } else {
        u.ships.ship_mut(id).flicker.state = 0;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::statics::{ShipRace, ShipStaticInfo, ShipType};

    struct EmpCase {
        flags: u32,
        vel: [u32; 3],
        auto: bool,
        shock: u32,
        pre: bool,
        pre_timer: [u32; 2],
        pre_count: i32,
        t0: u32,
        charges: i32,
        t1: u32,
        emp_flags: u32,
        emp_shocked: bool,
        emp_count: i32,
        emp_disabled: bool,
        emp_timer: [u32; 2],
        end_flags: u32,
        end_vel: [u32; 3],
        end_auto: bool,
        end_disabled: bool,
    }

    /// Inputs and results of emulating `ShipApplyEMP` (0x5ae410) and then `ShipDisableUpdate`
    /// (0x5ae850) in cataclysm.exe (Ghidra p-code emulation; the random spin, leech, ram and
    /// salvage helpers stubbed out).
    const EMP_CASES: &[EmpCase] = &[
    EmpCase { flags: 0x200003, vel: [0x435b7204, 0x43561e28, 0x4328bb18], auto: true, shock: 0x40d2c298, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x42c8f796, charges: 9, t1: 0x42c91796, emp_flags: 0xa00003, emp_shocked: true, emp_count: 9, emp_disabled: true, emp_timer: [0x42c8f796, 0x426d1aeb], end_flags: 0xa00003, end_vel: [0x43458036, 0x4340b4bd, 0x4317db95], end_auto: true, end_disabled: true },
    EmpCase { flags: 0x3, vel: [0xc2002d2b, 0xc356ec45, 0x41b9ba95], auto: false, shock: 0x40f197f9, pre: true, pre_timer: [0x43efbb19, 0x41514835], pre_count: 2, t0: 0x43f15d9c, charges: 10, t1: 0x43f1659c, emp_flags: 0x3, emp_shocked: true, emp_count: 12, emp_disabled: true, emp_timer: [0x43f15d9c, 0x42aa9df7], end_flags: 0x3, end_vel: [0xc1e6b7b3, 0xc3416e3e, 0x41a727ec], end_auto: false, end_disabled: true },
    EmpCase { flags: 0x3, vel: [0xc38b4dde, 0x4363bd2e, 0x426f155a], auto: true, shock: 0x4092daf2, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x43842f77, charges: 11, t1: 0x43843777, emp_flags: 0xa00003, emp_shocked: true, emp_count: 10, emp_disabled: true, emp_timer: [0x43842f77, 0x423791ae], end_flags: 0xa00003, end_vel: [0xc37abf5c, 0x434cf70f, 0x42572cd1], end_auto: true, end_disabled: true },
    EmpCase { flags: 0x800003, vel: [0xc37901ba, 0x42b9c310, 0xc25fbb25], auto: false, shock: 0x410c9312, pre: true, pre_timer: [0x43bd6ce4, 0x40224632], pre_count: 5, t0: 0x43beb523, charges: 13, t1: 0x43bebd23, emp_flags: 0x800003, emp_shocked: true, emp_count: 15, emp_disabled: true, emp_timer: [0x43beb523, 0x42afa90c], end_flags: 0x800003, end_vel: [0xc3601b27, 0x42a72f8e, 0xc2495ba1], end_auto: false, end_disabled: true },
    EmpCase { flags: 0x200003, vel: [0xc2882f0b, 0x438969a8, 0x435062c8], auto: false, shock: 0x40564e20, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x43704b34, charges: 12, t1: 0x43705b34, emp_flags: 0xa00003, emp_shocked: true, emp_count: 10, emp_disabled: true, emp_timer: [0x43704b34, 0x4205f0d4], end_flags: 0xa00003, end_vel: [0xc275217a, 0x437757c8, 0x433b8c1a], end_auto: false, end_disabled: true },
    EmpCase { flags: 0x800003, vel: [0x429b588d, 0x43271b45, 0xc30a2278], auto: false, shock: 0x408d8bb9, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x43f132dc, charges: 13, t1: 0x43f13adc, emp_flags: 0xa00003, emp_shocked: true, emp_count: 10, emp_disabled: true, emp_timer: [0x43f132dc, 0x4230eea7], end_flags: 0xa00003, end_vel: [0x428bcfb2, 0x43166557, 0xc2f8a471], end_auto: false, end_disabled: true },
    EmpCase { flags: 0x3, vel: [0x42f86b53, 0xc392b6a8, 0xc1a849eb], auto: true, shock: 0x403d15dd, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x43653cfa, charges: 4, t1: 0x43654cfa, emp_flags: 0xa00003, emp_shocked: true, emp_count: 4, emp_disabled: true, emp_timer: [0x43653cfa, 0x413d15dd], end_flags: 0xa00003, end_vel: [0x42df93ca, 0xc3840aca, 0xc19775ba], end_auto: true, end_disabled: true },
    EmpCase { flags: 0x200003, vel: [0x42ac7549, 0xc3661860, 0xc23e2fc0], auto: false, shock: 0x3f80a9bd, pre: true, pre_timer: [0x43f96ff5, 0x40abe398], pre_count: 5, t0: 0x43f97c85, charges: 11, t1: 0x43f98485, emp_flags: 0x200003, emp_shocked: true, emp_count: 15, emp_disabled: true, emp_timer: [0x43f97c85, 0x417533f8], end_flags: 0x200003, end_vel: [0x429b365b, 0xc34f15f0, 0xc22b2af9], end_auto: false, end_disabled: true },
    EmpCase { flags: 0x3, vel: [0xc32c0dd2, 0xc3110891, 0x43239d23], auto: true, shock: 0x41222186, pre: true, pre_timer: [0x42360f0f, 0x4147d3df], pre_count: 2, t0: 0x4237dc75, charges: 1, t1: 0x428c6e98, emp_flags: 0x3, emp_shocked: true, emp_count: 3, emp_disabled: true, emp_timer: [0x4237dc75, 0x41b15fe6], end_flags: 0x3, end_vel: [0xc32c0dd2, 0xc3110891, 0x43239d23], end_auto: true, end_disabled: false },
    EmpCase { flags: 0x800003, vel: [0xc1e099e2, 0x4389bd8b, 0xc11c3e96], auto: false, shock: 0x4128822e, pre: false, pre_timer: [0x00000000, 0x00000000], pre_count: 0, t0: 0x439eaccc, charges: 5, t1: 0x439eb4cc, emp_flags: 0xa00003, emp_shocked: true, emp_count: 5, emp_disabled: true, emp_timer: [0x439eaccc, 0x4252a2ba], end_flags: 0xa00003, end_vel: [0xc1ca2418, 0x4377eec7, 0xc10c9eba], end_auto: false, end_disabled: true },

    ];

    #[test]
    fn emp_matches_emulated_binary() {
        let f = f32::from_bits;
        const MASK: u32 = sof::DISABLED | sof::CRAZY;
        for (i, c) in EMP_CASES.iter().enumerate() {
            let mut st = ShipStaticInfo::new(ShipType::tHeavyInterceptor, String::new());
            st.ship_race = ShipRace::Taiidan;
            st.health.max_health = 100.0;
            st.time_shocked_by_emp = f(c.shock);
            let world = crate::statics::test_world(vec![st]);
            let mut u = Universe::new(Arc::new(world), crate::tunables::SimTweaks::compiled(), &[ShipRace::Taiidan]);
            let id = u.add_ship(ShipType::tHeavyInterceptor, 0, Vector::ZERO, None).unwrap();
            {
                let s = u.ships.ship_mut(id);
                s.flags = (s.flags & !MASK) | (c.flags & MASK);
                s.posinfo.velocity = Vector::new(f(c.vel[0]), f(c.vel[1]), f(c.vel[2]));
                s.autostabilize_ship = c.auto;
                if c.pre {
                    s.disable.disabled = true;
                    s.disable.emp_shocked = true;
                    s.disable.timer = Stopwatch { start: f(c.pre_timer[0]), duration: f(c.pre_timer[1]), real_time: false };
                    s.disable.emp_count = c.pre_count;
                }
            }
            u.total_time_elapsed = f(c.t0);
            apply_emp(&mut u, id, c.charges);
            let s = u.ships.ship(id);
            assert_eq!(s.flags & MASK, c.emp_flags & MASK, "case {i} flags");
            assert_eq!(s.disable.emp_shocked, c.emp_shocked, "case {i} shocked");
            assert_eq!(s.disable.emp_count, c.emp_count, "case {i} count");
            assert_eq!(s.disable.disabled, c.emp_disabled, "case {i} disabled");
            assert_eq!([s.disable.timer.start.to_bits(), s.disable.timer.duration.to_bits()], c.emp_timer, "case {i} timer");
            u.total_time_elapsed = f(c.t1);
            update(&mut u, id);
            let s = u.ships.ship(id);
            assert_eq!(s.flags & MASK, c.end_flags & MASK, "case {i} flags after");
            let v = s.posinfo.velocity;
            assert_eq!([v.x.to_bits(), v.y.to_bits(), v.z.to_bits()], c.end_vel, "case {i} velocity");
            assert_eq!(s.autostabilize_ship, c.end_auto, "case {i} autostabilise");
            assert_eq!(s.disable.disabled, c.end_disabled, "case {i} disabled after");
        }
    }
}
