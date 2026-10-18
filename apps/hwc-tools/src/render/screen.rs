//! Front-end screens (`.fib`) drawn at the engine's native 640×480: decorative bitmaps at their
//! own size from each atom's top-left corner, filled/outlined regions, and text labels printed
//! with the `.hff` fonts the way `feStaticTextDraw` and `fontPrintN` do.

use std::collections::HashMap;

use hwc_formats::big::BigArchive;
use hwc_formats::fib::{self, Atom, AtomKind, Justification, Screen};
use hwc_formats::font::{coverage_to_alpha, Font};
use hwc_formats::lif::Lif;

use super::canvas::{rgba8, Canvas, Rgba};

pub const WIDTH: usize = 640;
pub const HEIGHT: usize = 480;

/// Directory the engine loads front-end fonts from (English build).
const FONT_DIR: &str = "fonts\\english\\";

/// Resources resolved for one screen.
pub struct ScreenRenderer<'a> {
    big: &'a BigArchive,
    fonts: HashMap<String, Option<Font>>,
    bitmaps: HashMap<String, Option<Lif>>,
}

impl<'a> ScreenRenderer<'a> {
    pub fn new(big: &'a BigArchive) -> Self {
        Self {
            big,
            fonts: HashMap::new(),
            bitmaps: HashMap::new(),
        }
    }

    /// Draws the screen's atoms in file order. Atom 0 is the screen's own base region; the rest
    /// become its child regions, which `feRegionsAdd` inserts last-to-first so that the region
    /// list draws them back to front in file order.
    pub fn render(&mut self, screen: &Screen, canvas: &mut Canvas) {
        for atom in screen.atoms.iter().skip(1) {
            if atom.has(fib::ATOM_HIDDEN) {
                continue;
            }
            if let Some(text) = atom.text() {
                self.draw_text(canvas, atom, text);
            } else if let Some(path) = atom.bitmap_path().filter(|_| is_decorative(atom)) {
                if !self.draw_bitmap(canvas, atom.rect.left, atom.rect.top, path) {
                    outline_rect(canvas, atom, [1.0, 0.0, 1.0, 0.6]);
                }
            } else if is_panel(atom) {
                if atom.has(fib::ATOM_CONTENTS_VISIBLE) {
                    fill_rect(canvas, atom, rgba8(atom.content_color));
                }
                if atom.has(fib::ATOM_BORDER_VISIBLE) {
                    outline_rect(canvas, atom, rgba8(atom.border_color));
                }
            } else {
                self.draw_control(canvas, atom);
            }
        }
    }

    fn draw_control(&mut self, canvas: &mut Canvas, atom: &Atom) {
        let border = rgba8(atom.border_color);
        match atom.kind {
            AtomKind::BitmapButton => {
                let drawn = atom.bitmap.as_deref().is_some_and(|base| {
                    let path = format!("{}_off.lif", base.trim_end_matches(".lif"));
                    self.draw_bitmap(canvas, atom.rect.left, atom.rect.top, &path)
                });
                if !drawn {
                    outline_rect(canvas, atom, border);
                }
            }
            AtomKind::Button
            | AtomKind::ToggleButton
            | AtomKind::CheckBox
            | AtomKind::RadioButton
            | AtomKind::HorizontalSlider
            | AtomKind::VerticalSlider
            | AtomKind::DragButton
            | AtomKind::TextEntry
            | AtomKind::ListWindow
            | AtomKind::MenuItem => outline_rect(canvas, atom, border),
            // User regions are drawn by named engine callbacks (version number, ship views...).
            _ => {}
        }
    }

    /// Draws a `.lif` at its native size with its top-left corner at `(x, y)`.
    fn draw_bitmap(&mut self, canvas: &mut Canvas, x: i32, y: i32, path: &str) -> bool {
        let big = self.big;
        let lif = self
            .bitmaps
            .entry(path.to_ascii_lowercase())
            .or_insert_with(|| {
                let data = big.read_file(path).ok().flatten();
                if data.is_none() {
                    eprintln!("warning: bitmap {path} not in archive");
                }
                data.and_then(|d| {
                    Lif::parse(&d)
                        .map_err(|e| eprintln!("warning: bitmap {path}: {e}"))
                        .ok()
                })
            });
        let Some(lif) = lif else { return false };
        let rgba = lif.to_rgba();
        let w = lif.width as usize;
        for (i, px) in rgba.as_chunks::<4>().0.iter().enumerate() {
            let (px_x, px_y) = ((i % w) as i32, (i / w) as i32);
            canvas.blend(x + px_x, y + px_y, rgba8(*px));
        }
        true
    }

    fn font(&mut self, name: &str) -> Option<&Font> {
        let big = self.big;
        self.fonts
            .entry(name.to_ascii_lowercase())
            .or_insert_with(|| {
                let path = format!("{FONT_DIR}{name}");
                let font = big
                    .read_file(&path)
                    .ok()
                    .flatten()
                    .and_then(|d| Font::parse(&d).ok());
                if font.is_none() {
                    eprintln!("warning: font {path} unavailable");
                }
                font
            })
            .as_ref()
    }

    fn draw_text(&mut self, canvas: &mut Canvas, atom: &Atom, text: &str) {
        let Some(font) = atom.font().and_then(|name| self.font(name)) else {
            return;
        };
        // Strings are Latin-1; '&' marks the hot-key letter, which is printed brighter.
        let bytes: Vec<u8> = text
            .chars()
            .map(|c| u8::try_from(c).unwrap_or(b'?'))
            .collect();
        let printed = parse_hotkeys(&bytes);
        let plain: Vec<u8> = printed.iter().map(|&(c, _)| c).collect();
        let width = font.text_width(&plain);
        let r = atom.rect;
        let x = match atom.justification() {
            Justification::Right => r.right - width,
            Justification::Centre => r.left + (r.width() - width) / 2,
            Justification::Left | Justification::Default => r.left,
        };
        if let Some((dx, dy)) = shadow_offset(atom.drop_shadow()) {
            print(
                canvas,
                font,
                x + dx,
                r.top + dy,
                atom.border_color,
                &printed,
            );
        }
        print(canvas, font, x, r.top, atom.content_color, &printed);
    }
}

/// Splits a label into printed characters, flagging the hot-key letter that follows a single
/// `&`; `&&` prints one `&`.
fn parse_hotkeys(bytes: &[u8]) -> Vec<(u8, bool)> {
    let mut out = Vec::with_capacity(bytes.len());
    let mut iter = bytes.iter().copied();
    while let Some(b) = iter.next() {
        if b == b'&' {
            match iter.next() {
                Some(b'&') => out.push((b'&', false)),
                Some(next) => out.push((next, true)),
                None => break,
            }
        } else {
            out.push((b, false));
        }
    }
    out
}

/// `fontPrintN` brightens hot-key letters by 350/256 per channel.
fn hotkey_colour(c: [u8; 4]) -> [u8; 4] {
    let bright = |v: u8| (u32::from(v) * 350 / 256).min(255) as u8;
    [bright(c[0]), bright(c[1]), bright(c[2]), c[3]]
}

/// `fontPrintN`'s shadow offset for the first direction bit set (N, S, E, W, NE, NW, SE, SW).
fn shadow_offset(bits: u8) -> Option<(i32, i32)> {
    const OFFSETS: [(i32, i32); 8] = [
        (0, -1),
        (0, 1),
        (1, 0),
        (-1, 0),
        (1, -1),
        (-1, -1),
        (1, 1),
        (-1, 1),
    ];
    (bits != 0).then(|| OFFSETS[bits.trailing_zeros() as usize])
}

/// Prints glyph coverage as alpha in the given colour (the engine ignores the colour's alpha).
fn print(
    canvas: &mut Canvas,
    font: &Font,
    mut x: i32,
    y: i32,
    colour: [u8; 4],
    text: &[(u8, bool)],
) {
    for &(c, hotkey) in text {
        let Some(glyph) = font.glyph(c) else {
            x += font.spacing;
            continue;
        };
        let [r, g, b, _] = rgba8(if hotkey {
            hotkey_colour(colour)
        } else {
            colour
        });
        if let Some(bitmap) = font.bitmap(c) {
            let ox = x + i32::from(glyph.offset_x);
            let oy = y + i32::from(glyph.offset_y);
            for (i, &cov) in bitmap.coverage.iter().enumerate() {
                if cov == 0 {
                    continue;
                }
                let a = f32::from(coverage_to_alpha(cov)) / 255.0;
                let px = ox + (i % bitmap.width as usize) as i32;
                let py = oy + (i / bitmap.width as usize) as i32;
                canvas.blend(px, py, [r, g, b, a]);
            }
        }
        x += i32::from(glyph.width) + font.spacing;
    }
}

fn is_decorative(atom: &Atom) -> bool {
    matches!(
        atom.kind,
        AtomKind::DecorativeRegion | AtomKind::OpaqueDecorativeRegion
    )
}

fn is_panel(atom: &Atom) -> bool {
    matches!(
        atom.kind,
        AtomKind::Plain | AtomKind::StaticText | AtomKind::Divider
    )
}

fn fill_rect(canvas: &mut Canvas, atom: &Atom, colour: Rgba) {
    let r = atom.rect;
    for y in r.top..r.bottom {
        for x in r.left..r.right {
            canvas.blend(x, y, colour);
        }
    }
}

fn outline_rect(canvas: &mut Canvas, atom: &Atom, colour: Rgba) {
    let r = atom.rect;
    let colour = [colour[0], colour[1], colour[2], colour[3].max(0.8)];
    for x in r.left..r.right {
        canvas.blend(x, r.top, colour);
        canvas.blend(x, r.bottom - 1, colour);
    }
    for y in r.top + 1..r.bottom - 1 {
        canvas.blend(r.left, y, colour);
        canvas.blend(r.right - 1, y, colour);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_markers() {
        assert_eq!(parse_hotkeys(b"&Qu"), [(b'Q', true), (b'u', false)]);
        assert_eq!(
            parse_hotkeys(b"A&&B"),
            [(b'A', false), (b'&', false), (b'B', false)]
        );
        assert_eq!(hotkey_colour([255, 200, 0, 255]), [255, 255, 0, 255]);
    }

    #[test]
    fn shadow_directions() {
        assert_eq!(shadow_offset(0), None);
        assert_eq!(shadow_offset(0x40), Some((1, 1)));
    }
}
