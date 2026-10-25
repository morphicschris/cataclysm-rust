//! Processors and their repair beams (`Processor` module, 0x5b92a0..0x5b9d50): sProcessor,
//! bProcessor. With the auto-repair upgrade each `DPT_AutoRepair` dock point (at most four)
//! is a beam that picks the most damaged friendly ship in its cone and range and gives it
//! `twRepairRate[class]` health per second (to its most damaged module first) until it is
//! whole, out of reach or gone.
//!
//! With effects attached each beam shows the original's `EGT_REPAIR_BEAM` effect, steered from
//! here; a beam whose effect ends by itself lets go of its target, as in the original
//! (`ProcessorStopBeamOnShip`).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::disable::Stopwatch;
use crate::object::{sof, ShipId};
use crate::statics::{DockPointType, ShipStaticInfo};
use crate::universe::Universe;

/// `suShipHasUpgrade(ship, 0x40000)`: the processors' auto-repair upgrade.
pub const UPGRADE_AUTO_REPAIR: u32 = 0x4_0000;
/// Most repair beams a processor has.
pub const MAX_BEAMS: usize = 4;
/// The shortest stopwatch time the beams use.
const MIN_WAIT: f32 = 0.001;
/// Effect handle slots of the beams (+0x798 of each).
const SLOT_BEAM: u32 = 220;
/// Special-purpose slot of the beams (`EGT_REPAIR_BEAM`, 0xae7048).
const REPAIR_BEAM_EFFECT: usize = 18;
/// The widest a beam is drawn (0x8a2320).
const MAX_BEAM_WIDTH: f32 = 250.0;

/// One repair beam (8 dwords at +0x77c: ship, dock point, target, stopwatch, effect).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RepairBeam {
    /// The `DPT_AutoRepair` dock point it shines from.
    pub point: usize,
    /// The ship it repairs.
    pub target: Option<ShipId>,
    /// When to look for a target next.
    pub timer: Stopwatch,
    /// Its effect is showing.
    pub effect: bool,
}

/// Per-ship state: the beams (+0x77c, count at +0x7fc).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessorSpec {
    pub beams: Vec<RepairBeam>,
}

/// Processors have no static initialiser of their own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessorStatics {}

impl ProcessorStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&ProcessorSpec> {
    match &u.ships.get(id)?.spec {
        crate::ships::ShipSpec::Processor(p) => Some(p),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut ProcessorSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        crate::ships::ShipSpec::Processor(p) => Some(p),
        _ => None,
    }
}

/// The beams of a processor.
pub fn beams(u: &Universe, id: ShipId) -> &[RepairBeam] {
    spec(u, id).map_or(&[], |p| &p.beams)
}

/// `ProcessorInitRepairBeams` (0x5b96b0): one beam per auto-repair dock point, looking for a
/// target at once.
pub fn init_repair_beams(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let statics = u.statics.clone();
    let Some(s) = u.ships.get(id) else { return };
    let mut beams = Vec::new();
    if let Some(dock) = statics.ship(s.ship_type).and_then(|st| st.dock.as_ref()) {
        let t = DockPointType::AutoRepair.index();
        let first = dock.first_of_type[t] as i8 as i32;
        let count = dock.count_of_type[t] as i8 as i32;
        for i in 0..count {
            if beams.len() > MAX_BEAMS - 1 {
                break;
            }
            let mut timer = Stopwatch::default();
            timer.start(now, MIN_WAIT);
            beams.push(RepairBeam { point: (first + i) as usize, target: None, timer, effect: false });
        }
    }
    u.ships.ship_mut(id).spec = crate::ships::ShipSpec::Processor(ProcessorSpec { beams });
}

/// `ProcessorInit` (0x5b9a40) (the depot is set up by the base layer).
pub fn init(u: &mut Universe, id: ShipId) {
    init_repair_beams(u, id);
}

/// The world position of the beam's dock point and the world direction of its cone.
fn beam_origin(u: &Universe, id: ShipId, point: usize) -> Option<(Vector, Vector)> {
    let s = u.ships.get(id)?;
    let p = u.statics.ship(s.ship_type)?.dock.as_ref()?.points.get(point)?;
    let o = s.rotinfo.coordsys.mul_vec(p.position);
    let pos = Vector::new(st(ld(o.x) + ld(s.posinfo.position.x)), st(ld(o.y) + ld(s.posinfo.position.y)), st(ld(o.z) + ld(s.posinfo.position.z)));
    Some((pos, s.rotinfo.coordsys.mul_vec(p.cone_normal)))
}

/// `ProcessorRepairBeamCanReach` (0x5b9490): the target is within
/// `twProcessorAutoRepairDistance` of the beam's dock point and inside its cone of
/// `twProcessorAutoRepairConeAngle`.
pub fn beam_can_reach(u: &Universe, id: ShipId, point: usize, target: ShipId) -> bool {
    let tw = &u.tweaks.base;
    let cos_half = st((ld(tw.processor_repair_cone_angle) * 0.5 * 0.017_453_292).cos());
    let range_sq = st(ld(tw.processor_repair_distance) * ld(tw.processor_repair_distance));
    let Some((pos, normal)) = beam_origin(u, id, point) else { return false };
    let Some(t) = u.ships.get(target) else { return false };
    let q = t.posinfo.position;
    let d = Vector::new(st(ld(q.x) - ld(pos.x)), st(ld(q.y) - ld(pos.y)), st(ld(q.z) - ld(pos.z)));
    let dsq = st(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z));
    if range_sq < dsq {
        return false;
    }
    let dot = ld(normal.x) * ld(d.x) + ld(normal.y) * ld(d.y) + ld(normal.z) * ld(d.z);
    !(dot / fsqrt(ld(dsq)) < ld(cos_half))
}

/// The class repair rate (`twRepairRate`) of a ship's type (0 when it cannot be repaired).
pub fn repair_rate(u: &Universe, id: ShipId) -> f32 {
    let Some(s) = u.ships.get(id) else { return 0.0 };
    let class = u.statics.ship(s.ship_type).and_then(|st| st.ship_class).map(|c| c.index());
    class.and_then(|c| u.tweaks.base.repair_rate.get(c).copied()).unwrap_or(0.0)
}

/// `ProcessorFindRepairTarget` (0x5b95e0): the friendly ship with the lowest health fraction
/// that can be repaired and that the beam reaches.
pub fn find_repair_target(u: &Universe, id: ShipId, point: usize) -> Option<ShipId> {
    let me = u.ships.get(id)?;
    let mut best = 1e21f32;
    let mut found = None;
    for &o in &u.ship_list {
        let Some(s) = u.ships.get(o) else { continue };
        if s.player != me.player || s.flags & (sof::DEAD | sof::HIDE) != 0 {
            continue;
        }
        let Some(stat) = u.statics.ship(s.ship_type) else { continue };
        if !(0.0 < repair_rate(u, o)) || s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0 {
            continue;
        }
        if !(s.health < stat.health.max_health) {
            continue;
        }
        let frac = st(ld(stat.health.one_over_max_health) * ld(s.health));
        if frac < best && beam_can_reach(u, id, point, o) {
            best = frac;
            found = Some(o);
        }
    }
    found
}

/// `WorkerRepairMostDamagedModule` (0x5c3290): the ship or the module clamped to it with the
/// lowest health fraction.
pub fn most_damaged_module(u: &Universe, id: ShipId) -> ShipId {
    let frac = |x: ShipId| {
        let s = u.ships.ship(x);
        let max = u.statics.ship(s.ship_type).map_or(1.0, |st| st.health.max_health);
        st(ld(s.health) / ld(max))
    };
    let mut best = frac(id);
    let mut found = id;
    for m in modules(u, id) {
        let f = frac(m);
        if f < best {
            best = f;
            found = m;
        }
    }
    found
}

/// `ShipModuleFirst` / `ShipModuleNext`: the component-class ships clamped to a ship.
pub fn modules(u: &Universe, id: ShipId) -> Vec<ShipId> {
    crate::clamp::slaves_of(u, id)
        .into_iter()
        .filter(|&m| u.ships.get(m).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.ship_class == Some(crate::statics::ShipClass::Component)))
        .collect()
}

fn retarget_wait(u: &Universe) -> f32 {
    let d = u.tweaks.base.processor_repair_update_delay;
    if d <= MIN_WAIT {
        MIN_WAIT
    } else {
        d
    }
}

fn stop_beam(u: &mut Universe, id: ShipId, i: usize) {
    crate::effects::delete_slot(u, id, SLOT_BEAM + i as u32);
    let now = u.total_time_elapsed;
    let wait = retarget_wait(u);
    if let Some(b) = spec_mut(u, id).and_then(|p| p.beams.get_mut(i)) {
        b.target = None;
        b.effect = false;
        b.timer.start(now, wait);
    }
}

/// Whether the processor can use its beams now: built, upgraded, not disabled, dead, hidden,
/// salvaged, held in a depot, in hyperspace, or (+0x1c8 & 0x200000) busy.
fn beams_usable(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION == 0
        && crate::upgrade::has_upgrade(s, UPGRADE_AUTO_REPAIR)
        && s.flags & (sof::DISABLED | sof::DEAD | sof::HIDE) == 0
        && s.special_flags2 & crate::base::special2::BEING_SALVAGED == 0
        && !crate::salvage::is_in_depot(u, id)
        && s.flags & sof::HYPERSPACE == 0
        && s.special_flags & 0x20_0000 == 0
        && !s.disable.disabled
}

/// `ProcessorUpdateRepairBeams` (0x5b9780): repair the targets, drop the ones that are whole
/// or out of reach, and look for new ones when the beams' stopwatches run out.
pub fn update_repair_beams(u: &mut Universe, id: ShipId) {
    let n = spec(u, id).map_or(0, |p| p.beams.len());
    for i in 0..n {
        beam_effect_ended(u, id, i);
    }
    if !beams_usable(u, id) {
        for i in 0..n {
            stop_beam(u, id, i);
        }
        return;
    }
    let dt = u.phys_time_elapsed;
    for i in 0..n {
        let b = spec(u, id).expect("processor").beams[i];
        if let Some(t) = b.target {
            let keep = u.ships.get(t).is_some_and(|ts| {
                ts.flags & (sof::DEAD | sof::HIDE) == 0 && u.statics.ship(ts.ship_type).is_some_and(|st| ts.health < st.health.max_health)
            }) && beam_can_reach(u, id, b.point, t);
            if !keep {
                stop_beam(u, id, i);
            } else {
                let rate = repair_rate(u, t);
                let m = most_damaged_module(u, t);
                let max = u.statics.ship(u.ships.ship(m).ship_type).map_or(0.0, |st| st.health.max_health);
                let ms = u.ships.ship_mut(m);
                let h = st(ld(dt) * ld(rate) + ld(ms.health));
                ms.health = if max < h { max } else { h };
            }
        }
        let now = u.total_time_elapsed;
        let b = spec(u, id).expect("processor").beams[i];
        if b.target.is_none() {
            if b.timer.expired(now) {
                let found = find_repair_target(u, id, b.point);
                let wait = match found {
                    None => {
                        let r = u.tweaks.base.processor_repair_update_rate;
                        if r <= MIN_WAIT {
                            MIN_WAIT
                        } else {
                            r
                        }
                    }
                    Some(_) => MIN_WAIT,
                };
                let bm = &mut spec_mut(u, id).expect("processor").beams[i];
                if found.is_some() {
                    bm.target = found;
                }
                bm.timer.start(now, wait);
            }
            if spec(u, id).expect("processor").beams[i].target.is_none() {
                continue;
            }
        }
        update_beam_effect(u, id, i);
    }
}

/// `ProcessorUpdateRepairBeamEffect` (0x5b92a0): the beam effect follows the dock point and
/// the target (presentation only: announced when it starts).
fn update_beam_effect(u: &mut Universe, id: ShipId, i: usize) {
    let Some(b) = spec_mut(u, id).and_then(|p| p.beams.get_mut(i)) else { return };
    let Some(target) = b.target else { return };
    let point = b.point;
    if !b.effect {
        b.effect = true;
        u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: crate::ships::Effect::Beam { target } });
    }
    if u.effects.is_none() {
        return;
    }
    let Some((pos, _)) = beam_origin(u, id, point) else { return };
    let Some(t) = u.ships.get(target) else { return };
    let tp = t.posinfo.position;
    let r = u.statics.ship(t.ship_type).map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let width = if MAX_BEAM_WIDTH < r { MAX_BEAM_WIDTH } else { r };
    let mut dir = Vector::new(st(ld(tp.x) - ld(pos.x)), st(ld(tp.y) - ld(pos.y)), st(ld(tp.z) - ld(pos.z)));
    let dist = hwc_math::fastmath::fmath_sqrt_double((ld(dir.z) * ld(dir.z) + ld(dir.y) * ld(dir.y)) + ld(dir.x) * ld(dir.x));
    let k = st(1.0 / ld(st(dist)));
    dir = Vector::new(st(ld(k) * ld(dir.x)), st(ld(k) * ld(dir.y)), st(ld(k) * ld(dir.z)));
    let cs = hwc_math::Matrix::coord_sys_from_heading(dir);
    let vel = u.ships.ship(id).posinfo.velocity;
    let slot = SLOT_BEAM + i as u32;
    match crate::shockwave::fx::handle(u, id, slot) {
        Some(e) => crate::shockwave::fx::place(u, e, Some(pos), Some(vel), Some(cs)),
        None => {
            let Some(effect) = crate::shockwave::fx::special(u, REPAIR_BEAM_EFFECT) else { return };
            if !crate::effects::frequency_ok(u, effect) {
                return;
            }
            let params = [crate::effects::pf(st(dist)), crate::effects::pf(width)];
            let e = crate::effects::create(u, effect, None, Some(pos), Some(vel), Some(cs), 1.0, 0, &params);
            crate::effects::set_slot(u, id, slot, e);
        }
    }
}

/// `ProcessorStopBeamOnShip` for an effect that ended by itself: the beam showing it stops.
fn beam_effect_ended(u: &mut Universe, id: ShipId, i: usize) {
    let slot = SLOT_BEAM + i as u32;
    if crate::effects::slot(u, id, slot).is_some() && crate::shockwave::fx::handle(u, id, slot).is_none() {
        stop_beam(u, id, i);
    }
}

/// `ProcessorHouseKeep` (0x5b9a60) (the depot housekeeping runs generically).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    update_repair_beams(u, id);
}

/// `ProcessorAttack` (0x5b9d50): `attackPassive` out to the gun range, holding at 3000.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let max = stat.bullet_range[s.tactics];
    crate::attack::attack_passive(u, id, target, max, 3000.0);
}

/// `RKOAttackPassive`, the processors' passive attack: `attackSimple`.
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId) {
    crate::attack::attack_simple(u, id, target);
}

/// `ProcessorRemoveShipReferences` (0x5b9b20): beams on `gone` stop (the depot lets go of it
/// in the base layer).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let n = spec(u, id).map_or(0, |p| p.beams.len());
    for i in 0..n {
        if spec(u, id).expect("processor").beams[i].target == Some(gone) {
            stop_beam(u, id, i);
        }
    }
}

/// `ProcessorStopBeamOnShip` (0x5b9bd0), slot 13: an effect is being deleted; a beam showing it
/// stops. The simulation keeps no effect objects, so there is nothing to match.
pub fn stop_beam_on_ship(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

/// `ProcessorDied` (0x5b9a80): every beam stops.
pub fn died(u: &mut Universe, id: ShipId) {
    let n = spec(u, id).map_or(0, |p| p.beams.len());
    for i in 0..n {
        stop_beam(u, id, i);
    }
}
