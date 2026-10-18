//! `.mad` mesh animations: keyframed motion paths for the objects of a mesh, cut into named
//! animations (gun deploy, wings open, docking arms and so on).
//!
//! All values are little-endian. Cataclysm files are identical in layout to Homeworld 1's.
//!
//! Header (40 bytes), followed directly by the animation table:
//!
//! | offset | type    | field                                                          |
//! |--------|---------|----------------------------------------------------------------|
//! | 0      | [u8; 8] | identifier, `"Mellonn\0"`                                      |
//! | 8      | f32     | version, `0.91`                                                |
//! | 12     | u32     | string block length; the block is the last bytes of the file   |
//! | 16     | u32     | string block file offset (runtime pointer slot, informational) |
//! | 20     | f32     | total length of all animations, seconds                        |
//! | 24     | f32     | frames per second the animation was authored at                |
//! | 28     | u32     | object path count                                              |
//! | 32     | u32     | file offset of the object path table                           |
//! | 36     | u32     | animation count (at most 32)                                   |
//!
//! Animation (16 bytes): name (string block offset) `u32`, start time `f32`, end time `f32`,
//! flags `u32` (bit 0 = loops).
//!
//! Object path (48 bytes): name (string block offset) `u32`, CRC slot `u16` (zero on disk),
//! pad `u16`, animation membership bits `u32` (bit *n* = takes part in animation *n*), keyframe
//! count `u32`, then file offsets of: key times (`f32` each), tension/continuity/bias triples
//! (`f32×3` each) and six channel arrays (`f32` each) for x, y, z, heading, pitch, bank.
//!
//! Times are in seconds on one shared timeline; every animation is a `[start, end]` window of it.

use thiserror::Error;

pub const IDENTIFIER: &[u8; 8] = b"Mellonn\0";
pub const VERSION: f32 = 0.91;
pub const HEADER_LEN: usize = 40;
pub const ANIMATION_LEN: usize = 16;
pub const OBJECT_PATH_LEN: usize = 48;
pub const MAX_ANIMATIONS: usize = 32;

/// Animation flag: the animation loops.
pub const ANIM_LOOP: u32 = 0x1;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for a MAD header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 8]),
    #[error("unsupported version {0}")]
    BadVersion(f32),
    #[error("{0} animations, at most {MAX_ANIMATIONS} supported")]
    TooManyAnimations(u32),
    #[error(
        "{what} at offset {offset} with length {len} runs past end of file ({file_len} bytes)"
    )]
    OutOfBounds {
        what: &'static str,
        offset: usize,
        len: usize,
        file_len: usize,
    },
    #[error("string block of {len} bytes overlaps the header ({file_len} byte file)")]
    BadStringBlock { len: u32, file_len: usize },
    #[error("string at block offset {0} is out of range or unterminated")]
    BadString(u32),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq)]
pub struct Animation {
    pub name: String,
    /// Seconds.
    pub start_time: f32,
    /// Seconds.
    pub end_time: f32,
    pub flags: u32,
}

impl Animation {
    pub fn loops(&self) -> bool {
        self.flags & ANIM_LOOP != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Keyframe {
    /// Seconds.
    pub time: f32,
    /// Kochanek-Bartels tension, continuity, bias.
    pub tcb: [f32; 3],
    pub position: [f32; 3],
    /// Heading, pitch, bank.
    pub rotation: [f32; 3],
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPath {
    /// Name of the mesh object this path drives.
    pub name: String,
    /// Bit *n* set: the object moves in animation *n*.
    pub animation_bits: u32,
    pub keyframes: Vec<Keyframe>,
}

impl ObjectPath {
    pub fn in_animation(&self, index: usize) -> bool {
        index < 32 && self.animation_bits & (1 << index) != 0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Mad {
    pub version: f32,
    /// Seconds.
    pub length: f32,
    pub frames_per_second: f32,
    pub animations: Vec<Animation>,
    pub objects: Vec<ObjectPath>,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

struct Reader<'a> {
    data: &'a [u8],
    strings: &'a [u8],
}

impl<'a> Reader<'a> {
    fn floats(
        &self,
        what: &'static str,
        offset: u32,
        count: usize,
        width: usize,
    ) -> Result<Vec<f32>> {
        let oob = || Error::OutOfBounds {
            what,
            offset: offset as usize,
            len: count.saturating_mul(width).saturating_mul(4),
            file_len: self.data.len(),
        };
        let len = count.checked_mul(width * 4).ok_or_else(oob)?;
        let bytes = (offset as usize)
            .checked_add(len)
            .and_then(|end| self.data.get(offset as usize..end))
            .ok_or_else(oob)?;
        Ok(bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|&c| f32::from_le_bytes(c))
            .collect())
    }

    fn table(
        &self,
        what: &'static str,
        offset: u32,
        count: u32,
        stride: usize,
    ) -> Result<std::slice::ChunksExact<'a, u8>> {
        let oob = || Error::OutOfBounds {
            what,
            offset: offset as usize,
            len: (count as usize).saturating_mul(stride),
            file_len: self.data.len(),
        };
        let len = (count as usize).checked_mul(stride).ok_or_else(oob)?;
        let bytes = (offset as usize)
            .checked_add(len)
            .and_then(|end| self.data.get(offset as usize..end))
            .ok_or_else(oob)?;
        Ok(bytes.chunks_exact(stride))
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
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}

impl Mad {
    pub fn parse(data: &[u8]) -> Result<Mad> {
        if data.len() < HEADER_LEN {
            return Err(Error::TooShort(data.len()));
        }
        let ident: [u8; 8] = data[0..8].try_into().unwrap();
        if &ident != IDENTIFIER {
            return Err(Error::BadIdentifier(ident));
        }
        let version = f32_at(data, 8);
        if version != VERSION {
            return Err(Error::BadVersion(version));
        }
        let string_len = u32_at(data, 12);
        let string_at = data
            .len()
            .checked_sub(string_len as usize)
            .filter(|&at| at >= HEADER_LEN)
            .ok_or(Error::BadStringBlock {
                len: string_len,
                file_len: data.len(),
            })?;
        let object_count = u32_at(data, 28);
        let object_table = u32_at(data, 32);
        let animation_count = u32_at(data, 36);
        if animation_count as usize > MAX_ANIMATIONS {
            return Err(Error::TooManyAnimations(animation_count));
        }

        let r = Reader {
            data,
            strings: &data[string_at..],
        };

        let animations = r
            .table(
                "animations",
                HEADER_LEN as u32,
                animation_count,
                ANIMATION_LEN,
            )?
            .map(|a| {
                Ok(Animation {
                    name: r.string(u32_at(a, 0))?,
                    start_time: f32_at(a, 4),
                    end_time: f32_at(a, 8),
                    flags: u32_at(a, 12),
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let objects = r
            .table("object paths", object_table, object_count, OBJECT_PATH_LEN)?
            .map(|o| {
                let count = u32_at(o, 12) as usize;
                let times = r.floats("key times", u32_at(o, 16), count, 1)?;
                let tcb = r.floats("key parameters", u32_at(o, 20), count, 3)?;
                const CHANNELS: [&str; 6] = [
                    "x channel",
                    "y channel",
                    "z channel",
                    "heading channel",
                    "pitch channel",
                    "bank channel",
                ];
                let mut channels: [Vec<f32>; 6] = Default::default();
                for (j, ch) in channels.iter_mut().enumerate() {
                    *ch = r.floats(CHANNELS[j], u32_at(o, 24 + 4 * j), count, 1)?;
                }
                let keyframes = (0..count)
                    .map(|k| Keyframe {
                        time: times[k],
                        tcb: [tcb[3 * k], tcb[3 * k + 1], tcb[3 * k + 2]],
                        position: [channels[0][k], channels[1][k], channels[2][k]],
                        rotation: [channels[3][k], channels[4][k], channels[5][k]],
                    })
                    .collect();
                Ok(ObjectPath {
                    name: r.string(u32_at(o, 0))?,
                    animation_bits: u32_at(o, 8),
                    keyframes,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        Ok(Mad {
            version,
            length: f32_at(data, 20),
            frames_per_second: f32_at(data, 24),
            animations,
            objects,
        })
    }

    /// Index of the first animation called `name` (case-sensitive, as the game matched it).
    pub fn animation_index(&self, name: &str) -> Option<usize> {
        self.animations.iter().position(|a| a.name == name)
    }

    /// The path driving the mesh object called `name`.
    pub fn object(&self, name: &str) -> Option<&ObjectPath> {
        self.objects.iter().find(|o| o.name == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn put(v: &mut Vec<u8>, at: usize, bytes: &[u8]) {
        if v.len() < at + bytes.len() {
            v.resize(at + bytes.len(), 0);
        }
        v[at..at + bytes.len()].copy_from_slice(bytes);
    }

    #[test]
    fn one_object_two_keys() {
        let mut v = Vec::new();
        put(&mut v, 0, IDENTIFIER);
        put(&mut v, 8, &VERSION.to_le_bytes());
        put(&mut v, 24, &30.0f32.to_le_bytes());
        put(&mut v, 28, &1u32.to_le_bytes());
        let objs = HEADER_LEN + ANIMATION_LEN;
        put(&mut v, 32, &(objs as u32).to_le_bytes());
        put(&mut v, 36, &1u32.to_le_bytes());
        // animation 0: name at string offset 0
        put(&mut v, HEADER_LEN + 8, &1.0f32.to_le_bytes());
        put(&mut v, HEADER_LEN + 12, &ANIM_LOOP.to_le_bytes());
        let data = objs + OBJECT_PATH_LEN;
        let keys = 2usize;
        put(&mut v, objs, &5u32.to_le_bytes()); // name "Arm"
        put(&mut v, objs + 8, &1u32.to_le_bytes());
        put(&mut v, objs + 12, &(keys as u32).to_le_bytes());
        put(&mut v, objs + 16, &(data as u32).to_le_bytes());
        put(&mut v, objs + 20, &((data + 4 * keys) as u32).to_le_bytes());
        for j in 0..6 {
            let at = data + 16 * keys + 4 * keys * j;
            put(&mut v, objs + 24 + 4 * j, &(at as u32).to_le_bytes());
            put(&mut v, at + 4, &(j as f32 + 1.0).to_le_bytes());
        }
        put(&mut v, data + 4, &1.0f32.to_le_bytes());
        let strings = b"Open\0Arm\0";
        let end = v.len();
        put(&mut v, end, strings);
        put(&mut v, 12, &(strings.len() as u32).to_le_bytes());

        let m = Mad::parse(&v).unwrap();
        assert_eq!(m.animations[0].name, "Open");
        assert!(m.animations[0].loops());
        let o = m.object("Arm").unwrap();
        assert!(o.in_animation(0));
        assert_eq!(o.keyframes.len(), 2);
        assert_eq!(o.keyframes[1].time, 1.0);
        assert_eq!(o.keyframes[1].position, [1.0, 2.0, 3.0]);
        assert_eq!(o.keyframes[1].rotation, [4.0, 5.0, 6.0]);

        put(&mut v, objs + 12, &1000u32.to_le_bytes());
        assert!(matches!(Mad::parse(&v), Err(Error::OutOfBounds { .. })));
    }
}
