//! Optional Homeworld Remastered effects.
//!
//! When the player owns Homeworld Remastered, its ship death explosions and engine trails can
//! replace the stock Cataclysm ones (presentation only: the simulation and its effects are
//! untouched). Nothing from Remastered ships with this game: [`import`] copies what is needed
//! from the player's own install, and the rest of the crate plays it.
//!
//! * [`big`]: the Homeworld 2 `.big` archive format.
//! * [`lua`]: a reader for the Lua data files (effects, animations, ship events and tuning).
//! * [`fx`]: effect definitions; [`ships`]: death sequences and engine trails.
//! * [`library`]: an imported folder, loaded; [`player`]: runs effects and draws them.

pub mod big;
pub mod fx;
pub mod import;
pub mod library;
pub mod lua;
pub mod player;
pub mod ships;
pub mod sound;

pub use library::{Image, Library, TexId};
pub use player::{Anchor, DrawList, Player};
