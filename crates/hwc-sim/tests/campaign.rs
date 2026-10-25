//! The campaign chain: Mission 1 to Mission 6 with the fleet carried over between missions
//! (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped when neither is set). Each
//! mission is played by the scripted test orders and ends with the hyperspace jump (forced
//! with the original's debug override when the orders cannot finish it in time).

use std::sync::{Arc, OnceLock};

use hwc_formats::gamedata::GameData;
use hwc_sim::autoplay::{Autoplay, Ending};
use hwc_sim::campaign::Carry;
use hwc_sim::harness::world_hash;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    gd: GameData,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let gd = match GameData::from_env() {
            Some(Ok(gd)) => gd,
            _ => {
                eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
                return None;
            }
        };
        let src = GameDataSource(&gd);
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
        let tweaks = SimTweaks::load(&src);
        Some(Data { gd, statics, tweaks })
    })
    .as_ref()
}

fn start(d: &Data, m: u32, carry: Option<&Carry>) -> MissionRun {
    let script = load_kas_mission(&d.gd, &mission_script_name(m)).expect("mission script");
    let mut config = MissionConfig::new(m);
    config.log_calls = false;
    config.speech_seconds = Some(3.0);
    let src = GameDataSource(&d.gd);
    match carry {
        None => MissionRun::start(d.statics.clone(), d.tweaks.clone(), &src, script, config),
        Some(c) => MissionRun::start_campaign(d.statics.clone(), d.tweaks.clone(), &src, script, config, c),
    }
    .unwrap_or_else(|e| panic!("mission {m}: {e}"))
}

#[test]
fn campaign_chain_mission1_to_mission6_with_carry_over() {
    let Some(d) = data() else { return };
    let mut carry: Option<Carry> = None;
    for m in 1..=6u32 {
        let mut run = start(d, m, carry.as_ref());
        if let Some(c) = &carry {
            // The fleet exists (in hyperspace) before the mission starts, with the RUs and
            // the technology it left with.
            assert_eq!(run.universe.base.players[0].resource_units, c.resource_units, "mission {m} RUs");
            assert_eq!(run.universe.base.players[0].tech.has, c.player.tech.has, "mission {m} technology");
            assert!(run.universe.base.players[0].mothership.is_some(), "mission {m}: mothership in hyperspace");
        }
        let ending = Autoplay::new(16 * 90).play(&mut run, 16 * 900);
        assert!(matches!(ending, Ending::Hyperspace | Ending::Forced), "mission {m} ended {ending:?}");
        assert!(run.game.ships_have_arrived, "mission {m}: the fleet arrived");
        let c = run.carry().expect("carry-over");
        assert_eq!(c.mission, m);
        let ms: Vec<_> = c.fleet.iter().filter(|f| f.ship_type == ShipType::sMothership).collect();
        assert_eq!(ms.len(), 1, "mission {m}: one mothership carried");
        assert!(c.fleet.iter().all(|f| f.carried));
        // The text form brings back the same carry-over.
        let back = Carry::from_text(&c.to_text(), d.statics.clone(), d.tweaks.clone()).expect("carry text");
        assert_eq!(back.fleet, c.fleet, "mission {m} fleet text round trip");
        assert_eq!(back.resource_units, c.resource_units);
        assert_eq!(back.randy, c.randy);
        assert_eq!(back.total_time.to_bits(), c.total_time.to_bits());
        carry = Some(c);
    }
}

/// Mission 2 started from a carry-over and from its text form runs identically.
#[test]
fn mission2_from_carry_file_is_deterministic() {
    let Some(d) = data() else { return };
    let mut m1 = start(d, 1, None);
    while !m1.game.ships_have_arrived {
        m1.step();
    }
    m1.force_hyperspace().expect("forced jump");
    for _ in 0..16 * 60 {
        m1.step();
        if m1.mission_complete() {
            break;
        }
    }
    let c = m1.carry().expect("mission 1 carry-over");
    // Mission 1 is a mission without a roll call: the small ships are docked in the mothership.
    let ms = c.fleet.iter().find(|f| f.ship_type == ShipType::sMothership).unwrap();
    assert!(!ms.docked.is_empty(), "recons and workers travel inside the mothership");
    let text = Carry::from_text(&c.to_text(), d.statics.clone(), d.tweaks.clone()).unwrap();
    let mut a = start(d, 2, Some(&c));
    let mut b = start(d, 2, Some(&text));
    for _ in 0..16 * 40 {
        a.step();
        b.step();
    }
    assert!(a.game.ships_have_arrived);
    assert_eq!(world_hash(&a.universe), world_hash(&b.universe));
}

/// Missions after the first start from `warp.script`'s fleet when there is no carry-over.
#[test]
fn warp_fleet_starts_mission4() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 4).expect("warp.script level 4");
    assert_eq!(c.mission, 3);
    assert_eq!(c.resource_units, 5000);
    assert!(c.fleet.iter().any(|f| f.ship_type == ShipType::dEcclesiam));
    let mut run = start(d, 4, Some(&c));
    for _ in 0..16 * 30 {
        run.step();
    }
    // Mission 4's fleet fades in at the start point.
    assert!(run.game.ships_have_arrived);
}
