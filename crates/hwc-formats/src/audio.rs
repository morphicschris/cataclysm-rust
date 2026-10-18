//! Sound data: effect banks (`.bnk`), event/speech lookup tables (`.lut`), the streamed music
//! archive (`.wxh` index + `.wxd` data), the speech banks (`.vce`), and a decoder for the
//! frequency-quantised transform codec that every one of those sample payloads uses.
//!
//! All multi-byte values are little-endian. See `docs/formats/audio.md` for the byte layouts.
//!
//! # Codec summary
//!
//! Audio is stored as fixed-size blocks of `bitrate / 8` bytes per channel. Each block carries two
//! consecutive MDCT frames ("primary" and "secondary") on a fixed 86.13 Hz bin grid, so one block
//! is always 11.61 ms of sound regardless of the playback rate: 256 samples at 22 050 Hz (128
//! bins per frame), 512 at 44 100 Hz (256 bins) or 128 at 11 025 Hz (64 bins). Per-bin exponents
//! (grouped by 26 critical bands) persist from block to block until a block asks for a reset.

use std::f64::consts::PI;
use std::fmt;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AudioError {
    #[error("bad magic: expected {expected}, found {found:?}")]
    BadMagic { expected: &'static str, found: [u8; 4] },
    #[error("{0}")]
    Corrupt(String),
}

pub type Result<T, E = AudioError> = std::result::Result<T, E>;

fn corrupt<T>(msg: impl Into<String>) -> Result<T> {
    Err(AudioError::Corrupt(msg.into()))
}

/// Bounds-checked little-endian reader over a byte slice.
#[derive(Clone, Copy)]
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    what: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8], what: &'static str) -> Self {
        Self { bytes, pos: 0, what }
    }

    fn at(bytes: &'a [u8], pos: usize, what: &'static str) -> Self {
        Self { bytes, pos, what }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(n).filter(|&e| e <= self.bytes.len());
        match end {
            Some(end) => {
                let s = &self.bytes[self.pos..end];
                self.pos = end;
                Ok(s)
            }
            None => corrupt(format!("{}: truncated at offset {} (need {n} bytes)", self.what, self.pos)),
        }
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        Ok(self.take(N)?.try_into().expect("length checked"))
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }
    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.array()?))
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.array()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.array()?))
    }
}

fn expect_magic(c: &mut Cursor, expected: &'static str) -> Result<()> {
    let found = c.array::<4>()?;
    if found != expected.as_bytes() {
        return Err(AudioError::BadMagic { expected, found });
    }
    Ok(())
}

fn non_negative(v: i32, what: &str) -> Result<usize> {
    usize::try_from(v).or_else(|_| corrupt(format!("{what} is negative ({v})")))
}

// ------------------------------------------------------------------------------------------------
// Banks
// ------------------------------------------------------------------------------------------------

/// Size of one on-disk patch record.
pub const PATCH_RECORD_SIZE: usize = 56;
/// Size of the bank header that precedes the patch records.
pub const BANK_HEADER_SIZE: usize = 12;

/// Patch flag: the patch loops between `loop_start` and `loop_end`.
pub const PATCH_FLAG_LOOPING: u16 = 0x1;
/// Patch flag: "stretch" (named in HW1's headers; no observable use in the mixer).
pub const PATCH_FLAG_STRETCH: u16 = 0x2;

/// The `WAVEFORMATEX` describing the source recording / intended output of a patch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaveFormat {
    pub format_tag: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub extra_size: u16,
}

/// How a payload is encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// Frequency-quantised transform codec, `bitrate` bits per channel per 11.6 ms block.
    Fquant { bitrate: u16 },
    /// Raw 16-bit PCM (bitrate field zero). Not used by any shipped Cataclysm bank.
    Pcm16,
}

/// One sound in a `.bnk`.
#[derive(Debug, Clone, PartialEq)]
pub struct Patch<'a> {
    /// Index of the patch within its bank (what event LUTs refer to).
    pub index: usize,
    /// Record tag, `PCH0` in every shipped bank.
    pub tag: [u8; 4],
    pub priority: u32,
    pub pitch: i32,
    /// Payload offset from the start of the bank file.
    pub data_offset: u32,
    pub data_size: u32,
    /// Loop start, relative to the payload start (bytes; block aligned).
    pub loop_start: u32,
    /// Loop end, relative to the payload start (bytes; block aligned).
    pub loop_end: u32,
    /// Codec bits per block per channel; zero means PCM.
    pub bitrate: u16,
    pub flags: u16,
    pub volume: u8,
    pub pan: i8,
    pub format: WaveFormat,
    /// The encoded payload.
    pub data: &'a [u8],
}

impl Patch<'_> {
    pub fn looping(&self) -> bool {
        self.flags & PATCH_FLAG_LOOPING != 0
    }

    pub fn encoding(&self) -> Encoding {
        if self.bitrate == 0 {
            Encoding::Pcm16
        } else {
            Encoding::Fquant { bitrate: self.bitrate }
        }
    }

    pub fn channels(&self) -> u16 {
        self.format.channels.max(1)
    }

    /// Number of codec blocks (per channel), or PCM frames for PCM patches.
    pub fn num_blocks(&self) -> usize {
        match self.encoding() {
            Encoding::Fquant { bitrate } => self.data.len() / (usize::from(bitrate / 8) * usize::from(self.channels())),
            Encoding::Pcm16 => self.data.len() / (2 * usize::from(self.channels())),
        }
    }

    /// Playing time in seconds implied by the length fields.
    pub fn duration_secs(&self) -> f64 {
        match self.encoding() {
            Encoding::Fquant { .. } => self.num_blocks() as f64 * BLOCK_SECONDS,
            Encoding::Pcm16 => self.num_blocks() as f64 / f64::from(self.format.sample_rate.max(1)),
        }
    }

    /// Decode the patch. PCM patches are returned unchanged at their own rate.
    pub fn decode(&self, rate: OutputRate) -> Result<DecodedAudio> {
        match self.encoding() {
            Encoding::Fquant { bitrate } => decode_fquant(self.data, bitrate, self.channels(), rate),
            Encoding::Pcm16 => Ok(DecodedAudio {
                sample_rate: self.format.sample_rate,
                channels: self.channels(),
                samples: pcm16_samples(self.data).into_iter().map(f32::from).collect(),
            }),
        }
    }
}

/// Interpret raw bytes as little-endian signed 16-bit PCM (a trailing odd byte is ignored).
pub fn pcm16_samples(data: &[u8]) -> Vec<i16> {
    data.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect()
}

/// A parsed `.bnk` sound-effect bank (`BNK0`).
#[derive(Debug, Clone)]
pub struct Bank<'a> {
    /// Generation checksum; matches the checksum of the event LUT that indexes this bank.
    pub checksum: u32,
    pub patches: Vec<Patch<'a>>,
}

impl<'a> Bank<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes, "bank");
        expect_magic(&mut c, "BNK0")?;
        let checksum = c.u32()?;
        let count = non_negative(c.i32()?, "bank patch count")?;
        if count.saturating_mul(PATCH_RECORD_SIZE) > bytes.len() - BANK_HEADER_SIZE {
            return corrupt(format!("bank: {count} patch records exceed the file"));
        }
        let mut patches = Vec::with_capacity(count);
        for index in 0..count {
            let tag = c.array::<4>()?;
            let priority = c.u32()?;
            let pitch = c.i32()?;
            let data_offset = c.u32()?;
            let data_size = c.u32()?;
            let loop_start = c.u32()?;
            let loop_end = c.u32()?;
            let bitrate = c.u16()?;
            let flags = c.u16()?;
            let volume = c.u8()?;
            let pan = c.u8()? as i8;
            c.take(2)?; // padding
            let format = WaveFormat {
                format_tag: c.u16()?,
                channels: c.u16()?,
                sample_rate: c.u32()?,
                avg_bytes_per_sec: c.u32()?,
                block_align: c.u16()?,
                bits_per_sample: c.u16()?,
                extra_size: c.u16()?,
            };
            c.take(2)?; // padding

            let start = data_offset as usize;
            let end = start.checked_add(data_size as usize).filter(|&e| e <= bytes.len());
            let Some(end) = end else {
                return corrupt(format!("bank patch {index}: payload {start}+{data_size} outside file"));
            };
            if !bitrate.is_multiple_of(8) {
                return corrupt(format!("bank patch {index}: bitrate {bitrate} is not a whole number of bytes"));
            }
            if loop_start > loop_end || loop_end > data_size {
                return corrupt(format!("bank patch {index}: loop {loop_start}..{loop_end} outside payload of {data_size}"));
            }
            patches.push(Patch {
                index,
                tag,
                priority,
                pitch,
                data_offset,
                data_size,
                loop_start,
                loop_end,
                bitrate,
                flags,
                volume,
                pan,
                format,
                data: &bytes[start..end],
            });
        }
        Ok(Self { checksum, patches })
    }
}

// ------------------------------------------------------------------------------------------------
// Lookup tables
// ------------------------------------------------------------------------------------------------

/// Sound-effect event table (`PKG0`): for each (object, event) pair, up to `num_variations`
/// patch indices into the matching bank.
#[derive(Debug, Clone, PartialEq)]
pub struct SfxLut {
    pub checksum: u32,
    pub num_variations: u16,
    pub num_events: u16,
    pub num_objects: u16,
    /// `num_objects * num_events` rows of `1 + num_variations` values: count, then patch indices.
    pub table: Vec<i16>,
}

impl SfxLut {
    fn stride(&self) -> usize {
        usize::from(self.num_variations) + 1
    }

    /// Raw row for (object, event): `[count, patch, patch, ...]`.
    pub fn row(&self, object: usize, event: usize) -> Option<&[i16]> {
        if object >= usize::from(self.num_objects) || event >= usize::from(self.num_events) {
            return None;
        }
        let start = (object * usize::from(self.num_events) + event) * self.stride();
        self.table.get(start..start + self.stride())
    }

    /// The patch indices that may play for (object, event); empty when the event has no sound.
    pub fn variations(&self, object: usize, event: usize) -> &[i16] {
        match self.row(object, event) {
            Some(row) => {
                let n = usize::try_from(row[0]).unwrap_or(0).min(row.len() - 1);
                &row[1..1 + n]
            }
            None => &[],
        }
    }
}

/// Speech sentence table (`LUT1`): one row per (actor, event).
#[derive(Debug, Clone, PartialEq)]
pub struct SentenceLut {
    pub checksum: u32,
    pub num_columns: u16,
    pub num_events: u16,
    pub num_actors: u16,
    /// Codec bitrate of each actor's speech in the `.vce`.
    pub comp_bitrates: Vec<u16>,
    pub table: Vec<i16>,
}

/// Typed view of a four-column sentence row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sentence {
    pub priority: i16,
    pub num_variations: i16,
    /// Number of values the sentence's variable can take (rows per variation in the phrase LUT).
    pub max_variable: i16,
    /// First phrase LUT row of this sentence.
    pub phrase_index: i16,
}

impl SentenceLut {
    pub fn row(&self, actor: usize, event: usize) -> Option<&[i16]> {
        if actor >= usize::from(self.num_actors) || event >= usize::from(self.num_events) {
            return None;
        }
        let w = usize::from(self.num_columns);
        let start = (actor * usize::from(self.num_events) + event) * w;
        self.table.get(start..start + w)
    }

    pub fn sentence(&self, actor: usize, event: usize) -> Option<Sentence> {
        let r = self.row(actor, event)?;
        (r.len() >= 4).then(|| Sentence { priority: r[0], num_variations: r[1], max_variable: r[2], phrase_index: r[3] })
    }
}

/// Speech phrase table (`LUT2`): each row is one spoken sentence variant built from fragments.
#[derive(Debug, Clone, PartialEq)]
pub struct PhraseLut {
    pub checksum: u32,
    pub num_columns: u16,
    pub phrases: Vec<Phrase>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Phrase {
    /// Approximate playing time in milliseconds (0 for placeholder rows).
    pub duration_ms: i32,
    /// Variation weighting, 0..=100 (low byte of the second column).
    pub probability: u8,
    /// Fragment offsets into the `.vce` file; each points at a record (`INFO` or `DATA`).
    pub offsets: Vec<i32>,
    /// Rows that reference no audio (count of 1 but offset -1, or count 0).
    pub placeholder: bool,
}

/// An integer table (`VOL1` volume falloff, `RNG1` range breakpoints).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableLut {
    pub columns: u16,
    pub rows: u16,
    pub values: Vec<i32>,
}

/// A float table (`FRQ1`, per-band EQ attenuation against distance).
#[derive(Debug, Clone, PartialEq)]
pub struct FloatTableLut {
    pub columns: u16,
    pub rows: u16,
    pub values: Vec<f32>,
}

impl TableLut {
    pub fn get(&self, row: usize, col: usize) -> Option<i32> {
        (col < usize::from(self.columns)).then(|| self.values.get(row * usize::from(self.columns) + col).copied())?
    }
}

impl FloatTableLut {
    pub fn get(&self, row: usize, col: usize) -> Option<f32> {
        (col < usize::from(self.columns)).then(|| self.values.get(row * usize::from(self.columns) + col).copied())?
    }
}

/// Any of the `.lut` files in `soundfx\`, identified by magic.
#[derive(Debug, Clone, PartialEq)]
pub enum Lut {
    Sfx(SfxLut),
    Sentence(SentenceLut),
    Phrase(PhraseLut),
    Volume(TableLut),
    Range(TableLut),
    Frequency(FloatTableLut),
}

impl Lut {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes, "lut");
        let magic = c.array::<4>()?;
        match &magic {
            b"PKG0" => parse_sfx(c).map(Lut::Sfx),
            b"LUT1" => parse_sentence(c).map(Lut::Sentence),
            b"LUT2" => parse_phrase(c).map(Lut::Phrase),
            b"VOL1" => parse_table(c).map(Lut::Volume),
            b"RNG1" => parse_table(c).map(Lut::Range),
            b"FRQ1" => parse_float_table(c).map(Lut::Frequency),
            _ => Err(AudioError::BadMagic { expected: "PKG0/LUT1/LUT2/VOL1/RNG1/FRQ1", found: magic }),
        }
    }

    /// The four-character tag of this table kind.
    pub fn magic(&self) -> &'static str {
        match self {
            Lut::Sfx(_) => "PKG0",
            Lut::Sentence(_) => "LUT1",
            Lut::Phrase(_) => "LUT2",
            Lut::Volume(_) => "VOL1",
            Lut::Range(_) => "RNG1",
            Lut::Frequency(_) => "FRQ1",
        }
    }

    /// Generation checksum, for the tables that carry one.
    pub fn checksum(&self) -> Option<u32> {
        match self {
            Lut::Sfx(l) => Some(l.checksum),
            Lut::Sentence(l) => Some(l.checksum),
            Lut::Phrase(l) => Some(l.checksum),
            _ => None,
        }
    }
}

fn expect_end(c: &Cursor, what: &str) -> Result<()> {
    if c.pos != c.bytes.len() {
        return corrupt(format!("{what}: {} trailing bytes", c.bytes.len() - c.pos));
    }
    Ok(())
}

fn read_i16s(c: &mut Cursor, n: usize) -> Result<Vec<i16>> {
    let raw = c.take(n.checked_mul(2).ok_or_else(|| AudioError::Corrupt("table size overflow".into()))?)?;
    Ok(raw.as_chunks::<2>().0.iter().map(|&b| i16::from_le_bytes(b)).collect())
}

fn parse_sfx(mut c: Cursor) -> Result<SfxLut> {
    let checksum = c.u32()?;
    let num_variations = c.u16()?;
    let num_events = c.u16()?;
    let num_objects = c.u16()?;
    let n = (usize::from(num_variations) + 1) * usize::from(num_events) * usize::from(num_objects);
    let table = read_i16s(&mut c, n)?;
    expect_end(&c, "PKG0 lut")?;
    Ok(SfxLut { checksum, num_variations, num_events, num_objects, table })
}

fn parse_sentence(mut c: Cursor) -> Result<SentenceLut> {
    let checksum = c.u32()?;
    let num_columns = c.u16()?;
    let num_events = c.u16()?;
    let num_actors = c.u16()?;
    let n = usize::from(num_columns) * usize::from(num_events) * usize::from(num_actors);
    // HW1 hard-coded four bitrates; Cataclysm stores one per actor. Derive the count from the size.
    let remaining = c.bytes.len() - c.pos;
    let Some(extra) = remaining.checked_sub(n * 2).filter(|e| e.is_multiple_of(2)) else {
        return corrupt(format!("LUT1: {remaining} bytes left for a {n}-entry table"));
    };
    let comp_bitrates = (0..extra / 2).map(|_| c.u16()).collect::<Result<Vec<_>>>()?;
    let table = read_i16s(&mut c, n)?;
    expect_end(&c, "LUT1 lut")?;
    Ok(SentenceLut { checksum, num_columns, num_events, num_actors, comp_bitrates, table })
}

fn parse_phrase(mut c: Cursor) -> Result<PhraseLut> {
    let checksum = c.u32()?;
    let num_columns = c.u16()?;
    let num_rows = c.u16()?;
    if num_columns < 3 {
        return corrupt(format!("LUT2: only {num_columns} columns"));
    }
    let mut phrases = Vec::with_capacity(usize::from(num_rows));
    for row in 0..num_rows {
        let duration_ms = c.i32()?;
        let probability = c.u32()? as u8;
        let count = usize::from(c.u32()? as u8);
        let slots = (0..num_columns - 3).map(|_| c.i32()).collect::<Result<Vec<_>>>()?;
        if count > slots.len() {
            return corrupt(format!("LUT2 row {row}: {count} offsets but {} slots", slots.len()));
        }
        let offsets: Vec<i32> = slots[..count].to_vec();
        let placeholder = offsets.is_empty() || offsets.iter().any(|&o| o < 0);
        phrases.push(Phrase { duration_ms, probability, offsets, placeholder });
    }
    expect_end(&c, "LUT2 lut")?;
    Ok(PhraseLut { checksum, num_columns, phrases })
}

fn parse_table(mut c: Cursor) -> Result<TableLut> {
    let columns = c.u16()?;
    let rows = c.u16()?;
    let values = (0..usize::from(columns) * usize::from(rows)).map(|_| c.i32()).collect::<Result<Vec<_>>>()?;
    expect_end(&c, "table lut")?;
    Ok(TableLut { columns, rows, values })
}

fn parse_float_table(mut c: Cursor) -> Result<FloatTableLut> {
    let columns = c.u16()?;
    let rows = c.u16()?;
    let values = (0..usize::from(columns) * usize::from(rows)).map(|_| c.f32()).collect::<Result<Vec<_>>>()?;
    expect_end(&c, "FRQ1 lut")?;
    Ok(FloatTableLut { columns, rows, values })
}

// ------------------------------------------------------------------------------------------------
// Streams: music index, music data, speech banks
// ------------------------------------------------------------------------------------------------

/// Music stream flag: loop the track.
pub const MUSIC_FLAG_LOOPING: u16 = 0x1;

/// One track in the music index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MusicStream {
    /// Offset of the track's `DATA` record in the `.wxd`.
    pub offset: u32,
    pub flags: u16,
    /// Codec bits per block per channel.
    pub bitrate: u16,
    pub volume: u8,
    pub channels: u8,
}

/// `.wxh` music index (`WXH1`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MusicIndex {
    /// Must equal the checksum in the matching `.wxd`.
    pub checksum: u32,
    pub streams: Vec<MusicStream>,
}

impl MusicIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes, "wxh");
        expect_magic(&mut c, "WXH1")?;
        let checksum = c.u32()?;
        let count = non_negative(c.i32()?, "wxh stream count")?;
        if count.saturating_mul(12) > bytes.len() {
            return corrupt(format!("wxh: {count} streams exceed the file"));
        }
        let mut streams = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = c.u32()?;
            let flags = c.u16()?;
            let bitrate = c.u16()?;
            let volume = c.u8()?;
            let channels = c.u8()?;
            c.take(2)?; // padding
            streams.push(MusicStream { offset, flags, bitrate, volume, channels });
        }
        expect_end(&c, "wxh")?;
        Ok(Self { checksum, streams })
    }
}

/// Kind of a stream container file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamFileKind {
    /// `WXD1` music data.
    Music,
    /// `VCE0` speech data.
    Speech,
}

/// A `DATA` record, optionally preceded by an `INFO` record holding a subtitle.
#[derive(Clone, PartialEq, Eq)]
pub struct StreamRecord<'a> {
    /// Offset of the first tag (`INFO` if present, else `DATA`).
    pub offset: usize,
    /// Subtitle bytes (Latin-1, not terminated); `None` without `INFO`, empty for `INFO` of size 0.
    pub subtitle: Option<&'a [u8]>,
    /// Offset of the encoded payload (just past the `DATA` header).
    pub data_offset: usize,
    pub data: &'a [u8],
}

impl fmt::Debug for StreamRecord<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StreamRecord")
            .field("offset", &self.offset)
            .field("subtitle", &self.subtitle_text())
            .field("data_offset", &self.data_offset)
            .field("data_len", &self.data.len())
            .finish()
    }
}

impl StreamRecord<'_> {
    /// Offset just past this record.
    pub fn end(&self) -> usize {
        self.data_offset + self.data.len()
    }

    pub fn subtitle_text(&self) -> Option<String> {
        self.subtitle.map(|s| s.iter().map(|&b| char::from(b)).collect())
    }
}

/// A `WXD1` or `VCE0` file: 8-byte header (magic, checksum) followed by records.
#[derive(Debug, Clone, Copy)]
pub struct StreamFile<'a> {
    pub kind: StreamFileKind,
    pub checksum: u32,
    pub bytes: &'a [u8],
}

/// Length of the `WXD1`/`VCE0` header.
pub const STREAM_FILE_HEADER_SIZE: usize = 8;

impl<'a> StreamFile<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let mut c = Cursor::new(bytes, "stream file");
        let magic = c.array::<4>()?;
        let kind = match &magic {
            b"WXD1" => StreamFileKind::Music,
            b"VCE0" => StreamFileKind::Speech,
            _ => return Err(AudioError::BadMagic { expected: "WXD1/VCE0", found: magic }),
        };
        Ok(Self { kind, checksum: c.u32()?, bytes })
    }

    /// Read the record starting at `offset` (as stored in the music index or phrase LUT).
    pub fn record_at(&self, offset: usize) -> Result<StreamRecord<'a>> {
        let mut c = Cursor::at(self.bytes, offset, "stream record");
        if offset > self.bytes.len() {
            return corrupt(format!("stream record offset {offset} past end of file"));
        }
        let mut tag = c.array::<4>()?;
        let mut size = non_negative(c.i32()?, "stream record size")?;
        let mut subtitle = None;
        if &tag == b"INFO" {
            subtitle = Some(c.take(size)?);
            tag = c.array::<4>()?;
            size = non_negative(c.i32()?, "stream record size")?;
        }
        if &tag != b"DATA" {
            return corrupt(format!("stream record at {offset}: expected DATA, found {tag:?}"));
        }
        let data_offset = c.pos;
        let data = c.take(size)?;
        Ok(StreamRecord { offset, subtitle, data_offset, data })
    }

    /// Walk every record from the header to the end of the file.
    pub fn records(&self) -> Result<Vec<StreamRecord<'a>>> {
        let mut out = Vec::new();
        let mut pos = STREAM_FILE_HEADER_SIZE;
        while pos < self.bytes.len() {
            let r = self.record_at(pos)?;
            pos = r.end();
            out.push(r);
        }
        Ok(out)
    }
}

/// A music track resolved against its `.wxd`.
#[derive(Debug, Clone)]
pub struct MusicTrack<'a> {
    pub index: usize,
    pub info: MusicStream,
    pub record: StreamRecord<'a>,
}

impl MusicTrack<'_> {
    pub fn num_blocks(&self) -> usize {
        self.record.data.len() / (usize::from(self.info.bitrate / 8) * usize::from(self.info.channels.max(1)))
    }

    pub fn duration_secs(&self) -> f64 {
        self.num_blocks() as f64 * BLOCK_SECONDS
    }

    pub fn decode(&self, rate: OutputRate) -> Result<DecodedAudio> {
        decode_fquant(self.record.data, self.info.bitrate, u16::from(self.info.channels.max(1)), rate)
    }
}

/// Resolve every track of `index` inside the `.wxd` bytes, checking the checksums agree.
pub fn music_tracks<'a>(index: &MusicIndex, wxd: &'a [u8]) -> Result<Vec<MusicTrack<'a>>> {
    let file = StreamFile::parse(wxd)?;
    if file.kind != StreamFileKind::Music {
        return corrupt("music data file is not WXD1");
    }
    if file.checksum != index.checksum {
        return corrupt(format!("wxd checksum {:#x} != wxh checksum {:#x}", file.checksum, index.checksum));
    }
    index
        .streams
        .iter()
        .enumerate()
        .map(|(i, info)| {
            if info.bitrate == 0 || !info.bitrate.is_multiple_of(8) {
                return corrupt(format!("music stream {i}: bad bitrate {}", info.bitrate));
            }
            Ok(MusicTrack { index: i, info: *info, record: file.record_at(info.offset as usize)? })
        })
        .collect()
}

/// A `.vce` speech bank: every speech fragment, in file order.
#[derive(Debug, Clone)]
pub struct SpeechBank<'a> {
    pub checksum: u32,
    pub phrases: Vec<StreamRecord<'a>>,
}

impl<'a> SpeechBank<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        let file = StreamFile::parse(bytes)?;
        if file.kind != StreamFileKind::Speech {
            return corrupt("speech file is not VCE0");
        }
        Ok(Self { checksum: file.checksum, phrases: file.records()? })
    }

    /// Find the fragment a phrase LUT offset refers to.
    pub fn find(&self, offset: usize) -> Option<&StreamRecord<'a>> {
        self.phrases.binary_search_by_key(&offset, |r| r.offset).ok().map(|i| &self.phrases[i])
    }
}

// ------------------------------------------------------------------------------------------------
// Decoder
// ------------------------------------------------------------------------------------------------

/// Duration of one codec block (two MDCT hops) in seconds: 512 samples at 44.1 kHz.
pub const BLOCK_SECONDS: f64 = 512.0 / 44_100.0;

/// The largest block the decoder will read, in bytes; longer blocks are truncated.
const MAX_BLOCK_BYTES: usize = 260;
/// Number of frequency bins in the widest (44.1 kHz) configuration.
const MAX_BINS: usize = 256;
/// Critical band edges (bin indices) shared by the exponent groups.
const BAND_EDGES: [u16; 27] = [
    0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 15, 17, 20, 23, 27, 31, 37, 43, 51, 62, 74, 89, 110, 139, 180, 256,
];
/// Exponents at or above this value carry no audible coefficient.
const EXPONENT_LIMIT: usize = 6;

/// Output sample rate; each selects how many bins of each frame are synthesised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputRate {
    Hz11025,
    /// What the game's mixer runs at.
    Hz22050,
    Hz44100,
}

impl OutputRate {
    pub fn hz(self) -> u32 {
        match self {
            OutputRate::Hz11025 => 11_025,
            OutputRate::Hz22050 => 22_050,
            OutputRate::Hz44100 => 44_100,
        }
    }

    /// Bins per frame (= samples per hop).
    pub fn bins(self) -> usize {
        match self {
            OutputRate::Hz11025 => 64,
            OutputRate::Hz22050 => 128,
            OutputRate::Hz44100 => 256,
        }
    }

    /// Samples produced per channel by one block.
    pub fn samples_per_block(self) -> usize {
        2 * self.bins()
    }
}

/// Synthesis window applied before overlap-add.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SynthesisWindow {
    /// `sin(π(i + ½)/n)`: the symmetric sine window that cancels time-domain aliasing exactly.
    /// Measurably reduces the small discontinuity at every hop boundary that `Game` leaves in
    /// loud low-frequency material; offered as an enhancement.
    Symmetric,
    /// `sin(πi/n)`: what the original mixer uses (half a sample early, so slightly asymmetric).
    /// The default, because the recreation reproduces the original output first.
    #[default]
    Game,
}

/// Decoded audio as interleaved floating-point samples on a 16-bit scale.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAudio {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl DecodedAudio {
    pub fn frames(&self) -> usize {
        self.samples.len() / usize::from(self.channels.max(1))
    }

    pub fn duration_secs(&self) -> f64 {
        self.frames() as f64 / f64::from(self.sample_rate)
    }

    /// Convert to 16-bit the way the game's output stage does: round half to even, then hard-clamp.
    pub fn to_i16(&self) -> Vec<i16> {
        self.samples.iter().map(|&s| s.round_ties_even().clamp(-32768.0, 32767.0) as i16).collect()
    }

    /// Convert to 16-bit through [`soft_limit`] instead of a hard clamp (an enhancement, not original).
    pub fn to_i16_soft(&self) -> Vec<i16> {
        self.samples.iter().map(|&s| soft_limit(s).round().clamp(-32768.0, 32767.0) as i16).collect()
    }

    /// A complete RIFF/WAVE file (16-bit PCM) of [`DecodedAudio::to_i16`].
    pub fn to_wav(&self) -> Vec<u8> {
        let pcm = self.to_i16();
        let data_len = (pcm.len() * 2) as u32;
        let block_align = self.channels * 2;
        let mut out = Vec::with_capacity(44 + pcm.len() * 2);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVEfmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&self.channels.to_le_bytes());
        out.extend_from_slice(&self.sample_rate.to_le_bytes());
        out.extend_from_slice(&(self.sample_rate * u32::from(block_align)).to_le_bytes());
        out.extend_from_slice(&block_align.to_le_bytes());
        out.extend_from_slice(&16u16.to_le_bytes());
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm {
            out.extend_from_slice(&s.to_le_bytes());
        }
        out
    }
}

/// Logarithmic knee above half of full scale. Not in the original mixer (which hard-clamps); offered as an
/// optional enhancement for loud mixes.
pub fn soft_limit(x: f32) -> f32 {
    const KNEE: f32 = 0.5 * 32767.0;
    let over = x.abs() - KNEE;
    if over <= 0.0 {
        x
    } else {
        (KNEE + (1.0 + 0.75 * over / KNEE).ln() * KNEE).copysign(x)
    }
}

/// Decode a whole codec payload. `data` holds blocks of `bitrate / 8` bytes, channels interleaved
/// block by block. A trailing partial frame is ignored.
pub fn decode_fquant(data: &[u8], bitrate: u16, channels: u16, rate: OutputRate) -> Result<DecodedAudio> {
    decode_fquant_with(data, bitrate, channels, rate, SynthesisWindow::default())
}

/// [`decode_fquant`] with an explicit synthesis window.
pub fn decode_fquant_with(
    data: &[u8],
    bitrate: u16,
    channels: u16,
    rate: OutputRate,
    window: SynthesisWindow,
) -> Result<DecodedAudio> {
    if bitrate == 0 || !bitrate.is_multiple_of(8) {
        return corrupt(format!("invalid codec bitrate {bitrate}"));
    }
    if channels == 0 {
        return corrupt("zero channels");
    }
    let block = usize::from(bitrate / 8);
    let nch = usize::from(channels);
    let frames = data.len() / (block * nch);
    let per_block = rate.samples_per_block();

    let mut decoder = FqDecoder::new(rate, nch, window);
    let mut samples = vec![0.0f32; frames * per_block * nch];
    let mut chan_buf = vec![0.0f32; per_block];
    for (f, frame) in data.chunks_exact(block * nch).enumerate() {
        for (ch, blk) in frame.chunks_exact(block).enumerate() {
            decoder.decode_block(ch, blk, u32::from(bitrate), &mut chan_buf);
            let base = f * per_block * nch;
            for (i, &s) in chan_buf.iter().enumerate() {
                samples[base + i * nch + ch] = s;
            }
        }
    }
    Ok(DecodedAudio { sample_rate: rate.hz(), channels, samples })
}

/// Little-endian bit reader over one zero-padded block.
struct BlockBits {
    buf: [u8; MAX_BLOCK_BYTES + 4],
    pos: usize,
    limit: usize,
}

impl BlockBits {
    fn new(block: &[u8], bitrate_bits: u32) -> Self {
        let mut buf = [0u8; MAX_BLOCK_BYTES + 4];
        let n = block.len().min(MAX_BLOCK_BYTES);
        buf[..n].copy_from_slice(&block[..n]);
        let limit = (bitrate_bits as usize).min(MAX_BLOCK_BYTES * 8);
        Self { buf, pos: 0, limit }
    }

    fn more(&self) -> bool {
        self.pos < self.limit
    }

    /// Next `n` bits (n <= 24), least significant first.
    fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let word = u32::from_le_bytes([self.buf[byte], self.buf[byte + 1], self.buf[byte + 2], self.buf[byte + 3]]);
        self.pos += n as usize;
        (word >> ((self.pos - n as usize) & 7)) & ((1u32 << n) - 1)
    }

    /// Sign-magnitude value of `n` bits: the top bit is the sign.
    fn read_signed(&mut self, n: u32) -> i32 {
        let v = self.read(n);
        let magnitude = (v & ((1u32 << (n - 1)) - 1)) as i32;
        if v >> (n - 1) != 0 {
            -magnitude
        } else {
            magnitude
        }
    }
}

/// Per-channel decoder state carried from block to block.
#[derive(Clone)]
struct ChannelState {
    exponents: [u8; MAX_BINS],
    primary: [f32; MAX_BINS],
    secondary: [f32; MAX_BINS],
    /// Second frame's synthesis output, overlapped with the next block.
    carry: Vec<f32>,
}

/// Streaming decoder for one payload (any number of channels).
pub struct FqDecoder {
    rate: OutputRate,
    imdct: Imdct,
    window: Vec<f32>,
    step_scale: [f32; EXPONENT_LIMIT],
    channels: Vec<ChannelState>,
    highest_bin: usize,
}

impl FqDecoder {
    pub fn new(rate: OutputRate, channels: usize, window: SynthesisWindow) -> Self {
        let n = rate.samples_per_block();
        let phase = match window {
            SynthesisWindow::Symmetric => 0.5,
            SynthesisWindow::Game => 0.0,
        };
        let window = (0..n).map(|i| (PI * (i as f64 + phase) / n as f64).sin() as f32).collect();
        // Quantiser step for exponent e: 10^(e+1) spread over the 2^(e+1)-1 magnitude levels.
        let step_scale = std::array::from_fn(|e| (10f64.powi(e as i32 + 1) / f64::from((2u32 << e) - 1)) as f32);
        let state = ChannelState {
            exponents: [0; MAX_BINS],
            primary: [0.0; MAX_BINS],
            secondary: [0.0; MAX_BINS],
            carry: vec![0.0; n],
        };
        Self { rate, imdct: Imdct::new(n), window, step_scale, channels: vec![state; channels], highest_bin: 0 }
    }

    pub fn rate(&self) -> OutputRate {
        self.rate
    }

    /// Highest coefficient index seen with a non-zero value so far (including bins above the
    /// output bandwidth, which are dropped).
    pub fn highest_bin(&self) -> usize {
        self.highest_bin
    }

    /// Decode one block of channel `ch` into `out` (`rate.samples_per_block()` samples).
    pub fn decode_block(&mut self, ch: usize, block: &[u8], bitrate_bits: u32, out: &mut [f32]) {
        let bins = self.rate.bins();
        let top = dequantize(block, bitrate_bits, bins, &self.step_scale, &mut self.channels[ch]);
        self.highest_bin = self.highest_bin.max(top);
        self.synthesize(ch, out);
    }

    /// Inverse-transform the channel's two pending frames and overlap-add them onto the carry.
    fn synthesize(&mut self, ch: usize, out: &mut [f32]) {
        let bins = self.rate.bins();
        let n = 2 * bins;
        let state = &mut self.channels[ch];
        let mut first = [0.0f32; 2 * MAX_BINS];
        let first = &mut first[..n];
        out[..n].copy_from_slice(&state.carry);
        self.imdct.run(&state.primary[..bins], first);
        self.imdct.run(&state.secondary[..bins], &mut state.carry);
        let w = &self.window;
        for i in 0..bins {
            out[bins + i] = out[bins + i] * w[bins + i] + first[i] * w[i];
            state.carry[i] = first[bins + i] * w[bins + i] + state.carry[i] * w[i];
        }
    }
}

/// Unpack one block into `state.primary` / `state.secondary`. Returns the highest non-zero bin.
fn dequantize(block: &[u8], bitrate_bits: u32, bins: usize, steps: &[f32; EXPONENT_LIMIT], state: &mut ChannelState) -> usize {
    state.primary.fill(0.0);
    state.secondary.fill(0.0);
    let mut bits = BlockBits::new(block, bitrate_bits);
    let flags = bits.read(2);
    let band_step_bits = bits.read(3);
    let exponent_bits = bits.read(2);
    let bin_step_bits = bits.read(4);

    if flags & 1 != 0 {
        state.exponents.fill(0);
    }

    // Exponent updates, one per critical band, addressed by run-length band steps.
    if band_step_bits > 0 && exponent_bits > 0 {
        let mut band = 0usize;
        for i in 0..BAND_EDGES.len() - 1 {
            if !bits.more() {
                break;
            }
            let step = bits.read(band_step_bits) as usize;
            if i > 0 && step == 0 {
                break;
            }
            band += step;
            let e = bits.read(exponent_bits) as u8;
            if band + 1 >= BAND_EDGES.len() {
                break; // malformed: band index past the table
            }
            let range = usize::from(BAND_EDGES[band])..usize::from(BAND_EDGES[band + 1]);
            state.exponents[range].fill(e);
        }
    }

    let mut highest = 0;
    if bin_step_bits > 0 {
        let mut read_frame = |bits: &mut BlockBits, dest: &mut [f32; MAX_BINS], exps: &[u8; MAX_BINS]| {
            let mut bin = 0usize;
            for i in 0..MAX_BINS {
                if !bits.more() {
                    break;
                }
                let step = bits.read(bin_step_bits) as usize;
                if i > 0 && step == 0 {
                    break;
                }
                bin += step;
                if bin >= MAX_BINS {
                    break; // malformed
                }
                let e = usize::from(exps[bin]);
                let q = bits.read_signed(e as u32 + 2);
                if q != 0 && e < EXPONENT_LIMIT {
                    highest = highest.max(bin);
                    if bin < bins {
                        dest[bin] = steps[e] * q as f32;
                    }
                }
            }
        };
        read_frame(&mut bits, &mut state.primary, &state.exponents);
        if flags & 2 != 0 {
            state.secondary = state.primary;
        } else {
            read_frame(&mut bits, &mut state.secondary, &state.exponents);
        }
    }
    highest
}

/// Inverse MDCT of `n / 2` bins to `n` time samples via an `n / 4`-point complex FFT.
struct Imdct {
    n: usize,
    /// Pre/post rotation angles `2π(k + 1/8)/n`, as (cos, sin).
    rotation: Vec<(f32, f32)>,
    scale: f32,
    fft: Fft,
}

impl Imdct {
    fn new(n: usize) -> Self {
        let q = n / 4;
        let rotation = (0..q)
            .map(|k| {
                let a = 2.0 * PI * (k as f64 + 0.125) / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        // The game scales by 8/sqrt(n), which is 0.5 at its 22.05 kHz mixing size but makes the
        // 11.025 and 44.1 kHz renderings 3 dB louder/quieter. A constant keeps every rate at the
        // game's 22.05 kHz level.
        Self { n, rotation, scale: 0.5, fft: Fft::new(q) }
    }

    fn run(&self, spec: &[f32], out: &mut [f32]) {
        let n = self.n;
        let (h, q) = (n / 2, n / 4);
        let mut re = [0.0f32; MAX_BINS / 2];
        let mut im = [0.0f32; MAX_BINS / 2];
        for (k, &(c, s)) in self.rotation.iter().enumerate() {
            let a = 0.5 * spec[2 * k];
            let b = 0.5 * spec[h - 1 - 2 * k];
            re[k] = a * c + b * s;
            im[k] = b * c - a * s;
        }
        self.fft.forward(&mut re[..q], &mut im[..q]);

        let mut tmp = [0.0f32; 2 * MAX_BINS];
        for (k, &(c, s)) in self.rotation.iter().enumerate() {
            tmp[2 * k] = (re[k] * c + im[k] * s) * self.scale;
            tmp[h + 2 * k] = (im[k] * c - re[k] * s) * self.scale;
        }
        for i in (1..n).step_by(2) {
            tmp[i] = -tmp[n - 1 - i];
        }
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = if i < 3 * q { tmp[q + i] } else { -tmp[i - 3 * q] };
        }
    }
}

/// In-place iterative radix-2 complex FFT, forward sign (`e^{-2πi km/n}`).
struct Fft {
    n: usize,
    twiddles: Vec<(f32, f32)>,
    reversed: Vec<usize>,
}

impl Fft {
    fn new(n: usize) -> Self {
        assert!(n.is_power_of_two() && n >= 2);
        let twiddles = (0..n / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / n as f64;
                (a.cos() as f32, a.sin() as f32)
            })
            .collect();
        let shift = usize::BITS - n.trailing_zeros();
        let reversed = (0..n).map(|i| i.reverse_bits() >> shift).collect();
        Self { n, twiddles, reversed }
    }

    fn forward(&self, re: &mut [f32], im: &mut [f32]) {
        let n = self.n;
        for i in 0..n {
            let j = self.reversed[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= n {
            let half = len / 2;
            let stride = n / len;
            for start in (0..n).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.twiddles[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_direct_dft() {
        let n = 32;
        let fft = Fft::new(n);
        let xr: Vec<f32> = (0..n).map(|i| ((i * 7 % 11) as f32 - 5.0) * 0.3).collect();
        let xi: Vec<f32> = (0..n).map(|i| ((i * 3 % 5) as f32 - 2.0) * 0.1).collect();
        let (mut re, mut im) = (xr.clone(), xi.clone());
        fft.forward(&mut re, &mut im);
        for k in 0..n {
            let (mut sr, mut si) = (0.0f64, 0.0f64);
            for m in 0..n {
                let a = -2.0 * PI * (k * m) as f64 / n as f64;
                sr += f64::from(xr[m]) * a.cos() - f64::from(xi[m]) * a.sin();
                si += f64::from(xr[m]) * a.sin() + f64::from(xi[m]) * a.cos();
            }
            assert!((sr as f32 - re[k]).abs() < 1e-4 && (si as f32 - im[k]).abs() < 1e-4, "bin {k}");
        }
    }

    #[test]
    fn imdct_matches_direct_formula() {
        // The fast path must equal a direct IMDCT up to a constant scale and sign.
        let n = 64;
        let h = n / 2;
        let imdct = Imdct::new(n);
        let spec: Vec<f32> = (0..h).map(|k| ((k * 5 % 7) as f32 - 3.0) * 10.0).collect();
        let mut fast = vec![0.0f32; n];
        imdct.run(&spec, &mut fast);
        let direct: Vec<f64> = (0..n)
            .map(|i| {
                (0..h)
                    .map(|k| {
                        let ph = PI / h as f64 * (i as f64 + 0.5 + h as f64 / 2.0) * (k as f64 + 0.5);
                        f64::from(spec[k]) * ph.cos()
                    })
                    .sum()
            })
            .collect();
        let ratio = f64::from(fast[1]) / direct[1];
        for i in 0..n {
            assert!((f64::from(fast[i]) - ratio * direct[i]).abs() < 1e-2 * direct[i].abs().max(1.0), "sample {i}");
        }
    }

    /// Direct forward MDCT of one frame with the symmetric sine window (the presumed encoder).
    fn mdct(x: &[f64]) -> Vec<f32> {
        let n = x.len();
        let h = n / 2;
        (0..h)
            .map(|k| {
                (0..n)
                    .map(|i| {
                        let w = (PI * (i as f64 + 0.5) / n as f64).sin();
                        let ph = PI / h as f64 * (i as f64 + 0.5 + h as f64 / 2.0) * (k as f64 + 0.5);
                        w * x[i] * ph.cos()
                    })
                    .sum::<f64>() as f32
            })
            .collect()
    }

    #[test]
    fn symmetric_window_reconstructs_mdct_frames() {
        for rate in [OutputRate::Hz11025, OutputRate::Hz22050, OutputRate::Hz44100] {
            let h = rate.bins();
            let n = 2 * h;
            let blocks = 6;
            let len = (2 * blocks + 1) * h;
            let x: Vec<f64> =
                (0..len).map(|i| 3000.0 * (i as f64 * 0.07).sin() + 1500.0 * ((i * i % 97) as f64 / 97.0 - 0.5)).collect();
            let mut dec = FqDecoder::new(rate, 1, SynthesisWindow::Symmetric);
            let mut out = vec![0.0f32; blocks * n];
            for j in 0..blocks {
                let (p, s) = (2 * j, 2 * j + 1);
                dec.channels[0].primary[..h].copy_from_slice(&mdct(&x[p * h..p * h + n]));
                dec.channels[0].secondary[..h].copy_from_slice(&mdct(&x[s * h..s * h + n]));
                dec.synthesize(0, &mut out[j * n..(j + 1) * n]);
            }
            // Output lags the input by one hop; the first hop has no overlap partner.
            let pairs: Vec<(f64, f64)> = (n..blocks * n).map(|m| (f64::from(out[m]), x[m - h])).collect();
            let gain = pairs.iter().map(|(o, x)| o * x).sum::<f64>() / pairs.iter().map(|(_, x)| x * x).sum::<f64>();
            let err = pairs.iter().map(|(o, x)| (o - gain * x).abs()).fold(0.0, f64::max);
            // Perfect reconstruction: worst error well under 1/100 of a 16-bit step.
            assert!(gain > 1.0 && err / gain < 0.01, "{rate:?}: gain {gain}, max err {err}");
        }
    }

    #[test]
    fn silent_reset_block_decodes_to_zero() {
        let mut block = [0u8; 64];
        block[0] = 1; // reset exponents, nothing else
        let audio = decode_fquant(&block.repeat(4), 512, 1, OutputRate::Hz22050).unwrap();
        assert_eq!(audio.samples.len(), 4 * 256);
        assert!(audio.samples.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn sign_magnitude_unpack() {
        let mut b = BlockBits::new(&[0b0000_0111, 0, 0, 0], 32);
        assert_eq!(b.read_signed(3), -3);
        let mut b = BlockBits::new(&[0b0000_0011, 0, 0, 0], 32);
        assert_eq!(b.read_signed(3), 3);
    }

    #[test]
    fn soft_limit_is_monotonic_and_symmetric() {
        assert_eq!(soft_limit(1000.0), 1000.0);
        assert!(soft_limit(40_000.0) < 32767.0);
        assert_eq!(soft_limit(-20_000.0), -soft_limit(20_000.0));
    }
}
