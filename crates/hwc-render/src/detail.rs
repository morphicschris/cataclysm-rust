//! Geometry worked out when a ship is uploaded, for the modern ship look:
//!
//! * [`subdivide`]: one level of Phong tessellation (Boubekeur and Alexa, 2008). Each triangle
//!   becomes four; the new point on an edge the hull is shaded smoothly across is pulled out
//!   towards the tangent planes of its two ends, so curved hulls get rounder outlines. Hard
//!   edges and open edges keep their straight midpoint, so nothing cracks or bulges where the
//!   modeller meant a corner.
//! * [`occlusion`]: ambient occlusion baked per vertex by casting rays against the whole
//!   level, so recesses, joins and the undersides of overhangs darken. It costs nothing per
//!   frame.

use std::collections::HashMap;

use glam::Vec3;

use crate::mesh::{ShipVertex, TintVertex};

/// How far the smooth edges' midpoints move towards the curved surface (Phong tessellation's
/// α; 0: flat, 1: the full projection).
const PHONG_ALPHA: f32 = 0.75;

fn key(v: Vec3) -> [i32; 3] {
    (v * 1024.0).round().as_ivec3().to_array()
}

/// One level of Phong tessellation over one object's triangles, kept in their material lists
/// (`lists[m]` holds three corners per triangle). An edge is curved when every triangle on it
/// has the same normals at its two ends (the hull is shaded continuously across it); open
/// edges and edges with a change of normal stay straight.
pub fn subdivide(lists: &mut [Vec<(ShipVertex, TintVertex)>]) {
    let pos = |v: &ShipVertex| Vec3::from_array(v.position);
    let nrm = |v: &ShipVertex| Vec3::from_array(v.normal);
    // Per edge (by welded end points, smaller first): the normals each triangle on it has at
    // the two ends, as (at the smaller end, at the larger end).
    let mut edges: HashMap<([i32; 3], [i32; 3]), Vec<(Vec3, Vec3)>> = HashMap::new();
    for list in lists.iter() {
        for tri in list.chunks_exact(3) {
            for k in 0..3 {
                let (a, b) = (&tri[k].0, &tri[(k + 1) % 3].0);
                let (ka, kb) = (key(pos(a)), key(pos(b)));
                let entry = if ka <= kb {
                    ((ka, kb), (nrm(a), nrm(b)))
                } else {
                    ((kb, ka), (nrm(b), nrm(a)))
                };
                edges.entry(entry.0).or_default().push(entry.1);
            }
        }
    }
    let smooth = |ka: [i32; 3], kb: [i32; 3]| -> bool {
        let k = if ka <= kb { (ka, kb) } else { (kb, ka) };
        let Some(v) = edges.get(&k) else { return false };
        v.len() >= 2
            && v.iter()
                .all(|(na, nb)| na.dot(v[0].0) > 0.999 && nb.dot(v[0].1) > 0.999)
    };
    let mid = |a: &(ShipVertex, TintVertex), b: &(ShipVertex, TintVertex)| {
        let (pa, pb, na, nb) = (pos(&a.0), pos(&b.0), nrm(&a.0), nrm(&b.0));
        let m = (pa + pb) * 0.5;
        let p = if smooth(key(pa), key(pb)) {
            let qa = m - (m - pa).dot(na) * na;
            let qb = m - (m - pb).dot(nb) * nb;
            m.lerp((qa + qb) * 0.5, PHONG_ALPHA)
        } else {
            m
        };
        let n = (na + nb).normalize_or(na);
        let uv = [(a.0.uv[0] + b.0.uv[0]) * 0.5, (a.0.uv[1] + b.0.uv[1]) * 0.5];
        let tint = TintVertex {
            occlusion: (a.1.occlusion + b.1.occlusion) / 2,
            ..a.1
        };
        (
            ShipVertex {
                position: p.into(),
                normal: n.into(),
                uv,
            },
            tint,
        )
    };
    for list in lists.iter_mut() {
        let mut out = Vec::with_capacity(list.len() * 4);
        for tri in list.chunks_exact(3) {
            let (a, b, c) = (tri[0], tri[1], tri[2]);
            let (ab, bc, ca) = (mid(&a, &b), mid(&b, &c), mid(&c, &a));
            out.extend_from_slice(&[a, ab, ca, ab, b, bc, ca, bc, c, ab, bc, ca]);
        }
        *list = out;
    }
}

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

    fn corner(p: [f32; 3], n: [f32; 3]) -> (ShipVertex, TintVertex) {
        (
            ShipVertex {
                position: p,
                normal: n,
                uv: [0.0; 2],
            },
            TintVertex::default(),
        )
    }

    #[test]
    fn smooth_edges_bulge_and_hard_edges_stay_straight() {
        // Two triangles sharing the edge (0,0,0)-(0,1,0), with normals tilted out along the
        // edge as on a curved surface (smooth), and the same with a crease (each face's own
        // normal).
        let tilt = Vec3::new(0.0, 0.0, 1.0);
        let out_a = Vec3::new(-0.3, 0.0, 1.0).normalize();
        let out_b = Vec3::new(0.3, 0.0, 1.0).normalize();
        let smooth = vec![
            corner([0.0, 0.0, 0.0], tilt.into()),
            corner([-1.0, 0.0, 0.0], out_a.into()),
            corner([0.0, 1.0, 0.0], tilt.into()),
            corner([0.0, 0.0, 0.0], tilt.into()),
            corner([0.0, 1.0, 0.0], tilt.into()),
            corner([1.0, 0.0, 0.0], out_b.into()),
        ];
        let mut lists = vec![smooth.clone()];
        subdivide(&mut lists);
        assert_eq!(lists[0].len(), 24);
        // The outer edges are open: their midpoints stay on the line.
        // The first triangle's a-b midpoint, on the open edge (0,0,0)-(-1,0,0).
        let mid = lists[0][1].0.position;
        assert!(mid[2].abs() < 1e-6, "{mid:?}");
        // The shared edge's ends have the same normal on both sides: flat normals there, so it
        // stays straight too. Tilt one end on both sides and it bends.
        let bent = Vec3::new(0.0, -0.4, 1.0).normalize();
        let mut curved = smooth;
        curved[0].0.normal = bent.into();
        curved[3].0.normal = bent.into();
        let mut lists = vec![curved.clone()];
        subdivide(&mut lists);
        let on_edge: Vec<[f32; 3]> = lists[0]
            .iter()
            .map(|c| c.0.position)
            .filter(|p| p[0].abs() < 1e-6 && (p[1] - 0.5).abs() < 0.1)
            .collect();
        assert!(on_edge.iter().all(|p| p[2] > 1e-3), "{on_edge:?}");
        // Give the two sides different normals at one end: a crease, kept straight.
        curved[3].0.normal = tilt.into();
        let mut lists = vec![curved];
        subdivide(&mut lists);
        assert!(lists[0]
            .iter()
            .map(|c| c.0.position)
            .filter(|p| p[0].abs() < 1e-6 && (p[1] - 0.5).abs() < 0.1)
            .all(|p| p[2].abs() < 1e-6));
    }

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
