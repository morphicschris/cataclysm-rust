//! Software rendering of `.geo` ship meshes with their `.lif` textures.

use hwc_formats::big::BigArchive;
use hwc_formats::geo::{self, Geo};
use hwc_formats::lif::Lif;

use super::canvas::{rgba8, Canvas, Rgba, Texture};
use super::math::{orbit_direction, Camera, Mat4, Vec3};
use super::raster::{draw_triangle, ClipVertex, Cull, Target};

/// Default team colours (player 0 in `teamcolors.script`): base, stripe.
pub const DEFAULT_TEAM: [[u8; 3]; 2] = [[66, 108, 202], [255, 255, 255]];

/// Mesh axes as the engine uses them (`SpaceObj.h`): a ship's coordinate system has its up
/// vector in column 0, right in column 1 and heading in column 2, so meshes fly along +Z with +X
/// as their up direction.
const MESH_FRONT: Vec3 = Vec3::new(0.0, 0.0, 1.0);
const MESH_UP: Vec3 = Vec3::new(1.0, 0.0, 0.0);

const FOV_Y: f32 = 30.0;

/// One mesh ready to draw: parsed geometry, per-object world matrices and per-material textures.
pub struct MeshPart {
    pub path: String,
    pub geo: Geo,
    world: Vec<Mat4>,
    textures: Vec<Option<Texture>>,
}

/// One or more meshes sharing a frame of reference: a ship, or a mothership spine plus module
/// meshes drawn around it.
pub struct ShipModel {
    pub parts: Vec<MeshPart>,
}

/// How the ship is viewed.
#[derive(Debug, Clone, Copy)]
pub struct ShipView {
    /// Degrees around the ship's up axis; 0 looks at the nose.
    pub yaw: f32,
    /// Degrees above the ship's horizontal plane.
    pub pitch: f32,
    /// Fraction of the frame the ship's silhouette fills along its larger screen dimension.
    pub fill: f32,
    /// Shifts the ship on screen, in fractions of the image size (x right, y down).
    pub offset: [f32; 2],
}

impl Default for ShipView {
    fn default() -> Self {
        Self {
            yaw: 35.0,
            pitch: 20.0,
            fill: 0.9,
            offset: [0.0, 0.0],
        }
    }
}

/// Per-render state shared by every part.
struct Scene {
    camera: Camera,
    /// Normalised-device-coordinate offset applied after projection.
    shift: [f32; 2],
    /// Directions towards the key and fill lights.
    key: Vec3,
    fill: Vec3,
}

impl MeshPart {
    /// Loads a mesh and the textures its materials name (relative to the mesh's directory, as
    /// `meshTextureNameToPath` builds them).
    pub fn load(
        big: &BigArchive,
        mesh_path: &str,
        team: [[u8; 3]; 2],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let data = big
            .read_file(mesh_path)?
            .ok_or_else(|| format!("{mesh_path}: not in archive"))?;
        let geo = Geo::parse(&data).map_err(|e| format!("{mesh_path}: {e}"))?;
        let dir = mesh_path
            .rfind(['\\', '/'])
            .map_or("", |i| &mesh_path[..=i]);

        let textures = geo
            .materials
            .iter()
            .map(|m| {
                let name = m.texture.as_deref()?;
                let path = format!("{dir}{}.lif", name.trim_end_matches(".lif"));
                match load_lif(big, &path) {
                    Ok(lif) => Some(Texture::new(
                        lif.width as usize,
                        lif.height as usize,
                        lif.to_rgba_team(team[0], team[1]),
                    )),
                    Err(e) => {
                        eprintln!("warning: texture {path}: {e}");
                        None
                    }
                }
            })
            .collect();

        let world = world_matrices(&geo);
        Ok(Self {
            path: mesh_path.to_string(),
            geo,
            world,
            textures,
        })
    }

    /// Every vertex in world space.
    fn world_points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.geo.objects.iter().zip(&self.world).flat_map(|(o, m)| {
            o.vertices
                .iter()
                .map(move |v| m.transform_point(Vec3::from_array(v.position)))
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.geo.objects.iter().map(|o| o.polygons.len()).sum()
    }

    fn draw(&self, canvas: &mut Canvas, target: &mut Target<'_>, scene: &Scene) {
        for (object, world) in self.geo.objects.iter().zip(&self.world) {
            for poly in &object.polygons {
                let material = &self.geo.materials[poly.material as usize];
                let smooth = material.flags & geo::MATERIAL_SMOOTHING != 0;
                let face_normal = object.normals[poly.face_normal as usize];
                let tri = std::array::from_fn(|i| {
                    let v = object.vertices[poly.vertices[i] as usize];
                    let p = world.transform_point(Vec3::from_array(v.position));
                    let n_local = if smooth {
                        object.normals[v.normal as usize]
                    } else {
                        face_normal
                    };
                    let n = world
                        .transform_vector(Vec3::from_array(n_local))
                        .normalize();
                    let mut clip = scene.camera.project(p);
                    clip[0] += scene.shift[0] * clip[3];
                    clip[1] += scene.shift[1] * clip[3];
                    let [u, t] = poly.uvs[i];
                    ClipVertex {
                        clip,
                        attrs: [u, t, n.x, n.y, n.z],
                    }
                });
                let two_sided = material.flags & geo::MATERIAL_TWO_SIDED != 0;
                let cull = if two_sided { Cull::None } else { Cull::Back };
                let texture = self.textures[poly.material as usize].as_ref();
                let self_illum = material.flags & geo::MATERIAL_SELF_ILLUM != 0;
                let ambient = rgba8(material.ambient);
                let diffuse = rgba8(material.diffuse);

                draw_triangle(target, tri, cull, |f| {
                    let [u, t, nx, ny, nz] = f.attrs;
                    let base: Rgba = match texture {
                        Some(tex) => tex.sample(u, t),
                        None => [1.0, 1.0, 1.0, 1.0],
                    };
                    if base[3] < 0.02 {
                        return false;
                    }
                    let mut n = Vec3::new(nx, ny, nz).normalize();
                    if !f.front_facing {
                        n = -n;
                    }
                    let light = if self_illum {
                        [1.0; 3]
                    } else {
                        let k = n.dot(scene.key).max(0.0);
                        let fl = n.dot(scene.fill).max(0.0) * 0.25;
                        std::array::from_fn(|c| {
                            0.22 + ambient[c] * 0.1 + diffuse[c] * (k * 1.05 + fl)
                        })
                    };
                    let rgb: [f32; 3] = std::array::from_fn(|c| base[c] * light[c]);
                    canvas.set(f.x, f.y, [rgb[0], rgb[1], rgb[2], 1.0]);
                    true
                });
            }
        }
    }
}

impl ShipModel {
    pub fn load(
        big: &BigArchive,
        mesh_paths: &[&str],
        team: [[u8; 3]; 2],
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let parts = mesh_paths
            .iter()
            .map(|path| MeshPart::load(big, path, team))
            .collect::<Result<_, _>>()?;
        Ok(Self { parts })
    }

    fn world_points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.parts.iter().flat_map(MeshPart::world_points)
    }

    /// Centre and radius of a sphere enclosing every vertex in world space.
    pub fn bounds(&self) -> (Vec3, f32) {
        let (lo, hi) = self.world_points().fold(
            (
                Vec3::new(f32::MAX, f32::MAX, f32::MAX),
                Vec3::new(f32::MIN, f32::MIN, f32::MIN),
            ),
            |(lo, hi), p| (lo.min(p), hi.max(p)),
        );
        if lo.x > hi.x {
            return (Vec3::default(), 1.0);
        }
        let centre = (lo + hi) * 0.5;
        let radius = self
            .world_points()
            .map(|p| (p - centre).length())
            .fold(0.0, f32::max);
        (centre, radius.max(1e-3))
    }

    /// Places the camera so the ship's projected silhouette fills `view.fill` of the frame and
    /// is centred. Returns the camera and the normalised-device-coordinate shift to apply.
    fn frame(&self, view: &ShipView, aspect: f32) -> (Camera, [f32; 2]) {
        let (centre, radius) = self.bounds();
        let to_eye = orbit_direction(MESH_FRONT, MESH_UP, view.yaw, view.pitch);
        let camera_at = |distance: f32| {
            Camera::new(centre + to_eye * distance, -to_eye, MESH_UP, FOV_Y, aspect)
        };
        let points: Vec<Vec3> = self.world_points().collect();
        let fill = view.fill.clamp(0.05, 1.0);
        let mut distance = radius / (FOV_Y * 0.5).to_radians().sin();
        let mut shift = [0.0; 2];
        for _ in 0..4 {
            let camera = camera_at(distance);
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for &p in &points {
                let [x, y, _, w] = camera.project(p);
                for (k, v) in [x / w, y / w].into_iter().enumerate() {
                    lo[k] = lo[k].min(v);
                    hi[k] = hi[k].max(v);
                }
            }
            let extent = ((hi[0] - lo[0]) * 0.5).max((hi[1] - lo[1]) * 0.5);
            if !(extent.is_finite() && extent > 0.0) {
                break;
            }
            shift = [-(lo[0] + hi[0]) * 0.5, -(lo[1] + hi[1]) * 0.5];
            distance = (distance * extent / fill).max(radius * 1.05);
        }
        (camera_at(distance), shift)
    }

    /// Draws the ship onto `canvas` (which should start transparent for compositing).
    pub fn render(&self, canvas: &mut Canvas, view: &ShipView) {
        let aspect = canvas.width as f32 / canvas.height as f32;
        let (camera, centring) = self.frame(view, aspect);
        let scene = Scene {
            camera,
            shift: [
                centring[0] + view.offset[0] * 2.0,
                centring[1] - view.offset[1] * 2.0,
            ],
            // Key light from above, over the viewer's left shoulder; a dim fill from the right.
            key: (camera.up() * 0.7 - camera.right() * 0.5 - camera.forward() * 0.6).normalize(),
            fill: (-camera.up() * 0.4 + camera.right() * 0.8).normalize(),
        };
        let mut depth = vec![0.0f32; canvas.width * canvas.height];
        let mut target = Target {
            width: canvas.width,
            height: canvas.height,
            depth: Some(&mut depth),
        };
        for part in &self.parts {
            part.draw(canvas, &mut target, &scene);
        }
    }
}

fn load_lif(big: &BigArchive, path: &str) -> Result<Lif, Box<dyn std::error::Error>> {
    let data = big.read_file(path)?.ok_or("not in archive")?;
    Ok(Lif::parse(&data)?)
}

/// Accumulates each object's local matrix down the hierarchy (parent × local, as OpenGL's
/// matrix stack does when the engine renders daughters inside their parent's push).
fn world_matrices(geo: &Geo) -> Vec<Mat4> {
    fn resolve(geo: &Geo, i: usize, world: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = world[i] {
            return m;
        }
        let local = Mat4(geo.objects[i].local_matrix);
        let m = match geo.objects[i].parent {
            Some(p) if depth < geo.objects.len() => resolve(geo, p, world, depth + 1) * local,
            _ => local,
        };
        world[i] = Some(m);
        m
    }
    let mut world = vec![None; geo.objects.len()];
    (0..geo.objects.len())
        .map(|i| resolve(geo, i, &mut world, 0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_axes_are_right_handed() {
        // up × right = heading for the engine's (up, right, heading) column layout.
        let right = Vec3::new(0.0, 1.0, 0.0);
        assert_eq!(MESH_UP.cross(right), MESH_FRONT);
    }
}
