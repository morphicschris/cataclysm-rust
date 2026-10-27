//! The options file is written when options are kept, as the original's
//! `opOptionsSaveSettings` writes `Cataclysm.cfg` and `keys.cfg`: an ACCEPT in each in-game
//! options screen (audio, video, gameplay, key bindings) and in the modern ENHANCEMENTS screen,
//! and a HUD panel collapsed, are on disk at once; a CANCEL writes nothing; an option the
//! command line set for this run is not written; a new session from the file starts with what
//! was kept. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::Rect;
use hwc_ui::consts::key;
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::keys;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_menu::{self as menu, Hit, Kind, Layout, Model};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{self, Session};
use hwc_ui::AssetSource;

const KEYS: &str = "v 84\nkey  Move  77\nkey  Dock  68\nkey  Escape  27\n\
key  Open_BuildManager  66\nkey  Open_ResearchManager  82\nkey  Open_LaunchManager  83\n";
const SIZE: (i32, i32) = (1920, 1080);

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

/// A fresh directory for one test's options and key files.
fn dir(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/options-saved")
        .join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

struct Files {
    options: PathBuf,
    keys: PathBuf,
}

impl Files {
    fn new(name: &str) -> Files {
        let d = dir(name);
        Files {
            options: d.join("hwc-options.cfg"),
            keys: d.join("keys.cfg"),
        }
    }

    fn saved(&self) -> Options {
        assert!(self.options.is_file(), "the options file was written");
        Options::load(&self.options).unwrap()
    }

    fn text(&self) -> Option<String> {
        std::fs::read_to_string(&self.options).ok()
    }
}

fn opened(src: Rc<dyn AssetSource>, files: &Files, options: Options) -> (InGameUi, MockHost) {
    let mut ui = InGameUi::new(
        src,
        Setup {
            options,
            default_keys: Some(KEYS.into()),
            keys: None,
            keys_path: Some(files.keys.clone()),
            options_path: Some(files.options.clone()),
        },
    )
    .expect("in-game ui");
    ui.resize(SIZE.0, SIZE.1);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key_press(&mut ui, &mut host, key::ESCAPE);
    assert_eq!(ui.manager(), Some(Manager::EscMenu));
    (ui, host)
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

fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    let mut out = input(ui, host, InputEvent::MouseMove { x, y });
    out.extend(press(ui, host, true));
    out.extend(press(ui, host, false));
    out
}

fn key_press(ui: &mut InGameUi, host: &mut MockHost, code: u16) -> Vec<GameRequest> {
    let mut out = input(ui, host, InputEvent::KeyDown(code));
    out.extend(input(ui, host, InputEvent::KeyUp(code)));
    out
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn item(ui: &InGameUi, label: &str) -> (usize, Model, Layout) {
    let m = menu::model(&ui.fe).expect("a modern screen");
    let l = menu::layout(&m, SIZE.0, SIZE.1);
    let i = m.find(label).unwrap_or_else(|| {
        panic!(
            "no {label:?} on {:?}: {:?}",
            m.name,
            m.items.iter().map(|i| i.label.clone()).collect::<Vec<_>>()
        )
    });
    (i, m, l)
}

fn click(ui: &mut InGameUi, host: &mut MockHost, label: &str) -> Vec<GameRequest> {
    let (i, m, l) = item(ui, label);
    let r = match m.items[i].kind {
        Kind::Toggle { .. } => l.placed[i].row,
        _ => l.placed[i].control,
    };
    let (x, y) = match m.items[i].kind {
        Kind::Toggle { .. } => (r.x0 + 30, centre(r).1),
        _ => centre(r),
    };
    assert_eq!(
        menu::hit(&m, &l, None, SIZE.1, x, y),
        Hit::Item(i),
        "{label}"
    );
    click_at(ui, host, x, y)
}

fn to_screen(ui: &mut InGameUi, host: &mut MockHost, path: &[&str]) {
    for b in path {
        click(ui, host, b);
    }
}

fn drag_slider(ui: &mut InGameUi, host: &mut MockHost, label: &str, fraction: f32) {
    let (i, m, l) = item(ui, label);
    let Kind::Slider { fraction: now, .. } = m.items[i].kind else {
        panic!("{label} is not a slider");
    };
    let t = l.placed[i].control;
    let y = (t.y0 + t.y1) / 2;
    let at = |f: f32| t.x0 + (t.width() as f32 * f).round() as i32;
    settle(ui, host);
    input(ui, host, InputEvent::MouseMove { x: at(now), y });
    press(ui, host, true);
    for k in 1..=6 {
        let f = now + (fraction - now) * k as f32 / 6.0;
        input(ui, host, InputEvent::MouseMove { x: at(f), y });
    }
    press(ui, host, false);
}

fn option(ui: &InGameUi, key: &str) -> i32 {
    ui.fe.game.options.get(key)
}

/// A new front-end session reading the options from `path` (a restart of the game).
fn restarted(src: Rc<dyn AssetSource>, path: &Path) -> Session {
    Session::new(
        src,
        session::Setup {
            options_path: Some(path.to_path_buf()),
            original_cfg: None,
            default_keys: Some(KEYS.into()),
            keys: std::fs::read_to_string(path.with_file_name("keys.cfg")).ok(),
            keys_path: Some(path.with_file_name("keys.cfg")),
            scale: ScaleMode::Original,
        },
    )
    .expect("session")
}

#[test]
fn every_in_game_options_screen_writes_on_accept() {
    let Some(src) = source() else { return };
    let files = Files::new("accept");
    let (mut ui, mut host) = opened(src.clone(), &files, Options::default());
    click(&mut ui, &mut host, "OPTIONS");
    assert!(files.text().is_none(), "opening the options writes nothing");

    // Audio.
    click(&mut ui, &mut host, "AUDIO");
    drag_slider(&mut ui, &mut host, "SPEECH", 0.3);
    let speech = option(&ui, "SpeechVolume");
    assert_ne!(speech, Options::default().get("SpeechVolume"));
    click(&mut ui, &mut host, "ACCEPT!");
    assert_eq!(files.saved().get("SpeechVolume"), speech, "audio");

    // Video.
    click(&mut ui, &mut host, "VIDEO");
    let nolod = option(&ui, "noLOD");
    click(&mut ui, &mut host, "FULL DETAIL");
    click(&mut ui, &mut host, "ACCEPT!");
    assert_ne!(files.saved().get("noLOD"), nolod, "video");

    // Gameplay.
    click(&mut ui, &mut host, "GAMEPLAY");
    let compass = option(&ui, "ShowCompass");
    click(&mut ui, &mut host, "COMPASS");
    click(&mut ui, &mut host, "ACCEPT!");
    assert_ne!(files.saved().get("ShowCompass"), compass, "gameplay");

    // Key bindings: Dock to K, by double click on its row.
    click(&mut ui, &mut host, "KEY BINDINGS");
    let m = menu::model(&ui.fe).unwrap();
    let l = menu::layout(&m, SIZE.0, SIZE.1);
    let li = m.by_atom(&ui.fe, "OP_KeyBindingsList").unwrap();
    let Kind::List { rows, .. } = &m.items[li].kind else {
        panic!()
    };
    let dock = rows
        .iter()
        .position(|r| r.keys.as_ref().is_some_and(|k| k.action == "DOCK SHIPS"))
        .unwrap();
    let (x, y) = centre(l.placed[li].parts[dock]);
    settle(&mut ui, &mut host);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y });
    for _ in 0..2 {
        press(&mut ui, &mut host, true);
        press(&mut ui, &mut host, false);
    }
    key_press(&mut ui, &mut host, u16::from(b'K'));
    click(&mut ui, &mut host, "ACCEPT!");
    let saved = keys::KeyBindings::parse(&std::fs::read_to_string(&files.keys).unwrap());
    assert!(
        saved
            .of(keys::DOCK)
            .contains(&keys::Binding { key: 75, mods: 0 }),
        "keys"
    );

    // ENHANCEMENTS.
    key_press(&mut ui, &mut host, u16::from(b'E'));
    assert!(ui.state().mmenu.page.is_some());
    click(&mut ui, &mut host, "FXAA EDGE SMOOTHING");
    assert_eq!(files.saved().get("hwcFxaa"), 0, "nothing before ACCEPT");
    click(&mut ui, &mut host, "ACCEPT");
    assert_eq!(files.saved().get("hwcFxaa"), 1, "enhancements");

    // A HUD panel collapsed (kept at once).
    ui.fe.game.set_option_now("hwcObjectivesExpanded", 0);
    assert_eq!(files.saved().get("hwcObjectivesExpanded"), 0, "panel state");

    // A restart: a new session from the file has every one of them, and the in-game
    // interface of the next mission starts from it.
    let s = restarted(src.clone(), &files.options);
    let o = &s.fe.game.options;
    assert_eq!(o.get("SpeechVolume"), speech);
    assert_ne!(o.get("noLOD"), nolod);
    assert_ne!(o.get("ShowCompass"), compass);
    assert_eq!(o.get("hwcFxaa"), 1);
    assert!(s
        .fe
        .game
        .ig
        .keys
        .of(keys::DOCK)
        .contains(&keys::Binding { key: 75, mods: 0 }));
    let files2 = Files::new("accept-next");
    let (next, _) = opened(src, &files2, s.fe.game.options.clone());
    assert_eq!(option(&next, "hwcFxaa"), 1);
    assert_eq!(option(&next, "SpeechVolume"), speech);
}

#[test]
fn a_cancel_writes_nothing() {
    let Some(src) = source() else { return };
    let files = Files::new("cancel");
    let (mut ui, mut host) = opened(src, &files, Options::default());
    to_screen(&mut ui, &mut host, &["OPTIONS", "AUDIO"]);
    drag_slider(&mut ui, &mut host, "SPEECH", 0.3);
    click(&mut ui, &mut host, "CANCEL");
    assert!(files.text().is_none(), "audio cancelled: no file");
    // With a file there, a cancel leaves it as it was.
    click(&mut ui, &mut host, "GAMEPLAY");
    click(&mut ui, &mut host, "COMPASS");
    click(&mut ui, &mut host, "ACCEPT!");
    let before = files.text().unwrap();
    click(&mut ui, &mut host, "VIDEO");
    click(&mut ui, &mut host, "FULL DETAIL");
    click(&mut ui, &mut host, "CANCEL");
    assert_eq!(files.text().unwrap(), before, "video cancelled");
    key_press(&mut ui, &mut host, u16::from(b'E'));
    click(&mut ui, &mut host, "FXAA EDGE SMOOTHING");
    key_press(&mut ui, &mut host, key::ESCAPE);
    assert!(ui.state().mmenu.page.is_none());
    assert_eq!(files.text().unwrap(), before, "enhancements escaped");
    // A save while a screen is open (the game quitting) writes only what was accepted.
    click(&mut ui, &mut host, "AUDIO");
    drag_slider(&mut ui, &mut host, "SPEECH", 0.9);
    ui.fe.game.save_options();
    assert_eq!(files.text().unwrap(), before, "an open screen's change");
    assert!(!files.options.with_file_name("hwc-options.cfg.tmp").exists());
}

#[test]
fn a_command_line_option_is_not_written() {
    let Some(src) = source() else { return };
    let files = Files::new("locked");
    let mut o = Options::default();
    o.set("hwcSmoothZoom", 1);
    let (mut ui, mut host) = opened(src, &files, o);
    // `--smooth-zoom off` for this run.
    ui.lock_option("hwcSmoothZoom", 0);
    // A display the command line asked for (`--windowed`): in use, not in the options.
    let in_use = hwc_ui::display::Setting {
        mode: hwc_ui::display::Mode::Windowed,
        ..Default::default()
    };
    ui.set_display(None, in_use);
    click(&mut ui, &mut host, "OPTIONS");
    key_press(&mut ui, &mut host, u16::from(b'E'));
    click(&mut ui, &mut host, "FXAA EDGE SMOOTHING");
    click(&mut ui, &mut host, "ACCEPT");
    let saved = files.saved();
    assert_eq!(saved.get("hwcFxaa"), 1);
    assert_eq!(
        saved.get("hwcSmoothZoom"),
        1,
        "the file's value, not the override"
    );
    assert_eq!(
        saved.get(hwc_ui::display::MODE_KEY),
        Options::default().get(hwc_ui::display::MODE_KEY),
        "the command line's screen mode is not kept"
    );
}

/// The mission's kept options go back to the front end when it ends (`gameEnd`), and only
/// what was accepted.
#[test]
fn a_mission_hands_its_kept_options_back() {
    let Some(src) = source() else { return };
    let files = Files::new("adopt");
    let mut front = restarted(src.clone(), &files.options);
    let (mut ui, mut host) = opened(src, &files, front.fe.game.options.clone());
    to_screen(&mut ui, &mut host, &["OPTIONS", "GAMEPLAY"]);
    let compass = option(&ui, "ShowCompass");
    click(&mut ui, &mut host, "COMPASS");
    click(&mut ui, &mut host, "ACCEPT!");
    click(&mut ui, &mut host, "AUDIO");
    let speech = option(&ui, "SpeechVolume");
    drag_slider(&mut ui, &mut host, "SPEECH", 0.9);
    front.fe.game.adopt_kept(&ui.fe.game);
    front.fe.game.save_options();
    assert_ne!(front.fe.game.options.get("ShowCompass"), compass);
    assert_eq!(front.fe.game.options.get("SpeechVolume"), speech);
    let saved = files.saved();
    assert_ne!(saved.get("ShowCompass"), compass);
    assert_eq!(saved.get("SpeechVolume"), speech);
}
