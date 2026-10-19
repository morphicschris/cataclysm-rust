//! Per-tick world-state records: what a capture of the original and our
//! engine write after every universe update, for [`crate::compare`].
//!
//! # File layout (little-endian)
//!
//! ```text
//! magic "HWCSTATE" [8], version u16 (1), header_len u16 (bytes of header that follow)
//! header   source u8, stream_count u8, ship_record_len u16, label_len u16, label utf-8
//!          (readers skip any further header bytes)
//! frame*   frame_len u32, then frame_len bytes:
//!          update_counter u32, univ_time f32,
//!          stream_count x (x, y, z, c, n) u32,
//!          ship_count u32, ship_count x ship_record_len bytes
//!          (readers ignore bytes after the ships)
//! ship     ship_number u16, player u8, race u8, ship_type u16, reserved u16, flags u32,
//!          position [f32; 3], velocity [f32; 3], coordsys [f32; 9] (game memory order),
//!          rotspeed [f32; 3], health f32, fuel f32            (92 bytes in version 1)
//!          bytes beyond these are kept as `extra` and compared byte for byte
//! ```
//!
//! Ships are stored in the universe's ship-list order (live and dying alike), because the
//! order is itself simulation state: the original's checksums and many of its loops depend on
//! it. Floats are stored as their bit patterns and compared that way.

use std::io::{self, Read, Write};

use hwc_math::random::RanStream;

use crate::bytes::{put_f32, put_u16, put_u32, FormatError, Reader};

pub const MAGIC: [u8; 8] = *b"HWCSTATE";
pub const VERSION: u16 = 1;
/// Size of a version-1 ship record.
pub const SHIP_RECORD_LEN: usize = 92;

/// Who produced a state stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Unknown,
    /// The original game.
    Original,
    /// This recreation.
    Recreation,
    Other(u8),
}

impl Source {
    fn code(self) -> u8 {
        match self {
            Source::Unknown => 0,
            Source::Original => 1,
            Source::Recreation => 2,
            Source::Other(c) => c,
        }
    }

    fn from_code(c: u8) -> Self {
        match c {
            0 => Source::Unknown,
            1 => Source::Original,
            2 => Source::Recreation,
            c => Source::Other(c),
        }
    }
}

/// Stream header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateHeader {
    pub source: Source,
    /// Random streams per frame (27 for Cataclysm).
    pub stream_count: u8,
    /// Bytes per ship record (at least [`SHIP_RECORD_LEN`]).
    pub ship_record_len: u16,
    /// Free text: build, recording name, ...
    pub label: String,
}

impl StateHeader {
    pub fn new(source: Source, label: impl Into<String>) -> Self {
        Self {
            source,
            stream_count: hwc_math::random::STREAM_COUNT as u8,
            ship_record_len: SHIP_RECORD_LEN as u16,
            label: label.into(),
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut body = vec![self.source.code(), self.stream_count];
        put_u16(&mut body, self.ship_record_len);
        put_u16(&mut body, self.label.len() as u16);
        body.extend_from_slice(self.label.as_bytes());
        let mut out = MAGIC.to_vec();
        put_u16(&mut out, VERSION);
        put_u16(&mut out, body.len() as u16);
        out.extend(body);
        out
    }

    /// Parses the header at the start of `data`; returns it and its size.
    pub fn parse(data: &[u8]) -> Result<(StateHeader, usize), FormatError> {
        let mut r = Reader::new(data);
        if r.array::<8>("magic")? != MAGIC {
            return Err(FormatError::new(0, "not a state record (magic)"));
        }
        let version = r.u16("version")?;
        if version != VERSION {
            return Err(FormatError::new(8, format!("unsupported state record version {version}")));
        }
        let header_len = r.u16("header length")? as usize;
        let body = r.bytes(header_len, "header")?;
        let mut h = Reader::new(body);
        let source = Source::from_code(h.u8("source")?);
        let stream_count = h.u8("stream count")?;
        let ship_record_len = h.u16("ship record length")?;
        if (ship_record_len as usize) < SHIP_RECORD_LEN {
            return Err(FormatError::new(14, format!("ship record length {ship_record_len} too small")));
        }
        let label_len = h.u16("label length")? as usize;
        let label = String::from_utf8_lossy(h.bytes(label_len, "label")?).into_owned();
        Ok((
            StateHeader {
                source,
                stream_count,
                ship_record_len,
                label,
            },
            r.pos(),
        ))
    }
}

/// One ship as recorded.
#[derive(Debug, Clone, PartialEq)]
pub struct ShipState {
    /// `shipID.shipNumber`.
    pub ship_number: u16,
    pub player: u8,
    pub race: u8,
    pub ship_type: u16,
    pub reserved: u16,
    /// `SpaceObj.flags`.
    pub flags: u32,
    pub position: [f32; 3],
    pub velocity: [f32; 3],
    /// Rotation matrix in the order of the game's `matrix` (m11, m21, m31, m12, ...).
    pub coordsys: [f32; 9],
    pub rotspeed: [f32; 3],
    pub health: f32,
    pub fuel: f32,
    /// Record bytes beyond the version-1 fields.
    pub extra: Vec<u8>,
}

impl Default for ShipState {
    fn default() -> Self {
        Self {
            ship_number: 0,
            player: 0,
            race: 0,
            ship_type: 0,
            reserved: 0,
            flags: 0,
            position: [0.0; 3],
            velocity: [0.0; 3],
            coordsys: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            rotspeed: [0.0; 3],
            health: 0.0,
            fuel: 0.0,
            extra: Vec::new(),
        }
    }
}

/// `SOF_Dead`.
pub const SHIP_FLAG_DEAD: u32 = 0x800;

impl ShipState {
    pub fn is_dead(&self) -> bool {
        self.flags & SHIP_FLAG_DEAD != 0
    }

    /// Writes the record, padding `extra` with zeros to `record_len`.
    pub fn write(&self, out: &mut Vec<u8>, record_len: usize) {
        let start = out.len();
        put_u16(out, self.ship_number);
        out.push(self.player);
        out.push(self.race);
        put_u16(out, self.ship_type);
        put_u16(out, self.reserved);
        put_u32(out, self.flags);
        for v in self
            .position
            .iter()
            .chain(&self.velocity)
            .chain(&self.coordsys)
            .chain(&self.rotspeed)
            .chain([&self.health, &self.fuel])
        {
            put_f32(out, *v);
        }
        out.extend_from_slice(&self.extra);
        out.resize(start + record_len, 0);
    }

    pub fn parse(rec: &[u8]) -> Result<ShipState, FormatError> {
        let mut r = Reader::new(rec);
        let mut s = ShipState {
            ship_number: r.u16("ship number")?,
            player: r.u8("player")?,
            race: r.u8("race")?,
            ship_type: r.u16("ship type")?,
            reserved: r.u16("reserved")?,
            flags: r.u32("flags")?,
            ..ShipState::default()
        };
        let mut f = || r.f32("ship float");
        for v in s.position.iter_mut() {
            *v = f()?;
        }
        for v in s.velocity.iter_mut() {
            *v = f()?;
        }
        for v in s.coordsys.iter_mut() {
            *v = f()?;
        }
        for v in s.rotspeed.iter_mut() {
            *v = f()?;
        }
        s.health = f()?;
        s.fuel = f()?;
        s.extra = rec[SHIP_RECORD_LEN.min(rec.len())..].to_vec();
        Ok(s)
    }
}

/// The world after one universe update.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StateFrame {
    /// `universe.univUpdateCounter` after the update.
    pub update_counter: u32,
    /// `universe.totaltimeelapsed` after the update.
    pub univ_time: f32,
    pub streams: Vec<RanStream>,
    /// In ship-list order.
    pub ships: Vec<ShipState>,
}

impl StateFrame {
    /// Frame payload (without the length word).
    pub fn payload(&self, header: &StateHeader) -> Vec<u8> {
        let rec_len = header.ship_record_len as usize;
        let mut out = Vec::with_capacity(12 + 20 * self.streams.len() + rec_len * self.ships.len());
        put_u32(&mut out, self.update_counter);
        put_f32(&mut out, self.univ_time);
        for i in 0..header.stream_count as usize {
            let s = self.streams.get(i).copied().unwrap_or(RanStream { x: 0, y: 0, z: 0, c: 0, n: 0 });
            for v in [s.x, s.y, s.z, s.c, s.n] {
                put_u32(&mut out, v);
            }
        }
        put_u32(&mut out, self.ships.len() as u32);
        for s in &self.ships {
            s.write(&mut out, rec_len);
        }
        out
    }

    pub fn parse_payload(p: &[u8], header: &StateHeader) -> Result<StateFrame, FormatError> {
        let mut r = Reader::new(p);
        let update_counter = r.u32("update counter")?;
        let univ_time = r.f32("universe time")?;
        let mut streams = Vec::with_capacity(header.stream_count as usize);
        for _ in 0..header.stream_count {
            streams.push(RanStream {
                x: r.u32("stream")?,
                y: r.u32("stream")?,
                z: r.u32("stream")?,
                c: r.u32("stream")?,
                n: r.u32("stream")?,
            });
        }
        let n = r.u32("ship count")? as usize;
        let rec_len = header.ship_record_len as usize;
        if r.remaining() / rec_len.max(1) < n {
            return Err(FormatError::new(r.pos(), format!("{n} ships do not fit in the frame")));
        }
        let mut ships = Vec::with_capacity(n);
        for _ in 0..n {
            let at = r.pos();
            let rec = r.bytes(rec_len, "ship record")?;
            ships.push(ShipState::parse(rec).map_err(|e| FormatError::new(at + e.offset, e.message))?);
        }
        Ok(StateFrame {
            update_counter,
            univ_time,
            streams,
            ships,
        })
    }

    pub fn ship(&self, number: u16) -> Option<&ShipState> {
        self.ships.iter().find(|s| s.ship_number == number)
    }
}

/// Appends frames to a state stream.
pub struct StateWriter<W: Write> {
    out: W,
    header: StateHeader,
    scratch: Vec<u8>,
}

impl<W: Write> StateWriter<W> {
    pub fn new(mut out: W, header: StateHeader) -> io::Result<Self> {
        out.write_all(&header.to_bytes())?;
        Ok(Self {
            out,
            header,
            scratch: Vec::new(),
        })
    }

    pub fn header(&self) -> &StateHeader {
        &self.header
    }

    pub fn write_frame(&mut self, frame: &StateFrame) -> io::Result<()> {
        let payload = frame.payload(&self.header);
        self.write_payload(&payload)
    }

    /// Writes an already-built frame payload.
    pub fn write_payload(&mut self, payload: &[u8]) -> io::Result<()> {
        self.scratch.clear();
        put_u32(&mut self.scratch, payload.len() as u32);
        self.scratch.extend_from_slice(payload);
        self.out.write_all(&self.scratch)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.out.flush()
    }

    pub fn into_inner(self) -> W {
        self.out
    }
}

/// A whole state stream in memory.
#[derive(Debug, Clone, PartialEq)]
pub struct StateStream {
    pub header: StateHeader,
    pub frames: Vec<StateFrame>,
    /// True when the file ended inside a frame (the writer was killed); that frame is dropped.
    pub truncated: bool,
}

impl StateStream {
    pub fn parse(data: &[u8]) -> Result<StateStream, FormatError> {
        let (header, mut pos) = StateHeader::parse(data)?;
        let mut frames = Vec::new();
        let mut truncated = false;
        while pos < data.len() {
            let mut r = Reader::at(data, pos);
            let len = match r.u32("frame length") {
                Ok(l) => l as usize,
                Err(_) => {
                    truncated = true;
                    break;
                }
            };
            let Ok(payload) = r.bytes(len, "frame") else {
                truncated = true;
                break;
            };
            let frame = StateFrame::parse_payload(payload, &header)
                .map_err(|e| FormatError::new(pos + 4 + e.offset, format!("frame {}: {}", frames.len(), e.message)))?;
            frames.push(frame);
            pos = r.pos();
        }
        Ok(StateStream {
            header,
            frames,
            truncated,
        })
    }

    pub fn read_from(mut r: impl Read) -> io::Result<StateStream> {
        let mut data = Vec::new();
        r.read_to_end(&mut data)?;
        Ok(Self::parse(&data)?)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = StateWriter::new(Vec::new(), self.header.clone()).expect("writing to memory");
        for f in &self.frames {
            w.write_frame(f).expect("writing to memory");
        }
        w.into_inner()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn ship(n: u16, x: f32) -> ShipState {
        ShipState {
            ship_number: n,
            player: (n % 2) as u8,
            race: 1,
            ship_type: 10 + n,
            flags: 0x2003,
            position: [x, x * 2.0, -x],
            velocity: [0.5, 0.0, -0.25],
            rotspeed: [0.0, 0.01, 0.0],
            health: 300.0,
            fuel: 1000.0,
            ..ShipState::default()
        }
    }

    pub(crate) fn frame(counter: u32, xs: &[f32]) -> StateFrame {
        StateFrame {
            update_counter: counter,
            univ_time: counter as f32 / 16.0,
            streams: vec![RanStream::DEFAULT; 27],
            ships: xs.iter().enumerate().map(|(i, &x)| ship(i as u16 + 1, x)).collect(),
        }
    }

    #[test]
    fn round_trip() {
        let s = StateStream {
            header: StateHeader::new(Source::Recreation, "test"),
            frames: vec![frame(1, &[1.0, 2.0]), frame(2, &[]), frame(3, &[3.5])],
            truncated: false,
        };
        let b = s.to_bytes();
        assert_eq!(StateStream::parse(&b).unwrap(), s);
        let cut = StateStream::parse(&b[..b.len() - 5]).unwrap();
        assert!(cut.truncated);
        assert_eq!(cut.frames.len(), 2);
    }

    #[test]
    fn longer_ship_records_keep_extra_bytes() {
        let mut h = StateHeader::new(Source::Original, "");
        h.ship_record_len = 100;
        let mut f = frame(7, &[1.0]);
        f.ships[0].extra = vec![1, 2, 3, 4, 5, 6, 7, 8];
        let p = f.payload(&h);
        assert_eq!(StateFrame::parse_payload(&p, &h).unwrap(), f);
    }

    #[test]
    fn ship_record_is_92_bytes() {
        let mut v = Vec::new();
        ship(1, 1.0).write(&mut v, SHIP_RECORD_LEN);
        assert_eq!(v.len(), SHIP_RECORD_LEN);
        assert_eq!(ShipState::parse(&v).unwrap(), ship(1, 1.0));
    }
}
