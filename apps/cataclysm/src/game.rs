//! A running single-player mission: the simulation and its 3D view ([`World`]), the in-game
//! interface (`hwc_ui::ingame::InGameUi`) over a [`SimHost`], the game's own mouse and
//! keyboard handling of the main view (`mainrgn.c`: selection, band boxes, the move pie plate,
//! attack and focus clicks, the right-click menu, camera rotation and zoom), the mission's
//! music, speech and subtitles, and the end of the mission.
//!
//! The window and the headless tests drive it the same way: [`Game::input`] for each event,
//! [`Game::update`] once per frame, [`Game::render`] for the picture.

use std::collections::HashMap;
use std::rc::Rc;

use glam::Vec3;
use hwc_render::{OffscreenTarget, Renderer};
use hwc_sim::base::BaseEvent;
use hwc_sim::harness::group_centre;
use hwc_sim::statics::{ShipType, Technology};
use hwc_sim::{command, dock, formation, ShipId};
use hwc_ui::canvas::{Canvas, Color, Rect};
use hwc_ui::letterbox::{self, Letterbox};
use hwc_ui::subtitle::{self, Subtitles};
use hwc_ui::ingame::host::{Command, Formation, GameMessage, GameRequest, Owner};
use hwc_ui::ingame::keys::{self, Action};
use hwc_ui::ingame::strings::Strings;
use hwc_ui::ingame::{InGameUi, InputEvent, Setup};
use hwc_ui::session::Session;
use hwc_ui::text::{self, Shadow};
use hwc_ui::AssetSource;

use hwc_ui::ingame::cursor::{self, avail, Availability, Pointer, PointerAction};

use crate::audio::Audio;
use crate::host::{selectable, sim_id, SimHost, PLAYER};
use crate::mission::{Cue, CueReader};
use crate::nis::{self, Kind as NisKind, NisRun};
use hwc_nis::{Action as NisAction, Fired};
use crate::sensors::{self, Phase, Sensors};
use crate::world::World;

/// Pixels the pointer may move between press and release and still count as a click
/// (`selClickBoxWidth/Height`: a band box needs `|dx| + 1 > 5`).
const CLICK_SLOP: i32 = 5;
/// Right-button movement that still opens the menu (`MR_MouseMovementClickLimit`).
const RIGHT_CLICK_LIMIT: f32 = 6.0;
/// Frames the mission's letterbox takes to come in or go out. The simulation records only
/// that `wideScreenIn...` / `wideScreenOut` ran, not their frame counts; every shipped call in
/// mission 1 uses 90.
const WIDE_SCREEN_FRAMES: i32 = 90;
/// The mission-complete note stays this long before the game returns to the menu.
const COMPLETE_TIME: f32 = 10.0;
/// The debug key that forces the jump (`singlePlayerCheckDebugKeys`' C key; compiled out of
/// the retail game, here only with [`Game::debug_keys`]).
const DEBUG_HYPERSPACE_KEY: u16 = 0x43;
/// Message colour of a refused jump (`moMessageAdd(..., 0xff0000ff)`).
const RED: [u8; 4] = [255, 0, 0, 255];
/// `hyperspaceWhiteout`'s blue (70, 70, 255).
const WHITEOUT_BLUE: [f32; 3] = [70.0, 70.0, 255.0];
/// UI sound events of the move mechanism (`UI_MovementGUIon/off`).
const MOVEMENT_GUI_ON: u32 = 0x109;
const MOVEMENT_GUI_OFF: u32 = 0x10a;
const CLICK: u32 = 0x108;

const PLATE: Color = [60, 120, 255, 160];
const PLATE_LINE: Color = [255, 220, 60, 255];

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
}

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
    /// the selection's plane, with a height while Enable_Height_Move is held.
    Move {
        height: f32,
    },
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
    selecting: Vec<ShipId>,
    groups: HashMap<u8, Vec<ShipId>>,
    messages: Vec<GameMessage>,
    /// Speech subtitles, simulated subtitles and location cards (`Subtitle.c`).
    pub subtitles: Option<Subtitles>,
    /// The NIS letterbox the mission's `wideScreenIn/Out` drive.
    pub letterbox: Letterbox,
    /// The simulation's widescreen flag last seen.
    wide_screen: bool,
    pub mode: Mode,
    mouse: (i32, i32),
    left_down: Option<(i32, i32)>,
    right_down: bool,
    right_moved: f32,
    /// Paused by the UI (escape menu, objectives) or the pause key.
    pub paused: bool,
    pub end: Option<End>,
    pub music: Option<i32>,
    formation_index: usize,
    canvas: Canvas,
    target: Option<OffscreenTarget>,
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
}

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
        let mut sensors = Sensors::new(tweaks, sensors::Toggles::from_options(&options), icons);
        sensors.instant = options.get("InstantTransition") != 0;
        let pointer = Pointer::new(hwc_ui::Resources::new(source.clone()));
        let mut res = hwc_ui::Resources::new(source);
        res.set_language(hwc_ui::Language::from_index(options.get("language") as u32));
        let mut g = Game {
            world,
            ui,
            strings,
            cues: CueReader::default(),
            selection: Vec::new(),
            selecting: Vec::new(),
            groups: HashMap::new(),
            messages: Vec::new(),
            subtitles: None,
            letterbox: Letterbox::default(),
            wide_screen: false,
            mode: Mode::Select,
            mouse: (0, 0),
            left_down: None,
            right_down: false,
            right_moved: 0.0,
            paused: false,
            end: None,
            music: None,
            formation_index: 0,
            canvas: Canvas::new(640, 480),
            target: None,
            big_font: res.font("Arial_13.hff"),
            hotkey_font: res.font("default.hff"),
            real_time: 0.0,
            sensors,
            pointer,
            pan_arrows: [false; 4],
            hover: None,
            nis: None,
            keep_letterbox_after_nis: false,
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
        g.keep_letterbox_after_nis = g.world.mission_run().is_some_and(|r| r.game.config.mission == 1);
        g.world.lens_flares = options.get("enableLensFlare") != 0;
        let mut subs = Subtitles::load(&res, true, &g.world.team_schemes());
        if let Some(s) = subs.as_mut() {
            s.language = options.get("language").max(0) as usize;
        }
        g.subtitles = subs;
        Ok(g)
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
            messages: &self.messages,
            paused: self.paused,
            // Escape first cancels the move plate, the sensors manager or the selection
            // (`kbcbEscape`).
            escape_pending: self.mode != Mode::Select
                || self.sensors.is_open()
                || !self.selection.is_empty(),
            in_sensors: self.sensors.is_open(),
            resource_units: world.universe().base.players[PLAYER].resource_units,
            screen_pos: &pos,
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
                let min = self.world.camera.closest_zoom;
                self.world
                    .camera
                    .mouse_drag(dx as f32, dy as f32, both, min);
                return;
            }
            if let Mode::Move { height } = self.mode {
                if self.ui.action_held(keys::ENABLE_HEIGHT_MOVE) {
                    let scale = self.world.camera.distance / self.size().1 as f32 * 2.0;
                    self.mode = Mode::Move {
                        height: height - dy as f32 * scale,
                    };
                }
            }
        }
        let used = self.with_host(|ui, host| ui.handle_input(host, ev));
        if used {
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
                } else if self.right_down {
                    self.right_down = false;
                    if self.right_moved <= RIGHT_CLICK_LIMIT {
                        self.right_click(audio);
                    }
                }
            }
            InputEvent::Wheel { up } => {
                let min = self.world.camera.closest_zoom;
                self.world.camera.wheel(up, min);
            }
            _ => {}
        }
    }

    fn left_release(&mut self, start: Option<(i32, i32)>, audio: &mut Option<Audio>) {
        let size = self.size();
        let p = [self.mouse.0 as f32, self.mouse.1 as f32];
        if let Mode::Move { height } = self.mode {
            if let Some(dest) = self.move_destination(height) {
                let sel = self.mobile_selection();
                if !sel.is_empty() {
                    let from = group_centre(self.world.universe(), &sel);
                    let d = hwc_math::Vector::new(dest.x, dest.y, dest.z);
                    command::cl_move(
                        self.world.universe_mut(),
                        &sel,
                        command::waypoint_mode::ONCE,
                        from,
                        &[d],
                    );
                }
            }
            sound(audio, MOVEMENT_GUI_OFF);
            self.mode = Mode::Select;
            return;
        }
        let Some(start) = start else { return };
        let dragged = (start.0 - self.mouse.0).abs() >= CLICK_SLOP
            || (start.1 - self.mouse.1).abs() >= CLICK_SLOP;
        self.selecting.clear();
        if dragged {
            let a = [start.0 as f32, start.1 as f32];
            let inside = self.ships_in_box(a, p);
            if self.ui.action_held(keys::BANDBOX_FOCUS) {
                if !inside.is_empty() {
                    self.focus_on(inside, audio);
                }
            } else if self.ui.action_held(keys::BANDBOX_ATTACK)
                || self.ui.action_held(keys::BANDBOX_FORCE_ATTACK)
            {
                let force = self.ui.action_held(keys::BANDBOX_FORCE_ATTACK);
                let u = self.world.universe();
                let targets: Vec<ShipId> = inside
                    .into_iter()
                    .filter(|id| !self.selection.contains(id))
                    .filter(|&id| force || u.ship(id).is_some_and(|s| s.player != PLAYER))
                    .collect();
                self.attack(&targets);
            } else {
                let own: Vec<ShipId> = inside
                    .into_iter()
                    .filter(|&id| selectable(self.world.universe(), id))
                    .collect();
                if self.ui.action_held(keys::MULTIPLE_SELECT) {
                    for id in own {
                        if !self.selection.contains(&id) {
                            self.selection.push(id);
                        }
                    }
                } else {
                    self.selection = own;
                }
            }
            return;
        }
        // A click.
        match self.pick_ship(size, p) {
            Some(id) => self.object_click(id, audio),
            None => {
                if !self.ui.action_held(keys::MULTIPLE_SELECT) {
                    self.selection.clear();
                }
            }
        }
    }

    /// `mrObjectClick`.
    fn object_click(&mut self, id: ShipId, audio: &mut Option<Audio>) {
        let u = self.world.universe();
        let Some(s) = u.ship(id) else { return };
        let own = s.player == PLAYER;
        if self.ui.action_held(keys::BANDBOX_FOCUS) {
            self.focus_on(vec![id], audio);
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
        }
        sound(audio, CLICK);
    }

    /// Right click without moving the camera: the menu (`mrRightClickMenu`).
    fn right_click(&mut self, audio: &mut Option<Audio>) {
        if let Mode::Move { .. } = self.mode {
            self.mode = Mode::Select;
            sound(audio, MOVEMENT_GUI_OFF);
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        // `mrRightClickSmartOrder`: with a context-sensitive scheme the action under the
        // pointer, otherwise (and in the sensors manager) the menu.
        if !self.sensors.is_open() && self.context_scheme() {
            if let Some(h) = self.hover {
                let in_sel = matches!(h, Hover::Ship(id) if self.selection.contains(&id));
                if !in_sel && self.smart_order(h) {
                    return;
                }
            }
        }
        self.with_host(|ui, host| ui.open_popup(host, false));
    }

    /// A context-sensitive action scheme is on (`RightClick_CSA` or `Homeworld_CSA`).
    fn context_scheme(&self) -> bool {
        let o = &self.ui.fe.game.options;
        o.get("RightClick_CSA") != 0 || o.get("Homeworld_CSA") != 0
    }

    /// The classifier's action on an object (`mouseCursorActionClassify`).
    fn classify(&self, h: Hover) -> PointerAction {
        let sel = self.availability(&self.selection);
        let (tgt, facts) = self.target_facts(h);
        let classic = self.ui.fe.game.options.get("Homeworld_CSA") != 0;
        cursor::classify(&sel, &tgt, self.selection.len(), &facts, classic)
    }

    /// Carries out the right-click action on an object; false opens the menu instead.
    fn smart_order(&mut self, h: Hover) -> bool {
        match (self.classify(h), h) {
            (PointerAction::Harvest, Hover::Resource(r)) => {
                let sel = self.selection.clone();
                hwc_sim::resource::cl_collect_resource(self.world.universe_mut(), &sel, Some(r));
                true
            }
            (PointerAction::Attack, Hover::Ship(t)) => {
                self.attack(&[t]);
                true
            }
            (PointerAction::Dock, Hover::Ship(t)) => {
                let sel = self.selection.clone();
                dock::cl_dock(
                    self.world.universe_mut(),
                    &sel,
                    dock::purpose::DOCK,
                    Some(t),
                );
                true
            }
            _ => false,
        }
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
        t[avail::ALL_MISSILES] = any;
        for &id in ships {
            let Some(s) = u.ship(id) else { continue };
            let st = u.statics.ship(s.ship_type);
            let class = st.and_then(|st| st.ship_class);
            let ti = s.ship_type.index();
            if s.player == PLAYER || u.players.get(PLAYER).is_some_and(|p| p.is_ally(s.player)) {
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
            Hover::Resource(_) => {
                let mut t = [false; avail::LEN];
                t[avail::RESOURCE] = true;
                (t, facts)
            }
            Hover::Ship(id) => {
                let t = self.availability(&[id]);
                let u = self.world.universe();
                if let Some(s) = u.ship(id) {
                    facts.own_ship = s.player == PLAYER;
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
                }
                (t, facts)
            }
        }
    }

    /// The ship under the pointer among those the local player can see (in the sensors
    /// manager the fog of war hides ships).
    fn pick_ship(&self, size: (u32, u32), p: [f32; 2]) -> Option<ShipId> {
        if self.sensors.is_open() {
            let u = self.world.universe();
            let fog = sensors::Fog::of(u);
            self.world
                .ship_at_where(size, p, |id| sensors::ship_visible(u, &fog, id))
        } else {
            self.world.ship_at(size, p)
        }
    }

    /// Focus on ships: in the sensors manager, zoom back onto them.
    fn focus_on(&mut self, ships: Vec<ShipId>, audio: &mut Option<Audio>) {
        if self.sensors.is_open() {
            if let Some(e) = self.sensors.close_on(&self.world, &ships) {
                sound(audio, e);
            }
        } else {
            self.world.focus = ships;
        }
    }

    /// Opens or closes the sensors manager (`Toggle_SensorsManager`).
    fn toggle_sensors(&mut self, audio: &mut Option<Audio>) {
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

    /// Ships whose centres project inside the band box.
    fn ships_in_box(&self, a: [f32; 2], b: [f32; 2]) -> Vec<ShipId> {
        let size = self.size();
        let (lo, hi) = (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        );
        let u = self.world.universe();
        let fog = sensors::Fog::of(u);
        let in_sensors = self.sensors.is_open();
        u.ship_list
            .iter()
            .copied()
            .filter(|&id| !in_sensors || sensors::ship_visible(u, &fog, id))
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

    /// The pie plate's destination under the pointer.
    pub fn move_destination(&self, height: f32) -> Option<Vec3> {
        let c = self.selection_centre()?;
        let p = [self.mouse.0 as f32, self.mouse.1 as f32];
        let on_plane = self.world.plane_point(self.size(), p, c.z)?;
        Some(on_plane + Vec3::new(0.0, 0.0, height))
    }

    fn attack(&mut self, targets: &[ShipId]) {
        if targets.is_empty() || self.selection.is_empty() {
            return;
        }
        let sel = command::attack_capable(self.world.universe(), &self.selection);
        if !sel.is_empty() {
            command::cl_attack(self.world.universe_mut(), &sel, targets);
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
                    hwc_sim::construction::queue_build(
                        self.world.universe_mut(),
                        sim_id(factory),
                        t,
                        count,
                    );
                }
            }
            GameRequest::Unqueue {
                factory,
                ship_type,
                count,
            } => self.unqueue(sim_id(factory), ship_type, count),
            GameRequest::CancelBuild { factory, ship_type } => {
                self.cancel_build(sim_id(factory), ship_type)
            }
            GameRequest::PauseBuild {
                factory,
                ship_type,
                paused,
            } => {
                if let Some(j) = self.job_mut(sim_id(factory), ship_type) {
                    j.paused = paused;
                }
            }
            GameRequest::Research { tech, .. } => {
                hwc_sim::research::research_tech_if_lab_free(
                    self.world.universe_mut(),
                    PLAYER,
                    tech as usize,
                );
            }
            GameRequest::ClearLab { lab } => {
                hwc_sim::research::clear_research_lab(self.world.universe_mut(), PLAYER, lab)
            }
            GameRequest::Launch { launcher, ships } => {
                let ships: Vec<ShipId> = ships.into_iter().map(sim_id).collect();
                dock::cl_launch_multiple_ships(self.world.universe_mut(), &ships, sim_id(launcher));
            }
            GameRequest::SetAutoLaunch { launcher, on } => {
                let u = self.world.universe();
                let bit = dock::auto_launch_bit(u, sim_id(launcher));
                let bits = u.base.players[PLAYER].auto_launch;
                let bits = if on { bits | bit } else { bits & !bit };
                dock::cl_auto_launch(self.world.universe_mut(), bits, PLAYER);
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
                            && (owner == Owner::Enemy) == (s.player != PLAYER)
                    })
                });
            }
            GameRequest::DeselectTypes(types) => {
                let u = self.world.universe();
                self.selection.retain(|&id| {
                    u.ship(id).is_none_or(|s| {
                        !types.iter().any(|(t, _)| *t == s.ship_type.index() as u32)
                    })
                });
            }
            GameRequest::FocusShip(id) => {
                let id = sim_id(id);
                if selectable(self.world.universe(), id) {
                    self.selection = vec![id];
                }
                self.world.focus = vec![id];
            }
            GameRequest::Action { action, down } => self.action(action, down, audio),
            GameRequest::SetPaused(p) => self.paused = p,
            GameRequest::QuitToMain => out.push(Outcome::Leave),
            GameRequest::RestartLevel => out.push(Outcome::Restart),
            GameRequest::SaveGame(_) | GameRequest::LoadGame(_) => {
                self.post("Saving and loading are not available yet.")
            }
            GameRequest::Sound(e) => sound(audio, e),
            GameRequest::Speech { event, .. } => {
                if let Some(a) = audio {
                    a.speech(event);
                }
            }
            GameRequest::ApplyOptions | GameRequest::AudioSettings => {
                out.push(Outcome::ApplyOptions)
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
            GameRequest::SetRepairPriority { .. }
            | GameRequest::BeastSystems(_)
            | GameRequest::TraderAccept(_)
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
        if let Err(hwc_sim::campaign::HyperspaceRefused::ShipUnderConstruction) =
            self.world.run_mut().hyperspace_button()
        {
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

    fn job_mut(
        &mut self,
        factory: ShipId,
        ship_type: u32,
    ) -> Option<&mut hwc_sim::construction::BuildJob> {
        self.world
            .universe_mut()
            .base
            .factories
            .iter_mut()
            .find(|f| f.ship == Some(factory))?
            .jobs
            .iter_mut()
            .find(|j| j.n_jobs > 0 && j.info.is_some_and(|t| t.index() as u32 == ship_type))
    }

    /// `cmDecrement`: take queued ships off a job, never the one in progress.
    fn unqueue(&mut self, factory: ShipId, ship_type: u32, count: i32) {
        if let Some(j) = self.job_mut(factory, ship_type) {
            let n = count.min(j.n_jobs - 1).max(0);
            j.n_jobs -= n;
            j.n_total = (j.n_total - n).max(j.n_jobs);
        }
    }

    /// `cmCancelJobs`: the job goes, what the current ship cost so far comes back.
    fn cancel_build(&mut self, factory: ShipId, ship_type: u32) {
        let mut refund = 0;
        if let Some(j) = self.job_mut(factory, ship_type) {
            refund = j.cost_so_far;
            j.n_jobs = 0;
            j.n_total = 0;
            j.cost_so_far = 0;
            j.time_left = 0;
            j.paused = false;
        }
        self.world.universe_mut().base.players[PLAYER].resource_units += refund;
    }

    fn command(&mut self, c: Command, audio: &mut Option<Audio>) {
        let sel = self.selection.clone();
        if sel.is_empty() {
            return;
        }
        let u = self.world.universe_mut();
        match c {
            Command::Move => self.start_move(audio),
            Command::Cancel => command::cl_halt(u, &sel),
            Command::Dock => {
                dock::cl_dock(u, &sel, dock::purpose::DOCK, None);
            }
            Command::Retire => {
                dock::cl_dock(u, &sel, dock::purpose::RETIRE, None);
            }
            Command::Harvest => hwc_sim::resource::cl_collect_resource(u, &sel, None),
            Command::TacticsEvasive => {
                command::cl_set_tactics(u, &sel, hwc_sim::object::tactics::EVASIVE)
            }
            Command::TacticsNeutral => {
                command::cl_set_tactics(u, &sel, hwc_sim::object::tactics::NEUTRAL)
            }
            Command::TacticsAggressive => {
                command::cl_set_tactics(u, &sel, hwc_sim::object::tactics::AGGRESSIVE)
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
                if sel.len() > 1 {
                    command::cl_formation(u, &sel, t);
                }
            }
            other => self.post(&format!("{other:?}: not available yet")),
        }
    }

    fn start_move(&mut self, audio: &mut Option<Audio>) {
        if self.mobile_selection().is_empty() {
            return;
        }
        if self.mode == Mode::Select {
            self.mode = Mode::Move { height: 0.0 };
            sound(audio, MOVEMENT_GUI_ON);
        } else {
            self.mode = Mode::Select;
            sound(audio, MOVEMENT_GUI_OFF);
        }
    }

    /// Key actions the game handles (`mrKeyPress`).
    fn action(&mut self, a: Action, down: bool, audio: &mut Option<Audio>) {
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
        if !down {
            return;
        }
        if (136..=147).contains(&a.0) && self.sensors.toggles.toggle(a.0 as u8) {
            return;
        }
        if let Some((op, n)) = a.ship_group() {
            use hwc_ui::ingame::keys::GroupOp;
            match op {
                GroupOp::Create => {
                    self.groups.insert(n, self.selection.clone());
                }
                GroupOp::AddTo => {
                    let g = self.groups.entry(n).or_default();
                    for &id in &self.selection {
                        if !g.contains(&id) {
                            g.push(id);
                        }
                    }
                }
                GroupOp::Select => {
                    let u = self.world.universe();
                    if let Some(g) = self.groups.get_mut(&n) {
                        g.retain(|&id| u.ship(id).is_some());
                        self.selection = g.clone();
                    }
                }
            }
            return;
        }
        match a {
            keys::MOVE => self.start_move(audio),
            keys::FOCUS => {
                if !self.selection.is_empty() {
                    let sel = self.selection.clone();
                    self.focus_on(sel, audio);
                }
            }
            keys::TOGGLE_SENSORS_MANAGER => self.toggle_sensors(audio),
            keys::FOCUS_ON_MOTHERSHIP => {
                if let Some(ms) = self.world.universe().base.players[PLAYER].mothership {
                    self.world.focus = vec![ms];
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
                command::cl_formation(
                    self.world.universe_mut(),
                    &sel,
                    formation::CYCLE[self.formation_index],
                );
            }
            _ => {}
        }
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
    // Frame
    // ---------------------------------------------------------------------------------------

    /// One frame of `dt` seconds: the interface, the universe updates that are due, the
    /// mission's cues, the sound.
    pub fn update(&mut self, dt: f32, audio: &mut Option<Audio>) -> Vec<Outcome> {
        let mut out = Vec::new();
        if let Some((run, fired)) = self.queued_nis.take() {
            self.begin_nis(run, fired, audio);
        }
        self.real_time += dt;
        let requests = self.with_host(|ui, host| ui.update(host, dt));
        for r in requests {
            self.apply(r, audio, &mut out);
        }
        // Polled keys: zoom (the sensors camera's while the sensors manager is up).
        let min = self.world.camera.closest_zoom;
        if !self.sensors.zooming() {
            if self.ui.action_held(keys::ZOOM_IN) {
                self.world.camera.key_zoom(true, min);
            }
            if self.ui.action_held(keys::ZOOM_OUT) {
                self.world.camera.key_zoom(false, min);
            }
        }
        // The sensors manager: a mission opening or closing it, the zoom, panning.
        let kas_open = self.world.mission_run().is_some_and(|r| r.game.sensors_open);
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
                self.selecting =
                    self.ships_in_box([a.0 as f32, a.1 as f32], [b.0 as f32, b.1 as f32]);
            }
        }
        self.world.paused = (self.paused && self.nis.is_none())
            || self.movie
            || self.next_mission
            || self.nis_level_done;
        self.world.sensors_open = self.sensors.is_open() && self.nis.is_none();
        let steps = self.world.due_steps(f64::from(dt));
        for _ in 0..steps {
            if self.movie || self.next_mission || self.nis_level_done {
                break;
            }
            // An NIS level has no mission yet: only its universe updates.
            let opening = self.world.nis_level().is_some();
            if opening {
                self.world.step_universe();
            } else {
                if let (Some(a), None) = (self.autoplay.as_mut(), &self.nis) {
                    a.tick(self.world.run_mut());
                }
                self.world.step();
            }
            if let Some(a) = audio.as_mut() {
                a.sim_events(self.world.universe(), !self.world.effects_on);
                a.effect_events(self.world.universe());
            }
            if !opening {
                self.after_step(audio);
            }
            self.nis_step(audio);
            if !opening {
                self.campaign_step(audio, &mut out);
            }
        }
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
        self.update_presentation(dt, audio);
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

    /// The jump's movie and the next mission (`singlePlayerGameUpdate` state 5).
    fn campaign_step(&mut self, audio: &mut Option<Audio>, out: &mut Vec<Outcome>) {
        let h = &self.world.run().game.hyper;
        let mission = self.world.run().game.config.mission;
        if self.bink_pending && !h.bink_pending && h.whiteout_t > 0.0 {
            // `animBinkPlay(m, m + 1)`: music and sound stop, the game waits for the movie.
            self.movie = true;
            if let Some(a) = audio.as_mut() {
                a.stop_music(0.0);
            }
            out.push(Outcome::Animatic(mission + 1));
        }
        self.bink_pending = h.bink_pending;
        if self.world.run().mission_complete() && !self.next_mission {
            self.next_mission = true;
            out.push(Outcome::NextMission);
        }
    }

    /// The between-missions movie has ended (or was skipped): the game goes on.
    pub fn movie_finished(&mut self) {
        self.movie = false;
    }

    fn after_step(&mut self, audio: &mut Option<Audio>) {
        let u = &self.world.run().universe;
        self.selection.retain(|&id| selectable(u, id));
        for g in self.groups.values_mut() {
            g.retain(|&id| u.ship(id).is_some());
        }
        let now = u.total_time_elapsed;
        let mut posts = Vec::new();
        for e in &u.base.events {
            match e {
                BaseEvent::ShipBuilt {
                    ship_type, player, ..
                } if *player == PLAYER => {
                    let name = self
                        .strings
                        .get(&format!("str{}", ship_type.name()))
                        .unwrap_or(ship_type.name())
                        .trim()
                        .to_string();
                    posts.push(format!("{name} built"));
                }
                BaseEvent::Researched { player, tech } if *player == PLAYER => {
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
        if let (Some(s), Some(actor), Some(subs)) = (spoken, actor, self.subtitles.as_mut()) {
            let n = s.subtitles.len().max(1) as f32;
            let each = s.duration_ms.max(0) as f32 / 1000.0 / n;
            for t in &s.subtitles {
                subs.add(actor, event, &text::to_latin1(t), each);
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // NIS
    // ---------------------------------------------------------------------------------------

    /// An NIS starts playing (`singlePlayerStartNis`): the managers and menus close, the
    /// volumes are kept to restore, the letterbox comes in as the script says, and the
    /// events at time 0 run.
    pub fn begin_nis(&mut self, mut run: NisRun, fired: Vec<Fired>, audio: &mut Option<Audio>) {
        if self.sensors.is_open() {
            self.close_sensors(audio);
        }
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
        // `lensFlareNISSunRemove`.
        self.world.nis_sun.remove();
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
            // The bars stay as the mission's widescreen: its next `wideScreenOut` takes them
            // away (Mission 1 calls it when the first fleet-intel event ends, without a
            // `wideScreenIn` of its own).
            if let Some(r) = self.world.mission_run_mut() {
                r.game.wide_screen = true;
            }
            self.wide_screen = true;
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
        let full = letterbox::bar_height(w, h);
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
        if self
            .target
            .as_ref()
            .is_none_or(|t| (t.width, t.height) != px)
        {
            self.target = Some(OffscreenTarget::new(renderer, px.0, px.1));
        }
        self.world.prepare(renderer);
        let game_camera = self.nis.as_ref().map(|_| self.world.camera);
        if let Some(n) = &self.nis {
            n.view(&mut self.world);
        }
        if self.sensors.phase() == Phase::Sensors && self.nis.is_none() {
            self.render_sensors(renderer, px)?;
        } else {
            // The main-view part of a sensors zoom uses the main camera's clip planes.
            let saved = self.world.camera.tweaks;
            if let Some(m) = self.sensors.main_camera() {
                self.world.camera.tweaks = m.tweaks;
            }
            let frame = self.world.frame(px);
            let target = self.target.as_ref().expect("target");
            let rgba = self.world.render_offscreen(renderer, target, &frame);
            for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba?.as_chunks::<4>().0) {
                *dst = [src[0], src[1], src[2], 255];
            }
            self.world.camera.tweaks = saved;
        }
        if let Some(c) = game_camera {
            self.world.camera = c;
        }
        self.draw_nis_fade();
        if !self.letterbox.fully && self.nis.is_none() {
            self.draw_marks();
        }
        self.ui.letterboxed = self.letterbox.fully || self.nis.is_some();
        self.ui.draw(&mut self.canvas);
        self.draw_hyperspace_fades();
        // Over everything: the letterbox bars, then the subtitles (`rndRenderTask`).
        self.letterbox.draw(&mut self.canvas);
        if let Some(s) = self.subtitles.as_mut() {
            let now = self.world.universe().total_time_elapsed;
            let state = subtitle::DrawState {
                letterbox_fading: self.letterbox.fade != 0.0,
                letterboxed: self.letterbox.fully,
                movie: false,
            };
            s.resize(self.canvas.width, self.canvas.height);
            s.draw(&mut self.canvas, now, None, state);
            if s.take_message_ended() != 0 {
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

    pub fn canvas(&self) -> &Canvas {
        &self.canvas
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
                if self.context_scheme()
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
        let wide = self.world.mission_run().is_some_and(|r| r.game.wide_screen);
        if wide != self.wide_screen {
            self.wide_screen = wide;
            if wide {
                if self.letterbox.wide_screen_in(WIDE_SCREEN_FRAMES) {
                    sound(audio, letterbox::SOUND_IN);
                }
            } else {
                self.letterbox.wide_screen_out(WIDE_SCREEN_FRAMES);
                sound(audio, letterbox::SOUND_OUT);
            }
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
        let rgba = self.world.render_background(renderer, target, show_bg)?;
        for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba.as_chunks::<4>().0) {
            *dst = if show_bg {
                let m = |c: u8| (f32::from(c) * dim) as u8;
                [m(src[0]), m(src[1]), m(src[2]), 255]
            } else {
                clear
            };
        }
        let frame = self.world.frame(px);
        let mut groups: Vec<(u8, Vec<ShipId>)> =
            self.groups.iter().map(|(k, v)| (*k, v.clone())).collect();
        groups.sort_by_key(|g| g.0);
        let font = self.hotkey_font.clone();
        let inp = sensors::SceneInput {
            world: &self.world,
            size: px,
            selection: &self.selection,
            groups: &groups,
            time: self.real_time,
            font: font.as_deref(),
        };
        self.sensors.draw_under(&mut self.canvas, &inp);
        let u = self.world.universe();
        let fog = sensors::Fog::of(u);
        let meshes: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| frame.is_mesh(id) && self.sensors.draws_mesh(&inp, &fog, id))
            .collect();
        if !meshes.is_empty() {
            let rendered = self
                .world
                .render_ships_only(renderer, target, &frame, |id, _| meshes.contains(&id))?;
            if let Some(rgba) = rendered {
                for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba.as_chunks::<4>().0) {
                    if src[0] != 0 || src[1] != 0 || src[2] != 0 {
                        *dst = [src[0], src[1], src[2], 255];
                    }
                }
            }
        }
        self.sensors.draw_over(&mut self.canvas, &inp, &meshes);
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
        for &id in marked.iter().chain(selecting) {
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
            bars.push((
                ((lo[0] + hi[0]) * 0.5) as i32,
                ((lo[1] + hi[1]) * 0.5) as i32,
                r,
                f,
            ));
        }
        for (x, y, r, f) in bars {
            hwc_ui::ingame::overlay::draw_health_bar(&mut self.canvas, x, y, r, f);
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

        if let Mode::Move { height } = self.mode {
            self.draw_plate(height);
        }
    }

    /// The move pie plate: rings and spokes on the plane through the selection, the line to
    /// the destination and its height.
    fn draw_plate(&mut self, height: f32) {
        let size = self.size();
        let Some(c) = self.selection_centre() else {
            return;
        };
        let dest = self.move_destination(height);
        let radius = dest.map_or(2000.0, |d| (d - c).truncate().length().max(300.0));
        let mut lines: Vec<([f32; 2], [f32; 2], Color)> = Vec::new();
        let proj = |p: Vec3| self.world.project(size, p);
        for k in 1..=3 {
            let r = radius * k as f32 / 3.0;
            let pts: Vec<Option<[f32; 2]>> = (0..=48)
                .map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    proj(c + Vec3::new(a.cos() * r, a.sin() * r, 0.0))
                })
                .collect();
            for w in pts.windows(2) {
                if let (Some(a), Some(b)) = (w[0], w[1]) {
                    lines.push((a, b, PLATE));
                }
            }
        }
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            if let (Some(p), Some(q)) = (
                proj(c),
                proj(c + Vec3::new(a.cos() * radius, a.sin() * radius, 0.0)),
            ) {
                lines.push((p, q, PLATE));
            }
        }
        if let Some(d) = dest {
            let foot = Vec3::new(d.x, d.y, c.z);
            if let (Some(a), Some(b), Some(t)) = (proj(c), proj(foot), proj(d)) {
                lines.push((a, b, PLATE_LINE));
                lines.push((b, t, PLATE_LINE));
            }
        }
        for (a, b, col) in lines {
            self.canvas
                .line(a[0] as i32, a[1] as i32, b[0] as i32, b[1] as i32, col);
        }
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
        assert_eq!(whiteout_colour(0.25), Whiteout::Additive([70, 70, 255, 127]));
        // At 1/2 the full blue, at 1 black.
        assert_eq!(whiteout_colour(0.5), Whiteout::Opaque([69, 69, 254, 255]));
        assert_eq!(whiteout_colour(1.0), Whiteout::Opaque([0, 0, 0, 255]));
    }
}
