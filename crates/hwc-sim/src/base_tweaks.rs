//! Run-time tunables of the base-building code: `Resource.script` (harvesting, the resource
//! trickle, regrowth) and the construction, research, retire and salvage entries of
//! `Tweak.script`, with the compiled defaults (keyword tables at 0x8854dc and 0x8a2330).
//!
//! As in the original, keywords match exactly. The binary's table names the eleventh salvage
//! damage limit `twSalvageDamageLimit[CLASS_MonsterShip]` and stores it in the corvette slot;
//! the shipped `tweak.script` spells it `[CLASS_Composite]`, which therefore has no effect, and
//! the monster-ship slot keeps its 0.

use hwc_formats::script::ScriptFile;

use crate::statics::cscan::{scan_f32, scan_i32};
use crate::statics::DataSource;

const FILE_RESOURCE: &str = "Resource.script";
const FILE_TWEAK: &str = "Tweak.script";

/// The base-building tunables.
#[derive(Debug, Clone, PartialEq)]
pub struct BaseTweaks {
    // --- Resource.script
    /// `COLLECTRESOURCE_DIST` (0x885440).
    pub collect_resource_dist: f32,
    /// `MAX_RESOURCE_VELOCITY_TO_CHASE` (0x885444).
    pub max_resource_velocity_to_chase: f32,
    /// `ASTEROID_HARVEST_RANGE` (0x885448).
    pub asteroid_harvest_range: f32,
    /// `ASTEROID_MAXHARVEST_RANGE` (0x88544c).
    pub asteroid_max_harvest_range: f32,
    /// `CIRCLE_HARVEST_ASTEROIDS_BIGGEREQUAL_THAN` (0x885450).
    pub circle_harvest_asteroids_biggerequal_than: i32,
    /// `CIRCLE_RIGHT_VELOCITY` (0x885454) and `CIRCLE_RIGHT_THRUST` (0x885458).
    pub circle_right_velocity: f32,
    pub circle_right_thrust: f32,
    /// `ASTEROID_LO_ROTATE` / `ASTEROID_HI_ROTATE` per asteroid type and `ASTEROID_ROTATE_PROB`.
    pub asteroid_lo_rotate: [f32; 5],
    pub asteroid_hi_rotate: [f32; 5],
    pub asteroid_rotate_prob: i32,
    /// `COLLECTRESOURCE_ZONE` (0x88548c).
    pub collect_resource_zone: f32,
    /// `REGROW_RESOURCES_CHECK_RATE` / `_FRAME` (0x885490 / 0x885494).
    pub regrow_check_rate: u32,
    pub regrow_check_frame: u32,
    /// `REGROW_RATE_MODIFIER[n]`.
    pub regrow_rate_modifier: [f32; 8],
    /// `regenerateRUrate`, `regenerateRUsIfUnder`, `regenerateRUs` (0x8854b8..): the
    /// multiplayer resource trickle.
    pub regenerate_ru_rate: u32,
    pub regenerate_rus_if_under: i32,
    pub regenerate_rus: i32,
    /// `CHECK_HARV_OUTOFWORLD_RESOURCE_RATE` (0x8854c4).
    pub check_harv_out_of_world_rate: u32,

    // --- Tweak.script
    /// `cmFloatRefundRatio` (0x74a8b4).
    pub cm_float_refund_ratio: f32,
    /// `REFRESH_RESEARCH_RATE` / `_FRAME` (0x8a22e4 / 0x8a22e8).
    pub refresh_research_rate: u32,
    pub refresh_research_frame: u32,
    /// `TW_RETIRE_MONEYBACK_RATIO` (0xa47328).
    pub retire_moneyback_ratio: f32,
    /// `SALVAGE_MIN_RETROFIT_TIME` / `SALVAGE_MAX_RETROFIT_TIME`.
    pub salvage_min_retrofit_time: f32,
    pub salvage_max_retrofit_time: f32,
    /// `twSalTransSpeed` / `twSalTransMaxTime`.
    pub sal_trans_speed: f32,
    pub sal_trans_max_time: f32,
    /// `twRetireSalvageDistanceBelowDepot` and its tolerance.
    pub retire_salvage_distance_below_depot: f32,
    pub retire_salvage_distance_below_depot_tolerance: f32,
    /// `twSalvageDamageLimit[class]` (0xa47f80).
    pub salvage_damage_limit: [f32; 12],
    /// `twCrystalProcessingTime`.
    pub crystal_processing_time: f32,
    /// `twNotifySalvagersNeededRate` / `twNotifySalvageTargetTooStrongRate`.
    pub notify_salvagers_needed_rate: f32,
    pub notify_salvage_target_too_strong_rate: f32,
    /// `twRepairRate[class]` (0xa47f40): health per second workers and processor beams
    /// restore (0: the class cannot be repaired). The binary has no `CLASS_Composite` entry.
    pub repair_rate: [f32; 12],
    /// `twProcessorAutoRepairDistance`, `...ConeAngle` (degrees), `...UpdateRate`,
    /// `...UpdateDelay`, `...BeamWidth` (0x8a2310..0x8a2320).
    pub processor_repair_distance: f32,
    pub processor_repair_cone_angle: f32,
    pub processor_repair_update_rate: f32,
    pub processor_repair_update_delay: f32,
    pub processor_repair_beam_width: f32,

    // --- RACE_BEAST/beastweapon.script (0xaf7484..0xaf7498)
    /// `beastAttackCascadePoints`, `beastAttackCascadeDelay`, `beastAttackCascadeDistance`,
    /// `beastAttackTimeSpentStunned`, `beastAttackTimeToCompleteAttacks`, `stunnedSpinRate`.
    pub beast_cascade_points: f32,
    pub beast_cascade_delay: f32,
    pub beast_cascade_distance: f32,
    pub beast_time_spent_stunned: f32,
    pub beast_time_to_complete_attacks: f32,
    pub beast_stunned_spin_rate: f32,
}

impl Default for BaseTweaks {
    fn default() -> Self {
        Self {
            collect_resource_dist: 400.0,
            max_resource_velocity_to_chase: 50.0,
            asteroid_harvest_range: 400.0,
            asteroid_max_harvest_range: 600.0,
            circle_harvest_asteroids_biggerequal_than: 2,
            circle_right_velocity: 20.0,
            circle_right_thrust: 0.1,
            asteroid_lo_rotate: [-1.0; 5],
            asteroid_hi_rotate: [1.0; 5],
            asteroid_rotate_prob: 150,
            collect_resource_zone: 2000.0,
            regrow_check_rate: 255,
            regrow_check_frame: 86,
            regrow_rate_modifier: [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0],
            regenerate_ru_rate: 255,
            regenerate_rus_if_under: 400,
            regenerate_rus: 25,
            check_harv_out_of_world_rate: 127,
            cm_float_refund_ratio: 1.0,
            refresh_research_rate: 15,
            refresh_research_frame: 13,
            // Uninitialised memory in the original until Tweak.script sets them.
            retire_moneyback_ratio: 0.0,
            salvage_min_retrofit_time: 0.0,
            salvage_max_retrofit_time: 0.0,
            sal_trans_speed: 5.0,
            sal_trans_max_time: 9_999_999.0,
            retire_salvage_distance_below_depot: 0.0,
            retire_salvage_distance_below_depot_tolerance: 0.0,
            salvage_damage_limit: [0.0; 12],
            crystal_processing_time: 10.0,
            notify_salvagers_needed_rate: 15.0,
            notify_salvage_target_too_strong_rate: 5.0,
            repair_rate: [0.0; 12],
            processor_repair_distance: 2000.0,
            processor_repair_cone_angle: 90.0,
            processor_repair_update_rate: 5.0,
            processor_repair_update_delay: 0.5,
            processor_repair_beam_width: 250.0,
            beast_cascade_points: 0.0,
            beast_cascade_delay: 0.0,
            beast_cascade_distance: 0.0,
            beast_time_spent_stunned: 0.0,
            beast_time_to_complete_attacks: 0.0,
            beast_stunned_spin_rate: 0.0,
        }
    }
}

enum Slot {
    F(fn(&mut BaseTweaks) -> &mut f32),
    I(fn(&mut BaseTweaks) -> &mut i32),
    U(fn(&mut BaseTweaks) -> &mut u32),
}
use Slot::*;

macro_rules! lims {
    ($($name:literal => $i:literal),* $(,)?) => {
        &[$(($name, F(|t| &mut t.salvage_damage_limit[$i]))),*]
    };
}

const RESOURCE_KEYS: &[(&str, Slot)] = &[
    ("COLLECTRESOURCE_DIST", F(|t| &mut t.collect_resource_dist)),
    ("MAX_RESOURCE_VELOCITY_TO_CHASE", F(|t| &mut t.max_resource_velocity_to_chase)),
    ("ASTEROID_HARVEST_RANGE", F(|t| &mut t.asteroid_harvest_range)),
    ("ASTEROID_MAXHARVEST_RANGE", F(|t| &mut t.asteroid_max_harvest_range)),
    ("CIRCLE_HARVEST_ASTEROIDS_BIGGEREQUAL_THAN", I(|t| &mut t.circle_harvest_asteroids_biggerequal_than)),
    ("CIRCLE_RIGHT_VELOCITY", F(|t| &mut t.circle_right_velocity)),
    ("CIRCLE_RIGHT_THRUST", F(|t| &mut t.circle_right_thrust)),
    ("ASTEROID_LO_ROTATE[Asteroid0]", F(|t| &mut t.asteroid_lo_rotate[0])),
    ("ASTEROID_HI_ROTATE[Asteroid0]", F(|t| &mut t.asteroid_hi_rotate[0])),
    ("ASTEROID_LO_ROTATE[Asteroid1]", F(|t| &mut t.asteroid_lo_rotate[1])),
    ("ASTEROID_HI_ROTATE[Asteroid1]", F(|t| &mut t.asteroid_hi_rotate[1])),
    ("ASTEROID_LO_ROTATE[Asteroid2]", F(|t| &mut t.asteroid_lo_rotate[2])),
    ("ASTEROID_HI_ROTATE[Asteroid2]", F(|t| &mut t.asteroid_hi_rotate[2])),
    ("ASTEROID_LO_ROTATE[Asteroid3]", F(|t| &mut t.asteroid_lo_rotate[3])),
    ("ASTEROID_HI_ROTATE[Asteroid3]", F(|t| &mut t.asteroid_hi_rotate[3])),
    ("ASTEROID_LO_ROTATE[Asteroid4]", F(|t| &mut t.asteroid_lo_rotate[4])),
    ("ASTEROID_HI_ROTATE[Asteroid4]", F(|t| &mut t.asteroid_hi_rotate[4])),
    ("ASTEROID_ROTATE_PROB", I(|t| &mut t.asteroid_rotate_prob)),
    ("COLLECTRESOURCE_ZONE", F(|t| &mut t.collect_resource_zone)),
    ("REGROW_RESOURCES_CHECK_RATE", U(|t| &mut t.regrow_check_rate)),
    ("REGROW_RESOURCES_CHECK_FRAME", U(|t| &mut t.regrow_check_frame)),
    ("REGROW_RATE_MODIFIER[0]", F(|t| &mut t.regrow_rate_modifier[0])),
    ("REGROW_RATE_MODIFIER[1]", F(|t| &mut t.regrow_rate_modifier[1])),
    ("REGROW_RATE_MODIFIER[2]", F(|t| &mut t.regrow_rate_modifier[2])),
    ("REGROW_RATE_MODIFIER[3]", F(|t| &mut t.regrow_rate_modifier[3])),
    ("REGROW_RATE_MODIFIER[4]", F(|t| &mut t.regrow_rate_modifier[4])),
    ("REGROW_RATE_MODIFIER[5]", F(|t| &mut t.regrow_rate_modifier[5])),
    ("REGROW_RATE_MODIFIER[6]", F(|t| &mut t.regrow_rate_modifier[6])),
    ("REGROW_RATE_MODIFIER[7]", F(|t| &mut t.regrow_rate_modifier[7])),
    ("regenerateRUrate", U(|t| &mut t.regenerate_ru_rate)),
    ("regenerateRUsIfUnder", I(|t| &mut t.regenerate_rus_if_under)),
    ("regenerateRUs", I(|t| &mut t.regenerate_rus)),
    ("CHECK_HARV_OUTOFWORLD_RESOURCE_RATE", U(|t| &mut t.check_harv_out_of_world_rate)),
];

const TWEAK_KEYS: &[(&str, Slot)] = &[
    ("cmFloatRefundRatio", F(|t| &mut t.cm_float_refund_ratio)),
    ("REFRESH_RESEARCH_RATE", U(|t| &mut t.refresh_research_rate)),
    ("REFRESH_RESEARCH_FRAME", U(|t| &mut t.refresh_research_frame)),
    ("TW_RETIRE_MONEYBACK_RATIO", F(|t| &mut t.retire_moneyback_ratio)),
    ("SALVAGE_MIN_RETROFIT_TIME", F(|t| &mut t.salvage_min_retrofit_time)),
    ("SALVAGE_MAX_RETROFIT_TIME", F(|t| &mut t.salvage_max_retrofit_time)),
    ("twSalTransSpeed", F(|t| &mut t.sal_trans_speed)),
    ("twSalTransMaxTime", F(|t| &mut t.sal_trans_max_time)),
    ("twRetireSalvageDistanceBelowDepot", F(|t| &mut t.retire_salvage_distance_below_depot)),
    ("twRetireSalvageDistanceBelowDepotTolerance", F(|t| &mut t.retire_salvage_distance_below_depot_tolerance)),
    ("twCrystalProcessingTime", F(|t| &mut t.crystal_processing_time)),
    ("twNotifySalvagersNeededRate", F(|t| &mut t.notify_salvagers_needed_rate)),
    ("twNotifySalvageTargetTooStrongRate", F(|t| &mut t.notify_salvage_target_too_strong_rate)),
    ("twRepairRate[CLASS_Mothership]", F(|t| &mut t.repair_rate[0])),
    ("twRepairRate[CLASS_HeavyCruiser]", F(|t| &mut t.repair_rate[1])),
    ("twRepairRate[CLASS_Carrier]", F(|t| &mut t.repair_rate[2])),
    ("twRepairRate[CLASS_Destroyer]", F(|t| &mut t.repair_rate[3])),
    ("twRepairRate[CLASS_Frigate]", F(|t| &mut t.repair_rate[4])),
    ("twRepairRate[CLASS_Corvette]", F(|t| &mut t.repair_rate[5])),
    ("twRepairRate[CLASS_Fighter]", F(|t| &mut t.repair_rate[6])),
    ("twRepairRate[CLASS_Resource]", F(|t| &mut t.repair_rate[7])),
    ("twRepairRate[CLASS_NonCombat]", F(|t| &mut t.repair_rate[8])),
    ("twRepairRate[CLASS_MonsterShip]", F(|t| &mut t.repair_rate[9])),
    ("twRepairRate[CLASS_Component]", F(|t| &mut t.repair_rate[10])),
    ("twRepairRate[CLASS_Micro]", F(|t| &mut t.repair_rate[11])),
    ("twProcessorAutoRepairDistance", F(|t| &mut t.processor_repair_distance)),
    ("twProcessorAutoRepairConeAngle", F(|t| &mut t.processor_repair_cone_angle)),
    ("twProcessorAutoRepairUpdateRate", F(|t| &mut t.processor_repair_update_rate)),
    ("twProcessorAutoRepairUpdateDelay", F(|t| &mut t.processor_repair_update_delay)),
    ("twProcessorAutoRepairBeamWidth", F(|t| &mut t.processor_repair_beam_width)),
];

const FILE_BEAST_WEAPON: &str = "RACE_BEAST/beastweapon.script";

const BEAST_WEAPON_KEYS: &[(&str, Slot)] = &[
    ("beastAttackCascadePoints", F(|t| &mut t.beast_cascade_points)),
    ("beastAttackCascadeDelay", F(|t| &mut t.beast_cascade_delay)),
    ("beastAttackCascadeDistance", F(|t| &mut t.beast_cascade_distance)),
    ("beastAttackTimeSpentStunned", F(|t| &mut t.beast_time_spent_stunned)),
    ("beastAttackTimeToCompleteAttacks", F(|t| &mut t.beast_time_to_complete_attacks)),
    ("stunnedSpinRate", F(|t| &mut t.beast_stunned_spin_rate)),
];

const LIMIT_KEYS: &[(&str, Slot)] = lims! {
    "twSalvageDamageLimit[CLASS_Mothership]" => 0,
    "twSalvageDamageLimit[CLASS_HeavyCruiser]" => 1,
    "twSalvageDamageLimit[CLASS_Carrier]" => 2,
    "twSalvageDamageLimit[CLASS_Destroyer]" => 3,
    "twSalvageDamageLimit[CLASS_Frigate]" => 4,
    "twSalvageDamageLimit[CLASS_Corvette]" => 5,
    "twSalvageDamageLimit[CLASS_Fighter]" => 6,
    "twSalvageDamageLimit[CLASS_Resource]" => 7,
    "twSalvageDamageLimit[CLASS_NonCombat]" => 8,
    // The binary's table points this entry at the corvette slot.
    "twSalvageDamageLimit[CLASS_MonsterShip]" => 5,
    "twSalvageDamageLimit[CLASS_Component]" => 10,
    "twSalvageDamageLimit[CLASS_Micro]" => 11,
};

impl BaseTweaks {
    /// Compiled defaults overwritten by `Resource.script` and `Tweak.script` from `src`.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = BaseTweaks::default();
        for file in [FILE_RESOURCE, FILE_TWEAK, FILE_BEAST_WEAPON] {
            if let Some(script) = src.read(file).and_then(|b| ScriptFile::parse_bytes(&b).ok()) {
                t.apply(file, &script);
            }
        }
        t
    }

    /// Applies one script.
    pub fn apply(&mut self, file: &str, script: &ScriptFile) {
        let tables: &[&[(&str, Slot)]] = match file {
            FILE_RESOURCE => &[RESOURCE_KEYS],
            FILE_BEAST_WEAPON => &[BEAST_WEAPON_KEYS],
            _ => &[TWEAK_KEYS, LIMIT_KEYS],
        };
        for e in script.entries() {
            for table in tables {
                for (name, slot) in table.iter() {
                    if *name != e.key {
                        continue;
                    }
                    match slot {
                        F(f) => {
                            if let Some(v) = scan_f32(&e.raw) {
                                *f(self) = v;
                            }
                        }
                        I(f) => {
                            if let Some(v) = scan_i32(&e.raw) {
                                *f(self) = v;
                            }
                        }
                        U(f) => {
                            if let Some(v) = scan_i32(&e.raw) {
                                *f(self) = v as u32;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monster_ship_limit_lands_in_the_corvette_slot() {
        let mut t = BaseTweaks::default();
        let s = ScriptFile::parse(
            "twSalvageDamageLimit[CLASS_Corvette] 0.4\ntwSalvageDamageLimit[CLASS_MonsterShip] 0.9\ntwSalvageDamageLimit[CLASS_Composite] 0.7\n",
        );
        t.apply("Tweak.script", &s);
        assert_eq!(t.salvage_damage_limit[5], 0.9);
        assert_eq!(t.salvage_damage_limit[9], 0.0);
    }

    #[test]
    fn resource_keywords() {
        let mut t = BaseTweaks::default();
        let s = ScriptFile::parse("COLLECTRESOURCE_ZONE 4000.0\nregenerateRUs 25\nASTEROID_HI_ROTATE[Asteroid3] 0.2\n");
        t.apply("Resource.script", &s);
        assert_eq!(t.collect_resource_zone, 4000.0);
        assert_eq!(t.regenerate_rus, 25);
        assert_eq!(t.asteroid_hi_rotate[3], 0.2);
    }
}
