//! Run-time simulation tests over the real game data (`HWC_GAME_DIR` with `cataclysm.big`, or
//! `HWC_EXTRACTED_DIR` with an extracted copy). Skipped when neither is set.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_sim::harness::{scenario, Run};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, StaticWorld};
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

fn run(name: &str, ticks: u32) -> Option<(Vec<u64>, Run)> {
    let d = data()?;
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), scenario(name).unwrap()).unwrap();
    let hashes = run.run(ticks);
    Some((hashes, run))
}

#[test]
fn two_runs_hash_identically() {
    for name in hwc_sim::harness::SCENARIOS {
        let Some((a, _)) = run(name, 600) else { return };
        let (b, _) = run(name, 600).unwrap();
        assert_eq!(a, b, "{name}");
        // The state actually changes over time.
        assert_ne!(a[10], a[599], "{name}");
    }
}

#[test]
fn cloned_universe_continues_identically() {
    let Some((_, mut a)) = run("fleet", 150) else { return };
    let mut b_universe = a.universe.clone();
    for _ in 0..200 {
        a.universe.update();
        b_universe.update();
        assert_eq!(hwc_sim::harness::world_hash(&a.universe), hwc_sim::harness::world_hash(&b_universe));
    }
}

/// Bit patterns of a few ships after N ticks of the built-in scenarios. These pin the current
/// implementation (regression goldens); they were not produced by the original game. (The
/// combat slice changed the fleet values: the Acolytes of the delta formation now bump.)
#[test]
fn golden_trajectories() {
    let Some((hashes, run)) = run("fleet", 400) else { return };
    let bits = |i: usize| {
        let s = run.universe.ship(run.ids[i]).unwrap();
        let p = s.posinfo.position;
        [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]
    };
    let got = [bits(1), bits(4), bits(7), bits(9)];
    println!("fleet 400: hash {:#018x} ships {got:x?}", hashes[399]);
    let fleet_hash = hashes[399];
    let fleet = got;

    let (hashes, run) = run_named("patrol", 800);
    let s = run.universe.ship(run.ids[7]).unwrap();
    let p = s.posinfo.position;
    let got = [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()];
    println!("patrol 800: hash {:#018x} frigate {got:x?}", hashes[799]);
    assert_eq!(fleet, GOLDEN_FLEET_400);
    assert_eq!(fleet_hash, GOLDEN_FLEET_HASH_400);
    assert_eq!(got, GOLDEN_PATROL_800);
}

fn run_named(name: &str, ticks: u32) -> (Vec<u64>, Run) {
    run(name, ticks).unwrap()
}

const GOLDEN_FLEET_400: [[u32; 3]; 4] = [
    [0x44f3f212, 0xc48c31b0, 0x431e8a67],
    [0x44dece52, 0xc471a3ae, 0x431e7d34],
    [0xc52d9442, 0xc542142a, 0xc3c4f850],
    [0xc521f9a3, 0xc4d40108, 0xc3e3cd02],
];
const GOLDEN_FLEET_HASH_400: u64 = 0x2e81_a5dc_157d_efff;
const GOLDEN_PATROL_800: [u32; 3] = [0x45488e56, 0x44e72b24, 0x431f1d85];

#[test]
fn print_fleet() {
    if std::env::var("HWC_PRINT").is_err() {
        return;
    }
    let Some(d) = data() else { return };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), scenario("fleet").unwrap()).unwrap();
    for t in 0..400 {
        run.step();
        if t % 40 == 0 {
            for &id in &run.ids {
                let s = run.universe.ship(id).unwrap();
                let p = s.posinfo.position;
                let v = s.posinfo.velocity;
                println!("t{t} {} {:?} pos ({:.1},{:.1},{:.1}) vel ({:.1},{:.1},{:.1}) cmd {:?} ais {} aic {}", id.0, s.ship_type, p.x, p.y, p.z, v.x, v.y, v.z, s.command, s.ai_state, s.ai_state_command);
            }
        }
    }
}
