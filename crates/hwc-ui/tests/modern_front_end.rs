//! The modern front end (`hwcModernFrontEnd = 1`, the default) on the real game files: the
//! main menu and every screen behind it are the modern menus, built from the original
//! screens, which stay open underneath and do the work. Walks the menus with the mouse as a
//! player would (single player and its tabs, team colours, the options and their tabs, the
//! modern options, training, quit) at several window sizes, checks the panels stay in the
//! window on the left, and with `--front-end original` (`hwcModernFrontEnd = 0`) the original
//! screens come back. Screenshots go to `screenshots/front_end/` when `HWC_SCREENSHOTS` is
//! set. Needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::front_menu;
use hwc_ui::ingame::modern_menu::{self as menu, Kind, Layout, Model};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn session(src: Rc<dyn AssetSource>, size: (u32, u32)) -> Session {
    let mut s = Session::new(
        src,
        Setup {
            scale: ScaleMode::Original,
            ..Default::default()
        },
    )
    .expect("session");
    s.resize(size.0, size.1);
    settle(&mut s);
    s
}

fn frame(s: &mut Session) -> Vec<Request> {
    s.update(1.0 / 60.0)
}

fn settle(s: &mut Session) -> Vec<Request> {
    let mut out = Vec::new();
    for _ in 0..40 {
        out.extend(frame(s));
    }
    out
}

fn now(s: &Session) -> (Model, Layout) {
    let m = menu::model(&s.fe).expect("a modern screen");
    let (w, h) = s.fe.screen_size();
    let l = menu::layout_for(&s.fe, &m, w, h);
    (m, l)
}

/// Clicks the control labelled `label` on the modern screen.
fn click(s: &mut Session, label: &str) -> Vec<Request> {
    let mut out = settle(s);
    let (m, l) = now(s);
    let i = m
        .find(label)
        .unwrap_or_else(|| panic!("no {label} on {}: {:?}", m.name, labels(&m)));
    let r = l.placed[i].control;
    out.extend(click_at(s, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2));
    out
}

fn click_at(s: &mut Session, x: i32, y: i32) -> Vec<Request> {
    let mut out = Vec::new();
    s.mouse_move(f64::from(x), f64::from(y));
    out.extend(frame(s));
    s.mouse_button(true, true);
    out.extend(frame(s));
    s.mouse_button(true, false);
    out.extend(settle(s));
    out
}

fn labels(m: &Model) -> Vec<String> {
    m.items.iter().map(|i| i.label.clone()).collect()
}

fn top(s: &Session) -> String {
    s.fe.current_screen_name().unwrap_or("").to_string()
}

fn shot(s: &mut Session, name: &str) {
    let c = s.render();
    // The panels never leave the window.
    if std::env::var_os("HWC_SCREENSHOTS").is_some() {
        let dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/front_end");
        c.save_png(&dir.join(format!("{name}.png"))).expect("png");
    }
}

/// The screen on top is a modern one, its panel inside the window and on its left half.
fn check_panel(s: &Session) {
    let (m, l) = now(s);
    let (w, h) = s.fe.screen_size();
    let p = l.frame.panel;
    assert!(
        p.x0 >= 0 && p.y0 >= 0 && p.x1 <= w && p.y1 <= h,
        "{}: panel {p:?} out of {w}x{h}",
        m.name
    );
    if w >= 1280 {
        assert!(p.x0 < w / 8, "{}: panel {p:?} not on the left", m.name);
    }
    for (it, pl) in m.items.iter().zip(&l.placed) {
        if it.footer || l.body.is_none() {
            assert!(
                pl.row.x0 >= p.x0 && pl.row.x1 <= p.x1,
                "{}: {} {:?} outside the panel {p:?}",
                m.name,
                it.label,
                pl.row
            );
        }
    }
}

#[test]
fn the_main_menu_and_its_screens_are_modern() {
    let Some(src) = source() else { return };
    for size in [(1920, 1080), (1280, 720), (1024, 768), (640, 480)] {
        let tag = format!("{}x{}", size.0, size.1);
        let mut s = session(src.clone(), size);
        assert!(s.modern(), "on by default");
        let (m, _) = now(&s);
        assert_eq!(m.name, front_menu::MAIN);
        assert_eq!(
            labels(&m),
            [
                "TRAINING",
                "SINGLE PLAYER",
                "MULTIPLAYER",
                "OPTIONS",
                "CREDITS",
                "QUIT"
            ]
        );
        check_panel(&s);
        shot(&mut s, &format!("main_{tag}"));

        // Single player: three tabs, the team colours shown from the original's drawing.
        click(&mut s, "SINGLE PLAYER");
        assert_eq!(top(&s), "Create_new_game");
        let (m, _) = now(&s);
        let tabs: Vec<(String, bool)> = m
            .items
            .iter()
            .filter_map(|i| i.tab.map(|own| (i.label.clone(), own)))
            .collect();
        assert_eq!(
            tabs,
            [
                ("NEW GAME".to_string(), true),
                ("PLAYER SETUP".to_string(), false),
                ("LOAD GAME".to_string(), false)
            ]
        );
        assert!(m.items.iter().any(|i| matches!(i.kind, Kind::Embed { .. })));
        check_panel(&s);
        shot(&mut s, &format!("single_player_{tag}"));
        click(&mut s, "PLAYER SETUP");
        assert_eq!(top(&s), "Select_colour");
        check_panel(&s);
        shot(&mut s, &format!("player_setup_{tag}"));
        click(&mut s, "LOAD GAME");
        assert_eq!(top(&s), "Load_game");
        check_panel(&s);
        shot(&mut s, &format!("load_game_{tag}"));
        click(&mut s, "BACK");
        assert_eq!(top(&s), front_menu::MAIN);

        // The options: four tabs and ENHANCEMENTS.
        click(&mut s, "OPTIONS");
        assert_eq!(top(&s), "Audio_Options");
        let (m, _) = now(&s);
        assert!(m.find("ENHANCEMENTS").is_some(), "{:?}", labels(&m));
        check_panel(&s);
        shot(&mut s, &format!("audio_{tag}"));
        for (tab, screen) in [
            ("VIDEO", "Video_Options"),
            ("GAMEPLAY", "Gameplay_Options"),
            ("KEY BINDINGS", "Keyboard"),
        ] {
            click(&mut s, tab);
            assert_eq!(top(&s), screen);
            check_panel(&s);
            shot(&mut s, &format!("{}_{tag}", screen.to_lowercase()));
        }
        click(&mut s, "ENHANCEMENTS");
        assert!(s.fe.game.ig.mmenu.page.is_some());
        check_panel(&s);
        shot(&mut s, &format!("enhancements_{tag}"));
        click(&mut s, "CANCEL");
        assert!(s.fe.game.ig.mmenu.page.is_none());
        assert_eq!(top(&s), "Keyboard");
        click(&mut s, "CANCEL");
        assert_eq!(top(&s), front_menu::MAIN);

        click(&mut s, "TRAINING");
        assert_eq!(top(&s), "Select_tutorial");
        check_panel(&s);
        shot(&mut s, &format!("training_{tag}"));
        click(&mut s, "BACK");

        click(&mut s, "QUIT");
        assert_eq!(top(&s), "Quit_game");
        check_panel(&s);
        shot(&mut s, &format!("quit_{tag}"));
        let out = click(&mut s, "YES");
        assert!(out.contains(&Request::Quit), "{out:?}");
    }
}

#[test]
fn the_modern_options_switch_the_front_end_back_to_the_original() {
    let Some(src) = source() else { return };
    let mut s = session(src, (1920, 1080));
    click(&mut s, "OPTIONS");
    click(&mut s, "ENHANCEMENTS");
    click(&mut s, "MODERN MAIN MENU WITH A LIVE SCENE");
    let out = click(&mut s, "ACCEPT");
    assert!(out.contains(&Request::ApplyOptions), "{out:?}");
    assert!(!s.modern(), "the original front end at once");
    assert_eq!(s.fe.game.options.get(front_menu::OPTION_KEY), 0);
}

#[test]
fn the_original_front_end_stays_when_asked_for() {
    let Some(src) = source() else { return };
    let mut s = session(src, (1280, 720));
    s.use_original_look();
    assert!(!s.modern());
    s.fe.game.ig.mmenu.front_end = true;
    s.fe.game.options.set(front_menu::OPTION_KEY, 0);
    assert!(!s.modern());
}
