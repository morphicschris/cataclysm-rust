//! Docking points (`NUMBER_OF_DOCK_POINTS` / `DockPoint` lines, `scriptSetDockStatics`) completed
//! from the `Dok` chunks of the mesh extensions (`mexGetDockStaticInfo`).

use hwc_formats::mex::{ChunkData, Mex, Point, UNSET_PATTERN};
use hwc_formats::script::ScriptFile;

use super::cscan::{decomma, scan, scan_i32, Scanned};
use super::enums::DockPointType;
use hwc_math::Vector;

use super::v3;
use super::tweaks::DEG_TO_RAD;

/// `DockStaticPoint` (0x4c bytes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DockStaticPoint {
    /// +0x00. `None` when the type name was not recognised (the original stores -1).
    pub dock_type: Option<DockPointType>,
    /// +0x04. Index of the point within the ship.
    pub dock_index: usize,
    /// +0x08. Position from the mex.
    pub position: Vector,
    /// +0x14. Normalised cone direction; (1, 0, 0) when missing or zero.
    pub cone_normal: Vector,
    /// +0x20. Cosine of (cone + edge angle) of the mex point; cos(90 degrees) when the chunk is
    /// missing.
    pub cone_angle: f32,
    /// +0x24 / +0x28 / +0x2c: third to fifth fields of the `DockPoint` line.
    pub fly_away_dist: f32,
    pub min_dist: f32,
    pub max_dist: f32,
    /// +0x30 / +0x34: approach directions (0 = heading, 1 = up, 2 = attach point; the original
    /// replaces an equal pair with 0 and 2).
    pub heading_direction: i32,
    pub up_direction: i32,
    /// +0x38. Point name (first field of the line), matched against the mex.
    pub name: String,
    /// True when a matching `Dok`/`Dock` or `Sal`/`SALVAGE` chunk was found.
    pub mex_point_found: bool,
}

/// `DockStaticInfo`: a per-type index (bytes 0..0x23: first point of each type, 0xff for none;
/// 0x23..0x46: points per type), the points, and the `center` point used by docking code.
#[derive(Debug, Clone, PartialEq)]
pub struct DockStaticInfo {
    /// First point of each type, 0xff for none. Left all zero (as the original's cleared
    /// allocation) when no `DockPoint` line was read.
    pub first_of_type: [u8; DockPointType::COUNT],
    pub count_of_type: [u8; DockPointType::COUNT],
    /// Exactly `NUMBER_OF_DOCK_POINTS` entries; slots without a `DockPoint` line stay default.
    pub points: Vec<DockStaticPoint>,
    /// Index of the point named `center` (`dockFindDockStaticPoint`).
    pub center: Option<usize>,
}

impl Default for DockStaticInfo {
    fn default() -> Self {
        Self {
            first_of_type: [0; DockPointType::COUNT],
            count_of_type: [0; DockPointType::COUNT],
            points: Vec::new(),
            center: None,
        }
    }
}

impl DockStaticInfo {
    /// Reads the dock lines of a ship file. `None` without a positive `NUMBER_OF_DOCK_POINTS`.
    pub(crate) fn scan(script: &ScriptFile, warnings: &mut Vec<String>) -> Option<Self> {
        let mut info: Option<DockStaticInfo> = None;
        let mut filled = 0usize;
        for e in script.entries() {
            match info.as_mut() {
                None => {
                    if e.key == "NUMBER_OF_DOCK_POINTS" {
                        let n = scan_i32(&e.raw).unwrap_or(0);
                        if n < 1 {
                            return None;
                        }
                        info = Some(DockStaticInfo {
                            points: vec![DockStaticPoint::default(); n as usize],
                            ..Default::default()
                        });
                    }
                }
                Some(d) => {
                    if e.key != "DockPoint" {
                        continue;
                    }
                    let p = &mut d.points[filled];
                    let v = scan(&decomma(&e.raw), "%s%s%f%f%f%d%d");
                    let typ = v.get(1).and_then(Scanned::str);
                    if let Some(name) = v.first().and_then(Scanned::str) {
                        p.name = name.to_owned();
                    }
                    if let Some(x) = v.get(2).and_then(Scanned::float) {
                        p.fly_away_dist = x;
                    }
                    if let Some(x) = v.get(3).and_then(Scanned::float) {
                        p.min_dist = x;
                    }
                    if let Some(x) = v.get(4).and_then(Scanned::float) {
                        p.max_dist = x;
                    }
                    if let Some(x) = v.get(5).and_then(Scanned::int) {
                        p.heading_direction = x;
                    }
                    if let Some(x) = v.get(6).and_then(Scanned::int) {
                        p.up_direction = x;
                    }
                    p.dock_type = typ.and_then(DockPointType::from_name);
                    if p.heading_direction == p.up_direction {
                        warnings.push(format!("dock point {} has equal heading and up", p.name));
                        p.heading_direction = 0;
                        p.up_direction = 2;
                    }
                    filled += 1;
                    if filled >= d.points.len() {
                        break;
                    }
                }
            }
        }
        let mut info = info?;
        if filled > 0 {
            info.first_of_type = [0xff; DockPointType::COUNT];
            for (i, p) in info.points.iter().enumerate() {
                if let Some(t) = p.dock_type {
                    let slot = &mut info.first_of_type[t.index()];
                    if *slot == 0xff {
                        *slot = i as u8;
                    }
                    info.count_of_type[t.index()] = info.count_of_type[t.index()].wrapping_add(1);
                }
            }
        }
        Some(info)
    }

    /// `mexGetDockStaticInfo` and the `center` lookup.
    pub(crate) fn apply_mex(&mut self, mex: &Mex, warnings: &mut Vec<String>) {
        for (i, p) in self.points.iter_mut().enumerate() {
            p.dock_index = i;
            match find_dock_point(mex, &p.name) {
                Some(m) => {
                    p.mex_point_found = true;
                    p.position = v3(m.position);
                    p.cone_normal = v3(m.normal);
                    if p.cone_normal.is_zero() {
                        p.cone_normal.x = 1.0;
                    } else {
                        p.cone_normal.normalize();
                    }
                    let a = |x: Option<f32>| f64::from(x.unwrap_or(f32::from_bits(UNSET_PATTERN)));
                    p.cone_angle = ((a(m.edge_angle) + a(m.cone_angle)) * f64::from(DEG_TO_RAD)).cos() as f32;
                }
                None => {
                    p.position = Vector::ZERO;
                    p.cone_normal = Vector::new(1.0, 0.0, 0.0);
                    p.cone_angle = f64::from(std::f32::consts::FRAC_PI_2).cos() as f32;
                    warnings.push(format!("dock point {} not in mex", p.name));
                }
            }
        }
        self.center = self.find("center");
    }

    /// `dockFindDockStaticPoint`: index of the point with this name (ASCII case ignored).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.points.iter().position(|p| p.name.eq_ignore_ascii_case(name))
    }
}

/// `mexGetDockChunk`: a `Dok` chunk named `Dock`, or a `Sal` chunk named `SALVAGE`, whose point
/// name matches.
fn find_dock_point(mex: &Mex, name: &str) -> Option<Point> {
    mex.chunks.iter().find_map(|c| match &c.data {
        ChunkData::Dock { point, name: n }
            if c.tag.eq_ignore_ascii_case("Dok") && c.name.eq_ignore_ascii_case("Dock") && n.eq_ignore_ascii_case(name) =>
        {
            Some(*point)
        }
        ChunkData::Salvage { point, name: n }
            if c.tag.eq_ignore_ascii_case("Sal") && c.name.eq_ignore_ascii_case("SALVAGE") && n.eq_ignore_ascii_case(name) =>
        {
            Some(*point)
        }
        _ => None,
    })
}
