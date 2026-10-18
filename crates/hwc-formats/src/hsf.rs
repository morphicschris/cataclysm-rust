//! `.hsf` scene lighting.
//!
//! Despite living next to the hyperspace code in name, an HSF ("Homeworld scene file") holds the
//! lights for a mission, NIS or the ship showroom: one ambient term and one or two directional or
//! spot lights. The file is a 20-byte header followed by fixed 44-byte light records; the engine
//! reads records until the file runs out, so the header's count is informational.
//!
//! Header:
//!
//! | offset | type     | field                             |
//! |--------|----------|-----------------------------------|
//! | 0      | [u8; 12] | identifier `"rrasberry\0\0\0"`    |
//! | 12     | u32      | version, `0x100`                  |
//! | 16     | u32      | light count                       |
//!
//! Light record:
//!
//! | offset | type    | field                                                          |
//! |--------|---------|----------------------------------------------------------------|
//! | 0      | u32     | [`LightKind`]                                                  |
//! | 4      | f32 × 3 | position (exporter space; the engine swaps to GL axes)         |
//! | 16     | f32 × 3 | heading, pitch, bank (radians)                                 |
//! | 28     | f32     | spot cone angle (degrees; uninitialised `0xCDCDCDCD` otherwise)|
//! | 32     | f32     | spot edge angle (same)                                         |
//! | 36     | u8 × 3  | colour RGB                                                     |
//! | 39     | u8      | padding                                                        |
//! | 40     | f32     | intensity                                                      |

use thiserror::Error;

pub const IDENTIFIER: &[u8; 12] = b"rrasberry\0\0\0";
pub const VERSION: u32 = 0x100;
pub const HEADER_LEN: usize = 20;
pub const LIGHT_LEN: usize = 44;
/// Fill pattern of fields the exporter never wrote.
const UNSET: u32 = 0xCDCD_CDCD;

#[derive(Debug, Error)]
pub enum Error {
    #[error("file is {0} bytes, too short for an HSF header")]
    TooShort(usize),
    #[error("bad identifier {0:?}")]
    BadIdentifier([u8; 12]),
    #[error("unsupported version {0:#x}")]
    BadVersion(u32),
    #[error("header declares {declared} lights but the file holds {present}")]
    CountMismatch { declared: u32, present: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LightKind {
    Distant,
    Point,
    Spot,
    Linear,
    Area,
    Ambient,
    Other(u32),
}

impl LightKind {
    pub fn from_raw(v: u32) -> LightKind {
        match v {
            0 => LightKind::Distant,
            1 => LightKind::Point,
            2 => LightKind::Spot,
            3 => LightKind::Linear,
            4 => LightKind::Area,
            5 => LightKind::Ambient,
            n => LightKind::Other(n),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub kind: LightKind,
    pub position: [f32; 3],
    /// Heading, pitch, bank.
    pub orientation: [f32; 3],
    /// Spot cone and edge angles; `None` where the exporter left them unset.
    pub cone_angle: Option<f32>,
    pub edge_angle: Option<f32>,
    pub color: [u8; 3],
    pub intensity: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Hsf {
    pub version: u32,
    pub lights: Vec<Light>,
}

fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn opt_angle(b: &[u8], at: usize) -> Option<f32> {
    (u32_at(b, at) != UNSET).then(|| f32_at(b, at))
}

impl Hsf {
    pub fn parse(data: &[u8]) -> Result<Hsf> {
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
        let declared = u32_at(data, 16);
        let lights: Vec<Light> = data[HEADER_LEN..]
            .as_chunks::<LIGHT_LEN>()
            .0
            .iter()
            .map(|r| Light {
                kind: LightKind::from_raw(u32_at(r, 0)),
                position: [f32_at(r, 4), f32_at(r, 8), f32_at(r, 12)],
                orientation: [f32_at(r, 16), f32_at(r, 20), f32_at(r, 24)],
                cone_angle: opt_angle(r, 28),
                edge_angle: opt_angle(r, 32),
                color: [r[36], r[37], r[38]],
                intensity: f32_at(r, 40),
            })
            .collect();
        if lights.len() != declared as usize || !(data.len() - HEADER_LEN).is_multiple_of(LIGHT_LEN)
        {
            return Err(Error::CountMismatch {
                declared,
                present: lights.len(),
            });
        }
        Ok(Hsf { version, lights })
    }

    /// The ambient light, if the file has one.
    pub fn ambient(&self) -> Option<&Light> {
        self.lights.iter().find(|l| l.kind == LightKind::Ambient)
    }

    /// Non-ambient lights in file order (the first is the engine's key light).
    pub fn directional(&self) -> impl Iterator<Item = &Light> + '_ {
        self.lights.iter().filter(|l| l.kind != LightKind::Ambient)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_synthetic_file() {
        let mut d = vec![0u8; HEADER_LEN + LIGHT_LEN];
        d[..12].copy_from_slice(IDENTIFIER);
        d[12..16].copy_from_slice(&VERSION.to_le_bytes());
        d[16..20].copy_from_slice(&1u32.to_le_bytes());
        let r = HEADER_LEN;
        d[r..r + 4].copy_from_slice(&2u32.to_le_bytes());
        d[r + 28..r + 32].copy_from_slice(&30.0f32.to_le_bytes());
        d[r + 32..r + 36].copy_from_slice(&UNSET.to_le_bytes());
        d[r + 36..r + 39].copy_from_slice(&[1, 2, 3]);
        d[r + 40..r + 44].copy_from_slice(&1.5f32.to_le_bytes());
        let h = Hsf::parse(&d).unwrap();
        let l = h.lights[0];
        assert_eq!(l.kind, LightKind::Spot);
        assert_eq!((l.cone_angle, l.edge_angle), (Some(30.0), None));
        assert_eq!((l.color, l.intensity), ([1, 2, 3], 1.5));
        assert!(h.ambient().is_none());
        assert!(matches!(
            Hsf::parse(&d[..HEADER_LEN + 10]),
            Err(Error::CountMismatch { .. })
        ));
    }
}
