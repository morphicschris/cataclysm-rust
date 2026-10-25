//! Game-data tests of the campaign on the game path: mission 1 played until the fleet is in,
//! the jump forced with the debug key (the original's C key), the whiteout, the
//! between-missions movie request, the next mission loaded with the carried fleet and its
//! arrival; the roll call of mission 3 (started from the warp fleet); the nebula of mission 6;
//! a banked NIS camera. Screenshots go to `screenshots/game-campaign/`. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Slow in a debug build: run with
//! `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use glam::Vec3;
use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::sp_state;
use hwc_ui::ingame::{InputEvent, Manager};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1024, 768);
const DT: f32 = 1.0 / 16.0;

fn save(g: &Game, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game-campaign");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    g.canvas()
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
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
    let (device, queue) = hwc_render::headless_device().ok()?;
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
    session.resize(SIZE.0 as u32, SIZE.1 as u32);
    let audio = GameData::open(&dir)
        .map_err(|e| e.to_string())
        .and_then(|g| crate::audio::Audio::open(g, false))
        .ok();
    Some(Rig {
        data,
        session,
        renderer,
        audio,
    })
}

impl Rig {
    /// Loads a mission through the loading screen.
    fn load(&mut self, loading: Loading) -> Box<Game> {
        let mut loading = loading;
        for _ in 0..40 {
            match loading.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(mut g) => {
                    g.debug_keys = true;
                    return g;
                }
                Progress::Abandon(e) => panic!("mission {} did not start: {e}", loading.mission),
            }
        }
        panic!("loading never finished");
    }

    fn loading(&self, mission: u32, nis: bool) -> Loading {
        let mut l =
            Loading::new(self.data.clone(), &self.session, mission, 2, SIZE).expect("loading");
        l.opening_nis = nis;
        l
    }

    /// One frame; the outcomes other than options.
    fn frame(&mut self, g: &mut Game) -> Vec<Outcome> {
        g.update(DT, &mut self.audio)
            .into_iter()
            .filter(|o| *o != Outcome::ApplyOptions)
            .collect()
    }

    fn frames_until(
        &mut self,
        g: &mut Game,
        limit: u32,
        what: &str,
        mut done: impl FnMut(&Game) -> bool,
    ) {
        let mut n = 0;
        while !done(g) {
            let out = self.frame(g);
            assert!(
                out.is_empty(),
                "unexpected {out:?} while waiting for {what}"
            );
            n += 1;
            assert!(n < limit, "{what}: not reached in {limit} frames");
        }
    }

    fn shot(&mut self, g: &mut Game, name: &str) {
        g.render(&mut self.renderer, SIZE).expect("render");
        g.draw_cursor();
        save(g, name);
    }

    fn key(&mut self, g: &mut Game, code: u16) {
        g.input(InputEvent::KeyDown(code), &mut self.audio);
        g.input(InputEvent::KeyUp(code), &mut self.audio);
    }

    fn click(&mut self, g: &mut Game, x: i32, y: i32) {
        g.input(InputEvent::MouseMove { x, y }, &mut self.audio);
        self.frame(g);
        g.input(
            InputEvent::MouseButton {
                left: true,
                down: true,
            },
            &mut self.audio,
        );
        self.frame(g);
        g.input(
            InputEvent::MouseButton {
                left: true,
                down: false,
            },
            &mut self.audio,
        );
        self.frame(g);
    }
}

/// Mission 1 to mission 2 with the forced jump, through `Game::update` as the window drives
/// it.
#[test]
fn mission_one_to_two_with_the_forced_jump() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 60, "the fleet arriving in mission 1", |g| {
        g.world.run.game.ships_have_arrived && g.world.run.game.sp_state == sp_state::NONE
    });
    // Not offered yet: J does nothing; the debug key forces it.
    assert!(!g.world.run.game.hyper.player_can_hyperspace);
    rig.key(g, 0x4a);
    rig.frame(g);
    assert_eq!(
        g.world.run.game.sp_state,
        sp_state::NONE,
        "J jumped before the mission allowed it"
    );
    rig.key(g, 0x43);
    assert_eq!(g.world.run.game.sp_state, sp_state::WAITING_ROLLCALL);
    // Mission 1 has no roll call: five seconds of letterbox, then the fleet jumps.
    assert_ne!(g.ui.manager(), Some(Manager::RollCall));
    let mut whiteout_shots = 0;
    let mut movie = None;
    let mut frames = 0;
    loop {
        let out = rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 120, "the jump never finished");
        let h = g.world.run.game.hyper.clone();
        // The five seconds before a jump without roll call fade to black; then the whiteout
        // (opaque at its start).
        if !h.whiteout && whiteout_shots == 0 && h.fade > 0.5 {
            whiteout_shots += 1;
            rig.shot(g, "01_m1_jump_fade");
        } else if h.whiteout && whiteout_shots == 1 {
            whiteout_shots += 1;
            rig.shot(g, "02_m1_whiteout");
        }
        if let Some(Outcome::Animatic(n)) = out.first() {
            movie = Some(*n);
            rig.shot(g, "03_m1_before_movie");
            g.movie_finished();
        }
        if out.contains(&Outcome::NextMission) {
            break;
        }
    }
    assert_eq!(movie, Some(2), "Animatic02 was not asked for");
    assert!(g.world.run.mission_complete());
    let carry = g.world.run.carry().expect("carry");
    assert_eq!(carry.mission, 1);
    let ships = carry.fleet.len();
    assert!(ships >= 2, "the fleet did not jump: {ships}");
    drop(game);

    // Mission 2: the loading screen, then the fleet arrives through its windows.
    let l = rig.loading(2, true).with_carry(carry);
    let mut game = rig.load(l);
    let g = &mut *game;
    assert_eq!(g.world.run.game.config.mission, 2);
    let mut window_shot = false;
    let mut frames = 0;
    while !g.world.run.game.ships_have_arrived {
        let out = rig.frame(g);
        assert!(out.is_empty(), "unexpected {out:?}");
        frames += 1;
        assert!(frames < 16 * 120, "the fleet never arrived in mission 2");
        let ms = g.world.universe().base.players[0].mothership;
        let slicing = g
            .world
            .run
            .hyperspace_windows()
            .iter()
            .any(|&(id, _, clipt)| Some(id) == ms && clipt > 0.3 && clipt < 0.7);
        if !window_shot && slicing {
            rig.shot(g, "04_m2_arrival_windows");
            window_shot = true;
        }
    }
    assert!(window_shot, "no hyperspace window seen on arrival");
    rig.shot(g, "05_m2_arrived");
    // The location card (8 s, "Outskirts of Hiigaran System") is up for a while after.
    for _ in 0..16 * 3 {
        rig.frame(g);
    }
    rig.shot(g, "06_m2_location_card");
}

/// The opt-in `--rollcall-below-subtitles`: the roll call opens below the speech band instead
/// of the original's top-left place under it (`07b_m3_roll_call_below_band`).
#[test]
fn roll_call_below_the_band() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(3, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    g.rollcall_below_band = true;
    rig.frames_until(g, 16 * 90, "the fleet arriving in mission 3", |g| {
        g.world.run.game.ships_have_arrived && g.world.run.game.sp_state == sp_state::NONE
    });
    // A render places the screen for this resolution.
    g.render(&mut rig.renderer, SIZE).expect("render");
    rig.key(g, 0x43);
    rig.frame(g);
    rig.frame(g);
    assert_eq!(g.ui.manager(), Some(Manager::RollCall));
    for _ in 0..16 {
        rig.frame(g);
    }
    rig.shot(g, "07b_m3_roll_call_below_band");
    let band = g
        .subtitles
        .as_ref()
        .expect("subtitles")
        .band_rect(SIZE.0);
    let fe = &g.ui.fe;
    let go = fe.find_atom("GoNowHyperspace").expect("GoNowHyperspace");
    // 175 on the 640x480 layout, moved down by the band's bottom.
    assert_eq!(fe.atom(go).rect().y0, 175 + band.y1);
}

/// Mission 3 from the warp fleet: the debug jump opens the roll call; Quick Docking closes it.
#[test]
fn mission_three_roll_call() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(3, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 90, "the fleet arriving in mission 3", |g| {
        g.world.run.game.ships_have_arrived && g.world.run.game.sp_state == sp_state::NONE
    });
    // Launch what is docked so that the roll call has ships to count.
    for _ in 0..16 * 5 {
        rig.frame(g);
    }
    rig.key(g, 0x43);
    rig.frame(g);
    assert_eq!(g.world.run.game.sp_state, sp_state::WAITING_ROLLCALL);
    rig.frame(g);
    assert_eq!(
        g.ui.manager(),
        Some(Manager::RollCall),
        "the roll call screen did not open"
    );
    for _ in 0..16 {
        rig.frame(g);
    }
    let h = &g.world.run.game.hyper;
    eprintln!(
        "roll call: {} docked, {} remaining",
        h.rollcall_docked, h.rollcall_remaining
    );
    rig.shot(g, "07_m3_roll_call");
    rig.frame(g);
    let fe = &g.ui.fe;
    let go = fe.find_atom("GoNowHyperspace").expect("GoNowHyperspace");
    let em = fe
        .find_atom("EmergencyHyperspace")
        .expect("EmergencyHyperspace");
    let disabled = |a| {
        fe.atom(a)
            .region
            .and_then(|r| fe.region(r))
            .is_some_and(|r| r.status & hwc_ui::consts::RSF_REGION_DISABLED != 0)
    };
    assert!(
        disabled(go) != disabled(em),
        "exactly one jump button is disabled"
    );
    let target = if disabled(go) { em } else { go };
    let r = fe.atom(target).rect();
    let (x, y) = ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    rig.click(g, x, y);
    rig.frame(g);
    assert_ne!(
        g.ui.manager(),
        Some(Manager::RollCall),
        "the button did not close the roll call"
    );
    assert!(!g.world.run.game.hyper.rollcall_screen);
    // The fleet leaves through its windows; the last ship's slice drives the whiteout.
    let mut frames = 0;
    let mut shots = 0;
    loop {
        let out = rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 240, "the fleet of mission 3 never left");
        let (white, t) = (
            g.world.run.game.hyper.whiteout,
            g.world.run.game.hyper.whiteout_t,
        );
        if white && t < 0.5 && t > 0.1 && shots == 0 {
            rig.shot(g, "07b_m3_whiteout_flash");
            shots += 1;
        }
        if white && t > 0.55 && t < 0.95 && shots < 2 {
            rig.shot(g, "07c_m3_whiteout_fade");
            shots = 2;
        }
        if let Some(Outcome::Animatic(n)) = out.first() {
            assert_eq!(*n, 4);
            g.movie_finished();
        }
        if out.contains(&Outcome::NextMission) {
            break;
        }
    }
    eprintln!("mission 3 left after {frames} frames, {shots} whiteout shots");
}

/// Mission 6 (the Kadiir nebula) from the warp fleet: the nebula in the main view.
#[test]
fn mission_six_nebula() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(6, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    assert!(
        !g.world.universe().nebulae.list.is_empty(),
        "mission 6 has no nebula"
    );
    rig.frames_until(g, 16 * 90, "the fleet arriving in mission 6", |g| {
        g.world.run.game.ships_have_arrived
    });
    for _ in 0..16 * 2 {
        rig.frame(g);
    }
    rig.shot(g, "08_m6_arrived");
    // Look at the nearest nebula chunk from a distance.
    let u = g.world.universe();
    let eye = g.world.camera.eye();
    let chunk = u
        .nebulae
        .list
        .iter()
        .flat_map(|n| n.chunks.iter())
        .map(|c| Vec3::new(c.position.x, c.position.y, c.position.z))
        .min_by(|a, b| (*a - eye).length().total_cmp(&(*b - eye).length()))
        .expect("chunk");
    g.world.follow_focus = false;
    g.world.camera.look_at = chunk;
    g.world.camera.distance = 12_000.0;
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, "09_m6_nebula");
}

/// Mission 4 from the warp fleet starts with NIS 2 in its own level (`Mission04_NIS02`),
/// whose camera banks (the up vector off the vertical): drawn with the true bank, then the
/// mission follows.
#[test]
fn banked_nis_camera() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(4, true);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frame(g);
    let mut best = (0.0f32, 0u32);
    let mut frames = 0;
    let mut shot = false;
    while g.nis.as_ref().is_some_and(|n| n.kind.in_own_level()) {
        rig.frame(g);
        frames += 1;
        let Some(n) = g.nis.as_ref() else { break };
        let time = n.player.time;
        let c = n.player.camera;
        let f = Vec3::new(
            c.look_at.x - c.eye.x,
            c.look_at.y - c.eye.y,
            c.look_at.z - c.eye.z,
        )
        .normalize_or_zero();
        let up = Vec3::new(c.up.x, c.up.y, c.up.z);
        // The bank: the up vector against the vertical seen along the view.
        let r0 = f.cross(Vec3::Z).normalize_or_zero();
        let u0 = r0.cross(f);
        let bank = up.dot(r0).atan2(up.dot(u0)).to_degrees().abs();
        if bank > best.0 {
            best = (bank, frames);
        }
        if !shot && bank > 15.0 && frames > 16 * 3 {
            rig.shot(g, "10_nis_banked");
            eprintln!("banked NIS shot at {time:.1} s, bank {bank:.1} degrees");
            shot = true;
        }
        assert!(frames < 16 * 200, "NIS 2 did not end");
    }
    eprintln!("largest bank {:.1} degrees at frame {}", best.0, best.1);
    assert!(shot, "no banked camera in NIS 2");
    // The mission level takes over and its script starts.
    assert_eq!(g.world.run.game.config.mission, 4);
    for _ in 0..4 {
        rig.frame(g);
    }
    assert!(
        g.world.run.game.sp_state != sp_state::START_KAS,
        "mission 4 did not start after its NIS"
    );
    rig.shot(g, "11_m4_after_nis");
}
