//! Minimal Targa decoder for the background star sprites (`btg\bitmaps\*.tga`).

use super::canvas::Texture;

/// Decodes an uncompressed or RLE true-colour TGA (24 or 32 bits) to top-down RGBA.
pub fn decode(data: &[u8]) -> Result<Texture, String> {
    let header = data.get(..18).ok_or("TGA shorter than its header")?;
    let id_len = header[0] as usize;
    let image_type = header[2];
    let width = u16::from_le_bytes([header[12], header[13]]) as usize;
    let height = u16::from_le_bytes([header[14], header[15]]) as usize;
    let bpp = header[16] as usize / 8;
    let top_down = header[17] & 0x20 != 0;
    if header[1] != 0 || !matches!(image_type, 2 | 10) || !matches!(bpp, 3 | 4) {
        return Err(format!(
            "unsupported TGA (colour map {}, type {image_type}, {} bits)",
            header[1],
            bpp * 8
        ));
    }
    let mut src = data.get(18 + id_len..).ok_or("TGA truncated")?;
    let count = width * height;
    let mut pixels: Vec<[u8; 4]> = Vec::with_capacity(count);
    let take = |src: &mut &[u8]| -> Result<[u8; 4], String> {
        let p = src.get(..bpp).ok_or("TGA pixel data truncated")?;
        let px = [p[2], p[1], p[0], if bpp == 4 { p[3] } else { 255 }];
        *src = &src[bpp..];
        Ok(px)
    };
    while pixels.len() < count {
        if image_type == 2 {
            pixels.push(take(&mut src)?);
            continue;
        }
        let (&packet, rest) = src.split_first().ok_or("TGA RLE data truncated")?;
        src = rest;
        let run = (packet & 0x7f) as usize + 1;
        if packet & 0x80 != 0 {
            let px = take(&mut src)?;
            pixels.extend(std::iter::repeat_n(px, run));
        } else {
            for _ in 0..run {
                pixels.push(take(&mut src)?);
            }
        }
    }
    pixels.truncate(count);
    if !top_down {
        let rows: Vec<_> = pixels.chunks(width).rev().flatten().copied().collect();
        pixels = rows;
    }
    Ok(Texture::new(width, height, pixels.concat()))
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_bottom_up_bgra() {
        let mut d = vec![0u8; 18];
        d[2] = 2;
        d[12] = 1;
        d[14] = 2;
        d[16] = 32;
        d.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let t = super::decode(&d).unwrap();
        assert_eq!(t.rgba, vec![7, 6, 5, 8, 3, 2, 1, 4]);
    }
}
