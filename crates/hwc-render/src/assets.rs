//! Loading ship assets (`.shp` stats, `.lod` mesh lists, `.geo`/`.peo` meshes, `.lif` textures,
//! `.mex` extensions) from the game's data, independent of the GPU.

use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use hwc_formats::big::BigArchive;
use hwc_formats::geo::Geo;
use hwc_formats::lif::Lif;
use hwc_formats::mex::{ChunkData, Mex};
use hwc_formats::script::{ScriptFile, ShipStats};

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;

/// Somewhere game files can be read from by their archive path (`dir\file.ext`, any case).
pub trait AssetSource {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Every file path the source holds, in archive form (lowercase, backslash separators).
    fn list(&self) -> Vec<String>;
}

impl AssetSource for BigArchive {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_file(path).ok().flatten()
    }

    fn list(&self) -> Vec<String> {
        self.entries()
            .iter()
            .map(|e| hwc_formats::big::normalize_name(&e.name))
            .collect()
    }
}

/// A directory tree holding extracted game files with lowercase names.
#[derive(Debug, Clone)]
pub struct DirSource {
    pub root: PathBuf,
}

impl DirSource {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    fn walk(dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let rel = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}\\{name}")
            };
            if entry.path().is_dir() {
                Self::walk(&entry.path(), &rel, out);
            } else {
                out.push(rel);
            }
        }
    }
}

impl AssetSource for DirSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let rel: PathBuf = path.to_ascii_lowercase().split(['\\', '/']).collect();
        std::fs::read(self.root.join(rel)).ok()
    }

    fn list(&self) -> Vec<String> {
        let mut out = Vec::new();
        Self::walk(&self.root, "", &mut out);
        out
    }
}

fn read_required(src: &dyn AssetSource, path: &str) -> Result<Vec<u8>> {
    src.read(path)
        .ok_or_else(|| format!("{path}: not found").into())
}

/// Reads and parses a text script (`.shp`, `.lod`, `.script`).
pub fn read_script(src: &dyn AssetSource, path: &str) -> Result<ScriptFile> {
    Ok(ScriptFile::parse_bytes(&read_required(src, path)?).map_err(|e| format!("{path}: {e}"))?)
}

/// Everything up to and including the last path separator.
fn dir_of(path: &str) -> &str {
    path.rfind(['\\', '/']).map_or("", |i| &path[..=i])
}

// ---------------------------------------------------------------------------------------------
// Team colours
// ---------------------------------------------------------------------------------------------

/// A player's texture colours and the material colours derived from them for flat
/// team-coloured polygons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TeamScheme {
    pub base: [u8; 3],
    pub stripe: [u8; 3],
}

/// Team colour table from `teamcolors.script`.
#[derive(Debug, Clone, PartialEq)]
pub struct TeamColors {
    pub schemes: Vec<TeamScheme>,
    /// Scales (out of 65536) turning the base colour into material ambient and diffuse.
    pub ambient_adjust: i32,
    pub diffuse_adjust: i32,
}

impl Default for TeamColors {
    /// Cataclysm's shipped `teamcolors.script` values.
    fn default() -> Self {
        let rows: [([u8; 3], [u8; 3]); 8] = [
            ([66, 108, 202], [255, 255, 255]),
            ([150, 128, 128], [200, 0, 0]),
            ([253, 237, 37], [171, 45, 20]),
            ([255, 255, 255], [0, 128, 128]),
            ([55, 151, 8], [179, 247, 101]),
            ([255, 128, 64], [255, 255, 0]),
            ([227, 173, 83], [152, 111, 29]),
            ([200, 0, 0], [0, 0, 255]),
        ];
        Self {
            schemes: rows
                .iter()
                .map(|&(base, stripe)| TeamScheme { base, stripe })
                .collect(),
            ambient_adjust: 15000,
            diffuse_adjust: 55000,
        }
    }
}

impl TeamColors {
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut t = Self::default();
        let rgb = |e: &hwc_formats::script::Entry| -> Option<(usize, [u8; 3])> {
            let v: Vec<i64> = e.values.iter().filter_map(|v| v.as_i64()).collect();
            let [i, r, g, b] = v.as_slice() else {
                return None;
            };
            let c = |x: i64| x.clamp(0, 255) as u8;
            Some((usize::try_from(*i).ok()?, [c(*r), c(*g), c(*b)]))
        };
        for e in script.entries() {
            match e.key.as_str() {
                "BaseColor" | "StripeColor" => {
                    let Some((i, c)) = rgb(e) else { continue };
                    if i >= t.schemes.len() {
                        t.schemes.resize(
                            i + 1,
                            TeamScheme {
                                base: [255; 3],
                                stripe: [255; 3],
                            },
                        );
                    }
                    if e.key == "BaseColor" {
                        t.schemes[i].base = c;
                    } else {
                        t.schemes[i].stripe = c;
                    }
                }
                "AmbientAdjust" => t.ambient_adjust = e.i32().unwrap_or(t.ambient_adjust),
                "DiffuseAdjust" => t.diffuse_adjust = e.i32().unwrap_or(t.diffuse_adjust),
                _ => {}
            }
        }
        t
    }

    /// Ambient and diffuse material colours for flat team-coloured polygons. The engine derives
    /// both the base and the stripe variants from the base colour.
    pub fn material_colors(&self, scheme: &TeamScheme) -> ([f32; 3], [f32; 3]) {
        let scale = |adjust: i32| {
            scheme
                .base
                .map(|c| (i32::from(c) * adjust / 65536).clamp(0, 255) as f32 / 255.0)
        };
        (scale(self.ambient_adjust), scale(self.diffuse_adjust))
    }
}

// ---------------------------------------------------------------------------------------------
// Ships
// ---------------------------------------------------------------------------------------------

/// A ship type: its `.shp` file and parsed stats.
#[derive(Debug, Clone)]
pub struct ShipDef {
    /// Race directory, e.g. `race_sect`.
    pub race: String,
    /// Lowercase type name, e.g. `smothership`.
    pub name: String,
    pub stats: ShipStats,
}

impl ShipDef {
    pub fn load(src: &dyn AssetSource, race: &str, name: &str) -> Result<Self> {
        let script = read_script(src, &format!("{race}\\{name}.shp"))?;
        Ok(Self {
            race: race.to_ascii_lowercase(),
            name: name.to_ascii_lowercase(),
            stats: ShipStats::from_script(&script),
        })
    }

    /// Every ship type in `race_*` directories whose `.shp` names a LOD file and whose mesh
    /// exists, sorted by race then name.
    pub fn list_all(src: &dyn AssetSource) -> Vec<ShipDef> {
        let files: std::collections::HashSet<String> = src.list().into_iter().collect();
        let mut found: Vec<(String, String)> = files
            .iter()
            .filter_map(|p| {
                let (race, file) = p.split_once('\\')?;
                let name = file.strip_suffix(".shp")?;
                (race.starts_with("race_") && !name.contains('\\'))
                    .then(|| (race.to_owned(), name.to_owned()))
            })
            .collect();
        found.sort();
        found.dedup();
        found
            .into_iter()
            .filter_map(|(race, name)| ShipDef::load(src, &race, &name).ok())
            .filter(|d| d.stats.lod_file.is_some() && files.contains(&d.mesh_path(src)))
            .collect()
    }

    /// Archive path of the most detailed mesh, from the `.lod` file's first level (falling back
    /// to the conventional `name\rl0\lod0\name.geo`).
    pub fn mesh_path(&self, src: &dyn AssetSource) -> String {
        let from_lod = self.stats.lod_file.as_deref().and_then(|lod| {
            let script = read_script(src, &format!("{}\\{lod}", self.race)).ok()?;
            let mesh = script.get("pMeshFile0")?.string()?.to_owned();
            Some(format!("{}\\{mesh}", self.race).to_ascii_lowercase())
        });
        from_lod.unwrap_or_else(|| format!("{0}\\{1}\\rl0\\lod0\\{1}.geo", self.race, self.name))
    }

    pub fn mex_path(&self) -> Option<String> {
        let mex = self.stats.p_mex_data.as_deref()?;
        Some(format!("{}\\{mex}", self.race).to_ascii_lowercase())
    }
}

/// How a nav light blinks and looks, from a `.shp` `NavLight` line plus its `.mex` point.
#[derive(Debug, Clone, PartialEq)]
pub struct NavLightDef {
    pub name: String,
    /// Mesh-space position.
    pub position: Vec3,
    pub color: [u8; 3],
    pub flash_on: f32,
    pub flash_off: f32,
    pub start_delay: f32,
    /// Billboard edge length in world units.
    pub size: f32,
    pub min_lod: i32,
    /// Texture path without extension, or `None` for an untextured disc.
    pub texture: Option<String>,
}

impl NavLightDef {
    fn parse(entry: &hwc_formats::script::Entry) -> Option<Self> {
        let v = &entry.values;
        let name = v.first()?.to_string();
        let f = |i: usize| v.get(i).and_then(|x| x.as_f32()).unwrap_or(0.0);
        let texture = v
            .get(7)
            .map(|t| t.to_string())
            .filter(|t| !t.eq_ignore_ascii_case("NULL"));
        Some(Self {
            name,
            position: Vec3::ZERO,
            color: [255; 3],
            flash_on: f(2),
            flash_off: f(3),
            start_delay: f(4),
            size: f(5),
            min_lod: v.get(6).and_then(|x| x.as_i32()).unwrap_or(0),
            texture,
        })
    }
}

/// One `.geo` mesh with its decoded textures (kept as `.lif` so team colours can change).
#[derive(Debug, Clone)]
pub struct MeshPart {
    pub path: String,
    pub geo: Geo,
    /// One entry per material: the texture, if the material names one that loaded.
    pub textures: Vec<Option<Lif>>,
    /// Each polygon object's matrix accumulated down the hierarchy (mesh space).
    pub object_matrices: Vec<Mat4>,
}

impl MeshPart {
    pub fn load(src: &dyn AssetSource, mesh_path: &str) -> Result<Self> {
        let data = read_required(src, mesh_path)?;
        let geo = Geo::parse(&data).map_err(|e| format!("{mesh_path}: {e}"))?;
        let dir = dir_of(mesh_path);
        let textures = geo
            .materials
            .iter()
            .map(|m| {
                let name = m.texture.as_deref()?;
                let path = format!("{dir}{}.lif", name.trim_end_matches(".lif"));
                load_lif(src, &path)
                    .map_err(|e| eprintln!("warning: texture {path}: {e}"))
                    .ok()
            })
            .collect();
        let object_matrices = object_matrices(&geo);
        Ok(Self {
            path: mesh_path.to_owned(),
            geo,
            textures,
            object_matrices,
        })
    }

    pub fn triangle_count(&self) -> usize {
        self.geo.objects.iter().map(|o| o.polygons.len()).sum()
    }
}

pub fn load_lif(src: &dyn AssetSource, path: &str) -> Result<Lif> {
    Ok(Lif::parse(&read_required(src, path)?)?)
}

/// Accumulates each object's local matrix down the hierarchy (parent × local).
pub fn object_matrices(geo: &Geo) -> Vec<Mat4> {
    fn resolve(geo: &Geo, i: usize, cache: &mut [Option<Mat4>], depth: usize) -> Mat4 {
        if let Some(m) = cache[i] {
            return m;
        }
        let local = Mat4::from_cols_array(&geo.objects[i].local_matrix);
        let m = match geo.objects[i].parent {
            Some(p) if p < geo.objects.len() && depth < geo.objects.len() => {
                resolve(geo, p, cache, depth + 1) * local
            }
            _ => local,
        };
        cache[i] = Some(m);
        m
    }
    let mut cache = vec![None; geo.objects.len()];
    (0..geo.objects.len())
        .map(|i| resolve(geo, i, &mut cache, 0))
        .collect()
}

/// A ship ready for upload: one or more mesh parts sharing the ship's frame (a mothership and
/// its modules), its nav lights and the data the camera needs.
#[derive(Debug, Clone)]
pub struct ShipModel {
    pub name: String,
    pub parts: Vec<MeshPart>,
    pub nav_lights: Vec<NavLightDef>,
    /// Nav light bitmaps by texture name (as written in the `.shp`).
    pub nav_textures: Vec<(String, Lif)>,
    /// `Col0` sphere from the `.mex` (mesh space), if present.
    pub collision_sphere: Option<(Vec3, f32)>,
    pub minimum_zoom_distance: f32,
}

impl ShipModel {
    pub fn load(src: &dyn AssetSource, def: &ShipDef) -> Result<Self> {
        let mut model = Self {
            name: def.name.clone(),
            parts: Vec::new(),
            nav_lights: Vec::new(),
            nav_textures: Vec::new(),
            collision_sphere: None,
            minimum_zoom_distance: def.stats.minimum_zoom_distance.unwrap_or(0.0),
        };
        model.add_module(src, def)?;
        let mex = def
            .mex_path()
            .and_then(|p| src.read(&p))
            .and_then(|d| Mex::parse(&d).ok());
        model.collision_sphere = mex
            .as_ref()
            .and_then(Mex::collision_sphere)
            .map(|(c, r)| (Vec3::from_array(c), r));
        Ok(model)
    }

    /// Adds another ship type's mesh and nav lights in this ship's frame (mothership modules
    /// are modelled in the mothership's coordinates).
    pub fn add_module(&mut self, src: &dyn AssetSource, def: &ShipDef) -> Result<()> {
        self.parts.push(MeshPart::load(src, &def.mesh_path(src))?);
        let mex = def
            .mex_path()
            .and_then(|p| src.read(&p))
            .and_then(|d| Mex::parse(&d).ok());
        for entry in &def.stats.nav_lights {
            let Some(mut nav) = NavLightDef::parse(entry) else {
                continue;
            };
            let point = mex.as_ref().and_then(|m| {
                m.chunks.iter().find_map(|c| match &c.data {
                    ChunkData::NavLight {
                        position,
                        color,
                        name,
                        ..
                    } if name.eq_ignore_ascii_case(&nav.name) => Some((*position, *color)),
                    _ => None,
                })
            });
            let Some((position, color)) = point else {
                continue;
            };
            nav.position = Vec3::from_array(position);
            nav.color = color;
            if let Some(tex) = &nav.texture {
                if !self.nav_textures.iter().any(|(n, _)| n == tex) {
                    match load_lif(src, &format!("{tex}.lif")) {
                        Ok(lif) => self.nav_textures.push((tex.clone(), lif)),
                        Err(e) => eprintln!("warning: nav light texture {tex}: {e}"),
                    }
                }
            }
            self.nav_lights.push(nav);
        }
        Ok(())
    }

    /// Every vertex in mesh space.
    pub fn points(&self) -> impl Iterator<Item = Vec3> + '_ {
        self.parts.iter().flat_map(|part| {
            part.geo
                .objects
                .iter()
                .zip(&part.object_matrices)
                .flat_map(|(o, m)| {
                    o.vertices
                        .iter()
                        .map(move |v| m.transform_point3(Vec3::from_array(v.position)))
                })
        })
    }

    /// Centre and radius of a sphere around every vertex (mesh space).
    pub fn bounds(&self) -> (Vec3, f32) {
        let (lo, hi) = self.points().fold(
            (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)),
            |(lo, hi), p| (lo.min(p), hi.max(p)),
        );
        if lo.x > hi.x {
            return (Vec3::ZERO, 1.0);
        }
        let centre = (lo + hi) * 0.5;
        let r = self
            .points()
            .map(|p| p.distance(centre))
            .fold(0.0, f32::max);
        (centre, r.max(1e-3))
    }

    pub fn triangle_count(&self) -> usize {
        self.parts.iter().map(MeshPart::triangle_count).sum()
    }
}

/// Ship types whose mesh is modelled in `def`'s frame and is drawn with it: for a mothership,
/// the other types of the same race whose names extend its name (e.g. `smothershipcommand`).
pub fn mothership_modules<'a>(def: &ShipDef, all: &'a [ShipDef]) -> Vec<&'a ShipDef> {
    if !def.name.ends_with("mothership") {
        return Vec::new();
    }
    all.iter()
        .filter(|d| d.race == def.race && d.name != def.name && d.name.starts_with(&def.name))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_material_colours_use_the_base_colour() {
        let t = TeamColors::default();
        let (amb, dif) = t.material_colors(&t.schemes[0]);
        // 202 * 15000 / 65536 = 46, 202 * 55000 / 65536 = 169
        assert!((amb[2] - 46.0 / 255.0).abs() < 1e-6);
        assert!((dif[2] - 169.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn team_script_parsing() {
        let s =
            ScriptFile::parse("BaseColor 1, 1, 2, 3\nStripeColor 1, 4, 5, 6\nAmbientAdjust 100\n");
        let t = TeamColors::from_script(&s);
        assert_eq!(
            t.schemes[1],
            TeamScheme {
                base: [1, 2, 3],
                stripe: [4, 5, 6]
            }
        );
        assert_eq!(t.ambient_adjust, 100);
        assert_eq!(t.schemes.len(), 8);
    }

    #[test]
    fn nav_light_line() {
        let s = ScriptFile::parse(
            "NavLight spine1, NAVLIGHT_FadeOnOff, 1, 2, 0.5, 35, 2, etg\\textures\\star32\n",
        );
        let e = s.get("NavLight").unwrap();
        let n = NavLightDef::parse(e).unwrap();
        assert_eq!(n.name, "spine1");
        assert_eq!(
            (n.flash_on, n.flash_off, n.start_delay, n.size),
            (1.0, 2.0, 0.5, 35.0)
        );
        assert_eq!(n.min_lod, 2);
        assert_eq!(n.texture.as_deref(), Some("etg\\textures\\star32"));
    }

    fn data_source() -> Option<Box<dyn AssetSource>> {
        if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
            return Some(Box::new(DirSource::new(dir)));
        }
        let game = hwc_formats::game_dir()?;
        let big = BigArchive::open(game.join("cataclysm.big")).ok()?;
        Some(Box::new(big))
    }

    #[test]
    fn loads_every_ship_from_game_data() {
        let Some(src) = data_source() else {
            eprintln!("skipping: set HWC_EXTRACTED_DIR or HWC_GAME_DIR");
            return;
        };
        let all = ShipDef::list_all(src.as_ref());
        assert!(all.len() > 50, "only {} ships", all.len());
        let mut failures = Vec::new();
        for def in &all {
            match ShipModel::load(src.as_ref(), def) {
                Ok(m) => assert!(m.triangle_count() > 0, "{} has no triangles", def.name),
                Err(e) => failures.push(format!("{}: {e}", def.name)),
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
        let kuunlan = all.iter().find(|d| d.name == "smothership").unwrap();
        assert!(mothership_modules(kuunlan, &all).len() >= 5);
        let model = ShipModel::load(src.as_ref(), kuunlan).unwrap();
        assert!(model.collision_sphere.is_some());
        assert!(
            !model.nav_lights.is_empty(),
            "{:?}",
            kuunlan.stats.nav_lights.len()
        );
        assert_eq!(model.nav_textures.len(), 1);
    }
}
