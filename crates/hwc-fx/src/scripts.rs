//! The effect event tables loaded from `ETG\*.script` (`etgStartup` and the `scriptSet`
//! callbacks): which effects play for deaths, gun fire/hits/bullets, damage, resourcing,
//! tractor beams, hyperspace and the special-purpose slots, plus bullet colours and the
//! "big death" factors. Each table entry is an LOD list: every assignment to the same slot
//! appends another level (`etgLODLoad`).

use crate::cfmt::{scan_float, scan_int, StrTok};
use crate::compiler::{CompileError, FileSource, Library};

/// Race names (`StrToShipRace`, Cataclysm table at 0x87f8c8).
pub const RACES: &[&str] = &["RACE_SECT", "RACE_BEAST", "RACE_KUSHAN", "RACE_TAIDAN", "RACE_P1", "RACE_TRADERS", "RACE_DERELICT"];
/// Ship classes (`StrToShipClass`, 0x87f988).
pub const CLASSES: &[&str] = &[
    "CLASS_Mothership",
    "CLASS_HeavyCruiser",
    "CLASS_Carrier",
    "CLASS_Destroyer",
    "CLASS_Frigate",
    "CLASS_Corvette",
    "CLASS_Fighter",
    "CLASS_Resource",
    "CLASS_NonCombat",
    "CLASS_MonsterShip",
    "CLASS_Component",
    "CLASS_Micro",
];
/// Derelict types (`StrToDerelictType`, 0x87fa70).
pub const DERELICTS: &[&str] = &[
    "Crate",
    "Homeworld",
    "PlanetOfOrigin",
    "PlanetOfOrigin_scarred",
    "Galaxy",
    "LavaBall",
    "MattWorld",
    "HyperspaceGate",
];
/// Gun sound types (`StrToGunSoundType`, 0x87fca8).
pub const GUN_SOUNDS: &[&str] = &[
    "GS_LargeEnergyCannon",
    "GS_LargeIonCannon",
    "GS_LargePlasmaBomb",
    "GS_LargeProjectile",
    "GS_MediumEnergyCannon",
    "GS_MediumIonCannon",
    "GS_MediumPlasmaBomb",
    "GS_MediumProjectile",
    "GS_MineLauncher",
    "GS_MissleLauncher",
    "GS_SmallEnergyCannon",
    "GS_SmallIonCannon",
    "GS_SmallPlasmaBomb",
    "GS_SmallProjectile",
    "GS_VeryLargeEnergyCannon",
    "GS_VeryLargeIonCannon",
    "GS_VeryLargePlasmaBomb",
    "GS_Laser",
    "GS_BigGun",
    "GS_BeastWeapon",
];
/// Special-purpose effect slots (`StrToEffectNum`, 0x87ff50): name and slot number.
pub const SPECIAL_EFFECTS: &[(&str, usize)] = &[
    ("EGT_MINE_WALL_EFFECT", 0),
    ("EGT_BURST_CHARING_EFFECT", 1),
    ("EGT_BURST_MUZZLE_EFFECT", 2),
    ("EGT_BURST_BULLET_EFFECT", 3),
    ("EGT_BURST_HIT_EFFECT", 4),
    ("EGT_BURST_EXPLOSION_EFFECT", 5),
    ("EGT_CLOAK_ON", 6),
    ("EGT_CLOAK_OFF", 7),
    ("EGT_GRAVWELL_ON", 8),
    ("EGT_GRAVWELL_OFF", 9),
    ("EGT_LIGHTNING_GLOW", 10),
    ("EGT_BULLET_DEFLECT", 12),
    ("EGT_CRATE_GENERATED", 13),
    ("EGT_CRATE_IS_FOUND_SHIP", 14),
    ("EGT_CRATE_IS_FOUND_MONEY", 15),
    ("EGT_CRATE_IS_FOUND_RESEARCH", 16),
    ("EGT_CRATE_TIME_OUT", 17),
    ("EGT_REPAIR_BEAM", 18),
    ("EGT_CAUGHT_GRAVWELL", 19),
    ("EGT_WELDING_EFFECT", 20),
    ("EGT_LINKING_EFFECT", 26),
    ("EGT_RESOURCE_SOLID", 21),
    ("EGT_VORTEX_EFFECT", 22),
    ("EGT_BIG_GUN_BULLET_EXPLOSION", 27),
    ("EGT_BIG_GUN_CHARGING", 28),
    ("EGT_ELECTRO_MAGNETIC_PULSE", 23),
    ("EGT_SUCK_SHIP", 34),
    ("EGT_MOTHER_EXPLOSION_SHOCKWAVE", 42),
    ("EGT_DISTORTION_FIELD", 39),
    ("EGT_AFTER_BURNER", 40),
    ("EGT_RAM_VENT", 41),
    ("EGT_CRYSTAL_SHOCK_WAVE", 36),
    ("EGT_BULLET_CUTTING_BEAM", 37),
    ("EGT_RESOURCE_SHIP", 35),
    ("EGT_SHOCKWAVE_STAR", 33),
    ("EGT_LEECH_COLLECT_EFFECT", 24),
    ("EGT_LEECH_VENT_EFFECT", 25),
    ("EGT_LINKING_EFFECT", 26),
    ("EGT_SLIPGATE_OPENING_EFFECT", 29),
    ("EGT_SLIPGATE_CLOSING_EFFECT", 30),
    ("EGT_SLIPGATE_PERMANENT_EFFECT", 31),
    ("EGT_RAMMING_FRIGATE_IMPACT_EFFECT", 32),
    ("EGT_WORKER_RESOURCE_SOLID", 38),
];
/// Number of special-purpose slots.
pub const NUM_SPECIAL_EFFECTS: usize = 43;
/// Explosion types (`EDT_*`).
pub const EXPLOSION_TYPES: &[&str] = &["damage", "projectile", "beam", "collision", "special"];
/// Gun event kinds (`EGT_GunFire` ...); the fifth slot is unused.
pub const GUN_FIRE: usize = 0;
pub const GUN_HIT: usize = 1;
pub const GUN_BULLET: usize = 2;
pub const GUN_BULLET_DESTROYED: usize = 3;

/// The script files `etgStartup` reads, in order (0x870b08).
pub const SCRIPT_FILES: &[&str] = &[
    "General.script",
    "RACE_SECT.script",
    "RACE_BEAST.script",
    "RACE_KUSHAN.script",
    "RACE_TAIDAN.script",
    "RACE_P1.script",
    "RACE_TRADERS.script",
    "RACE_DERELICT.script",
];

/// An LOD list of effect statics (`etglod`); `None` means the slot was set to `NULL`.
pub type Lod = Option<Vec<usize>>;

/// `R2BulletColor` / `R1BulletColor` defaults: the first race row gets the second colour.
const DEFAULT_BULLET_COLOR: u32 = 0xffff_b864; // colRGB(100,184,255)
const R1_BULLET_COLOR: u32 = 0xff00_b4ff; // colRGB(255,180,0)

/// The event tables.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EtgTables {
    pub death: Vec<Lod>,
    pub death_derelict: Vec<Lod>,
    pub gun: Vec<Lod>,
    pub resource: Vec<Lod>,
    pub tractor_beam: Vec<Lod>,
    pub special: Vec<Lod>,
    pub damage: Vec<Lod>,
    pub hyperspace: Lod,
    pub bullet_color: Vec<u32>,
    pub big_death: Vec<i32>,
    pub big_death_derelict: Vec<i32>,
    pub number_effects: i32,
    pub history_scalar_min: i32,
    pub software_scalar_damage: f32,
    pub software_scalar_hit: f32,
    pub software_scalar_fire: f32,
    /// Problems that the original would have turned into out-of-range writes or fatal errors.
    pub warnings: Vec<String>,
}

impl Default for EtgTables {
    fn default() -> Self {
        let mut bullet_color = vec![DEFAULT_BULLET_COLOR; RACES.len() * GUN_SOUNDS.len()];
        for c in bullet_color.iter_mut().take(GUN_SOUNDS.len()) {
            *c = R1_BULLET_COLOR;
        }
        Self {
            death: vec![None; RACES.len() * CLASSES.len() * 5],
            death_derelict: vec![None; DERELICTS.len() * 5],
            gun: vec![None; RACES.len() * GUN_SOUNDS.len() * 5],
            resource: vec![None; RACES.len() * 2],
            tractor_beam: vec![None; RACES.len()],
            special: vec![None; NUM_SPECIAL_EFFECTS],
            damage: vec![None; CLASSES.len() * 3],
            hyperspace: None,
            bullet_color,
            big_death: vec![-10; RACES.len() * CLASSES.len()],
            big_death_derelict: vec![-10; DERELICTS.len()],
            number_effects: -1,
            history_scalar_min: 64,
            software_scalar_damage: 0.707,
            software_scalar_hit: 0.707,
            software_scalar_fire: 0.866,
            warnings: Vec::new(),
        }
    }
}

fn index_of(table: &[&str], name: &str) -> Option<usize> {
    table.iter().position(|t| t.eq_ignore_ascii_case(name))
}

impl EtgTables {
    /// Death event for race, class and explosion type.
    pub fn death_event(&self, race: usize, class: usize, kind: usize) -> Option<&[usize]> {
        self.death.get((race * CLASSES.len() + class) * 5 + kind)?.as_deref()
    }

    /// Gun event (`EGT_*`) for race and gun sound type.
    pub fn gun_event(&self, race: usize, gun_sound: usize, kind: usize) -> Option<&[usize]> {
        self.gun.get((race * GUN_SOUNDS.len() + gun_sound) * 5 + kind)?.as_deref()
    }

    /// Special-purpose effect slot.
    pub fn special_effect(&self, slot: usize) -> Option<&[usize]> {
        self.special.get(slot)?.as_deref()
    }

    /// Damage effect for a class and level (0 light, 1 heavy, 2 dying).
    pub fn damage_effect(&self, class: usize, level: usize) -> Option<&[usize]> {
        self.damage.get(class * 3 + level)?.as_deref()
    }

    /// Every static index referenced by any table.
    pub fn all_effects(&self) -> Vec<usize> {
        let mut v: Vec<usize> = Vec::new();
        for list in [&self.death, &self.death_derelict, &self.gun, &self.resource, &self.tractor_beam, &self.special, &self.damage] {
            for l in list.iter().flatten() {
                v.extend(l);
            }
        }
        if let Some(h) = &self.hyperspace {
            v.extend(h);
        }
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// `parseLine`: split a script line into keyword and value; `None` for blank, comment and
/// `[section]` lines.
fn parse_line(raw: &str) -> Option<(String, String)> {
    if raw.is_empty() || raw.starts_with('[') || raw.starts_with(']') {
        return None;
    }
    let mut line = raw.trim_end_matches([' ', '\t']).to_string();
    if let Some(p) = line.find(';') {
        line.truncate(p);
    }
    if let Some(p) = line.find("//") {
        line.truncate(p);
    }
    let name_start = line.find(|c| c != ' ' && c != '\t')?;
    let rest = &line[name_start..];
    match rest.find([' ', '\t', '=']) {
        None => Some((rest.to_string(), String::new())),
        Some(sep) => {
            let name = rest[..sep].to_string();
            let value = rest[sep + 1..].trim_start_matches([' ', '\t']).trim_end_matches([' ', '\t']).to_string();
            Some((name, value))
        }
    }
}

/// Splits a file into lines the way `fileLineRead` does (CR ends a line, a following LF is
/// swallowed, lines are cut at `n - 1` bytes).
fn file_lines(data: &[u8], n: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < data.len() {
        let mut line = Vec::new();
        while line.len() < n - 1 && pos < data.len() {
            let ch = data[pos];
            pos += 1;
            if ch == b'\r' {
                if pos < data.len() && data[pos] == b'\n' {
                    pos += 1;
                }
                break;
            }
            line.push(ch);
        }
        out.push(String::from_utf8_lossy(&line).into_owned());
    }
    out
}

struct Loader<'a> {
    lib: &'a mut Library,
    files: &'a dyn FileSource,
    tables: EtgTables,
    errors: Vec<CompileError>,
}

impl Loader<'_> {
    fn load(&mut self, name: &str) -> Option<usize> {
        let path = format!("ETG\\{name}");
        match self.lib.load_effect(self.files, &path) {
            Ok(i) => Some(i),
            Err(e) => {
                self.errors.push(e);
                None
            }
        }
    }

    fn lod_set(&mut self, slot: SlotRef, name: &str) {
        let entry: &mut Lod = match slot {
            SlotRef::Death(i) => &mut self.tables.death[i],
            SlotRef::DeathDerelict(i) => &mut self.tables.death_derelict[i],
            SlotRef::Gun(i) => &mut self.tables.gun[i],
            SlotRef::Resource(i) => &mut self.tables.resource[i],
            SlotRef::Tractor(i) => &mut self.tables.tractor_beam[i],
            SlotRef::Special(i) => &mut self.tables.special[i],
            SlotRef::Damage(i) => &mut self.tables.damage[i],
            SlotRef::Hyperspace => &mut self.tables.hyperspace,
        };
        if name.eq_ignore_ascii_case("NULL") {
            *entry = None;
            return;
        }
        let _ = entry;
        let idx = self.load(name);
        let entry: &mut Lod = match slot {
            SlotRef::Death(i) => &mut self.tables.death[i],
            SlotRef::DeathDerelict(i) => &mut self.tables.death_derelict[i],
            SlotRef::Gun(i) => &mut self.tables.gun[i],
            SlotRef::Resource(i) => &mut self.tables.resource[i],
            SlotRef::Tractor(i) => &mut self.tables.tractor_beam[i],
            SlotRef::Special(i) => &mut self.tables.special[i],
            SlotRef::Damage(i) => &mut self.tables.damage[i],
            SlotRef::Hyperspace => &mut self.tables.hyperspace,
        };
        if let Some(i) = idx {
            entry.get_or_insert_with(Vec::new).push(i);
        }
    }

    fn warn(&mut self, msg: String) {
        self.tables.warnings.push(msg);
    }

    fn explosion_type(&mut self, tok: Option<String>) -> Option<usize> {
        let t = tok.unwrap_or_default();
        let r = index_of(EXPLOSION_TYPES, &t);
        if r.is_none() {
            self.warn(format!("unrecognised explosion type '{t}'"));
        }
        r
    }

    fn race_sound(&mut self, tok: &mut StrTok) -> Option<usize> {
        let race = tok.next_tok(", \t").unwrap_or_default();
        let sound = tok.next_tok(", \t").unwrap_or_default();
        match (index_of(RACES, &race), index_of(GUN_SOUNDS, &sound)) {
            (Some(r), Some(s)) => Some(r * GUN_SOUNDS.len() + s),
            _ => {
                self.warn(format!("bad race/gun sound '{race}, {sound}'"));
                None
            }
        }
    }

    fn handle(&mut self, key: &str, value: &str) {
        let mut tok = StrTok::new(value);
        match key {
            "LoadEffect" => {
                self.load(value);
            }
            "SoftwareVersion" => {
                let hw = tok.next_tok(", \t").unwrap_or_default();
                let sw = tok.next_tok(", \t").unwrap_or_default();
                if let (Some(h), Some(s)) = (self.load(&hw), self.load(&sw)) {
                    self.lib.statics[h].software_version = Some(s);
                }
            }
            "setDeathEvent" => {
                let first = tok.next_tok(", \t").unwrap_or_default();
                let slot = if let Some(race) = index_of(RACES, &first) {
                    let class = tok.next_tok(", \t").unwrap_or_default();
                    let ex = tok.next_tok(", \t");
                    match (index_of(CLASSES, &class), self.explosion_type(ex)) {
                        (Some(c), Some(e)) => Some(SlotRef::Death((race * CLASSES.len() + c) * 5 + e)),
                        _ => None,
                    }
                } else {
                    let ex = tok.next_tok(", \t");
                    match (index_of(DERELICTS, &first), self.explosion_type(ex)) {
                        (Some(d), Some(e)) => Some(SlotRef::DeathDerelict(d * 5 + e)),
                        _ => None,
                    }
                };
                let name = tok.next_tok(", \t").unwrap_or_default();
                match slot {
                    Some(s) => self.lod_set(s, &name),
                    None => self.warn(format!("setDeathEvent {value}: bad slot")),
                }
            }
            "setFireEvent" | "setHitEvent" | "setBulletEffect" | "setBulletDestroyEvent" => {
                let Some(base) = self.race_sound(&mut tok) else { return };
                let name = tok.next_tok(", \t").unwrap_or_default();
                let kind = match key {
                    "setFireEvent" => GUN_FIRE,
                    "setHitEvent" => GUN_HIT,
                    "setBulletEffect" => GUN_BULLET,
                    _ => GUN_BULLET_DESTROYED,
                };
                if key == "setHitEvent" && name.eq_ignore_ascii_case("NULL") {
                    // The original clears the *fire* event here.
                    self.tables.gun[base * 5 + GUN_FIRE] = None;
                    return;
                }
                self.lod_set(SlotRef::Gun(base * 5 + kind), &name);
            }
            "setBulletColor" => {
                let Some(base) = self.race_sound(&mut tok) else { return };
                let r = tok.next_tok(", \t").and_then(|s| scan_int(&s)).unwrap_or(0);
                let g = tok.next_tok(", \t").and_then(|s| scan_int(&s)).unwrap_or(0);
                let b = tok.next_tok(", \t").and_then(|s| scan_int(&s)).unwrap_or(0);
                self.tables.bullet_color[base] = crate::cfmt::col_rgba(r, g, b, 0xff);
            }
            "bigBoomFactor" => {
                let first = tok.next_tok(", \t").unwrap_or_default();
                if let Some(race) = index_of(RACES, &first) {
                    let class = tok.next_tok(", \t").unwrap_or_default();
                    let v = tok.next_tok(", \t").and_then(|s| scan_int(&s));
                    if let (Some(c), Some(v)) = (index_of(CLASSES, &class), v) {
                        self.tables.big_death[race * CLASSES.len() + c] = v as i32;
                    }
                } else if let Some(d) = index_of(DERELICTS, &first) {
                    if let Some(v) = tok.next_tok(", \t").and_then(|s| scan_int(&s)) {
                        self.tables.big_death_derelict[d] = v as i32;
                    }
                }
            }
            "setAsteroidEffect" | "setGaseousEffect" => {
                let race = tok.next_tok(", \t").unwrap_or_default();
                let name = tok.next_tok(", \t").unwrap_or_default();
                if let Some(r) = index_of(RACES, &race) {
                    let k = usize::from(key == "setGaseousEffect");
                    self.lod_set(SlotRef::Resource(r * 2 + k), &name);
                }
            }
            "setTractorBeamEffect" => {
                let race = tok.next_tok(", \t").unwrap_or_default();
                let name = tok.next_tok(", \t").unwrap_or_default();
                if let Some(r) = index_of(RACES, &race) {
                    self.lod_set(SlotRef::Tractor(r), &name);
                }
            }
            "setSpecialPurposeEffect" => {
                let which = tok.next_tok(", \t").unwrap_or_default();
                let name = tok.next_tok(", \t").unwrap_or_default();
                match SPECIAL_EFFECTS.iter().find(|(n, _)| n.eq_ignore_ascii_case(&which)) {
                    Some(&(_, slot)) => self.lod_set(SlotRef::Special(slot), &name),
                    None => self.warn(format!("unknown special effect '{which}'")),
                }
            }
            "setDamageEvent" => {
                let class = tok.next_tok(", \t").unwrap_or_default();
                let level = tok.next_tok(", \t").unwrap_or_default();
                let name = tok.next_tok(", \t").unwrap_or_default();
                let lv = ["light", "heavy", "dying"].iter().position(|l| l.eq_ignore_ascii_case(&level));
                match (index_of(CLASSES, &class), lv) {
                    (Some(c), Some(l)) => self.lod_set(SlotRef::Damage(c * 3 + l), &name),
                    _ => self.warn(format!("setDamageEvent {value}: bad slot")),
                }
            }
            "setHyperspaceEvent" => {
                let name = tok.next_tok(", \t").unwrap_or_default();
                self.lod_set(SlotRef::Hyperspace, &name);
            }
            "NumberEffects" => self.tables.number_effects = scan_int(value).unwrap_or(0) as i32,
            "etgHistoryScalarMin" => self.tables.history_scalar_min = scan_int(value).unwrap_or(0) as i32,
            "etgSoftwareScalarDamage" => self.tables.software_scalar_damage = scan_float(value).unwrap_or(0.0),
            "etgSoftwareScalarHit" => self.tables.software_scalar_hit = scan_float(value).unwrap_or(0.0),
            "etgSoftwareScalarFire" => self.tables.software_scalar_fire = scan_float(value).unwrap_or(0.0),
            _ => {}
        }
    }
}

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum SlotRef {
    Death(usize),
    DeathDerelict(usize),
    Gun(usize),
    Resource(usize),
    Tractor(usize),
    Special(usize),
    Damage(usize),
    Hyperspace,
}

/// Result of [`load_scripts`].
pub struct ScriptLoad {
    pub tables: EtgTables,
    /// Effects that failed to compile (the original would have stopped with a fatal error).
    pub errors: Vec<CompileError>,
}

/// `etgStartup`: read the eight effect scripts, loading every effect they reference.
pub fn load_scripts(lib: &mut Library, files: &dyn FileSource) -> ScriptLoad {
    let mut loader = Loader { lib, files, tables: EtgTables::default(), errors: Vec::new() };
    for name in SCRIPT_FILES {
        let path = format!("ETG\\{name}");
        let Some(data) = files.read_file(&path) else {
            loader.warn(format!("missing script {path}"));
            continue;
        };
        for line in file_lines(&data, 650) {
            if let Some((k, v)) = parse_line(&line) {
                loader.handle(&k, &v);
            }
        }
    }
    loader.tables.number_effects = -1;
    ScriptLoad { tables: loader.tables, errors: loader.errors }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_line_parsing() {
        assert_eq!(parse_line("LoadEffect   foo.ebg  // hi"), Some(("LoadEffect".into(), "foo.ebg".into())));
        assert_eq!(parse_line("   // only comment"), None);
        assert_eq!(parse_line("[section]"), None);
        assert_eq!(
            parse_line("setDeathEvent\tRACE_BEAST,CLASS_Fighter,damage,DestDamageSmall.ebg"),
            Some(("setDeathEvent".into(), "RACE_BEAST,CLASS_Fighter,damage,DestDamageSmall.ebg".into()))
        );
    }

    #[test]
    fn default_bullet_colours() {
        let t = EtgTables::default();
        assert_eq!(t.bullet_color[0], R1_BULLET_COLOR);
        assert_eq!(t.bullet_color[GUN_SOUNDS.len()], DEFAULT_BULLET_COLOR);
    }
}
