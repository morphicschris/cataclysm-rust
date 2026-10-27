//! Remastered sound events: `sound/sfx/etg/<event>/<event>.lua` (written by the Relic Audio
//! Tool) and the `.fda` clips beside it.
//!
//! An event name such as `EXPLOSION/SMALL/ETG_EXPLODE_SMALL_PLASMA` names the folder
//! `sound/sfx/etg/explosion/small/etg_explode_small_plasma/`; the event's `.lua` gives a volume
//! envelope over distance from the listener (`envelope = { { distance, volume, ... }, ... }`) and
//! a random container plays one of the folder's clips.
//!
//! An `.fda` is an AIFF-C file (`FORM`/`AIFC`) whose `COMM` chunk names the compression
//! "Relic Codec v1.6"; its `SSND` data starts with a big-endian `u16` bit rate (bits per block)
//! followed by the blocks. That codec is the block-coded MDCT format of Cataclysm's own sound
//! banks (same critical bands, exponents and bit layout), so the game's mixer plays the blocks as
//! they are.

use crate::lua::{self, Value};

/// Root of the effect sounds in the archive.
pub const SOUND_ROOT: &str = "sound/sfx/etg";

/// The folder of a sound event (normalised path).
pub fn event_dir(name: &str) -> String {
    format!(
        "{SOUND_ROOT}/{}",
        name.trim()
            .trim_matches('/')
            .to_ascii_lowercase()
            .replace('\\', "/")
    )
}

/// A clip: mono or interleaved-stereo Relic blocks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fda {
    pub channels: u16,
    pub sample_rate: u32,
    /// Bits per block (`bitrate / 8` bytes per block and channel).
    pub bitrate: u16,
    /// The blocks.
    pub data: Vec<u8>,
}

fn be16(b: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(o)?, *b.get(o + 1)?]))
}

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes([
        *b.get(o)?,
        *b.get(o + 1)?,
        *b.get(o + 2)?,
        *b.get(o + 3)?,
    ]))
}

/// The integer part of an 80-bit IEEE extended float (AIFF sample rates).
fn extended_to_u32(b: &[u8]) -> Option<u32> {
    let exp = i32::from(be16(b, 0)? & 0x7fff) - 16383;
    let mant = u64::from(be32(b, 2)?) << 32 | u64::from(be32(b, 6)?);
    if !(0..64).contains(&exp) {
        return Some(0);
    }
    Some((mant >> (63 - exp)) as u32)
}

impl Fda {
    /// Parses an `.fda`; `None` for anything but Relic-coded AIFF-C.
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.get(..4)? != b"FORM" || b.get(8..12)? != b"AIFC" {
            return None;
        }
        let mut o = 12;
        let (mut channels, mut rate, mut relic, mut ssnd) = (0u16, 0u32, false, None);
        while o + 8 <= b.len() {
            let id = &b[o..o + 4];
            let size = be32(b, o + 4)? as usize;
            let body = o + 8;
            match id {
                b"COMM" => {
                    channels = be16(b, body)?;
                    rate = extended_to_u32(b.get(body + 8..body + 18)?)?;
                    let name_len = *b.get(body + 22)? as usize;
                    relic = b
                        .get(body + 23..body + 23 + name_len)?
                        .starts_with(b"Relic Codec");
                }
                b"SSND" => {
                    let start = body + 8 + be32(b, body)? as usize;
                    ssnd = Some((start, (body + size).min(b.len())));
                }
                _ => {}
            }
            o = body + size + (size & 1);
        }
        let (start, end) = ssnd?;
        if !relic || channels == 0 || start + 2 > end {
            return None;
        }
        Some(Self {
            channels,
            sample_rate: rate,
            bitrate: be16(b, start)?,
            data: b[start + 2..end].to_vec(),
        })
    }

    /// Bytes per block of one channel.
    pub fn block_bytes(&self) -> usize {
        usize::from(self.bitrate / 8).max(1)
    }

    /// The first channel's blocks (de-interleaved for stereo clips).
    pub fn mono(&self) -> Vec<u8> {
        if self.channels <= 1 {
            return self.data.clone();
        }
        let bb = self.block_bytes();
        self.data
            .chunks(bb * usize::from(self.channels))
            .flat_map(|f| f[..bb.min(f.len())].iter().copied())
            .collect()
    }
}

/// A sound event's volume envelope: `(distance, volume 0..1)` by increasing distance.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Envelope(pub Vec<(f32, f32)>);

impl Envelope {
    /// Reads the envelope from an event `.lua`.
    pub fn parse(src: &[u8]) -> Self {
        let c = lua::parse_bytes(src);
        let mut v: Vec<(f32, f32)> =
            c.global("envelope")
                .and_then(Value::as_table)
                .map_or_else(Vec::new, |t| {
                    t.items
                        .iter()
                        .filter_map(|p| {
                            let d = p.get("distance")?.as_num()? as f32;
                            let vol = p.get("volume")?.as_num()? as f32;
                            Some((d, vol))
                        })
                        .collect()
                });
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        Self(v)
    }

    /// Volume at `distance` (linear between points; silent beyond the last).
    pub fn volume(&self, distance: f32) -> f32 {
        let Some(first) = self.0.first() else {
            return 1.0;
        };
        if distance <= first.0 {
            return first.1;
        }
        for w in self.0.windows(2) {
            if distance <= w[1].0 {
                let f = (distance - w[0].0) / (w[1].0 - w[0].0).max(1e-6);
                return w[0].1 + (w[1].1 - w[0].1) * f;
            }
        }
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fda(channels: u16, payload: &[u8]) -> Vec<u8> {
        let mut comm = Vec::new();
        comm.extend_from_slice(&channels.to_be_bytes());
        comm.extend_from_slice(&2u32.to_be_bytes());
        comm.extend_from_slice(&16u16.to_be_bytes());
        comm.extend_from_slice(&[0x40, 0x0e, 0xac, 0x44, 0, 0, 0, 0, 0, 0]);
        comm.extend_from_slice(b"COMP");
        comm.push(16);
        comm.extend_from_slice(b"Relic Codec v1.6\0");
        let mut ssnd = vec![0u8; 8];
        ssnd.extend_from_slice(&16u16.to_be_bytes());
        ssnd.extend_from_slice(payload);
        let mut out = b"FORM\0\0\0\0AIFC".to_vec();
        for (id, body) in [(b"COMM", comm), (b"SSND", ssnd)] {
            out.extend_from_slice(id);
            out.extend_from_slice(&(body.len() as u32).to_be_bytes());
            out.extend_from_slice(&body);
            if body.len() & 1 == 1 {
                out.push(0);
            }
        }
        out
    }

    #[test]
    fn parses_relic_aifc() {
        let f = Fda::parse(&fda(1, &[1, 2, 3, 4])).unwrap();
        assert_eq!((f.channels, f.sample_rate, f.bitrate), (1, 44100, 16));
        assert_eq!(f.data, vec![1, 2, 3, 4]);
        let s = Fda::parse(&fda(2, &[1, 2, 3, 4, 5, 6, 7, 8])).unwrap();
        assert_eq!(s.mono(), vec![1, 2, 5, 6]);
    }

    #[test]
    fn envelope() {
        let e = Envelope::parse(b"envelope = {\n  {\n    distance = 0.000000,\n    volume = 0.5,\n    equalizer = { 1, 1, },\n  },\n  {\n    distance = 100,\n    volume = 0.1,\n  }\n}\ncontainer = 1");
        assert_eq!(e.0, vec![(0.0, 0.5), (100.0, 0.1)]);
        assert!((e.volume(50.0) - 0.3).abs() < 1e-6);
        assert_eq!(e.volume(101.0), 0.0);
        assert_eq!(
            event_dir("EXPLOSION/SMALL/ETG_EXPLODE_SMALL_PLASMA"),
            "sound/sfx/etg/explosion/small/etg_explode_small_plasma"
        );
    }
}
