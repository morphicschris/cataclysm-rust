//! The Beast, crystals, processors, repair droids and worker repair over the real game data
//! (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped when neither is set): a Beast heavy cruiser
//! infects and converts an enemy frigate and its beam cascades through the frigate's escort;
//! a processor's beam heals a damaged ally; repair droids crawl out of a damaged processor; a
//! worker repairs an ally. Every scenario runs twice with identical hashes.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::ships::{beast, crystal, processor, repair_droid, worker_repair};
use hwc_sim::statics::{DataSource, DirSource, DockPointType, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::{ShipId, SimTweaks};

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

fn scenario(name: &'static str, players: Vec<ShipRace>, spawns: Vec<Spawn>, orders: Vec<(u32, Order)>) -> Scenario {
    Scenario { name, players, spawns, orders, resources: Vec::new(), resource_units: Vec::new() }
}

fn new_run(s: Scenario) -> Option<Run> {
    let d = data()?;
    Some(Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap())
}

/// Two runs of the same set-up give the same hashes.
fn deterministic(make: impl Fn() -> Option<Run>, ticks: u32) {
    let (Some(mut a), Some(mut b)) = (make(), make()) else { return };
    assert_eq!(a.run(ticks), b.run(ticks));
}

// --- the Beast

fn beast_attack() -> Scenario {
    let v = Vector::new;
    let mut spawns = vec![
        Spawn { ship_type: ShipType::bHeavyCruiser, player: 1, position: v(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: v(3000.0, 0.0, 0.0) },
    ];
    // The frigate's escort, within the cascade distance of it and of each other.
    for i in 0..3 {
        spawns.push(Spawn { ship_type: ShipType::sAcolyte, player: 0, position: v(3600.0 + i as f32 * 400.0, 500.0, 0.0) });
    }
    scenario("beast attack", vec![ShipRace::Somtaaw, ShipRace::Beast], spawns, vec![(2, Order::Special { ships: vec![0], targets: vec![1] })])
}

/// The heavy cruiser has the cascade upgrade, its infection guns are charged, and the Beast
/// has support units to spare.
fn armed_beast_run() -> Option<Run> {
    let mut run = new_run(beast_attack())?;
    let hc = run.ids[0];
    let u = &mut run.universe;
    u.base.players[1].su_capacity = 10_000;
    let s = u.ships.ship_mut(hc);
    s.upgrade_flags |= beast::UPGRADE_CASCADE;
    for g in &mut s.guns {
        if g.level == hwc_sim::bullet::weapon::BEASTINFECT {
            g.last_time_fired = -1000.0;
        }
    }
    Some(run)
}

#[test]
fn heavy_cruiser_infects_and_converts_an_enemy() {
    let Some(mut run) = armed_beast_run() else { return };
    let (hc, frigate) = (run.ids[0], run.ids[1]);
    assert!(run.universe.ships.ship(hc).beast.cascade.is_some(), "cascade set up at creation");
    let mut infected_at = None;
    for t in 0..2400 {
        run.step();
        if infected_at.is_none() && run.universe.ships.get(frigate).is_some_and(|s| s.beast.timer != 0.0) {
            infected_at = Some(t);
            let s = run.universe.ships.ship(frigate);
            assert_eq!(s.beast.infector, Some(hc));
            assert_eq!(s.beast.player, Some(1));
            assert!(s.flags & hwc_sim::object::sof::DISABLED != 0, "an infected ship is disabled");
            assert_eq!(s.beast.attacks, 1);
        }
        if run.universe.ships.get(frigate).is_none() {
            break;
        }
    }
    let at = infected_at.expect("the frigate was infected");
    assert!(run.universe.ships.get(frigate).is_none(), "the infected frigate was replaced");
    let converted: Vec<_> = run.universe.ships.iter().filter(|s| s.ship_type == ShipType::bMultiBeamFrigate).collect();
    assert_eq!(converted.len(), 1);
    let n = converted[0];
    assert_eq!(n.player, 1, "the Beast owns the converted ship");
    assert!(n.special_flags2 & beast::special2::BEASTIFIED != 0);
    assert_eq!(beast::ships_beastified(&run.universe, Some(ShipType::bMultiBeamFrigate), 0), 0);
    // The original's index (type + player * 8) puts player 1's count 8 types on.
    assert_eq!(run.universe.base.stats.beastified[ShipType::bMultiBeamFrigate.index() + 8], 1);
    // The special order ended when the gun fired.
    assert!(run.universe.ships.ship(hc).command.is_none());
    eprintln!("infected at tick {at}");
}

#[test]
fn infection_beam_cascades_through_the_escort() {
    let Some(mut run) = armed_beast_run() else { return };
    let hc = run.ids[0];
    let escort = [run.ids[2], run.ids[3], run.ids[4]];
    let mut max_entries = 0;
    let mut infected = std::collections::BTreeSet::new();
    for _ in 0..1200 {
        run.step();
        let c = run.universe.ships.ship(hc).beast.cascade.as_ref().unwrap();
        max_entries = max_entries.max(c.entries.len());
        for &e in &escort {
            if run.universe.ships.get(e).is_some_and(|s| s.beast.timer != 0.0) {
                infected.insert(e);
            }
        }
    }
    assert!(max_entries >= 2, "the beam jumped on ({max_entries} beams)");
    assert!(infected.len() >= 2, "the escort got infected: {infected:?}");
    // All beams are gone once their bullets are.
    assert!(run.universe.ships.ship(hc).beast.cascade.as_ref().unwrap().entries.iter().all(|e| e.bullet.is_some()));
}

#[test]
fn beast_runs_are_deterministic() {
    deterministic(armed_beast_run, 900);
}

#[test]
fn no_cascade_without_the_upgrade_in_multiplayer() {
    let Some(mut run) = new_run(beast_attack()) else { return };
    let (hc, frigate) = (run.ids[0], run.ids[1]);
    run.run(200);
    assert!(run.universe.ships.ship(hc).beast.cascade.as_ref().unwrap().target.is_none());
    assert_eq!(run.universe.ships.ship(frigate).beast.timer, 0.0);
}

#[test]
fn kas_beastify_infects_or_leaves_for_scuttling() {
    let Some(mut run) = new_run(beast_attack()) else { return };
    let (hc, frigate) = (run.ids[0], run.ids[1]);
    let rest = beast::kas_ships_beastify(&mut run.universe, &[frigate, hc], hc);
    assert_eq!(rest, vec![hc], "the heavy cruiser has no infection statistics");
    assert!(run.universe.ships.ship(frigate).beast.timer > 0.0);
    assert!(beast::kas_infection_terminal(&run.universe, &[frigate]));
}

#[test]
fn infection_stats_from_the_ship_files() {
    let Some(d) = data() else { return };
    let s = d.statics.ship(ShipType::sAcolyte).unwrap();
    assert_eq!(beast::infection_stats(s), Some((2.0, 1, Some(ShipType::bAcolyte))));
    assert!(beast::infection_stats(d.statics.ship(ShipType::bHeavyCruiser).unwrap()).is_none());
    assert_eq!(d.tweaks.base.beast_cascade_points, 20.0);
    assert_eq!(d.tweaks.base.beast_cascade_distance, 2000.0);
}

// --- processors

/// A processor with the auto-repair upgrade and a damaged frigate in front of its first
/// repair beam.
fn processor_run() -> Option<Run> {
    let d = data()?;
    let stat = d.statics.ship(ShipType::sProcessor)?;
    let dock = stat.dock.as_ref()?;
    let t = DockPointType::AutoRepair.index();
    let p = &dock.points[dock.first_of_type[t] as usize];
    let local = Vector::new(p.position.x + p.cone_normal.x * 900.0, p.position.y + p.cone_normal.y * 900.0, p.position.z + p.cone_normal.z * 900.0);
    let world = hwc_sim::universe::DEFAULT_SHIP_MATRIX.mul_vec(local);
    let s = scenario(
        "processor repair",
        vec![ShipRace::Somtaaw],
        vec![
            Spawn { ship_type: ShipType::sProcessor, player: 0, position: Vector::ZERO },
            Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: world },
        ],
        Vec::new(),
    );
    let mut run = new_run(s)?;
    let (pr, fr) = (run.ids[0], run.ids[1]);
    let u = &mut run.universe;
    u.ships.ship_mut(pr).upgrade_flags |= processor::UPGRADE_AUTO_REPAIR;
    let max = u.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health;
    u.ships.ship_mut(fr).health = max * 0.5;
    Some(run)
}

#[test]
fn processor_beam_heals_a_damaged_ally() {
    let Some(mut run) = processor_run() else { return };
    let (pr, fr) = (run.ids[0], run.ids[1]);
    assert_eq!(processor::beams(&run.universe, pr).len(), 4, "four DPT_AutoRepair points");
    let before = run.universe.ships.ship(fr).health;
    run.run(16 * 5);
    let b = processor::beams(&run.universe, pr);
    assert!(b.iter().any(|b| b.target == Some(fr)), "a beam took the frigate: {b:?}");
    let after = run.universe.ships.ship(fr).health;
    let rate = run.universe.tweaks.base.repair_rate[hwc_sim::statics::ShipClass::Frigate.index()];
    assert!(rate > 0.0);
    // Every beam that reaches the frigate repairs it (the four repair points share one
    // position): about five seconds of the frigate rate per beam, less the first updates
    // looking for it.
    let on = b.iter().filter(|b| b.target == Some(fr)).count() as f32;
    let healed = after - before;
    assert!(healed > rate * on * 4.0 && healed <= rate * on * 5.0 + 1.0, "healed {healed} (rate {rate}, {on} beams)");
    // Whole again: the beam lets go.
    let max = run.universe.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health;
    run.universe.ships.ship_mut(fr).health = max;
    run.run(2);
    assert!(processor::beams(&run.universe, pr).iter().all(|b| b.target.is_none()));
}

#[test]
fn processor_without_upgrade_does_not_heal() {
    let Some(mut run) = processor_run() else { return };
    let (pr, fr) = (run.ids[0], run.ids[1]);
    run.universe.ships.ship_mut(pr).upgrade_flags &= !processor::UPGRADE_AUTO_REPAIR;
    let before = run.universe.ships.ship(fr).health;
    run.run(48);
    assert_eq!(run.universe.ships.ship(fr).health, before);
}

#[test]
fn processor_runs_are_deterministic() {
    deterministic(processor_run, 160);
}

// --- repair droids

fn droid_run() -> Option<Run> {
    let s = scenario("droids", vec![ShipRace::Somtaaw], vec![Spawn { ship_type: ShipType::sProcessor, player: 0, position: Vector::ZERO }], Vec::new());
    let mut run = new_run(s)?;
    let pr = run.ids[0];
    let u = &mut run.universe;
    u.base.options.repair_bots = true;
    let max = u.statics.ship(ShipType::sProcessor).unwrap().health.max_health;
    let s = u.ships.ship_mut(pr);
    s.upgrade_flags |= hwc_sim::upgrade::BIT_REPAIR;
    s.health = max * 0.5;
    Some(run)
}

#[test]
fn repair_droids_leave_a_damaged_ship_that_regenerates() {
    let Some(mut run) = droid_run() else { return };
    let pr = run.ids[0];
    let before = run.universe.ships.ship(pr).health;
    run.run(16 * 6);
    let s = run.universe.ships.ship(pr);
    let arr = s.droids.as_ref().expect("the processor has droids");
    assert_eq!(arr.droids.len(), 5);
    assert!(arr.droids.iter().filter(|d| d.state != repair_droid::state::DOCKED).count() >= 2, "droids launched: {:?}", arr.droids.iter().map(|d| d.state).collect::<Vec<_>>());
    assert!(s.health > before, "the ship regenerates");
    // Whole again: the droids head home and dock.
    let max = run.universe.statics.ship(ShipType::sProcessor).unwrap().health.max_health;
    run.universe.ships.ship_mut(pr).health = max;
    // (The regeneration stops updating the array once the ship is whole, as in the original;
    // drive it directly.)
    for _ in 0..16 * 20 {
        repair_droid::update_array(&mut run.universe, pr, None, false);
    }
    let arr = run.universe.ships.ship(pr).droids.as_ref().unwrap();
    assert!(arr.droids.iter().all(|d| matches!(d.state, repair_droid::state::RETURNING | repair_droid::state::DOCKING | repair_droid::state::DOCKED)));
}

#[test]
fn droid_runs_are_deterministic() {
    deterministic(droid_run, 160);
}

// --- worker repair

fn worker_run() -> Option<Run> {
    let s = scenario(
        "worker repair",
        vec![ShipRace::Somtaaw],
        vec![
            Spawn { ship_type: ShipType::sWorker, player: 0, position: Vector::ZERO },
            Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: Vector::new(2500.0, 0.0, 0.0) },
        ],
        Vec::new(),
    );
    let mut run = new_run(s)?;
    let (w, fr) = (run.ids[0], run.ids[1]);
    let max = run.universe.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health;
    run.universe.ships.ship_mut(fr).health = max * 0.5;
    worker_repair::cl_repair(&mut run.universe, &[w], &[fr]);
    Some(run)
}

#[test]
fn worker_repairs_a_damaged_ally() {
    let Some(mut run) = worker_run() else { return };
    let (w, fr) = (run.ids[0], run.ids[1]);
    assert!(worker_repair::is_repairing(&run.universe, w));
    let before = run.universe.ships.ship(fr).health;
    let mut repairing = false;
    for _ in 0..16 * 60 {
        run.step();
        if run.universe.ships.ship(w).worker_repair.state == worker_repair::state::REPAIRING {
            repairing = true;
        }
        let ws = run.universe.ships.ship(w).worker_repair;
        if ws.target == Some(fr) {
            assert_eq!(run.universe.ships.ship(fr).worker_repair.repairers, 1);
        }
    }
    assert!(repairing, "the worker reached the repair state");
    assert!(run.universe.ships.ship(fr).health > before, "the frigate was repaired");
}

#[test]
fn worker_runs_are_deterministic() {
    deterministic(worker_run, 400);
}

// --- crystals

#[test]
fn crystals_are_unselectable_resource_holders() {
    let Some(d) = data() else { return };
    let s = scenario("crystal", vec![ShipRace::Somtaaw], vec![Spawn { ship_type: ShipType::dCrystal0, player: 0, position: Vector::ZERO }], Vec::new());
    let Some(run) = new_run(s) else { return };
    let c: ShipId = run.ids[0];
    assert!(crystal::is_crystal(&run.universe, c));
    let s = run.universe.ships.ship(c);
    assert_eq!(s.flags & hwc_sim::object::sof::SELECTABLE, 0);
    let value = crystal::spec(&run.universe, c).unwrap().resources;
    let file = d.statics.ship(ShipType::dCrystal0).unwrap().custom_entries.iter().find(|(k, _)| k == "resourceValue").map(|(_, v)| v.split_whitespace().next().unwrap().parse::<i32>().unwrap());
    assert_eq!(Some(value), file);
}

/// The infection beam draws `eboltCascade.ebg` (the Beast weapon's bullet effect), and
/// `beastCascadeEffectUpdate` keeps the effect's `length` parameter at the distance from the
/// beam's source to its target, so the ribbons reach the infected ship.
#[test]
fn infection_beam_effect_spans_source_to_target() {
    let Some(mut run) = armed_beast_run() else { return };
    let src: Box<dyn DataSource> = if let Ok(dir) = std::env::var("HWC_EXTRACTED_DIR") {
        Box::new(DirSource::new(dir).unwrap())
    } else {
        Box::new(BigArchive::open(std::path::Path::new(&std::env::var("HWC_GAME_DIR").unwrap()).join("cataclysm.big")).unwrap())
    };
    let data = hwc_sim::effects::EffectData::load_from(src.as_ref());
    run.universe.effects = Some(Box::new(hwc_sim::effects::Effects::new(&data)));
    let mut checked = 0;
    for _ in 0..1200 {
        run.step();
        let u = &run.universe;
        let fx = u.effects.as_ref().unwrap();
        for &bid in &u.bullet_list {
            let b = u.bullets.bullet(bid);
            if b.weapon != hwc_sim::bullet::weapon::BEASTINFECT {
                continue;
            }
            let e = fx.bullet_effect.get(&bid).and_then(|&e| fx.world.effect(e)).expect("the beam has its effect");
            let l = b.length_vec;
            let d = (l.x * l.x + l.y * l.y + l.z * l.z).sqrt();
            let len = f32::from_bits(e.variables[2]);
            // (A beam fired this update has not been through its effect update yet.)
            if b.time_lived > 0.0 && d > 1.0 {
                assert!((len - d).abs() < 0.01 * d, "effect length {len} vs beam {d}");
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "a beam was seen");
}

#[test]
fn heavy_corvette_fires_a_burst_at_the_middle_of_its_targets() {
    use hwc_sim::ships::heavy_corvette::state;
    use hwc_sim::ships::ShipSpec;
    let v = Vector::new;
    let spawns = vec![
        Spawn { ship_type: ShipType::bHeavyCorvette, player: 0, position: v(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::tResourceCollector, player: 1, position: v(9000.0, 0.0, 0.0) },
    ];
    let orders = vec![(1, Order::Special { ships: vec![0], targets: vec![1] })];
    let Some(mut run) = new_run(scenario("burst", vec![ShipRace::Beast, ShipRace::Taiidan], spawns, orders)) else { return };
    let id = run.ids[0];
    let (mut bursts, mut cooled, mut flew) = (0, false, false);
    for _ in 0..1600 {
        run.step();
        bursts = bursts.max(run.universe.bullets.iter().filter(|b| b.burst).count());
        if let ShipSpec::HeavyCorvette(c) = &run.universe.ships.ship(id).spec {
            flew |= c.burst_state == state::FLY_TO_FIRE_POINT;
            cooled |= c.cooldown;
        }
    }
    assert!(flew, "the corvette flies to its firing point");
    assert!(bursts >= 2, "both guns fire a burst bullet, saw {bursts}");
    assert!(cooled, "the guns cool after a burst");
    assert!(run.universe.ships.ship(id).posinfo.position.x > 3000.0, "it closed to burst range");
}
