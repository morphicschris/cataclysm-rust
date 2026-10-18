//! Gun statics: the `GUN n { ... }` blocks of a ship file (`scriptSetGunStatics`) completed with
//! the muzzle points of the mesh extensions (`mexGetGunStaticInfo`).

use hwc_formats::mex::{ChunkData, Mex, UNSET_PATTERN};
use hwc_formats::script::ScriptFile;

use super::cscan::{scan_f32, scan_i32};
use super::enums::{GunSize, GunType, WeaponType};
use super::math::{normalize, Mat3, Vec3, IDENTITY};
use super::tweaks::{deg_to_rad, DEG_TO_RAD};

/// `GunStatic` (0x11c bytes each in Cataclysm). Offsets are within the gun record.
#[derive(Debug, Clone, PartialEq)]
pub struct GunStatic {
    /// +0x00 `Type`. `None` when the name was not recognised (the original stores -1).
    pub gun_type: Option<GunType>,
    /// +0x0c `StartUpgradeLevel`: weapon technology the gun starts with.
    pub start_upgrade_level: WeaponType,
    /// +0x10 `EndUpgradeLevel`: best technology it can be upgraded to.
    pub end_upgrade_level: WeaponType,
    /// +0x14 `GunSize`.
    pub gun_size: GunSize,
    /// +0x18 `DamageMult`: multiplier on the damage of the weapon technology in use.
    pub damage_mult: f32,
    /// +0x1c. Bit 0 (`GF_MultiLevelMatrix`) is set from the mesh hierarchy; not computed here.
    pub flags: u32,
    /// +0x20. Index of the gun within the ship.
    pub gun_index: usize,
    /// +0x24. Muzzle position (object coordinates) from the `GunN` mex chunk.
    pub position: Vec3,
    /// +0x30. Normalised muzzle direction; (1, 0, 0) when the mex gives a zero vector.
    pub gun_normal: Vec3,
    /// +0x3c `MinAngle` as a cosine, or from the mex cone when neither angle is given.
    pub cos_min_angle_from_norm: f32,
    /// +0x40 `MaxAngle` as a cosine.
    pub cos_max_angle_from_norm: f32,
    /// +0x44 / +0x68. Default barrel orientation, filled from the mesh hierarchy for
    /// `GUN_NewGimble` guns (not computed here; identity).
    pub default_gun_orientation: Mat3,
    pub default_gun_orientation_non_concat: Mat3,
    /// +0x8c..+0xa0 `minturnangle`, `maxturnangle`, `mindeclination`, `maxdeclination`,
    /// `maxanglespeed`, `maxdeclinationspeed` (radians; degrees in the file).
    pub min_turn_angle: f32,
    pub max_turn_angle: f32,
    pub min_declination: f32,
    pub max_declination: f32,
    pub max_angle_speed: f32,
    pub max_declination_speed: f32,
    /// +0xa4 `angletracking` (defaults to 1 when zero).
    pub angle_tracking: f32,
    /// +0xa8 `declinationtracking` (defaults to 1 when zero).
    pub declination_tracking: f32,
    /// +0xac `MaxMissiles`.
    pub max_missiles: i32,
    /// +0xb0 `TriggerHappy` as a cosine.
    pub trigger_happy: f32,
    /// +0xb4 / +0xc0 per-tactic damage and +0xcc / +0xd0 base damage. HW1 read these from
    /// `DamageLo`/`DamageHi`; Cataclysm only warns about those keywords, so they stay zero here
    /// and the damage comes from the weapon technology at run time.
    pub gun_damage_lo: [f32; 3],
    pub gun_damage_hi: [f32; 3],
    pub base_gun_damage_lo: f32,
    pub base_gun_damage_hi: f32,
    /// +0xd4 `BulletLength`.
    pub bullet_length: f32,
    /// +0xd8 `BulletLifeTime`; `BulletRange / BulletSpeed` when not given.
    pub bullet_life_time: f32,
    /// +0xdc `BulletSpeed`.
    pub bullet_speed: f32,
    /// +0xe0 `BulletRange`.
    pub bullet_range: f32,
    /// +0xe4 `BulletMass`.
    pub bullet_mass: f32,
    /// +0xe8 `FireTime`.
    pub fire_time: f32,
    /// +0xec `BurstFireTime`.
    pub burst_fire_time: f32,
    /// +0xf0 `BurstWaitTime`.
    pub burst_wait_time: f32,
    /// +0xf4 `BulletLifeTime2` (second fire mode); derived like the first when not given.
    pub bullet_life_time2: f32,
    /// +0xf8 `BulletSpeed2`.
    pub bullet_speed2: f32,
    /// +0xfc `BulletRange2`.
    pub bullet_range2: f32,
    /// +0x100 `FireTime2`.
    pub fire_time2: f32,
    /// +0x104 `BarrelLength`.
    pub barrel_length: f32,
    /// +0x108 `RecoilLength`.
    pub recoil_length: f32,
    /// +0x10c `OffsetX/Y/Z`: pivot offset for slaved barrels.
    pub offset: Vec3,
    /// +0x118 `SlaveDriver` (-1 when the gun drives itself).
    pub slave_driver: i32,
    /// Obsolete HW1 keywords present in the block (the original only prints a warning).
    pub obsolete_keywords: Vec<String>,
    /// True when the `GunN` mex chunk was found.
    pub mex_point_found: bool,
}

impl Default for GunStatic {
    fn default() -> Self {
        Self {
            gun_type: Some(GunType::Fixed),
            start_upgrade_level: WeaponType::Projectile,
            end_upgrade_level: WeaponType::Projectile,
            gun_size: GunSize::Small,
            damage_mult: 0.0,
            flags: 0,
            gun_index: 0,
            position: [0.0; 3],
            gun_normal: [0.0; 3],
            cos_min_angle_from_norm: 0.0,
            cos_max_angle_from_norm: 0.0,
            default_gun_orientation: IDENTITY,
            default_gun_orientation_non_concat: IDENTITY,
            min_turn_angle: 0.0,
            max_turn_angle: 0.0,
            min_declination: 0.0,
            max_declination: 0.0,
            max_angle_speed: 0.0,
            max_declination_speed: 0.0,
            angle_tracking: 0.0,
            declination_tracking: 0.0,
            max_missiles: 0,
            trigger_happy: 0.0,
            gun_damage_lo: [0.0; 3],
            gun_damage_hi: [0.0; 3],
            base_gun_damage_lo: 0.0,
            base_gun_damage_hi: 0.0,
            bullet_length: 0.0,
            bullet_life_time: 0.0,
            bullet_speed: 0.0,
            bullet_range: 0.0,
            bullet_mass: 0.0,
            fire_time: 0.0,
            burst_fire_time: 0.0,
            burst_wait_time: 0.0,
            bullet_life_time2: 0.0,
            bullet_speed2: 0.0,
            bullet_range2: 0.0,
            fire_time2: 0.0,
            barrel_length: 0.0,
            recoil_length: 0.0,
            offset: [0.0; 3],
            slave_driver: -1,
            obsolete_keywords: Vec::new(),
            mex_point_found: false,
        }
    }
}

/// `scriptSetCosAngCB`: degrees in the file, cosine stored.
fn cos_deg(raw: &str) -> Option<f32> {
    scan_f32(raw).map(|d| (f64::from(d) * f64::from(DEG_TO_RAD)).cos() as f32)
}

impl GunStatic {
    /// Applies one keyword of a gun block. Returns false for keywords the table does not bind.
    fn apply(&mut self, key: &str, raw: &str, warnings: &mut Vec<String>) -> bool {
        let f = |target: &mut f32| {
            if let Some(v) = scan_f32(raw) {
                *target = v;
            }
        };
        let ang = |target: &mut f32| {
            if let Some(v) = scan_f32(raw) {
                *target = deg_to_rad(v);
            }
        };
        match key {
            "SoundType" | "BulletType" | "DamageLo" | "DamageHi" => {
                self.obsolete_keywords.push(key.to_owned());
            }
            "StartUpgradeLevel" | "EndUpgradeLevel" => match WeaponType::from_name(raw) {
                Some(w) if key == "StartUpgradeLevel" => self.start_upgrade_level = w,
                Some(w) => self.end_upgrade_level = w,
                None => warnings.push(format!("unrecognised weapon type {raw:?}")),
            },
            "GunSize" => match GunSize::from_name(raw) {
                Some(s) => self.gun_size = s,
                None => warnings.push(format!("unrecognised gun size {raw:?}")),
            },
            "DamageMult" => f(&mut self.damage_mult),
            "Type" => self.gun_type = GunType::from_script_name(raw),
            "MaxMissiles" => {
                if let Some(v) = scan_i32(raw) {
                    self.max_missiles = v;
                }
            }
            "BulletLifeTime" => f(&mut self.bullet_life_time),
            "BulletLength" => f(&mut self.bullet_length),
            "BulletRange" => f(&mut self.bullet_range),
            "BulletSpeed" => f(&mut self.bullet_speed),
            "BulletMass" => f(&mut self.bullet_mass),
            "FireTime" => f(&mut self.fire_time),
            "BurstFireTime" => f(&mut self.burst_fire_time),
            "BurstWaitTime" => f(&mut self.burst_wait_time),
            "MinAngle" => {
                if let Some(c) = cos_deg(raw) {
                    self.cos_min_angle_from_norm = c;
                }
            }
            "MaxAngle" => {
                if let Some(c) = cos_deg(raw) {
                    self.cos_max_angle_from_norm = c;
                }
            }
            "TriggerHappy" => {
                if let Some(c) = cos_deg(raw) {
                    self.trigger_happy = c;
                }
            }
            "BulletLifeTime2" => f(&mut self.bullet_life_time2),
            "BulletRange2" => f(&mut self.bullet_range2),
            "BulletSpeed2" => f(&mut self.bullet_speed2),
            "FireTime2" => f(&mut self.fire_time2),
            "minturnangle" => ang(&mut self.min_turn_angle),
            "maxturnangle" => ang(&mut self.max_turn_angle),
            "mindeclination" => ang(&mut self.min_declination),
            "maxdeclination" => ang(&mut self.max_declination),
            "maxanglespeed" => ang(&mut self.max_angle_speed),
            "maxdeclinationspeed" => ang(&mut self.max_declination_speed),
            "angletracking" => f(&mut self.angle_tracking),
            "declinationtracking" => f(&mut self.declination_tracking),
            "BarrelLength" => f(&mut self.barrel_length),
            "RecoilLength" => f(&mut self.recoil_length),
            "OffsetX" => f(&mut self.offset[0]),
            "OffsetY" => f(&mut self.offset[1]),
            "OffsetZ" => f(&mut self.offset[2]),
            "SlaveDriver" => {
                if let Some(v) = scan_i32(raw) {
                    self.slave_driver = v;
                }
            }
            _ => return false,
        }
        true
    }
}

/// Result of scanning a ship file for gun blocks.
#[derive(Debug, Clone, Default)]
pub(crate) struct GunScan {
    /// `None` when the file has no positive `NUMBER_OF_GUNS`.
    pub guns: Option<Vec<GunStatic>>,
    /// Keywords inside gun blocks that the gun table does not bind.
    pub unbound: Vec<String>,
    pub warnings: Vec<String>,
}

/// The original's gun reader: wait for `NUMBER_OF_GUNS` (a count below 1 ends the scan), then for
/// each `GUN n` read a block opened by a line starting with `{` and closed by one starting with
/// `}`. Guns are stored at the index given after `GUN`, not in file order.
pub(crate) fn scan_guns(script: &ScriptFile) -> GunScan {
    enum State {
        WantCount,
        WantGun,
        WantOpen,
        InBlock,
    }
    let mut out = GunScan::default();
    let mut state = State::WantCount;
    let mut guns: Vec<GunStatic> = Vec::new();
    let mut current: Option<usize> = None;
    for e in script.entries() {
        let key = e.key.as_str();
        match state {
            State::WantCount => {
                if key == "NUMBER_OF_GUNS" {
                    let n = scan_i32(&e.raw).unwrap_or(0);
                    if n < 1 {
                        return out;
                    }
                    guns = (0..n as usize).map(|_| GunStatic::default()).collect();
                    state = State::WantGun;
                }
            }
            State::WantGun => {
                if key == "GUN" {
                    let idx = scan_i32(&e.raw).unwrap_or(-1);
                    current = usize::try_from(idx).ok().filter(|&i| i < guns.len());
                    if current.is_none() {
                        out.warnings.push(format!("GUN index {idx} outside NUMBER_OF_GUNS {}", guns.len()));
                    }
                    state = State::WantOpen;
                }
            }
            State::WantOpen => {
                if key.starts_with('{') {
                    state = State::InBlock;
                }
            }
            State::InBlock => {
                if key.starts_with('}') {
                    if let Some(g) = current.and_then(|i| guns.get_mut(i)) {
                        g.gun_damage_lo = [g.base_gun_damage_lo; 3];
                        g.gun_damage_hi = [g.base_gun_damage_hi; 3];
                    }
                    current = None;
                    state = State::WantGun;
                } else if let Some(g) = current.and_then(|i| guns.get_mut(i)) {
                    if !g.apply(key, &e.raw, &mut out.warnings) {
                        out.unbound.push(key.to_owned());
                    }
                }
            }
        }
    }
    if !guns.is_empty() {
        for (i, g) in guns.iter_mut().enumerate() {
            g.gun_index = i;
        }
        out.guns = Some(guns);
    }
    out
}

/// Fills in the bullet lifetimes the file leaves out and returns the longest and shortest bullet
/// range over all guns, seeded with gun 0's.
pub(crate) fn finish_ranges(guns: &mut [GunStatic]) -> (f32, f32) {
    let first = guns.first().map_or(0.0, |g| g.bullet_range);
    let (mut max, mut min) = (first, first);
    for g in guns.iter_mut() {
        if g.angle_tracking == 0.0 {
            g.angle_tracking = 1.0;
        }
        if g.declination_tracking == 0.0 {
            g.declination_tracking = 1.0;
        }
        if g.bullet_life_time == 0.0 {
            g.bullet_life_time = g.bullet_range / g.bullet_speed;
        }
        if g.bullet_life_time2 == 0.0 {
            let range = if g.bullet_range2 > 0.0 { g.bullet_range2 } else { g.bullet_range };
            let speed = if g.bullet_speed2 > 0.0 { g.bullet_speed2 } else { g.bullet_speed };
            g.bullet_life_time2 = range / speed;
        }
        if max < g.bullet_range {
            max = g.bullet_range;
        }
        if g.bullet_range < min {
            min = g.bullet_range;
        }
    }
    (max, min)
}

/// Angle field of a mex point, using the raw fill pattern when the exporter left it unset (the
/// original reads whatever bits are there).
fn raw_angle(a: Option<f32>) -> f32 {
    a.unwrap_or(f32::from_bits(UNSET_PATTERN))
}

/// `mexGetGunStaticInfo`: muzzle position and direction from the `Gun`/`GunN` chunks; when the
/// file gave neither `MinAngle` nor `MaxAngle`, the firing cone comes from the chunk's angles.
pub(crate) fn apply_mex(guns: &mut [GunStatic], mex: &Mex) {
    for (i, g) in guns.iter_mut().enumerate() {
        let name = format!("Gun{i}");
        let point = mex.chunks.iter().find_map(|c| match &c.data {
            ChunkData::Gun(p) if c.tag.eq_ignore_ascii_case("Gun") && c.name.eq_ignore_ascii_case(&name) => Some(*p),
            _ => None,
        });
        let Some(p) = point else { continue };
        g.mex_point_found = true;
        g.position = p.position;
        g.gun_normal = p.normal;
        if g.gun_normal == [0.0; 3] {
            g.gun_normal[0] = 1.0;
        } else {
            normalize(&mut g.gun_normal);
        }
        if g.cos_min_angle_from_norm == 0.0 && g.cos_max_angle_from_norm == 0.0 {
            let cone = f64::from(raw_angle(p.cone_angle));
            let edge = f64::from(raw_angle(p.edge_angle));
            let k = f64::from(DEG_TO_RAD);
            g.cos_min_angle_from_norm = (edge * k).cos() as f32;
            g.cos_max_angle_from_norm = ((cone + edge) * k).cos() as f32;
        }
    }
}
