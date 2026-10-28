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

/// Missions 1 and 2 are completed for real by the scripted player (no forced jump): every
/// objective, the "Hyperspace" objective, and the jump; mission 2 starts from mission 1's
/// carry-over.
#[test]
#[ignore = "autoplay tuning: since the AI-player and formation fixes, M1 ends with too few workers for M2 to finish from its carry"]
fn missions_1_and_2_complete_for_real() {
    let Some(d) = data() else { return };
    let mut carry: Option<Carry> = None;
    for m in 1..=2u32 {
        let mut run = start(d, m, carry.as_ref());
        let mut ap = Autoplay::new(16 * 4500);
        let ending = ap.play(&mut run, 16 * 6000);
        assert_eq!(ending, Ending::Hyperspace, "mission {m}: {:?}", ap.notes);
        assert!(!ap.forced, "mission {m}: the jump was forced");
        assert!(
            run.game.log.iter().any(|e| e.text.starts_with("ObjectiveCreate \"Hyperspace\"")),
            "mission {m}: the mission offered the jump"
        );
        assert!(!run.game.mission_failed);
        let c = run.carry().expect("carry-over");
        if m == 1 {
            // The small ships travel docked in the mothership.
            let ms = c.fleet.iter().find(|f| f.ship_type == ShipType::sMothership).unwrap();
            assert!(!ms.docked.is_empty(), "mission 1: small ships carried docked");
        }
        carry = Some(c);
    }
}

/// Mission 4 from the `warp.script` fleet is completed for real: hangar module rebuilt, ten
/// Acolytes guarding the Clee-San (protect orders, `ShipsSelectGuarding`), the infection
/// researched, the pirate cruiser infected, the jump offered and taken.
#[test]
fn mission4_from_warp_completes_for_real() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 4).expect("warp.script level 4");
    let mut run = start(d, 4, Some(&c));
    let mut ap = Autoplay::new(16 * 3000);
    let ending = ap.play(&mut run, 16 * 3500);
    assert_eq!(ending, Ending::Hyperspace, "{:?}", ap.notes);
    for label in ["Replace", "Escort"] {
        assert!(run.game.log.iter().any(|e| e.text == format!("ObjectiveDestroy {label:?}")), "objective {label} completed");
    }
    assert!(run.game.log.iter().any(|e| e.text == "ObjectiveSet \"Research\" = 1"));
}

/// Mission 3 from the `warp.script` fleet is completed for real: the Clee-San is found and
/// handed over, the Mimic sent to it, the mothership brought near so that it docks
/// (`ShipsDockStay`, then `TeamDockedReadyForLaunch` once it is inside), the jump offered and
/// taken.
#[test]
#[ignore = "autoplay tuning: the scripted scouting runs into the mine window after the Clee-San is found"]
fn mission3_from_warp_completes_for_real() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 3).expect("warp.script level 3");
    let mut run = start(d, 3, Some(&c));
    let mut ap = Autoplay::new(16 * 3000);
    let ending = ap.play(&mut run, 16 * 3600);
    assert_eq!(ending, Ending::Hyperspace, "{:?}", ap.notes);
    assert!(!ap.forced);
    for label in ["Find", "Mimic"] {
        assert!(run.game.log.iter().any(|e| e.text == format!("ObjectiveDestroy {label:?}")), "objective {label} completed");
    }
    assert!(run.game.log.iter().any(|e| e.text.starts_with("ObjectiveCreate \"Hyperspace\"")));
    // The Clee-San travels on inside the mothership.
    let c = run.carry().expect("carry-over");
    assert!(c.fleet.iter().any(|f| f.docked.iter().any(|x| x.ship_type == ShipType::dEcclesiam)), "the Clee-San is carried");
}

/// Mission 6 from the `warp.script` fleet is completed for real: the recons scout the nav
/// points (the last by finding the heavy cruiser near it), the mothership keeps with the
/// Caal-Shto, decoys set off the mines that stop it, the Taiidan scouts are hunted, the heavy
/// cruiser's group is cleared before the last leg, the Caal-Shto slips through the gate and
/// the jump is taken.
#[test]
#[ignore = "autoplay tuning: since the 1:1 formation attack and holding pattern, the scripted fleet wakes the Death fleet at the heavy cruiser group"]
fn mission6_from_warp_completes_for_real() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 6).expect("warp.script level 6");
    let mut run = start(d, 6, Some(&c));
    let mut ap = Autoplay::new(16 * 3500);
    let ending = ap.play(&mut run, 16 * 4000);
    assert_eq!(ending, Ending::Hyperspace, "{:?}", ap.notes);
    assert!(!ap.forced);
    assert!(run.game.log.iter().any(|e| e.text == "ObjectiveDestroy \"Escort\""), "the Caal-Shto was escorted");
}

/// Mission 5 from the `warp.script` fleet. Still fails: after the Bentusi's end its fighter
/// teams (killed by its explosion before the scuttle message reaches them) re-create six Beast
/// Acolytes each from the Beast carrier, launched one by one through its launch points as the
/// original does, and the 36 Acolytes (plus the bombers and corvettes that follow) beat the
/// scripted fleet and kill the Caal-Shto. Run with
/// `cargo test --release -p hwc-sim --test campaign mission5 -- --ignored`.
#[test]
#[ignore = "the scripted fleet loses the Caal-Shto to the Beast's re-created Acolytes"]
fn mission5_from_warp_completes_for_real() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 5).expect("warp.script level 5");
    let mut run = start(d, 5, Some(&c));
    let mut ap = Autoplay::new(16 * 3500);
    let ending = ap.play(&mut run, 16 * 4000);
    assert_eq!(ending, Ending::Hyperspace, "{:?}", ap.notes);
}

/// The real campaign: Mission 1 to Mission 6, each from the previous one's carry-over, every
/// mission completed (no forced jump). Long (about 15 minutes) and not yet passing (the chain
/// breaks at mission 2/3: mission 1 leaves too few workers and RUs). Run with
/// `cargo test --release -p hwc-sim --test campaign real_campaign_chain -- --ignored`.
#[test]
#[ignore = "long; the scripted economy does not yet carry the fleet through missions 2-3"]
fn real_campaign_chain_mission1_to_mission6() {
    let Some(d) = data() else { return };
    let mut carry: Option<Carry> = None;
    for m in 1..=6u32 {
        let mut run = start(d, m, carry.as_ref());
        let mut ap = Autoplay::new(16 * 4500);
        let ending = ap.play(&mut run, 16 * 6000);
        assert_eq!(ending, Ending::Hyperspace, "mission {m}: {:?}", ap.notes);
        carry = Some(run.carry().expect("carry-over"));
    }
}

/// Mission 6 opens as the original's does: the Caal-Shto, which came into mission 5 mid-level
/// and joined the player, jumps with the fleet (the skipped roll call clears its "went out
/// mid-level" mark, as `singlePlayerGameUpdate` does) and joins mission 6's CaalShto team; and
/// the ships launched on arrival take their fog-of-war viewers to their parade slots, so no
/// viewer is left at the origin to "discover" the level's mines there. The first fleet intel
/// is the Taiidan base signal (0x10002142, then 0x10002144), not the minefield (0x10002145).
#[test]
fn mission6_opens_with_the_caal_shto_and_the_base_signal() {
    let Some(d) = data() else { return };
    let src = GameDataSource(&d.gd);
    let c = Carry::warp(d.statics.clone(), d.tweaks.clone(), &src, 5).expect("warp.script level 5");
    let mut run = start(d, 5, Some(&c));
    let caal_shto = |run: &MissionRun| run.universe.ship_list.iter().any(|&id| run.universe.ships.get(id).is_some_and(|s| s.ship_type == ShipType::kCarrier && s.player == 0));
    let mut ap = Autoplay::new(16 * 3500);
    let mut steps = 0;
    while !caal_shto(&run) {
        ap.play(&mut run, 16);
        steps += 16;
        assert!(steps < 16 * 3000, "the Caal-Shto never joined the player in mission 5");
    }
    // Until it is out of its hyperspace window.
    ap.play(&mut run, 16 * 30);
    run.force_hyperspace().expect("forced jump");
    while !run.mission_complete() {
        run.step();
        steps += 1;
        assert!(steps < 16 * 3200, "mission 5's jump never ended");
    }
    let c = run.carry().expect("carry-over");
    assert!(c.fleet.iter().any(|f| f.ship_type == ShipType::kCarrier), "the Caal-Shto is carried");
    let mut m6 = start(d, 6, Some(&c));
    let speech = |m: &MissionRun| m.game.log.iter().filter(|e| e.text.starts_with("SpeechEvent")).map(|e| e.text.clone()).collect::<Vec<_>>();
    for _ in 0..16 * 180 {
        m6.step();
        if speech(&m6).len() >= 2 {
            break;
        }
    }
    let said = speech(&m6);
    assert_eq!(said.get(..2), Some(&["SpeechEvent 0x10002142".to_string(), "SpeechEvent 0x10002144".to_string()][..]), "{said:?}");
    assert!(caal_shto(&m6), "the Caal-Shto is in mission 6");
}
