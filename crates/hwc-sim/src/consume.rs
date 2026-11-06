//! `consume*` (0x45b7b0..0x45c200): a ship being eaten by a beam. A depot does this to the crystal
//! it processes. The beam comes from a point on the depot, sparks fly from where it bites, and a
//! clip plane sweeps the hull away over the time given (the renderer asks for it with
//! [`clip_plane`]). The RKO uses the same code in the campaign.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_PARTICLE_STREAM;
use hwc_math::{Matrix, Vector};

use crate::effects::{self, pf};
use crate::object::ShipId;
use crate::universe::Universe;

/// The state (ship +0x570..+0x594; the beam effect stays with the effects).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Consume {
    /// +0x570.
    pub active: bool,
    /// +0x574 (cleared): stopped.
    pub stopped: bool,
    /// +0x578: where the beam comes from.
    pub target: Vector,
    /// +0x584: the stopwatch.
    pub start: f32,
    pub duration: f32,
}

/// `DAT_0086e600`: the time when the caller gives none. `DAT_008a230c` is what a depot gives.
pub const DEFAULT_TIME: f32 = 10.0;
/// `etgSpecialPurposeEffectTable`: the beam and its sparks.
const BEAM_EFFECT_SLOT: usize = 35;
const SPARK_EFFECT_SLOT: usize = 34;
/// 0x86e66c..0x86e678: sparks per update, and the weights that place them on the cut.
const SPARKS: i32 = 5;
const SPARK_CENTRE_WEIGHT: f32 = 0.5;
const SPARK_WEIGHT: (f32, f32) = (0.5, 5.0);
/// 0x86e608: the edges of a bounding box (`DAT_0086e608` pairs, first index then second).
const BOX_EDGES: [(usize, usize); 12] =
    [(0, 1), (1, 2), (2, 3), (3, 0), (4, 5), (5, 6), (6, 7), (7, 4), (0, 4), (1, 5), (2, 6), (3, 7)];

/// `consumeIsActive` (0x45b7e0).
pub fn is_active(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.base.consume.active)
}

/// `consumeStart` (0x45b960): anything clamped to the ship is let go and destroyed first.
pub fn start(u: &mut Universe, id: ShipId, target: Vector, time: f32) {
    if is_active(u, id) {
        return;
    }
    for slave in crate::clamp::slaves_of(u, id) {
        crate::clamp::unclamp(u, slave);
        let player = u.ships.get(slave).map_or(0, |s| s.player as i32);
        crate::damage::apply_damage_to_target(u, slave, 1.0e9, 0, crate::damage::how::KILLED_BY_SHOCKWAVE, player, None);
    }
    let time = if time <= 0.0 { DEFAULT_TIME } else { time };
    let now = u.total_time_elapsed;
    u.ships.ship_mut(id).base.consume = Consume { active: true, stopped: false, target, start: now, duration: time };
    update_beam(u, id);
}

/// `consumeGetProgress` (0x45ba60): how far along, 0..1.
pub fn progress(u: &Universe, id: ShipId) -> f64 {
    let Some(s) = u.ships.get(id) else { return 0.0 };
    let c = &s.base.consume;
    if c.duration <= 0.0 {
        return 0.0;
    }
    ((ld(u.total_time_elapsed) - ld(c.start)) / ld(c.duration)).min(1.0)
}

/// `consumeSetTarget` (0x45ba30).
pub fn set_target(u: &mut Universe, id: ShipId, target: Vector) {
    u.ships.ship_mut(id).base.consume.target = target;
    update_beam(u, id);
}

/// `consumeStop` (0x45c200): the beam stops, the ship stays as eaten as it is.
pub fn stop(u: &mut Universe, id: ShipId) {
    if !is_active(u, id) {
        return;
    }
    u.ships.ship_mut(id).base.consume.stopped = true;
    delete_beam(u, id);
}

/// The beam effect goes with the ship (`MothershipDeleteEffect`).
pub fn delete_beam(u: &mut Universe, id: ShipId) {
    if let Some(fx) = u.effects.as_mut() {
        if let Some(e) = fx.consume_beam.remove(&id) {
            fx.world.delete_effect(e);
        }
    }
}

/// `consumeGetClipPlane` (0x45b800), in the ship's own space: what is on the far side of the plane
/// `normal . p + d >= 0` is gone. The plane starts a forward length beyond the centre on the side
/// the beam comes from and ends as far on the other side.
pub fn clip_plane(u: &Universe, id: ShipId) -> Option<(Vector, f32)> {
    let s = u.ships.get(id)?;
    if !s.base.consume.active {
        return None;
    }
    let stat = u.statics.ship(s.ship_type)?;
    let radius = stat.header.coll.coll_sphere_size;
    let frac = progress(u, id) as f32;
    let cs: &Matrix = &s.rotinfo.coordsys;
    let p = s.posinfo.position;
    let t = s.base.consume.target;
    let mut dir = cs.vec_mul(Vector::new(t.x - p.x, t.y - p.y, t.z - p.z));
    dir.normalize();
    let along = radius - (frac + frac) * radius;
    let on = Vector::new(dir.x * along, dir.y * along, dir.z * along);
    let n = Vector::new(-dir.x, -dir.y, -dir.z);
    Some((n, -(on.x * n.x + on.y * n.y + on.z * n.z)))
}

/// `consumeUpdate` (0x45bde0, each think of the ship): sparks along the cut, the beam kept alive.
pub fn update(u: &mut Universe, id: ShipId) {
    if u.effects.is_none() {
        return;
    }
    let Some(s) = u.ships.get(id) else { return };
    let c = s.base.consume;
    if !c.active || c.stopped || progress(u, id) >= 1.0 {
        return;
    }
    let Some((n, d)) = clip_plane(u, id) else { return };
    // Where the plane cuts the edges of the ship's collision box (the corners at +0xec).
    let corners: Vec<Vector> = s.coll.rect.iter().map(|&c| s.rotinfo.coordsys.vec_mul(c)).collect();
    let mut cut = Vec::with_capacity(12);
    for (a, b) in BOX_EDGES {
        let (a, b) = (corners[a], corners[b]);
        let e = Vector::new(b.x - a.x, b.y - a.y, b.z - a.z);
        let denom = n.x * e.x + n.y * e.y + n.z * e.z;
        if denom == 0.0 {
            continue;
        }
        let k = -(n.x * a.x + n.y * a.y + n.z * a.z + d) / denom;
        if (0.0..=1.0).contains(&k) {
            cut.push(Vector::new(a.x + e.x * k, a.y + e.y * k, a.z + e.z * k));
        }
    }
    if cut.len() > 2 {
        sparks(u, id, &cut);
    }
    let has_beam = u.effects.as_ref().is_some_and(|fx| fx.consume_beam.contains_key(&id));
    if !has_beam {
        update_beam(u, id);
    }
}

/// `consumeSpawnSparks` (0x45bb40): each spark at a random blend of the points of the cut.
fn sparks(u: &mut Universe, id: ShipId, cut: &[Vector]) {
    let Some(effect) = u.effects.as_ref().and_then(|fx| fx.pick(fx.tables.special_effect(SPARK_EFFECT_SLOT))) else { return };
    let n = cut.len() as f32;
    let centre = cut.iter().fold(Vector::ZERO, |a, b| Vector::new(a.x + b.x, a.y + b.y, a.z + b.z));
    let centre = Vector::new(centre.x / n, centre.y / n, centre.z / n);
    for _ in 0..SPARKS {
        let mut total = SPARK_CENTRE_WEIGHT;
        let mut p = Vector::new(centre.x * total, centre.y * total, centre.z * total);
        for c in cut {
            let w = u.randy.frandom_between_ext(RAN_PARTICLE_STREAM, SPARK_WEIGHT.0, SPARK_WEIGHT.1) as f32;
            p = Vector::new(p.x + c.x * w, p.y + c.y * w, p.z + c.z * w);
            total += w;
        }
        let k = 1.0 / total;
        let s = u.ships.ship(id);
        let w = s.rotinfo.coordsys.mul_vec(Vector::new(p.x * k, p.y * k, p.z * k));
        let at = Vector::new(w.x + s.posinfo.position.x, w.y + s.posinfo.position.y, w.z + s.posinfo.position.z);
        let t = s.base.consume.target;
        let mut vel = Vector::new(t.x - at.x, t.y - at.y, t.z - at.z);
        vel.normalize();
        let cs = Matrix::coord_sys_from_heading(vel);
        if effects::frequency_ok(u, effect) {
            effects::create(u, effect, None, Some(at), Some(vel), Some(cs), 1.0, 0x78, &[]);
        }
    }
}

/// `consumeUpdateBeamEffect` (0x45c080).
fn update_beam(u: &mut Universe, id: ShipId) {
    if u.effects.is_none() {
        return;
    }
    let Some(s) = u.ships.get(id) else { return };
    let c = s.base.consume;
    let p = s.posinfo.position;
    let d = Vector::new(st(ld(p.x) - ld(c.target.x)), st(ld(p.y) - ld(c.target.y)), st(ld(p.z) - ld(c.target.z)));
    let dist = st(d.mag_ext());
    let mut heading = d;
    heading.normalize();
    let cs = Matrix::coord_sys_from_heading(heading);
    let radius = u.statics.ship(s.ship_type).map_or(0.0, |st| st.header.coll.avoid_coll_sphere_size);
    let existing = u.effects.as_ref().and_then(|fx| fx.consume_beam.get(&id).copied());
    if let Some(e) = existing {
        if let Some(ef) = u.effects.as_mut().and_then(|fx| fx.world.effects.iter_mut().find(|x| x.id == e)) {
            ef.position = c.target;
            ef.coordsys = cs;
        }
        return;
    }
    let Some(effect) = u.effects.as_ref().and_then(|fx| fx.pick(fx.tables.special_effect(BEAM_EFFECT_SLOT))) else { return };
    if !effects::frequency_ok(u, effect) {
        return;
    }
    let remaining = st(ld(c.duration) - (ld(u.total_time_elapsed) - ld(c.start)));
    let crystal = if crate::ships::crystal::is_crystal(u, id) { 1.0f32 } else { 0.0 };
    let params = [pf(dist), pf(radius * 0.5), pf(remaining), pf(crystal)];
    if let Some(e) = effects::create(u, effect, None, Some(c.target), Some(Vector::ZERO), Some(cs), 1.0, 0x20, &params) {
        if let Some(fx) = u.effects.as_mut() {
            fx.consume_beam.insert(id, e);
        }
    }
}
