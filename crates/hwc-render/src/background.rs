//! `.btg` sky backgrounds. The 2D page is wrapped onto a sphere around the eye: nebula
//! triangles are Gouraud-shaded with their vertex colours over the clear colour, and stars are
//! additive screen-aligned sprites drawn at their bitmap's pixel size.

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use hwc_formats::btg::Btg;

use crate::assets::{AssetSource, Result};
use crate::tga::{self, Image};

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub struct SkyVertex {
    /// Unit direction from the eye.
    pub direction: [f32; 3],
    pub color: [f32; 3],
}

/// Stars sharing one sprite bitmap.
#[derive(Debug, Clone)]
pub struct StarGroup {
    pub texture: String,
    pub image: Image,
    /// Per star: direction and RGB tint.
    pub stars: Vec<(Vec3, [f32; 3])>,
}

/// A background ready for upload.
#[derive(Debug, Clone)]
pub struct Background {
    pub name: String,
    pub btg: Btg,
    pub clear_color: [f32; 3],
    /// Triangle list.
    pub vertices: Vec<SkyVertex>,
    pub star_groups: Vec<StarGroup>,
}

/// Unprojects a page position onto the unit sphere: the page's width spans 360° of azimuth
/// (mirrored) and its height 180° from +Z down to -Z.
pub fn page_to_sphere(page_size: [i32; 2], x: f64, y: f64) -> Vec3 {
    let [pw, ph] = page_size.map(|v| f64::from(v.max(1)));
    let x = (pw - 1.0) - x;
    let theta = 2.0 * std::f64::consts::PI * x / pw + std::f64::consts::FRAC_PI_2;
    let phi = std::f64::consts::PI * y / ph;
    Vec3::new(
        (theta.cos() * phi.sin()) as f32,
        (theta.sin() * phi.sin()) as f32,
        phi.cos() as f32,
    )
}

/// A vertex's colour without fast blending: scaled by alpha × brightness and never darker than
/// the clear colour unless nearly opaque.
pub fn vertex_color(color: [i32; 4], brightness: i32, background: [i32; 3]) -> [f32; 3] {
    let a = (color[3] * brightness * 255) >> 16;
    std::array::from_fn(|k| {
        let c = (color[k] * a) >> 8;
        let c = if a < 252 { c.max(background[k]) } else { c };
        c.clamp(0, 255) as f32 / 255.0
    })
}

impl Background {
    pub fn from_btg(src: &dyn AssetSource, name: &str, btg: Btg) -> Self {
        let bg = btg.background.map(|c| c.clamp(0, 255));
        let colours: Vec<[f32; 3]> = btg
            .vertices
            .iter()
            .map(|v| vertex_color(v.color, v.brightness, bg))
            .collect();
        let dirs: Vec<Vec3> = btg
            .vertices
            .iter()
            .map(|v| page_to_sphere(btg.page_size, v.position[0], v.position[1]))
            .collect();
        let vertices = btg
            .polygons
            .iter()
            .filter(|p| p.vertices.iter().all(|&i| (i as usize) < dirs.len()))
            .flat_map(|p| {
                p.vertices.map(|i| SkyVertex {
                    direction: dirs[i as usize].into(),
                    color: colours[i as usize],
                })
            })
            .collect();

        let mut star_groups: Vec<StarGroup> = Vec::new();
        for star in &btg.stars {
            let dir = page_to_sphere(btg.page_size, star.position[0], star.position[1]);
            let tint = [0, 1, 2].map(|k| star.color[k].clamp(0, 255) as f32 / 255.0);
            if let Some(g) = star_groups.iter_mut().find(|g| g.texture == star.texture) {
                g.stars.push((dir, tint));
                continue;
            }
            let path = format!("btg\\bitmaps\\{}", star.texture);
            let image = src
                .read(&path)
                .ok_or_else(|| "not found".to_string())
                .and_then(|d| tga::decode(&d));
            match image {
                Ok(image) => star_groups.push(StarGroup {
                    texture: star.texture.clone(),
                    image,
                    stars: vec![(dir, tint)],
                }),
                Err(e) => eprintln!("warning: star sprite {path}: {e}"),
            }
        }
        Self {
            name: name.to_owned(),
            clear_color: bg.map(|c| c as f32 / 255.0),
            btg,
            vertices,
            star_groups,
        }
    }

    pub fn load(src: &dyn AssetSource, path: &str) -> Result<Self> {
        let data = src.read(path).ok_or_else(|| format!("{path}: not found"))?;
        let btg = Btg::parse(&data).map_err(|e| format!("{path}: {e}"))?;
        Ok(Self::from_btg(src, path, btg))
    }

    pub fn star_count(&self) -> usize {
        self.star_groups.iter().map(|g| g.stars.len()).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_corners() {
        let top = page_to_sphere([100, 50], 0.0, 0.0);
        assert!((top - Vec3::Z).length() < 1e-6);
        let mid = page_to_sphere([100, 50], 99.0, 25.0);
        assert!((mid - Vec3::Y).length() < 1e-5);
    }

    #[test]
    fn colour_rules() {
        // Half-transparent vertices never drop below the clear colour.
        assert_eq!(
            vertex_color([255, 0, 0, 128], 255, [10, 20, 30]),
            [126.0 / 255.0, 20.0 / 255.0, 30.0 / 255.0]
        );
        // Opaque vertices may.
        assert_eq!(vertex_color([0, 0, 0, 255], 256, [10, 20, 30]), [0.0; 3]);
    }
}
