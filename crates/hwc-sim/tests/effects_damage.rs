//! Damage and hyperspace effects over the real game data (`HWC_EXTRACTED_DIR` for the damage
//! scenarios, `HWC_DATA_DIR` / `HWC_GAME_DIR` for Mission01; skipped when unset): damage
//! effects start per level as the health falls and stop when it rises, runs with effects are
//! deterministic, and the fleet arriving at the start of Mission01 opens one hyperspace window
//! per arriving ship that is not clamped to another.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use hwc_fx::runtime::EffectId;
use hwc_math::Vector;
use hwc_sim::effects::{damage, hyperspace, EffectData, FxOwner};
use hwc_sim::harness::{Run, Scenario, Spawn};
use hwc_sim::object::ShipId;
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
        let Ok(dir) = std::env::var("HWC_EXTRACTED_DIR") else {
            eprintln!("HWC_EXTRACTED_DIR not set; skipping");
            return None;
        };
        let src: Box<dyn DataSource> = Box::new(DirSource::new(dir).ok()?);
        let statics = Arc::new(StaticWorld::load_from(src.as_ref(), LoadOptions::default()));
        let tweaks = SimTweaks::load(src.as_ref());
        let effects = EffectData::load_from(src.as_ref());
        (!effects.is_empty()).then_some(Data { statics, tweaks, effects })
    })
    .as_ref()
}

/// A capital ship type whose mesh has an engine nozzle and damage points (so every level can
/// show).
fn damageable(d: &Data) -> ShipType {
    let candidates = [ShipType::sCarrier, ShipType::sDreadnought, ShipType::sMultiBeamFrigate, ShipType::sHiveFrigate];
    for t in candidates {
        let Some(st) = d.statics.ship(t) else { continue };
        let Some(mex) = st.header.mex.as_ref() else { continue };
        if mex.find("Eng", "Eng0").is_some() && !damage::damage_lights(mex).is_empty() {
            return t;
        }
    }
    panic!("no candidate ship has Eng0 and Dmg chunks");
}

fn one_ship(t: ShipType) -> Scenario {
    Scenario {
        name: "damage effects",
        players: vec![ShipRace::Somtaaw],
        spawns: vec![Spawn { ship_type: t, player: 0, position: Vector::new(0.0, 0.0, 0.0) }],
        orders: Vec::new(),
        resources: Vec::new(),
        resource_units: Vec::new(),
    }
}

fn set_health(run: &mut Run, id: ShipId, ratio: f32) {
    let t = run.universe.ships.ship(id).ship_type;
    let max = run.universe.statics.ship(t).unwrap().health.max_health;
    run.universe.ships.ship_mut(id).health = max * ratio;
}

/// Damage effects playing per level.
fn counts(run: &Run, id: ShipId) -> [usize; 3] {
    match damage::slots(&run.universe, id) {
        Some(s) => [s.count(0), s.count(1), s.count(2)],
        None => [0; 3],
    }
}

/// Every effect in a slot still exists.
fn slots_alive(run: &Run, id: ShipId) -> bool {
    damage::slots(&run.universe, id).is_none_or(|s| s.effects().all(|(_, e)| hwc_sim::effects::exists(&run.universe, e)))
}

/// Runs health `ratio` for `ticks` updates and returns the most effects seen per level and
/// the hashes.
fn hold(run: &mut Run, id: ShipId, ratio: f32, ticks: u32) -> ([usize; 3], Vec<u64>) {
    let mut most = [0usize; 3];
    let mut hashes = Vec::new();
    for _ in 0..ticks {
        set_health(run, id, ratio);
        hashes.push(run.step());
        let c = counts(run, id);
        for l in 0..3 {
            most[l] = most[l].max(c[l]);
            assert!(c[l] <= damage::MAX_EFFECTS[l], "level {l} over its limit");
        }
        assert!(slots_alive(run, id), "a slot refers to a deleted effect");
    }
    (most, hashes)
}

fn damage_run(d: &Data) -> (Vec<u64>, Vec<[usize; 3]>) {
    let t = damageable(d);
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), one_ship(t)).unwrap();
    hwc_sim::effects::attach(&mut run.universe, &d.effects);
    let id = run.ids[0];
    let mut all = Vec::new();
    let mut seen = Vec::new();
    // Full health: nothing.
    let (m, h) = hold(&mut run, id, 1.0, 20);
    all.extend(h);
    seen.push(m);
    // Between the heavy and the light limit: the light level only.
    let (m, h) = hold(&mut run, id, 0.3, 200);
    all.extend(h);
    seen.push(m);
    // Between the dying and the heavy limit: light and heavy.
    let (m, h) = hold(&mut run, id, 0.1, 200);
    all.extend(h);
    seen.push(m);
    // Nearly dead (every think may start one): every level, up to its limit.
    let (m, h) = hold(&mut run, id, 0.01, 50);
    all.extend(h);
    seen.push(m);
    // Repaired: everything stops at once.
    set_health(&mut run, id, 1.0);
    all.push(run.step());
    seen.push(counts(&run, id));
    // Below the heavy limit again, then just above it: the heavy and dying levels stop.
    let (_, h) = hold(&mut run, id, 0.01, 50);
    all.extend(h);
    set_health(&mut run, id, 0.3);
    all.push(run.step());
    let c = counts(&run, id);
    seen.push([c[0].min(1), c[1], c[2]]);
    (all, seen)
}

#[test]
fn damage_effects_follow_health() {
    let Some(d) = data() else { return };
    let (_, seen) = damage_run(d);
    assert_eq!(seen[0], [0, 0, 0], "full health");
    assert_eq!(seen[1], [1, 0, 0], "below the light limit");
    assert_eq!(seen[2], [1, 2, 0], "below the heavy limit");
    assert_eq!(seen[3], [1, 2, 1], "below the dying limit");
    assert_eq!(seen[4], [0, 0, 0], "repaired");
    assert_eq!(seen[5][1..], [0, 0], "heavy and dying stop above their limits");
}

#[test]
fn damage_effects_are_deterministic() {
    let Some(d) = data() else { return };
    let (a, _) = damage_run(d);
    let (b, _) = damage_run(d);
    assert_eq!(a, b);
}

#[test]
fn damage_effects_leave_gameplay_alone() {
    let Some(d) = data() else { return };
    let t = damageable(d);
    // Same health schedule with and without effects: the world hashes agree (damage effects
    // change no gameplay state except on the crystals).
    let run_with = |fx: bool| {
        let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), one_ship(t)).unwrap();
        if fx {
            hwc_sim::effects::attach(&mut run.universe, &d.effects);
        }
        let id = run.ids[0];
        let mut h = Vec::new();
        for (ratio, n) in [(0.3, 60), (0.01, 60), (1.0, 10)] {
            for _ in 0..n {
                set_health(&mut run, id, ratio);
                h.push(run.step());
            }
        }
        h
    };
    assert_eq!(run_with(true), run_with(false));
}

#[test]
fn damaged_crystal_flickers_instead() {
    let Some(d) = data() else { return };
    if d.statics.ship(ShipType::dCrystal0).is_none() {
        return;
    }
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), one_ship(ShipType::dCrystal0)).unwrap();
    hwc_sim::effects::attach(&mut run.universe, &d.effects);
    let id = run.ids[0];
    let class = d.statics.ship(ShipType::dCrystal0).unwrap().ship_class.map_or(0, |c| c.index());
    if run.universe.effects.as_ref().unwrap().tables.damage_effect(class, 0).is_none() {
        return;
    }
    let mut flickered = false;
    for _ in 0..20 {
        set_health(&mut run, id, 0.01);
        run.step();
        let s = run.universe.ships.ship(id);
        flickered |= s.flicker.state == 2;
        assert_eq!(counts(&run, id), [0, 0, 0], "crystals show no damage effects");
    }
    assert!(flickered, "the crystal never flickered");
}

// -------------------------------------------------------------------------------- Mission01

mod mission {
    use super::*;
    use hwc_formats::gamedata::GameData;
    use hwc_kas::Mission;
    use hwc_sim::harness::world_hash;
    use hwc_sim::mission::{load_kas_mission, sp_state, GameDataSource, MissionConfig, MissionRun};

    struct MData {
        gd: GameData,
        statics: Arc<StaticWorld>,
        tweaks: SimTweaks,
        script: Mission,
        effects: EffectData,
    }

    fn mdata() -> Option<&'static MData> {
        static DATA: OnceLock<Option<MData>> = OnceLock::new();
        DATA.get_or_init(|| {
            let gd = match GameData::from_env() {
                Some(Ok(gd)) => gd,
                _ => {
                    eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
                    return None;
                }
            };
            let script = load_kas_mission(&gd, "Mission01").ok()?;
            let src = GameDataSource(&gd);
            let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
            let tweaks = SimTweaks::load(&src);
            let effects = EffectData::load_from(&src);
            if effects.is_empty() {
                return None;
            }
            Some(MData { gd, statics, tweaks, script, effects })
        })
        .as_ref()
    }

    fn start(d: &MData) -> MissionRun {
        let mut config = MissionConfig::new(1);
        config.log_calls = false;
        config.speech_seconds = Some(3.0);
        let mut run = MissionRun::start(d.statics.clone(), d.tweaks.clone(), &GameDataSource(&d.gd), d.script.clone(), config).expect("mission starts");
        hwc_sim::effects::attach(&mut run.universe, &d.effects);
        run
    }

    struct Arrival {
        /// Window effects seen, by effect, with their owner.
        windows: BTreeMap<EffectId, ShipId>,
        /// Arriving ships and whether they show a window.
        arriving: BTreeMap<ShipId, bool>,
        /// Windows still alive when every ship had arrived.
        left: usize,
        hashes: Vec<u64>,
    }

    fn arrive(d: &MData) -> Arrival {
        let mut run = start(d);
        let window = run.universe.effects.as_ref().unwrap().tables.hyperspace.as_ref().and_then(|l| l.first().copied()).expect("hyperspace event");
        let mut windows = BTreeMap::new();
        let mut arriving = BTreeMap::new();
        let mut hashes = Vec::new();
        let mut began = false;
        for _ in 0..4000 {
            run.step();
            hashes.push(world_hash(&run.universe));
            for a in &run.game.arriving {
                if let Some(id) = a.ship {
                    arriving.entry(id).or_insert_with(|| hyperspace::should_display(&run.universe, id));
                }
            }
            let fx = run.universe.effects.as_ref().unwrap();
            for e in fx.world.effects.iter().filter(|e| e.stat == window) {
                if let Some(FxOwner::Ship(s)) = e.owner.and_then(FxOwner::from_id) {
                    windows.insert(e.id, s);
                }
            }
            began |= run.game.sp_state == sp_state::ARRIVING;
            if began && run.game.ships_have_arrived {
                break;
            }
        }
        assert!(run.game.ships_have_arrived, "the fleet never arrived");
        let fx = run.universe.effects.as_ref().unwrap();
        let left = fx.world.effects.iter().filter(|e| e.stat == window).count();
        Arrival { windows, arriving, left, hashes }
    }

    #[test]
    fn mission01_arrival_opens_one_window_per_ship() {
        let Some(d) = mdata() else { return };
        let a = arrive(d);
        assert!(!a.arriving.is_empty());
        let shown: BTreeSet<ShipId> = a.arriving.iter().filter(|(_, &s)| s).map(|(&id, _)| id).collect();
        let owners: Vec<ShipId> = a.windows.values().copied().collect();
        let distinct: BTreeSet<ShipId> = owners.iter().copied().collect();
        assert!(!shown.is_empty(), "no arriving ship shows a window");
        assert_eq!(owners.len(), distinct.len(), "a ship opened two windows");
        assert_eq!(distinct, shown, "windows for exactly the ships that are not clamped");
        assert_eq!(a.left, 0, "every window closed when the fleet was in");
    }

    #[test]
    fn mission01_arrival_with_effects_is_deterministic() {
        let Some(d) = mdata() else { return };
        let a = arrive(d);
        let b = arrive(d);
        assert_eq!(a.hashes, b.hashes);
        assert_eq!(a.windows, b.windows);
    }
}
