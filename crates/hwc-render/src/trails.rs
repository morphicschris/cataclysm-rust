//! Engine trails and glows.
//!
//! Strike craft leave ribbon trails: every few simulation ticks the nozzle's world position and
//! the ship's axes are pushed into a ring of segments, and each frame the ring is drawn as a
//! camera-facing ribbon (plus a translucent sheath close up, or a plain line far away) behind a
//! glow sprite at the nozzle. Capital ships have no ribbon; a glow mesh scaled by the trail
//! sizes grows out of each nozzle while the ship moves and shrinks when it stops.
//!
//! Cataclysm changes over Homeworld 1 that are reproduced here: the nozzle follows the mesh
//! object it is attached to (so animated engines carry their trails), the nozzle glow and the
//! capital glow take the team colour, and trails are suppressed while docked, hyperspacing or
//! cloaked (see [`TrailGate`]).
//!
//! Use: build a [`ShipTrailDef`] per ship type, a [`ShipTrails`] per ship; call
//! [`ShipTrails::update`] every simulation tick and [`ShipTrails::draw`] every frame into an
//! [`EffectBatch`].

use glam::{Mat3, Mat4, Vec2, Vec3};
use hwc_formats::script::{ScriptFile, ShipStats};

use crate::assets::EngineNozzle;
use crate::effects::{
    Blend, EffectBatch, EffectTexture, GlowMeshDraw, GlowMeshKind, Line, Sprite, Strip, FULL_UV,
};

/// Most trail slots a ship has (`Eng0`..`Eng3`).
pub const MAX_TRAILS: usize = 4;
/// Players with trail colours.
pub const MAX_TEAMS: usize = 8;
/// Detail levels above this draw ribbon trails as lines.
pub const LINE_CUTOFF_LOD: usize = 2;

/// Alpha of the sheath faces.
const PYRAMID_ALPHA_LO: u8 = 15;
const PYRAMID_ALPHA_MID: u8 = 79;
const PYRAMID_ALPHA_HI: u8 = 143;

/// Tuning from `trail.script`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailTweaks {
    pub ran_max: f32,
    pub ran_jitter: f32,
    pub velratio_thresh: f32,
    pub velratio_accelthresh: f32,
    pub velratio_cutoff: f32,
    pub accelthresh: f32,
    pub xadj: f32,
    pub halfwidth: f32,
    pub halfwidth_falloff: f32,
    pub halfwidth_min: f32,
    pub glow_radialadjust: f32,
    pub glow_baseradialscale: f32,
    pub burn_sizescale: f32,
    pub burn_coloradjust: i32,
    pub coloradjust_freq: i32,
    pub color_adjust: i32,
    pub exponent_base: f32,
    pub exponent_range: f32,
    pub expansion_ticks: u8,
    /// Ribbon trails are curved through their recorded points with this many pieces per segment
    /// (a user-requested change, `hwcSmoothTrails`); 1 draws the original's straight segments.
    pub smoothing: usize,
}

impl Default for TrailTweaks {
    /// The values in Cataclysm's `trail.script`.
    fn default() -> Self {
        Self {
            ran_max: 5.0,
            ran_jitter: 0.75,
            velratio_thresh: 0.5,
            velratio_accelthresh: 0.75,
            velratio_cutoff: 0.9,
            accelthresh: 7.7,
            xadj: 8.0,
            halfwidth: 11.5,
            halfwidth_falloff: 0.7,
            halfwidth_min: 3.2,
            glow_radialadjust: 0.82,
            glow_baseradialscale: 0.2,
            burn_sizescale: 1.7,
            burn_coloradjust: -100,
            coloradjust_freq: 20,
            color_adjust: 12,
            exponent_base: 3.75,
            exponent_range: 0.55,
            expansion_ticks: 16,
            smoothing: 1,
        }
    }
}

impl TrailTweaks {
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut t = Self::default();
        let reals: [(&str, &mut f32); 14] = [
            ("RAN_MAX", &mut t.ran_max),
            ("RAN_JITTER", &mut t.ran_jitter),
            ("VELRATIO_THRESH", &mut t.velratio_thresh),
            ("VELRATIO_ACCELTHRESH", &mut t.velratio_accelthresh),
            ("VELRATIO_CUTOFF", &mut t.velratio_cutoff),
            ("ACCELTHRESH", &mut t.accelthresh),
            ("XADJ", &mut t.xadj),
            ("HALFWIDTH", &mut t.halfwidth),
            ("HALFWIDTH_FALLOFF", &mut t.halfwidth_falloff),
            ("HALFWIDTH_MIN", &mut t.halfwidth_min),
            ("GLOW_RADIALADJUST", &mut t.glow_radialadjust),
            ("GLOW_BASERADIALSCALE", &mut t.glow_baseradialscale),
            ("BURN_SIZESCALE", &mut t.burn_sizescale),
            ("TRAIL_EXPONENT_BASE", &mut t.exponent_base),
        ];
        for (k, slot) in reals {
            if let Some(v) = script.get(k).and_then(|e| e.f32()) {
                *slot = v;
            }
        }
        if let Some(v) = script.get("TRAIL_EXPONENT_RANGE").and_then(|e| e.f32()) {
            t.exponent_range = v;
        }
        let ints: [(&str, &mut i32); 3] = [
            ("BURN_COLORADJUST", &mut t.burn_coloradjust),
            ("COLORADJUST_FREQ", &mut t.coloradjust_freq),
            ("COLOR_ADJUST", &mut t.color_adjust),
        ];
        for (k, slot) in ints {
            if let Some(v) = script.get(k).and_then(|e| e.i32()) {
                *slot = v;
            }
        }
        if let Some(v) = script.get("TRAIL_EXPANSION_TICKS").and_then(|e| e.i32()) {
            t.expansion_ticks = v.clamp(1, 255) as u8;
        }
        t
    }
}

// ---------------------------------------------------------------------------------------------
// Colours

/// The four trail control-point colours of each player (`TrailColor` in `teamcolors.script`).
#[derive(Debug, Clone, PartialEq)]
pub struct TeamTrailColors {
    pub teams: [[[u8; 3]; 4]; MAX_TEAMS],
    /// `BaseToTrailFactors0..3`: hue offset, lightness and saturation scales for deriving the
    /// control points from a base colour.
    pub factors: [[f32; 3]; 4],
}

impl Default for TeamTrailColors {
    /// Cataclysm's `teamcolors.script`.
    fn default() -> Self {
        let rows: [[u8; 3]; MAX_TEAMS] = [
            [255, 255, 255],
            [225, 0, 0],
            [253, 237, 37],
            [0, 255, 255],
            [179, 247, 101],
            [225, 255, 0],
            [179, 247, 101],
            [255, 255, 255],
        ];
        Self {
            teams: rows.map(|c| {
                [
                    [0; 3],
                    c,
                    c.map(|v| (u16::from(v) * 128 / 255) as u8),
                    [0; 3],
                ]
            }),
            factors: [
                [0.0, 0.2, 1.0],
                [0.0, 1.4, 1.0],
                [0.0, 0.8, 1.2],
                [0.0, 0.05, 1.0],
            ],
        }
    }
}

impl TeamTrailColors {
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut t = Self::default();
        for e in script.entries() {
            let v: Vec<f64> = e.values.iter().filter_map(|v| v.as_f64()).collect();
            if e.key == "TrailColor" {
                if let [p, i, r, g, b] = v[..] {
                    let (p, i) = (p as usize, i as usize);
                    if p < MAX_TEAMS && i < 4 {
                        t.teams[p][i] = [r, g, b].map(|c| c.clamp(0.0, 255.0) as u8);
                    }
                }
            } else if let Some(n) = e.key.strip_prefix("BaseToTrailFactors") {
                if let (Ok(n), [h, l, s]) = (n.parse::<usize>(), &v[..]) {
                    if n < 4 {
                        t.factors[n] = [*h as f32, *l as f32, *s as f32];
                    }
                }
            }
        }
        t
    }

    /// Recomputes a player's control points from a chosen base colour, as the game does when
    /// team colours change.
    pub fn derive_from_base(&mut self, team: usize, base: [u8; 3]) {
        if team >= MAX_TEAMS {
            return;
        }
        for (i, f) in self.factors.iter().enumerate() {
            self.teams[team][i] = adjust_hls(base, *f);
        }
    }
}

fn rgb_to_hls(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let l = (max + min) / 2.0;
    if max == min {
        return [0.0, l, 0.0];
    }
    let d = max - min;
    let s = if l <= 0.5 {
        d / (max + min)
    } else {
        d / (2.0 - max - min)
    };
    let h = if c[0] == max {
        (c[1] - c[2]) / d
    } else if c[1] == max {
        2.0 + (c[2] - c[0]) / d
    } else {
        4.0 + (c[0] - c[1]) / d
    };
    [(h / 6.0).rem_euclid(1.0), l, s]
}

fn hls_to_rgb([h, l, s]: [f32; 3]) -> [f32; 3] {
    if s == 0.0 {
        return [l; 3];
    }
    let m2 = if l <= 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let m1 = 2.0 * l - m2;
    let f = |h: f32| {
        let h = h.rem_euclid(1.0) * 6.0;
        if h < 1.0 {
            m1 + (m2 - m1) * h
        } else if h < 3.0 {
            m2
        } else if h < 4.0 {
            m1 + (m2 - m1) * (4.0 - h)
        } else {
            m1
        }
    };
    [f(h + 1.0 / 3.0), f(h), f(h - 1.0 / 3.0)]
}

fn adjust_hls(base: [u8; 3], [dh, dl, ds]: [f32; 3]) -> [u8; 3] {
    let [mut h, mut l, mut s] = rgb_to_hls(base.map(|v| f32::from(v) / 255.0));
    h += dh;
    if h < 0.0 {
        h += 1.0;
    }
    if h > 1.0 {
        h -= 1.0;
    }
    l = (l * dl).min(1.0);
    s = (s * ds).min(1.0);
    hls_to_rgb([h, l, s]).map(|v| (v * 255.0).round().clamp(0.0, 255.0) as u8)
}

fn hsv_saturation_shift(c: [u8; 3], delta: i32) -> [u8; 3] {
    let f = c.map(|v| f32::from(v) / 255.0);
    let max = f[0].max(f[1]).max(f[2]);
    let min = f[0].min(f[1]).min(f[2]);
    let v = max;
    let s = if max > 0.0 { (max - min) / max } else { 0.0 };
    let h = if max == min {
        0.0
    } else if f[0] == max {
        (f[1] - f[2]) / (max - min)
    } else if f[1] == max {
        2.0 + (f[2] - f[0]) / (max - min)
    } else {
        4.0 + (f[0] - f[1]) / (max - min)
    };
    let s = (s + delta as f32 / 255.0).clamp(0.0, 1.0);
    let h6 = (h).rem_euclid(6.0);
    let i = h6.floor();
    let fr = h6 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * fr);
    let t = v * (1.0 - s * (1.0 - fr));
    let rgb = match i as i32 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    };
    rgb.map(|x| (x * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// A ship type's trail colour ramp (`trailSegments`, `trailGranularity`, `trailColor`).
#[derive(Debug, Clone, PartialEq)]
pub struct TrailStatic {
    pub segments: usize,
    /// Ticks skipped between recorded segments.
    pub granularity: i32,
    /// Per team, per segment colour.
    pub colors: Vec<Vec<[u8; 3]>>,
    /// Per team, per segment: whether the colour is a control point from the file.
    pub keys: Vec<Vec<bool>>,
}

impl TrailStatic {
    /// Parses the ramp from a `.shp` file; `None` when it has none (capital ships).
    pub fn from_stats(stats: &ShipStats) -> Option<Self> {
        let get = |k: &str| stats.other.get(k).and_then(|v| v.first());
        let segments = get("trailSegments")?.i32()?.clamp(2, 999) as usize;
        let granularity = get("trailGranularity").and_then(|e| e.i32()).unwrap_or(1);
        let mut set: Vec<Vec<Option<[u8; 3]>>> = vec![vec![None; segments]; MAX_TEAMS];
        for e in stats.other.get("trailColor")? {
            let v: Vec<i64> = e.values.iter().filter_map(|v| v.as_i64()).collect();
            let [team, index, r, g, b] = v[..] else {
                continue;
            };
            let (team, index) = (team as usize, index as usize);
            if team < MAX_TEAMS && index < segments {
                set[team][index] = Some([r, g, b].map(|c| c.clamp(0, 255) as u8));
            }
        }
        let mut colors = Vec::new();
        let mut keys = Vec::new();
        for mut team in set {
            // The first and last entries take the nearest set colours.
            let first = team.iter().flatten().next().copied().unwrap_or([0; 3]);
            let last = team.iter().rev().flatten().next().copied().unwrap_or(first);
            team[0].get_or_insert(first);
            team[segments - 1].get_or_insert(last);
            let mut ramp = vec![[0u8; 3]; segments];
            let mut key = vec![false; segments];
            let mut last_index = 0;
            for i in 0..segments {
                if let Some(c) = team[i] {
                    ramp[i] = c;
                    key[i] = true;
                    let lc = ramp[last_index];
                    for (j, slot) in ramp.iter_mut().enumerate().take(i).skip(last_index + 1) {
                        *slot = interpolate(lc, c, last_index, i, j);
                    }
                    last_index = i;
                }
            }
            colors.push(ramp);
            keys.push(key);
        }
        Some(Self {
            segments,
            granularity,
            colors,
            keys,
        })
    }

    /// Replaces a team's control points, in order, with `control` and re-interpolates (the
    /// game's recolourise after team colours are set).
    pub fn recolorize(&mut self, team: usize, control: &[[u8; 3]; 4]) {
        let (Some(ramp), Some(keys)) = (self.colors.get_mut(team), self.keys.get(team)) else {
            return;
        };
        let mut cp = 0;
        let mut last_index = 0;
        for i in 0..ramp.len() {
            if !keys[i] {
                continue;
            }
            if let Some(c) = control.get(cp) {
                ramp[i] = *c;
            }
            cp += 1;
            let lc = ramp[last_index];
            for j in last_index + 1..i {
                ramp[j] = interpolate(lc, ramp[i], last_index, i, j);
            }
            last_index = i;
        }
    }

    pub fn recolorize_all(&mut self, colors: &TeamTrailColors) {
        for team in 0..MAX_TEAMS {
            self.recolorize(team, &colors.teams[team]);
        }
    }
}

fn interpolate(a: [u8; 3], b: [u8; 3], ia: usize, ib: usize, j: usize) -> [u8; 3] {
    let (wa, wb, d) = ((ib - j) as u32, (j - ia) as u32, (ib - ia) as u32);
    std::array::from_fn(|c| ((u32::from(a[c]) * wa + u32::from(b[c]) * wb) / d) as u8)
}

// ---------------------------------------------------------------------------------------------
// Ship type definition

/// Per-slot trail settings from the `.shp` file (`trailWidth`, `trailWidth2`, ...); -1 means
/// "use the default".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailSlotParams {
    pub style: u32,
    pub width: f32,
    pub height: f32,
    /// Degrees.
    pub angle: f32,
    pub ribbon_adjust: f32,
    pub length: f32,
    pub scale_cap: f32,
    pub sprite_radius: f32,
    pub sprite_offset: f32,
}

impl Default for TrailSlotParams {
    fn default() -> Self {
        Self {
            style: 1,
            width: -1.0,
            height: -1.0,
            angle: -1.0,
            ribbon_adjust: -1.0,
            length: -1.0,
            scale_cap: -1.0,
            sprite_radius: -1.0,
            sprite_offset: 0.0,
        }
    }
}

impl TrailSlotParams {
    pub fn from_stats(stats: &ShipStats, slot: usize) -> Self {
        let mut p = Self::default();
        let suffix = if slot == 0 {
            String::new()
        } else {
            (slot + 1).to_string()
        };
        let f = |k: &str| {
            stats
                .other
                .get(&format!("{k}{suffix}"))
                .and_then(|v| v.first())
                .and_then(|e| e.f32())
        };
        if let Some(v) = f("trailStyle") {
            p.style = v.max(0.0) as u32;
        }
        let fields: [(&str, &mut f32); 8] = [
            ("trailWidth", &mut p.width),
            ("trailHeight", &mut p.height),
            ("trailAngle", &mut p.angle),
            ("trailRibbonAdjust", &mut p.ribbon_adjust),
            ("trailLength", &mut p.length),
            ("trailScaleCap", &mut p.scale_cap),
            ("trailSpriteRadius", &mut p.sprite_radius),
            ("trailSpriteOffset", &mut p.sprite_offset),
        ];
        for (k, slot) in fields {
            if let Some(v) = f(k) {
                *slot = v;
            }
        }
        p
    }
}

/// Which capital glow mesh family a ship uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlowFamily {
    /// Generic glow (or the frigate glow for slots with style 3).
    Standard,
    /// Kushan/Somtaaw-style mothership glow (stretched 1.5× sideways).
    MothershipR1,
    MothershipR2,
    /// The Turanic (P1) mothership glow.
    MothershipP1,
}

/// Engine trail data for a ship type.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipTrailDef {
    /// Ribbon colour ramp; `None` for ships with capital glows.
    pub ramp: Option<TrailStatic>,
    pub slots: [TrailSlotParams; MAX_TRAILS],
    /// Nozzles by slot; the number of trails is the highest slot present plus one.
    pub nozzles: Vec<EngineNozzle>,
    /// Collision sphere radius (default nozzle-glow size).
    pub collision_radius: f32,
    /// Race index in the game's order (Somtaaw, Beast, Kushan, Taiidan, Turanic, Traders,
    /// Derelict).
    pub race: u32,
    pub is_mothership: bool,
}

/// Race index of a `race_*` directory name.
pub fn race_index(race_dir: &str) -> u32 {
    match race_dir.to_ascii_lowercase().as_str() {
        "race_sect" => 0,
        "race_beast" => 1,
        "race_kushan" => 2,
        "race_taidan" => 3,
        "race_p1" => 4,
        "race_traders" => 5,
        _ => 6,
    }
}

impl ShipTrailDef {
    pub fn new(
        stats: &ShipStats,
        nozzles: Vec<EngineNozzle>,
        collision_radius: f32,
        race: u32,
    ) -> Self {
        Self {
            ramp: TrailStatic::from_stats(stats),
            slots: std::array::from_fn(|i| TrailSlotParams::from_stats(stats, i)),
            nozzles,
            collision_radius,
            race,
            is_mothership: stats
                .shipclass
                .as_deref()
                .is_some_and(|c| c.eq_ignore_ascii_case("CLASS_Mothership")),
        }
    }

    /// Loads everything from game data for a ship type.
    pub fn load(src: &dyn crate::assets::AssetSource, def: &crate::assets::ShipDef) -> Self {
        let nozzles = crate::assets::engine_nozzles(src, def);
        let radius = def
            .mex_path()
            .and_then(|p| src.read(&p))
            .and_then(|d| hwc_formats::mex::Mex::parse(&d).ok())
            .and_then(|m| m.collision_sphere())
            .map_or(1.0, |(_, r)| r);
        Self::new(&def.stats, nozzles, radius, race_index(&def.race))
    }

    pub fn trail_count(&self) -> usize {
        self.nozzles.iter().map(|n| n.index + 1).max().unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------------------------
// Dynamic state

/// Expansion state of a capital glow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlowState {
    Contracting,
    Expanding,
    Contracted,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Segment {
    position: Vec3,
    rotation: Mat3,
    translation: Vec3,
    horizontal: Vec3,
    vertical: Vec3,
    wide: bool,
}

/// One engine's trail.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipTrail {
    pub slot: usize,
    /// 0/1: ribbon (1 adds the sheath close up); 3/4: capital glow; 5: frigate glow.
    pub style: u32,
    pub width: f32,
    pub height: f32,
    /// Radians, or -1 for none.
    pub angle: f32,
    pub ribbon_adjust: f32,
    pub scale_cap: f32,
    pub state: GlowState,
    pub counter: u8,
    pub exponent: f32,
    last_vel_sq: f32,
    prev_vel_sq: f32,
    ran_counter: i32,
    ran: f32,
    grain_counter: i32,
    head: usize,
    length: usize,
    segments: Vec<Segment>,
}

impl ShipTrail {
    fn new(def: &ShipTrailDef, slot: usize) -> Self {
        let p = &def.slots[slot];
        let segments = def.ramp.as_ref().map_or(1, |r| r.segments);
        let mut t = Self {
            slot,
            style: p.style,
            width: p.width,
            height: p.height,
            angle: if p.angle == -1.0 {
                -1.0
            } else {
                p.angle.to_radians()
            },
            ribbon_adjust: p.ribbon_adjust,
            scale_cap: p.scale_cap,
            state: GlowState::Contracted,
            counter: 0,
            exponent: 4.0,
            last_vel_sq: 0.0,
            prev_vel_sq: 0.0,
            ran_counter: 0,
            ran: 0.0,
            grain_counter: 0,
            head: 0,
            length: 0,
            segments: vec![Segment::default(); segments],
        };
        if def.ramp.is_none() {
            // Capital glow: raw angle, ribbon defaults to 1.28 or comes from the length.
            t.angle = p.angle;
            t.ribbon_adjust = if p.ribbon_adjust == -1.0 {
                1.28
            } else {
                p.ribbon_adjust
            };
            if p.length != -1.0 {
                t.ribbon_adjust = p.length / t.width;
            }
            t.style = if p.style == 3 { 5 } else { 4 };
        }
        t
    }

    pub fn is_glow(&self) -> bool {
        self.style > 2
    }

    /// Recorded segments.
    pub fn len(&self) -> usize {
        self.length
    }

    pub fn is_empty(&self) -> bool {
        self.length == 0
    }

    /// Forgets the recorded segments.
    pub fn zero_length(&mut self) {
        self.head = 0;
        self.length = 0;
        self.grain_counter = 0;
    }

    fn half_width(&self, tw: &TrailTweaks) -> f32 {
        if self.width == -1.0 {
            tw.halfwidth
        } else {
            0.5 * self.width
        }
    }

    fn height_scalar(&self) -> f32 {
        if self.height == -1.0 || self.width == -1.0 {
            0.25
        } else {
            self.height / self.width
        }
    }

    fn ribbon(&self) -> f32 {
        if self.ribbon_adjust == -1.0 {
            1.0
        } else {
            self.ribbon_adjust
        }
    }

    /// Engine-glow tint level for this slot (`None` stops the scan over later slots).
    fn glow_level(&self, ticks: u8) -> Option<f32> {
        if self.state == GlowState::Contracted {
            return None;
        }
        Some(if self.counter == 0 {
            1.0
        } else if self.state == GlowState::Expanding {
            1.0 - f32::from(self.counter) / f32::from(ticks)
        } else {
            f32::from(self.counter) / f32::from(ticks)
        })
    }
}

/// The conditions under which a ship draws its trails this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrailGate {
    pub moving: bool,
    pub docked: bool,
    pub hyperspacing: bool,
    pub cloaked: bool,
    /// Flagged not to draw trails, clamped or disabled.
    pub suppressed: bool,
    /// Dodging (or still within its dodge time) without an order that keeps it moving.
    pub idle_dodge: bool,
}

impl TrailGate {
    pub fn allows_trail(&self) -> bool {
        self.moving
            && !self.docked
            && !self.hyperspacing
            && !self.cloaked
            && !self.suppressed
            && !self.idle_dodge
    }
}

/// Per-tick input for [`ShipTrails::update`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailTick<'a> {
    /// Mesh-to-world matrix (rotation and position).
    pub transform: Mat4,
    /// Ship-space matrix of each mesh object of the level of detail drawn (from
    /// [`crate::mesh::Skeleton::resolve`], or [`crate::mesh::Skeleton::rest_matrices`] when not
    /// animated): every nozzle goes through its object's matrix, or the first object's when it
    /// names none (`trailGetNozzleWorldPosition`). `None` keeps nozzles at their file
    /// positions (only right for a ship whose first object sits at the origin).
    pub objects: Option<(&'a crate::mesh::Skeleton, &'a [Mat4])>,
}

/// Per-frame input for [`ShipTrails::draw`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrailFrame<'a> {
    pub transform: Mat4,
    pub objects: Option<(&'a crate::mesh::Skeleton, &'a [Mat4])>,
    pub velocity: Vec3,
    /// Current maximum speed (with speed upgrades).
    pub max_velocity: f32,
    pub gate: TrailGate,
    /// The ship's detail level.
    pub lod: usize,
    /// Player index for the colour ramp.
    pub team: usize,
    /// Normalised team colour for the nozzle and capital glows.
    pub team_color: [f32; 3],
    pub eye: Vec3,
    /// The ship's depth in eye space (negative in front), for the glow scale cap.
    pub eye_space_z: f32,
    /// Trail scale (the ship's N-LIPS scale; 1 normally).
    pub scale: f32,
    /// Overall alpha (fading ships).
    pub fade: f32,
    pub glow_texture: EffectTexture,
}

/// Small deterministic generator standing in for the game's trail random streams.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    /// 0..n with four decimal places, as the game's `trailRealRand`.
    fn real(&mut self, n: f32) -> f32 {
        let m = (n * 10000.0) as u32;
        if m == 0 {
            return 0.0;
        }
        (self.next() % m) as f32 / 10000.0
    }
}

/// All trails of one ship.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipTrails {
    pub trails: Vec<ShipTrail>,
    rng: Rng,
}

fn nozzle_offset(
    def: &ShipTrailDef,
    slot: usize,
    objects: Option<(&crate::mesh::Skeleton, &[Mat4])>,
) -> Vec3 {
    let Some(n) = def.nozzles.iter().find(|n| n.index == slot) else {
        return Vec3::ZERO;
    };
    // The nozzle's object in this level of detail (`mexSetEngineNozzle` looks the name up in
    // every level); a nozzle naming no object, or one the level lacks, takes the first
    // object's matrix (`univShipGetBindEntry` maps "none" to object 0).
    let m = objects
        .and_then(|(sk, mats)| {
            let i = n.attached_to.as_deref().and_then(|name| sk.find(name)).unwrap_or(0);
            mats.get(i).or(mats.first()).copied()
        })
        .unwrap_or(Mat4::IDENTITY);
    m.transform_point3(n.position)
}

fn rgba(c: [u8; 3], a: u8, fade: f32) -> [f32; 4] {
    [
        f32::from(c[0]) / 255.0,
        f32::from(c[1]) / 255.0,
        f32::from(c[2]) / 255.0,
        f32::from(a) / 255.0 * fade,
    ]
}

fn color_byte(v: f32) -> u8 {
    (v * 256.0).round().clamp(0.0, 255.0) as u8
}

impl ShipTrails {
    pub fn new(def: &ShipTrailDef, seed: u32) -> Self {
        Self {
            trails: (0..def.trail_count())
                .map(|s| ShipTrail::new(def, s))
                .collect(),
            rng: Rng(seed | 1),
        }
    }

    /// Records the nozzle positions for this simulation tick (ribbons) and advances the glow
    /// expansion counters.
    pub fn update(&mut self, def: &ShipTrailDef, tweaks: &TrailTweaks, tick: &TrailTick<'_>) {
        let rotation = Mat3::from_mat4(tick.transform);
        let translation = tick.transform.w_axis.truncate();
        for trail in &mut self.trails {
            if trail.is_glow() {
                trail.exponent = self.rng.real(tweaks.exponent_range) + tweaks.exponent_base;
                if trail.counter > 0 {
                    trail.counter -= 1;
                }
                if trail.counter == 0 {
                    match trail.state {
                        GlowState::Contracting => trail.state = GlowState::Contracted,
                        GlowState::Expanding => trail.state = GlowState::Expanded,
                        _ => {}
                    }
                }
                continue;
            }
            let Some(ramp) = &def.ramp else { continue };
            trail.grain_counter -= 1;
            if trail.grain_counter >= 0 {
                continue;
            }
            trail.grain_counter = ramp.granularity;
            let n = trail.segments.len();
            let offset = nozzle_offset(def, trail.slot, tick.objects);
            let axis = |v: Vec3| {
                let v = if trail.angle == -1.0 {
                    v
                } else {
                    (Mat3::from_rotation_z(trail.angle) * v).normalize_or_zero()
                };
                rotation * v
            };
            trail.segments[trail.head] = Segment {
                position: rotation * offset + translation,
                rotation,
                translation,
                horizontal: axis(Vec3::Y),
                vertical: axis(Vec3::X),
                wide: false,
            };
            if trail.length < n {
                trail.length += 1;
            }
            trail.head = (trail.head + 1) % n;
        }
    }

    /// Engine glow levels per slot for the ship's colour tint.
    pub fn engine_glow_levels(&self, tweaks: &TrailTweaks) -> [f32; 4] {
        let mut out = [0.0; 4];
        for (i, t) in self.trails.iter().enumerate().take(4) {
            match t.glow_level(tweaks.expansion_ticks) {
                Some(v) => out[i] = v,
                None => break,
            }
        }
        out
    }

    /// Draws the trails into `out`.
    pub fn draw(
        &mut self,
        def: &ShipTrailDef,
        tweaks: &TrailTweaks,
        frame: &TrailFrame<'_>,
        out: &mut EffectBatch,
    ) {
        self.draw_styled(def, tweaks, frame, None, out);
    }

    /// [`draw`](Self::draw) with ribbon trails drawn in `style` instead of the original's
    /// ribbons (capital glows are unchanged).
    pub fn draw_styled(
        &mut self,
        def: &ShipTrailDef,
        tweaks: &TrailTweaks,
        frame: &TrailFrame<'_>,
        style: Option<&RibbonStyle>,
        out: &mut EffectBatch,
    ) {
        let draw = frame.gate.allows_trail();
        for i in 0..self.trails.len() {
            if self.trails[i].is_glow() {
                let trail = &mut self.trails[i];
                let ticks = tweaks.expansion_ticks;
                if !draw {
                    match trail.state {
                        GlowState::Contracted => continue,
                        GlowState::Expanding | GlowState::Expanded => {
                            trail.state = GlowState::Contracting;
                            trail.counter = ticks.saturating_sub(trail.counter);
                        }
                        GlowState::Contracting => {}
                    }
                } else {
                    match trail.state {
                        GlowState::Contracting => {
                            trail.state = GlowState::Expanding;
                            trail.counter = ticks.saturating_sub(trail.counter);
                        }
                        GlowState::Contracted => {
                            trail.state = GlowState::Expanding;
                            trail.counter = ticks;
                        }
                        _ => {}
                    }
                }
                draw_capital_glow(def, tweaks, trail, frame, out);
            } else {
                if !draw {
                    self.trails[i].zero_length();
                    continue;
                }
                match style {
                    Some(st) => draw_styled_ribbon(def, tweaks, &self.trails[i], frame, st, out),
                    None => {
                        let mut rng = self.rng;
                        draw_ribbon(def, tweaks, &mut self.trails[i], frame, &mut rng, out);
                        self.rng = rng;
                    }
                }
            }
        }
    }
}

/// A replacement look for ribbon trails (the optional Homeworld Remastered trails): a textured
/// camera-facing ribbon through the recorded nozzle positions in the team's trail colour,
/// shaded along its length by `ramp`, with an optional glow sprite at the nozzle.
#[derive(Debug, Clone, PartialEq)]
pub struct RibbonStyle {
    pub texture: EffectTexture,
    /// Full width in world units.
    pub width: f32,
    /// Brightness from the nozzle (first) to the end of the trail (last), 0..1; the ribbon fades
    /// from the brightest value to the last.
    pub ramp: Vec<f32>,
    /// Most recorded positions drawn.
    pub max_points: usize,
    /// Nozzle glow: texture and radius.
    pub glow: Option<(EffectTexture, f32)>,
    /// How much the bright part of the trail is whitened (a hot core), 0..1.
    pub hot: f32,
    pub blend: Blend,
}

fn draw_styled_ribbon(
    def: &ShipTrailDef,
    tw: &TrailTweaks,
    trail: &ShipTrail,
    frame: &TrailFrame<'_>,
    style: &RibbonStyle,
    out: &mut EffectBatch,
) {
    if def.ramp.is_none() {
        return;
    }
    let rotation = Mat3::from_mat4(frame.transform);
    let translation = frame.transform.w_axis.truncate();
    let mut nozzle = nozzle_offset(def, trail.slot, frame.objects);
    nozzle.z -= 0.3 * tw.xadj;
    let current = rotation * (nozzle * frame.scale) + translation;
    let speed = frame.velocity.length();
    let color = |k: f32, a: f32| {
        let w = style.hot * k;
        let c = frame.team_color.map(|v| (v + (1.0 - v) * w) * k);
        [c[0], c[1], c[2], a * frame.fade]
    };
    if let Some((tex, radius)) = style.glow {
        let velratio = speed / frame.max_velocity.max(1e-6);
        if velratio >= tw.velratio_thresh && frame.lod < 3 {
            out.sprites.push(Sprite {
                position: current,
                half_size: Vec2::splat(radius * (0.6 + 0.4 * velratio.min(1.0))),
                rotation: 0.0,
                color: color(1.0, 1.0),
                uv: FULL_UV,
                texture: tex,
                blend: Blend::Additive,
            });
        }
    }
    if speed < 0.001 || trail.length <= 3 {
        return;
    }
    let n_seg = trail.segments.len();
    let n = trail.length.min(style.max_points.max(2));
    let mut points = Vec::with_capacity(n + 1);
    points.push(current);
    let mut index = if trail.head == 0 { n_seg - 1 } else { trail.head - 1 };
    for _ in 0..n {
        let p = trail.segments[index].position;
        if !p.is_finite() {
            break;
        }
        points.push(p);
        index = if index <= 1 { n_seg - 1 } else { index - 1 };
    }
    // Like the original ribbon, the oldest walked record is not drawn (a full ring wraps back
    // round to the newest one there, which drew a stray line along the trail).
    if points.len() > 2 {
        points.pop();
    }
    if points.len() < 2 {
        return;
    }
    if tw.smoothing > 1 && points.len() > 2 {
        let k = tw.smoothing;
        let mut smooth = Vec::with_capacity((points.len() - 1) * k + 1);
        for i in 0..points.len() - 1 {
            for j in 0..k {
                smooth.push(catmull_rom(&points, i, j as f32 / k as f32));
            }
        }
        smooth.push(points[points.len() - 1]);
        points = smooth;
    }
    // One strip (no seams between pieces), from the ramp's brightest (at the nozzle) to its end.
    let first = style.ramp.iter().copied().fold(0.0, f32::max);
    let last = style.ramp.last().copied().unwrap_or(0.0);
    out.strips.push(Strip {
        points,
        width: style.width * frame.scale,
        start_color: color(first, 1.0),
        end_color: color(last, 1.0),
        texture: style.texture,
        blend: style.blend,
    });
}

/// The point `t` (0..1) of the way from `p[i]` to `p[i + 1]` on a Catmull-Rom curve through
/// `p` (the ends repeat).
fn catmull_rom(p: &[Vec3], i: usize, t: f32) -> Vec3 {
    let a = p[i.saturating_sub(1)];
    let b = p[i];
    let c = p[(i + 1).min(p.len() - 1)];
    let d = p[(i + 2).min(p.len() - 1)];
    let (t2, t3) = (t * t, t * t * t);
    0.5 * ((2.0 * b) + (c - a) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (3.0 * b - a - 3.0 * c + d) * t3)
}

fn draw_capital_glow(
    def: &ShipTrailDef,
    tweaks: &TrailTweaks,
    trail: &mut ShipTrail,
    frame: &TrailFrame<'_>,
    out: &mut EffectBatch,
) {
    if trail.width == -1.0 || trail.state == GlowState::Contracted {
        return;
    }
    let mut family = if trail.width == 1.0 && trail.height == 1.0 {
        if def.race == 0 || def.race == 2 {
            GlowFamily::MothershipR1
        } else {
            GlowFamily::MothershipR2
        }
    } else {
        GlowFamily::Standard
    };
    if def.race == 4 && def.is_mothership {
        family = GlowFamily::MothershipP1;
        trail.width = 1.0;
        trail.height = 1.0;
        trail.ribbon_adjust = 1.0;
    }
    let speed = frame.velocity.length();
    if frame.max_velocity <= 0.0 || speed * 0.5 / frame.max_velocity == 0.0 {
        match trail.state {
            GlowState::Expanding | GlowState::Expanded => {
                trail.state = GlowState::Contracting;
                trail.counter = tweaks.expansion_ticks;
            }
            GlowState::Contracted => return,
            GlowState::Contracting => {}
        }
    }
    let nozzle = nozzle_offset(def, trail.slot, frame.objects);
    let (translate, factor) = match family {
        GlowFamily::Standard => {
            let f = if trail.scale_cap == -1.0 || trail.scale_cap == 0.0 {
                1.0
            } else {
                1.0 - frame.eye_space_z * trail.scale_cap
            };
            (nozzle, f)
        }
        GlowFamily::MothershipP1 => (nozzle, 1.0),
        _ => (Vec3::new(0.0, 0.0, nozzle.z), 1.0),
    };
    let ribbon_len = trail.ribbon_adjust * trail.width;
    let z = if trail.counter == 0 {
        ribbon_len
    } else {
        let frac = f32::from(trail.counter) / f32::from(tweaks.expansion_ticks);
        let frac = if trail.state == GlowState::Expanding {
            1.0 - frac
        } else {
            frac
        };
        let z = frac * ribbon_len;
        if z == 0.0 {
            0.023
        } else {
            z
        }
    };
    let mut scale = Vec3::new(trail.height, trail.width, z) * factor;
    let kind = match family {
        GlowFamily::MothershipP1 => GlowMeshKind::MothershipP1,
        _ if trail.style == 5 => GlowMeshKind::FrigateGlow,
        GlowFamily::Standard => GlowMeshKind::Glow,
        GlowFamily::MothershipR1 => {
            scale *= Vec3::new(1.0, 1.5, 1.0);
            GlowMeshKind::MothershipR1
        }
        GlowFamily::MothershipR2 => GlowMeshKind::MothershipR2,
    };
    let tc = frame.team_color.map(color_byte);
    out.glow_meshes.push(GlowMeshDraw {
        mesh: kind.mesh_index(frame.lod),
        transform: frame.transform * Mat4::from_translation(translate) * Mat4::from_scale(scale),
        color: rgba(tc, 0xa0, frame.fade),
        exponent: trail.exponent,
    });
}

#[allow(clippy::too_many_lines)]
fn draw_ribbon(
    def: &ShipTrailDef,
    tw: &TrailTweaks,
    trail: &mut ShipTrail,
    frame: &TrailFrame<'_>,
    rng: &mut Rng,
    out: &mut EffectBatch,
) {
    let Some(ramp) = &def.ramp else { return };
    let team = frame.team.min(ramp.colors.len().saturating_sub(1));
    let colors = &ramp.colors[team];
    let n_seg = trail.segments.len();
    let slot = &def.slots[trail.slot];
    let rotation = Mat3::from_mat4(frame.transform);
    let translation = frame.transform.w_axis.truncate();
    let mut nozzle = nozzle_offset(def, trail.slot, frame.objects);
    let speed = frame.velocity.length();
    let max_v = frame.max_velocity.max(1e-6);
    let head = trail.head;
    trail.segments[head].wide = false;

    // Nozzle glow sprite, with afterburner flag.
    if frame.lod < 3 {
        let mut wide = false;
        trail.ran_counter -= 1;
        if trail.ran_counter < 0 {
            trail.ran = 1.0 - (rng.next() % 1000) as f32 * (tw.ran_jitter * 0.0001);
            trail.ran_counter = tw.ran_max as i32;
        }
        let velratio = speed / max_v;
        if velratio >= tw.velratio_thresh {
            let mut rad = def.collision_radius;
            if slot.sprite_radius != -1.0 {
                rad = slot.sprite_radius;
            }
            let rad = (1.0 - tw.glow_baseradialscale)
                * (tw.glow_radialadjust * velratio * rad * trail.ran)
                + tw.glow_baseradialscale * rad;
            let mut pos = nozzle;
            pos.z -= 0.6 * tw.xadj;
            pos.z -= slot.sprite_offset;
            let world = rotation * pos + translation;
            let tc = frame.team_color.map(|v| color_byte(v * 0.8));
            out.sprites.push(Sprite {
                position: world,
                half_size: Vec2::splat(rad),
                rotation: 0.0,
                color: rgba(tc, 255, frame.fade),
                uv: FULL_UV,
                texture: frame.glow_texture,
                blend: Blend::Additive,
            });
            if trail.last_vel_sq - trail.prev_vel_sq > tw.accelthresh
                && velratio > tw.velratio_accelthresh
            {
                wide = true;
            }
        }
        if wide {
            trail.segments[head].wide = true;
        }
    }

    nozzle.z -= 0.3 * tw.xadj;
    nozzle *= frame.scale;
    let current = rotation * nozzle + translation;
    if speed < 0.001 {
        return;
    }
    if trail.length > 3 {
        let n = trail.length.min(38);
        let mut positions = vec![Vec3::ZERO; n + 1];
        let mut horizontals = vec![Vec3::ZERO; n + 1];
        let mut verticals = vec![Vec3::ZERO; n + 1];
        // Walk back from the newest segment; like the original, index 0 is skipped when
        // wrapping.
        let mut index = if trail.head == 0 {
            n_seg - 1
        } else {
            trail.head - 1
        };
        for i in 0..n {
            let s = &trail.segments[index];
            positions[i + 1] = s.position;
            horizontals[i + 1] = s.horizontal;
            verticals[i + 1] = s.vertical;
            index = if index <= 1 { n_seg - 1 } else { index - 1 };
        }
        horizontals[0] = horizontals[1];
        verticals[0] = verticals[1];
        let mut wides: Vec<bool> = (0..n).map(|i| trail.segments[i].wide).collect();
        wides.push(false);
        positions[n] = positions[n - 1];
        positions[0] = current;
        let mut n = n;
        if let Some(bad) = positions[..n].iter().position(|p| !p.is_finite()) {
            n = bad.saturating_sub(1);
        }
        // Each drawn point's recorded segment (for the colour ramp).
        let mut src: Vec<usize> = (0..=n).collect();
        if tw.smoothing > 1 && n >= 2 {
            let k = tw.smoothing;
            let (p, h, v) = (&positions[..n], &horizontals[..n], &verticals[..n]);
            let mut np = Vec::with_capacity((n - 1) * k + 2);
            let (mut nh, mut nv, mut nw, mut ns) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
            for i in 0..n - 1 {
                for j in 0..k {
                    let t = j as f32 / k as f32;
                    np.push(catmull_rom(p, i, t));
                    nh.push(h[i].lerp(h[i + 1], t).normalize_or(h[i]));
                    nv.push(v[i].lerp(v[i + 1], t).normalize_or(v[i]));
                    nw.push(wides[i] && j == 0);
                    ns.push(i);
                }
            }
            np.push(p[n - 1]);
            nh.push(h[n - 1]);
            nv.push(v[n - 1]);
            nw.push(wides[n - 1]);
            ns.push(n - 1);
            let m = np.len();
            np.push(p[n - 1]);
            nh.push(h[n - 1]);
            nv.push(v[n - 1]);
            nw.push(false);
            ns.push(n - 1);
            (positions, horizontals, verticals, wides, src, n) = (np, nh, nv, nw, ns, m);
        }
        // The ribbon narrows by the falloff per recorded segment, so by a share of it per piece.
        let piece = TrailTweaks {
            halfwidth_falloff: tw.halfwidth_falloff / tw.smoothing.max(1) as f32,
            ..*tw
        };
        let tw = &piece;
        let surprise = |i: usize, n: usize, c: [u8; 3], rng: &mut Rng| -> [u8; 3] {
            if tw.color_adjust == 0 || tw.coloradjust_freq == 0 || i as i64 > n as i64 - 5 {
                return c;
            }
            if (rng.next() % 100) as i32 > tw.coloradjust_freq {
                return c;
            }
            let r = (rng.next() % tw.color_adjust.max(1) as u32) as i32;
            let up = rng.next() & 1 != 0;
            c.map(|v| {
                let v = i32::from(v);
                (if up { (v + r).min(255) } else { (v - r).max(0) }) as u8
            })
        };
        let seg_color = |i: usize| colors[(i + usize::from(i == 0)).min(colors.len() - 1)];
        if frame.lod > LINE_CUTOFF_LOD {
            let alpha = (163.0 * frame.fade) as u8;
            let width = if frame.lod == 3 { 2.0 } else { 1.0 };
            for i in 0..n.saturating_sub(1) {
                let ca = surprise(i, n, seg_color(src[i]), rng);
                let cb = surprise(i + 1, n, seg_color(src[i + 1]), rng);
                out.lines.push(Line {
                    from: positions[i],
                    to: positions[i + 1],
                    from_color: rgba(ca, alpha, 1.0),
                    to_color: rgba(cb, alpha, 1.0),
                    width,
                    blend: Blend::Additive,
                });
            }
        } else {
            let mut ctx = RibbonContext {
                half_width: trail.half_width(tw) - tw.halfwidth_falloff,
                half_height: (if trail.height == -1.0 {
                    tw.halfwidth * 0.25
                } else {
                    trail.height * 0.5
                }) - tw.halfwidth_falloff,
                last_to: Vec3::ZERO,
                last_to_hi: Vec3::ZERO,
                cb: [0; 3],
                pyramid: PyramidState::default(),
                sheath: SheathState::default(),
            };
            #[allow(clippy::needless_range_loop)]
            for i in 0..n.saturating_sub(1) {
                let c = surprise(i, n, seg_color(src[i]), rng);
                ribbon_segment(tw, trail, frame, &mut ctx, i, &positions, c, out);
                if frame.lod < 2 {
                    if trail.style == 1 {
                        pyramid_segment(
                            tw,
                            trail,
                            frame,
                            &mut ctx,
                            i,
                            &positions,
                            c,
                            &horizontals,
                            &verticals,
                            wides[i],
                            out,
                        );
                    }
                } else {
                    sheath_segment(
                        tw,
                        trail,
                        frame,
                        &mut ctx,
                        i,
                        &positions,
                        c,
                        &horizontals,
                        out,
                    );
                }
                if ctx.half_width > tw.halfwidth_min {
                    ctx.half_width -= tw.halfwidth_falloff;
                    ctx.half_height -= trail.height_scalar() * tw.halfwidth_falloff;
                }
            }
        }
        if trail.ran_counter == 0 {
            trail.prev_vel_sq = trail.last_vel_sq;
            trail.last_vel_sq = frame.velocity.length_squared();
        }
    }
}

#[derive(Default)]
struct PyramidState {
    b0: Vec3,
    b1: Vec3,
    b2: Vec3,
    b0lo: Vec3,
    b0hi: Vec3,
    cb: [u8; 3],
}

#[derive(Default)]
struct SheathState {
    b0: Vec3,
    b1: Vec3,
    b2: Vec3,
    cb: [u8; 3],
}

struct RibbonContext {
    half_width: f32,
    half_height: f32,
    last_to: Vec3,
    last_to_hi: Vec3,
    cb: [u8; 3],
    pyramid: PyramidState,
    sheath: SheathState,
}

/// The camera-facing ribbon between two segments.
#[allow(clippy::too_many_arguments)]
fn ribbon_segment(
    tw: &TrailTweaks,
    trail: &ShipTrail,
    frame: &TrailFrame<'_>,
    ctx: &mut RibbonContext,
    i: usize,
    positions: &[Vec3],
    c: [u8; 3],
    out: &mut EffectBatch,
) {
    let (mut from, mut to) = (positions[i], positions[i + 1]);
    let veye = (from - frame.eye).normalize_or_zero();
    let vseg = (to - from).normalize_or_zero();
    let vcross = vseg.cross(veye).normalize_or_zero();
    let r = trail.ribbon();
    let mut from_hi = from + vcross * (r * (ctx.half_height + tw.halfwidth_falloff));
    let to_hi = to + vcross * (r * ctx.half_height);
    from -= vcross * (r * (ctx.half_height + tw.halfwidth_falloff));
    to -= vcross * (r * ctx.half_height);
    if i == 0 {
        ctx.cb = c;
    } else {
        from = ctx.last_to;
        from_hi = ctx.last_to_hi;
    }
    let alpha = (127.0 * frame.fade) as u8;
    out.quad(
        [
            (from, rgba(ctx.cb, alpha, 1.0)),
            (from_hi, rgba(ctx.cb, alpha, 1.0)),
            (to_hi, rgba(c, alpha, 1.0)),
            (to, rgba(c, alpha, 1.0)),
        ],
        Blend::Additive,
    );
    ctx.last_to = to;
    ctx.last_to_hi = to_hi;
    ctx.cb = c;
}

/// The four-sided translucent sheath around the ribbon (close up, style 1).
#[allow(clippy::too_many_arguments)]
fn pyramid_segment(
    tw: &TrailTweaks,
    trail: &ShipTrail,
    frame: &TrailFrame<'_>,
    ctx: &mut RibbonContext,
    i: usize,
    positions: &[Vec3],
    c_in: [u8; 3],
    horiz: &[Vec3],
    vert: &[Vec3],
    wide: bool,
    out: &mut EffectBatch,
) {
    let vs = trail.height_scalar();
    let half_width = trail.half_width(tw);
    let scale = frame.scale;
    let st = &mut ctx.pyramid;
    let (a1, a2, a0lo, a0hi, c);
    if i == 0 {
        let a0 = positions[0];
        let t = half_width * scale;
        a1 = a0 - horiz[0] * t;
        a2 = a0 + horiz[0] * t;
        let t = t * vs;
        a0lo = a0 - vert[0] * t;
        a0hi = a0 + vert[0] * t;
        st.cb = c_in;
        c = c_in;
    } else {
        a1 = st.b1;
        a2 = st.b2;
        a0lo = st.b0lo;
        a0hi = st.b0hi;
        c = st.cb;
        st.cb = c_in;
    }
    let b0 = positions[i + 1];
    let (width, height) = if wide {
        st.cb = hsv_saturation_shift(st.cb, tw.burn_coloradjust);
        (
            ctx.half_width * tw.burn_sizescale,
            vs * ctx.half_width * tw.burn_sizescale,
        )
    } else {
        (ctx.half_width, vs * ctx.half_width)
    };
    let t = width * scale;
    let b1 = b0 - horiz[i + 1] * t;
    let b2 = b0 + horiz[i + 1] * t;
    let t = height * scale;
    let b0lo = b0 - vert[i + 1] * t;
    let b0hi = b0 + vert[i + 1] * t;
    let cb = st.cb;
    let f = frame.fade;
    let (lo, hi) = (PYRAMID_ALPHA_LO, PYRAMID_ALPHA_HI);
    for quad in [
        [(a1, c, lo), (b1, cb, lo), (b0hi, cb, hi), (a0hi, c, hi)],
        [(a0hi, c, hi), (b0hi, cb, hi), (b2, cb, lo), (a2, c, lo)],
        [(a2, c, lo), (b2, cb, lo), (b0lo, cb, hi), (a0lo, c, hi)],
        [(a0lo, c, hi), (b0lo, cb, hi), (b1, cb, lo), (a1, c, lo)],
    ] {
        out.quad(
            quad.map(|(p, col, a)| (p, rgba(col, a, f))),
            Blend::Additive,
        );
    }
    st.b0 = b0;
    st.b1 = b1;
    st.b2 = b2;
    st.b0lo = b0lo;
    st.b0hi = b0hi;
}

/// A flat translucent band along the trail (middle distance).
#[allow(clippy::too_many_arguments)]
fn sheath_segment(
    tw: &TrailTweaks,
    trail: &ShipTrail,
    frame: &TrailFrame<'_>,
    ctx: &mut RibbonContext,
    i: usize,
    positions: &[Vec3],
    c_in: [u8; 3],
    horiz: &[Vec3],
    out: &mut EffectBatch,
) {
    let scalar = 0.9;
    let half_width = scalar * trail.half_width(tw);
    let scale = frame.scale;
    let st = &mut ctx.sheath;
    let (a1, a2, c);
    if i == 0 {
        let a0 = positions[0];
        let t = half_width * scale;
        a1 = a0 - horiz[0] * t;
        a2 = a0 + horiz[0] * t;
        st.cb = c_in;
        c = c_in;
    } else {
        a1 = st.b1;
        a2 = st.b2;
        c = st.cb;
        st.cb = c_in;
    }
    let b0 = positions[i + 1];
    let t = scalar * ctx.half_width * scale;
    let b1 = b0 - horiz[i + 1] * t;
    let b2 = b0 + horiz[i + 1] * t;
    let cb = st.cb;
    let a = PYRAMID_ALPHA_MID;
    let f = frame.fade;
    out.quad(
        [
            (a1, rgba(c, a, f)),
            (b1, rgba(cb, a, f)),
            (b2, rgba(cb, a, f)),
            (a2, rgba(c, a, f)),
        ],
        Blend::Additive,
    );
    st.b0 = b0;
    st.b1 = b1;
    st.b2 = b2;
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwc_formats::script::ScriptFile;

    const SHP: &str = "trailSegments 10\ntrailGranularity 2\ntrailColor 0, 0, 0, 0, 0\ntrailColor 0, 1, 238, 238, 238\ntrailColor 0, 4, 141, 106, 87\ntrailColor 0, 9, 0, 0, 0\ntrailWidth 12\ntrailHeight 6\ntrailAngle 0\ntrailSpriteRadius 30\ntrailSpriteOffset -3.5\nshipclass CLASS_Fighter\n";

    fn def() -> ShipTrailDef {
        let stats = ShipStats::from_script(&ScriptFile::parse(SHP));
        ShipTrailDef::new(
            &stats,
            vec![EngineNozzle {
                index: 0,
                position: Vec3::new(0.0, 0.0, -20.0),
                normal: -Vec3::Z,
                attached_to: None,
            }],
            10.0,
            0,
        )
    }

    #[test]
    fn nozzles_go_through_their_object_or_the_first_one() {
        use crate::mesh::{Skeleton, SkeletonObject};
        // A root object off the origin (the ramming frigate's sits at z = -162) and a child.
        let sk = Skeleton {
            objects: vec![
                SkeletonObject {
                    name: "Hull".into(),
                    parent: None,
                    rest: Mat4::from_translation(Vec3::new(0.0, 0.0, -162.0)),
                },
                SkeletonObject {
                    name: "Nose".into(),
                    parent: Some(0),
                    rest: Mat4::from_translation(Vec3::new(0.0, 0.0, -12.0)),
                },
            ],
        };
        let mats = sk.rest_matrices();
        let mut d = def();
        let nozzle = |index, z, attached: Option<&str>| EngineNozzle {
            index,
            position: Vec3::new(1.0, 0.0, z),
            normal: -Vec3::Z,
            attached_to: attached.map(str::to_string),
        };
        d.nozzles = vec![
            nozzle(0, -68.5, Some("hull")),
            nozzle(1, 236.0, Some("Nose")),
            nozzle(2, 10.0, None),
            nozzle(3, 20.0, Some("NotInThisLevel")),
        ];
        let objects = Some((&sk, mats.as_slice()));
        assert_eq!(nozzle_offset(&d, 0, objects), Vec3::new(1.0, 0.0, -230.5));
        assert_eq!(nozzle_offset(&d, 1, objects), Vec3::new(1.0, 0.0, 62.0));
        // No object named, or one this level lacks: the first object's matrix.
        assert_eq!(nozzle_offset(&d, 2, objects), Vec3::new(1.0, 0.0, -152.0));
        assert_eq!(nozzle_offset(&d, 3, objects), Vec3::new(1.0, 0.0, -142.0));
        // Without the objects, the file position.
        assert_eq!(nozzle_offset(&d, 0, None), Vec3::new(1.0, 0.0, -68.5));
    }

    #[test]
    fn colour_ramp_interpolates_between_keys() {
        let d = def();
        let r = d.ramp.as_ref().unwrap();
        assert_eq!(r.segments, 10);
        assert_eq!(r.colors[0][1], [238, 238, 238]);
        // Halfway between index 1 and 4.
        assert_eq!(r.colors[0][2], [205, 194, 187]);
        assert!(r.keys[0][4] && !r.keys[0][5]);
        assert_eq!(d.slots[0].width, 12.0);
        assert_eq!(d.slots[0].sprite_offset, -3.5);
        assert_eq!(d.slots[1].width, -1.0);
    }

    #[test]
    fn recolorize_replaces_control_points_in_order() {
        let mut r = def().ramp.unwrap();
        r.recolorize(0, &[[1, 1, 1], [100, 0, 0], [50, 0, 0], [0, 0, 0]]);
        assert_eq!(r.colors[0][1], [100, 0, 0]);
        assert_eq!(r.colors[0][4], [50, 0, 0]);
        assert_eq!(r.colors[0][0], [1, 1, 1]);
    }

    #[test]
    fn ribbon_ring_and_drawing() {
        let d = def();
        let tw = TrailTweaks::default();
        let mut trails = ShipTrails::new(&d, 7);
        assert_eq!(trails.trails.len(), 1);
        for step in 0..30 {
            let t = Mat4::from_translation(Vec3::new(0.0, 0.0, step as f32 * 10.0));
            trails.update(
                &d,
                &tw,
                &TrailTick {
                    transform: t,
                    objects: None,
                },
            );
        }
        // Granularity 2: one record every third tick.
        assert_eq!(trails.trails[0].len(), 10);
        let frame = TrailFrame {
            transform: Mat4::from_translation(Vec3::new(0.0, 0.0, 300.0)),
            objects: None,
            velocity: Vec3::new(0.0, 0.0, 100.0),
            max_velocity: 100.0,
            gate: TrailGate {
                moving: true,
                ..Default::default()
            },
            lod: 0,
            team: 0,
            team_color: [1.0; 3],
            eye: Vec3::new(500.0, 0.0, 300.0),
            eye_space_z: -500.0,
            scale: 1.0,
            fade: 1.0,
            glow_texture: EffectTexture::WHITE,
        };
        let mut out = EffectBatch::default();
        trails.draw(&d, &tw, &frame, &mut out);
        assert_eq!(out.sprites.len(), 1, "nozzle glow");
        assert!(!out.triangles.is_empty(), "ribbon");
        let far = TrailFrame { lod: 3, ..frame };
        let mut lines = EffectBatch::default();
        trails.draw(&d, &tw, &far, &mut lines);
        assert!(!lines.lines.is_empty());
        assert!(lines.sprites.is_empty());
        // Stopping clears the ribbon.
        let stopped = TrailFrame {
            gate: TrailGate::default(),
            ..frame
        };
        trails.draw(&d, &tw, &stopped, &mut EffectBatch::default());
        assert!(trails.trails[0].is_empty());
    }

    #[test]
    fn smooth_trails_curve_through_the_recorded_points() {
        let d = def();
        let mut tw = TrailTweaks::default();
        let mut trails = ShipTrails::new(&d, 7);
        let at = |step: usize| {
            let a = step as f32 * 0.1;
            Vec3::new(1000.0 * a.cos(), 0.0, 1000.0 * a.sin())
        };
        for step in 0..30 {
            let tick = TrailTick {
                transform: Mat4::from_translation(at(step)),
                objects: None,
            };
            trails.update(&d, &tw, &tick);
        }
        let frame = TrailFrame {
            transform: Mat4::from_translation(at(29)),
            objects: None,
            velocity: Vec3::new(0.0, 0.0, 100.0),
            max_velocity: 100.0,
            gate: TrailGate {
                moving: true,
                ..Default::default()
            },
            lod: 3,
            team: 0,
            team_color: [1.0; 3],
            eye: Vec3::new(0.0, 3000.0, 0.0),
            eye_space_z: -3000.0,
            scale: 1.0,
            fade: 1.0,
            glow_texture: EffectTexture::WHITE,
        };
        let draw = |tw: &TrailTweaks, trails: &mut ShipTrails| {
            let mut out = EffectBatch::default();
            trails.draw(&d, tw, &frame, &mut out);
            out.lines
        };
        let straight = draw(&tw, &mut trails);
        tw.smoothing = 4;
        let smooth = draw(&tw, &mut trails);
        assert!(smooth.len() >= 3 * straight.len(), "{} vs {}", smooth.len(), straight.len());
        for w in smooth.windows(2) {
            assert!((w[0].to - w[1].from).length() < 1e-3, "gap");
        }
        for l in &smooth {
            assert!(l.from_color[3] > 0.0 && l.to_color[3] > 0.0);
        }
        // The styled ribbon: one strip through the curved points.
        let style = RibbonStyle {
            texture: EffectTexture::WHITE,
            width: 5.0,
            ramp: vec![1.0, 0.5, 0.0],
            max_points: 38,
            glow: None,
            hot: 0.5,
            blend: Blend::Additive,
        };
        let near = TrailFrame { lod: 0, ..frame };
        let mut out = EffectBatch::default();
        trails.draw_styled(&d, &tw, &near, Some(&style), &mut out);
        assert_eq!(out.strips.len(), 1);
        let s = &out.strips[0];
        assert!(s.points.len() > 20, "{}", s.points.len());
        assert!(s.start_color[0] > 0.5 && s.start_color[3] == 1.0, "{:?}", s.start_color);
    }

    #[test]
    fn capital_glow_expands_and_contracts() {
        let stats = ShipStats::from_script(&ScriptFile::parse(
            "trailWidth 30\ntrailHeight 20\ntrailStyle 3\n",
        ));
        let d = ShipTrailDef::new(
            &stats,
            vec![EngineNozzle {
                index: 0,
                position: Vec3::ZERO,
                normal: Vec3::Z,
                attached_to: None,
            }],
            100.0,
            0,
        );
        assert!(d.ramp.is_none());
        let tw = TrailTweaks::default();
        let mut trails = ShipTrails::new(&d, 3);
        assert_eq!(trails.trails[0].style, 5);
        assert!((trails.trails[0].ribbon_adjust - 1.28).abs() < 1e-6);
        let mut frame = TrailFrame {
            transform: Mat4::IDENTITY,
            objects: None,
            velocity: Vec3::new(0.0, 0.0, 50.0),
            max_velocity: 100.0,
            gate: TrailGate {
                moving: true,
                ..Default::default()
            },
            lod: 0,
            team: 0,
            team_color: [1.0, 0.5, 0.2],
            eye: Vec3::new(0.0, 500.0, 0.0),
            eye_space_z: -500.0,
            scale: 1.0,
            fade: 1.0,
            glow_texture: EffectTexture::WHITE,
        };
        let mut out = EffectBatch::default();
        trails.draw(&d, &tw, &frame, &mut out);
        assert_eq!(trails.trails[0].state, GlowState::Expanding);
        assert_eq!(out.glow_meshes.len(), 1);
        assert_eq!(
            out.glow_meshes[0].mesh,
            GlowMeshKind::FrigateGlow.mesh_index(0)
        );
        for _ in 0..16 {
            trails.update(
                &d,
                &tw,
                &TrailTick {
                    transform: Mat4::IDENTITY,
                    objects: None,
                },
            );
        }
        assert_eq!(trails.trails[0].state, GlowState::Expanded);
        assert_eq!(trails.engine_glow_levels(&tw), [1.0, 0.0, 0.0, 0.0]);
        frame.gate.moving = false;
        trails.draw(&d, &tw, &frame, &mut EffectBatch::default());
        assert_eq!(trails.trails[0].state, GlowState::Contracting);
        assert!(trails.trails[0].exponent >= tw.exponent_base);
    }

    #[test]
    fn team_trail_colours_from_script() {
        let s =
            ScriptFile::parse("TrailColor 1, 1, 10, 20, 30\nBaseToTrailFactors1 0.0, 1.4, 1.0\n");
        let mut c = TeamTrailColors::from_script(&s);
        assert_eq!(c.teams[1][1], [10, 20, 30]);
        c.derive_from_base(0, [66, 108, 202]);
        // Lightness 0.2 x darkens the first control point.
        assert!(c.teams[0][0].iter().all(|&v| v < 60));
        let back = hls_to_rgb(rgb_to_hls([0.2, 0.4, 0.8]));
        assert!((back[2] - 0.8).abs() < 1e-5);
        assert_eq!(hsv_saturation_shift([255, 0, 0], -255), [255, 255, 255]);
    }
}
