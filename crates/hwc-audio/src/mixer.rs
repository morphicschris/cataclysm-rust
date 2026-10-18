//! The low-level mixer: effect voices, streams and the block-by-block mix.
//!
//! This reproduces the original sound library ("soundlow"/"smixer"/"sstream"): up to 32 effect
//! voices addressed by handles, four streaming channels (three for speech, one for music) fed
//! from queues of data records, linear volume/pan/pitch fades counted in mixer blocks, per-voice
//! 9-band EQ, a master EQ, and a single inverse transform per output channel. One call to
//! [`Mixer::process_block`] produces 256 stereo frames at 22 050 Hz (11.6 ms), exactly one
//! iteration of the game's mixer loop.

use std::collections::VecDeque;
use std::sync::Arc;

use crate::codec::{self, Dequantizer, Eq, Frames, Limiter, Quality, Synth, Window, BINS, BLOCKS_PER_SECOND, BLOCK_FRAMES, FLAT_EQ};

/// Handle value meaning "no sound" (the game's `SOUND_DEFAULT` / `SOUND_ERR`).
pub const NO_HANDLE: i32 = -1;
/// Hard limit on effect voices.
pub const MAX_VOICES: usize = 32;
/// Voice count the game starts with.
pub const DEFAULT_VOICES: usize = 16;
/// Lowest voice count the game allows.
pub const MIN_VOICES: usize = 8;
/// Number of streams the game creates (three speech, then music).
pub const NUM_STREAMS: usize = 4;
/// Queue capacity of a stream (the game keeps one slot of its ring of ten free).
pub const STREAM_QUEUE_LEN: usize = 9;
/// Minimum length of any fade, in blocks (the game clamps every requested fade to this).
pub const MIN_FADE_BLOCKS: i32 = 20;

pub const PAN_CENTER: i16 = 0;
pub const PAN_LEFT: i16 = -90;
pub const PAN_RIGHT: i16 = 90;
pub const VOL_MAX: i16 = 255;

/// Priorities used by the game.
pub mod priority {
    pub const STREAM: i32 = 101;
    pub const MAX: i32 = 100;
    pub const HIGH: i32 = 75;
    pub const NORMAL: i32 = 50;
    pub const LOW: i32 = 25;
    pub const MIN: i32 = -1;
}

/// Lifecycle state of a voice or stream channel (values as in the game).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Status {
    Free = 0,
    InUse = 1,
    Playing = 4,
    Stopping = 7,
    /// A looping voice asked to stop: finish the loop pass, then play the tail after the loop.
    LoopEnd = 9,
    Restart = 10,
}

/// One effect patch: a contiguous block-coded payload inside a bank, with absolute positions.
#[derive(Clone)]
pub struct PatchRef {
    pub data: Arc<[u8]>,
    pub start: usize,
    pub end: usize,
    pub loop_start: usize,
    pub loop_end: usize,
    pub bitrate: u16,
    pub flags: u16,
    /// Rate of the source recording; decides how many bins the mixer decodes.
    pub sample_rate: u32,
    pub volume: u8,
    pub pan: i8,
    pub priority: u32,
}

impl PatchRef {
    pub fn block_bytes(&self) -> usize {
        usize::from(self.bitrate / 8).max(1)
    }

    pub fn duration_secs(&self) -> f64 {
        ((self.end - self.start) / self.block_bytes()) as f64 * codec::BLOCK_SECONDS
    }
}

/// Mixer behaviour switches. The defaults reproduce the game.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MixerOptions {
    pub window: Window,
    pub limiter: Limiter,
    /// Swap left and right on output (the game's "reverse stereo" option).
    pub reverse_stereo: bool,
}

/// Automatic quality/voice reduction when the game stalls (the original "panic mode").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AutoMode {
    #[default]
    Off,
    Auto,
}

struct Voice {
    priority: i32,
    status: Status,
    handle: i32,
    mute: bool,
    patch: Option<PatchRef>,
    volume: f32,
    vol_fade: f32,
    vol_target: i16,
    vol_ticks: i16,
    pan: i16,
    pan_target: i16,
    pan_fade: i16,
    pan_ticks: i16,
    pitch: f32,
    pitch_target: f32,
    pitch_fade: f32,
    pitch_ticks: i32,
    heading: i16,
    size: usize,
    looping: bool,
    pos: usize,
    factor_l: f32,
    factor_r: f32,
    exponents: [u8; BINS],
    frames: Frames,
    filter: Eq,
    cardioid: Eq,
    use_cardioid: bool,
}

impl Voice {
    fn new() -> Self {
        Self {
            priority: -1,
            status: Status::Free,
            handle: NO_HANDLE,
            mute: false,
            patch: None,
            volume: 0.0,
            vol_fade: 0.0,
            vol_target: 0,
            vol_ticks: 0,
            pan: 0,
            pan_target: 0,
            pan_fade: 0,
            pan_ticks: 0,
            pitch: 0.0,
            pitch_target: 0.0,
            pitch_fade: 0.0,
            pitch_ticks: 0,
            heading: 0,
            size: 0,
            looping: false,
            pos: 0,
            factor_l: 0.0,
            factor_r: 0.0,
            exponents: [0; BINS],
            frames: Frames::default(),
            filter: FLAT_EQ,
            cardioid: FLAT_EQ,
            use_cardioid: false,
        }
    }
}

/// Left/right gains for a volume (0..255) and pan angle (-179..179). Beyond +-90 degrees the far
/// side's gain goes negative, which the game does not prevent (sounds "behind" are phase-flipped).
pub fn volume_pan_factors(volume: f32, pan: i16) -> (f32, f32) {
    let f = volume * 0.003_921_569;
    let (mut l, mut r) = (f, f);
    if pan < 0 {
        r = f32::from(pan + 90) * f * 0.011_111_111;
    } else if pan > 0 {
        l = f32::from(90 - pan) * f * 0.011_111_111;
    }
    (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0))
}

/// Round half to even, the way the game converts its float volumes back to integers.
pub(crate) fn round_i32(x: f32) -> i32 {
    f64::from(x).round_ties_even() as i32
}

fn fade_blocks(seconds: f32) -> i32 {
    ((seconds * BLOCKS_PER_SECOND) as i32).max(MIN_FADE_BLOCKS)
}

/// Where a stream entry's bytes come from.
#[derive(Clone)]
pub struct StreamData {
    pub bytes: Arc<[u8]>,
    pub start: usize,
    pub len: usize,
}

impl StreamData {
    fn slice(&self) -> &[u8] {
        &self.bytes[self.start..self.start + self.len]
    }
}

/// One record queued on a stream.
#[derive(Clone)]
pub struct StreamEntry {
    /// `None` queues silence of `len` bytes.
    pub data: Option<StreamData>,
    pub len: usize,
    pub looping: bool,
    pub volume: i16,
    pub pan: i16,
    pub channels: u16,
    pub bitrate: u16,
    pub fade_in: f32,
    /// Equaliser applied to mono entries (the speech radio EQ).
    pub eq: Option<Eq>,
    /// Caller's tag (speech event / track number), reported back by [`Mixer::stream_playing_tag`].
    pub tag: i64,
    factor_l: f32,
    factor_r: f32,
}

impl StreamEntry {
    /// An entry playing `data` (a `DATA` payload) with the given level, pan, layout and codec rate.
    pub fn new(data: StreamData, volume: i16, pan: i16, channels: u16, bitrate: u16) -> Self {
        let len = data.len;
        let mut e = Self {
            data: Some(data),
            len,
            looping: false,
            volume,
            pan,
            channels,
            bitrate,
            fade_in: 0.0,
            eq: None,
            tag: -1,
            factor_l: 0.0,
            factor_r: 0.0,
        };
        e.compute_factors();
        e
    }

    /// Silence lasting `seconds` (the game's `SOUND_FLAGS_QUEUESILENCE`).
    pub fn silence(seconds: f32, volume: i16, channels: u16, bitrate: u16) -> Self {
        let len = (seconds * BLOCKS_PER_SECOND * f32::from(bitrate >> 3)) as usize;
        let mut e = Self {
            data: None,
            len,
            looping: false,
            volume,
            pan: 0,
            channels,
            bitrate,
            fade_in: 0.0,
            eq: None,
            tag: -1,
            factor_l: 0.0,
            factor_r: 0.0,
        };
        e.compute_factors();
        e
    }

    fn compute_factors(&mut self) {
        let mut l = f32::from(self.volume) / 255.0;
        let mut r = l;
        if self.pan < 0 {
            r *= f32::from(PAN_RIGHT + self.pan) / f32::from(PAN_RIGHT);
        } else if self.pan > 0 {
            l *= f32::from(PAN_RIGHT - self.pan) / f32::from(PAN_RIGHT);
        }
        self.factor_l = l;
        self.factor_r = r;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamState {
    Idle,
    Starting,
    Playing,
}

struct StreamChannel {
    status: Status,
    volume: f32,
    vol_fade: f32,
    vol_target: i16,
    vol_ticks: i16,
    pan: i16,
    channels: u16,
    bitrate: u16,
    size: usize,
    factor_l: f32,
    factor_r: f32,
    exp_l: [u8; BINS],
    exp_r: [u8; BINS],
    frames_l: Frames,
    frames_r: Frames,
}

struct Stream {
    handle: i32,
    state: StreamState,
    pending: VecDeque<StreamEntry>,
    current: Option<(StreamEntry, usize)>,
    /// Entry index that was playing when an auto-stop was requested.
    chan: StreamChannel,
}

impl Stream {
    fn new(handle: i32) -> Self {
        Self {
            handle,
            state: StreamState::Idle,
            pending: VecDeque::new(),
            current: None,
            chan: StreamChannel {
                status: Status::Free,
                volume: 0.0,
                vol_fade: 0.0,
                vol_target: 0,
                vol_ticks: 0,
                pan: 0,
                channels: 1,
                bitrate: 512,
                size: 128,
                factor_l: 0.0,
                factor_r: 0.0,
                exp_l: [0; BINS],
                exp_r: [0; BINS],
                frames_l: Frames::default(),
                frames_r: Frames::default(),
            },
        }
    }
}

#[derive(Default)]
struct Panic {
    active: bool,
    duration: u32,
    last_frame: u64,
    saved_voices: usize,
}

/// The mixer state. Not thread-safe by itself; share it as `Arc<Mutex<Mixer>>`.
pub struct Mixer {
    opts: MixerOptions,
    deq: Dequantizer,
    voices: Vec<Voice>,
    num_voices: usize,
    used: usize,
    counter: i32,
    streams: Vec<Stream>,
    master_eq: Eq,
    quality: Quality,
    quality_auto: AutoMode,
    voices_auto: AutoMode,
    synth_l: Synth,
    synth_r: Synth,
    ticks: u64,
    render_frames: u64,
    game_running: bool,
    panic: Panic,
    mix_l: Frames,
    mix_r: Frames,
}

impl Default for Mixer {
    fn default() -> Self {
        Self::new(MixerOptions::default())
    }
}

impl Mixer {
    pub fn new(opts: MixerOptions) -> Self {
        let mut counter = 0;
        let streams = (0..NUM_STREAMS)
            .map(|i| {
                let h = (counter << 8) | i as i32;
                counter += 1;
                Stream::new(h)
            })
            .collect();
        Self {
            opts,
            deq: Dequantizer::default(),
            voices: (0..MAX_VOICES).map(|_| Voice::new()).collect(),
            num_voices: DEFAULT_VOICES,
            used: 0,
            counter,
            streams,
            master_eq: FLAT_EQ,
            quality: Quality::Normal,
            quality_auto: AutoMode::Off,
            voices_auto: AutoMode::Off,
            synth_l: Synth::new(opts.window),
            synth_r: Synth::new(opts.window),
            ticks: 0,
            render_frames: 0,
            game_running: false,
            panic: Panic::default(),
            mix_l: Frames::default(),
            mix_r: Frames::default(),
        }
    }

    pub fn options(&self) -> MixerOptions {
        self.opts
    }

    /// Change output options (window changes take effect on new synthesis state).
    pub fn set_options(&mut self, opts: MixerOptions) {
        if opts.window != self.opts.window {
            self.synth_l = Synth::new(opts.window);
            self.synth_r = Synth::new(opts.window);
        }
        self.opts = opts;
    }

    /// Number of blocks mixed so far.
    pub fn blocks_mixed(&self) -> u64 {
        self.ticks
    }

    // --------------------------------------------------------------------------------------------
    // Configuration
    // --------------------------------------------------------------------------------------------

    /// Set the number of effect voices (clamped to 8..=32) and whether stalls may reduce it.
    pub fn set_num_voices(&mut self, n: usize, mode: AutoMode) {
        self.num_voices = n.clamp(MIN_VOICES, MAX_VOICES);
        self.voices_auto = mode;
    }

    pub fn num_voices(&self) -> usize {
        self.num_voices
    }

    /// Set the synthesis quality; `Auto` starts in normal quality and drops to low on stalls.
    pub fn set_quality(&mut self, quality: Quality, mode: AutoMode) {
        self.quality = quality;
        self.quality_auto = if quality == Quality::Low { AutoMode::Off } else { mode };
    }

    pub fn quality(&self) -> Quality {
        self.quality
    }

    /// Eight-band master EQ (the ninth band, above 11 kHz, is never audible).
    pub fn set_master_eq(&mut self, eq: &[f32]) {
        for (d, s) in self.master_eq.iter_mut().zip(eq) {
            *d = *s;
        }
    }

    /// Tell the stall detector whether a game is running and a frame has been rendered.
    pub fn note_render_frame(&mut self, game_running: bool) {
        self.game_running = game_running;
        self.render_frames += 1;
    }

    // --------------------------------------------------------------------------------------------
    // Effect voices
    // --------------------------------------------------------------------------------------------

    fn voice_of(&self, handle: i32) -> Option<usize> {
        if handle < 0 {
            return None;
        }
        let ch = (handle & 0xff) as usize;
        (ch < MAX_VOICES && self.voices[ch].handle == handle).then_some(ch)
    }

    fn release(&mut self, ch: usize) {
        let v = &mut self.voices[ch];
        v.handle = NO_HANDLE;
        v.patch = None;
        v.status = Status::Free;
        v.priority = -1;
        self.used = self.used.saturating_sub(1);
    }

    fn take_free(&mut self, priority: i32) -> Option<usize> {
        let ch = self.voices[..self.num_voices].iter().position(|v| v.status == Status::Free)?;
        self.used += 1;
        self.voices[ch].status = Status::InUse;
        self.voices[ch].priority = priority;
        Some(ch)
    }

    /// Find a voice for a new sound. Two voices are held in reserve: once they are needed, a
    /// quieter lower-priority voice is faded out first (it is not reusable until the fade ends,
    /// so the reserve is what the new sound gets). Priorities above 100 bypass the reserve.
    fn get_voice(&mut self, priority: i32) -> Option<usize> {
        if self.used + 2 < self.num_voices || priority > priority::MAX {
            if let Some(ch) = self.take_free(priority) {
                return Some(ch);
            }
        }
        let mut best_prio = priority.max(priority::LOW);
        let mut best_vol = 255.0f32;
        let mut best_ticks = 255i16;
        let mut victim = None;
        for (i, v) in self.voices[..self.num_voices].iter().enumerate() {
            if v.status == Status::Playing && v.priority < best_prio && v.volume <= best_vol && v.vol_ticks <= best_ticks {
                best_vol = v.volume;
                best_prio = v.priority;
                best_ticks = v.vol_ticks;
                victim = Some(i);
            }
        }
        let victim = victim?;
        let h = self.voices[victim].handle;
        self.stop(h, 0.0);
        self.take_free(priority)
    }

    /// Start a patch. `volume` -1 means full; `freq` -1 means unshifted. Returns a handle or
    /// [`NO_HANDLE`]. Looping patches started with `fade_in` ramp up over 20 blocks; every
    /// sound pans in from the centre over 20 blocks, as in the game.
    #[allow(clippy::too_many_arguments)]
    pub fn play(
        &mut self,
        patch: &PatchRef,
        eq: Option<&Eq>,
        freq: f32,
        pan: i16,
        priority: i32,
        volume: i16,
        start_at_loop: bool,
        fade_in: bool,
        mute: bool,
    ) -> i32 {
        let volume = if volume == -1 {
            VOL_MAX
        } else if !(1..=255).contains(&volume) {
            return NO_HANDLE;
        } else {
            volume
        };
        let freq = if freq == -1.0 { 1.0 } else { freq };
        let Some(ch) = self.get_voice(priority) else { return NO_HANDLE };
        // Handles are (play counter << 8) | voice; keep them positive forever.
        let handle = ((self.counter & 0x7f_ffff) << 8) | ch as i32;
        self.counter = self.counter.wrapping_add(1);

        let v = &mut self.voices[ch];
        let status = v.status;
        *v = Voice::new();
        v.status = status;
        v.priority = priority::NORMAL;
        v.factor_l = 1.0;
        v.factor_r = 1.0;
        v.filter = eq.copied().unwrap_or(FLAT_EQ);
        v.patch = Some(patch.clone());
        v.handle = handle;
        v.looping = patch.flags != 0;
        v.pitch = freq;
        v.mute = mute;
        if !fade_in || !v.looping {
            v.volume = f32::from(volume);
        }
        self.set_volume(handle, volume, 0.0);
        self.set_pan(handle, pan, 0.0);
        let v = &mut self.voices[ch];
        (v.factor_l, v.factor_r) = volume_pan_factors(v.volume, v.pan);
        if freq != v.pitch_target {
            v.pitch_target = freq;
            v.pitch_ticks = 0;
            v.pitch_fade = 0.0;
        }
        v.pos = if start_at_loop { patch.loop_start } else { patch.start };
        // Sources above 22 049 Hz (every shipped patch) get the full 128 bins, slower ones 64.
        v.size = if patch.sample_rate > 22_049 { 128 } else { 64 };
        v.status = Status::Playing;
        handle
    }

    /// Stop a voice. Looping voices with a tail end their loop and play the tail when `fade` is
    /// positive; everything else fades out over at least 20 blocks.
    pub fn stop(&mut self, handle: i32, fade: f32) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        let v = &mut self.voices[ch];
        if v.status == Status::Free {
            return false;
        }
        if let Some(p) = &v.patch {
            if v.looping && p.loop_end < p.end && fade > 0.0 {
                v.status = Status::LoopEnd;
                return true;
            }
        }
        let ticks = fade_blocks(fade);
        v.vol_ticks = ticks as i16;
        v.status = Status::Stopping;
        v.vol_target = -1;
        v.vol_fade = (-1.0 - v.volume) / ticks as f32;
        if v.vol_fade == 0.0 {
            v.vol_fade = if -1.0 < v.volume { -0.01 } else { 0.01 };
        }
        true
    }

    /// Stop every voice (and optionally every stream) with a fade.
    pub fn stop_all(&mut self, fade: f32, streams: bool) {
        for i in 0..self.num_voices {
            let h = self.voices[i].handle;
            if h >= 0 {
                self.stop(h, fade);
            }
        }
        if streams {
            for i in 0..self.streams.len() {
                let h = self.streams[i].handle;
                self.stream_volume(h, -1, fade);
            }
        }
    }

    /// Fade a voice to `volume` (1..255; 0 or less stops it with a one-second fade).
    pub fn set_volume(&mut self, handle: i32, volume: i16, fade: f32) -> bool {
        let volume = if volume > 255 {
            255
        } else if volume < 1 {
            return self.stop(handle, 1.0);
        } else {
            volume
        };
        let Some(ch) = self.voice_of(handle) else { return false };
        let v = &mut self.voices[ch];
        if volume != v.vol_target {
            if i32::from(volume) == round_i32(v.volume) {
                v.vol_target = volume;
                v.vol_ticks = 0;
                v.vol_fade = 0.0;
                return true;
            }
            let ticks = fade_blocks(fade);
            v.vol_ticks = ticks as i16;
            v.vol_target = volume;
            v.vol_fade = (f32::from(volume) - v.volume) / ticks as f32;
            if v.vol_fade == 0.0 {
                v.vol_fade = if f32::from(volume) < v.volume { -0.01 } else { 0.01 };
            }
        }
        true
    }

    /// Sweep a voice's pan angle (in whole degrees per block, as the game does).
    pub fn set_pan(&mut self, handle: i32, pan: i16, fade: f32) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        let v = &mut self.voices[ch];
        if pan == v.pan {
            v.pan_target = pan;
            v.pan_ticks = 0;
            v.pan_fade = 0;
            return true;
        }
        let ticks = fade_blocks(fade);
        v.pan_ticks = ticks as i16;
        v.pan_target = pan;
        v.pan_fade = ((i32::from(pan) - i32::from(v.pan)) / ticks).clamp(-180, 180) as i16;
        if v.pan_fade == 0 {
            v.pan_fade = if pan < v.pan { -1 } else { 1 };
        }
        true
    }

    /// Glide a voice's pitch factor to `freq` over 20 blocks.
    pub fn set_frequency(&mut self, handle: i32, freq: f32) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        let v = &mut self.voices[ch];
        if freq != v.pitch_target {
            if freq == v.pitch {
                v.pitch_target = freq;
                v.pitch_ticks = 0;
                v.pitch_fade = 0.0;
            } else {
                v.pitch_target = freq;
                v.pitch_ticks = MIN_FADE_BLOCKS;
                v.pitch_fade = (freq - v.pitch) * 0.05;
            }
        }
        true
    }

    /// Replace a voice's 9-band EQ.
    pub fn set_eq(&mut self, handle: i32, eq: &Eq) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        self.voices[ch].filter = *eq;
        true
    }

    /// The game's fake doppler: a second EQ shaped by the angle between the ship's heading and
    /// the camera. `cardioid` is indexed by heading in degrees (see [`crate::tables::Cardioid`]).
    #[allow(clippy::too_many_arguments)]
    pub fn set_ship_heading(
        &mut self,
        handle: i32,
        heading: i16,
        high_band: usize,
        low_band: usize,
        vel_factor: f32,
        ship_factor: f32,
        cardioid: &crate::tables::Cardioid,
    ) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        let v = &mut self.voices[ch];
        if heading != v.heading {
            v.heading = heading;
            let front = ((cardioid.at(i32::from(heading)) - 1.0) * vel_factor * ship_factor + 1.0).clamp(0.4, 1.0);
            let back = ((cardioid.at(180 - i32::from(heading)) - 1.0) * vel_factor * ship_factor + 1.0).clamp(0.4, 1.0);
            let (lo, hi) = (low_band.min(9), high_band.min(9));
            for b in v.cardioid[..lo].iter_mut() {
                *b = front;
            }
            for i in lo..hi {
                v.cardioid[i] = (i - lo) as f32 * ((back - front) / (hi - lo) as f32) + front;
            }
            for b in v.cardioid[hi.max(lo)..].iter_mut() {
                *b = back;
            }
        }
        v.use_cardioid = true;
        true
    }

    /// Restart a voice from the beginning on the next block.
    pub fn restart(&mut self, handle: i32) -> bool {
        let Some(ch) = self.voice_of(handle) else { return false };
        self.voices[ch].status = Status::Restart;
        true
    }

    /// Whether a handle no longer refers to a sounding voice.
    pub fn is_over(&self, handle: i32) -> bool {
        match self.voice_of(handle) {
            Some(ch) => self.voices[ch].status <= Status::InUse,
            None => true,
        }
    }

    /// Current (volume, pan) of a voice, for inspection.
    pub fn voice_state(&self, handle: i32) -> Option<(f32, i16)> {
        self.voice_of(handle).map(|ch| (self.voices[ch].volume, self.voices[ch].pan))
    }

    /// Number of voices currently allocated.
    pub fn voices_in_use(&self) -> usize {
        self.used
    }

    /// Stop the lowest-priority voices until no more than the voice limit are allocated.
    pub fn panic_trim(&mut self) {
        while self.used > self.num_voices {
            let mut best_vol = 255.0f32;
            let mut best_prio = priority::LOW;
            let mut best_ticks = 255i16;
            let mut victim = None;
            for (i, v) in self.voices.iter().enumerate() {
                if v.status == Status::Playing && v.priority < best_prio && v.volume <= best_vol && v.vol_ticks <= best_ticks {
                    best_vol = v.volume;
                    best_prio = v.priority;
                    best_ticks = v.vol_ticks;
                    victim = Some(i);
                }
            }
            let Some(victim) = victim else { return };
            let h = self.voices[victim].handle;
            // A stopping voice is no longer `Playing`, so the next pass picks another one.
            self.stop(h, 0.0);
        }
    }

    // --------------------------------------------------------------------------------------------
    // Streams
    // --------------------------------------------------------------------------------------------

    /// Handle of stream `index` (0..4: three speech streams, then music).
    pub fn stream_handle(&self, index: usize) -> i32 {
        self.streams[index].handle
    }

    /// Create an extra stream (beyond the game's four) and return its handle.
    pub fn add_stream(&mut self) -> i32 {
        let h = ((self.counter & 0x7f_ffff) << 8) | self.streams.len() as i32;
        self.counter = self.counter.wrapping_add(1);
        self.streams.push(Stream::new(h));
        h
    }

    fn stream_of(&self, handle: i32) -> Option<usize> {
        if handle < 0 {
            return None;
        }
        let i = (handle & 0xff) as usize;
        (i < self.streams.len() && self.streams[i].handle == handle).then_some(i)
    }

    /// Queue an entry. With `wait` an idle stream does not start until a later entry is queued
    /// without it (speech queues all but the last fragment this way). Returns false when the
    /// queue is full.
    pub fn stream_queue(&mut self, handle: i32, mut entry: StreamEntry, wait: bool) -> bool {
        let Some(i) = self.stream_of(handle) else { return false };
        let s = &mut self.streams[i];
        if s.pending.len() >= STREAM_QUEUE_LEN {
            return false;
        }
        entry.compute_factors();
        s.pending.push_back(entry);
        if s.state == StreamState::Idle && !wait {
            s.state = StreamState::Starting;
        }
        true
    }

    /// Fade a stream's level. `-1` fades out, then stops it and drops anything still queued.
    pub fn stream_volume(&mut self, handle: i32, volume: i16, fade: f32) -> bool {
        let Some(i) = self.stream_of(handle) else { return false };
        let mut autostop = false;
        let volume = if volume > 255 {
            255
        } else if volume == -1 {
            autostop = true;
            0
        } else {
            volume.max(0)
        };
        let ticks = fade_blocks(fade);
        let s = &mut self.streams[i];
        let c = &mut s.chan;
        if volume != c.vol_target || autostop {
            if i32::from(volume) == round_i32(c.volume) {
                c.vol_target = volume;
                c.vol_ticks = 1;
                c.vol_fade = 0.0;
            } else {
                c.vol_ticks = ticks as i16;
                c.vol_target = volume;
                c.vol_fade = (f32::from(volume) - c.volume) / ticks as f32;
                if c.vol_fade == 0.0 {
                    c.vol_fade = if f32::from(volume) < c.volume { -0.01 } else { 0.01 };
                }
            }
            if autostop {
                c.status = Status::Stopping;
                s.pending.clear();
            }
        }
        true
    }

    /// Whether a stream has nothing playing.
    pub fn stream_over(&self, handle: i32) -> bool {
        match self.stream_of(handle) {
            Some(i) => self.streams[i].chan.status <= Status::InUse && self.streams[i].state != StreamState::Starting,
            None => true,
        }
    }

    /// Whether a stream's level is changing.
    pub fn stream_fading(&self, handle: i32) -> bool {
        self.stream_of(handle).is_some_and(|i| self.streams[i].chan.vol_fade != 0.0)
    }

    /// A stream's current level, truncated like the game's.
    pub fn stream_get_volume(&self, handle: i32) -> Option<i16> {
        self.stream_of(handle).map(|i| self.streams[i].chan.volume as i16)
    }

    /// Tag of the entry currently being played on a stream.
    pub fn stream_playing_tag(&self, handle: i32) -> Option<i64> {
        let i = self.stream_of(handle)?;
        self.streams[i].current.as_ref().map(|(e, _)| e.tag)
    }

    /// Entries waiting on a stream (not counting the one playing).
    pub fn stream_queued(&self, handle: i32) -> usize {
        self.stream_of(handle).map_or(0, |i| self.streams[i].pending.len())
    }

    /// The streamer's work for one block: start streams that have been asked to start.
    fn service_streams(&mut self) {
        for s in &mut self.streams {
            if s.state != StreamState::Starting {
                continue;
            }
            let Some(entry) = s.pending.pop_front() else {
                s.state = StreamState::Idle;
                continue;
            };
            let c = &mut s.chan;
            c.factor_l = entry.factor_l;
            c.factor_r = entry.factor_r;
            c.vol_target = entry.volume;
            if entry.fade_in == 0.0 {
                c.vol_ticks = 0;
                c.vol_fade = 0.0;
                c.volume = f32::from(entry.volume);
            } else {
                c.volume = 0.0;
                let ticks = ((entry.fade_in * BLOCKS_PER_SECOND) as i32).max(1);
                c.vol_ticks = ticks as i16;
                c.vol_fade = f32::from(c.vol_target) / ticks as f32;
                if c.vol_fade == 0.0 {
                    c.vol_fade = if f32::from(c.vol_target) < 0.0 { -0.01 } else { 0.01 };
                }
            }
            c.pan = entry.pan;
            c.channels = entry.channels;
            c.bitrate = entry.bitrate;
            c.status = Status::Playing;
            s.current = Some((entry, 0));
            s.state = StreamState::Playing;
        }
    }

    /// Move a stream past a finished entry: loop it, continue with the next, or run dry.
    fn advance_entry(s: &mut Stream) {
        let Some((entry, pos)) = &mut s.current else { return };
        if *pos < entry.len {
            return;
        }
        if entry.looping {
            *pos = 0;
            return;
        }
        match s.pending.pop_front() {
            Some(next) => s.current = Some((next, 0)),
            None => {
                s.current = None;
                s.state = StreamState::Idle;
                s.chan.status = Status::Stopping;
            }
        }
    }

    fn mix_streams(&mut self) {
        let size = self.quality.bins();
        for s in &mut self.streams {
            if s.chan.status < Status::Playing {
                continue;
            }
            let Some((entry, pos)) = &mut s.current else {
                if s.chan.status == Status::Stopping {
                    s.chan.status = Status::InUse;
                    s.state = if s.pending.is_empty() { StreamState::Idle } else { StreamState::Starting };
                }
                continue;
            };
            let c = &mut s.chan;
            let block = usize::from(c.bitrate >> 3).max(1);
            let bins = c.size.min(size);
            let read = |entry: &StreamEntry, at: usize| -> Vec<u8> {
                match &entry.data {
                    Some(d) => {
                        let src = d.slice();
                        let end = (at + block).min(src.len());
                        src.get(at..end).map(<[u8]>::to_vec).unwrap_or_default()
                    }
                    None => Vec::new(),
                }
            };
            let bytes = read(entry, *pos);
            self.deq.block(&bytes, u32::from(c.bitrate), bins, &mut c.exp_l, &mut c.frames_l);
            *pos += block;

            if c.vol_ticks != 0 {
                c.vol_ticks -= 1;
                c.volume += c.vol_fade;
                if c.vol_ticks < 1 {
                    c.vol_fade = 0.0;
                    c.volume = f32::from(c.vol_target);
                    c.vol_ticks = 0;
                    c.vol_target = -1;
                }
                if round_i32(c.volume) > 255 {
                    c.volume = 255.0;
                }
                if round_i32(c.volume) < 0 {
                    c.volume = 0.0;
                }
                if c.status == Status::Stopping && c.volume == 0.0 {
                    s.current = None;
                    s.state = if s.pending.is_empty() { StreamState::Idle } else { StreamState::Starting };
                    c.status = Status::Free;
                    continue;
                }
                (c.factor_l, c.factor_r) = volume_pan_factors(c.volume, c.pan);
            }

            if c.channels < 2 {
                if let Some(eq) = &entry.eq {
                    codec::equalize(&mut c.frames_l.primary, eq, size);
                    codec::equalize(&mut c.frames_l.secondary, eq, size);
                }
            }
            codec::mix(&mut self.mix_l.primary, &c.frames_l.primary, c.factor_l, size);
            codec::mix(&mut self.mix_l.secondary, &c.frames_l.secondary, c.factor_l, size);
            if c.channels == 2 {
                let bytes = read(entry, *pos);
                self.deq.block(&bytes, u32::from(c.bitrate), bins, &mut c.exp_r, &mut c.frames_r);
                *pos += block;
                codec::mix(&mut self.mix_r.primary, &c.frames_r.primary, c.factor_r, size);
                codec::mix(&mut self.mix_r.secondary, &c.frames_r.secondary, c.factor_r, size);
            } else {
                codec::mix(&mut self.mix_r.primary, &c.frames_l.primary, c.factor_r, size);
                codec::mix(&mut self.mix_r.secondary, &c.frames_l.secondary, c.factor_r, size);
            }
            Self::advance_entry(s);
        }
    }

    fn mix_voices(&mut self) {
        let size = self.quality.bins();
        for ch in 0..self.num_voices {
            if self.voices[ch].status < Status::Playing {
                continue;
            }
            let Some(patch) = self.voices[ch].patch.clone() else {
                self.release(ch);
                continue;
            };
            if !self.voices[ch].looping && self.voices[ch].pos >= patch.end {
                self.release(ch);
                continue;
            }
            let v = &mut self.voices[ch];
            match v.status {
                Status::LoopEnd => {
                    v.status = Status::Playing;
                    v.pos = patch.loop_end;
                    v.looping = false;
                }
                Status::Restart => {
                    v.status = Status::Playing;
                    v.pos = patch.start;
                }
                _ => {}
            }
            let block = patch.block_bytes();
            let end = (v.pos + block).min(patch.data.len());
            let bytes = patch.data.get(v.pos..end).unwrap_or(&[]);
            self.deq.block(bytes, u32::from(patch.bitrate), v.size.min(size), &mut v.exponents, &mut v.frames);
            v.pos += block;
            if v.looping && v.pos >= patch.loop_end {
                v.pos = patch.loop_start;
            }

            if v.vol_ticks != 0 {
                v.vol_ticks -= 1;
                v.volume += v.vol_fade;
                if v.vol_ticks < 1 {
                    v.volume = f32::from(v.vol_target);
                    v.vol_fade = 0.0;
                    v.vol_ticks = 0;
                    v.vol_target = -1;
                }
                if round_i32(v.volume) > 255 {
                    v.volume = 255.0;
                }
                if round_i32(v.volume) < 0 {
                    v.volume = 0.0;
                }
                if v.status == Status::Stopping && round_i32(v.volume) == 0 {
                    self.release(ch);
                    continue;
                }
                (v.factor_l, v.factor_r) = volume_pan_factors(v.volume, v.pan);
            }
            if v.pan_ticks != 0 {
                v.pan += v.pan_fade;
                v.pan_ticks -= 1;
                if v.pan_ticks < 1 {
                    v.pan_fade = 0;
                    v.pan = v.pan_target;
                    v.pan_ticks = 0;
                }
                if v.pan > 179 {
                    v.pan -= 358;
                }
                if v.pan < -179 {
                    v.pan += 358;
                }
                (v.factor_l, v.factor_r) = volume_pan_factors(v.volume, v.pan);
            }
            if v.pitch_ticks != 0 {
                v.pitch_ticks -= 1;
                v.pitch += v.pitch_fade;
                if v.pitch_ticks < 1 {
                    v.pitch_fade = 0.0;
                    v.pitch = v.pitch_target;
                    v.pitch_ticks = 0;
                }
            }
            codec::pitch_shift(&mut v.frames.primary, v.pitch, size);
            codec::pitch_shift(&mut v.frames.secondary, v.pitch, size);
            if v.use_cardioid {
                codec::equalize(&mut v.frames.primary, &v.cardioid, size);
                codec::equalize(&mut v.frames.secondary, &v.cardioid, size);
            }
            codec::equalize(&mut v.frames.primary, &v.filter, size);
            codec::equalize(&mut v.frames.secondary, &v.filter, size);
            if !v.mute {
                codec::mix(&mut self.mix_l.primary, &v.frames.primary, v.factor_l, size);
                codec::mix(&mut self.mix_l.secondary, &v.frames.secondary, v.factor_l, size);
                codec::mix(&mut self.mix_r.primary, &v.frames.primary, v.factor_r, size);
                codec::mix(&mut self.mix_r.secondary, &v.frames.secondary, v.factor_r, size);
            }
        }
    }

    /// The stall detector, run every 22 blocks (about a quarter second).
    fn check_panic(&mut self) {
        if !self.game_running || !self.ticks.is_multiple_of(22) {
            return;
        }
        let stalled = self.panic.last_frame != 0 && self.panic.last_frame == self.render_frames;
        if stalled {
            if !self.panic.active {
                if self.quality_auto == AutoMode::Auto {
                    self.quality = Quality::Low;
                }
                if self.voices_auto == AutoMode::Auto {
                    self.num_voices = MIN_VOICES;
                    self.panic_trim();
                }
                self.panic.active = true;
            }
            self.panic.duration = 1;
        } else if self.panic.duration >= 16 {
            if self.panic.active {
                if self.quality_auto == AutoMode::Auto {
                    self.quality = Quality::Normal;
                }
                if self.voices_auto == AutoMode::Auto && self.panic.saved_voices > 0 {
                    self.num_voices = self.panic.saved_voices;
                }
                self.panic.active = false;
            }
            self.panic.duration = 0;
        } else {
            self.panic.duration += 1;
        }
        if !self.panic.active {
            self.panic.saved_voices = self.num_voices;
        }
        self.panic.last_frame = self.render_frames;
    }

    /// Mix one block: 256 interleaved stereo frames of 16-bit audio at 22 050 Hz.
    pub fn process_block(&mut self, out: &mut [i16; 2 * BLOCK_FRAMES]) {
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        self.process_block_f32(&mut f);
        for i in 0..BLOCK_FRAMES {
            out[2 * i] = codec::to_i16(f[0][i], self.opts.limiter);
            out[2 * i + 1] = codec::to_i16(f[1][i], self.opts.limiter);
        }
    }

    /// Mix one block into floating-point left/right buffers on a 16-bit scale (before limiting).
    pub fn process_block_f32(&mut self, out: &mut [[f32; BLOCK_FRAMES]; 2]) {
        self.check_panic();
        self.mix_l = Frames::default();
        self.mix_r = Frames::default();
        self.service_streams();
        self.mix_streams();
        self.mix_voices();
        let size = self.quality.bins();
        for frame in [&mut self.mix_l.primary, &mut self.mix_l.secondary, &mut self.mix_r.primary, &mut self.mix_r.secondary] {
            codec::equalize(frame, &self.master_eq, size);
        }
        let (mut l, mut r) = ([0.0f32; BLOCK_FRAMES], [0.0f32; BLOCK_FRAMES]);
        self.synth_l.block(&self.mix_l.primary, &self.mix_l.secondary, self.quality, &mut l);
        self.synth_r.block(&self.mix_r.primary, &self.mix_r.secondary, self.quality, &mut r);
        if self.opts.reverse_stereo {
            std::mem::swap(&mut l, &mut r);
        }
        out[0] = l;
        out[1] = r;
        self.ticks += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn silent_patch(blocks: usize, looping: bool) -> PatchRef {
        // Every block is "reset exponents, nothing else": decodes to silence.
        let mut data = vec![0u8; blocks * 64];
        for b in data.chunks_mut(64) {
            b[0] = 1;
        }
        let len = data.len();
        PatchRef {
            data: data.into(),
            start: 0,
            end: len,
            loop_start: 0,
            loop_end: len,
            bitrate: 512,
            flags: u16::from(looping),
            sample_rate: 22_050,
            volume: 255,
            pan: 0,
            priority: 50,
        }
    }

    #[test]
    fn pan_factors_match_game_formula() {
        assert_eq!(volume_pan_factors(255.0, 0), (1.0, 1.0));
        let (l, r) = volume_pan_factors(255.0, 45);
        assert!((l - 0.5).abs() < 1e-6 && (r - 1.0).abs() < 1e-6);
        let (l, r) = volume_pan_factors(255.0, -90);
        assert!((l - 1.0).abs() < 1e-6 && r.abs() < 1e-6);
        // Behind: the far side goes negative.
        let (l, _) = volume_pan_factors(255.0, 179);
        assert!(l < -0.9);
    }

    #[test]
    fn one_shot_releases_after_its_blocks() {
        let mut m = Mixer::default();
        let p = silent_patch(5, false);
        let h = m.play(&p, None, -1.0, 0, 50, 200, false, true, false);
        assert!(h >= 0);
        let mut out = [0i16; 512];
        for _ in 0..5 {
            m.process_block(&mut out);
            assert!(!m.is_over(h));
        }
        m.process_block(&mut out);
        assert!(m.is_over(h));
        assert_eq!(m.voices_in_use(), 0);
    }

    #[test]
    fn fades_take_at_least_twenty_blocks() {
        let mut m = Mixer::default();
        let p = silent_patch(4, true);
        let h = m.play(&p, None, -1.0, 0, 50, 255, false, false, false);
        m.stop(h, 0.0);
        let mut out = [0i16; 512];
        let mut n = 0;
        while !m.is_over(h) {
            m.process_block(&mut out);
            n += 1;
            assert!(n < 100);
        }
        // Volume -1 is reached after 20 blocks; it rounds to 0 one block earlier.
        assert_eq!(n, 20);
    }

    #[test]
    fn looping_fade_in_and_pan_sweep() {
        let mut m = Mixer::default();
        let p = silent_patch(4, true);
        let h = m.play(&p, None, -1.0, 45, 50, 200, false, true, false);
        assert_eq!(m.voice_state(h), Some((0.0, 0)));
        let mut out = [0i16; 512];
        m.process_block(&mut out);
        let (v, pan) = m.voice_state(h).unwrap();
        assert!((v - 10.0).abs() < 1e-4, "{v}");
        assert_eq!(pan, 2); // 45 / 20 degrees per block, integer
        for _ in 0..19 {
            m.process_block(&mut out);
        }
        assert_eq!(m.voice_state(h), Some((200.0, 45)));
    }

    #[test]
    fn reserve_voices_and_stealing() {
        let mut m = Mixer::default();
        m.set_num_voices(8, AutoMode::Off);
        let p = silent_patch(400, true);
        let low: Vec<i32> = (0..6).map(|_| m.play(&p, None, -1.0, 0, 10, 100, false, false, false)).collect();
        assert!(low.iter().all(|&h| h >= 0));
        assert_eq!(m.voices_in_use(), 6);
        // The 7th sound must steal: one low-priority voice starts fading, the reserve is used.
        let h = m.play(&p, None, -1.0, 0, 60, 100, false, false, false);
        assert!(h >= 0);
        // Priority is reset to 50 once playing, so with no lower voice left a further one fails...
        let mut ok = 0;
        for _ in 0..4 {
            if m.play(&p, None, -1.0, 0, 60, 100, false, false, false) >= 0 {
                ok += 1;
            }
        }
        assert_eq!(m.voices_in_use(), 8);
        assert!(ok <= 1);
        // ...but a priority above 100 bypasses the reserve only if a voice is free.
        assert!(m.play(&p, None, -1.0, 0, 101, 100, false, false, false) < 0);
    }

    #[test]
    fn stream_queue_waits_and_autostops() {
        let mut m = Mixer::default();
        let h = m.stream_handle(0);
        let data: Arc<[u8]> = vec![1u8; 64 * 10].into();
        let e = StreamEntry::new(StreamData { bytes: data.clone(), start: 0, len: 640 }, 200, 0, 1, 512);
        assert!(m.stream_queue(h, e.clone(), true));
        let mut out = [0i16; 512];
        m.process_block(&mut out);
        assert!(m.stream_over(h));
        assert!(m.stream_queue(h, e.clone(), false));
        assert!(!m.stream_over(h));
        // Two 10-block entries play back to back, then the stream stops.
        for _ in 0..20 {
            m.process_block(&mut out);
            assert!(!m.stream_over(h));
        }
        m.process_block(&mut out);
        assert!(m.stream_over(h));

        let mut looping = e.clone();
        looping.looping = true;
        m.stream_queue(h, looping, false);
        for _ in 0..50 {
            m.process_block(&mut out);
        }
        assert!(!m.stream_over(h));
        assert_eq!(m.stream_get_volume(h), Some(200));
        m.stream_volume(h, -1, 0.5);
        let mut n = 0;
        while !m.stream_over(h) {
            m.process_block(&mut out);
            n += 1;
            assert!(n < 200);
        }
        assert_eq!(n, 43); // 0.5 s = 43 blocks
    }
}
