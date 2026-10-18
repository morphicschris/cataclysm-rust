//! `manifest.json`: what was imported, from where, and the hashes needed for incremental
//! re-import and `--verify`. The types live in `hwc_formats::manifest` so the engine can read the
//! data directory without depending on the importer; they are re-exported here.

pub use hwc_formats::manifest::*;
