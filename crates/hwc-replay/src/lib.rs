//! Parity harness: how close is the recreation to the original `cataclysm.exe`?
//!
//! The plan (see `docs/parity.md`) is to record a game in the original, replay the same inputs
//! through our engine and compare the world state tick by tick. This crate holds the pieces
//! that do not depend on the simulation itself:
//!
//! * [`demo`]: the original's demo recordings (`.dem`, header `BaNng`): raw keyboard/mouse
//!   snapshots plus task-tick counts, starting from saved random-number streams. Reader and
//!   writer, byte-exact round trip.
//! * [`pkts`]: the original's packet recordings (`.pkts`, "Record game" in the in-game menu or
//!   `/packetRecord`): the lockstep sync packets that carry every player command, with the
//!   original's own per-packet sync checksums. This is what a deterministic replay consumes.
//! * [`netlog`]: the text sync log the original writes with `/logOn` / `/logOnVerbose`
//!   (`netlog.txt`): per-packet checksums and, verbose, one line per ship.
//! * [`state`]: our compact per-universe-tick world-state record.
//! * [`compare`]: compares two state streams with no tolerance on bit-exact fields and reports
//!   the first divergence with context.
//! * [`checksum`]: the original's universe sync checksum, recomputed from a state frame.

pub mod checksum;
pub mod compare;
pub mod demo;
pub mod netlog;
pub mod pkts;
pub mod state;

mod bytes;

pub use bytes::FormatError;
