//! Screenshots for the README: the modern main menu over its live scene, three cutscenes, the
//! start of two missions and a Mission 1 battle, each at 1280x720 with 4x MSAA and a 2x render
//! scale (supersampled; 2560x1440 x 4 samples stays inside the sample budget, so nothing is cut)
//! and the default (modern) look. Needs the game install (`HWC_GAME_DIR`) and a GPU adapter;
//! skips otherwise. Shots go to `screenshots/readme/` (or `HWC_README_SHOTS`):
//! `cargo test --release -p cataclysm readme_shots -- --ignored --nocapture`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::{RenderQuality, Renderer};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::menu_scene::{MenuSceneLoader, QualityOverrides};
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1280, 720);
const DT: f32 = 1.0 / 16.0;
const SCALE: f32 = 2.0;
const MSAA: u32 = 4;

fn shot_dir() -> PathBuf {
    let dir = std::env::var_os("HWC_README_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/readme"));
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    dir
}

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
}

fn rig() -> Option<Rig> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue, counts) = crate::test_gpu::device_with_msaa()?;
    let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
    renderer.set_msaa_support(&counts);
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
    let audio = GameData::open(&dir)
        .map_err(|e| e.to_string())
        .and_then(|g| crate::audio::Audio::open(g, false))
        .ok();
    Some(Rig { data, session, renderer, audio })
}

impl Rig {
    fn load(&mut self, mission: u32, nis: bool) -> Box<Game> {
        let mut l = Loading::new(self.data.clone(), &self.session, mission, 2, SIZE).expect("loading");
        l.opening_nis = nis;
        l.loader.play_nislets = false;
        self.finish(l)
    }

    fn finish(&mut self, mut l: Loading) -> Box<Game> {
        for _ in 0..40 {
            match l.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission {} did not start: {e}", l.mission),
            }
        }
        panic!("loading never finished");
    }

    fn frame(&mut self, g: &mut Game) -> Vec<Outcome> {
        let out = g.update(DT, &mut self.audio);
        for o in &out {
            if let Outcome::Animatic(_) = o {
                g.movie_finished();
            }
        }
        out
    }

    fn shot(&mut self, g: &mut Game, name: &str) {
        g.quality = RenderQuality { scale: SCALE, msaa: MSAA, ..g.quality };
        // The first frame at a new quality builds its targets.
        g.render(&mut self.renderer, SIZE).expect("render");
        g.render(&mut self.renderer, SIZE).expect("render");
        if let Some(p) = g.render_plan() {
            eprintln!("{name}: drawn at {}x{} x{}", p.render.0, p.render.1, p.samples);
        }
        g.canvas().save_png(&shot_dir().join(format!("{name}.png"))).expect("png");
    }

    /// Plays an opening NIS in its own level, taking a shot at each of `times` (seconds).
    fn nis_shots(&mut self, g: &mut Game, shots: &[(f32, &str)]) {
        let mut done = vec![false; shots.len()];
        assert!(g.world.nis_level().is_some(), "the mission did not start with its NIS level");
        let mut frames = 0;
        while g.nis.is_none() {
            self.frame(g);
            frames += 1;
            assert!(frames < 16 * 10, "the NIS never started");
        }
        while g.nis.as_ref().is_some_and(|n| n.kind.in_own_level()) && !done.iter().all(|d| *d) {
            self.frame(g);
            frames += 1;
            assert!(frames < 16 * 240, "the NIS never reached its last shot");
            let Some(time) = g.nis.as_ref().map(|n| n.player.time) else { break };
            for (i, (at, name)) in shots.iter().enumerate() {
                if !done[i] && time >= *at {
                    done[i] = true;
                    self.shot(g, name);
                }
            }
        }
    }
}

#[test]
#[ignore = "long: renders the README screenshots"]
fn readme_shots() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    eprintln!("GPU MSAA support {:?}", rig.renderer.msaa_support());

    // 1. The modern main menu over its live scene.
    {
        let mut loader = MenuSceneLoader::new(rig.data.clone(), None);
        let mut scene = loop {
            if let Some(sc) = loader.advance(&mut rig.renderer).expect("scene loads") {
                break sc;
            }
        };
        let over = QualityOverrides { scale: Some(SCALE), msaa: Some(MSAA), fxaa: None };
        scene.set_options(&rig.session.fe.game.options.clone(), over);
        let s = &mut rig.session;
        for _ in 0..(25.0 * 60.0) as usize {
            s.update(1.0 / 60.0);
            scene.update(1.0 / 60.0);
        }
        let (w, h) = s.fe.screen_size();
        s.mouse_move(f64::from(w - 8), f64::from(h - 8));
        for _ in 0..60 {
            s.update(1.0 / 60.0);
            scene.update(1.0 / 60.0);
            scene.render(&mut rig.renderer, s.canvas_mut()).expect("scene");
            s.render_over_backdrop();
        }
        s.canvas.save_png(&shot_dir().join("01_main_menu.png")).expect("png");
    }

    // 2. Cutscenes: Mission 1's opening, the hangar module ejected in Mission 4's, and the
    // nebula of Mission 6's.
    {
        let mut game = rig.load(1, true);
        rig.nis_shots(&mut game, &[(22.0, "02_cutscene_m1_opening"), (48.0, "03_cutscene_m1_kuun_lan")]);
    }
    {
        let mut game = rig.load(4, true);
        rig.nis_shots(&mut game, &[(63.0, "04_cutscene_m4_hangar_ejected")]);
    }
    {
        let mut game = rig.load(6, true);
        rig.nis_shots(&mut game, &[(53.0, "05_cutscene_m6_nebula")]);
    }

    // 3. In game: Mission 1 under way, then the autoplay fleet's first fight.
    {
        let mut game = rig.load(1, false);
        let g = &mut *game;
        let mut play = hwc_sim::autoplay::Autoplay::new(16 * 60 * 30);
        for _ in 0..16 * 40 {
            if g.nis.is_none() {
                play.tick(g.world.run_mut());
            }
            rig.frame(g);
        }
        rig.shot(g, "06_ingame_m1");

        // A battle beside the Kuun-Lan: Somtaaw Acolytes and multi-beam frigates against a
        // Turanic raider force, each side ordered to attack the other.
        use hwc_sim::statics::ShipType as T;
        let me = crate::host::PLAYER;
        let them = 1;
        let u = g.world.universe_mut();
        let p = u
            .ships
            .iter()
            .find(|s| s.player == me)
            .map_or(hwc_math::Vector::ZERO, |s| s.posinfo.position);
        let at = |x: f32, y: f32, z: f32| hwc_math::Vector::new(p.x + x, p.y + y, p.z + z);
        let mut ours = Vec::new();
        let mut theirs = Vec::new();
        for i in 0..8 {
            let f = i as f32;
            ours.extend(hwc_sim::base::create_ship(u, T::sAcolyte, me, at(3000.0, -600.0 + 150.0 * f, 0.0), Default::default()).ok());
            theirs.extend(hwc_sim::base::create_ship(u, T::p1Fighter, them, at(5200.0, -500.0 + 140.0 * f, 200.0), Default::default()).ok());
        }
        for i in 0..2 {
            let f = i as f32;
            ours.extend(hwc_sim::base::create_ship(u, T::sMultiBeamFrigate, me, at(2600.0, -400.0 + 800.0 * f, -100.0), Default::default()).ok());
        }
        for i in 0..3 {
            let f = i as f32;
            theirs.extend(hwc_sim::base::create_ship(u, T::p1StandardCorvette, them, at(5600.0, -500.0 + 500.0 * f, 0.0), Default::default()).ok());
        }
        theirs.extend(hwc_sim::base::create_ship(u, T::p1IonArrayFrigate, them, at(6000.0, 0.0, 0.0), Default::default()).ok());
        eprintln!("battle: {} ours, {} theirs", ours.len(), theirs.len());
        rig.frame(g);
        let u = g.world.universe_mut();
        hwc_sim::command::cl_attack(u, &ours, &theirs);
        hwc_sim::command::cl_attack(u, &theirs, &ours);
        g.world.follow_focus = false;
        g.world.camera.look_at = glam::Vec3::new(p.x + 4200.0, p.y, p.z);
        g.world.camera.distance = 3200.0;
        g.world.camera.declination = (-20f32).to_radians();
        for (wait, name) in [(16 * 6, "07_battle"), (16 * 4, "08_battle_later")] {
            let mut best = 0;
            for _ in 0..wait {
                rig.frame(g);
                best = best.max(g.world.universe().effects.as_ref().map_or(0, |e| e.world.effects.len()));
            }
            // Follow the fight's middle.
            let u = g.world.universe();
            let alive: Vec<_> = ours.iter().chain(&theirs).filter_map(|&id| u.ship(id)).map(|s| s.posinfo.position).collect();
            if !alive.is_empty() {
                let n = alive.len() as f32;
                let c = alive.iter().fold(glam::Vec3::ZERO, |a, v| a + glam::Vec3::new(v.x, v.y, v.z)) / n;
                g.world.camera.look_at = c;
            }
            eprintln!("{name}: {} ships left, up to {best} effects", alive.len());
            rig.shot(g, name);
        }
    }
}
