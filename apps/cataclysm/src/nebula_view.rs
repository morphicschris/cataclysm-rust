//! The simulation's nebulae handed to [`hwc_render::NebulaRenderer`], and the camera handed
//! back to the simulation (its render list and the tendrils it builds depend on it).

use hwc_math::Vector;
use hwc_render::nebula::{ChunkInput, CylinderInput, NebulaInput, NebulaParams, TendrilInput};
use hwc_render::{FxView, NebulaRenderer, OrbitCamera, Renderer};
use hwc_sim::nebula::{self, tendril_flags, NebulaCamera};
use hwc_sim::Universe;

fn vector(v: glam::Vec3) -> Vector {
    Vector::new(v.x, v.y, v.z)
}

fn camera_of(cam: &OrbitCamera) -> NebulaCamera {
    NebulaCamera {
        eye: vector(cam.eye()),
        look_at: vector(cam.look_at),
        clip_far: cam.tweaks.clip_far,
    }
}

/// `mrCamera` for the next universe update.
pub fn set_camera(u: &mut Universe, cam: &OrbitCamera) {
    u.nebulae.camera = Some(camera_of(cam));
}

/// The nebulae as renderer input for this camera.
pub fn inputs<'a>(u: &'a Universe, cam: &NebulaCamera) -> Vec<NebulaInput<'a>> {
    u.nebulae
        .list
        .iter()
        .map(|neb| {
            let drawn: Vec<usize> = nebula::drawn_tendrils(neb, cam).collect();
            NebulaInput {
                chunks: neb
                    .chunks
                    .iter()
                    .map(|c| ChunkInput {
                        position: c.position,
                        d_pos: c.d_pos,
                        tendrils: c.tendrils,
                        in_render_list: nebula::in_render_list(&u.nebulae.tweaks, cam, c.position),
                    })
                    .collect(),
                tendrils: neb
                    .tendrils
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        let (a, b) = t.ends().unwrap_or((usize::MAX, usize::MAX));
                        let lod = |k: usize| CylinderInput {
                            slices: t.lods[k].slices,
                            stacks: t.lods[k].stacks,
                            vertices: &t.lods[k].vertices,
                        };
                        TendrilInput {
                            a,
                            b,
                            inactive: t.flags & tendril_flags::INACTIVE != 0,
                            leading: t.flags & tendril_flags::LEADING != 0,
                            trailing: t.flags & tendril_flags::TRAILING != 0,
                            drawn: drawn.contains(&i),
                            real_colour: t.real_colour,
                            fade_factor: t.fade_factor,
                            lods: [lod(0), lod(1)],
                        }
                    })
                    .collect(),
            }
        })
        .collect()
}

/// Prepares the nebula pass (creating the renderer on first use); false when there is
/// nothing to draw.
pub fn prepare(
    slot: &mut Option<NebulaRenderer>,
    renderer: &Renderer,
    u: &Universe,
    cam: &OrbitCamera,
    size: (u32, u32),
) -> bool {
    if u.nebulae.list.is_empty() {
        return false;
    }
    let neb = slot.get_or_insert_with(|| NebulaRenderer::new(renderer));
    let ncam = camera_of(cam);
    let params = NebulaParams {
        eye: ncam.eye,
        distance_desaturation: u.nebulae.tweaks.distance_desaturation,
        bright: true,
    };
    let view = FxView::from_orbit(cam, size.0, size.1);
    neb.prepare(&inputs(u, &ncam), &params, &view);
    neb.vertex_count() > 0
}
