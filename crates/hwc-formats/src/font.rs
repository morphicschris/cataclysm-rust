//! `.hff` bitmap fonts.
//!
//! A font file is a memory image of the engine's font header with pointers stored as file
//! offsets, followed by a table of per-character rectangles, one 8-bit coverage image holding
//! every glyph side by side, and a small palette. All values are little-endian.
//!
//! | offset | type        | field                                                   |
//! |--------|-------------|---------------------------------------------------------|
//! | 0      | [u8; 8]     | identifier `"Orannge\0"`                                |
//! | 8      | u16         | version, `0x100`                                        |
//! | 10     | u16         | flags (always 0: monochrome, no colour/alpha variants)  |
//! | 12     | i32         | number of characters present                            |
//! | 16     | i32         | extra advance added after every glyph (may be negative) |
//! | 20     | i32         | full line height                                        |
//! | 24     | i32         | baseline (equal to the full height in every font)       |
//! | 28     | u32         | name offset (always 0)                                  |
//! | 32     | i32 × 2     | image width and height                                  |
//! | 40     | i32         | palette colour count (always 2)                         |
//! | 44     | u32         | palette offset (RGBA, 4 bytes per colour)               |
//! | 48     | u32         | image offset                                            |
//! | 52     | u32         | runtime handle slot (junk, `0xabab`)                    |
//! | 56     | [u8; 12]    | reserved                                                |
//! | 68     | u32 × 256   | offset of each character's record, 0 if absent          |
//!
//! Character record (12 bytes): `u, v, width, height, offset_x, offset_y`, all `i16`. The glyph
//! occupies `width - offset_x` by `height - offset_y` pixels of the image starting at
//! `(u + offset_x, v + offset_y)`; the offsets are zero in every shipped font.
//!
//! The image is one byte per pixel, rows top to bottom, holding a 4-bit coverage value (0–15)
//! rather than a palette index: the engine expands it to alpha as `value * 16`.

use thiserror::Error;

pub const IDENTIFIER: &[u8; 8] = b"Orannge\0";
pub const VERSION: u16 = 0x100;
pub const HEADER_LEN: usize = 68;
pub const CHAR_TABLE_LEN: usize = 256 * 4;
pub const CHAR_RECORD_LEN: usize = 12;
/// Largest coverage value stored in a font image.
pub const MAX_COVERAGE: u8 = 15;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a font header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u16),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("negative or oversized {what}: {value}")]
    BadDimension { what: &'static str, value: i32 },
    #[error("character {code}: rectangle {rect:?} lies outside the {width}x{height} image")]
    GlyphOutsideImage {
        code: u8,
        rect: [i32; 4],
        width: u32,
        height: u32,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// Placement of one character in the font image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub code: u8,
    /// Top-left corner of the character cell in the image.
    pub u: i16,
    pub v: i16,
    /// Cell size; also the horizontal advance (before [`Font::spacing`]).
    pub width: i16,
    pub height: i16,
    pub offset_x: i16,
    pub offset_y: i16,
}

impl Glyph {
    /// Width of the stored bitmap.
    pub fn bitmap_width(&self) -> u32 {
        (self.width - self.offset_x).max(0) as u32
    }

    /// Height of the stored bitmap.
    pub fn bitmap_height(&self) -> u32 {
        (self.height - self.offset_y).max(0) as u32
    }
}

/// One character's pixels cut out of the font image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlyphBitmap {
    pub width: u32,
    pub height: u32,
    /// Row-major, top row first, one 4-bit coverage value (0..=15) per pixel.
    pub coverage: Vec<u8>,
}

impl GlyphBitmap {
    /// Coverage expanded to 8-bit alpha (0 → 0, 15 → 255).
    pub fn alpha(&self) -> Vec<u8> {
        self.coverage
            .iter()
            .map(|&c| coverage_to_alpha(c))
            .collect()
    }
}

/// Expands a stored 4-bit coverage value to 8-bit alpha. Values above 15 saturate.
pub fn coverage_to_alpha(c: u8) -> u8 {
    c.min(MAX_COVERAGE) * 17
}

#[derive(Debug, Clone, PartialEq)]
pub struct Font {
    pub version: u16,
    pub flags: u16,
    /// Character count recorded in the header.
    pub declared_count: u32,
    /// Extra horizontal advance after each glyph; also the advance of a missing character.
    pub spacing: i32,
    pub full_height: i32,
    pub baseline: i32,
    pub image_width: u32,
    pub image_height: u32,
    /// RGBA. Always black then white; the image does not index it.
    pub palette: Vec<[u8; 4]>,
    /// `image_width * image_height` coverage bytes, top row first.
    pub image: Vec<u8>,
    glyphs: Box<[Option<Glyph>; 256]>,
}

fn read_u16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn read_i16(b: &[u8], at: usize) -> i16 {
    i16::from_le_bytes([b[at], b[at + 1]])
}

fn read_u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn read_i32(b: &[u8], at: usize) -> i32 {
    read_u32(b, at) as i32
}

fn slice<'a>(data: &'a [u8], what: &'static str, offset: usize, len: usize) -> Result<&'a [u8]> {
    offset
        .checked_add(len)
        .and_then(|end| data.get(offset..end))
        .ok_or(Error::OutOfBounds {
            what,
            offset,
            len,
            file_len: data.len(),
        })
}

fn dimension(what: &'static str, value: i32) -> Result<u32> {
    // Fonts are tiny; anything past 64K pixels on a side is corrupt.
    if (0..=0x10000).contains(&value) {
        Ok(value as u32)
    } else {
        Err(Error::BadDimension { what, value })
    }
}

impl Font {
    pub fn parse(data: &[u8]) -> Result<Font> {
        if data.len() < HEADER_LEN + CHAR_TABLE_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[..8].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = read_u16(data, 8);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let flags = read_u16(data, 10);
        let declared_count = dimension("character count", read_i32(data, 12))?;
        let spacing = read_i32(data, 16);
        let full_height = read_i32(data, 20);
        let baseline = read_i32(data, 24);
        let image_width = dimension("image width", read_i32(data, 32))?;
        let image_height = dimension("image height", read_i32(data, 36))?;
        let colours = dimension("palette size", read_i32(data, 40))?;
        let palette_offset = read_u32(data, 44) as usize;
        let image_offset = read_u32(data, 48) as usize;

        let palette = slice(data, "palette", palette_offset, colours as usize * 4)?
            .as_chunks::<4>()
            .0
            .to_vec();
        let image_len = image_width as usize * image_height as usize;
        let image = slice(data, "image", image_offset, image_len)?.to_vec();

        let mut glyphs = Box::new([None; 256]);
        for (code, slot) in glyphs.iter_mut().enumerate() {
            let offset = read_u32(data, HEADER_LEN + code * 4) as usize;
            if offset == 0 {
                continue;
            }
            let r = slice(data, "character record", offset, CHAR_RECORD_LEN)?;
            let glyph = Glyph {
                code: code as u8,
                u: read_i16(r, 0),
                v: read_i16(r, 2),
                width: read_i16(r, 4),
                height: read_i16(r, 6),
                offset_x: read_i16(r, 8),
                offset_y: read_i16(r, 10),
            };
            let x = i32::from(glyph.u) + i32::from(glyph.offset_x);
            let y = i32::from(glyph.v) + i32::from(glyph.offset_y);
            let (w, h) = (glyph.bitmap_width() as i32, glyph.bitmap_height() as i32);
            if x < 0
                || y < 0
                || glyph.width < glyph.offset_x
                || glyph.height < glyph.offset_y
                || x + w > image_width as i32
                || y + h > image_height as i32
            {
                return Err(Error::GlyphOutsideImage {
                    code: code as u8,
                    rect: [x, y, w, h],
                    width: image_width,
                    height: image_height,
                });
            }
            *slot = Some(glyph);
        }

        Ok(Font {
            version,
            flags,
            declared_count,
            spacing,
            full_height,
            baseline,
            image_width,
            image_height,
            palette,
            image,
            glyphs,
        })
    }

    pub fn glyph(&self, code: u8) -> Option<&Glyph> {
        self.glyphs[code as usize].as_ref()
    }

    /// All characters present, in code order.
    pub fn glyphs(&self) -> impl Iterator<Item = &Glyph> + '_ {
        self.glyphs.iter().flatten()
    }

    /// Cuts a character's coverage bitmap out of the font image.
    pub fn bitmap(&self, code: u8) -> Option<GlyphBitmap> {
        let g = self.glyph(code)?;
        let (w, h) = (g.bitmap_width() as usize, g.bitmap_height() as usize);
        let x = (g.u + g.offset_x) as usize;
        let y = (g.v + g.offset_y) as usize;
        let stride = self.image_width as usize;
        let mut coverage = Vec::with_capacity(w * h);
        for row in y..y + h {
            let start = row * stride + x;
            coverage.extend_from_slice(&self.image[start..start + w]);
        }
        Some(GlyphBitmap {
            width: w as u32,
            height: h as u32,
            coverage,
        })
    }

    /// Horizontal advance of a byte string the way the engine lays it out: each present character
    /// advances by its width plus [`Font::spacing`], each missing one by the spacing alone. The
    /// `&` hot-key marker is not treated specially here.
    pub fn text_width(&self, text: &[u8]) -> i32 {
        text.iter()
            .map(|&c| match self.glyph(c) {
                Some(g) => i32::from(g.width) + self.spacing,
                None => self.spacing,
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_font() -> Vec<u8> {
        // Two glyphs ('A' 2x2 at u=0, 'B' 1x2 at u=2) in a 3x2 image.
        let records = HEADER_LEN + CHAR_TABLE_LEN;
        let image = records + 2 * CHAR_RECORD_LEN;
        let palette = image + 6;
        let mut d = vec![0u8; palette + 8];
        d[..8].copy_from_slice(IDENTIFIER);
        d[8..10].copy_from_slice(&VERSION.to_le_bytes());
        for (at, v) in [
            (12, 2i32),
            (16, 1),
            (20, 2),
            (24, 2),
            (32, 3),
            (36, 2),
            (40, 2),
        ] {
            d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        d[44..48].copy_from_slice(&(palette as u32).to_le_bytes());
        d[48..52].copy_from_slice(&(image as u32).to_le_bytes());
        for (i, (code, u, w)) in [(b'A', 0i16, 2i16), (b'B', 2, 1)].into_iter().enumerate() {
            let rec = records + i * CHAR_RECORD_LEN;
            let at = HEADER_LEN + code as usize * 4;
            d[at..at + 4].copy_from_slice(&(rec as u32).to_le_bytes());
            for (j, v) in [u, 0, w, 2, 0, 0].into_iter().enumerate() {
                d[rec + j * 2..rec + j * 2 + 2].copy_from_slice(&v.to_le_bytes());
            }
        }
        d[image..image + 6].copy_from_slice(&[15, 0, 7, 0, 15, 3]);
        d[palette + 4..palette + 8].copy_from_slice(&[255, 255, 255, 0]);
        d
    }

    #[test]
    fn parses_synthetic_font() {
        let font = Font::parse(&tiny_font()).unwrap();
        assert_eq!(font.glyphs().count(), 2);
        let a = font.bitmap(b'A').unwrap();
        assert_eq!((a.width, a.height), (2, 2));
        assert_eq!(a.coverage, [15, 0, 0, 15]);
        assert_eq!(a.alpha(), [255, 0, 0, 255]);
        assert_eq!(font.bitmap(b'B').unwrap().coverage, [7, 3]);
        assert!(font.bitmap(b'C').is_none());
        assert_eq!(font.text_width(b"AB?"), (2 + 1) + (1 + 1) + 1);
    }

    #[test]
    fn rejects_glyph_outside_image() {
        let mut d = tiny_font();
        let rec = HEADER_LEN + CHAR_TABLE_LEN;
        d[rec..rec + 2].copy_from_slice(&5i16.to_le_bytes());
        assert!(matches!(
            Font::parse(&d),
            Err(Error::GlyphOutsideImage { code: b'A', .. })
        ));
        assert!(matches!(Font::parse(&d[..100]), Err(Error::TooShort(100))));
    }
}
