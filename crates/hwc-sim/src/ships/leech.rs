//! The Leech (`Leech` module, 0x5b12d0..0x5b2ac0): sLeech.
//!
//! Given targets with its special (and the leech upgrade), the Leech picks one with a free
//! leech dock point (`DPT_Leech`), flies into the point's cone, latches on (clamped to the
//! target) and drains it every second: in RU mode (7) it gains the target type's
//! `leechRUsPerSecond` up to its capacity and does `leechHPsPerSecond` damage while the target
//! has more than twice that health; in health mode (8) it only does the damage; in attach mode
//! (6) it just sits. When it is full it lets go and docks to unload; when the target is spent
//! it moves on to the next one. An aggressive Leech goes for enemies near it by itself. It is
//! hidden from the other players unless a detector sees it ([`crate::visibility`]), and an EMP
//! makes it let go.

use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{ld, st};
use hwc_math::random::{RAN_GAME, RAN_PARTICLE_STREAM};
use hwc_math::{Matrix, Vector};

use crate::command::order;
use crate::object::{sof, tactics, ShipId};
use crate::statics::{DockPointType, DockStaticPoint, ShipStaticInfo};
use crate::universe::{SpecialEvent, Universe};

use super::{Effect, ShipSpec};

/// Leech states (`ship+0x638`).
pub mod state {
    pub const IDLE: u8 = 0;
    /// Pick a target and a dock point.
    pub const CHOOSE: u8 = 1;
    /// Fly into the dock point's cone.
    pub const APPROACH: u8 = 2;
    /// Fly onto the point and latch.
    pub const LATCH: u8 = 3;
    /// Latched, not draining.
    pub const ATTACHED: u8 = 6;
    /// Draining resource units (and some health).
    pub const DRAIN_RUS: u8 = 7;
    /// Draining health.
    pub const DRAIN_HEALTH: u8 = 8;
    /// Let go and back off (then done).
    pub const DETACH: u8 = 9;
    /// Full or the target died: back off, then unload.
    pub const FULL: u8 = 10;
    /// Done: clean up.
    pub const DONE: u8 = 11;
    pub const DONE2: u8 = 12;
    /// The target has too little health left: back off and try the next one.
    pub const TARGET_SPENT: u8 = 13;
    /// A special order was just given (`clSpecial`).
    pub const ORDERED: u8 = 14;
}

/// Upgrade bit the Leech needs to leech (`LeechSpecialTarget`).
pub const UPGRADE_LEECH: u32 = 0x40;
/// Range of the aggressive Leech's search for victims (`LeechAutoTarget`).
const AUTO_TARGET_RANGE: f32 = 7000.0;
/// `aitrackHeadingAndUp` accuracy while latching.
const LATCH_ACCURACY: f32 = f32::from_bits(0x3f7f_f972);
/// `ship+0x1cc` bit: no collision avoidance while latching (`aishipFlyToPointAvoidingObjs`).
const NO_AVOID: u32 = 0x40;

/// Per-ship state (`ship+0x638..+0x680`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LeechSpec {
    pub state: u8,
    /// Drain mode (+0x639): 6 attach, 7 resource units, 8 health.
    pub mode: u8,
    /// The mode asked for (+0x63a, `LeechSetState`), taken over at the next drain tick.
    pub wanted_mode: u8,
    /// The latch animation is open (+0x63b).
    pub latch_open: bool,
    /// +0x63c.
    pub target: Option<ShipId>,
    /// The ships it was told to leech (+0x640).
    pub targets: Option<Vec<ShipId>>,
    /// Reserved leech point on the target (+0x644, -1 none).
    pub dock_point: i32,
    /// Distance along the cone to approach at (+0x648).
    pub cone_dist: f32,
    /// When the drain effect is replayed (+0x64c).
    pub effect_time: f32,
    /// Next drain tick (+0x650).
    pub drain_time: f32,
    /// Go and unload resource units (+0x674), unloading (+0x675).
    pub go_unload: bool,
    pub unloading: bool,
    /// +0x676.
    pub resume: bool,
    /// Clamp to the target at the next housekeeping (+0x677).
    pub clamp_now: bool,
    /// An EMP hit: drop the order at the next housekeeping (+0x678).
    pub emp_release: bool,
    /// The last target and until when it is remembered (+0x680, +0x67c).
    pub recent_target: Option<ShipId>,
    pub recent_target_time: f32,
}

impl Default for LeechSpec {
    /// `LeechInit` (0x5b2540).
    fn default() -> Self {
        Self {
            state: state::IDLE,
            mode: state::DRAIN_HEALTH,
            wanted_mode: state::DRAIN_HEALTH,
            latch_open: false,
            target: None,
            targets: None,
            dock_point: -1,
            cone_dist: 0.0,
            effect_time: 0.0,
            drain_time: 0.0,
            go_unload: false,
            unloading: false,
            resume: false,
            clamp_now: false,
            emp_release: false,
            recent_target: None,
            recent_target_time: 0.0,
        }
    }
}

/// Per-type statics (`LeechStaticInit` 0x5b2510 reads nothing; the drain rates are the
/// targets' `leechRUsPerSecond` / `leechHPsPerSecond`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct LeechStatics {}

impl LeechStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&LeechSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Leech(l) => Some(l),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut LeechSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Leech(l) => Some(l),
        _ => None,
    }
}

fn l(u: &mut Universe, id: ShipId) -> &mut LeechSpec {
    spec_mut(u, id).expect("leech")
}

/// `LeechInit` (0x5b2540).
pub fn init(u: &mut Universe, id: ShipId) {
    u.ships.ship_mut(id).spec = ShipSpec::Leech(LeechSpec::default());
}

/// `LeechClose` (0x5b25b0): the target list goes.
pub fn close(u: &mut Universe, id: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.targets = None;
    }
}

/// `LeechGetState` (0x5b12d0): the mode asked for.
pub fn get_state(u: &Universe, id: ShipId) -> Option<u8> {
    spec(u, id).map(|s| s.wanted_mode)
}

/// `LeechSetState` (0x5b12e0): ask for a drain mode (6, 7 or 8).
pub fn set_state(u: &mut Universe, id: ShipId, mode: u8) {
    if let Some(s) = spec_mut(u, id) {
        s.wanted_mode = mode;
        // The leech acknowledges the mode (0x40008b1 / 0x40008af / 0x40008b0).
        let e = match mode {
            6 => Some(crate::speech::event::LEECH_HOLD),
            7 => Some(crate::speech::event::LEECH_DRAIN_RUS),
            8 => Some(crate::speech::event::LEECH_DRAIN_HEALTH),
            _ => None,
        };
        if let Some(e) = e {
            crate::speech::queue(u, Some(id), e, 0);
        }
    }
}

/// `LeechIsActive` (0x5b2720).
pub fn is_active(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|s| s.state != state::IDLE)
}

/// `LeechPlayDrainEffect` (0x5b1360): the drain effect of health mode.
///
/// With effects, as the original: when the leech has a drain effect
/// (`EGT_LEECH_VENT_EFFECT` at the lower of its and its target's level of detail) that may
/// start now, it starts on the leech (attached in position and orientation, parameters 1.0
/// and 1.0) and is replayed after 3 to 5 seconds drawn from the particle stream. Without
/// effects it is replayed after 3 seconds.
fn play_drain_effect(u: &mut Universe, id: ShipId, mode: u8) {
    if mode != state::DRAIN_HEALTH {
        return;
    }
    if let Some(t) = spec(u, id).and_then(|s| s.target) {
        u.special_events.push(SpecialEvent::Special {
            ship: id,
            effect: Effect::Beam { target: t },
        });
    }
    let now = u.total_time_elapsed;
    if u.effects.is_some() {
        drain_effect(u, id);
        return;
    }
    if spec(u, id).and_then(|s| s.target).is_some() {
        l(u, id).effect_time = st(ld(now) + 3.0);
    }
}

/// `EGT_LEECH_VENT_EFFECT` in the special-purpose effect table.
const DRAIN_EFFECT_SLOT: usize = 25;

/// The effect part of `LeechPlayDrainEffect` (see [`play_drain_effect`]).
fn drain_effect(u: &mut Universe, id: ShipId) {
    let Some(fx) = u.effects.as_ref() else { return };
    // The lower of the two ships' levels of detail: both are the simulation's.
    let Some(effect) = fx.pick(fx.tables.special_effect(DRAIN_EFFECT_SLOT)) else { return };
    if !crate::effects::frequency_ok(u, effect) {
        return;
    }
    if crate::effects::in_render_list(u, id.into()) {
        let one = crate::effects::pf(1.0);
        let flags = crate::effects::attach::POSITION | crate::effects::attach::COORDSYS;
        crate::effects::create(u, effect, Some(id.into()), None, None, None, 0.0, flags, &[one, one]);
    }
    let r = u.effects.as_mut().expect("effects").world.randy.random(RAN_PARTICLE_STREAM);
    let now = u.total_time_elapsed;
    l(u, id).effect_time = st((f64::from(r) * ld(f32::from_bits(0x3000_0000)) + 3.0) + ld(now));
}

/// The leech point `idx` of the target type.
fn point(u: &Universe, target: ShipId, idx: i32) -> Option<DockStaticPoint> {
    let t = u.ships.get(target)?;
    let d = u.statics.ship(t.ship_type)?.dock.as_ref()?;
    d.points.get(usize::try_from(idx).ok()?).cloned()
}

fn is_free_leech_point(p: &DockStaticPoint, reserved: &[bool], j: usize) -> bool {
    p.dock_type == Some(DockPointType::Leech) && !reserved.get(j).copied().unwrap_or(false)
}

/// Releases the reserved point (the code repeated through `LeechUpdate`).
fn release_point(u: &mut Universe, id: ShipId) {
    let s = l(u, id);
    if s.dock_point == -1 {
        return;
    }
    let (dp, t) = (s.dock_point, s.target);
    s.dock_point = -1;
    if let Some(occ) = t
        .and_then(|t| u.ships.get_mut(t))
        .and_then(|t| t.base.occupancy.as_mut())
    {
        if let Some(r) = occ.reserved.get_mut(dp as usize) {
            *r = false;
        }
    }
}

/// `LeechChooseTarget` (0x5b1450): a random ship of the list with a free leech point (in RU
/// mode one with more than four times the type's RUs per second in health), stream 10.
fn choose_target(u: &mut Universe, id: ShipId) -> bool {
    let s = l(u, id);
    s.target = None;
    let mode = s.mode;
    let Some(list) = s.targets.clone() else {
        return false;
    };
    if list.is_empty() {
        return false;
    }
    let mut cands = Vec::new();
    for (i, &t) in list.iter().enumerate() {
        let Some(ts) = u.ships.get(t) else { continue };
        let Some(tst) = u.statics.ship(ts.ship_type) else {
            continue;
        };
        let rich = mode != state::DRAIN_RUS
            || (tst.leech_rus_per_second.wrapping_shl(2) as f32) < ts.health;
        let free = ts.base.clamp.as_ref().and_then(|c| c.host).is_none();
        let Some(occ) = ts.base.occupancy.as_ref() else {
            continue;
        };
        if !(rich && free) {
            continue;
        }
        let Some(d) = tst.dock.as_ref() else { continue };
        if d.points
            .iter()
            .enumerate()
            .any(|(j, p)| is_free_leech_point(p, &occ.reserved, j))
        {
            cands.push(i);
        }
    }
    if !cands.is_empty() {
        let r = u.randy.random(RAN_GAME);
        l(u, id).target = Some(list[cands[(r % cands.len() as u32) as usize]]);
    }
    l(u, id).target.is_some()
}

/// `LeechReserveDockPoint` (0x5b1620): a random free leech point of the target (stream 10),
/// marked reserved.
fn reserve_dock_point(u: &mut Universe, id: ShipId) -> bool {
    let Some(t) = l(u, id).target else {
        return false;
    };
    let Some(ts) = u.ships.get(t) else {
        return false;
    };
    let Some(occ) = ts.base.occupancy.as_ref() else {
        return false;
    };
    let Some(d) = u.statics.ship(ts.ship_type).and_then(|x| x.dock.as_ref()) else {
        return false;
    };
    let free: Vec<usize> = d
        .points
        .iter()
        .enumerate()
        .filter(|(j, p)| is_free_leech_point(p, &occ.reserved, *j))
        .map(|(j, _)| j)
        .collect();
    if free.is_empty() {
        return false;
    }
    let r = u.randy.random(RAN_GAME);
    let j = free[(r % free.len() as u32) as usize];
    l(u, id).dock_point = j as i32;
    if let Some(r) = u
        .ships
        .ship_mut(t)
        .base
        .occupancy
        .as_mut()
        .and_then(|o| o.reserved.get_mut(j))
    {
        *r = true;
    }
    true
}

/// `LeechDetach` (0x5b1720): the target is gone; let go and pick another (in RU mode with no
/// other target, go and unload what it has).
fn detach(u: &mut Universe, id: ShipId) {
    let old = l(u, id).state;
    l(u, id).target = None;
    match old {
        1..=4 | 6..=12 => {}
        state::ORDERED => {
            l(u, id).state = state::IDLE;
            return;
        }
        _ => return,
    }
    // (The point of a vanished target is not released: the target is already cleared.)
    release_point(u, id);
    let s = u.ships.ship_mut(id);
    s.flags &= !sof::NO_BUMPS;
    s.special_flags2 &= !NO_AVOID;
    if choose_target(u, id) {
        l(u, id).state = state::CHOOSE;
    } else if l(u, id).mode == state::DRAIN_RUS {
        if u.ships.ship(id).base.resources != 0 {
            let s = l(u, id);
            s.go_unload = true;
            s.resume = false;
        }
    } else {
        l(u, id).state = state::DONE;
    }
    if l(u, id).latch_open {
        l(u, id).latch_open = false;
    }
    crate::clamp::unclamp(u, id);
}

/// `LeechBuildTargetList` (0x5b1860): the ordered ships, with a mothership module standing for
/// its mothership (once) and strike craft, components and micro ships left out.
fn build_target_list(u: &Universe, targets: &[ShipId]) -> Vec<ShipId> {
    let mut out = Vec::new();
    let mut module_added = false;
    for &t in targets {
        let Some(ts) = u.ships.get(t) else { continue };
        let ti = ts.ship_type.index();
        if matches!(ti, 0x13..=0x18 | 0x1a..=0x20 | 0x36..=0x39) {
            if !module_added {
                if let Some(m) = ts.base.dock_with {
                    out.push(m);
                    module_added = true;
                }
            }
            continue;
        }
        let class = u
            .statics
            .ship(ts.ship_type)
            .and_then(|s| s.ship_class)
            .map_or(-1, |c| c.index() as i32);
        if !matches!(class, 5 | 6 | 10 | 11) {
            out.push(t);
        }
    }
    out
}

/// The world position, cone direction and the dock point of the reserved point.
fn point_frame(u: &Universe, id: ShipId) -> Option<(DockStaticPoint, Vector, Vector, Matrix)> {
    let s = spec(u, id)?;
    let t = u.ships.get(s.target?)?;
    let p = point(u, t.id, s.dock_point)?;
    let m = t.rotinfo.coordsys;
    let pos = crate::dock::point_world_position(t, p.position);
    let n = m.mul_vec(p.cone_normal);
    Some((p, pos, n, m))
}

fn sub(a: Vector, b: Vector) -> Vector {
    Vector::new(
        st(ld(a.x) - ld(b.x)),
        st(ld(a.y) - ld(b.y)),
        st(ld(a.z) - ld(b.z)),
    )
}

/// Outside the point's cone: `n . d < |d| cos(cone)`. Returns the test and `|d|`.
fn outside_cone(me: Vector, pos: Vector, n: Vector, p: &DockStaticPoint) -> (bool, f32) {
    let d = sub(me, pos);
    let dist = st(fmath_sqrt_double(
        (ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z),
    ));
    let dot = (ld(n.x) * ld(d.x) + ld(n.y) * ld(d.y)) + ld(n.z) * ld(d.z);
    (dot < ld(dist) * ld(p.cone_angle), dist)
}

fn fly(u: &mut Universe, id: ShipId, dest: Vector, flags: u32, limit: f32, target: ShipId) {
    let v = u.ships.ship(target).posinfo.velocity;
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(dest), flags, limit, Some(v));
}

fn heading_and_up(u: &mut Universe, id: ShipId, h: Vector, up: Vector) {
    let statics = u.statics.clone();
    let dt = u.phys_time_elapsed;
    let Some(stat) = statics.ship(u.ships.ship(id).ship_type) else {
        return;
    };
    crate::aitrack::heading_and_up(u.ships.ship_mut(id), stat, dt, h, up, LATCH_ACCURACY);
}

fn reached(u: &Universe, id: ShipId, dest: Vector, bounds: f32) -> bool {
    crate::aitrack::move_reached_destination(u.ships.ship(id), dest, bounds)
}

/// Opens the latch when within 50 of `dest`.
fn open_latch_near(u: &mut Universe, id: ShipId, dest: Vector) {
    if !l(u, id).latch_open && reached(u, id, dest, 50.0) {
        l(u, id).latch_open = true;
    }
}

fn start_latch(u: &mut Universe, id: ShipId) {
    l(u, id).state = state::LATCH;
    let s = u.ships.ship_mut(id);
    s.flags |= sof::NO_BUMPS;
    s.special_flags2 |= NO_AVOID;
}

/// Takes over the mode asked for (6, 7 or 8).
fn sync_mode(u: &mut Universe, id: ShipId) {
    let s = l(u, id);
    if matches!(
        s.wanted_mode,
        state::ATTACHED | state::DRAIN_RUS | state::DRAIN_HEALTH
    ) {
        s.mode = s.wanted_mode;
        s.resume = false;
    }
}

/// Remember the target for five seconds (+0x680 / +0x67c).
fn remember_target(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let s = l(u, id);
    if let Some(t) = s.target {
        s.recent_target = Some(t);
        s.recent_target_time = st(ld(now) + 5.0);
    }
}

/// `LeechUpdate` (0x5b1970): one step of the state machine. `start`: 0 a new special order
/// with `targets`, 1 carry on, 2 the order changed. True when the Leech is idle again.
fn update(u: &mut Universe, id: ShipId, targets: &[ShipId], start: u8) -> bool {
    let target = l(u, id).target;
    if start == 0 {
        let list = build_target_list(u, targets);
        let s = l(u, id);
        s.state = state::CHOOSE;
        s.targets = Some(list);
    } else if start == 2 && l(u, id).state != state::IDLE {
        l(u, id).state = state::DONE;
    }
    let now = u.total_time_elapsed;
    let st_ = l(u, id).state;
    let target = target.filter(|t| u.ships.get(*t).is_some());
    if target.is_none() && st_ != state::CHOOSE && st_ != state::DONE {
        detach(u, id);
        return l(u, id).state == state::IDLE;
    }
    // (Choosing and finishing do not use the old target.)
    let target = target.unwrap_or(id);
    match st_ {
        state::CHOOSE => {
            release_point(u, id);
            if choose_target(u, id) && reserve_dock_point(u, id) {
                if let Some((p, pos, n, _)) = point_frame(u, id) {
                    let (outside, dist) =
                        outside_cone(u.ships.ship(id).posinfo.position, pos, n, &p);
                    if outside {
                        let d = if dist < p.min_dist {
                            p.min_dist
                        } else if p.max_dist < dist {
                            p.max_dist
                        } else {
                            dist
                        };
                        let s = l(u, id);
                        s.cone_dist = d;
                        s.state = state::APPROACH;
                    } else {
                        start_latch(u, id);
                    }
                    return false;
                }
            }
            l(u, id).state = state::DONE;
        }
        state::APPROACH => {
            let Some((p, pos, n, _)) = point_frame(u, id) else {
                return false;
            };
            let (outside, _) = outside_cone(u.ships.ship(id).posinfo.position, pos, n, &p);
            if outside {
                u.ships.ship_mut(id).ship_idle = false;
                let k = l(u, id).cone_dist;
                let dest = Vector::new(
                    st(ld(k) * ld(n.x) + ld(pos.x)),
                    st(ld(k) * ld(n.y) + ld(pos.y)),
                    st(ld(k) * ld(n.z) + ld(pos.z)),
                );
                fly(u, id, dest, 7, -100.0, target);
                open_latch_near(u, id, dest);
                if !reached(u, id, dest, p.min_dist) {
                    return false;
                }
            }
            start_latch(u, id);
        }
        state::LATCH => {
            let Some((p, pos, n, _)) = point_frame(u, id) else {
                return false;
            };
            let sys = Matrix::coord_sys_from_heading(n);
            let h = crate::dock::direction_vector(p.heading_direction, &sys);
            let up = crate::dock::direction_vector(p.up_direction, &sys);
            u.ships.ship_mut(id).ship_idle = false;
            heading_and_up(u, id, h, up);
            let y6 = st(ld(n.y) * 6.0);
            let z6 = st(ld(n.z) * 6.0);
            let dest = Vector::new(
                st(ld(n.x) * 6.0 + ld(pos.x)),
                st(ld(y6) + ld(pos.y)),
                st(ld(z6) + ld(pos.z)),
            );
            fly(u, id, dest, 4, -35.0, target);
            open_latch_near(u, id, dest);
            if !reached(u, id, dest, 2.0) {
                return false;
            }
            sync_mode(u, id);
            let s = l(u, id);
            s.clamp_now = true;
            s.state = s.mode;
            let m = s.mode;
            play_drain_effect(u, id, 5);
            play_drain_effect(u, id, m);
        }
        state::ATTACHED => {
            let old = l(u, id).mode;
            sync_mode(u, id);
            let m = l(u, id).mode;
            if m != old {
                play_drain_effect(u, id, m);
                l(u, id).state = m;
            }
        }
        state::DRAIN_RUS | state::DRAIN_HEALTH => {
            if now < l(u, id).drain_time {
                return false;
            }
            let statics = u.statics.clone();
            let tst = statics
                .ship(u.ships.ship(target).ship_type)
                .expect("statics");
            let hps = tst.leech_hps_per_second;
            let player = u.ships.ship(id).player as i32;
            // Health above twice a tick's damage (the x87 sum).
            let healthy = |u: &Universe| ld(hps) + ld(hps) < ld(u.ships.ship(target).health);
            if st_ == state::DRAIN_RUS {
                let cap = statics
                    .ship(u.ships.ship(id).ship_type)
                    .map_or(0, |x| x.max_resources);
                if healthy(u) {
                    let s = u.ships.ship_mut(id);
                    s.base.resources = s.base.resources.wrapping_add(tst.leech_rus_per_second);
                    if cap < s.base.resources {
                        s.base.resources = cap;
                    }
                }
                if cap <= u.ships.ship(id).base.resources {
                    l(u, id).state = state::FULL;
                    remember_target(u, id);
                }
                if !healthy(u) {
                    if l(u, id).state == state::DRAIN_RUS {
                        l(u, id).state = state::TARGET_SPENT;
                        remember_target(u, id);
                    }
                } else if crate::damage::apply_damage_to_target(
                    u,
                    target,
                    hps,
                    0,
                    crate::damage::how::KILLED_BY_PLAYER,
                    player,
                    Some(id),
                ) {
                    l(u, id).state = state::FULL;
                    remember_target(u, id);
                }
            } else if crate::damage::apply_damage_to_target(
                u,
                target,
                hps,
                0,
                crate::damage::how::KILLED_BY_PLAYER,
                player,
                Some(id),
            ) {
                l(u, id).state = state::FULL;
                remember_target(u, id);
            }
            let old = l(u, id).mode;
            sync_mode(u, id);
            let m = l(u, id).mode;
            if m != old {
                play_drain_effect(u, id, m);
                l(u, id).state = m;
            }
            l(u, id).drain_time = st(ld(now) + 1.0);
        }
        state::DETACH | state::FULL | state::TARGET_SPENT => {
            let Some((p, pos, n, m)) = point_frame(u, id) else {
                return false;
            };
            crate::clamp::unclamp(u, id);
            let k = p.min_dist;
            let dest = Vector::new(
                st(ld(k) * ld(n.x) + ld(pos.x)),
                st(ld(k) * ld(n.y) + ld(pos.y)),
                st(ld(k) * ld(n.z) + ld(pos.z)),
            );
            let h = crate::dock::direction_vector(p.heading_direction, &m);
            let up = crate::dock::direction_vector(p.up_direction, &m);
            u.ships.ship_mut(id).ship_idle = false;
            heading_and_up(u, id, h, up);
            fly(u, id, dest, 0, p.max_dist, target);
            let tpos = u.ships.ship(target).posinfo.position;
            if l(u, id).latch_open && reached(u, id, tpos, p.min_dist) {
                l(u, id).latch_open = false;
            }
            if !reached(u, id, dest, st(ld(p.min_dist) * 0.2)) {
                return false;
            }
            match st_ {
                state::DETACH => {
                    release_point(u, id);
                    l(u, id).state = state::DONE;
                }
                state::FULL => {
                    let s = l(u, id);
                    s.go_unload = true;
                    s.resume = true;
                    s.state = state::DONE;
                    release_point(u, id);
                }
                _ => {
                    release_point(u, id);
                    if let Some(list) = l(u, id).targets.as_mut() {
                        if let Some(i) = list.iter().position(|&x| x == target) {
                            list.remove(i);
                        }
                    }
                    if choose_target(u, id) {
                        l(u, id).state = state::CHOOSE;
                        return false;
                    }
                    if u.ships.ship(id).base.resources != 0 {
                        let s = l(u, id);
                        s.go_unload = true;
                        s.resume = true;
                    }
                    l(u, id).state = state::DONE;
                }
            }
        }
        state::DONE | state::DONE2 => {
            remember_target(u, id);
            l(u, id).latch_open = false;
            release_point(u, id);
            let s = u.ships.ship_mut(id);
            s.flags &= !sof::NO_BUMPS;
            s.special_flags2 &= !NO_AVOID;
            crate::clamp::unclamp(u, id);
            let s = l(u, id);
            s.state = state::IDLE;
            s.targets = None;
        }
        _ => {}
    }
    l(u, id).state == state::IDLE
}

/// `LeechSpecialTarget` (0x5b2640): with the upgrade, leech the targets.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if targets.is_empty() || spec(u, id).is_none() {
        return true;
    }
    if !u
        .ships
        .get(id)
        .is_some_and(|s| crate::upgrade::has_upgrade(s, UPGRADE_LEECH))
    {
        return true;
    }
    let st_ = l(u, id).state;
    let start = if st_ == state::IDLE || st_ == state::ORDERED {
        0
    } else {
        1
    };
    update(u, id, targets, start)
}

/// `LeechOrderChanged` (0x5b26b0): the Leech lets go.
pub fn order_changed(u: &mut Universe, id: ShipId) {
    if spec(u, id).is_some() {
        update(u, id, &[], 2);
    }
}

/// `LeechAutoTarget` (0x5b2740): an idle Leech goes for the enemies near it.
fn auto_target(u: &mut Universe, id: ShipId) {
    if u.ships.ship(id).command.is_some() {
        return;
    }
    let found = crate::tactics::enemies_within_proximity(u, id, AUTO_TARGET_RANGE);
    if !found.is_empty() {
        super::cl_special(u, &[id], &found);
    }
}

/// `LeechHouseKeep` (0x5b27e0).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    if spec(u, id).is_none() {
        return;
    }
    let now = u.total_time_elapsed;
    {
        let s = l(u, id);
        if s.recent_target_time != 0.0 && s.recent_target_time < now {
            s.recent_target_time = 0.0;
            s.recent_target = None;
        }
    }
    // (The "no target" status message of the local player is not modelled.)
    if l(u, id).clamp_now {
        l(u, id).clamp_now = false;
        if let Some(t) = l(u, id).target.filter(|t| u.ships.get(*t).is_some()) {
            crate::clamp::clamp_obj_to_obj(u, id, t);
        }
    }
    if l(u, id).go_unload {
        let s = l(u, id);
        s.go_unload = false;
        s.unloading = true;
        let dw = crate::dock::find_nearest_ship_to_dock_at(
            u,
            id,
            crate::dock::purpose::DEPOSIT_RESOURCES,
        );
        crate::dock::cl_dock(u, &[id], crate::dock::purpose::DEPOSIT_RESOURCES, dw);
    }
    if u.ships.ship(id).base.resources < 1 && l(u, id).unloading {
        let s = l(u, id);
        s.unloading = false;
        s.resume = false;
    }
    let ordered_special = |u: &Universe| {
        u.ships
            .ship(id)
            .command
            .and_then(|c| u.commands.get(c))
            .map(|c| c.order)
    };
    if l(u, id).state == state::IDLE && ordered_special(u) == Some(order::SPECIAL) {
        crate::command::remove_ships_from_doing_stuff(u, &[id]);
    }
    if u.ships.ship(id).tactics == tactics::AGGRESSIVE
        && u.update_counter & 15 == 14
        && ordered_special(u) != Some(order::SPECIAL)
    {
        auto_target(u, id);
    }
    if l(u, id).state == state::DRAIN_HEALTH && l(u, id).effect_time < now {
        play_drain_effect(u, id, state::DRAIN_HEALTH);
    }
    if l(u, id).emp_release {
        l(u, id).emp_release = false;
        crate::command::remove_ships_from_doing_stuff(u, &[id]);
    }
}

/// `LeechRemoveShipReferences` (0x5b26d0).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(s) = spec_mut(u, id) else { return };
    if let Some(list) = s.targets.as_mut() {
        if let Some(i) = list.iter().position(|&x| x == gone) {
            list.remove(i);
        }
    }
    if s.target == Some(gone) {
        detach(u, id);
    }
    let s = l(u, id);
    if s.recent_target == Some(gone) {
        s.recent_target_time = 0.0;
        s.recent_target = None;
    }
}

/// `LeechDied` (0x5b25e0): the reserved point is released.
pub fn died(u: &mut Universe, id: ShipId) {
    if spec(u, id).is_none() {
        return;
    }
    release_point(u, id);
    l(u, id).targets = None;
}

/// `ShipApplyEMP` on a Leech: it drops its order at the next housekeeping (+0x678).
pub fn emp_shocked(u: &mut Universe, id: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.emp_release = true;
    }
}

/// The `clSpecial` hook (`ship+0x638 = 0xe` for Leeches given a special order).
pub fn special_order_given(u: &mut Universe, id: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.state = state::ORDERED;
    }
}
