//! The modern research manager (`hwcModernResearchManager = 1`, the default) on the real game
//! files with the mock host: it replaces the original's look (the screen stays open
//! underneath for its keys and its list), its lab tabs, topic rows (double clicks included)
//! and buttons act like the original's regions, one Escape closes it, it switches to and from
//! the modern build manager, the Beast gets its one lab, and the modern HUD's panels keep
//! clear of it. Screenshots go to `screenshots/resui/mock_*.png` when `HWC_SCREENSHOTS` is
//! set. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::consts::sound;
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_build_manager as mbm;
use hwc_ui::ingame::modern_research_manager::{self as mrm, Button, Hit};
use hwc_ui::ingame::research::{self, Picture};
use hwc_ui::ingame::{modern_hud, modern_objectives, modern_panel};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

const KEYS: &str = "v 84\nkey  Escape  27\nkey  Open_BuildManager  66\n\
key  Open_ResearchManager  82\n";

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
            options_path: None,
        },
    )
    .expect("in-game ui");
    ui.resize(size.0, size.1);
    ui
}

fn ui(src: Rc<dyn AssetSource>, size: (i32, i32)) -> InGameUi {
    ui_with(src, size, Options::default())
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

fn press(ui: &mut InGameUi, host: &mut MockHost, down: bool) -> Vec<GameRequest> {
    ui.handle_input(host, InputEvent::MouseButton { left: true, down });
    frame(ui, host)
}

/// A click as a player would make it: away from any earlier click's double-click time.
fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    out.extend(press(ui, host, true));
    out.extend(press(ui, host, false));
    out
}

/// Two clicks in quick succession at the same place.
fn double_click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    settle(ui, host);
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    for _ in 0..2 {
        out.extend(press(ui, host, true));
        out.extend(press(ui, host, false));
    }
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

fn layout(ui: &InGameUi, size: (i32, i32)) -> mrm::Layout {
    mrm::layout(size.0, size.1, ui.state().view.labs.len())
}

/// The screen's list window: its first row on screen and its selection.
fn list(ui: &InGameUi) -> (usize, Option<u32>) {
    let a = ui.fe.find_atom("RM_TechListWindow").expect("the list");
    let l = ui.fe.list(a).expect("a list window");
    (l.upper, l.selected.map(|i| l.items[i].data))
}

/// The on-screen row of a technology (or of a heading, by name).
fn row_of(ui: &InGameUi, tech: TechId) -> usize {
    let rows = research::visible_rows(&ui.state().view);
    let i = rows
        .iter()
        .position(|r| r.2 && r.1 == tech)
        .expect("the topic is listed");
    i - list(ui).0
}

fn heading_row(ui: &InGameUi, name: &str) -> usize {
    let rows = research::visible_rows(&ui.state().view);
    rows.iter().position(|r| !r.2 && r.0 == name).unwrap() - list(ui).0
}

fn row_point(ui: &InGameUi, l: &mrm::Layout, tech: TechId) -> (i32, i32) {
    let (_, y) = centre(l.row(row_of(ui, tech)));
    (l.name_x + 20, y)
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

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) -> Canvas {
    let mut canvas = background(size);
    ui.draw(&mut canvas);
    if std::env::var_os("HWC_SCREENSHOTS").is_some() {
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/resui");
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

fn researches(out: &[GameRequest]) -> Vec<(TechId, usize)> {
    out.iter()
        .filter_map(|r| match r {
            GameRequest::Research { tech, lab } => Some((*tech, *lab)),
            _ => None,
        })
        .collect()
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 82);
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert!(mrm::active(&ui.fe));
    assert_eq!(
        ui.fe.current_screen_name(),
        Some("Research_Manager"),
        "the original screen stays open for its keys and list"
    );
    let l = layout(&ui, size);
    assert_eq!(l.labs.len(), 5);
    settle(&mut ui, &mut host);
    let c = shot(&mut ui, size, &format!("mock_open_{tag}"));
    assert_eq!(
        c.get(l.panel.x0, l.panel.y0 + 40),
        modern_objectives::ACCENT,
        "the panel's accent strip"
    );
    let bg = background(size);
    let (ox, oy) = (l.panel.x0 - 30, l.panel.y1 - 5);
    if ox > 0 {
        assert!(c.get(ox, oy)[2] < bg.get(ox, oy)[2], "the scene is dimmed");
    }
    // The list is the screen's own, sized to the modern page.
    let a = ui.fe.find_atom("RM_TechListWindow").unwrap();
    assert_eq!(ui.fe.list(a).unwrap().max_visible, l.page);

    // A topic row: the release selects it (with the click sound) and shows it.
    let (x, y) = row_point(&ui, &l, 6);
    let out = click_at(&mut ui, &mut host, x, y);
    assert_eq!(list(&ui).1, Some(6));
    assert_eq!(ui.state().rm.picture, Picture::Tech(6));
    assert_eq!(ui.state().rm.selected_lab, Some(2), "its lab is selected");
    assert!(out.contains(&GameRequest::Sound(sound::CLICK)));
    assert!(researches(&out).is_empty(), "a single click only selects");
    shot(&mut ui, size, &format!("mock_selected_{tag}"));
    // A heading cannot be picked.
    let (_, hy) = centre(l.row(heading_row(&ui, "WEAPONS MODULE")));
    click_at(&mut ui, &mut host, l.heading_x + 10, hy);
    assert_eq!(list(&ui).1, Some(6));
    // A press that leaves the list before its release selects nothing.
    let (x8, y8) = row_point(&ui, &l, 8);
    settle(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::MouseMove { x: x8, y: y8 });
    press(&mut ui, &mut host, true);
    let (px, py) = centre(l.picture);
    ui.handle_input(&host, InputEvent::MouseMove { x: px, y: py });
    press(&mut ui, &mut host, false);
    assert_eq!(list(&ui).1, Some(6));

    // Research: the screen's callback, the request, its speech.
    let (bx, by) = centre(l.buttons[&Button::Research]);
    let out = click_at(&mut ui, &mut host, bx, by);
    assert_eq!(researches(&out), [(6, 2)]);
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Speech {
            event: research::SPEECH_RESEARCH,
            ..
        }
    )));
    assert_eq!(host.labs[2].topic, Some(6));
    host.labs[2].progress = 0.45;
    frame(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::MouseMove { x, y });
    frame(&mut ui, &mut host);
    shot(&mut ui, size, &format!("mock_researching_{tag}"));
    // A press dragged off a button does nothing.
    settle(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::MouseMove { x: bx, y: by });
    press(&mut ui, &mut host, true);
    ui.handle_input(&host, InputEvent::MouseMove { x: bx, y: by - 300 });
    let out = press(&mut ui, &mut host, false);
    assert!(!out.iter().any(|r| matches!(
        r,
        GameRequest::ClearLab { .. } | GameRequest::Research { .. }
    )));

    // A double click on a topic researches it, once: the second press is the list's
    // double-click message.
    let out = double_click_at(&mut ui, &mut host, x8, y8);
    assert_eq!(researches(&out), [(8, 4)], "{out:?}");
    assert_eq!(host.labs[4].topic, Some(8));
    // A double click on a button acts once (its second press is not a press).
    let (cx, cy) = centre(l.buttons[&Button::Clear]);
    let out = double_click_at(&mut ui, &mut host, cx, cy);
    let clears: Vec<_> = out
        .iter()
        .filter(|r| matches!(r, GameRequest::ClearLab { .. }))
        .collect();
    assert_eq!(clears, [&GameRequest::ClearLab { lab: 4 }]);
    assert_eq!(host.labs[4].status, LabStatus::Idle);

    // Lab tabs: a press selects the lab and shows its module; the absent lab does nothing.
    let (tx, ty) = centre(l.labs[0]);
    click_at(&mut ui, &mut host, tx, ty);
    assert_eq!(ui.state().rm.selected_lab, Some(0));
    assert_eq!(ui.state().rm.picture, Picture::Module(0));
    let (tx, ty) = centre(l.labs[3]);
    click_at(&mut ui, &mut host, tx, ty);
    assert_eq!(ui.state().rm.selected_lab, Some(0), "the absent lab");
    shot(&mut ui, size, &format!("mock_lab_{tag}"));
    // Clear lab stops the selected lab's research.
    let out = click_at(&mut ui, &mut host, cx, cy);
    assert!(out.contains(&GameRequest::ClearLab { lab: 0 }));
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Speech {
            event: research::SPEECH_CLEAR_LAB,
            ..
        }
    )));

    // Info: the description over the picture; the I key is the screen's and turns it off.
    let (x, y) = row_point(&ui, &l, 5);
    click_at(&mut ui, &mut host, x, y);
    let (ix, iy) = centre(l.buttons[&Button::Info]);
    click_at(&mut ui, &mut host, ix, iy);
    assert!(ui.state().rm.extended_info);
    shot(&mut ui, size, &format!("mock_info_{tag}"));
    key(&mut ui, &mut host, b'I' as u16);
    assert!(!ui.state().rm.extended_info);

    // The screen's keys: R researches the selected topic, L clears its lab.
    let (x, y) = row_point(&ui, &l, 3);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(list(&ui).1, Some(3));
    let out = key(&mut ui, &mut host, b'R' as u16);
    assert_eq!(researches(&out), [(3, 0)]);
    let out = key(&mut ui, &mut host, b'L' as u16);
    assert!(out.contains(&GameRequest::ClearLab { lab: 0 }));

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

    // Build opens the modern build manager; its Research button comes back here.
    let (x, y) = centre(l.buttons[&Button::Build]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(mbm::active(&ui.fe) && !mrm::active(&ui.fe));
    let bl = mbm::layout(size.0, size.1);
    let (x, y) = centre(bl.buttons[&mbm::Button::Research]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert!(mrm::active(&ui.fe));
    // The screen's B key goes to the build manager too, and R there comes back.
    key(&mut ui, &mut host, b'B' as u16);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    key(&mut ui, &mut host, b'R' as u16);
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert!(mrm::active(&ui.fe));
    // One Escape closes it.
    key(&mut ui, &mut host, 27);
    assert_eq!(ui.manager(), None, "one Escape closes the research manager");
    // Close.
    key(&mut ui, &mut host, 82);
    let (x, y) = centre(l.buttons[&Button::Close]);
    let out = click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), None);
    assert!(out.contains(&GameRequest::Sound(sound::CLICK)));
    // Systems opens the launch manager.
    key(&mut ui, &mut host, 82);
    let (x, y) = centre(l.buttons[&Button::Systems]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.manager(), Some(Manager::Launch));
}

#[test]
fn modern_research_manager_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn modern_research_manager_768() {
    run((1024, 768), "1024x768");
}

#[test]
fn modern_research_manager_640() {
    run((640, 480), "640x480");
}

/// A list longer than the page: the wheel, the scroll bar's thumb and track.
#[test]
fn long_list_scrolls() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui(src, size);
    let mut host = host();
    for id in 11..41 {
        host.techs.push(Tech {
            id,
            name: format!("TOPIC {id}"),
            cost: 0,
            time: 90.0,
            lab: 4,
            state: TechState::Available,
            upgrade: id % 3 == 0,
            description: String::new(),
            key: String::new(),
        });
        host.rows.push(ResearchRow::Tech(id));
    }
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 82);
    settle(&mut ui, &mut host);
    let l = layout(&ui, size);
    let total = research::visible_rows(&ui.state().view).len();
    assert!(total > l.page);
    let (x, y) = centre(l.list);
    ui.handle_input(&host, InputEvent::MouseMove { x, y });
    ui.handle_input(&host, InputEvent::Wheel { up: false });
    frame(&mut ui, &mut host);
    assert_eq!(list(&ui).0, 3);
    ui.handle_input(&host, InputEvent::Wheel { up: true });
    frame(&mut ui, &mut host);
    assert_eq!(list(&ui).0, 0);
    // The track pages; the thumb drags to the end.
    let tx = (l.scroll_track.x0 + l.scroll_track.x1) / 2;
    click_at(&mut ui, &mut host, tx, l.scroll_track.y1 - 2);
    assert_eq!(list(&ui).0, l.page.min(total - l.page));
    let th = l.thumb(total, list(&ui).0).unwrap();
    assert_eq!(
        mrm::hit(&l, tx, th.y0 + 2, l.page, total, list(&ui).0),
        Hit::Thumb
    );
    settle(&mut ui, &mut host);
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: tx,
            y: th.y0 + 2,
        },
    );
    press(&mut ui, &mut host, true);
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: tx,
            y: th.y0 + 2000,
        },
    );
    frame(&mut ui, &mut host);
    press(&mut ui, &mut host, false);
    assert_eq!(list(&ui).0, total - l.page);
    // A row scrolled into view is picked by what is on screen.
    let (x, y) = row_point(&ui, &l, 40);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(list(&ui).1, Some(40));
    shot(&mut ui, size, "mock_long_1920x1080");
}

/// The Beast: one lab for every topic.
#[test]
fn beast() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui(src, size);
    let mut host = MockHost::beast();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 82);
    assert_eq!(ui.fe.current_screen_name(), Some("Beast_Research_Manager"));
    assert!(mrm::active(&ui.fe));
    let l = layout(&ui, size);
    assert_eq!(l.labs.len(), 1);
    // Its lab button is off the original screen: the tab only shows the lab.
    let (x, y) = centre(l.labs[0]);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().rm.selected_lab, None);
    // Its lab is busy: Research only selects it.
    let (x, y) = row_point(&ui, &l, 8);
    click_at(&mut ui, &mut host, x, y);
    let (bx, by) = centre(l.buttons[&Button::Research]);
    let out = click_at(&mut ui, &mut host, bx, by);
    assert!(researches(&out).is_empty());
    assert_eq!(ui.state().rm.selected_lab, Some(0));
    let (cx, cy) = centre(l.buttons[&Button::Clear]);
    let out = click_at(&mut ui, &mut host, cx, cy);
    assert!(out.contains(&GameRequest::ClearLab { lab: 0 }));
    let out = click_at(&mut ui, &mut host, bx, by);
    assert_eq!(researches(&out), [(8, 0)], "every topic in the one lab");
    shot(&mut ui, size, "mock_beast_1920x1080");
}

/// Clicks outside the panel do nothing; on by default, and the flags bring back the original.
#[test]
fn outside_clicks_and_switches() {
    let Some(src) = source() else { return };
    let size = (1920, 1080);
    let mut ui = ui(src.clone(), size);
    let mut host = host();
    frame(&mut ui, &mut host);
    ui.open(&host, Manager::Research);
    frame(&mut ui, &mut host);
    let l = layout(&ui, size);
    assert_eq!(mrm::hit(&l, 900, 1070, 0, 0, 0), Hit::Outside);
    let before = ui.state().rm.selected_lab;
    let out = click_at(&mut ui, &mut host, 900, 1070);
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert_eq!(ui.state().rm.selected_lab, before);
    assert!(researches(&out).is_empty());
    // A right click does nothing either.
    let (x, y) = row_point(&ui, &l, 6);
    ui.handle_input(&host, InputEvent::MouseMove { x, y });
    ui.handle_input(
        &host,
        InputEvent::MouseButton {
            left: false,
            down: true,
        },
    );
    ui.handle_input(
        &host,
        InputEvent::MouseButton {
            left: false,
            down: false,
        },
    );
    frame(&mut ui, &mut host);
    assert_ne!(list(&ui).1, Some(6));
    // On by default; the key or the original HUD bring back the original screen.
    let open = |options: Options, host: &mut MockHost| {
        let mut ui = ui_with(src.clone(), size, options);
        frame(&mut ui, host);
        ui.open(host, Manager::Research);
        mrm::active(&ui.fe)
    };
    assert!(open(Options::default(), &mut host));
    let mut off = Options::default();
    off.set_bool(mrm::OPTION_KEY, false);
    assert!(!open(off, &mut host));
    let mut old_hud = Options::default();
    old_hud.set_bool(modern_hud::OPTION_KEY, false);
    assert!(!open(old_hud, &mut host));
    // With the original research manager the original screen draws (its title bar art).
    let mut off = Options::default();
    off.set_bool(mrm::OPTION_KEY, false);
    let mut ui = ui_with(src, size, off);
    frame(&mut ui, &mut host);
    ui.open(&host, Manager::Research);
    frame(&mut ui, &mut host);
    let c = shot(&mut ui, size, "mock_original_1920x1080");
    let bg = background(size);
    let l = layout(&ui, size);
    assert_ne!(
        c.get(l.panel.x0, l.panel.y0 + 40),
        modern_objectives::ACCENT
    );
    let _ = bg;
}
