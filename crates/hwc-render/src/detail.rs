//! Geometry worked out when a ship is uploaded, for the modern ship look: ambient occlusion
//! baked per vertex by casting rays against the whole level ([`occlusion`]), so recesses,
//! joins and the undersides of overhangs darken. It costs nothing per frame.

use glam::Vec3;

/// A bounding volume hierarchy over triangles, for the occlusion rays.
struct Bvh {
    tris: Vec<[Vec3; 3]>,
    nodes: Vec<Node>,
}

struct Node {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle and count; inner: left child index (right is `left + 1`), count 0.
    first: u32,
    count: u32,
}

impl Bvh {
    fn new(mut tris: Vec<[Vec3; 3]>) -> Self {
        let mut nodes = Vec::with_capacity(tris.len() * 2 / 3 + 1);
        nodes.push(Node {
            min: Vec3::ZERO,
            max: Vec3::ZERO,
            first: 0,
            count: tris.len() as u32,
        });
        let mut stack = vec![0usize];
        while let Some(i) = stack.pop() {
            let (first, count) = (nodes[i].first as usize, nodes[i].count as usize);
            let slice = &mut tris[first..first + count];
            let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for t in slice.iter() {
                for p in t {
                    min = min.min(*p);
                    max = max.max(*p);
                }
            }
            nodes[i].min = min;
            nodes[i].max = max;
            if count <= 4 {
                continue;
            }
            let axis = (max - min).max_position();
            let centre = |t: &[Vec3; 3]| (t[0] + t[1] + t[2])[axis];
            slice.sort_unstable_by(|a, b| centre(a).total_cmp(&centre(b)));
            let half = count / 2;
            let left = nodes.len();
            nodes.push(Node {
                min,
                max,
                first: first as u32,
                count: half as u32,
            });
            nodes.push(Node {
                min,
                max,
                first: (first + half) as u32,
                count: (count - half) as u32,
            });
            nodes[i].first = left as u32;
            nodes[i].count = 0;
            stack.push(left);
            stack.push(left + 1);
        }
        Self { tris, nodes }
    }

    /// Distance to the nearest hit along `dir` from `origin` within `max`, either side.
    fn hit(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<f32> {
        let inv = dir.recip();
        let mut best = max;
        let mut found = false;
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let n = &self.nodes[stack[top] as usize];
            let t0 = (n.min - origin) * inv;
            let t1 = (n.max - origin) * inv;
            let near = t0.min(t1).max_element();
            let far = t0.max(t1).min_element();
            if far < near.max(0.0) || near > best {
                continue;
            }
            if n.count == 0 {
                if top + 2 <= stack.len() {
                    stack[top] = n.first;
                    stack[top + 1] = n.first + 1;
                    top += 2;
                }
                continue;
            }
            for t in &self.tris[n.first as usize..(n.first + n.count) as usize] {
                if let Some(d) = ray_triangle(origin, dir, t) {
                    if d < best {
                        best = d;
                        found = true;
                    }
                }
            }
        }
        found.then_some(best)
    }
}

/// Möller-Trumbore, both faces.
fn ray_triangle(o: Vec3, d: Vec3, t: &[Vec3; 3]) -> Option<f32> {
    let e1 = t[1] - t[0];
    let e2 = t[2] - t[0];
    let p = d.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return None;
    }
    let inv = 1.0 / det;
    let s = o - t[0];
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = d.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let dist = e2.dot(q) * inv;
    (dist > 0.0).then_some(dist)
}

/// Rays cast per point.
const RAYS: usize = 24;

/// Ambient occlusion at each of `points` (position and normal, in the same space as
/// `triangles`), from 0 (open) to 1 (closed in), counting what lies within `reach`: the share
/// of cosine-weighted rays over the normal's hemisphere that hit, nearer hits counting more.
pub fn occlusion(triangles: Vec<[Vec3; 3]>, points: &[(Vec3, Vec3)], reach: f32) -> Vec<f32> {
    if triangles.is_empty() || points.is_empty() || reach <= 0.0 {
        return vec![0.0; points.len()];
    }
    let bvh = Bvh::new(triangles);
    // A fixed spread of directions about +Z (Hammersley points, cosine-weighted).
    let dirs: Vec<Vec3> = (0..RAYS)
        .map(|i| {
            let u = (i as f32 + 0.5) / RAYS as f32;
            let v = (i as u32).reverse_bits() as f32 / 4_294_967_296.0;
            let r = u.sqrt();
            let phi = std::f32::consts::TAU * v;
            Vec3::new(r * phi.cos(), r * phi.sin(), (1.0 - u).max(0.0).sqrt())
        })
        .collect();
    let lift = reach * 0.01;
    let one = |&(p, n): &(Vec3, Vec3)| -> f32 {
        let n = n.normalize_or(Vec3::Z);
        let (t, b) = n.any_orthonormal_pair();
        let origin = p + n * lift;
        let mut sum = 0.0;
        for d in &dirs {
            let dir = t * d.x + b * d.y + n * d.z;
            if let Some(h) = bvh.hit(origin, dir, reach) {
                sum += 1.0 - h / reach;
            }
        }
        sum / RAYS as f32
    };
    let threads = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(8);
    let chunk = points.len().div_ceil(threads).max(64);
    let mut out = vec![0.0; points.len()];
    std::thread::scope(|s| {
        for (dst, src) in out.chunks_mut(chunk).zip(points.chunks(chunk)) {
            let one = &one;
            s.spawn(move || {
                for (d, p) in dst.iter_mut().zip(src) {
                    *d = one(p);
                }
            });
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occlusion_darkens_inside_a_corner() {
        // A floor and a wall meeting at x = 0.
        let floor = [
            [
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, -1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
            ],
            [
                Vec3::new(-1.0, -1.0, 0.0),
                Vec3::new(1.0, 1.0, 0.0),
                Vec3::new(-1.0, 1.0, 0.0),
            ],
        ];
        let wall = [
            [
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 0.0),
                Vec3::new(0.0, 1.0, 2.0),
            ],
            [
                Vec3::new(0.0, -1.0, 0.0),
                Vec3::new(0.0, 1.0, 2.0),
                Vec3::new(0.0, -1.0, 2.0),
            ],
        ];
        let tris: Vec<[Vec3; 3]> = floor.into_iter().chain(wall).collect();
        let ao = occlusion(
            tris,
            &[
                (Vec3::new(0.95, 0.0, 0.0), Vec3::Z),
                (Vec3::new(0.05, 0.0, 0.0), Vec3::Z),
            ],
            1.0,
        );
        assert!(ao[0] < 0.05, "{ao:?}");
        assert!(ao[1] > 0.2, "{ao:?}");
    }
}
