//! Fog of war, per-player visibility and the automatic behaviour of computer ships over the
//! real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped when neither is set).

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::universe::SpecialEvent;
use hwc_sim::SimTweaks;

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let src: Box<dyn DataSource> = if let Ok(dir) = std::env::var("HWC_EXTRACTED_DIR") {
            Box::new(DirSource::new(dir).ok()?)
        } else if let Ok(dir) = std::env::var("HWC_GAME_DIR") {
            Box::new(BigArchive::open(std::path::Path::new(&dir).join("cataclysm.big")).ok()?)
        } else {
            return None;
        };
        let statics = Arc::new(StaticWorld::load_from(src.as_ref(), LoadOptions::default()));
        let tweaks = SimTweaks::load(src.as_ref());
        Some(Data { statics, tweaks })
    })
    .as_ref()
}

fn run(s: Scenario, ticks: u32) -> Option<(Vec<u64>, Vec<SpecialEvent>, Run)> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let mut hashes = Vec::new();
    let mut events = Vec::new();
    for _ in 0..ticks {
        hashes.push(run.step());
        events.extend(run.universe.special_events.iter().cloned());
    }
    Some((hashes, events, run))
}

/// A recon flies from the corner of the world towards an enemy frigate: the fog grid counts
/// its viewer cells, explores cells as it moves and reports the enemy once.
#[test]
fn fog_of_war_explores_and_discovers() {
    let s = Scenario {
        name: "fog",
        players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        spawns: vec![
            Spawn { ship_type: ShipType::sRecon, player: 0, position: Vector::new(-60000.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: Vector::new(-15000.0, 0.0, 0.0) },
        ],
        orders: vec![
            (0, Order::FogOfWar { player: 0 }),
            // (New ships are neutral and fire back; the frigate is evasive so the recon lives.)
            (0, Order::Tactics { ships: vec![1], tactics: hwc_sim::object::tactics::EVASIVE }),
            (1, Order::Move { ships: vec![0], mode: 0, waypoints: vec![Vector::new(-20000.0, 0.0, 0.0)] }),
        ],
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let Some((h1, events, run)) = run(s.clone(), 900) else { return };
    let g = run.universe.fog.grids[0].as_ref().expect("grid");
    assert_eq!(g.dims, [20, 20, 20]);
    let viewers: i32 = g.cells.iter().map(|c| c.viewers).sum();
    assert!(viewers > 0);
    let explored = g.cells.iter().filter(|c| c.explored).count();
    assert!(explored > 27, "the recon explored new cells ({explored})");
    let found = events.iter().filter(|e| matches!(e, SpecialEvent::EnemyDiscovered { .. })).count();
    assert_eq!(found, 1, "{events:?}");
    let (h2, _, _) = self::run(s, 900).unwrap();
    assert_eq!(h1, h2);
}

/// Computer-player ships pick fights with enemies in their vision range (multiplayer rules).
#[test]
fn computer_ships_attack_enemies_in_range() {
    let s = Scenario {
        name: "auto",
        players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        spawns: vec![
            Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: Vector::new(3000.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: Vector::new(3000.0, 800.0, 0.0) },
        ],
        orders: vec![(0, Order::ComputerPlayer { player: 1, difficulty: 2, primary_enemy: Some(0) })],
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let Some((h1, _, run)) = run(s.clone(), 3000) else { return };
    let attacking = run.ids[1..]
        .iter()
        .filter(|&&id| {
            let u = &run.universe;
            u.ship(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == hwc_sim::command::order::ATTACK)
        })
        .count();
    let target_hurt = run.universe.ship(run.ids[0]).is_none_or(|s| s.health < run.universe.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health);
    assert!(attacking > 0 || target_hurt, "no computer ship went on the attack");
    let (h2, _, _) = self::run(s, 3000).unwrap();
    assert_eq!(h1, h2);
}
