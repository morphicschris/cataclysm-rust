//! End-to-end import of the player's install. Skips when `HWC_GAME_DIR` is unset.
//!
//! Output goes to `HWC_IMPORT_TEST_DIR` if set, otherwise under Cargo's per-target temp dir.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use hwc_formats::gamedata::{BinkTablesSource, Layout};
use hwc_formats::GameData;
use hwc_import::install::FileKind;
use hwc_import::{hash, import, no_progress, verify, DataDir, ImportOptions, Manifest, Progress, VerifyOptions};

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
    let mut warnings = Vec::new();
    let first = import(&game, &out, ImportOptions::default(), &mut |p| {
        if let Progress::Warning(w) = p {
            warnings.push(w.to_string());
        }
    })
    .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert!(first.warnings.is_empty(), "{:?}", first.warnings);
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
    // The Bink tables are derived from binkw32.dll, not copied.
    let bink = m.file(hwc_video::TABLES_FILE_NAME).expect("bink tables missing from manifest");
    assert_eq!(bink.kind, FileKind::CodecTables);
    assert!(bink.source.path.eq_ignore_ascii_case("binkw32.dll"));
    assert_eq!(bink.sha256, hash::sha256_file(&out.join(hwc_video::TABLES_FILE_NAME), &mut |_| {}).unwrap());
    let tables = hwc_video::BinkTables::load(out.join(hwc_video::TABLES_FILE_NAME)).unwrap();
    let from_dll = hwc_video::BinkTables::from_install(&game).unwrap();
    assert_eq!(tables.serialize(), from_dll.serialize());
    // Missions match the reference digests of the known build.
    assert_eq!(m.missions.status, "extracted");
    assert!(m.missions.verified_exe);
    assert_eq!(m.missions.missions.len(), 18);
    for f in m.files.iter().filter(|f| f.kind != FileKind::CodecTables) {
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
    assert!(v.warnings.is_empty(), "{:?}", v.warnings);
    assert!(v.assets_checked() > 16_000);
    for kind in ["lif", "geo", "peo", "mex", "mad", "btg", "hff", "fib", "hsf", "nis", "script", "bnk", "lut", "wxh", "wxd", "vce", "kasb", "bink"] {
        assert!(v.by_kind.get(kind).is_some_and(|c| c.ok > 0), "no {kind} checked");
    }

    // Second import copies nothing and leaves the manifest byte-identical.
    let manifest_bytes = fs::read(out.join("manifest.json")).unwrap();
    let second = import(&game, &out, ImportOptions::default(), &mut no_progress).unwrap();
    assert!(second.copied.is_empty(), "{:?}", second.copied);
    assert!(second.removed.is_empty());
    assert!(!second.missions_written);
    assert_eq!(second.bytes_copied, 0);
    assert!(second.skipped.iter().any(|p| p == hwc_video::TABLES_FILE_NAME));
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
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

    // Damaged Bink tables are caught by verify (hash and load) and re-extracted alone.
    let tables_path = out.join(hwc_video::TABLES_FILE_NAME);
    let mut damaged = fs::read(&tables_path).unwrap();
    damaged.truncate(damaged.len() / 2);
    fs::write(&tables_path, damaged).unwrap();
    let v = verify(&out, VerifyOptions::default(), &mut no_progress).unwrap();
    assert_eq!(v.hash_failures.len(), 1, "{:?}", v.hash_failures);
    assert_eq!(v.parse_failures.len(), 1, "{:?}", v.parse_failures);
    let fourth = import(&game, &out, ImportOptions::default(), &mut no_progress).unwrap();
    assert_eq!(fourth.copied, vec![hwc_video::TABLES_FILE_NAME.to_string()]);
    assert_eq!(fourth.manifest.file(hwc_video::TABLES_FILE_NAME).unwrap().sha256, bink.sha256);
    assert!(verify(&out, VerifyOptions { assets: false, ..Default::default() }, &mut no_progress).unwrap().ok());

    // The shared reader opens the data directory, the install and the bare archive.
    let gd = GameData::open(&out).unwrap();
    assert_eq!(gd.layout(), Layout::Imported);
    assert_eq!(gd.bink_tables(), Some(BinkTablesSource::Imported(tables_path.clone())));
    assert!(gd.music_path().unwrap().starts_with(&out));
    assert!(gd.beast_speech_path().is_some() && gd.sect_speech_path().is_some());
    assert_eq!(gd.movies_dir(), Some(out.join("movies")));
    assert_eq!(gd.missions_dir(), Some(out.join("missions")));
    assert!(gd.mission_ir("mission01").unwrap().is_some());
    let inst = GameData::open(&game).unwrap();
    assert_eq!(inst.layout(), Layout::Install);
    assert!(matches!(inst.bink_tables(), Some(BinkTablesSource::Dll(_))));
    assert!(inst.exe_path().is_some() && inst.movie_path("animatic01.bik").is_some());
    assert_eq!(inst.read("soundevent.script").unwrap(), gd.read("soundevent.script").unwrap());
    let bare = GameData::open(game.join("cataclysm.big")).unwrap();
    assert_eq!(bare.layout(), Layout::Archive);
    assert_eq!(bare.archive().entries().len(), gd.archive().entries().len());

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
