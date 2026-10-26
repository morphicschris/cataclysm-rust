//! The 3D view's image-quality options through the game's own path: Mission 1 drawn in every
//! mode (render scale, MSAA, FXAA, the scene at window resolution under a scaled interface),
//! and (`--ignored --release`) frame times and comparison screenshots of Mission 1 combat at
//! 1600x900. Needs `HWC_GAME_DIR` and a GPU adapter; skips otherwise. Screenshots go to
//! `screenshots/aa/` (or `HWC_AA_SHOTS`).

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::{RenderQuality, Renderer};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

fn shot_dir() -> PathBuf {
    let dir = std::env::var_os("HWC_AA_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/aa"));
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    dir
}

struct Rig {
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
    data: Rc<GameData>,
}

fn rig(size: (i32, i32)) -> Option<Rig> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue, counts) = crate::test_gpu::device_with_msaa()?;
    let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
    renderer.set_msaa_support(&counts);
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
    Some(Rig {
        session,
        renderer,
        audio,
        data,
    })
}

fn mission_one(rig: &mut Rig, size: (i32, i32)) -> Box<Game> {
    let mut l = Loading::new(rig.data.clone(), &rig.session, 1, 2, size).expect("loading");
    l.opening_nis = false;
    l.loader.play_nislets = false;
    loop {
        if let Progress::Ready(g) = l.advance(&rig.session, Some(&mut rig.renderer), DT) {
            break g;
        }
    }
}

/// Lit pixels and smoothed-edge pixels (neither near black nor saturated) of a frame.
fn stats(g: &Game) -> (usize, usize) {
    let px = &g.canvas().pixels;
    let lit = px.iter().filter(|p| p[..3] != [0, 0, 0]).count();
    (lit, px.len())
}

fn modes(supported: &[u32]) -> Vec<(String, RenderQuality)> {
    let q = |scale: f32, msaa: u32, fxaa: bool| RenderQuality { scale, msaa, fxaa };
    let mut v = vec![
        ("off".to_string(), RenderQuality::default()),
        ("scale-0.5".into(), q(0.5, 1, false)),
        ("scale-1.5".into(), q(1.5, 1, false)),
        ("scale-2".into(), q(2.0, 1, false)),
        ("fxaa".into(), q(1.0, 1, true)),
    ];
    for &n in supported.iter().filter(|&&n| n > 1) {
        v.push((format!("msaa-{n}"), q(1.0, n, false)));
    }
    if supported.contains(&4) {
        v.push(("scale-1.5-msaa-4".into(), q(1.5, 4, false)));
        v.push(("scale-2-msaa-4".into(), q(2.0, 4, false)));
    }
    if supported.contains(&2) {
        v.push(("scale-2-msaa-2".into(), q(2.0, 2, false)));
    }
    if supported.contains(&8) {
        // The heaviest request: cut back by the sample budget.
        v.push(("scale-2-msaa-8".into(), q(2.0, 8, false)));
    }
    v
}

#[test]
fn every_quality_mode_draws_mission_one() {
    let size = (640, 480);
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = mission_one(&mut rig, size);
    let g = &mut *game;
    for _ in 0..40 {
        g.update(DT, &mut rig.audio);
    }
    g.render(&mut rig.renderer, size).expect("render");
    let base = stats(g).0;
    assert!(base > 0, "nothing drawn");
    for (name, q) in modes(rig.renderer.msaa_support()) {
        g.quality = q;
        g.render(&mut rig.renderer, size).unwrap_or_else(|e| panic!("{name}: {e}"));
        let (lit, total) = stats(g);
        assert_eq!(total, (size.0 * size.1) as usize);
        // The same view: about as much of the frame is drawn in every mode.
        let ratio = lit as f64 / base as f64;
        assert!((0.8..1.25).contains(&ratio), "{name}: {lit} lit pixels vs {base}");
        assert_eq!(
            rig.renderer.sample_count(),
            hwc_render::quality::pick_sample_count(q.msaa, rig.renderer.msaa_support()),
            "{name}"
        );
        assert!(g.scene_layer().is_none(), "{name}: no layer at the canvas size");
    }
    // A scaled front end: the scene is drawn at the window's pixels and shown under the
    // canvas, whose untouched pixels are transparent.
    g.quality = RenderQuality::default();
    g.present = Some((1280, 960));
    g.render(&mut rig.renderer, size).expect("render");
    let (rgba, _) = g.scene_layer().expect("scene layer");
    let clear = rgba.chunks(4).filter(|p| p[3] == 0).count();
    let total = (size.0 * size.1) as usize;
    assert!(clear > total / 3, "{clear} of {total} pixels show the scene");
    assert!(clear < total, "the interface is drawn over the scene");
    g.present = None;
    rig.renderer.set_sample_count(1);
}

/// Frame times of each mode in Mission 1 combat at 1600x900 (the same frame drawn repeatedly
/// once the autoplay fleet is fighting), and screenshots of off, 2x and 4x MSAA. Run with
/// `--ignored --release --nocapture`.
#[test]
#[ignore]
fn combat_frame_times_and_shots() {
    let size: (i32, i32) = std::env::var("HWC_AA_SIZE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        .unwrap_or((1600, 900));
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = mission_one(&mut rig, size);
    let g = &mut *game;
    let mut play = hwc_sim::autoplay::Autoplay::new(16 * 60 * 30);
    // Play on (not drawn) until a fight is on, then put the camera on it.
    let mut fight = None;
    for frame in 0..16 * 60 * 25 {
        if g.nis.is_none() {
            play.tick(g.world.run_mut());
        }
        for o in g.update(DT, &mut rig.audio) {
            if let crate::game::Outcome::Animatic(_) = o {
                g.movie_finished();
            }
        }
        if frame % 16 == 0 && g.nis.is_none() {
            let u = g.world.universe();
            let fighting: Vec<_> = u
                .ship_list
                .iter()
                .copied()
                .filter(|&id| u.ship(id).is_some_and(|s| s.recently_attacked != 0))
                .collect();
            let fx = u.effects.as_ref().map_or(0, |e| e.world.effects.len());
            let min_fx: usize = std::env::var("HWC_AA_MIN_FX")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(20);
            if fighting.len() >= 4 && fx >= min_fx {
                fight = Some((frame, fighting[0], fighting.len(), fx));
                break;
            }
        }
    }
    let Some((frame, id, ships, fx)) = fight else {
        panic!("no combat in Mission 1");
    };
    // A few frames on with the camera on the fight.
    let p = g.world.universe().ship(id).map(|s| s.posinfo.position).unwrap();
    g.world.follow_focus = false;
    g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
    g.world.camera.distance = 2600.0;
    g.world.camera.declination = (-18f32).to_radians();
    for _ in 0..8 {
        play.tick(g.world.run_mut());
        g.update(DT, &mut rig.audio);
    }
    let fx_now = g.world.universe().effects.as_ref().map_or(0, |e| e.world.effects.len());
    eprintln!(
        "combat at update {frame}: {ships} ships under fire, {fx} effects ({fx_now} when drawn); GPU {:?}",
        rig.renderer.msaa_support()
    );
    let dir = shot_dir();
    let n = 48;
    let mut rows = Vec::new();
    for (name, q) in modes(rig.renderer.msaa_support()) {
        g.quality = q;
        // The first frame builds the targets and pipelines.
        g.render(&mut rig.renderer, size).expect("render");
        let mut t = Vec::with_capacity(n);
        for _ in 0..n {
            let s = std::time::Instant::now();
            g.render(&mut rig.renderer, size).expect("render");
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
        t.sort_by(f64::total_cmp);
        let mean = t.iter().sum::<f64>() / n as f64;
        let p95 = t[n * 95 / 100];
        let max = t[n - 1];
        let p = g.render_plan().expect("plan");
        let plan = format!("{}x{} {}x", p.render.0, p.render.1, p.samples);
        rows.push(format!("{name:18} {plan:16} mean {mean:6.1} ms  p95 {p95:6.1}  max {max:6.1}"));
        if ["off", "scale-2", "msaa-4", "fxaa", "scale-2-msaa-4"].contains(&name.as_str()) {
            g.canvas()
                .save_png(&dir.join(format!("m01-combat-{name}.png")))
                .expect("png");
        }
    }
    // A scaled front end (`--scale fit`) in a window of this size: the 640x480 layout with the
    // scene at the viewport's pixels, against the scene at the layout's pixels.
    let layout = (640, 480);
    let s = (size.0 as f32 / 640.0).min(size.1 as f32 / 480.0);
    let viewport = ((640.0 * s) as u32, (480.0 * s) as u32);
    for (name, present) in [("fit-layout", None), ("fit-window", Some(viewport))] {
        g.quality = RenderQuality::default();
        g.present = present;
        g.render(&mut rig.renderer, layout).expect("render");
        let mut t = Vec::with_capacity(n);
        for _ in 0..n {
            let s = std::time::Instant::now();
            g.render(&mut rig.renderer, layout).expect("render");
            let _ = g.scene_layer();
            t.push(s.elapsed().as_secs_f64() * 1e3);
        }
        t.sort_by(f64::total_cmp);
        let mean = t.iter().sum::<f64>() / n as f64;
        let p = g.render_plan().expect("plan");
        let plan = format!("{}x{} {}x", p.render.0, p.render.1, p.samples);
        rows.push(format!(
            "{name:18} {plan:16} mean {mean:6.1} ms  p95 {:6.1}  max {:6.1}",
            t[n * 95 / 100],
            t[n - 1]
        ));
    }
    g.present = None;
    g.quality = RenderQuality::default();
    rig.renderer.set_sample_count(1);
    eprintln!("Mission 1 combat, {}x{}, {n} frames each:", size.0, size.1);
    for r in rows {
        eprintln!("  {r}");
    }
}
