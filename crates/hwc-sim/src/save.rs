//! Saved games: a versioned file holding everything a running game needs to carry on exactly
//! where it was saved (see `docs/sim/savegame.md`).
//!
//! The simulation state ([`crate::mission::MissionRun`]: the universe with its ships, bullets,
//! missiles, resources, derelicts, commands, formations, players, research, construction,
//! random streams, effects, nebulae and fog of war; the KAS interpreter's variables, timers and
//! team states; the game-side mission state: objectives, pings, hyperspace, the presentation
//! queue) derives `serde`, and a saved game is that state encoded with `bincode` (floats as
//! their exact bits) and compressed with deflate. What comes from the game files and never
//! changes while a game runs — the ship statics, the combat routines built from them, the
//! compiled effects and their event tables, the mission IR — is not saved: loading links the
//! state back to the data the game has loaded ([`Relink`]).
//!
//! File layout (little endian):
//!
//! | Bytes | Content |
//! | --- | --- |
//! | 8 | magic `HWCSAVE\0` |
//! | 4 | [`FORMAT_VERSION`] |
//! | 4 | header length `n` |
//! | n | [`SaveHeader`], bincode (description, mission, date, game time, thumbnail) |
//! | rest | the payload, bincode, deflate-compressed |
//!
//! The header can be read without the payload (the save list), and the version is checked
//! before anything else is decoded: a file of another version is refused with
//! [`SaveError::Version`].

use std::cell::{Cell, RefCell};
use std::fmt;
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

use bincode::Options as _;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::combat_statics::CombatStatics;
use crate::statics::StaticWorld;

/// The file's magic bytes.
pub const MAGIC: [u8; 8] = *b"HWCSAVE\0";

/// The version of the save format. Raise it whenever a saved type changes shape (a field added,
/// removed, reordered or retyped). A field added at version `n` is read with `added_in_<n>`
/// ([`added_in_2`]), so files of older versions still load with the field at its default;
/// files of a newer version, or older ones whose shape is not known, are refused.
pub const FORMAT_VERSION: u32 = 7;

/// The oldest version still read.
pub const OLDEST_VERSION: u32 = 1;

/// The shapes files of version 1 were written in, newest first: version 1 was not raised while
/// fields were added to it, so its files are tried as each shape it had until one decodes
/// whole. Shape `n` lacks every field added at a later shape (see [`added_in_2`]).
const VERSION_1_SHAPES: &[u32] = &[1];

/// The largest payload a save may decode to (guards against damaged files).
const MAX_PAYLOAD: u64 = 1 << 30;

/// A small picture of the game when it was saved, for the save list.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct Thumbnail {
    pub width: u32,
    pub height: u32,
    /// RGB, row by row from the top.
    pub rgb: Vec<u8>,
}

/// What the save list shows of a saved game; readable without decoding the game.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SaveHeader {
    /// The name the player typed (or the autosave's name).
    pub description: String,
    /// The mission number (1..17; 0 for the tutorial).
    pub mission: u32,
    /// The mission's name as the game shows it.
    pub mission_name: String,
    /// When the game was saved: seconds since 1970-01-01 UTC.
    pub saved_at: u64,
    /// Game time in the mission (`universe.totaltimeelapsed`).
    pub game_time: f32,
    /// The universe update counter.
    pub frame: u32,
    /// Saved by the mission itself (`SaveLevel`), not by the player.
    pub autosave: bool,
    /// The single-player difficulty.
    pub skill: i32,
    /// A fingerprint of the mission script the game ran (the IR is game data and is not
    /// saved); loading checks it against the script the game has.
    pub script_fingerprint: u64,
    /// The program that wrote the file.
    pub writer: String,
    pub thumbnail: Option<Thumbnail>,
}

/// Why a saved game could not be written or read.
#[derive(Debug)]
pub enum SaveError {
    Io(std::io::Error),
    /// Not a saved game of this recreation (the original's saves have no magic).
    NotASave,
    /// A saved game of another format version.
    Version { found: u32, expected: u32 },
    /// The file is damaged or does not match this program's game state.
    Corrupt(String),
    /// The saved game was made with a different mission script.
    Script { mission: u32 },
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SaveError::Io(e) => write!(f, "{e}"),
            SaveError::NotASave => write!(f, "not a saved game"),
            SaveError::Version { found, expected } => {
                write!(f, "saved game format version {found}; this version of the game reads version {expected}")
            }
            SaveError::Corrupt(e) => write!(f, "the saved game is damaged or from a different build ({e})"),
            SaveError::Script { mission } => write!(f, "the saved game was made with a different mission {mission} script"),
        }
    }
}

impl std::error::Error for SaveError {}

impl From<std::io::Error> for SaveError {
    fn from(e: std::io::Error) -> Self {
        SaveError::Io(e)
    }
}

fn codec() -> impl bincode::Options {
    bincode::DefaultOptions::new().with_fixint_encoding().with_little_endian().reject_trailing_bytes().with_limit(MAX_PAYLOAD)
}

/// The game data a loaded state is linked back to.
#[derive(Clone)]
pub struct Relink {
    pub statics: Arc<StaticWorld>,
    pub combat: Arc<CombatStatics>,
    /// The compiled effects and their tables (when the game has effects).
    pub effects: Option<crate::effects::EffectData>,
    /// The mission IR of the saved mission.
    pub mission: Option<Arc<hwc_kas::Mission>>,
}

impl Relink {
    /// Links to `statics`, building the combat routines from them.
    pub fn new(statics: Arc<StaticWorld>) -> Self {
        let combat = Arc::new(CombatStatics::build(&statics));
        Relink { statics, combat, effects: None, mission: None }
    }
}

thread_local! {
    static RELINK: RefCell<Option<Relink>> = const { RefCell::new(None) };
    /// The shape of the file being decoded (the version, or a version 1 shape).
    static SHAPE: Cell<u32> = const { Cell::new(FORMAT_VERSION) };
}

/// Reads a field added at shape `n`: a file of an older shape does not have it, so it is left
/// at its default without reading anything.
fn added_in<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(n: u32, d: D) -> Result<T, D::Error> {
    added_in_or(n, d, T::default)
}

/// [`added_in`] for a field whose value in an older file is not its type's default.
/// Version 4: [`crate::tunables::SimTweaks::spheresize_ignore_numdeclinations`]; version 5:
/// [`crate::combat_tweaks::CombatTweaks::kamikaze_speed_burst`].
pub fn added_in_or<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(n: u32, d: D, old: impl FnOnce() -> T) -> Result<T, D::Error> {
    if SHAPE.with(Cell::get) < n {
        Ok(old())
    } else {
        T::deserialize(d)
    }
}

/// `#[serde(default, deserialize_with = "crate::save::added_in_2")]` on a field added at
/// version 2: [`crate::universe::Universe::letterbox`].
pub fn added_in_2<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    added_in(2, d)
}

/// `#[serde(default, deserialize_with = "crate::save::added_in_3")]` on a field added at
/// version 3: [`crate::base::ShipBase::avoid_exception`].
pub fn added_in_3<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    added_in(3, d)
}

/// `#[serde(default, deserialize_with = "crate::save::added_in_5")]` on a field added at
/// version 5: [`crate::object::Ship::kamikaze_state`] and [`crate::object::Ship::kamikaze_vector`].
pub fn added_in_5<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    added_in(5, d)
}

/// `#[serde(default, deserialize_with = "crate::save::added_in_6")]` on a field added at
/// version 6: [`crate::mission::FleetShip::upgrades`] and [`crate::mission::Game::fleet_upgrades_kept`].
pub fn added_in_6<'de, D: serde::Deserializer<'de>, T: Deserialize<'de> + Default>(d: D) -> Result<T, D::Error> {
    added_in(6, d)
}

fn current<T>(f: impl FnOnce(&Relink) -> T) -> T {
    RELINK.with(|r| f(r.borrow().as_ref().expect("simulation state deserialised outside save::read (no game data to link it to)")))
}

pub(crate) fn statics() -> Arc<StaticWorld> {
    current(|r| r.statics.clone())
}

pub(crate) fn combat() -> Arc<CombatStatics> {
    current(|r| r.combat.clone())
}

pub(crate) fn etg_tables() -> Arc<hwc_fx::EtgTables> {
    current(|r| r.effects.as_ref().map(|e| e.tables.clone())).unwrap_or_else(|| Arc::new(hwc_fx::EtgTables::default()))
}

/// Runs `f` with `relink` as the game data every deserialised state links to.
pub fn with_relink<R>(relink: &Relink, f: impl FnOnce() -> R) -> R {
    let old = RELINK.with(|r| r.borrow_mut().replace(relink.clone()));
    let lib = relink.effects.as_ref().map(|e| e.lib.clone()).unwrap_or_default();
    let mission = relink.mission.clone();
    let out = hwc_fx::runtime::relink::with_library(lib, || match mission {
        Some(m) => hwc_kas::interp::relink::with_mission(m, f),
        None => f(),
    });
    RELINK.with(|r| *r.borrow_mut() = old);
    out
}

/// A fingerprint of a mission script (FNV-1a of its encoding).
pub fn script_fingerprint(m: &hwc_kas::Mission) -> u64 {
    let bytes = codec().serialize(m).unwrap_or_default();
    let mut h = crate::harness::Fnv64::default();
    h.bytes(&bytes);
    h.finish()
}

/// Encodes a saved game.
pub fn encode<T: Serialize>(header: &SaveHeader, payload: &T) -> Result<Vec<u8>, SaveError> {
    let head = codec().serialize(header).map_err(|e| SaveError::Corrupt(e.to_string()))?;
    let body = codec().serialize(payload).map_err(|e| SaveError::Corrupt(e.to_string()))?;
    let mut out = Vec::with_capacity(16 + head.len() + body.len() / 4);
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    out.extend_from_slice(&(head.len() as u32).to_le_bytes());
    out.extend_from_slice(&head);
    let mut z = flate2::write::DeflateEncoder::new(out, flate2::Compression::fast());
    z.write_all(&body)?;
    Ok(z.finish()?)
}

/// Splits a file into its header and compressed payload after checking magic and version.
fn split(bytes: &[u8]) -> Result<(SaveHeader, &[u8]), SaveError> {
    if bytes.len() < 16 || bytes[..8] != MAGIC {
        return Err(SaveError::NotASave);
    }
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if !(OLDEST_VERSION..=FORMAT_VERSION).contains(&version) {
        return Err(SaveError::Version { found: version, expected: FORMAT_VERSION });
    }
    let n = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let head = bytes.get(16..16 + n).ok_or_else(|| SaveError::Corrupt("header cut short".into()))?;
    let header: SaveHeader = codec().deserialize(head).map_err(|e| SaveError::Corrupt(e.to_string()))?;
    Ok((header, &bytes[16 + n..]))
}

/// The header of an encoded saved game.
pub fn decode_header(bytes: &[u8]) -> Result<SaveHeader, SaveError> {
    split(bytes).map(|(h, _)| h)
}

/// Decodes a saved game, linking its state to `relink`'s game data. When `relink` has a
/// mission script, it must be the one the game was saved with.
pub fn decode<T: DeserializeOwned>(bytes: &[u8], relink: &Relink) -> Result<(SaveHeader, T), SaveError> {
    let (header, body) = split(bytes)?;
    let version = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if let Some(m) = &relink.mission {
        if header.script_fingerprint != 0 && script_fingerprint(m) != header.script_fingerprint {
            return Err(SaveError::Script { mission: header.mission });
        }
    }
    let mut raw = Vec::new();
    flate2::read::DeflateDecoder::new(body).take(MAX_PAYLOAD).read_to_end(&mut raw).map_err(|e| SaveError::Corrupt(e.to_string()))?;
    let shapes = if version == 1 { VERSION_1_SHAPES } else { std::slice::from_ref(&version) };
    let mut first_error = None;
    for &shape in shapes {
        let old = SHAPE.with(|s| s.replace(shape));
        let decoded = with_relink(relink, || codec().deserialize(&raw));
        SHAPE.with(|s| s.set(old));
        match decoded {
            Ok(payload) => return Ok((header, payload)),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(SaveError::Corrupt(first_error.map_or_else(String::new, |e| e.to_string())))
}

/// Writes a saved game to `path` (through a temporary file, so a failed save leaves any older
/// file of that name as it was).
pub fn write<T: Serialize>(path: &Path, header: &SaveHeader, payload: &T) -> Result<(), SaveError> {
    let bytes = encode(header, payload)?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(())
}

/// Reads the header of the saved game at `path` (only the start of the file is read).
pub fn read_header(path: &Path) -> Result<SaveHeader, SaveError> {
    let mut f = std::fs::File::open(path)?;
    let mut start = [0u8; 16];
    f.read_exact(&mut start).map_err(|_| SaveError::NotASave)?;
    if start[..8] != MAGIC {
        return Err(SaveError::NotASave);
    }
    let n = u32::from_le_bytes(start[12..16].try_into().unwrap()) as usize;
    let mut bytes = start.to_vec();
    if (OLDEST_VERSION..=FORMAT_VERSION).contains(&u32::from_le_bytes(start[8..12].try_into().unwrap())) {
        if n > 64 << 20 {
            return Err(SaveError::Corrupt("header too large".into()));
        }
        bytes.resize(16 + n, 0);
        f.read_exact(&mut bytes[16..]).map_err(|e| SaveError::Corrupt(e.to_string()))?;
    }
    decode_header(&bytes)
}

/// Reads the saved game at `path`.
pub fn read<T: DeserializeOwned>(path: &Path, relink: &Relink) -> Result<(SaveHeader, T), SaveError> {
    decode(&std::fs::read(path)?, relink)
}

/// Seconds since 1970 now.
pub fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct P {
        a: Vec<f32>,
        b: String,
        #[serde(with = "hwc_math::serde_array")]
        c: [u32; 40],
    }

    struct Empty;
    impl crate::statics::DataSource for Empty {
        fn read(&self, _: &str) -> Option<Vec<u8>> {
            None
        }
        fn exists(&self, _: &str) -> bool {
            false
        }
    }

    fn relink() -> Relink {
        Relink::new(Arc::new(StaticWorld::load_from(&Empty, Default::default())))
    }

    #[test]
    fn round_trip_keeps_float_bits() {
        let mut c = [0u32; 40];
        c[39] = 9;
        let p = P { a: vec![f32::NAN, -0.0, 1.0e-42, f32::INFINITY], b: "x".into(), c };
        let h = SaveHeader { description: "Test".into(), mission: 3, ..Default::default() };
        let bytes = encode(&h, &p).unwrap();
        assert_eq!(decode_header(&bytes).unwrap(), h);
        let (h2, q): (SaveHeader, P) = decode(&bytes, &relink()).unwrap();
        assert_eq!(h2.description, "Test");
        let bits = |v: &[f32]| v.iter().map(|x| x.to_bits()).collect::<Vec<_>>();
        assert_eq!(bits(&q.a), bits(&p.a));
        assert_eq!((q.b, q.c), (p.b, p.c));
    }

    #[test]
    fn other_versions_are_refused() {
        let mut bytes = encode(&SaveHeader::default(), &1u32).unwrap();
        bytes[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
        match decode::<u32>(&bytes, &relink()) {
            Err(SaveError::Version { found, expected }) => assert_eq!((found, expected), (FORMAT_VERSION + 1, FORMAT_VERSION)),
            other => panic!("{other:?}"),
        }
        let msg = decode_header(&bytes).unwrap_err().to_string();
        assert!(msg.contains("version"), "{msg}");
        assert!(matches!(decode_header(b"not a save file at all"), Err(SaveError::NotASave)));
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Before {
        a: u32,
        c: u8,
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct After {
        a: u32,
        #[serde(default, deserialize_with = "added_in_2")]
        b: Vec<u16>,
        c: u8,
    }

    #[test]
    fn version_1_files_load_without_later_fields() {
        let mut bytes = encode(&SaveHeader::default(), &Before { a: 5, c: 9 }).unwrap();
        bytes[8..12].copy_from_slice(&1u32.to_le_bytes());
        decode_header(&bytes).unwrap();
        let (_, got): (SaveHeader, After) = decode(&bytes, &relink()).unwrap();
        assert_eq!(got, After { a: 5, b: Vec::new(), c: 9 });
        // A file of this version has the field.
        let now = After { a: 1, b: vec![2, 3], c: 4 };
        let (_, got): (SaveHeader, After) = decode(&encode(&SaveHeader::default(), &now).unwrap(), &relink()).unwrap();
        assert_eq!(got, now);
    }

    #[test]
    fn damaged_payloads_are_refused() {
        let mut bytes = encode(&SaveHeader::default(), &vec![1u32; 100]).unwrap();
        let n = bytes.len();
        bytes.truncate(n - 3);
        assert!(matches!(decode::<Vec<u32>>(&bytes, &relink()), Err(SaveError::Corrupt(_))));
    }

    #[test]
    fn files_round_trip_and_headers_read_alone() {
        let dir = std::env::temp_dir().join(format!("hwc-save-test-{}", std::process::id()));
        let path = dir.join("slot.hwcsave");
        let h = SaveHeader { description: "Slot".into(), thumbnail: Some(Thumbnail { width: 2, height: 1, rgb: vec![1; 6] }), ..Default::default() };
        write(&path, &h, &vec![7u8; 1000]).unwrap();
        assert_eq!(read_header(&path).unwrap(), h);
        let (_, v): (SaveHeader, Vec<u8>) = read(&path, &relink()).unwrap();
        assert_eq!(v, vec![7u8; 1000]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
