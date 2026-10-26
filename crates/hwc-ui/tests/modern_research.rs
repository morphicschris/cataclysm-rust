//! The modern HUD's research panel on the real game files with the mock host: directly under
//! the build queue (under the objectives while the queue is empty), clear of the system log,
//! its header folds it (kept in the options as `hwcResearchExpanded`), its rows take a click
//! without acting (the original's list does nothing on a click), it hides with nothing
//! researched and in a cutscene, folds or hides over the research manager, keeps a finished
//! topic as a fading "COMPLETE" row, and `--hud original` brings the original list back.
//! Screenshots go to `screenshots/hud/` when `HWC_SCREENSHOTS` is set. Needs `HWC_DATA_DIR`
//! or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_panel::{self, Arrangement};
use hwc_ui::ingame::{modern_hud, modern_research};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn ui(src: Rc<dyn AssetSource>, size: (i32, i32)) -> InGameUi {
    let mut ui = InGameUi::new(
        src,
        Setup {
            options: Options::default(),
            default_keys: Some("v 84\nkey  Escape  27\n".into()),
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

fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> (Vec<GameRequest>, bool) {
    for _ in 0..20 {
        frame(ui, host);
    }
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    let button = |down| InputEvent::MouseButton { left: true, down };
    let took = ui.handle_input(host, button(true));
    out.extend(frame(ui, host));
    ui.handle_input(host, button(false));
    out.extend(frame(ui, host));
    (out, took)
}

fn host() -> MockHost {
    let mut host = MockHost::somtaaw();
    host.objectives.push(Objective {
        text: "Defend the Kuun-Lan until the salvage team returns".into(),
        description: String::new(),
        hyperspace: false,
        primary: true,
        complete: false,
    });
    host.messages.push(GameMessage {
        text: "ACOLYTE built".into(),
        colour: [255, 255, 255, 255],
        ship: None,
        time: host.time,
    });
    host
}

fn arrangement(ui: &InGameUi, size: (i32, i32)) -> Arrangement {
    let view = ui.state().view.clone();
    modern_panel::arrange(&ui.fe, size.0, size.1, &view)
}

fn out_dir() -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("HWC_SCREENSHOTS")?;
    let dir = if std::path::Path::new(&dir).is_dir() {
        std::path::PathBuf::from(dir).join("hud")
    } else {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hud")
    };
    std::fs::create_dir_all(&dir).unwrap();
    Some(dir)
}

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) -> Canvas {
    let mut canvas = Canvas::new(size.0, size.1);
    canvas.pixels.fill([60, 70, 90, 255]);
    ui.draw(&mut canvas);
    if let Some(dir) = out_dir() {
        canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
    }
    canvas
}

fn assert_apart(a: &Arrangement) {
    let r = a.rects();
    for i in 0..r.len() {
        for j in i + 1..r.len() {
            assert!(
                !modern_hud::overlaps(r[i], r[j]),
                "{:?} and {:?} overlap",
                r[i],
                r[j]
            );
        }
    }
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    let build = a.build.clone().expect("build queue");
    let r = a.research.clone().expect("research panel");
    let log = a.log.clone().expect("system log");
    assert!(r.expanded, "expanded by default");
    assert_apart(&a);
    // Directly under the build queue, in its column, above the log.
    assert_eq!(r.header.y0, build.below);
    assert_eq!(r.header.x0, build.header.x0);
    assert!(r.panel.y1 <= log.panel.y0);
    // The mock's two labs at work, in technology order.
    assert_eq!(r.rows.len() + r.hidden, 2);
    let names: Vec<_> = a.research_entries.iter().map(|e| e.name.clone()).collect();
    assert_eq!(names.len(), 2);
    assert!(a.research_entries[0].tech < a.research_entries[1].tech);
    assert_eq!(a.left_below(), Some(r.below));
    let c = shot(&mut ui, size, &format!("research_expanded_{tag}"));
    assert_eq!(
        c.get(r.panel.x0, r.panel.y1 - 2),
        hwc_ui::ingame::modern_objectives::ACCENT
    );

    // A row takes the click and asks nothing of the game (nor opens a manager).
    if let Some(row) = r.rows.first() {
        let (out, took) = click_at(&mut ui, &mut host, row.rect.x0 + 30, row.rect.y0 + 4);
        assert!(took, "the click went through the research panel");
        assert!(out.is_empty(), "{out:?}");
        assert_eq!(ui.manager(), None);
    }

    // The header folds it, remembered in the options; the header alone is left.
    let h = r.header;
    assert!(click_at(&mut ui, &mut host, h.x0 + h.height(), (h.y0 + h.y1) / 2).1);
    assert!(!ui.fe.game.options.get_bool(modern_research::EXPANDED_KEY));
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: size.1 / 2,
        },
    );
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    let f = a.research.clone().unwrap();
    assert!(!f.expanded && f.rows.is_empty());
    assert_eq!(f.panel, f.header);
    assert_eq!(f.header.y0, r.header.y0);
    shot(&mut ui, size, &format!("research_collapsed_{tag}"));
    let h = f.header;
    click_at(&mut ui, &mut host, h.x0 + h.height(), (h.y0 + h.y1) / 2);
    assert!(ui.fe.game.options.get_bool(modern_research::EXPANDED_KEY));

    // An empty build queue: directly under the objectives.
    host.jobs.clear();
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    assert!(a.build.is_none());
    let objs = a.objectives.clone().unwrap();
    assert_eq!(a.research.as_ref().unwrap().header.y0, objs.below);
    assert_apart(&a);

    // A topic finishes: a COMPLETE row, gone after a few seconds.
    let lab = host
        .labs
        .iter()
        .position(|l| l.status == LabStatus::Researching)
        .unwrap();
    let t = host.labs[lab].topic.unwrap();
    host.labs[lab].status = LabStatus::Idle;
    host.labs[lab].topic = None;
    host.techs.iter_mut().find(|k| k.id == t).unwrap().state = TechState::Researched;
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    let done: Vec<_> = a
        .research_entries
        .iter()
        .filter(|e| e.complete.is_some())
        .collect();
    assert_eq!(done.len(), 1);
    assert_eq!(done[0].tech, t);
    shot(&mut ui, size, &format!("research_complete_{tag}"));
    let secs = modern_research::COMPLETE_HOLD + modern_research::COMPLETE_FADE;
    for _ in 0..(secs * 30.0) as usize + 2 {
        frame(&mut ui, &mut host);
    }
    let a = arrangement(&ui, size);
    assert!(a.research_entries.iter().all(|e| e.complete.is_none()));

    // Nothing researched: no panel.
    for l in &mut host.labs {
        l.status = LabStatus::Idle;
        l.topic = None;
    }
    frame(&mut ui, &mut host);
    for _ in 0..(secs * 30.0) as usize + 2 {
        frame(&mut ui, &mut host);
    }
    assert!(arrangement(&ui, size).research.is_none());

    // In a cutscene the panel hides with the others.
    let mut host = self::host();
    frame(&mut ui, &mut host);
    assert!(arrangement(&ui, size).research.is_some());
    ui.fe.game.ig.overlay.cutscene = true;
    assert!(arrangement(&ui, size).research.is_none());
    ui.fe.game.ig.overlay.cutscene = false;

    // `--hud original`: the original list instead.
    ui.fe.game.options.set_bool(modern_hud::OPTION_KEY, false);
    frame(&mut ui, &mut host);
    assert!(arrangement(&ui, size).research.is_none());
    shot(&mut ui, size, &format!("research_original_{tag}"));
}

#[test]
fn research_panel_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn research_panel_768() {
    run((1024, 768), "1024x768");
}

#[test]
fn research_panel_640() {
    run((640, 480), "640x480");
}

/// Over the research manager the panel stays clear of its frame (folds or hides), and a
/// header click there does not reach the manager.
fn over_manager(size: (i32, i32)) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    // The original manager's frame (the modern managers have their own tests).
    ui.fe
        .game
        .options
        .set_bool(hwc_ui::ingame::modern_build_manager::OPTION_KEY, false);
    ui.fe
        .game
        .options
        .set_bool(hwc_ui::ingame::modern_research_manager::OPTION_KEY, false);
    let mut host = host();
    frame(&mut ui, &mut host);
    ui.open(&host, Manager::Research);
    for _ in 0..10 {
        frame(&mut ui, &mut host);
    }
    assert_eq!(ui.manager(), Some(Manager::Research));
    let a = arrangement(&ui, size);
    let content = modern_hud::manager_content(size.0, size.1);
    for r in a.rects() {
        assert!(
            !modern_hud::overlaps(r, content),
            "{r:?} covers {content:?}"
        );
    }
    assert_apart(&a);
    if let Some(r) = a.research {
        let h: Rect = r.header;
        assert!(click_at(&mut ui, &mut host, h.x0 + h.height(), (h.y0 + h.y1) / 2).1);
        assert_eq!(ui.manager(), Some(Manager::Research));
        assert!(
            ui.fe.game.options.get_bool(modern_research::EXPANDED_KEY),
            "not saved over a manager"
        );
    }
}

#[test]
fn research_panel_over_the_research_manager_1080p() {
    over_manager((1920, 1080));
}

#[test]
fn research_panel_over_the_research_manager_768() {
    over_manager((1024, 768));
}
