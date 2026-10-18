//! Simulation layer of the Homeworld: Cataclysm recreation.
//!
//! So far this holds the *static* game data: everything the original engine loads into memory at
//! startup before a mission begins (ship and weapon statistics, docking and navigation-light
//! points, resource and derelict definitions, research dependencies, unit caps, team colours),
//! together with the values the engine derives from them while loading. See [`statics`] and
//! `docs/sim/static-info.md`.

pub mod statics;

pub use statics::{LoadOptions, StaticWorld};
