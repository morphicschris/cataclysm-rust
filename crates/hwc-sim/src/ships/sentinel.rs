//! Sentinels and their force fields (`Sentinel` module, 0x5c0490..0x5c18d0): sSentinel,
//! bSentinel.
//!
//! Three or more sentinels in a sentinel formation (type 9, the flat field wall laid out by
//! [`wall_layout`]; type 10, the field sphere around a protected ship) raise a force field once
//! each has reached its slot: every sentinel except the leader owns one or two field triangles
//! (itself and two neighbours). While a sentinel's field is fully up, enemy bullets, beams and
//! missiles that cross one of its triangles are stopped there, ships do not bump through it,
//! and hits on the sentinel itself go to the field. The field has a strength (2000/4000/6000
//! by upgrade level, regenerating 2/4/7 per frame); when it is shot below zero it goes down
//! until fully recharged. A DFG nearby suppresses fields for a while (`+0x97c`, `+0x980`).
//!
//! The per-sentinel field state lives in [`SentinelSpec`] (`+0x638..+0x988`).
//!
//! The field drawing (`SentinelQueueFieldHit`, `SentinelProcessFieldHitsNear`,
//! `SentinelFlushFieldHits`, `SentinelUpdateFieldSegment(s)`, `SentinelFieldHitEffect`) is
//! render-only; the game draws it from [`SentinelSpec`] (`apps/cataclysm`, `sentinel_field`).
//!
//! The player raises a field with the Force Field special action ([`crate::player_orders::mr_protect_ships`]):
//! with ships band-boxed, up to 12 sentinels guard each one in a field sphere (laid out by
//! [`crate::formation::sphere_specifics`], the sentinels clamped to the ship while their field
//! is up); with none, three or more make a field wall. Unlink Force Field turns them back into
//! an ordinary sphere or wall ([`crate::player_orders::mr_formation_revert_special`]).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_i32, ftol, ld, round_magic, st, Ext};
use hwc_math::Vector;

use crate::object::{Bullet, Ship, ShipId};
use crate::ships::{Effect, ShipSpec};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::{SpecialEvent, Universe};

/// Formation types of sentinels.
pub mod formation_kind {
    /// The field wall.
    pub const WALL: u32 = 9;
    /// The field sphere around a protected ship.
    pub const SPHERE: u32 = 10;
}

/// Field strength per level (0x8bc5c0) and its regeneration per frame (0x8bc5cc).
pub const FIELD_STRENGTH: [f32; 3] = [2000.0, 4000.0, 6000.0];
pub const FIELD_REGEN: [f32; 3] = [2.0, 4.0, 7.0];
/// How close to its slot a sentinel must be for the field (0x8bc570).
const IN_POSITION: f32 = 50.0;
/// Field rise rate per second (0x8bc57c) and fade of the "just up" glow (0x8bc580).
const FIELD_RISE: f32 = f32::from_bits(0x3e4c_cccd);
const FIELD_GLOW_FADE: f32 = f32::from_bits(0x3f19_999a);
/// Spacing of the wall slots (0x875820).
const WALL_SPACING: f32 = 300.0;
/// Minimum range of a sentinel's attack (`SentinelAttack`).
const ATTACK_MIN_RANGE: f32 = 1000.0;
/// Upgrade bits that raise the field level (`FUN_0047cd40`).
const UPGRADE_FIELD_2: u32 = 0x4000;
const UPGRADE_FIELD_1: u32 = 0x2000;
/// `specialFlags2` bit set while a sentinel is in a sentinel formation.
pub const SPECIAL2_FIELD_FORMATION: u32 = 0x1000;

/// Per-ship state (`ShipSpecifics`, +0x638).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SentinelSpec {
    /// +0x638: the field of this sentinel's formation is on here.
    pub field_on: bool,
    /// +0x63c: how far the field has risen (0..1; it blocks at exactly 1).
    pub rise: f32,
    /// +0x640/+0x644: the field triangles this sentinel owns (itself and two neighbours).
    pub segments: Vec<(ShipId, ShipId)>,
    /// +0x964: field level 0..2 (the formation's lowest upgrade).
    pub level: usize,
    /// +0x968: when the field was last hit.
    pub last_hit: f32,
    /// +0x96c: the glow of a freshly raised field (fades out).
    pub glow: f32,
    /// +0x974: field strength.
    pub strength: f32,
    /// +0x978: shot down, until fully recharged.
    pub down: bool,
    /// +0x97c / +0x980: suppressed (by a DFG) until the time.
    pub suppressed: bool,
    pub suppressed_until: f32,
    /// +0x984: at its slot.
    pub in_position: bool,
}

/// Per-type statics (`SentinelStaticInit`, 0x5c06b0: the side-step keywords, which the combat
/// statics read).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SentinelStatics {}

impl SentinelStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

fn spec_of(s: &Ship) -> Option<&SentinelSpec> {
    match &s.spec {
        ShipSpec::Sentinel(x) => Some(x),
        _ => None,
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&SentinelSpec> {
    u.ships.get(id).and_then(spec_of)
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut SentinelSpec> {
    match u.ships.get_mut(id).map(|s| &mut s.spec) {
        Some(ShipSpec::Sentinel(x)) => Some(x),
        _ => None,
    }
}

/// `SentinelIsSentinel` (0x5c0490).
pub fn is_sentinel_type(t: ShipType) -> bool {
    matches!(t, ShipType::sSentinel | ShipType::bSentinel)
}

/// `SentinelInit` (0x5c0700).
pub fn init(u: &mut Universe, id: ShipId) {
    u.ships.ship_mut(id).spec = ShipSpec::Sentinel(SentinelSpec::default());
}

/// The formation type of a ship's command, when it is a formation.
fn formation_type(u: &Universe, s: &Ship) -> Option<u32> {
    let c = s.command.and_then(|c| u.commands.get(c))?;
    (c.attributes & crate::command::attr::FORMATION != 0).then_some(c.formation.formation_type)
}

/// `SentinelInFieldFormation` (0x5c1800): a sentinel in a field sphere (type 10).
pub fn in_field_formation(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| is_sentinel_type(s.ship_type) && formation_type(u, s) == Some(formation_kind::SPHERE))
}

/// `SentinelInSentinelFormation` (0x5c1840): a sentinel in a field wall or sphere.
pub fn in_sentinel_formation(u: &Universe, id: ShipId) -> bool {
    u.ships
        .get(id)
        .is_some_and(|s| is_sentinel_type(s.ship_type) && matches!(formation_type(u, s), Some(formation_kind::WALL | formation_kind::SPHERE)))
}

/// `SentinelAttack` (0x5c0740): `attackPassive` from the gun range down to 1000.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let max = stat.bullet_range[s.tactics.min(2)];
    crate::attack::attack_passive(u, id, target, max, ATTACK_MIN_RANGE);
}

/// `SentinelAttackPassive` (0x5c0770): sentinels holding a field sphere do not fire.
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if in_field_formation(u, id) {
        return;
    }
    if rotate {
        crate::attack::attack_passive_rotate(u, id, target);
    } else {
        crate::attack::attack_simple(u, id, target);
    }
}

/// `SentinelHouseKeep` (0x5c07c0): the field recharges; three frames in four the formation's
/// field is updated.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let counter = u.update_counter;
    let Some(s) = spec_mut(u, id) else { return };
    let lvl = s.level.min(2);
    if s.strength < FIELD_STRENGTH[lvl] {
        s.strength = st(ld(FIELD_REGEN[lvl]) + ld(s.strength));
    }
    if FIELD_STRENGTH[lvl] <= s.strength {
        s.down = false;
    }
    if counter & 0xc != 0 {
        update_formation(u, id);
    }
}

/// `SentinelGetFieldStrengthFraction` (0x5c0820).
pub fn field_strength_fraction(u: &Universe, id: ShipId) -> f32 {
    spec(u, id).map_or(0.0, |s| st(ld(s.strength) / ld(FIELD_STRENGTH[s.level.min(2)])))
}

/// `SentinelFieldIsUp` (0x5c17d0): the field is on and fully risen.
pub fn field_is_up(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(field_is_up_on)
}

fn field_is_up_on(s: &Ship) -> bool {
    spec_of(s).is_some_and(|x| x.field_on && x.rise == 1.0)
}

/// A sentinel with its field up (the test the collision code makes before the segments).
fn field_blocks(s: &Ship) -> bool {
    is_sentinel_type(s.ship_type) && field_is_up_on(s)
}

/// `SentinelFieldTakeDamage` (0x5c15e0): a bullet or beam hits the field.
pub fn field_take_damage(u: &mut Universe, id: ShipId, bullet: &Bullet) {
    let now = u.total_time_elapsed;
    let Some(s) = u.ships.get(id) else { return };
    let mult = crate::damage::weapon_damage(bullet.bullet_type, s.armour_level);
    let dmg = round_magic(ld(mult) * ld(bullet.damage));
    let Some(f) = spec_mut(u, id) else { return };
    f.last_hit = now;
    f.strength = st(ld(f.strength) - fild_i32(dmg));
    if f.strength < 0.0 {
        f.down = true;
    }
    u.special_events.push(SpecialEvent::Special { ship: id, effect: Effect::FieldHit });
}

/// `SentinelFieldDamageEffect` (0x5c1650): a missile hits the field (whole points of its
/// damage), showing the missile's hit effect where it is.
fn field_missile_damage(u: &mut Universe, id: ShipId, missile: &crate::missile::Missile) {
    let damage = missile.damage;
    let now = u.total_time_elapsed;
    if let Some(f) = spec_mut(u, id) {
        f.last_hit = now;
    }
    field_hit_effect(u, id, missile);
    let Some(f) = spec_mut(u, id) else { return };
    f.strength = st(ld(f.strength) - ftol(ld(damage)) as Ext);
    if f.strength < 0.0 {
        f.down = true;
    }
    u.special_events.push(SpecialEvent::Special { ship: id, effect: Effect::FieldHit });
}

/// The hit effect a missile carries (`missile->hitEffect`, +0x204): the gun-hit event of the
/// firing ship's race and the missile's bullet type (`missileShoot`); level mines use the
/// mine row (bullet type 9) of their owner's race (`scriptSetMineCB`).
fn missile_hit_lod(u: &Universe, m: &crate::missile::Missile) -> Option<Vec<usize>> {
    let fx = u.effects.as_ref()?;
    let (race, bullet_type) = if m.level_mine_tag == 6 {
        (u.players.get(m.player?)?.race.index(), 9)
    } else {
        (m.race.index(), m.bullet_type as usize)
    };
    fx.tables.gun_event(race, bullet_type, crate::effects::GUN_HIT).map(<[usize]>::to_vec)
}

/// The effect part of `SentinelFieldDamageEffect`: the missile's hit effect at the missile,
/// facing its way and moving with the sentinel, scaled by the sentinel, with the parameters
/// 100 (integer) and 0 (when hit effects are on).
fn field_hit_effect(u: &mut Universe, id: ShipId, m: &crate::missile::Missile) {
    let Some(fx) = u.effects.as_ref() else { return };
    let lod = missile_hit_lod(u, m);
    let Some(effect) = fx.pick(lod.as_deref()) else { return };
    if !fx.options.hit || !crate::effects::in_render_list(u, id.into()) || !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let vel = u.ships.ship(id).posinfo.velocity;
    let scale = 1.0;
    crate::effects::create(u, effect, None, Some(m.posinfo.position), Some(vel), Some(m.rotinfo.coordsys), scale, 0, &[100, 0]);
}

/// `FUN_004361a0`: whether the ray from `p1` through `p2` (beyond `p2` too) crosses the
/// triangle `(a, b, c)`; also the ray parameter of the crossing (in units of `p2 - p1`).
pub fn ray_hits_triangle(p1: Vector, p2: Vector, a: Vector, b: Vector, c: Vector) -> (bool, f32) {
    let n = Vector::triangle_normal(c, b, a);
    let dot = |d: [Ext; 3]| (d[2] * ld(n.z) + d[0] * ld(n.x)) + d[1] * ld(n.y);
    let denom = st(dot([ld(p1.x) - ld(p2.x), ld(p1.y) - ld(p2.y), ld(p1.z) - ld(p2.z)]));
    if -1e-7 < ld(denom) && ld(denom) < 1e-7 {
        return (false, 0.0);
    }
    let t = st(dot([ld(p1.x) - ld(a.x), ld(p1.y) - ld(a.y), ld(p1.z) - ld(a.z)]) / ld(denom));
    if !(0.0 <= t) {
        return (false, t);
    }
    let q = Vector::new(
        st(ld(st(ld(t) * (ld(p2.x) - ld(p1.x)))) + ld(p1.x)),
        st(ld(st(ld(t) * (ld(p2.y) - ld(p1.y)))) + ld(p1.y)),
        st(ld(st(ld(t) * (ld(p2.z) - ld(p1.z)))) + ld(p1.z)),
    );
    for (base, to) in [(a, b), (b, c), (c, a)] {
        let e = Vector::new(st(ld(to.x) - ld(base.x)), st(ld(to.y) - ld(base.y)), st(ld(to.z) - ld(base.z)));
        let mut m = Vector::new(
            st(ld(e.y) * ld(n.z) - ld(e.z) * ld(n.y)),
            st(ld(e.z) * ld(n.x) - ld(e.x) * ld(n.z)),
            st(ld(e.x) * ld(n.y) - ld(e.y) * ld(n.x)),
        );
        m.normalize();
        let side = (ld(base.x) - ld(q.x)) * ld(m.x) + (ld(base.y) - ld(q.y)) * ld(m.y) + (ld(base.z) - ld(q.z)) * ld(m.z);
        if side < 0.0 {
            return (false, t);
        }
    }
    (true, t)
}

/// The triangles of a sentinel as positions (itself, then each segment's two ships).
fn triangles(u: &Universe, id: ShipId) -> Vec<(Vector, Vector, Vector)> {
    let Some(s) = u.ships.get(id) else { return Vec::new() };
    let Some(f) = spec_of(s) else { return Vec::new() };
    let p = s.posinfo.position;
    f.segments
        .iter()
        .map(|&(a, b)| {
            let pa = u.ships.get(a).map_or(Vector::ZERO, |x| x.posinfo.position);
            let pb = u.ships.get(b).map_or(Vector::ZERO, |x| x.posinfo.position);
            (p, pa, pb)
        })
        .collect()
}

/// `FUN_004366e0`: the nearest of the sentinel's field triangles that the ray from `from`
/// along `dir` crosses, and how far along. `None` when it crosses none.
pub fn field_segment_hit(u: &Universe, id: ShipId, from: Vector, dir: Vector) -> Option<(usize, f32)> {
    let to = Vector::new(st(ld(from.x) + ld(dir.x)), st(ld(from.y) + ld(dir.y)), st(ld(from.z) + ld(dir.z)));
    let mut best: Option<(usize, f32)> = None;
    let mut best_t = f32::from_bits(0x6258_d727);
    for (i, (p, a, b)) in triangles(u, id).into_iter().enumerate() {
        let (hit, t) = ray_hits_triangle(from, to, p, a, b);
        if hit && t < best_t {
            best_t = t;
            best = Some((i, t));
        }
    }
    best
}

/// The field test of the bullet walk (`collCheckBulletTargetColl`): an enemy bullet whose path
/// this frame crosses one of the target sentinel's raised field triangles. Returns how far
/// along its path.
pub fn bullet_hits_field(u: &Universe, target: ShipId, bullet: &Bullet) -> Option<f32> {
    let t = u.ships.get(target)?;
    if !field_blocks(t) || bullet.player == t.player {
        return None;
    }
    let (_, d) = field_segment_hit(u, target, bullet.posinfo.position, bullet.heading)?;
    (d < bullet.travel_dist).then_some(d)
}

/// The field test of the beam walk (`collCheckBeamCollWithTargetsInBlob`): where an enemy beam
/// meets the target sentinel's raised field.
pub fn beam_hits_field(u: &Universe, target: ShipId, beam: &Bullet) -> Option<f32> {
    let t = u.ships.get(target)?;
    if !field_blocks(t) || beam.player == t.player {
        return None;
    }
    field_segment_hit(u, target, beam.posinfo.position, beam.heading).map(|(_, d)| d)
}

/// The sentinel part of `univBulletCollidedWithTarget`: a bullet or beam that hits a sentinel
/// whose field is up hits the field instead (no damage to the ship). Returns whether it did.
pub fn absorbs_hit(u: &mut Universe, target: ShipId, bullet: &Bullet) -> bool {
    if !u.ships.get(target).is_some_and(field_blocks) {
        return false;
    }
    field_take_damage(u, target, bullet);
    true
}

/// The force-field test of `collCheckMissileShipColl`: a sentinel with its field up stops an
/// enemy missile heading through a field triangle (the missile is destroyed; the field takes
/// its damage).
pub fn missile_hits_field(u: &mut Universe, sentinel: ShipId, missile: &crate::missile::Missile) -> bool {
    let Some(s) = u.ships.get(sentinel) else { return false };
    if !field_blocks(s) || missile.player == Some(s.player) {
        return false;
    }
    let p = missile.posinfo.position;
    let v = missile.posinfo.velocity;
    let ahead = Vector::new(st(ld(v.x) + ld(p.x)), st(ld(v.y) + ld(p.y)), st(ld(v.z) + ld(p.z)));
    for (sp, a, b) in triangles(u, sentinel) {
        if ray_hits_triangle(p, ahead, sp, a, b).0 {
            field_missile_damage(u, sentinel, missile);
            return true;
        }
    }
    false
}

/// `FUN_004367b0`, the bump-collision filter: a ship does not bump into a sentinel whose field
/// is up (unless it is a kamikaze), nor a sentinel into the ship its sphere protects.
pub fn field_prevents_bump(a: &Ship, b: &Ship) -> bool {
    let kamikaze = |s: &Ship| s.special_flags & crate::object::special::KAMIKAZE != 0;
    let fa = is_sentinel_type(a.ship_type) && field_is_up_on(a);
    let fb = is_sentinel_type(b.ship_type) && field_is_up_on(b);
    (fa && !kamikaze(b)) || (fb && !kamikaze(a))
    // (A sentinel whose field is not up and that protects the other ship: commands do not keep
    // the protect list, so that case never arises.)
}

/// `SentinelRemoveShipReferences` (0x5c1710): triangles with the lost ship go (the last one
/// moves into the hole; the moved one is not checked again this pass).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(f) = spec_mut(u, id) else { return };
    let mut i = 0;
    while i < f.segments.len() {
        let (a, b) = f.segments[i];
        if a == gone || b == gone {
            let n = f.segments.len();
            if i < n - 1 {
                f.segments[i] = f.segments[n - 1];
            }
            f.segments.truncate(n - 1);
        }
        i += 1;
    }
}

/// `SentinelFormationChanged` (0x5c1770), clFormation's slot-34 hook: flags the sentinel as in
/// a sentinel formation (or not). Always stays in the formation.
pub fn formation_changed(u: &mut Universe, id: ShipId, formation_type: u32) -> bool {
    let on = matches!(formation_type, formation_kind::WALL | formation_kind::SPHERE);
    let s = u.ships.ship_mut(id);
    if on {
        s.special_flags2 |= SPECIAL2_FIELD_FORMATION;
    } else {
        s.special_flags2 &= !SPECIAL2_FIELD_FORMATION;
    }
    u.special_events.push(SpecialEvent::Special { ship: id, effect: Effect::State { state: u32::from(!on) } });
    true
}

/// The sentinels' part of `univShipThink`: every frame but each 32nd, a suppression that has
/// run out ends (+0x97c / +0x980).
pub fn think(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let counter = u.update_counter;
    if counter & 0x1f == 0 {
        return;
    }
    if let Some(f) = spec_mut(u, id) {
        if f.suppressed_until < now {
            f.suppressed = false;
        }
    }
}

/// The DFG's effect on a sentinel field (`DFGFrigateHouseKeep`, 0x5ab8d0): suppressed until
/// `until` (the DFG passes now + 2 s).
pub fn suppress_field(u: &mut Universe, id: ShipId, until: f32) {
    if let Some(f) = spec_mut(u, id) {
        f.suppressed = true;
        f.suppressed_until = until;
    }
}

/// `SentinelUpdateFormation` (0x5c11b0), run by every sentinel; the formation leader does the
/// work: which sentinels are at their slots, which fields are on, how far they have risen;
/// in a sphere the sentinels clamp to the protected ship. Other sentinels drop their own
/// field flag and any clamp (so a sentinel updated after its leader shows no field).
fn update_formation(u: &mut Universe, id: ShipId) {
    let Some(me) = u.ships.get(id) else { return };
    let cmd = me.command.and_then(|c| u.commands.get(c)).cloned();
    if let Some(c) = cmd.as_ref() {
        let ftype = c.formation.formation_type;
        let is_form = c.attributes & crate::command::attr::FORMATION != 0;
        if is_form && matches!(ftype, formation_kind::WALL | formation_kind::SPHERE) && c.selection.first() == Some(&id) {
            leader_update(u, id, c.id);
            return;
        }
        if ftype == formation_kind::SPHERE {
            return;
        }
    }
    if let Some(f) = spec_mut(u, id) {
        f.field_on = false;
    }
    crate::clamp::unclamp(u, id);
}

fn leader_update(u: &mut Universe, id: ShipId, cid: crate::object::CommandId) {
    let dt = u.phys_time_elapsed;
    let mut centre = id;
    let c = u.commands.get(cid).expect("command").clone();
    if c.formation.formation_type == formation_kind::SPHERE {
        // The sphere is around the first ship it guards; guarding nothing, or around a ship
        // that has left the universe (docked), it becomes a wall.
        let to_wall = |u: &mut Universe| {
            if let Some(cm) = u.commands.get_mut(cid) {
                cm.formation.formation_type = formation_kind::WALL;
            }
            crate::formation::calculate_offsets(u, cid);
        };
        match c.protect.first() {
            Some(&p) if c.attributes & crate::command::attr::PROTECTING != 0 => centre = p,
            _ => to_wall(u),
        }
        if !u.ship_list.contains(&centre) {
            to_wall(u);
            centre = id;
        }
    }
    let sel = u.commands.get(cid).expect("command").selection.clone();
    for &s in &sel {
        if let Some(f) = spec_mut(u, s) {
            f.field_on = false;
        }
    }
    let (cpos, csys) = {
        let c = u.ships.ship(centre);
        (c.coll.coll_position, c.rotinfo.coordsys)
    };
    for &s in &sel {
        let Some(sh) = u.ships.get(s) else { continue };
        let p = sh.posinfo.position;
        let rel = Vector::new(st(ld(p.x) - ld(cpos.x)), st(ld(p.y) - ld(cpos.y)), st(ld(p.z) - ld(cpos.z)));
        let off = csys.mul_vec(sh.formation_offset);
        let (dx, dy, dz) = (ld(rel.x) - ld(off.x), ld(rel.y) - ld(off.y), ld(rel.z) - ld(off.z));
        let d = fsqrt((dz * dz + dy * dy) + dx * dx).abs();
        if let Some(f) = spec_mut(u, s) {
            f.in_position = d <= ld(IN_POSITION);
            f.field_on = false;
        }
    }
    let n = sel.len();
    for &s in &sel {
        let Some(sh) = u.ships.get(s) else { continue };
        let Some(f) = spec_of(sh) else { continue };
        let partners_ready = |x: ShipId| spec(u, x).is_some_and(|p| p.in_position);
        let mut ok = f.in_position && f.segments.iter().all(|&(a, b)| partners_ready(a) && partners_ready(b));
        let blocked = n < 3 || f.down || sh.disable.emp_shocked || f.suppressed;
        let segments = f.segments.clone();
        let f = spec_mut(u, s).expect("sentinel");
        if blocked {
            ok = false;
        }
        if !ok {
            f.rise = 0.0;
        } else {
            if f.rise == 0.0 {
                f.glow = 1.0;
            }
            f.rise = st(ld(dt) * ld(FIELD_RISE) + ld(f.rise));
            if 1.0 < ld(f.rise) {
                f.rise = 1.0;
                if 0.0 < f.glow {
                    f.glow = st(ld(f.glow) - ld(FIELD_GLOW_FADE) * ld(dt));
                    if f.glow < 0.0 {
                        f.glow = 0.0;
                    }
                }
            }
        }
        f.field_on = ok;
        for (a, b) in segments {
            let a_on = spec(u, a).is_some_and(|x| x.field_on);
            let pick = if !a_on {
                Some(a)
            } else if !spec(u, b).is_some_and(|x| x.field_on) {
                Some(b)
            } else {
                None
            };
            if let Some(f) = pick.and_then(|p| spec_mut(u, p)) {
                f.field_on = ok;
            }
        }
    }
    // A sphere's sentinels hold on to the guarded ship while their field is up.
    let c = u.commands.get(cid).expect("command");
    if c.formation.formation_type != formation_kind::SPHERE {
        return;
    }
    let host = c.protect.first().copied();
    for &s in &sel {
        let on = spec(u, s).is_some_and(|f| f.field_on);
        if !on {
            crate::clamp::unclamp(u, s);
        } else if u.ships.get(s).is_some_and(|x| x.base.clamp.as_ref().and_then(|ci| ci.host).is_none()) {
            if let Some(h) = host.filter(|&h| u.ships.get(h).is_some()) {
                crate::clamp::clamp_obj_to_obj(u, s, h);
            }
        }
    }
}

/// `FUN_0047cb40`: the field triangles of the sentinel in slot `k` of a field sphere whose
/// top pole is slot `w` (`formationSpecificsSphere`): itself and two sentinels of the slots
/// above it, so that the twelve close the sphere.
pub fn set_sphere_segments(u: &mut Universe, selection: &[ShipId], w: usize, k: usize) {
    const SEGMENTS: [&[(usize, usize)]; 12] = [
        &[],
        &[],
        &[(0, 1)],
        &[(0, 2)],
        &[(0, 3)],
        &[(0, 4), (1, 0)],
        &[(1, 5)],
        &[(1, 6), (2, 1)],
        &[(2, 7), (3, 2)],
        &[(4, 3), (3, 8)],
        &[(5, 4), (4, 9), (6, 5)],
        &[(6, 7), (7, 8), (8, 9), (9, 10), (10, 6)],
    ];
    let list: Vec<(ShipId, ShipId)> = SEGMENTS
        .get(k - w)
        .map(|segs| segs.iter().filter_map(|&(a, b)| Some((*selection.get(w + a)?, *selection.get(w + b)?))).collect())
        .unwrap_or_default();
    if let Some(&id) = selection.get(k) {
        if let Some(f) = spec_mut(u, id) {
            f.segments = list;
        }
    }
}

/// `FUN_0047cd40`: the field level of a sentinel formation, the lowest of its sentinels'
/// upgrades (2 with 0x4000, 1 with 0x2000), stored in every sentinel.
pub fn set_field_level(u: &mut Universe, selection: &[ShipId]) {
    let mut level = 2usize;
    for &s in selection {
        let Some(sh) = u.ships.get(s) else { continue };
        if level == 2 && !crate::upgrade::has_upgrade(sh, UPGRADE_FIELD_2) {
            level = 1;
        }
        if level == 1 && !crate::upgrade::has_upgrade(sh, UPGRADE_FIELD_1) {
            level = 0;
        }
    }
    for &s in selection {
        if let Some(f) = spec_mut(u, s) {
            f.level = level;
        }
    }
}

/// The wall slots (`FUN_0047d460`, in units of the spacing, x up and y right in the leader's
/// frame) after the leader, ring by ring, and the field triangles each slot's sentinel owns
/// (indices into the selection).
/// A wall slot: its position and the field triangles of the sentinel in it.
type WallSlot = ((i8, i8), &'static [(usize, usize)]);

const WALL_SLOTS: [WallSlot; 24] = [
    ((1, 0), &[]),
    ((0, 1), &[(1, 0)]),
    ((-1, 0), &[(2, 0)]),
    ((0, -1), &[(3, 0), (0, 1)]),
    ((1, 1), &[(1, 2)]),
    ((0, 2), &[(5, 2)]),
    ((-1, 1), &[(2, 3), (6, 2)]),
    ((-2, 0), &[(7, 3)]),
    ((-1, -1), &[(8, 3), (3, 4)]),
    ((0, -2), &[(9, 4)]),
    ((1, -1), &[(10, 4), (4, 1)]),
    ((2, 0), &[(11, 1), (1, 5)]),
    ((2, 1), &[(12, 5)]),
    ((1, 2), &[(13, 5), (5, 6)]),
    ((0, 3), &[(14, 6)]),
    ((-1, 2), &[(6, 7), (15, 6)]),
    ((-2, 1), &[(16, 7), (7, 8)]),
    ((-3, 0), &[(17, 8)]),
    ((-2, -1), &[(18, 8), (8, 9)]),
    ((-1, -2), &[(19, 9), (9, 10)]),
    ((0, -3), &[(20, 10)]),
    ((1, -2), &[(21, 10), (10, 11)]),
    ((2, -1), &[(22, 11), (11, 12)]),
    ((3, 0), &[(23, 12), (12, 13)]),
];

/// `FUN_0047d460`: the field wall (formation type 9, and a wall of sentinels only): the leader
/// in the middle, the others on rings of slots around it in its plane, each owning the field
/// triangles between itself and its inner neighbours. At most 25 sentinels get slots.
pub fn wall_layout(u: &mut Universe, selection: &[ShipId]) {
    set_field_level(u, selection);
    let Some(&leader) = selection.first() else { return };
    u.ships.ship_mut(leader).formation_offset = Vector::ZERO;
    if let Some(f) = spec_mut(u, leader) {
        f.segments.clear();
    }
    let s = ld(WALL_SPACING);
    for (k, ((x, y), segs)) in WALL_SLOTS.iter().enumerate() {
        let Some(&id) = selection.get(k + 1) else { return };
        let coord = |c: i8| -> f32 {
            match c {
                0 => 0.0,
                1 => WALL_SPACING,
                -1 => -WALL_SPACING,
                2 => st(s + s),
                -2 => -st(s + s),
                3 => st(s * 3.0),
                _ => -st(s * 3.0),
            }
        };
        u.ships.ship_mut(id).formation_offset = Vector::new(coord(*x), coord(*y), 0.0);
        let list: Vec<(ShipId, ShipId)> = segs.iter().filter_map(|&(a, b)| Some((*selection.get(a)?, *selection.get(b)?))).collect();
        if let Some(f) = spec_mut(u, id) {
            f.segments = list;
        }
    }
}

/// Whether every ship of the selection is a sentinel (a wall of sentinels is laid out as a
/// field wall).
pub fn all_sentinels(u: &Universe, selection: &[ShipId]) -> bool {
    selection.iter().all(|&s| u.ships.get(s).is_some_and(|x| is_sentinel_type(x.ship_type)))
}
