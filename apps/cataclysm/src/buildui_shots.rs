//! Game-data screenshots of the modern build manager (`hwc_ui::ingame::modern_build_manager`,
//! `--build-ui modern`, the default) in mission 1: acolytes and recons in production at the
//! Kuun-Lan, more picked in the manager, a ship in the view, at 1920x1080, 1024x768 and
//! 640x480 (and with Info on). Screenshots go to `screenshots/buildui/`. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Slow in a debug build: run with
//! `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::sp_state;
use hwc_sim::statics::ShipType;
use hwc_ui::canvas::Rect;
use hwc_ui::ingame::consmgr::{self, Row};
use hwc_ui::ingame::modern_build_manager::{self as mbm, Button};
use hwc_ui::ingame::{modern_hud, modern_panel, InputEvent, Manager};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
    size: (i32, i32),
}

fn rig(size: (i32, i32)) -> Option<Rig> {
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
    session.resize(size.0 as u32, size.1 as u32);
    Some(Rig {
        data,
        session,
        renderer,
        audio: None,
        size,
    })
}

impl Rig {
    fn mission_one(&mut self) -> Box<Game> {
        let mut l =
            Loading::new(self.data.clone(), &self.session, 1, 2, self.size).expect("loading");
        l.opening_nis = false;
        l.loader.play_nislets = false;
        for _ in 0..40 {
            match l.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
            }
        }
        panic!("loading never finished");
    }

    fn frame(&mut self, g: &mut Game) {
        let out = g.update(DT, &mut self.audio);
        assert!(
            !out.contains(&Outcome::NextMission),
            "mission 1 ended: {out:?}"
        );
    }

    fn input(&mut self, g: &mut Game, ev: InputEvent) {
        g.input(ev, &mut self.audio);
    }

    /// A click clear of any double-click time.
    fn click(&mut self, g: &mut Game, x: i32, y: i32) {
        for _ in 0..10 {
            self.frame(g);
        }
        self.input(g, InputEvent::MouseMove { x, y });
        self.frame(g);
        self.input(
            g,
            InputEvent::MouseButton {
                left: true,
                down: true,
            },
        );
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

    fn key(&mut self, g: &mut Game, code: u16) {
        self.input(g, InputEvent::KeyDown(code));
        self.frame(g);
        self.input(g, InputEvent::KeyUp(code));
        self.frame(g);
    }

    fn shot(&mut self, g: &mut Game, name: &str) {
        for _ in 0..2 {
            g.render(&mut self.renderer, self.size).expect("render");
        }
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/buildui");
        std::fs::create_dir_all(&dir).expect("screenshot dir");
        g.canvas()
            .save_png(&dir.join(format!("{name}.png")))
            .expect("png");
    }
}

fn boxed(g: &Game) -> bool {
    g.letterbox.fully || g.nis.is_some()
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

/// The on-screen row of a ship type in the open manager.
fn row_of(g: &Game, t: ShipType) -> Option<usize> {
    let st = g.ui.state();
    let i = consmgr::rows(&st.view)
        .iter()
        .position(|r| matches!(r, Row::Ship(o) if o.ship_type == t.index() as u32))?;
    i.checked_sub(st.cm.top_row)
}

fn in_mission(size: (i32, i32), tag: &str) {
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    assert!(mbm::enabled(&g.ui.fe.game.options), "on by default");
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("the player's mothership");
    {
        let u = g.world.universe_mut();
        u.base.players[0].resource_units = 3_450;
        for (t, n) in [(ShipType::sAcolyte, 6), (ShipType::sRecon, 2)] {
            if hwc_sim::construction::queue_build(u, ms, t, n) == 0 {
                hwc_sim::construction::build_jobs_add(u, ms, t, n);
            }
        }
    }
    for _ in 0..16 * 6 {
        rig.frame(g);
    }
    // B opens the build manager, in the modern look.
    // One frame drawn: the interface takes the window's size.
    g.render(&mut rig.renderer, size).expect("render");
    rig.key(g, b'B' as u16);
    assert_eq!(g.ui.manager(), Some(Manager::Construction));
    assert!(mbm::active(&g.ui.fe));
    let l = mbm::layout(size.0, size.1);
    // Pick two workers (select, then add), and one more acolyte with + (straight into the
    // queue); the acolyte stays in the view.
    if let Some(w) = row_of(g, ShipType::sWorker) {
        let (_, y) = centre(l.row(w));
        rig.click(g, l.name_x + 10, y);
        let (px, py) = centre(l.plus(w));
        rig.click(g, px, py);
        rig.click(g, px, py);
    }
    let a = row_of(g, ShipType::sAcolyte).expect("acolytes are offered in mission 1");
    let (_, y) = centre(l.row(a));
    rig.click(g, l.name_x + 10, y);
    let (px, py) = centre(l.plus(a));
    rig.click(g, px, py);
    // Rest the pointer on the list, off any control.
    rig.input(
        g,
        InputEvent::MouseMove {
            x: l.name_x + 40,
            y: l.row(a).y0 + 2,
        },
    );
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, &format!("m1_build_manager_{tag}"));
    let st = g.ui.state();
    assert_eq!(st.cm.ship_view, Some(ShipType::sAcolyte.index() as u32));
    let img = st
        .cm
        .ship_view_image
        .as_ref()
        .expect("the ship view is rendered");
    let covered = img.pixels.iter().filter(|p| p[3] > 0).count();
    assert!(
        covered > img.pixels.len() / 50,
        "the acolyte fills some of the view: {covered} of {}",
        img.pixels.len()
    );
    // The modern HUD's panels keep clear of the manager.
    let view = st.view.clone();
    let arr = modern_panel::arrange(&g.ui.fe, size.0, size.1, &view);
    for r in arr.rects() {
        assert!(
            !modern_hud::overlaps(r, l.panel),
            "{r:?} over {:?} at {tag}",
            l.panel
        );
    }
    // Info: the description over the ship.
    let (ix, iy) = centre(l.buttons[&Button::Info]);
    rig.click(g, ix, iy);
    assert!(g.ui.state().cm.extended_info);
    rig.input(
        g,
        InputEvent::MouseMove {
            x: l.name_x + 40,
            y: l.row(a).y0 + 2,
        },
    );
    rig.frame(g);
    rig.shot(g, &format!("m1_build_manager_info_{tag}"));
    rig.click(g, ix, iy);
    // Build sends the workers; one Escape closes.
    let (bx, by) = centre(l.buttons[&Button::Build]);
    rig.click(g, bx, by);
    assert!(g.ui.state().cm.pending.values().all(|&n| n == 0));
    rig.input(
        g,
        InputEvent::MouseMove {
            x: size.0 / 2,
            y: size.1 / 2,
        },
    );
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, &format!("m1_build_manager_built_{tag}"));
    rig.key(g, 27);
    assert_eq!(g.ui.manager(), None, "one Escape closes the manager");
}

#[test]
fn modern_build_manager_in_mission_one_1080p() {
    in_mission((1920, 1080), "1920x1080");
}

#[test]
fn modern_build_manager_in_mission_one_768() {
    in_mission((1024, 768), "1024x768");
}

#[test]
fn modern_build_manager_in_mission_one_640() {
    in_mission((640, 480), "640x480");
}
