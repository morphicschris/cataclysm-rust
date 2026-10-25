//! Effects of special abilities over the real game data (`HWC_EXTRACTED_DIR`; skipped when it
//! is unset): the ACV's pulse, sentinel field hits, the Leech's drain, DFG deflections and the
//! gravity well's switch-on, caught-ship and switch-off effects start where the original
//! starts them and end. Every scenario runs twice with effects and must hash the same.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use hwc_fx::runtime::EffectId;
use hwc_math::Vector;
use hwc_sim::effects::{EffectData, FxOwner};
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::object::ShipId;
use hwc_sim::ships::{acv, leech, sentinel, ShipSpec};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
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
        let src: Box<dyn DataSource> = Box::new(DirSource::new(dir).ok()?);
        let statics = Arc::new(StaticWorld::load_from(src.as_ref(), LoadOptions::default()));
        let tweaks = SimTweaks::load(src.as_ref());
        let effects = EffectData::load_from(src.as_ref());
        (!effects.is_empty()).then_some(Data { statics, tweaks, effects })
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

/// What became of one effect: its static's file name, its owner when first seen, and the
/// ticks it was first and last seen at.
#[derive(Debug, Clone)]
struct Seen {
    name: String,
    owner: Option<FxOwner>,
    first: u32,
    last: u32,
}

/// A run with effects attached; `setup` changes the universe before the first update and
/// `watch` sees every update. Returns the hashes, every effect seen and the run.
fn run_fx(s: Scenario, ticks: u32, setup: &dyn Fn(&mut Run), watch: &mut dyn FnMut(&Run, u32)) -> (Vec<u64>, BTreeMap<EffectId, Seen>, Run) {
    let d = data().expect("data");
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    hwc_sim::effects::attach(&mut run.universe, &d.effects);
    setup(&mut run);
    let mut hashes = Vec::new();
    let mut seen: BTreeMap<EffectId, Seen> = BTreeMap::new();
    for t in 0..ticks {
        hashes.push(run.step());
        let fx = run.universe.effects.as_ref().expect("effects");
        for e in &fx.world.effects {
            let name = fx.world.lib.statics[e.stat].name.to_ascii_lowercase();
            seen.entry(e.id).and_modify(|x| x.last = t).or_insert(Seen { name, owner: e.owner.and_then(FxOwner::from_id), first: t, last: t });
        }
        watch(&run, t);
    }
    (hashes, seen, run)
}

/// [`run_fx`] twice: the hash sequences must agree.
fn twice(s: Scenario, ticks: u32, setup: &dyn Fn(&mut Run), watch: &mut dyn FnMut(&Run, u32)) -> (BTreeMap<EffectId, Seen>, Run) {
    let (a, seen, run) = run_fx(s.clone(), ticks, setup, watch);
    let (b, _, _) = run_fx(s, ticks, setup, &mut |_, _| {});
    assert_eq!(a, b, "the two runs differ");
    (seen, run)
}

fn named<'a>(seen: &'a BTreeMap<EffectId, Seen>, name: &str) -> Vec<&'a Seen> {
    seen.values().filter(|s| s.name == name).collect()
}

/// The effect ended before the run did.
fn ended(s: &Seen, ticks: u32) -> bool {
    s.last + 1 < ticks
}

fn upgrade(run: &mut Run, i: usize, bits: u32) {
    let id = run.ids[i];
    run.universe.ships.ship_mut(id).upgrade_flags |= bits;
}

/// The ACV's EMP starts `electromagneticpulse.ebg` on the ACV, which ends by itself.
#[test]
fn acv_emp_pulse_effect() {
    if data().is_none() {
        return;
    }
    let s = scenario(
        "acv emp effect",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![spawn(ShipType::sACV, 0, v(0.0, 0.0, 0.0)), spawn(ShipType::tHeavyInterceptor, 1, v(1200.0, 0.0, 0.0)), spawn(ShipType::tStandardFrigate, 1, v(-1800.0, 0.0, 0.0))],
        vec![(2, Order::Special { ships: vec![0], targets: vec![] })],
    );
    let ticks = 16 * 20;
    let (seen, run) = twice(s, ticks, &|r| upgrade(r, 0, acv::UPGRADE_EMP), &mut |_, _| {});
    let pulses = named(&seen, "electromagneticpulse.ebg");
    assert_eq!(pulses.len(), 1, "{seen:?}");
    let p = pulses[0];
    assert_eq!(p.owner, Some(FxOwner::Ship(run.ids[0])));
    assert!(p.first <= 3, "started when the EMP went off ({})", p.first);
    assert!(ended(p, ticks), "the pulse ended ({p:?})");
}

/// Level mines heading into a raised sentinel wall are stopped there and show their hit
/// effect (the mine row of their owner's race; no owner) at the field, which ends. (Single
/// player, so that the mines belong to player 1.)
#[test]
fn sentinel_field_missile_hit_effect() {
    if data().is_none() {
        return;
    }
    let mut spawns = Vec::new();
    for i in 0..5 {
        spawns.push(spawn(ShipType::sSentinel, 0, v(0.0, (i as f32 - 2.0) * 250.0, 0.0)));
    }
    let sentinels: Vec<usize> = (0..5).collect();
    let mut orders = vec![
        (0, Order::Tactics { ships: sentinels.clone(), tactics: 1 }),
        (0, Order::Formation { ships: sentinels, formation_type: sentinel::formation_kind::WALL }),
    ];
    for k in 0..6 {
        orders.push((300 + k * 20, Order::Mine { position: v(1500.0, (k as f32 - 2.5) * 120.0, 60.0) }));
    }
    let s = scenario("sentinel field effects", vec![ShipRace::Somtaaw, ShipRace::Taiidan], spawns, orders);
    let ticks = 1200;
    let mut field_hits = Vec::new();
    let (seen, run) = twice(s, ticks, &|r| r.universe.single_player = true, &mut |r, t| {
        for e in &r.universe.special_events {
            if let hwc_sim::universe::SpecialEvent::Special { effect: hwc_sim::ships::Effect::FieldHit, ship } = e {
                if r.ids[..5].contains(ship) {
                    field_hits.push(t);
                }
            }
        }
    });
    assert!(!field_hits.is_empty(), "the field was hit");
    let fx = run.universe.effects.as_ref().unwrap();
    let race = run.universe.players[1].race.index();
    let lod = fx.tables.gun_event(race, 9, hwc_sim::effects::GUN_HIT).expect("mine hit effect");
    let name = fx.world.lib.statics[lod[0]].name.to_ascii_lowercase();
    let hits: Vec<&Seen> = named(&seen, &name).into_iter().filter(|s| s.owner.is_none() && field_hits.contains(&s.first)).collect();
    assert!(!hits.is_empty(), "{name} at the field hits {field_hits:?}: {seen:?}");
    assert!(hits.iter().all(|h| ended(h, ticks)), "{hits:?}");
}

/// A Leech draining health shows `leechvent.ebg` on itself every 3 to 5 seconds.
#[test]
fn leech_drain_effect() {
    if data().is_none() {
        return;
    }
    let s = scenario(
        "leech drain effect",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![spawn(ShipType::sLeech, 0, v(-1500.0, 200.0, 0.0)), spawn(ShipType::tIonCannonFrigate, 1, v(0.0, 0.0, 0.0))],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    let setup = |r: &mut Run| {
        upgrade(r, 0, leech::UPGRADE_LEECH);
        let id = r.ids[0];
        leech::set_state(&mut r.universe, id, leech::state::DRAIN_HEALTH);
    };
    let ticks = 1600;
    let mut delays = Vec::new();
    let mut last = 0.0f32;
    let (seen, run) = twice(s, ticks, &setup, &mut |r, _| {
        let u = &r.universe;
        if let ShipSpec::Leech(l) = &u.ships.ship(r.ids[0]).spec {
            if l.effect_time != last && l.effect_time != 0.0 {
                delays.push(l.effect_time - u.total_time_elapsed);
                last = l.effect_time;
            }
        }
    });
    let drains = named(&seen, "leechvent.ebg");
    assert!(drains.len() >= 2, "replayed: {seen:?}");
    assert!(drains.iter().all(|d| d.owner == Some(FxOwner::Ship(run.ids[0]))));
    assert!(ended(drains[0], ticks));
    assert!(!delays.is_empty());
    // Set during the update, read after it (one frame later).
    assert!(delays.iter().all(|&d| (2.9..=5.0).contains(&d)), "{delays:?}");
}

/// Enemy bullets in a DFG field (`univDFGFieldEffect`, called directly: the collision walk
/// does not test DFG fields yet) are deflected, each deflection showing
/// `defensefielddeflect.ebg` owned by the frigate, which ends; the run hashes the same twice.
#[test]
fn dfg_deflection_effect() {
    if data().is_none() {
        return;
    }
    let s = scenario(
        "dfg deflect effect",
        vec![ShipRace::Taiidan, ShipRace::Taiidan],
        vec![spawn(ShipType::tDFGFrigate, 0, v(0.0, 0.0, 0.0)), spawn(ShipType::tHeavyInterceptor, 1, v(1500.0, 0.0, 0.0))],
        Vec::new(),
    );
    let ticks = 16 * 12;
    let go = || {
        let mut deflected = 0;
        let mut shot = 0;
        // Shoot and deflect by hand, a few frames apart, then let the effects run.
        let d = data().unwrap();
        let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s.clone()).unwrap();
        hwc_sim::effects::attach(&mut run.universe, &d.effects);
        let (dfg, fighter) = (run.ids[0], run.ids[1]);
        let mut hashes = Vec::new();
        let mut seen: BTreeMap<EffectId, Seen> = BTreeMap::new();
        for t in 0..ticks {
            if t < 16 * 4 && t % 4 == 0 {
                let before: Vec<_> = run.universe.bullets.iter().map(|b| b.id).collect();
                if hwc_sim::gun::shoot(&mut run.universe, fighter, 0, Some(dfg)) {
                    shot += 1;
                }
                let new: Vec<_> = run.universe.bullets.iter().map(|b| b.id).filter(|b| !before.contains(b)).collect();
                for b in new {
                    let life = run.universe.bullets.bullet(b).total_lifetime;
                    hwc_sim::ships::dfg::field_effect(&mut run.universe, dfg, b);
                    if run.universe.bullets.get(b).is_some_and(|x| x.total_lifetime != life) {
                        deflected += 1;
                    }
                }
            }
            hashes.push(run.step());
            let fx = run.universe.effects.as_ref().unwrap();
            for e in &fx.world.effects {
                let name = fx.world.lib.statics[e.stat].name.to_ascii_lowercase();
                seen.entry(e.id).and_modify(|x| x.last = t).or_insert(Seen { name, owner: e.owner.and_then(FxOwner::from_id), first: t, last: t });
            }
        }
        (hashes, seen, dfg, shot, deflected)
    };
    let (h1, seen, dfg, shot, deflected) = go();
    let (h2, ..) = go();
    assert_eq!(h1, h2);
    assert!(shot > 0 && deflected > 0, "{shot} shots, {deflected} deflected");
    let deflects = named(&seen, "defensefielddeflect.ebg");
    assert_eq!(deflects.len(), deflected, "{seen:?}");
    assert!(deflects.iter().all(|d| d.owner == Some(FxOwner::Ship(dfg))));
    assert!(deflects.iter().all(|d| ended(d, ticks)));
}

/// A gravity well switched on shows `gravwellon.ebg` (kept as its field effect) and catches
/// nearby fighters, which show `caughtgravwell.ebg`; switched off it lets the field effect
/// wind down and plays `gravwelloff.ebg`.
#[test]
fn gravwell_effects() {
    if data().is_none() {
        return;
    }
    let off_at = 16 * 20;
    let s = scenario(
        "gravwell effects",
        vec![ShipRace::Taiidan, ShipRace::Taiidan],
        vec![
            spawn(ShipType::tGravWellGenerator, 0, v(0.0, 0.0, 0.0)),
            spawn(ShipType::tHeavyInterceptor, 1, v(1200.0, 0.0, 0.0)),
            spawn(ShipType::tHeavyInterceptor, 1, v(-900.0, 600.0, 0.0)),
        ],
        vec![(2, Order::Special { ships: vec![0], targets: vec![] }), (off_at, Order::Special { ships: vec![0], targets: vec![] })],
    );
    let ticks = off_at + 16 * 20;
    let mut field: Vec<Option<EffectId>> = Vec::new();
    let (seen, run) = twice(s, ticks, &|_| {}, &mut |r, _| field.push(hwc_sim::effects::slot(&r.universe, r.ids[0], 100)));
    let well = run.ids[0];
    let on = named(&seen, "gravwellon.ebg");
    assert!(!on.is_empty(), "{seen:?}");
    assert!(on.iter().all(|e| e.owner == Some(FxOwner::Ship(well))));
    // Kept while the field is on, let go when it is switched off; then it winds down.
    let kept = field[(off_at - 10) as usize].expect("field effect kept");
    assert!(field[(off_at + 1) as usize].is_none());
    assert!(ended(&seen[&kept], ticks), "{:?}", seen[&kept]);
    assert!(seen[&kept].last >= off_at - 1);
    let off = named(&seen, "gravwelloff.ebg");
    assert_eq!(off.len(), 1, "{seen:?}");
    assert!(off[0].first + 1 >= off_at && off[0].first <= off_at + 1, "{:?}", off[0]);
    let caught = named(&seen, "caughtgravwell.ebg");
    let fighters: Vec<FxOwner> = run.ids[1..].iter().map(|&s: &ShipId| FxOwner::Ship(s)).collect();
    assert!(!caught.is_empty(), "{seen:?}");
    assert!(caught.iter().all(|c| c.owner.is_some_and(|o| fighters.contains(&o)) && c.first < off_at + 2));
    assert!(caught.iter().any(|c| ended(c, ticks)));
}

/// Without effects the same scenario hashes as it always did: attaching effects changes
/// nothing for the ACV and the DFG (whose effect decisions touch no game state).
#[test]
fn effects_leave_gameplay_alone() {
    let Some(d) = data() else { return };
    let s = scenario(
        "dfg no fx",
        vec![ShipRace::Taiidan, ShipRace::Somtaaw],
        vec![
            spawn(ShipType::tDFGFrigate, 0, v(0.0, 0.0, 0.0)),
            spawn(ShipType::tStandardFrigate, 0, v(600.0, 0.0, 0.0)),
            spawn(ShipType::sMultiBeamFrigate, 1, v(4000.0, 0.0, 0.0)),
            spawn(ShipType::sACV, 1, v(3000.0, 600.0, 0.0)),
        ],
        vec![(1, Order::Attack { ships: vec![2], targets: vec![1] }), (40, Order::Special { ships: vec![3], targets: vec![] })],
    );
    let ticks = 16 * 30;
    let setup = |r: &mut Run| upgrade(r, 3, acv::UPGRADE_EMP);
    let (with, _, _) = run_fx(s.clone(), ticks, &setup, &mut |_, _| {});
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    setup(&mut run);
    let without: Vec<u64> = (0..ticks).map(|_| run.step()).collect();
    assert_eq!(with, without);
}
