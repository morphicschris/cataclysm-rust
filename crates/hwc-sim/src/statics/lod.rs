//! Level-of-detail tables (`.lod` files, `lodTableReadScript`).

use hwc_formats::script::ScriptFile;

use super::cscan::{scan_f32, scan_i32, scan_rgb};
use super::source::{join, DataSource};

/// Most levels a table can hold.
pub const MAX_LEVELS: usize = 6;

/// What a level draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LodType {
    /// Ends the table.
    #[default]
    Invalid,
    Mesh,
    TinySprite,
    SubPixel,
    Function,
    Null,
}

impl LodType {
    /// `lodTypeRead`: the value is upper-cased and searched for each type name in turn, so
    /// `MESH` anywhere in it selects a mesh.
    fn parse(value: &str) -> Option<Self> {
        let v = value.to_ascii_uppercase();
        [
            ("INVALID", LodType::Invalid),
            ("MESH", LodType::Mesh),
            ("TINYSPRITE", LodType::TinySprite),
            ("SUBPIXEL", LodType::SubPixel),
            ("FUNCTION", LodType::Function),
            ("NULL", LodType::Null),
        ]
        .into_iter()
        .find(|(name, _)| v.contains(name))
        .map(|(_, t)| t)
    }
}

/// One level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LodLevel {
    pub kind: LodType,
    /// `mOnN`, `bOnN`: the level is switched to when the squared camera distance falls below
    /// `bOn` (scaled at run time).
    pub m_on: f32,
    pub b_on: f32,
    /// `mOffN`, `bOffN`: the level is left when the squared distance exceeds `bOff`.
    pub m_off: f32,
    pub b_off: f32,
    /// `pMeshFileN`, joined to the LOD file's directory.
    pub mesh_file: Option<String>,
    /// Whether `mesh_file` exists; the original substitutes the default ship mesh otherwise.
    pub mesh_exists: bool,
}

/// A parsed `.lod` file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LodInfo {
    /// Path the table was read from.
    pub file: String,
    /// `pointColor`: colour of the object when drawn as a point.
    pub point_color: [u8; 4],
    /// `useHomeworldLODSystem`.
    pub use_homeworld_lod_system: i32,
    /// Levels up to (not including) the first invalid one.
    pub levels: Vec<LodLevel>,
}

impl LodInfo {
    /// Reads `dir + file`. `None` when the file does not exist.
    pub fn load(src: &dyn DataSource, dir: &str, file: &str) -> Option<Self> {
        let path = join(dir, file);
        let script = ScriptFile::parse_bytes(&src.read(&path)?).ok()?;
        let mut info = LodInfo { file: path, ..Default::default() };
        let mut levels: [LodLevel; MAX_LEVELS] = Default::default();
        for e in script.entries() {
            let key = e.key.as_str();
            if key == "pointColor" {
                if let Some(c) = scan_rgb(&e.raw) {
                    info.point_color = c;
                }
                continue;
            }
            if key == "useHomeworldLODSystem" {
                if let Some(v) = scan_i32(&e.raw) {
                    info.use_homeworld_lod_system = v;
                }
                continue;
            }
            let Some((prefix, index)) = split_level_key(key) else { continue };
            let level = &mut levels[index];
            match prefix {
                "type" => {
                    if let Some(t) = LodType::parse(&e.raw) {
                        level.kind = t;
                    }
                }
                "mOn" | "bOn" | "mOff" | "bOff" => {
                    if let Some(v) = scan_f32(&e.raw) {
                        *match prefix {
                            "mOn" => &mut level.m_on,
                            "bOn" => &mut level.b_on,
                            "mOff" => &mut level.m_off,
                            _ => &mut level.b_off,
                        } = v;
                    }
                }
                "pMeshFile" => {
                    let path = join(dir, &e.raw);
                    level.mesh_exists = src.exists(&path);
                    level.mesh_file = Some(path);
                }
                _ => {}
            }
        }
        info.levels = levels.into_iter().take_while(|l| l.kind != LodType::Invalid).collect();
        Some(info)
    }
}

/// `type3` -> `("type", 3)` for the per-level keywords (levels 0 to 5).
fn split_level_key(key: &str) -> Option<(&str, usize)> {
    let (prefix, digit) = key.split_at(key.len().checked_sub(1)?);
    let index = digit.parse::<usize>().ok().filter(|&i| i < MAX_LEVELS)?;
    matches!(prefix, "type" | "mOn" | "bOn" | "mOff" | "bOff" | "pMeshFile").then_some((prefix, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_keys() {
        assert_eq!(split_level_key("bOff5"), Some(("bOff", 5)));
        assert_eq!(split_level_key("type6"), None);
        assert_eq!(split_level_key("pointColor"), None);
        assert_eq!(LodType::parse("Mesh"), Some(LodType::Mesh));
        assert_eq!(LodType::parse("TinySprite"), Some(LodType::TinySprite));
    }
}
