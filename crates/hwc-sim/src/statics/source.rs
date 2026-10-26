//! Where static data files come from: the game's `.big` archive, or a directory holding an
//! extracted copy of it. Lookups follow the engine: ASCII case is ignored and `\` and `/` are
//! interchangeable.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hwc_formats::big::{normalize_name, BigArchive};

/// Read-only access to game files by their archive path (e.g. `RACE_SECT\sAcolyte.shp`).
pub trait DataSource {
    /// The file's bytes, or `None` when it does not exist or cannot be read.
    fn read(&self, path: &str) -> Option<Vec<u8>>;

    /// Whether the file exists (the engine's `fileExists`).
    fn exists(&self, path: &str) -> bool;
}

impl DataSource for BigArchive {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.read_file(path).ok().flatten()
    }

    fn exists(&self, path: &str) -> bool {
        self.find(path).is_some()
    }
}

/// An extracted archive on disk. All file names are indexed up front so lookups can ignore case
/// the way the archive does.
#[derive(Debug, Clone)]
pub struct DirSource {
    root: PathBuf,
    files: BTreeMap<String, PathBuf>,
}

impl DirSource {
    pub fn new(root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = root.as_ref().to_path_buf();
        let mut files = BTreeMap::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)? {
                let entry = entry?;
                let path = entry.path();
                if entry.file_type()?.is_dir() {
                    stack.push(path);
                } else if let Ok(rel) = path.strip_prefix(&root) {
                    let key = normalize_name(&rel.to_string_lossy());
                    files.insert(key, path);
                }
            }
        }
        Ok(Self { root, files })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl DataSource for DirSource {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        std::fs::read(self.files.get(&normalize_name(path))?).ok()
    }

    fn exists(&self, path: &str) -> bool {
        self.files.contains_key(&normalize_name(path))
    }
}

/// Joins an engine directory prefix (which ends in a separator, or is empty) and a file name.
pub(crate) fn join(dir: &str, file: &str) -> String {
    format!("{dir}{file}")
}
