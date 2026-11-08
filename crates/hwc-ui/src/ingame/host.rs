//! What the in-game screens need from the game, as a trait the simulation implements
//! ([`GameUiHost`]), and what they ask it to do ([`GameRequest`]).
//!
//! The UI never touches simulation structures: every frame it copies what it shows from the
//! host into a [`HostView`], and everything a click or a key does comes back as a request the
//! game applies in its own update (like the engine's `clWrap*` command layer calls).

use crate::ingame::keys::Action;

/// A ship's identity as the host knows it (index, handle, ...). Stable while the ship lives.
pub type ShipId = u32;
/// A ship type number (`ShipType`, as in `hwc-sim`'s statics: 0x12 sMothership, ...).
pub type ShipType = u32;
/// A technology number (0..41).
pub type TechId = u32;

/// The two playable races of Cataclysm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Race {
    /// Kiith Somtaaw (race 0, "R1" files).
    #[default]
    Somtaaw,
    /// The Beast (race 1, "R2" files).
    Beast,
}

impl Race {
    /// Index used in file names (`build_race1_...`): 1 or 2.
    pub fn file_index(self) -> u32 {
        match self {
            Race::Somtaaw => 1,
            Race::Beast => 2,
        }
    }
}

/// The local player's numbers.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlayerInfo {
    pub race: Race,
    /// Resource units (`player.resourceUnits`).
    pub resource_units: i32,
    /// Support-unit capacity (`player+0xc`), used (`player+0x10`), and needed by the ships
    /// queued at the factories (the build manager adds its own unconfirmed counts).
    pub support_capacity: i32,
    pub support_used: i32,
    pub support_pending: i32,
    /// Ships alive (taskbar / overlay count).
    pub ship_count: i32,
    pub single_player: bool,
    /// A network game (no pause, no saving; the escape menu has no save/load).
    pub multiplayer: bool,
    /// The game is paused ("PAUSED" is shown).
    pub paused: bool,
    /// The sensors manager is open (pan arrows, menu entries).
    pub in_sensors: bool,
    /// Training mission running (`tutorial == 1`).
    pub tutorial: bool,
    /// Mission number (single player), for the objectives screen.
    pub mission: i32,
}

/// A ship that builds or launches ships: the mothership or one of the two carrier slots.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Factory {
    pub ship: ShipId,
    pub ship_type: ShipType,
    /// Display name ("Kuun-Lan", "Carrier"...).
    pub name: String,
    /// The mothership (slot 0); carriers fill slots 1 and 2.
    pub mothership: bool,
}

/// Whether a ship of the construction list can be added (`itemstat` after
/// `cmUpdateShipAvailable`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BuildStatus {
    #[default]
    Buildable,
    /// Not researched / not acquired / not buildable here: the row is not shown.
    Hidden,
    /// Shown greyed: unit cap reached, not enough support units, or the factory is busy.
    Greyed,
}

/// One ship type of the construction list.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BuildOption {
    pub ship_type: ShipType,
    pub name: String,
    /// Ship class (`CLASS_*`: 0 mothership .. 4 frigate are capital ships for the speech).
    pub class: u32,
    /// RU cost of one ship (`buildCost`).
    pub cost: i32,
    /// Support units one ship takes (`static+0x410`).
    pub support_units: i32,
    pub status: BuildStatus,
    /// Ships the unit caps still allow, queued ones counted (None: no cap). Adding more
    /// fails with the cap speech.
    pub cap_left: Option<i32>,
    /// Whether the cap that would stop it is the total-ships cap (speech `0x2001039`) rather
    /// than the type/class cap (`0x2001038`).
    pub total_cap: bool,
    /// Ship description for the extended info view (`svShipDescriptionDraw`).
    pub description: String,
}

/// A row of the construction list: a class heading or a ship (`shipavailable`).
#[derive(Debug, Clone, PartialEq)]
pub enum BuildRow {
    /// Heading text, e.g. "FIGHTER CLASS".
    Class(String),
    Ship(BuildOption),
}

/// One factory's build job for a ship type (`shipinprogress` slot).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BuildJob {
    pub factory: ShipId,
    pub ship_type: ShipType,
    /// Ships still to build, the one in progress included (`nJobs`).
    pub count: i32,
    /// Size of the batch (`nJobsTotal`), for the batch progress bar.
    pub batch_total: i32,
    pub paused: bool,
    /// Progress of the ship being built, 0..1 (`1 - timeLeft / timeStart`).
    pub progress: f32,
    /// RU cost of one ship and RUs already spent on the current one.
    pub cost: i32,
    pub cost_so_far: i32,
}

/// Lab state (`player+0x360`, one per research module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabStatus {
    /// The module is not built.
    #[default]
    Absent,
    Idle,
    Researching,
    /// Held (topic kept, no progress).
    Paused,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Lab {
    /// Module name ("HANGAR MODULE"...), shown over the picture when the lab is selected.
    pub name: String,
    pub status: LabStatus,
    pub topic: Option<TechId>,
    /// 0..1.
    pub progress: f32,
}

/// What the player can do with a technology.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TechState {
    /// Dependencies missing.
    #[default]
    Unavailable,
    /// Can be researched now.
    Available,
    Researching,
    Researched,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tech {
    pub id: TechId,
    pub name: String,
    /// Research cost (RUs) and time (seconds).
    pub cost: i32,
    pub time: f32,
    /// Lab (module) index the tech is researched in (`0x886260[tech]`).
    pub lab: usize,
    pub state: TechState,
    /// A follow-up upgrade rather than a new capability (`kind 3` rows, other colours).
    pub upgrade: bool,
    /// Text of the info panel.
    pub description: String,
    /// Technology key (`TechTypeToString`, e.g. `TECH_CLOAKING`): the picture is
    /// `ResearchGUI\Race<n>\<key>.lif`.
    pub key: String,
}

/// One row of the research list (`rmGetCurrentPrintList` entries).
#[derive(Debug, Clone, PartialEq)]
pub enum ResearchRow {
    /// Module heading.
    Header { name: String },
    /// A technology (index into [`HostView::techs`] by id).
    Tech(TechId),
}

/// A ship docked in a launcher.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DockedShip {
    pub ship: ShipId,
    pub ship_type: ShipType,
    /// Ship class (`CLASS_*`: 5 corvette, 6 fighter, 7 resource...).
    pub class: u32,
    pub name: String,
}

/// A ship that holds docked ships: mothership or carrier.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Launcher {
    pub factory: Factory,
    pub docked: Vec<DockedShip>,
    /// Ships leave as soon as they are built or repaired (`player+0x20` bit); the check box
    /// shows the opposite ("stay docked").
    pub auto_launch: bool,
    /// Fighter / corvette slots in use and available.
    pub fighters: (i32, i32),
    pub corvettes: (i32, i32),
}

/// A mothership module (Somtaaw module panel, `lmModuleSlotsFill`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Module {
    pub ship: ShipId,
    pub ship_type: ShipType,
    pub name: String,
    /// 0..1.
    pub health: f32,
    /// Repair priority 0..2; None when the mothership lacks the repair-priority upgrade
    /// (the buttons are disabled).
    pub repair_priority: Option<i32>,
}

/// A selected ship as the info panel and the overlay show it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ShipInfo {
    pub ship: ShipId,
    pub ship_type: ShipType,
    pub name: String,
    /// Ship class (`CLASS_*`): drives grouping and the legend.
    pub class: u32,
    pub health: f32,
    pub max_health: f32,
    /// Upgrade bits installed (`ship+0x3f4`), researched for the player (`player+0x652c`), and
    /// possible for the type (`suGetShipTypeUpgradeMask`).
    pub upgrades: u32,
    pub upgrades_researched: u32,
    pub upgrades_possible: u32,
    /// Tech level (`ship+0x3f0`) and whether it can still be raised.
    pub tech_level: i32,
    pub tech_level_raisable: bool,
    /// Veterancy rank (`ship+0x5f8`).
    pub rank: i32,
    /// Hot-key group (1..9), if any.
    pub hotkey_group: Option<u8>,
    /// Experience rank name ("ROOKIE"...) and class name ("FIGHTER").
    pub rank_name: String,
    pub class_name: String,
    /// Whose ship: the selected-ships list and the band-box list colour by it.
    pub owner: Owner,
    pub guns: Vec<GunInfo>,
    /// Current order as a short word (taskbar legend categories).
    pub order: OrderKind,
    /// Tactics: 0 evasive, 1 neutral, 2 aggressive.
    pub tactics: i32,
    /// Fuel 0..1 (strike craft) if the ship uses fuel.
    pub fuel: Option<f32>,
    /// Resource units carried (harvesters).
    pub resources: Option<i32>,
}

/// Owner of a listed ship.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Owner {
    #[default]
    Own,
    /// An ally's ship, or one of ours flagged as borrowed/allied-controlled.
    Ally,
    /// Own ship that cannot take orders right now (listed grey).
    Disabled,
    Enemy,
}

/// One gun as the ship info panel lists it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GunInfo {
    /// "SMALL", "MEDIUM", "LARGE", "VERY LARGE".
    pub size: String,
    /// "PROJECTILE", "ENERGY", "ION"...
    pub kind: String,
    /// Damage with upgrades and experience applied.
    pub damage: f32,
    /// A gun upgrade is researched but not installed.
    pub upgrade_pending: bool,
}

/// Order categories of the tactical legend (`TB_Moving`, `TB_Attacking`...).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrderKind {
    #[default]
    Idle,
    Moving,
    Attacking,
    Guarding,
    Docking,
    Other,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Objective {
    pub text: String,
    /// Fleet intelligence text shown when the objective is clicked.
    pub description: String,
    /// The "hyperspace" objective: clicking it jumps instead.
    pub hyperspace: bool,
    pub primary: bool,
    pub complete: bool,
}

/// What the mouse-over tooltip describes (its colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TooltipKind {
    Resource,
    Missile,
    Enemy,
    #[default]
    Own,
    Ally,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tooltip {
    pub text: String,
    pub kind: TooltipKind,
}

/// A line in the game messages panel.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GameMessage {
    pub text: String,
    pub colour: [u8; 4],
    /// Clicking focuses this ship.
    pub ship: Option<ShipId>,
    /// Game time it was posted (seconds).
    pub time: f32,
}

/// Something the right-click menu can offer for the selection (`selGetCommandAvailability`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Command {
    Move,
    Attack,
    Guard,
    Dock,
    Launch,
    Retire,
    Scuttle,
    Cancel,
    Harvest,
    Repair,
    Salvage,
    Upgrade,
    Hyperspace,
    EnterSlipgate,
    Kamikaze,
    Special,
    Link,
    Unlink,
    Leech,
    /// The "Select Leech" submenu (the leech upgrade).
    SelectLeech,
    /// Leeching Leeches let go (`mrLeechStop`).
    Unleech,
    /// The Leeches' drain mode: 6 Spy, 7 Leech, 8 Vent (`mrLeechSet`).
    LeechMode(u8),
    /// The "Select Mimic" and "Mimic" submenus (the disguise upgrade).
    Mimic,
    /// Drop the disguise (`mrMimicUndisguise`).
    UnMimic,
    /// Sentinels raise a force field (`mrProtectShips`).
    ForceField,
    /// Sentinels drop their field formation (`mrFormationRevertSpecial`).
    UnlinkForceField,
    Cloak,
    Swarmers,
    CancelSwarmers,
    BigGun,
    CancelBigGun,
    BeastAttack,
    Emp,
    Ram,
    Repulse,
    FireMissiles,
    Waypoints,
    TacticsEvasive,
    TacticsNeutral,
    TacticsAggressive,
    Formation(Formation),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Formation {
    Delta,
    Broad,
    Delta3D,
    Claw,
    Wall,
    Sphere,
    Custom,
    Parade,
}

/// Everything the game must do for the UI. The UI keeps going without waiting for it: the next
/// frame's [`HostView`] shows the result.
#[derive(Debug, Clone, PartialEq)]
pub enum GameRequest {
    /// Queue ships at a factory (`cmBuildJobsAdd`; RUs are taken gradually while building).
    Build {
        factory: ShipId,
        ship_type: ShipType,
        count: i32,
    },
    /// Take queued ships off a job, never the one in progress (`cmDecrement`).
    Unqueue {
        factory: ShipId,
        ship_type: ShipType,
        count: i32,
    },
    /// Cancel a factory's whole job for a type, refunding what the current ship cost so far
    /// (`cmCancelJobs`).
    CancelBuild {
        factory: ShipId,
        ship_type: ShipType,
    },
    /// Pause or resume a factory's job for a type (`clWrapPauseJobs` / `clWrapUnpauseJobs`).
    PauseBuild {
        factory: ShipId,
        ship_type: ShipType,
        paused: bool,
    },
    /// Start researching a tech in a lab (`rmResearchTechIfLabFree`).
    Research {
        tech: TechId,
        lab: usize,
    },
    /// Stop a lab's research (`rmClearLab`).
    ClearLab {
        lab: usize,
    },
    /// Launch these ships from a launcher (`clWrapLaunchMultipleShips`).
    Launch {
        launcher: ShipId,
        ships: Vec<ShipId>,
    },
    SetAutoLaunch {
        launcher: ShipId,
        on: bool,
    },
    /// Module repair priority (launch manager module panel).
    SetRepairPriority {
        module_type: ShipType,
        priority: i32,
    },
    /// Beast mothership emergency power allocation (`LM_BeastSystemsSet`, 0..5).
    BeastSystems(u32),
    /// A line of the multiplayer chat, to everyone or the allies (`gcInGameChatEntry`).
    Chat { allies: bool, text: String },
    /// Give the selected ships a command (popup menu or key).
    Command(Command),
    /// Narrow the selection to one ship type (clicking the selected-ships list).
    SelectType {
        ship_type: ShipType,
        owner: Owner,
    },
    /// Drop these types from the selection (list lines marked with the multiple-select key).
    DeselectTypes(Vec<(ShipType, Owner)>),
    /// Select one ship and focus the camera on it (clicking a message).
    FocusShip(ShipId),
    /// "Select Mimic": the disguise menu the "Mimic" submenu shows (`MimicSetDisguiseTablesFlag`).
    MimicType(u8),
    /// "Mimic": the selection takes this entry of the picked menu (`pmMimicDisguiseAs`).
    MimicItem(usize),
    /// A key action the game itself handles (camera, selection, time, sensors...), on press
    /// (`down == true`) or release.
    Action {
        action: Action,
        down: bool,
    },
    /// Pause / unpause the game (managers in single player, escape menu, objectives).
    SetPaused(bool),
    /// Escape menu choices.
    SaveGame(String),
    LoadGame(String),
    RestartLevel,
    QuitToMain,
    /// Accept the trader's offer for a tech.
    TraderAccept(TechId),
    TraderLeave,
    /// A key that skips the fleet intel event while the mission's sensors lockout is on
    /// (`smSensorsSkip`, from Escape or Skip_Sensors).
    SkipFleetIntel,
    /// Tab with `hwcTabShipIcons` (a user-requested change): the next ship-icon mode.
    CycleShipIcons,
    /// A UI sound event (`soundEventPlay(NULL, event)`).
    Sound(u32),
    /// A speech event (`speechEventFleet` / `speechEventQueue`), with its parameter (ship type)
    /// where it has one.
    Speech {
        event: u32,
        param: Option<u32>,
    },
    /// The options were accepted in game (apply audio/video, save the configuration).
    ApplyOptions,
    /// Audio options changed in game.
    AudioSettings,
    /// The hyperspace roll call's buttons (`spGoNowHyperspaceCB`, `spEmergencyHyperspaceCB`,
    /// `spAbortHyperspaceCB`).
    HyperspaceGoNow,
    EmergencyHyperspace,
    AbortHyperspace,
}

/// The single-player hyperspace roll call (`HyperspaceRollCall` screen) while it is up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RollCall {
    /// Ships docked so far (0xaaf91c) and still to dock (0xaaf920).
    pub docked: i32,
    pub remaining: i32,
    /// "Quick docking" is unavailable (a ship of the player is in combat); otherwise
    /// "emergency hyperspace" is (`HyperspaceRollCallBegin` disables exactly one of them).
    pub go_now_disabled: bool,
}

/// Everything the in-game screens read from the game.
pub trait GameUiHost {
    fn player(&self) -> PlayerInfo;
    /// Game time in seconds (`universe.totaltimeelapsed`).
    fn game_time(&self) -> f32;
    /// Time since the mission's level loaded (the taskbar clock, `moDrawRUsAndTime`).
    fn mission_time(&self) -> f32;
    /// Ships that build: index 0 the mothership (if any), then carriers.
    fn factories(&self) -> Vec<Factory>;
    /// The construction list (headings and ships, display order) for this factory
    /// (`cmShipsAvailable` after `cmUpdateShipAvailable`).
    fn build_list(&self, factory: ShipId) -> Vec<BuildRow>;
    /// Every factory's queued jobs.
    fn build_jobs(&self) -> Vec<BuildJob>;
    /// Labs 0..5 (Hangar, Armour, Engineering, Advanced, Weapons; the Beast has only lab 0).
    fn labs(&self) -> Vec<Lab>;
    fn techs(&self) -> Vec<Tech>;
    /// Rows of the research list in display order.
    fn research_rows(&self) -> Vec<ResearchRow>;
    /// Mothership and carriers with their docked ships.
    fn launchers(&self) -> Vec<Launcher>;
    /// Somtaaw mothership modules (up to 7, panel order).
    fn modules(&self) -> Vec<Module>;
    /// Beast emergency power allocation (0..5).
    fn beast_systems(&self) -> u32 {
        4
    }
    fn selection(&self) -> Vec<ShipInfo>;
    /// Ships inside the band box being dragged.
    fn selecting(&self) -> Vec<ShipInfo> {
        Vec::new()
    }
    /// Commands available for the current selection (enables popup menu items).
    fn available_commands(&self) -> Vec<Command>;
    fn objectives(&self) -> Vec<Objective>;
    fn messages(&self) -> Vec<GameMessage>;
    /// "Fleet intelligence" text of the objectives screen.
    fn fleet_intel(&self) -> String {
        String::new()
    }
    /// What has been under the mouse long enough for a tooltip (the game applies the
    /// two-second delay).
    fn tooltip(&self) -> Option<Tooltip> {
        None
    }
    /// Whether Escape has something to cancel in the game (move or targeting mode, a
    /// selection, the sensors manager, an NIS...); otherwise it opens the escape menu.
    fn escape_pending(&self) -> bool {
        false
    }
    /// Waypoints are being placed (`piePointModeOnOff(4, ...)`): the waypoint mode (0 linear,
    /// 1 circular, 2 end to end); `None` otherwise.
    fn placing_waypoints(&self) -> Option<u8> {
        None
    }
    /// Player names with colours (multiplayer overlay).
    fn player_names(&self) -> Vec<(String, [u8; 4])> {
        Vec::new()
    }
    /// Screen position of ships for popup-menu target highlights (None when off screen).
    fn ship_screen_pos(&self, _ship: ShipId) -> Option<(i32, i32, i32)> {
        None
    }
    /// The trader's offers (`TM_TechList`).
    fn trader_offers(&self) -> Vec<Tech> {
        Vec::new()
    }
    /// The single-player jump's label while the mission allows it
    /// (`singlePlayerCanHyperspaceGet`, `spMissionEndLabelGet`: "Hyperspace" or "End
    /// Mission"); the right-click menu offers it.
    fn hyperspace_label(&self) -> Option<String> {
        None
    }
    /// The hyperspace roll call, while its screen is up.
    fn roll_call(&self) -> Option<RollCall> {
        None
    }

    /// The Mimic submenus' contents.
    fn mimic_menus(&self) -> MimicMenus {
        MimicMenus::default()
    }

    /// The drain mode all the selected Leeches have (6, 7 or 8; 0 when they differ or there
    /// are none): the "Select Leech" submenu ticks it.
    fn leech_state(&self) -> u8 {
        0
    }
}

/// The disguise menus the special-action menu offers (`pmBuildMenuItems`, items 0x21 / 0x22).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MimicMenus {
    /// "Select Mimic": the disguise menus (slot, name), one per valid player and the asteroids.
    pub types: Vec<(u8, String)>,
    /// The menu picked (`MimicGetDisguiseTablesFlag`).
    pub current: u8,
    /// "Mimic": the picked menu's entries the selection may take.
    pub items: Vec<String>,
}

/// A copy of what the host showed this frame.
#[derive(Debug, Clone, Default)]
pub struct HostView {
    pub player: PlayerInfo,
    pub time: f32,
    /// [`GameUiHost::mission_time`].
    pub mission_time: f32,
    pub factories: Vec<Factory>,
    /// Construction list of the selected factory.
    pub build_list: Vec<BuildRow>,
    pub jobs: Vec<BuildJob>,
    pub labs: Vec<Lab>,
    pub techs: Vec<Tech>,
    pub research_rows: Vec<ResearchRow>,
    pub launchers: Vec<Launcher>,
    pub modules: Vec<Module>,
    pub beast_systems: u32,
    pub selection: Vec<ShipInfo>,
    pub selecting: Vec<ShipInfo>,
    pub commands: Vec<Command>,
    pub objectives: Vec<Objective>,
    pub messages: Vec<GameMessage>,
    pub fleet_intel: String,
    pub tooltip: Option<Tooltip>,
    pub escape_pending: bool,
    pub player_names: Vec<(String, [u8; 4])>,
    pub trader_offers: Vec<Tech>,
    /// Popup target highlights: (ship, x, y, radius).
    pub ship_positions: Vec<(ShipId, i32, i32, i32)>,
    pub hyperspace_label: Option<String>,
    pub roll_call: Option<RollCall>,
    pub mimic: MimicMenus,
    /// [`GameUiHost::leech_state`].
    pub leech_state: u8,
    /// [`GameUiHost::placing_waypoints`].
    pub waypoints: Option<u8>,
}

impl HostView {
    /// Copies the host's state; `factory` picks the construction list.
    pub fn capture(host: &dyn GameUiHost, factory: Option<ShipId>) -> HostView {
        let factories = host.factories();
        let selected = factory
            .filter(|f| factories.iter().any(|x| x.ship == *f))
            .or_else(|| factories.first().map(|f| f.ship));
        let selection = host.selection();
        let ship_positions = selection
            .iter()
            .filter_map(|s| {
                host.ship_screen_pos(s.ship)
                    .map(|(x, y, r)| (s.ship, x, y, r))
            })
            .collect();
        HostView {
            player: host.player(),
            time: host.game_time(),
            mission_time: host.mission_time(),
            build_list: selected.map(|f| host.build_list(f)).unwrap_or_default(),
            factories,
            jobs: host.build_jobs(),
            labs: host.labs(),
            techs: host.techs(),
            research_rows: host.research_rows(),
            launchers: host.launchers(),
            modules: host.modules(),
            beast_systems: host.beast_systems(),
            selection,
            selecting: host.selecting(),
            commands: host.available_commands(),
            objectives: host.objectives(),
            messages: host.messages(),
            fleet_intel: host.fleet_intel(),
            tooltip: host.tooltip(),
            escape_pending: host.escape_pending(),
            player_names: host.player_names(),
            trader_offers: host.trader_offers(),
            ship_positions,
            hyperspace_label: host.hyperspace_label(),
            roll_call: host.roll_call(),
            mimic: host.mimic_menus(),
            leech_state: host.leech_state(),
            waypoints: host.placing_waypoints(),
        }
    }

    pub fn tech(&self, id: TechId) -> Option<&Tech> {
        self.techs.iter().find(|t| t.id == id)
    }

    /// Jobs of one factory.
    pub fn jobs_of(&self, factory: ShipId) -> impl Iterator<Item = &BuildJob> {
        self.jobs.iter().filter(move |j| j.factory == factory)
    }

    pub fn job(&self, factory: ShipId, ship_type: ShipType) -> Option<&BuildJob> {
        self.jobs
            .iter()
            .find(|j| j.factory == factory && j.ship_type == ship_type)
    }
}
