//! Single-player missions: a level plus its KAS script running on the simulation.
//!
//! [`MissionRun::start`] follows the original's path into a campaign mission
//! (`singlePlayerStartGame` -> the opening NIS -> `singlePlayerNisStoppedCB` ->
//! `singlePlayerLoadNewLevel` -> `levelStartNext`): the player's fleet is put into hyperspace,
//! the universe is reset (game time is kept), the mission level is loaded and the
//! single-player state machine is set to start the KAS mission on the next frame.
//! [`MissionRun::step`] is one `univUpdate`: the parts of `singlePlayerGameUpdate` that matter
//! before the mission's hyperspace exit (KAS mission start, NIS/NISlet requests, the hyperspace
//! arrival of the player's fleet with its effect states, `G_ShipsHaveArrived`) run where the
//! original runs them, and `kasExecute` runs from the computer-player slot every
//! `AIPLAYER_UPDATE_RATE[AI_ADV]` + 1 frames, skipped while a hyperspace transition is in
//! progress.
//!
//! NIS and NISlets are not played: a request is logged and treated as finished (optionally
//! after a configurable time), which sets the same `G_Nis<n>Complete` / `G_Nislet<n>Complete`
//! variables the original's NIS-stopped callback sets.
//!
//! See `docs/sim/missions.md`.

use std::fmt;
use std::sync::Arc;

use hwc_formats::nis::{object_key, Nis, NisScript, ObjectCategory};
use hwc_kas::interp::{kas_frame_due, Interpreter};
use hwc_kas::{kas_hash, var_hash, Hash, Mission as KasMission};
use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::level::{self, LevelContext, LevelError, LevelState, ResourceTweaks};
use crate::object::{sof, ShipId};
use crate::statics::{DataSource, ShipClass, ShipRace, ShipType, StaticWorld};
use crate::tunables::SimTweaks;
use crate::universe::{SimEvent, Universe, DEFAULT_SHIP_MATRIX};

/// The human player.
pub const HUMAN_PLAYER: usize = 0;

/// `AIPLAYER_UPDATE_RATE[AI_ADV]`: the single-player computer player (and so KAS) runs when
/// `univUpdateCounter & 15 == SP_KAS_UPDATE_FRAME & 15`.
pub const SP_KAS_UPDATE_RATE: u32 = 15;

/// Single-player states (`singlePlayerGameInfo.hyperspaceState`, 0xaaf910).
pub mod sp_state {
    pub const NONE: u32 = 0;
    /// The hyperspace button was pressed: roll call (or the five seconds before a jump without
    /// one).
    pub const WAITING_ROLLCALL: u32 = 2;
    /// 2.5 s for Fleet Command's announcement.
    pub const WAITING_FLEET_COMMAND: u32 = 3;
    /// The fleet turns to the mothership's heading and jumps.
    pub const SHIPS_LEAVING: u32 = 4;
    /// Whiteout and the between-missions movie.
    pub const WHITEOUT: u32 = 5;
    pub const START_KAS: u32 = 6;
    pub const KAS_STARTED: u32 = 7;
    pub const ARRIVING: u32 = 9;
    /// The jump failed (`Game_HyperspaceFailed`).
    pub const FAILED: u32 = 10;
}

/// Per-ship hyperspace states (`shipHyperspaceState`, ship info +0x20).
pub mod ship_hs {
    pub const INITIAL: u32 = 4;
    pub const WAITING: u32 = 6;
    pub const DECELERATING: u32 = 7;
}

/// Hyperspace effect states (`hsState`).
pub mod hs_effect {
    pub const INACTIVE: u32 = 0;
    pub const POPUP_INTO: u32 = 1;
    pub const POPUP_OUTOF: u32 = 2;
    pub const SLICING_INTO: u32 = 3;
    pub const SLICING_OUTOF: u32 = 4;
    pub const COLLAPSE_INTO: u32 = 5;
    pub const COLLAPSE_OUTOF: u32 = 6;
    pub const FINISHED: u32 = 7;
}

/// `singleplayer.script` (table at 0x88a0e0).
#[derive(Debug, Clone, PartialEq)]
pub struct SpTweaks {
    pub clipt_neg_thresh: f32,
    pub clipt_neg_scalar: f32,
    pub clipt_neg_finish: f32,
    pub clipt_pos_thresh: f32,
    pub clipt_pos_scalar: f32,
    pub clipt_pos_start: f32,
    pub slice_rate: f32,
    pub mothership_always_arrives_first: bool,
    pub base_waittime: f32,
    pub base_arrivetime: f32,
    pub base_mothership_arrivetime: f32,
    pub random_waittime: f32,
    pub decelerate_time: f32,
    /// `HYPERSPACE_ACCELERATE_TIME` (0x88a06c).
    pub accelerate_time: f32,
    /// `HYPERSPACE_WHITEOUTTIME` (0x88a080).
    pub whiteout_time: f32,
    /// `HYPERSPACE_RANGE`, squared as the script table stores it (0x88a0c4).
    pub hyperspace_range_sq: f32,
    /// `HYPERSPACE_HEADINGACCURACY`, the cosine of the script's angle (0x88a078).
    pub heading_accuracy: f32,
    pub starting_rus: i32,
    /// `SUBMESSAGE_SAFETY_TIMEOUT`: how long `IntelEventEnded` waits for a subtitle that never
    /// reports its end.
    pub submessage_safety_timeout: f32,
    /// `BaseFleetStrengthForLevel` per mission (low, high).
    pub base_fleet_strength: Vec<(u32, f32, f32)>,
}

impl Default for SpTweaks {
    fn default() -> Self {
        SpTweaks {
            clipt_neg_thresh: -0.4,
            clipt_neg_scalar: 0.33,
            clipt_neg_finish: 0.9,
            clipt_pos_thresh: 0.5,
            clipt_pos_scalar: 0.5,
            clipt_pos_start: 1.0,
            slice_rate: 0.075,
            mothership_always_arrives_first: true,
            base_waittime: 3.0,
            base_arrivetime: 2.0,
            base_mothership_arrivetime: 1.0,
            random_waittime: 0.2,
            decelerate_time: 3.0,
            accelerate_time: 3.0,
            whiteout_time: 0.1,
            hyperspace_range_sq: 9_000_000.0,
            heading_accuracy: 0.98,
            starting_rus: 200,
            submessage_safety_timeout: 15.0,
            base_fleet_strength: Vec::new(),
        }
    }
}

impl SpTweaks {
    /// Defaults overlaid with `singleplayer.script`.
    pub fn load(src: &dyn DataSource) -> Self {
        use crate::statics::cscan::{parse_bool, scan, scan_f32, scan_i32, Scanned};
        let mut t = SpTweaks::default();
        let Some(lines) = level::read_lines(src, "singleplayer.script") else { return t };
        for (k, v) in lines {
            let f = |dst: &mut f32| {
                if let Some(x) = scan_f32(&v) {
                    *dst = x;
                }
            };
            match k.to_ascii_uppercase().as_str() {
                "HS_CLIPT_NEG_THRESH" => f(&mut t.clipt_neg_thresh),
                "HS_CLIPT_NEG_SCALAR" => f(&mut t.clipt_neg_scalar),
                "HS_CLIPT_NEG_FINISH" => f(&mut t.clipt_neg_finish),
                "HS_CLIPT_POS_THRESH" => f(&mut t.clipt_pos_thresh),
                "HS_CLIPT_POS_SCALAR" => f(&mut t.clipt_pos_scalar),
                "HS_CLIPT_POS_START" => f(&mut t.clipt_pos_start),
                "HYPERSPACE_SLICE_RATE" => f(&mut t.slice_rate),
                "HYPERSPACE_MOTHERSHIP_ALWAYS_ARRIVES_FIRST" => t.mothership_always_arrives_first = parse_bool(&v),
                "HYPERSPACE_BASE_WAITTIME" => f(&mut t.base_waittime),
                "HYPERSPACE_BASE_ARRIVETIME" => f(&mut t.base_arrivetime),
                "HYPERSPACE_BASE_MOTHERSHIPARRIVETIME" => f(&mut t.base_mothership_arrivetime),
                "HYPERSPACE_RANDOM_WAITTIME" => f(&mut t.random_waittime),
                "HYPERSPACE_DECELERATE_TIME" => f(&mut t.decelerate_time),
                "HYPERSPACE_ACCELERATE_TIME" => f(&mut t.accelerate_time),
                "HYPERSPACE_WHITEOUTTIME" => f(&mut t.whiteout_time),
                // scriptSetReal32SqrCB / scriptSetCosAngCB.
                "HYPERSPACE_RANGE" => {
                    if let Some(x) = scan_f32(&v) {
                        t.hyperspace_range_sq = st(ld(x) * ld(x));
                    }
                }
                "HYPERSPACE_HEADINGACCURACY" => {
                    if let Some(x) = scan_f32(&v) {
                        t.heading_accuracy = st(fcos(ld(x) * ld(level::DEG_TO_RAD)));
                    }
                }
                "SUBMESSAGE_SAFETY_TIMEOUT" => f(&mut t.submessage_safety_timeout),
                "SINGLEPLAYER_STARTINGRUS" => {
                    if let Some(x) = scan_i32(&v) {
                        t.starting_rus = x;
                    }
                }
                "BASEFLEETSTRENGTHFORLEVEL" => {
                    let s = scan(&v, "%d %f %f");
                    if let (Some(Scanned::Int(l)), Some(Scanned::Float(a)), Some(Scanned::Float(b))) = (s.first(), s.get(1), s.get(2)) {
                        t.base_fleet_strength.push((*l as u32, *a, *b));
                    }
                }
                _ => {}
            }
        }
        t
    }
}

/// Choices for a headless run.
#[derive(Debug, Clone, PartialEq)]
pub struct MissionConfig {
    /// The mission number (1..17).
    pub mission: u32,
    /// `MissionSkillGet` (the single-player difficulty, 0x88a0d8; the game's default is 2).
    pub skill: i32,
    /// How long a requested NISlet "plays" before it counts as finished (0: at once).
    pub nislet_seconds: f32,
    /// When set, a speech event ends after this many seconds (standing in for the subtitle that
    /// reports the end of an intel event); otherwise only the safety timeout ends it, as in the
    /// original with no subtitles drawn.
    pub speech_seconds: Option<f32>,
    /// Record every KAS runtime call in the log.
    pub log_calls: bool,
    /// A front end plays the NIS and NISlets the mission asks for: requests wait in
    /// [`Game::pending_nis`] until [`MissionRun::nis_finished`] (otherwise they count as
    /// finished at once, or after [`nislet_seconds`](Self::nislet_seconds)).
    pub hold_nis: bool,
}

impl MissionConfig {
    pub fn new(mission: u32) -> Self {
        MissionConfig { mission, skill: 2, nislet_seconds: 0.0, speech_seconds: None, log_calls: true, hold_nis: false }
    }
}

/// What a log line records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// Level loading, NIS skips, hyperspace, single-player state changes.
    Flow,
    /// A team entered a state or got an FSM.
    State,
    /// Objectives created, destroyed, completed.
    Objective,
    /// A KAS runtime call.
    Call,
    /// A runtime function the host does not implement yet (logged once per call).
    Unimplemented,
    /// A warning from the interpreter or the host.
    Warning,
}

/// One log line.
#[derive(Debug, Clone, PartialEq)]
pub struct LogEntry {
    /// `univUpdateCounter` when it happened.
    pub frame: u32,
    /// Game time.
    pub time: f32,
    pub kind: LogKind,
    pub text: String,
}

impl fmt::Display for LogEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{:6} {:8.3}s] {:?}: {}", self.frame, self.time, self.kind, self.text)
    }
}

/// A mission objective (`Objective`).
#[derive(Debug, Clone, PartialEq)]
pub struct Objective {
    pub label: String,
    pub brief: String,
    pub full: String,
    pub primary: bool,
    pub complete: bool,
}

/// A ship of the player's fleet travelling in hyperspace between levels (an entry of the
/// original's `ShipsInHyperspace` list, with the ships inside it).
#[derive(Debug, Clone, PartialEq)]
pub struct FleetShip {
    pub ship_type: ShipType,
    /// Offset from the mothership when the fleet left (ship info +0x2c..+0x34: along the
    /// mothership's heading, right and up axes).
    pub offset: Vector,
    /// Ships docked inside it.
    pub docked: Vec<FleetShip>,
    /// A module that rides on the mothership.
    pub module: bool,
    /// Carried over from a previous mission (created without the mission's starting modules,
    /// docked ships really inside their host) rather than made up from the opening NIS.
    pub carried: bool,
    /// The ship's own state that survives the jump (the original keeps the ship object).
    pub color_scheme: Option<usize>,
    pub tactics: Option<usize>,
    pub attributes: u32,
}

impl FleetShip {
    /// A ship of the opening NIS's fleet.
    pub fn new(ship_type: ShipType) -> Self {
        FleetShip { ship_type, offset: Vector::ZERO, docked: Vec::new(), module: false, carried: false, color_scheme: None, tactics: None, attributes: 0 }
    }

    /// Every ship type in this entry and inside it.
    pub fn types(&self) -> Vec<ShipType> {
        let mut v = vec![self.ship_type];
        for d in &self.docked {
            v.extend(d.types());
        }
        v
    }
}

/// A ship on its way in from hyperspace (`UpdateArrivingShip`).
#[derive(Debug, Clone, PartialEq)]
pub struct ArrivingShip {
    pub ship_type: ShipType,
    /// The ship, once it is back in the universe.
    pub ship: Option<ShipId>,
    pub offset: Vector,
    /// `shipHyperspaceState`.
    pub state: u32,
    /// `timeToHyperspace` (+0x24).
    pub time: f32,
    /// `hsState`, `clipt` (+0x38), rate (+0x3c), `hsFlags` (+0x44).
    pub hs_state: u32,
    pub clipt: f32,
    pub rate: f32,
    pub hs_flags: u32,
    pub mothership: bool,
    pub clamped_to_mothership: bool,
    pub docked: Vec<FleetShip>,
    /// The fleet entry this ship came from (its carried state).
    pub entry: Option<Box<FleetShip>>,
}

/// Camera state the KAS camera functions read and write.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CameraState {
    pub angle: f32,
    pub declination: f32,
    pub distance: f32,
    pub focus: Vec<ShipId>,
    pub scripted: bool,
}

/// Game-side state of a running mission (everything but the universe and the interpreter).
#[derive(Debug, Clone)]
pub struct Game {
    /// Presentation calls KAS made (speech, location cards, music), in order, for the game's
    /// front end to play; the simulation never reads them (drain it after each step).
    pub presentation: Vec<Presentation>,
    pub config: MissionConfig,
    pub sp_tweaks: SpTweaks,
    pub level: LevelState,
    /// Named KAS selections (`SHIPS_x`), in creation order.
    pub selections: Vec<(Hash, Vec<ShipId>)>,
    pub objectives: Vec<Objective>,
    pub pings: Vec<String>,
    pub pointers: Vec<String>,
    pub tut_flags: Vec<String>,
    /// The sensors manager is open (0x900620) and the KAS sensors flag (0x900608).
    pub sensors_open: bool,
    pub sensors_kas_flag: i32,
    pub sensors_look_at: Vector,
    pub wide_screen: bool,
    pub camera: CameraState,
    pub music: Option<i32>,
    pub techs: Vec<String>,
    pub research_allowed: Vec<String>,
    pub construction: Vec<(String, String, i32)>,
    /// `player->resourceUnits` per player.
    pub resource_units: Vec<i32>,
    pub pending_save: Option<(i32, String)>,
    pub mission_failed: bool,
    pub delete_when_mission_ends: Vec<ShipId>,
    /// Repair modifiers set by KAS (+0x3dc), by ship.
    pub repair_modifiers: Vec<(ShipId, f32)>,
    /// Ships docked inside another ship by KAS (`DockInstant`, `ShipsCreateLaunch`): (ship, host).
    pub docked: Vec<(ShipId, ShipId)>,
    /// 0x91b324: the last intel event (subtitle) has ended.
    pub intel_ended: bool,
    /// 0x8e9dd0: when `IntelEventEnded` started waiting without a subtitle.
    pub intel_stamp: f32,
    /// When the current speech event ends ([`MissionConfig::speech_seconds`]).
    pub speech_end: Option<f32>,
    /// Single-player state machine.
    pub sp_state: u32,
    /// The player's fleet in hyperspace.
    pub fleet: Vec<FleetShip>,
    pub arriving: Vec<ArrivingShip>,
    /// A NISlet "playing": (number, finish time).
    pub nislet: Option<(i32, f32)>,
    pub ships_have_arrived: bool,
    /// A NIS (`G_PlayNis`) or NISlet (`G_PlayNislet`) waiting for the front end to play it
    /// ([`MissionConfig::hold_nis`]).
    pub pending_nis: Option<NisRequest>,
    /// The single-player state of the trip between missions (roll call, the fleet leaving, the
    /// ships in hyperspace); see [`crate::campaign`].
    pub hyper: crate::campaign::HyperState,
    /// Label names for the log (team labels from the level, names from the mission IR).
    pub names: std::collections::HashMap<Hash, String>,
    pub log: Vec<LogEntry>,
    pub(crate) frame: u32,
    pub(crate) time: f32,
}

impl Game {
    pub fn name(&self, h: Hash) -> String {
        self.names.get(&(h & !hwc_kas::GLOBAL_FLAG)).cloned().unwrap_or_else(|| format!("#{h:08x}"))
    }

    pub fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        let text = text.into();
        self.log.push(LogEntry { frame: self.frame, time: self.time, kind, text });
    }

    /// The selection with this label, created empty on first use (`kasGrowSelectionPtr`).
    pub fn selection_mut(&mut self, h: Hash) -> &mut Vec<ShipId> {
        let i = match self.selections.iter().position(|s| s.0 == h) {
            Some(i) => i,
            None => {
                self.selections.push((h, Vec::new()));
                self.selections.len() - 1
            }
        };
        &mut self.selections[i].1
    }

    pub fn selection(&self, h: Hash) -> Option<&Vec<ShipId>> {
        self.selections.iter().find(|s| s.0 == h).map(|s| &s.1)
    }

    pub fn objective(&self, label: &str) -> Option<&Objective> {
        self.objectives.iter().find(|o| o.label.eq_ignore_ascii_case(label))
    }

    /// A ship left the universe: out of every selection (`kasShipDied`) and team
    /// (`aitShipDied`), swapping the last entry into its place as the original does.
    pub fn ship_removed(&mut self, id: ShipId) {
        for (_, s) in &mut self.selections {
            swap_remove_all(s, id);
        }
        for t in &mut self.level.teams {
            swap_remove_all(&mut t.ships, id);
        }
        self.level.clamps.retain(|&(m, h)| m != id && h != id);
        for m in &mut self.level.motherships {
            if *m == Some(id) {
                *m = None;
            }
        }
        self.docked.retain(|&(s, h)| s != id && h != id);
        self.delete_when_mission_ends.retain(|&s| s != id);
    }
}

/// `growSelectRemoveShip` / `clRemoveTargetFromSelection`: remove every occurrence, moving the
/// last entry into the hole.
pub fn swap_remove_all(v: &mut Vec<ShipId>, id: ShipId) -> bool {
    let mut found = false;
    let mut i = 0;
    while i < v.len() {
        if v[i] == id {
            v.swap_remove(i);
            found = true;
        } else {
            i += 1;
        }
    }
    found
}

/// Why a mission could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionError(pub String);

impl fmt::Display for MissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MissionError {}

impl From<LevelError> for MissionError {
    fn from(e: LevelError) -> Self {
        MissionError(e.0)
    }
}

/// The player's fleet after the opening NIS of the campaign (`nis\n01.nis` with its
/// `.script`): every Somtaaw ship path the script keeps at the end (`remainAtEnd`). When the
/// level changes, ships that are not capital ships or modules go inside the mothership; the
/// mothership (with its modules, and in mission 1 the docking bay the NIS adds) goes into
/// hyperspace.
pub fn opening_nis_fleet(src: &dyn DataSource, statics: &StaticWorld, mission: u32) -> Result<Vec<FleetShip>, MissionError> {
    let nis_bytes = src.read("nis\\n01.nis").ok_or_else(|| MissionError("nis\\n01.nis not found".into()))?;
    let nis = Nis::parse(&nis_bytes).map_err(|e| MissionError(format!("n01.nis: {e:?}")))?;
    let script_bytes = src.read("nis\\n01.script").ok_or_else(|| MissionError("nis\\n01.script not found".into()))?;
    let script = NisScript::parse_bytes(&script_bytes).map_err(|e| MissionError(format!("n01.script: {e:?}")))?;
    let mut remain: Vec<String> = Vec::new();
    for e in &script.events {
        if e.keyword.eq_ignore_ascii_case("remainAtEnd") {
            if let Some(o) = &e.object {
                remain.push(object_key(o));
            }
        }
    }
    let mut kept: Vec<ShipType> = Vec::new();
    for path in &nis.objects {
        let ObjectCategory::Ship(race) = path.category() else { continue };
        if race != ShipRace::Somtaaw.index() as u32 {
            continue;
        }
        if !remain.contains(&object_key(&path.name)) {
            continue;
        }
        if let Some(t) = ShipType::from_name(path.base_name()) {
            kept.push(t);
        }
    }
    let mut fleet = Vec::new();
    let mut docked = Vec::new();
    for t in kept {
        let stat = statics.ship(t);
        let capital = stat.is_some_and(|s| s.is_capital_ship || s.ship_class == Some(ShipClass::Component));
        if t == ShipType::sMothership {
            fleet.push(FleetShip::new(t));
            // (The mission modules come with the mothership: `univCreateShip` creates them.)
            if mission == 1 {
                fleet.push(FleetShip { module: true, ..FleetShip::new(ShipType::sMothershipDockingBay) });
            }
        } else if capital {
            fleet.push(FleetShip::new(t));
        } else {
            docked.push(FleetShip::new(t));
        }
    }
    match fleet.iter_mut().find(|f| f.ship_type == ShipType::sMothership) {
        Some(ms) => ms.docked = docked,
        None => return Err(MissionError("opening NIS has no mothership".into())),
    }
    Ok(fleet)
}

/// A running mission: the universe, the KAS interpreter and the game-side state.
#[derive(Debug, Clone)]
pub struct MissionRun {
    pub universe: Universe,
    pub kas: Interpreter,
    pub game: Game,
}

impl MissionRun {
    /// Sets up mission `config.mission` from `src` with the mission script `script`: the
    /// player's fleet (from the opening NIS for mission 1) goes into hyperspace and the level is
    /// loaded; the KAS mission starts on the first [`step`](Self::step).
    pub fn start(
        statics: Arc<StaticWorld>,
        tweaks: SimTweaks,
        src: &dyn DataSource,
        script: KasMission,
        config: MissionConfig,
    ) -> Result<Self, MissionError> {
        let mission = config.mission;
        if mission != 1 {
            return Err(MissionError(format!(
                "mission {mission}: the fleet comes over from the previous mission (use MissionRun::start_campaign with its Carry, or Carry::warp)"
            )));
        }
        let fleet = opening_nis_fleet(src, &statics, mission)?;
        Self::load(statics, tweaks, src, script, config, fleet, None)
    }

    /// Starts the mission after the one `carry` was taken from (`singlePlayerLoadNewLevel`):
    /// the fleet that hyperspaced out arrives here with its RUs, research and technology.
    /// `config.mission` must be the next mission.
    pub fn start_campaign(
        statics: Arc<StaticWorld>,
        tweaks: SimTweaks,
        src: &dyn DataSource,
        script: KasMission,
        config: MissionConfig,
        carry: &crate::campaign::Carry,
    ) -> Result<Self, MissionError> {
        if config.mission != carry.mission + 1 {
            return Err(MissionError(format!("the carried fleet left mission {}; it arrives in mission {}, not {}", carry.mission, carry.mission + 1, config.mission)));
        }
        let fleet = crate::campaign::wipe_unavailable_modules(&carry.fleet, config.mission);
        Self::load(statics, tweaks, src, script, config, fleet, Some(carry))
    }

    fn load(
        statics: Arc<StaticWorld>,
        tweaks: SimTweaks,
        src: &dyn DataSource,
        script: KasMission,
        config: MissionConfig,
        fleet: Vec<FleetShip>,
        carry: Option<&crate::campaign::Carry>,
    ) -> Result<Self, MissionError> {
        let mission = config.mission;
        let mut universe = Universe::new(statics.clone(), tweaks, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        universe.single_player = true;
        universe.sp_difficulty = config.skill.max(0) as u32;
        universe.base.options.mission = mission as i32;
        // gameStart runs with the single-player flag set: the players take the single-player
        // research tables (rmInitializeResearchStruct), not the multiplayer ones.
        crate::base::init(&mut universe);
        if let Some(c) = carry {
            crate::campaign::restore_player(&mut universe, c);
        }
        let sp_tweaks = SpTweaks::load(src);

        let mut game = Game {
            config,
            sp_tweaks,
            level: LevelState::default(),
            selections: Vec::new(),
            objectives: Vec::new(),
            pings: Vec::new(),
            pointers: Vec::new(),
            tut_flags: Vec::new(),
            sensors_open: false,
            sensors_kas_flag: 0,
            sensors_look_at: Vector::ZERO,
            wide_screen: false,
            camera: CameraState::default(),
            presentation: Vec::new(),
            music: None,
            techs: Vec::new(),
            research_allowed: Vec::new(),
            construction: Vec::new(),
            resource_units: vec![0; 2],
            pending_save: None,
            mission_failed: false,
            delete_when_mission_ends: Vec::new(),
            repair_modifiers: Vec::new(),
            docked: Vec::new(),
            intel_ended: false,
            intel_stamp: 0.0,
            speech_end: None,
            sp_state: sp_state::NONE,
            fleet,
            arriving: Vec::new(),
            nislet: None,
            ships_have_arrived: false,
            pending_nis: None,
            hyper: crate::campaign::HyperState::default(),
            names: script.names.iter().map(|(h, n)| (*h, n.clone())).collect(),
            log: Vec::new(),
            frame: 0,
            time: 0.0,
        };
        match carry {
            None => game.log(LogKind::Flow, format!("mission {mission}: opening NIS skipped (nis\\n01.nis); fleet in hyperspace: {}", fleet_summary(&game.fleet))),
            Some(c) => {
                game.hyper.old_auto_launch = c.old_auto_launch;
                game.resource_units[HUMAN_PLAYER] = c.resource_units;
                game.log(LogKind::Flow, format!("mission {mission}: fleet from mission {} in hyperspace: {} ({} RUs)", c.mission, fleet_summary(&game.fleet), c.resource_units));
            }
        }

        // singlePlayerLoadNewLevel: the fleet strength scales the level (0 for mission 1).
        let fleet_modifier = fleet_modifier(&game, &statics, mission);
        let ctx = LevelContext { single_player: true, mission, fleet_modifier, resources: ResourceTweaks::load(src) };
        let (dir, file) = level::mission_paths(mission, None);
        game.log(LogKind::Flow, format!("loading {dir}{file} (fleet modifier {fleet_modifier})"));
        level::load_level(&mut universe, src, &dir, &file, &ctx, &mut game.level)?;
        // levelPreInit: the ships in hyperspace are needed whatever the level excludes.
        for f in &game.fleet {
            level::flag_ship_needed(&mut universe, f.ship_type, true);
        }
        // clPresetShipsToPosition after the level is in.
        level::preset_ships_to_position(&mut universe);
        if carry.is_some() {
            crate::campaign::create_fleet_in_hyperspace(&mut universe, &mut game);
        }
        for w in game.level.warnings.clone() {
            game.log(LogKind::Warning, w);
        }
        for (h, n) in game.level.names.clone() {
            game.names.entry(h).or_insert(n);
        }
        let ships = universe.ship_list.len();
        let teams = game.level.teams.len();
        game.log(
            LogKind::Flow,
            format!(
                "level loaded: {ships} ships, {teams} KAS teams, {} points, {} volumes, {} paths, {} resources, {} derelicts",
                game.level.points.len(),
                game.level.volumes.len(),
                game.level.paths.len(),
                game.level.resources.len(),
                game.level.derelicts.len()
            ),
        );
        if carry.is_none() {
            // levelInit's single-player starting RUs are replaced by the mission's own RUsSet.
            game.resource_units = game.level.resource_units.clone();
            if game.resource_units.len() < 2 {
                game.resource_units.resize(2, 0);
            }
        } else {
            // The end of singlePlayerLoadNewLevel: the upgrades the research gives.
            crate::upgrade::player_update_from_research(&mut universe, HUMAN_PLAYER);
            game.hyper.fade = 1.0;
            game.hyper.fade_reset = true;
        }

        let mut kas = Interpreter::new(script);
        for t in &game.level.teams {
            kas.register_team(t.label);
        }
        // singlePlayerNisStoppedCB sets the state that starts the KAS mission next frame.
        game.sp_state = sp_state::START_KAS;
        Ok(MissionRun { universe, kas, game })
    }

    /// One universe update with the single-player and KAS work in their places.
    pub fn step(&mut self) {
        let MissionRun { universe, kas, game } = self;
        let mut hooks = Hooks { kas, game };
        universe.update_with(&mut hooks);
        // Ships that died this update leave KAS selections and teams.
        let dead: Vec<ShipId> = universe.events.iter().filter_map(|e| if let SimEvent::ShipDied { ship, .. } = e { Some(*ship) } else { None }).collect();
        for id in dead {
            game.ship_removed(id);
        }
        level::update_clamped(universe, &game.level);
    }

    /// Runs `n` updates.
    pub fn run(&mut self, n: u32) {
        for _ in 0..n {
            self.step();
        }
    }

    /// `singlePlayerNisStoppedCB` for a held request: the NIS or NISlet the front end played
    /// has ended, and its completion variable is set (`G_Nis<n>Complete` /
    /// `G_Nislet<n>Complete`). A roll call the player asked for while it played
    /// (`roll_call_requested`, 0x910224) begins now. Returns false when nothing was waiting.
    pub fn nis_finished(&mut self, roll_call_requested: bool) -> bool {
        let Some(req) = self.game.pending_nis.take() else { return false };
        let name = match req {
            NisRequest::Nis(n) => format!("G_Nis{n}Complete"),
            NisRequest::Nislet(n) => format!("G_Nislet{n}Complete"),
        };
        self.kas.set_var(None, var_hash(&name), 1);
        self.game.log(LogKind::Flow, format!("{req:?} finished: {name} = 1"));
        if roll_call_requested {
            let _ = self.hyperspace_button();
        }
        true
    }

    /// The hyperspace button (`HyperspaceRollCallBegin`); see [`crate::campaign`].
    pub fn hyperspace_button(&mut self) -> Result<(), crate::campaign::HyperspaceRefused> {
        crate::campaign::hyperspace_button(&mut self.universe, &mut self.game)
    }

    /// The hyperspace button with the debug override (the original's C key): the jump is
    /// allowed before the mission offers it.
    pub fn force_hyperspace(&mut self) -> Result<(), crate::campaign::HyperspaceRefused> {
        self.game.hyper.hyperspace_override = true;
        self.hyperspace_button()
    }

    /// The roll call's "go now" button.
    pub fn hyperspace_go_now(&mut self) -> bool {
        crate::campaign::go_now(&mut self.universe, &mut self.game)
    }

    /// The roll call's "emergency hyperspace" button.
    pub fn emergency_hyperspace(&mut self) -> bool {
        crate::campaign::emergency_hyperspace(&mut self.universe, &mut self.game)
    }

    /// The fleet has jumped and the next mission is due.
    pub fn mission_complete(&self) -> bool {
        self.game.hyper.level_complete
    }

    /// What reaches the next mission, once [`mission_complete`](Self::mission_complete).
    pub fn carry(&self) -> Option<crate::campaign::Carry> {
        crate::campaign::Carry::take(&self.universe, &self.game)
    }

    /// Every ship's hyperspace window state, arriving and leaving: `(ship, hsState, clipt)`
    /// for ships whose window is open (renderers clip the hull with it).
    pub fn hyperspace_windows(&self) -> Vec<(ShipId, u32, f32)> {
        let mut v: Vec<(ShipId, u32, f32)> = self
            .game
            .arriving
            .iter()
            .filter(|a| a.hs_state != hs_effect::INACTIVE)
            .filter_map(|a| Some((a.ship?, a.hs_state, a.clipt)))
            .collect();
        for (&id, r) in &self.game.hyper.ships {
            if !matches!(r.hs_state, hs_effect::INACTIVE | hs_effect::FINISHED) && self.universe.ship(id).is_some() && self.universe.ship_list.contains(&id) {
                v.push((id, r.hs_state, r.clipt));
            }
        }
        v
    }

    /// The current state hash of a team, by label name.
    pub fn team_state(&self, team: &str) -> Option<Hash> {
        self.kas.team(kas_hash(team)).and_then(|t| t.state)
    }

    /// A KAS variable (`team` `None` for mission scope; `G_` names are global).
    pub fn var(&self, team: Option<&str>, name: &str) -> Option<i32> {
        self.kas.var(team.map(kas_hash), var_hash(name))
    }
}

fn fleet_summary(fleet: &[FleetShip]) -> String {
    fleet
        .iter()
        .map(|f| if f.docked.is_empty() { f.ship_type.to_string() } else { format!("{} (+{} docked)", f.ship_type, f.docked.len()) })
        .collect::<Vec<_>>()
        .join(", ")
}

/// `spFleetModifier` from `BaseFleetStrengthForLevel`: where the fleet's build cost plus RUs
/// falls between the mission's two thresholds.
fn fleet_modifier(game: &Game, statics: &StaticWorld, mission: u32) -> f32 {
    let Some(&(_, lo, hi)) = game.sp_tweaks.base_fleet_strength.iter().find(|b| b.0 == mission) else { return 0.0 };
    if lo == 0.0 || hi <= lo {
        return 0.0;
    }
    let mut total = 0.0f32;
    for f in &game.fleet {
        let cost = |t: ShipType| statics.ship(t).filter(|s| s.ship_class != Some(ShipClass::Mothership)).map_or(0.0, |s| s.build_cost as f32);
        total = st(ld(total) + ld(cost(f.ship_type)));
        for d in &f.docked {
            total = st(ld(total) + ld(cost(d.ship_type)));
        }
    }
    total = st(ld(game.resource_units.first().copied().unwrap_or(0) as f32) + ld(total));
    if total <= lo {
        0.0
    } else if total < hi {
        st((ld(total) - ld(lo)) / (ld(hi) - ld(lo))).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// The parts of the frame the mission adds to `univUpdate`.
struct Hooks<'a> {
    kas: &'a mut Interpreter,
    game: &'a mut Game,
}

impl crate::universe::UpdateHooks for Hooks<'_> {
    fn before_bullet_collisions(&mut self, u: &mut Universe) {
        self.game.frame = u.update_counter;
        self.game.time = u.total_time_elapsed;
        single_player_update(u, self.kas, self.game);
    }

    fn computer_players(&mut self, u: &mut Universe) {
        // aiplayerUpdateAll: KAS is skipped while a hyperspace transition is in progress.
        let state = self.game.sp_state;
        if state != sp_state::NONE && state != sp_state::KAS_STARTED {
            return;
        }
        if kas_frame_due(u.update_counter, SP_KAS_UPDATE_RATE, 0) {
            let mut host = crate::kas_host::SimHost { u, g: self.game };
            self.kas.execute(&mut host);
        }
        // `ShipApplyEMP` reads the mission variable `G_NaggarokEMP` (`kasVarFind`).
        u.kas_naggarok_emp = self.kas.var(None, var_hash("G_NaggarokEMP"));
    }
}

/// The start-point the player's fleet arrives at (`StartPointPlayer`), or the origin.
fn start_point(game: &Game) -> [f32; 4] {
    game.level.point(kas_hash("StartPointPlayer")).unwrap_or([0.0; 4])
}

/// `singlePlayerGameUpdate` (0x5148f0), the parts that run before the mission's exit.
fn single_player_update(u: &mut Universe, kas: &mut Interpreter, game: &mut Game) {
    crate::campaign::fade_update(u, game);
    // G_PlayNis / G_PlayNislet requests (variables at mission scope, destroyed when seen).
    if let Some(n) = kas.remove_var(None, var_hash("G_PlayNis")) {
        if n != 0 && game.config.hold_nis {
            game.log(LogKind::Flow, format!("NIS {n} requested (waiting for the front end)"));
            game.pending_nis = Some(NisRequest::Nis(n));
        } else if n != 0 {
            game.log(LogKind::Flow, format!("NIS {n} requested: not played (counts as finished)"));
            kas.set_var(None, var_hash(&format!("G_Nis{n}Complete")), 1);
        }
    }
    if let Some(n) = kas.var(None, var_hash("G_PlayNislet")) {
        kas.remove_var(None, var_hash("G_PlayNislet"));
        if n != 0 && game.config.hold_nis {
            game.log(LogKind::Flow, format!("NISlet {n} of mission {} requested (waiting for the front end)", game.config.mission));
            game.pending_nis = Some(NisRequest::Nislet(n));
        } else if n != 0 {
            let until = st(ld(u.total_time_elapsed) + ld(game.config.nislet_seconds));
            game.log(LogKind::Flow, format!("NISlet {n} of mission {} requested: not played (finishes at {until:.3}s)", game.config.mission));
            game.nislet = Some((n, until));
        }
    }
    if let Some((n, until)) = game.nislet {
        if !(u.total_time_elapsed < until) {
            game.nislet = None;
            kas.set_var(None, var_hash(&format!("G_Nislet{n}Complete")), 1);
            game.log(LogKind::Flow, format!("NISlet {n} finished: G_Nislet{n}Complete = 1"));
        }
    }

    crate::campaign::update_midlevel(u, game);
    match game.sp_state {
        sp_state::START_KAS => {
            if game.hyper.level_complete {
                // The next level is due: nothing runs until it is loaded.
                return;
            }
            game.sp_state = sp_state::KAS_STARTED;
            game.log(LogKind::Flow, format!("kasMissionStart(\"mission{:02}\")", game.config.mission));
            if crate::campaign::fades_in(game.config.mission) && game.fleet.iter().any(|f| f.carried) {
                game.hyper.fade_arrival_pending = true;
            }
            let mut host = crate::kas_host::SimHost { u, g: game };
            kas.start(&mut host);
        }
        sp_state::ARRIVING => arrival_update(u, kas, game),
        sp_state::WAITING_ROLLCALL | sp_state::WAITING_FLEET_COMMAND | sp_state::SHIPS_LEAVING | sp_state::WHITEOUT | sp_state::FAILED => {
            crate::campaign::update(u, game);
        }
        _ => {}
    }
}

/// `kasfStartLevelByHyperspacingShips`: the player's fleet comes in from hyperspace (state 9;
/// the ships re-enter the universe on the next single-player update).
pub(crate) fn begin_arrival(game: &mut Game) {
    game.sp_state = sp_state::ARRIVING;
    game.log(LogKind::Flow, "hyperspacing ships in (state 9)");
    let fleet = std::mem::take(&mut game.fleet);
    let ids = std::mem::take(&mut game.hyper.fleet_ids);
    for (k, f) in fleet.into_iter().enumerate() {
        let mothership = f.ship_type == ShipType::sMothership;
        game.arriving.push(ArrivingShip {
            ship_type: f.ship_type,
            ship: ids.get(k).copied().flatten(),
            offset: f.offset,
            state: ship_hs::INITIAL,
            time: 0.0,
            hs_state: hs_effect::INACTIVE,
            clipt: 0.0,
            rate: 0.0,
            hs_flags: 0,
            mothership,
            clamped_to_mothership: f.module,
            docked: f.docked.clone(),
            entry: f.carried.then(|| Box::new(f)),
        });
    }
}

/// The arrival branch of `singlePlayerGameUpdate` state 9 (fade-in table entry 0): every
/// arriving ship steps through `UpdateArrivingShip`; when all are in, `G_ShipsHaveArrived`.
fn arrival_update(u: &mut Universe, kas: &mut Interpreter, game: &mut Game) {
    let sp = start_point(game);
    let carried = game.arriving.iter().any(|a| a.entry.is_some());
    let all = if carried && crate::campaign::fades_in(game.config.mission) {
        crate::campaign::fade_in_arrival(u, game, sp)
    } else {
        let mut all = true;
        for i in 0..game.arriving.len() {
            let done = update_arriving_ship(u, game, i, sp);
            all &= done;
        }
        all
    };
    if !all {
        return;
    }
    kas.set_var(None, var_hash("G_ShipsHaveArrived"), 1);
    game.ships_have_arrived = true;
    game.sp_state = sp_state::NONE;
    game.log(LogKind::Flow, "all ships arrived: G_ShipsHaveArrived = 1 (state 0)");
    let arrived = std::mem::take(&mut game.arriving);
    if carried {
        crate::campaign::arrival_complete(u, game);
        return;
    }
    // clAutoLaunch / LaunchAllInternalShipsOfPlayerThatMustBeLaunched: the ships that
    // travelled docked come out.
    for a in &arrived {
        let Some(host) = a.ship else { continue };
        for (k, d) in a.docked.iter().enumerate() {
            launch_new_ship(u, game, d.ship_type, host, k);
        }
    }
}

/// The ship of arriving entry `i`, created when it re-enters the universe (in the original it
/// kept its identity in the hyperspace list; here it is created now and gets the next ship
/// number). A carried ship comes without the mission's starting modules (its own modules are
/// entries of their own) and with the ships that travelled inside it.
pub(crate) fn create_arriving_ship(u: &mut Universe, game: &mut Game, i: usize) -> Option<ShipId> {
    if let Some(id) = game.arriving[i].ship {
        return Some(id);
    }
    let t = game.arriving[i].ship_type;
    let made = match game.arriving[i].entry.clone() {
        Some(e) => crate::campaign::create_carried_ship(u, &e),
        None => u.add_ship(t, HUMAN_PLAYER, Vector::ZERO, None),
    };
    match made {
        Ok(id) => {
            game.arriving[i].ship = Some(id);
            if game.arriving[i].mothership {
                if game.level.motherships.is_empty() {
                    game.level.motherships.push(None);
                }
                game.level.motherships[HUMAN_PLAYER] = Some(id);
            } else if game.arriving[i].clamped_to_mothership {
                if let Some(ms) = game.arriving.iter().find(|o| o.mothership).and_then(|o| o.ship) {
                    level::attach_module(u, &mut game.level, id, ms);
                }
            }
            Some(id)
        }
        Err(e) => {
            game.log(LogKind::Warning, format!("{t}: {e}"));
            game.arriving[i].state = 0;
            None
        }
    }
}

/// Puts a ship that travelled docked next to its host (the launch manoeuvre itself is not
/// modelled).
fn launch_new_ship(u: &mut Universe, game: &mut Game, t: ShipType, host: ShipId, k: usize) {
    let Some(h) = u.ships.get(host) else { return };
    let (pos, coordsys) = (h.posinfo.position, h.rotinfo.coordsys);
    let r = u.statics.ship(h.ship_type).map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let side = if k.is_multiple_of(2) { 1.0 } else { -1.0 };
    let along = (k / 2) as f32 * 200.0;
    let offset = coordsys.col(1).scale(side * r) + coordsys.col(2).scale(along);
    match u.add_ship(t, HUMAN_PLAYER, pos + offset, None) {
        Ok(id) => {
            if let Some(s) = u.ships.get_mut(id) {
                s.rotinfo.coordsys = coordsys;
            }
            game.log(LogKind::Flow, format!("{t} launched from {} as ship {}", u.ships.ship(host).ship_type, id.0));
        }
        Err(e) => game.log(LogKind::Warning, format!("{t}: {e}")),
    }
}

/// `UpdateArrivingShip` (0x513340). Returns true once the ship is fully in.
fn update_arriving_ship(u: &mut Universe, game: &mut Game, i: usize, sp: [f32; 4]) -> bool {
    let now = u.total_time_elapsed;
    let tw = game.sp_tweaks.clone();
    if game.arriving[i].ship.is_none() && create_arriving_ship(u, game, i).is_none() {
        return true;
    }
    let a = game.arriving[i].clone();
    let id = a.ship.expect("arriving ship exists");
    if let Some(s) = u.ships.get_mut(id) {
        s.autostabilize_ship = false;
        s.ship_idle = false;
    }
    match a.state {
        ship_hs::INITIAL => {
            let rot = st(ld(sp[3]) * ld(level::DEG_TO_RAD));
            let m = Matrix::rot_about_z(st(fcos(ld(rot))), st(fsin(ld(rot))));
            let o = m.mul_vec(Vector::new(a.offset.x, -a.offset.y, a.offset.z));
            let p = Vector::new(st(ld(o.x) + ld(sp[0])), st(ld(o.y) + ld(sp[1])), st(ld(o.z) + ld(sp[2])));
            put_ship_outside_at(u, id, p, rot);
            let wait = if a.mothership || a.clamped_to_mothership {
                st(ld(tw.base_waittime) + ld(tw.base_waittime))
            } else {
                let r = u.randy.random(RAN_GAME);
                st(ld(tw.base_waittime) * 3.0 + ld(tw.random_waittime) * 2.328_306_4e-10 * fild_u32(r))
            };
            game.arriving[i].state = ship_hs::WAITING;
            game.arriving[i].time = st(ld(now) + ld(wait));
            if let Some(s) = u.ships.get_mut(id) {
                s.flags &= !sof::HYPERSPACE;
                s.flags |= sof::HIDE;
            }
            crate::effects::hyperspace::note_state(u, id, ship_hs::WAITING, game.arriving[i].hs_state);
            false
        }
        ship_hs::WAITING => {
            if !(a.time < now) {
                return false;
            }
            if let Some(s) = u.ships.get_mut(id) {
                s.posinfo.velocity = Vector::ZERO;
                s.flags &= !sof::HIDE;
            }
            let a = &mut game.arriving[i];
            a.state = ship_hs::DECELERATING;
            a.time = st(ld(tw.decelerate_time) + ld(now));
            // hsStart(ship, HYPERSPACE_SLICE_RATE, arriving, hsShouldDisplayEffect(ship))
            a.rate = tw.slice_rate;
            a.clipt = 1.0;
            a.hs_state = hs_effect::POPUP_OUTOF;
            let hs_state = a.hs_state;
            let display = crate::effects::hyperspace::should_display(u, id);
            crate::effects::hyperspace::start(u, id, hs_state, display);
            crate::effects::hyperspace::note_state(u, id, ship_hs::DECELERATING, hs_state);
            false
        }
        ship_hs::DECELERATING => {
            // hsContinue: the original re-places the window each time the ship is drawn; here
            // once per update, with the state the last frame was drawn with.
            if a.hs_state != hs_effect::INACTIVE && a.hs_state != hs_effect::FINISHED {
                let display = crate::effects::hyperspace::should_display(u, id);
                if display {
                    crate::effects::hyperspace::continue_effect(u, id, a.hs_state, a.clipt, false);
                }
            }
            match a.hs_state {
                hs_effect::INACTIVE => {
                    a.time < now
                }
                hs_effect::SLICING_OUTOF => {
                    // Wait while another capital ship is still popping out or done.
                    let others = game.arriving.iter().enumerate().any(|(j, o)| j != i && (o.hs_state == hs_effect::POPUP_OUTOF || o.hs_state == hs_effect::FINISHED));
                    if !others {
                        hs_step(u, game, i, &tw);
                    }
                    false
                }
                hs_effect::COLLAPSE_OUTOF => {
                    // The mothership collapses last (HYPERSPACE_MOTHERSHIP_ALWAYS_ARRIVES_FIRST
                    // only affects the timing above).
                    let waiting = a.mothership && game.arriving.iter().enumerate().any(|(j, o)| j != i && o.hs_state != hs_effect::INACTIVE);
                    if !waiting {
                        hs_step(u, game, i, &tw);
                    }
                    false
                }
                _ => {
                    hs_step(u, game, i, &tw);
                    false
                }
            }
        }
        _ => false,
    }
}

/// `PutShipOutsideAt` for the campaign's fade-in arrival.
pub(crate) fn put_ship_outside_at_pub(u: &mut Universe, id: ShipId, p: Vector, yaw: f32) {
    put_ship_outside_at(u, id, p, yaw);
}

/// `PutShipOutsideAt`: back in the universe at `p` with the default orientation turned by `yaw`.
fn put_ship_outside_at(u: &mut Universe, id: ShipId, p: Vector, yaw: f32) {
    if let Some(s) = u.ships.get_mut(id) {
        s.flags &= !sof::HIDE;
        s.posinfo.position = p;
        s.posinfo.velocity = Vector::ZERO;
        s.rotinfo.coordsys = DEFAULT_SHIP_MATRIX;
    }
    crate::campaign::bring_outside(u, id);
    level::rotate_yaw(u, id, yaw);
}

/// `hsUpdate` on an arriving ship, with its effect side: the window effect is deleted when
/// the collapse ends.
fn hs_step(u: &mut Universe, game: &mut Game, i: usize, tw: &SpTweaks) {
    let a = &mut game.arriving[i];
    let ended = hs_update(a, tw);
    let (state, hs_state) = (a.state, a.hs_state);
    if let Some(id) = a.ship {
        if ended {
            crate::effects::hyperspace::finish(u, id);
        }
        crate::effects::hyperspace::note_state(u, id, state, hs_state);
    }
}

/// `hsUpdate` (the hyperspace effect's clip plane and state changes). Returns true when the
/// collapse ended (the original then deletes the window effect).
fn hs_update(a: &mut ArrivingShip, tw: &SpTweaks) -> bool {
    hs_update_fields(&mut a.hs_state, &mut a.clipt, &mut a.rate, a.hs_flags, tw)
}

/// [`hs_update`] on the fields themselves (arriving and leaving ships keep them apart).
pub(crate) fn hs_update_fields(hs_state: &mut u32, clipt: &mut f32, rate: &mut f32, hs_flags: u32, tw: &SpTweaks) -> bool {
    let s = *hs_state;
    if s == hs_effect::INACTIVE || s == hs_effect::FINISHED {
        return false;
    }
    let limit: f64 = match s {
        hs_effect::POPUP_INTO | hs_effect::POPUP_OUTOF | hs_effect::COLLAPSE_INTO | hs_effect::COLLAPSE_OUTOF => {
            *clipt = st(ld(*clipt) - ld(*rate) * 2.5);
            0.0
        }
        _ => {
            let k = if 0.0 <= *clipt {
                if tw.clipt_pos_thresh <= *clipt {
                    tw.clipt_pos_start
                } else {
                    tw.clipt_pos_scalar
                }
            } else if *clipt <= tw.clipt_neg_thresh {
                tw.clipt_neg_finish
            } else {
                tw.clipt_neg_scalar
            };
            *clipt = st(ld(*clipt) - ld(st(ld(*rate) * ld(k))));
            -1.0
        }
    };
    if !(ld(*clipt) < limit) {
        return false;
    }
    if hs_flags & 1 != 0 {
        *clipt = limit as f32;
        return false;
    }
    *clipt = 1.0;
    let mut ended = false;
    *hs_state = match s {
        hs_effect::POPUP_INTO => hs_effect::SLICING_INTO,
        hs_effect::POPUP_OUTOF => hs_effect::SLICING_OUTOF,
        hs_effect::SLICING_INTO => hs_effect::COLLAPSE_INTO,
        hs_effect::SLICING_OUTOF => hs_effect::COLLAPSE_OUTOF,
        _ => {
            *clipt = -2.0;
            *rate = 0.0;
            ended = true;
            if s == hs_effect::COLLAPSE_INTO {
                hs_effect::FINISHED
            } else {
                hs_effect::INACTIVE
            }
        }
    };
    ended
}

/// The game's files through [`hwc_formats::gamedata::GameData`] as a [`DataSource`].
pub struct GameDataSource<'a>(pub &'a hwc_formats::gamedata::GameData);

impl DataSource for GameDataSource<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path).ok().flatten()
    }

    fn exists(&self, path: &str) -> bool {
        self.0.contains(path)
    }
}

/// A NIS the mission asked for (the number in `G_PlayNis` / `G_PlayNislet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NisRequest {
    Nis(i32),
    Nislet(i32),
}

/// A presentation call made by KAS, recorded as data.
#[derive(Debug, Clone, PartialEq)]
pub enum Presentation {
    /// `SpeechEvent(event, variable)` at game time `time`.
    Speech { event: i32, variable: i32, time: f32 },
    /// `LocationCard(milliseconds, text)`.
    LocationCard { text: String, milliseconds: i32, time: f32 },
    /// `MusicPlay(track)`.
    Music { track: i32, time: f32 },
}

/// The KAS script of a mission (`"Mission01"`, `"Tutorial1"`): the imported `.kasb` of a data
/// directory, else extracted from the install's `cataclysm.exe`.
pub fn load_kas_mission(gd: &hwc_formats::gamedata::GameData, name: &str) -> Result<KasMission, MissionError> {
    if let Some(bytes) = gd.mission_ir(name).map_err(|e| MissionError(format!("{name}: {e}")))? {
        return hwc_kas::binary::decode(&bytes).map_err(|e| MissionError(format!("{name}.kasb: {e}")));
    }
    let exe = gd.exe_path().ok_or_else(|| MissionError(format!("{name}: no imported mission and no cataclysm.exe to extract it from")))?;
    let bytes = std::fs::read(&exe).map_err(|e| MissionError(format!("{}: {e}", exe.display())))?;
    let all = hwc_kas::extract::extract_each(&bytes).map_err(|e| MissionError(format!("extracting missions: {e}")))?;
    for m in all.into_iter().flatten() {
        if m.name.eq_ignore_ascii_case(name) {
            return Ok(m);
        }
    }
    Err(MissionError(format!("{name}: not found in {}", exe.display())))
}

/// The mission's script name for a mission number (`Mission01`).
pub fn mission_script_name(mission: u32) -> String {
    format!("Mission{mission:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hs_effect_runs_popup_slice_collapse() {
        let tw = SpTweaks { slice_rate: 0.025, ..SpTweaks::default() };
        let mut a = ArrivingShip {
            ship_type: ShipType::sMothership,
            ship: None,
            offset: Vector::ZERO,
            state: ship_hs::DECELERATING,
            time: 0.0,
            hs_state: hs_effect::POPUP_OUTOF,
            clipt: 1.0,
            rate: 0.025,
            hs_flags: 0,
            mothership: false,
            clamped_to_mothership: false,
            docked: Vec::new(),
            entry: None,
        };
        let mut seen = vec![a.hs_state];
        for _ in 0..1000 {
            hs_update(&mut a, &tw);
            if *seen.last().unwrap() != a.hs_state {
                seen.push(a.hs_state);
            }
            if a.hs_state == hs_effect::INACTIVE {
                break;
            }
        }
        assert_eq!(seen, vec![hs_effect::POPUP_OUTOF, hs_effect::SLICING_OUTOF, hs_effect::COLLAPSE_OUTOF, hs_effect::INACTIVE]);
    }

    #[test]
    fn swap_remove_matches_the_original() {
        let mut v = vec![ShipId(1), ShipId(2), ShipId(3), ShipId(2), ShipId(4)];
        assert!(swap_remove_all(&mut v, ShipId(2)));
        assert_eq!(v, vec![ShipId(1), ShipId(4), ShipId(3)]);
    }
}
