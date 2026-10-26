//! Saved games on disk: where they go, what a file holds besides the simulation, and the list
//! the save and load screens show (`hwc_ui::saves::SaveStore`). The file format itself is
//! `hwc_sim::save` (see `docs/sim/savegame.md`).
//!
//! The original keeps its games in `SavedGames\SinglePlayer` and `SavedGames\Training` of the
//! install, named by the description typed. Here they go in the player's own folder, never the
//! original install: `saves\` of the data directory made by the importer when it can be
//! written, else `%APPDATA%\hwc-cataclysm\saves` (`HWC_SAVE_DIR` overrides both), with the same
//! two sub-folders and the description as the file name (`.hwcsave`).

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use hwc_formats::gamedata::Layout;
use hwc_formats::GameData;
use hwc_sim::mission::{load_kas_mission, mission_script_name, MissionRun};
use hwc_sim::save::{self, Relink, SaveError, SaveHeader};
use hwc_sim::statics::StaticWorld;
use hwc_sim::ShipId;
use hwc_ui::saves::{sort_entries, SaveEntry, SaveFolder, SaveStore, Thumbnail};
use serde::{Deserialize, Serialize};

use crate::data::Assets;
use crate::hotkeys::HotKeyGroups;
use crate::pings::Pings;
use crate::world::ViewState;

/// The extension of a saved game.
pub const EXT: &str = "hwcsave";
/// The name of the quick save (the original's list keeps an entry of this name first).
pub const QUICK_SAVE: &str = "Quick Save";
/// Overrides where the saved games go.
pub const ENV_SAVE_DIR: &str = "HWC_SAVE_DIR";

/// The interface's part of a saved game: what the player sees and has set up that is not in
/// the simulation (`SaveMaxSelection` of the selection and hot-key groups, the camera, the
/// music track, the pings).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UiState {
    pub view: ViewState,
    pub selection: Vec<ShipId>,
    pub hotkeys: HotKeyGroups,
    pub music: Option<i32>,
    pub pings: Pings,
    pub ping_sensor_states: Vec<(ShipId, i32)>,
    pub pings_seeded: bool,
    pub letterbox: hwc_ui::letterbox::Letterbox,
    pub keep_letterbox_after_nis: bool,
    pub formation_index: usize,
    /// The latest mission autosave (`CurrentLevelName`): what "restart level" loads.
    pub level_save: Option<PathBuf>,
}

#[derive(Serialize)]
struct PayloadRef<'a> {
    run: &'a MissionRun,
    ui: &'a UiState,
}

/// A saved game read back.
#[derive(Deserialize)]
pub struct Payload {
    pub run: MissionRun,
    pub ui: UiState,
}

/// Writes a game.
pub fn write(path: &Path, header: &SaveHeader, run: &MissionRun, ui: &UiState) -> Result<(), SaveError> {
    save::write(path, header, &PayloadRef { run, ui })
}

/// Reads a game, linked to the game data.
pub fn read(path: &Path, relink: &Relink) -> Result<(SaveHeader, Payload), SaveError> {
    save::read(path, relink)
}

/// The game data a saved game of `mission` links back to: the statics, the effects and the
/// mission's script.
pub fn relink(assets: &Assets, statics: Arc<StaticWorld>, mission: u32) -> Result<Relink, String> {
    let mut r = Relink::new(statics);
    let fx = hwc_sim::effects::EffectData::load_from(assets);
    if !fx.is_empty() {
        r.effects = Some(fx);
    }
    let script = load_kas_mission(assets.data(), &mission_script_name(mission)).map_err(|e| e.to_string())?;
    r.mission = Some(Arc::new(script));
    Ok(r)
}

/// The description a saved game's header holds.
pub fn read_description(path: &Path) -> Option<String> {
    save::read_header(path).ok().map(|h| h.description)
}

/// The header of a game being saved.
pub fn header(run: &MissionRun, description: &str, autosave: bool, thumbnail: Option<save::Thumbnail>) -> SaveHeader {
    let mission = run.game.config.mission;
    SaveHeader {
        description: description.to_string(),
        mission,
        mission_name: mission_name(mission),
        saved_at: save::now(),
        game_time: run.universe.total_time_elapsed,
        frame: run.universe.update_counter,
        autosave,
        skill: run.game.config.skill,
        script_fingerprint: save::script_fingerprint(run.kas.mission()),
        writer: format!("cataclysm {}", env!("CARGO_PKG_VERSION")),
        thumbnail,
    }
}

/// How the lists name a mission.
pub fn mission_name(mission: u32) -> String {
    if mission == 0 {
        "Training".into()
    } else {
        format!("Mission {mission:02}")
    }
}

/// The list's second line: the mission and when it was saved (UTC).
pub fn detail(h: &SaveHeader) -> String {
    format!("{} - {}", h.mission_name.to_ascii_uppercase(), date_text(h.saved_at))
}

/// `secs` since 1970 as `26 OCT 2026 14:03 UTC`.
pub fn date_text(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Days to the civil date (proleptic Gregorian).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    const MONTHS: [&str; 12] = ["JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC"];
    format!("{d} {} {y} {:02}:{:02} UTC", MONTHS[(m - 1) as usize], rem / 3600, rem % 3600 / 60)
}

/// A file name for a description: characters a file name cannot hold become `_`.
pub fn file_stem(name: &str) -> String {
    let mut s: String = name
        .trim()
        .chars()
        .take(hwc_ui::saves::MAX_NAME.max(64))
        .map(|c| if hwc_ui::saves::name_char_allowed(c) { c } else { '_' })
        .collect();
    while s.ends_with('.') || s.ends_with(' ') {
        s.pop();
    }
    if s.is_empty() {
        s = "Saved Game".into();
    }
    let upper = s.to_ascii_uppercase();
    let base = upper.split('.').next().unwrap_or("");
    let reserved = matches!(base, "CON" | "PRN" | "AUX" | "NUL")
        || (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && base.as_bytes()[3].is_ascii_digit());
    if reserved {
        s.insert(0, '_');
    }
    s
}

/// The player's save folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveDir {
    pub root: PathBuf,
}

fn writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(".hwc-write-test");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

impl SaveDir {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        SaveDir { root: root.into() }
    }

    /// Where this game's saves go: `HWC_SAVE_DIR`, else the data directory's `saves` when the
    /// game runs from one and it can be written, else `%APPDATA%\hwc-cataclysm\saves`. Never
    /// inside an original install.
    pub fn locate(data: &GameData) -> SaveDir {
        if let Some(d) = std::env::var_os(ENV_SAVE_DIR) {
            return SaveDir::new(d);
        }
        if data.layout() == Layout::Imported {
            if let Some(r) = data.root() {
                let d = r.join("saves");
                if writable(&d) {
                    return SaveDir::new(d);
                }
            }
        }
        let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
        SaveDir::new(base.join("hwc-cataclysm").join("saves"))
    }

    pub fn folder(&self, f: SaveFolder) -> PathBuf {
        self.root.join(match f {
            SaveFolder::SinglePlayer => "SinglePlayer",
            SaveFolder::Training => "Training",
        })
    }

    pub fn path_for(&self, f: SaveFolder, name: &str) -> PathBuf {
        self.folder(f).join(format!("{}.{EXT}", file_stem(name)))
    }

    /// The saved games of a folder with their headers (files that are not saved games of this
    /// version are listed with what can be said about them, so they can still be deleted).
    pub fn entries(&self, f: SaveFolder) -> Vec<(PathBuf, Result<SaveHeader, SaveError>)> {
        let Ok(rd) = std::fs::read_dir(self.folder(f)) else { return Vec::new() };
        let mut v: Vec<(PathBuf, Result<SaveHeader, SaveError>)> = rd
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(EXT)))
            .map(|p| {
                let h = save::read_header(&p);
                (p, h)
            })
            .collect();
        v.sort_by(|a, b| a.0.cmp(&b.0));
        v
    }
}

impl SaveStore for SaveDir {
    fn list(&self, folder: SaveFolder) -> Vec<SaveEntry> {
        let mut v: Vec<SaveEntry> = self
            .entries(folder)
            .into_iter()
            .map(|(p, h)| {
                let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                match h {
                    Ok(h) => SaveEntry {
                        key: p.to_string_lossy().into_owned(),
                        name: if h.description.is_empty() { stem } else { h.description.clone() },
                        detail: detail(&h),
                        thumbnail: h.thumbnail.map(|t| Rc::new(Thumbnail { width: t.width, height: t.height, rgb: t.rgb })),
                    },
                    Err(e) => SaveEntry {
                        key: p.to_string_lossy().into_owned(),
                        name: stem,
                        detail: match e {
                            SaveError::Version { found, .. } => format!("FORMAT {found}: CANNOT BE LOADED"),
                            _ => "DAMAGED".into(),
                        },
                        thumbnail: None,
                    },
                }
            })
            .collect();
        sort_entries(&mut v);
        v
    }

    fn delete(&self, key: &str) -> bool {
        let p = Path::new(key);
        // Only files of the save folder.
        p.starts_with(&self.root) && std::fs::remove_file(p).is_ok()
    }

    fn key_for(&self, folder: SaveFolder, name: &str) -> String {
        self.path_for(folder, name).to_string_lossy().into_owned()
    }

    fn exists(&self, key: &str) -> bool {
        Path::new(key).is_file()
    }
}

/// A thumbnail of `canvas` at most `w` pixels wide (point sampled).
pub fn thumbnail(canvas: &hwc_ui::Canvas, w: u32) -> Option<save::Thumbnail> {
    let (cw, ch) = (canvas.width.max(0) as u32, canvas.height.max(0) as u32);
    if cw == 0 || ch == 0 {
        return None;
    }
    let w = w.min(cw);
    let h = (ch * w / cw).max(1);
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for y in 0..h {
        let sy = y * ch / h;
        for x in 0..w {
            let sx = x * cw / w;
            let p = canvas.pixels[(sy * cw + sx) as usize];
            rgb.extend_from_slice(&p[..3]);
        }
    }
    Some(save::Thumbnail { width: w, height: h, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(date_text(0), "1 JAN 1970 00:00 UTC");
        assert_eq!(date_text(1_793_000_000), "26 OCT 2026 07:33 UTC");
        assert_eq!(date_text(951_782_400), "29 FEB 2000 00:00 UTC");
    }

    #[test]
    fn file_names() {
        assert_eq!(file_stem("02 - Outskirts of Hiigaran System"), "02 - Outskirts of Hiigaran System");
        assert_eq!(file_stem("a/b:c?"), "a_b_c_");
        assert_eq!(file_stem("  "), "Saved Game");
        assert_eq!(file_stem("con"), "_con");
        assert_eq!(file_stem("trailing..."), "trailing");
    }

    #[test]
    fn folders_and_keys() {
        let d = SaveDir::new("X:\\saves");
        assert_eq!(d.path_for(SaveFolder::SinglePlayer, QUICK_SAVE), Path::new("X:\\saves\\SinglePlayer\\Quick Save.hwcsave"));
        assert_eq!(d.path_for(SaveFolder::Training, "LESSON 01"), Path::new("X:\\saves\\Training\\LESSON 01.hwcsave"));
        assert!(!d.delete("C:\\Windows\\win.ini"));
    }
}
