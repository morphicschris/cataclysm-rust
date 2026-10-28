//! The hull mesh the hit tests refine against (`meshShipSegmentCollide`, 0x4b0ea0): the
//! polygons of one level of a ship's `.lod` table, with each object's transform accumulated
//! down the mesh hierarchy.

use std::sync::Arc;

use hwc_formats::geo::Geo;

use super::enums::ShipType;
use super::lod::{LodInfo, LodType};
use super::source::DataSource;

/// The level the bullet, beam and missile tests use (0x748b20 / 0x748b24).
pub const HIT_TEST_LOD: usize = 3;
/// The fixed level for ship type 0x5c (0x87bc0c).
const TYPE_5C_LOD: usize = 4;

/// One polygon object, in the order the original walks them.
#[derive(Debug, Clone, PartialEq)]
pub struct HullObject {
    /// The object's transform in mesh space (column-major 4×4: the mother objects'
    /// `localMatrix` times its own).
    pub matrix: [f32; 16],
    pub vertices: Vec<[f32; 3]>,
    /// Face normals followed by vertex normals, as the mesh stores them.
    pub normals: Vec<[f32; 3]>,
    /// `(face normal index, corners)` per polygon.
    pub polygons: Vec<(u32, [u16; 3])>,
    /// A top-level object (`pSister` chain of the first object): the nearest-hit distance
    /// starts over before it and its daughters.
    pub starts_top_level: bool,
}

/// A ship type's hull for the hit tests.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HullMesh {
    pub objects: Vec<HullObject>,
}

impl HullMesh {
    /// The mesh of level `min(HIT_TEST_LOD, levels - 1)` of the type's table (`None` when that
    /// level is not a mesh or does not load).
    pub fn load(src: &dyn DataSource, lod: &LodInfo, ship_type: ShipType) -> Option<Arc<Self>> {
        let want = if ship_type.index() == 0x5c { TYPE_5C_LOD } else { HIT_TEST_LOD };
        let level = lod.levels.get(want.min(lod.levels.len().checked_sub(1)?))?;
        if level.kind != LodType::Mesh || !level.mesh_exists {
            return None;
        }
        let geo = Geo::parse(&src.read(level.mesh_file.as_deref()?)?).ok()?;
        Some(Arc::new(Self::from_geo(&geo)))
    }

    /// The objects in `meshShipSegmentCollide`'s order: for each object of the first object's
    /// sister chain, its daughters depth first (each daughter after its own daughters), then
    /// the object itself.
    pub fn from_geo(geo: &Geo) -> Self {
        let mut objects = Vec::new();
        let mut top = (!geo.objects.is_empty()).then_some(0);
        let mut guard = 0;
        while let Some(i) = top {
            let first = objects.len();
            let ident = identity();
            let m = mul(&ident, &geo.objects[i].local_matrix);
            let mut child = geo.objects[i].first_child;
            while let Some(c) = child {
                walk(geo, c, &m, &mut objects, 0);
                child = geo.objects.get(c).and_then(|o| o.next_sibling);
            }
            objects.push(object(geo, i, m));
            objects[first].starts_top_level = true;
            top = geo.objects[i].next_sibling;
            guard += 1;
            if guard > geo.objects.len() {
                break;
            }
        }
        Self { objects }
    }
}

/// `meshHierarchySegmentCollideR` (0x4b0e30).
fn walk(geo: &Geo, i: usize, parent: &[f32; 16], out: &mut Vec<HullObject>, depth: usize) {
    let Some(o) = geo.objects.get(i) else { return };
    if depth > geo.objects.len() {
        return;
    }
    let m = mul(parent, &o.local_matrix);
    let mut child = o.first_child;
    while let Some(c) = child {
        walk(geo, c, &m, out, depth + 1);
        child = geo.objects.get(c).and_then(|x| x.next_sibling);
    }
    out.push(object(geo, i, m));
}

fn object(geo: &Geo, i: usize, matrix: [f32; 16]) -> HullObject {
    let o = &geo.objects[i];
    HullObject {
        matrix,
        vertices: o.vertices.iter().map(|v| v.position).collect(),
        normals: o.normals.clone(),
        polygons: o.polygons.iter().map(|p| (p.face_normal, p.vertices)).collect(),
        starts_top_level: false,
    }
}

fn identity() -> [f32; 16] {
    let mut m = [0.0; 16];
    m[0] = 1.0;
    m[5] = 1.0;
    m[10] = 1.0;
    m[15] = 1.0;
    m
}

/// Column-major `a × b` (`glMultMatrixf` onto `a`).
fn mul(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut r = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut s = 0.0f32;
            for k in 0..4 {
                s += a[k * 4 + row] * b[col * 4 + k];
            }
            r[col * 4 + row] = s;
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn product_is_column_major() {
        let mut t = identity();
        t[12] = 5.0;
        let mut s = identity();
        s[0] = 2.0;
        // Translate after scaling: (1, 0, 0) -> (2 + 5, 0, 0).
        let m = mul(&t, &s);
        assert_eq!(m[0], 2.0);
        assert_eq!(m[12], 5.0);
    }
}
