//! Readers for the original game's data formats. Game data is never bundled: tests that need it read
//! the user's install from the `HWC_GAME_DIR` environment variable and skip when it is unset.

pub mod big;
pub mod lzss;
pub mod lif;
pub mod geo;
pub mod script;
pub mod audio;
pub mod mex;
pub mod mad;
pub mod btg;
pub mod font;
pub mod fib;
pub mod hsf;
pub mod nis;
pub mod manifest;
pub mod datadir;
pub mod gamedata;
pub mod check;

pub use gamedata::GameData;

use std::path::PathBuf;

/// Location of the original game install, for integration tests and tools.
pub fn game_dir() -> Option<PathBuf> {
    std::env::var_os("HWC_GAME_DIR").map(PathBuf::from)
}
