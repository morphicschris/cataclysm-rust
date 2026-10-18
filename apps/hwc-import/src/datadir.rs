//! Engine-side access to an imported data directory. The reader lives in
//! `hwc_formats::datadir` (and `hwc_formats::GameData` wraps it together with the development
//! layouts); it is re-exported here for existing users of the importer crate.

pub use hwc_formats::datadir::*;
