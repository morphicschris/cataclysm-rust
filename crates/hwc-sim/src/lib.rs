//! Simulation layer of the Homeworld: Cataclysm recreation.
//!
//! * [`statics`]: the static game data the engine loads at startup (ship and weapon statistics,
//!   docking and navigation-light points, resources, derelicts, research, unit caps, team
//!   colours) with the values it derives while loading. See `docs/sim/static-info.md`.
//! * The run-time simulation, ported from `cataclysm.exe` with its floating-point evaluation
//!   order: the object model ([`object`]), the universe and its fixed 16 Hz update
//!   ([`universe`]), physics ([`physics`]), collision blobs ([`blobs`]), the AI layers that fly
//!   ships ([`aitrack`], [`aiship`]), move orders and formations ([`command`], [`formation`]),
//!   run-time tunables ([`tunables`]) and a determinism harness ([`harness`]); missiles and
//!   mines ([`missile`]), the per-type ship modules and their special abilities ([`ships`]),
//!   cloaking ([`cloak`]), disabling ([`disable`]), per-player visibility ([`visibility`]) and
//!   fog of war ([`fow`]). See `docs/sim/runtime.md`.

// The x87 comparisons the original makes treat NaN in specific ways; `!(a < b)` is written
// deliberately where the binary tests "not less" (true for NaN).
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod aimove;
pub mod aiship;
pub mod aitrack;
pub mod attack;
pub mod autobehaviour;
pub mod autoplay;
pub mod base;
pub mod base_tweaks;
pub mod battle_ping;
pub mod blobs;
pub mod bullet;
pub mod campaign;
pub mod clamp;
pub mod cloak;
pub mod collision;
pub mod combat_statics;
pub mod combat_tweaks;
pub mod command;
pub mod construction;
pub mod damage;
pub mod disable;
pub mod dock;
pub mod effects;
#[cfg(test)]
mod dock_rows;
pub mod flightman;
pub mod formation;
pub mod fow;
pub mod gun;
pub mod harness;
pub mod kas_host;
pub mod level;
pub mod link;
pub mod missile;
pub mod mission;
pub mod modules;
pub mod nebula;
pub mod nis_level;
pub mod nis_support;
pub mod object;
pub mod parade;
pub mod player_orders;
pub mod physics;
pub mod present;
pub mod research;
pub mod resource;
pub mod salvage;
pub mod save;
pub mod ships;
pub mod shockwave;
pub mod statics;
pub mod tactics;
pub mod target;
pub mod think;
pub mod tunables;
pub mod universe;
pub mod upgrade;
pub mod visibility;

pub use object::{Ship, ShipId};
pub use statics::{LoadOptions, StaticWorld};
pub use tunables::SimTweaks;
pub use universe::Universe;
