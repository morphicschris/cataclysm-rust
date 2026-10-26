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
    g.canvas()
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

struct Rig {
    data: Rc<GameData>,
    session: Session,
}

impl Rig {
    /// The NIS level has handed over: the mission loads from it.
    fn mission(&self, game: Box<Game>, renderer: &mut Renderer) -> Box<Game> {
        let skill = game.world.skill();
        let crate::world::Sim::NisLevel(nis) = game.world.into_sim() else {
            panic!("not an NIS level");
        };
        let mut l = Loading::after_nis_level(self.data.clone(), &self.session, *nis, skill, SIZE)
            .expect("loading");
        for _ in 0..20 {
            match l.advance(&self.session, Some(&mut *renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission did not start after its NIS: {e}"),
            }
        }
        panic!("loading never finished");
    }
}

type Started = (Box<Game>, Renderer, Option<crate::audio::Audio>, Rig);

fn start() -> Option<Started> {
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
    let mut loading = Loading::new(data.clone(), &session, 1, 2, SIZE).ok()?;
    loading.opening_nis = true;
    for _ in 0..20 {
        match loading.advance(&session, Some(&mut renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => return Some((g, renderer, audio, Rig { data, session })),
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
fn shot_at(
    g: &mut Game,
    r: &mut Renderer,
    audio: &mut Option<crate::audio::Audio>,
    want: Kind,
    t: f32,
    name: &str,
) -> bool {
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
    let Some((mut game, mut r, mut audio, rig)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    g.update(DT, &mut audio);
    assert_eq!(
        kind(g),
        Some(Kind::Opening),
        "the opening NIS did not start"
    );
    {
        let n = g.nis.as_ref().unwrap();
        assert_eq!(n.player.data.objects.len(), 46);
        assert!((n.player.length() - 110.633).abs() < 0.01);
        // It plays in its own level; the mission is not loaded yet.
        assert_eq!(
            g.world.nis_level().map(|l| (l.mission, l.nis)),
            Some((1, 1))
        );
        assert!(g.world.mission_run().is_none());
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
        assert!(
            shot_at(g, &mut r, &mut audio, Kind::Opening, t, name),
            "opening ended before {t}s"
        );
        let b = brightness(g);
        eprintln!(
            "{name}: brightness {b:.1}, camera {:?}",
            g.nis.as_ref().map(|n| n.player.camera.eye)
        );
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
    // The NIS ends; the mission loads behind its own loading screen, starts and asks for
    // NISlet 1.
    let mut guard = 0;
    while !g
        .update(DT, &mut audio)
        .contains(&crate::game::Outcome::NisLevelDone)
    {
        guard += 1;
        assert!(guard < 16 * 10, "the opening did not end");
    }
    assert_eq!(kind(g), None);
    let mut game = rig.mission(game, &mut r);
    let g = &mut *game;
    assert_eq!(g.world.run().var(None, "G_Nis1Complete"), Some(1));
    assert!(g.world.run().game.config.mission == 1);
    let mut guard = 0;
    while kind(g) != Some(Kind::Nislet(1)) {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 20, "NISlet 1 did not start");
    }
    eprintln!(
        "NISlet 1 starts at game time {}",
        g.world.universe().total_time_elapsed
    );
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
    assert_eq!(g.world.run().var(None, "G_Nislet1Complete"), Some(1));
    // Then the mission goes on: the fleet comes in from hyperspace.
    let mut guard = 0;
    while !g.world.run().game.ships_have_arrived {
        g.update(DT, &mut audio);
        guard += 1;
        assert!(guard < 16 * 120, "the fleet never arrived");
    }
    g.render(&mut r, SIZE).expect("render");
    save(g, "mission01_after_nis");
}

#[test]
fn escape_skips_the_opening() {
    let Some((mut game, _r, mut audio, _rig)) = start() else {
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
    assert_ne!(
        kind(g),
        Some(Kind::Opening),
        "Escape did not end the opening"
    );
}

/// Escape pressed again and again from the first frame: the opening ends, the mission loads
/// behind its own loading screen, NISlet 1 is skipped the same way and the mission goes on.
#[test]
fn rapid_escape_through_the_opening_and_nislet() {
    let Some((mut game, mut r, mut audio, rig)) = start() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let esc = |g: &mut Game, audio: &mut Option<crate::audio::Audio>| {
        g.input(InputEvent::KeyDown(hwc_ui::consts::key::ESCAPE), audio);
        g.input(InputEvent::KeyUp(hwc_ui::consts::key::ESCAPE), audio);
    };
    let mut guard = 0;
    loop {
        esc(&mut game, &mut audio);
        let out = game.update(DT, &mut audio);
        game.render(&mut r, SIZE).expect("render");
        if out.contains(&crate::game::Outcome::NisLevelDone) {
            break;
        }
        guard += 1;
        assert!(guard < 16 * 30, "Escape did not end the opening");
    }
    let mut game = rig.mission(game, &mut r);
    let g = &mut *game;
    let mut nislet = false;
    for _ in 0..16 * 60 {
        for _ in 0..3 {
            esc(g, &mut audio);
        }
        let out = g.update(DT, &mut audio);
        assert!(
            out.iter().all(|o| matches!(
                o,
                crate::game::Outcome::ApplyOptions | crate::game::Outcome::Animatic(_)
            )),
            "unexpected {out:?}"
        );
        g.render(&mut r, SIZE).expect("render");
        nislet |= kind(g) == Some(Kind::Nislet(1));
        if nislet && kind(g).is_none() && g.world.run().game.ships_have_arrived {
            break;
        }
    }
    assert!(nislet, "NISlet 1 never started");
    assert_eq!(g.world.run().var(None, "G_Nislet1Complete"), Some(1));
}
