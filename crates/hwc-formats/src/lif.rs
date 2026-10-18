//! `.lif` textures ("layered image format").
//!
//! Every `.lif` in Cataclysm is a single image (no mip levels) with a fixed 48-byte little-endian
//! header followed by the payloads it points at. All offsets are relative to the start of the file.
//!
//! | offset | type     | field                                               |
//! |--------|----------|-----------------------------------------------------|
//! | 0      | [u8; 8]  | identifier, `"Willy 7\0"`                           |
//! | 8      | u32      | version, always `0x104`                             |
//! | 12     | u32      | flags (see the `FLAG_*` constants)                  |
//! | 16     | u32      | width                                               |
//! | 20     | u32      | height                                              |
//! | 24     | u32      | CRC of the palette                                  |
//! | 28     | u32      | CRC of the original, unquantised image              |
//! | 32     | u32      | offset of the pixel data                            |
//! | 36     | u32      | offset of the 256-entry RGBA palette, 0 if none     |
//! | 40     | u32      | offset of the team-colour 0 (base) effect table     |
//! | 44     | u32      | offset of the team-colour 1 (stripe) effect table   |
//!
//! Two pixel layouts occur:
//!
//! * **Indexed** (palette offset non-zero): one byte per pixel indexing a 256-entry palette of
//!   RGBA bytes. Both team effect tables are always present and are 256 bytes long — one effect
//!   strength per palette entry — even when the team flags are clear.
//! * **RGBA** (palette offset zero): four bytes per pixel, R G B A. A team effect table is
//!   `width * height` bytes (one strength per pixel) and is present only when the matching team
//!   flag is set; otherwise its header offset is stale and must be ignored. Some UI images carry
//!   `FLAG_PALETTED` yet have no palette and RGBA data; the palette offset is authoritative.

use thiserror::Error;

/// The identifier every `.lif` file starts with.
pub const IDENTIFIER: &[u8; 8] = b"Willy 7\0";
/// The only version that occurs.
pub const VERSION: u32 = 0x104;
/// Size of the fixed header.
pub const HEADER_LEN: usize = 48;
/// Number of entries in a palette (and in an indexed image's team effect tables).
pub const PALETTE_LEN: usize = 256;

/// Image was quantised to a palette (not reliable on its own; see the module docs).
pub const FLAG_PALETTED: u32 = 0x02;
/// Image has a meaningful alpha channel.
pub const FLAG_ALPHA: u32 = 0x08;
/// Image takes the team base colour.
pub const FLAG_TEAM_COLOR0: u32 = 0x10;
/// Image takes the team stripe colour.
pub const FLAG_TEAM_COLOR1: u32 = 0x20;
/// Set only on front-end / UI images (menus, research screen, cursors, icons), many of which have
/// non-power-of-two sizes. Its exact meaning to the engine is unconfirmed; it does not change the
/// file layout.
pub const FLAG_UI: u32 = 0x100;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a LIF header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u32),
    #[error("invalid dimensions {0}x{1}")]
    BadDimensions(u32, u32),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
}

pub type Result<T> = std::result::Result<T, Error>;

/// How the pixel data is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PixelFormat {
    /// One palette index per pixel.
    Indexed,
    /// Four bytes per pixel, R G B A.
    Rgba,
}

/// A decoded `.lif` file.
#[derive(Debug, Clone)]
pub struct Lif {
    pub version: u32,
    pub flags: u32,
    pub width: u32,
    pub height: u32,
    pub palette_crc: u32,
    pub image_crc: u32,
    pub format: PixelFormat,
    /// Raw pixel data: `width * height` indices or `width * height * 4` RGBA bytes.
    pub pixels: Vec<u8>,
    /// Palette entries as RGBA, present for [`PixelFormat::Indexed`].
    pub palette: Option<Vec<[u8; 4]>>,
    /// Team base colour strength (0..=255): per palette entry for indexed images, per pixel for RGBA.
    pub team_effect0: Option<Vec<u8>>,
    /// Team stripe colour strength, laid out like `team_effect0`.
    pub team_effect1: Option<Vec<u8>>,
}

fn read_u32(data: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(data[at..at + 4].try_into().unwrap())
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

impl Lif {
    pub fn parse(data: &[u8]) -> Result<Lif> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[0..8].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = read_u32(data, 8);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let flags = read_u32(data, 12);
        let width = read_u32(data, 16);
        let height = read_u32(data, 20);
        let palette_crc = read_u32(data, 24);
        let image_crc = read_u32(data, 28);
        let data_off = read_u32(data, 32) as usize;
        let palette_off = read_u32(data, 36) as usize;
        let team0_off = read_u32(data, 40) as usize;
        let team1_off = read_u32(data, 44) as usize;

        let pixel_count = (width as usize)
            .checked_mul(height as usize)
            .filter(|&n| n > 0 && n <= 1 << 26)
            .ok_or(Error::BadDimensions(width, height))?;

        let format = if palette_off != 0 {
            PixelFormat::Indexed
        } else {
            PixelFormat::Rgba
        };
        let (pixels, palette, team_effect0, team_effect1) = match format {
            PixelFormat::Indexed => {
                let pixels = slice(data, "pixel data", data_off, pixel_count)?.to_vec();
                let palette = slice(data, "palette", palette_off, PALETTE_LEN * 4)?
                    .as_chunks::<4>()
                    .0
                    .to_vec();
                let table = |what, off| -> Result<Option<Vec<u8>>> {
                    if off == 0 {
                        return Ok(None);
                    }
                    Ok(Some(slice(data, what, off, PALETTE_LEN)?.to_vec()))
                };
                let t0 = table("team effect 0", team0_off)?;
                let t1 = table("team effect 1", team1_off)?;
                (pixels, Some(palette), t0, t1)
            }
            PixelFormat::Rgba => {
                let pixels = slice(data, "pixel data", data_off, pixel_count * 4)?.to_vec();
                let table = |what, flag, off| -> Result<Option<Vec<u8>>> {
                    if flags & flag == 0 || off == 0 {
                        return Ok(None);
                    }
                    Ok(Some(slice(data, what, off, pixel_count)?.to_vec()))
                };
                let t0 = table("team effect 0", FLAG_TEAM_COLOR0, team0_off)?;
                let t1 = table("team effect 1", FLAG_TEAM_COLOR1, team1_off)?;
                (pixels, None, t0, t1)
            }
        };

        Ok(Lif {
            version,
            flags,
            width,
            height,
            palette_crc,
            image_crc,
            format,
            pixels,
            palette,
            team_effect0,
            team_effect1,
        })
    }

    pub fn pixel_count(&self) -> usize {
        self.width as usize * self.height as usize
    }

    pub fn has_alpha(&self) -> bool {
        self.flags & FLAG_ALPHA != 0
    }

    pub fn uses_team_color0(&self) -> bool {
        self.flags & FLAG_TEAM_COLOR0 != 0
    }

    pub fn uses_team_color1(&self) -> bool {
        self.flags & FLAG_TEAM_COLOR1 != 0
    }

    /// Decode to tightly packed RGBA8, rows top to bottom as stored, without team colouring.
    /// The result is always `width * height * 4` bytes.
    pub fn to_rgba(&self) -> Vec<u8> {
        match (&self.format, &self.palette) {
            (PixelFormat::Indexed, Some(palette)) => self
                .pixels
                .iter()
                .flat_map(|&i| palette[i as usize])
                .collect(),
            _ => self.pixels.clone(),
        }
    }

    /// Per-pixel team colour strengths (0..=255) for the base (`which == 0`) or stripe
    /// (`which == 1`) colour, or `None` if this image does not take that team colour.
    pub fn team_mask(&self, which: usize) -> Option<Vec<u8>> {
        let (flag, table) = match which {
            0 => (FLAG_TEAM_COLOR0, self.team_effect0.as_ref()),
            1 => (FLAG_TEAM_COLOR1, self.team_effect1.as_ref()),
            _ => return None,
        };
        if self.flags & flag == 0 {
            return None;
        }
        let table = table?;
        Some(match self.format {
            PixelFormat::Indexed => self.pixels.iter().map(|&i| table[i as usize]).collect(),
            PixelFormat::Rgba => table.clone(),
        })
    }

    /// Decode to RGBA8 with team colours blended in: each pixel moves towards `base` by its
    /// base-colour strength, then towards `stripe` by its stripe strength. Alpha is untouched.
    pub fn to_rgba_team(&self, base: [u8; 3], stripe: [u8; 3]) -> Vec<u8> {
        let mut rgba = self.to_rgba();
        for (mask, color) in [(self.team_mask(0), base), (self.team_mask(1), stripe)] {
            let Some(mask) = mask else { continue };
            for (px, &m) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&mask) {
                let m = m as u32;
                for (c, &t) in px[..3].iter_mut().zip(&color) {
                    *c = ((*c as u32 * (255 - m) + t as u32 * m + 127) / 255) as u8;
                }
            }
        }
        rgba
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(flags: u32, w: u32, h: u32, offs: [u32; 4]) -> Vec<u8> {
        let mut v = IDENTIFIER.to_vec();
        for x in [VERSION, flags, w, h, 0, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        for x in offs {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v
    }

    #[test]
    fn indexed_with_team() {
        let (w, h) = (2u32, 2u32);
        let pal = HEADER_LEN as u32 + 4;
        let t0 = pal + 1024;
        let mut v = header(
            FLAG_PALETTED | FLAG_TEAM_COLOR0,
            w,
            h,
            [48, pal, t0, t0 + 256],
        );
        v.extend_from_slice(&[0, 1, 1, 0]);
        let mut p = vec![0u8; 1024];
        p[0..4].copy_from_slice(&[10, 20, 30, 255]);
        p[4..8].copy_from_slice(&[0, 0, 0, 128]);
        v.extend_from_slice(&p);
        let mut t = vec![0u8; 256];
        t[1] = 255;
        v.extend_from_slice(&t);
        v.extend_from_slice(&[0u8; 256]);
        let lif = Lif::parse(&v).unwrap();
        assert_eq!(lif.format, PixelFormat::Indexed);
        let rgba = lif.to_rgba();
        assert_eq!(rgba.len(), 16);
        assert_eq!(&rgba[0..8], &[10, 20, 30, 255, 0, 0, 0, 128]);
        assert_eq!(lif.team_mask(0).unwrap(), vec![0, 255, 255, 0]);
        assert!(lif.team_mask(1).is_none());
        let team = lif.to_rgba_team([200, 100, 50], [0, 0, 0]);
        assert_eq!(&team[4..8], &[200, 100, 50, 128]);
    }

    #[test]
    fn rgba_ignores_stale_team_offset() {
        let mut v = header(FLAG_PALETTED | FLAG_ALPHA | FLAG_UI, 1, 1, [48, 0, 51, 0]);
        v.extend_from_slice(&[1, 2, 3, 4]);
        let lif = Lif::parse(&v).unwrap();
        assert_eq!(lif.format, PixelFormat::Rgba);
        assert!(lif.team_effect0.is_none());
        assert_eq!(lif.to_rgba(), vec![1, 2, 3, 4]);
    }

    #[test]
    fn rejects_truncated() {
        let v = header(0, 4, 4, [48, 0, 0, 0]);
        assert!(matches!(Lif::parse(&v), Err(Error::OutOfBounds { .. })));
        assert!(matches!(Lif::parse(&v[..20]), Err(Error::TooShort(20))));
    }
}
