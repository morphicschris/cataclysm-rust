//! `.btg` backgrounds: a 2D page of coloured vertices and triangles (the nebula/dust gradient)
//! plus textured star sprites, wrapped onto the sky sphere at runtime.
//!
//! All values are little-endian and records are packed (no alignment padding). Cataclysm files
//! are identical in layout to Homeworld 1 version `0x600`.
//!
//! Header (84 bytes):
//!
//! | offset | type   | field                                                        |
//! |--------|--------|--------------------------------------------------------------|
//! | 0      | u32    | version, `0x600`                                             |
//! | 4      | u32    | vertex count                                                 |
//! | 8      | u32    | star count                                                   |
//! | 12     | u32    | polygon count                                                |
//! | 16     | i32×2  | editor scroll x, y                                           |
//! | 24     | u32    | editor zoom (the bits of an `f32`)                           |
//! | 28     | i32×2  | page width, height (2560 × 2048 in every shipped file)       |
//! | 36     | i32×3  | colour multiplier R, G, B (0-255)                            |
//! | 48     | i32×3  | background clear colour R, G, B                              |
//! | 60     | u32×5  | editor toggles: vertices, polygons, stars, outlines, blends  |
//! | 80     | i32    | editor render mode                                           |
//!
//! Then `vertex count` vertices (40 bytes: flags `u32`, x `f64`, y `f64`, RGBA `i32×4`,
//! brightness `i32`), `star count` stars (flags `u32`, x `f64`, y `f64`, RGBA `i32×4`, name
//! length `u32`, then that many bytes of texture name without a terminator) and `polygon count`
//! triangles (16 bytes: flags `u32`, vertex indices `u32×3`).

use thiserror::Error;

pub const VERSION: u32 = 0x600;
pub const HEADER_LEN: usize = 84;
pub const VERTEX_LEN: usize = 40;
/// Star record size before its texture name.
pub const STAR_FIXED_LEN: usize = 40;
pub const POLYGON_LEN: usize = 16;
/// Star texture names must fit the original 48-byte buffer with a terminator.
pub const MAX_STAR_NAME_LEN: usize = 47;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a BTG header")]
    TooShort(usize),
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
    #[error("star {star} texture name is {len} bytes (limit {MAX_STAR_NAME_LEN})")]
    StarNameTooLong { star: usize, len: usize },
    #[error("polygon {polygon} vertex index {index} out of range ({vertex_count} vertices)")]
    IndexOutOfRange {
        polygon: usize,
        index: u32,
        vertex_count: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vertex {
    pub flags: u32,
    /// Page coordinates, `0..=page_size`.
    pub position: [f64; 2],
    /// RGBA, 0-255.
    pub color: [i32; 4],
    pub brightness: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Star {
    pub flags: u32,
    /// Page coordinates.
    pub position: [f64; 2],
    /// RGBA, 0-255.
    pub color: [i32; 4],
    /// Sprite texture file name, e.g. `"B03.tga"`.
    pub texture: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Polygon {
    pub flags: u32,
    /// Indices into [`Btg::vertices`].
    pub vertices: [u32; 3],
}

/// Editor display toggles saved with the file; the game ignores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditorToggles {
    pub vertices: bool,
    pub polygons: bool,
    pub stars: bool,
    pub outlines: bool,
    pub blends: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Btg {
    pub version: u32,
    pub scroll: [i32; 2],
    pub zoom: f32,
    /// Width, height.
    pub page_size: [i32; 2],
    /// RGB, 0-255.
    pub color_multiplier: [i32; 3],
    /// RGB, 0-255.
    pub background: [i32; 3],
    pub editor: EditorToggles,
    pub render_mode: i32,
    pub vertices: Vec<Vertex>,
    pub stars: Vec<Star>,
    pub polygons: Vec<Polygon>,
    /// Bytes after the polygon list (zero in every shipped file).
    pub trailing_bytes: usize,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn i32_at(b: &[u8], at: usize) -> i32 {
    i32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f64_at(b: &[u8], at: usize) -> f64 {
    f64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

fn rgba_at(b: &[u8], at: usize) -> [i32; 4] {
    [
        i32_at(b, at),
        i32_at(b, at + 4),
        i32_at(b, at + 8),
        i32_at(b, at + 12),
    ]
}

struct Cursor<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, what: &'static str, len: usize) -> Result<&'a [u8]> {
        let bytes = self
            .at
            .checked_add(len)
            .and_then(|end| self.data.get(self.at..end))
            .ok_or(Error::OutOfBounds {
                what,
                offset: self.at,
                len,
                file_len: self.data.len(),
            })?;
        self.at += len;
        Ok(bytes)
    }

    fn records(
        &mut self,
        what: &'static str,
        count: u32,
        stride: usize,
    ) -> Result<std::slice::ChunksExact<'a, u8>> {
        let len = (count as usize)
            .checked_mul(stride)
            .ok_or(Error::OutOfBounds {
                what,
                offset: self.at,
                len: usize::MAX,
                file_len: self.data.len(),
            })?;
        Ok(self.take(what, len)?.chunks_exact(stride))
    }
}

impl Btg {
    pub fn parse(data: &[u8]) -> Result<Btg> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let h = data;
        let version = u32_at(h, 0);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let vertex_count = u32_at(h, 4);
        let star_count = u32_at(h, 8);
        let polygon_count = u32_at(h, 12);
        let mut c = Cursor {
            data,
            at: HEADER_LEN,
        };

        let vertices: Vec<Vertex> = c
            .records("vertices", vertex_count, VERTEX_LEN)?
            .map(|v| Vertex {
                flags: u32_at(v, 0),
                position: [f64_at(v, 4), f64_at(v, 12)],
                color: rgba_at(v, 20),
                brightness: i32_at(v, 36),
            })
            .collect();

        // Each star is variable-length, so a corrupt count must not drive a huge allocation.
        let mut stars = Vec::with_capacity((star_count as usize).min(data.len() / STAR_FIXED_LEN));
        for star in 0..star_count as usize {
            let s = c.take("star", STAR_FIXED_LEN)?;
            let name_len = u32_at(s, 36) as usize;
            if name_len > MAX_STAR_NAME_LEN {
                return Err(Error::StarNameTooLong {
                    star,
                    len: name_len,
                });
            }
            let name = c.take("star texture name", name_len)?;
            let end = name.iter().position(|&b| b == 0).unwrap_or(name.len());
            stars.push(Star {
                flags: u32_at(s, 0),
                position: [f64_at(s, 4), f64_at(s, 12)],
                color: rgba_at(s, 20),
                texture: String::from_utf8_lossy(&name[..end]).into_owned(),
            });
        }

        let polygons = c
            .records("polygons", polygon_count, POLYGON_LEN)?
            .enumerate()
            .map(|(polygon, p)| {
                let vertices = [u32_at(p, 4), u32_at(p, 8), u32_at(p, 12)];
                if let Some(&index) = vertices.iter().find(|&&i| i >= vertex_count) {
                    return Err(Error::IndexOutOfRange {
                        polygon,
                        index,
                        vertex_count: vertex_count as usize,
                    });
                }
                Ok(Polygon {
                    flags: u32_at(p, 0),
                    vertices,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Btg {
            version,
            scroll: [i32_at(h, 16), i32_at(h, 20)],
            zoom: f32::from_bits(u32_at(h, 24)),
            page_size: [i32_at(h, 28), i32_at(h, 32)],
            color_multiplier: [i32_at(h, 36), i32_at(h, 40), i32_at(h, 44)],
            background: [i32_at(h, 48), i32_at(h, 52), i32_at(h, 56)],
            editor: EditorToggles {
                vertices: u32_at(h, 60) != 0,
                polygons: u32_at(h, 64) != 0,
                stars: u32_at(h, 68) != 0,
                outlines: u32_at(h, 72) != 0,
                blends: u32_at(h, 76) != 0,
            },
            render_mode: i32_at(h, 80),
            vertices,
            stars,
            polygons,
            trailing_bytes: data.len() - c.at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_and_star() {
        let mut v = vec![0u8; HEADER_LEN];
        v[0..4].copy_from_slice(&VERSION.to_le_bytes());
        v[4..8].copy_from_slice(&3u32.to_le_bytes());
        v[8..12].copy_from_slice(&1u32.to_le_bytes());
        v[12..16].copy_from_slice(&1u32.to_le_bytes());
        v[24..28].copy_from_slice(&0.5f32.to_le_bytes());
        for i in 0..3 {
            let mut rec = [0u8; VERTEX_LEN];
            rec[4..12].copy_from_slice(&(i as f64 * 10.0).to_le_bytes());
            rec[36..40].copy_from_slice(&255i32.to_le_bytes());
            v.extend_from_slice(&rec);
        }
        let mut star = [0u8; STAR_FIXED_LEN];
        star[36..40].copy_from_slice(&7u32.to_le_bytes());
        v.extend_from_slice(&star);
        v.extend_from_slice(b"B03.tga");
        for w in [0u32, 0, 1, 2] {
            v.extend_from_slice(&w.to_le_bytes());
        }
        let b = Btg::parse(&v).unwrap();
        assert_eq!(b.zoom, 0.5);
        assert_eq!(b.vertices[2].position, [20.0, 0.0]);
        assert_eq!(b.stars[0].texture, "B03.tga");
        assert_eq!(b.polygons[0].vertices, [0, 1, 2]);
        assert_eq!(b.trailing_bytes, 0);

        let n = v.len();
        v[n - 4..].copy_from_slice(&3u32.to_le_bytes());
        assert!(matches!(Btg::parse(&v), Err(Error::IndexOutOfRange { .. })));
    }
}
