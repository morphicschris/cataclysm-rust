//! Combat scenarios over the real game data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped
//! when neither is set): ships fire, hit, damage and kill each other, deterministically.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_sim::harness::{scenario, Run};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, StaticWorld};
use hwc_sim::universe::SimEvent;
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

#[derive(Debug, Default)]
struct Tally {
    fired: usize,
    hits: usize,
    deaths: Vec<(u32, u16)>,
    hashes: Vec<u64>,
}

fn play(name: &str, ticks: u32) -> Option<(Tally, Run)> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), scenario(name).unwrap()).unwrap();
    let mut t = Tally::default();
    for tick in 0..ticks {
        t.hashes.push(run.step());
        for e in &run.universe.events {
            match e {
                SimEvent::GunFired { .. } => t.fired += 1,
                SimEvent::BulletHit { .. } => t.hits += 1,
                SimEvent::ShipDied { ship, .. } => t.deaths.push((tick, ship.0)),
                _ => {}
            }
        }
        if std::env::var("HWC_PRINT").is_ok() && tick % 80 == 0 {
            for &id in &run.ids {
                if let Some(s) = run.universe.ship(id) {
                    let p = s.posinfo.position;
                    println!(
                        "t{tick} {} {:?} hp {:.0} pos ({:.0},{:.0},{:.0}) cmd {:?} tgt {:?} as {} fm {}",
                        id.0, s.ship_type, s.health, p.x, p.y, p.z, s.command, s.attack.target.map(|t| t.0), s.ai_state_attack, s.flightman
                    );
                }
            }
        }
    }
    Some((t, run))
}

#[test]
fn dogfight_fights_to_a_result() {
    let Some((a, run)) = play("dogfight", 1600) else { return };
    println!("dogfight: fired {} hits {} deaths {:?}", a.fired, a.hits, a.deaths);
    assert!(a.fired > 50, "guns fire");
    assert!(a.hits > 10, "bullets hit");
    assert!(!a.deaths.is_empty(), "ships die");
    // Dead ships are gone from the universe.
    for (_, id) in &a.deaths {
        assert!(run.universe.ship(hwc_sim::ShipId(*id)).is_none());
    }
    let (b, _) = play("dogfight", 1600).unwrap();
    assert_eq!(a.hashes, b.hashes);
}

#[test]
fn frigates_engage_a_destroyer() {
    let Some((a, run)) = play("frigates", 1600) else { return };
    println!("frigates: fired {} hits {} deaths {:?}", a.fired, a.hits, a.deaths);
    assert!(a.fired > 10, "guns fire");
    assert!(a.hits > 10, "beams and shells hit");
    let d = run.universe.ship(run.ids[3]).map(|s| s.health);
    println!("destroyer health {d:?}");
    let max = run.universe.statics.ship(hwc_sim::statics::ShipType::tStandardDestroyer).unwrap().health.max_health;
    assert!(d.is_none_or(|h| h < max), "the destroyer takes damage");
    let (b, _) = play("frigates", 1600).unwrap();
    assert_eq!(a.hashes, b.hashes);
}
