//! Effects of harvesting, salvaging and KAS `SpawnEffect` over the real game data
//! (`HWC_EXTRACTED_DIR`; skipped when unset): the beams start and stop where the original
//! starts and stops them, and attaching effects changes nothing in the simulation.

use std::sync::{Arc, OnceLock};

use hwc_math::Vector;
use hwc_sim::effects::{self, EffectData};
use hwc_sim::harness::{scenario, Order, Run, Scenario, Spawn};
use hwc_sim::resource::{ResourceKind, FX_SLOT_BEAM};
use hwc_sim::statics::{DirSource, LoadOptions, ShipRace, ShipType, StaticWorld, Technology};
use hwc_sim::{ShipId, SimTweaks};

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    fx: EffectData,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = std::env::var("HWC_EXTRACTED_DIR").ok()?;
        let src = DirSource::new(dir).ok()?;
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = SimTweaks::load(&src);
        let fx = EffectData::load_from(&src);
        if fx.is_empty() {
            return None;
        }
        Some(Data { statics, tweaks, fx })
    })
    .as_ref()
}

fn start(s: Scenario, with_effects: bool) -> Option<Run> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    if with_effects {
        effects::attach(&mut run.universe, &d.fx);
    }
    Some(run)
}

/// The static of the beam effect a ship holds, if any.
fn beam_stat(run: &Run, ship: ShipId) -> Option<usize> {
    let u = &run.universe;
    let e = effects::slot(u, ship, FX_SLOT_BEAM)?;
    u.effects.as_ref()?.world.effect(e).map(|ef| ef.stat)
}

fn named(name: &str) -> usize {
    data().unwrap().fx.lib.get(name).unwrap_or_else(|| panic!("{name} not loaded"))
}

/// Workers harvesting asteroids run the worker resourcing beam (`EGT_WORKER_RESOURCE_SOLID`)
/// while they collect and drop it when they leave to unload; the world is the same with and
/// without effects.
#[test]
fn worker_harvest_beam_starts_and_stops() {
    let Some(mut run) = start(scenario("base").unwrap(), true) else { return };
    let beam = named("SectBeast_resource_solid.ebg");
    let workers: Vec<ShipId> = run.ids[1..5].to_vec();
    let mut hashes = Vec::new();
    let mut started: Option<(ShipId, u32)> = None;
    let mut stopped = None;
    for _ in 0..6000 {
        hashes.push(run.step());
        match started {
            None => {
                started = workers.iter().copied().find(|&w| beam_stat(&run, w) == Some(beam)).map(|w| (w, run.tick()));
                if let Some((w, _)) = started {
                    // The beam has no owner and carries its length and half the resource's
                    // avoidance radius.
                    let u = &run.universe;
                    let fx = u.effects.as_ref().unwrap();
                    let ef = fx.world.effect(effects::slot(u, w, FX_SLOT_BEAM).unwrap()).unwrap();
                    assert_eq!(ef.owner, None);
                    let len = f32::from_bits(ef.variables[0]);
                    assert!(len > 0.0 && len < 3000.0, "beam length {len}");
                    assert!(f32::from_bits(ef.variables[1]) > 0.0);
                }
            }
            Some((w, _)) => {
                if effects::slot(&run.universe, w, FX_SLOT_BEAM).is_none() {
                    stopped = Some(run.tick());
                    break;
                }
            }
        }
    }
    let (w, t0) = started.expect("a worker turned its beam on");
    let t1 = stopped.expect("the beam went off again");
    assert!(t1 > t0);
    // The worker had collected something by then.
    assert!(run.universe.ships.ship(w).base.resources > 0);
    // Effects do not change the simulation.
    let mut plain = start(scenario("base").unwrap(), false).unwrap();
    let h2: Vec<u64> = (0..hashes.len()).map(|_| plain.step()).collect();
    assert_eq!(hashes, h2);
}

/// A Taiidan resource collector turns on its race's asteroid resourcing effect, owned by the
/// collector, and keeps it updated while it collects.
#[test]
fn resource_collector_harvest_effect() {
    let s = Scenario {
        name: "collector",
        players: vec![ShipRace::Taiidan],
        spawns: vec![Spawn { ship_type: ShipType::tResourceCollector, player: 0, position: Vector::new(0.0, 0.0, 0.0) }],
        orders: vec![(0, Order::Collect { ships: vec![0] })],
        resources: vec![hwc_sim::harness::ResourceSpawn { kind: ResourceKind::Asteroid(2), position: Vector::new(4000.0, 500.0, 0.0) }],
        resource_units: Vec::new(),
    };
    let Some(mut run) = start(s.clone(), true) else { return };
    let ship = run.ids[0];
    let expected = {
        let fx = run.universe.effects.as_ref().unwrap();
        fx.pick(fx.tables.resource[ShipRace::Taiidan.index() * 2].as_deref()).expect("Taiidan asteroid effect")
    };
    let mut hashes = Vec::new();
    let mut seen = None;
    for _ in 0..3000 {
        hashes.push(run.step());
        if beam_stat(&run, ship) == Some(expected) {
            seen = Some(run.tick());
            break;
        }
    }
    seen.expect("the harvest effect came on");
    // It follows the collector for some updates and carries the asteroid's bounding radius.
    for _ in 0..30 {
        hashes.push(run.step());
    }
    let u = &run.universe;
    if let Some(e) = effects::slot(u, ship, FX_SLOT_BEAM) {
        let ef = u.effects.as_ref().unwrap().world.effect(e).expect("effect");
        assert_eq!(ef.owner, Some(effects::FxOwner::Ship(ship).id()));
        let size = u.statics.asteroids[2].as_ref().unwrap().info.header.coll.coll_sphere_size;
        assert_eq!(ef.variables[1], size.to_bits());
        assert!(f32::from_bits(ef.variables[0]) > 0.0);
    }
    let mut plain = start(s, false).unwrap();
    let h2: Vec<u64> = (0..hashes.len()).map(|_| plain.step()).collect();
    assert_eq!(hashes, h2);
}

fn salvage_scenario() -> Scenario {
    Scenario {
        name: "salvage",
        players: vec![ShipRace::Somtaaw, ShipRace::Derelict],
        spawns: vec![
            Spawn { ship_type: ShipType::sMothership, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::sWorker, player: 0, position: Vector::new(2000.0, -300.0, 0.0) },
            Spawn { ship_type: ShipType::sWorker, player: 0, position: Vector::new(2000.0, 300.0, 0.0) },
            Spawn { ship_type: ShipType::dIonArrayFrigate, player: 1, position: Vector::new(6000.0, 0.0, 0.0) },
        ],
        orders: vec![(1, Order::Salvage { ships: vec![1, 2], targets: vec![3] })],
        resources: Vec::new(),
        resource_units: Vec::new(),
    }
}

fn prepare_salvage(run: &mut Run) {
    let u = &mut run.universe;
    u.base.players[0].tech.add(Technology::Salvage.index());
    hwc_sim::upgrade::player_update_from_research(u, 0);
    for i in [1, 2] {
        hwc_sim::upgrade::apply_upgrades(u, run.ids[i]);
    }
    let t = run.ids[3];
    let max = u.statics.ship(ShipType::dIonArrayFrigate).unwrap().health.max_health;
    u.ships.ship_mut(t).health = max * 0.3;
    u.ships.ship_mut(t).tactics = hwc_sim::object::tactics::NEUTRAL;
}

/// Salvagers latched to salvage points (not latch points) run their race's tractor beam,
/// which keeps no velocity of its own; it is gone once they let go.
#[test]
fn salvage_tractor_beam() {
    let Some(mut run) = start(salvage_scenario(), true) else { return };
    prepare_salvage(&mut run);
    let expected = {
        let fx = run.universe.effects.as_ref().unwrap();
        fx.pick(fx.tables.tractor_beam[ShipRace::Somtaaw.index()].as_deref()).expect("Somtaaw tractor beam")
    };
    assert_eq!(expected, named("WorkerSalvage.ebg"));
    let workers = [run.ids[1], run.ids[2]];
    let mut hashes = Vec::new();
    let mut beam_on = None;
    let mut released = None;
    for _ in 0..6000 {
        hashes.push(run.step());
        let on: Vec<ShipId> = workers.iter().copied().filter(|&w| beam_stat(&run, w) == Some(expected)).collect();
        if beam_on.is_none() {
            if let Some(&w) = on.first() {
                let u = &run.universe;
                let ef = u.effects.as_ref().unwrap().world.effect(effects::slot(u, w, FX_SLOT_BEAM).unwrap()).unwrap();
                assert_eq!(ef.owner, Some(effects::FxOwner::Ship(w).id()));
                assert_eq!(ef.velocity, Vector::ZERO);
                beam_on = Some(run.tick());
            }
        } else if workers.iter().all(|&w| effects::slot(&run.universe, w, FX_SLOT_BEAM).is_none()) {
            released = Some(run.tick());
            break;
        }
    }
    let t0 = beam_on.expect("a tractor beam came on");
    let t1 = released.expect("the salvagers let go");
    assert!(t1 > t0);
    let mut plain = start(salvage_scenario(), false).unwrap();
    prepare_salvage(&mut plain);
    let h2: Vec<u64> = (0..hashes.len()).map(|_| plain.step()).collect();
    assert_eq!(hashes, h2);
}

/// `SpawnEffect`: the named effect on each ship, attached to it, with the ship's bounding
/// radius and the integer parameter as a float; unknown names start nothing.
#[test]
fn kas_spawn_effect() {
    let Some(mut run) = start(scenario("base").unwrap(), true) else { return };
    run.step();
    let ships = vec![run.ids[0], run.ids[1]];
    let stat = named("resource_solid.ebg");
    let before = run.universe.effects.as_ref().unwrap().world.effects.len();
    hwc_sim::kas_host::spawn_effect(&mut run.universe, &ships, "resource_solid.ebg", 3);
    hwc_sim::kas_host::spawn_effect(&mut run.universe, &ships, "no_such_effect.ebg", 3);
    let u = &run.universe;
    let fx = u.effects.as_ref().unwrap();
    assert_eq!(fx.world.effects.len(), before + 2);
    for &s in &ships {
        let ef = fx.world.effects.iter().find(|e| e.owner == Some(effects::FxOwner::Ship(s).id()) && e.stat == stat).expect("effect on ship");
        let size = u.statics.ship(u.ships.ship(s).ship_type).unwrap().header.coll.coll_sphere_size;
        assert_eq!(ef.variables[0], size.to_bits());
        assert_eq!(ef.variables[1], 3.0f32.to_bits());
        assert_eq!(ef.flags & effects::attach::FULL, effects::attach::FULL);
    }
}

/// Two runs with effects attached give the same world.
#[test]
fn harvesting_with_effects_is_deterministic() {
    let Some(mut a) = start(scenario("base").unwrap(), true) else { return };
    let mut b = start(scenario("base").unwrap(), true).unwrap();
    for _ in 0..1500 {
        assert_eq!(a.step(), b.step());
    }
    let n = |r: &Run| r.universe.effects.as_ref().unwrap().world.effects.len();
    assert_eq!(n(&a), n(&b));
}
