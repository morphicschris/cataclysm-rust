//! Overlay text drawn with one of the game's `.hff` bitmap fonts: every glyph row becomes a few
//! filled one-pixel-high runs in the [`Overlay`], so the same text shows in the window and in
//! offscreen screenshots.

use hwc_formats::font::Font;
use hwc_render::AssetSource;

use crate::overlay::Overlay;

/// Coverage levels a run is quantised to (keeps the number of rectangles small).
const LEVELS: u8 = 4;

/// Runs of one glyph: `(x, y, width, coverage level)` relative to the glyph's cell origin.
type GlyphRuns = Vec<(i32, i32, i32, u8)>;

/// A font ready to lay text out into an overlay.
pub struct TextFont {
    font: Font,
    /// Per character: rows of `(x, width, level)` runs relative to the glyph's cell origin.
    runs: Vec<Option<GlyphRuns>>,
}

impl TextFont {
    /// Loads `fonts\<language>\<name>` (e.g. `arial_13.hff`).
    pub fn load(src: &dyn AssetSource, name: &str) -> Option<Self> {
        let data = src.read(&format!("fonts\\english\\{name}"))?;
        let font = Font::parse(&data).ok()?;
        let runs = (0..=255u8)
            .map(|code| {
                let g = font.glyph(code)?;
                let bm = font.bitmap(code)?;
                let mut out = Vec::new();
                for y in 0..bm.height as usize {
                    let row = &bm.coverage[y * bm.width as usize..(y + 1) * bm.width as usize];
                    let mut x = 0;
                    while x < row.len() {
                        let level = quantise(row[x]);
                        if level == 0 {
                            x += 1;
                            continue;
                        }
                        let start = x;
                        while x < row.len() && quantise(row[x]) == level {
                            x += 1;
                        }
                        out.push((
                            i32::from(g.offset_x) + start as i32,
                            i32::from(g.offset_y) + y as i32,
                            (x - start) as i32,
                            level,
                        ));
                    }
                }
                Some(out)
            })
            .collect();
        Some(Self { font, runs })
    }

    pub fn line_height(&self) -> f32 {
        self.font.full_height.max(8) as f32
    }

    /// Width of `text` in pixels.
    pub fn width(&self, text: &str) -> f32 {
        self.font.text_width(&bytes(text)) as f32
    }

    /// Draws `text` with its top-left corner at `(x, y)`, with a one-pixel shadow.
    pub fn draw(&self, o: &mut Overlay, x: f32, y: f32, text: &str, color: [f32; 4]) {
        let shadow = [0.0, 0.0, 0.0, color[3] * 0.8];
        self.draw_plain(o, x + 1.0, y + 1.0, text, shadow);
        self.draw_plain(o, x, y, text, color);
    }

    fn draw_plain(&self, o: &mut Overlay, x: f32, y: f32, text: &str, color: [f32; 4]) {
        let (x, y) = (x.round(), y.round());
        let mut pen = 0i32;
        for c in bytes(text) {
            let Some(g) = self.font.glyph(c) else {
                pen += self.font.spacing;
                continue;
            };
            if let Some(runs) = &self.runs[usize::from(c)] {
                for &(rx, ry, w, level) in runs {
                    let a = color[3] * f32::from(level) / f32::from(LEVELS);
                    let lo = [x + (pen + rx) as f32, y + ry as f32];
                    o.fill(lo, [lo[0] + w as f32, lo[1] + 1.0], [color[0], color[1], color[2], a]);
                }
            }
            pen += i32::from(g.width) + self.font.spacing;
        }
    }

    /// Splits `text` into lines no wider than `max_width` pixels (at word boundaries).
    pub fn wrap(&self, text: &str, max_width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for para in text.split('\n') {
            let mut line = String::new();
            for word in para.split_whitespace() {
                let candidate = if line.is_empty() {
                    word.to_owned()
                } else {
                    format!("{line} {word}")
                };
                if !line.is_empty() && self.width(&candidate) > max_width {
                    lines.push(std::mem::take(&mut line));
                    line = word.to_owned();
                } else {
                    line = candidate;
                }
            }
            lines.push(line);
        }
        lines
    }
}

fn quantise(coverage: u8) -> u8 {
    // 0..15 coverage to 0..LEVELS, rounding.
    ((u16::from(coverage.min(15)) * u16::from(LEVELS) + 7) / 15) as u8
}

/// The game's fonts are 8-bit (Latin-1); other characters become '?'.
fn bytes(text: &str) -> Vec<u8> {
    text.chars()
        .map(|c| u8::try_from(u32::from(c)).unwrap_or(b'?'))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coverage_levels() {
        assert_eq!(quantise(0), 0);
        assert_eq!(quantise(15), LEVELS);
        assert_eq!(quantise(1), 0);
        assert!(quantise(8) >= 2);
    }

    #[test]
    fn latin1_bytes() {
        assert_eq!(bytes("aé€"), vec![b'a', 0xe9, b'?']);
    }
}
