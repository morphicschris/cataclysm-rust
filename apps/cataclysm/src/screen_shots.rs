//! Game-data checks and screenshots of very wide and very large screens (the screen modes, a
//! user-requested addition): mission 1 at 5120x1440 (32:9), 3440x1440 (21:9) and 3840x2160
//! (16:9, 4K) with the modern HUD, the build manager, the cutscene letterbox and the sensors
//! manager, then the original HUD and the original build manager (the original's fixed
//! 640x480 layouts, placed by `feResRepositionX/Y`), and the main menu's video options with the
//! screen modes and a full-screen resolution list. The 3D view keeps the original's vertical
//! field of view (Hor+: the wider screen sees more to the sides, nothing is stretched).
//! Screenshots go to `screenshots/screen/`. Needs the game install (`HWC_GAME_DIR`) and a GPU
//! adapter; skips otherwise. Slow in a debug build: run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_sim::mission::sp_state;
use hwc_ui::canvas::{Canvas, Rect};
use hwc_ui::display::{Mode, Monitor, Setting, VideoMode};
use hwc_ui::ingame::{modern_build_manager as mbm, modern_hud, modern_panel, InputEvent, Manager};
use hwc_ui::letterbox::{bar_height, tall_bar_height};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::m1_checks::{mission_one, Rig};
use crate::sensors::Phase;

const BLACK: [u8; 4] = [0, 0, 0, 255];

fn save(canvas: &Canvas, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/screen");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    canvas
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

fn boxed(g: &Game) -> bool {
    g.letterbox.fully || g.nis.is_some()
}

fn shot(g: &mut Game, rig: &mut Rig, name: &str) {
    rig.render(g);
    rig.render(g);
    save(g.canvas(), name);
}

fn key(g: &mut Game, rig: &mut Rig, code: u16) {
    g.input(InputEvent::KeyDown(code), &mut rig.audio);
    rig.frame(g);
    g.input(InputEvent::KeyUp(code), &mut rig.audio);
    rig.frame(g);
}

fn inside(r: Rect, w: i32, h: i32) -> bool {
    r.x0 >= 0 && r.y0 >= 0 && r.x1 <= w && r.y1 <= h && r.x0 < r.x1 && r.y0 < r.y1
}

/// Rows of the canvas that are black from end to end.
fn black_row(c: &Canvas, y: i32) -> bool {
    (0..c.width).step_by(7).all(|x| c.get(x, y) == BLACK)
}

/// Mission 1 at a wide screen size, every view checked and shot.
fn wide_screen(size: (i32, i32), tag: &str) {
    let Some((mut game, mut rig)) = mission_one(size, false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let (w, h) = size;
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    for _ in 0..48 {
        rig.frame(g);
    }

    // The 3D view: the original's vertical field of view at every shape (Hor+), square pixels.
    let fov = g.world.camera.tweaks.field_of_view.to_radians();
    let aspect = w as f32 / h as f32;
    let p = g.world.camera.projection(aspect);
    let ys = 1.0 / (fov / 2.0).tan();
    assert!((p.y_axis.y - ys).abs() < 1e-4, "vertical FOV kept at {tag}");
    assert!(
        (p.x_axis.x * aspect - p.y_axis.y).abs() < 1e-4,
        "no stretching at {tag}"
    );
    let hfov = 2.0 * ((fov / 2.0).tan() * aspect).atan();
    eprintln!(
        "{tag}: vertical FOV {:.1} deg, horizontal {:.1} deg",
        fov.to_degrees(),
        hfov.to_degrees()
    );

    // The modern HUD: the panels in the corners, inside the screen.
    shot(g, &mut rig, &format!("m1_hud_{tag}"));
    assert_eq!((g.canvas().width, g.canvas().height), size);
    let view = g.ui.state().view.clone();
    let arr = modern_panel::arrange(&g.ui.fe, w, h, &view);
    let rects = arr.rects();
    assert!(!rects.is_empty());
    for r in &rects {
        assert!(inside(*r, w, h), "{r:?} off the {tag} screen");
        let (left, right) = (r.x0, w - r.x1);
        assert!(
            left.min(right) <= modern_hud::scale_for(h).ceil() as i32 * 24,
            "{r:?} not at a side of the {tag} screen"
        );
    }
    let res = arr.resources.as_ref().expect("the resources panel");
    assert!(
        w - res.panel.x1 < w / 50,
        "resources at the right edge: {:?}",
        res.panel
    );

    // The build manager: centred, inside, clear of the HUD's panels.
    key(g, &mut rig, b'B' as u16);
    assert_eq!(g.ui.manager(), Some(Manager::Construction));
    for _ in 0..4 {
        rig.frame(g);
    }
    shot(g, &mut rig, &format!("m1_build_manager_{tag}"));
    let l = mbm::layout(w, h);
    assert!(inside(l.panel, w, h), "{:?} at {tag}", l.panel);
    assert!(
        ((l.panel.x0 + l.panel.x1) / 2 - w / 2).abs() <= 2,
        "the manager is centred at {tag}: {:?}",
        l.panel
    );
    let view = g.ui.state().view.clone();
    for r in modern_panel::arrange(&g.ui.fe, w, h, &view).rects() {
        assert!(!modern_hud::overlaps(r, l.panel), "{r:?} over the manager");
    }
    key(g, &mut rig, 27);
    assert_eq!(g.ui.manager(), None);

    // The letterbox: the tall bars (the default) are 14% of the height; the original's
    // 1.85:1 bars are none at all on a screen wider than 1.85:1.
    g.letterbox.fully = true;
    g.letterbox.tall = true;
    rig.frame(g);
    shot(g, &mut rig, &format!("m1_letterbox_{tag}"));
    let full = tall_bar_height(w, h);
    assert_eq!(full, bar_height(h * 4 / 3, h));
    let c = g.canvas();
    assert!(
        (h - full..h).all(|y| black_row(c, y)),
        "the bottom bar at {tag}"
    );
    assert!(
        !black_row(c, h - full - 2),
        "the bar is {full} tall at {tag}"
    );
    assert_eq!(
        bar_height(w, h),
        if aspect > 1.85 { 0 } else { bar_height(w, h) }
    );
    g.letterbox.fully = false;
    rig.frame(g);

    // The sensors manager.
    g.toggle_sensors(&mut rig.audio);
    let mut n = 0;
    while g.sensors.phase() != Phase::Sensors || g.sensors.zooming() {
        rig.frame(g);
        rig.render(g);
        n += 1;
        assert!(n < 16 * 10, "the sensors manager did not open");
    }
    for _ in 0..8 {
        rig.frame(g);
    }
    shot(g, &mut rig, &format!("m1_sensors_{tag}"));
    let c = g.canvas();
    // The view fills the screen: something is drawn near each side.
    let drawn =
        |x0: i32, x1: i32| (x0..x1).any(|x| (0..h).step_by(5).any(|y| c.get(x, y) != BLACK));
    assert!(
        drawn(0, w / 20) && drawn(w - w / 20, w),
        "the sensors view at {tag}"
    );
    g.toggle_sensors(&mut rig.audio);
    for _ in 0..32 {
        rig.frame(g);
    }

    // The original HUD and the original build manager: their 640x480 layouts placed on the
    // big screen (the taskbar and the overlay in their corners, the manager centred).
    g.ui.fe.game.options.set_bool(modern_hud::OPTION_KEY, false);
    for _ in 0..4 {
        rig.frame(g);
    }
    shot(g, &mut rig, &format!("m1_original_hud_{tag}"));
    key(g, &mut rig, b'B' as u16);
    assert_eq!(g.ui.manager(), Some(Manager::Construction));
    assert!(!mbm::active(&g.ui.fe));
    for _ in 0..4 {
        rig.frame(g);
    }
    shot(g, &mut rig, &format!("m1_original_build_manager_{tag}"));
    let screen = g.ui.fe.current_screen().expect("the manager's screen");
    let atoms = &g.ui.fe.screens[screen].atoms;
    let (x0, x1) = atoms.iter().fold((i32::MAX, i32::MIN), |(a, b), at| {
        let r = at.rect();
        (a.min(r.x0), b.max(r.x1))
    });
    assert!(
        x0 >= 0 && x1 <= w,
        "the original manager inside at {tag}: {x0}..{x1}"
    );
    key(g, &mut rig, 27);
}

#[test]
fn wide_screen_5120x1440() {
    wide_screen((5120, 1440), "5120x1440");
}

#[test]
fn wide_screen_3440x1440() {
    wide_screen((3440, 1440), "3440x1440");
}

#[test]
fn wide_screen_3840x2160() {
    wide_screen((3840, 2160), "3840x2160");
}

/// The main menu's video options at 5120x1440: the screen modes in the rendering systems'
/// list, the monitor's full-screen modes with their refresh rates, and the borderless
/// window's greyed desktop resolution.
#[test]
fn video_options_list_the_screen_modes() {
    let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else {
        eprintln!("skipped: HWC_GAME_DIR missing");
        return;
    };
    let Ok(data) = GameData::open(&dir) else {
        return;
    };
    let data = Rc::new(data);
    let source: Rc<dyn AssetSource> = data.clone();
    let mut s = Session::new(
        source,
        Setup {
            scale: ScaleMode::Original,
            ..Setup::default()
        },
    )
    .expect("session");
    let size = (5120, 1440);
    s.resize(size.0, size.1);
    let mut modes = Vec::new();
    for (w, h) in [
        (640, 480),
        (800, 600),
        (1024, 768),
        (1280, 1024),
        (1920, 1080),
        (2560, 1440),
        (3840, 1080),
        (5120, 1440),
    ] {
        for r in [60_000, 120_000, 239_757] {
            modes.push(VideoMode::new(w, h, r));
        }
    }
    let monitor = Monitor {
        size: (5120, 1440),
        refresh_mhz: 239_757,
        modes,
    };
    s.set_display(Some(monitor), Setting::default());
    let shoot = |s: &mut Session, mode: Mode, name: &str| {
        s.fe.game.options.set("hwcDisplayMode", mode.option_value());
        s.fe.game.options.set("screenWidth", 5120);
        s.fe.game.options.set("screenHeight", 1440);
        s.fe.game.options_snapshot();
        s.fe.all_screens_delete();
        s.fe.screen_start("Video_Options");
        for _ in 0..3 {
            s.update(1.0 / 30.0);
        }
        let a =
            s.fe.find_atom("OP_Resolution")
                .expect("the resolution list");
        let rows: Vec<String> =
            s.fe.list(a)
                .expect("list")
                .items
                .iter()
                .map(|i| i.text.clone())
                .collect();
        save(s.render(), name);
        rows
    };
    let rows = shoot(&mut s, Mode::Fullscreen, "fe_video_fullscreen_5120x1440");
    assert_eq!(rows.len(), 8 * 3);
    assert!(rows.contains(&"5120x1440 @ 240Hz".to_string()), "{rows:?}");
    // A window: the listed sizes that fit the desktop (not 2560x1600, 3840x1600, 3840x2160).
    let rows = shoot(&mut s, Mode::Windowed, "fe_video_window_5120x1440");
    assert_eq!(rows.len(), 16);
    let rows = shoot(&mut s, Mode::Borderless, "fe_video_borderless_5120x1440");
    assert_eq!(rows, ["5120x1440 desktop"]);
    // The main menu itself at 32:9.
    s.fe.all_screens_delete();
    s.fe.screen_start(hwc_ui::session::MAIN_SCREEN);
    for _ in 0..3 {
        s.update(1.0 / 30.0);
    }
    save(s.render(), "fe_main_menu_5120x1440");
}
