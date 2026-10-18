//! Triangle rasteriser: near-plane clipping, back-face culling, a reciprocal-depth z-buffer and
//! perspective-correct interpolation of `N` attributes.

/// A vertex after projection: clip-space `[x, y, z, w]` plus attributes to interpolate.
#[derive(Debug, Clone, Copy)]
pub struct ClipVertex<const N: usize> {
    pub clip: [f32; 4],
    pub attrs: [f32; N],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cull {
    /// Draw both faces.
    None,
    /// Skip faces wound clockwise on screen (OpenGL's default front face is counter-clockwise).
    Back,
}

/// A fragment handed to the shading callback.
#[derive(Debug, Clone, Copy)]
pub struct Fragment<const N: usize> {
    pub x: usize,
    pub y: usize,
    pub attrs: [f32; N],
    /// Whether the triangle faces the viewer (counter-clockwise on screen).
    pub front_facing: bool,
}

/// Render target dimensions plus an optional depth buffer holding `1 / w` (0 = empty).
pub struct Target<'a> {
    pub width: usize,
    pub height: usize,
    pub depth: Option<&'a mut [f32]>,
}

/// Closest distance in front of the eye that is drawn.
const NEAR: f32 = 1e-3;

/// Rasterises one triangle. `shade` returns `false` to discard a fragment (no depth write).
pub fn draw_triangle<const N: usize>(
    target: &mut Target<'_>,
    tri: [ClipVertex<N>; 3],
    cull: Cull,
    mut shade: impl FnMut(Fragment<N>) -> bool,
) {
    let poly = clip_near(&tri);
    for i in 1..poly.len().saturating_sub(1) {
        raster([poly[0], poly[i], poly[i + 1]], target, cull, &mut shade);
    }
}

/// Clips a triangle against `w >= NEAR`, returning a convex polygon of up to four vertices.
fn clip_near<const N: usize>(tri: &[ClipVertex<N>; 3]) -> Vec<ClipVertex<N>> {
    if tri.iter().all(|v| v.clip[3] >= NEAR) {
        return tri.to_vec();
    }
    let mut out = Vec::with_capacity(4);
    for i in 0..3 {
        let (a, b) = (tri[i], tri[(i + 1) % 3]);
        let (ina, inb) = (a.clip[3] >= NEAR, b.clip[3] >= NEAR);
        if ina {
            out.push(a);
        }
        if ina != inb {
            let t = (NEAR - a.clip[3]) / (b.clip[3] - a.clip[3]);
            out.push(ClipVertex {
                clip: std::array::from_fn(|k| a.clip[k] + (b.clip[k] - a.clip[k]) * t),
                attrs: std::array::from_fn(|k| a.attrs[k] + (b.attrs[k] - a.attrs[k]) * t),
            });
        }
    }
    out
}

fn raster<const N: usize>(
    tri: [ClipVertex<N>; 3],
    target: &mut Target<'_>,
    cull: Cull,
    shade: &mut impl FnMut(Fragment<N>) -> bool,
) {
    let (w, h) = (target.width as f32, target.height as f32);
    // Screen positions (y down), reciprocal w, and attributes pre-divided by w.
    let screen = tri.map(|v| {
        let inv_w = 1.0 / v.clip[3];
        let sx = (v.clip[0] * inv_w * 0.5 + 0.5) * w;
        let sy = (0.5 - v.clip[1] * inv_w * 0.5) * h;
        (sx, sy, inv_w, v.attrs.map(|a| a * inv_w))
    });
    let [(x0, y0, ..), (x1, y1, ..), (x2, y2, ..)] = screen;
    let area = (x1 - x0) * (y2 - y0) - (y1 - y0) * (x2 - x0);
    if area == 0.0 || !area.is_finite() {
        return;
    }
    // With y pointing down, a counter-clockwise (GL front) triangle has negative area.
    let front_facing = area < 0.0;
    if cull == Cull::Back && !front_facing {
        return;
    }

    let min_x = x0.min(x1).min(x2).floor().max(0.0) as usize;
    let max_x = (x0.max(x1).max(x2).ceil().min(w) as usize).min(target.width);
    let min_y = y0.min(y1).min(y2).floor().max(0.0) as usize;
    let max_y = (y0.max(y1).max(y2).ceil().min(h) as usize).min(target.height);
    let inv_area = 1.0 / area;
    let edge = |ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32| {
        (bx - ax) * (py - ay) - (by - ay) * (px - ax)
    };

    for py in min_y..max_y {
        let fy = py as f32 + 0.5;
        for px in min_x..max_x {
            let fx = px as f32 + 0.5;
            let b0 = edge(x1, y1, x2, y2, fx, fy) * inv_area;
            let b1 = edge(x2, y2, x0, y0, fx, fy) * inv_area;
            let b2 = edge(x0, y0, x1, y1, fx, fy) * inv_area;
            if b0 < 0.0 || b1 < 0.0 || b2 < 0.0 {
                continue;
            }
            let inv_w = b0 * screen[0].2 + b1 * screen[1].2 + b2 * screen[2].2;
            let idx = py * target.width + px;
            if let Some(depth) = target.depth.as_deref() {
                if inv_w <= depth[idx] {
                    continue;
                }
            }
            let attrs = std::array::from_fn(|k| {
                (b0 * screen[0].3[k] + b1 * screen[1].3[k] + b2 * screen[2].3[k]) / inv_w
            });
            let kept = shade(Fragment {
                x: px,
                y: py,
                attrs,
                front_facing,
            });
            if kept {
                if let Some(depth) = target.depth.as_deref_mut() {
                    depth[idx] = inv_w;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, w: f32) -> ClipVertex<1> {
        ClipVertex {
            clip: [x, y, w, w],
            attrs: [w],
        }
    }

    #[test]
    fn culls_clockwise_and_fills_counter_clockwise() {
        let mut count = 0;
        let mut target = Target {
            width: 8,
            height: 8,
            depth: None,
        };
        let ccw = [v(-1.0, -1.0, 1.0), v(1.0, -1.0, 1.0), v(-1.0, 1.0, 1.0)];
        draw_triangle(&mut target, ccw, Cull::Back, |_| {
            count += 1;
            true
        });
        assert!(count > 20);
        let cw = [ccw[0], ccw[2], ccw[1]];
        draw_triangle(&mut target, cw, Cull::Back, |_| panic!("should be culled"));
    }

    #[test]
    fn perspective_correct_attribute() {
        // Attribute equals w, so the interpolated value must equal 1 / interpolated (1/w).
        let mut target = Target {
            width: 16,
            height: 16,
            depth: None,
        };
        let tri = [v(-1.0, -1.0, 1.0), v(4.0, -4.0, 4.0), v(-1.0, 1.0, 1.0)];
        draw_triangle(&mut target, tri, Cull::None, |f| {
            assert!(f.attrs[0] >= 1.0 - 1e-3 && f.attrs[0] <= 4.0 + 1e-3);
            true
        });
    }
}
