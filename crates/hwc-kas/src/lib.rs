//! Homeworld: Cataclysm mission scripting (KAS).
//!
//! Cataclysm's single-player missions are KAS state machines compiled to C by kas2c and linked
//! into the executable. This crate represents a mission as data and runs it with the original
//! runtime semantics:
//!
//! * [`ir`]: the mission IR (FSMs, states, INIT/WATCH code, conditions, runtime calls by
//!   kas2c table index with typed arguments, localised strings, label hashes).
//! * [`functions`]: the 486-entry kas2c runtime function table (generated).
//! * [`host`]: the game interface: [`KasRuntime`] (one typed method per runtime function,
//!   generated) and [`KasHost`] (time, random numbers, language, hooks).
//! * [`interp`]: the interpreter (`kasMissionStart`, `kasExecute`, `kasJump`, `kasFSMCreate`,
//!   scoped variables and timers, team message queues, IFONCE, the KAS pause).
//! * Loaders: [`json`] (exact structured form exported from the machine code, the dev path),
//!   [`text`] (the `.kas` text rendering, lossy), [`binary`] (compact cache form). [`print`](mod@print)
//!   renders IR back to KAS-like text.
//! * [`extract`]: notes on reading missions straight from the player's `cataclysm.exe`.
//!
//! No game content is part of this crate: missions are loaded at run time from data the player
//! provides.

#![warn(missing_docs)]

pub mod binary;
pub mod extract;
pub mod functions;
mod hash;
pub mod host;
pub mod interp;
pub mod ir;
pub mod json;
pub mod print;
pub mod text;

pub use functions::{FuncId, ParamType, FUNCTIONS};
pub use hash::{kas_hash, kas_hash_bytes, var_hash, Hash, GLOBAL_FLAG};
pub use host::{Cx, KasHost, KasRuntime, Point, Scope, Ships, Str, Team, Value};
pub use interp::Interpreter;
pub use ir::Mission;

use std::fmt;

/// A load or parse error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Error {
    /// Description, innermost context last.
    pub message: String,
}

impl Error {
    /// A new error.
    pub fn new(message: impl Into<String>) -> Self {
        Error { message: message.into() }
    }

    /// Prefixes the message with where it happened.
    pub fn context(mut self, at: &str) -> Self {
        self.message = format!("{at}: {}", self.message);
        self
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for Error {}
