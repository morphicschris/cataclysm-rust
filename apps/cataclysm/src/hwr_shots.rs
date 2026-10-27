//! The optional Homeworld Remastered deaths and trails (`hwcRemasteredFx`) through the game's
//! own path: Mission 1 with a few of the player's ships destroyed, drawn with the stock and the
//! Remastered effects. Needs `HWC_GAME_DIR`, the Remastered import (`HWC_HWR_DIR`, see
//! [`crate::hwr_fx::find_dir`]) and a GPU adapter; skips otherwise. Screenshots go to
//! `screenshots/hwr/` (or `HWC_HWR_SHOTS`).

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::ShipId;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

fn shot_dir() -> PathBuf {
    let dir = std::env::var_os("HWC_HWR_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hwr"));
    std::fs::create_dir_all(&dir).expect("screenshot directory");
    dir
}

struct Rig {
    session: Session,
    renderer: Renderer,
    data: Rc<GameData>,
}

fn rig(size: (i32, i32)) -> Option<Rig> {
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
    session.resize(size.0 as u32, size.1 as u32);
    Some(Rig {
        session,
        renderer,
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

/// The player's ships to destroy: the nearest few to the mothership, not the mothership.
fn victims(g: &Game) -> (Vec<ShipId>, glam::Vec3) {
    let u = g.world.universe();
    let ms = u.base.players.first().and_then(|p| p.mothership);
    let centre = ms.and_then(|id| u.ship(id)).map_or(glam::Vec3::ZERO, |s| {
        let p = s.posinfo.position;
        glam::Vec3::new(p.x, p.y, p.z)
    });
    let mut v: Vec<(f32, ShipId)> = u
        .ship_list
        .iter()
        .filter(|&&id| Some(id) != ms)
        .filter_map(|&id| {
            let s = u.ship(id)?;
            let st = u.statics.ship(s.ship_type)?;
            if s.flags & hwc_sim::base::SOF_MODULE != 0
                || s.base.clamp.is_some()
                || st.header.coll.coll_sphere_size > 400.0
            {
                return None;
            }
            let p = s.posinfo.position;
            Some(((glam::Vec3::new(p.x, p.y, p.z) - centre).length(), id))
        })
        .collect();
    // Warships first.
    let rank = |id: ShipId| {
        let class = u
            .ship(id)
            .and_then(|s| u.statics.ship(s.ship_type))
            .and_then(|st| st.ship_class);
        match class {
            Some(hwc_sim::statics::ShipClass::Fighter | hwc_sim::statics::ShipClass::Corvette) => 0,
            Some(hwc_sim::statics::ShipClass::Frigate) => 1,
            _ => 2,
        }
    };
    v.sort_by(|a, b| rank(a.1).cmp(&rank(b.1)).then(a.0.total_cmp(&b.0)));
    (v.into_iter().take(4).map(|(_, id)| id).collect(), centre)
}

/// The largest ship that is not the player's mothership or one of its modules.
fn big_victim(g: &Game) -> Option<ShipId> {
    let u = g.world.universe();
    let ms = u.base.players.first().and_then(|p| p.mothership);
    u.ship_list
        .iter()
        .copied()
        .filter(|&id| Some(id) != ms)
        .filter_map(|id| {
            let s = u.ship(id)?;
            if s.flags & hwc_sim::base::SOF_MODULE != 0 || s.base.clamp.is_some() {
                return None;
            }
            Some((
                u.statics.ship(s.ship_type)?.header.coll.coll_sphere_size,
                id,
            ))
        })
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, id)| id)
}

/// A ship (not the mothership or a module) whose class or type name contains `name`
/// (`frigate`, `worker`...), ignoring case.
fn ship_named(g: &Game, name: &str) -> Option<ShipId> {
    let u = g.world.universe();
    let ms = u.base.players.first().and_then(|p| p.mothership);
    u.ship_list
        .iter()
        .copied()
        .filter(|&id| Some(id) != ms)
        .find(|&id| {
            let Some(s) = u.ship(id) else { return false };
            let Some(st) = u.statics.ship(s.ship_type) else {
                return false;
            };
            if s.flags & hwc_sim::base::SOF_MODULE != 0 || s.base.clamp.is_some() {
                return false;
            }
            let text = format!("{:?} {:?}", s.ship_type, st.ship_class).to_ascii_lowercase();
            text.contains(name)
        })
}

#[test]
fn remastered_big_death_draws() {
    let size = (960, 540);
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    if crate::hwr_fx::find_dir(rig.data.root()).is_none() {
        eprintln!("skipped: no Remastered import (set HWC_HWR_DIR)");
        return;
    }
    let dir = shot_dir();
    for hwr in [false, true] {
        let mut game = mission_one(&mut rig, size);
        let g = &mut *game;
        g.world.set_remastered_fx(hwr);
        for _ in 0..32 {
            g.update(DT, &mut None);
        }
        let id = big_victim(g).expect("a big ship");
        let (p, r, t) = {
            let u = g.world.universe();
            let s = u.ship(id).unwrap();
            let st = u.statics.ship(s.ship_type).unwrap();
            (
                s.posinfo.position,
                st.header.coll.coll_sphere_size,
                s.ship_type,
            )
        };
        eprintln!("hwr {hwr}: destroying {id:?} {t:?} radius {r}");
        g.world.follow_focus = false;
        g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
        g.world.camera.distance = r * 5.0;
        g.world.camera.declination = (-20f32).to_radians();
        hwc_sim::damage::apply_damage_to_target(g.world.universe_mut(), id, 1.0e9, 0, 0, 0, None);
        let tag = if hwr { "hwr" } else { "stock" };
        let keep = [8usize, 24, 48, 96, 160];
        for f in 0..=160 {
            g.update(DT, &mut None);
            g.render(&mut rig.renderer, size).expect("render");
            if let Some(k) = keep.iter().position(|&x| x == f) {
                g.canvas()
                    .save_png(&dir.join(format!("m01-bigdeath-{tag}-{k}.png")))
                    .expect("png");
            }
        }
    }
}

#[test]
fn remastered_deaths_draw() {
    let size = (960, 540);
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    if crate::hwr_fx::find_dir(rig.data.root()).is_none() {
        eprintln!("skipped: no Remastered import (set HWC_HWR_DIR)");
        return;
    }
    let dir = shot_dir();
    for hwr in [false, true] {
        let mut game = mission_one(&mut rig, size);
        let g = &mut *game;
        assert_eq!(g.world.set_remastered_fx(hwr), hwr);
        for _ in 0..32 {
            g.update(DT, &mut None);
        }
        let (ids, centre) = victims(g);
        assert!(!ids.is_empty(), "no ships to destroy");
        let (target, radius) = {
            let u = g.world.universe();
            let s = u.ship(ids[0]).unwrap();
            let p = s.posinfo.position;
            let r = u
                .statics
                .ship(s.ship_type)
                .map_or(50.0, |st| st.header.coll.coll_sphere_size);
            for &id in &ids {
                let s = u.ship(id).unwrap();
                let st = u.statics.ship(s.ship_type).unwrap();
                eprintln!(
                    "  {id:?} {:?} {:?} radius {}",
                    s.ship_type, st.ship_class, st.header.coll.coll_sphere_size
                );
            }
            (glam::Vec3::new(p.x, p.y, p.z), r)
        };
        eprintln!(
            "hwr {hwr}: destroying {:?} near {centre:?}, camera on {target:?}",
            ids
        );
        g.world.follow_focus = false;
        g.world.camera.look_at = target;
        g.world.camera.distance = (radius * 8.0).max(400.0);
        g.world.camera.declination = (-20f32).to_radians();
        for &id in &ids {
            let u = g.world.universe_mut();
            hwc_sim::damage::apply_damage_to_target(u, id, 1.0e9, 0, 0, 0, None);
            for _ in 0..3 {
                g.update(DT, &mut None);
                g.render(&mut rig.renderer, size).expect("render");
            }
        }
        let tag = if hwr { "hwr" } else { "stock" };
        // Drawn every update, as in play (textures load on the first frame that needs them).
        let keep = [2usize, 6, 12, 24, 48];
        for f in 0..=48 {
            g.update(DT, &mut None);
            g.render(&mut rig.renderer, size).expect("render");
            if let Some(k) = keep.iter().position(|&x| x == f) {
                g.canvas()
                    .save_png(&dir.join(format!("m01-death-{tag}-{k}.png")))
                    .expect("png");
            }
        }
    }
}

#[test]
fn remastered_trails_draw() {
    let size = (960, 540);
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    if crate::hwr_fx::find_dir(rig.data.root()).is_none() {
        eprintln!("skipped: no Remastered import (set HWC_HWR_DIR)");
        return;
    }
    let dir = shot_dir();
    for hwr in [false, true] {
        let mut game = mission_one(&mut rig, size);
        let g = &mut *game;
        assert_eq!(g.world.set_remastered_fx(hwr), hwr);
        for _ in 0..32 {
            g.update(DT, &mut None);
        }
        let (ids, _) = victims(g);
        // Send them off and follow from behind and above.
        let at = |g: &Game, id: ShipId| g.world.universe().ship(id).map(|s| s.posinfo.position);
        let from = at(g, ids[0]).unwrap();
        let to = hwc_math::Vector::new(from.x + 6000.0, from.y + 2000.0, from.z);
        hwc_sim::command::cl_move(g.world.universe_mut(), &ids, 0, from, &[to]);
        g.world.follow_focus = false;
        g.world.camera.distance = 900.0;
        g.world.camera.declination = (-30f32).to_radians();
        let tag = if hwr { "hwr" } else { "stock" };
        for f in 0..=60 {
            g.update(DT, &mut None);
            if let Some(p) = at(g, ids[0]) {
                g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
            }
            g.render(&mut rig.renderer, size).expect("render");
            if f % 30 == 0 {
                g.canvas()
                    .save_png(&dir.join(format!("m01-trail-{tag}-{}.png", f / 30)))
                    .expect("png");
            }
        }
    }
}

/// Frames for a comparison video (`--ignored`): `HWC_HWR_FRAMES/<scene>-<stock|hwr>/NNNN.png` at
/// 30 frames a second, a group of fighters and then the biggest ship destroyed.
#[test]
#[ignore]
fn remastered_death_video_frames() {
    let size = (960, 540);
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let Some(out) = std::env::var_os("HWC_HWR_FRAMES").map(PathBuf::from) else {
        eprintln!("skipped: set HWC_HWR_FRAMES");
        return;
    };
    const FPS: f32 = 30.0;
    let scenes = std::env::var("HWC_HWR_SCENES").unwrap_or_else(|_| "fighters,capital".into());
    for scene in scenes.split(',') {
        for hwr in [false, true] {
            let mut game = mission_one(&mut rig, size);
            let g = &mut *game;
            g.world.set_remastered_fx(hwr);
            g.world.interpolate = true;
            for _ in 0..32 {
                g.update(DT, &mut None);
            }
            let ids: Vec<ShipId> = match scene {
                "fighters" => victims(g).0,
                "capital" => big_victim(g).into_iter().collect(),
                name => ship_named(g, name).into_iter().collect(),
            };
            if ids.is_empty() {
                let u = g.world.universe();
                for &id in &u.ship_list {
                    let s = u.ship(id).unwrap();
                    eprintln!("  {id:?} {:?} flags {:x} clamp {} dock {}", s.ship_type, s.flags, s.base.clamp.is_some(), s.base.dock_state);
                }
                panic!("no ship for scene {scene}");
            }
            let (p, r) = {
                let u = g.world.universe();
                let s = u.ship(ids[0]).unwrap();
                (
                    s.posinfo.position,
                    u.statics
                        .ship(s.ship_type)
                        .unwrap()
                        .header
                        .coll
                        .coll_sphere_size,
                )
            };
            g.world.follow_focus = false;
            g.world.camera.look_at = glam::Vec3::new(p.x, p.y, p.z);
            g.world.camera.distance = if scene == "fighters" { 650.0 } else { r * 5.0 };
            g.world.camera.declination = (-20f32).to_radians();
            let dir = out.join(format!("{scene}-{}", if hwr { "hwr" } else { "stock" }));
            std::fs::create_dir_all(&dir).unwrap();
            let seconds = match scene {
                "fighters" => 6.0,
                "capital" => 12.0,
                _ => 8.0,
            };
            let frames = (seconds * FPS) as usize;
            for f in 0..frames {
                // The fighters die one after another from half a second in, the capital ship at once.
                let at = |k: usize| (0.5 + 0.4 * k as f32) * FPS;
                for (k, &id) in ids.iter().enumerate() {
                    if f == at(k) as usize || (scene != "fighters" && f == 15 && k == 0) {
                        hwc_sim::damage::apply_damage_to_target(
                            g.world.universe_mut(),
                            id,
                            1.0e9,
                            0,
                            0,
                            0,
                            None,
                        );
                    }
                }
                g.update(1.0 / FPS, &mut None);
                g.render(&mut rig.renderer, size).expect("render");
                g.canvas()
                    .save_png(&dir.join(format!("{f:04}.png")))
                    .expect("png");
            }
        }
    }
}
