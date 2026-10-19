//! The game's files for the sandbox and mission modes: one [`GameData`] (an imported data
//! directory, an install, or a bare `cataclysm.big`) seen as the renderer's [`AssetSource`] and
//! the simulation's [`DataSource`].

use std::path::{Path, PathBuf};

use hwc_formats::gamedata::{find_ci, GameData};
use hwc_render::AssetSource;
use hwc_sim::statics::DataSource;

pub struct GameAssets {
    pub data: GameData,
    /// Where `data` was opened from (to open a second handle for the audio engine).
    pub path: PathBuf,
}

impl GameAssets {
    /// Opens `path`. A `.big` inside a game install opens the install instead, so the mission
    /// scripts (in `cataclysm.exe`), music and speech are available too.
    pub fn open(path: &Path) -> Result<Self, String> {
        let install = path
            .is_file()
            .then(|| path.parent())
            .flatten()
            .filter(|dir| find_ci(dir, "cataclysm.exe", false).is_some())
            .map(Path::to_path_buf);
        let path = install.unwrap_or_else(|| path.to_path_buf());
        let data = GameData::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Ok(Self { data, path })
    }

    /// A second, independent handle on the same data.
    pub fn reopen(&self) -> Result<GameData, String> {
        GameData::open(&self.path).map_err(|e| format!("{}: {e}", self.path.display()))
    }
}

impl AssetSource for GameAssets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.data.read(path).ok().flatten()
    }

    fn list(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .data
            .archive()
            .entries()
            .iter()
            .map(|e| hwc_formats::big::normalize_name(&e.name))
            .collect();
        names.extend(self.data.overrides().keys().cloned());
        names.sort();
        names.dedup();
        names
    }
}

impl DataSource for GameAssets {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.data.read(path).ok().flatten()
    }

    fn exists(&self, path: &str) -> bool {
        self.data.contains(path)
    }
}
