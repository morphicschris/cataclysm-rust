//! The ramming frigate (`RammingFrigate` module, 0x5ba110..0x5bcab0): sRammingFrigate, bRammingFrigate.
//!
//! A rammer given a target (attack order or special target) turns to face it, closes in, then
//! charges: it drives straight at the target at a speed above its normal top speed. When the
//! charge bumps into the target (the bump collision code calls [`collide`]) the rammer snaps
//! its bow onto the target's hull at the nearest snap point, deals its ram damage (killing
//! fighters, micro ships, ACVs, MCVs and Beast heavy corvettes outright), clamps itself to the
//! target and pushes it along its own heading for `RamPushTime` seconds, cutting into it with
//! `cuttingBeamDamagePerSecond`. Afterwards it waits `RamWaitAfterPushTime` before the next run.
//!
//! The states (`+0x638`): 0 idle, 1 turning towards the target, 2 closing in, 3 charging,
//! 5 pushing (4 is never entered).
//!
//! With effects attached the rammer makes the original's effects: the impact when it latches on
//! (`EGT_RAMMING_FRIGATE_IMPACT_EFFECT`), the vents of its collapsing bow
//! (`EGT_RAM_VENT`) and the cutting beam while it pushes (`EGT_BULLET_CUTTING_BEAM`).
//!
//! Not reproduced: the animations, sounds, chatter and trail scaling of the original
//! (presentation only; the state changes are reported as [`Effect::State`] events), and the
//! ray tests against the target's polygon mesh (`meshShipSegmentCollide`: the snap point and the
//! cutting beam's length), which use the target's collision box here (they test the
//! full-detail mesh; the simulation keeps only the LOD 3 hull of the hit tests,
//! [`crate::statics::hull`]).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st, Ext};
use hwc_math::{Matrix, Vector};

use crate::disable::Stopwatch;
use crate::object::{Ship, ShipId};
use crate::shockwave::fx;
use crate::ships::{Effect, ShipSpec, TypeStatics};
use crate::statics::{ShipClass, ShipStaticInfo, ShipType};
use crate::universe::{SpecialEvent, Universe};

/// Ramming states (`+0x638`).
pub mod state {
    pub const IDLE: u8 = 0;
    /// Turning to face the target until the recovery time is over.
    pub const TURNING: u8 = 1;
    /// Flying towards the target until close enough to charge.
    pub const CLOSING: u8 = 2;
    /// Charging at the target.
    pub const CHARGING: u8 = 3;
    /// Clamped to the target, pushing it.
    pub const PUSHING: u8 = 5;
}

/// Upgrade bit: faster, longer charges (`suShipHasUpgrade(ship, 0x100)`).
pub const UPGRADE_RAM: u32 = 0x100;
/// Upgrade bit: the upgraded cutting beam (`0x2000000`).
pub const UPGRADE_CUTTING_BEAM: u32 = 0x200_0000;
/// Upgrade bit: immune to Beast infection when ramming Beast ships (`0x400000`).
pub const UPGRADE_INFECTION_IMMUNE: u32 = 0x40_0000;

/// `specialFlags & 0x80` (HW1's `SPECIAL_SinglePlayerWithinHyperSpaceRange` bit): the rammer
/// drops what it is doing every frame while it is set.
const SPECIAL_LEAVING: u32 = 0x80;

/// Initial value of the counter at `+0x648` (0x86e014).
const INITIAL_COUNTER: i32 = 300;
/// Heading accuracy while turning and closing (0x86e018).
const HEADING_ACCURACY: f32 = f32::from_bits(0x3f7d_70a4);
/// Duration of the charge before an upgraded rammer gives up (0x8bbb50).
const CHARGE_TIME: f32 = 25.0;
/// Charge speed factor, normal and upgraded (0x8bbb6c, 0x8bbb70).
const CHARGE_SPEED: [f32; 2] = [f32::from_bits(0x3f99_999a), 2.0];
/// The heading blend weights of the charge (0x8bbb5c, 0x8bbb60).
const BLEND_NEW: f32 = f32::from_bits(0x3d8f_5c29);
const BLEND_OLD: f32 = 1.0;
/// Speed above which an unupgraded charge loses speed when turning (0x8bbb64).
const CHARGE_TURN_SPEED: f32 = 400.0;
/// Most speed lost per frame to turning (0x8bbb68).
const CHARGE_TURN_LOSS: f32 = f32::from_bits(0x3dcc_cccd);
/// Extra distances beyond the target's radius: start the charge (fighters, corvettes / others),
/// abort a charge at small targets (0x8bbb78, 0x8bbb74, 0x8bbb7c).
const CHARGE_DIST_SMALL: f32 = 600.0;
const CHARGE_DIST_BIG: f32 = 2000.0;
const ABORT_DIST: f32 = 700.0;
/// Facing needed to start closing in (0x8bbb80) and to keep charging (0x8bbb84).
const FACING_TO_CLOSE: f32 = 0.5;
const FACING_TO_CHARGE: f32 = f32::from_bits(0x3eae_147b);
/// Delay before the line of sight is checked again (0x8bbb88).
const SIGHT_RETRY: f32 = 4.0;
/// Half length of the snap-point ray (0x8bbb90) and the distance behind the rammer its snap
/// candidates are measured from (0x8bbb94).
const SNAP_RAY: f32 = 500.0;
const SNAP_BEHIND: f32 = -1500.0;
/// Duration of the collapse animation (0x8bbb98).
const COLLAPSE_TIME: f32 = f32::from_bits(0x3f4c_cccd);
/// Push force, normal and upgraded (0x8bbbdc, 0x8bbbe0).
const PUSH_FORCE: [f32; 2] = [500_000.0, 800_000.0];
/// `RammingFrigatePushRotate` facing threshold and torque cap (0x8bbbe4, 0x8bbbe8).
const PUSH_ROTATE_FACING: f32 = f32::from_bits(0x3e4c_cccd);
const PUSH_ROTATE_MAX: f32 = 400.0;
/// Speed bonus of an upgraded charge (0x8bbbec).
const RAM_SPEED_BONUS: f32 = f32::from_bits(0x3f19_999a);
/// Start duration of every stopwatch (0.01 s).
const INIT_TIMER: f32 = f32::from_bits(0x3c23_d70a);

/// Effect handle slot of the cutting beam (+0x674).
const SLOT_CUTTING_BEAM: u32 = 200;
/// Special-purpose slots: the impact (0xae7080), the cutting beam (0xae7094) and the vents
/// (0xae70a4).
const IMPACT_EFFECT: usize = 32;
const CUTTING_BEAM_EFFECT: usize = 37;
const VENT_EFFECT: usize = 41;
/// The vents only show at the finer levels of detail (0x8bbbb4).
const VENT_MAX_LOD: u8 = 3;
/// The collapse is far enough along for the vents after this long (0x8bbb9c).
const VENT_DELAY: f32 = f32::from_bits(0x3e99_999a);
/// The bow corners the vents blow from (`RammingFrigateGetSnapBounds`, 0x8bbba0..0x8bbbb0):
/// starboard top, port top, starboard bottom, port bottom.
const VENT_POINTS: [Vector; 4] =
    [Vector::new(37.0, -50.0, -43.0), Vector::new(-34.0, -50.0, -43.0), Vector::new(37.0, 50.0, -43.0), Vector::new(-34.0, 50.0, -43.0)];
/// Where the cutting beam starts, in the rammer's coordinates (0x8bbbcc, 0x91cea8, 0x8bbbd0),
/// and how far it reaches (0x8bbbd4).
const CUTTING_BEAM_POINT: Vector = Vector::new(-75.0, 0.0, -25.0);
const CUTTING_BEAM_REACH: f32 = 2000.0;

/// The snap points, in the order `RammingFrigateFindSnapPoint` looks them up.
const SNAP_POINTS: [&str; 9] =
    ["SnapCenter", "SnapStarbFT", "SnapStarbFB", "SnapPortFT", "SnapPortFB", "SnapStarbBT", "SnapStarbBB", "SnapPortBT", "SnapPortBB"];

/// Per-ship state (`ShipSpecifics`, +0x638).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct RammingSpec {
    /// +0x638: see [`state`].
    pub state: u8,
    /// +0x648: 300 at creation, 0 once a charge starts (nothing else reads it).
    pub counter: i32,
    /// +0x64c: the ship being rammed.
    pub target: Option<ShipId>,
    /// +0x658: the push.
    pub push_timer: Stopwatch,
    /// +0x668: the target's heading when the push started.
    pub push_heading: Vector,
    /// +0x67c: recovery after a push or a failed ram (`RamWaitAfterPushTime`).
    pub recover_timer: Stopwatch,
    /// +0x68c: the charge (upgraded rammers give up when it runs out).
    pub charge_timer: Stopwatch,
    /// +0x69c / +0x6ac: the collapse animation after the push starts.
    pub collapse_timer: Stopwatch,
    pub collapsing: bool,
    /// +0x6b0: the target is an enemy (the cutting beam only damages enemies).
    pub enemy_target: bool,
    /// +0x6b4: the line-of-sight retry while closing in.
    pub sight_timer: Stopwatch,
    /// +0x674: the cutting-beam effect is on (presentation).
    pub beam_on: bool,
}

/// Per-type statics (`RammingFrigateStaticInit`, 0x5ba480; the ship file's ram keywords).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct RammingStatics {
    /// `RamDamage`.
    pub ram_damage: f32,
    /// `cuttingBeamDamagePerSecond` and `cuttingBeamUpgradedDamagePerSecond`.
    pub cutting_damage: f32,
    pub cutting_damage_upgraded: f32,
    /// `RamPushTime`.
    pub push_time: f32,
    /// `RamWaitAfterPushTime`.
    pub wait_after_push: f32,
}

impl RammingStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        Self {
            ram_damage: stat.ram_damage,
            cutting_damage: stat.cutting_beam_damage_per_second,
            cutting_damage_upgraded: stat.cutting_beam_upgraded_damage_per_second,
            push_time: stat.ram_push_time,
            wait_after_push: stat.ram_wait_after_push_time,
        }
    }
}

/// Whether a type is a ramming frigate.
pub fn is_rammer_type(t: ShipType) -> bool {
    matches!(t, ShipType::sRammingFrigate | ShipType::bRammingFrigate)
}

fn ram_statics(u: &Universe, t: ShipType) -> RammingStatics {
    match crate::ships::type_statics(u, t) {
        TypeStatics::Ramming(s) => s.clone(),
        _ => RammingStatics::default(),
    }
}

fn spec_of(s: &Ship) -> Option<&RammingSpec> {
    match &s.spec {
        ShipSpec::Ramming(r) => Some(r),
        _ => None,
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&RammingSpec> {
    u.ships.get(id).and_then(spec_of)
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut RammingSpec> {
    match u.ships.get_mut(id).map(|s| &mut s.spec) {
        Some(ShipSpec::Ramming(r)) => Some(r),
        _ => None,
    }
}

fn has_upgrade(u: &Universe, id: ShipId, bit: u32) -> bool {
    u.ships.get(id).is_some_and(|s| crate::upgrade::has_upgrade(s, bit))
}

fn effect(u: &mut Universe, ship: ShipId, effect: Effect) {
    u.special_events.push(SpecialEvent::Special { ship, effect });
}

/// `RammingFrigateIsImmuneType` (0x5ba110): the RKO and the Nomad moon parts cannot be rammed
/// safely by anything that could be infected.
fn is_immune_type(t: ShipType) -> bool {
    matches!(t, ShipType::dRKO | ShipType::dNomadMoon | ShipType::dNomadFieldEmitter)
}

/// Beast ship types (`0x20 < type < 0x3c`).
fn is_beast_type(t: ShipType) -> bool {
    (0x21..0x3c).contains(&t.index())
}

/// `RammingFrigateTargetIsImmune` (0x5ba140): an immune type or a ship with a Beast cascade,
/// or clamped to one.
fn target_is_immune(u: &Universe, id: ShipId) -> bool {
    let immune = |sid: ShipId| u.ships.get(sid).is_some_and(|s| is_immune_type(s.ship_type) || beast_has_cascade(u, sid));
    if immune(id) {
        return true;
    }
    crate::clamp::host_of(u, id).is_some_and(immune)
}

/// `beastShipHasCascade` (Beast module).
fn beast_has_cascade(u: &Universe, id: ShipId) -> bool {
    crate::ships::beast::ship_has_cascade(u, id)
}

/// `RammingFrigateHasRamUpgrade` (0x5ba1f0).
pub fn has_ram_upgrade(u: &Universe, id: ShipId) -> bool {
    has_upgrade(u, id, UPGRADE_RAM)
}

/// `RammingFrigateResetRamTimer` (0x5ba210): start the recovery time.
fn reset_ram_timer(u: &mut Universe, id: ShipId) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    if !is_rammer_type(t) {
        return;
    }
    let wait = ram_statics(u, t).wait_after_push;
    let now = u.total_time_elapsed;
    if let Some(r) = spec_mut(u, id) {
        r.recover_timer.start(now, wait);
    }
}

/// `RammingFrigateSetState` (0x5ba250).
pub fn set_state(u: &mut Universe, id: ShipId, new: u8) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let Some(old) = spec(u, id).map(|r| r.state) else { return };
    let now = u.total_time_elapsed;
    let wait = ram_statics(u, t).wait_after_push;
    crate::effects::delete_slot(u, id, SLOT_CUTTING_BEAM);
    if old == state::PUSHING && new != state::PUSHING {
        crate::clamp::unclamp(u, id);
        if let Some(r) = spec_mut(u, id) {
            r.recover_timer.start(now, wait);
        }
    }
    if let Some(r) = spec_mut(u, id) {
        r.beam_on = false;
    }
    let s = u.ships.ship_mut(id);
    if new == state::IDLE {
        s.ship_idle = true;
        s.autostabilize_ship = true;
    }
    let ShipSpec::Ramming(r) = &mut s.spec else { return };
    if new == state::IDLE {
        r.target = None;
    } else if new == state::CHARGING && old != state::CHARGING {
        r.charge_timer.start(now, CHARGE_TIME);
    }
    if new == state::PUSHING && old != state::PUSHING {
        r.collapsing = true;
        r.collapse_timer.start(now, COLLAPSE_TIME);
    }
    r.state = new;
    effect(u, id, Effect::State { state: u32::from(new) });
}

/// `RammingFrigateInit` (0x5ba4f0).
pub fn init(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let mut r = RammingSpec { counter: INITIAL_COUNTER, ..Default::default() };
    r.charge_timer.start(now, INIT_TIMER);
    r.recover_timer.start(now, INIT_TIMER);
    r.sight_timer.start(now, INIT_TIMER);
    r.collapse_timer.start(now, INIT_TIMER);
    u.ships.ship_mut(id).spec = ShipSpec::Ramming(r);
}

/// `RammingFrigateClose` (0x5bb490).
pub fn close(u: &mut Universe, id: ShipId) {
    if let Some(r) = spec_mut(u, id) {
        r.target = None;
    }
}

/// `RammingFrigateCanRam` (0x5ba580, inlined in the attack routines): whether `a` and `b` are
/// the same structure (the same ship, or clamped to each other or to the same host).
pub(crate) fn same_structure(u: &Universe, a: Option<ShipId>, b: ShipId) -> bool {
    let Some(a) = a else { return false };
    if a == b {
        return true;
    }
    let (Some(sa), Some(sb)) = (u.ships.get(a), u.ships.get(b)) else { return false };
    let (Some(ca), Some(cb)) = (sa.base.clamp.as_ref(), sb.base.clamp.as_ref()) else { return false };
    (ca.host.is_some() && ca.host == cb.host) || ca.host == Some(b) || cb.host == Some(a)
}

/// Starts a run at `target` (the tail shared by the attack and special-target routines).
fn start_run(u: &mut Universe, id: ShipId, target: ShipId) {
    set_state(u, id, state::IDLE);
    set_state(u, id, state::TURNING);
    let player = u.ships.ship(id).player;
    let t = u.ships.ship(target);
    let enemy = t.player != player && !matches!(t.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1);
    if let Some(r) = spec_mut(u, id) {
        r.target = Some(target);
        r.enemy_target = enemy;
    }
}

/// `RammingFrigateAttack` (0x5ba5e0): ram the attack target (unless already ramming it or a
/// ship of the same structure).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if u.ships.get(target).is_none() {
        return;
    }
    let cur = spec(u, id).and_then(|r| r.target);
    if cur == Some(target) || same_structure(u, cur, target) {
        return;
    }
    start_run(u, id, target);
}

/// `RammingFrigateSpecialActivate` (0x5ba6a0): drop the target.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    if let Some(r) = spec_mut(u, id) {
        r.target = None;
    }
    set_state(u, id, state::IDLE);
    let s = u.ships.ship_mut(id);
    s.ship_idle = true;
    s.autostabilize_ship = true;
    true
}

/// `RammingFrigateSpecialTarget` (0x5ba6d0): ram the last of the targets that is inside the
/// world. Done (true) when there is none.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let mut pick = None;
    for &t in targets {
        if let Some(s) = u.ships.get(t) {
            if point_in_universe(u, s.posinfo.position) {
                pick = Some(t);
            }
        }
    }
    let Some(t) = pick else { return true };
    if spec(u, id).and_then(|r| r.target) != Some(t) {
        start_run(u, id, t);
    }
    false
}

/// `RammingFrigateRemoveShipReferences` (0x5ba810).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if spec(u, id).and_then(|r| r.target) != Some(gone) {
        return;
    }
    if is_pushing(u, id) {
        set_state(u, id, state::IDLE);
    }
    set_state(u, id, state::IDLE);
    if let Some(r) = spec_mut(u, id) {
        r.target = None;
    }
}

/// `RammingFrigateRemoveTargetRef` (0x5ba870): drops the effect handles (+0x650, +0x674) that
/// point at `gone`; ships are never effects, so nothing changes here.
pub fn remove_target_ref(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

/// `RammingFrigateDied` (0x5bb450).
pub fn died(u: &mut Universe, id: ShipId) {
    crate::effects::delete_slot(u, id, SLOT_CUTTING_BEAM);
    if let Some(r) = spec_mut(u, id) {
        r.beam_on = false;
        r.target = None;
    }
}

/// `RammingFrigateCancelRam` (0x5bb3a0), the move/dock/halt hooks (slots 20, 22 and 27): stop
/// and keep the ship in the order's selection.
pub fn cancel_ram(u: &mut Universe, id: ShipId) -> bool {
    set_state(u, id, state::IDLE);
    true
}

/// `RammingFrigateIsLiveRammer` (0x5bb4a0).
pub fn is_live_rammer(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| is_rammer_type(s.ship_type))
}

/// `RammingFrigateIsRammer` (0x5bb4c0).
pub fn is_rammer(u: &Universe, id: ShipId) -> bool {
    is_live_rammer(u, id)
}

/// `RammingFrigateIsPushing` (0x5bb4f0).
pub fn is_pushing(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|r| r.state == state::PUSHING)
}

/// `RammingFrigateIsRamming` (0x5bc920): a rammer with a run in progress.
pub fn is_ramming(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|r| r.state != state::IDLE)
}

/// `RammingFrigateStopPushing` (0x5bb510): let go of the pushed ship (EMP, hyperspace, ...).
pub fn stop_pushing(u: &mut Universe, id: ShipId) {
    if is_pushing(u, id) {
        set_state(u, id, state::IDLE);
    }
}

/// `RammingFrigateGetRamSpeedBonus` (0x5bca50), the addition to `suGetShipSpeedMult`: an
/// upgraded rammer charges faster.
pub fn ram_speed_bonus(ship: &Ship) -> Ext {
    match spec_of(ship) {
        Some(r) if r.state == state::CHARGING && crate::upgrade::has_upgrade(ship, UPGRADE_RAM) => ld(RAM_SPEED_BONUS),
        _ => 0.0,
    }
}

/// `RammingFrigatePointInUniverse` (0x5bc8a0).
pub fn point_in_universe(u: &Universe, p: Vector) -> bool {
    let b = u.world_bounds;
    let inside = |v: f32, half: f32| -half <= v && v <= half;
    inside(p.z, b.z) && inside(p.y, b.y) && inside(p.x, b.x)
}

/// `RammingFrigateBlendHeading` (0x5ba8a0): turns the ship to the weighted mean of `dir` and
/// its heading at once.
fn blend_heading(u: &mut Universe, id: ShipId, dir: Vector, w_dir: f32, w_heading: f32) {
    let statics = u.statics.clone();
    let s = u.ships.ship_mut(id);
    let h = s.heading();
    let sum = st(ld(w_dir) + ld(w_heading));
    let mix = |hc: f32, dc: f32| st((ld(hc) * ld(w_heading) + ld(w_dir) * ld(dc)) / ld(sum));
    let mut nh = Vector::new(mix(h.x, dir.x), mix(h.y, dir.y), mix(h.z, dir.z));
    nh.normalize();
    s.rotinfo.coordsys = Matrix::coord_sys_from_heading(nh);
    if let Some(stat) = statics.ship(s.ship_type) {
        crate::collision::update_coll_rect(s, stat);
    }
}

/// `RammingFrigateHouseKeep` (0x5baa40).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let now = u.total_time_elapsed;
    let dt = u.phys_time_elapsed;
    let Some(me) = u.ships.get(id) else { return };
    let Some(stat) = statics.ship(me.ship_type) else { return };
    let rs = ram_statics(u, me.ship_type);
    if me.special_flags & SPECIAL_LEAVING != 0 {
        set_state(u, id, state::IDLE);
    }
    let Some(r) = spec(u, id) else { return };
    let target = r.target;
    let (collapsing, collapse) = (r.collapsing, r.collapse_timer);
    if collapsing {
        if collapse.expired(now) {
            if let Some(r) = spec_mut(u, id) {
                r.collapsing = false;
            }
        } else if ld(VENT_DELAY) < ld(now) - ld(collapse.start) {
            vent_effects(u, id);
        }
    }
    let Some(tid) = target else { return };
    let Some(t) = u.ships.get(tid).cloned() else { return };
    let tstat = statics.ship(t.ship_type);
    let me = u.ships.ship(id).clone();
    let tp = t.posinfo.position;
    let mp = me.posinfo.position;
    let dist = st(Vector::distance_ext(mp, tp));
    let mut dir = Vector::new(st(ld(tp.x) - ld(mp.x)), st(ld(tp.y) - ld(mp.y)), st(ld(tp.z) - ld(mp.z)));
    dir.normalize();
    let h = me.heading();
    let facing = (ld(dir.x) * ld(h.x) + ld(dir.y) * ld(h.y)) + ld(dir.z) * ld(h.z);
    let trad = tstat.map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let tclass = tstat.and_then(|s| s.ship_class);
    let small_target = matches!(tclass, Some(ShipClass::Fighter) | Some(ShipClass::Corvette));
    let cur = spec(u, id).map_or(state::IDLE, |r| r.state);
    let mut next = None;
    match cur {
        state::TURNING => {
            let recovered = spec(u, id).is_some_and(|r| r.recover_timer.expired(now));
            if facing <= ld(FACING_TO_CLOSE) || !recovered {
                let s = u.ships.ship_mut(id);
                crate::aitrack::heading(s, stat, &tw, dt, &mut dir, HEADING_ACCURACY, HEADING_ACCURACY, stat.sinbank, 0);
            } else {
                next = Some(state::CLOSING);
            }
        }
        state::CLOSING => {
            let near = ld(trad) + ld(CHARGE_DIST_SMALL) > ld(dist) || (!small_target && ld(trad) + ld(CHARGE_DIST_BIG) > ld(dist));
            if near && spec(u, id).is_some_and(|r| r.sight_timer.expired(now)) {
                if crate::collision::line_of_sight(u, id, Some(tid), mp, dir) {
                    if let Some(r) = spec_mut(u, id) {
                        r.counter = 0;
                    }
                    set_state(u, id, state::CHARGING);
                } else if let Some(r) = spec_mut(u, id) {
                    r.sight_timer.start(now, SIGHT_RETRY);
                }
            }
            u.ships.ship_mut(id).autostabilize_ship = false;
            crate::aiship::fly_to_point_avoiding_objs(u, id, Some(tp), 6, -30.0, Some(t.posinfo.velocity));
            let s = u.ships.ship_mut(id);
            crate::aitrack::heading(s, stat, &tw, dt, &mut dir, HEADING_ACCURACY, HEADING_ACCURACY, stat.sinbank, 0);
        }
        state::CHARGING => {
            // A point far beyond the target along the line to it, used as the desired velocity.
            let aim = Vector::new(st(ld(dir.x) * 99999.0 + ld(tp.x)), st(ld(dir.y) * 99999.0 + ld(tp.y)), st(ld(dir.z) * 99999.0 + ld(tp.z)));
            charge(u, id, &rs, dir, facing);
            let s = u.ships.ship_mut(id);
            crate::aitrack::heading(s, stat, &tw, dt, &mut dir, 0.0, 0.0, 0.0, 0);
            crate::aitrack::velocity_vector(s, stat, &tw, dt, aim);
            if small_target && ld(trad) + ld(ABORT_DIST) < ld(dist) {
                next = Some(state::TURNING);
            }
        }
        state::PUSHING => {
            let expired = spec(u, id).is_some_and(|r| r.push_timer.expired(now));
            if !expired && point_in_universe(u, mp) {
                push(u, id, rs, dt);
            } else {
                next = Some(state::IDLE);
            }
        }
        _ => {}
    }
    if let Some(n) = next {
        set_state(u, id, n);
    }
    if spec(u, id).is_some_and(|r| r.state != state::PUSHING) {
        check_collision(u, id, tid);
    }
    let player = u.ships.ship(id).player;
    if !crate::fow::ship_visible_to_player(u, tid, player) {
        crate::ships::cl_special(u, &[id], &[]);
    }
}

/// The charging part of [`housekeep`]: speed up along the line to the target (or give up when
/// facing away), and swing the heading towards it.
fn charge(u: &mut Universe, id: ShipId, rs: &RammingStatics, dir: Vector, facing: Ext) {
    let now = u.total_time_elapsed;
    let upgraded = has_upgrade(u, id, UPGRADE_RAM);
    {
        let s = u.ships.ship_mut(id);
        s.special_flags2 |= crate::base::special2::DOCKING;
        s.ship_idle = false;
        s.autostabilize_ship = false;
    }
    let k = ld(CHARGE_SPEED[usize::from(upgraded)]);
    let v = u.ships.ship(id).posinfo.velocity;
    let vsq = |v: Vector| (ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z);
    let speed = fsqrt(vsq(v));
    let mut nv = Vector::new(st(speed * k * ld(dir.x)), st(speed * k * ld(dir.y)), st(speed * k * ld(dir.z)));
    let mut cvn = v;
    let mut nvn = nv;
    cvn.normalize();
    nvn.normalize();
    let turning = (ld(nvn.x) * ld(cvn.x) + ld(nvn.y) * ld(cvn.y)) + ld(nvn.z) * ld(cvn.z);
    if ld(FACING_TO_CHARGE) <= facing || ld(FACING_TO_CHARGE) <= turning {
        if !upgraded && ld(CHARGE_TURN_SPEED) * ld(CHARGE_TURN_SPEED) < vsq(v) {
            let mut d = Vector::distance_ext(cvn, nvn);
            if ld(CHARGE_TURN_LOSS) < d {
                d = ld(CHARGE_TURN_LOSS);
            }
            let f = 1.0 - d;
            nv = Vector::new(st(f * ld(nv.x)), st(f * ld(nv.y)), st(f * ld(nv.z)));
        }
        u.ships.ship_mut(id).posinfo.velocity = nv;
        blend_heading(u, id, dir, BLEND_NEW, BLEND_OLD);
    } else {
        set_state(u, id, state::TURNING);
    }
    if upgraded && spec(u, id).is_some_and(|r| r.charge_timer.expired(now)) {
        set_state(u, id, state::TURNING);
        if let Some(r) = spec_mut(u, id) {
            r.recover_timer.start(now, rs.wait_after_push);
        }
    }
}

/// The pushing part of [`housekeep`]: push the target along the rammer's heading, turn it to
/// face the way it was facing, and cut into it.
fn push(u: &mut Universe, id: ShipId, rs: RammingStatics, dt: f32) {
    let upgraded = has_upgrade(u, id, UPGRADE_RAM);
    let k = PUSH_FORCE[usize::from(upgraded)];
    let h = u.ships.ship(id).heading();
    let Some(r) = spec(u, id) else { return };
    let (Some(tid), push_heading) = (r.target, r.push_heading) else { return };
    if let Some(t) = u.ships.get_mut(tid) {
        let f = &mut t.posinfo.force;
        f.x = st(ld(h.x) * ld(k) + ld(f.x));
        f.y = st(ld(st(ld(h.y) * ld(k))) + ld(f.y));
        f.z = st(ld(st(ld(h.z) * ld(k))) + ld(f.z));
    }
    push_rotate(u, tid, push_heading, PUSH_ROTATE_FACING, PUSH_ROTATE_MAX);
    apply_ram_damage(u, id, &rs, dt);
}

/// `RammingFrigatePushRotate` (0x5bc650): torque the pushed ship back towards `heading`
/// (at most `max_torque` per axis). True when it needs no turning.
fn push_rotate(u: &mut Universe, tid: ShipId, heading: Vector, facing_ok: f32, max_torque: f32) -> bool {
    let statics = u.statics.clone();
    let inv_dt = 1.0 / ld(u.phys_time_elapsed);
    let Some(t) = u.ships.get_mut(tid) else { return false };
    if t.rotation_frozen() {
        return true;
    }
    if crate::resource::is_worker(t.ship_type) {
        return false;
    }
    let Some(tstat) = statics.ship(t.ship_type) else { return false };
    let h = t.heading();
    let dot = st((ld(h.x) * ld(heading.x) + ld(h.y) * ld(heading.y)) + ld(h.z) * ld(heading.z));
    if dot <= -1.0 {
        crate::physics::apply_rot(t, max_torque, 1);
        return false;
    }
    if facing_ok <= dot {
        if t.flags & crate::object::sof::CRAZY == 0 {
            t.rotinfo.rotspeed = Vector::ZERO;
        }
        return true;
    }
    let local = t.rotinfo.coordsys.vec_mul(heading);
    let moi = tstat.header.moment_of_inertia;
    if local.y != 0.0 {
        let want = if 0.0 <= local.z {
            -st(ld(local.y) * ld(t.nonstat.turn_speed[0]))
        } else if 0.0 <= local.y {
            -t.nonstat.turn_speed[0]
        } else {
            t.nonstat.turn_speed[0]
        };
        let have = t.rotinfo.rotspeed.x;
        let (mut torque, dir) =
            if want <= have { (st((ld(have) - ld(want)) * ld(moi.x) * inv_dt), 1) } else { (st((ld(want) - ld(have)) * ld(moi.x) * inv_dt), 0) };
        if max_torque < torque {
            torque = max_torque;
        }
        crate::physics::apply_rot(t, torque, dir);
    }
    if local.x != 0.0 {
        let want = st(ld(local.x) * ld(t.nonstat.turn_speed[1]));
        let have = t.rotinfo.rotspeed.y;
        let (torque, dir) =
            if have < want { (st((ld(want) - ld(have)) * ld(moi.y) * inv_dt), 2) } else { (st((ld(have) - ld(want)) * ld(moi.y) * inv_dt), 3) };
        crate::physics::apply_rot(t, if max_torque < torque { max_torque } else { torque }, dir);
    }
    false
}

/// `RammingFrigateApplyRamDamage` (0x5bc550): the cutting beam's damage this frame (enemies
/// only).
fn apply_ram_damage(u: &mut Universe, id: ShipId, rs: &RammingStatics, dt: f32) {
    let Some(r) = spec(u, id) else { return };
    if !r.enemy_target {
        return;
    }
    let Some(tid) = r.target else { return };
    let beam_on = r.beam_on;
    if u.effects.is_some() {
        match fx::handle(u, id, SLOT_CUTTING_BEAM) {
            None => cutting_beam_effect(u, id, tid),
            Some(e) => {
                let at = fx::local_to_world(u, id, CUTTING_BEAM_POINT);
                let cs = u.ships.ship(id).rotinfo.coordsys;
                fx::place(u, e, Some(at), None, Some(cs));
            }
        }
    }
    if !beam_on {
        if let Some(r) = spec_mut(u, id) {
            r.beam_on = true;
        }
        effect(u, id, Effect::Beam { target: tid });
    }
    let per_sec = if has_upgrade(u, id, UPGRADE_CUTTING_BEAM) { rs.cutting_damage_upgraded } else { rs.cutting_damage };
    let player = u.ships.ship(id).player as i32;
    crate::damage::apply_damage_to_target(u, tid, st(ld(dt) * ld(per_sec)), 0, crate::damage::how::KILLED_BY_PLAYER, player, Some(id));
}

/// `RammingFrigateImpactEffect` (0x5bc3b0): the cutting beam, from the bow along the heading
/// to where it meets the target (no beam when it does not, or starts inside it). Its last
/// parameter picks the upgraded beam's look (Somtaaw 2, Beast 1).
fn cutting_beam_effect(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(effect) = fx::special_first(u, CUTTING_BEAM_EFFECT) else { return };
    if !fx::fire_enabled(u) || !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let me = u.ships.ship(id);
    let cs = me.rotinfo.coordsys;
    let h = me.heading();
    let start = fx::local_to_world(u, id, CUTTING_BEAM_POINT);
    let Some(t) = u.ships.get(target) else { return };
    let Some(tstat) = u.statics.ship(t.ship_type) else { return };
    let (hit_t, _) = crate::collision::rect_line(t, tstat, start, h, CUTTING_BEAM_REACH);
    if hit_t < 0.0 {
        return;
    }
    let hit = start.add_scaled(h, hit_t);
    let dist = st(Vector::distance_ext(start, hit));
    if dist == 0.0 {
        return;
    }
    let look = match me.ship_type {
        _ if !crate::upgrade::has_upgrade(me, UPGRADE_CUTTING_BEAM) => 0,
        ShipType::sRammingFrigate => 2,
        ShipType::bRammingFrigate => 1,
        _ => 0,
    };
    let params = [0, 0, crate::effects::pf(dist), look];
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    let e = crate::effects::create(u, effect, None, Some(start), Some(Vector::ZERO), Some(cs), 1.0, flags, &params);
    crate::effects::set_slot(u, id, SLOT_CUTTING_BEAM, e);
}

/// `RammingFrigateRestartEffect` (0x5ba410): the impact when the rammer latches on, riding
/// with it.
fn restart_effect(u: &mut Universe, id: ShipId) {
    let Some(effect) = fx::special(u, IMPACT_EFFECT) else { return };
    if !fx::hit_enabled(u) || !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    crate::effects::create(u, effect, Some(id.into()), None, None, None, 1.0, flags, &[]);
}

/// The vents of `RammingFrigateHouseKeep` while the bow collapses: one from the starboard top
/// corner pointing to port, and the pushes (`RammingFrigatePushEffect`, 0x5bc260) from the other
/// three corners, all moving with the rammer.
fn vent_effects(u: &mut Universe, id: ShipId) {
    if u.effects.is_none() {
        return;
    }
    let [p2, p3, p4, p5] = VENT_POINTS.map(|p| fx::local_to_world(u, id, p));
    let mut across = Vector::new(st(ld(p4.x) - ld(p2.x)), st(ld(p4.y) - ld(p2.y)), st(ld(p4.z) - ld(p2.z)));
    across.normalize();
    let back = Vector::new(-across.x, -across.y, -across.z);
    vent(u, id, p2, across);
    vent(u, id, p3, across);
    vent(u, id, p4, back);
    vent(u, id, p5, back);
}

/// One vent at `at` blowing along `dir` (the level of detail is its parameter).
fn vent(u: &mut Universe, id: ShipId, at: Vector, dir: Vector) {
    let Some(effect) = fx::special_first(u, VENT_EFFECT) else { return };
    if !fx::hit_enabled(u) || !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let lod = fx::lod(u);
    if lod >= VENT_MAX_LOD {
        return;
    }
    let vel = u.ships.ship(id).posinfo.velocity;
    let cs = Matrix::coord_sys_from_heading(dir);
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    crate::effects::create(u, effect, None, Some(at), Some(vel), Some(cs), 1.0, flags, &[u32::from(lod)]);
}

/// `RammingFrigateCheckCollision` (0x5bc950): the rammer's own contact test with its target
/// every frame it is not pushing.
fn check_collision(u: &mut Universe, a: ShipId, b: ShipId) {
    let statics = u.statics.clone();
    let (Some(sa), Some(sb)) = (u.ships.get(a), u.ships.get(b)) else { return };
    let (Some(asta), Some(bsta)) = (statics.ship(sa.ship_type), statics.ship(sb.ship_type)) else { return };
    let sum = st(ld(asta.header.coll.coll_sphere_size) + ld(bsta.header.coll.coll_sphere_size));
    let (ca, cb) = (sa.coll.coll_position, sb.coll.coll_position);
    let d = Vector::new(st(ld(cb.x) - ld(ca.x)), st(ld(cb.y) - ld(ca.y)), st(ld(cb.z) - ld(ca.z)));
    let dsq = (ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z);
    if !(dsq < ld(sum) * ld(sum)) {
        return;
    }
    let dist = st(Vector::distance_ext(ca, cb));
    if !crate::collision::rect_bump_pub(sa, asta, sb, bsta) {
        return;
    }
    collide(u, a, b);
    if is_rammer(u, b) {
        crate::collision::objects_collided(u, a, b, sum, d, dist);
    }
}

/// `RammingFrigateCollide` (0x5bbed0), called for every bump between two ships: two rammers
/// bounce off each other; a charging rammer that hits something tries to latch on
/// ([`check_bump`]) or, ramming a Beast ship it is not protected against, gets infected.
pub fn collide(u: &mut Universe, a: ShipId, b: ShipId) {
    let (Some(ta), Some(tb)) = (u.ships.get(a).map(|s| s.ship_type), u.ships.get(b).map(|s| s.ship_type)) else { return };
    let (ra, rb) = (is_rammer_type(ta), is_rammer_type(tb));
    if ra && rb {
        stop_pushing(u, a);
        stop_pushing(u, b);
        reset_ram_timer(u, a);
        reset_ram_timer(u, b);
        set_state(u, a, state::TURNING);
        set_state(u, b, state::TURNING);
        return;
    }
    let (r, o) = if rb {
        (b, a)
    } else if ra {
        (a, b)
    } else {
        return;
    };
    if spec(u, r).is_none_or(|s| s.state != state::CHARGING) {
        return;
    }
    let (tr, to) = (u.ships.ship(r).ship_type, u.ships.ship(o).ship_type);
    if !is_beast_type(tr) && !is_immune_type(tr) {
        let (pr, po) = (u.ships.ship(r).player, u.ships.ship(o).player);
        if (is_beast_type(to) || is_immune_type(to)) && pr != po && !has_upgrade(u, r, UPGRADE_INFECTION_IMMUNE) && target_is_immune(u, o) {
            // The rammer catches the Beast infection.
            crate::ships::beast::infect_ship(u, r, o);
            set_state(u, r, state::IDLE);
            return;
        }
    }
    check_bump(u, r, o);
}

/// `RammingFrigateCheckBump` (0x5bba40): the charge hit `bumped`. Latch onto it and start
/// pushing when it (or the structure it belongs to) is the target and nothing is in the way;
/// otherwise back off.
fn check_bump(u: &mut Universe, r: ShipId, bumped: ShipId) {
    let now = u.total_time_elapsed;
    let root = crate::clamp::host_of(u, bumped).unwrap_or(bumped);
    let mut ok = !is_rammer(u, root);
    if crate::clamp::host_of(u, root).is_some() {
        ok = false;
    }
    let cur = spec(u, r).and_then(|s| s.target);
    if cur != Some(root) && cur.and_then(|c| crate::clamp::host_of(u, c)) != Some(root) {
        ok = false;
    }
    if is_pushing(u, r) {
        ok = false;
    }
    let back_off = |u: &mut Universe| {
        reset_ram_timer(u, r);
        set_state(u, r, state::TURNING);
    };
    let rpos = u.ships.ship(r).posinfo.position;
    if !point_in_universe(u, rpos) || !ok {
        back_off(u);
        return;
    }
    // Another rammer already on the target and touching us: both back off.
    let has_clamp_info = |u: &Universe, id: ShipId| u.ships.get(id).is_some_and(|s| s.base.clamp.is_some());
    if has_clamp_info(u, root) {
        let mut touching = false;
        let statics = u.statics.clone();
        for other in crate::clamp::slaves_of(u, root) {
            let (Some(so), Some(sr)) = (u.ships.get(other), u.ships.get(r)) else { continue };
            if !is_rammer_type(so.ship_type) {
                continue;
            }
            let (Some(ost), Some(rst)) = (statics.ship(so.ship_type), statics.ship(sr.ship_type)) else { continue };
            if crate::collision::rect_bump_pub(sr, rst, so, ost) {
                if is_pushing(u, other) {
                    set_state(u, other, state::IDLE);
                }
                touching = true;
            }
        }
        if touching {
            back_off(u);
            return;
        }
    }
    if cur.is_some_and(|c| is_rammer(u, c)) {
        back_off(u);
        return;
    }
    // Leeches on the rammer are shaken off; anything else clamped to it stops the ram.
    let own = crate::clamp::slaves_of(u, r);
    if has_clamp_info(u, r) && !own.is_empty() {
        let mut free = true;
        for &s in own.iter().rev() {
            if u.ships.get(s).is_some_and(|x| x.ship_type == ShipType::sLeech) {
                crate::command::remove_ships_from_doing_stuff(u, &[s]);
            } else {
                free = false;
            }
        }
        if !free {
            back_off(u);
            return;
        }
    }
    // (The single-player cinematic check - no ramming while the wide-screen bars are in - has
    // no counterpart: the universe knows no cinematics.)
    let bp = u.ships.ship(bumped).posinfo.position;
    let mut dir = Vector::new(st(ld(bp.x) - ld(rpos.x)), st(ld(bp.y) - ld(rpos.y)), st(ld(bp.z) - ld(rpos.z)));
    dir.normalize();
    blend_heading(u, r, dir, 1.0, 0.0);
    let rs = ram_statics(u, u.ships.ship(r).ship_type);
    let (found, hit) = find_snap_point(u, r, bumped);
    if found {
        let player = u.ships.ship(r).player as i32;
        if let Some(h) = hit {
            let how = crate::damage::how::KILLED_BY_PLAYER;
            crate::damage::apply_damage_to_target(u, h, rs.ram_damage, 0, how, player, Some(r));
            let hs = u.ships.ship(h);
            let class = crate::base::class_of(u, hs.ship_type);
            let fragile = matches!(class, Some(ShipClass::Micro) | Some(ShipClass::Fighter))
                || crate::ships::acv::is_acv(hs.ship_type)
                || matches!(hs.ship_type, ShipType::sMCV | ShipType::bHeavyCorvette);
            if fragile && 0.0 <= hs.health {
                let finish = st(ld(hs.health) + 1.0);
                crate::damage::apply_damage_to_target(u, h, finish, 0, how, player, Some(r));
            }
        }
        if hit.and_then(|h| u.ships.get(h)).is_some_and(|h| 0.0 <= h.health) {
            if let Some(s) = spec_mut(u, r) {
                s.target = Some(root);
            }
            crate::clamp::clamp_obj_to_obj(u, r, root);
            if let Some(s) = spec_mut(u, r) {
                s.push_timer.start(now, rs.push_time);
            }
            set_state(u, r, state::PUSHING);
            let rh = u.ships.ship(root).heading();
            if let Some(s) = spec_mut(u, r) {
                s.push_heading = rh;
            }
            restart_effect(u, r);
            return;
        }
    } else if let Some(h) = hit {
        if is_rammer(u, h) && is_pushing(u, h) {
            set_state(u, h, state::IDLE);
        }
    }
    reset_ram_timer(u, r);
    set_state(u, r, state::TURNING);
}

/// `RammingFrigateRayTest` (0x5bb550): where the line through `point` along `dir` (500 m each
/// way) meets `ship`, as an offset from `point`. The original tests the ship's mesh; this uses
/// its collision box.
fn ray_test(u: &Universe, ship: ShipId, dir: Vector, point: Vector) -> Option<Vector> {
    let s = u.ships.get(ship)?;
    let stat = u.statics.ship(s.ship_type)?;
    let a = ld(SNAP_RAY);
    let start = Vector::new(st(ld(point.x) - a * ld(dir.x)), st(ld(point.y) - ld(st(a * ld(dir.y)))), st(ld(point.z) - ld(st(a * ld(dir.z)))));
    let (t, _) = crate::collision::rect_line(s, stat, start, dir, st(2.0 * a));
    if t < 0.0 {
        return None;
    }
    let hit = start.add_scaled(dir, t);
    Some(Vector::new(st(ld(hit.x) - ld(point.x)), st(ld(hit.y) - ld(point.y)), st(ld(hit.z) - ld(point.z))))
}

/// The best snap so far (`RammingFrigateTraceSnapPoint`'s in/out parameters).
struct Snap {
    found: bool,
    dist: f32,
    position: Vector,
    hit: Option<ShipId>,
}

/// `RammingFrigateTraceSnapPoint` (0x5bb630): try every snap point against `ship`; keep the
/// rammer position nearest to `behind`.
fn trace_snap_points(u: &Universe, r: &Ship, ship: ShipId, points: &[Vector], behind: Vector, best: &mut Snap) {
    let h = r.heading();
    let rp = r.posinfo.position;
    for &lp in points {
        let off = r.rotinfo.coordsys.mul_vec(lp);
        let world = Vector::new(st(ld(off.x) + ld(rp.x)), st(ld(off.y) + ld(rp.y)), st(ld(off.z) + ld(rp.z)));
        let Some(o) = ray_test(u, ship, h, world) else { continue };
        let cand = Vector::new(st(ld(o.x) + ld(rp.x)), st(ld(o.y) + ld(rp.y)), st(ld(o.z) + ld(rp.z)));
        if !best.found {
            best.found = true;
            best.position = cand;
            best.hit = Some(ship);
            best.dist = st(Vector::distance_ext(cand, behind));
        } else {
            let d = Vector::distance_ext(cand, behind);
            if d < ld(best.dist) {
                best.dist = st(d);
                best.position = cand;
                best.hit = Some(ship);
            }
        }
    }
}

/// `RammingFrigateFindSnapPoint` (0x5bb760): move the rammer so that one of its snap points
/// sits on the hull of `bumped` (or of the structure it belongs to: its host and the ships
/// clamped to it, except leeches and sentinels). Returns whether it latched and the ship hit;
/// ships under construction and other rammers cannot be latched onto.
fn find_snap_point(u: &mut Universe, r: ShipId, bumped: ShipId) -> (bool, Option<ShipId>) {
    let statics = u.statics.clone();
    let me = u.ships.ship(r).clone();
    let Some(stat) = statics.ship(me.ship_type) else { return (false, None) };
    let h = me.heading();
    let p = me.posinfo.position;
    let b = ld(SNAP_BEHIND);
    let behind = Vector::new(st(ld(h.x) * b + ld(p.x)), st(ld(st(ld(h.y) * b)) + ld(p.y)), st(ld(st(ld(h.z) * b)) + ld(p.z)));
    let points: Vec<Vector> = stat
        .dock
        .as_ref()
        .map(|d| SNAP_POINTS.iter().filter_map(|n| d.points.iter().find(|pt| pt.name.eq_ignore_ascii_case(n)).map(|pt| pt.position)).collect())
        .unwrap_or_default();
    let mut best = Snap { found: false, dist: 0.0, position: Vector::ZERO, hit: None };
    trace_snap_points(u, &me, bumped, &points, behind, &mut best);
    if u.ships.get(bumped).is_some_and(|s| s.base.clamp.is_some()) {
        let mut structure = bumped;
        if let Some(host) = crate::clamp::host_of(u, bumped) {
            if host != bumped {
                trace_snap_points(u, &me, host, &points, behind, &mut best);
                structure = host;
            }
        }
        let mut i = 0;
        loop {
            let slaves = crate::clamp::slaves_of(u, structure);
            let Some(&s) = slaves.get(i) else { break };
            i += 1;
            let t = u.ships.get(s).map(|x| x.ship_type);
            if !matches!(t, Some(ShipType::sLeech) | Some(ShipType::sSentinel) | Some(ShipType::bSentinel)) {
                trace_snap_points(u, &me, s, &points, behind, &mut best);
            }
        }
    }
    if best.found {
        // `univSetShipPosition`: the fog-of-war viewer moves with the ship.
        crate::base::set_ship_position(u, r, best.position);
    }
    let mut found = best.found;
    if let Some(hs) = best.hit.and_then(|h| u.ships.get(h)) {
        if found && (hs.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0 || is_rammer_type(hs.ship_type)) {
            found = false;
        }
    }
    (found, best.hit)
}

/// `univRemoveResourceRefsFromRammingFrigates` (0x544250): a dying resource is dropped by every
/// rammer targeting it. Rammers only ever target ships here, so this is
/// [`remove_ship_refs`] on every rammer.
pub fn remove_refs_from_rammers(u: &mut Universe, gone: ShipId) {
    for id in u.ship_list.clone() {
        if is_rammer(u, id) {
            remove_ship_refs(u, id, gone);
        }
    }
}
