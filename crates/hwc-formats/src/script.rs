//! Reader for the plain-text "tweak" files the engine is configured with: global `*.script` files,
//! ship statics (`*.shp`), level-of-detail lists (`*.lod`), resource distributions (`*.dist`),
//! mission and level layouts (`*.mission`, `*.level`, `*.missphere`, and the extension-less object
//! files inside mission folders), credits/plug screens (`*.cred`, `*.plug`) and similar.
//!
//! # Syntax accepted by the engine
//!
//! The format is line oriented; there is no tokenizer in the original beyond splitting each line
//! into a keyword and a value string, which the per-keyword setter then scans:
//!
//! * A line whose **first column** is `[` or `]` is not an entry. `[Name]` headers are purely
//!   cosmetic to the engine (every key is global within the file); this reader keeps them as
//!   [`Section`]s so that files with several headers stay distinguishable.
//! * A comment runs from the first `;` or the first `//` to the end of the line. There is no
//!   quoting and no escape, so a `;` inside a string value also ends it. C block comments (`/* */`)
//!   are **not** comments; their lines simply become entries nobody binds.
//! * Leading blanks are skipped. The keyword runs up to the first space, tab or `=`; the value is
//!   the rest of the line with leading and trailing blanks removed. (So `key=value` works but in
//!   `key = value` the value is `= value`.) A keyword with no value is still an entry.
//! * Keywords are matched case-sensitively. Array-style keywords such as
//!   `thruststrength[TRANS_UP]` or Cataclysm's `AIPLAYER_UPDATE_RATE[AI_BEG]` are ordinary keywords
//!   whose spelling happens to include brackets; [`Entry::key_base`] / [`Entry::key_indices`] split
//!   them for convenience.
//! * `{` and `}` on their own line are entries too (keyword `{` / `}`); `.shp` files use them to
//!   delimit `GUN n` blocks.
//! * Values are scanned per keyword with C `scanf` conventions: commas and blanks separate fields,
//!   floats may carry a trailing `f` (`240.0f`) or an exponent (`5e-5`), `?` stands for an unknown
//!   field and reads as zero. [`Value`] mirrors that tokenisation; [`Entry::raw`] keeps the exact
//!   value text for setters that need it.
//! * There is no include directive. (`IncludeShips` in mission files is an ordinary keyword.)
//!
//! Files are 8-bit text (Windows-1252 in practice for the localised string tables); they are
//! decoded as Latin-1 so every byte round-trips.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::io;
use std::path::Path;

use thiserror::Error;

/// Longest line (including terminator) the original line reader accepts.
pub const ENGINE_MAX_LINE: usize = 650;

#[derive(Debug, Error)]
pub enum ScriptError {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("not a text file: control byte {byte:#04x} at offset {offset}")]
    NotText { offset: usize, byte: u8 },
}

/// One scanned field of a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// Decimal or `0x` hexadecimal integer.
    Int(i64),
    /// Number with a fraction, an exponent or a trailing `f`.
    Float(f64),
    /// Anything else: identifiers, enum names, file names, words of a string.
    Word(String),
    /// A lone `?` placeholder (the engine substitutes `0`).
    Unknown,
}

impl Value {
    /// Classifies one field.
    pub fn parse(token: &str) -> Value {
        if token == "?" {
            return Value::Unknown;
        }
        if let Some(i) = parse_int(token) {
            return Value::Int(i);
        }
        if let Some(f) = parse_float(token) {
            return Value::Float(f);
        }
        Value::Word(token.to_owned())
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|f| f as f32)
    }

    /// Integer value; floats are truncated the way `%d` would stop at the decimal point.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::Float(f) => Some(f.trunc() as i64),
            _ => None,
        }
    }

    pub fn as_i32(&self) -> Option<i32> {
        self.as_i64().and_then(|i| i32::try_from(i).ok())
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Word(s) => Some(s),
            _ => None,
        }
    }

    /// Boolean the way the tweak files intend it: `TRUE`/`YES` (any case) or a nonzero number.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Word(s) => Some(s.eq_ignore_ascii_case("TRUE") || s.eq_ignore_ascii_case("YES")),
            Value::Int(i) => Some(*i != 0),
            Value::Float(f) => Some(*f != 0.0),
            Value::Unknown => None,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(i) => write!(f, "{i}"),
            Value::Float(x) => write!(f, "{x}"),
            Value::Word(s) => f.write_str(s),
            Value::Unknown => f.write_str("?"),
        }
    }
}

fn split_sign(s: &str) -> (bool, &str) {
    match s.as_bytes().first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, s),
    }
}

fn parse_int(token: &str) -> Option<i64> {
    let (neg, body) = split_sign(token);
    let magnitude = if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        i64::from_str_radix(hex, 16).ok()?
    } else {
        if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        body.parse::<i64>().ok()?
    };
    Some(if neg { -magnitude } else { magnitude })
}

fn parse_float(token: &str) -> Option<f64> {
    let (_, body) = split_sign(token);
    let body = body.strip_suffix(['f', 'F']).unwrap_or(body);
    let bytes = body.as_bytes();
    let mut i = 0;
    let int_digits = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if bytes.get(i) == Some(&b'.') {
        i += 1;
        frac_digits = bytes[i..].iter().take_while(|b| b.is_ascii_digit()).count();
        i += frac_digits;
    }
    if int_digits + frac_digits == 0 {
        return None;
    }
    if matches!(bytes.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(bytes.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = bytes[j.min(bytes.len())..]
            .iter()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if exp_digits == 0 {
            return None;
        }
        i = j + exp_digits;
    }
    if i != bytes.len() {
        return None;
    }
    let end = token.len() - usize::from(token.ends_with(['f', 'F']));
    token[..end].parse::<f64>().ok()
}

/// Splits a value string into fields the way the setters see them (blanks and commas separate).
pub fn tokenize_value(raw: &str) -> Vec<Value> {
    raw.split([',', ' ', '\t'])
        .filter(|t| !t.is_empty())
        .map(Value::parse)
        .collect()
}

/// One `keyword value...` line.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    /// Keyword exactly as written (case and any `[index]` suffix preserved).
    pub key: String,
    /// Value text after comment removal and blank trimming; what a setter would receive.
    pub raw: String,
    /// `raw` split into fields.
    pub values: Vec<Value>,
    /// 1-based source line.
    pub line: usize,
}

impl Entry {
    /// Keyword without its bracketed suffix: `thruststrength[TRANS_UP]` -> `thruststrength`.
    pub fn key_base(&self) -> &str {
        match self.key.find('[') {
            Some(i) if i > 0 => &self.key[..i],
            _ => &self.key,
        }
    }

    /// Bracketed parts of the keyword: `specialDoorOffset[R1][Carrier]` -> `["R1", "Carrier"]`.
    pub fn key_indices(&self) -> Vec<&str> {
        let mut out = Vec::new();
        let mut rest = &self.key[self.key_base().len()..];
        while let Some(open) = rest.strip_prefix('[') {
            match open.find(']') {
                Some(close) => {
                    out.push(&open[..close]);
                    rest = &open[close + 1..];
                }
                None => break,
            }
        }
        out
    }

    pub fn value(&self, i: usize) -> Option<&Value> {
        self.values.get(i)
    }

    pub fn f32(&self) -> Option<f32> {
        self.values.first().and_then(Value::as_f32)
    }

    pub fn i32(&self) -> Option<i32> {
        self.values.first().and_then(Value::as_i32)
    }

    pub fn bool(&self) -> Option<bool> {
        self.values.first().and_then(Value::as_bool)
    }

    /// The whole value as one string (file names, display strings); `None` when empty.
    pub fn string(&self) -> Option<&str> {
        (!self.raw.is_empty()).then_some(self.raw.as_str())
    }
}

/// Entries following a `[name]` header (or the file start, for the unnamed leading section).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Section {
    /// Text between the brackets; empty for entries that precede any header.
    pub name: String,
    /// Line of the header; 0 for the implicit leading section.
    pub line: usize,
    pub entries: Vec<Entry>,
}

/// A parsed tweak file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ScriptFile {
    /// Sections in file order. An unnamed section is present only if entries precede the first
    /// header.
    pub sections: Vec<Section>,
    /// Lines longer than [`ENGINE_MAX_LINE`]; the engine would have split these.
    pub overlong_lines: Vec<usize>,
}

/// Which grammar a text file follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Dialect {
    /// The keyword/value grammar described in the module docs.
    Tweak,
    /// Output of the C preprocessor (`#line` markers): effect `.ebg` sources and the NIS/animatic
    /// event scripts. These have their own grammars, though they still split into keyword lines.
    Preprocessed,
}

/// True when `bytes` looks like one of the game's text files (no control bytes besides tab,
/// line feed, carriage return and the DOS end-of-file marker).
pub fn is_text(bytes: &[u8]) -> bool {
    first_control_byte(bytes).is_none()
}

fn first_control_byte(bytes: &[u8]) -> Option<(usize, u8)> {
    bytes
        .iter()
        .position(|&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x1a))
        .map(|i| (i, bytes[i]))
}

fn latin1(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// Splits one line into `(keyword, value)` following the engine's rules, or `None` for blank,
/// comment-only and header lines.
pub fn split_line(line: &str) -> Option<(&str, &str)> {
    if line.is_empty() || line.starts_with(['[', ']']) {
        return None;
    }
    let mut body = line;
    if let Some(i) = body.find(';') {
        body = &body[..i];
    }
    if let Some(i) = body.find("//") {
        body = &body[..i];
    }
    let body = body.trim_start_matches([' ', '\t']);
    if body.is_empty() {
        return None;
    }
    let key_end = body.find([' ', '\t', '=']).unwrap_or(body.len());
    let key = &body[..key_end];
    let value = body.get(key_end + 1..).unwrap_or("");
    let value = value
        .trim_start_matches([' ', '\t'])
        .trim_end_matches([' ', '\t', '\r', '\n']);
    Some((key, value))
}

impl ScriptFile {
    /// Parses text that is already decoded.
    pub fn parse(text: &str) -> ScriptFile {
        let mut file = ScriptFile::default();
        let mut current = Section::default();
        for (idx, line) in text.split('\n').enumerate() {
            let line_no = idx + 1;
            let line = line.strip_suffix('\r').unwrap_or(line);
            let line = line.trim_end_matches('\u{1a}');
            if line.len() + 1 > ENGINE_MAX_LINE {
                file.overlong_lines.push(line_no);
            }
            if let Some(header) = line.strip_prefix('[') {
                let name = header.split(']').next().unwrap_or(header).trim();
                let finished = std::mem::replace(
                    &mut current,
                    Section {
                        name: name.to_owned(),
                        line: line_no,
                        entries: Vec::new(),
                    },
                );
                if finished.line != 0 || !finished.entries.is_empty() {
                    file.sections.push(finished);
                }
                continue;
            }
            if let Some((key, raw)) = split_line(line) {
                current.entries.push(Entry {
                    key: key.to_owned(),
                    raw: raw.to_owned(),
                    values: tokenize_value(raw),
                    line: line_no,
                });
            }
        }
        if current.line != 0 || !current.entries.is_empty() {
            file.sections.push(current);
        }
        file
    }

    /// Decodes and parses raw file bytes, rejecting binary data.
    pub fn parse_bytes(bytes: &[u8]) -> Result<ScriptFile, ScriptError> {
        if let Some((offset, byte)) = first_control_byte(bytes) {
            return Err(ScriptError::NotText { offset, byte });
        }
        Ok(ScriptFile::parse(&latin1(bytes)))
    }

    pub fn load(path: impl AsRef<Path>) -> Result<ScriptFile, ScriptError> {
        ScriptFile::parse_bytes(&std::fs::read(path)?)
    }

    /// Every entry in file order, ignoring section boundaries (as the engine does).
    pub fn entries(&self) -> impl Iterator<Item = &Entry> {
        self.sections.iter().flat_map(|s| s.entries.iter())
    }

    /// Last entry with this exact keyword (later lines override earlier ones in the engine).
    pub fn get(&self, key: &str) -> Option<&Entry> {
        self.entries().filter(|e| e.key == key).last()
    }

    /// All entries with this exact keyword, in order.
    pub fn get_all<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Entry> + 'a {
        self.entries().filter(move |e| e.key == key)
    }

    /// Name of the first non-empty section header (the object name in `.shp`/`.lod` files).
    pub fn title(&self) -> Option<&str> {
        self.sections
            .iter()
            .map(|s| s.name.as_str())
            .find(|n| !n.is_empty())
    }

    pub fn dialect(&self) -> Dialect {
        if self.entries().any(|e| e.key == "#line") {
            Dialect::Preprocessed
        } else {
            Dialect::Tweak
        }
    }

    pub fn entry_count(&self) -> usize {
        self.sections.iter().map(|s| s.entries.len()).sum()
    }
}

// ---------------------------------------------------------------------------------------------
// Ship statics (.shp)
// ---------------------------------------------------------------------------------------------

/// Index order of `thruststrength[...]`.
pub const THRUST_DIRECTIONS: [&str; 6] = [
    "TRANS_UP",
    "TRANS_DOWN",
    "TRANS_RIGHT",
    "TRANS_LEFT",
    "TRANS_FORWARD",
    "TRANS_BACK",
];
/// Index order of `rotstrength[...]`.
pub const ROT_DIRECTIONS: [&str; 6] = [
    "ROT_YAWLEFT",
    "ROT_YAWRIGHT",
    "ROT_PITCHUP",
    "ROT_PITCHDOWN",
    "ROT_ROLLRIGHT",
    "ROT_ROLLLEFT",
];
/// Index order of `turnspeed[...]`.
pub const TURN_AXES: [&str; 3] = ["TURN_YAW", "TURN_PITCH", "TURN_ROLL"];

/// Keywords written as `keyword  Tactic  Class  values...` (per-tactic, per-target-class tables).
pub const TACTICS_KEYWORDS: &[&str] = &[
    "flyPastDist",
    "maxFlyAwayDist",
    "breakRange",
    "triggerHappy",
    "faceTargetAccuracy",
    "maxAttackTime",
    "flightmanTurnaround",
    "flightmanAIP",
    "flightmanEvasiveBehind",
    "flightmanEvasiveFront",
    "flightmanEvasivePure",
];

/// One `GUN n { ... }` block.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GunStats {
    /// The number after `GUN`.
    pub index: Option<i32>,
    /// Line of the `GUN` keyword.
    pub line: usize,
    /// `Type` (e.g. `GUN_Gimble`, `GUN_MissileLauncher`).
    pub gun_type: Option<String>,
    /// `GunSize` (Cataclysm).
    pub gun_size: Option<String>,
    /// `StartUpgradeLevel` (Cataclysm).
    pub start_upgrade_level: Option<String>,
    /// `EndUpgradeLevel` (Cataclysm).
    pub end_upgrade_level: Option<String>,
    /// `DamageMult` (Cataclysm; replaces HW1's `DamageLo`/`DamageHi`).
    pub damage_mult: Option<f32>,
    /// `DamageLo`.
    pub damage_lo: Option<f32>,
    /// `DamageHi`.
    pub damage_hi: Option<f32>,
    /// `BulletType`.
    pub bullet_type: Option<String>,
    /// `SoundType`.
    pub sound_type: Option<String>,
    /// `BulletRange`.
    pub bullet_range: Option<f32>,
    /// `BulletSpeed`.
    pub bullet_speed: Option<f32>,
    /// `BulletSpeed2` (Cataclysm; second fire mode).
    pub bullet_speed2: Option<f32>,
    /// `BulletLength`.
    pub bullet_length: Option<f32>,
    /// `BulletMass`.
    pub bullet_mass: Option<f32>,
    /// `BulletLifeTime`.
    pub bullet_life_time: Option<f32>,
    /// `FireTime`.
    pub fire_time: Option<f32>,
    /// `FireTime2` (Cataclysm; second fire mode).
    pub fire_time2: Option<f32>,
    /// `BurstFireTime`.
    pub burst_fire_time: Option<f32>,
    /// `BurstWaitTime`.
    pub burst_wait_time: Option<f32>,
    /// `MinAngle` (degrees from the gun normal).
    pub min_angle: Option<f32>,
    /// `MaxAngle`.
    pub max_angle: Option<f32>,
    /// `TriggerHappy`.
    pub trigger_happy: Option<f32>,
    /// `MaxMissiles`.
    pub max_missiles: Option<i32>,
    /// Keywords not covered by a field above, by keyword.
    pub other: BTreeMap<String, Vec<Entry>>,
}

impl GunStats {
    fn apply(&mut self, e: &Entry) {
        let word = || e.string().map(str::to_owned);
        match e.key.as_str() {
            "Type" => self.gun_type = word(),
            "GunSize" => self.gun_size = word(),
            "StartUpgradeLevel" => self.start_upgrade_level = word(),
            "EndUpgradeLevel" => self.end_upgrade_level = word(),
            "DamageMult" => self.damage_mult = e.f32(),
            "DamageLo" => self.damage_lo = e.f32(),
            "DamageHi" => self.damage_hi = e.f32(),
            "BulletType" => self.bullet_type = word(),
            "SoundType" => self.sound_type = word(),
            "BulletRange" => self.bullet_range = e.f32(),
            "BulletSpeed" => self.bullet_speed = e.f32(),
            "BulletSpeed2" => self.bullet_speed2 = e.f32(),
            "BulletLength" => self.bullet_length = e.f32(),
            "BulletMass" => self.bullet_mass = e.f32(),
            "BulletLifeTime" => self.bullet_life_time = e.f32(),
            "FireTime" => self.fire_time = e.f32(),
            "FireTime2" => self.fire_time2 = e.f32(),
            "BurstFireTime" => self.burst_fire_time = e.f32(),
            "BurstWaitTime" => self.burst_wait_time = e.f32(),
            "MinAngle" => self.min_angle = e.f32(),
            "MaxAngle" => self.max_angle = e.f32(),
            "TriggerHappy" => self.trigger_happy = e.f32(),
            "MaxMissiles" => self.max_missiles = e.i32(),
            _ => self.other.entry(e.key.clone()).or_default().push(e.clone()),
        }
    }
}

/// A `keyword Tactic Class values...` line.
#[derive(Debug, Clone, PartialEq)]
pub struct TacticsStat {
    pub key: String,
    /// `Evasive`, `Neutral` or `Aggressive`.
    pub tactic: String,
    /// `CLASS_...` or `Default`.
    pub class: String,
    pub values: Vec<Value>,
    pub line: usize,
}

/// Typed view of a `.shp` file. Field names follow the keywords; every keyword without a field
/// lands in [`ShipStats::other`], so nothing in the file is dropped.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShipStats {
    /// Header name (`[bAcolyte]`).
    pub name: String,
    /// `LODFile`.
    pub lod_file: Option<String>,
    /// `pMexData`.
    pub p_mex_data: Option<String>,
    /// `mass`.
    pub mass: Option<f32>,
    /// `momentOfInertiaX`.
    pub moment_of_inertia_x: Option<f32>,
    /// `momentOfInertiaY`.
    pub moment_of_inertia_y: Option<f32>,
    /// `momentOfInertiaZ`.
    pub moment_of_inertia_z: Option<f32>,
    /// `maxvelocity`.
    pub maxvelocity: Option<f32>,
    /// `maxrot`.
    pub maxrot: Option<f32>,
    /// `thruststrength[...]`, indexed as [`THRUST_DIRECTIONS`].
    pub thruststrength: [Option<f32>; 6],
    /// `rotstrength[...]`, indexed as [`ROT_DIRECTIONS`].
    pub rotstrength: [Option<f32>; 6],
    /// `turnspeed[...]`, indexed as [`TURN_AXES`].
    pub turnspeed: [Option<f32>; 3],
    /// `shipclass` (`CLASS_Fighter`, ...).
    pub shipclass: Option<String>,
    /// `isCapitalShip`.
    pub is_capital_ship: Option<bool>,
    /// `buildCost` (resource units).
    pub build_cost: Option<i32>,
    /// `buildTime` (seconds).
    pub build_time: Option<f32>,
    /// `supportUnits` (Cataclysm unit cap cost).
    pub support_units: Option<i32>,
    /// `providesSU` (Cataclysm).
    pub provides_su: Option<i32>,
    /// `maxhealth`.
    pub maxhealth: Option<f32>,
    /// `groupSize`.
    pub group_size: Option<i32>,
    /// `repairTime`.
    pub repair_time: Option<f32>,
    /// `repairDamage`.
    pub repair_damage: Option<f32>,
    /// `repairCombatTime`.
    pub repair_combat_time: Option<f32>,
    /// `repairCombatDamage`.
    pub repair_combat_damage: Option<f32>,
    /// `repairDroids` (Cataclysm).
    pub repair_droids: Option<i32>,
    /// `externalBuild` (Cataclysm).
    pub external_build: Option<bool>,
    /// `fowRadius` (Cataclysm fog-of-war radius).
    pub fow_radius: Option<f32>,
    /// `TimeShockedByEMP` (Cataclysm).
    pub time_shocked_by_emp: Option<f32>,
    /// `explosionType` (Cataclysm).
    pub explosion_type: Option<String>,
    /// `minimumZoomDistance`.
    pub minimum_zoom_distance: Option<f32>,
    /// `NUMBER_OF_GUNS` as declared (may disagree with `guns.len()`).
    pub number_of_guns: Option<i32>,
    /// `GUN n { ... }` blocks in file order.
    pub guns: Vec<GunStats>,
    /// Per-tactic/per-class lines ([`TACTICS_KEYWORDS`]).
    pub tactics: Vec<TacticsStat>,
    /// `DockPoint` lines.
    pub dock_points: Vec<Entry>,
    /// `NavLight` lines.
    pub nav_lights: Vec<Entry>,
    /// Every other keyword, in file order per keyword.
    pub other: BTreeMap<String, Vec<Entry>>,
}

fn indexed_slot<const N: usize>(
    slots: &mut [Option<f32>; N],
    names: &[&str; N],
    e: &Entry,
) -> bool {
    let idx = e.key_indices();
    match idx.as_slice() {
        [one] => match names.iter().position(|n| n == one) {
            Some(i) => {
                slots[i] = e.f32();
                true
            }
            None => false,
        },
        _ => false,
    }
}

impl ShipStats {
    pub fn from_script(file: &ScriptFile) -> ShipStats {
        let mut s = ShipStats {
            name: file.title().unwrap_or_default().to_owned(),
            ..Default::default()
        };
        let mut pending_gun: Option<GunStats> = None;
        let mut in_block = false;
        for e in file.entries() {
            match e.key.as_str() {
                "{" => {
                    in_block = true;
                    continue;
                }
                "}" => {
                    in_block = false;
                    if let Some(g) = pending_gun.take() {
                        s.guns.push(g);
                    }
                    continue;
                }
                _ => {}
            }
            if in_block {
                match pending_gun.as_mut() {
                    Some(g) => g.apply(e),
                    None => s.other.entry(e.key.clone()).or_default().push(e.clone()),
                }
                continue;
            }
            s.apply(e, &mut pending_gun);
        }
        if let Some(g) = pending_gun {
            s.guns.push(g);
        }
        s
    }

    fn apply(&mut self, e: &Entry, pending_gun: &mut Option<GunStats>) {
        let word = || e.string().map(str::to_owned);
        if TACTICS_KEYWORDS.contains(&e.key.as_str()) {
            let mut it = e.values.iter();
            let tactic = it.next().map(Value::to_string).unwrap_or_default();
            let class = it.next().map(Value::to_string).unwrap_or_default();
            self.tactics.push(TacticsStat {
                key: e.key.clone(),
                tactic,
                class,
                values: it.cloned().collect(),
                line: e.line,
            });
            return;
        }
        match e.key_base() {
            "thruststrength" if indexed_slot(&mut self.thruststrength, &THRUST_DIRECTIONS, e) => {
                return
            }
            "rotstrength" if indexed_slot(&mut self.rotstrength, &ROT_DIRECTIONS, e) => return,
            "turnspeed" if indexed_slot(&mut self.turnspeed, &TURN_AXES, e) => return,
            _ => {}
        }
        match e.key.as_str() {
            "LODFile" => self.lod_file = word(),
            "pMexData" => self.p_mex_data = word(),
            "mass" => self.mass = e.f32(),
            "momentOfInertiaX" => self.moment_of_inertia_x = e.f32(),
            "momentOfInertiaY" => self.moment_of_inertia_y = e.f32(),
            "momentOfInertiaZ" => self.moment_of_inertia_z = e.f32(),
            "maxvelocity" => self.maxvelocity = e.f32(),
            "maxrot" => self.maxrot = e.f32(),
            "shipclass" => self.shipclass = word(),
            "isCapitalShip" => self.is_capital_ship = e.bool(),
            "buildCost" => self.build_cost = e.i32(),
            "buildTime" => self.build_time = e.f32(),
            "supportUnits" => self.support_units = e.i32(),
            "providesSU" => self.provides_su = e.i32(),
            "maxhealth" => self.maxhealth = e.f32(),
            "groupSize" => self.group_size = e.i32(),
            "repairTime" => self.repair_time = e.f32(),
            "repairDamage" => self.repair_damage = e.f32(),
            "repairCombatTime" => self.repair_combat_time = e.f32(),
            "repairCombatDamage" => self.repair_combat_damage = e.f32(),
            "repairDroids" => self.repair_droids = e.i32(),
            "externalBuild" => self.external_build = e.bool(),
            "fowRadius" => self.fow_radius = e.f32(),
            "TimeShockedByEMP" => self.time_shocked_by_emp = e.f32(),
            "explosionType" => self.explosion_type = word(),
            "minimumZoomDistance" => self.minimum_zoom_distance = e.f32(),
            "NUMBER_OF_GUNS" => self.number_of_guns = e.i32(),
            "GUN" => {
                if let Some(g) = pending_gun.take() {
                    self.guns.push(g);
                }
                *pending_gun = Some(GunStats {
                    index: e.i32(),
                    line: e.line,
                    ..Default::default()
                });
            }
            "DockPoint" => self.dock_points.push(e.clone()),
            "NavLight" => self.nav_lights.push(e.clone()),
            _ => self.other.entry(e.key.clone()).or_default().push(e.clone()),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Keyword survey
// ---------------------------------------------------------------------------------------------

/// Which ship keywords occur where, split into top-level and gun-block keywords.
#[derive(Debug, Clone, Default)]
pub struct ShipKeywordSurvey {
    /// Top-level keyword -> files using it.
    pub top_level: BTreeMap<String, BTreeSet<String>>,
    /// Keyword inside `{ }` blocks -> files using it.
    pub gun: BTreeMap<String, BTreeSet<String>>,
    pub files: usize,
}

/// A keyword the original Homeworld engine never binds.
#[derive(Debug, Clone, PartialEq)]
pub struct UnboundKeyword<'a> {
    pub key: &'a str,
    pub files: &'a BTreeSet<String>,
    /// HW1 keyword that differs only by letter case, if any (such lines are ignored by a
    /// case-sensitive loader).
    pub case_variant_of: Option<&'static str>,
}

impl ShipKeywordSurvey {
    pub fn add(&mut self, file_name: &str, file: &ScriptFile) {
        self.files += 1;
        let mut depth = 0usize;
        for e in file.entries() {
            match e.key.as_str() {
                "{" => depth += 1,
                "}" => depth = depth.saturating_sub(1),
                key => {
                    let map = if depth > 0 {
                        &mut self.gun
                    } else {
                        &mut self.top_level
                    };
                    map.entry(key.to_owned())
                        .or_default()
                        .insert(file_name.to_owned());
                }
            }
        }
    }

    fn unbound<'a>(
        map: &'a BTreeMap<String, BTreeSet<String>>,
        known: &'static [&'static str],
    ) -> Vec<UnboundKeyword<'a>> {
        map.iter()
            .filter(|(k, _)| !known.contains(&k.as_str()))
            .map(|(k, files)| UnboundKeyword {
                key: k,
                files,
                case_variant_of: known.iter().copied().find(|h| h.eq_ignore_ascii_case(k)),
            })
            .collect()
    }

    /// Top-level keywords used by Cataclysm that HW1 never binds.
    pub fn cataclysm_only_top_level(&self) -> Vec<UnboundKeyword<'_>> {
        Self::unbound(&self.top_level, HW1_SHIP_KEYWORDS)
    }

    /// Gun-block keywords used by Cataclysm that HW1 never binds.
    pub fn cataclysm_only_gun(&self) -> Vec<UnboundKeyword<'_>> {
        Self::unbound(&self.gun, HW1_GUN_KEYWORDS)
    }

    /// HW1 ship keywords that no surveyed file uses.
    pub fn hw1_unused(&self) -> Vec<&'static str> {
        HW1_SHIP_KEYWORDS
            .iter()
            .copied()
            .filter(|k| !self.top_level.contains_key(*k))
            .chain(
                HW1_GUN_KEYWORDS
                    .iter()
                    .copied()
                    .filter(|k| !self.gun.contains_key(*k)),
            )
            .collect()
    }
}

/// Every keyword the Homeworld 1 loader binds for ship, derelict, missile, mine and resource
/// statics files (top level, including the per-ship-type extension tables, dock/nav-light/salvage
/// point lists, trail settings and the `GUN` header). Gun-block keywords are in
/// [`HW1_GUN_KEYWORDS`].
pub const HW1_SHIP_KEYWORDS: &[&str] = &[
    "AngleDotProdThreshold",
    "angleVariance",
    "approachAndWaitDistance",
    "approxcollmodifier",
    "asteroidCanBreak",
    "asteroidCanShrink",
    "asteroidMinShrink",
    "attackfuelburnrate",
    "attackRange",
    "AttackRotationSpeed",
    "avoidcollmodifier",
    "battleReCloakTime",
    "BeamFireTime",
    "BindAnimations",
    "BindGun",
    "blastRadiusDamage",
    "blastRadiusShockWave",
    "blipColor",
    "blipThickness",
    "Break2SphereizeFreq",
    "breakInAwayDist",
    "breakinto[Asteroid0]",
    "breakinto[Asteroid1]",
    "breakinto[Asteroid2]",
    "breakinto[Asteroid3]",
    "breakinto[Asteroid4]",
    "breakRange",
    "buildCost",
    "buildTime",
    "BulletDamageMultiplier",
    "BulletLifeExtension",
    "bulletRange[0]",
    "bulletRange[1]",
    "bulletRange[2]",
    "canBuildBigShips",
    "canBuildShips",
    "canHandleNumShipsDepositingRU",
    "canHandleNumShipsDocking",
    "cannotForceAttackIfOwnShip",
    "canReceiveCorvettes",
    "canReceiveFighters",
    "canReceiveOtherNonCapitalShips",
    "canReceiveResourceCollectors",
    "canReceiveResources",
    "canReceiveShips",
    "canReceiveShipsForRetire",
    "canReceiveShipsPermanently",
    "canSingleClickSpecialActivate",
    "canSpecialBandBoxFriendlies",
    "canTargetMultipleTargets",
    "cantMoveAndAttack",
    "CapitalDistanceRepairStart",
    "CapitalDistanceRepairStart2",
    "capitalShipHealthPerSecond",
    "CIRCLE_RIGHT_THRUST",
    "CIRCLE_RIGHT_VELOCITY",
    "circleRange",
    "clearanceDirection",
    "clearanceDistance",
    "CloakFuelBurnRate",
    "CloakingRadius",
    "CloakingTime",
    "collBoxOffsetX",
    "collBoxOffsetY",
    "collBoxOffsetZ",
    "collBoxScaleForward",
    "collBoxScaleRight",
    "collBoxScaleUp",
    "collSideModifiers[TRANS_BACK]",
    "collSideModifiers[TRANS_DOWN]",
    "collSideModifiers[TRANS_FORWARD]",
    "collSideModifiers[TRANS_LEFT]",
    "collSideModifiers[TRANS_RIGHT]",
    "collSideModifiers[TRANS_UP]",
    "collspheresize",
    "CoolDownTimePerLaser",
    "DamageRate",
    "DamageReductionHigh",
    "DamageReductionLow",
    "DeCloakingTime",
    "dockLightColor",
    "dockLightFar",
    "dockLightNear",
    "DockOveride",
    "DockPoint",
    "dockShipRange",
    "dockWithRotationSpeed",
    "droneDeploymentRange",
    "DropRange",
    "DropStopRange",
    "dustcloudCanShrink",
    "dustcloudMinShrink",
    "EffectConstant",
    "externalRegenerateRate",
    "faceTargetAccuracy",
    "fieldRadius",
    "finaldockDistance",
    "fireDownTime",
    "flightmanAIP",
    "flightmanEvasiveBehind",
    "flightmanEvasiveFront",
    "flightmanEvasivePure",
    "flightmanTurnaround",
    "FlyAwayDist[CLASS_Carrier]",
    "FlyAwayDist[CLASS_Corvette]",
    "FlyAwayDist[CLASS_Destroyer]",
    "FlyAwayDist[CLASS_Fighter]",
    "FlyAwayDist[CLASS_Frigate]",
    "FlyAwayDist[CLASS_HeavyCruiser]",
    "FlyAwayDist[CLASS_Mothership]",
    "FlyAwayDist[CLASS_NonCombat]",
    "FlyAwayDist[CLASS_Resource]",
    "FlyAwayTolerance",
    "flyPastDist",
    "flyToDistance",
    "forced_drop_damage_hi",
    "forced_drop_damage_lo",
    "forced_drop_lifetime",
    "ForceDeCloakAtFuelPercent",
    "formationPaddingModifier",
    "fuelburnrate",
    "fullMovementFreedom",
    "gascloudCanShrink",
    "gascloudMinShrink",
    "getTechTime",
    "GravWellRadius",
    "groupSize",
    "GUN",
    "gunReFireTime",
    "gunsFireTime",
    "harvestableByMultipleShips",
    "harvestAmount",
    "harvestRate",
    "healthPerSecond",
    "healthRemovedPerSecond",
    "HealthThreshold",
    "hyperspaceColor",
    "immobile",
    "internalRegenerateRate",
    "isCapitalShip",
    "LODFile",
    "lowfuelpoint",
    "MadDockAnims",
    "MadGunAnims",
    "madGunOpenDamagedHealthThreshold",
    "MadMothershipDockingAnims",
    "MadSpecialCaseAnims",
    "MadStartInfo",
    "madWingOpenDamagedHealthThreshold",
    "mass",
    "max_rot_speed",
    "max_rotate",
    "maxAttackTime",
    "MaxCloakingTime",
    "maxCollDamage",
    "maxDockableCorvettes",
    "maxDockableFighters",
    "maxFlyAwayDist",
    "maxfuel",
    "maxhealth",
    "maxPushingVelocitySingle",
    "maxresources",
    "maxrot",
    "maxvelocity",
    "maxvelocity_FORCED",
    "minBulletRange[0]",
    "minBulletRange[1]",
    "minBulletRange[2]",
    "MinCharge",
    "minCollDamage",
    "MINE_RANGESQR",
    "MINE_RANGESQR_FORCED",
    "MINE_STOP_FRICTION",
    "MineClearDistance",
    "MineDropDistance",
    "mineRegenerateTime",
    "MineSpacing",
    "minimumZoomDistance",
    "MissileLagVolleyTime",
    "MissileRegenerateTime",
    "MissileVolleyTime",
    "momentOfInertiaX",
    "momentOfInertiaY",
    "momentOfInertiaZ",
    "MultiBeamRange",
    "MultipleTargettingofSingleBullet",
    "N-LIPS",
    "NavLight",
    "nebulaCanShrink",
    "nebulaMinShrink",
    "NEED_BIGR1",
    "NEED_BIGR2",
    "noLightClampingDistance",
    "NUM_NEEDED_FOR_SALVAGE",
    "NUMBER_OF_DOCK_OVERIDES",
    "NUMBER_OF_DOCK_POINTS",
    "NUMBER_OF_GUNS",
    "NUMBER_OF_NAV_LIGHTS",
    "NUMBER_OF_SALVAGE_POINTS",
    "NumMinesInSide",
    "NumTargetsCanAttack",
    "OperationTime",
    "Overlay_Selection_Scalar",
    "passiveAttackPenaltyExempt",
    "passiveRetaliateZone",
    "percentageThatGetThrough",
    "pitchdescend",
    "pitchturn",
    "pMexData",
    "preciseSelection",
    "ProbeDispatchMaxVelocity",
    "pumpFuelRate",
    "R1final_dock_distance",
    "R1parallel_dock_distance",
    "R1VerticalDockDistance",
    "R2DockFinalDistance",
    "RangeCheckRate",
    "ReChargeRate",
    "renderlistFade",
    "renderlistLimit",
    "repairApproachDistance",
    "repairBeamCapable",
    "repairCombatDamage",
    "repairCombatTime",
    "repairDamage",
    "repairOtherShipRate",
    "repairResourceCollectorRate",
    "repairTime",
    "repositionTime",
    "repulseForce",
    "resourcesAtOneTime",
    "resourcevalue",
    "rightOfWay",
    "rotate_acelleration",
    "rotate_recover_time",
    "rotate_slow",
    "rotate_time",
    "rotateToRetaliate",
    "RotationAngle",
    "rotationStopDistance",
    "rotstrength[ROT_PITCHDOWN]",
    "rotstrength[ROT_PITCHUP]",
    "rotstrength[ROT_ROLLLEFT]",
    "rotstrength[ROT_ROLLRIGHT]",
    "rotstrength[ROT_YAWLEFT]",
    "rotstrength[ROT_YAWRIGHT]",
    "salvageable",
    "SalvagePoint",
    "scaleFactor",
    "scanrate",
    "SearchRadius",
    "SearchRate",
    "SearchRateAfterFound",
    "SensorBlinkRate",
    "SensorCircleRadius",
    "shipclass",
    "sinbank",
    "sloppyRotThreshold",
    "specialActivateIsContinuous",
    "specialDoorInterpolationPerSecond",
    "specialDoorOffset",
    "specialDoorOffset[P1][P1IonArrayFrigate]",
    "specialDoorOffset[P2][P2FuelPod]",
    "specialDoorOffset[P2][P2MultiBeamFrigate]",
    "specialDoorOffset[R1][AdvanceSupportFrigate]",
    "specialDoorOffset[R1][Carrier]",
    "specialDoorOffset[R1][CloakGenerator]",
    "specialDoorOffset[R1][CryoTray]",
    "specialDoorOffset[R1][DDDFrigate]",
    "specialDoorOffset[R1][DFGFrigate]",
    "specialDoorOffset[R1][GravWellGenerator]",
    "specialDoorOffset[R1][HeavyCruiser]",
    "specialDoorOffset[R1][IonCannonFrigate]",
    "specialDoorOffset[R1][MissileDestroyer]",
    "specialDoorOffset[R1][P1IonArrayFrigate]",
    "specialDoorOffset[R1][P2FuelPod]",
    "specialDoorOffset[R1][P2MultiBeamFrigate]",
    "specialDoorOffset[R1][ResourceController]",
    "specialDoorOffset[R1][StandardDestroyer]",
    "specialDoorOffset[R1][StandardFrigate]",
    "specialDoorOffset[R2][AdvanceSupportFrigate]",
    "specialDoorOffset[R2][Carrier]",
    "specialDoorOffset[R2][CloakGenerator]",
    "specialDoorOffset[R2][CryoTray]",
    "specialDoorOffset[R2][DDDFrigate]",
    "specialDoorOffset[R2][DFGFrigate]",
    "specialDoorOffset[R2][GravWellGenerator]",
    "specialDoorOffset[R2][HeavyCruiser]",
    "specialDoorOffset[R2][IonCannonFrigate]",
    "specialDoorOffset[R2][MissileDestroyer]",
    "specialDoorOffset[R2][P1IonArrayFrigate]",
    "specialDoorOffset[R2][P2FuelPod]",
    "specialDoorOffset[R2][P2MultiBeamFrigate]",
    "specialDoorOffset[R2][ResourceController]",
    "specialDoorOffset[R2][StandardDestroyer]",
    "specialDoorOffset[R2][StandardFrigate]",
    "startdockTolerance",
    "stopRotMultiplier",
    "svCoverage",
    "svFirePower",
    "svManeuverability",
    "TargetOwnBullets",
    "targetStartDockDistance",
    "thruststrength[TRANS_BACK]",
    "thruststrength[TRANS_DOWN]",
    "thruststrength[TRANS_FORWARD]",
    "thruststrength[TRANS_LEFT]",
    "thruststrength[TRANS_RIGHT]",
    "thruststrength[TRANS_UP]",
    "tooCloseRange",
    "trailAngle",
    "trailAngle2",
    "trailAngle3",
    "trailAngle4",
    "trailColor",
    "trailGranularity",
    "trailHeight",
    "trailHeight2",
    "trailHeight3",
    "trailHeight4",
    "trailLength",
    "trailLength2",
    "trailLength3",
    "trailLength4",
    "trailRibbonAdjust",
    "trailRibbonAdjust2",
    "trailRibbonAdjust3",
    "trailRibbonAdjust4",
    "trailScaleCap",
    "trailScaleCap2",
    "trailScaleCap3",
    "trailScaleCap4",
    "trailSegments",
    "trailSpriteOffset",
    "trailSpriteOffset2",
    "trailSpriteOffset3",
    "trailSpriteOffset4",
    "trailSpriteRadius",
    "trailSpriteRadius2",
    "trailSpriteRadius3",
    "trailSpriteRadius4",
    "trailStyle",
    "trailStyle2",
    "trailStyle3",
    "trailStyle4",
    "trailWidth",
    "trailWidth2",
    "trailWidth3",
    "trailWidth4",
    "triggerHappy",
    "TriggerSpeed",
    "turnspeed[TURN_PITCH]",
    "turnspeed[TURN_ROLL]",
    "turnspeed[TURN_YAW]",
    "VisibleState",
    "warmupdowntime",
    "WILL_FIT_CARRIER",
    "worldRender",
    "xrot",
    "yrot",
    "zrot",
];
/// Keywords the Homeworld 1 loader binds inside a `GUN n { ... }` block.
pub const HW1_GUN_KEYWORDS: &[&str] = &[
    "angletracking",
    "BarrelLength",
    "BulletLength",
    "BulletLifeTime",
    "BulletMass",
    "BulletRange",
    "BulletSpeed",
    "BulletType",
    "BurstFireTime",
    "BurstWaitTime",
    "DamageHi",
    "DamageLo",
    "declinationtracking",
    "FireTime",
    "MaxAngle",
    "maxanglespeed",
    "maxdeclination",
    "maxdeclinationspeed",
    "MaxMissiles",
    "maxturnangle",
    "MinAngle",
    "mindeclination",
    "minturnangle",
    "OffsetX",
    "OffsetY",
    "OffsetZ",
    "RecoilLength",
    "SlaveDriver",
    "SoundType",
    "TriggerHappy",
    "Type",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers() {
        assert_eq!(Value::parse("42"), Value::Int(42));
        assert_eq!(Value::parse("-7"), Value::Int(-7));
        assert_eq!(Value::parse("0x1F"), Value::Int(31));
        assert_eq!(Value::parse("3.5"), Value::Float(3.5));
        assert_eq!(Value::parse("240.0f"), Value::Float(240.0));
        assert_eq!(Value::parse("-60339.4"), Value::Float(-60339.4));
        assert_eq!(Value::parse("5e-5"), Value::Float(5e-5));
        assert_eq!(Value::parse(".5"), Value::Float(0.5));
        assert_eq!(Value::parse("1."), Value::Float(1.0));
        assert_eq!(Value::parse("?"), Value::Unknown);
        assert_eq!(Value::parse("inf"), Value::Word("inf".into()));
        assert_eq!(Value::parse("e5"), Value::Word("e5".into()));
        assert_eq!(Value::parse("Rl0"), Value::Word("Rl0".into()));
        assert_eq!(Value::parse("1e"), Value::Word("1e".into()));
    }

    #[test]
    fn lines() {
        assert_eq!(split_line("mass   10   ; ship mass"), Some(("mass", "10")));
        assert_eq!(
            split_line("\tkey\tvalue words // c"),
            Some(("key", "value words"))
        );
        assert_eq!(split_line("key=v"), Some(("key", "v")));
        assert_eq!(split_line("key = v"), Some(("key", "= v")));
        assert_eq!(split_line("lonely"), Some(("lonely", "")));
        assert_eq!(split_line("   ; only comment"), None);
        assert_eq!(split_line("[Section]"), None);
        assert_eq!(split_line("str  a; b"), Some(("str", "a")));
        assert_eq!(split_line(" [indented]"), Some(("[indented]", "")));
    }

    #[test]
    fn file_structure() {
        let text = "; header\r\nloose 1\r\n[Ship]\r\nthruststrength[TRANS_UP] 850\r\n\
                    GUN 0\r\n{\r\nType GUN_Gimble\r\nDamageMult 1.5\r\nOdd x\r\n}\r\n\
                    flyPastDist Evasive Default 3300.0\r\nDockPoint Link,DPT_Link,185,185,200,0,2\r\n\
                    trailColor 0, 1, 238, 238, 238\r\nmaxhealth 270\r\nmaxhealth 300\r\n";
        let f = ScriptFile::parse(text);
        assert_eq!(f.sections.len(), 2);
        assert_eq!(f.sections[0].name, "");
        assert_eq!(f.sections[1].name, "Ship");
        assert_eq!(f.title(), Some("Ship"));
        let e = f.get("thruststrength[TRANS_UP]").unwrap();
        assert_eq!(e.key_base(), "thruststrength");
        assert_eq!(e.key_indices(), vec!["TRANS_UP"]);
        assert_eq!(e.line, 4);
        assert_eq!(f.get("trailColor").unwrap().values.len(), 5);

        let s = ShipStats::from_script(&f);
        assert_eq!(s.name, "Ship");
        assert_eq!(s.thruststrength[0], Some(850.0));
        assert_eq!(s.maxhealth, Some(300.0));
        assert_eq!(s.guns.len(), 1);
        assert_eq!(s.guns[0].index, Some(0));
        assert_eq!(s.guns[0].gun_type.as_deref(), Some("GUN_Gimble"));
        assert_eq!(s.guns[0].damage_mult, Some(1.5));
        assert!(s.guns[0].other.contains_key("Odd"));
        assert_eq!(s.tactics[0].class, "Default");
        assert_eq!(s.tactics[0].values, vec![Value::Float(3300.0)]);
        assert_eq!(s.dock_points.len(), 1);
        assert!(s.other.contains_key("loose") && s.other.contains_key("trailColor"));

        let mut survey = ShipKeywordSurvey::default();
        survey.add("ship.shp", &f);
        let only: Vec<_> = survey.cataclysm_only_gun().iter().map(|u| u.key).collect();
        assert_eq!(only, vec!["DamageMult", "Odd"]);
    }

    #[test]
    fn rejects_binary() {
        assert!(matches!(
            ScriptFile::parse_bytes(b"RMF99ba\0\0"),
            Err(ScriptError::NotText { offset: 7, byte: 0 })
        ));
        assert!(ScriptFile::parse_bytes("café 1\n".as_bytes()).is_ok());
    }
}
