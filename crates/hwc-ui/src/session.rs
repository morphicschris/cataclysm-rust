//! A ready-to-run front end: loads the screens and strings, registers the game's callbacks,
//! places the 640x480 layout in a window, routes input, runs the credits and handles what the
//! menus ask for. Used by the `cataclysm` executable, the screenshot tour and the tests.

use std::path::PathBuf;
use std::rc::Rc;

use crate::assets::{AssetSource, Language, Resources};
use crate::canvas::Canvas;
use crate::cataclysm::{self, Cataclysm, Request};
use crate::fe::FrontEnd;
use crate::options::Options;
use crate::scale::{Placement, ScaleMode};

/// The `.fib` file with the main menu and options screens.
pub const FRONT_END_FIB: &str = "FEMan\\Front_End.fib";
/// The first screen.
pub const MAIN_SCREEN: &str = "Main_game_screen";

/// Inputs to [`Session::new`] besides the game files.
#[derive(Debug, Default, Clone)]
pub struct Setup {
    /// Our options file (loaded if present, written when options are accepted).
    pub options_path: Option<PathBuf>,
    /// Text of the original `Cataclysm.cfg`, imported when there is no options file yet.
    pub original_cfg: Option<String>,
    /// Text of `default_keys.cfg` for the key binding lists.
    pub default_keys: Option<String>,
    pub scale: ScaleMode,
}

pub struct Session {
    pub fe: FrontEnd<Cataclysm>,
    pub canvas: Canvas,
    pub placement: Placement,
    pub scale: ScaleMode,
    window: (u32, u32),
    /// Requests the application has to act on (quit, apply video options, ...).
    pub pending: Vec<Request>,
    skip_credits: bool,
}

impl Session {
    pub fn new(source: Rc<dyn AssetSource>, setup: Setup) -> Result<Session, String> {
        let fib_data = source
            .read(FRONT_END_FIB)
            .ok_or_else(|| format!("{FRONT_END_FIB} not found in the game data"))?;
        let mut options = match &setup.options_path {
            Some(p) if p.is_file() => {
                Options::load(p).map_err(|e| format!("{}: {e}", p.display()))?
            }
            _ => {
                let mut o = Options::default();
                if let Some(cfg) = &setup.original_cfg {
                    o.import_original_cfg(cfg);
                }
                o
            }
        };
        // The front end offers English, French and German.
        if !(0..=2).contains(&options.get("language")) {
            options.set("language", 0);
        }
        let mut res = Resources::new(source.clone());
        res.set_language(Language::from_index(options.get("language") as u32));
        let res = Rc::new(res);
        let mut game = Cataclysm::new(options);
        game.config_path = setup.options_path.clone();
        let text = |name: &str| {
            source
                .read(name)
                .map(|d| String::from_utf8_lossy(&d).into_owned())
        };
        let lang_script = format!("{:?}.script", res.language()).to_ascii_lowercase();
        game.load_strings(
            text(&lang_script).as_deref(),
            text("tweak.script").as_deref(),
        );
        if let Some(t) = text("teamcolors.script") {
            game.colors.load_defaults(&t);
        }
        if let Some(k) = &setup.default_keys {
            game.load_key_bindings(k);
        }
        let mut fe = FrontEnd::new(game, res);
        cataclysm::register(&mut fe);
        fe.load_fib_data(&fib_data)
            .map_err(|e| format!("{FRONT_END_FIB}: {e}"))?;
        let placement = Placement::new(setup.scale, (640, 480));
        let mut s = Session {
            fe,
            canvas: Canvas::new(placement.canvas.0, placement.canvas.1),
            placement,
            scale: setup.scale,
            window: (640, 480),
            pending: Vec::new(),
            skip_credits: false,
        };
        s.fe.screen_start(MAIN_SCREEN);
        Ok(s)
    }

    /// The window was resized (or created).
    pub fn resize(&mut self, width: u32, height: u32) {
        self.window = (width.max(1), height.max(1));
        self.placement = Placement::new(self.scale, self.window);
        let (cw, ch) = self.placement.canvas;
        self.canvas.resize(cw, ch);
        self.fe.set_screen_size(cw, ch);
    }

    pub fn set_scale(&mut self, scale: ScaleMode) {
        self.scale = scale;
        let (w, h) = self.window;
        self.resize(w, h);
    }

    pub fn mouse_move(&mut self, x: f64, y: f64) {
        let (cx, cy) = self.placement.to_canvas(x, y);
        self.fe.mouse_move(cx, cy);
    }

    pub fn mouse_button(&mut self, left: bool, down: bool) {
        if down && self.fe.game.credits.is_some() {
            self.skip_credits = true;
        }
        self.fe.mouse_button(left, down);
    }

    pub fn key_down(&mut self, code: u16) {
        if code == crate::consts::key::ESCAPE && self.fe.game.credits.is_some() {
            self.skip_credits = true;
            return;
        }
        self.fe.key_down(code);
    }

    pub fn key_up(&mut self, code: u16) {
        self.fe.key_up(code);
    }

    /// One frame of front-end processing; returns the requests the application must handle.
    pub fn update(&mut self, dt: f32) -> Vec<Request> {
        let res = self.fe.resources().clone();
        if let Some(c) = &mut self.fe.game.credits {
            c.update(&res, dt, std::mem::take(&mut self.skip_credits));
            if c.finished {
                self.fe.game.credits = None;
                // The credits run on top of the menu, which stays as it was.
                self.fe.input.stuck.clear();
            }
            return std::mem::take(&mut self.pending);
        }
        self.fe.process(dt);
        let mut out = std::mem::take(&mut self.pending);
        for r in std::mem::take(&mut self.fe.game.requests) {
            // No game yet: report, then come back to the main menu like the engine does when a
            // game ends.
            if matches!(r, Request::StartGame { .. } | Request::LoadGame(_))
                && self.fe.stack.len() <= 1
            {
                self.fe.screen_start(MAIN_SCREEN);
            }
            out.push(r);
        }
        out
    }

    pub fn render(&mut self) -> &Canvas {
        match &self.fe.game.credits {
            Some(c) => c.render(&mut self.canvas),
            None => self.fe.render(&mut self.canvas),
        }
        &self.canvas
    }
}
