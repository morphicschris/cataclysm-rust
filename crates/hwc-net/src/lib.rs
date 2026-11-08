//! Network games (`docs/multiplayer-plan.md`, stages 4 and 5).
//!
//! The original is a lockstep game: every machine runs the same simulation and only the
//! players' orders travel, two universe updates to a turn. This crate does the same with the
//! port's deterministic simulation:
//!
//! * [`lockstep`]: the turns. Each human player sends one packet per turn with the orders
//!   given (applied a few turns later on every machine, in player order) and the checksum of
//!   the state it last reached, so a desynchronisation is found at once.
//! * [`wire`]: the messages and their framing.
//! * [`tcp`]: a host and its clients over TCP; the host relays every packet (a star, as the
//!   original's routing server was).
//! * [`lobby`]: the game being set up before it starts: slots, races, options, chat.
//! * [`discovery`]: finding games on the local network by UDP broadcast.
//! * [`session`]: all of it together, for the game: the lobby, then the running game.

pub mod discovery;
pub mod lobby;
pub mod lockstep;
pub mod session;
pub mod tcp;
pub mod wire;

/// The version of the protocol; peers with another are refused.
pub const PROTOCOL: u32 = 1;

/// The build a game needs every machine to run: lockstep needs the same simulation
/// everywhere. The package version and the protocol (the app may add its own build id).
pub fn build_id() -> String {
    format!("hwc-net {} p{}", env!("CARGO_PKG_VERSION"), PROTOCOL)
}

/// The TCP port games are hosted on, and the UDP port games are found on.
pub const GAME_PORT: u16 = 28470;
pub const DISCOVERY_PORT: u16 = 28471;
