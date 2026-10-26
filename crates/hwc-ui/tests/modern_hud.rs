//! The modern HUD on the real game files with the mock host: the objectives panel's header
//! toggles it (and the choice is kept in the options), its lines are hit-tested like the
//! original's list (an objective opens fleet intelligence, the hyperspace entry jumps), it
//! moves below a speech portrait, and the RU / SU readout is drawn. Screenshots go to
//! `screenshots/hud/` when `HWC_SCREENSHOTS` is set. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`;
//! skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_objectives::{self, EXPANDED_KEY};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::subtitle::HudTop;
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

/// Waits out the double-click time, then clicks.
fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    for _ in 0..20 {
        frame(ui, host);
    }
    ui.handle_input(host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    let button = |down| InputEvent::MouseButton { left: true, down };
    ui.handle_input(host, button(true));
    out.extend(frame(ui, host));
    ui.handle_input(host, button(false));
    out.extend(frame(ui, host));
    out
}

fn host() -> MockHost {
    let mut host = MockHost::somtaaw();
    host.objectives.push(Objective {
        text: "Escort the salvage team through the debris field and return them safely to \
               the Kuun-Lan before the Turanic raiders arrive"
            .into(),
        description: String::new(),
        hyperspace: false,
        primary: true,
        complete: false,
    });
    host.objectives.push(Objective {
        text: "Hyperspace".into(),
        description: String::new(),
        hyperspace: true,
        primary: true,
        complete: false,
    });
    host
}

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) {
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
    ui.draw(&mut canvas);
    let Some(dir) = std::env::var_os("HWC_SCREENSHOTS") else {
        return;
    };
    let dir = if std::path::Path::new(&dir).is_dir() {
        std::path::PathBuf::from(dir).join("hud")
    } else {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hud")
    };
    std::fs::create_dir_all(&dir).unwrap();
    canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    let p = modern_objectives::panel(&ui.fe, size.0, size.1, &view).expect("panel");
    assert!(p.expanded, "expanded by default");
    assert_eq!(p.items.len(), 4);
    assert!(p.items[2].lines.len() >= 2, "the long objective wraps");
    shot(&mut ui, size, &format!("objectives_expanded_{tag}"));

    // A line opens fleet intelligence on that objective, paused.
    let r = p.items[0].rect;
    let out = click_at(&mut ui, &mut host, r.x0 + 40, (r.y0 + r.y1) / 2);
    assert_eq!(ui.manager(), Some(Manager::Objectives));
    assert!(out.contains(&GameRequest::SetPaused(true)));
    ui.handle_input(&host, InputEvent::KeyDown(27));
    frame(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::KeyUp(27));
    frame(&mut ui, &mut host);
    assert_eq!(ui.manager(), None);

    // The hyperspace entry jumps instead.
    let r = p.items[3].rect;
    let out = click_at(&mut ui, &mut host, r.x0 + 40, (r.y0 + r.y1) / 2);
    assert_eq!(ui.manager(), None);
    assert!(
        out.iter().any(|r| format!("{r:?}").contains("Hyperspace")),
        "no hyperspace request: {out:?}"
    );

    // The header collapses the panel; the lines are no longer there to click.
    let h = p.header;
    click_at(&mut ui, &mut host, h.x0 + 40, (h.y0 + h.y1) / 2);
    assert!(
        !ui.fe.game.options.get_bool(EXPANDED_KEY),
        "collapsed and remembered"
    );
    let view = ui.state().view.clone();
    let c = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert!(c.items.is_empty());
    assert!(c.header.x0 == h.x0 && c.header.y0 == h.y0 && c.header.x1 < h.x1);
    shot(&mut ui, size, &format!("objectives_collapsed_{tag}"));
    let r = p.items[0].rect;
    click_at(&mut ui, &mut host, r.x0 + 40, (r.y0 + r.y1) / 2);
    assert_eq!(ui.manager(), None, "a hidden line was clicked");
    // And expands it again.
    click_at(&mut ui, &mut host, h.x0 + 40, (h.y0 + h.y1) / 2);
    assert!(ui.fe.game.options.get_bool(EXPANDED_KEY));

    // A speaker portrait moves the panel (and the readout) below it.
    let bottom = size.1 / 12;
    ui.subtitle_top(HudTop {
        drawn: true,
        portrait_bottom: Some(bottom),
    });
    let view = ui.state().view.clone();
    let q = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert_eq!(q.header.y0, p.header.y0 + bottom);
    shot(&mut ui, size, &format!("objectives_below_portrait_{tag}"));

    // A new objective flashes the header.
    host.objectives.push(Objective {
        text: "Investigate the beacon".into(),
        description: String::new(),
        hyperspace: false,
        primary: false,
        complete: false,
    });
    frame(&mut ui, &mut host);
    shot(&mut ui, size, &format!("objectives_new_{tag}"));
}

#[test]
fn objectives_panel_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn objectives_panel_768() {
    run((1024, 768), "1024x768");
}
