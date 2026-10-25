//! The NIS cinematics in the game (`singlePlayerStartNis` 0x516790, the NIS task and
//! `singlePlayerNisStoppedCB` 0x516550), played with `hwc_nis` on the simulation.
//!
//! * NIS levels (table 0x88a3f4: missions 1, 4, 6, 7, 10, 12, 14..17 start with one): the
//!   campaign's opening (`nis\n01.nis` in `Mission01_NIS01`, from `singlePlayerStartGame`) and
//!   the NIS between missions (`nis\n02.nis` in `Mission04_NIS02` before mission 4, `n04` in
//!   `Mission06_NIS04` before mission 6...: `singlePlayerStartNis` with a level of its own,
//!   from the jump's state 5). The NIS level is loaded on its own loading screen
//!   (`hwc_sim::nis_level::NisLevelRun::start`, with the carried fleet in hyperspace) into a
//!   [`World`] of its own; the NIS plays centred on the level's start point (or a centre
//!   derelict, [`level_placement`]); when it stops, the mission is loaded on a second loading
//!   screen (`NisLevelRun::finish`) and its KAS starts. An NIS mothership gets the modules the
//!   fleet carries into the mission.
//! * NISlets (`G_PlayNislet`) and mission NIS (`G_PlayNis`): the mission is started with
//!   `MissionConfig::hold_nis`, so the request waits in `Game::pending_nis`; it is played in
//!   the mission's universe at `StartPointPlayer`, turned by its heading, while the mission
//!   goes on, and `MissionRun::nis_finished` sets `G_Nislet<n>Complete` / `G_Nis<n>Complete`
//!   when it stops.
//!
//! While an NIS plays the mouse and keys do nothing except Escape, which skips to its end once
//! the letterbox is in (`kbcbEscape` -> `nisGoToEnd`).

use glam::Vec3;
use hwc_math::{Matrix, Vector};
use hwc_nis::data::Category;
use hwc_nis::{Fired, NisData, NisPlayer, Stage};
use hwc_render::lighting::Light;
use hwc_sim::level::LevelState;
use hwc_sim::nis_level::{NisLevelRun, NisPlacement};
use hwc_sim::statics::{DerelictType, ShipType};
use hwc_sim::Universe;

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
    load_files(assets, &nis, &script)
}

/// Reads and loads the NIS `nis` with its script `script`.
pub fn load_files(assets: &Assets, nis: &str, script: &str) -> Result<NisData, String> {
    let read = |p: &str| hwc_render::AssetSource::read(assets, p).ok_or_else(|| format!("{p} not found"));
    let n = read(nis)?;
    let s = read(script)?;
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
    placement_radians(Vector::new(point[0], point[1], point[2]), a)
}

/// A position with the NIS turned about +z by `angle` radians.
fn placement_radians(position: Vector, angle: f32) -> (Vector, Matrix) {
    use hwc_math::fpu::{ld, st};
    let (s, c) = (hwc_math::trig::fsin(ld(angle)), hwc_math::trig::fcos(ld(angle)));
    let rot = Matrix::rot_about_z(st(c), st(s));
    (position, Matrix::IDENTITY.mul_mat(&rot))
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
            volumes: None,
            saved_camera: None,
            saved_lighting: None,
            lights: [white, white],
            number_lights: 0,
        }
    }

    /// Starts an NIS in `world`'s universe at `at` (`nisStart`, then
    /// `nisNewObjectCreate` for every path). `modules` are the mothership modules the fleet
    /// carries into the mission. Returns the time-0 events.
    pub fn start(
        world: &mut World,
        data: NisData,
        kind: Kind,
        at: (Vector, Matrix),
        mission: u32,
        modules: &[ShipType],
    ) -> (NisRun, Vec<Fired>) {
        let (position, matrix) = at;
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

/// `singlePlayerStartNis`' centring of an NIS level's NIS. Without a centre object the NIS
/// plays at the level's `StartPointPlayer`, turned about +z by its angle. With one (the derelict
/// of type `G_NisCentreDerelict`, else the ship of `NisCentreShip`) it plays at that object:
/// when the NIS has an object to match (`nisShipStartPosition`: the path flagged as the centre,
/// else the first of the race, type and `G_NisCentreInstance` instance) the NIS is moved and
/// turned so that object starts where the real one is; otherwise it is turned by the centre's
/// heading on the xy plane.
pub fn level_placement(data: &NisData, p: &NisPlacement, u: &Universe, level: &LevelState) -> (Vector, Matrix) {
    let instance = p.centre_derelict.and_then(|(_, i, _)| i);
    // A derelict asked for but not in the level leaves no centre at all.
    let centre = match p.centre_derelict {
        Some((t, _, found)) => found.and_then(|i| level.derelicts.get(i)).map(|d| {
            let name = usize::try_from(t).ok().and_then(DerelictType::from_index).map(DerelictType::name);
            (d.position, d.coordsys, Some(DERELICT_RACE), name)
        }),
        None => p.centre_ship.and_then(|id| u.ship(id)).map(|s| (s.posinfo.position, s.rotinfo.coordsys, None, Some(s.ship_type.name()))),
    };
    let Some((position, coordsys, race, type_name)) = centre else {
        return placement_radians(p.position, p.angle);
    };
    let heading = Vector::new(coordsys.m[6], coordsys.m[7], 0.0).normalized();
    let turn = (heading.x as f64).acos() as f32;
    let angle = if heading.y >= 0.0 { turn } else { std::f32::consts::TAU - turn };
    let Some(index) = data.centre_object(race, type_name, instance) else {
        return placement_radians(position, angle);
    };
    let (start, start_matrix) = data.start_position(index);
    let matrix = coordsys.mul_mat(&start_matrix.transpose());
    let offset = matrix.mul_vec(start);
    (Vector::new(position.x - offset.x, position.y - offset.y, position.z - offset.z), matrix)
}

/// `NSR_Derelict`: the race of the derelict paths.
const DERELICT_RACE: u32 = 11;

/// The modules the carried fleet brings into the NIS level (`spGetHyperspaceShipTypesAttachedToType12`
/// over the fleet waiting in hyperspace: the fleet's modules after `spWipeUnavailableModulesInHyperspace`,
/// in fleet order). The opening of mission 1 has no fleet in hyperspace.
pub fn level_modules(run: &NisLevelRun) -> Vec<ShipType> {
    run.carry.as_ref().map_or_else(Vec::new, |c| {
        hwc_sim::campaign::wipe_unavailable_modules(&c.fleet, run.mission)
            .iter()
            .filter(|f| f.module)
            .map(|f| f.ship_type)
            .collect()
    })
}

/// The NIS of the NIS level in `world` (`NisLevelRun::nis_files`) started at the level's
/// placement. Returns the run and its time-0 events.
pub fn start_level(world: &mut World) -> Result<(NisRun, Vec<Fired>), String> {
    let run = world.nis_level().ok_or("the world holds no NIS level")?;
    let (mission, n) = (run.mission, run.nis);
    let kind = if mission == 1 { Kind::Opening } else { Kind::Level(n) };
    let (nis, script) = run.nis_files();
    let data = load_files(world.assets(), &nis, &script)?;
    let modules = level_modules(run);
    let (position, matrix) = level_placement(&data, &run.placement, &run.universe, &run.level);
    let (run, fired) = NisRun::start(world, data, kind, (position, matrix), mission, &modules);
    // The NIS objects need their art before the first frame.
    world.load_new_art();
    Ok((run, fired))
}
