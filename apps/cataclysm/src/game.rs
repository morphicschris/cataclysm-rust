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
use crate::sensors::{self, Phase, Sensors};
use crate::world::World;

/// Pixels the pointer may move between press and release and still count as a click
/// (`selClickBoxWidth/Height`: a band box needs `|dx| + 1 > 5`).
const CLICK_SLOP: i32 = 5;
/// Right-button movement that still opens the menu (`MR_MouseMovementClickLimit`).
const RIGHT_CLICK_LIMIT: f32 = 6.0;
/// Subtitles without a known length stay this long (`subTitleShortest`).
const SUBTITLE_SHORTEST: f32 = 2.0;
/// The mission-complete note stays this long before the game returns to the menu.
const COMPLETE_TIME: f32 = 10.0;
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
    /// Subtitle text and the game time it goes away.
    subtitles: Vec<(String, f32)>,
    location: Option<(String, f32)>,
    pub mode: Mode,
    mouse: (i32, i32),
    left_down: Option<(i32, i32)>,
    right_down: bool,
    right_moved: f32,
    /// Paused by the UI (escape menu, objectives) or the pause key.
    pub paused: bool,
    /// The mission's `RUsSet` value last copied onto the player.
    kas_rus: Option<i32>,
    pub end: Option<End>,
    pub music: Option<i32>,
    formation_index: usize,
    canvas: Canvas,
    target: Option<OffscreenTarget>,
    subtitle_font: Option<Rc<hwc_formats::font::Font>>,
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
            subtitles: Vec::new(),
            location: None,
            mode: Mode::Select,
            mouse: (0, 0),
            left_down: None,
            right_down: false,
            right_moved: 0.0,
            paused: false,
            kas_rus: None,
            end: None,
            music: None,
            formation_index: 0,
            canvas: Canvas::new(640, 480),
            target: None,
            subtitle_font: res.font("Arial_13.hff"),
            big_font: res.font("Arial_13.hff"),
            hotkey_font: res.font("default.hff"),
            real_time: 0.0,
            sensors,
            pointer,
            pan_arrows: [false; 4],
            hover: None,
        };
        g.sync_resource_units();
        Ok(g)
    }

    /// The mission sets RUs on its own state (`RUsSet`); construction charges the player's.
    fn sync_resource_units(&mut self) {
        let kas = self.world.run.game.resource_units.first().copied();
        if kas != self.kas_rus {
            self.kas_rus = kas;
            if let Some(r) = kas {
                self.world.universe_mut().base.players[PLAYER].resource_units = r;
            }
        }
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
            run: &world.run,
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
            GameRequest::Command(c) => self.command(c, audio),
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
            GameRequest::SetRepairPriority { .. }
            | GameRequest::BeastSystems(_)
            | GameRequest::TraderAccept(_)
            | GameRequest::TraderLeave => {}
        }
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
            keys::PAUSE => self.paused = !self.paused,
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
        let kas_open = self.world.run.game.sensors_open;
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
        self.world.paused = self.paused;
        let steps = self.world.due_steps(f64::from(dt));
        for _ in 0..steps {
            self.world.step();
            if let Some(a) = audio.as_mut() {
                a.sim_events(self.world.universe());
            }
            self.after_step(audio);
        }
        if let Some(a) = audio.as_mut() {
            let eye = self.world.camera.eye();
            let at = self.world.camera.look_at;
            a.game_update(
                eye.to_array(),
                at.to_array(),
                !self.paused,
                &self.world.run.universe,
            );
        }
        let now = self.world.universe().total_time_elapsed;
        self.subtitles.retain(|s| s.1 > now);
        if self.location.as_ref().is_some_and(|l| l.1 <= now) {
            self.location = None;
        }
        if let Some(End::Complete { at }) = self.end {
            if self.real_time - at > COMPLETE_TIME {
                out.push(Outcome::Leave);
            }
        }
        out
    }

    fn after_step(&mut self, audio: &mut Option<Audio>) {
        self.sync_resource_units();
        let u = &self.world.run.universe;
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
        for cue in self.cues.poll(&mut self.world.run) {
            match cue {
                Cue::Music(track) => {
                    self.music = Some(track);
                    if let Some(a) = audio.as_mut() {
                        a.play_music(track);
                    }
                }
                Cue::Speech { event, variable } => {
                    let spoken = audio
                        .as_mut()
                        .and_then(|a| a.mission_speech(event, variable));
                    if let Some(s) = spoken {
                        let secs = (s.duration_ms.max(0) as f32 / 1000.0).max(SUBTITLE_SHORTEST);
                        // The subtitle's end is what ends the intel event in the original.
                        self.world.run.game.speech_end = Some(now + secs);
                        let text = s.subtitles.join(" ");
                        if !text.trim().is_empty() {
                            self.subtitles = vec![(text, now + secs)];
                        }
                    }
                }
                Cue::LocationCard { milliseconds, text } => {
                    let secs = (milliseconds.max(1000) as f32 / 1000.0).min(30.0);
                    self.location = Some((text, now + secs));
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
        if self.sensors.phase() == Phase::Sensors {
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
            self.world.camera.tweaks = saved;
            for (dst, src) in self.canvas.pixels.iter_mut().zip(rgba?.as_chunks::<4>().0) {
                *dst = [src[0], src[1], src[2], 255];
            }
        }
        self.draw_marks();
        self.draw_text();
        self.ui.draw(&mut self.canvas);
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

    /// Draws the game's pointer (`mouseDraw`) at the mouse position.
    pub fn draw_cursor(&mut self) {
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

    /// Subtitles (`subtitle.script` region 1: 80,10 - 620,57 scaled to the screen, Arial_13,
    /// white with a drop shadow), the location card and the mission-complete note.
    fn draw_text(&mut self) {
        let (w, h) = (self.canvas.width, self.canvas.height);
        let Some(font) = self.subtitle_font.clone() else {
            return;
        };
        let lh = text::height(&font);
        let x0 = 80 * w / 640;
        let x1 = 620 * w / 640;
        let mut y = 10 * h / 480;
        for (s, _) in self.subtitles.clone() {
            for line in wrap(&font, &s, x1 - x0) {
                text::print(
                    &mut self.canvas,
                    &font,
                    x0,
                    y,
                    [255, 255, 255, 255],
                    &text::to_latin1(&line),
                    (Shadow::SE, [0, 0, 0, 255]),
                );
                y += lh;
            }
        }
        let big = self.big_font.clone().unwrap_or(font);
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
        if let Some((card, _)) = self.location.clone() {
            for (i, line) in card.split("\\n").enumerate() {
                centred(&mut self.canvas, line, h * 3 / 10 + i as i32 * (lh + 2));
            }
        }
        if let Some(End::Complete { .. }) = self.end {
            centred(&mut self.canvas, "MISSION COMPLETE", h / 2 - lh);
            centred(
                &mut self.canvas,
                "The next mission is not playable yet - returning to the main menu",
                h / 2 + 4,
            );
        }
    }
}

fn sound(audio: &mut Option<Audio>, e: u32) {
    if let Some(a) = audio.as_mut() {
        a.event(e);
    }
}

/// Words of `s` in lines no wider than `width`.
fn wrap(font: &hwc_formats::font::Font, s: &str, width: i32) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        for word in para.split_whitespace() {
            let cand = if line.is_empty() {
                word.to_string()
            } else {
                format!("{line} {word}")
            };
            if text::width(font, &text::to_latin1(&cand)) > width && !line.is_empty() {
                out.push(std::mem::replace(&mut line, word.to_string()));
            } else {
                line = cand;
            }
        }
        if !line.is_empty() {
            out.push(line);
        }
    }
    out
}
