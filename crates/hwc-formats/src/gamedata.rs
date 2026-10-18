//! One entry point to the game's files, whatever form they are in.
//!
//! [`GameData::open`] accepts:
//!
//! * an **imported data directory** (has `manifest.json`; written by `hwc-import`): the layout the
//!   engine uses in normal play, with `overrides\` layered over `base\cataclysm.big`;
//! * for development, an **original install directory** (has `cataclysm.big`), read in place;
//! * or a **bare `.big` file**, which gives only the archive namespace.
//!
//! The archive namespace (`soundfx\guns.bnk`, `race_sect\...\x.geo`) is read with
//! [`GameData::read`]; the files outside it (music and speech banks, movies, missions, Bink
//! tables) are located with the path accessors, which return `None` when that layout has no such
//! file.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::big::{normalize_name, BigArchive};
use crate::datadir::{
    self, read_layered, DataDir, Origin, BEAST_SPEECH_FILE, BINK_TABLES_FILE, MISSIONS_DIR, MOVIES_DIR, MUSIC_FILE,
    SECT_SPEECH_FILE,
};
use crate::manifest::{Manifest, MANIFEST_FILE};

/// File name of the archive in an install.
pub const INSTALL_BIG: &str = "cataclysm.big";
/// File name of the executable in an install.
pub const INSTALL_EXE: &str = "cataclysm.exe";
/// File name of RAD's Bink library in an install (the source of the Bink codec tables).
pub const INSTALL_BINKW32: &str = "binkw32.dll";
/// Movie folder of an install.
pub const INSTALL_MOVIES_DIR: &str = "Movies";

/// Environment variable naming an imported data directory.
pub const ENV_DATA_DIR: &str = "HWC_DATA_DIR";
/// Environment variable naming an original install (see [`crate::game_dir`]).
pub const ENV_GAME_DIR: &str = "HWC_GAME_DIR";

/// Which kind of location a [`GameData`] was opened from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// An `hwc-import` data directory.
    Imported,
    /// The original install, read in place (development).
    Install,
    /// A lone `.big` archive (development; archive namespace only).
    Archive,
}

/// Where the Bink codec tables can be loaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BinkTablesSource {
    /// The importer's tables file; load with `hwc_video::BinkTables::load`.
    Imported(PathBuf),
    /// The install's `binkw32.dll`; load with `hwc_video::BinkTables::from_binkw32` on its bytes.
    Dll(PathBuf),
}

#[derive(Debug)]
enum Inner {
    Imported(Box<DataDir>),
    Install { root: PathBuf, archive: BigArchive },
    Archive { archive: BigArchive },
}

/// The game's files, opened from an imported data directory, an install or a bare archive.
#[derive(Debug)]
pub struct GameData {
    inner: Inner,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

fn open_big(path: &Path) -> io::Result<BigArchive> {
    BigArchive::open(path).map_err(|e| invalid(format!("{}: {e}", path.display())))
}

/// Finds a file or folder named `name` (ASCII case ignored) directly inside `dir`.
pub fn find_ci(dir: &Path, name: &str, want_dir: bool) -> Option<PathBuf> {
    let exact = dir.join(name);
    let rd = fs::read_dir(dir).ok()?;
    let found = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
        .find(|p| if want_dir { p.is_dir() } else { p.is_file() });
    found.or_else(|| {
        let ok = if want_dir { exact.is_dir() } else { exact.is_file() };
        ok.then_some(exact)
    })
}

static NO_OVERRIDES: std::sync::OnceLock<HashMap<String, PathBuf>> = std::sync::OnceLock::new();

impl GameData {
    /// Opens a data directory (with `manifest.json`), an install directory (with
    /// `cataclysm.big`) or a `.big` file.
    pub fn open(path: impl AsRef<Path>) -> io::Result<GameData> {
        let path = path.as_ref();
        if path.is_file() {
            return Ok(GameData { inner: Inner::Archive { archive: open_big(path)? } });
        }
        if !path.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("{}: no such file or directory", path.display())));
        }
        if path.join(MANIFEST_FILE).is_file() {
            return Ok(GameData { inner: Inner::Imported(Box::new(DataDir::open(path)?)) });
        }
        match find_ci(path, INSTALL_BIG, false) {
            Some(big) => Ok(GameData { inner: Inner::Install { root: path.to_path_buf(), archive: open_big(&big)? } }),
            None => Err(invalid(format!(
                "{}: neither an imported data directory ({MANIFEST_FILE}) nor a game install ({INSTALL_BIG})",
                path.display()
            ))),
        }
    }

    /// From `HWC_DATA_DIR` (an imported data directory), else `HWC_GAME_DIR` (an install).
    /// `None` when neither is set.
    pub fn from_env() -> Option<io::Result<GameData>> {
        [ENV_DATA_DIR, ENV_GAME_DIR].iter().find_map(std::env::var_os).map(GameData::open)
    }

    /// Wraps an already opened data directory.
    pub fn from_data_dir(dir: DataDir) -> GameData {
        GameData { inner: Inner::Imported(Box::new(dir)) }
    }

    pub fn layout(&self) -> Layout {
        match self.inner {
            Inner::Imported(_) => Layout::Imported,
            Inner::Install { .. } => Layout::Install,
            Inner::Archive { .. } => Layout::Archive,
        }
    }

    /// The data or install directory; `None` for a bare archive.
    pub fn root(&self) -> Option<&Path> {
        match &self.inner {
            Inner::Imported(d) => Some(d.root()),
            Inner::Install { root, .. } => Some(root),
            Inner::Archive { .. } => None,
        }
    }

    /// The imported data directory, when that is the layout.
    pub fn data_dir(&self) -> Option<&DataDir> {
        match &self.inner {
            Inner::Imported(d) => Some(d),
            _ => None,
        }
    }

    /// The manifest of an imported data directory.
    pub fn manifest(&self) -> Option<&Manifest> {
        self.data_dir().map(DataDir::manifest)
    }

    /// The base archive (without overrides).
    pub fn archive(&self) -> &BigArchive {
        match &self.inner {
            Inner::Imported(d) => d.archive(),
            Inner::Install { archive, .. } | Inner::Archive { archive } => archive,
        }
    }

    /// Loose files that shadow archive entries, by normalised name (empty unless imported).
    pub fn overrides(&self) -> &HashMap<String, PathBuf> {
        match &self.inner {
            Inner::Imported(d) => d.overrides(),
            _ => NO_OVERRIDES.get_or_init(HashMap::new),
        }
    }

    /// Reads a file of the archive namespace by game path, overrides first, and says where it
    /// came from. `Ok(None)` when it does not exist.
    pub fn read_with_origin(&self, name: &str) -> io::Result<Option<(Vec<u8>, Origin)>> {
        read_layered(self.archive(), self.overrides(), name)
    }

    /// Reads a file of the archive namespace by game path, overrides first.
    pub fn read(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(self.read_with_origin(name)?.map(|(b, _)| b))
    }

    /// Whether `name` exists in the overrides or the archive.
    pub fn contains(&self, name: &str) -> bool {
        self.overrides().contains_key(&normalize_name(name)) || self.archive().find(name).is_some()
    }

    /// A file outside the archive by its install name (`Music4Cat.wxd`, `default_keys.cfg`),
    /// ignoring case.
    pub fn install_file(&self, name: &str) -> Option<PathBuf> {
        match &self.inner {
            Inner::Imported(d) => d.install_file(name),
            Inner::Install { root, .. } => find_ci(root, name, false),
            Inner::Archive { .. } => None,
        }
    }

    /// The streamed music bank.
    pub fn music_path(&self) -> Option<PathBuf> {
        self.install_file(MUSIC_FILE)
    }

    /// The Beast speech bank.
    pub fn beast_speech_path(&self) -> Option<PathBuf> {
        self.install_file(BEAST_SPEECH_FILE)
    }

    /// The Somtaaw speech bank.
    pub fn sect_speech_path(&self) -> Option<PathBuf> {
        self.install_file(SECT_SPEECH_FILE)
    }

    /// The folder of the Bink movies.
    pub fn movies_dir(&self) -> Option<PathBuf> {
        match &self.inner {
            Inner::Imported(d) => Some(d.root().join(MOVIES_DIR)).filter(|p| p.is_dir()),
            Inner::Install { root, .. } => find_ci(root, INSTALL_MOVIES_DIR, true),
            Inner::Archive { .. } => None,
        }
    }

    /// A movie by file name (`Animatic01.bik`), ignoring case.
    pub fn movie_path(&self, name: &str) -> Option<PathBuf> {
        match &self.inner {
            Inner::Imported(d) => d.movie_path(name),
            _ => find_ci(&self.movies_dir()?, name, false),
        }
    }

    /// The folder of the extracted mission IR (imported data only: an install has its missions
    /// inside `cataclysm.exe`, see [`Self::exe_path`]).
    pub fn missions_dir(&self) -> Option<PathBuf> {
        self.data_dir().map(|d| d.root().join(MISSIONS_DIR)).filter(|p| p.is_dir())
    }

    /// Encoded mission IR (`hwc_kas::binary`) of an imported data directory.
    pub fn mission_ir(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        match self.data_dir() {
            Some(d) => d.mission_ir(name),
            None => Ok(None),
        }
    }

    /// The original executable of an install (to extract missions from during development).
    pub fn exe_path(&self) -> Option<PathBuf> {
        match &self.inner {
            Inner::Install { root, .. } => find_ci(root, INSTALL_EXE, false),
            _ => None,
        }
    }

    /// The importer's Bink tables file (`bink_tables.bin`), when present.
    pub fn bink_tables_path(&self) -> Option<PathBuf> {
        let d = self.data_dir()?;
        d.bink_tables_path().or_else(|| Some(d.root().join(BINK_TABLES_FILE))).filter(|p| p.is_file())
    }

    /// Where to load the Bink codec tables from: the imported tables file, or for an install its
    /// `binkw32.dll`.
    pub fn bink_tables(&self) -> Option<BinkTablesSource> {
        match &self.inner {
            Inner::Imported(_) => self.bink_tables_path().map(BinkTablesSource::Imported),
            Inner::Install { root, .. } => find_ci(root, INSTALL_BINKW32, false).map(BinkTablesSource::Dll),
            Inner::Archive { .. } => None,
        }
    }

    /// `rel` (a `/`-separated path) inside the data or install directory.
    pub fn path(&self, rel: &str) -> Option<PathBuf> {
        self.root().map(|r| datadir::join_rel(r, rel))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("hwc-formats-gamedata-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn empty_big() -> Vec<u8> {
        let mut big = b"RBF1.23".to_vec();
        big.extend_from_slice(&0i32.to_le_bytes());
        big.extend_from_slice(&1i32.to_le_bytes());
        big
    }

    #[test]
    fn opens_install_and_bare_archive() {
        let dir = scratch("install");
        fs::write(dir.join("Cataclysm.BIG"), empty_big()).unwrap();
        fs::write(dir.join("music4cat.wxd"), b"WXD1").unwrap();
        fs::write(dir.join("BINKW32.DLL"), b"MZ").unwrap();
        fs::create_dir_all(dir.join("MOVIES")).unwrap();
        fs::write(dir.join("MOVIES").join("animatic01.bik"), b"BIK").unwrap();

        let g = GameData::open(&dir).unwrap();
        assert_eq!(g.layout(), Layout::Install);
        assert!(g.music_path().unwrap().ends_with("music4cat.wxd"));
        assert!(g.sect_speech_path().is_none());
        assert!(g.movie_path("Animatic01.bik").is_some());
        assert!(g.missions_dir().is_none());
        assert!(matches!(g.bink_tables(), Some(BinkTablesSource::Dll(_))));
        assert!(g.read("soundevent.script").unwrap().is_none());

        let g = GameData::open(dir.join("Cataclysm.BIG")).unwrap();
        assert_eq!(g.layout(), Layout::Archive);
        assert!(g.root().is_none() && g.music_path().is_none() && g.bink_tables().is_none());

        assert!(GameData::open(dir.join("MOVIES")).is_err());
        assert!(GameData::open(dir.join("nothing")).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
