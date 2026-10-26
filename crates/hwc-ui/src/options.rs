//! Player options and their persistence.
//!
//! The original keeps two stores (see `docs/frontend.md`): `Cataclysm.cfg`, a text file of
//! `name value` lines written from one table of tweakable variables on exit and read back with the
//! tweak-script parser at start-up, and the registry key `HKLM\SOFTWARE\Sierra On-Line\Cataclysm`
//! for the display device and resolution. This module keeps every one of those values under its
//! original name in a single file of our own format:
//!
//! ```text
//! # hwc options v1
//! MusicVolume = 60
//! PlayerName = "Fleet Commander"
//! ```
//!
//! One `key = value` per line, `#` comments, integers or double-quoted strings (with `\"` and
//! `\\` escapes). Unknown keys are kept, so newer versions do not lose settings.

use std::collections::BTreeMap;
use std::path::Path;

/// A stored value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Int(i32),
    Str(String),
}

/// Original variable names with their built-in defaults (`Cataclysm.cfg` table, then the
/// registry values). Values are the engine's initial values.
pub const DEFAULTS: &[(&str, i32)] = &[
    ("effectsLevel", 0),
    ("speakerSetting", 1),
    ("brightnessVal", 30),
    ("detailThreshold", 10),
    ("noLOD", 0),
    ("enableSmoothing", 1),
    ("enableCPV", 0),
    ("enableLensFlare", 0),
    ("enableRepairbots", 0),
    ("showBackgrounds", 1),
    ("texLinearFiltering", 1),
    ("enableStipple", 0),
    ("enableTrails", 1),
    ("language", 0),
    ("fuzzyBlobs", 1),
    ("PlayerBaseColor", 0),
    ("PlayerStripeColor", 0),
    ("ColorsPicked", 0),
    ("PlayerRace", 0),
    ("InstantTransition", 0),
    ("PrevColor0.base", -16_777_216),
    ("PrevColor0.detail", -16_777_216),
    ("PrevColor1.base", -16_777_216),
    ("PrevColor1.detail", -16_777_216),
    ("PrevColor2.base", -16_777_216),
    ("PrevColor2.detail", -16_777_216),
    ("PrevColor3.base", -16_777_216),
    ("PrevColor3.detail", -16_777_216),
    ("PrevColor4.base", -16_777_216),
    ("PrevColor4.detail", -16_777_216),
    ("Equalizer0", 0),
    ("Equalizer1", 0),
    ("Equalizer2", 0),
    ("Equalizer3", 0),
    ("Equalizer4", 0),
    ("Equalizer5", 0),
    ("Equalizer6", 0),
    ("Equalizer7", 0),
    ("MusicVolume", 60),
    ("SFXVolume", 75),
    ("SpeechVolume", 75),
    ("VoiceCommands", 1),
    ("VoiceStatus", 1),
    ("VoiceChatter", 1),
    ("opMouseSens", 50),
    ("InfoOverlay", 1),
    ("opBattleChatter", 50),
    ("spCurrentSelected", 0),
    ("etgDamageEffectsEnabled", 1),
    ("etgHitEffectsEnabled", 1),
    ("etgFireEffectsEnabled", 1),
    ("etgBulletEffectsEnabled", 1),
    ("etgHistoryScalar", 256),
    ("NumChannels", 8),
    ("AutoChannel", 0),
    ("SoundQuality", 0),
    ("FirewallDetect", 2),
    ("TutorialNeeded", 1),
    ("LargePopupMenuFonts", 0),
    ("Homeworld_CSA", 0),
    ("RightClick_CSA", 1),
    ("AutoFocusOnWaypoints", 0),
    ("ShowTaskbarBackdrop", 0),
    ("ShowTaskbarButtons", 0),
    ("ExperienceMessages", 0),
    ("HomeworldTaskbar", 0),
    ("ShowAsteriod0", 1),
    ("ShowResources", 1),
    ("ShowFOWBlobs", 1),
    ("ShowBackground", 1),
    ("ShowAreaVisited", 1),
    ("ShowAreaOccupied", 0),
    ("ShowIcons", 1),
    ("ShowEnemyShips", 1),
    ("ShowFriendlyShips", 1),
    ("ShowUniverseExtents", 0),
    ("ShowMeteors", 1),
    ("ShowCompass", 0),
    ("ShowObjectivesPointer", 1),
    ("ShowSlipgatePaths", 1),
    ("HardwareTransformLighting", 0),
    ("TextureDetail", 50),
    ("GameFramerateOption", 1),
    ("DetailQuality", 0),
    ("SetTextureRequirements", 0),
    ("DifficultySetting", 2),
    // The recreation's own options (not in the original), each a user-requested change.
    // 1: ships are drawn at full detail whatever their distance; 0: the original's
    // level-of-detail selection (`--lod`).
    ("hwcFullDetailShips", 1),
    // 1: with ships selected, a right click on empty space opens the move plate and
    // Shift+right click the menu; 0: the original's right click (`--right-click`).
    ("hwcRightClickMove", 1),
    // Registry values.
    ("deviceIndex", -1),
    ("screenWidth", 640),
    ("screenHeight", 480),
    ("screenDepth", 16),
];

/// String-valued options (`PlayerName`, `PlayerPwd`, registry device names).
pub const STRING_DEFAULTS: &[(&str, &str)] = &[
    ("PlayerName", ""),
    ("PlayerPwd", ""),
    ("deviceToSelect", ""),
    ("glToSelect", ""),
    ("d3dToSelect", ""),
];

/// The option store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    values: BTreeMap<String, Value>,
}

impl Default for Options {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        for &(k, v) in DEFAULTS {
            values.insert(k.to_string(), Value::Int(v));
        }
        for &(k, v) in STRING_DEFAULTS {
            values.insert(k.to_string(), Value::Str(v.to_string()));
        }
        Self { values }
    }
}

impl Options {
    /// An integer option (0 if unknown or not an integer).
    pub fn get(&self, key: &str) -> i32 {
        match self.values.get(key) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        }
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.get(key) != 0
    }

    pub fn set(&mut self, key: &str, v: i32) {
        self.values.insert(key.to_string(), Value::Int(v));
    }

    pub fn set_bool(&mut self, key: &str, v: bool) {
        self.set(key, i32::from(v));
    }

    pub fn get_str(&self, key: &str) -> &str {
        match self.values.get(key) {
            Some(Value::Str(s)) => s,
            _ => "",
        }
    }

    pub fn set_str(&mut self, key: &str, v: &str) {
        self.values
            .insert(key.to_string(), Value::Str(v.to_string()));
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Value)> {
        self.values.iter().map(|(k, v)| (k.as_str(), v))
    }

    /// Serialises in the file format described in the module docs.
    pub fn to_text(&self) -> String {
        let mut out = String::from("# hwc options v1\n# Front-end settings; names are the original Cataclysm.cfg/registry names.\n");
        for (k, v) in &self.values {
            match v {
                Value::Int(i) => out.push_str(&format!("{k} = {i}\n")),
                Value::Str(s) => {
                    let esc = s.replace('\\', "\\\\").replace('"', "\\\"");
                    out.push_str(&format!("{k} = \"{esc}\"\n"));
                }
            }
        }
        out
    }

    /// Parses the file format, starting from the defaults. Malformed lines are reported and
    /// skipped.
    pub fn from_text(text: &str) -> (Options, Vec<String>) {
        let mut opts = Options::default();
        let mut errors = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                errors.push(format!("line {}: expected `key = value`", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if k.is_empty() || k.contains(char::is_whitespace) {
                errors.push(format!("line {}: bad key", n + 1));
                continue;
            }
            if let Some(body) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                let mut s = String::new();
                let mut it = body.chars();
                while let Some(c) = it.next() {
                    if c == '\\' {
                        if let Some(n) = it.next() {
                            s.push(n);
                        }
                    } else {
                        s.push(c);
                    }
                }
                opts.values.insert(k.to_string(), Value::Str(s));
            } else if let Ok(i) = v.parse::<i32>() {
                opts.values.insert(k.to_string(), Value::Int(i));
            } else {
                errors.push(format!("line {}: bad value for {k}", n + 1));
            }
        }
        (opts, errors)
    }

    /// Loads from a file; a missing file gives the defaults.
    pub fn load(path: &Path) -> std::io::Result<Options> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                let (opts, errors) = Options::from_text(&text);
                for e in errors {
                    eprintln!("hwc-ui: {}: {e}", path.display());
                }
                Ok(opts)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Options::default()),
            Err(e) => Err(e),
        }
    }

    /// Writes atomically (temporary file, then rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("cfg.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    /// Imports the original `Cataclysm.cfg` (`name value` lines, `language=0` style also
    /// accepted), keeping only known integer options.
    pub fn import_original_cfg(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
                continue;
            }
            let mut parts = line.splitn(2, |c: char| c.is_whitespace() || c == '=');
            let (Some(k), Some(v)) = (parts.next(), parts.next()) else {
                continue;
            };
            let v = v.trim().trim_start_matches('=').trim();
            if DEFAULTS.iter().any(|&(d, _)| d == k) {
                if let Ok(i) = v.parse::<i32>() {
                    self.set(k, i);
                }
            } else if STRING_DEFAULTS.iter().any(|&(d, _)| d == k) {
                self.set_str(k, v);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recreation_options_default_on_and_round_trip() {
        let o = Options::default();
        assert!(o.get_bool("hwcFullDetailShips"));
        assert!(o.get_bool("hwcRightClickMove"));
        let (back, errors) = Options::from_text("hwcFullDetailShips = 0
hwcRightClickMove = 0
");
        assert!(errors.is_empty());
        assert!(!back.get_bool("hwcFullDetailShips"));
        assert!(!back.get_bool("hwcRightClickMove"));
    }

    #[test]
    fn round_trip() {
        let mut o = Options::default();
        o.set("MusicVolume", 12);
        o.set_str("PlayerName", "Kiith \"Somtaaw\" \\ 1");
        o.set("NewerKey", -3);
        let (back, errors) = Options::from_text(&o.to_text());
        assert!(errors.is_empty());
        assert_eq!(back, o);
        assert_eq!(back.get("MusicVolume"), 12);
        assert_eq!(back.get("SFXVolume"), 75);
    }

    #[test]
    fn bad_lines_are_reported() {
        let (o, errors) = Options::from_text("MusicVolume = loud\nnot a line\n# ok\nSFXVolume=3\n");
        assert_eq!(errors.len(), 2);
        assert_eq!(o.get("MusicVolume"), 60);
        assert_eq!(o.get("SFXVolume"), 3);
    }

    #[test]
    fn original_cfg_import() {
        let mut o = Options::default();
        o.import_original_cfg("language=2\nMusicVolume 40\nbogus 5\nPlayerName Bob\n");
        assert_eq!(o.get("language"), 2);
        assert_eq!(o.get("MusicVolume"), 40);
        assert_eq!(o.get("bogus"), 0);
        assert_eq!(o.get_str("PlayerName"), "Bob");
    }
}
