//! Audio engine for the Homeworld: Cataclysm recreation.
//!
//! Layers, bottom up:
//!
//! * [`codec`]: the spectral primitives of the original mixer (block dequantiser, per-band EQ,
//!   coefficient-domain pitch shift, inverse transform, 16-bit output stage).
//! * [`mixer`]: the original sound library: 32 effect voices with priorities, handles and fades,
//!   four streams (three speech, one music) with queues and loop handling, and the 22 050 Hz
//!   stereo block mix.
//! * [`tables`] and [`data`]: the distance/EQ tables, the cardioid table, listener geometry,
//!   tweaks, and loading the game's banks, event tables, music and speech.
//! * [`system`]: what the game calls: sound events resolved through the `PKG0` tables, speech
//!   sentences through `LUT1`/`LUT2` and the `.vce` files, music tracks, volumes, listener.
//! * [`offline`]: deterministic rendering of a scripted sequence to WAV, and level/spectrum
//!   measurements; [`output`] (feature `output`): real-time playback through cpal, converted to
//!   the device rate by [`resample`].
//!
//! The defaults reproduce the original; improvements (symmetric synthesis window, soft limiter, music crossfades, music ducking under speech) are opt-in.

pub mod codec;
pub mod data;
pub mod mixer;
pub mod offline;
#[cfg(feature = "output")]
pub mod output;
pub mod resample;
pub mod system;
pub mod tables;

pub use data::{DataSource, Race, SoundData};
pub use mixer::{Mixer, MixerOptions, NO_HANDLE};
pub use system::{event, BankKind, Emitter, GameContext, ShipSounds, SoundSystem};
