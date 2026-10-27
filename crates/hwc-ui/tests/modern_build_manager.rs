//! The modern build manager (`hwcModernBuildManager = 1`, the default) on the real game files with
//! the mock host: it replaces the original's look (the screen stays open underneath for its
//! keys), its rows, - / + controls, tabs and buttons act like the original's regions (double
//! clicks included), one Escape closes it, and the modern HUD's panels keep clear of it.
//! Screenshots go to `screenshots/buildui/mock_*.png`. Needs `HWC_DATA_DIR` or
//! `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::ingame::consmgr::{self, Row};
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::{MockHost, CARRIER_1, MOTHERSHIP};
use hwc_ui::ingame::modern_build_manager::{self as mbm, Button, Hit};
use hwc_ui::ingame::{modern_hud, modern_objectives, modern_panel};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

const KEYS: &str = "v 84\nkey  Escape  27\nkey  Open_BuildManager  66\n";

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn ui(src: Rc<dyn AssetSource>, size: (i32, i32)) -> InGameUi {
    let mut options = Options::default();
    options.set_bool(mbm::OPTION_KEY, true);
    let mut ui = InGameUi::new(
        src,
        Setup {
            options,
            default_keys: Some(KEYS.into()),
            keys: None,
            keys_path: None,
            options_path: None,
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

fn press(ui: &mut InGameUi, host: &mut MockHost, left: bool, down: bool) -> Vec<GameRequest> {
    ui.handle_input(host, InputEvent::MouseButton { left, down });
    frame(ui, host)
}

/// A click as a player would make it: away from any earlier click's double-click time.
fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    out.extend(press(ui, host, true, true));
    out.extend(press(ui, host, true, false));
    out
}

fn right_click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    frame(ui, host);
    let mut out = press(ui, host, false, true);
    out.extend(press(ui, host, false, false));
    out
}

fn key(ui: &mut InGameUi, host: &mut MockHost, code: u16) -> Vec<GameRequest> {
    ui.handle_input(host, InputEvent::KeyDown(code));
    let mut out = frame(ui, host);
    ui.handle_input(host, InputEvent::KeyUp(code));
    out.extend(frame(ui, host));
    out
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

/// The on-screen row of a ship type.
fn row_of(ui: &InGameUi, t: ShipType) -> usize {
    let top = ui.state().cm.top_row;
    consmgr::rows(&ui.state().view)
        .iter()
        .position(|r| matches!(r, Row::Ship(o) if o.ship_type == t))
        .unwrap()
        - top
}

fn background(size: (i32, i32)) -> Canvas {
    let mut canvas = Canvas::new(size.0, size.1);
    for y in 0..size.1 {
        for x in 0..size.0 {
            let t = x as f32 / size.0 as f32;
            let u = y as f32 / size.1 as f32;
            canvas.pixels[(y * size.0 + x) as usize] =
                [(170.0 - 60.0 * t) as u8, (90.0 + 50.0 * u) as u8, 40, 255];
        }
    }
    canvas
}

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) -> Canvas {
    let mut canvas = background(size);
    ui.draw(&mut canvas);
    if std::env::var_os("HWC_SCREENSHOTS").is_some() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/buildui");
        std::fs::create_dir_all(&dir).unwrap();
        canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
    }
    canvas
}

fn host() -> MockHost {
    let mut host = MockHost::somtaaw();
    host.objectives.push(Objective {
        text: "Defend the Kuun-Lan".into(),
        description: String::new(),
        hyperspace: false,
        primary: true,
        complete: false,
    });
    host
}

fn layout(size: (i32, i32)) -> mbm::Layout {
    mbm::layout(size.0, size.1)
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 66);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(mbm::active(&ui.fe));
    assert_eq!(
        ui.fe.current_screen_name(),
        Some("Construction_manager"),
        "the original screen stays open for its keys"
    );
    let l = layout(size);
    // The original's frame is not drawn: inside the panel the accent strip, outside it the
    // dimmed scene.
    let c = shot(&mut ui, size, &format!("mock_open_{tag}"));
    assert_eq!(
        c.get(l.panel.x0, l.panel.y0 + 40),
        modern_objectives::ACCENT
    );
    let bg = background(size);
    let (ox, oy) = (l.panel.x0 - 30, l.panel.y1 - 5);
    if ox > 0 {
        assert!(c.get(ox, oy)[0] < bg.get(ox, oy)[0], "the scene is dimmed");
    }

    // A row: the first click selects, the next adds one (picked, not yet sent).
    let r = row_of(&ui, 0);
    let (_, y) = centre(l.row(r));
    click_at(&mut ui, &mut host, l.name_x + 10, y);
    assert!(ui.state().cm.selected.contains(&0));
    assert_eq!(ui.state().cm.pending.get(&0).copied().unwrap_or(0), 0);
    click_at(&mut ui, &mut host, l.name_x + 10, y);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&1));
    assert_eq!(ui.state().cm.ship_view, Some(0));
    // + adds, - takes away; a held + repeats every quarter second.
    let (px, py) = centre(l.plus(r));
    click_at(&mut ui, &mut host, px, py);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&2));
    let (mx, my) = centre(l.minus(r));
    click_at(&mut ui, &mut host, mx, my);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&1));
    settle(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::MouseMove { x: px, y: py });
    press(&mut ui, &mut host, true, true);
    for _ in 0..16 {
        frame(&mut ui, &mut host);
    }
    press(&mut ui, &mut host, true, false);
    let held = ui.state().cm.pending.get(&0).copied().unwrap();
    assert!((3..=4).contains(&held), "held + for half a second: {held}");
    // A double click on + adds once: its second press is the double-click message, which
    // the arrows do not see (as in the original).
    let before = held;
    settle(&mut ui, &mut host);
    press(&mut ui, &mut host, true, true);
    press(&mut ui, &mut host, true, false);
    press(&mut ui, &mut host, true, true);
    press(&mut ui, &mut host, true, false);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&(before + 1)));
    // The list sees it: a double click on a selected row adds two.
    let before = before + 1;
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: l.name_x + 10,
            y,
        },
    );
    settle(&mut ui, &mut host);
    press(&mut ui, &mut host, true, true);
    press(&mut ui, &mut host, true, false);
    press(&mut ui, &mut host, true, true);
    press(&mut ui, &mut host, true, false);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&(before + 2)));
    // A right click takes one away.
    right_click_at(&mut ui, &mut host, l.name_x + 10, y);
    assert_eq!(ui.state().cm.pending.get(&0), Some(&(before + 1)));
    // Adding to the acolytes in production goes straight into the factory's queue.
    let a = row_of(&ui, 1);
    let (px, py) = centre(l.plus(a));
    let out = click_at(&mut ui, &mut host, px, py);
    assert!(
        out.contains(&GameRequest::Build {
            factory: MOTHERSHIP,
            ship_type: 1,
            count: 1
        }),
        "{out:?}"
    );
    // A greyed ship is refused (and flashes).
    let g = row_of(&ui, 2);
    let (px, py) = centre(l.plus(g));
    click_at(&mut ui, &mut host, px, py);
    assert_eq!(ui.state().cm.pending.get(&2).copied().unwrap_or(0), 0);
    // A frigate picked too, the ship view on it, Info on.
    let f = row_of(&ui, 7);
    let (px, py) = centre(l.plus(f));
    click_at(&mut ui, &mut host, px, py);
    assert_eq!(ui.state().cm.pending.get(&7), Some(&1));
    // A second needs more support units than are free: refused.
    click_at(&mut ui, &mut host, px, py);
    assert_eq!(ui.state().cm.pending.get(&7), Some(&1));
    ui.handle_input(&host, InputEvent::MouseMove { x: px, y: py });
    frame(&mut ui, &mut host);
    shot(&mut ui, size, &format!("mock_picked_{tag}"));
    let (ix, iy) = centre(l.buttons[&Button::Info]);
    click_at(&mut ui, &mut host, ix, iy);
    assert!(ui.state().cm.extended_info);
    shot(&mut ui, size, &format!("mock_info_{tag}"));
    // The I key is the screen's own: it toggles back.
    key(&mut ui, &mut host, b'I' as u16);
    assert!(!ui.state().cm.extended_info);

    // The modern HUD's panels keep clear of the panel.
    let view = ui.state().view.clone();
    let arr = modern_panel::arrange(&ui.fe, size.0, size.1, &view);
    for r in arr.rects() {
        assert!(
            !modern_hud::overlaps(r, l.panel),
            "{r:?} over {:?}",
            l.panel
        );
    }

    // Build sends the picked counts (the button: pressed, then released over it).
    let (bx, by) = centre(l.buttons[&Button::Build]);
    let out = click_at(&mut ui, &mut host, bx, by);
    let picked = before + 1;
    assert!(
        out.contains(&GameRequest::Build {
            factory: MOTHERSHIP,
            ship_type: 0,
            count: picked
        }),
        "{out:?}"
    );
    assert!(out.contains(&GameRequest::Sound(hwc_ui::consts::sound::CLICK)));
    assert!(ui.state().cm.pending.is_empty());
    // A press dragged off the button does nothing.
    settle(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::MouseMove { x: bx, y: by });
    press(&mut ui, &mut host, true, true);
    ui.handle_input(&host, InputEvent::MouseMove { x: bx, y: by - 200 });
    let out = press(&mut ui, &mut host, true, false);
    assert!(!out.iter().any(|r| matches!(r, GameRequest::Build { .. })));

    // Pause and cancel the selected acolytes.
    let a = row_of(&ui, 1);
    let (_, ay) = centre(l.row(a));
    click_at(&mut ui, &mut host, l.name_x + 10, ay);
    assert!(ui.state().cm.selected.contains(&1));
    let (x, y) = centre(l.buttons[&Button::Pause]);
    let out = click_at(&mut ui, &mut host, x, y);
    assert!(out.contains(&GameRequest::PauseBuild {
        factory: MOTHERSHIP,
        ship_type: 1,
        paused: true
    }));
    shot(&mut ui, size, &format!("mock_paused_{tag}"));
    // The screen's P key resumes.
    let out = key(&mut ui, &mut host, b'P' as u16);
    assert!(out.contains(&GameRequest::PauseBuild {
        factory: MOTHERSHIP,
        ship_type: 1,
        paused: false
    }));
    let (x, y) = centre(l.buttons[&Button::Cancel]);
    let out = click_at(&mut ui, &mut host, x, y);
    assert!(out.contains(&GameRequest::CancelBuild {
        factory: MOTHERSHIP,
        ship_type: 1
    }));

    // The carrier tab; the empty second carrier slot does nothing.
    let (x, y) = centre(l.tabs[2]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.factory, Some(MOTHERSHIP));
    let (x, y) = centre(l.tabs[1]);
    let out = click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.factory, Some(CARRIER_1));
    assert!(out.iter().any(
        |r| matches!(r, GameRequest::Speech { event, .. } if *event == consmgr::SPEECH_FACTORY)
    ));
    shot(&mut ui, size, &format!("mock_carrier_{tag}"));
    let (x, y) = centre(l.tabs[0]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.factory, Some(MOTHERSHIP));

    // The wheel scrolls the list (it is longer than a page at no size here, so it stays).
    let (x, y) = centre(l.list);
    ui.handle_input(&host, InputEvent::MouseMove { x, y });
    ui.handle_input(&host, InputEvent::Wheel { up: false });
    frame(&mut ui, &mut host);
    let total = consmgr::rows(&ui.state().view).len();
    assert_eq!(
        ui.state().cm.top_row,
        if total > l.page {
            3.min(total - l.page)
        } else {
            0
        }
    );

    // Research swaps the manager; S from there is the research manager's business.
    let (x, y) = centre(l.buttons[&Button::Research]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert!(!mbm::active(&ui.fe));
    // Back with B; one Escape closes it.
    ui.close();
    key(&mut ui, &mut host, 66);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    key(&mut ui, &mut host, 27);
    assert_eq!(ui.manager(), None, "one Escape closes the build manager");
    key(&mut ui, &mut host, 66);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    // Close.
    let (x, y) = centre(l.buttons[&Button::Close]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), None);
    // Systems opens the launch manager.
    key(&mut ui, &mut host, 66);
    let (x, y) = centre(l.buttons[&Button::Systems]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), Some(Manager::Launch));
}

#[test]
fn modern_build_manager_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn modern_build_manager_768() {
    run((1024, 768), "1024x768");
}

#[test]
fn modern_build_manager_640() {
    run((640, 480), "640x480");
}

/// Clicks outside the panel do nothing; off by default.
#[test]
fn outside_clicks_and_default() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui(src.clone(), size);
    let mut host = host();
    frame(&mut ui, &mut host);
    ui.open(&host, Manager::Construction);
    frame(&mut ui, &mut host);
    let l = layout(size);
    assert_eq!(mbm::hit(&l, 900, 1070, 0, 0, 0), Hit::Outside);
    let out = click_at(&mut ui, &mut host, 900, 1070);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(out.iter().all(|r| !matches!(r, GameRequest::Build { .. })));
    // The modern one by default; the options key, or the original HUD, bring the original
    // back.
    let open = |options: Options, host: &mut MockHost| {
        let mut ui = InGameUi::new(
            src.clone(),
            Setup {
                options,
                default_keys: Some(KEYS.into()),
                keys: None,
                keys_path: None,
                options_path: None,
            },
        )
        .unwrap();
        ui.resize(size.0, size.1);
        frame(&mut ui, host);
        ui.open(host, Manager::Construction);
        mbm::active(&ui.fe)
    };
    assert!(open(Options::default(), &mut host));
    let mut off = Options::default();
    off.set_bool(mbm::OPTION_KEY, false);
    assert!(!open(off, &mut host));
    let mut old_hud = Options::default();
    old_hud.set_bool(modern_hud::OPTION_KEY, false);
    assert!(!open(old_hud, &mut host));
}
