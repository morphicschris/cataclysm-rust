//! The live backdrop of the main menu: the Kuun-Lan as it is in Mission 1 (its ore can still
//! on), lying in Mission 1's asteroid field under its sky and lighting, with a few workers
//! harvesting the asteroids around it and flying back to unload, and the camera drifting
//! slowly round it.
//!
//! This is a modern addition: the original's front end has a still picture behind its menus.
//! It reuses the mission machinery: Mission 1's level is loaded as for play
//! ([`mission::Loader::start`], `singlePlayerLoadNewLevel`), then emptied of every ship (the
//! level's own and the fleet waiting in hyperspace are never made), and the mothership is
//! created where the level's `StartPointPlayer` is, with Mission 1's modules
//! (`univMissionModuleTypesGet`) and the docking bay the opening NIS gives it. The KAS script
//! never runs: only the universe updates ([`World::step_universe`], as during an NIS level),
//! so there is no letterbox, speech, objective or enemy. The workers harvest with
//! `clCollectResource`; the asteroids they work are topped up now and then so they never run
//! out and the workers never wander off to fields further away.
//!
//! The menu draws its own interface over the picture every frame, so the 3D view is drawn at
//! most [`RENDER_HZ`] times a second and the last picture is copied in between.

// (Until the menu uses it.)

use std::collections::BTreeMap;
use std::rc::Rc;

use glam::Vec3;
use hwc_formats::GameData;
use hwc_math::Vector;
use hwc_render::{OffscreenTarget, Renderer};
use hwc_sim::object::ShipId;
use hwc_sim::resource::{ResourceId, ResourceKind};
use hwc_sim::statics::ShipType;
use hwc_ui::canvas::Canvas;

use crate::data::Assets;
use crate::mission::Loader;
use crate::world::{Sim, World};

/// How often a second the 3D view is drawn at most.
pub const RENDER_HZ: f32 = 30.0;
/// The most render scale the backdrop draws at, whatever the options say: it sits behind the
/// menu, and on an integrated GPU a full-size picture with the modern look costs about 7 ms at
/// 1920x1080 (0.75 with the heat haze off, about 5).
pub const MAX_SCALE: f32 = 0.75;
/// The workers harvesting round the mothership.
const WORKERS: usize = 6;
/// Asteroids added round the mothership (Mission 1's own field near the start point has only
/// four), between [`FIELD_INNER`] and [`FIELD_OUTER`] from its centre.
const FIELD_ASTEROIDS: usize = 70;
const FIELD_INNER: f32 = 3200.0;
const FIELD_OUTER: f32 = 11000.0;
/// Asteroids further than this from the mothership are not given to the workers.
const FIELD_REACH: f32 = 12000.0;
/// Seconds between top-ups of the worked asteroids.
const REFILL_EVERY: f32 = 20.0;
/// The camera drifts to and fro round the mothership: its mean angle is [`VIEW_ANGLE`] round
/// from the main light's (radians; this looks at a three-quarter view of the Kuun-Lan's
/// stern, its engines towards the camera and the bright part of Mission 1's sky behind it), the swing either side of it (radians)
/// and its period (seconds).
const VIEW_ANGLE: f32 = 4.55;
const ANGLE_SWING: f32 = 0.45;
const ANGLE_PERIOD: f32 = 240.0;
/// The camera's slow rise and fall: mean and swing of the declination (radians, negative is
/// above), and its period in seconds.
const DECLINATION: f32 = -0.3;
const DECLINATION_SWING: f32 = 0.12;
const DECLINATION_PERIOD: f32 = 97.0;
/// Camera distance in mothership collision radii, with a slow breath in and out.
const DISTANCE_RADII: f32 = 3.7;
const DISTANCE_SWING: f32 = 0.06;
const DISTANCE_PERIOD: f32 = 131.0;

enum Step {
    Statics,
    Level,
    World,
    Upload,
    Done,
}

/// Loads the scene a piece at a time ([`MenuSceneLoader::advance`] once a frame), so the menu
/// stays responsive while it does.
pub struct MenuSceneLoader {
    assets: Assets,
    player_colors: Option<([u8; 3], [u8; 3])>,
    loader: Loader,
    run: Option<hwc_sim::mission::MissionRun>,
    setup: Option<Setup>,
    world: Option<World>,
    step: Step,
}

/// What the scene set up in the universe.
#[derive(Debug, Clone)]
struct Setup {
    mothership: ShipId,
    workers: Vec<ShipId>,
    /// The asteroids the workers may harvest, with the resource units they started with.
    field: BTreeMap<ResourceId, i32>,
    /// The camera's mean angle ([`base_angle`]).
    base_angle: f32,
}

impl MenuSceneLoader {
    /// `player_colors` are the player's team colours as picked in the front end (see
    /// [`crate::play::player_colors`]).
    pub fn new(data: Rc<GameData>, player_colors: Option<([u8; 3], [u8; 3])>) -> Self {
        Self {
            assets: Assets(data),
            player_colors,
            loader: Loader::new(1, 1),
            run: None,
            setup: None,
            world: None,
            step: Step::Statics,
        }
    }

    /// Does one piece of the work (the statics, the level, the art, the upload); returns the
    /// scene when it is ready.
    pub fn advance(&mut self, renderer: &mut Renderer) -> Result<Option<MenuScene>, String> {
        match self.step {
            Step::Statics => {
                self.loader.load_statics(&self.assets);
                self.step = Step::Level;
            }
            Step::Level => {
                let mut run = self.loader.start(&self.assets)?;
                clear(&mut run);
                self.run = Some(run);
                self.step = Step::World;
            }
            Step::World => {
                let run = self.run.take().ok_or("menu scene: no level")?;
                let mut world =
                    World::new(self.assets.clone(), Sim::Mission(run), self.player_colors)?;
                // The scene moves the camera itself.
                world.follow_focus = false;
                world.focus.clear();
                world.interpolate = true;
                self.setup = Some(populate(&mut world)?);
                // The art of the ships just made.
                world.load_new_art();
                self.world = Some(world);
                self.step = Step::Upload;
            }
            Step::Upload => {
                let mut world = self.world.take().ok_or("menu scene: no world")?;
                world.prepare(renderer);
                let setup = self.setup.take().ok_or("menu scene: not set up")?;
                self.step = Step::Done;
                return Ok(Some(MenuScene::new(world, setup)));
            }
            Step::Done => return Err("menu scene: already loaded".into()),
        }
        Ok(None)
    }
}

/// Empties the level of ships: its own (Mission 1's KAS teams and hidden helpers) and the
/// fleet waiting in hyperspace, which never comes out; the KAS script never starts.
fn clear(run: &mut hwc_sim::mission::MissionRun) {
    run.game.fleet.clear();
    run.game.hyper.fleet_ids.clear();
    let u = &mut run.universe;
    let ids: Vec<ShipId> = u.ships.iter().map(|s| s.id).collect();
    for id in ids {
        hwc_sim::damage::really_delete_ship(u, id);
    }
}

/// The camera's mean angle: [`VIEW_ANGLE`] round from the main light's.
fn base_angle(world: &World) -> f32 {
    world
        .lighting
        .lights
        .first()
        .map_or(0.0, |l| l.direction.y.atan2(l.direction.x))
        + VIEW_ANGLE
}

/// Whether `p` (relative to the mothership's centre) is clear of the path the camera's eye can
/// take ([`MenuScene::place_camera`], with room for the sideways offset).
fn clear_of_camera(p: Vec3, base: f32, radius: f32) -> bool {
    const CLEARANCE: f32 = 2600.0;
    let d = radius * DISTANCE_RADII;
    (0..=16).all(|i| {
        let a = base + ANGLE_SWING * (i as f32 / 8.0 - 1.0);
        (0..=4).all(|j| {
            let dec = DECLINATION + DECLINATION_SWING * (j as f32 / 2.0 - 1.0);
            let eye = Vec3::new(dec.cos() * a.cos(), dec.cos() * a.sin(), -dec.sin()) * d;
            eye.distance(p) > CLEARANCE
        })
    })
}

/// Puts the Kuun-Lan where Mission 1's `StartPointPlayer` is, the asteroid field round it
/// (clear of the camera's path) and its workers, harvesting.
fn populate(world: &mut World) -> Result<Setup, String> {
    let base = base_angle(world);
    let run = world.run_mut();
    let start = run
        .game
        .level
        .point(hwc_kas::kas_hash("StartPointPlayer"))
        .ok_or("mission 1 has no StartPointPlayer")?;
    let u = &mut run.universe;
    let at = Vector::new(start[0], start[1], start[2]);
    let ms = u
        .add_ship(ShipType::sMothership, 0, at, None)
        .map_err(|e| format!("mothership: {e:?}"))?;
    // As the fleet comes out of hyperspace (`PutShipOutsideAt`): turned by the start point's
    // rotation.
    hwc_sim::level::rotate_yaw(u, ms, start[3] * hwc_sim::level::DEG_TO_RAD);
    let bay = u
        .add_ship(ShipType::sMothershipDockingBay, 0, at, None)
        .map_err(|e| format!("docking bay: {e:?}"))?;
    hwc_sim::level::attach_module(u, &mut run.game.level, bay, ms);
    if run.game.level.motherships.is_empty() {
        run.game.level.motherships.push(None);
    }
    run.game.level.motherships[0] = Some(ms);

    // A field round the ship: a flattened shell, smallest asteroids most common (the smallest
    // kind is not harvested; it is there for the look).
    let mut seed = 0x1234_5678u32;
    let mut rand = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let radius = u
        .statics
        .ship(ShipType::sMothership)
        .map_or(1250.0, |st| st.header.coll.coll_sphere_size);
    let mut placed = 0;
    while placed < FIELD_ASTEROIDS {
        let a = rand() * std::f32::consts::TAU;
        let r = FIELD_INNER + (FIELD_OUTER - FIELD_INNER) * rand().sqrt();
        let z = (rand() - 0.5) * 0.5 * r;
        if !clear_of_camera(Vec3::new(r * a.cos(), r * a.sin(), z), base, radius) {
            continue;
        }
        placed += 1;
        let k = match rand() {
            x if x < 0.35 => 0,
            x if x < 0.65 => 1,
            x if x < 0.88 => 2,
            _ => 3,
        };
        let p = Vector::new(at.x + r * a.cos(), at.y + r * a.sin(), at.z + z);
        hwc_sim::resource::add_resource(u, ResourceKind::Asteroid(k), p);
    }

    // The asteroids within reach, nearest first.
    let mut near: Vec<(f32, ResourceId, i32)> = u
        .base
        .resources
        .list
        .iter()
        .filter(|r| matches!(r.kind, ResourceKind::Asteroid(k) if k > 0))
        .map(|r| (dist(r.position, at), r.id, r.resource_value))
        .filter(|(d, ..)| *d < FIELD_REACH)
        .collect();
    near.sort_by(|a, b| a.0.total_cmp(&b.0));
    let field: BTreeMap<ResourceId, i32> = near.iter().map(|&(_, id, v)| (id, v)).collect();

    let mut workers = Vec::new();
    for k in 0..WORKERS {
        let a = k as f32 / WORKERS as f32 * std::f32::consts::TAU;
        let p = Vector::new(
            at.x + 2600.0 * a.cos(),
            at.y + 2600.0 * a.sin(),
            at.z - 400.0,
        );
        let w = u
            .add_ship(ShipType::sWorker, 0, p, None)
            .map_err(|e| format!("worker: {e:?}"))?;
        workers.push(w);
    }
    for (k, &w) in workers.iter().enumerate() {
        let target = near.get(k % near.len().max(1)).map(|n| n.1);
        hwc_sim::resource::cl_collect_resource(u, &[w], target);
    }
    Ok(Setup {
        mothership: ms,
        workers,
        field,
        base_angle: base,
    })
}

fn dist(a: Vector, b: Vector) -> f32 {
    let (x, y, z) = (a.x - b.x, a.y - b.y, a.z - b.z);
    (x * x + y * y + z * z).sqrt()
}

/// The running backdrop.
pub struct MenuScene {
    world: World,
    setup: Setup,
    /// Where the mothership sits across the picture, 0..1 of the width.
    focus_x: f32,
    quality: hwc_render::RenderQuality,
    target: Option<OffscreenTarget>,
    target_quality: hwc_render::RenderQuality,
    /// The last picture drawn, and its size.
    picture: Vec<[u8; 4]>,
    picture_size: (i32, i32),
    /// Seconds since the last picture.
    since_render: f32,
    /// Seconds the scene has run.
    time: f32,
    since_refill: f32,
    /// The mothership's centre and collision radius.
    centre: Vec3,
    radius: f32,
    /// The camera's mean angle ([`VIEW_ANGLE`] from the main light's).
    base_angle: f32,
    /// The last picture's milliseconds: frame preparation, drawing with the read-back, copy.
    draw_ms: [f64; 3],
}

impl MenuScene {
    fn new(world: World, setup: Setup) -> Self {
        let u = world.universe();
        let (centre, radius) = u.ship(setup.mothership).map_or((Vec3::ZERO, 1500.0), |s| {
            let p = s.posinfo.position;
            let r = u
                .statics
                .ship(s.ship_type)
                .map_or(1500.0, |st| st.header.coll.coll_sphere_size);
            (Vec3::new(p.x, p.y, p.z), r)
        });
        let base_angle = setup.base_angle;
        let mut s = Self {
            world,
            setup,
            focus_x: 0.62,
            quality: hwc_render::RenderQuality::default(),
            target: None,
            target_quality: hwc_render::RenderQuality::default(),
            picture: Vec::new(),
            picture_size: (0, 0),
            since_render: f32::INFINITY,
            time: 0.0,
            since_refill: 0.0,
            centre,
            radius,
            base_angle,
            draw_ms: [0.0; 3],
        };
        s.place_camera();
        s
    }

    /// Where the ship sits horizontally, 0..1 of the width (0.62 by default: clear of a menu
    /// panel on the left third).
    pub fn set_focus_x(&mut self, x: f32) {
        self.focus_x = x.clamp(0.0, 1.0);
    }

    /// The 3D view's quality and the ship look from the options, as the game reads them
    /// (`Game::apply_modern_options`), with the render scale at most [`MAX_SCALE`] and no heat
    /// haze (the engines are far off; it is the dearest post effect).
    pub fn set_options(&mut self, options: &hwc_ui::options::Options) {
        let mut quality = hwc_render::RenderQuality {
            post: crate::game::post_fx(options),
            ..hwc_render::RenderQuality::from_options(
                options.get("hwcRenderScale"),
                options.get("hwcMsaa"),
                options.get("hwcFxaa"),
            )
        };
        quality.scale = quality.scale.min(MAX_SCALE);
        quality.post.heat = 0.0;
        self.quality = quality;
        let w = &mut self.world;
        w.lens_flares = options.get("enableLensFlare") != 0;
        w.full_detail = options.get("hwcFullDetailShips") != 0;
        w.modern_fields = options.get("hwcModernFields") != 0;
        w.set_smooth_trails(options.get("hwcSmoothTrails") != 0);
        w.set_glows_in_proportion(options.get("hwcGlowsInProportion") != 0);
        w.ship_look = crate::game::ship_look(options);
        // A new picture with the new look at once.
        self.since_render = f32::INFINITY;
    }

    /// Real time: the universe runs (at the game's 16 updates a second) and the camera drifts.
    pub fn update(&mut self, dt: f32) {
        let dt = dt.clamp(0.0, 0.25);
        let steps = self.world.due_steps(f64::from(dt));
        for _ in 0..steps {
            self.world.step_universe();
        }
        self.time += dt;
        self.since_render += dt;
        self.since_refill += dt;
        if self.since_refill >= REFILL_EVERY {
            self.since_refill = 0.0;
            self.keep_up();
        }
        self.place_camera();
    }

    /// Tops up the field and sends idle workers back to work.
    fn keep_up(&mut self) {
        let u = self.world.universe_mut();
        for r in u.base.resources.list.iter_mut() {
            if let Some(&v) = self.setup.field.get(&r.id) {
                r.resource_value = r.resource_value.max(v);
            }
        }
        let idle: Vec<ShipId> = self
            .setup
            .workers
            .iter()
            .copied()
            .filter(|&w| {
                u.ship(w).is_some_and(|s| {
                    s.command.and_then(|c| u.commands.get(c)).is_none()
                        && s.flags & hwc_sim::object::sof::HIDE == 0
                })
            })
            .collect();
        if !idle.is_empty() {
            hwc_sim::resource::cl_collect_resource(u, &idle, None);
        }
    }

    /// The camera's angle, declination and distance at the scene's time.
    fn place_camera(&mut self) {
        let wave = |period: f32| (self.time * std::f32::consts::TAU / period).sin();
        let c = &mut self.world.camera;
        c.angle = self.base_angle + ANGLE_SWING * wave(ANGLE_PERIOD);
        c.declination = DECLINATION + DECLINATION_SWING * wave(DECLINATION_PERIOD);
        c.distance = self.radius * DISTANCE_RADII * (1.0 + DISTANCE_SWING * wave(DISTANCE_PERIOD));
        c.look_at = self.centre;
    }

    /// Moves the look-at point sideways so the mothership lands at `focus_x` across a picture
    /// of `aspect`.
    fn offset_camera(&mut self, aspect: f32) {
        let c = &mut self.world.camera;
        c.look_at = self.centre;
        let eye = c.eye();
        let forward = (c.look_at - eye).normalize_or_zero();
        let right = forward.cross(c.up()).normalize_or_zero();
        let half = (c.tweaks.field_of_view.to_radians() * 0.5).tan() * aspect;
        let ndc = 2.0 * self.focus_x - 1.0;
        c.look_at = self.centre - right * (c.distance * half * ndc);
    }

    /// Draws the 3D view into `canvas`, every pixel, at the canvas's size: a new picture at most
    /// [`RENDER_HZ`] times a second, the last one otherwise.
    pub fn render(&mut self, renderer: &mut Renderer, canvas: &mut Canvas) -> Result<(), String> {
        let size = (canvas.width, canvas.height);
        if size.0 <= 0 || size.1 <= 0 {
            return Ok(());
        }
        if self.picture_size != size || self.since_render >= 1.0 / RENDER_HZ {
            self.draw(renderer, size)?;
            self.since_render = 0.0;
        }
        canvas.pixels.copy_from_slice(&self.picture);
        Ok(())
    }

    fn draw(&mut self, renderer: &mut Renderer, size: (i32, i32)) -> Result<(), String> {
        let px = (size.0 as u32, size.1 as u32);
        let samples = renderer.set_sample_count(self.quality.msaa);
        if self.target.as_ref().is_none_or(|t| {
            t.plan.logical != px || t.plan.samples != samples || self.target_quality != self.quality
        }) {
            self.target = Some(OffscreenTarget::with_quality(
                renderer,
                px,
                px,
                self.quality,
            ));
            self.target_quality = self.quality;
        }
        let t0 = std::time::Instant::now();
        self.world.prepare(renderer);
        self.offset_camera(px.0 as f32 / px.1 as f32);
        let frame = self.world.frame(px);
        let target = self.target.as_ref().expect("target");
        let t1 = std::time::Instant::now();
        let rgba = self.world.render_offscreen(renderer, target, &frame)?;
        let t2 = std::time::Instant::now();
        self.picture.clear();
        self.picture.extend(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .map(|p| [p[0], p[1], p[2], 255]),
        );
        self.picture
            .resize((size.0 * size.1) as usize, [0, 0, 0, 255]);
        self.picture_size = size;
        let ms = |a: std::time::Instant, b: std::time::Instant| (b - a).as_secs_f64() * 1e3;
        self.draw_ms = [ms(t0, t1), ms(t1, t2), ms(t2, std::time::Instant::now())];
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Instant;

    use super::*;

    fn save(canvas: &Canvas, name: &str) {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/menu_scene");
        std::fs::create_dir_all(&dir).expect("screenshot dir");
        canvas
            .save_png(&dir.join(format!("{name}.png")))
            .expect("png");
    }

    /// Loads the backdrop headlessly, runs it a minute and draws it at 1920x1080 and 1280x720
    /// (`screenshots/menu_scene/`), with the time each picture takes. Needs the game install
    /// (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise.
    #[test]
    #[ignore]
    fn menu_scene_shots() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else {
            eprintln!("skipped: HWC_GAME_DIR missing");
            return;
        };
        let Some((device, queue)) = crate::test_gpu::device() else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let data = Rc::new(GameData::open(&dir).expect("game data"));
        let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
        let gpu_timing = renderer.set_gpu_timing(true);
        let t0 = Instant::now();
        let mut loader = MenuSceneLoader::new(data, None);
        let mut scene = loop {
            let t = Instant::now();
            if let Some(s) = loader.advance(&mut renderer).expect("load") {
                break s;
            }
            eprintln!("load step {:.0} ms", t.elapsed().as_secs_f64() * 1e3);
        };
        eprintln!("loaded in {:.2} s", t0.elapsed().as_secs_f64());
        scene.set_options(&hwc_ui::options::Options::default());
        scene.set_focus_x(0.62);
        {
            let u = scene.world.universe();
            for s in u.ships.iter() {
                eprintln!(
                    "ship {:?} {:?} at {:?}",
                    s.id, s.ship_type, s.posinfo.position
                );
            }
            eprintln!("field: {} asteroids", scene.setup.field.len());
        }
        let dt = 1.0 / 60.0;
        let mut big = Canvas::new(1920, 1080);
        let mut small = Canvas::new(1280, 720);
        let mut times: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let mut gpu: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
        let shots = [2.0, 15.0, 30.0, 45.0, 60.0];
        let mut next = 0;
        let mut t = 0.0f32;
        while next < shots.len() {
            scene.update(dt);
            t += dt;
            if t >= shots[next] {
                for (name, c) in [("720", &mut small), ("1080", &mut big)] {
                    // Two pictures each: the first after a size change makes the target.
                    for _ in 0..3 {
                        scene.since_render = f32::INFINITY;
                        let s = Instant::now();
                        scene.render(&mut renderer, c).expect("render");
                        times
                            .entry(name)
                            .or_default()
                            .push(s.elapsed().as_secs_f64() * 1e3);
                    }
                    if let Some(g) = renderer.take_gpu_ms() {
                        gpu.entry(name).or_default().push(g);
                    }
                    eprintln!(
                        "  {name}: prepare {:.1} ms, draw and read back {:.1} ms, copy {:.1} ms",
                        scene.draw_ms[0], scene.draw_ms[1], scene.draw_ms[2]
                    );
                    save(c, &format!("{name}_{:02}s", shots[next] as i32));
                }
                let u = scene.world.universe();
                for &w in &scene.setup.workers {
                    if let Some(s) = u.ship(w) {
                        eprintln!(
                            "  {t:.0}s worker {:?} at {:?} order {:?}",
                            w,
                            s.posinfo.position,
                            s.command.and_then(|c| u.commands.get(c)).map(|c| c.order)
                        );
                    }
                }
                next += 1;
            }
        }
        for (name, v) in &times {
            // The first picture of each size builds the target.
            let warm: Vec<f64> = v.chunks(3).flat_map(|c| c[1..].to_vec()).collect();
            let avg = warm.iter().sum::<f64>() / warm.len() as f64;
            let max = warm.iter().cloned().fold(0.0, f64::max);
            eprintln!("render {name}: {avg:.1} ms average, {max:.1} ms worst");
        }
        if !gpu_timing {
            eprintln!("no GPU timestamps on this adapter");
        }
        for (name, v) in &gpu {
            let avg = v.iter().sum::<f64>() / v.len() as f64;
            eprintln!("GPU {name}: {avg:.2} ms average over {} pictures", v.len());
        }
        // A cached frame only copies the picture.
        let s = Instant::now();
        scene.render(&mut renderer, &mut big).expect("render");
        eprintln!(
            "cached 1080 frame: {:.2} ms",
            s.elapsed().as_secs_f64() * 1e3
        );
    }

    /// The scene left an hour: the workers keep harvesting and unloading, stay near the
    /// mothership, and the mothership stays where it is. Needs `HWC_GAME_DIR` and a GPU.
    #[test]
    #[ignore]
    fn menu_scene_hour() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else {
            eprintln!("skipped: HWC_GAME_DIR missing");
            return;
        };
        let Some((device, queue)) = crate::test_gpu::device() else {
            eprintln!("skipped: no GPU adapter");
            return;
        };
        let data = Rc::new(GameData::open(&dir).expect("game data"));
        let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
        let mut loader = MenuSceneLoader::new(data, None);
        let mut scene = loop {
            if let Some(s) = loader.advance(&mut renderer).expect("load") {
                break s;
            }
        };
        let start = Instant::now();
        let dt = 1.0 / 30.0;
        let mut ru = 0;
        let mut was_full = vec![false; scene.setup.workers.len()];
        let mut unloads = 0;
        for k in 0..(3600 * 30) {
            scene.update(dt);
            let u = scene.world.universe();
            for (i, &w) in scene.setup.workers.iter().enumerate() {
                let s = u.ship(w).expect("worker lost");
                let full = s.base.resources > 0;
                if was_full[i] && !full {
                    unloads += 1;
                }
                was_full[i] = full;
            }
            if k % (30 * 300) == 0 {
                let ms = u.ship(scene.setup.mothership).expect("mothership");
                let p = ms.posinfo.position;
                let far = scene
                    .setup
                    .workers
                    .iter()
                    .filter_map(|&w| u.ship(w))
                    .map(|s| dist(s.posinfo.position, p))
                    .fold(0.0f32, f32::max);
                let now = u.base.players[0].resource_units;
                eprintln!(
                    "{:>4} s: mothership at {:?}, furthest worker {far:.0}, RUs {now} (+{}), unloads {unloads}, {} ships",
                    k / 30,
                    p,
                    now - ru,
                    u.ship_list.len()
                );
                ru = now;
                assert!(far < FIELD_REACH + 4000.0, "a worker wandered off");
                assert!(
                    dist(
                        p,
                        Vector::new(scene.centre.x, scene.centre.y, scene.centre.z)
                    ) < 50.0
                );
            }
        }
        assert!(unloads > 50, "the workers stopped unloading ({unloads})");
        eprintln!("an hour in {:.1} s", start.elapsed().as_secs_f64());
    }
}
