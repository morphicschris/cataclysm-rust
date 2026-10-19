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
        let (mx, my) = g.mouse();
        self.session.draw_pointer(g.canvas_mut(), mx, my);
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
    let (device, queue) = hwc_render::headless_device().ok()?;
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
        g.world.run.game.ships_have_arrived,
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

    // Right click without moving: the menu.
    rig.click(g, SIZE.0 / 2, SIZE.1 / 2, false);
    rig.render(g, "09_right_click_menu");
    rig.key(g, 0x1b);

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
