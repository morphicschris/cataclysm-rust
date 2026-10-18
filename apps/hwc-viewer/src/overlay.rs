//! 2D line overlays (selection brackets, drag box, the move plate) drawn on top of the 3D
//! scene: on the GPU for the window, on the CPU for offscreen screenshots.

use hwc_render::wgpu;

/// A line in pixel coordinates (origin top left) with an RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Line {
    pub a: [f32; 2],
    pub b: [f32; 2],
    pub color: [f32; 4],
}

/// Lines to draw this frame.
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub lines: Vec<Line>,
}

impl Overlay {
    pub fn line(&mut self, a: [f32; 2], b: [f32; 2], color: [f32; 4]) {
        self.lines.push(Line { a, b, color });
    }

    pub fn rect(&mut self, lo: [f32; 2], hi: [f32; 2], color: [f32; 4]) {
        let (x0, y0, x1, y1) = (lo[0], lo[1], hi[0], hi[1]);
        self.line([x0, y0], [x1, y0], color);
        self.line([x1, y0], [x1, y1], color);
        self.line([x1, y1], [x0, y1], color);
        self.line([x0, y1], [x0, y0], color);
    }

    /// Corner brackets around a box (the game's selection marker style).
    pub fn brackets(&mut self, lo: [f32; 2], hi: [f32; 2], color: [f32; 4]) {
        let (x0, y0, x1, y1) = (lo[0], lo[1], hi[0], hi[1]);
        let l = ((x1 - x0).min(y1 - y0) * 0.3).max(3.0);
        for (cx, cy, dx, dy) in [
            (x0, y0, 1.0, 1.0),
            (x1, y0, -1.0, 1.0),
            (x0, y1, 1.0, -1.0),
            (x1, y1, -1.0, -1.0),
        ] {
            self.line([cx, cy], [cx + dx * l, cy], color);
            self.line([cx, cy], [cx, cy + dy * l], color);
        }
    }

    /// A closed polyline.
    pub fn polygon(&mut self, points: &[[f32; 2]], color: [f32; 4]) {
        for i in 0..points.len() {
            self.line(points[i], points[(i + 1) % points.len()], color);
        }
    }

    /// Draws every line into tightly packed RGBA8 rows (alpha-blended).
    pub fn rasterize(&self, rgba: &mut [u8], width: u32, height: u32) {
        for l in &self.lines {
            let (dx, dy) = (l.b[0] - l.a[0], l.b[1] - l.a[1]);
            let steps = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
            for i in 0..=steps {
                let t = i as f32 / steps as f32;
                let x = (l.a[0] + dx * t).round() as i32;
                let y = (l.a[1] + dy * t).round() as i32;
                if x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
                    continue;
                }
                let p = ((y as u32 * width + x as u32) * 4) as usize;
                let alpha = l.color[3];
                for c in 0..3 {
                    let src = l.color[c] * 255.0;
                    let dst = f32::from(rgba[p + c]);
                    rgba[p + c] = (src * alpha + dst * (1.0 - alpha))
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
                rgba[p + 3] = 255;
            }
        }
    }
}

const SHADER: &str = r"
struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) color: vec4<f32> };
@vertex fn vs(@location(0) pos: vec2<f32>, @location(1) color: vec4<f32>) -> VsOut {
    var o: VsOut;
    o.pos = vec4<f32>(pos, 0.0, 1.0);
    o.color = color;
    return o;
}
@fragment fn fs(i: VsOut) -> @location(0) vec4<f32> { return i.color; }
";

/// GPU renderer for [`Overlay`] lines (one pass that loads the existing colour).
pub struct OverlayRenderer {
    pipeline: wgpu::RenderPipeline,
    buffer: Option<(wgpu::Buffer, u64)>,
}

impl OverlayRenderer {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("overlay"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("overlay"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("overlay"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 24,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::LineList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        Self {
            pipeline,
            buffer: None,
        }
    }

    pub fn draw(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        overlay: &Overlay,
    ) {
        if overlay.lines.is_empty() {
            return;
        }
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let mut data: Vec<f32> = Vec::with_capacity(overlay.lines.len() * 12);
        for l in &overlay.lines {
            for p in [l.a, l.b] {
                data.extend_from_slice(&[p[0] / w * 2.0 - 1.0, 1.0 - p[1] / h * 2.0]);
                data.extend_from_slice(&l.color);
            }
        }
        let bytes: Vec<u8> = data.iter().flat_map(|f| f.to_le_bytes()).collect();
        let needed = bytes.len() as u64;
        if self.buffer.as_ref().is_none_or(|(_, cap)| *cap < needed) {
            let cap = needed.next_power_of_two();
            let buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("overlay"),
                size: cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.buffer = Some((buf, cap));
        }
        let (buf, _) = self.buffer.as_ref().expect("buffer created above");
        queue.write_buffer(buf, 0, &bytes);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlay"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, buf.slice(..needed));
        pass.draw(0..(overlay.lines.len() as u32 * 2), 0..1);
    }
}
