//! Floating-point RGBA image, textures and PNG output.

use std::path::Path;

/// Colour with components in 0..=1 (values may exceed 1 while accumulating additive light).
pub type Rgba = [f32; 4];

/// An RGBA image with `f32` components, rows top to bottom.
#[derive(Debug, Clone)]
pub struct Canvas {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<Rgba>,
}

impl Canvas {
    pub fn new(width: usize, height: usize, fill: Rgba) -> Self {
        Self {
            width,
            height,
            pixels: vec![fill; width * height],
        }
    }

    pub fn get(&self, x: usize, y: usize) -> Rgba {
        self.pixels[y * self.width + x]
    }

    pub fn set(&mut self, x: usize, y: usize, c: Rgba) {
        self.pixels[y * self.width + x] = c;
    }

    /// "Over" blend of `c` (straight alpha) onto the pixel, if it lies inside the image.
    pub fn blend(&mut self, x: i32, y: i32, c: Rgba) {
        if let Some(i) = self.index(x, y) {
            self.pixels[i] = over(c, self.pixels[i]);
        }
    }

    /// Adds `c * c.alpha` to the colour channels (GL `SRC_ALPHA, ONE` blending).
    pub fn add(&mut self, x: i32, y: i32, c: Rgba) {
        if let Some(i) = self.index(x, y) {
            let p = &mut self.pixels[i];
            for k in 0..3 {
                p[k] += c[k] * c[3];
            }
        }
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        let (x, y) = (usize::try_from(x).ok()?, usize::try_from(y).ok()?);
        (x < self.width && y < self.height).then(|| y * self.width + x)
    }

    /// Draws `other` over this canvas (same size), using its alpha.
    pub fn composite_over(&mut self, other: &Canvas) {
        assert_eq!((self.width, self.height), (other.width, other.height));
        for (d, &s) in self.pixels.iter_mut().zip(&other.pixels) {
            *d = over(s, *d);
        }
    }

    /// Box-filters the image down by an integer factor (used for supersampling).
    pub fn downsample(&self, factor: usize) -> Canvas {
        if factor <= 1 {
            return self.clone();
        }
        let (w, h) = (self.width / factor, self.height / factor);
        let n = (factor * factor) as f32;
        let mut out = Canvas::new(w, h, [0.0; 4]);
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for sy in 0..factor {
                    for sx in 0..factor {
                        let p = self.get(x * factor + sx, y * factor + sy);
                        // Premultiply so transparent samples do not darken edges.
                        for k in 0..3 {
                            acc[k] += p[k] * p[3];
                        }
                        acc[3] += p[3];
                    }
                }
                let a = acc[3] / n;
                let c = if acc[3] > 0.0 {
                    [acc[0] / acc[3], acc[1] / acc[3], acc[2] / acc[3], a]
                } else {
                    [0.0; 4]
                };
                out.set(x, y, c);
            }
        }
        out
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        self.pixels
            .iter()
            .flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8))
            .collect()
    }

    pub fn save_png(&self, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width as u32, self.height as u32);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header()?;
        writer.write_image_data(&self.to_rgba8())?;
        writer.finish()?;
        Ok(())
    }
}

/// Porter-Duff "over" with straight alpha.
pub fn over(src: Rgba, dst: Rgba) -> Rgba {
    let sa = src[3].clamp(0.0, 1.0);
    let da = dst[3] * (1.0 - sa);
    let a = sa + da;
    if a <= 0.0 {
        return [0.0; 4];
    }
    let mix = |k: usize| (src[k] * sa + dst[k] * da) / a;
    [mix(0), mix(1), mix(2), a]
}

pub fn rgba8(c: [u8; 4]) -> Rgba {
    c.map(|v| f32::from(v) / 255.0)
}

/// An RGBA8 texture, rows top to bottom.
#[derive(Debug, Clone)]
pub struct Texture {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

impl Texture {
    pub fn new(width: usize, height: usize, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), width * height * 4);
        Self {
            width,
            height,
            rgba,
        }
    }

    pub fn texel(&self, x: usize, y: usize) -> Rgba {
        let i = (y * self.width + x) * 4;
        rgba8(self.rgba[i..i + 4].try_into().unwrap())
    }

    /// Bilinear sample with wrapping, `(0, 0)` at the top-left corner of the first row.
    pub fn sample(&self, u: f32, v: f32) -> Rgba {
        let x = u * self.width as f32 - 0.5;
        let y = v * self.height as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |c: f32, n: usize| (c as i64).rem_euclid(n as i64) as usize;
        let (xa, xb) = (wrap(x0, self.width), wrap(x0 + 1.0, self.width));
        let (ya, yb) = (wrap(y0, self.height), wrap(y0 + 1.0, self.height));
        let lerp = |a: Rgba, b: Rgba, t: f32| -> Rgba {
            std::array::from_fn(|k| a[k] + (b[k] - a[k]) * t)
        };
        let top = lerp(self.texel(xa, ya), self.texel(xb, ya), fx);
        let bottom = lerp(self.texel(xa, yb), self.texel(xb, yb), fx);
        lerp(top, bottom, fy)
    }
}
