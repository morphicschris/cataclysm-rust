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
        panic!("loading never finished: {:?}", loading.failure());
    }

    /// The NIS level's NIS has stopped: the mission loads from it on its own loading screen.
    fn after_nis_level(&mut self, game: Box<Game>) -> Box<Game> {
        let skill = game.world.skill();
        let crate::world::Sim::NisLevel(nis) = game.world.into_sim() else {
            panic!("not an NIS level");
        };
        let l = Loading::after_nis_level(self.data.clone(), &self.session, *nis, skill, SIZE)
            .expect("loading");
        self.load(l)
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
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    // Not offered yet: J does nothing; the debug key forces it.
    assert!(!g.world.run().game.hyper.player_can_hyperspace);
    rig.key(g, 0x4a);
    rig.frame(g);
    assert_eq!(
        g.world.run().game.sp_state,
        sp_state::NONE,
        "J jumped before the mission allowed it"
    );
    rig.key(g, 0x43);
    assert_eq!(g.world.run().game.sp_state, sp_state::WAITING_ROLLCALL);
    // Mission 1 has no roll call: five seconds of letterbox, then the fleet jumps.
    assert_ne!(g.ui.manager(), Some(Manager::RollCall));
    let mut whiteout_shots = 0;
    let mut movie = None;
    let mut frames = 0;
    loop {
        let out = rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 120, "the jump never finished");
        let h = g.world.run().game.hyper.clone();
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
    assert!(g.world.run().mission_complete());
    let carry = g.world.run().carry().expect("carry");
    assert_eq!(carry.mission, 1);
    let ships = carry.fleet.len();
    assert!(ships >= 2, "the fleet did not jump: {ships}");
    drop(game);

    // Mission 2: the loading screen, then the fleet arrives through its windows.
    let l = rig.loading(2, true).with_carry(carry, None);
    let mut game = rig.load(l);
    let g = &mut *game;
    assert_eq!(g.world.run().game.config.mission, 2);
    let mut window_shot = false;
    let mut frames = 0;
    while !g.world.run().game.ships_have_arrived {
        let out = rig.frame(g);
        assert!(out.is_empty(), "unexpected {out:?}");
        frames += 1;
        assert!(frames < 16 * 120, "the fleet never arrived in mission 2");
        let ms = g.world.universe().base.players[0].mothership;
        let slicing = g
            .world
            .run()
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
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
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
    let band = g.subtitles.as_ref().expect("subtitles").band_rect(SIZE.0);
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
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    // Launch what is docked so that the roll call has ships to count.
    for _ in 0..16 * 5 {
        rig.frame(g);
    }
    rig.key(g, 0x43);
    rig.frame(g);
    assert_eq!(g.world.run().game.sp_state, sp_state::WAITING_ROLLCALL);
    rig.frame(g);
    assert_eq!(
        g.ui.manager(),
        Some(Manager::RollCall),
        "the roll call screen did not open"
    );
    for _ in 0..16 {
        rig.frame(g);
    }
    let h = &g.world.run().game.hyper;
    eprintln!(
        "roll call: {} docked, {} remaining",
        h.rollcall_docked, h.rollcall_remaining
    );
    rig.shot(g, "07_m3_roll_call");
    // Off the screen the camera still turns (right drag) and zooms (wheel); the screen stays.
    {
        use hwc_ui::ingame::InputEvent as E;
        assert!(!g
            .ui
            .fe
            .screen_contains(hwc_ui::ingame::rollcall::SCREEN, 600, 400));
        let angle = g.world.camera.angle;
        g.input(E::MouseMove { x: 600, y: 400 }, &mut rig.audio);
        g.input(
            E::MouseButton {
                left: false,
                down: true,
            },
            &mut rig.audio,
        );
        g.input(E::MouseMove { x: 700, y: 400 }, &mut rig.audio);
        g.input(
            E::MouseButton {
                left: false,
                down: false,
            },
            &mut rig.audio,
        );
        rig.frame(g);
        assert_ne!(
            g.world.camera.angle, angle,
            "the camera did not turn in the roll call"
        );
        let distance = g.world.camera.distance;
        g.input(E::Wheel { up: false }, &mut rig.audio);
        for _ in 0..8 {
            rig.frame(g);
        }
        assert_ne!(
            g.world.camera.distance, distance,
            "the camera did not zoom in the roll call"
        );
        assert_eq!(g.ui.manager(), Some(Manager::RollCall));
        g.input(
            E::MouseMove {
                x: SIZE.0 as i32 / 2,
                y: SIZE.1 as i32 / 2,
            },
            &mut rig.audio,
        );
    }
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
    assert!(!g.world.run().game.hyper.rollcall_screen);
    // The fleet leaves through its windows; the last ship's slice drives the whiteout.
    let mut frames = 0;
    let mut shots = 0;
    loop {
        let out = rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 240, "the fleet of mission 3 never left");
        let (white, t) = (
            g.world.run().game.hyper.whiteout,
            g.world.run().game.hyper.whiteout_t,
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
        g.world.run().game.ships_have_arrived
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
    assert_eq!(g.world.nis_level().map(|n| n.nis), Some(2));
    rig.frame(g);
    let mut best = (0.0f32, 0u32);
    let mut done = false;
    let mut frames = 0;
    let mut shot = false;
    let mut ejected_shot = false;
    let mut beast_shot = false;
    let hangar = |g: &Game| {
        g.world
            .universe()
            .ships
            .iter()
            .any(|s| s.ship_type == hwc_sim::statics::ShipType::bSectMoship)
    };
    assert!(hangar(g), "NIS 2 has no hangar module");
    while g.nis.as_ref().is_some_and(|n| n.kind.in_own_level()) {
        done |= rig.frame(g).contains(&Outcome::NisLevelDone);
        frames += 1;
        let Some(n) = g.nis.as_ref() else { break };
        let time = n.player.time;
        // The hangar module is ejected at 60.5 s and tumbles away to the end; the module
        // housekeeping leaves it alone while an NIS plays (it used to explode at once).
        if n.kind.in_own_level() && time < 78.0 {
            assert!(hangar(g), "the hangar module died at {time:.1} s");
        }
        // At 43.7 s the Beast takes the hangar: electric bolts crawl over it (`ebolts`, for
        // 2.5 s) and its infected look blends in over 4 s (`beastEffect`).
        if !beast_shot && time > 45.0 {
            let u = g.world.universe();
            let h = u
                .ships
                .iter()
                .find(|s| s.ship_type == hwc_sim::statics::ShipType::bSectMoship)
                .expect("hangar");
            assert!(h.beast.timer != 0.0, "beastEffect did not reach the hangar");
            assert!(h.flicker.state != 0, "ebolts did not reach the hangar");
            rig.shot(g, "10_nis_hangar_beast");
            beast_shot = true;
        }
        if !ejected_shot && time > 64.0 {
            rig.shot(g, "10_nis_hangar_ejected");
            ejected_shot = true;
        }
        let Some(n) = g.nis.as_ref() else { break };
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
    // The mission level loads behind its own loading screen and its script starts.
    let mut frames = 0;
    while !done {
        done = rig.frame(g).contains(&Outcome::NisLevelDone);
        frames += 1;
        assert!(frames < 16, "the NIS level did not hand over");
    }
    let mut game = rig.after_nis_level(game);
    let g = &mut *game;
    assert_eq!(g.world.run().game.config.mission, 4);
    assert_eq!(g.world.run().var(None, "G_Nis2Complete"), Some(1));
    for _ in 0..4 {
        rig.frame(g);
    }
    assert!(
        g.world.run().game.sp_state != sp_state::START_KAS,
        "mission 4 did not start after its NIS"
    );
    rig.shot(g, "11_m4_after_nis");
}

/// Mission 1: a Ctrl band box over the whole view gathers only ships that are not the
/// player's (`mrSelectHold`'s attack filters), a plain one only the player's, an Alt one
/// everything; then the sensors manager with an objective ping and a new-ship ping and the
/// tactical overlay's legend (`12_m1_sensors_pings`).
#[test]
fn band_boxes_and_sensors_pings() {
    use crate::host::PLAYER;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 60, "the fleet arriving in mission 1", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    g.render(&mut rig.renderer, SIZE).expect("render");
    let drag = |rig: &mut Rig, g: &mut Game, modifier: Option<u16>| -> Vec<hwc_sim::ShipId> {
        if let Some(k) = modifier {
            g.input(InputEvent::KeyDown(k), &mut rig.audio);
        }
        g.input(InputEvent::MouseMove { x: 2, y: 2 }, &mut rig.audio);
        g.input(
            InputEvent::MouseButton {
                left: true,
                down: true,
            },
            &mut rig.audio,
        );
        g.input(
            InputEvent::MouseMove {
                x: SIZE.0 - 3,
                y: SIZE.1 - 3,
            },
            &mut rig.audio,
        );
        rig.frame(g);
        let gathered = g.selecting.clone();
        g.input(
            InputEvent::MouseButton {
                left: true,
                down: false,
            },
            &mut rig.audio,
        );
        if let Some(k) = modifier {
            g.input(InputEvent::KeyUp(k), &mut rig.audio);
        }
        rig.frame(g);
        gathered
    };
    let own = |g: &Game, id: hwc_sim::ShipId| {
        g.world
            .universe()
            .ship(id)
            .is_some_and(|s| s.player == PLAYER)
    };
    let plain = drag(&mut rig, g, None);
    assert!(!plain.is_empty(), "nothing of the fleet in view");
    assert!(plain.iter().all(|&id| own(g, id)));
    let ctrl = drag(&mut rig, g, Some(0x11));
    assert!(
        ctrl.iter().all(|&id| !own(g, id)),
        "a Ctrl band box lit up own ships: {ctrl:?}"
    );
    let alt = drag(&mut rig, g, Some(0x12));
    assert!(plain.iter().all(|id| alt.contains(id)));

    // The sensors manager with two pings around the mothership.
    let u = g.world.universe();
    let ms = u.base.players[PLAYER].mothership.expect("mothership");
    let at = u.ship(ms).expect("mothership").posinfo.position;
    let now = u.total_time_elapsed;
    g.pings.anomaly_point("TestObjective", at, now);
    g.pings
        .new_ship(hwc_math::Vector::new(at.x + 3000.0, at.y, at.z), now);
    g.ui.fe.game.ig.overlay.tactical = true;
    g.toggle_sensors(&mut rig.audio);
    rig.frames_until(g, 16 * 10, "the sensors manager", |g| {
        g.sensors.phase() == crate::sensors::Phase::Sensors && !g.sensors.zooming()
    });
    // A second into the pulse.
    for _ in 0..16 {
        rig.frame(g);
    }
    rig.shot(g, "12_m1_sensors_pings");
    // The two test pings, beside whatever the mission and its battles have up.
    assert!(g
        .pings
        .list
        .iter()
        .any(|p| p.mask == crate::pings::mask::NEW_SHIPS));
    assert_eq!(g.pings.anomaly_remove("testobj"), 1);
}

/// Mission 1's scripted intro: the script opens the sensors manager for the fleet intel and
/// pings the Taiidan with `PingAddShips` / `PingAddPoint`; the pings are in the list and drawn
/// while the letterbox is up (`13_m1_intel_pings`). The Taiidan ships no ship of the player
/// sees are hidden by the fog of war in the sensors manager.
#[test]
fn intro_pings_show_in_the_fleet_intel_and_the_fog_hides_the_enemy() {
    use crate::host::PLAYER;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    let mut frames = 0;
    let mut kas_pings = 0;
    while frames < 16 * 600 {
        let out = rig.frame(g);
        assert!(!out.contains(&Outcome::NextMission));
        frames += 1;
        // The script's own pings (not the level's anomalous derelicts or big-gun shells).
        kas_pings = g
            .pings
            .list
            .iter()
            .filter(|p| {
                matches!(&p.eval, crate::pings::Eval::Anomaly { name, .. }
                    if name != crate::pings::DERELICT_PING && name != crate::pings::BIG_GUN_PING)
            })
            .count();
        if kas_pings > 0
            && g.sensors.phase() == crate::sensors::Phase::Sensors
            && !g.sensors.zooming()
        {
            break;
        }
    }
    assert!(
        kas_pings > 0,
        "no anomaly pings from the mission script in {frames} frames"
    );
    assert_eq!(
        g.sensors.phase(),
        crate::sensors::Phase::Sensors,
        "the sensors manager is open"
    );
    for _ in 0..16 {
        rig.frame(g);
    }
    rig.shot(g, "13_m1_intel_pings");
    let u = g.world.universe();
    let fog = crate::sensors::Fog::of(u);
    assert!(fog.grid.is_some(), "the local player has a fog grid");
    let enemies: Vec<_> = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| u.ship(id).is_some_and(|s| s.player != PLAYER))
        .collect();
    assert!(!enemies.is_empty());
    let hidden = enemies
        .iter()
        .filter(|&&id| !crate::sensors::ship_visible(u, &fog, id))
        .count();
    assert!(hidden > 0, "every enemy ship is shown through the fog");
}

fn save_cam(g: &Game, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/cam");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    g.canvas()
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

/// Mission 1 from the opening NIS: the Kuun-Lan is never drawn before its window opens, and
/// the ships that travelled inside it come out through its launch points after the arrival
/// (`singlePlayerLoadNewLevel` docks them, `clAutoLaunch` at `G_ShipsHaveArrived` launches
/// them). Screenshots go to `screenshots/cam/`.
#[test]
fn mission_one_arrival_from_the_opening() {
    use hwc_sim::statics::ShipType;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, true);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    assert!(
        g.world.nis_level().is_some(),
        "mission 1 did not start with its NIS level"
    );
    rig.frame(g);
    let mut done = false;
    let mut frames = 0;
    for _ in 0..16 * 3 {
        rig.frame(g);
    }
    while !done {
        g.skip_nis();
        done = rig.frame(g).contains(&Outcome::NisLevelDone);
        if !done && frames < 6 {
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, &format!("m1_opening_skipped_{frames}"));
        }
        frames += 1;
        assert!(frames < 16 * 30, "the opening NIS did not end");
    }
    eprintln!("opening NIS ended {frames} frames after the skip");
    let mut game = rig.after_nis_level(game);
    let g = &mut *game;
    let mut log: Vec<String> = Vec::new();
    let mut bad: Vec<u32> = Vec::new();
    let mut frames = 0u32;
    let mut shots = 0;
    let mut was_drawn = false;
    let mut docked = 0;
    let mut before_shot = false;
    let (mut flash_max, mut flash_shot) = (0f32, false);
    loop {
        let r = g.world.run();
        if r.game.ships_have_arrived && r.game.sp_state == sp_state::NONE {
            break;
        }
        rig.frame(g);
        frames += 1;
        assert!(
            frames < 16 * 400,
            "the fleet never arrived in mission 1:\n{}",
            log.join("\n")
        );
        let u = g.world.universe();
        let nis = g
            .nis
            .as_ref()
            .map(|n| format!("{:?} {:.1}", n.kind, n.player.time));
        let ms = u.ship_list.iter().copied().find(|&id| {
            u.ship(id)
                .is_some_and(|s| s.ship_type == ShipType::sMothership && s.player == 0)
        });
        let Some(ms) = ms else {
            if frames.is_multiple_of(32) {
                log.push(format!(
                    "{frames}: no mothership; nis {nis:?} sp {}",
                    g.world.run().game.sp_state
                ));
            }
            continue;
        };
        let drawn = hwc_sim::effects::ship_drawn(u, ms);
        let flash = g.world.run().game.hyper.arrival_flash;
        flash_max = flash_max.max(flash);
        if !flash_shot && flash > 0.0 && flash < 0.6 {
            flash_shot = true;
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, "m1_arrival_white_flash");
        }
        if !drawn && !before_shot {
            // The view on the start point while the Kuun-Lan waits in hyperspace.
            before_shot = true;
            let u = g.world.universe();
            let at = u.ship(ms).map_or([0.0; 3], |s| {
                [
                    s.posinfo.position.x,
                    s.posinfo.position.y,
                    s.posinfo.position.z,
                ]
            });
            g.world.follow_focus = false;
            g.world.camera.look_at = glam::Vec3::from_array(at);
            g.world.camera.distance = 7000.0;
            g.world.camera.declination = (-20f32).to_radians();
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, "m1_arrival_before_window");
            g.world.follow_focus = true;
        }
        let u = g.world.universe();
        docked = docked.max(
            u.ship(ms)
                .and_then(|s| s.base.inside.as_ref())
                .map_or(0, |i| i.ships.len()),
        );
        if !drawn {
            let others: Vec<String> = u
                .ship_list
                .iter()
                .filter_map(|&id| u.ship(id))
                .filter(|s| {
                    s.player == 0
                        && s.ship_type.to_string().starts_with('s')
                        && hwc_sim::effects::ship_drawn(u, s.id)
                })
                .map(|s| s.ship_type.to_string())
                .collect();
            if !others.is_empty() {
                log.push(format!("{frames}: drawn before the Kuun-Lan: {others:?}"));
                bad.push(frames);
            }
        }
        let window = g
            .world
            .run()
            .hyperspace_windows()
            .iter()
            .find(|w| w.0 == ms)
            .map(|w| (w.1, w.2));
        if drawn != was_drawn || frames.is_multiple_of(8) {
            log.push(format!(
                "{frames}: ms {} drawn {drawn} window {window:?} nis {nis:?} sp {}",
                ms.0,
                g.world.run().game.sp_state
            ));
        }
        if drawn && !was_drawn {
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, &format!("m1_arrival_drawn_f{frames}"));
            if window.is_none() {
                bad.push(frames);
            }
        }
        if drawn && window.is_some() && shots < 3 && frames.is_multiple_of(6) {
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, &format!("m1_arrival_window_{shots}"));
            shots += 1;
        }
        was_drawn = drawn;
    }
    eprintln!("{}", log.join("\n"));
    assert!(
        bad.is_empty(),
        "the Kuun-Lan was drawn without its window at frames {bad:?}"
    );
    assert!(
        flash_max > 0.99 && flash_shot,
        "no white flash after the NISlet ({flash_max})"
    );
    assert!(
        g.world.run().game.hyper.arrival_flash <= 0.0,
        "the flash outlasted the arrival"
    );
    // The ships that travelled docked come out through the Kuun-Lan's launch points.
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("mothership");
    let inside = |g: &Game| {
        g.world
            .universe()
            .ship(ms)
            .and_then(|s| s.base.inside.as_ref())
            .map_or(0, |i| i.ships.len())
    };
    eprintln!(
        "ships inside the Kuun-Lan on the way in: {docked}; at the end of the arrival: {}",
        inside(g)
    );
    let mut launching_shot = false;
    g.world.focus = vec![ms];
    g.world.camera.distance = 5000.0;
    for f in 0..16 * 40 {
        rig.frame(g);
        let u = g.world.universe();
        let launching = u.ship_list.iter().any(|&id| {
            u.ship(id).is_some_and(|s| {
                s.player == 0
                    && s.command.is_some_and(|c| {
                        u.commands.get(c).is_some_and(|c| c.launch_from == Some(ms))
                    })
            })
        });
        if launching && !launching_shot && f > 8 {
            g.render(&mut rig.renderer, SIZE).expect("render");
            save_cam(g, "m1_arrival_launching");
            launching_shot = true;
            eprintln!("launching at frame {f} after the arrival");
        }
        if inside(g) == 0 && f > 16 * 10 {
            break;
        }
    }
    g.render(&mut rig.renderer, SIZE).expect("render");
    save_cam(g, "m1_arrival_launched");
    assert!(docked > 0, "nothing travelled inside the Kuun-Lan");
    assert_eq!(inside(g), 0, "the docked ships did not launch");
    assert!(launching_shot, "no ship came out through a launch point");
}

/// Mission 1 once the fleet is in: F on a worker focuses, F again eases in to frame it
/// (`zoomInCloseAsPossible`); the wheel stops at the framing distance and, with the hull
/// limit, before the near clip plane reaches the hull; a ship far off is drawn or not the
/// same whether it lies at the centre of the picture or near its corner. Screenshots go to
/// `screenshots/cam/` (prefix `cam_`).
#[test]
fn camera_focus_zoom_and_draw_distance() {
    use hwc_sim::statics::ShipType;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 60, "the fleet arriving in mission 1", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    for _ in 0..16 * 20 {
        rig.frame(g);
    }
    let u = g.world.universe();
    let worker = u
        .ship_list
        .iter()
        .copied()
        .find(|&id| {
            u.ship(id)
                .is_some_and(|s| s.player == 0 && s.ship_type == ShipType::sWorker)
        })
        .expect("a worker");
    g.selection = vec![worker];
    rig.key(g, 0x46);
    for _ in 0..16 {
        rig.frame(g);
    }
    assert_eq!(g.world.focus, vec![worker]);
    rig.shot(g, "x");
    save_cam(g, "cam_focus_once");
    let once = g.world.camera.distance;
    rig.key(g, 0x46);
    for _ in 0..24 {
        rig.frame(g);
    }
    rig.shot(g, "x");
    save_cam(g, "cam_focus_twice");
    let twice = g.world.camera.distance;
    eprintln!("F: {once:.0}; F again: {twice:.0}");
    let fit = g.world.focus_min_zoom(); // new
    assert!(
        twice < once * 0.5,
        "the second F did not zoom in: {once} -> {twice}"
    ); // new
    assert!(
        (twice - fit).abs() < fit * 0.02,
        "not at the framing distance {fit}: {twice}"
    ); // new
       // The wheel all the way in.
    for _ in 0..30 {
        g.input(InputEvent::Wheel { up: true }, &mut rig.audio);
        rig.frame(g);
    }
    for _ in 0..16 {
        rig.frame(g);
    }
    rig.shot(g, "x");
    save_cam(g, "cam_zoom_closest");
    let closest = g.world.camera.distance;
    eprintln!("closest zoom on a worker: {closest:.1}");
    let ships = g.world.focus_ships(); // new
    assert!(
        closest >= g.world.camera.clear_distance(&ships) - 0.5,
        "the near plane cuts the hull at {closest}"
    ); // new
    g.world.zoom_clear = false; // new
    for _ in 0..30 {
        // new
        g.input(InputEvent::Wheel { up: true }, &mut rig.audio); // new
        rig.frame(g); // new
    } // new
    rig.shot(g, "x"); // new
    save_cam(g, "cam_zoom_closest_original_limit"); // new
    eprintln!(
        "closest with the original limit: {:.1}",
        g.world.camera.distance
    ); // new
    g.world.zoom_clear = true; // new

    // Draw distance: the camera on the mothership at the farthest zoom, a Taiidan ship 21000
    // beyond it; seen once head-on (straight past the mothership) and once with the camera
    // turned 40 degrees about the mothership, which puts it near the top right corner.
    let u = g.world.universe();
    let ms = u.base.players[0].mothership.expect("mothership");
    let centre = glam::Vec3::from_array(
        u.ship(ms)
            .map(|s| {
                [
                    s.posinfo.position.x,
                    s.posinfo.position.y,
                    s.posinfo.position.z,
                ]
            })
            .unwrap(),
    );
    let far = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| u.ship(id).is_some_and(|s| s.player == 1))
        .max_by(|a, b| {
            let r = |id: hwc_sim::ShipId| {
                u.ship(id)
                    .and_then(|s| u.statics.ship(s.ship_type))
                    .map_or(0.0, |st| st.header.coll.coll_sphere_size)
            };
            r(*a).total_cmp(&r(*b))
        })
        .expect("an enemy ship");
    eprintln!("far ship: {}", u.ship(far).unwrap().ship_type);
    g.world.follow_focus = false;
    g.world.focus.clear();
    let along = glam::Vec3::new(-0.8, 0.5, -0.33).normalize();
    let side = along.cross(glam::Vec3::Z).normalize();
    let up = side.cross(along);
    let target = centre + along * 21000.0;
    {
        let s = g.world.universe_mut().ships.ship_mut(far);
        s.posinfo.position = hwc_math::Vector::new(target.x, target.y, target.z);
    }
    let turn = 40f32.to_radians();
    let mut seen = Vec::new();
    for (view, dir) in [
        ("head_on", along),
        (
            "corner",
            along * turn.cos() + (-side - up).normalize() * turn.sin(),
        ),
    ] {
        let eye = centre - dir * 14500.0;
        g.world.camera.set_eye_look_up(eye, centre, None);
        let drawn = g
            .world
            .frame((SIZE.0 as u32, SIZE.1 as u32))
            .ship_levels()
            .iter()
            .any(|&(id, _)| id == far);
        let in_front = (target - eye).dot(dir);
        rig.shot(g, "x");
        save_cam(g, &format!("cam_draw_{view}"));
        eprintln!(
            "{view}: depth {in_front:.0}, from the eye {:.0}: drawn {drawn}",
            (target - eye).length()
        );
        seen.push(drawn);
    }
    assert_eq!(seen[0], seen[1], "drawn {seen:?} head-on / near the corner");
    // `--draw-distance far`: beyond the render list, drawn within the far clip distance of the
    // eye (by radial distance) only where the player's ships see it, so the far view shows
    // nothing the sensors have not revealed. The enemy goes out along the view line.
    // The far clip opened out so the walk leaves what the fleet sees.
    let eye = centre - along * 14500.0;
    g.world.camera.set_eye_look_up(eye, centre, None);
    let clip_far = g.world.camera.tweaks.clip_far;
    g.world.camera.tweaks.clip_far = 150_000.0;
    let (mut seen_drawn, mut hidden_skipped) = (0, 0);
    for step in 0..40 {
        let target = centre + along * (4000.0 + 3000.0 * step as f32);
        let reach = (target - eye).length();
        if reach >= g.world.camera.tweaks.clip_far {
            break;
        }
        {
            let s = g.world.universe_mut().ships.ship_mut(far);
            s.posinfo.position = hwc_math::Vector::new(target.x, target.y, target.z);
        }
        let drawn_with = |g: &mut Game, render_list: bool| {
            g.world.render_list = render_list;
            g.world
                .frame((SIZE.0 as u32, SIZE.1 as u32))
                .ship_levels()
                .iter()
                .any(|&(id, _)| id == far)
        };
        let listed = drawn_with(g, true);
        let drawn = drawn_with(g, false);
        let u = g.world.universe();
        let visible = crate::sensors::ship_visible(u, &crate::sensors::Fog::of(u), far);
        if visible {
            assert!(drawn, "far mode: a seen ship at {reach:.0} is not drawn");
            seen_drawn += 1;
        } else if !listed {
            assert!(!drawn, "far mode: an unseen ship at {reach:.0} is drawn");
            hidden_skipped += 1;
        }
    }
    eprintln!("far mode: {seen_drawn} seen and drawn, {hidden_skipped} unseen and hidden");
    assert!(seen_drawn > 0 && hidden_skipped > 0, "the walk out did not cover both");
    g.world.render_list = true;
    g.world.camera.tweaks.clip_far = clip_far;
    let _ = std::fs::remove_file(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game-campaign/x.png"),
    );
}

/// Mission 3's minefield: the level's mines are drawn as meshes in the main view, and the
/// sensors manager shows them only once the script calls `ShowMinesInSensors`.
#[test]
fn mission_three_mines() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(3, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 90, "the fleet arriving in mission 3", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    let u = g.world.universe();
    let mines: Vec<Vec3> = u
        .missile_list
        .iter()
        .filter_map(|&m| u.missiles.get(m))
        .filter(|m| m.kind == hwc_sim::missile::kind::MINE)
        .map(|m| {
            Vec3::new(
                m.posinfo.position.x,
                m.posinfo.position.y,
                m.posinfo.position.z,
            )
        })
        .collect();
    assert!(mines.len() > 100, "{} mines", mines.len());
    assert!(!g.world.run().game.show_mines_in_sensors);
    g.world.follow_focus = false;
    g.world.focus.clear();
    g.world.camera.look_at = mines[0];
    g.world.camera.distance = 600.0;
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, "10_m3_mine_close");
    g.world.camera.distance = 6000.0;
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, "10b_m3_minefield");

    // The player's ships that can attack are selected; a Ctrl band box over the whole view
    // gathers the mines in it (the main view does not hide them), and its release orders the
    // attack on them.
    let fleet: Vec<hwc_sim::ShipId> = {
        let u = g.world.universe();
        let own: Vec<_> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| crate::host::selectable(u, id))
            .collect();
        hwc_sim::command::attack_capable(u, &own)
    };
    assert!(!fleet.is_empty(), "nothing of the fleet can attack");
    g.selection = fleet.clone();
    g.input(InputEvent::KeyDown(0x11), &mut rig.audio);
    g.input(InputEvent::MouseMove { x: 2, y: 2 }, &mut rig.audio);
    g.input(
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
        &mut rig.audio,
    );
    g.input(
        InputEvent::MouseMove {
            x: SIZE.0 - 3,
            y: SIZE.1 - 3,
        },
        &mut rig.audio,
    );
    rig.frame(g);
    let gathered = g.selecting.clone();
    rig.shot(g, "10c_m3_mines_band_box");
    g.input(
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
        &mut rig.audio,
    );
    g.input(InputEvent::KeyUp(0x11), &mut rig.audio);
    rig.frame(g);
    let boxed: Vec<_> = gathered
        .iter()
        .filter(|id| id.mine().is_some())
        .copied()
        .collect();
    eprintln!("the Ctrl band box gathered {} mines", boxed.len());
    assert!(
        !boxed.is_empty(),
        "the Ctrl band box found no mine: {gathered:?}"
    );
    let u = g.world.universe();
    let ordered = g.selection.iter().any(|&id| {
        u.ship(id)
            .and_then(|s| s.command)
            .and_then(|c| u.commands.get(c))
            .is_some_and(|c| c.attack_targets.iter().any(|t| boxed.contains(t)))
    });
    assert!(ordered, "no attack order on the boxed mines");
}

/// Mission 1 once the fleet is in: the Alt focus (`ccFocusClose`) recentres on a ship at the
/// current zoom distance, also when repeated (`hwcAltFocusKeepsZoom`); with the original's
/// behaviour it closes in to frame the ship.
#[test]
fn alt_focus_keeps_the_zoom() {
    use hwc_sim::statics::ShipType;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 60, "the fleet arriving in mission 1", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    let u = g.world.universe();
    let workers: Vec<_> = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| {
            u.ship(id)
                .is_some_and(|s| s.player == 0 && s.ship_type == ShipType::sWorker)
        })
        .collect();
    assert!(workers.len() >= 2, "two workers");
    assert!(g.world.alt_focus_keeps_zoom, "on by default");
    g.world.focus_on(vec![workers[0]], false);
    for _ in 0..32 {
        rig.frame(g);
    }
    let before = g.world.camera.distance;
    g.world.focus_on(vec![workers[1]], true);
    for _ in 0..32 {
        rig.frame(g);
    }
    assert_eq!(g.world.focus, vec![workers[1]]);
    let after = g.world.camera.distance;
    assert!(
        (after - before).abs() < 1.0,
        "Alt focus zoomed: {before} -> {after}"
    );
    g.world.focus_on(vec![workers[1]], true);
    for _ in 0..32 {
        rig.frame(g);
    }
    let again = g.world.camera.distance;
    assert!(
        (again - before).abs() < 1.0,
        "Alt focus again zoomed: {before} -> {again}"
    );

    g.world.alt_focus_keeps_zoom = false;
    g.world.focus_on(vec![workers[0]], true);
    for _ in 0..48 {
        rig.frame(g);
    }
    let fit = g.world.focus_min_zoom();
    let close = g.world.camera.distance;
    eprintln!("kept: {before:.0}; original close-up: {close:.0} (framing {fit:.0})");
    assert!(
        (close - fit).abs() < fit * 0.02,
        "original Alt focus not at the framing distance {fit}: {close}"
    );
}

/// Mission 4: the genetics module infects the Clee-San and the Beast's infection beam
/// (`eboltCascade.ebg`, its length kept at the source-to-target distance by
/// `beastCascadeEffectUpdate`) is drawn from the module to the ship.
#[test]
fn mission_four_infection_beam() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(4, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    let beam = |g: &Game| {
        let u = g.world.universe();
        u.bullet_list.iter().copied().find(|&b| {
            u.bullets.get(b).is_some_and(|b| {
                b.weapon == hwc_sim::bullet::weapon::BEASTINFECT && b.time_lived > 0.0
            })
        })
    };
    rig.frames_until(g, 16 * 90, "the fleet arriving in mission 4", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    // The Clee-San is escorted to the genetics module (a Beast mothership hulk), which then
    // infects it (the script's "Switch": unlimited infection and SpecialOn the Clee-San).
    // Escorting takes the player's fleet; here the Clee-San is put next to the module and
    // the module told to infect it.
    {
        let u = g.world.universe_mut();
        let find = |u: &hwc_sim::Universe, t| {
            u.ships
                .iter()
                .find(|s| s.ship_type == t)
                .map(|s| (s.id, s.posinfo.position))
        };
        let (module, mp) =
            find(u, hwc_sim::statics::ShipType::dBeastMothership).expect("genetics module");
        let (cs, old) = find(u, hwc_sim::statics::ShipType::dEcclesiam).expect("the Clee-San");
        let new = hwc_math::Vector::new(mp.x + 2000.0, mp.y, mp.z);
        u.ships.ship_mut(cs).posinfo.position = new;
        hwc_sim::fow::move_viewer(u, cs, old, new);
        hwc_sim::ships::beast::kas_set_infection_unlimited(u, &[module], true);
        hwc_sim::ships::cl_special(u, &[module], &[cs]);
        hwc_sim::ships::beast::kas_set_infection_unlimited(u, &[module], false);
    }
    rig.frames_until(g, 16 * 30, "the infection beam", |g| beam(g).is_some());
    for i in 0..3 {
        let Some(bid) = beam(g) else { break };
        let b = g.world.universe().bullets.bullet(bid);
        let p = Vec3::new(
            b.posinfo.position.x,
            b.posinfo.position.y,
            b.posinfo.position.z,
        );
        let l = Vec3::new(b.length_vec.x, b.length_vec.y, b.length_vec.z);
        g.world.follow_focus = false;
        g.world.focus.clear();
        g.world.camera.look_at = p + l * 0.5;
        g.world.camera.distance = (l.length() * 1.2).max(800.0);
        rig.frame(g);
        rig.shot(g, &format!("12_m4_infection_beam_{i}"));
        for _ in 0..8 {
            rig.frame(g);
        }
    }
}

/// The types of the ships the main camera focuses on.
fn focus_types(g: &Game) -> Vec<String> {
    let u = g.world.universe();
    g.world
        .focus
        .iter()
        .filter_map(|id| u.ship(*id))
        .map(|s| format!("{:?} {:?}", s.ship_type, s.id))
        .collect()
}

/// Plays on, logging every focus the mission's script gives the main camera with three shots
/// of it (`<name>_cut_<n>_<i>`), until `done` says so after a cut or `frames` frames pass.
/// The lines, one per cut.
fn scripted_cuts(
    rig: &mut Rig,
    g: &mut Game,
    name: &str,
    frames: u32,
    mut done: impl FnMut(&Game, usize) -> bool,
) -> Vec<String> {
    let mut last = (g.world.focus.clone(), g.world.chase);
    let mut cuts = Vec::new();
    for frame in 0..frames {
        rig.frame(g);
        if (&g.world.focus, g.world.chase) == (&last.0, last.1) {
            continue;
        }
        last = (g.world.focus.clone(), g.world.chase);
        let c = g.world.camera;
        let line = format!(
            "frame {frame}: focus {:?}, angle {:.1}, declination {:.1}, distance {:.0}, chase {}",
            focus_types(g),
            c.angle.to_degrees(),
            c.declination.to_degrees(),
            c.distance,
            g.world.chase,
        );
        eprintln!("{line}");
        for (i, wait) in [1, 24, 48].into_iter().enumerate() {
            for _ in 0..wait {
                rig.frame(g);
            }
            rig.shot(g, &format!("{name}_cut_{}_{i}", cuts.len()));
        }
        cuts.push(line);
        if done(g, cuts.len()) {
            break;
        }
    }
    eprintln!(
        "{name}: game time {:.0} s, universe paused {}",
        g.world.universe().total_time_elapsed,
        g.world.universe().universe_paused
    );
    cuts
}

/// Sends a KAS message to every team (`MsgSendAll`) or to the named one.
fn kas_message(g: &mut Game, to: Option<&str>, msg: u32) {
    let kas = &mut g.world.run_mut().kas;
    let teams: Vec<_> = match to {
        Some(t) => vec![hwc_kas::kas_hash(t)],
        None => kas.teams().map(|t| t.label).collect(),
    };
    for t in teams {
        kas.post_message(t, None, msg);
    }
}

/// Mission 5's scripted camera: the Beast ambushing the Bentusi, the Kiith Maanan carrier's
/// hyperspace signature (`CameraFocus` on the Caal-Shto: angle 62, declination 16, distance
/// 1200) and the cruiser the Beast takes over. The Beast is let loose by the message its
/// discovery sends to every team; shots of each cut in `screenshots/game-campaign/13_m5_*`.
#[test]
#[ignore]
fn mission_five_scripted_camera() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(5, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 120, "the fleet arriving in mission 5", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    kas_message(g, None, 0x0fe5fa04);
    let cuts = scripted_cuts(&mut rig, g, "13_m5", 16 * 900, |g, n| {
        n > 4 && focus_types(g).iter().any(|t| t.contains("HeavyCruiser"))
    });
    let carrier = cuts
        .iter()
        .find(|l| l.contains("kCarrier"))
        .expect("no cut to the carrier");
    assert!(
        carrier.contains("angle 62.0, declination 16.0, distance 1200"),
        "{carrier}"
    );
}

/// Mission 5's Bentusi going down: once the Bentusi station is gone the Beast's heavy cruiser
/// is left at 20% health with its self-repair off (`TeamHealthSet(20)` and
/// `ShipsRepairModifierSet(THISTEAMSHIPS, 0)` on the Bentusi's last message), so it keeps
/// smoking for the rest of the mission. The cruiser is put next to the station so it starts
/// infecting at once, and the station is scuttled the way the fleet intel does it.
#[test]
#[ignore]
fn mission_five_cruiser_smokes_after_the_bentusi() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(5, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 120, "the fleet arriving in mission 5", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    let team = |g: &Game, t: &str| {
        g.world
            .run()
            .game
            .level
            .team(hwc_kas::kas_hash(t))
            .map(|t| t.ships.clone())
            .unwrap_or_default()
    };
    let city = team(g, "trCity");
    let cruiser = team(g, "bHCruise1")[0];
    {
        let u = g.world.universe_mut();
        let cp = u.ships.ship(city[0]).posinfo.position;
        let old = u.ships.ship(cruiser).posinfo.position;
        let new = hwc_math::Vector::new(cp.x + 4000.0, cp.y, cp.z);
        u.ships.ship_mut(cruiser).posinfo.position = new;
        hwc_sim::fow::move_viewer(u, cruiser, old, new);
    }
    kas_message(g, None, 0x0fe5fa04);
    let infecting = hwc_kas::kas_hash("Infecting");
    rig.frames_until(g, 16 * 60, "the cruiser infecting the Bentusi", |g| {
        g.world
            .run()
            .kas
            .team(hwc_kas::kas_hash("bHCruise1"))
            .and_then(|t| t.state)
            == Some(infecting)
    });
    // The Bentusi found (their own discovery message), then scuttled.
    kas_message(g, Some("trCity"), 0x015e8174);
    let found = hwc_kas::kas_hash("Found");
    rig.frames_until(g, 16 * 10, "the Bentusi found", |g| {
        g.world
            .run()
            .kas
            .team(hwc_kas::kas_hash("trCity"))
            .and_then(|t| t.state)
            == Some(found)
    });
    {
        let u = g.world.universe_mut();
        let now = u.total_time_elapsed;
        for &id in &city {
            if let Some(s) = u.ships.get_mut(id) {
                s.death_time = now + 1.0;
            }
        }
    }
    kas_message(g, Some("trCity"), hwc_kas::kas_hash("Scuttle"));
    let attack = hwc_kas::kas_hash("ATTACKPlayer");
    rig.frames_until(
        g,
        16 * 60,
        "the Bentusi gone and the cruiser let loose",
        |g| {
            g.world
                .run()
                .kas
                .team(hwc_kas::kas_hash("bHCruise1"))
                .and_then(|t| t.state)
                == Some(attack)
        },
    );
    let ratio = |g: &Game| {
        let u = g.world.universe();
        let s = u.ships.ship(cruiser);
        s.health / u.statics.ship(s.ship_type).unwrap().health.max_health
    };
    let start = ratio(g);
    assert!(
        (start - 0.2).abs() < 0.01,
        "cruiser at {start} of its health"
    );
    for _ in 0..16 * 60 {
        rig.frame(g);
    }
    let end = ratio(g);
    let heavy = g
        .world
        .universe()
        .effects
        .as_ref()
        .and_then(|e| e.damage.get(&cruiser))
        .map_or(0, |d| d.count(hwc_sim::effects::damage::HEAVY));
    eprintln!("cruiser health {start} -> {end}, heavy damage effects {heavy}");
    assert!(
        end <= start + 1e-4,
        "the cruiser repaired itself: {start} -> {end}"
    );
    assert!(heavy > 0, "the cruiser shows no heavy damage");
}

/// Mission 5's two carriers as the script has them. The Kiith Maanan carrier (the Caal-Shto)
/// and its guards dock their fighters and wait in hyperspace, come in on the hyperspace
/// signature and turn into the player's (not controllable); when the Bentusi go the guards
/// scuttle and the Caal-Shto is left helpless, not retaliated against, kept at 40% health or
/// below with its self-repair off. The Beast carrier crawls (top speed 1), and below 80%
/// average health sends its docked last wave out, which tells it to "leave": it hyperspaces
/// out.
#[test]
#[ignore]
fn mission_five_carriers() {
    use hwc_sim::object::sof;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(5, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 120, "the fleet arriving in mission 5", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    let team = |g: &Game, t: &str| {
        g.world
            .run()
            .game
            .level
            .team(hwc_kas::kas_hash(t))
            .map(|t| t.ships.clone())
            .unwrap_or_default()
    };
    let state = |g: &Game, t: &str| {
        g.world
            .run()
            .kas
            .team(hwc_kas::kas_hash(t))
            .and_then(|t| t.state)
    };
    let ratio = |g: &Game, id| {
        let u = g.world.universe();
        u.ships.get(id).map_or(-1.0, |s| {
            s.health / u.statics.ship(s.ship_type).unwrap().health.max_health
        })
    };
    let flags = |g: &Game, id| g.world.universe().ships.get(id).map(|s| s.flags);
    let caal = team(g, "CaalShto");
    let guards: Vec<_> = [team(g, "CaalShtoGuard1"), team(g, "CaalShtoGuard2")].concat();
    let bcarrier = team(g, "bCarrier1");
    eprintln!("Caal-Shto {caal:?}, guards {guards:?}, Beast carrier {bcarrier:?}");
    assert!(!caal.is_empty() && !guards.is_empty() && !bcarrier.is_empty());
    rig.frames_until(g, 16 * 60, "the Caal-Shto waiting in hyperspace", |g| {
        caal.iter()
            .chain(&guards)
            .all(|&id| flags(g, id).is_some_and(|f| f & sof::HYPERSPACE != 0))
    });
    for &id in &bcarrier {
        let s = g.world.universe().ships.ship(id);
        assert!(
            s.special_flags & hwc_sim::object::special::CUSTOM_MAX_VELOCITY != 0
                && s.special_max_velocity == 1.0,
            "Beast carrier top speed {} (special flags {:#x})",
            s.special_max_velocity,
            s.special_flags
        );
    }

    // The hyperspace signature.
    kas_message(g, None, 0x0d89f74e);
    rig.frames_until(g, 16 * 60, "the Caal-Shto in and the player's", |g| {
        let u = g.world.universe();
        caal.iter().chain(&guards).all(|&id| {
            u.ships.get(id).is_some_and(|s| {
                s.flags & sof::HYPERSPACE == 0 && s.player == 0 && s.special_flags2 & 0x0010_0000 != 0
            })
        })
    });

    // The Bentusi destroyed.
    kas_message(g, None, 0x010a2ed4);
    rig.frames_until(g, 16 * 10, "the Caal-Shto left helpless", |g| {
        state(g, "CaalShto") == Some(0x08761820)
    });
    for _ in 0..16 * 5 {
        rig.frame(g);
    }
    for &id in &caal {
        let s = g.world.universe().ships.ship(id);
        eprintln!(
            "Caal-Shto {id:?}: health {:.3}, flags {:#x}, special {:#x}, repair {}",
            ratio(g, id),
            s.flags,
            s.special_flags,
            s.base.health_mult
        );
        assert!((ratio(g, id) - 0.4).abs() < 0.01, "health {}", ratio(g, id));
        assert!(s.flags & sof::DISABLED != 0, "not helpless");
        assert!(s.special_flags & 0x4_0000 != 0, "not set to non-retaliation");
        assert!(s.base.health_mult == 0.0, "still repairs itself");
    }
    // Healed back over 40% it is set back.
    {
        let u = g.world.universe_mut();
        for &id in &caal {
            let m = u.statics.ship(u.ships.ship(id).ship_type).unwrap().health.max_health;
            u.ships.ship_mut(id).health = m * 0.9;
        }
    }
    for _ in 0..16 * 2 {
        rig.frame(g);
    }
    for &id in &caal {
        assert!(ratio(g, id) <= 0.401, "Caal-Shto at {}", ratio(g, id));
    }
    rig.frames_until(g, 16 * 30, "the guards scuttled", |g| {
        guards
            .iter()
            .all(|&id| g.world.universe().ships.get(id).is_none_or(|s| s.flags & sof::DEAD != 0))
    });

    // The Beast carrier hurt below 80%: the last wave launches and the carrier leaves.
    {
        let u = g.world.universe_mut();
        for &id in &bcarrier {
            let m = u.statics.ship(u.ships.ship(id).ship_type).unwrap().health.max_health;
            u.ships.ship_mut(id).health = m * 0.7;
        }
    }
    rig.frames_until(g, 16 * 90, "the Beast carrier hyperspacing out", |g| {
        bcarrier
            .iter()
            .all(|&id| flags(g, id).is_none_or(|f| f & sof::HYPERSPACE != 0))
    });
}

/// Mission 4's scripted camera: the pirates' heavy cruiser hyperspacing in (`CameraFocusClose`
/// at angle 341, declination -5), the Beast taking it over at the genetics module, and the
/// rest of the mission's cuts. Started with the "blurb5" message its fleet intel waits for;
/// shots in `screenshots/game-campaign/14_m4_*`.
#[test]
#[ignore]
fn mission_four_scripted_camera() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(4, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    rig.frames_until(g, 16 * 120, "the fleet arriving in mission 4", |g| {
        g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == sp_state::NONE
    });
    // A mission started on its own has a second Clee-San (the level's and the fleet's); with
    // two in focus the ride along switches itself off, so one goes.
    let extra: Vec<_> = {
        let u = g.world.universe();
        let all: Vec<_> = u
            .ships
            .iter()
            .filter(|s| s.ship_type == hwc_sim::statics::ShipType::dEcclesiam)
            .map(|s| s.id)
            .collect();
        all.into_iter().skip(1).collect()
    };
    for id in extra {
        let u = g.world.universe_mut();
        let effects = std::mem::take(&mut u.effects);
        hwc_sim::damage::really_delete_ship(u, id);
        u.effects = effects;
    }
    // The Clee-San escorted to the genetics module (put next to it) and infected, with the
    // ride along on board (`CameraSetChaseMode`); then the pirates and their cruiser.
    {
        let u = g.world.universe_mut();
        let find = |u: &hwc_sim::Universe, t| {
            u.ships
                .iter()
                .find(|s| s.ship_type == t)
                .map(|s| (s.id, s.posinfo.position))
        };
        let (_, mp) =
            find(u, hwc_sim::statics::ShipType::dBeastMothership).expect("genetics module");
        let (cs, old) = find(u, hwc_sim::statics::ShipType::dEcclesiam).expect("the Clee-San");
        let new = hwc_math::Vector::new(mp.x + 2000.0, mp.y, mp.z);
        u.ships.ship_mut(cs).posinfo.position = new;
        hwc_sim::fow::move_viewer(u, cs, old, new);
    }
    kas_message(g, Some("FleetIntel"), hwc_kas::kas_hash("blurb2"));
    let mut cuts = scripted_cuts(&mut rig, g, "14_m4a", 16 * 600, |g, n| {
        n >= 3 && focus_types(g).iter().any(|t| t.contains("Mothership"))
    });
    for _ in 0..16 * 20 {
        rig.frame(g);
    }
    kas_message(g, Some("FleetIntel"), hwc_kas::kas_hash("blurb5"));
    cuts.extend(scripted_cuts(&mut rig, g, "14_m4b", 16 * 600, |g, n| {
        n >= 2 && focus_types(g).iter().any(|t| t.contains("sMothership"))
    }));
    // The cruiser brought to the genetics module, which takes it over ("blurb6").
    {
        let u = g.world.universe_mut();
        let find = |u: &hwc_sim::Universe, t| {
            u.ships
                .iter()
                .find(|s| s.ship_type == t)
                .map(|s| (s.id, s.posinfo.position))
        };
        let (_, mp) =
            find(u, hwc_sim::statics::ShipType::dBeastMothership).expect("genetics module");
        let (hc, old) =
            find(u, hwc_sim::statics::ShipType::tHeavyCruiser).expect("the heavy cruiser");
        let new = hwc_math::Vector::new(mp.x - 4000.0, mp.y, mp.z);
        u.ships.ship_mut(hc).posinfo.position = new;
        hwc_sim::fow::move_viewer(u, hc, old, new);
    }
    cuts.extend(scripted_cuts(&mut rig, g, "14_m4c", 16 * 600, |_, n| {
        n >= 5
    }));
    assert!(
        cuts.iter().any(|l| l.contains("chase true")),
        "no ride along on the Clee-San: {cuts:?}"
    );
    assert!(
        cuts.iter().any(|l| l.contains("HeavyCruiser")),
        "no cut to the heavy cruiser: {cuts:?}"
    );
}

/// Mission 6's opening NIS (n04 in `Mission06_NIS04`, "Into The Nebula"): its level's `Fog`
/// line loads the plasma cloud, the NIS starts without it, and at 45.3 s (frame 1360) its `fog`
/// event fades the cloud in over 13 s while the sky fades out. Run with
/// `cargo test --release -p cataclysm mission_six_nis_fog -- --ignored`; shots in
/// `screenshots/game-campaign/15_m6_nis_*`.
#[test]
#[ignore = "long: plays a minute of NIS 4"]
fn mission_six_nis_fog() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(6, true);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    assert_eq!(g.world.nis_level().map(|n| n.nis), Some(4));
    assert!(g.world.plasma.texture, "the level's Fog line loads the cloud");
    rig.frames_until(g, 16 * 10, "NIS 4 starting", |g| g.nis.is_some());
    let mut shots = [(40.0f32, "15_m6_nis_before_fog", false), (51.5, "15_m6_nis_fog_fading_in", false), (58.8, "15_m6_nis_fog", false)];
    let mut frames = 0;
    while g.nis.as_ref().is_some_and(|n| n.kind.in_own_level()) {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 200, "NIS 4 never ended");
        let Some(time) = g.nis.as_ref().map(|n| n.player.time) else { break };
        for (at, name, done) in &mut shots {
            if !*done && time >= *at {
                *done = true;
                rig.shot(g, name);
                let p = &g.world.plasma;
                eprintln!("{name}: {time:.1} s active {} sky {:.2} colour {:08x} scale {}", p.active, p.sky, p.colour, p.scale);
                if *at < 45.0 {
                    assert!(!p.active && p.sky == 1.0, "fog before its event");
                } else if *at > 58.5 {
                    assert!(p.active && p.sky == 0.0 && p.colour == 0xffdc_dcdc, "fog not in at {time:.1} s");
                }
            }
        }
        if shots.iter().all(|s| s.2) {
            break;
        }
    }
    assert!(shots.iter().all(|s| s.2), "NIS 4 ended before the fog");
}

/// Mission 7's slipgates: no icons and not selectable (`SlipgateInit`), each with its effect
/// (the permanent glow, then the opening vortex as the convoy comes through), and convoy
/// ships the script hands over without control are not selectable and drawn as allies.
/// `cargo test --release -p cataclysm mission_seven_slipgates -- --ignored --nocapture`
/// Fleet looks: a crystal half eaten by a depot's beam, a hive frigate with its wings open, a
/// retrofit in progress and a cloaked fighter fading.
#[test]
#[ignore]
fn fleet_looks() {
    use hwc_sim::statics::ShipType as T;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    for _ in 0..16 {
        rig.frame(g);
    }
    let u = g.world.universe_mut();
    let p = u.ships.iter().find(|s| s.player == crate::host::PLAYER).map_or(hwc_math::Vector::ZERO, |s| s.posinfo.position);
    let at = |x: f32, y: f32| hwc_math::Vector::new(p.x + x, p.y + y, p.z);
    let crystal = hwc_sim::base::create_ship(u, T::dCrystal0, crate::host::PLAYER, at(4000.0, 0.0), Default::default()).expect("crystal");
    hwc_sim::consume::start(u, crystal, at(3000.0, 0.0), 10.0);
    let hive = hwc_sim::base::create_ship(u, T::sHiveFrigate, crate::host::PLAYER, at(4000.0, 1800.0), Default::default()).expect("hive");
    if let hwc_sim::ships::ShipSpec::Hive(h) = &mut u.ships.ship_mut(hive).spec {
        h.state = 1;
    }
    let look = |g: &mut Game, id: hwc_sim::ShipId, d: f32| {
        let p = g.world.universe().ship(id).expect("ship").posinfo.position;
        g.world.follow_focus = false;
        g.world.camera.look_at = Vec3::new(p.x, p.y, p.z);
        g.world.camera.distance = d;
    };
    look(g, crystal, 2500.0);
    for _ in 0..16 * 5 {
        rig.frame(g);
    }
    assert!(hwc_sim::consume::is_active(g.world.universe(), crystal));
    rig.shot(g, "30_fleet_crystal_half_eaten");
    look(g, hive, 1800.0);
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, "31_fleet_hive_wings_open");

    // The player's own cloaked fighter: lit, without textures.
    let u = g.world.universe_mut();
    let fighter = hwc_sim::base::create_ship(u, T::bCloakedFighter, crate::host::PLAYER, at(4000.0, -1500.0), Default::default()).expect("fighter");
    u.ships.ship_mut(fighter).upgrade_flags |= hwc_sim::upgrade::type_mask(T::bCloakedFighter);
    look(g, fighter, 500.0);
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, "32_fleet_own_fighter_before_cloaking");
    let u = g.world.universe_mut();
    hwc_sim::cloak::toggle(u, fighter);
    let mut cloaked = false;
    for _ in 0..16 * 8 {
        rig.frame(g);
        if hwc_sim::cloak::is_cloaked(g.world.universe().ship(fighter).expect("fighter")) {
            cloaked = true;
            break;
        }
    }
    assert!(cloaked, "the fighter cloaks");
    rig.shot(g, "32b_fleet_own_cloaked_fighter");

    // The special orders explain themselves: nothing to salvage near the workers.
    let u = g.world.universe_mut();
    let worker = hwc_sim::base::create_ship(u, T::sWorker, crate::host::PLAYER, at(-2000.0, -2000.0), Default::default()).expect("worker");
    u.ships.ship_mut(worker).upgrade_flags |= hwc_sim::upgrade::type_mask(T::sWorker);
    g.messages.clear();
    hwc_sim::player_orders::special_target_order(
        g.world.universe_mut(),
        crate::host::PLAYER,
        &[worker],
        None,
        hwc_sim::player_orders::TargetOrder::Salvage,
    );
    rig.frame(g);
    let said: Vec<String> = g.messages.iter().map(|m| m.text.clone()).collect();
    println!("messages: {said:?}");
    assert!(said.iter().any(|m| m.contains("No Salvagable Ships Found")), "{said:?}");
    assert_eq!(g.messages.last().and_then(|m| m.ship), Some(crate::host::ui_id(worker)), "the message focuses on the worker");
    rig.shot(g, "34_fleet_nothing_to_salvage");
    // The Naggarok under way after the script's G_NaggarokEMP: its hull's ambient pulses
    // blue-white (meshCurrentMaterialDefault), a band per material.
    let u = g.world.universe_mut();
    let nag = hwc_sim::base::create_ship(u, T::dBeastMothership, 1, at(-30_000.0, 0.0), Default::default()).expect("naggarok");
    let to = at(-30_000.0, 40_000.0);
    hwc_sim::command::cl_move(u, &[nag], hwc_sim::command::waypoint_mode::ONCE, at(-30_000.0, 0.0), &[to]);
    g.world.run_mut().set_var(None, "G_NaggarokEMP", 20);
    for _ in 0..16 * 6 {
        rig.frame(g);
    }
    look(g, nag, 9_000.0);
    for _ in 0..4 {
        rig.frame(g);
    }
    // (It only moves under its mission's script: the shot gives it a speed.)
    g.world.universe_mut().ships.ship_mut(nag).posinfo.velocity = hwc_math::Vector::new(0.0, 0.0, 200.0);
    rig.shot(g, "33_fleet_naggarok_emp");
    for _ in 0..6 {
        rig.frame(g);
        g.world.universe_mut().ships.ship_mut(nag).posinfo.velocity = hwc_math::Vector::new(0.0, 0.0, 200.0);
    }
    rig.shot(g, "33b_fleet_naggarok_emp_later");

}

/// A Beast player on screen, which the campaign never shows: the Mission 1 player turned Beast,
/// with a Beast mothership and some of its fleet, and its build, research and launch managers.
#[test]
#[ignore]
fn beast_player_look() {
    use hwc_sim::statics::{ShipRace, ShipType as T};
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    let mut frames = 0;
    while g.ui.fleet_intel || frames < 16 * 3 {
        if g.ui.fleet_intel && frames % 16 == 0 {
            rig.key(g, 0x1b);
        }
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "the opening intel never ended");
    }
    let player = crate::host::PLAYER;
    let u = g.world.universe_mut();
    let own: Vec<_> = u.ships.iter().filter(|s| s.player == player).map(|s| s.id).collect();
    let p = own.first().and_then(|&id| u.ship(id)).map_or(hwc_math::Vector::ZERO, |s| s.posinfo.position);
    let at = |x: f32, y: f32| hwc_math::Vector::new(p.x + x, p.y + y, p.z);
    for id in own {
        hwc_sim::damage::really_delete_ship(u, id);
    }
    u.players[player].race = ShipRace::Beast;
    u.base.players[player].research_table = hwc_sim::base::ResearchTableKind::Beast;
    // As a multiplayer game sets a Beast player up: its own research table and lab.
    // (Mission 1's level file excludes the Beast's types; a skirmish level does not.)
    hwc_sim::level::flag_everything_needed(u);
    u.single_player = false;
    hwc_sim::research::init_player(u, player, 0);
    u.base.players[player].resource_units = 5000;
    let ms = hwc_sim::base::create_ship(u, T::bMothership, player, at(0.0, 0.0), Default::default()).expect("mothership");
    u.base.players[player].mothership = Some(ms);
    for (i, t) in [T::bCarrier, T::bHeavyCruiser, T::bRammingFrigate, T::bHiveFrigate, T::bIonArrayFrigate, T::bHeavyCorvette, T::bACV, T::bAcolyte, T::bWorker]
        .into_iter()
        .enumerate()
    {
        let x = 3000.0 + 1200.0 * i as f32;
        hwc_sim::base::create_ship(u, t, player, at(x, 1500.0 * ((i % 3) as f32 - 1.0)), Default::default()).expect("ship");
    }
    g.world.follow_focus = false;
    g.world.camera.look_at = Vec3::new(p.x + 3000.0, p.y, p.z);
    g.world.camera.distance = 16_000.0;
    for _ in 0..16 {
        rig.frame(g);
    }
    assert_eq!(g.world.universe().players[player].race, ShipRace::Beast);
    rig.shot(g, "40_beast_player_fleet");
    g.selection = vec![ms];
    rig.key(g, b'B' as u16);
    for _ in 0..4 {
        rig.frame(g);
    }
    let offered = hwc_sim::level::construction_list(g.world.universe(), ms);
    assert!(!offered.is_empty(), "the Beast mothership offers ships");
    rig.shot(g, "41_beast_player_build");
    rig.key(g, 27);
    rig.frame(g);
    rig.key(g, b'R' as u16);
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, "42_beast_player_research");
    rig.key(g, 27);
    rig.frame(g);
    rig.key(g, b'S' as u16);
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, "43_beast_player_launch");
}

#[test]
#[ignore]
fn mission_seven_slipgates() {
    use hwc_sim::ships::slipgate;
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut l = rig.loading(7, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    for _ in 0..16 {
        rig.frame(g);
    }
    let u = g.world.universe();
    let gates: Vec<_> = u
        .ships
        .iter()
        .filter(|s| slipgate::is_slipgate_type(s.ship_type))
        .map(|s| s.id)
        .collect();
    assert!(!gates.is_empty(), "mission 7 has no slipgates");
    for &id in &gates {
        let s = u.ship(id).expect("gate");
        assert_eq!(
            s.flags & hwc_sim::object::sof::SELECTABLE,
            0,
            "{id:?} selectable"
        );
        assert!(!crate::host::selectable(u, id));
        assert!(
            hwc_sim::effects::slot(u, id, 0x644).is_some(),
            "{id:?} ({:?}) has no effect",
            s.ship_type
        );
    }
    // A ship of the player's beside each gate, so the gates are seen (and drawn).
    {
        let u = g.world.universe_mut();
        let mothership = u.base.players[crate::host::PLAYER].mothership;
        let own: Vec<_> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| Some(id) != mothership && crate::host::selectable(u, id))
            .collect();
        for (&gate, &ship) in gates.iter().zip(&own) {
            let p = u.ship(gate).expect("gate").posinfo.position;
            let new = hwc_math::Vector::new(p.x + 6000.0, p.y, p.z);
            hwc_sim::base::set_ship_position(u, ship, new);
        }
    }
    let look = |g: &mut Game, id| {
        let p = g.world.universe().ship(id).expect("ship").posinfo.position;
        g.world.follow_focus = false;
        g.world.camera.look_at = Vec3::new(p.x, p.y, p.z);
        g.world.camera.distance = 30_000.0;
    };
    look(g, gates[0]);
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, "20_m7_slipgate_closed");
    // Wait for a gate to open.
    let mut opened = None;
    let mut handed = None;
    for _ in 0..16 * 900 {
        rig.frame(g);
        let u = g.world.universe();
        if handed.is_none() {
            handed = u
                .ship_list
                .iter()
                .copied()
                .find(|&id| u.ship(id).is_some_and(crate::host::handed_over));
        }
        if opened.is_none() {
            opened = gates
                .iter()
                .copied()
                .find(|&id| slipgate::state(u, id) != slipgate::gate_state::CLOSED);
            if let Some(gate) = opened {
                look(g, gate);
                g.world.camera.distance = 12_000.0;
                for i in 0..96 {
                    rig.frame(g);
                    if i % 16 == 15 {
                        rig.shot(g, &format!("21_m7_slipgate_opening_{}", i / 16));
                    }
                }
            }
        }
        if opened.is_some() && handed.is_some() {
            break;
        }
    }
    assert!(opened.is_some(), "no slipgate opened");
    if let Some(id) = handed {
        let u = g.world.universe();
        let s = u.ship(id).expect("ship");
        assert!(s.base.disable_reasons[1], "handed-over ship takes orders");
        assert!(!crate::host::selectable(u, id));
        eprintln!("handed over: {id:?} {:?}", s.ship_type);
        look(g, id);
        g.world.camera.distance = 20_000.0;
        for _ in 0..4 {
            rig.frame(g);
        }
        rig.shot(g, "22_m7_convoy_handed_over");
    } else {
        eprintln!("no convoy ship was handed over in the time allowed");
    }
}

/// Every saved game in the player's folder reads with this build (`--ignored`). Run it after any
/// change to saved state: nothing else tests files written by older builds.
#[test]
#[ignore]
fn the_players_saved_games_still_load() {
    let Some(mut rig) = rig() else { return };
    let dir = std::path::PathBuf::from(std::env::var("APPDATA").unwrap()).join("hwc-cataclysm/saves/SinglePlayer");
    let mut l = rig.loading(1, false);
    l.loader.play_nislets = false;
    let mut game = rig.load(l);
    let g = &mut *game;
    let statics = g.world.universe().statics.clone();
    let assets = crate::data::Assets(rig.data.clone());
    let (mut ok, mut bad) = (0, 0);
    let mut names: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).collect();
    names.sort();
    for path in names {
        let Ok(h) = hwc_sim::save::read_header(&path) else { println!("NOHEADER {}", path.display()); bad += 1; continue };
        let Ok(relink) = crate::saves::relink(&assets, statics.clone(), h.mission) else { println!("NORELINK {}", path.display()); bad += 1; continue };
        let raw = std::fs::read(&path).unwrap();
        let v = u32::from_le_bytes(raw[8..12].try_into().unwrap());
        match crate::saves::read(&path, &relink) {
            Ok((_, mut payload)) => {
                ok += 1;
                // And plays on: a few seconds of the simulation from where it was saved.
                payload.run.after_load();
                for _ in 0..16 * 5 {
                    payload.run.step();
                }
                println!("OK v{v} {}", path.file_name().unwrap().to_string_lossy());
            }
            Err(err) => { println!("BAD v{v} {}: {err}", path.file_name().unwrap().to_string_lossy()); bad += 1; }
        }
    }
    println!("saved games: {ok} load, {bad} do not");
    assert_eq!(bad, 0);
}

/// A skirmish started as Start Skirmish starts one (Coldwar, the player Beast against a
/// Somtaaw computer player): it loads, runs, and both fleets are where the map puts them.
#[test]
#[ignore]
fn skirmish_plays() {
    let Some(mut rig) = rig() else { return };
    let assets = crate::data::Assets(rig.data.clone());
    let mut options = hwc_sim::skirmish::game_types(&assets)
        .into_iter()
        .next()
        .expect("Default")
        .options();
    options.num_computers = 1;
    let req = hwc_ui::skirmish::SkirmishRequest {
        map: "Coldwar2".into(),
        options,
        race: 1,
        name: "Tester".into(),
    };
    let l = Loading::skirmish(rig.data.clone(), &rig.session, req, SIZE).expect("loading");
    let mut game = rig.load(l);
    let g = &mut *game;
    for _ in 0..16 * 4 {
        let out = rig.frame(g);
        assert!(out.is_empty(), "{out:?}");
    }
    let u = g.world.universe();
    assert!(u.mp.active);
    let ms0 = u.base.players[0].mothership.expect("our mothership");
    let ms1 = u.base.players[1].mothership.expect("their mothership");
    assert_eq!(u.ships.ship(ms0).ship_type, hwc_sim::statics::ShipType::bMothership);
    assert_eq!(u.ships.ship(ms1).ship_type, hwc_sim::statics::ShipType::sMothership);
    let ours = u.ship_list.iter().filter(|&&id| u.ships.ship(id).player == 0).count();
    println!("skirmish: {ours} ships of ours, {} RUs", u.base.players[0].resource_units);
    rig.shot(g, "40_skirmish_start");
    // The Beast player's build and research managers on a real skirmish level (not the
    // Mission 1 level turned Beast of `beast_player_look`).
    g.selection = vec![ms0];
    rig.key(g, b'B' as u16);
    for _ in 0..4 {
        rig.frame(g);
    }
    let offered = hwc_sim::level::construction_list(g.world.universe(), ms0);
    println!("the Beast mothership offers {} ships", offered.len());
    assert!(!offered.is_empty(), "the Beast mothership offers ships");
    rig.shot(g, "40a_skirmish_beast_build");
    rig.key(g, 27);
    rig.frame(g);
    rig.key(g, b'R' as u16);
    for _ in 0..4 {
        rig.frame(g);
    }
    rig.shot(g, "40b_skirmish_beast_research");
    rig.key(g, 27);
    rig.frame(g);
    let p = g.world.universe().ship(ms1).expect("ship").posinfo.position;
    g.world.follow_focus = false;
    g.world.camera.look_at = Vec3::new(p.x, p.y, p.z);
    g.world.camera.distance = 9000.0;
    for _ in 0..8 {
        rig.frame(g);
    }
    rig.shot(g, "41_skirmish_enemy");

    // A crate (`MG_CratesEnabled`) lies where it was put and is drawn with its derelict mesh.
    let u = g.world.universe_mut();
    let crate_at = hwc_math::Vector::new(p.x + 4000.0, p.y, p.z);
    let now = u.total_time_elapsed;
    u.mp.crates.list.push(hwc_sim::crates::Crate { position: crate_at, created: now });
    g.world.camera.look_at = Vec3::new(crate_at.x, crate_at.y, crate_at.z);
    g.world.camera.distance = 1500.0;
    for _ in 0..32 {
        rig.frame(g);
    }
    rig.shot(g, "41a_skirmish_crate");
    assert!(g.world.drawn_crates() > 0, "the crate is drawn");
    g.world.universe_mut().mp.crates.list.clear();

    // Hyperspace (on in the default game): the menu offers it for the mothership, the plate
    // opens in the sensors manager, and the click jumps it for its cost.
    g.selection = vec![ms0];
    g.world.universe_mut().base.players[0].resource_units = 100_000;
    rig.frame(g);
    assert!(g.ui.fe.game.ig.view.commands.contains(&hwc_ui::ingame::host::Command::Hyperspace));
    g.hyperspace_button(&mut None);
    assert!(g.sensors.is_open(), "the sensors manager opens for the jump");
    rig.frame(g);
    rig.shot(g, "41b_skirmish_hyperspace_plate");
    g.execute_plate(&mut None);
    let u = g.world.universe();
    let paid = 100_000 - u.base.players[0].resource_units;
    let cmd = u.ships.ship(ms0).command.and_then(|c| u.commands.get(c)).map(|c| c.order);
    println!("hyperspace: paid {paid}, order {cmd:?}");
    assert!(paid >= 2400, "at least the mothership's minimum cost");
    assert_eq!(cmd, Some(hwc_sim::command::order::MP_HYPERSPACE));
    for _ in 0..8 {
        rig.frame(g);
    }

    // Their mothership goes ("destroy enemy command ship" is on): they are out, we win, and
    // the game ends back at the menu.
    let u = g.world.universe_mut();
    u.ships.ship_mut(ms1).health = 0.0;
    hwc_sim::damage::add_ship_to_delete_list(u, ms1, -1);
    // The end shows the statistics (`gstatOpen`); Done goes back to the menu.
    for _ in 0..60 * 40 {
        rig.frame(g);
        if !g.end_stats.is_empty() {
            break;
        }
    }
    assert_eq!(g.end_stats.len(), 2, "the statistics show");
    assert!(g.end_stats[0].local && g.end_stats[0].values[7] >= 0);
    assert!(g.end_stats[0].values[0] > 0, "the time survived");
    rig.frame(g);
    rig.shot(g, "42_skirmish_statistics");
    g.ui.fe.game.ig.request(hwc_ui::ingame::host::GameRequest::StatsDone);
    let mut left = false;
    for _ in 0..10 {
        if rig.frame(g).contains(&Outcome::Leave) {
            left = true;
            break;
        }
    }
    let said: Vec<String> = g.messages.iter().map(|m| m.text.clone()).collect();
    println!("messages: {said:?}");
    assert!(said.iter().any(|m| m == "Computer 1 has died"), "{said:?}");
    assert!(said.iter().any(|m| m == "Tester has won"), "{said:?}");
    assert!(left, "the game ends");
}

/// A skirmish is saved (in the MultiPlayer folder, its mission number marking it a
/// skirmish) and loads back as the same game: the same ships, frame and players.
#[test]
#[ignore]
fn skirmish_saves_and_loads() {
    let Some(mut rig) = rig() else { return };
    let assets = crate::data::Assets(rig.data.clone());
    let mut options = hwc_sim::skirmish::game_types(&assets).into_iter().next().expect("Default").options();
    options.num_computers = 1;
    let req = hwc_ui::skirmish::SkirmishRequest { map: "Coldwar2".into(), options, race: 1, name: "Tester".into() };
    let l = Loading::skirmish(rig.data.clone(), &rig.session, req, SIZE).expect("loading");
    let mut game = rig.load(l);
    let g = &mut *game;
    for _ in 0..16 * 4 {
        rig.frame(g);
    }
    let dir = std::env::temp_dir().join(format!("hwc-skirmish-save-{}", std::process::id()));
    let saves = Rc::new(crate::saves::SaveDir::new(&dir));
    g.set_saves(saves.clone());
    assert!(g.can_save().is_ok());
    let path = g.save_named("Skirmish Test").expect("saved");
    assert!(path.starts_with(dir.join("MultiPlayer")), "{}", path.display());
    let h = hwc_sim::save::read_header(&path).expect("header");
    assert_eq!(h.mission, crate::saves::SKIRMISH_MISSION);
    // The skirmish load screen's list (`Skirmish_Load`) shows it.
    let listed = hwc_ui::saves::SaveStore::list(&*saves, hwc_ui::saves::SaveFolder::Multiplayer);
    assert_eq!(listed.len(), 1);
    assert!(listed[0].detail.starts_with("SKIRMISH"), "{}", listed[0].detail);
    let (ships, frame, players) = {
        let u = g.world.universe();
        (u.ship_list.len(), u.update_counter, u.mp.num_players)
    };
    drop(game);
    let l = Loading::from_save(rig.data.clone(), &rig.session, &path, SIZE).expect("loading the save");
    let mut game = rig.load(l);
    let u = game.world.universe();
    println!("saved {ships} ships at frame {frame}; loaded {} at {}", u.ship_list.len(), u.update_counter);
    assert!(u.mp.active, "a multiplayer game");
    assert_eq!(u.mp.num_players, players);
    assert_eq!(u.ship_list.len(), ships);
    assert_eq!(u.update_counter, frame);
    for _ in 0..16 {
        rig.frame(&mut game);
    }
    assert!(game.world.universe().update_counter > frame, "and it runs on");
    let _ = std::fs::remove_dir_all(&dir);
}

/// A recorded game (`gpSaveTheRecordedGame`, `gpLoadTheRecordedGame`): recording saves the
/// game and keeps every order from then on; played back, the same game unfolds (the same
/// state, update for update) with the player's own orders shut out.
#[test]
#[ignore]
fn a_recorded_game_plays_back() {
    use hwc_sim::orders::Order;
    let Some(mut rig) = rig() else { return };
    let assets = crate::data::Assets(rig.data.clone());
    let mut options = hwc_sim::skirmish::game_types(&assets).into_iter().next().expect("Default").options();
    options.num_computers = 1;
    let req = hwc_ui::skirmish::SkirmishRequest { map: "Coldwar2".into(), options, race: 1, name: "Tester".into() };
    let l = Loading::skirmish(rig.data.clone(), &rig.session, req, SIZE).expect("loading");
    let mut game = rig.load(l);
    let g = &mut *game;
    for _ in 0..16 * 2 {
        rig.frame(g);
    }
    let dir = std::env::temp_dir().join(format!("hwc-recording-{}", std::process::id()));
    let saves = Rc::new(crate::saves::SaveDir::new(&dir));
    g.set_saves(saves.clone());
    let save = g.start_recording("Rec Test").expect("recording");
    assert!(save.starts_with(dir.join("RecordedGames")));
    // Orders while recording: the player's ships move off.
    let u = g.world.universe();
    let ours: Vec<_> = u.ship_list.iter().copied().filter(|&id| u.ships.ship(id).player == crate::host::player()).collect();
    let from = u.ships.ship(ours[0]).posinfo.position;
    for k in 0..3 {
        for _ in 0..16 {
            rig.frame(g);
        }
        let to = from + hwc_math::Vector::new(3000.0 * (k + 1) as f32, 1000.0, 0.0);
        assert!(g.give(Order::Move { ships: ours.clone(), mode: 0, from, waypoints: vec![to] }));
    }
    for _ in 0..16 * 4 {
        rig.frame(g);
    }
    let target = g.world.universe().update_counter;
    let sum = hwc_sim::orders::sync_checksum(g.world.universe());
    let recorded = g.recording.as_ref().map_or(0, |r| r.recording.events.len());
    g.finish_recording();
    assert_eq!(recorded, 3, "the three orders");
    assert!(crate::saves::recording_path(&save).exists());
    drop(game);

    let mut l = Loading::from_save(rig.data.clone(), &rig.session, &save, SIZE).expect("loading the recording");
    l.playback = true;
    let mut game = rig.load(l);
    assert!(game.playback.is_some());
    // The player's orders are shut out during playback.
    assert!(!game.give(Order::Halt { ships: ours.clone() }));
    while game.world.universe().update_counter < target {
        rig.frame(&mut game);
    }
    let u = game.world.universe();
    println!("played back to update {} (recorded to {target})", u.update_counter);
    assert_eq!(u.update_counter, target);
    assert_eq!(hwc_sim::orders::sync_checksum(u), sum, "the same game");
    let _ = std::fs::remove_dir_all(&dir);
}
