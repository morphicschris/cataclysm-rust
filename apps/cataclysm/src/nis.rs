//! The NIS cinematics in the game (`singlePlayerStartNis` 0x516790, the NIS task and
//! `singlePlayerNisStoppedCB` 0x516550), played with `hwc_nis` on the simulation.
//!
//! * NIS levels (table 0x88a3f4: missions 1, 4, 6, 7, 10, 12, 14..17 start with one): the
//!   campaign's opening (`nis\n01.nis` in `Mission01_NIS01`, from `singlePlayerStartGame`) and
//!   the NIS between missions (`nis\n02.nis` in `Mission04_NIS02` before mission 4, `n04` in
//!   `Mission06_NIS04` before mission 6...: `singlePlayerStartNis` with a level of its own,
//!   from the jump's state 5). The NIS plays at the origin of its level; when it stops the
//!   mission level is loaded and KAS starts. Here the NIS level gets a universe and a
//!   [`World`] of its own, built on the loading screen next to the mission's; the mission's
//!   world waits ([`NisRun::pending`]) without updating, which holds the mission start
//!   (`MissionRun`'s first step starts KAS) until the NIS ends. An NIS mothership gets the
//!   modules the fleet carries into the mission.
//! * NISlets (`G_PlayNislet`) and mission NIS (`G_PlayNis`): the mission is started with
//!   `MissionConfig::hold_nis`, so the request waits in `Game::pending_nis`; it is played in
//!   the mission's universe at `StartPointPlayer`, turned by its heading, while the mission
//!   goes on, and `MissionRun::nis_finished` sets `G_Nislet<n>Complete` / `G_Nis<n>Complete`
//!   when it stops.
//!
//! While an NIS plays the mouse and keys do nothing except Escape, which skips to its end once
//! the letterbox is in (`kbcbEscape` -> `nisGoToEnd`).

use std::sync::Arc;

use glam::Vec3;
use hwc_math::{Matrix, Vector};
use hwc_nis::data::Category;
use hwc_nis::{Fired, NisData, NisPlayer, Stage};
use hwc_render::lighting::Light;
use hwc_sim::level::{self, LevelContext, LevelState, ResourceTweaks};
use hwc_sim::mission::MissionRun;
use hwc_sim::statics::{ShipRace, StaticWorld};
use hwc_sim::{SimTweaks, Universe};

use crate::data::Assets;
use crate::world::World;

/// Which NIS is playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The campaign's opening before mission 1 (NIS 1 in `Mission01_NIS01`).
    Opening,
    /// NIS `n` in its own level before a mission (missions 4, 6, 7...).
    Level(u32),
    /// `G_PlayNis` n.
    Mission(u32),
    /// `G_PlayNislet` n.
    Nislet(u32),
}

impl Kind {
    /// The NIS plays in a level of its own, with the mission waiting behind it.
    pub fn in_own_level(self) -> bool {
        matches!(self, Kind::Opening | Kind::Level(_))
    }
}

/// An NIS in progress.
pub struct NisRun {
    pub kind: Kind,
    pub player: NisPlayer,
    pub stage: Stage,
    /// The mission's world while the opening plays in the NIS level's.
    pub pending: Option<World>,
    /// Volumes when the NIS started (restored when it stops).
    pub volumes: Option<hwc_audio::system::Volumes>,
    /// The game camera and lighting while a NISlet or mission NIS plays in the mission's
    /// universe.
    pub saved_camera: Option<hwc_render::OrbitCamera>,
    pub saved_lighting: Option<hwc_render::SceneLighting>,
    /// Both lights as the NIS set them (`numberLights` shows one or two).
    pub lights: [Light; 2],
    pub number_lights: usize,
}

/// The NIS files of a kind in a mission.
pub fn files(kind: Kind, mission: u32) -> Option<(String, String)> {
    match kind {
        Kind::Opening => hwc_nis::tables::nis_files(1),
        Kind::Level(n) | Kind::Mission(n) => hwc_nis::tables::nis_files(n),
        Kind::Nislet(n) => hwc_nis::tables::nislet_files(mission, n),
    }
}

/// Reads and loads an NIS.
pub fn load(assets: &Assets, kind: Kind, mission: u32) -> Result<NisData, String> {
    let (nis, script) = files(kind, mission).ok_or_else(|| format!("no NIS for {kind:?} in mission {mission}"))?;
    let read = |p: &str| hwc_render::AssetSource::read(assets, p).ok_or_else(|| format!("{p} not found"));
    let n = read(&nis)?;
    let s = read(&script)?;
    let d = NisData::load(&n, Some(&s)).map_err(|e| format!("{nis}: {e}"))?;
    for (line, what) in &d.ignored {
        eprintln!("note: {script}:{line}: {what} (ignored, as by the original)");
    }
    Ok(d)
}

/// The NIS position and attitude of `singlePlayerStartNis`: a point (x, y, z, heading in
/// degrees) turned about +z.
pub fn placement(point: [f32; 4]) -> (Vector, Matrix) {
    use hwc_math::fpu::{ld, st};
    let a = st(ld(point[3]) * ld(0.017_453_292f32));
    let (s, c) = (hwc_math::trig::fsin(ld(a)), hwc_math::trig::fcos(ld(a)));
    let rot = Matrix::rot_about_z(st(c), st(s));
    (Vector::new(point[0], point[1], point[2]), Matrix::IDENTITY.mul_mat(&rot))
}

fn v3(v: Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

impl NisRun {
    fn new(kind: Kind, player: NisPlayer, stage: Stage) -> NisRun {
        let white = Light {
            direction: Vec3::Z,
            ambient: Vec3::ZERO,
            diffuse: Vec3::ONE,
        };
        NisRun {
            kind,
            player,
            stage,
            pending: None,
            volumes: None,
            saved_camera: None,
            saved_lighting: None,
            lights: [white, white],
            number_lights: 0,
        }
    }

    /// Starts an NIS in `world`'s universe at `point` (`nisStart`, then
    /// `nisNewObjectCreate` for every path). `modules` are the mothership modules the fleet
    /// carries into the mission. Returns the time-0 events.
    pub fn start(
        world: &mut World,
        data: NisData,
        kind: Kind,
        point: [f32; 4],
        mission: u32,
        modules: &[hwc_sim::statics::ShipType],
    ) -> (NisRun, Vec<Fired>) {
        let (position, matrix) = placement(point);
        let eye = world.camera.eye();
        let (player, step) = NisPlayer::start(data, position, matrix, Vector::new(eye.x, eye.y, eye.z));
        let stage = Stage::create(world.universe_mut(), &player, mission, modules);
        // `lensFlareNISSunAdd`.
        world.nis_sun.add();
        for l in &stage.log {
            eprintln!("note: NIS: {l}");
        }
        let mut run = NisRun::new(kind, player, stage);
        if let Some(l) = world.lighting.lights.first() {
            run.lights = [*l, world.lighting.lights.get(1).copied().unwrap_or(*l)];
        }
        run.number_lights = world.lighting.lights.len();
        let rest = run.stage.apply(world.universe_mut(), &step.fired);
        (run, rest)
    }

    /// One NIS task step after a universe update: the player moves the objects and the
    /// camera, the stage applies the object events; the rest comes back for presentation.
    /// The second value is true when the NIS is over.
    pub fn tick(&mut self, world: &mut World) -> (Vec<Fired>, bool) {
        self.stage.forget_dead(world.universe(), &mut self.player);
        let step = self.player.tick();
        self.stage.write_poses(world.universe_mut(), &self.player, false);
        for id in self.stage.jumped(world.universe(), &self.player) {
            world.cut_trails(id);
        }
        let rest = self.stage.apply(world.universe_mut(), &step.fired);
        self.place_lights(world);
        self.update_sun(world);
        (rest, step.finished)
    }

    /// The NIS sun flare follows its source object and fades with its show and hide events.
    pub fn update_sun(&mut self, world: &mut World) {
        if let Some(p) = self.stage.sun_position(&self.player) {
            world.nis_sun.set_position(v3(p));
        }
        let enabled = world.lens_flares;
        for e in self.stage.take_sun_events() {
            world
                .nis_sun
                .set_visible(enabled, e == hwc_nis::stage::SunEvent::Show);
        }
    }

    /// The light-source objects carry the two lights (`lightSetPosition`, directional).
    pub fn place_lights(&mut self, world: &mut World) {
        for (i, path) in self.player.data.objects.iter().enumerate() {
            if !matches!(path.category, Category::Ship(_)) {
                continue;
            }
            let slot = match path.type_name.to_ascii_lowercase().as_str() {
                "nislightsource1" => 0,
                "nislightsource2" => 1,
                _ => continue,
            };
            let p = v3(self.player.objects[i].position);
            if p.length_squared() > 0.0 {
                self.lights[slot].direction = p.normalize();
            }
        }
        self.apply_lights(world);
    }

    pub fn apply_lights(&self, world: &mut World) {
        if self.number_lights == 0 {
            return;
        }
        world.lighting.lights = self.lights[..self.number_lights.min(2)].to_vec();
    }

    /// The NIS camera on the world's camera for this frame.
    pub fn view(&self, world: &mut World) {
        let c = &self.player.camera;
        world.set_view(v3(c.eye), v3(c.look_at), v3(c.up), c.fov);
        if let Some(n) = c.clip_near {
            world.camera.tweaks.clip_near = n;
        }
        if let Some(f) = c.clip_far {
            world.camera.tweaks.clip_far = f;
        }
    }
}

/// The universe of an NIS level (`singlePlayerLoadNewLevel` for `MissionNN_NISKK`), built
/// like the mission's.
pub fn level_universe(
    assets: &Assets,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    skill: i32,
    mission: u32,
    nis: u32,
) -> Result<(Universe, LevelState), String> {
    let mut u = Universe::new(statics, tweaks, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
    u.single_player = true;
    u.sp_difficulty = skill.max(0) as u32;
    hwc_sim::base::init(&mut u);
    u.base.options.mission = mission as i32;
    let ctx = LevelContext {
        single_player: true,
        mission,
        fleet_modifier: 0.0,
        resources: ResourceTweaks::load(assets),
    };
    let (dir, file) = level::mission_paths(mission, Some(nis));
    let mut state = LevelState::default();
    level::load_level(&mut u, assets, &dir, &file, &ctx, &mut state).map_err(|e| e.0)?;
    Ok((u, state))
}

/// The NIS that plays in its own level before `mission` (table 0x88a3f4 with its NIS from
/// 0x88a440), if any.
pub fn level_nis(mission: u32) -> Option<u32> {
    hwc_nis::tables::mission_nis(mission).filter(|_| hwc_nis::tables::nis_level_first(mission))
}

/// The opening (mission 1) or another NIS level: the NIS level's world with the NIS started
/// in it, and the mission's world waiting. `mission` is the started mission (its first step
/// will start KAS).
pub fn opening(
    assets: &Assets,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    mission: World,
    colors: Option<([u8; 3], [u8; 3])>,
) -> Result<(World, NisRun, Vec<Fired>), String> {
    let m = mission.run.game.config.mission;
    let nis = level_nis(m).ok_or_else(|| format!("mission {m} has no NIS level"))?;
    let kind = if m == 1 { Kind::Opening } else { Kind::Level(nis) };
    let data = load(assets, kind, m)?;
    let skill = mission.run.game.config.skill;
    // spGetHyperspaceShipTypesAttachedToType12: the modules the fleet brings.
    let mut modules = hwc_sim::nis_support::hyperspace_ship_types_attached_to_mothership(
        &mission.run.universe,
        &mission.run.game.hyper.in_hyperspace,
    );
    if modules.is_empty() {
        // The carried modules may be held by the level's clamps rather than the ships' own
        // clamp record: the fleet list says which entries are modules.
        modules = mission.run.game.fleet.iter().filter(|f| f.module).map(|f| f.ship_type).collect();
    }
    let (u, state) = level_universe(assets, statics, tweaks, skill, m, nis)?;
    let mut nis_run: MissionRun = mission.run.clone();
    nis_run.universe = u;
    nis_run.game.level = state;
    let mut world = World::new(assets.clone(), nis_run, colors)?;
    world.follow_focus = false;
    let (mut run, fired) = NisRun::start(&mut world, data, kind, [0.0; 4], m, &modules);
    // The NIS objects need their art before the first frame.
    world.load_new_art();
    run.pending = Some(mission);
    Ok((world, run, fired))
}
