//! Loading the game's sound data: banks, event tables, distance tables, music and speech.

use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use hwc_formats::audio::{self, Bank, Lut, MusicIndex, PhraseLut, SentenceLut, SfxLut, SpeechBank};
use hwc_formats::big::BigArchive;
use hwc_formats::GameData;
use hwc_formats::script::ScriptFile;

use crate::codec::{Eq, FLAT_EQ};
use crate::mixer::PatchRef;
use crate::tables::{DistanceTables, Tweaks};

/// Errors while loading sound data.
#[derive(Debug)]
pub enum DataError {
    Io(PathBuf, std::io::Error),
    Archive(String),
    Format(String, audio::AudioError),
    Missing(String),
    Mismatch(String),
}

impl fmt::Display for DataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DataError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            DataError::Archive(e) => write!(f, "archive: {e}"),
            DataError::Format(what, e) => write!(f, "{what}: {e}"),
            DataError::Missing(what) => write!(f, "missing {what}"),
            DataError::Mismatch(what) => write!(f, "generation checksum mismatch: {what}"),
        }
    }
}

impl std::error::Error for DataError {}

pub type Result<T> = std::result::Result<T, DataError>;

/// Where to find the files: the game's data ([`GameData`]: an imported data directory, or an
/// install or archive during development), and/or an extracted copy of the archive, the archive
/// itself and the install directory.
///
/// Archive files are looked up in the extracted copy first, then in the game data (overrides,
/// then archive), then in `archive`. The stream files (music, speech) come from the game data
/// when it has them, otherwise from `game_dir`.
pub struct DataSource {
    pub extracted: Option<PathBuf>,
    pub archive: Option<BigArchive>,
    pub game_dir: Option<PathBuf>,
    pub game: Option<GameData>,
}

impl DataSource {
    /// From the environment: `HWC_DATA_DIR` (an imported data directory) when set; otherwise
    /// `HWC_EXTRACTED_DIR` and `HWC_GAME_DIR`, opening the archive from the install when no
    /// extracted copy is given. An `HWC_EXTRACTED_DIR` also layers over `HWC_DATA_DIR`.
    pub fn from_env() -> Option<Self> {
        let extracted = std::env::var_os("HWC_EXTRACTED_DIR").map(PathBuf::from);
        if let Some(dir) = std::env::var_os(hwc_formats::gamedata::ENV_DATA_DIR) {
            let mut src = Self::from_game_data(GameData::open(PathBuf::from(dir)).ok()?);
            src.extracted = extracted;
            return Some(src);
        }
        let game_dir = hwc_formats::game_dir();
        Self::new(extracted, game_dir).ok()
    }

    /// Everything from the game's data (see [`GameData::open`]).
    pub fn from_game_data(game: GameData) -> Self {
        Self { extracted: None, archive: None, game_dir: None, game: Some(game) }
    }

    pub fn new(extracted: Option<PathBuf>, game_dir: Option<PathBuf>) -> Result<Self> {
        let archive = match (&extracted, &game_dir) {
            (None, Some(g)) => {
                let path = g.join("cataclysm.big");
                Some(BigArchive::open(&path).map_err(|e| DataError::Archive(format!("{}: {e}", path.display())))?)
            }
            _ => None,
        };
        if extracted.is_none() && game_dir.is_none() {
            return Err(DataError::Missing("data location (extracted dir or game dir)".into()));
        }
        Ok(Self { extracted, archive, game_dir, game: None })
    }

    /// Read a file stored in the archive (`soundfx\guns.bnk`, `soundevent.script`, ...).
    pub fn read_archived(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(dir) = &self.extracted {
            let path = name.split(['\\', '/']).fold(dir.clone(), |p, c| p.join(c));
            if path.exists() {
                return std::fs::read(&path).map_err(|e| DataError::Io(path, e));
            }
        }
        if let Some(g) = &self.game {
            if let Some(bytes) = g.read(name).map_err(|e| DataError::Archive(format!("{name}: {e}")))? {
                return Ok(bytes);
            }
        }
        if let Some(a) = &self.archive {
            return a
                .read_file(name)
                .map_err(|e| DataError::Archive(format!("{name}: {e}")))?
                .ok_or_else(|| DataError::Missing(name.into()));
        }
        Err(DataError::Missing(name.into()))
    }

    /// Read a file kept outside the archive (`Music4Cat.wxd`, `SectSpeech.vce`), from the game
    /// data or the install directory (case-insensitive name match).
    pub fn read_install(&self, name: &str) -> Result<Vec<u8>> {
        if let Some(path) = self.game.as_ref().and_then(|g| g.install_file(name)) {
            return std::fs::read(&path).map_err(|e| DataError::Io(path, e));
        }
        let dir = self.game_dir.as_ref().ok_or_else(|| DataError::Missing(format!("{name} (no game dir)")))?;
        let path = find_case_insensitive(dir, name).unwrap_or_else(|| dir.join(name));
        std::fs::read(&path).map_err(|e| DataError::Io(path, e))
    }
}

fn find_case_insensitive(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// A parsed effect bank whose patches can be handed to the mixer.
pub struct SoundBank {
    pub name: String,
    pub checksum: u32,
    pub patches: Vec<PatchRef>,
}

impl SoundBank {
    pub fn parse(name: &str, bytes: Vec<u8>) -> Result<Self> {
        let data: Arc<[u8]> = bytes.into();
        let bank = Bank::parse(&data).map_err(|e| DataError::Format(name.into(), e))?;
        let patches = bank
            .patches
            .iter()
            .map(|p| {
                let start = p.data_offset as usize;
                PatchRef {
                    data: data.clone(),
                    start,
                    end: start + p.data.len(),
                    loop_start: start + p.loop_start as usize,
                    loop_end: start + p.loop_end as usize,
                    bitrate: if p.bitrate == 0 { 512 } else { p.bitrate },
                    flags: p.flags,
                    sample_rate: p.format.sample_rate,
                    volume: p.volume,
                    pan: p.pan,
                    priority: p.priority,
                }
            })
            .collect();
        Ok(Self { name: name.into(), checksum: bank.checksum, patches })
    }

    /// A patch by number; negative or out-of-range numbers give `None` (the game's "no sound").
    pub fn patch(&self, n: i32) -> Option<&PatchRef> {
        usize::try_from(n).ok().and_then(|n| self.patches.get(n))
    }
}

/// One music track resolved inside the `.wxd`.
#[derive(Debug, Clone, Copy)]
pub struct TrackInfo {
    pub data_offset: usize,
    pub len: usize,
    pub looping: bool,
    pub bitrate: u16,
    pub volume: u8,
    pub channels: u8,
}

impl TrackInfo {
    pub fn duration_secs(&self) -> f64 {
        let frame = usize::from(self.bitrate / 8) * usize::from(self.channels.max(1));
        (self.len / frame.max(1)) as f64 * crate::codec::BLOCK_SECONDS
    }
}

/// The music index plus the whole data file.
pub struct MusicData {
    pub bytes: Arc<[u8]>,
    pub tracks: Vec<TrackInfo>,
}

impl MusicData {
    pub fn parse(wxh: &[u8], wxd: Vec<u8>) -> Result<Self> {
        let index = MusicIndex::parse(wxh).map_err(|e| DataError::Format("music4cat.wxh".into(), e))?;
        let bytes: Arc<[u8]> = wxd.into();
        let tracks = audio::music_tracks(&index, &bytes)
            .map_err(|e| DataError::Format("Music4Cat.wxd".into(), e))?
            .iter()
            .map(|t| TrackInfo {
                data_offset: t.record.data_offset,
                len: t.record.data.len(),
                looping: t.info.flags & audio::MUSIC_FLAG_LOOPING != 0,
                bitrate: t.info.bitrate,
                volume: t.info.volume,
                channels: t.info.channels,
            })
            .collect();
        Ok(Self { bytes, tracks })
    }
}

/// A speech fragment in a `.vce`.
#[derive(Debug, Clone)]
pub struct Fragment {
    pub data_offset: usize,
    pub len: usize,
    pub subtitle: Option<String>,
}

/// One race's speech: sentence and phrase tables plus the voice file.
pub struct SpeechData {
    pub sentences: SentenceLut,
    pub phrases: PhraseLut,
    pub bytes: Arc<[u8]>,
    /// Fragment by the offset the phrase table uses.
    pub fragments: HashMap<usize, Fragment>,
}

impl SpeechData {
    pub fn parse(sentence: &[u8], phrase: &[u8], vce: Vec<u8>) -> Result<Self> {
        let Lut::Sentence(sentences) = Lut::parse(sentence).map_err(|e| DataError::Format("sentence lut".into(), e))? else {
            return Err(DataError::Missing("LUT1 sentence table".into()));
        };
        let Lut::Phrase(phrases) = Lut::parse(phrase).map_err(|e| DataError::Format("phrase lut".into(), e))? else {
            return Err(DataError::Missing("LUT2 phrase table".into()));
        };
        let bytes: Arc<[u8]> = vce.into();
        let bank = SpeechBank::parse(&bytes).map_err(|e| DataError::Format("vce".into(), e))?;
        if bank.checksum != sentences.checksum || phrases.checksum != sentences.checksum {
            return Err(DataError::Mismatch("speech tables / voice file".into()));
        }
        let fragments = bank
            .phrases
            .iter()
            .map(|r| {
                let sub = r.subtitle_text().filter(|s| !s.is_empty());
                (r.offset, Fragment { data_offset: r.data_offset, len: r.data.len(), subtitle: sub })
            })
            .collect();
        Ok(Self { sentences, phrases, bytes, fragments })
    }

    /// Codec rate of the speech in the voice file.
    pub fn bitrate(&self) -> u16 {
        self.sentences.comp_bitrates.first().copied().unwrap_or(512)
    }
}

/// The two speech races of Cataclysm.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Race {
    /// The Somtaaw ("Sect") voice set.
    #[default]
    Somtaaw,
    Beast,
}

impl Race {
    /// Object index into race-dependent tables (the UI event table).
    pub fn index(self) -> usize {
        match self {
            Race::Somtaaw => 0,
            Race::Beast => 1,
        }
    }
}

/// The equaliser settings from `soundfx\scripts\eq_*.script`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StreamEq {
    pub enabled: bool,
    pub eq: Eq,
}

impl StreamEq {
    /// Parse `flags`/`eq0..eq7`; the ninth band copies the eighth, as the game does.
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut eq = FLAT_EQ;
        for (i, b) in eq.iter_mut().enumerate().take(8) {
            if let Some(v) = script.get(&format!("eq{i}")).and_then(|e| e.f32()) {
                *b = v;
            }
        }
        eq[8] = eq[7];
        let enabled = script.get("flags").and_then(|e| e.i32()).unwrap_or(0) != 0;
        Self { enabled, eq }
    }

    pub fn as_option(&self) -> Option<Eq> {
        self.enabled.then_some(self.eq)
    }
}

/// Everything the sound system needs from the game files.
pub struct SoundData {
    pub guns: SoundBank,
    pub ships: SoundBank,
    pub special: SoundBank,
    pub ui: SoundBank,
    pub gun_events: SfxLut,
    pub gun_spec_events: SfxLut,
    pub ship_cmn_events: SfxLut,
    pub ship_events: SfxLut,
    pub derelict_events: SfxLut,
    pub spec_exp_events: SfxLut,
    pub spec_hit_events: SfxLut,
    pub ui_events: SfxLut,
    pub tables: DistanceTables,
    pub tweaks: Tweaks,
    pub eq_fleet: StreamEq,
    pub eq_intel: StreamEq,
    pub music: Option<MusicData>,
    pub speech_somtaaw: Option<SpeechData>,
    pub speech_beast: Option<SpeechData>,
}

fn sfx_lut(src: &DataSource, name: &str) -> Result<SfxLut> {
    let path = format!("soundfx\\{name}");
    match Lut::parse(&src.read_archived(&path)?).map_err(|e| DataError::Format(path.clone(), e))? {
        Lut::Sfx(l) => Ok(l),
        _ => Err(DataError::Missing(format!("{path} is not a PKG0 table"))),
    }
}

impl SoundData {
    /// Load everything. Music and speech are optional (they need the install directory).
    pub fn load(src: &DataSource) -> Result<Self> {
        let bank = |n: &str| SoundBank::parse(n, src.read_archived(&format!("soundfx\\{n}.bnk"))?);
        let guns = bank("guns")?;
        let ships = bank("ships")?;
        let special = bank("specialeffects")?;
        let ui = bank("ui")?;
        let gun_events = sfx_lut(src, "gunevents.lut")?;
        let gun_spec_events = sfx_lut(src, "gunspecevents.lut")?;
        let ship_cmn_events = sfx_lut(src, "shipcmnevents.lut")?;
        let ship_events = sfx_lut(src, "shipevents.lut")?;
        let derelict_events = sfx_lut(src, "derelictevents.lut")?;
        let spec_exp_events = sfx_lut(src, "specexpevents.lut")?;
        let spec_hit_events = sfx_lut(src, "spechitevents.lut")?;
        let ui_events = sfx_lut(src, "uievents.lut")?;
        for (lut, b, what) in [
            (&gun_events, &guns, "gunevents/guns.bnk"),
            (&ship_cmn_events, &ships, "shipcmnevents/ships.bnk"),
            (&spec_exp_events, &special, "specexpevents/specialeffects.bnk"),
            (&ui_events, &ui, "uievents/ui.bnk"),
        ] {
            if lut.checksum != b.checksum {
                return Err(DataError::Mismatch(what.into()));
            }
        }
        let table = |n: &str| -> Result<Lut> {
            let path = format!("soundfx\\{n}");
            Lut::parse(&src.read_archived(&path)?).map_err(|e| DataError::Format(path, e))
        };
        let (Lut::Volume(vol), Lut::Range(rng), Lut::Frequency(frq)) = (table("volume.lut")?, table("range.lut")?, table("frequency.lut")?)
        else {
            return Err(DataError::Missing("VOL1/RNG1/FRQ1 tables".into()));
        };
        let tables = DistanceTables::new(&vol, &rng, &frq);
        let script = |n: &str| -> Option<ScriptFile> { src.read_archived(n).ok().and_then(|b| ScriptFile::parse_bytes(&b).ok()) };
        let tweaks = script("soundevent.script").map(|s| Tweaks::from_script(&s)).unwrap_or_default();
        let eq_of = |n: &str| script(n).map(|s| StreamEq::from_script(&s)).unwrap_or(StreamEq { enabled: false, eq: FLAT_EQ });
        let eq_fleet = eq_of("soundfx\\scripts\\eq_fleet.script");
        let eq_intel = eq_of("soundfx\\scripts\\eq_intel.script");

        let music = match (src.read_archived("soundfx\\music4cat.wxh"), src.read_install("Music4Cat.wxd")) {
            (Ok(h), Ok(d)) => Some(MusicData::parse(&h, d)?),
            _ => None,
        };
        let speech = |race: &str, file: &str| -> Option<SpeechData> {
            let s = src.read_archived(&format!("soundfx\\{race}sentence.lut")).ok()?;
            let p = src.read_archived(&format!("soundfx\\{race}phrase.lut")).ok()?;
            let v = src.read_install(file).ok()?;
            SpeechData::parse(&s, &p, v).ok()
        };
        Ok(Self {
            guns,
            ships,
            special,
            ui,
            gun_events,
            gun_spec_events,
            ship_cmn_events,
            ship_events,
            derelict_events,
            spec_exp_events,
            spec_hit_events,
            ui_events,
            tables,
            tweaks,
            eq_fleet,
            eq_intel,
            music,
            speech_somtaaw: speech("sect", "SectSpeech.vce"),
            speech_beast: speech("beast", "BeastSpeech.vce"),
        })
    }

    pub fn speech(&self, race: Race) -> Option<&SpeechData> {
        match race {
            Race::Somtaaw => self.speech_somtaaw.as_ref(),
            Race::Beast => self.speech_beast.as_ref(),
        }
    }
}
