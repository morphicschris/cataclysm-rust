//! Player options and their persistence.
//!
//! The original keeps two stores (see `docs/frontend.md`): `Cataclysm.cfg`, a text file of
//! `name value` lines written from one table of tweakable variables and read back with the
//! tweak-script parser at start-up, and the registry key `HKLM\SOFTWARE\Sierra On-Line\Cataclysm`
//! for the display device and resolution. Both are written, with `keys.cfg`, by
//! `opOptionsSaveSettings` (through `utyOptionsFileWrite` and `kbSaveBindings`): at start-up,
//! when the front-end or in-game options are entered or accepted, and when a game ends
//! (`gameEnd`). This module keeps every one of those values under its original name in a single
//! file of our own format:
//!
//! ```text
//! # hwc options v1
//! MusicVolume = 45
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
    // A user-requested change: the music starts 25% quieter than the original's 60 (slider
    // level 0.615 -> 0.461). A value saved in the options file is kept.
    ("MusicVolume", 45),
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
    // 1: double-clicking one of the player's ships selects every ship of that type the
    // player owns on screen (a user-requested change); 0: the original's plain click.
    ("hwcDoubleClickSelectType", 1),
    // 1: in a formation attack the wingmen always fire at the leader's target (a
    // user-requested change); 0: the original, where they keep their own targets unless the
    // leader's is under no command (or the tactics are aggressive).
    ("hwcFormationFocusFire", 0),
    // 1: lines from the selected ships to what they were ordered to, with a circle there,
    // coloured by the order; 0: none, the original (`--order-lines`).
    ("hwcOrderLines", 1),
    // Class icons over the ships: 2 every ship, 1 the ships away from the camera (fading out
    // as they come close), 0 none, the original (`--ship-icons`).
    ("hwcShipIcons", 2),
    // 1: Tab cycles the ship icons and Shift+Tab is Toggle_Ship_Info; 0: Tab is the
    // original's Toggle_Ship_Info.
    ("hwcTabShipIcons", 1),
    // 1: a selected ship's health bar has a thin black outline; 0: the original bar
    // (`--health-bars`).
    ("hwcHealthBrackets", 1),
    // 1: the mouse wheel eases the zoom to each notch's distance; 0: the original's
    // immediate step per notch (`--zoom`).
    ("hwcSmoothZoom", 1),
    // 1: focusing again on the ships already in focus zooms in to frame them (the
    // original's autozoom); 0: a repeated focus does nothing (`--focus-zoom`).
    ("hwcFocusZoom", 1),
    // 1: the Alt focus recentres on the ships at the current zoom distance; 0: the
    // original's close-up on them (`--alt-focus`).
    ("hwcAltFocusKeepsZoom", 1),
    // 1: the closest zoom on focused ships also keeps the near clip plane outside their
    // hulls; 0: the original's limit alone (`--zoom-limit`).
    ("hwcZoomClear", 1),
    // 1: opening the sensors manager while the camera follows ships keeps the sensors camera
    // following them (until it is panned); 0: the original's still camera (`--sensors-follow`).
    ("hwcSensorsFollow", 1),
    // 1: the focus key in the sensors manager centres the sensors camera on the selection
    // and follows it; 0: the original, closing onto them (`--sensors-focus`).
    ("hwcSensorsFocusStays", 1),
    // 1: ships, effects and asteroids are drawn by the original's render list (within a
    // radial distance of the look-at point); 0: everything within the far clip distance
    // of the eye (`--draw-distance`).
    ("hwcRenderList", 1),
    // 1: ship deaths and engine trails use the Homeworld Remastered effects imported from the
    // player's own Remastered install (`hwc-import --hwr`; a user-requested addition); 0 or
    // nothing imported: the original's.
    ("hwcRemasteredFx", 0),
    // 1: engine trails curve smoothly through their recorded points (a user-requested change);
    // 0: the original's straight segments.
    ("hwcSmoothTrails", 1),
    // 1: capital-ship engine glows keep their size relative to the hull at every zoom (a
    // user-requested change); 0: the original's glows that grow with distance.
    ("hwcGlowsInProportion", 1),
    // 1: hulls drawn with the modern ship look, worked out from the game's own meshes and
    // textures (mipmapped textures, per-pixel light with a highlight, a bump from the
    // texture, smoothed facets; a user-requested change); 0: the original's per-vertex light.
    ("hwcShipLook", 1),
    // With the ship look, each a user-requested addition, 0 turning it off: 1 the ambient light
    // takes the background's colours; rounder hull outlines and baked ambient occlusion;
    // ships near the camera's focus cast shadows; bright colours roll off instead of clipping,
    // with dithering against banding.
    ("hwcSkyLight", 1),
    ("hwcShipDetail", 1),
    ("hwcShadows", 1),
    ("hwcToneMap", 1),
    // 1: a soft glow around bright things (engines, lit panels, weapons, explosions); 0 none.
    ("hwcBloom", 1),
    // 1: the picture shimmers around hot, bright spots; 0 none.
    ("hwcHeatHaze", 1),
    // The 3D view's internal render scale in percent, 50..200 (100: the original, drawn at
    // the screen's pixels; above 100 supersampled; `--render-scale`).
    ("hwcRenderScale", 100),
    // MSAA samples for the 3D view: 1 (off, the original), 2, 4 or 8 (`--msaa`).
    ("hwcMsaa", 1),
    // 1: FXAA over the 3D view (`--fxaa`).
    ("hwcFxaa", 0),
    // 1: in a scaled front-end mode (`--scale fit/integer/stretch`) the 3D view is drawn at
    // the window's pixels under the scaled interface; 0: at the 640x480 layout's pixels
    // (`--scene-res`).
    ("hwcSceneAtWindow", 1),
    // 1: the modern HUD (resources and selected ships top right, objectives and build queue
    // top left, system log bottom left); 0: the original's SU / RU / time box, objectives
    // list, construction list, messages and selected-ship list (`--hud`).
    ("hwcModernHud", 1),
    // The modern objectives panel: 1 expanded, 0 collapsed (its header toggles it).
    ("hwcObjectivesExpanded", 1),
    // The modern resources panel (RU / SU / time, top right): 1 expanded, 0 collapsed.
    ("hwcResourcesExpanded", 1),
    // The modern build queue (top left, under the objectives): 1 expanded, 0 collapsed.
    ("hwcBuildQueueExpanded", 1),
    // The modern research panel (top left, under the build queue): 1 expanded, 0 collapsed.
    ("hwcResearchExpanded", 1),
    // The modern system log (game messages, bottom left): 1 expanded, 0 collapsed.
    ("hwcSystemLogExpanded", 1),
    // The modern selected-ships panel (top right, under the resources): 1 expanded, 0
    // collapsed.
    ("hwcSelectedExpanded", 1),
    // 1: the build manager in the modern HUD's style (`--build-ui`); 0: the original screen.
    // The original HUD (`hwcModernHud = 0`, `--hud original`) brings back the original too.
    ("hwcModernBuildManager", 1),
    // 1: the research manager in the modern HUD's style (`--research-ui`); 0: the original
    // screen. The original HUD brings back the original too.
    ("hwcModernResearchManager", 1),
    // 1: the in-game escape menu, its options and confirmations, the objectives screen and
    // the systems (launch) manager in the modern HUD's style, with the modern options
    // screen (`--menus`); 0: the original screens. The original HUD brings back the
    // original too.
    ("hwcModernMenus", 1),
    // The screen mode (`--window` / `--borderless` / `--fullscreen`, `--display-mode`):
    // 0 a window at screenWidth x screenHeight, 1 a borderless window covering the monitor
    // at its own resolution (the default, a user-requested change), 2 exclusive full screen
    // at screenWidth x screenHeight (see `crate::display`).
    ("hwcDisplayMode", 1),
    // The full-screen refresh rate in millihertz; 0: the monitor's current rate.
    ("hwcRefreshRate", 0),
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
        let mut keys = std::collections::BTreeSet::new();
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
            keys.insert(k.to_string());
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
        // A file saved before the modern managers became the default (it has every key of
        // its day, the HUD's among them, but not the research manager's) holds the build
        // manager's old default, 0, which nobody chose: the new default applies.
        if keys.contains("hwcModernHud")
            && keys.contains("hwcModernBuildManager")
            && !keys.contains("hwcModernResearchManager")
        {
            opts.values
                .insert("hwcModernBuildManager".to_string(), Value::Int(1));
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
        write_atomic(path, &self.to_text())
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

/// Writes `text` to `path` through a temporary file beside it, renamed over the old one, so a
/// crash or a full disk never leaves half a file. The directory is made when missing.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = path.with_file_name(name);
    if let Err(e) = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path)) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_writes_replace_the_file_and_leave_no_temporary() {
        let dir = std::env::temp_dir().join(format!("hwc-atomic-{}", std::process::id()));
        let path = dir.join("sub").join("hwc-options.cfg");
        let mut o = Options::default();
        o.set("SFXVolume", 12);
        o.save(&path).unwrap();
        o.set("SFXVolume", 34);
        o.save(&path).unwrap();
        assert_eq!(Options::load(&path).unwrap().get("SFXVolume"), 34);
        assert!(!dir.join("sub").join("hwc-options.cfg.tmp").exists());
        write_atomic(
            &dir.join("keys.cfg"),
            "v 84
",
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.join("keys.cfg")).unwrap(),
            "v 84
"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn recreation_options_default_on_and_round_trip() {
        let o = Options::default();
        assert!(o.get_bool("hwcFullDetailShips"));
        assert!(o.get_bool("hwcRightClickMove"));
        let (back, errors) = Options::from_text(
            "hwcFullDetailShips = 0
hwcRightClickMove = 0
",
        );
        assert!(errors.is_empty());
        assert!(!back.get_bool("hwcFullDetailShips"));
        assert!(!back.get_bool("hwcRightClickMove"));
    }

    #[test]
    fn image_quality_options_default_to_the_original() {
        let o = Options::default();
        assert_eq!(o.get("hwcRenderScale"), 100);
        assert_eq!(o.get("hwcMsaa"), 1);
        assert_eq!(o.get("hwcFxaa"), 0);
        assert!(o.get_bool("hwcSmoothZoom"));
        assert!(o.get_bool("hwcSceneAtWindow"));
        let (back, errors) = Options::from_text(
            "hwcRenderScale = 150
hwcMsaa = 4
hwcFxaa = 1
hwcSmoothZoom = 0
",
        );
        assert!(errors.is_empty());
        assert_eq!(back.get("hwcRenderScale"), 150);
        assert_eq!(back.get("hwcMsaa"), 4);
        assert!(back.get_bool("hwcFxaa"));
        assert!(!back.get_bool("hwcSmoothZoom"));
    }

    #[test]
    fn camera_options_default_on() {
        let o = Options::default();
        for k in ["hwcFocusZoom", "hwcZoomClear", "hwcRenderList"] {
            assert!(o.get_bool(k), "{k}");
        }
        let (back, errors) =
            Options::from_text("hwcFocusZoom = 0\nhwcZoomClear = 0\nhwcRenderList = 0\n");
        assert!(errors.is_empty());
        for k in ["hwcFocusZoom", "hwcZoomClear", "hwcRenderList"] {
            assert!(!back.get_bool(k), "{k}");
        }
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

    /// The music starts 25% quieter than the original's 60 (a user-requested change); a value
    /// saved in the options file, or imported from `Cataclysm.cfg`, wins.
    #[test]
    fn music_volume_default_is_lowered_but_saved_values_are_kept() {
        assert_eq!(Options::default().get("MusicVolume"), 45);
        let (saved, _) = Options::from_text("MusicVolume = 60\n");
        assert_eq!(saved.get("MusicVolume"), 60);
        let (other, _) = Options::from_text("SFXVolume = 10\n");
        assert_eq!(other.get("MusicVolume"), 45);
        let mut imported = Options::default();
        imported.import_original_cfg("MusicVolume 80\n");
        assert_eq!(imported.get("MusicVolume"), 80);
    }

    #[test]
    fn bad_lines_are_reported() {
        let (o, errors) = Options::from_text("MusicVolume = loud\nnot a line\n# ok\nSFXVolume=3\n");
        assert_eq!(errors.len(), 2);
        assert_eq!(o.get("MusicVolume"), 45);
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

    #[test]
    fn modern_managers_default_on_even_in_older_saved_files() {
        let d = Options::default();
        assert!(d.get_bool("hwcModernBuildManager"));
        assert!(d.get_bool("hwcModernResearchManager"));
        // A file saved before the research manager's key existed: its 0 was the old default.
        let (old, _) =
            Options::from_text("hwcModernHud = 1\nhwcModernBuildManager = 0\nMusicVolume = 50\n");
        assert!(old.get_bool("hwcModernBuildManager"));
        assert!(old.get_bool("hwcModernResearchManager"));
        // Saved since then: the choice stands.
        let (new, _) = Options::from_text(
            "hwcModernHud = 1\nhwcModernBuildManager = 0\nhwcModernResearchManager = 0\n",
        );
        assert!(!new.get_bool("hwcModernBuildManager"));
        assert!(!new.get_bool("hwcModernResearchManager"));
        // A hand-written line alone is taken as it is.
        let (hand, _) = Options::from_text("hwcModernBuildManager = 0\n");
        assert!(!hand.get_bool("hwcModernBuildManager"));
        // Round trip.
        let (back, _) = Options::from_text(&new.to_text());
        assert!(!back.get_bool("hwcModernBuildManager"));
    }
}
