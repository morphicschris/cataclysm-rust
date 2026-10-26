//! Game-data screenshots of the modern RU / SU readout (`hwc_ui::ingame::modern_hud`) in
//! mission 1: at 1920x1080 and at 1024x768 (each with the original box for comparison), and
//! during the letterboxed fleet intel, where the overlay and so the readout are hidden; the
//! research panel with research running (expanded, folded, a topic just finished) and the
//! full HUD with every list panel up.
//! Screenshots go to `screenshots/hud/`. Needs the game install (`HWC_GAME_DIR`) and a GPU
//! adapter; skips otherwise. Slow in a debug build: run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::sp_state;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::modern_hud::{self, OPTION_KEY};
use hwc_ui::ingame::{modern_objectives, InputEvent};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

fn save(canvas: &Canvas, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hud");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    canvas
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

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
    let audio = GameData::open(&dir)
        .map_err(|e| e.to_string())
        .and_then(|g| crate::audio::Audio::open(g, false))
        .ok();
    Some(Rig {
        data,
        session,
        renderer,
        audio,
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

    /// Renders and saves a frame; whether the readout was drawn in it.
    fn click(&mut self, g: &mut Game, x: i32, y: i32) {
        // Clear of the double-click time.
        for _ in 0..12 {
            self.frame(g);
        }
        g.input(InputEvent::MouseMove { x, y }, &mut self.audio);
        self.frame(g);
        let button = |down| InputEvent::MouseButton { left: true, down };
        g.input(button(true), &mut self.audio);
        self.frame(g);
        g.input(button(false), &mut self.audio);
        self.frame(g);
    }

    fn shot(&mut self, g: &mut Game, name: &str) -> bool {
        // The subtitles tell the overlay where the HUD top is as they draw, after the
        // overlay: one frame first, as the window would have drawn.
        g.render(&mut self.renderer, self.size).expect("render");
        let before = modern_hud::drawn_count();
        g.render(&mut self.renderer, self.size).expect("render");
        save(g.canvas(), name);
        modern_hud::drawn_count() > before
    }
}

/// The RU bar's left end shows the fill gradient's first colour.
fn ru_fill_on_canvas(c: &Canvas) -> bool {
    let l = modern_hud::layout(c.width, c.height);
    let p = c.get(l.ru.bar.x0 + 4, (l.ru.bar.y0 + l.ru.bar.y1) / 2);
    p[0] == 255 && (150..175).contains(&p[1]) && p[2] < 20
}

/// The overlay's hiding rule (`Game::render`): letterbox fully in, or an NIS playing.
fn boxed(g: &Game) -> bool {
    g.letterbox.fully || g.nis.is_some()
}

fn in_mission(size: (i32, i32), tag: &str) {
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    // A few seconds for the bars to settle.
    for _ in 0..48 {
        rig.frame(g);
    }
    assert!(
        g.ui.fe.game.options.get_bool(OPTION_KEY),
        "modern HUD is the default"
    );
    assert!(
        rig.shot(g, &format!("m1_modern_{tag}")),
        "readout not drawn at {tag}"
    );
    assert!(
        ru_fill_on_canvas(g.canvas()),
        "the RU bar is not drawn at {tag}"
    );
    // Spend 400 RUs: the bar eases down.
    g.world.universe_mut().base.players[0].resource_units -= 400;
    for _ in 0..5 {
        rig.frame(g);
    }
    assert!(rig.shot(g, &format!("m1_modern_spend_{tag}")));

    // The Tactical Officer's speech band is up now (mission 1 has no objectives yet, and
    // its later speech comes in the letterbox or the sensors manager): with an objective put
    // in for the picture, both panels sit below the band and portrait.
    assert!(
        g.ui.state().overlay.top > 0,
        "no speech portrait at the arrival"
    );
    g.world
        .mission_run_mut()
        .expect("mission")
        .game
        .objectives
        .push(hwc_sim::mission::Objective {
            label: "layout check".into(),
            brief: "Harvest the asteroid field".into(),
            full: String::new(),
            primary: true,
            complete: false,
        });
    rig.frame(g);
    assert!(rig.shot(g, &format!("m1_speech_band_{tag}")));
    let view = g.ui.state().view.clone();
    let p = modern_objectives::panel(&g.ui.fe, size.0, size.1, &view).expect("panel");
    assert!(
        p.header.y0 >= g.ui.state().overlay.top,
        "the panel is under the portrait"
    );
    g.world
        .mission_run_mut()
        .expect("mission")
        .game
        .objectives
        .pop();
    rig.frame(g);

    // The objectives panel: a click on its header collapses it, another expands it.
    let header = |g: &Game| {
        let view = g.ui.state().view.clone();
        modern_objectives::panel(&g.ui.fe, size.0, size.1, &view).map(|p| (p.header, p.expanded))
    };
    let mut n = 0;
    // The objective put in above stays listed (destroyed: greyed out); wait for the game's own.
    let none_live = |g: &Game| g.ui.state().view.objectives.is_empty();
    while (header(g).is_none() || none_live(g) || boxed(g)) && n < 16 * 600 {
        rig.frame(g);
        n += 1;
    }
    if let Some((h, expanded)) = header(g) {
        assert!(expanded, "the objectives panel starts expanded");
        for _ in 0..16 {
            rig.frame(g);
        }
        assert!(rig.shot(g, &format!("m1_objectives_{tag}")));
        let (x, y) = (h.x0 + h.height(), (h.y0 + h.y1) / 2);
        rig.click(g, x, y);
        assert_eq!(
            header(g).map(|p| p.1),
            Some(false),
            "the header click collapses"
        );
        // Off the header, so the shot shows it without the hover colour.
        g.input(
            InputEvent::MouseMove {
                x: size.0 / 2,
                y: size.1 / 2,
            },
            &mut rig.audio,
        );
        // Past the new objective's header flash.
        for _ in 0..48 {
            rig.frame(g);
        }
        assert!(rig.shot(g, &format!("m1_objectives_collapsed_{tag}")));
        rig.click(g, x, y);
        assert_eq!(header(g).map(|p| p.1), Some(true));
    } else {
        eprintln!("no objectives in mission 1 yet at {tag}");
    }

    g.ui.fe.game.options.set_bool(OPTION_KEY, false);
    rig.frame(g);
    assert!(
        !rig.shot(g, &format!("m1_original_{tag}")),
        "the modern readout drew with the original box at {tag}"
    );
}

#[test]
fn modern_hud_in_mission_one_1080p() {
    in_mission((1920, 1080), "1920x1080");
}

#[test]
fn modern_hud_in_mission_one_768() {
    in_mission((1024, 768), "1024x768");
}

/// Mission 1's letterboxed intro (the fleet intel): the readout is hidden with the rest of
/// the overlay, and comes back once the letterbox has gone.
#[test]
fn modern_hud_hidden_in_the_intro_letterbox() {
    let Some(mut rig) = rig((1920, 1080)) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    let mut frames = 0;
    while !boxed(g) {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "no letterbox in mission 1's intro");
    }
    for _ in 0..16 {
        rig.frame(g);
    }
    assert!(boxed(g), "the letterbox went before the shot");
    assert!(
        !rig.shot(g, "m1_intro_letterbox_hidden"),
        "the readout shows in the letterbox"
    );
}

/// Starts research for the player in the first three labs that have a technology to
/// research, some way along each; the topics' techs, in lab order.
fn start_research(g: &mut Game) -> Vec<usize> {
    use hwc_sim::base::{NUM_LABS, NUM_TECHS};
    use hwc_sim::research;
    let u = g.world.universe_mut();
    u.base.players[0].can_do_research = true;
    let mut started = Vec::new();
    for lab in 0..NUM_LABS {
        if started.len() == 3 {
            break;
        }
        let p = &u.base.players[0];
        let table = research::table_for(u, p.research_table);
        let Some(t) = (0..NUM_TECHS).find(|&t| {
            research::tech_lab(t) == lab as i32
                && p.tech.has[t] != 1
                && !p.topics.iter().any(|x| x.tech == t)
                && table.tech_time.get(t).copied().unwrap_or(0) > 0
        }) else {
            continue;
        };
        research::assign_lab(u, 0, lab, t);
        let done = [0.3, 0.65, 0.85][started.len()];
        if let Some(x) = u.base.players[0].topics.iter_mut().find(|x| x.tech == t) {
            x.time_left *= 1.0 - done;
        }
        started.push(t);
    }
    started
}

/// Queues acolytes and recons at the mothership (the build queue).
fn queue_ships(g: &mut Game) {
    use hwc_sim::statics::ShipType;
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("the player's mothership");
    let u = g.world.universe_mut();
    u.base.players[0].resource_units = 50_000;
    for (t, n) in [(ShipType::sAcolyte, 6), (ShipType::sRecon, 2)] {
        if hwc_sim::construction::queue_build(u, ms, t, n) == 0 {
            hwc_sim::construction::build_jobs_add(u, ms, t, n);
        }
    }
}

/// Mission 1 with research running in three labs and ships queued: the research panel
/// directly under the build queue, expanded and folded, then a topic finishing (its
/// "COMPLETE" row). Screenshots `m1_research_<size>.png`, `m1_research_collapsed_<size>.png`
/// and `m1_research_complete_<size>.png`.
fn research_panel(size: (i32, i32), tag: &str) {
    use hwc_ui::ingame::{modern_panel, modern_research};
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    queue_ships(g);
    let techs = start_research(g);
    assert!(techs.len() >= 2, "no research to start: {techs:?}");
    for _ in 0..48 {
        rig.frame(g);
    }
    let arrange = |g: &Game| {
        let view = g.ui.state().view.clone();
        modern_panel::arrange(&g.ui.fe, size.0, size.1, &view)
    };
    assert!(rig.shot(g, &format!("m1_research_{tag}")));
    let a = arrange(g);
    let r = a.research.clone().expect("no research panel");
    assert!(r.expanded, "the research panel starts expanded at {tag}");
    assert_eq!(r.rows.len() + r.hidden, techs.len());
    if let Some(b) = &a.build {
        assert_eq!(r.header.y0, b.below, "not under the build queue");
    }
    if let Some(l) = &a.log {
        assert!(r.panel.y1 <= l.panel.y0, "into the system log");
    }
    g.ui.fe
        .game
        .options
        .set_bool(modern_research::EXPANDED_KEY, false);
    rig.frame(g);
    assert!(rig.shot(g, &format!("m1_research_collapsed_{tag}")));
    let f = arrange(g).research.expect("folded research panel");
    assert!(!f.expanded && f.panel == f.header);
    g.ui.fe
        .game
        .options
        .set_bool(modern_research::EXPANDED_KEY, true);
    // The nearest topic finishes within the next research update.
    {
        let u = g.world.universe_mut();
        let p = &mut u.base.players[0];
        let t = p
            .topics
            .iter_mut()
            .find(|x| x.tech == techs[2.min(techs.len() - 1)]);
        t.expect("topic").time_left = 0.5;
    }
    let complete = |g: &Game| {
        arrange(g)
            .research_entries
            .iter()
            .any(|e| e.complete.is_some())
    };
    let mut n = 0;
    while !complete(g) && n < 16 * 4 {
        rig.frame(g);
        n += 1;
    }
    assert!(complete(g), "no COMPLETE row at {tag}");
    assert!(rig.shot(g, &format!("m1_research_complete_{tag}")));
}

#[test]
fn research_panel_in_mission_one_1080p() {
    research_panel((1920, 1080), "1920x1080");
}

#[test]
fn research_panel_in_mission_one_768() {
    research_panel((1024, 768), "1024x768");
}

/// Mission 1 with every modern panel up: acolytes and recons queued at the Kuun-Lan (the
/// build queue), research running (the research panel), the player's ships selected (the
/// selected-ships panel), a ship built (the system log); then the new panels folded from the
/// options. Screenshots
/// `m1_full_hud_<size>.png` and `m1_full_hud_collapsed_<size>.png`.
fn full_hud(size: (i32, i32), tag: &str) {
    use hwc_ui::ingame::{
        modern_build, modern_log, modern_panel, modern_research, modern_selected,
    };
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
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
    queue_ships(g);
    start_research(g);
    // Until a ship is built (its message) with more still queued.
    let built =
        |g: &Game| !g.ui.state().view.messages.is_empty() && !g.ui.state().view.jobs.is_empty();
    let mut n = 0;
    while !built(g) && n < 16 * 300 {
        rig.frame(g);
        n += 1;
    }
    let u = g.world.universe();
    g.selection = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| crate::host::selectable(u, id))
        .filter(|&id| {
            u.ship(id)
                .is_some_and(|s| s.player == 0 && Some(id) != Some(ms))
        })
        .collect();
    for _ in 0..8 {
        rig.frame(g);
    }
    assert!(rig.shot(g, &format!("m1_full_hud_{tag}")));
    let view = g.ui.state().view.clone();
    let a = modern_panel::arrange(&g.ui.fe, size.0, size.1, &view);
    assert!(a.build.is_some(), "no build queue at {tag}");
    assert!(a.research.is_some(), "no research panel at {tag}");
    assert!(a.log.is_some(), "no system log at {tag}");
    assert!(
        a.selected.is_some() || view.selection.is_empty(),
        "no selected-ships panel at {tag}"
    );
    let r = a.rects();
    for i in 0..r.len() {
        for j in i + 1..r.len() {
            assert!(
                !modern_hud::overlaps(r[i], r[j]),
                "{:?} / {:?} at {tag}",
                r[i],
                r[j]
            );
        }
    }
    for k in [
        modern_build::EXPANDED_KEY,
        modern_research::EXPANDED_KEY,
        modern_log::EXPANDED_KEY,
        modern_selected::EXPANDED_KEY,
    ] {
        g.ui.fe.game.options.set_bool(k, false);
    }
    rig.frame(g);
    assert!(rig.shot(g, &format!("m1_full_hud_collapsed_{tag}")));
    for k in [
        modern_build::EXPANDED_KEY,
        modern_research::EXPANDED_KEY,
        modern_log::EXPANDED_KEY,
        modern_selected::EXPANDED_KEY,
    ] {
        g.ui.fe.game.options.set_bool(k, true);
    }
}

#[test]
fn full_modern_hud_in_mission_one_1080p() {
    full_hud((1920, 1080), "1920x1080");
}

#[test]
fn full_modern_hud_in_mission_one_768() {
    full_hud((1024, 768), "1024x768");
}

#[test]
fn full_modern_hud_in_mission_one_640() {
    full_hud((640, 480), "640x480");
}
