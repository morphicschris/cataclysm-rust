//! Every Somtaaw and Beast ship type over the real game data (`HWC_GAME_DIR` or
//! `HWC_EXTRACTED_DIR`; skipped when neither is set): each is created with every upgrade its
//! type can have and put through attack, special, halt and dock orders. Multiplayer can field
//! any of them, so none may panic, and every armed type must hurt what it is told to attack.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipClass, ShipRace, ShipType, StaticWorld};
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

fn run_for(d: &Data, race: ShipRace, t: ShipType, orders: Vec<(u32, Order)>, ticks: u32) -> (f32, f32, bool) {
    let carrier = if race == ShipRace::Beast { ShipType::bCarrier } else { ShipType::sCarrier };
    let spawns = vec![
        Spawn { ship_type: t, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::tStandardDestroyer, player: 1, position: Vector::new(3500.0, 0.0, 0.0) },
        Spawn { ship_type: carrier, player: 0, position: Vector::new(-4000.0, 0.0, 0.0) },
    ];
    let s = Scenario {
        name: "fleet",
        players: vec![race, ShipRace::Taiidan],
        spawns,
        orders,
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let id = run.ids[0];
    let tg = run.ids[1];
    let mask = hwc_sim::upgrade::type_mask(t);
    run.universe.ships.ship_mut(id).upgrade_flags |= mask;
    let start = run.universe.ships.get(tg).map_or(0.0, |s| s.health);
    run.run(ticks);
    let hurt = run.universe.ships.get(tg).map_or(start, |s| start - s.health);
    let alive = run.universe.ships.get(id).is_some();
    let moved = run.universe.ships.get(id).map_or(0.0, |s| s.posinfo.position.x);
    (hurt, moved, alive)
}

#[test]
fn every_somtaaw_and_beast_type_takes_orders() {
    let Some(d) = data() else { return };
    let mut mute = Vec::new();
    for race in [ShipRace::Somtaaw, ShipRace::Beast] {
        for t in race.ship_types() {
            let Some(st) = d.statics.ship(t) else {
                println!("{t:?}: no ship data");
                continue;
            };
            let class = st.ship_class;
            let armed = st.guns.as_ref().is_some_and(|g| !g.is_empty());
            let module = hwc_sim::ships::module_of(t);
            let attack = vec![(1, Order::Attack { ships: vec![0], targets: vec![1] })];
            let (hurt, x, alive) = run_for(d, race, t, attack, 1600);
            let special = vec![
                (1, Order::Special { ships: vec![0], targets: vec![] }),
                (200, Order::Special { ships: vec![0], targets: vec![1] }),
                (500, Order::Halt { ships: vec![0] }),
                (520, Order::Dock { ships: vec![0], purpose: hwc_sim::dock::purpose::DOCK, with: Some(2) }),
            ];
            let _ = run_for(d, race, t, special, 900);
            println!("{t:?} {class:?} {module:?} armed={armed} hurt={hurt:.0} x={x:.0} alive={alive}");
            let component = class == Some(ShipClass::Component);
            if armed && !component && hurt <= 0.0 && alive {
                mute.push(t);
            }
        }
    }
    println!("armed but did no damage: {mute:?}");
}

/// A depot with workers set to salvage, the target a little way off.
fn salvage_run(d: &Data, race: ShipRace, depot: ShipType, worker: ShipType, target: ShipType, owner: usize) -> Run {
    let mut spawns = vec![
        Spawn { ship_type: depot, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: target, player: owner, position: Vector::new(1500.0, 0.0, 0.0) },
    ];
    for i in 0..4 {
        spawns.push(Spawn { ship_type: worker, player: 0, position: Vector::new(800.0, 200.0 * i as f32, 0.0) });
    }
    let orders = vec![(1, Order::Salvage { ships: vec![2, 3, 4, 5], targets: vec![1] })];
    let s = Scenario {
        name: "salvage",
        players: vec![race, ShipRace::Taiidan],
        spawns,
        orders,
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    for i in 0..run.ids.len() {
        let id = run.ids[i];
        if i != 1 {
            let t = run.universe.ships.ship(id).ship_type;
            run.universe.ships.ship_mut(id).upgrade_flags |= hwc_sim::upgrade::type_mask(t);
        }
    }
    run
}

/// Runs until the depot takes the target in; the slot's state.
fn until_in_depot(run: &mut Run, ticks: u32) -> bool {
    let (depot, target) = (run.ids[0], run.ids[1]);
    for _ in 0..ticks {
        run.step();
        let held = run.universe.ships.ship(depot).base.depot.as_ref().is_some_and(|d| d.iter().any(|s| s.ship == Some(target)));
        if held {
            return true;
        }
    }
    false
}

#[test]
fn a_depot_eats_a_crystal_for_its_resources() {
    let Some(d) = data() else { return };
    let mut run = salvage_run(d, ShipRace::Somtaaw, ShipType::sCarrier, ShipType::sWorker, ShipType::dCrystal0, 1);
    run.universe.base.options.flags |= 0x20;
    let (depot, crystal) = (run.ids[0], run.ids[1]);
    run.universe.ships.ship_mut(crystal).player = 1;
    assert!(until_in_depot(&mut run, 16 * 600), "the workers bring the crystal in");
    let before = run.universe.base.players[0].resource_units;
    let mut consumed = false;
    for _ in 0..16 * 15 {
        run.step();
        if run.universe.ships.get(crystal).is_none() {
            break;
        }
        consumed |= hwc_sim::consume::is_active(&run.universe, crystal);
    }
    assert!(consumed, "a beam eats the crystal");
    assert!(run.universe.ships.get(crystal).is_none(), "the crystal is gone after ten seconds");
    assert!(!run.universe.ships.ship(depot).base.depot.as_ref().unwrap().iter().any(|s| s.ship.is_some()), "the slot is free");
    let got = run.universe.base.players[0].resource_units - before;
    // The last payment is the one before the beam finishes, as in the original.
    assert!((5900..=6000).contains(&got), "its resource value, in pieces: {got}");
}

#[test]
fn a_beast_depot_infects_what_it_salvages() {
    let Some(d) = data() else { return };
    let mut run = salvage_run(d, ShipRace::Beast, ShipType::bCarrier, ShipType::bWorker, ShipType::sMultiBeamFrigate, 1);
    let target = run.ids[1];
    run.universe.ships.ship_mut(target).health = 1.0;
    assert!(until_in_depot(&mut run, 16 * 600), "the workers bring it in");
    let mut infected = false;
    for _ in 0..16 * 30 {
        run.step();
        match run.universe.ships.get(target) {
            Some(s) => infected |= s.flags & hwc_sim::object::sof::CRAZY != 0,
            None => break,
        }
    }
    assert!(infected, "the depot infects it rather than converting it");
}

#[test]
fn a_big_gun_unit_goes_into_the_depot_and_is_counted() {
    let Some(d) = data() else { return };
    let mut run = salvage_run(d, ShipRace::Somtaaw, ShipType::sCarrier, ShipType::sWorker, ShipType::dBigGunUnit, 1);
    let target = run.ids[1];
    assert!(until_in_depot(&mut run, 16 * 600), "the workers bring it in");
    run.run(16 * 60);
    assert!(run.universe.ships.get(target).is_none(), "the unit is gone, not converted");
    assert_eq!(run.universe.base.stats.ships_salvaged(ShipType::dBigGunUnit, 0), 1);
}

#[test]
fn ships_enter_the_slipgates_they_fit() {
    use hwc_sim::player_orders::{mr_dock_at_slipgate, mr_enter_nearest_slipgate};
    let Some(d) = data() else { return };
    let spawns = vec![
        Spawn { ship_type: ShipType::sAcolyte, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::sCarrier, player: 0, position: Vector::new(0.0, 3000.0, 0.0) },
        Spawn { ship_type: ShipType::dSlipgateSmall, player: 0, position: Vector::new(4000.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::dSlipgateLarge, player: 0, position: Vector::new(-9000.0, 0.0, 0.0) },
    ];
    let s = Scenario { name: "gates", players: vec![ShipRace::Somtaaw], spawns, orders: Vec::new(), resources: Vec::new(), resource_units: Vec::new() };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let (acolyte, carrier, small, large) = (run.ids[0], run.ids[1], run.ids[2], run.ids[3]);
    // The nearest gate is the small one, which only the Acolyte fits.
    assert_eq!(mr_enter_nearest_slipgate(&mut run.universe, 0, &[acolyte, carrier], false), Some(small));
    assert_eq!(run.universe.ships.ship(acolyte).base.dock_with, Some(small));
    assert_eq!(run.universe.ships.ship(carrier).base.dock_with, None);
    // The large one takes them both.
    mr_dock_at_slipgate(&mut run.universe, &[acolyte, carrier], large, false);
    assert_eq!(run.universe.ships.ship(carrier).base.dock_with, Some(large));
    // The small one says the carrier does not fit.
    run.universe.messages.clear();
    mr_dock_at_slipgate(&mut run.universe, &[carrier], small, false);
    assert_eq!(run.universe.messages.first().map(|m| m.key), Some("strNotAllShipsCanEnterSlipgate"));
    // Mission 15's are out of bounds for the key.
    assert_eq!(mr_enter_nearest_slipgate(&mut run.universe, 0, &[acolyte], true), None);
}

/// A Beast player from scratch, as multiplayer gives one: with its research done, each type of the
/// Beast's build table comes out of a mothership.
#[test]
fn a_beast_player_builds_the_whole_fleet() {
    use hwc_sim::construction::{build_table, queue_build};
    let Some(d) = data() else { return };
    let table = build_table(ShipRace::Beast, true);
    assert!(!table.is_empty(), "the Beast has a build table");
    let mut failed = Vec::new();
    for (flag, t) in table.iter().copied().filter(|(f, _)| *f != 0) {
        let t = ShipType::from_index(usize::from(t)).unwrap();
        let spawns = vec![Spawn { ship_type: ShipType::bMothership, player: 0, position: Vector::new(0.0, 0.0, 0.0) }];
        let s = Scenario { name: "beast", players: vec![ShipRace::Beast], spawns, orders: Vec::new(), resources: Vec::new(), resource_units: Vec::new() };
        let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
        let ms = run.ids[0];
        run.universe.base.players[0].resource_units = 10_000_000;
        // (A cruiser needs more support than a bare mothership has.)
        run.universe.base.players[0].su_capacity = 1000;
        for tech in 0..hwc_sim::base::NUM_TECHS {
            hwc_sim::fow::tech_add(&mut run.universe, 0, tech);
        }
        if flag == 2 {
            run.universe.base.players[0].acquired[t.index()] = true;
        }
        if queue_build(&mut run.universe, ms, t, 1) == 0 {
            failed.push((t, "refused"));
            continue;
        }
        let mut built = false;
        for _ in 0..16 * 600 {
            run.step();
            built = run.universe.ships.iter().any(|s| s.ship_type == t && s.special_flags2 & hwc_sim::base::special2::UNDER_CONSTRUCTION == 0);
            if built {
                break;
            }
        }
        if !built {
            failed.push((t, "never finished"));
        }
    }
    assert!(failed.is_empty(), "{failed:?}");
}

#[test]
fn every_animated_type_loads_its_animations() {
    let Some(d) = data() else { return };
    for t in [ShipType::sWorker, ShipType::bWorker, ShipType::sCarrier, ShipType::sAcolyte, ShipType::sLeech, ShipType::bIonArrayFrigate, ShipType::sSentinel, ShipType::sHiveFrigate] {
        let m = d.statics.ship(t).and_then(|s| s.mad.as_ref());
        assert!(m.is_some(), "{t:?} has no animations");
    }
    let ion = d.statics.ship(ShipType::bIonArrayFrigate).and_then(|s| s.mad.as_ref()).unwrap();
    assert_eq!(ion.gun_open.len(), 1, "the ion array's guns open");
    assert!(ion.start_placed.is_some());
    let w = d.statics.ship(ShipType::sWorker).and_then(|s| s.mad.as_ref()).unwrap();
    assert_eq!(w.remap.as_ref().map(Vec::len), Some(6));
    assert!(w.remap.as_ref().unwrap().iter().all(|&i| i >= 0), "{:?}", w.remap);
}

fn one_run(d: &Data, race: ShipRace, spawns: Vec<Spawn>, orders: Vec<(u32, Order)>) -> Run {
    let s = Scenario { name: "one", players: vec![race, ShipRace::Taiidan], spawns, orders, resources: Vec::new(), resource_units: Vec::new() };
    Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap()
}

/// The ion array frigate's guns open before its first shot, and close when it stops attacking.
#[test]
fn ion_array_guns_open_before_they_fire() {
    use hwc_sim::mad::gun;
    let Some(d) = data() else { return };
    let spawns = vec![
        Spawn { ship_type: ShipType::bIonArrayFrigate, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::tResourceCollector, player: 1, position: Vector::new(2000.0, 0.0, 0.0) },
    ];
    let orders = vec![(1, Order::Attack { ships: vec![0], targets: vec![1] }), (16 * 40, Order::Halt { ships: vec![0] })];
    let mut run = one_run(d, ShipRace::Beast, spawns, orders);
    let id = run.ids[0];
    let (mut opening, mut fired_while_shut, mut fired) = (false, false, false);
    for _ in 0..16 * 39 {
        run.step();
        let s = run.universe.ships.ship(id);
        let state = s.base.mad.map_or(0, |m| m.gun_state);
        opening |= state == gun::OPENING;
        let shot = s.guns.iter().any(|g| g.last_time_fired > 0.0);
        fired_while_shut |= shot && !fired && state != gun::OPEN;
        fired |= shot;
    }
    assert!(opening, "the guns open");
    assert!(fired, "and fire");
    assert!(!fired_while_shut, "only once open");
    run.run(16 * 10);
    let state = run.universe.ships.ship(id).base.mad.map_or(0, |m| m.gun_state);
    assert!(matches!(state, gun::CLOSING | gun::CLOSED), "they close after, {state}");
}

/// A ship leaving a Somtaaw carrier's bay waits for the doors, which shut again later.
#[test]
fn carrier_doors_open_for_the_bay_and_shut_after() {
    let Some(d) = data() else { return };
    let spawns = vec![
        Spawn { ship_type: ShipType::sCarrier, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: Vector::new(2500.0, 0.0, 0.0) },
    ];
    let orders = vec![(1, Order::Dock { ships: vec![1], purpose: hwc_sim::dock::purpose::DOCK, with: Some(0) })];
    let mut run = one_run(d, ShipRace::Somtaaw, spawns, orders);
    let carrier = run.ids[0];
    let (mut opened, mut shut_again) = (false, false);
    for _ in 0..16 * 180 {
        run.step();
        let m = run.universe.ships.ship(carrier).base.mad;
        let closed = m.is_none_or(|m| m.door_closed);
        shut_again |= opened && closed;
        opened |= !closed;
    }
    assert!(opened, "the doors open");
    assert!(shut_again, "and shut after");
}

/// An ACV unlinks by turning, then two Acolytes play the split side by side and go free, with the
/// damage the ACV took shared between them.
#[test]
fn an_acv_turns_then_splits_into_two_acolytes() {
    let Some(d) = data() else { return };
    let spawns = vec![Spawn { ship_type: ShipType::sACV, player: 0, position: Vector::new(0.0, 0.0, 0.0) }];
    let mut run = one_run(d, ShipRace::Somtaaw, spawns, Vec::new());
    let acv = run.ids[0];
    let max = d.statics.ship(ShipType::sACV).unwrap().health.max_health;
    run.universe.ships.ship_mut(acv).health = max * 0.5;
    hwc_sim::link::cl_unlink(&mut run.universe, acv);
    let mut steps = 0;
    while run.universe.ships.get(acv).is_some() {
        run.step();
        steps += 1;
        assert!(steps < 16 * 60, "the ACV never split");
    }
    let parts: Vec<_> = run.universe.ships.iter().filter(|s| s.ship_type == ShipType::sAcolyte).map(|s| s.id).collect();
    assert_eq!(parts.len(), 2);
    let pmax = d.statics.ship(ShipType::sAcolyte).unwrap().health.max_health;
    for &p in &parts {
        let s = run.universe.ships.ship(p);
        assert!(s.health < pmax, "shares the damage: {} of {pmax}", s.health);
        assert_ne!(s.base.link.state, 0, "in the split");
    }
    let split_started = hwc_sim::mad::current(&run.universe, parts[0]);
    assert_eq!(split_started, 3, "plays Unlink");
    let dist = |r: &Run| (r.universe.ships.ship(parts[0]).posinfo.position - r.universe.ships.ship(parts[1]).posinfo.position).mag_sq().sqrt();
    let apart = dist(&run);
    assert!(apart > 0.0);
    run.run(16 * 4);
    for &p in &parts {
        assert_eq!(run.universe.ships.ship(p).base.link.state, 0, "free after the split");
    }
    assert!((dist(&run) - apart).abs() < apart, "they stayed side by side until then");
}
