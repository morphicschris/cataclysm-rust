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
use crate::mesh::{build_level_geometry, ShipVertex, Skeleton, TintVertex};
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
    white: wgpu::TextureView,
    /// Indexed by `two_sided as usize | (blended as usize) << 1`.
    ship_pipelines: Vec<wgpu::RenderPipeline>,
    sky_pipeline: wgpu::RenderPipeline,
    star_pipeline: wgpu::RenderPipeline,
    billboard_pipeline: wgpu::RenderPipeline,
    glow_pipeline: wgpu::RenderPipeline,
    /// Indexed by blend: additive, alpha.
    effect_pipelines: [wgpu::RenderPipeline; 2],
    screen_pipelines: [wgpu::RenderPipeline; 2],
    line_pipelines: [wgpu::RenderPipeline; 2],
    nav_buffer: DynamicBuffer,
    effect_buffer: DynamicBuffer,
    screen_buffer: DynamicBuffer,
    line_buffer: DynamicBuffer,
    effect_textures: Vec<wgpu::BindGroup>,
    glow_meshes: Vec<Option<(wgpu::Buffer, u32)>>,
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
    fade: f32,
    tint_enabled: bool,
    tint: [f32; 4],
    engine_glow: [f32; 4],
    team: [f32; 4],
    exponent: f32,
}

impl Default for InstanceParams {
    fn default() -> Self {
        Self {
            clip_plane: None,
            fade: 1.0,
            tint_enabled: false,
            tint: [0.0; 4],
            engine_glow: [0.0; 4],
            team: [1.0; 4],
            exponent: 1.0,
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
        tint: p.tint,
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

        let white = Self::make_texture(device, queue, 1, 1, &[255; 4], "white");

        let ship_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ship"),
            bind_group_layouts: &[
                Some(&frame_layout),
                Some(&material_layout),
                Some(&object_layout),
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

        let ship_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<ShipVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        };
        let tint_vertex_layout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<TintVertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![3 => Uint32x2],
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
                0 => Float32x3, 1 => Float32x2, 2 => Float32x2, 3 => Float32x4, 4 => Float32x2
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
                    module: &shader,
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
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
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
                    layout: &ship_layout,
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
            layout: &sky_layout,
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
            layout: &sprite_pipeline_layout,
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
            layout: &sprite_pipeline_layout,
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
            layout: &glow_layout,
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
                layout: &sprite_pipeline_layout,
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
                layout: &sprite_pipeline_layout,
                vs: "vs_screen",
                fs: "fs_effect",
                vertex: vec![Some(effect_vertex_layout.clone())],
                cull: None,
                blend: Some(blend),
                depth_test: false,
                depth_write: false,
            })
        });
        let line_pipelines = blends.map(|blend| {
            build(Spec {
                label: "lines",
                layout: &sprite_pipeline_layout,
                vs: "vs_line",
                fs: "fs_effect",
                vertex: vec![Some(line_vertex_layout.clone())],
                cull: None,
                blend: Some(blend),
                depth_test: true,
                depth_write: false,
            })
        });

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
            white,
            ship_pipelines,
            sky_pipeline,
            star_pipeline,
            billboard_pipeline,
            glow_pipeline,
            effect_pipelines,
            screen_pipelines,
            line_pipelines,
            nav_buffer: DynamicBuffer::new("nav lights"),
            effect_buffer: DynamicBuffer::new("effects"),
            screen_buffer: DynamicBuffer::new("screen effects"),
            line_buffer: DynamicBuffer::new("lines"),
            effect_textures: Vec::new(),
            glow_meshes: Vec::new(),
        };
        let white = renderer.sprite_bind_group(&renderer.white, &renderer.linear_sampler);
        renderer.effect_textures.push(white);
        renderer
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn color_format(&self) -> wgpu::TextureFormat {
        self.color_format
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
        let geometry = build_level_geometry(model);
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
                    Self::make_texture(
                        &self.device,
                        &self.queue,
                        lif.width,
                        lif.height,
                        &rgba,
                        &part.path,
                    )
                });
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
                            resource: wgpu::BindingResource::Sampler(&self.linear_sampler),
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
            skeleton: geometry.skeleton,
            edges: geometry.edges,
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
            sky,
            stars,
            star_batches,
        }
    }

    /// A depth buffer matching a colour target.
    pub fn create_depth(&self, width: u32, height: u32) -> wgpu::TextureView {
        self.device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
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
        let frame = FrameUniform {
            view: mat(view),
            proj: mat(proj),
            sky_view_proj: mat(proj * cam.view_rotation()),
            global_ambient: v4(scene.lighting.global_ambient),
            light_dir: [0, 1]
                .map(|i| light(i).map_or([0.0; 4], |l| v4(l.direction.normalize_or_zero()))),
            light_ambient: [0, 1].map(|i| light(i).map_or([0.0; 4], |l| v4(l.ambient))),
            light_diffuse: [0, 1].map(|i| light(i).map_or([0.0; 4], |l| v4(l.diffuse))),
            params: [lights.len().min(2) as f32, width as f32, height as f32, 0.0],
            inv_view: mat(inv_view),
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
                            );
                            draw_main = false;
                        }
                        box_lines(1.0 - phase.value * 0.25, 1.0 - phase.value, &mut internal);
                    }
                }
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

        // Nav lights: billboards built on the CPU each frame, grouped by texture.
        let mut nav_vertices: Vec<SpriteVertex> = Vec::new();
        let mut nav_draws: Vec<(&wgpu::BindGroup, u32, u32)> = Vec::new();
        if scene.nav_lights {
            for inst in scene.ships {
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
                            [c[0], c[1], c[2], 1.0],
                            if *has_alpha { 0.0 } else { 2.0 },
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
                pass.set_pipeline(&self.sky_pipeline);
                pass.set_vertex_buffer(0, buffer.slice(..));
                pass.draw(0..*count, 0..1);
            }
            if let Some(stars) = &bg.stars {
                pass.set_pipeline(&self.star_pipeline);
                pass.set_vertex_buffer(0, stars.slice(..));
                for b in &bg.star_batches {
                    pass.set_bind_group(3, &b.bind_group, &[]);
                    pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
                }
            }
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
                pass.set_pipeline(
                    &self.ship_pipelines[usize::from(b.two_sided) | usize::from(blended) << 1],
                );
                pass.set_bind_group(1, &level.materials[b.material], &[]);
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
            pass.set_pipeline(&self.billboard_pipeline);
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
            pass.set_pipeline(&self.glow_pipeline);
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
            &self.effect_pipelines,
            self.effect_buffer.buffer(),
            &tess.world_runs,
        );
        runs(
            &mut pass,
            &self.line_pipelines,
            self.line_buffer.buffer(),
            &tess.line_runs,
        );
        runs(
            &mut pass,
            &self.screen_pipelines,
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
/// bitmap's alpha, 2 ignores it (opaque bitmaps blend additively at full strength).
fn push_quad(
    out: &mut Vec<SpriteVertex>,
    centre: [f32; 3],
    half: [f32; 2],
    y_down: bool,
    color: [f32; 4],
    mode: f32,
) {
    let flip = if y_down { 1.0 } else { -1.0 };
    let corner = |sx: f32, sy: f32| SpriteVertex {
        centre,
        corner: [sx * half[0], sy * half[1] * flip],
        uv: [(sx + 1.0) / 2.0, (sy + 1.0) / 2.0],
        color,
        mode: [mode, 0.0],
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
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        ..Default::default()
    }))
    .map_err(|e| format!("no GPU adapter: {e}"))?;
    pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        .map_err(|e| format!("no GPU device: {e}"))
}

/// An offscreen colour + depth target that can be read back as RGBA8.
pub struct OffscreenTarget {
    pub width: u32,
    pub height: u32,
    color: wgpu::Texture,
    pub color_view: wgpu::TextureView,
    pub depth_view: wgpu::TextureView,
}

/// Format used by [`OffscreenTarget`]; build the [`Renderer`] with it.
pub const OFFSCREEN_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

impl OffscreenTarget {
    pub fn new(renderer: &Renderer, width: u32, height: u32) -> Self {
        let color = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("offscreen"),
            size: wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: OFFSCREEN_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&Default::default());
        Self {
            width,
            height,
            color,
            color_view,
            depth_view: renderer.create_depth(width, height),
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
    /// image is read back.
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
        let device = renderer.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render_ex(
            &mut encoder,
            &self.color_view,
            &self.depth_view,
            (self.width, self.height),
            scene,
            extras,
        );
        after(renderer, &mut encoder, &self.color_view, &self.depth_view);
        let row = self.width * 4;
        let padded =
            row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"),
            size: u64::from(padded) * u64::from(self.height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            self.color.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(self.height),
                },
            },
            wgpu::Extent3d {
                width: self.width,
                height: self.height,
                depth_or_array_layers: 1,
            },
        );
        renderer.queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
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
        let mut out = Vec::with_capacity((row * self.height) as usize);
        for y in 0..self.height as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&data[start..start + row as usize]);
        }
        drop(data);
        buffer.unmap();
        Ok(out)
    }
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
