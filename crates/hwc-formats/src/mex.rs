//! `.mex` mesh extensions: the named points and volumes that go with a mesh (collision sphere and
//! box, gun, engine, docking, salvage, damage-effect and navigation-light points).
//!
//! All values are little-endian. A file is a 12-byte header followed by `chunk_count` chunks laid
//! end to end; nothing in the file uses absolute offsets.
//!
//! Header:
//!
//! | offset | type    | field                                   |
//! |--------|---------|-----------------------------------------|
//! | 0      | [u8; 8] | identifier, `"mannngo\0"`               |
//! | 8      | u16     | version: `0x106` in every shipped file  |
//! | 10     | u16     | chunk count                             |
//!
//! Chunk header (16 bytes), followed by `size` bytes of body:
//!
//! | offset | type    | field                                                  |
//! |--------|---------|--------------------------------------------------------|
//! | 0      | [u8; 4] | tag, NUL-terminated (`Col`, `Rct`, `Gun`, ...)         |
//! | 4      | [u8; 8] | chunk name, NUL-terminated (`Col0`, `GUN3`, `Dock`...) |
//! | 12     | u32     | body size                                              |
//!
//! Fixed-size text fields are NUL-terminated; the bytes after the terminator are tool garbage
//! (`0xAB`, `0xCD`) and are ignored. Angles the exporter never set hold the `0xCDCDCDCD` fill
//! pattern; those read as `None`.
//!
//! Body layouts (a "point" is position `f32×3`, normal `f32×3`, cone angle `f32`, edge angle
//! `f32`, 32 bytes):
//!
//! | tag   | size | body                                                                 |
//! |-------|------|----------------------------------------------------------------------|
//! | `Col` | 20   | level `u32`, centre `f32×3`, radius `f32`                            |
//! | `Rct` | 28   | level `u32`, corner `f32×3`, extent `f32×3`                          |
//! | `Gun` | 32   | point                                                                |
//! | `Dmg` | 32   | point                                                                |
//! | `Eng` | 64   | point, attached object name `[u8; 32]` (32-byte HW1 form accepted)   |
//! | `Dok` | 52   | point, dock point name `[u8; 20]`                                    |
//! | `Sal` | 52   | point, salvage point name `[u8; 20]`                                 |
//! | `Nav` | 68   | position `f32×3`, colour (R, B, G, pad) `u8×4`, light name `[u8; 20]`, attached object name `[u8; 32]` (36-byte HW1 form accepted) |
//!
//! Any other tag is kept as [`ChunkData::Unknown`].

use thiserror::Error;

pub const IDENTIFIER: &[u8; 8] = b"mannngo\0";
/// Version found in every Cataclysm file. Other versions are accepted (the layout is chunked).
pub const VERSION: u16 = 0x106;
pub const HEADER_LEN: usize = 12;
pub const CHUNK_HEADER_LEN: usize = 16;
pub const POINT_LEN: usize = 32;
/// Fill pattern left in angle fields the exporter never wrote.
pub const UNSET_PATTERN: u32 = 0xCDCD_CDCD;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a MEX header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("chunk {index} at offset {offset} with length {len} runs past end of file ({file_len} bytes)")]
    OutOfBounds {
        index: usize,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("chunk {index} ({tag}) body is {size} bytes, needs at least {need}")]
    ChunkTooSmall {
        index: usize,
        tag: String,
        size: usize,
        need: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// A directed point with a cone of influence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub position: [f32; 3],
    /// Not necessarily unit length; may be zero.
    pub normal: [f32; 3],
    /// Degrees; `None` when the exporter left the field unset.
    pub cone_angle: Option<f32>,
    /// Degrees; `None` when the exporter left the field unset.
    pub edge_angle: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChunkData {
    /// `Col`: bounding sphere used for collision.
    CollisionSphere {
        level: u32,
        centre: [f32; 3],
        radius: f32,
    },
    /// `Rct`: collision box given by its minimum corner and extent.
    CollisionBox {
        level: u32,
        corner: [f32; 3],
        extent: [f32; 3],
    },
    /// `Gun`: weapon muzzle (`GUNn`), repair nozzle (`REP0`) or tractor beam (`BEAM0`).
    Gun(Point),
    /// `Dmg`: location for damage effects (not read by the Homeworld 1 code).
    Damage(Point),
    /// `Eng`: engine nozzle.
    Engine {
        point: Point,
        /// Mesh object the nozzle follows when the mesh animates (Cataclysm only).
        attached_to: Option<String>,
    },
    /// `Dok`: docking point.
    Dock {
        point: Point,
        name: String,
    },
    /// `Sal`: salvage attachment point.
    Salvage {
        point: Point,
        name: String,
    },
    /// `Nav`: navigation light.
    NavLight {
        position: [f32; 3],
        /// RGB (stored on disk as R, B, G).
        color: [u8; 3],
        name: String,
        /// Mesh object the light follows when the mesh animates (Cataclysm only).
        attached_to: Option<String>,
    },
    Unknown(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    /// Chunk tag, e.g. `"Col"`.
    pub tag: String,
    /// Chunk name, e.g. `"Col0"`, `"GUN3"`, `"Dock"`.
    pub name: String,
    /// Body size as stored.
    pub size: u32,
    pub data: ChunkData,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mex {
    pub version: u16,
    pub chunks: Vec<Chunk>,
    /// Bytes after the last chunk (zero in every shipped file).
    pub trailing_bytes: usize,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn vec3_at(b: &[u8], at: usize) -> [f32; 3] {
    [f32_at(b, at), f32_at(b, at + 4), f32_at(b, at + 8)]
}

fn opt_angle(b: &[u8], at: usize) -> Option<f32> {
    let bits = u32_at(b, at);
    (bits != UNSET_PATTERN).then_some(f32::from_bits(bits))
}

/// Text up to the first NUL in a fixed-size field.
fn text(field: &[u8]) -> String {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    String::from_utf8_lossy(&field[..end]).into_owned()
}

fn point(b: &[u8]) -> Point {
    Point {
        position: vec3_at(b, 0),
        normal: vec3_at(b, 12),
        cone_angle: opt_angle(b, 24),
        edge_angle: opt_angle(b, 28),
    }
}

impl ChunkData {
    /// Minimum body size for a known tag.
    fn min_size(tag: &str) -> usize {
        match tag {
            "Col" => 20,
            "Rct" => 28,
            "Gun" | "Dmg" | "Eng" => POINT_LEN,
            "Dok" | "Sal" => POINT_LEN + 20,
            "Nav" => 36,
            _ => 0,
        }
    }

    fn decode(tag: &str, b: &[u8]) -> ChunkData {
        match tag {
            "Col" => ChunkData::CollisionSphere {
                level: u32_at(b, 0),
                centre: vec3_at(b, 4),
                radius: f32_at(b, 16),
            },
            "Rct" => ChunkData::CollisionBox {
                level: u32_at(b, 0),
                corner: vec3_at(b, 4),
                extent: vec3_at(b, 16),
            },
            "Gun" => ChunkData::Gun(point(b)),
            "Dmg" => ChunkData::Damage(point(b)),
            "Eng" => ChunkData::Engine {
                point: point(b),
                attached_to: b.get(POINT_LEN..).filter(|r| !r.is_empty()).map(text),
            },
            "Dok" => ChunkData::Dock {
                point: point(b),
                name: text(&b[POINT_LEN..POINT_LEN + 20]),
            },
            "Sal" => ChunkData::Salvage {
                point: point(b),
                name: text(&b[POINT_LEN..POINT_LEN + 20]),
            },
            "Nav" => ChunkData::NavLight {
                position: vec3_at(b, 0),
                color: [b[12], b[14], b[13]],
                name: text(&b[16..36]),
                attached_to: b.get(36..).filter(|r| !r.is_empty()).map(text),
            },
            _ => ChunkData::Unknown(b.to_vec()),
        }
    }
}

impl Mex {
    pub fn parse(data: &[u8]) -> Result<Mex> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[0..8].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = u16::from_le_bytes([data[8], data[9]]);
        let count = u16::from_le_bytes([data[10], data[11]]) as usize;

        let mut chunks = Vec::with_capacity(count);
        let mut at = HEADER_LEN;
        for index in 0..count {
            let oob = |offset: usize, len: usize| Error::OutOfBounds {
                index,
                offset,
                len,
                file_len: data.len(),
            };
            let head = data
                .get(at..at + CHUNK_HEADER_LEN)
                .ok_or_else(|| oob(at, CHUNK_HEADER_LEN))?;
            let size = u32_at(head, 12);
            let body_at = at + CHUNK_HEADER_LEN;
            let body = body_at
                .checked_add(size as usize)
                .and_then(|end| data.get(body_at..end))
                .ok_or_else(|| oob(body_at, size as usize))?;
            let tag = text(&head[0..4]);
            let need = ChunkData::min_size(&tag);
            if body.len() < need {
                return Err(Error::ChunkTooSmall {
                    index,
                    tag,
                    size: body.len(),
                    need,
                });
            }
            chunks.push(Chunk {
                name: text(&head[4..12]),
                size,
                data: ChunkData::decode(&tag, body),
                tag,
            });
            at = body_at + body.len();
        }
        Ok(Mex {
            version,
            chunks,
            trailing_bytes: data.len() - at,
        })
    }

    /// Chunks with the given tag (case-insensitive), in file order.
    pub fn chunks_tagged<'a>(&'a self, tag: &'a str) -> impl Iterator<Item = &'a Chunk> + 'a {
        self.chunks
            .iter()
            .filter(move |c| c.tag.eq_ignore_ascii_case(tag))
    }

    /// First chunk with the given tag and chunk name (both case-insensitive).
    pub fn find(&self, tag: &str, name: &str) -> Option<&Chunk> {
        self.chunks
            .iter()
            .find(|c| c.tag.eq_ignore_ascii_case(tag) && c.name.eq_ignore_ascii_case(name))
    }

    /// The gun point named `name` (e.g. `"Gun0"`, `"Rep0"`, `"Beam0"`).
    pub fn gun(&self, name: &str) -> Option<&Point> {
        match &self.find("Gun", name)?.data {
            ChunkData::Gun(p) => Some(p),
            _ => None,
        }
    }

    /// The docking point whose dock name is `name` (case-insensitive).
    pub fn dock(&self, name: &str) -> Option<&Point> {
        self.chunks.iter().find_map(|c| match &c.data {
            ChunkData::Dock { point, name: n } if n.eq_ignore_ascii_case(name) => Some(point),
            _ => None,
        })
    }

    /// The salvage point whose name is `name` (case-insensitive).
    pub fn salvage(&self, name: &str) -> Option<&Point> {
        self.chunks.iter().find_map(|c| match &c.data {
            ChunkData::Salvage { point, name: n } if n.eq_ignore_ascii_case(name) => Some(point),
            _ => None,
        })
    }

    /// The `Col0` collision sphere as (centre, radius).
    pub fn collision_sphere(&self) -> Option<([f32; 3], f32)> {
        match self.find("Col", "Col0")?.data {
            ChunkData::CollisionSphere { centre, radius, .. } => Some((centre, radius)),
            _ => None,
        }
    }

    /// The `Rct0` collision box as (minimum corner, extent).
    pub fn collision_box(&self) -> Option<([f32; 3], [f32; 3])> {
        match self.find("Rct", "Rct0")?.data {
            ChunkData::CollisionBox { corner, extent, .. } => Some((corner, extent)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(out: &mut Vec<u8>, tag: &[u8], name: &[u8], body: &[u8]) {
        let mut head = [0xABu8; CHUNK_HEADER_LEN];
        head[..4].fill(0);
        head[..tag.len()].copy_from_slice(tag);
        head[4..4 + name.len()].copy_from_slice(name);
        head[4 + name.len()] = 0;
        head[12..].copy_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&head);
        out.extend_from_slice(body);
    }

    fn floats(v: &[f32]) -> Vec<u8> {
        v.iter().flat_map(|f| f.to_le_bytes()).collect()
    }

    #[test]
    fn chunks_decode() {
        let mut v = IDENTIFIER.to_vec();
        v.extend_from_slice(&VERSION.to_le_bytes());
        v.extend_from_slice(&3u16.to_le_bytes());
        let mut col = 0u32.to_le_bytes().to_vec();
        col.extend(floats(&[1.0, 2.0, 3.0, 9.0]));
        chunk(&mut v, b"Col", b"Col0", &col);
        let mut eng = floats(&[0.0, 0.0, -5.0, 0.0, 0.0, -1.0]);
        eng.extend_from_slice(&[0xCD; 8]);
        let mut attach = [0xCDu8; 32];
        attach[..5].copy_from_slice(b"Nose\0");
        eng.extend_from_slice(&attach);
        chunk(&mut v, b"Eng", b"Eng0", &eng);
        let mut nav = floats(&[1.0, 1.0, 1.0]);
        nav.extend_from_slice(&[0x10, 0x30, 0x20, 0xCD]);
        let mut name = [0xCDu8; 20];
        name[..4].copy_from_slice(b"RED\0");
        nav.extend_from_slice(&name);
        chunk(&mut v, b"Nav", b"Nav", &nav);

        let m = Mex::parse(&v).unwrap();
        assert_eq!(m.chunks.len(), 3);
        assert_eq!(m.trailing_bytes, 0);
        assert_eq!(m.collision_sphere(), Some(([1.0, 2.0, 3.0], 9.0)));
        match &m.chunks[1].data {
            ChunkData::Engine { point, attached_to } => {
                assert_eq!(point.cone_angle, None);
                assert_eq!(attached_to.as_deref(), Some("Nose"));
            }
            other => panic!("{other:?}"),
        }
        match &m.chunks[2].data {
            ChunkData::NavLight {
                color,
                name,
                attached_to,
                ..
            } => {
                assert_eq!(*color, [0x10, 0x20, 0x30]);
                assert_eq!(name, "RED");
                assert_eq!(*attached_to, None);
            }
            other => panic!("{other:?}"),
        }

        let mut short = v.clone();
        short.truncate(v.len() - 1);
        assert!(matches!(
            Mex::parse(&short),
            Err(Error::OutOfBounds { index: 2, .. })
        ));
    }
}
