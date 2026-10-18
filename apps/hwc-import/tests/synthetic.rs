//! Import behaviour on a minimal fake install (no game data needed).

use std::fs;
use std::path::{Path, PathBuf};

use hwc_import::install::{DEFAULT_KEYS, MOVIES};
use hwc_import::{import, no_progress, DataDir, ImportError, ImportOptions, Progress};

fn scratch(name: &str) -> PathBuf {
    let base = option_env!("CARGO_TARGET_TMPDIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dir = base.join(format!("hwc-import-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// An archive with no entries is a valid `.big`.
fn fake_install(dir: &Path) {
    let mut big = b"RBF1.23".to_vec();
    big.extend_from_slice(&0i32.to_le_bytes());
    big.extend_from_slice(&1i32.to_le_bytes());
    fs::write(dir.join("Cataclysm.big"), big).unwrap();
    fs::write(dir.join("cataclysm.exe"), b"MZ this is not the real game").unwrap();
    fs::write(dir.join("music4cat.wxd"), b"WXD1\0\0\0\0").unwrap();
    fs::write(dir.join("BeastSpeech.vce"), b"VCE0\0\0\0\0").unwrap();
    fs::write(dir.join("SectSpeech.vce"), b"VCE0\0\0\0\0").unwrap();
    fs::write(dir.join(DEFAULT_KEYS), b"; keys\n").unwrap();
    fs::create_dir_all(dir.join("movies")).unwrap();
    for m in MOVIES {
        fs::write(dir.join("movies").join(m.to_ascii_lowercase()), b"BIKi movie").unwrap();
    }
}

#[test]
fn unknown_exe_imports_with_warning() {
    let root = scratch("synthetic");
    let (inst, out) = (root.join("install"), root.join("data"));
    fs::create_dir_all(&inst).unwrap();
    fake_install(&inst);
    // A DLL without the codec tables: the import warns and continues without movies.
    fs::write(inst.join("BinkW32.dll"), b"MZ not the real library").unwrap();

    let mut warnings = Vec::new();
    let report = import(&inst, &out, ImportOptions::default(), &mut |p| {
        if let Progress::Warning(w) = p {
            warnings.push(w.to_string());
        }
    })
    .unwrap();
    assert!(warnings.iter().any(|w| w.contains("unrecognised build")), "{warnings:?}");
    assert_eq!(report.manifest.exe.known_version, None);
    assert_eq!(report.manifest.missions.status, "failed");
    assert!(!report.manifest.missions.verified_exe);
    assert!(warnings.iter().any(|w| w.contains("Bink codec tables") && w.contains("movies will be unavailable")), "{warnings:?}");
    assert!(report.manifest.file(hwc_video::TABLES_FILE_NAME).is_none());
    assert!(!out.join(hwc_video::TABLES_FILE_NAME).exists());
    let v = hwc_import::verify(&out, hwc_import::VerifyOptions::default(), &mut no_progress).unwrap();
    assert!(v.hash_failures.is_empty(), "{:?}", v.hash_failures);
    assert!(v.warnings.iter().any(|w| w.contains("movies are unavailable")), "{:?}", v.warnings);
    // The shared reader sees the same data directory.
    let gd = hwc_formats::GameData::open(&out).unwrap();
    assert_eq!(gd.layout(), hwc_formats::gamedata::Layout::Imported);
    assert!(gd.bink_tables().is_none());
    assert!(gd.movie_path("ANIMATIC01.BIK").is_some());
    // Canonical output names regardless of the install's spelling.
    assert!(out.join("base/cataclysm.big").is_file());
    assert!(out.join("audio/Music4Cat.wxd").is_file());
    assert!(out.join("movies/Animatic01.bik").is_file());
    assert!(out.join("overrides").is_dir());
    assert_eq!(report.copied.len(), 5 + MOVIES.len());

    // Overrides shadow the archive (which is empty here).
    fs::create_dir_all(out.join("overrides/Race_Sect")).unwrap();
    fs::write(out.join("overrides/Race_Sect/Test.SHP"), b"x 1\n").unwrap();
    let data = DataDir::open(&out).unwrap();
    let (bytes, origin) = data.read("race_sect\\test.shp").unwrap().unwrap();
    assert_eq!((bytes.as_slice(), origin), (&b"x 1\n"[..], hwc_import::datadir::Origin::Override));
    assert!(data.read("race_sect\\other.shp").unwrap().is_none());
    assert!(data.music_path().unwrap().ends_with("Music4Cat.wxd"));

    // Removing a movie from the install makes it invalid; nothing is half-imported.
    fs::remove_file(inst.join("movies/sierra.bik")).unwrap();
    let Err(ImportError::NotAnInstall { problems, .. }) = import(&inst, &out, ImportOptions::default(), &mut no_progress) else {
        panic!("expected NotAnInstall");
    };
    assert_eq!(problems, vec!["Movies/sierra.bik is missing".to_string()]);
    fs::remove_dir_all(&root).ok();
}

#[test]
fn refuses_unsafe_outputs() {
    let root = scratch("unsafe");
    let inst = root.join("install");
    fs::create_dir_all(&inst).unwrap();
    fake_install(&inst);

    let err = import(&inst, &inst.join("data"), ImportOptions::default(), &mut no_progress).unwrap_err();
    assert!(matches!(err, ImportError::UnsafeOutput { .. }), "{err}");
    assert!(!inst.join("data").exists());

    let occupied = root.join("occupied");
    fs::create_dir_all(&occupied).unwrap();
    fs::write(occupied.join("precious.txt"), b"keep").unwrap();
    let err = import(&inst, &occupied, ImportOptions::default(), &mut no_progress).unwrap_err();
    assert!(matches!(err, ImportError::OutputNotEmpty(_)), "{err}");
    assert!(!occupied.join("base").exists());
    // --force allows it and leaves unrelated files alone.
    import(&inst, &occupied, ImportOptions { force: true }, &mut no_progress).unwrap();
    assert_eq!(fs::read(occupied.join("precious.txt")).unwrap(), b"keep");
    fs::remove_dir_all(&root).ok();
}
