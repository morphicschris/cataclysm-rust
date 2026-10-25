//! Turning particle systems into [`RenderList`] entries (the drawing half of `partRender*`).
//!
//! Visibility and "hit" counting follow the original renderers (a system with no hits is
//! marked dead). Positions are converted to world space with the effect's N-Lips-scaled
//! coordinate system, except world-space systems (already in world space) and billboards,
//! which the original places with the unscaled coordinate system.

use hwc_math::random::{Randy, RAN_PARTICLE_STREAM};
use hwc_math::{HMatrix, Matrix, Vector};

use crate::compiler::Library;
use crate::particle::{self, pflag, sysflag, DataRef, DataSource, LensFlare, PartSystem, PartType, TR_INVALID};
use crate::render::{Blend, Bolt, Cube, Flare, Line, MeshInstance, Point, PrimKind, RenderList, Sprite};
use crate::runtime::{sof, Effect, Owner};

/// A data source with nothing in it (morph tables then read as empty).
struct NoData;

impl DataSource for NoData {
    fn dword(&self, _: DataRef, _: u32) -> u32 {
        0
    }
}

fn add(a: Vector, b: Vector) -> Vector {
    Vector::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn scale(a: Vector, k: f32) -> Vector {
    Vector::new(a.x * k, a.y * k, a.z * k)
}

/// The transform the original had on the GL stack for a non-world-space system.
fn effect_transform(e: &Effect, scaled: &Matrix, owner: Option<&Owner>) -> HMatrix {
    let local = HMatrix::from_matrix_and_pos(scaled, e.position);
    match owner {
        Some(o) if e.flags & sof::OWNER_COORDSYS != 0 => {
            let om = Matrix::copy_and_scale(&o.coordsys, o.nlips);
            HMatrix::from_matrix_and_pos(&om, o.position).mul_hmat(&local)
        }
        _ => local,
    }
}

fn texture_name(lib: &Library, tex: u32) -> Option<String> {
    if tex == TR_INVALID {
        None
    } else {
        lib.textures.get(tex as usize).cloned()
    }
}

fn mesh_name(lib: &Library, mesh: u32) -> Option<String> {
    if mesh == 0 || mesh == 0xffff_ffff {
        None
    } else {
        lib.meshes.get(mesh as usize - 1).cloned()
    }
}

/// Emit one system; returns the hit count (`0` kills the system).
///
/// Morphing meshes need the morph table to find their next frame, which this variant cannot
/// read: their next mesh is reported as the current one. [`emit_system_with`] is the full
/// version.
#[allow(clippy::too_many_arguments)]
pub fn emit_system(
    sys: &PartSystem,
    e: &Effect,
    scaled: &Matrix,
    owner: Option<&Owner>,
    lib: &Library,
    randy: &mut Randy,
    flares: &mut [LensFlare],
    out: &mut RenderList,
) -> u32 {
    let mut copy = sys.clone();
    emit_system_with(&mut copy, &NoData, e, scaled, owner, lib, randy, flares, out)
}

/// [`emit_system`] with access to the effect data. Like the original renderer, finding a
/// morphing mesh's next frame (`partAdvanceMeshMorph`) may count down the particle's loop
/// counter, hence `&mut`.
#[allow(clippy::too_many_arguments)]
pub fn emit_system_with(
    sys: &mut PartSystem,
    data: &dyn DataSource,
    e: &Effect,
    scaled: &Matrix,
    owner: Option<&Owner>,
    lib: &Library,
    randy: &mut Randy,
    flares: &mut [LensFlare],
    out: &mut RenderList,
) -> u32 {
    let kind = match sys.kind {
        PartType::Billboard => PrimKind::Sprites,
        PartType::Points => PrimKind::Points,
        PartType::Lines => PrimKind::Lines,
        PartType::EBolt => PrimKind::Bolts,
        PartType::Cubes => PrimKind::Cubes,
        PartType::Mesh => PrimKind::Meshes,
        PartType::LensFlare => PrimKind::Flares,
    };
    let before = out.kind_len(kind);
    let lit = sys.particles.first().is_some_and(|p| p.lit != 0);
    let alpha = sys.flags & sysflag::ALPHA != 0;
    let hits = emit_particles(sys, data, e, scaled, owner, lib, randy, flares, out);
    out.push_span(kind, before, lit, alpha);
    hits
}

#[allow(clippy::too_many_arguments)]
fn emit_particles(
    sys: &mut PartSystem,
    data: &dyn DataSource,
    e: &Effect,
    scaled: &Matrix,
    owner: Option<&Owner>,
    lib: &Library,
    randy: &mut Randy,
    flares: &mut [LensFlare],
    out: &mut RenderList,
) -> u32 {
    let world = sys.flags & sysflag::WORLDSPACE != 0;
    let xf = effect_transform(e, scaled, owner);
    let to_world = |p: Vector| if world { p } else { xf.transform_point(p) };
    let alpha_sys = sys.flags & sysflag::ALPHA != 0;
    let mut hits = 0u32;
    match sys.kind {
        PartType::Billboard => {
            let mut current = sys.tex;
            for p in &sys.particles {
                if p.lifespan < 0.0 || p.waitspan > 0.0 {
                    continue;
                }
                hits += 1;
                let position = if world {
                    p.position
                } else if e.flags & sof::OWNER_COORDSYS != 0 {
                    xf.transform_point(p.position)
                } else {
                    add(sys.mat.mul_vec(p.position), sys.position)
                };
                if p.tstruct.is_none() && p.tex != TR_INVALID {
                    current = p.tex;
                }
                let texture = texture_name(lib, current);
                let blend = if texture.is_some() {
                    if p.flags & pflag::ADDITIVE != 0 {
                        Blend::Additive
                    } else {
                        Blend::Alpha
                    }
                } else if p.icolor[3] == 1.0 {
                    Blend::Opaque
                } else {
                    Blend::Alpha
                };
                out.sprites.push(Sprite {
                    position,
                    size: [p.scale_x, p.scale_y],
                    rotation: p.rot,
                    color: p.icolor,
                    bias: p.bias,
                    texture,
                    uv: sys.uv,
                    slices: u32::from(sys.slices),
                    blend,
                    stipple: p.flags & pflag::STIPPLE != 0,
                    additive: p.flags & pflag::ADDITIVE != 0,
                    illum: p.illum,
                });
            }
        }
        PartType::Points => {
            for p in &sys.particles {
                if p.lifespan < 0.0 {
                    continue;
                }
                hits += 1;
                if p.waitspan > 0.0 {
                    continue;
                }
                let blend = if !alpha_sys {
                    Blend::Opaque
                } else if sys.particles[0].flags & pflag::ADDITIVE != 0 {
                    Blend::Additive
                } else {
                    Blend::Alpha
                };
                out.points.push(Point {
                    position: to_world(p.position),
                    size: if p.scale_x <= 3.0 { p.scale_x } else { 3.0 },
                    color: p.icolor,
                    blend,
                });
            }
        }
        PartType::Lines => {
            for p in &sys.particles {
                if p.lifespan < 0.0 {
                    continue;
                }
                hits += 1;
                if p.waitspan > 0.0 {
                    continue;
                }
                let end = if world {
                    let mut rv = p.w_vel;
                    rv.normalize();
                    if rv.x.is_nan()
                        || rv.y.is_nan()
                        || rv.z.is_nan()
                        || (rv.x.abs() < hwc_math::REALLY_SMALL && rv.y.abs() <= hwc_math::REALLY_SMALL && rv.z.abs() <= hwc_math::REALLY_SMALL)
                    {
                        rv.z = 1.0;
                    }
                    if !p.length.is_nan() {
                        rv = scale(rv, p.length);
                    }
                    add(p.position, rv)
                } else {
                    let mut rv = scale(p.rvec, p.vel_r);
                    rv.z += p.vel_lof;
                    if rv.x == 0.0 && rv.y == 0.0 && rv.z == 0.0 {
                        rv.z = 1.0;
                    }
                    rv.normalize();
                    add(p.position, scale(rv, p.length))
                };
                let blend = if !alpha_sys {
                    Blend::Opaque
                } else if p.flags & pflag::ADDITIVE != 0 {
                    Blend::Additive
                } else {
                    Blend::Alpha
                };
                out.lines.push(Line {
                    start: to_world(p.position),
                    end: to_world(end),
                    width: p.scale_x,
                    color: p.icolor,
                    blend,
                });
            }
        }
        PartType::EBolt => {
            for p in &sys.particles {
                if p.lifespan < 0.0 {
                    continue;
                }
                hits += 1;
                if p.waitspan > 0.0 {
                    continue;
                }
                let mut dir = if world {
                    let mut d = p.w_vel;
                    d.normalize();
                    d
                } else {
                    Vector::new(p.vel_r * p.rvec.x, p.vel_r * p.rvec.y, p.vel_r * p.rvec.z + p.vel_lof)
                };
                let tiny = |v: f32| v.abs() <= 1e-19;
                if dir.x.is_nan() || dir.y.is_nan() || dir.z.is_nan() || (dir.x.abs() < 1e-19 && tiny(dir.y) && tiny(dir.z)) {
                    dir.z = 1.0;
                }
                if !world {
                    dir.normalize();
                }
                let segs = p.segments;
                let step_len = p.length / f32::from(segs);
                if !step_len.is_nan() {
                    dir = scale(dir, step_len);
                }
                let t = p.turbulance;
                let amp = ((t * dir.x) * (t * dir.x) + (t * dir.y) * (t * dir.y) + (t * dir.z) * (t * dir.z)).sqrt() * 0.5;
                let lo = -amp;
                let k = (amp - lo) * 2.328_306_4e-10;
                let mut cur = p.position;
                let mut pts = vec![to_world(cur)];
                for i in 0..u32::from(segs) {
                    cur = add(cur, dir);
                    let mut v = cur;
                    if i + 1 < u32::from(segs) {
                        v.x = randy.random(RAN_PARTICLE_STREAM) as f32 * k + lo + cur.x;
                        v.y = randy.random(RAN_PARTICLE_STREAM) as f32 * k + lo + cur.y;
                        v.z = randy.random(RAN_PARTICLE_STREAM) as f32 * k + lo + cur.z;
                    }
                    pts.push(to_world(v));
                }
                out.bolts.push(Bolt {
                    points: pts,
                    width: p.scale_x,
                    color: p.icolor,
                    blend: if p.flags & pflag::ADDITIVE != 0 { Blend::Additive } else { Blend::Alpha },
                });
            }
        }
        PartType::Cubes => {
            for p in &sys.particles {
                if p.lifespan < 0.0 {
                    continue;
                }
                hits += 1;
                if p.waitspan > 0.0 {
                    continue;
                }
                out.cubes.push(Cube {
                    position: to_world(p.position),
                    size: p.scale_x * 0.5,
                    color: p.icolor,
                    blend: if alpha_sys { Blend::Alpha } else { Blend::Opaque },
                });
            }
        }
        PartType::Mesh => {
            let mut current = sys.tex;
            let (sys_mat, sys_pos, sys_flags) = (sys.mat, sys.position, sys.flags);
            for p in &mut sys.particles {
                if p.lifespan < 0.0 {
                    continue;
                }
                hits += 1;
                if p.waitspan > 0.0 || p.mesh == 0 || p.mesh == 0xffff_ffff {
                    continue;
                }
                if p.tstruct.is_none() && p.tex != TR_INVALID {
                    current = p.tex;
                }
                let mut heading = if world {
                    let mut v = p.w_vel;
                    if sys.flags & sysflag::WORLDSPACE_VELOCITY == 0 {
                        v = Vector::new(v.x - p.effect_vel.x, v.y - p.effect_vel.y, v.z - p.effect_vel.z);
                    }
                    v
                } else {
                    scale(p.rvec, p.vel_r)
                };
                if heading.x == 0.0 && heading.y == 0.0 && heading.z == 0.0 {
                    heading.z = 1.0;
                } else {
                    heading.normalize();
                }
                let local = if p.flags & pflag::XYZSCALE != 0 {
                    let rx = HMatrix::rot_about_x(-1.0, 0.0);
                    let s = HMatrix { m: [p.tumble[0], 0.0, 0.0, 0.0, 0.0, p.tumble[1], 0.0, 0.0, 0.0, 0.0, p.tumble[2], 0.0, 0.0, 0.0, 0.0, 1.0] };
                    HMatrix::from_matrix_and_pos(scaled, e.position).mul_hmat(&rx).mul_hmat(&s)
                } else {
                    let mut m = HMatrix::from_matrix_and_pos(&Matrix::IDENTITY, p.position);
                    if p.flags & (pflag::PSEUDO_BILLBOARD | pflag::TRUE_BILLBOARD) == 0 {
                        let o = Matrix::coord_sys_from_heading(heading);
                        m = m.mul_hmat(&HMatrix::from_matrix(&o));
                    }
                    for (axis, ang) in p.tumble.iter().enumerate() {
                        if *ang != 0.0 {
                            let (c, s) = (ang.cos(), ang.sin());
                            let r = match axis {
                                0 => HMatrix::rot_about_x(c, s),
                                1 => HMatrix::rot_about_y(c, s),
                                _ => HMatrix::rot_about_z(c, s),
                            };
                            m = m.mul_hmat(&r);
                        }
                    }
                    let sc = p.scale_x / e.nlips;
                    let len = if p.length != 5.0 { p.length / e.nlips } else { sc };
                    if p.scale_x != 1.0 {
                        let s = HMatrix { m: [sc, 0.0, 0.0, 0.0, 0.0, sc, 0.0, 0.0, 0.0, 0.0, len, 0.0, 0.0, 0.0, 0.0, 1.0] };
                        m = m.mul_hmat(&s);
                    }
                    if world {
                        m
                    } else {
                        xf.mul_hmat(&m)
                    }
                };
                let mut billboard_velocity = if world { p.w_vel } else { scale(p.rvec, p.vel_r) };
                if sys_flags & sysflag::WORLDSPACE_VELOCITY == 0 {
                    billboard_velocity = Vector::new(
                        billboard_velocity.x - p.effect_vel.x,
                        billboard_velocity.y - p.effect_vel.y,
                        billboard_velocity.z - p.effect_vel.z,
                    );
                }
                if billboard_velocity.x == 0.0 && billboard_velocity.y == 0.0 && billboard_velocity.z == 0.0 {
                    billboard_velocity.z = 1.0;
                } else {
                    billboard_velocity.normalize();
                }
                let (morph, morph_base) = match p.mstruct {
                    Some(r) => {
                        let next = particle::advance_mesh_morph(data, p);
                        let next_mesh = data.dword(r, (next as u32).wrapping_mul(8));
                        let frac = p.mesh_frame - (p.mesh_frame as i32) as f32;
                        let named = |m: u32| if m == 0 { mesh_name(lib, p.mesh) } else { mesh_name(lib, m) };
                        (Some((named(next_mesh), frac)), named(data.dword(r, 0)))
                    }
                    None => (None, None),
                };
                out.meshes.push(MeshInstance {
                    mesh: mesh_name(lib, p.mesh),
                    morph,
                    transform: local,
                    position: to_world(p.position),
                    heading,
                    color: p.icolor,
                    bias: p.bias,
                    texture: texture_name(lib, current),
                    blend: if alpha_sys {
                        if p.flags & pflag::ADDITIVE != 0 {
                            Blend::Additive
                        } else {
                            Blend::Alpha
                        }
                    } else {
                        Blend::Opaque
                    },
                    lit: p.lit != 0,
                    illum: p.illum,
                    specular_exponent: (p.flags & pflag::SPECULAR != 0).then_some(p.exponent),
                    color_scheme: p.color_scheme,
                    depth_write: p.flags & pflag::NO_DEPTH_WRITE == 0,
                    pseudo_billboard: p.flags & pflag::PSEUDO_BILLBOARD != 0,
                    true_billboard: p.flags & pflag::TRUE_BILLBOARD != 0,
                    additive: p.flags & pflag::ADDITIVE != 0,
                    alpha_system: alpha_sys,
                    xyz_scale: p.flags & pflag::XYZSCALE != 0,
                    effect_color: owner.map_or(0, |o| o.hyperspace_color),
                    morph_base,
                    billboard_velocity,
                    system_coordsys: sys_mat,
                    system_position: sys_pos,
                    local_position: p.position,
                    world_space: world,
                });
            }
        }
        PartType::LensFlare => {
            // The original only clears the first particle's flare flag and counts one hit.
            if let Some(fi) = sys.particles.first().and_then(|p| p.lens_flare) {
                if let Some(f) = flares.get_mut(fi) {
                    f.pending = 0;
                }
            }
            for p in &sys.particles {
                // The flare object holds the particle position as stored (world space for the
                // world-space systems that create flares).
                if let Some((fi, f)) = p.lens_flare.and_then(|fi| flares.get(fi).map(|f| (fi, f))) {
                    if f.enabled != 0 && p.lifespan >= 0.0 && p.waitspan <= 0.0 {
                        out.flares.push(Flare { position: f.position, intensity: f.intensity, index: fi });
                    }
                }
            }
            hits = 1;
        }
    }
    hits
}
