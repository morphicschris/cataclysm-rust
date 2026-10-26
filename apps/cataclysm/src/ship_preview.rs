//! The build manager's ship view for the modern build manager preview
//! (`hwc_ui::ingame::modern_build_manager`): the selected ship type drawn turning slowly, in
//! the player's colours, into an image the interface shows in its preview box. A small
//! software renderer (perspective-correct textures, a depth buffer, two lights, 2x2
//! supersampling) over the ship's most detailed mesh; the original renders its ship view
//! with the 3D engine. Only used while the modern build manager is up.

use std::collections::BTreeMap;
use std::rc::Rc;

use glam::{Mat3, Vec3};
use hwc_formats::geo::MATERIAL_SMOOTHING;
use hwc_render::assets::{ShipDef, ShipModel};
use hwc_sim::statics::ShipType;
use hwc_ui::canvas::Image;

use crate::data::Assets;

/// Degrees a second the ship turns.
const TURN_RATE: f32 = 16.0;
/// Pictures a second while turning (the view is redrawn only when the angle steps).
const STEPS_PER_SECOND: f32 = 30.0;
const PITCH: f32 = 16.0;
const FOV_Y: f32 = 28.0;
/// Fraction of the box the ship's bounding sphere spans.
const FILL: f32 = 0.92;
const SUPERSAMPLE: i32 = 2;

struct Texture {
    w: usize,
    h: usize,
    rgba: Vec<[u8; 4]>,
}

impl Texture {
    fn sample(&self, u: f32, v: f32) -> [f32; 4] {
        let x = (u.rem_euclid(1.0) * self.w as f32) as usize % self.w;
        let y = (v.rem_euclid(1.0) * self.h as f32) as usize % self.h;
        let p = self.rgba[y * self.w + x];
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    }
}

struct Tri {
    p: [Vec3; 3],
    n: [Vec3; 3],
    uv: [[f32; 2]; 3],
    texture: Option<usize>,
    colour: [f32; 3],
}

/// A ship ready to draw: its triangles in a frame with the ship's up as +Y, nose to -Z.
struct Prepared {
    tris: Vec<Tri>,
    textures: Vec<Texture>,
    centre: Vec3,
    radius: f32,
}

/// Mesh axes (+X up, +Y right, +Z forward) to the view's (+Y up, +X right, -Z forward).
fn mesh_to_view() -> Mat3 {
    Mat3::from_cols(
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, 0.0, -1.0),
    )
}

fn prepare(model: &ShipModel, team: ([u8; 3], [u8; 3])) -> Prepared {
    let m = mesh_to_view();
    let mut tris = Vec::new();
    let mut textures = Vec::new();
    for part in &model.parts {
        let mut slot = Vec::new();
        for t in &part.textures {
            match t {
                Some(lif) => {
                    let rgba = lif.to_rgba_team(team.0, team.1);
                    slot.push(Some(textures.len()));
                    textures.push(Texture {
                        w: lif.width.max(1) as usize,
                        h: lif.height.max(1) as usize,
                        rgba: rgba.as_chunks::<4>().0.to_vec(),
                    });
                }
                None => slot.push(None),
            }
        }
        for (object, world) in part.geo.objects.iter().zip(&part.object_matrices) {
            let rot = Mat3::from_mat4(*world);
            for poly in &object.polygons {
                let Some(material) = part.geo.materials.get(poly.material as usize) else {
                    continue;
                };
                let smooth = material.flags & MATERIAL_SMOOTHING != 0;
                let face = object
                    .normals
                    .get(poly.face_normal as usize)
                    .copied()
                    .unwrap_or([0.0, 0.0, 1.0]);
                let mut ok = true;
                let corner = |i: usize, ok: &mut bool| {
                    let Some(v) = object.vertices.get(poly.vertices[i] as usize) else {
                        *ok = false;
                        return (Vec3::ZERO, Vec3::Y);
                    };
                    let p = m * world.transform_point3(Vec3::from_array(v.position));
                    let n = if smooth {
                        object
                            .normals
                            .get(v.normal as usize)
                            .copied()
                            .unwrap_or(face)
                    } else {
                        face
                    };
                    (p, (m * (rot * Vec3::from_array(n))).normalize_or_zero())
                };
                let c = [corner(0, &mut ok), corner(1, &mut ok), corner(2, &mut ok)];
                if !ok {
                    continue;
                }
                let d = material.diffuse;
                tris.push(Tri {
                    p: [c[0].0, c[1].0, c[2].0],
                    n: [c[0].1, c[1].1, c[2].1],
                    uv: poly.uvs,
                    texture: slot.get(poly.material as usize).copied().flatten(),
                    colour: [d[0] as f32, d[1] as f32, d[2] as f32],
                });
            }
        }
    }
    let (c, r) = model.bounds();
    let centre = m * c;
    Prepared {
        tris,
        textures,
        centre,
        radius: r.max(1.0),
    }
}

/// Draws `ship` turned `yaw` degrees into a `w` x `h` image (transparent around it).
fn render(ship: &Prepared, w: i32, h: i32, yaw: f32) -> Image {
    let (sw, sh) = ((w * SUPERSAMPLE).max(1), (h * SUPERSAMPLE).max(1));
    let n = (sw * sh) as usize;
    let mut depth = vec![f32::MAX; n];
    let mut colour = vec![[0.0f32; 4]; n];
    let turn = Mat3::from_rotation_x(PITCH.to_radians()) * Mat3::from_rotation_y(yaw.to_radians());
    let half = (FOV_Y.to_radians() / 2.0).tan();
    let aspect = sw as f32 / sh as f32;
    // Far enough that the sphere fits the narrower side.
    let fit = if aspect < 1.0 { half * aspect } else { half };
    let dist = ship.radius / (fit * FILL);
    let key = Vec3::new(-0.45, 0.75, 0.5).normalize();
    let fill = Vec3::new(0.7, -0.1, 0.4).normalize();
    let rim = Vec3::new(0.2, 0.3, -1.0).normalize();
    let project = |p: Vec3| -> (f32, f32, f32) {
        let v = turn * (p - ship.centre);
        let z = dist - v.z;
        let inv = 1.0 / z.max(1e-3);
        let x = (v.x * inv / half / aspect * 0.5 + 0.5) * sw as f32;
        let y = (0.5 - v.y * inv / half * 0.5) * sh as f32;
        (x, y, inv)
    };
    for t in &ship.tris {
        let s = t.p.map(project);
        let nrm = t.n.map(|n| turn * n);
        let area = (s[1].0 - s[0].0) * (s[2].1 - s[0].1) - (s[2].0 - s[0].0) * (s[1].1 - s[0].1);
        if area.abs() < 1e-6 {
            continue;
        }
        let x0 = s
            .iter()
            .map(|p| p.0)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as i32;
        let x1 = s
            .iter()
            .map(|p| p.0)
            .fold(f32::MIN, f32::max)
            .ceil()
            .min(sw as f32) as i32;
        let y0 = s
            .iter()
            .map(|p| p.1)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as i32;
        let y1 = s
            .iter()
            .map(|p| p.1)
            .fold(f32::MIN, f32::max)
            .ceil()
            .min(sh as f32) as i32;
        let tex = t.texture.and_then(|i| ship.textures.get(i));
        for y in y0..y1 {
            for x in x0..x1 {
                let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                let e = |a: usize, b: usize| {
                    (s[b].0 - s[a].0) * (py - s[a].1) - (s[b].1 - s[a].1) * (px - s[a].0)
                };
                let (w0, w1, w2) = (e(1, 2) / area, e(2, 0) / area, e(0, 1) / area);
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                // 1/z interpolates linearly on screen; depth is its reciprocal.
                let inv = w0 * s[0].2 + w1 * s[1].2 + w2 * s[2].2;
                let z = 1.0 / inv;
                let i = (y * sw + x) as usize;
                if z >= depth[i] {
                    continue;
                }
                let (a, b, c) = (w0 * s[0].2 / inv, w1 * s[1].2 / inv, w2 * s[2].2 / inv);
                let mut base = match tex {
                    Some(tx) => {
                        let u = a * t.uv[0][0] + b * t.uv[1][0] + c * t.uv[2][0];
                        let v = a * t.uv[0][1] + b * t.uv[1][1] + c * t.uv[2][1];
                        let p = tx.sample(u, v);
                        if p[3] < 64.0 {
                            continue;
                        }
                        [p[0], p[1], p[2]]
                    }
                    None => t.colour,
                };
                depth[i] = z;
                let mut nn = (nrm[0] * a + nrm[1] * b + nrm[2] * c).normalize_or_zero();
                // Two-sided: the side facing the viewer.
                if nn.z < 0.0 {
                    nn = -nn;
                }
                let light = 0.30
                    + 0.95 * nn.dot(key).max(0.0)
                    + 0.30 * nn.dot(fill).max(0.0)
                    + 0.35 * nn.dot(rim).max(0.0);
                let spec = nn.dot((key + Vec3::Z).normalize()).max(0.0).powi(24) * 70.0;
                for (k, ch) in base.iter_mut().enumerate() {
                    // A touch of warm rim light, matching the interface's gold.
                    let warm = [1.0, 0.85, 0.6][k];
                    *ch = (*ch * light + spec * warm).min(255.0);
                }
                colour[i] = [base[0], base[1], base[2], 255.0];
            }
        }
    }
    // 2x2 down: average the samples (colour weighted by coverage).
    let mut img = Image::new(w, h, [0, 0, 0, 0]);
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f32; 4];
            for dy in 0..SUPERSAMPLE {
                for dx in 0..SUPERSAMPLE {
                    let s = colour[((y * SUPERSAMPLE + dy) * sw + x * SUPERSAMPLE + dx) as usize];
                    let a = s[3] / 255.0;
                    acc[0] += s[0] * a;
                    acc[1] += s[1] * a;
                    acc[2] += s[2] * a;
                    acc[3] += s[3];
                }
            }
            let k = (SUPERSAMPLE * SUPERSAMPLE) as f32;
            let a = acc[3] / k;
            if a > 0.0 {
                let cov = a / 255.0;
                img.pixels[(y * w + x) as usize] = [
                    (acc[0] / k / cov).min(255.0) as u8,
                    (acc[1] / k / cov).min(255.0) as u8,
                    (acc[2] / k / cov).min(255.0) as u8,
                    a.round() as u8,
                ];
            }
        }
    }
    img
}

/// The ship view's state: models loaded so far and the last picture's key.
#[derive(Default)]
pub struct ShipPreview {
    defs: Option<Vec<ShipDef>>,
    ships: BTreeMap<(u32, [u8; 6]), Option<Rc<Prepared>>>,
    last: Option<(u32, i32, i32, u32)>,
    /// When the shown type changed (the turn starts from the same angle for each ship).
    since: Option<(u32, f32)>,
}

impl ShipPreview {
    fn ship(&mut self, src: &Assets, t: u32, team: ([u8; 3], [u8; 3])) -> Option<Rc<Prepared>> {
        let key = (
            t,
            [
                team.0[0], team.0[1], team.0[2], team.1[0], team.1[1], team.1[2],
            ],
        );
        if let Some(p) = self.ships.get(&key) {
            return p.clone();
        }
        let st = ShipType::from_index(t as usize)?;
        let defs = self.defs.get_or_insert_with(|| ShipDef::list_all(src));
        let (race, name) = (
            st.race().name().to_ascii_lowercase(),
            st.name().to_ascii_lowercase(),
        );
        let p = defs
            .iter()
            .find(|d| d.race == race && d.name == name)
            .and_then(|d| ShipModel::load(src, d).ok())
            .map(|m| Rc::new(prepare(&m, team)));
        self.ships.insert(key, p.clone());
        p
    }

    /// The picture for ship type `t` in a `w` x `h` box at real time `now`, when it differs
    /// from the last one handed out (None: keep the last).
    pub fn update(
        &mut self,
        src: &Assets,
        team: ([u8; 3], [u8; 3]),
        t: u32,
        (w, h): (i32, i32),
        now: f32,
    ) -> Option<Option<Image>> {
        let start = match self.since {
            Some((st, at)) if st == t => at,
            _ => {
                self.since = Some((t, now));
                now
            }
        };
        let step = ((now - start).max(0.0) * STEPS_PER_SECOND) as u32;
        let key = (t, w, h, step);
        if self.last == Some(key) {
            return None;
        }
        self.last = Some(key);
        let Some(ship) = self.ship(src, t, team) else {
            return Some(None);
        };
        if w < 4 || h < 4 {
            return Some(None);
        }
        let yaw = 205.0 + step as f32 / STEPS_PER_SECOND * TURN_RATE;
        Some(Some(render(&ship, w, h, yaw)))
    }

    /// The view closed: the next opening starts afresh.
    pub fn clear(&mut self) {
        self.last = None;
        self.since = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube() -> Prepared {
        let mut tris = Vec::new();
        let v = |x: f32, y: f32, z: f32| Vec3::new(x, y, z);
        let faces = [
            (
                [
                    v(-1., -1., 1.),
                    v(1., -1., 1.),
                    v(1., 1., 1.),
                    v(-1., 1., 1.),
                ],
                Vec3::Z,
            ),
            (
                [
                    v(-1., -1., -1.),
                    v(-1., 1., -1.),
                    v(1., 1., -1.),
                    v(1., -1., -1.),
                ],
                -Vec3::Z,
            ),
            (
                [
                    v(-1., 1., -1.),
                    v(-1., 1., 1.),
                    v(1., 1., 1.),
                    v(1., 1., -1.),
                ],
                Vec3::Y,
            ),
        ];
        for (q, n) in faces {
            for idx in [[0, 1, 2], [0, 2, 3]] {
                tris.push(Tri {
                    p: idx.map(|i| q[i]),
                    n: [n; 3],
                    uv: [[0.0; 2]; 3],
                    texture: None,
                    colour: [200.0, 180.0, 160.0],
                });
            }
        }
        Prepared {
            tris,
            textures: Vec::new(),
            centre: Vec3::ZERO,
            radius: 3f32.sqrt(),
        }
    }

    #[test]
    fn renders_inside_the_box_with_a_clear_border() {
        let img = render(&cube(), 80, 60, 30.0);
        assert_eq!((img.width, img.height), (80, 60));
        assert!(img.get(40, 30)[3] == 255, "the middle is covered");
        assert_eq!(img.get(0, 0)[3], 0, "the corners stay clear");
        assert_eq!(img.get(79, 59)[3], 0);
        // Lit: not black.
        let p = img.get(40, 30);
        assert!(p[0] > 60, "{p:?}");
        // Edges are smoothed (some partly covered pixels).
        assert!(img.pixels.iter().any(|p| p[3] > 0 && p[3] < 255));
    }

    #[test]
    fn mesh_axes_put_the_ships_up_on_screen_up() {
        let m = mesh_to_view();
        assert_eq!(m * Vec3::X, Vec3::Y);
        assert_eq!(m * Vec3::Z, -Vec3::Z);
        assert!((m.determinant() - 1.0).abs() < 1e-6, "no mirror");
    }
}
