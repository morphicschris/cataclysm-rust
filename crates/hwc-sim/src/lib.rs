//! Simulation layer of the Homeworld: Cataclysm recreation.
//!
//! * [`statics`]: the static game data the engine loads at startup (ship and weapon statistics,
//!   docking and navigation-light points, resources, derelicts, research, unit caps, team
//!   colours) with the values it derives while loading. See `docs/sim/static-info.md`.
//! * The run-time simulation, ported from `cataclysm.exe` with its floating-point evaluation
//!   order: the object model ([`object`]), the universe and its fixed 16 Hz update
//!   ([`universe`]), physics ([`physics`]), collision blobs ([`blobs`]), the AI layers that fly
//!   ships ([`aitrack`], [`aiship`]), move orders and formations ([`command`], [`formation`]),
//!   run-time tunables ([`tunables`]) and a determinism harness ([`harness`]). See
//!   `docs/sim/runtime.md`.

// The x87 comparisons the original makes treat NaN in specific ways; `!(a < b)` is written
// deliberately where the binary tests "not less" (true for NaN).
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod aiship;
pub mod aitrack;
pub mod blobs;
pub mod command;
pub mod formation;
pub mod harness;
pub mod object;
pub mod physics;
pub mod statics;
pub mod tactics;
pub mod tunables;
pub mod universe;

pub use object::{Ship, ShipId};
pub use statics::{LoadOptions, StaticWorld};
pub use tunables::SimTweaks;
pub use universe::Universe;
