//! The missile-carrying special ships over the real game data (`HWC_GAME_DIR` or
//! `HWC_EXTRACTED_DIR`; skipped when neither is set): Acolyte link missiles, the ACV's EMP,
//! the minelayer's attack runs and mine walls, and missile-destroyer volleys with missile
//! regeneration. Every scenario also runs twice and must hash the same.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::object::ShipId;
use hwc_sim::ships::{acv, minelayer, ShipSpec};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::universe::{SpecialEvent, Universe};
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

fn v(x: f32, y: f32, z: f32) -> Vector {
    Vector::new(x, y, z)
}

fn spawn(ship_type: ShipType, player: usize, position: Vector) -> Spawn {
    Spawn { ship_type, player, position }
}

fn scenario(name: &'static str, players: Vec<ShipRace>, spawns: Vec<Spawn>, orders: Vec<(u32, Order)>) -> Scenario {
    Scenario { name, players, spawns, orders, resources: Vec::new(), resource_units: Vec::new() }
}

/// A run: the setup hook changes the universe after the ships are placed (upgrades), `watch`
/// sees every update.
fn run_with(statics: Arc<StaticWorld>, s: Scenario, ticks: u32, setup: &dyn Fn(&mut Run), watch: &mut dyn FnMut(&Run, &[SpecialEvent])) -> (Vec<u64>, Run) {
    let d = data().expect("data");
    let mut run = Run::new(statics, d.tweaks.clone(), s).unwrap();
    setup(&mut run);
    let mut hashes = Vec::new();
    for _ in 0..ticks {
        hashes.push(run.step());
        let ev = run.universe.special_events.clone();
        watch(&run, &ev);
    }
    (hashes, run)
}

/// Runs a scenario twice; the hashes must agree.
fn twice(statics: Arc<StaticWorld>, s: Scenario, ticks: u32, setup: &dyn Fn(&mut Run), watch: &mut dyn FnMut(&Run, &[SpecialEvent])) -> Run {
    let (a, run) = run_with(statics.clone(), s.clone(), ticks, setup, watch);
    let (b, _) = run_with(statics, s, ticks, setup, &mut |_, _| {});
    assert_eq!(a, b, "the two runs differ");
    run
}

fn upgrade(run: &mut Run, ships: &[usize], bit: u32) {
    for &i in ships {
        let id = run.ids[i];
        run.universe.ships.ship_mut(id).upgrade_flags |= bit;
    }
}

fn missiles_left(u: &Universe, id: ShipId, guns: &[usize]) -> i32 {
    u.ships.get(id).map_or(0, |s| guns.iter().map(|&g| s.guns[g].num_missiles).sum())
}

/// Two Acolytes with the link-missile upgrade are ordered to link-attack a frigate: they
/// attack it together, open their gun pods and launch all four link missiles (launchers 2
/// and 3) once within 4 km; the missiles hit.
#[test]
fn acolyte_pair_launches_link_missiles() {
    let Some(d) = data() else { return };
    let s = scenario(
        "acolyte link missiles",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            spawn(ShipType::sAcolyte, 0, v(-6000.0, 0.0, 0.0)),
            spawn(ShipType::sAcolyte, 0, v(-6000.0, 300.0, 0.0)),
            spawn(ShipType::tStandardFrigate, 1, v(2000.0, 0.0, 0.0)),
        ],
        vec![(1, Order::Special { ships: vec![0, 1], targets: vec![2] })],
    );
    let setup = |run: &mut Run| upgrade(run, &[0, 1], hwc_sim::ships::acolyte::UPGRADE_LINK_MISSILES);
    let mut link_fired = 0;
    let mut hits = 0;
    let mut linked_seen = false;
    let run = twice(d.statics.clone(), s, 1600, &setup, &mut |run, ev| {
        for e in ev {
            match e {
                SpecialEvent::MissileFired { gun, .. } if *gun >= 2 => link_fired += 1,
                SpecialEvent::MissileHit { .. } => hits += 1,
                _ => {}
            }
        }
        if let Some(ShipSpec::Acolyte(a)) = run.universe.ships.get(run.ids[0]).map(|s| &s.spec) {
            linked_seen |= a.link_attack && a.link_target == Some(run.ids[2]);
        }
    });
    let u = &run.universe;
    println!("link missiles fired {link_fired}, hits {hits}");
    assert!(linked_seen, "the special order started a link attack");
    assert_eq!(link_fired, 4, "both Acolytes launched both link missiles");
    assert!(hits >= 1, "link missiles hit");
    for i in 0..2 {
        assert_eq!(missiles_left(u, run.ids[i], &[2, 3]), 0);
    }
}

/// Without the upgrade the same order fires no link missiles (the Acolytes still attack).
#[test]
fn acolyte_without_upgrade_keeps_link_missiles() {
    let Some(d) = data() else { return };
    let s = scenario(
        "acolyte no upgrade",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![spawn(ShipType::sAcolyte, 0, v(-6000.0, 0.0, 0.0)), spawn(ShipType::tStandardFrigate, 1, v(2000.0, 0.0, 0.0))],
        // (New ships are neutral and fire back; the frigate is made evasive so the Acolyte's
        // run is not cut short.)
        vec![(0, Order::Tactics { ships: vec![1], tactics: hwc_sim::object::tactics::EVASIVE }), (1, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    let mut link_fired = 0;
    let run = twice(d.statics.clone(), s, 400, &|_| {}, &mut |_, ev| {
        link_fired += ev.iter().filter(|e| matches!(e, SpecialEvent::MissileFired { gun, .. } if *gun >= 2)).count();
    });
    assert_eq!(link_fired, 0);
    assert!(run.universe.ships.get(run.ids[0]).is_some(), "the Acolyte survived");
    assert_eq!(missiles_left(&run.universe, run.ids[0], &[2, 3]), 2);
}

/// An ACV with the EMP upgrade fires its EMP among enemy fighters and a frigate: the ones in
/// range are disabled, the far one is not; the ACV fires, recharges and is ready again after
/// 77 s.
#[test]
fn acv_emp_disables_enemies_in_range() {
    let Some(d) = data() else { return };
    let s = scenario(
        "acv emp",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            spawn(ShipType::sACV, 0, v(0.0, 0.0, 0.0)),
            spawn(ShipType::tHeavyInterceptor, 1, v(1200.0, 0.0, 0.0)),
            spawn(ShipType::tHeavyInterceptor, 1, v(0.0, 1500.0, 200.0)),
            spawn(ShipType::tStandardFrigate, 1, v(-1800.0, 0.0, 0.0)),
            spawn(ShipType::tStandardFrigate, 1, v(9000.0, 0.0, 0.0)),
        ],
        // (The enemies are made evasive, as new ships used to be, so the ACV lives to recharge.)
        vec![(0, Order::Tactics { ships: vec![1, 2, 3, 4], tactics: hwc_sim::object::tactics::EVASIVE }), (2, Order::Special { ships: vec![0], targets: vec![] })],
    );
    let setup = |run: &mut Run| upgrade(run, &[0], acv::UPGRADE_EMP);
    // (The EMP goes off while the order is issued, before the update that clears the events,
    // so the state sequence is checked instead of the blast event.)
    let mut disabled = [false; 5];
    let mut states = Vec::new();
    let run = twice(d.statics.clone(), s, 16 * 80, &setup, &mut |run, _| {
        for (i, flag) in disabled.iter_mut().enumerate() {
            *flag |= run.universe.ships.get(run.ids[i]).is_some_and(|s| s.disable.disabled);
        }
        if let Some(ShipSpec::Acv(a)) = run.universe.ships.get(run.ids[0]).map(|s| &s.spec) {
            if states.last() != Some(&a.emp_state) {
                states.push(a.emp_state);
            }
        }
    });
    assert_eq!(disabled, [false, true, true, true, false]);
    assert_eq!(states, vec![acv::emp::READY, acv::emp::FIRING, acv::emp::RECHARGING, acv::emp::READY]);
    let far = run.universe.ships.ship(run.ids[4]);
    assert_eq!(far.disable.emp_count, 0);
}

/// Given a distant target, the ACV flies to it and fires when within 2.5 km.
#[test]
fn acv_flies_to_emp_target() {
    let Some(d) = data() else { return };
    let s = scenario(
        "acv emp target",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![spawn(ShipType::sACV, 0, v(0.0, 0.0, 0.0)), spawn(ShipType::tStandardFrigate, 1, v(9000.0, 0.0, 0.0))],
        vec![(2, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    let setup = |run: &mut Run| upgrade(run, &[0], acv::UPGRADE_EMP);
    let mut moving = false;
    let mut fired_at = None;
    let run = twice(d.statics.clone(), s, 16 * 60, &setup, &mut |run, ev| {
        moving |= acv::is_moving_to_emp_target(&run.universe, run.ids[0]);
        if fired_at.is_none() && ev.iter().any(|e| matches!(e, SpecialEvent::Special { effect: hwc_sim::ships::Effect::EmpBlast { .. }, .. })) {
            let a = run.universe.ships.ship(run.ids[0]).posinfo.position;
            let b = run.universe.ships.ship(run.ids[1]).posinfo.position;
            fired_at = Some(Vector::distance_ext(a, b));
        }
    });
    assert!(moving, "the ACV moved to its target");
    let dist = fired_at.expect("the EMP fired");
    assert!(dist < 2500.0, "fired within range ({dist})");
    assert!(run.universe.ships.ship(run.ids[1]).disable.emp_count >= 1);
}

/// A static world whose minelayer lays walls of `n`² mines.
fn minelayer_world(n: i32) -> Arc<StaticWorld> {
    let d = data().expect("data");
    let mut w = (*d.statics).clone();
    let stat = w.ships[ShipType::tMinelayerCorvette.index()].as_mut().expect("minelayer");
    for (k, val) in stat.custom_entries.iter_mut() {
        if k == "NumMinesInSide" {
            *val = n.to_string();
        }
    }
    Arc::new(w)
}

/// The minelayer's force drop lays a 2x2 wall: four forced mines in a new formation, each
/// flying to its own wall point behind the ship; the formation is closed once the wall is
/// complete. An enemy corvette then flies into the wall and the mines go for it.
#[test]
fn minelayer_lays_wall_and_mines_hit_a_ship() {
    let Some(_) = data() else { return };
    let statics = minelayer_world(2);
    let s = scenario(
        "mine wall",
        vec![ShipRace::Taiidan, ShipRace::Somtaaw],
        vec![spawn(ShipType::tMinelayerCorvette, 0, v(0.0, 0.0, 0.0)), spawn(ShipType::sACV, 1, v(-30000.0, 0.0, 0.0))],
        vec![(2, Order::Special { ships: vec![0], targets: vec![] }), (16 * 40, Order::Move { ships: vec![1], mode: 0, waypoints: vec![v(-800.0, 0.0, 0.0)] })],
    );
    let mut dropped = 0;
    let mut hits = 0;
    let mut formation_full_with = None;
    let mut target_health = None;
    let run = twice(statics, s, 16 * 120, &|_| {}, &mut |run, ev| {
        dropped += ev.iter().filter(|e| matches!(e, SpecialEvent::MissileFired { .. })).count();
        hits += ev.iter().filter(|e| matches!(e, SpecialEvent::MissileHit { .. })).count();
        if formation_full_with.is_none() {
            if let Some(f) = run.universe.mine_formations.iter().find(|f| f.full) {
                formation_full_with = Some(f.mines.clone());
            }
        }
        if run.tick() == 16 * 40 {
            target_health = run.universe.ships.get(run.ids[1]).map(|s| s.health);
        }
    });
    let u = &run.universe;
    let mines = formation_full_with.expect("the wall was completed");
    assert_eq!(mines.len(), 4);
    assert!(dropped >= 4);
    if let Some(ShipSpec::Minelayer(m)) = u.ships.get(run.ids[0]).map(|s| &s.spec) {
        assert_eq!(m.mining_status, minelayer::mining::DONE_WAIT);
        assert!(m.formation.is_none());
    }
    println!("mine hits {hits}");
    assert!(hits >= 1, "mines hit the intruder");
    let after = u.ships.get(run.ids[1]).map_or(0.0, |s| s.health);
    assert!(after < target_health.expect("health"), "the intruder took damage");
}

/// The wall points of a forced 2x2 wall lie behind the ship, spaced `MineSpacing` apart, and
/// the mines get there.
#[test]
fn forced_mines_reach_their_wall_points() {
    let Some(_) = data() else { return };
    let statics = minelayer_world(2);
    let s = scenario(
        "mine wall points",
        vec![ShipRace::Taiidan],
        vec![spawn(ShipType::tMinelayerCorvette, 0, v(0.0, 0.0, 0.0))],
        vec![(2, Order::Special { ships: vec![0], targets: vec![] })],
    );
    let run = twice(statics, s, 16 * 60, &|_| {}, &mut |_, _| {});
    let u = &run.universe;
    let mines: Vec<_> = u.missile_list.iter().map(|&m| u.missiles.missile(m)).collect();
    assert_eq!(mines.len(), 4);
    for m in &mines {
        assert!(m.forced);
        let d = Vector::distance_ext(m.posinfo.position, m.wall_point);
        assert!(d < 50.0, "mine {:?} is {d} from its wall point", m.id);
    }
    // Neighbouring wall points are one spacing (2000 m in the data) apart.
    let d01 = Vector::distance_ext(mines[0].wall_point, mines[1].wall_point);
    assert!((d01 - 2000.0).abs() < 1.0, "{d01}");
}

/// A minelayer attacking a carrier makes passes and drops (non-forced) mines near it.
#[test]
fn minelayer_attack_run_drops_mines() {
    let Some(d) = data() else { return };
    let s = scenario(
        "minelayer attack",
        vec![ShipRace::Taiidan, ShipRace::Somtaaw],
        vec![spawn(ShipType::tMinelayerCorvette, 0, v(-5000.0, 0.0, 0.0)), spawn(ShipType::sCarrier, 1, v(1000.0, 0.0, 0.0))],
        // (The carrier is made evasive, as new ships used to be.)
        vec![(0, Order::Tactics { ships: vec![1], tactics: hwc_sim::object::tactics::EVASIVE }), (1, Order::Attack { ships: vec![0], targets: vec![1] })],
    );
    let mut dropped = 0;
    let mut states = std::collections::BTreeSet::new();
    let run = twice(d.statics.clone(), s, 16 * 90, &|_| {}, &mut |run, ev| {
        dropped += ev.iter().filter(|e| matches!(e, SpecialEvent::MissileFired { .. })).count();
        if let Some(s) = run.universe.ships.get(run.ids[0]) {
            states.insert(s.ai_state_attack);
        }
    });
    println!("attack-run states {states:?}, mines dropped {dropped}");
    assert!(dropped >= 1);
    assert!(states.contains(&minelayer::run::KILL));
    assert!(run.universe.missile_list.iter().all(|&m| !run.universe.missiles.missile(m).forced));
}

/// A missile destroyer's special targeting fires volleys at two targets (one missile per
/// launcher per volley, alternating targets) until its launchers are empty; 12 s after the
/// last volley order its missiles start to regenerate.
#[test]
fn missile_destroyer_volley_and_regeneration() {
    let Some(d) = data() else { return };
    let s = scenario(
        "missile volley",
        vec![ShipRace::Taiidan, ShipRace::Somtaaw],
        vec![
            spawn(ShipType::tMissileDestroyer, 0, v(0.0, 0.0, 0.0)),
            spawn(ShipType::sMultiBeamFrigate, 1, v(5000.0, 0.0, 0.0)),
            spawn(ShipType::sMultiBeamFrigate, 1, v(5000.0, 2000.0, 0.0)),
        ],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1, 2] })],
    );
    let launchers = [0usize, 1, 2, 3];
    let mut fired_ticks = Vec::new();
    let mut targets = std::collections::BTreeSet::new();
    let mut empty_at = None;
    let mut counts = Vec::new();
    let run = twice(d.statics.clone(), s, 16 * 30, &|_| {}, &mut |run, ev| {
        let u = &run.universe;
        for e in ev {
            if let SpecialEvent::MissileFired { missile, .. } = e {
                fired_ticks.push(run.tick());
                if let Some(t) = u.missiles.get(*missile).and_then(|m| m.target) {
                    targets.insert(t);
                }
            }
        }
        let n = missiles_left(u, run.ids[0], &launchers);
        if empty_at.is_none() && n == 0 {
            empty_at = Some(run.tick());
        }
        counts.push(n);
    });
    println!("volley: {} missiles, empty at {empty_at:?}", fired_ticks.len());
    assert_eq!(fired_ticks.len(), 32, "4 launchers x 8 missiles");
    assert_eq!(targets.len(), 2, "both targets were fired at");
    // Volleys of four, 0.2 s apart.
    assert_eq!(fired_ticks.iter().filter(|&&t| t == fired_ticks[0]).count(), 4);
    let empty = empty_at.expect("launchers emptied") as usize;
    // No regeneration within MissileLagVolleyTime (12 s) of the last volley order...
    let lag_end = empty + 16 * 11;
    assert!(counts[empty..lag_end].iter().all(|&n| n == 0));
    // ...then one missile every 0.575 s.
    let last = *counts.last().unwrap();
    assert!(last >= 5, "regenerated {last}");
    assert!(counts.windows(2).all(|w| w[1] >= w[0] || w[1] + 4 >= w[0]));
    let _ = run;
}
