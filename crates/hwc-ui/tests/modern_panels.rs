//! The modern HUD's build queue, system log and selected-ships panels on the real game files
//! with the mock host: where they go next to the objectives and resources panels at 1920x1080,
//! 1024x768 and 640x480 (never overlapping), their headers fold them (kept in the options),
//! their rows are hit-tested like the original's lines (a selected-ship row selects its type,
//! a message about a ship focuses it), clicks on them do not reach the game, they stay clear
//! of the build manager's frame, and `--hud original` brings the original lines back.
//! Screenshots go to `screenshots/hud/` when `HWC_SCREENSHOTS` is set. Needs `HWC_DATA_DIR`
//! or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::{MockHost, CARRIER_1, MOTHERSHIP};
use hwc_ui::ingame::modern_panel::{self, Arrangement};
use hwc_ui::ingame::{modern_build, modern_hud, modern_log, modern_selected};
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

/// Waits out the double-click time, then clicks; the requests and whether the UI took the
/// press.
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

fn ship(id: ShipId, t: ShipType, name: &str, health: f32) -> ShipInfo {
    ShipInfo {
        ship: id,
        ship_type: t,
        name: name.into(),
        health,
        max_health: 1000.0,
        ..Default::default()
    }
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
    // A second factory's job of the same type, and another type.
    host.jobs.push(BuildJob {
        factory: CARRIER_1,
        ship_type: 0x01,
        count: 2,
        batch_total: 2,
        paused: false,
        progress: 0.1,
        cost: 85,
        cost_so_far: 8,
    });
    host.jobs.push(BuildJob {
        factory: MOTHERSHIP,
        ship_type: 26,
        count: 1,
        batch_total: 1,
        paused: false,
        progress: 0.65,
        cost: 400,
        cost_so_far: 260,
    });
    // Messages: old ones (dimmed history), a red warning about a ship, the newest.
    let msg = |text: &str, colour, ship, time| GameMessage {
        text: text.into(),
        colour,
        ship,
        time,
    };
    let white = [255, 255, 255, 255];
    host.messages
        .insert(0, msg("Resources collected: 450 RU", white, None, 600.0));
    host.messages
        .insert(0, msg("WORKER built", white, Some(30), 590.0));
    host.messages.push(msg(
        "Kuun-Lan under attack",
        [255, 0, 0, 255],
        Some(MOTHERSHIP),
        752.0,
    ));
    host.messages
        .push(msg("ACOLYTE built", white, Some(10), 753.5));
    // Six recons (one bar, their average) besides the acolytes and the frigate.
    for i in 0..6 {
        let h = [1000.0, 900.0, 700.0, 300.0, 1000.0, 150.0][i];
        host.selection.push(ship(50 + i as u32, 3, "RECON", h));
    }
    host
}

fn arrangement(ui: &InGameUi, size: (i32, i32)) -> Arrangement {
    let view = ui.state().view.clone();
    modern_panel::arrange(&ui.fe, size.0, size.1, &view)
}

fn background(size: (i32, i32)) -> Canvas {
    let mut canvas = Canvas::new(size.0, size.1);
    // A bright nebula-like stand-in for the 3D view, to check readability.
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
    let mut canvas = background(size);
    ui.draw(&mut canvas);
    if let Some(dir) = out_dir() {
        canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
    }
    canvas
}

/// Saves the part of `c` around `r` (with a margin) as its own picture.
fn crop(c: &Canvas, r: Rect, name: &str) {
    let Some(dir) = out_dir() else { return };
    let m = 12;
    let (x0, y0) = ((r.x0 - m).max(0), (r.y0 - m).max(0));
    let (x1, y1) = ((r.x1 + m).min(c.width), (r.y1 + m).min(c.height));
    let mut out = Canvas::new(x1 - x0, y1 - y0);
    for y in y0..y1 {
        for x in x0..x1 {
            out.pixels[((y - y0) * out.width + (x - x0)) as usize] = c.get(x, y);
        }
    }
    out.save_png(&dir.join(format!("{name}.png"))).unwrap();
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

fn header_click(ui: &mut InGameUi, host: &mut MockHost, h: Rect) -> bool {
    click_at(ui, host, h.x0 + h.height(), (h.y0 + h.y1) / 2).1
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    let (objs, res) = (a.objectives.clone().unwrap(), a.resources.unwrap());
    let build = a.build.clone().expect("build queue");
    let log = a.log.clone().expect("system log");
    let sel = a.selected.clone().expect("selected ships");
    assert!(
        build.expanded && log.expanded && sel.expanded,
        "expanded by default"
    );
    assert_apart(&a);
    // The build queue directly under the objectives, the selected ships under the resources,
    // the log on the bottom margin; all inside the screen.
    assert_eq!(build.header.y0, objs.below);
    assert_eq!(build.header.x0, objs.header.x0);
    assert_eq!(sel.header.y0, res.bottom);
    assert_eq!(sel.panel.x1, res.panel.x1);
    for r in a.rects() {
        assert!(
            r.x0 >= 0 && r.y0 >= 0 && r.x1 <= size.0 && r.y1 <= size.1,
            "{r:?}"
        );
    }
    let s = modern_hud::scale_for(size.1);
    // The log grows up from the bottom-left corner, its header on top of the messages.
    let corner = size.1 - (16.0 * s).round() as i32;
    assert_eq!(log.panel.y1, corner);
    assert_eq!(log.header.y0, log.panel.y0);
    assert!(log.items.iter().all(|i| i.rect.y0 >= log.header.y1));
    // Three types queued (acolytes from two factories), three selected, the last 8 messages.
    assert_eq!(build.rows.len() + build.hidden, 3);
    assert_eq!(sel.rows.len() + sel.hidden, 3);
    assert!(log.items.len() <= modern_log::HISTORY);
    assert_eq!(
        log.items.last().unwrap().message,
        host.messages.len() - 1,
        "newest last"
    );
    let c = shot(&mut ui, size, &format!("hud_full_{tag}"));
    crop(&c, build.panel, &format!("build_queue_expanded_{tag}"));
    crop(&c, log.panel, &format!("system_log_expanded_{tag}"));
    crop(&c, sel.panel, &format!("selected_expanded_{tag}"));
    // The accent strip on each.
    for p in [build.panel, log.panel, sel.panel] {
        assert_eq!(
            c.get(p.x0, p.y1 - 2),
            hwc_ui::ingame::modern_objectives::ACCENT
        );
    }

    // A selected-ship row selects its type, as the original's line does.
    let row = sel.rows[0];
    let g = modern_selected::groups(&ui.state().view.selection);
    let (out, took) = click_at(
        &mut ui,
        &mut host,
        row.rect.x0 + 30,
        (row.rect.y0 + row.rect.y1) / 2,
    );
    assert!(took);
    assert!(
        out.contains(&GameRequest::SelectType {
            ship_type: g[0].ship_type,
            owner: g[0].owner
        }),
        "{out:?}"
    );
    // A message about a ship focuses it; the red warning is about the mothership.
    let a = arrangement(&ui, size);
    let log = a.log.clone().unwrap();
    let item = log
        .items
        .iter()
        .find(|i| host.messages[i.message].ship == Some(MOTHERSHIP))
        .expect("warning listed");
    let (out, took) = click_at(
        &mut ui,
        &mut host,
        item.rect.x0 + 30,
        (item.rect.y0 + item.rect.y1) / 2,
    );
    assert!(took);
    assert!(out.contains(&GameRequest::FocusShip(MOTHERSHIP)), "{out:?}");
    // Clicks elsewhere on a panel are used up (they do not reach the 3D view).
    let b = a.build.clone().unwrap();
    let (out, took) = click_at(&mut ui, &mut host, b.panel.x1 - 3, b.panel.y1 - 2);
    assert!(took, "the click went through the build queue");
    assert!(out.is_empty(), "{out:?}");

    // Each header folds its panel, remembered in the options.
    let keys = [
        modern_build::EXPANDED_KEY,
        modern_log::EXPANDED_KEY,
        modern_selected::EXPANDED_KEY,
    ];
    for k in keys {
        let a = arrangement(&ui, size);
        let h = match k {
            modern_build::EXPANDED_KEY => a.build.unwrap().header,
            modern_log::EXPANDED_KEY => a.log.unwrap().header,
            _ => a.selected.unwrap().header,
        };
        assert!(header_click(&mut ui, &mut host, h));
        assert!(!ui.fe.game.options.get_bool(k), "{k} not saved");
    }
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: size.1 / 2,
        },
    );
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    let (b, l, sl) = (
        a.build.clone().unwrap(),
        a.log.clone().unwrap(),
        a.selected.clone().unwrap(),
    );
    assert!(!b.expanded && !l.expanded && !sl.expanded);
    assert!(b.rows.is_empty() && l.items.is_empty() && sl.rows.is_empty());
    assert_eq!(b.panel, b.header, "folded: the header alone");
    assert_eq!(
        l.header.y1, log.panel.y1,
        "folded, the header sits in the corner"
    );
    assert_apart(&a);
    let c = shot(&mut ui, size, &format!("hud_collapsed_{tag}"));
    crop(&c, b.panel, &format!("build_queue_collapsed_{tag}"));
    crop(&c, l.panel, &format!("system_log_collapsed_{tag}"));
    crop(&c, sl.panel, &format!("selected_collapsed_{tag}"));
    // Folded, nothing is drawn under the header's text: the background shows through.
    let bg = background(size);
    assert_eq!(
        c.get(b.header.x0 + 1, b.header.y0 + 1),
        bg.get(b.header.x0 + 1, b.header.y0 + 1)
    );
    // A new message flashes the folded log's header.
    host.messages.push(GameMessage {
        text: "Research complete: Ion Cannon".into(),
        colour: [0, 255, 0, 255],
        ship: None,
        time: host.time,
    });
    frame(&mut ui, &mut host);
    let c = shot(&mut ui, size, &format!("system_log_flash_{tag}"));
    let p = (l.header.x1 - 3, l.header.y0 + 3);
    assert_ne!(c.get(p.0, p.1), bg.get(p.0, p.1), "no flash");
    // And back.
    for k in keys {
        let a = arrangement(&ui, size);
        let h = match k {
            modern_build::EXPANDED_KEY => a.build.unwrap().header,
            modern_log::EXPANDED_KEY => a.log.unwrap().header,
            _ => a.selected.unwrap().header,
        };
        header_click(&mut ui, &mut host, h);
        assert!(ui.fe.game.options.get_bool(k));
    }

    // An empty queue and an empty selection: those panels go.
    host.jobs.clear();
    host.selection.clear();
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    assert!(a.build.is_none() && a.selected.is_none() && a.log.is_some());

    // `--hud original`: no modern panels; the original lines are back.
    let mut host = self::host();
    ui.fe.game.options.set_bool(modern_hud::OPTION_KEY, false);
    frame(&mut ui, &mut host);
    let a = arrangement(&ui, size);
    assert!(a.rects().is_empty());
    shot(&mut ui, size, &format!("hud_original_lists_{tag}"));
}

#[test]
fn list_panels_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn list_panels_768() {
    run((1024, 768), "1024x768");
}

#[test]
fn list_panels_640() {
    run((640, 480), "640x480");
}

/// Over the build manager the panels stay clear of its frame: they fold, or hide when even
/// their header would reach it; a header click there does not reach the manager.
fn over_build_manager(size: (i32, i32), tag: &str) {
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
    ui.open(&host, Manager::Construction);
    for _ in 0..10 {
        frame(&mut ui, &mut host);
    }
    assert_eq!(ui.manager(), Some(Manager::Construction));
    let a = arrangement(&ui, size);
    let content = modern_hud::manager_content(size.0, size.1);
    for r in a.rects() {
        assert!(
            !modern_hud::overlaps(r, content),
            "{r:?} covers {content:?}"
        );
    }
    assert_apart(&a);
    shot(&mut ui, size, &format!("build_manager_lists_{tag}"));
    if let Some(b) = a.build {
        assert!(header_click(&mut ui, &mut host, b.header));
        assert_eq!(ui.manager(), Some(Manager::Construction));
        assert!(
            ui.fe.game.options.get_bool(modern_build::EXPANDED_KEY),
            "not saved"
        );
    }
}

#[test]
fn list_panels_over_the_build_manager_1080p() {
    over_build_manager((1920, 1080), "1920x1080");
}

#[test]
fn list_panels_over_the_build_manager_768() {
    over_build_manager((1024, 768), "1024x768");
}

#[test]
fn list_panels_over_the_build_manager_640() {
    over_build_manager((640, 480), "640x480");
}
