//! First-run importer: turns the player's own Homeworld: Cataclysm install into this game's data
//! directory. Nothing from the original game ships with the project; everything the engine needs
//! is taken from the install the player owns.
//!
//! * [`install::detect`] recognises and validates an install (read-only).
//! * [`import()`] builds or incrementally refreshes a data directory, reporting [`Progress`].
//! * [`verify()`] re-hashes a data directory against its manifest and parse-checks every asset.
//! * [`DataDir`] is the engine-side reader: loose files in `overrides\` first, then the archive.
//!
//! The layout and manifest are documented in `docs/import.md`.

use std::path::PathBuf;

use thiserror::Error;

pub mod datadir;
pub mod hash;
mod import;
pub mod install;
pub mod manifest;
pub mod missions;
pub mod verify;

pub use datadir::DataDir;
pub use import::{import, ImportOptions, ImportReport};
pub use install::{detect, Install};
pub use manifest::Manifest;
pub use verify::{verify, VerifyOptions, VerifyReport};

/// Errors that stop an import.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("{} is not a usable Homeworld: Cataclysm install:\n  {}", dir.display(), problems.join("\n  "))]
    NotAnInstall { dir: PathBuf, problems: Vec<String> },
    #[error("refusing to write to {}: {reason}", dir.display())]
    UnsafeOutput { dir: PathBuf, reason: String },
    #[error("{} is not empty and has no manifest; pass --force to import into it anyway", .0.display())]
    OutputNotEmpty(PathBuf),
    #[error("{}: {source}", path.display())]
    Io { path: PathBuf, source: std::io::Error },
}

impl ImportError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(std::io::Error) -> ImportError {
        let path = path.into();
        move |source| ImportError::Io { path, source }
    }
}

/// Phases of an import or verification, in order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Detect,
    Copy,
    Missions,
    Finish,
    VerifyHashes,
    VerifyAssets,
}

/// Progress notifications for a launcher UI or the command line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Progress<'a> {
    Stage(Stage),
    /// A file is being copied (or, while verifying, hashed).
    FileStarted { path: &'a str, size: u64 },
    /// A file was already up to date.
    FileSkipped { path: &'a str },
    /// Overall byte progress of the current stage.
    Bytes { done: u64, total: u64 },
    /// Assets parse-checked so far during verification.
    Assets { done: usize, total: usize },
    Warning(&'a str),
}

/// A progress callback that ignores everything.
pub fn no_progress(_: &Progress) {}
