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

/// A filled axis-aligned rectangle in pixel coordinates with an RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub lo: [f32; 2],
    pub hi: [f32; 2],
    pub color: [f32; 4],
}

/// Lines and filled rectangles to draw this frame (rectangles first, then lines).
#[derive(Debug, Clone, Default)]
pub struct Overlay {
    pub lines: Vec<Line>,
    pub rects: Vec<Rect>,
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

    /// A filled rectangle.
    pub fn fill(&mut self, lo: [f32; 2], hi: [f32; 2], color: [f32; 4]) {
        if hi[0] > lo[0] && hi[1] > lo[1] {
            self.rects.push(Rect { lo, hi, color });
        }
    }

    /// A closed polyline.
    pub fn polygon(&mut self, points: &[[f32; 2]], color: [f32; 4]) {
        for i in 0..points.len() {
            self.line(points[i], points[(i + 1) % points.len()], color);
        }
    }

    /// Draws every rectangle and line into tightly packed RGBA8 rows (alpha-blended).
    pub fn rasterize(&self, rgba: &mut [u8], width: u32, height: u32) {
        for r in &self.rects {
            let x0 = r.lo[0].round().max(0.0) as u32;
            let y0 = r.lo[1].round().max(0.0) as u32;
            let x1 = (r.hi[0].round().max(0.0) as u32).min(width);
            let y1 = (r.hi[1].round().max(0.0) as u32).min(height);
            for y in y0..y1 {
                for x in x0..x1 {
                    blend_pixel(rgba, ((y * width + x) * 4) as usize, r.color);
                }
            }
        }
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
                blend_pixel(rgba, ((y as u32 * width + x as u32) * 4) as usize, l.color);
            }
        }
    }
}

fn blend_pixel(rgba: &mut [u8], p: usize, color: [f32; 4]) {
    let alpha = color[3];
    for c in 0..3 {
        let src = color[c] * 255.0;
        let dst = f32::from(rgba[p + c]);
        rgba[p + c] = (src * alpha + dst * (1.0 - alpha))
            .round()
            .clamp(0.0, 255.0) as u8;
    }
    rgba[p + 3] = 255;
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

/// GPU renderer for [`Overlay`] rectangles and lines (one pass that loads the existing colour).
pub struct OverlayRenderer {
    lines: wgpu::RenderPipeline,
    triangles: wgpu::RenderPipeline,
    buffer: Option<(wgpu::Buffer, u64)>,
}

fn pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    module: &wgpu::ShaderModule,
    layout: &wgpu::PipelineLayout,
    topology: wgpu::PrimitiveTopology,
) -> wgpu::RenderPipeline {
    device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("overlay"),
        layout: Some(layout),
        vertex: wgpu::VertexState {
            module,
            entry_point: Some("vs"),
            compilation_options: Default::default(),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 24,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
            })],
        },
        fragment: Some(wgpu::FragmentState {
            module,
            entry_point: Some("fs"),
            compilation_options: Default::default(),
            targets: &[Some(wgpu::ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: wgpu::ColorWrites::ALL,
            })],
        }),
        primitive: wgpu::PrimitiveState {
            topology,
            ..Default::default()
        },
        depth_stencil: None,
        multisample: Default::default(),
        multiview_mask: None,
        cache: None,
    })
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
        Self {
            lines: pipeline(device, format, &module, &layout, wgpu::PrimitiveTopology::LineList),
            triangles: pipeline(
                device,
                format,
                &module,
                &layout,
                wgpu::PrimitiveTopology::TriangleList,
            ),
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
        if overlay.lines.is_empty() && overlay.rects.is_empty() {
            return;
        }
        let (w, h) = (size.0.max(1) as f32, size.1.max(1) as f32);
        let mut data: Vec<f32> =
            Vec::with_capacity(overlay.rects.len() * 36 + overlay.lines.len() * 12);
        let mut push = |p: [f32; 2], c: &[f32; 4]| {
            data.extend_from_slice(&[p[0] / w * 2.0 - 1.0, 1.0 - p[1] / h * 2.0]);
            data.extend_from_slice(c);
        };
        for r in &overlay.rects {
            let (a, b) = (r.lo, r.hi);
            for p in [a, [b[0], a[1]], b, a, b, [a[0], b[1]]] {
                push(p, &r.color);
            }
        }
        for l in &overlay.lines {
            push(l.a, &l.color);
            push(l.b, &l.color);
        }
        let rect_vertices = overlay.rects.len() as u32 * 6;
        let line_vertices = overlay.lines.len() as u32 * 2;
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
        pass.set_vertex_buffer(0, buf.slice(..needed));
        if rect_vertices > 0 {
            pass.set_pipeline(&self.triangles);
            pass.draw(0..rect_vertices, 0..1);
        }
        if line_vertices > 0 {
            pass.set_pipeline(&self.lines);
            pass.draw(rect_vertices..rect_vertices + line_vertices, 0..1);
        }
    }
}
