//! From "start game" to a running mission (`singlePlayerStartGame` -> `horseRaceInit` ->
//! level loading -> `horseRaceShutdown`): the loading screen is drawn, then one piece of the
//! work is done per frame and the bar moves on, until the [`Game`] is ready. The window and the
//! headless tests go through the same steps.
//!
//! A mission that starts with an NIS level (`hwc_sim::nis_level`) loads twice, each behind a
//! loading screen of its own as `singlePlayerLoadNewLevel` shows one for each: first the NIS
//! level, whose [`Game`] plays the NIS; when it stops ([`crate::game::Outcome::NisLevelDone`])
//! [`Loading::after_nis_level`] loads the mission itself from it (`NisLevelRun::finish`).

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::nis_level::NisLevelRun;
use hwc_ui::cataclysm::unpack_color;
use hwc_ui::session::Session;
use hwc_ui::AssetSource;

use crate::data::Assets;
use crate::game::Game;
use crate::hotkeys::CarriedGroups;
use crate::loading::LoadingScreen;
use crate::mission::{self, Loader};
use crate::world::{Sim, World};

/// How long the "not playable yet" note stays before the menu comes back.
pub const UNSUPPORTED_TIME: f32 = 4.0;

enum Step {
    Start,
    Statics,
    Level,
    Art,
    Nis,
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
    language: u32,
    difficulty: i32,
    source: Rc<dyn AssetSource>,
    assets: Assets,
    pub loader: Loader,
    sim: Option<Sim>,
    world: Option<World>,
    /// The NIS of the NIS level, started in its world.
    nis: Option<(crate::nis::NisRun, Vec<hwc_nis::Fired>)>,
    /// Play the NIS level before the missions that have one (the original always does).
    pub opening_nis: bool,
    /// The NIS level that has just played: the mission loads from it.
    after_nis: Option<Box<NisLevelRun>>,
    /// `G_NisCentreDerelict` / `G_NisCentreInstance` of the mission before.
    centre: Option<(i32, Option<i32>)>,
    /// The hot-key groups of the fleet coming from the mission before.
    groups: Option<CarriedGroups>,
    game: Option<Game>,
    step: Step,
    /// Loading a saved game: its file, and its interface once read.
    saved: Option<std::path::PathBuf>,
    saved_ui: Option<crate::saves::UiState>,
    /// Where the game's saves go (given to the game when it is ready).
    pub saves: Option<Rc<crate::saves::SaveDir>>,
    /// A skirmish to start instead of a mission.
    skirmish: Option<hwc_ui::skirmish::SkirmishRequest>,
    /// The saved game is a recorded game's start: its recording plays back.
    pub playback: bool,
    /// The mission number of the saved game being loaded ([`crate::saves::SKIRMISH_MISSION`]
    /// for a skirmish).
    saved_mission: u32,
    /// A network game to start instead: its setup, and the session that plays it.
    network: Option<crate::lan::NetStart>,
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
            language,
            difficulty,
            source,
            assets: Assets(data),
            loader: Loader::new(mission, difficulty),
            sim: None,
            world: None,
            nis: None,
            opening_nis: std::env::var_os("HWC_SKIP_NIS").is_none(),
            after_nis: None,
            centre: None,
            groups: None,
            game: None,
            step: Step::Start,
            saved: None,
            saved_ui: None,
            saves: None,
            skirmish: None,
            saved_mission: 0,
            playback: false,
            network: None,
        })
    }

    /// A network game behind a loading screen: the same game on every machine.
    pub fn network(data: Rc<GameData>, session: &Session, start: crate::lan::NetStart, size: (i32, i32)) -> Result<Loading, String> {
        let mut l = Loading::new(data, session, 0, 2, size)?;
        l.opening_nis = false;
        l.loader.multiplayer = true;
        l.network = Some(start);
        Ok(l)
    }

    /// A skirmish (`utyStartSkirmish` → `gameStart`) behind a loading screen.
    pub fn skirmish(
        data: Rc<GameData>,
        session: &Session,
        req: hwc_ui::skirmish::SkirmishRequest,
        size: (i32, i32),
    ) -> Result<Loading, String> {
        let mut l = Loading::new(data, session, 0, 2, size)?;
        l.opening_nis = false;
        l.loader.multiplayer = true;
        l.skirmish = Some(req);
        Ok(l)
    }

    /// Loading a saved game (`gameStart(file)`: `PreLoadGame`, the statics, `LoadGame`) behind
    /// the loading screen of its mission. No level file is read and no NIS level plays: the
    /// game carries on where it was saved.
    pub fn from_save(
        data: Rc<GameData>,
        session: &Session,
        path: &std::path::Path,
        size: (i32, i32),
    ) -> Result<Loading, String> {
        let h = hwc_sim::save::read_header(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let skirmish = h.mission == crate::saves::SKIRMISH_MISSION;
        // A skirmish loads behind the loading screen a skirmish starts with, the statics for
        // multiplayer.
        let mission = if skirmish { 0 } else { h.mission };
        let mut l = Loading::new(data, session, mission, if skirmish { 2 } else { h.skill }, size)?;
        l.saved = Some(path.to_path_buf());
        l.saved_mission = h.mission;
        l.loader.multiplayer = skirmish;
        l.opening_nis = false;
        Ok(l)
    }

    /// The next mission of the campaign: the fleet that jumped out of the mission before
    /// arrives in it (`singlePlayerLoadNewLevel`).
    /// `centre` is the mission before's `G_NisCentreDerelict` and `G_NisCentreInstance`, which
    /// centre an NIS level's NIS on a derelict.
    pub fn with_carry(
        mut self,
        carry: hwc_sim::campaign::Carry,
        centre: Option<(i32, Option<i32>)>,
    ) -> Self {
        self.loader.carry = Some(carry);
        self.centre = centre;
        self
    }

    /// The hot-key groups of the fleet coming in (they are put back on its ships once the
    /// mission's level has loaded).
    pub fn with_groups(mut self, groups: Option<CarriedGroups>) -> Self {
        self.groups = groups;
        self
    }

    /// The second loading of a mission with an NIS level: its NIS has stopped, and the
    /// mission's own level loads (`singlePlayerLoadNewLevel(0, 0)` behind a new loading
    /// screen) with the fleet the NIS level held.
    pub fn after_nis_level(
        data: Rc<GameData>,
        session: &Session,
        nis: NisLevelRun,
        difficulty: i32,
        size: (i32, i32),
    ) -> Result<Loading, String> {
        let mut l = Loading::new(data, session, nis.mission, difficulty, size)?;
        // The statics stay loaded from the NIS level.
        l.loader.statics = Some(nis.universe.statics.clone());
        l.loader.tweaks = Some(nis.universe.tweaks.clone());
        l.after_nis = Some(Box::new(nis));
        Ok(l)
    }

    /// The NIS level runs before this mission.
    fn plays_nis_level(&self) -> bool {
        self.opening_nis && hwc_sim::nis_level::nis_level_before(self.mission).is_some()
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
                // The NIS level's own loading picture.
                if self.after_nis.is_none() && self.plays_nis_level() {
                    let nis = hwc_sim::nis_level::nis_level_before(self.mission);
                    let size = (self.screen.canvas.width, self.screen.canvas.height);
                    match LoadingScreen::for_level(
                        self.source.clone(),
                        self.language,
                        self.mission,
                        nis,
                        self.difficulty,
                        size,
                    ) {
                        Ok(s) => self.screen = s,
                        Err(e) => eprintln!("cataclysm: {e}"),
                    }
                }
                if mission::supported(self.mission) || self.skirmish.is_some() || self.network.is_some() || self.saved_mission == crate::saves::SKIRMISH_MISSION {
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
            Step::Level => {
                // `singlePlayerStartNis(.., load level)` for an NIS level, else (or after it)
                // `singlePlayerLoadNewLevel` for the mission.
                let sim = match self.after_nis.take() {
                    Some(n) => self.loader.finish(*n, &self.assets).map(Sim::Mission),
                    None if self.saved.is_some() => self.read_saved().map(Sim::Mission),
                    None if self.network.is_some() => {
                        let setup = self.network.as_ref().expect("network game").setup.clone();
                        self.loader.start_network(&self.assets, &setup).map(Sim::Mission)
                    }
                    None if self.skirmish.is_some() => {
                        let req = self.skirmish.clone().expect("skirmish");
                        let seed = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |d| d.subsec_nanos());
                        self.loader
                            .start_skirmish(&self.assets, &req, seed)
                            .map(Sim::Mission)
                    }
                    None if self.plays_nis_level() => self
                        .loader
                        .start_nis_level(&self.assets, self.centre)
                        .map(|n| Sim::NisLevel(Box::new(n))),
                    None => self.loader.start(&self.assets).map(Sim::Mission),
                };
                match sim {
                    Ok(sim) => {
                        self.sim = Some(sim);
                        self.screen.set_progress(0.55);
                        Step::Art
                    }
                    Err(e) => Step::Failed(e, UNSUPPORTED_TIME),
                }
            }
            Step::Art => {
                let sim = self.sim.take().expect("sim");
                match World::new(self.assets.clone(), sim, player_colors(session)) {
                    Ok(w) => {
                        self.world = Some(w);
                        self.screen.set_progress(0.7);
                        Step::Nis
                    }
                    Err(e) => Step::Failed(e, UNSUPPORTED_TIME),
                }
            }
            Step::Nis => {
                // `singlePlayerStartNis`: the NIS level's NIS starts (mission 1's opening, or
                // the NIS before a mission of table 0x88a3f4).
                let world = self.world.as_mut().expect("world");
                if world.nis_level().is_some() {
                    world.follow_focus = false;
                    match crate::nis::start_level(world) {
                        Ok(n) => self.nis = Some(n),
                        Err(e) => return self.fail(e),
                    }
                }
                self.screen.set_progress(0.8);
                Step::Interface
            }
            Step::Interface => {
                let world = self.world.take().expect("world");
                match Game::new(self.source.clone(), session, world) {
                    Ok(mut g) => {
                        if let Some(d) = &self.saves {
                            g.set_saves(d.clone());
                        }
                        if let Some(req) = &self.skirmish {
                            g.local_name = Some(req.name.clone());
                        }
                        if let Some(n) = self.network.take() {
                            let local = n.setup.local_player;
                            crate::host::set_player(local);
                            g.local_name = n.names.get(local).cloned();
                            g.net = Some(crate::net_game::NetLink::new(n.session, n.names));
                        }
                        if let Some(ui) = self.saved_ui.take() {
                            g.restore_ui(ui);
                            if let Some(p) = &self.saved {
                                g.after_load(p);
                            }
                        }
                        if let (true, Some(p)) = (self.playback, &self.saved) {
                            if let Err(e) = g.start_playback(p) {
                                return self.fail(e);
                            }
                        }
                        if let Some((run, fired)) = self.nis.take() {
                            g.queue_nis(run, fired);
                        }
                        if let Some(groups) = self.groups.take() {
                            match g.world.mission_run() {
                                Some(run) => {
                                    g.hotkeys =
                                        groups.arrive(&run.universe, &run.game.hyper.fleet_ids)
                                }
                                None => g.carried_groups = Some(groups),
                            }
                        }
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
                    game.prepare(r);
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

    /// `LoadGame`: the saved game, linked to the statics just loaded.
    fn read_saved(&mut self) -> Result<hwc_sim::mission::MissionRun, String> {
        let path = self.saved.clone().expect("saved game");
        let statics = self.loader.statics.clone().expect("statics");
        let relink = crate::saves::relink(&self.assets, statics, self.saved_mission)?;
        let (_, mut payload) =
            crate::saves::read(&path, &relink).map_err(|e| format!("{}: {e}", path.display()))?;
        // A level without a mission file (the tutorial, a skirmish) keeps the statics as
        // loaded.
        if self.saved_mission != crate::saves::SKIRMISH_MISSION {
            let _ = payload
                .run
                .restore_ship_availability(&hwc_sim::mission::GameDataSource(self.assets.data()));
        }
        self.saved_ui = Some(payload.ui);
        Ok(payload.run)
    }

    fn fail(&mut self, why: String) -> Progress {
        self.step = Step::Failed(why, UNSUPPORTED_TIME);
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
