//! Runs effects headless: every effect survives a few seconds of updates, and a selection
//! (explosions, muzzle flashes, impacts, trails, hyperspace, Beast effects) is deterministic.

mod common;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use hwc_fx::compiler::Library;
use hwc_fx::render::RenderList;
use hwc_fx::{load_scripts, FxWorld};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Vector};

fn library(data: &common::Data) -> Library {
    let mut lib = Library::new();
    let load = load_scripts(&mut lib, data);
    assert!(load.errors.is_empty());
    for f in common::effect_files(data) {
        let _ = lib.load_effect(data, &f);
    }
    lib
}

fn hash_frame(h: &mut DefaultHasher, world: &FxWorld, list: &RenderList) {
    world.effects.len().hash(h);
    for s in world.systems.iter().flatten() {
        for p in &s.particles {
            p.position.x.to_bits().hash(h);
            p.position.y.to_bits().hash(h);
            p.position.z.to_bits().hash(h);
            p.lifespan.to_bits().hash(h);
            for c in p.icolor {
                c.to_bits().hash(h);
            }
            p.scale_x.to_bits().hash(h);
        }
    }
    for s in &list.sprites {
        s.position.x.to_bits().hash(h);
        s.size[0].to_bits().hash(h);
    }
    for b in &list.bolts {
        for p in &b.points {
            p.x.to_bits().hash(h);
        }
    }
    list.len().hash(h);
}

/// Run `name` for `ticks` universe updates; returns (hash, max particles, primitives drawn).
fn run(lib: &Library, name: &str, ticks: usize) -> (u64, usize, usize) {
    run_with_warnings(lib, name, ticks).0
}

fn run_with_warnings(lib: &Library, name: &str, ticks: usize) -> ((u64, usize, usize), Vec<String>) {
    let mut world = FxWorld::new(lib.clone());
    world.spawn_named(name, Vector::new(0.0, 0.0, 0.0), Matrix::IDENTITY, &[]).expect(name);
    let mut h = DefaultHasher::new();
    let mut max_particles = 0;
    let mut drawn = 0;
    for _ in 0..ticks {
        let mut list = RenderList::default();
        world.step(UNIVERSE_UPDATE_PERIOD, Some(&mut list));
        max_particles = max_particles.max(world.particle_count());
        drawn += list.len();
        hash_frame(&mut h, &world, &list);
    }
    ((h.finish(), max_particles, drawn), world.warnings)
}

#[test]
fn every_effect_runs() {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let lib = library(&data);
    let mut silent = Vec::new();
    let mut warnings = std::collections::BTreeSet::new();
    let mut n = 0;
    for s in lib.statics.iter().filter(|s| s.is_loaded()) {
        let ((_, maxp, drawn), w) = run_with_warnings(&lib, &s.name, 48);
        for w in w {
            warnings.insert(format!("{}: {w}", s.name));
        }
        n += 1;
        if maxp == 0 && drawn == 0 {
            silent.push(s.name.clone());
        }
    }
    eprintln!("ran {n} effects; {} produced no particles on their own: {:?}", silent.len(), silent);
    for w in &warnings {
        eprintln!("runtime warning: {w}");
    }
    assert!(n > 250);
}

#[test]
fn selected_effects_are_deterministic() {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let lib = library(&data);
    let selection = [
        "DDSfireball.ebg",       // explosion fireball
        "DestDamageSmall.ebg",   // small ship death
        "bigdeath.ebg",          // mothership-class death
        "FireEnergySmall01.ebg", // muzzle flash
        "HitEnergy.ebg",         // bullet impact
        "afterburner.ebg",       // engine trail
        "BulletMissiles.ebg",    // missile trail
        "HitBeast.ebg",          // Beast impact
        "eboltCascade.ebg",      // Beast electric bolts
        "PostBoomBEAST.ebg",     // Beast explosion
    ];
    let mut produced = 0;
    for name in selection {
        if lib.get(name).is_none() {
            eprintln!("{name}: not in this data set");
            continue;
        }
        let a = run(&lib, name, 96);
        let b = run(&lib, name, 96);
        assert_eq!(a, b, "{name} is not deterministic");
        eprintln!("{name}: max {} particles, {} primitives over 6 s", a.1, a.2);
        if a.2 > 0 {
            produced += 1;
        }
    }
    assert!(produced >= 6, "only {produced} of the selection drew anything");
}

#[test]
fn hyperspace_effect_runs() {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let mut lib = Library::new();
    let load = load_scripts(&mut lib, &data);
    let Some(levels) = load.tables.hyperspace.clone() else {
        eprintln!("no hyperspace effect bound");
        return;
    };
    let name = lib.statics[levels[0]].name.clone();
    let a = run(&lib, &name, 64);
    let b = run(&lib, &name, 64);
    assert_eq!(a, b);
    eprintln!("{name}: max {} particles, {} primitives", a.1, a.2);
}
