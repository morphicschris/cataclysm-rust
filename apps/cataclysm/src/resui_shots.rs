//! Game-data screenshots of the modern research manager
//! (`hwc_ui::ingame::modern_research_manager`, `--research-ui modern`, the default) in
//! mission 1: three research modules on the Kuun-Lan, topics offered (the mission's own, and
//! a few more allowed the way `TechSetResearch` allows them), one already known, one started
//! from the manager's Research button and running, at 1920x1080, 1024x768 and 640x480 (and
//! with Info on, and a lab's module picture). Screenshots go to `screenshots/resui/`. Needs the
//! game install (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Slow in a debug build:
//! run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::sp_state;
use hwc_sim::research;
use hwc_sim::statics::{ShipType, Technology};
use hwc_ui::canvas::Rect;
use hwc_ui::ingame::host::{LabStatus, TechState};
use hwc_ui::ingame::modern_research_manager::{self as mrm, Button};
use hwc_ui::ingame::research::Picture;
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
        for down in [true, false] {
            self.input(g, InputEvent::MouseButton { left: true, down });
            self.frame(g);
        }
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
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/resui");
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

/// The on-screen row of a technology in the open manager's list.
fn row_of(g: &Game, tech: usize) -> Option<usize> {
    let st = g.ui.state();
    let rows = hwc_ui::ingame::research::visible_rows(&st.view);
    let i = rows.iter().position(|r| r.2 && r.1 == tech as u32)?;
    let a = g.ui.fe.find_atom("RM_TechListWindow")?;
    i.checked_sub(g.ui.fe.list(a)?.upper)
}

/// The first technologies of a lab, in technology order.
fn lab_techs(lab: i32, n: usize) -> Vec<usize> {
    Technology::ALL
        .iter()
        .map(|t| t.index())
        .filter(|&i| research::tech_lab(i) == lab)
        .take(n)
        .collect()
}

fn in_mission(size: (i32, i32), tag: &str) {
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    assert!(mrm::enabled(&g.ui.fe.game.options), "on by default");
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    // Research modules on the Kuun-Lan (hangar, armour, weapons), a few topics allowed as
    // the missions' TechSetResearch allows them, one of them already known.
    let hangar = lab_techs(0, 3);
    let armour = lab_techs(1, 2);
    let weapons = lab_techs(4, 3);
    {
        let u = g.world.universe_mut();
        for t in [
            ShipType::sMothershipDockingBay,
            ShipType::sMothershipArmour,
            ShipType::sMothershipWeapons,
        ] {
            research::activate_lab(u, 0, t, false);
        }
        for &t in hangar.iter().chain(&armour).chain(&weapons) {
            research::allow_research(u, 0, t);
        }
        u.base.players[0].tech.add(hangar[0]);
    }
    for _ in 0..8 {
        rig.frame(g);
    }
    // One frame drawn: the interface takes the window's size. R opens the manager.
    g.render(&mut rig.renderer, size).expect("render");
    rig.key(g, b'R' as u16);
    assert_eq!(g.ui.manager(), Some(Manager::Research));
    assert!(mrm::active(&g.ui.fe));
    let l = mrm::layout(size.0, size.1, g.ui.state().view.labs.len());
    // The gating: the allowed topics are offered, the known one is listed as researched.
    let view = g.ui.state().view.clone();
    let state = |t: usize| view.tech(t as u32).map(|t| t.state);
    assert_eq!(state(hangar[0]), Some(TechState::Researched));
    for &t in hangar[1..].iter().chain(&armour).chain(&weapons) {
        assert_eq!(state(t), Some(TechState::Available), "topic {t}");
    }
    let rest = |x: i32, y: i32| InputEvent::MouseMove { x, y };
    // Pick a weapons topic: its picture and lab show.
    let w = row_of(g, weapons[0]).expect("the weapons topic is listed");
    let (_, y) = centre(l.row(w));
    rig.click(g, l.name_x + 20, y);
    assert_eq!(g.ui.state().rm.picture, Picture::Tech(weapons[0] as u32));
    rig.input(g, rest(l.name_x + 60, l.row(w).y0 + 3));
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, &format!("m1_research_manager_{tag}"));
    // Research it: the lab starts.
    let (bx, by) = centre(l.buttons[&Button::Research]);
    rig.click(g, bx, by);
    assert!(
        research::researching(g.world.universe(), 0, weapons[0]),
        "the research button started the topic"
    );
    // A second topic by double click, in the hangar lab.
    let h = row_of(g, hangar[1]).expect("the hangar topic is listed");
    let (_, hy) = centre(l.row(h));
    for _ in 0..10 {
        rig.frame(g);
    }
    rig.input(g, rest(l.name_x + 20, hy));
    rig.frame(g);
    for _ in 0..2 {
        for down in [true, false] {
            rig.input(g, InputEvent::MouseButton { left: true, down });
            rig.frame(g);
        }
    }
    assert!(
        research::researching(g.world.universe(), 0, hangar[1]),
        "a double click researches"
    );
    // Let them run a while.
    for _ in 0..16 * 8 {
        rig.frame(g);
    }
    let w = row_of(g, weapons[0]).unwrap();
    let (_, y) = centre(l.row(w));
    rig.click(g, l.name_x + 20, y);
    rig.input(g, rest(l.name_x + 60, l.row(w).y0 + 3));
    for _ in 0..4 {
        rig.frame(g);
    }
    let st = g.ui.state();
    let lab = &st.view.labs[4];
    assert_eq!(lab.status, LabStatus::Researching);
    assert!(lab.progress > 0.0, "the weapons lab is making progress");
    rig.shot(g, &format!("m1_research_manager_researching_{tag}"));
    // The modern HUD's panels keep clear of the manager.
    let view = g.ui.state().view.clone();
    let arr = modern_panel::arrange(&g.ui.fe, size.0, size.1, &view);
    for r in arr.rects() {
        assert!(
            !modern_hud::overlaps(r, l.panel),
            "{r:?} over {:?} at {tag}",
            l.panel
        );
    }
    // Info: the description over the picture.
    let (ix, iy) = centre(l.buttons[&Button::Info]);
    rig.click(g, ix, iy);
    assert!(g.ui.state().rm.extended_info);
    rig.input(g, rest(l.name_x + 60, l.row(w).y0 + 3));
    rig.frame(g);
    rig.shot(g, &format!("m1_research_manager_info_{tag}"));
    rig.click(g, ix, iy);
    // A lab tab: the module's picture.
    let (tx, ty) = centre(l.labs[0]);
    rig.click(g, tx, ty);
    assert_eq!(g.ui.state().rm.picture, Picture::Module(0));
    rig.input(g, rest(size.0 / 2, l.panel.y1 - 4));
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, &format!("m1_research_manager_lab_{tag}"));
    // Build swaps to the modern build manager and back; one Escape closes.
    let (x, y) = centre(l.buttons[&Button::Build]);
    rig.click(g, x, y);
    assert_eq!(g.ui.manager(), Some(Manager::Construction));
    assert!(hwc_ui::ingame::modern_build_manager::active(&g.ui.fe));
    rig.key(g, b'R' as u16);
    assert_eq!(g.ui.manager(), Some(Manager::Research));
    rig.key(g, 27);
    assert_eq!(g.ui.manager(), None, "one Escape closes the manager");
}

#[test]
fn modern_research_manager_in_mission_one_1080p() {
    in_mission((1920, 1080), "1920x1080");
}

#[test]
fn modern_research_manager_in_mission_one_768() {
    in_mission((1024, 768), "1024x768");
}

#[test]
fn modern_research_manager_in_mission_one_640() {
    in_mission((640, 480), "640x480");
}
