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
    let mut loading = Loading::new(data, &session, 1, 2, size).ok()?;
    loading.opening_nis = false;
    loading.loader.play_nislets = nislets;
    for _ in 0..40 {
        match loading.advance(&session, Some(&mut renderer), DT) {
            Progress::Working => {}
            Progress::Ready(g) => {
                return Some((
                    g,
                    Rig {
                        renderer,
                        audio,
                        size,
                    },
                ))
            }
            Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
        }
    }
    panic!("loading never finished");
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
