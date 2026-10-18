//! Finding and validating the player's original install.
//!
//! Everything here only reads: files are opened read-only and nothing is ever created inside the
//! install directory. File names are matched case-insensitively so installs copied through
//! case-sensitive file systems (Wine prefixes, Linux mounts) are still recognised.

use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::hash;
use crate::ImportError;

/// The archive with every model, texture, script and sound effect.
pub const BIG: &str = "cataclysm.big";
/// The executable; only read to identify the version and to extract the missions.
pub const EXE: &str = "cataclysm.exe";
/// Streamed music data (its index, `soundfx\music4cat.wxh`, lives in the archive).
pub const MUSIC: &str = hwc_formats::datadir::MUSIC_FILE;
/// Beast speech fragments.
pub const BEAST_SPEECH: &str = hwc_formats::datadir::BEAST_SPEECH_FILE;
/// Somtaaw speech fragments.
pub const SECT_SPEECH: &str = hwc_formats::datadir::SECT_SPEECH_FILE;
/// Default key bindings shipped with the game.
pub const DEFAULT_KEYS: &str = "default_keys.cfg";
/// Player settings; written by the original on first run, so it may be absent.
pub const SETTINGS: &str = "Cataclysm.cfg";
/// Renderer device statistics shipped with the game (optional).
pub const DEVSTATS: &str = "devstats.dat";
/// Movie folder.
pub const MOVIES_DIR: &str = "Movies";
/// RAD's Bink library; only read, to extract the codec tables the movie decoder needs (optional:
/// without it the movies are unavailable).
pub const BINKW32: &str = "binkw32.dll";

/// The Bink movies of the retail release.
pub const MOVIES: [&str; 20] = [
    "Animatic01.bik",
    "Animatic02.bik",
    "Animatic03.bik",
    "Animatic04.bik",
    "Animatic05.bik",
    "Animatic06.bik",
    "Animatic07.bik",
    "Animatic08.bik",
    "Animatic09.bik",
    "Animatic10.bik",
    "Animatic11.bik",
    "Animatic12.bik",
    "Animatic13.bik",
    "Animatic14.bik",
    "Animatic15.bik",
    "Animatic16.bik",
    "Animatic17.bik",
    "Animatic18.bik",
    "bdogintro.bik",
    "sierra.bik",
];

/// An executable build this importer has been checked against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownExe {
    /// Stable identifier recorded in the manifest.
    pub id: &'static str,
    /// Human-readable description.
    pub description: &'static str,
    pub sha256: &'static str,
    pub size: u64,
    /// SHA-256 of each mission's encoded IR as extracted from this build, for checking the
    /// extraction (see [`crate::missions::check_reference`]).
    pub missions: Option<MissionDigests>,
}

/// Reference digests of the missions extracted from one executable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MissionDigests {
    /// The [`crate::missions::EXTRACTOR`] value the digests were taken with. The IR (and so its
    /// digest) can change with the extractor; a different extractor cannot be checked.
    pub extractor: &'static str,
    /// (mission name, SHA-256 of `missions/<name>.kasb`), in extraction order.
    pub missions: &'static [(&'static str, &'static str)],
}

/// Executables whose missions the extractor is validated against.
pub const KNOWN_EXES: &[KnownExe] = &[KnownExe {
    id: "retail-2000-08-25",
    description: "Homeworld: Cataclysm retail release (build of 25 August 2000)",
    sha256: "b4ba4b044fcdad04fbbe0445dcf806fca3f03bfaaaf86978eda29baaf5128da1",
    size: 5_359_747,
    missions: Some(MissionDigests {
        extractor: "hwc-kas-extract/0.1.0+ir1",
        missions: &[
            ("Mission01", "1bd6725114b4c6ede334e3570d0a5d6de75addbe253a85f3d84aef3393eb3ccf"),
            ("Mission02", "6bc4a464f5eeb83af66f04702fdf73c339325f63866f130d11ddea1b6faa4278"),
            ("Mission03", "deabf7d710b4cce8cd74e8b507f787771aa123c04ab749ff9c119cf15aa0e1c4"),
            ("Mission04", "f51d35cf727634649b93441e247e5198c9a3e6dc4919abc402de96b507ea6caa"),
            ("Mission05", "95af08bf4d6dcbd96511cf8deb1caf38cb9b9f65443a5ed2b6552bf7a9fdfab6"),
            ("Mission06", "49c9f2864943d8f2eecef5b8d43b52227707d46aea25a6bc21109e010d4e8850"),
            ("Mission07", "6e7cf454acb26be05fde92ddf519801a119dfa5b1d36f2331ae675e8c640b986"),
            ("Mission08", "9a5f5209d9b115de4e41202d755130fb4a2071cb57168e80116b6dd2ff73e85c"),
            ("Mission09", "179eeb85f2d015b0e3d07a60361d937310117fbfa0323c8ece826e4169b3b875"),
            ("Mission10", "566a84cc1b930fbd6ecbfeee6cfe3f70ed53297d27c7862811c82455320d9879"),
            ("Mission11", "88f1506cda2985e3aafbaf49d1edb6ef1d51c0558a9072d16acd67c8ac62af8d"),
            ("Mission12", "2d15f3373c5945ee8c073291bc7d9cc62896cf76227985cd9ef0345f52bf1222"),
            ("Mission13", "e7fba174e1e3f3418b3a4fb782da10fa7b500cb8d7322d3ec84c08e20b06a170"),
            ("Mission14", "5e429541915a45122fe0304b83edb6536bf7daef54e0a820e0f607fdf8783394"),
            ("Mission15", "85c36ee945a5338ddbd94ef27ed7335804ce9499f3ea72b0b4e3be6d5b9d1892"),
            ("Mission16", "2bb3da672d6a3bfd87c08052402432e16339d70a8a4f53e1e2c3bfa6bc5d5f08"),
            ("Mission17", "ebb29b9b0be5527c7a7b2991e7bbe9cbeeb72176068e36aff3babb025637cc98"),
            ("Tutorial1", "76f713dc6d2a3c59d92d62b5d74ceffd51915182a5a424a1122ab3e471de70bf"),
        ],
    }),
}];

/// Looks an executable hash up in [`KNOWN_EXES`].
pub fn identify_exe(sha256: &str) -> Option<&'static KnownExe> {
    KNOWN_EXES.iter().find(|k| k.sha256.eq_ignore_ascii_case(sha256))
}

/// The executable's identity.
#[derive(Debug, Clone)]
pub struct ExeInfo {
    pub size: u64,
    pub sha256: String,
    /// `None` for a build this importer does not know. Data import still works; mission
    /// extraction is attempted but marked unverified.
    pub known: Option<&'static KnownExe>,
}

/// One file the importer takes from the install.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceFile {
    /// Absolute path in the install.
    pub path: PathBuf,
    /// Path relative to the install root, `/`-separated, as found on disk.
    pub rel: String,
    /// Where it goes in the data directory, `/`-separated.
    pub dest: String,
    pub kind: FileKind,
}

pub use hwc_formats::manifest::FileKind;

/// A recognised install.
#[derive(Debug, Clone)]
pub struct Install {
    pub dir: PathBuf,
    pub exe: ExeInfo,
    /// The executable's path (not copied; used for identification and mission extraction).
    pub exe_path: PathBuf,
    /// Files to copy, in import order (archive first, movies last).
    pub files: Vec<SourceFile>,
    /// `binkw32.dll`, when present: the source of the Bink codec tables.
    pub binkw32: Option<SourceFile>,
    /// Non-fatal observations (unknown exe, extra movies, ...).
    pub warnings: Vec<String>,
}

impl Install {
    /// Total bytes of everything that would be copied.
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().filter_map(|f| f.path.metadata().ok()).map(|m| m.len()).sum()
    }
}

/// Finds `name` in `dir`, ignoring ASCII case. Returns the on-disk file name.
fn find_ci(dir: &Path, name: &str) -> Option<String> {
    let exact = dir.join(name);
    if exact.is_file() {
        // The OS may be case-insensitive; report the real on-disk spelling when we can.
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.eq_ignore_ascii_case(name) {
                    return Some(n);
                }
            }
        }
        return Some(name.to_owned());
    }
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.eq_ignore_ascii_case(name) && e.path().is_file()).then_some(n)
    })
}

fn find_dir_ci(dir: &Path, name: &str) -> Option<String> {
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let n = e.file_name().to_string_lossy().into_owned();
        (n.eq_ignore_ascii_case(name) && e.path().is_dir()).then_some(n)
    })
}

fn read_prefix(path: &Path, n: usize) -> std::io::Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(n);
    File::open(path)?.take(n as u64).read_to_end(&mut buf)?;
    Ok(buf)
}

/// Inspects `dir` and returns what it contains, or [`ImportError::NotAnInstall`] listing every
/// missing or malformed required file.
pub fn detect(dir: &Path) -> Result<Install, ImportError> {
    if !dir.is_dir() {
        return Err(ImportError::NotAnInstall { dir: dir.to_path_buf(), problems: vec!["not a directory".into()] });
    }
    let mut problems = Vec::new();
    let mut warnings = Vec::new();
    let mut files = Vec::new();

    // (canonical name, kind, destination folder, required, expected magic)
    let top: [(&str, FileKind, &str, bool, &[u8]); 7] = [
        (BIG, FileKind::Archive, "base", true, b"RBF1.23"),
        (MUSIC, FileKind::Music, "audio", true, b"WXD1"),
        (BEAST_SPEECH, FileKind::Speech, "audio", true, b"VCE0"),
        (SECT_SPEECH, FileKind::Speech, "audio", true, b"VCE0"),
        (DEFAULT_KEYS, FileKind::Config, "config", true, b""),
        (SETTINGS, FileKind::Config, "config", false, b""),
        (DEVSTATS, FileKind::Config, "config", false, b""),
    ];
    for (name, kind, folder, required, magic) in top {
        match find_ci(dir, name) {
            Some(on_disk) => {
                let path = dir.join(&on_disk);
                if !magic.is_empty() {
                    match read_prefix(&path, magic.len()) {
                        Ok(p) if p == magic => {}
                        Ok(_) => problems.push(format!("{on_disk}: unexpected contents (no {} signature)", String::from_utf8_lossy(magic))),
                        Err(e) => problems.push(format!("{on_disk}: {e}")),
                    }
                }
                files.push(SourceFile { path, rel: on_disk, dest: format!("{folder}/{name}"), kind });
            }
            None if required => problems.push(format!("{name} is missing")),
            None => {}
        }
    }
    if files.iter().any(|f| f.kind == FileKind::Archive) {
        let big = &files[0].path;
        if let Err(e) = hwc_formats::big::BigArchive::open(big) {
            problems.push(format!("{BIG}: {e}"));
        }
    }

    let exe_name = find_ci(dir, EXE);
    let exe_path = exe_name.as_ref().map(|n| dir.join(n));
    let mut exe = None;
    match &exe_path {
        None => problems.push(format!("{EXE} is missing")),
        Some(p) => match read_prefix(p, 2) {
            Ok(mz) if mz == b"MZ" => match hash::sha256_file(p, &mut |_| {}) {
                Ok(sha256) => {
                    let size = p.metadata().map(|m| m.len()).unwrap_or(0);
                    let known = identify_exe(&sha256);
                    if known.is_none() {
                        warnings.push(format!(
                            "{EXE} is an unrecognised build (sha256 {sha256}, {size} bytes): data will be imported, \
                             but mission extraction is unverified for this version"
                        ));
                    }
                    exe = Some(ExeInfo { size, sha256, known });
                }
                Err(e) => problems.push(format!("{EXE}: {e}")),
            },
            Ok(_) => problems.push(format!("{EXE}: not a Windows executable")),
            Err(e) => problems.push(format!("{EXE}: {e}")),
        },
    }

    match find_dir_ci(dir, MOVIES_DIR) {
        None => problems.push(format!("{MOVIES_DIR} folder is missing")),
        Some(mdir) => {
            let movie_dir = dir.join(&mdir);
            let mut found: Vec<(String, PathBuf)> = fs::read_dir(&movie_dir)
                .map(|rd| {
                    rd.flatten()
                        .filter(|e| e.path().is_file())
                        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
                        .filter(|(n, _)| n.to_ascii_lowercase().ends_with(".bik"))
                        .collect()
                })
                .unwrap_or_default();
            found.sort();
            for expected in MOVIES {
                if !found.iter().any(|(n, _)| n.eq_ignore_ascii_case(expected)) {
                    problems.push(format!("{MOVIES_DIR}/{expected} is missing"));
                }
            }
            for (on_disk, path) in found {
                let canonical = MOVIES.iter().find(|m| m.eq_ignore_ascii_case(&on_disk)).copied();
                if canonical.is_none() {
                    warnings.push(format!("{MOVIES_DIR}/{on_disk} is not a retail movie; copying it anyway"));
                }
                match read_prefix(&path, 3) {
                    Ok(p) if p == b"BIK" => {}
                    _ => problems.push(format!("{MOVIES_DIR}/{on_disk}: not a Bink file")),
                }
                files.push(SourceFile {
                    path,
                    rel: format!("{mdir}/{on_disk}"),
                    dest: format!("movies/{}", canonical.unwrap_or(&on_disk)),
                    kind: FileKind::Movie,
                });
            }
        }
    }

    let binkw32 = match find_ci(dir, BINKW32) {
        Some(on_disk) => Some(SourceFile {
            path: dir.join(&on_disk),
            rel: on_disk,
            dest: hwc_video::TABLES_FILE_NAME.to_owned(),
            kind: FileKind::CodecTables,
        }),
        None => {
            warnings.push(format!("{BINKW32} is missing: the Bink codec tables cannot be imported, so movies will be unavailable"));
            None
        }
    };

    match (problems.is_empty(), exe, exe_path) {
        (true, Some(exe), Some(exe_path)) => Ok(Install { dir: dir.to_path_buf(), exe, exe_path, files, binkw32, warnings }),
        _ => Err(ImportError::NotAnInstall { dir: dir.to_path_buf(), problems }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_exe_lookup() {
        let k = identify_exe("B4BA4B044FCDAD04FBBE0445DCF806FCA3F03BFAAAF86978EDA29BAAF5128DA1").unwrap();
        assert_eq!(k.id, "retail-2000-08-25");
        assert!(identify_exe("00").is_none());
    }

    /// A change to the extractor id means the IR may have changed: re-take the reference
    /// digests (import the known build and copy `missions[].sha256` from the manifest).
    #[test]
    fn reference_digests_match_current_extractor() {
        for k in KNOWN_EXES {
            let d = k.missions.expect("known build without reference digests");
            assert_eq!(d.extractor, crate::missions::EXTRACTOR, "{}: reference digests are stale", k.id);
            assert_eq!(d.missions.len(), 18);
        }
    }

    #[test]
    fn empty_dir_lists_every_required_file() {
        let dir = std::env::temp_dir().join(format!("hwc-import-empty-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let Err(ImportError::NotAnInstall { problems, .. }) = detect(&dir) else { panic!("expected NotAnInstall") };
        for name in [BIG, EXE, MUSIC, BEAST_SPEECH, SECT_SPEECH, DEFAULT_KEYS, MOVIES_DIR] {
            assert!(problems.iter().any(|p| p.contains(name)), "{name} not reported in {problems:?}");
        }
        assert!(!problems.iter().any(|p| p.contains(SETTINGS)), "optional file reported");
        fs::remove_dir_all(&dir).ok();
    }
}
