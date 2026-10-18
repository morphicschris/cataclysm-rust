//! Engine-side access to an imported data directory.
//!
//! Archive names (`race_sect\smothership\...`) resolve to a loose file under `overrides\` when
//! one exists (matched ignoring ASCII case, `/` or `\` separators), otherwise to the entry in
//! `base\cataclysm.big`. Files outside the archive (music, speech, movies, missions) have fixed
//! paths given by the manifest.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use hwc_formats::big::{normalize_name, BigArchive};

use crate::import::{MISSIONS_DIR, OVERRIDES_DIR};
use crate::install::{self, FileKind};
use crate::manifest::Manifest;

/// An opened data directory.
#[derive(Debug)]
pub struct DataDir {
    root: PathBuf,
    manifest: Manifest,
    archive: BigArchive,
    /// Normalised archive name -> loose file.
    overrides: HashMap<String, PathBuf>,
}

/// Where [`DataDir::read`] found a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Override,
    Archive,
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

/// Lists every file under `dir` as (normalised archive-style name, path).
pub fn scan_overrides(dir: &Path) -> io::Result<HashMap<String, PathBuf>> {
    let mut out = HashMap::new();
    if !dir.is_dir() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            let path = e.path();
            if e.file_type()?.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                let name: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
                out.insert(normalize_name(&name.join("\\")), path);
            }
        }
    }
    Ok(out)
}

impl DataDir {
    /// Opens a data directory written by [`crate::import()`].
    pub fn open(root: impl AsRef<Path>) -> io::Result<DataDir> {
        let root = root.as_ref().to_path_buf();
        let manifest = Manifest::load(&root)?.ok_or_else(|| invalid(format!("{}: no manifest; run hwc-import", root.display())))?;
        let big = manifest
            .files
            .iter()
            .find(|f| f.kind == FileKind::Archive)
            .ok_or_else(|| invalid("manifest lists no archive".into()))?;
        let archive = BigArchive::open(root.join(&big.path)).map_err(|e| invalid(format!("{}: {e}", big.path)))?;
        let overrides = scan_overrides(&root.join(OVERRIDES_DIR))?;
        Ok(DataDir { root, manifest, archive, overrides })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    /// The base archive (without overrides).
    pub fn archive(&self) -> &BigArchive {
        &self.archive
    }

    /// Loose override files, by normalised name.
    pub fn overrides(&self) -> &HashMap<String, PathBuf> {
        &self.overrides
    }

    /// Reads an archive-namespace file, preferring an override.
    pub fn read(&self, name: &str) -> io::Result<Option<(Vec<u8>, Origin)>> {
        if let Some(p) = self.overrides.get(&normalize_name(name)) {
            return fs::read(p).map(|b| Some((b, Origin::Override)));
        }
        match self.archive.read_file(name) {
            Ok(Some(b)) => Ok(Some((b, Origin::Archive))),
            Ok(None) => Ok(None),
            Err(e) => Err(invalid(format!("{name}: {e}"))),
        }
    }

    /// Whether `name` exists in the overrides or the archive.
    pub fn contains(&self, name: &str) -> bool {
        self.overrides.contains_key(&normalize_name(name)) || self.archive.find(name).is_some()
    }

    fn path_of(&self, rel: &str) -> PathBuf {
        rel.split('/').fold(self.root.clone(), |p, part| p.join(part))
    }

    /// Path of a file the importer copied, by its original install name (`Music4Cat.wxd`,
    /// `SectSpeech.vce`, `Animatic01.bik`, `default_keys.cfg`), ignoring case.
    pub fn install_file(&self, name: &str) -> Option<PathBuf> {
        self.manifest
            .files
            .iter()
            .find(|f| f.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .map(|f| self.path_of(&f.path))
    }

    /// The streamed music data.
    pub fn music_path(&self) -> Option<PathBuf> {
        self.install_file(install::MUSIC)
    }

    /// Movie path by file name (`Animatic01.bik`).
    pub fn movie_path(&self, name: &str) -> Option<PathBuf> {
        self.manifest
            .files
            .iter()
            .filter(|f| f.kind == FileKind::Movie)
            .find(|f| f.path.rsplit('/').next().is_some_and(|n| n.eq_ignore_ascii_case(name)))
            .map(|f| self.path_of(&f.path))
    }

    /// Encoded mission IR (`hwc_kas::binary`), when missions have been extracted.
    pub fn mission_ir(&self, name: &str) -> io::Result<Option<Vec<u8>>> {
        let Some(rel) = self.manifest.missions.missions.iter().find(|m| m.name.eq_ignore_ascii_case(name)).and_then(|m| m.path.clone())
        else {
            return Ok(None);
        };
        debug_assert!(rel.starts_with(MISSIONS_DIR));
        fs::read(self.path_of(&rel)).map(Some)
    }
}
