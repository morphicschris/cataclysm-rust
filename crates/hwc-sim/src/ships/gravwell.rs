//! Gravity well generators (`GravWellGenerator` module, 0x5aeab0..0x5af7f0): kGravWellGenerator,
//! tGravWellGenerator.
//!
//! The special switches the well on or off. While it is on, every `scanrate`-th frame (the
//! update counter masked by `scanrate`) it catches the strike craft within `GravWellRadius`
//! (fighters, corvettes, micro craft and workers; not salvage corvettes) whatever their owner:
//! a caught ship cannot fire (`specialFlags` 0x10000000) and its engines stop
//! (`dontapplyforceever`); the well brakes it (velocity times `EffectConstant` per frame) until
//! it is slow, then sets it tumbling (random spins up to `xrot/yrot/zrot`) and pushes it out
//! with `repulseForce` until it leaves the radius. A well that has used up its
//! `OperationTime` collapses: it lets everything go, turns derelict and blows up 3.5 seconds
//! later (only ownerless wells use up time in Cataclysm).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::random::{RAN_ETG_CAT, RAN_GAME};
use hwc_math::Vector;

use crate::object::{sof, special, ShipId};
use crate::statics::{ShipClass, ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Seconds between a well's collapse and its explosion.
const TIME_BEFORE_EXPLODE: f32 = 3.5;
/// Squared speed under which a caught ship stops braking and starts tumbling.
const STOPPED_SPEED_SQ: f32 = 2500.0;
/// A caught ship is set spinning about an axis only while its spin about it is below this.
const SPIN_LIMIT: f32 = 0.7;

/// A ship the well holds (`GravStruct`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Held {
    pub ship: ShipId,
    /// 0: braking, 1: tumbling and pushed out.
    pub state: u32,
    /// The spins it is given.
    pub spin: Vector,
}

/// Per-ship state (+0x638..+0x65c).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct GravWellSpec {
    /// +0x638: the field is on.
    pub field_on: bool,
    /// +0x63c: the well has been fired (nothing sets it in Cataclysm).
    pub fired: bool,
    /// +0x640: the ships held.
    pub held: Vec<Held>,
    /// +0x64c: seconds the field has been on.
    pub time_on: f32,
    /// +0x654: ready.
    pub ready: bool,
    /// +0x658: the well has collapsed.
    pub derelict: bool,
    /// When each caught ship may show its caught effect again (the caught ship's +0x328).
    pub caught_effect_time: CaughtEffectTimes,
}

/// The caught ships' effect timers ([`GravWellSpec::caught_effect_time`]). Effect state: set
/// only with effects, left out of comparisons and of the debug output (and so of the world
/// hash).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CaughtEffectTimes(pub Vec<(ShipId, f32)>);

impl CaughtEffectTimes {
    fn get(&self, ship: ShipId) -> f32 {
        self.0.iter().find(|(s, _)| *s == ship).map_or(0.0, |&(_, t)| t)
    }

    fn set(&mut self, ship: ShipId, t: f32) {
        match self.0.iter_mut().find(|(s, _)| *s == ship) {
            Some(e) => e.1 = t,
            None => self.0.push((ship, t)),
        }
    }
}

impl PartialEq for CaughtEffectTimes {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for CaughtEffectTimes {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("..")
    }
}

/// Per-type statics (`GravWellGeneratorStaticInit` 0x5aeab0).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct GravWellStatics {
    pub radius: f32,
    pub radius_sq: f32,
    pub operation_time: f32,
    pub effect_constant: f32,
    pub scan_rate: u32,
    pub repulse_force: f32,
    pub warmup_down_time: f32,
    pub rot: Vector,
}

impl GravWellStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        use crate::statics::cscan::{scan_f32, scan_i32};
        let mut s = Self::default();
        for (k, v) in &stat.custom_entries {
            let f = scan_f32(v);
            match k.as_str() {
                "GravWellRadius" => s.radius = f.unwrap_or(s.radius),
                "OperationTime" => s.operation_time = f.unwrap_or(s.operation_time),
                "EffectConstant" => s.effect_constant = f.unwrap_or(s.effect_constant),
                "scanrate" => s.scan_rate = scan_i32(v).map_or(s.scan_rate, |x| x as u32),
                "repulseForce" => s.repulse_force = f.unwrap_or(s.repulse_force),
                "warmupdowntime" => s.warmup_down_time = f.unwrap_or(s.warmup_down_time),
                "xrot" => s.rot.x = f.unwrap_or(s.rot.x),
                "yrot" => s.rot.y = f.unwrap_or(s.rot.y),
                "zrot" => s.rot.z = f.unwrap_or(s.rot.z),
                _ => {}
            }
        }
        s.radius_sq = st(ld(s.radius) * ld(s.radius));
        s
    }
}

fn statics(u: &Universe, t: ShipType) -> GravWellStatics {
    match super::type_statics(u, t) {
        super::TypeStatics::GravWell(g) => g.clone(),
        _ => GravWellStatics::default(),
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&GravWellSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::GravWell(g) => Some(g),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut GravWellSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::GravWell(g) => Some(g),
        _ => None,
    }
}

/// Whether a type is a gravity well generator.
pub fn is_grav_well(t: ShipType) -> bool {
    matches!(t, ShipType::kGravWellGenerator | ShipType::tGravWellGenerator)
}

/// Whether a well holds `ship`.
pub fn holds(u: &Universe, well: ShipId, ship: ShipId) -> bool {
    spec(u, well).is_some_and(|g| g.held.iter().any(|h| h.ship == ship))
}

/// `GravWellGeneratorInit` (0x5aeb00).
pub fn init(u: &mut Universe, id: ShipId) {
    let g = GravWellSpec { ready: true, ..Default::default() };
    u.ships.ship_mut(id).spec = ShipSpec::GravWell(g);
}

/// A held ship goes free: its engines, rotation and guns work again.
fn release(u: &mut Universe, ship: ShipId) {
    if let Some(s) = u.ships.get_mut(ship) {
        s.dont_apply_force_ever &= !1;
        s.dont_rotate_ever = false;
        s.special_flags &= !special::CANT_FIRE;
    }
}

fn release_all(u: &mut Universe, id: ShipId) {
    let held = spec_mut(u, id).map(|g| std::mem::take(&mut g.held)).unwrap_or_default();
    for h in held {
        release(u, h.ship);
    }
}

/// `GravWellClose` (0x5af1d0).
pub fn close(u: &mut Universe, id: ShipId) {
    if let Some(g) = spec_mut(u, id) {
        g.held.clear();
    }
}

/// `GravWellAttack` (0x5af7f0).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    crate::attack::attack_passive(u, id, target, 4000.0, 3000.0);
}

/// `turnoffGravwell` (0x5aed00): the well lets everything go, its field effect is left to
/// finish by itself and the switch-off effect plays.
pub fn turn_off(u: &mut Universe, id: ShipId) {
    if let Some(g) = spec_mut(u, id) {
        g.field_on = false;
    }
    release_all(u, id);
    let_field_effect_go(u, id);
    field_effect(u, id, OFF_EFFECT_SLOT);
}

/// `EGT_GRAVWELL_ON`, `EGT_GRAVWELL_OFF` and `EGT_CAUGHT_GRAVWELL` in the special-purpose
/// effect table.
const ON_EFFECT_SLOT: usize = 8;
const OFF_EFFECT_SLOT: usize = 9;
const CAUGHT_EFFECT_SLOT: usize = 19;
/// The well's field effect (+0x65c) among the per-ship effect slots.
const FIELD_EFFECT: u32 = 100;
/// The second variable of a field effect that runs until it is told to stop.
const RUNS_UNTIL_STOPPED: f32 = 999_999.0;
/// The range of seconds after which a caught ship shows its caught effect again (0x8ba578).
const CAUGHT_EFFECT_INTERVAL: [f32; 2] = [3.0, 5.0];

/// The well's switch-on or switch-off effect: on the well, attached to it in every respect,
/// with the well's race as parameter. Returns it.
fn field_effect(u: &mut Universe, id: ShipId, slot: usize) -> Option<hwc_fx::runtime::EffectId> {
    let fx = u.effects.as_ref()?;
    let effect = fx.pick(fx.tables.special_effect(slot))?;
    if !crate::effects::in_render_list(u, id.into()) {
        return None;
    }
    let s = u.ships.ship(id);
    let (pos, cs) = (s.posinfo.position, s.rotinfo.coordsys);
    let race = u.statics.ship(s.ship_type).map_or(0, |st| st.ship_race.index() as u32);
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    crate::effects::create(u, effect, Some(id.into()), Some(pos), None, Some(cs), 1.0, flags, &[race])
}

/// The switch-on effect, kept as the well's field effect while it runs until stopped.
fn start_field_effect(u: &mut Universe, id: ShipId) {
    let Some(e) = field_effect(u, id, ON_EFFECT_SLOT) else { return };
    let lasting = u.effects.as_ref().and_then(|fx| fx.world.effect(e)).and_then(|ef| ef.variables.get(1).copied()).is_some_and(|v| f32::from_bits(v) == RUNS_UNTIL_STOPPED);
    crate::effects::set_slot(u, id, FIELD_EFFECT, lasting.then_some(e));
}

/// The well's field effect loses its owner and is told how long it ran (its second
/// variable), so that it winds down by itself.
fn let_field_effect_go(u: &mut Universe, id: ShipId) {
    let Some(e) = crate::effects::slot(u, id, FIELD_EFFECT) else { return };
    crate::effects::set_slot(u, id, FIELD_EFFECT, None);
    let Some(fx) = u.effects.as_mut() else { return };
    if let Some(ef) = fx.world.effects.iter_mut().find(|x| x.id == e) {
        ef.owner = None;
        let age = ef.time_elapsed.to_bits();
        if let Some(v) = ef.variables.get_mut(1) {
            *v = age;
        }
    }
}

/// `runShipEffect` (0x5aeb40): a caught ship's effect, on the ship and attached to it in
/// every respect, with its bounding-sphere radius as parameter; the ship shows it again 3 to
/// 5 seconds later (drawn from the effect stream).
fn caught_effect(u: &mut Universe, well: ShipId, ship: ShipId) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(effect) = fx.pick(fx.tables.special_effect(CAUGHT_EFFECT_SLOT)) else { return };
    if !crate::effects::in_render_list(u, ship.into()) || !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let s = u.ships.ship(ship);
    let (pos, vel, cs) = (s.posinfo.position, s.posinfo.velocity, s.rotinfo.coordsys);
    let radius = u.statics.ship(s.ship_type).map_or(0.0, |st| st.header.coll.coll_sphere_size);
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    crate::effects::create(u, effect, Some(ship.into()), Some(pos), Some(vel), Some(cs), 1.0, flags, &[crate::effects::pf(radius)]);
    let [lo, hi] = CAUGHT_EFFECT_INTERVAL;
    let delay = if hi <= lo {
        lo
    } else {
        let r = u.effects.as_mut().expect("effects").world.randy.random(RAN_ETG_CAT);
        let x = st((ld(hi) - ld(lo)) * f64::from(r) * ld(f32::from_bits(0x2f80_0000)) + ld(lo));
        if x < lo {
            lo
        } else {
            x.min(hi)
        }
    };
    let at = st(ld(delay) + ld(u.total_time_elapsed));
    if let Some(g) = spec_mut(u, well) {
        g.caught_effect_time.set(ship, at);
    }
}

/// `GravWellGeneratorSpecialActivate` (0x5aee40): the special toggles the field (it can only
/// come on while operating time is left).
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let st_ = statics(u, u.ships.ship(id).ship_type);
    let Some(g) = spec_mut(u, id) else { return true };
    if g.fired {
        return true;
    }
    g.ready = false;
    if g.field_on {
        turn_off(u, id);
        return true;
    }
    if g.time_on < st_.operation_time {
        g.field_on = true;
        // (The opening animation and speech are presentation.)
        start_field_effect(u, id);
    }
    true
}

/// `GravWellGeneratorJustDisabled` (0x5aef60): a disabled well switches off.
pub fn just_disabled(u: &mut Universe, id: ShipId) {
    if spec(u, id).is_some_and(|g| g.field_on) {
        special_activate(u, id);
    }
}

/// `GravWellGeneratorAddObj` (0x5aef80): catch a ship (a ship already held only has its guns
/// locked again). The tumble spins are drawn now.
pub fn add_obj(u: &mut Universe, id: ShipId, ship: ShipId) {
    let Some(s) = u.ships.get(ship) else { return };
    if s.flags & sof::DEAD != 0 {
        return;
    }
    if holds(u, id, ship) {
        u.ships.ship_mut(ship).special_flags |= special::CANT_FIRE;
        return;
    }
    let rot = statics(u, u.ships.ship(id).ship_type).rot;
    let mut r = |m: f32| st(u.randy.frandom_between_ext(RAN_GAME, -m, m));
    let spin = Vector::new(r(rot.x), r(rot.y), r(rot.z));
    if let Some(g) = spec_mut(u, id) {
        g.held.push(Held { ship, state: 0, spin });
    }
    u.ships.ship_mut(ship).special_flags |= special::CANT_FIRE;
}

/// `GravAddObjectsInProximity` (0x5af0d0).
pub fn add_objects_in_proximity(u: &mut Universe, id: ShipId) {
    let r2 = statics(u, u.ships.ship(id).ship_type).radius_sq;
    let me = u.ships.ship(id).posinfo.position;
    let list = u.ship_list.clone();
    for sid in list {
        let Some(s) = u.ships.get(sid) else { continue };
        let class = u.statics.ship(s.ship_type).and_then(|st| st.ship_class);
        let caught = matches!(class, Some(ShipClass::Corvette | ShipClass::Fighter | ShipClass::Micro)) || s.ship_type == ShipType::sWorker;
        if !caught || matches!(s.ship_type, ShipType::kSalCapCorvette | ShipType::tSalCapCorvette) {
            continue;
        }
        let p = s.posinfo.position;
        let (dx, dy, dz) = (st(ld(p.x) - ld(me.x)), st(ld(p.y) - ld(me.y)), st(ld(p.z) - ld(me.z)));
        if ((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz)) <= ld(r2) {
            add_obj(u, id, sid);
        }
    }
}

/// `GravWellGeneratorHouseKeep` (0x5af1f0).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(g) = spec(u, id) else { return };
    if g.derelict || !g.field_on {
        return;
    }
    let now = u.total_time_elapsed;
    let st_ = statics(u, u.ships.ship(id).ship_type);
    {
        let g = spec_mut(u, id).expect("spec");
        g.ready = true;
        // Only a well without an owner uses up its operating time (every ship here has one).
    }
    if spec(u, id).is_some_and(|g| g.time_on < st_.operation_time) {
        if st_.scan_rate & u.update_counter == st_.scan_rate {
            if crate::effects::slot(u, id, FIELD_EFFECT).is_none() {
                start_field_effect(u, id);
            }
            add_objects_in_proximity(u, id);
        }
    } else {
        release_all(u, id);
        let_field_effect_go(u, id);
        let g = spec_mut(u, id).expect("spec");
        g.field_on = false;
        g.derelict = true;
        let s = u.ships.ship_mut(id);
        s.death_time = st(ld(now) + ld(TIME_BEFORE_EXPLODE));
        s.flags = (s.flags & !sof::SELECTABLE) | sof::DISABLED;
    }
    let me = u.ships.ship(id).posinfo.position;
    let held = spec(u, id).map(|g| g.held.clone()).unwrap_or_default();
    for h in held {
        let Some(s) = u.ships.get(h.ship) else { continue };
        let p = s.posinfo.position;
        let (dx, dy, dz) = (st(ld(p.x) - ld(me.x)), st(ld(p.y) - ld(me.y)), st(ld(p.z) - ld(me.z)));
        let dsq = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
        if !(dsq < st_.radius_sq) || s.flags & sof::DEAD != 0 {
            remove_ship_refs(u, id, h.ship);
            continue;
        }
        if u.effects.is_some() && spec(u, id).is_some_and(|g| g.caught_effect_time.get(h.ship) < now) {
            caught_effect(u, id, h.ship);
        }
        let s = u.ships.ship_mut(h.ship);
        s.dont_apply_force_ever |= 1;
        match h.state {
            0 => {
                let k = ld(st_.effect_constant);
                let v = &mut s.posinfo.velocity;
                v.x = st(k * ld(v.x));
                v.y = st(k * ld(v.y));
                v.z = st(k * ld(v.z));
                let slow = (ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z) < ld(STOPPED_SPEED_SQ);
                if slow {
                    if let Some(x) = spec_mut(u, id).and_then(|g| g.held.iter_mut().find(|x| x.ship == h.ship)) {
                        x.state = 1;
                    }
                }
            }
            _ => {
                s.autostabilize_ship = false;
                let t = s.rotinfo.torque;
                if t.x == 0.0 && t.y == 0.0 && t.z == 0.0 {
                    let rs = s.rotinfo.rotspeed;
                    let within = |x: f32| x < SPIN_LIMIT && -SPIN_LIMIT < x;
                    if within(rs.x) {
                        s.rotinfo.torque.x = h.spin.x;
                    }
                    if within(rs.y) {
                        s.rotinfo.torque.y = h.spin.y;
                    }
                    if within(rs.z) {
                        s.rotinfo.torque.z = h.spin.z;
                    }
                }
                let inv = 1.0 / fsqrt(ld(dsq));
                let k = ld(st_.repulse_force);
                s.posinfo.force = Vector::new(st(k * ld(st(inv * ld(dx)))), st(k * ld(st(ld(dy) * inv))), st(k * inv * ld(dz)));
            }
        }
    }
}

/// `GravWellGeneratorRemoveShipReferences` (0x5af170): a held ship goes free.
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(g) = spec_mut(u, id) else { return };
    let Some(i) = g.held.iter().position(|h| h.ship == gone) else { return };
    g.held.remove(i);
    release(u, gone);
}

/// `GravWellGeneratorDied` (0x5aebf0).
pub fn died(u: &mut Universe, id: ShipId) {
    release_all(u, id);
}

/// `gravwellIsShipStuckForHyperspaceing` (0x5aec70): a gravity well with its field on, in the
/// ship's collision blob, has the ship inside its radius (it cannot jump).
pub fn is_ship_stuck_for_hyperspace(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let Some(b) = s.coll_my_blob else { return false };
    let Some(blob) = u.blobs.blobs.get(b.0 as usize) else { return false };
    let p = s.posinfo.position;
    blob.ships.iter().any(|&w| {
        let Some(ws) = u.ships.get(w) else { return false };
        if !is_grav_well(ws.ship_type) || !spec(u, w).is_some_and(|g| g.field_on) {
            return false;
        }
        let q = ws.posinfo.position;
        let (dx, dy, dz) = (st(ld(p.x) - ld(q.x)), st(ld(p.y) - ld(q.y)), st(ld(p.z) - ld(q.z)));
        ((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz)) < ld(statics(u, ws.ship_type).radius_sq)
    })
}

/// The well has collapsed (`GravDerelict`, +0x658): its self-destruct timer may blow it up.
pub fn is_derelict(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|g| g.derelict)
}
