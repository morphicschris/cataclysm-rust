//! `manifest.json` of an imported data directory: what was imported, from where, and the hashes
//! needed for incremental re-import and verification. Written by `hwc-import`, read by the
//! engine through [`crate::datadir::DataDir`] / [`crate::gamedata::GameData`]. See
//! `docs/import.md` for the format.

use std::fs;
use std::io;
use std::path::Path;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

/// What a file in the data directory is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// The base `.big` archive.
    Archive,
    /// Streamed music data.
    Music,
    /// A speech bank.
    Speech,
    /// A Bink movie.
    Movie,
    /// Key bindings and settings.
    Config,
    /// Codec tables derived from a library of the install (`bink_tables.bin` from `binkw32.dll`).
    CodecTables,
}

/// File name of the manifest inside the data directory.
pub const MANIFEST_FILE: &str = "manifest.json";
/// Value of [`Manifest::format`].
pub const FORMAT: &str = "hwc-data";
/// Bumped on any incompatible change to the layout or the manifest itself. A data directory with
/// a different version is re-imported from scratch.
pub const MANIFEST_VERSION: u32 = 1;

/// Modification time as seconds and nanoseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mtime {
    pub secs: i64,
    pub nanos: u32,
}

impl Mtime {
    pub fn of(meta: &fs::Metadata) -> Option<Mtime> {
        let t = meta.modified().ok()?;
        Some(match t.duration_since(SystemTime::UNIX_EPOCH) {
            Ok(d) => Mtime { secs: d.as_secs() as i64, nanos: d.subsec_nanos() },
            Err(e) => {
                let d = e.duration();
                Mtime { secs: -(d.as_secs() as i64), nanos: d.subsec_nanos() }
            }
        })
    }
}

/// Where an output came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRecord {
    /// Path relative to the install root, `/`-separated.
    pub path: String,
    pub size: u64,
    pub mtime: Option<Mtime>,
    pub sha256: String,
}

/// One file in the data directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileRecord {
    /// Path relative to the data directory, `/`-separated.
    pub path: String,
    pub kind: FileKind,
    pub size: u64,
    pub sha256: String,
    /// Output mtime when written; lets re-import skip unchanged files without rehashing.
    pub mtime: Option<Mtime>,
    pub source: SourceRecord,
}

/// The source executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExeRecord {
    pub path: String,
    pub size: u64,
    pub sha256: String,
    /// Identifier from the importer's table of known builds, `None` if unrecognised.
    pub known_version: Option<String>,
}

/// One extracted mission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionRecord {
    /// Script name, e.g. `mission01`.
    pub name: String,
    /// `missions/<name>.kasb`, absent while the extractor is unavailable.
    pub path: Option<String>,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

/// Mission extraction state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MissionsRecord {
    /// `extracted` (every mission lowered), `partial` (some failed; see `errors`) or `failed`.
    pub status: String,
    /// Extractor identifier and version; a change triggers re-extraction.
    pub extractor: String,
    /// The exe the missions were extracted from.
    pub exe_sha256: String,
    /// False when the exe is not a known build: the IR was not checked against reference digests.
    pub verified_exe: bool,
    pub missions: Vec<MissionRecord>,
    /// Errors from the last extraction attempt.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub errors: Vec<String>,
}

/// The whole manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: String,
    pub manifest_version: u32,
    /// `hwc-import` crate version that wrote it.
    pub importer_version: String,
    /// Unix seconds of the last import that changed anything.
    pub updated_unix: i64,
    /// Install directory imported from (informational; the data directory does not depend on it).
    pub install_dir: String,
    pub exe: ExeRecord,
    /// Every copied file, sorted by path.
    pub files: Vec<FileRecord>,
    pub missions: MissionsRecord,
    /// Folder of loose files that take precedence over the archive (always `overrides`).
    pub overrides_dir: String,
}

impl Manifest {
    pub fn file(&self, path: &str) -> Option<&FileRecord> {
        self.files.iter().find(|f| f.path == path)
    }

    /// Reads `<dir>/manifest.json`; `Ok(None)` when there is none.
    pub fn load(dir: &Path) -> io::Result<Option<Manifest>> {
        let path = dir.join(MANIFEST_FILE);
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        let m: Manifest = serde_json::from_str(&text)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", path.display())))?;
        if m.format != FORMAT {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("{}: not an hwc-data manifest", path.display())));
        }
        Ok(Some(m))
    }

    /// Writes the manifest atomically (temporary file, then rename).
    pub fn save(&self, dir: &Path) -> io::Result<()> {
        let text = serde_json::to_string_pretty(self).map_err(io::Error::other)?;
        let tmp = dir.join(format!("{MANIFEST_FILE}.partial"));
        fs::write(&tmp, text + "\n")?;
        fs::rename(&tmp, dir.join(MANIFEST_FILE))
    }
}

/// Seconds since the Unix epoch now.
pub fn now_unix() -> i64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
