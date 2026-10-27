//! Level loading (`LevelLoad.c`): a mission's `.mission` file and the `.missphere` files it
//! names, turned into universe objects in the order the original creates them.
//!
//! The entry point is [`load_level`], the equivalent of `levelStartNext` (the single-player
//! path of `levelInit`): it detects MissionMan output (positions are then absolute and the
//! sphere rotation is ignored), applies the sensors-manager keywords (`smUniverseSize*` set the
//! world bounds), then walks the `MissionSphere` / `Lighting` / `Background` / `LensFlare` /
//! `Fog` lines in file order. Every mission sphere's file is walked line by line with
//! Cataclysm's keyword table (`Ships`, `AIPoint`, `AIPath`, `AIBox`, `AISphere`, `Resources`,
//! `MeteorStorm`, `Mine`, `Slipgate`, `Derelict`), so ships get their numbers, KAS teams their
//! place in the computer player's team list and resources their random numbers in the
//! original order.
//!
//! Asteroids and dust clouds become the simulation's resource objects ([`crate::resource`]),
//! harvestable, in the original creation order and with the original's random numbers. What
//! the simulation has no objects for yet (gas clouds, derelicts, resource volumes) is kept here
//! as plain records ([`LevelState`]), with the random numbers the original draws for them
//! consumed from the game stream; the resources are recorded there too.

use hwc_formats::script::split_line;
use hwc_kas::{kas_hash, Hash};
use hwc_math::fpu::{fild_u32, ftol_i32, ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::command;
use crate::formation;
use crate::object::ShipId;
use crate::physics;
use crate::statics::cscan::{decomma, scan, Scanned};
use crate::statics::{DataSource, DerelictType, ShipRace, ShipType};
use crate::universe::Universe;

/// `DEG_TO_RAD` as the binary multiplies it (`0.017453292f`).
pub const DEG_TO_RAD: f32 = 0.017_453_292;

/// The computer player in a single-player game (`universe.players[1]`), which owns every
/// labelled team created by a level.
pub const COMPUTER_PLAYER: usize = 1;

/// A labelled volume (`Volume`): `AISphere` or `AIBox`.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Volume {
    /// `VOLUME_SPHERE` (type 0): centre and radius (half the diameter in the file).
    Sphere { center: Vector, radius: f32 },
    /// `VOLUME_AA_BOX` (type 1): the axis-aligned extent.
    Box { min: Vector, max: Vector },
}

impl Volume {
    /// `volPointInside`.
    pub fn contains(&self, p: Vector) -> bool {
        match *self {
            Volume::Sphere { center, radius } => {
                let d = distance_squared(p, center);
                d <= ld(radius) * ld(radius)
            }
            Volume::Box { min, max } => {
                min.x <= p.x && p.x <= max.x && min.y <= p.y && p.y <= max.y && min.z <= p.z && p.z <= max.z
            }
        }
    }

    /// `volFindCenter`.
    pub fn center(&self) -> Vector {
        match *self {
            Volume::Sphere { center, .. } => center,
            Volume::Box { min, max } => Vector::new(
                st((ld(max.x) + ld(min.x)) * 0.5),
                st((ld(max.y) + ld(min.y)) * 0.5),
                st((ld(max.z) + ld(min.z)) * 0.5),
            ),
        }
    }
}

/// `aiuFindDistanceSquared` (0x41a2d0): the differences and the sum stay in the FPU, summed
/// z first.
pub fn distance_squared(a: Vector, b: Vector) -> f64 {
    let dx = ld(a.x) - ld(b.x);
    let dy = ld(a.y) - ld(b.y);
    let dz = ld(a.z) - ld(b.z);
    (dz * dz + dy * dy) + dx * dx
}

/// A labelled path (`AIPath`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct KasPath {
    pub closed: bool,
    /// Points by index (`n/total` in the file, 1-based there); unset points stay at the origin.
    pub points: Vec<Vector>,
}

/// A KAS team (`AITeam` of type `ScriptTeam`) as far as missions use it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct KasTeam {
    /// `kasLabel` (+0x58): the label hash.
    pub label: Hash,
    /// The team's ships (`shipList`, +0x1c/+0x20), in the order they joined.
    pub ships: Vec<ShipId>,
    /// `kasOrigShipsCount` (+0x70).
    pub orig_count: i32,
    /// `kasOrigShipsType` (+0x6c): the type of the last ship line that named the team.
    pub orig_type: Option<ShipType>,
    /// `newships` (+0x14), decremented for every ship a level adds.
    pub new_ships: i32,
    /// `kasTactics` (+0x74) and `kasFormation` (+0x78), set by `kasFSMCreate` (1 and 7).
    pub kas_tactics: i32,
    pub kas_formation: i32,
    /// The team's AI moves (`moves`, `curMove`, `teamDelay`): see [`crate::aimove`].
    pub moves: crate::aimove::TeamMoves,
}

impl KasTeam {
    pub fn new(label: Hash) -> Self {
        KasTeam { label, ships: Vec::new(), orig_count: 0, orig_type: None, new_ships: 0, kas_tactics: 0, kas_formation: 0, moves: crate::aimove::TeamMoves::default() }
    }
}

/// What a resource object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ResourceKind {
    /// `OBJ_AsteroidType`, with the asteroid type 0..4.
    Asteroid(u8),
    /// `OBJ_DustType`, with the cloud type.
    DustCloud(u8),
    /// `OBJ_GasType`, with the cloud type.
    GasCloud(u8),
}

/// A resource object created by a level, as the level set it up. Asteroids and dust clouds
/// also exist in the simulation ([`crate::resource`], `id`); gas clouds are records only.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LevelResource {
    pub kind: ResourceKind,
    /// The simulation's resource object.
    pub id: Option<crate::resource::ResourceId>,
    pub position: Vector,
    pub velocity: Vector,
    /// Random spin of the larger asteroids (`rotinfo.rotspeed`).
    pub rot_speed: Vector,
    pub attributes: u32,
    pub attributes_param: i16,
    /// Index into [`LevelState::resource_volumes`] when the volume regrows.
    pub volume: Option<usize>,
}

/// The layout of a resource volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum VolumeShape {
    Sphere,
    Cylinder,
    Rectangle,
}

/// A resource volume (`ResourceVolume`, 0xb0 bytes); only volumes with the regrow attribute are
/// kept by the original.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResourceVolume {
    pub shape: VolumeShape,
    pub centre: Vector,
    pub radius: f32,
    pub length: f32,
    pub roty: f32,
    pub rotz: f32,
    pub number: i32,
    pub attributes: u32,
    pub attributes_param: i16,
    /// Object type: 2 asteroid, 4 gas cloud, 5 dust cloud.
    pub obj_type: u32,
    /// `probResources[10][2]`, `cumResources[10][2]` and `sumTotal[2]` (all `uword`).
    pub prob: [[u16; 2]; 10],
    pub cum: [[u16; 2]; 10],
    pub sum: [u16; 2],
}

/// A derelict created by a level.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LevelDerelict {
    pub derelict_type: DerelictType,
    pub position: Vector,
    pub coordsys: Matrix,
    pub attributes: u32,
    pub attributes_param: i16,
    pub color_scheme: Option<u8>,
}

/// Sensors-manager tweaks set from the level file (`SMMissionTweaks`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SmTweaks {
    pub depth_cue_radius: f32,
    pub depth_cue_start_radius: f32,
    pub circle_border: i32,
    pub zoom_max: f32,
    pub zoom_min: f32,
    pub initial_distance: f32,
    pub universe_size: Vector,
    pub song_number: i32,
}

impl Default for SmTweaks {
    fn default() -> Self {
        SmTweaks {
            depth_cue_radius: 50_000.0,
            depth_cue_start_radius: 20_000.0,
            circle_border: 0,
            zoom_max: 250_000.0,
            zoom_min: 50_000.0,
            initial_distance: 0.0,
            universe_size: Vector::new(100_000.0, 100_000.0, 100_000.0),
            song_number: -1,
        }
    }
}

/// Asteroid spin tables from `resource.script` (`ASTEROID_LO_ROTATE`, `ASTEROID_HI_ROTATE`,
/// `ASTEROID_ROTATE_PROB`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResourceTweaks {
    pub lo_rotate: [f32; 5],
    pub hi_rotate: [f32; 5],
    pub rotate_prob: u32,
    /// `attrVELOCITY_TOWARDS` (0x8a2234).
    pub velocity_towards: f32,
    /// `attrRING_WIDTH_SCALE` (0x8a2248).
    pub ring_width_scale: f32,
}

impl Default for ResourceTweaks {
    fn default() -> Self {
        ResourceTweaks {
            lo_rotate: [-1.0; 5],
            hi_rotate: [1.0; 5],
            rotate_prob: 150,
            velocity_towards: 200.0,
            ring_width_scale: 0.5,
        }
    }
}

impl ResourceTweaks {
    /// Reads `resource.script` over the built-in defaults.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = ResourceTweaks::default();
        let Some(lines) = read_lines(src, "resource.script") else { return t };
        for (key, value) in lines {
            let idx = |k: &str| -> Option<usize> {
                let open = k.find('[')?;
                let name = k[open + 1..].trim_end_matches(']');
                crate::statics::AsteroidType::from_name(name).map(|a| a.index())
            };
            let base = key.split('[').next().unwrap_or("");
            if base.eq_ignore_ascii_case("ASTEROID_LO_ROTATE") {
                if let (Some(i), Some(v)) = (idx(&key), crate::statics::cscan::scan_f32(&value)) {
                    t.lo_rotate[i] = v;
                }
            } else if base.eq_ignore_ascii_case("ASTEROID_HI_ROTATE") {
                if let (Some(i), Some(v)) = (idx(&key), crate::statics::cscan::scan_f32(&value)) {
                    t.hi_rotate[i] = v;
                }
            } else if base.eq_ignore_ascii_case("ASTEROID_ROTATE_PROB") {
                if let Some(v) = crate::statics::cscan::scan_i32(&value) {
                    t.rotate_prob = v as u32;
                }
            }
        }
        t
    }
}

/// Game state the loader reads.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LevelContext {
    /// `singlePlayerGame`.
    pub single_player: bool,
    /// The current mission (`singlePlayerGameInfo.currentMission`), which selects the mothership
    /// modules.
    pub mission: u32,
    /// `spFleetModifier` (0x9101dc): 0..1, scales `numShipsUpper` and flagged resources.
    pub fleet_modifier: f32,
    pub resources: ResourceTweaks,
}

/// Everything a level created besides ships, and the KAS labels it defined.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct LevelState {
    /// The level file is MissionMan output (positions absolute).
    pub mission_man: bool,
    pub sm: SmTweaks,
    /// `kasLabelledVectorAdd`: points with their fourth component (a rotation in degrees).
    pub points: Vec<(Hash, [f32; 4])>,
    pub volumes: Vec<(Hash, Volume)>,
    pub paths: Vec<(Hash, KasPath)>,
    /// The computer player's KAS teams, in team-list order.
    pub teams: Vec<KasTeam>,
    pub resources: Vec<LevelResource>,
    /// Regrowing resource volumes (`universe.ResourceVolumeList`).
    pub resource_volumes: Vec<ResourceVolume>,
    pub derelicts: Vec<LevelDerelict>,
    /// Module ships clamped to their host (module, host).
    pub clamps: Vec<(ShipId, ShipId)>,
    /// `player->PlayerMothership` per player.
    pub motherships: Vec<Option<ShipId>>,
    /// `player->resourceUnits` per player as the level set them.
    pub resource_units: Vec<i32>,
    /// Lines whose keyword is recognised but not modelled (`MeteorStorm`, `Mine`, `Slipgate`,
    /// `Lighting`, ...), as `(keyword, value)`.
    pub unmodelled: Vec<(String, String)>,
    /// Problems the original would have reported.
    pub warnings: Vec<String>,
    /// Label texts seen (teams, points, volumes, paths), for logs.
    pub names: Vec<(Hash, String)>,
    /// The last slipgate created without a partner yet (0xadb61c): gates pair in file order.
    pub unpaired_slipgate: Option<ShipId>,
}

impl LevelState {
    pub fn point(&self, h: Hash) -> Option<[f32; 4]> {
        self.points.iter().find(|p| p.0 == h).map(|p| p.1)
    }

    pub fn volume(&self, h: Hash) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.0 == h).map(|v| &v.1)
    }

    pub fn path(&self, h: Hash) -> Option<&KasPath> {
        self.paths.iter().find(|p| p.0 == h).map(|p| &p.1)
    }

    pub fn team(&self, h: Hash) -> Option<&KasTeam> {
        self.teams.iter().find(|t| t.label == h)
    }

    pub fn team_mut(&mut self, h: Hash) -> Option<&mut KasTeam> {
        self.teams.iter_mut().find(|t| t.label == h)
    }

    /// The team of this label, created at the end of the team list when missing (`aitCreate`).
    pub fn team_or_create(&mut self, h: Hash) -> &mut KasTeam {
        let i = match self.teams.iter().position(|t| t.label == h) {
            Some(i) => i,
            None => {
                self.teams.push(KasTeam::new(h));
                self.teams.len() - 1
            }
        };
        &mut self.teams[i]
    }

    fn name(&mut self, label: &str) -> Hash {
        let h = kas_hash(label);
        if !self.names.iter().any(|n| n.0 == h) {
            self.names.push((h, label.to_string()));
        }
        h
    }

    fn set_mothership(&mut self, player: usize, id: ShipId) {
        if self.motherships.len() <= player {
            self.motherships.resize(player + 1, None);
        }
        self.motherships[player] = Some(id);
    }
}

/// Why a level could not be loaded.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LevelError(pub String);

impl std::fmt::Display for LevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LevelError {}

/// The directory and file names of a single-player level, as `singlePlayerLoadNewLevel` builds
/// them: `SinglePlayer\MissionNN\MissionNN.mission`, or for the NIS level that precedes a
/// mission `SinglePlayer\MissionNN_NISMM\MissionNN_NISMM.mission`.
pub fn mission_paths(mission: u32, nis: Option<u32>) -> (String, String) {
    match nis {
        None => (format!("SinglePlayer\\Mission{mission:02}\\"), format!("Mission{mission:02}.mission")),
        Some(n) => (
            format!("SinglePlayer\\Mission{mission:02}_NIS{n:02}\\"),
            format!("Mission{mission:02}_NIS{n:02}.mission"),
        ),
    }
}

/// Reads a text file as `(keyword, value)` lines with the engine's line rules.
pub fn read_lines(src: &dyn DataSource, path: &str) -> Option<Vec<(String, String)>> {
    let bytes = src.read(path)?;
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    Some(
        text.split('\n')
            .filter_map(|l| split_line(l.trim_end_matches('\r')))
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

/// `isLevelMissionManGenerated`: the first line starts with `; MissionMan`.
pub fn is_mission_man(src: &dyn DataSource, path: &str) -> bool {
    src.read(path).is_some_and(|b| b.starts_with(b"; MissionMan"))
}

/// `StrToObjType` for the resource object names.
fn resource_obj_type(s: &str) -> Option<u32> {
    if s.eq_ignore_ascii_case("Asteroid") {
        Some(2)
    } else if s.eq_ignore_ascii_case("GasCloud") {
        Some(4)
    } else if s.eq_ignore_ascii_case("DustCloud") {
        Some(5)
    } else if s.eq_ignore_ascii_case("Nebula") {
        Some(3)
    } else {
        None
    }
}

/// `StrToTypeOfFormation` (table at 0x875858); `None` for unknown names such as
/// `NULL_FORMATION`.
pub fn formation_from_name(s: &str) -> Option<u32> {
    const NAMES: [&str; 11] = [
        "DELTA_FORMATION",
        "BROAD_FORMATION",
        "DELTA3D_FORMATION",
        "CLAW_FORMATION",
        "WALL_FORMATION",
        "SPHERE_FORMATION",
        "CUSTOM_FORMATION",
        "NO_FORMATION",
        "SAME_FORMATION",
        "FORCEFIELD_WALL_FORMATION",
        "FORCEFIELD_SPHERE_FORMATION",
    ];
    NAMES.iter().position(|n| n.eq_ignore_ascii_case(s)).map(|i| i as u32)
}

/// Ship types whose creation makes the ship its player's mothership (`scriptSetShipsCB`):
/// sMothership, bMothership, kCarrier, tCarrier, trFloatingCity.
pub fn is_mothership_type(t: ShipType) -> bool {
    matches!(t, ShipType::sMothership | ShipType::bMothership | ShipType::kCarrier | ShipType::tCarrier | ShipType::trFloatingCity)
}

/// `univMissionModuleTypesGet` for a single-player game: the modules a new sMothership comes
/// with in this mission.
pub fn mission_module_types(mission: u32) -> &'static [ShipType] {
    use ShipType::*;
    match mission {
        1 | 2 => &[sMothershipEnginesLower, sMothershipEnginesUpper, sMothershipCommand, sMothershipLowerSpine, sMothershipOreCanArmour, sMothershipEngineering],
        3 => &[sMothershipEnginesLower, sMothershipEnginesUpper, sMothershipCommand, sMothershipLowerSpine, sMothershipEngineering],
        _ => &[sMothershipEnginesUpper, sMothershipCommand, sMothershipEngineering],
    }
}

/// `univRotateObjYaw`: rotate about the ship's own X axis.
pub fn rotate_yaw(u: &mut Universe, id: ShipId, angle: f32) {
    rotate_ship(u, id, angle, false);
}

/// `univRotateObjPitch`: rotate about the ship's own Y axis.
pub fn rotate_pitch(u: &mut Universe, id: ShipId, angle: f32) {
    rotate_ship(u, id, angle, true);
}

fn rotate_ship(u: &mut Universe, id: ShipId, angle: f32, pitch: bool) {
    let (s, c) = (st(fsin(ld(angle))), st(fcos(ld(angle))));
    let rot = if pitch { Matrix::rot_about_y(c, s) } else { Matrix::rot_about_x(c, s) };
    let statics = u.statics.clone();
    let Some(ship) = u.ships.get_mut(id) else { return };
    ship.rotinfo.coordsys = ship.rotinfo.coordsys.mul_mat(&rot);
    if let Some(stat) = statics.ship(ship.ship_type) {
        physics::update_obj_rot_info(ship, stat);
    }
}

/// `univCreateShip` as levels, NIS and KAS use it ([`crate::base::spawn_ship`]): a new ship
/// and, for a mothership, its mission modules clamped to it (created by the base layer for the
/// mission in `mission`).
pub fn create_ship(
    u: &mut Universe,
    state: &mut LevelState,
    mission: u32,
    ship_type: ShipType,
    player: usize,
    position: Vector,
) -> Result<ShipId, LevelError> {
    let _ = state;
    if u.single_player {
        u.base.options.mission = mission as i32;
    }
    u.add_ship(ship_type, player, position, None).map_err(|e| LevelError(format!("{ship_type}: {e}")))
}

/// `clampObjToObj` for a mothership module: the module takes the host's position and
/// orientation and is clamped to it ([`crate::clamp`]).
pub fn attach_module(u: &mut Universe, state: &mut LevelState, module: ShipId, host: ShipId) {
    let _ = state;
    let Some(h) = u.ships.get(host) else { return };
    let (pos, vel, coordsys) = (h.posinfo.position, h.posinfo.velocity, h.rotinfo.coordsys);
    if let Some(m) = u.ships.get_mut(module) {
        m.posinfo.position = pos;
        m.posinfo.velocity = vel;
        m.rotinfo.coordsys = coordsys;
        m.flags |= crate::base::SOF_MODULE;
    }
    crate::clamp::clamp_obj_to_obj(u, module, host);
}

/// `updateClampedObjectNoPhys` for every clamped module: copy the host's position, velocity
/// and orientation.
pub fn update_clamped(u: &mut Universe, state: &LevelState) {
    let statics = u.statics.clone();
    for &(m, h) in &state.clamps {
        let Some(host) = u.ships.get(h) else { continue };
        let (pos, vel, coordsys) = (host.posinfo.position, host.posinfo.velocity, host.rotinfo.coordsys);
        if let Some(ms) = u.ships.get_mut(m) {
            ms.posinfo.position = pos;
            ms.posinfo.velocity = vel;
            ms.rotinfo.coordsys = coordsys;
            if let Some(stat) = statics.ship(ms.ship_type) {
                physics::update_obj_rot_info(ms, stat);
            }
        }
    }
}

/// Where a mission sphere puts its contents (`MissionSphereInfo`).
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
struct SphereInfo {
    player: usize,
    position: Vector,
    rotz: f32,
}

impl SphereInfo {
    /// Position of a sphere entry: absolute for MissionMan files, else rotated by the sphere's
    /// angle and offset by its position.
    fn place(&self, mission_man: bool, p: Vector) -> Vector {
        if mission_man {
            return p;
        }
        let m = Matrix::rot_about_z(st(fcos(ld(self.rotz))), st(fsin(ld(self.rotz))));
        let r = m.mul_vec(p);
        Vector::new(st(ld(r.x) + ld(self.position.x)), st(ld(r.y) + ld(self.position.y)), st(ld(r.z) + ld(self.position.z)))
    }
}

/// `levelStartNext` (0x49f2e0): loads `dir`+`file` into the universe.
pub fn load_level(u: &mut Universe, src: &dyn DataSource, dir: &str, file: &str, ctx: &LevelContext, state: &mut LevelState) -> Result<(), LevelError> {
    let path = format!("{dir}{file}");
    let lines = read_lines(src, &path).ok_or_else(|| LevelError(format!("{path}: not found")))?;
    state.mission_man = is_mission_man(src, &path);
    if state.motherships.len() < u.players.len() {
        state.motherships.resize(u.players.len(), None);
    }
    if state.resource_units.len() < u.players.len() {
        state.resource_units.resize(u.players.len(), 0);
    }

    // SetSMMissionTweaks: defaults, then the keywords anywhere in the file.
    let mut sm = SmTweaks { universe_size: state.sm.universe_size, ..SmTweaks::default() };
    for (k, v) in &lines {
        let f = || crate::statics::cscan::scan_f32(v);
        let i = || crate::statics::cscan::scan_i32(v);
        match k.to_ascii_lowercase().as_str() {
            "smdepthcueradius" => sm.depth_cue_radius = f().unwrap_or(sm.depth_cue_radius),
            "smdepthcuestartradius" => sm.depth_cue_start_radius = f().unwrap_or(sm.depth_cue_start_radius),
            "smcircleborder" => sm.circle_border = i().unwrap_or(sm.circle_border),
            "smzoommax" => sm.zoom_max = f().unwrap_or(sm.zoom_max),
            "smzoommin" => sm.zoom_min = f().unwrap_or(sm.zoom_min),
            "sminitialdistance" => sm.initial_distance = f().unwrap_or(sm.initial_distance),
            "smuniversesizex" => sm.universe_size.x = f().unwrap_or(sm.universe_size.x),
            "smuniversesizey" => sm.universe_size.y = f().unwrap_or(sm.universe_size.y),
            "smuniversesizez" => sm.universe_size.z = f().unwrap_or(sm.universe_size.z),
            "songnumber" => sm.song_number = i().unwrap_or(sm.song_number),
            _ => {}
        }
    }
    // The world bounds are the universe size plus 5000 on every axis.
    u.world_bounds = Vector::new(st(ld(sm.universe_size.x) + 5000.0), st(ld(sm.universe_size.y) + 5000.0), st(ld(sm.universe_size.z) + 5000.0));
    state.sm = sm;

    ship_exclusions(u, &lines, ctx);
    for (k, v) in &lines {
        match k.to_ascii_lowercase().as_str() {
            "missionsphere" => mission_sphere(u, src, dir, v, ctx, state)?,
            "lighting" | "background" | "lensflare" | "fog" => state.unmodelled.push((k.clone(), v.clone())),
            _ => {}
        }
    }
    Ok(())
}

/// levelPreInit: everything is needed, then the level's ExcludeShips / IncludeShips in file
/// order (`MissionPreloadScriptTable`).
fn ship_exclusions(u: &mut Universe, lines: &[(String, String)], ctx: &LevelContext) {
    flag_everything_needed(u);
    for (k, v) in lines {
        match k.to_ascii_lowercase().as_str() {
            "excludeships" => exclude_ship(u, v, false, ctx),
            "includeships" => exclude_ship(u, v, true, ctx),
            _ => {}
        }
    }
}

/// The level's ExcludeShips / IncludeShips again, for a saved game: the statics they change
/// are not saved, and a loaded game links to statics fresh from the game files.
pub fn reapply_ship_exclusions(u: &mut Universe, src: &dyn DataSource, dir: &str, file: &str, ctx: &LevelContext) -> Result<(), LevelError> {
    let path = format!("{dir}{file}");
    let lines = read_lines(src, &path).ok_or_else(|| LevelError(format!("{path}: not found")))?;
    ship_exclusions(u, &lines, ctx);
    Ok(())
}

/// `universeFlagEverythingNeeded`: every ship type's statics are needed.
pub fn flag_everything_needed(u: &mut Universe) {
    let w = std::sync::Arc::make_mut(&mut u.statics);
    for s in w.ships.iter_mut().flatten() {
        s.info_needed = true;
    }
}

/// `SetInfoNeededForShipAndRelatedStaticInfo` for one type (the fleet in hyperspace is
/// flagged this way after the level's exclusions).
pub fn flag_ship_needed(u: &mut Universe, t: ShipType, needed: bool) {
    let w = std::sync::Arc::make_mut(&mut u.statics);
    if let Some(s) = w.ships.get_mut(t.index()).and_then(Option::as_mut) {
        s.info_needed = needed;
    }
}

/// `rmEnableShip` (0x4ef090): a Somtaaw (race 0) or Beast (race 1) type's dependency set is
/// enabled (mode 0) or disabled (mode -2) in that race's research tables. Only the race's own
/// types count, plus the Super Acolyte (`dAcolyte`) under race 0, which the multiplayer table
/// keeps disabled whatever is asked.
pub fn enable_ship(u: &mut Universe, race: usize, t: ShipType, enabled: bool) {
    let i = t.index();
    if !(i < 0x3c || t == ShipType::dAcolyte) {
        return;
    }
    let mode = if enabled { 0 } else { -2 };
    let w = std::sync::Arc::make_mut(&mut u.statics);
    match race {
        0 => {
            w.research.single_player_somtaaw.ship_mode.insert(t, mode);
            let mp = if t == ShipType::dAcolyte { -2 } else { mode };
            w.research.multiplayer_somtaaw.ship_mode.insert(t, mp);
        }
        1 => {
            w.research.beast.ship_mode.insert(t, mode);
        }
        _ => {}
    }
}

/// `llExcludeShip` (0x49e0e0) for `ExcludeShips` / `IncludeShips race,<type|All>`. In single
/// player the race names the player's side: `RACE_SECT` is the player's race and `RACE_BEAST`
/// the other one of the two. A single type also goes through `rmEnableShip`; `All` only flags
/// the race's types needed or not (`universeFlagRaceNeeded`).
fn exclude_ship(u: &mut Universe, field: &str, needed: bool, ctx: &LevelContext) {
    let mut parts = field.split(',').map(str::trim);
    let Some(race) = parts.next().and_then(ShipRace::from_name) else { return };
    let player_race = u.players.first().map_or(0, |p| p.race.index());
    let mut r = race.index();
    if ctx.single_player {
        r = match race {
            ShipRace::Somtaaw => player_race,
            ShipRace::Beast => usize::from(player_race == 0),
            _ => r,
        };
    }
    let Some(what) = parts.next() else { return };
    if what.eq_ignore_ascii_case("All") {
        let Some(mapped) = ShipRace::from_index(r) else { return };
        for i in crate::research::race_types(mapped) {
            if let Some(t) = ShipType::from_index(i) {
                flag_ship_needed(u, t, needed);
            }
        }
        return;
    }
    let Some(t) = ShipType::from_name(what) else { return };
    flag_ship_needed(u, t, needed);
    enable_ship(u, r, t, needed);
}

/// Whether the construction manager offers `t` at `factory` (`cmUpdateShipAvailable`): the
/// factory's build table lists it, an acquire-first type (Beast) has been acquired, the
/// player's research allows it (`rmCanBuildShip`, which honours `rmEnableShip`), and the
/// level loaded its statics. Whether a job can be queued right now (unit caps, support units,
/// a free build point) is a separate question ([`crate::construction::queue_build`]).
pub fn construction_offers(u: &Universe, factory: ShipId, t: ShipType) -> bool {
    let Some(f) = u.ships.get(factory) else { return false };
    let Some(fstat) = u.statics.ship(f.ship_type) else { return false };
    let table = crate::construction::build_table(fstat.ship_race, fstat.can_build_big_ships);
    let Some(&(flag, _)) = table.iter().find(|&&(flag, ty)| flag != 0 && usize::from(ty) == t.index()) else { return false };
    let player = f.player;
    if flag == 2 && !u.base.players.get(player).is_some_and(|p| p.acquired[t.index()]) {
        return false;
    }
    let at_mothership = f.ship_type == ShipType::sMothership;
    if !crate::research::can_build_ship(u, player, t, at_mothership) {
        return false;
    }
    u.statics.ship(t).is_some_and(|s| s.info_needed)
}

/// The types the construction manager offers at `factory`, in its list order.
pub fn construction_list(u: &Universe, factory: ShipId) -> Vec<ShipType> {
    let Some(fstat) = u.ships.get(factory).and_then(|f| u.statics.ship(f.ship_type)) else { return Vec::new() };
    crate::construction::build_table(fstat.ship_race, fstat.can_build_big_ships)
        .iter()
        .filter(|&&(flag, _)| flag != 0)
        .filter_map(|&(_, ty)| ShipType::from_index(usize::from(ty)))
        .filter(|&t| construction_offers(u, factory, t))
        .collect()
}

/// `scriptSetMissionSphereCB` (0x49da80), single-player path.
fn mission_sphere(u: &mut Universe, src: &dyn DataSource, dir: &str, field: &str, ctx: &LevelContext, state: &mut LevelState) -> Result<(), LevelError> {
    let f = decomma(field);
    let v = if state.mission_man { scan(&f, "%d %s %s %f %f %f %f %f %s %d") } else { scan(&f, "%d %s %f %f %f %f %f %s %d") };
    let off = usize::from(state.mission_man);
    let player = v.first().and_then(Scanned::int).unwrap_or(-1);
    let fl = |i: usize| v.get(i + off).and_then(Scanned::float).unwrap_or(0.0);
    let position = Vector::new(fl(2), fl(3), fl(4));
    let rot_deg = fl(6);
    let contents = v.get(7 + off).and_then(Scanned::str).map(str::to_string).ok_or_else(|| LevelError(format!("bad MissionSphere: {field}")))?;
    let rus = v.get(8 + off).and_then(Scanned::int).unwrap_or(0);
    let rotz = if rot_deg < 1000.0 { st(ld(rot_deg) * ld(DEG_TO_RAD)) } else { st(position.y.atan2(position.x) as f64 - std::f64::consts::FRAC_PI_2) };
    // Single player: every sphere's contents belong to the computer player.
    let owner = if ctx.single_player { COMPUTER_PLAYER } else { player.max(0) as usize };
    if player >= 0 && (player as usize) < state.resource_units.len() {
        state.resource_units[player as usize] = rus;
    }
    let info = SphereInfo { player: owner, position, rotz };
    let path = format!("{dir}{contents}");
    let lines = read_lines(src, &path).ok_or_else(|| LevelError(format!("{path}: not found")))?;
    for (k, value) in lines {
        match k.to_ascii_lowercase().as_str() {
            "ships" => ships_line(u, &value, info, ctx, state)?,
            "aipoint" => ai_point(&value, state),
            "aipath" => ai_path(&value, state),
            "aibox" => ai_box(&value, state),
            "aisphere" => ai_sphere(&value, state),
            "resources" => resources_line(u, src, dir, &value, info, ctx, state),
            "derelict" => derelict_line(&value, info, state),
            "mine" => {
                // `scriptSetMineCB`: x, y, z and an angle (commas allowed); the mine goes at
                // the raw coordinates.
                let v: Vec<f32> = crate::statics::cscan::decomma(&value).split_whitespace().filter_map(crate::statics::cscan::scan_f32).collect();
                if v.len() >= 3 {
                    crate::missile::add_level_mine(u, Vector::new(v[0], v[1], v[2]));
                }
            }
            "slipgate" => slipgate_line(u, &value, info, ctx, state)?,
            "meteorstorm" => state.unmodelled.push((k.clone(), value.clone())),
            _ => {}
        }
    }
    Ok(())
}

/// `scriptSetSlipgateCB` (0x49c680): `x, y, z, n/m, angle, type`. The gate belongs to the
/// sphere's player, is yawed by the angle, cannot be targeted or bumped, and is disabled
/// (reason 6); consecutive gates are each other's partners.
fn slipgate_line(u: &mut Universe, field: &str, info: SphereInfo, ctx: &LevelContext, state: &mut LevelState) -> Result<(), LevelError> {
    let f = decomma(field);
    let v = scan(&f, "%f %f %f %d/%d %f %s");
    let fl = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
    let Some(type_s) = v.get(6).and_then(Scanned::str) else {
        return Err(LevelError(format!("Bad Slipgate point definition: {field}")));
    };
    let ship_type = ShipType::from_name(type_s).ok_or_else(|| LevelError(format!("Bad shiptype in {field}")))?;
    let mut angle = st(ld(fl(5)) * ld(DEG_TO_RAD));
    let p = info.place(state.mission_man, Vector::new(fl(0), fl(1), fl(2)));
    if !state.mission_man {
        angle = st(ld(angle) + ld(info.rotz));
    }
    if u.statics.ship(ship_type).is_none() {
        state.warnings.push(format!("no statics for {ship_type}; slipgate skipped"));
        return Ok(());
    }
    let id = create_ship(u, state, ctx.mission, ship_type, info.player, p)?;
    rotate_yaw(u, id, angle);
    match state.unpaired_slipgate.take() {
        Some(other) => {
            crate::ships::slipgate::set_partner(u, id, other);
            crate::ships::slipgate::set_partner(u, other, id);
        }
        None => state.unpaired_slipgate = Some(id),
    }
    if let Some(s) = u.ships.get_mut(id) {
        s.flags = (s.flags & !crate::object::sof::TARGETABLE) | crate::object::sof::NOT_BUMPABLE;
        crate::base::set_disable_reason(s, true, 6);
    }
    Ok(())
}

/// `scriptSetShipsCB` (0x49ca50).
fn ships_line(u: &mut Universe, field: &str, info: SphereInfo, ctx: &LevelContext, state: &mut LevelState) -> Result<(), LevelError> {
    let field = field.strip_prefix('?').unwrap_or(field);
    let field = decomma(field);
    let mut player = info.player;
    let mut team: Option<Hash> = None;
    let mut start = 0usize;
    if field.as_bytes().first().is_some_and(|c| c.is_ascii_alphabetic()) {
        let label = field.split_ascii_whitespace().next().unwrap_or("");
        start = label.len();
        player = COMPUTER_PLAYER;
        let h = state.name(label);
        state.team_or_create(h);
        team = Some(h);
    }
    let v = scan(&field[start..], "%f %f %f %f %s %s %d %s %d %d %d");
    let fl = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
    let pos = Vector::new(fl(0), fl(1), fl(2));
    let mut rot = st(ld(fl(3)) * ld(DEG_TO_RAD));
    let race_s = v.get(4).and_then(Scanned::str).unwrap_or("");
    let type_s = v.get(5).and_then(Scanned::str).unwrap_or("");
    let mut num = v.get(6).and_then(Scanned::int).unwrap_or(0).max(0) as u32;
    let misc = v.get(7).and_then(Scanned::str).unwrap_or("").to_string();
    let attributes = v.get(8).and_then(Scanned::int).unwrap_or(0) as u32;
    let attributes_param = v.get(9).and_then(Scanned::int).unwrap_or(0) as i16;
    let upper = if ctx.single_player { v.get(10).and_then(Scanned::int).unwrap_or(0).max(0) as u32 } else { 0 };
    if upper != 0 && ctx.fleet_modifier != 0.0 && upper > num {
        let diff = ld(fild_u32(upper - num) as f32) * ld(ctx.fleet_modifier);
        num += ftol_i32(diff + 0.5).max(0) as u32;
    }
    let race = ShipRace::from_name(race_s).ok_or_else(|| LevelError(format!("bad ship race in {field}")))?;
    let ship_type = ShipType::from_name(type_s).ok_or_else(|| LevelError(format!("bad ship type in {field}")))?;
    let _ = race;

    let mut parade = false;
    let mut use_as_mothership = false;
    let mut color: Option<i32> = None;
    let mut formation: Option<u32> = None;
    for tok in misc.split(['|', '?', ',']).filter(|t| !t.is_empty()) {
        if tok.eq_ignore_ascii_case("PARADE_FORMATION") {
            parade = true;
        } else if tok.eq_ignore_ascii_case("UseAsMothership") {
            use_as_mothership = true;
        } else if let Some(rest) = tok.strip_prefix("colorScheme") {
            if let Some(Scanned::Int(c)) = scan(rest, "(%d)").first() {
                color = Some(*c);
            }
        } else if !tok.starts_with(['0', '?']) {
            formation = formation_from_name(tok);
        }
    }
    if let Some(h) = team {
        let t = state.team_or_create(h);
        t.orig_count += num as i32;
        t.orig_type = Some(ship_type);
    }
    if u.statics.ship(ship_type).is_none() {
        state.warnings.push(format!("no statics for {ship_type}; {num} ship(s) skipped"));
        return Ok(());
    }

    let finish = |u: &mut Universe, state: &mut LevelState, id: ShipId| {
        if let Some(s) = u.ships.get_mut(id) {
            s.attributes = attributes;
            if let Some(c) = color {
                s.color_scheme = c as usize & 0xff;
            }
            if s.color_scheme > 7 {
                s.color_scheme = 7;
            }
        }
        let _ = attributes_param;
        if let Some(h) = team {
            let t = state.team_or_create(h);
            t.ships.push(id);
            t.new_ships -= 1;
        }
    };

    if num == 1 {
        let p = info.place(state.mission_man, pos);
        if !state.mission_man {
            rot = st(ld(rot) + ld(info.rotz));
        }
        let id = create_ship(u, state, ctx.mission, ship_type, player, p)?;
        finish(u, state, id);
        rotate_yaw(u, id, rot);
        if is_mothership_type(ship_type) || use_as_mothership {
            state.set_mothership(player, id);
        } else if parade {
            state.warnings.push(format!("{ship_type}: parade formation not modelled"));
        }
    } else if num > 1 {
        if parade {
            state.warnings.push(format!("{ship_type} x{num}: parade formation not modelled"));
        }
        let mut p = info.place(state.mission_man, pos);
        if !state.mission_man && !parade {
            rot = st(ld(rot) + ld(info.rotz));
        }
        let mut sel = Vec::with_capacity(num as usize);
        for _ in 0..num {
            let id = create_ship(u, state, ctx.mission, ship_type, player, p)?;
            finish(u, state, id);
            if !parade {
                rotate_yaw(u, id, rot);
            }
            sel.push(id);
            let r = u.statics.ship(ship_type).map_or(0.0, |s| s.header.coll.coll_sphere_size);
            p.z = st(ld(r) + ld(r) + ld(p.z));
        }
        if let Some(fm) = formation.filter(|_| !parade) {
            if fm <= formation::kind::CUSTOM {
                command::cl_formation(u, &sel, fm);
            } else {
                state.warnings.push(format!("{ship_type} x{num}: formation {fm} not modelled"));
            }
        }
    }
    Ok(())
}

/// `scriptSetAIPointCB`.
fn ai_point(field: &str, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %f");
    if v.len() < 4 {
        state.warnings.push(format!("bad AIPoint definition: {field}"));
        return;
    }
    let f = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
    let label = v[0].str().unwrap_or("");
    let h = state.name(label);
    state.points.push((h, [f(1), f(2), f(3), f(4)]));
}

/// `scriptSetAIPathCB`.
fn ai_path(field: &str, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %s %d/%d");
    if v.len() != 7 {
        state.warnings.push(format!("bad AIPath point definition: {field}"));
        return;
    }
    let f = |i: usize| v[i].float().unwrap_or(0.0);
    let h = state.name(v[0].str().unwrap_or(""));
    let closed = v[4].str().is_some_and(|s| s.eq_ignore_ascii_case("closed"));
    let index = v[5].int().unwrap_or(1) - 1;
    let total = v[6].int().unwrap_or(0).max(0) as usize;
    if state.path(h).is_none() {
        state.paths.push((h, KasPath { closed, points: vec![Vector::ZERO; total] }));
    }
    let path = &mut state.paths.iter_mut().find(|p| p.0 == h).expect("path").1;
    if index >= 0 && (index as usize) < path.points.len() {
        path.points[index as usize] = Vector::new(f(1), f(2), f(3));
    } else {
        state.warnings.push(format!("AIPath point out of range: {field}"));
    }
}

/// `scriptSetAIBoxCB`: centre and size to extent.
fn ai_box(field: &str, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %f %f %f");
    if v.len() != 7 {
        state.warnings.push(format!("bad AIBox definition: {field}"));
        return;
    }
    let f = |i: usize| ld(v[i].float().unwrap_or(0.0));
    let (x, y, z, w, h, d) = (f(1), f(2), f(3), f(4), f(5), f(6));
    let min = Vector::new(st(x - w * 0.5), st(y - h * 0.5), st(z - d * 0.5));
    let max = Vector::new(st(w * 0.5 + x), st(h * 0.5 + y), st(d * 0.5 + z));
    let h = state.name(v[0].str().unwrap_or(""));
    state.volumes.push((h, Volume::Box { min, max }));
}

/// `scriptSetAISphereCB`: the file gives the diameter.
fn ai_sphere(field: &str, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %f");
    if v.len() != 5 {
        state.warnings.push(format!("bad AISphere definition: {field}"));
        return;
    }
    let f = |i: usize| v[i].float().unwrap_or(0.0);
    let center = Vector::new(f(1), f(2), f(3));
    let h = state.name(v[0].str().unwrap_or(""));
    state.volumes.push((h, Volume::Sphere { center, radius: st(ld(f(4)) * 0.5) }));
}

/// `scriptSetDerelictCB`.
fn derelict_line(field: &str, info: SphereInfo, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %f %f %d %d %s");
    let f = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
    let Some(t) = v.first().and_then(Scanned::str).and_then(DerelictType::from_name) else {
        state.warnings.push(format!("bad derelict type in {field}"));
        return;
    };
    let pitch = st(-(ld(f(4)) * ld(DEG_TO_RAD)));
    let mut yaw = st(ld(f(5)) * ld(DEG_TO_RAD));
    let pos = info.place(state.mission_man, Vector::new(f(1), f(2), f(3)));
    if !state.mission_man {
        yaw = st(ld(yaw) + ld(info.rotz));
    }
    let mut m = crate::universe::DEFAULT_SHIP_MATRIX;
    m = m.mul_mat(&Matrix::rot_about_y(st(fcos(ld(pitch))), st(fsin(ld(pitch)))));
    m = m.mul_mat(&Matrix::rot_about_x(st(fcos(ld(yaw))), st(fsin(ld(yaw)))));
    let color = v.get(8).and_then(Scanned::str).and_then(|s| s.strip_prefix("colorScheme")).and_then(|r| scan(r, "(%d)").first().and_then(Scanned::int));
    state.derelicts.push(LevelDerelict {
        derelict_type: t,
        position: pos,
        coordsys: m,
        attributes: v.get(6).and_then(Scanned::int).unwrap_or(0) as u32,
        attributes_param: v.get(7).and_then(Scanned::int).unwrap_or(0) as i16,
        color_scheme: color.map(|c| c as u8),
    });
}

/// `GetResourceDistribution`: the `.dist` file's relative frequencies and their running sums.
fn resource_distribution(src: &dyn DataSource, dir: &str, file: &str, obj_type: u32, vol: &mut ResourceVolume, state: &mut LevelState) {
    let names: &[&str] = match obj_type {
        2 => &["Asteroid0", "Asteroid1", "Asteroid2", "Asteroid3", "Asteroid4"],
        4 => &["GasCloud0", "GasCloud1"],
        _ => &["DustCloud0", "DustCloud1", "DustCloud2", "DustCloud3"],
    };
    vol.prob = [[0; 2]; 10];
    if let Some(lines) = read_lines(src, &format!("{dir}{file}")) {
        for (k, v) in lines {
            if let Some(i) = names.iter().position(|n| n.eq_ignore_ascii_case(&k)) {
                let s = scan(&decomma(&v), "%d %d");
                let a = s.first().and_then(Scanned::int).unwrap_or(-1);
                let b = s.get(1).and_then(Scanned::int).unwrap_or(a);
                vol.prob[i] = [a as u16, b as u16];
            }
        }
    } else {
        state.warnings.push(format!("{dir}{file}: distribution not found"));
    }
    for j in 0..2 {
        let mut cum: u16 = 0;
        for i in 0..10 {
            cum = cum.wrapping_add(vol.prob[i][j]);
            vol.cum[i][j] = cum;
        }
        if cum == 0 {
            state.warnings.push(format!("no distribution set in {file}"));
        }
        vol.sum[j] = cum;
    }
}

/// `scriptSetResourcesCB` (0x49be10).
fn resources_line(u: &mut Universe, src: &dyn DataSource, dir: &str, field: &str, info: SphereInfo, ctx: &LevelContext, state: &mut LevelState) {
    let v = scan(&decomma(field), "%s %f %f %f %s %s %d %f %f %f %f %d %d");
    let f = |i: usize| v.get(i).and_then(Scanned::float).unwrap_or(0.0);
    let s = |i: usize| v.get(i).and_then(Scanned::str).unwrap_or("").to_string();
    let dist = s(5);
    if dist == "0" {
        state.warnings.push(format!("no distribution specified in {field}"));
    }
    let Some(obj_type) = resource_obj_type(&s(0)) else {
        state.warnings.push(format!("unknown resource type in {field}"));
        return;
    };
    let mut number = v.get(6).and_then(Scanned::int).unwrap_or(0);
    let radius = f(7);
    let length = f(8);
    let roty = f(9);
    let mut rotz = st(ld(f(10)) * ld(DEG_TO_RAD));
    let attributes = v.get(11).and_then(Scanned::int).unwrap_or(0) as u32;
    let attributes_param = v.get(12).and_then(Scanned::int).unwrap_or(0) as i16;
    let centre = info.place(state.mission_man, Vector::new(f(1), f(2), f(3)));
    if !state.mission_man {
        rotz = st(ld(rotz) + ld(info.rotz));
    }
    if ctx.single_player && attributes & 0x4000 != 0 {
        // ATTRIBUTES_ScaleResources: more with a strong fleet when KillerCollDamage is also set,
        // fewer otherwise.
        let k = if attributes & 0x8 != 0 { ld(ctx.fleet_modifier) } else { 1.0 - ld(ctx.fleet_modifier) };
        number = ftol_i32(ld(number as f32) * k);
    }
    let layout = s(4);
    let shape = if layout.eq_ignore_ascii_case("Sphere") {
        VolumeShape::Sphere
    } else if layout.eq_ignore_ascii_case("Cylinder") {
        VolumeShape::Cylinder
    } else if layout.eq_ignore_ascii_case("Rectangle") {
        VolumeShape::Rectangle
    } else if layout.eq_ignore_ascii_case("Nebula") {
        // nebGo over (radius, length, roty) with 10000 chunks and `number` tendrils.
        crate::nebula::neb_go(u, src, centre, Vector::new(radius, length, roty), crate::nebula::LEVEL_CHUNKS, number.max(0) as u32, attributes);
        return;
    } else {
        state.unmodelled.push(("Resources".into(), field.to_string()));
        return;
    };
    let mut vol = ResourceVolume {
        shape,
        centre,
        radius,
        length,
        roty: if shape == VolumeShape::Cylinder { st(ld(roty) * ld(DEG_TO_RAD)) } else { roty },
        rotz,
        number,
        attributes,
        attributes_param,
        obj_type,
        prob: [[0; 2]; 10],
        cum: [[0; 2]; 10],
        sum: [0; 2],
    };
    resource_distribution(src, dir, &dist, obj_type, &mut vol, state);
    let regrow = attributes & 0x800 != 0;
    let vol_index = regrow.then_some(state.resource_volumes.len());
    for _ in 0..number.max(0) {
        let p = random_point_in_volume(u, &vol, &ctx.resources);
        add_resource_at(u, &vol, vol_index, false, p, &ctx.resources, state);
    }
    if regrow {
        state.resource_volumes.push(vol);
    }
}

/// `frandom`-style draw `[0, 1)` scaled: `(float)rand * scale`, with `scale` already rounded.
fn rand_scaled(u: &mut Universe, scale: f32) -> f64 {
    fild_u32(u.randy.random(RAN_GAME)) * ld(scale)
}

/// `AddResourceToSphere` / `AddResourceToCylinder` / `AddResourceToRectangle`: a random point.
fn random_point_in_volume(u: &mut Universe, vol: &ResourceVolume, tw: &ResourceTweaks) -> Vector {
    const INV: f64 = 2.328_306_4e-10;
    match vol.shape {
        VolumeShape::Sphere => {
            let r = ld(vol.radius);
            let p = if vol.attributes & 2 == 0 {
                let scale = st((r + r) * INV);
                let x = st(rand_scaled(u, scale) - r);
                let y = st(rand_scaled(u, scale) - r);
                let z = st(rand_scaled(u, scale) - r);
                Vector::new(x, y, z)
            } else {
                // ATTRIBUTES_ShellOfResources: a ring between width and radius.
                let width = st(ld(tw.ring_width_scale) * r);
                let a = u.randy.random(RAN_GAME);
                let b = u.randy.random(RAN_GAME);
                let c = u.randy.random(RAN_GAME);
                let decl = fild_u32(c) * 6.501_858_5e-10 - 1.396_263_5;
                let rr = st((r - ld(width)) * INV * ld(st(fild_u32(a))) + ld(width));
                let roty = Matrix::rot_about_y(st(fcos(decl)), st(fsin(decl)));
                let ang = ld(st(ld(st(fild_u32(b))) * 1.462_918_1e-9));
                let rotz = Matrix::rot_about_z(st(fcos(ang)), st(fsin(ang)));
                rotz.mul_vec(roty.mul_vec(Vector::new(rr, 0.0, 0.0)))
            };
            Vector::new(st(ld(p.x) + ld(vol.centre.x)), st(ld(p.y) + ld(vol.centre.y)), st(ld(p.z) + ld(vol.centre.z)))
        }
        VolumeShape::Cylinder => {
            let roty = Matrix::rot_about_y(st(fcos(ld(vol.roty))), st(fsin(ld(vol.roty))));
            let rotz = Matrix::rot_about_z(st(fcos(ld(vol.rotz))), st(fsin(ld(vol.rotz))));
            let half = st(ld(vol.length) * 0.5);
            let dia = st(ld(vol.radius) * 2.0);
            let x = st(rand_scaled(u, st(ld(vol.length) * INV)) - ld(half));
            let y = st(rand_scaled(u, st(ld(dia) * INV)) - ld(vol.radius));
            let z = st(rand_scaled(u, st(ld(dia) * INV)) - ld(vol.radius));
            let p = rotz.mul_vec(roty.mul_vec(Vector::new(x, y, z)));
            Vector::new(st(ld(p.x) + ld(vol.centre.x)), st(ld(p.y) + ld(vol.centre.y)), st(ld(p.z) + ld(vol.centre.z)))
        }
        VolumeShape::Rectangle => {
            let between = |u: &mut Universe, lo: f32, hi: f32| st(rand_scaled(u, st((ld(hi) - ld(lo)) * INV)) + ld(lo));
            let x = between(u, -vol.radius, vol.radius);
            let y = between(u, -vol.length, vol.length);
            let z = between(u, -vol.roty, vol.roty);
            let rotz = Matrix::rot_about_z(st(fcos(ld(vol.rotz))), st(fsin(ld(vol.rotz))));
            let p = rotz.mul_vec(Vector::new(x, y, z));
            Vector::new(st(ld(p.x) + ld(vol.centre.x)), st(ld(p.y) + ld(vol.centre.y)), st(ld(p.z) + ld(vol.centre.z)))
        }
    }
}

/// The resource type drawn from a volume's distribution (`random() % sumTotal` against the
/// running sums).
fn random_resource_type(u: &mut Universe, vol: &ResourceVolume, regrowing: bool) -> u8 {
    let j = usize::from(regrowing);
    let r = u.randy.random(RAN_GAME);
    let sum = u32::from(vol.sum[j]);
    if sum == 0 {
        return 0;
    }
    let r = r % sum;
    (0..10).find(|&i| r < u32::from(vol.cum[i][j])).unwrap_or(0) as u8
}

/// `AddResourceToVolumeAtPosition` (0x49b350) for a level being loaded (no overlap test): the
/// type from the volume's distribution, then the object (`univAddAsteroid` /
/// `univAddDustCloud` / `univAddGasCloud`, each drawing a random orientation), then the
/// asteroid's spin and drift.
fn add_resource_at(u: &mut Universe, vol: &ResourceVolume, vol_index: Option<usize>, regrowing: bool, p: Vector, tw: &ResourceTweaks, state: &mut LevelState) {
    use crate::resource::{self, ResourceKind as Kind};
    let t = random_resource_type(u, vol, regrowing);
    let kind = match vol.obj_type {
        2 => ResourceKind::Asteroid(t),
        4 => ResourceKind::GasCloud(t),
        _ => ResourceKind::DustCloud(t),
    };
    let sim_kind = match kind {
        ResourceKind::Asteroid(t) => Some(Kind::Asteroid(t)),
        ResourceKind::DustCloud(t) => Some(Kind::DustCloud(t)),
        ResourceKind::GasCloud(_) => None,
    };
    let id = sim_kind.and_then(|k| resource::add_resource(u, k, p));
    if id.is_none() {
        // No simulation object (gas clouds, or statics missing): the original still draws the
        // object's orientation.
        resource::random_orientation(u);
        if sim_kind.is_some() {
            state.warnings.push(format!("{kind:?}: no statics; kept as a record only"));
        }
    }
    let mut res = LevelResource {
        kind,
        id,
        position: p,
        velocity: Vector::ZERO,
        rot_speed: Vector::ZERO,
        attributes: vol.attributes,
        attributes_param: vol.attributes_param,
        volume: vol_index.filter(|_| vol.attributes & 0x800 != 0),
    };
    if vol.obj_type == 2 {
        spin_asteroid(u, &mut res, t, tw);
        if vol.attributes & 4 != 0 {
            // ATTRIBUTES_VelToMothership: towards the "TowardsVector" point.
            let h = kas_hash("TowardsVector");
            let d = state.point(h).map_or(Vector::ZERO, |p| Vector::new(p[0], p[1], p[2]));
            let n = d.normalized();
            res.velocity = Vector::new(st(ld(n.x) * ld(tw.velocity_towards)), st(ld(n.y) * ld(tw.velocity_towards)), st(ld(n.z) * ld(tw.velocity_towards)));
        }
        if vol.attributes & 0x200 != 0 {
            // A meteor storm volume gives its asteroids a drift and spins them again.
            spin_asteroid(u, &mut res, t, tw);
        }
    }
    if let Some(r) = id.and_then(|id| u.base.resources.get_mut(id)) {
        r.attributes = res.attributes;
        r.attributes_param = res.attributes_param;
        r.volume = res.volume;
        r.rotspeed = res.rot_speed;
        r.velocity = res.velocity;
        r.is_moving = res.velocity != Vector::ZERO;
    }
    state.resources.push(res);
}

/// `AddRandomRotationToAsteroid`: the larger asteroids may get a random spin.
fn spin_asteroid(u: &mut Universe, res: &mut LevelResource, t: u8, tw: &ResourceTweaks) {
    if t <= 2 {
        return;
    }
    if u.randy.random(RAN_GAME) & 0xff > tw.rotate_prob {
        return;
    }
    let i = usize::from(t).min(4);
    let lo = tw.lo_rotate[i];
    let scale = st((ld(tw.hi_rotate[i]) - ld(lo)) * 2.328_306_4e-10);
    let mut r = || st(rand_scaled(u, scale) + ld(lo));
    res.rot_speed = Vector::new(r(), r(), r());
}

/// Puts the ships of every formation command at their formation slots
/// (`clPresetShipsToPosition` / `setFormationToDo`, run once after a level is loaded).
pub fn preset_ships_to_position(u: &mut Universe) {
    let parades: Vec<_> = u.commands.iter().filter(|c| c.order == command::order::MILITARY_PARADE).filter_map(|c| c.parade.clone()).collect();
    for p in parades {
        crate::parade::set_parade(u, &p);
    }
    let statics = u.statics.clone();
    let cmds: Vec<_> = u.commands.iter().filter(|c| c.attributes & command::attr::FORMATION != 0).cloned().collect();
    for c in cmds {
        if c.selection.len() < 2 {
            continue;
        }
        let Some(leader) = u.ships.get(c.selection[0]) else { continue };
        let origin = leader.coll.coll_position;
        let coordsys = if c.formation.locked { c.formation.coordsys } else { leader.rotinfo.coordsys };
        for &id in &c.selection[1..] {
            let Some(s) = u.ships.get_mut(id) else { continue };
            let p = match c.formation.formation_type {
                formation::kind::SPHERE | 10 => Vector::new(
                    st(ld(origin.x) + ld(s.formation_offset.x)),
                    st(ld(origin.y) + ld(s.formation_offset.y)),
                    st(ld(origin.z) + ld(s.formation_offset.z)),
                ),
                _ => {
                    let o = coordsys.mul_vec(s.formation_offset);
                    Vector::new(st(ld(o.x) + ld(origin.x)), st(ld(o.y) + ld(origin.y)), st(ld(o.z) + ld(origin.z)))
                }
            };
            s.posinfo.position = p;
            s.rotinfo.coordsys = coordsys;
            if let Some(stat) = statics.ship(s.ship_type) {
                physics::update_obj_rot_info(s, stat);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formation_names() {
        assert_eq!(formation_from_name("delta3d_formation"), Some(2));
        assert_eq!(formation_from_name("SPHERE_FORMATION"), Some(5));
        assert_eq!(formation_from_name("NULL_FORMATION"), None);
    }

    #[test]
    fn mission_paths_follow_the_original_format() {
        assert_eq!(mission_paths(1, None), ("SinglePlayer\\Mission01\\".into(), "Mission01.mission".into()));
        assert_eq!(mission_paths(4, Some(2)).1, "Mission04_NIS02.mission");
    }

    #[test]
    fn box_volume_extent_and_centre() {
        let mut st = LevelState::default();
        ai_box("Everything,6014.8,6766.7,0.0,500000.0,500000.0,300000.0", &mut st);
        let v = st.volume(kas_hash("Everything")).unwrap();
        assert!(v.contains(Vector::new(0.0, 0.0, 0.0)));
        assert!(!v.contains(Vector::new(0.0, 0.0, 200_000.0)));
        let c = v.center();
        assert!((c.x - 6014.8).abs() < 0.1 && (c.y - 6766.7).abs() < 0.1);
        ai_sphere("Unexplored,-21431.0,-49458.8,-13193.0,42000.0", &mut st);
        match st.volume(kas_hash("unexplored")).unwrap() {
            Volume::Sphere { radius, .. } => assert_eq!(*radius, 21000.0),
            _ => panic!("sphere expected"),
        }
    }

    #[test]
    fn path_points_by_index() {
        let mut st = LevelState::default();
        ai_path("ProxPatrol,1.0,2.0,3.0,Closed,2/3", &mut st);
        ai_path("ProxPatrol,4.0,5.0,6.0,Closed,1/3", &mut st);
        let p = st.path(kas_hash("ProxPatrol")).unwrap();
        assert!(p.closed);
        assert_eq!(p.points.len(), 3);
        assert_eq!(p.points[0], Vector::new(4.0, 5.0, 6.0));
        assert_eq!(p.points[1], Vector::new(1.0, 2.0, 3.0));
    }

    /// An in-memory archive.
    struct Mem(Vec<(String, String)>);

    impl DataSource for Mem {
        fn read(&self, path: &str) -> Option<Vec<u8>> {
            let norm = |p: &str| p.replace('\\', "/").to_ascii_lowercase();
            self.0.iter().find(|f| norm(&f.0) == norm(path)).map(|f| f.1.clone().into_bytes())
        }

        fn exists(&self, path: &str) -> bool {
            self.read(path).is_some()
        }
    }

    fn ship(t: ShipType, race: ShipRace, size: f32) -> crate::statics::ShipStaticInfo {
        let mut s = crate::statics::ShipStaticInfo::new(t, String::new());
        s.ship_race = race;
        s.health.max_health = 100.0;
        s.header.coll.coll_sphere_size = size;
        s
    }

    #[test]
    fn level_creates_ships_teams_and_resources_in_file_order() {
        let level = "; MissionMan script
[x]
MissionSphere    -1,?,ResourceSphere,0.0,0.0,0.0,30000.0,0.0,Sphere.missphere
smUniverseSizeX 1000
smUniverseSizeY 2000
smUniverseSizeZ 3000
";
        let sphere = "; MissionMan
            AIPoint    StartPointPlayer,1.0,2.0,3.0,45.0
            Ships    Alpha,100.0,0.0,0.0,90.0,RACE_TAIDAN,tHeavyInterceptor,3,NULL_FORMATION|?|colorScheme(2),0,?,3
            Ships    Beta,0.0,0.0,0.0,0.0,RACE_KUSHAN,kHeavyInterceptor,1,NULL_FORMATION|?|?,0,?,1
            Ships    Alpha,0.0,500.0,0.0,0.0,RACE_TAIDAN,tHeavyInterceptor,1,NULL_FORMATION|?|?,0,?,1
            Ships    Gamma,0.0,0.0,0.0,0.0,RACE_TAIDAN,tHeavyInterceptor,0,NULL_FORMATION|?|?,0,?,1
            Resources    Asteroid,0.0,0.0,0.0,Sphere,a.dist,2,1000.0,0.0,0.0,0.0,0,?
";
        let src = Mem(vec![
            ("SinglePlayer/Mission01/Mission01.mission".into(), level.into()),
            ("SinglePlayer/Mission01/Sphere.missphere".into(), sphere.into()),
            ("SinglePlayer/Mission01/a.dist".into(), "Asteroid0 1\n".into()),
        ]);
        let statics = std::sync::Arc::new(crate::statics::test_world(vec![
            ship(ShipType::tHeavyInterceptor, ShipRace::Taiidan, 20.0),
            ship(ShipType::kHeavyInterceptor, ShipRace::Kushan, 25.0),
        ]));
        let mut u = Universe::new(statics, crate::SimTweaks::compiled(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        u.single_player = true;
        let ctx = LevelContext { single_player: true, mission: 1, fleet_modifier: 0.0, resources: ResourceTweaks::default() };
        let mut st = LevelState::default();
        let (dir, file) = mission_paths(1, None);
        load_level(&mut u, &src, &dir, &file, &ctx, &mut st).unwrap();

        assert!(st.mission_man);
        assert_eq!(u.world_bounds, Vector::new(6000.0, 7000.0, 8000.0));
        let labels: Vec<Hash> = st.teams.iter().map(|t| t.label).collect();
        assert_eq!(labels, vec![kas_hash("Alpha"), kas_hash("Beta"), kas_hash("Gamma")]);
        let alpha = st.team(kas_hash("Alpha")).unwrap();
        assert_eq!(alpha.ships, vec![ShipId(0), ShipId(1), ShipId(2), ShipId(4)]);
        assert_eq!((alpha.orig_count, alpha.new_ships), (4, -4));
        assert_eq!(st.team(kas_hash("Gamma")).unwrap().ships.len(), 0);
        // Labelled ships belong to the computer player; a group is stacked along z by twice the
        // collision radius.
        assert!(u.ships.iter().all(|s| s.player == COMPUTER_PLAYER));
        let z: Vec<f32> = (0..3).map(|i| u.ships.ship(ShipId(i)).posinfo.position.z).collect();
        assert_eq!(z, vec![0.0, 40.0, 80.0]);
        assert_eq!(u.ships.ship(ShipId(0)).color_scheme, 2);
        // The yaw turns the ship about its own up axis (+Z for the default orientation), so the
        // heading stays in the horizontal plane.
        let h = u.ships.ship(ShipId(0)).heading();
        assert!(h.z.abs() < 1e-6 && (h.x * h.x + h.y * h.y - 1.0).abs() < 1e-5);
        // Two asteroids of type 0: three random numbers for the position, one for the type and
        // three for the orientation `univAddAsteroid` gives it (`univInitSpaceObjPosRot` with a
        // random rotation; drawn even though this test world has no asteroid statics), no spin
        // for small asteroids.
        assert_eq!(st.resources.len(), 2);
        let mut r = hwc_math::Randy::default();
        for _ in 0..14 {
            r.random(RAN_GAME);
        }
        assert_eq!(u.randy, r);
        assert!(st.resources.iter().all(|x| x.kind == ResourceKind::Asteroid(0) && x.position.x.abs() <= 1000.0));
    }

    /// `ExcludeShips race,All` only unloads the race's types; a single type also goes through
    /// `rmEnableShip`. The Super Acolyte is a derelict type, so excluding the derelicts leaves it
    /// out of the construction manager.
    #[test]
    fn include_and_exclude_ships_flag_needed_types() {
        let level = "[x]
ExcludeShips    RACE_DERELICT,All
ExcludeShips    RACE_SECT,All
IncludeShips    RACE_SECT,sAcolyte
ExcludeShips    RACE_SECT,sMimic
";
        let src = Mem(vec![("SinglePlayer/Mission01/Mission01.mission".into(), level.into())]);
        let statics = std::sync::Arc::new(crate::statics::test_world(vec![
            ship(ShipType::sAcolyte, ShipRace::Somtaaw, 10.0),
            ship(ShipType::sMimic, ShipRace::Somtaaw, 10.0),
            ship(ShipType::sRecon, ShipRace::Somtaaw, 10.0),
            ship(ShipType::dAcolyte, ShipRace::Derelict, 10.0),
            ship(ShipType::tHeavyInterceptor, ShipRace::Taiidan, 10.0),
        ]));
        let mut u = Universe::new(statics, crate::SimTweaks::compiled(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        u.single_player = true;
        let ctx = LevelContext { single_player: true, mission: 1, fleet_modifier: 0.0, resources: ResourceTweaks::default() };
        let mut st = LevelState::default();
        let (dir, file) = mission_paths(1, None);
        load_level(&mut u, &src, &dir, &file, &ctx, &mut st).unwrap();
        let needed = |t: ShipType| u.statics.ship(t).unwrap().info_needed;
        assert!(needed(ShipType::sAcolyte));
        assert!(!needed(ShipType::sMimic) && !needed(ShipType::sRecon) && !needed(ShipType::dAcolyte));
        assert!(needed(ShipType::tHeavyInterceptor));
        let sp = &u.statics.research.single_player_somtaaw.ship_mode;
        assert_eq!((sp.get(&ShipType::sAcolyte), sp.get(&ShipType::sMimic), sp.get(&ShipType::sRecon)), (Some(&0), Some(&-2), None));
        // The multiplayer table keeps the Super Acolyte disabled whatever is asked.
        enable_ship(&mut u, 0, ShipType::dAcolyte, true);
        assert_eq!(u.statics.research.single_player_somtaaw.ship_mode.get(&ShipType::dAcolyte), Some(&0));
        assert_eq!(u.statics.research.multiplayer_somtaaw.ship_mode.get(&ShipType::dAcolyte), Some(&-2));
    }

    #[test]
    fn point_keeps_fourth_component() {
        let mut st = LevelState::default();
        ai_point("StartPointPlayer,35058.2,16290.6,0.0,-151.0", &mut st);
        assert_eq!(st.point(kas_hash("StartPointPlayer")), Some([35058.2, 16290.6, 0.0, -151.0]));
    }
}
