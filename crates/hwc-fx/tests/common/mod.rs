//! Shared helpers for tests that need the game data (skipped when it is not configured).

#![allow(dead_code)]

use hwc_formats::gamedata::GameData;
use hwc_fx::compiler::{DirSource, FileSource};

pub enum Data {
    Game(GameData),
    Dir(DirSource),
}

impl FileSource for Data {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Data::Game(g) => g.read_file(path),
            Data::Dir(d) => d.read_file(path),
        }
    }
}

/// `HWC_GAME_DIR` (or `HWC_DATA_DIR`) via `GameData`, else `HWC_EXTRACTED_DIR`.
pub fn data() -> Option<Data> {
    if let Some(Ok(g)) = GameData::from_env() {
        return Some(Data::Game(g));
    }
    let dir = std::env::var_os("HWC_EXTRACTED_DIR")?;
    Some(Data::Dir(DirSource { root: dir.into() }))
}

/// Every `ETG\*.ebg` file.
pub fn effect_files(d: &Data) -> Vec<String> {
    let mut v: Vec<String> = match d {
        Data::Game(g) => g
            .archive()
            .entries()
            .iter()
            .map(|e| e.name.clone())
            .filter(|n| {
                let l = n.to_ascii_lowercase();
                l.starts_with("etg\\") && l.ends_with(".ebg") && l.matches('\\').count() == 1
            })
            .collect(),
        Data::Dir(s) => {
            let dir = hwc_formats::gamedata::find_ci(&s.root, "etg", true).expect("etg directory");
            std::fs::read_dir(dir)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.to_ascii_lowercase().ends_with(".ebg"))
                .map(|n| format!("ETG\\{n}"))
                .collect()
        }
    };
    v.sort();
    v
}
