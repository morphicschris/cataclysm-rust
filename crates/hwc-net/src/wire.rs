//! The messages between machines and their framing: a 32-bit little-endian length, then the
//! message in bincode.

use serde::{Deserialize, Serialize};

use crate::lobby::{LobbyRequest, LobbyState};
use crate::lockstep::TurnPacket;

/// The largest message accepted (a lobby or a turn is far smaller).
pub const MAX_MESSAGE: usize = 1 << 20;

/// A message of the game protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Msg {
    /// A client introduces itself.
    Hello { protocol: u32, build: String, name: String },
    /// The host turns a client away.
    Refused { reason: String },
    /// The host takes a client in and says its number.
    Welcome { client: u32 },
    /// The game being set up, as the host has it now.
    Lobby(LobbyState),
    /// A client's change to its own slot.
    Request(LobbyRequest),
    /// A line of chat.
    Chat { from: String, text: String },
    /// The game starts: the setup every machine plays (with the receiver's own player) and
    /// the players' names.
    Start { setup: hwc_sim::skirmish::GameSetup, names: Vec<String>, delay: u32 },
    /// A player's orders for a turn.
    Turn(TurnPacket),
    /// A player left; the game goes on without them from the turn.
    Dropped { player: u8, from_turn: u32 },
    /// The sender leaves.
    Leave,
}

/// The message with its length in front.
pub fn encode(msg: &Msg) -> Vec<u8> {
    encode_any(msg)
}

/// Any serialisable message (the lobby server's too) with its length in front.
pub fn encode_any<T: Serialize>(msg: &T) -> Vec<u8> {
    let body = bincode::serialize(msg).expect("messages serialise");
    let mut out = Vec::with_capacity(body.len() + 4);
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// Why a stream of messages could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WireError {
    TooLarge(usize),
    Malformed(String),
}

impl std::fmt::Display for WireError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WireError::TooLarge(n) => write!(f, "a message of {n} bytes"),
            WireError::Malformed(e) => write!(f, "a malformed message: {e}"),
        }
    }
}

/// Splits a byte stream back into messages.
#[derive(Debug, Default)]
pub struct Decoder {
    buf: Vec<u8>,
}

impl Decoder {
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// The next whole message, if one has come.
    pub fn next(&mut self) -> Option<Result<Msg, WireError>> {
        self.next_as()
    }

    /// The next whole message as `T`.
    pub fn next_as<T: serde::de::DeserializeOwned>(&mut self) -> Option<Result<T, WireError>> {
        let frame = match self.next_frame()? {
            Ok(f) => f,
            Err(e) => return Some(Err(e)),
        };
        Some(bincode::deserialize(&frame).map_err(|e| WireError::Malformed(e.to_string())))
    }

    /// The next whole frame's body, undecoded.
    pub fn next_frame(&mut self) -> Option<Result<Vec<u8>, WireError>> {
        if self.buf.len() < 4 {
            return None;
        }
        let n = u32::from_le_bytes(self.buf[..4].try_into().expect("4 bytes")) as usize;
        if n > MAX_MESSAGE {
            return Some(Err(WireError::TooLarge(n)));
        }
        if self.buf.len() < 4 + n {
            return None;
        }
        let body = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Some(Ok(body))
    }

    /// The bytes not yet taken as frames (handed on when a connection becomes a pipe).
    pub fn take_rest(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.buf)
    }
}
