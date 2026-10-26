//! Guards for determinism that need no game data.
//!
//! The simulation has to give the same results in every process and every build (lockstep,
//! replays, the 1:1 comparison with the original). Std's hashed collections iterate in an
//! order that is seeded per process (`RandomState`), so the crate does not use them at all:
//! ordered collections (`BTreeMap`, `BTreeSet`, `Vec`) are used instead, and clippy is told
//! the same (`clippy.toml`, `disallowed-types`).

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src") {
        let path = entry.expect("entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// No source file of the simulation names a hashed collection or a randomly seeded hasher, so
/// no iteration order in it can depend on hashing.
#[test]
fn the_simulation_uses_no_hashed_collections() {
    const BANNED: [&str; 5] = ["HashMap", "HashSet", "RandomState", "DefaultHasher", "hash_map"];
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    assert!(files.len() > 10, "found the sources ({})", files.len());
    let mut hits = Vec::new();
    for f in &files {
        let text = std::fs::read_to_string(f).expect("read");
        for (n, line) in text.lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            for b in BANNED {
                if code.contains(b) {
                    hits.push(format!("{}:{}: {b}", f.strip_prefix(&src).unwrap_or(f).display(), n + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "hashed collections in the simulation (use BTreeMap/BTreeSet/Vec):\n{}", hits.join("\n"));
}
