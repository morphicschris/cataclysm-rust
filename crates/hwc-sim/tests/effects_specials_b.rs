//! Effects of the ramming frigate, repair droids, worker repair, processors, the mothership's
//! big gun and shock waves over the real game data (`HWC_EXTRACTED_DIR`; skipped when unset):
//! each system starts its effect (the right effect file) where the original does and ends it
//! where the original does, and runs with effects attached stay deterministic.

use std::sync::{Arc, OnceLock};

use hwc_math::Vector;
use hwc_sim::effects::{self, EffectData};
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::ships::{mothership, processor, ramming, worker_repair, ShipSpec};
use hwc_sim::shockwave;
use hwc_sim::statics::{DataSource, DirSource, DockPointType, LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    effects: EffectData,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let dir = std::env::var("HWC_EXTRACTED_DIR").ok()?;
        let src = DirSource::new(dir).ok()?;
        let src: &dyn DataSource = &src;
        let statics = Arc::new(StaticWorld::load_from(src, LoadOptions::default()));
        let tweaks = SimTweaks::load(src);
        let effects = EffectData::load_from(src);
        if effects.is_empty() {
            return None;
        }
        Some(Data { statics, tweaks, effects })
    })
    .as_ref()
}

fn scenario(name: &'static str, players: Vec<ShipRace>, spawns: Vec<Spawn>, orders: Vec<(u32, Order)>) -> Scenario {
    Scenario { name, players, spawns, orders, resources: Vec::new(), resource_units: Vec::new() }
}

/// A run with effects attached.
fn new_run(s: Scenario) -> Option<Run> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    effects::attach(&mut run.universe, &d.effects);
    Some(run)
}

fn v(x: f32, y: f32, z: f32) -> Vector {
    Vector::new(x, y, z)
}

/// How many live effects run the effect file `name`.
fn live(run: &Run, name: &str) -> usize {
    let fx = run.universe.effects.as_ref().expect("effects attached");
    fx.world.effects.iter().filter(|e| fx.world.lib.statics[e.stat].name.eq_ignore_ascii_case(name)).count()
}

/// Two runs give the same hashes and the same effects.
fn deterministic(make: impl Fn() -> Option<Run>, ticks: u32) {
    let (Some(mut a), Some(mut b)) = (make(), make()) else { return };
    for _ in 0..ticks {
        assert_eq!(a.step(), b.step());
        let (fa, fb) = (a.universe.effects.as_ref().unwrap(), b.universe.effects.as_ref().unwrap());
        let sig = |f: &effects::Effects| f.world.effects.iter().map(|e| (e.stat, e.position.x.to_bits(), e.time_elapsed.to_bits())).collect::<Vec<_>>();
        assert_eq!(sig(fa), sig(fb));
    }
}

// --- the ramming frigate

fn ram_run() -> Option<Run> {
    let s = scenario(
        "ram fx",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            Spawn { ship_type: ShipType::sRammingFrigate, player: 0, position: v(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardDestroyer, player: 1, position: v(3000.0, 400.0, 0.0) },
        ],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    new_run(s)
}

fn ram_state(run: &Run, id: hwc_sim::ShipId) -> u8 {
    match run.universe.ship(id).map(|s| &s.spec) {
        Some(ShipSpec::Ramming(r)) => r.state,
        _ => 255,
    }
}

/// Latching on makes the impact and the vents; pushing shows the cutting beam, which goes when
/// the push ends.
#[test]
fn rammer_impact_vents_and_cutting_beam() {
    let Some(mut run) = ram_run() else { return };
    let rammer = run.ids[0];
    let (mut impact, mut vents, mut beam, mut beam_while_not_pushing) = (false, false, false, false);
    let mut pushed = false;
    for _ in 0..1600 {
        run.step();
        let st = ram_state(&run, rammer);
        impact |= live(&run, "RamFrigateImpact.ebg") > 0;
        vents |= live(&run, "RamVent.ebg") > 0;
        let b = live(&run, "BulletCuttingBeam.ebg");
        if st == ramming::state::PUSHING {
            pushed = true;
            beam |= b > 0;
            assert!(b <= 1, "one cutting beam at a time");
        } else if b > 0 {
            beam_while_not_pushing = true;
        }
    }
    assert!(pushed, "the rammer pushed");
    assert!(impact, "the impact effect started");
    assert!(vents, "the vents blew");
    assert!(beam, "the cutting beam showed while pushing");
    assert!(!beam_while_not_pushing, "the cutting beam ends with the push");
}

#[test]
fn rammer_runs_with_effects_are_deterministic() {
    deterministic(ram_run, 1200);
}

// --- repair droids

fn droid_run() -> Option<Run> {
    let s = scenario("droid fx", vec![ShipRace::Somtaaw], vec![Spawn { ship_type: ShipType::sProcessor, player: 0, position: Vector::ZERO }], Vec::new());
    let mut run = new_run(s)?;
    let pr = run.ids[0];
    let u = &mut run.universe;
    u.base.options.repair_bots = true;
    let max = u.statics.ship(ShipType::sProcessor).unwrap().health.max_health;
    let s = u.ships.ship_mut(pr);
    s.upgrade_flags |= hwc_sim::upgrade::BIT_REPAIR;
    s.health = max * 0.1;
    Some(run)
}

/// Welding droids throw sparks; the sparks are short and stop when nobody welds.
#[test]
fn repair_droids_weld_with_sparks() {
    let Some(mut run) = droid_run() else { return };
    let pr = run.ids[0];
    let mut sparks = 0;
    for _ in 0..16 * 30 {
        run.step();
        let n = run.universe.ships.ship(pr).droids.as_ref().map_or(0, |a| a.sparks.len());
        sparks += n;
        if n > 0 {
            assert!(live(&run, "WeldingSparks.ebg") > 0, "a spark made its effect");
        }
    }
    assert!(sparks > 0, "the droids welded");
    // Whole again: no more welding, the sparks die out.
    let max = run.universe.statics.ship(ShipType::sProcessor).unwrap().health.max_health;
    run.universe.ships.ship_mut(pr).health = max;
    run.run(16 * 20);
    assert_eq!(live(&run, "WeldingSparks.ebg"), 0);
}

#[test]
fn droid_runs_with_effects_are_deterministic() {
    deterministic(droid_run, 400);
}

// --- worker repair

fn worker_run() -> Option<Run> {
    let s = scenario(
        "worker fx",
        vec![ShipRace::Somtaaw],
        vec![
            Spawn { ship_type: ShipType::sWorker, player: 0, position: Vector::ZERO },
            Spawn { ship_type: ShipType::sMultiBeamFrigate, player: 0, position: v(2500.0, 0.0, 0.0) },
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

/// The repair beam runs while the worker repairs and goes when it lets go of the target.
#[test]
fn worker_repair_beam_starts_and_ends() {
    let Some(mut run) = worker_run() else { return };
    let (w, fr) = (run.ids[0], run.ids[1]);
    let mut seen = false;
    for _ in 0..16 * 60 {
        run.step();
        let ws = run.universe.ships.ship(w).worker_repair;
        let n = live(&run, "Repair_Corvette.ebg");
        if ws.state == worker_repair::state::REPAIRING && ws.beam_effect {
            assert_eq!(n, 1, "one beam while repairing");
            seen = true;
        }
        if ws.target.is_none() {
            assert_eq!(n, 0, "no beam without a target");
        }
        if seen {
            break;
        }
    }
    assert!(seen, "the beam started");
    // The worker is told to do something else: the beam goes.
    worker_repair::order_changed(&mut run.universe, w);
    assert_eq!(live(&run, "Repair_Corvette.ebg"), 0);
    assert!(run.universe.ships.get(fr).is_some());
}

#[test]
fn worker_runs_with_effects_are_deterministic() {
    deterministic(worker_run, 400);
}

// --- processors

fn processor_run() -> Option<Run> {
    let d = data()?;
    let stat = d.statics.ship(ShipType::sProcessor)?;
    let dock = stat.dock.as_ref()?;
    let t = DockPointType::AutoRepair.index();
    let p = &dock.points[dock.first_of_type[t] as usize];
    let local = Vector::new(p.position.x + p.cone_normal.x * 900.0, p.position.y + p.cone_normal.y * 900.0, p.position.z + p.cone_normal.z * 900.0);
    let world = hwc_sim::universe::DEFAULT_SHIP_MATRIX.mul_vec(local);
    let s = scenario(
        "processor fx",
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
    u.ships.ship_mut(fr).health = max * 0.2;
    Some(run)
}

/// Each beam on the frigate shows a repair beam; they go when the frigate is whole.
#[test]
fn processor_beams_start_and_end() {
    let Some(mut run) = processor_run() else { return };
    let (pr, fr) = (run.ids[0], run.ids[1]);
    run.run(16 * 3);
    let on = processor::beams(&run.universe, pr).iter().filter(|b| b.target == Some(fr)).count();
    assert!(on > 0, "a beam took the frigate");
    assert_eq!(live(&run, "Repair_Corvette.ebg"), on, "one effect per beam");
    let max = run.universe.statics.ship(ShipType::sMultiBeamFrigate).unwrap().health.max_health;
    run.universe.ships.ship_mut(fr).health = max;
    run.run(2);
    assert_eq!(live(&run, "Repair_Corvette.ebg"), 0, "the beams stopped");
}

#[test]
fn processor_runs_with_effects_are_deterministic() {
    deterministic(processor_run, 200);
}

// --- the big gun

fn big_gun_run() -> Option<Run> {
    let s = scenario(
        "big gun fx",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            Spawn { ship_type: ShipType::sMothership, player: 0, position: Vector::ZERO },
            Spawn { ship_type: ShipType::sMothershipBigGun, player: 0, position: Vector::ZERO },
        ],
        Vec::new(),
    );
    let mut run = new_run(s)?;
    let (ms, gun) = (run.ids[0], run.ids[1]);
    hwc_sim::clamp::clamp_obj_to_obj(&mut run.universe, gun, ms);
    let gi = mothership::big_gun_index(&run.universe, ms)?;
    run.universe.ships.ship_mut(ms).guns[gi].last_time_fired = -10_000.0;
    Some(run)
}

/// The charge effect starts with the charge, is let go of when the gun fires, and is deleted
/// when the big-gun module goes.
#[test]
fn big_gun_charge_effect() {
    let Some(mut run) = big_gun_run() else { return };
    let (ms, gun) = (run.ids[0], run.ids[1]);
    assert!(mothership::has_big_gun_module(&run.universe, ms));
    let h = run.universe.ships.ship(ms).heading();
    let point = Vector::new(h.x * 15000.0, h.y * 15000.0, h.z * 15000.0);
    mothership::cl_big_gun(&mut run.universe, &[ms], point);
    let mut charging = false;
    for _ in 0..16 * 25 {
        run.step();
        if mothership::state(&run.universe, ms) == mothership::big_gun::CHARGING {
            charging = true;
            break;
        }
    }
    assert!(charging, "the big gun charged");
    assert_eq!(live(&run, "BigGunMuzzleFlash.ebg"), 1, "the charge effect started");
    // The module goes: the charge effect is deleted.
    mothership::remove_ship_refs(&mut run.universe, ms, gun);
    assert_eq!(live(&run, "BigGunMuzzleFlash.ebg"), 0, "the charge effect ended");
}

#[test]
fn big_gun_runs_with_effects_are_deterministic() {
    let make = || {
        let mut run = big_gun_run()?;
        let ms = run.ids[0];
        let h = run.universe.ships.ship(ms).heading();
        mothership::cl_big_gun(&mut run.universe, &[ms], Vector::new(h.x * 15000.0, h.y * 15000.0, h.z * 15000.0));
        Some(run)
    };
    deterministic(make, 16 * 30);
}

// --- shock waves

fn wave_run(kind: u32) -> Option<Run> {
    let s = scenario("wave fx", vec![ShipRace::Somtaaw], vec![Spawn { ship_type: ShipType::sWorker, player: 0, position: v(30000.0, 0.0, 0.0) }], Vec::new());
    let mut run = new_run(s)?;
    shockwave::create(&mut run.universe, kind, Vector::ZERO, 0);
    Some(run)
}

/// A mothership's death wave starts its effect once its delay is over, and only then.
#[test]
fn mothership_wave_effect_starts_after_its_delay() {
    let Some(mut run) = wave_run(shockwave::kind::MOTHERSHIP_DEATH) else { return };
    run.step();
    assert_eq!(live(&run, "MotherShockWaveExplode.ebg"), 0, "nothing during the delay");
    let mut started = false;
    for _ in 0..16 * 5 {
        run.step();
        if live(&run, "MotherShockWaveExplode.ebg") > 0 {
            started = true;
            break;
        }
    }
    assert!(started, "the wave's effect started");
    assert_eq!(live(&run, "MotherShockWaveExplode.ebg"), 1, "one effect per wave");
    // The wave's effect ends by itself; the wave is gone once it has grown out.
    run.run(16 * 30);
    assert!(run.universe.shockwaves.waves.is_empty());
}

/// The Dreadnought's wave has no effect of its own but throws off sparks.
#[test]
fn dreadnought_wave_throws_sparks() {
    let Some(mut run) = wave_run(shockwave::kind::DREADNOUGHT) else { return };
    let mut sparks = false;
    for _ in 0..16 * 6 {
        run.step();
        sparks |= live(&run, "ShockWaveStar.ebg") > 0;
    }
    assert!(sparks, "sparks on the ring");
}

#[test]
fn wave_runs_with_effects_are_deterministic() {
    deterministic(|| wave_run(shockwave::kind::DREADNOUGHT), 16 * 8);
    deterministic(|| wave_run(shockwave::kind::MOTHERSHIP_DEATH), 16 * 8);
}

/// Without effects the same runs hash as before (the effect hooks change nothing).
#[test]
fn effects_change_no_gameplay_here() {
    let Some(d) = data() else { return };
    let s = scenario(
        "ram fx",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            Spawn { ship_type: ShipType::sRammingFrigate, player: 0, position: v(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardDestroyer, player: 1, position: v(3000.0, 400.0, 0.0) },
        ],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    let mut off = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let Some(mut on) = ram_run() else { return };
    for _ in 0..800 {
        assert_eq!(off.step(), on.step());
    }
}
