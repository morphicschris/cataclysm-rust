//! Every effect in the game data compiles, and the effect scripts bind them.

mod common;

use hwc_fx::bytecode::{self, Instr};
use hwc_fx::compiler::Library;
use hwc_fx::load_scripts;

/// Files the original compiler cannot load either: `BulletBigGunGlow.ebg` uses an undeclared
/// symbol (`bgglow_m`), which would be a null dereference in `etgFunctionCallScan`. Its
/// `LoadEffect` line is commented out in `General.script`, so the game never loads it.
const KNOWN_BROKEN: &[&str] = &["bulletbiggunglow.ebg"];

#[test]
fn every_effect_file_compiles() {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let files = common::effect_files(&data);
    assert!(files.len() > 200, "found only {} effect files", files.len());
    let mut lib = Library::new();
    let mut failures = Vec::new();
    for f in &files {
        let known_broken = KNOWN_BROKEN.iter().any(|b| f.to_ascii_lowercase().ends_with(b));
        match lib.load_effect(&data, f) {
            Err(e) if !known_broken => failures.push(e.to_string()),
            Ok(_) if known_broken => failures.push(format!("{f} was expected to fail")),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{} of {} failed:\n{}", failures.len(), files.len(), failures.join("\n"));
    // Every code block decodes cleanly and ends in End (or is empty).
    let mut instrs = 0;
    let mut unterminated = 0;
    for s in lib.statics.iter().filter(|s| s.is_loaded()) {
        for block in &s.code {
            let d = bytecode::disassemble(block);
            instrs += d.len();
            assert!(d.iter().all(|(_, i)| !matches!(i, Instr::Unknown(_))), "{}", s.name);
            // Scripts with an unclosed block (e.g. SWHitProjectile.ebg) have no final End.
            if matches!(d.last(), Some((_, i)) if *i != Instr::End) {
                unterminated += 1;
            }
        }
    }
    eprintln!("compiled {} effects ({} instructions, {} textures, {} meshes, {} warnings, {} blocks without a final End)",
        files.len(), instrs, lib.textures.len(), lib.meshes.len(), lib.warnings.len(), unterminated);
    for w in lib.warnings.iter().take(20) {
        eprintln!("warning: {w}");
    }
}

#[test]
fn effect_scripts_load() {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let mut lib = Library::new();
    let load = load_scripts(&mut lib, &data);
    assert!(load.errors.is_empty(), "{:?}", load.errors);
    let t = &load.tables;
    let bound = t.all_effects();
    eprintln!("scripts bound {} effects; {} statics loaded; {} warnings", bound.len(), lib.statics.len(), t.warnings.len());
    for w in &t.warnings {
        eprintln!("warning: {w}");
    }
    assert!(bound.len() > 100);
    // Beast fighter damage death and a gun muzzle flash exist.
    assert!(t.death_event(1, 6, 0).is_some());
    assert!(t.gun_event(2, 13, hwc_fx::scripts::GUN_FIRE).is_some());
    assert_eq!(t.software_scalar_fire, 0.866);
    // Every effect a script binds is loaded.
    for i in bound {
        assert!(lib.statics[i].is_loaded(), "{}", lib.statics[i].name);
    }
}
