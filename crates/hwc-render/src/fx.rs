//! Drawing `hwc-fx` particle effects the way the engine's `partRender*` functions did.
//!
//! [`EffectRenderer`] turns a [`hwc_fx::RenderList`] (camera independent) into GL-equivalent
//! geometry for one camera and records it as a render pass over a colour and depth target that
//! already hold the scene (normally right after [`crate::Renderer::render_ex`], in the same
//! command encoder). It reproduces the original state handling per primitive:
//!
//! * **Billboards** (`partRenderBillSystem`): quads in the eye-space plane through the particle
//!   (`rndBillboardEnable`), `±scaleX/2 × ±scaleY/2`, rotated by the particle's angle, texel
//!   rectangle divided by the texture's size on disk with `v0` on the bottom edge, bilinear
//!   filtering, `GL_MODULATE`. Blending as the original decides it: a texture with an alpha
//!   channel always blends, additively when `PART_ADDITIVE` is set; otherwise the quad is
//!   opaque at full alpha and alpha blended below it. Depth test on, depth writes off, no
//!   culling. Untextured particles are discs (`partCircleSolid3`) whose radius is the full
//!   `scaleX`, with the original's swapped sine/cosine (the fan starts at the top and runs
//!   clockwise) and colours rounded to bytes. The colour bias (`setAddColor`) is the GL path's
//!   second, additive pass with the "alternate" texture (RGB 200, original alpha).
//! * **Lines, bolts, points** (`partRenderLineSystem`, `partRenderEBoltSystem`,
//!   `partRenderPointSystem`): pixel-wide primitives with GL's non-antialiased rasterisation
//!   (width rounded, at least one pixel, wide lines widened along the minor axis, points
//!   square and at most 3 pixels), depth writes left on as in the original. Bolts never enable
//!   blending in the original and are drawn opaque unless [`FxOptions::blend_bolts`] is set.
//! * **Meshes** (`partRenderMeshSystem`): the first polygon object of the `.geo`, oriented by
//!   [`hwc_fx::render::MeshInstance::model_matrix`] (pseudo-billboards against this camera),
//!   morphing between two meshes by the frame fraction, textured with the particle's texture
//!   and lit per vertex (`partMeshMaterialPrepare`: ambient `icolor × ambient/diffuse` of the
//!   first material, diffuse `icolor`; `setIllum` overrides the ambient light from that
//!   particle on) or unlit in `icolor` (bias added the saturating way of the GL path).
//!   Per-particle depth writes (`setDepthWrite`), culling per material (two-sided ones are not
//!   culled and are lit from the side facing the camera).
//! * **Cubes**: `drawBox` boxes (no ETG function creates them).
//! * **Lens flares** (`lensFlareComputeVisibility`/`lensFlareRenderAll`): see
//!   [`EffectRenderer::prepare_lens_flares`].
//!
//! Systems are drawn in the order the effects drew them ([`hwc_fx::render::RenderList::spans`]).
//!
//! Per frame:
//! ```ignore
//! renderer.render_ex(&mut encoder, &color, &depth, size, &scene, &extras);
//! let view = FxView::from_orbit(&camera, size.0, size.1);
//! fx.prepare(&renderer, &source, &render_list, &view, &lighting);
//! fx.prepare_lens_flares(&mut world.lens_flares, &view, None);
//! fx.draw(&renderer, &mut encoder, &color, &depth);
//! ```

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec2, Vec3, Vec4};
use hwc_formats::geo::{self, Geo};
use hwc_fx::particle::LensFlare;
use hwc_fx::render::{self as fxr, PrimKind, RenderList};
use hwc_math::Vector;
use wgpu::util::DeviceExt;

use crate::assets::AssetSource;
use crate::camera::OrbitCamera;
use crate::effects::{self, EffectBatch, EffectTexture};
use crate::gpu::{Renderer, DEPTH_FORMAT};
use crate::lighting::SceneLighting;

// ---------------------------------------------------------------------------------------------
// Camera and options

/// The camera effects are drawn for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxView {
    /// World to eye.
    pub view: Mat4,
    /// Eye to clip (wgpu depth range).
    pub proj: Mat4,
    /// Eye position in world space.
    pub eye: Vec3,
    /// Viewport size in pixels.
    pub viewport: Vec2,
}

impl FxView {
    pub fn new(view: Mat4, proj: Mat4, width: u32, height: u32) -> Self {
        Self {
            view,
            proj,
            eye: view.inverse().w_axis.truncate(),
            viewport: Vec2::new(width.max(1) as f32, height.max(1) as f32),
        }
    }

    /// The view the renderer uses for an orbit camera and a viewport.
    pub fn from_orbit(camera: &OrbitCamera, width: u32, height: u32) -> Self {
        let aspect = width.max(1) as f32 / height.max(1) as f32;
        let mut v = Self::new(camera.view(), camera.projection(aspect), width, height);
        v.eye = camera.eye();
        v
    }

    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }

    /// World-space directions of the screen's right and up (the eye-space X and Y axes).
    pub fn screen_axes(&self) -> (Vec3, Vec3) {
        let inv = self.view.inverse();
        (
            inv.x_axis.truncate().normalize_or_zero(),
            inv.y_axis.truncate().normalize_or_zero(),
        )
    }
}

/// Choices beyond the original behaviour (all defaults reproduce the original GL path).
#[derive(Debug, Clone, PartialEq)]
pub struct FxOptions {
    /// Draw the second, additive colour-bias pass of billboards (`setAddColor`). On in the
    /// original whenever the bias is non-zero.
    pub sprite_bias_pass: bool,
    /// Blend electric bolts with the mode their particles ask for. The original never enables
    /// `GL_BLEND` for bolts, so they come out opaque (default `false`).
    pub blend_bolts: bool,
    /// Multiplier for line widths and point sizes, which the original gives in pixels. 1.0
    /// keeps the original pixel sizes at any resolution; a value such as `height / 480` keeps
    /// their size relative to the screen instead.
    pub pixel_scale: f32,
    /// Whether lines, bolts, points and cubes write depth (they do in the original).
    pub thin_primitives_write_depth: bool,
}

impl Default for FxOptions {
    fn default() -> Self {
        Self {
            sprite_bias_pass: true,
            blend_bolts: false,
            pixel_scale: 1.0,
            thin_primitives_write_depth: true,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// CPU-side frame data

/// Blend state of a draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FxBlend {
    /// `GL_BLEND` off.
    Opaque,
    /// `GL_SRC_ALPHA, GL_ONE_MINUS_SRC_ALPHA`.
    Alpha,
    /// `GL_SRC_ALPHA, GL_ONE` (`rndAdditiveBlends(TRUE)`).
    Additive,
}

/// The GL state a run of triangles is drawn with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FxState {
    /// Vertices are normalised device coordinates (no depth test).
    pub screen: bool,
    pub blend: FxBlend,
    pub depth_write: bool,
    /// Back faces culled (counter-clockwise front faces).
    pub cull: bool,
}

/// A vertex of the effect pass. World-space vertices are moved by `offset` pixels on screen.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct FxVertex {
    pub position: [f32; 3],
    pub color: [f32; 4],
    pub uv: [f32; 2],
    pub offset: [f32; 2],
}

/// A run of triangles with one state and texture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxDraw {
    pub state: FxState,
    /// Index into the renderer's texture table (0 is white).
    pub texture: u32,
    pub first: u32,
    pub count: u32,
}

/// Geometry prepared for one frame.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FxFrameData {
    pub vertices: Vec<FxVertex>,
    pub draws: Vec<FxDraw>,
}

impl FxFrameData {
    pub fn clear(&mut self) {
        self.vertices.clear();
        self.draws.clear();
    }

    /// Appends triangles (a multiple of three vertices).
    pub fn push(&mut self, state: FxState, texture: u32, vertices: &[FxVertex]) {
        if vertices.is_empty() {
            return;
        }
        let first = self.vertices.len() as u32;
        let count = vertices.len() as u32;
        self.vertices.extend_from_slice(vertices);
        if let Some(last) = self.draws.last_mut() {
            if last.state == state && last.texture == texture && last.first + last.count == first
            {
                last.count += count;
                return;
            }
        }
        self.draws.push(FxDraw {
            state,
            texture,
            first,
            count,
        });
    }
}

// ---------------------------------------------------------------------------------------------
// Assets

/// A registered effect texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FxTexture {
    /// Size on disk (texel rectangles are divided by it).
    pub width: u32,
    pub height: u32,
    /// The bitmap has an alpha channel (`TRF_Alpha`).
    pub alpha: bool,
    /// Index in the texture table.
    pub index: u32,
    /// The "alternate" used by the colour-bias pass: RGB 200, the original alpha.
    pub alternate: Option<u32>,
}

/// A material of an effect mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxMaterial {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub flags: u16,
}

/// A triangle of an effect mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FxPolygon {
    pub vertices: [usize; 3],
    pub face_normal: Vec3,
    pub uvs: [[f32; 2]; 3],
    pub material: usize,
}

/// The first polygon object of an ETG `.geo` mesh, as `partRenderMeshSystem` draws it (the
/// object's local matrix and any further objects are ignored, as in the original).
#[derive(Debug, Clone, PartialEq)]
pub struct FxMesh {
    pub positions: Vec<Vec3>,
    pub normals: Vec<Vec3>,
    pub polygons: Vec<FxPolygon>,
    pub materials: Vec<FxMaterial>,
}

impl FxMesh {
    pub fn from_geo(geo: &Geo) -> Option<Self> {
        let object = geo.objects.first()?;
        let normal = |i: u32| {
            object
                .normals
                .get(i as usize)
                .map_or(Vec3::Z, |n| Vec3::from_array(*n))
        };
        let rgb = |c: [u8; 4]| [c[0], c[1], c[2]].map(|v| f32::from(v) / 255.0);
        Some(Self {
            positions: object
                .vertices
                .iter()
                .map(|v| Vec3::from_array(v.position))
                .collect(),
            normals: object.vertices.iter().map(|v| normal(v.normal)).collect(),
            polygons: object
                .polygons
                .iter()
                .filter(|p| p.vertices.iter().all(|&v| (v as usize) < object.vertices.len()))
                .map(|p| FxPolygon {
                    vertices: p.vertices.map(usize::from),
                    face_normal: normal(p.face_normal),
                    uvs: p.uvs,
                    material: usize::from(p.material),
                })
                .collect(),
            materials: geo
                .materials
                .iter()
                .map(|m| FxMaterial {
                    ambient: rgb(m.ambient),
                    diffuse: rgb(m.diffuse),
                    flags: m.flags,
                })
                .collect(),
        })
    }
}

/// The archive path of an effect texture registered as `name` (`ETG\textures\glow32`,
/// possibly with doubled separators or a source extension): lower case, single backslashes,
/// `.lif`.
pub fn texture_file(name: &str) -> String {
    let mut path = normalize_path(name);
    let stem_start = path.rfind('\\').map_or(0, |i| i + 1);
    if let Some(dot) = path[stem_start..].rfind('.') {
        path.truncate(stem_start + dot);
    }
    path.push_str(".lif");
    path
}

/// The archive path of an effect mesh registered as `name`.
pub fn mesh_file(name: &str) -> String {
    let path = normalize_path(name);
    let stem_start = path.rfind('\\').map_or(0, |i| i + 1);
    if path[stem_start..].contains('.') {
        path
    } else {
        path + ".geo"
    }
}

fn normalize_path(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.trim().chars() {
        let c = if c == '/' { '\\' } else { c.to_ascii_lowercase() };
        if c == '\\' && (out.is_empty() || out.ends_with('\\')) {
            continue;
        }
        out.push(c);
    }
    out
}

/// Texture and mesh lookups the tessellator needs.
pub trait FxAssets {
    fn texture(&self, name: &str) -> Option<FxTexture>;
    fn mesh(&self, name: &str) -> Option<&FxMesh>;
}

#[derive(Default)]
struct AssetCache {
    textures: HashMap<String, Option<FxTexture>>,
    meshes: HashMap<String, Option<Arc<FxMesh>>>,
}

impl FxAssets for AssetCache {
    fn texture(&self, name: &str) -> Option<FxTexture> {
        self.textures.get(&texture_file(name)).copied().flatten()
    }

    fn mesh(&self, name: &str) -> Option<&FxMesh> {
        self.meshes.get(&mesh_file(name))?.as_deref()
    }
}

// ---------------------------------------------------------------------------------------------
// Tessellation (CPU, testable without a GPU)

fn v3(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn hmat(m: &hwc_math::HMatrix) -> Mat4 {
    Mat4::from_cols_array(&m.m)
}

/// `colRealToUbyte` as `partRenderBillSystem` inlines it: round to nearest, 255 when the
/// rounded value reaches 256.
fn real_to_ubyte(c: f32) -> f32 {
    let v = (c * 256.0).round_ties_even();
    if v >= 256.0 {
        1.0
    } else {
        v.clamp(0.0, 255.0) / 255.0
    }
}

/// `partSaturateAdd`: `base + add`, scaled so the largest channel is at most 1.
pub fn saturate_add(add: [f32; 3], base: [f32; 3]) -> [f32; 3] {
    let s = [base[0] + add[0], base[1] + add[1], base[2] + add[2]];
    let max = s[0].max(s[1]).max(s[2]);
    let k = if max == 0.0 { 1.0 } else { 1.0 / max };
    s.map(|c| c * k)
}

/// GL's non-antialiased width for a requested line width or point size.
fn raster_width(w: f32) -> f32 {
    let r = w.round();
    if r.is_nan() || r < 1.0 {
        1.0
    } else {
        r
    }
}

fn vertex(p: Vec3, color: [f32; 4], uv: [f32; 2]) -> FxVertex {
    FxVertex {
        position: p.into(),
        color,
        uv,
        offset: [0.0; 2],
    }
}

/// Corners of a billboard quad in eye-space units around its centre, in the original's
/// vertex order (bottom-left, bottom-right, top-right, top-left before rotation), with the
/// texel corner each takes (`0` = u0/v0, `1` = u1/v1).
pub fn billboard_corners(half_x: f32, half_y: f32, rotation: f32) -> [([f32; 2], [usize; 2]); 4] {
    let (sx, sy) = (half_x, half_y);
    let base = [
        ([-sx, -sy], [0, 0]),
        ([sx, -sy], [1, 0]),
        ([sx, sy], [1, 1]),
        ([-sx, sy], [0, 1]),
    ];
    if rotation == 0.0 {
        return base;
    }
    let (sin, cos) = (rotation.sin(), rotation.cos());
    base.map(|([x, y], t)| ([x * cos - y * sin, x * sin + y * cos], t))
}

/// Disc outline of `partCircleSolid3`: `slices` points from the top going clockwise (x from
/// the sine, y from the cosine), then the closing point at the top again.
pub fn disc_outline(radius: f32, slices: u32) -> Vec<[f32; 2]> {
    let mut out = Vec::with_capacity(slices as usize + 1);
    let mut theta = 0.0f64;
    let step = std::f64::consts::TAU / f64::from(slices.max(1));
    for _ in 0..slices {
        out.push([
            (theta.sin() as f32) * radius,
            (theta.cos() as f32) * radius,
        ]);
        theta += step;
    }
    out.push([0.0, radius]);
    out
}

struct Tess<'a> {
    view: &'a FxView,
    vp: Mat4,
    right: Vec3,
    up: Vec3,
    options: &'a FxOptions,
    lighting: &'a SceneLighting,
    assets: &'a dyn FxAssets,
    out: &'a mut FxFrameData,
}

impl Tess<'_> {
    fn world_state(&self, blend: FxBlend, depth_write: bool, cull: bool) -> FxState {
        FxState {
            screen: false,
            blend,
            depth_write,
            cull,
        }
    }

    fn quad(&self, centre: Vec3, corner: [f32; 2]) -> Vec3 {
        centre + self.right * corner[0] + self.up * corner[1]
    }

    fn sprite(&mut self, s: &fxr::Sprite) {
        let centre = v3(s.position);
        let state = |blend| FxState {
            screen: false,
            blend,
            depth_write: false,
            cull: false,
        };
        let opaque_or_alpha = if s.color[3] == 1.0 {
            FxBlend::Opaque
        } else {
            FxBlend::Alpha
        };
        let Some(name) = &s.texture else {
            // partCircleSolid3: a triangle fan of radius scaleX in the eye plane.
            let c = s.color.map(real_to_ubyte);
            let ring = disc_outline(s.size[0], s.slices);
            let mut verts = Vec::with_capacity(ring.len() * 3);
            for w in ring.windows(2) {
                verts.push(vertex(centre, c, [0.5; 2]));
                verts.push(vertex(self.quad(centre, w[0]), c, [0.5; 2]));
                verts.push(vertex(self.quad(centre, w[1]), c, [0.5; 2]));
            }
            self.out.push(state(opaque_or_alpha), 0, &verts);
            return;
        };
        let tex = self.assets.texture(name);
        let (index, w, h, alpha) = match tex {
            Some(t) => (t.index, t.width.max(1) as f32, t.height.max(1) as f32, t.alpha),
            None => (0, 1.0, 1.0, false),
        };
        let blend = if alpha {
            if s.additive {
                FxBlend::Additive
            } else {
                FxBlend::Alpha
            }
        } else {
            opaque_or_alpha
        };
        let uvx = [s.uv[0][0] as f32 / w, s.uv[1][0] as f32 / w];
        let uvy = [s.uv[0][1] as f32 / h, s.uv[1][1] as f32 / h];
        let (uvx, uvy) = if tex.is_some() {
            (uvx, uvy)
        } else {
            ([0.0, 1.0], [0.0, 1.0])
        };
        let corners = billboard_corners(s.size[0] * 0.5, s.size[1] * 0.5, s.rotation);
        let (right, up) = (self.right, self.up);
        let quad = |color: [f32; 4]| {
            let v: Vec<FxVertex> = corners
                .iter()
                .map(|(c, t)| {
                    let p = centre + right * c[0] + up * c[1];
                    vertex(p, color, [uvx[t[0]], uvy[t[1]]])
                })
                .collect();
            [v[0], v[1], v[2], v[0], v[2], v[3]]
        };
        let main = quad(s.color);
        self.out.push(state(blend), index, &main);
        // The GL path's colour bias: the quad again, additively, in the bias colour with the
        // alternate texture (only alpha textures get one; others stay bound).
        let bias_sum = s.bias[0] + s.bias[1] + s.bias[2];
        if self.options.sprite_bias_pass && tex.is_some() && bias_sum > 0.0 {
            let c = [
                s.bias[0].clamp(0.0, 1.0),
                s.bias[1].clamp(0.0, 1.0),
                s.bias[2].clamp(0.0, 1.0),
                s.color[3],
            ];
            let second_blend = if blend == FxBlend::Opaque {
                FxBlend::Opaque
            } else {
                FxBlend::Additive
            };
            let t = tex.and_then(|t| t.alternate).unwrap_or(index);
            let v = quad(c);
            self.out.push(state(second_blend), t, &v);
        }
    }

    /// Clips a segment to the near plane; returns the end points in world space.
    fn clip_segment(&self, a: Vec3, b: Vec3) -> Option<(Vec3, Vec3, Vec4, Vec4)> {
        let ca = self.vp * a.extend(1.0);
        let cb = self.vp * b.extend(1.0);
        match (ca.z < 0.0, cb.z < 0.0) {
            (true, true) => None,
            (false, false) => Some((a, b, ca, cb)),
            (a_out, _) => {
                let t = ca.z / (ca.z - cb.z);
                let p = a.lerp(b, t);
                let cp = self.vp * p.extend(1.0);
                if a_out {
                    Some((p, b, cp, cb))
                } else {
                    Some((a, p, ca, cp))
                }
            }
        }
    }

    /// A non-antialiased wide line as a quad widened along the minor screen axis.
    fn line_quad(&self, a: Vec3, b: Vec3, width: f32, color: [f32; 4], out: &mut Vec<FxVertex>) {
        let Some((a, b, ca, cb)) = self.clip_segment(a, b) else {
            return;
        };
        let half = self.view.viewport * 0.5;
        let sa = ca.truncate().truncate() / ca.w.max(1e-6) * half;
        let sb = cb.truncate().truncate() / cb.w.max(1e-6) * half;
        let d = sb - sa;
        let w = raster_width(width * self.options.pixel_scale) * 0.5;
        let off = if d.x.abs() >= d.y.abs() {
            [0.0, w]
        } else {
            [w, 0.0]
        };
        let neg = [-off[0], -off[1]];
        let v = |p: Vec3, o: [f32; 2]| FxVertex {
            position: p.into(),
            color,
            uv: [0.5; 2],
            offset: o,
        };
        out.extend_from_slice(&[
            v(a, neg),
            v(b, neg),
            v(b, off),
            v(a, neg),
            v(b, off),
            v(a, off),
        ]);
    }

    fn thin_state(&self, blend: FxBlend) -> FxState {
        self.world_state(blend, self.options.thin_primitives_write_depth, false)
    }

    fn line(&mut self, l: &fxr::Line) {
        let mut verts = Vec::with_capacity(6);
        self.line_quad(v3(l.start), v3(l.end), l.width, l.color, &mut verts);
        let state = self.thin_state(blend_of(l.blend));
        self.out.push(state, 0, &verts);
    }

    fn bolt(&mut self, b: &fxr::Bolt) {
        let mut verts = Vec::new();
        for w in b.points.windows(2) {
            self.line_quad(v3(w[0]), v3(w[1]), b.width, b.color, &mut verts);
        }
        let blend = if self.options.blend_bolts {
            blend_of(b.blend)
        } else {
            FxBlend::Opaque
        };
        let state = self.thin_state(blend);
        self.out.push(state, 0, &verts);
    }

    fn point(&mut self, p: &fxr::Point) {
        let c = self.vp * v3(p.position).extend(1.0);
        if c.z < 0.0 || c.w <= 0.0 {
            return;
        }
        let h = raster_width(p.size * self.options.pixel_scale) * 0.5;
        let pos = v3(p.position);
        let v = |x: f32, y: f32| FxVertex {
            position: pos.into(),
            color: p.color,
            uv: [0.5; 2],
            offset: [x, y],
        };
        let verts = [
            v(-h, -h),
            v(h, -h),
            v(h, h),
            v(-h, -h),
            v(h, h),
            v(-h, h),
        ];
        let state = self.thin_state(blend_of(p.blend));
        self.out.push(state, 0, &verts);
    }

    fn cube(&mut self, cube: &fxr::Cube) {
        let h = cube.size * 0.5;
        let c = v3(cube.position);
        let corner = |i: usize| {
            c + Vec3::new(
                if i >= 4 { h } else { -h },
                if matches!(i, 2 | 3 | 6 | 7) { h } else { -h },
                if matches!(i, 1 | 2 | 5 | 6) { h } else { -h },
            )
        };
        const FACES: [[usize; 4]; 6] = [
            [0, 1, 2, 3],
            [3, 2, 6, 7],
            [7, 6, 5, 4],
            [4, 5, 1, 0],
            [5, 6, 2, 1],
            [7, 4, 0, 3],
        ];
        let mut verts = Vec::with_capacity(36);
        for f in FACES {
            let q = f.map(|i| vertex(corner(i), cube.color, [0.5; 2]));
            verts.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
        }
        let state = self.thin_state(blend_of(cube.blend));
        self.out.push(state, 0, &verts);
    }

    /// Fixed-function lighting of one vertex (`GL_LIGHT_MODEL_AMBIENT` = `ambient`).
    fn light(&self, n: Vec3, ambient: Vec3, mat_ambient: Vec3, mat_diffuse: Vec3) -> Vec3 {
        let mut c = ambient * mat_ambient;
        for l in self.lighting.lights.iter().take(2) {
            let d = n.dot(l.direction.normalize_or_zero()).max(0.0);
            c += l.ambient * mat_ambient + l.diffuse * mat_diffuse * d;
        }
        c.clamp(Vec3::ZERO, Vec3::ONE)
    }

    fn mesh(&mut self, m: &fxr::MeshInstance, system_lit: bool, ambient: Vec3) {
        let Some(name) = &m.mesh else { return };
        let Some(mesh) = self.assets.mesh(name) else {
            return;
        };
        let (next, frac) = match &m.morph {
            Some((next, frac)) => (
                next.as_deref().and_then(|n| self.assets.mesh(n)),
                frac.clamp(0.0, 1.0),
            ),
            None => (None, 0.0),
        };
        let next = next.filter(|n| n.positions.len() == mesh.positions.len());
        let topology = m
            .morph_base
            .as_deref()
            .filter(|_| m.morph.is_some())
            .and_then(|n| self.assets.mesh(n))
            .filter(|b| b.positions.len() == mesh.positions.len())
            .unwrap_or(mesh);
        let model = hmat(&m.model_matrix(Vector::new(
            self.view.eye.x,
            self.view.eye.y,
            self.view.eye.z,
        )));
        let normal_matrix = Mat3::from_mat4(model).inverse().transpose();
        let position = |i: usize| {
            let p = mesh.positions[i];
            let p = next.map_or(p, |n| p.lerp(n.positions[i], frac));
            model.transform_point3(p)
        };
        let vertex_normal = |i: usize| {
            let n = mesh.normals[i];
            next.map_or(n, |nm| n.lerp(nm.normals[i], frac))
        };
        let tex = m.texture.as_deref().and_then(|t| self.assets.texture(t));
        let textured = m.texture.is_some();
        let blend = if textured {
            if tex.is_some_and(|t| t.alpha) || m.alpha_system {
                if m.additive {
                    FxBlend::Additive
                } else {
                    FxBlend::Alpha
                }
            } else {
                FxBlend::Opaque
            }
        } else if m.alpha_system {
            FxBlend::Alpha
        } else {
            FxBlend::Opaque
        };
        let alpha = if m.alpha_system { m.color[3] } else { 1.0 };
        let mut base = [m.color[0], m.color[1], m.color[2]];
        if m.bias[0] + m.bias[1] + m.bias[2] > 0.0 {
            base = saturate_add(m.bias, base);
        }
        let lit = if textured { m.lit } else { system_lit };
        let icolor = Vec3::new(m.color[0], m.color[1], m.color[2]);
        let first_material = topology.materials.first().copied();
        let ambient_factor = first_material.map_or(0.0, |mat| {
            if mat.diffuse[0] == 0.0 {
                0.0
            } else {
                mat.ambient[0] / mat.diffuse[0]
            }
        });
        let texture_index = tex.map_or(0, |t| t.index);
        for (cull, two_sided) in [(true, false), (false, true)] {
            let mut verts = Vec::new();
            for poly in &topology.polygons {
                let material = topology.materials.get(poly.material).copied();
                let flags = material.map_or(0, |mat| mat.flags);
                if (flags & geo::MATERIAL_TWO_SIDED != 0) != two_sided {
                    continue;
                }
                let smooth = flags & geo::MATERIAL_SMOOTHING != 0;
                let p = poly.vertices.map(position);
                let mut face = (normal_matrix * poly.face_normal).normalize_or_zero();
                if two_sided && face.dot(self.view.eye - p[0]) < 0.0 {
                    face = -face;
                }
                for (k, &vi) in poly.vertices.iter().enumerate() {
                    let rgb = if lit {
                        let mut n = if smooth {
                            (normal_matrix * vertex_normal(vi)).normalize_or_zero()
                        } else {
                            face
                        };
                        if two_sided && smooth && n.dot(face) < 0.0 {
                            n = -n;
                        }
                        let (ma, md) = if textured {
                            (icolor * ambient_factor, icolor)
                        } else {
                            material.map_or((Vec3::ZERO, Vec3::ONE), |mat| {
                                (
                                    Vec3::from_array(mat.ambient),
                                    Vec3::from_array(mat.diffuse),
                                )
                            })
                        };
                        self.light(n, ambient, ma, md).to_array()
                    } else {
                        base
                    };
                    let a = if lit && textured { m.color[3] } else { alpha };
                    let a = if lit && !textured { 1.0 } else { a };
                    let uv = if textured { poly.uvs[k] } else { [0.5; 2] };
                    verts.push(vertex(p[k], [rgb[0], rgb[1], rgb[2], a], uv));
                }
            }
            let state = self.world_state(blend, m.depth_write, cull);
            self.out.push(state, texture_index, &verts);
        }
    }
}

fn blend_of(b: fxr::Blend) -> FxBlend {
    match b {
        fxr::Blend::Opaque => FxBlend::Opaque,
        fxr::Blend::Alpha => FxBlend::Alpha,
        fxr::Blend::Additive => FxBlend::Additive,
    }
}

/// Builds the GL-equivalent geometry of a render list for one camera.
pub fn tessellate(
    list: &RenderList,
    view: &FxView,
    lighting: &SceneLighting,
    options: &FxOptions,
    assets: &dyn FxAssets,
    out: &mut FxFrameData,
) {
    let (right, up) = view.screen_axes();
    let mut t = Tess {
        view,
        vp: view.view_proj(),
        right,
        up,
        options,
        lighting,
        assets,
        out,
    };
    for span in list.draw_order() {
        let range = span.start..span.start + span.count;
        match span.kind {
            PrimKind::Sprites => list.sprites.get(range).into_iter().flatten().for_each(|s| t.sprite(s)),
            PrimKind::Lines => list.lines.get(range).into_iter().flatten().for_each(|l| t.line(l)),
            PrimKind::Bolts => list.bolts.get(range).into_iter().flatten().for_each(|b| t.bolt(b)),
            PrimKind::Points => list.points.get(range).into_iter().flatten().for_each(|p| t.point(p)),
            PrimKind::Cubes => list.cubes.get(range).into_iter().flatten().for_each(|c| t.cube(c)),
            PrimKind::Meshes => {
                // handleIllum: a particle's illum replaces the ambient light for the rest of
                // the system.
                let mut ambient = lighting.global_ambient;
                for m in list.meshes.get(range).into_iter().flatten() {
                    if m.illum != 0.0 {
                        ambient = Vec3::splat(m.illum);
                    }
                    t.mesh(m, span.lit, ambient);
                }
            }
            // Flare particles only mark their flare as drawn; see `prepare_lens_flares`.
            PrimKind::Flares => {}
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lens flares

/// One element of a lens flare (`Element radius offset image r g b`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FlareElement {
    /// Half size as a fraction of the screen height.
    pub radius: f32,
    /// Position along the line from the screen centre (0) through the light (1).
    pub offset: f32,
    /// Index into [`LensFlareSet::rects`], or a prefab: -1 hexagon, -2 circle, -3 circle with
    /// a bright centre, -4 circle with a dark centre.
    pub image: i32,
    pub color: [u8; 3],
}

/// A `LensFlares\<name>.script`.
#[derive(Debug, Clone, PartialEq)]
pub struct LensFlareSet {
    pub elements: Vec<FlareElement>,
    /// Texel rectangles `[x, y, w, h]` already divided by 256.
    pub rects: Vec<[f32; 4]>,
    /// `TextureFile` (relative to `LensFlares\`, extension as written).
    pub texture_file: Option<String>,
    /// `FadeoutFrameSize`: how far outside the screen (in NDC units) flares fade out.
    pub fadeout_frame_size: f32,
}

impl LensFlareSet {
    /// The engine's value before any script sets `FadeoutFrameSize`.
    pub const DEFAULT_FADEOUT_FRAME_SIZE: f32 = 1.2;

    pub fn parse(text: &str) -> Self {
        let mut set = Self {
            elements: Vec::new(),
            rects: Vec::new(),
            texture_file: None,
            fadeout_frame_size: Self::DEFAULT_FADEOUT_FRAME_SIZE,
        };
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            let mut words = line.split_whitespace();
            let Some(key) = words.next() else { continue };
            let rest: Vec<&str> = words.collect();
            let f = |i: usize| rest.get(i).and_then(|w| w.parse::<f32>().ok());
            let n = |i: usize| rest.get(i).and_then(|w| w.parse::<i32>().ok());
            match key.to_ascii_lowercase().as_str() {
                "element" => {
                    if let (Some(radius), Some(offset), Some(image), Some(r), Some(g), Some(b)) =
                        (f(0), f(1), n(2), n(3), n(4), n(5))
                    {
                        set.elements.push(FlareElement {
                            radius,
                            offset,
                            image,
                            color: [r, g, b].map(|c| c as u8),
                        });
                    }
                }
                "texture" => {
                    if let (Some(x), Some(y), Some(w), Some(h)) = (n(0), n(1), n(2), n(3)) {
                        set.rects
                            .push([x, y, w, h].map(|v| v as f32 * (1.0 / 256.0)));
                    }
                }
                "texturefile" => set.texture_file = rest.first().map(|s| s.to_string()),
                "fadeoutframesize" => {
                    if let Some(v) = f(0) {
                        set.fadeout_frame_size = v;
                    }
                }
                _ => {}
            }
        }
        set
    }

    /// Loads `lensflares\<name>.script` (`.script` optional in `name`).
    pub fn load(src: &dyn AssetSource, name: &str) -> Option<Self> {
        let mut path = format!("lensflares\\{}", normalize_path(name));
        if !path.ends_with(".script") {
            path.push_str(".script");
        }
        let data = src.read(&path)?;
        Some(Self::parse(&String::from_utf8_lossy(&data)))
    }

    /// Archive path of the flare texture.
    pub fn texture_path(&self) -> Option<String> {
        self.texture_file
            .as_deref()
            .map(|f| texture_file(&format!("lensflares\\{f}")))
    }
}

/// Per-flare state the original keeps in the flare object.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FlareState {
    pub visible: bool,
    /// Fade in/out (+0x18).
    pub fade: f32,
    /// Fade towards the screen edge (+0x1c).
    pub edge: f32,
    /// Distance to the camera (+0x20).
    pub distance: f32,
    /// Screen position in pixels, y down (+0x24, +0x28).
    pub screen: Vec2,
}

/// The original draws at most this many flares.
pub const MAX_LENS_FLARES: usize = 8;

/// `lensFlareComputeVisibility` for every flare, then `lensFlareRenderAll`'s geometry.
/// `occluded(position, direction to the eye, distance)` stands in for `collCheckRectLine`
/// against ships and derelicts; it is only asked for flares added since the last frame (state
/// 2), as in the original. Returns the slots drawn (flare indices).
#[allow(clippy::too_many_arguments)]
pub fn lens_flare_pass(
    flares: &mut [LensFlare],
    states: &mut Vec<FlareState>,
    set: &LensFlareSet,
    texture: Option<FxTexture>,
    view: &FxView,
    occluded: Option<&dyn Fn(Vec3, Vec3, f32) -> bool>,
    out: &mut FxFrameData,
) -> Vec<usize> {
    if states.len() < flares.len() {
        states.resize(flares.len(), FlareState::default());
    }
    let (w, h) = (view.viewport.x as i32, view.viewport.y as i32);
    let vp = view.view_proj();
    let big = set.fadeout_frame_size;
    let mut slots: Vec<usize> = Vec::with_capacity(MAX_LENS_FLARES);
    for (i, f) in flares.iter().enumerate() {
        if f.enabled == 0 {
            states[i].visible = false;
            continue;
        }
        states[i].visible = true;
        let pos = v3(f.position);
        let to_eye = view.eye - pos;
        let dist = to_eye.length();
        let slot = if slots.len() < MAX_LENS_FLARES {
            slots.push(i);
            slots.len() - 1
        } else {
            let mut best = 0usize;
            let mut chosen = None;
            for (k, &s) in slots.iter().enumerate().skip(1) {
                if !states[s].visible {
                    chosen = Some(k);
                    break;
                }
                if states[slots[best]].distance < states[s].distance && dist < states[s].distance
                {
                    best = k;
                }
            }
            chosen.unwrap_or(best)
        };
        let st = &mut states[i];
        let blocked = match f.pending {
            2 => occluded.is_some_and(|o| o(pos, to_eye.normalize_or_zero(), dist)),
            1 => true,
            _ => false,
        };
        if blocked {
            if st.fade > 0.0 {
                st.fade *= 0.6;
                if st.fade < 1e-9 {
                    st.visible = false;
                }
            }
        } else if st.fade < 1.0 {
            st.fade += (1.0 - st.fade) * 0.4;
            if st.fade > 1.0 {
                st.fade = 1.0;
            }
        }
        if st.visible {
            let c = vp * pos.extend(1.0);
            st.visible = false;
            if c.w > 0.0 {
                let (x, y) = (c.x / c.w, c.y / c.w);
                st.edge = if x.abs() > 1.0 || y.abs() > 1.0 {
                    let e = (big - x.abs()).min(big - y.abs());
                    if e <= 0.0 {
                        0.0
                    } else {
                        e / (big - 1.0)
                    }
                } else {
                    1.0
                };
                if st.edge > 0.0 {
                    st.screen = Vec2::new(
                        (w / 2) as f32 + (w as f32 * x * 0.5).round_ties_even(),
                        (h / 2) as f32 - (h as f32 * y * 0.5).round_ties_even(),
                    );
                    st.distance = dist;
                    st.visible = true;
                }
            }
        }
        slots[slot] = i;
    }
    for &i in &slots {
        let st = states[i];
        if !st.visible {
            continue;
        }
        let k = st.edge * st.fade * flares[i].intensity;
        if k != 0.0 {
            flare_geometry(set, texture, view, st.screen, k, out);
        }
        if flares[i].active != 0 {
            flares[i].pending = 1;
        }
    }
    slots
}

fn flare_geometry(
    set: &LensFlareSet,
    texture: Option<FxTexture>,
    view: &FxView,
    light: Vec2,
    k: f32,
    out: &mut FxFrameData,
) {
    let (w, h) = (view.viewport.x as i32, view.viewport.y as i32);
    let (cx, cy) = ((w / 2) as f32, (h / 2) as f32);
    let (wf, hf) = (w as f32, h as f32);
    let d = light - Vec2::new(cx, cy);
    let ndc = |x: f32, y: f32| [x / wf * 2.0 - 1.0, 1.0 - y / hf * 2.0, 0.0];
    let screen_vertex = |x: f32, y: f32, color: [f32; 4], uv: [f32; 2]| FxVertex {
        position: ndc(x, y),
        color,
        uv,
        offset: [0.0; 2],
    };
    for e in &set.elements {
        let size = hf * e.radius;
        let ex = d.x * e.offset + cx;
        let ey = d.y * e.offset + cy;
        let channel = |c: u8| (f32::from(c) * k).clamp(0.0, 255.0);
        let rgb = e.color.map(channel);
        let col = |scale: f32| [rgb[0] * scale / 256.0, rgb[1] * scale / 256.0, rgb[2] * scale / 256.0, 1.0];
        const B: f32 = 0.325;
        if e.image >= 0 {
            let Some(rect) = set.rects.get(e.image as usize) else {
                continue;
            };
            let Some(t) = texture else { continue };
            let c = col(1.0);
            let (u0, v0, u1, v1) = (rect[0], rect[1], rect[0] + rect[2], rect[1] + rect[3]);
            let (x0, y0, x1, y1) = (ex - size + B, ey - size + B, ex + size + B, ey + size + B);
            let q = [
                screen_vertex(x0, y0, c, [u0, v0]),
                screen_vertex(x1, y0, c, [u1, v0]),
                screen_vertex(x1, y1, c, [u1, v1]),
                screen_vertex(x0, y1, c, [u0, v1]),
            ];
            let blend = if t.alpha {
                FxBlend::Additive
            } else {
                FxBlend::Opaque
            };
            let state = FxState {
                screen: true,
                blend,
                depth_write: false,
                cull: false,
            };
            out.push(state, t.index, &[q[0], q[1], q[2], q[0], q[2], q[3]]);
        } else {
            let slices = |r: f32| ((r * 40.0 * 4.0) as i32).clamp(6, 32);
            let (n, centre, rim) = match e.image {
                -1 => (6, col(1.0), col(1.0)),
                -2 => (slices(e.radius), col(1.0), col(1.0)),
                -3 => (slices(e.radius), col(1.0), col(0.25)),
                _ => (slices(e.radius), col(0.25), col(1.0)),
            };
            let mut ring = Vec::with_capacity(n as usize + 1);
            let step = std::f64::consts::TAU / f64::from(n);
            for s in 0..n {
                let a = step * f64::from(s);
                ring.push((
                    a.cos() as f32 * size + ex + B,
                    a.sin() as f32 * size + ey + B,
                ));
            }
            ring.push((ex + size + B, ey + B));
            let mut verts = Vec::with_capacity(ring.len() * 3);
            for pair in ring.windows(2) {
                verts.push(screen_vertex(ex + B, ey + B, centre, [0.5; 2]));
                verts.push(screen_vertex(pair[0].0, pair[0].1, rim, [0.5; 2]));
                verts.push(screen_vertex(pair[1].0, pair[1].1, rim, [0.5; 2]));
            }
            let state = FxState {
                screen: true,
                blend: FxBlend::Additive,
                depth_write: false,
                cull: false,
            };
            out.push(state, 0, &verts);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// GPU

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct FxFrameUniform {
    view_proj: [[f32; 4]; 4],
    viewport: [f32; 4],
}

/// Draws `hwc-fx` render lists (see the module documentation).
pub struct EffectRenderer {
    pub options: FxOptions,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    texture_layout: wgpu::BindGroupLayout,
    color_format: wgpu::TextureFormat,
    pipelines: HashMap<FxState, wgpu::RenderPipeline>,
    uniform: wgpu::Buffer,
    frame_group: wgpu::BindGroup,
    sampler: wgpu::Sampler,
    vertex_buffer: Option<(wgpu::Buffer, u64)>,
    texture_groups: Vec<wgpu::BindGroup>,
    assets: AssetCache,
    flare_set: Option<LensFlareSet>,
    flare_texture: Option<FxTexture>,
    flare_states: Vec<FlareState>,
    frame: FxFrameData,
    view: Option<FxView>,
    batch_textures: HashMap<String, EffectTexture>,
}

impl EffectRenderer {
    pub fn new(renderer: &Renderer) -> Self {
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hwc fx shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("fx.wgsl").into()),
        });
        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fx texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fx"),
            bind_group_layouts: &[Some(&frame_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fx frame"),
            size: std::mem::size_of::<FxFrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fx frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        // partFilter(TRUE): bilinear, no mipmaps; textures repeat.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("fx"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mut fx = Self {
            options: FxOptions::default(),
            shader,
            pipeline_layout,
            texture_layout,
            color_format: renderer.color_format(),
            pipelines: HashMap::new(),
            uniform,
            frame_group,
            sampler,
            vertex_buffer: None,
            texture_groups: Vec::new(),
            assets: AssetCache::default(),
            flare_set: None,
            flare_texture: None,
            flare_states: Vec::new(),
            frame: FxFrameData::default(),
            view: None,
            batch_textures: HashMap::new(),
        };
        fx.add_texture(renderer, 1, 1, &[255; 4]);
        fx
    }

    fn add_texture(&mut self, renderer: &Renderer, width: u32, height: u32, rgba: &[u8]) -> u32 {
        let texture = renderer.device().create_texture_with_data(
            renderer.queue(),
            &wgpu::TextureDescriptor {
                label: Some("fx texture"),
                size: wgpu::Extent3d {
                    width: width.max(1),
                    height: height.max(1),
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            rgba,
        );
        let view = texture.create_view(&Default::default());
        let group = renderer
            .device()
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fx texture"),
                layout: &self.texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.sampler),
                    },
                ],
            });
        self.texture_groups.push(group);
        self.texture_groups.len() as u32 - 1
    }

    /// Loads (once) the texture registered as `name`; `None` if it is missing.
    pub fn load_texture(
        &mut self,
        renderer: &Renderer,
        src: &dyn AssetSource,
        name: &str,
    ) -> Option<FxTexture> {
        let path = texture_file(name);
        if let Some(t) = self.assets.textures.get(&path) {
            return *t;
        }
        let loaded = crate::assets::load_lif(src, &path).ok().map(|lif| {
            let alpha = lif.has_alpha();
            let mut rgba = lif.to_rgba();
            if !alpha {
                rgba.chunks_mut(4).for_each(|p| p[3] = 255);
            }
            let index = self.add_texture(renderer, lif.width, lif.height, &rgba);
            let alternate = alpha.then(|| {
                let alt: Vec<u8> = rgba
                    .chunks(4)
                    .flat_map(|p| [200, 200, 200, p[3]])
                    .collect();
                self.add_texture(renderer, lif.width, lif.height, &alt)
            });
            FxTexture {
                width: lif.width,
                height: lif.height,
                alpha,
                index,
                alternate,
            }
        });
        self.assets.textures.insert(path, loaded);
        loaded
    }

    /// Loads (once) the mesh registered as `name`.
    pub fn load_mesh(&mut self, src: &dyn AssetSource, name: &str) -> Option<&FxMesh> {
        let path = mesh_file(name);
        self.assets
            .meshes
            .entry(path.clone())
            .or_insert_with(|| {
                let data = src.read(&path)?;
                let geo = Geo::parse(&data).ok()?;
                FxMesh::from_geo(&geo).map(Arc::new)
            })
            .as_deref()
    }

    /// Loads `LensFlares\<name>.script` and its texture (the level's `LensFlare` line;
    /// multiplayer levels use `ps-50-300mm-zoom`). Returns whether the script was found.
    pub fn load_lens_flares(
        &mut self,
        renderer: &Renderer,
        src: &dyn AssetSource,
        name: &str,
    ) -> bool {
        let Some(set) = LensFlareSet::load(src, name) else {
            return false;
        };
        self.flare_texture = set
            .texture_path()
            .and_then(|p| self.load_texture(renderer, src, &p));
        self.flare_set = Some(set);
        true
    }

    /// The loaded lens-flare script.
    pub fn lens_flares(&self) -> Option<&LensFlareSet> {
        self.flare_set.as_ref()
    }

    /// Lens-flare state by flare index (for inspection).
    pub fn flare_states(&self) -> &[FlareState] {
        &self.flare_states
    }

    fn load_list_assets(&mut self, renderer: &Renderer, src: &dyn AssetSource, list: &RenderList) {
        for s in &list.sprites {
            if let Some(t) = &s.texture {
                self.load_texture(renderer, src, t);
            }
        }
        for m in &list.meshes {
            if let Some(t) = &m.texture {
                self.load_texture(renderer, src, t);
            }
            let names = [
                m.mesh.as_deref(),
                m.morph.as_ref().and_then(|(n, _)| n.as_deref()),
                m.morph_base.as_deref(),
            ];
            for n in names.into_iter().flatten() {
                self.load_mesh(src, n);
            }
        }
    }

    /// Starts a frame: loads what `list` needs and builds its geometry for `view`.
    pub fn prepare(
        &mut self,
        renderer: &Renderer,
        src: &dyn AssetSource,
        list: &RenderList,
        view: &FxView,
        lighting: &SceneLighting,
    ) {
        self.frame.clear();
        self.view = Some(*view);
        self.load_list_assets(renderer, src, list);
        tessellate(
            list,
            view,
            lighting,
            &self.options,
            &self.assets,
            &mut self.frame,
        );
    }

    /// Adds this frame's lens flares (after [`prepare`](Self::prepare); the original draws them
    /// after every object). Updates the flares' state the way the original does (the draw
    /// marks them as not drawn until their particle system draws again). Does nothing until
    /// [`load_lens_flares`](Self::load_lens_flares) succeeded.
    pub fn prepare_lens_flares(
        &mut self,
        flares: &mut [LensFlare],
        view: &FxView,
        occluded: Option<&dyn Fn(Vec3, Vec3, f32) -> bool>,
    ) {
        let Some(set) = &self.flare_set else { return };
        lens_flare_pass(
            flares,
            &mut self.flare_states,
            set,
            self.flare_texture,
            view,
            occluded,
            &mut self.frame,
        );
    }

    /// The geometry prepared for this frame.
    pub fn frame(&self) -> &FxFrameData {
        &self.frame
    }

    fn pipeline(&mut self, device: &wgpu::Device, state: FxState) {
        if self.pipelines.contains_key(&state) {
            return;
        }
        let blend = match state.blend {
            FxBlend::Opaque => None,
            FxBlend::Alpha => Some(wgpu::BlendState::ALPHA_BLENDING),
            FxBlend::Additive => Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
        };
        let layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<FxVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x4, 2 => Float32x2, 3 => Float32x2
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fx"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some(if state.screen {
                    "vs_fx_screen"
                } else {
                    "vs_fx_world"
                }),
                compilation_options: Default::default(),
                buffers: &[Some(layout)],
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: state.cull.then_some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(state.depth_write && !state.screen),
                depth_compare: Some(if state.screen {
                    wgpu::CompareFunction::Always
                } else {
                    wgpu::CompareFunction::Less
                }),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_fx"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: self.color_format,
                    blend,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.pipelines.insert(state, pipeline);
    }

    /// Records the prepared frame as a render pass that keeps `color` and `depth` (which must
    /// be the scene's targets, [`DEPTH_FORMAT`] depth).
    pub fn draw(
        &mut self,
        renderer: &Renderer,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        let Some(view) = self.view else { return };
        if self.frame.draws.is_empty() {
            return;
        }
        let device = renderer.device().clone();
        let states: Vec<FxState> = self.frame.draws.iter().map(|d| d.state).collect();
        for s in states {
            self.pipeline(&device, s);
        }
        let uniform = FxFrameUniform {
            view_proj: view.view_proj().to_cols_array_2d(),
            viewport: [view.viewport.x, view.viewport.y, 0.0, 0.0],
        };
        renderer
            .queue()
            .write_buffer(&self.uniform, 0, bytemuck::bytes_of(&uniform));
        let bytes: &[u8] = bytemuck::cast_slice(&self.frame.vertices);
        let size = bytes.len() as u64;
        if self.vertex_buffer.as_ref().is_none_or(|(_, cap)| *cap < size) {
            let cap = size.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fx vertices"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vertex_buffer = Some((buffer, cap));
        }
        let Some((buffer, _)) = &self.vertex_buffer else {
            return;
        };
        renderer.queue().write_buffer(buffer, 0, bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("effects"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.frame_group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..));
        for d in &self.frame.draws {
            let Some(p) = self.pipelines.get(&d.state) else {
                continue;
            };
            pass.set_pipeline(p);
            let tex = self
                .texture_groups
                .get(d.texture as usize)
                .unwrap_or(&self.texture_groups[0]);
            pass.set_bind_group(1, tex, &[]);
            pass.draw(d.first..d.first + d.count, 0..1);
        }
    }

    /// [`prepare`](Self::prepare) followed by [`draw`](Self::draw).
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        renderer: &Renderer,
        src: &dyn AssetSource,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        list: &RenderList,
        view: &FxView,
        lighting: &SceneLighting,
    ) {
        self.prepare(renderer, src, list, view, lighting);
        self.draw(renderer, encoder, color, depth);
    }

    /// Approximate alternative for callers that only have an [`EffectBatch`]: sprites, discs,
    /// lines, bolts and points become batch primitives (textures registered with the
    /// [`Renderer`]). The batch has no opaque or depth-writing modes, no draw order across
    /// kinds and no meshes or flares, so prefer [`prepare`](Self::prepare)/[`draw`](Self::draw).
    pub fn fill_batch(
        &mut self,
        renderer: &mut Renderer,
        src: &dyn AssetSource,
        list: &RenderList,
        out: &mut EffectBatch,
    ) {
        let blend = |b: fxr::Blend| match b {
            fxr::Blend::Additive => effects::Blend::Additive,
            _ => effects::Blend::Alpha,
        };
        let c3 = |v: Vector| Vec3::new(v.x, v.y, v.z);
        for s in &list.sprites {
            let Some(name) = &s.texture else {
                // Discs: a fan of world-space triangles cannot face the camera without it, so
                // use an untextured sprite of the disc's size.
                out.sprites.push(effects::Sprite {
                    position: c3(s.position),
                    half_size: Vec2::splat(s.size[0]),
                    rotation: 0.0,
                    color: s.color,
                    uv: effects::FULL_UV,
                    texture: EffectTexture::WHITE,
                    blend: blend(s.blend),
                });
                continue;
            };
            let path = texture_file(name);
            let (texture, w, h, alpha) = match self.batch_textures.get(&path) {
                Some(t) => {
                    let info = self.assets.textures.get(&path).copied().flatten();
                    (*t, info.map_or(1, |i| i.width), info.map_or(1, |i| i.height), info.is_some_and(|i| i.alpha))
                }
                None => {
                    let Ok(lif) = crate::assets::load_lif(src, &path) else {
                        continue;
                    };
                    let mut rgba = lif.to_rgba();
                    if !lif.has_alpha() {
                        rgba.chunks_mut(4).for_each(|p| p[3] = 255);
                    }
                    let t = renderer.create_effect_texture(lif.width, lif.height, &rgba);
                    self.batch_textures.insert(path.clone(), t);
                    (t, lif.width, lif.height, lif.has_alpha())
                }
            };
            let (w, h) = (w.max(1) as f32, h.max(1) as f32);
            out.sprites.push(effects::Sprite {
                position: c3(s.position),
                half_size: Vec2::new(s.size[0], s.size[1]) * 0.5,
                rotation: s.rotation,
                color: s.color,
                // The batch puts uv[3] on the bottom edge; the original puts v0 there.
                uv: [
                    s.uv[0][0] as f32 / w,
                    s.uv[1][1] as f32 / h,
                    s.uv[1][0] as f32 / w,
                    s.uv[0][1] as f32 / h,
                ],
                texture,
                blend: if alpha && s.additive {
                    effects::Blend::Additive
                } else {
                    effects::Blend::Alpha
                },
            });
        }
        let line = |a: Vector, b: Vector, width: f32, color: [f32; 4], bl: fxr::Blend| effects::Line {
            from: c3(a),
            to: c3(b),
            from_color: color,
            to_color: color,
            width: raster_width(width),
            blend: blend(bl),
        };
        for l in &list.lines {
            out.lines.push(line(l.start, l.end, l.width, l.color, l.blend));
        }
        for b in &list.bolts {
            for w in b.points.windows(2) {
                out.lines.push(line(w[0], w[1], b.width, b.color, b.blend));
            }
        }
        for p in &list.points {
            let mut l = line(p.position, p.position, p.size, p.color, p.blend);
            l.to = l.from + Vec3::splat(1e-3);
            out.lines.push(l);
        }
    }
}

/// Uploads a one-off vertex list (used by tests to check GPU state handling).
#[doc(hidden)]
pub fn upload_vertices(device: &wgpu::Device, vertices: &[FxVertex]) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fx test vertices"),
        contents: bytemuck::cast_slice(vertices),
        usage: wgpu::BufferUsages::VERTEX,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> FxView {
        let view = Mat4::look_at_rh(Vec3::new(0.0, -100.0, 0.0), Vec3::ZERO, Vec3::Z);
        let proj = Mat4::perspective_rh(1.0, 1.0, 1.0, 1000.0);
        FxView::new(view, proj, 200, 200)
    }

    struct NoAssets;
    impl FxAssets for NoAssets {
        fn texture(&self, name: &str) -> Option<FxTexture> {
            (name == "tex").then_some(FxTexture {
                width: 64,
                height: 32,
                alpha: true,
                index: 3,
                alternate: Some(4),
            })
        }
        fn mesh(&self, _: &str) -> Option<&FxMesh> {
            None
        }
    }

    fn sprite() -> fxr::Sprite {
        fxr::Sprite {
            position: Vector::ZERO,
            size: [10.0, 20.0],
            rotation: 0.0,
            color: [1.0, 0.5, 0.25, 1.0],
            bias: [0.0; 3],
            texture: Some("tex".into()),
            uv: [[0, 0], [32, 16]],
            slices: 8,
            blend: fxr::Blend::Alpha,
            stipple: false,
            additive: true,
            illum: 0.0,
        }
    }

    fn run(list: &RenderList) -> FxFrameData {
        let mut out = FxFrameData::default();
        tessellate(
            list,
            &view(),
            &SceneLighting::default(),
            &FxOptions::default(),
            &NoAssets,
            &mut out,
        );
        out
    }

    #[test]
    fn paths_are_normalised() {
        assert_eq!(texture_file("ETG\\\\textures\\glow32"), "etg\\textures\\glow32.lif");
        assert_eq!(texture_file("LensFlares\\lensflare.psd"), "lensflares\\lensflare.lif");
        assert_eq!(mesh_file("ETG\\meshes\\misc\\bullet.geo"), "etg\\meshes\\misc\\bullet.geo");
        assert_eq!(mesh_file("etg/meshes/cross"), "etg\\meshes\\cross.geo");
    }

    #[test]
    fn billboard_uses_screen_axes_and_texel_rect() {
        let mut list = RenderList::default();
        list.sprites.push(sprite());
        let f = run(&list);
        assert_eq!(f.draws.len(), 1);
        let d = f.draws[0];
        // Alpha texture + PART_ADDITIVE: additive, no depth writes, no culling.
        assert_eq!(d.state.blend, FxBlend::Additive);
        assert!(!d.state.depth_write && !d.state.cull);
        assert_eq!(d.texture, 3);
        let v = &f.vertices;
        // Bottom-left corner: -x right, -y up; screen right is +X, up is +Z here.
        assert_eq!(v[0].position, [-5.0, 0.0, -10.0]);
        assert_eq!(v[0].uv, [0.0, 0.0]);
        // Top-right: (u1 / width, v1 / height).
        assert_eq!(v[2].position, [5.0, 0.0, 10.0]);
        assert_eq!(v[2].uv, [0.5, 0.5]);
    }

    #[test]
    fn rotation_turns_counter_clockwise_on_screen() {
        let c = billboard_corners(1.0, 1.0, std::f32::consts::FRAC_PI_2);
        // The bottom-right corner (1, -1) turns to the top-right (1, 1).
        let [x, y] = c[1].0;
        assert!((x - 1.0).abs() < 1e-6 && (y - 1.0).abs() < 1e-6);
    }

    #[test]
    fn opaque_textures_ignore_the_additive_flag() {
        let mut s = sprite();
        s.texture = Some("missing".into());
        let mut list = RenderList::default();
        list.sprites.push(s.clone());
        assert_eq!(run(&list).draws[0].state.blend, FxBlend::Opaque);
        s.color[3] = 0.5;
        list.sprites[0] = s;
        assert_eq!(run(&list).draws[0].state.blend, FxBlend::Alpha);
    }

    #[test]
    fn bias_adds_a_second_additive_pass_with_the_alternate() {
        let mut s = sprite();
        s.bias = [0.5, 0.0, 0.0];
        s.additive = false;
        let mut list = RenderList::default();
        list.sprites.push(s);
        let f = run(&list);
        assert_eq!(f.draws.len(), 2);
        assert_eq!(f.draws[0].state.blend, FxBlend::Alpha);
        assert_eq!(f.draws[1].state.blend, FxBlend::Additive);
        assert_eq!(f.draws[1].texture, 4);
        assert_eq!(f.vertices[6].color, [0.5, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn discs_start_at_the_top_and_use_scale_as_radius() {
        let ring = disc_outline(2.0, 4);
        assert_eq!(ring.len(), 5);
        assert_eq!(ring[0], [0.0, 2.0]);
        assert!((ring[1][0] - 2.0).abs() < 1e-6 && ring[1][1].abs() < 1e-6);
        assert_eq!(ring[4], [0.0, 2.0]);
        let mut s = sprite();
        s.texture = None;
        s.slices = 4;
        let mut list = RenderList::default();
        list.sprites.push(s);
        let f = run(&list);
        assert_eq!(f.vertices.len(), 12);
        assert_eq!(f.draws[0].state.blend, FxBlend::Opaque);
        // Colour bytes: round(0.5 * 256) = 128.
        assert!((f.vertices[0].color[1] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn wide_lines_widen_along_the_minor_axis() {
        let mut list = RenderList::default();
        list.lines.push(fxr::Line {
            start: Vector::new(-10.0, 0.0, 0.0),
            end: Vector::new(10.0, 0.0, 1.0),
            width: 2.6,
            color: [1.0; 4],
            blend: fxr::Blend::Additive,
        });
        let f = run(&list);
        assert_eq!(f.vertices.len(), 6);
        // x-major on screen: pushed vertically by half the rounded width (3 px).
        assert_eq!(f.vertices[0].offset, [-0.0, -1.5]);
        assert!(f.draws[0].state.depth_write);
    }

    #[test]
    fn bolts_are_opaque_by_default() {
        let mut list = RenderList::default();
        list.bolts.push(fxr::Bolt {
            points: vec![Vector::ZERO, Vector::new(0.0, 0.0, 5.0), Vector::new(1.0, 0.0, 10.0)],
            width: 1.0,
            color: [0.5, 0.5, 1.0, 0.5],
            blend: fxr::Blend::Additive,
        });
        let f = run(&list);
        assert_eq!(f.vertices.len(), 12);
        assert_eq!(f.draws[0].state.blend, FxBlend::Opaque);
    }

    #[test]
    fn spans_keep_system_order() {
        let mut list = RenderList::default();
        list.lines.push(fxr::Line {
            start: Vector::ZERO,
            end: Vector::new(0.0, 0.0, 5.0),
            width: 1.0,
            color: [1.0; 4],
            blend: fxr::Blend::Alpha,
        });
        list.push_span(PrimKind::Lines, 0, false, true);
        list.sprites.push(sprite());
        list.push_span(PrimKind::Sprites, 0, false, false);
        let f = run(&list);
        assert_eq!(f.draws[0].state.blend, FxBlend::Alpha);
        assert_eq!(f.draws[1].state.blend, FxBlend::Additive);
    }

    #[test]
    fn flare_script_parses() {
        let set = LensFlareSet::parse(
            "FadeoutFrameSize 1.1\nTextureFile lensflare.psd\nTexture 0 64 64 64\n// Element 1 1 1 1 1 1\nElement\t0.5\t-0.49\t0\t255\t128\t0\nElement 0.1 1 -2 1 2 3",
        );
        assert_eq!(set.fadeout_frame_size, 1.1);
        assert_eq!(set.rects, vec![[0.0, 0.25, 0.25, 0.25]]);
        assert_eq!(set.elements.len(), 2);
        assert_eq!(set.elements[0].color, [255, 128, 0]);
        assert_eq!(set.elements[1].image, -2);
        assert_eq!(set.texture_path().as_deref(), Some("lensflares\\lensflare.lif"));
    }

    #[test]
    fn flares_fade_in_and_mark_themselves_undrawn() {
        let set = LensFlareSet::parse("Texture 0 0 64 64\nElement 0.1 1.0 0 255 255 255");
        let tex = FxTexture {
            width: 256,
            height: 256,
            alpha: true,
            index: 1,
            alternate: None,
        };
        let mut flares = vec![LensFlare {
            position: Vector::ZERO,
            intensity: 1.0,
            enabled: 1,
            pending: 2,
            active: 1,
        }];
        let mut states = Vec::new();
        let mut out = FxFrameData::default();
        let slots = lens_flare_pass(&mut flares, &mut states, &set, Some(tex), &view(), None, &mut out);
        assert_eq!(slots, vec![0]);
        assert!((states[0].fade - 0.4).abs() < 1e-6);
        assert_eq!(states[0].screen, Vec2::new(100.0, 100.0));
        assert_eq!(flares[0].pending, 1);
        assert_eq!(out.draws.len(), 1);
        assert!(out.draws[0].state.screen);
        // Element half size = 0.1 × 200 px around (100.325, 100.325).
        assert!((out.vertices[0].position[0] - ((80.325 / 200.0) * 2.0 - 1.0)).abs() < 1e-6);
        // Not drawn by its system since: fades out.
        out.clear();
        lens_flare_pass(&mut flares, &mut states, &set, Some(tex), &view(), None, &mut out);
        assert!((states[0].fade - 0.24).abs() < 1e-6);
        // Behind the camera: invisible.
        flares[0].position = Vector::new(0.0, -200.0, 0.0);
        flares[0].pending = 0;
        out.clear();
        lens_flare_pass(&mut flares, &mut states, &set, Some(tex), &view(), None, &mut out);
        assert!(!states[0].visible);
        assert!(out.draws.is_empty());
    }

    #[test]
    fn saturating_bias() {
        assert_eq!(saturate_add([0.5, 0.0, 0.0], [1.0, 0.5, 0.0]), [1.0, 1.0 / 3.0, 0.0]);
        assert_eq!(saturate_add([0.0; 3], [0.0; 3]), [0.0; 3]);
    }
}
