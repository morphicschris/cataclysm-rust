//! Modern image-quality options for the 3D view, all off by default (the original look):
//!
//! * an internal render scale (supersampling above 1, a cheaper picture below 1): the scene is
//!   drawn offscreen at the scaled size and filtered back to the output size on the GPU;
//! * MSAA, with the sample counts the adapter supports;
//! * FXAA, a cheap post filter over the finished picture.
//!
//! Every size is planned here ([`plan`]) so the scaled targets never exceed the device's
//! texture limit or a per-frame sample budget ([`SAMPLE_BUDGET`]) that keeps a frame on a
//! small integrated GPU well clear of a driver reset (TDR).
//!
//! Camera framing and anything sized in pixels (stars, lines, screen sprites) follow the
//! *logical* size, the picture's size before scaling, so a scaled picture frames and weighs
//! exactly like the unscaled one.

/// The lowest and highest internal render scale.
pub const SCALE_MIN: f32 = 0.5;
pub const SCALE_MAX: f32 = 2.0;

/// The most samples (render pixels × MSAA samples) one scene frame may draw. A 1600x900 view
/// at scale 2 with 2x MSAA is 11.5 M; 4x MSAA on top of scale 2 would be 23 M and is cut back
/// (the scale first, then the samples). Measured on an Intel UHD 750: see `docs/render-quality.md`.
pub const SAMPLE_BUDGET: u64 = 16_588_800; // 3840 x 2160 x 2

/// The 3D view's quality options.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderQuality {
    /// Internal render scale, [`SCALE_MIN`]..=[`SCALE_MAX`] (1: the output size).
    pub scale: f32,
    /// MSAA samples per pixel (1: none).
    pub msaa: u32,
    /// FXAA over the finished picture.
    pub fxaa: bool,
    /// Bloom, heat haze, tone mapping and dithering over the scene ([`crate::post`]).
    pub post: crate::post::PostFx,
}

impl Default for RenderQuality {
    fn default() -> Self {
        Self {
            scale: 1.0,
            msaa: 1,
            fxaa: false,
            post: crate::post::PostFx::default(),
        }
    }
}

impl RenderQuality {
    /// From the options file's integer keys: scale in percent, MSAA samples, FXAA 0/1.
    pub fn from_options(scale_percent: i32, msaa: i32, fxaa: i32) -> Self {
        Self {
            scale: clamp_scale(scale_percent as f32 / 100.0),
            msaa: msaa.clamp(1, 8) as u32,
            fxaa: fxaa != 0,
            post: crate::post::PostFx::default(),
        }
    }

    pub fn is_original(&self) -> bool {
        *self == Self::default()
    }
}

/// The scale clamped to [`SCALE_MIN`]..=[`SCALE_MAX`] (1 when not finite).
pub fn clamp_scale(s: f32) -> f32 {
    if s.is_finite() {
        s.clamp(SCALE_MIN, SCALE_MAX)
    } else {
        1.0
    }
}

/// The largest count in `supported` that is not above `want` (at least 1).
pub fn pick_sample_count(want: u32, supported: &[u32]) -> u32 {
    supported
        .iter()
        .copied()
        .filter(|&n| n <= want.max(1))
        .max()
        .unwrap_or(1)
}

/// The MSAA sample counts the adapter can render [`crate::OFFSCREEN_FORMAT`] and
/// [`crate::DEPTH_FORMAT`] with. Counts other than 1 and 4 need the device to have been
/// created with [`wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`]; without it the
/// list is WebGPU's guaranteed `[1, 4]`.
pub fn supported_sample_counts(adapter: &wgpu::Adapter, device_features: wgpu::Features) -> Vec<u32> {
    if !device_features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
        return vec![1, 4];
    }
    let color = adapter.get_texture_format_features(crate::OFFSCREEN_FORMAT).flags;
    let depth = adapter.get_texture_format_features(crate::DEPTH_FORMAT).flags;
    let mut v: Vec<u32> = [2, 4, 8]
        .into_iter()
        .filter(|&n| color.sample_count_supported(n) && depth.sample_count_supported(n))
        .collect();
    // A resolve of the colour target is needed as well.
    if !color.contains(wgpu::TextureFormatFeatureFlags::MULTISAMPLE_RESOLVE) {
        v.clear();
    }
    v.insert(0, 1);
    v
}

/// The sizes of one frame's targets.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderPlan {
    /// The size the camera frames and pixel-sized primitives are measured in.
    pub logical: (u32, u32),
    /// The picture kept on the GPU for display (the logical size, or the window's pixels when
    /// the frame is shown larger than it is laid out).
    pub present: (u32, u32),
    /// The size the scene is drawn at: `present` × the effective scale.
    pub render: (u32, u32),
    /// MSAA samples per render pixel.
    pub samples: u32,
    /// The scale used after the limits.
    pub scale: f32,
    /// The scale or the samples were cut to stay within the limits.
    pub limited: bool,
}

fn scaled(size: (u32, u32), s: f32) -> (u32, u32) {
    (
        ((size.0 as f32 * s).round() as u32).max(1),
        ((size.1 as f32 * s).round() as u32).max(1),
    )
}

fn samples_of(render: (u32, u32), samples: u32) -> u64 {
    u64::from(render.0) * u64::from(render.1) * u64::from(samples)
}

/// Plans the targets: `present` drawn at `scale` with `samples` per pixel, kept within
/// `max_dim` (the device's largest 2D texture) and `budget` samples. Over budget the scale
/// comes down first (not below 1 unless asked for), then the samples halve, and only then
/// does the scale drop below 1.
pub fn plan(
    logical: (u32, u32),
    present: (u32, u32),
    scale: f32,
    samples: u32,
    max_dim: u32,
    budget: u64,
) -> RenderPlan {
    let logical = (logical.0.max(1), logical.1.max(1));
    let max_dim = max_dim.max(1);
    // The present size itself must fit a texture.
    let present = {
        let p = (present.0.max(1), present.1.max(1));
        let fit = (max_dim as f32 / p.0 as f32).min(max_dim as f32 / p.1 as f32);
        if fit < 1.0 {
            (
                ((p.0 as f32 * fit).floor() as u32).clamp(1, max_dim),
                ((p.1 as f32 * fit).floor() as u32).clamp(1, max_dim),
            )
        } else {
            p
        }
    };
    let wanted = clamp_scale(scale);
    let mut s = wanted;
    let mut n = samples.max(1);
    let mut limited = false;
    // Texture limit.
    let dim_fit = (max_dim as f32 / present.0 as f32).min(max_dim as f32 / present.1 as f32);
    if s > dim_fit {
        s = dim_fit;
        limited = true;
    }
    let area = present.0 as f32 * present.1 as f32;
    let budget_scale = |n: u32| (budget as f32 / (area * n as f32)).sqrt();
    // The scale above 1 goes first.
    if samples_of(scaled(present, s), n) > budget {
        s = s.min(budget_scale(n).max(wanted.min(1.0)));
        limited = true;
    }
    // Then the samples.
    while n > 1 && samples_of(scaled(present, s), n) > budget {
        n /= 2;
        limited = true;
    }
    // Then the scale below 1 (a window larger than the budget at one sample).
    if samples_of(scaled(present, s), n) > budget {
        s = s.min(budget_scale(n));
        limited = true;
    }
    let mut render = scaled(present, s);
    // Rounding must not undo the limits.
    while samples_of(render, n) > budget && (render.0 > 1 || render.1 > 1) {
        render = (render.0.saturating_sub(1).max(1), render.1.saturating_sub(1).max(1));
    }
    render = (render.0.min(max_dim), render.1.min(max_dim));
    RenderPlan {
        logical,
        present,
        render,
        samples: n,
        scale: s,
        limited,
    }
}

// ---------------------------------------------------------------------------------------------
// GPU post-processing: MSAA resolve, resampling and FXAA.

const POST_SHADER: &str = r#"
struct Params {
    src_size: vec2<f32>,
    dst_size: vec2<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> p: Params;

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    // One triangle over the whole target.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    return vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
}

fn texel(x: i32, y: i32) -> vec4<f32> {
    let m = vec2<i32>(p.src_size) - vec2<i32>(1, 1);
    return textureLoad(src, clamp(vec2<i32>(x, y), vec2<i32>(0, 0), m), 0);
}

// Resampling: the exact area average of the source texels under each target pixel when
// shrinking (a box filter weighted by coverage), bilinear when enlarging.
@fragment
fn fs_resample(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let ratio = p.src_size / p.dst_size;
    let o = floor(pos.xy);
    if (ratio.x <= 1.0 && ratio.y <= 1.0) {
        return textureSampleLevel(src, samp, (o + 0.5) / p.dst_size, 0.0);
    }
    let a = o * ratio;
    let b = (o + 1.0) * ratio;
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    let y0 = i32(floor(a.y));
    let x0 = i32(floor(a.x));
    for (var j = 0; j < 8; j = j + 1) {
        let y = f32(y0 + j);
        if (y >= b.y) { break; }
        let wy = min(y + 1.0, b.y) - max(y, a.y);
        for (var i = 0; i < 8; i = i + 1) {
            let x = f32(x0 + i);
            if (x >= b.x) { break; }
            let w = wy * (min(x + 1.0, b.x) - max(x, a.x));
            sum = sum + texel(x0 + i, y0 + j) * w;
            total = total + w;
        }
    }
    return sum / max(total, 1e-6);
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

fn tap(uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(src, samp, uv, 0.0).rgb;
}

// FXAA: find the local edge direction from the corner lumas, blur along it, and keep the
// wider blur only when it stays within the neighbourhood's luma range.
@fragment
fn fs_fxaa(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let inv = 1.0 / p.src_size;
    let uv = pos.xy * inv;
    let m = tap(uv);
    let nw = luma(tap(uv + vec2<f32>(-1.0, -1.0) * inv));
    let ne = luma(tap(uv + vec2<f32>(1.0, -1.0) * inv));
    let sw = luma(tap(uv + vec2<f32>(-1.0, 1.0) * inv));
    let se = luma(tap(uv + vec2<f32>(1.0, 1.0) * inv));
    let lm = luma(m);
    let lo = min(lm, min(min(nw, ne), min(sw, se)));
    let hi = max(lm, max(max(nw, ne), max(sw, se)));
    // Flat areas stay untouched.
    if (hi - lo < max(1.0 / 16.0, hi * 0.125)) {
        return vec4<f32>(m, 1.0);
    }
    var dir = vec2<f32>(-((nw + ne) - (sw + se)), (nw + sw) - (ne + se));
    let reduce = max((nw + ne + sw + se) * (0.25 / 8.0), 1.0 / 128.0);
    let k = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * k, vec2<f32>(-8.0), vec2<f32>(8.0)) * inv;
    let near = 0.5 * (tap(uv + dir * (1.0 / 3.0 - 0.5)) + tap(uv + dir * (2.0 / 3.0 - 0.5)));
    let far = near * 0.5 + 0.25 * (tap(uv - dir * 0.5) + tap(uv + dir * 0.5));
    let lf = luma(far);
    if (lf < lo || lf > hi) {
        return vec4<f32>(near, 1.0);
    }
    return vec4<f32>(far, 1.0);
}
"#;

/// The resample and FXAA pipelines.
pub(crate) struct PostPipelines {
    layout: wgpu::BindGroupLayout,
    resample: wgpu::RenderPipeline,
    fxaa: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
}

impl PostPipelines {
    pub(crate) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hwc post"),
            source: wgpu::ShaderSource::Wgsl(POST_SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hwc post"),
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
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
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
            label: Some("hwc post"),
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
            label: Some("hwc post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Self {
            resample: pipeline("fs_resample"),
            fxaa: pipeline("fs_fxaa"),
            layout,
            sampler,
        }
    }

    /// A pass from `src` to `dst` (`fxaa` or resampling), bound once at target creation.
    pub(crate) fn step(
        &self,
        device: &wgpu::Device,
        src: &wgpu::TextureView,
        src_size: (u32, u32),
        dst: wgpu::TextureView,
        dst_size: (u32, u32),
        fxaa: bool,
    ) -> PostStep {
        use wgpu::util::DeviceExt;
        let params: [f32; 4] = [
            src_size.0 as f32,
            src_size.1 as f32,
            dst_size.0 as f32,
            dst_size.1 as f32,
        ];
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("hwc post"),
            contents: bytemuck::cast_slice(&params),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hwc post"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(src),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: buffer.as_entire_binding(),
                },
            ],
        });
        PostStep {
            pipeline: if fxaa {
                self.fxaa.clone()
            } else {
                self.resample.clone()
            },
            bind,
            dst,
            _params: buffer,
        }
    }
}

/// One recorded post-processing pass.
pub(crate) struct PostStep {
    pipeline: wgpu::RenderPipeline,
    bind: wgpu::BindGroup,
    dst: wgpu::TextureView,
    _params: wgpu::Buffer,
}

impl PostStep {
    pub(crate) fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hwc post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.dst,
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
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BIG: u32 = 8192;

    #[test]
    fn original_plan_is_one_to_one() {
        let p = plan((1600, 900), (1600, 900), 1.0, 1, BIG, SAMPLE_BUDGET);
        assert_eq!((p.logical, p.present, p.render, p.samples), ((1600, 900), (1600, 900), (1600, 900), 1));
        assert_eq!(p.scale, 1.0);
        assert!(!p.limited);
    }

    #[test]
    fn scale_multiplies_the_present_size() {
        let p = plan((1600, 900), (1600, 900), 2.0, 1, BIG, SAMPLE_BUDGET);
        assert_eq!(p.render, (3200, 1800));
        let p = plan((1600, 900), (1600, 900), 0.5, 1, BIG, SAMPLE_BUDGET);
        assert_eq!(p.render, (800, 450));
        let p = plan((1600, 900), (1600, 900), 1.5, 2, BIG, SAMPLE_BUDGET);
        assert_eq!((p.render, p.samples), ((2400, 1350), 2));
        assert!(!p.limited);
    }

    #[test]
    fn scale_is_clamped_to_its_range() {
        assert_eq!(clamp_scale(5.0), SCALE_MAX);
        assert_eq!(clamp_scale(0.1), SCALE_MIN);
        assert_eq!(clamp_scale(f32::NAN), 1.0);
        let p = plan((640, 480), (640, 480), 9.0, 1, BIG, SAMPLE_BUDGET);
        assert_eq!(p.render, (1280, 960));
    }

    #[test]
    fn texture_limit_caps_the_scale() {
        let p = plan((3000, 1000), (3000, 1000), 2.0, 1, 4096, u64::MAX);
        assert!(p.limited);
        assert!(p.render.0 <= 4096 && p.render.1 <= 4096, "{p:?}");
        assert_eq!(p.render.0, 4096);
        // A present size beyond the limit shrinks to fit.
        let p = plan((5000, 100), (5000, 100), 1.0, 1, 4096, u64::MAX);
        assert!(p.present.0 <= 4096 && p.render.0 <= 4096);
    }

    #[test]
    fn budget_cuts_scale_then_samples() {
        // 1600x900 at 2x with 4 samples: 23 M, over the budget. The scale comes down first,
        // keeping the 4 samples.
        let p = plan((1600, 900), (1600, 900), 2.0, 4, BIG, SAMPLE_BUDGET);
        assert!(p.limited);
        assert_eq!(p.samples, 4);
        assert!(samples_of(p.render, p.samples) <= SAMPLE_BUDGET);
        assert!(p.scale >= 1.0 && p.scale < 2.0, "{p:?}");
        // 4K at 8 samples: the scale stays 1 and the samples halve.
        let p = plan((3840, 2160), (3840, 2160), 1.0, 8, BIG, SAMPLE_BUDGET);
        assert_eq!((p.render, p.samples), ((3840, 2160), 2));
        // A huge window at one sample has to come below scale 1.
        let p = plan((7680, 4320), (7680, 4320), 1.0, 1, BIG, SAMPLE_BUDGET);
        assert!(p.scale < 1.0 && samples_of(p.render, 1) <= SAMPLE_BUDGET, "{p:?}");
    }

    #[test]
    fn layered_present_size() {
        // A 640x480 layout shown in a 1440x1080 viewport: the scene is drawn at the viewport's
        // pixels, framed as the layout.
        let p = plan((640, 480), (1440, 1080), 1.0, 1, BIG, SAMPLE_BUDGET);
        assert_eq!((p.logical, p.present, p.render), ((640, 480), (1440, 1080), (1440, 1080)));
    }

    #[test]
    fn sample_count_picking() {
        assert_eq!(pick_sample_count(8, &[1, 2, 4, 8]), 8);
        assert_eq!(pick_sample_count(8, &[1, 4]), 4);
        assert_eq!(pick_sample_count(2, &[1, 4]), 1);
        assert_eq!(pick_sample_count(0, &[1, 4]), 1);
        assert_eq!(pick_sample_count(16, &[]), 1);
    }

    #[test]
    fn options_keys() {
        let q = RenderQuality::from_options(100, 1, 0);
        assert!(q.is_original());
        let q = RenderQuality::from_options(150, 4, 1);
        assert_eq!((q.scale, q.msaa, q.fxaa), (1.5, 4, true));
        assert_eq!(RenderQuality::from_options(1000, 99, 0).scale, SCALE_MAX);
        assert_eq!(RenderQuality::from_options(-5, -3, 0).msaa, 1);
    }
}
