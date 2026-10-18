//! Harvestable resources: asteroids, dust clouds and nebulae (`universeStaticInit`,
//! `InitStatDustCloudInfo`, `InitStatNebulaInfo`).

use hwc_formats::script::ScriptFile;

use super::cscan::{decomma, parse_bool, scan, scan_f32, scan_i32, Scanned};
use super::enums::{AsteroidType, DustCloudType, NebulaType};
use super::header::{zero_rect_info, HeaderKeys, HealthInfo, StaticHeader};
use hwc_math::trig::{fcos_f32, fsin_f32};
use hwc_math::{Matrix, Vector};
use super::Loader;

/// Fields common to the resource kinds (`ResourceStaticInfo`).
#[derive(Debug, Clone, Default)]
pub struct ResourceInfo {
    pub header: StaticHeader,
    pub health: HealthInfo,
    /// +0xb4 `resourcevalue`: resource units the object holds.
    pub resource_value: i32,
    /// +0xb8 `harvestableByMultipleShips`: how many collectors may work it at once.
    pub harvestable_by_multiple_ships: i32,
    /// Source file.
    pub file: String,
    /// Lines no table binds.
    pub unbound: Vec<String>,
}

impl ResourceInfo {
    fn apply(&mut self, key: &str, raw: &str) -> bool {
        match key {
            "resourcevalue" => {
                if let Some(v) = scan_i32(raw) {
                    self.resource_value = v;
                }
            }
            "harvestableByMultipleShips" => {
                if let Some(v) = scan_i32(raw) {
                    self.harvestable_by_multiple_ships = v;
                }
            }
            _ => return self.health.apply(key, raw),
        }
        true
    }
}

/// One fragment an asteroid breaks into (`AsteroidBreakInfo` line, 0x44 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct AsteroidBreakInfo {
    /// Asteroid type of the fragment (raw value; normally an [`AsteroidType`] index).
    pub asteroid_type: i32,
    /// Offset from the parent.
    pub position: Vector,
    /// Orientation: rotation about x by `rotX`, then about y by `rotY` twice. The original
    /// applies the y rotation twice and ignores `rotZ`; reproduced as is.
    pub coord_sys: Matrix,
    /// Separation speed.
    pub speed: f32,
    /// Spin about x, y, z.
    pub rot_speed: Vector,
}

/// An asteroid (`AsteroidStaticInfo`, 200 bytes).
#[derive(Debug, Clone)]
pub struct AsteroidStaticInfo {
    pub info: ResourceInfo,
    /// +0xbc.
    pub asteroid_type: AsteroidType,
    /// +0xc0 / +0xc4 `NUMBER_OF_ASTEROID_PIECES` / `AsteroidBreakInfo` lines (Cataclysm).
    pub break_pieces: Vec<AsteroidBreakInfo>,
}

/// A dust cloud or nebula (`DustCloudStaticInfo` / `NebulaStaticInfo`, 200 bytes).
#[derive(Debug, Clone)]
pub struct CloudStaticInfo<T> {
    pub info: ResourceInfo,
    /// +0xbc.
    pub kind: T,
    /// +0xc0 `dustcloudCanShrink` / `nebulaCanShrink`.
    pub can_shrink: bool,
    /// +0xc4 `dustcloudMinShrink` / `nebulaMinShrink`.
    pub min_shrink: f32,
}

pub type DustCloudStaticInfo = CloudStaticInfo<DustCloudType>;
pub type NebulaStaticInfo = CloudStaticInfo<NebulaType>;

/// Loads a resource file through the resource keyword table.
fn read_resource(
    ld: &mut Loader,
    dir: &str,
    name: &str,
    keys: HeaderKeys,
    mut extra: impl FnMut(&str, &str) -> bool,
) -> Option<(ResourceInfo, ScriptFile)> {
    let file = format!("{dir}{name}.shp");
    if !ld.src.exists(&file) {
        return None;
    }
    let script = ld.read_script(&file)?;
    let mut r = ResourceInfo { file, ..Default::default() };
    for e in script.entries() {
        let (key, raw) = (e.key.as_str(), e.raw.as_str());
        let bound = ld.apply_header_files(&mut r.header, key, raw, dir)
            || r.header.apply(key, raw, keys)
            || r.apply(key, raw)
            || extra(key, raw);
        if !bound {
            r.unbound.push(key.to_owned());
        }
    }
    let tw = ld.tweaks.clone();
    r.header.precalc(&tw);
    r.health.precalc(&tw);
    Some((r, script))
}

const ASTEROID_KEYS: HeaderKeys = HeaderKeys {
    coll_sphere_size: false,
    immobile: true,
    right_of_way: true,
    coll_modifiers: true,
    coll_box_scale: true,
};

const CLOUD_KEYS: HeaderKeys = HeaderKeys {
    coll_sphere_size: false,
    immobile: true,
    right_of_way: false,
    coll_modifiers: true,
    coll_box_scale: true,
};

/// `Resources\Asteroids\AsteroidN.shp`.
pub(crate) fn load_asteroid(ld: &mut Loader, t: AsteroidType) -> Option<AsteroidStaticInfo> {
    let breaks = ["NUMBER_OF_ASTEROID_PIECES", "AsteroidBreakInfo"];
    let (mut info, script) =
        read_resource(ld, "Resources\\Asteroids\\", t.name(), ASTEROID_KEYS, |k, _| breaks.contains(&k))?;
    if t == AsteroidType::Asteroid0 {
        zero_rect_info(&mut info.header.coll);
    }
    Some(AsteroidStaticInfo { info, asteroid_type: t, break_pieces: read_break_info(&script) })
}

/// The asteroid break reader: `NUMBER_OF_ASTEROID_PIECES` (a count below 1 stops the scan),
/// then that many `AsteroidBreakInfo` lines.
fn read_break_info(script: &ScriptFile) -> Vec<AsteroidBreakInfo> {
    let mut count: Option<usize> = None;
    let mut pieces = Vec::new();
    for e in script.entries() {
        match count {
            None => {
                if e.key == "NUMBER_OF_ASTEROID_PIECES" {
                    let n = scan_i32(&e.raw).unwrap_or(0);
                    if n < 1 {
                        break;
                    }
                    count = Some(n as usize);
                }
            }
            Some(n) => {
                if e.key != "AsteroidBreakInfo" {
                    continue;
                }
                let v = scan(&decomma(&e.raw), "%d %f %f %f %f %f %f %f %f %f %f");
                let fl = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
                let (rx, ry) = (fl(4), fl(5));
                // The CRT's cos/sin compile to FCOS/FSIN on the loaded float.
                let (cx, sx) = (fcos_f32(rx) as f32, fsin_f32(rx) as f32);
                let (cy, sy) = (fcos_f32(ry) as f32, fsin_f32(ry) as f32);
                let roty = Matrix::rot_about_y(cy, sy);
                let m = Matrix::rot_about_x(cx, sx).mul_mat(&roty).mul_mat(&roty);
                pieces.push(AsteroidBreakInfo {
                    asteroid_type: v.first().and_then(Scanned::int).unwrap_or(0),
                    position: Vector::new(fl(1), fl(2), fl(3)),
                    coord_sys: m,
                    speed: fl(7),
                    rot_speed: Vector::new(fl(8), fl(9), fl(10)),
                });
                if pieces.len() == n {
                    break;
                }
            }
        }
    }
    pieces
}

/// Shared tail of the cloud loaders: zeroed box and every sphere radius set to `size`.
fn set_cloud_size(info: &mut ResourceInfo, size: f32) {
    let c = &mut info.header.coll;
    zero_rect_info(c);
    c.coll_sphere_size_sqr = size * size;
    c.original_coll_sphere_size = size;
    c.approx_coll_sphere_size = size;
    c.avoid_coll_sphere_size = size;
    c.avoid_coll_sphere_pad = size;
    c.coll_sphere_size = size;
}

fn load_cloud<T: Copy>(
    ld: &mut Loader,
    dir: &str,
    name: &str,
    kind: T,
    shrink_keys: (&str, &str),
    size: f32,
) -> Option<CloudStaticInfo<T>> {
    let (mut can_shrink, mut min_shrink) = (false, 0.0f32);
    let (mut info, _) = read_resource(ld, dir, name, CLOUD_KEYS, |k, raw| {
        if k == shrink_keys.0 {
            can_shrink = parse_bool(raw);
        } else if k == shrink_keys.1 {
            if let Some(v) = scan_f32(raw) {
                min_shrink = v;
            }
        } else {
            return false;
        }
        true
    })?;
    set_cloud_size(&mut info, size);
    Some(CloudStaticInfo { info, kind, can_shrink, min_shrink })
}

/// `Resources\DustClouds\DustCloudN.shp`; size `DUSTCLOUD_RADIAL_SCALE * DUSTCLOUD_RADII[N]`.
pub(crate) fn load_dust_cloud(ld: &mut Loader, t: DustCloudType) -> Option<DustCloudStaticInfo> {
    let size = ld.tweaks.dustcloud_radial_scale * ld.tweaks.dustcloud_radii[t.index()];
    load_cloud(
        ld,
        "Resources\\DustClouds\\",
        t.name(),
        t,
        ("dustcloudCanShrink", "dustcloudMinShrink"),
        size,
    )
}

/// `Resources\Nebulae\Nebula0.shp`; size `NEB_RADIUS`.
pub(crate) fn load_nebula(ld: &mut Loader, t: NebulaType) -> Option<NebulaStaticInfo> {
    let size = ld.tweaks.neb_radius;
    load_cloud(ld, "Resources\\Nebulae\\", t.name(), t, ("nebulaCanShrink", "nebulaMinShrink"), size)
}
