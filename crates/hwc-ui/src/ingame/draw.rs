//! Small 2D helpers shared by the in-game screens (`primTriSolid2`, `primTriOutline2`,
//! translucent rectangles, aligned text).

use hwc_formats::font::Font;

use crate::canvas::{Canvas, Color, Rect};
use crate::text::{self, Shadow};

/// `primTriSolid2`: fills a triangle (pixel centres inside, top-left rule).
pub fn fill_triangle(c: &mut Canvas, p: [(i32, i32); 3], colour: Color) {
    let (x0, x1) = (
        p.iter().map(|v| v.0).min().unwrap_or(0),
        p.iter().map(|v| v.0).max().unwrap_or(0),
    );
    let (y0, y1) = (
        p.iter().map(|v| v.1).min().unwrap_or(0),
        p.iter().map(|v| v.1).max().unwrap_or(0),
    );
    let edge = |a: (i32, i32), b: (i32, i32), x: f32, y: f32| {
        (b.0 as f32 - a.0 as f32) * (y - a.1 as f32) - (b.1 as f32 - a.1 as f32) * (x - a.0 as f32)
    };
    let area = edge(p[0], p[1], p[2].0 as f32, p[2].1 as f32);
    if area == 0.0 {
        return;
    }
    // Only the pixels on the canvas (a corner projected from near the camera can be billions
    // of pixels away).
    let (x0, x1) = (x0.max(0), x1.min(c.width - 1));
    let (y0, y1) = (y0.max(0), y1.min(c.height - 1));
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(p[1], p[2], fx, fy) * area.signum();
            let w1 = edge(p[2], p[0], fx, fy) * area.signum();
            let w2 = edge(p[0], p[1], fx, fy) * area.signum();
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                c.blend(x, y, colour);
            }
        }
    }
}

/// `primTriOutline2`: the three edges, one pixel wide.
pub fn outline_triangle(c: &mut Canvas, p: [(i32, i32); 3], colour: Color) {
    for i in 0..3 {
        let (a, b) = (p[i], p[(i + 1) % 3]);
        c.line(a.0, a.1, b.0, b.1, colour);
    }
}

/// `primRectTranslucent2` + `primRectOutline2`.
pub fn translucent_rect(c: &mut Canvas, r: Rect, fill: Color, border: Option<Color>) {
    c.blend_rect(r, fill);
    if let Some(b) = border {
        c.outline_rect(r, b);
    }
}

/// Text without shadow.
pub fn print(c: &mut Canvas, font: &Font, x: i32, y: i32, colour: Color, s: &str) -> i32 {
    text::print(
        c,
        font,
        x,
        y,
        colour,
        &text::to_latin1(s),
        (Shadow::None, [0, 0, 0, 255]),
    )
}

/// Text with a one-pixel drop shadow in `shadow`.
pub fn print_shadow(
    c: &mut Canvas,
    font: &Font,
    x: i32,
    y: i32,
    colour: Color,
    shadow: Color,
    s: &str,
) -> i32 {
    text::print(
        c,
        font,
        x,
        y,
        colour,
        &text::to_latin1(s),
        (Shadow::SE, shadow),
    )
}

pub fn width(font: &Font, s: &str) -> i32 {
    text::width(font, &text::to_latin1(s))
}

/// Text with its right edge at `right`.
pub fn print_right(c: &mut Canvas, font: &Font, right: i32, y: i32, colour: Color, s: &str) {
    let w = width(font, s);
    print(c, font, right - w, y, colour, s);
}

/// Text centred on `cx`.
pub fn print_centred(c: &mut Canvas, font: &Font, cx: i32, y: i32, colour: Color, s: &str) {
    let w = width(font, s);
    print(c, font, cx - w / 2, y, colour, s);
}

/// Splits text into lines no wider than `width` (words kept whole; `\n` breaks).
pub fn wrap(font: &Font, s: &str, width_px: i32) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if width(font, &candidate) > width_px && !line.is_empty() {
                out.push(std::mem::take(&mut line));
                line = word.to_string();
            } else {
                line = candidate;
            }
        }
        out.push(line);
    }
    out
}

/// `primRectTextured` with blending: the image stretched over `dst`, each texel multiplied by
/// `modulate` (`glColor`), nearest sampling.
pub fn textured_rect(c: &mut Canvas, img: &crate::canvas::Image, dst: Rect, modulate: Color) {
    if dst.width() <= 0 || dst.height() <= 0 || img.width <= 0 || img.height <= 0 {
        return;
    }
    let m = |a: u8, b: u8| ((u32::from(a) * u32::from(b) + 127) / 255) as u8;
    for y in dst.y0..dst.y1 {
        let sy = ((y - dst.y0) as i64 * img.height as i64 / dst.height() as i64) as i32;
        for x in dst.x0..dst.x1 {
            let sx = ((x - dst.x0) as i64 * img.width as i64 / dst.width() as i64) as i32;
            let p = img.get(sx, sy);
            c.blend(
                x,
                y,
                [
                    m(p[0], modulate[0]),
                    m(p[1], modulate[1]),
                    m(p[2], modulate[2]),
                    m(p[3], modulate[3]),
                ],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_fill_stays_inside() {
        let mut c = Canvas::new(20, 20);
        c.clear([0, 0, 0, 255]);
        fill_triangle(&mut c, [(2, 2), (2, 12), (12, 7)], [255, 0, 0, 255]);
        assert_eq!(c.get(4, 7), [255, 0, 0, 255]);
        assert_eq!(c.get(15, 7), [0, 0, 0, 255]);
        assert_eq!(c.get(11, 2), [0, 0, 0, 255]);
    }
}
