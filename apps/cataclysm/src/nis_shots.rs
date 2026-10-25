//! Screenshot test of the NIS player on the game path: New Game -> the opening NIS
//! (`nis\n01.nis` in `Mission01_NIS01`) -> mission 1, whose first state plays NISlet 1, then
//! the fleet arrives. Frames at several times of both go to `screenshots/nis/`. A second test
//! skips the opening with Escape. Needs the game install (`HWC_GAME_DIR`) and a GPU adapter;
//! skips otherwise.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_ui::ingame::InputEvent;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::nis::Kind;
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1024, 768);
const DT: f32 = 1.0 / 16.0;

fn save(g: &Game, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/nis");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    g.canvas().save_png(&dir.join(format!("{name}.png"))).expect("png");
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
    loading.opening_nis = true;
    for _ in 0..20 {
        match loading.advance(&session, Some(&mut renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => return Some((g, renderer, audio)),
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    }
    panic!("loading never finished");
}

fn kind(g: &Game) -> Option<Kind> {
    g.nis.as_ref().map(|n| n.kind)
}

fn nis_time(g: &Game) -> f32 {
    g.nis.as_ref().map_or(-1.0, |n| n.player.time)
}

/// Updates until the NIS time reaches `t` (or the NIS ends), then draws and saves.
fn shot_at(g: &mut Game, r: &mut Renderer, audio: &mut Option<crate::audio::Audio>, want: Kind, t: f32, name: &str) -> bool {
    let mut guard = 0;
    while kind(g) == Some(want) && nis_time(g) < t {
        g.update(DT, audio);
        guard += 1;
        assert!(guard < 16 * 200, "NIS stuck at {}", nis_time(g));
    }
    if kind(g) != Some(want) {
        return false;
    }
    g.render(r, SIZE).expect("render");
    save(g, name);
    true
}

/// The average brightness of the picture between the letterbox bars.
fn brightness(g: &Game) -> f32 {
    let c = g.canvas();
    let (w, h) = (c.width, c.height);
    let mut sum = 0u64;
    let mut n = 0u64;
    for y in (h / 6..h * 5 / 6).step_by(4) {
        for x in (0..w).step_by(4) {
            let p = c.get(x, y);
            sum += u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]);
            n += 3;
        }
    }
    sum as f32 / n.max(1) as f32
}

#[test]
fn opening_nis_then_nislet_then_mission() {
    let Some((mut game, mut r, mut audio)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    g.update(DT, &mut audio);
    assert_eq!(kind(g), Some(Kind::Opening), "the opening NIS did not start");
    {
        let n = g.nis.as_ref().unwrap();
        assert_eq!(n.player.data.objects.len(), 46);
        assert!((n.player.length() - 110.633).abs() < 0.01);
        // The mission waits behind it, its script not started.
        let pending = n.pending.as_ref().expect("mission world");
        assert_eq!(pending.run.game.sp_state, hwc_sim::mission::sp_state::START_KAS);
    }
    // Faded from black at the start (fadeSet 100%, fadeTo 0% over 60 frames).
    g.render(&mut r, SIZE).expect("render");
    save(g, "n01_00_start");
    let dark = brightness(g);
    let times: [(f32, &str); 10] = [
        (2.5, "n01_01_faded_in"),
        (7.0, "n01_01b_speech"),
        (11.0, "n01_02_location_card"),
        (20.0, "n01_03_mothership"),
        (26.0, "n01_04_fov34"),
        (33.0, "n01_05_recons"),
        (53.0, "n01_06_speech_subtitle"),
        (75.0, "n01_07"),
        (95.0, "n01_08"),
        (109.0, "n01_09_fading_out"),
    ];
    let mut bright = 0.0f32;
    for (t, name) in times {
        assert!(shot_at(g, &mut r, &mut audio, Kind::Opening, t, name), "opening ended before {t}s");
        let b = brightness(g);
        eprintln!("{name}: brightness {b:.1}, camera {:?}", g.nis.as_ref().map(|n| n.player.camera.eye));
        if t == 2.5 {
            bright = b;
        }
        if t == 26.0 {
            assert_eq!(g.nis.as_ref().unwrap().player.camera.fov, 34.0);
        }
    }
    assert!(bright > dark, "no fade in: {dark} -> {bright}");
    assert!(g.letterbox.fully, "the letterbox is not in during the NIS");
    // The location card from `subtitle 0 4000` at frame 300 (10 s).
    // The NIS ends, the mission starts and asks for NISlet 1.
    let mut guard = 0;
    while kind(g) == Some(Kind::Opening) {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 10, "the opening did not end");
    }
    assert!(g.world.run.game.config.mission == 1);
    let mut guard = 0;
    while kind(g) != Some(Kind::Nislet(1)) {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 20, "NISlet 1 did not start");
    }
    eprintln!("NISlet 1 starts at game time {}", g.world.universe().total_time_elapsed);
    for (t, name) in [
        (2.0, "nislet1_01"),
        (24.5, "nislet1_02_location_card"),
        (40.0, "nislet1_03_kushan"),
        (75.0, "nislet1_04_taiidan"),
        (95.0, "nislet1_05_battle"),
    ] {
        if !shot_at(g, &mut r, &mut audio, Kind::Nislet(1), t, name) {
            break;
        }
    }
    let mut guard = 0;
    while kind(g) == Some(Kind::Nislet(1)) {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 200, "NISlet 1 did not end");
    }
    assert_eq!(g.world.run.var(None, "G_Nislet1Complete"), Some(1));
    // Then the mission goes on: the fleet comes in from hyperspace.
    let mut guard = 0;
    while !g.world.run.game.ships_have_arrived {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 120, "the fleet never arrived");
    }
    g.render(&mut r, SIZE).expect("render");
    save(g, "mission01_after_nis");
}

#[test]
fn escape_skips_the_opening() {
    let Some((mut game, _r, mut audio)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    for _ in 0..16 * 5 {
        g.update(DT, &mut audio);
    }
    assert_eq!(kind(g), Some(Kind::Opening));
    g.input(InputEvent::KeyDown(hwc_ui::consts::key::ESCAPE), &mut audio);
    for _ in 0..4 {
        g.update(DT, &mut audio);
    }
    assert_ne!(kind(g), Some(Kind::Opening), "Escape did not end the opening");
}

