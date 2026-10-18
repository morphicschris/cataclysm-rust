//! The part every static object shares (`StaticHeader`, 0xa4 bytes in Cataclysm) and the health
//! and guidance blocks that follow it in ships, derelicts and missiles, together with the
//! derivations the original performs on them while loading.

use std::sync::Arc;

use hwc_formats::mex::{ChunkData, Mex};

use super::cscan::{parse_bool, scan_f32, scan_i32};
use super::lod::LodInfo;
use hwc_math::Vector;

use super::v3;
use super::tweaks::Tweaks;

/// Collision geometry (`StaticCollInfo`, header offset 0x34). Cataclysm inserts
/// `coll_rect_centre` after the rectangle offset.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StaticCollInfo {
    /// +0x34. Rough sphere radius used for approximate tests (`approxcollmodifier` applied).
    pub approx_coll_sphere_size: f32,
    /// +0x38. Radius used for collision avoidance (`avoidcollmodifier` applied).
    pub avoid_coll_sphere_size: f32,
    /// +0x3c. Avoidance radius with padding.
    pub avoid_coll_sphere_pad: f32,
    /// +0x40 `approxcollmodifier`.
    pub approx_coll_modifier: f32,
    /// +0x44 `avoidcollmodifier`.
    pub avoid_coll_modifier: f32,
    /// +0x48. Sphere radius before any runtime scaling.
    pub original_coll_sphere_size: f32,
    /// +0x4c. Bounding sphere radius (`collspheresize` in ship files, normally from the mesh).
    pub coll_sphere_size: f32,
    /// +0x50.
    pub coll_sphere_size_sqr: f32,
    /// +0x54. Sphere centre in object coordinates.
    pub coll_sphere_offset: Vector,
    /// +0x60. Minimum corner of the collision box (x = up, y = right, z = forward).
    pub coll_rect_offset: Vector,
    /// +0x6c (Cataclysm). Computed as `(corner + length) / 2` per axis, exactly as the original
    /// does (this is not the geometric centre unless the corner is at the origin).
    pub coll_rect_centre: Vector,
    /// +0x78.
    pub up_length: f32,
    /// +0x7c.
    pub right_length: f32,
    /// +0x80.
    pub forward_length: f32,
    /// +0x84. Box diagonal.
    pub diagonal_length: f32,
    /// +0x88 `preciseSelection`: use precise selection at LODs at or below this (0xff = never).
    pub precise_selection: i32,
    /// +0x8c `collBoxScaleForward`.
    pub coll_box_scale_forward: f32,
    /// +0x90 `collBoxScaleUp`.
    pub coll_box_scale_up: f32,
    /// +0x94 `collBoxScaleRight`.
    pub coll_box_scale_right: f32,
    /// +0x98..+0xa0 `collBoxOffsetX/Y/Z`.
    pub coll_box_offset: Vector,
}

/// `StaticHeader`.
#[derive(Debug, Clone, Default)]
pub struct StaticHeader {
    /// `LODFile` as written.
    pub lod_file: Option<String>,
    /// +0x00. The level-of-detail table (`None` if the file never named one).
    pub lod: Option<Arc<LodInfo>>,
    /// `pMexData` as written.
    pub mex_file: Option<String>,
    /// +0x04. Mesh extensions; the default ship's when the named file is missing.
    pub mex: Option<Arc<Mex>>,
    /// True when `mex` is the default ship's fallback.
    pub mex_is_default: bool,
    /// +0x08 `mass`.
    pub mass: f32,
    /// +0x0c..+0x14 `momentOfInertiaX/Y/Z`.
    pub moment_of_inertia: Vector,
    /// +0x18 `1 / mass` (infinite for massless objects, as in the original).
    pub one_over_mass: f32,
    /// +0x1c..+0x24.
    pub one_over_moment_of_inertia: Vector,
    /// +0x28 `maxvelocity` (times `GLOBAL_SHIP_SPEED_MODIFIER` for guided objects).
    pub max_velocity: f32,
    /// +0x2c `maxrot`.
    pub max_rot: f32,
    /// +0x31 `immobile`.
    pub immobile: bool,
    /// +0x32 `rightOfWay`.
    pub right_of_way: i8,
    /// +0x34.
    pub coll: StaticCollInfo,
}

/// Health block (+0xa4).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HealthInfo {
    /// +0xa4 `maxhealth`.
    pub max_health: f32,
    /// +0xa8.
    pub one_over_max_health: f32,
    /// +0xac `minCollDamage`.
    pub min_coll_damage: f32,
    /// +0xb0 `maxCollDamage`.
    pub max_coll_damage: f32,
}

/// Guidance block (+0xb4): thrust, rotational strength and turn speed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GuidanceInfo {
    /// +0xb4 `thruststrength[TRANS_*]` (up, down, right, left, forward, back); after loading
    /// these are forces (`acceleration * mass * GLOBAL_SHIP_SPEED_MODIFIER`).
    pub thrust_strength: [f32; 6],
    /// +0xcc `rotstrength[ROT_*]` (yaw left/right, pitch up/down, roll right/left); after
    /// loading these are torques (multiplied by the matching moment of inertia).
    pub rot_strength: [f32; 6],
    /// +0xe4 `turnspeed[TURN_*]` (yaw, pitch, roll).
    pub turn_speed: [f32; 3],
}

/// Which header keywords a given object table binds.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeaderKeys {
    pub coll_sphere_size: bool,
    pub immobile: bool,
    pub right_of_way: bool,
    pub coll_modifiers: bool,
    /// The mine table's `collBoxScale*` entries point outside the structure in the original
    /// (they never reach the mine's statics), so mines leave these unbound.
    pub coll_box_scale: bool,
}

impl HeaderKeys {
    pub const SHIP: Self = Self {
        coll_sphere_size: true,
        immobile: true,
        right_of_way: true,
        coll_modifiers: true,
        coll_box_scale: true,
    };
}

const THRUST_KEYS: [&str; 6] = [
    "thruststrength[TRANS_UP]",
    "thruststrength[TRANS_DOWN]",
    "thruststrength[TRANS_RIGHT]",
    "thruststrength[TRANS_LEFT]",
    "thruststrength[TRANS_FORWARD]",
    "thruststrength[TRANS_BACK]",
];
const ROT_KEYS: [&str; 6] = [
    "rotstrength[ROT_YAWLEFT]",
    "rotstrength[ROT_YAWRIGHT]",
    "rotstrength[ROT_PITCHUP]",
    "rotstrength[ROT_PITCHDOWN]",
    "rotstrength[ROT_ROLLRIGHT]",
    "rotstrength[ROT_ROLLLEFT]",
];
const TURN_KEYS: [&str; 3] = ["turnspeed[TURN_YAW]", "turnspeed[TURN_PITCH]", "turnspeed[TURN_ROLL]"];

fn set_f32(target: &mut f32, raw: &str) {
    if let Some(v) = scan_f32(raw) {
        *target = v;
    }
}

impl StaticHeader {
    /// Applies one of the plain header keywords (`LODFile` and `pMexData` are handled by the
    /// loader). Returns false when the keyword is not a header keyword of this table.
    pub(crate) fn apply(&mut self, key: &str, raw: &str, keys: HeaderKeys) -> bool {
        let c = &mut self.coll;
        match key {
            "mass" => set_f32(&mut self.mass, raw),
            "momentOfInertiaX" => set_f32(&mut self.moment_of_inertia.x, raw),
            "momentOfInertiaY" => set_f32(&mut self.moment_of_inertia.y, raw),
            "momentOfInertiaZ" => set_f32(&mut self.moment_of_inertia.z, raw),
            "collBoxScaleRight" if keys.coll_box_scale => set_f32(&mut c.coll_box_scale_right, raw),
            "collBoxScaleUp" if keys.coll_box_scale => set_f32(&mut c.coll_box_scale_up, raw),
            "collBoxScaleForward" if keys.coll_box_scale => set_f32(&mut c.coll_box_scale_forward, raw),
            "collBoxOffsetX" => set_f32(&mut c.coll_box_offset.x, raw),
            "collBoxOffsetY" => set_f32(&mut c.coll_box_offset.y, raw),
            "collBoxOffsetZ" => set_f32(&mut c.coll_box_offset.z, raw),
            "immobile" if keys.immobile => self.immobile = parse_bool(raw),
            "rightOfWay" if keys.right_of_way => {
                if let Some(v) = scan_i32(raw) {
                    self.right_of_way = v as i8;
                }
            }
            "collspheresize" if keys.coll_sphere_size => set_f32(&mut c.coll_sphere_size, raw),
            "approxcollmodifier" if keys.coll_modifiers => set_f32(&mut c.approx_coll_modifier, raw),
            "avoidcollmodifier" if keys.coll_modifiers => set_f32(&mut c.avoid_coll_modifier, raw),
            "maxvelocity" => set_f32(&mut self.max_velocity, raw),
            "maxrot" => set_f32(&mut self.max_rot, raw),
            "preciseSelection" => {
                if let Some(v) = scan_i32(raw) {
                    c.precise_selection = v;
                }
            }
            _ => return false,
        }
        true
    }

    /// `precalcStaticHeader`: collision geometry from the mesh extensions, then the collision
    /// modifiers, avoidance padding and reciprocal mass terms.
    pub(crate) fn precalc(&mut self, tweaks: &Tweaks) {
        if let Some(mex) = self.mex.clone() {
            set_collision_info(&mut self.coll, &mex);
        }
        let c = &mut self.coll;
        if c.approx_coll_modifier != 0.0 {
            c.approx_coll_sphere_size *= c.approx_coll_modifier;
        }
        if c.avoid_coll_modifier != 0.0 {
            c.avoid_coll_sphere_size *= c.avoid_coll_modifier;
        }
        let pad = if c.avoid_coll_sphere_size > 1000.0 {
            tweaks.avoid_obj_padding_scale_big
        } else {
            tweaks.avoid_obj_padding_scale
        };
        c.avoid_coll_sphere_pad = pad * c.avoid_coll_sphere_size;
        self.one_over_mass = 1.0 / self.mass;
        let moi = self.moment_of_inertia;
        self.one_over_moment_of_inertia = Vector::new(1.0 / moi.x, 1.0 / moi.y, 1.0 / moi.z);
    }
}

/// `mexSetCollisionInfo`: sphere from the `Col0` chunk (scaled by the largest box scale, offset
/// by `collBoxOffset`), box from `Rct0` (each axis scaled about its centre) or, without one, a
/// cube around the sphere. A box whose half-diagonal (+10%) exceeds the sphere enlarges every
/// sphere radius.
pub(crate) fn set_collision_info(c: &mut StaticCollInfo, mex: &Mex) {
    let fix = |s: f32| if s == 0.0 { 1.0 } else { s };
    let scale_forward = fix(c.coll_box_scale_forward);
    let scale_up = fix(c.coll_box_scale_up);
    let scale_right = fix(c.coll_box_scale_right);
    let max_scale = scale_up.max(scale_forward).max(scale_right);

    let sphere = mex.chunks.iter().find_map(|ch| match ch.data {
        ChunkData::CollisionSphere { centre, radius, .. }
            if ch.tag.eq_ignore_ascii_case("Col") && ch.name.eq_ignore_ascii_case("Col0") =>
        {
            Some((centre, radius))
        }
        _ => None,
    });
    if let Some((centre, radius)) = sphere {
        let r = (f64::from(max_scale) * f64::from(radius)) as f32;
        c.original_coll_sphere_size = r;
        c.coll_sphere_size = r;
        c.approx_coll_sphere_size = r;
        c.avoid_coll_sphere_size = (f64::from(max_scale) * f64::from(radius) + 3.0) as f32;
        c.coll_sphere_offset = v3(centre) + c.coll_box_offset;
        c.coll_sphere_size_sqr = c.coll_sphere_size * c.coll_sphere_size;
    }

    let rect = mex.chunks.iter().find_map(|ch| match ch.data {
        ChunkData::CollisionBox { corner, extent, .. }
            if ch.tag.eq_ignore_ascii_case("Rct") && ch.name.eq_ignore_ascii_case("Rct0") =>
        {
            Some((corner, extent))
        }
        _ => None,
    });
    match rect {
        Some((corner, extent)) => {
            let mut offset = corner;
            let mut len = extent;
            for (axis, scale) in [(0, scale_up), (1, scale_right), (2, scale_forward)] {
                if scale != 1.0 {
                    let old = len[axis];
                    let new = scale * old;
                    len[axis] = new;
                    offset[axis] -= ((f64::from(new) - f64::from(old)) * 0.5) as f32;
                }
            }
            c.coll_rect_offset = v3(offset);
            c.up_length = len[0];
            c.right_length = len[1];
            c.forward_length = len[2];
            let half = len.map(|l| f64::from(l) * 0.5);
            let reach = ((half[0] * half[0] + half[1] * half[1] + half[2] * half[2]).sqrt() * 1.1) as f32;
            if c.coll_sphere_size < reach {
                c.original_coll_sphere_size = reach;
                c.coll_sphere_size = reach;
                c.approx_coll_sphere_size = reach;
                c.avoid_coll_sphere_size = reach;
                c.avoid_coll_sphere_pad = reach;
                c.coll_sphere_size_sqr = reach * reach;
            }
        }
        None => {
            let half = c.coll_sphere_size * 0.5;
            let o = c.coll_sphere_offset;
            c.coll_rect_offset = Vector::new(o.x - half, o.y - half, o.z - half);
            c.up_length = scale_up * c.coll_sphere_size;
            c.right_length = scale_right * c.coll_sphere_size;
            c.forward_length = scale_forward * c.coll_sphere_size;
        }
    }
    let l = [c.up_length, c.right_length, c.forward_length].map(f64::from);
    c.diagonal_length = (l[0] * l[0] + (l[1] * l[1] + l[2] * l[2])).sqrt() as f32;
    let lens = [c.up_length, c.right_length, c.forward_length];
    let o = c.coll_rect_offset;
    let mid = |len: f32, corner: f32| ((f64::from(len) + f64::from(corner)) * 0.5) as f32;
    c.coll_rect_centre = Vector::new(mid(lens[0], o.x), mid(lens[1], o.y), mid(lens[2], o.z));
}

/// `collZeroRectInfo`: clears the box corner and lengths (used for asteroid 0, dust clouds and
/// nebulae).
pub(crate) fn zero_rect_info(c: &mut StaticCollInfo) {
    c.coll_rect_offset = Vector::ZERO;
    c.up_length = 0.0;
    c.right_length = 0.0;
    c.forward_length = 0.0;
}

impl HealthInfo {
    pub(crate) fn apply(&mut self, key: &str, raw: &str) -> bool {
        match key {
            "maxhealth" => set_f32(&mut self.max_health, raw),
            "minCollDamage" => set_f32(&mut self.min_coll_damage, raw),
            "maxCollDamage" => set_f32(&mut self.max_coll_damage, raw),
            _ => return false,
        }
        true
    }

    /// Default collision damage (when both are zero) and the health reciprocal.
    pub(crate) fn precalc(&mut self, tweaks: &Tweaks) {
        if self.min_coll_damage == 0.0 && self.max_coll_damage == 0.0 {
            self.min_coll_damage = self.max_health * tweaks.coll_damage_default_min_fraction;
            self.max_coll_damage = self.max_health * tweaks.coll_damage_default_max_fraction;
        }
        if self.max_health != 0.0 {
            self.one_over_max_health = 1.0 / self.max_health;
        }
    }
}

impl GuidanceInfo {
    pub(crate) fn apply(&mut self, key: &str, raw: &str) -> bool {
        if let Some(i) = THRUST_KEYS.iter().position(|k| *k == key) {
            set_f32(&mut self.thrust_strength[i], raw);
        } else if let Some(i) = ROT_KEYS.iter().position(|k| *k == key) {
            set_f32(&mut self.rot_strength[i], raw);
        } else if let Some(i) = TURN_KEYS.iter().position(|k| *k == key) {
            set_f32(&mut self.turn_speed[i], raw);
        } else {
            return false;
        }
        true
    }

    /// `precalcStaticInfoGuidance`: thrust becomes force, rotational strength becomes torque,
    /// and the header's top speed takes the global speed modifier.
    pub(crate) fn precalc(&mut self, header: &mut StaticHeader, tweaks: &Tweaks) {
        let speed = f64::from(tweaks.global_ship_speed_modifier);
        for t in &mut self.thrust_strength {
            *t = (speed * f64::from(*t) * f64::from(header.mass)) as f32;
        }
        header.max_velocity = (speed * f64::from(header.max_velocity)) as f32;
        let moi = header.moment_of_inertia;
        for (i, r) in self.rot_strength.iter_mut().enumerate() {
            *r *= [moi.x, moi.y, moi.z][i / 2];
        }
    }
}
