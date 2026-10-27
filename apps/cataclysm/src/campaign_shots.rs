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
        panic!("loading never finished");
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
    // `--draw-distance far`: drawn within the far clip distance of the eye, by radial distance.
    g.world.render_list = false;
    for dir in [
        along,
        along * turn.cos() + (-side - up).normalize() * turn.sin(),
    ] {
        let eye = centre - dir * 14500.0;
        g.world.camera.set_eye_look_up(eye, centre, None);
        let drawn = g
            .world
            .frame((SIZE.0 as u32, SIZE.1 as u32))
            .ship_levels()
            .iter()
            .any(|&(id, _)| id == far);
        let reach = (target - eye).length();
        assert_eq!(
            drawn,
            reach < g.world.camera.tweaks.clip_far,
            "far mode at {reach}"
        );
    }
    g.world.render_list = true;
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
