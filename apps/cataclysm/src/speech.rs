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
    canvas.save_png(&dir.join(format!("{name}.png"))).expect("png");
}

fn start() -> Option<(Box<Game>, Renderer, Option<crate::audio::Audio>)> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue) = hwc_render::headless_device().ok()?;
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
    loading.loader.play_nislets = false;
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
        if g.world.run.game.intel_ended {
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
        s.simulate(1, 6000, b"Tactical Officer: skipping this event fades the text out.");
        s.update(now);
    }
    g.ui.fleet_intel = true;
    g.world.run.game.sensors_kas_flag = 1;
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
        g.canvas().save_png(&dir.join(format!("{name}.png"))).expect("png");
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
            .run
            .game
            .arriving
            .iter()
            .any(|a| a.hs_state != hwc_sim::mission::hs_effect::INACTIVE);
        if arriving && !hyper && frames > 60 {
            hyper = true;
            snap(g, &mut r, "01_hyperspace_arrival");
        }
        let u = g.world.universe();
        let fight = u.bullet_list.first().map(|&b| u.bullets.bullet(b).posinfo.position);
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
    eprintln!("frames {frames}: hyperspace shot {hyper}, combat shots {shots}, most live effects {most}");
    assert!(most > 0, "no effects ever ran");
}
