//! Run-time tunables of the combat code (`Tactics.script`, `CommandLayer.script`,
//! `FlightMan.script`, `Tweak.script` and the combat entries of `AIShip.script`), with the
//! defaults compiled into `cataclysm.exe`. Keyword tables: Tactics at 0x89ddc0, CommandLayer at
//! 0x749078, FlightMan at 0x874428, Tweak at 0x8a2330, AIShip at 0x743780.

use hwc_formats::script::ScriptFile;
use hwc_math::fpu::{ld, st};
use hwc_math::trig::fcos;

use crate::statics::cscan::{scan_f32, scan_i32};
use crate::statics::tweaks::parse_row_details;
use crate::statics::{DataSource, ShipType};

/// The single-precision degree-to-radian factor the angle setters multiply by.
const DEG_TO_RAD: f32 = 0.017_453_292;

/// Flight-manoeuvre tunables (FlightMan.script).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FlightManTweaks {
    pub flipturn_maxrotspeedflip: f32,
    pub flipturn_pauseafterfliptime: f32,
    pub flipturn_flipaccelmodifier: f32,
    pub flipturn_afterflipthrustmod: f32,
    pub flipturn_rollthrustmod: f32,
    pub flipturn_rollaccelmodifier: f32,
    pub flipturn_maxrollspeed: f32,
    pub immelman_thrustmod: f32,
    pub immelman_accelmodifierflip: f32,
    pub immelman_maxrotspeedflip: f32,
    pub immelman_rollaccelmodifier: f32,
    pub immelman_maxrollspeed: f32,
    pub immelman_rollthrustmod: f32,
    pub split_s_thrustmod: f32,
    pub split_s_accelmodifierflip: f32,
    pub split_s_maxrotspeedpitch: f32,
    pub split_s_accelmodifierroll: f32,
    pub split_s_maxrotspeedroll: f32,
    pub hardbank_thrustmod: f32,
    pub hardbank_accelmodifierflip: f32,
    pub hardbank_maxrotspeedpitch: f32,
    pub hardbank_accelmodifierroll: f32,
    pub hardbank_maxrotspeedroll: f32,
    pub softbank_thrustmod: f32,
    pub softbank_accelmodifierflip: f32,
    pub softbank_maxrotspeedpitch: f32,
    pub softbank_accelmodifierroll: f32,
    pub softbank_maxrotspeedroll: f32,
    pub barrel_roll_accelmodifierroll: f32,
    pub barrel_roll_maxrotspeedroll: f32,
    pub whipstrafe_escapedist_z: f32,
    pub whipstrafe_bulletrangeoptimum: f32,
    /// Squared.
    pub whipstrafe_maxangcomparefactor: f32,
    pub whipstrafe_flybyovershoot: f32,
    pub whipstrafe_minvelocity: f32,
    pub whipstrafe_reachdestinationrange: f32,
    /// Cosine.
    pub whipstrafe_facedownup_accuracy: f32,
    pub whipstrafe_whipthrustmod: f32,
    pub whipstrafe_whiptime: f32,
    pub rollaway_thrustmod: f32,
    pub rollaway_accelmodifierroll: f32,
    pub rollaway_maxrotspeedroll: f32,
    pub rollaway_thrustdowntime: f32,
    pub splitsevasive_accelmodifierpitch: f32,
    pub splitsevasive_maxrotspeedpitch: f32,
    pub splitsevasive_thrustmod: f32,
    pub splitsevasive_accelmodifierroll: f32,
    pub splitsevasive_maxrotspeedroll: f32,
    pub loyoyo_thrustmod: [f32; 5],
    pub loyoyo_accelmodifierpitch: f32,
    pub loyoyo_maxrotspeedpitch: f32,
    pub loyoyo_goup_time: f32,
    pub loyoyo_godown_time: f32,
    pub hiyoyo_thrustmod: [f32; 5],
    pub hiyoyo_accelmodifierpitch: f32,
    pub hiyoyo_maxrotspeedpitch: f32,
    pub hiyoyo_goup_time: f32,
    pub hiyoyo_godown_time: f32,
    pub sidestep_thrustmod: f32,
    pub sidestep_thrustmodturn: f32,
    pub sidestep_accelmodifieryaw: f32,
    pub sidestep_maxrotspeedyaw: f32,
    pub sidestep_accelmodifierpitch: f32,
    pub sidestep_maxrotspeedpitch: f32,
    pub sidestep_accelmodifierroll: f32,
    pub sidestep_maxrotspeedroll: f32,
    pub sidestep_pausetime: f32,
    pub sidestep_bootittime: f32,
    /// Radians.
    pub sidestep_rollrotate: f32,
    pub sidestep_pitchrotate: f32,
    pub slalom_minvelocity: f32,
    pub slalom_reachdestinationrange: f32,
    pub slalom_maxwaypointtime: f32,
    pub slalom_run_length: f32,
    pub slalom_randomiterations: u32,
    pub slalom_baseiterations: u32,
    /// Radians.
    pub sandwich_angle: f32,
    pub sandwich_maxtime: f32,
    pub sandwich_reachdestinationrange: f32,
    pub sandwich_minvelocity: f32,
    pub sandwich_pincher_scale: f32,
    pub token_evasivepure_checkrate: u32,
    pub token_evasivepure_checkframe: u32,
    pub token_evasivepure_prob: u32,
    pub evasivepure_prob: [u32; 3],
    pub split_prob: [u32; 3],
    pub sandwich_prob: [u32; 3],
    pub num_evasivepure_maneuvers: [u16; 3],
    // Compiled constants without a keyword.
    pub endover_accelmodifier: f32,
    pub endover_maxrotspeed: f32,
    pub celeb_flip_accelmodifierroll: f32,
    pub celeb_flip_maxrotspeedroll: f32,
    pub swarmer_barrelroll_accelmodifierroll: f32,
    pub swarmer_barrelroll_maxrotspeedroll: f32,
    pub roll180_accelmodifierroll: f32,
    pub roll180_maxrotspeedroll: f32,
}

impl Default for FlightManTweaks {
    fn default() -> Self {
        let b = f32::from_bits;
        Self {
            flipturn_maxrotspeedflip: 1.0,
            flipturn_pauseafterfliptime: 1.0,
            flipturn_flipaccelmodifier: 0.25,
            flipturn_afterflipthrustmod: 1.0,
            flipturn_rollthrustmod: 0.5,
            flipturn_rollaccelmodifier: 0.5,
            flipturn_maxrollspeed: 1.0,
            immelman_thrustmod: 1.0,
            immelman_accelmodifierflip: 0.25,
            immelman_maxrotspeedflip: 1.0,
            immelman_rollaccelmodifier: 0.25,
            immelman_maxrollspeed: 1.0,
            immelman_rollthrustmod: 0.5,
            split_s_thrustmod: 1.0,
            split_s_accelmodifierflip: 0.25,
            split_s_maxrotspeedpitch: 1.0,
            split_s_accelmodifierroll: 0.25,
            split_s_maxrotspeedroll: 1.0,
            hardbank_thrustmod: 1.0,
            hardbank_accelmodifierflip: 0.25,
            hardbank_maxrotspeedpitch: 1.0,
            hardbank_accelmodifierroll: 0.25,
            hardbank_maxrotspeedroll: 1.0,
            softbank_thrustmod: 1.0,
            softbank_accelmodifierflip: 0.25,
            softbank_maxrotspeedpitch: 1.0,
            softbank_accelmodifierroll: 0.25,
            softbank_maxrotspeedroll: 1.0,
            barrel_roll_accelmodifierroll: 0.25,
            barrel_roll_maxrotspeedroll: 1.0,
            whipstrafe_escapedist_z: 500.0,
            whipstrafe_bulletrangeoptimum: b(0x3e99_999a),
            whipstrafe_maxangcomparefactor: 0.25,
            whipstrafe_flybyovershoot: b(0x3fa6_6666),
            whipstrafe_minvelocity: -200.0,
            whipstrafe_reachdestinationrange: 110.0,
            whipstrafe_facedownup_accuracy: b(0x3f7a_e148),
            whipstrafe_whipthrustmod: 1.0,
            whipstrafe_whiptime: 3.0,
            rollaway_thrustmod: 1.0,
            rollaway_accelmodifierroll: 1.0,
            rollaway_maxrotspeedroll: 1.0,
            rollaway_thrustdowntime: 0.5,
            splitsevasive_accelmodifierpitch: 0.5,
            splitsevasive_maxrotspeedpitch: 1.0,
            splitsevasive_thrustmod: 1.0,
            splitsevasive_accelmodifierroll: 0.5,
            splitsevasive_maxrotspeedroll: 1.0,
            loyoyo_thrustmod: [0.5, 0.5, 0.5, 1.0, 0.5],
            loyoyo_accelmodifierpitch: 0.5,
            loyoyo_maxrotspeedpitch: 1.0,
            loyoyo_goup_time: 2.0,
            loyoyo_godown_time: 0.5,
            hiyoyo_thrustmod: [0.5, 0.5, 0.5, 1.0, 0.5],
            hiyoyo_accelmodifierpitch: 0.5,
            hiyoyo_maxrotspeedpitch: 1.0,
            hiyoyo_goup_time: 0.5,
            hiyoyo_godown_time: 2.0,
            sidestep_thrustmod: 1.0,
            sidestep_thrustmodturn: b(0x3e4c_cccd),
            sidestep_accelmodifieryaw: 0.5,
            sidestep_maxrotspeedyaw: 1.0,
            sidestep_accelmodifierpitch: 0.25,
            sidestep_maxrotspeedpitch: 0.5,
            sidestep_accelmodifierroll: 0.5,
            sidestep_maxrotspeedroll: 1.0,
            sidestep_pausetime: 0.5,
            sidestep_bootittime: 2.0,
            sidestep_rollrotate: b(0x3fdf_66f3),
            sidestep_pitchrotate: b(0x3fdf_66f3),
            slalom_minvelocity: -300.0,
            slalom_reachdestinationrange: 160.0,
            slalom_maxwaypointtime: 10.0,
            slalom_run_length: 1000.0,
            slalom_randomiterations: 3,
            slalom_baseiterations: 2,
            sandwich_angle: b(0x3f49_0fdb),
            sandwich_maxtime: 10.0,
            sandwich_reachdestinationrange: 160.0,
            sandwich_minvelocity: -300.0,
            sandwich_pincher_scale: 0.5,
            token_evasivepure_checkrate: 63,
            token_evasivepure_checkframe: 47,
            token_evasivepure_prob: 50,
            evasivepure_prob: [50; 3],
            split_prob: [255; 3],
            sandwich_prob: [255; 3],
            num_evasivepure_maneuvers: [2; 3],
            endover_accelmodifier: 0.25,
            endover_maxrotspeed: 8.0,
            celeb_flip_accelmodifierroll: 0.25,
            celeb_flip_maxrotspeedroll: 1.5,
            swarmer_barrelroll_accelmodifierroll: 0.25,
            swarmer_barrelroll_maxrotspeedroll: 2.0,
            roll180_accelmodifierroll: 0.25,
            roll180_maxrotspeedroll: 2.0,
        }
    }
}

/// Combat tunables.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CombatTweaks {
    // --- AIShip.script
    pub interceptor_break_tolerance: f32,
    pub interceptor_break_min_velocity: f32,
    pub interceptor_kill_min_velocity: f32,
    /// Cosine.
    pub row_going_mostly_updown_angle: f32,
    pub avoid_row_ship_by_scale: f32,
    pub avoid_row_ship_by_add: f32,
    pub avoid_row_r1mothership_by: f32,
    pub row_go_down_instead_of_up_ratio: f32,
    pub dont_use_velocity_pred_if_below: f32,
    /// `rowAvoidBy` (0x8a783c): distance a ship keeps out of the way of each type.
    #[serde(with = "hwc_math::serde_array")]
    pub row_avoid_by: [f32; ShipType::COUNT],

    // --- CommandLayer.script
    pub fighter_pairup_prob: u32,
    pub fighter_pair_check_rate: [u32; 3],
    pub fighter_single_check_rate: [u32; 3],
    /// Squared.
    pub fighter_max_threat_dist: [f32; 3],
    /// Squared.
    pub wingman_join_max_dist: f32,
    pub multipletargets_retarget_rate: u32,
    pub fighter_break_angle_min: i32,
    pub fighter_break_angle_max: i32,
    pub fighter_break_vertical_angle_min: i32,
    pub fighter_break_vertical_angle_max: i32,
    /// `PROTECT_GET_THIS_CLOSE_CAP` / `_STRIKE` (0x749030, 0x749034): how close guards stay.
    pub protect_get_this_close_cap: f32,
    pub protect_get_this_close_strike: f32,
    /// `PROTECT_STILL_VELOCITY` (0x749038): below this a protected ship counts as still.
    pub protect_still_velocity: f32,
    /// `RESCONTROLLER_FOLLOW_BEHIND` (0x749070).
    pub rescontroller_follow_behind: f32,

    // --- Tactics.script
    pub enemy_near_zone: f32,
    pub passive_retaliation_fire_time_modifier_lo: f32,
    pub passive_retaliation_fire_time_modifier_hi: f32,
    pub attacking_from_above_min_dist: f32,
    pub attacking_from_above_min_ratio: f32,
    pub tactics_guard_return_to_guarding_distance: f32,
    /// Read with the float setter although used as frame masks: the float's bit pattern is
    /// what the original compares (see [`Self::guard_check_rate_bits`]).
    pub tactics_guard_condition_check_rate: f32,
    pub tactics_guard_condition_check_frame: f32,
    pub burst_speed: f32,
    pub burst_radius: f32,
    pub hold_formation_during_battle: [u32; 3],
    /// Squared.
    pub moving_attack_turns_into_move_command_distance: f32,

    // --- Tweak.script
    pub blast_constant: f32,
    pub blast_damage_max_percent: f32,
    pub blast_constant_scuttle: f32,
    pub blast_radius_scuttle_bonus: f32,
    pub blast_damage_max_percent_scuttle: f32,
    /// `SCUTTLE_SHIPDEATHTIMEMIN` / `MAX` (0x911990, 0x8a21fc): the self-destruct delay range.
    pub scuttle_death_time_min: f32,
    pub scuttle_death_time_max: f32,
    /// Yaw/pitch/roll ranges (lo, hi) of the spin an explosion, a bullet hit and a dying ship
    /// add.
    pub explosion_rock: [[f32; 2]; 3],
    pub bullet_rock: [[f32; 2]; 3],
    pub careen_rock: [[f32; 2]; 3],
    /// `YAWCRAZYLO`.. (0x8a2144): the spin of a disabled ship.
    pub crazy_rock: [[f32; 2]; 3],
    /// `kamikazeDamage[type]` (Tactics.script, 0xa58640): damage a kamikaze ship of the type
    /// does to what it rams.
    pub kamikaze_damage: Vec<f32>,
    /// `kamikazeSpeedBurst` (Tactics.script, 0xa58888; 0 compiled): a kamikaze closing in
    /// (`SPECIAL_KamikazeCrazyFast`) flies at this times its top speed. Saved from version 5;
    /// older files take the shipped script's 3.5.
    #[serde(default = "kamikaze_speed_burst", deserialize_with = "kamikaze_speed_burst_v5")]
    pub kamikaze_speed_burst: f32,
    /// `burstRange` (Tactics.script, 0xa585f8): how near the point a heavy corvette's burst
    /// fires from. Saved from version 7; older files take the shipped scripts' values.
    #[serde(default = "burst_range", deserialize_with = "burst_range_v7")]
    pub burst_range: f32,
    /// `burstChargeTime` (0xa5861c).
    #[serde(default = "burst_charge_time", deserialize_with = "burst_charge_time_v7")]
    pub burst_charge_time: f32,
    /// `burstCoolDownTime` (0xa58884).
    #[serde(default = "burst_cool_down_time", deserialize_with = "burst_cool_down_time_v7")]
    pub burst_cool_down_time: f32,
    /// `TW_BURST_ATTACK_FORCE_MODIFIER` (Tweak.script, 0xa47fdc): scales the push of a burst.
    #[serde(default = "burst_force_modifier", deserialize_with = "burst_force_modifier_v7")]
    pub burst_force_modifier: f32,
    /// Missile target loss (Tweak.script): frame mask, checks per missile, distance,
    /// miss chance by the target's tactics, bonus when it dodges or manoeuvres.
    pub missile_target_loss_rate: u32,
    pub missile_num_times_check: i32,
    pub missile_min_dist_before_target_loss: f32,
    pub missile_randomness: [f32; 3],
    pub missile_boost_if_dodging: f32,
    pub missile_boost_if_flightman: f32,
    pub friendly_fire_modifier: f32,
    pub armour_piercing_modifier: f32,
    pub collision_clear_time: f32,
    /// Squared.
    pub glance_collision_vel_ratio: f32,
    pub collision_repulse_factor: f32,
    pub checkbogey_frame: u32,
    pub check_passive_attack_rate: u32,
    pub check_passive_attack_whilemoving_rate: u32,
    pub check_attackmove_to_move_rate: u32,
    pub check_attackmove_to_move_frame: u32,
    pub tw_noone_see_time: f32,
    pub tw_first_see_time: f32,
    pub tw_second_see_time: f32,
    pub tw_first_see_percentage: u32,
    pub tw_second_see_percentage: u32,
    /// Per class (CLASS_MonsterShip shares the Corvette slot, as in the binary).
    pub cap_ship_lie_flat_distance: [f32; 12],
    /// Squared.
    pub cap_ship_lie_flat_speed_sq: [f32; 12],
    pub holding_pattern_pathtime: f32,
    pub holding_pattern_path_distance: f32,
    pub holding_pattern_num_turns: f32,
    pub holding_pattern_path_max_speed: f32,
    pub holding_pattern_heading_coefficient: f32,
    pub holding_pattern_right_coefficient: f32,

    pub flightman: FlightManTweaks,
}

impl Default for CombatTweaks {
    fn default() -> Self {
        Self {
            interceptor_break_tolerance: 110.0,
            interceptor_break_min_velocity: -200.0,
            interceptor_kill_min_velocity: -200.0,
            row_going_mostly_updown_angle: f32::from_bits(0x3f34_fdf4),
            avoid_row_ship_by_scale: 4.0,
            avoid_row_ship_by_add: 200.0,
            avoid_row_r1mothership_by: 1000.0,
            row_go_down_instead_of_up_ratio: f32::from_bits(0x3e99_999a),
            dont_use_velocity_pred_if_below: 1000.0,
            row_avoid_by: ROW_AVOID_BY,
            fighter_pairup_prob: 255,
            fighter_pair_check_rate: [7; 3],
            fighter_single_check_rate: [31; 3],
            fighter_max_threat_dist: [4.0e6; 3],
            wingman_join_max_dist: 40000.0,
            multipletargets_retarget_rate: 31,
            fighter_break_angle_min: 25,
            fighter_break_angle_max: 45,
            fighter_break_vertical_angle_min: 10,
            fighter_break_vertical_angle_max: 30,
            protect_get_this_close_cap: 2000.0,
            protect_get_this_close_strike: 2000.0,
            protect_still_velocity: 20.0,
            rescontroller_follow_behind: 1.5,
            enemy_near_zone: 0.0,
            passive_retaliation_fire_time_modifier_lo: 0.0,
            passive_retaliation_fire_time_modifier_hi: 0.0,
            attacking_from_above_min_dist: 250.0,
            attacking_from_above_min_ratio: 2.0,
            tactics_guard_return_to_guarding_distance: 0.0,
            tactics_guard_condition_check_rate: 0.0,
            tactics_guard_condition_check_frame: 0.0,
            burst_speed: 0.0,
            burst_radius: 0.0,
            hold_formation_during_battle: [0; 3],
            moving_attack_turns_into_move_command_distance: 0.0,
            blast_constant: 100_000.0,
            blast_damage_max_percent: 0.5,
            blast_constant_scuttle: 150_000.0,
            blast_radius_scuttle_bonus: f32::from_bits(0x3faa_3d71),
            blast_damage_max_percent_scuttle: 0.75,
            scuttle_death_time_min: 0.0,
            scuttle_death_time_max: 1.0,
            explosion_rock: [[-100.0, 100.0]; 3],
            bullet_rock: [[-100.0, 100.0]; 3],
            careen_rock: [[-100.0, 100.0]; 3],
            crazy_rock: [[-10.0, 10.0]; 3],
            kamikaze_damage: vec![0.0; ShipType::COUNT],
            kamikaze_speed_burst: 0.0,
            burst_range: 0.0,
            burst_charge_time: 0.0,
            burst_cool_down_time: 0.0,
            burst_force_modifier: 0.0,
            missile_target_loss_rate: 0,
            missile_num_times_check: 0,
            missile_min_dist_before_target_loss: 0.0,
            missile_randomness: [0.0; 3],
            missile_boost_if_dodging: 0.0,
            missile_boost_if_flightman: 0.0,
            friendly_fire_modifier: 0.5,
            armour_piercing_modifier: 1.5,
            collision_clear_time: 5.0,
            glance_collision_vel_ratio: 4.0,
            collision_repulse_factor: 500.0,
            checkbogey_frame: 1,
            check_passive_attack_rate: 31,
            check_passive_attack_whilemoving_rate: 31,
            check_attackmove_to_move_rate: 31,
            check_attackmove_to_move_frame: 22,
            tw_noone_see_time: 0.0,
            tw_first_see_time: 0.0,
            tw_second_see_time: 0.0,
            tw_first_see_percentage: 0,
            tw_second_see_percentage: 0,
            cap_ship_lie_flat_distance: [0.0; 12],
            cap_ship_lie_flat_speed_sq: [0.0; 12],
            holding_pattern_pathtime: 0.0,
            holding_pattern_path_distance: 0.0,
            holding_pattern_num_turns: 0.0,
            holding_pattern_path_max_speed: 0.0,
            holding_pattern_heading_coefficient: 0.0,
            holding_pattern_right_coefficient: 0.0,
            flightman: FlightManTweaks::default(),
        }
    }
}

/// Compiled `rowAvoidBy` (0x8a783c), before AIShip.script's `rowDetails`.
const ROW_AVOID_BY: [f32; ShipType::COUNT] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1000.0, 1000.0, 1000.0, 0.0, 1000.0, 1000.0, 1600.0, 1600.0, 1000.0, 0.0, 1000.0,
    0.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0, 800.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0,
    2000.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1000.0, 1000.0, 0.0, 1000.0, 1000.0, 1500.0, 1600.0, 1000.0,
    1000.0, 0.0, 1000.0, 0.0, 2000.0, 2000.0, 2000.0, 2000.0, 2000.0, 0.0, 2000.0, 1000.0, 0.0, 1600.0, 1000.0,
    1000.0, 0.0, 1500.0, 0.0, 0.0, 1000.0, 0.0, 0.0, 0.0, 1500.0, 1000.0, 0.0, 1600.0, 1000.0, 1000.0, 1000.0,
    1500.0, 0.0, 0.0, 1000.0, 0.0, 1500.0, 0.0, 0.0, 1000.0, 0.0, 1500.0, 1000.0, 1000.0, 0.0, 1000.0, 0.0, 1700.0,
    0.0, 2000.0, 1000.0, 1700.0, 1700.0, 1000.0, 1000.0, 1000.0, 2000.0, 1000.0, 2000.0, 2000.0, 2500.0, 1000.0,
    1000.0, 0.0, 0.0, 0.0, 0.0, 500.0, 500.0, 500.0, 500.0, 500.0, 500.0, 500.0, 0.0, 0.0, 0.0, 500.0, 500.0, 2000.0,
    0.0, 2000.0, 4000.0, 500.0, 0.0, 1700.0, 1700.0, 500.0, 1600.0, 1000.0, 1000.0, 1000.0, 1000.0, 0.0, 0.0, 0.0,
];

#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Conv {
    Real,
    /// `scriptSetReal32SqrCB`.
    Sqr,
    /// `scriptSetCosAngCB`.
    CosAng,
    /// `scriptSetAngCB`.
    Ang,
    /// `scriptSetSdwordCB` / `scriptSetUdwordCB` / `scriptSetSwordCB`.
    Int,
}

enum Target {
    F(fn(&mut CombatTweaks) -> &mut f32),
    U(fn(&mut CombatTweaks) -> &mut u32),
    I(fn(&mut CombatTweaks) -> &mut i32),
    W(fn(&mut CombatTweaks) -> &mut u16),
}

use Conv::*;
use Target::*;

macro_rules! fm {
    ($field:ident) => {
        F(|t| &mut t.flightman.$field)
    };
    ($field:ident [$i:expr]) => {
        F(|t| &mut t.flightman.$field[$i])
    };
}

const FILE_FLIGHTMAN: &str = "FlightMan.script";
const FILE_TACTICS: &str = "Tactics.script";
const FILE_CL: &str = "CommandLayer.script";
const FILE_TWEAK: &str = "Tweak.script";
const FILE_AISHIP: &str = "AIShip.script";

/// Class names in `CLASS_*` order with the slot each keyword writes (MonsterShip aliases the
/// Corvette slot in both cap-ship tables).
const LIE_FLAT_CLASSES: [(&str, usize); 12] = [
    ("CLASS_Mothership", 0),
    ("CLASS_HeavyCruiser", 1),
    ("CLASS_Carrier", 2),
    ("CLASS_Destroyer", 3),
    ("CLASS_Frigate", 4),
    ("CLASS_Corvette", 5),
    ("CLASS_Fighter", 6),
    ("CLASS_Resource", 7),
    ("CLASS_NonCombat", 8),
    ("CLASS_MonsterShip", 5),
    ("CLASS_Component", 10),
    ("CLASS_Micro", 11),
];

const BINDINGS: &[(&str, &str, Conv, Target)] = &[
    (FILE_AISHIP, "INTERCEPTORBREAK_TOLERANCE", Real, F(|t| &mut t.interceptor_break_tolerance)),
    (FILE_AISHIP, "INTERCEPTORBREAK_MINVELOCITY", Real, F(|t| &mut t.interceptor_break_min_velocity)),
    (FILE_AISHIP, "INTERCEPTORKILL_MINVELOCITY", Real, F(|t| &mut t.interceptor_kill_min_velocity)),
    (FILE_AISHIP, "ROW_GOING_MOSTLY_UPDOWN_ANGLE", CosAng, F(|t| &mut t.row_going_mostly_updown_angle)),
    (FILE_AISHIP, "AVOID_ROW_SHIP_BY_SCALE", Real, F(|t| &mut t.avoid_row_ship_by_scale)),
    (FILE_AISHIP, "AVOID_ROW_SHIP_BY_ADD", Real, F(|t| &mut t.avoid_row_ship_by_add)),
    (FILE_AISHIP, "AVOID_ROW_R1MOTHERSHIP_BY", Real, F(|t| &mut t.avoid_row_r1mothership_by)),
    (FILE_AISHIP, "ROW_GO_DOWN_INSTEAD_OF_UP_RATIO", Real, F(|t| &mut t.row_go_down_instead_of_up_ratio)),
    (FILE_AISHIP, "DONTUSEVELOCITYPRED_IFBELOW", Real, F(|t| &mut t.dont_use_velocity_pred_if_below)),
    (FILE_CL, "FIGHTER_PAIRUP_PROB", Int, U(|t| &mut t.fighter_pairup_prob)),
    (FILE_CL, "FIGHTER_PAIR_CHECK_RATE[Evasive]", Int, U(|t| &mut t.fighter_pair_check_rate[0])),
    (FILE_CL, "FIGHTER_PAIR_CHECK_RATE[Neutral]", Int, U(|t| &mut t.fighter_pair_check_rate[1])),
    (FILE_CL, "FIGHTER_PAIR_CHECK_RATE[Aggressive]", Int, U(|t| &mut t.fighter_pair_check_rate[2])),
    (FILE_CL, "FIGHTER_SINGLE_CHECK_RATE[Evasive]", Int, U(|t| &mut t.fighter_single_check_rate[0])),
    (FILE_CL, "FIGHTER_SINGLE_CHECK_RATE[Neutral]", Int, U(|t| &mut t.fighter_single_check_rate[1])),
    (FILE_CL, "FIGHTER_SINGLE_CHECK_RATE[Aggressive]", Int, U(|t| &mut t.fighter_single_check_rate[2])),
    (FILE_CL, "FIGHTER_MAX_THREAT_DIST[Evasive]", Sqr, F(|t| &mut t.fighter_max_threat_dist[0])),
    (FILE_CL, "FIGHTER_MAX_THREAT_DIST[Neutral]", Sqr, F(|t| &mut t.fighter_max_threat_dist[1])),
    (FILE_CL, "FIGHTER_MAX_THREAT_DIST[Aggressive]", Sqr, F(|t| &mut t.fighter_max_threat_dist[2])),
    (FILE_CL, "WINGMAN_JOIN_MAX_DIST", Sqr, F(|t| &mut t.wingman_join_max_dist)),
    (FILE_CL, "MULTIPLETARGETS_RETARGET_RATE", Int, U(|t| &mut t.multipletargets_retarget_rate)),
    (FILE_CL, "FIGHTER_BREAK_ANGLE_MIN", Int, I(|t| &mut t.fighter_break_angle_min)),
    (FILE_CL, "FIGHTER_BREAK_ANGLE_MAX", Int, I(|t| &mut t.fighter_break_angle_max)),
    (FILE_CL, "PROTECT_GET_THIS_CLOSE_CAP", Real, F(|t| &mut t.protect_get_this_close_cap)),
    (FILE_CL, "PROTECT_GET_THIS_CLOSE_STRIKE", Real, F(|t| &mut t.protect_get_this_close_strike)),
    (FILE_CL, "PROTECT_STILL_VELOCITY", Real, F(|t| &mut t.protect_still_velocity)),
    (FILE_CL, "RESCONTROLLER_FOLLOW_BEHIND", Real, F(|t| &mut t.rescontroller_follow_behind)),
    (FILE_CL, "FIGHTER_BREAK_VERTICAL_ANGLE_MIN", Int, I(|t| &mut t.fighter_break_vertical_angle_min)),
    (FILE_CL, "FIGHTER_BREAK_VERTICAL_ANGLE_MAX", Int, I(|t| &mut t.fighter_break_vertical_angle_max)),
    (FILE_TACTICS, "EnemyNearZone", Real, F(|t| &mut t.enemy_near_zone)),
    (FILE_TACTICS, "kamikazeSpeedBurst", Real, F(|t| &mut t.kamikaze_speed_burst)),
    (FILE_TACTICS, "PassiveRetaliationFireTimeModifierLow", Real, F(|t| &mut t.passive_retaliation_fire_time_modifier_lo)),
    (FILE_TACTICS, "PassiveRetaliationFireTimeModifierHi", Real, F(|t| &mut t.passive_retaliation_fire_time_modifier_hi)),
    (FILE_TACTICS, "ATTACKING_FROM_ABOVE_MIN_DIST", Real, F(|t| &mut t.attacking_from_above_min_dist)),
    (FILE_TACTICS, "ATTACKING_FROM_ABOVE_MIN_RATIO", Real, F(|t| &mut t.attacking_from_above_min_ratio)),
    (FILE_TACTICS, "tacticsGuardReturnToGuardingDistance", Real, F(|t| &mut t.tactics_guard_return_to_guarding_distance)),
    (FILE_TACTICS, "tacticsGuardConditionCheckRate", Real, F(|t| &mut t.tactics_guard_condition_check_rate)),
    (FILE_TACTICS, "tacticsGuardConditionCheckFrame", Real, F(|t| &mut t.tactics_guard_condition_check_frame)),
    (FILE_TACTICS, "burstRange", Real, F(|t| &mut t.burst_range)),
    (FILE_TACTICS, "burstChargeTime", Real, F(|t| &mut t.burst_charge_time)),
    (FILE_TACTICS, "burstCoolDownTime", Real, F(|t| &mut t.burst_cool_down_time)),
    (FILE_TWEAK, "TW_BURST_ATTACK_FORCE_MODIFIER", Real, F(|t| &mut t.burst_force_modifier)),
    (FILE_TACTICS, "burstSpeed", Real, F(|t| &mut t.burst_speed)),
    (FILE_TACTICS, "burstRadius", Real, F(|t| &mut t.burst_radius)),
    (FILE_TACTICS, "holdFormationDuringBattle[Evasive]", Int, U(|t| &mut t.hold_formation_during_battle[0])),
    (FILE_TACTICS, "holdFormationDuringBattle[Neutral]", Int, U(|t| &mut t.hold_formation_during_battle[1])),
    (FILE_TACTICS, "holdFormationDuringBattle[Aggressive]", Int, U(|t| &mut t.hold_formation_during_battle[2])),
    (FILE_TACTICS, "movingAttackTurnsIntoMoveCommandDistance", Sqr, F(|t| &mut t.moving_attack_turns_into_move_command_distance)),
    (FILE_TWEAK, "BLAST_CONSTANT", Real, F(|t| &mut t.blast_constant)),
    (FILE_TWEAK, "BLAST_DAMAGE_MAX_PERCENT", Real, F(|t| &mut t.blast_damage_max_percent)),
    (FILE_TWEAK, "BLAST_CONSTANT_SCUTTLE", Real, F(|t| &mut t.blast_constant_scuttle)),
    (FILE_TWEAK, "BLAST_RADIUS_SCUTTLE_BONUS", Real, F(|t| &mut t.blast_radius_scuttle_bonus)),
    (FILE_TWEAK, "BLAST_DAMAGE_MAX_PERCENT_SCUTTLE", Real, F(|t| &mut t.blast_damage_max_percent_scuttle)),
    (FILE_TWEAK, "SCUTTLE_SHIPDEATHTIMEMIN", Real, F(|t| &mut t.scuttle_death_time_min)),
    (FILE_TWEAK, "SCUTTLE_SHIPDEATHTIMEMAX", Real, F(|t| &mut t.scuttle_death_time_max)),
    (FILE_TWEAK, "YAWEXPLOSIONROCKLO", Real, F(|t| &mut t.explosion_rock[0][0])),
    (FILE_TWEAK, "YAWEXPLOSIONROCKHI", Real, F(|t| &mut t.explosion_rock[0][1])),
    (FILE_TWEAK, "PITCHEXPLOSIONROCKLO", Real, F(|t| &mut t.explosion_rock[1][0])),
    (FILE_TWEAK, "PITCHEXPLOSIONROCKHI", Real, F(|t| &mut t.explosion_rock[1][1])),
    (FILE_TWEAK, "ROLLEXPLOSIONROCKLO", Real, F(|t| &mut t.explosion_rock[2][0])),
    (FILE_TWEAK, "ROLLEXPLOSIONROCKHI", Real, F(|t| &mut t.explosion_rock[2][1])),
    (FILE_TWEAK, "YAWROCKLO", Real, F(|t| &mut t.bullet_rock[0][0])),
    (FILE_TWEAK, "YAWROCKHI", Real, F(|t| &mut t.bullet_rock[0][1])),
    (FILE_TWEAK, "PITCHROCKLO", Real, F(|t| &mut t.bullet_rock[1][0])),
    (FILE_TWEAK, "PITCHROCKHI", Real, F(|t| &mut t.bullet_rock[1][1])),
    (FILE_TWEAK, "ROLLROCKLO", Real, F(|t| &mut t.bullet_rock[2][0])),
    (FILE_TWEAK, "ROLLROCKHI", Real, F(|t| &mut t.bullet_rock[2][1])),
    (FILE_TWEAK, "YAWCRAZYLO", Real, F(|t| &mut t.crazy_rock[0][0])),
    (FILE_TWEAK, "YAWCRAZYHI", Real, F(|t| &mut t.crazy_rock[0][1])),
    (FILE_TWEAK, "PITCHCRAZYLO", Real, F(|t| &mut t.crazy_rock[1][0])),
    (FILE_TWEAK, "PITCHCRAZYHI", Real, F(|t| &mut t.crazy_rock[1][1])),
    (FILE_TWEAK, "ROLLCRAZYLO", Real, F(|t| &mut t.crazy_rock[2][0])),
    (FILE_TWEAK, "ROLLCRAZYHI", Real, F(|t| &mut t.crazy_rock[2][1])),
    (FILE_TWEAK, "tweakCheckMissileTargetLossRate", Int, U(|t| &mut t.missile_target_loss_rate)),
    (FILE_TWEAK, "tweakNumTimesCheck", Int, I(|t| &mut t.missile_num_times_check)),
    (FILE_TWEAK, "tweakMinDistBeforeTargetLoss", Real, F(|t| &mut t.missile_min_dist_before_target_loss)),
    (FILE_TWEAK, "tweakRandomNess[Evasive]", Real, F(|t| &mut t.missile_randomness[0])),
    (FILE_TWEAK, "tweakRandomNess[Neutral]", Real, F(|t| &mut t.missile_randomness[1])),
    (FILE_TWEAK, "tweakRandomNess[Aggressive]", Real, F(|t| &mut t.missile_randomness[2])),
    (FILE_TWEAK, "tweakBoostIfDodging", Real, F(|t| &mut t.missile_boost_if_dodging)),
    (FILE_TWEAK, "tweakBoostIfFlightMan", Real, F(|t| &mut t.missile_boost_if_flightman)),
    (FILE_TWEAK, "YAWCAREENLO", Real, F(|t| &mut t.careen_rock[0][0])),
    (FILE_TWEAK, "YAWCAREENHI", Real, F(|t| &mut t.careen_rock[0][1])),
    (FILE_TWEAK, "PITCHCAREENLO", Real, F(|t| &mut t.careen_rock[1][0])),
    (FILE_TWEAK, "PITCHCAREENHI", Real, F(|t| &mut t.careen_rock[1][1])),
    (FILE_TWEAK, "ROLLCAREENLO", Real, F(|t| &mut t.careen_rock[2][0])),
    (FILE_TWEAK, "ROLLCAREENHI", Real, F(|t| &mut t.careen_rock[2][1])),
    (FILE_TWEAK, "friendlyFireModifier", Real, F(|t| &mut t.friendly_fire_modifier)),
    (FILE_TWEAK, "armourPiercingModifier", Real, F(|t| &mut t.armour_piercing_modifier)),
    (FILE_TWEAK, "COLLISION_CLEAR_TIME", Real, F(|t| &mut t.collision_clear_time)),
    (FILE_TWEAK, "GLANCE_COLLISION_VEL_RATIO", Sqr, F(|t| &mut t.glance_collision_vel_ratio)),
    (FILE_TWEAK, "COLLISION_REPULSE_FACTOR", Real, F(|t| &mut t.collision_repulse_factor)),
    (FILE_TWEAK, "CHECKBOGEY_FRAME", Int, U(|t| &mut t.checkbogey_frame)),
    (FILE_TWEAK, "CHECK_ATTACKMOVE_TO_MOVE_RATE", Int, U(|t| &mut t.check_attackmove_to_move_rate)),
    (FILE_TWEAK, "CHECK_ATTACKMOVE_TO_MOVE_FRAME", Int, U(|t| &mut t.check_attackmove_to_move_frame)),
    (FILE_TWEAK, "CHECK_PASSIVE_ATTACK_RATE", Int, U(|t| &mut t.check_passive_attack_rate)),
    (FILE_TWEAK, "CHECK_PASSIVE_ATTACK_WHILEMOVING_RATE", Int, U(|t| &mut t.check_passive_attack_whilemoving_rate)),
    (FILE_TWEAK, "TW_NOONE_SEE_TIME", Real, F(|t| &mut t.tw_noone_see_time)),
    (FILE_TWEAK, "TW_FIRST_SEE_TIME", Real, F(|t| &mut t.tw_first_see_time)),
    (FILE_TWEAK, "TW_SECOND_SEE_TIME", Real, F(|t| &mut t.tw_second_see_time)),
    (FILE_TWEAK, "TW_FIRST_SEE_PERCENTAGE", Int, U(|t| &mut t.tw_first_see_percentage)),
    (FILE_TWEAK, "TW_SECOND_SEE_PERCENTAGE", Int, U(|t| &mut t.tw_second_see_percentage)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_PATHTIME", Real, F(|t| &mut t.holding_pattern_pathtime)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_PATH_DISTANCE", Real, F(|t| &mut t.holding_pattern_path_distance)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_NUM_TURNS", Real, F(|t| &mut t.holding_pattern_num_turns)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_PATH_MAX_SPEED", Real, F(|t| &mut t.holding_pattern_path_max_speed)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_HEADING_COEFFICIENT", Real, F(|t| &mut t.holding_pattern_heading_coefficient)),
    (FILE_TWEAK, "TW_HOLDING_PATTERN_RIGHT_COEFFICIENT", Real, F(|t| &mut t.holding_pattern_right_coefficient)),
    (FILE_FLIGHTMAN, "FLIPTURN_MAXROTSPEEDFLIP", Real, fm!(flipturn_maxrotspeedflip)),
    (FILE_FLIGHTMAN, "FLIPTURN_PAUSEAFTERFLIPTIME", Real, fm!(flipturn_pauseafterfliptime)),
    (FILE_FLIGHTMAN, "FLIPTURN_FLIPACCELMODIFIER", Real, fm!(flipturn_flipaccelmodifier)),
    (FILE_FLIGHTMAN, "FLIPTURN_AFTERFLIPTHRUSTMOD", Real, fm!(flipturn_afterflipthrustmod)),
    (FILE_FLIGHTMAN, "FLIPTURN_ROLLTHRUSTMOD", Real, fm!(flipturn_rollthrustmod)),
    (FILE_FLIGHTMAN, "FLIPTURN_ROLLACCELMODIFIER", Real, fm!(flipturn_rollaccelmodifier)),
    (FILE_FLIGHTMAN, "FLIPTURN_MAXROLLSPEED", Real, fm!(flipturn_maxrollspeed)),
    (FILE_FLIGHTMAN, "IMMELMAN_THRUSTMOD", Real, fm!(immelman_thrustmod)),
    (FILE_FLIGHTMAN, "IMMELMAN_ACCELMODIFIERFLIP", Real, fm!(immelman_accelmodifierflip)),
    (FILE_FLIGHTMAN, "IMMELMAN_MAXROTSPEEDFLIP", Real, fm!(immelman_maxrotspeedflip)),
    (FILE_FLIGHTMAN, "IMMELMAN_ROLLACCELMODIFIER", Real, fm!(immelman_rollaccelmodifier)),
    (FILE_FLIGHTMAN, "IMMELMAN_MAXROLLSPEED", Real, fm!(immelman_maxrollspeed)),
    (FILE_FLIGHTMAN, "IMMELMAN_ROLLTHRUSTMOD", Real, fm!(immelman_rollthrustmod)),
    (FILE_FLIGHTMAN, "SPLIT_S_THRUSTMOD", Real, fm!(split_s_thrustmod)),
    (FILE_FLIGHTMAN, "SPLIT_S_ACCELMODIFIERFLIP", Real, fm!(split_s_accelmodifierflip)),
    (FILE_FLIGHTMAN, "SPLIT_S_MAXROTSPEEDPITCH", Real, fm!(split_s_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "SPLIT_S_ACCELMODIFIERROLL", Real, fm!(split_s_accelmodifierroll)),
    (FILE_FLIGHTMAN, "SPLIT_S_MAXROTSPEEDROLL", Real, fm!(split_s_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "HARDBANK_THRUSTMOD", Real, fm!(hardbank_thrustmod)),
    (FILE_FLIGHTMAN, "HARDBANK_ACCELMODIFIERFLIP", Real, fm!(hardbank_accelmodifierflip)),
    (FILE_FLIGHTMAN, "HARDBANK_MAXROTSPEEDPITCH", Real, fm!(hardbank_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "HARDBANK_ACCELMODIFIERROLL", Real, fm!(hardbank_accelmodifierroll)),
    (FILE_FLIGHTMAN, "HARDBANK_MAXROTSPEEDROLL", Real, fm!(hardbank_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "SOFTBANK_THRUSTMOD", Real, fm!(softbank_thrustmod)),
    (FILE_FLIGHTMAN, "SOFTBANK_ACCELMODIFIERFLIP", Real, fm!(softbank_accelmodifierflip)),
    (FILE_FLIGHTMAN, "SOFTBANK_MAXROTSPEEDPITCH", Real, fm!(softbank_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "SOFTBANK_ACCELMODIFIERROLL", Real, fm!(softbank_accelmodifierroll)),
    (FILE_FLIGHTMAN, "SOFTBANK_MAXROTSPEEDROLL", Real, fm!(softbank_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "BARREL_ROLL_ACCELMODIFIERROLL", Real, fm!(barrel_roll_accelmodifierroll)),
    (FILE_FLIGHTMAN, "BARREL_ROLL_MAXROTSPEEDROLL", Real, fm!(barrel_roll_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_ESCAPEDIST_Z", Real, fm!(whipstrafe_escapedist_z)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_BULLETRANGEOPTIMUM", Real, fm!(whipstrafe_bulletrangeoptimum)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_MAXANGCOMPAREFACTOR", Sqr, fm!(whipstrafe_maxangcomparefactor)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_FLYBYOVERSHOOT", Real, fm!(whipstrafe_flybyovershoot)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_MINVELOCITY", Real, fm!(whipstrafe_minvelocity)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_REACHDESTINATIONRANGE", Real, fm!(whipstrafe_reachdestinationrange)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_FACEDOWNUP_ACCURACY", CosAng, fm!(whipstrafe_facedownup_accuracy)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_WHIPTHRUSTMOD", Real, fm!(whipstrafe_whipthrustmod)),
    (FILE_FLIGHTMAN, "WHIPSTRAFE_WHIPTIME", Real, fm!(whipstrafe_whiptime)),
    (FILE_FLIGHTMAN, "ROLLAWAY_THRUSTMOD", Real, fm!(rollaway_thrustmod)),
    (FILE_FLIGHTMAN, "ROLLAWAY_ACCELMODIFIERROLL", Real, fm!(rollaway_accelmodifierroll)),
    (FILE_FLIGHTMAN, "ROLLAWAY_MAXROTSPEEDROLL", Real, fm!(rollaway_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "ROLLAWAY_THRUSTDOWNTIME", Real, fm!(rollaway_thrustdowntime)),
    (FILE_FLIGHTMAN, "SPLITSEVASIVE_ACCELMODIFIERPITCH", Real, fm!(splitsevasive_accelmodifierpitch)),
    (FILE_FLIGHTMAN, "SPLITSEVASIVE_MAXROTSPEEDPITCH", Real, fm!(splitsevasive_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "SPLITSEVASIVE_THRUSTMOD", Real, fm!(splitsevasive_thrustmod)),
    (FILE_FLIGHTMAN, "SPLITSEVASIVE_ACCELMODIFIERROLL", Real, fm!(splitsevasive_accelmodifierroll)),
    (FILE_FLIGHTMAN, "SPLITSEVASIVE_MAXROTSPEEDROLL", Real, fm!(splitsevasive_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "LOYOYO_THRUSTMOD0", Real, fm!(loyoyo_thrustmod[0])),
    (FILE_FLIGHTMAN, "LOYOYO_THRUSTMOD1", Real, fm!(loyoyo_thrustmod[1])),
    (FILE_FLIGHTMAN, "LOYOYO_THRUSTMOD2", Real, fm!(loyoyo_thrustmod[2])),
    (FILE_FLIGHTMAN, "LOYOYO_THRUSTMOD3", Real, fm!(loyoyo_thrustmod[3])),
    (FILE_FLIGHTMAN, "LOYOYO_THRUSTMOD4", Real, fm!(loyoyo_thrustmod[4])),
    (FILE_FLIGHTMAN, "LOYOYO_ACCELMODIFIERPITCH", Real, fm!(loyoyo_accelmodifierpitch)),
    (FILE_FLIGHTMAN, "LOYOYO_MAXROTSPEEDPITCH", Real, fm!(loyoyo_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "LOYOYO_GOUP_TIME", Real, fm!(loyoyo_goup_time)),
    (FILE_FLIGHTMAN, "LOYOYO_GODOWN_TIME", Real, fm!(loyoyo_godown_time)),
    (FILE_FLIGHTMAN, "HIYOYO_THRUSTMOD0", Real, fm!(hiyoyo_thrustmod[0])),
    (FILE_FLIGHTMAN, "HIYOYO_THRUSTMOD1", Real, fm!(hiyoyo_thrustmod[1])),
    (FILE_FLIGHTMAN, "HIYOYO_THRUSTMOD2", Real, fm!(hiyoyo_thrustmod[2])),
    (FILE_FLIGHTMAN, "HIYOYO_THRUSTMOD3", Real, fm!(hiyoyo_thrustmod[3])),
    (FILE_FLIGHTMAN, "HIYOYO_THRUSTMOD4", Real, fm!(hiyoyo_thrustmod[4])),
    (FILE_FLIGHTMAN, "HIYOYO_ACCELMODIFIERPITCH", Real, fm!(hiyoyo_accelmodifierpitch)),
    (FILE_FLIGHTMAN, "HIYOYO_MAXROTSPEEDPITCH", Real, fm!(hiyoyo_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "HIYOYO_GOUP_TIME", Real, fm!(hiyoyo_goup_time)),
    (FILE_FLIGHTMAN, "HIYOYO_GODOWN_TIME", Real, fm!(hiyoyo_godown_time)),
    (FILE_FLIGHTMAN, "SIDESTEP_THRUSTMOD", Real, fm!(sidestep_thrustmod)),
    (FILE_FLIGHTMAN, "SIDESTEP_THRUSTMODTURN", Real, fm!(sidestep_thrustmodturn)),
    (FILE_FLIGHTMAN, "SIDESTEP_ACCELMODIFIERYAW", Real, fm!(sidestep_accelmodifieryaw)),
    (FILE_FLIGHTMAN, "SIDESTEP_MAXROTSPEEDYAW", Real, fm!(sidestep_maxrotspeedyaw)),
    (FILE_FLIGHTMAN, "SIDESTEP_ACCELMODIFIERPITCH", Real, fm!(sidestep_accelmodifierpitch)),
    (FILE_FLIGHTMAN, "SIDESTEP_MAXROTSPEEDPITCH", Real, fm!(sidestep_maxrotspeedpitch)),
    (FILE_FLIGHTMAN, "SIDESTEP_ACCELMODIFIERROLL", Real, fm!(sidestep_accelmodifierroll)),
    (FILE_FLIGHTMAN, "SIDESTEP_MAXROTSPEEDROLL", Real, fm!(sidestep_maxrotspeedroll)),
    (FILE_FLIGHTMAN, "SIDESTEP_PAUSETIME", Real, fm!(sidestep_pausetime)),
    (FILE_FLIGHTMAN, "SIDESTEP_BOOTITTIME", Real, fm!(sidestep_bootittime)),
    (FILE_FLIGHTMAN, "SIDESTEP_ROLLROTATE", Ang, fm!(sidestep_rollrotate)),
    (FILE_FLIGHTMAN, "SIDESTEP_PITCHROTATE", Ang, fm!(sidestep_pitchrotate)),
    (FILE_FLIGHTMAN, "SLALOM_MINVELOCITY", Real, fm!(slalom_minvelocity)),
    (FILE_FLIGHTMAN, "SLALOM_REACHDESTINATIONRANGE", Real, fm!(slalom_reachdestinationrange)),
    (FILE_FLIGHTMAN, "SLALOM_MAXWAYPOINTTIME", Real, fm!(slalom_maxwaypointtime)),
    (FILE_FLIGHTMAN, "SLALOM_RUN_LENGTH", Real, fm!(slalom_run_length)),
    (FILE_FLIGHTMAN, "SLALOM_RANDOMITERATIONS", Int, U(|t| &mut t.flightman.slalom_randomiterations)),
    (FILE_FLIGHTMAN, "SLALOM_BASEITERATIONS", Int, U(|t| &mut t.flightman.slalom_baseiterations)),
    (FILE_FLIGHTMAN, "SANDWICH_ANGLE", Ang, fm!(sandwich_angle)),
    (FILE_FLIGHTMAN, "SANDWICH_MAXTIME", Real, fm!(sandwich_maxtime)),
    (FILE_FLIGHTMAN, "SANDWICH_REACHDESTINATIONRANGE", Real, fm!(sandwich_reachdestinationrange)),
    (FILE_FLIGHTMAN, "SANDWICH_MINVELOCITY", Real, fm!(sandwich_minvelocity)),
    (FILE_FLIGHTMAN, "SANDWICH_PINCHER_SCALE", Real, fm!(sandwich_pincher_scale)),
    (FILE_FLIGHTMAN, "FLIGHTMAN_TOKEN_EVASIVEPURE_CHECKRATE", Int, U(|t| &mut t.flightman.token_evasivepure_checkrate)),
    (FILE_FLIGHTMAN, "FLIGHTMAN_TOKEN_EVASIVEPURE_CHECKFRAME", Int, U(|t| &mut t.flightman.token_evasivepure_checkframe)),
    (FILE_FLIGHTMAN, "FLIGHTMAN_TOKEN_EVASIVEPURE_PROB", Int, U(|t| &mut t.flightman.token_evasivepure_prob)),
    (FILE_FLIGHTMAN, "FLIGHTMAN_EVASIVEPURE_PROB[Evasive]", Int, U(|t| &mut t.flightman.evasivepure_prob[0])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_EVASIVEPURE_PROB[Neutral]", Int, U(|t| &mut t.flightman.evasivepure_prob[1])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_EVASIVEPURE_PROB[Aggressive]", Int, U(|t| &mut t.flightman.evasivepure_prob[2])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SPLIT_PROB[Evasive]", Int, U(|t| &mut t.flightman.split_prob[0])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SPLIT_PROB[Neutral]", Int, U(|t| &mut t.flightman.split_prob[1])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SPLIT_PROB[Aggressive]", Int, U(|t| &mut t.flightman.split_prob[2])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SANDWICH_PROB[Evasive]", Int, U(|t| &mut t.flightman.sandwich_prob[0])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SANDWICH_PROB[Neutral]", Int, U(|t| &mut t.flightman.sandwich_prob[1])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_SANDWICH_PROB[Aggressive]", Int, U(|t| &mut t.flightman.sandwich_prob[2])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_NUM_EVASIVEPURE_MANEUVERS[Evasive]", Int, W(|t| &mut t.flightman.num_evasivepure_maneuvers[0])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_NUM_EVASIVEPURE_MANEUVERS[Neutral]", Int, W(|t| &mut t.flightman.num_evasivepure_maneuvers[1])),
    (FILE_FLIGHTMAN, "FLIGHTMAN_NUM_EVASIVEPURE_MANEUVERS[Aggressive]", Int, W(|t| &mut t.flightman.num_evasivepure_maneuvers[2])),
];

/// The shipped scripts' burst settings, for files older than version 7.
fn burst_range() -> f32 {
    5000.0
}
fn burst_charge_time() -> f32 {
    0.1
}
fn burst_cool_down_time() -> f32 {
    7.5
}
fn burst_force_modifier() -> f32 {
    0.2
}
fn burst_range_v7<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    crate::save::added_in_or(7, d, burst_range)
}
fn burst_charge_time_v7<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    crate::save::added_in_or(7, d, burst_charge_time)
}
fn burst_cool_down_time_v7<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    crate::save::added_in_or(7, d, burst_cool_down_time)
}
fn burst_force_modifier_v7<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    crate::save::added_in_or(7, d, burst_force_modifier)
}

/// `kamikazeSpeedBurst` as the shipped Tactics.script sets it.
fn kamikaze_speed_burst() -> f32 {
    3.5
}

/// Reads [`CombatTweaks::kamikaze_speed_burst`], added at save version 5.
fn kamikaze_speed_burst_v5<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f32, D::Error> {
    crate::save::added_in_or(5, d, kamikaze_speed_burst)
}

impl CombatTweaks {
    /// Compiled defaults overwritten by the scripts found in `src`, then the derived values.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = CombatTweaks::default();
        for file in [FILE_AISHIP, FILE_CL, FILE_TACTICS, FILE_TWEAK, FILE_FLIGHTMAN] {
            let Some(script) = src.read(file).and_then(|b| ScriptFile::parse_bytes(&b).ok()) else {
                continue;
            };
            t.apply(file, &script);
        }
        t
    }

    /// Applies the lines of one tunables file.
    pub fn apply(&mut self, file: &str, script: &ScriptFile) {
        for e in script.entries() {
            let (key, raw) = (e.key.as_str(), e.raw.as_str());
            if file == FILE_TACTICS {
                if let Some(t) = key.strip_prefix("kamikazeDamage[").and_then(|k| k.strip_suffix(']')).and_then(ShipType::from_name) {
                    if let Some(v) = scan_f32(raw) {
                        self.kamikaze_damage[t.index()] = v;
                    }
                    continue;
                }
            }
            if file == FILE_AISHIP && key == "rowDetails" {
                if let Some((t, _, avoid)) = parse_row_details(raw) {
                    self.row_avoid_by[t.index()] = avoid;
                }
                continue;
            }
            if file == FILE_TWEAK {
                for (name, slot) in LIE_FLAT_CLASSES {
                    if key.strip_prefix("capShipLieFlatInGeneralDistanceTolerance[").and_then(|k| k.strip_suffix(']')) == Some(name) {
                        if let Some(v) = scan_f32(raw) {
                            self.cap_ship_lie_flat_distance[slot] = v;
                        }
                    }
                    if key.strip_prefix("capShipLieFlatInGeneralSpeedSqrTolerance[").and_then(|k| k.strip_suffix(']')) == Some(name) {
                        if let Some(v) = scan_f32(raw) {
                            self.cap_ship_lie_flat_speed_sq[slot] = st(ld(v) * ld(v));
                        }
                    }
                }
            }
            for (_, _, conv, target) in BINDINGS.iter().filter(|b| b.0 == file && b.1 == key) {
                match (conv, target) {
                    (Int, U(f)) => {
                        if let Some(v) = scan_i32(raw) {
                            *f(self) = v as u32;
                        }
                    }
                    (Int, I(f)) => {
                        if let Some(v) = scan_i32(raw) {
                            *f(self) = v;
                        }
                    }
                    (Int, W(f)) => {
                        if let Some(v) = scan_i32(raw) {
                            *f(self) = v as u16;
                        }
                    }
                    (conv, F(f)) => {
                        if let Some(v) = scan_f32(raw) {
                            *f(self) = match conv {
                                Real | Int => v,
                                Sqr => st(ld(v) * ld(v)),
                                CosAng => st(fcos(ld(v) * ld(DEG_TO_RAD))),
                                Ang => st(ld(v) * ld(DEG_TO_RAD)),
                            };
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// `tacticsGuardConditionCheckRate` as the frame mask the original uses (the float's bits).
    pub fn guard_check_rate_bits(&self) -> (u32, u32) {
        (self.tactics_guard_condition_check_rate.to_bits(), self.tactics_guard_condition_check_frame.to_bits())
    }
}
