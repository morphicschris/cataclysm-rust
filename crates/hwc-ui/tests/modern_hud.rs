//! The modern HUD on the real game files with the mock host: the objectives panel's header
//! toggles it (and the choice is kept in the options), its lines are hit-tested like the
//! original's list (an objective opens fleet intelligence), the hyperspace entry is a button
//! that jumps on the release over it (and a compact one by the folded header), it
//! moves below a speech portrait, completed and destroyed objectives stay listed greyed out;
//! the resources panel folds from its header; both panels stay up over the build manager,
//! clear of its frame, and take the clicks on them. Screenshots go to `screenshots/hud/`
//! when `HWC_SCREENSHOTS` is set. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::modern_hud;
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

fn hyperspace(out: &[GameRequest]) -> bool {
    out.iter().any(|r| format!("{r:?}").contains("Hyperspace"))
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

fn shot(ui: &mut InGameUi, size: (i32, i32), name: &str) -> Canvas {
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
        return canvas;
    };
    let dir = if std::path::Path::new(&dir).is_dir() {
        std::path::PathBuf::from(dir).join("hud")
    } else {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hud")
    };
    std::fs::create_dir_all(&dir).unwrap();
    canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
    canvas
}

fn run(size: (i32, i32), tag: &str) {
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    let p = modern_objectives::panel(&ui.fe, size.0, size.1, &view).expect("panel");
    assert!(p.expanded, "expanded by default");
    assert_eq!(p.items.len(), 3, "the hyperspace entry is the button");
    assert!(p.items[2].lines.len() >= 2, "the long objective wraps");
    let b = p.button.clone().expect("hyperspace button");
    assert!(!b.compact && b.label == "HYPERSPACE");
    assert!(b.rect.y0 >= p.header.y1 && b.rect.y1 <= p.items[0].rect.y0);
    assert!(b.rect.width() > p.panel.width() * 9 / 10, "full width");
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

    // The hyperspace button: brighter under the pointer, pressed while held, and it jumps on
    // the release over it.
    let (bx, by) = ((b.rect.x0 + b.rect.x1) / 2, (b.rect.y0 + b.rect.y1) / 2);
    let idle = shot(&mut ui, size, &format!("objectives_button_{tag}"));
    for _ in 0..20 {
        frame(&mut ui, &mut host);
    }
    ui.handle_input(&host, InputEvent::MouseMove { x: bx, y: by });
    frame(&mut ui, &mut host);
    let hover = shot(&mut ui, size, &format!("objectives_button_hover_{tag}"));
    let lum = |c: &Canvas, r: hwc_ui::canvas::Rect| {
        let mut t = 0u64;
        for y in r.y0..r.y1 {
            for x in r.x0..r.x1 {
                let p = c.get(x, y);
                t += p[0] as u64 + p[1] as u64 + p[2] as u64;
            }
        }
        t
    };
    assert!(lum(&hover, b.rect) > lum(&idle, b.rect), "hover brightens");
    let button = |down| InputEvent::MouseButton { left: true, down };
    ui.handle_input(&host, button(true));
    let out = frame(&mut ui, &mut host);
    assert!(!hyperspace(&out), "jumped on the press: {out:?}");
    shot(&mut ui, size, &format!("objectives_button_pressed_{tag}"));
    ui.handle_input(&host, button(false));
    let out = frame(&mut ui, &mut host);
    assert_eq!(ui.manager(), None);
    assert!(hyperspace(&out), "no hyperspace request: {out:?}");
    // Pressed, then let go off the button: nothing.
    for _ in 0..20 {
        frame(&mut ui, &mut host);
    }
    ui.handle_input(&host, button(true));
    frame(&mut ui, &mut host);
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: size.1 / 2,
        },
    );
    ui.handle_input(&host, button(false));
    let out = frame(&mut ui, &mut host);
    assert!(!hyperspace(&out), "jumped off the button: {out:?}");
    // Paused, the click does nothing, as the original.
    host.player.paused = true;
    let out = click_at(&mut ui, &mut host, bx, by);
    assert!(!hyperspace(&out), "jumped while paused: {out:?}");
    shot(&mut ui, size, &format!("objectives_button_paused_{tag}"));
    host.player.paused = false;

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
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: size.1 / 2,
        },
    );
    shot(&mut ui, size, &format!("objectives_collapsed_{tag}"));
    // The compact button by the folded header still jumps, and leaves the panel folded.
    let cb = c.button.clone().expect("compact button");
    assert!(cb.compact && cb.rect.x0 >= c.header.x1);
    let (cx, cy) = ((cb.rect.x0 + cb.rect.x1) / 2, (cb.rect.y0 + cb.rect.y1) / 2);
    ui.handle_input(&host, InputEvent::MouseMove { x: cx, y: cy });
    frame(&mut ui, &mut host);
    shot(&mut ui, size, &format!("objectives_collapsed_hover_{tag}"));
    let out = click_at(&mut ui, &mut host, cx, cy);
    assert!(hyperspace(&out), "the compact button: {out:?}");
    assert!(!ui.fe.game.options.get_bool(EXPANDED_KEY));
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
    ui.subtitle_top(HudTop {
        drawn: true,
        portrait_bottom: None,
    });
    // The slide back up happens as the overlay draws.
    let mut canvas = Canvas::new(size.0, size.1);
    for _ in 0..400 {
        if ui.state().overlay.top == 0 {
            break;
        }
        frame(&mut ui, &mut host);
        ui.draw(&mut canvas);
    }
    assert_eq!(ui.state().overlay.top, 0, "back up after the portrait");

    // The script marks one objective complete and destroys another: both stay listed, greyed
    // out, in the game's order; the destroyed one is no longer clickable, the hyperspace
    // entry still is.
    host.objectives[2].complete = true;
    let destroyed = host.objectives.remove(0);
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    let p = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert_eq!(p.items.len(), 4);
    assert_eq!(p.items[0].lines[0].text, destroyed.text);
    assert!(p.items[0].complete && p.items[0].live.is_none());
    assert!(
        p.items[1].complete,
        "the mock's harvest objective is complete"
    );
    assert!(p.items[2].complete && p.items[2].live == Some(1));
    assert!(p.button.is_some(), "the hyperspace entry");
    assert!(!p.items[3].complete);
    let mouse = (size.0 / 2, size.1 / 2);
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: mouse.0,
            y: mouse.1,
        },
    );
    let c = shot(&mut ui, size, &format!("objectives_completed_{tag}"));
    // The filled diamond of the destroyed one: grey, not gold.
    let (mx, my) = p.items[0].marker;
    let d = c.get(mx as i32, my as i32);
    assert!(d[0].abs_diff(d[2]) < 40 && d[0] > 100, "{d:?}");
    let r = p.items[0].rect;
    click_at(&mut ui, &mut host, r.x0 + 40, (r.y0 + r.y1) / 2);
    assert_eq!(
        ui.manager(),
        None,
        "a destroyed objective opened the objectives screen"
    );
    let r = p.button.as_ref().unwrap().rect;
    let out = click_at(&mut ui, &mut host, r.x0 + 40, (r.y0 + r.y1) / 2);
    assert!(hyperspace(&out));

    // The resources panel folds from its header and remembers it.
    let view = ui.state().view.clone();
    let l = modern_hud::panel(&ui.fe, size.0, size.1, &view);
    assert!(l.expanded, "the resources panel starts expanded");
    let full = l.header;
    click_at(
        &mut ui,
        &mut host,
        full.x0 + full.height(),
        (full.y0 + full.y1) / 2,
    );
    assert!(!ui.fe.game.options.get_bool(modern_hud::EXPANDED_KEY));
    let view = ui.state().view.clone();
    let f = modern_hud::panel(&ui.fe, size.0, size.1, &view);
    assert!(!f.expanded && f.header.x1 == full.x1 && f.header.x0 > full.x0);
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: mouse.0,
            y: mouse.1,
        },
    );
    shot(&mut ui, size, &format!("resources_collapsed_{tag}"));
    click_at(
        &mut ui,
        &mut host,
        f.header.x0 + f.header.height(),
        (f.header.y0 + f.header.y1) / 2,
    );
    assert!(ui.fe.game.options.get_bool(modern_hud::EXPANDED_KEY));
    ui.handle_input(
        &host,
        InputEvent::MouseMove {
            x: mouse.0,
            y: mouse.1,
        },
    );
    shot(&mut ui, size, &format!("resources_expanded_{tag}"));
}

/// Both panels stay up over the build manager, clear of its frame, drawn over it; a click on
/// a header folds the panel for as long as the manager is open, and does not reach the
/// manager.
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
    let view = ui.state().view.clone();
    let content = modern_hud::manager_content(size.0, size.1);
    let res = modern_hud::panel(&ui.fe, size.0, size.1, &view);
    let objs = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert!(
        !modern_hud::overlaps(res.panel, content),
        "{:?} {content:?}",
        res.panel
    );
    assert!(
        !modern_hud::overlaps(objs.panel, content),
        "{:?} {content:?}",
        objs.panel
    );
    let before = modern_hud::drawn_count();
    let c = shot(&mut ui, size, &format!("build_manager_{tag}"));
    assert!(
        modern_hud::drawn_count() > before,
        "the readout is hidden by the manager"
    );
    if res.expanded {
        let p = c.get(res.panel.x0 + 1, res.panel.y1 - 3);
        assert_eq!(p, modern_objectives::ACCENT, "the accent strip is drawn");
    }
    let p = c.get(objs.header.x0 + 1, objs.header.y0 + 3);
    assert_eq!(
        p,
        modern_objectives::ACCENT,
        "the objectives panel is drawn"
    );

    // A header click folds the panel over the manager only; the manager stays open.
    let h = res.header;
    click_at(&mut ui, &mut host, h.x0 + h.height(), (h.y0 + h.y1) / 2);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(
        ui.fe.game.options.get_bool(modern_hud::EXPANDED_KEY),
        "not saved"
    );
    let view = ui.state().view.clone();
    let folded = modern_hud::panel(&ui.fe, size.0, size.1, &view);
    assert_eq!(folded.expanded, !res.expanded);
    let h = objs.header;
    click_at(&mut ui, &mut host, h.x0 + 40, (h.y0 + h.y1) / 2);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(ui.fe.game.options.get_bool(EXPANDED_KEY), "not saved");
    let view = ui.state().view.clone();
    let o = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert_eq!(o.expanded, !objs.expanded);
    // The manager still takes clicks elsewhere: its keys close it, and the folds go back to
    // the options file's.
    ui.handle_input(&host, InputEvent::KeyDown(27));
    frame(&mut ui, &mut host);
    ui.handle_input(&host, InputEvent::KeyUp(27));
    frame(&mut ui, &mut host);
    assert_eq!(ui.manager(), None);
    let view = ui.state().view.clone();
    assert!(modern_hud::panel(&ui.fe, size.0, size.1, &view).expanded);
    assert!(
        modern_objectives::panel(&ui.fe, size.0, size.1, &view)
            .unwrap()
            .expanded
    );

    // The hyperspace button takes its click over the manager too, on the release.
    ui.open(&host, Manager::Construction);
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    let b = modern_objectives::panel(&ui.fe, size.0, size.1, &view)
        .unwrap()
        .button
        .expect("button over the manager");
    let out = click_at(
        &mut ui,
        &mut host,
        (b.rect.x0 + b.rect.x1) / 2,
        (b.rect.y0 + b.rect.y1) / 2,
    );
    assert!(hyperspace(&out), "over the manager: {out:?}");

    // In a cutscene (the letterbox, fleet intel) the panels hide over the manager too.
    ui.open(&host, Manager::Construction);
    frame(&mut ui, &mut host);
    ui.letterboxed = true;
    let before = modern_hud::drawn_count();
    let mut canvas = Canvas::new(size.0, size.1);
    ui.draw(&mut canvas);
    assert_eq!(modern_hud::drawn_count(), before, "drawn in a cutscene");
}

#[test]
fn panels_over_the_build_manager_1080p() {
    over_build_manager((1920, 1080), "1920x1080");
}

#[test]
fn panels_over_the_build_manager_768() {
    over_build_manager((1024, 768), "1024x768");
}

/// At 640x480 the expanded panels would cover the manager's frame: they fold by themselves
/// while it is open, and unfold from their headers.
#[test]
fn panels_fold_over_the_build_manager_640() {
    let size = (640, 480);
    let Some(src) = source() else { return };
    let mut ui = ui(src, size);
    let mut host = host();
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    assert!(modern_hud::panel(&ui.fe, size.0, size.1, &view).expanded);
    ui.open(&host, Manager::Construction);
    frame(&mut ui, &mut host);
    let view = ui.state().view.clone();
    let res = modern_hud::panel(&ui.fe, size.0, size.1, &view);
    let objs = modern_objectives::panel(&ui.fe, size.0, size.1, &view).unwrap();
    assert!(!res.expanded && !objs.expanded, "folded over the manager");
    let content = modern_hud::manager_content(size.0, size.1);
    assert!(!modern_hud::overlaps(res.panel, content));
    assert!(!modern_hud::overlaps(objs.panel, content));
    shot(&mut ui, size, "build_manager_640x480");
    let h = res.header;
    click_at(&mut ui, &mut host, h.x0 + h.height(), (h.y0 + h.y1) / 2);
    let view = ui.state().view.clone();
    assert!(
        modern_hud::panel(&ui.fe, size.0, size.1, &view).expanded,
        "unfolded on demand"
    );
    assert_eq!(ui.manager(), Some(Manager::Construction));
}

#[test]
fn objectives_panel_1080p() {
    run((1920, 1080), "1920x1080");
}

#[test]
fn objectives_panel_768() {
    run((1024, 768), "1024x768");
}
