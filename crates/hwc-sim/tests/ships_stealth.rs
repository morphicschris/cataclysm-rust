//! Stealth ships over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped when
//! neither is set): Mimic disguises, the Leech, cloakers and proximity sensors; every scenario
//! runs twice and must give the same hashes.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::object::ShipId;
use hwc_sim::ships::{leech, mimic, ShipSpec};
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

fn scenario(
    name: &'static str,
    players: Vec<ShipRace>,
    spawns: Vec<Spawn>,
    orders: Vec<(u32, Order)>,
) -> Scenario {
    Scenario {
        name,
        players,
        spawns,
        orders,
        resources: Vec::new(),
        resource_units: Vec::new(),
    }
}

/// A run with a hook to adjust the universe once before the first tick (upgrades...).
fn start(s: Scenario, setup: impl Fn(&mut Run)) -> Option<Run> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    setup(&mut run);
    Some(run)
}

fn upgrade(run: &mut Run, i: usize, bits: u32) {
    let id = run.ids[i];
    run.universe.ships.ship_mut(id).upgrade_flags |= bits;
}

fn sees(u: &Universe, viewer: ShipId, target: ShipId) -> bool {
    hwc_sim::tactics::enemies_within_proximity(u, viewer, 20000.0).contains(&target)
}

fn targeted_by_commands(u: &Universe, t: ShipId) -> bool {
    u.commands.iter().any(|c| c.attack_targets.contains(&t))
}

fn mimic_state(u: &Universe, id: ShipId) -> u8 {
    match &u.ships.ship(id).spec {
        ShipSpec::Mimic(m) => m.state,
        _ => panic!("not a mimic"),
    }
}

/// A Mimic takes on the look of an enemy Beast recon: once the morph is done that player does
/// not see it (no proximity contact) while its owner still does; ending the disguise shows it
/// again. (A disguise as a type of another race, above 0x3b, hides it from every player.)
#[test]
fn mimic_disguises_and_is_ignored() {
    let s = scenario(
        "mimic",
        vec![ShipRace::Somtaaw, ShipRace::Beast],
        vec![
            Spawn {
                ship_type: ShipType::sMimic,
                player: 0,
                position: Vector::new(0.0, 0.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::bRecon,
                player: 1,
                position: Vector::new(5000.0, 0.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::bRecon,
                player: 1,
                position: Vector::new(5500.0, 400.0, 0.0),
            },
        ],
        vec![
            (
                1,
                Order::Special {
                    ships: vec![0],
                    targets: vec![1],
                },
            ),
            (
                200,
                Order::Special {
                    ships: vec![0],
                    targets: vec![],
                },
            ),
        ],
    );
    let go = |s: Scenario| {
        let mut run = start(s, |r| upgrade(r, 0, mimic::UPGRADE_DISGUISE))?;
        let (m, f) = (run.ids[0], run.ids[2]);
        let mut hashes = Vec::new();
        let mut seen_before = false;
        let mut hidden_at = None;
        let mut shown_again = None;
        for t in 0..320u32 {
            hashes.push(run.step());
            let u = &run.universe;
            if t == 0 {
                seen_before = sees(u, f, m);
            }
            if hidden_at.is_none() && mimic_state(u, m) == mimic::state::DISGUISED {
                hidden_at = Some(t);
                assert!(mimic::is_disguised_as_player(u, m, 1));
                assert!(!mimic::fools_player_as_resource(u, m, 1));
                assert!(!hwc_sim::visibility::visible_to_player(u, m, 1));
                assert!(
                    hwc_sim::visibility::visible_to_player(u, m, 0),
                    "the owner still sees it"
                );
                assert_eq!(mimic::apparent_ship_type(u, m), Some(ShipType::bRecon));
            }
            if hidden_at.is_some() && t < 200 {
                assert!(
                    !sees(u, f, m),
                    "tick {t}: the recon saw the disguised Mimic"
                );
            }
            if t > 200 && shown_again.is_none() && mimic_state(u, m) == mimic::state::NONE {
                shown_again = Some(t);
            }
        }
        Some((hashes, seen_before, hidden_at, shown_again, run))
    };
    let Some((h1, seen_before, hidden_at, shown_again, run)) = go(s.clone()) else {
        return;
    };
    println!("mimic: hidden at {hidden_at:?}, shown again at {shown_again:?}");
    assert!(seen_before, "the recon sees the Mimic before it disguises");
    let hidden_at = hidden_at.expect("the Mimic disguised");
    assert!(
        hidden_at > 20 && hidden_at < 60,
        "the morph takes a short delay plus 20 frames ({hidden_at})"
    );
    assert!(shown_again.is_some(), "the disguise ended");
    let u = &run.universe;
    assert!(hwc_sim::visibility::visible_to_player(u, run.ids[0], 1));
    assert!(sees(u, run.ids[2], run.ids[0]));
    let (h2, ..) = go(s).unwrap();
    assert_eq!(h1, h2, "mimic runs differ");
}

/// Without the disguise upgrade the special does nothing.
#[test]
fn mimic_needs_the_upgrade() {
    let s = scenario(
        "mimic no upgrade",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            Spawn {
                ship_type: ShipType::sMimic,
                player: 0,
                position: Vector::new(0.0, 0.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::tHeavyInterceptor,
                player: 1,
                position: Vector::new(800.0, 0.0, 0.0),
            },
        ],
        vec![(
            1,
            Order::Special {
                ships: vec![0],
                targets: vec![1],
            },
        )],
    );
    let Some(mut run) = start(s, |_| {}) else {
        return;
    };
    for _ in 0..100 {
        run.step();
    }
    assert_eq!(mimic_state(&run.universe, run.ids[0]), mimic::state::NONE);
}

/// A Leech in resource mode latches onto an ion cannon frigate, drains resource units and
/// health every second, and stays hidden from the frigate's player.
#[test]
fn leech_latches_and_drains() {
    let s = scenario(
        "leech",
        vec![ShipRace::Somtaaw, ShipRace::Taiidan],
        vec![
            Spawn {
                ship_type: ShipType::sLeech,
                player: 0,
                position: Vector::new(-1500.0, 200.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::tIonCannonFrigate,
                player: 1,
                position: Vector::new(0.0, 0.0, 0.0),
            },
        ],
        vec![(
            1,
            Order::Special {
                ships: vec![0],
                targets: vec![1],
            },
        )],
    );
    let go = |s: Scenario| {
        let mut run = start(s, |r| {
            upgrade(r, 0, leech::UPGRADE_LEECH);
            let id = r.ids[0];
            leech::set_state(&mut r.universe, id, leech::state::DRAIN_RUS);
        })?;
        let (l, f) = (run.ids[0], run.ids[1]);
        let max = run
            .universe
            .statics
            .ship(ShipType::tIonCannonFrigate)
            .unwrap()
            .health
            .max_health;
        let mut hashes = Vec::new();
        let mut latched_at = None;
        let mut states = Vec::new();
        for t in 0..1600u32 {
            hashes.push(run.step());
            let u = &run.universe;
            let st = match &u.ships.ship(l).spec {
                ShipSpec::Leech(x) => x.state,
                _ => unreachable!(),
            };
            if states.last() != Some(&st) {
                states.push(st);
            }
            if latched_at.is_none() && st == leech::state::DRAIN_RUS {
                latched_at = Some(t);
            }
            assert!(
                !hwc_sim::visibility::visible_to_player(u, l, 1),
                "tick {t}: the Leech was seen"
            );
        }
        let u = &run.universe;
        let ru = u.ships.ship(l).base.resources;
        let health = u.ships.get(f).map(|x| x.health);
        Some((hashes, latched_at, states, ru, health, max, run))
    };
    let Some((h1, latched_at, states, ru, health, max, run)) = go(s.clone()) else {
        return;
    };
    println!("leech: latched at {latched_at:?}, states {states:?}, RUs {ru}, frigate health {health:?} of {max}");
    let latched_at = latched_at.expect("the Leech latched on");
    assert!(states.contains(&leech::state::LATCH));
    let u = &run.universe;
    assert_eq!(
        u.ships
            .ship(run.ids[0])
            .base
            .clamp
            .as_ref()
            .and_then(|c| c.host),
        Some(run.ids[1]),
        "clamped to the frigate"
    );
    // One tick of RUs and damage per second of drain.
    let secs = (1600 - latched_at) as f32 / 16.0;
    assert!(ru > 0 && ru as f32 <= secs + 1.0, "RUs {ru} after {secs} s");
    let h = health.expect("the frigate is alive");
    let rate = u
        .statics
        .ship(ShipType::tIonCannonFrigate)
        .unwrap()
        .leech_hps_per_second;
    assert!(
        h < max && max - h <= rate * (secs + 1.0),
        "damage {} after {secs} s",
        max - h
    );
    assert!(leech::is_active(u, run.ids[0]));
    let (h2, ..) = go(s).unwrap();
    assert_eq!(h1, h2, "leech runs differ");
}

fn cloak_of(u: &Universe, id: ShipId) -> hwc_sim::cloak::Cloak {
    *hwc_sim::ships::cloak_ref(u.ships.ship(id)).expect("cloaker")
}

/// A cloaked fighter under attack cloaks: power drains while cloaked, the attacker drops it as
/// a target and no longer sees it; decloaking shows it again and the power recharges.
#[test]
fn cloaked_fighter_cloaks_and_is_dropped_as_target() {
    let s = scenario(
        "cloak",
        vec![ShipRace::Beast, ShipRace::Taiidan],
        vec![
            Spawn {
                ship_type: ShipType::bCloakedFighter,
                player: 0,
                position: Vector::new(0.0, 0.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::tStandardFrigate,
                player: 1,
                position: Vector::new(8000.0, 0.0, 0.0),
            },
        ],
        vec![
            (
                1,
                Order::Attack {
                    ships: vec![1],
                    targets: vec![0],
                },
            ),
            (
                2,
                Order::Special {
                    ships: vec![0],
                    targets: vec![],
                },
            ),
            (
                300,
                Order::Special {
                    ships: vec![0],
                    targets: vec![],
                },
            ),
        ],
    );
    let go = |s: Scenario| {
        let mut run = start(s, |_| {})?;
        let (c, f) = (run.ids[0], run.ids[1]);
        let mut hashes = Vec::new();
        let mut events = Vec::new();
        let mut cloaked_at = None;
        let mut targeted_before = false;
        let mut power_at_decloak = 0.0;
        for t in 0..500u32 {
            hashes.push(run.step());
            let u = &run.universe;
            events.extend(
                u.special_events
                    .iter()
                    .filter(|e| {
                        matches!(
                            e,
                            SpecialEvent::Cloaked { .. } | SpecialEvent::Decloaked { .. }
                        )
                    })
                    .cloned(),
            );
            if t == 1 {
                targeted_before = targeted_by_commands(u, c);
            }
            let cl = hwc_sim::cloak::is_cloaked(u.ships.ship(c));
            if cloaked_at.is_none() && cl {
                cloaked_at = Some(t);
                assert!(
                    !targeted_by_commands(u, c),
                    "the attacker dropped the cloaked fighter"
                );
                assert!(!hwc_sim::visibility::visible_to_player(u, c, 1));
            }
            if cl {
                assert!(
                    !sees(u, f, c),
                    "tick {t}: the frigate sees the cloaked fighter"
                );
            }
            if t == 299 {
                power_at_decloak = cloak_of(u, c).power;
            }
        }
        Some((
            hashes,
            events,
            cloaked_at,
            targeted_before,
            power_at_decloak,
            run,
        ))
    };
    let Some((h1, events, cloaked_at, targeted_before, power, run)) = go(s.clone()) else {
        return;
    };
    println!("cloak: cloaked at {cloaked_at:?}, power at 299 {power}, events {events:?}");
    assert!(targeted_before, "the frigate was attacking the fighter");
    let cloaked_at = cloaked_at.expect("the fighter cloaked");
    assert!(cloaked_at > 20 && cloaked_at < 60);
    // 0.1 per update while cloaked.
    let expect = 1000.0 - 0.1 * (299 - cloaked_at) as f32;
    assert!(
        (power - expect).abs() < 0.5,
        "power {power}, expected about {expect}"
    );
    assert_eq!(events.len(), 2, "cloaked and decloaked once each");
    let u = &run.universe;
    let c = run.ids[0];
    assert!(!hwc_sim::cloak::is_cloaked(u.ships.ship(c)));
    let k = cloak_of(u, c);
    assert_eq!(k.state, hwc_sim::cloak::state::VISIBLE);
    assert!(k.power > power, "the power recharges while visible");
    assert!(hwc_sim::visibility::visible_to_player(u, c, 1));
    let (h2, ..) = go(s).unwrap();
    assert_eq!(h1, h2, "cloak runs differ");
}

/// A proximity sensor with the sensors upgrade reveals a cloaked fighter to its player (the
/// detector capability); without the upgrade the fighter stays hidden. The sensor marks what it
/// senses either way.
#[test]
fn proximity_sensor_reveals_cloaked_ship() {
    let s = scenario(
        "sensor",
        vec![ShipRace::Beast, ShipRace::Kushan],
        vec![
            Spawn {
                ship_type: ShipType::bCloakedFighter,
                player: 0,
                position: Vector::new(0.0, 0.0, 0.0),
            },
            Spawn {
                ship_type: ShipType::kProximitySensor,
                player: 1,
                position: Vector::new(4700.0, 0.0, 0.0),
            },
        ],
        vec![(
            2,
            Order::Special {
                ships: vec![0],
                targets: vec![],
            },
        )],
    );
    let go = |s: Scenario, detector: bool| {
        let mut run = start(s, |r| {
            if detector {
                upgrade(r, 1, hwc_sim::visibility::UPGRADE_DETECTOR);
            }
        })?;
        let (c, p) = (run.ids[0], run.ids[1]);
        let mut hashes = Vec::new();
        let mut cloaked_at = None;
        for t in 0..100u32 {
            hashes.push(run.step());
            if cloaked_at.is_none() && hwc_sim::cloak::is_cloaked(run.universe.ships.ship(c)) {
                cloaked_at = Some(t);
            }
        }
        let u = &run.universe;
        let visible = hwc_sim::visibility::visible_to_player(u, c, 1);
        let sensed = u.ships.ship(c).sensed_by & 2 != 0;
        let sensor_state = match &u.ships.ship(p).spec {
            ShipSpec::ProximitySensor(x) => x.state,
            _ => unreachable!(),
        };
        Some((
            hashes,
            cloaked_at,
            visible,
            sensed,
            sensor_state,
            sees(u, p, c),
        ))
    };
    let Some((h1, cloaked_at, visible, sensed, sensor_state, seen)) = go(s.clone(), true) else {
        return;
    };
    println!("sensor: cloaked at {cloaked_at:?}, visible {visible}, sensed {sensed}, state {sensor_state}");
    assert!(cloaked_at.is_some());
    assert!(
        visible && seen,
        "the sensor's player sees the cloaked fighter"
    );
    assert!(sensed);
    assert_eq!(sensor_state, hwc_sim::ships::proximity::state::TRACKING);
    let (h2, ..) = go(s.clone(), true).unwrap();
    assert_eq!(h1, h2, "sensor runs differ");
    let (_, cloaked_at, visible, sensed, _, seen) = go(s, false).unwrap();
    assert!(cloaked_at.is_some());
    assert!(!visible && !seen, "no detector upgrade: still hidden");
    assert!(sensed, "the sensor still senses it");
}
