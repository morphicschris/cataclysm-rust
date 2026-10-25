//! Expanding shock waves (`ShockWave.c`, 0x510b00..0x512120): the ring a dying mothership or
//! crystal sends out, the Dreadnought cannon's blast, the Nomad moon's field pulse and the
//! detonation of a mothership big-gun shell.
//!
//! A wave is a sphere shell centred where it was created. After its delay it grows from its
//! start radius at a fixed speed until it reaches its maximum radius; every update each ship
//! inside the shell (between `radius - thickness` and `radius`) is pushed outwards, spun and
//! damaged (`swApplyToShip`), and big-gun shells in flight (the "special ships") are pushed
//! too (`swApplyToSpecialShip`). The Dreadnought and Nomad waves also fling light ships with
//! the field slow-down state ([`crate::disable::apply_field_slowdown`]), which hands their
//! movement to the push for a second.
//!
//! The six templates (`swCreate` types 0..5) are compiled-in data (0x889cd8..0x889f40).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_PARTICLE_STREAM;
use hwc_math::Vector;

use crate::object::{BulletId, ShipId};
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// The wave kinds (`swCreate`'s first argument).
pub mod kind {
    /// Big-gun shell detonation in a multiplayer game.
    pub const BIG_GUN: u32 = 0;
    /// Big-gun shell detonation in a single-player game.
    pub const BIG_GUN_SP: u32 = 1;
    /// A mothership's death.
    pub const MOTHERSHIP_DEATH: u32 = 2;
    /// A crystal's death.
    pub const CRYSTAL: u32 = 3;
    /// The Dreadnought cannon.
    pub const DREADNOUGHT: u32 = 4;
    /// The Nomad moon's field pulse.
    pub const NOMAD_PULSE: u32 = 5;
}

/// One shock wave (`ShockWave`, 25 dwords).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shockwave {
    /// +0x00: the kind ([`kind`]).
    pub kind: u32,
    /// +0x04: the effect it shows.
    pub effect: i32,
    /// +0x08: centre.
    pub centre: Vector,
    /// +0x14..+0x20: damage `a + b / d + c / sqrt(d) + e / d^2` at distance `d`.
    pub damage_const: f32,
    pub damage_per_dist: f32,
    pub damage_per_sqrt_dist: f32,
    pub damage_per_dist_sq: f32,
    /// +0x24 / +0x28: push `(push + size^2 * push_per_size_sq) * speed` on a ship of
    /// collision radius `size`.
    pub push: f32,
    pub push_per_size_sq: f32,
    /// +0x2c: growth in metres per second.
    pub speed: f32,
    /// +0x30 / +0x34: creation time and delay before it starts growing.
    pub start_time: f32,
    pub delay: f32,
    /// +0x38: thickness of the shell.
    pub thickness: f32,
    /// +0x3c / +0x40 / +0x44: current, starting and final radius.
    pub radius: f32,
    pub start_radius: f32,
    pub max_radius: f32,
    /// +0x4c: the player that caused it (damage is credited to it; the Dreadnought's wave
    /// spares its allies).
    pub player: usize,
    /// +0x50: it has started growing (its effect was created).
    pub started: bool,
    /// +0x5c: its sound.
    pub sound: i32,
    /// +0x60: damage scale (big-gun shells: the range falloff).
    pub damage_scale: f32,
}

/// The type templates of `swCreate` (0x889cd8, 0x889d40, 0x889da8, 0x889e10, 0x889e78,
/// 0x889ee0).
fn template(kind: u32) -> Shockwave {
    let base = Shockwave {
        kind,
        effect: 0,
        centre: Vector::ZERO,
        damage_const: 0.0,
        damage_per_dist: 0.0,
        damage_per_sqrt_dist: 0.0,
        damage_per_dist_sq: 0.0,
        push: 0.0,
        push_per_size_sq: 0.0,
        speed: 0.0,
        start_time: 0.0,
        delay: 0.0,
        thickness: 0.0,
        radius: 0.0,
        start_radius: 0.0,
        max_radius: 0.0,
        player: 0,
        started: false,
        sound: -1,
        damage_scale: 1.0,
    };
    match kind {
        kind::BIG_GUN | kind::BIG_GUN_SP => Shockwave {
            effect: 0x1b,
            damage_per_dist_sq: f32::from_bits(0x513a_43b7),
            push: 1000.0,
            speed: 2420.0,
            thickness: 1000.0,
            start_radius: 1000.0,
            max_radius: if kind == kind::BIG_GUN { 13000.0 } else { 18000.0 },
            ..base
        },
        kind::MOTHERSHIP_DEATH => Shockwave {
            effect: 0x2a,
            damage_const: 10.0,
            damage_per_dist: 1000.0,
            push: 100.0,
            speed: 2420.0,
            delay: f32::from_bits(0x4053_3333),
            thickness: 1000.0,
            start_radius: 1000.0,
            max_radius: 20000.0,
            ..base
        },
        kind::CRYSTAL => Shockwave {
            effect: 0x24,
            damage_per_dist_sq: f32::from_bits(0x520b_b2c9),
            push: 500.0,
            speed: 5000.0,
            thickness: 1000.0,
            max_radius: 18000.0,
            ..base
        },
        kind::DREADNOUGHT => Shockwave {
            effect: -1,
            damage_const: 10.0,
            damage_per_dist: 1000.0,
            push: 2500.0,
            push_per_size_sq: 10.0,
            speed: 1500.0,
            thickness: 200.0,
            start_radius: 1000.0,
            max_radius: 10000.0,
            ..base
        },
        kind::NOMAD_PULSE => Shockwave {
            effect: -1,
            push: 5000.0,
            push_per_size_sq: 150.0,
            speed: 2500.0,
            thickness: 4000.0,
            start_radius: 4300.0,
            max_radius: 25000.0,
            ..base
        },
        _ => base,
    }
}

/// Damage scale of `swCreateFromKAS` waves (0x889f6c).
const KAS_DAMAGE_SCALE: f32 = 0.01;
/// Share of a Nomad pulse victim's maximum health it loses when first flung (0x889f70).
const NOMAD_DAMAGE_FRACTION: f32 = 0.2;
/// Spin a wave gives a ship (0x889f74).
const SPIN: f32 = 100.0;
/// The Dreadnought's wave flings ships lighter than this (0x889f78).
const DREADNOUGHT_FLING_MASS: f32 = 700.0;
/// One frame in this many shows a spark on the Dreadnought and Nomad waves (0x889fe0).
const SPARK_ODDS: u32 = 9;
/// Push factor on big-gun shells (0x889fe4).
const SHELL_PUSH: f32 = 0.01;
/// A multiplayer big-gun detonation within this distance of a Beast DFG frigate does half
/// damage (0x889fe8, 0x889fec).
const DFG_SHIELD_RANGE: f32 = 8000.0;
const DFG_SHIELD_FACTOR: f32 = 0.5;
/// `swRandomFloat`'s spread (0x889fa4).
const SPARK_SPREAD: f32 = 13.270_273;
/// `ApplyDamageToTarget`'s "how did I die" for wave damage.
const HOW_SHOCKWAVE: i32 = 0x29a;

/// The waves in flight (`swList`, 0x9036e0) and the big-gun shells they push
/// (`swSpecialShipList`, 0x9036f0).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shockwaves {
    pub waves: Vec<Shockwave>,
    pub special: Vec<BulletId>,
}

fn add(u: &mut Universe, mut w: Shockwave, centre: Vector, player: usize) -> usize {
    w.centre = centre;
    w.start_time = u.total_time_elapsed;
    w.player = player;
    w.started = false;
    u.shockwaves.waves.push(w);
    u.shockwaves.waves.len() - 1
}

/// `swCreate` (0x510c20): a wave of `kind` at `centre`, caused by `player`.
pub fn create(u: &mut Universe, kind: u32, centre: Vector, player: usize) {
    add(u, template(kind), centre, player);
}

/// `swCreateFromKAS` (0x510d10): a KAS wave (the big-gun template, almost no damage).
pub fn create_from_kas(u: &mut Universe, centre: Vector) {
    let i = add(u, template(kind::BIG_GUN), centre, 0);
    u.shockwaves.waves[i].damage_scale = KAS_DAMAGE_SCALE;
}

/// `swCreateCustom` (0x510da0): a big-gun detonation with its damage scaled. In multiplayer
/// games each Beast DFG frigate within range of the blast halves the damage.
pub fn create_custom(u: &mut Universe, centre: Vector, player: usize, damage_scale: f32) {
    let k = if u.single_player { kind::BIG_GUN_SP } else { kind::BIG_GUN };
    let i = add(u, template(k), centre, player);
    u.shockwaves.waves[i].damage_scale = damage_scale;
    if k == kind::BIG_GUN {
        for &sid in &u.ship_list {
            let Some(s) = u.ships.get(sid) else { continue };
            if s.ship_type == ShipType::bDFGFrigate && within_range(centre, s.posinfo.position, DFG_SHIELD_RANGE) {
                let w = &mut u.shockwaves.waves[i];
                w.damage_scale = st(ld(DFG_SHIELD_FACTOR) * ld(w.damage_scale));
            }
        }
    }
}

/// `RKOWithinRange` (0x5be7a0): `|a - b|^2 < range^2`.
pub fn within_range(a: Vector, b: Vector, range: f32) -> bool {
    within_range_sq(a, b, ld(range) * ld(range))
}

/// `RKOWithinRangeSq` (0x5be7f0): `|a - b|^2 < range_sq`.
pub fn within_range_sq(a: Vector, b: Vector, range_sq: f64) -> bool {
    let (dx, dy, dz) = (ld(a.x) - ld(b.x), ld(a.y) - ld(b.y), ld(a.z) - ld(b.z));
    (dx * dx + dy * dy) + dz * dz < range_sq
}

/// `swSafeDistanceGet` (0x510b00): how far from a blast counts as safe (the multiplayer
/// big-gun wave's final radius plus its thickness).
pub fn safe_distance() -> f64 {
    ld(13000.0) + ld(1000.0)
}

/// `swAnyActive` (0x511f10): a big-gun shell is in flight.
pub fn any_active(u: &Universe) -> bool {
    !u.shockwaves.special.is_empty()
}

/// `MothershipIsBigGunBullet` (0x5b7800).
pub fn is_big_gun_bullet(u: &Universe, b: BulletId) -> bool {
    u.bullets.get(b).is_some_and(|b| b.weapon == crate::bullet::weapon::BIGGUN)
}

/// `swAddSpecialShip` (0x5120f0).
pub fn add_special(u: &mut Universe, b: BulletId) {
    if is_big_gun_bullet(u, b) {
        u.shockwaves.special.push(b);
    }
}

/// `swRemoveSpecialShip` (0x512120).
pub fn remove_special(u: &mut Universe, b: BulletId) {
    if is_big_gun_bullet(u, b) {
        u.shockwaves.special.retain(|&x| x != b);
    }
}

/// `swRebuildSpecialShipList` (0x511f30): after loading, every big-gun shell in flight.
pub fn rebuild_special(u: &mut Universe) {
    let list: Vec<BulletId> = u.bullet_list.iter().copied().filter(|&b| is_big_gun_bullet(u, b)).collect();
    u.shockwaves.special.extend(list);
}

/// `FUN_004df830`: the force applied to the velocity at once (`F / m * dt`).
fn apply_force_now(u: &mut Universe, id: ShipId) {
    let dt = ld(u.phys_time_elapsed);
    let Some(oom) = u.statics.ship(u.ships.ship(id).ship_type).map(|s| s.header.one_over_mass) else { return };
    let s = u.ships.ship_mut(id);
    let f = s.posinfo.force;
    let fy = st(ld(oom) * ld(f.y) * dt);
    let fz = st(dt * ld(oom) * ld(f.z));
    let v = &mut s.posinfo.velocity;
    v.x = st(ld(oom) * ld(f.x) * dt + ld(v.x));
    v.y = st(ld(fy) + ld(v.y));
    v.z = st(ld(fz) + ld(v.z));
}

/// `swApplyToShip` (0x510f00).
fn apply_to_ship(u: &mut Universe, w: &Shockwave, id: ShipId) {
    let statics = u.statics.clone();
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let p = s.posinfo.position;
    let dx = st(ld(p.x) - ld(w.centre.x));
    let dy = st(ld(p.y) - ld(w.centre.y));
    let dz_ext = ld(p.z) - ld(w.centre.z);
    let dz = st(dz_ext);
    let dsq = st((dz_ext * ld(dz) + ld(dy) * ld(dy)) + ld(dx) * ld(dx));
    let r = ld(w.radius);
    if !(ld(dsq) < r * r) {
        return;
    }
    let inner = ld(w.radius) - ld(w.thickness);
    if !(ld(dsq) > inner * inner) {
        return;
    }
    let dist = st(fsqrt(ld(dsq)));
    let size = ld(stat.header.coll.coll_sphere_size);
    let size_term = st(size * size * ld(w.push_per_size_sq) * ld(w.speed));
    let mut n = Vector::new(dx, dy, dz);
    n.normalize();
    let mag = ld(w.push) * ld(w.speed) + ld(size_term);
    let f = Vector::new(st(mag * ld(n.x)), st(mag * ld(n.y)), st(mag * ld(n.z)));
    let mut extra = 0.0f32;
    if crate::disable::point_in_universe(p, u.world_bounds) {
        let s = u.ships.ship_mut(id);
        let fo = &mut s.posinfo.force;
        fo.x = st(ld(f.x) + ld(fo.x));
        fo.y = st(ld(f.y) + ld(fo.y));
        fo.z = st(ld(f.z) + ld(fo.z));
        let fling = w.kind == kind::NOMAD_PULSE || (w.kind == kind::DREADNOUGHT && ld(stat.header.mass) < ld(DREADNOUGHT_FLING_MASS));
        if fling {
            if w.kind == kind::NOMAD_PULSE && !s.disable.slowed {
                extra = st(ld(NOMAD_DAMAGE_FRACTION) * ld(stat.health.max_health));
            }
            crate::disable::apply_field_slowdown(u, id, 1.0, 1.0);
            u.ships.ship_mut(id).posinfo.force = f;
            apply_force_now(u, id);
        }
    }
    // Spin against the push.
    let k = ld(SPIN);
    let tx = -(k * ld(n.x));
    let ty = -st(k * ld(n.y));
    let tz = -st(k * ld(n.z));
    {
        let s = u.ships.ship_mut(id);
        let t = &mut s.rotinfo.torque;
        t.x = st(tx + ld(t.x));
        t.y = st(ld(ty) + ld(t.y));
        t.z = st(ld(tz) + ld(t.z));
    }
    let total = if ld(dist) > 0.0 {
        let inner = st(((ld(extra) + ld(w.damage_const)) + ld(w.damage_per_dist) / ld(dist)) + ld(w.damage_per_dist_sq) / ld(dsq));
        ld(w.damage_per_sqrt_dist) / fsqrt(ld(dist)) + ld(inner)
    } else {
        ld(extra)
    };
    let damage = st(total * ld(w.damage_scale));
    let s = u.ships.ship_mut(id);
    if w.kind == kind::BIG_GUN_SP {
        s.attributes |= 0x2_0000;
    }
    let spared = s.flags & crate::base::SOF_MODULE != 0 || (w.kind == kind::BIG_GUN_SP && s.attributes & 0x1_0000 != 0);
    if !spared {
        crate::damage::apply_damage_to_target(u, id, damage, 0, HOW_SHOCKWAVE, w.player as i32, None);
    }
    if u.ships.get(id).is_some() {
        crate::ships::ramming::stop_pushing(u, id);
    }
}

/// `swApplyToSpecialShip` (0x511f70): a big-gun shell inside the shell is pushed along.
fn apply_to_special(u: &mut Universe, w: &Shockwave, b: BulletId) {
    let dt = ld(u.phys_time_elapsed);
    let Some(bl) = u.bullets.get_mut(b) else { return };
    let p = bl.posinfo.position;
    let dx = st(ld(p.x) - ld(w.centre.x));
    let dy = st(ld(p.y) - ld(w.centre.y));
    let dz = st(ld(p.z) - ld(w.centre.z));
    let dsq = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
    if !(ld(dsq) < ld(w.radius) * ld(w.radius)) {
        return;
    }
    let inner = st(ld(w.radius) - ld(w.thickness));
    if !(ld(inner) * ld(inner) < ld(dsq)) {
        return;
    }
    let mut n = Vector::new(dx, dy, dz);
    n.normalize();
    let k = ld(w.speed) * ld(w.push);
    // The push goes through the shell's force (which is then cleared) into its velocity.
    let fo = bl.posinfo.force;
    let f = Vector::new(st(k * ld(n.x) + ld(fo.x)), st(k * ld(n.y) + ld(fo.y)), st(k * ld(n.z) + ld(fo.z)));
    bl.posinfo.force = Vector::ZERO;
    let s = ld(SHELL_PUSH);
    let v = &mut bl.posinfo.velocity;
    v.x = st(ld(st(ld(f.x) * s)) * dt + ld(v.x));
    v.y = st(ld(st(ld(f.y) * s)) * dt + ld(v.y));
    v.z = st(ld(st(ld(f.z) * s)) * dt + ld(v.z));
}

/// `swUpdate` (0x5111f0): grow the wave and apply it. False once it has reached its final
/// radius (it is then removed).
fn update(u: &mut Universe, i: usize) -> bool {
    let w = u.shockwaves.waves[i];
    if !(w.radius < w.max_radius) {
        return false;
    }
    let t = st((ld(u.total_time_elapsed) - ld(w.delay)) - ld(w.start_time));
    if !(ld(t) > 0.0) {
        return true;
    }
    {
        let wm = &mut u.shockwaves.waves[i];
        if !wm.started {
            wm.started = true;
            wm.radius = wm.start_radius;
        }
        wm.radius = st(ld(t) * ld(wm.speed) + ld(wm.start_radius));
    }
    let w = u.shockwaves.waves[i];
    let single_player = u.single_player;
    let list = u.ship_list.clone();
    for sid in list {
        let Some(s) = u.ships.get(sid) else { continue };
        let t = s.ship_type;
        let mut hit = true;
        if w.kind == kind::NOMAD_PULSE
            && (matches!(t, ShipType::dNomadMoon | ShipType::dNomadFieldEmitter | ShipType::dRKO)
                || s.ship_race == ShipRace::Beast
                || (single_player && s.player == 1))
        {
            hit = false;
        }
        if w.kind == kind::DREADNOUGHT && (is_ally(u, sid, w.player) || matches!(t, ShipType::dNomadMoon | ShipType::dNomadFieldEmitter | ShipType::dRKO)) {
            hit = false;
        }
        let untargetable = u.statics.ship(t).is_some_and(|st| st.capability_flags & 0x4000 != 0);
        if !untargetable && hit {
            apply_to_ship(u, &w, sid);
        }
    }
    let special = u.shockwaves.special.clone();
    for b in special {
        apply_to_special(u, &w, b);
    }
    if w.kind == kind::DREADNOUGHT || w.kind == kind::NOMAD_PULSE {
        // The sparks on the ring are presentation, but they draw from the particle stream.
        let n = SPARK_ODDS - 1;
        if u.randy.random(RAN_PARTICLE_STREAM).is_multiple_of(n) {
            u.randy.random(RAN_PARTICLE_STREAM);
            spark_angle(u);
        }
    }
    true
}

/// `swRandomFloat` (0x5117e0): an angle biased towards the equator (only its draws on the
/// particle stream matter here).
fn spark_angle(u: &mut Universe) -> f32 {
    let r = u.randy.random(RAN_PARTICLE_STREAM);
    let a = st((ld(SPARK_SPREAD) - 1.0) * f64::from(r) * ld(2.328_306_4e-10) + 1.0);
    let a = st((1.0 / (ld(a) * ld(a))) * ld(std::f32::consts::FRAC_PI_2));
    let r = u.randy.random(RAN_PARTICLE_STREAM);
    if f64::from(r) * ld(4.656_613e-10) - 1.0 > 0.0 {
        st(ld(a) + ld(std::f32::consts::FRAC_PI_2))
    } else {
        st(ld(std::f32::consts::FRAC_PI_2) - ld(a))
    }
}

/// `allianceIsShipAlly` (0x403b10): the ship belongs to `player` or one of its allies (a
/// crystal never does; ships with attribute 0x100 always do).
pub fn is_ally(u: &Universe, id: ShipId, player: usize) -> bool {
    let s = u.ships.ship(id);
    if matches!(s.ship_type, ShipType::dCrystal0 | ShipType::dCrystal1) {
        return false;
    }
    if s.player != player && s.attributes & 0x100 == 0 {
        return u.players.get(player).is_some_and(|p| p.is_ally(s.player));
    }
    true
}

/// `swUpdateAll` (0x5114f0): every wave, in creation order; finished waves are removed.
pub fn update_all(u: &mut Universe) {
    let mut i = 0;
    while i < u.shockwaves.waves.len() {
        if update(u, i) {
            i += 1;
        } else {
            u.shockwaves.waves.remove(i);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_match_the_tables() {
        let d = template(kind::DREADNOUGHT);
        assert_eq!((d.speed, d.thickness, d.start_radius, d.max_radius), (1500.0, 200.0, 1000.0, 10000.0));
        let n = template(kind::NOMAD_PULSE);
        assert_eq!((n.push, n.push_per_size_sq, n.max_radius), (5000.0, 150.0, 25000.0));
        let b = template(kind::BIG_GUN_SP);
        assert_eq!(b.damage_per_dist_sq, 49_999_998_976.0);
        assert_eq!(template(kind::MOTHERSHIP_DEATH).delay, f32::from_bits(0x4053_3333));
    }

    #[test]
    fn within_range_is_strict() {
        assert!(within_range(Vector::new(0.0, 0.0, 0.0), Vector::new(3.0, 4.0, 0.0), 5.1));
        assert!(!within_range(Vector::new(0.0, 0.0, 0.0), Vector::new(3.0, 4.0, 0.0), 5.0));
    }
}
