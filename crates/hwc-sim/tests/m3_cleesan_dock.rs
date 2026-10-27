//! Mission 3's ending (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped when neither
//! is set): the Clee-San docks with the Kuun-Lan. Its up for the dock (`h x (up x h)`, square to
//! the cone but not unit length) can never line up to 0.995 at a point whose up is not square to
//! the cone, so `dockFlyToConeOrigin` lets the Clee-San (and only it) slide in once its heading
//! is within 0.995 and its up within 0.78; with the general rule it hung in front of the dock
//! and the mission never ended.

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_math::{Matrix, Vector};
use hwc_sim::campaign::Carry;
use hwc_sim::dock;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

#[test]
fn mission3_cleesan_docks_with_the_kuun_lan() {
    let Some(Ok(gd)) = GameData::from_env() else {
        eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
        return;
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let c = Carry::warp(statics.clone(), tweaks.clone(), &src, 3).expect("warp.script level 3");
    let script = load_kas_mission(&gd, &mission_script_name(3)).expect("mission script");
    let mut config = MissionConfig::new(3);
    config.log_calls = false;
    let mut run = MissionRun::start_campaign(statics, tweaks, &src, script, config, &c).expect("mission 3");
    run.run(16);
    // The universe alone from here: the script would hand the Clee-San over and stop it.
    let u = &mut run.universe;
    let ms = u.base.players[0].mothership.expect("Kuun-Lan");
    let clee = u.ship_list.iter().copied().find(|&id| u.ships.ship(id).ship_type == ShipType::dEcclesiam).expect("Clee-San");
    // Out along the cone of the medium dock "dbmedinsb", where the hung save was docking (it
    // takes that point with the docking bay module built).
    let d = u.ships.ship(ms);
    let info = u.statics.ship(d.ship_type).and_then(|s| s.dock.as_ref()).expect("Kuun-Lan dock points");
    let idx = info.points.iter().position(|p| p.name == "dbmedinsb").expect("dbmedinsb");
    let p = &info.points[idx];
    let near = dock::point_world_position(d, p.position) + d.rotinfo.coordsys.mul_vec(p.cone_normal) * 3000.0;
    hwc_sim::base::set_ship_position(u, clee, near);
    // Handed over to the player as `TeamSwitchPlayerOwner` does.
    let s = u.ships.ship_mut(clee);
    s.posinfo.velocity = Vector::ZERO;
    s.player = 0;
    s.attributes &= !0x100;
    assert_eq!(dock::cl_dock(u, &[clee], dock::purpose::DOCK_STAY, Some(ms)), 1);

    // On the cone the Clee-San turns to face down it with the point's up made square to that
    // heading but not unit length (`h x (up x h)`): its up can line up exactly and still give
    // a dot product of |up x h|, well short of 0.995 at the Kuun-Lan's medium dock.
    let mut short_up = None;
    for _ in 0..16 * 240 {
        u.update();
        let s = u.ships.ship(clee);
        if s.base.dock_state == dock::state::FLY_TO_CONE && s.base.dock_point != idx as i32 {
            dock::free_point(u, clee, ms);
            dock::reserve_specific_dock_point(u, clee, ms, idx);
        }
        let s = u.ships.ship(clee);
        if s.base.dock_state == dock::state::INSIDE {
            break;
        }
        if short_up.is_none() && s.base.dock_state == dock::state::FLY_TO_ORIGIN {
            let p = dock::reserved_point(u, clee, ms).expect("reserved point").clone();
            let d = u.ships.ship(ms);
            let h = -d.rotinfo.coordsys.mul_vec(p.cone_normal);
            let up0 = dock::direction_vector(p.up_direction, &d.rotinfo.coordsys);
            let up = h.cross(up0.cross(h));
            short_up = Some(up.mag_sq().sqrt());
            let up = up * (1.0 / up.mag_sq().sqrt());
            let s = u.ships.ship_mut(clee);
            s.rotinfo.coordsys = Matrix::from_cols(up, h.cross(up), h);
            s.rotinfo.rotspeed = Vector::ZERO;
        }
    }
    let short_up = short_up.expect("the Clee-San reached the cone");
    assert!(short_up < 0.995, "the dock point's up is not square to its cone ({short_up})");
    assert_eq!(u.ships.ship(clee).base.dock_state, dock::state::INSIDE, "the Clee-San docked");
}
