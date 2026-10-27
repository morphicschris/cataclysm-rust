//! Combat scenarios over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped
//! when neither is set): ships fire, hit, damage and kill each other, deterministically.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_sim::harness::{scenario, Run};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, StaticWorld};
use hwc_sim::universe::SimEvent;
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

#[derive(Debug, Default)]
struct Tally {
    fired: usize,
    hits: usize,
    deaths: Vec<(u32, u16)>,
    hashes: Vec<u64>,
}

fn play(name: &str, ticks: u32) -> Option<(Tally, Run)> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), scenario(name).unwrap()).unwrap();
    let mut t = Tally::default();
    for tick in 0..ticks {
        t.hashes.push(run.step());
        for e in &run.universe.events {
            match e {
                SimEvent::GunFired { .. } => t.fired += 1,
                SimEvent::BulletHit { .. } => t.hits += 1,
                SimEvent::ShipDied { ship, .. } => t.deaths.push((tick, ship.0)),
                _ => {}
            }
        }
        if std::env::var("HWC_PRINT").is_ok() && tick % 80 == 0 {
            for &id in &run.ids {
                if let Some(s) = run.universe.ship(id) {
                    let p = s.posinfo.position;
                    println!(
                        "t{tick} {} {:?} hp {:.0} pos ({:.0},{:.0},{:.0}) cmd {:?} tgt {:?} as {} fm {}",
                        id.0, s.ship_type, s.health, p.x, p.y, p.z, s.command, s.attack.target.map(|t| t.0), s.ai_state_attack, s.flightman
                    );
                }
            }
        }
    }
    Some((t, run))
}

#[test]
fn dogfight_fights_to_a_result() {
    let Some((a, run)) = play("dogfight", 1600) else { return };
    println!("dogfight: fired {} hits {} deaths {:?}", a.fired, a.hits, a.deaths);
    assert!(a.fired > 50, "guns fire");
    assert!(a.hits > 10, "bullets hit");
    assert!(!a.deaths.is_empty(), "ships die");
    // Dead ships are gone from the universe.
    for (_, id) in &a.deaths {
        assert!(run.universe.ship(hwc_sim::ShipId(*id)).is_none());
    }
    let (b, _) = play("dogfight", 1600).unwrap();
    assert_eq!(a.hashes, b.hashes);
}

#[test]
fn frigates_engage_a_destroyer() {
    let Some((a, run)) = play("frigates", 1600) else { return };
    println!("frigates: fired {} hits {} deaths {:?}", a.fired, a.hits, a.deaths);
    assert!(a.fired > 10, "guns fire");
    assert!(a.hits > 10, "beams and shells hit");
    let d = run.universe.ship(run.ids[3]).map(|s| s.health);
    println!("destroyer health {d:?}");
    let max = run.universe.statics.ship(hwc_sim::statics::ShipType::tStandardDestroyer).unwrap().health.max_health;
    assert!(d.is_none_or(|h| h < max), "the destroyer takes damage");
    let (b, _) = play("frigates", 1600).unwrap();
    assert_eq!(a.hashes, b.hashes);
}

/// Battle pings: once the dogfight is on, the collision blob holding it is flagged and split
/// into ping sub-blobs that hold the fighting ships, on the battle-ping frames.
#[test]
fn dogfight_makes_battle_pings() {
    let Some(d) = data() else { return };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), scenario("dogfight").unwrap()).unwrap();
    let props = hwc_sim::battle_ping::ping_blob_properties(None);
    let mut seen = 0;
    for _ in 0..800 {
        run.step();
        if !hwc_sim::battle_ping::is_battle_ping_frame(&run.universe) {
            continue;
        }
        let before = hwc_sim::harness::world_hash(&run.universe);
        let pings = hwc_sim::battle_ping::battle_pings(&mut run.universe, &props);
        assert_eq!(before, hwc_sim::harness::world_hash(&run.universe), "battle pings change nothing");
        for p in &pings {
            assert!(!p.ships.is_empty());
            assert!(p.radius > 0.0);
            for &s in &p.ships {
                let pos = run.universe.ship(s).unwrap().posinfo.position;
                assert!((pos - p.centre).mag_sq().sqrt() <= 2.0 * p.radius, "a ship of the ping lies within it");
            }
        }
        seen += pings.len();
    }
    assert!(seen > 0, "the dogfight shows up as battle pings");
}

/// Attacking in formation (`clProcess`, formation attack): six Acolytes in delta formation,
/// ordered to attack a target 20 km away, close on it in formation (one command, formation
/// attribute kept, the wingmen in their slots near the leader), and the formation is still
/// theirs when the attack ends.
#[test]
fn formation_keeps_formation_when_ordered_to_attack() {
    use hwc_math::Vector;
    use hwc_sim::harness::{Order, Scenario, Spawn};
    use hwc_sim::statics::{ShipRace, ShipType};
    let Some(d) = data() else { return };
    let mut spawns: Vec<Spawn> = (0..6).map(|i| Spawn { ship_type: ShipType::sAcolyte, player: 0, position: Vector::new(-10000.0, i as f32 * 200.0, 0.0) }).collect();
    spawns.push(Spawn { ship_type: ShipType::tResourceCollector, player: 1, position: Vector::new(10000.0, 0.0, 0.0) });
    let ours: Vec<usize> = (0..6).collect();
    let sc = Scenario {
        name: "formation_attack",
        players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        spawns,
        orders: vec![
            (0, Order::Tactics { ships: ours.clone(), tactics: hwc_sim::object::tactics::NEUTRAL }),
            (0, Order::Formation { ships: ours.clone(), formation_type: hwc_sim::formation::kind::DELTA }),
            (160, Order::Attack { ships: ours.clone(), targets: vec![6] }),
        ],
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), sc).unwrap();
    let ids = run.ids.clone();
    let mut approached = 0;
    let mut ended_in_formation = false;
    for tick in 0..16 * 120 {
        run.step();
        let u = &run.universe;
        if tick < 170 {
            continue;
        }
        let cmd = |id| u.ship(id).and_then(|s| s.command).and_then(|c| u.commands.get(c));
        let Some(c) = cmd(ids[0]) else { continue };
        assert!(c.attributes & hwc_sim::command::attr::FORMATION != 0, "the formation is kept (tick {tick})");
        assert!(ids[..6].iter().all(|&id| u.ship(id).is_none() || cmd(id).map(|x| x.id) == Some(c.id)), "one command (tick {tick})");
        if c.order == hwc_sim::command::order::ATTACK && !c.attack_in_range {
            approached += 1;
            let lp = u.ship(ids[0]).unwrap().posinfo.position;
            for &id in &ids[1..6] {
                let p = u.ship(id).unwrap().posinfo.position;
                assert!((p - lp).mag_sq().sqrt() < 3000.0, "wingman {} stays near the leader on the approach (tick {tick})", id.0);
            }
        }
        if u.ship(ids[6]).is_none() && c.order == hwc_sim::command::order::NULL {
            ended_in_formation = true;
            break;
        }
    }
    assert!(approached > 100, "the formation closed on the target in formation ({approached} updates)");
    assert!(ended_in_formation, "the attack ended and the ships are back in their formation");
}

/// Retaliation by tactics (`univCheckShipState` / `tacticsDelegateAttackCommand`): idle ships
/// that are shot at fire back in Neutral and Aggressive, whatever they were doing before
/// (nothing, a finished move, a halt, a formation), with the player's fog of war on.
#[test]
fn idle_ships_return_fire_by_tactics() {
    use hwc_math::Vector;
    use hwc_sim::harness::{Order, Scenario, Spawn};
    use hwc_sim::object::tactics;
    use hwc_sim::statics::{ShipRace, ShipType};
    let Some(d) = data() else { return };
    let pairs = [
        (ShipType::sAcolyte, ShipType::tMultiGunCorvette, 1600.0),
        (ShipType::sMultiBeamFrigate, ShipType::tIonCannonFrigate, 2500.0),
        (ShipType::sACV, ShipType::tHeavyInterceptor, 2500.0),
    ];
    let setups = ["idle", "moved", "halted", "formation", "fog"];
    let mut failures = Vec::new();
    for (ours, theirs, dist) in pairs {
        for t in [tactics::NEUTRAL, tactics::AGGRESSIVE] {
            for setup in setups {
                let mine: Vec<usize> = (0..3).collect();
                let mut spawns: Vec<Spawn> = (0..3).map(|i| Spawn { ship_type: ours, player: 0, position: Vector::new(0.0, i as f32 * 300.0, 0.0) }).collect();
                spawns.push(Spawn { ship_type: theirs, player: 1, position: Vector::new(dist, 0.0, 0.0) });
                let mut orders = vec![(0, Order::Tactics { ships: mine.clone(), tactics: t })];
                match setup {
                    "moved" => orders.push((0, Order::Move { ships: mine.clone(), mode: 0, waypoints: vec![Vector::new(-300.0, 0.0, 0.0)] })),
                    "halted" => orders.push((0, Order::Halt { ships: mine.clone() })),
                    "formation" => orders.push((0, Order::Formation { ships: mine.clone(), formation_type: hwc_sim::formation::kind::DELTA })),
                    "fog" => orders.push((0, Order::FogOfWar { player: 0 })),
                    _ => {}
                }
                orders.push((320, Order::Attack { ships: vec![3], targets: vec![0] }));
                let sc = Scenario { name: "retaliation", players: vec![ShipRace::Somtaaw, ShipRace::Taiidan], spawns, orders, resources: Vec::new(), resource_units: Vec::new() };
                let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), sc).unwrap();
                let ids = run.ids.clone();
                let (mut fired, mut hit) = (0, 0);
                for _ in 0..16 * 40 {
                    run.step();
                    for e in &run.universe.events {
                        match e {
                            SimEvent::GunFired { ship, .. } if ids[..3].contains(ship) => fired += 1,
                            SimEvent::BulletHit { ship, .. } if ids[..3].contains(ship) => hit += 1,
                            _ => {}
                        }
                    }
                }
                println!("{ours:?} vs {theirs:?} tactics {t} {setup}: hit {hit} times, fired {fired}");
                if hit > 0 && fired == 0 {
                    failures.push(format!("{ours:?} vs {theirs:?} tactics {t} {setup}"));
                }
            }
        }
    }
    assert!(failures.is_empty(), "ships that did not fire back: {failures:?}");
}
