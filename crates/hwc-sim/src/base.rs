//! The base-building layer's shared state: what Cataclysm keeps per player (resource units,
//! support units, unit-cap counters, research and upgrade state), per ship (docking, clamping,
//! construction, resourcing, salvage) and per game (factories, resources, statistics, game
//! options), plus ship creation (`univCreateShip`) and the clean-up when a ship dies.
//!
//! The per-ship fields live in [`ShipBase`] (embedded in [`Ship`]); the per-player and per-game
//! fields in [`BaseState`] (embedded in [`Universe`]). Field docs give the offsets in
//! Cataclysm's `Ship` (0x63c bytes plus ship specifics) and `Player` (0x6530 bytes) structures.

use hwc_math::fpu::ld;
use hwc_math::{Matrix, Vector};

use crate::object::{sof, NonStats, ObjRef, PosInfo, RotInfo, Ship, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::{CreateError, Universe, DEFAULT_SHIP_MATRIX};

/// Number of ship types (`TOTAL_NUM_SHIPS`).
pub const NUM_SHIP_TYPES: usize = ShipType::COUNT;
/// Number of technologies in a technology set.
pub const NUM_TECHS: usize = 41;
/// Number of research labs (one per research module).
pub const NUM_LABS: usize = 5;

/// `Ship.flags` bit set on the module ships `univCreateMothershipModules` clamps to a
/// mothership (bit 30).
pub const SOF_MODULE: u32 = 0x4000_0000;
/// `Ship.flags` bit: the ship is inside another (`SOF_Hide`).
pub const SOF_HIDE: u32 = sof::HIDE;

/// `Ship.flags2` (+0x18) bits.
pub mod flags2 {
    /// The ship is being launched (set by `InitializeDockVars` for purpose 2).
    pub const LAUNCHING: u32 = 0x0000_0001;
    /// Mimic disguise active.
    pub const MIMIC_DISGUISED: u32 = 0x0000_0008;
}

/// `Ship.specialFlags` (+0x1c8) bits used by docking and building.
pub mod special {
    /// Set when a ship docks for purposes 1 and 9 (cleared when the dock order ends).
    pub const DOCK_STAY: u32 = 0x0000_1000;
    /// Out of the dock cone, still clearing the dock point.
    pub const LEAVING_DOCK: u32 = 0x0100_0000;
    /// The ship is counted in the unit-cap totals (`unitCapCreateShip`).
    pub const UNIT_CAP_COUNTED: u32 = 0x2000_0000;
    /// The ship is a salvager (has a salvage specification).
    pub const SALVAGER: u32 = 0x0008_0000;
    /// The salvage tow was started this pass (`salvSpecialTarget` state 10).
    pub const SALVAGE_TOWING: u32 = 0x0800_0000;
    /// The ship is hyperspacing.
    pub const HYPERSPACING: u32 = 0x0020_0000;
    /// The ship is in range of something (repair, `+0x1c8 & 0x400000` picks the combat repair
    /// rate).
    pub const IN_COMBAT_REPAIR: u32 = 0x0040_0000;
}

/// `Ship.specialFlags2` (+0x1cc) bits used by docking and building.
pub mod special2 {
    /// KAS flagged the ship to flicker when hit by an EMP (`+0x1cc & 0x4000`).
    pub const KAS_EMP_FLICKER: u32 = 0x0000_4000;
    /// Materialising: health grows while the ship appears (`+0x1cc & 0x80000`).
    pub const MATERIALISING: u32 = 0x0008_0000;
    /// The ship's own special is busy (sentinels, processors: `+0x1cc & 0x800000`).
    pub const SPECIAL_BUSY: u32 = 0x0080_0000;
    /// External construction in progress: the hull is being built.
    pub const UNDER_CONSTRUCTION: u32 = 0x0000_0008;
    /// The ship is docking (no bump collisions with its dock).
    pub const DOCKING: u32 = 0x0000_0040;
    /// The harvester's resource changed or vanished; look for a new one in the zone.
    pub const HARVEST_LOOK_IN_ZONE: u32 = 0x0400_0000;
    /// The harvesting beam is on.
    pub const HARVESTING: u32 = 0x0000_0800;
    /// Collected resources are not counted (`+0x1cc & 0x200000`).
    pub const NO_RU_GAIN: u32 = 0x0020_0000;
    /// Support units were not charged for this ship.
    pub const NO_SUPPORT_UNITS: u32 = 0x0010_0000;
    /// The ship's support-unit capacity has been added to its player.
    pub const SU_CAPACITY_ADDED: u32 = 0x1000_0000;
    /// The ship is being salvaged (towed to a depot).
    pub const BEING_SALVAGED: u32 = 0x0200_0000;
    /// Unrestricted docking (crystal / special cases).
    pub const DOCK_ANYWHERE: u32 = 0x0010_0000;
    /// Ally docking allowed.
    pub const ALLY_DOCKABLE: u32 = 0x2000_0000;
}

/// `Ship.attributes` (+0x38) bits used here.
pub mod attributes {
    /// Ships may only latch onto this ship (`ATTRIBUTES_...`, 0x400).
    pub const LATCH_ONLY: u32 = 0x0000_0400;
    /// Not launched automatically (0x100).
    pub const DONT_LAUNCH: u32 = 0x0000_0100;
    /// May dock with anyone (0x40000).
    pub const DOCK_WITH_ANYONE: u32 = 0x0004_0000;
    /// The resource is a mission prop: not harvested or found (0x200).
    pub const RESOURCE_UNHARVESTABLE: u32 = 0x0000_0200;
}

/// Game-option bits (`tpGameCreated.flag`, 0x87bfb4).
pub mod game_flags {
    pub const RESEARCH_ENABLED: u32 = 0x0008;
    pub const HARVESTING_ENABLED: u32 = 0x0020;
    pub const RESOURCE_INJECTIONS: u32 = 0x0040;
    pub const RESOURCE_LUMP_SUM: u32 = 0x0080;
    pub const UNIT_CAPS_ENABLED: u32 = 0x0200;
    pub const CARRIER_IS_MOTHERSHIP: u32 = 0x2000;
    pub const HYPERSPACE: u32 = 0x4000;
}

/// A technology set (`TechSet`, 0xa8 bytes): one flag per technology plus a mode word
/// (2 = all technologies, -2 = disabled, 0 = as listed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Techs {
    #[serde(with = "hwc_math::serde_array")]
    pub has: [i32; NUM_TECHS],
    pub mode: i32,
}

impl Default for Techs {
    fn default() -> Self {
        Self { has: [0; NUM_TECHS], mode: 0 }
    }
}

impl Techs {
    pub const MODE_ALL: i32 = 2;
    pub const MODE_DISABLED: i32 = -2;

    /// From a statics dependency set (mode 0).
    pub fn from_set(set: crate::statics::TechSet) -> Self {
        let mut t = Techs::default();
        for tech in set.iter() {
            t.has[tech.index()] = 1;
        }
        t
    }

    /// `rmTechSetHas`.
    pub fn contains(&self, tech: usize) -> bool {
        self.mode == Self::MODE_ALL || self.has.get(tech).is_some_and(|&v| v == 1)
    }

    /// `rmTechSetAdd` on the set alone; [`crate::fow::tech_add`] adds a player's technology
    /// with the fog-of-war refresh the original makes.
    pub fn add(&mut self, tech: usize) {
        if let Some(v) = self.has.get_mut(tech) {
            *v = 1;
        }
    }

    /// `rmTechSetSetMode`.
    pub fn set_mode(&mut self, mode: i32) {
        match mode {
            -2 => self.mode = -2,
            -1 => *self = Techs::default(),
            2 => self.mode = 2,
            _ => self.mode = 0,
        }
    }

    /// `rmTechSetAnd`.
    pub fn and(a: &Techs, b: &Techs) -> Techs {
        if a.mode == 2 && b.mode == 2 {
            return Techs { has: [0; NUM_TECHS], mode: 2 };
        }
        if a.mode == 2 {
            return Techs { has: b.has, mode: 0 };
        }
        if b.mode == 2 {
            return Techs { has: a.has, mode: 0 };
        }
        let mut out = Techs::default();
        for i in 0..NUM_TECHS {
            out.has[i] = a.has[i] & b.has[i];
        }
        out
    }
}

/// Which research table a player uses (`player+0x350`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ResearchTableKind {
    /// `SPRS*`.
    SinglePlayerSomtaaw,
    /// `MPRS*`.
    #[default]
    MultiplayerSomtaaw,
    /// `MPRB*`.
    Beast,
}

/// A research topic (`ResearchTopic`, 0x18 bytes in the player's topic list).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ResearchTopic {
    /// Unique number (stands for the node pointer the labs hold).
    pub id: u32,
    /// +0x10.
    pub tech: usize,
    /// +0x14: time left (seconds; one is taken off per research update).
    pub time_left: f32,
}

/// Lab states (`player+0x360[lab].status`).
pub mod lab_status {
    pub const ABSENT: i32 = 0;
    pub const IDLE: i32 = 1;
    pub const RESEARCHING: i32 = 2;
    pub const PAUSED: i32 = 3;
}

/// A research lab (`player+0x360`, 8 bytes each).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Lab {
    pub status: i32,
    /// The topic being researched (its [`ResearchTopic::id`]).
    pub topic: Option<u32>,
}

/// Per-player base state (fields of Cataclysm's `Player`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlayerBase {
    /// +0x00: the player's mothership.
    pub mothership: Option<ShipId>,
    /// +0x04 `resourceUnits`.
    pub resource_units: i32,
    /// +0x0c: support-unit capacity.
    pub su_capacity: i32,
    /// +0x10: support units in use.
    pub su_used: i32,
    /// +0x18 `playerState` (0 alive, 1 dead).
    pub state: i32,
    /// +0x1c: controlled by the computer.
    pub ai: bool,
    /// +0x20: auto-launch bits (1 = mothership, 2/4/8 = carriers).
    pub auto_launch: u32,
    /// The single-player "may be researched" flags per technology (0xa49a00, set by KAS
    /// `TechSetResearch` / `AllowPlayerToResearch`, cleared only by `gameStart`, so they carry
    /// over from mission to mission). One global array in the original; kept on the human
    /// player here.
    pub research_allowed: Vec<bool>,
    /// +0x2c: ships counted against the unit caps.
    pub total_ships: i32,
    /// +0x30: ships per type (unit caps).
    pub ship_count: Vec<i32>,
    /// +0x274: ships per class (unit caps).
    pub class_count: [i32; ShipClass::COUNT],
    /// +0x29c: modules counted with motherships.
    pub module_count: i32,
    /// +0x2a4 `HasTechnology`.
    pub tech: Techs,
    /// +0x34c `CanDoResearch`.
    pub can_do_research: bool,
    /// +0x350: the research table in use.
    pub research_table: ResearchTableKind,
    /// +0x354: research topics in progress, in list order.
    pub topics: Vec<ResearchTopic>,
    pub next_topic_id: u32,
    /// +0x360: the five labs.
    pub labs: [Lab; NUM_LABS],
    /// +0x388: per ship type, the technologies its upgrades need (acquired by docking for the
    /// Beast).
    pub ship_upgrade_techs: Vec<Techs>,
    /// +0x62b0: ship type acquired (Beast).
    pub acquired: Vec<bool>,
    /// +0x6504: highest tech level researched (0..2).
    pub max_tech_level: i32,
    /// +0x6508: gun upgrade types available.
    pub gun_types: [i32; 9],
    /// +0x652c: researched upgrade bits.
    pub upgrade_bits: u32,
    /// The computer player's difficulty (aiplayer +4: 0 easy, 1 medium, 2 hard).
    pub ai_difficulty: i32,
    /// The computer player's bigotry (how readily it attacks players other than its primary
    /// enemy, 0..100).
    pub ai_bigotry: u8,
    /// The computer player's primary enemy (aiplayer +0x20).
    pub ai_primary_enemy: Option<usize>,
}

impl PlayerBase {
    pub fn new(race: ShipRace) -> Self {
        let mut p = PlayerBase {
            mothership: None,
            resource_units: 0,
            su_capacity: 0,
            su_used: 0,
            state: 0,
            ai: false,
            // gameStart turns every auto-launch bit on.
            auto_launch: 0x1f,
            research_allowed: vec![false; NUM_TECHS],
            total_ships: 0,
            ship_count: vec![0; NUM_SHIP_TYPES],
            class_count: [0; ShipClass::COUNT],
            module_count: 0,
            tech: Techs::default(),
            can_do_research: true,
            research_table: if race == ShipRace::Beast { ResearchTableKind::Beast } else { ResearchTableKind::MultiplayerSomtaaw },
            topics: Vec::new(),
            next_topic_id: 1,
            labs: [Lab::default(); NUM_LABS],
            ship_upgrade_techs: vec![Techs::default(); NUM_SHIP_TYPES],
            acquired: vec![false; NUM_SHIP_TYPES],
            max_tech_level: 0,
            gun_types: [0; 9],
            upgrade_bits: 0,
            ai_difficulty: 0,
            ai_bigotry: 50,
            ai_primary_enemy: None,
        };
        // suPlayerInit: the first gun type is always available.
        p.gun_types[0] = 1;
        p
    }
}

/// Per-player game statistics the base code updates (`gameStats`, stride 0x7f8).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct PlayerStats {
    /// 0xa080b8: total resource units gained.
    pub ru_total: i32,
    /// 0xa080bc: resource units collected by harvesting.
    pub ru_collected: i32,
    /// 0xa080c0: resource units spent on deterministic builds.
    pub ru_spent: i32,
    /// 0xa080cc: resource units gained by retiring ships and the resource trickle.
    pub ru_other: i32,
    /// 0xa08358: ships built per class.
    pub built_per_class: [i32; ShipClass::COUNT],
    /// `totalInjectedResources`: resource units from injections and the lump sum.
    #[serde(default, deserialize_with = "crate::save::added_in_9")]
    pub ru_injected: i32,
    /// 0xa080b0: when the player was killed (`univKillPlayer`; 0 while alive).
    #[serde(default, deserialize_with = "crate::save::added_in_12")]
    pub died_at: f32,
    /// 0xa080d8: crystals the player's depots took in (`salvDepotProcessCrystal`).
    #[serde(default, deserialize_with = "crate::save::added_in_12")]
    pub crystals: i32,
    /// 0xa085d4: enemy ships the player killed, per class; 0xa08850: the player's ships lost,
    /// per class (`gameStatsShipDied`; linked ACVs and MCVs count twice, Swarmers not at all).
    #[serde(default, deserialize_with = "crate::save::added_in_12")]
    pub killed_per_class: [i32; ShipClass::COUNT],
    #[serde(default, deserialize_with = "crate::save::added_in_12")]
    pub lost_per_class: [i32; ShipClass::COUNT],
    /// 0xa08880: players the player put out of the game (`univKillOtherPlayersIfDead`).
    #[serde(default, deserialize_with = "crate::save::added_in_12")]
    pub players_killed: i32,
}

/// The game-wide statistics.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GameStats {
    pub players: Vec<PlayerStats>,
    /// 0xa080a0: resource units collected by everyone.
    pub ru_collected_all: i32,
    /// 0xaaf938: resource units collected by player 0 in single player.
    pub sp_ru_collected: i32,
    /// 0x911a20: ships salvaged, `[8][145]`. `gameStatsAddShipSalvaged` writes
    /// `[type + player * 8]` while the getter reads `[player * 145 + type]`; both are kept as
    /// in the original (see [`GameStats::add_ship_salvaged`]).
    pub salvaged: Vec<i32>,
    /// 0x912df8: ships beastified, written and read like [`GameStats::salvaged`].
    pub beastified: Vec<i32>,
}

impl Default for GameStats {
    fn default() -> Self {
        Self { players: Vec::new(), ru_collected_all: 0, sp_ru_collected: 0, salvaged: vec![0; 8 * NUM_SHIP_TYPES], beastified: vec![0; 8 * NUM_SHIP_TYPES] }
    }
}

impl GameStats {
    /// `gameStatsShipDied` (0x53a340), the counts per class: a ship killed by a player's
    /// weapons (`howDidIDie` 0x29a..0x29c, or 0x3ec) counts for that player, and every ship
    /// counts as lost for its owner; a ship removed by the game (`howDidIDie` 1000) does not
    /// count.
    pub fn ship_died(&mut self, owner: usize, ship_type: ShipType, class: Option<ShipClass>, how: i32, who: i32) {
        if how == 1000 {
            return;
        }
        let killer = if (0x29a..=0x29c).contains(&how) || how == 0x3ec { who } else { 99 };
        let swarmer = matches!(ship_type, ShipType::sSwarmer | ShipType::bSwarmer);
        let twice = matches!(ship_type, ShipType::sACV | ShipType::sMCV | ShipType::bACV);
        let n = if twice { 2 } else { 1 };
        let Some(c) = class.map(|c| c.index()) else { return };
        if killer != owner as i32 && (0..8).contains(&killer) && !swarmer {
            if let Some(p) = self.players.get_mut(killer as usize) {
                p.killed_per_class[c] += n;
            }
        }
        if !swarmer {
            if let Some(p) = self.players.get_mut(owner) {
                p.lost_per_class[c] += n;
            }
        }
    }

    /// `gameStatsAddShipSalvaged` (0x53b180): increments the cell at `type + player * 8` of the
    /// 8 x 145 table (the original's index; the counts of players other than 0 land in other
    /// players' or other types' cells).
    pub fn add_ship_salvaged(&mut self, ship_type: ShipType, player: usize) {
        let i = ship_type.index() + player * 8;
        if let Some(c) = self.salvaged.get_mut(i) {
            *c += 1;
        }
    }

    /// The salvage-count getter (0x53b0e0): `[player * 145 + type]`.
    pub fn ships_salvaged(&self, ship_type: ShipType, player: usize) -> i32 {
        self.salvaged.get(player * NUM_SHIP_TYPES + ship_type.index()).copied().unwrap_or(0)
    }

    /// The beastified count of `univCheckBeastInfectionComplete` (0x53e8f0): the cell at
    /// `type + player * 8`, as for the salvage counts.
    pub fn add_ship_beastified(&mut self, ship_type: ShipType, player: usize) {
        if let Some(c) = self.beastified.get_mut(ship_type.index() + player * 8) {
            *c += 1;
        }
    }

    /// `gameStatsGetShipsBeastified` (0x53b130): `[player * 145 + type]`, or the player's sum
    /// over all types for `None`.
    pub fn ships_beastified(&self, ship_type: Option<ShipType>, player: usize) -> i32 {
        match ship_type {
            Some(t) => self.beastified.get(player * NUM_SHIP_TYPES + t.index()).copied().unwrap_or(0),
            None => (0..NUM_SHIP_TYPES).map(|t| self.beastified.get(player * NUM_SHIP_TYPES + t).copied().unwrap_or(0)).sum(),
        }
    }

    fn player(&mut self, p: usize) -> &mut PlayerStats {
        if self.players.len() <= p {
            self.players.resize(p + 1, PlayerStats::default());
        }
        &mut self.players[p]
    }
}

/// How the game was set up (globals the base code tests).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GameOptions {
    /// `tpGameCreated.flag` (0x87bfb4), see [`game_flags`].
    pub flags: u32,
    /// `tutorial` (0x8e9b50).
    pub tutorial: bool,
    /// `multiPlayerGame` (0x8e9b44): a networked game. The base code then leaves resource
    /// accounting to the deterministic command stream; the simulation always runs the
    /// single-machine path, so this stays false.
    pub multiplayer_network: bool,
    /// Current single-player mission (0xaaf900), 0 outside single player.
    pub mission: i32,
    /// `REFRESH_RESEARCH_*` and the resource trickle use the tunables; the resource-injection
    /// interval (0x87bfa0) and lump-sum frame (0x87bfa8).
    pub injection_interval: u32,
    pub lump_sum_frame: u32,
    /// The "repair bots" video option (0xaf76b4): ships' repair droids are updated (purely
    /// visual; off in the headless simulation).
    pub repair_bots: bool,
}

impl Default for GameOptions {
    fn default() -> Self {
        Self {
            flags: game_flags::RESEARCH_ENABLED | game_flags::HARVESTING_ENABLED,
            tutorial: false,
            multiplayer_network: false,
            mission: 0,
            injection_interval: 0,
            lump_sum_frame: 0,
            repair_bots: false,
        }
    }
}

/// `ClampInfo` (+0x1bc, 0x38 bytes): what a ship is clamped to and what is clamped to it.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ClampInfo {
    /// +0x00: the host (`None` when only hosting slaves).
    pub host: Option<ShipId>,
    /// +0x04: orientation relative to the host.
    pub rel_coordsys: Matrix,
    /// +0x28: position relative to the host, in host coordinates.
    pub rel_position: Vector,
    /// +0x34: the ships clamped to this one (`None` when there are none).
    pub slaves: Option<Vec<ShipId>>,
}

/// Dock-point occupancy of a ship with dock points (+0x4a8): points in use per dock-point type
/// and the reservation of every point.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DockOccupancy {
    #[serde(with = "hwc_math::serde_array")]
    pub used_per_type: [u8; 35],
    pub reserved: Vec<bool>,
}

/// Ships inside a carrier, mothership or other dock (+0x4b4, a list plus two counters).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct InsideShips {
    pub ships: Vec<ShipId>,
    /// +0x0c: fighters and micro ships inside.
    pub fighters: i16,
    /// +0x0e: corvettes and resourcers inside.
    pub corvettes: i16,
}

/// The harvesting routine a resourcer uses (`ship+0x390`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum HarvestFn {
    #[default]
    None,
    /// `R1ResourcerHarvestsAsteroid`.
    Resourcer,
    /// `WorkerUpdateBeam`.
    Worker,
}

/// A salvager's job (`SalvageSpec`, at +0x644 for workers).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SalvageSpec {
    /// [0]: state (0..14).
    pub state: u32,
    /// [1]: the target.
    pub target: Option<ShipId>,
    /// [2]: the depot.
    pub depot: Option<ShipId>,
    /// [4]: approaching the latch point (no bumps with the target).
    pub latching: bool,
    /// [8]: the target's dock point the salvager latches to.
    pub dock_point: Option<usize>,
    /// [9]: shaken off what it was latched to: it stops at its next housekeeping
    /// (`salvCheckDocked`).
    #[serde(default, deserialize_with = "crate::save::added_in_7")]
    pub shaken_off: bool,
}

/// Mothership module break-away state (`ShipModule` specifics, +0x638..).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ModuleSpec {
    /// +0x638: 1 once the module has broken away from its host.
    pub broken_away: bool,
    /// +0x63c: time the drift ends.
    pub drift_end: f32,
    /// +0x640: drift velocity per frame.
    pub drift: Vector,
    /// +0x658.
    pub drifting: bool,
    /// +0x65c: spin angle.
    pub spin: f32,
    /// +0x660: spin axis.
    pub spin_axis: Vector,
    /// +0x66c: explode at the end of the drift (else stop and hide).
    pub explode: bool,
    /// +0x670: time the break-away started.
    pub start: f32,
}

/// Link state of two ships merging (Acolyte + Acolyte -> ACV; +0x650 partner, +0x654 state).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct LinkSpec {
    pub partner: Option<ShipId>,
    /// 0 none, 2 initiator approaching, 3 aligning, 4 merging, 5 passive partner.
    pub state: u32,
    /// Merge timer (frames the link animation runs).
    pub timer: f32,
}

/// Per-ship base state (fields of Cataclysm's `Ship`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShipBase {
    /// +0x1bc `clampInfo`.
    pub clamp: Option<ClampInfo>,
    /// +0x1d4 / +0x1d8: resourcing flags (resource gone, look for another / harvesting).
    pub res_gone: bool,
    pub res_harvesting: bool,
    /// +0x1dc..+0x20c: disable reasons; +0x210 = any of them.
    pub disable_reasons: [bool; 13],
    pub disabled: bool,
    /// +0x214: salvagers latched on, per player.
    pub salvagers: [i32; 8],
    /// +0x238: the player whose salvagers claimed this ship (-1 none).
    pub salvage_owner: i32,
    /// +0x330: resource units carried.
    pub resources: i32,
    /// +0x338: last health regeneration.
    pub last_regen: f32,
    /// +0x353 / +0x354: harvesting state machine.
    pub harvest_state: u8,
    pub harvest_state2: u8,
    /// +0x38c: harvest effect on.
    pub harvest_effect: bool,
    /// +0x390: harvesting routine.
    pub harvest_fn: HarvestFn,
    /// +0x3dc: health multiplier (1).
    pub health_mult: f32,
    /// +0x3e8: external-construction progress (0..1; 1 for ships not built externally).
    pub build_progress: f32,
    /// +0x494 `dockwith`.
    pub dock_with: Option<ShipId>,
    /// +0x498: distance along the dock cone.
    pub dock_dist: f32,
    /// +0x49c: `ShipDocksAtShip` state.
    pub dock_state: u32,
    /// +0x4a0: reserved dock point (-1 none).
    pub dock_point: i32,
    /// +0x4a4: dock timer (universe time).
    pub dock_timer: f32,
    /// +0x4a8: occupancy of this ship's dock points.
    pub occupancy: Option<DockOccupancy>,
    /// +0x4ac: the ship that built this one.
    pub builder: Option<ShipId>,
    /// +0x4b4: ships inside this one.
    pub inside: Option<InsideShips>,
    /// +0x598 / +0x5a0: upgrade in progress and when it ends.
    pub upgrading: bool,
    pub upgrade_end: f32,
    /// +0x5a4: the dock cone normal of the previous frame.
    pub prev_cone_normal: Vector,
    /// +0x615: the depot slot dock point (-1 none).
    pub depot_point: i32,
    /// +0x608: a ship this one does not avoid, nor modules clamped to it, and which does not
    /// avoid this one (a towed message pod's depot, once it is coming up to the bay).
    #[serde(default, deserialize_with = "crate::save::added_in_3")]
    pub avoid_exception: Option<ShipId>,
    /// +0x4bc, +0x300..: the mesh animation clock.
    #[serde(default, deserialize_with = "crate::save::added_in_8")]
    pub mad: Option<crate::mad::MadState>,
    /// +0x660, +0x66c: the heading and up an unlinking ship turns to.
    #[serde(default, deserialize_with = "crate::save::added_in_8")]
    pub link_attitude: (Vector, Vector),
    /// What a linked ship remembers of its two parts, for the unlink.
    #[serde(default, deserialize_with = "crate::save::added_in_8")]
    pub link_parts: Option<crate::link::LinkedParts>,
    /// +0x570..: being eaten by a beam (`consume*`).
    #[serde(default, deserialize_with = "crate::save::added_in_7")]
    pub consume: crate::consume::Consume,
    /// +0x618 / +0x61c: salvage notification and depot re-check times.
    pub salvage_notify_time: f32,
    pub salvage_depot_check: f32,
    /// +0x620: slipgate timer.
    pub slipgate_timer: f32,
    /// Worker harvesting timer (+0x63c).
    pub worker_timer: f32,
    /// A worker's (or other salvager's) salvage job.
    pub salvage: SalvageSpec,
    /// Module break-away state.
    pub module: ModuleSpec,
    /// Ship linking.
    pub link: LinkSpec,
    /// Mesh animation the dock and harvest code waits for (`ship+0x4bc`: index, finished).
    /// Animations complete immediately here (the `.mad` timing is not modelled).
    pub mad_anim: i32,
    pub mad_done: bool,
    pub mad_paused: bool,
    /// Mothership repair priorities per module slot (+0x6a0.., stride 8).
    pub repair_priority: [i32; 7],
    /// +0x3e8: how far a materialising ship has appeared (0 = not started).
    pub materialise_fraction: f32,
    /// +0x5b4: the materialising stopwatch.
    pub materialise_timer: crate::disable::Stopwatch,
    /// Salvage depot slots (carriers, processors, motherships).
    pub depot: Option<Vec<crate::salvage::DepotSlot>>,
}

impl Default for ShipBase {
    fn default() -> Self {
        Self {
            clamp: None,
            res_gone: false,
            res_harvesting: false,
            disable_reasons: [false; 13],
            disabled: false,
            salvagers: [0; 8],
            salvage_owner: -1,
            resources: 0,
            last_regen: 0.0,
            harvest_state: 0,
            harvest_state2: 0,
            harvest_effect: false,
            harvest_fn: HarvestFn::None,
            health_mult: 1.0,
            build_progress: 1.0,
            dock_with: None,
            dock_dist: 0.0,
            dock_state: 0,
            dock_point: -1,
            dock_timer: 0.0,
            occupancy: None,
            builder: None,
            inside: None,
            upgrading: false,
            upgrade_end: 0.0,
            prev_cone_normal: Vector::ZERO,
            depot_point: -1,
            avoid_exception: None,
            mad: None,
            link_parts: None,
            link_attitude: (Vector::ZERO, Vector::ZERO),
            consume: Default::default(),
            salvage_notify_time: f32::from_bits(0x6258_d727),
            salvage_depot_check: 0.0,
            slipgate_timer: 0.0,
            worker_timer: 0.0,
            salvage: SalvageSpec::default(),
            module: ModuleSpec::default(),
            link: LinkSpec::default(),
            mad_anim: -1,
            mad_done: false,
            mad_paused: false,
            repair_priority: [0; 7],
            materialise_fraction: 0.0,
            materialise_timer: crate::disable::Stopwatch::default(),
            depot: None,
        }
    }
}

/// Game-wide base state.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct BaseState {
    pub options: GameOptions,
    /// `universe.curPlayerIndex`: the player whose build manager runs on this machine.
    pub cur_player: usize,
    pub players: Vec<PlayerBase>,
    /// `listofShipsInProgress` (0xae8ce0): the factories.
    pub factories: Vec<crate::construction::Factory>,
    /// Carriers registered as factories (0x8d3af0: ship, player), in registration order.
    pub carriers: Vec<(ShipId, usize)>,
    /// 0xae8a80: ships of each type awaiting creation (network games).
    pub pending_by_type: Vec<i32>,
    pub stats: GameStats,
    /// The resource objects.
    pub resources: crate::resource::ResourceTable,
    /// Per ship type: default upgrade bits (static+0x450, set by `suInitStaticUpgradeBits`
    /// for mission ships; 0 otherwise).
    pub static_upgrade_bits: Vec<u32>,
    /// `resourcesHaveBeenAboveBefore` (0x74aa24).
    pub resources_were_above: bool,
    /// Resource trickle bookkeeping (0xa07f44: frame of the last injection).
    pub last_injection_frame: u32,
    /// `twNotifySalvagersNeededRate` timer (0x8fcbf4).
    pub salvage_notify_time: f32,
    /// What the base code did this update, for presentation.
    pub events: Vec<BaseEvent>,
}

/// Things the base code reports (speech, messages and effects in the original).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum BaseEvent {
    /// A ship finished construction (`cmShipBuildComplete`).
    ShipBuilt { ship: Option<ShipId>, ship_type: ShipType, player: usize },
    /// A ship was launched from its dock.
    ShipLaunched { ship: ShipId, from: ShipId },
    /// A ship docked.
    ShipDocked { ship: ShipId, with: ShipId },
    /// Resource units were delivered.
    ResourcesDelivered { ship: ShipId, player: usize, amount: i32 },
    /// A technology was researched.
    Researched { player: usize, tech: usize },
    /// A ship was salvaged and now belongs to `player`.
    Salvaged { ship: ShipId, player: usize },
    /// Two ships linked into one.
    Linked { new_ship: ShipId },
    /// A linked ship split into two. (Saved games hold these events: new ones go last.)
    Unlinked { old: ShipId, a: ShipId, b: ShipId },
}

impl BaseState {
    /// Base state for players of these races.
    pub fn new(races: &[ShipRace]) -> Self {
        let mut s = BaseState {
            players: races.iter().map(|&r| PlayerBase::new(r)).collect(),
            pending_by_type: vec![0; NUM_SHIP_TYPES],
            static_upgrade_bits: vec![0; NUM_SHIP_TYPES],
            ..Default::default()
        };
        s.stats.players = vec![PlayerStats::default(); races.len()];
        s
    }

    pub fn player(&self, p: usize) -> &PlayerBase {
        &self.players[p]
    }

    pub fn player_mut(&mut self, p: usize) -> &mut PlayerBase {
        &mut self.players[p]
    }

    pub fn single_player_game(u: &Universe) -> bool {
        u.single_player
    }
}

/// `univCreateShip` flags (`param_5`).
pub mod create_flags {
    /// The ship was built (external-build hulls start empty).
    pub const BUILT: u32 = 1;
    /// Do not apply the player's upgrades.
    pub const NO_UPGRADES: u32 = 2;
    /// Do not add to the universe lists (it is created inside its builder).
    pub const NOT_IN_LISTS: u32 = 4;
}

/// How [`create_ship`] creates a ship.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CreateOptions {
    /// See [`create_flags`].
    pub flags: u32,
    /// The ship that built it (`+0x4ac`).
    pub builder: Option<ShipId>,
    /// Do not create the mothership's modules (`param_7`, level loading).
    pub no_modules: bool,
    /// Heading to face (default orientation when `None`).
    pub heading: Option<Vector>,
}

/// The ship type's class.
pub fn class_of(u: &Universe, t: ShipType) -> Option<ShipClass> {
    u.statics.ship(t).and_then(|s| s.ship_class)
}

/// The class as the original's integer (-1 when unknown).
pub fn class_index(u: &Universe, t: ShipType) -> i32 {
    class_of(u, t).map_or(-1, |c| c.index() as i32)
}

/// `univCreateShip` (0x53d3c0): a new ship with everything the base code attaches to it.
pub fn create_ship(
    u: &mut Universe,
    ship_type: ShipType,
    player: usize,
    position: Vector,
    opts: CreateOptions,
) -> Result<ShipId, CreateError> {
    let statics = u.statics.clone();
    let stat = statics.ship(ship_type).ok_or(CreateError::NoStatics(ship_type))?;
    if player >= u.players.len() {
        return Err(CreateError::NoSuchPlayer(player));
    }
    let id = ShipId(u.ship_number);
    u.ship_number = u.ship_number.wrapping_add(1);
    let g = &stat.guidance;
    let mut flags = sof::ROTATABLE | sof::IMPACTABLE | sof::SELECTABLE | sof::TARGETABLE;
    if ld(u.tweaks.verybigship_size) < ld(stat.header.coll.coll_sphere_size) {
        flags |= sof::BIG_OBJECT;
    }
    let mut coordsys = DEFAULT_SHIP_MATRIX;
    if let Some(h) = opts.heading {
        coordsys = Matrix::coord_sys_from_heading(h.normalized());
    }
    let mut ship = Ship::new(id, ship_type, stat.ship_race, player);
    ship.flags = flags;
    ship.posinfo = PosInfo { position, havent_calculated_dist: true, ..Default::default() };
    ship.rotinfo = RotInfo { coordsys, ..Default::default() };
    ship.nonstat = NonStats { thrust_strength: g.thrust_strength, rot_strength: g.rot_strength, turn_speed: g.turn_speed };
    ship.max_velocity_copy = stat.header.max_velocity;
    ship.time_created = u.total_time_elapsed;
    ship.color_scheme = u.players[player].color_scheme;
    ship.guns = crate::gun::init_guns(stat);
    // suShipInit, then the player's upgrades.
    ship.armour_level = crate::upgrade::initial_tech_level(ship_type);
    ship.upgrade_flags = 0;
    if opts.flags & create_flags::NO_UPGRADES == 0 {
        crate::upgrade::apply_to_ship(&mut ship, &u.base, stat);
    }
    // Docking state and the dock-point occupancy.
    ship.base.occupancy = stat.dock.as_ref().map(|d| DockOccupancy { used_per_type: [0; 35], reserved: vec![false; d.points.len()] });
    if stat.can_receive_ships_permanently || stat.can_build_ships || stat.capability_flags & 0x8000 != 0 {
        ship.base.inside = Some(InsideShips::default());
    }
    // External construction starts with an empty hull.
    if stat.external_build && opts.flags & create_flags::BUILT != 0 {
        ship.health = 0.0;
        ship.base.build_progress = 0.0;
        ship.special_flags2 |= special2::UNDER_CONSTRUCTION;
    } else {
        ship.health = stat.health.max_health;
        ship.base.build_progress = 1.0;
        ship.special_flags2 &= !special2::UNDER_CONSTRUCTION;
    }
    crate::physics::update_obj_rot_info(&mut ship, stat);
    u.ships.insert(ship);

    // Factories register when created (not for externally built hulls).
    let is_factory_type = matches!(ship_type, ShipType::sMothership | ShipType::sCarrier | ShipType::bMothership | ShipType::bCarrier);
    if (opts.builder.is_none() || !stat.external_build)
        && (!u.single_player || player == 0)
        && stat.can_build_ships
        && is_factory_type
    {
        crate::construction::add_factory(u, id, stat.can_build_big_ships);
    }
    crate::modules::custom_init(u, id);
    crate::ships::init(u, id);
    crate::mad::set_up_initial(u, id, opts.builder.is_some());
    crate::salvage::depot_init(u, id);
    crate::construction::add_ship_support_units(u, id);
    if opts.flags & create_flags::NOT_IN_LISTS == 0 {
        let mut objs = crate::blobs::Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
        u.blobs.add_to_nearest(ObjRef::Ship(id), &mut objs, &statics);
        u.space_obj_list.push(ObjRef::Ship(id));
        u.ship_list.push(id);
        u.impactable_list.push(ObjRef::Ship(id));
        crate::fow::add_viewer(u, id);
    }
    if ship_type != ShipType::sMothership || !opts.no_modules || u.base.options.mission == 1 {
        crate::modules::create_mothership_modules(u, id, position, player, opts);
    }
    u.ships.ship_mut(id).base.builder = opts.builder;
    crate::visibility::init(u, id);
    // Multiplayer: derelict-race ships (other than crystals) are disabled for good.
    if u.multiplayer_game && stat.ship_race == ShipRace::Derelict && !matches!(ship_type, ShipType::dCrystal0 | ShipType::dCrystal1) {
        u.ships.ship_mut(id).flags2 |= crate::object::flags2::PERMANENTLY_DISABLED;
        crate::disable::disable_for_time(u, id, f32::from_bits(0x6258_d727));
    }
    Ok(id)
}

/// A ship placed by level loading (`scriptSetShipsCB`): `univCreateShip`, the type's default
/// upgrade bits, the unit-cap counters, the lab of a Somtaaw research module, and the player's
/// mothership pointer.
pub fn spawn_ship(u: &mut Universe, ship_type: ShipType, player: usize, position: Vector, heading: Option<Vector>) -> Result<ShipId, CreateError> {
    let id = create_ship(u, ship_type, player, position, CreateOptions { heading, ..Default::default() })?;
    crate::upgrade::init_static_upgrade_bits(u, id);
    crate::construction::unit_cap_create_ship(u, id);
    if u.players[player].race == ShipRace::Somtaaw {
        crate::research::activate_lab(u, player, ship_type, false);
    }
    if matches!(ship_type, ShipType::sMothership | ShipType::bMothership | ShipType::kCarrier | ShipType::tCarrier | ShipType::trFloatingCity) {
        u.base.players[player].mothership = Some(id);
    }
    Ok(id)
}

/// `univRemoveShipFromOutside` (0x542a30): the ship leaves the universe lists (it is going
/// inside another ship) but keeps existing.
/// `fow_viewer` is the original's second argument: the ship stops being a fog-of-war viewer
/// (docking, the instant KAS docks, the end of a hyperspace jump), or stays one (the fleet
/// leaving the level or going into mid-level hyperspace).
pub fn remove_ship_from_outside(u: &mut Universe, id: ShipId, fow_viewer: bool) {
    u.blobs.object_died(ObjRef::Ship(id));
    let s = u.ships.ship_mut(id);
    s.coll_my_blob = None;
    u.ship_list.retain(|&s| s != id);
    u.impactable_list.retain(|&o| o != ObjRef::Ship(id));
    u.space_obj_list.retain(|&o| o != ObjRef::Ship(id));
    u.render_list.retain(|&o| o != ObjRef::Ship(id));
    let s = u.ships.ship_mut(id);
    s.flags |= SOF_HIDE;
    s.recent_attacker = None;
    s.recently_attacked = 0;
    s.firing_at_us = None;
    s.recently_fired_upon = 0;
    s.getting_rocked = None;
    if fow_viewer {
        crate::fow::remove_viewer(u, id);
    }
    // Bullets and other ships stop targeting it.
    crate::command::remove_ship_from_being_targeted(u, id);
}

/// The list part of `dockPutShipOutside`: back into the blob of `near` (or the nearest) and the
/// universe lists.
pub fn add_ship_to_outside(u: &mut Universe, id: ShipId, near: ShipId) {
    let statics = u.statics.clone();
    let blob = u.ships.get(near).and_then(|n| n.coll_my_blob);
    let mut objs = crate::blobs::Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
    match blob {
        Some(b) if u.blobs.get(b).is_some() => u.blobs.add_to_specific(b, ObjRef::Ship(id), &mut objs, &statics),
        _ => u.blobs.add_to_nearest(ObjRef::Ship(id), &mut objs, &statics),
    }
    u.space_obj_list.push(ObjRef::Ship(id));
    u.ship_list.push(id);
    u.impactable_list.push(ObjRef::Ship(id));
    u.ships.ship_mut(id).flags &= !SOF_HIDE;
}

/// `univSetShipPosition` (0x548180): moves the ship, and its fog-of-war viewer with it.
pub fn set_ship_position(u: &mut Universe, id: ShipId, p: Vector) {
    crate::nis_support::set_ship_position(u, id, p);
}

/// `ShipSetDisableReason` (0x...): one of 13 reasons; the ship is disabled while any is set.
pub fn set_disable_reason(s: &mut Ship, on: bool, reason: usize) {
    s.base.disable_reasons[reason] = on;
    s.base.disabled = s.base.disable_reasons.iter().any(|&r| r);
}

/// `univPlayerHasShipOfType` (0x53a6d0): a ship of the type counted for the player, not under
/// construction. The count check reads the unit-cap counters, so with unit caps off only the
/// list walk decides (the counter test is skipped when the counters are not maintained).
pub fn player_has_ship_of_type(u: &Universe, t: ShipType, player: usize) -> bool {
    if u.base.unit_caps_maintained() && u.base.players[player].ship_count[t.index()] == 0 {
        return false;
    }
    u.ship_list.iter().any(|&id| {
        u.ships.get(id).is_some_and(|s| s.ship_type == t && s.player == player && s.special_flags2 & special2::UNDER_CONSTRUCTION == 0)
    })
}

impl BaseState {
    /// Whether the unit-cap counters are maintained (`unitCapsEnabled`, 0x8a74d0).
    pub fn unit_caps_maintained(&self) -> bool {
        true
    }
}

/// `dockAddResourcesToPlayer` (0x45e810) and the identical inlined unloads: credit `amount`
/// resource units to `player`.
pub fn add_resources_to_player(u: &mut Universe, player: usize, amount: i32) {
    if amount <= 0 {
        return;
    }
    u.base.players[player].resource_units += amount;
    let sp = u.single_player;
    let st = u.base.stats.player(player);
    st.ru_collected += amount;
    st.ru_total += amount;
    u.base.stats.ru_collected_all += amount;
    if sp && player == 0 {
        u.base.stats.sp_ru_collected += amount;
    }
}

/// Clean-up when a ship dies (`univDeleteDeadShip` and `clShipDied` for the base layer): the
/// factory and construction jobs, docking, ships inside, clamping, unit caps, labs, support
/// units, salvage and links.
pub fn ship_died(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let (player, ship_type, race) = (s.player, s.ship_type, s.ship_race);
    // Custom died callbacks.
    crate::modules::ship_died(u, id);
    crate::salvage::depot_died(u, id);
    if stat.can_build_ships && (!u.single_player || player == 0) {
        crate::construction::remove_factory(u, id);
    }
    if stat.external_build {
        crate::construction::cancel_external_build_job(u, id);
        u.ships.ship_mut(id).special_flags2 &= !special2::UNDER_CONSTRUCTION;
        if u.ships.ship(id).base.dock_with.is_some() {
            crate::dock::remove_ship_from_docking(u, id);
        }
    }
    // Ships inside die with it.
    if let Some(inside) = u.ships.ship_mut(id).base.inside.take() {
        for sid in inside.ships {
            crate::command::cl_ship_died(u, sid);
            ship_died_inside(u, sid);
        }
    }
    // Anyone docking with or launching from it lets go.
    let others: Vec<ShipId> = u.ships.iter().filter(|o| o.base.dock_with == Some(id) && o.id != id).map(|o| o.id).collect();
    for o in others {
        crate::dock::remove_ship_from_docking(u, o);
    }
    crate::salvage::ship_died(u, id);
    crate::link::ship_died(u, id);
    crate::clamp::unclamp_all_slaves(u, id);
    crate::clamp::unclamp(u, id);
    crate::construction::unit_cap_delete_ship(u, id);
    if race == ShipRace::Somtaaw {
        crate::research::deactivate_lab(u, player, ship_type);
    } else if ship_type == ShipType::bMothership {
        crate::research::deactivate_beast_lab(u, player);
    }
    crate::construction::remove_ship_support_units(u, id);
    if u.base.players[player].mothership == Some(id) {
        u.base.players[player].mothership = None;
    }
}

/// A ship inside a dying ship is wiped out with it.
fn ship_died_inside(u: &mut Universe, id: ShipId) {
    crate::construction::unit_cap_delete_ship(u, id);
    crate::construction::remove_ship_support_units(u, id);
    u.ships.remove(id);
}

/// `univDeleteWipeInsideShipOutOfExistence`: a ship that is inside another (retired, merged)
/// leaves the game.
pub fn wipe_inside_ship(u: &mut Universe, id: ShipId) {
    if let Some(host) = find_ship_i_am_inside(u, id) {
        crate::dock::remove_ship_from_inside(u, id, host);
    }
    if u.ships.get(id).is_none() {
        return;
    }
    // A ship still outside (one arriving from hyperspace, say) leaves its collision blob too.
    u.blobs.object_died(ObjRef::Ship(id));
    crate::command::cl_ship_died(u, id);
    crate::construction::unit_cap_delete_ship(u, id);
    crate::construction::remove_ship_support_units(u, id);
    u.ship_list.retain(|&s| s != id);
    u.impactable_list.retain(|&o| o != ObjRef::Ship(id));
    u.space_obj_list.retain(|&o| o != ObjRef::Ship(id));
    u.render_list.retain(|&o| o != ObjRef::Ship(id));
    u.ships.remove(id);
}

/// `univFindShipIAmInside`.
pub fn find_ship_i_am_inside(u: &Universe, id: ShipId) -> Option<ShipId> {
    u.ships.iter().find(|s| s.base.inside.as_ref().is_some_and(|i| i.ships.contains(&id))).map(|s| s.id)
}


/// Game start for the base layer (`gameStart`): every player's research state.
pub fn init(u: &mut Universe) {
    for p in 0..u.base.players.len() {
        crate::research::init_player(u, p, -1);
    }
}

/// The base-layer work `univUpdate` does after the collision blobs: the build manager
/// (`cmBuildTaskFunction`) and, every `REFRESH_RESEARCH_RATE` frames, research.
pub fn after_blobs(u: &mut Universe) {
    u.base.events.clear();
    crate::construction::build_task(u);
    let (rate, frame) = (u.tweaks.base.refresh_research_rate, u.tweaks.base.refresh_research_frame);
    if rate & u.update_counter == frame {
        crate::research::update_research(u);
    }
}
