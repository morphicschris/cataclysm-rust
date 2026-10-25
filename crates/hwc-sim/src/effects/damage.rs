//! Damage effects (`Damage.c`): smoke, fire and sparks on ships as their health drops
//! (`dmgShipThink`, `dmgPlayEffect`, `dmgStopEffect`, `dmgStopSingleEffect`).
//!
//! Each ship shows at most one "light" effect (at its first engine nozzle), two "heavy"
//! effects (at random `Dmg` points of its mesh) and one "dying" effect (at its centre). Every
//! think, a level whose health threshold is passed may start one more effect; the chance
//! grows as the health falls below the threshold. A level whose threshold is no longer passed
//! loses its effects at once. The damage crystals flicker instead of smoking.
//!
//! The original also turns the engine trails "wobbly" below a threshold; trails are drawn by
//! the presentation, so the flag is only recorded here ([`trail_wobbly`]).

use hwc_fx::runtime::EffectId;
use hwc_math::fpu::{ftol_i32, ld, st};
use hwc_math::random::{RAN_DAMAGE, RAN_PARTICLE_STREAM};
use hwc_math::{HMatrix, Matrix, Vector};

use crate::object::ShipId;
use crate::statics::{ShipClass, ShipType};
use crate::universe::Universe;

use super::{attach, create, frequency_ok, in_render_list, pf, FxOwner};

/// Effects per level (`DAT_0086efa0`: light 1, heavy 2, dying 1).
pub const MAX_EFFECTS: [usize; 3] = [1, 2, 1];
/// `DMG_All`: every level.
pub const ALL: usize = 3;

/// `DMG_Light`, `DMG_Heavy`, `DMG_Dying`.
pub const LIGHT: usize = 0;
pub const HEAVY: usize = 1;
pub const DYING: usize = 2;

/// `damage.script` defaults (0x86efac..0x86efc0): health ratios at or below which strike
/// craft trails wobble and their light damage shows, and the same for everything else plus
/// the heavy and dying levels.
pub const STRIKE_TRAIL_LIMIT: f32 = 0.6;
pub const STRIKE_LIGHT_LIMIT: f32 = 0.4;
pub const TRAIL_LIMIT: f32 = 0.6;
pub const LIGHT_LIMIT: f32 = 0.4;
pub const HEAVY_LIMIT: f32 = 0.2;
pub const DYING_LIMIT: f32 = 0.075;

/// The most `DmgN` chunks a mesh can name (`dmgGetLights` stops at 50).
const MAX_DAMAGE_LIGHTS: usize = 50;

/// `shipHyperspaceState` while a ship waits to come out of hyperspace.
const SHIP_HS_WAITING: u32 = 6;
/// `hsState` values that mean "no window effect running".
const HS_INACTIVE: u32 = 0;
const HS_FINISHED: u32 = 7;

/// The damage-effect record of one ship.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DamageSlots {
    /// `ship->showingDamage[level][i]` (+0x3ac).
    pub showing: [[Option<EffectId>; 2]; 3],
    /// The last `trailMakeWobbly` request (`trail->wobbly` of every engine trail).
    pub wobbly: bool,
    /// The ship's hyperspace info as `dmgRandomDamageLocation` reads it (ship info +0x20
    /// `shipHyperspaceState` and +0x40 `hsState`), mirrored by the code that changes it
    /// ([`super::hyperspace::note_state`]).
    pub hs_ship_state: u32,
    pub hs_state: u32,
}

impl DamageSlots {
    /// Effects playing at `level` (`dmgNumEffects`).
    pub fn count(&self, level: usize) -> usize {
        self.showing[level].iter().take(MAX_EFFECTS[level]).filter(|e| e.is_some()).count()
    }

    /// Every damage effect playing, by level.
    pub fn effects(&self) -> impl Iterator<Item = (usize, EffectId)> + '_ {
        self.showing
            .iter()
            .enumerate()
            .flat_map(|(l, row)| row.iter().take(MAX_EFFECTS[l]).flatten().map(move |&e| (l, e)))
    }
}

/// The damage record of a ship (`None` without effects or before its first damage).
pub fn slots(u: &Universe, id: ShipId) -> Option<&DamageSlots> {
    u.effects.as_ref()?.damage.get(&id)
}

/// Whether the ship's engine trails should wobble (the presentation applies it).
pub fn trail_wobbly(u: &Universe, id: ShipId) -> bool {
    slots(u, id).is_some_and(|s| s.wobbly)
}

/// `trailMakeWobbly`.
fn make_wobbly(u: &mut Universe, id: ShipId, wobbly: bool) {
    if let Some(fx) = u.effects.as_mut() {
        fx.damage.entry(id).or_default().wobbly = wobbly;
    }
}

/// `dmgShipThink`: start or stop the damage effects of a ship from its health.
pub fn ship_think(u: &mut Universe, id: ShipId) {
    let Some(fx) = u.effects.as_ref() else { return };
    if !fx.options.damage || !in_render_list(u, FxOwner::Ship(id)) {
        return;
    }
    let Some(s) = u.ships.get(id) else { return };
    // Hulls still being built show no damage.
    if s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0 {
        return;
    }
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let ratio_ext = ld(s.health) / ld(stat.health.max_health);
    let ratio = st(ratio_ext);
    let class = stat.ship_class.map_or(0, |c| c.index());
    let strike = matches!(stat.ship_class, Some(ShipClass::Corvette | ShipClass::Fighter));
    if strike {
        // The trail test compares the unrounded quotient.
        make_wobbly(u, id, ratio_ext <= ld(STRIKE_TRAIL_LIMIT));
        if ratio <= STRIKE_LIGHT_LIMIT {
            play_effect(u, id, class, LIGHT, st(ld(ratio) / ld(STRIKE_LIGHT_LIMIT)));
        } else {
            stop_effect(u, id, LIGHT);
        }
        return;
    }
    make_wobbly(u, id, ratio_ext <= ld(TRAIL_LIMIT));
    for (level, limit) in [(LIGHT, LIGHT_LIMIT), (HEAVY, HEAVY_LIMIT), (DYING, DYING_LIMIT)] {
        if ratio <= limit {
            play_effect(u, id, class, level, st(ld(ratio) / ld(limit)));
        } else {
            stop_effect(u, id, level);
        }
    }
}

/// How likely `dmgPlayEffect` is to start an effect this think: one in `1 + trunc(20 *
/// fraction)` (fraction = health ratio over the level's limit), so a ship just below a
/// limit is unlikely to show the effect at once and a nearly dead one certain to.
pub fn chance_divisor(fraction: f32) -> u32 {
    let n = 1i32.wrapping_sub(ftol_i32(ld(fraction) * ld(-20.0)));
    if n < 0 {
        1
    } else {
        n as u32
    }
}

/// `dmgPlayEffect`: maybe start one more effect of `level` on the ship.
fn play_effect(u: &mut Universe, id: ShipId, class: usize, level: usize, fraction: f32) {
    let n = chance_divisor(fraction);
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(lod) = fx.tables.damage_effect(class, level).map(<[usize]>::to_vec) else { return };
    let lod_level = usize::from(fx.options.lod);
    let Some(s) = u.ships.get(id) else { return };
    let crystal = matches!(s.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1);
    // The first roll decides whether anything happens this think.
    if n > 0 {
        let r = u.effects.as_mut().unwrap().world.randy.random(RAN_PARTICLE_STREAM);
        if !r.is_multiple_of(n) {
            return;
        }
    }
    if crystal {
        // Crystals flicker for half a second instead (gameplay state, as in the original).
        let now = u.total_time_elapsed;
        let s = u.ships.ship_mut(id);
        s.flicker.next = now;
        s.flicker.end = st(ld(now) + 0.5);
        s.flicker.state = 2;
        return;
    }
    let fx = u.effects.as_mut().unwrap();
    if fx.damage.get(&id).map_or(0, |d| d.count(level)) >= MAX_EFFECTS[level] {
        return;
    }
    let Some(&stat_fx) = lod.get(lod_level) else { return };
    let Some((loc, cs)) = location(u, id, level) else { return };
    let s = u.ships.ship(id);
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let size = stat.header.coll.coll_sphere_size;
    let health = st(ld(stat.health.one_over_max_health) * ld(s.health));
    // Ship space to world space.
    let local = HMatrix::from_matrix_and_pos(&cs, loc);
    let ship = HMatrix::from_matrix_and_pos(&s.rotinfo.coordsys, s.posinfo.position);
    let world = ship.mul_hmat(&local);
    let coordsys = Matrix::from_hmatrix(&world);
    let position = Vector::new(world.m[12], world.m[13], world.m[14]);
    let velocity = s.posinfo.velocity;
    // (Ship scale: `magnitudeSquared`, always 1.)
    let nlips = 1.0;
    if !frequency_ok(u, stat_fx) {
        return;
    }
    let flags = attach::VELOCITY | attach::NLIPS;
    let Some(e) = create(u, stat_fx, Some(FxOwner::Ship(id)), Some(position), Some(velocity), Some(coordsys), nlips, flags, &[pf(health), pf(size)]) else {
        return;
    };
    let fx = u.effects.as_mut().unwrap();
    let row = &mut fx.damage.entry(id).or_default().showing[level];
    // There is always a free slot (counted above); the original stops with a fatal error
    // otherwise.
    if let Some(slot) = row.iter_mut().take(MAX_EFFECTS[level]).find(|x| x.is_none()) {
        *slot = Some(e);
    }
}

/// Where an effect of `level` goes, in ship space: the first engine nozzle (light), a random
/// damage point (heavy) or the centre (dying). `None`: no effect this time.
fn location(u: &mut Universe, id: ShipId, level: usize) -> Option<(Vector, Matrix)> {
    let s = u.ships.get(id)?;
    let stat = u.statics.ship(s.ship_type)?;
    let scale = 1.0f32;
    match level {
        LIGHT => {
            let mex = stat.header.mex.as_ref()?;
            let p = match &mex.find("Eng", "Eng0")?.data {
                hwc_formats::mex::ChunkData::Engine { point, .. } => *point,
                hwc_formats::mex::ChunkData::Damage(p) | hwc_formats::mex::ChunkData::Gun(p) => *p,
                _ => return None,
            };
            Some((scaled(p.position, scale), coord_sys_from_vector(p.normal)))
        }
        HEAVY => random_damage_location(u, id),
        _ => Some((Vector::ZERO, Matrix::IDENTITY)),
    }
}

fn scaled(p: [f32; 3], scale: f32) -> Vector {
    Vector::new(st(ld(scale) * ld(p[0])), st(ld(scale) * ld(p[1])), st(ld(scale) * ld(p[2])))
}

/// The ship's damage lights (`ship->damageLights`, filled by `dmgGetLights` when the ship is
/// created): position and normal of the mesh's `Dmg0`, `Dmg1`, ... chunks, up to the first
/// one missing.
pub fn damage_lights(mex: &hwc_formats::mex::Mex) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for i in 0..MAX_DAMAGE_LIGHTS {
        // The name's digit is `'0' + i` (so `Dmg:` follows `Dmg9`).
        let name = format!("Dmg{}", char::from(b'0' + i as u8));
        let Some(c) = mex.find("Dmg", &name) else { break };
        let p = match &c.data {
            hwc_formats::mex::ChunkData::Damage(p) | hwc_formats::mex::ChunkData::Gun(p) => *p,
            hwc_formats::mex::ChunkData::Engine { point, .. } => *point,
            _ => break,
        };
        out.push((p.position, p.normal));
    }
    out
}

/// `dmgRandomDamageLocation`: one of the ship's damage lights, picked with the damage random
/// stream. None without lights, or while the ship waits in or slices through hyperspace.
fn random_damage_location(u: &mut Universe, id: ShipId) -> Option<(Vector, Matrix)> {
    let s = u.ships.get(id)?;
    let stat = u.statics.ship(s.ship_type)?;
    let lights = stat.header.mex.as_ref().map(|m| damage_lights(m)).unwrap_or_default();
    if lights.is_empty() {
        return None;
    }
    let fx = u.effects.as_mut()?;
    let r = fx.world.randy.random(RAN_DAMAGE);
    let (pos, normal) = lights[(r % lights.len() as u32) as usize];
    let loc = scaled(pos, 1.0);
    if let Some(d) = fx.damage.get(&id) {
        if d.hs_ship_state == SHIP_HS_WAITING || (d.hs_state != HS_INACTIVE && d.hs_state != HS_FINISHED) {
            return None;
        }
    }
    Some((loc, coord_sys_from_vector(normal)))
}

/// `mexVecToVec`: a mesh direction, normalised; a zero vector means straight along x.
fn mex_vec(v: [f32; 3]) -> Vector {
    let mut r = Vector::new(v[0], v[1], v[2]);
    if r.x == 0.0 && r.y == 0.0 && r.z == 0.0 {
        r.x = 1.0;
        return r;
    }
    r.normalize();
    r
}

/// `dmgCoordSysFromVector`: a coordinate system whose heading is the mesh direction `axis`.
pub fn coord_sys_from_vector(axis: [f32; 3]) -> Matrix {
    let mut lof = mex_vec(axis);
    let mut right = lof.cross(Vector::new(0.0, 0.0, 1.0));
    let mut up = lof.cross(right);
    lof.normalize();
    right.normalize();
    up.normalize();
    Matrix::from_cols(up, right, lof)
}

/// `dmgStopEffect`: delete the damage effects of one level ([`ALL`]: every level, and the
/// trails stop wobbling).
pub fn stop_effect(u: &mut Universe, id: ShipId, level: usize) {
    if level == ALL {
        if let Some(fx) = u.effects.as_mut() {
            if let Some(d) = fx.damage.get_mut(&id) {
                d.wobbly = false;
            }
        }
    }
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(slots) = fx.damage.get_mut(&id) else { return };
    for (l, row) in slots.showing.iter_mut().enumerate() {
        if level != ALL && l != level {
            continue;
        }
        for slot in row.iter_mut().take(MAX_EFFECTS[l]) {
            if let Some(e) = slot.take() {
                fx.world.delete_effect(e);
            }
        }
    }
}

/// `dmgStopSingleEffect` (the script's `damageDone()`): forget the effect, which deletes
/// itself.
pub fn stop_single_effect(u: &mut Universe, id: ShipId, effect: EffectId) {
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(slots) = fx.damage.get_mut(&id) else { return };
    for (l, row) in slots.showing.iter_mut().enumerate() {
        for slot in row.iter_mut().take(MAX_EFFECTS[l]) {
            if *slot == Some(effect) {
                *slot = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chance_grows_as_health_falls() {
        assert_eq!(chance_divisor(1.0), 21);
        assert_eq!(chance_divisor(0.5), 11);
        assert_eq!(chance_divisor(0.049), 1);
        assert_eq!(chance_divisor(0.0), 1);
        // Truncation toward zero, as `__ftol`.
        assert_eq!(chance_divisor(0.99), 20);
    }

    #[test]
    fn crystal_types_are_the_originals() {
        assert_eq!(ShipType::dCrystal0 as u32, 0x7e);
        assert_eq!(ShipType::dCrystal1 as u32, 0x7f);
        assert_eq!(ShipClass::Corvette as u32, 5);
        assert_eq!(ShipClass::Fighter as u32, 6);
    }

    #[test]
    fn coordsys_heads_along_the_normal() {
        let m = coord_sys_from_vector([0.0, 2.0, 0.0]);
        assert_eq!(m.col(2), Vector::new(0.0, 1.0, 0.0));
        // right = lof x z, up = lof x right.
        assert_eq!(m.col(1), Vector::new(1.0, 0.0, 0.0));
        assert_eq!(m.col(0), Vector::new(0.0, 0.0, -1.0));
        // A zero normal points along x.
        let z = coord_sys_from_vector([0.0, 0.0, 0.0]);
        assert_eq!(z.col(2), Vector::new(1.0, 0.0, 0.0));
    }

    #[test]
    fn slots_count_per_level() {
        let mut d = DamageSlots::default();
        d.showing[HEAVY][0] = Some(3);
        d.showing[HEAVY][1] = Some(4);
        d.showing[LIGHT][1] = Some(9); // beyond the light level's single slot: ignored
        assert_eq!(d.count(HEAVY), 2);
        assert_eq!(d.count(LIGHT), 0);
        assert_eq!(d.effects().collect::<Vec<_>>(), vec![(HEAVY, 3), (HEAVY, 4)]);
    }
}
