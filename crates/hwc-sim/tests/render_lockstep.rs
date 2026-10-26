//! Presentation must not feed back into the simulation: the same mission run headless and
//! drawn every frame (`effects::draw`) with a moving camera handed to the simulation
//! (`nebulae.camera`, `update_render_list`) gives the same world hash every update, with the
//! same intel-event timing. Both runs carry the effects (they decide when a hulk leaves the
//! universe); without them a run diverges at the first death. Needs the game (`HWC_DATA_DIR` or `HWC_GAME_DIR`); skipped
//! when neither is set.

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_math::Vector;
use hwc_sim::autoplay::Autoplay;
use hwc_sim::campaign::Carry;
use hwc_sim::harness::world_hash;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::nebula::NebulaCamera;
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

fn start(gd: &GameData, statics: &Arc<StaticWorld>, tweaks: &SimTweaks, m: u32) -> MissionRun {
    let src = GameDataSource(gd);
    let script = load_kas_mission(gd, &mission_script_name(m)).expect("mission script");
    let mut config = MissionConfig::new(m);
    config.log_calls = false;
    config.speech_seconds = Some(3.0);
    if m == 1 {
        MissionRun::start(statics.clone(), tweaks.clone(), &src, script, config).expect("mission")
    } else {
        let c = Carry::warp(statics.clone(), tweaks.clone(), &src, m).expect("warp fleet");
        MissionRun::start_campaign(statics.clone(), tweaks.clone(), &src, script, config, &c).expect("mission")
    }
}

/// Plays `updates` of mission `m` twice, side by side, with the scripted player giving the
/// same orders; the second run draws the effects with a camera circling the mothership.
fn lockstep(m: u32, updates: u32) {
    let Some(Ok(gd)) = GameData::from_env() else {
        eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
        return;
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let mut a = start(&gd, &statics, &tweaks, m);
    let mut b = start(&gd, &statics, &tweaks, m);
    // Both runs carry the effects (MissionConfig::effects): they are part of the simulation.
    assert!(a.universe.effects.is_some() && b.universe.effects.is_some(), "effects attached at load");
    let mut pa = Autoplay::new(u32::MAX);
    let mut pb = Autoplay::new(u32::MAX);
    let mut list = hwc_fx::RenderList::default();
    for k in 0..updates {
        // A camera 8 km from the mothership, going round it.
        let centre = b.universe.base.players[0].mothership.and_then(|ms| b.universe.ships.get(ms)).map_or(Vector::ZERO, |s| s.posinfo.position);
        let ang = k as f32 * 0.01;
        let eye = Vector::new(centre.x + 8000.0 * ang.cos(), centre.y + 8000.0 * ang.sin(), centre.z + 2000.0);
        b.universe.nebulae.camera = Some(NebulaCamera { eye, look_at: centre, clip_far: 200_000.0 });
        b.universe.update_render_list(eye, 200_000.0);
        pa.tick(&mut a);
        pb.tick(&mut b);
        a.step();
        b.step();
        list.clear();
        hwc_sim::effects::draw(&mut b.universe, &mut list);
        let (ha, hb) = (world_hash(&a.universe), world_hash(&b.universe));
        assert_eq!(
            ha,
            hb,
            "mission {m}: headless and drawn runs differ at update {} ({} vs {} ships)",
            a.universe.update_counter,
            a.universe.ship_list.len(),
            b.universe.ship_list.len()
        );
    }
}

#[test]
fn mission1_headless_and_drawn_runs_match() {
    lockstep(1, 16 * 300);
}

/// Mission 6 has nebulae (fog and tendrils depend on the camera).
#[test]
fn mission6_headless_and_drawn_runs_match() {
    lockstep(6, 16 * 200);
}
