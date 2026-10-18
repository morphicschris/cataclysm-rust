//! End-to-end import of the player's install. Skips when `HWC_GAME_DIR` is unset.
//!
//! Output goes to `HWC_IMPORT_TEST_DIR` if set, otherwise under Cargo's per-target temp dir.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use hwc_import::{hash, import, no_progress, verify, DataDir, ImportOptions, Manifest, VerifyOptions};

/// (size, mtime, sha256) of every file under `dir`.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, (u64, SystemTime, String)> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            let meta = e.metadata().unwrap();
            if meta.is_dir() {
                stack.push(p);
            } else {
                let h = hash::sha256_file(&p, &mut |_| {}).unwrap();
                out.insert(p, (meta.len(), meta.modified().unwrap(), h));
            }
        }
    }
    out
}

fn out_dir() -> PathBuf {
    let base = std::env::var_os("HWC_IMPORT_TEST_DIR")
        .map(PathBuf::from)
        .or_else(|| option_env!("CARGO_TARGET_TMPDIR").map(PathBuf::from))
        .unwrap_or_else(std::env::temp_dir);
    base.join("hwc-import-real")
}

#[test]
fn import_verify_idempotent_and_install_untouched() {
    let Some(game) = hwc_formats::game_dir() else {
        eprintln!("HWC_GAME_DIR not set; skipping");
        return;
    };
    let out = out_dir();
    let _ = fs::remove_dir_all(&out);
    let before = snapshot(&game);

    // First import.
    let first = import(&game, &out, ImportOptions::default(), &mut no_progress).unwrap();
    eprintln!(
        "first import: {} files, {:.1} MB copied, {:.1} MB total, {:.2}s",
        first.copied.len(),
        first.bytes_copied as f64 / 1e6,
        first.bytes_total as f64 / 1e6,
        first.elapsed.as_secs_f64()
    );
    let m = &first.manifest;
    assert_eq!(m.manifest_version, hwc_import::manifest::MANIFEST_VERSION);
    assert_eq!(m.exe.known_version.as_deref(), Some("retail-2000-08-25"));
    assert!(first.skipped.is_empty());
    assert_eq!(first.copied.len(), m.files.len());
    for path in ["base/cataclysm.big", "audio/Music4Cat.wxd", "audio/BeastSpeech.vce", "audio/SectSpeech.vce", "config/default_keys.cfg"] {
        assert!(m.file(path).is_some(), "{path} missing from manifest");
    }
    assert_eq!(m.files.iter().filter(|f| f.path.starts_with("movies/")).count(), 20);
    for f in &m.files {
        assert_eq!(f.sha256, f.source.sha256, "{}", f.path);
        assert_eq!(f.size, f.source.size, "{}", f.path);
    }
    assert_eq!(Manifest::load(&out).unwrap().as_ref(), Some(m));
    assert!(m.missions.missions.len() >= 18, "{:?}", m.missions);

    // Verify.
    let v = verify(&out, VerifyOptions::default(), &mut no_progress).unwrap();
    eprintln!("verify: {} hashed, {} assets parsed, {:.2}s", v.files_hashed, v.assets_checked(), v.elapsed.as_secs_f64());
    assert!(v.hash_failures.is_empty(), "{:?}", v.hash_failures);
    assert!(v.parse_failures.is_empty(), "{:?}", &v.parse_failures[..v.parse_failures.len().min(20)]);
    assert!(v.assets_checked() > 16_000);
    for kind in ["lif", "geo", "peo", "mex", "mad", "btg", "hff", "fib", "hsf", "nis", "script", "bnk", "lut", "wxh", "wxd", "vce"] {
        assert!(v.by_kind.get(kind).is_some_and(|c| c.ok > 0), "no {kind} checked");
    }

    // Second import copies nothing and leaves the manifest byte-identical.
    let manifest_bytes = fs::read(out.join("manifest.json")).unwrap();
    let second = import(&game, &out, ImportOptions::default(), &mut no_progress).unwrap();
    assert!(second.copied.is_empty(), "{:?}", second.copied);
    assert!(second.removed.is_empty());
    assert!(!second.missions_written);
    assert_eq!(second.bytes_copied, 0);
    assert_eq!(fs::read(out.join("manifest.json")).unwrap(), manifest_bytes);

    // A damaged output is caught by verify and repaired by re-import (only that file).
    let keys = out.join("config").join("default_keys.cfg");
    let mut damaged = fs::read(&keys).unwrap();
    damaged.push(b'!');
    fs::write(&keys, damaged).unwrap();
    let v = verify(&out, VerifyOptions { assets: false, ..Default::default() }, &mut no_progress).unwrap();
    assert_eq!(v.hash_failures.len(), 1, "{:?}", v.hash_failures);
    let third = import(&game, &out, ImportOptions::default(), &mut no_progress).unwrap();
    assert_eq!(third.copied, vec!["config/default_keys.cfg".to_string()]);
    assert!(verify(&out, VerifyOptions { assets: false, ..Default::default() }, &mut no_progress).unwrap().ok());

    // Overrides take precedence and are parse-checked.
    let data = DataDir::open(&out).unwrap();
    let name = "soundevent.script";
    let (orig, origin) = data.read(name).unwrap().unwrap();
    assert_eq!(origin, hwc_import::datadir::Origin::Archive);
    fs::write(out.join("overrides").join("SoundEvent.script"), b"; modded\n").unwrap();
    fs::write(out.join("overrides").join("broken.lif"), b"not a texture").unwrap();
    let data = DataDir::open(&out).unwrap();
    let (modded, origin) = data.read(name).unwrap().unwrap();
    assert_eq!((modded.as_slice(), origin), (&b"; modded\n"[..], hwc_import::datadir::Origin::Override));
    assert_ne!(orig, modded);
    let v = verify(&out, VerifyOptions { hashes: false, ..Default::default() }, &mut no_progress).unwrap();
    assert_eq!(v.overrides, 2);
    assert_eq!(v.parse_failures.len(), 1, "{:?}", v.parse_failures);
    assert!(v.parse_failures[0].0.contains("broken.lif"));
    fs::remove_dir_all(out.join("overrides")).unwrap();

    // The install was only read.
    assert_eq!(snapshot(&game), before, "the original install changed");
}

#[test]
fn missions_are_extracted() {
    let Some(game) = hwc_formats::game_dir() else {
        eprintln!("HWC_GAME_DIR not set; skipping");
        return;
    };
    let exe = fs::read(game.join("cataclysm.exe")).unwrap();
    let e = hwc_import::missions::extract(&exe);
    assert!(e.errors.is_empty(), "{:?}", e.errors);
    assert_eq!(e.missions.len(), 18);
    for (name, ir) in &e.missions {
        let m = hwc_kas::binary::decode(ir.as_ref().unwrap()).unwrap();
        assert_eq!(&m.name, name);
    }
}
