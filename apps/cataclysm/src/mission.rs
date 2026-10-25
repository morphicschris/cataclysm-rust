//! Starting a single-player mission on the simulation and turning what its KAS script does
//! into presentation cues (music, speech, location cards, the mission's end).
//!
//! Speech, location cards and music come from the mission's presentation queue
//! (`Game::presentation`, drained after each update); a failed mission from
//! `Game::mission_failed`, a won one from `Game::mission_completed`.

use std::sync::Arc;

use hwc_sim::campaign::Carry;
use hwc_sim::level::LevelState;
use hwc_sim::mission::{
    load_kas_mission, mission_script_name, MissionConfig, MissionRun, Presentation,
};
use hwc_sim::nis_level::NisLevelRun;
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

/// Reads the mission's presentation queue and end flags into [`Cue`]s.
#[derive(Debug, Default)]
pub struct CueReader {
    completed: bool,
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
        if run.game.mission_completed && !self.completed {
            self.completed = true;
            out.push(Cue::Completed);
        }
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

/// Missions the simulation can start: the campaign's seventeen (mission 1 from the opening
/// NIS, the others with the fleet carried over from the mission before, or the `warp.script`
/// fleet when started directly).
pub fn supported(n: u32) -> bool {
    (1..=hwc_nis::tables::MISSIONS).contains(&n)
}

/// The pieces of a mission start, so the loading screen can show progress between them.
pub struct Loader {
    pub mission: u32,
    pub skill: i32,
    pub statics: Option<Arc<StaticWorld>>,
    pub tweaks: Option<SimTweaks>,
    /// The game plays the NIS and NISlets the mission asks for (`MissionConfig::hold_nis`);
    /// otherwise the simulation counts them as played at once.
    pub play_nislets: bool,
    /// The fleet coming from the mission before (`singlePlayerLoadNewLevel`); without it a
    /// mission after the first starts from the `warp.script` fleet (the original's warp).
    pub carry: Option<Carry>,
}

impl Loader {
    pub fn new(mission: u32, skill: i32) -> Self {
        Self {
            mission,
            skill,
            statics: None,
            tweaks: None,
            play_nislets: true,
            carry: None,
        }
    }

    /// Ship and game statics (the slow part).
    pub fn load_statics(&mut self, assets: &Assets) {
        if self.statics.is_some() {
            return;
        }
        let options = LoadOptions {
            single_player: true,
            ..LoadOptions::default()
        };
        self.statics = Some(Arc::new(StaticWorld::load_from(assets, options)));
        self.tweaks = Some(SimTweaks::load(assets));
    }

    /// The mission's KAS script and configuration.
    fn script_and_config(
        &self,
        assets: &Assets,
    ) -> Result<(hwc_kas::Mission, MissionConfig), String> {
        let name = mission_script_name(self.mission);
        let script = load_kas_mission(assets.data(), &name).map_err(|e| e.to_string())?;
        let mut config = MissionConfig::new(self.mission);
        config.skill = self.skill;
        // The game plays the NIS and NISlets and reports their end.
        config.hold_nis = self.play_nislets;
        Ok((script, config))
    }

    /// The fleet coming into the mission: the one carried over, or for a mission after the
    /// first started directly the `warp.script` fleet (none for mission 1 from the start).
    fn take_carry(&mut self, assets: &Assets) -> Result<Option<Carry>, String> {
        if self.mission == 1 && self.carry.is_none() {
            return Ok(None);
        }
        match self.carry.take() {
            Some(c) => Ok(Some(c)),
            None => {
                let statics = self.statics.clone().expect("statics");
                let tweaks = self.tweaks.clone().expect("tweaks");
                Carry::warp(statics, tweaks, assets, self.mission)
                    .map(Some)
                    .ok_or_else(|| format!("no warp fleet for mission {}", self.mission))
            }
        }
    }

    /// The NIS level before the mission (`singlePlayerStartNis(.., load level)`), with the
    /// fleet waiting in hyperspace. `centre` is the mission before's `G_NisCentreDerelict` and
    /// `G_NisCentreInstance`.
    pub fn start_nis_level(
        &mut self,
        assets: &Assets,
        centre: Option<(i32, Option<i32>)>,
    ) -> Result<NisLevelRun, String> {
        if !supported(self.mission) {
            return Err(format!("mission {} is not part of the campaign", self.mission));
        }
        self.load_statics(assets);
        let statics = self.statics.clone().expect("statics");
        let tweaks = self.tweaks.clone().expect("tweaks");
        let carry = self.take_carry(assets)?;
        let mut run =
            NisLevelRun::start(statics, tweaks, assets, self.mission, carry.as_ref(), centre)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("mission {} has no NIS level", self.mission))?;
        run.universe.sp_difficulty = self.skill.max(0) as u32;
        Ok(run)
    }

    /// The mission after its NIS level has played (`NisLevelRun::finish`: the mission's
    /// level with the fleet the NIS level held, `G_Nis<n>Complete` set).
    pub fn finish(&mut self, nis: NisLevelRun, assets: &Assets) -> Result<MissionRun, String> {
        self.load_statics(assets);
        let statics = self.statics.clone().expect("statics");
        let tweaks = self.tweaks.clone().expect("tweaks");
        let (script, config) = self.script_and_config(assets)?;
        nis.finish(statics, tweaks, assets, script, config)
            .map_err(|e| e.to_string())
    }

    /// The level and the KAS script (`singlePlayerLoadNewLevel`).
    pub fn start(&mut self, assets: &Assets) -> Result<MissionRun, String> {
        if !supported(self.mission) {
            return Err(format!("mission {} is not part of the campaign", self.mission));
        }
        self.load_statics(assets);
        let statics = self.statics.clone().expect("statics");
        let tweaks = self.tweaks.clone().expect("tweaks");
        let (script, config) = self.script_and_config(assets)?;
        if self.mission == 1 && self.carry.is_none() {
            return MissionRun::start(statics, tweaks, assets, script, config)
                .map_err(|e| e.to_string());
        }
        let carry = match self.carry.take() {
            Some(c) => c,
            None => Carry::warp(statics.clone(), tweaks.clone(), assets, self.mission)
                .ok_or_else(|| format!("no warp fleet for mission {}", self.mission))?,
        };
        MissionRun::start_campaign(statics, tweaks, assets, script, config, &carry)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_campaign_starts() {
        assert!(supported(1));
        assert!(supported(2));
        assert!(supported(17));
        assert!(!supported(0));
        assert!(!supported(18));
    }
}
