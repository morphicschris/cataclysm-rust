//! `.geo` / `.peo` meshes.
//!
//! A mesh file is a 68-byte header, an array of polygon objects, then material, vertex, normal,
//! polygon and string data reached through absolute file offsets. All values are little-endian.
//! `.peo` files are the "paged" variant (identifier `RMF99ba`) whose materials name packed texture
//! pages and whose polygons carry UV-seam fix-up flags; the binary layout is identical.
//!
//! Header:
//!
//! | offset | type    | field                                                     |
//! |--------|---------|-----------------------------------------------------------|
//! | 0      | [u8; 8] | identifier, `"RMF97ba\0"` or `"RMF99ba\0"` (paged)        |
//! | 8      | u32     | version: `0x500` (one file uses `0x402`, same layout)     |
//! | 12     | u32     | offset of the mesh name, 0 if none                        |
//! | 16     | u32     | file size minus this header                               |
//! | 20     | u32     | per-instance working size                                 |
//! | 24     | u32     | public material count                                     |
//! | 28     | u32     | local material count                                      |
//! | 32     | u32     | offset of the public materials                            |
//! | 36     | u32     | offset of the local materials (public ones follow them)   |
//! | 40     | u32     | polygon object count                                      |
//! | 44     | [u8;24] | reserved                                                  |
//!
//! Polygon object (116 bytes, starting at offset 68). Compared with the Homeworld 1 layout it
//! gains one list offset, the per-vertex attribute list, between the vertex and normal lists:
//!
//! | offset | type      | field                                                  |
//! |--------|-----------|--------------------------------------------------------|
//! | 0      | u32       | offset of the object name                              |
//! | 4      | u8        | hierarchy flags                                        |
//! | 5      | u8, u16   | runtime scratch (object index, name CRC)               |
//! | 8      | u32 × 4   | vertex, face-normal, vertex-normal and polygon counts  |
//! | 24     | u32       | offset of the vertex list (16 bytes each)              |
//! | 28     | u32       | offset of the vertex attribute list (16 bytes each)    |
//! | 32     | u32       | offset of the normal list (16 bytes each)              |
//! | 36     | u32       | offset of the polygon list (40 bytes each)             |
//! | 40     | u32 × 3   | offsets of the parent, first child and next sibling    |
//! | 52     | f32 × 16  | local transform, column-major                          |
//!
//! The normal list holds the face normals followed by the vertex normals; polygons and vertices
//! index into the whole list.

use thiserror::Error;

pub const IDENTIFIER: &[u8; 8] = b"RMF97ba\0";
pub const IDENTIFIER_PAGED: &[u8; 8] = b"RMF99ba\0";
pub const VERSION: u32 = 0x500;
pub const VERSION_OLD: u32 = 0x402;
pub const HEADER_LEN: usize = 68;
pub const OBJECT_LEN: usize = 116;
pub const MATERIAL_LEN: usize = 32;
pub const VERTEX_LEN: usize = 16;
pub const VERTEX_ATTR_LEN: usize = 16;
pub const NORMAL_LEN: usize = 16;
pub const POLYGON_LEN: usize = 40;

/// Material flag: the texture animates.
pub const MATERIAL_ANIMATING_TEXTURE: u16 = 0x01;
/// Material flag: smooth (Gouraud) shading.
pub const MATERIAL_SMOOTHING: u16 = 0x02;
/// Material flag: flat shading allowed.
pub const MATERIAL_ALLOW_FLAT: u16 = 0x04;
/// Material flag: render both faces.
pub const MATERIAL_TWO_SIDED: u16 = 0x08;
/// Material flag: surface takes the team base colour.
pub const MATERIAL_BASE_COLOR: u16 = 0x10;
/// Material flag: surface takes the team stripe colour.
pub const MATERIAL_STRIPE_COLOR: u16 = 0x20;
/// Material flag: self-illuminated.
pub const MATERIAL_SELF_ILLUM: u16 = 0x40;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a GEO header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u32),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("unterminated string at offset {0}")]
    UnterminatedString(usize),
    #[error("{what} offset {offset:#x} does not point at a polygon object")]
    BadObjectLink { what: &'static str, offset: u32 },
    #[error("object {object}: {what} index {index} out of range (limit {limit})")]
    IndexOutOfRange {
        object: usize,
        what: &'static str,
        index: u32,
        limit: u32,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: Option<String>,
    /// RGBA.
    pub ambient: [u8; 4],
    pub diffuse: [u8; 4],
    pub specular: [u8; 4],
    pub k_alpha: f32,
    /// Texture name relative to the mesh (no extension), if textured.
    pub texture: Option<String>,
    pub flags: u16,
    /// Number of fully self-illuminated palette colours in the texture.
    pub full_ambient_count: u8,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Index into [`PolygonObject::normals`].
    pub normal: u32,
}

/// Per-vertex attributes new in Cataclysm meshes. The colour is mostly opaque black; the flag
/// values seen are 0, 1, 2, 3, 0x10, 0x12 and 0x13. The trailing words are always zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttr {
    /// RGBA.
    pub color: [u8; 4],
    pub flags: u32,
    pub reserved: [u32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Polygon {
    /// Index into [`PolygonObject::normals`].
    pub face_normal: u32,
    pub vertices: [u16; 3],
    /// Index into [`Geo::materials`].
    pub material: u16,
    /// Texture coordinates (s, t) for each corner.
    pub uvs: [[f32; 2]; 3],
    /// Zero in `.geo`; UV-seam fix-up bits in paged `.peo` meshes.
    pub flags: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PolygonObject {
    pub name: String,
    pub flags: u8,
    pub face_normal_count: u32,
    pub vertex_normal_count: u32,
    pub vertices: Vec<Vertex>,
    /// Same length as `vertices`.
    pub vertex_attrs: Vec<VertexAttr>,
    /// Face normals followed by vertex normals.
    pub normals: Vec<[f32; 3]>,
    pub polygons: Vec<Polygon>,
    /// Indices into [`Geo::objects`].
    pub parent: Option<usize>,
    pub first_child: Option<usize>,
    pub next_sibling: Option<usize>,
    /// Column-major 4×4 transform relative to the parent.
    pub local_matrix: [f32; 16],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Geo {
    pub version: u32,
    /// True for the `RMF99ba` (paged texture) variant.
    pub paged: bool,
    pub name: Option<String>,
    pub local_size: u32,
    pub local_material_count: u32,
    pub public_material_count: u32,
    /// Local materials followed by public ones; polygons index this list.
    pub materials: Vec<Material>,
    pub objects: Vec<PolygonObject>,
}

struct Reader<'a> {
    data: &'a [u8],
}

impl<'a> Reader<'a> {
    fn bytes(&self, what: &'static str, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.data.get(offset..end))
            .ok_or(Error::OutOfBounds {
                what,
                offset,
                len,
                file_len: self.data.len(),
            })
    }

    fn table(
        &self,
        what: &'static str,
        offset: u32,
        count: u32,
        stride: usize,
    ) -> Result<std::slice::ChunksExact<'a, u8>> {
        let len = (count as usize)
            .checked_mul(stride)
            .ok_or(Error::OutOfBounds {
                what,
                offset: offset as usize,
                len: usize::MAX,
                file_len: self.data.len(),
            })?;
        Ok(self.bytes(what, offset as usize, len)?.chunks_exact(stride))
    }

    fn string(&self, offset: u32) -> Result<String> {
        let rest = self.data.get(offset as usize..).ok_or(Error::OutOfBounds {
            what: "string",
            offset: offset as usize,
            len: 1,
            file_len: self.data.len(),
        })?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::UnterminatedString(offset as usize))?;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }

    fn opt_string(&self, offset: u32) -> Result<Option<String>> {
        if offset == 0 {
            Ok(None)
        } else {
            self.string(offset).map(Some)
        }
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn rgba_at(b: &[u8], at: usize) -> [u8; 4] {
    b[at..at + 4].try_into().unwrap()
}

impl Geo {
    pub fn parse(data: &[u8]) -> Result<Geo> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[0..8].try_into().unwrap();
        let paged = match &ident {
            i if i == IDENTIFIER => false,
            i if i == IDENTIFIER_PAGED => true,
            _ => return Err(Error::BadIdentifier(ident)),
        };
        let version = u32_at(data, 8);
        if version != VERSION && version != VERSION_OLD {
            return Err(Error::BadVersion(version));
        }
        let r = Reader { data };
        let name = r.opt_string(u32_at(data, 12))?;
        let local_size = u32_at(data, 20);
        let public_material_count = u32_at(data, 24);
        let local_material_count = u32_at(data, 28);
        let local_material_off = u32_at(data, 36);
        let object_count = u32_at(data, 40);

        let material_count = local_material_count.saturating_add(public_material_count);
        let materials = r
            .table(
                "materials",
                local_material_off,
                material_count,
                MATERIAL_LEN,
            )?
            .map(|m| {
                Ok(Material {
                    name: r.opt_string(u32_at(m, 0))?,
                    ambient: rgba_at(m, 4),
                    diffuse: rgba_at(m, 8),
                    specular: rgba_at(m, 12),
                    k_alpha: f32_at(m, 16),
                    texture: r.opt_string(u32_at(m, 20))?,
                    flags: u16_at(m, 24),
                    full_ambient_count: m[26],
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let link = |what: &'static str, offset: u32| -> Result<Option<usize>> {
            if offset == 0 {
                return Ok(None);
            }
            let rel = (offset as usize).checked_sub(HEADER_LEN);
            match rel {
                Some(rel) if rel % OBJECT_LEN == 0 && rel / OBJECT_LEN < object_count as usize => {
                    Ok(Some(rel / OBJECT_LEN))
                }
                _ => Err(Error::BadObjectLink { what, offset }),
            }
        };

        let objects = r
            .table(
                "polygon objects",
                HEADER_LEN as u32,
                object_count,
                OBJECT_LEN,
            )?
            .enumerate()
            .map(|(index, o)| {
                let vertex_count = u32_at(o, 8);
                let face_normal_count = u32_at(o, 12);
                let vertex_normal_count = u32_at(o, 16);
                let polygon_count = u32_at(o, 20);
                let normal_count = face_normal_count.saturating_add(vertex_normal_count);

                let check = |what, index_value: u32, limit: u32| {
                    if index_value < limit {
                        Ok(())
                    } else {
                        Err(Error::IndexOutOfRange {
                            object: index,
                            what,
                            index: index_value,
                            limit,
                        })
                    }
                };

                let vertices = r
                    .table("vertices", u32_at(o, 24), vertex_count, VERTEX_LEN)?
                    .map(|v| {
                        let normal = u32_at(v, 12);
                        check("vertex normal", normal, normal_count)?;
                        Ok(Vertex {
                            position: [f32_at(v, 0), f32_at(v, 4), f32_at(v, 8)],
                            normal,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                let vertex_attrs = r
                    .table(
                        "vertex attributes",
                        u32_at(o, 28),
                        vertex_count,
                        VERTEX_ATTR_LEN,
                    )?
                    .map(|a| VertexAttr {
                        color: rgba_at(a, 0),
                        flags: u32_at(a, 4),
                        reserved: [u32_at(a, 8), u32_at(a, 12)],
                    })
                    .collect();
                let normals = r
                    .table("normals", u32_at(o, 32), normal_count, NORMAL_LEN)?
                    .map(|n| [f32_at(n, 0), f32_at(n, 4), f32_at(n, 8)])
                    .collect();
                let polygons = r
                    .table("polygons", u32_at(o, 36), polygon_count, POLYGON_LEN)?
                    .map(|p| {
                        let face_normal = u32_at(p, 0);
                        check("face normal", face_normal, normal_count)?;
                        let vertices = [u16_at(p, 4), u16_at(p, 6), u16_at(p, 8)];
                        for &v in &vertices {
                            check("polygon vertex", v as u32, vertex_count)?;
                        }
                        let material = u16_at(p, 10);
                        check("material", material as u32, material_count)?;
                        Ok(Polygon {
                            face_normal,
                            vertices,
                            material,
                            uvs: [
                                [f32_at(p, 12), f32_at(p, 16)],
                                [f32_at(p, 20), f32_at(p, 24)],
                                [f32_at(p, 28), f32_at(p, 32)],
                            ],
                            flags: u16_at(p, 36),
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;

                let mut local_matrix = [0.0; 16];
                for (i, m) in local_matrix.iter_mut().enumerate() {
                    *m = f32_at(o, 52 + i * 4);
                }

                Ok(PolygonObject {
                    name: r.string(u32_at(o, 0))?,
                    flags: o[4],
                    face_normal_count,
                    vertex_normal_count,
                    vertices,
                    vertex_attrs,
                    normals,
                    polygons,
                    parent: link("parent", u32_at(o, 40))?,
                    first_child: link("child", u32_at(o, 44))?,
                    next_sibling: link("sibling", u32_at(o, 48))?,
                    local_matrix,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Geo {
            version,
            paged,
            name,
            local_size,
            local_material_count,
            public_material_count,
            materials,
            objects,
        })
    }

    /// Indices of objects without a parent, in file order.
    pub fn roots(&self) -> impl Iterator<Item = usize> + '_ {
        self.objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.parent.is_none())
            .map(|(i, _)| i)
    }

    /// Direct children of `object`, following the first-child / next-sibling chain.
    pub fn children(&self, object: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let mut next = self.objects.get(object).and_then(|o| o.first_child);
        while let Some(i) = next {
            if out.contains(&i) {
                break;
            }
            out.push(i);
            next = self.objects[i].next_sibling;
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(v: &mut Vec<u8>, at: usize, bytes: &[u8]) {
        if v.len() < at + bytes.len() {
            v.resize(at + bytes.len(), 0);
        }
        v[at..at + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn single_triangle() {
        let mut v = Vec::new();
        put(&mut v, 0, IDENTIFIER);
        put(&mut v, 8, &VERSION.to_le_bytes());
        let mat = HEADER_LEN + OBJECT_LEN;
        let verts = mat + MATERIAL_LEN;
        let attrs = verts + 3 * VERTEX_LEN;
        let norms = attrs + 3 * VERTEX_ATTR_LEN;
        let polys = norms + NORMAL_LEN;
        let strings = polys + POLYGON_LEN;
        for (at, val) in [(24, 1u32), (32, mat as u32), (36, mat as u32), (40, 1)] {
            put(&mut v, at, &val.to_le_bytes());
        }
        let o = HEADER_LEN;
        for (at, val) in [
            (0, strings as u32),
            (8, 3),
            (12, 1),
            (20, 1),
            (24, verts as u32),
            (28, attrs as u32),
            (32, norms as u32),
            (36, polys as u32),
        ] {
            put(&mut v, o + at, &val.to_le_bytes());
        }
        put(&mut v, o + 52, &1.0f32.to_le_bytes());
        put(&mut v, mat + 20, &(strings as u32 + 5).to_le_bytes());
        put(&mut v, verts + VERTEX_LEN, &2.0f32.to_le_bytes());
        put(&mut v, polys + 4, &[0, 0, 1, 0, 2, 0]);
        put(&mut v, strings, b"root\0tex\0");
        let g = Geo::parse(&v).unwrap();
        assert_eq!(g.objects.len(), 1);
        assert_eq!(g.objects[0].name, "root");
        assert_eq!(g.materials[0].texture.as_deref(), Some("tex"));
        assert_eq!(g.objects[0].vertices[1].position, [2.0, 0.0, 0.0]);
        assert_eq!(g.objects[0].polygons[0].vertices, [0, 1, 2]);
        assert_eq!(g.roots().collect::<Vec<_>>(), vec![0]);

        put(&mut v, polys + 8, &[3, 0]);
        assert!(matches!(Geo::parse(&v), Err(Error::IndexOutOfRange { .. })));
    }
}
