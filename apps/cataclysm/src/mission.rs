//! Starting a single-player mission on the simulation and turning what its KAS script does
//! into presentation cues (music, speech, location cards, the mission's end).
//!
//! Speech, location cards and music come from the mission's presentation queue
//! (`Game::presentation`, drained after each update); a failed mission from
//! `Game::mission_failed`. `MissionCompleted` has no host implementation yet and is read from
//! the mission log's `Unimplemented` lines.

use std::sync::Arc;

use hwc_sim::level::LevelState;
use hwc_sim::mission::{
    load_kas_mission, mission_script_name, LogKind, MissionConfig, MissionRun, Presentation,
};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

use crate::data::Assets;

/// Something the mission asked the presentation layer to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    Music(i32),
    Speech {
        event: u32,
        variable: i32,
    },
    LocationCard {
        milliseconds: i32,
        text: String,
    },
    /// `MissionCompleted()`: the level is won.
    Completed,
    /// `MissionFailed()`.
    Failed,
}

/// Reads the mission's presentation queue and log into [`Cue`]s.
#[derive(Debug, Default)]
pub struct CueReader {
    seen: usize,
    failed: bool,
}

impl CueReader {
    /// New cues since the last call (drains the mission's presentation queue).
    pub fn poll(&mut self, run: &mut MissionRun) -> Vec<Cue> {
        let mut out: Vec<Cue> = run
            .game
            .presentation
            .drain(..)
            .map(|p| match p {
                Presentation::Speech {
                    event, variable, ..
                } => Cue::Speech {
                    event: event as u32,
                    variable,
                },
                Presentation::LocationCard {
                    text, milliseconds, ..
                } => Cue::LocationCard { milliseconds, text },
                Presentation::Music { track, .. } => Cue::Music(track),
            })
            .collect();
        let log = &run.game.log;
        for e in &log[self.seen.min(log.len())..] {
            if e.kind == LogKind::Unimplemented {
                let name = e.text.split([' ', '(']).next().unwrap_or("");
                if name == "MissionCompleted" || name == "GameEnd" {
                    out.push(Cue::Completed);
                }
            }
        }
        self.seen = log.len();
        if run.game.mission_failed && !self.failed {
            self.failed = true;
            out.push(Cue::Failed);
        }
        out
    }
}

/// The level's `Background` and `Lighting` lines (asset names without folder or extension).
pub fn level_presentation(level: &LevelState) -> (Option<String>, Option<String>) {
    let find = |key: &str| {
        level
            .unmodelled
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .and_then(|(_, v)| v.split(',').next())
            .map(|v| v.trim().to_owned())
            .filter(|v| !v.is_empty())
    };
    (find("Background"), find("Lighting"))
}

/// Missions the simulation can start (`MissionRun::start` carries no fleet over from an
/// earlier mission yet, so only the first one).
pub fn supported(n: u32) -> bool {
    n == 1
}

/// The pieces of a mission start, so the loading screen can show progress between them.
pub struct Loader {
    pub mission: u32,
    pub skill: i32,
    pub statics: Option<Arc<StaticWorld>>,
    pub tweaks: Option<SimTweaks>,
}

impl Loader {
    pub fn new(mission: u32, skill: i32) -> Self {
        Self {
            mission,
            skill,
            statics: None,
            tweaks: None,
        }
    }

    /// Ship and game statics (the slow part).
    pub fn load_statics(&mut self, assets: &Assets) {
        let options = LoadOptions {
            single_player: true,
            ..LoadOptions::default()
        };
        self.statics = Some(Arc::new(StaticWorld::load_from(assets, options)));
        self.tweaks = Some(SimTweaks::load(assets));
    }

    /// The level and the KAS script (`singlePlayerLoadNewLevel`).
    pub fn start(&mut self, assets: &Assets) -> Result<MissionRun, String> {
        if !supported(self.mission) {
            return Err(format!(
                "mission {} is not supported yet (the simulation starts only mission 1)",
                self.mission
            ));
        }
        if self.statics.is_none() {
            self.load_statics(assets);
        }
        let statics = self.statics.clone().expect("statics");
        let tweaks = self.tweaks.clone().expect("tweaks");
        let name = mission_script_name(self.mission);
        let script = load_kas_mission(assets.data(), &name).map_err(|e| e.to_string())?;
        let mut config = MissionConfig::new(self.mission);
        config.skill = self.skill;
        MissionRun::start(statics, tweaks, assets, script, config).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_mission_one_starts() {
        assert!(supported(1));
        assert!(!supported(2));
    }
}
