//! `.btg` sky backgrounds, drawn the way `btgRender` does: the 2D page is wrapped onto a sphere
//! around the camera, the nebula triangles are Gouraud-shaded with their vertex colours, and
//! the stars are additive screen-aligned sprites at their texture's pixel size.

use std::collections::HashMap;

use hwc_formats::big::BigArchive;
use hwc_formats::btg::Btg;

use super::canvas::{Canvas, Texture};
use super::math::{orbit_direction, Camera, Vec3};
use super::raster::{draw_triangle, ClipVertex, Cull, Target};
use super::tga;

/// The engine's world is Z-up; the page's top edge maps to +Z.
const WORLD_UP: Vec3 = Vec3::new(0.0, 0.0, 1.0);
const WORLD_FRONT: Vec3 = Vec3::new(1.0, 0.0, 0.0);
const FOV_Y: f32 = 60.0;

pub struct Background {
    pub btg: Btg,
    /// Star sprites by texture name; `None` when the bitmap is missing.
    sprites: HashMap<String, Option<Texture>>,
}

/// Where the camera looks, in degrees.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkyView {
    pub yaw: f32,
    pub pitch: f32,
}

impl Background {
    pub fn load(big: &BigArchive, path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let data = big
            .read_file(path)?
            .ok_or_else(|| format!("{path}: not in archive"))?;
        let btg = Btg::parse(&data).map_err(|e| format!("{path}: {e}"))?;
        let mut sprites = HashMap::new();
        for star in &btg.stars {
            sprites.entry(star.texture.clone()).or_insert_with(|| {
                let sprite_path = format!("btg\\bitmaps\\{}", star.texture);
                let loaded = big
                    .read_file(&sprite_path)
                    .map_err(|e| e.to_string())
                    .and_then(|d| d.ok_or_else(|| "not in archive".to_string()))
                    .and_then(|d| tga::decode(&d));
                loaded
                    .map_err(|e| eprintln!("warning: star sprite {sprite_path}: {e}"))
                    .ok()
            });
        }
        Ok(Self { btg, sprites })
    }

    /// Unprojects a page position onto the unit sphere (`btgConvertAVert` with zero offsets).
    fn sphere_point(&self, x: f64, y: f64) -> Vec3 {
        let [pw, ph] = self.btg.page_size.map(f64::from);
        let x = (pw - 1.0) - x;
        let theta = (2.0 * std::f64::consts::PI * x / pw + std::f64::consts::FRAC_PI_2) as f32;
        let phi = (std::f64::consts::PI * y / ph) as f32;
        Vec3::new(theta.cos() * phi.sin(), theta.sin() * phi.sin(), phi.cos())
    }

    pub fn camera(view: SkyView, aspect: f32) -> Camera {
        let forward = orbit_direction(WORLD_FRONT, WORLD_UP, view.yaw, view.pitch);
        Camera::new(Vec3::default(), forward, WORLD_UP, FOV_Y, aspect)
    }

    /// Fills `canvas` with the background as seen from the sphere's centre.
    pub fn render(&self, canvas: &mut Canvas, view: SkyView) {
        let bg = self.btg.background.map(|c| c.clamp(0, 255));
        let clear = [bg[0], bg[1], bg[2]].map(|c| c as f32 / 255.0);
        canvas.pixels.fill([clear[0], clear[1], clear[2], 1.0]);
        let camera = Self::camera(view, canvas.width as f32 / canvas.height as f32);

        // Vertex colours as btgColorVertices computes them without fast blending: scale by
        // alpha × brightness and never drop below the clear colour unless nearly opaque.
        let colours: Vec<[f32; 3]> = self
            .btg
            .vertices
            .iter()
            .map(|v| {
                let a = (v.color[3] * v.brightness * 255) >> 16;
                std::array::from_fn(|k| {
                    let c = (v.color[k] * a) >> 8;
                    let c = if a < 252 { c.max(bg[k]) } else { c };
                    c.clamp(0, 255) as f32 / 255.0
                })
            })
            .collect();
        let positions: Vec<Vec3> = self
            .btg
            .vertices
            .iter()
            .map(|v| self.sphere_point(v.position[0], v.position[1]))
            .collect();

        let mut target = Target {
            width: canvas.width,
            height: canvas.height,
            depth: None,
        };
        for poly in &self.btg.polygons {
            let tri = poly.vertices.map(|i| ClipVertex {
                clip: camera.project(positions[i as usize]),
                attrs: colours[i as usize],
            });
            draw_triangle(&mut target, tri, Cull::None, |f| {
                let [r, g, b] = f.attrs;
                canvas.set(f.x, f.y, [r, g, b, 1.0]);
                true
            });
        }

        // Stars: additive sprites centred on the projected star position.
        let (w, h) = (canvas.width as f32, canvas.height as f32);
        for star in &self.btg.stars {
            let Some(Some(sprite)) = self.sprites.get(&star.texture) else {
                continue;
            };
            let [x, y, _, cw] =
                camera.project(self.sphere_point(star.position[0], star.position[1]));
            if cw <= 0.0 {
                continue;
            }
            let cx = (x / cw * 0.5 + 0.5) * w;
            let cy = (0.5 - y / cw * 0.5) * h;
            let tint = star.color.map(|c| c.clamp(0, 255) as f32 / 255.0);
            let x0 = (cx - sprite.width as f32 / 2.0).round() as i32;
            let y0 = (cy - sprite.height as f32 / 2.0).round() as i32;
            for sy in 0..sprite.height {
                for sx in 0..sprite.width {
                    let t = sprite.texel(sx, sy);
                    // The engine inverts the stored alpha when it loads star bitmaps.
                    let a = 1.0 - t[3];
                    let c = [t[0] * tint[0], t[1] * tint[1], t[2] * tint[2], a];
                    canvas.add(x0 + sx as i32, y0 + sy as i32, c);
                }
            }
        }
    }
}
