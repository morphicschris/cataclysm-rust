//! Ramming frigates, hive frigates with their swarmers and sentinel force fields over the real
//! game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped when neither is set).

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::object::ShipId;
use hwc_sim::ships::{hive, ramming, sentinel, Effect, ShipSpec};
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

fn scenario(name: &'static str, spawns: Vec<Spawn>, orders: Vec<(u32, Order)>) -> Scenario {
    Scenario { name, players: vec![ShipRace::Somtaaw, ShipRace::Taiidan], spawns, orders, resources: Vec::new(), resource_units: Vec::new() }
}

fn new_run(s: Scenario) -> Option<Run> {
    let d = data()?;
    Some(Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap())
}

/// Runs a scenario, calling `each` after every tick; returns the hashes.
fn run_with(s: Scenario, ticks: u32, mut each: impl FnMut(&Run)) -> Option<Vec<u64>> {
    let mut run = new_run(s)?;
    let mut hashes = Vec::new();
    for _ in 0..ticks {
        hashes.push(run.step());
        each(&run);
    }
    Some(hashes)
}

fn v(x: f32, y: f32, z: f32) -> Vector {
    Vector::new(x, y, z)
}

fn ram_state(run: &Run, id: ShipId) -> u8 {
    match run.universe.ship(id).map(|s| &s.spec) {
        Some(ShipSpec::Ramming(r)) => r.state,
        _ => 255,
    }
}

/// A ramming frigate ordered to ram an enemy frigate turns, closes in, charges, latches on
/// and pushes it, cutting into it; afterwards it recovers.
#[test]
fn rammer_rams_and_pushes() {
    let s = scenario(
        "ram",
        vec![
            Spawn { ship_type: ShipType::sRammingFrigate, player: 0, position: v(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: v(3000.0, 400.0, 0.0) },
        ],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1] })],
    );
    let Some(mut first) = new_run(s.clone()) else { return };
    let (rammer, target) = (first.ids[0], first.ids[1]);
    let start = first.universe.ship(target).unwrap().posinfo.position;
    let max_health = first.universe.ship(target).unwrap().health;
    let mut seen = [false; 6];
    let mut pushed_from = None;
    let mut hashes = Vec::new();
    for _ in 0..1600 {
        hashes.push(first.step());
        let st = ram_state(&first, rammer);
        if (st as usize) < seen.len() {
            seen[st as usize] = true;
        }
        if st == ramming::state::PUSHING && pushed_from.is_none() {
            pushed_from = first.universe.ship(target).map(|t| t.posinfo.position);
        }
    }
    assert!(seen[1] && seen[2] && seen[3], "turned, closed in and charged: {seen:?}");
    assert!(seen[5], "latched on and pushed: {seen:?}");
    let t = first.universe.ship(target);
    let health = t.map_or(0.0, |t| t.health);
    assert!(health < max_health, "the ram and the cutting beam damaged the target ({health} of {max_health})");
    if let (Some(t), Some(p0)) = (t, pushed_from) {
        let moved = (t.posinfo.position - p0).mag_sq().sqrt();
        assert!(moved > 50.0, "the target was pushed ({moved} m from {p0:?}, start {start:?})");
    }
    let again = run_with(s, 1600, |_| {}).unwrap();
    assert_eq!(hashes, again);
}

fn hive_spec(run: &Run, id: ShipId) -> hive::HiveSpec {
    match run.universe.ship(id).map(|s| &s.spec) {
        Some(ShipSpec::Hive(h)) => h.clone(),
        _ => panic!("not a hive"),
    }
}

/// A hive frigate starts with six swarmers inside; given a target it launches them, sends
/// them to attack, and recalls them when told: they dock back inside.
#[test]
fn hive_launches_attacks_and_recalls() {
    let s = scenario(
        "hive",
        vec![
            Spawn { ship_type: ShipType::sHiveFrigate, player: 0, position: v(0.0, 0.0, 0.0) },
            Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: v(3000.0, 0.0, 0.0) },
        ],
        vec![(1, Order::Special { ships: vec![0], targets: vec![1] }), (900, Order::Special { ships: vec![0], targets: vec![] })],
    );
    let Some(mut run) = new_run(s.clone()) else { return };
    let h = run.ids[0];
    let spec = hive_spec(&run, h);
    assert_eq!(spec.count, 6);
    assert!(spec.swarmers.iter().all(|s| s.is_some()));
    for sw in spec.swarmers.iter().flatten() {
        assert!(!run.universe.ship_list.contains(sw), "created inside");
        assert_eq!(hive::swarmer_state(&run.universe, *sw), hive::swarmer::DOCKED);
    }
    let mut max_out = 0;
    let mut attacking = false;
    let mut states = Vec::new();
    let mut hashes = Vec::new();
    for tick in 0..1800 {
        hashes.push(run.step());
        let spec = hive_spec(&run, h);
        if states.last() != Some(&spec.state) {
            states.push(spec.state);
        }
        let out = spec.swarmers.iter().flatten().filter(|s| run.universe.ship_list.contains(s)).count();
        max_out = max_out.max(out);
        if spec.swarmers.iter().flatten().any(|&s| hive::swarmer_state(&run.universe, s) == hive::swarmer::ATTACKING) {
            attacking = true;
        }
        let _ = tick;
    }
    assert!(max_out >= 3, "swarmers launched ({max_out} out at most), hive states {states:?}");
    assert!(attacking, "swarmers attacked, hive states {states:?}");
    assert!(states.contains(&hive::state::RECALLING) || states.contains(&hive::state::PREPARE_RECALL), "recalled: {states:?}");
    let again = run_with(s, 1800, |_| {}).unwrap();
    assert_eq!(hashes, again);
}

fn field(run: &Run, id: ShipId) -> Option<sentinel::SentinelSpec> {
    match run.universe.ship(id).map(|s| &s.spec) {
        Some(ShipSpec::Sentinel(f)) => Some(f.clone()),
        _ => None,
    }
}

/// Five sentinels in a field wall reach their slots and raise the field; an enemy frigate
/// shooting at them hits the field: the field takes the hits and loses strength, and a
/// sentinel whose field stays up keeps its health. (Faithful quirk: a sentinel housekept after
/// its formation leader drops its own field flag, so only some of them are shielded.)
#[test]
fn sentinel_field_wall_stops_fire() {
    let mut spawns = Vec::new();
    for i in 0..5 {
        spawns.push(Spawn { ship_type: ShipType::sSentinel, player: 0, position: v(0.0, (i as f32 - 2.0) * 250.0, 0.0) });
    }
    spawns.push(Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: v(3000.0, 0.0, 0.0) });
    let sentinels: Vec<usize> = (0..5).collect();
    let s = scenario(
        "sentinels",
        spawns,
        vec![
            (0, Order::Tactics { ships: (0..6).collect(), tactics: 1 }),
            (0, Order::Formation { ships: sentinels.clone(), formation_type: sentinel::formation_kind::WALL }),
            (300, Order::Attack { ships: vec![5], targets: sentinels.clone() }),
        ],
    );
    let Some(mut run) = new_run(s.clone()) else { return };
    let ids: Vec<ShipId> = run.ids[..5].to_vec();
    let mut segments = 0;
    let mut up_at = None;
    let mut hits = 0;
    let mut always_up = [true; 5];
    let mut health_then = [0.0f32; 5];
    let mut hashes = Vec::new();
    for tick in 0..900u32 {
        hashes.push(run.step());
        segments = segments.max(ids.iter().filter_map(|&i| field(&run, i)).map(|f| f.segments.len()).sum::<usize>());
        if up_at.is_none() && ids.iter().any(|&i| sentinel::field_is_up(&run.universe, i)) {
            up_at = Some(tick);
        }
        if tick == 250 {
            for (k, &i) in ids.iter().enumerate() {
                health_then[k] = run.universe.ship(i).map_or(0.0, |s| s.health);
            }
        }
        if tick >= 250 {
            for (k, &i) in ids.iter().enumerate() {
                always_up[k] &= sentinel::field_is_up(&run.universe, i);
            }
        }
        hits += run.universe.special_events.iter().filter(|e| matches!(e, SpecialEvent::Special { effect: Effect::FieldHit, .. })).count();
    }
    assert_eq!(segments, 4, "the wall of five has four field triangles (the leader and the first slot own none)");
    let up = up_at.expect("the field came up");
    assert!(up < 300, "field up at tick {up}");
    assert!(hits > 0, "the field took hits");
    let shielded: Vec<usize> = (0..5).filter(|&k| always_up[k]).collect();
    assert!(!shielded.is_empty(), "some sentinel kept its field up");
    for k in shielded {
        let h = run.universe.ship(ids[k]).map(|s| s.health);
        assert_eq!(h, Some(health_then[k]), "sentinel {k} behind its field kept its health");
    }
    let again = run_with(s, 900, |_| {}).unwrap();
    assert_eq!(hashes, again);
}

/// The triangle test of the field (`FUN_004361a0`) on a hand-made triangle.
#[test]
fn field_triangle_test() {
    let (a, b, c) = (v(0.0, 0.0, 0.0), v(0.0, 300.0, 0.0), v(300.0, 0.0, 0.0));
    let (hit, t) = sentinel::ray_hits_triangle(v(50.0, 50.0, -100.0), v(50.0, 50.0, -99.0), a, b, c);
    assert!(hit);
    assert!((t - 100.0).abs() < 1e-3, "{t}");
    let (hit, _) = sentinel::ray_hits_triangle(v(250.0, 250.0, -100.0), v(250.0, 250.0, -99.0), a, b, c);
    assert!(!hit, "outside the triangle");
    let (hit, _) = sentinel::ray_hits_triangle(v(50.0, 50.0, -100.0), v(50.0, 50.0, -101.0), a, b, c);
    assert!(!hit, "pointing away");
}

/// The same wall approached by level mines (missile objects that chase ships): a mine that
/// heads into a raised field dies there and drains it; none hits a sentinel whose field is up.
#[test]
fn sentinel_field_stops_missiles() {
    let mut spawns = Vec::new();
    for i in 0..5 {
        spawns.push(Spawn { ship_type: ShipType::sSentinel, player: 0, position: v(0.0, (i as f32 - 2.0) * 250.0, 0.0) });
    }
    let sentinels: Vec<usize> = (0..5).collect();
    let mut orders = vec![
        (0, Order::Tactics { ships: sentinels.clone(), tactics: 1 }),
        (0, Order::Formation { ships: sentinels, formation_type: sentinel::formation_kind::WALL }),
    ];
    for k in 0..6 {
        orders.push((300 + k * 20, Order::Mine { position: v(1500.0, (k as f32 - 2.5) * 120.0, 60.0) }));
    }
    let s = scenario("sentinels vs mines", spawns, orders);
    let Some(mut run) = new_run(s.clone()) else { return };
    let ids: Vec<ShipId> = run.ids[..5].to_vec();
    let mut fired = 0;
    let mut field_hits = 0;
    let mut hits_on_shielded = 0;
    let mut hashes = Vec::new();
    for _ in 0..1200u32 {
        hashes.push(run.step());
        for e in &run.universe.special_events {
            match e {
                SpecialEvent::MissileDied { .. } => fired += 1,
                SpecialEvent::Special { effect: Effect::FieldHit, ship } if ids.contains(ship) => field_hits += 1,
                SpecialEvent::MissileHit { target: hwc_sim::object::ObjRef::Ship(t), .. }
                    if ids.contains(t) && sentinel::field_is_up(&run.universe, *t) =>
                {
                    hits_on_shielded += 1;
                }
                _ => {}
            }
        }
    }
    assert!(fired > 0, "mines died");
    assert!(field_hits > 0, "the field was hit ({fired} mines died)");
    assert_eq!(hits_on_shielded, 0);
    let again = run_with(s, 1200, |_| {}).unwrap();
    assert_eq!(hashes, again);
}

/// The Force Field order (`mrProtectShips`) on a guarded ship: twelve upgraded sentinels form
/// the field sphere (type 10) around it, take their slots on the 13-ship sphere, raise the
/// field (twenty triangles), clamp to the ship and take an enemy frigate's fire on the field.
/// Unlink Force Field turns them into an ordinary guarding sphere and lets go; without a
/// target the order makes a field wall.
#[test]
fn sentinel_field_sphere_guards_a_ship() {
    use hwc_sim::command::attr;
    use hwc_sim::formation::kind;
    use hwc_sim::player_orders::{mr_formation_revert_special, mr_protect_ships};
    let mut spawns = Vec::new();
    for i in 0..12 {
        spawns.push(Spawn { ship_type: ShipType::sSentinel, player: 0, position: v(-800.0, (i as f32 - 5.5) * 150.0, 0.0) });
    }
    spawns.push(Spawn { ship_type: ShipType::sProcessor, player: 0, position: v(0.0, 0.0, 0.0) });
    spawns.push(Spawn { ship_type: ShipType::tStandardFrigate, player: 1, position: v(4000.0, 0.0, 0.0) });
    let s = scenario("sentinel sphere", spawns, vec![(400, Order::Attack { ships: vec![13], targets: vec![12] })]);
    let Some(mut run) = new_run(s) else { return };
    let ids: Vec<ShipId> = run.ids[..12].to_vec();
    let guarded = run.ids[12];
    // Without the force-field upgrade the order does nothing.
    mr_protect_ships(&mut run.universe, &ids, Some(&[guarded]));
    assert!(run.universe.ship(ids[0]).unwrap().command.is_none());
    for &i in &ids {
        run.universe.ships.ship_mut(i).upgrade_flags |= 0x1000;
    }
    mr_protect_ships(&mut run.universe, &ids, Some(&[guarded]));
    let cid = run.universe.ship(ids[0]).unwrap().command.expect("command");
    let c = run.universe.commands.get(cid).unwrap();
    assert_eq!(c.formation.formation_type, kind::SENTINEL_SPHERE);
    assert!(c.attributes & attr::PROTECTING != 0 && c.protect == vec![guarded]);
    assert_eq!(c.selection.len(), 12);
    let segments: usize = ids.iter().filter_map(|&i| field(&run, i)).map(|f| f.segments.len()).sum();
    assert_eq!(segments, 20, "the twelve slots of the sphere own twenty field triangles");
    let mut up_at = None;
    let mut hits = 0;
    let mut located = 0;
    for tick in 0..1500u32 {
        run.step();
        let up = ids.iter().filter(|&&i| sentinel::field_is_up(&run.universe, i)).count();
        if up_at.is_none() && up >= 6 {
            up_at = Some(tick);
        }
        hits += run.universe.special_events.iter().filter(|e| matches!(e, SpecialEvent::Special { effect: Effect::FieldHit, .. })).count();
        // Each hit says where it met the field, near the sentinel hit (for the ripples).
        for &(sid, at) in &run.universe.field_hits {
            let p = run.universe.ship(sid).unwrap().posinfo.position;
            let d = ((at.x - p.x).powi(2) + (at.y - p.y).powi(2) + (at.z - p.z).powi(2)).sqrt();
            assert!(d < 3000.0, "field hit {d} from its sentinel");
            located += 1;
        }
    }
    assert!(located > 0, "the field hits were located");
    let up = up_at.expect("the field sphere came up");
    assert!(up < 1000, "field up at tick {up}");
    let clamped = ids.iter().filter(|&&i| run.universe.ship(i).unwrap().base.clamp.as_ref().and_then(|c| c.host) == Some(guarded)).count();
    assert!(clamped >= 6, "{clamped} sentinels clamped to the guarded ship");
    assert!(hits > 0, "the field took the frigate's fire");
    // The sentinels that reached their slots sit on the sphere around the guarded ship.
    let centre = run.universe.ship(guarded).unwrap().coll.coll_position;
    let r = run.universe.ship(ids[0]).unwrap().formation_offset;
    let radius = (r.x * r.x + r.y * r.y + r.z * r.z).sqrt();
    for &i in ids.iter().filter(|&&i| field(&run, i).is_some_and(|f| f.in_position)) {
        let p = run.universe.ship(i).unwrap().posinfo.position;
        let d = ((p.x - centre.x).powi(2) + (p.y - centre.y).powi(2) + (p.z - centre.z).powi(2)).sqrt();
        assert!((d - radius).abs() < 60.0, "sentinel {i:?} at {d}, sphere radius {radius}");
    }
    let leader = run.universe.commands.get(cid).unwrap().selection[0];
    mr_formation_revert_special(&mut run.universe, &[leader]);
    let cid = run.universe.ship(ids[0]).unwrap().command.expect("command");
    let c = run.universe.commands.get(cid).unwrap();
    assert_eq!(c.formation.formation_type, kind::SPHERE);
    assert!(c.attributes & attr::PROTECTING != 0);
    for _ in 0..4 {
        run.step();
    }
    assert!(ids.iter().all(|&i| !sentinel::field_is_up(&run.universe, i)));
    assert!(ids.iter().all(|&i| run.universe.ship(i).unwrap().base.clamp.as_ref().and_then(|c| c.host).is_none()));
    // No target: a field wall.
    mr_protect_ships(&mut run.universe, &ids[..5], None);
    let cid = run.universe.ship(ids[0]).unwrap().command.expect("command");
    assert_eq!(run.universe.commands.get(cid).unwrap().formation.formation_type, kind::SENTINEL_WALL);
}

/// An ordinary sphere formation guarding a ship surrounds it: the escorts fly to slots on a
/// sphere around the guarded ship (the original left this to `formationSpecificsSphere`).
#[test]
fn sphere_formation_surrounds_the_guarded_ship() {
    use hwc_sim::command::{cl_formation, cl_protect};
    use hwc_sim::formation::kind;
    let mut spawns = Vec::new();
    for i in 0..6 {
        spawns.push(Spawn { ship_type: ShipType::sAcolyte, player: 0, position: v(-1500.0, (i as f32 - 2.5) * 100.0, 0.0) });
    }
    spawns.push(Spawn { ship_type: ShipType::sProcessor, player: 0, position: v(0.0, 0.0, 0.0) });
    let s = scenario("sphere guard", spawns, Vec::new());
    let Some(mut run) = new_run(s) else { return };
    let ids: Vec<ShipId> = run.ids[..6].to_vec();
    let guarded = run.ids[6];
    cl_protect(&mut run.universe, &ids, &[guarded]);
    cl_formation(&mut run.universe, &ids, kind::SPHERE);
    for _ in 0..900 {
        run.step();
    }
    let centre = run.universe.ship(guarded).unwrap().posinfo.position;
    for &i in &ids {
        let s = run.universe.ship(i).unwrap();
        let o = s.formation_offset;
        let want = (o.x * o.x + o.y * o.y + o.z * o.z).sqrt();
        assert!(want >= 500.0, "slot radius {want}");
        let p = s.posinfo.position;
        let d = ((p.x - centre.x).powi(2) + (p.y - centre.y).powi(2) + (p.z - centre.z).powi(2)).sqrt();
        assert!((d - want).abs() < 150.0, "escort {i:?} at {d}, slot radius {want}");
    }
}
