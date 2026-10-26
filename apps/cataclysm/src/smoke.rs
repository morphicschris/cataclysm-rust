//! Headless smoke test of a mission through the game's own code path: the loading screen,
//! Mission 1 on the simulation, and a scripted input sequence (select ships, open the build
//! manager, queue a ship, issue a move) over a few hundred updates. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. With `HWC_SCREENSHOTS` set, PNGs of
//! the loading screen and the in-game view with the HUD and the managers go to
//! `screenshots/game/` (or to `HWC_SCREENSHOTS` when it names a directory).

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::command::order;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::consmgr::{self, Row};
use hwc_ui::ingame::host::BuildStatus;
use hwc_ui::ingame::{InputEvent, Manager};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Mode};
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1024, 768);
const DT: f32 = 1.0 / 16.0;

fn shot_dir() -> Option<PathBuf> {
    let v = std::env::var_os("HWC_SCREENSHOTS")?;
    // A directory, or any other value for the repository's `screenshots/game`.
    let dir = if std::path::Path::new(&v).is_dir() {
        PathBuf::from(v)
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game")
    };
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

fn save(canvas: &Canvas, name: &str) {
    if let Some(dir) = shot_dir() {
        canvas
            .save_png(&dir.join(format!("{name}.png")))
            .expect("png");
    }
}

struct Rig {
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
}

impl Rig {
    fn frame(&mut self, g: &mut Game) {
        let out = g.update(DT, &mut self.audio);
        assert!(out.is_empty(), "unexpected outcome {out:?}");
    }

    fn frames(&mut self, g: &mut Game, n: usize) {
        for _ in 0..n {
            self.frame(g);
        }
    }

    fn input(&mut self, g: &mut Game, ev: InputEvent) {
        g.input(ev, &mut self.audio);
    }

    fn key(&mut self, g: &mut Game, code: u16) {
        self.input(g, InputEvent::KeyDown(code));
        self.frame(g);
        self.input(g, InputEvent::KeyUp(code));
        self.frame(g);
    }

    fn click(&mut self, g: &mut Game, x: i32, y: i32, left: bool) {
        self.input(g, InputEvent::MouseMove { x, y });
        self.frame(g);
        self.input(g, InputEvent::MouseButton { left, down: true });
        self.frame(g);
        self.input(g, InputEvent::MouseButton { left, down: false });
        self.frame(g);
    }

    fn drag(&mut self, g: &mut Game, a: (i32, i32), b: (i32, i32)) {
        self.input(g, InputEvent::MouseMove { x: a.0, y: a.1 });
        self.input(
            g,
            InputEvent::MouseButton {
                left: true,
                down: true,
            },
        );
        self.frame(g);
        self.input(g, InputEvent::MouseMove { x: b.0, y: b.1 });
        self.frame(g);
        self.input(
            g,
            InputEvent::MouseButton {
                left: true,
                down: false,
            },
        );
        self.frame(g);
    }

    fn render(&mut self, g: &mut Game, name: &str) {
        g.render(&mut self.renderer, SIZE).expect("render");
        g.draw_cursor();
        save(g.canvas(), name);
    }

    /// Waits out the interface's double-click time.
    fn settle(&mut self, g: &mut Game) {
        self.frames(g, 12);
    }
}

fn setup() -> Option<(Rc<GameData>, Rig)> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue) = crate::test_gpu::device()?;
    let renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
    let read = |name: &str| {
        data.install_file(name)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let source: Rc<dyn AssetSource> = data.clone();
    let mut session = Session::new(
        source,
        Setup {
            options_path: None,
            original_cfg: read("Cataclysm.cfg"),
            default_keys: read("default_keys.cfg"),
            keys: None,
            keys_path: None,
            scale: ScaleMode::Original,
        },
    )
    .ok()?;
    session.resize(SIZE.0 as u32, SIZE.1 as u32);
    // Muted: the sound engine runs (speech lengths and subtitles) but no device is opened.
    let audio = GameData::open(&dir)
        .map_err(|e| e.to_string())
        .and_then(|g| crate::audio::Audio::open(g, false))
        .ok();
    Some((
        data,
        Rig {
            session,
            renderer,
            audio,
        },
    ))
}

/// The centre of a ship on screen.
fn ship_px(g: &Game, id: hwc_sim::ShipId) -> Option<(i32, i32)> {
    let s = g.world.universe().ship(id)?;
    let (lo, hi) = g.world.screen_box((SIZE.0 as u32, SIZE.1 as u32), s)?;
    Some((
        ((lo[0] + hi[0]) / 2.0) as i32,
        ((lo[1] + hi[1]) / 2.0) as i32,
    ))
}

#[test]
fn mission_one_plays_through_the_game_path() {
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    // Loading screen, then the mission.
    let mut loading = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
    // These tests start in the mission itself (the opening NIS has its own test).
    loading.opening_nis = false;
    loading.loader.play_nislets = false;
    save(loading.render(SIZE), "01_loading_start");
    let mut steps = 0;
    let mut game = loop {
        steps += 1;
        assert!(steps < 20, "loading never finished");
        match loading.advance(&rig.session, Some(&mut rig.renderer), DT) {
            Progress::Working => {
                if steps == 3 {
                    save(loading.render(SIZE), "02_loading_progress");
                }
            }
            Progress::Ready(g) => break g,
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    };
    let g = &mut *game;
    // The fleet arrives from hyperspace (update 220 in mission 1).
    rig.frames(g, 120);
    rig.render(g, "03_hyperspace_arrival");
    rig.frames(g, 140);
    assert!(
        g.world.run().game.ships_have_arrived,
        "fleet still in hyperspace"
    );
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("mothership");
    rig.render(g, "04_fleet_arrived");

    // Click the mothership: selected alone.
    let p = ship_px(g, ms).expect("mothership on screen");
    rig.click(g, p.0, p.1, true);
    assert_eq!(g.selection, vec![ms], "click selects the mothership");
    // Band box over the whole view: the player's ships.
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    assert!(g.selection.len() > 1, "band box selects the fleet");
    rig.render(g, "05_band_box_selection");

    // B: the build manager.
    rig.key(g, 0x42);
    assert_eq!(g.ui.manager(), Some(Manager::Construction));
    rig.render(g, "06_build_manager");
    // Queue the first buildable ship: select its row, add one, BUILD.
    let rows = consmgr::rows(&g.ui.state().view);
    let row = rows
        .iter()
        .position(|r| matches!(r, Row::Ship(o) if o.status == BuildStatus::Buildable))
        .expect("something buildable");
    let Row::Ship(opt) = &rows[row] else {
        unreachable!()
    };
    let ship_type = opt.ship_type;
    let line =
        g.ui.fe
            .resources()
            .font("default.hff")
            .map(|f| hwc_ui::text::height(&f) + 2)
            .expect("font");
    let list = g.ui.fe.find_atom("CM_Ships").expect("CM_Ships");
    let r = g.ui.fe.atom(list).rect();
    let (x, y) = (r.x0 + 40, r.y0 + 2 + row as i32 * line + line / 2);
    rig.click(g, x, y, true);
    rig.settle(g);
    rig.click(g, x, y, true);
    rig.settle(g);
    let b = g.ui.fe.find_atom("CM_BuildShips").expect("build button");
    let br = g.ui.fe.atom(b).rect();
    rig.click(g, (br.x0 + br.x1) / 2, (br.y0 + br.y1) / 2, true);
    let queued = g
        .world
        .universe()
        .base
        .factories
        .iter()
        .flat_map(|f| &f.jobs)
        .any(|j| j.n_jobs > 0 && j.info.is_some_and(|t| t.index() as u32 == ship_type));
    assert!(queued, "ship type {ship_type} queued at the factory");
    rig.frames(g, 16);
    rig.render(g, "07_build_manager_queued");
    // Escape closes the manager.
    rig.key(g, 0x1b);
    assert_eq!(g.ui.manager(), None);

    // Select the strike craft and workers (not the mothership) and move them.
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    g.selection.retain(|&id| id != ms);
    if g.selection.is_empty() {
        rig.click(g, p.0, p.1, true);
    }
    let movers = g.selection.clone();
    rig.key(g, 0x4d); // M
    assert!(matches!(g.mode, Mode::Move { .. }), "move plate up");
    rig.input(
        g,
        InputEvent::MouseMove {
            x: SIZE.0 / 2 + 150,
            y: SIZE.1 / 2 + 60,
        },
    );
    rig.frame(g);
    rig.render(g, "08_move_pie_plate");
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    rig.frame(g);
    assert_eq!(g.mode, Mode::Select);
    let u = g.world.universe();
    assert!(
        movers.iter().any(|&id| u
            .ship(id)
            .and_then(|s| s.command)
            .and_then(|c| u.commands.get(c))
            .is_some_and(|c| c.order == order::MOVE)),
        "move order given"
    );

    // Right click without moving on empty space: the move plate (the user-requested default);
    // Shift+right click and the original's right click: the menu.
    let menu_open = |g: &Game| hwc_ui::ingame::popup::is_open(&g.ui.fe);
    let candidates = [(1, 2), (1, 4), (3, 4), (1, 1), (3, 1), (3, 3), (1, 3)];
    let empty = candidates
        .iter()
        .map(|&(a, b)| (SIZE.0 * a / 4, SIZE.1 * b / 5))
        .find(|&(x, y)| {
            rig.input(g, InputEvent::MouseMove { x, y });
            rig.frame(g);
            g.hover.is_none()
        })
        .expect("some empty space on screen");
    let (ex, ey) = empty;
    rig.click(g, ex, ey, false);
    assert!(matches!(g.mode, Mode::Move { .. }), "right click opens the plate");
    assert!(!menu_open(g));
    rig.click(g, ex, ey, false);
    assert_eq!(g.mode, Mode::Select, "a second right click cancels it");
    rig.input(g, InputEvent::KeyDown(0x10));
    rig.click(g, ex, ey, false);
    rig.input(g, InputEvent::KeyUp(0x10));
    rig.frame(g);
    assert_eq!(g.mode, Mode::Select);
    assert!(menu_open(g), "Shift+right click opens the menu");
    rig.key(g, 0x1b);
    // On a ship: what it did before (here the menu or the ship's action), never the plate.
    if let Some(p) = ship_px(g, ms) {
        rig.input(g, InputEvent::MouseMove { x: p.0, y: p.1 });
        rig.frame(g);
        if g.hover.is_some() {
            rig.click(g, p.0, p.1, false);
            assert_eq!(g.mode, Mode::Select, "a right click on a ship opens no plate");
            if menu_open(g) {
                rig.key(g, 0x1b);
            }
        }
    }
    g.right_click_move = false;
    rig.click(g, ex, ey, false);
    assert_eq!(g.mode, Mode::Select);
    assert!(menu_open(g), "the original's right click opens the menu");
    rig.render(g, "09_right_click_menu");
    rig.key(g, 0x1b);
    g.right_click_move = true;

    // A few hundred updates more, then the research and launch managers.
    rig.frames(g, 300);
    rig.render(g, "10_in_game_hud");
    rig.key(g, 0x52); // R
    assert_eq!(g.ui.manager(), Some(Manager::Research));
    rig.render(g, "11_research_manager");
    rig.key(g, 0x1b);
    rig.key(g, 0x53); // S
    assert_eq!(g.ui.manager(), Some(Manager::Launch));
    rig.render(g, "12_launch_manager");
    rig.key(g, 0x1b);
    // Escape with nothing to cancel: the escape menu, which pauses.
    rig.key(g, 0x1b);
    rig.key(g, 0x1b);
    assert_eq!(g.ui.manager(), Some(Manager::EscMenu));
    assert!(g.paused);
    let t = g.world.universe().update_counter;
    rig.frames(g, 10);
    assert_eq!(g.world.universe().update_counter, t, "paused");
    rig.render(g, "13_escape_menu");
    assert!(g.world.universe().update_counter > 500);
}

fn save_sensors(canvas: &Canvas, name: &str) {
    if std::env::var_os("HWC_SCREENSHOTS").is_none() {
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/sensors");
    std::fs::create_dir_all(&dir).expect("dir");
    canvas
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

impl Rig {
    fn shot(&mut self, g: &mut Game, name: &str) {
        g.render(&mut self.renderer, SIZE).expect("render");
        g.draw_cursor();
        save_sensors(g.canvas(), name);
    }
}

/// The sensors manager (zoom out and back, fleet, ranges, selection and a move order inside
/// it) and the pointer for the modifiers and over an enemy. With `HWC_SCREENSHOTS` set the
/// frames go to `screenshots/sensors/`.
#[test]
fn sensors_manager_and_pointers() {
    use crate::sensors::Phase;
    use hwc_ui::ingame::cursor::CursorKind;
    const SPACE: u16 = 0x20;
    const CTRL: u16 = 0x11;
    const ALT: u16 = 0x12;
    const LEFT: u16 = 0x25;
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut loading = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
    // These tests start in the mission itself (the opening NIS has its own test).
    loading.opening_nis = false;
    loading.loader.play_nislets = false;
    let mut game = loop {
        match loading.advance(&rig.session, Some(&mut rig.renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => break g,
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    };
    let g = &mut *game;
    rig.frames(g, 270);
    assert!(g.world.run().game.ships_have_arrived);
    // The fleet, without the mothership.
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("mothership");
    // The build list is the simulation's (no Super Acolyte in Mission 1).
    let offered = hwc_sim::level::construction_list(g.world.universe(), ms);
    eprintln!("mission 1 build list: {offered:?}");
    assert!(offered
        .iter()
        .all(|t| !t.name().to_ascii_lowercase().contains("super")));
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    g.selection.retain(|&id| id != ms);
    assert!(!g.selection.is_empty(), "fleet selected");

    // Ctrl: the attack pointer and a red band box; Alt: the eye and a grey one.
    for (key, name, kind) in [
        (CTRL, "01_ctrl_attack_band_box", CursorKind::Attack),
        (ALT, "02_alt_focus_band_box", CursorKind::FocusNoSelect),
    ] {
        rig.input(g, InputEvent::KeyDown(key));
        rig.input(g, InputEvent::MouseMove { x: 300, y: 200 });
        rig.input(
            g,
            InputEvent::MouseButton {
                left: true,
                down: true,
            },
        );
        rig.frame(g);
        rig.input(g, InputEvent::MouseMove { x: 620, y: 450 });
        rig.frame(g);
        assert_eq!(g.cursor(), kind, "{name}");
        rig.shot(g, name);
        // Let go of the button away from everything with the modifier released first.
        rig.input(g, InputEvent::KeyUp(key));
        rig.input(g, InputEvent::MouseMove { x: 300, y: 200 });
        rig.input(
            g,
            InputEvent::MouseButton {
                left: true,
                down: false,
            },
        );
        rig.frame(g);
        rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
        g.selection.retain(|&id| id != ms);
    }
    // Over an enemy ship: the attack pointer. Put the camera on the nearest one.
    let u = g.world.universe();
    let ms_pos = u.ship(ms).unwrap().posinfo.position;
    let enemy = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| {
            u.ship(id)
                .is_some_and(|s| s.player != 0 && s.flags & hwc_sim::object::sof::HIDE == 0)
        })
        .min_by(|&a, &b| {
            let d = |id: hwc_sim::ShipId| {
                let p = u.ship(id).unwrap().posinfo.position;
                let v = glam::Vec3::new(p.x - ms_pos.x, p.y - ms_pos.y, p.z - ms_pos.z);
                v.length()
            };
            d(a).total_cmp(&d(b))
        });
    if let Some(e) = enemy {
        let keep = g.world.focus.clone();
        g.world.focus = vec![e];
        rig.frames(g, 2);
        if let Some(p) = ship_px(g, e) {
            rig.input(g, InputEvent::MouseMove { x: p.0, y: p.1 });
            rig.frame(g);
            rig.shot(g, "03_cursor_over_enemy");
            if !hwc_sim::command::attack_capable(g.world.universe(), &g.selection).is_empty() {
                assert_eq!(
                    g.cursor(),
                    CursorKind::Attack,
                    "attack pointer over an enemy"
                );
            }
        }
        g.world.focus = keep;
        rig.frames(g, 2);
    } else {
        eprintln!("no enemy ship in the level yet");
    }
    // Over the mothership with the fleet selected: docking.
    if let Some(p) = ship_px(g, ms) {
        rig.input(g, InputEvent::MouseMove { x: p.0, y: p.1 });
        rig.frame(g);
        rig.shot(g, "04_cursor_over_mothership");
    }

    // Space: the sensors manager zooms out.
    rig.input(
        g,
        InputEvent::MouseMove {
            x: SIZE.0 / 2 + 200,
            y: SIZE.1 / 2,
        },
    );
    rig.input(g, InputEvent::KeyDown(SPACE));
    rig.input(g, InputEvent::KeyUp(SPACE));
    rig.frame(g);
    assert!(g.sensors.is_open(), "sensors open");
    let mut k = 0;
    let mut phases = Vec::new();
    while g.sensors.zooming() {
        rig.frame(g);
        phases.push(g.sensors.phase());
        rig.shot(g, &format!("10_zoom_out_{k:02}"));
        k += 1;
        assert!(k < 40, "zoom never ends");
    }
    assert!(phases.contains(&Phase::Main) && phases.contains(&Phase::Sensors));
    rig.frames(g, 4);
    rig.shot(g, "20_sensors_view");
    // Pan left a little.
    let before = g.world.camera.look_at;
    rig.input(g, InputEvent::KeyDown(LEFT));
    rig.frames(g, 10);
    rig.input(g, InputEvent::KeyUp(LEFT));
    rig.frames(g, 10);
    assert!(g.world.camera.look_at.distance(before) > 1.0, "panned");
    rig.shot(g, "21_sensors_panned");
    // Band box in the sensors view selects the fleet again.
    g.selection.clear();
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    assert!(!g.selection.is_empty(), "band box in sensors");
    g.selection.retain(|&id| id != ms);
    rig.shot(g, "22_sensors_selected");
    // The move plate inside the sensors view, and the order.
    let movers = g.selection.clone();
    rig.key(g, 0x4d);
    assert!(matches!(g.mode, Mode::Move { .. }));
    rig.input(
        g,
        InputEvent::MouseMove {
            x: SIZE.0 / 2 + 120,
            y: SIZE.1 / 2 + 40,
        },
    );
    rig.frame(g);
    rig.shot(g, "23_sensors_move_plate");
    // Shift lifts the destination in the sensors manager too.
    rig.input(g, InputEvent::KeyDown(0x10));
    rig.frame(g);
    rig.shot(g, "23b_sensors_plate_shift");
    rig.input(
        g,
        InputEvent::MouseMove {
            x: SIZE.0 / 2 + 120,
            y: SIZE.1 / 2 - 20,
        },
    );
    rig.frame(g);
    rig.shot(g, "23c_sensors_plate_height");
    assert_eq!(g.plate().unwrap().spec, crate::plate::Spec::Height);
    assert!(g.plate().unwrap().spec_z > 100.0, "height {}", g.plate().unwrap().spec_z);
    rig.input(g, InputEvent::KeyUp(0x10));
    rig.frame(g);
    rig.shot(g, "23d_sensors_plate_released");
    assert_eq!(g.plate().unwrap().spec, crate::plate::Spec::Plane);
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    rig.frame(g);
    let u = g.world.universe();
    assert!(
        movers.iter().any(|&id| u
            .ship(id)
            .and_then(|s| s.command)
            .and_then(|c| u.commands.get(c))
            .is_some_and(|c| c.order == order::MOVE)),
        "move order from the sensors view"
    );
    // Ctrl in the sensors view: attack pointer and band box.
    rig.input(g, InputEvent::KeyDown(CTRL));
    rig.input(g, InputEvent::MouseMove { x: 200, y: 200 });
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    rig.frame(g);
    rig.input(g, InputEvent::MouseMove { x: 500, y: 400 });
    rig.frame(g);
    rig.shot(g, "24_sensors_ctrl_band_box");
    rig.input(g, InputEvent::KeyUp(CTRL));
    rig.input(g, InputEvent::MouseMove { x: 200, y: 200 });
    rig.input(
        g,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    rig.frame(g);
    // Space again: zoom back in.
    rig.input(g, InputEvent::KeyDown(SPACE));
    rig.input(g, InputEvent::KeyUp(SPACE));
    let mut k = 0;
    while g.sensors.is_open() {
        rig.frame(g);
        rig.shot(g, &format!("30_zoom_in_{k:02}"));
        k += 1;
        assert!(k < 40, "zoom in never ends");
    }
    rig.frames(g, 2);
    rig.shot(g, "31_back_in_main_view");
}

/// A small deterministic generator for the stress test's input.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn below(&mut self, n: u32) -> u32 {
        self.next() % n.max(1)
    }
}

/// Mission 1 played for a long time through the game path with random input (clicks, band
/// boxes, the managers, sensors, pause, Ctrl/Alt/Shift cursors, focus, speech skip), the
/// sound engine mixing as a device would and a frame drawn every few updates. Looks for
/// panics anywhere in the game. Slow: `HWC_STRESS_FRAMES` sets its length (default about 20
/// minutes of game time). Run with `--ignored --release`.
#[test]
#[ignore]
fn mission_one_random_play() {
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let total: u32 = std::env::var("HWC_STRESS_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 60 * 20);
    let seed: u64 = std::env::var("HWC_STRESS_SEED")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1);
    let mut rng = Lcg(seed);
    let load = |rig: &mut Rig| {
        let mut l = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
        l.opening_nis = false;
        for _ in 0..20 {
            if let Progress::Ready(g) = l.advance(&rig.session, Some(&mut rig.renderer), DT) {
                return g;
            }
        }
        panic!("loading never finished");
    };
    let mut game = load(&mut rig);
    const KEYS: [u16; 16] = [
        0x20, 0x1b, 0x42, 0x52, 0x53, 0x4d, 0x46, 0x48, 0x50, 0x13, 0x09, 0x31, 0x32, 0x44, 0x4b,
        0x5a,
    ];
    const MODS: [u16; 3] = [0x10, 0x11, 0x12];
    let mut held: Option<u16> = None;
    for frame in 0..total {
        let g = &mut *game;
        match rng.below(40) {
            0..=9 => {
                let (x, y) = (rng.below(SIZE.0 as u32) as i32, rng.below(SIZE.1 as u32) as i32);
                g.input(InputEvent::MouseMove { x, y }, &mut rig.audio);
            }
            10..=12 => {
                let left = rng.below(4) != 0;
                g.input(InputEvent::MouseButton { left, down: true }, &mut rig.audio);
                g.input(InputEvent::MouseButton { left, down: false }, &mut rig.audio);
            }
            13 => {
                let a = (rng.below(SIZE.0 as u32) as i32, rng.below(SIZE.1 as u32) as i32);
                let b = (rng.below(SIZE.0 as u32) as i32, rng.below(SIZE.1 as u32) as i32);
                g.input(InputEvent::MouseMove { x: a.0, y: a.1 }, &mut rig.audio);
                g.input(InputEvent::MouseButton { left: true, down: true }, &mut rig.audio);
                g.input(InputEvent::MouseMove { x: b.0, y: b.1 }, &mut rig.audio);
                g.input(InputEvent::MouseButton { left: true, down: false }, &mut rig.audio);
            }
            14 => {
                let k = KEYS[rng.below(KEYS.len() as u32) as usize];
                g.input(InputEvent::KeyDown(k), &mut rig.audio);
                g.input(InputEvent::KeyUp(k), &mut rig.audio);
            }
            15 => match held.take() {
                Some(m) => g.input(InputEvent::KeyUp(m), &mut rig.audio),
                None => {
                    let m = MODS[rng.below(3) as usize];
                    g.input(InputEvent::KeyDown(m), &mut rig.audio);
                    held = Some(m);
                }
            },
            16 => {
                // Right-drag: camera rotation.
                g.input(InputEvent::MouseButton { left: false, down: true }, &mut rig.audio);
                let (x, y) = (rng.below(SIZE.0 as u32) as i32, rng.below(SIZE.1 as u32) as i32);
                g.input(InputEvent::MouseMove { x, y }, &mut rig.audio);
                g.input(InputEvent::MouseButton { left: false, down: false }, &mut rig.audio);
            }
            _ => {}
        }
        let out = g.update(DT, &mut rig.audio);
        if let Some(a) = rig.audio.as_mut() {
            a.pump(6);
        }
        if frame % 4 == 0 {
            g.render(&mut rig.renderer, SIZE).expect("render");
            g.draw_cursor();
        }
        let mut reload = false;
        for o in out {
            match o {
                crate::game::Outcome::Animatic(_) => g.movie_finished(),
                crate::game::Outcome::ApplyOptions => {}
                _ => reload = true,
            }
        }
        if frame % (16 * 60) == 0 {
            eprintln!(
                "stress: frame {frame}, update {}, {} ships",
                g.world.universe().update_counter,
                g.world.universe().ship_list.len()
            );
        }
        if reload {
            eprintln!("stress: mission left at frame {frame}; starting again");
            game = load(&mut rig);
        }
    }
}

/// Mission 1 played by the simulation's autoplay (the fleet arrives, builds, harvests and
/// fights the Taiidan raiders in the asteroid field) through the game path with every frame
/// drawn, the camera kept on the fighting, effects and sounds live and the mixer pumped as a
/// device would. Looks for panics in combat: hits on ships and asteroids, explosions, hulks,
/// asteroid break-up and many sounds at once. `HWC_STRESS_FRAMES` sets its length. Run with
/// `--ignored --release`.
#[test]
#[ignore]
fn mission_one_autoplay_combat() {
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let total: u32 = std::env::var("HWC_STRESS_FRAMES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16 * 60 * 15);
    // `HWC_STRESS_SIZE` (WxH): the picture size.
    let size = std::env::var("HWC_STRESS_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or(SIZE);
    let mut slowest = 0.0f64;
    // `HWC_RENDER_FROM`: draw only from this frame on (the simulation runs the same).
    let render_from: u32 = std::env::var("HWC_RENDER_FROM")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut l = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
    l.opening_nis = false;
    let mut game = loop {
        if let Progress::Ready(g) = l.advance(&rig.session, Some(&mut rig.renderer), DT) {
            break g;
        }
    };
    let g = &mut *game;
    let mut play = hwc_sim::autoplay::Autoplay::new(16 * 60 * 30);
    let mut rng = Lcg(7);
    let (mut most_fx, mut most_ships) = (0usize, 0usize);
    for frame in 0..total {
        if g.nis.is_none() {
            play.tick(g.world.run_mut());
        }
        // Every few seconds the camera goes to a ship that is being shot at.
        if frame % 48 == 0 && g.nis.is_none() {
            let u = g.world.universe();
            let fighting: Vec<_> = u
                .ship_list
                .iter()
                .copied()
                .filter(|&id| u.ship(id).is_some_and(|s| s.recently_attacked != 0))
                .collect();
            if !fighting.is_empty() {
                let id = fighting[rng.below(fighting.len() as u32) as usize];
                let p = u.ship(id).map(|s| s.posinfo.position).unwrap();
                g.world.follow_focus = false;
                g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
                g.world.camera.distance = 2000.0 + rng.below(6000) as f32;
            }
        }
        let (x, y) = (rng.below(SIZE.0 as u32) as i32, rng.below(SIZE.1 as u32) as i32);
        g.input(InputEvent::MouseMove { x, y }, &mut rig.audio);
        let out = g.update(DT, &mut rig.audio);
        for o in out {
            match o {
                crate::game::Outcome::Animatic(_) => g.movie_finished(),
                crate::game::Outcome::ApplyOptions => {}
                o => {
                    eprintln!("combat: {o:?} at frame {frame}");
                }
            }
        }
        if let Some(a) = rig.audio.as_mut() {
            a.pump(6);
        }
        if frame >= render_from {
            let t = std::time::Instant::now();
            g.render(&mut rig.renderer, size).expect("render");
            let ms = t.elapsed().as_secs_f64() * 1e3;
            slowest = slowest.max(ms);
            if ms > 120.0 {
                let h = crate::draw_check::history();
                eprintln!("slow frame {frame}: {ms:.0} ms; {}", h.lines().last().unwrap_or(""));
            }
        }
        let u = g.world.universe();
        let fx = u.effects.as_ref().map_or(0, |e| e.world.effects.len());
        most_fx = most_fx.max(fx);
        most_ships = most_ships.max(u.ship_list.len());
        if frame % (16 * 30) == 0 {
            crate::crash::gpu_report(rig.renderer.device());
            eprintln!(
                "combat: frame {frame}, update {}, {} ships, {} effects, {} resources",
                u.update_counter,
                u.ship_list.len(),
                fx,
                u.base.resources.list.len()
            );
        }
        if g.world.mission_run().is_some_and(|r| r.mission_complete() || r.game.mission_failed) {
            eprintln!("combat: mission over at frame {frame}");
            break;
        }
    }
    eprintln!("combat: at most {most_fx} effects and {most_ships} ships; slowest frame {slowest:.0} ms");
}


/// Hot-key groups: Ctrl+1 makes the selection group 1, 1 selects it again.
#[test]
fn hot_key_groups() {
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut loading = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
    loading.opening_nis = false;
    loading.loader.play_nislets = false;
    let mut game = loop {
        if let Progress::Ready(g) = loading.advance(&rig.session, Some(&mut rig.renderer), DT) {
            break g;
        }
    };
    let g = &mut *game;
    rig.frames(g, 260);
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    let fleet = g.selection.clone();
    assert!(!fleet.is_empty(), "nothing selected");
    rig.input(g, InputEvent::KeyDown(0x11));
    rig.key(g, 0x31);
    rig.input(g, InputEvent::KeyUp(0x11));
    rig.frame(g);
    // Clicking empty space clears the selection.
    rig.click(g, 3, SIZE.1 / 2, true);
    rig.settle(g);
    g.selection.clear();
    rig.key(g, 0x31);
    let mut got = g.selection.clone();
    let mut want = fleet.clone();
    got.sort();
    want.sort();
    assert_eq!(got, want, "1 did not select group 1");
    assert_eq!(g.hotkeys.group_of(fleet[0]), Some(1));
    // The number is drawn over the selected ships, at the right end of their bars.
    let mut draw = |g: &mut Game| {
        g.render(&mut rig.renderer, SIZE).expect("render");
        g.canvas().pixels.clone()
    };
    let with = draw(g);
    let groups = std::mem::take(&mut g.hotkeys);
    let without = draw(g);
    g.hotkeys = groups;
    let changed = with.iter().zip(&without).filter(|(a, b)| a != b).count();
    assert!(changed > 0, "no hot-key number drawn");
    rig.render(g, "hot_key_numbers");
    // Pressing 1 again with the group selected focuses the camera on it.
    g.world.focus.clear();
    rig.key(g, 0x31);
    let mut focus = g.world.focus.clone();
    focus.sort();
    assert_eq!(focus, want, "1 again did not focus on group 1");
    // Shift+1 adds the group to the selection; Ctrl+2 takes ships out of group 1.
    let one = vec![fleet[0]];
    g.selection = one.clone();
    rig.input(g, InputEvent::KeyDown(0x11));
    rig.key(g, 0x32);
    rig.input(g, InputEvent::KeyUp(0x11));
    assert_eq!(g.hotkeys.group_of(fleet[0]), Some(2));
    g.selection.clear();
    rig.input(g, InputEvent::KeyDown(0x10));
    rig.key(g, 0x31);
    rig.input(g, InputEvent::KeyUp(0x10));
    let mut got = g.selection.clone();
    got.sort();
    let rest: Vec<_> = want.iter().copied().filter(|id| *id != fleet[0]).collect();
    assert_eq!(got, rest, "Shift+1 did not add group 1");
    // Ctrl+` with exactly group 2 selected removes it.
    g.selection = one;
    rig.input(g, InputEvent::KeyDown(0x11));
    rig.key(g, 0xc0);
    rig.input(g, InputEvent::KeyUp(0x11));
    assert_eq!(g.hotkeys.group_of(fleet[0]), None, "Ctrl+` did not remove group 2");
}

/// Frame times of the sensors manager in Mission 1 (printed; `--ignored`).
#[test]
#[ignore]
fn sensors_frame_times() {
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let size = (1600, 900);
    let mut loading = Loading::new(data.clone(), &rig.session, 1, 2, size).expect("loading");
    loading.opening_nis = false;
    loading.loader.play_nislets = false;
    let mut game = loop {
        if let Progress::Ready(g) = loading.advance(&rig.session, Some(&mut rig.renderer), DT) {
            break g;
        }
    };
    let g = &mut *game;
    rig.frames(g, 16 * 30);
    let time = |g: &mut Game, rig: &mut Rig, n: usize| {
        let mut v = Vec::new();
        for _ in 0..n {
            let t = std::time::Instant::now();
            g.update(DT, &mut rig.audio);
            let u = t.elapsed();
            g.render(&mut rig.renderer, size).expect("render");
            v.push((u, t.elapsed()));
        }
        let total: Vec<f64> = v.iter().map(|x| x.1.as_secs_f64() * 1e3).collect();
        let upd: f64 = v.iter().map(|x| x.0.as_secs_f64() * 1e3).sum::<f64>() / n as f64;
        let mean = total.iter().sum::<f64>() / n as f64;
        let max = total.iter().cloned().fold(0.0, f64::max);
        (upd, mean, max)
    };
    let main = time(g, &mut rig, 64);
    rig.key(g, 0x20);
    rig.frames(g, 40);
    assert!(g.sensors.is_open(), "sensors did not open");
    let sensors = time(g, &mut rig, 64);
    eprintln!("main view: update {:.1} ms, frame {:.1} ms (max {:.1})", main.0, main.1, main.2);
    eprintln!("sensors:   update {:.1} ms, frame {:.1} ms (max {:.1})", sensors.0, sensors.1, sensors.2);
}

/// Full detail draws every ship at its top level while the original's level is still worked
/// out; the move plate's height mode follows the pointer up the vertical and survives a Shift
/// release and press; the order goes to the raised point.
#[test]
fn full_detail_and_the_move_plate_height() {
    const SHIFT: u16 = 0x10;
    let Some((data, mut rig)) = setup() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut loading = Loading::new(data.clone(), &rig.session, 1, 2, SIZE).expect("loading");
    loading.opening_nis = false;
    loading.loader.play_nislets = false;
    let mut game = loop {
        match loading.advance(&rig.session, Some(&mut rig.renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => break g,
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    };
    let g = &mut *game;
    rig.frames(g, 260);
    let px = (SIZE.0 as u32, SIZE.1 as u32);

    // Level of detail: from far away the original's selection drops levels; full detail
    // draws level 0 and still reports the original's level.
    assert!(g.world.full_detail, "full detail is the default");
    let distance = g.world.camera.distance;
    g.world.camera.distance = 60_000.0;
    g.world.full_detail = false;
    for _ in 0..3 {
        let _ = g.world.frame(px);
    }
    let original = g.world.frame(px).ship_levels();
    assert!(original.iter().any(|&(_, l)| l > 0), "nothing dropped a level: {original:?}");
    g.world.full_detail = true;
    let full = g.world.frame(px).ship_levels();
    assert!(!full.is_empty() && full.iter().all(|&(_, l)| l == 0), "{full:?}");
    assert!(full.iter().any(|&(id, _)| g.world.ship_lod(id).unwrap_or(0) > 0));
    g.world.camera.distance = distance;
    rig.frames(g, 4);

    // The plate: select the movable ships and press M.
    rig.drag(g, (2, 2), (SIZE.0 - 3, SIZE.1 - 3));
    let ms = g.world.universe().base.players[0].mothership;
    g.selection.retain(|&id| Some(id) != ms);
    assert!(!g.selection.is_empty());
    rig.key(g, 0x4d);
    assert!(g.plate().is_some(), "plate up");
    let start = (SIZE.0 / 2 + 120, SIZE.1 / 2 + 80);
    rig.input(g, InputEvent::MouseMove { x: start.0, y: start.1 });
    rig.frame(g);
    rig.render(g, "30_plate_plane");
    let plate = *g.plate().unwrap();
    assert_eq!(plate.spec, crate::plate::Spec::Plane);
    assert_eq!(plate.spec_z, 0.0);
    let foot = plate.plane;
    // Shift: height mode; the pointer is put back on the plane point.
    rig.input(g, InputEvent::KeyDown(SHIFT));
    rig.frame(g);
    rig.render(g, "31_plate_shift");
    assert_eq!(g.plate().unwrap().spec, crate::plate::Spec::Height);
    let _ = g.take_pointer_warp();
    // Up the screen, a little to the side: the destination rises over the same foot.
    rig.input(g, InputEvent::MouseMove { x: start.0 + 15, y: start.1 - 90 });
    rig.frame(g);
    rig.render(g, "32_plate_height");
    let plate = *g.plate().unwrap();
    assert!(plate.spec_z > 100.0, "height {}", plate.spec_z);
    assert!(plate.height.truncate().distance(foot.truncate()) < 1.0);
    assert!(g.take_pointer_warp().is_some(), "the pointer is held on the vertical");
    // Higher on the screen is higher in space.
    rig.input(g, InputEvent::MouseMove { x: start.0, y: start.1 - 150 });
    rig.frame(g);
    rig.render(g, "33_plate_higher");
    let higher = g.plate().unwrap().spec_z;
    assert!(higher > plate.spec_z, "{higher} <= {}", plate.spec_z);
    // Shift up: back on the plate, the height kept.
    rig.input(g, InputEvent::KeyUp(SHIFT));
    rig.frame(g);
    rig.render(g, "34_plate_released");
    let released = *g.plate().unwrap();
    assert_eq!(released.spec, crate::plate::Spec::Plane);
    assert!((released.height.z - released.plane.z - higher).abs() < 1.0);
    // Shift again: height mode from that height.
    rig.input(g, InputEvent::KeyDown(SHIFT));
    rig.frame(g);
    rig.render(g, "35_plate_again");
    assert_eq!(g.plate().unwrap().spec, crate::plate::Spec::Height);
    assert!((g.plate().unwrap().spec_z - higher).abs() < 60.0);
    rig.input(g, InputEvent::KeyUp(SHIFT));
    rig.frame(g);
    rig.render(g, "36_plate_before_click");
    // Click: the ships go to the raised point.
    let dest = g.plate().unwrap().destination();
    assert!(dest.z - g.plate().unwrap().centre.z > 100.0);
    rig.input(g, InputEvent::MouseButton { left: true, down: true });
    rig.input(g, InputEvent::MouseButton { left: true, down: false });
    rig.frame(g);
    assert_eq!(g.mode, Mode::Select);
    let u = g.world.universe();
    let moving = g.selection.iter().any(|&id| {
        u.ship(id)
            .and_then(|s| s.command)
            .and_then(|c| u.commands.get(c))
            .is_some_and(|c| c.order == order::MOVE)
    });
    assert!(moving, "move order given");
}
