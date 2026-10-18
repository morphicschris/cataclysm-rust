//! Multi-stream random number generator (`Randy.c`).
//!
//! Each stream is Marsaglia's "add-with-carry + LCG" combination: a subtract-with-borrow
//! lag generator on `(x, y, z, c)` plus a 69069 linear congruential generator on `n`.
//!
//! Cataclysm differences from HW1 (verified in `cataclysm.exe`):
//! * there are **27** streams (HW1 has 25); streams 25 and 26 are new;
//! * `ranStartup` resets every stream and then advances stream 0 by a time-derived count
//!   (`ranRandomize(0)` inlined);
//! * a new function (0x4eafa0, called from `gameStart`) resets streams `1..27` to the default
//!   parameters, leaving stream 0 untouched.
//!
//! The time source for `ranStartup`/`ranRandomize` is a Win32 call reached through the
//! copy-protection import trampoline, so its identity cannot be read statically; the code
//! reads the dword at offset 12 of a 16-byte `SYSTEMTIME`-sized buffer and keeps its low
//! byte (`wSecond` if it is `GetSystemTime`/`GetLocalTime`). Callers pass that count in
//! explicitly; it only affects stream 0 at start-up and the AI stream at AI start-up.
//!
//! Observed stream use in `cataclysm.exe` (callers pushing a constant stream index):
//! 0 game-start/music/effects, 1 special-ship logic (cloak, Mimic, Acolyte, beast infection),
//! 2 sound mixer, 4 clouds, 5 nebulae, 6 computer player and KAS `Random`, 7 damage
//! locations, 8 battle chatter, 9 NIS statics, 10 core game logic (`random()`/`frandom()`),
//! 11 sound events, 12 speech, 13..17 trails, 25 particles, 26 effects (ETG).

/// Number of streams in Cataclysm.
pub const STREAM_COUNT: usize = 27;

/// HW1 `RAN_ParticleStream`.
pub const RAN_PARTICLE_STREAM: usize = 0;
/// HW1 `RAN_ETG` (Cataclysm uses it for special-ship logic; effects moved to 26).
pub const RAN_ETG: usize = 1;
/// HW1 `RAN_Sound`.
pub const RAN_SOUND: usize = 2;
/// HW1 `RAN_Trails`.
pub const RAN_TRAILS: usize = 3;
/// HW1 `RAN_Clouds`.
pub const RAN_CLOUDS: usize = 4;
/// HW1 `RAN_Nebulae`.
pub const RAN_NEBULAE: usize = 5;
/// HW1 `RAN_AIPlayer`.
pub const RAN_AI_PLAYER: usize = 6;
/// HW1 `RAN_Damage`.
pub const RAN_DAMAGE: usize = 7;
/// HW1 `RAN_Battle`.
pub const RAN_BATTLE: usize = 8;
/// HW1 `RAN_Static`.
pub const RAN_STATIC: usize = 9;
/// HW1 `RAN_Game`: the stream behind `random()`, `frandom()`, `gamerand()`.
pub const RAN_GAME: usize = 10;
/// HW1 `RAN_SoundGameThread`.
pub const RAN_SOUND_GAME_THREAD: usize = 11;
/// HW1 `RAN_SoundBothThreads`.
pub const RAN_SOUND_BOTH_THREADS: usize = 12;
/// HW1 `RAN_Trails0` (13..=17 are `RAN_Trails0..4`).
pub const RAN_TRAILS0: usize = 13;
/// HW1 `RAN_SoundGameThread0` (18..=24 are `RAN_SoundGameThread0..6`).
pub const RAN_SOUND_GAME_THREAD0: usize = 18;
/// Cataclysm: particle systems (`partMakePosition`, `partCreateSystemWithDelta`, ...).
pub const RAN_PARTICLES_CAT: usize = 25;
/// Cataclysm: effect scripts (`etgFRandom`, `etgIRandom`, `etgCRandom`, ...).
pub const RAN_ETG_CAT: usize = 26;

/// `ranstream`: generator state. Field order matches the saved-game layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct RanStream {
    pub x: u32,
    pub y: u32,
    pub z: u32,
    pub c: u32,
    pub n: u32,
}

impl RanStream {
    /// `ranParametersReset` defaults.
    pub const DEFAULT: RanStream = RanStream {
        x: 521_288_629,
        y: 362_436_069,
        z: 1_613_801,
        c: 1,
        n: 1_131_199_209,
    };

    /// `ranRandomFn`: advance and return the next value.
    #[inline]
    pub fn next_u32(&mut self) -> u32 {
        let xc = self.x.wrapping_add(self.c);
        let s = if self.y > xc {
            self.c = 0;
            self.y.wrapping_sub(xc)
        } else {
            self.c = 1;
            xc.wrapping_sub(self.y).wrapping_sub(18)
        };
        self.x = self.y;
        self.y = self.z;
        self.z = s;
        self.n = self.n.wrapping_mul(69069).wrapping_add(1_013_904_243);
        s.wrapping_add(self.n)
    }
}

impl Default for RanStream {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The game's set of random streams (`ranStream[]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Randy {
    pub streams: [RanStream; STREAM_COUNT],
}

impl Default for Randy {
    fn default() -> Self {
        Self {
            streams: [RanStream::DEFAULT; STREAM_COUNT],
        }
    }
}

impl Randy {
    /// `ranStartup`: reset all streams, then advance stream 0 `time_count & 0xFF` times.
    pub fn startup(time_count: u32) -> Self {
        let mut r = Self::default();
        r.randomize(0, time_count);
        r
    }

    /// `ranRandomize(index)`: advance a stream `time_count & 0xFF` times.
    pub fn randomize(&mut self, index: usize, time_count: u32) {
        for _ in 0..(time_count & 0xFF) {
            self.streams[index].next_u32();
        }
    }

    /// Cataclysm's game-start reset (0x4eafa0): streams `1..STREAM_COUNT` back to defaults,
    /// stream 0 untouched.
    pub fn reset_game_streams(&mut self) {
        for s in &mut self.streams[1..] {
            *s = RanStream::DEFAULT;
        }
    }

    /// `ranRandom(index)`.
    #[inline]
    pub fn random(&mut self, index: usize) -> u32 {
        self.streams[index].next_u32()
    }

    /// `ranNumberGet`.
    pub fn number_get(&self, index: usize) -> u32 {
        self.streams[index].n
    }

    /// `ranNumberSet`.
    pub fn number_set(&mut self, index: usize, n: u32) {
        self.streams[index].n = n;
    }

    /// `ranParametersSet`.
    pub fn parameters_set(&mut self, index: usize, p: RanStream) {
        self.streams[index] = p;
    }

    /// `ranParametersGet`.
    pub fn parameters_get(&self, index: usize) -> RanStream {
        self.streams[index]
    }

    /// `ranParametersReset`.
    pub fn parameters_reset(&mut self, index: usize) {
        self.streams[index] = RanStream::DEFAULT;
    }

    /// `randyrandom(stream, n)`: `ranRandom(stream) % n` (unsigned).
    #[inline]
    pub fn random_mod(&mut self, index: usize, n: u32) -> u32 {
        self.random(index) % n
    }

    /// `randyrandombetween(stream, a, b)`: `a + ranRandom % (b - a + 1)` in 32-bit wrapping
    /// unsigned arithmetic, as the macro expands for `udword` operands.
    #[inline]
    pub fn random_between(&mut self, index: usize, a: i32, b: i32) -> i32 {
        let span = (b as u32).wrapping_sub(a as u32).wrapping_add(1);
        (self.random(index) % span).wrapping_add(a as u32) as i32
    }

    /// `frandyrandom(stream, n)` as a register value: the unsigned random number converted
    /// exactly, times `n * 2^-32` (`(real32)UDWORD_Max` is 2^32, so the factor is exact).
    #[inline]
    pub fn frandom_ext(&mut self, index: usize, n: f32) -> f64 {
        let r = f64::from(self.random(index));
        r * (f64::from(n) * (1.0 / 4_294_967_296.0))
    }

    /// `frandyrandombetween(stream, a, b)` as a register value: `frandom(b - a) + a`, where
    /// `b - a` is evaluated in a register before the multiply.
    #[inline]
    pub fn frandom_between_ext(&mut self, index: usize, a: f32, b: f32) -> f64 {
        let r = f64::from(self.random(index));
        let span = f64::from(b) - f64::from(a);
        r * (span * (1.0 / 4_294_967_296.0)) + f64::from(a)
    }

    /// `random(n)`: [`RAN_GAME`] modulo `n`.
    #[inline]
    pub fn game_random(&mut self, n: u32) -> u32 {
        self.random_mod(RAN_GAME, n)
    }

    /// `frandom(n)` on [`RAN_GAME`] (register value).
    #[inline]
    pub fn game_frandom_ext(&mut self, n: f32) -> f64 {
        self.frandom_ext(RAN_GAME, n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(v: [u32; 5]) -> RanStream {
        RanStream {
            x: v[0],
            y: v[1],
            z: v[2],
            c: v[3],
            n: v[4],
        }
    }

    /// Outputs of emulating `ranRandomFn` in cataclysm.exe, chained call to call.
    #[test]
    fn sequence_from_default_matches_binary() {
        let mut s = RanStream::DEFAULT;
        let expected = [
            0x845f5ac7u32,
            0xf342c8c6,
            0xba3a71a7,
            0x350684c9,
            0xa4b6fe30,
            0x87739305,
            0xcfa36a6e,
            0x4eeab12d,
        ];
        for e in expected {
            assert_eq!(s.next_u32(), e);
        }
        assert_eq!(s, st([0x2aa8a56, 0x189fd8, 0x977e59c, 0x1, 0x4572cb91]));
    }

    #[test]
    fn sequence_from_custom_state_matches_binary() {
        let mut s = st([0xdeadbeef, 0x12345678, 0xffffffff, 0, 0x80000000]);
        let expected = [0x88e85bd8u32, 0xa4e36710, 0x0022547d, 0xdc04f5c4, 0x3f9f00b3, 0x978e9a25];
        for e in expected {
            assert_eq!(s.next_u32(), e);
        }
        assert_eq!(s, st([0x21524120, 0xba4511ec, 0x12345657, 0x1, 0x855a43ce]));
    }

    #[test]
    fn game_stream_reset_spares_stream_zero() {
        let mut r = Randy::startup(37);
        let s0 = r.streams[0];
        assert_ne!(s0, RanStream::DEFAULT);
        r.random(RAN_GAME);
        r.random(RAN_ETG_CAT);
        r.reset_game_streams();
        assert_eq!(r.streams[0], s0);
        assert!(r.streams[1..].iter().all(|s| *s == RanStream::DEFAULT));
    }

    #[test]
    fn randomize_uses_low_byte_only() {
        let mut a = Randy::default();
        let mut b = Randy::default();
        a.randomize(3, 0x105);
        b.randomize(3, 5);
        assert_eq!(a, b);
    }

    #[test]
    fn frandom_is_in_range() {
        let mut r = Randy::default();
        for _ in 0..1000 {
            let v = r.game_frandom_ext(10.0);
            assert!((0.0..10.0).contains(&v));
            let w = r.frandom_between_ext(RAN_GAME, -2.0, 3.0);
            assert!((-2.0..3.0).contains(&w));
            let i = r.random_between(RAN_GAME, -3, 3);
            assert!((-3..=3).contains(&i));
        }
    }
}
