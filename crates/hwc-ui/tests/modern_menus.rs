//! The modern menus (`hwcModernMenus = 1`, the default) on the real game files with the mock
//! host: the escape menu, the options hub and every options screen, the key bindings, the
//! confirmations and the objectives replace the original's look while the original screens
//! stay open underneath, and every control acts through the original's regions: buttons,
//! check boxes, radio buttons (segments and drop-downs), sliders (dragged), the key list
//! (rebinding by double click and by Return), the save list and its text entry; one Escape
//! goes back one level; the modern options screen keeps its changes until ACCEPT. With
//! `--menus original` (`hwcModernMenus = 0`) the original screens come back. Screenshots of
//! every screen go to `screenshots/menus/mock_*.png` when `HWC_SCREENSHOTS` is set. Needs
//! `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::consts::{key, sound};
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::keys;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_menu::{self as menu, Hit, Kind, Layout, Model};
use hwc_ui::ingame::modern_options as mo;
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

const KEYS: &str = "v 84\nkey  Move  77\nkey  Dock  68\nkey  Escape  27\n\
key  Open_BuildManager  66\nkey  Open_ResearchManager  82\nkey  Open_LaunchManager  83\n";

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn ui_with(src: Rc<dyn AssetSource>, size: (i32, i32), options: Options) -> InGameUi {
    let mut ui = InGameUi::new(
        src,
        Setup {
            options,
            default_keys: Some(KEYS.into()),
            keys: None,
            keys_path: None,
        },
    )
    .expect("in-game ui");
    ui.resize(size.0, size.1);
    ui
}

fn frame(ui: &mut InGameUi, host: &mut MockHost) -> Vec<GameRequest> {
    let out = ui.update(host, 1.0 / 30.0);
    for r in &out {
        host.apply(r);
    }
    out
}

fn settle(ui: &mut InGameUi, host: &mut MockHost) {
    for _ in 0..20 {
        frame(ui, host);
    }
}

fn input(ui: &mut InGameUi, host: &mut MockHost, ev: InputEvent) -> Vec<GameRequest> {
    ui.handle_input(host, ev);
    frame(ui, host)
}

fn press(ui: &mut InGameUi, host: &mut MockHost, down: bool) -> Vec<GameRequest> {
    input(ui, host, InputEvent::MouseButton { left: true, down })
}

/// A click as a player would make it, clear of any earlier click's double-click time.
fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    let mut out = input(ui, host, InputEvent::MouseMove { x, y });
    out.extend(press(ui, host, true));
    out.extend(press(ui, host, false));
    out
}

fn key(ui: &mut InGameUi, host: &mut MockHost, code: u16) -> Vec<GameRequest> {
    let mut out = input(ui, host, InputEvent::KeyDown(code));
    out.extend(input(ui, host, InputEvent::KeyUp(code)));
    out
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn now(ui: &InGameUi, size: (i32, i32)) -> (Model, Layout) {
    let m = menu::model(&ui.fe).expect("a modern screen");
    let l = menu::layout(&m, size.0, size.1);
    (m, l)
}

/// The item labelled `label` on the screen up now.
fn item(ui: &InGameUi, size: (i32, i32), label: &str) -> (usize, Model, Layout) {
    let (m, l) = now(ui, size);
    let i = m.find(label).unwrap_or_else(|| {
        panic!(
            "no {label:?} on {:?}: {:?}",
            m.name,
            m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
        )
    });
    (i, m, l)
}

/// Clicks the control labelled `label`.
fn click(
    ui: &mut InGameUi,
    host: &mut MockHost,
    size: (i32, i32),
    label: &str,
) -> Vec<GameRequest> {
    let (i, m, l) = item(ui, size, label);
    let r = match m.items[i].kind {
        Kind::Toggle { .. } => l.placed[i].row,
        _ => l.placed[i].control,
    };
    let (x, y) = centre(r);
    let (x, y) = match m.items[i].kind {
        Kind::Toggle { .. } => (r.x0 + 30, y),
        _ => (x, y),
    };
    assert_eq!(
        menu::hit(&m, &l, None, size.1, x, y),
        Hit::Item(i),
        "{label}"
    );
    click_at(ui, host, x, y)
}

fn screen(ui: &InGameUi) -> &str {
    ui.fe.current_screen_name().unwrap_or("")
}

fn background(size: (i32, i32)) -> Canvas {
    let mut canvas = Canvas::new(size.0, size.1);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let t = x as f32 / size.0 as f32;
            let u = y as f32 / size.1 as f32;
            canvas.pixels[(y * size.0 + x) as usize] = [
                (30.0 + 50.0 * t) as u8,
                (40.0 + 40.0 * u) as u8,
                (70.0 + 50.0 * u) as u8,
                255,
            ];
        }
    }
    canvas
}

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) -> Canvas {
    let mut canvas = background(size);
    ui.draw(&mut canvas);
    if std::env::var_os("HWC_SCREENSHOTS").is_some() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/menus");
        std::fs::create_dir_all(&dir).unwrap();
        canvas
            .save_png(&dir.join(format!("{name}_{}x{}.png", size.0, size.1)))
            .unwrap();
    }
    canvas
}

/// Rests the pointer away from every control (for a screenshot without hover).
fn rest(ui: &mut InGameUi, host: &mut MockHost, size: (i32, i32)) {
    input(
        ui,
        host,
        InputEvent::MouseMove {
            x: 4,
            y: size.1 - 4,
        },
    );
    settle(ui, host);
}

/// Every screen of the modern menus, at a size, into the screenshots.
fn tour(size: (i32, i32)) {
    let Some(src) = source() else { return };
    let mut ui = ui_with(src, size, Options::default());
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(ui.manager(), Some(Manager::EscMenu));
    assert!(menu::active(&ui.fe));
    rest(&mut ui, &mut host, size);
    shot(&mut ui, size, "mock_pause");
    // Hover a button for its look.
    let (i, _, l) = item(&ui, size, "OPTIONS");
    let (x, y) = centre(l.placed[i].control);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y });
    settle(&mut ui, &mut host);
    shot(&mut ui, size, "mock_pause_hover");
    click(&mut ui, &mut host, size, "OPTIONS");
    assert_eq!(screen(&ui), "Game_options");
    rest(&mut ui, &mut host, size);
    shot(&mut ui, size, "mock_options");
    for (button, name, tag) in [
        ("AUDIO", "Audio", "audio"),
        ("VIDEO", "Video", "video"),
        ("GAMEPLAY", "Gameplay", "gameplay"),
        ("KEY BINDINGS", "Keys", "keys"),
    ] {
        click(&mut ui, &mut host, size, button);
        assert_eq!(screen(&ui), name);
        rest(&mut ui, &mut host, size);
        shot(&mut ui, size, &format!("mock_{tag}"));
        match name {
            "Audio" => {
                click(&mut ui, &mut host, size, "ADVANCED SPEECH SETTINGS");
                assert_eq!(screen(&ui), "Advanced_speech");
                rest(&mut ui, &mut host, size);
                shot(&mut ui, size, "mock_advanced_speech");
                click(&mut ui, &mut host, size, "CHANGE");
                assert_eq!(screen(&ui), "InGameEqualizer");
                rest(&mut ui, &mut host, size);
                shot(&mut ui, size, "mock_equalizer");
                key(&mut ui, &mut host, key::ESCAPE);
                assert_eq!(screen(&ui), "Advanced_speech");
                key(&mut ui, &mut host, key::ESCAPE);
                assert_eq!(screen(&ui), "Audio");
            }
            "Video" => {
                click(&mut ui, &mut host, size, "CUSTOM EFFECTS");
                assert_eq!(screen(&ui), "Custom_options");
                rest(&mut ui, &mut host, size);
                shot(&mut ui, size, "mock_custom_effects");
                key(&mut ui, &mut host, key::ESCAPE);
                assert_eq!(screen(&ui), "Video");
                click(&mut ui, &mut host, size, "TEXTURE INFO");
                assert_eq!(screen(&ui), "InGameVideo_Explanation");
                rest(&mut ui, &mut host, size);
                shot(&mut ui, size, "mock_texture_info");
                key(&mut ui, &mut host, key::ESCAPE);
                assert_eq!(screen(&ui), "Video");
            }
            _ => {}
        }
        key(&mut ui, &mut host, key::ESCAPE);
        assert_eq!(
            screen(&ui),
            "Game_options",
            "one Escape goes back from {name}"
        );
    }
    click(&mut ui, &mut host, size, mo::HUB_LABEL);
    assert!(ui.state().mmenu.page.is_some());
    rest(&mut ui, &mut host, size);
    shot(&mut ui, size, "mock_enhancements");
    key(&mut ui, &mut host, key::ESCAPE);
    assert!(ui.state().mmenu.page.is_none());
    assert_eq!(screen(&ui), "Game_options");
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(screen(&ui), "In_game_esc_menu2");
    for (button, name, tag) in [
        ("SAVE GAME", "In_game_save", "save"),
        ("LOAD GAME", "In_game_load", "load"),
        ("QUIT", "Quit_game2", "quit"),
    ] {
        click(&mut ui, &mut host, size, button);
        assert_eq!(screen(&ui), name);
        rest(&mut ui, &mut host, size);
        shot(&mut ui, size, &format!("mock_{tag}"));
        key(&mut ui, &mut host, key::ESCAPE);
        assert_eq!(
            screen(&ui),
            "In_game_esc_menu2",
            "one Escape goes back from {name}"
        );
    }
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(
        ui.manager(),
        None,
        "Escape on the pause menu goes back to the game"
    );
    // Game over, and the objectives.
    ui.game_over(&host);
    rest(&mut ui, &mut host, size);
    assert_eq!(screen(&ui), "SP_Game_Over");
    shot(&mut ui, size, "mock_game_over");
    ui.close();
    host.fleet_intel = "Fleet intelligence reports a Turanic raider fleet moving to intercept. \
        Protect the Kuun-Lan while it completes its repairs.\nDestroy the raiders' carrier \
        before it can bring reinforcements."
        .into();
    hwc_ui::ingame::escmenu::open_objectives(&mut ui.fe, None);
    frame(&mut ui, &mut host);
    rest(&mut ui, &mut host, size);
    assert_eq!(screen(&ui), "Single_Player_Objective");
    shot(&mut ui, size, "mock_objectives");
}

#[test]
fn every_screen_at_1080() {
    tour((1920, 1080));
}

#[test]
fn every_screen_at_768() {
    tour((1024, 768));
}

#[test]
fn every_screen_at_480() {
    tour((640, 480));
}

const SIZE: (i32, i32) = (1920, 1080);

/// The escape menu open on a fresh interface.
fn opened(options: Options) -> Option<(InGameUi, MockHost)> {
    let src = source()?;
    let mut ui = ui_with(src, SIZE, options);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(ui.manager(), Some(Manager::EscMenu));
    Some((ui, host))
}

fn to_screen(ui: &mut InGameUi, host: &mut MockHost, path: &[&str]) {
    for b in path {
        click(ui, host, SIZE, b);
    }
}

/// Drags the slider labelled `label` from its knob to `fraction` of its track.
fn drag_slider(ui: &mut InGameUi, host: &mut MockHost, label: &str, fraction: f32) {
    let (i, m, l) = item(ui, SIZE, label);
    let Kind::Slider { fraction: now, .. } = m.items[i].kind else {
        panic!("{label} is not a slider");
    };
    let t = l.placed[i].control;
    let y = (t.y0 + t.y1) / 2;
    let at = |f: f32| t.x0 + (t.width() as f32 * f).round() as i32;
    settle(ui, host);
    input(ui, host, InputEvent::MouseMove { x: at(now), y });
    press(ui, host, true);
    // Along the track, and off it up and down while held (the original keeps the pointer).
    for k in 1..=6 {
        let f = now + (fraction - now) * k as f32 / 6.0;
        input(
            ui,
            host,
            InputEvent::MouseMove {
                x: at(f),
                y: y + 30 * (k % 2),
            },
        );
    }
    press(ui, host, false);
}

fn slider_value(ui: &InGameUi, atom: &str) -> u32 {
    let a = ui.fe.find_atom(atom).expect("the slider");
    ui.fe.slider(a).expect("a slider").value
}

fn option(ui: &InGameUi, key: &str) -> i32 {
    ui.fe.game.options.get(key)
}

fn current_choice(ui: &InGameUi, label: &str) -> Option<usize> {
    let (i, m, _) = item(ui, SIZE, label);
    match &m.items[i].kind {
        Kind::Choice { current, .. } => *current,
        k => panic!("{label} is {k:?}"),
    }
}

#[test]
fn pause_menu_buttons_hot_keys_and_a_cancelled_press() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    assert!(menu::active(&ui.fe));
    assert_eq!(
        screen(&ui),
        "In_game_esc_menu2",
        "the original screen stays open"
    );
    let (m, _) = now(&ui, SIZE);
    let labels: Vec<&str> = m.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["QUIT", "SAVE GAME", "LOAD GAME", "OPTIONS", "BACK TO GAME"]
    );
    assert_eq!(m.items[4].key, "B");
    assert!(
        m.items[4].focused,
        "the original's first tab stop has the focus"
    );
    // A press dragged off a button does nothing.
    let (i, _, l) = item(&ui, SIZE, "OPTIONS");
    let (x, y) = centre(l.placed[i].control);
    settle(&mut ui, &mut host);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y });
    press(&mut ui, &mut host, true);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y: y + 300 });
    press(&mut ui, &mut host, false);
    settle(&mut ui, &mut host);
    assert_eq!(screen(&ui), "In_game_esc_menu2");
    // The press focused OPTIONS (as the original's press does).
    let (m, _) = now(&ui, SIZE);
    assert!(m.items[m.find("OPTIONS").unwrap()].focused);
    // Keyboard: Home to the first tab stop (BACK TO GAME), Down through the others, Return
    // runs the focused one.
    key(&mut ui, &mut host, key::HOME);
    let (m, _) = now(&ui, SIZE);
    assert!(m.items[m.find("BACK TO GAME").unwrap()].focused);
    for _ in 0..4 {
        key(&mut ui, &mut host, key::DOWN);
    }
    let (m, _) = now(&ui, SIZE);
    assert!(m.items[m.find("OPTIONS").unwrap()].focused, "{:?}", m.items);
    key(&mut ui, &mut host, key::RETURN);
    assert_eq!(screen(&ui), "Game_options");
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(screen(&ui), "In_game_esc_menu2");
    // Hot keys: O opens the options; BACK TO GAME goes back to the game (unpaused).
    key(&mut ui, &mut host, u16::from(b'O'));
    assert_eq!(screen(&ui), "Game_options");
    key(&mut ui, &mut host, key::ESCAPE);
    let out = click(&mut ui, &mut host, SIZE, "BACK TO GAME");
    assert_eq!(ui.manager(), None);
    assert!(out.contains(&GameRequest::SetPaused(false)));
    // The click sound came from the original button.
    assert!(out.contains(&GameRequest::Sound(sound::CLICK)));
}

#[test]
fn audio_sliders_drag_toggles_and_cancel_restores() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    to_screen(&mut ui, &mut host, &["OPTIONS", "AUDIO"]);
    assert_eq!(screen(&ui), "Audio");
    let sfx = option(&ui, "SFXVolume");
    drag_slider(&mut ui, &mut host, "SOUND F/X", 0.2);
    let v = slider_value(&ui, "OP_SFX_Volume");
    assert!((15..=25).contains(&v), "dragged to about 20: {v}");
    assert_eq!(
        option(&ui, "SFXVolume"),
        v as i32,
        "the original's slider callback ran"
    );
    // Past the end while held: the end (the pointer is kept on the slider).
    drag_slider(&mut ui, &mut host, "MUSIC", 1.3);
    assert!(slider_value(&ui, "OP_Music_Volume") >= 98);
    // The channel count shows the user region's figure.
    let (i, m, _) = item(&ui, SIZE, "# CHANNELS:");
    let Kind::Slider { readout, .. } = &m.items[i].kind else {
        panic!()
    };
    assert_eq!(readout, &(option(&ui, "NumChannels") + 8).to_string());
    let auto = option(&ui, "AutoChannel");
    click(&mut ui, &mut host, SIZE, "# CHANNELS AUTO");
    assert_ne!(option(&ui, "AutoChannel"), auto, "the check box toggled");
    let (i, m, _) = item(&ui, SIZE, "# CHANNELS AUTO");
    assert_eq!(m.items[i].kind, Kind::Toggle { on: auto == 0 });
    // Cancel: back to the hub, every value as it was.
    click(&mut ui, &mut host, SIZE, "CANCEL");
    assert_eq!(screen(&ui), "Game_options");
    assert_eq!(option(&ui, "SFXVolume"), sfx);
    assert_eq!(option(&ui, "AutoChannel"), auto);
    // Accept keeps a change and asks for it to be applied.
    click(&mut ui, &mut host, SIZE, "AUDIO");
    drag_slider(&mut ui, &mut host, "SPEECH", 0.5);
    let out = click(&mut ui, &mut host, SIZE, "ACCEPT!");
    assert_eq!(screen(&ui), "Game_options");
    assert!(out.contains(&GameRequest::ApplyOptions));
    assert!((45..=55).contains(&option(&ui, "SpeechVolume")));
}

#[test]
fn radio_groups_as_segments_and_drop_downs() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    to_screen(&mut ui, &mut host, &["OPTIONS", "VIDEO"]);
    assert_eq!(screen(&ui), "Video");
    let (i, m, l) = item(&ui, SIZE, "MODEL QUALITY");
    assert!(!l.placed[i].dropdown);
    let Kind::Choice { labels, .. } = &m.items[i].kind else {
        panic!()
    };
    assert_eq!(labels, &["LOW", "MEDIUM", "HIGH"]);
    let (x, y) = centre(l.placed[i].parts[0]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(current_choice(&ui, "MODEL QUALITY"), Some(0));
    assert_eq!(
        option(&ui, "DetailQuality"),
        2,
        "LOW is the radio button of value 2"
    );
    // A check box with its label over it.
    let nolod = option(&ui, "noLOD");
    click(&mut ui, &mut host, SIZE, "FULL DETAIL");
    assert_ne!(option(&ui, "noLOD"), nolod);
    key(&mut ui, &mut host, key::ESCAPE);
    // The speakers: a drop-down.
    to_screen(&mut ui, &mut host, &["AUDIO", "ADVANCED SPEECH SETTINGS"]);
    assert_eq!(screen(&ui), "Advanced_speech");
    let (i, m, l) = item(&ui, SIZE, "SPEAKER SETTINGS");
    assert!(l.placed[i].dropdown);
    let Kind::Choice { labels, .. } = &m.items[i].kind else {
        panic!()
    };
    let phones = labels.iter().position(|l| l == "HEADPHONES").unwrap();
    click(&mut ui, &mut host, SIZE, "SPEAKER SETTINGS");
    assert_eq!(
        ui.state().mmenu.dropdown,
        Some(i),
        "the click opened the list"
    );
    shot(&mut ui, SIZE, "mock_dropdown");
    let rows = l.dropdown_rows(i, labels.len(), SIZE.1);
    let (x, y) = centre(rows[phones]);
    let (m2, l2) = now(&ui, SIZE);
    assert_eq!(
        menu::hit(&m2, &l2, Some(i), SIZE.1, x, y),
        Hit::Choice(i, phones)
    );
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().mmenu.dropdown, None, "choosing closes it");
    assert_eq!(current_choice(&ui, "SPEAKER SETTINGS"), Some(phones));
    assert_eq!(option(&ui, "speakerSetting"), 2);
    // Escape closes an open list, and only that.
    click(&mut ui, &mut host, SIZE, "SPEAKER SETTINGS");
    assert!(ui.state().mmenu.dropdown.is_some());
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(ui.state().mmenu.dropdown, None);
    assert_eq!(screen(&ui), "Advanced_speech");
    // A click outside an open list closes it without acting.
    click(&mut ui, &mut host, SIZE, "SPEAKER SETTINGS");
    click_at(&mut ui, &mut host, 5, 5);
    assert_eq!(ui.state().mmenu.dropdown, None);
    assert_eq!(screen(&ui), "Advanced_speech");
    // The equalizer's vertical sliders drag.
    click(&mut ui, &mut host, SIZE, "CHANGE");
    assert_eq!(screen(&ui), "InGameEqualizer");
    let (i, _, l) = item(&ui, SIZE, "344");
    let t = l.placed[i].control;
    let x = (t.x0 + t.x1) / 2;
    settle(&mut ui, &mut host);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y: t.y0 });
    press(&mut ui, &mut host, true);
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x,
            y: t.y0 + t.height() * 3 / 4,
        },
    );
    press(&mut ui, &mut host, false);
    let v = slider_value(&ui, "OP_Equalizer3");
    assert!((70..=80).contains(&v), "{v}");
}

#[test]
fn gameplay_sections_toggles_and_accept() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    to_screen(&mut ui, &mut host, &["OPTIONS", "GAMEPLAY"]);
    let (m, _) = now(&ui, SIZE);
    let section = |label: &str| m.sections[m.items[m.find(label).unwrap()].section].clone();
    assert_eq!(section("COMPASS"), "SENSORS OPTIONS");
    assert_eq!(section("HIDDEN"), "TASKBAR");
    assert_eq!(section("CATACLYSM DEFAULT"), "USER INTERFACE");
    assert_eq!(section("RIGHT CLICK"), "CONTEXT SENSITIVE MOUSE");
    assert_eq!(section("SMALL FONTS"), "MISCELLANEOUS");
    assert_eq!(section("MOUSE SENSITIVITY"), "");
    let compass = option(&ui, "ShowCompass");
    click(&mut ui, &mut host, SIZE, "COMPASS");
    assert_ne!(option(&ui, "ShowCompass"), compass);
    let out = click(&mut ui, &mut host, SIZE, "ACCEPT!");
    assert!(out.contains(&GameRequest::ApplyOptions));
    assert_ne!(option(&ui, "ShowCompass"), compass);
    assert_eq!(screen(&ui), "Game_options");
}

#[test]
fn key_rebinding_by_double_click_and_by_return() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    to_screen(&mut ui, &mut host, &["OPTIONS", "KEY BINDINGS"]);
    assert_eq!(screen(&ui), "Keys");
    let (m, l) = now(&ui, SIZE);
    let li = m
        .by_atom(&ui.fe, "OP_KeyBindingsList")
        .expect("the key list");
    let Kind::List {
        rows, total, page, ..
    } = &m.items[li].kind
    else {
        panic!()
    };
    assert_eq!(*total, 147);
    assert_eq!(rows.len(), *page);
    let dock = rows
        .iter()
        .position(|r| r.keys.as_ref().is_some_and(|k| k.action == "DOCK SHIPS"))
        .expect("the dock row");
    assert_eq!(rows[dock].keys.as_ref().unwrap().keys, ["D"]);
    // A double click on the row waits for a key.
    let (x, y) = centre(l.placed[li].parts[dock]);
    settle(&mut ui, &mut host);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y });
    for _ in 0..2 {
        press(&mut ui, &mut host, true);
        press(&mut ui, &mut host, false);
    }
    assert_eq!(ui.state().binding, Some(keys::DOCK));
    let (m, _) = now(&ui, SIZE);
    let Kind::List { rows, .. } = &m.items[li].kind else {
        panic!()
    };
    assert!(rows[dock].keys.as_ref().unwrap().binding);
    shot(&mut ui, SIZE, "mock_keys_binding");
    key(&mut ui, &mut host, u16::from(b'K'));
    assert_eq!(ui.state().binding, None);
    assert!(ui
        .state()
        .keys
        .of(keys::DOCK)
        .contains(&keys::Binding { key: 75, mods: 0 }));
    // Return on the selected row waits for a key too.
    key(&mut ui, &mut host, key::RETURN);
    assert_eq!(ui.state().binding, Some(keys::DOCK));
    key(&mut ui, &mut host, u16::from(b'J'));
    assert!(ui
        .state()
        .keys
        .of(keys::DOCK)
        .contains(&keys::Binding { key: 74, mods: 0 }));
    // The wheel and the scroll bar move the original list.
    let upper = |ui: &InGameUi| {
        let a = ui.fe.find_atom("OP_KeyBindingsList").unwrap();
        ui.fe.list(a).unwrap().upper
    };
    let before = upper(&ui);
    input(&mut ui, &mut host, InputEvent::Wheel { up: false });
    assert_eq!(upper(&ui), before + 3);
    let (m, l) = now(&ui, SIZE);
    let Kind::List {
        total, page, top, ..
    } = &m.items[li].kind
    else {
        panic!()
    };
    let page = *page;
    let th = l.thumb(li, *total, page, *top).unwrap();
    let track = l.placed[li].track.unwrap();
    settle(&mut ui, &mut host);
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: th.x0 + 1,
            y: th.y0 + 2,
        },
    );
    press(&mut ui, &mut host, true);
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: th.x0 + 1,
            y: track.y1 + 50,
        },
    );
    press(&mut ui, &mut host, false);
    assert_eq!(upper(&ui), 147 - page, "dragged to the end");
    click_at(&mut ui, &mut host, th.x0 + 1, track.y0 + 2);
    assert_eq!(upper(&ui), 147 - 2 * page, "a click on the track pages");
    // Reset to default: the bindings as saved.
    click(&mut ui, &mut host, SIZE, "RESET TO DEFAULT");
    assert_eq!(
        ui.state().keys.of(keys::DOCK),
        &[keys::Binding { key: 68, mods: 0 }]
    );
    key(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(screen(&ui), "Game_options");
}

#[test]
fn save_entry_types_and_saves() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    click(&mut ui, &mut host, SIZE, "SAVE GAME");
    assert_eq!(screen(&ui), "In_game_save");
    for c in "Fleet One".chars() {
        let code = c.to_ascii_uppercase() as u16;
        ui.handle_input(&host, InputEvent::KeyDown(code));
        ui.handle_input(&host, InputEvent::Char(c));
        frame(&mut ui, &mut host);
        input(&mut ui, &mut host, InputEvent::KeyUp(code));
    }
    let (m, _) = now(&ui, SIZE);
    let e = m.by_atom(&ui.fe, "IG_TextEntryWindowInit").unwrap();
    assert_eq!(
        m.items[e].kind,
        Kind::Entry {
            text: "Fleet One".into(),
            focused: true
        }
    );
    // The typing went to the entry, not to the hot keys (S, D, B).
    assert_eq!(screen(&ui), "In_game_save");
    let out = click(&mut ui, &mut host, SIZE, "SAVE GAME");
    assert!(
        out.contains(&GameRequest::SaveGame("Fleet One".into())),
        "{out:?}"
    );
    assert_eq!(ui.manager(), None);
}

#[test]
fn confirmations_game_over_and_objectives() {
    let Some((mut ui, mut host)) = opened(Options::default()) else {
        return;
    };
    click(&mut ui, &mut host, SIZE, "QUIT");
    assert_eq!(screen(&ui), "Quit_game2");
    let (m, l) = now(&ui, SIZE);
    assert_eq!(m.title, "QUIT GAME?");
    assert_eq!(
        l.placed[0].control.y0, l.placed[1].control.y0,
        "side by side"
    );
    click(&mut ui, &mut host, SIZE, "NO");
    assert_eq!(screen(&ui), "In_game_esc_menu2");
    click(&mut ui, &mut host, SIZE, "QUIT");
    let out = click(&mut ui, &mut host, SIZE, "YES");
    assert!(out.contains(&GameRequest::QuitToMain));
    // Game over: restart.
    ui.game_over(&host);
    let (m, _) = now(&ui, SIZE);
    assert_eq!(m.title, "GAME OVER");
    let out = click(&mut ui, &mut host, SIZE, "RESTART LEVEL");
    assert!(out.contains(&GameRequest::RestartLevel));
    // Fleet intelligence: its text; OKAY closes it.
    hwc_ui::ingame::escmenu::open_objectives(&mut ui.fe, None);
    frame(&mut ui, &mut host);
    let (m, _) = now(&ui, SIZE);
    assert_eq!(m.title, "FLEET INTELLIGENCE");
    assert!(m
        .items
        .iter()
        .any(|i| matches!(&i.kind, Kind::Text { paragraphs } if !paragraphs.is_empty())));
    click(&mut ui, &mut host, SIZE, "OKAY");
    assert_eq!(ui.manager(), None);
}

#[test]
fn modern_options_wait_for_accept() {
    let mut o = Options::default();
    o.set("hwcMsaa", 1);
    let Some((mut ui, mut host)) = opened(o) else {
        return;
    };
    ui.lock_option("hwcSmoothZoom", 0);
    click(&mut ui, &mut host, SIZE, "OPTIONS");
    // E opens it from the hub.
    key(&mut ui, &mut host, u16::from(b'E'));
    assert!(ui.state().mmenu.page.is_some());
    let (m, _) = now(&ui, SIZE);
    assert_eq!(m.name, mo::NAME);
    let smooth = m.find("SMOOTH WHEEL ZOOM").unwrap();
    assert!(m.items[smooth].disabled, "set on the command line");
    assert_eq!(m.items[smooth].kind, Kind::Toggle { on: false });
    assert_eq!(
        m.items[smooth].note.as_deref(),
        Some("SET ON THE COMMAND LINE")
    );
    shot(&mut ui, SIZE, "mock_enhancements_locked");
    // FXAA on, 4x MSAA, the render scale to the top; the locked one stays.
    click(&mut ui, &mut host, SIZE, "FXAA EDGE SMOOTHING");
    let (i, _, l) = item(&ui, SIZE, "ANTI-ALIASING (MSAA)");
    let (x, y) = centre(l.placed[i].parts[2]);
    click_at(&mut ui, &mut host, x, y);
    drag_slider(&mut ui, &mut host, "RENDER SCALE", 1.0);
    click(&mut ui, &mut host, SIZE, "SMOOTH WHEEL ZOOM");
    let page = ui.state().mmenu.page.clone().unwrap();
    let k = |key: &str| mo::ENTRIES.iter().position(|e| e.key == key).unwrap();
    assert_eq!(page.values[k("hwcFxaa")], 1);
    assert_eq!(page.values[k("hwcMsaa")], 4);
    assert_eq!(page.values[k("hwcRenderScale")], 200);
    assert_eq!(
        page.values[k("hwcSmoothZoom")],
        0,
        "a locked option does not change"
    );
    assert_eq!(option(&ui, "hwcFxaa"), 0, "nothing is kept before ACCEPT");
    // Escape forgets them.
    key(&mut ui, &mut host, key::ESCAPE);
    assert!(ui.state().mmenu.page.is_none());
    assert_eq!(screen(&ui), "Game_options");
    assert_eq!(option(&ui, "hwcFxaa"), 0);
    // Again with the keyboard: Down to MSAA, Right to 2x, Down to FXAA, Space; Return
    // accepts.
    key(&mut ui, &mut host, u16::from(b'E'));
    key(&mut ui, &mut host, key::DOWN);
    key(&mut ui, &mut host, key::RIGHT);
    key(&mut ui, &mut host, key::DOWN);
    key(&mut ui, &mut host, key::SPACE);
    let page = ui.state().mmenu.page.clone().unwrap();
    assert_eq!(page.values[k("hwcMsaa")], 2);
    assert_eq!(page.values[k("hwcFxaa")], 1);
    let out = key(&mut ui, &mut host, key::RETURN);
    assert!(ui.state().mmenu.page.is_none());
    assert!(out.contains(&GameRequest::ApplyOptions));
    assert_eq!(option(&ui, "hwcMsaa"), 2);
    assert_eq!(option(&ui, "hwcFxaa"), 1);
    assert_eq!(
        option(&ui, "hwcSmoothZoom"),
        1,
        "the locked one is left as it was"
    );
    // DEFAULTS puts them back (until accepted).
    click(&mut ui, &mut host, SIZE, mo::HUB_LABEL);
    click(&mut ui, &mut host, SIZE, "DEFAULTS");
    let page = ui.state().mmenu.page.clone().unwrap();
    assert_eq!(page.values[k("hwcMsaa")], 1);
    click(&mut ui, &mut host, SIZE, "ACCEPT");
    assert_eq!(option(&ui, "hwcMsaa"), 1);
    // Turning the modern menus off brings the original screens back at once.
    click(&mut ui, &mut host, SIZE, mo::HUB_LABEL);
    click(&mut ui, &mut host, SIZE, "MODERN MENUS AND SYSTEMS MANAGER");
    click(&mut ui, &mut host, SIZE, "ACCEPT");
    assert!(!menu::active(&ui.fe));
    assert_eq!(screen(&ui), "Game_options");
}

#[test]
fn the_original_screens_with_the_switch_off() {
    let mut o = Options::default();
    o.set_bool(menu::OPTION_KEY, false);
    let Some((mut ui, mut host)) = opened(o) else {
        return;
    };
    assert!(!menu::active(&ui.fe));
    // A click on the original's button where the original draws it.
    let a = ui.fe.find_atom("OP_InGameOptions_Start").unwrap();
    let r = ui.fe.atom(a).rect();
    click_at(&mut ui, &mut host, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    assert_eq!(screen(&ui), "Game_options");
    shot(&mut ui, SIZE, "mock_original_menus");
    // The original HUD brings them back too.
    let mut o = Options::default();
    o.set_bool(hwc_ui::ingame::modern_hud::OPTION_KEY, false);
    let Some((ui, _)) = opened(o) else { return };
    assert!(!menu::active(&ui.fe));
}
