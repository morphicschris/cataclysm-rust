//! Mission mode: starting a single-player mission on the simulation and turning what its KAS
//! script does into presentation cues (music, speech, location cards) and simple overlay text
//! (objectives, subtitles, notes about NIS sequences that are not played).
//!
//! The KAS host records most presentation calls only in the mission log, so the cues are read
//! back from the log's call lines (`team: Name(args) = result`); this needs the mission's
//! `log_calls` option, which mission mode turns on.

use std::sync::Arc;

use hwc_sim::level::LevelState;
use hwc_sim::mission::{
    load_kas_mission, mission_script_name, LogKind, MissionConfig, MissionRun,
};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

use crate::data::GameAssets;

/// Something the mission asked the presentation layer to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Cue {
    Music(i32),
    Speech { event: u32, variable: i32 },
    LocationCard { milliseconds: i32, text: String },
    /// A note for the screen (NIS skipped, mission failed, ...).
    Notice(String),
}

/// Reads new mission log lines into [`Cue`]s.
#[derive(Debug, Default)]
pub struct CueReader {
    seen: usize,
}

impl CueReader {
    pub fn poll(&mut self, run: &MissionRun) -> Vec<Cue> {
        let log = &run.game.log;
        let mut out = Vec::new();
        for e in &log[self.seen.min(log.len())..] {
            match e.kind {
                LogKind::Call => {
                    if let Some(c) = parse_call(&e.text).and_then(|(name, args)| cue_from_call(name, &args)) {
                        out.push(c);
                    }
                }
                LogKind::Flow => {
                    if let Some(n) = notice_from_flow(&e.text) {
                        out.push(Cue::Notice(n));
                    }
                }
                _ => {}
            }
        }
        self.seen = log.len();
        out
    }
}

/// `"Team: Name(a, b) = r"` -> `("Name", ["a", "b"])`. Quoted strings keep their commas.
pub fn parse_call(text: &str) -> Option<(&str, Vec<String>)> {
    let body = text.split_once(": ").map_or(text, |(_, b)| b);
    let open = body.find('(')?;
    let name = &body[..open];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return None;
    }
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut depth = 0;
    for c in body[open + 1..].chars() {
        match c {
            '"' => {
                quoted = !quoted;
                cur.push(c);
            }
            '(' if !quoted => {
                depth += 1;
                cur.push(c);
            }
            ')' if !quoted && depth == 0 => {
                if !cur.trim().is_empty() {
                    args.push(cur.trim().to_owned());
                }
                return Some((name, args));
            }
            ')' if !quoted => {
                depth -= 1;
                cur.push(c);
            }
            ',' if !quoted && depth == 0 => args.push(std::mem::take(&mut cur).trim().to_owned()),
            _ => cur.push(c),
        }
    }
    None
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches('"').to_owned()
}

fn cue_from_call(name: &str, args: &[String]) -> Option<Cue> {
    let int = |i: usize| args.get(i).and_then(|a| a.trim().parse::<i64>().ok());
    match name {
        "MusicPlay" => Some(Cue::Music(int(0)? as i32)),
        "SpeechEvent" => Some(Cue::Speech {
            event: int(0)? as u32,
            variable: int(1).unwrap_or(0) as i32,
        }),
        "LocationCard" => Some(Cue::LocationCard {
            milliseconds: int(0)? as i32,
            text: unquote(args.get(1)?),
        }),
        "MissionFailed" => Some(Cue::Notice("Mission failed".into())),
        _ => None,
    }
}

fn notice_from_flow(text: &str) -> Option<String> {
    if text.contains("opening NIS skipped") {
        let nis = text
            .split('(')
            .nth(1)
            .and_then(|r| r.split(')').next())
            .unwrap_or("nis");
        return Some(format!("Opening NIS skipped ({nis})"));
    }
    if text.starts_with("NIS ") && text.contains("not played") {
        return Some(format!("{} skipped (not played)", text.split(" requested").next().unwrap_or("NIS")));
    }
    if text.starts_with("NISlet ") && text.contains("requested") {
        return Some(format!("{} skipped (not played)", text.split(" of mission").next().unwrap_or("NISlet")));
    }
    if text.contains("hyperspacing ships in") {
        return Some("Fleet arriving from hyperspace".into());
    }
    None
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

/// Starts mission `n` with call logging on (the cue reader needs it).
pub fn start(assets: &GameAssets, n: u32, skill: i32) -> Result<MissionRun, String> {
    let name = mission_script_name(n);
    let script = load_kas_mission(&assets.data, &name).map_err(|e| e.to_string())?;
    let options = LoadOptions {
        single_player: true,
        ..LoadOptions::default()
    };
    let statics = Arc::new(StaticWorld::load_from(assets, options));
    let tweaks = SimTweaks::load(assets);
    let mut config = MissionConfig::new(n);
    config.skill = skill;
    config.log_calls = true;
    MissionRun::start(statics, tweaks, assets, script, config).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_calls() {
        let (n, a) = parse_call("FleetIntel: SpeechEvent(268439766, 0) = 0").unwrap();
        assert_eq!(n, "SpeechEvent");
        assert_eq!(a, vec!["268439766", "0"]);
        assert_eq!(
            cue_from_call(n, &a),
            Some(Cue::Speech {
                event: 268439766,
                variable: 0
            })
        );
        let (n, a) = parse_call("X: LocationCard(5000, \"Kharak, Higara\") = 0").unwrap();
        assert_eq!(
            cue_from_call(n, &a),
            Some(Cue::LocationCard {
                milliseconds: 5000,
                text: "Kharak, Higara".into()
            })
        );
        let (n, a) = parse_call("FleetIntel: MusicPlay(36) = 0").unwrap();
        assert_eq!(cue_from_call(n, &a), Some(Cue::Music(36)));
        let (n, a) = parse_call("T: StartLevelByHyperspacingShips() = 0").unwrap();
        assert_eq!(n, "StartLevelByHyperspacingShips");
        assert!(a.is_empty());
        assert!(parse_call("no call here").is_none());
    }

    #[test]
    fn notices() {
        assert_eq!(
            notice_from_flow("NISlet 1 of mission 1 requested: not played (finishes at 0.125s)"),
            Some("NISlet 1 skipped (not played)".into())
        );
        assert_eq!(
            notice_from_flow("mission 1: opening NIS skipped (nis\\n01.nis); fleet in hyperspace: x"),
            Some("Opening NIS skipped (nis\\n01.nis)".into())
        );
        assert_eq!(notice_from_flow("NISlet 1 finished: G_Nislet1Complete = 1"), None);
    }
}
