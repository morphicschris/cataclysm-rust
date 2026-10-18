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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_axes_map_to_world() {
        let m = mesh_to_world();
        assert_eq!(m.transform_vector3(Vec3::X), Vec3::Z);
        assert_eq!(m.transform_vector3(Vec3::Y), Vec3::Y);
        assert_eq!(m.transform_vector3(Vec3::Z), -Vec3::X);
        assert!((Mat3::from_mat4(m).determinant() - 1.0).abs() < 1e-6);
    }
}
