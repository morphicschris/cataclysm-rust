//! From "start game" to a running mission (`singlePlayerStartGame` -> `horseRaceInit` ->
//! level loading -> `horseRaceShutdown`): the loading screen is drawn, then one piece of the
//! work is done per frame and the bar moves on, until the [`Game`] is ready. The window and the
//! headless tests go through the same steps.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::MissionRun;
use hwc_ui::cataclysm::unpack_color;
use hwc_ui::session::Session;
use hwc_ui::AssetSource;

use crate::data::Assets;
use crate::game::Game;
use crate::loading::LoadingScreen;
use crate::mission::{self, Loader};
use crate::world::World;

/// How long the "not playable yet" note stays before the menu comes back.
pub const UNSUPPORTED_TIME: f32 = 4.0;

enum Step {
    Start,
    Statics,
    Level,
    Art,
    Interface,
    Upload,
    Failed(String, f32),
}

/// What [`Loading::advance`] produced.
pub enum Progress {
    Working,
    Ready(Box<Game>),
    /// Could not start (an unsupported mission, missing data): back to the menu.
    Abandon(String),
}

pub struct Loading {
    pub screen: LoadingScreen,
    pub mission: u32,
    source: Rc<dyn AssetSource>,
    assets: Assets,
    loader: Loader,
    run: Option<MissionRun>,
    world: Option<World>,
    game: Option<Game>,
    step: Step,
}

/// The player's team colours from the options (`PlayerBaseColor` / `PlayerStripeColor`, once
/// picked).
pub fn player_colors(session: &Session) -> Option<([u8; 3], [u8; 3])> {
    let o = &session.fe.game.options;
    if !o.get_bool("ColorsPicked") {
        return None;
    }
    let b = unpack_color(o.get("PlayerBaseColor"));
    let s = unpack_color(o.get("PlayerStripeColor"));
    Some(([b[0], b[1], b[2]], [s[0], s[1], s[2]]))
}

impl Loading {
    pub fn new(
        data: Rc<GameData>,
        session: &Session,
        mission: u32,
        difficulty: i32,
        size: (i32, i32),
    ) -> Result<Loading, String> {
        let source: Rc<dyn AssetSource> = data.clone();
        let language = session.fe.game.options.get("language").max(0) as u32;
        let screen = LoadingScreen::new(source.clone(), language, mission, difficulty, size)?;
        Ok(Loading {
            screen,
            mission,
            source,
            assets: Assets(data),
            loader: Loader::new(mission, difficulty),
            run: None,
            world: None,
            game: None,
            step: Step::Start,
        })
    }

    /// The note shown instead of loading, if loading failed.
    pub fn failure(&self) -> Option<&str> {
        match &self.step {
            Step::Failed(m, _) => Some(m),
            _ => None,
        }
    }

    /// Does the next piece of work. `dt` counts down the failure note; `renderer` uploads
    /// the models when there is a GPU.
    pub fn advance(
        &mut self,
        session: &Session,
        renderer: Option<&mut Renderer>,
        dt: f32,
    ) -> Progress {
        let next = match std::mem::replace(&mut self.step, Step::Start) {
            Step::Start => {
                if mission::supported(self.mission) {
                    self.screen.set_progress(0.05);
                    Step::Statics
                } else {
                    Step::Failed(
                        format!("Mission {} is not playable yet.", self.mission),
                        UNSUPPORTED_TIME,
                    )
                }
            }
            Step::Statics => {
                self.loader.load_statics(&self.assets);
                self.screen.set_progress(0.4);
                Step::Level
            }
            Step::Level => match self.loader.start(&self.assets) {
                Ok(run) => {
                    self.run = Some(run);
                    self.screen.set_progress(0.55);
                    Step::Art
                }
                Err(e) => Step::Failed(e, UNSUPPORTED_TIME),
            },
            Step::Art => {
                let run = self.run.take().expect("run");
                match World::new(self.assets.clone(), run, player_colors(session)) {
                    Ok(w) => {
                        self.world = Some(w);
                        self.screen.set_progress(0.8);
                        Step::Interface
                    }
                    Err(e) => Step::Failed(e, UNSUPPORTED_TIME),
                }
            }
            Step::Interface => {
                let world = self.world.take().expect("world");
                match Game::new(self.source.clone(), session, world) {
                    Ok(g) => {
                        self.game = Some(g);
                        self.screen.set_progress(0.9);
                        Step::Upload
                    }
                    Err(e) => Step::Failed(e, UNSUPPORTED_TIME),
                }
            }
            Step::Upload => {
                let mut game = self.game.take().expect("game");
                if let Some(r) = renderer {
                    game.world.prepare(r);
                }
                self.screen.set_progress(1.0);
                return Progress::Ready(Box::new(game));
            }
            Step::Failed(m, left) => {
                let left = left - dt;
                if left <= 0.0 {
                    return Progress::Abandon(m);
                }
                Step::Failed(m, left)
            }
        };
        self.step = next;
        Progress::Working
    }

    /// The loading screen (with the failure note, if any).
    pub fn render(&mut self, size: (i32, i32)) -> &hwc_ui::Canvas {
        let note = self.failure().map(str::to_string);
        self.screen.render(size);
        if let Some(n) = note {
            self.screen.note(&n);
        }
        &self.screen.canvas
    }
}
