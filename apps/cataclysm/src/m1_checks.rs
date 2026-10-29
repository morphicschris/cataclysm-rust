//! Game-data checks of Mission 1 from the user's play reports: the letterbox sliding in for
//! the KAS fleet-intel scenes (over the sensors manager too), and the sensors manager's frame
//! time with the per-section profile. Needs the game install (`HWC_GAME_DIR`) and a GPU
//! adapter; skips otherwise. Screenshots go to `screenshots/game5/`. Slow in a debug build:
//! run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::WideScreenKind;
use hwc_ui::canvas::Canvas;
use hwc_ui::letterbox::{bar_height, tall_bar_height};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};
use crate::sensors::Phase;

const DT: f32 = 1.0 / 16.0;

fn save(canvas: &Canvas, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/game5");
    std::fs::create_dir_all(&dir).expect("screenshot dir");
    canvas
        .save_png(&dir.join(format!("{name}.png")))
        .expect("png");
}

pub struct Rig {
    pub renderer: Renderer,
    pub audio: Option<crate::audio::Audio>,
    pub size: (i32, i32),
}

/// Mission 1 at `size` without its opening NIS; `nislets` plays the NISlets it asks for.
pub fn mission_one(size: (i32, i32), nislets: bool) -> Option<(Box<Game>, Rig)> {
    mission_one_with(size, nislets, false)
}

/// Mission 1 at `size`; `opening` plays the opening NIS first, `nislets` the NISlets.
pub fn mission_one_with(
    size: (i32, i32),
    nislets: bool,
    opening: bool,
) -> Option<(Box<Game>, Rig)> {
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
    session.resize(size.0 as u32, size.1 as u32);
    let audio = GameData::open(&dir)
        .map_err(|e| e.to_string())
        .and_then(|g| crate::audio::Audio::open(g, false))
        .ok();
    let mut loading = Loading::new(data.clone(), &session, 1, 2, size).ok()?;
    loading.opening_nis = opening;
    loading.loader.play_nislets = nislets;
    let load = |mut loading: Loading, renderer: &mut Renderer| {
        for _ in 0..40 {
            match loading.advance(&session, Some(&mut *renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
            }
        }
        panic!("loading never finished");
    };
    let mut game = load(loading, &mut renderer);
    let mut rig = Rig {
        renderer,
        audio,
        size,
    };
    if game.world.nis_level().is_some() {
        // The opening NIS plays in its own level, drawn every frame; then the mission's own
        // level loads with the fleet it held.
        let mut frames = 0;
        while !game
            .update(DT, &mut rig.audio)
            .contains(&Outcome::NisLevelDone)
        {
            rig.render(&mut game);
            frames += 1;
            assert!(frames < 16 * 600, "the opening NIS did not end");
        }
        let skill = game.world.skill();
        let crate::world::Sim::NisLevel(nis) = game.world.into_sim() else {
            panic!("not an NIS level");
        };
        let mut l = Loading::after_nis_level(data, &session, *nis, skill, size).ok()?;
        l.loader.play_nislets = nislets;
        game = load(l, &mut rig.renderer);
    }
    Some((game, rig))
}

impl Rig {
    pub fn frame(&mut self, g: &mut Game) {
        let out = g.update(DT, &mut self.audio);
        for o in &out {
            if let Outcome::Animatic(_) = o {
                g.movie_finished();
            }
        }
        assert!(
            !out.contains(&Outcome::NextMission),
            "mission 1 ended: {out:?}"
        );
    }

    pub fn render(&mut self, g: &mut Game) {
        g.render(&mut self.renderer, self.size).expect("render");
    }
}

/// Every pixel of the rectangle `x0..x1` x `y0..y1` is `c`.
fn rect_is(c: &Canvas, (x0, x1): (i32, i32), (y0, y1): (i32, i32), colour: [u8; 4]) -> bool {
    (y0..y1).all(|y| (x0..x1).all(|x| c.get(x, y) == colour))
}

/// The KAS fleet-intel scenes of Mission 1 (`wideScreenInDontCloseSensors(90)` before the
/// sensors manager opens on the threat, `wideScreenOut(90)` when the speech ends): each call
/// slides the bars in over its 90 frames (3 s) and out again, with the sensors manager left
/// open, and the bars are drawn over the sensors view. The NISlet's bars at the start stay
/// until the first scene's `wideScreenOut` (they must not get stuck), and the bars of the
/// incoming-fighters scene go when it ends. Screenshots: `m1_intel_letterbox_sliding`,
/// `m1_intel_letterbox_over_sensors`.
#[test]
fn letterbox_slides_in_for_fleet_intel_over_the_sensors() {
    let size = (1600, 900);
    let Some((mut game, mut rig)) = mission_one(size, true) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    // The tall bars are the default (a user-requested change): 126 pixels at 1600x900, where
    // the original's 1.85:1 picture leaves 18.
    assert!(g.letterbox.tall);
    let full = tall_bar_height(size.0, size.1);
    assert_eq!((full, bar_height(size.0, size.1)), (126, 18));
    let mut applied = 0;
    // The last call coming in found the bars out: frame it was applied.
    let mut sliding_from: Option<(usize, WideScreenKind, bool)> = None;
    let mut slides = 0;
    let mut slid_over_sensors = false;
    let mut sliding_shot = false;
    let mut sensors_shot = false;
    let mut nislet_bars_gone = false;
    for frame in 0..16 * 900 {
        let was_in = g.letterbox.fully;
        rig.frame(g);
        for (e, sensors_open) in &g.wide_screen_log[applied..] {
            eprintln!(
                "{:7.2}s frame {frame:5}: {:?}({}) sensors open {sensors_open}, bars were in {was_in}",
                e.time, e.kind, e.frames
            );
            assert_eq!(e.frames, 90, "mission 1's calls all use 90 frames");
            if e.kind == WideScreenKind::Out {
                nislet_bars_gone = true;
                sliding_from = None;
            } else if !was_in {
                sliding_from = Some((frame, e.kind, *sensors_open));
            }
        }
        applied = g.wide_screen_log.len();
        if let Some((from, kind, sensors_were_open)) = sliding_from {
            let t = (frame - from) as f32 * DT;
            if g.letterbox.fully {
                eprintln!("  bars in after {t:.2} s");
                assert!((2.9..3.4).contains(&t), "the bars took {t} s to come in");
                slides += 1;
                sliding_from = None;
                // No wide-screen call closes the sensors manager (Cataclysm's
                // `kasfWideScreenIn` has the body of `...InDontCloseSensors`).
                if sensors_were_open {
                    assert!(g.sensors.is_open(), "{kind:?} closed the sensors manager");
                }
            } else {
                assert!(
                    g.letterbox.fade > 0.0 || frame == from,
                    "the bars are not moving {t} s after {kind:?}"
                );
                if !sliding_shot && g.letterbox.fade > 0.45 && g.letterbox.fade < 0.6 {
                    rig.render(g);
                    let c = g.canvas();
                    let y = (full as f32 * g.letterbox.fade) as i32;
                    assert!(y > 2, "{y}");
                    // Grey under black of alpha `fade` while moving.
                    let p = c.get(c.width - 10, 2);
                    assert!(
                        p[0] > 30 && p[0] < 80 && p[0] == p[1] && p[1] == p[2],
                        "{p:?}"
                    );
                    save(c, "m1_intel_letterbox_sliding");
                    sliding_shot = true;
                }
            }
        }
        let in_sensors = g.sensors.phase() == Phase::Sensors && g.nis.is_none();
        if in_sensors && g.letterbox.active() && nislet_bars_gone {
            slid_over_sensors = true;
            if g.letterbox.fully && !sensors_shot {
                // Let the speech band settle, then the shot.
                for _ in 0..16 {
                    rig.frame(g);
                }
                if g.letterbox.fully && g.sensors.phase() == Phase::Sensors {
                    rig.render(g);
                    let c = g.canvas();
                    save(c, "m1_intel_letterbox_over_sensors");
                    // The speech goes over the top bar (its letterbox region), so only the
                    // right end of that one is checked.
                    let black = [0, 0, 0, 255];
                    assert!(
                        rect_is(c, (size.0 - 60, size.0), (0, full), black)
                            && rect_is(c, (0, size.0), (size.1 - full, size.1), black),
                        "the bars are not drawn over the sensors view"
                    );
                    assert!(
                        !rect_is(c, (0, size.0), (full, full + 1), black),
                        "the bars are taller than {full}"
                    );
                    sensors_shot = true;
                }
            }
        }
        if slides >= 1 && sensors_shot && sliding_shot && !g.letterbox.active() {
            break;
        }
    }
    assert!(
        nislet_bars_gone,
        "no wideScreenOut ever took the NISlet's bars away"
    );
    // The first scene keeps the NISlet's bars; the incoming fighters (356 s in) bring them in.
    assert!(
        slides >= 1,
        "the bars never slid in for a fleet-intel scene"
    );
    assert!(!g.letterbox.active(), "the bars stayed after the scene");
    assert!(
        slid_over_sensors,
        "the bars were never up over the sensors view"
    );
    assert!(sliding_shot && sensors_shot);
}

/// One profiled stretch of frames: `n` frames of update, render and the canvas upload (as
/// the window does it), with the profile's averages. With `sensors`, only frames with the
/// sensors manager fully open count (the mission's scripts close it now and then: it is
/// opened again, unprofiled).
fn profiled_frames(
    g: &mut Game,
    rig: &mut Rig,
    presenter: &mut hwc_ui::present::Presenter,
    n: usize,
    sensors: bool,
) -> crate::profile::Report {
    let (device, queue) = (rig.renderer.device().clone(), rig.renderer.queue().clone());
    let _ = crate::profile::report_now();
    let mut done = 0;
    let mut guard = 0;
    while done < n {
        guard += 1;
        assert!(guard < n * 20, "the sensors manager would not stay open");
        if sensors && (g.sensors.phase() != Phase::Sensors || g.sensors.zooming()) {
            crate::profile::disable();
            if !g.sensors.is_open() {
                g.toggle_sensors(&mut rig.audio);
            }
            rig.frame(g);
            rig.render(g);
            crate::profile::enable();
            continue;
        }
        done += 1;
        crate::profile::frame_start();
        let span = crate::profile::span("update");
        let out = g.update(DT, &mut rig.audio);
        drop(span);
        assert!(!out.contains(&Outcome::NextMission));
        let span = crate::profile::span("render");
        g.render(&mut rig.renderer, rig.size).expect("render");
        drop(span);
        let span = crate::profile::span("upload");
        let c = g.canvas();
        let rgba = c.pixels.as_flattened();
        crate::profile::count("upload_bytes", rgba.len() as u64);
        presenter.upload_rgba(&device, &queue, (c.width as u32, c.height as u32), rgba);
        // Wait for the copy, as presenting the frame would.
        queue.submit([]);
        let _ = device.poll(hwc_render::wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        drop(span);
        let _ = crate::profile::frame_end();
    }
    crate::profile::report_now()
}

/// The sensors manager's frame time in Mission 1 at 1600x900 and 1920x1080 with the frame
/// profile (`--profile`): CPU time per step, GPU time per pass, counts. Prints the reports;
/// asserts the sensors frame stays under 16 ms on average. `--ignored --nocapture`, release.
#[test]
#[ignore]
fn sensors_manager_profile() {
    crate::profile::enable();
    for size in [(1600, 900), (1920, 1080)] {
        let Some((mut game, mut rig)) = mission_one(size, false) else {
            eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
            return;
        };
        let timed = rig.renderer.set_gpu_timing(true);
        eprintln!(
            "{}x{}: GPU timestamp queries {}",
            size.0,
            size.1,
            if timed { "on" } else { "not supported" }
        );
        let device = rig.renderer.device().clone();
        let mut presenter = hwc_ui::present::Presenter::new(
            &device,
            hwc_render::wgpu::TextureFormat::Rgba8Unorm,
            hwc_ui::present::Filter::Nearest,
        );
        let g = &mut *game;
        // The fleet in, the frigates and fighters out (and clear of the scripted intel
        // scenes, which open and close the manager themselves).
        for _ in 0..16 * 160 {
            rig.frame(g);
        }
        // Warm up both views, then measure.
        if g.sensors.is_open() {
            g.toggle_sensors(&mut rig.audio);
        }
        for _ in 0..40 {
            rig.frame(g);
            rig.render(g);
        }
        assert_eq!(g.sensors.phase(), Phase::Main);
        let main = profiled_frames(g, &mut rig, &mut presenter, 64, false);
        g.toggle_sensors(&mut rig.audio);
        // Through the zoom into the manager.
        for _ in 0..200 {
            rig.frame(g);
            rig.render(g);
            if g.sensors.phase() == Phase::Sensors && !g.sensors.zooming() {
                break;
            }
        }
        for _ in 0..8 {
            rig.frame(g);
            rig.render(g);
        }
        assert_eq!(
            g.sensors.phase(),
            Phase::Sensors,
            "the sensors manager did not open"
        );
        // The old way first: discs and dimming on the CPU, meshes read back whole.
        g.sensors_cpu_layer = true;
        let cpu = profiled_frames(g, &mut rig, &mut presenter, 64, true);
        g.sensors_cpu_layer = false;
        let sensors = profiled_frames(g, &mut rig, &mut presenter, 64, true);
        save(g.canvas(), &format!("m1_sensors_{}x{}", size.0, size.1));
        eprintln!(
            "== {}x{} sensors manager, CPU discs (before)
{}",
            size.0,
            size.1,
            cpu.text()
        );
        eprintln!("== {}x{} main view\n{}", size.0, size.1, main.text());
        eprintln!(
            "== {}x{} sensors manager\n{}",
            size.0,
            size.1,
            sensors.text()
        );
        let frame = sensors.cpu_ms("frame").unwrap_or(f64::MAX);
        assert!(
            frame < 16.0,
            "the sensors frame takes {frame:.1} ms at {size:?}"
        );
    }
}

/// The sensors manager's backdrop dimming and range discs drawn on the GPU give the picture
/// the CPU canvas gave, to within one step of 8-bit rounding, in Mission 1 at 1600x900 with
/// the fleet's discs overlapping. Writes `m1_sensors_gpu_layer` and `m1_sensors_cpu_layer`.
#[test]
fn sensors_gpu_layer_matches_the_cpu_canvas() {
    let size = (1600, 900);
    let Some((mut game, mut rig)) = mission_one(size, false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    for _ in 0..16 * 140 {
        rig.frame(g);
    }
    if !g.sensors.is_open() {
        g.toggle_sensors(&mut rig.audio);
    }
    for _ in 0..40 {
        rig.frame(g);
    }
    assert_eq!(
        g.sensors.phase(),
        Phase::Sensors,
        "the sensors manager did not open"
    );
    // A frame first: a speech card expiring would change the second picture.
    rig.render(g);
    rig.render(g);
    let mut pictures = Vec::new();
    for (cpu, background) in [(false, true), (true, true), (false, false), (true, false)] {
        g.sensors_cpu_layer = cpu;
        g.sensors.toggles.background = background;
        rig.render(g);
        pictures.push(g.canvas().clone());
    }
    g.sensors_cpu_layer = false;
    g.sensors.toggles.background = true;
    save(&pictures[0], "m1_sensors_gpu_layer");
    save(&pictures[1], "m1_sensors_cpu_layer");
    for (gpu, cpu, what) in [
        (&pictures[0], &pictures[1], "backdrop"),
        (&pictures[2], &pictures[3], "clear colour"),
    ] {
        let mut differ = 0;
        let mut worst = 0;
        let mut blue = 0;
        for (a, b) in gpu.pixels.iter().zip(&cpu.pixels) {
            let d = (0..4).map(|i| a[i].abs_diff(b[i])).max().unwrap_or(0);
            differ += usize::from(d > 0);
            worst = worst.max(d);
            blue += usize::from(b[2] > b[0] + 40);
        }
        eprintln!(
            "{what}: {differ} of {} pixels differ, by at most {worst}; {blue} disc pixels",
            cpu.pixels.len()
        );
        assert!(blue > 10_000, "no range discs in the picture");
        assert!(
            worst <= 1,
            "the GPU layer is {worst} steps off the CPU canvas ({what})"
        );
    }
}

/// The Kuun-Lan in the sensors manager in Mission 1, closed onto: its modules are drawn with
/// it, as `smShipMeshDraw` draws a ship's clamped slaves. Writes `m1_sensors_kuun_lan`.
#[test]
fn sensors_draw_the_kuun_lan_with_its_modules() {
    let Some((mut game, mut rig)) = mission_one((1600, 900), false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    for _ in 0..16 * 140 {
        rig.frame(g);
    }
    let ms = g.world.universe().base.players[0]
        .mothership
        .expect("mothership");
    let drawn = crate::sensors::Sensors::with_clamped(g.world.universe(), &[ms]);
    eprintln!("the Kuun-Lan and {} modules", drawn.len() - 1);
    assert!(drawn.len() > 1, "no modules clamped to the Kuun-Lan");
    if !g.sensors.is_open() {
        g.toggle_sensors(&mut rig.audio);
    }
    for _ in 0..40 {
        rig.frame(g);
    }
    assert_eq!(g.sensors.phase(), Phase::Sensors);
    g.sensors
        .close_on(&g.world, &[ms])
        .expect("focus on the Kuun-Lan");
    for _ in 0..40 {
        rig.frame(g);
    }
    rig.render(g);
    save(g.canvas(), "m1_sensors_kuun_lan");
}

/// The Kuun-Lan half out of its hyperspace window in Mission 1 at 1600x900, seen from the
/// side: its nav lights show only on the part that has come out (they are cut by the window's
/// plane, as the hull is). Screenshots: `m1_arrival_nav_lights_{0..3}`. The plane cut itself is
/// checked in `hwc-render`'s `nav_lights_follow_the_hyperspace_clip`.
#[test]
fn kuun_lan_arrival_nav_lights() {
    use hwc_render::effects::hs_state::SLICING_OUTOF;
    let size = (1600, 900);
    let Some((mut game, mut rig)) = mission_one(size, false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    for _ in 0..16 * 60 {
        rig.frame(g);
        let ms = g.world.universe().base.players[0].mothership;
        let window = g
            .world
            .run()
            .hyperspace_windows()
            .into_iter()
            .find(|&(id, state, clipt)| {
                Some(id) == ms && state == SLICING_OUTOF && clipt > 0.45 && clipt < 0.6
            });
        let Some((id, _, _)) = window else { continue };
        let s = g.world.universe().ship(id).expect("mothership");
        let p = s.posinfo.position;
        let heading = s.rotinfo.coordsys.col(2);
        g.world.follow_focus = false;
        g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
        g.world.camera.distance = 4500.0;
        for k in 0..4 {
            g.world.camera.angle =
                heading.y.atan2(heading.x) + 0.6 + k as f32 * std::f32::consts::FRAC_PI_2;
            g.world.camera.declination = -0.3;
            rig.render(g);
            save(g.canvas(), &format!("m1_arrival_nav_lights_{k}"));
        }
        return;
    }
    panic!("the Kuun-Lan never came half out of its hyperspace window");
}

/// The messages that start Mission 1's other KAS fleet-intel scenes (sent to `FleetIntel` by
/// the mission's other teams as the battle goes): the Taiidan destroyer, the proximity
/// sensors under attack, the resource collectors, the flank frigates.
const SCENE_MESSAGES: [(u32, &str); 4] = [
    (0x0f92dd92, "destroyer"),
    (0x0f19cfe7, "proximity sensors"),
    (0x04d89963, "resourcers"),
    (0x0939002e, "flank frigates"),
];

/// One letterboxed fleet-intel scene: when its bars came in and went, why the intel event
/// that took them away ended, and when its last subtitle or location card ran out.
#[derive(Debug)]
struct Scene {
    /// None for the opening's scene, which keeps the NISlet's bars.
    bars_in: Option<f32>,
    bars_out: f32,
    reason: Option<hwc_sim::mission::IntelEndReason>,
    /// When the last speech subtitle or location card shown before the bars went expires
    /// (`subTitleShortest` at least, as `subCreateNonScrollyLines` makes it).
    cards_out: Option<f32>,
}

/// Mission 1 played as in a game (the opening NIS, the NISlets, speech with subtitles drawn
/// every frame), with the KAS scenes the opening does not reach started by their messages
/// once the script waits for one: its scenes from the trace.
fn mission_one_intel_scenes() -> Option<Vec<Scene>> {
    use crate::intel_trace::IntelTraceKind as K;
    use hwc_kas::kas_hash;
    let (mut game, mut rig) = mission_one_with((640, 480), true, true)?;
    let g = &mut *game;
    let shortest = g
        .subtitles
        .as_ref()
        .map_or(0.0, |s| s.script.title_shortest);
    let mut next = 0;
    let mut idle = 0;
    for _ in 0..16 * 60 * 16 {
        rig.frame(g);
        rig.render(g);
        let run = g.world.run();
        let now = run.universe.total_time_elapsed;
        let waiting = run.team_state("FleetIntel") == Some(kas_hash("WAITMessage"))
            && !g.letterbox.active()
            && !g.letterbox.fully
            && !g.subtitles.as_ref().is_some_and(|s| s.any_cards());
        idle = if waiting { idle + 1 } else { 0 };
        // After the opening's scenes (the raiders come at about 240 s), a few seconds idle.
        if now > 300.0 && idle > 16 * 5 {
            let Some(&(msg, what)) = SCENE_MESSAGES.get(next) else {
                break;
            };
            eprintln!("{now:8.2}s sending the {what} scene's message");
            g.world
                .run_mut()
                .kas
                .post_message(kas_hash("FleetIntel"), None, msg);
            next += 1;
            idle = 0;
        }
    }
    assert_eq!(
        next,
        SCENE_MESSAGES.len(),
        "the script never waited for every scene"
    );
    let mut scenes = Vec::new();
    let mut bars_in = None;
    let mut reason = None;
    let mut cards_out = None;
    for e in &g.intel_trace.entries {
        eprintln!("{:8.2}s {:?}", e.time, e.kind);
        match e.kind {
            K::Speech { seconds, .. } | K::LocationCard { seconds } => {
                cards_out = Some(e.time + seconds.max(shortest));
            }
            K::IntelEnded(r) => reason = Some(r),
            K::WideScreen { kind, .. } if kind != WideScreenKind::Out => {
                bars_in.get_or_insert(e.time);
            }
            K::WideScreen { .. } => scenes.push(Scene {
                bars_in: bars_in.take(),
                bars_out: e.time,
                reason: reason.take(),
                cards_out,
            }),
            _ => {}
        }
    }
    Some(scenes)
}

/// Mission 1's KAS fleet-intel scenes, the ones that bring up the sensors manager too: the
/// bars of each go (`wideScreenOut`) when its last subtitle or location card runs out, as in
/// the original, where every subtitle region running out of cards ends the intel event. None
/// waits for `SUBMESSAGE_SAFETY_TIMEOUT` (the raiders' and the resource collectors' scenes
/// end on a location card, which HW1's letterbox-only check missed). Release.
#[test]
fn intel_scenes_lose_their_bars_with_their_subtitles() {
    let Some(scenes) = mission_one_intel_scenes() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    for s in &scenes {
        let late = s.cards_out.map(|c| s.bars_out - c);
        eprintln!(
            "bars in {:?} out {:7.2}s, cards out {:?}, {:?}, late {late:?}",
            s.bars_in, s.bars_out, s.cards_out, s.reason
        );
    }
    // The opening (the NISlet's bars to the bombers), the raiders, and the four sent.
    assert!(scenes.len() >= 6, "{} scenes", scenes.len());
    assert!(scenes.iter().skip(1).all(|s| s.bars_in.is_some()));
    for s in &scenes {
        assert_eq!(
            s.reason,
            Some(hwc_sim::mission::IntelEndReason::Reported),
            "the scene to {:.2} s ended on {:?}",
            s.bars_out,
            s.reason
        );
        // The mission's KAS runs once every 16 updates (`SP_KAS_UPDATE_FRAME`): up to a
        // second after the card goes, as in the original.
        let late = s.bars_out - s.cards_out.expect("no speech in the scene");
        assert!(
            (0.0..=1.0 + DT).contains(&late),
            "the bars at {:.2} s went {late:.2} s after the scene's last card",
            s.bars_out
        );
    }
}

/// Time compression as in Cataclysm (`taskIncreaseTimeCompression` 0x525520 and the rest):
/// the backslash toggles normal speed and 8x, Ctrl+= / Ctrl+- step through 1/4..8x, each
/// change says "<n> X Time Compression", and at 8x the universe runs 8 updates a frame.
#[test]
fn time_compression_keys() {
    use hwc_ui::ingame::InputEvent;
    let Some((mut game, mut rig)) = mission_one((800, 600), false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let key = |g: &mut Game, rig: &mut Rig, codes: &[u16]| {
        for &c in codes {
            g.input(InputEvent::KeyDown(c), &mut rig.audio);
        }
        for &c in codes.iter().rev() {
            g.input(InputEvent::KeyUp(c), &mut rig.audio);
        }
        rig.frame(g);
    };
    // Past the opening fleet intel (it holds the keyboard; Escape skips it).
    let mut frames = 0;
    while g.ui.fleet_intel || frames < 16 * 20 {
        if g.ui.fleet_intel && frames % 16 == 0 {
            key(g, &mut rig, &[0x1b]);
        }
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "the opening intel never ended");
    }
    let last = |g: &Game| {
        g.messages
            .last()
            .map(|m| m.text.clone())
            .unwrap_or_default()
    };
    assert_eq!(g.speed_level, 2);
    key(g, &mut rig, &[220]);
    assert_eq!(g.speed_level, 6, "backslash goes to the fastest speed");
    assert_eq!(last(g), "8 X Time Compression");
    let t0 = g.world.universe().total_time_elapsed;
    for _ in 0..4 {
        rig.frame(g);
    }
    let dt = g.world.universe().total_time_elapsed - t0;
    eprintln!("4 frames of 1/16 s at 8x: {dt} s of game time");
    assert!(dt > 1.9, "8x ran only {dt} s of game time in 0.25 s");
    key(g, &mut rig, &[0x11, 189]);
    assert_eq!(g.speed_level, 5);
    assert_eq!(last(g), "6 X Time Compression");
    key(g, &mut rig, &[220]);
    assert_eq!(
        g.speed_level, 2,
        "backslash at any other speed goes back to normal"
    );
    assert_eq!(last(g), "1 X Time Compression");
    for _ in 0..3 {
        key(g, &mut rig, &[0x11, 189]);
    }
    assert_eq!(g.speed_level, 0, "slowest is 1/4");
    assert_eq!(last(g), "1/4 X Time Compression");
    key(g, &mut rig, &[0x11, 187]);
    assert_eq!(g.speed_level, 1);
    assert_eq!(last(g), "1/2 X Time Compression");
}

/// Orders are acknowledged by the ships as in the original: the first selected ship answers
/// a tactics change, an attack order and a cancelled attack, and only the local player's
/// ships speak.
#[test]
fn ships_acknowledge_orders() {
    use hwc_sim::speech::event;
    use hwc_ui::ingame::host::Command;
    use hwc_ui::ingame::InputEvent;
    let Some((mut game, mut rig)) = mission_one((800, 600), false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    if rig.audio.is_none() {
        eprintln!("skipped: no sound data");
        return;
    }
    let g = &mut *game;
    let mut frames = 0;
    while g.ui.fleet_intel || frames < 16 * 20 {
        if g.ui.fleet_intel && frames % 16 == 0 {
            g.input(InputEvent::KeyDown(0x1b), &mut rig.audio);
            g.input(InputEvent::KeyUp(0x1b), &mut rig.audio);
        }
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "the opening intel never ended");
    }
    // Let the intel's own lines finish.
    for _ in 0..16 * 30 {
        rig.frame(g);
    }
    let u = g.world.universe();
    let mothership = u.base.players[crate::host::PLAYER]
        .mothership
        .expect("mothership");
    let own: Vec<_> = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| id != mothership && crate::host::selectable(u, id))
        .collect();
    // The Kiithid fleet the script hands over (`SwitchOwner` without control, specialFlags2
    // 0x100000) neither takes orders nor speaks, as in the original.
    let (silent, own): (Vec<_>, Vec<_>) = own.into_iter().partition(|&id| {
        u.ship(id)
            .is_some_and(|s| s.special_flags2 & 0x10_0000 != 0)
    });
    let ships = hwc_sim::command::attack_capable(u, &own);
    assert!(
        !ships.is_empty(),
        "Mission 1 has no fighting ships of the player's own"
    );
    let ty = u.ship(ships[0]).expect("ship").ship_type;
    eprintln!("speaking ship: {ty:?}");
    let acolyte = ty.index() as i32;
    let speaking = |rig: &Rig| rig.audio.as_ref().and_then(|a| a.speaking(0));

    g.selection = ships.clone();
    g.command(Command::TacticsAggressive, &mut rig.audio);
    rig.frame(g);
    assert_eq!(speaking(&rig), Some((event::TACTICS_AGGRESSIVE, acolyte)));

    g.attack(&[mothership]);
    rig.frame(g);
    assert_eq!(speaking(&rig), Some((event::ATTACK, acolyte)));
    let attacking = g
        .world
        .universe()
        .ship(ships[0])
        .and_then(|s| s.command)
        .is_some();
    assert!(attacking, "the attack order was not given");

    g.command(Command::Cancel, &mut rig.audio);
    rig.frame(g);
    assert_eq!(speaking(&rig), Some((event::ATTACK_CANCELLED, acolyte)));

    // A ship of the handed-over fleet stays silent.
    let before = speaking(&rig);
    if let Some(&k) = silent.first() {
        hwc_sim::speech::by_ship(g.world.universe_mut(), k, event::MOVE_CANCELLED);
        rig.frame(g);
        assert_eq!(speaking(&rig), before);
    }

    // An enemy ship asked to speak stays silent.
    let before = speaking(&rig);
    let u = g.world.universe_mut();
    if let Some(enemy) = u
        .ship_list
        .iter()
        .copied()
        .find(|&id| u.ship(id).is_some_and(|s| s.player != crate::host::PLAYER))
    {
        hwc_sim::speech::by_ship(u, enemy, event::MOVE);
        rig.frame(g);
        assert_eq!(speaking(&rig), before);
    }
}

/// The kamikaze, scuttle and waypoint orders (`kbcbKamikaze`, `mrScuttle`, `mrWaypointMove*`):
/// waypoints are placed on the plate and flown in the chosen mode, the kamikaze key turns
/// attacking fighters into rammers, and scuttling takes a second press (never a mothership).
#[test]
fn kamikaze_scuttle_and_waypoint_orders() {
    use hwc_sim::object::special;
    use hwc_sim::speech::{event, Speech};
    use hwc_ui::ingame::host::Command;
    use hwc_ui::ingame::keys;
    use hwc_ui::ingame::InputEvent;
    let Some((mut game, mut rig)) = mission_one((800, 600), false) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    let mut frames = 0;
    while g.ui.fleet_intel || frames < 16 * 5 {
        if g.ui.fleet_intel && frames % 16 == 0 {
            g.input(InputEvent::KeyDown(0x1b), &mut rig.audio);
            g.input(InputEvent::KeyUp(0x1b), &mut rig.audio);
        }
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "the opening intel never ended");
    }
    // Three Acolytes of the player's own beside the mothership, and an enemy carrier ahead.
    let u = g.world.universe_mut();
    let mothership = u.base.players[crate::host::PLAYER]
        .mothership
        .expect("mothership");
    let p = u.ship(mothership).expect("mothership").posinfo.position;
    let mut make = |t: hwc_sim::statics::ShipType, player: usize, x: f32, y: f32| {
        let at = p + hwc_math::Vector::new(x, y, 0.0);
        let id = hwc_sim::base::create_ship(u, t, player, at, Default::default()).expect("ship");
        hwc_sim::construction::unit_cap_create_ship(u, id);
        id
    };
    use hwc_sim::statics::ShipType::{sAcolyte, sCarrier};
    let ships = vec![
        make(sAcolyte, 0, 3000.0, 0.0),
        make(sAcolyte, 0, 3000.0, 300.0),
        make(sAcolyte, 0, 3000.0, -300.0),
    ];
    let enemy = make(sCarrier, 1, 9000.0, 0.0);
    // Mission 1 still has its bars up here; the carrier may be hurt anyway
    // (`VULNERABLE_IN_WIDESCREEN`).
    u.ships.ship_mut(enemy).special_flags2 |= 0x0100_0000;
    let ty = |g: &Game, id| g.world.universe().ship(id).expect("ship").ship_type.index() as i32;
    let said = |g: &Game, e: u32| {
        g.world
            .universe()
            .speech
            .iter()
            .any(|s: &Speech| s.event == e)
    };

    // Waypoints: two points on the plate, the circular mode, then the order.
    g.selection = ships.clone();
    g.action(keys::WAYPOINTS, true, &mut rig.audio);
    assert!(g.plate().is_some(), "the waypoint plate did not come up");
    let start = g.plate().expect("plate").centre;
    for (k, off) in [
        glam::Vec3::new(3000.0, 0.0, 0.0),
        glam::Vec3::new(3000.0, 3000.0, 500.0),
    ]
    .into_iter()
    .enumerate()
    {
        if let crate::game::Mode::Move(p) = &mut g.mode {
            p.height = start + off;
            p.plane = glam::Vec3::new(p.height.x, p.height.y, start.z);
        }
        g.waypoint_add(&mut rig.audio);
        assert_eq!(g.placed_waypoints().map(|w| w.len()), Some(k + 1));
    }
    g.action(keys::CANCEL_LAST_WAYPOINT, true, &mut rig.audio);
    assert_eq!(g.placed_waypoints().map(|w| w.len()), Some(1));
    g.waypoint_add(&mut rig.audio);
    g.action(keys::TOGGLE_WAYPOINT_MODES, true, &mut rig.audio);
    assert_eq!(
        g.messages.last().map(|m| m.text.as_str()),
        Some("Waypoint Mode = Patrol Circular")
    );
    g.action(keys::WAYPOINTS, true, &mut rig.audio);
    assert!(
        g.plate().is_none(),
        "the plate stayed up after the waypoint order"
    );
    assert!(said(g, event::MOVE), "no ship answered the waypoint move");
    let u = g.world.universe();
    let c = u
        .ship(ships[0])
        .and_then(|s| s.command)
        .and_then(|c| u.commands.get(c))
        .expect("a move command");
    assert_eq!(c.order, hwc_sim::command::order::MOVE);
    assert_eq!(c.waypoint_mode, hwc_sim::command::waypoint_mode::LOOP);
    assert_eq!(c.waypoints.len(), 2);
    g.action(keys::WAYPOINT_MODE_NORMAL, true, &mut rig.audio);
    rig.frame(g);

    // Kamikaze: with no ship attacking the first says it has no target.
    g.selection = ships.clone();
    g.command(Command::Cancel, &mut rig.audio);
    rig.frame(g);
    g.command(Command::Kamikaze, &mut rig.audio);
    assert!(said(g, event::KAMIKAZE_NO_TARGETS), "no 'no targets' line");
    rig.frame(g);
    g.attack(&[enemy]);
    rig.frame(g);
    g.command(Command::Kamikaze, &mut rig.audio);
    assert!(
        g.world
            .universe()
            .speech
            .iter()
            .any(|s| s.event == event::KAMIKAZE && s.variable == ty(g, ships[0])),
        "the kamikaze order was not acknowledged"
    );
    for &id in &ships {
        let s = g.world.universe().ship(id).expect("ship");
        assert_ne!(
            s.special_flags & special::KAMIKAZE,
            0,
            "{id:?} did not turn kamikaze"
        );
    }
    for _ in 0..16 * 3 {
        rig.frame(g);
    }
    let running = ships.iter().any(|&id| {
        g.world
            .universe()
            .ship(id)
            .is_some_and(|s| s.kamikaze_state != 0)
    });
    assert!(running, "no kamikaze run started");
    // The first to reach the enemy rams it: it blows up and the carrier takes the damage.
    let health = g.world.universe().ship(enemy).expect("carrier").health;
    let mut t = 0;
    while ships
        .iter()
        .all(|&id| g.world.universe().ship(id).is_some())
    {
        rig.frame(g);
        t += 1;
        assert!(t < 16 * 60, "no kamikaze reached its target");
    }
    eprintln!("rammed after {:.1} s", t as f32 / 16.0);
    let after = g.world.universe().ship(enemy).map_or(0.0, |e| e.health);
    assert!(
        after < health,
        "the rammed carrier took no damage ({health} -> {after})"
    );
    // A fresh Acolyte told to kamikaze, then to stop, is a plain fighter again.
    let spare = {
        let u = g.world.universe_mut();
        let p = u.ship(mothership).expect("mothership").posinfo.position
            + hwc_math::Vector::new(3000.0, 0.0, 0.0);
        let id =
            hwc_sim::base::create_ship(u, sAcolyte, 0, p, Default::default()).expect("acolyte");
        hwc_sim::construction::unit_cap_create_ship(u, id);
        id
    };
    g.selection = vec![spare];
    g.attack(&[enemy]);
    rig.frame(g);
    g.command(Command::Kamikaze, &mut rig.audio);
    let flags = |g: &Game| g.world.universe().ship(spare).expect("spare").special_flags;
    assert_ne!(flags(g) & special::KAMIKAZE, 0);
    g.command(Command::Cancel, &mut rig.audio);
    rig.frame(g);
    assert_eq!(
        flags(g) & special::KAMIKAZE,
        0,
        "a halt keeps the kamikaze flag"
    );

    // Scuttle: the mothership never; a fighter on the second press.
    g.selection = vec![mothership];
    g.command(Command::Scuttle, &mut rig.audio);
    g.command(Command::Scuttle, &mut rig.audio);
    assert_eq!(
        g.world
            .universe()
            .ship(mothership)
            .expect("mothership")
            .death_time,
        0.0
    );
    rig.frame(g);
    let victim = spare;
    g.selection = vec![victim];
    g.command(Command::Scuttle, &mut rig.audio);
    assert!(
        said(g, event::SCUTTLE_REQUEST),
        "fleet command did not ask to confirm"
    );
    assert_eq!(
        g.world.universe().ship(victim).expect("ship").death_time,
        0.0,
        "scuttled without confirmation"
    );
    rig.frame(g);
    g.command(Command::Scuttle, &mut rig.audio);
    assert!(
        g.world.universe().ship(victim).expect("ship").death_time > 0.0,
        "the second press did not scuttle"
    );
    for _ in 0..16 * 4 {
        rig.frame(g);
    }
    assert!(
        g.world.universe().ship(victim).is_none(),
        "the scuttled ship is still there"
    );
}
