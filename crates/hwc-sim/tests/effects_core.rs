//! Core effects over the real game data (`HWC_EXTRACTED_DIR`; skipped when unset): guns fire
//! with their muzzle and bullet effects, hits make hit effects, dead ships stay as hulks until
//! their explosion deletes them, and runs with effects attached are deterministic.

use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

use hwc_sim::effects::{self, EffectData};
use hwc_sim::harness::{scenario, world_hash, Run};
use hwc_sim::object::sof;
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, StaticWorld};
use hwc_sim::universe::SimEvent;
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

fn run(name: &str, with_fx: bool) -> Option<Run> {
    let d = data()?;
    let mut r = Run::new(d.statics.clone(), d.tweaks.clone(), scenario(name).unwrap()).unwrap();
    if with_fx {
        effects::attach(&mut r.universe, &d.effects);
    }
    Some(r)
}

fn effect_names(r: &Run) -> Vec<String> {
    let fx = r.universe.effects.as_ref().unwrap();
    fx.world.effects.iter().map(|e| fx.world.lib.statics[e.stat].name.to_ascii_lowercase()).collect()
}

#[test]
fn dogfight_makes_gun_and_hit_effects_deterministically() {
    let Some(mut a) = run("dogfight", true) else { return };
    let mut b = run("dogfight", true).unwrap();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let (mut fired, mut hits, mut max_effects) = (0, 0, 0);
    for _ in 0..1200 {
        let ha = a.step();
        let hb = b.step();
        assert_eq!(ha, hb);
        for e in &a.universe.events {
            match e {
                SimEvent::GunFired { .. } => fired += 1,
                SimEvent::BulletHit { .. } => hits += 1,
                _ => {}
            }
        }
        seen.extend(effect_names(&a));
        max_effects = max_effects.max(a.universe.effects.as_ref().unwrap().world.effects.len());
        // Every bullet effect belongs to a live bullet.
        let fx = a.universe.effects.as_ref().unwrap();
        for bid in fx.bullet_effect.keys() {
            assert!(a.universe.bullets.get(*bid).is_some());
        }
    }
    assert!(fired > 0 && hits > 0, "fired {fired} hits {hits}");
    assert!(max_effects > 0);
    eprintln!("dogfight: fired {fired} hits {hits} max effects {max_effects} seen {seen:?}");
    assert!(seen.len() >= 2, "effects seen: {seen:?}");
}

#[test]
fn dead_ships_stay_as_hulks_until_their_explosion_ends() {
    let Some(mut r) = run("frigates", true) else { return };
    let mut died_at: Option<(hwc_sim::ShipId, u32)> = None;
    let mut wiped_at = None;
    let mut hidden = false;
    for tick in 0..6000 {
        r.step();
        for e in &r.universe.events {
            if let SimEvent::ShipDied { ship, .. } = e {
                died_at.get_or_insert((*ship, tick));
            }
        }
        if let Some((id, t0)) = died_at {
            if wiped_at.is_none() {
                match r.universe.ships.get(id) {
                    Some(s) if tick > t0 => {
                        assert!(s.flags & sof::DEAD != 0);
                        assert!(!r.universe.ship_list.contains(&id));
                        assert!(r.universe.hulks.contains(&id));
                        hidden |= s.flags & sof::HIDE != 0;
                    }
                    Some(_) => {}
                    None => wiped_at = Some(tick),
                }
            } else {
                break;
            }
        }
    }
    let (_, t0) = died_at.expect("a ship died");
    let t1 = wiped_at.expect("the hulk was deleted by its explosion");
    // The death effects keep the hulk for between half a second and ten seconds.
    assert!(t1 > t0 + 4 && t1 <= t0 + 170, "died {t0} wiped {t1}");
    eprintln!("hulk: died {t0} wiped {t1} hidden {hidden}");
}

#[test]
fn without_effects_nothing_changes_and_ships_leave_at_once() {
    let Some(mut r) = run("frigates", false) else { return };
    for _ in 0..3000 {
        r.step();
        assert!(r.universe.hulks.is_empty());
    }
    assert!(r.universe.effects.is_none());
}

#[test]
fn effects_runs_replay_the_same_hashes() {
    let Some(mut a) = run("frigates", true) else { return };
    let mut b = run("frigates", true).unwrap();
    for _ in 0..3000 {
        a.step();
        b.step();
        assert_eq!(world_hash(&a.universe), world_hash(&b.universe));
        // Drawing must not change the simulation.
        let mut list = hwc_fx::RenderList::default();
        effects::draw(&mut a.universe, &mut list);
    }
    assert_eq!(world_hash(&a.universe), world_hash(&b.universe));
}

#[test]
fn big_death_keeps_the_hulk_for_the_big_explosion() {
    let Some(mut r) = run("big_death", true) else { return };
    let moship = r.ids[3];
    let mut died = None;
    for tick in 0..3000u32 {
        r.step();
        if died.is_none() && r.universe.hulks.contains(&moship) {
            died = Some(tick);
            let fx = r.universe.effects.as_ref().unwrap();
            let names: Vec<String> = fx.world.effects.iter().filter(|e| e.owner == Some(effects::FxOwner::Ship(moship).id())).map(|e| fx.world.lib.statics[e.stat].name.to_ascii_lowercase()).collect();
            assert!(names.iter().any(|n| n.starts_with("bigdeath")), "{names:?}");
        }
        if let Some(t0) = died {
            if r.universe.ships.get(moship).is_none() {
                // `bigdeath.ebg` deletes its parent at 10 s.
                let lived = tick - t0;
                assert!((150..=170).contains(&lived), "hulk lived {lived} updates");
                return;
            }
        }
    }
    panic!("the mothership did not die and leave: died {died:?}");
}
