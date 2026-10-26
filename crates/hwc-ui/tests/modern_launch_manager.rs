//! The modern systems manager (the launch manager under `hwcModernMenus = 1`, the default) on
//! the real game files with the mock host: it replaces the original's look while the
//! original screen stays open underneath for its keys and its list; its launcher tabs,
//! REMAIN DOCKED switches, docked-ship rows, scroll bar, repair-priority buttons, the
//! Beast's emergency power choices and its buttons act through the original's regions;
//! BUILD / RESEARCH switch to the other managers; one Escape closes it; with the switch off
//! the original screen comes back. Screenshots go to `screenshots/menus/mock_systems_*.png`
//! when `HWC_SCREENSHOTS` is set. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::{MockHost, CARRIER_1, MOTHERSHIP};
use hwc_ui::ingame::modern_launch_manager::{self as mlm, Button, Hit};
use hwc_ui::ingame::{modern_hud, modern_menu, modern_panel};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

const KEYS: &str = "v 84\nkey  Escape  27\nkey  Open_BuildManager  66\n\
key  Open_ResearchManager  82\nkey  Open_LaunchManager  83\n";

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

fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    let mut out = input(ui, host, InputEvent::MouseMove { x, y });
    for down in [true, false] {
        out.extend(input(
            ui,
            host,
            InputEvent::MouseButton { left: true, down },
        ));
    }
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

fn click(ui: &mut InGameUi, host: &mut MockHost, r: Rect) -> Vec<GameRequest> {
    let (x, y) = centre(r);
    click_at(ui, host, x, y)
}

fn background(size: (i32, i32)) -> Canvas {
    let mut canvas = Canvas::new(size.0, size.1);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let t = x as f32 / size.0 as f32;
            let u = y as f32 / size.1 as f32;
            canvas.pixels[(y * size.0 + x) as usize] =
                [(40.0 + 60.0 * t) as u8, (70.0 + 50.0 * u) as u8, 130, 255];
        }
    }
    canvas
}

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) {
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
}

fn rest(ui: &mut InGameUi, host: &mut MockHost, size: (i32, i32), l: &mlm::Layout) {
    input(
        ui,
        host,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: l.frame.panel.y1 - 3,
        },
    );
    settle(ui, host);
}

fn list_selected(ui: &InGameUi) -> Option<u32> {
    let a = ui.fe.find_atom("LM_ShipsToLaunch")?;
    let l = ui.fe.list(a)?;
    l.selected.map(|i| l.items[i].data)
}

fn row_of(ui: &InGameUi, ship_type: u32) -> usize {
    let a = ui.fe.find_atom("LM_ShipsToLaunch").unwrap();
    let l = ui.fe.list(a).unwrap();
    l.items
        .iter()
        .position(|i| i.can_select && i.data == ship_type)
        .expect("the row")
        - l.upper
}

fn run(size: (i32, i32)) {
    let Some(src) = source() else { return };
    let mut ui = ui_with(src, size, Options::default());
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, u16::from(b'S'));
    assert_eq!(ui.manager(), Some(Manager::Launch));
    assert!(mlm::active(&ui.fe));
    assert_eq!(ui.fe.current_screen_name(), Some("Launch_Manager"));
    let l = mlm::layout_now(&ui.fe);
    assert_eq!(
        l.modules.len(),
        host.modules.len(),
        "the Somtaaw module panel"
    );
    rest(&mut ui, &mut host, size, &l);
    shot(&mut ui, size, "mock_systems");
    // A docked type: its row selects it (the list window's own selection).
    let r = row_of(&ui, 0x01);
    click(&mut ui, &mut host, l.rows[r]);
    assert_eq!(list_selected(&ui), Some(0x01));
    rest(&mut ui, &mut host, size, &l);
    shot(&mut ui, size, "mock_systems_selected");
    // LAUNCH ONE, then LAUNCH GROUP: one acolyte, then the rest.
    let out = click(&mut ui, &mut host, l.button(Button::LaunchOne));
    let launched: Vec<_> = out
        .iter()
        .filter_map(|r| match r {
            GameRequest::Launch { launcher, ships } => Some((*launcher, ships.len())),
            _ => None,
        })
        .collect();
    assert_eq!(launched, [(MOTHERSHIP, 1)]);
    let out = click(&mut ui, &mut host, l.button(Button::Launch));
    assert!(out
        .iter()
        .any(|r| matches!(r, GameRequest::Launch { ships, .. } if ships.len() == 2)));
    // REMAIN DOCKED on the mothership: the check box asks for auto-launch off.
    let out = click(&mut ui, &mut host, l.stay[0]);
    assert!(
        out.contains(&GameRequest::SetAutoLaunch {
            launcher: MOTHERSHIP,
            on: true
        }) || out.contains(&GameRequest::SetAutoLaunch {
            launcher: MOTHERSHIP,
            on: false
        })
    );
    // A repair priority.
    let out = click(&mut ui, &mut host, l.modules[1].1[2]);
    assert!(
        out.iter()
            .any(|r| matches!(r, GameRequest::SetRepairPriority { priority: 2, .. })),
        "{out:?}"
    );
    // The carrier's tab: its docked ships.
    click(&mut ui, &mut host, l.tabs[1]);
    assert_eq!(ui.state().lm.launcher, Some(CARRIER_1));
    rest(&mut ui, &mut host, size, &l);
    shot(&mut ui, size, "mock_systems_carrier");
    let out = click(&mut ui, &mut host, l.button(Button::LaunchAll));
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Launch { launcher, .. } if *launcher == CARRIER_1
    )));
    // An empty carrier slot does nothing.
    click(&mut ui, &mut host, l.tabs[2]);
    assert_eq!(ui.state().lm.launcher, Some(CARRIER_1));
    // The modern HUD's panels keep clear of it.
    let view = ui.state().view.clone();
    let arr = modern_panel::arrange(&ui.fe, size.0, size.1, &view);
    for r in arr.rects() {
        assert!(!modern_hud::overlaps(r, l.frame.panel), "{r:?} at {size:?}");
    }
    // BUILD and RESEARCH, and back with S; one Escape closes.
    click(&mut ui, &mut host, l.button(Button::Build));
    assert_eq!(ui.manager(), Some(Manager::Construction));
    key(&mut ui, &mut host, u16::from(b'S'));
    assert_eq!(ui.manager(), Some(Manager::Launch));
    click(&mut ui, &mut host, l.button(Button::Research));
    assert_eq!(ui.manager(), Some(Manager::Research));
    key(&mut ui, &mut host, u16::from(b'S'));
    assert_eq!(ui.manager(), Some(Manager::Launch));
    key(&mut ui, &mut host, 27);
    assert_eq!(ui.manager(), None, "one Escape closes it");
}

#[test]
fn systems_manager_1080() {
    run((1920, 1080));
}

#[test]
fn systems_manager_768() {
    run((1024, 768));
}

#[test]
fn systems_manager_480() {
    run((640, 480));
}

#[test]
fn hot_keys_scroll_bar_and_wheel() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui_with(src, size, Options::default());
    let mut host = MockHost::somtaaw();
    // Enough types for the list to scroll.
    let names = [
        (0x00, "RECON"),
        (0x02, "MIMIC"),
        (0x03, "ACOLYTE II"),
        (0x11, "LEECH"),
        (0x21, "SEEKER"),
        (0x22, "HUNTER"),
        (0x23, "STINGER"),
        (0x24, "SENTINEL"),
        (0x25, "VULTURE"),
        (0x04, "MULTIBEAM"),
        (0x05, "ACOLYTE MKII"),
        (0x26, "PHASER"),
    ];
    for (k, (t, n)) in names.iter().enumerate() {
        host.launchers[0].docked.push(DockedShip {
            ship: 100 + k as u32,
            ship_type: *t,
            class: if *t == 4 || *t == 5 { 5 } else { 6 },
            name: n.to_string(),
        });
    }
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, u16::from(b'S'));
    settle(&mut ui, &mut host);
    let upper = |ui: &InGameUi| {
        let a = ui.fe.find_atom("LM_ShipsToLaunch").unwrap();
        ui.fe.list(a).unwrap().upper
    };
    let l = mlm::layout_now(&ui.fe);
    let (x, y) = centre(l.rows[2]);
    input(&mut ui, &mut host, InputEvent::MouseMove { x, y });
    input(&mut ui, &mut host, InputEvent::Wheel { up: false });
    assert_eq!(upper(&ui), 3, "the original list's wheel step");
    input(&mut ui, &mut host, InputEvent::Wheel { up: true });
    assert_eq!(upper(&ui), 0);
    let a = ui.fe.find_atom("LM_ShipsToLaunch").unwrap();
    let total = ui.fe.list(a).unwrap().items.len();
    let th = l.thumb(total, 0).expect("a thumb");
    assert_eq!(
        mlm::hit(&l, th.x0 + 1, th.y0 + 2, l.rows.len(), total, 0),
        Hit::Thumb
    );
    settle(&mut ui, &mut host);
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: th.x0 + 1,
            y: th.y0 + 2,
        },
    );
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: th.x0 + 1,
            y: l.scroll_track.y1 + 40,
        },
    );
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    assert_eq!(upper(&ui), total - l.rows.len(), "dragged to the end");
    shot(&mut ui, size, "mock_systems_scrolled");
    // The keys stay the original screen's: A launches all.
    let out = key(&mut ui, &mut host, u16::from(b'A'));
    assert!(out.iter().any(|r| matches!(r, GameRequest::Launch { .. })));
}

#[test]
fn beast_emergency_power() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui_with(src, size, Options::default());
    let mut host = MockHost::beast();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, u16::from(b'S'));
    assert_eq!(ui.fe.current_screen_name(), Some("Beast_Launch_Manager"));
    let l = mlm::layout_now(&ui.fe);
    assert_eq!(l.power.len(), 6, "the six choices");
    assert!(l.modules.is_empty());
    rest(&mut ui, &mut host, size, &l);
    shot(&mut ui, size, "mock_systems_beast");
    // The first choice (THRUST) is the radio button of value 1.
    let out = click(&mut ui, &mut host, l.power[0]);
    assert!(out.contains(&GameRequest::BeastSystems(1)), "{out:?}");
}

#[test]
fn the_original_screen_with_the_switch_off() {
    let Some(src) = source() else { return };
    let mut o = Options::default();
    o.set_bool(modern_menu::OPTION_KEY, false);
    let mut ui = ui_with(src, (1920, 1080), o);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, u16::from(b'S'));
    assert_eq!(ui.manager(), Some(Manager::Launch));
    assert!(!mlm::active(&ui.fe));
    // The original's button where the original draws it.
    let a = ui.fe.find_atom("LM_LaunchAll").unwrap();
    let r = ui.fe.atom(a).rect();
    let out = click(&mut ui, &mut host, r);
    assert!(out.iter().any(|r| matches!(r, GameRequest::Launch { .. })));
}
