//! Pure-Rust decoder for the Bink 1 movies shipped with Homeworld: Cataclysm.
//!
//! Every movie in `Movies\*.bik` is revision `f` Bink video, 640×480 YUV 4:2:0 without alpha;
//! the animatics run at 30 fps, `sierra.bik` at 15 fps, and none of them carries an audio
//! track (the animatic soundtracks are speech events driven by the matching `.script` files).
//! See `docs/formats/bink.md` for the format notes and how the original game plays the movies.
//!
//! The codec's constant tables are not part of this crate: they are read from the player's own
//! `binkw32.dll` ([`BinkTables::from_binkw32`]), or from the file the importer made of them
//! ([`import_tables`], [`BinkTables::load`]).
//!
//! ```no_run
//! let tables = hwc_video::BinkTables::from_install("C:/Games/Cataclysm")?;
//! let movie = hwc_video::BinkFile::open("Animatic01.bik")?;
//! for frame in movie.frames(&tables)? {
//!     let frame = frame?;
//!     // frame.rgba is width * height * 4 bytes, frame.time is in seconds
//! }
//! # Ok::<(), hwc_video::Error>(())
//! ```

mod bits;
pub mod container;
pub mod tables;
pub mod video;

pub use container::{AudioTrack, BinkFile, FrameEntry, FramePacket, Header};
pub use tables::{import_tables, BinkTables, TABLES_FILE_NAME};
pub use video::{yuv_to_rgba, Frame, Frames, VideoDecoder, YuvFrame};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Bink file")]
    NotBink,
    #[error("file is truncated")]
    Truncated,
    #[error("invalid data: {0}")]
    Invalid(&'static str),
    #[error("Bink table not found in binkw32.dll: {0}")]
    TablesNotFound(&'static str),
    #[error("Bink table does not match the expected checksum: {0}")]
    TablesMismatch(&'static str),
    #[error("unsupported: {0}")]
    Unsupported(&'static str),
}

pub type Result<T> = std::result::Result<T, Error>;
