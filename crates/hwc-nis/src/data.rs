//! A loaded NIS (`nisLoad` 0x4cfe30): the motion paths of the `.nis` file as the game holds
//! them in memory, and the events compiled from its `.script`.
//!
//! The game reads the file body into one allocation and starts its spline curves directly on
//! the arrays in it (`bsCurveStartPrealloced`). Two details of that matter for the motion:
//!
//! * the heading, pitch and bank arrays are converted from degrees to radians in place;
//! * on the last segment of a curve `bsCurveUpdate` reads one element past the end of the
//!   value array and of the time array, which is whatever the file has next.
//!
//! So the body is kept as a byte image, converted in place like the original, and every curve
//! takes its arrays from the image with that one extra element. Reads past the end of the
//! body (into memory the game did not allocate for it) come back as 0.
//!
//! Object paths are sorted by their parent's name offset as the loader does (`qsort` with
//! `nisCompareFunc`); a stable sort is used here, so paths with the same parent keep their file
//! order where MSVC's `qsort` may not.

use std::fmt;

use hwc_formats::nis::{split_instance, NisScript};
use hwc_math::spline::{SplineCurve, Tcb};

use crate::events::{self, Event};

/// `NIS_Identifier`.
pub const IDENTIFIER: &[u8; 12] = b"Crannnberry\0";
/// `NIS_VersionNumber`.
pub const VERSION: u32 = 0x101;
/// Header plus `NIS_HeaderExtra`: where the body starts.
pub const BODY_START: usize = 0x84;
/// Size of an object (and light) path record.
pub const OBJECT_PATH_LEN: usize = 0x38;
/// Size of a camera path record.
pub const CAMERA_PATH_LEN: usize = 0x2c;
/// `NIS_OneKeyframe`: a path with this many samples is placed once and never moves.
pub const ONE_KEYFRAME: i32 = 5;
/// `NIS_FirstKeyFrame`: the sample holding a one-keyframe path's only key.
pub const FIRST_KEYFRAME: usize = 2;
/// The loader's degrees-to-radians factor (a `float` constant).
pub const DEG_TO_RAD: f32 = 0.017_453_292;
/// `1/30` as the script parser multiplies frame numbers (a `float` constant).
pub const FRAME_TO_SECONDS: f32 = 0.033_333_335;

/// What an object path animates (the path's race field after loading).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// A ship of a race (0 Somtaaw .. 5 traders, 6 the derelict-race ships such as buoys,
    /// planets and the NIS light and lens-flare anchors).
    Ship(u32),
    Asteroid,
    DustCloud,
    GasCloud,
    Nebula,
    Derelict,
    Effect,
    Generic,
    /// `Missile` paths of a ship race (race 14 after loading; the type is the race).
    Missile(u32),
    Other(u32),
}

impl Category {
    /// The path's race field as `nisLoad` leaves it.
    pub fn race(self) -> u32 {
        match self {
            Category::Ship(r) => r,
            Category::Asteroid => 7,
            Category::DustCloud => 8,
            Category::GasCloud => 9,
            Category::Nebula => 10,
            Category::Derelict => 11,
            Category::Effect => 12,
            Category::Generic => 13,
            Category::Missile(_) => 14,
            Category::Other(r) => r,
        }
    }
}

/// `StrToNisRace` (table 0x87f908, compared without regard to case).
pub fn nis_race(name: &str) -> Option<u32> {
    const RACES: [&str; 15] = [
        "RACE_SECT",
        "RACE_BEAST",
        "RACE_KUSHAN",
        "RACE_TAIDAN",
        "RACE_P1",
        "RACE_TRADERS",
        "RACE_DERELICT",
        "Asteroids",
        "DustClouds",
        "GasClouds",
        "Nebulae",
        "Derelicts",
        "ETG",
        "Misc",
        "Missile",
    ];
    RACES.iter().position(|r| r.eq_ignore_ascii_case(name)).map(|i| i as u32)
}

/// Six spline curves (x, y, z, heading, pitch, bank) over one set of keys.
#[derive(Debug, Clone, PartialEq)]
pub struct Curves {
    /// `nSamples`: keys including the end conditions.
    pub samples: i32,
    /// Absolute key times, with the element after the array.
    pub times: Vec<f32>,
    pub params: Vec<Tcb>,
    /// Values per channel, each with the element after the array.
    pub channels: [Vec<f32>; 6],
}

impl Curves {
    /// `bsCurveStartPrealloced` on channel `c`.
    pub fn curve(&self, c: usize) -> SplineCurve {
        SplineCurve {
            time_elapsed: 0.0,
            current_point: 1,
            n_points: self.samples,
            points: self.channels[c].clone(),
            times: self.times.clone(),
            params: self.params.clone(),
        }
    }

    /// Value of channel `c` at sample `i`.
    pub fn value(&self, c: usize, i: usize) -> f32 {
        self.channels[c].get(i).copied().unwrap_or(0.0)
    }

    /// A path that is placed once (`NIS_OneKeyframe`).
    pub fn is_static(&self) -> bool {
        self.samples == ONE_KEYFRAME
    }
}

/// One animated object.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPath {
    /// The name as written in the file, e.g. `sRecon(2)`.
    pub name: String,
    /// The type name without the instance, e.g. `sRecon`.
    pub type_name: String,
    /// The instance number (0 when the name has none).
    pub instance: i32,
    pub category: Category,
    /// Index of the parent path, if any (unused by Cataclysm's update).
    pub parent: Option<usize>,
    /// The start delay field (the times already include it; the game ignores it).
    pub time_offset: f32,
    pub curves: Curves,
}

/// One camera.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPath {
    pub time_offset: f32,
    pub curves: Curves,
}

/// Why an NIS could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadError(pub String);

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LoadError {}

fn err(s: impl Into<String>) -> LoadError {
    LoadError(s.into())
}

/// A loaded NIS (`nisheader`).
#[derive(Debug, Clone, PartialEq)]
pub struct NisData {
    /// Length in seconds.
    pub length: f32,
    /// Loop point (negative: the NIS ends).
    pub loop_point: f32,
    pub objects: Vec<ObjectPath>,
    pub cameras: Vec<CameraPath>,
    /// Script events sorted by time, then by their order in the script (`nisEventSortCB`).
    pub events: Vec<Event>,
    /// `wideScreenIn` is not an event: the parser sets the letterbox fade-in (seconds) when
    /// the script is loaded (`nisScissorBlendSet` with 0).
    pub scissor_fade_in: Option<f32>,
    /// `midPointLookAt` (`iLookyObject`).
    pub looky_object: Option<usize>,
    /// Lines whose keyword the original does not know (they are ignored, as `scriptSet` does).
    pub ignored: Vec<(usize, String)>,
}

/// The file body as the game holds it after loading.
struct Image {
    bytes: Vec<u8>,
}

impl Image {
    fn u32_at(&self, at: usize) -> u32 {
        self.bytes
            .get(at..at + 4)
            .map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
    }

    fn f32_at(&self, at: usize) -> f32 {
        f32::from_bits(self.u32_at(at))
    }

    fn put_f32(&mut self, at: usize, v: f32) {
        if let Some(b) = self.bytes.get_mut(at..at + 4) {
            b.copy_from_slice(&v.to_le_bytes());
        }
    }

    /// `count` floats at `offset`, with reads outside the image as 0.
    fn floats(&self, offset: usize, count: usize) -> Vec<f32> {
        (0..count).map(|i| self.f32_at(offset + i * 4)).collect()
    }

    /// The degrees-to-radians pass of `nisLoad` over `n` values.
    fn convert_to_radians(&mut self, offset: usize, n: usize) {
        for i in 0..n {
            let at = offset + i * 4;
            let v = self.f32_at(at);
            self.put_f32(at, v * DEG_TO_RAD);
        }
    }

    /// The curves of a path whose sample count sits at `rec + count_at`.
    fn curves(&self, rec: usize, count_at: usize) -> Curves {
        let n = self.u32_at(rec + count_at) as i32;
        let len = n.max(0) as usize;
        let times_at = self.u32_at(rec + count_at + 8) as usize;
        let params_at = self.u32_at(rec + count_at + 12) as usize;
        let tcb = self.floats(params_at, len * 3);
        let params = tcb
            .as_chunks::<3>()
            .0
            .iter()
            .map(|k| Tcb {
                tension: k[0],
                continuity: k[1],
                bias: k[2],
            })
            .collect();
        let channels = std::array::from_fn(|c| {
            self.floats(self.u32_at(rec + count_at + 16 + c * 4) as usize, len + 1)
        });
        Curves {
            samples: n,
            times: self.floats(times_at, len + 1),
            params,
            channels,
        }
    }
}

/// A NUL-terminated string at `offset` in the string block.
fn string_at(block: &[u8], offset: u32) -> Option<String> {
    let rest = block.get(offset as usize..)?;
    let end = rest.iter().position(|&b| b == 0)?;
    Some(rest[..end].iter().map(|&b| b as char).collect())
}

impl NisData {
    /// `nisLoad`: the `.nis` file and, when given, its `.script` text.
    pub fn load(nis: &[u8], script: Option<&[u8]>) -> Result<NisData, LoadError> {
        if nis.len() < BODY_START {
            return Err(err(format!("NIS file of {} bytes", nis.len())));
        }
        if &nis[..12] != IDENTIFIER {
            return Err(err("not an NIS file"));
        }
        let rd = |at: usize| u32::from_le_bytes(nis[at..at + 4].try_into().unwrap());
        if rd(12) != VERSION {
            return Err(err(format!("NIS version {:#x}", rd(12))));
        }
        let strings_at = rd(20) as usize;
        let strings_len = rd(24) as usize;
        if strings_at < BODY_START || strings_at > nis.len() {
            return Err(err("bad string block offset"));
        }
        let strings = nis
            .get(strings_at..strings_at + strings_len)
            .ok_or_else(|| err("string block past the end"))?;
        // The header allocation holds everything before the string block.
        let mut image = Image {
            bytes: nis[..strings_at].to_vec(),
        };
        let length = f32::from_bits(rd(28));
        let loop_point = f32::from_bits(rd(32));
        let n_objects = rd(36) as usize;
        let objects_at = rd(40) as usize;
        let n_cameras = rd(44) as usize;
        let cameras_at = rd(48) as usize;
        if objects_at + n_objects * OBJECT_PATH_LEN > image.bytes.len()
            || cameras_at + n_cameras * CAMERA_PATH_LEN > image.bytes.len()
        {
            return Err(err("path records past the end of the body"));
        }

        // Sort the path records by their parent's string offset.
        let mut order: Vec<usize> = (0..n_objects).collect();
        order.sort_by_key(|&i| image.u32_at(objects_at + i * OBJECT_PATH_LEN + 8));
        let records: Vec<Vec<u8>> = order
            .iter()
            .map(|&i| {
                let at = objects_at + i * OBJECT_PATH_LEN;
                image.bytes[at..at + OBJECT_PATH_LEN].to_vec()
            })
            .collect();
        for (k, r) in records.iter().enumerate() {
            let at = objects_at + k * OBJECT_PATH_LEN;
            image.bytes[at..at + OBJECT_PATH_LEN].copy_from_slice(r);
        }

        let mut objects: Vec<ObjectPath> = Vec::with_capacity(n_objects);
        for k in 0..n_objects {
            let rec = objects_at + k * OBJECT_PATH_LEN;
            let n = image.u32_at(rec + 0x10) as usize;
            for c in 3..6 {
                let at = image.u32_at(rec + 0x20 + c * 4) as usize;
                image.convert_to_radians(at, n);
            }
            let raw_name = string_at(strings, image.u32_at(rec + 4))
                .ok_or_else(|| err(format!("object {k}: bad name")))?;
            let (base, instance) = split_name(&raw_name);
            let race = image.u32_at(rec + 0xc);
            let category = match race {
                0..=6 if base.eq_ignore_ascii_case("Missile") => Category::Missile(race),
                0..=6 => Category::Ship(race),
                7 => Category::Asteroid,
                8 => Category::DustCloud,
                9 => Category::GasCloud,
                10 => Category::Nebula,
                11 => Category::Derelict,
                12 => Category::Effect,
                13 => Category::Generic,
                r => Category::Other(r),
            };
            let parent_name = image.u32_at(rec + 8);
            let parent = if parent_name == 0 {
                None
            } else {
                let p = string_at(strings, parent_name).unwrap_or_default();
                let (pb, pi) = split_name(&p);
                // The parent is searched among the paths already seen, by ship type and
                // instance.
                objects
                    .iter()
                    .position(|o| o.type_name.eq_ignore_ascii_case(&pb) && o.instance == pi)
            };
            objects.push(ObjectPath {
                name: raw_name.clone(),
                type_name: base,
                instance,
                category,
                parent,
                time_offset: image.f32_at(rec + 0x14),
                curves: Curves {
                    samples: 0,
                    times: Vec::new(),
                    params: Vec::new(),
                    channels: Default::default(),
                },
            });
        }
        let mut cameras = Vec::with_capacity(n_cameras);
        for k in 0..n_cameras {
            let rec = cameras_at + k * CAMERA_PATH_LEN;
            let n = image.u32_at(rec + 4) as usize;
            for c in 3..6 {
                let at = image.u32_at(rec + 0x14 + c * 4) as usize;
                image.convert_to_radians(at, n);
            }
            cameras.push(rec);
        }
        // Curves read the converted image.
        for (k, o) in objects.iter_mut().enumerate() {
            o.curves = image.curves(objects_at + k * OBJECT_PATH_LEN, 0x10);
        }
        let cameras = cameras
            .into_iter()
            .map(|rec| CameraPath {
                time_offset: image.f32_at(rec + 8),
                curves: image.curves(rec, 4),
            })
            .collect();

        let mut data = NisData {
            length,
            loop_point,
            objects,
            cameras,
            events: Vec::new(),
            scissor_fade_in: None,
            looky_object: None,
            ignored: Vec::new(),
        };
        if let Some(text) = script {
            let script =
                NisScript::parse_bytes(text).map_err(|e| err(format!("script: {e}")))?;
            events::compile(&mut data, &script);
        }
        Ok(data)
    }

    /// `nisFindObjectByName`: `[<race>\]<type>[ (<instance>)]`, matched on race, instance
    /// and the type name without regard to case. Without a race prefix the race is 0.
    pub fn find_object(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        let (mut name, instance) = match name.find('(') {
            // The character before the bracket is cut off with it (the script writes a blank).
            Some(i) => {
                let digits = name[i + 1..].trim_end_matches(')').trim();
                let cut = name[..i].char_indices().last().map_or(0, |(j, _)| j);
                (&name[..cut], digits.parse().unwrap_or(0))
            }
            None => (name, 0),
        };
        let mut race = 0u32;
        if let Some(i) = name.find(['\\', '/']) {
            race = nis_race(&name[..i]).unwrap_or(u32::MAX);
            name = &name[i + 1..];
        }
        if name.eq_ignore_ascii_case("Missile") {
            race = 14;
        }
        self.objects.iter().position(|o| {
            o.instance == instance
                && o.category.race() == race
                && o.type_name.eq_ignore_ascii_case(name)
        })
    }

    /// `nisShipStartPosition`: where an object starts, in NIS space (the position swizzled
    /// to the engine's axes, the orientation from the first sample).
    pub fn start_position(&self, index: usize) -> (hwc_math::Vector, hwc_math::Matrix) {
        let c = &self.objects[index].curves;
        (
            hwc_math::Vector::new(-c.value(2, 0), c.value(0, 0), c.value(1, 0)),
            crate::euler::ship_euler_to_matrix(c.value(3, 0), c.value(4, 0), c.value(5, 0)),
        )
    }
}

/// The type name and instance of a path name (`strchr('(')` + `sscanf("%d")`).
fn split_name(name: &str) -> (String, i32) {
    let (base, inst) = split_instance(name);
    (base.to_string(), inst as i32)
}
