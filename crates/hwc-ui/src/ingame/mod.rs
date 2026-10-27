//! The in-game user interface: construction, launch, research and trader managers (front-end
//! screens from `FeMan\*.fib`), the main overlay, the ship info panel, the right-click menu, the
//! in-game escape menu with its options screens, the objectives screen and the configurable
//! keys. It reads the game through [`GameUiHost`] and answers with [`GameRequest`]s.
//!
//! The application calls, every frame: [`InGameUi::handle_input`] for each input event (it
//! says whether the UI took it), [`InGameUi::update`] (returns the requests to apply), and
//! [`InGameUi::draw`] on a canvas that already holds the 3D view.

pub mod consmgr;
pub mod cursor;
pub mod draw;
pub mod escmenu;
pub mod host;
pub mod icons;
pub mod keylist;
pub mod keys;
pub mod launchmgr;
pub mod mock;
pub mod modern_build;
pub mod modern_build_manager;
pub mod modern_hud;
pub mod modern_launch_manager;
pub mod modern_log;
pub mod modern_manager;
pub mod modern_menu;
pub mod modern_objectives;
pub mod modern_options;
pub mod modern_panel;
pub mod modern_proxy;
pub mod modern_research;
pub mod modern_research_manager;
pub mod modern_selected;
pub mod overlay;
pub mod popup;
pub mod research;
pub mod rollcall;
pub mod shipprint;
pub mod strings;
pub mod trader;

use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use crate::assets::{AssetSource, Language, Resources};
use crate::canvas::{Canvas, Image, Rect};
use crate::cataclysm::{self, Cataclysm, Request};
use crate::consts::key;
use crate::fe::FrontEnd;
use crate::options::Options;

pub use host::{GameRequest, GameUiHost, HostView};
use keys::{Action, KeyBindings};
use strings::Strings;

pub type Fe = FrontEnd<Cataclysm>;

/// The `.fib` files of the in-game screens.
pub const FIBS: &[&str] = &[
    "construction_manager",
    "beast_construction_manager",
    "launch_manager",
    "beast_launch_manager",
    "research_manager",
    "beast_research_manager",
    "in_game_esc_menu",
    "single_player_objective",
    "trader_interface",
    "hyperspace_roll_call",
];

/// The full-screen screen in front of the game, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Construction,
    Launch,
    Research,
    Trader,
    EscMenu,
    Objectives,
    /// The single-player hyperspace roll call.
    RollCall,
}

/// In-game state the screen callbacks work on (kept in [`Cataclysm::ig`]).
#[derive(Default)]
pub struct IgState {
    /// What the host showed this frame.
    pub view: HostView,
    /// Requests for the game, in order.
    pub requests: Vec<GameRequest>,
    pub strings: Strings,
    pub keys: KeyBindings,
    pub default_keys: KeyBindings,
    /// Bindings as saved in `keys.cfg` (Cancel and "reset" go back to them).
    pub saved_keys: KeyBindings,
    /// Where `keys.cfg` is written.
    pub keys_path: Option<PathBuf>,
    /// Action waiting for a key in the rebinding list.
    pub binding: Option<Action>,
    pub manager: Option<Manager>,
    pub cm: consmgr::State,
    /// The modern build manager's own state (`--build-ui`).
    pub mbm: modern_build_manager::State,
    /// The modern research manager's own state (`--research-ui`).
    pub mrm: modern_research_manager::State,
    pub lm: launchmgr::State,
    /// The modern menus' own state (`--menus`).
    pub mmenu: modern_menu::State,
    /// The modern systems manager's own state (`--menus`).
    pub mlm: modern_launch_manager::State,
    /// Modern options the command line set for this run: (options key, value). The modern
    /// options screen shows them and does not change them.
    pub locked_options: Vec<(String, i32)>,
    pub rm: research::State,
    pub tm: trader::State,
    pub esc: escmenu::State,
    pub overlay: overlay::State,
    pub ship_print: shipprint::State,
    pub popup: popup::State,
    /// The game was paused by the UI (escape menu, objectives) and must resume on close.
    pub paused_by_ui: bool,
    /// A roll-call button closed the roll call screen (see [`rollcall`]).
    pub rollcall_dismissed: bool,
    /// The roll call opened with "quick docking" disabled (else "emergency hyperspace").
    pub rollcall_go_now_disabled: bool,
}

impl IgState {
    pub fn request(&mut self, r: GameRequest) {
        self.requests.push(r);
    }
}

/// Input events from the application, in canvas pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    MouseMove { x: i32, y: i32 },
    MouseButton { left: bool, down: bool },
    Wheel { up: bool },
    KeyDown(u16),
    KeyUp(u16),
    Char(char),
}

/// Setup of [`InGameUi::new`].
#[derive(Debug, Default, Clone)]
pub struct Setup {
    pub options: Options,
    /// `default_keys.cfg` text.
    pub default_keys: Option<String>,
    /// `keys.cfg` text (the player's bindings); the defaults when absent or older.
    pub keys: Option<String>,
    /// Where accepted options write `keys.cfg`.
    pub keys_path: Option<PathBuf>,
    /// Where accepted options are written (the front end's options file).
    pub options_path: Option<PathBuf>,
}

pub struct InGameUi {
    pub fe: Fe,
    held: HashSet<u16>,
    captured: bool,
    /// The letterbox is fully in: the main overlay is not drawn (`moDraw`).
    pub letterboxed: bool,
    /// A fleet intel event holds the keyboard (`smFleetIntel`): bound actions do nothing
    /// except Escape and Skip_Sensors, which skip the event.
    pub fleet_intel: bool,
    /// A left / right press over the modern HUD's panels while a full-screen manager was
    /// open: the manager saw neither it nor its release.
    panel_press: [bool; 2],
    /// The pointer as the application gave it (a modern screen may have put the screens'
    /// own pointer elsewhere, on the original screen underneath).
    mouse: (i32, i32),
}

/// The managers that cover the screen: the overlay is not drawn under them (the modern HUD's
/// panels are drawn over them).
pub fn full_screen_manager(m: Option<Manager>) -> bool {
    matches!(
        m,
        Some(Manager::Construction | Manager::Launch | Manager::Research | Manager::Trader)
    )
}

fn read_text(source: &dyn AssetSource, name: &str) -> Option<String> {
    source
        .read(name)
        .map(|d| String::from_utf8_lossy(&d).into_owned())
}

/// The `v <n>` version line of a keys file.
fn keys_version(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("v ")?.trim().parse().ok())
}

impl InGameUi {
    pub fn new(source: Rc<dyn AssetSource>, setup: Setup) -> Result<InGameUi, String> {
        let mut res = Resources::new(source.clone());
        res.set_language(Language::from_index(setup.options.get("language") as u32));
        let lang = format!("{:?}.script", res.language()).to_ascii_lowercase();
        let res = Rc::new(res);
        let mut game = Cataclysm::new(setup.options);
        game.load_strings(
            read_text(&*source, &lang).as_deref(),
            read_text(&*source, "tweak.script").as_deref(),
        );
        let mut strings = read_text(&*source, &lang)
            .map(|t| Strings::parse(&t))
            .unwrap_or_default();
        if let Some(en) = read_text(&*source, "english.script") {
            strings.fill_from(&Strings::parse(&en));
        }
        game.ig.strings = strings;
        if let Some(k) = &setup.default_keys {
            game.load_key_bindings(k);
        }
        // `kbLoadBindings`: keys.cfg unless missing or older than the defaults.
        if let Some(k) = &setup.keys {
            let default_v = setup.default_keys.as_deref().and_then(keys_version);
            if keys_version(k) >= default_v {
                game.ig.keys = KeyBindings::parse(k);
                game.ig.saved_keys = game.ig.keys.clone();
            }
        }
        game.ig.keys_path = setup.keys_path.clone();
        game.config_path = setup.options_path.clone();
        // The default colours as the front end has them, so the colours saved with the
        // options are the same.
        if let Some(t) = read_text(&*source, "teamcolors.script") {
            game.colors.load_defaults(&t);
        }
        if let Some(t) = read_text(&*source, "mainoverlay.script") {
            game.ig.overlay.load_script(&t);
        }
        if let Some(t) = read_text(&*source, "OverlayIcons\\overlayicons.script") {
            game.ig.overlay.load_icons(&t);
        }
        let mut fe = FrontEnd::new(game, res);
        cataclysm::register(&mut fe);
        register(&mut fe);
        for f in FIBS {
            let name = format!("FEMan\\{f}.fib");
            let data = source
                .read(&name)
                .ok_or_else(|| format!("{name} not found in the game data"))?;
            fe.load_fib_data(&data)
                .map_err(|e| format!("{name}: {e}"))?;
        }
        // The save screen's overwrite question lives in the front end's file.
        if let Some(data) = source.read(r"FEMan\front_end.fib") {
            if let Err(e) = fe.load_fib_screens(&data, &[escmenu::OVERWRITE_POPUP]) {
                fe.log_once(format!("front_end.fib: {e}"));
            }
        }
        Ok(InGameUi {
            fe,
            held: HashSet::new(),
            captured: false,
            letterboxed: false,
            fleet_intel: false,
            panel_press: [false; 2],
            mouse: (0, 0),
        })
    }

    pub fn state(&self) -> &IgState {
        &self.fe.game.ig
    }

    pub fn state_mut(&mut self) -> &mut IgState {
        &mut self.fe.game.ig
    }

    /// Places the 640x480 layouts on a screen of this size (`feResRepositionX/Y`).
    pub fn resize(&mut self, width: i32, height: i32) {
        self.fe.set_screen_size(width, height);
    }

    /// What the subtitle pass drew this frame (`moSetTop`): the top-left and top-right panels
    /// move below a speaker portrait.
    pub fn subtitle_top(&mut self, hud: crate::subtitle::HudTop) {
        self.fe.game.ig.overlay.set_top(hud);
    }

    /// Opt-in layout change: the roll call (which the original keeps at its 640x480 position in
    /// the top-left corner at every resolution, under the speech band and portrait) moves
    /// down by `dy` pixels. Takes effect the next time the screen opens.
    /// A modern option the command line set for this run: the modern options screen shows
    /// `value` and does not change it.
    pub fn lock_option(&mut self, key: &str, value: i32) {
        let locked = &mut self.fe.game.ig.locked_options;
        locked.retain(|(k, _)| k != key);
        locked.push((key.to_string(), value));
    }

    /// The monitor the window is on and the display in use (the modern options' screen mode
    /// and resolution follow them).
    pub fn set_display(
        &mut self,
        monitor: Option<crate::display::Monitor>,
        in_use: crate::display::Setting,
    ) {
        self.fe.game.monitor = monitor;
        self.fe.game.display = in_use;
    }

    pub fn set_rollcall_offset(&mut self, dy: i32) {
        self.fe.set_screen_offset(rollcall::SCREEN, 0, dy);
    }

    /// Refreshes the copy of the host's state the screens show.
    pub fn refresh(&mut self, host: &dyn GameUiHost) {
        let factory = self.fe.game.ig.cm.factory;
        self.fe.game.ig.view = HostView::capture(host, factory);
        self.captured = true;
    }

    pub fn manager(&self) -> Option<Manager> {
        self.fe.game.ig.manager
    }

    /// Opens a manager screen (what the keys, taskbar and menu do).
    pub fn open(&mut self, host: &dyn GameUiHost, m: Manager) {
        self.refresh(host);
        if m == Manager::Trader {
            trader::open(&mut self.fe);
        } else {
            open_manager(&mut self.fe, m);
        }
    }

    pub fn close(&mut self) {
        close_manager(&mut self.fe);
    }

    /// The single-player mission was lost: the escape menu shows "GAME OVER" from now on.
    pub fn game_over(&mut self, host: &dyn GameUiHost) {
        self.fe.game.ig.esc.game_over = true;
        self.open(host, Manager::EscMenu);
    }

    /// Opens the right-click menu at the mouse (the game decides when a right click has no
    /// order to give: `mrRightClickSmartOrder`), or the special-actions menu.
    pub fn open_popup(&mut self, host: &dyn GameUiHost, special: bool) {
        self.refresh(host);
        let (x, y) = self.fe.mouse_pos();
        let kind = if special {
            popup::MenuKind::Special
        } else {
            popup::MenuKind::Main
        };
        popup::open(&mut self.fe, kind, x, y);
    }

    /// The picture of the build manager's ship view, rendered by the application for the
    /// type [`InGameUi::ship_view`] asks for.
    pub fn set_ship_view_image(&mut self, img: Option<Image>) {
        self.fe.game.ig.cm.ship_view_image = img.map(Rc::new);
    }

    /// One input event; returns `true` when the UI used it (the game must not).
    pub fn handle_input(&mut self, host: &dyn GameUiHost, ev: InputEvent) -> bool {
        // Keys and clicks act on the game as it is now.
        if !self.captured || !matches!(ev, InputEvent::MouseMove { .. }) {
            self.refresh(host);
        }
        match ev {
            InputEvent::MouseMove { x, y } => {
                self.mouse = (x, y);
                self.fe.mouse_move(x, y);
                if modern_menu::active(&self.fe) {
                    modern_menu::mouse_move(&mut self.fe, x, y);
                }
                if modern_launch_manager::active(&self.fe) {
                    modern_launch_manager::mouse_move(&mut self.fe, x, y);
                }
                if modern_build_manager::active(&self.fe) {
                    modern_build_manager::mouse_move(&mut self.fe, x, y);
                }
                if modern_research_manager::active(&self.fe) {
                    modern_research_manager::mouse_move(&mut self.fe, x, y);
                }
                popup::mouse_move(&mut self.fe, x, y);
                shipprint::mouse_move(&mut self.fe, x, y);
                self.fe.game.ig.overlay.set_mouse(x, y);
                self.fe.game.ig.manager.is_some()
            }
            InputEvent::MouseButton { left, down } => {
                if self.fe.game.ig.manager.is_some() {
                    if self.panel_click(left, down) {
                        return true;
                    }
                    if modern_build_manager::active(&self.fe) {
                        modern_build_manager::mouse_button(&mut self.fe, left, down);
                        return true;
                    }
                    if modern_research_manager::active(&self.fe) {
                        modern_research_manager::mouse_button(&mut self.fe, left, down);
                        return true;
                    }
                    if modern_menu::active(&self.fe) {
                        modern_menu::mouse_button(&mut self.fe, left, down);
                        return true;
                    }
                    if modern_launch_manager::active(&self.fe) {
                        modern_launch_manager::mouse_button(&mut self.fe, left, down);
                        return true;
                    }
                    self.fe.mouse_button(left, down);
                    return true;
                }
                // The button state is global in the original: a release after a manager has
                // closed still lets go of the button the screens saw pressed.
                if !down {
                    self.fe.mouse_button(left, false);
                }
                let (x, y) = self.fe.mouse_pos();
                popup::mouse_button(&mut self.fe, left, down, x, y)
                    || shipprint::mouse_button(&mut self.fe, left, down, x, y)
                    || overlay::mouse_button(&mut self.fe, left, down, x, y)
            }
            InputEvent::Wheel { up } => {
                if modern_build_manager::active(&self.fe) {
                    modern_build_manager::wheel(&mut self.fe, up);
                    return true;
                }
                if modern_research_manager::active(&self.fe) {
                    modern_research_manager::wheel(&mut self.fe, up);
                    return true;
                }
                if modern_menu::active(&self.fe) {
                    modern_menu::wheel(&mut self.fe, up);
                    return true;
                }
                if modern_launch_manager::active(&self.fe) {
                    modern_launch_manager::wheel(&mut self.fe, up);
                    return true;
                }
                if self.fe.game.ig.manager.is_some() {
                    self.fe.wheel(up);
                    return true;
                }
                false
            }
            InputEvent::KeyDown(code) => {
                let repeat = !self.held.insert(code);
                if self.fe.game.ig.manager.is_some() {
                    if modern_menu::active(&self.fe) && modern_menu::key_down(&mut self.fe, code) {
                        return true;
                    }
                    self.fe.key_down(code);
                    return true;
                }
                if repeat {
                    return !self.fe.game.ig.keys.actions_with_key(code).is_empty();
                }
                self.key_action_down(code)
            }
            InputEvent::KeyUp(code) => {
                self.held.remove(&code);
                // The key state is global in the original (`keyScanCode`): a key let go after
                // its press closed a manager (Escape) is up for the screens too, or the next
                // manager would not see its next press.
                self.fe.key_up(code);
                if self.fe.game.ig.manager.is_some() {
                    return true;
                }
                self.key_action_up(code)
            }
            InputEvent::Char(c) => {
                if self.fe.game.ig.manager.is_some() {
                    if self.fe.game.ig.mmenu.page.is_some() && modern_menu::active(&self.fe) {
                        return true;
                    }
                    self.fe.char_typed(c);
                    return true;
                }
                false
            }
        }
    }

    /// A button over the modern HUD's panels while a full-screen manager is open: the panels
    /// are drawn over the manager, so they take the click (a header folds its panel, an
    /// objective line acts as usual) and the manager does not see it, nor the release of a
    /// press they took. Whether it was used.
    fn panel_click(&mut self, left: bool, down: bool) -> bool {
        let slot = usize::from(!left);
        if !down {
            let used = std::mem::take(&mut self.panel_press[slot]);
            if used && left {
                // The objectives panel's hyperspace button acts on the release.
                let (x, y) = self.mouse;
                overlay::modern_release(&mut self.fe, x, y);
            }
            return used;
        }
        if !full_screen_manager(self.fe.game.ig.manager) || self.fe.game.ig.overlay.cutscene {
            return false;
        }
        let (x, y) = self.mouse;
        overlay::layout(&mut self.fe);
        if !overlay::over_modern_panel(&self.fe, x, y) {
            return false;
        }
        self.panel_press[slot] = true;
        if left {
            overlay::modern_click(&mut self.fe, x, y);
        }
        true
    }

    /// Shift/Alt/Ctrl held now, as binding modifier flags.
    /// The modifier keys held (`keys::MOD_*`).
    pub fn modifiers_held(&self) -> u8 {
        self.modifiers()
    }

    fn modifiers(&self) -> u8 {
        let mut m = 0;
        if self.held.contains(&key::SHIFT) {
            m |= keys::MOD_SHIFT;
        }
        if self.held.contains(&key::ALT) {
            m |= keys::MOD_ALT;
        }
        if self.held.contains(&key::CONTROL) {
            m |= keys::MOD_CTRL;
        }
        m
    }

    /// `kbProcessKeyDown`: overlay option digits while the options key is held, then the
    /// popup menu's keys, then every action bound to the key with the held modifiers.
    fn key_action_down(&mut self, code: u16) -> bool {
        let mods = self.modifiers();
        if self.action_held(keys::TOGGLE_OVERLAY_OPTIONS) && (0x30..=0x39).contains(&code) {
            let digit = (code - 0x30) as u8;
            if shipprint::visible(&self.fe) {
                shipprint::toggle_option(&mut self.fe, digit);
            } else {
                overlay::toggle_option(&mut self.fe, digit);
            }
            return true;
        }
        if popup::process_key(&mut self.fe, code) {
            return true;
        }
        if let Some(a) = self.tab_ship_icons(code, mods) {
            match a {
                Some(a) => execute_action(&mut self.fe, a, true),
                None => self.fe.game.ig.request(GameRequest::CycleShipIcons),
            }
            return true;
        }
        let actions = self.fe.game.ig.keys.actions_for(code, mods);
        if self.fleet_intel {
            if actions
                .iter()
                .any(|&a| a == keys::ESCAPE || a == keys::SKIP_SENSORS)
            {
                self.fe.game.ig.request(GameRequest::SkipFleetIntel);
            }
            return !actions.is_empty();
        }
        for a in &actions {
            if !a.is_polled() {
                execute_action(&mut self.fe, *a, true);
            }
        }
        !actions.is_empty()
    }

    /// A user-requested change (`hwcTabShipIcons`): Tab cycles the ship icons (`None`) and
    /// Shift+Tab does what Tab did (Toggle_Ship_Info), while Tab is bound to nothing else and
    /// Shift+Tab to nothing at all.
    fn tab_ship_icons(&self, code: u16, mods: u8) -> Option<Option<Action>> {
        if code != key::TAB || self.fleet_intel || self.fe.game.options.get("hwcTabShipIcons") == 0
        {
            return None;
        }
        let kb = &self.fe.game.ig.keys;
        let plain = kb.actions_for(key::TAB, 0);
        if plain.iter().any(|&a| a != keys::TOGGLE_SHIP_INFO) {
            return None;
        }
        match mods {
            0 => Some(None),
            keys::MOD_SHIFT if kb.actions_for(key::TAB, keys::MOD_SHIFT).is_empty() => {
                Some(plain.first().copied())
            }
            _ => None,
        }
    }

    /// `kbProcessKeyUp`: the up handlers of actions using the key.
    fn key_action_up(&mut self, code: u16) -> bool {
        let actions = self.fe.game.ig.keys.actions_with_key(code);
        let mut used = false;
        for a in actions.into_iter().filter(|a| a.has_up()) {
            execute_action(&mut self.fe, a, false);
            used = true;
        }
        used
    }

    /// Whether a polled action (pan, zoom, band-box modifiers, overlay options, key pages)
    /// is held now.
    pub fn action_held(&self, a: Action) -> bool {
        self.fe
            .game
            .ig
            .keys
            .is_held(a, &self.held, self.modifiers())
    }

    /// One frame: refreshes the host copy, runs the screens, returns the requests.
    pub fn update(&mut self, host: &dyn GameUiHost, dt: f32) -> Vec<GameRequest> {
        self.refresh(host);
        self.fe.game.ig.overlay.cutscene = self.letterboxed || self.fleet_intel;
        // Polled keys of the overlay.
        let held = self.action_held(keys::TOGGLE_OVERLAY_OPTIONS);
        self.fe.game.ig.overlay.options_held = held;
        let page = [keys::SHOW_KEYS_1, keys::SHOW_KEYS_2, keys::SHOW_KEYS_3]
            .iter()
            .position(|&a| self.action_held(a))
            .map(|i| i as u8 + 1);
        self.fe.game.ig.overlay.keys_page = page;
        rollcall::tick(&mut self.fe);
        if self.fe.game.ig.manager.is_some() {
            self.fe.process(dt);
            cataclysm::tick(&mut self.fe, dt);
            consmgr::tick(&mut self.fe, dt);
            modern_build_manager::tick(&mut self.fe);
            launchmgr::tick(&mut self.fe);
            research::tick(&mut self.fe);
            modern_research_manager::tick(&mut self.fe);
            modern_menu::tick(&mut self.fe);
        } else {
            self.fe.time += f64::from(dt);
        }
        overlay::tick(&mut self.fe, dt);
        // A screen that closed itself (a callback deleted the last screen).
        if self.fe.game.ig.manager.is_some() && self.fe.stack.len() <= 1 {
            close_manager(&mut self.fe);
        }
        // No modern screen hands the pointer on any more: the screens' pointer is the
        // application's again.
        if !self.proxied() && self.fe.mouse_pos() != self.mouse {
            self.fe.mouse_move(self.mouse.0, self.mouse.1);
        }
        self.take_requests()
    }

    /// A modern screen hands the pointer on to the original screen underneath.
    fn proxied(&self) -> bool {
        modern_menu::active(&self.fe) || modern_launch_manager::active(&self.fe)
    }

    /// Requests queued so far (also returned by [`InGameUi::update`]).
    pub fn take_requests(&mut self) -> Vec<GameRequest> {
        let mut out = std::mem::take(&mut self.fe.game.ig.requests);
        out.extend(self.fe.sounds.drain(..).map(GameRequest::Sound));
        for r in std::mem::take(&mut self.fe.game.requests) {
            match r {
                Request::SoundEvent(e) => out.push(GameRequest::Sound(e)),
                Request::Speech(e) => out.push(GameRequest::Speech {
                    event: e,
                    param: None,
                }),
                Request::AudioSettings => out.push(GameRequest::AudioSettings),
                Request::ApplyOptions => out.push(GameRequest::ApplyOptions),
                Request::Quit => out.push(GameRequest::QuitToMain),
                _ => {}
            }
        }
        out
    }

    /// Draws the UI over the 3D view already in `canvas` (same size as the screen given to
    /// [`InGameUi::resize`]).
    pub fn draw(&mut self, canvas: &mut Canvas) {
        // The modern HUD's panels hide in cutscenes: the letterbox, an NIS, fleet intel.
        self.fe.game.ig.overlay.cutscene = self.letterboxed || self.fleet_intel;
        let full_screen = full_screen_manager(self.fe.game.ig.manager);
        if !full_screen {
            if !self.letterboxed {
                overlay::draw(&mut self.fe, canvas);
            }
            shipprint::draw(&mut self.fe, canvas);
            popup::draw(&mut self.fe, canvas);
        }
        if modern_build_manager::active(&self.fe) {
            modern_build_manager::draw(&mut self.fe, canvas);
        } else if modern_research_manager::active(&self.fe) {
            modern_research_manager::draw(&mut self.fe, canvas);
        } else if modern_menu::active(&self.fe) {
            modern_menu::draw(&mut self.fe, canvas);
        } else if modern_launch_manager::active(&self.fe) {
            modern_launch_manager::draw(&mut self.fe, canvas);
        } else if self.fe.game.ig.manager.is_some() {
            self.fe.render_over(canvas);
        }
        if full_screen {
            overlay::draw_modern_panels(&mut self.fe, canvas);
        }
    }

    /// Where the 3D ship picture of the build manager goes, and which ship type it shows
    /// (`SV_ShipView`): the application renders it and hands it back with
    /// [`InGameUi::set_ship_view_image`].
    pub fn ship_view(&self) -> Option<(Rect, host::ShipType)> {
        if modern_build_manager::active(&self.fe) {
            return modern_build_manager::ship_view(&self.fe);
        }
        consmgr::ship_view(&self.fe)
    }
}

/// Every in-game callback.
pub fn register(fe: &mut Fe) {
    consmgr::register(fe);
    launchmgr::register(fe);
    research::register(fe);
    trader::register(fe);
    escmenu::register(fe);
    rollcall::register(fe);
}

/// The screen of a manager for the player's race.
fn screen_name(fe: &Fe, m: Manager) -> &'static str {
    let beast = fe.game.ig.view.player.race == host::Race::Beast;
    match (m, beast) {
        (Manager::Construction, false) => "Construction_manager",
        (Manager::Construction, true) => "Beast_Construction_manager",
        (Manager::Launch, false) => "Launch_Manager",
        (Manager::Launch, true) => "Beast_Launch_Manager",
        (Manager::Research, false) => "Research_Manager",
        (Manager::Research, true) => "Beast_Research_Manager",
        (Manager::Trader, _) => "Trader_Interface",
        (Manager::EscMenu, _) => escmenu::screen_for(fe),
        (Manager::Objectives, _) => "Single_Player_Objective",
        (Manager::RollCall, _) => rollcall::SCREEN,
    }
}

/// Opens a manager, replacing one already open (`mrBuildShips`, `mrResearch`, `mrLaunch`...).
pub fn open_manager(fe: &mut Fe, m: Manager) {
    if fe.game.ig.manager.is_some() {
        fe.all_screens_delete();
    }
    popup::close(fe);
    // The managers leave the game running; the escape menu (except in multiplayer) and fleet
    // intelligence pause it.
    let pause = match m {
        Manager::EscMenu => !fe.game.ig.view.player.multiplayer,
        Manager::Objectives => true,
        _ => false,
    };
    if pause && !fe.game.ig.paused_by_ui {
        fe.game.ig.paused_by_ui = true;
        fe.game.ig.request(GameRequest::SetPaused(true));
    }
    fe.game.ig.manager = Some(m);
    match m {
        Manager::Construction => {
            consmgr::begin(fe);
            modern_build_manager::reset(fe);
        }
        Manager::Launch => {
            launchmgr::begin(fe);
            modern_launch_manager::reset(fe);
        }
        Manager::Research => {
            research::begin(fe);
            modern_research_manager::reset(fe);
        }
        Manager::EscMenu => {
            fe.game.ig.request(GameRequest::Sound(0x108));
            modern_menu::reset(fe);
        }
        Manager::Objectives => modern_menu::reset(fe),
        _ => {}
    }
    let name = screen_name(fe, m);
    fe.screen_start(name);
    match m {
        Manager::Construction => consmgr::started(fe),
        Manager::Launch => launchmgr::started(fe),
        Manager::Research => research::started(fe),
        Manager::RollCall => rollcall::started(fe),
        _ => {}
    }
}

/// Closes whatever manager is open and resumes the game if the UI paused it.
pub fn close_manager(fe: &mut Fe) {
    fe.all_screens_delete();
    fe.game.ig.manager = None;
    fe.mouse_clip(None);
    if fe.game.ig.paused_by_ui {
        fe.game.ig.paused_by_ui = false;
        fe.game.ig.request(GameRequest::SetPaused(false));
    }
}

/// Runs a key action (`kbExecuteAction`): UI actions here, ship commands and the rest go to
/// the game.
pub fn execute_action(fe: &mut Fe, a: Action, down: bool) {
    use keys::*;
    if down {
        match a {
            OPEN_BUILD_MANAGER => return open_manager(fe, Manager::Construction),
            OPEN_RESEARCH_MANAGER => return open_manager(fe, Manager::Research),
            OPEN_LAUNCH_MANAGER => return open_manager(fe, Manager::Launch),
            ESCAPE => {
                // `kbcbEscape`: the menu first, then whatever the game has to cancel, then
                // the escape menu.
                if popup::is_open(fe) {
                    return popup::close(fe);
                }
                if !fe.game.ig.view.escape_pending {
                    return open_manager(fe, Manager::EscMenu);
                }
            }
            OPEN_MENU => return popup::open_at_mouse(fe),
            TOGGLE_SHIP_INFO => return shipprint::toggle(fe),
            NEXT_OVERLAY_VIEW => return overlay::next_view(fe),
            CHANGE_OVERLAY_BACKDROPS => return overlay::toggle_backdrops(fe),
            TOGGLE_TACTICAL_DISPLAY => overlay::toggle_tactical(fe),
            PAGE_UP_GAME_CHAT => {
                fe.game.ig.overlay.message_scroll += 1;
                return;
            }
            PAGE_DOWN_GAME_CHAT => {
                let s = &mut fe.game.ig.overlay.message_scroll;
                *s = s.saturating_sub(1);
                return;
            }
            _ => {}
        }
    } else if a == MULTIPLE_SELECT {
        overlay::apply_deselect(fe);
    } else if a == SHIP_SPECIAL && !fe.game.ig.view.selection.is_empty() && !popup::is_open(fe) {
        // Ship_Special released without being used as a modifier: the special-actions menu.
        let (x, y) = fe.mouse_pos();
        popup::open(fe, popup::MenuKind::Special, x, y);
    }
    if let Some(c) = command_of(a) {
        if down {
            fe.game.ig.request(GameRequest::Command(c));
        }
        return;
    }
    fe.game.ig.request(GameRequest::Action { action: a, down });
}

/// The ship command a key action gives the selection.
pub fn command_of(a: Action) -> Option<host::Command> {
    use host::{Command as C, Formation as F};
    use keys::*;
    Some(match a {
        DOCK => C::Dock,
        RETIRE => C::Retire,
        CANCEL_ORDERS => C::Cancel,
        SCUTTLE => C::Scuttle,
        KAMIKAZE => C::Kamikaze,
        HYPERSPACE => C::Hyperspace,
        UPGRADE => C::Upgrade,
        ENTER_SLIPGATE => C::EnterSlipgate,
        HARVEST => C::Harvest,
        REPAIR => C::Repair,
        SALVAGE => C::Salvage,
        LEECH => C::Leech,
        LINK => C::Link,
        UNLINK => C::Unlink,
        CLOAK => C::Cloak,
        LAUNCH_SWARMERS => C::Swarmers,
        BIG_GUN_ATTACK => C::BigGun,
        BEAST_ATTACK => C::BeastAttack,
        EMP => C::Emp,
        RAM => C::Ram,
        REPULSE => C::Repulse,
        FIRE_MISSILES => C::FireMissiles,
        TACTICS_EVASIVE => C::TacticsEvasive,
        TACTICS_NEUTRAL => C::TacticsNeutral,
        TACTICS_AGGRESSIVE => C::TacticsAggressive,
        FORMATION_DELTA => C::Formation(F::Delta),
        FORMATION_BROAD => C::Formation(F::Broad),
        FORMATION_DELTA3D => C::Formation(F::Delta3D),
        FORMATION_CLAW => C::Formation(F::Claw),
        FORMATION_WALL => C::Formation(F::Wall),
        FORMATION_SPHERE => C::Formation(F::Sphere),
        FORMATION_CUSTOM => C::Formation(F::Custom),
        FORMATION_PARADE => C::Formation(F::Parade),
        _ => return None,
    })
}
