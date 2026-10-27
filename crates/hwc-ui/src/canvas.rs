//! Software 2D target: an RGBA8 framebuffer with the handful of primitives the front end uses.
//!
//! The original draws its menus through OpenGL with blending switched off and an alpha test on
//! every bitmap, so a pixel is either written whole or left alone. Rasterising that on the CPU is
//! exact and cheap at menu resolutions; the window then shows the finished frame as one texture
//! (see [`crate::present`]).

use std::path::Path;

/// RGBA colour, 8 bits per channel.
pub type Color = [u8; 4];

pub const BLACK: Color = [0, 0, 0, 255];
pub const WHITE: Color = [255, 255, 255, 255];

/// How far outside the clip a line's ends may be before the line is cut to it first.
const LINE_REACH: i32 = 1 << 14;

/// Liang-Barsky: the part of the segment `a`-`b` inside `[x0, y0, x1, y1]`, as the parameters
/// along it.
fn clip_segment(a: [f64; 2], b: [f64; 2], r: [f64; 4]) -> Option<(f64, f64)> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [
        (-d[0], a[0] - r[0]),
        (d[0], r[2] - a[0]),
        (-d[1], a[1] - r[1]),
        (d[1], r[3] - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then_some((t0, t1))
}

/// Rectangle with exclusive right/bottom edges, the engine's `rectangle` (x0, y0, x1, y1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub const fn new(x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self::new(x, y, x + w, y + h)
    }

    pub fn width(&self) -> i32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> i32 {
        self.y1 - self.y0
    }

    /// Point test used by mouse hit testing (inclusive top-left, exclusive bottom-right).
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x0 && y >= self.y0 && x < self.x1 && y < self.y1
    }

    /// Strictly-inside test (`IS_IN_RECT` in the frame code: all four edges exclusive).
    pub fn strictly_contains(&self, x: i32, y: i32) -> bool {
        x > self.x0 && x < self.x1 && y > self.y0 && y < self.y1
    }

    pub fn offset(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x0 + dx, self.y0 + dy, self.x1 + dx, self.y1 + dy)
    }
}

/// How a bitmap is combined with what is already on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlitMode {
    /// Alpha test: pixels whose alpha is strictly greater than the threshold are written
    /// unblended, the rest are skipped (`glAlphaFunc(GL_GREATER, t)` with blending off).
    AlphaTest(u8),
    /// Straight-alpha "over" blending.
    Blend,
    /// Every pixel written as is (the `glDrawPixels` path for oversized decorations).
    Opaque,
}

/// An RGBA8 image, rows top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<Color>,
}

impl Image {
    pub fn new(width: i32, height: i32, fill: Color) -> Self {
        let n = (width.max(0) * height.max(0)) as usize;
        Self {
            width,
            height,
            pixels: vec![fill; n],
        }
    }

    /// Builds an image from tightly packed RGBA8 bytes.
    pub fn from_rgba(width: i32, height: i32, rgba: &[u8]) -> Self {
        let pixels = rgba.as_chunks::<4>().0.to_vec();
        debug_assert_eq!(pixels.len(), (width * height) as usize);
        Self {
            width,
            height,
            pixels,
        }
    }

    pub fn get(&self, x: i32, y: i32) -> Color {
        self.pixels[(y * self.width + x) as usize]
    }

    /// The image turned a quarter turn clockwise. Like the engine, which only ever rotates its
    /// square frame pieces, the result is `width` x `width`: rows past the source height read as
    /// transparent.
    pub fn rotated_cw(&self) -> Image {
        let n = self.width;
        let mut out = Image::new(n, n, [0; 4]);
        for y in 0..n {
            for x in 0..n {
                // Destination (x, y) shows source (y, n-1-x).
                let (sx, sy) = (y, n - 1 - x);
                if sy < self.height {
                    out.pixels[(y * n + x) as usize] = self.get(sx, sy);
                }
            }
        }
        out
    }

    /// Rotates clockwise `quarters` times.
    pub fn rotated(&self, quarters: u32) -> Image {
        let mut img = self.clone();
        for _ in 0..quarters % 4 {
            img = img.rotated_cw();
        }
        img
    }
}

/// The framebuffer.
#[derive(Debug, Clone)]
pub struct Canvas {
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<Color>,
    clip: Rect,
}

impl Canvas {
    pub fn new(width: i32, height: i32) -> Self {
        Self {
            width,
            height,
            pixels: vec![BLACK; (width * height).max(0) as usize],
            clip: Rect::new(0, 0, width, height),
        }
    }

    /// Resizes (contents are cleared).
    pub fn resize(&mut self, width: i32, height: i32) {
        if (width, height) != (self.width, self.height) {
            *self = Canvas::new(width, height);
        }
    }

    pub fn clear(&mut self, c: Color) {
        self.pixels.fill(c);
    }

    pub fn get(&self, x: i32, y: i32) -> Color {
        self.pixels[(y * self.width + x) as usize]
    }

    /// Restricts drawing to `r` (intersected with the canvas).
    pub fn set_clip(&mut self, r: Option<Rect>) {
        let full = Rect::new(0, 0, self.width, self.height);
        self.clip = match r {
            None => full,
            Some(r) => Rect::new(
                r.x0.max(0),
                r.y0.max(0),
                r.x1.min(self.width),
                r.y1.min(self.height),
            ),
        };
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        self.clip
            .contains(x, y)
            .then(|| (y * self.width + x) as usize)
    }

    /// Writes a pixel unblended (alpha forced opaque).
    pub fn put(&mut self, x: i32, y: i32, c: Color) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = [c[0], c[1], c[2], 255];
        }
    }

    /// Blends a pixel with straight alpha.
    pub fn blend(&mut self, x: i32, y: i32, c: Color) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = over(c, self.pixels[i]);
        }
    }

    /// Solid rectangle, unblended (`primRectSolid2` with blending off).
    pub fn fill_rect(&mut self, r: Rect, c: Color) {
        let (x0, x1) = (r.x0.max(self.clip.x0), r.x1.min(self.clip.x1));
        let (y0, y1) = (r.y0.max(self.clip.y0), r.y1.min(self.clip.y1));
        // A rectangle wholly outside the clip (e.g. off the right edge) draws nothing.
        if x0 >= x1 {
            return;
        }
        let c = [c[0], c[1], c[2], 255];
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            self.pixels[row + x0 as usize..row + x1 as usize].fill(c);
        }
    }

    /// Translucent rectangle (`primRectTranslucent2`). Only the part inside the clip is
    /// visited (a rectangle projected from near the camera can be billions of pixels wide).
    pub fn blend_rect(&mut self, r: Rect, c: Color) {
        let (x0, x1) = (r.x0.max(self.clip.x0), r.x1.min(self.clip.x1));
        let (y0, y1) = (r.y0.max(self.clip.y0), r.y1.min(self.clip.y1));
        for y in y0..y1 {
            for x in x0..x1 {
                self.blend(x, y, c);
            }
        }
    }

    /// Additive rectangle (`SRC_ALPHA, ONE`): `c * alpha` added to every pixel, saturating.
    pub fn add_rect(&mut self, r: Rect, c: Color) {
        let (x0, x1) = (r.x0.max(self.clip.x0), r.x1.min(self.clip.x1));
        let (y0, y1) = (r.y0.max(self.clip.y0), r.y1.min(self.clip.y1));
        let a = u32::from(c[3]);
        let add = [0, 1, 2].map(|i| (u32::from(c[i]) * a / 255) as u8);
        for y in y0..y1 {
            let row = (y * self.width) as usize;
            for p in &mut self.pixels[row + x0 as usize..row + x1.max(x0) as usize] {
                for i in 0..3 {
                    p[i] = p[i].saturating_add(add[i]);
                }
            }
        }
    }

    /// One-pixel outline of `r`, bottom edge at `y1 - 1` like `primRectOutline2`.
    /// Only the pixels inside the clip are visited.
    pub fn outline_rect(&mut self, r: Rect, c: Color) {
        let bottom = r.y1.saturating_sub(1);
        let (cx0, cx1) = (self.clip.x0, self.clip.x1 - 1);
        let (cy0, cy1) = (self.clip.y0, self.clip.y1 - 1);
        for x in r.x0.max(cx0)..=r.x1.min(cx1) {
            self.put(x, r.y0, c);
            self.put(x, bottom, c);
        }
        for y in r.y0.max(cy0)..=bottom.min(cy1) {
            self.put(r.x0, y, c);
            self.put(r.x1, y, c);
        }
    }

    /// Non-antialiased line (both endpoints drawn).
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        self.line_stippled(x0, y0, x1, y1, c, 0xffff);
    }

    /// A line drawn through a 16-pixel stipple pattern like `glLineStipple(1, pattern)`: the
    /// n-th pixel from the start is drawn when bit `n % 16` of the pattern is set.
    ///
    /// A line reaching far outside the canvas (an end projected from a point beside the camera
    /// comes out at billions of pixels, or saturated at `i32::MAX`) is first cut to the
    /// clip with a margin, its pattern kept in step; walking all of it would take minutes,
    /// and past `2^30` pixels the stepping overflows and never reaches its end.
    pub fn line_stippled(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color, pattern: u16) {
        self.line_clipped(x0, y0, x1, y1, c, pattern, false);
    }

    /// Non-antialiased line blended with straight alpha (both endpoints drawn).
    pub fn line_blend(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Color) {
        self.line_clipped(x0, y0, x1, y1, c, 0xffff, true);
    }

    /// [`line_blend`](Self::line_blend) through a 16-pixel stipple pattern.
    pub fn line_blend_stippled(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        c: Color,
        pattern: u16,
    ) {
        self.line_clipped(x0, y0, x1, y1, c, pattern, true);
    }

    /// [`line_stippled`](Self::line_stippled), each pixel put or (`blend`) blended.
    #[allow(clippy::too_many_arguments)]
    fn line_clipped(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        c: Color,
        pattern: u16,
        blend: bool,
    ) {
        let near = |v: i32, lo: i32, hi: i32| v >= lo - LINE_REACH && v <= hi + LINE_REACH;
        let (cl, ct, cr, cb) = (self.clip.x0, self.clip.y0, self.clip.x1, self.clip.y1);
        if near(x0, cl, cr) && near(x1, cl, cr) && near(y0, ct, cb) && near(y1, ct, cb) {
            self.bresenham(x0, y0, x1, y1, c, pattern, 0, blend);
            return;
        }
        let Some((t0, t1)) = clip_segment(
            [f64::from(x0), f64::from(y0)],
            [f64::from(x1), f64::from(y1)],
            [
                f64::from(cl) - 2.0,
                f64::from(ct) - 2.0,
                f64::from(cr) + 2.0,
                f64::from(cb) + 2.0,
            ],
        ) else {
            return;
        };
        let at = |t: f64| {
            (
                (f64::from(x0) + (f64::from(x1) - f64::from(x0)) * t).round() as i32,
                (f64::from(y0) + (f64::from(y1) - f64::from(y0)) * t).round() as i32,
            )
        };
        let (a, b) = (at(t0), at(t1));
        // The pattern goes on from where the whole line's walk would be.
        let skipped = (i64::from(a.0) - i64::from(x0))
            .abs()
            .max((i64::from(a.1) - i64::from(y0)).abs());
        self.bresenham(a.0, a.1, b.0, b.1, c, pattern, (skipped % 16) as u32, blend);
    }

    /// The line's walk, both ends drawn, from bit `phase` of the pattern; the ends are near
    /// enough to the canvas for the error terms to stay small.
    #[allow(clippy::too_many_arguments)]
    fn bresenham(
        &mut self,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        c: Color,
        pattern: u16,
        phase: u32,
        blend: bool,
    ) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        let mut n = phase;
        loop {
            if pattern & (1 << (n % 16)) != 0 {
                if blend {
                    self.blend(x, y, c);
                } else {
                    self.put(x, y, c);
                }
            }
            n += 1;
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draws `img` with its top-left corner at `(x, y)`.
    pub fn blit(&mut self, img: &Image, x: i32, y: i32, mode: BlitMode) {
        // Clip the source span once per row.
        let sx0 = (self.clip.x0 - x).max(0);
        let sx1 = (self.clip.x1 - x).min(img.width);
        let sy0 = (self.clip.y0 - y).max(0);
        let sy1 = (self.clip.y1 - y).min(img.height);
        if sx0 >= sx1 || sy0 >= sy1 {
            return;
        }
        for sy in sy0..sy1 {
            let src = (sy * img.width) as usize;
            let row = (y + sy) * self.width + x;
            for sx in sx0..sx1 {
                let p = img.pixels[src + sx as usize];
                let d = &mut self.pixels[(row + sx) as usize];
                match mode {
                    BlitMode::AlphaTest(t) => {
                        if p[3] > t {
                            *d = [p[0], p[1], p[2], 255];
                        }
                    }
                    BlitMode::Blend => *d = over(p, *d),
                    BlitMode::Opaque => *d = [p[0], p[1], p[2], 255],
                }
            }
        }
    }

    /// Draws `img` stretched into `dst` with nearest sampling (used for full-screen pictures).
    pub fn blit_scaled(&mut self, img: &Image, dst: Rect, alpha: u8) {
        if dst.width() <= 0 || dst.height() <= 0 || img.width <= 0 || img.height <= 0 {
            return;
        }
        for y in dst.y0..dst.y1 {
            let sy = ((y - dst.y0) as i64 * img.height as i64 / dst.height() as i64) as i32;
            for x in dst.x0..dst.x1 {
                let sx = ((x - dst.x0) as i64 * img.width as i64 / dst.width() as i64) as i32;
                let mut p = img.get(sx, sy);
                p[3] = ((p[3] as u32 * alpha as u32) / 255) as u8;
                self.blend(x, y, p);
            }
        }
    }

    /// Tightly packed RGBA8 bytes.
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels.iter().flatten().copied().collect()
    }

    pub fn save_png(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().map_err(std::io::Error::other)?;
        writer
            .write_image_data(&self.to_rgba8())
            .map_err(std::io::Error::other)?;
        writer.finish().map_err(std::io::Error::other)?;
        Ok(())
    }
}

/// Straight-alpha "over" on 8-bit colours; the destination stays opaque.
pub fn over(src: Color, dst: Color) -> Color {
    let a = src[3] as u32;
    if a == 255 {
        return [src[0], src[1], src[2], 255];
    }
    if a == 0 {
        return dst;
    }
    let mix = |s: u8, d: u8| ((s as u32 * a + d as u32 * (255 - a) + 127) / 255) as u8;
    [
        mix(src[0], dst[0]),
        mix(src[1], dst[1]),
        mix(src[2], dst[2]),
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stippled_lines_follow_the_pattern() {
        let mut c = Canvas::new(32, 1);
        c.clear([0, 0, 0, 255]);
        c.line_stippled(0, 0, 31, 0, [255, 255, 255, 255], 0xff00);
        let lit: Vec<bool> = (0..32).map(|x| c.get(x, 0)[0] == 255).collect();
        for (x, &on) in lit.iter().enumerate() {
            assert_eq!(on, x % 16 >= 8, "pixel {x}");
        }
        let mut d = Canvas::new(8, 8);
        d.line(0, 0, 7, 7, [255, 0, 0, 255]);
        assert!((0..8).all(|i| d.get(i, i)[0] == 255));
    }

    /// Lines and rectangles from points projected from beside the camera (saturated to the
    /// `i32` range) end at once, and draw what they cross of the canvas.
    #[test]
    fn huge_lines_and_rectangles_end() {
        let mut c = Canvas::new(64, 48);
        c.clear(BLACK);
        let started = std::time::Instant::now();
        c.line(10, 10, i32::MAX, i32::MIN, WHITE);
        c.line(i32::MIN, 20, i32::MAX, 20, WHITE);
        c.line(5, 5, 900_000_000, 5, WHITE);
        c.line_stippled(-2_000_000_000, 30, 2_000_000_000, 30, WHITE, 0xf0f0);
        let mut r = Canvas::new(64, 48);
        r.clear(BLACK);
        let huge = Rect::new(i32::MIN / 2, i32::MIN / 2, i32::MAX / 2, i32::MAX / 2);
        r.blend_rect(huge, [255, 0, 0, 128]);
        r.outline_rect(
            Rect::new(-1_000_000_000, -1_000_000_000, 1_000_000_000, 1_000_000_000),
            WHITE,
        );
        r.outline_rect(Rect::new(2, 2, i32::MAX, i32::MAX), WHITE);
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        // The horizontal lines cross the whole width; the first goes up and to the right.
        assert!((0..64).all(|x| c.get(x, 20)[1] == 255), "row 20");
        assert!((5..64).all(|x| c.get(x, 5)[1] == 255), "row 5");
        assert_eq!(c.get(10, 10)[1], 255);
        // The stipple stays in step with the whole line's walk.
        let lit = (0..64).filter(|&x| c.get(x, 30)[1] == 255).count();
        assert_eq!(lit, 32);
        assert!(r.get(30, 30)[0] > 100 && r.get(30, 30)[1] == 0);
        assert_eq!(r.get(2, 10), WHITE);
    }

    /// Lines whose ends are within reach of the canvas are walked as before, pixel for pixel.
    #[test]
    fn lines_near_the_canvas_are_unchanged() {
        let reference = |x0: i32, y0: i32, x1: i32, y1: i32, pattern: u16| {
            let mut lit = std::collections::HashSet::new();
            let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
            let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
            let (mut x, mut y, mut err, mut n) = (x0, y0, dx + dy, 0u32);
            loop {
                if pattern & (1 << (n % 16)) != 0 {
                    lit.insert((x, y));
                }
                n += 1;
                if x == x1 && y == y1 {
                    break lit;
                }
                let e2 = 2 * err;
                if e2 >= dy {
                    err += dy;
                    x += sx;
                }
                if e2 <= dx {
                    err += dx;
                    y += sy;
                }
            }
        };
        for (x0, y0, x1, y1, pattern) in [
            (-300, -200, 500, 77, 0xffff),
            (39, 0, -1000, 29, 0x3c3c),
            (-16000, 15, 16000, 16, 0x0ff0),
        ] {
            let mut a = Canvas::new(40, 30);
            a.clear(BLACK);
            a.line_stippled(x0, y0, x1, y1, WHITE, pattern);
            let want = reference(x0, y0, x1, y1, pattern);
            for y in 0..30 {
                for x in 0..40 {
                    assert_eq!(a.get(x, y) == WHITE, want.contains(&(x, y)), "({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn rotation_is_clockwise() {
        // 2x2: red top-left, green top-right.
        let mut img = Image::new(2, 2, [0; 4]);
        img.pixels[0] = [255, 0, 0, 255];
        img.pixels[1] = [0, 255, 0, 255];
        let r = img.rotated_cw();
        // Red moves to the top-right, green to the bottom-right.
        assert_eq!(r.get(1, 0), [255, 0, 0, 255]);
        assert_eq!(r.get(1, 1), [0, 255, 0, 255]);
        assert_eq!(img.rotated(4), img);
    }

    #[test]
    fn alpha_test_and_clip() {
        let mut c = Canvas::new(4, 4);
        let mut img = Image::new(2, 1, [10, 20, 30, 1]);
        img.pixels[1] = [40, 50, 60, 2];
        c.blit(&img, 3, 0, BlitMode::AlphaTest(1));
        // Only the first pixel lands on the canvas, and it fails the alpha test.
        assert_eq!(c.get(3, 0), BLACK);
        c.blit(&img, 2, 0, BlitMode::AlphaTest(1));
        assert_eq!(c.get(3, 0), [40, 50, 60, 255]);
        assert_eq!(c.get(2, 0), BLACK);
    }

    #[test]
    fn fill_and_outline() {
        let mut c = Canvas::new(8, 8);
        c.fill_rect(Rect::new(-2, 1, 3, 3), [1, 2, 3, 0]);
        assert_eq!(c.get(0, 1), [1, 2, 3, 255]);
        assert_eq!(c.get(3, 1), BLACK);
        c.outline_rect(Rect::new(4, 4, 6, 7), WHITE);
        assert_eq!(c.get(6, 4), WHITE);
        assert_eq!(c.get(4, 6), WHITE);
        assert_eq!(c.get(5, 5), BLACK);
    }
}

#[cfg(test)]
mod fill_rect_tests {
    use super::*;

    #[test]
    fn fill_rect_off_the_right_edge_draws_nothing() {
        let mut c = Canvas::new(8, 4);
        c.fill_rect(Rect::new(20, 0, 30, 4), [255, 0, 0, 255]);
        assert!(c.pixels.iter().all(|p| *p == BLACK));
    }
}
