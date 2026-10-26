//! Drawing nebula tendrils the way `nebRender` / `nebRenderNebula` did on the OpenGL path.
//!
//! The simulation (`hwc_sim::nebula`) owns the nebulae: chunks, tendrils, the cylinders made
//! the first time a tendril is seen, and the chunks' wobble. This module takes that state as
//! plain input ([`NebulaInput`]) for one camera and produces the original's geometry:
//!
//! * per tendril (table order), when the simulation says it is drawn (active, not behind the
//!   eye, an end within 1.1 × the far clip): the detail level, 0 when either end chunk is in
//!   the render list, else 1 (`nebLOD`); its colour, darkened and desaturated by the mean
//!   distance of its ends from the eye (`nebColourTendril`, `nebColorDarkenAndDesaturate`)
//!   with the alpha scaled by its fade factor;
//! * the joiners of its end chunks (`nebRenderChunk` / `nebDrawChunk2`): a chunk where exactly
//!   two drawable tendrils meet gets a band of quads from the end ring of the first to the
//!   start ring of the second, once per frame, skipped at the free end of a chain;
//! * the tendril itself (`nebDrawTendril`): quads between successive rings, the start ring
//!   moved by the start chunk's wobble and the end ring by the end chunk's, the outer rings of
//!   a chain transparent.
//!
//! State as the original sets it: no lighting or texture (the GL path; lighting is only used
//! by the software renderer), alpha blending (`SRC_ALPHA, ONE_MINUS_SRC_ALPHA`), depth test on
//! and depth writes off, back faces culled, drawn after everything else in the scene. Fog is
//! never enabled on the GL path, so the fog density `nebSetFog` computes is not applied.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;
use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{fild_u32, ftol, ftol_i32, ld, round_magic, st};
use hwc_math::Vector;
use wgpu::util::DeviceExt;

use crate::fx::FxView;
use crate::gpu::{Renderer, DEPTH_FORMAT};

/// `TENDRILBRIGHTEN`: added to the alpha when blending is not a fast feature.
pub const TENDRIL_BRIGHTEN: u8 = 60;
/// `MAXD` of `nebColourTendril`.
pub const COLOUR_MAX_DISTANCE: f32 = 40_000.0;

/// One detail level of a tendril's cylinder: `slices * (stacks + 1)` vertices, ring by ring.
#[derive(Debug, Clone, Copy, Default)]
pub struct CylinderInput<'a> {
    pub slices: u32,
    pub stacks: u32,
    pub vertices: &'a [Vector],
}

impl CylinderInput<'_> {
    fn ready(&self) -> bool {
        self.slices > 0 && !self.vertices.is_empty()
    }
}

/// A tendril as the renderer needs it.
#[derive(Debug, Clone, Copy, Default)]
pub struct TendrilInput<'a> {
    /// Start and end chunk (indices into [`NebulaInput::chunks`]).
    pub a: usize,
    pub b: usize,
    /// `NEB_TENDRIL_INACTIVE` (switched off: never drawn, not counted at the chunks).
    pub inactive: bool,
    /// First / last segment of its chain.
    pub leading: bool,
    pub trailing: bool,
    /// Selected for drawing by the simulation's `drawn_tendrils` for this camera.
    pub drawn: bool,
    /// `realColour` RGBA.
    pub real_colour: [u8; 4],
    pub fade_factor: f32,
    pub lods: [CylinderInput<'a>; 2],
}

/// A chunk as the renderer needs it.
#[derive(Debug, Clone, Copy, Default)]
pub struct ChunkInput {
    pub position: Vector,
    /// Wobble offset (`dPos`).
    pub d_pos: Vector,
    /// Attached tendrils in attachment order (indices into [`NebulaInput::tendrils`]).
    pub tendrils: [Option<u16>; 3],
    /// In the universe's render list (`univSpaceObjInRenderList`).
    pub in_render_list: bool,
}

/// One nebula.
#[derive(Debug, Clone, Default)]
pub struct NebulaInput<'a> {
    pub chunks: Vec<ChunkInput>,
    pub tendrils: Vec<TendrilInput<'a>>,
}

/// Frame parameters.
#[derive(Debug, Clone, Copy)]
pub struct NebulaParams {
    /// Camera eye (world).
    pub eye: Vector,
    /// `NEB_DISTANCE_DESATURATION`.
    pub distance_desaturation: f32,
    /// `glCapFastFeature(GL_BLEND)`: true on any hardware renderer (no alpha boost).
    pub bright: bool,
}

/// A vertex: world position and RGBA.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct NebulaVertex {
    pub position: [f32; 3],
    pub color: [u8; 4],
}

/// `nebColorDarkenAndDesaturate`: in HSV, value minus `dark/255` and saturation minus
/// `desat/255` (both floored at 0), back to bytes rounding `x * 256`; alpha kept.
pub fn darken_and_desaturate(c: [u8; 4], dark: i32, desat: i32) -> [u8; 4] {
    const INV255: f32 = 0.003_921_569;
    let r = st(fild_u32(u32::from(c[0])) * ld(INV255));
    let g = st(fild_u32(u32::from(c[1])) * ld(INV255));
    let b = st(fild_u32(u32::from(c[2])) * ld(INV255));
    let (h, mut s, mut v) = rgb_to_hsv(r, g, b);
    v = st(ld(v) - f64::from(dark) * ld(INV255));
    if v < 0.0 {
        v = 0.0;
    }
    s = st(ld(s) - f64::from(desat) * ld(INV255));
    if s < 0.0 {
        s = 0.0;
    }
    let (r, g, b) = hsv_to_rgb(h, s, v);
    let byte = |x: f32| {
        let n = round_magic(ld(x) * 256.0);
        if n < 0x100 {
            (n & 0xff) as u8
        } else {
            0xff
        }
    };
    [byte(r), byte(g), byte(b), c[3]]
}

/// `colRGBToHSV` (hue 0..1, -1 when undefined).
pub fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = if r <= g.max(b) { g.max(b) } else { r };
    let min = if g.min(b) < r { g.min(b) } else { r };
    let v = max;
    let s = if max == 0.0 { 0.0 } else { st((ld(max) - ld(min)) / ld(max)) };
    if s == 0.0 {
        return (-1.0, s, v);
    }
    let delta = ld(max) - ld(min);
    let h = if r == max {
        st((ld(g) - ld(b)) / delta)
    } else if g == max {
        st((ld(b) - ld(r)) / delta + 2.0)
    } else {
        st((ld(r) - ld(g)) / delta + 4.0)
    };
    let mut h = st(ld(h) * ld(0.166_666_67f32));
    if h < 0.0 {
        h = st(ld(h) + 1.0);
    }
    (h, s, v)
}

/// `colHSVToRGB`.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    if s == 0.0 {
        return (v, v, v);
    }
    let h = if h == 1.0 { 0.0 } else { h };
    let h6 = ld(h) * 6.0;
    let i = h6.floor();
    let f = h6 - i;
    let p = st((1.0 - ld(s)) * ld(v));
    let q = st((1.0 - f * ld(s)) * ld(v));
    let t = st((1.0 - (1.0 - f) * ld(s)) * ld(v));
    match ftol(i) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        5 => (v, p, q),
        _ => (0.0, 0.0, 0.0),
    }
}

/// `nebColourTendril`: the tendril's colour this frame from the distances of its end chunks.
pub fn tendril_colour(real: [u8; 4], fade: f32, pa: Vector, pb: Vector, eye: Vector, desaturation: f32) -> [u8; 4] {
    let dist = |p: Vector| {
        let (x, y, z) = (ld(p.x) - ld(eye.x), ld(p.y) - ld(eye.y), ld(p.z) - ld(eye.z));
        fmath_sqrt_double((z * z + y * y) + x * x)
    };
    let max = ld(COLOUR_MAX_DISTANCE);
    let da = ld(st(dist(pa))).min(max);
    let db = dist(pb).min(max);
    let d = (db + da) * 0.5 / max * ld(desaturation);
    let desat = ftol_i32(d);
    let dark = ftol_i32(d * 0.75);
    let mut c = darken_and_desaturate(real, dark, desat);
    c[3] = ftol_i32(fild_u32(u32::from(c[3])) * ld(fade)) as u8;
    c
}

fn with_alpha(c: [u8; 4], a: u8, bright: bool) -> [u8; 4] {
    let a = if bright { a } else { a.wrapping_add(TENDRIL_BRIGHTEN) };
    [c[0], c[1], c[2], a]
}

fn add(v: Vector, d: Vector) -> [f32; 3] {
    [st(ld(v.x) + ld(d.x)), st(ld(v.y) + ld(d.y)), st(ld(v.z) + ld(d.z))]
}

/// Appends a GL quad as two triangles, keeping its winding.
fn quad(out: &mut Vec<NebulaVertex>, q: [NebulaVertex; 4]) {
    out.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
}

/// The triangles of all nebulae for one frame, in the original's drawing order.
pub fn build_vertices(nebulae: &[NebulaInput], params: &NebulaParams) -> Vec<NebulaVertex> {
    let mut out = Vec::new();
    for neb in nebulae {
        build_nebula(neb, params, &mut out);
    }
    out
}

fn build_nebula(neb: &NebulaInput, params: &NebulaParams, out: &mut Vec<NebulaVertex>) {
    if neb.tendrils.is_empty() {
        return;
    }
    let ok = |t: &TendrilInput| t.a < neb.chunks.len() && t.b < neb.chunks.len();
    // Colours: every drawable tendril is coloured before anything is drawn (the original
    // colours them as it reaches them; the joiners may read a neighbour's colour).
    let colours: Vec<[u8; 4]> = neb
        .tendrils
        .iter()
        .map(|t| if ok(t) { tendril_colour(t.real_colour, t.fade_factor, neb.chunks[t.a].position, neb.chunks[t.b].position, params.eye, params.distance_desaturation) } else { t.real_colour })
        .collect();
    let mut chunk_done = vec![false; neb.chunks.len()];
    for (i, t) in neb.tendrils.iter().enumerate() {
        if t.inactive || !t.drawn || !ok(t) {
            continue;
        }
        let lod = if neb.chunks[t.a].in_render_list || neb.chunks[t.b].in_render_list { 0 } else { 1 };
        if !t.leading {
            chunk_joiner(neb, t.a, lod, &colours, &mut chunk_done, params, out);
        }
        if !t.trailing {
            chunk_joiner(neb, t.b, lod, &colours, &mut chunk_done, params, out);
        }
        draw_tendril(neb, t, lod, colours[i], params, out);
    }
}

/// `nebRenderChunk` / `nebDrawChunk2`.
fn chunk_joiner(neb: &NebulaInput, c: usize, lod: usize, colours: &[[u8; 4]], done: &mut [bool], params: &NebulaParams, out: &mut Vec<NebulaVertex>) {
    if done[c] {
        return;
    }
    done[c] = true;
    let chunk = &neb.chunks[c];
    let live: Vec<usize> = chunk
        .tendrils
        .iter()
        .flatten()
        .map(|&t| usize::from(t))
        .filter(|&t| t < neb.tendrils.len() && !neb.tendrils[t].inactive && neb.tendrils[t].lods[lod].ready())
        .collect();
    if live.len() != 2 {
        return;
    }
    let (ia, ib) = (live[0], live[1]);
    let (ta, tb) = (&neb.tendrils[ia].lods[lod], &neb.tendrils[ib].lods[lod]);
    let (ca, cb) = (colours[ia], colours[ib]);
    let (ca, cb) = (with_alpha(ca, ca[3], params.bright), with_alpha(cb, cb[3], params.bright));
    let n = ta.slices as usize;
    let last = ta.vertices.len() - n;
    for i in 0..n {
        let get = |l: &CylinderInput, k: usize| l.vertices.get(k).copied().unwrap_or(Vector::ZERO);
        let a0 = get(ta, last + i);
        let b0 = get(tb, i);
        let a1 = get(ta, last + (i + 1) % n);
        let b1 = get(tb, (i + 1) % tb.slices as usize);
        let v = |p: Vector, color| NebulaVertex { position: add(p, chunk.d_pos), color };
        quad(out, [v(a0, ca), v(b0, cb), v(b1, cb), v(a1, ca)]);
    }
}

/// `nebDrawTendril`.
fn draw_tendril(neb: &NebulaInput, t: &TendrilInput, lod: usize, colour: [u8; 4], params: &NebulaParams, out: &mut Vec<NebulaVertex>) {
    let l = &t.lods[lod];
    if !l.ready() {
        return;
    }
    let (da, db) = (neb.chunks[t.a].d_pos, neb.chunks[t.b].d_pos);
    let normal = with_alpha(colour, colour[3], params.bright);
    let faded = with_alpha(colour, 0, params.bright);
    let n = l.slices as usize;
    let stacks = l.stacks as usize;
    let vert = |ring: usize, k: usize| {
        let p = l.vertices.get(ring * n + k).copied().unwrap_or(Vector::ZERO);
        if ring == 0 {
            NebulaVertex { position: add(p, da), color: if t.leading { faded } else { normal } }
        } else if ring == stacks {
            NebulaVertex { position: add(p, db), color: if t.trailing { faded } else { normal } }
        } else {
            NebulaVertex { position: [p.x, p.y, p.z], color: normal }
        }
    };
    for j in 1..=stacks {
        for i in 0..n {
            let k = (i + 1) % n;
            quad(out, [vert(j - 1, i), vert(j, i), vert(j, k), vert(j - 1, k)]);
        }
    }
}

// ------------------------------------------------------------------------------- GPU

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct NebulaUniform {
    view_proj: [[f32; 4]; 4],
}

const SHADER: &str = r#"
struct Frame { view_proj: mat4x4<f32> };
@group(0) @binding(0) var<uniform> frame: Frame;
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs_nebula(@location(0) p: vec3<f32>, @location(1) c: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = frame.view_proj * vec4<f32>(p, 1.0);
    o.color = c;
    return o;
}
@fragment fn fs_nebula(i: VsOut) -> @location(0) vec4<f32> { return i.color; }
"#;

/// Draws the nebulae over a scene already in the colour and depth targets.
pub struct NebulaRenderer {
    pipeline: wgpu::RenderPipeline,
    uniform: wgpu::Buffer,
    group: wgpu::BindGroup,
    vertex_buffer: Option<(wgpu::Buffer, u64)>,
    vertices: Vec<NebulaVertex>,
    view_proj: Mat4,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
    /// MSAA samples the pipeline was built for.
    samples: u32,
}

impl NebulaRenderer {
    pub fn new(renderer: &Renderer) -> Self {
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("hwc nebula"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("nebula frame"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("nebula frame"),
            contents: bytemuck::bytes_of(&NebulaUniform { view_proj: Mat4::IDENTITY.to_cols_array_2d() }),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("nebula frame"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("nebula"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let samples = renderer.sample_count();
        let color_format = renderer.color_format();
        let pipeline = Self::build_pipeline(device, &shader, &pipeline_layout, color_format, samples);
        NebulaRenderer {
            pipeline,
            uniform,
            group,
            vertex_buffer: None,
            vertices: Vec::new(),
            view_proj: Mat4::IDENTITY,
            shader,
            pipeline_layout,
            color_format,
            samples,
        }
    }

    fn build_pipeline(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        pipeline_layout: &wgpu::PipelineLayout,
        color_format: wgpu::TextureFormat,
        samples: u32,
    ) -> wgpu::RenderPipeline {
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<NebulaVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Unorm8x4],
        };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("nebula"),
            layout: Some(pipeline_layout),
            vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_nebula"), compilation_options: Default::default(), buffers: &[Some(vertex_layout)] },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_nebula"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: color_format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// Builds this frame's geometry.
    pub fn prepare(&mut self, nebulae: &[NebulaInput], params: &NebulaParams, view: &FxView) {
        self.vertices = build_vertices(nebulae, params);
        self.view_proj = view.view_proj();
    }

    /// Triangles prepared (three vertices each).
    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    /// Records the prepared frame as a render pass that keeps `color` and `depth`.
    pub fn draw(&mut self, renderer: &Renderer, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView, depth: &wgpu::TextureView) {
        if self.vertices.is_empty() {
            return;
        }
        let device = renderer.device();
        if self.samples != renderer.sample_count() {
            self.samples = renderer.sample_count();
            self.pipeline = Self::build_pipeline(device, &self.shader, &self.pipeline_layout, self.color_format, self.samples);
        }
        renderer.queue().write_buffer(&self.uniform, 0, bytemuck::bytes_of(&NebulaUniform { view_proj: self.view_proj.to_cols_array_2d() }));
        let bytes: &[u8] = bytemuck::cast_slice(&self.vertices);
        let size = bytes.len() as u64;
        if self.vertex_buffer.as_ref().is_none_or(|(_, cap)| *cap < size) {
            let cap = size.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("nebula vertices"), size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.vertex_buffer = Some((buffer, cap));
        }
        let Some((buffer, _)) = &self.vertex_buffer else { return };
        renderer.queue().write_buffer(buffer, 0, bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("nebulae"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.group, &[]);
        pass.set_vertex_buffer(0, buffer.slice(..size));
        pass.draw(0..self.vertices.len() as u32, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn darkening_keeps_hue_and_alpha() {
        assert_eq!(darken_and_desaturate([255, 64, 102, 163], 0, 0), [255, 64, 102, 163]);
        let c = darken_and_desaturate([255, 64, 102, 163], 75, 100);
        assert_eq!(c[3], 163);
        assert!(c[0] < 255 && c[0] > c[2] && c[2] > c[1], "{c:?}");
        // Fully desaturated grey at the value left.
        let g = darken_and_desaturate([200, 50, 50, 10], 0, 255);
        assert_eq!((g[0], g[1]), (g[1], g[2]));
        // `x * 256` rounding with the clamp: white stays 255.
        assert_eq!(darken_and_desaturate([255, 255, 255, 0], 0, 0), [255, 255, 255, 0]);
    }

    #[test]
    fn hsv_round_trip() {
        for &(r, g, b) in &[(1.0f32, 0.25f32, 0.4f32), (0.2, 0.9, 0.1), (0.3, 0.3, 0.8), (0.5, 0.5, 0.5)] {
            let (h, s, v) = rgb_to_hsv(r, g, b);
            let (r2, g2, b2) = hsv_to_rgb(h, s, v);
            assert!((r - r2).abs() < 1e-5 && (g - g2).abs() < 1e-5 && (b - b2).abs() < 1e-5, "{r} {g} {b} -> {r2} {g2} {b2}");
        }
    }

    #[test]
    fn colour_darkens_with_distance_and_fades() {
        let eye = Vector::ZERO;
        let near = tendril_colour([255, 64, 102, 163], 1.0, Vector::new(100.0, 0.0, 0.0), Vector::new(200.0, 0.0, 0.0), eye, 150.0);
        let far = tendril_colour([255, 64, 102, 163], 0.5, Vector::new(90_000.0, 0.0, 0.0), Vector::new(80_000.0, 0.0, 0.0), eye, 150.0);
        assert_eq!(near, [255, 64, 102, 163]);
        assert!(far[0] < near[0]);
        assert_eq!(far[3], 81);
    }

    fn ring_cylinder(z0: f32, stacks: u32) -> Vec<Vector> {
        let mut v = Vec::new();
        for j in 0..=stacks {
            for i in 0..6 {
                let a = i as f32 * std::f32::consts::TAU / 6.0;
                v.push(Vector::new(-a.sin() * 100.0, a.cos() * 100.0, z0 + j as f32 * 1000.0));
            }
        }
        v
    }

    #[test]
    fn a_chain_of_two_draws_two_tendrils_and_one_joiner() {
        let c0 = ring_cylinder(0.0, 1);
        let c1 = ring_cylinder(1000.0, 1);
        fn lods(v: &[Vector]) -> [CylinderInput<'_>; 2] {
            let l = CylinderInput { slices: 6, stacks: 1, vertices: v };
            [l, l]
        }
        let chunk = |z: f32, t: [Option<u16>; 3]| ChunkInput { position: Vector::new(0.0, 0.0, z), d_pos: Vector::ZERO, tendrils: t, in_render_list: false };
        let neb = NebulaInput {
            chunks: vec![chunk(0.0, [Some(0), None, None]), chunk(1000.0, [Some(0), Some(1), None]), chunk(2000.0, [Some(1), None, None])],
            tendrils: vec![
                TendrilInput { a: 0, b: 1, leading: true, drawn: true, real_colour: [255, 64, 102, 163], fade_factor: 1.0, lods: lods(&c0), ..Default::default() },
                TendrilInput { a: 1, b: 2, trailing: true, drawn: true, real_colour: [255, 64, 102, 163], fade_factor: 1.0, lods: lods(&c1), ..Default::default() },
            ],
        };
        let p = NebulaParams { eye: Vector::new(5000.0, 0.0, 0.0), distance_desaturation: 150.0, bright: true };
        let v = build_vertices(std::slice::from_ref(&neb), &p);
        // Two tendrils of 6 quads, one joiner of 6 quads, 6 vertices per quad.
        assert_eq!(v.len(), 3 * 6 * 6);
        // The joiner at the middle chunk comes first, then the leading tendril, whose start
        // ring is transparent.
        assert_eq!(v[0].color[3], 163);
        assert_eq!(v[36].color[3], 0);
        assert_eq!(v[37].color[3], 163);
        assert!(v.iter().any(|x| x.color[3] > 0));
        // Nothing drawn when the simulation selected nothing.
        let mut none = neb;
        for t in &mut none.tendrils {
            t.drawn = false;
        }
        assert!(build_vertices(&[none], &p).is_empty());
    }
}
