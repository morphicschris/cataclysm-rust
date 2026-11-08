//! A running single-player mission: the simulation and its 3D view ([`World`]), the in-game
//! interface (`hwc_ui::ingame::InGameUi`) over a [`SimHost`], the game's own mouse and
//! keyboard handling of the main view (`mainrgn.c`: selection, band boxes, the move pie plate,
//! attack and focus clicks, the right-click menu, camera rotation and zoom), the mission's
//! music, speech and subtitles, and the end of the mission.
//!
//! The window and the headless tests drive it the same way: [`Game::input`] for each event,
//! [`Game::update`] once per frame, [`Game::render`] for the picture.

use std::rc::Rc;

use glam::Vec3;
use hwc_math::timing::{GAME_SPEED_DEFAULT, GAME_SPEED_FACTORS, GAME_SPEED_MAX};
use hwc_render::{EffectBatch, OffscreenTarget, Renderer};
use hwc_sim::base::BaseEvent;
use hwc_sim::harness::group_centre;
use hwc_sim::statics::{ShipType, Technology};
use hwc_sim::{command, dock, formation, ShipId};
use hwc_ui::canvas::{Canvas, Color, Rect};
use hwc_ui::ingame::host::{Command, Formation, GameMessage, GameRequest, Owner};
use hwc_ui::ingame::keys::{self, Action};
use hwc_ui::ingame::strings::Strings;
use hwc_ui::ingame::{InGameUi, InputEvent, Setup};
use hwc_ui::letterbox::Letterbox;
use hwc_ui::session::Session;
use hwc_ui::subtitle::{self, Subtitles};
use hwc_ui::text::{self, Shadow};
use hwc_ui::AssetSource;

use hwc_ui::ingame::cursor::{self, avail, Availability, Pointer, PointerAction};

use crate::audio::Audio;
use crate::bandbox;
use crate::host::{player, selectable, sim_id, SimHost};
use hwc_sim::orders::Order;
use crate::hotkeys::{CarriedGroups, HotKeyGroups};
use crate::intel_trace::IntelTraceKind;
use crate::mission::{Cue, CueReader};
use crate::nis::{self, Kind as NisKind, NisRun};
use crate::pings::{self, Owner as PingOwner, Pings, Tweaks as PingTweaks};
use crate::plate::{self, Plate};
use crate::profile;
use crate::sensors::{self, Phase, Sensors};
use crate::tactical;
use crate::world::World;
use hwc_nis::{Action as NisAction, Fired};

/// Pixels the pointer may move between press and release and still count as a click
/// (`selClickBoxWidth/Height`: a band box needs `|dx| + 1 > 5`).
const CLICK_SLOP: i32 = 5;
/// Seconds between two clicks on the same ship for a double click (as the front end's).
const DOUBLE_CLICK_TIME: f32 = 0.5;
/// Outline of the ships a band box gathers (`selSelectingDraw`, colour at 0x888970).
const SELECTING_COLOR: Color = [192, 192, 0, 255];
/// The tactical overlay's text colour (`TO_TextColor`, 0xffff4040).
const PING_LEGEND_TEXT: Color = [0x40, 0x40, 0xff, 255];
/// Right-button movement that still opens the menu (`MR_MouseMovementClickLimit`).
const RIGHT_CLICK_LIMIT: f32 = 6.0;
/// The mission-complete note stays this long before the game returns to the menu.
const COMPLETE_TIME: f32 = 10.0;
/// The debug key that forces the jump (`singlePlayerCheckDebugKeys`' C key; compiled out of
/// the retail game, here only with [`Game::debug_keys`]).
const DEBUG_HYPERSPACE_KEY: u16 = 0x43;
/// Message colour of a refused jump (`moMessageAdd(..., 0xff0000ff)`).
const RED: [u8; 4] = [255, 0, 0, 255];
/// `hyperspaceWhiteout`'s blue (70, 70, 255).
const WHITEOUT_BLUE: [f32; 3] = [70.0, 70.0, 255.0];
/// UI sound events of the move mechanism (`piePointModeOnOff`, `mrMoveShipsExecute`).
const MOVEMENT_GUI_ON: u32 = plate::SOUND_ON;
const MOVEMENT_GUI_OFF: u32 = plate::SOUND_OFF;
const CLICK: u32 = 0x108;

/// What the application must do after a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Quit to the main menu.
    Leave,
    /// Start the mission again (escape menu "restart").
    Restart,
    /// The options were changed in game (audio levels and the like).
    ApplyOptions,
    /// The between-missions movie `Movies\AnimaticNN.bik` (`animBinkPlay(m, m + 1)`): the
    /// game waits until [`Game::movie_finished`].
    Animatic(u32),
    /// The fleet has jumped: the next mission is due (`singlePlayerLoadNewLevel`) with
    /// `MissionRun::carry`.
    NextMission,
    /// The NIS of the NIS level has stopped (`singlePlayerNisStoppedCB`): the mission loads
    /// (`NisLevelRun::finish`) from the world's NIS level.
    NisLevelDone,
    /// Load this saved game (`gpLoadGame`: the game ends and the saved one loads).
    LoadGame(std::path::PathBuf),
}

/// The modern quick save and quick load keys (`hwcQuickSave`; Shift+F5 and Shift+F9, as the
/// original has none and F5..F9 are its formation keys).
pub const QUICK_SAVE_KEY: u16 = 0x74;
pub const QUICK_LOAD_KEY: u16 = 0x78;
/// How often (in drawn frames) the picture for a saved game's thumbnail is taken.
const THUMB_EVERY: u64 = 30;
/// Thumbnail width in pixels.
const THUMB_WIDTH: u32 = 160;

/// What is under the pointer (`mouseCursorObjPtr`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hover {
    Ship(ShipId),
    Resource(hwc_sim::resource::ResourceId),
}

/// What the left button does in the main view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Mode {
    Select,
    /// The move pie plate is up (`piePointSpecMode`): the destination follows the pointer on
    /// the selection's plane, or up and down its vertical while Enable_Height_Move is held.
    Move(Plate),
}

/// How the mission ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum End {
    Complete { at: f32 },
    Failed,
}

pub struct Game {
    pub world: World,
    pub ui: InGameUi,
    strings: Strings,
    cues: CueReader,
    pub selection: Vec<ShipId>,
    pub(crate) selecting: Vec<ShipId>,
    /// The targets the special-action menu was opened for by a `Ship_Special` band box
    /// (`pmOpenForTargets`); empty for the plain menu.
    pub(crate) special_targets: Vec<ShipId>,
    /// The ship under the pointer when the open menu opened.
    menu_target: Option<ShipId>,
    /// The menu was open after the last check ([`Game::note_menu_open`]).
    menu_was_open: bool,
    /// The context target of the special action highlighted in the open menu
    /// (`pmSetContextTarget`).
    menu_context: Option<(hwc_sim::player_orders::SpecialItem, ShipId)>,
    /// The hot-key groups (`selHotKeyGroup`).
    pub hotkeys: HotKeyGroups,
    /// Groups coming from the mission before, held until the mission's own level loads (an NIS
    /// level plays first).
    pub carried_groups: Option<CarriedGroups>,
    /// The hot-key number fonts by level of detail (`selGroupFont0..3`).
    group_fonts: [Option<Rc<hwc_formats::font::Font>>; 4],
    pub(crate) messages: Vec<GameMessage>,
    /// The local player's name in a multiplayer game (the skirmish screen's player name).
    pub local_name: Option<String>,
    /// The network game this is, if any: orders go through it and it paces the updates.
    pub net: Option<crate::net_game::NetLink>,
    /// The statistics shown at the end of a multiplayer game (`gstatOpen`); the game waits
    /// under them until Done.
    pub end_stats: Vec<hwc_ui::ingame::gamestats::StatPlayer>,
    /// Speech subtitles, simulated subtitles and location cards (`Subtitle.c`).
    pub subtitles: Option<Subtitles>,
    /// The NIS letterbox the mission's `wideScreenIn/Out` drive.
    pub letterbox: Letterbox,
    /// The mission's wide-screen calls as they were applied, with the game time and whether
    /// the sensors manager was open (tests).
    #[cfg(test)]
    pub wide_screen_log: Vec<(hwc_sim::mission::WideScreenEvent, bool)>,
    /// The fleet-intel presentation's trace (speech, subtitles, intel ends, wide screen).
    pub intel_trace: crate::intel_trace::IntelTrace,
    pub mode: Mode,
    mouse: (i32, i32),
    left_down: Option<(i32, i32)>,
    right_down: bool,
    right_moved: f32,
    /// Paused by the UI (escape menu, objectives) or the pause key.
    pub paused: bool,
    /// The time-compression level (`taskTimeCompressionLevel` 0xa585ec), an index into
    /// `GAME_SPEED_FACTORS`; not saved (`universeReset` sets it back to normal).
    pub speed_level: usize,
    pub end: Option<End>,
    pub music: Option<i32>,
    formation_index: usize,
    canvas: Canvas,
    target: Option<OffscreenTarget>,
    /// The 3D view's image-quality options (render scale, MSAA, FXAA); the original look by
    /// default (`hwcRenderScale`, `hwcMsaa`, `hwcFxaa`, `--render-scale`, `--msaa`, `--fxaa`).
    pub quality: hwc_render::RenderQuality,
    /// The window pixels the frame is shown at, when larger than the canvas (a scaled front-end
    /// mode): the 3D view is then drawn at those pixels and shown under the canvas
    /// (`hwcSceneAtWindow`, `--scene-res`). None: drawn at the canvas size.
    pub present: Option<(u32, u32)>,
    /// The main view's 3D pixels as copied into the canvas this frame, when the scene is shown
    /// at window resolution: the canvas pixels that still equal them let it through.
    scene_copy: Vec<[u8; 4]>,
    /// The quality the target was made with.
    target_quality: hwc_render::RenderQuality,
    /// The scene layer is shown this frame.
    layer_active: bool,
    /// Rows at the top and at the bottom of this frame covered by the letterbox bars, and
    /// whether a fade or flash covered the whole picture: drawn over the scene, they must not
    /// let the window-resolution scene show through where they match it (black bars over
    /// black space).
    layer_cover: (i32, bool),
    /// The sensors manager's backdrop dimming and range discs drawn on the CPU canvas, and
    /// its meshes read back whole, as before (for comparing the two).
    pub sensors_cpu_layer: bool,
    /// The modern build manager's ship view (only drawn while that preview is up).
    ship_preview: crate::ship_preview::ShipPreview,
    /// The wheel eases the zoom (`hwcSmoothZoom`, `--zoom`): a user-requested change, on by
    /// default; off: the original's immediate step per notch.
    pub smooth_zoom: bool,
    /// In a scaled front-end mode the 3D view is drawn at the window's pixels
    /// (`hwcSceneAtWindow`, `--scene-res`); the application sets [`present`](Self::present).
    pub scene_at_window: bool,
    big_font: Option<Rc<hwc_formats::font::Font>>,
    real_time: f32,
    /// The sensors manager.
    pub sensors: Sensors,
    /// The game's pointer.
    pub pointer: Pointer,
    /// Overlay pan arrows held (left, right, up, down).
    pan_arrows: [bool; 4],
    /// What is under the pointer this frame.
    pub hover: Option<Hover>,
    hotkey_font: Option<Rc<hwc_formats::font::Font>>,
    /// The NIS playing, if any.
    pub nis: Option<NisRun>,
    /// The next NIS end leaves the letterbox in (`nisUpdateTask`'s 0x8f1f24: set when
    /// mission 1's script starts).
    keep_letterbox_after_nis: bool,
    /// Notes printed once per kind of unhandled NIS event.
    nis_notes: Vec<String>,
    /// The debug keys are on (the C key forces the jump).
    pub debug_keys: bool,
    /// Opt-in (`--rollcall-below-subtitles`): the hyperspace roll call opens below the speech
    /// band instead of at the original's fixed top-left place, where the band and portrait
    /// are drawn over it.
    pub rollcall_below_band: bool,
    /// The between-missions movie is playing: no universe updates until it ends.
    pub movie: bool,
    /// The simulation's `bink_pending` last seen.
    bink_pending: bool,
    /// [`Outcome::NextMission`] was reported.
    next_mission: bool,
    /// The NIS level's NIS has stopped ([`Outcome::NisLevelDone`]).
    nis_level_done: bool,
    /// Development (`--autoplay`): the simulation's test player gives the orders.
    pub autoplay: Option<hwc_sim::autoplay::Autoplay>,
    /// An NIS set up on the loading screen, started on the first frame.
    queued_nis: Option<(NisRun, Vec<Fired>)>,
    /// The sensors manager's pings.
    pub pings: Pings,
    /// The battle pings' blob properties (`pingBob*` of `ping.script`).
    ping_blobs: hwc_sim::tunables::BlobProperties,
    /// The local player's proximity sensors' states after the last update.
    ping_sensor_states: Vec<(ShipId, i32)>,
    /// The level's own pings (anomalous derelicts) were made.
    pings_seeded: bool,
    /// Ping sounds of the last sensors frame, played on the next update.
    ping_sounds: Vec<u32>,
    /// The move plate's colours and readout font (`tweak.script`).
    plate_colors: plate::Colors,
    plate_font: Option<Rc<hwc_formats::font::Font>>,
    /// Where the plate moved the pointer this frame: the application puts the system pointer
    /// there (`mousePositionSet`).
    pointer_warp: Option<(i32, i32)>,
    /// A user-requested change, on by default (`hwcRightClickMove`, `--right-click`): with
    /// ships selected a right click on empty space opens the move plate and Shift+right click
    /// the menu; a right click on a ship or a resource does what it always did. Off: the
    /// original's right click (the menu, or the context-sensitive action).
    pub right_click_move: bool,
    /// Right click confirms the open move plate and left click cancels it: a plate opened by
    /// that right click, or (with [`right_click_move`](Self::right_click_move)) by the move key
    /// or the menu. Otherwise the original's: left confirms, right cancels.
    plate_from_right: bool,
    /// The waypoints placed so far while the plate places waypoints (`mrWaypointMoveBegin`:
    /// 0x9def40, count 0x9df030); `None` for a plain move. Only meaningful with the plate up.
    waypoints: Option<Vec<Vec3>>,
    /// The move plate is picking where the Siege Cannon fires (`piePointModeOnOff(3, ..)`).
    big_gun_plate: bool,
    /// The waypoint mode (0x91a658): 0 linear, 1 circular, 2 end to end (the command's
    /// [`command::waypoint_mode`]). Kept for the session, as the original keeps it.
    waypoint_mode: u32,
    waypoint_rings: plate::WaypointRings,
    waypoint_frame: Option<std::time::Instant>,
    /// When the last scuttle order asked for confirmation (`mrScuttle`'s `confirmTime`,
    /// 0x8b0018, -7 at start).
    scuttle_confirm: f32,
    /// A user-requested change, on by default (`hwcDoubleClickSelectType`): double-clicking
    /// one of the player's ships selects all the player's ships of its type on screen.
    pub double_click_type: bool,
    /// User-requested changes, on by default (see [`crate::tactical`]): lines from the
    /// selected ships to their orders' targets (`hwcOrderLines`, `--order-lines`), class
    /// icons over the ships (`hwcShipIcons`, `--ship-icons`, cycled by Tab) and the selected
    /// ships' health bars outlined (`hwcHealthBrackets`, `--health-bars`).
    pub order_lines: bool,
    pub ship_icons: tactical::IconMode,
    pub health_brackets: bool,
    /// The last plain click on one of the player's ships, for the double click.
    last_ship_click: Option<(ShipId, std::time::Instant)>,
    /// Where the saved games are (none: saving is off, as in the headless tests that do not
    /// set it).
    pub saves: Option<Rc<crate::saves::SaveDir>>,
    /// The latest mission autosave written or loaded (`CurrentLevelName`): "restart level"
    /// loads it.
    pub level_save: Option<std::path::PathBuf>,
    /// The latest picture of the main view for a saved game's thumbnail.
    thumb: Option<hwc_sim::save::Thumbnail>,
    frames: u64,
    /// The music to start on the next update (a loaded game plays its track from the start).
    resume_music: Option<i32>,
    /// A saved game to load on the next update (the quick-load key).
    pending_load: Option<std::path::PathBuf>,
    /// Shift+F5 / Shift+F9 quick save and load (`hwcQuickSave`, off by default).
    pub quick_save_keys: bool,
}

/// A factory job with ships still to build: (factory, job, ship type).
type PendingJob = (ShipId, usize, ShipType);

fn v3(v: hwc_math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

impl Game {
    /// The interface for a started mission. `session` provides the options, key bindings and
    /// the language.
    pub fn new(
        source: Rc<dyn AssetSource>,
        session: &Session,
        world: World,
    ) -> Result<Game, String> {
        let options = session.fe.game.options.clone();
        let ig = &session.fe.game.ig;
        let ui = InGameUi::new(
            source.clone(),
            Setup {
                options: options.clone(),
                default_keys: Some(ig.default_keys.to_config()),
                keys: Some(ig.keys.to_config()),
                keys_path: ig.keys_path.clone(),
                options_path: session.fe.game.config_path.clone(),
            },
        )?;
        let strings = ui.state().strings.clone();
        let read = |name: &str| {
            source
                .read(name)
                .and_then(|d| hwc_formats::script::ScriptFile::parse_bytes(&d).ok())
        };
        let tweaks = sensors::Tweaks::from_scripts(
            read("sensors.script").as_ref(),
            read("cameracommand.script").as_ref(),
        );
        let icons = read("Tactical.script")
            .map(|s| sensors::Icons::from_script(&s))
            .unwrap_or_default();
        let tweak = read("tweak.script");
        let pings = Pings::new(PingTweaks::from_script(read("ping.script").as_ref()));
        let ping_blobs = pings::blob_properties(source.as_ref());
        let mut sensors = Sensors::new(tweaks, sensors::Toggles::from_options(&options), icons);
        sensors.instant = options.get("InstantTransition") != 0;
        sensors.follow_focus = options.get("hwcSensorsFollow") != 0;
        sensors.focus_stays = options.get("hwcSensorsFocusStays") != 0;
        let pointer = Pointer::new(hwc_ui::Resources::new(source.clone()));
        let mut res = hwc_ui::Resources::new(source);
        res.set_language(hwc_ui::Language::from_index(options.get("language") as u32));
        let group_fonts = std::array::from_fn(|i| {
            let key = format!("selFontName{i}");
            let name = tweak
                .as_ref()
                .and_then(|t| t.get(&key))
                .and_then(|e| e.string().map(str::to_string))
                .unwrap_or_else(|| format!("TO_{i}.hff"));
            res.font(&name)
        });
        let mut g = Game {
            world,
            ui,
            strings,
            cues: CueReader::default(),
            selection: Vec::new(),
            selecting: Vec::new(),
            special_targets: Vec::new(),
            menu_target: None,
            menu_was_open: false,
            menu_context: None,
            hotkeys: HotKeyGroups::new(),
            carried_groups: None,
            group_fonts,
            messages: Vec::new(),
            local_name: None,
            net: None,
            end_stats: Vec::new(),
            subtitles: None,
            letterbox: Letterbox::default(),
            #[cfg(test)]
            wide_screen_log: Vec::new(),
            intel_trace: crate::intel_trace::IntelTrace::new(),
            mode: Mode::Select,
            mouse: (0, 0),
            left_down: None,
            right_down: false,
            right_moved: 0.0,
            paused: false,
            speed_level: GAME_SPEED_DEFAULT,
            end: None,
            music: None,
            formation_index: 0,
            canvas: Canvas::new(640, 480),
            target: None,
            quality: hwc_render::RenderQuality {
                post: post_fx(&options),
                ..hwc_render::RenderQuality::from_options(
                    options.get("hwcRenderScale"),
                    options.get("hwcMsaa"),
                    options.get("hwcFxaa"),
                )
            },
            present: None,
            scene_copy: Vec::new(),
            target_quality: hwc_render::RenderQuality::default(),
            layer_active: false,
            layer_cover: (0, false),
            sensors_cpu_layer: false,
            ship_preview: Default::default(),
            smooth_zoom: options.get("hwcSmoothZoom") != 0,
            scene_at_window: options.get("hwcSceneAtWindow") != 0,
            big_font: res.font("Arial_13.hff"),
            hotkey_font: res.font("default.hff"),
            real_time: 0.0,
            sensors,
            pointer,
            pan_arrows: [false; 4],
            hover: None,
            nis: None,
            keep_letterbox_after_nis: false,
            pings,
            ping_blobs,
            ping_sensor_states: Vec::new(),
            pings_seeded: false,
            ping_sounds: Vec::new(),
            plate_colors: plate::Colors::from_tweaks(tweak.as_ref()),
            plate_font: res.font(&plate::readout_font_name(tweak.as_ref())),
            pointer_warp: None,
            right_click_move: options.get("hwcRightClickMove") != 0,
            double_click_type: options.get("hwcDoubleClickSelectType") != 0,
            order_lines: options.get("hwcOrderLines") != 0,
            ship_icons: tactical::IconMode::from_option(options.get("hwcShipIcons")),
            health_brackets: options.get("hwcHealthBrackets") != 0,
            last_ship_click: None,
            plate_from_right: false,
            waypoints: None,
            big_gun_plate: false,
            waypoint_mode: 0,
            waypoint_rings: plate::WaypointRings::default(),
            waypoint_frame: None,
            scuttle_confirm: -7.0,
            saves: None,
            level_save: None,
            thumb: None,
            frames: 0,
            resume_music: None,
            pending_load: None,
            quick_save_keys: options.get("hwcQuickSave") != 0,
            nis_notes: Vec::new(),
            queued_nis: None,
            debug_keys: std::env::var_os("HWC_DEBUG_KEYS").is_some(),
            rollcall_below_band: false,
            movie: false,
            bink_pending: false,
            next_mission: false,
            nis_level_done: false,
            autoplay: None,
        };
        // Mission 1's start sets the flag that keeps the letterbox of its first NIS.
        g.keep_letterbox_after_nis = g
            .world
            .mission_run()
            .is_some_and(|r| r.game.config.mission == 1);
        // `hwcOriginalLetterbox` 1: the original's 1.85:1 bars at any screen shape.
        g.letterbox.tall = options.get("hwcOriginalLetterbox") == 0;
        g.world.lens_flares = options.get("enableLensFlare") != 0;
        g.world.full_detail = options.get("hwcFullDetailShips") != 0;
        g.world.focus_zoom = options.get("hwcFocusZoom") != 0;
        g.world.alt_focus_keeps_zoom = options.get("hwcAltFocusKeepsZoom") != 0;
        g.world.zoom_clear = options.get("hwcZoomClear") != 0;
        g.world.render_list = options.get("hwcRenderList") != 0;
        g.world.formation_focus_leader = options.get("hwcFormationFocusFire") != 0;
        g.world.battle_reports = options.get("hwcBattleReports") != 0;
        g.world.chatter_frequency = Some(options.get("opBattleChatter") as f32 * 0.01);
        g.world
            .set_remastered_fx(options.get("hwcRemasteredFx") != 0);
        g.world
            .set_smooth_trails(options.get("hwcSmoothTrails") != 0);
        g.world
            .set_glows_in_proportion(options.get("hwcGlowsInProportion") != 0);
        g.world.modern_fields = options.get("hwcModernFields") != 0;
        g.world.ship_look = ship_look(&options);
        let mut subs = Subtitles::load(&res, true, &g.world.team_schemes());
        if let Some(s) = subs.as_mut() {
            s.language = options.get("language").max(0) as usize;
        }
        g.subtitles = subs;
        Ok(g)
    }

    /// The recreation's own options from the options store, applied at once (the modern
    /// options screen's ACCEPT): the 3D view's quality, the camera and controls, the
    /// letterbox. The interface switches (HUD, managers, menus) are read from the store as
    /// they are drawn.
    pub fn apply_modern_options(&mut self, options: &hwc_ui::options::Options) {
        self.quality = hwc_render::RenderQuality {
            post: post_fx(options),
            ..hwc_render::RenderQuality::from_options(
                options.get("hwcRenderScale"),
                options.get("hwcMsaa"),
                options.get("hwcFxaa"),
            )
        };
        self.smooth_zoom = options.get("hwcSmoothZoom") != 0;
        self.scene_at_window = options.get("hwcSceneAtWindow") != 0;
        self.right_click_move = options.get("hwcRightClickMove") != 0;
        self.double_click_type = options.get("hwcDoubleClickSelectType") != 0;
        self.order_lines = options.get("hwcOrderLines") != 0;
        self.ship_icons = tactical::IconMode::from_option(options.get("hwcShipIcons"));
        self.health_brackets = options.get("hwcHealthBrackets") != 0;
        self.sensors.follow_focus = options.get("hwcSensorsFollow") != 0;
        self.sensors.focus_stays = options.get("hwcSensorsFocusStays") != 0;
        self.letterbox.tall = options.get("hwcOriginalLetterbox") == 0;
        self.world.full_detail = options.get("hwcFullDetailShips") != 0;
        self.world.focus_zoom = options.get("hwcFocusZoom") != 0;
        self.world.alt_focus_keeps_zoom = options.get("hwcAltFocusKeepsZoom") != 0;
        self.world.zoom_clear = options.get("hwcZoomClear") != 0;
        self.world.render_list = options.get("hwcRenderList") != 0;
        self.world.formation_focus_leader = options.get("hwcFormationFocusFire") != 0;
        self.world.battle_reports = options.get("hwcBattleReports") != 0;
        self.world.chatter_frequency = Some(options.get("opBattleChatter") as f32 * 0.01);
        self.world
            .set_remastered_fx(options.get("hwcRemasteredFx") != 0);
        self.world
            .set_smooth_trails(options.get("hwcSmoothTrails") != 0);
        self.world
            .set_glows_in_proportion(options.get("hwcGlowsInProportion") != 0);
        self.world.modern_fields = options.get("hwcModernFields") != 0;
        self.world.ship_look = ship_look(options);
    }

    fn screen_pos(world: &World, size: (u32, u32), id: ShipId) -> Option<(i32, i32, i32)> {
        let s = world.universe().ship(id)?;
        let (lo, hi) = world.screen_box(size, s)?;
        Some((
            ((lo[0] + hi[0]) * 0.5) as i32,
            ((lo[1] + hi[1]) * 0.5) as i32,
            ((hi[0] - lo[0]) * 0.5) as i32,
        ))
    }

    fn size(&self) -> (u32, u32) {
        (
            self.canvas.width.max(1) as u32,
            self.canvas.height.max(1) as u32,
        )
    }

    /// Runs `f` with the UI and a host over the current state.
    fn with_host<R>(&mut self, f: impl FnOnce(&mut InGameUi, &SimHost) -> R) -> R {
        let size = self.size();
        let world = &self.world;
        let pos = move |id: ShipId| Game::screen_pos(world, size, id);
        let host = SimHost {
            world,
            strings: &self.strings,
            selection: &self.selection,
            selecting: &self.selecting,
            special_targets: &self.special_targets,
            messages: &self.messages,
            paused: self.paused,
            // Escape first cancels the move plate, the sensors manager or the selection
            // (`kbcbEscape`).
            escape_pending: self.mode != Mode::Select
                || self.sensors.is_open()
                || !self.selection.is_empty(),
            placing_waypoints: self.placing_waypoints().then_some(self.waypoint_mode as u8),
            in_sensors: self.sensors.is_open(),
            resource_units: world.universe().base.players[player()].resource_units,
            screen_pos: &pos,
            end_stats: &self.end_stats,
        };
        f(&mut self.ui, &host)
    }

    // ---------------------------------------------------------------------------------------
    // Input
    // ---------------------------------------------------------------------------------------

    /// One input event in canvas pixels.
    pub fn input(&mut self, ev: InputEvent, audio: &mut Option<Audio>) {
        if self.nis.is_some() || self.world.nis_level().is_some() {
            // The NIS locks the controls out (`mrDisable`, `mouseDisable`); Escape skips to
            // its end (`kbcbEscape` -> `nisGoToEnd`) once the letterbox is in.
            match ev {
                InputEvent::MouseMove { x, y } => self.mouse = (x, y),
                InputEvent::KeyDown(hwc_ui::consts::key::ESCAPE) => self.skip_nis(),
                _ => {}
            }
            return;
        }
        if self.quick_save_keys && self.ui.modifiers_held() == keys::MOD_SHIFT {
            match ev {
                InputEvent::KeyDown(QUICK_SAVE_KEY) => {
                    let r = self.save_named(crate::saves::QUICK_SAVE);
                    self.report_save(r);
                    return;
                }
                InputEvent::KeyDown(QUICK_LOAD_KEY) => {
                    if let Some(p) = self.quick_save_path().filter(|p| p.is_file()) {
                        self.pending_load = Some(p);
                    } else {
                        self.post("There is no quick save to load.");
                    }
                    return;
                }
                _ => {}
            }
        }
        if self.debug_keys && ev == InputEvent::KeyDown(DEBUG_HYPERSPACE_KEY) {
            let r = self.world.run_mut().force_hyperspace();
            eprintln!("cataclysm: debug hyperspace: {r:?}");
            return;
        }
        if let InputEvent::MouseMove { x, y } = ev {
            let (dx, dy) = (x - self.mouse.0, y - self.mouse.1);
            self.mouse = (x, y);
            if self.right_down {
                // Camera rotation, or zoom with both buttons (`mrCameraMotion`); the sensors
                // camera while the sensors manager is open, nothing while it zooms.
                self.right_moved += (dx.abs() + dy.abs()) as f32;
                if self.sensors.zooming() {
                    return;
                }
                let both = self.left_down.is_some();
                let min = self.zoom_min();
                if both {
                    self.world.user_zoomed();
                }
                self.world
                    .camera
                    .mouse_drag(dx as f32, dy as f32, both, min);
                return;
            }
        }
        // A camera drag ends wherever the button is let go (over the roll call too).
        let ends_drag = self.right_down
            && ev
                == (InputEvent::MouseButton {
                    left: false,
                    down: false,
                });
        let used = self.with_host(|ui, host| ui.handle_input(host, ev));
        if used && !ends_drag {
            return;
        }
        // The sensors viewport takes nothing while it zooms (`smViewportProcess`).
        if self.sensors.zooming()
            && matches!(
                ev,
                InputEvent::MouseButton { .. } | InputEvent::Wheel { .. }
            )
        {
            self.left_down = None;
            self.right_down = false;
            return;
        }
        match ev {
            InputEvent::MouseButton { left: true, down } => {
                if down {
                    self.left_down = Some(self.mouse);
                } else {
                    let start = self.left_down.take();
                    if self.right_down {
                        return;
                    }
                    self.left_release(start, audio);
                }
            }
            InputEvent::MouseButton { left: false, down } => {
                if down {
                    self.right_down = true;
                    self.right_moved = 0.0;
                    // Pressing right while band-boxing stops the band box.
                    self.left_down = None;
                    self.selecting.clear();
                    // The plate holds still while the camera turns (`piePointModePause`).
                    if let Mode::Move(p) = &mut self.mode {
                        p.pause(true);
                    }
                } else if self.right_down {
                    self.right_down = false;
                    if let Mode::Move(p) = &mut self.mode {
                        p.pause(false);
                    }
                    // The roll call locks the orders out (`mrDisabled`): only the camera.
                    if self.right_moved <= RIGHT_CLICK_LIMIT
                        && self.ui.manager() != Some(hwc_ui::ingame::Manager::RollCall)
                    {
                        self.right_click(audio);
                    }
                }
            }
            InputEvent::Wheel { up } => {
                let min = self.zoom_min();
                self.world.user_zoomed();
                if self.smooth_zoom {
                    self.world.camera.wheel_smooth(up, min);
                } else {
                    self.world.camera.wheel(up, min);
                }
            }
            _ => {}
        }
    }

    fn left_release(&mut self, start: Option<(i32, i32)>, audio: &mut Option<Audio>) {
        let size = self.size();
        let p = [self.mouse.0 as f32, self.mouse.1 as f32];
        if let Mode::Move(_) = self.mode {
            // Placing waypoints, a click adds one (`piePointSpecExecute` -> `mrWaypointAdd`).
            if self.placing_waypoints() {
                self.waypoint_add(audio);
                return;
            }
            // A plate opened by right click is confirmed with right click; left cancels it.
            if self.plate_from_right {
                self.mode = Mode::Select;
                sound(audio, MOVEMENT_GUI_OFF);
            } else {
                self.execute_plate(audio);
            }
            return;
        }
        let Some(start) = start else { return };
        let dragged = (start.0 - self.mouse.0).abs() >= CLICK_SLOP
            || (start.1 - self.mouse.1).abs() >= CLICK_SLOP;
        self.selecting.clear();
        if dragged {
            let a = [start.0 as f32, start.1 as f32];
            self.band_box_release(a, p, audio);
            return;
        }
        // A click.
        match self.pick_ship(size, p) {
            Some(id) => self.object_click(id, audio),
            None => {
                // Only two clicks in a row on the same ship make a double click.
                self.last_ship_click = None;
                if !self.ui.action_held(keys::MULTIPLE_SELECT) {
                    self.selection.clear();
                }
            }
        }
    }

    /// Campaign mission 15 (`DAT_008e9b4c` and `DAT_00aaf900 == 15`), whose slipgates are out of
    /// bounds.
    fn on_mission_15(&self) -> bool {
        self.world.mission() == 15
    }

    /// `mrBigGunBeginTargeting` (0x560cf0) and `MothershipBigGunBeginTargeting` (0x5b6ae0): the
    /// player's mothership alone is selected and, when its Siege Cannon has recharged, the move
    /// plate opens in the sensors manager to pick the point to fire at.
    fn begin_big_gun(&mut self, audio: &mut Option<Audio>) {
        use hwc_sim::ships::mothership;
        const RED: [u8; 4] = [255, 0, 0, 255];
        let disabled = self.world.mission_run().is_some_and(|r| r.game.big_gun_disabled);
        let u = self.world.universe_mut();
        if disabled {
            u.messages.push(hwc_sim::speech::Message { key: "strCantFireBigGun", colour: RED, ship: None });
            return;
        }
        let Some(ms) = u.ships.iter().find(|s| s.player == player() && s.ship_type == ShipType::sMothership).map(|s| s.id) else {
            return;
        };
        if !mothership::has_big_gun_module(u, ms) {
            return;
        }
        self.selection = vec![ms];
        if !mothership::can_begin_targeting(u, ms) {
            u.messages.push(hwc_sim::speech::Message { key: "strNotifyBigGunCharging", colour: RED, ship: None });
            return;
        }
        let Some((c, size)) = self.plate_centre() else {
            return;
        };
        self.mode = Mode::Move(Plate::new(c, size));
        self.plate_from_right = false;
        self.waypoints = None;
        self.big_gun_plate = true;
        sound(audio, MOVEMENT_GUI_ON);
        if !self.sensors.is_open() {
            self.toggle_sensors(audio);
        }
    }

    /// Orders the move the plate points at and closes it.
    pub(crate) fn execute_plate(&mut self, audio: &mut Option<Audio>) {
        if let (Mode::Move(plate), true) = (self.mode, self.big_gun_plate) {
            // `MothershipBigGunTargetPicked` (0x5b6700).
            self.big_gun_plate = false;
            let dest = plate.destination();
            if self.sensors.is_open() {
                self.close_sensors(audio);
            }
            let ms: Vec<ShipId> = self.selection.clone();
            if dest.is_finite() {
                self.give(Order::BigGun { ships: ms, point: hwc_math::Vector::new(dest.x, dest.y, dest.z) });
            }
            sound(audio, MOVEMENT_GUI_OFF);
            self.mode = Mode::Select;
            return;
        }
        if let Mode::Move(plate) = self.mode {
            // `piePointSpecExecute` -> `mrMoveShipsExecute`.
            let dest = plate.destination();
            let sel = self.mobile_selection();
            if !sel.is_empty() && dest.is_finite() {
                let c = plate.centre;
                let from = hwc_math::Vector::new(c.x, c.y, c.z);
                let d = hwc_math::Vector::new(dest.x, dest.y, dest.z);
                self.give(Order::Move { ships: sel.clone(), mode: command::waypoint_mode::ONCE, from, waypoints: vec![d] });
                hwc_sim::speech::by_first(self.world.universe_mut(), &sel, hwc_sim::speech::event::MOVE);
            }
            sound(audio, MOVEMENT_GUI_OFF);
            self.mode = Mode::Select;
        }
    }

    /// The player's selectable ships of `id`'s type whose centre is on screen, `id` first.
    pub(crate) fn same_type_on_screen(&self, id: ShipId) -> Vec<ShipId> {
        let size = self.size();
        let u = self.world.universe();
        let Some(kind) = u.ship(id).map(|s| s.ship_type) else {
            return vec![id];
        };
        let mut sel = vec![id];
        for &other in &u.ship_list {
            if other == id || !selectable(u, other) {
                continue;
            }
            let Some(s) = u.ship(other) else { continue };
            if s.player != player() || s.ship_type != kind {
                continue;
            }
            let on_screen = Self::screen_pos(&self.world, size, other).is_some_and(|(x, y, _)| {
                x >= 0 && y >= 0 && x < size.0 as i32 && y < size.1 as i32
            });
            if on_screen {
                sel.push(other);
            }
        }
        sel
    }

    /// `mrObjectClick`.
    fn object_click(&mut self, id: ShipId, audio: &mut Option<Audio>) {
        // A mine: clicking it with ships selected attacks it (it is never the player's).
        if id.mine().is_some() {
            if !self.ui.action_held(keys::BANDBOX_FOCUS) && !self.selection.is_empty() {
                self.attack(&[id]);
            }
            return;
        }
        let u = self.world.universe();
        let Some(s) = u.ship(id) else { return };
        let own = s.player == player();
        if self.ui.action_held(keys::BANDBOX_FOCUS) {
            self.focus_on(vec![id], true, audio);
            return;
        }
        let ctrl_shift = self.ui.action_held(keys::BANDBOX_FORCE_ATTACK);
        if !own || ctrl_shift {
            if !self.selection.is_empty() && !self.selection.contains(&id) {
                self.attack(&[id]);
            }
            return;
        }
        if !selectable(u, id) {
            return;
        }
        if self.ui.action_held(keys::MULTIPLE_SELECT) {
            if let Some(i) = self.selection.iter().position(|&x| x == id) {
                self.selection.remove(i);
            } else {
                self.selection.push(id);
            }
        } else {
            let now = std::time::Instant::now();
            let double = self.double_click_type
                && self.last_ship_click.is_some_and(|(last, t)| {
                    last == id && now.duration_since(t).as_secs_f32() <= DOUBLE_CLICK_TIME
                });
            if double {
                self.last_ship_click = None;
                self.selection = self.same_type_on_screen(id);
                sound(audio, CLICK);
                return;
            }
            self.last_ship_click = Some((id, now));
            // `mrFormationShips`: the clicked ship's formation comes with it.
            let mut sel = vec![id];
            if let Some(c) = s.command.and_then(|c| u.commands.get(c)) {
                if c.attributes & command::attr::FORMATION != 0 {
                    sel = c
                        .selection
                        .iter()
                        .copied()
                        .filter(|&x| selectable(u, x))
                        .collect();
                }
            }
            self.selection = sel;
            self.speak_selected();
        }
        sound(audio, CLICK);
    }

    /// Right click without moving the camera: the menu (`mrRightClickMenu`).
    fn right_click(&mut self, audio: &mut Option<Audio>) {
        // Placing waypoints the menu offers ending, cancelling and the waypoint mode.
        if self.placing_waypoints() {
            self.with_host(|ui, host| ui.open_popup(host, false));
            return;
        }
        if let Mode::Move { .. } = self.mode {
            if self.plate_from_right {
                self.execute_plate(audio);
            } else {
                self.mode = Mode::Select;
                sound(audio, MOVEMENT_GUI_OFF);
            }
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        // The user-requested right click: on empty space the move plate, Shift for the menu.
        if self.right_click_opens_plate() {
            self.start_move(audio);
            self.plate_from_right = matches!(self.mode, Mode::Move(_));
            return;
        }
        // `mrRightClickSmartOrder`: with a context-sensitive scheme (or the user-requested
        // right click) the action under the pointer, otherwise (and in the sensors manager)
        // the menu.
        if !self.sensors.is_open() && self.smart_right_click() {
            if let Some(h) = self.hover {
                let in_sel = matches!(h, Hover::Ship(id) if self.selection.contains(&id));
                if !in_sel && self.smart_order(h) {
                    return;
                }
            }
        }
        self.with_host(|ui, host| ui.open_popup(host, false));
    }

    /// With [`right_click_move`](Self::right_click_move): a right click with nothing under
    /// the pointer, Shift not held and ships that can move selected opens the move plate.
    fn right_click_opens_plate(&self) -> bool {
        self.right_click_move
            && self.hover.is_none()
            && !self.ui.action_held(keys::MULTIPLE_SELECT)
            && !self.ui.action_held(keys::ENABLE_HEIGHT_MOVE)
            && !self.mobile_selection().is_empty()
    }

    /// Takes the pointer position the plate asked for this frame (the window code moves the
    /// system pointer there).
    pub fn take_pointer_warp(&mut self) -> Option<(i32, i32)> {
        self.pointer_warp.take()
    }

    /// A context-sensitive action scheme is on (`RightClick_CSA` or `Homeworld_CSA`).
    fn context_scheme(&self) -> bool {
        let o = &self.ui.fe.game.options;
        o.get("RightClick_CSA") != 0 || o.get("Homeworld_CSA") != 0
    }

    /// A right click on an object does its context-sensitive action: with a context-sensitive
    /// scheme, or with the user-requested right click ([`right_click_move`](Self::right_click_move))
    /// unless Shift (which asks for the menu) is held.
    fn smart_right_click(&self) -> bool {
        self.context_scheme()
            || (self.right_click_move && !self.ui.action_held(keys::MULTIPLE_SELECT))
    }

    /// The classifier's action on an object (`mouseCursorActionClassify`).
    fn classify(&self, h: Hover) -> PointerAction {
        let sel = self.availability(&self.selection);
        let (tgt, facts) = self.target_facts(h);
        let classic = self.ui.fe.game.options.get("Homeworld_CSA") != 0;
        cursor::classify(&sel, &tgt, self.selection.len(), &facts, classic)
    }

    /// Carries out the right-click action on an object; false opens the menu instead.
    /// What a Mimic order on the object takes the shape of: a ship, or an asteroid (not a dust
    /// cloud or nebula).
    fn mimic_target(&self, h: Hover) -> Option<hwc_sim::player_orders::MimicTarget> {
        use hwc_sim::player_orders::MimicTarget;
        match h {
            Hover::Ship(id) => Some(MimicTarget::Ship(id)),
            Hover::Resource(r) => {
                let res = self.world.universe().base.resources.get(r)?;
                matches!(res.kind, hwc_sim::resource::ResourceKind::Asteroid(_))
                    .then_some(MimicTarget::Asteroid)
            }
        }
    }

    fn smart_order(&mut self, h: Hover) -> bool {
        if self.classify(h) == PointerAction::Mimic {
            // `mrMimicDisguiseAsTarget`.
            if let Some(m) = self.mimic_target(h) {
                let sel = self.selection.clone();
                self.give(Order::MimicAsTarget { ships: sel, target: m });
                return true;
            }
        }
        match (self.classify(h), h) {
            (PointerAction::Harvest, Hover::Resource(r)) => {
                let sel = self.selection.clone();
                self.harvest(&sel, Some(r));
                true
            }
            (PointerAction::Attack, Hover::Ship(t)) => {
                self.attack(&[t]);
                true
            }
            (
                PointerAction::SlipgateSmall | PointerAction::SlipgateMedium | PointerAction::SlipgateLarge,
                Hover::Ship(t),
            ) => {
                let sel = self.selection.clone();
                let mission_15 = self.on_mission_15();
                self.give(Order::DockAtSlipgate { ships: sel, gate: t, mission_15 });
                true
            }
            (PointerAction::Dock, Hover::Ship(t)) => {
                let sel = self.selection.clone();
                self.dock(&sel, dock::purpose::DOCK, Some(t));
                true
            }
            // Sentinels shield the ally (`mrProtectShips`).
            (PointerAction::ForceField, Hover::Ship(t)) => {
                let sel = self.selection.clone();
                self.give(Order::ProtectShips { ships: sel, targets: Some(vec![t]) });
                true
            }
            // The special orders on the object (`mrSpecialTargetOrder(target, order)`).
            (a, Hover::Ship(t)) => {
                use hwc_sim::player_orders::TargetOrder as O;
                let order = match a {
                    PointerAction::Salvage => O::Salvage,
                    PointerAction::Repair => O::Repair,
                    PointerAction::Leech => O::Leech,
                    PointerAction::Swarm => O::Swarm,
                    PointerAction::Emp => O::Emp,
                    PointerAction::BeastWeapon => O::Infect,
                    PointerAction::Ramming => O::Ram,
                    PointerAction::FireMissile => O::LinkMissiles,
                    _ => return false,
                };
                let sel = self.selection.clone();
                self.give(Order::SpecialTarget { ships: sel, targets: Some(vec![t]), order });
                true
            }
            _ => false,
        }
    }

    /// Notes the ship under the pointer when the menu opens (the user-requested right click
    /// gives the menu's special actions that ship as their target).
    fn note_menu_open(&mut self) {
        let open = hwc_ui::ingame::popup::is_open(&self.ui.fe);
        if open && !self.menu_was_open {
            self.menu_target = match self.hover {
                Some(Hover::Ship(id)) => Some(id),
                _ => None,
            };
        }
        self.menu_was_open = open;
    }

    /// The targets of a special action picked from the menu: the band-boxed ones, else (with
    /// the user-requested right click) the ship the menu was opened over when it suits the
    /// action, else the action's context target (`pmSetContextTarget`).
    fn menu_targets(&self, item: hwc_sim::player_orders::SpecialItem) -> Vec<ShipId> {
        if !self.special_targets.is_empty() {
            return self.special_targets.clone();
        }
        let u = self.world.universe();
        if self.right_click_move {
            if let (Some(t), Some(kind)) = (self.menu_target, item.target_kind()) {
                if hwc_sim::player_orders::is_special_target(u, player(), t, kind) {
                    return vec![t];
                }
            }
        }
        match self.menu_context {
            Some((i, t)) if i == item && u.ship(t).is_some() => vec![t],
            _ => Vec::new(),
        }
    }

    /// While the menu is open: the highlighted special action's context target (the nearest
    /// suitable object to the first selected ship, `pmUpdateHighlight` / `pmSetContextTarget`)
    /// and the boxes the menu draws around its targets (`pmDrawItemTargetHighlights`).
    fn update_menu_targets(&mut self) {
        let p = &self.ui.fe.game.ig.popup;
        if p.open.is_none() {
            self.menu_context = None;
            self.menu_target = None;
            return;
        }
        let item = p
            .highlight
            .and_then(|i| p.items.get(i))
            .and_then(|it| it.action)
            .and_then(hwc_ui::ingame::command_of)
            .and_then(crate::host::special_item);
        if let (Some(item), true) = (item, self.special_targets.is_empty()) {
            let u = self.world.universe();
            self.menu_context =
                hwc_sim::player_orders::context_target(u, player(), &self.selection, item)
                    .map(|t| (item, t));
        }
        let shown = item.map_or_else(Vec::new, |i| self.menu_targets(i));
        let size = self.size();
        let u = self.world.universe();
        let boxes = shown
            .into_iter()
            .filter_map(|id| {
                let (x, y, r) = Game::screen_pos(&self.world, size, id)?;
                Some((x, y, r, hwc_sim::shockwave::is_ally(u, id, player())))
            })
            .collect();
        self.ui.fe.game.ig.popup.target_boxes = boxes;
    }

    /// `selGetCommandAvailability` over a set of ships, as far as the simulation tells.
    fn availability(&self, ships: &[ShipId]) -> Availability {
        use hwc_sim::upgrade::has_upgrade;
        let u = self.world.universe();
        let mut t = [false; avail::LEN];
        let all = [
            (avail::ALL_LEECH, 0x40),
            (avail::ALL_EMP, 0x200),
            (avail::ALL_MIMIC, 0x20),
            (avail::ALL_HIVE, 0x800),
        ];
        let any = !ships.is_empty();
        for (i, _) in all {
            t[i] = any;
        }
        t[avail::ALL_RAMMING] = any;
        t[avail::ALL_BEAST_CASCADE] = any;
        t[avail::ALL_LINK_MISSILES] = any;
        t[avail::ALL_MISSILES] = any;
        for &id in ships {
            let Some(s) = u.ship(id) else { continue };
            let st = u.statics.ship(s.ship_type);
            let class = st.and_then(|st| st.ship_class);
            let ti = s.ship_type.index();
            // A Mimic disguised as one of the player's ships counts as an ally
            // (`selGetCommandAvailability`).
            if s.player == player()
                || u.players.get(player()).is_some_and(|p| p.is_ally(s.player))
                || hwc_sim::ships::mimic::is_disguised_as_player(u, id, player())
            {
                t[avail::ALLY] = true;
            } else {
                t[avail::ENEMY] = true;
            }
            if class == Some(hwc_sim::statics::ShipClass::Resource) && matches!(ti, 0x0e | 0x31) {
                t[avail::HARVEST] = true;
            }
            if has_upgrade(s, 0x40000) && !matches!(ti, 0x10 | 0x33) {
                t[avail::REPAIR] = true;
            }
            if has_upgrade(s, 0x80000) {
                t[avail::SALVAGE] = true;
            }
            if !command::attack_capable(u, &[id]).is_empty() {
                t[avail::ATTACK] = true;
            }
            match ti {
                0x70 => t[avail::SLIPGATE_SMALL] = true,
                0x71 => t[avail::SLIPGATE_MEDIUM] = true,
                0x72 => t[avail::SLIPGATE_LARGE] = true,
                _ => {}
            }
            for (bit, i) in [
                (0x800, avail::HIVE),
                (0x200, avail::EMP),
                (0x40, avail::LEECH),
                (0x8, avail::LINK),
                (0x20, avail::MIMIC),
                (0x400, avail::FORCE_FIELD),
            ] {
                if has_upgrade(s, bit) {
                    t[i] = true;
                }
            }
            if has_upgrade(s, 0x1000) || has_upgrade(s, 0x2000) || has_upgrade(s, 0x4000) {
                t[avail::MISSILES] = true;
            } else {
                t[avail::ALL_MISSILES] = false;
            }
            for (i, bit) in all {
                if !has_upgrade(s, bit) {
                    t[i] = false;
                }
            }
            if matches!(ti, 6 | 0x30) {
                t[avail::RAMMING] = true;
            } else {
                t[avail::ALL_RAMMING] = false;
            }
            if hwc_sim::ships::beast::ship_can_cascade_now(u, id) {
                t[avail::BEAST_CASCADE] = true;
            } else {
                t[avail::ALL_BEAST_CASCADE] = false;
            }
            // Super Acolytes with a link missile left (`AcolyteHasLinkMissiles`).
            if hwc_sim::ships::acolyte::has_link_missiles(u, id) {
                t[avail::LINK_MISSILES] = true;
            } else {
                t[avail::ALL_LINK_MISSILES] = false;
            }
            if ti == 5 {
                t[avail::MIMIC_TYPE] = true;
            }
            if has_upgrade(s, 0x20000) {
                if hwc_sim::cloak::is_cloaked(s) {
                    t[avail::CLOAKED] = true;
                } else {
                    t[avail::CLOAK] = true;
                }
            }
        }
        t
    }

    /// The object under the pointer as the classifier sees it.
    fn target_facts(&self, h: Hover) -> (Availability, cursor::TargetFacts) {
        let mut facts = cursor::TargetFacts::default();
        match h {
            Hover::Resource(r) => {
                let mut t = [false; avail::LEN];
                t[avail::RESOURCE] = true;
                facts.mimic_shape = self.mimic_target(h).is_some_and(|m| {
                    let u = self.world.universe();
                    self.world.mission_run().is_some_and(|run| {
                        hwc_sim::player_orders::mimic_can_take_shape(
                            u,
                            &run.game.disguise_tables,
                            &self.selection,
                            m,
                        )
                    })
                }) && self.world.universe().base.resources.get(r).is_some();
                (t, facts)
            }
            Hover::Ship(id) => {
                let t = self.availability(&[id]);
                let u = self.world.universe();
                if let Some(s) = u.ship(id) {
                    facts.own_ship = s.player == player();
                    facts.all_can_dock = facts.own_ship
                        && !self.selection.is_empty()
                        && self.selection.iter().all(|&a| {
                            dock::ship_can_dock_with(u, a, id, dock::purpose::DOCK, false)
                        });
                    let link = hwc_sim::upgrade::has_upgrade(s, 0x8);
                    facts.link_with_all = link
                        && self.selection.iter().all(|&a| {
                            u.ship(a).is_some_and(|x| {
                                hwc_sim::upgrade::has_upgrade(x, 0x8) && x.ship_type == s.ship_type
                            })
                        });
                    facts.mimic_shape = self.world.mission_run().is_some_and(|run| {
                        hwc_sim::player_orders::mimic_can_take_shape(
                            u,
                            &run.game.disguise_tables,
                            &self.selection,
                            hwc_sim::player_orders::MimicTarget::Ship(id),
                        )
                    });
                }
                (t, facts)
            }
        }
    }

    /// The ship under the pointer among those the local player can see (in the sensors
    /// manager the fog of war hides ships).
    /// Mines can be picked too (`selClickFromArray` takes any targetable object): in the
    /// sensors manager only those it shows.
    /// A mothership's module picks the mothership (`selSelectionClick`).
    fn pick_ship(&self, size: (u32, u32), p: [f32; 2]) -> Option<ShipId> {
        let u = self.world.universe();
        self.pick_object(size, p).map(|id| {
            if id.mine().is_some() {
                id
            } else {
                bandbox::stand_in(u, id)
            }
        })
    }

    fn pick_object(&self, size: (u32, u32), p: [f32; 2]) -> Option<ShipId> {
        if self.sensors.is_open() {
            let u = self.world.universe();
            let fog = sensors::Fog::of(u);
            self.world.object_at_where(
                size,
                p,
                |id| sensors::ship_visible(u, &fog, id),
                |id| {
                    self.mine_facts(id)
                        .is_some_and(|f| !f.hidden && !f.cloaked_unseen)
                },
            )
        } else {
            self.world.object_at_where(size, p, |_| true, |_| true)
        }
    }

    /// The band-box facts of a mine (`id` is its selection id) as the local player sees it
    /// now.
    fn mine_facts(&self, id: ShipId) -> Option<bandbox::Facts> {
        let u = self.world.universe();
        let t = hwc_sim::target::get(u, id)?;
        let shown = self
            .world
            .mission_run()
            .is_some_and(|r| r.game.show_mines_in_sensors);
        let fog_hidden = sensors::Fog::of(u).point_hidden(t.posinfo.position);
        bandbox::Facts::of_mine(u, id, self.sensors.is_open(), shown, fog_hidden)
    }

    /// The band-box facts of a ship or a mine.
    fn object_facts(&self, id: ShipId) -> Option<bandbox::Facts> {
        if id.mine().is_some() {
            self.mine_facts(id)
        } else {
            bandbox::Facts::of(self.world.universe(), id, player())
        }
    }

    /// Focus on ships (`ccFocus`, or `ccFocusClose` with `close`: the Alt focus): in the
    /// sensors manager, zoom back onto them, or (the modern focus key) centre the sensors
    /// camera on them.
    fn focus_on(&mut self, ships: Vec<ShipId>, close: bool, audio: &mut Option<Audio>) {
        if self.sensors.is_open() {
            if !close && self.sensors.focus_stays {
                self.sensors.focus_here(&mut self.world, &ships);
            } else if let Some(e) = self.sensors.close_on(&self.world, &ships) {
                sound(audio, e);
            }
        } else {
            self.world.focus_on(ships, close);
        }
    }

    /// The main camera's remembered angle, declination and distance for the mission's
    /// `CameraGetAngleDeg` and the like (the sensors manager keeps the main camera aside while
    /// it is open): degrees, the angle 0..360.
    fn kas_camera_in(&mut self) {
        let c = *self.sensors.main_camera().unwrap_or(&self.world.camera);
        let Some(run) = self.world.mission_run_mut() else {
            return;
        };
        let g = &mut run.game.camera;
        g.angle = c.angle.to_degrees().rem_euclid(360.0);
        g.declination = c.declination.to_degrees();
        g.distance = c.distance;
    }

    /// The mission's camera calls of the last update (`kasfCameraFocus` and the rest) on the
    /// main camera. A focus on ships first goes to a clean main screen (`spMainScreen(1)`:
    /// the sensors manager closes for good); an empty selection is the player's.
    fn kas_camera_orders(&mut self) {
        use hwc_sim::mission::CameraOrder;
        let Some(run) = self.world.mission_run_mut() else {
            return;
        };
        let orders = std::mem::take(&mut run.game.camera_orders);
        for o in orders {
            match o {
                CameraOrder::Focus {
                    ships,
                    close,
                    cut,
                    unless_same,
                } => {
                    let selected = ships.is_empty();
                    let ships = if selected {
                        self.selection.clone()
                    } else {
                        ships
                    };
                    if ships.is_empty() || (unless_same && self.world.focuses_on(&ships)) {
                        continue;
                    }
                    if !selected {
                        self.sensors.close_for_good(&mut self.world);
                    }
                    self.world.script_focus(ships, close, cut);
                }
                CameraOrder::AngleDeclination { angle, declination } => {
                    match self.sensors.main_camera_mut() {
                        Some(c) => {
                            (c.angle, c.declination) = (angle, declination);
                            c.rotate_angle(0.0);
                            c.rotate_declination(0.0);
                        }
                        None => self.world.script_angles(angle, declination),
                    }
                }
                CameraOrder::Distance { distance, scripted } => {
                    match self.sensors.main_camera_mut() {
                        Some(c) => c.distance = distance,
                        None => self.world.script_distance(distance, scripted),
                    }
                }
                CameraOrder::CancelFocus => self.world.cancel_focus(),
                CameraOrder::Chase(on) => self.world.chase = on,
            }
        }
    }

    /// The closest the camera may zoom: the sensors camera's own limit while the sensors
    /// manager has it, else the focus's (`cameraMinimumZoom` / `NewSetFocusPoint`).
    fn zoom_min(&self) -> f32 {
        if self.sensors.is_open() || self.sensors.zooming() || self.nis.is_some() {
            self.world.camera.closest_zoom
        } else {
            self.world.focus_min_zoom()
        }
    }

    /// Opens or closes the sensors manager (`Toggle_SensorsManager`).
    pub(crate) fn toggle_sensors(&mut self, audio: &mut Option<Audio>) {
        if self.sensors.is_open() {
            self.close_sensors(audio);
        } else if let Some(e) = self.sensors.begin(&mut self.world) {
            self.left_down = None;
            self.selecting.clear();
            sound(audio, e);
        }
    }

    fn close_sensors(&mut self, audio: &mut Option<Audio>) {
        // `ccLockOnTargetNow`: the main camera looks at its focus.
        let look = if self.world.focus.is_empty() {
            self.sensors
                .main_camera()
                .map_or(self.world.camera.look_at, |c| c.look_at)
        } else {
            v3(group_centre(self.world.universe(), &self.world.focus))
        };
        if let Some(e) = self.sensors.close(&self.world, look) {
            sound(audio, e);
            // A plate far from where the camera goes is cancelled.
            if let (Mode::Move { .. }, Some(c)) = (self.mode, self.selection_centre()) {
                if (c - look).length_squared() >= sensors::MAX_MOVE_DISTANCE_SQ {
                    self.mode = Mode::Select;
                }
            }
        }
    }

    /// The band box's mode from the modifiers held.
    fn band_box_mode(&self) -> bandbox::Mode {
        bandbox::Mode::from_held(|a| self.ui.action_held(a))
    }

    /// Ships inside the box with their facts for the local player; an attack box also finds
    /// mines (`selRectDragFunction` with its "anything" option).
    fn band_box_inside(
        &self,
        mode: bandbox::Mode,
        a: [f32; 2],
        b: [f32; 2],
    ) -> Vec<(ShipId, bandbox::Facts)> {
        let u = self.world.universe();
        let mut inside: Vec<(ShipId, bandbox::Facts)> = self
            .ships_in_box(a, b)
            .into_iter()
            .filter_map(|id| Some((id, bandbox::Facts::of(u, id, player())?)))
            .collect();
        if matches!(mode, bandbox::Mode::Attack | bandbox::Mode::ForceAttack) {
            inside.extend(
                self.mines_in_box(a, b)
                    .into_iter()
                    .filter_map(|id| Some((id, self.mine_facts(id)?))),
            );
        }
        inside
    }

    /// Mines (by selection id) whose centres project inside the band box.
    fn mines_in_box(&self, a: [f32; 2], b: [f32; 2]) -> Vec<ShipId> {
        let size = self.size();
        let (lo, hi) = (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        );
        let u = self.world.universe();
        u.missile_list
            .iter()
            .filter_map(|&mid| ShipId::from_mine(mid))
            .filter(|&id| {
                self.world.mine_box(size, id).is_some()
                    && hwc_sim::target::get(u, id).is_some_and(|t| {
                        self.world
                            .project(size, v3(t.posinfo.position))
                            .is_some_and(|p| {
                                p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]
                            })
                    })
            })
            .collect()
    }

    /// The "selecting" list of a band box being dragged (`mrSelectHold`).
    fn band_box_selecting(&self, mode: bandbox::Mode, a: [f32; 2], b: [f32; 2]) -> Vec<ShipId> {
        let inside = self.band_box_inside(mode, a, b);
        let u = self.world.universe();
        let sel = bandbox::Selected {
            ships: &self.selection,
            attack_capable: !bandbox::attack_capable(u, &self.selection).is_empty(),
            hold_capable: !bandbox::hold_capable(u, &self.selection).is_empty(),
            special: bandbox::special_gather(u, &self.selection),
        };
        bandbox::selecting_with(
            mode,
            &inside,
            &sel,
            |id| bandbox::stand_in(u, id),
            |id| bandbox::Facts::of(u, id, player()),
        )
    }

    /// `mrBandBoxRelease`: the band box's order.
    fn band_box_release(&mut self, a: [f32; 2], b: [f32; 2], audio: &mut Option<Audio>) {
        use bandbox::Mode;
        let mode = self.band_box_mode();
        let selecting = self.band_box_selecting(mode, a, b);
        match mode {
            Mode::ForceAttack | Mode::Attack => {
                if selecting.is_empty() {
                    return;
                }
                // The selection narrows to the ships able to attack.
                let attackers = bandbox::attack_capable(self.world.universe(), &self.selection);
                self.selection = attackers.clone();
                if attackers.is_empty() {
                    return;
                }
                let facts: Vec<(ShipId, bandbox::Facts)> = selecting
                    .iter()
                    .filter_map(|&id| Some((id, self.object_facts(id)?)))
                    .collect();
                let targets =
                    bandbox::attack_targets(&facts, &attackers, mode == Mode::ForceAttack);
                if !targets.is_empty() {
                    self.speak_attack(&attackers, &targets);
                    self.give(Order::Attack { ships: attackers, targets });
                }
            }
            Mode::Guard => {
                if selecting.is_empty() {
                    return;
                }
                let guards = bandbox::hold_capable(self.world.universe(), &self.selection);
                self.selection = guards.clone();
                if !guards.is_empty() {
                    // The mothership's voice acknowledges the guard order.
                    hwc_sim::speech::queue(self.world.universe_mut(), None, hwc_sim::speech::event::GUARD, 0);
                    self.give(Order::Protect { ships: guards, targets: selecting });
                }
            }
            Mode::Focus => {
                if !selecting.is_empty() {
                    self.focus_on(selecting, true, audio);
                }
            }
            Mode::MultipleSelect => {
                for id in selecting {
                    if !self.selection.contains(&id) {
                        self.selection.push(id);
                    }
                }
            }
            // `Ship_Special`: the selection keeps the ships with a special-target routine, and
            // the special-action menu opens for the boxed ships that are not selected
            // (`pmOpenForTargets(1, targets)`).
            Mode::Special => {
                let mut sel = std::mem::take(&mut self.selection);
                hwc_sim::player_orders::special_target_capable(self.world.universe(), &mut sel);
                self.selection = sel;
                if self.selection.is_empty() {
                    return;
                }
                let targets: Vec<ShipId> = selecting
                    .into_iter()
                    .filter(|id| !self.selection.contains(id))
                    .collect();
                if !targets.is_empty() {
                    self.special_targets = targets;
                    self.with_host(|ui, host| ui.open_popup(host, true));
                }
            }
            Mode::Select => {
                self.selection = selecting;
                if !self.selection.is_empty() {
                    self.speak_selected();
                }
            }
        }
    }

    /// Ships whose centres project inside the band box.
    fn ships_in_box(&self, a: [f32; 2], b: [f32; 2]) -> Vec<ShipId> {
        let size = self.size();
        let (lo, hi) = (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        );
        let u = self.world.universe();
        // Others' ships the sensors do not show now (out of sensor range, cloaked) are left
        // out in both views, so a band box never finds an enemy the player has not seen.
        let fog = sensors::Fog::of(u);
        u.ship_list
            .iter()
            .copied()
            .filter(|&id| sensors::ship_visible(u, &fog, id))
            .filter(|&id| {
                u.ship(id).is_some_and(|s| {
                    s.flags & hwc_sim::object::sof::HIDE == 0
                        && s.flags & hwc_sim::base::SOF_MODULE == 0
                        && self
                            .world
                            .project(size, v3(s.posinfo.position))
                            .is_some_and(|p| {
                                p[0] >= lo[0] && p[0] <= hi[0] && p[1] >= lo[1] && p[1] <= hi[1]
                            })
                })
            })
            .collect()
    }

    /// Selected ships that can move (`MakeShipsMobile`).
    fn mobile_selection(&self) -> Vec<ShipId> {
        let u = self.world.universe();
        self.selection
            .iter()
            .copied()
            .filter(|&id| {
                u.ship(id).is_some_and(|s| {
                    u.statics
                        .ship(s.ship_type)
                        .is_some_and(|st| st.header.max_velocity > 0.0)
                })
            })
            .collect()
    }

    fn selection_centre(&self) -> Option<Vec3> {
        (!self.selection.is_empty())
            .then(|| v3(group_centre(self.world.universe(), &self.selection)))
    }

    /// `selCentrePointCompute`: the middle of the selection's bounding box and its ships'
    /// average collision size.
    fn plate_centre(&self) -> Option<(Vec3, f32)> {
        let u = self.world.universe();
        let c = pings::selection_centre(u, &self.selection)?;
        let sizes: Vec<f32> = self
            .selection
            .iter()
            .filter_map(|&id| u.ship(id))
            .map(|s| {
                u.statics
                    .ship(s.ship_type)
                    .map_or(0.0, |st| st.header.coll.coll_sphere_size)
            })
            .collect();
        let avg = sizes.iter().sum::<f32>() / sizes.len().max(1) as f32;
        Some((v3(c), avg))
    }

    /// The camera the plate works with (the sensors camera while the sensors manager is up).
    fn plate_view(&self) -> plate::View {
        let size = self.size();
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let c = &self.world.camera;
        plate::View {
            view: c.view(),
            proj: c.projection(aspect),
            eye: c.eye(),
            size,
        }
    }

    pub(crate) fn attack(&mut self, targets: &[ShipId]) {
        if targets.is_empty() || self.selection.is_empty() {
            return;
        }
        let sel = command::attack_capable(self.world.universe(), &self.selection);
        if !sel.is_empty() {
            self.speak_attack(&sel, targets);
            self.give(Order::Attack { ships: sel, targets: targets.to_vec() });
        }
    }

    /// `speechEventAttack`: the first attacker acknowledges. With the force-attack keys held
    /// the line says what is attacked: the player's own ships, else enemies, else resources
    /// (and nothing for mines alone).
    fn speak_attack(&mut self, attackers: &[ShipId], targets: &[ShipId]) {
        use hwc_sim::speech::event;
        let u = self.world.universe();
        let e = if self.ui.action_held(keys::BANDBOX_FORCE_ATTACK) {
            let (mut own, mut enemy, mut resources) = (0, 0, 0);
            for &t in targets {
                if t.mine().is_some() {
                    continue;
                }
                match u.ship(t) {
                    Some(s) if s.player == player() => own += 1,
                    Some(_) => enemy += 1,
                    None => resources += 1,
                }
            }
            if own > 0 {
                event::ATTACK_OWN_SHIPS
            } else if enemy > 0 {
                event::ATTACK
            } else if resources > 0 {
                event::ATTACK_RESOURCES
            } else {
                return;
            }
        } else {
            event::ATTACK
        };
        hwc_sim::speech::by_first(self.world.universe_mut(), attackers, e);
    }

    /// The selection acknowledges being picked (`mrObjectClick`, `mrBandBoxRelease`): its
    /// first ship, or, with the player's mothership in it, the mothership's voice.
    fn speak_selected(&mut self) {
        let u = self.world.universe_mut();
        let ms = u.base.players[player()].mothership.filter(|&m| {
            self.selection.contains(&m)
                && u.ship(m)
                    .and_then(|s| u.statics.ship(s.ship_type))
                    .is_some_and(|st| {
                        st.ship_class == Some(hwc_sim::statics::ShipClass::Mothership)
                    })
        });
        match ms {
            Some(m) => {
                let t = u.ship(m).map_or(0, |s| s.ship_type.index() as i32);
                hwc_sim::speech::queue(u, None, hwc_sim::speech::event::SELECTED, t);
            }
            None => hwc_sim::speech::by_first(u, &self.selection, hwc_sim::speech::event::SELECTED),
        }
    }

    /// Plays the speech the orders and ships asked for since the last frame.
    fn ship_speech(&mut self, audio: &mut Option<Audio>) {
        if let Some(a) = audio.as_mut() {
            let beast = self.world.universe().players.get(player()).is_some_and(|p| p.race == hwc_sim::statics::ShipRace::Beast);
            a.set_player_race(beast);
        }
        // The simulation's status messages (`moMessageAdd`), such as the Leech's
        // "nonviable target" note.
        let notes = std::mem::take(&mut self.world.universe_mut().messages);
        for m in notes {
            let text = self.strings.text(m.key).trim().to_string();
            self.messages.push(GameMessage {
                text,
                colour: m.colour,
                ship: m.ship.map(crate::host::ui_id),
                time: self.world.universe().total_time_elapsed,
            });
        }
        let speech = std::mem::take(&mut self.world.universe_mut().speech);
        if let Some(a) = audio.as_mut() {
            for s in speech {
                a.ship_speech(self.world.universe(), player(), s);
            }
        }
        // For the next updates: the speech queue's refusals and the hot-key groups.
        self.world.speech_gate = audio.as_ref().map(|a| {
            let mut g = a.speech_gate();
            for (n, ships) in self.hotkeys.non_empty() {
                for &id in ships {
                    g.groups.insert(id, n);
                }
            }
            g
        });
    }


    // ---------------------------------------------------------------------------------------
    // Orders
    // ---------------------------------------------------------------------------------------

    /// Gives an order of the local player: at once in a game on this machine, in the next
    /// packet in a network game (applied on every machine at the same update). True when it
    /// was applied now.
    pub(crate) fn give(&mut self, order: Order) -> bool {
        if let Some(n) = self.net.as_mut() {
            n.session.give(order);
            return false;
        }
        let tables = self.world.mission_run().map(|r| r.game.disguise_tables.clone());
        hwc_sim::orders::apply(self.world.universe_mut(), player(), &order, tables.as_ref());
        true
    }

    /// The harvest order (`clWrapCollectResource`), the first harvester answering (each that
    /// finds nothing says so when no resource was named).
    fn harvest(&mut self, sel: &[ShipId], resource: Option<hwc_sim::resource::ResourceId>) {
        use hwc_sim::speech::{self, event};
        let u = self.world.universe_mut();
        let harvesters = hwc_sim::orders::harvesters(u, sel);
        let Some(&first) = harvesters.first() else {
            return;
        };
        match resource {
            Some(_) => speech::by_ship(u, first, event::HARVEST),
            None => {
                for &id in &harvesters {
                    let found = hwc_sim::resource::find_nearest_resource(u, id, 0.0, hwc_math::Vector::new(0.0, 0.0, 0.0));
                    match found {
                        Some(_) if id == first => speech::by_ship(u, id, event::HARVEST),
                        Some(_) => {}
                        None => speech::by_ship(u, id, event::HARVEST_NOTHING),
                    }
                }
            }
        }
        self.give(Order::Harvest { ships: harvesters, resource });
    }

    /// `clWrapDock`: the dock order, and the first ship's acknowledgement once it has somewhere
    /// to dock (retiring is announced by the mothership's voice, a slipgate trip is a move). In
    /// a network game the order is not applied yet, and the ship answers at once.
    fn dock(&mut self, sel: &[ShipId], purpose: i32, dock_with: Option<ShipId>) {
        use hwc_sim::speech::{self, event};
        let applied = self.give(Order::Dock { ships: sel.to_vec(), purpose, with: dock_with });
        let u = self.world.universe_mut();
        let Some(&first) = sel.first() else { return };
        if applied && u.ship(first).and_then(|s| s.base.dock_with).is_none() {
            return;
        }
        match purpose {
            dock::purpose::RETIRE => speech::queue(u, None, event::RETIRE, 0),
            dock::purpose::SLIPGATE => speech::by_ship(u, first, event::MOVE),
            _ => speech::by_ship(u, first, event::DOCK),
        }
    }

    // ---------------------------------------------------------------------------------------
    // Requests from the interface
    // ---------------------------------------------------------------------------------------

    fn apply(&mut self, r: GameRequest, audio: &mut Option<Audio>, out: &mut Vec<Outcome>) {
        match r {
            GameRequest::Build {
                factory,
                ship_type,
                count,
            } => {
                if let Some(t) = ShipType::from_index(ship_type as usize) {
                    self.give(Order::Build { factory: sim_id(factory), ship_type: t, count });
                }
            }
            GameRequest::Unqueue {
                factory,
                ship_type,
                count,
            } => {
                if let Some(t) = ShipType::from_index(ship_type as usize) {
                    self.give(Order::Unqueue { factory: sim_id(factory), ship_type: t, count });
                }
            }
            GameRequest::CancelBuild { factory, ship_type } => {
                if let Some(t) = ShipType::from_index(ship_type as usize) {
                    self.give(Order::CancelBuild { factory: sim_id(factory), ship_type: t });
                }
            }
            GameRequest::PauseBuild {
                factory,
                ship_type,
                paused,
            } => {
                if let Some(t) = ShipType::from_index(ship_type as usize) {
                    self.give(Order::PauseBuild { factory: sim_id(factory), ship_type: t, paused });
                }
            }
            GameRequest::Research { tech, .. } => {
                self.give(Order::Research { tech: tech as usize });
            }
            GameRequest::ClearLab { lab } => {
                self.give(Order::ClearLab { lab });
            }
            GameRequest::Launch { launcher, ships } => {
                let ships: Vec<ShipId> = ships.into_iter().map(sim_id).collect();
                self.give(Order::Launch { launcher: sim_id(launcher), ships });
            }
            GameRequest::SetAutoLaunch { launcher, on } => {
                let u = self.world.universe();
                let bit = dock::auto_launch_bit(u, sim_id(launcher));
                let bits = u.base.players[player()].auto_launch;
                let bits = if on { bits | bit } else { bits & !bit };
                self.give(Order::AutoLaunch { bits });
            }
            GameRequest::Command(Command::Hyperspace) => self.hyperspace_button(),
            GameRequest::Command(c) => self.command(c, audio),
            GameRequest::HyperspaceGoNow => {
                self.world.run_mut().hyperspace_go_now();
            }
            GameRequest::EmergencyHyperspace => {
                self.world.run_mut().emergency_hyperspace();
            }
            GameRequest::AbortHyperspace => self.abort_hyperspace(),
            GameRequest::SelectType { ship_type, owner } => {
                let u = self.world.universe();
                self.selection.retain(|&id| {
                    u.ship(id).is_some_and(|s| {
                        s.ship_type.index() as u32 == ship_type
                            && (owner == Owner::Enemy) == (s.player != player())
                    })
                });
                // `moProcessClick`: the first of them answers.
                let sel = self.selection.clone();
                hwc_sim::speech::by_first(
                    self.world.universe_mut(),
                    &sel,
                    hwc_sim::speech::event::SELECTED,
                );
            }
            GameRequest::DeselectTypes(types) => {
                let u = self.world.universe();
                self.selection.retain(|&id| {
                    u.ship(id).is_none_or(|s| {
                        !types.iter().any(|(t, _)| *t == s.ship_type.index() as u32)
                    })
                });
            }
            GameRequest::MimicType(slot) => {
                if let Some(r) = self.world.mission_run_mut() {
                    r.game.mimic_slot = usize::from(slot).min(8);
                }
            }
            GameRequest::MimicItem(i) => {
                let sel = self.selection.clone();
                if let Some(slot) = self.world.mission_run().map(|r| r.game.mimic_slot) {
                    self.give(Order::MimicAs { ships: sel, slot, index: i });
                }
            }
            GameRequest::FocusShip(id) => {
                let id = sim_id(id);
                if selectable(self.world.universe(), id) {
                    self.selection = vec![id];
                }
                self.world.focus_on(vec![id], false);
            }
            GameRequest::Action { action, down } => self.action(action, down, audio),
            GameRequest::SetPaused(p) => self.paused = p,
            // A multiplayer game shows its statistics before the menu (`utyGameQuitToMain` →
            // `gstatOpen`).
            GameRequest::QuitToMain if self.world.universe().mp.active && self.end_stats.is_empty() => self.open_end_stats(),
            GameRequest::QuitToMain | GameRequest::StatsDone => out.push(Outcome::Leave),
            GameRequest::RestartLevel => out.push(Outcome::Restart),
            GameRequest::SaveGame(name) => {
                let r = self.save_named(&name);
                self.report_save(r);
            }
            GameRequest::LoadGame(key) => out.push(Outcome::LoadGame(key.into())),
            GameRequest::Sound(e) => sound(audio, e),
            GameRequest::Speech { event, .. } => {
                if let Some(a) = audio {
                    a.speech(event);
                }
            }
            GameRequest::ApplyOptions | GameRequest::AudioSettings => {
                out.push(Outcome::ApplyOptions)
            }
            GameRequest::CycleShipIcons => {
                self.ship_icons = self.ship_icons.next();
                let v = self.ship_icons.to_option();
                // Kept and saved at once, so a loaded game, the next mission and the next run
                // start with it.
                self.ui.fe.game.set_option_now("hwcShipIcons", v);
                self.post(self.ship_icons.message());
            }
            GameRequest::SkipFleetIntel => {
                let now = self.world.universe().total_time_elapsed;
                if let Some(s) = self.subtitles.as_mut() {
                    if s.skip(now) {
                        if let Some(a) = audio.as_mut() {
                            a.stop_mission_speech(subtitle::SKIP_FADE);
                        }
                    }
                }
            }
            // `lmSlotNProcess` -> the mothership keeps a priority per module type.
            GameRequest::SetRepairPriority { module_type, priority } => {
                if let Some(t) = ShipType::from_index(module_type as usize) {
                    self.give(Order::RepairPriority { module_type: t, priority });
                }
            }
            // `lmBeastSystemsSet` (0x49b210): the Beast mothership's emergency power goes where
            // the buttons say.
            GameRequest::BeastSystems(v) => {
                self.give(Order::BeastSystems { mode: v });
            }
            GameRequest::Chat { allies, text } => self.chat(allies, &text),
            GameRequest::TraderAccept(_)
            | GameRequest::TraderLeave => {}
        }
    }

    /// The hyperspace key, objective or menu item (`mrHyperspaceBegin` ->
    /// `HyperspaceRollCallBegin`): nothing while paused or before the mission allows it; a
    /// ship under construction refuses it with a message.
    fn hyperspace_button(&mut self) {
        if self.paused || !self.world.run().game.hyper.player_can_hyperspace {
            return;
        }
        let r = self.world.run_mut().hyperspace_button();
        if r.is_ok() {
            // `ccFocusOnMyMothership`: the camera turns to the mothership for the jump.
            if let Some(ms) = self.world.universe().base.players[player()].mothership {
                self.world.focus_on(vec![ms], false);
            }
        }
        if let Err(hwc_sim::campaign::HyperspaceRefused::ShipUnderConstruction) = r {
            let text = self
                .strings
                .get("strShipsStillUnderConstruction")
                .unwrap_or("Complete or Cancel External Construction Before Continuing")
                .trim()
                .to_string();
            self.messages.push(GameMessage {
                text,
                colour: RED,
                ship: None,
                time: self.world.universe().total_time_elapsed,
            });
        }
    }

    /// The roll call's "cancel docking" (`spAbortHyperspaceCB`): back to the mission, the
    /// small ships stop docking, the jump can be started again, and the "hyperspace aborted"
    /// speech is queued. The auto-launch setting is not restored (as in the original).
    fn abort_hyperspace(&mut self) {
        self.world.run_mut().abort_hyperspace();
    }

    pub(crate) fn command(&mut self, c: Command, audio: &mut Option<Audio>) {
        let sel = self.selection.clone();
        if sel.is_empty() {
            return;
        }
        let mission_15 = self.on_mission_15();
        match c {
            Command::Move => self.start_move(audio),
            Command::Cancel => {
                self.give(Order::Halt { ships: sel });
            }
            Command::Dock => self.dock(&sel, dock::purpose::DOCK, None),
            Command::Retire => self.dock(&sel, dock::purpose::RETIRE, None),
            Command::Harvest => self.harvest(&sel, None),
            Command::TacticsEvasive => {
                self.give(Order::Tactics { ships: sel, tactics: hwc_sim::object::tactics::EVASIVE });
            }
            Command::TacticsNeutral => {
                self.give(Order::Tactics { ships: sel, tactics: hwc_sim::object::tactics::NEUTRAL });
            }
            Command::TacticsAggressive => {
                self.give(Order::Tactics { ships: sel, tactics: hwc_sim::object::tactics::AGGRESSIVE });
            }
            Command::Formation(f) => {
                let t = match f {
                    Formation::Delta => formation::kind::DELTA,
                    Formation::Broad => formation::kind::BROAD,
                    Formation::Delta3D => formation::kind::DELTA3D,
                    Formation::Claw => formation::kind::CLAW,
                    Formation::Wall => formation::kind::WALL,
                    Formation::Sphere => formation::kind::SPHERE,
                    Formation::Custom | Formation::Parade => formation::kind::CUSTOM,
                };
                // `mrSetTheFormation`: the mothership's voice names the formation (custom is
                // line 7, the parade line 8).
                let line = match f {
                    Formation::Custom => 7,
                    Formation::Parade => 8,
                    _ => t as i32,
                };
                hwc_sim::speech::queue(self.world.universe_mut(), None, hwc_sim::speech::event::FORMATION_SET, line);
                if sel.len() > 1 {
                    self.give(Order::Formation { ships: sel, formation: t });
                }
            }
            Command::Upgrade => {
                self.give(Order::Upgrade { ships: sel });
            }
            Command::EnterSlipgate => {
                self.give(Order::EnterSlipgate { ships: sel, mission_15 });
            }
            Command::BigGun => self.begin_big_gun(audio),
            // `mrBigGunCancel`.
            Command::CancelBigGun => {
                self.give(Order::CancelBigGun { ships: sel });
            }
            // `kbcbKamikaze`: the selection keeps only the ships that can.
            Command::Kamikaze => {
                if hwc_sim::player_orders::kamikaze_request(self.world.universe_mut(), &mut self.selection) {
                    let ships = self.selection.clone();
                    self.give(Order::Kamikaze { ships });
                    sound(audio, CLICK);
                }
            }
            // `mrLeechSet` (the Spy / Leech / Vent keys and the "Select Leech" submenu): the
            // selected Leeches take the drain mode.
            Command::LeechMode(m) => {
                self.give(Order::LeechMode { ships: sel, mode: m });
            }
            // `mrScuttle`: a first press asks, a second within the reconfirm time scuttles.
            Command::Scuttle => {
                let r = hwc_sim::player_orders::scuttle_request(
                    self.world.universe_mut(),
                    &mut self.selection,
                    &mut self.scuttle_confirm,
                );
                if r == hwc_sim::player_orders::Scuttle::Confirmed {
                    let ships = self.selection.clone();
                    self.give(Order::Scuttle { ships });
                    sound(audio, CLICK);
                }
            }
            // The special-action menu's items (`pmExecuteSpecialItem`) on the band-boxed targets
            // the menu was opened for, else on the item's context target; the same orders'
            // keys act without one (the repair key on each ship's nearest ally).
            c if crate::host::special_item(c).is_some() => {
                let item = crate::host::special_item(c).expect("special item");
                let from_menu = self
                    .ui
                    .fe
                    .game
                    .ig
                    .popup
                    .executed
                    .take()
                    .and_then(hwc_ui::ingame::command_of)
                    == Some(c);
                let targets = if from_menu {
                    self.menu_targets(item)
                } else {
                    Vec::new()
                };
                if !from_menu && item == hwc_sim::player_orders::SpecialItem::Repair {
                    self.give(Order::RepairKey { ships: sel });
                } else {
                    self.give(Order::Special { ships: sel, item, targets });
                }
                self.special_targets.clear();
            }
            other => self.post(&format!("{other:?}: not available yet")),
        }
    }

    /// The hot-key group keys (`kbcbGroupAssignN`, `kbcbGroupSelect`, `kbcbGroupAddSelect`).
    fn group_key(&mut self, op: keys::GroupOp, n: u8, audio: &mut Option<Audio>) {
        use keys::GroupOp;
        let u = self.world.universe();
        let r = match op {
            GroupOp::Create => self.hotkeys.assign(n, &self.selection),
            GroupOp::Select => {
                let mut sel = std::mem::take(&mut self.selection);
                let r = self
                    .hotkeys
                    .select(n, &mut sel, |id| crate::hotkeys::takes_part(u, id));
                self.selection = sel;
                r
            }
            GroupOp::AddSelect => {
                let mut sel = std::mem::take(&mut self.selection);
                let r = self
                    .hotkeys
                    .add_to_selection(n, &mut sel, |id| crate::hotkeys::can_add(u, id));
                self.selection = sel;
                r
            }
        };
        self.respond(r, audio);
    }

    /// A group key's sound, speech and focus.
    fn respond(&mut self, r: crate::hotkeys::Response, audio: &mut Option<Audio>) {
        if let Some(e) = r.sound {
            sound(audio, e);
        }
        if let (Some((event, n)), Some(a)) = (r.speech, audio.as_mut()) {
            a.fleet_speech(event, i32::from(n));
        }
        if r.focus {
            // In the main view `ccFocus`; in the sensors manager `smCullAndFocusSelecting`.
            let sel = self.selection.clone();
            self.focus_on(sel, false, audio);
        }
    }

    /// Waypoints are being placed (the plate is up in waypoint mode).
    fn placing_waypoints(&self) -> bool {
        matches!(self.mode, Mode::Move(_)) && self.waypoints.is_some()
    }

    /// `mrWaypointMoveBegin` (0x55f7d0): the plate comes up for placing waypoints (a move
    /// plate already up is put away first).
    fn waypoint_move_begin(&mut self, audio: &mut Option<Audio>) {
        if self.mobile_selection().is_empty() {
            return;
        }
        let Some((c, size)) = self.plate_centre() else {
            return;
        };
        self.mode = Mode::Move(Plate::new(c, size));
        self.plate_from_right = false;
        self.big_gun_plate = false;
        self.waypoints = Some(Vec::new());
        sound(audio, MOVEMENT_GUI_ON);
    }

    /// `mrWaypointAdd` (0x55f720): the plate's point is the next waypoint (at most 19). With
    /// `AutoFocusOnWaypoints` the sensors camera looks at it.
    pub(crate) fn waypoint_add(&mut self, audio: &mut Option<Audio>) {
        let Mode::Move(plate) = self.mode else { return };
        let dest = plate.destination();
        let Some(w) = self.waypoints.as_mut() else {
            return;
        };
        if !dest.is_finite() {
            return;
        }
        if w.len() < plate::MAX_WAYPOINTS {
            w.push(dest);
            sound(audio, plate::SOUND_CHIMES);
        }
        if self.sensors.is_open() && self.ui.fe.game.options.get_bool("AutoFocusOnWaypoints") {
            self.sensors.look_at(&mut self.world, dest);
        }
    }

    /// `mrWaypointMoveExecute` (0x55f8f0): the selection moves along the waypoints in the
    /// waypoint mode (`clWrapMove`), the first ship answering; the plate goes.
    fn waypoint_move_execute(&mut self, audio: &mut Option<Audio>) {
        let points = self.waypoints.take().unwrap_or_default();
        let sel = self.mobile_selection();
        if !sel.is_empty() && !points.is_empty() {
            if let Some((c, _)) = self.plate_centre() {
                let from = hwc_math::Vector::new(c.x, c.y, c.z);
                let wps: Vec<hwc_math::Vector> = points
                    .iter()
                    .map(|p| hwc_math::Vector::new(p.x, p.y, p.z))
                    .collect();
                self.give(Order::Move { ships: sel.clone(), mode: self.waypoint_mode, from, waypoints: wps });
                hwc_sim::speech::by_first(self.world.universe_mut(), &sel, hwc_sim::speech::event::MOVE);
                sound(audio, plate::SOUND_CHIMES);
            }
        }
        self.mode = Mode::Select;
    }

    /// `mrWaypointModeSet` (0x55fab0) and `mrWaypointModeCycle` (0x55fa40): the mode, and a
    /// message naming it.
    fn set_waypoint_mode(&mut self, mode: u32) {
        self.waypoint_mode = mode;
        let key = match mode {
            0 => "strWaypointModeNormal",
            1 => "strWaypointModePatrolCircular",
            _ => "strWaypointModePatrolEndToEnd",
        };
        let text = self.strings.text(key);
        self.post(&text);
    }

    fn start_move(&mut self, audio: &mut Option<Audio>) {
        if self.mobile_selection().is_empty() {
            return;
        }
        if self.mode == Mode::Select {
            let Some((c, size)) = self.plate_centre() else {
                return;
            };
            self.mode = Mode::Move(Plate::new(c, size));
            self.plate_from_right = self.right_click_move;
            self.waypoints = None;
            self.big_gun_plate = false;
            sound(audio, MOVEMENT_GUI_ON);
        } else {
            self.mode = Mode::Select;
            sound(audio, MOVEMENT_GUI_OFF);
        }
    }

    /// Key actions the game handles (`mrKeyPress`).
    pub(crate) fn action(&mut self, a: Action, down: bool, audio: &mut Option<Audio>) {
        // The overlay's pan arrows are held like keys.
        let arrows = [
            keys::PAN_SENSORS_LEFT,
            keys::PAN_SENSORS_RIGHT,
            keys::PAN_SENSORS_UP,
            keys::PAN_SENSORS_DOWN,
        ];
        if let Some(i) = arrows.iter().position(|&x| x == a) {
            self.pan_arrows[i] = down;
            return;
        }
        // `kbcbHeightMovementDown/Up`: not while the camera turns.
        if a == keys::ENABLE_HEIGHT_MOVE {
            if let (Mode::Move(p), false) = (&mut self.mode, self.right_down) {
                p.toggle(down);
            }
            return;
        }
        if !down {
            return;
        }
        if (136..=147).contains(&a.0) && self.sensors.toggles.toggle(a.0 as u8) {
            return;
        }
        if let Some((op, n)) = a.ship_group() {
            self.group_key(op, n, audio);
            return;
        }
        match a {
            keys::MOVE => self.start_move(audio),
            // `mrWaypointMove`: begin placing waypoints, or order the move along them.
            keys::WAYPOINTS => {
                if self.placing_waypoints() {
                    self.waypoint_move_execute(audio);
                } else {
                    self.waypoint_move_begin(audio);
                }
            }
            keys::TOGGLE_WAYPOINT_MODES => self.set_waypoint_mode((self.waypoint_mode + 1) % 3),
            keys::WAYPOINT_MODE_NORMAL => self.set_waypoint_mode(0),
            keys::WAYPOINT_MODE_CIRCULAR => self.set_waypoint_mode(1),
            keys::WAYPOINT_MODE_END_TO_END => self.set_waypoint_mode(2),
            // `mrWaypointRemoveLast`.
            keys::CANCEL_LAST_WAYPOINT => {
                if let Some(w) = self.waypoints.as_mut() {
                    w.pop();
                }
            }
            keys::FOCUS => {
                if !self.selection.is_empty() {
                    let sel = self.selection.clone();
                    self.focus_on(sel, false, audio);
                }
            }
            keys::TOGGLE_SENSORS_MANAGER => self.toggle_sensors(audio),
            keys::FOCUS_ON_MOTHERSHIP => {
                if let Some(ms) = self.world.universe().base.players[player()].mothership {
                    self.world.focus_on(vec![ms], false);
                }
            }
            keys::CANCEL_FOCUS => self.world.focus.clear(),
            keys::SELECT_ALL_VISIBLE => {
                let size = self.size();
                let (w, h) = (size.0 as f32, size.1 as f32);
                let all = self.ships_in_box([0.0, 0.0], [w, h]);
                self.selection = all
                    .into_iter()
                    .filter(|&id| selectable(self.world.universe(), id))
                    .collect();
            }
            keys::SELECT_NONE => self.selection.clear(),
            keys::REMOVE_SELECTED_SHIP_GROUP => {
                let r = self.hotkeys.remove_selected(&self.selection);
                self.respond(r, audio);
            }
            keys::ESCAPE => {
                if self.mode != Mode::Select {
                    self.mode = Mode::Select;
                    sound(audio, MOVEMENT_GUI_OFF);
                } else if self.sensors.is_open() {
                    // `smCancelMoveOrClose`.
                    self.close_sensors(audio);
                } else {
                    self.selection.clear();
                }
            }
            // `kbcbAccelerateTime` / `kbcbDecelerateTime` (Ctrl+= / Ctrl+-): one level up or
            // down, not while the letterbox comes in.
            keys::ACCELERATE_TIME if self.letterbox.fade_in == 0.0 => {
                if self.speed_level < GAME_SPEED_MAX {
                    self.set_speed_level(self.speed_level + 1, true);
                }
            }
            keys::DECELERATE_TIME if self.letterbox.fade_in == 0.0 => {
                if self.speed_level > 0 {
                    self.set_speed_level(self.speed_level - 1, true);
                }
            }
            // `kbcbToggleGameSpeed` (backslash): normal speed to the fastest, anything else back to
            // normal.
            keys::TOGGLE_GAME_SPEED if self.letterbox.fade_in == 0.0 => {
                let level = if self.speed_level == GAME_SPEED_DEFAULT {
                    GAME_SPEED_MAX
                } else {
                    GAME_SPEED_DEFAULT
                };
                self.set_speed_level(level, true);
            }
            // `kbcbPause`: not while the letterbox comes in or is in.
            keys::PAUSE if self.letterbox.fade_in == 0.0 && !self.letterbox.fully => {
                self.paused = !self.paused
            }
            keys::NEXT_FORMATION | keys::PREV_FORMATION if self.selection.len() > 1 => {
                let step = if a == keys::NEXT_FORMATION {
                    1
                } else {
                    formation::CYCLE.len() - 1
                };
                self.formation_index = (self.formation_index + step) % formation::CYCLE.len();
                let sel = self.selection.clone();
                self.give(Order::Formation { ships: sel, formation: formation::CYCLE[self.formation_index] });
            }
            _ => {}
        }
    }

    /// `taskApplyTimeCompressionLevel` (0x525410), with `taskShowTimeCompressionMessage`
    /// (0x525440: "8 X Time Compression", "1/4 X Time Compression") when `message`.
    fn set_speed_level(&mut self, level: usize, message: bool) {
        self.speed_level = level;
        let factor = GAME_SPEED_FACTORS[level];
        self.world.speed_factor = factor;
        if message {
            let what = self
                .strings
                .get("strTimesTimeCompression")
                .unwrap_or("X Time Compression")
                .trim()
                .to_string();
            let text = if factor < 1.0 {
                format!("1/{} {what}", (1.0 / factor) as i32)
            } else {
                format!("{} {what}", factor as i32)
            };
            self.post(&text);
        }
    }

    /// `taskResetTimeCompression` (0x5254c0) / `...WithMessage` (0x5254f0): back to normal
    /// speed if it is not.
    fn reset_speed(&mut self, message: bool) {
        if self.speed_level != GAME_SPEED_DEFAULT {
            self.set_speed_level(GAME_SPEED_DEFAULT, message);
        }
    }

    /// A player's name in a multiplayer game: the local player's own, `Computer <n>` for the
    /// others (`gameStart`'s `"%s %i"` with `strComputerName`).
    fn player_name(&self, p: usize) -> String {
        if let Some(name) = self.net.as_ref().and_then(|n| n.names.get(p)) {
            return name.clone();
        }
        if p == self.world.universe().mp.local_player {
            let n = self.local_name.clone().unwrap_or_else(|| self.ui.fe.game.options.get_str("PlayerName").to_string());
            if !n.trim().is_empty() {
                return n.trim().to_string();
            }
        }
        let computer = self.strings.get("strComputerName").unwrap_or("Computer").trim().to_string();
        format!("{computer} {p}")
    }

    /// The multiplayer rules' news: a player out (`univKillPlayer`'s message), the winners
    /// (`DeclarePlayerWin`), and the end of the game (back to the main menu).
    fn multiplayer_events(&mut self, _out: &mut Vec<Outcome>) {
        let events = std::mem::take(&mut self.world.universe_mut().mp.events);
        let text = |g: &Self, key: &str, fallback: &str| g.strings.get(key).unwrap_or(fallback).trim().to_string();
        for e in events {
            match e {
                hwc_sim::multiplayer::MpEvent::PlayerDefeated { player, .. } => {
                    let line = if player == self.world.universe().mp.local_player {
                        text(self, "strHaveBeenDefeated", "You have been defeated")
                    } else {
                        format!("{} {}", self.player_name(player), text(self, "strPlayerHasDied", "has died"))
                    };
                    self.post(&line);
                }
                hwc_sim::multiplayer::MpEvent::Won { winners } => {
                    let names: Vec<String> = (0..8).filter(|p| winners & (1 << p) != 0).map(|p| self.player_name(p)).collect();
                    let line = match names.len() {
                        0 => text(self, "strAllPlayersDead", "All players have been killed"),
                        1 => format!("{} {}", names[0], text(self, "strPlayerHasWon", "has won")),
                        _ => format!("{} {}", names.join(", "), text(self, "strPlayersHaveWon", "have won")),
                    };
                    self.post(&line);
                }
                hwc_sim::multiplayer::MpEvent::GameOver => {
                    if self.end_stats.is_empty() {
                        self.open_end_stats();
                    }
                }
                _ => {}
            }
        }
    }

    /// `gcInGameChatEntry`'s send: to everyone else, to the allies, or (`/name ...`) to one
    /// player; the line shows here too (`moMessageAdd`, blue when not for everyone).
    fn chat(&mut self, allies: bool, text: &str) {
        let Some(n) = self.net.as_ref() else { return };
        let me = player();
        let names = n.names.clone();
        let count = names.len().min(16);
        let everyone = ((1u32 << count) - 1) as u16 & !(1u16 << me);
        let (to, line, private) = match hwc_ui::ingame::gamechat::whisper_to(&names, text) {
            Some((p, rest)) if p != me && !rest.is_empty() => (1u16 << p, format!("[to {}] {rest}", names[p]), true),
            Some(_) => return,
            None if allies => {
                let mask = self.world.universe().players.get(me).map_or(0, |pl| pl.allies) as u16 & everyone;
                (mask, format!("[allies] {text}"), true)
            }
            None => (everyone, text.to_string(), false),
        };
        if to == 0 {
            return;
        }
        if let Some(n) = self.net.as_mut() {
            n.session.game_chat(to, text.strip_prefix('/').and_then(|_| line.split_once("] ").map(|(_, r)| r)).unwrap_or(text));
        }
        let me_name = names.get(me).cloned().unwrap_or_default();
        self.messages.push(GameMessage {
            text: format!("<{me_name}> {line}"),
            colour: if private { [100, 140, 255, 255] } else { [255, 255, 255, 255] },
            ship: None,
            time: self.world.universe().total_time_elapsed,
        });
    }

    /// `gstatOpen` with `gstatCollectPlayerStats` (0x4842b0): every player's statistics, and
    /// the game stops under them.
    fn open_end_stats(&mut self) {
        use hwc_sim::statics::ShipClass as C;
        let u = self.world.universe();
        let n = u.mp.num_players.max(1).min(u.base.players.len());
        let me = player();
        let schemes = self.world.team_schemes();
        let mut out = Vec::new();
        for p in 0..n {
            let st = u.base.stats.players.get(p).cloned().unwrap_or_default();
            let sum = |a: &[i32], cs: &[C]| cs.iter().map(|c| a.get(c.index()).copied().unwrap_or(0)).sum::<i32>();
            let fighters = [C::Corvette, C::Fighter, C::Micro];
            let frigates = [C::Frigate];
            let destroyers = [C::HeavyCruiser, C::Destroyer];
            let others = [C::Carrier, C::Resource, C::NonCombat, C::Component];
            let survived = if st.died_at != 0.0 { st.died_at } else { u.total_time_elapsed };
            let techs = (0..0x29).filter(|&t| u.base.players[p].tech.contains(t)).count() as i32;
            let (b, l, k) = (&st.built_per_class, &st.lost_per_class, &st.killed_per_class);
            let values = [
                survived as i32,
                st.players_killed,
                st.ru_collected,
                st.crystals,
                techs,
                sum(b, &fighters),
                sum(l, &fighters),
                sum(k, &fighters),
                sum(b, &frigates),
                sum(l, &frigates),
                sum(k, &frigates),
                sum(b, &destroyers),
                sum(l, &destroyers),
                sum(k, &destroyers),
                sum(b, &others),
                sum(l, &others),
                sum(k, &others),
            ];
            let c = schemes.get(p).map_or([200, 200, 200], |s| s.0);
            out.push(hwc_ui::ingame::gamestats::StatPlayer {
                name: self.player_name(p),
                colour: [c[0], c[1], c[2], 255],
                local: p == me,
                ally: p != me && u.players.get(me).is_some_and(|pl| pl.is_ally(p)),
                values,
            });
        }
        self.end_stats = out;
        self.paused = true;
    }

    fn post(&mut self, text: &str) {
        self.messages.push(GameMessage {
            text: text.to_string(),
            colour: [255, 255, 255, 255],
            ship: None,
            time: self.world.universe().total_time_elapsed,
        });
    }

    // ---------------------------------------------------------------------------------------
    // Saved games
    // ---------------------------------------------------------------------------------------

    /// Where saved games go and the save lists read them from.
    pub fn set_saves(&mut self, dir: Rc<crate::saves::SaveDir>) {
        let store: Rc<dyn hwc_ui::saves::SaveStore> = dir.clone();
        self.ui.fe.game.saves = Some(store);
        self.saves = Some(dir);
    }

    /// Whether the game can be saved now: a mission is running, with no NIS or movie playing
    /// and the mission not over (the original's escape menu is not reachable then either).
    pub fn can_save(&self) -> Result<(), &'static str> {
        if self.world.mission_run().is_none() || self.nis.is_some() || self.queued_nis.is_some() {
            return Err("The game cannot be saved during a cinematic.");
        }
        if self.movie || self.next_mission || self.nis_level_done || self.end.is_some() {
            return Err("The game cannot be saved now.");
        }
        if self.world.run().game.pending_nis.is_some() {
            return Err("The game cannot be saved during a cinematic.");
        }
        Ok(())
    }

    /// The interface's part of a saved game.
    pub fn ui_state(&self) -> crate::saves::UiState {
        crate::saves::UiState {
            view: self.world.view_state(),
            selection: self.selection.clone(),
            hotkeys: self.hotkeys.clone(),
            music: self.music,
            pings: self.pings.clone(),
            ping_sensor_states: self.ping_sensor_states.clone(),
            pings_seeded: self.pings_seeded,
            letterbox: self.letterbox.clone(),
            keep_letterbox_after_nis: self.keep_letterbox_after_nis,
            formation_index: self.formation_index,
            level_save: self.level_save.clone(),
        }
    }

    /// Puts back the interface of a loaded game (the simulation is the world's already). The
    /// saved music track starts again from its beginning, as in the original.
    pub fn restore_ui(&mut self, ui: crate::saves::UiState) {
        self.world.restore_view(&ui.view);
        let u = self.world.universe();
        self.selection = ui
            .selection
            .into_iter()
            .filter(|&id| selectable(u, id))
            .collect();
        self.hotkeys = ui.hotkeys;
        self.music = ui.music;
        self.resume_music = ui.music;
        // The letterbox keeps the screen shape's choice of bar height.
        let tall = self.letterbox.tall;
        self.letterbox = ui.letterbox;
        self.letterbox.tall = tall;
        self.keep_letterbox_after_nis = ui.keep_letterbox_after_nis;
        let tweaks = self.pings.tweaks.clone();
        self.pings = ui.pings;
        self.pings.tweaks = tweaks;
        self.ping_sensor_states = ui.ping_sensor_states;
        self.pings_seeded = ui.pings_seeded;
        self.formation_index = ui.formation_index;
        self.level_save = ui.level_save;
        self.paused = false;
        self.carried_groups = None;
    }

    /// Writes the game to `path` under `description`.
    pub fn save_to(
        &mut self,
        path: &std::path::Path,
        description: &str,
        autosave: bool,
    ) -> Result<(), String> {
        if self.world.universe().mp.active {
            // (Multiplayer saved games are not written yet: the save format's level comes
            // from a mission number.)
            return Err("saving a skirmish is not supported yet".into());
        }
        let ui = self.ui_state();
        let run = self.world.run();
        let header = crate::saves::header(run, description, autosave, self.thumb.clone());
        crate::saves::write(path, &header, run, &ui).map_err(|e| e.to_string())
    }

    /// The folder this game's saves go in.
    fn save_folder(&self) -> hwc_ui::saves::SaveFolder {
        if self.world.mission() == 0 {
            hwc_ui::saves::SaveFolder::Training
        } else {
            hwc_ui::saves::SaveFolder::SinglePlayer
        }
    }

    /// The quick save's file.
    pub fn quick_save_path(&self) -> Option<std::path::PathBuf> {
        let d = self.saves.as_ref()?;
        Some(d.path_for(self.save_folder(), crate::saves::QUICK_SAVE))
    }

    /// `gpSaveGame`: saves under the name the player typed.
    pub fn save_named(&mut self, name: &str) -> Result<std::path::PathBuf, String> {
        let name = name.trim();
        if name.chars().count() < hwc_ui::saves::MIN_NAME {
            return Err(self.string_or(
                "strAtLeast2Chars",
                "A saved game's name needs at least 2 characters.",
            ));
        }
        self.can_save().map_err(str::to_string)?;
        let dir = self.saves.clone().ok_or("Saved games are not available.")?;
        let path = dir.path_for(self.save_folder(), name);
        self.save_to(&path, name, false)?;
        Ok(path)
    }

    /// The message after a save (`strSavedGame`, or the error in red).
    fn report_save(&mut self, r: Result<std::path::PathBuf, String>) {
        match r {
            Ok(_) => {
                let t = self.string_or("strSavedGame", "Game saved.");
                self.post(&t);
            }
            Err(e) => {
                eprintln!("cataclysm: save: {e}");
                self.messages.push(GameMessage {
                    text: e,
                    colour: RED,
                    ship: None,
                    time: self.world.universe().total_time_elapsed,
                });
            }
        }
    }

    fn string_or(&self, key: &str, default: &str) -> String {
        self.strings
            .get(key)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    /// `kasfSaveLevelDeferred`: the autosave the mission asked for this update
    /// (`NN - <name>`, the tutorial's in the training folder), which "restart level" loads.
    fn autosave_if_due(&mut self) {
        let Some(a) = self.world.run().game.autosave.clone() else {
            return;
        };
        if let Some(dir) = self.saves.clone() {
            let folder = if a.training() {
                hwc_ui::saves::SaveFolder::Training
            } else {
                hwc_ui::saves::SaveFolder::SinglePlayer
            };
            let name = a.file_name();
            let path = dir.path_for(folder, &name);
            self.level_save = Some(path.clone());
            let r = self.save_to(&path, &name, true).map(|_| path);
            self.report_save(r);
        }
        self.world.run_mut().autosave_written();
    }

    /// After loading a saved game (`kasAutoSaveOnLoad`): a mission autosave counts the load
    /// and is written again at once; it becomes the level "restart" loads.
    pub fn after_load(&mut self, path: &std::path::Path) {
        if self.world.run_mut().loaded() {
            self.level_save = Some(path.to_path_buf());
            let description = crate::saves::read_description(path).unwrap_or_default();
            if let Err(e) = self.save_to(path, &description, true) {
                eprintln!("cataclysm: {}: {e}", path.display());
            }
            self.world.run_mut().autosave_written();
        }
    }

    // ---------------------------------------------------------------------------------------
    // Frame
    // ---------------------------------------------------------------------------------------

    /// One frame of `dt` seconds: the interface, the universe updates that are due, the
    /// mission's cues, the sound.
    pub fn update(&mut self, dt: f32, audio: &mut Option<Audio>) -> Vec<Outcome> {
        let mut out = Vec::new();
        crate::watchdog::detail(|| self.watch_line());
        crate::watchdog::checkpoint("update: interface");
        if let Some(p) = self.pending_load.take() {
            out.push(Outcome::LoadGame(p));
            return out;
        }
        if let Some((run, fired)) = self.queued_nis.take() {
            self.begin_nis(run, fired, audio);
        }
        if let Some(track) = self.resume_music.take() {
            if let Some(a) = audio.as_mut() {
                a.play_music(track);
            }
        }
        self.real_time += dt;
        self.world.chase_restore();
        self.note_menu_open();
        let requests = self.with_host(|ui, host| ui.update(host, dt));
        for r in requests {
            self.apply(r, audio, &mut out);
        }
        self.ui.fe.game.ig.popup.executed = None;
        self.note_menu_open();
        // The band-boxed special targets last as long as their menu.
        if self.ui.fe.game.ig.popup.open.is_none() {
            self.special_targets.clear();
        }
        self.update_menu_targets();
        // Polled keys: zoom (the sensors camera's while the sensors manager is up).
        let min = self.zoom_min();
        // The smooth wheel zoom and the close-up focus ease towards their target (on the real
        // frame time).
        let main = !(self.sensors.is_open() || self.sensors.zooming() || self.nis.is_some());
        self.world.ease_camera(dt, min, main);
        if !self.sensors.zooming() {
            if self.ui.action_held(keys::ZOOM_IN) {
                self.world.user_zoomed();
                self.world.camera.key_zoom(true, min);
            }
            if self.ui.action_held(keys::ZOOM_OUT) {
                self.world.user_zoomed();
                self.world.camera.key_zoom(false, min);
            }
        }
        // The sensors manager: a mission opening or closing it, the zoom, panning.
        crate::watchdog::checkpoint("update: sensors manager");
        let kas_open = self
            .world
            .mission_run()
            .is_some_and(|r| r.game.sensors_open);
        if kas_open != self.sensors.is_open() && !self.sensors.zooming() {
            if kas_open {
                self.toggle_sensors(audio);
            } else {
                self.close_sensors(audio);
            }
        }
        let pan = [
            self.pan_arrows[0] || self.ui.action_held(keys::PAN_SENSORS_LEFT),
            self.pan_arrows[1] || self.ui.action_held(keys::PAN_SENSORS_RIGHT),
            self.pan_arrows[2] || self.ui.action_held(keys::PAN_SENSORS_UP),
            self.pan_arrows[3] || self.ui.action_held(keys::PAN_SENSORS_DOWN),
        ];
        let menu_open = hwc_ui::ingame::popup::is_open(&self.ui.fe);
        for e in self.sensors.update(&mut self.world, dt, pan, menu_open) {
            sound(audio, e);
        }
        self.update_pointer();
        // The band box being dragged lists its ships.
        if let (Some(a), Mode::Select, false) = (self.left_down, self.mode, self.right_down) {
            let b = self.mouse;
            if (a.0 - b.0).abs() >= CLICK_SLOP || (a.1 - b.1).abs() >= CLICK_SLOP {
                let mode = self.band_box_mode();
                self.selecting = self.band_box_selecting(
                    mode,
                    [a.0 as f32, a.1 as f32],
                    [b.0 as f32, b.1 as f32],
                );
            } else {
                self.selecting.clear();
            }
        }
        self.world.paused = (self.paused && self.nis.is_none())
            || self.movie
            || self.next_mission
            || self.nis_level_done;
        self.world.sensors_open = self.sensors.is_open() && self.nis.is_none();
        // `subTitleAdd` drops the time compression for every subtitle it queues.
        if self
            .subtitles
            .as_mut()
            .is_some_and(|s| std::mem::take(&mut s.added) > 0)
        {
            self.reset_speed(true);
        }
        // A network game neither pauses nor compresses time (every machine runs at the
        // same pace), and reads the network every frame.
        if self.net.is_some() {
            self.paused = false;
            self.world.paused = false;
            if self.speed_level != GAME_SPEED_DEFAULT {
                self.set_speed_level(GAME_SPEED_DEFAULT, false);
            }
            let lines = self.net.as_mut().map(|n| {
                n.poll();
                let mut lines = n.messages();
                lines.extend(n.stall_note());
                lines
            });
            for line in lines.unwrap_or_default() {
                self.post(&line);
            }
        }
        let steps = self.world.due_steps(f64::from(dt));
        crate::watchdog::checkpoint("update: simulation steps");
        for _ in 0..steps {
            if self.movie || self.next_mission || self.nis_level_done {
                break;
            }
            // In a network game a turn starts only once every player's orders for it have
            // come (they are applied then); otherwise this frame waits.
            if self.net.as_ref().is_some_and(|n| n.mid_turn == 0) {
                let tables = self.world.mission_run().map(|r| r.game.disguise_tables.clone());
                let net = self.net.as_mut().expect("network game");
                if !net.before_update(self.world.universe_mut(), tables.as_ref()) {
                    break;
                }
            }
            // An NIS level has no mission yet: only its universe updates.
            let opening = self.world.nis_level().is_some();
            let before = self.hotkeys.snapshot(self.world.universe());
            let jobs = self.pending_jobs();
            if opening {
                self.world.step_universe();
            } else {
                if let (Some(a), None) = (self.autoplay.as_mut(), &self.nis) {
                    a.tick(self.world.run_mut());
                }
                self.kas_camera_in();
                self.world.step();
                self.kas_camera_orders();
            }
            if let Some(n) = self.net.as_mut() {
                n.after_update(self.world.universe());
            }
            if let Some(a) = audio.as_mut() {
                a.sim_events(self.world.universe(), !self.world.effects_on);
                a.ship_sounds(self.world.universe());
                let world = &self.world;
                a.effect_events(world.universe(), |id| world.remastered_hides_effect(id));
                a.remastered_sounds(self.world.take_remastered_sounds());
            }
            if !opening {
                self.groups_after_step(&before, audio);
                self.pings_after_step(&jobs);
                self.after_step(audio);
                // `kasfSaveLevelDeferred`: the mission's autosave, before an NIS it asked for
                // starts.
                self.autosave_if_due();
            }
            self.nis_step(audio);
            if !opening {
                self.campaign_step(audio, &mut out);
            }
        }
        self.ship_speech(audio);
        if let Some(a) = audio.as_mut() {
            let (eye, at) = match &self.nis {
                Some(n) => {
                    let c = &n.player.camera;
                    (
                        Vec3::new(c.eye.x, c.eye.y, c.eye.z),
                        Vec3::new(c.look_at.x, c.look_at.y, c.look_at.z),
                    )
                }
                None => (self.world.camera.eye(), self.world.camera.look_at),
            };
            a.game_update(
                eye.to_array(),
                at.to_array(),
                !self.paused,
                self.world.universe(),
            );
        }
        for e in std::mem::take(&mut self.ping_sounds) {
            sound(audio, e);
        }
        self.world.chase_apply();
        crate::watchdog::checkpoint("update: presentation");
        self.update_presentation(dt, audio);
        crate::watchdog::checkpoint("update: done");
        self.multiplayer_events(&mut out);
        if self.nis_level_done {
            out.push(Outcome::NisLevelDone);
        }
        if let Some(End::Complete { at }) = self.end {
            if self.real_time - at > COMPLETE_TIME {
                out.push(Outcome::Leave);
            }
        }
        out
    }

    /// The game's state in a line for the main-loop watchdog's report.
    fn watch_line(&self) -> String {
        format!(
            "mission {}{}, nis {}, sensors {:?} (open {}, zooming {}, kas {}), bars fade {:.2} in {}, movie {}, paused {}, real time {:.2}",
            self.world.mission(),
            if self.world.nis_level().is_some() { " (NIS level)" } else { "" },
            self.nis.is_some(),
            self.sensors.phase(),
            self.sensors.is_open(),
            self.sensors.zooming(),
            self.world.mission_run().is_some_and(|r| r.game.sensors_open),
            self.letterbox.fade,
            self.letterbox.fully,
            self.movie,
            self.paused,
            self.real_time,
        )
    }

    /// The jump's movie and the next mission (`singlePlayerGameUpdate` state 5).
    fn campaign_step(&mut self, audio: &mut Option<Audio>, out: &mut Vec<Outcome>) {
        let h = &self.world.run().game.hyper;
        let mission = self.world.run().game.config.mission;
        let pending = h.bink_pending;
        if self.bink_pending && !pending && h.whiteout_t > 0.0 {
            // `animBinkPlay(m, m + 1)`: music and sound stop, the game waits for the movie
            // (at normal speed).
            self.movie = true;
            self.reset_speed(false);
            if let Some(a) = audio.as_mut() {
                a.stop_music(0.0);
            }
            out.push(Outcome::Animatic(mission + 1));
        }
        self.bink_pending = pending;
        if self.world.run().mission_complete() && !self.next_mission {
            self.next_mission = true;
            out.push(Outcome::NextMission);
        }
    }

    /// The between-missions movie has ended (or was skipped): the game goes on.
    pub fn movie_finished(&mut self) {
        self.movie = false;
    }

    /// The hot-key groups after an update: links and unlinks pass the group on; dead, removed
    /// and given-away ships leave (`univDeleteDeadShip`, `univRemoveShipReferences`,
    /// `SelectionSwitchPlayerOwner`).
    fn groups_after_step(&mut self, before: &crate::hotkeys::Snapshot, audio: &mut Option<Audio>) {
        let u = &self.world.run().universe;
        self.hotkeys.relink(before, u);
        let died: Vec<ShipId> = u
            .events
            .iter()
            .filter_map(|e| match e {
                hwc_sim::universe::SimEvent::ShipDied { ship, .. } => Some(*ship),
                _ => None,
            })
            .collect();
        for n in self.hotkeys.prune(u, player(), &died) {
            if let Some(a) = audio.as_mut() {
                a.fleet_speech(crate::hotkeys::SPEECH_DEFEAT, i32::from(n));
            }
        }
    }

    /// The local player's factory jobs that have ships to build.
    fn pending_jobs(&self) -> Vec<PendingJob> {
        let u = self.world.universe();
        let mut out = Vec::new();
        for f in &u.base.factories {
            let Some(fs) = f
                .ship
                .filter(|&id| u.ship(id).is_some_and(|s| s.player == player()))
            else {
                continue;
            };
            for (i, j) in f.jobs.iter().enumerate() {
                if let (true, Some(t)) = (j.n_jobs > 0, j.info) {
                    out.push((fs, i, t));
                }
            }
        }
        out
    }

    /// The pings the game makes itself, after an update: the level's anomalous derelicts,
    /// big-gun shells, proximity-sensor alerts, the last ship of a build job; deaths and the
    /// evaluation task.
    fn pings_after_step(&mut self, jobs_before: &[PendingJob]) {
        use hwc_sim::ships::{proximity, ShipSpec, TypeStatics};
        use hwc_sim::universe::SimEvent;
        // The mission script's ping calls of this update, and the battles on a battle-ping
        // frame (`univUpdate` -> `pingBattlePingsCreate`).
        let (orders, battles) = {
            let run = self.world.run_mut();
            let orders = std::mem::take(&mut run.game.ping_orders);
            let u = &mut run.universe;
            let battles = if hwc_sim::battle_ping::is_battle_ping_frame(u) {
                hwc_sim::battle_ping::battle_pings(u, &self.ping_blobs)
            } else {
                Vec::new()
            };
            (orders, battles)
        };
        let run = self.world.run();
        let u = &run.universe;
        let now = u.total_time_elapsed;
        let p = &mut self.pings;
        if !self.pings_seeded {
            self.pings_seeded = true;
            for d in &run.game.level.derelicts {
                if d.attributes & 0x20 != 0 {
                    p.anomaly_point(pings::DERELICT_PING, d.position, now);
                }
            }
        }
        p.kas_orders(u, orders, now);
        p.battles(battles, now);
        for &(ship, loss) in &u.battle_losses {
            p.battle_loss(u, ship, loss);
        }
        for e in &u.events {
            match *e {
                SimEvent::ShipDied { ship, position, .. } => {
                    p.object_died(PingOwner::Ship(ship), position);
                }
                SimEvent::GunFired { bullet, .. } => {
                    if let Some(b) = u
                        .bullets
                        .get(bullet)
                        .filter(|b| b.bullet_type == pings::BIG_GUN_BULLET)
                    {
                        p.anomaly_object(
                            pings::BIG_GUN_PING,
                            PingOwner::Bullet(bullet),
                            b.posinfo.position,
                            now,
                        );
                    }
                }
                _ => {}
            }
        }
        // `pingRemovePingByOwner` as a shell goes.
        let gone: Vec<PingOwner> = p
            .list
            .iter()
            .filter_map(|q| match q.owner {
                Some(PingOwner::Bullet(b)) if u.bullets.get(b).is_none() => {
                    Some(PingOwner::Bullet(b))
                }
                _ => None,
            })
            .collect();
        for o in gone {
            p.remove_by_owner(o);
        }
        // Proximity sensors that just found something.
        let found = |st: i32| st == proximity::state::FOUND || st == proximity::state::TRACKING;
        let mut states = Vec::new();
        for &id in &u.ship_list {
            let Some(s) = u.ship(id).filter(|s| s.player == player()) else {
                continue;
            };
            let ShipSpec::ProximitySensor(spec) = &s.spec else {
                continue;
            };
            let was = self
                .ping_sensor_states
                .iter()
                .find(|(x, _)| *x == id)
                .map(|&(_, st)| st);
            if was == Some(proximity::state::SEARCHING) && found(spec.state) {
                if let TypeStatics::ProximitySensor(st) =
                    hwc_sim::ships::type_statics(u, s.ship_type)
                {
                    p.proximity(
                        id,
                        s.posinfo.position,
                        st.search_radius,
                        st.sensor_blink_rate.max(1e-3),
                        st.blip_color,
                        now,
                    );
                }
            }
            states.push((id, spec.state));
        }
        self.ping_sensor_states = states;
        // The last ship of a job: a ping at its factory.
        let mut pending: Vec<PendingJob> = jobs_before.to_vec();
        for e in &u.base.events {
            let BaseEvent::ShipBuilt {
                ship_type, player, ..
            } = *e
            else {
                continue;
            };
            if player != crate::host::player() {
                continue;
            }
            let done = pending.iter().position(|&(f, i, t)| {
                t == ship_type
                    && u.base.factories.iter().any(|fa| {
                        fa.ship == Some(f) && fa.jobs.get(i).is_some_and(|j| j.n_jobs == 0)
                    })
            });
            if let Some(k) = done {
                let (f, ..) = pending.remove(k);
                if let Some(fs) = u.ship(f) {
                    p.new_ship(fs.posinfo.position, now);
                }
            }
        }
        let evaluated = p.update(u, now, |id| {
            u.ship(id)
                .is_some_and(|s| matches!(&s.spec, ShipSpec::ProximitySensor(x) if found(x.state)))
        });
        // `battlePingEvaluate`: what each battle makes the fleet say.
        let u = &mut self.world.run_mut().universe;
        for mut b in evaluated {
            hwc_sim::battle_status::evaluate(u, b.centre, &mut b.stats, &b.ships);
        }
    }

    /// `pingListDraw` in the sensors view: the circles, their sounds and, with the tactical
    /// overlay, the legend.
    fn draw_pings(&mut self, px: (u32, u32), sounds: &mut Vec<u32>) {
        let u = self.world.universe();
        let fog = sensors::Fog::of(u);
        let now = u.total_time_elapsed;
        let eye = self.world.camera.eye();
        let world = &self.world;
        let centre_of = |o: PingOwner| match o {
            PingOwner::Ship(id) => u.ship(id).map(|s| s.posinfo.position),
            PingOwner::Bullet(b) => u.bullets.get(b).map(|b| b.posinfo.position),
        };
        let project = |c: hwc_math::Vector, r: f32| {
            sensors::project_sphere(world, px, v3(c), r).map(|k| (k.x, k.y, k.r))
        };
        let (circles, sound, masks) = self.pings.frame(
            now,
            hwc_math::Vector::new(eye.x, eye.y, eye.z),
            centre_of,
            project,
            |c| fog.point_hidden(c),
            self.sensors.zooming(),
        );
        for c in &circles {
            pings::draw_circle(&mut self.canvas, c);
        }
        sounds.extend(sound);
        if !self.ui.fe.game.ig.overlay.tactical {
            return;
        }
        let legend = self.pings.legend(now, masks);
        let Some(font) = self.group_fonts[2].clone() else {
            return;
        };
        let rh = text::height(&font);
        let mut y = rh;
        for (name, colour) in legend {
            let c = pings::Circle {
                x: rh * 3 / 2,
                y: y + rh / 2,
                r: rh / 2,
                segments: self.pings.tweaks.to_segments,
                color: colour,
            };
            pings::draw_circle(&mut self.canvas, &c);
            let label = self.strings.get(name).unwrap_or(name).trim().to_string();
            text::print(
                &mut self.canvas,
                &font,
                (f64::from(rh) * 2.5) as i32,
                y,
                PING_LEGEND_TEXT,
                label.as_bytes(),
                (Shadow::from_bits(0), [0, 0, 0, 255]),
            );
            y += rh + 1;
        }
    }

    fn after_step(&mut self, audio: &mut Option<Audio>) {
        let u = &self.world.run().universe;
        // An unlinked ship's parts stay selected and in focus (`processUnlinkToDo`).
        for e in &u.base.events {
            if let BaseEvent::Unlinked { old, a, b } = *e {
                if self.selection.contains(&old) {
                    self.selection.extend([a, b]);
                }
            }
        }
        self.selection.retain(|&id| selectable(u, id));
        let now = u.total_time_elapsed;
        let mut posts = Vec::new();
        for e in &u.base.events {
            match e {
                BaseEvent::ShipBuilt {
                    ship_type, player, ..
                } if *player == crate::host::player() => {
                    let name = self
                        .strings
                        .get(&format!("str{}", ship_type.name()))
                        .unwrap_or(ship_type.name())
                        .trim()
                        .to_string();
                    posts.push(format!("{name} built"));
                }
                BaseEvent::Researched { player, tech } if *player == crate::host::player() => {
                    let name = Technology::from_index(*tech)
                        .and_then(|t| self.strings.get(&format!("str{}", t.name())))
                        .unwrap_or("")
                        .to_string();
                    posts.push(format!("Research complete: {name}"));
                }
                _ => {}
            }
        }
        for p in posts {
            self.messages.push(GameMessage {
                text: p,
                colour: [255, 255, 255, 255],
                ship: None,
                time: now,
            });
        }
        if self.messages.len() > 50 {
            let n = self.messages.len() - 50;
            self.messages.drain(..n);
        }
        for cue in self.cues.poll(self.world.run_mut()) {
            match cue {
                Cue::Music(track) => {
                    self.music = Some(track);
                    if let Some(a) = audio.as_mut() {
                        a.play_music(track);
                    }
                }
                Cue::Speech { event, variable } => self.speak(event, variable, audio),
                Cue::LocationCard { milliseconds, text } => {
                    self.intel_trace.push(
                        now,
                        IntelTraceKind::LocationCard {
                            seconds: milliseconds as f32 * 0.001,
                        },
                    );
                    if let Some(subs) = self.subtitles.as_mut() {
                        subs.location_card(milliseconds, &text::to_latin1(&text));
                    }
                }
                Cue::Completed => {
                    if self.end.is_none() {
                        self.end = Some(End::Complete { at: self.real_time });
                    }
                }
                Cue::Failed => {
                    if self.end.is_none() {
                        self.end = Some(End::Failed);
                        self.with_host(|ui, host| ui.game_over(host));
                    }
                }
            }
        }
    }

    /// A speech event on the intel stream with its subtitles: each subtitled fragment reaches
    /// the subtitle system with its actor; the last letterbox card expiring is what ends the
    /// intel event.
    fn speak(&mut self, event: u32, variable: i32, audio: &mut Option<Audio>) {
        let spoken = audio
            .as_mut()
            .and_then(|a| a.mission_speech(event, variable));
        let mission = self.world.mission();
        let actor = subtitle::actor_for_speech(0, event, true, mission);
        let now = self.world.universe().total_time_elapsed;
        let mut traced = None;
        if let (Some(s), Some(actor), Some(subs)) = (spoken, actor, self.subtitles.as_mut()) {
            let n = s.subtitles.len().max(1) as f32;
            let each = s.duration_ms.max(0) as f32 / 1000.0 / n;
            for t in &s.subtitles {
                subs.add(actor, event, &text::to_latin1(t), each);
            }
            if !s.subtitles.is_empty() {
                traced = Some(IntelTraceKind::Speech {
                    event,
                    seconds: s.duration_ms.max(0) as f32 / 1000.0,
                    fragments: s.subtitles.len(),
                });
            }
        }
        self.intel_trace
            .push(now, traced.unwrap_or(IntelTraceKind::Unsubtitled { event }));
    }

    // ---------------------------------------------------------------------------------------
    // NIS
    // ---------------------------------------------------------------------------------------

    /// An NIS starts playing (`singlePlayerStartNis`): the managers and menus close, the
    /// volumes are kept to restore, the letterbox comes in as the script says, and the
    /// events at time 0 run.
    pub fn begin_nis(&mut self, mut run: NisRun, fired: Vec<Fired>, audio: &mut Option<Audio>) {
        // `spMainScreen`'s `smSensorsCloseForGood`: at once, even in the middle of a zoom (a
        // zoom left going, or refused because one was, kept the manager open through the
        // cutscene, and the camera it saved and gave back at its end was the sensors one).
        self.sensors.close_for_good(&mut self.world);
        self.mode = Mode::Select;
        self.left_down = None;
        self.right_down = false;
        self.selecting.clear();
        run.volumes = audio.as_ref().map(|a| a.volumes());
        if !run.kind.in_own_level() {
            run.saved_camera = Some(self.world.camera);
            run.saved_lighting = Some(self.world.lighting.clone());
        }
        if let Some(secs) = run.player.data.scissor_fade_in {
            // `nisScissorBlendSet` sets the fade-in when the script loads; the NIS task moves
            // it from there.
            self.letterbox.fade_in = secs;
        }
        self.nis = Some(run);
        // `nisStartup`: normal speed.
        self.reset_speed(false);
        self.nis_present(fired, audio);
    }

    /// Whether an NIS made on the loading screen waits for the first frame.
    pub fn has_queued_nis(&self) -> bool {
        self.queued_nis.is_some()
    }

    /// An NIS made on the loading screen (the opening); it starts with the first frame.
    pub fn queue_nis(&mut self, run: NisRun, fired: Vec<Fired>) {
        self.queued_nis = Some((run, fired));
    }

    /// Uploads the art of the world.
    pub fn prepare(&mut self, renderer: &mut Renderer) {
        self.world.prepare(renderer);
    }

    /// Escape during an NIS: to its end, unless the letterbox is still coming in.
    pub fn skip_nis(&mut self) {
        let fading_in = self.letterbox.fade_in > 0.0;
        if let Some(n) = self.nis.as_mut() {
            if n.player.can_skip(fading_in) {
                n.player.go_to_end();
            }
        }
    }

    /// One NIS task step after a universe update (and the start of an NIS the mission asked
    /// for).
    fn nis_step(&mut self, audio: &mut Option<Audio>) {
        if self.nis.is_none() {
            self.nis_requests(audio);
        }
        let Some(mut run) = self.nis.take() else {
            return;
        };
        let (fired, finished) = run.tick(&mut self.world);
        self.nis = Some(run);
        self.nis_present(fired, audio);
        if finished {
            self.end_nis(audio);
        }
    }

    /// `G_PlayNis` / `G_PlayNislet`: the request the simulation holds for the game
    /// (`MissionConfig::hold_nis`) starts playing.
    fn nis_requests(&mut self, audio: &mut Option<Audio>) {
        let Some(run) = self.world.mission_run() else {
            return;
        };
        let mission = run.game.config.mission;
        let kind = match run.game.pending_nis {
            Some(hwc_sim::mission::NisRequest::Nis(n)) => NisKind::Mission(n.max(0) as u32),
            Some(hwc_sim::mission::NisRequest::Nislet(n)) => NisKind::Nislet(n.max(0) as u32),
            None => return,
        };
        let point = run
            .game
            .level
            .point(hwc_kas::kas_hash("StartPointPlayer"))
            .unwrap_or([0.0; 4]);
        let modules = hwc_sim::nis_support::hyperspace_ship_types_attached_to_mothership(
            &run.universe,
            &run.game.hyper.in_hyperspace,
        );
        let assets = self.world.assets().clone();
        match nis::load(&assets, kind, mission) {
            Ok(data) => {
                let at = nis::placement(point);
                let (run, fired) =
                    NisRun::start(&mut self.world, data, kind, at, mission, &modules);
                self.world.load_new_art();
                self.begin_nis(run, fired, audio);
            }
            Err(e) => {
                // Without its files the NIS counts as played.
                eprintln!("cataclysm: {e}");
                self.world.run_mut().nis_finished(false);
            }
        }
    }

    /// `nisStop` and `singlePlayerNisStoppedCB`.
    fn end_nis(&mut self, audio: &mut Option<Audio>) {
        let Some(mut run) = self.nis.take() else {
            return;
        };
        let report = run.player.stop();
        run.stage.stop(self.world.universe_mut(), &report);
        // `lensFlareNISSunRemove`, `plasmaCloudReset`.
        self.world.nis_sun.remove();
        self.world.plasma.reset();
        if let Some(a) = audio.as_mut() {
            if report.stop_music {
                a.stop_music(0.0);
            }
            if let Some(v) = run.volumes {
                a.set_volumes(v);
            }
        }
        if let Some(s) = self.subtitles.as_mut() {
            s.reset();
        }
        // The bars go when the NIS ends, unless mission 1's start asked to keep them.
        if self.keep_letterbox_after_nis {
            self.keep_letterbox_after_nis = false;
            // The bars stay in: the mission's next `wideScreenOut` takes them away (Mission 1
            // calls it when the first fleet-intel event ends; its `wideScreenIn` before that
            // finds them fully in and does nothing).
        } else {
            self.letterbox.fully = false;
        }
        if let Some(c) = run.saved_camera.take() {
            self.world.camera = c;
        }
        if let Some(l) = run.saved_lighting.take() {
            self.world.lighting = l;
        }
        match run.kind {
            // `singlePlayerNisStoppedCB` + `singlePlayerLoadNewLevel`: the mission loads.
            NisKind::Opening | NisKind::Level(_) => self.nis_level_done = true,
            // `singlePlayerNisStoppedCB`: the completion variable.
            NisKind::Nislet(_) | NisKind::Mission(_) => {
                self.world.run_mut().nis_finished(false);
            }
        }
    }

    /// The events of an NIS step the presentation handles.
    fn nis_present(&mut self, fired: Vec<Fired>, audio: &mut Option<Audio>) {
        for f in fired {
            let ship = f.object.and_then(|i| {
                self.nis
                    .as_ref()
                    .and_then(|n| n.stage.ship(self.world.universe(), i))
            });
            match f.action {
                NisAction::FleetSpeechEvent { event, variable }
                | NisAction::SpeechEvent {
                    event, variable, ..
                } => {
                    if !f.seeking {
                        self.speak(event as u32, variable, audio);
                    }
                }
                NisAction::SoundEvent { event } => {
                    if !f.seeking {
                        sound(audio, event as u32);
                    }
                }
                NisAction::Subtitle { index, ms } => {
                    let text = hwc_nis::tables::subtitle_string(index)
                        .and_then(|k| self.strings.get(k))
                        .map(|t| t.trim().to_string());
                    if let (Some(t), Some(subs)) = (text, self.subtitles.as_mut()) {
                        // `nisSubtitle`: a location card on actor 0x70, event 0.
                        let mut b = format!(
                            "#r{:x}#t{:x}",
                            subtitle::region::LOCATION_CARD,
                            subtitle::THEME_LOCATION_CARD
                        )
                        .into_bytes();
                        b.extend_from_slice(&text::to_latin1(&t));
                        subs.add(subtitle::ACTOR_LOCATION_CARD, 0, &b, ms as f32 * 0.001);
                    }
                }
                NisAction::StartMusic(track) => {
                    self.music = Some(track);
                    if let Some(a) = audio.as_mut() {
                        a.stop_music(0.0);
                        a.play_music(track);
                    }
                }
                NisAction::StopMusic(fade) => {
                    if let Some(a) = audio.as_mut() {
                        a.stop_music(fade);
                    }
                }
                NisAction::MusicVolume(v) => {
                    if let Some(a) = audio.as_mut() {
                        a.set_music_volume(v);
                    }
                }
                NisAction::SpeechVolume(v) => {
                    if let Some(a) = audio.as_mut() {
                        a.set_speech_volume(v);
                    }
                }
                NisAction::SfxVolume(v) => {
                    if let Some(a) = audio.as_mut() {
                        a.set_sfx_volume(v);
                    }
                }
                NisAction::WideScreenOut(secs) => {
                    // `nisScissorOut` (no sound, unlike the mission's `wideScreenOut`).
                    let l = &mut self.letterbox;
                    if l.fade_in != 0.0 {
                        l.fade_time = 0.0;
                        l.fade_in = 0.0;
                        l.fade = 0.0;
                    }
                    l.fade_out = secs;
                }
                NisAction::LightAmbient { light, rgb } | NisAction::LightDiffuse { light, rgb } => {
                    let ambient = matches!(f.action, NisAction::LightAmbient { .. });
                    let c = Vec3::new(rgb[0] as f32, rgb[1] as f32, rgb[2] as f32) * 0.003_921_569;
                    if let Some(n) = self.nis.as_mut() {
                        let l = &mut n.lights[light.clamp(0, 1) as usize];
                        if ambient {
                            l.ambient = c;
                        } else {
                            l.diffuse = c;
                        }
                        n.apply_lights(&mut self.world);
                    }
                }
                NisAction::GlobalAmbient(rgb) => {
                    self.world.lighting.global_ambient =
                        Vec3::new(rgb[0] as f32, rgb[1] as f32, rgb[2] as f32) * 0.003_921_569;
                }
                NisAction::NumberLights(n) => {
                    if let Some(r) = self.nis.as_mut() {
                        r.number_lights = n.clamp(1, 2) as usize;
                        r.apply_lights(&mut self.world);
                    }
                }
                NisAction::Fog(ref args) => {
                    // `nisFog`.
                    let now = self.world.universe().total_time_elapsed;
                    self.world.plasma.nis_fog(args, now);
                }
                NisAction::MeshAnimStart(ref name) => {
                    if let Some(id) = ship {
                        if !self.world.play_anim(id, name) {
                            self.nis_note(format!("meshAnimStart {name}: no such animation"));
                        }
                    }
                }
                // Done by the player (fade, camera, flags) or the stage (objects).
                NisAction::Show
                | NisAction::Hide
                | NisAction::HideUniverseToggle
                | NisAction::PauseUniverseToggle
                | NisAction::FadeSet { .. }
                | NisAction::FadeTo { .. }
                | NisAction::CameraFov(_)
                | NisAction::CameraClipNear(_)
                | NisAction::CameraClipFar(_)
                | NisAction::RemainAtEnd { .. }
                | NisAction::KeepMovingAtEnd
                | NisAction::Death { .. } => {}
                ref other => {
                    let k = format!("{other:?}");
                    let k = k.split(['(', ' ', '{']).next().unwrap_or("").to_string();
                    self.nis_note(format!("NIS event {k} is not shown yet"));
                }
            }
        }
    }

    fn nis_note(&mut self, note: String) {
        if !self.nis_notes.contains(&note) {
            eprintln!("note: {note}");
            self.nis_notes.push(note);
        }
    }

    /// The NIS fade to black (or white) over the picture between the letterbox bars
    /// (`mrRegionDraw`).
    fn draw_nis_fade(&mut self) {
        let Some(n) = &self.nis else {
            return;
        };
        let f = n.player.fade;
        if f.level == 0.0 {
            return;
        }
        let (w, h) = (self.canvas.width, self.canvas.height);
        let full = self.letterbox.full_height(w, h);
        let bar = if self.letterbox.fade == 0.0 {
            full
        } else {
            (full as f32 * self.letterbox.fade) as i32
        };
        let c = if f.white { 255 } else { 0 };
        let a = (f.level.clamp(0.0, 1.0) * 255.0).round() as u8;
        let r = Rect::new(0, (bar - 7).max(0), w, (h - bar + 7).min(h));
        self.canvas.blend_rect(r, [c, c, c, a]);
    }

    /// The end of `mrRegionDraw`: the single-player fade to black (not while an NIS plays)
    /// and the hyperspace whiteout, both over the whole screen.
    fn draw_hyperspace_fades(&mut self) {
        let Some(run) = self.world.mission_run() else {
            return;
        };
        let h = &run.game.hyper;
        let (w, ht) = (self.canvas.width, self.canvas.height);
        let full = Rect::new(0, 0, w, ht);
        if h.fade > 0.0 && self.nis.is_none() {
            let a = (h.fade.min(1.0) * 255.0) as u8;
            self.canvas.blend_rect(full, [0, 0, 0, a]);
        }
        // The white flash after mission 1's NISlet, added over the view.
        if h.arrival_flash > 0.0 {
            let a = (h.arrival_flash.min(1.0) * 255.0) as u8;
            self.canvas.add_rect(full, [255, 255, 255, a]);
        }
        if h.whiteout {
            match whiteout_colour(h.whiteout_t) {
                Whiteout::Additive(c) => self.canvas.add_rect(full, c),
                Whiteout::Opaque(c) => self.canvas.fill_rect(full, c),
            }
        }
    }

    /// Draws the frame: the 3D view, the game's marks (health bars of the selection, band box,
    /// move plate), subtitles and the interface. The pointer is the application's.
    pub fn render(&mut self, renderer: &mut Renderer, size: (i32, i32)) -> Result<(), String> {
        if (self.canvas.width, self.canvas.height) != size {
            self.canvas.resize(size.0, size.1);
            self.ui.resize(size.0, size.1);
        }
        let px = (size.0.max(1) as u32, size.1.max(1) as u32);
        let samples = renderer.set_sample_count(self.quality.msaa);
        let present = self
            .present
            .filter(|p| p.0 > px.0 || p.1 > px.1)
            .unwrap_or(px);
        let mut quality = self.quality;
        // The sensors manager is a flat map: no bloom or heat haze over its blips and discs.
        if self.sensors.is_open() {
            quality.post = hwc_render::PostFx::default();
        }
        if self.target.as_ref().is_none_or(|t| {
            t.plan.logical != px
                || t.plan.samples != samples
                || t.plan.present != present
                || self.target_quality != quality
        }) {
            let t = OffscreenTarget::with_quality(renderer, px, present, quality);
            if t.plan.limited {
                eprintln!(
                    "cataclysm: render scale {:.2} with {}x MSAA cut to {:.2} ({}x{}) to stay within the GPU budget",
                    quality.scale, samples, t.plan.scale, t.plan.render.0, t.plan.render.1
                );
            }
            self.target = Some(t);
            self.target_quality = quality;
        }
        self.layer_active = false;
        profile::count(
            "canvas_px",
            (self.canvas.width.max(0) * self.canvas.height.max(0)) as u64,
        );
        let span = profile::span("prepare");
        self.world.prepare(renderer);
        drop(span);
        let game_camera = self.nis.as_ref().map(|_| self.world.camera);
        if let Some(n) = &self.nis {
            n.view(&mut self.world);
        }
        if self.sensors.phase() == Phase::Sensors && self.nis.is_none() {
            let _p = profile::span("sm.view");
            self.render_sensors(renderer, px)?;
        } else {
            let _p = profile::span("main.view");
            // The main-view part of a sensors zoom uses the main camera's clip planes.
            let saved = self.world.camera.tweaks;
            if let Some(m) = self.sensors.main_camera() {
                self.world.camera.tweaks = m.tweaks;
            }
            let span = profile::span("main.frame_prep");
            let frame = self.world.frame(px);
            drop(span);
            let target = self.target.as_ref().expect("target");
            let span = profile::span("main.scene_pass");
            let rgba = self.world.render_offscreen(renderer, target, &frame);
            drop(span);
            gpu_profile(renderer, "main.scene");
            let span = profile::span("main.scene_copy");
            profile::count("canvas_copies", 1);
            for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba?.as_chunks::<4>().0) {
                *dst = [src[0], src[1], src[2], 255];
            }
            drop(span);
            self.world.camera.tweaks = saved;
            if target.plan.present != px {
                self.scene_copy.clone_from(&self.canvas.pixels);
                self.layer_active = true;
            }
        }
        if let Some(c) = game_camera {
            self.world.camera = c;
        }
        self.draw_nis_fade();
        // The main view's picture for a saved game's thumbnail, now and then.
        self.frames += 1;
        if self.frames % THUMB_EVERY == 1 && self.nis.is_none() && self.ui.manager().is_none() {
            self.thumb = crate::saves::thumbnail(&self.canvas, THUMB_WIDTH);
        }
        if !self.letterbox.fully && self.nis.is_none() {
            let _p = profile::span("marks");
            self.draw_marks();
        }
        self.ui.letterboxed = self.letterbox.fully || self.nis.is_some();
        self.update_ship_preview();
        let span = profile::span("hud");
        self.ui.draw(&mut self.canvas);
        drop(span);
        let _p = profile::span("fades_letterbox_subtitles");
        self.draw_hyperspace_fades();
        // Over everything: the letterbox bars, then the subtitles (`rndRenderTask`).
        self.letterbox.draw(&mut self.canvas);
        self.layer_cover = (
            self.letterbox
                .drawn_height(self.canvas.width, self.canvas.height),
            self.full_screen_cover(),
        );
        if let Some(s) = self.subtitles.as_mut() {
            let now = self.world.universe().total_time_elapsed;
            let state = subtitle::DrawState {
                letterbox_fading: self.letterbox.fade != 0.0,
                letterboxed: self.letterbox.fully,
                movie: false,
            };
            s.resize(self.canvas.width, self.canvas.height);
            s.draw(&mut self.canvas, now, None, state);
            let report = s.take_message_ended();
            self.intel_trace.message_ended(now, report);
            let r = &s.regions[subtitle::region::LETTERBOX];
            self.intel_trace
                .region(now, r.cards.len(), r.aborted, r.continuous);
            if report != 0 {
                if let Some(run) = self.world.mission_run_mut() {
                    run.game.intel_ended = true;
                }
            }
            // `moSetTop`: the HUD's top panels go below a portrait (next frame's overlay).
            self.ui.subtitle_top(s.hud_top());
            if self.rollcall_below_band {
                let band = s.band_rect(self.canvas.width);
                self.ui.set_rollcall_offset(band.y1);
            }
        }
        self.draw_text();
        Ok(())
    }

    /// The modern build manager's ship view: the selected type turning in its box.
    fn update_ship_preview(&mut self) {
        let Some((r, t)) = self.ui.ship_view() else {
            self.ship_preview.clear();
            return;
        };
        if !hwc_ui::ingame::modern_build_manager::active(&self.ui.fe) {
            return;
        }
        let team = self
            .world
            .team_schemes()
            .first()
            .copied()
            .unwrap_or(([66, 108, 202], [255, 255, 255]));
        let _p = profile::span("ship_preview");
        if let Some(img) = self.ship_preview.update(
            self.world.assets(),
            team,
            t,
            (r.width(), r.height()),
            self.real_time,
        ) {
            self.ui.set_ship_view_image(img);
        }
    }

    pub fn canvas(&self) -> &Canvas {
        &self.canvas
    }

    /// The sizes the 3D view was last drawn with.
    #[cfg(test)]
    pub fn render_plan(&self) -> Option<hwc_render::RenderPlan> {
        self.target.as_ref().map(|t| t.plan)
    }

    /// When the 3D view was drawn at window resolution this frame: the canvas as RGBA with
    /// the pixels nothing was drawn over made transparent, and the full-resolution scene to
    /// show under it. Call after the last drawing into the canvas (the pointer).
    pub fn scene_layer(&self) -> Option<(Vec<u8>, &hwc_render::wgpu::TextureView)> {
        if !self.layer_active
            || self.scene_copy.len() != self.canvas.pixels.len()
            || self.layer_cover.1
        {
            return None;
        }
        let target = self.target.as_ref()?;
        let w = self.canvas.width.max(1) as usize;
        let h = self.canvas.height.max(0) as usize;
        let bars = self.layer_cover.0.max(0) as usize;
        let mut rgba = Vec::with_capacity(self.canvas.pixels.len() * 4);
        for (i, (p, s)) in self.canvas.pixels.iter().zip(&self.scene_copy).enumerate() {
            let y = i / w;
            let covered = y < bars || y + bars >= h;
            let a = if !covered && p[..3] == s[..3] { 0 } else { 255 };
            rgba.extend_from_slice(&[p[0], p[1], p[2], a]);
        }
        Some((rgba, target.present_view()))
    }

    /// Whether a fade or flash was drawn over the whole picture this frame (the NIS fade,
    /// the single-player fade to black, the arrival flash, the hyperspace whiteout).
    fn full_screen_cover(&self) -> bool {
        let nis = self
            .nis
            .as_ref()
            .is_some_and(|n| n.player.fade.level != 0.0);
        let hyper = self.world.mission_run().is_some_and(|r| {
            let h = &r.game.hyper;
            (h.fade > 0.0 && self.nis.is_none()) || h.arrival_flash > 0.0 || h.whiteout
        });
        nis || hyper
    }

    /// What is under the pointer and which pointer shows (`mouseSetCursorSetting`).
    fn update_pointer(&mut self) {
        let size = self.size();
        let p = [self.mouse.0 as f32, self.mouse.1 as f32];
        let over_ui = self.ui.manager().is_some() || hwc_ui::ingame::popup::is_open(&self.ui.fe);
        self.hover = if over_ui || self.sensors.zooming() {
            None
        } else {
            self.pick_ship(size, p).map(Hover::Ship).or_else(|| {
                (!self.sensors.is_open())
                    .then(|| self.world.resource_at(size, p).map(Hover::Resource))
                    .flatten()
            })
        };
        let band = self.left_down.is_some() && self.mode == Mode::Select;
        let action = match self.hover {
            Some(h)
                if self.smart_right_click()
                    && !band
                    && !self.sensors.is_open()
                    && !matches!(h, Hover::Ship(id) if self.selection.contains(&id)) =>
            {
                Some(self.classify(h))
            }
            _ => None,
        };
        let ui = &self.ui;
        let state = cursor::PointerState {
            action,
            screen_up: ui.manager().is_some(),
            menu_open: hwc_ui::ingame::popup::is_open(&ui.fe),
            move_plate: matches!(self.mode, Mode::Move { .. }),
            camera_motion: self.right_down && self.right_moved > RIGHT_CLICK_LIMIT,
            ..Default::default()
        }
        .with_keys(|a| ui.action_held(a));
        let now = self.world.universe().total_time_elapsed;
        self.pointer.update(&state, now);
    }

    /// The letterbox and the subtitles, once a frame after the universe updates.
    fn update_presentation(&mut self, dt: f32, audio: &mut Option<Audio>) {
        // Every `wideScreenIn...` / `wideScreenOut` the mission made since the last frame, in
        // order and with its own frame count. None of them closes the sensors manager
        // (Cataclysm's `kasfWideScreenIn` has the body of `...InDontCloseSensors`); the bars
        // are drawn over it as over the main view.
        let events = self
            .world
            .mission_run_mut()
            .map(|r| std::mem::take(&mut r.game.wide_screen_events))
            .unwrap_or_default();
        let ends = self
            .world
            .mission_run_mut()
            .map(|r| std::mem::take(&mut r.game.intel_ends))
            .unwrap_or_default();
        for e in ends {
            self.intel_trace
                .push(e.time, IntelTraceKind::IntelEnded(e.reason));
        }
        for e in events {
            let coming_in = e.kind != hwc_sim::mission::WideScreenKind::Out;
            if coming_in {
                // `kasfWideScreenIn` and the other ins: normal speed, said every time.
                self.set_speed_level(GAME_SPEED_DEFAULT, true);
            }
            if let Some(id) = self.letterbox.wide_screen_call(coming_in, e.frames) {
                sound(audio, id);
            }
            self.intel_trace.push(
                e.time,
                IntelTraceKind::WideScreen {
                    kind: e.kind,
                    frames: e.frames,
                    sensors_open: self.sensors.is_open(),
                },
            );
            #[cfg(test)]
            self.wide_screen_log.push((e, self.sensors.is_open()));
        }
        if let Some(a) = audio.as_ref() {
            let now = self.world.universe().total_time_elapsed;
            self.intel_trace.playing(now, !a.mission_speech_over());
        }
        self.letterbox.update(dt);
        // Fleet intel holds the keyboard (`smFleetIntel`).
        self.ui.fleet_intel = self
            .world
            .mission_run()
            .is_some_and(|r| r.game.sensors_kas_flag != 0);
        let now = self.world.universe().total_time_elapsed;
        if let Some(s) = self.subtitles.as_mut() {
            s.update(now);
            // `subAnyCardsOnScreen` for the intel safety timeout: the cards this frame's draw
            // keeps (also right when no frame is drawn, as in the headless tests).
            let on_screen = s.any_cards_left(now);
            if let Some(run) = self.world.mission_run_mut() {
                run.game.subtitles_on_screen = on_screen;
            }
        }
    }

    /// Draws the game's pointer (`mouseDraw`) at the mouse position.
    pub fn draw_cursor(&mut self) {
        if self.nis.is_some() {
            return;
        }
        let (x, y) = self.mouse;
        self.pointer.draw(&mut self.canvas, x, y);
    }

    /// The move plate, while it is up.
    #[cfg(test)]
    pub fn plate(&self) -> Option<&Plate> {
        match &self.mode {
            Mode::Move(p) => Some(p),
            Mode::Select => None,
        }
    }

    /// The waypoints placed so far, while placing them.
    #[cfg(test)]
    pub fn placed_waypoints(&self) -> Option<&Vec<Vec3>> {
        self.waypoints.as_ref().filter(|_| self.placing_waypoints())
    }

    /// The pointer shown this frame.
    #[cfg(test)]
    pub fn cursor(&self) -> cursor::CursorKind {
        self.pointer.kind
    }

    /// The sensors view (`smSceneRender`): the dimmed backdrop, the 2D layer under the ships,
    /// the large ships' meshes, then points, icons and hot-key numbers.
    fn render_sensors(&mut self, renderer: &mut Renderer, px: (u32, u32)) -> Result<(), String> {
        let target = self.target.as_ref().expect("target");
        let (clear, dim) = self.sensors.background();
        let show_bg = self.sensors.toggles.background;
        let span = profile::span("sm.frame_prep");
        let frame = self.world.frame(px);
        drop(span);
        let groups: Vec<(u8, Vec<ShipId>)> = self
            .hotkeys
            .non_empty()
            .map(|(n, g)| (n, g.to_vec()))
            .collect();
        let font = self.hotkey_font.clone();
        let inp = sensors::SceneInput {
            world: &self.world,
            size: px,
            selection: &self.selection,
            groups: &groups,
            time: self.real_time,
            font: font.as_deref(),
        };
        // The backdrop, dimmed, with the range discs over it: drawn on the GPU in one pass
        // (the CPU's picture to within one step of 8-bit rounding), unless `sensors_cpu_layer`.
        let gpu_layer = !self.sensors_cpu_layer;
        let mut layer = EffectBatch::default();
        if gpu_layer {
            layer.pixel_triangles = self.sensors.background_layer(&inp);
        }
        // Read back straight into the canvas when it is the target's size.
        let span = profile::span("sm.background_pass");
        let whole = (target.width * target.height) as usize;
        if self.canvas.pixels.len() == whole {
            self.world.render_background(
                renderer,
                target,
                show_bg,
                &layer,
                self.canvas.pixels.as_flattened_mut(),
            )?;
        } else {
            let mut rgba = vec![0u8; whole * 4];
            self.world
                .render_background(renderer, target, show_bg, &layer, &mut rgba)?;
            for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba.as_chunks::<4>().0) {
                *dst = *src;
            }
        }
        drop(span);
        gpu_profile(renderer, "sm.background");
        if !gpu_layer {
            let _p = profile::span("sm.background_copy");
            profile::count("canvas_copies", 1);
            for dst in self.canvas.pixels.iter_mut() {
                *dst = if show_bg {
                    let m = |c: u8| (f32::from(c) * dim) as u8;
                    [m(dst[0]), m(dst[1]), m(dst[2]), 255]
                } else {
                    clear
                };
            }
        }
        self.sensors.draw_under(&mut self.canvas, &inp, gpu_layer);
        let u = self.world.universe();
        let fog = sensors::Fog::of(u);
        let meshes: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| frame.is_mesh(id) && self.sensors.draws_mesh(&inp, &fog, id))
            .collect();
        let meshes: Vec<ShipId> = Sensors::with_clamped(u, &meshes)
            .into_iter()
            .filter(|&id| frame.is_mesh(id))
            .collect();
        profile::count("sm.meshes", meshes.len() as u64);
        if !meshes.is_empty() {
            let span = profile::span("sm.meshes_pass");
            // Only the part of the picture the meshes can cover is read back.
            let region = (!self.sensors_cpu_layer)
                .then(|| mesh_region(&self.world, px, &meshes))
                .flatten();
            let rendered =
                self.world
                    .render_ships_only(renderer, target, &frame, region, |id, _| {
                        meshes.contains(&id)
                    })?;
            drop(span);
            gpu_profile(renderer, "sm.meshes");
            if let Some((rgba, [rx, ry, rw, rh])) = rendered {
                let _p = profile::span("sm.meshes_copy");
                profile::count("sm.meshes_px", u64::from(rw * rh));
                let w = self.canvas.width.max(0) as usize;
                for (row, src_row) in rgba.chunks_exact(rw as usize * 4).enumerate() {
                    let at = (ry as usize + row) * w + rx as usize;
                    let Some(dst_row) = self.canvas.pixels.get_mut(at..at + rw as usize) else {
                        continue;
                    };
                    for (dst, src) in dst_row.iter_mut().zip(src_row.as_chunks::<4>().0) {
                        if src[0] != 0 || src[1] != 0 || src[2] != 0 {
                            *dst = [src[0], src[1], src[2], 255];
                        }
                    }
                }
            }
        }
        self.sensors.draw_over(&mut self.canvas, &inp, &meshes);
        let mut sounds = Vec::new();
        let span = profile::span("sm.pings");
        self.draw_pings(px, &mut sounds);
        drop(span);
        self.ping_sounds.extend(sounds);
        let inp = sensors::SceneInput {
            world: &self.world,
            size: px,
            selection: &self.selection,
            groups: &groups,
            time: self.real_time,
            font: font.as_deref(),
        };
        self.sensors.draw_hotkey_numbers(&mut self.canvas, &inp);
        Ok(())
    }

    fn draw_marks(&mut self) {
        let size = self.size();
        let in_sensors = self.sensors.phase() == Phase::Sensors;
        let u = self.world.universe();
        let mut bars = Vec::new();
        // Health bars are the main view's (`selSelectedDraw`).
        let marked: &[ShipId] = if in_sensors { &[] } else { &self.selection };
        let selecting: &[ShipId] = if in_sensors { &[] } else { &self.selecting };
        // `selSelectingDraw`: the band box's gathered ships are outlined, not given bars.
        let mut outlines = Vec::new();
        for &id in selecting {
            let boxed = match u.ship(id) {
                Some(s) if s.flags & hwc_sim::object::sof::DEAD == 0 => {
                    self.world.screen_box(size, s)
                }
                Some(_) => None,
                None => self.world.mine_box(size, id),
            };
            if let Some((lo, hi)) = boxed {
                outlines.push(Rect::new(
                    lo[0] as i32,
                    lo[1] as i32,
                    hi[0] as i32,
                    hi[1] as i32,
                ));
            }
        }
        for (i, &id) in marked.iter().enumerate() {
            let Some(s) = u.ship(id) else { continue };
            if s.flags & hwc_sim::object::sof::DEAD != 0 {
                continue;
            }
            let Some(st) = u.statics.ship(s.ship_type) else {
                continue;
            };
            let Some((lo, hi)) = self.world.screen_box(size, s) else {
                continue;
            };
            let r = (hi[0] - lo[0]) * 0.5;
            let f = s.health / st.health.max_health.max(1.0);
            // `selSelectedDraw`: a mothership's status is drawn a level of detail lower.
            let mut lod = self.world.ship_lod(id).unwrap_or(0);
            if st.ship_class == Some(hwc_sim::statics::ShipClass::Mothership) && lod > 0 {
                lod -= 1;
            }
            // The number goes with the selection's bars, not the band box's.
            let number = (i < marked.len())
                .then(|| self.hotkeys.group_of(id))
                .flatten();
            bars.push((
                ((lo[0] + hi[0]) * 0.5) as i32,
                ((lo[1] + hi[1]) * 0.5) as i32,
                r,
                f,
                number.map(|n| (n, lod)),
            ));
        }
        if !in_sensors {
            let icons = &self.sensors.icons;
            tactical::draw_icons(&mut self.canvas, &self.world, size, icons, self.ship_icons);
            if self.order_lines {
                tactical::draw_order_lines(&mut self.canvas, &self.world, size, &self.selection);
            }
        }
        for r in outlines {
            self.canvas.outline_rect(r, SELECTING_COLOR);
        }
        for (x, y, r, f, number) in bars {
            use hwc_ui::ingame::overlay;
            overlay::draw_health_bar(&mut self.canvas, x, y, r, f);
            if self.health_brackets {
                overlay::draw_health_outline(&mut self.canvas, overlay::status_rect(x, y, r));
            }
            if let Some((n, lod)) = number {
                let font = self.group_fonts[overlay::hotkey_font_index(lod)].as_deref();
                if let Some(font) = font {
                    let rect = overlay::status_rect(x, y, r);
                    let label = crate::hotkeys::LABELS[usize::from(n)];
                    overlay::draw_hotkey_number(&mut self.canvas, font, rect, label);
                }
            }
        }
        // `mrSelectingDraw`: the band box in the modifier's colour, or before it is dragged
        // (main view only) the box of the ship under the pointer.
        if let (Some(a), Mode::Select, false) = (self.left_down, self.mode, self.right_down) {
            let b = self.mouse;
            let ui = &self.ui;
            let color = cursor::band_box_color(|x| ui.action_held(x));
            if (a.0 - b.0).abs() >= CLICK_SLOP || (a.1 - b.1).abs() >= CLICK_SLOP {
                let r = Rect::new(
                    a.0.min(b.0),
                    a.1.min(b.1),
                    a.0.max(b.0) + 1,
                    a.1.max(b.1) + 1,
                );
                self.canvas.outline_rect(r, color);
            } else if !in_sensors {
                let p = [b.0 as f32, b.1 as f32];
                let boxed = self
                    .world
                    .ship_at(size, p)
                    .and_then(|id| self.world.screen_box(size, self.world.universe().ship(id)?));
                if let Some((lo, hi)) = boxed {
                    let r = Rect::new(lo[0] as i32, lo[1] as i32, hi[0] as i32, hi[1] as i32);
                    self.canvas.outline_rect(r, color);
                }
            }
        }

        if matches!(self.mode, Mode::Move(_)) {
            self.draw_plate();
        }
    }

    /// The move pie plate (`piePointSpecDraw`): the point work of the frame, then the disc,
    /// the lines to the destination, the ships' drop lines and the distance readout.
    fn draw_plate(&mut self) {
        let Mode::Move(mut plate) = self.mode else {
            return;
        };
        // The selection is gone (`pieShipDied`): the plate goes too.
        let Some((centre, avg)) = self.plate_centre() else {
            self.mode = Mode::Select;
            return;
        };
        let view = self.plate_view();
        let universe = v3(self.world.level().sm.universe_size);
        let in_sensors = self.sensors.is_open();
        let mut pointer = self.mouse;
        if plate.update(centre, avg, &view, &mut pointer, universe, in_sensors) {
            self.mouse = pointer;
            self.pointer_warp = Some(pointer);
        }
        let ships = self
            .sensors
            .toggles
            .pieplate_lines
            .then(|| self.drop_ships(universe));
        // `pieWaypointPathDraw`: the path so far, and the line in the waypoint mode's colour.
        let now = std::time::Instant::now();
        let dt = self
            .waypoint_frame
            .map_or(0.0, |t| now.duration_since(t).as_secs_f32());
        self.waypoint_frame = Some(now);
        if let Some(points) = &self.waypoints {
            self.waypoint_rings
                .draw(&mut self.canvas, &view, centre, points, dt);
            plate.line_color = plate::WAYPOINT_LINE_COLORS[(self.waypoint_mode as usize).min(2)];
        }
        let font = self.plate_font.clone();
        let readout_at = (!self.right_down).then_some(self.mouse);
        let chime = plate.draw(
            &mut self.canvas,
            &view,
            &self.plate_colors,
            ships.as_deref(),
            font.as_deref(),
            readout_at,
        );
        if let Some(e) = chime {
            self.ping_sounds.push(e);
        }
        self.mode = Mode::Move(plate);
    }

    /// The ships `pieAllShipsToPiePlateDraw` draws a line for: alive, shown, seen by the
    /// player, inside the universe; not swarmers or components.
    fn drop_ships(&self, universe: Vec3) -> Vec<plate::DropShip> {
        use hwc_sim::object::sof;
        use hwc_sim::statics::ShipClass;
        let u = self.world.universe();
        let fog = sensors::Fog::of(u);
        u.ship_list
            .iter()
            .filter_map(|&id| {
                let s = u.ship(id)?;
                if s.flags & (sof::DEAD | sof::HIDE) != 0
                    || matches!(s.ship_type, ShipType::sSwarmer | ShipType::bSwarmer)
                    || s.base.dock_state == hwc_sim::dock::state::INSIDE
                {
                    return None;
                }
                let st = u.statics.ship(s.ship_type)?;
                if st.ship_class == Some(ShipClass::Component)
                    || !sensors::ship_visible(u, &fog, id)
                {
                    return None;
                }
                let p = v3(s.posinfo.position);
                if (0..3).any(|i| p[i].abs() > universe[i]) {
                    return None;
                }
                Some(plate::DropShip {
                    id,
                    position: p,
                    radius: st.header.coll.coll_sphere_size,
                    selected: self.selection.contains(&id),
                })
            })
            .collect()
    }

    /// The mission-complete note.
    fn draw_text(&mut self) {
        let (w, h) = (self.canvas.width, self.canvas.height);
        let Some(big) = self.big_font.clone() else {
            return;
        };
        let lh = text::height(&big);
        let centred = |canvas: &mut Canvas, s: &str, y: i32| {
            let b = text::to_latin1(s);
            let tw = text::width(&big, &b);
            text::print(
                canvas,
                &big,
                (w - tw) / 2,
                y,
                [255, 255, 255, 255],
                &b,
                (Shadow::SE, [0, 0, 0, 255]),
            );
        };
        if let Some(End::Complete { .. }) = self.end {
            centred(&mut self.canvas, "MISSION COMPLETE", h / 2 - lh);
        }
    }
}

/// How the whiteout is drawn this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Whiteout {
    /// The first half: the blue added over the picture with this alpha.
    Additive([u8; 4]),
    /// The second half: opaque, from the blue down to black.
    Opaque([u8; 4]),
}

/// The whiteout at `t` (1 when the fleet has gone, falling as `UpdateArrivingShip` does):
/// below one half the blue (70, 70, 255) is added with alpha `2t`, above it the screen is the
/// blue scaled by `1 - 2(t - 1/2)`.
pub fn whiteout_colour(t: f32) -> Whiteout {
    let [r, g, b] = WHITEOUT_BLUE;
    if t < 0.5 {
        let a = ((t * 2.0) * 255.0) as i32;
        Whiteout::Additive([r as u8, g as u8, b as u8, a.clamp(0, 255) as u8])
    } else {
        let c = ((1.0 - (t - 0.5) * 2.0) * 255.0) as i32;
        let c = c.clamp(0, 255);
        let k = |v: f32| ((v as i32 * c) >> 8) as u8;
        Whiteout::Opaque([k(r), k(g), k(b), 255])
    }
}

/// The screen rectangle (`x, y, w, h`) the sensors manager's ship meshes can cover: their
/// collision spheres doubled, projected, with a margin; `None` (the whole picture) when one is
/// behind the camera or the rectangles cover most of the screen anyway.
fn mesh_region(world: &World, size: (u32, u32), ships: &[ShipId]) -> Option<[u32; 4]> {
    let u = world.universe();
    let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
    for &id in ships {
        let s = u.ship(id)?;
        let r = u
            .statics
            .ship(s.ship_type)
            .map_or(50.0, |st| st.header.coll.coll_sphere_size)
            .max(20.0);
        let p = glam::Vec3::new(
            s.coll.coll_position.x,
            s.coll.coll_position.y,
            s.coll.coll_position.z,
        );
        let c = sensors::project_sphere(world, size, p, r * 2.0)?;
        let r = c.r + 4.0;
        lo = [lo[0].min(c.x - r), lo[1].min(c.y - r)];
        hi = [hi[0].max(c.x + r), hi[1].max(c.y + r)];
    }
    let (w, h) = (size.0 as f32, size.1 as f32);
    let x0 = lo[0].floor().clamp(0.0, w);
    let y0 = lo[1].floor().clamp(0.0, h);
    let x1 = hi[0].ceil().clamp(0.0, w);
    let y1 = hi[1].ceil().clamp(0.0, h);
    if x1 <= x0 || y1 <= y0 || (x1 - x0) * (y1 - y0) > w * h * 0.5 {
        return None;
    }
    Some([x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32])
}

/// The GPU time of the offscreen render just made, into the profile as pass `name`.
fn gpu_profile(renderer: &mut Renderer, name: &'static str) {
    if let Some(ms) = renderer.take_gpu_ms() {
        profile::gpu_ms(name, ms);
    }
}

/// `mrHarvestResources`: the selection's harvesters collect `resource`, or each the nearest
/// one (`FUN_00445260`). The first acknowledges; with nothing left to harvest each says so.
fn sound(audio: &mut Option<Audio>, e: u32) {
    if let Some(a) = audio.as_mut() {
        a.event(e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whiteout_follows_mr_region_draw() {
        assert_eq!(whiteout_colour(0.0), Whiteout::Additive([70, 70, 255, 0]));
        assert_eq!(
            whiteout_colour(0.25),
            Whiteout::Additive([70, 70, 255, 127])
        );
        // At 1/2 the full blue, at 1 black.
        assert_eq!(whiteout_colour(0.5), Whiteout::Opaque([69, 69, 254, 255]));
        assert_eq!(whiteout_colour(1.0), Whiteout::Opaque([0, 0, 0, 255]));
    }
}

/// The hull look from `hwcShipLook` (1 the modern ship look, 0 the original's) and the parts
/// of it that have their own switch: `hwcSkyLight`, `hwcAmbientOcclusion`, `hwcShadows`
/// and `hwcToneMap`.
pub fn ship_look(options: &hwc_ui::options::Options) -> hwc_render::ShipLook {
    if options.get("hwcShipLook") == 0 {
        return hwc_render::ShipLook::original();
    }
    let on = |key| options.get(key) != 0;
    let full = hwc_render::ShipLook::modern();
    hwc_render::ShipLook {
        sky_light: if on("hwcSkyLight") {
            full.sky_light
        } else {
            0.0
        },
        occlusion: if on("hwcAmbientOcclusion") {
            full.occlusion
        } else {
            0.0
        },
        shadows: on("hwcShadows"),
        tone_map: on("hwcToneMap"),
        ..full
    }
}

/// The post effects from `hwcBloom`, `hwcHeatHaze` and `hwcToneMap` (which also dithers).
pub fn post_fx(options: &hwc_ui::options::Options) -> hwc_render::PostFx {
    let on = |key| options.get(key) != 0;
    let full = hwc_render::PostFx::modern();
    hwc_render::PostFx {
        bloom: if on("hwcBloom") { full.bloom } else { 0.0 },
        heat: if on("hwcHeatHaze") { full.heat } else { 0.0 },
        tone_map: on("hwcToneMap"),
        dither: on("hwcToneMap"),
    }
}
