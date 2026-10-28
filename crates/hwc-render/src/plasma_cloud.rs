//! Cataclysm's plasma cloud (`plasmaCloud*`, 0x576130..0x578260): the fog of the nebula NIS
//! (n04 before mission 6, n05 before mission 7). Two layers of the cloud texture
//! (`race_derelict\dplasmacloud\rl0\lod0\cloud`) scroll across the whole screen, added over
//! the scene in the cloud's colour, while the sky background fades out.
//!
//! * A level's `Fog r,g,b, r2,g2,b2, speed, scale` line (`plasmaCloudSetFromScript` 0x5774f0)
//!   sets the colours, the scroll speed and the scale (stored ×1e7), loads the texture when the
//!   scale is above 0 (and unloads it otherwise), and switches the cloud on with it.
//! * `nisStart` / `nisStop` / `levelPreInit` switch it off (`plasmaCloudReset` 0x5776a0).
//! * The NIS event `fog active seconds r g b r2 g2 b2 speed scale` (`nisFogSet` 0x4ce8c0,
//!   `nisFog` 0x4cbd20) sets the switch and fades the colours, speed and scale to the new values
//!   over `seconds` of game time (`plasmaCloudFadeTo` 0x5776c0), or sets them at once.
//! * Each frame `plasmaCloudUpdate` (0x577780) steps the fade and sets the background's colour
//!   multiplier (`btgSetColourMultiplier(1, ..)`): 1 with the cloud off, 0 with it on at a
//!   scale above 0, faded between the two with the scale.
//! * `plasmaCloudRender` (0x577c40, after the scene) draws it while it is on, its texture loaded
//!   and `scale × 3.333e-8` above 0: the layers' texture coordinates move by
//!   `speed × frame time × their velocity` (wrapped by 100), and each layer is one screen quad,
//!   texture modulated by the first colour × that factor (at most 1), blended
//!   `SRC_ALPHA, ONE` with depth writes off. The second colour is faded but never drawn.

use bytemuck::{Pod, Zeroable};
use hwc_math::fpu::{fild_i32, ftol_i32, ld, st};

use crate::gpu::Renderer;

/// The texture the cloud draws (`.lif` under the game data).
pub const TEXTURE: &str = "race_derelict\\dplasmacloud\\rl0\\lod0\\cloud.lif";

/// `0x721ea8`: the scale times this is the cloud's strength.
const SCALE_TO_STRENGTH: f32 = 3.333_333_3e-8;

/// A colour packed as the original keeps it: `0xffBBGGRR`, red in the low byte.
fn pack(r: i32, g: i32, b: i32) -> u32 {
    ((((b as u32 & 0xff) | 0xffff_ff00) << 8 | (g as u32 & 0xff)) << 8) | (r as u32 & 0xff)
}

fn channel(c: u32, i: u32) -> i32 {
    ((c >> (8 * i)) & 0xff) as i32
}

/// One scrolling layer: texture offset, velocity and size (`0x91b0a4`, `0x91b098`, `0x91b0c4`
/// and their `v` counterparts).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Layer {
    pub u: f32,
    pub v: f32,
    pub du: f32,
    pub dv: f32,
    pub size: f32,
}

/// What [`PlasmaCloud::render`] asks to draw.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudDraw {
    /// The quads' colour, 0..1 (`glColor3f`).
    pub colour: [f32; 3],
    pub layers: [Layer; 2],
}

/// The cloud's state (the globals at 0xad7f54..0xad7f8c and the layers at 0x91b098..).
#[derive(Debug, Clone, PartialEq)]
pub struct PlasmaCloud {
    /// 0xad7f54.
    pub active: bool,
    /// The texture is loaded (0x91b0f0 non-zero).
    pub texture: bool,
    /// 0xad7f58 / 0xad7f5c.
    pub colour: u32,
    pub colour2: u32,
    /// 0xad7f60 (scroll speed) / 0xad7f64 (scale, ×1e7).
    pub speed: f32,
    pub scale: f32,
    /// The fade (0xad7f68..0xad7f8c): target values, values at its start, start and end time
    /// (`end` 0: no fade).
    to: (u32, u32, f32, f32),
    from: (u32, u32, f32, f32),
    start: f32,
    end: f32,
    /// The background's colour multiplier (`btgSetColourMultiplier(1, ..)`, 0x8d37d4).
    pub sky: f32,
    /// 0x8b0860: the layers are set up on the next render.
    fresh: bool,
    /// 0x91b174: the time of the last render.
    last: f32,
    pub layers: [Layer; 2],
}

impl Default for PlasmaCloud {
    fn default() -> Self {
        Self {
            active: false,
            texture: false,
            colour: 0,
            colour2: 0,
            speed: 0.0,
            scale: 0.0,
            to: (0, 0, 0.0, 0.0),
            from: (0, 0, 0.0, 0.0),
            start: 0.0,
            end: 0.0,
            sky: 1.0,
            fresh: true,
            last: 0.0,
            layers: [Layer::default(); 2],
        }
    }
}

fn scan<T: std::str::FromStr>(it: &mut dyn Iterator<Item = &str>) -> Option<T> {
    it.next().and_then(|s| s.trim().parse().ok())
}

impl PlasmaCloud {
    /// `plasmaCloudReset`: off, the background at full colour.
    pub fn reset(&mut self) {
        self.active = false;
        self.sky = 1.0;
    }

    /// `levelPreInit`: off and the texture unloaded (`plasmaCloudTextureUnload`).
    pub fn level_pre_init(&mut self) {
        self.reset();
        self.texture = false;
    }

    /// `plasmaCloudTextureLoad` (`LoadGame`).
    pub fn load_texture(&mut self) {
        self.texture = true;
    }

    /// `plasmaCloudSetFromScript`: a level's `Fog` line (`"%d,%d,%d,%d,%d,%d,%f,%f"`).
    pub fn set_from_script(&mut self, args: &str) {
        self.active = false;
        self.sky = 1.0;
        if args.trim().is_empty() {
            self.texture = false;
            self.end = 0.0;
            self.speed = 0.0;
            self.scale = 0.0;
            self.colour = 0;
            self.colour2 = 0;
            return;
        }
        let mut it = args.split(',');
        let ints: Vec<Option<i32>> = (0..6).map(|_| scan(&mut it)).collect();
        let speed: Option<f32> = scan(&mut it);
        let scale: Option<f32> = scan(&mut it);
        let (Some(speed), Some(scale)) = (speed, scale) else { return };
        let Some(c) = ints.into_iter().collect::<Option<Vec<i32>>>() else { return };
        let scale = st(ld(scale) * 1e7);
        self.speed = speed;
        self.colour = pack(c[0], c[1], c[2]);
        self.colour2 = pack(c[3], c[4], c[5]);
        self.end = 0.0;
        self.scale = scale;
        if scale <= 0.0 {
            self.texture = false;
            self.active = false;
        } else {
            self.texture = true;
            self.active = true;
        }
    }

    /// `plasmaCloudFadeTo(seconds, colour, colour2, speed, scale)` at game time `now`.
    pub fn fade_to(&mut self, seconds: f32, colour: u32, colour2: u32, speed: f32, scale: f32, now: f32) {
        if 0.0 < seconds {
            self.from = (self.colour, self.colour2, self.speed, self.scale);
            self.end = st(ld(now) + ld(seconds));
            self.start = now;
            self.to = (colour, colour2, speed, scale);
        } else {
            self.speed = speed;
            self.end = 0.0;
            self.scale = scale;
            self.colour = colour;
            self.colour2 = colour2;
        }
    }

    /// The NIS `fog` event (`nisFogSet` reads `"%d %f %d %d %d %d %d %d %f %f"`; `nisFog`
    /// applies it).
    pub fn nis_fog(&mut self, args: &str, now: f32) {
        let mut it = args.split_whitespace();
        let active: i32 = scan(&mut it).unwrap_or(0);
        let seconds: f32 = scan(&mut it).unwrap_or(0.0);
        let c: Vec<i32> = (0..6).map(|_| scan(&mut it).unwrap_or(0)).collect();
        let speed: f32 = scan(&mut it).unwrap_or(0.0);
        let scale: f32 = scan(&mut it).unwrap_or(0.0);
        self.active = active as u8 != 0;
        self.fade_to(seconds, pack(c[0], c[1], c[2]), pack(c[3], c[4], c[5]), speed, st(ld(scale) * 1e7), now);
    }

    /// `plasmaCloudUpdate` at game time `now` (before the scene is drawn).
    pub fn update(&mut self, now: f32) {
        if !self.active {
            self.sky = 1.0;
            return;
        }
        if self.end == 0.0 {
            return;
        }
        let sky_of = |scale: f32| if scale == 0.0 { 1.0 } else { 0.0 };
        if self.end <= now {
            (self.colour, self.colour2, self.speed, self.scale) = self.to;
            self.end = 0.0;
            self.sky = sky_of(self.scale);
            return;
        }
        let t = (ld(now) - ld(self.start)) / (ld(self.end) - ld(self.start));
        self.speed = st((ld(self.to.2) - ld(self.from.2)) * t + ld(self.from.2));
        self.scale = st((ld(self.to.3) - ld(self.from.3)) * t + ld(self.from.3));
        let blend = |from: u32, to: u32| {
            let ch = |i: u32| {
                let (a, b) = (fild_i32(channel(from, i)), fild_i32(channel(to, i)));
                let v = (b - a) * t + a;
                ftol_i32(v.clamp(0.0, 255.0))
            };
            pack(ch(0), ch(1), ch(2))
        };
        self.colour = blend(self.from.0, self.to.0);
        self.colour2 = blend(self.from.1, self.to.1);
        let (a, b) = (sky_of(self.to.3), sky_of(self.from.3));
        self.sky = st((ld(a) - ld(b)) * t + ld(b));
    }

    /// The scroll and draw state of `plasmaCloudRender` at game time `now` (after the scene);
    /// `None` when nothing is drawn.
    pub fn render(&mut self, now: f32) -> Option<CloudDraw> {
        if !self.active || !self.texture {
            return None;
        }
        let mut strength = st(ld(self.scale) * ld(SCALE_TO_STRENGTH));
        if 1.0 < strength {
            strength = 1.0;
        } else if strength == 0.0 {
            return None;
        }
        if self.fresh {
            // The floats as the original stores them (-0.0505, -0.015, 0.2; 0.05, 0.005, 0.25).
            let f = f32::from_bits;
            self.layers = [
                Layer { u: 0.0, v: 0.5, du: f(0xbd4e_d917), dv: f(0xbc75_c28f), size: f(0x3e4c_cccd) },
                Layer { u: 0.0, v: 0.5, du: f(0x3d4c_cccd), dv: f(0x3ba3_d70a), size: 0.25 },
            ];
            self.fresh = false;
        } else {
            let dt = st(ld(now) - ld(self.last));
            self.last = now;
            let k = ld(self.speed) * ld(dt);
            for l in &mut self.layers {
                let u = st(k * ld(l.du) + ld(l.u));
                let v = st(k * ld(l.dv) + ld(l.v));
                l.u = u;
                l.v = v;
                if 100.0 < u {
                    l.u = st(ld(u) - 100.0);
                    l.v = st(ld(v) - 100.0);
                }
            }
        }
        let c = |i: u32| {
            let x = st(fild_i32(channel(self.colour, i)) * ld(strength)).clamp(0.0, 255.0);
            x * 0.003_906_25
        };
        Some(CloudDraw { colour: [c(0), c(1), c(2)], layers: self.layers })
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct CloudVertex {
    position: [f32; 2],
    uv: [f32; 2],
    colour: [f32; 4],
}

const SHADER: &str = r#"
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) colour: vec4<f32>,
};

@group(0) @binding(0) var cloud_tex: texture_2d<f32>;
@group(0) @binding(1) var cloud_sampler: sampler;

@vertex
fn vs_cloud(@location(0) position: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) colour: vec4<f32>) -> VsOut {
    var out: VsOut;
    out.clip = vec4<f32>(position, 0.0, 1.0);
    out.uv = uv;
    out.colour = colour;
    return out;
}

@fragment
fn fs_cloud(in: VsOut) -> @location(0) vec4<f32> {
    return textureSample(cloud_tex, cloud_sampler, in.uv) * in.colour;
}
"#;

/// The quads of a [`CloudDraw`] for a `width` × `height` screen: each layer from
/// `(-1 + 0.65/w, 1 - 0.65/h)` to `(1 + 0.65/w, -1 - 0.65/h)` as the original places them,
/// texture `(u, v)` at the top left and `(u + size, v + size)` at the bottom right.
fn quads(draw: &CloudDraw, width: u32, height: u32) -> Vec<CloudVertex> {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let left = 0.325 / w * 2.0 - 1.0;
    let right = (w + 0.325) / w * 2.0 - 1.0;
    let top = 1.0 - 0.325 / h * 2.0;
    let bottom = 1.0 - (h + 0.325) / h * 2.0;
    let colour = [draw.colour[0], draw.colour[1], draw.colour[2], 1.0];
    let mut out = Vec::with_capacity(12);
    for l in &draw.layers {
        let v = |x: f32, y: f32, u: f32, t: f32| CloudVertex { position: [x, y], uv: [u, t], colour };
        let (a, b, c, d) = (v(left, top, l.u, l.v), v(left, bottom, l.u, l.v + l.size), v(right, bottom, l.u + l.size, l.v + l.size), v(right, top, l.u + l.size, l.v));
        out.extend_from_slice(&[a, b, c, a, c, d]);
    }
    out
}

/// Draws a [`CloudDraw`] over a frame (screen quads, no depth).
pub struct PlasmaCloudRenderer {
    pipeline: wgpu::RenderPipeline,
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    group: wgpu::BindGroup,
    color_format: wgpu::TextureFormat,
    samples: u32,
    vertex_buffer: Option<(wgpu::Buffer, u64)>,
    vertices: Vec<CloudVertex>,
}

impl PlasmaCloudRenderer {
    /// A renderer for the cloud texture (`rgba`, `width` × `height`).
    pub fn new(renderer: &Renderer, width: u32, height: u32, rgba: &[u8]) -> Self {
        use wgpu::util::DeviceExt;
        let device = renderer.device();
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("hwc plasma cloud"), source: wgpu::ShaderSource::Wgsl(SHADER.into()) });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("plasma cloud"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let texture = device.create_texture_with_data(
            renderer.queue(),
            &wgpu::TextureDescriptor {
                label: Some("plasma cloud"),
                size: wgpu::Extent3d { width: width.max(1), height: height.max(1), depth_or_array_layers: 1 },
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
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("plasma cloud"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("plasma cloud"),
            layout: &layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) }, wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("plasma cloud"), bind_group_layouts: &[Some(&layout)], immediate_size: 0 });
        let color_format = renderer.color_format();
        let samples = renderer.sample_count();
        let pipeline = Self::build_pipeline(device, &shader, &pipeline_layout, color_format, samples);
        Self { pipeline, shader, pipeline_layout, group, color_format, samples, vertex_buffer: None, vertices: Vec::new() }
    }

    fn build_pipeline(device: &wgpu::Device, shader: &wgpu::ShaderModule, layout: &wgpu::PipelineLayout, color_format: wgpu::TextureFormat, samples: u32) -> wgpu::RenderPipeline {
        let vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<CloudVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
        };
        // SRC_ALPHA, ONE.
        let add = wgpu::BlendComponent { src_factor: wgpu::BlendFactor::SrcAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add };
        device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("plasma cloud"),
            layout: Some(layout),
            vertex: wgpu::VertexState { module: shader, entry_point: Some("vs_cloud"), compilation_options: Default::default(), buffers: &[Some(vertex_layout)] },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState { count: samples, ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: shader,
                entry_point: Some("fs_cloud"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: color_format, blend: Some(wgpu::BlendState { color: add, alpha: add }), write_mask: wgpu::ColorWrites::COLOR })],
            }),
            multiview_mask: None,
            cache: None,
        })
    }

    /// This frame's quads (`None`: nothing to draw).
    pub fn prepare(&mut self, draw: Option<&CloudDraw>, width: u32, height: u32) {
        self.vertices = draw.map(|d| quads(d, width, height)).unwrap_or_default();
    }

    pub fn vertex_count(&self) -> usize {
        self.vertices.len()
    }

    pub fn draw(&mut self, renderer: &Renderer, encoder: &mut wgpu::CommandEncoder, color: &wgpu::TextureView) {
        if self.vertices.is_empty() {
            return;
        }
        let device = renderer.device();
        if self.samples != renderer.sample_count() {
            self.samples = renderer.sample_count();
            self.pipeline = Self::build_pipeline(device, &self.shader, &self.pipeline_layout, self.color_format, self.samples);
        }
        let bytes: &[u8] = bytemuck::cast_slice(&self.vertices);
        let size = bytes.len() as u64;
        if self.vertex_buffer.as_ref().is_none_or(|(_, cap)| *cap < size) {
            let cap = size.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor { label: Some("plasma cloud vertices"), size: cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            self.vertex_buffer = Some((buffer, cap));
        }
        let Some((buffer, _)) = &self.vertex_buffer else { return };
        renderer.queue().write_buffer(buffer, 0, bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("plasma cloud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
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
    fn mission6_nis_fog_fades_in_and_the_sky_out() {
        let mut c = PlasmaCloud::default();
        // mission06_nis04.mission.
        c.set_from_script("255,255,255, 32,8,8, 0.5,1.0");
        assert!(c.active && c.texture);
        assert_eq!(c.colour, 0xffff_ffff);
        assert_eq!(c.colour2, 0xff08_0820);
        // nisStart, then n04's events at frames 1 and 1360.
        c.reset();
        c.nis_fog("0 0 0 0 0 0 0 0 0 0", 0.0);
        c.update(0.1);
        assert_eq!(c.render(0.1), None);
        assert_eq!(c.sky, 1.0);
        c.nis_fog("1 13 220 220 220 180 40 20 1.0 5.0", 45.0);
        c.update(51.5);
        assert!((c.sky - 0.5).abs() < 1e-6, "{}", c.sky);
        assert_eq!(c.colour & 0xff, 110);
        assert!(c.render(51.5).is_some());
        c.update(60.0);
        assert_eq!(c.sky, 0.0);
        assert_eq!(c.colour, 0xffdc_dcdc);
        let d = c.render(61.0).unwrap();
        // Strength 5e7 × 3.33e-8 clamps to 1: 220/256.
        assert!((d.colour[0] - 220.0 / 256.0).abs() < 1e-6);
        // The layers scroll at speed 1 by their velocities, from the time of the last scroll
        // (none yet: 0, as in the original).
        assert!((d.layers[1].u - 3.05).abs() < 1e-5, "{:?}", d.layers);
        let d = c.render(62.0).unwrap();
        assert!((d.layers[1].u - 3.1).abs() < 1e-5, "{:?}", d.layers);
        // nisStop.
        c.reset();
        c.update(62.0);
        assert_eq!((c.render(62.0), c.sky), (None, 1.0));
    }

    #[test]
    fn a_zero_fog_line_unloads_the_texture() {
        let mut c = PlasmaCloud::default();
        c.set_from_script("255,255,255, 32,8,8, 0.5,1.0");
        c.set_from_script("0,0,0,0,0,0,0,0");
        assert!(!c.active && !c.texture);
        c.nis_fog("1 0 220 220 220 0 0 0 1.0 5.0", 0.0);
        assert_eq!(c.render(1.0), None, "no texture, nothing drawn");
    }

    #[test]
    fn quads_cover_the_screen() {
        let d = CloudDraw { colour: [1.0; 3], layers: [Layer { u: 0.0, v: 0.5, du: 0.0, dv: 0.0, size: 0.2 }; 2] };
        let q = quads(&d, 640, 480);
        assert_eq!(q.len(), 12);
        assert!(q[0].position[0] < -0.99 && q[0].position[1] > 0.99);
        assert!(q[2].position[0] > 1.0 && q[2].position[1] < -1.0);
        assert_eq!(q[2].uv, [0.2, 0.7]);
    }
}
