//! The modern ship look, a set of enhancements over the original's fixed-function hulls, all
//! worked out at load and draw time from the game's own meshes and textures (nothing is
//! stored or shipped):
//!
//! * mipmapped, anisotropically filtered hull textures, so a ship at a distance does not
//!   shimmer;
//! * lighting per pixel instead of per vertex, with the same ambient and diffuse terms plus a
//!   soft highlight (the original sets no material specular);
//! * a bump taken from each texture's brightness, so panel lines and greebles catch the light;
//! * smoothed facets: flat-shaded faces meeting at a shallow angle share a normal, so curved
//!   hulls look curved while hard edges stay hard.
//!
//! [`ShipLook::original`] turns everything off and draws exactly as before. Textures and
//! normals are made when a ship is uploaded, so a change of look takes effect on ships
//! uploaded after it; the per-pixel terms change at once.

/// Which enhancements the ship pipeline uses.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShipLook {
    /// Mipmaps and anisotropic filtering on hull textures.
    pub filtering: bool,
    /// Lighting evaluated per pixel.
    pub per_pixel: bool,
    /// Strength of the highlight (0: none). Only with `per_pixel`.
    pub highlight: f32,
    /// Depth of the bump from texture brightness, in texels (0: none). Only with `per_pixel`.
    pub bump: f32,
    /// Flat faces meeting at less than this many degrees share a smoothed normal (0: off).
    pub crease_degrees: f32,
}

impl ShipLook {
    /// The original's hulls.
    pub fn original() -> Self {
        Self {
            filtering: false,
            per_pixel: false,
            highlight: 0.0,
            bump: 0.0,
            crease_degrees: 0.0,
        }
    }

    /// Every enhancement at its default strength.
    pub fn modern() -> Self {
        Self {
            filtering: true,
            per_pixel: true,
            highlight: 0.35,
            bump: 1.5,
            crease_degrees: 40.0,
        }
    }

    pub fn is_original(&self) -> bool {
        *self == Self::original()
    }

    /// The `look` row of the frame uniform: x per-pixel, y highlight, z bump.
    pub(crate) fn uniform(&self) -> [f32; 4] {
        if self.per_pixel {
            [1.0, self.highlight.max(0.0), self.bump.max(0.0), 0.0]
        } else {
            [0.0; 4]
        }
    }
}

impl Default for ShipLook {
    fn default() -> Self {
        Self::original()
    }
}

/// The mip chain of an RGBA8 image below level 0, each level a 2x2 box filter of the one above
/// (odd edges fold in the last row or column), down to 1x1.
pub fn mip_chain(width: u32, height: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = Vec::new();
    let (mut w, mut h) = (width.max(1), height.max(1));
    let mut cur = rgba.to_vec();
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let xs = [(2 * x).min(w - 1), (2 * x + 1).min(w - 1)];
                let ys = [(2 * y).min(h - 1), (2 * y + 1).min(h - 1)];
                for c in 0..4 {
                    let mut sum = 0u32;
                    for &sy in &ys {
                        for &sx in &xs {
                            sum += u32::from(cur[((sy * w + sx) * 4 + c) as usize]);
                        }
                    }
                    next[((y * nw + x) * 4 + c) as usize] = ((sum + 2) / 4) as u8;
                }
            }
        }
        out.push((nw, nh, next.clone()));
        cur = next;
        (w, h) = (nw, nh);
    }
    out
}

/// Gives flat-shaded triangles smoothed normals: each corner takes the angle-weighted sum of
/// the face normals of the triangles around its position whose faces are within
/// `crease_degrees` of its own. `triangles` holds three positions per triangle; returns three
/// normals per triangle.
pub fn smoothed_normals(triangles: &[[f32; 3]], crease_degrees: f32) -> Vec<[f32; 3]> {
    use glam::Vec3;
    use std::collections::HashMap;
    let tri_count = triangles.len() / 3;
    let p = |i: usize| Vec3::from_array(triangles[i]);
    let faces: Vec<Vec3> = (0..tri_count)
        .map(|t| {
            (p(3 * t + 1) - p(3 * t))
                .cross(p(3 * t + 2) - p(3 * t))
                .normalize_or_zero()
        })
        .collect();
    // Corner angles, for weighting.
    let angle = |i: usize| {
        let (t, k) = (i / 3, i % 3);
        let (a, b, c) = (p(i), p(3 * t + (k + 1) % 3), p(3 * t + (k + 2) % 3));
        (b - a)
            .normalize_or_zero()
            .dot((c - a).normalize_or_zero())
            .clamp(-1.0, 1.0)
            .acos()
    };
    // Corners grouped by position (welded to a small grid so seams between copies meet).
    let key = |v: Vec3| (v * 1024.0).round().as_ivec3().to_array();
    let mut groups: HashMap<[i32; 3], Vec<usize>> = HashMap::new();
    for i in 0..tri_count * 3 {
        groups.entry(key(p(i))).or_default().push(i);
    }
    let cos_crease = crease_degrees.to_radians().cos();
    (0..tri_count * 3)
        .map(|i| {
            let own = faces[i / 3];
            let mut sum = Vec3::ZERO;
            for &j in &groups[&key(p(i))] {
                let f = faces[j / 3];
                if f.dot(own) >= cos_crease {
                    sum += f * angle(j);
                }
            }
            let n = sum.normalize_or_zero();
            if n == Vec3::ZERO { own } else { n }.to_array()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mips_halve_down_to_one_pixel() {
        let rgba: Vec<u8> = (0..6 * 3 * 4).map(|i| (i * 7 % 256) as u8).collect();
        let chain = mip_chain(6, 3, &rgba);
        let sizes: Vec<(u32, u32)> = chain.iter().map(|(w, h, _)| (*w, *h)).collect();
        assert_eq!(sizes, [(3, 1), (1, 1)]);
        assert!(chain
            .iter()
            .all(|(w, h, p)| p.len() == (w * h * 4) as usize));
        let flat = mip_chain(4, 4, &[200; 64]);
        assert!(flat.iter().all(|(_, _, p)| p.iter().all(|&v| v == 200)));
    }

    #[test]
    fn shallow_folds_smooth_and_sharp_edges_stay() {
        // Two triangles folded 20° along the shared edge (0,0,0)-(0,1,0), and the same at 90°.
        let fold = |deg: f32| {
            let r = deg.to_radians();
            vec![
                [0.0, 0.0, 0.0],
                [-1.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0],
                [r.cos(), 0.0, r.sin()],
            ]
        };
        let n = smoothed_normals(&fold(20.0), 40.0);
        // The shared corners agree; the outer ones keep their faces.
        assert!((glam::Vec3::from(n[0]) - glam::Vec3::from(n[3])).length() < 1e-5);
        assert!((glam::Vec3::from(n[1]) - glam::Vec3::Z * -1.0).length() < 1e-5);
        let sharp = smoothed_normals(&fold(90.0), 40.0);
        assert!((glam::Vec3::from(sharp[0]) - glam::Vec3::from(sharp[3])).length() > 0.5);
    }
}
