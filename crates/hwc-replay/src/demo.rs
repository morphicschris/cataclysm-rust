//! Demo recordings (`.dem`): the original's keyboard/mouse recorder (`Demo.c`).
//!
//! A demo is a capture of the *user interface* input, not of game commands: the recorder
//! writes the state of every random-number stream at the start, a block of user options, and
//! then, for every pass of the task scheduler, the number of 240 Hz task ticks that pass
//! covered followed by a snapshot of the mouse and the non-idle keys taken just before the UI
//! regions are processed. Playback overwrites the live input with the snapshots and the tick
//! counts, so the whole game (front end included) re-runs deterministically, provided every
//! piece of UI code behaves exactly as in the recording.
//!
//! # Layout (all little-endian)
//!
//! ```text
//! header        ident "BaNng\0" [6], version u16 (7), startTime f32,
//!               initialStateSize i32, then N random streams of (x, y, z, c, n) u32
//!               N = 27 in Cataclysm (header 0x22c bytes), 25 in Homeworld (0x204)
//! options       initialStateSize bytes of (type u32, value) entries, see [`OptionValue`]
//! input         one snapshot written when recording starts
//! then repeat   ticks i32                                  (scheduler pass, demNumberTicksSave)
//!               snapshot: time f32, mouseX i16, mouseY i16, frameTicks u32,
//!                         mouseButtons u8, taskNumber u8, pad u8, nKeys u8,
//!                         nKeys x (key index u8, key state u8)   (demStateSave)
//! ```
//!
//! The region task is once-per-frame, so ticks and snapshots strictly alternate; a file may
//! end after a ticks word. The retail builds carry no checksums in the snapshots (the
//! `DEM_CHECKSUM` block is compiled out), and the pad byte is uninitialised stack memory,
//! kept verbatim for a byte-exact round trip.
//!
//! The shipped `demos\abc640x480.dem` is a Homeworld (25-stream) file; Cataclysm's own player
//! would mis-read it. Retail Cataclysm also has no way to start the recorder or the player:
//! the `/demoRecord` and `/demoPlay` switches were removed and nothing else sets the flags.

use hwc_math::random::RanStream;

use crate::bytes::{put_f32, put_u16, put_u32, FormatError, Reader};

/// File identifier.
pub const IDENT: [u8; 6] = *b"BaNng\0";
/// The only version either game writes or accepts.
pub const VERSION: u16 = 7;
/// Task scheduler rate: the unit of [`DemoEvent::Ticks`].
pub const TASK_FREQUENCY: f32 = 240.0;
/// Random streams saved by Homeworld.
pub const STREAMS_HOMEWORLD: usize = 25;
/// Random streams saved by Cataclysm.
pub const STREAMS_CATACLYSM: usize = 27;
/// Size of a snapshot without its key list.
pub const SNAPSHOT_FIXED_SIZE: usize = 16;

/// Which game wrote a demo, told apart by the number of saved random streams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flavour {
    Homeworld,
    Cataclysm,
}

impl Flavour {
    pub fn stream_count(self) -> usize {
        match self {
            Flavour::Homeworld => STREAMS_HOMEWORLD,
            Flavour::Cataclysm => STREAMS_CATACLYSM,
        }
    }

    /// Names of the option-table entries in the order the game saves them.
    pub fn option_names(self) -> &'static [&'static str] {
        match self {
            // Retail Homeworld, as found in the shipped demo.
            Flavour::Homeworld => &[
                "smInstantTransition",
                "taskTimeElapsed",
                "determCompPlayer",
                "CAMERA_MOUSE_SENS",
                "batGlobalFrequencyModifier",
            ],
            // cataclysm.exe's table at 0x8b76a0.
            Flavour::Cataclysm => &[
                "smInstantTransition",
                "taskTimeElapsed",
                "determCompPlayer",
                "noDefaultComputerPlayer",
                "CAMERA_MOUSE_SENS",
                "real32 at 0x74674c",
            ],
        }
    }
}

/// One entry of the option block. The type word selects the size of the value.
#[derive(Debug, Clone, PartialEq)]
pub enum OptionValue {
    /// Type 0.
    Dword(u32),
    /// Type 1 (stored as a dword).
    Bool(u32),
    /// Type 2.
    Real32(f32),
    /// Type 3.
    Byte(u8),
    /// Type 4.
    Bool8(u8),
    /// Type `0x100 + n`: a string of `n` bytes including its terminator.
    String(Vec<u8>),
    /// Type `0x10000 + n`: a structure of `n` bytes.
    Struct(Vec<u8>),
}

const OPT_STRING: u32 = 0x100;
const OPT_STRUCT: u32 = 0x1_0000;

impl OptionValue {
    fn type_code(&self) -> u32 {
        match self {
            OptionValue::Dword(_) => 0,
            OptionValue::Bool(_) => 1,
            OptionValue::Real32(_) => 2,
            OptionValue::Byte(_) => 3,
            OptionValue::Bool8(_) => 4,
            OptionValue::String(s) => OPT_STRING + s.len() as u32,
            OptionValue::Struct(s) => OPT_STRUCT + s.len() as u32,
        }
    }

    /// Parses an option block; fails unless the entries fill it exactly.
    pub fn parse_block(block: &[u8]) -> Result<Vec<OptionValue>, FormatError> {
        let mut r = Reader::new(block);
        let mut out = Vec::new();
        while !r.is_empty() {
            let at = r.pos();
            let ty = r.u32("option type")?;
            let v = match ty {
                0 => OptionValue::Dword(r.u32("option value")?),
                1 => OptionValue::Bool(r.u32("option value")?),
                2 => OptionValue::Real32(r.f32("option value")?),
                3 => OptionValue::Byte(r.u8("option value")?),
                4 => OptionValue::Bool8(r.u8("option value")?),
                t if t >= OPT_STRUCT => OptionValue::Struct(r.bytes((t - OPT_STRUCT) as usize, "option struct")?.to_vec()),
                t if t > OPT_STRING => OptionValue::String(r.bytes((t - OPT_STRING) as usize, "option string")?.to_vec()),
                t => return Err(FormatError::new(at, format!("unknown option type {t:#x}"))),
            };
            out.push(v);
        }
        Ok(out)
    }

    /// Serialises an option list in the game's layout.
    pub fn write_block(values: &[OptionValue]) -> Vec<u8> {
        let mut out = Vec::new();
        for v in values {
            put_u32(&mut out, v.type_code());
            match v {
                OptionValue::Dword(x) | OptionValue::Bool(x) => put_u32(&mut out, *x),
                OptionValue::Real32(x) => put_f32(&mut out, *x),
                OptionValue::Byte(x) | OptionValue::Bool8(x) => out.push(*x),
                OptionValue::String(s) | OptionValue::Struct(s) => out.extend_from_slice(s),
            }
        }
        out
    }
}

/// One key of a snapshot: the scan-code index and its packed state byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyState {
    pub index: u8,
    pub state: u8,
}

impl KeyState {
    /// Key is held down.
    pub fn pressed(self) -> bool {
        self.state & 1 != 0
    }

    /// Two-bit "stick" field (the key was hit since it was last polled).
    pub fn stick(self) -> u8 {
        (self.state >> 1) & 3
    }

    /// Signed five-bit count of presses not yet consumed.
    pub fn num_pressed(self) -> i8 {
        (self.state as i8) >> 3
    }
}

/// The input state at the start of one UI pass.
#[derive(Debug, Clone, PartialEq)]
pub struct InputSnapshot {
    /// `taskTimeElapsed` when the snapshot was taken, seconds.
    pub time: f32,
    pub mouse_x: i16,
    pub mouse_y: i16,
    /// `utyNFrameTicks`.
    pub frame_ticks: u32,
    pub mouse_buttons: u8,
    /// `taskNumberCalls` at save time.
    pub task_number: u8,
    /// Uninitialised in the original; kept for a byte-exact round trip.
    pub pad: u8,
    /// Keys whose state byte is non-zero, in scan-code order.
    pub keys: Vec<KeyState>,
}

impl InputSnapshot {
    fn read(r: &mut Reader<'_>) -> Result<Self, FormatError> {
        let time = r.f32("snapshot time")?;
        let mouse_x = r.i16("mouse x")?;
        let mouse_y = r.i16("mouse y")?;
        let frame_ticks = r.u32("frame ticks")?;
        let mouse_buttons = r.u8("mouse buttons")?;
        let task_number = r.u8("task number")?;
        let pad = r.u8("pad")?;
        let n = r.u8("key count")? as usize;
        let raw = r.bytes(n * 2, "key list")?;
        let keys = raw
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| KeyState {
                index: c[0],
                state: c[1],
            })
            .collect();
        Ok(Self {
            time,
            mouse_x,
            mouse_y,
            frame_ticks,
            mouse_buttons,
            task_number,
            pad,
            keys,
        })
    }

    fn write(&self, out: &mut Vec<u8>) {
        put_f32(out, self.time);
        put_u16(out, self.mouse_x as u16);
        put_u16(out, self.mouse_y as u16);
        put_u32(out, self.frame_ticks);
        out.push(self.mouse_buttons);
        out.push(self.task_number);
        out.push(self.pad);
        out.push(self.keys.len() as u8);
        for k in &self.keys {
            out.push(k.index);
            out.push(k.state);
        }
    }

    /// Whether a key is held in this snapshot.
    pub fn key_down(&self, index: u8) -> bool {
        self.keys.iter().any(|k| k.index == index && k.pressed())
    }
}

/// One record of the event stream, in file order.
#[derive(Debug, Clone, PartialEq)]
pub enum DemoEvent {
    /// Task ticks covered by the next scheduler pass.
    Ticks(i32),
    /// Input for the UI pass of that scheduler pass.
    Input(InputSnapshot),
}

/// A parsed demo file.
#[derive(Debug, Clone, PartialEq)]
pub struct Demo {
    pub version: u16,
    /// `taskTimeElapsed` when recording started.
    pub start_time: f32,
    /// Random streams at the start (25 or 27, see [`Flavour`]).
    pub streams: Vec<RanStream>,
    /// The option block, raw (see [`Demo::options`]).
    pub initial_state: Vec<u8>,
    pub events: Vec<DemoEvent>,
}

impl Demo {
    /// Parses a demo, working out how many random streams the header holds.
    ///
    /// Both layouts are tried; a layout is acceptable when its option block parses exactly
    /// and its event stream ends exactly at the end of the file. When both are acceptable,
    /// the one whose snapshot times agree better with the tick counts wins.
    pub fn parse(data: &[u8]) -> Result<Demo, FormatError> {
        let cat = Self::parse_with_streams(data, STREAMS_CATACLYSM);
        let hw = Self::parse_with_streams(data, STREAMS_HOMEWORLD);
        let ok = |d: &Result<Demo, FormatError>| matches!(d, Ok(d) if OptionValue::parse_block(&d.initial_state).is_ok());
        match (ok(&cat), ok(&hw)) {
            (true, false) => cat,
            (false, true) => hw,
            (true, true) => {
                let (c, h) = (cat?, hw?);
                if h.timing_error() < c.timing_error() {
                    Ok(h)
                } else {
                    Ok(c)
                }
            }
            (false, false) => cat.and(Err(FormatError::new(12, "option block does not parse with 25 or 27 streams"))),
        }
    }

    /// Parses a demo whose header holds `stream_count` random streams.
    pub fn parse_with_streams(data: &[u8], stream_count: usize) -> Result<Demo, FormatError> {
        let mut r = Reader::new(data);
        let ident = r.array::<6>("ident")?;
        if ident != IDENT {
            return Err(FormatError::new(0, format!("not a demo: ident {ident:02x?}")));
        }
        let version = r.u16("version")?;
        if version != VERSION {
            return Err(FormatError::new(6, format!("unsupported demo version {version:#x}")));
        }
        let start_time = r.f32("start time")?;
        let state_size = r.i32("initial state size")?;
        if state_size < 0 {
            return Err(FormatError::new(12, format!("negative initial state size {state_size}")));
        }
        let mut streams = Vec::with_capacity(stream_count);
        for _ in 0..stream_count {
            streams.push(RanStream {
                x: r.u32("random stream")?,
                y: r.u32("random stream")?,
                z: r.u32("random stream")?,
                c: r.u32("random stream")?,
                n: r.u32("random stream")?,
            });
        }
        let initial_state = r.bytes(state_size as usize, "initial state")?.to_vec();

        let mut events = Vec::new();
        events.push(DemoEvent::Input(InputSnapshot::read(&mut r)?));
        while !r.is_empty() {
            events.push(DemoEvent::Ticks(r.i32("tick count")?));
            if r.is_empty() {
                break;
            }
            events.push(DemoEvent::Input(InputSnapshot::read(&mut r)?));
        }
        Ok(Demo {
            version,
            start_time,
            streams,
            initial_state,
            events,
        })
    }

    /// Serialises the demo; `parse(to_bytes(d)) == d` and parsing then writing a file
    /// reproduces it byte for byte.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&IDENT);
        put_u16(&mut out, self.version);
        put_f32(&mut out, self.start_time);
        put_u32(&mut out, self.initial_state.len() as u32);
        for s in &self.streams {
            for v in [s.x, s.y, s.z, s.c, s.n] {
                put_u32(&mut out, v);
            }
        }
        out.extend_from_slice(&self.initial_state);
        for e in &self.events {
            match e {
                DemoEvent::Ticks(t) => put_u32(&mut out, *t as u32),
                DemoEvent::Input(s) => s.write(&mut out),
            }
        }
        out
    }

    pub fn flavour(&self) -> Flavour {
        if self.streams.len() == STREAMS_HOMEWORLD {
            Flavour::Homeworld
        } else {
            Flavour::Cataclysm
        }
    }

    /// The option block decoded.
    pub fn options(&self) -> Result<Vec<OptionValue>, FormatError> {
        OptionValue::parse_block(&self.initial_state)
    }

    /// Input snapshots in order.
    pub fn inputs(&self) -> impl Iterator<Item = &InputSnapshot> {
        self.events.iter().filter_map(|e| match e {
            DemoEvent::Input(s) => Some(s),
            DemoEvent::Ticks(_) => None,
        })
    }

    /// Tick counts in order.
    pub fn ticks(&self) -> impl Iterator<Item = i32> + '_ {
        self.events.iter().filter_map(|e| match e {
            DemoEvent::Ticks(t) => Some(*t),
            DemoEvent::Input(_) => None,
        })
    }

    /// Total task ticks recorded.
    pub fn total_ticks(&self) -> i64 {
        self.ticks().map(i64::from).sum()
    }

    /// Length of the recording in seconds of task time.
    pub fn duration(&self) -> f64 {
        self.total_ticks() as f64 / TASK_FREQUENCY as f64
    }

    /// Replays the scheduler's clock (`taskTimeElapsed += ticks / taskFrequency`, single
    /// precision) over the tick counts and returns, for each snapshot after the first, the
    /// clock it predicts.
    pub fn predicted_times(&self) -> Vec<f32> {
        let mut clock = self.inputs().next().map_or(self.start_time, |s| s.time);
        let mut out = Vec::new();
        for e in self.events.iter().skip(1) {
            match e {
                DemoEvent::Ticks(t) => {
                    let delta = (*t as f64 / TASK_FREQUENCY as f64) as f32;
                    clock = (clock as f64 + delta as f64) as f32;
                }
                DemoEvent::Input(_) => out.push(clock),
            }
        }
        out
    }

    /// Largest gap between the recorded snapshot times and [`Demo::predicted_times`].
    pub fn timing_error(&self) -> f64 {
        self.inputs()
            .skip(1)
            .zip(self.predicted_times())
            .map(|(s, p)| (s.time as f64 - p as f64).abs())
            .fold(0.0, f64::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(streams: usize) -> Demo {
        Demo {
            version: VERSION,
            start_time: 1.5,
            streams: (0..streams as u32)
                .map(|i| RanStream {
                    x: i,
                    y: i * 3,
                    z: 7,
                    c: i & 1,
                    n: 0xdead_0000 + i,
                })
                .collect(),
            initial_state: OptionValue::write_block(&[
                OptionValue::Bool(1),
                OptionValue::Real32(1.5),
                OptionValue::Bool(1),
                OptionValue::Bool(0),
                OptionValue::Real32(1.0),
                OptionValue::Real32(0.5),
            ]),
            events: vec![
                DemoEvent::Input(InputSnapshot {
                    time: 1.5,
                    mouse_x: 320,
                    mouse_y: 240,
                    frame_ticks: 0,
                    mouse_buttons: 0,
                    task_number: 0,
                    pad: 0x67,
                    keys: vec![],
                }),
                DemoEvent::Ticks(24),
                DemoEvent::Input(InputSnapshot {
                    time: 1.6,
                    mouse_x: -3,
                    mouse_y: 479,
                    frame_ticks: 24,
                    mouse_buttons: 1,
                    task_number: 1,
                    pad: 0,
                    keys: vec![KeyState { index: 0x11, state: 0x09 }, KeyState { index: 0x44, state: 0x01 }],
                }),
                DemoEvent::Ticks(12),
            ],
        }
    }

    #[test]
    fn round_trip_both_flavours() {
        for (n, flavour) in [(27, Flavour::Cataclysm), (25, Flavour::Homeworld)] {
            let d = sample(n);
            let bytes = d.to_bytes();
            assert_eq!(bytes.len(), 16 + 20 * n + 48 + 16 + 4 + 20 + 4);
            let back = Demo::parse(&bytes).unwrap();
            assert_eq!(back, d);
            assert_eq!(back.flavour(), flavour);
            assert_eq!(back.to_bytes(), bytes);
        }
    }

    #[test]
    fn key_state_fields() {
        let k = KeyState { index: 0, state: 0b0001_0111 };
        assert!(k.pressed());
        assert_eq!(k.stick(), 3);
        assert_eq!(k.num_pressed(), 2);
        assert_eq!(KeyState { index: 0, state: 0xf8 }.num_pressed(), -1);
    }

    #[test]
    fn rejects_bad_header() {
        let mut b = sample(27).to_bytes();
        b[0] = b'X';
        assert!(Demo::parse(&b).is_err());
        let mut b = sample(27).to_bytes();
        b[6] = 8;
        assert!(Demo::parse(&b).is_err());
        let b = sample(27).to_bytes();
        assert!(Demo::parse(&b[..b.len() - 3]).is_err());
    }

    #[test]
    fn option_block_types() {
        let v = vec![
            OptionValue::Dword(0x80),
            OptionValue::Byte(3),
            OptionValue::Bool8(1),
            OptionValue::String(b"abc\0".to_vec()),
            OptionValue::Struct(vec![1, 2, 3, 4, 5]),
        ];
        let b = OptionValue::write_block(&v);
        assert_eq!(OptionValue::parse_block(&b).unwrap(), v);
        assert!(OptionValue::parse_block(&[9, 0, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(OptionValue::parse_block(&b[..b.len() - 1]).is_err());
    }
}
