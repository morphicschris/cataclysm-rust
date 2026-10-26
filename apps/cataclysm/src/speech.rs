//! Screenshot test of the speech presentation in Mission 1: the translucent band with the
//! speaker portrait and subtitle, the letterbox coming in for a fleet intel event, the subtitle
//! over the letterbox bar, the location card, and skipping. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. PNGs go to `screenshots/speech/`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::InputEvent;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::subtitle::region;
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1024, 768);
const DT: f32 = 1.0 / 16.0;

fn save(canvas: &Canvas, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/speech");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    canvas
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

fn start() -> Option<(Box<Game>, Renderer, Option<crate::audio::Audio>)> {
    start_with(false)
}

/// Mission 1 without its opening NIS; `nislets` plays the NISlets the mission asks for.
fn start_with(nislets: bool) -> Option<(Box<Game>, Renderer, Option<crate::audio::Audio>)> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue) = crate::test_gpu::device()?;
    let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
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
    let mut loading = Loading::new(data, &session, 1, 2, SIZE).ok()?;
    loading.opening_nis = false;
    loading.loader.play_nislets = nislets;
    for _ in 0..20 {
        match loading.advance(&session, Some(&mut renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => return Some((g, renderer, audio)),
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    }
    panic!("loading never finished");
}

fn shot(g: &mut Game, r: &mut Renderer, name: &str) {
    g.render(r, SIZE).expect("render");
    save(g.canvas(), name);
}

fn letterbox_cards(g: &Game) -> usize {
    g.subtitles
        .as_ref()
        .map_or(0, |s| s.regions[region::LETTERBOX].cards.len())
}

#[test]
fn mission_one_speech_and_letterbox() {
    let Some((mut game, mut r, mut audio)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    assert!(g.subtitles.is_some(), "Subtitle.script did not load");
    {
        let s = g.subtitles.as_ref().unwrap();
        // Fleet Command's and the Tactical Officer's portraits load.
        assert!(s.picture(0).is_some() && s.picture(1).is_some());
        assert!(s.picture(9).is_none());
    }
    let mut band = false;
    let mut moving = false;
    let mut boxed = false;
    let mut location = false;
    let mut ended = 0;
    let mut frames = 0;
    // Up to 6 minutes of mission time.
    while frames < 16 * 360 && !(band && moving && boxed) {
        let out = g.update(DT, &mut audio);
        assert!(out.is_empty(), "unexpected outcome {out:?}");
        frames += 1;
        if g.world.run().game.intel_ended {
            ended += 1;
        }
        let cards = letterbox_cards(g);
        let lb = g.letterbox.clone();
        if cards > 0 && !lb.active() && !band {
            // A few frames in, past the fade.
            for _ in 0..8 {
                g.update(DT, &mut audio);
            }
            if letterbox_cards(g) > 0 {
                band = true;
                shot(g, &mut r, "01_speech_band");
                let px = g.canvas().get(SIZE.0 / 2, 30);
                eprintln!("band pixel {px:?}");
            }
        }
        if lb.fade > 0.3 && lb.fade < 0.7 && !moving {
            moving = true;
            shot(g, &mut r, "02_letterbox_moving");
        }
        if lb.fully && cards > 0 && !boxed {
            for _ in 0..8 {
                g.update(DT, &mut audio);
            }
            boxed = true;
            shot(g, &mut r, "03_letterbox_speech");
            let top = g.canvas().get(5, 5);
            assert_eq!(top, [0, 0, 0, 255], "letterbox bar is black");
        }
        let cards3 = g
            .subtitles
            .as_ref()
            .map_or(0, |s| s.regions[region::LOCATION_CARD].cards.len());
        if cards3 > 0 && !location {
            location = true;
            shot(g, &mut r, "04_location_card");
        }
    }
    eprintln!("frames {frames}: band {band} moving {moving} boxed {boxed} location {location} ended {ended}");
    assert!(band, "no subtitle outside a letterbox");
    assert!(moving && boxed, "no letterboxed fleet intel event");

    // Skipping: a simulated subtitle while the fleet intel lockout holds the keys.
    let now = g.world.universe().total_time_elapsed;
    if let Some(s) = g.subtitles.as_mut() {
        s.simulate(
            1,
            6000,
            b"Tactical Officer: skipping this event fades the text out.",
        );
        s.update(now);
    }
    g.ui.fleet_intel = true;
    g.world.run_mut().game.sensors_kas_flag = 1;
    g.input(InputEvent::KeyDown(0x1b), &mut audio);
    g.update(DT, &mut audio);
    g.input(InputEvent::KeyUp(0x1b), &mut audio);
    let aborted = g
        .subtitles
        .as_ref()
        .is_some_and(|s| s.regions[region::LETTERBOX].aborted);
    assert!(aborted, "Escape did not skip the fleet intel event");
}

/// The simulation's effects in Mission 1 (`screenshots/game-effects/`): the fleet arriving
/// through the hyperspace window effect, then the first fighting with the camera moved onto
/// it (muzzle flashes, bullets, hits, deaths).
#[test]
fn mission_one_effects() {
    let Some((mut game, mut r, mut audio)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    assert!(g.world.effects_on, "no ETG effects attached");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game-effects");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    let snap = |g: &mut Game, r: &mut Renderer, name: &str| {
        g.render(r, SIZE).expect("render");
        g.canvas()
            .save_png(&dir.join(format!("{name}.png")))
            .expect("png");
    };
    let mut hyper = false;
    let mut shots = 0;
    let mut frames = 0;
    let mut most = 0;
    while frames < 16 * 600 && shots < 3 {
        g.update(DT, &mut audio);
        frames += 1;
        let live = g
            .world
            .universe()
            .effects
            .as_ref()
            .map_or(0, |e| e.world.effects.len());
        most = most.max(live);
        let arriving = g
            .world
            .run()
            .game
            .arriving
            .iter()
            .any(|a| a.hs_state != hwc_sim::mission::hs_effect::INACTIVE);
        if arriving && !hyper && frames > 60 {
            hyper = true;
            snap(g, &mut r, "01_hyperspace_arrival");
        }
        let u = g.world.universe();
        let fight = u
            .bullet_list
            .first()
            .map(|&b| u.bullets.bullet(b).posinfo.position);
        if let Some(p) = fight {
            if frames % 32 == 0 && live > 0 {
                g.world.follow_focus = false;
                g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
                g.world.camera.distance = 1800.0;
                // Let the view settle on the fight for a moment.
                for _ in 0..3 {
                    g.update(DT, &mut audio);
                }
                shots += 1;
                snap(g, &mut r, &format!("{:02}_combat", shots + 1));
            }
        }
    }
    eprintln!(
        "frames {frames}: hyperspace shot {hyper}, combat shots {shots}, most live effects {most}"
    );
    assert!(most > 0, "no effects ever ran");
}

/// Mission 1's scripted opening with its NISlet and the fleet-intel event that shows the
/// proximity sensors in the sensors manager: afterwards the letterbox goes and the controls
/// come back. A timeline of the presentation state is printed.
#[test]
fn mission_one_intro_hands_over_control() {
    let Some((mut game, _r, mut audio)) = start_with(true) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let mut last = String::new();
    let mut sensors_seen = false;
    let mut free_at = None;
    for frame in 0..16 * 600 {
        let out = g.update(DT, &mut audio);
        for o in out {
            if let crate::game::Outcome::Animatic(_) = o {
                g.movie_finished();
            }
        }
        let run = g.world.run();
        let state = format!(
            "nis {:?} kas_sensors {} intel_flag {} wide {} lb_fully {} lb_fade {:.2} sensors_open {} ui_intel {} intel_ended {} sp_state {}",
            g.nis.as_ref().map(|n| n.kind),
            run.game.sensors_open,
            run.game.sensors_kas_flag,
            run.game.wide_screen,
            g.letterbox.fully,
            g.letterbox.fade,
            g.sensors.is_open(),
            g.ui.fleet_intel,
            run.game.intel_ended,
            run.game.sp_state,
        );
        if state != last {
            eprintln!(
                "intro {frame:5} ({:6.1}s): {state}",
                run.universe.total_time_elapsed
            );
            last = state;
        }
        sensors_seen |= run.game.sensors_open;
        let free = sensors_seen
            && g.nis.is_none()
            && !run.game.sensors_open
            && !g.letterbox.fully
            && !g.letterbox.active()
            && !g.ui.fleet_intel;
        if free {
            free_at = Some(frame);
            break;
        }
    }
    assert!(sensors_seen, "the intro never showed the sensors manager");
    // The first fleet-intel event ends (`IntelEventEnded`) and its `wideScreenOut` takes the
    // NISlet's bars away: well before the second event (about 350 s in).
    let t = free_at.map(|f| f as f32 * DT);
    assert!(
        t.is_some_and(|t| t < 330.0),
        "the letterbox stayed after the first fleet-intel event (free at {t:?})"
    );
}

/// The Kuun-Lan coming out of hyperspace in Mission 1: the blue window (`hsEnd`) over the
/// HyperWindow effect (`screenshots/game-effects/05_kuun_lan_arrival.png`).
#[test]
fn kuun_lan_arrival_window() {
    let Some((mut game, mut r, mut audio)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let mut seen = false;
    for _ in 0..16 * 60 {
        g.update(DT, &mut audio);
        let ms = g.world.universe().base.players[0].mothership;
        let window = g
            .world
            .run()
            .hyperspace_windows()
            .into_iter()
            .find(|&(id, state, clipt)| {
                use hwc_render::effects::hs_state::{SLICING_INTO, SLICING_OUTOF};
                Some(id) == ms
                    && matches!(state, SLICING_INTO | SLICING_OUTOF)
                    && clipt > 0.4
                    && clipt < 0.6
            });
        if let (Some((id, _, _)), Some(_)) = (window, ms) {
            let s = g.world.universe().ship(id).unwrap();
            let p = s.posinfo.position;
            let heading = s.rotinfo.coordsys.col(2);
            g.world.follow_focus = false;
            g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
            g.world.camera.distance = 5000.0;
            // From the side, a little ahead.
            let dir =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game-effects");
            std::fs::create_dir_all(&dir).expect("screenshot dir");
            for k in 0..4 {
                g.world.camera.angle =
                    heading.y.atan2(heading.x) + 0.6 + k as f32 * std::f32::consts::FRAC_PI_2;
                g.world.camera.declination = -0.3;
                g.render(&mut r, SIZE).expect("render");
                let name = if k == 0 {
                    "05_kuun_lan_arrival.png".to_string()
                } else {
                    format!("05_kuun_lan_arrival_{k}.png")
                };
                g.canvas().save_png(&dir.join(name)).expect("png");
            }
            seen = true;
            break;
        }
    }
    assert!(
        seen,
        "the Kuun-Lan never came half out of its hyperspace window"
    );
}
