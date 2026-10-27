//! Mission 2's ambush (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped when neither
//! is set): the `Mimic` team's Turanic standard corvettes take the disguise upgrade from
//! `GiveTechToAI("TECH_MIMIC")` and `UpgradeShips`, arrive looking like the player's Kushan
//! Advance Support Frigates (hidden from the player as themselves), and drop the disguise at
//! the ambush.

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_sim::autoplay::Autoplay;
use hwc_sim::campaign::Carry;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::ships::mimic;
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

#[test]
fn mission2_mimics_arrive_disguised_as_the_players_frigates() {
    let Some(Ok(gd)) = GameData::from_env() else {
        eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
        return;
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let c = Carry::warp(statics.clone(), tweaks.clone(), &src, 2).expect("warp.script level 2");
    let script = load_kas_mission(&gd, &mission_script_name(2)).expect("mission script");
    let mut config = MissionConfig::new(2);
    config.log_calls = false;
    config.speech_seconds = Some(3.0);
    let mut run = MissionRun::start_campaign(statics, tweaks, &src, script, config, &c).expect("mission 2");
    let mut ap = Autoplay::new(16 * 4500);
    let (mut disguised, mut unmasked) = (false, false);
    for _ in 0..16 * 1200 {
        ap.play(&mut run, 1);
        let u = &run.universe;
        let corvettes: Vec<_> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| u.ships.ship(id).player != 0 && u.ships.ship(id).ship_type == ShipType::p1StandardCorvette)
            .filter(|&id| hwc_sim::upgrade::has_upgrade(u.ships.ship(id), mimic::UPGRADE_DISGUISE))
            .collect();
        if corvettes.len() == 4 && corvettes.iter().all(|&id| mimic::is_disguised_as_player(u, id, 0)) {
            assert!(corvettes.iter().all(|&id| mimic::apparent_ship_type(u, id) == Some(ShipType::kAdvanceSupportFrigate)));
            disguised = true;
        }
        if disguised && !corvettes.is_empty() && corvettes.iter().all(|&id| !mimic::is_disguised(u, id)) {
            unmasked = true;
            break;
        }
    }
    assert!(disguised, "the Mimic team arrived disguised: {:?}", ap.notes);
    assert!(unmasked, "the disguise dropped at the ambush: {:?}", ap.notes);
}
