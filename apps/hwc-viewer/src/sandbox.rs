//! Sandbox mode: a live `hwc-sim` universe rendered with the game camera. Ships can be
//! selected (click or drag a box), ordered to move on the movement plane (`M` then click; a
//! simple version of the game's move pie plate is drawn), put into formation (`F` cycles) and
//! ordered to attack (`A` then click an enemy). Bullets and beams are drawn as lines, hits as
//! sparks, deaths as an expanding flash, and every ship has a health bar.
//!
//! The simulation runs at the original's 16 Hz through the task scheduler model of
//! `hwc_math::timing` (240 Hz ticks, the universe task every 15). Rendering shows the latest
//! update as the original does; `--interpolate` blends between the last two updates instead.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Mat4, Vec3, Vec4};
use hwc_formats::big::BigArchive;
use hwc_formats::hsf::Hsf;
use hwc_math::timing::{TaskTicker, TIMER_RESOLUTION};
use hwc_render::assets::{mothership_modules, read_script, AssetSource};
use hwc_render::{
    Background, CameraTweaks, GpuBackground, GpuShip, OrbitCamera, Renderer, Scene, SceneLighting,
    ShipDef, ShipInstance, ShipModel, TeamColors, TeamPaint,
};
use hwc_sim::harness::{group_centre, Run, Scenario};
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::{command, formation, ShipId, SimTweaks};

use crate::overlay::Overlay;

const SELECT_COLOR: [f32; 4] = [0.2, 1.0, 0.3, 0.9];
const BOX_COLOR: [f32; 4] = [0.4, 0.9, 1.0, 0.8];
const PLATE_COLOR: [f32; 4] = [0.3, 0.6, 1.0, 0.55];
const DEST_COLOR: [f32; 4] = [1.0, 0.85, 0.2, 0.9];
const ATTACK_COLOR: [f32; 4] = [1.0, 0.25, 0.2, 0.9];
/// Bullet colours by weapon technology (projectile, energy, ion, plasma, laser, -, -, big gun,
/// beast infection). The original takes them from the gun events; these are stand-ins.
const WEAPON_COLORS: [[f32; 4]; 9] = [
    [1.0, 0.9, 0.4, 1.0],
    [0.4, 1.0, 0.5, 1.0],
    [0.5, 0.95, 1.0, 1.0],
    [1.0, 0.5, 0.2, 1.0],
    [1.0, 0.3, 0.3, 1.0],
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 0.8, 0.6, 1.0],
    [0.7, 1.0, 0.3, 0.9],
];
const HIT_COLOR: [f32; 4] = [1.0, 1.0, 0.7, 0.9];
const EXPLOSION_COLOR: [f32; 4] = [1.0, 0.6, 0.2, 1.0];
/// How long hit sparks and explosion flashes stay (seconds of game time).
const HIT_TIME: f32 = 0.25;
const EXPLOSION_TIME: f32 = 1.5;

/// A short-lived effect drawn in the overlay.
#[derive(Debug, Clone, Copy)]
struct Effect {
    position: Vec3,
    radius: f32,
    start: f32,
    explosion: bool,
}

/// A ship type ready to draw.
struct ShipArt {
    gpu: Option<GpuShip>,
    model: ShipModel,
}

/// What the left mouse button does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Select,
    /// Choosing a destination on the movement plane.
    Move,
    /// Choosing a ship to attack.
    Attack,
}

pub struct Sandbox {
    src: BigArchive,
    pub run: Run,
    previous: BTreeMap<ShipId, (Vec3, Mat4)>,
    art: BTreeMap<ShipType, ShipArt>,
    team_colors: TeamColors,
    pub lighting: SceneLighting,
    background_path: Option<String>,
    background: Option<GpuBackground>,
    pub camera: OrbitCamera,
    pub selection: Vec<ShipId>,
    pub mode: Mode,
    pub interpolate: bool,
    pub paused: bool,
    ticker: TaskTicker,
    tick_carry: f64,
    /// Last move destination, shown until the ships arrive.
    pub destination: Option<Vec3>,
    pub formation_index: usize,
    pub nav_lights: bool,
    pub status: String,
    /// The camera keeps looking at these ships (the game's camera focus).
    pub focus: Vec<ShipId>,
    effects: Vec<Effect>,
}

/// Engine vector to render vector.
fn v3(v: hwc_math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

impl Sandbox {
    pub fn new(src: BigArchive, scenario: Scenario, interpolate: bool) -> Result<Self, String> {
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = SimTweaks::load(&src);
        let run = Run::new(statics, tweaks, scenario).map_err(|e| e.to_string())?;
        let ctweaks = read_script(&src, "camera.script")
            .map(|s| CameraTweaks::from_script(&s))
            .unwrap_or_default();
        let team_colors = read_script(&src, "teamcolors.script")
            .map(|s| TeamColors::from_script(&s))
            .unwrap_or_default();
        let mut camera = OrbitCamera::new(ctweaks, 9000.0);
        camera.angle = (-150.0f32).to_radians();
        camera.declination = (-35.0f32).to_radians();
        camera.rotate_angle(0.0);
        camera.rotate_declination(0.0);
        let lighting = AssetSource::read(&src, "hsf\\default.hsf")
            .and_then(|d| Hsf::parse(&d).ok())
            .map(|h| SceneLighting::from_hsf(&h))
            .unwrap_or_default();
        let mut sb = Self {
            src,
            run,
            previous: BTreeMap::new(),
            art: BTreeMap::new(),
            team_colors,
            lighting,
            background_path: Some("btg\\mission01.btg".into()),
            background: None,
            camera,
            selection: Vec::new(),
            mode: Mode::Select,
            interpolate,
            paused: false,
            ticker: TaskTicker::new(15),
            tick_carry: 0.0,
            destination: None,
            formation_index: 0,
            nav_lights: true,
            status: String::new(),
            focus: Vec::new(),
            effects: Vec::new(),
        };
        sb.load_art()?;
        sb.camera.look_at = sb.fleet_centre();
        sb.snapshot();
        Ok(sb)
    }

    pub fn set_background(&mut self, path: Option<String>) {
        self.background_path = path;
        self.background = None;
    }

    fn load_art(&mut self) -> Result<(), String> {
        let all = ShipDef::list_all(&self.src);
        let types: Vec<ShipType> = self
            .run
            .universe
            .ships
            .iter()
            .map(|s| s.ship_type)
            .collect();
        for t in types {
            if self.art.contains_key(&t) {
                continue;
            }
            let race = t.race().name().to_ascii_lowercase();
            let name = t.name().to_ascii_lowercase();
            let def = all
                .iter()
                .find(|d| d.race == race && d.name == name)
                .cloned()
                .ok_or_else(|| format!("no model for {race}\\{name}"))?;
            let mut model = ShipModel::load(&self.src, &def).map_err(|e| e.to_string())?;
            for m in mothership_modules(&def, &all) {
                if let Err(e) = model.add_module(&self.src, m) {
                    eprintln!("warning: module {}: {e}", m.name);
                }
            }
            self.art.insert(t, ShipArt { gpu: None, model });
        }
        Ok(())
    }

    /// Uploads models and the background (call before drawing).
    pub fn prepare(&mut self, renderer: &Renderer) {
        let owners: BTreeMap<ShipType, usize> = self.run.universe.ships.iter().map(|s| (s.ship_type, s.player)).collect();
        for (t, art) in self.art.iter_mut() {
            let player = owners.get(t).copied().unwrap_or(0);
            let scheme = self.team_colors.schemes.get(player).or(self.team_colors.schemes.first()).copied();
            if art.gpu.is_none() {
                let paint = match scheme {
                    Some(s) => TeamPaint::Scheme(&self.team_colors, s),
                    None => TeamPaint::None,
                };
                art.gpu = Some(renderer.upload_ship(&art.model, paint));
            }
        }
        if self.background.is_none() {
            if let Some(p) = &self.background_path {
                match Background::load(&self.src, p) {
                    Ok(bg) => self.background = Some(renderer.upload_background(&bg)),
                    Err(e) => {
                        eprintln!("warning: {e}");
                        self.background_path = None;
                    }
                }
            }
        }
    }

    pub fn fleet_centre(&self) -> Vec3 {
        let ids: Vec<ShipId> = self.run.universe.ship_list.clone();
        v3(group_centre(&self.run.universe, &ids))
    }

    fn transform_of(s: &hwc_sim::Ship) -> (Vec3, Mat4) {
        let m = &s.rotinfo.coordsys.m;
        let p = v3(s.posinfo.position);
        let mat = Mat4::from_cols(
            Vec4::new(m[0], m[1], m[2], 0.0),
            Vec4::new(m[3], m[4], m[5], 0.0),
            Vec4::new(m[6], m[7], m[8], 0.0),
            p.extend(1.0),
        );
        (p, mat)
    }

    fn snapshot(&mut self) {
        self.previous = self
            .run
            .universe
            .ships
            .iter()
            .map(|s| (s.id, Self::transform_of(s)))
            .collect();
    }

    /// One universe update (keeps the previous state for interpolation).
    pub fn step(&mut self) {
        self.snapshot();
        self.run.step();
        let now = self.run.universe.total_time_elapsed;
        self.effects.retain(|e| now - e.start < if e.explosion { EXPLOSION_TIME } else { HIT_TIME });
        for e in &self.run.universe.events {
            match e {
                hwc_sim::universe::SimEvent::BulletHit { position, .. } => {
                    self.effects.push(Effect { position: v3(*position), radius: 0.0, start: now, explosion: false })
                }
                hwc_sim::universe::SimEvent::ShipDied { position, radius, .. } => {
                    self.effects.push(Effect { position: v3(*position), radius: *radius, start: now, explosion: true })
                }
                _ => {}
            }
        }
        let u = &self.run.universe;
        self.selection.retain(|id| u.ship(*id).is_some());
        self.focus.retain(|id| u.ship(*id).is_some());
        self.update_focus();
        let u = &self.run.universe;
        let busy = self.selection.iter().any(|id| {
            u.ship(*id)
                .and_then(|s| s.command)
                .and_then(|c| u.commands.get(c))
                .is_some_and(|c| c.order == command::order::MOVE)
        });
        if !busy {
            self.destination = None;
        }
    }

    /// Keeps the camera on the focused ships.
    pub fn update_focus(&mut self) {
        if !self.focus.is_empty() {
            let c = group_centre(&self.run.universe, &self.focus);
            self.camera.look_at = v3(c);
        }
    }

    /// Advances real time: the task scheduler hands out 240 Hz ticks and runs the universe
    /// task once per 15 (at most 8 times per frame, like the original).
    pub fn advance(&mut self, seconds: f64) {
        if self.paused {
            return;
        }
        self.tick_carry += seconds * f64::from(TIMER_RESOLUTION);
        let ticks = self.tick_carry.floor();
        self.tick_carry -= ticks;
        let calls = self.ticker.advance(ticks as u32);
        for _ in 0..calls {
            self.step();
        }
    }

    /// Blend factor between the previous and the current update.
    fn alpha(&self) -> f32 {
        if !self.interpolate || self.paused {
            return 1.0;
        }
        ((f64::from(self.ticker.carry) + self.tick_carry) / f64::from(self.ticker.ticks_per_call))
            .clamp(0.0, 1.0) as f32
    }

    /// Render transform of each ship.
    fn render_transforms(&self) -> Vec<(ShipId, ShipType, Mat4, f32)> {
        let a = self.alpha();
        let age = self.run.universe.total_time_elapsed;
        self.run
            .universe
            .ships
            .iter()
            .map(|s| {
                let (p, m) = Self::transform_of(s);
                let m = match (a < 1.0, self.previous.get(&s.id)) {
                    (true, Some((pp, pm))) => {
                        let pos = pp.lerp(p, a);
                        let cols = |m: &Mat4, i: usize| m.col(i).truncate();
                        let blend = |i: usize| {
                            cols(pm, i)
                                .lerp(cols(&m, i), a)
                                .normalize_or_zero()
                                .extend(0.0)
                        };
                        Mat4::from_cols(blend(0), blend(1), blend(2), pos.extend(1.0))
                    }
                    _ => m,
                };
                (s.id, s.ship_type, m, age - s.time_created + 0.75)
            })
            .collect()
    }

    pub fn instances(&self) -> Vec<ShipInstance<'_>> {
        self.render_transforms()
            .into_iter()
            .filter_map(|(_, t, m, age)| {
                let gpu = self.art.get(&t)?.gpu.as_ref()?;
                Some(ShipInstance {
                    ship: gpu,
                    transform: m,
                    age,
                })
            })
            .collect()
    }

    pub fn scene<'a>(&'a self, instances: &'a [ShipInstance<'a>]) -> Scene<'a> {
        Scene {
            camera: &self.camera,
            lighting: &self.lighting,
            background: self.background.as_ref(),
            ships: instances,
            nav_lights: self.nav_lights,
        }
    }

    fn view_proj(&self, size: (u32, u32)) -> Mat4 {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        self.camera.projection(aspect) * self.camera.view()
    }

    /// World point to pixel (None behind the camera).
    pub fn project(&self, size: (u32, u32), p: Vec3) -> Option<[f32; 2]> {
        let c = self.view_proj(size) * p.extend(1.0);
        if c.w <= 1e-3 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some([
            (n.x + 1.0) * 0.5 * size.0 as f32,
            (1.0 - n.y) * 0.5 * size.1 as f32,
        ])
    }

    /// Ray through a pixel: (origin, direction).
    fn ray(&self, size: (u32, u32), px: [f32; 2]) -> (Vec3, Vec3) {
        let inv = self.view_proj(size).inverse();
        let nx = px[0] / size.0 as f32 * 2.0 - 1.0;
        let ny = 1.0 - px[1] / size.1 as f32 * 2.0;
        let a = inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * Vec4::new(nx, ny, 1.0, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        (a, (b - a).normalize())
    }

    /// The movement plane: horizontal through the selection's centre.
    fn plane_point(&self, size: (u32, u32), px: [f32; 2]) -> Option<Vec3> {
        if self.selection.is_empty() {
            return None;
        }
        let c = v3(group_centre(&self.run.universe, &self.selection));
        let (o, d) = self.ray(size, px);
        if d.z.abs() < 1e-5 {
            return None;
        }
        let t = (c.z - o.z) / d.z;
        (t > 0.0).then(|| o + d * t)
    }

    /// Screen box of a ship (its collision sphere's projection).
    fn screen_box(&self, size: (u32, u32), s: &hwc_sim::Ship) -> Option<([f32; 2], [f32; 2])> {
        let r = self
            .run
            .universe
            .statics
            .ship(s.ship_type)
            .map_or(50.0, |st| st.header.coll.coll_sphere_size)
            .max(20.0);
        let c = v3(s.coll.coll_position);
        let p = self.project(size, c)?;
        let right = Vec3::new(
            self.camera.view().row(0).x,
            self.camera.view().row(0).y,
            self.camera.view().row(0).z,
        );
        let q = self.project(size, c + right * r)?;
        let rad = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2))
            .sqrt()
            .max(6.0);
        Some(([p[0] - rad, p[1] - rad], [p[0] + rad, p[1] + rad]))
    }

    /// Click selection: the nearest ship whose screen box contains the point.
    pub fn click_select(&mut self, size: (u32, u32), px: [f32; 2], add: bool) {
        let mut best: Option<(f32, ShipId)> = None;
        for s in self.run.universe.ships.iter() {
            if let Some((lo, hi)) = self.screen_box(size, s) {
                if px[0] >= lo[0] && px[0] <= hi[0] && px[1] >= lo[1] && px[1] <= hi[1] {
                    let d = (v3(s.posinfo.position) - self.camera.eye()).length();
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, s.id));
                    }
                }
            }
        }
        if !add {
            self.selection.clear();
        }
        if let Some((_, id)) = best {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
        }
    }

    /// Box selection (the original selects the ships whose centres fall inside the band box).
    pub fn box_select(&mut self, size: (u32, u32), a: [f32; 2], b: [f32; 2], add: bool) {
        let (lo, hi) = (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        );
        if !add {
            self.selection.clear();
        }
        let ids: Vec<ShipId> = self.run.universe.ship_list.clone();
        for id in ids {
            let s = self.run.universe.ship(id).expect("ship");
            if let Some(p) = self.project(size, v3(s.posinfo.position)) {
                if p[0] >= lo[0]
                    && p[0] <= hi[0]
                    && p[1] >= lo[1]
                    && p[1] <= hi[1]
                    && !self.selection.contains(&id)
                {
                    self.selection.push(id);
                }
            }
        }
    }

    /// Moves the selection to the plane point under the cursor.
    pub fn move_to_cursor(&mut self, size: (u32, u32), px: [f32; 2]) -> bool {
        let Some(p) = self.plane_point(size, px) else {
            return false;
        };
        let from = group_centre(&self.run.universe, &self.selection);
        let dest = hwc_math::Vector::new(p.x, p.y, p.z);
        let sel = self.selection.clone();
        command::cl_move(
            &mut self.run.universe,
            &sel,
            command::waypoint_mode::ONCE,
            from,
            &[dest],
        );
        self.destination = Some(p);
        self.status = format!(
            "move {} ships to ({:.0}, {:.0}, {:.0})",
            sel.len(),
            p.x,
            p.y,
            p.z
        );
        true
    }

    /// `F`: next formation for the selection.
    pub fn cycle_formation(&mut self) {
        if self.selection.len() < 2 {
            self.status = "formation needs at least two ships".into();
            return;
        }
        let t = formation::CYCLE[self.formation_index % formation::CYCLE.len()];
        self.formation_index += 1;
        let sel = self.selection.clone();
        command::cl_formation(&mut self.run.universe, &sel, t);
        self.status = format!("formation: {}", formation::name(t));
    }

    /// `A` then click: the selection attacks the clicked ship.
    pub fn attack_clicked(&mut self, size: (u32, u32), px: [f32; 2]) -> bool {
        let mut best: Option<(f32, ShipId)> = None;
        for s in self.run.universe.ships.iter() {
            if let Some((lo, hi)) = self.screen_box(size, s) {
                if px[0] >= lo[0] && px[0] <= hi[0] && px[1] >= lo[1] && px[1] <= hi[1] {
                    let d = (v3(s.posinfo.position) - self.camera.eye()).length();
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, s.id));
                    }
                }
            }
        }
        let Some((_, target)) = best else {
            return false;
        };
        let sel: Vec<ShipId> = self.selection.iter().copied().filter(|&id| id != target).collect();
        command::cl_attack(&mut self.run.universe, &sel, &[target]);
        let t = self.run.universe.ship(target).map(|s| s.ship_type);
        self.status = format!("{} ships attack {:?}", sel.len(), t);
        true
    }

    pub fn halt(&mut self) {
        let sel = self.selection.clone();
        command::cl_halt(&mut self.run.universe, &sel);
        self.destination = None;
        self.status = "halt".into();
    }

    /// Selection markers, the drag box and the move plate.
    pub fn overlay(
        &self,
        size: (u32, u32),
        cursor: Option<[f32; 2]>,
        drag: Option<([f32; 2], [f32; 2])>,
    ) -> Overlay {
        let mut o = Overlay::default();
        let transforms = self.render_transforms();
        self.combat_overlay(&mut o, size);
        for id in &self.selection {
            let Some(s) = self.run.universe.ship(*id) else {
                continue;
            };
            let mut s = s.clone();
            if let Some((_, _, m, _)) = transforms.iter().find(|t| t.0 == *id) {
                let off = s.coll.coll_offset;
                let p = m.col(3).truncate();
                s.coll.coll_position = hwc_math::Vector::new(p.x + off.x, p.y + off.y, p.z + off.z);
            }
            if let Some((lo, hi)) = self.screen_box(size, &s) {
                o.brackets(lo, hi, SELECT_COLOR);
            }
        }
        if let Some((a, b)) = drag {
            o.rect(
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
                BOX_COLOR,
            );
        }
        if self.mode == Mode::Move && !self.selection.is_empty() {
            let c = v3(group_centre(&self.run.universe, &self.selection));
            let target = cursor.and_then(|px| self.plane_point(size, px));
            let radius = target.map_or(2000.0, |t| (t - c).truncate().length().max(200.0));
            self.plate(&mut o, size, c, radius);
            if let Some(t) = target {
                if let (Some(a), Some(b)) = (self.project(size, c), self.project(size, t)) {
                    o.line(a, b, DEST_COLOR);
                    o.rect(
                        [b[0] - 4.0, b[1] - 4.0],
                        [b[0] + 4.0, b[1] + 4.0],
                        DEST_COLOR,
                    );
                }
            }
        }
        if let Some(d) = self.destination {
            if let Some(p) = self.project(size, d) {
                o.line([p[0] - 8.0, p[1]], [p[0] + 8.0, p[1]], DEST_COLOR);
                o.line([p[0], p[1] - 8.0], [p[0], p[1] + 8.0], DEST_COLOR);
            }
        }
        o
    }

    /// Bullets, beams, hit sparks, explosion flashes and health bars.
    fn combat_overlay(&self, o: &mut Overlay, size: (u32, u32)) {
        let u = &self.run.universe;
        for &bid in &u.bullet_list {
            let b = u.bullets.bullet(bid);
            let p = v3(b.posinfo.position);
            let tail = if hwc_sim::bullet::is_beam(b.weapon) {
                p + v3(b.heading) * b.beam_travel_dist
            } else {
                p - v3(b.heading) * b.length_mag.max(30.0)
            };
            let c = WEAPON_COLORS[(b.weapon as usize).min(WEAPON_COLORS.len() - 1)];
            if let (Some(a), Some(z)) = (self.project(size, p), self.project(size, tail)) {
                o.line(a, z, c);
            }
        }
        let now = u.total_time_elapsed;
        for e in &self.effects {
            let Some(c) = self.project(size, e.position) else { continue };
            if e.explosion {
                let k = ((now - e.start) / EXPLOSION_TIME).clamp(0.0, 1.0);
                let r_world = e.radius.max(40.0) * (0.5 + 2.5 * k);
                let right = self.camera.view().row(0).truncate();
                let r = self.project(size, e.position + right * r_world).map_or(10.0, |q| ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2)).sqrt());
                let mut col = EXPLOSION_COLOR;
                col[3] = 1.0 - k;
                for ring in [1.0, 0.6] {
                    let pts: Vec<[f32; 2]> = (0..24)
                        .map(|i| {
                            let a = i as f32 / 24.0 * std::f32::consts::TAU;
                            [c[0] + a.cos() * r * ring, c[1] + a.sin() * r * ring]
                        })
                        .collect();
                    o.polygon(&pts, col);
                }
                for i in 0..8 {
                    let a = i as f32 / 8.0 * std::f32::consts::TAU + k;
                    o.line([c[0] + a.cos() * r * 0.3, c[1] + a.sin() * r * 0.3], [c[0] + a.cos() * r * 1.2, c[1] + a.sin() * r * 1.2], col);
                }
            } else {
                o.line([c[0] - 4.0, c[1] - 4.0], [c[0] + 4.0, c[1] + 4.0], HIT_COLOR);
                o.line([c[0] - 4.0, c[1] + 4.0], [c[0] + 4.0, c[1] - 4.0], HIT_COLOR);
            }
        }
        for s in u.ships.iter() {
            let Some(max) = u.statics.ship(s.ship_type).map(|st| st.health.max_health) else { continue };
            let Some((lo, hi)) = self.screen_box(size, s) else { continue };
            let frac = (s.health / max).clamp(0.0, 1.0);
            let w = (hi[0] - lo[0]).max(16.0);
            let (x0, y) = (lo[0], lo[1] - 6.0);
            let col = if frac > 0.6 {
                [0.2, 1.0, 0.2, 0.9]
            } else if frac > 0.3 {
                [1.0, 0.9, 0.2, 0.9]
            } else {
                [1.0, 0.2, 0.2, 0.9]
            };
            o.line([x0, y], [x0 + w, y], [0.2, 0.2, 0.2, 0.8]);
            o.line([x0, y], [x0 + w * frac, y], col);
            o.line([x0, y + 1.0], [x0 + w * frac, y + 1.0], col);
            if let Some(t) = s.attack.target.and_then(|t| u.ship(t)) {
                if self.selection.contains(&s.id) {
                    if let (Some(a), Some(b)) = (self.project(size, v3(s.posinfo.position)), self.project(size, v3(t.posinfo.position))) {
                        o.line(a, b, [ATTACK_COLOR[0], ATTACK_COLOR[1], ATTACK_COLOR[2], 0.35]);
                    }
                }
            }
        }
    }

    /// The move pie plate: concentric rings and spokes on the movement plane.
    fn plate(&self, o: &mut Overlay, size: (u32, u32), c: Vec3, radius: f32) {
        for k in 1..=3 {
            let r = radius * k as f32 / 3.0;
            let pts: Vec<[f32; 2]> = (0..48)
                .filter_map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    self.project(size, c + Vec3::new(a.cos() * r, a.sin() * r, 0.0))
                })
                .collect();
            if pts.len() == 48 {
                o.polygon(&pts, PLATE_COLOR);
            }
        }
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            if let (Some(p), Some(q)) = (
                self.project(size, c),
                self.project(size, c + Vec3::new(a.cos() * radius, a.sin() * radius, 0.0)),
            ) {
                o.line(p, q, PLATE_COLOR);
            }
        }
    }

    pub fn title(&self) -> String {
        let u = &self.run.universe;
        format!(
            "hwc-viewer sandbox - tick {} ({:.1} s) - {} ships, {} selected - {}{}{}",
            u.update_counter,
            u.total_time_elapsed,
            u.ship_list.len(),
            self.selection.len(),
            match self.mode {
                Mode::Move => "MOVE: click a point - ",
                Mode::Attack => "ATTACK: click a ship - ",
                Mode::Select => "",
            },
            if self.paused { "PAUSED - " } else { "" },
            self.status
        )
    }
}
