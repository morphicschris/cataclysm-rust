//! Modern post-processing over the finished scene, all off by default (the original's picture):
//!
//! * bloom: the bright parts (engines, lit panels, weapon fire, explosions) are cut out at a
//!   quarter of the size, blurred twice and added back as a soft glow;
//! * heat haze: the picture shimmers around those hot spots, the blurred bloom picture giving
//!   the strength, moving noise the direction;
//! * tone mapping: colours that the bloom pushes past white roll off along a shoulder instead
//!   of clipping flat;
//! * dithering: a little noise below one step of 8-bit colour, which breaks up the bands in the
//!   backgrounds' smooth gradients.
//!
//! The passes run at the scene's size, before the resample and FXAA steps of
//! [`crate::quality`]. At 1600x900 the bloom passes touch 0.36 M quarter-size pixels five
//! times and the composite 1.44 M once.

/// The post effects' settings.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PostFx {
    /// Strength of the bloom added back (0: none).
    pub bloom: f32,
    /// Strength of the heat haze (0: none).
    pub heat: f32,
    /// Roll bright colours off instead of clipping them.
    pub tone_map: bool,
    /// Dither below one 8-bit step.
    pub dither: bool,
}

impl PostFx {
    /// Every effect at its default strength.
    pub fn modern() -> Self {
        Self {
            bloom: 1.0,
            heat: 1.0,
            tone_map: true,
            dither: true,
        }
    }

    pub fn is_off(&self) -> bool {
        self.bloom <= 0.0 && self.heat <= 0.0 && !self.tone_map && !self.dither
    }

    fn needs_bloom_chain(&self) -> bool {
        self.bloom > 0.0 || self.heat > 0.0
    }
}

const SHADER: &str = r#"
struct Params {
    full: vec2<f32>,
    small: vec2<f32>,
    // x bloom, y heat, z tone map, w dither
    fx: vec4<f32>,
    // x seconds, y bright threshold
    misc: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var glow: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;
@group(0) @binding(3) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

// Quarter size from the scene: a 4x4 box of texels (four bilinear taps), kept where it is
// brighter than its surroundings (eight taps 24 pixels out) by more than the threshold, so
// small hot things bloom (engines, beams, lit windows, explosions) and the backgrounds' broad
// bright nebulae do not.
@fragment
fn fs_bright(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / p.small;
    let t = 1.0 / p.full;
    var c = vec3<f32>(0.0);
    for (var j = 0; j < 2; j++) {
        for (var i = 0; i < 2; i++) {
            let o = (vec2<f32>(f32(i), f32(j)) * 2.0 - 1.0) * t;
            c += textureSampleLevel(src, samp, uv + o, 0.0).rgb;
        }
    }
    c *= 0.25;
    var around = 0.0;
    for (var i = 0; i < 8; i++) {
        let a = f32(i) * 0.785398;
        around += luma(textureSampleLevel(src, samp, uv + vec2<f32>(cos(a), sin(a)) * 24.0 * t, 0.0).rgb);
    }
    let hot = luma(c) - around / 8.0 - p.misc.y;
    let k = clamp(hot * 4.0, 0.0, 1.0);
    return vec4<f32>(c * k, 1.0);
}

fn blur(pos: vec2<f32>, dir: vec2<f32>) -> vec4<f32> {
    let uv = pos / p.small;
    let step = dir * 1.5 / p.small;
    let w = array<f32, 5>(0.227, 0.194, 0.121, 0.054, 0.016);
    var c = textureSampleLevel(src, samp, uv, 0.0).rgb * w[0];
    for (var i = 1; i < 5; i++) {
        let o = step * f32(i);
        c += (textureSampleLevel(src, samp, uv + o, 0.0).rgb
            + textureSampleLevel(src, samp, uv - o, 0.0).rgb) * w[i];
    }
    return vec4<f32>(c, 1.0);
}

@fragment
fn fs_blur_h(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(pos.xy, vec2<f32>(1.0, 0.0));
}

@fragment
fn fs_blur_v(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return blur(pos.xy, vec2<f32>(0.0, 1.0));
}

fn hash(q: vec2<f32>) -> f32 {
    let h = dot(q, vec2<f32>(127.1, 311.7));
    return fract(sin(h) * 43758.5453);
}

fn noise(q: vec2<f32>) -> f32 {
    let i = floor(q);
    let f = fract(q);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash(i), hash(i + vec2<f32>(1.0, 0.0)), u.x),
               mix(hash(i + vec2<f32>(0.0, 1.0)), hash(i + vec2<f32>(1.0, 1.0)), u.x), u.y);
}

// The shoulder: unchanged up to 0.75, then easing towards 1.
fn shoulder(c: vec3<f32>) -> vec3<f32> {
    let k = 0.75;
    let over = max(c - vec3<f32>(k), vec3<f32>(0.0));
    let eased = vec3<f32>(k) + (1.0 - k) * (vec3<f32>(1.0) - exp(-over / (1.0 - k)));
    return select(c, eased, c > vec3<f32>(k));
}

@fragment
fn fs_composite(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = pos.xy / p.full;
    let g = textureSampleLevel(glow, samp, uv, 0.0).rgb;
    var at = uv;
    if (p.fx.y > 0.0) {
        // Shimmer, a few pixels at most, rising slowly: around hot spots, not over the glowing
        // surface itself (whose own brightness turns it down).
        let here = luma(textureSampleLevel(src, samp, uv, 0.0).rgb);
        let heat = clamp(luma(g) * 3.0, 0.0, 1.0) * clamp(1.5 - here * 2.0, 0.0, 1.0) * p.fx.y;
        let q = uv * vec2<f32>(p.full.x / p.full.y, 1.0) * 60.0;
        let t = p.misc.x;
        let n = vec2<f32>(noise(q + vec2<f32>(0.0, t * 3.0)), noise(q * 1.3 + vec2<f32>(t * 2.1, 17.0)));
        at = uv + (n - 0.5) * heat * 4.0 / p.full;
    }
    var c = textureSampleLevel(src, samp, at, 0.0).rgb + g * p.fx.x;
    if (p.fx.z > 0.5) {
        c = shoulder(c);
    }
    if (p.fx.w > 0.5) {
        c += (hash(pos.xy + vec2<f32>(fract(p.misc.x * 7.0) * 113.0)) - 0.5) / 255.0;
    }
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    full: [f32; 2],
    small: [f32; 2],
    fx: [f32; 4],
    misc: [f32; 4],
}

struct Pass {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    dst: wgpu::TextureView,
}

/// The post passes of one target, bound once when the target is made.
pub(crate) struct ModernPost {
    passes: Vec<Pass>,
    uniform: wgpu::Buffer,
    params: Params,
    start: std::time::Instant,
}

impl ModernPost {
    /// Passes from `scene` (`size`) into `out` (same size), with `fx`.
    pub(crate) fn new(
        device: &wgpu::Device,
        format: wgpu::TextureFormat,
        scene: &wgpu::TextureView,
        out: wgpu::TextureView,
        size: (u32, u32),
        fx: PostFx,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hwc modern post"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let texture = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hwc modern post"),
            entries: &[
                texture(0),
                texture(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("hwc modern post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hwc modern post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });
        let small = ((size.0 / 4).max(1), (size.1 / 4).max(1));
        let params = Params {
            full: [size.0 as f32, size.1 as f32],
            small: [small.0 as f32, small.1 as f32],
            fx: [
                fx.bloom.max(0.0),
                fx.heat.max(0.0),
                f32::from(u8::from(fx.tone_map)),
                f32::from(u8::from(fx.dither)),
            ],
            misc: [0.0, 0.12, 0.0, 0.0],
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hwc modern post"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let small_tex = |label| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: small.0,
                        height: small.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hwc modern post"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(a),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(b),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            })
        };
        let mut passes = Vec::new();
        let glow = if fx.needs_bloom_chain() {
            let (a, b) = (small_tex("bloom a"), small_tex("bloom b"));
            passes.push(Pass {
                pipeline: pipeline("fs_bright"),
                bind: bind(scene, scene),
                dst: a.clone(),
            });
            let (h, v) = (pipeline("fs_blur_h"), pipeline("fs_blur_v"));
            for _ in 0..2 {
                passes.push(Pass {
                    pipeline: h.clone(),
                    bind: bind(&a, &a),
                    dst: b.clone(),
                });
                passes.push(Pass {
                    pipeline: v.clone(),
                    bind: bind(&b, &b),
                    dst: a.clone(),
                });
            }
            a
        } else {
            // Nothing to add: a black quarter picture.
            small_tex("no bloom")
        };
        passes.push(Pass {
            pipeline: pipeline("fs_composite"),
            bind: bind(scene, &glow),
            dst: out,
        });
        Self {
            passes,
            uniform,
            params,
            start: std::time::Instant::now(),
        }
    }

    pub(crate) fn record(&self, queue: &wgpu::Queue, encoder: &mut wgpu::CommandEncoder) {
        let mut params = self.params;
        params.misc[0] = self.start.elapsed().as_secs_f32() % 1000.0;
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&params));
        for p in &self.passes {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hwc modern post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &p.dst,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&p.pipeline);
            pass.set_bind_group(0, &p.bind, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}
