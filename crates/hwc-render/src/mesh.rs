//! Flattening `.geo` meshes into GPU-ready triangle lists grouped by material.

use bytemuck::{Pod, Zeroable};
use glam::{Mat3, Mat4, Vec3};
use hwc_formats::geo;

use crate::assets::{MeshPart, ShipModel};

/// Mesh space to the engine's world space for a ship with identity orientation: the mesh's
/// up (+X), right (+Y) and heading (+Z) become world +Z, +Y and -X, the same turn the ship
/// viewer applies with a -90° rotation about Y.
pub fn mesh_to_world() -> Mat4 {
    Mat4::from_cols(
        glam::Vec4::new(0.0, 0.0, 1.0, 0.0),
        glam::Vec4::new(0.0, 1.0, 0.0, 0.0),
        glam::Vec4::new(-1.0, 0.0, 0.0, 0.0),
        glam::Vec4::W,
    )
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct ShipVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

/// How a material is drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialInfo {
    pub ambient: [f32; 3],
    pub diffuse: [f32; 3],
    pub flags: u16,
    /// Index into the part's texture list (same as the material index) if it has a texture.
    pub textured: bool,
    /// The texture carries alpha, so the material is blended.
    pub alpha: bool,
}

impl MaterialInfo {
    pub fn two_sided(&self) -> bool {
        self.flags & geo::MATERIAL_TWO_SIDED != 0
    }
    pub fn self_illuminated(&self) -> bool {
        self.flags & geo::MATERIAL_SELF_ILLUM != 0
    }
    pub fn team_colored(&self) -> bool {
        self.flags & (geo::MATERIAL_BASE_COLOR | geo::MATERIAL_STRIPE_COLOR) != 0
    }
}

/// A run of vertices sharing one material of one part.
#[derive(Debug, Clone, PartialEq)]
pub struct Batch {
    pub part: usize,
    pub material: usize,
    pub first_vertex: u32,
    pub vertex_count: u32,
}

/// All of a ship's triangles (in mesh space) and per-part material descriptions.
#[derive(Debug, Clone, Default)]
pub struct ShipGeometry {
    pub vertices: Vec<ShipVertex>,
    pub batches: Vec<Batch>,
    pub materials: Vec<Vec<MaterialInfo>>,
}

fn rgb(c: [u8; 4]) -> [f32; 3] {
    [c[0], c[1], c[2]].map(|v| f32::from(v) / 255.0)
}

fn part_materials(part: &MeshPart) -> Vec<MaterialInfo> {
    part.geo
        .materials
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let tex = part.textures.get(i).and_then(Option::as_ref);
            MaterialInfo {
                ambient: rgb(m.ambient),
                diffuse: rgb(m.diffuse),
                flags: m.flags,
                textured: tex.is_some(),
                alpha: tex.is_some_and(|t| t.has_alpha()),
            }
        })
        .collect()
}

/// Builds triangle lists. Smooth materials take per-vertex normals, others the face normal on
/// all three corners, which gives the flat look of `GL_FLAT` shading.
pub fn build_geometry(model: &ShipModel) -> ShipGeometry {
    let mut out = ShipGeometry::default();
    for (part_index, part) in model.parts.iter().enumerate() {
        let materials = part_materials(part);
        let mut per_material: Vec<Vec<ShipVertex>> = vec![Vec::new(); materials.len()];
        for (object, &matrix) in part.geo.objects.iter().zip(&part.object_matrices) {
            let normal_matrix = Mat3::from_mat4(matrix).inverse().transpose();
            for poly in &object.polygons {
                let m = poly.material as usize;
                if m >= materials.len() {
                    continue;
                }
                let smooth = materials[m].flags & geo::MATERIAL_SMOOTHING != 0;
                let face = object.normals.get(poly.face_normal as usize).copied();
                for (corner, &vi) in poly.vertices.iter().enumerate() {
                    let Some(v) = object.vertices.get(vi as usize) else {
                        continue;
                    };
                    let n = if smooth {
                        object.normals.get(v.normal as usize).copied().or(face)
                    } else {
                        face
                    }
                    .unwrap_or([0.0, 0.0, 1.0]);
                    let n = (normal_matrix * Vec3::from_array(n)).normalize_or_zero();
                    per_material[m].push(ShipVertex {
                        position: matrix.transform_point3(Vec3::from_array(v.position)).into(),
                        normal: n.into(),
                        uv: poly.uvs[corner],
                    });
                }
            }
        }
        for (material, verts) in per_material.into_iter().enumerate() {
            if verts.len() < 3 {
                continue;
            }
            let count = verts.len() - verts.len() % 3;
            out.batches.push(Batch {
                part: part_index,
                material,
                first_vertex: out.vertices.len() as u32,
                vertex_count: count as u32,
            });
            out.vertices.extend_from_slice(&verts[..count]);
        }
        out.materials.push(materials);
    }
    out
}

/// Per-vertex tint attributes (Cataclysm's per-vertex colour delta and effect flags), kept as a
/// second vertex stream beside [`ShipVertex`].
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Pod, Zeroable, Default)]
pub struct TintVertex {
    /// Packed RGBA bytes; R, G and B are signed deltas.
    pub color: u32,
    pub flags: u32,
    /// Baked ambient occlusion, 0 (open) to 255 (closed in) ([`crate::detail::occlusion`]).
    pub occlusion: u32,
}

/// One mesh object of a level, in the ship's object hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub struct SkeletonObject {
    pub name: String,
    /// Index of the parent object (in the same skeleton).
    pub parent: Option<usize>,
    /// The object's matrix relative to its parent when no animation drives it.
    pub rest: Mat4,
}

/// The object hierarchy of one level of detail: every part's objects, in part order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Skeleton {
    pub objects: Vec<SkeletonObject>,
}

impl Skeleton {
    pub fn from_model(model: &ShipModel) -> Self {
        let mut objects = Vec::new();
        for part in &model.parts {
            let base = objects.len();
            let count = part.geo.objects.len();
            for o in &part.geo.objects {
                objects.push(SkeletonObject {
                    name: o.name.clone(),
                    parent: o.parent.filter(|&p| p < count).map(|p| base + p),
                    rest: Mat4::from_cols_array(&o.local_matrix),
                });
            }
        }
        Self { objects }
    }

    /// Index of the first object with this name (case-insensitive).
    pub fn find(&self, name: &str) -> Option<usize> {
        self.objects
            .iter()
            .position(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// Each object's matrix in ship (mesh) space: the parent chain of local matrices, where
    /// `local(i)` supplies an object's current local matrix (its rest matrix when not animated).
    pub fn resolve(&self, local: impl Fn(usize) -> Mat4) -> Vec<Mat4> {
        let n = self.objects.len();
        let mut out: Vec<Option<Mat4>> = vec![None; n];
        for start in 0..n {
            // Walk up to the first resolved ancestor, then resolve back down.
            let mut chain = Vec::new();
            let mut cur = Some(start);
            while let Some(i) = cur {
                if out[i].is_some() || chain.len() > n {
                    break;
                }
                chain.push(i);
                cur = self.objects[i].parent;
            }
            for &i in chain.iter().rev() {
                let parent = self.objects[i]
                    .parent
                    .and_then(|p| out[p])
                    .unwrap_or(Mat4::IDENTITY);
                out[i] = Some(parent * local(i));
            }
        }
        out.into_iter()
            .map(|m| m.unwrap_or(Mat4::IDENTITY))
            .collect()
    }

    /// Rest-pose matrices in ship space.
    pub fn rest_matrices(&self) -> Vec<Mat4> {
        self.resolve(|i| self.objects[i].rest)
    }
}

/// A run of vertices sharing one object and one material.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectBatch {
    /// Index into [`Skeleton::objects`].
    pub object: usize,
    pub part: usize,
    pub material: usize,
    pub first_vertex: u32,
    pub vertex_count: u32,
}

/// A level's triangles in object-local space, so object matrices can change every frame.
#[derive(Debug, Clone, Default)]
pub struct LevelGeometry {
    pub vertices: Vec<ShipVertex>,
    /// Parallel to `vertices`.
    pub tints: Vec<TintVertex>,
    pub batches: Vec<ObjectBatch>,
    pub materials: Vec<Vec<MaterialInfo>>,
    pub skeleton: Skeleton,
    /// Mesh edges (object index, local end points) for effects that crawl over the hull.
    pub edges: Vec<(usize, Vec3, Vec3)>,
}

/// Most hull edges kept per level for edge effects.
const MAX_EDGES: usize = 512;

/// Builds per-object triangle lists in object space (unlike [`build_geometry`], which bakes the
/// rest pose into the vertices).
pub fn build_level_geometry(model: &ShipModel) -> LevelGeometry {
    build_level_geometry_smoothed(model, 0.0)
}

/// [`build_level_geometry`], with each object's flat-shaded faces given smoothed normals
/// where they meet at less than `crease_degrees` ([`crate::look::smoothed_normals`]; 0: off).
pub fn build_level_geometry_smoothed(model: &ShipModel, crease_degrees: f32) -> LevelGeometry {
    build_level_geometry_detailed(
        model,
        Detail {
            crease_degrees,
            ..Detail::default()
        },
    )
}

/// The geometry the modern ship look works out at upload.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Detail {
    /// Smoothed normals below this crease angle (0: off).
    pub crease_degrees: f32,
    /// Bake ambient occlusion into the vertices ([`crate::detail::occlusion`]).
    pub occlusion: bool,
}

/// [`build_level_geometry`] with the modern look's geometry: smoothed normals and baked
/// occlusion as `detail` asks.
pub fn build_level_geometry_detailed(model: &ShipModel, detail: Detail) -> LevelGeometry {
    let crease_degrees = detail.crease_degrees;
    let mut out = LevelGeometry {
        skeleton: Skeleton::from_model(model),
        ..Default::default()
    };
    let mut object_base = 0;
    let mut all_edges = Vec::new();
    for (part_index, part) in model.parts.iter().enumerate() {
        let materials = part_materials(part);
        for (oi, object) in part.geo.objects.iter().enumerate() {
            let global = object_base + oi;
            let mut per_material: Vec<Vec<(ShipVertex, TintVertex)>> =
                vec![Vec::new(); materials.len()];
            for poly in &object.polygons {
                let m = poly.material as usize;
                if m >= materials.len() {
                    continue;
                }
                let smooth = materials[m].flags & geo::MATERIAL_SMOOTHING != 0;
                let face = object.normals.get(poly.face_normal as usize).copied();
                let mut corners = Vec::with_capacity(3);
                for (corner, &vi) in poly.vertices.iter().enumerate() {
                    let Some(v) = object.vertices.get(vi as usize) else {
                        continue;
                    };
                    let n = if smooth {
                        object.normals.get(v.normal as usize).copied().or(face)
                    } else {
                        face
                    }
                    .unwrap_or([0.0, 0.0, 1.0]);
                    let tint = object
                        .vertex_attrs
                        .get(vi as usize)
                        .map(|a| TintVertex {
                            color: u32::from_le_bytes(a.color),
                            flags: a.flags,
                            occlusion: 0,
                        })
                        .unwrap_or_default();
                    corners.push(Vec3::from_array(v.position));
                    per_material[m].push((
                        ShipVertex {
                            position: v.position,
                            normal: Vec3::from_array(n).normalize_or_zero().into(),
                            uv: poly.uvs[corner],
                        },
                        tint,
                    ));
                }
                if corners.len() == 3 {
                    all_edges.push((global, corners[0], corners[1]));
                }
            }
            if crease_degrees > 0.0 {
                smooth_flat_materials(&mut per_material, &materials, crease_degrees);
            }
            for (material, verts) in per_material.into_iter().enumerate() {
                if verts.len() < 3 {
                    continue;
                }
                let count = verts.len() - verts.len() % 3;
                out.batches.push(ObjectBatch {
                    object: global,
                    part: part_index,
                    material,
                    first_vertex: out.vertices.len() as u32,
                    vertex_count: count as u32,
                });
                for (v, t) in &verts[..count] {
                    out.vertices.push(*v);
                    out.tints.push(*t);
                }
            }
        }
        object_base += part.geo.objects.len();
        out.materials.push(materials);
    }
    if detail.occlusion {
        bake_occlusion(&mut out);
    }
    // Keep an evenly spread subset of edges.
    let step = all_edges.len().div_ceil(MAX_EDGES).max(1);
    out.edges = all_edges.into_iter().step_by(step).collect();
    out
}

/// Bakes ambient occlusion into a level's tints, with the objects in their rest pose: rays
/// reach a fifth of the level's radius.
fn bake_occlusion(level: &mut LevelGeometry) {
    let rest = level.skeleton.rest_matrices();
    let mut world = vec![(Vec3::ZERO, Vec3::Z); level.vertices.len()];
    for b in &level.batches {
        let m = rest.get(b.object).copied().unwrap_or(Mat4::IDENTITY);
        let nm = Mat3::from_mat4(m).inverse().transpose();
        let range = b.first_vertex as usize..(b.first_vertex + b.vertex_count) as usize;
        for i in range {
            let v = &level.vertices[i];
            world[i] = (
                m.transform_point3(Vec3::from_array(v.position)),
                (nm * Vec3::from_array(v.normal)).normalize_or(Vec3::Z),
            );
        }
    }
    let (min, max) = world.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), (p, _)| {
        (lo.min(*p), hi.max(*p))
    });
    let reach = (max - min).length() * 0.5 * 0.2;
    let tris: Vec<[Vec3; 3]> = world.chunks_exact(3).map(|t| [t[0].0, t[1].0, t[2].0]).collect();
    // One ray cast per distinct point and normal.
    let key = |(p, n): &(Vec3, Vec3)| {
        let q = |v: Vec3, s: f32| (v * s).round().as_ivec3().to_array();
        (q(*p, 1024.0), q(*n, 64.0))
    };
    let mut index = std::collections::HashMap::new();
    let mut points = Vec::new();
    let slots: Vec<usize> = world
        .iter()
        .map(|w| {
            *index.entry(key(w)).or_insert_with(|| {
                points.push(*w);
                points.len() - 1
            })
        })
        .collect();
    let ao = crate::detail::occlusion(tris, &points, reach);
    for (t, slot) in level.tints.iter_mut().zip(slots) {
        t.occlusion = (ao[slot].clamp(0.0, 1.0) * 255.0).round() as u32;
    }
}

/// Smooths the normals of one object's flat-shaded materials together (the smooth ones keep
/// the normals they were modelled with). A smoothed normal that turns against the modelled
/// face is flipped, in case a mesh winds its faces the other way round.
fn smooth_flat_materials(
    per_material: &mut [Vec<(ShipVertex, TintVertex)>],
    materials: &[MaterialInfo],
    crease_degrees: f32,
) {
    let flat = |m: usize| materials[m].flags & geo::MATERIAL_SMOOTHING == 0;
    let mut positions = Vec::new();
    for (m, verts) in per_material.iter().enumerate() {
        if flat(m) {
            let whole = verts.len() - verts.len() % 3;
            positions.extend(verts[..whole].iter().map(|(v, _)| v.position));
        }
    }
    if positions.is_empty() {
        return;
    }
    let mut normals = crate::look::smoothed_normals(&positions, crease_degrees).into_iter();
    for (m, verts) in per_material.iter_mut().enumerate() {
        if !flat(m) {
            continue;
        }
        let whole = verts.len() - verts.len() % 3;
        for (v, _) in &mut verts[..whole] {
            let Some(n) = normals.next() else { return };
            let n = Vec3::from_array(n);
            let old = Vec3::from_array(v.normal);
            v.normal = if n.dot(old) < 0.0 { -n } else { n }.into();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skeleton_resolves_parent_chains() {
        let t = |x: f32| Mat4::from_translation(Vec3::new(x, 0.0, 0.0));
        let sk = Skeleton {
            objects: vec![
                SkeletonObject {
                    name: "child".into(),
                    parent: Some(1),
                    rest: t(1.0),
                },
                SkeletonObject {
                    name: "root".into(),
                    parent: None,
                    rest: t(10.0),
                },
            ],
        };
        let m = sk.rest_matrices();
        assert_eq!(m[0].transform_point3(Vec3::ZERO), Vec3::new(11.0, 0.0, 0.0));
        assert_eq!(sk.find("ROOT"), Some(1));
    }

    #[test]
    fn mesh_axes_map_to_world() {
        let m = mesh_to_world();
        assert_eq!(m.transform_vector3(Vec3::X), Vec3::Z);
        assert_eq!(m.transform_vector3(Vec3::Y), Vec3::Y);
        assert_eq!(m.transform_vector3(Vec3::Z), -Vec3::X);
        assert!((Mat3::from_mat4(m).determinant() - 1.0).abs() < 1e-6);
    }
}
