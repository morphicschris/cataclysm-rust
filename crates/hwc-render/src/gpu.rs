//! The wgpu renderer. It owns pipelines and per-frame buffers but no window: callers hand it a
//! device and queue and render into any texture view of the configured colour format.

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use wgpu::util::DeviceExt;

use crate::assets::{ShipModel, TeamColors, TeamScheme};
use crate::background::Background;
use crate::background::SkyVertex;
use crate::camera::OrbitCamera;
use crate::lighting::SceneLighting;
use crate::mesh::{build_geometry, ShipVertex};

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
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct ObjectUniform {
    model: [[f32; 4]; 4],
    normal_model: [[f32; 4]; 4],
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

/// Team colouring applied when a ship is uploaded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TeamPaint<'a> {
    /// Textures as stored, without team colours.
    None,
    Scheme(&'a TeamColors, TeamScheme),
}

struct GpuBatch {
    first_vertex: u32,
    vertex_count: u32,
    pipeline: usize,
    bind_group: wgpu::BindGroup,
    blended: bool,
}

/// A ship's meshes, materials and nav lights on the GPU.
pub struct GpuShip {
    vertices: wgpu::Buffer,
    batches: Vec<GpuBatch>,
    nav_lights: Vec<crate::assets::NavLightDef>,
    /// Bind group per nav light texture name.
    nav_textures: Vec<(String, wgpu::BindGroup, bool)>,
    pub triangle_count: usize,
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
    nav_buffer: Option<(wgpu::Buffer, u64)>,
}

fn mat(m: Mat4) -> [[f32; 4]; 4] {
    m.to_cols_array_2d()
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
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX, true)],
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
        let object_capacity = 16;
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

        struct Spec<'a> {
            label: &'a str,
            layout: &'a wgpu::PipelineLayout,
            vs: &'a str,
            fs: &'a str,
            vertex: wgpu::VertexBufferLayout<'a>,
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
                    buffers: &[Some(s.vertex)],
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
                    vertex: ship_vertex_layout.clone(),
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
            vertex: sky_vertex_layout,
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
            vertex: sprite_vertex_layout.clone(),
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
            vertex: sprite_vertex_layout,
            cull: None,
            blend: Some(additive),
            depth_test: true,
            depth_write: false,
        });

        Self {
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
            nav_buffer: None,
        }
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

    /// Uploads a ship with the given team colouring.
    pub fn upload_ship(&self, model: &ShipModel, paint: TeamPaint<'_>) -> GpuShip {
        let geometry = build_geometry(model);
        let vertices = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&model.name),
                contents: bytemuck::cast_slice(if geometry.vertices.is_empty() {
                    &[ShipVertex {
                        position: [0.0; 3],
                        normal: [0.0; 3],
                        uv: [0.0; 2],
                    }][..]
                } else {
                    &geometry.vertices
                }),
                usage: wgpu::BufferUsages::VERTEX,
            });

        // One texture per (part, material) that has one, coloured for the team.
        let textures: Vec<Vec<Option<wgpu::TextureView>>> = model
            .parts
            .iter()
            .map(|part| {
                part.textures
                    .iter()
                    .map(|lif| {
                        let lif = lif.as_ref()?;
                        let rgba = match paint {
                            TeamPaint::None => lif.to_rgba(),
                            TeamPaint::Scheme(_, s) => lif.to_rgba_team(s.base, s.stripe),
                        };
                        Some(Self::make_texture(
                            &self.device,
                            &self.queue,
                            lif.width,
                            lif.height,
                            &rgba,
                            &part.path,
                        ))
                    })
                    .collect()
            })
            .collect();

        let batches = geometry
            .batches
            .iter()
            .map(|b| {
                let info = &geometry.materials[b.part][b.material];
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
                let view = textures[b.part][b.material].as_ref().unwrap_or(&self.white);
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
                let blended = info.alpha;
                GpuBatch {
                    first_vertex: b.first_vertex,
                    vertex_count: b.vertex_count,
                    pipeline: usize::from(info.two_sided()) | usize::from(blended) << 1,
                    bind_group,
                    blended,
                }
            })
            .collect();

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
            vertices,
            batches,
            nav_lights: model.nav_lights.clone(),
            nav_textures,
            triangle_count: geometry.vertices.len() / 3,
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
        (width, height): (u32, u32),
        scene: &Scene<'_>,
    ) {
        let aspect = width as f32 / height.max(1) as f32;
        let cam = scene.camera;
        let proj = cam.projection(aspect);
        let view = cam.view();
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
        };
        self.queue
            .write_buffer(&self.frame_buffer, 0, bytemuck::bytes_of(&frame));

        // Per-ship transforms.
        let needed = scene.ships.len().max(1) as u64;
        if needed > self.object_capacity {
            self.object_capacity = needed.next_power_of_two();
            (self.object_buffer, self.object_bind_group) =
                Self::object_storage(&self.device, &self.object_layout, self.object_capacity);
        }
        for (i, inst) in scene.ships.iter().enumerate() {
            let normal =
                Mat4::from_mat3(glam::Mat3::from_mat4(inst.transform).inverse().transpose());
            let u = ObjectUniform {
                model: mat(inst.transform),
                normal_model: mat(normal),
            };
            self.queue.write_buffer(
                &self.object_buffer,
                i as u64 * OBJECT_STRIDE,
                bytemuck::bytes_of(&u),
            );
        }

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
        if !nav_vertices.is_empty() {
            let bytes: &[u8] = bytemuck::cast_slice(&nav_vertices);
            let size = bytes.len() as u64;
            if self.nav_buffer.as_ref().is_none_or(|(_, cap)| *cap < size) {
                let cap = size.next_power_of_two();
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("nav lights"),
                    size: cap,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.nav_buffer = Some((buffer, cap));
            }
            if let Some((buffer, _)) = &self.nav_buffer {
                self.queue.write_buffer(buffer, 0, bytes);
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

        // Opaque batches first, then blended ones.
        for blended_pass in [false, true] {
            for (i, inst) in scene.ships.iter().enumerate() {
                let offset = (i as u64 * OBJECT_STRIDE) as u32;
                pass.set_bind_group(2, &self.object_bind_group, &[offset]);
                pass.set_vertex_buffer(0, inst.ship.vertices.slice(..));
                for b in inst
                    .ship
                    .batches
                    .iter()
                    .filter(|b| b.blended == blended_pass)
                {
                    pass.set_pipeline(&self.ship_pipelines[b.pipeline]);
                    pass.set_bind_group(1, &b.bind_group, &[]);
                    pass.draw(b.first_vertex..b.first_vertex + b.vertex_count, 0..1);
                }
            }
        }

        if let (Some((buffer, _)), false) = (&self.nav_buffer, nav_draws.is_empty()) {
            pass.set_pipeline(&self.billboard_pipeline);
            pass.set_vertex_buffer(0, buffer.slice(..));
            for (bind_group, first, count) in nav_draws {
                pass.set_bind_group(3, bind_group, &[]);
                pass.draw(first..first + count, 0..1);
            }
        }
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
        let device = renderer.device.clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        renderer.render(
            &mut encoder,
            &self.color_view,
            &self.depth_view,
            (self.width, self.height),
            scene,
        );
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
    }
}
