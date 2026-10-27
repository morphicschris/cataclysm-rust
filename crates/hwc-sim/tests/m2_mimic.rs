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

/// Mission 3's disguise menus (`SetCurrentMimicItem`): the player's Mimic, with the Holographic
/// Emitter researched, takes the first entry of "My Ships" (a Recon) from the special-action
/// menu's "Mimic" submenu, only the entries below the MCV index, and drops it with "UnMimic".
#[test]
fn mission3_player_mimic_takes_a_shape_from_the_disguise_menu() {
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
    config.speech_seconds = Some(3.0);
    let mut run = MissionRun::start_campaign(statics, tweaks, &src, script, config, &c).expect("mission 3");
    run.run(16 * 5);
    let t = run.game.disguise_tables.clone();
    let types: Vec<i32> = t.entries[0].iter().map(|e| e.ship_type).collect();
    assert_eq!(types, ["sRecon", "sAcolyte", "sACV", "sWorker", "sRammingFrigate"].map(|n| ShipType::from_name(n).unwrap().index() as i32));
    assert_eq!(hwc_sim::player_orders::mimic_entry_count(&t, 0, false), 3);
    assert_eq!(hwc_sim::player_orders::mimic_entry_count(&t, 0, true), 5);
    // A saved game rebuilds the menus from its log.
    run.game.disguise_tables = Default::default();
    run.after_load();
    assert_eq!(run.game.disguise_tables, t);
    // A Mimic beside the mothership, with the disguise upgrade.
    let u = &mut run.universe;
    let tech = hwc_sim::statics::Technology::from_name("TECH_MIMIC").unwrap();
    u.base.players[0].tech.add(tech.index());
    hwc_sim::upgrade::player_update_from_research(u, 0);
    let ms = u.base.players[0].mothership.expect("mothership");
    let p = u.ships.ship(ms).posinfo.position + hwc_math::Vector::new(3000.0, 0.0, 0.0);
    let id = hwc_sim::base::create_ship(u, ShipType::sMimic, 0, p, Default::default()).expect("mimic");
    hwc_sim::upgrade::apply_upgrades(u, id);
    assert!(hwc_sim::upgrade::has_upgrade(u.ships.ship(id), mimic::UPGRADE_DISGUISE));
    // Beyond the MCV index a single Mimic is left out.
    hwc_sim::player_orders::mimic_disguise_as(u, &t, 0, &[id], 3);
    run.run(16 * 4);
    assert!(!mimic::is_disguised(&run.universe, id));
    hwc_sim::player_orders::mimic_disguise_as(&mut run.universe, &t, 0, &[id], 0);
    run.run(16 * 4);
    assert!(mimic::is_disguised(&run.universe, id));
    assert_eq!(mimic::apparent_ship_type(&run.universe, id), Some(ShipType::sRecon));
    hwc_sim::player_orders::mr_mimic_undisguise(&mut run.universe, &[id]);
    run.run(16 * 4);
    assert!(!mimic::is_disguised(&run.universe, id));
}
