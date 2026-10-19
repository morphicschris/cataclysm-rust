//! The game's front-end callbacks: what the C functions bound to atom names do in Cataclysm
//! (main menu, single-player setup, options screens, language, credits). Behaviour follows the
//! retail executable's handlers (`uty*`, `op*`, `cp*`, `kb*`, `gp*`); things the reimplementation
//! cannot do yet (start a mission, play sounds, change video mode) are reported as
//! [`Request`]s for the application.

use std::path::PathBuf;

use crate::assets::Language;
use crate::canvas::{BlitMode, Color, Image, Rect, BLACK, WHITE};
use crate::colpick::ColorPicker;
use crate::consts::*;
use crate::credits::Credits;
use crate::fe::{Call, FrontEnd};
use crate::fer::Fer;
use crate::options::Options;
use crate::region::{AtomRef, RegionId};
use crate::text::{self, Shadow};

/// Something the front end asks the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    /// `QuitGame`: leave the program.
    Quit,
    /// A single-player game or the training mission would start here.
    StartGame { tutorial: bool, difficulty: i32 },
    /// Load a saved game.
    LoadGame(String),
    /// Multiplayer screens (not recreated yet).
    Multiplayer,
    /// Open a web page (troubleshooting link).
    OpenUrl(String),
    /// A UI sound event (`soundEventPlay(NULL, event)`); see [`crate::consts::sound`].
    SoundEvent(u32),
    /// A speech event (`speechEventQueue` with no ship, random variable and variation).
    Speech(u32),
    /// An audio option changed (volume, channel, quality or equaliser control, or a cancel
    /// that restored them): apply the audio options now.
    AudioSettings,
    /// Switch the display to this resolution (accepting a new mode, or reverting it).
    DisplayMode { width: i32, height: i32 },
    /// Options were accepted: apply audio/video settings, save the config.
    ApplyOptions,
}

/// Seconds the "keep this display mode?" box waits before restoring the old mode
/// (`opTimerExpired`).
pub const COUNTDOWN_SECONDS: f32 = 22.0;

/// The display-mode confirmation in progress (`opCountdownBoxStart` .. `opCountdownYes/No`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Countdown {
    pub elapsed: f32,
    /// The mode to go back to.
    pub previous: (i32, i32),
}

/// Front-end state of the game.
pub struct Cataclysm {
    pub options: Options,
    /// Settings when the options screens were entered (`opOptionsSaveSettings`).
    saved: Options,
    /// Restore point for the Advanced / Equalizer / Custom Effects sub-screens.
    sub_saved: Options,
    /// A custom-effects setting changed (`opEffectsToVideoAccept` then selects "Custom").
    custom_changed: bool,
    /// An equaliser band moved (the speaker preset becomes "Custom").
    eq_modified: bool,
    /// Training mode flag (`utySinglePlayerGameStart`'s tutorial override).
    tutorial: bool,
    pub requests: Vec<Request>,
    /// Text of the version user region.
    pub version: String,
    /// Font of the version number (`selFontName2`).
    pub version_font: String,
    pub renderers: Vec<String>,
    pub resolutions: Vec<(i32, i32)>,
    pub key_bindings: Vec<(String, String)>,
    pub saved_games: Vec<String>,
    pub training_games: Vec<String>,
    pub colors: ColorPicker,
    pub credits: Option<Credits>,
    /// Where the options are saved on accept.
    pub config_path: Option<PathBuf>,
    /// Display mode in use (what `screenWidth`/`screenHeight` were when last applied).
    pub display: (i32, i32),
    pub countdown: Option<Countdown>,
    /// `strOPCountdown0..2`: the confirmation box's three lines.
    pub countdown_text: [String; 3],
}

impl Cataclysm {
    pub fn new(options: Options) -> Self {
        let colors = ColorPicker::from_options(&options);
        Self {
            saved: options.clone(),
            sub_saved: options.clone(),
            options,
            custom_changed: false,
            eq_modified: false,
            tutorial: false,
            requests: Vec::new(),
            version: "Version CataclysmV1.00".into(),
            version_font: "TO_2.hff".into(),
            renderers: vec!["Direct3D: wgpu".into(), "OpenGL: wgpu".into()],
            resolutions: vec![
                (640, 480),
                (800, 600),
                (1024, 768),
                (1280, 960),
                (1600, 1200),
            ],
            key_bindings: Vec::new(),
            saved_games: Vec::new(),
            training_games: Vec::new(),
            colors,
            credits: None,
            config_path: None,
            display: (0, 0),
            countdown: None,
            countdown_text: [
                "Mode switch successful.".into(),
                "Do you want to keep this setting?".into(),
                "Original settings will restore in %ds.".into(),
            ],
        }
    }

    /// Reads the default key bindings (`default_keys.cfg`: `key <command> [SHIFT+]<code>`).
    pub fn load_key_bindings(&mut self, text: &str) {
        self.key_bindings = text
            .lines()
            .filter_map(|l| {
                let mut it = l.split_whitespace();
                (it.next()? == "key").then_some(())?;
                let name = it.next()?.replace('_', " ").to_uppercase();
                let code = it.next()?;
                Some((name, key_name(code)))
            })
            .collect();
    }

    /// Picks the version text and font from the game's string table and tweak script.
    pub fn load_strings(&mut self, strings: Option<&str>, tweak: Option<&str>) {
        let word = strings
            .and_then(|s| script_value(s, "strVersion"))
            .unwrap_or_else(|| "Version".into());
        self.version = format!("{word} CataclysmV1.00");
        for (i, line) in self.countdown_text.iter_mut().enumerate() {
            if let Some(t) = strings.and_then(|s| script_value(s, &format!("strOPCountdown{i}"))) {
                *line = t;
            }
        }
        if let Some(f) = tweak.and_then(|t| script_value(t, "selFontName2")) {
            self.version_font = f;
        }
    }

    /// Saves the options if a config path is set.
    pub fn save_options(&mut self) {
        self.colors.store(&mut self.options);
        if let Some(path) = &self.config_path {
            if let Err(e) = self.options.save(path) {
                eprintln!("hwc-ui: saving {}: {e}", path.display());
            }
        }
    }
}

/// Second whitespace-separated field of the line whose first field is `key`.
fn script_value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.split(';').next()?.trim();
        let rest = l.strip_prefix(key)?;
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        Some(rest.trim().to_string())
    })
}

/// Readable key name for a `default_keys.cfg` code (`SHIFT+83` -> `SHIFT+S`).
fn key_name(code: &str) -> String {
    let (prefix, num) = match code.rsplit_once('+') {
        Some((p, n)) => (format!("{p}+"), n),
        None => (String::new(), code),
    };
    let name = match num.parse::<u32>() {
        Ok(n @ 0x30..=0x39) | Ok(n @ 0x41..=0x5a) => {
            char::from_u32(n).map(String::from).unwrap_or_default()
        }
        Ok(n @ 0x70..=0x7b) => format!("F{}", n - 0x6f),
        Ok(0x20) => "SPACE".into(),
        Ok(0x09) => "TAB".into(),
        Ok(0x1b) => "ESC".into(),
        Ok(0x0d) => "ENTER".into(),
        Ok(0xc0) => "`".into(),
        Ok(n) => format!("#{n}"),
        Err(_) => num.to_string(),
    };
    format!("{prefix}{name}")
}

type Fe = FrontEnd<Cataclysm>;

/// Registers every Cataclysm front-end callback, then logging stubs for the names the `.fib`
/// files use that have no implementation.
pub fn register(fe: &mut Fe) {
    // Main menu.
    fe.callback_add("SinglePlayerOptions", single_player_options);
    fe.callback_add("SinglePlayerGame", single_player_game);
    fe.callback_add("FE_Multiplayer_Game", multiplayer_start);
    fe.callback_add("MG_ConnectionBack", |fe, _| {
        fe.all_screens_delete();
        fe.screen_start("Main_game_screen");
    });
    fe.callback_add("OP_Options_Start", options_start);
    fe.callback_add("FE_HideMenuOrGotoQuitGame", |fe, _| {
        let base = fe.current_base();
        fe.screen_start_from(base, "Quit_game");
    });
    fe.callback_add("FE_SHOWCREDITS", |fe, _| {
        let res = fe.resources().clone();
        fe.game.credits = Credits::start(&res, "Credits-button.cred");
        if fe.game.credits.is_none() {
            fe.log_once("credits: Credits\\Credits-button.cred unavailable".into());
        }
    });
    fe.callback_add("QuitGame", |fe, _| fe.game.requests.push(Request::Quit));
    fe.draw_callback_add("FE_DrawVersionNum", draw_version);

    // Training and single player.
    fe.callback_add("Tutorial1", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.tutorial = true;
        start_game(fe, "Tutorial1");
    });
    fe.callback_add("StartSinglePlayerGame", |fe, call| {
        start_game(fe, &call.name)
    });
    fe.callback_add("TA_StartTutorial", |fe, _| {
        fe.game.tutorial = true;
        start_game(fe, "Tutorial1");
    });
    fe.callback_add("TA_StartSinglePlayer", |fe, _| {
        fe.game.tutorial = true;
        fe.game.options.set("TutorialNeeded", 0);
        start_game(fe, "TA_StartSinglePlayer");
    });
    fe.callback_add("TutorialAlertCancel", |fe, _| {
        fe.all_screens_delete();
        fe.screen_start("Main_game_screen");
    });
    fe.callback_add("SP_GameDifficulty", |fe, call| {
        radio_option(fe, call, "DifficultySetting")
    });
    fe.callback_add("FE_GameWindowInit", |fe, call| game_list(fe, call, false));
    fe.callback_add("FE_TutorialGameWindowInit", |fe, call| {
        game_list(fe, call, true)
    });
    fe.callback_add("LoadMultiSinglePlayerGameFromMain", load_game);
    fe.callback_add("SelectSinglePlayerGame", |fe, _| {
        fe.screen_start("Load_game");
    });
    crate::colpick::register(fe);

    // Options: navigation.
    fe.callback_add("OP_Options_Accept", options_accept);
    fe.callback_add("OP_Options_Cancel", |fe, _| {
        fe.game.options = fe.game.saved.clone();
        fe.current_screen_delete();
        fe.screen_start("Main_game_screen");
        fe.game.requests.push(Request::AudioSettings);
    });
    fe.callback_add("OP_Countdown_Yes", |fe, _| {
        fe.game.countdown = None;
        fe.current_screen_delete();
    });
    fe.callback_add("OP_Countdown_No", |fe, _| {
        if let Some(c) = fe.game.countdown.take() {
            fe.current_screen_delete();
            revert_display_mode(fe, c.previous);
        }
    });
    fe.draw_callback_add("OP_DrawCountdownBox", draw_countdown_box);
    fe.callback_add("OP_Advanced_To_Audio_Accept", |fe, _| {
        snapshot(&mut fe.game, ADVANCED_KEYS);
        back_to(fe, "Audio_Options");
    });
    fe.callback_add("OP_Advanced_To_Audio_Cancel", |fe, _| {
        restore(&mut fe.game, ADVANCED_KEYS);
        back_to(fe, "Audio_Options");
        fe.game.requests.push(Request::AudioSettings);
    });
    fe.callback_add("OP_Equalizer_To_Audio_Accept", |fe, _| {
        if fe.game.eq_modified {
            fe.game.options.set("speakerSetting", 3);
        }
        snapshot(&mut fe.game, EQ_KEYS);
        back_to(fe, "Audio_Options");
    });
    fe.callback_add("OP_Equalizer_To_Audio_Cancel", |fe, _| {
        restore(&mut fe.game, EQ_KEYS);
        back_to(fe, "Audio_Options");
        fe.game.requests.push(Request::AudioSettings);
    });
    fe.callback_add("OP_Effects_To_Video_Accept", |fe, _| {
        if fe.game.custom_changed {
            fe.game.options.set("effectsLevel", 2);
        }
        snapshot(&mut fe.game, EFFECT_KEYS);
        back_to(fe, "Video_Options");
    });
    fe.callback_add("OP_Effects_To_Video_Cancel", |fe, _| {
        restore(&mut fe.game, EFFECT_KEYS);
        back_to(fe, "Video_Options");
    });

    // Options: controls bound to option values.
    for b in CHECKS {
        fe.callback_add(b.name, check_option);
    }
    for s in SLIDERS {
        fe.callback_add(s.name, slider_option);
    }
    fe.slider_handler_add("option", slider_changed);
    for (name, key) in RADIOS {
        fe.callback_add(name, |fe, call| {
            let key = RADIOS.iter().find(|r| r.0 == call.name).map_or("", |r| r.1);
            radio_option(fe, call, key);
        });
        let _ = key;
    }
    fe.callback_add("OP_Speaker", speaker);
    fe.callback_add("OP_Effects", effects_level);
    fe.callback_add("OP_CataclysmFramerate", framerate);
    fe.callback_add("VO_Language", language);
    fe.callback_add("OP_NoLOD", no_lod);
    fe.callback_add("OP_SmallFonts", |fe, call| {
        if fe.first_call(call) {
            let v = !fe.game.options.get_bool("LargePopupMenuFonts");
            fe.toggle_set(&call.name, v);
        } else if let Some(a) = call.atom {
            let v = !fe.atom(a).checked();
            fe.game.options.set_bool("LargePopupMenuFonts", v);
        }
    });
    fe.callback_add("OP_RightClickCSA", |fe, call| {
        csa(
            fe,
            call,
            "RightClick_CSA",
            "Homeworld_CSA",
            "OP_HomeworldCSA",
        )
    });
    fe.callback_add("OP_HomeworldCSA", |fe, call| {
        csa(
            fe,
            call,
            "Homeworld_CSA",
            "RightClick_CSA",
            "OP_RightClickCSA",
        )
    });
    fe.callback_add("OP_Resources", |fe, call| {
        if fe.first_call(call) {
            let v = fe.game.options.get_bool("ShowResources")
                && fe.game.options.get_bool("ShowMeteors");
            fe.toggle_set(&call.name, v);
        } else if let Some(a) = call.atom {
            let v = fe.atom(a).checked();
            fe.game.options.set_bool("ShowResources", v);
            fe.game.options.set_bool("ShowMeteors", v);
        }
    });
    fe.callback_add("OP_HomeworldClassic", |fe, _| gameplay_preset(fe, true));
    fe.callback_add("OP_CataclysmDefault", |fe, _| gameplay_preset(fe, false));
    fe.callback_add("OP_Render", render_list);
    fe.callback_add("OP_Resolution", resolution_list);
    fe.callback_add("OP_KeyBindingsList", key_list);
    fe.callback_add("OP_KeyBindingsPoolList", key_list);
    fe.list_drawer_add("OP_KeyBindingsList", key_item_draw);
    fe.list_drawer_add("OP_KeyBindingsPoolList", key_item_draw);
    fe.callback_add("OP_KeyResetToDefault", |fe, _| {
        fe.log_once("key bindings are read-only (default_keys.cfg)".into())
    });
    fe.callback_add("OP_TroubleShoot", |fe, _| {
        fe.game.requests.push(Request::OpenUrl(
            "http://homeworld.sierra.com/cataclysm/support/".into(),
        ))
    });
    fe.callback_add("OP_Speech_Test", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            fe.game.requests.push(Request::Speech(sound::SPEECH_TEST));
        }
    });
    fe.callback_add("OP_SoundFX_Test", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            fe.ui_sound(sound::SFX_TEST);
        }
    });
    fe.draw_callback_add("OP_NumChannels", draw_num_channels);

    register_stubs(fe);
}

/// Logging stubs for every known callback name without an implementation.
pub fn register_stubs<G>(fe: &mut FrontEnd<G>) {
    for &(name, _) in crate::known::FUNCTIONS {
        if !fe.has_callback(name) {
            fe.callback_add(name, stub_function::<G>);
        }
    }
    for &(name, _) in crate::known::DRAW_FUNCTIONS {
        if !fe.has_draw_callback(name) {
            fe.draw_callback_add(name, stub_draw::<G>);
        }
    }
}

fn stub_function<G>(fe: &mut FrontEnd<G>, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let files = crate::known::FUNCTIONS
        .iter()
        .find(|f| f.0 == call.name)
        .map(|f| f.1.join(", "))
        .unwrap_or_default();
    fe.log_once(format!(
        "callback '{}' ({files}) is not implemented",
        call.name
    ));
}

fn stub_draw<G>(fe: &mut FrontEnd<G>, _fer: &mut Fer, atom: AtomRef, _rect: Rect) {
    let name = fe.atom(atom).name.clone().unwrap_or_default();
    fe.log_once(format!("draw callback '{name}' is not implemented"));
}

// ---------------------------------------------------------------------------------------------
// Main menu and single player
// ---------------------------------------------------------------------------------------------

/// `utySinglePlayerOptions`.
fn single_player_options(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) {
        return;
    }
    fe.current_screen_delete();
    fe.screen_start("Create_new_game");
    fe.game.options.set("PlayerRace", 0);
}

/// `mgStartMultiPlayerGameScreens` (navigation only): the connection-method screen of
/// `multiplayer_game.fib`, whose actions are logging stubs apart from Back.
fn multiplayer_start(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) {
        return;
    }
    if fe.find_screen("Connection_Method").is_none() {
        let data = fe.resources().read("FEMan/Multiplayer_Game.fib");
        match data.map(|d| fe.load_fib_data(&d)) {
            Some(Ok(())) => {}
            _ => {
                fe.log_once("multiplayer_game.fib unavailable".into());
                return;
            }
        }
    }
    fe.game.requests.push(Request::Multiplayer);
    fe.all_screens_delete();
    fe.screen_start("Connection_Method");
}

/// `utySinglePlayerGame`.
fn single_player_game(fe: &mut Fe, _call: &Call) {
    fe.current_screen_delete();
    fe.screen_start("Create_new_game");
}

/// `opOptionsStart`.
fn options_start(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) {
        return;
    }
    fe.game.saved = fe.game.options.clone();
    fe.game.sub_saved = fe.game.options.clone();
    fe.current_screen_delete();
    fe.screen_start("Audio_Options");
}

/// `opOptionsAccept`: back to the main menu; a new resolution is switched to and must be
/// confirmed in the countdown box, otherwise the old one comes back.
fn options_accept(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    fe.current_screen_delete();
    fe.screen_start("Main_game_screen");
    fe.game.saved = fe.game.options.clone();
    fe.game.save_options();
    fe.game.requests.push(Request::ApplyOptions);
    let mode = (
        fe.game.options.get("screenWidth"),
        fe.game.options.get("screenHeight"),
    );
    if mode != fe.game.display && fe.game.resolutions.contains(&mode) {
        let previous = fe.game.display;
        fe.game.display = mode;
        fe.game.requests.push(Request::DisplayMode {
            width: mode.0,
            height: mode.1,
        });
        fe.game.countdown = Some(Countdown {
            elapsed: 0.0,
            previous,
        });
        fe.screen_start("Countdown_Message_Box");
    }
}

/// `opCountdownNo` / `opTimerExpired`: restore the previous display mode.
fn revert_display_mode(fe: &mut Fe, previous: (i32, i32)) {
    fe.game.display = previous;
    fe.game.options.set("screenWidth", previous.0);
    fe.game.options.set("screenHeight", previous.1);
    fe.game.saved = fe.game.options.clone();
    fe.game.save_options();
    fe.game.requests.push(Request::DisplayMode {
        width: previous.0,
        height: previous.1,
    });
}

/// Per-frame work of the game's front-end callbacks: the display-mode countdown
/// (`opTimerExpired` drops every screen and restarts the main menu).
pub fn tick(fe: &mut Fe, dt: f32) {
    let Some(c) = &mut fe.game.countdown else {
        return;
    };
    c.elapsed += dt;
    if c.elapsed > COUNTDOWN_SECONDS {
        let previous = c.previous;
        fe.game.countdown = None;
        fe.all_screens_delete();
        fe.screen_start("Main_game_screen");
        revert_display_mode(fe, previous);
    }
}

/// `opCountdownBoxDraw`: three centred lines in `default.hff`. The seconds shown are the
/// timeout less two, a constant (the engine never counts it down).
fn draw_countdown_box(fe: &mut Fe, fer: &mut Fer, _atom: AtomRef, rect: Rect) {
    let Some(font) = fer.res.font("default.hff") else {
        return;
    };
    let colour = [255, 200, 0, 255];
    let h = text::height(&font);
    let seconds = COUNTDOWN_SECONDS as i32 - 2;
    for (i, line) in fe.game.countdown_text.iter().enumerate() {
        let line = line.replace("%d", &seconds.to_string());
        let t = text::to_latin1(&line);
        let x = rect.x0 + (rect.width() - text::width(&font, &t)) / 2;
        let y = rect.y0 + h * (i as i32 + 1);
        text::print(fer.canvas, &font, x, y, colour, &t, (Shadow::None, BLACK));
    }
}

/// An audio option changed: the application applies them right away, like the `op*Process`
/// handlers that call `soundEvent*Vol`, `soundSetNumVoices`, `soundMixerSetMode` and
/// `soundEventMasterEQ` as the controls move.
fn audio_changed(fe: &mut Fe) {
    fe.game.requests.push(Request::AudioSettings);
}

/// Option keys whose controls take effect on the sound system immediately.
const AUDIO_KEYS: &[&str] = &[
    "SpeechVolume",
    "SFXVolume",
    "MusicVolume",
    "NumChannels",
    "AutoChannel",
    "SoundQuality",
    "speakerSetting",
    "Equalizer0",
    "Equalizer1",
    "Equalizer2",
    "Equalizer3",
    "Equalizer4",
    "Equalizer5",
    "Equalizer6",
    "Equalizer7",
];

fn back_to(fe: &mut Fe, screen: &str) {
    fe.current_screen_delete();
    fe.screen_start(screen);
}

/// `utySinglePlayerGameStart`: new players are sent to the training alert first.
fn start_game(fe: &mut Fe, name: &str) {
    fe.game.colors.accept(&mut fe.game.options);
    let tutorial = name == "Tutorial1";
    if !tutorial
        && !fe.game.tutorial
        && fe.game.training_games.is_empty()
        && fe.game.options.get_bool("TutorialNeeded")
    {
        fe.screen_start("Tutorial_Alert");
        return;
    }
    fe.all_screens_delete();
    fe.game.tutorial = false;
    let difficulty = fe.game.options.get("DifficultySetting");
    fe.game.requests.push(Request::StartGame {
        tutorial,
        difficulty,
    });
}

/// `gpGameWindowInit`: the saved-game list.
fn game_list(fe: &mut Fe, call: &Call, training: bool) {
    let Some(a) = call.atom else { return };
    if !fe.first_call(call) {
        return;
    }
    fe.list_init(a, "Arial_12.hff", 18);
    let games = if training {
        fe.game.training_games.clone()
    } else {
        fe.game.saved_games.clone()
    };
    if let Some(l) = fe.list_mut(a) {
        for (i, g) in games.iter().enumerate() {
            l.add(g.clone(), i as u32, i == 0);
        }
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// `utyLoadMultiSinglePlayerGame`: load the selected saved game.
fn load_game(fe: &mut Fe, _call: &Call) {
    let selected = ["FE_GameWindowInit", "FE_TutorialGameWindowInit"]
        .iter()
        .find_map(|n| {
            let a = fe.find_atom(n)?;
            let l = fe.list(a)?;
            l.selected
                .and_then(|i| l.items.get(i))
                .map(|item| item.text.clone())
        });
    match selected {
        Some(name) => {
            fe.all_screens_delete();
            fe.game.requests.push(Request::LoadGame(name));
        }
        None => fe.log_once("no saved game selected".into()),
    }
}

/// `versionNumDraw`: the build string on a black strip.
fn draw_version(fe: &mut Fe, fer: &mut Fer, _atom: AtomRef, rect: Rect) {
    fer.canvas.fill_rect(rect, BLACK);
    let Some(font) = fer.res.font(&fe.game.version_font) else {
        return;
    };
    let y = rect.y0 + text::height(&font);
    let t = text::to_latin1(&fe.game.version);
    text::print(
        fer.canvas,
        &font,
        rect.x0,
        y,
        WHITE,
        &t,
        (Shadow::None, BLACK),
    );
}

// ---------------------------------------------------------------------------------------------
// Options
// ---------------------------------------------------------------------------------------------

/// A check box (or toggle / bitmap button) bound to one option.
struct CheckBinding {
    name: &'static str,
    key: &'static str,
    /// The control mirroring this one (a custom-effects bitmap button and its check box).
    partner: Option<&'static str>,
    /// Touching it makes the effects level "Custom".
    custom: bool,
}

const fn chk(name: &'static str, key: &'static str) -> CheckBinding {
    CheckBinding {
        name,
        key,
        partner: None,
        custom: false,
    }
}

const fn fx(
    name: &'static str,
    key: &'static str,
    partner: &'static str,
    custom: bool,
) -> CheckBinding {
    CheckBinding {
        name,
        key,
        partner: Some(partner),
        custom,
    }
}

const CHECKS: &[CheckBinding] = &[
    chk("OP_AutoChannels", "AutoChannel"),
    chk("OP_Voice_Commands", "VoiceCommands"),
    chk("OP_Voice_Status", "VoiceStatus"),
    chk("OP_Voice_Chatter", "VoiceChatter"),
    chk("OP_AreaExplored", "ShowAreaVisited"),
    chk("OP_HWTaskbar", "HomeworldTaskbar"),
    chk("OP_TaskbarButtons", "ShowTaskbarButtons"),
    chk("OP_TaskbarBackdrop", "ShowTaskbarBackdrop"),
    chk("OP_ExperienceMessages", "ExperienceMessages"),
    chk("OP_AutoFocusOnWaypoints", "AutoFocusOnWaypoints"),
    chk("OP_SensorsSphere", "ShowFOWBlobs"),
    chk("OP_SensorsWireframe", "ShowAreaOccupied"),
    chk("OP_Dust", "ShowAsteriod0"),
    chk("OP_Background", "ShowBackground"),
    chk("OP_Compass", "ShowCompass"),
    chk("OP_ObjectivePointers", "ShowObjectivesPointer"),
    chk("OP_SlipgatePaths", "ShowSlipgatePaths"),
    chk("OP_CapitalShipOverlay", "ShowIcons"),
    fx("VO_Filter", "texLinearFiltering", "VO_BitmapFilter", true),
    fx("VO_BitmapFilter", "texLinearFiltering", "VO_Filter", false),
    fx(
        "VO_Background",
        "showBackgrounds",
        "VO_BitmapBackground",
        true,
    ),
    fx(
        "VO_BitmapBackground",
        "showBackgrounds",
        "VO_Background",
        false,
    ),
    fx(
        "VO_InstantTransition",
        "InstantTransition",
        "VO_BitmapInstantTransition",
        true,
    ),
    fx(
        "VO_BitmapInstantTransition",
        "InstantTransition",
        "VO_InstantTransition",
        false,
    ),
    fx("VO_Stipple", "enableStipple", "VO_BitmapStipple", true),
    fx("VO_BitmapStipple", "enableStipple", "VO_Stipple", false),
    fx(
        "VO_SensorsBlobs",
        "fuzzyBlobs",
        "VO_BitmapSensorsBlobs",
        true,
    ),
    fx(
        "VO_BitmapSensorsBlobs",
        "fuzzyBlobs",
        "VO_SensorsBlobs",
        false,
    ),
    fx(
        "VO_Damage",
        "etgDamageEffectsEnabled",
        "VO_BitmapDamage",
        true,
    ),
    fx(
        "VO_BitmapDamage",
        "etgDamageEffectsEnabled",
        "VO_Damage",
        false,
    ),
    fx(
        "VO_Bullet",
        "etgBulletEffectsEnabled",
        "VO_BitmapBullet",
        true,
    ),
    fx(
        "VO_BitmapBullet",
        "etgBulletEffectsEnabled",
        "VO_Bullet",
        false,
    ),
    fx(
        "VO_Muzzle",
        "etgFireEffectsEnabled",
        "VO_BitmapMuzzle",
        true,
    ),
    fx(
        "VO_BitmapMuzzle",
        "etgFireEffectsEnabled",
        "VO_Muzzle",
        false,
    ),
    fx(
        "VO_Hit",
        "etgHitEffectsEnabled",
        "VO_BitmapHitEffects",
        true,
    ),
    fx(
        "VO_BitmapHitEffects",
        "etgHitEffectsEnabled",
        "VO_Hit",
        false,
    ),
    fx("VO_DynamicCPV", "enableCPV", "VO_BitmapDynamicCPV", true),
    fx("VO_BitmapDynamicCPV", "enableCPV", "VO_DynamicCPV", false),
    fx(
        "VO_LensFlare",
        "enableLensFlare",
        "VO_BitmapLensFlare",
        true,
    ),
    fx(
        "VO_BitmapLensFlare",
        "enableLensFlare",
        "VO_LensFlare",
        false,
    ),
    fx(
        "VO_Repairbots",
        "enableRepairbots",
        "VO_BitmapRepairbots",
        true,
    ),
    fx(
        "VO_BitmapRepairbots",
        "enableRepairbots",
        "VO_Repairbots",
        false,
    ),
];

/// The usual option check box: shows the value when the screen opens, stores it when clicked.
fn check_option(fe: &mut Fe, call: &Call) {
    let Some(b) = CHECKS.iter().find(|b| b.name == call.name) else {
        return;
    };
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.options.get_bool(b.key);
        fe.toggle_set(b.name, v);
        if b.custom {
            fe.game.custom_changed = true;
        }
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let v = fe.atom(a).checked();
    fe.game.options.set_bool(b.key, v);
    if AUDIO_KEYS.contains(&b.key) {
        audio_changed(fe);
    }
    if let Some(p) = b.partner {
        fe.toggle_set(p, v);
    }
    if b.custom {
        fe.game.custom_changed = true;
    }
}

/// How a slider maps to its option.
#[derive(Clone, Copy)]
enum SliderMap {
    Direct,
    /// `30 - value` (model detail threshold).
    Inverted30,
    /// Effect history scalar above its 64 minimum.
    History,
    /// An equaliser band, stored as value * 1.025.
    Eq,
}

struct SliderBinding {
    name: &'static str,
    key: &'static str,
    max: u32,
    map: SliderMap,
}

const fn sld(name: &'static str, key: &'static str, max: u32, map: SliderMap) -> SliderBinding {
    SliderBinding {
        name,
        key,
        max,
        map,
    }
}

/// Lowest effect history scalar (`etgHistoryScalarMin`).
const HISTORY_MIN: i32 = 64;
/// Hardware voices below the channel slider's range (`soundGetVoiceLimits`: 8..32).
const VOICE_MIN: i32 = 8;
const VOICE_MAX: i32 = 32;

const SLIDERS: &[SliderBinding] = &[
    sld("OP_Speech_Volume", "SpeechVolume", 100, SliderMap::Direct),
    sld("OP_SFX_Volume", "SFXVolume", 100, SliderMap::Direct),
    sld("OP_Music_Volume", "MusicVolume", 100, SliderMap::Direct),
    sld(
        "OP_Num_Channels",
        "NumChannels",
        (VOICE_MAX - VOICE_MIN) as u32,
        SliderMap::Direct,
    ),
    sld("OP_Brightness", "brightnessVal", 70, SliderMap::Direct),
    sld(
        "OP_Detail_Threshold",
        "detailThreshold",
        21,
        SliderMap::Inverted30,
    ),
    sld("OP_Texture_Detail", "TextureDetail", 100, SliderMap::Direct),
    sld(
        "OP_NumEffects",
        "etgHistoryScalar",
        (256 - HISTORY_MIN) as u32,
        SliderMap::History,
    ),
    sld(
        "OP_Mouse_Sensitivity",
        "opMouseSens",
        100,
        SliderMap::Direct,
    ),
    sld(
        "OP_BattleChatter",
        "opBattleChatter",
        100,
        SliderMap::Direct,
    ),
    sld("OP_Equalizer1", "Equalizer0", 100, SliderMap::Eq),
    sld("OP_Equalizer2", "Equalizer1", 100, SliderMap::Eq),
    sld("OP_Equalizer3", "Equalizer2", 100, SliderMap::Eq),
    sld("OP_Equalizer4", "Equalizer3", 100, SliderMap::Eq),
    sld("OP_Equalizer5", "Equalizer4", 100, SliderMap::Eq),
    sld("OP_Equalizer6", "Equalizer5", 100, SliderMap::Eq),
    sld("OP_Equalizer7", "Equalizer6", 100, SliderMap::Eq),
    sld("OP_Equalizer8", "Equalizer7", 100, SliderMap::Eq),
];

fn slider_value(o: &Options, b: &SliderBinding) -> u32 {
    let v = match b.map {
        SliderMap::Direct | SliderMap::Eq => o.get(b.key),
        SliderMap::Inverted30 => 30 - o.get(b.key),
        SliderMap::History => o.get(b.key) - HISTORY_MIN,
    };
    v.max(0) as u32
}

/// On screen creation: range, value, and the value handler (the `op*Process` functions).
fn slider_option(fe: &mut Fe, call: &Call) {
    let Some(b) = SLIDERS.iter().find(|b| b.name == call.name) else {
        return;
    };
    let Some(a) = call.atom else { return };
    if !fe.first_call(call) {
        return;
    }
    let v = slider_value(&fe.game.options, b);
    fe.slider_setup(a, b.max, v, Some("option"));
    if b.name == "OP_Detail_Threshold" {
        let no_lod = fe.game.options.get_bool("noLOD");
        fe.set_disabled(a, no_lod);
    }
    if matches!(b.map, SliderMap::Eq) {
        fe.game.eq_modified = false;
    }
}

/// A bound slider moved.
fn slider_changed(fe: &mut Fe, region: RegionId) {
    let Some(a) = fe.region(region).and_then(|r| r.atom) else {
        return;
    };
    let Some(name) = fe.atom(a).name.clone() else {
        return;
    };
    let Some(b) = SLIDERS.iter().find(|b| b.name == name) else {
        return;
    };
    let v = fe.slider(a).map_or(0, |s| s.value) as i32;
    match b.map {
        SliderMap::Direct => fe.game.options.set(b.key, v),
        SliderMap::Inverted30 => fe.game.options.set(b.key, 30 - v),
        SliderMap::Eq => {
            fe.game.options.set(b.key, (v as f32 * 1.025) as i32);
            fe.game.eq_modified = true;
        }
        SliderMap::History => {
            // `opNumEffectsProcess`: moving it off a preset's value selects "Custom".
            let level = fe.game.options.get("effectsLevel");
            let max = 256 - HISTORY_MIN;
            if (level == 0 && v != max) || (level == 1 && v != 0) {
                fe.game.options.set("effectsLevel", 2);
                fe.radio_set("OP_Effects", 2);
            }
            fe.game.options.set(b.key, v + HISTORY_MIN);
        }
    }
    if AUDIO_KEYS.contains(&b.key) {
        audio_changed(fe);
    }
}

/// Radio groups bound to one option.
const RADIOS: &[(&str, &str)] = &[
    ("OP_DCTQuality", "SoundQuality"),
    ("OP_LODDetail", "DetailQuality"),
];

/// The usual option radio group.
fn radio_option(fe: &mut Fe, call: &Call, key: &str) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.options.get(key);
        fe.radio_set(&call.name, v.max(0) as u32);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let v = fe.atom(a).radio_value as i32;
    fe.game.options.set(key, v);
    if AUDIO_KEYS.contains(&key) {
        audio_changed(fe);
    }
}

/// Equaliser presets of the speaker types (multimedia, stereo/surround, headphones).
const SPEAKER_EQ: [[i32; 8]; 3] = [
    [100, 66, 33, 0, 0, 0, 0, 0],
    [0; 8],
    [0, 17, 33, 50, 50, 33, 17, 0],
];

/// `opSpeaker`.
fn speaker(fe: &mut Fe, call: &Call) {
    radio_option(fe, call, "speakerSetting");
    let s = fe.game.options.get("speakerSetting");
    if (0..3).contains(&s) {
        for (i, v) in SPEAKER_EQ[s as usize].iter().enumerate() {
            fe.game.options.set(&format!("Equalizer{i}"), *v);
        }
    } else {
        fe.game.options.set("speakerSetting", 3);
    }
}

/// `opEffects` + `opUpdateVideoSettings`: High and Low are presets, Custom keeps the values.
fn effects_level(fe: &mut Fe, call: &Call) {
    radio_option(fe, call, "effectsLevel");
    apply_effects_preset(&mut fe.game.options);
    let history = fe.game.options.get("etgHistoryScalar");
    if let Some(a) = fe.find_atom("OP_NumEffects") {
        if let Some(s) = fe.slider_mut(a) {
            s.value = (history - HISTORY_MIN).max(0) as u32;
        }
    }
}

fn apply_effects_preset(o: &mut Options) {
    let level = o.get("effectsLevel");
    let set = |o: &mut Options, keys: &[&str], v: bool| keys.iter().for_each(|k| o.set_bool(k, v));
    match level {
        0 => {
            set(
                o,
                &[
                    "texLinearFiltering",
                    "showBackgrounds",
                    "enableSmoothing",
                    "enableCPV",
                    "enableLensFlare",
                    "enableRepairbots",
                    "fuzzyBlobs",
                    "enableTrails",
                    "etgHitEffectsEnabled",
                    "etgDamageEffectsEnabled",
                    "etgFireEffectsEnabled",
                    "etgBulletEffectsEnabled",
                ],
                true,
            );
            set(o, &["enableStipple", "InstantTransition"], false);
            o.set("etgHistoryScalar", 256);
        }
        1 => {
            set(
                o,
                &[
                    "enableSmoothing",
                    "enableStipple",
                    "enableTrails",
                    "InstantTransition",
                ],
                true,
            );
            set(
                o,
                &[
                    "texLinearFiltering",
                    "showBackgrounds",
                    "enableCPV",
                    "enableLensFlare",
                    "enableRepairbots",
                    "fuzzyBlobs",
                    "etgHitEffectsEnabled",
                    "etgFireEffectsEnabled",
                ],
                false,
            );
            o.set("etgHistoryScalar", HISTORY_MIN);
        }
        2 => {}
        _ => o.set("effectsLevel", 2),
    }
}

/// `opNoLOD`: full detail disables the detail threshold slider.
fn no_lod(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.options.get_bool("noLOD");
        fe.toggle_set(&call.name, v);
    } else if !fe.last_call(call) {
        let v = fe.atom(a).checked();
        fe.game.options.set_bool("noLOD", v);
    }
    let v = fe.game.options.get_bool("noLOD");
    if let Some(s) = fe.find_atom("OP_Detail_Threshold") {
        fe.set_disabled(s, v);
    }
}

/// The two context-sensitive-action check boxes exclude each other.
fn csa(fe: &mut Fe, call: &Call, key: &str, other_key: &str, other: &str) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.options.get_bool(key);
        fe.toggle_set(&call.name, v);
        return;
    }
    let v = fe.atom(a).checked();
    fe.game.options.set_bool(key, v);
    fe.game.options.set_bool(other_key, !v);
    fe.toggle_set(other, !v);
}

/// Gameplay check boxes and their option keys, refreshed by the presets.
const GAMEPLAY_TOGGLES: &[(&str, &str)] = &[
    ("OP_HWTaskbar", "HomeworldTaskbar"),
    ("OP_TaskbarButtons", "ShowTaskbarButtons"),
    ("OP_TaskbarBackdrop", "ShowTaskbarBackdrop"),
    ("OP_ExperienceMessages", "ExperienceMessages"),
    ("OP_HomeworldCSA", "Homeworld_CSA"),
    ("OP_RightClickCSA", "RightClick_CSA"),
    ("OP_AreaExplored", "ShowAreaVisited"),
    ("OP_SensorsSphere", "ShowFOWBlobs"),
    ("OP_SensorsWireframe", "ShowAreaOccupied"),
    ("OP_Dust", "ShowAsteriod0"),
    ("OP_Background", "ShowBackground"),
    ("OP_Compass", "ShowCompass"),
    ("OP_ObjectivePointers", "ShowObjectivesPointer"),
    ("OP_SlipgatePaths", "ShowSlipgatePaths"),
    ("OP_CapitalShipOverlay", "ShowIcons"),
];

fn refresh_gameplay_toggles(fe: &mut Fe) {
    for (name, key) in GAMEPLAY_TOGGLES {
        let v = fe.game.options.get_bool(key);
        fe.toggle_set(name, v);
    }
    let r = fe.game.options.get_bool("ShowResources") && fe.game.options.get_bool("ShowMeteors");
    fe.toggle_set("OP_Resources", r);
}

/// "Homeworld classic" and "Cataclysm default" gameplay presets.
fn gameplay_preset(fe: &mut Fe, classic: bool) {
    let o = &mut fe.game.options;
    for k in [
        "HomeworldTaskbar",
        "ShowTaskbarButtons",
        "ShowTaskbarBackdrop",
        "Homeworld_CSA",
    ] {
        o.set_bool(k, classic);
    }
    o.set_bool("ExperienceMessages", false);
    o.set_bool("RightClick_CSA", !classic);
    for k in [
        "ShowAsteriod0",
        "ShowResources",
        "ShowFOWBlobs",
        "ShowBackground",
        "ShowIcons",
        "ShowEnemyShips",
        "ShowFriendlyShips",
        "ShowMeteors",
        "ShowObjectivesPointer",
        "ShowSlipgatePaths",
    ] {
        o.set_bool(k, true);
    }
    o.set_bool("ShowAreaVisited", !classic);
    o.set_bool("ShowAreaOccupied", false);
    o.set_bool("ShowCompass", classic);
    refresh_gameplay_toggles(fe);
}

/// `FUN_004d6c50`: "fastest game possible" lowers audio and video settings, "Cataclysm
/// default" restores them.
fn framerate(fe: &mut Fe, call: &Call) {
    radio_option(fe, call, "GameFramerateOption");
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let full = fe.game.options.get("GameFramerateOption") != 0;
    let o = &mut fe.game.options;
    o.set("NumChannels", if full { 8 } else { 0 });
    o.set("AutoChannel", 0);
    for k in ["VoiceCommands", "VoiceStatus", "VoiceChatter"] {
        o.set_bool(k, full);
    }
    o.set("opBattleChatter", if full { 50 } else { 0 });
    o.set("SoundQuality", if full { 0 } else { 2 });
    o.set("noLOD", 0);
    o.set("detailThreshold", if full { 10 } else { 30 });
    o.set("TextureDetail", if full { 50 } else { 0 });
    o.set("DetailQuality", if full { 0 } else { 2 });
    for k in [
        "texLinearFiltering",
        "showBackgrounds",
        "enableSmoothing",
        "enableCPV",
        "enableLensFlare",
        "enableRepairbots",
        "fuzzyBlobs",
        "enableTrails",
        "etgHitEffectsEnabled",
        "etgDamageEffectsEnabled",
        "etgFireEffectsEnabled",
        "etgBulletEffectsEnabled",
        "ShowAreaVisited",
        "ShowAreaOccupied",
        "ShowMeteors",
        "ShowAsteriod0",
        "ShowBackground",
    ] {
        o.set_bool(k, full);
    }
    o.set_bool("enableStipple", !full);
    o.set_bool("InstantTransition", !full);
    o.set("etgHistoryScalar", if full { 256 } else { HISTORY_MIN });
    o.set("effectsLevel", if full { 0 } else { 2 });
    o.set_bool("ExperienceMessages", false);
    o.set_bool("ShowCompass", false);
    o.set_bool("ShowFOWBlobs", false);
    refresh_gameplay_toggles(fe);
}

/// `utyLanguageToggle`: switches fonts and hot keys.
fn language(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.options.get("language");
        fe.radio_set(&call.name, v.max(0) as u32);
        return;
    }
    let v = fe.atom(a).radio_value;
    fe.game.options.set("language", v as i32);
    fe.resources().switch_language(Language::from_index(v));
    fe.hotkeys_update_all();
}

const ADVANCED_KEYS: &[&str] = &[
    "VoiceCommands",
    "VoiceStatus",
    "VoiceChatter",
    "opBattleChatter",
    "SoundQuality",
    "speakerSetting",
    "Equalizer0",
    "Equalizer1",
    "Equalizer2",
    "Equalizer3",
    "Equalizer4",
    "Equalizer5",
    "Equalizer6",
    "Equalizer7",
];
const EQ_KEYS: &[&str] = &[
    "speakerSetting",
    "Equalizer0",
    "Equalizer1",
    "Equalizer2",
    "Equalizer3",
    "Equalizer4",
    "Equalizer5",
    "Equalizer6",
    "Equalizer7",
];
const EFFECT_KEYS: &[&str] = &[
    "effectsLevel",
    "texLinearFiltering",
    "showBackgrounds",
    "InstantTransition",
    "enableSmoothing",
    "enableCPV",
    "enableLensFlare",
    "enableRepairbots",
    "DetailQuality",
    "enableStipple",
    "enableTrails",
    "fuzzyBlobs",
    "etgHitEffectsEnabled",
    "etgDamageEffectsEnabled",
    "etgFireEffectsEnabled",
    "etgBulletEffectsEnabled",
];

fn snapshot(g: &mut Cataclysm, keys: &[&str]) {
    for k in keys {
        let v = g.options.get(k);
        g.sub_saved.set(k, v);
    }
}

fn restore(g: &mut Cataclysm, keys: &[&str]) {
    for k in keys {
        let v = g.sub_saved.get(k);
        g.options.set(k, v);
    }
}

/// `opRender`: rendering systems.
fn render_list(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if !fe.first_call(call) {
        return;
    }
    let height = list_row_height(fe, "Arial_12.hff");
    fe.list_init(a, "Arial_12.hff", height);
    let renderers = fe.game.renderers.clone();
    if let Some(l) = fe.list_mut(a) {
        for (i, r) in renderers.iter().enumerate() {
            l.add(r.clone(), i as u32, i == 0);
        }
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// `opResolution`: display modes; picking one stores it.
fn resolution_list(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let height = list_row_height(fe, "Arial_12.hff");
        fe.list_init(a, "Arial_12.hff", height);
        let cur = (
            fe.game.options.get("screenWidth"),
            fe.game.options.get("screenHeight"),
        );
        let modes = fe.game.resolutions.clone();
        if let Some(l) = fe.list_mut(a) {
            for (i, &(w, h)) in modes.iter().enumerate() {
                l.add(format!("{w} x {h}"), i as u32, (w, h) == cur);
            }
            l.view_selection();
        }
        if let Some(id) = fe.atom(a).region {
            fe.list_scroll_adjust(id);
        }
        return;
    }
    let sel = fe
        .list(a)
        .and_then(|l| l.selected)
        .map(|i| fe.game.resolutions[i]);
    if let Some((w, h)) = sel {
        fe.game.options.set("screenWidth", w);
        fe.game.options.set("screenHeight", h);
    }
}

/// Row height of the option lists: one and a half lines of the list font.
fn list_row_height(fe: &Fe, font: &str) -> i32 {
    let h = fe.resources().font(font).map_or(12, |f| text::height(&f));
    h / 2 + h
}

/// Key binding lists (read-only).
fn key_list(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if !fe.first_call(call) {
        return;
    }
    let height = list_row_height(fe, "Arial_12.hff");
    fe.list_init(a, "Arial_12.hff", height);
    let binds = fe.game.key_bindings.clone();
    if let Some(l) = fe.list_mut(a) {
        for (i, (cmd, key)) in binds.iter().enumerate() {
            l.add(format!("{cmd}\t{key}"), i as u32, false);
        }
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// Two columns: command and key.
fn key_item_draw(fe: &Fe, fer: &mut Fer, row: Rect, item: &crate::controls::ListItem) {
    let Some(font) = fer.res.font("Arial_12.hff") else {
        return;
    };
    let colour = if item.selected {
        colour::LIST_ITEM_SELECTED
    } else {
        colour::LIST_ITEM_STANDARD
    };
    let (cmd, key) = item.text.split_once('\t').unwrap_or((&item.text, ""));
    let _ = fe;
    let none = (Shadow::None, BLACK);
    text::print(
        fer.canvas,
        &font,
        row.x0,
        row.y0,
        colour,
        &text::to_latin1(cmd),
        none,
    );
    let k = text::to_latin1(key);
    let x = row.x1 - text::width(&font, &k) - 4;
    text::print(fer.canvas, &font, x, row.y0, colour, &k, none);
}

/// `opDrawNumChannels`: the channel count on the panel colour.
fn draw_num_channels(fe: &mut Fe, fer: &mut Fer, _atom: AtomRef, rect: Rect) {
    fer.canvas.fill_rect(rect, [90, 99, 122, 255]);
    let Some(font) = fer.res.font("HW_EuroseCond_11.hff") else {
        return;
    };
    let n = fe.game.options.get("NumChannels") + VOICE_MIN;
    text::print(
        fer.canvas,
        &font,
        rect.x0,
        rect.y0,
        WHITE,
        n.to_string().as_bytes(),
        (Shadow::None, BLACK),
    );
}

/// Draws an image filling `rect` (helper for the colour picker preview).
pub(crate) fn draw_image_in(fer: &mut Fer, img: &Image, rect: Rect) {
    if img.width == rect.width() && img.height == rect.height() {
        fer.canvas.blit(img, rect.x0, rect.y0, BlitMode::Opaque);
    } else {
        fer.canvas.blit_scaled(img, rect, 255);
    }
}

/// Packs an RGB colour the engine's way (`colRGB`: alpha in the top byte, then blue, green,
/// red) for the colour options.
pub fn pack_color(c: Color) -> i32 {
    (0xff00_0000u32 | (c[2] as u32) << 16 | (c[1] as u32) << 8 | c[0] as u32) as i32
}

pub fn unpack_color(v: i32) -> Color {
    let v = v as u32;
    [
        (v & 0xff) as u8,
        (v >> 8 & 0xff) as u8,
        (v >> 16 & 0xff) as u8,
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_values() {
        let s = "strVersionX  nope\nstrVersion      Version  \n";
        assert_eq!(script_value(s, "strVersion").as_deref(), Some("Version"));
        let t = "selFontName2   TO_2.hff ;actual font --> Arial 7pt\n";
        assert_eq!(script_value(t, "selFontName2").as_deref(), Some("TO_2.hff"));
    }

    #[test]
    fn key_names() {
        assert_eq!(key_name("77"), "M");
        assert_eq!(key_name("SHIFT+83"), "SHIFT+S");
        assert_eq!(key_name("112"), "F1");
    }

    #[test]
    fn colors_pack() {
        let c = [66, 108, 202, 255];
        assert_eq!(unpack_color(pack_color(c)), c);
        assert_eq!(pack_color([0, 0, 0, 255]), -16_777_216);
    }

    #[test]
    fn effect_presets() {
        let mut o = Options::default();
        o.set("effectsLevel", 1);
        apply_effects_preset(&mut o);
        assert!(!o.get_bool("texLinearFiltering"));
        assert!(o.get_bool("etgDamageEffectsEnabled"));
        assert_eq!(o.get("etgHistoryScalar"), 64);
        o.set("effectsLevel", 7);
        apply_effects_preset(&mut o);
        assert_eq!(o.get("effectsLevel"), 2);
    }
}
