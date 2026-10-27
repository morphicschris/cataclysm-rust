//! Homeworld 2 effect definitions (`art/fx/*.lua`).
//!
//! Each file assigns `fx = { style = "STYLE_...", properties = { property_NN = { name, type,
//! value } } }`. Curves (`VARTYPE_ARRAY_TIME*`) run over the effect's normalised lifetime 0..1:
//! `TIMEFLOAT` is a flat list `t0, v0, t1, v1, ...`, `TIMECOLOUR` and `TIMEVECTOR3` are entries
//! `{t, r, g, b, a}` / `{t, x, y, z}`. The typed views ([`Ring`], [`Spray`], ...) read the
//! properties the stock effects use, with neutral defaults for anything missing.

use std::collections::HashMap;

use crate::big::normalize;
use crate::lua::{self, Value};

/// A curve of scalars over normalised time.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Curve(pub Vec<(f32, f32)>);

/// A curve of RGBA colours.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ColourCurve(pub Vec<(f32, [f32; 4])>);

/// A curve of vectors.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VecCurve(pub Vec<(f32, [f32; 3])>);

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn span<T: Copy>(keys: &[(f32, T)], t: f32) -> Option<(T, T, f32)> {
    let first = keys.first()?;
    if keys.len() == 1 || t <= first.0 {
        return Some((first.1, first.1, 0.0));
    }
    for w in keys.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t <= b.0 {
            let d = b.0 - a.0;
            let f = if d > 1e-6 { (t - a.0) / d } else { 1.0 };
            return Some((a.1, b.1, f));
        }
    }
    let last = keys.last()?.1;
    Some((last, last, 0.0))
}

impl Curve {
    pub fn constant(v: f32) -> Self {
        Self(vec![(0.0, v)])
    }

    pub fn at(&self, t: f32) -> f32 {
        span(&self.0, t).map_or(0.0, |(a, b, f)| lerp(a, b, f))
    }

    pub fn max(&self) -> f32 {
        self.0.iter().map(|k| k.1).fold(0.0, f32::max)
    }
}

impl ColourCurve {
    pub fn at(&self, t: f32) -> [f32; 4] {
        span(&self.0, t).map_or([1.0; 4], |(a, b, f)| {
            std::array::from_fn(|i| lerp(a[i], b[i], f))
        })
    }
}

impl VecCurve {
    pub fn at(&self, t: f32) -> [f32; 3] {
        span(&self.0, t).map_or([0.0; 3], |(a, b, f)| {
            std::array::from_fn(|i| lerp(a[i], b[i], f))
        })
    }
}

/// One property value.
#[derive(Debug, Clone, PartialEq)]
pub enum Prop {
    Float(f32),
    Int(i32),
    Bool(bool),
    Str(String),
    Colour([f32; 4]),
    Curve(Curve),
    Colours(ColourCurve),
    Vectors(VecCurve),
}

/// The kind of an effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Ring,
    Spray,
    Combo,
    Beam,
    Light,
    Trail,
    LensFlare,
    Hyperspace,
    Other,
}

impl Style {
    fn parse(s: &str) -> Self {
        match s.to_ascii_uppercase().as_str() {
            "STYLE_RING" => Self::Ring,
            "STYLE_SPRAY" => Self::Spray,
            "STYLE_COMBO" => Self::Combo,
            "STYLE_BEAM" => Self::Beam,
            "STYLE_LIGHT" => Self::Light,
            "STYLE_TRAIL" => Self::Trail,
            "STYLE_LENSFLARE" => Self::LensFlare,
            "STYLE_HYPERSPACE" => Self::Hyperspace,
            _ => Self::Other,
        }
    }
}

/// A parsed effect file.
#[derive(Debug, Clone, PartialEq)]
pub struct FxDef {
    /// Lowercase name (the file stem), which other effects and ship events refer to.
    pub name: String,
    pub style: Style,
    pub props: HashMap<String, Prop>,
}

fn nums(v: &Value) -> Vec<f32> {
    v.as_table().map_or_else(Vec::new, |t| {
        t.items
            .iter()
            .filter_map(|v| v.as_num())
            .map(|n| n as f32)
            .collect()
    })
}

/// `entry_NN = {...}` rows of a table (or its positional items), in key order.
fn entries(v: &Value) -> Vec<Vec<f32>> {
    let Some(t) = v.as_table() else {
        return Vec::new();
    };
    let mut keyed: Vec<(&str, Vec<f32>)> = t
        .fields
        .iter()
        .map(|(k, v)| (k.as_str(), nums(v)))
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(b.0));
    let mut rows: Vec<Vec<f32>> = keyed.into_iter().map(|(_, r)| r).collect();
    rows.extend(t.items.iter().map(nums).filter(|r| !r.is_empty()));
    rows
}

fn sorted<T>(mut v: Vec<(f32, T)>) -> Vec<(f32, T)> {
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    v
}

fn prop(ty: &str, v: &Value) -> Option<Prop> {
    Some(match ty.to_ascii_uppercase().as_str() {
        "VARTYPE_FLOAT" => Prop::Float(v.as_num()? as f32),
        "VARTYPE_INT" => Prop::Int(v.as_num()? as i32),
        "VARTYPE_BOOL" => Prop::Bool(v.as_num()? != 0.0),
        "VARTYPE_STRING" => Prop::Str(v.as_str()?.to_string()),
        "VARTYPE_COLOUR" => {
            let n = nums(v);
            Prop::Colour(std::array::from_fn(|i| n.get(i).copied().unwrap_or(1.0)))
        }
        "VARTYPE_ARRAY_TIMEFLOAT" => {
            let n = nums(v);
            Prop::Curve(Curve(sorted(
                n.chunks_exact(2).map(|c| (c[0], c[1])).collect(),
            )))
        }
        "VARTYPE_ARRAY_TIMECOLOUR" => Prop::Colours(ColourCurve(sorted(
            entries(v)
                .into_iter()
                .filter(|r| r.len() >= 5)
                .map(|r| (r[0], [r[1], r[2], r[3], r[4]]))
                .collect(),
        ))),
        "VARTYPE_ARRAY_TIMEVECTOR3" => Prop::Vectors(VecCurve(sorted(
            entries(v)
                .into_iter()
                .filter(|r| r.len() >= 4)
                .map(|r| (r[0], [r[1], r[2], r[3]]))
                .collect(),
        ))),
        _ => return None,
    })
}

impl FxDef {
    /// Parses an effect file; `name` is its stem.
    pub fn parse(name: &str, src: &[u8]) -> Option<Self> {
        let chunk = lua::parse_bytes(src);
        let fx = chunk.global("fx")?;
        let style = Style::parse(fx.get("style").and_then(Value::as_str).unwrap_or(""));
        let mut props = HashMap::new();
        if let Some(t) = fx.get("properties").and_then(Value::as_table) {
            for (_, p) in &t.fields {
                let (Some(n), Some(ty), Some(v)) = (
                    p.get("name").and_then(Value::as_str),
                    p.get("type").and_then(Value::as_str),
                    p.get("value"),
                ) else {
                    continue;
                };
                if let Some(pv) = prop(ty, v) {
                    props.insert(n.to_ascii_lowercase(), pv);
                }
            }
        }
        Some(Self {
            name: name.to_ascii_lowercase(),
            style,
            props,
        })
    }

    fn p(&self, name: &str) -> Option<&Prop> {
        self.props.get(&name.to_ascii_lowercase())
    }

    pub fn float(&self, name: &str, default: f32) -> f32 {
        match self.p(name) {
            Some(Prop::Float(f)) => *f,
            Some(Prop::Int(i)) => *i as f32,
            Some(Prop::Curve(c)) => c.at(0.0),
            _ => default,
        }
    }

    pub fn int(&self, name: &str, default: i32) -> i32 {
        match self.p(name) {
            Some(Prop::Int(i)) => *i,
            Some(Prop::Float(f)) => *f as i32,
            _ => default,
        }
    }

    pub fn bool(&self, name: &str, default: bool) -> bool {
        match self.p(name) {
            Some(Prop::Bool(b)) => *b,
            Some(Prop::Int(i)) => *i != 0,
            _ => default,
        }
    }

    /// A string property, empty strings as `None`.
    pub fn string(&self, name: &str) -> Option<&str> {
        match self.p(name) {
            Some(Prop::Str(s)) if !s.trim().is_empty() => Some(s.trim()),
            _ => None,
        }
    }

    pub fn curve(&self, name: &str, default: f32) -> Curve {
        match self.p(name) {
            Some(Prop::Curve(c)) if !c.0.is_empty() => c.clone(),
            Some(Prop::Float(f)) => Curve::constant(*f),
            _ => Curve::constant(default),
        }
    }

    pub fn colours(&self, name: &str) -> ColourCurve {
        match self.p(name) {
            Some(Prop::Colours(c)) if !c.0.is_empty() => c.clone(),
            Some(Prop::Colour(c)) => ColourCurve(vec![(0.0, *c)]),
            _ => ColourCurve(vec![(0.0, [1.0; 4])]),
        }
    }

    pub fn colour(&self, name: &str) -> [f32; 4] {
        match self.p(name) {
            Some(Prop::Colour(c)) => *c,
            Some(Prop::Colours(c)) => c.at(0.0),
            _ => [1.0; 4],
        }
    }

    pub fn vectors(&self, name: &str) -> VecCurve {
        match self.p(name) {
            Some(Prop::Vectors(v)) => v.clone(),
            _ => VecCurve::default(),
        }
    }

    /// Names of the effects this one starts (combo slots, spray particles, spawns), lowercase.
    pub fn children(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut keys: Vec<&String> = self.props.keys().collect();
        keys.sort();
        for k in keys {
            let is_fx = (k.starts_with("fx") && k[2..].chars().all(|c| c.is_ascii_digit()))
                || k == "particle_fx"
                || k == "spawn_fx"
                || k == "birthspawn_fx";
            if is_fx {
                if let Some(s) = self.string(k) {
                    out.push(s.to_ascii_lowercase());
                }
            }
        }
        out
    }

    /// Texture or animation paths this effect draws with (normalised).
    pub fn textures(&self) -> Vec<String> {
        ["texture"]
            .iter()
            .filter_map(|k| self.string(k))
            .map(normalize)
            .collect()
    }
}

/// How a primitive blends (`Blending`: 2 adds, the others blend by alpha).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlendMode {
    Additive,
    Alpha,
}

impl BlendMode {
    pub fn from_int(b: i32) -> Self {
        if b == 2 {
            Self::Additive
        } else {
            Self::Alpha
        }
    }
}

/// `STYLE_RING`: a (usually camera-facing) textured quad.
#[derive(Debug, Clone, PartialEq)]
pub struct Ring {
    pub duration: f32,
    pub looping: bool,
    pub radius: Curve,
    pub radius_epsilon: f32,
    pub colour: ColourCurve,
    pub offset: VecCurve,
    pub spin_z: Curve,
    pub spin_random: bool,
    pub texture: Option<String>,
    pub random_bitmap: bool,
    pub blend: BlendMode,
    pub billboard: bool,
    pub spawn: Option<String>,
    pub birth_spawn: Option<String>,
}

impl Ring {
    pub fn from_def(d: &FxDef) -> Self {
        Self {
            duration: d.float("Duration", 1.0).max(0.01),
            looping: d.bool("Loop", false),
            radius: d.curve("Radius", 1.0),
            radius_epsilon: d.float("RadiusEpsilon", 0.0),
            colour: d.colours("Colour"),
            offset: d.vectors("Offset"),
            spin_z: d.curve("SpinZ", 0.0),
            spin_random: d.bool("SpinRandom", false),
            texture: d.string("Texture").map(normalize),
            random_bitmap: d.bool("RandomBitmap", false),
            blend: BlendMode::from_int(d.int("Blending", 1)),
            billboard: d.bool("Billboard", true),
            spawn: d.string("Spawn_Fx").map(str::to_ascii_lowercase),
            birth_spawn: d.string("BirthSpawn_Fx").map(str::to_ascii_lowercase),
        }
    }
}

/// `STYLE_SPRAY`: an emitter whose particles are other effects.
#[derive(Debug, Clone, PartialEq)]
pub struct Spray {
    pub duration: f32,
    pub looping: bool,
    /// Particles per second.
    pub rate: Curve,
    /// Initial particle speed.
    pub speed: Curve,
    /// Cone half-angle in degrees around the emitter's direction (360: every direction).
    pub deviation: Curve,
    pub direction: i32,
    pub offset: VecCurve,
    pub volume: VecCurve,
    pub drag: f32,
    pub inherit_velocity: f32,
    pub scale_speed: bool,
    pub particle_scale: Curve,
    pub particle: Option<String>,
    pub force_first: bool,
}

impl Spray {
    pub fn from_def(d: &FxDef) -> Self {
        Self {
            duration: d.float("Emitter_Duration", 1.0).max(0.0),
            looping: d.bool("Emitter_Loop", false),
            rate: d.curve("Emitter_Rate", 0.0),
            speed: d.curve("Particle_Speed", 0.0),
            deviation: d.curve("Emitter_Deviation", 0.0),
            direction: d.int("Emitter_Direction", 0),
            offset: d.vectors("Emitter_Offset"),
            volume: d.vectors("Emitter_Volume"),
            drag: d.float("Emitter_Drag", 1.0),
            inherit_velocity: d.float("Emitter_InheritVelocity", 0.0),
            scale_speed: d.bool("Emitter_ScaleSpeed", false),
            particle_scale: d.curve("Particle_Scale", 1.0),
            particle: d.string("Particle_Fx").map(str::to_ascii_lowercase),
            force_first: d.bool("Force_First", false),
        }
    }
}

/// `STYLE_TRAIL`: a ribbon left behind by a moving parent.
#[derive(Debug, Clone, PartialEq)]
pub struct Trail {
    pub duration: f32,
    pub width: f32,
    /// How long a point of the ribbon lives, seconds.
    pub length: f32,
    pub head: [f32; 4],
    pub tail: [f32; 4],
    pub texture: Option<String>,
    pub blend: BlendMode,
}

impl Trail {
    pub fn from_def(d: &FxDef) -> Self {
        Self {
            duration: d.float("Duration", 1.0).max(0.01),
            width: d.float("Width", 1.0),
            length: d.float("Length", 1.0).max(0.01),
            head: d.colour("ColourHead"),
            tail: d.colour("ColourTail"),
            texture: d.string("Texture").map(normalize),
            blend: BlendMode::from_int(d.int("Blending", 2)),
        }
    }
}

/// `STYLE_LIGHT`: a dynamic light; drawn here as a soft glow.
#[derive(Debug, Clone, PartialEq)]
pub struct Light {
    pub duration: f32,
    pub radius: Curve,
    pub colour: ColourCurve,
}

impl Light {
    pub fn from_def(d: &FxDef) -> Self {
        Self {
            duration: d.float("Duration", 1.0).max(0.01),
            radius: d.curve("Radius", 1.0),
            colour: d.colours("Colour"),
        }
    }
}

/// `STYLE_COMBO`: up to eight effects started together.
#[derive(Debug, Clone, PartialEq)]
pub struct Combo {
    pub children: Vec<String>,
}

/// A compiled effect.
#[derive(Debug, Clone, PartialEq)]
pub enum Effect {
    Ring(Ring),
    Spray(Spray),
    Combo(Combo),
    Trail(Trail),
    Light(Light),
    /// Styles not played (beams, lens flares, hyperspace).
    Unsupported,
}

impl Effect {
    pub fn from_def(d: &FxDef) -> Self {
        match d.style {
            Style::Ring => Self::Ring(Ring::from_def(d)),
            Style::Spray => Self::Spray(Spray::from_def(d)),
            Style::Combo => Self::Combo(Combo {
                children: d.children(),
            }),
            Style::Trail => Self::Trail(Trail::from_def(d)),
            Style::Light => Self::Light(Light::from_def(d)),
            _ => Self::Unsupported,
        }
    }
}

/// Texture rectangle `[u0, v0, u1, v1]` of a whole image.
pub const FULL_UV: [f32; 4] = [0.0, 0.0, 1.0, 1.0];

/// One frame of an animation: an image, or a cell of a sprite sheet.
#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub path: String,
    pub uv: [f32; 4],
}

/// An animated texture (`*.anim`): a list of images (`texture = { ... }`) or cells of sprite
/// sheets (`layout = { rows, columns }`, `textureLayout = { { image, row, column }, ... }`; frames
/// run along each row, e.g. 8 rows of 4 on a 512 x 1024 sheet).
#[derive(Debug, Clone, PartialEq)]
pub struct Anim {
    pub fps: f32,
    pub loops: bool,
    pub frames: Vec<Frame>,
}

impl Anim {
    pub fn parse(src: &[u8]) -> Self {
        let c = lua::parse_bytes(src);
        let mut frames: Vec<Frame> =
            c.global("texture")
                .and_then(Value::as_table)
                .map_or_else(Vec::new, |t| {
                    t.items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(|p| Frame {
                            path: normalize(p),
                            uv: FULL_UV,
                        })
                        .collect()
                });
        if let Some(cells) = c.global("textureLayout").and_then(Value::as_table) {
            let layout = c.global("layout").and_then(Value::as_table);
            let dim = |i: usize| {
                layout
                    .and_then(|l| l.items.get(i))
                    .and_then(Value::as_num)
                    .unwrap_or(1.0)
                    .max(1.0) as f32
            };
            let (rows, cols) = (dim(0), dim(1));
            for cell in cells.items.iter().filter_map(Value::as_table) {
                let Some(path) = cell.items.first().and_then(Value::as_str) else {
                    continue;
                };
                let n = |i: usize| cell.items.get(i).and_then(Value::as_num).unwrap_or(0.0) as f32;
                let (y, x) = (n(1), n(2));
                frames.push(Frame {
                    path: normalize(path),
                    uv: [x / cols, y / rows, (x + 1.0) / cols, (y + 1.0) / rows],
                });
            }
        }
        Self {
            fps: c.global("fps").and_then(Value::as_num).unwrap_or(15.0) as f32,
            loops: c.global("loopCount").and_then(Value::as_num).unwrap_or(1.0) != 1.0,
            frames,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RING: &str = r#"fx = {
properties = {
property_03 = { name = "Colour", type = "VARTYPE_ARRAY_TIMECOLOUR", value = {
 entry_01 = { 1, 1, 0.5, 0, 0, }, entry_00 = { 0, 1, 1, 1, 1, }, }, },
property_00 = { name = "Radius", type = "VARTYPE_ARRAY_TIMEFLOAT", value = { 0, 2, 1, 4, }, },
property_15 = { name = "Texture", type = "VARTYPE_STRING", value = "DATA:ART/FX/TEXTURES/BIGFIREBALL.TGA", },
property_14 = { name = "Blending", type = "VARTYPE_INT", value = 2, },
property_08 = { name = "Duration", type = "VARTYPE_FLOAT", value = 3, },
property_20 = { name = "Spawn_Fx", type = "VARTYPE_STRING", value = "Child_FX", },
},
style = "STYLE_RING",
}"#;

    #[test]
    fn ring_definition() {
        let d = FxDef::parse("Test", RING.as_bytes()).unwrap();
        assert_eq!(d.name, "test");
        assert_eq!(d.style, Style::Ring);
        let Effect::Ring(r) = Effect::from_def(&d) else {
            panic!()
        };
        assert_eq!(r.duration, 3.0);
        assert_eq!(r.radius.at(0.5), 3.0);
        assert_eq!(r.colour.at(0.5), [1.0, 0.75, 0.5, 0.5]);
        assert_eq!(
            r.texture.as_deref(),
            Some("art/fx/textures/bigfireball.tga")
        );
        assert_eq!(r.blend, BlendMode::Additive);
        assert_eq!(d.children(), vec!["child_fx".to_string()]);
    }

    #[test]
    fn curves_clamp() {
        let c = Curve(vec![(0.0, 1.0), (0.5, 3.0), (1.0, 3.0)]);
        assert_eq!(c.at(-1.0), 1.0);
        assert_eq!(c.at(0.25), 2.0);
        assert_eq!(c.at(2.0), 3.0);
        assert_eq!(Curve::default().at(0.3), 0.0);
    }

    #[test]
    fn anim() {
        let a =
            Anim::parse(b"fps = 20;\nloopCount =01\n;\ntexture = {\n \"data:/art/fx/A.tga\",\n}");
        assert_eq!(a.fps, 20.0);
        assert!(!a.loops);
        assert_eq!(
            a.frames,
            vec![Frame {
                path: "art/fx/a.tga".into(),
                uv: FULL_UV
            }]
        );
    }

    #[test]
    fn sprite_sheet_anim() {
        let src = "fps = 30;\nlayout = {2,4};\ntextureLayout = {\n { \"data:ART/S.tga\", 0, 0 },\n { \"data:ART/S.tga\", 1, 3 },\n}";
        let a = Anim::parse(src.as_bytes());
        assert_eq!(a.frames.len(), 2);
        assert_eq!(a.frames[0].path, "art/s.tga");
        assert_eq!(a.frames[1].uv, [0.75, 0.5, 1.0, 1.0]);
    }
}
