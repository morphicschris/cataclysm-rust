//! Front end on the real game files. Needs `HWC_DATA_DIR` (an imported data directory) or
//! `HWC_GAME_DIR` (an install); skipped otherwise.

use std::rc::Rc;

use hwc_formats::fib::Fib;
use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::{AssetSource, Canvas, FrontEnd, Resources};

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn session(src: Rc<dyn AssetSource>, options: Option<std::path::PathBuf>) -> Session {
    let mut s = Session::new(
        src,
        Setup {
            options_path: options,
            scale: ScaleMode::Original,
            ..Default::default()
        },
    )
    .expect("session");
    s.resize(640, 480);
    s
}

fn frame(s: &mut Session) -> Vec<Request> {
    s.update(1.0 / 60.0)
}

/// Clicks a named control as a player would: not as the second half of a double click
/// (which, as in the original, buttons do not see), so the double-click time passes first.
fn click(s: &mut Session, name: &str) -> Vec<Request> {
    let mut out = Vec::new();
    for _ in 0..32 {
        out.extend(frame(s));
    }
    let a =
        s.fe.find_atom(name)
            .unwrap_or_else(|| panic!("no {name} on {:?}", s.fe.stack_names()));
    let r = s.fe.atom(a).rect();
    s.fe.mouse_move((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    out.extend(frame(s));
    s.mouse_button(true, true);
    out.extend(frame(s));
    s.mouse_button(true, false);
    out.extend(frame(s));
    out
}

#[test]
fn every_screen_of_every_fib_starts_and_draws() {
    let Some(src) = source() else { return };
    let names = [
        "front_end",
        "beast_construction_manager",
        "beast_launch_manager",
        "beast_research_manager",
        "choose_server",
        "console",
        "construction_manager",
        "csm-all",
        "front_end_demo",
        "game_chat",
        "horse_race",
        "hyperspace_roll_call",
        "in_game_esc_menu",
        "launch_manager",
        "multiplayer_game",
        "multiplayer_lan_game",
        "research_manager",
        "sensors_manager",
        "single_player_objective",
        "taskbar",
        "trader_interface",
    ];
    let mut screens = 0;
    for n in names {
        let data = src
            .read(&format!("FEMan\\{n}.fib"))
            .unwrap_or_else(|| panic!("{n}.fib"));
        let fib = Fib::parse(&data).unwrap();
        let res = Rc::new(Resources::new(src.clone()));
        let mut fe = FrontEnd::new((), res);
        hwc_ui::cataclysm::register_stubs(&mut fe);
        fe.load_fib(&fib);
        let mut canvas = Canvas::new(640, 480);
        for s in &fib.screens {
            fe.all_screens_delete();
            assert!(fe.screen_start(&s.name).is_some(), "{n}: {}", s.name);
            fe.process(0.016);
            fe.render(&mut canvas);
            screens += 1;
        }
        fe.all_screens_delete();
        // Only the root and main regions remain.
        assert_eq!(fe.regions.len(), 2, "{n}: regions leaked");
    }
    assert_eq!(screens, 154);
}

#[test]
fn main_menu_navigation_and_options_round_trip() {
    let Some(src) = source() else { return };
    let dir = std::env::temp_dir().join(format!("hwc-ui-test-{}", std::process::id()));
    let cfg = dir.join("hwc-options.cfg");
    let _ = std::fs::remove_file(&cfg);
    let mut s = session(src.clone(), Some(cfg.clone()));
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);

    // Options: every tab, change some values, accept.
    click(&mut s, "OP_Options_Start");
    assert_eq!(s.fe.stack_names(), ["Audio_Options"]);
    click(&mut s, "OP_AutoChannels");
    click(&mut s, "Audio_Options-TO-Advanced");
    assert_eq!(s.fe.stack_names(), ["Advanced"]);
    click(&mut s, "Advanced-TO-Equalizer");
    assert_eq!(s.fe.stack_names(), ["Equalizer"]);
    click(&mut s, "OP_Equalizer_To_Audio_Accept");
    assert_eq!(s.fe.stack_names(), ["Audio_Options"]);
    click(&mut s, "Audio_Options-TO-Video_Options");
    let screen = s.fe.current_screen().unwrap();
    let low =
        s.fe.atoms_named(screen, "OP_Effects")
            .into_iter()
            .find(|&a| s.fe.atom(a).radio_value == 1)
            .unwrap();
    let r = s.fe.atom(low).rect();
    s.fe.mouse_move(r.x0 + 4, r.y0 + 4);
    frame(&mut s);
    s.mouse_button(true, true);
    frame(&mut s);
    s.mouse_button(true, false);
    frame(&mut s);
    assert_eq!(s.fe.game.options.get("effectsLevel"), 1);
    assert_eq!(s.fe.game.options.get("texLinearFiltering"), 0);
    click(&mut s, "Video_Options-TO-Options_New");
    assert_eq!(s.fe.stack_names(), ["Options_New"]);
    click(&mut s, "OP_Effects_To_Video_Accept");
    // Accepting the custom effects screen selects "Custom".
    assert_eq!(s.fe.game.options.get("effectsLevel"), 2);
    click(&mut s, "Video_Options-TO-Gameplay_Options");
    click(&mut s, "OP_HomeworldClassic");
    assert_eq!(s.fe.game.options.get("HomeworldTaskbar"), 1);
    click(&mut s, "Gameplay_Options-TO-Keyboard");
    assert_eq!(s.fe.stack_names(), ["Keyboard"]);
    let reqs = click(&mut s, "OP_Options_Accept");
    assert!(reqs.contains(&Request::ApplyOptions));
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);
    assert!(cfg.is_file());

    // A new session reads them back.
    let s2 = session(src.clone(), Some(cfg.clone()));
    assert_eq!(s2.fe.game.options.get("AutoChannel"), 1);
    assert_eq!(s2.fe.game.options.get("effectsLevel"), 2);
    assert_eq!(s2.fe.game.options.get("HomeworldTaskbar"), 1);

    // Cancel restores.
    click(&mut s, "OP_Options_Start");
    click(&mut s, "OP_AutoChannels");
    assert_eq!(s.fe.game.options.get("AutoChannel"), 0);
    click(&mut s, "OP_Options_Cancel");
    assert_eq!(s.fe.game.options.get("AutoChannel"), 1);

    // Training, single player, quit.
    s.key_down(u16::from(b'T'));
    frame(&mut s);
    s.key_up(u16::from(b'T'));
    frame(&mut s);
    assert_eq!(s.fe.stack_names(), ["Select_tutorial"]);
    click(&mut s, "Select_tutorial-TO-Main_game_screen");
    click(&mut s, "SinglePlayerOptions");
    assert_eq!(s.fe.stack_names(), ["Create_new_game"]);
    click(&mut s, "StartSinglePlayerGame");
    assert_eq!(s.fe.stack_names(), ["Create_new_game", "Tutorial_Alert"]);
    let reqs = click(&mut s, "TA_StartSinglePlayer");
    assert!(reqs.iter().any(|r| matches!(
        r,
        Request::StartGame {
            tutorial: false,
            ..
        }
    )));
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);
    assert_eq!(s.fe.game.options.get("TutorialNeeded"), 0);
    click(&mut s, "FE_HideMenuOrGotoQuitGame");
    assert_eq!(s.fe.stack_names(), ["Main_game_screen", "Quit_game"]);
    // The hidden Escape button says no.
    s.key_down(hwc_ui::consts::key::ESCAPE);
    frame(&mut s);
    s.key_up(hwc_ui::consts::key::ESCAPE);
    frame(&mut s);
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);
    click(&mut s, "FE_HideMenuOrGotoQuitGame");
    let reqs = click(&mut s, "QuitGame");
    assert!(reqs.contains(&Request::Quit));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn every_front_end_callback_is_bound() {
    let Some(src) = source() else { return };
    let s = session(src, None);
    let fib = Fib::parse(
        &s.fe
            .resources()
            .read(hwc_ui::session::FRONT_END_FIB)
            .unwrap(),
    )
    .unwrap();
    let mut missing = Vec::new();
    for screen in &fib.screens {
        for a in &screen.atoms {
            let Some(name) = &a.name else { continue };
            if a.raw_kind == 1 && !s.fe.has_draw_callback(name) {
                missing.push(name.clone());
            }
            // User regions only draw (their function flag just logs "not found" on create).
            if a.raw_kind != 1
                && a.flags & hwc_formats::fib::ATOM_FUNCTION != 0
                && !s.fe.has_callback(name)
            {
                missing.push(name.clone());
            }
        }
    }
    assert!(missing.is_empty(), "{missing:?}");
    // No front-end callback is a logging stub.
    for (name, files) in hwc_ui::known::FUNCTIONS {
        if files.contains(&"front_end.fib") {
            let ok = s.fe.has_callback(name);
            assert!(ok, "{name}");
        }
    }
}

#[test]
fn main_menu_pixels() {
    let Some(src) = source() else { return };
    let mut s = session(src, None);
    frame(&mut s);
    let c = s.render();
    // Button middle texture (red), label (yellow), decorative title, black border region.
    let px = |x: i32, y: i32| c.get(x, y);
    let button = px(375, 263);
    assert!(
        button[0] > 2 * button[2] && button[0] > 40 && button[1] < 20,
        "{button:?}"
    );
    assert_eq!(px(5, 470), [0, 0, 0, 255]);
    // The version string is drawn in white on the top-left strip.
    let white = (37..174)
        .flat_map(|x| (0..28).map(move |y| (x, y)))
        .filter(|&(x, y)| px(x, y)[0] > 200)
        .count();
    assert!(white > 50);
}

#[test]
fn labels_in_every_language() {
    let Some(src) = source() else { return };
    let dir = std::env::temp_dir().join(format!("hwc-ui-lang-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("hwc-options.cfg");
    let mut o = hwc_ui::options::Options::default();
    o.set("language", 2);
    o.save(&cfg).unwrap();
    let mut s = session(src, Some(cfg));
    assert_eq!(s.fe.resources().language(), hwc_ui::Language::German);
    let main = s.fe.find_screen("Main_game_screen").unwrap();
    let label = s.fe.screens[main]
        .atoms
        .iter()
        .find(|a| a.text.as_deref() == Some(b"&SINGLE PLAYER".as_slice()))
        .unwrap();
    assert_eq!(label.texts.len(), 3);
    assert_eq!(label.texts[1], b"&UN JOUEUR");
    assert_eq!(label.texts[2], b"EINZEL&SPIELER");
    frame(&mut s);
    s.render();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Clicks item `n` of the resolution list.
fn pick_resolution(s: &mut Session, n: usize) {
    let a = s.fe.find_atom("OP_Resolution").expect("resolution list");
    let rect = s.fe.atom(a).rect();
    let l = s.fe.list(a).expect("list");
    let y = (rect.y0..rect.y1)
        .find(|&y| l.item_at(rect, y) == Some(n))
        .expect("visible item");
    s.fe.mouse_move((rect.x0 + rect.x1) / 2, y + 2);
    frame(s);
    s.mouse_button(true, true);
    frame(s);
    s.mouse_button(true, false);
    frame(s);
}

#[test]
fn new_display_mode_is_confirmed_or_restored() {
    let Some(src) = source() else { return };
    let mut s = session(src, None);
    click(&mut s, "OP_Options_Start");
    click(&mut s, "Audio_Options-TO-Video_Options");
    pick_resolution(&mut s, 1);
    let out = click(&mut s, "OP_Options_Accept");
    assert!(out.contains(&Request::DisplayMode {
        width: 800,
        height: 600
    }));
    assert_eq!(
        s.fe.stack_names(),
        ["Main_game_screen", "Countdown_Message_Box"]
    );
    // Nobody answers: after 22 seconds the old mode comes back on the main menu.
    let mut out = Vec::new();
    for _ in 0..23 {
        out.extend(s.update(1.0));
    }
    assert!(out.contains(&Request::DisplayMode {
        width: 640,
        height: 480
    }));
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);
    assert_eq!(s.fe.game.options.get("screenWidth"), 640);
    // "Yes" keeps the new mode.
    click(&mut s, "OP_Options_Start");
    click(&mut s, "Audio_Options-TO-Video_Options");
    pick_resolution(&mut s, 2);
    click(&mut s, "OP_Options_Accept");
    let out = click(&mut s, "OP_Countdown_Yes");
    assert!(!out.iter().any(|r| matches!(r, Request::DisplayMode { .. })));
    assert_eq!(s.fe.stack_names(), ["Main_game_screen"]);
    assert_eq!(s.fe.game.display, (1024, 768));
    // The audio test buttons.
    click(&mut s, "OP_Options_Start");
    let out = click(&mut s, "OP_SoundFX_Test");
    assert!(out.contains(&Request::SoundEvent(hwc_ui::consts::sound::SFX_TEST)));
    let out = click(&mut s, "OP_Speech_Test");
    assert!(out.contains(&Request::Speech(hwc_ui::consts::sound::SPEECH_TEST)));
}
