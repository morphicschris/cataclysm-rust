//! External construction through the game's own path: Mission 1, the Kuun-Lan builds a
//! Processor, and the hull is drawn at about 0%, 30%, 70% and 100% progress (solid where it is
//! built, a translucent blue ghost ahead of the cut). Needs `HWC_GAME_DIR` and a GPU adapter;
//! skips otherwise. Screenshots go to `screenshots/build/` (or `HWC_BUILD_SHOTS`).

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::{RenderQuality, Renderer};
use hwc_sim::base::special2;
use hwc_sim::statics::ShipType;
use hwc_sim::ShipId;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

fn shot_dir() -> PathBuf {
    let dir = std::env::var_os("HWC_BUILD_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/build")
        });
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    dir
}

fn mission_one(size: (i32, i32)) -> Option<(Box<Game>, Renderer, Option<crate::audio::Audio>)> {
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
    let mut l = Loading::new(data.clone(), &session, 1, 2, size).expect("loading");
    l.opening_nis = false;
    l.loader.play_nislets = false;
    let game = loop {
        if let Progress::Ready(g) = l.advance(&session, Some(&mut renderer), DT) {
            break g;
        }
    };
    Some((game, renderer, None))
}

/// The hull being built at the mothership (flagged under construction), with its progress.
fn hull(g: &Game, t: ShipType) -> Option<(ShipId, f32)> {
    let u = g.world.universe();
    u.ships
        .iter()
        .find(|s| s.ship_type == t && s.special_flags2 & special2::UNDER_CONSTRUCTION != 0)
        .map(|s| (s.id, s.base.build_progress))
}

/// Summed colour difference of two frames within `r` pixels of the image centre (where the
/// hull is framed).
fn centre_difference(a: &[[u8; 4]], b: &[[u8; 4]], (w, h): (i32, i32), r: i32) -> u64 {
    let mut d = 0u64;
    for y in (h / 2 - r).max(0)..(h / 2 + r).min(h) {
        for x in (w / 2 - r).max(0)..(w / 2 + r).min(w) {
            let i = (y * w + x) as usize;
            d += (0..3)
                .map(|c| u64::from(a[i][c].abs_diff(b[i][c])))
                .sum::<u64>();
        }
    }
    d
}

/// One update; NISes are skipped and animatics end at once.
fn step(g: &mut Game, audio: &mut Option<crate::audio::Audio>) {
    if g.nis.is_some() {
        g.skip_nis();
    }
    for o in g.update(DT, audio) {
        if let crate::game::Outcome::Animatic(_) = o {
            g.movie_finished();
        }
    }
}

#[test]
fn mission_one_processor_builds_in_from_the_stern() {
    let size = (800, 600);
    let Some((mut game, mut renderer, mut audio)) = mission_one(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let g = &mut *game;
    // Play on until the Kuun-Lan has arrived and the mission is under way.
    let mut frames = 0;
    loop {
        let r = g.world.run();
        if r.game.ships_have_arrived && r.game.sp_state == hwc_sim::mission::sp_state::NONE {
            break;
        }
        step(g, &mut audio);
        frames += 1;
        assert!(frames < 16 * 600, "the Kuun-Lan never arrived");
    }
    let ms = {
        let u = g.world.universe();
        u.base.players[0]
            .mothership
            .expect("the player's mothership")
    };
    {
        let u = g.world.universe_mut();
        u.base.players[0].resource_units = 50_000;
        let queued = hwc_sim::construction::queue_build(u, ms, ShipType::sProcessor, 1);
        if queued == 0 {
            // Not yet researched or enabled by the mission: queue it the way the build manager's
            // job list does.
            hwc_sim::construction::build_jobs_add(u, ms, ShipType::sProcessor, 1);
        }
    }
    // Wait for the hull to appear outside.
    let mut started = None;
    for _ in 0..16 * 60 {
        step(g, &mut audio);
        if let Some(h) = hull(g, ShipType::sProcessor) {
            started = Some(h.0);
            break;
        }
    }
    let id = started.expect("the Processor hull appears");
    let dir = shot_dir();
    // Where it sits: the mothership's frame.
    {
        let u = g.world.universe();
        let (m, h) = (u.ship(ms).unwrap(), u.ship(id).unwrap());
        let d = h.posinfo.position - m.posinfo.position;
        let r = &m.rotinfo.coordsys;
        let local = |c: usize| r.col(c).dot(d);
        eprintln!(
            "hull {:?} at mothership-local (up {:.0}, right {:.0}, forward {:.0}), dock state {:#x}, clamped {}",
            id,
            local(0),
            local(1),
            local(2),
            h.base.dock_state,
            h.base.clamp.is_some()
        );
    }
    let mut taken = Vec::new();
    let mut frames_at = Vec::new();
    let full_detail = g.world.full_detail;
    for (name, target) in [("000", 0.0f32), ("030", 0.3), ("070", 0.7), ("100", 0.995)] {
        let mut progress = 0.0;
        for _ in 0..16 * 60 * 10 {
            match hull(g, ShipType::sProcessor) {
                Some((_, p)) if p >= target => {
                    progress = p;
                    break;
                }
                Some(_) => {}
                None => break,
            }
            step(g, &mut audio);
            // Keep the mission's own events (NISlets, speech) from moving the camera.
        }
        let Some((_, p)) = hull(g, ShipType::sProcessor) else {
            panic!("the hull finished before {name}%");
        };
        let _ = progress;
        let pos = g.world.universe().ship(id).unwrap().posinfo.position;
        g.world.follow_focus = false;
        g.world.camera.look_at = glam::Vec3::new(pos.x, pos.y, pos.z);
        g.world.camera.distance = 1500.0;
        g.world.camera.declination = (-20f32).to_radians();
        g.world.camera.angle = 60f32.to_radians();
        for (suffix, q, fd) in [
            ("", RenderQuality::default(), full_detail),
            (
                "-msaa4-scale1.5-full",
                RenderQuality {
                    scale: 1.5,
                    msaa: 4,
                    fxaa: false,
                    ..Default::default()
                },
                true,
            ),
        ] {
            g.quality = q;
            g.world.full_detail = fd;
            g.render(&mut renderer, size).expect("render");
            g.canvas()
                .save_png(&dir.join(format!("m01-processor-{name}{suffix}.png")))
                .expect("png");
            if suffix.is_empty() {
                frames_at.push(g.canvas().pixels.clone());
            }
        }
        g.quality = RenderQuality::default();
        g.world.full_detail = full_detail;
        renderer.set_sample_count(1);
        taken.push((name, p));
    }
    // Finished: the flag clears and the hull draws solid.
    for _ in 0..16 * 30 {
        if hull(g, ShipType::sProcessor).is_none() {
            break;
        }
        step(g, &mut audio);
    }
    assert!(
        hull(g, ShipType::sProcessor).is_none(),
        "the Processor completes"
    );
    g.render(&mut renderer, size).expect("render");
    g.canvas()
        .save_png(&dir.join("m01-processor-done.png"))
        .expect("png");
    let done = g.canvas().pixels.clone();
    let diff: Vec<u64> = frames_at
        .iter()
        .map(|f| centre_difference(f, &done, size, 160))
        .collect();
    eprintln!("shots at progress {taken:?}; difference from the finished hull {diff:?}");
    assert!(taken[1].1 >= 0.3 && taken[1].1 < 0.4, "{taken:?}");
    assert!(taken[2].1 >= 0.7 && taken[2].1 < 0.8, "{taken:?}");
    // The hull fills in: each shot is closer to the finished ship than the one before.
    assert!(
        diff[0] > diff[1] && diff[1] > diff[2] && diff[2] > diff[3],
        "{diff:?}"
    );
    assert!(diff[0] > 4 * diff[3].max(1), "{diff:?}");
}
