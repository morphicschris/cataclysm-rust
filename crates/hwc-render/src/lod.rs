//! Level-of-detail tables (`.lod` files) and per-instance level selection.
//!
//! Cataclysm picks a ship's level from how big its collision sphere looks on screen: the
//! projected radius (in normalised device units, as the selection-circle code computes it) is
//! compared against fixed thresholds and the result is shifted by a global detail bias. The
//! distance-based hysteresis of Homeworld 1 (`bOn`/`bOff` squared distances times a scale
//! factor) is still present for tables flagged to use it, and is available here as
//! [`LodMode::Distance`]; [`AutoLod`] reproduces Homeworld 1's polygon-budget control of that
//! scale factor (opt-in).

use glam::{Mat4, Vec3};
use hwc_formats::script::ScriptFile;

/// Most levels a table holds.
pub const MAX_LEVELS: usize = 6;

/// Screen-radius thresholds: above the first is level 0, above the second level 1, and so on;
/// at or below the last is level 5.
pub const SCREEN_THRESHOLDS: [f32; 5] = [0.4, 0.12, 0.045, 0.017, 0.0065];

/// What a level draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodKind {
    Mesh,
    TinySprite,
    SubPixel,
    Function,
    Null,
}

impl LodKind {
    fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_uppercase().as_str() {
            "MESH" => Self::Mesh,
            "SPRITE" | "TINYSPRITE" => Self::TinySprite,
            "SUBPIXEL" => Self::SubPixel,
            "FUNCTION" => Self::Function,
            "NULL" => Self::Null,
            _ => return None,
        })
    }
}

/// One `typeN` block of a `.lod` file.
#[derive(Debug, Clone, PartialEq)]
pub struct LodLevel {
    pub kind: LodKind,
    /// Mesh path relative to the race directory (`pMeshFileN`).
    pub mesh_file: Option<String>,
    pub m_on: f32,
    /// Squared camera distance below which the next more detailed level switches back on.
    pub b_on: f32,
    pub m_off: f32,
    /// Squared camera distance above which this level drops to the next one.
    pub b_off: f32,
}

/// A parsed `.lod` file.
#[derive(Debug, Clone, PartialEq)]
pub struct LodTable {
    pub point_color: [u8; 3],
    pub levels: Vec<LodLevel>,
}

impl LodTable {
    /// Reads levels `type0`, `type1`, ... until the first missing or unknown type, as the loader
    /// stops at the first empty slot.
    pub fn from_script(script: &ScriptFile) -> Self {
        let point_color = script
            .get("pointColor")
            .map(|e| {
                let v: Vec<i64> = e.values.iter().filter_map(|v| v.as_i64()).collect();
                let c = |i: usize| v.get(i).copied().unwrap_or(255).clamp(0, 255) as u8;
                [c(0), c(1), c(2)]
            })
            .unwrap_or([255; 3]);
        let mut levels = Vec::new();
        for i in 0..MAX_LEVELS {
            let Some(kind) = script
                .get(&format!("type{i}"))
                .map(|e| e.raw.trim().to_owned())
                .and_then(|s| LodKind::parse(&s))
            else {
                break;
            };
            let f = |key: &str| {
                script
                    .get(&format!("{key}{i}"))
                    .and_then(|e| e.raw.trim().parse::<f64>().ok())
                    .unwrap_or(0.0) as f32
            };
            levels.push(LodLevel {
                kind,
                mesh_file: script
                    .get(&format!("pMeshFile{i}"))
                    .map(|e| e.raw.trim().to_owned()),
                m_on: f("mOn"),
                b_on: f("bOn"),
                m_off: f("mOff"),
                b_off: f("bOff"),
            });
        }
        Self {
            point_color,
            levels,
        }
    }

    pub fn len(&self) -> usize {
        self.levels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.levels.is_empty()
    }
}

/// Radius of a sphere on screen as the selection code measures it: the horizontal offset, in
/// normalised device units, between the projected centre and the centre moved by `radius` along
/// the eye's x axis. Negative when the sphere is behind the eye.
pub fn screen_radius(view: Mat4, proj: Mat4, centre: Vec3, radius: f32) -> f32 {
    let c = view.transform_point3(centre);
    let clip_c = proj * c.extend(1.0);
    let clip_r = proj * (c + Vec3::X * radius).extend(1.0);
    if clip_c.w == 0.0 {
        return -1.0;
    }
    (clip_r.x - clip_c.x) / clip_c.w
}

/// How [`LodSelector`] chooses levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LodMode {
    /// Cataclysm's default: screen-radius thresholds plus the detail bias.
    ScreenSize,
    /// Homeworld 1 style squared-distance hysteresis (`bOn`/`bOff` × scale factor), never more
    /// detailed than the bias level.
    Distance,
}

/// Global level-of-detail settings shared by every instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodSelector {
    pub mode: LodMode,
    /// Detail bias: added to the screen-size level (0 = full detail).
    pub bias: u8,
    /// Scale applied to the distance thresholds in [`LodMode::Distance`].
    pub scale_factor: f32,
}

impl Default for LodSelector {
    fn default() -> Self {
        Self {
            mode: LodMode::ScreenSize,
            bias: 0,
            scale_factor: 1.0,
        }
    }
}

impl LodSelector {
    /// Level index for an instance. `current` is the level it used last frame (needed for the
    /// distance hysteresis), `screen_radius` its projected collision radius and
    /// `distance_squared` its squared distance from the eye.
    pub fn select(
        &self,
        table_len: usize,
        current: usize,
        screen_radius: f32,
        distance_squared: f32,
        table: Option<&LodTable>,
    ) -> usize {
        if table_len == 0 {
            return 0;
        }
        let last = table_len - 1;
        let bias = usize::from(self.bias);
        match (self.mode, table) {
            (LodMode::Distance, Some(table)) => {
                let mut level = current.min(last);
                let off = |l: usize| table.levels[l].b_off * self.scale_factor;
                let on = |l: usize| table.levels[l].b_on * self.scale_factor;
                if distance_squared > off(level) {
                    level += 1;
                    while level < table_len && distance_squared > off(level) {
                        level += 1;
                    }
                    level = level.min(last);
                } else {
                    while level > 0 && distance_squared < on(level - 1) {
                        level -= 1;
                    }
                }
                if level < bias {
                    level = bias.min(last);
                }
                level
            }
            _ => {
                let base = if !(0.0..=SCREEN_THRESHOLDS[0]).contains(&screen_radius) {
                    0
                } else {
                    SCREEN_THRESHOLDS
                        .iter()
                        .position(|&t| screen_radius > t)
                        .unwrap_or(SCREEN_THRESHOLDS.len())
                };
                (base + bias).min(last)
            }
        }
    }
}

/// Homeworld 1's automatic detail control: nudges the distance scale factor so the polygons
/// drawn per frame stay near a target (`autolod.script`). Opt-in; Cataclysm's screen-size
/// selection does not use it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AutoLod {
    pub scale_factor_delta: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub target_polys: u32,
    pub poly_delta: u32,
    going_up: Option<bool>,
}

impl Default for AutoLod {
    /// `autolod.script`'s values for hardware renderers.
    fn default() -> Self {
        Self {
            scale_factor_delta: 0.015,
            min_scale: 0.1,
            max_scale: 1.0,
            target_polys: 1900,
            poly_delta: 400,
            going_up: None,
        }
    }
}

impl AutoLod {
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut a = Self::default();
        let f = |k: &str| script.get(k).and_then(|e| e.f32());
        a.scale_factor_delta = f("alodScaleFactorDelta").unwrap_or(a.scale_factor_delta);
        a.min_scale = f("alodFastMinScale").unwrap_or(a.min_scale);
        a.max_scale = f("alodFastMaxScale").unwrap_or(a.max_scale);
        a.target_polys = f("alodFastTargetPolys").map_or(a.target_polys, |v| v as u32);
        a.poly_delta = f("alodFastTargetDelta").map_or(a.poly_delta, |v| v as u32);
        a
    }

    /// Adjusts `selector.scale_factor` after a frame that drew `polys` polygons.
    pub fn adjust(&mut self, selector: &mut LodSelector, polys: u32) {
        if self.target_polys == 0 || self.poly_delta == 0 {
            selector.scale_factor = self.max_scale;
            return;
        }
        let delta = self.target_polys.abs_diff(polys);
        let mut step = self.scale_factor_delta;
        if delta > self.poly_delta {
            step *= 2.0;
            if delta > 2 * self.poly_delta {
                step *= 2.0;
            }
        }
        if polys > self.target_polys {
            if self.going_up == Some(true) && polys < self.target_polys + self.poly_delta {
                return;
            }
            self.going_up = Some(false);
            selector.scale_factor = (selector.scale_factor - step).max(self.min_scale);
        } else {
            if self.going_up == Some(false) && polys + self.poly_delta > self.target_polys {
                return;
            }
            self.going_up = Some(true);
            selector.scale_factor = (selector.scale_factor + step).min(self.max_scale);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ACOLYTE: &str = "pointColor 255,0,0\ntype0 MESH\npMeshFile0 a\\rl0\\lod0\\a.peo\nmOn0 1.0\nbOn0 16650\nmOff0 1.0\nbOff0 18499\ntype1 MESH\npMeshFile1 a\\rl0\\lod1\\a.peo\nmOn1 1.0\nbOn1 232179\nmOff1 1.0\nbOff1 257977\ntype2 MESH\npMeshFile2 x\nbOn2 1057039\nbOff2 1174487\n;type3 SUBPIXEL\n";

    #[test]
    fn parses_levels_until_a_gap() {
        let t = LodTable::from_script(&ScriptFile::parse(ACOLYTE));
        assert_eq!(t.len(), 3);
        assert_eq!(t.point_color, [255, 0, 0]);
        assert_eq!(t.levels[1].b_off, 257977.0);
        assert_eq!(
            t.levels[0].mesh_file.as_deref(),
            Some("a\\rl0\\lod0\\a.peo")
        );
    }

    #[test]
    fn screen_size_thresholds_and_bias() {
        let s = LodSelector::default();
        let pick = |r| s.select(6, 0, r, 0.0, None);
        assert_eq!(pick(0.5), 0);
        assert_eq!(pick(0.4), 1);
        assert_eq!(pick(0.12), 2);
        assert_eq!(pick(0.05), 2);
        assert_eq!(pick(0.02), 3);
        assert_eq!(pick(0.01), 4);
        assert_eq!(pick(0.001), 5);
        assert_eq!(pick(-1.0), 0, "behind the eye uses the bias level");
        let biased = LodSelector { bias: 2, ..s };
        assert_eq!(biased.select(4, 0, 0.5, 0.0, None), 2);
        assert_eq!(
            biased.select(4, 0, 0.01, 0.0, None),
            3,
            "clamped to the last level"
        );
    }

    #[test]
    fn distance_hysteresis() {
        let t = LodTable::from_script(&ScriptFile::parse(ACOLYTE));
        let s = LodSelector {
            mode: LodMode::Distance,
            ..Default::default()
        };
        assert_eq!(s.select(3, 0, 0.0, 18000.0, Some(&t)), 0);
        assert_eq!(s.select(3, 0, 0.0, 19000.0, Some(&t)), 1);
        // Between bOn0 and bOff0 a ship at level 1 stays at level 1.
        assert_eq!(s.select(3, 1, 0.0, 17000.0, Some(&t)), 1);
        assert_eq!(s.select(3, 1, 0.0, 16000.0, Some(&t)), 0);
        assert_eq!(s.select(3, 0, 0.0, 1e9, Some(&t)), 2);
    }

    #[test]
    fn screen_radius_matches_projection() {
        let view = Mat4::look_at_rh(Vec3::new(0.0, -100.0, 0.0), Vec3::ZERO, Vec3::Z);
        let proj = Mat4::perspective_rh(90f32.to_radians(), 1.0, 1.0, 1000.0);
        let r = screen_radius(view, proj, Vec3::ZERO, 10.0);
        assert!((r - 0.1).abs() < 1e-4, "{r}");
        assert!(screen_radius(view, proj, Vec3::new(0.0, -200.0, 0.0), 10.0) < 0.0);
    }

    #[test]
    fn auto_lod_moves_the_scale_factor() {
        let mut a = AutoLod::default();
        let mut s = LodSelector::default();
        a.adjust(&mut s, 5000);
        assert!((s.scale_factor - (1.0 - 0.06)).abs() < 1e-6);
        a.adjust(&mut s, 100);
        assert!((s.scale_factor - 1.0).abs() < 1e-6);
    }
}
