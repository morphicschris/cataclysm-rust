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
pub const MUSIC: &str = "Music4Cat.wxd";
/// Beast speech fragments.
pub const BEAST_SPEECH: &str = "BeastSpeech.vce";
/// Somtaaw speech fragments.
pub const SECT_SPEECH: &str = "SectSpeech.vce";
/// Default key bindings shipped with the game.
pub const DEFAULT_KEYS: &str = "default_keys.cfg";
/// Player settings; written by the original on first run, so it may be absent.
pub const SETTINGS: &str = "Cataclysm.cfg";
/// Renderer device statistics shipped with the game (optional).
pub const DEVSTATS: &str = "devstats.dat";
/// Movie folder.
pub const MOVIES_DIR: &str = "Movies";

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
}

/// Executables whose missions the extractor is validated against.
pub const KNOWN_EXES: &[KnownExe] = &[KnownExe {
    id: "retail-2000-08-25",
    description: "Homeworld: Cataclysm retail release (build of 25 August 2000)",
    sha256: "b4ba4b044fcdad04fbbe0445dcf806fca3f03bfaaaf86978eda29baaf5128da1",
    size: 5_359_747,
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

/// What a copied file is, recorded in the manifest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Archive,
    Music,
    Speech,
    Movie,
    Config,
}

/// A recognised install.
#[derive(Debug, Clone)]
pub struct Install {
    pub dir: PathBuf,
    pub exe: ExeInfo,
    /// The executable's path (not copied; used for identification and mission extraction).
    pub exe_path: PathBuf,
    /// Files to copy, in import order (archive first, movies last).
    pub files: Vec<SourceFile>,
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

    match (problems.is_empty(), exe, exe_path) {
        (true, Some(exe), Some(exe_path)) => Ok(Install { dir: dir.to_path_buf(), exe, exe_path, files, warnings }),
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
