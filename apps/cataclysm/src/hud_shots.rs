//! Game-data screenshots of the modern RU / SU readout (`hwc_ui::ingame::modern_hud`) in
//! mission 1: at 1920x1080 and at 1024x768 (each with the original box for comparison), and
//! during the letterboxed fleet intel, where the overlay and so the readout are hidden.
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
    // Spend 400 RUs: the bar dips and "−400" floats up.
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
    while (header(g).is_none() || boxed(g)) && n < 16 * 600 {
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
