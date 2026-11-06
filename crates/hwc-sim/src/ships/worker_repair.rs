//! Workers repairing friendly ships (`WorkerRepair*`, 0x5c2410..0x5c3750, and the repair half
//! of `WorkerSpecialTarget` / `WorkerOrderChanged` / `WorkerHouseKeep` /
//! `WorkerRemoveShipReferences` / `WorkerDied`, 0x5c40c0..0x5c43f0).
//!
//! A repair order (`kasfRepair`, the "repair" context command) is a special order whose workers
//! take the repair role. Each picks the nearest damaged ally among the targets that no other
//! worker repairs (else one that is being repaired, else the least picked of the whole ones),
//! flies to within `5 x` its radius (800..1500) of its collision box, faces it, opens its jack
//! and repairs `twRepairRate[class]` health per second (ten times that in the tutorial) on its
//! most damaged module until it is whole; then it picks again. With no targets left the order
//! is done.
//!
//! The worker's jack is a mesh animation (open before the beam starts, closed
//! `jackRetractTime` seconds after the last repair). The simulation has no animation clock: an
//! animation counts as finished one update after it starts.
//!
//! With effects attached the beam is the original's `EGT_REPAIR_BEAM` effect. As in the
//! original, a beam whose effect cannot start (no effect, or too many of them) does not count
//! as on: the worker then keeps no beam length to back off to.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::object::{sof, CommandId, ShipId};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

/// `jackRetractTime` when the ship file has none.
const DEFAULT_JACK_RETRACT_TIME: f32 = 6.0;
/// The jack animations.
const ANIM_OPEN: i32 = 0;
const ANIM_CLOSE: i32 = 1;
/// "Never" for the jack timer (0x6258d727).
const NEVER: f32 = f32::from_bits(0x6258_d727);
/// Effect handle slot of the repair beam (+0x674).
const SLOT_REPAIR_BEAM: u32 = 210;
/// Special-purpose slot of the repair beam (`EGT_REPAIR_BEAM`, 0xae7048).
const REPAIR_BEAM_EFFECT: usize = 18;

/// `aitrackHeadingFunc` accuracy while facing the target (0x3f7d70a4).
const FACE_ACCURACY: f32 = f32::from_bits(0x3f7d_70a4);

/// What a worker's special does (+0x640).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Role {
    Repair,
    Salvage,
    /// None given (`WorkerInit`, `WorkerOrderChanged`): the special is the salvage routine.
    #[default]
    None,
}

/// Repair states (+0x66c).
pub mod state {
    pub const CHOOSE: u32 = 0;
    pub const APPROACH: u32 = 1;
    pub const OPEN_JACK: u32 = 3;
    pub const REPAIRING: u32 = 4;
    pub const DONE: u32 = 5;
}

/// The worker-repair fields of a ship.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct WorkerRepairState {
    /// +0x600: workers that picked the ship while it was whole.
    pub idle_picks: i32,
    /// +0x604: workers repairing it.
    pub repairers: i32,
    /// +0x640.
    pub role: Role,
    /// +0x66c: see [`state`].
    pub state: u32,
    /// +0x670: the ship being repaired, +0x608 the same (cleared when it dies).
    pub target: Option<ShipId>,
    pub claimed: Option<ShipId>,
    /// +0x678: the beam is on; +0x674 its effect exists; +0x67c its length (the distance from
    /// the worker's beam point to the target's box, which the worker keeps).
    pub beam: bool,
    pub beam_effect: bool,
    pub beam_length: f32,
    /// +0x63c: when the jack closes.
    pub jack_time: f32,
    /// The mesh animation playing (-1 none) and whether it has finished (+0x4bc +0xc / +0x14).
    pub anim: i32,
    pub anim_finished: bool,
    /// The animation was started this update.
    pub anim_fresh: bool,
}

/// The repair statics of a worker type (`WorkerRepairStaticInit`, 0x5c2410, and
/// `jackRetractTime` of `WorkerStaticInit`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RepairStatics {
    pub jack_retract_time: f32,
    pub repair_distance: f32,
    pub repair_cone: f32,
    pub repair_beam_width: f32,
    /// cos(repairCone / 2).
    pub cos_half_cone: f32,
    /// The `gun0` dock point: where the beam comes from.
    pub beam_point: Vector,
}

/// Reads the worker's repair statics from its ship file.
pub fn repair_statics(stat: &ShipStaticInfo) -> RepairStatics {
    let mut r = RepairStatics { jack_retract_time: DEFAULT_JACK_RETRACT_TIME, repair_distance: 0.0, repair_cone: 0.0, repair_beam_width: 0.0, cos_half_cone: 0.0, beam_point: Vector::ZERO };
    for (k, v) in &stat.custom_entries {
        let f = crate::statics::cscan::scan_f32(v);
        match k.as_str() {
            "jackRetractTime" => r.jack_retract_time = f.unwrap_or(r.jack_retract_time),
            "repairDistance" => r.repair_distance = f.unwrap_or(r.repair_distance),
            "repairCone" => r.repair_cone = f.unwrap_or(r.repair_cone),
            "repairBeamWidth" => r.repair_beam_width = f.unwrap_or(r.repair_beam_width),
            _ => {}
        }
    }
    r.cos_half_cone = st((ld(r.repair_cone) * 0.5 * 0.017_453_292).cos());
    if let Some(p) = stat.dock.as_ref().and_then(|d| d.points.iter().find(|p| p.name.eq_ignore_ascii_case("gun0"))) {
        r.beam_point = p.position;
    }
    r
}

/// `WorkerIsWorker` (0x5c3750).
pub fn is_worker(t: ShipType) -> bool {
    matches!(t, ShipType::sWorker | ShipType::bWorker)
}

fn start_anim(u: &mut Universe, id: ShipId, anim: i32) {
    let flags = if anim == ANIM_CLOSE { crate::mad::STOP_AT_END } else { 0 };
    crate::mad::start(u, id, anim, flags);
}

/// `WorkerInit` (0x5c4060), repair part.
pub fn init(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get_mut(id) else { return };
    let counts = (s.worker_repair.idle_picks, s.worker_repair.repairers);
    s.worker_repair = WorkerRepairState { idle_picks: counts.0, repairers: counts.1, role: Role::None, jack_time: NEVER, anim: -1, anim_finished: true, ..Default::default() };
}

/// The repair command (the original's special-order wrapper with the repair flag, used by
/// `kasfRepair` and the context menu): a special order on the targets, the workers in the
/// repair role.
pub fn cl_repair(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let sel: Vec<ShipId> = ships.iter().copied().filter(|&s| u.ships.get(s).is_some()).collect();
    if sel.is_empty() {
        return;
    }
    // (The worker in the selection acknowledges the order.)
    crate::command::remove_ships_from_doing_stuff(u, &sel);
    let mut c = crate::command::new_command_pub(sel.clone());
    c.order = crate::command::order::SPECIAL;
    c.special_targets = targets.to_vec();
    let cid = u.commands.add_pub(c);
    for &s in &sel {
        let sh = u.ships.ship_mut(s);
        sh.command = Some(cid);
        if sh.special_flags & crate::object::special::ROW_GETTING_OUT_OF_WAY != 0 {
            crate::aiship::row_remove_ship(sh);
        }
    }
    for &s in &sel {
        if is_worker(u.ships.ship(s).ship_type) {
            u.ships.ship_mut(s).worker_repair.role = Role::Repair;
        }
    }
}

/// Whether the worker's special order is a repair.
pub fn is_repairing(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.worker_repair.role == Role::Repair)
}

fn repair_rate(u: &Universe, id: ShipId) -> f32 {
    crate::ships::processor::repair_rate(u, id)
}

/// `WorkerRepairChooseLeastBusy` (0x5c25b0): of the whole ships, the nearest of those picked
/// least often (else the first picked more often that was nearest so far).
fn choose_least_busy(u: &Universe, id: ShipId, list: &[ShipId]) -> Option<ShipId> {
    let p = u.ships.ship(id).posinfo.position;
    let least = list.iter().map(|&t| u.ships.ship(t).worker_repair.idle_picks).fold(i32::MAX, |a, b| if b <= a { b } else { a });
    let mut best = 1e21f32;
    let (mut best_i, mut other_i) = (None, None);
    for (i, &t) in list.iter().enumerate() {
        let s = u.ships.ship(t);
        let q = s.posinfo.position;
        let (dx, dy, dz) = (ld(q.x) - ld(p.x), ld(q.y) - ld(p.y), ld(q.z) - ld(p.z));
        let d = st(dx * dx + dy * dy + dz * dz);
        if d < best {
            if s.worker_repair.idle_picks != least {
                if best_i.is_none() && other_i.is_none() {
                    other_i = Some(i);
                }
            } else {
                best = d;
                best_i = Some(i);
            }
        }
    }
    best_i.or(other_i).map(|i| list[i])
}

/// `WorkerRepairChooseTarget` (0x5c26b0): drops the targets that are not allies or cannot be
/// repaired, then picks the nearest damaged one nobody repairs (else the first busy one that
/// was nearest so far); with none damaged, the least picked whole one (`idle` true).
fn choose_target(u: &Universe, id: ShipId, targets: &mut Vec<ShipId>) -> (Option<ShipId>, bool) {
    let me = u.ships.ship(id);
    let p = me.posinfo.position;
    let player = me.player;
    let mut whole = Vec::new();
    let mut best = 1e21f32;
    let (mut best_i, mut busy_i) = (None, None);
    let mut i = 0;
    while i < targets.len() {
        let t = targets[i];
        let ok = u.ships.get(t).is_some() && crate::shockwave::is_ally(u, t, player) && 0.0 < repair_rate(u, t);
        if !ok {
            targets.swap_remove(i);
            continue;
        }
        let s = u.ships.ship(t);
        if s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION == 0 {
            let m = crate::ships::processor::most_damaged_module(u, t);
            let ms = u.ships.ship(m);
            let max = u.statics.ship(ms.ship_type).map_or(0.0, |st| st.health.max_health);
            if max <= ms.health {
                whole.push(t);
            } else {
                let q = s.posinfo.position;
                let (dx, dy, dz) = (ld(q.x) - ld(p.x), ld(q.y) - ld(p.y), ld(q.z) - ld(p.z));
                let d = st(dx * dx + dy * dy + dz * dz);
                if d < best {
                    if s.worker_repair.repairers != 0 {
                        if best_i.is_none() && busy_i.is_none() {
                            busy_i = Some(i);
                        }
                    } else {
                        best = d;
                        best_i = Some(i);
                    }
                }
            }
        }
        i += 1;
    }
    if let Some(i) = best_i.or(busy_i) {
        return (Some(targets[i]), false);
    }
    if !whole.is_empty() {
        return (choose_least_busy(u, id, &whole), true);
    }
    (None, false)
}

/// `WorkerRepairGetShipBox` (0x5c2a20): the collision box of the ship (centre, lengths) merged
/// with its modules' (in their own coordinates, as the original does).
fn ship_box(u: &Universe, id: ShipId) -> (Vector, Vector) {
    let one = |t: ShipType| {
        let c = &u.statics.ship(t).expect("statics").header.coll;
        let e = Vector::new(c.up_length, c.right_length, c.forward_length);
        let o = c.coll_rect_offset;
        (Vector::new(st(ld(e.x) * 0.5 + ld(o.x)), st(ld(e.y) * 0.5 + ld(o.y)), st(ld(e.z) * 0.5 + ld(o.z))), e)
    };
    let mut b = one(u.ships.ship(id).ship_type);
    for m in crate::ships::processor::modules(u, id) {
        b = merge_boxes(b, one(u.ships.ship(m).ship_type));
    }
    b
}

/// `WorkerRepairMergeBoxes` (0x5c28d0).
fn merge_boxes(a: (Vector, Vector), b: (Vector, Vector)) -> (Vector, Vector) {
    let axis = |ac: f32, ae: f32, bc: f32, be: f32| {
        let mut hi = st(ld(bc) + ld(be));
        let ahi = st(ld(ac) + ld(ae));
        if hi < ahi {
            hi = ahi;
        }
        let alo = st(ld(ac) - ld(ae));
        let blo = st(ld(bc) - ld(be));
        let lo = if blo <= alo { blo } else { alo };
        let e = st((ld(hi) - ld(lo)) * 0.5);
        (st(ld(e) + ld(lo)), e)
    };
    let x = axis(a.0.x, a.1.x, b.0.x, b.1.x);
    let y = axis(a.0.y, a.1.y, b.0.y, b.1.y);
    let z = axis(a.0.z, a.1.z, b.0.z, b.1.z);
    (Vector::new(x.0, y.0, z.0), Vector::new(x.1, y.1, z.1))
}

/// `WorkerRepairWorldToLocal` (0x5c2f00).
fn world_to_local(u: &Universe, p: Vector, id: ShipId) -> Vector {
    let s = u.ships.ship(id);
    let d = Vector::new(st(ld(p.x) - ld(s.posinfo.position.x)), st(ld(p.y) - ld(s.posinfo.position.y)), st(ld(p.z) - ld(s.posinfo.position.z)));
    s.rotinfo.coordsys.vec_mul(d)
}

/// `WorkerRepairClosestPointOnBox` (0x5c2b30): the distance from `p` to the ship's box (per
/// axis outside the box; inside, the coordinate itself counts, as in the original).
fn distance_to_box(u: &Universe, p: Vector, id: ShipId) -> f64 {
    let l = world_to_local(u, p, id);
    let (c, e) = ship_box(u, id);
    let axis = |v: f32, c: f32, e: f32| {
        let lo = st(ld(c) - ld(e));
        let hi = st(ld(e) + ld(c));
        if v < lo {
            st(ld(lo) - ld(v))
        } else if hi < v {
            st(ld(hi) - ld(v))
        } else {
            v
        }
    };
    let (x, y, z) = (axis(l.x, c.x, e.x), axis(l.y, c.y, e.y), axis(l.z, c.z, e.z));
    fsqrt(ld(x) * ld(x) + ld(y) * ld(y) + ld(z) * ld(z))
}

/// `WorkerRepairCalcRepairPos` (0x5c2f50): the worker is within `5 x` the target's radius
/// (800..1500) of its box.
fn in_repair_range(u: &Universe, id: ShipId, target: ShipId) -> bool {
    let tstat = u.statics.ship(u.ships.ship(target).ship_type).expect("statics");
    let r = st(ld(tstat.header.coll.coll_sphere_size) * 5.0).clamp(800.0, 1500.0);
    let l = world_to_local(u, u.ships.ship(id).posinfo.position, target);
    let (c, e) = ship_box(u, target);
    let within = |v: f32, c: f32, e: f32| {
        let m = ld(r) + ld(e);
        ld(c) - m < ld(v) && ld(v) < ld(c) + ld(r) + ld(e)
    };
    within(l.x, c.x, e.x) && within(l.y, c.y, e.y) && within(l.z, c.z, e.z)
}

/// `WorkerRepairMoveToTarget` (0x5c3090): fly to the target until in range; in range, back
/// off to the beam's length, else hold station matching its velocity; face it. True when in
/// range and facing it within the repair cone.
fn move_to_target(u: &mut Universe, id: ShipId) -> bool {
    let Some(target) = u.ships.ship(id).worker_repair.target else { return false };
    if u.ships.get(target).is_none() {
        return false;
    }
    let statics = u.statics.clone();
    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics");
    let rs = repair_statics(stat);
    let in_range = in_repair_range(u, id, target);
    let me = u.ships.ship(id).clone();
    let t = u.ships.ship(target).clone();
    let mut dir = Vector::new(t.posinfo.position.x - me.posinfo.position.x, t.posinfo.position.y - me.posinfo.position.y, t.posinfo.position.z - me.posinfo.position.z);
    dir.normalize();
    let h = me.heading();
    let facing = st(ld(st(ld(h.x) * ld(dir.x))) + ld(st(ld(h.y) * ld(dir.y))) + ld(st(ld(h.z) * ld(dir.z))));
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    if !in_range {
        crate::aiship::fly_to_point_avoiding_objs(u, id, Some(t.posinfo.position), 0, 0.0, None);
    } else {
        let mut backed_off = false;
        if me.worker_repair.beam {
            let d = Vector::new(st(ld(me.posinfo.position.x) - ld(t.posinfo.position.x)), st(ld(me.posinfo.position.y) - ld(t.posinfo.position.y)), st(ld(me.posinfo.position.z) - ld(t.posinfo.position.z)));
            let dist = fsqrt(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z));
            if dist < ld(me.worker_repair.beam_length) {
                let k = (1.0 / dist) * (ld(me.worker_repair.beam_length) - dist);
                let point = Vector::new(st(k * ld(d.x) + ld(me.posinfo.position.x)), st(k * ld(d.y) + ld(me.posinfo.position.y)), st(k * ld(d.z) + ld(me.posinfo.position.z)));
                crate::aiship::fly_to_point_avoiding_objs(u, id, Some(point), 0, 0.0, None);
                backed_off = true;
            }
        }
        if !backed_off {
            let s = u.ships.ship_mut(id);
            crate::aitrack::force_zero_velocity(s);
            crate::aitrack::velocity_vector(s, stat, &tw, dt, t.posinfo.velocity);
        }
    }
    let s = u.ships.ship(id);
    let mut d = Vector::new(t.posinfo.position.x - s.posinfo.position.x, t.posinfo.position.y - s.posinfo.position.y, t.posinfo.position.z - s.posinfo.position.z);
    d.normalize();
    let s = u.ships.ship_mut(id);
    if crate::aitrack::heading(s, stat, &tw, dt, &mut d, FACE_ACCURACY, FACE_ACCURACY, 0.0, 0) {
        crate::aitrack::zero_rotation_anywhere(s, stat, &tw, dt);
    }
    in_range && rs.cos_half_cone <= facing
}

/// `WorkerRepairReleaseTarget` (0x5c2e60).
pub fn release_target(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get_mut(id) else { return };
    let w = &mut s.worker_repair;
    let target = w.target.take();
    if let Some(t) = target {
        if w.claimed == Some(t) {
            w.claimed = None;
        }
    }
    w.beam = false;
    w.beam_effect = false;
    w.beam_length = 0.0;
    w.state = state::CHOOSE;
    if let Some(t) = target.and_then(|t| u.ships.get_mut(t)) {
        t.worker_repair.repairers -= 1;
    }
    crate::effects::delete_slot(u, id, SLOT_REPAIR_BEAM);
}

/// The world position of the worker's beam point.
fn beam_point(u: &Universe, id: ShipId) -> Vector {
    let rs = repair_statics(u.statics.ship(u.ships.ship(id).ship_type).expect("statics"));
    crate::shockwave::fx::local_to_world(u, id, rs.beam_point)
}

/// The width the repair beam is drawn with at `len`: as wide as a 10 degree cone, else as a
/// 26 degree cone or the target's diameter, whichever is less, when that is wider.
fn beam_width(u: &Universe, target: ShipId, len: f64) -> f32 {
    let narrow = st(2.0 * 0.174_532_923_847_436_9f64.tan());
    let wide = st(2.0 * 0.453_785_602_003_335_95f64.tan());
    let r = u.statics.ship(u.ships.ship(target).ship_type).map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let diameter = st(ld(r) + ld(r));
    let a = ld(narrow) * len;
    let b = st(ld(wide) * len);
    let cap = if diameter < b { diameter } else { b };
    if a <= ld(cap) {
        if b <= diameter {
            b
        } else {
            diameter
        }
    } else {
        st(a)
    }
}

/// `WorkerRepairBeamEffect` (0x5c2c60): the beam starts; its length is the distance from the
/// worker's beam point to the target's box.
fn beam_effect(u: &mut Universe, id: ShipId) {
    let Some(target) = u.ships.ship(id).worker_repair.target else { return };
    if u.ships.get(target).is_none() {
        return;
    }
    let p = beam_point(u, id);
    if u.effects.is_some() {
        let effect = crate::shockwave::fx::special(u, REPAIR_BEAM_EFFECT);
        let Some(effect) = effect.filter(|&e| crate::effects::frequency_ok(u, e)) else {
            u.ships.ship_mut(id).worker_repair.beam_effect = false;
            return;
        };
        let (me, t) = (u.ships.ship(id), u.ships.ship(target));
        let cs = hwc_math::Matrix::coord_sys_from_heading(crate::aiship::trajectory(me, t));
        let vel = me.posinfo.velocity;
        let len_ext = distance_to_box(u, p, target);
        let len = st(len_ext);
        let width = beam_width(u, target, len_ext);
        let w = &mut u.ships.ship_mut(id).worker_repair;
        w.beam = true;
        w.beam_length = len;
        let flags = crate::effects::attach::COORDSYS | crate::effects::attach::VELOCITY | crate::effects::attach::NLIPS;
        let params = [crate::effects::pf(len), crate::effects::pf(width)];
        let e = crate::effects::create(u, effect, Some(id.into()), Some(p), Some(vel), Some(cs), 1.0, flags, &params);
        crate::effects::set_slot(u, id, SLOT_REPAIR_BEAM, e);
        u.ships.ship_mut(id).worker_repair.beam_effect = e.is_some();
        u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: crate::ships::Effect::Beam { target } });
        return;
    }
    let len = st(distance_to_box(u, p, target));
    let w = &mut u.ships.ship_mut(id).worker_repair;
    w.beam = true;
    w.beam_effect = true;
    w.beam_length = len;
    u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: crate::ships::Effect::Beam { target } });
}

fn incapable(u: &Universe, id: ShipId) -> bool {
    let s = u.ships.ship(id);
    s.flags & (sof::DISABLED | sof::DEAD | sof::HIDE) != 0 || s.disable.disabled
}

/// `WorkerRepairUpdate` (0x5c32f0): one step of the repair. True when the order is done.
pub fn repair_update(u: &mut Universe, id: ShipId, targets: &mut Vec<ShipId>) -> bool {
    if incapable(u, id) {
        release_target(u, id);
        return false;
    }
    let now = u.total_time_elapsed;
    let retract = repair_statics(u.statics.ship(u.ships.ship(id).ship_type).expect("statics")).jack_retract_time;
    match u.ships.ship(id).worker_repair.state {
        state::CHOOSE => {
            if targets.is_empty() {
                u.ships.ship_mut(id).worker_repair.state = state::DONE;
                return false;
            }
            let (t, idle) = choose_target(u, id, targets);
            let Some(t) = t else { return false };
            if idle {
                u.ships.ship_mut(t).worker_repair.idle_picks += 1;
                {
                    let w = &mut u.ships.ship_mut(id).worker_repair;
                    w.claimed = Some(t);
                    w.target = Some(t);
                }
                move_to_target(u, id);
                // The release counts the ship off its repairers although it was counted as
                // picked while whole (as in the original).
                release_target(u, id);
                return false;
            }
            u.ships.ship_mut(t).worker_repair.repairers += 1;
            let w = &mut u.ships.ship_mut(id).worker_repair;
            w.claimed = Some(t);
            w.target = Some(t);
            w.state = state::APPROACH;
            false
        }
        state::APPROACH => {
            if move_to_target(u, id) {
                u.ships.ship_mut(id).worker_repair.state = state::OPEN_JACK;
            }
            false
        }
        state::OPEN_JACK => {
            move_to_target(u, id);
            let (anim, held) = (crate::mad::current(u, id), crate::mad::paused(u, id));
            if anim == ANIM_OPEN {
                u.ships.ship_mut(id).worker_repair.jack_time = st(ld(now) + ld(retract));
                if held {
                    beam_effect(u, id);
                    u.ships.ship_mut(id).worker_repair.state = state::REPAIRING;
                }
            } else if anim == -1 || held {
                start_anim(u, id, ANIM_OPEN);
                u.ships.ship_mut(id).worker_repair.jack_time = st(ld(now) + ld(retract));
            }
            false
        }
        state::REPAIRING => {
            if let Some(e) = crate::shockwave::fx::handle(u, id, SLOT_REPAIR_BEAM) {
                let p = beam_point(u, id);
                crate::shockwave::fx::place(u, e, Some(p), None, None);
            }
            move_to_target(u, id);
            let (anim, held) = (crate::mad::current(u, id), crate::mad::paused(u, id));
            let keep_open = if anim != ANIM_OPEN {
                if anim == -1 || held {
                    start_anim(u, id, ANIM_OPEN);
                    true
                } else {
                    false
                }
            } else {
                true
            };
            if keep_open {
                u.ships.ship_mut(id).worker_repair.jack_time = st(ld(now) + ld(retract));
            }
            let Some(t) = u.ships.ship(id).worker_repair.target else { return false };
            if u.ships.get(t).is_none() {
                return false;
            }
            let rate = repair_rate(u, t);
            let m = crate::ships::processor::most_damaged_module(u, t);
            let mut add = st(ld(u.phys_time_elapsed) * ld(rate));
            if u.base.options.tutorial {
                add = st(ld(add) * 10.0);
            }
            let max = u.statics.ship(u.ships.ship(m).ship_type).map_or(0.0, |s| s.health.max_health);
            let ms = u.ships.ship_mut(m);
            ms.health = st(ld(add) + ld(ms.health));
            if max <= ms.health {
                ms.health = max;
            }
            if ms.health == max {
                // (The worker reports the repair done.)
                release_target(u, id);
            }
            false
        }
        state::DONE => {
            release_target(u, id);
            true
        }
        _ => true,
    }
}

/// `WorkerSpecialTarget` (0x5c40c0), repair role: the command's targets are filtered in place.
/// True when done (the role is dropped).
pub fn special_target(u: &mut Universe, id: ShipId, cid: CommandId, targets: &[ShipId]) -> bool {
    let mut t = targets.to_vec();
    let done = repair_update(u, id, &mut t);
    if t.len() != targets.len() {
        if let Some(c) = u.commands.get_mut(cid) {
            c.special_targets = t;
        }
    }
    if done {
        u.ships.ship_mut(id).worker_repair.role = Role::None;
    }
    done
}

/// `WorkerOrderChanged` (0x5c4120), repair part: the target is let go and the role dropped.
pub fn order_changed(u: &mut Universe, id: ShipId) {
    release_target(u, id);
    if let Some(s) = u.ships.get_mut(id) {
        s.worker_repair.role = Role::None;
    }
}

/// `WorkerHouseKeep` (0x5c41d0), repair part: the jack closes some time after the last
/// repair; the beam starts once in place, or everything stops when the worker is disabled.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let w = u.ships.ship(id).worker_repair;
    if w.jack_time < now && crate::mad::current(u, id) != ANIM_CLOSE && crate::mad::paused(u, id) {
        start_anim(u, id, ANIM_CLOSE);
        u.ships.ship_mut(id).worker_repair.jack_time = NEVER;
    }
    if !incapable(u, id) {
        let w = u.ships.ship(id).worker_repair;
        if w.beam && !w.beam_effect {
            beam_effect(u, id);
        }
    } else {
        release_target(u, id);
    }
}

/// `WorkerRemoveShipReferences` (0x5c4340), repair part.
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(s) = u.ships.get_mut(id) else { return };
    if s.worker_repair.claimed == Some(gone) {
        s.worker_repair.claimed = None;
    }
    if s.worker_repair.target == Some(gone) {
        release_target(u, id);
    }
}

/// `WorkerDied` (0x5c43f0), repair part.
pub fn died(u: &mut Universe, id: ShipId) {
    release_target(u, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merging_boxes() {
        let a = (Vector::new(0.0, 0.0, 0.0), Vector::new(1.0, 1.0, 1.0));
        let b = (Vector::new(3.0, 0.0, 0.0), Vector::new(1.0, 2.0, 0.5));
        let (c, e) = merge_boxes(a, b);
        assert_eq!(c, Vector::new(1.5, 0.0, 0.0));
        assert_eq!(e, Vector::new(2.5, 2.0, 1.0));
    }
}
