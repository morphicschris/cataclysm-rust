//! Viewer state: which ship, background, team colours and lighting are shown, and the camera.
//! Nothing here depends on the window system.

use std::time::Instant;

use hwc_formats::hsf::Hsf;
use hwc_render::assets::{mothership_modules, read_script, AssetSource};
use hwc_render::{
    mesh_to_world, Background, CameraTweaks, GpuBackground, GpuShip, OrbitCamera, Renderer, Scene,
    SceneLighting, ShipDef, ShipInstance, ShipModel, TeamColors, TeamPaint,
};

/// Ship viewer defaults from `shipview.script` (Cataclysm values).
const SV_ANGLE: f32 = -150.0;
const SV_DECLINATION: f32 = -20.0;
const SV_INITIAL_DISTANCE: f32 = 900.0;

pub struct Viewer {
    src: Box<dyn AssetSource>,
    pub ships: Vec<ShipDef>,
    pub ship_index: usize,
    pub backgrounds: Vec<String>,
    /// `None` shows a black sky.
    pub background_index: Option<usize>,
    pub team_colors: TeamColors,
    /// `None` shows textures without team colours.
    pub team: Option<usize>,
    pub lighting: SceneLighting,
    pub lighting_name: String,
    pub camera: OrbitCamera,
    /// Closest zoom for the current ship (its minimum zoom distance, at least the camera's).
    pub min_zoom: f32,
    pub nav_lights: bool,
    model: Option<ShipModel>,
    gpu_ship: Option<GpuShip>,
    gpu_background: Option<GpuBackground>,
    ship_shown_at: Instant,
}

impl Viewer {
    pub fn new(src: Box<dyn AssetSource>) -> Result<Self, String> {
        let ships = ShipDef::list_all(src.as_ref());
        if ships.is_empty() {
            return Err("no ships found in the archive".into());
        }
        let mut backgrounds: Vec<String> = src
            .list()
            .into_iter()
            .filter(|p| {
                p.starts_with("btg\\") && p.ends_with(".btg") && p.matches('\\').count() == 1
            })
            .collect();
        backgrounds.sort();
        let tweaks = read_script(src.as_ref(), "camera.script")
            .map(|s| CameraTweaks::from_script(&s))
            .unwrap_or_default();
        let team_colors = read_script(src.as_ref(), "teamcolors.script")
            .map(|s| TeamColors::from_script(&s))
            .unwrap_or_default();
        let mut v = Self {
            src,
            ships,
            ship_index: 0,
            backgrounds,
            background_index: None,
            team_colors,
            team: Some(0),
            lighting: SceneLighting::default(),
            lighting_name: "light.script default".into(),
            camera: OrbitCamera::new(tweaks, SV_INITIAL_DISTANCE),
            min_zoom: tweaks.min_zoomout_distance,
            nav_lights: true,
            model: None,
            gpu_ship: None,
            gpu_background: None,
            ship_shown_at: Instant::now(),
        };
        if let Err(e) = v.set_lighting("hsf\\default.hsf") {
            eprintln!("warning: {e}");
        }
        Ok(v)
    }

    pub fn set_lighting(&mut self, path: &str) -> Result<(), String> {
        let path = normalize_asset(path, "hsf", "hsf");
        let data = self.src.read(&path).ok_or(format!("{path}: not found"))?;
        let hsf = Hsf::parse(&data).map_err(|e| format!("{path}: {e}"))?;
        self.lighting = SceneLighting::from_hsf(&hsf);
        self.lighting_name = path;
        Ok(())
    }

    pub fn select_ship_by_name(&mut self, name: &str) -> Result<(), String> {
        let wanted = name.to_ascii_lowercase().replace('/', "\\");
        let index = self
            .ships
            .iter()
            .position(|d| d.name == wanted || format!("{}\\{}", d.race, d.name) == wanted)
            .ok_or_else(|| {
                format!("no ship named {name:?} (try e.g. smothership, race_beast\\bcarrier)")
            })?;
        self.ship_index = index;
        Ok(())
    }

    pub fn select_background(&mut self, name: &str) -> Result<(), String> {
        let path = normalize_asset(name, "btg", "btg");
        let index = self
            .backgrounds
            .iter()
            .position(|b| *b == path)
            .ok_or_else(|| format!("no background {path:?}"))?;
        self.background_index = Some(index);
        Ok(())
    }

    pub fn current(&self) -> &ShipDef {
        &self.ships[self.ship_index]
    }

    pub fn step_ship(&mut self, delta: isize) {
        let n = self.ships.len() as isize;
        self.ship_index = (self.ship_index as isize + delta).rem_euclid(n) as usize;
        self.model = None;
    }

    /// Jumps to the first ship of the next (or previous) race.
    pub fn step_race(&mut self, delta: isize) {
        let race = self.current().race.clone();
        let races: Vec<&str> = {
            let mut r: Vec<&str> = self.ships.iter().map(|d| d.race.as_str()).collect();
            r.dedup();
            r
        };
        let i = races.iter().position(|r| *r == race).unwrap_or(0) as isize;
        let next = races[(i + delta).rem_euclid(races.len() as isize) as usize].to_owned();
        self.ship_index = self.ships.iter().position(|d| d.race == next).unwrap_or(0);
        self.model = None;
    }

    pub fn step_background(&mut self, delta: isize) {
        // Cycle through "none" and every background.
        let n = self.backgrounds.len() as isize + 1;
        let cur = self.background_index.map_or(0, |i| i as isize + 1);
        let next = (cur + delta).rem_euclid(n);
        self.background_index = (next > 0).then(|| next as usize - 1);
        self.gpu_background = None;
    }

    pub fn step_team(&mut self) {
        let n = self.team_colors.schemes.len();
        self.team = match self.team {
            Some(i) if i + 1 < n => Some(i + 1),
            Some(_) => None,
            None => Some(0),
        };
        self.gpu_ship = None;
    }

    /// Loads the current ship (plus mothership modules) if needed and frames the camera on it.
    pub fn ensure_model(&mut self) -> Result<(), String> {
        if self.model.is_some() {
            return Ok(());
        }
        let def = self.current().clone();
        let mut model = ShipModel::load(self.src.as_ref(), &def).map_err(|e| e.to_string())?;
        for module in mothership_modules(&def, &self.ships) {
            if let Err(e) = model.add_module(self.src.as_ref(), module) {
                eprintln!("warning: module {}: {e}", module.name);
            }
        }
        self.frame_camera(&model);
        self.model = Some(model);
        self.gpu_ship = None;
        self.ship_shown_at = Instant::now();
        Ok(())
    }

    /// Places the camera at the ship viewer's default angles, looking at the ship's centre
    /// (its collision sphere, or the centre of all meshes when modules are attached) from far
    /// enough away to show the whole model. Zooming afterwards follows the in-game limits.
    fn frame_camera(&mut self, model: &ShipModel) {
        let bounds = model.bounds();
        let (centre, _) = match model.collision_sphere {
            Some(sphere) if model.parts.len() == 1 => sphere,
            _ => bounds,
        };
        let radius = bounds.1;
        let tweaks = self.camera.tweaks;
        let mut cam = OrbitCamera::new(tweaks, SV_INITIAL_DISTANCE);
        cam.look_at = mesh_to_world().transform_point3(centre);
        cam.angle = SV_ANGLE.to_radians();
        cam.declination = SV_DECLINATION.to_radians();
        cam.rotate_angle(0.0);
        cam.mouse_sens = self.camera.mouse_sens;
        self.min_zoom = model.minimum_zoom_distance.max(cam.closest_zoom);
        let fit = radius / (tweaks.field_of_view.to_radians() * 0.5).sin() * 1.1;
        cam.distance = fit.clamp(self.min_zoom, cam.farthest_zoom);
        self.camera = cam;
    }

    pub fn reset_camera(&mut self) {
        if let Some(model) = self.model.take() {
            self.frame_camera(&model);
            self.model = Some(model);
        }
    }

    /// Makes sure GPU copies of the ship and background exist.
    pub fn prepare(&mut self, renderer: &Renderer) -> Result<(), String> {
        self.ensure_model()?;
        if self.gpu_ship.is_none() {
            let model = self.model.as_ref().expect("model loaded above");
            let paint = match self.team {
                Some(i) => TeamPaint::Scheme(&self.team_colors, self.team_colors.schemes[i]),
                None => TeamPaint::None,
            };
            self.gpu_ship = Some(renderer.upload_ship(model, paint));
        }
        if self.gpu_background.is_none() {
            if let Some(i) = self.background_index {
                let path = &self.backgrounds[i];
                match Background::load(self.src.as_ref(), path) {
                    Ok(bg) => self.gpu_background = Some(renderer.upload_background(&bg)),
                    Err(e) => eprintln!("warning: {e}"),
                }
            }
        }
        Ok(())
    }

    /// Records the frame. Call [`prepare`](Self::prepare) first.
    pub fn render(
        &self,
        renderer: &mut Renderer,
        encoder: &mut hwc_render::wgpu::CommandEncoder,
        color: &hwc_render::wgpu::TextureView,
        depth: &hwc_render::wgpu::TextureView,
        size: (u32, u32),
    ) {
        let instances = self.instances(None);
        renderer.render(encoder, color, depth, size, &self.scene(&instances));
    }

    pub fn scene<'a>(&'a self, instances: &'a [ShipInstance<'a>]) -> Scene<'a> {
        Scene {
            camera: &self.camera,
            lighting: &self.lighting,
            background: self.gpu_background.as_ref(),
            ships: instances,
            nav_lights: self.nav_lights,
        }
    }

    pub fn instances(&self, age: Option<f32>) -> Vec<ShipInstance<'_>> {
        self.gpu_ship
            .iter()
            .map(|ship| ShipInstance {
                ship,
                transform: mesh_to_world(),
                age: age.unwrap_or_else(|| self.ship_shown_at.elapsed().as_secs_f32()),
            })
            .collect()
    }

    pub fn title(&self) -> String {
        let d = self.current();
        let tris = self.gpu_ship.as_ref().map_or(0, |s| s.triangle_count);
        let bg = self
            .background_index
            .map_or("none", |i| self.backgrounds[i].as_str());
        let team = self.team.map_or("off".to_string(), |t| t.to_string());
        format!(
            "hwc-viewer - {}\\{} ({}/{}) - {tris} triangles - background {bg} - team {team}",
            d.race,
            d.name,
            self.ship_index + 1,
            self.ships.len()
        )
    }
}

/// Accepts `name`, `name.ext` or `dir\name.ext` and returns the archive path `dir\name.ext`.
pub fn normalize_asset(name: &str, dir: &str, ext: &str) -> String {
    let mut p = name.to_ascii_lowercase().replace('/', "\\");
    if !p.contains('\\') {
        p = format!("{dir}\\{p}");
    }
    if !p.ends_with(&format!(".{ext}")) {
        p.push('.');
        p.push_str(ext);
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_names() {
        assert_eq!(
            normalize_asset("mission13", "btg", "btg"),
            "btg\\mission13.btg"
        );
        assert_eq!(normalize_asset("BTG/M.btg", "btg", "btg"), "btg\\m.btg");
    }
}
