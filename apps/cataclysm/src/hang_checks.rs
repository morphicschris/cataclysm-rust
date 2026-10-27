//! Game-data checks for a reported hang: the sensors manager toggled on and off over and over
//! as Mission 1's cutscenes end (the opening NIS level and its hand-over to the mission, the
//! NISlets, the fleet-intel letterboxes, the scenes where the script opens or closes the
//! manager). The frames run as the window runs them (update, then render), under the
//! main-loop watchdog: a frame that never ends prints the watchdog's report (with the stuck
//! thread's stack) and aborts the test run instead of hanging it. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Slow in a debug build: run with
//! `--release`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use hwc_formats::GameData;
use hwc_kas::kas_hash;
use hwc_render::Renderer;
use hwc_ui::ingame::InputEvent;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};

const SPACE: u16 = 0x20;

/// The messages that start Mission 1's later fleet-intel scenes (the battle sends them).
const SCENE_MESSAGES: [u32; 4] = [0x0f92dd92, 0x0f19cfe7, 0x04d89963, 0x0939002e];

/// A small deterministic generator.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }
}

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
    size: (i32, i32),
    dt: f32,
}

/// The options of a played install, when one is given (`HWC_HANG_OPTIONS`: a directory with
/// `hwc-options.cfg` and `keys.cfg`), copied so the run cannot change them; the defaults
/// otherwise.
fn user_options() -> (Option<PathBuf>, Option<String>, Option<PathBuf>) {
    let Some(dir) = std::env::var_os("HWC_HANG_OPTIONS").map(PathBuf::from) else {
        return (None, None, None);
    };
    let to = std::env::temp_dir().join(format!("hwc-hang-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&to);
    let copy = |name: &str| {
        let dst = to.join(name);
        std::fs::copy(dir.join(name), &dst).ok().map(|_| dst)
    };
    let options = copy("hwc-options.cfg");
    let keys = copy("keys.cfg");
    let text = keys.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    (options, text, keys)
}

impl Rig {
    fn new(size: (i32, i32), dt: f32) -> Option<Rig> {
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
        let (options_path, keys, keys_path) = user_options();
        let mut session = Session::new(
            source,
            Setup {
                options_path,
                original_cfg: read("Cataclysm.cfg"),
                default_keys: read("default_keys.cfg"),
                keys,
                keys_path,
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
            dt,
        })
    }

    fn load(&mut self, mut loading: Loading) -> Box<Game> {
        for _ in 0..4000 {
            let _busy = crate::watchdog::enter("loading");
            match loading.advance(&self.session, Some(&mut self.renderer), self.dt) {
                Progress::Working => {}
                Progress::Ready(mut g) => {
                    // As the window does with the options in force.
                    let options = g.ui.fe.game.options.clone();
                    g.apply_modern_options(&options);
                    return g;
                }
                Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
            }
        }
        panic!("loading never finished");
    }

    /// Mission 1 from its start: the opening NIS level first.
    fn mission_one(&mut self) -> Box<Game> {
        let mut l = Loading::new(self.data.clone(), &self.session, 1, 2, self.size)
            .expect("mission 1's loading");
        l.opening_nis = true;
        l.loader.play_nislets = true;
        self.load(l)
    }
}

/// What happened over a run, to show the sequences were reached.
#[derive(Default, Debug)]
struct Seen {
    presses: usize,
    presses_near_ends: usize,
    nis_ends: usize,
    nis_level_ends: usize,
    letterbox_ins: usize,
    letterbox_outs: usize,
    /// The mission's `sensors_open` going on and off (the player's toggles and the script's).
    opens: usize,
    closes: usize,
    intel_messages: usize,
}

/// The sensors key as a player works it: held for a few frames (auto-repeating when held
/// long), then let go.
#[derive(Default)]
struct Key {
    held: Option<u32>,
    hold_for: u32,
}

impl Key {
    fn frame(
        &mut self,
        g: &mut Game,
        audio: &mut Option<crate::audio::Audio>,
        press: bool,
        rng: &mut Lcg,
    ) -> bool {
        match self.held {
            Some(n) => {
                if n >= self.hold_for {
                    g.input(InputEvent::KeyUp(SPACE), audio);
                    self.held = None;
                } else {
                    // The system's auto-repeat after a quarter second, every other frame.
                    if n >= 15 && n % 2 == 0 {
                        g.input(InputEvent::KeyDown(SPACE), audio);
                    }
                    self.held = Some(n + 1);
                }
                false
            }
            None if press => {
                g.input(InputEvent::KeyDown(SPACE), audio);
                self.hold_for = match rng.next() % 8 {
                    0 => 0,
                    7 => 20,
                    k => k,
                };
                if self.hold_for == 0 {
                    g.input(InputEvent::KeyUp(SPACE), audio);
                } else {
                    self.held = Some(1);
                }
                true
            }
            None => false,
        }
    }
}

/// Plays Mission 1 from the opening NIS with the sensors key pressed in bursts: every frame
/// or two for a while around each end of a cutscene, a letterbox or a scripted open or close
/// (the frames before are known only after: the bursts come again just after each), and at a
/// random rhythm otherwise. The later fleet-intel scenes are started by their messages once
/// the script waits for them. Each frame renders as in the window. `seconds` is the length of
/// the mission part in game time.
fn toggle_through_mission_one(seed: u64, dt: f32, size: (i32, i32), seconds: f32) -> Option<Seen> {
    let mut rig = Rig::new(size, dt)?;
    crate::watchdog::install(|report| {
        eprintln!("{report}");
        eprintln!("hang_checks: a frame never ended; aborting");
        std::process::abort();
    });
    let mut rng = Lcg(seed);
    let mut seen = Seen::default();
    let mut game = rig.mission_one();
    let mut key = Key::default();
    let mut burst = 0u32;
    let mut next_random = 8;
    let mut frame = 0usize;
    let mut was_nis = game.nis.is_some();
    let mut was_kas_open = false;
    let mut wide_seen = 0;
    let mut idle = 0;
    let mut slowest = Duration::ZERO;
    let fps = (1.0 / dt).round() as u32;
    loop {
        frame += 1;
        crate::watchdog::frame(frame as u64);
        let busy = crate::watchdog::enter("test frame");
        let started = Instant::now();
        // The key: in a burst on most frames, else now and then.
        let press = if burst > 0 {
            burst -= 1;
            !rng.next().is_multiple_of(3)
        } else {
            frame >= next_random
        };
        if key.frame(&mut game, &mut rig.audio, press, &mut rng) {
            seen.presses += 1;
            if burst > 0 {
                seen.presses_near_ends += 1;
            } else {
                next_random = frame + 1 + (rng.next() % (fps * 2)) as usize;
            }
        }
        let out = game.update(rig.dt, &mut rig.audio);
        let mut level_done = false;
        for o in &out {
            match o {
                Outcome::Animatic(_) => game.movie_finished(),
                Outcome::NisLevelDone => level_done = true,
                Outcome::NextMission | Outcome::Leave => {
                    drop(busy);
                    eprintln!("hang_checks: the mission ended: {out:?}");
                    return Some(seen);
                }
                _ => {}
            }
        }
        if level_done {
            // `after_nis_level`: the mission's level loads with the NIS level's fleet; the
            // key presses in between go nowhere (the window ignores them while loading).
            seen.nis_level_ends += 1;
            let skill = game.world.skill();
            let crate::world::Sim::NisLevel(nis) = game.world.into_sim() else {
                panic!("not an NIS level");
            };
            let mut l = Loading::after_nis_level(rig.data.clone(), &rig.session, *nis, skill, size)
                .expect("the mission after the NIS level");
            l.loader.play_nislets = true;
            game = rig.load(l);
            key = Key::default();
            burst = 3 * fps / 2;
            was_nis = game.nis.is_some();
            was_kas_open = false;
            wide_seen = game.wide_screen_log.len();
            drop(busy);
            continue;
        }
        game.render(&mut rig.renderer, size).expect("render");
        drop(busy);
        slowest = slowest.max(started.elapsed());
        // The ends just passed: a burst around them.
        let nis = game.nis.is_some();
        if was_nis && !nis {
            seen.nis_ends += 1;
            burst = 3 * fps / 2;
        }
        if nis && !was_nis {
            burst = fps;
        }
        was_nis = nis;
        let kas_open = game
            .world
            .mission_run()
            .is_some_and(|r| r.game.sensors_open);
        if kas_open != was_kas_open {
            if kas_open {
                seen.opens += 1;
            } else {
                seen.closes += 1;
            }
            burst = burst.max(fps);
        }
        was_kas_open = kas_open;
        for (e, _) in &game.wide_screen_log[wide_seen..] {
            if e.kind == hwc_sim::mission::WideScreenKind::Out {
                seen.letterbox_outs += 1;
            } else {
                seen.letterbox_ins += 1;
            }
            burst = 3 * fps / 2;
        }
        wide_seen = game.wide_screen_log.len();
        if game.world.nis_level().is_some() {
            assert!(frame < (fps * 600) as usize, "the opening NIS never ended");
            continue;
        }
        // The later fleet-intel scenes, once the script waits for one.
        let run = game.world.run();
        let now = run.universe.total_time_elapsed;
        let waiting = run.team_state("FleetIntel") == Some(kas_hash("WAITMessage"))
            && !game.letterbox.active();
        idle = if waiting { idle + 1 } else { 0 };
        if now > 250.0 && idle > fps * 3 {
            if let Some(&msg) = SCENE_MESSAGES.get(seen.intel_messages) {
                game.world
                    .run_mut()
                    .kas
                    .post_message(kas_hash("FleetIntel"), None, msg);
                seen.intel_messages += 1;
                idle = 0;
            }
        }
        if now > seconds {
            break;
        }
    }
    eprintln!("hang_checks: seed {seed}, dt {dt:.4}: {seen:?}, slowest frame {slowest:?}");
    Some(seen)
}

/// The reported hang: the sensors manager toggled as cutscenes and briefings end, through
/// the opening NIS, its hand-over to the mission, the NISlets and the fleet-intel scenes,
/// at the window's frame rates. Release.
#[test]
fn sensors_toggled_as_cutscenes_end() {
    let runs: [(u64, f32); 2] = [(1, 1.0 / 60.0), (2, 1.0 / 16.0)];
    let seconds = std::env::var("HWC_HANG_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(420.0);
    for (seed, dt) in runs {
        let Some(seen) = toggle_through_mission_one(seed, dt, (800, 600), seconds) else {
            eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
            return;
        };
        assert_eq!(seen.nis_level_ends, 1, "{seen:?}");
        assert!(seen.nis_ends >= 1, "no NISlet ended: {seen:?}");
        assert!(seen.letterbox_outs >= 1, "no letterbox went: {seen:?}");
        assert!(seen.presses_near_ends > 50, "{seen:?}");
    }
}

/// The sensors view drawn with one of the player's ships right beside the camera's plane, as
/// the camera passes ships and the explored-area grid while it zooms (or comes back from a
/// cutscene where it was left): the ship's centre projects billions of pixels off the screen
/// (`i32::MAX` once cut to whole pixels). Its class icon's outline was walked pixel by pixel
/// from there, and past `2^30` pixels the walk's error terms overflowed and it never reached
/// its end: the game stopped responding. The frame now draws at once. Release.
#[test]
fn sensors_view_with_a_ship_beside_the_camera_plane() {
    use crate::sensors::Phase;
    let size = (2560, 1440);
    let Some((mut game, mut rig)) = crate::m1_checks::mission_one_with(size, false, true) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    crate::watchdog::install(|report| {
        eprintln!("{report}");
        eprintln!("hang_checks: a frame never ended; aborting");
        std::process::abort();
    });
    let g = &mut *game;
    // The fleet comes out of hyperspace a little after the start.
    let own = |g: &Game| {
        let u = g.world.universe();
        u.ship_list
            .iter()
            .any(|&id| u.ship(id).is_some_and(|s| s.player == crate::host::PLAYER))
    };
    for _ in 0..16 * 120 {
        if own(g) {
            break;
        }
        rig.frame(g);
    }
    // Everything the sensors view can draw (the played options show the explored areas).
    let t = &mut g.sensors.toggles;
    (
        t.areas_visited,
        t.areas_occupied,
        t.universe_extents,
        t.slipgate_paths,
    ) = (true, true, true, true);
    (t.objective_pointers, t.compass, t.icons, t.pieplate_lines) = (true, true, true, true);
    g.toggle_sensors(&mut rig.audio);
    for _ in 0..16 * 10 {
        if g.sensors.phase() == Phase::Sensors && !g.sensors.zooming() {
            break;
        }
        rig.frame(g);
    }
    assert!(g.sensors.phase() == Phase::Sensors && !g.sensors.zooming());
    let u = g.world.universe();
    let ship = u
        .ship_list
        .iter()
        .filter_map(|&id| u.ship(id))
        .find(|s| s.player == crate::host::PLAYER && crate::sensors::icon_index(u, s.id).is_some())
        .map(|s| {
            let p = s.coll.coll_position;
            glam::Vec3::new(p.x, p.y, p.z)
        })
        .expect("a ship of the player's");
    let forward = glam::Vec3::X;
    let side = glam::Vec3::Y;
    // Where exactly the centre lands depends on the f32 rounding of the camera's matrices, so
    // a range of places just in front of the camera's plane, on either side.
    let mut slowest = Duration::ZERO;
    for k in 0..72 {
        let depth = 0.001 + 0.0005 * (k % 12) as f32;
        let lateral = [3000.0, -6000.0, 12000.0, -25000.0, 0.0, 30.0][k / 12];
        let eye = ship - forward * depth - side * lateral;
        let mut cam = g.world.camera;
        crate::sensors::place(&mut cam, eye, eye + forward * 3000.0);
        g.world.camera = cam;
        let started = Instant::now();
        let busy = crate::watchdog::enter("test sensors frame");
        rig.render(g);
        drop(busy);
        let took = started.elapsed();
        slowest = slowest.max(took);
        assert!(
            took < Duration::from_secs(1),
            "depth {depth} lateral {lateral}: the frame took {took:?}"
        );
    }
    eprintln!("slowest sensors frame {slowest:?}");
}

/// The sensors manager opened (still zooming out) as Mission 1's opening NISlet starts: the
/// NIS closes it at once, as the original's `spMainScreen` does, and the cutscene ends on the
/// main camera with the manager shut. A close that waited for the zoom (refused while one was
/// going) kept the manager open through the cutscene, and the camera the NIS saved and gave
/// back at its end was the sensors camera in mid-zoom, left wherever it was.
#[test]
fn an_nis_closes_the_sensors_manager_mid_zoom() {
    let size = (800, 600);
    let Some((mut game, mut rig)) = crate::m1_checks::mission_one_with(size, true, true) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let mut main_camera = None;
    let mut started = false;
    for _ in 0..16 * 120 {
        if g.nis.is_none() && !g.sensors.is_open() {
            main_camera = Some(g.world.camera);
            g.toggle_sensors(&mut rig.audio);
        }
        rig.frame(g);
        if g.nis.is_some() {
            started = true;
            break;
        }
    }
    assert!(started, "the NISlet never started");
    assert!(
        !g.sensors.is_open(),
        "the NIS left the sensors manager open"
    );
    let kas_open = g.world.run().game.sensors_open;
    assert!(
        !kas_open,
        "the mission still thinks the sensors manager is open"
    );
    let main = main_camera.expect("opened");
    assert_eq!(
        (main.distance, main.look_at),
        (g.world.camera.distance, g.world.camera.look_at),
        "the NIS did not start from the main camera"
    );
    let mut frames = 0;
    while g.nis.is_some() {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "the NISlet never ended");
    }
    assert!(!g.sensors.is_open());
    assert_eq!(g.sensors.phase(), crate::sensors::Phase::Main);
    rig.render(g);
}
