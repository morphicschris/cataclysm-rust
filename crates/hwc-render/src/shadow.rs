//! Ship shadows for the modern look ([`crate::look::ShipLook::shadows`]): one shadow map from
//! the main light, an orthographic box around the camera's look-at point sized by its
//! distance, so the ships the player is looking at shadow themselves and each other. Ships
//! outside the box are unshadowed, fading out towards its edge.

use glam::{Mat4, Vec3};

use crate::camera::OrbitCamera;
use crate::gpu::DEPTH_FORMAT;

/// The shadow map's size in texels (16 MB of depth).
pub const SHADOW_MAP_SIZE: u32 = 2048;

/// World to the shadow map's clip space for a light shining from `towards_light` onto the
/// camera's focus, and the size of one texel in world units. The box is snapped to whole
/// texels so shadows do not crawl as the camera moves.
pub fn shadow_matrix(camera: &OrbitCamera, towards_light: Vec3) -> (Mat4, f32) {
    let l = towards_light.normalize_or(Vec3::Z);
    let radius = (camera.distance * 0.9).clamp(200.0, 60_000.0);
    let up = if l.z.abs() > 0.95 { Vec3::Y } else { Vec3::Z };
    let view = Mat4::look_at_rh(l * radius * 2.0, Vec3::ZERO, up);
    let texel = 2.0 * radius / SHADOW_MAP_SIZE as f32;
    // The focus in light space, snapped.
    let f = view.transform_point3(camera.look_at);
    let snapped = Vec3::new(
        (f.x / texel).round() * texel,
        (f.y / texel).round() * texel,
        f.z,
    );
    let proj = Mat4::orthographic_rh(
        snapped.x - radius,
        snapped.x + radius,
        snapped.y - radius,
        snapped.y + radius,
        -snapped.z - radius * 2.0,
        -snapped.z + radius * 2.0,
    );
    (proj * view, texel)
}

/// The shadow map, its bind group for the ship pipelines and the depth-only pipeline.
pub(crate) struct Shadows {
    pub(crate) layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The bind group the ship pipelines take (a 1x1 map until [`Shadows::ensure_map`]).
    pub(crate) bind: wgpu::BindGroup,
    map: Option<wgpu::TextureView>,
    pub(crate) pipeline: Option<wgpu::RenderPipeline>,
}

fn depth_view(device: &wgpu::Device, size: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: DEPTH_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl Shadows {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let dummy = depth_view(device, 1);
        let bind = Self::bind_group(device, &layout, &sampler, &dummy);
        Self {
            layout,
            sampler,
            bind,
            map: None,
            pipeline: None,
        }
    }

    fn bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        view: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow"),
            layout,
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

    /// The full-size map, made the first time shadows are drawn.
    pub(crate) fn ensure_map(&mut self, device: &wgpu::Device) -> &wgpu::TextureView {
        if self.map.is_none() {
            let view = depth_view(device, SHADOW_MAP_SIZE);
            self.bind = Self::bind_group(device, &self.layout, &self.sampler, &view);
            self.map = Some(view);
        }
        self.map.as_ref().expect("shadow map")
    }

    /// The depth-only pipeline: ship positions through `vs_shadow`, both faces, with a slope
    /// bias against acne.
    pub(crate) fn build_pipeline(
        &mut self,
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        frame_layout: &wgpu::BindGroupLayout,
        object_layout: &wgpu::BindGroupLayout,
        vertex_stride: u64,
    ) {
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shadow"),
            bind_group_layouts: &[Some(frame_layout), None, Some(object_layout)],
            immediate_size: 0,
        });
        self.pipeline = Some(
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("shadow"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_shadow"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: vertex_stride,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    })],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: Default::default(),
                    bias: wgpu::DepthBiasState {
                        constant: 2,
                        slope_scale: 2.0,
                        clamp: 0.0,
                    },
                }),
                multisample: Default::default(),
                fragment: None,
                multiview_mask: None,
                cache: None,
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::CameraTweaks;
    use glam::Vec4Swizzles;

    #[test]
    fn the_focus_lands_in_the_middle_of_the_map() {
        let mut cam = OrbitCamera::new(CameraTweaks::default(), 2000.0);
        cam.look_at = Vec3::new(5000.0, -300.0, 120.0);
        let (m, texel) = shadow_matrix(&cam, Vec3::new(1.0, -3.0, 1.0));
        let c = m * cam.look_at.extend(1.0);
        let ndc = c.xyz() / c.w;
        assert!(ndc.x.abs() < 0.01 && ndc.y.abs() < 0.01, "{ndc:?}");
        assert!(ndc.z > 0.0 && ndc.z < 1.0, "{ndc:?}");
        assert!((texel - 2.0 * 1800.0 / SHADOW_MAP_SIZE as f32).abs() < 1e-3);
    }
}
