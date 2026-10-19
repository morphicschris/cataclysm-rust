//! Missiles and mines over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped
//! when neither is set): launchers fire missiles that fly, hit and damage; level mines find
//! and chase ships; everything is deterministic.

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

fn duel() -> Scenario {
    Scenario {
        name: "missile duel",
        players: vec![ShipRace::Taiidan, ShipRace::Somtaaw],
        spawns: vec![
            Spawn { ship_type: ShipType::tMissileDestroyer, player: 0, position: Vector::new(-2500.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 1, position: Vector::new(2500.0, 300.0, 0.0) },
        ],
        orders: vec![(1, Order::Attack { ships: vec![0], targets: vec![1] })],
        resources: Vec::new(),
        resource_units: Vec::new(),
    }
}

#[derive(Default, Debug)]
struct Tally {
    fired: usize,
    hits: usize,
    died: usize,
    max_live: usize,
    hashes: Vec<u64>,
}

fn run(s: Scenario, ticks: u32) -> Option<(Tally, Run)> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let mut t = Tally::default();
    for _ in 0..ticks {
        t.hashes.push(run.step());
        for e in &run.universe.special_events {
            match e {
                SpecialEvent::MissileFired { .. } => t.fired += 1,
                SpecialEvent::MissileHit { .. } => t.hits += 1,
                SpecialEvent::MissileDied { .. } => t.died += 1,
                _ => {}
            }
        }
        t.max_live = t.max_live.max(run.universe.missile_list.len());
    }
    Some((t, run))
}

#[test]
fn missile_destroyer_fires_missiles_that_hit() {
    let Some((a, run)) = run(duel(), 1200) else { return };
    println!("{a:?}", a = (a.fired, a.hits, a.died, a.max_live));
    assert!(a.fired > 0, "no missile fired");
    assert!(a.hits > 0, "no missile hit");
    let target = run.universe.ship(run.ids[1]).map(|s| s.health);
    let max = run.universe.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health;
    assert!(target.is_none_or(|h| h < max), "target undamaged");
    // Launchers use up missiles.
    if let Some(s) = run.universe.ship(run.ids[0]) {
        assert!(s.guns.iter().any(|g| g.num_missiles < 8));
    }
    let (b, _) = self::run(duel(), 1200).unwrap();
    assert_eq!(a.hashes, b.hashes, "missile runs differ");
}

#[test]
fn level_mine_chases_a_ship() {
    let s = Scenario {
        name: "mine",
        players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        spawns: vec![Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: Vector::new(0.0, 0.0, 0.0) }],
        orders: vec![(0, Order::Mine { position: Vector::new(900.0, 0.0, 0.0) })],
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let Some((t, run)) = run(s.clone(), 600) else { return };
    println!("mine: {:?}", (t.hits, t.died));
    assert_eq!(t.hits, 1, "the mine should hit the frigate once");
    assert!(run.universe.missile_list.is_empty());
    let (u, _) = self::run(s, 600).unwrap();
    assert_eq!(t.hashes, u.hashes);
}
