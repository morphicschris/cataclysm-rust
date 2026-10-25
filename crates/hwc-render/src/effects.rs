//! The renderer's input format for particles, sprites, lines and other effect geometry.
//!
//! Effect systems (engine trails here, `hwc-fx` later) fill an [`EffectBatch`] each frame and
//! hand it to [`crate::Renderer::render_ex`]. Primitives are drawn after all ships, depth tested
//! against them but without writing depth, in submission order within each kind: triangles,
//! glow meshes, sprites, strips, lines, then screen-space flares (no depth test).
//!
//! Colours are linear 0..1 RGBA; with [`Blend::Additive`] the alpha scales the added colour, as
//! the engine's `GL_SRC_ALPHA, GL_ONE` blending does.

use glam::{Mat4, Vec2, Vec3};

/// A texture registered with [`crate::Renderer::create_effect_texture`]. [`EffectTexture::WHITE`]
/// is always available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct EffectTexture(pub u32);

impl EffectTexture {
    pub const WHITE: Self = Self(0);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blend {
    /// `src × alpha + dst`.
    #[default]
    Additive,
    /// `src × alpha + dst × (1 - alpha)`.
    Alpha,
}

/// Texture coordinates of a sub-rectangle: `[u0, v0, u1, v1]`.
pub type UvRect = [f32; 4];

pub const FULL_UV: UvRect = [0.0, 0.0, 1.0, 1.0];

/// A camera-facing square or rectangle centred on a world position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub position: Vec3,
    /// Half width and half height in world units.
    pub half_size: Vec2,
    /// Rotation in the screen plane, radians.
    pub rotation: f32,
    pub color: [f32; 4],
    pub uv: UvRect,
    pub texture: EffectTexture,
    pub blend: Blend,
}

/// A world-space line drawn `width` pixels wide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub from: Vec3,
    pub to: Vec3,
    pub from_color: [f32; 4],
    pub to_color: [f32; 4],
    pub width: f32,
    pub blend: Blend,
}

/// A coloured, optionally textured world-space triangle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectVertex {
    pub position: Vec3,
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Triangle {
    pub vertices: [EffectVertex; 3],
    pub texture: EffectTexture,
    pub blend: Blend,
}

/// A screen-space quad such as a lens-flare element: centre in normalised device coordinates
/// (x right, y up, -1..1), half size in pixels. Drawn without depth testing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlareQuad {
    pub centre: Vec2,
    pub half_size: Vec2,
    pub color: [f32; 4],
    pub uv: UvRect,
    pub texture: EffectTexture,
    pub blend: Blend,
}

/// A camera-facing ribbon through a list of points, e.g. an electric bolt. `width` is the full
/// world-space width; colours fade from `start_color` to `end_color` along the strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub points: Vec<Vec3>,
    pub width: f32,
    pub start_color: [f32; 4],
    pub end_color: [f32; 4],
    pub texture: EffectTexture,
    pub blend: Blend,
}

/// Which engine-glow mesh to draw (Homeworld's `misc\EngineGlows` set, four detail levels each).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GlowMeshKind {
    EnergyBolt,
    Glow,
    FrigateGlow,
    MothershipR1,
    MothershipR2,
    MothershipP1,
}

impl GlowMeshKind {
    /// Index into the engine's table of glow meshes for a ship detail level (levels 0 and 1
    /// share the most detailed mesh).
    pub fn mesh_index(self, lod: usize) -> usize {
        let detail = lod.saturating_sub(1).min(3);
        match self {
            Self::EnergyBolt => 0,
            Self::Glow => 1 + detail,
            Self::FrigateGlow => 5 + detail,
            Self::MothershipR1 => 9 + detail,
            Self::MothershipR2 => 13 + detail,
            Self::MothershipP1 => 17 + detail,
        }
    }
}

/// File names of the glow meshes, in table order.
pub const GLOW_MESH_NAMES: [&str; 21] = [
    "energybolt",
    "engine_glow00",
    "engine_glow01",
    "engine_glow02",
    "engine_glow03",
    "engine_glow_frigate00",
    "engine_glow_frigate01",
    "engine_glow_frigate02",
    "engine_glow_frigate03",
    "r1moship0",
    "r1moship1",
    "r1moship2",
    "r1moship3",
    "r2moship0",
    "r2moship1",
    "r2moship2",
    "r2moship3",
    "p1moship0",
    "p1moship1",
    "p1moship2",
    "p1moship3",
];

/// A glow mesh drawn additively with a view-dependent falloff (`colour × (N·V)^exponent`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowMeshDraw {
    /// Index into [`GLOW_MESH_NAMES`].
    pub mesh: usize,
    pub transform: Mat4,
    pub color: [f32; 4],
    pub exponent: f32,
}

/// Everything effect systems want drawn this frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectBatch {
    pub triangles: Vec<Triangle>,
    pub glow_meshes: Vec<GlowMeshDraw>,
    pub sprites: Vec<Sprite>,
    pub strips: Vec<Strip>,
    pub lines: Vec<Line>,
    pub flares: Vec<FlareQuad>,
}

impl EffectBatch {
    pub fn clear(&mut self) {
        self.triangles.clear();
        self.glow_meshes.clear();
        self.sprites.clear();
        self.strips.clear();
        self.lines.clear();
        self.flares.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.triangles.is_empty()
            && self.glow_meshes.is_empty()
            && self.sprites.is_empty()
            && self.strips.is_empty()
            && self.lines.is_empty()
            && self.flares.is_empty()
    }

    /// Adds an untextured quad `a b c d` (in order around its edge) as two triangles.
    pub fn quad(&mut self, corners: [(Vec3, [f32; 4]); 4], blend: Blend) {
        let v = |(position, color): (Vec3, [f32; 4])| EffectVertex {
            position,
            color,
            uv: [0.5, 0.5],
        };
        let [a, b, c, d] = corners.map(v);
        for vertices in [[a, b, c], [a, c, d]] {
            self.triangles.push(Triangle {
                vertices,
                texture: EffectTexture::WHITE,
                blend,
            });
        }
    }

    /// Appends another batch.
    pub fn extend(&mut self, other: &EffectBatch) {
        self.triangles.extend_from_slice(&other.triangles);
        self.glow_meshes.extend_from_slice(&other.glow_meshes);
        self.sprites.extend_from_slice(&other.sprites);
        self.strips.extend(other.strips.iter().cloned());
        self.lines.extend_from_slice(&other.lines);
        self.flares.extend_from_slice(&other.flares);
    }
}

/// Vertex of tessellated effect triangles (world or screen space).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct EffectGpuVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
}

/// Vertex of a screen-width line quad.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineGpuVertex {
    pub a: [f32; 3],
    pub b: [f32; 3],
    pub color: [f32; 4],
    /// End (0 = a, 1 = b), side (±1), width in pixels.
    pub shape: [f32; 3],
}

/// A run of vertices drawn with one texture and blend mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DrawRun {
    pub texture: EffectTexture,
    pub blend: Blend,
    pub first: u32,
    pub count: u32,
}

/// Camera information tessellation needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EffectCamera {
    pub eye: Vec3,
    /// Unit world vectors of the screen's right and up directions.
    pub right: Vec3,
    pub up: Vec3,
    /// Viewport size in pixels.
    pub viewport: Vec2,
}

/// An [`EffectBatch`] turned into vertex lists.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TessellatedEffects {
    pub world: Vec<EffectGpuVertex>,
    pub world_runs: Vec<DrawRun>,
    pub lines: Vec<LineGpuVertex>,
    pub line_runs: Vec<DrawRun>,
    pub screen: Vec<EffectGpuVertex>,
    pub screen_runs: Vec<DrawRun>,
}

fn push_run(runs: &mut Vec<DrawRun>, texture: EffectTexture, blend: Blend, first: u32, count: u32) {
    if count == 0 {
        return;
    }
    if let Some(last) = runs.last_mut() {
        if last.texture == texture && last.blend == blend && last.first + last.count == first {
            last.count += count;
            return;
        }
    }
    runs.push(DrawRun {
        texture,
        blend,
        first,
        count,
    });
}

fn uv_corner(uv: UvRect, sx: f32, sy: f32) -> [f32; 2] {
    // sx, sy in -1..1; v grows downwards in the bitmap.
    [
        uv[0] + (uv[2] - uv[0]) * (sx + 1.0) * 0.5,
        uv[1] + (uv[3] - uv[1]) * (1.0 - sy) * 0.5,
    ]
}

const QUAD_CORNERS: [(f32, f32); 6] = [
    (-1.0, -1.0),
    (1.0, -1.0),
    (1.0, 1.0),
    (-1.0, -1.0),
    (1.0, 1.0),
    (-1.0, 1.0),
];

impl EffectBatch {
    /// Builds vertex lists for drawing: triangles, then sprites, then strips (world space),
    /// lines, and screen-space flares.
    pub fn tessellate(&self, cam: &EffectCamera) -> TessellatedEffects {
        let mut out = TessellatedEffects::default();
        for t in &self.triangles {
            let first = out.world.len() as u32;
            for v in &t.vertices {
                out.world.push(EffectGpuVertex {
                    position: v.position.into(),
                    color: v.color,
                    uv: v.uv,
                });
            }
            push_run(&mut out.world_runs, t.texture, t.blend, first, 3);
        }
        for s in &self.sprites {
            let first = out.world.len() as u32;
            let (sin, cos) = s.rotation.sin_cos();
            let right = cam.right * cos + cam.up * sin;
            let up = cam.up * cos - cam.right * sin;
            for (sx, sy) in QUAD_CORNERS {
                let p = s.position + right * (sx * s.half_size.x) + up * (sy * s.half_size.y);
                out.world.push(EffectGpuVertex {
                    position: p.into(),
                    color: s.color,
                    uv: uv_corner(s.uv, sx, sy),
                });
            }
            push_run(&mut out.world_runs, s.texture, s.blend, first, 6);
        }
        for strip in &self.strips {
            let n = strip.points.len();
            if n < 2 {
                continue;
            }
            let first = out.world.len() as u32;
            let half = strip.width * 0.5;
            let side = |i: usize| {
                let p = strip.points[i];
                let dir = strip.points[(i + 1).min(n - 1)] - strip.points[i.saturating_sub(1)];
                let s = dir.cross(cam.eye - p).normalize_or_zero();
                if s == Vec3::ZERO {
                    cam.right * half
                } else {
                    s * half
                }
            };
            let color = |i: usize| {
                let t = i as f32 / (n - 1) as f32;
                std::array::from_fn::<f32, 4, _>(|c| {
                    strip.start_color[c] + (strip.end_color[c] - strip.start_color[c]) * t
                })
            };
            for i in 0..n - 1 {
                let (a, b) = (strip.points[i], strip.points[i + 1]);
                let (sa, sb) = (side(i), side(i + 1));
                let (ca, cb) = (color(i), color(i + 1));
                let ua = i as f32 / (n - 1) as f32;
                let ub = (i + 1) as f32 / (n - 1) as f32;
                let v = |p: Vec3, c: [f32; 4], u: f32, v: f32| EffectGpuVertex {
                    position: p.into(),
                    color: c,
                    uv: [u, v],
                };
                let quad = [
                    v(a - sa, ca, ua, 1.0),
                    v(b - sb, cb, ub, 1.0),
                    v(b + sb, cb, ub, 0.0),
                    v(a - sa, ca, ua, 1.0),
                    v(b + sb, cb, ub, 0.0),
                    v(a + sa, ca, ua, 0.0),
                ];
                out.world.extend_from_slice(&quad);
            }
            let count = out.world.len() as u32 - first;
            push_run(
                &mut out.world_runs,
                strip.texture,
                strip.blend,
                first,
                count,
            );
        }
        for l in &self.lines {
            let first = out.lines.len() as u32;
            for (end, side) in [
                (0.0, -1.0),
                (1.0, -1.0),
                (1.0, 1.0),
                (0.0, -1.0),
                (1.0, 1.0),
                (0.0, 1.0),
            ] {
                out.lines.push(LineGpuVertex {
                    a: l.from.into(),
                    b: l.to.into(),
                    color: if end > 0.5 { l.to_color } else { l.from_color },
                    shape: [end, side, l.width.max(1.0)],
                });
            }
            push_run(&mut out.line_runs, EffectTexture::WHITE, l.blend, first, 6);
        }
        let px = Vec2::new(2.0, 2.0) / cam.viewport.max(Vec2::ONE);
        for f in &self.flares {
            let first = out.screen.len() as u32;
            for (sx, sy) in QUAD_CORNERS {
                let p = f.centre + Vec2::new(sx, sy) * f.half_size * px;
                out.screen.push(EffectGpuVertex {
                    position: [p.x, p.y, 0.0],
                    color: f.color,
                    uv: uv_corner(f.uv, sx, sy),
                });
            }
            push_run(&mut out.screen_runs, f.texture, f.blend, first, 6);
        }
        out
    }
}

/// Converts 0..255 RGBA bytes to a colour.
pub fn rgba8(c: [u8; 4]) -> [f32; 4] {
    c.map(|v| f32::from(v) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glow_mesh_lod_mapping() {
        assert_eq!(
            GLOW_MESH_NAMES[GlowMeshKind::Glow.mesh_index(0)],
            "engine_glow00"
        );
        assert_eq!(
            GLOW_MESH_NAMES[GlowMeshKind::Glow.mesh_index(1)],
            "engine_glow00"
        );
        assert_eq!(
            GLOW_MESH_NAMES[GlowMeshKind::Glow.mesh_index(2)],
            "engine_glow01"
        );
        assert_eq!(
            GLOW_MESH_NAMES[GlowMeshKind::FrigateGlow.mesh_index(9)],
            "engine_glow_frigate03"
        );
        assert_eq!(
            GLOW_MESH_NAMES[GlowMeshKind::MothershipP1.mesh_index(3)],
            "p1moship2"
        );
    }

    #[test]
    fn quads_make_two_triangles() {
        let mut b = EffectBatch::default();
        let c = [1.0; 4];
        b.quad(
            [(Vec3::ZERO, c), (Vec3::X, c), (Vec3::ONE, c), (Vec3::Y, c)],
            Blend::Alpha,
        );
        assert_eq!(b.triangles.len(), 2);
        assert!(!b.is_empty());
        b.clear();
        assert!(b.is_empty());
    }
}

/// The ship statics the hyperspace window (`hsEnd`) is sized and coloured from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HyperspaceWindowShape {
    /// `staticCollInfo` forward, up and right lengths and the collision sphere offset.
    pub forward_length: f32,
    pub up_length: f32,
    pub right_length: f32,
    pub coll_sphere_offset: Vec3,
    /// `hyperspaceBoxScale` (statics +0x3d0).
    pub box_scale: f32,
    /// `hyperspaceColor` RGB; black means the default 70, 90, 255.
    pub color: [u8; 3],
}

/// `hsState` values the window is drawn for.
pub mod hs_state {
    pub const POPUP_INTO: u32 = 1;
    pub const POPUP_OUTOF: u32 = 2;
    pub const SLICING_INTO: u32 = 3;
    pub const SLICING_OUTOF: u32 = 4;
    pub const COLLAPSE_INTO: u32 = 5;
    pub const COLLAPSE_OUTOF: u32 = 6;
}

/// `HS_DIST / 2`: how far beyond the bounding box the window reaches.
const HS_DIST_2: f32 = 1.08 / 2.0;
/// The window's alpha (`hsRectangle(.., 90, ..)`).
const HS_ALPHA: u8 = 90;
/// `hsDefaultColor`.
const HS_DEFAULT_COLOR: [u8; 3] = [70, 90, 255];

/// `hsDesat`: the colour with its HSV saturation scaled.
fn desaturate(c: [u8; 3], scale: f32) -> [f32; 3] {
    let [r, g, b] = c.map(|v| f32::from(v) / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let s = if max > 0.0 { (max - min) / max } else { 0.0 };
    let h = if max == min {
        0.0
    } else if max == r {
        ((g - b) / (max - min)).rem_euclid(6.0)
    } else if max == g {
        (b - r) / (max - min) + 2.0
    } else {
        (r - g) / (max - min) + 4.0
    };
    let s = s * scale;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let (r1, g1, b1) = match h as i32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r1 + m, g1 + m, b1 + m].map(|v| (v * 255.0).round() / 255.0)
}

/// The hyperspace window `hsEnd` draws after a ship arriving or leaving through hyperspace,
/// on top of its `HyperWindow` effect: an additive rectangle at the clip plane (`hsRectangle`,
/// alpha 90, `HS_DIST` times the ship's box scaled by its `hyperspaceBoxScale`, the up extent
/// opening and closing with `clipt` while it pops up and collapses) with a two-pixel outline
/// 40 brighter in red and green, and while popping up or collapsing the 1.5-pixel line across
/// it (`hsLine`, the colour desaturated to 70 %, fading with `clipt`). `transform` is the ship's
/// model matrix (its position and coordinate system). Nothing for other states.
pub fn hyperspace_window(out: &mut EffectBatch, transform: Mat4, shape: &HyperspaceWindowShape, state: u32, clipt: f32) {
    use hs_state::*;
    let (z, open, line_alpha) = match state {
        POPUP_INTO | POPUP_OUTOF => (shape.forward_length, 1.0 - clipt, Some(clipt)),
        SLICING_INTO | SLICING_OUTOF => (clipt * shape.forward_length, 1.0, None),
        COLLAPSE_INTO | COLLAPSE_OUTOF => (-shape.forward_length, clipt, Some(1.0 - clipt)),
        _ => return,
    };
    let c = if shape.color == [0, 0, 0] { HS_DEFAULT_COLOR } else { shape.color };
    let f = |v: u8| f32::from(v) / 255.0;
    let a = f(HS_ALPHA);
    let fill = [f(c[0]), f(c[1]), f(c[2]), a];
    let outline = [f(c[0].wrapping_add(40)), f(c[1].wrapping_add(40)), f(c[2]), a];
    // The rectangle's x extent comes from the up length and its y extent from the right length.
    let half_x = HS_DIST_2 * shape.up_length * shape.box_scale * open;
    let half_y = HS_DIST_2 * shape.right_length * shape.box_scale;
    let o = Vec3::new(shape.coll_sphere_offset.x, shape.coll_sphere_offset.y, z);
    let corners = [
        Vec3::new(-half_x, -half_y, 0.0),
        Vec3::new(-half_x, half_y, 0.0),
        Vec3::new(half_x, half_y, 0.0),
        Vec3::new(half_x, -half_y, 0.0),
    ]
    .map(|p| transform.transform_point3(o + p));
    out.quad(corners.map(|p| (p, fill)), Blend::Additive);
    for i in 0..4 {
        out.lines.push(Line { from: corners[i], to: corners[(i + 1) % 4], from_color: outline, to_color: outline, width: 2.0, blend: Blend::Additive });
    }
    if let Some(t) = line_alpha {
        let d = desaturate(c, 0.7);
        // `(ubyte)(255 * t)`.
        let la = ((255.0 * t.clamp(0.0, 1.0)) as u8) as f32 / 255.0;
        let color = [d[0], d[1], d[2], la];
        let len = 0.8 * shape.right_length;
        let from = transform.transform_point3(o + Vec3::new(0.0, -len, 0.0));
        let to = transform.transform_point3(o + Vec3::new(0.0, len, 0.0));
        out.lines.push(Line { from, to, from_color: color, to_color: color, width: 1.5, blend: Blend::Additive });
    }
}

#[cfg(test)]
mod hs_tests {
    use super::*;

    fn shape() -> HyperspaceWindowShape {
        HyperspaceWindowShape { forward_length: 100.0, up_length: 40.0, right_length: 60.0, coll_sphere_offset: Vec3::ZERO, box_scale: 1.0, color: [0, 0, 0] }
    }

    #[test]
    fn window_is_the_default_blue_and_additive() {
        let mut b = EffectBatch::default();
        hyperspace_window(&mut b, Mat4::IDENTITY, &shape(), hs_state::SLICING_OUTOF, 0.5);
        assert_eq!(b.triangles.len(), 2);
        let t = &b.triangles[0];
        assert_eq!(t.blend, Blend::Additive);
        assert_eq!(t.vertices[0].color, [70.0 / 255.0, 90.0 / 255.0, 1.0, 90.0 / 255.0]);
        assert_eq!(t.vertices[0].position, Vec3::new(-0.54 * 40.0, -0.54 * 60.0, 50.0));
        assert_eq!(b.lines.len(), 4);
    }

    #[test]
    fn popup_opens_and_draws_the_cross_line() {
        let mut b = EffectBatch::default();
        hyperspace_window(&mut b, Mat4::IDENTITY, &shape(), hs_state::POPUP_OUTOF, 1.0);
        // Fully closed at clipt 1: zero width, the line at full alpha.
        assert_eq!(b.triangles[0].vertices[0].position.x, 0.0);
        assert_eq!(b.lines.len(), 5);
        assert_eq!(b.lines[4].from_color[3], 1.0);
        let mut b = EffectBatch::default();
        hyperspace_window(&mut b, Mat4::IDENTITY, &shape(), 0, 0.0);
        assert!(b.is_empty());
    }

    #[test]
    fn desaturation_keeps_value() {
        let d = desaturate([70, 90, 255], 0.7);
        assert!((d[2] - 1.0).abs() < 1e-6);
        assert!(d[0] > 70.0 / 255.0 && d[1] > 90.0 / 255.0);
    }
}
