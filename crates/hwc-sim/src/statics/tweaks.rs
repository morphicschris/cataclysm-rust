//! Global tunables that feed into the static data. The original keeps these as C globals with
//! compiled-in defaults and lets startup tweak scripts (`Tweak.script`, `AIShip.script`, ...)
//! overwrite them before any ship file is read. Only the globals the static loaders read are kept
//! here; each lists the keyword and file that set it. Keywords are matched exactly (case
//! sensitive), so a script line spelled differently from the binary's table is ignored, as in the
//! original (e.g. `aiship.script`'s `FIGHTER_BANK` and `BIGSHIP_*` lines have no effect).

use hwc_formats::script::ScriptFile;

use super::cscan::scan_f32;
use super::source::DataSource;

/// Converts degrees the way the angle setters do (single-precision degree-to-radian factor).
pub(crate) const DEG_TO_RAD: f32 = 0.017_453_292;

pub(crate) fn deg_to_rad(deg: f32) -> f32 {
    (f64::from(deg) * f64::from(DEG_TO_RAD)) as f32
}

/// Values of the globals the static loaders read.
#[derive(Debug, Clone, PartialEq)]
pub struct Tweaks {
    /// `COLLDAMAGE_DEFAULTMIN_FRACTION` (Tweak.script): default `minCollDamage` as a fraction of
    /// `maxhealth`.
    pub coll_damage_default_min_fraction: f32,
    /// `COLLDAMAGE_DEFAULTMAX_FRACTION` (Tweak.script).
    pub coll_damage_default_max_fraction: f32,
    /// `BLAST_RADIUS_MULTIPLE_DEFAULT` (Tweak.script): default `blastRadiusShockWave`.
    pub blast_radius_multiple_default: f32,
    /// `RETALIATE_ZONE` (Tweak.script): default `passiveRetaliateZone`.
    pub retaliate_zone: f32,
    /// `MINING_BASE_VERTICAL_DIST_HACK` (Tweak.script): added to the trMiningBase collision
    /// offsets.
    pub mining_base_vertical_dist_hack: f32,
    /// `capitalSideModifiers[TRANS_*]` (Tweak.script): default `collSideModifiers` of capital
    /// ships.
    pub capital_side_modifiers: [f32; 6],
    /// `GLOBAL_SHIP_SPEED_MODIFIER` (Tactics.script).
    pub global_ship_speed_modifier: f32,
    /// `GLOBAL_SHIP_HEALTH_MODIFIER` (CommandLayer.script).
    pub global_ship_health_modifier: f32,
    /// `STRIKECRAFT_PADDING_MODIFIER` (Formation.script).
    pub strikecraft_padding_modifier: f32,
    /// `AVOID_OBJ_PADDING_SCALE` (AIShip.script).
    pub avoid_obj_padding_scale: f32,
    /// `AVOID_OBJ_PADDING_SCALE_BIG` (AIShip.script); used when the avoidance sphere exceeds
    /// 1000 m.
    pub avoid_obj_padding_scale_big: f32,
    /// Bank sines per class (AIShip.script `*_BANK`, given in degrees). `FIGHTER_BANK` has no
    /// table entry and keeps its default.
    pub fighter_bank: f32,
    pub corvette_bank: f32,
    pub frigate_bank: f32,
    pub destroyer_bank: f32,
    pub massiveship_bank: f32,
    pub noship_bank: f32,
    /// Turn pitches in radians (AIShip.script `*_TURNPITCH`, degrees in the file).
    pub frigate_turnpitch: f32,
    pub destroyer_turnpitch: f32,
    pub massiveship_turnpitch: f32,
    pub noship_turnpitch: f32,
    /// Descend pitches in radians (AIShip.script `*_DESCENDPITCH`).
    pub frigate_descendpitch: f32,
    pub destroyer_descendpitch: f32,
    pub massiveship_descendpitch: f32,
    pub noship_descendpitch: f32,
    /// `ASTEROID_HARVEST_RANGE` (Resource.script): weapon range of gunless resource collectors.
    pub asteroid_harvest_range: f32,
    /// `DUSTCLOUD_RADIAL_SCALE` (Cloud.script).
    pub dustcloud_radial_scale: f32,
    /// `DUSTCLOUD_RADII[DustCloudN]` (Cloud.script).
    pub dustcloud_radii: [f32; 4],
    /// `NEB_RADIUS` (Nebulae.script): collision size of nebula tendrils.
    pub neb_radius: f32,
    /// `HYPERSPACEGATE_HEALTH`, `_WIDTH`, `_HEIGHT` (SinglePlayer.script).
    pub hyperspace_gate_health: f32,
    pub hyperspace_gate_width: f32,
    pub hyperspace_gate_height: f32,
    /// Default `rightOfWay` per ship type (table at 0x8a7a80), overwritten by the
    /// `rowDetails <type> <rightOfWay>, <avoidBy>` lines of AIShip.script.
    pub right_of_way: [i8; super::ShipType::COUNT],
}

impl Default for Tweaks {
    /// The compiled-in defaults (bit-exact with the executable's data section).
    fn default() -> Self {
        Self {
            coll_damage_default_min_fraction: f32::from_bits(0x3e4c_cccd),
            coll_damage_default_max_fraction: 1.0,
            blast_radius_multiple_default: 1.5,
            retaliate_zone: 1000.0,
            mining_base_vertical_dist_hack: 3400.0,
            capital_side_modifiers: [1.0; 6],
            global_ship_speed_modifier: 1.0,
            global_ship_health_modifier: 1.0,
            strikecraft_padding_modifier: f32::from_bits(0x3f99_999a),
            avoid_obj_padding_scale: f32::from_bits(0x3fcc_cccd),
            avoid_obj_padding_scale_big: 2.0,
            fighter_bank: f32::from_bits(0x3f7d_70a4),
            corvette_bank: f32::from_bits(0x3f34_fdf4),
            frigate_bank: 0.5,
            destroyer_bank: f32::from_bits(0x3eb3_3333),
            massiveship_bank: f32::from_bits(0x3e2e_147b),
            noship_bank: f32::from_bits(0x3c23_d70a),
            frigate_turnpitch: f32::from_bits(0x3f06_0a92),
            destroyer_turnpitch: f32::from_bits(0x3edf_66f3),
            massiveship_turnpitch: f32::from_bits(0x3eb2_b8c3),
            noship_turnpitch: f32::from_bits(0x3937_02d3),
            frigate_descendpitch: f32::from_bits(0x3eb2_b8c3),
            destroyer_descendpitch: f32::from_bits(0x3e86_0a92),
            massiveship_descendpitch: f32::from_bits(0x3e32_b8c3),
            noship_descendpitch: f32::from_bits(0x3937_02d3),
            asteroid_harvest_range: 400.0,
            dustcloud_radial_scale: 9.0,
            dustcloud_radii: [350.0, 500.0, 800.0, 1300.0],
            neb_radius: 100.0,
            hyperspace_gate_health: 20000.0,
            hyperspace_gate_width: 1000.0,
            hyperspace_gate_height: 1000.0,
            right_of_way: super::ship::RIGHT_OF_WAY,
        }
    }
}

/// How a keyword converts its value.
#[derive(Clone, Copy)]
enum Conv {
    Real,
    /// Degrees in the file, sine of the angle stored.
    SinAng,
    /// Degrees in the file, radians stored.
    Ang,
}

type Field = fn(&mut Tweaks) -> &mut f32;

/// (file, keyword, conversion, target), in the order the original reads the files.
const BINDINGS: &[(&str, &str, Conv, Field)] = &[
    ("AIShip.script", "AVOID_OBJ_PADDING_SCALE", Conv::Real, |t| &mut t.avoid_obj_padding_scale),
    ("AIShip.script", "AVOID_OBJ_PADDING_SCALE_BIG", Conv::Real, |t| &mut t.avoid_obj_padding_scale_big),
    ("AIShip.script", "CORVETTE_BANK", Conv::SinAng, |t| &mut t.corvette_bank),
    ("AIShip.script", "FRIGATE_BANK", Conv::SinAng, |t| &mut t.frigate_bank),
    ("AIShip.script", "DESTROYER_BANK", Conv::SinAng, |t| &mut t.destroyer_bank),
    ("AIShip.script", "MASSIVESHIP_BANK", Conv::SinAng, |t| &mut t.massiveship_bank),
    ("AIShip.script", "NOSHIP_BANK", Conv::SinAng, |t| &mut t.noship_bank),
    ("AIShip.script", "FRIGATE_TURNPITCH", Conv::Ang, |t| &mut t.frigate_turnpitch),
    ("AIShip.script", "DESTROYER_TURNPITCH", Conv::Ang, |t| &mut t.destroyer_turnpitch),
    ("AIShip.script", "MASSIVESHIP_TURNPITCH", Conv::Ang, |t| &mut t.massiveship_turnpitch),
    ("AIShip.script", "NOSHIP_TURNPITCH", Conv::Ang, |t| &mut t.noship_turnpitch),
    ("AIShip.script", "FRIGATE_DESCENDPITCH", Conv::Ang, |t| &mut t.frigate_descendpitch),
    ("AIShip.script", "DESTROYER_DESCENDPITCH", Conv::Ang, |t| &mut t.destroyer_descendpitch),
    ("AIShip.script", "MASSIVESHIP_DESCENDPITCH", Conv::Ang, |t| &mut t.massiveship_descendpitch),
    ("AIShip.script", "NOSHIP_DESCENDPITCH", Conv::Ang, |t| &mut t.noship_descendpitch),
    ("CommandLayer.script", "GLOBAL_SHIP_HEALTH_MODIFIER", Conv::Real, |t| &mut t.global_ship_health_modifier),
    ("Formation.script", "STRIKECRAFT_PADDING_MODIFIER", Conv::Real, |t| &mut t.strikecraft_padding_modifier),
    ("Tweak.script", "RETALIATE_ZONE", Conv::Real, |t| &mut t.retaliate_zone),
    ("Tweak.script", "BLAST_RADIUS_MULTIPLE_DEFAULT", Conv::Real, |t| &mut t.blast_radius_multiple_default),
    ("Tweak.script", "COLLDAMAGE_DEFAULTMIN_FRACTION", Conv::Real, |t| &mut t.coll_damage_default_min_fraction),
    ("Tweak.script", "COLLDAMAGE_DEFAULTMAX_FRACTION", Conv::Real, |t| &mut t.coll_damage_default_max_fraction),
    ("Tweak.script", "capitalSideModifiers[TRANS_UP]", Conv::Real, |t| &mut t.capital_side_modifiers[0]),
    ("Tweak.script", "capitalSideModifiers[TRANS_DOWN]", Conv::Real, |t| &mut t.capital_side_modifiers[1]),
    ("Tweak.script", "capitalSideModifiers[TRANS_RIGHT]", Conv::Real, |t| &mut t.capital_side_modifiers[2]),
    ("Tweak.script", "capitalSideModifiers[TRANS_LEFT]", Conv::Real, |t| &mut t.capital_side_modifiers[3]),
    ("Tweak.script", "capitalSideModifiers[TRANS_FORWARD]", Conv::Real, |t| &mut t.capital_side_modifiers[4]),
    ("Tweak.script", "capitalSideModifiers[TRANS_BACK]", Conv::Real, |t| &mut t.capital_side_modifiers[5]),
    ("Tweak.script", "MINING_BASE_VERTICAL_DIST_HACK", Conv::Real, |t| &mut t.mining_base_vertical_dist_hack),
    ("Resource.script", "ASTEROID_HARVEST_RANGE", Conv::Real, |t| &mut t.asteroid_harvest_range),
    ("Cloud.script", "DUSTCLOUD_RADIAL_SCALE", Conv::Real, |t| &mut t.dustcloud_radial_scale),
    ("Cloud.script", "DUSTCLOUD_RADII[DustCloud0]", Conv::Real, |t| &mut t.dustcloud_radii[0]),
    ("Cloud.script", "DUSTCLOUD_RADII[DustCloud1]", Conv::Real, |t| &mut t.dustcloud_radii[1]),
    ("Cloud.script", "DUSTCLOUD_RADII[DustCloud2]", Conv::Real, |t| &mut t.dustcloud_radii[2]),
    ("Cloud.script", "DUSTCLOUD_RADII[DustCloud3]", Conv::Real, |t| &mut t.dustcloud_radii[3]),
    ("SinglePlayer.script", "HYPERSPACEGATE_HEALTH", Conv::Real, |t| &mut t.hyperspace_gate_health),
    ("SinglePlayer.script", "HYPERSPACEGATE_WIDTH", Conv::Real, |t| &mut t.hyperspace_gate_width),
    ("SinglePlayer.script", "HYPERSPACEGATE_HEIGHT", Conv::Real, |t| &mut t.hyperspace_gate_height),
    ("Nebulae.script", "NEB_RADIUS", Conv::Real, |t| &mut t.neb_radius),
    ("Tactics.script", "GLOBAL_SHIP_SPEED_MODIFIER", Conv::Real, |t| &mut t.global_ship_speed_modifier),
];

impl Tweaks {
    /// Defaults overwritten by the startup tweak scripts found in `src`. Missing files are
    /// skipped, like the original.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = Tweaks::default();
        let mut files: Vec<&str> = BINDINGS.iter().map(|b| b.0).collect();
        files.dedup();
        for file in files {
            let Some(script) = src.read(file).and_then(|b| ScriptFile::parse_bytes(&b).ok()) else {
                continue;
            };
            t.apply(file, &script);
        }
        t
    }

    /// Applies the lines of one tweak file.
    pub fn apply(&mut self, file: &str, script: &ScriptFile) {
        for e in script.entries() {
            if file == "AIShip.script" && e.key == "rowDetails" {
                if let Some((t, row, _)) = parse_row_details(&e.raw) {
                    self.right_of_way[t.index()] = row as i8;
                }
                continue;
            }
            for &(_, _, conv, field) in BINDINGS.iter().filter(|b| b.0 == file && b.1 == e.key) {
                if let Some(v) = scan_f32(&e.raw) {
                    *field(self) = match conv {
                        Conv::Real => v,
                        Conv::Ang => deg_to_rad(v),
                        Conv::SinAng => (f64::from(v) * f64::from(DEG_TO_RAD)).sin() as f32,
                    };
                }
            }
        }
    }
}

/// `rowSetDetails` (0x4140a0): `"%s %d %f"` after the commas are blanked. Returns the ship
/// type, its right of way and the distance other ships keep out of its way.
pub fn parse_row_details(raw: &str) -> Option<(super::ShipType, i32, f32)> {
    use super::cscan::{decomma, scan, Scanned};
    let v = scan(&decomma(raw), "%s %d %f");
    let t = match v.first() {
        Some(Scanned::Str(n)) => super::ShipType::from_name(n)?,
        _ => return None,
    };
    let row = match v.get(1) {
        Some(Scanned::Int(i)) => *i,
        _ => 0,
    };
    let avoid = match v.get(2) {
        Some(Scanned::Float(f)) => *f,
        _ => 0.0,
    };
    Some((t, row, avoid))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_keywords_only() {
        let mut t = Tweaks::default();
        let s = ScriptFile::parse("FIGHTER_BANK 85.0\nCORVETTE_BANK 90\nBIGSHIP_BANK 30\nFRIGATE_TURNPITCH 180\n");
        t.apply("AIShip.script", &s);
        assert_eq!(t.fighter_bank, Tweaks::default().fighter_bank);
        assert!((t.corvette_bank - 1.0).abs() < 1e-6);
        assert_eq!(t.massiveship_bank, Tweaks::default().massiveship_bank);
        assert!((t.frigate_turnpitch - std::f32::consts::PI).abs() < 1e-5);
    }
}
