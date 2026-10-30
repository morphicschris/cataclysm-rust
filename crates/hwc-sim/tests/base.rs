//! Base-building scenarios over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`;
//! skipped when neither is set): building and launching, harvesting, research, linking and
//! salvage, each run twice to check determinism.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::base::BaseEvent;
use hwc_sim::harness::{scenario, Order, Run, Scenario, Spawn};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld, Technology};
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

fn start(s: Scenario) -> Option<Run> {
    let d = data()?;
    Some(Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap())
}

/// Runs until `pred` holds (at most `max` updates); returns the tick and the hash then.
fn run_until(run: &mut Run, max: u32, mut pred: impl FnMut(&Run) -> bool) -> Option<(u32, u64)> {
    for _ in 0..max {
        let h = run.step();
        if pred(run) {
            return Some((run.tick(), h));
        }
    }
    None
}

fn events(run: &Run) -> &[BaseEvent] {
    &run.universe.base.events
}

#[test]
fn mothership_comes_with_its_modules() {
    let Some(run) = start(scenario("base").unwrap()) else { return };
    let u = &run.universe;
    let ms = run.ids[0];
    let modules: Vec<ShipType> = u.ships.iter().filter(|s| s.base.clamp.as_ref().and_then(|c| c.host) == Some(ms)).map(|s| s.ship_type).collect();
    assert_eq!(modules, vec![ShipType::sMothershipEnginesUpper, ShipType::sMothershipCommand, ShipType::sMothershipEngineering]);
    // The workers got the next ship numbers.
    assert_eq!(run.ids[1].0, 4);
    assert!(u.base.factories.iter().any(|f| f.ship == Some(ms)));
}

/// `ShipModuleHouseKeep`: a module with no host breaks away and explodes a couple of seconds
/// later, but not while an NIS plays (n02 flies the Kuun-Lan's hangar, `bSectMoship`, free).
#[test]
fn hostless_module_waits_for_the_nis_to_end() {
    for nis in [false, true] {
        let Some(mut run) = start(scenario("base").unwrap()) else { return };
        run.universe.nis_running = nis;
        let id = hwc_sim::base::create_ship(&mut run.universe, ShipType::bSectMoship, 0, Vector::new(5000.0, 0.0, 0.0), Default::default()).unwrap();
        let died = run_until(&mut run, 16 * 10, |r| r.universe.ships.get(id).is_none());
        assert_eq!(died.is_some(), !nis, "nis running {nis}");
    }
}

/// `ApplyDamageToTarget`: a hit on one of the mothership's own modules hurts the mothership,
/// and nothing is hurt while an NIS plays.
#[test]
fn module_damage_goes_to_the_mothership() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let ms = run.ids[0];
    let u = &mut run.universe;
    let module = u.ships.iter().find(|s| s.base.clamp.as_ref().and_then(|c| c.host) == Some(ms)).map(|s| s.id).unwrap();
    let (mh, ph) = (u.ships.ship(module).health, u.ships.ship(ms).health);
    let killed = hwc_sim::damage::apply_damage_to_target(u, module, mh * 2.0, 0, 0, 99, None);
    assert!(!killed);
    assert_eq!(u.ships.ship(module).health, mh);
    assert!(u.ships.ship(ms).health < ph);
    let ph = u.ships.ship(ms).health;
    u.nis_running = true;
    hwc_sim::damage::apply_damage_to_target(u, ms, 100.0, 0, 0, 99, None);
    assert_eq!(u.ships.ship(ms).health, ph);
    u.nis_running = false;
    // While the letterbox is in, only ships the mission marked vulnerable in wide screen.
    u.letterbox.wide_screen_in(30);
    for _ in 0..17 {
        u.letterbox.tick();
    }
    assert!(u.letterbox.fully);
    hwc_sim::damage::apply_damage_to_target(u, ms, 100.0, 0, 0, 99, None);
    assert_eq!(u.ships.ship(ms).health, ph);
    u.ships.ship_mut(ms).special_flags2 |= 0x0100_0000;
    hwc_sim::damage::apply_damage_to_target(u, ms, 100.0, 0, 0, 99, None);
    assert!(u.ships.ship(ms).health < ph);
    // The bars stop counting as in on the first step out.
    u.ships.ship_mut(ms).special_flags2 &= !0x0100_0000;
    u.letterbox.wide_screen_out(30);
    u.letterbox.tick();
    assert!(!u.letterbox.fully);
}

#[test]
fn build_a_ship_and_launch_it() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let ru0 = run.universe.base.players[0].resource_units;
    let su0 = run.universe.base.players[0].su_used;
    let mut built = None;
    let got = run_until(&mut run, 2000, |r| {
        for e in events(r) {
            if let BaseEvent::ShipBuilt { ship: Some(s), ship_type: ShipType::sRecon, .. } = e {
                built = Some(*s);
            }
        }
        built.is_some()
    });
    let (tick, _) = got.expect("a recon ship is built");
    let recon = built.unwrap();
    let u = &run.universe;
    let stat = u.statics.ship(ShipType::sRecon).unwrap();
    // Built in its build time (the docking bay hull is clamped on the first frames and builds
    // in parallel).
    assert!(tick >= stat.build_time as u32 && tick < stat.build_time as u32 + 20, "built at {tick}");
    assert_eq!(u.base.players[0].su_used, su0 + stat.support_units);
    assert!(u.base.players[0].resource_units < ru0);
    // Auto-launch: it is put outside at a launch point and flies out of the cone.
    let s = u.ship(recon).unwrap();
    assert!(u.ship_list.contains(&recon));
    assert_eq!(s.base.dock_state, hwc_sim::dock::state::FLY_OUT);
    // When the launch completes it joins the military parade around the mothership (GroupShip).
    use hwc_sim::command::order;
    let order_of = |r: &Run| r.universe.ship(recon).and_then(|s| s.command).and_then(|c| r.universe.commands.get(c)).map(|c| c.order);
    run_until(&mut run, 400, |r| order_of(r) != Some(order::LAUNCH_SHIP)).expect("launch completes");
    let ms_id = run.ids[0];
    let s = run.universe.ship(recon).unwrap();
    let c = run.universe.commands.get(s.command.expect("a parade command")).unwrap();
    assert_eq!(c.order, order::MILITARY_PARADE);
    assert_eq!(c.parade.as_ref().unwrap().around, ms_id);
    assert_eq!(s.base.dock_point, -1);
    assert!(s.base.dock_with.is_none());
    let ms = run.universe.ship(run.ids[0]).unwrap();
    let free = ms.base.occupancy.as_ref().unwrap();
    assert!(free.reserved.iter().filter(|&&r| r).count() >= 1, "the docking bay hull keeps its build point");
}

/// Runs the base scenario until the docking bay hull is clamped and building; returns the
/// mothership and the hull.
fn docking_bay_building(run: &mut Run) -> (hwc_sim::object::ShipId, hwc_sim::object::ShipId) {
    let ms = run.ids[0];
    let t = ShipType::sMothershipDockingBay;
    run_until(run, 400, |r| {
        hwc_sim::construction::job(&r.universe, ms, t).is_some_and(|j| j.hull_launched && j.cost_so_far > 20)
    })
    .expect("the docking bay hull builds");
    (ms, hwc_sim::construction::job(&run.universe, ms, t).unwrap().hull.unwrap())
}

#[test]
fn cancelling_a_module_removes_its_hull() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let t = ShipType::sMothershipDockingBay;
    let (ms, hull) = docking_bay_building(&mut run);
    let paid = hwc_sim::construction::job(&run.universe, ms, t).unwrap().cost_so_far;
    let ru = run.universe.base.players[0].resource_units;
    let ratio = run.universe.tweaks.base.cm_float_refund_ratio;
    hwc_sim::construction::cancel_job(&mut run.universe, ms, t);
    // `cmCancelJobs`: the half-built hull goes, what it cost comes back.
    assert!(run.universe.ship(hull).is_none());
    assert_eq!(run.universe.base.players[0].resource_units, ru + (paid as f32 * ratio) as i32);
    let j = hwc_sim::construction::job(&run.universe, ms, t).unwrap();
    assert!(j.n_jobs == 0 && !j.hull_exists && j.hull.is_none());
    // Its build point is free: it can be built again, from a new hull.
    assert_eq!(hwc_sim::construction::queue_build(&mut run.universe, ms, t, 1), 1);
    run_until(&mut run, 100, |r| hwc_sim::construction::job(&r.universe, ms, t).is_some_and(|j| j.hull.is_some_and(|h| h != hull)))
        .expect("a new hull starts");
}

#[test]
fn a_hull_left_by_an_old_cancel_is_removed() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let t = ShipType::sMothershipDockingBay;
    let (ms, hull) = docking_bay_building(&mut run);
    // What the old build-manager cancel left: the job emptied, the hull still there.
    let u = &mut run.universe;
    let fi = u.base.factories.iter().position(|f| f.ship == Some(ms)).unwrap();
    let j = &mut u.base.factories[fi].jobs[t.index()];
    (j.n_jobs, j.n_total, j.cost_so_far, j.time_left) = (0, 0, 0, 0);
    assert_eq!(hwc_sim::construction::remove_orphan_hulls(u), vec![hull]);
    assert!(u.ship(hull).is_none());
    let j = hwc_sim::construction::job(u, ms, t).unwrap();
    assert!(!j.hull_exists && !j.hull_launched && j.hull.is_none());
    assert_eq!(hwc_sim::construction::queue_build(u, ms, t, 1), 1);
    run_until(&mut run, 100, |r| hwc_sim::construction::job(&r.universe, ms, t).is_some_and(|j| j.hull.is_some()))
        .expect("a new hull starts");
    // A hull its job holds is left alone.
    assert!(hwc_sim::construction::remove_orphan_hulls(&mut run.universe).is_empty());
}

#[test]
fn workers_harvest_and_deliver() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let mut delivered = 0;
    run_until(&mut run, 4000, |r| {
        for e in events(r) {
            if let BaseEvent::ResourcesDelivered { amount, .. } = e {
                delivered += amount;
            }
        }
        delivered >= 1200
    })
    .expect("two workers unload");
    let stats = &run.universe.base.stats.players[0];
    assert!(stats.ru_collected >= 1200);
    // Resource units left the asteroids.
    let left: i32 = run.universe.base.resources.list.iter().map(|r| r.resource_value).sum();
    let total: i32 = scenario("base").unwrap().resources.iter().map(|r| match r.kind {
        hwc_sim::resource::ResourceKind::Asteroid(t) => run.universe.statics.asteroids[t as usize].as_ref().unwrap().info.resource_value,
        _ => 0,
    }).sum();
    assert!(left <= total - 1200);
}

#[test]
fn research_a_technology() {
    let Some(mut run) = start(scenario("base").unwrap()) else { return };
    let drive = Technology::DriveFighter.index();
    // No docking bay yet: no lab for fighter drives.
    assert!(!hwc_sim::research::research_tech_if_lab_free(&mut run.universe, 0, drive));
    run_until(&mut run, 3000, |r| r.universe.base.players[0].labs[0].status == hwc_sim::base::lab_status::IDLE).expect("the docking bay is built");
    assert!(!hwc_sim::research::can_build_ship(&run.universe, 0, ShipType::sAcolyte, true));
    assert!(hwc_sim::research::research_tech_if_lab_free(&mut run.universe, 0, drive));
    let time = run.universe.statics.research.multiplayer_somtaaw.tech_time[drive];
    let start = run.tick();
    let (tick, _) = run_until(&mut run, 16 * (time as u32 + 5), |r| r.universe.base.players[0].tech.contains(drive)).expect("researched");
    // One second of research per 16 frames.
    assert!((tick - start) / 16 >= time as u32 && (tick - start) / 16 <= time as u32 + 2, "{} frames for {time} s", tick - start);
    assert!(hwc_sim::research::can_build_ship(&run.universe, 0, ShipType::sAcolyte, true));
    let ms = run.ids[0];
    assert_eq!(hwc_sim::construction::queue_build(&mut run.universe, ms, ShipType::sAcolyte, 2), 2);
}

fn link_scenario() -> Scenario {
    Scenario {
        name: "link",
        players: vec![ShipRace::Somtaaw],
        spawns: vec![
            Spawn { ship_type: ShipType::sAcolyte, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::sAcolyte, player: 0, position: Vector::new(600.0, 300.0, 0.0) },
        ],
        orders: vec![(0, Order::Link { a: 0, b: 1 })],
        resources: Vec::new(),
        resource_units: Vec::new(),
    }
}

#[test]
fn two_acolytes_link_into_an_acv() {
    let Some(mut run) = start(link_scenario()) else { return };
    let mut acv = None;
    let (_, h1) = run_until(&mut run, 2000, |r| {
        for e in events(r) {
            if let BaseEvent::Linked { new_ship } = e {
                acv = Some(*new_ship);
            }
        }
        acv.is_some()
    })
    .expect("linked");
    let u = &run.universe;
    let acv = u.ship(acv.unwrap()).unwrap();
    assert_eq!(acv.ship_type, ShipType::sACV);
    assert!(u.ship(run.ids[0]).is_none() && u.ship(run.ids[1]).is_none());
    assert_eq!(acv.health, u.statics.ship(ShipType::sACV).unwrap().health.max_health);
    // Deterministic.
    let mut again = start(link_scenario()).unwrap();
    let mut done = false;
    let (_, h2) = run_until(&mut again, 2000, |r| {
        done |= events(r).iter().any(|e| matches!(e, BaseEvent::Linked { .. }));
        done
    })
    .unwrap();
    assert_eq!(h1, h2);
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
    // The salvage technology, taken over by the workers; the derelict is damaged enough.
    let u = &mut run.universe;
    u.base.players[0].tech.add(Technology::Salvage.index());
    hwc_sim::upgrade::player_update_from_research(u, 0);
    for i in [1, 2] {
        hwc_sim::upgrade::apply_upgrades(u, run.ids[i]);
    }
    let t = run.ids[3];
    let max = u.statics.ship(ShipType::dIonArrayFrigate).unwrap().health.max_health;
    u.ships.ship_mut(t).health = max * 0.3;
    // A derelict does not run from the salvagers.
    u.ships.ship_mut(t).tactics = hwc_sim::object::tactics::NEUTRAL;
}

#[test]
fn salvage_a_derelict() {
    let Some(mut run) = start(salvage_scenario()) else { return };
    prepare_salvage(&mut run);
    assert!(hwc_sim::upgrade::has_upgrade(run.universe.ship(run.ids[1]).unwrap(), hwc_sim::upgrade::BIT_SALVAGE));
    let mut salvaged = None;
    let (tick, h1) = run_until(&mut run, 6000, |r| {
        for e in events(r) {
            if let BaseEvent::Salvaged { ship, .. } = e {
                salvaged = Some(*ship);
            }
        }
        salvaged.is_some()
    })
    .expect("salvaged");
    let u = &run.universe;
    let s = u.ship(salvaged.unwrap()).unwrap();
    assert_eq!(s.ship_type, ShipType::dIonArrayFrigate);
    assert_eq!(s.player, 0);
    assert!(u.ship(run.ids[3]).is_none());
    // gameStatsAddShipSalvaged writes [type + player * 8]; the getter reads [player * 145 + type]
    // (both index player 0 the same way).
    assert_eq!(u.base.stats.ships_salvaged(ShipType::dIonArrayFrigate, 0), 1);
    let mut again = start(salvage_scenario()).unwrap();
    prepare_salvage(&mut again);
    for _ in 0..tick {
        again.step();
    }
    assert_eq!(hwc_sim::harness::world_hash(&again.universe), h1);
}

#[test]
fn salvage_count_quirk_for_other_players() {
    let mut st = hwc_sim::base::GameStats::default();
    st.add_ship_salvaged(ShipType::sRecon, 1);
    // Player 1's count lands at type + 8 ...
    assert_eq!(st.ships_salvaged(ShipType::sRecon, 1), 0);
    assert_eq!(st.salvaged[8], 1);
    // ... which the getter reads as player 0's type 8.
    assert_eq!(st.ships_salvaged(ShipType::from_index(8).unwrap(), 0), 1);
}

fn message_pod_scenario() -> Scenario {
    let mut s = salvage_scenario();
    s.name = "message pod";
    s.spawns[3] = Spawn { ship_type: ShipType::dMessagePod, player: 1, position: Vector::new(6000.0, 0.0, 0.0) };
    s
}

fn prepare_message_pod(run: &mut Run) {
    let u = &mut run.universe;
    u.base.players[0].tech.add(Technology::Salvage.index());
    hwc_sim::upgrade::player_update_from_research(u, 0);
    for i in [1, 2] {
        hwc_sim::upgrade::apply_upgrades(u, run.ids[i]);
    }
}

/// Mission 2's message pod is not converted to the salvaging player as other derelicts are:
/// `salvDepotHouseKeep` hands pods and buoys to `salvDepotProcessDockIn` (0x4f50c0), which flies
/// the pod into a bay of the mothership, counts it as salvaged and deletes it, so it never joins
/// the fleet (nor carries over to the next mission).
#[test]
fn message_pod_docks_in_and_is_gone() {
    let Some(mut run) = start(message_pod_scenario()) else { return };
    prepare_message_pod(&mut run);
    let pod = run.ids[3];
    let mut converted = false;
    let (tick, h1) = run_until(&mut run, 9000, |r| {
        converted |= events(r).iter().any(|e| matches!(e, BaseEvent::Salvaged { .. }));
        r.universe.base.stats.ships_salvaged(ShipType::dMessagePod, 0) == 1
    })
    .expect("pod salvaged");
    let u = &run.universe;
    assert!(u.ship(pod).is_none());
    assert!(u.ships.iter().all(|s| s.ship_type != ShipType::dMessagePod), "no pod left in either fleet");
    assert!(!converted, "a pod is not converted");
    // The bay it went into is free again.
    let m = u.ship(run.ids[0]).unwrap();
    assert!(m.base.occupancy.as_ref().unwrap().reserved.iter().all(|r| !r));
    let mut again = start(message_pod_scenario()).unwrap();
    prepare_message_pod(&mut again);
    for _ in 0..tick {
        again.step();
    }
    assert_eq!(hwc_sim::harness::world_hash(&again.universe), h1);
}
