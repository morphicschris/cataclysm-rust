//! `.nis` in-engine cinematics ("non-interactive sequences") and their companion `.script` event
//! lists.
//!
//! The binary `.nis` holds keyframed motion: one path per actor (ships, derelicts, asteroids,
//! light/lens-flare anchors), one or more camera paths, and optionally light paths. Each path is
//! a TCB (Kochanek–Bartels) spline over `n` keys of x, y, z, heading, pitch and bank, with the
//! first two and the last keys acting as spline end conditions. Everything that *happens* during
//! the sequence (show/hide, speech, fades, FOV changes, ...) lives in the text `.script` file of
//! the same name, parsed by [`NisScript`]. All binary values are little-endian.
//!
//! Header (68 bytes, followed by 64 reserved runtime bytes, so data starts at 132):
//!
//! | offset | type     | field                                                   |
//! |--------|----------|---------------------------------------------------------|
//! | 0      | [u8; 12] | identifier `"Crannnberry\0"`                            |
//! | 12     | u32      | version, `0x101`                                        |
//! | 16     | u32      | flags (runtime, 0 in files)                             |
//! | 20     | u32      | string block offset (always the tail of the file)       |
//! | 24     | u32      | string block length                                     |
//! | 28     | f32      | length in seconds                                       |
//! | 32     | f32      | loop point in seconds, negative for none                |
//! | 36     | u32, u32 | object path count, offset                               |
//! | 44     | u32, u32 | camera path count, offset                               |
//! | 52     | u32, u32 | light path count, offset                                |
//! | 60     | u32, u32 | event count, offset (always 0 events in shipped files)  |
//!
//! Object (and light) path, 56 bytes:
//!
//! | offset | type    | field                                                          |
//! |--------|---------|----------------------------------------------------------------|
//! | 0      | u32     | offset of the per-key duration array (f32 × n)                 |
//! | 4      | u32     | string-block offset of the object name, e.g. `sRecon(2)`       |
//! | 8      | u32     | string-block offset of the parent's name, 0 for none           |
//! | 12     | u32     | race / object category (see [`ObjectCategory`])                |
//! | 16     | i32     | key count `n`                                                  |
//! | 20     | f32     | start delay in seconds (negative for none)                     |
//! | 24     | u32     | offset of key times (f32 × n, absolute seconds)                |
//! | 28     | u32     | offset of TCB parameters (f32 × 3 × n)                         |
//! | 32     | u32 × 6 | offsets of the x, y, z, heading, pitch, bank arrays (f32 × n)  |
//!
//! Camera path, 44 bytes: the same from offset 16 onwards, preceded by only the duration-array
//! offset (no name, parent or race).
//!
//! Event record (24 bytes, never present in shipped files): time f32, opcode u32, object index,
//! script order index, two parameter words.

use thiserror::Error;

pub const IDENTIFIER: &[u8; 12] = b"Crannnberry\0";
pub const VERSION: u32 = 0x101;
pub const HEADER_LEN: usize = 68;
/// Runtime scratch space between the header and the first path.
pub const HEADER_EXTRA_LEN: usize = 64;
pub const OBJECT_PATH_LEN: usize = 56;
pub const CAMERA_PATH_LEN: usize = 44;
pub const EVENT_LEN: usize = 24;
/// Frame rate that `at frame N` script times are expressed in.
pub const FRAME_RATE: f32 = 30.0;
/// Hard ceiling on keys per path, to reject corrupt counts before allocating.
const MAX_KEYS: usize = 1 << 20;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for an NIS header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 12]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u32),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("bad {what} count {count}")]
    BadCount { what: &'static str, count: i64 },
    #[error("string offset {0} is not inside the string block")]
    BadString(u32),
    #[error("script line {line}: {message}")]
    Script { line: usize, message: String },
}

pub type Result<T> = std::result::Result<T, Error>;

/// What kind of thing an object path animates, from its race field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectCategory {
    /// A ship of the given race (0 Somtaaw, 1 Beast, 2 Kushan, 3 Taiidan, 4 Turanic, 5 Bentusi
    /// traders, as observed).
    Ship(u32),
    /// A derelict, including light-source and lens-flare anchor objects.
    Derelict,
    Asteroid,
    Other(u32),
}

impl ObjectCategory {
    pub fn from_race(race: u32) -> ObjectCategory {
        match race {
            0..=5 => ObjectCategory::Ship(race),
            6 => ObjectCategory::Derelict,
            7 => ObjectCategory::Asteroid,
            n => ObjectCategory::Other(n),
        }
    }
}

/// One spline key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Key {
    /// Absolute time in seconds from the start of the sequence.
    pub time: f32,
    /// Time from this key to the next (0 for the leading end-condition keys).
    pub duration: f32,
    pub tension: f32,
    pub continuity: f32,
    pub bias: f32,
    pub position: [f32; 3],
    /// Heading, pitch, bank in degrees.
    pub rotation: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPath {
    /// Name as written, with an optional `(instance)` suffix.
    pub name: String,
    pub parent: Option<String>,
    pub race: u32,
    /// Delay before the path starts, in seconds; negative for none.
    pub time_offset: f32,
    pub keys: Vec<Key>,
}

/// Splits `Name(3)` into (`Name`, 3); a name without a suffix is instance 0.
pub fn split_instance(name: &str) -> (&str, u32) {
    match name.find('(') {
        Some(i) => {
            let digits = name[i + 1..].trim_end_matches(')').trim();
            (name[..i].trim_end(), digits.parse().unwrap_or(0))
        }
        None => (name, 0),
    }
}

/// Canonical form for matching `.script` object references (`RACE_SECT\sRecon (2)`) against
/// `.nis` path names (`sRecon(2)`): directory prefix dropped, blanks removed, lower case.
pub fn object_key(name: &str) -> String {
    let leaf = name.rsplit(['\\', '/']).next().unwrap_or(name);
    leaf.chars()
        .filter(|c| !c.is_whitespace())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl ObjectPath {
    pub fn category(&self) -> ObjectCategory {
        ObjectCategory::from_race(self.race)
    }

    pub fn base_name(&self) -> &str {
        split_instance(&self.name).0
    }

    pub fn instance(&self) -> u32 {
        split_instance(&self.name).1
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct CameraPath {
    pub time_offset: f32,
    pub keys: Vec<Key>,
}

/// A binary event record (the format allows them; shipped files keep events in `.script`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawEvent {
    pub time: f32,
    pub opcode: u32,
    pub object: i32,
    pub order: i32,
    pub params: [u32; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Nis {
    pub version: u32,
    pub flags: u32,
    /// Length in seconds.
    pub length: f32,
    /// Loop point in seconds, if the sequence loops.
    pub loop_point: Option<f32>,
    pub objects: Vec<ObjectPath>,
    pub cameras: Vec<CameraPath>,
    pub lights: Vec<ObjectPath>,
    pub events: Vec<RawEvent>,
}

struct Reader<'a> {
    data: &'a [u8],
    strings: &'a [u8],
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

impl<'a> Reader<'a> {
    fn bytes(&self, what: &'static str, offset: usize, len: usize) -> Result<&'a [u8]> {
        offset
            .checked_add(len)
            .and_then(|end| self.data.get(offset..end))
            .ok_or(Error::OutOfBounds {
                what,
                offset,
                len,
                file_len: self.data.len(),
            })
    }

    fn floats(&self, what: &'static str, offset: u32, count: usize) -> Result<Vec<f32>> {
        Ok(self
            .bytes(what, offset as usize, count * 4)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect())
    }

    fn string(&self, offset: u32) -> Result<String> {
        let rest = self
            .strings
            .get(offset as usize..)
            .ok_or(Error::BadString(offset))?;
        let end = rest
            .iter()
            .position(|&b| b == 0)
            .ok_or(Error::BadString(offset))?;
        Ok(rest[..end].iter().map(|&b| b as char).collect())
    }

    fn count(what: &'static str, raw: u32, limit: usize) -> Result<usize> {
        let n = raw as i32;
        if n < 0 || n as usize > limit {
            return Err(Error::BadCount {
                what,
                count: n.into(),
            });
        }
        Ok(n as usize)
    }

    /// Reads the key arrays of a path whose key count sits at `rec[at]`.
    fn keys(&self, rec: &[u8], durations: u32, at: usize) -> Result<(f32, Vec<Key>)> {
        let n = Self::count("key", u32_at(rec, at), MAX_KEYS)?;
        let time_offset = f32_at(rec, at + 4);
        let durations = self.floats("durations", durations, n)?;
        let times = self.floats("key times", u32_at(rec, at + 8), n)?;
        let tcb = self.floats("TCB parameters", u32_at(rec, at + 12), n * 3)?;
        let mut channels = Vec::with_capacity(6);
        for c in 0..6 {
            channels.push(self.floats("curve", u32_at(rec, at + 16 + c * 4), n)?);
        }
        let keys = (0..n)
            .map(|i| Key {
                time: times[i],
                duration: durations[i],
                tension: tcb[i * 3],
                continuity: tcb[i * 3 + 1],
                bias: tcb[i * 3 + 2],
                position: [channels[0][i], channels[1][i], channels[2][i]],
                rotation: [channels[3][i], channels[4][i], channels[5][i]],
            })
            .collect();
        Ok((time_offset, keys))
    }

    fn object_paths(&self, what: &'static str, count: u32, offset: u32) -> Result<Vec<ObjectPath>> {
        let n = Self::count(what, count, self.data.len() / OBJECT_PATH_LEN)?;
        self.bytes(what, offset as usize, n * OBJECT_PATH_LEN)?
            .as_chunks::<OBJECT_PATH_LEN>()
            .0
            .iter()
            .map(|rec| {
                let (time_offset, keys) = self.keys(rec, u32_at(rec, 0), 16)?;
                let parent = match u32_at(rec, 8) {
                    0 => None,
                    o => Some(self.string(o)?),
                };
                Ok(ObjectPath {
                    name: self.string(u32_at(rec, 4))?,
                    parent,
                    race: u32_at(rec, 12),
                    time_offset,
                    keys,
                })
            })
            .collect()
    }
}

impl Nis {
    pub fn parse(data: &[u8]) -> Result<Nis> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 12] = data[..12].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = u32_at(data, 12);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let mut r = Reader { data, strings: &[] };
        r.strings = r.bytes(
            "string block",
            u32_at(data, 20) as usize,
            u32_at(data, 24) as usize,
        )?;
        let loop_point = f32_at(data, 32);

        let objects = r.object_paths("object paths", u32_at(data, 36), u32_at(data, 40))?;
        let n_cameras = Reader::count(
            "camera path",
            u32_at(data, 44),
            data.len() / CAMERA_PATH_LEN,
        )?;
        let cameras = r
            .bytes(
                "camera paths",
                u32_at(data, 48) as usize,
                n_cameras * CAMERA_PATH_LEN,
            )?
            .as_chunks::<CAMERA_PATH_LEN>()
            .0
            .iter()
            .map(|rec| {
                let (time_offset, keys) = r.keys(rec, u32_at(rec, 0), 4)?;
                Ok(CameraPath { time_offset, keys })
            })
            .collect::<Result<Vec<_>>>()?;
        let lights = r.object_paths("light paths", u32_at(data, 52), u32_at(data, 56))?;
        let n_events = Reader::count("event", u32_at(data, 60), data.len() / EVENT_LEN)?;
        let events = r
            .bytes("events", u32_at(data, 64) as usize, n_events * EVENT_LEN)?
            .as_chunks::<EVENT_LEN>()
            .0
            .iter()
            .map(|e| RawEvent {
                time: f32_at(e, 0),
                opcode: u32_at(e, 4),
                object: u32_at(e, 8) as i32,
                order: u32_at(e, 12) as i32,
                params: [u32_at(e, 16), u32_at(e, 20)],
            })
            .collect();

        Ok(Nis {
            version,
            flags: u32_at(data, 16),
            length: f32_at(data, 28),
            loop_point: (loop_point >= 0.0).then_some(loop_point),
            objects,
            cameras,
            lights,
            events,
        })
    }

    /// Index of the object path a `.script` `with` line refers to.
    pub fn find_object(&self, script_name: &str) -> Option<usize> {
        let key = object_key(script_name);
        self.objects.iter().position(|o| object_key(&o.name) == key)
    }
}

/// When a script event fires.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScriptTime {
    /// `at frame N` (30 frames per second).
    Frame(f32),
    /// `at N`, in seconds.
    Seconds(f32),
}

impl ScriptTime {
    pub fn seconds(self) -> f32 {
        match self {
            ScriptTime::Frame(f) => f / FRAME_RATE,
            ScriptTime::Seconds(s) => s,
        }
    }
}

/// One event line of a `.script` file.
#[derive(Debug, Clone, PartialEq)]
pub struct ScriptEvent {
    /// 1-based source line.
    pub line: usize,
    pub time: ScriptTime,
    /// Object named by the enclosing `with` block, as written.
    pub object: Option<String>,
    /// Event keyword, e.g. `show`, `fadeTo`, `fleetSpeechEvent`.
    pub keyword: String,
    /// Everything after the keyword, trimmed.
    pub args: String,
}

/// A parsed `.script` event list.
///
/// The files are C-preprocessor output: `#line` markers and other `#` lines are skipped. Each
/// remaining line is a keyword and an argument string. `at frame N` / `at SECONDS` sets the time
/// for following events; `with OBJECT` followed by a `{ ... }` block sets the object they apply
/// to; everything else is an event.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NisScript {
    pub events: Vec<ScriptEvent>,
    /// Every object named in a `with` line, in order of appearance.
    pub objects: Vec<String>,
}

fn script_error(line: usize, message: impl Into<String>) -> Error {
    Error::Script {
        line,
        message: message.into(),
    }
}

impl NisScript {
    /// Parses script text. Bytes are taken as Latin-1 by [`NisScript::parse_bytes`].
    pub fn parse(text: &str) -> Result<NisScript> {
        let mut script = NisScript::default();
        let mut time = ScriptTime::Frame(0.0);
        let mut object: Option<String> = None;
        let mut in_block = false;
        for (index, raw) in text.lines().enumerate() {
            let line_no = index + 1;
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (keyword, args) = match line.find(char::is_whitespace) {
                Some(i) => (&line[..i], line[i..].trim()),
                None => (line, ""),
            };
            match keyword {
                "at" => {
                    time = parse_time(args)
                        .ok_or_else(|| script_error(line_no, format!("bad time {args:?}")))?
                }
                "with" => {
                    if in_block {
                        return Err(script_error(line_no, "`with` inside a block"));
                    }
                    if args.is_empty() {
                        return Err(script_error(line_no, "`with` without an object"));
                    }
                    script.objects.push(args.to_string());
                    object = Some(args.to_string());
                }
                "{" => {
                    if in_block || object.is_none() {
                        return Err(script_error(line_no, "unexpected `{`"));
                    }
                    in_block = true;
                }
                "}" => {
                    if !in_block {
                        return Err(script_error(line_no, "unmatched `}`"));
                    }
                    in_block = false;
                    object = None;
                }
                _ => script.events.push(ScriptEvent {
                    line: line_no,
                    time,
                    object: object.clone(),
                    keyword: keyword.to_string(),
                    args: args.to_string(),
                }),
            }
        }
        if in_block {
            return Err(script_error(text.lines().count(), "unterminated block"));
        }
        Ok(script)
    }

    pub fn parse_bytes(data: &[u8]) -> Result<NisScript> {
        let text: String = data.iter().map(|&b| b as char).collect();
        NisScript::parse(&text)
    }
}

fn parse_time(args: &str) -> Option<ScriptTime> {
    let mut words = args.split_whitespace();
    let first = words.next()?;
    let time = if first.eq_ignore_ascii_case("frame") {
        ScriptTime::Frame(words.next()?.parse().ok()?)
    } else {
        ScriptTime::Seconds(first.trim_end_matches(['s', 'S']).parse().ok()?)
    };
    words.next().is_none().then_some(time)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_blocks_and_times() {
        let text = "#line 1 \"x.sc\"\nat frame 0\n   fadeSet 100%, 0\nwith RACE_SECT\\sRecon (2)\n{\n  at frame 45\n     show\n#   hide\n}\nat 2.5\n  stopMusic\n";
        let s = NisScript::parse(text).unwrap();
        assert_eq!(s.events.len(), 3);
        assert_eq!(s.events[0].keyword, "fadeSet");
        assert_eq!(s.events[0].args, "100%, 0");
        assert_eq!(s.events[1].object.as_deref(), Some("RACE_SECT\\sRecon (2)"));
        assert_eq!(s.events[1].time.seconds(), 1.5);
        assert_eq!(s.events[2].object, None);
        assert_eq!(s.events[2].time, ScriptTime::Seconds(2.5));
        assert_eq!(object_key("RACE_SECT\\sRecon (2)"), object_key("sRecon(2)"));
        assert_eq!(split_instance("sRecon(2)"), ("sRecon", 2));
        assert!(NisScript::parse("}\n").is_err());
        assert!(NisScript::parse("with a\n{\n").is_err());
        assert!(NisScript::parse("at frame x\n").is_err());
    }

    #[test]
    fn parses_synthetic_nis() {
        // One object with two keys, no cameras; arrays then strings.
        let paths = HEADER_LEN + HEADER_EXTRA_LEN;
        let arrays = paths + OBJECT_PATH_LEN;
        let n = 2usize;
        // durations, times, tcb(3n), six channels
        let floats = n + n + 3 * n + 6 * n;
        let strings = arrays + floats * 4;
        let mut d = vec![0u8; strings];
        d.extend_from_slice(b"sWorker(3)\0");
        let put =
            |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_le_bytes());
        d[..12].copy_from_slice(IDENTIFIER);
        put(&mut d, 12, VERSION);
        put(&mut d, 20, strings as u32);
        put(&mut d, 24, 11);
        put(&mut d, 28, 10.0f32.to_bits());
        put(&mut d, 32, (-1.0f32).to_bits());
        put(&mut d, 36, 1);
        put(&mut d, 40, paths as u32);
        for at in [48, 56, 64] {
            put(&mut d, at, strings as u32);
        }
        put(&mut d, paths, arrays as u32);
        put(&mut d, paths + 12, 0);
        put(&mut d, paths + 16, n as u32);
        let mut cursor = arrays + n * 4;
        for slot in 0..8 {
            put(&mut d, paths + 24 + slot * 4, cursor as u32);
            cursor += if slot == 1 { 3 * n * 4 } else { n * 4 };
        }
        let x = arrays + (n + n + 3 * n) * 4;
        put(&mut d, x + 4, 5.0f32.to_bits());
        let nis = Nis::parse(&d).unwrap();
        assert_eq!(nis.loop_point, None);
        let o = &nis.objects[0];
        assert_eq!((o.base_name(), o.instance()), ("sWorker", 3));
        assert_eq!(o.category(), ObjectCategory::Ship(0));
        assert_eq!(o.keys[1].position[0], 5.0);
        assert_eq!(nis.find_object("RACE_SECT\\sWorker (3)"), Some(0));
        put(&mut d, paths + 16, 1 << 30);
        assert!(Nis::parse(&d).is_err());
    }
}
