//! The wgpu renderer. It owns pipelines and per-frame buffers but no window: callers hand it a
//! device and queue and render into any texture view of the configured colour format.
//!
//! Ships keep their mesh objects separate (vertices in object space) so every object can take a
//! per-frame matrix: rest pose, `.mad` animation, or effects that move parts. Each uploaded ship
//! may hold several levels of detail. [`Renderer::render`] draws a [`Scene`] the way the first
//! version of this renderer did; [`Renderer::render_ex`] adds per-instance render states (LOD,
//! pose, Cataclysm's special effects) and an [`EffectBatch`].

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec2, Vec3, Vec4};
use wgpu::util::DeviceExt;

use crate::assets::{AssetSource, ShipModel, TeamColors, TeamScheme};
use crate::background::Background;
use crate::background::SkyVertex;
use crate::camera::OrbitCamera;
use crate::effects::{
    Blend, EffectBatch, EffectCamera, EffectGpuVertex, EffectTexture, LineGpuVertex, Strip,
    GLOW_MESH_NAMES,
};
use crate::lighting::SceneLighting;
use crate::lod::LodTable;
use crate::mesh::{build_level_geometry_detailed, LevelGeometry, ShipVertex, Skeleton, TintVertex};
use crate::render_state::{plane_to_world, PhaseStage, ShipRenderState};

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const OBJECT_STRIDE: u64 = 256;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct FrameUniform {
    view: [[f32; 4]; 4],
    proj: [[f32; 4]; 4],
    sky_view_proj: [[f32; 4]; 4],
    global_ambient: [f32; 4],
    light_dir: [[f32; 4]; 2],
    light_ambient: [[f32; 4]; 2],
    light_diffuse: [[f32; 4]; 2],
    params: [f32; 4],
    inv_view: [[f32; 4]; 4],
    /// [`ShipLook::uniform`](crate::look::ShipLook).
    look: [[f32; 4]; 2],
    /// The sky's light from +X, -X, +Y, -Y, +Z and -Z ([`Background::ambient_cube`]).
    sky: [[f32; 4]; 6],
    /// World to the shadow map's clip space.
    shadow_view_proj: [[f32; 4]; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_model: [[f32; 4]; 4],
    clip_plane: [f32; 4],
    /// Fade, tint enabled, clip enabled, glow exponent.
    params: [f32; 4],
    tint: [f32; 4],
    engine_glow: [f32; 4],
    team: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct MaterialUniform {
    ambient: [f32; 4],
    diffuse: [f32; 4],
    flags: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SpriteVertex {
    centre: [f32; 3],
    corner: [f32; 2],
    uv: [f32; 2],
    color: [f32; 4],
    mode: [f32; 2],
    /// World-space clip plane of a billboard (used when `mode[1]` is 1).
    plane: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GlowVertex {
    position: [f32; 3],
    normal: [f32; 3],
}

/// Team colouring applied when a ship is uploaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TeamPaint<'a> {
    /// Textures as stored, without team colours.
    None,
    Scheme(&'a TeamColors, TeamScheme),
}

struct GpuBatch {
    object: usize,
    first_vertex: u32,
    vertex_count: u32,
    material: usize,
    two_sided: bool,
    blended: bool,
}

/// One level of detail of a ship on the GPU.
struct GpuLevel {
    vertices: wgpu::Buffer,
    tints: wgpu::Buffer,
    batches: Vec<GpuBatch>,
    materials: Vec<wgpu::BindGroup>,
    skeleton: Skeleton,
    edges: Vec<(usize, Vec3, Vec3)>,
    triangle_count: usize,
}

/// A ship's meshes (one or more levels of detail), materials and nav lights on the GPU.
pub struct GpuShip {
    levels: Vec<GpuLevel>,
    nav_lights: Vec<crate::assets::NavLightDef>,
    /// Bind group per nav light texture name.
    nav_textures: Vec<(String, wgpu::BindGroup, bool)>,
    /// Triangles in the most detailed level.
    pub triangle_count: usize,
    lod_table: Option<LodTable>,
    bounds: (Vec3, f32),
}

impl GpuShip {
    pub fn level_count(&self) -> usize {
        self.levels.len()
    }

    /// The object hierarchy of a level (clamped to the levels present).
    pub fn skeleton(&self, level: usize) -> &Skeleton {
        &self.levels[level.min(self.levels.len() - 1)].skeleton
    }

    pub fn level_triangle_count(&self, level: usize) -> usize {
        self.levels[level.min(self.levels.len() - 1)].triangle_count
    }

    pub fn lod_table(&self) -> Option<&LodTable> {
        self.lod_table.as_ref()
    }

    /// Mesh-space bounding sphere of the most detailed level.
    pub fn bounds(&self) -> (Vec3, f32) {
        self.bounds
    }
}

struct StarBatch {
    bind_group: wgpu::BindGroup,
    first_vertex: u32,
    vertex_count: u32,
}

/// A background on the GPU.
pub struct GpuBackground {
    clear_color: [f32; 3],
    /// The sky's light for the modern look ([`Background::ambient_cube`]).
    ambient: [[f32; 4]; 6],
    sky: Option<(wgpu::Buffer, u32)>,
    stars: Option<wgpu::Buffer>,
    star_batches: Vec<StarBatch>,
}

/// One ship instance to draw: its GPU data and its mesh-to-world matrix (see
/// [`crate::mesh::mesh_to_world`] for the default orientation).
pub struct ShipInstance<'a> {
    pub ship: &'a GpuShip,
    pub transform: Mat4,
    /// Seconds since the ship appeared, for nav light blinking.
    pub age: f32,
}

/// What to draw this frame.
pub struct Scene<'a> {
    pub camera: &'a OrbitCamera,
    pub lighting: &'a SceneLighting,
    pub background: Option<&'a GpuBackground>,
    pub ships: &'a [ShipInstance<'a>],
    pub nav_lights: bool,
}

/// Additions to a [`Scene`] for [`Renderer::render_ex`].
#[derive(Clone, Copy, Default)]
pub struct SceneExtras<'a> {
    /// Per-ship render state, index-matched with [`Scene::ships`]; missing entries use
    /// [`ShipRenderState::default`].
    pub ship_states: &'a [ShipRenderState<'a>],
    pub effects: Option<&'a EffectBatch>,
}

/// Assets for engine glows and trails, loaded by [`Renderer::load_engine_effects`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EngineEffectAssets {
    /// `etg\textures\glow32`, the nozzle glow sprite ([`EffectTexture::WHITE`] if missing).
    pub glow_texture: EffectTexture,
    /// How many of the 21 glow meshes loaded.
    pub glow_meshes: usize,
}

/// Growable vertex buffer reused between frames.
struct DynamicBuffer {
    buffer: Option<(wgpu::Buffer, u64)>,
    label: &'static str,
}

impl DynamicBuffer {
    const fn new(label: &'static str) -> Self {
        Self {
            buffer: None,
            label,
        }
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let size = bytes.len() as u64;
        if self.buffer.as_ref().is_none_or(|(_, cap)| *cap < size) {
            let cap = size.next_power_of_two();
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buffer = Some((buffer, cap));
        }
        if let Some((buffer, _)) = &self.buffer {
            queue.write_buffer(buffer, 0, bytes);
        }
    }

    fn buffer(&self) -> Option<&wgpu::Buffer> {
        self.buffer.as_ref().map(|(b, _)| b)
    }
}

pub struct Renderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    color_format: wgpu::TextureFormat,
    frame_buffer: wgpu::Buffer,
    frame_bind_group: wgpu::BindGroup,
    object_layout: wgpu::BindGroupLayout,
    object_buffer: wgpu::Buffer,
    object_bind_group: wgpu::BindGroup,
    object_capacity: u64,
    material_layout: wgpu::BindGroupLayout,
    sprite_layout: wgpu::BindGroupLayout,
    linear_sampler: wgpu::Sampler,
    nearest_sampler: wgpu::Sampler,
    /// Trilinear, anisotropic: hull textures uploaded with mipmaps ([`ShipLook::filtering`]).
    mip_sampler: wgpu::Sampler,
    /// The ship look new uploads and frames use ([`Renderer::set_ship_look`]).
    ship_look: crate::look::ShipLook,
    /// Level geometry already worked out for a look (the baked occlusion is slow
    /// enough to do once per level, not once per team colour).
    geometry_cache: std::sync::Mutex<std::collections::HashMap<String, std::sync::Arc<LevelGeometry>>>,
    /// The modern look's shadow map.
    shadows: crate::shadow::Shadows,
    white: wgpu::TextureView,
    kit: PipelineKit,
    pipes: Pipelines,
    /// MSAA samples the pipelines are built for (1: none).
    sample_count: u32,
    /// Sample counts the adapter supports for the colour and depth formats.
    msaa_support: Vec<u32>,
    nav_buffer: DynamicBuffer,
    effect_buffer: DynamicBuffer,
    screen_buffer: DynamicBuffer,
    line_buffer: DynamicBuffer,
    pixel_buffer: DynamicBuffer,
    effect_textures: Vec<wgpu::BindGroup>,
    glow_meshes: Vec<Option<(wgpu::Buffer, u32)>>,
    /// GPU timing of offscreen renders ([`Renderer::set_gpu_timing`]).
    timer: Option<GpuTimer>,
    /// GPU milliseconds of the last offscreen render, when timed.
    last_gpu_ms: Option<f64>,
    /// The background's colour multiplier ([`Renderer::set_sky_brightness`]).
    sky_brightness: f32,
}

/// Timestamp queries around an offscreen render's command encoder.
struct GpuTimer {
    queries: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// Nanoseconds per timestamp tick.
    period: f32,
}

impl GpuTimer {
    /// Needs `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS`.
    fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<GpuTimer> {
        let needed = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        if !device.features().contains(needed) {
            return None;
        }
        let queries = device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu timer"),
            ty: wgpu::QueryType::Timestamp,
            count: 2,
        });
        let buffer = |usage, label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: 16,
                usage,
                mapped_at_creation: false,
            })
        };
        Some(GpuTimer {
            queries,
            resolve: buffer(
                wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
                "gpu timer resolve",
            ),
            read: buffer(
                wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                "gpu timer read",
            ),
            period: queue.get_timestamp_period(),
        })
    }
}

/// What the scene pipelines are built from, kept so they can be rebuilt for another sample
/// count ([`Renderer::set_sample_count`]).
struct PipelineKit {
    shader: wgpu::ShaderModule,
    ship_layout: wgpu::PipelineLayout,
    glow_layout: wgpu::PipelineLayout,
    sky_layout: wgpu::PipelineLayout,
    sprite_pipeline_layout: wgpu::PipelineLayout,
    color_format: wgpu::TextureFormat,
}

/// The scene pipelines for one sample count.
struct Pipelines {
    /// Indexed by `two_sided as usize | (blended as usize) << 1`.
    ship_pipelines: Vec<wgpu::RenderPipeline>,
    sky_pipeline: wgpu::RenderPipeline,
    star_pipeline: wgpu::RenderPipeline,
    billboard_pipeline: wgpu::RenderPipeline,
    glow_pipeline: wgpu::RenderPipeline,
    /// Indexed by blend: additive, alpha.
    effect_pipelines: [wgpu::RenderPipeline; 2],
    screen_pipelines: [wgpu::RenderPipeline; 2],
    /// [`crate::effects::PixelTriangle`]s: alpha blended, the target's alpha kept.
    pixel_pipeline: wgpu::RenderPipeline,
    line_pipelines: [wgpu::RenderPipeline; 2],
}

impl PipelineKit {
    fn build(&self, device: &wgpu::Device, samples: u32) -> Pipelines {
        let shader = &self.shader;
        let color_format = self.color_format;
        let (ship_layout, glow_layout, sky_layout, sprite_pipeline_layout) = (
            &self.ship_layout,
            &self.glow_layout,
            &self.sky_layout,
            &self.sprite_pipeline_layout,
        );
        let multisample = wgpu::MultisampleState {
            count: samples,
            ..Default::default()
        };
        let ship_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ShipVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        };
        let tint_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TintVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![3 => Uint32x3],
        };
        let glow_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<GlowVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };
        let sky_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SkyVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3],
        };
        let sprite_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<SpriteVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32x2,
                5 => Float32x4
            ],
        };
        let effect_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<EffectGpuVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x4, 2 => Float32x2],
        };
        let line_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<LineGpuVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![
                0 => Float32x3, 1 => Float32x3, 2 => Float32x4, 3 => Float32x3
            ],
        };

        let alpha_blend = wgpu::BlendState::ALPHA_BLENDING;
        let additive = wgpu::BlendState {
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
        };
        let blends = [additive, alpha_blend];

        struct Spec<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            vs: &'a str,
            fs: &'a str,
            vertex: Vec<Option<wgpu::VertexBufferLayout<'a>>>,
            cull: Option<wgpu::Face>,
            blend: Option<wgpu::BlendState>,
            depth_test: bool,
            depth_write: bool,
        }
        let build = |s: Spec<'_>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(s.label),
                layout: Some(s.layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some(s.vs),
                    compilation_options: Default::default(),
                    buffers: &s.vertex,
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: s.cull,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(s.depth_write),
                    depth_compare: Some(if s.depth_test {
                        wgpu::CompareFunction::Less
                    } else {
                        wgpu::CompareFunction::Always
                    }),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(s.fs),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color_format,
                        blend: s.blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let ship_pipelines = (0..4)
            .map(|i| {
                let two_sided = i & 1 != 0;
                let blended = i & 2 != 0;
                build(Spec {
                    label: "ship",
                    layout: ship_layout,
                    vs: "vs_ship",
                    fs: "fs_ship",
                    vertex: vec![
                        Some(ship_vertex_layout.clone()),
                        Some(tint_vertex_layout.clone()),
                    ],
                    cull: (!two_sided).then_some(wgpu::Face::Back),
                    blend: blended.then_some(alpha_blend),
                    depth_test: true,
                    depth_write: true,
                })
            })
            .collect();
        let sky_pipeline = build(Spec {
            label: "sky",
            layout: sky_layout,
            vs: "vs_sky",
            fs: "fs_sky",
            vertex: vec![Some(sky_vertex_layout)],
            cull: None,
            blend: None,
            depth_test: false,
            depth_write: false,
        });
        let star_pipeline = build(Spec {
            label: "stars",
            layout: sprite_pipeline_layout,
            vs: "vs_star",
            fs: "fs_sprite",
            vertex: vec![Some(sprite_vertex_layout.clone())],
            cull: None,
            blend: Some(additive),
            depth_test: false,
            depth_write: false,
        });
        let billboard_pipeline = build(Spec {
            label: "billboards",
            layout: sprite_pipeline_layout,
            vs: "vs_billboard",
            fs: "fs_sprite",
            vertex: vec![Some(sprite_vertex_layout)],
            cull: None,
            blend: Some(additive),
            depth_test: true,
            depth_write: false,
        });
        let glow_pipeline = build(Spec {
            label: "engine glow",
            layout: glow_layout,
            vs: "vs_glow",
            fs: "fs_glow",
            vertex: vec![Some(glow_vertex_layout)],
            cull: Some(wgpu::Face::Back),
            blend: Some(additive),
            depth_test: true,
            depth_write: false,
        });
        let effect_pipelines = blends.map(|blend| {
            build(Spec {
                label: "effects",
                layout: sprite_pipeline_layout,
                vs: "vs_effect",
                fs: "fs_effect",
                vertex: vec![Some(effect_vertex_layout.clone())],
                cull: None,
                blend: Some(blend),
                depth_test: true,
                depth_write: false,
            })
        });
        let screen_pipelines = blends.map(|blend| {
            build(Spec {
                label: "screen effects",
                layout: sprite_pipeline_layout,
                vs: "vs_screen",
                fs: "fs_effect",
                vertex: vec![Some(effect_vertex_layout.clone())],
                cull: None,
                blend: Some(blend),
                depth_test: false,
                depth_write: false,
            })
        });
        let pixel_pipeline = build(Spec {
            label: "pixel triangles",
            layout: sprite_pipeline_layout,
            vs: "vs_screen",
            fs: "fs_pixel",
            vertex: vec![Some(effect_vertex_layout.clone())],
            cull: None,
            blend: Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::SrcAlpha,
                    dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::Zero,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
            }),
            depth_test: false,
            depth_write: false,
        });
        let line_pipelines = blends.map(|blend| {
            build(Spec {
                label: "lines",
                layout: sprite_pipeline_layout,
                vs: "vs_line",
                fs: "fs_effect",
                vertex: vec![Some(line_vertex_layout.clone())],
                cull: None,
                blend: Some(blend),
                depth_test: true,
                depth_write: false,
            })
        });

        Pipelines {
            ship_pipelines,
            sky_pipeline,
            star_pipeline,
            billboard_pipeline,
            glow_pipeline,
            effect_pipelines,
            screen_pipelines,
            pixel_pipeline,
            line_pipelines,
        }
    }
}

fn mat(m: Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
}

fn blend_index(b: Blend) -> usize {
    match b {
        Blend::Additive => 0,
        Blend::Alpha => 1,
    }
}

/// Whether a nav light is lit `t` seconds after its ship appeared: dark for the start delay,
/// then on for `flash_on` seconds and off for `flash_off`, repeating.
pub fn nav_light_on(t: f32, flash_on: f32, flash_off: f32, start_delay: f32) -> bool {
    if t <= start_delay {
        return false;
    }
    let period = flash_on + flash_off;
    if period <= 0.0 {
        return true;
    }
    (t - start_delay).rem_euclid(period) < flash_on
}

/// Per-object shader parameters shared by every object of one drawn instance.
#[derive(Clone, Copy)]
struct InstanceParams {
    clip_plane: Option<Vec4>,
    untextured: bool,
    fade: f32,
    tint_enabled: bool,
    tint: [f32; 4],
    engine_glow: [f32; 4],
    team: [f32; 4],
    exponent: f32,
    /// The still-to-be-built part of a hull under construction (blue ambient).
    ghost: bool,
}

impl Default for InstanceParams {
    fn default() -> Self {
        Self {
            clip_plane: None,
            untextured: false,
            fade: 1.0,
            tint_enabled: false,
            tint: [0.0; 4],
            engine_glow: [0.0; 4],
            team: [1.0; 4],
            exponent: 1.0,
            ghost: false,
        }
    }
}

fn object_uniform(model: Mat4, p: &InstanceParams) -> ObjectUniform {
    let normal = Mat4::from_mat3(Mat3::from_mat4(model).inverse().transpose());
    ObjectUniform {
        model: mat(model),
        normal_model: mat(normal),
        clip_plane: p.clip_plane.unwrap_or(Vec4::ZERO).to_array(),
        params: [
            p.fade,
            f32::from(u8::from(p.tint_enabled)),
            f32::from(u8::from(p.clip_plane.is_some())),
            p.exponent,
        ],
        tint: [p.tint[0], p.tint[1], f32::from(u8::from(p.ghost)), f32::from(u8::from(p.untextured))],
        engine_glow: p.engine_glow,
        team: p.team,
    }
}

/// One ship mesh pass to draw this frame.
struct DrawItem<'s> {
    ship: &'s GpuShip,
    level: usize,
    /// Uniform slot of the first object.
    first_slot: u32,
    transparent: bool,
    /// Draws every batch without back-face culling (hulls under construction are open).
    two_sided: bool,
    /// Squared distance from the eye, for sorting the transparent list.
    depth: f32,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        color_format: wgpu::TextureFormat,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hwc shaders"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });

        let uniform_entry = |binding, visibility, dynamic| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: dynamic,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let vf = wgpu::ShaderStages::VERTEX_FRAGMENT;

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("frame"),
            entries: &[uniform_entry(0, vf, false)],
        });
        let material_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("material"),
            entries: &[
                uniform_entry(0, vf, false),
                texture_entry(1),
                sampler_entry(2),
            ],
        });
        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
            entries: &[uniform_entry(0, vf, true)],
        });
        let sprite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("sprite"),
            entries: &[texture_entry(0), sampler_entry(1)],
        });

        let frame_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let frame_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("frame"),
            layout: &frame_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: frame_buffer.as_entire_binding(),
            }],
        });
        let object_capacity = 64;
        let (object_buffer, object_bind_group) =
            Self::object_storage(device, &object_layout, object_capacity);

        let sampler = |filter| {
            device.create_sampler(&wgpu::SamplerDescriptor {
                label: None,
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: filter,
                min_filter: filter,
                ..Default::default()
            })
        };
        let linear_sampler = sampler(wgpu::FilterMode::Linear);
        let nearest_sampler = sampler(wgpu::FilterMode::Nearest);
        let mip_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hull mips"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let white = Self::make_texture(device, queue, 1, 1, &[255; 4], "white");

        let mut shadows = crate::shadow::Shadows::new(device);
        shadows.build_pipeline(
            device,
            &shader,
            &frame_layout,
            &object_layout,
            std::mem::size_of::<ShipVertex>() as u64,
        );
        let ship_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ship"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&material_layout),
                Some(&object_layout),
                Some(&shadows.layout),
            ],
            immediate_size: 0,
        });
        let glow_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("glow"),
            bind_group_layouts: &[Some(&frame_layout), None, Some(&object_layout)],
            immediate_size: 0,
        });
        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[Some(&frame_layout)],
            immediate_size: 0,
        });
        let sprite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("sprite"),
                bind_group_layouts: &[Some(&frame_layout), None, None, Some(&sprite_layout)],
                immediate_size: 0,
            });

        let kit = PipelineKit {
            shader,
            ship_layout,
            glow_layout,
            sky_layout,
            sprite_pipeline_layout,
            color_format,
        };
        let pipes = kit.build(device, 1);
        let mut renderer = Self {
            device: device.clone(),
            queue: queue.clone(),
            color_format,
            frame_buffer,
            frame_bind_group,
            object_layout,
            object_buffer,
            object_bind_group,
            object_capacity,
            material_layout,
            sprite_layout,
            linear_sampler,
            nearest_sampler,
            mip_sampler,
            ship_look: crate::look::ShipLook::original(),
            geometry_cache: Default::default(),
            shadows,
            white,
            kit,
            pipes,
            sample_count: 1,
            msaa_support: vec![1, 4],
            nav_buffer: DynamicBuffer::new("nav lights"),
            effect_buffer: DynamicBuffer::new("effects"),
            screen_buffer: DynamicBuffer::new("screen effects"),
            line_buffer: DynamicBuffer::new("lines"),
            pixel_buffer: DynamicBuffer::new("pixel triangles"),
            effect_textures: Vec::new(),
            glow_meshes: Vec::new(),
            timer: None,
            last_gpu_ms: None,
            sky_brightness: 1.0,
        };
        let white = renderer.sprite_bind_group(&renderer.white, &renderer.linear_sampler);
        renderer.effect_textures.push(white);
        renderer
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    /// Times each offscreen render on the GPU with timestamp queries (for profiling). Returns
    /// whether the device can (it needs `TIMESTAMP_QUERY` and
    /// `TIMESTAMP_QUERY_INSIDE_ENCODERS`, which [`request_device`] asks for when the adapter
    /// has them).
    pub fn set_gpu_timing(&mut self, on: bool) -> bool {
        self.timer = if on {
            self.timer.take().or_else(|| GpuTimer::new(&self.device, &self.queue))
        } else {
            None
        };
        self.last_gpu_ms = None;
        self.timer.is_some()
    }

    /// GPU milliseconds of the last offscreen render (drawing, resolve, post passes and the
    /// copy for the read-back), when [`set_gpu_timing`](Self::set_gpu_timing) is on; taken
    /// once.
    pub fn take_gpu_ms(&mut self) -> Option<f64> {
        self.last_gpu_ms.take()
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Scales the sky background's colours and stars (`btgSetColourMultiplier`, which the
    /// plasma cloud uses to fade the sky out under its fog; [`crate::PlasmaCloud::sky`]).
    /// 1 draws the sky as it is.
    pub fn set_sky_brightness(&mut self, m: f32) {
        self.sky_brightness = m.clamp(0.0, 1.0);
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
    }

    /// MSAA samples per pixel the scene pipelines draw with (1: no multisampling). Targets
    /// handed to [`render_ex`](Self::render_ex), [`crate::EffectRenderer::draw`] and
    /// [`crate::NebulaRenderer::draw`] must have this sample count.
    pub fn sample_count(&self) -> u32 {
        self.sample_count
    }

    /// The sample counts the device can render the scene with (always contains 1). Without
    /// [`set_msaa_support`](Self::set_msaa_support) this is WebGPU's guaranteed `[1, 4]`.
    pub fn msaa_support(&self) -> &[u32] {
        &self.msaa_support
    }

    /// Records the sample counts the adapter supports (see [`supported_sample_counts`]).
    pub fn set_msaa_support(&mut self, counts: &[u32]) {
        let mut v: Vec<u32> = counts.iter().copied().filter(|n| n.is_power_of_two()).collect();
        v.push(1);
        v.sort_unstable();
        v.dedup();
        self.msaa_support = v;
    }

    /// Rebuilds the scene pipelines for `samples` per pixel, the nearest supported count at
    /// or below it; returns the count used.
    pub fn set_sample_count(&mut self, samples: u32) -> u32 {
        let n = crate::quality::pick_sample_count(samples, &self.msaa_support);
        if n != self.sample_count {
            self.pipes = self.kit.build(&self.device, n);
            self.sample_count = n;
        }
        n
    }

    /// `model`'s geometry for `detail`, worked out once.
    fn level_geometry(&self, model: &ShipModel, detail: crate::mesh::Detail) -> std::sync::Arc<LevelGeometry> {
        let mut key = format!("{detail:?}");
        for p in &model.parts {
            key.push('|');
            key.push_str(&p.path);
            // Parts placed differently (mothership modules) are different geometry.
            let placed: f64 = p
                .geo
                .objects
                .iter()
                .flat_map(|o| o.local_matrix)
                .enumerate()
                .map(|(i, v)| f64::from(v) * (i % 7 + 1) as f64)
                .sum();
            key.push_str(&format!("@{placed}"));
        }
        if let Some(g) = self.geometry_cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
            return g;
        }
        let g = std::sync::Arc::new(build_level_geometry_detailed(model, detail));
        if let Ok(mut c) = self.geometry_cache.lock() {
            c.insert(key, g.clone());
        }
        g
    }

    /// The ship look: textures and smoothed normals apply to ships uploaded after this call,
    /// the per-pixel lighting, highlight and bump to the next frame.
    pub fn set_ship_look(&mut self, look: crate::look::ShipLook) {
        self.ship_look = look;
    }

    pub fn ship_look(&self) -> crate::look::ShipLook {
        self.ship_look
    }

    fn object_storage(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u64,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: capacity * OBJECT_STRIDE,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(std::mem::size_of::<ObjectUniform>() as u64),
                }),
            }],
        });
        (buffer, bind_group)
    }

    fn make_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
        label: &str,
    ) -> wgpu::TextureView {
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
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
        texture.create_view(&Default::default())
    }

    /// An RGBA8 texture with its whole mip chain ([`crate::look::mip_chain`]).
    fn make_mipped_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        rgba: &[u8],
        label: &str,
    ) -> wgpu::TextureView {
        let (width, height) = (width.max(1), height.max(1));
        let chain = crate::look::mip_chain(width, height, rgba);
        let mut data = rgba.to_vec();
        for (_, _, level) in &chain {
            data.extend_from_slice(level);
        }
        let texture = device.create_texture_with_data(
            queue,
            &wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: chain.len() as u32 + 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &data,
        );
        texture.create_view(&Default::default())
    }

    fn sprite_bind_group(
        &self,
        view: &wgpu::TextureView,
        sampler: &wgpu::Sampler,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("sprite"),
            layout: &self.sprite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        })
    }

    /// Registers an RGBA8 texture (top-down rows) for effect primitives.
    pub fn create_effect_texture(&mut self, width: u32, height: u32, rgba: &[u8]) -> EffectTexture {
        let view = Self::make_texture(&self.device, &self.queue, width, height, rgba, "effect");
        let bg = self.sprite_bind_group(&view, &self.linear_sampler);
        self.effect_textures.push(bg);
        EffectTexture(self.effect_textures.len() as u32 - 1)
    }

    /// Loads the nozzle glow sprite and the `misc\EngineGlows` meshes.
    pub fn load_engine_effects(&mut self, src: &dyn AssetSource) -> EngineEffectAssets {
        let glow_texture = crate::assets::load_lif(src, "etg\\textures\\glow32.lif")
            .map(|lif| {
                // Bitmaps without an alpha channel upload as opaque RGB.
                let mut rgba = lif.to_rgba();
                if !lif.has_alpha() {
                    rgba.chunks_mut(4).for_each(|p| p[3] = 255);
                }
                self.create_effect_texture(lif.width, lif.height, &rgba)
            })
            .unwrap_or(EffectTexture::WHITE);
        self.glow_meshes = GLOW_MESH_NAMES
            .iter()
            .map(|name| {
                let data = src.read(&format!("misc\\engineglows\\{name}.geo"))?;
                let geo = hwc_formats::geo::Geo::parse(&data).ok()?;
                let mats = crate::assets::object_matrices(&geo);
                let mut verts = Vec::new();
                for (o, m) in geo.objects.iter().zip(&mats) {
                    let nm = Mat3::from_mat4(*m).inverse().transpose();
                    for p in &o.polygons {
                        let face = o.normals.get(p.face_normal as usize).copied();
                        for &vi in &p.vertices {
                            let Some(v) = o.vertices.get(vi as usize) else {
                                continue;
                            };
                            let n = o
                                .normals
                                .get(v.normal as usize)
                                .copied()
                                .or(face)
                                .unwrap_or([0.0, 0.0, 1.0]);
                            verts.push(GlowVertex {
                                position: m.transform_point3(Vec3::from_array(v.position)).into(),
                                normal: (nm * Vec3::from_array(n)).normalize_or_zero().into(),
                            });
                        }
                    }
                }
                let count = verts.len() - verts.len() % 3;
                (count > 0).then(|| {
                    let buffer =
                        self.device
                            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                                label: Some(name),
                                contents: bytemuck::cast_slice(&verts[..count]),
                                usage: wgpu::BufferUsages::VERTEX,
                            });
                    (buffer, count as u32)
                })
            })
            .collect();
        EngineEffectAssets {
            glow_texture,
            glow_meshes: self.glow_meshes.iter().flatten().count(),
        }
    }

    fn upload_level(&self, model: &ShipModel, paint: TeamPaint<'_>) -> GpuLevel {
        let look = self.ship_look;
        let geometry = self.level_geometry(model, look.detail());
        let (verts, tints) = if geometry.vertices.is_empty() {
            (
                vec![ShipVertex {
                    position: [0.0; 3],
                    normal: [0.0; 3],
                    uv: [0.0; 2],
                }],
                vec![TintVertex::default()],
            )
        } else {
            (geometry.vertices.clone(), geometry.tints.clone())
        };
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&model.name),
                contents: bytemuck::cast_slice(&verts),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let tints = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("tints"),
                contents: bytemuck::cast_slice(&tints),
                usage: wgpu::BufferUsages::VERTEX,
            });

        // One bind group per (part, material), with the texture coloured for the team.
        let mut materials = Vec::new();
        let mut material_index: Vec<Vec<usize>> = Vec::new();
        for (part, infos) in model.parts.iter().zip(&geometry.materials) {
            let mut row = Vec::new();
            for (mi, info) in infos.iter().enumerate() {
                let view = part.textures.get(mi).and_then(Option::as_ref).map(|lif| {
                    let rgba = match paint {
                        TeamPaint::None => lif.to_rgba(),
                        TeamPaint::Scheme(_, s) => lif.to_rgba_team(s.base, s.stripe),
                    };
                    let make = if look.filtering {
                        Self::make_mipped_texture
                    } else {
                        Self::make_texture
                    };
                    make(&self.device, &self.queue, lif.width, lif.height, &rgba, &part.path)
                });
                let sampler = if look.filtering && view.is_some() {
                    &self.mip_sampler
                } else {
                    &self.linear_sampler
                };
                let (mut ambient, mut diffuse) = (info.ambient, info.diffuse);
                if info.team_colored() {
                    if let TeamPaint::Scheme(colors, scheme) = paint {
                        (ambient, diffuse) = colors.material_colors(&scheme);
                    }
                }
                let uniform = MaterialUniform {
                    ambient: [ambient[0], ambient[1], ambient[2], 1.0],
                    diffuse: [diffuse[0], diffuse[1], diffuse[2], 1.0],
                    flags: [
                        f32::from(u8::from(info.self_illuminated())),
                        f32::from(u8::from(info.textured)),
                        0.0,
                        0.0,
                    ],
                };
                let buffer = self
                    .device
                    .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some("material"),
                        contents: bytemuck::bytes_of(&uniform),
                        usage: wgpu::BufferUsages::UNIFORM,
                    });
                let view = view.as_ref().unwrap_or(&self.white);
                let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: Some("material"),
                    layout: &self.material_layout,
                    entries: &[
                        wgpu::BindGroupEntry {
                            binding: 0,
                            resource: buffer.as_entire_binding(),
                        },
                        wgpu::BindGroupEntry {
                            binding: 1,
                            resource: wgpu::BindingResource::TextureView(view),
                        },
                        wgpu::BindGroupEntry {
                            binding: 2,
                            resource: wgpu::BindingResource::Sampler(sampler),
                        },
                    ],
                });
                row.push(materials.len());
                materials.push(bind_group);
            }
            material_index.push(row);
        }

        let batches = geometry
            .batches
            .iter()
            .map(|b| {
                let info = &geometry.materials[b.part][b.material];
                GpuBatch {
                    object: b.object,
                    first_vertex: b.first_vertex,
                    vertex_count: b.vertex_count,
                    material: material_index[b.part][b.material],
                    two_sided: info.two_sided(),
                    blended: info.alpha,
                }
            })
            .collect();
        GpuLevel {
            vertices,
            tints,
            batches,
            materials,
            triangle_count: geometry.vertices.len() / 3,
            skeleton: geometry.skeleton.clone(),
            edges: geometry.edges.clone(),
        }
    }

    /// Uploads a ship with the given team colouring (one level of detail).
    pub fn upload_ship(&self, model: &ShipModel, paint: TeamPaint<'_>) -> GpuShip {
        self.upload_ship_lods(std::slice::from_ref(model), None, paint)
    }

    /// Uploads every level of detail of a ship (most detailed first) with its `.lod` table.
    /// Nav lights come from the first level.
    ///
    /// # Panics
    /// If `levels` is empty.
    pub fn upload_ship_lods(
        &self,
        levels: &[ShipModel],
        table: Option<LodTable>,
        paint: TeamPaint<'_>,
    ) -> GpuShip {
        assert!(!levels.is_empty(), "a ship needs at least one level");
        let model = &levels[0];
        let gpu_levels: Vec<GpuLevel> =
            levels.iter().map(|m| self.upload_level(m, paint)).collect();
        let nav_textures = model
            .nav_textures
            .iter()
            .map(|(name, lif)| {
                let view = Self::make_texture(
                    &self.device,
                    &self.queue,
                    lif.width,
                    lif.height,
                    &lif.to_rgba(),
                    name,
                );
                (
                    name.clone(),
                    self.sprite_bind_group(&view, &self.linear_sampler),
                    lif.has_alpha(),
                )
            })
            .collect();
        GpuShip {
            triangle_count: gpu_levels[0].triangle_count,
            levels: gpu_levels,
            nav_lights: model.nav_lights.clone(),
            nav_textures,
            lod_table: table,
            bounds: model.bounds(),
        }
    }

    pub fn upload_background(&self, bg: &Background) -> GpuBackground {
        let sky = (!bg.vertices.is_empty()).then(|| {
            let buffer = self
                .device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("sky"),
                    contents: bytemuck::cast_slice(&bg.vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                });
            (buffer, bg.vertices.len() as u32)
        });
        let mut star_vertices: Vec<SpriteVertex> = Vec::new();
        let mut star_batches = Vec::new();
        for group in &bg.star_groups {
            let view = Self::make_texture(
                &self.device,
                &self.queue,
                group.image.width,
                group.image.height,
                &group.image.rgba,
                &group.texture,
            );
            let first = star_vertices.len() as u32;
            let (w, h) = (group.image.width as f32, group.image.height as f32);
            for (dir, tint) in &group.stars {
                push_quad(
                    &mut star_vertices,
                    dir.to_array(),
                    [w / 2.0, h / 2.0],
                    true,
                    [tint[0], tint[1], tint[2], 1.0],
                    1.0,
                    None,
                );
            }
            star_batches.push(StarBatch {
                bind_group: self.sprite_bind_group(&view, &self.nearest_sampler),
                first_vertex: first,
                vertex_count: star_vertices.len() as u32 - first,
            });
        }
        let stars = (!star_vertices.is_empty()).then(|| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("stars"),
                    contents: bytemuck::cast_slice(&star_vertices),
                    usage: wgpu::BufferUsages::VERTEX,
                })
        });
        GpuBackground {
            clear_color: bg.clear_color,
            ambient: bg.ambient_cube().map(|c| [c[0], c[1], c[2], 0.0]),
            sky,
            stars,
            star_batches,
        }
    }

    /// A depth buffer matching a single-sampled colour target.
    pub fn create_depth(&self, width: u32, height: u32) -> wgpu::TextureView {
        self.create_depth_samples(width, height, 1)
    }

    /// A depth buffer with `samples` per pixel.
    pub fn create_depth_samples(&self, width: u32, height: u32, samples: u32) -> wgpu::TextureView {
        self.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default())
    }

    /// Records the scene into `encoder`, clearing `color` and `depth` (both `width` × `height`).
    pub fn render(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        size: (u32, u32),
        scene: &Scene<'_>,
    ) {
        self.render_ex(encoder, color, depth, size, scene, &SceneExtras::default());
    }

    /// [`render`](Self::render) with per-instance render states and effect primitives.
    pub fn render_ex(
        &mut self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        (width, height): (u32, u32),
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
    ) {
        let aspect = width as f32 / height.max(1) as f32;
        let cam = scene.camera;
        let proj = cam.projection(aspect);
        let view = cam.view();
        let inv_view = view.inverse();
        let eye = cam.eye();
        let lights = &scene.lighting.lights;
        let light = |i: usize| lights.get(i).copied();
        let v4 = |v: Vec3| [v.x, v.y, v.z, 0.0];
        let mut look = self.ship_look.uniform();
        let shadows_on = look[1][1] > 0.5 && !lights.is_empty();
        let (shadow_vp, texel) = match light(0) {
            Some(l) if shadows_on => crate::shadow::shadow_matrix(cam, l.direction),
            _ => (Mat4::IDENTITY, 0.0),
        };
        look[1][1] = f32::from(u8::from(shadows_on));
        look[1][3] = texel;
        let frame = FrameUniform {
            view: mat(view),
            proj: mat(proj),
            sky_view_proj: mat(proj * cam.view_rotation()),
            global_ambient: v4(scene.lighting.global_ambient),
            light_dir: [0, 1]
                .map(|i| light(i).map_or([0.0; 4], |l| v4(l.direction.normalize_or_zero()))),
            light_ambient: [0, 1].map(|i| light(i).map_or([0.0; 4], |l| v4(l.ambient))),
            light_diffuse: [0, 1].map(|i| light(i).map_or([0.0; 4], |l| v4(l.diffuse))),
            params: [lights.len().min(2) as f32, width as f32, height as f32, self.sky_brightness],
            inv_view: mat(inv_view),
            look,
            sky: scene.background.map_or([[0.0; 4]; 6], |b| b.ambient),
            shadow_view_proj: mat(shadow_vp),
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));

        // ---- Ship passes and their per-object uniforms ----
        let default_state = ShipRenderState::default();
        let mut uniforms: Vec<ObjectUniform> = Vec::new();
        let mut items: Vec<DrawItem<'_>> = Vec::new();
        let mut internal = EffectBatch::default();
        #[allow(clippy::too_many_arguments)]
        fn push_item<'a>(
            uniforms: &mut Vec<ObjectUniform>,
            items: &mut Vec<DrawItem<'a>>,
            eye: Vec3,
            ship: &'a GpuShip,
            level: usize,
            transform: Mat4,
            objects: &[Mat4],
            params: InstanceParams,
            transparent: bool,
            two_sided: bool,
        ) {
            let first_slot = uniforms.len() as u32;
            for m in objects {
                uniforms.push(object_uniform(transform * *m, &params));
            }
            let centre = transform.transform_point3(ship.bounds.0);
            items.push(DrawItem {
                ship,
                level,
                first_slot,
                transparent,
                two_sided,
                depth: centre.distance_squared(eye),
            });
        }
        for (i, inst) in scene.ships.iter().enumerate() {
            let state = extras.ship_states.get(i).unwrap_or(&default_state);
            let ship = inst.ship;
            let level = state.lod.min(ship.levels.len() - 1);
            let skeleton = &ship.levels[level].skeleton;
            let pose = state.pose;
            let objects = skeleton.resolve(|j| {
                pose.and_then(|p| p.get(j).copied().flatten())
                    .unwrap_or(skeleton.objects[j].rest)
            });
            let tint = &state.tint;
            let base = InstanceParams {
                clip_plane: state.clip_plane.map(|p| plane_to_world(inst.transform, p)),
                untextured: state.untextured,
                fade: state.fade.clamp(0.0, 1.0),
                tint_enabled: tint.enabled,
                tint: [-0.15 * tint.glow.clamp(0.0, 1.0), tint.flash, 0.0, 0.0],
                engine_glow: tint.engine_glow,
                team: [
                    tint.team_color[0],
                    tint.team_color[1],
                    tint.team_color[2],
                    1.0,
                ],
                exponent: 1.0,
                ghost: false,
            };
            let transparent = state.is_transparent();
            let mut draw_main = true;
            if let Some((infected, progress)) = state.infection {
                let progress = progress.clamp(0.0, 1.0);
                draw_main = progress < 1.0;
                let lvl = level.min(infected.levels.len() - 1);
                let sk = &infected.levels[lvl].skeleton;
                let objs = sk.rest_matrices();
                let t = inst.transform * Mat4::from_scale(Vec3::splat(1.01));
                let p = InstanceParams {
                    fade: progress,
                    ..base
                };
                push_item(
                    &mut uniforms,
                    &mut items,
                    eye,
                    infected,
                    lvl,
                    t,
                    &objs,
                    p,
                    true,
                    false,
                );
            }
            if let Some(phase) = state.phase {
                let bx = phase.collision_box;
                let box_lines = |scale: f32, alpha: f32, out: &mut EffectBatch| {
                    let centre = bx.corner + bx.extent * 0.5;
                    let half = bx.extent * 0.5 * scale;
                    phase_box_lines(inst.transform, centre, half, alpha, out);
                };
                match phase.stage {
                    PhaseStage::In => box_lines(2.0 - phase.value, phase.value, &mut internal),
                    PhaseStage::Sweep => {
                        let plane = crate::render_state::phase_sweep_plane(&bx, phase.value);
                        if let Some(other) = phase.converted {
                            let lvl = level.min(other.levels.len() - 1);
                            let objs = other.levels[lvl].skeleton.rest_matrices();
                            let p = InstanceParams {
                                clip_plane: Some(plane_to_world(inst.transform, -plane)),
                                ..base
                            };
                            push_item(
                                &mut uniforms,
                                &mut items,
                                eye,
                                other,
                                lvl,
                                inst.transform,
                                &objs,
                                p,
                                true,
                                false,
                            );
                        }
                        let p = InstanceParams {
                            clip_plane: Some(plane_to_world(inst.transform, plane)),
                            ..base
                        };
                        push_item(
                            &mut uniforms,
                            &mut items,
                            eye,
                            ship,
                            level,
                            inst.transform,
                            &objects,
                            p,
                            true,
                            false,
                        );
                        draw_main = false;
                        box_lines(1.0, 252.0 / 255.0, &mut internal);
                    }
                    PhaseStage::Out => {
                        if let Some(other) = phase.converted {
                            let lvl = level.min(other.levels.len() - 1);
                            let objs = other.levels[lvl].skeleton.rest_matrices();
                            push_item(
                                &mut uniforms,
                                &mut items,
                                eye,
                                other,
                                lvl,
                                inst.transform,
                                &objs,
                                base,
                                true,
                                false,
                            );
                            draw_main = false;
                        }
                        box_lines(1.0 - phase.value * 0.25, 1.0 - phase.value, &mut internal);
                    }
                }
            }
            // External construction: two passes cut by one plane across the heading axis. The
            // built part draws solid, the rest as a quarter-opaque ghost with a blue ambient;
            // both with back faces, since the cut leaves the hull open.
            if let (true, Some(c)) = (draw_main, state.construction) {
                let (built, ghost) =
                    crate::render_state::construction_planes(&c.collision_box, c.progress);
                let solid = InstanceParams {
                    clip_plane: Some(plane_to_world(inst.transform, built)),
                    ..base
                };
                push_item(
                    &mut uniforms,
                    &mut items,
                    eye,
                    ship,
                    level,
                    inst.transform,
                    &objects,
                    solid,
                    state.transparent || base.fade < 1.0,
                    true,
                );
                if c.progress < 1.0 {
                    let rest = InstanceParams {
                        clip_plane: Some(plane_to_world(inst.transform, ghost)),
                        fade: base.fade * crate::render_state::CONSTRUCTION_GHOST_ALPHA,
                        ghost: true,
                        ..base
                    };
                    push_item(
                        &mut uniforms,
                        &mut items,
                        eye,
                        ship,
                        level,
                        inst.transform,
                        &objects,
                        rest,
                        true,
                        true,
                    );
                }
                draw_main = false;
            }
            if draw_main {
                push_item(
                    &mut uniforms,
                    &mut items,
                    eye,
                    ship,
                    level,
                    inst.transform,
                    &objects,
                    base,
                    transparent,
                    false,
                );
            }
            if state.emp_lightning {
                emp_lightning(
                    &ship.levels[level].edges,
                    &objects,
                    inst.transform,
                    state.time,
                    &mut internal,
                );
            }
        }

        // Effects: user batch plus internal effects.
        let mut effects = internal;
        if let Some(e) = extras.effects {
            effects.extend(e);
        }
        let mut glow_draws = Vec::new();
        for g in &effects.glow_meshes {
            let Some(Some((buffer, count))) = self.glow_meshes.get(g.mesh) else {
                continue;
            };
            let p = InstanceParams {
                team: g.color,
                exponent: g.exponent,
                ..InstanceParams::default()
            };
            glow_draws.push((buffer, *count, uniforms.len() as u32));
            uniforms.push(object_uniform(g.transform, &p));
        }
        let effect_cam = EffectCamera {
            eye,
            right: inv_view.x_axis.truncate().normalize_or_zero(),
            up: inv_view.y_axis.truncate().normalize_or_zero(),
            viewport: Vec2::new(width as f32, height as f32),
        };
        let tess = effects.tessellate(&effect_cam);

        // Upload uniforms in one go.
        let needed = (uniforms.len() as u64).max(1);
        if needed > self.object_capacity {
            self.object_capacity = needed.next_power_of_two();
            (self.object_buffer, self.object_bind_group) =
                Self::object_storage(&self.device, &self.object_layout, self.object_capacity);
        }
        if !uniforms.is_empty() {
            let mut bytes = vec![0u8; uniforms.len() * OBJECT_STRIDE as usize];
            for (i, u) in uniforms.iter().enumerate() {
                let at = i * OBJECT_STRIDE as usize;
                bytes[at..at + std::mem::size_of::<ObjectUniform>()]
                    .copy_from_slice(bytemuck::bytes_of(u));
            }
            self.queue.write_buffer(&self.object_buffer, 0, &bytes);
        }
        self.effect_buffer
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&tess.world));
        self.screen_buffer.upload(
            &self.device,
            &self.queue,
            bytemuck::cast_slice(&tess.screen),
        );
        self.line_buffer
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&tess.lines));
        self.pixel_buffer
            .upload(&self.device, &self.queue, bytemuck::cast_slice(&tess.pixel));

        // Nav lights: billboards built on the CPU each frame, grouped by texture. They are
        // drawn with the hull's render state (`RenderNAVLights` runs between `hsContinue` and
        // `hsEnd`, under the hyperspace clip plane, and with the hull's fade).
        let mut nav_vertices: Vec<SpriteVertex> = Vec::new();
        let mut nav_draws: Vec<(&wgpu::BindGroup, u32, u32)> = Vec::new();
        if scene.nav_lights {
            for (i, inst) in scene.ships.iter().enumerate() {
                let state = extras.ship_states.get(i).unwrap_or(&default_state);
                let plane = state.clip_plane.map(|p| plane_to_world(inst.transform, p));
                let fade = state.fade.clamp(0.0, 1.0);
                for (name, bind_group, has_alpha) in &inst.ship.nav_textures {
                    let first = nav_vertices.len() as u32;
                    for nav in &inst.ship.nav_lights {
                        if nav.texture.as_deref() != Some(name.as_str())
                            || !nav_light_on(inst.age, nav.flash_on, nav.flash_off, nav.start_delay)
                        {
                            continue;
                        }
                        let centre = inst.transform.transform_point3(nav.position);
                        let c = nav.color.map(|v| f32::from(v) / 255.0);
                        push_quad(
                            &mut nav_vertices,
                            centre.to_array(),
                            [nav.size / 2.0; 2],
                            false,
                            [c[0], c[1], c[2], fade],
                            if *has_alpha { 0.0 } else { 2.0 },
                            plane,
                        );
                    }
                    let count = nav_vertices.len() as u32 - first;
                    if count > 0 {
                        nav_draws.push((bind_group, first, count));
                    }
                }
            }
        }
        self.nav_buffer.upload(
            &self.device,
            &self.queue,
            bytemuck::cast_slice(&nav_vertices),
        );

        // The modern look's shadow map: the opaque ships' depth from the main light.
        if shadows_on {
            let map = self.shadows.ensure_map(&self.device).clone();
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &map,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if let Some(pipeline) = &self.shadows.pipeline {
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &self.frame_bind_group, &[]);
                for item in items.iter().filter(|i| !i.transparent) {
                    let level = &item.ship.levels[item.level];
                    pass.set_vertex_buffer(0, level.vertices.slice(..));
                    for b in level.batches.iter().filter(|b| !b.blended) {
                        let offset =
                            ((item.first_slot as usize + b.object) as u64 * OBJECT_STRIDE) as u32;
                        pass.set_bind_group(2, &self.object_bind_group, &[offset]);
                        pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
                    }
                }
            }
        }

        let clear = scene
            .background
            .map_or([0.0; 3], |b| b.clear_color)
            .map(f64::from);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: clear[0],
                        g: clear[1],
                        b: clear[2],
                        a: 1.0,
                    }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(1.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &self.frame_bind_group, &[]);

        if let Some(bg) = scene.background {
            if let Some((buffer, count)) = &bg.sky {
                pass.set_pipeline(&self.pipes.sky_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some(stars) = &bg.stars {
                pass.set_pipeline(&self.pipes.star_pipeline);
                pass.set_vertex_buffer(0, stars.slice(..));
                for b in &bg.star_batches {
                    pass.set_bind_group(3, &b.bind_group, &[]);
                    pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
                }
            }
        }

        // The 2D layer over the background, under the ships.
        if let (Some(buffer), false) = (self.pixel_buffer.buffer(), tess.pixel.is_empty()) {
            pass.set_pipeline(&self.pipes.pixel_pipeline);
            pass.set_bind_group(3, &self.effect_textures[0], &[]);
            pass.set_vertex_buffer(0, buffer.slice(..));
            pass.draw(0..tess.pixel.len() as u32, 0..1);
        }

        let draw_item = |pass: &mut wgpu::RenderPass<'_>,
                         item: &DrawItem<'_>,
                         filter: Option<bool>,
                         force_blend: bool| {
            let level = &item.ship.levels[item.level];
            pass.set_vertex_buffer(0, level.vertices.slice(..));
            pass.set_vertex_buffer(1, level.tints.slice(..));
            for b in &level.batches {
                if filter.is_some_and(|f| b.blended != f) {
                    continue;
                }
                let blended = b.blended || force_blend;
                let offset = ((item.first_slot as usize + b.object) as u64 * OBJECT_STRIDE) as u32;
                pass.set_bind_group(2, &self.object_bind_group, &[offset]);
                let two_sided = b.two_sided || item.two_sided;
                pass.set_pipeline(
                    &self.pipes.ship_pipelines[usize::from(two_sided) | usize::from(blended) << 1],
                );
                pass.set_bind_group(1, &level.materials[b.material], &[]);
                pass.set_bind_group(3, &self.shadows.bind, &[]);
                pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
            }
        };

        // Opaque ships: opaque batches first, then blended ones.
        for blended_pass in [false, true] {
            for item in items.iter().filter(|i| !i.transparent) {
                draw_item(&mut pass, item, Some(blended_pass), false);
            }
        }

        if let (Some(buffer), false) = (self.nav_buffer.buffer(), nav_draws.is_empty()) {
            pass.set_pipeline(&self.pipes.billboard_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            for (bind_group, first, count) in nav_draws {
                pass.set_bind_group(3, bind_group, &[]);
                pass.draw(first..first + count, 0..1);
            }
        }

        // Transparent ships, farthest first.
        let mut transparent: Vec<&DrawItem<'_>> = items.iter().filter(|i| i.transparent).collect();
        transparent.sort_by(|a, b| b.depth.total_cmp(&a.depth));
        for item in transparent {
            draw_item(&mut pass, item, None, true);
        }

        // Effects.
        if !glow_draws.is_empty() {
            pass.set_pipeline(&self.pipes.glow_pipeline);
            for (buffer, count, slot) in &glow_draws {
                pass.set_bind_group(
                    2,
                    &self.object_bind_group,
                    &[(*slot as u64 * OBJECT_STRIDE) as u32],
                );
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
        }
        let runs = |pass: &mut wgpu::RenderPass<'_>,
                    pipelines: &[wgpu::RenderPipeline; 2],
                    buffer: Option<&wgpu::Buffer>,
                    runs: &[crate::effects::DrawRun]| {
            let Some(buffer) = buffer else { return };
            if runs.is_empty() {
                return;
            }
            pass.set_vertex_buffer(0, buffer.slice(..));
            for r in runs {
                pass.set_pipeline(&pipelines[blend_index(r.blend)]);
                let tex = self
                    .effect_textures
                    .get(r.texture.0 as usize)
                    .unwrap_or(&self.effect_textures[0]);
                pass.set_bind_group(3, tex, &[]);
                pass.draw(r.first..r.first + r.count, 0..1);
            }
        };
        runs(
            &mut pass,
            &self.pipes.effect_pipelines,
            self.effect_buffer.buffer(),
            &tess.world_runs,
        );
        runs(
            &mut pass,
            &self.pipes.line_pipelines,
            self.line_buffer.buffer(),
            &tess.line_runs,
        );
        runs(
            &mut pass,
            &self.pipes.screen_pipelines,
            self.screen_buffer.buffer(),
            &tess.screen_runs,
        );
    }
}

/// Green wire box of the phase effect, with a grid on each face.
fn phase_box_lines(transform: Mat4, centre: Vec3, half: Vec3, alpha: f32, out: &mut EffectBatch) {
    let color = [0.0, 1.0, 0.0, alpha.clamp(0.0, 1.0)];
    let corner = |i: usize| {
        centre
            + Vec3::new(
                if i & 1 != 0 { half.x } else { -half.x },
                if i & 2 != 0 { half.y } else { -half.y },
                if i & 4 != 0 { half.z } else { -half.z },
            )
    };
    for i in 0..8usize {
        for bit in [1usize, 2, 4] {
            if i & bit == 0 {
                out.lines.push(crate::effects::Line {
                    from: transform.transform_point3(corner(i)),
                    to: transform.transform_point3(corner(i | bit)),
                    from_color: color,
                    to_color: color,
                    width: 1.0,
                    blend: Blend::Additive,
                });
            }
        }
    }
}

/// Lightning crawling over a window of hull edges that moves with time.
fn emp_lightning(
    edges: &[(usize, Vec3, Vec3)],
    objects: &[Mat4],
    transform: Mat4,
    time: f32,
    out: &mut EffectBatch,
) {
    if edges.is_empty() {
        return;
    }
    let window = edges.len().min(24);
    let start = ((time * 30.0) as usize) % edges.len();
    let mut seed = (time * 1000.0) as u32 ^ 0x9e37_79b9;
    let mut rand = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) - 0.5
    };
    for k in 0..window {
        let (obj, a, b) = edges[(start + k * 7) % edges.len()];
        let m = transform * objects.get(obj).copied().unwrap_or(Mat4::IDENTITY);
        let (a, b) = (m.transform_point3(a), m.transform_point3(b));
        let len = a.distance(b);
        let mut points = vec![a];
        for s in 1..4 {
            let t = s as f32 / 4.0;
            let jitter = Vec3::new(rand(), rand(), rand()) * len * 0.3;
            points.push(a.lerp(b, t) + jitter);
        }
        points.push(b);
        out.strips.push(Strip {
            points,
            width: (len * 0.08).clamp(0.5, 6.0),
            start_color: [0.6, 0.7, 1.0, 0.9],
            end_color: [0.4, 0.5, 1.0, 0.6],
            texture: EffectTexture::WHITE,
            blend: Blend::Additive,
        });
    }
}

/// Appends two triangles centred on `centre` with half extents `half`. Pixel-space quads
/// (stars) have y pointing down; world-space billboards have y up. `mode` 1 inverts the
/// bitmap's alpha, 2 ignores it (opaque bitmaps blend additively at full strength). A
/// billboard with a `plane` is cut by it (world space) like the hull it belongs to.
fn push_quad(
    out: &mut Vec<SpriteVertex>,
    centre: [f32; 3],
    half: [f32; 2],
    y_down: bool,
    color: [f32; 4],
    mode: f32,
    plane: Option<Vec4>,
) {
    let flip = if y_down { 1.0 } else { -1.0 };
    let corner = |sx: f32, sy: f32| SpriteVertex {
        centre,
        corner: [sx * half[0], sy * half[1] * flip],
        uv: [(sx + 1.0) / 2.0, (sy + 1.0) / 2.0],
        color,
        mode: [mode, f32::from(u8::from(plane.is_some()))],
        plane: plane.unwrap_or(Vec4::ZERO).to_array(),
    };
    let quad = [
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, -1.0),
        corner(1.0, 1.0),
        corner(-1.0, 1.0),
    ];
    out.extend_from_slice(&quad);
}

// ---------------------------------------------------------------------------------------------
// Headless rendering

/// Creates a device without a window (for screenshots and tests).
pub fn headless_device() -> Result<(wgpu::Device, wgpu::Queue), String> {
    headless_device_with_msaa().map(|(d, q, _)| (d, q))
}

/// [`headless_device`], also giving the MSAA sample counts it can render the scene with
/// (for [`Renderer::set_msaa_support`]).
pub fn headless_device_with_msaa() -> Result<(wgpu::Device, wgpu::Queue, Vec<u32>), String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) = request_device(&adapter)?;
    let counts = crate::quality::supported_sample_counts(&adapter, device.features());
    Ok((device, queue, counts))
}

/// Opens `adapter`'s device with the adapter-specific format features when it has them (so
/// MSAA can use 2 and 8 samples as well as WebGPU's 4).
pub fn request_device(adapter: &wgpu::Adapter) -> Result<(wgpu::Device, wgpu::Queue), String> {
    // Timestamp queries only for the profile (`Renderer::set_gpu_timing`).
    let wanted = wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES
        | wgpu::Features::TIMESTAMP_QUERY
        | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    let desc = wgpu::DeviceDescriptor {
        required_features: adapter.features() & wanted,
        ..Default::default()
    };
    pollster::block_on(adapter.request_device(&desc)).map_err(|e| format!("no GPU device: {e}"))
}

/// An offscreen colour + depth target that can be read back as RGBA8.
///
/// `width` × `height` is the logical size: the picture read back, the camera's aspect and the
/// unit of pixel-sized primitives. With [`with_quality`](Self::with_quality) the scene may be
/// drawn larger or smaller (render scale), with MSAA, filtered by FXAA, and also kept on the
/// GPU at a larger display size ([`present_view`](Self::present_view)).
pub struct OffscreenTarget {
    pub width: u32,
    pub height: u32,
    /// The sizes and sample count this target was made for.
    pub plan: crate::quality::RenderPlan,
    /// The scene's colour attachment (multisampled with MSAA).
    pub color_view: wgpu::TextureView,
    /// The scene's depth attachment (same samples as `color_view`).
    pub depth_view: wgpu::TextureView,
    /// The single-sampled scene colour the MSAA attachment resolves into (MSAA only).
    resolve_view: Option<wgpu::TextureView>,
    /// Bloom, heat haze, tone mapping and dithering straight after the scene ([`crate::post`]).
    modern_post: Option<crate::post::ModernPost>,
    /// Passes after the scene: resampling to the display size, FXAA, resampling to the logical
    /// size.
    post: Vec<crate::quality::PostStep>,
    /// The finished picture at the display size.
    present: (wgpu::Texture, wgpu::TextureView),
    /// The finished picture at the logical size, copied to the CPU.
    readback: wgpu::Texture,
    /// The mappable buffer the picture is copied into, kept between frames (allocating a
    /// frame-sized buffer every frame cost about a millisecond on an integrated GPU).
    readback_buffer: std::sync::Mutex<Option<(wgpu::Buffer, u64)>>,
}

/// Format used by [`OffscreenTarget`]; build the [`Renderer`] with it.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl OffscreenTarget {
    /// A target drawn at its own size with the renderer's sample count.
    pub fn new(renderer: &Renderer, width: u32, height: u32) -> Self {
        Self::with_quality(
            renderer,
            (width, height),
            (width, height),
            crate::quality::RenderQuality::default(),
        )
    }

    /// A target for a `logical`-sized picture shown at `present` pixels, drawn with `quality`'s
    /// scale and FXAA and the renderer's MSAA samples (call [`Renderer::set_sample_count`]
    /// first). The sizes are limited by [`crate::quality::plan`]; when the budget would need
    /// fewer samples than the renderer draws, the scale is cut further instead.
    pub fn with_quality(
        renderer: &Renderer,
        logical: (u32, u32),
        present: (u32, u32),
        quality: crate::quality::RenderQuality,
    ) -> Self {
        use crate::quality::{plan as make_plan, SAMPLE_BUDGET};
        let device = &renderer.device;
        let max_dim = device.limits().max_texture_dimension_2d;
        let n = renderer.sample_count();
        let mut plan = make_plan(logical, present, quality.scale, n, max_dim, SAMPLE_BUDGET);
        if plan.samples != n {
            plan = make_plan(logical, present, quality.scale, 1, max_dim, SAMPLE_BUDGET / u64::from(n));
            plan.samples = n;
            plan.limited = true;
        }
        let tex = |label: &str, size: (u32, u32), samples: u32, usage: wgpu::TextureUsages| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: OFFSCREEN_FORMAT,
                usage,
                view_formats: &[],
            })
        };
        let image = wgpu::TextureUsages::RENDER_ATTACHMENT
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC;
        let (scene, resolve) = if plan.samples > 1 {
            let ms = tex(
                "offscreen msaa",
                plan.render,
                plan.samples,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
            );
            (ms, Some(tex("offscreen resolve", plan.render, 1, image)))
        } else {
            (tex("offscreen", plan.render, 1, image), None)
        };
        let color_view = scene.create_view(&Default::default());
        let resolve_view = resolve.as_ref().map(|t| t.create_view(&Default::default()));
        // The chain of single-sampled pictures after the scene.
        let mut post = Vec::new();
        let mut cur = resolve.unwrap_or(scene);
        let mut cur_size = plan.render;
        let modern_post = (!quality.post.is_off()).then(|| {
            let out = tex("offscreen modern post", cur_size, 1, image);
            let p = crate::post::ModernPost::new(
                device,
                OFFSCREEN_FORMAT,
                &cur.create_view(&Default::default()),
                out.create_view(&Default::default()),
                cur_size,
                quality.post,
            );
            cur = out;
            p
        });
        let needs_post = cur_size != plan.present || quality.fxaa || plan.present != plan.logical;
        let pipes = needs_post.then(|| crate::quality::PostPipelines::new(device, OFFSCREEN_FORMAT));
        let mut step = |cur: &mut wgpu::Texture, cur_size: &mut (u32, u32), size: (u32, u32), fxaa: bool| {
            let Some(pipes) = &pipes else { return };
            let next = tex(
                if fxaa { "offscreen fxaa" } else { "offscreen resample" },
                size,
                1,
                image,
            );
            let src = cur.create_view(&Default::default());
            post.push(pipes.step(
                device,
                &src,
                *cur_size,
                next.create_view(&Default::default()),
                size,
                fxaa,
            ));
            *cur = next;
            *cur_size = size;
        };
        if cur_size != plan.present {
            step(&mut cur, &mut cur_size, plan.present, false);
        }
        if quality.fxaa {
            step(&mut cur, &mut cur_size, plan.present, true);
        }
        let present_tex = cur.clone();
        if cur_size != plan.logical {
            step(&mut cur, &mut cur_size, plan.logical, false);
        }
        let present_view = present_tex.create_view(&Default::default());
        Self {
            width: plan.logical.0,
            height: plan.logical.1,
            plan,
            color_view,
            depth_view: renderer.create_depth_samples(plan.render.0, plan.render.1, plan.samples),
            resolve_view,
            modern_post,
            post,
            present: (present_tex, present_view),
            readback: cur,
            readback_buffer: std::sync::Mutex::new(None),
        }
    }

    /// The finished picture at the display size (the plan's `present`), for drawing straight
    /// to the screen; valid after a render.
    pub fn present_view(&self) -> &wgpu::TextureView {
        &self.present.1
    }

    /// The display-size texture behind [`present_view`](Self::present_view).
    pub fn present_texture(&self) -> &wgpu::Texture {
        &self.present.0
    }

    /// Records the MSAA resolve and the post passes into `encoder`.
    fn finish(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        if let Some(resolve) = &self.resolve_view {
            // An empty pass that only resolves the samples.
            let _ = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("msaa resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    depth_slice: None,
                    resolve_target: Some(resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        if let Some(p) = &self.modern_post {
            p.record(queue, encoder);
        }
        for s in &self.post {
            s.record(encoder);
        }
    }

    /// Renders `scene` and returns tightly packed RGBA8 rows, top to bottom.
    pub fn render(&self, renderer: &mut Renderer, scene: &Scene<'_>) -> Result<Vec<u8>, String> {
        self.render_ex(renderer, scene, &SceneExtras::default())
    }

    /// [`render`](Self::render) with render states and effects.
    pub fn render_ex(
        &self,
        renderer: &mut Renderer,
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
    ) -> Result<Vec<u8>, String> {
        self.render_ex_with(renderer, scene, extras, |_, _, _, _| {})
    }

    /// [`render_ex`](Self::render_ex), then `after` records more passes into the same encoder
    /// over the scene's colour and depth (e.g. [`crate::EffectRenderer::draw`]) before the
    /// image is resolved, filtered and read back.
    pub fn render_ex_with(
        &self,
        renderer: &mut Renderer,
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
        after: impl FnOnce(
            &mut Renderer,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            &wgpu::TextureView,
        ),
    ) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.render_region_with(renderer, scene, extras, None, Sink::Grow(&mut out), after)?;
        Ok(out)
    }

    /// [`render_ex`](Self::render_ex) into `out`, which must hold the whole picture
    /// (`width * height * 4` bytes): saves the allocation and a copy when the caller keeps
    /// its own frame.
    pub fn render_ex_into(
        &self,
        renderer: &mut Renderer,
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
        out: &mut [u8],
    ) -> Result<(), String> {
        let need = (self.width * self.height * 4) as usize;
        if out.len() != need {
            return Err(format!("render_ex_into: {} bytes for a {need}-byte picture", out.len()));
        }
        self.render_region_with(renderer, scene, extras, None, Sink::Fill(out), |_, _, _, _| {})
    }

    /// [`render_ex`](Self::render_ex), reading back only the rectangle `x, y, w, h` (clamped to
    /// the target; rows of `w` pixels, top to bottom). For pictures that only cover part of
    /// the target, it saves most of the copy back.
    pub fn render_ex_region(
        &self,
        renderer: &mut Renderer,
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
        region: [u32; 4],
    ) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.render_region_with(renderer, scene, extras, Some(region), Sink::Grow(&mut out), |_, _, _, _| {})?;
        Ok(out)
    }

    /// The rectangle `region` leaves of the target (all of it for `None`).
    pub fn clamp_region(&self, region: Option<[u32; 4]>) -> [u32; 4] {
        let [x, y, w, h] = region.unwrap_or([0, 0, self.width, self.height]);
        let x = x.min(self.width.saturating_sub(1));
        let y = y.min(self.height.saturating_sub(1));
        [x, y, w.clamp(1, self.width - x), h.clamp(1, self.height - y)]
    }

    fn render_region_with(
        &self,
        renderer: &mut Renderer,
        scene: &Scene<'_>,
        extras: &SceneExtras<'_>,
        region: Option<[u32; 4]>,
        sink: Sink<'_>,
        after: impl FnOnce(
            &mut Renderer,
            &mut wgpu::CommandEncoder,
            &wgpu::TextureView,
            &wgpu::TextureView,
        ),
    ) -> Result<(), String> {
        if renderer.sample_count() != self.plan.samples {
            return Err(format!(
                "offscreen target made for {} samples, the renderer draws {}",
                self.plan.samples,
                renderer.sample_count()
            ));
        }
        let device = renderer.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        if let Some(t) = &renderer.timer {
            encoder.write_timestamp(&t.queries, 0);
        }
        renderer.render_ex(
            &mut encoder,
            &self.color_view,
            &self.depth_view,
            (self.width, self.height),
            scene,
            extras,
        );
        after(renderer, &mut encoder, &self.color_view, &self.depth_view);
        self.finish(&renderer.queue, &mut encoder);
        let [rx, ry, rw, rh] = self.clamp_region(region);
        let row = rw * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let size = u64::from(padded) * u64::from(rh);
        let mut kept = self.readback_buffer.lock().map_err(|_| "readback buffer poisoned")?;
        if kept.as_ref().is_none_or(|(_, s)| *s < size) {
            // Room for the whole target, so a smaller region never needs a new one.
            let full = u64::from(
                (self.width * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
                    * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
            ) * u64::from(self.height);
            let s = full.max(size);
            *kept = Some((
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("readback"),
                    size: s,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                }),
                s,
            ));
        }
        let buffer = &kept.as_ref().expect("readback buffer").0;
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &self.readback,
                mip_level: 0,
                origin: wgpu::Origin3d { x: rx, y: ry, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(rh),
                },
            },
            wgpu::Extent3d {
                width: rw,
                height: rh,
                depth_or_array_layers: 1,
            },
        );
        if let Some(t) = &renderer.timer {
            encoder.write_timestamp(&t.queries, 1);
            encoder.resolve_query_set(&t.queries, 0..2, &t.resolve, 0);
            encoder.copy_buffer_to_buffer(&t.resolve, 0, &t.read, 0, 16);
        }
        renderer.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..size);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let timer_rx = renderer.timer.as_ref().map(|t| {
            let (tx, rx) = std::sync::mpsc::channel();
            t.read.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            rx
        });
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .map_err(|e| format!("GPU poll failed: {e}"))?;
        rx.recv()
            .map_err(|e| e.to_string())?
            .map_err(|e| format!("readback failed: {e}"))?;
        let data = slice
            .get_mapped_range()
            .map_err(|e| format!("readback failed: {e}"))?;
        let row = row as usize;
        let rows = (0..rh as usize).map(|y| &data[y * padded as usize..y * padded as usize + row]);
        match sink {
            Sink::Grow(out) => {
                out.clear();
                out.reserve(row * rh as usize);
                rows.for_each(|r| out.extend_from_slice(r));
            }
            Sink::Fill(out) => {
                for (dst, r) in out.chunks_exact_mut(row).zip(rows) {
                    dst.copy_from_slice(r);
                }
            }
        }
        drop(data);
        buffer.unmap();
        if let (Some(t), Some(rx)) = (&renderer.timer, timer_rx) {
            if rx.recv().is_ok_and(|r| r.is_ok()) {
                let ticks = t.read.slice(..).get_mapped_range().ok().map(|d| {
                    let at = |i: usize| u64::from_le_bytes(d[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
                    at(1).saturating_sub(at(0))
                });
                t.read.unmap();
                renderer.last_gpu_ms = ticks.map(|n| n as f64 * f64::from(t.period) / 1e6);
            }
        }
        Ok(())
    }
}

/// Where a read-back picture goes.
enum Sink<'a> {
    /// Appended to a cleared vector.
    Grow(&'a mut Vec<u8>),
    /// Into a slice of exactly its size.
    Fill(&'a mut [u8]),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_light_blinking() {
        assert!(!nav_light_on(0.4, 1.0, 1.0, 0.5));
        assert!(nav_light_on(0.6, 1.0, 1.0, 0.5));
        assert!(!nav_light_on(1.6, 1.0, 1.0, 0.5));
        assert!(nav_light_on(2.6, 1.0, 1.0, 0.5));
        assert!(nav_light_on(5.0, 0.0, 0.0, 0.0));
    }

    #[test]
    fn uniform_sizes_are_aligned() {
        assert_eq!(std::mem::size_of::<FrameUniform>() % 16, 0);
        assert!(std::mem::size_of::<ObjectUniform>() as u64 <= OBJECT_STRIDE);
        assert_eq!(std::mem::size_of::<ObjectUniform>() % 16, 0);
    }
}
