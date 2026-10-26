//! Shows a finished [`Canvas`] in a window through wgpu: the frame is uploaded as a texture
//! and drawn as one quad into the viewport chosen by [`crate::scale::Placement`].

use crate::canvas::Canvas;

const SHADER: &str = r#"
struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs(@builtin(vertex_index) i: u32) -> Out {
    // Two triangles covering the viewport.
    var p = array<vec2<f32>, 6>(
        vec2(0.0, 0.0), vec2(1.0, 0.0), vec2(0.0, 1.0),
        vec2(0.0, 1.0), vec2(1.0, 0.0), vec2(1.0, 1.0));
    let q = p[i];
    var o: Out;
    o.pos = vec4(q.x * 2.0 - 1.0, 1.0 - q.y * 2.0, 0.0, 1.0);
    o.uv = q;
    return o;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

@fragment
fn fs(i: Out) -> @location(0) vec4<f32> {
    return vec4(textureSample(tex, samp, i.uv).rgb, 1.0);
}

// A frame with transparent pixels, blended over what is already drawn.
@fragment
fn fs_over(i: Out) -> @location(0) vec4<f32> {
    return textureSample(tex, samp, i.uv);
}
"#;

/// Texture filtering when the frame is scaled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Filter {
    #[default]
    Nearest,
    Linear,
}

pub struct Presenter {
    pipeline: wgpu::RenderPipeline,
    /// The frame blended over a scene layer ([`draw_layered`](Self::draw_layered)).
    over_pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    texture: Option<(wgpu::Texture, wgpu::BindGroup, u32, u32)>,
}

impl Presenter {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat, filter: Filter) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hwc-ui present"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hwc-ui frame"),
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
            label: Some("hwc-ui present"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let make = |entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("hwc-ui present"),
                layout: Some(&pipeline_layout),
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
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipeline = make("fs", None);
        let over_pipeline = make("fs_over", Some(wgpu::BlendState::ALPHA_BLENDING));
        let f = match filter {
            Filter::Nearest => wgpu::FilterMode::Nearest,
            Filter::Linear => wgpu::FilterMode::Linear,
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hwc-ui frame"),
            mag_filter: f,
            min_filter: f,
            ..Default::default()
        });
        Self {
            pipeline,
            over_pipeline,
            layout,
            sampler,
            texture: None,
        }
    }

    /// Uploads the frame (the texture is recreated when the canvas size changes).
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, canvas: &Canvas) {
        let (w, h) = (canvas.width.max(1) as u32, canvas.height.max(1) as u32);
        self.upload_rgba(device, queue, (w, h), &canvas.to_rgba8());
    }

    /// Uploads a `size` frame of RGBA8 rows (alpha used by [`draw_layered`](Self::draw_layered)).
    pub fn upload_rgba(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        (w, h): (u32, u32),
        rgba: &[u8],
    ) {
        if !matches!(&self.texture, Some((_, _, tw, th)) if (*tw, *th) == (w, h)) {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("hwc-ui frame"),
                size: wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("hwc-ui frame"),
                layout: &self.layout,
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
            self.texture = Some((texture, bind, w, h));
        }
        let Some((texture, ..)) = &self.texture else {
            return;
        };
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
    }

    /// Clears the target to black and draws the frame into `viewport` (x, y, w, h in pixels).
    pub fn draw(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (f32, f32, f32, f32),
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hwc-ui present"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        let Some((_, bind, ..)) = &self.texture else {
            return;
        };
        let (x, y, w, h) = viewport;
        if w < 1.0 || h < 1.0 {
            return;
        }
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, bind, &[]);
        pass.draw(0..6, 0..1);
    }

    /// Clears the target, draws `scene` (a full-resolution picture of the viewport, e.g. the 3D
    /// view at window pixels) into `viewport`, then the uploaded frame over it, blended by its
    /// alpha: the frame's transparent pixels let the scene through.
    pub fn draw_layered(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        viewport: (f32, f32, f32, f32),
        scene: &wgpu::TextureView,
    ) {
        let scene_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hwc-ui scene layer"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(scene),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("hwc-ui present layered"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        let (x, y, w, h) = viewport;
        if w < 1.0 || h < 1.0 {
            return;
        }
        pass.set_viewport(x, y, w, h, 0.0, 1.0);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &scene_bind, &[]);
        pass.draw(0..6, 0..1);
        if let Some((_, bind, ..)) = &self.texture {
            pass.set_pipeline(&self.over_pipeline);
            pass.set_bind_group(0, bind, &[]);
            pass.draw(0..6, 0..1);
        }
    }
}

/// Runs the presenter without a window: draws `frame` into a `size` target at `viewport` and
/// reads the result back. Used by the screenshot tour to check the GPU path.
pub fn render_offscreen(
    frame: &Canvas,
    size: (u32, u32),
    viewport: (f32, f32, f32, f32),
    filter: Filter,
) -> Result<Canvas, String> {
    let instance =
        wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    let adapter =
        pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .map_err(|e| format!("no GPU adapter: {e}"))?;
    let (device, queue) =
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
            .map_err(|e| format!("no GPU device: {e}"))?;
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut presenter = Presenter::new(&device, format, filter);
    presenter.upload(&device, &queue, frame);
    let (w, h) = (size.0.max(1), size.1.max(1));
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hwc-ui offscreen"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut encoder = device.create_command_encoder(&Default::default());
    presenter.draw(&mut encoder, &view, viewport);
    let row = w * 4;
    let padded =
        row.div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT) * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("hwc-ui readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
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
    let mut out = Canvas::new(w as i32, h as i32);
    for y in 0..h as usize {
        let src = &data[y * padded as usize..y * padded as usize + row as usize];
        for (x, p) in src.as_chunks::<4>().0.iter().enumerate() {
            out.pixels[y * w as usize + x] = [p[0], p[1], p[2], 255];
        }
    }
    drop(data);
    buffer.unmap();
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wgpu::util::DeviceExt;

    /// The scene layer shows through the frame's transparent pixels only.
    #[test]
    fn layered_frame_lets_the_scene_through_transparent_pixels() {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let Ok(adapter) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
        else {
            eprintln!("skipping: no GPU adapter");
            return;
        };
        let Ok((device, queue)) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
        else {
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let mut p = Presenter::new(&device, format, Filter::Nearest);
        // A 2x1 frame: left pixel opaque blue, right pixel transparent.
        p.upload_rgba(&device, &queue, (2, 1), &[0, 0, 255, 255, 9, 9, 9, 0]);
        // The scene, at twice the frame's resolution: solid red.
        let scene = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: None,
                size: wgpu::Extent3d { width: 4, height: 2, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &[255, 0, 0, 255].repeat(8),
        );
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 64, height: 4, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        p.draw_layered(
            &device,
            &mut encoder,
            &target.create_view(&Default::default()),
            (0.0, 0.0, 64.0, 4.0),
            &scene.create_view(&Default::default()),
        );
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(4),
                },
            },
            wgpu::Extent3d { width: 64, height: 4, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        let slice = buffer.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        device
            .poll(wgpu::PollType::Wait { submission_index: None, timeout: None })
            .unwrap();
        let data = slice.get_mapped_range().unwrap();
        let px = |x: usize| &data[x * 4..x * 4 + 3];
        assert_eq!(px(4), [0, 0, 255], "the frame's opaque pixel");
        assert_eq!(px(60), [255, 0, 0], "the scene under the transparent pixel");
    }
}
