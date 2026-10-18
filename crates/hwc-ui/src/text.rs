//! Text output with the engine's `.hff` bitmap fonts (`fontPrint`, `fontWidth`, drop shadows).
//!
//! Strings are 8-bit (Latin-1). A single `&` marks the next character as the hot-key letter,
//! which is printed brighter and is not itself printed or measured; `&&` prints one `&`.

use hwc_formats::font::Font;

use crate::canvas::{Canvas, Color};

/// Channel multipliers (out of 256) for the hot-key letter.
const BRIGHT_FACTOR: u32 = 350;

/// Drop-shadow direction, as stored in bits 20..27 of an atom's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shadow {
    None,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Shadow {
    /// The shadow selected by the flag bits (the lowest set bit wins).
    pub fn from_bits(bits: u8) -> Shadow {
        const ALL: [Shadow; 8] = [
            Shadow::N,
            Shadow::S,
            Shadow::E,
            Shadow::W,
            Shadow::NE,
            Shadow::NW,
            Shadow::SE,
            Shadow::SW,
        ];
        if bits == 0 {
            Shadow::None
        } else {
            ALL[bits.trailing_zeros() as usize]
        }
    }

    fn offset(self) -> Option<(i32, i32)> {
        Some(match self {
            Shadow::None => return None,
            Shadow::N => (0, -1),
            Shadow::S => (0, 1),
            Shadow::E => (1, 0),
            Shadow::W => (-1, 0),
            Shadow::NE => (1, -1),
            Shadow::NW => (-1, -1),
            Shadow::SE => (1, 1),
            Shadow::SW => (-1, 1),
        })
    }
}

/// Converts UI text to the engine's 8-bit encoding.
pub fn to_latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| u8::try_from(c).unwrap_or(b'?')).collect()
}

/// Splits a string into printed characters, flagging hot-key letters.
pub fn printed_chars(bytes: &[u8]) -> Vec<(u8, bool)> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut it = bytes.iter().copied();
    while let Some(b) = it.next() {
        if b == b'&' {
            match it.next() {
                None | Some(0) => break,
                Some(b'&') => out.push((b'&', false)),
                Some(c) => out.push((c, true)),
            }
        } else {
            out.push((b, false));
        }
    }
    out
}

/// Width of a string in pixels (`fontWidth`): each present glyph advances by its width plus
/// the font spacing, each missing one by the spacing.
pub fn width(font: &Font, text: &[u8]) -> i32 {
    printed_chars(text)
        .iter()
        .map(|&(c, _)| match font.glyph(c) {
            Some(g) => i32::from(g.width) + font.spacing,
            None => font.spacing,
        })
        .sum()
}

/// `fontHeight`: the font's full line height.
pub fn height(font: &Font) -> i32 {
    font.full_height
}

/// The hot-key colour: each channel scaled by 350/256, saturating.
pub fn bright(c: Color) -> Color {
    let b = |v: u8| (u32::from(v) * BRIGHT_FACTOR / 256).min(255) as u8;
    [b(c[0]), b(c[1]), b(c[2]), c[3]]
}

/// `fontPrint` with an optional shadow: the shadow is printed first, offset one pixel, in the
/// shadow colour. Returns the advance.
pub fn print(
    canvas: &mut Canvas,
    font: &Font,
    x: i32,
    y: i32,
    color: Color,
    text: &[u8],
    shadow: (Shadow, Color),
) -> i32 {
    if let Some((dx, dy)) = shadow.0.offset() {
        print_plain(canvas, font, x + dx, y + dy, shadow.1, text);
    }
    print_plain(canvas, font, x, y, color, text)
}

fn print_plain(
    canvas: &mut Canvas,
    font: &Font,
    mut x: i32,
    y: i32,
    color: Color,
    text: &[u8],
) -> i32 {
    let start = x;
    for (c, hot) in printed_chars(text) {
        let Some(glyph) = font.glyph(c) else {
            x += font.spacing;
            continue;
        };
        let col = if hot { bright(color) } else { color };
        if let Some(bitmap) = font.bitmap(c) {
            let ox = x + i32::from(glyph.offset_x);
            let oy = y + i32::from(glyph.offset_y);
            let w = bitmap.width as usize;
            for (i, &cov) in bitmap.coverage.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                // The engine expands 4-bit coverage to alpha as value * 16.
                let a = (u32::from(cov) * 16).min(255) as u8;
                canvas.blend(
                    ox + (i % w) as i32,
                    oy + (i / w) as i32,
                    [col[0], col[1], col[2], a],
                );
            }
        }
        x += i32::from(glyph.width) + font.spacing;
    }
    x - start
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_parsing() {
        assert_eq!(
            printed_chars(b"&Quit"),
            vec![(b'Q', true), (b'u', false), (b'i', false), (b't', false)]
        );
        assert_eq!(
            printed_chars(b"A&&B"),
            vec![(b'A', false), (b'&', false), (b'B', false)]
        );
        assert_eq!(printed_chars(b"trail&"), printed_chars(b"trail"));
    }

    #[test]
    fn bright_saturates() {
        assert_eq!(bright([255, 200, 0, 255]), [255, 255, 0, 255]);
        assert_eq!(bright([100, 0, 10, 7]), [136, 0, 13, 7]);
    }

    #[test]
    fn shadow_bits() {
        assert_eq!(Shadow::from_bits(0), Shadow::None);
        assert_eq!(Shadow::from_bits(0x40), Shadow::SE);
        assert_eq!(Shadow::from_bits(0x41), Shadow::N);
    }
}
