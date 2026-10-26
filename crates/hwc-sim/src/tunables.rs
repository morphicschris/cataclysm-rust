//! Run-time tunables of the simulation: the globals the movement code reads (`AIShip.c`,
//! `AITrack.c`, `Formation.c`, `Physics`, collision blobs, tactics padding), with the defaults
//! compiled into `cataclysm.exe` and the startup tweak scripts that overwrite them.
//!
//! The keyword tables were dumped from the executable (`{name, setter, address}` records, e.g.
//! the AIShip table at 0x743780, AITrack at 0x7443a0, Formation at 0x875908, Tweak at
//! 0x8a2330). Keywords are matched exactly, so lines the binary does not know (such as
//! `aiship.script`'s `BIGSHIP_*` and `FIGHTER_BANK`) have no effect, as in the original.

use hwc_formats::script::ScriptFile;
use hwc_math::fpu::{ld, st};
use hwc_math::trig::fcos;

use crate::statics::cscan::{parse_bool, scan_f32, scan_i32};
use crate::statics::DataSource;

/// The single-precision degree-to-radian factor every angle setter multiplies by.
const DEG_TO_RAD: f32 = 0.017_453_292;

/// Collision blob properties (`collBlobProperties`, all from `Tweak.script`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BlobProperties {
    pub density_low: f32,
    pub density_high: f32,
    pub start_sphere_size: f32,
    pub radius_combine_margin: f32,
    pub smallest_radius: f32,
    pub biggest_radius: f32,
    /// `bobBiggestRadiusDefault`, restored by `bobResetProperties`.
    pub biggest_radius_default: f32,
    pub overlap_factor: f32,
    /// `sqrt(overlap_factor)`, set by the overlap setter.
    pub sqrt_overlap_factor: f32,
}

/// Every run-time tunable the movement slice reads.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SimTweaks {
    // --- AIShip.script (table 0x743780, values at 0x7436a0..)
    pub velocity_scale_factor: f32,
    pub avoid_obj_scale_factor: f32,
    pub combat_avoid_obj_scale_factor: f32,
    pub probe_avoid_obj_scale_factor: f32,
    pub avoid_obj_padding_scale: f32,
    pub avoid_obj_padding_scale_big: f32,
    pub avoid_obj_row_padding_scale: f32,
    pub avoid_obj_formation_padding_scale: f32,
    pub avoid_obj_mship_milparade_padding_scale: f32,
    pub avoid_obj_r2mship_milparade_padding_scale: f32,
    pub avoid_obj_obscuredpoint_scale: f32,
    pub avoid_panic_factor: f32,
    pub avoid_spread_out_dist: f32,
    pub car_turn_power: f32,
    /// Cosine (degrees in the file).
    pub flyship_heading_accuracy: f32,
    pub flyship_attack_accuracy: f32,
    pub descend_pitch4_dist: f32,
    pub descend_pitch3_dist: f32,
    pub descend_pitch2_dist: f32,
    pub min_dist_for_fancy_descend: f32,
    /// Tangent (degrees in the file).
    pub min_angle_for_fancy_descend: f32,
    pub min_dist_to_track_heading: f32,
    /// Tangent (degrees in the file).
    pub max_angle_for_track_heading: f32,
    pub pass_right_factor: f32,
    /// Squared cosine (degrees in the file).
    pub use_pass_right_angle: f32,
    pub avoid_min_vel: f32,
    /// Derived by `AISHIP_OTHER_CALCULATIONS`: `-AVOID_MIN_VEL`.
    pub neg_avoid_min_vel: f32,
    /// Derived: `1 / DESCEND_PITCH4_DIST` and the two band reciprocals.
    pub one_over_descend_pitch4_dist: f32,
    pub one_over_descend_3_minus_4_dist: f32,
    pub one_over_descend_2_minus_3_dist: f32,
    pub do_avoid_objs: bool,

    // --- AITrack.script (table 0x7443a0)
    pub error_velocity_squared: f32,
    pub still_velocity_lo: f32,
    pub still_velocity_hi: f32,
    pub still_rot_lo: f32,
    pub still_rot_hi: f32,
    pub stabilize_z_lo: f32,
    pub stabilize_z_hi: f32,
    pub aitrackright_upaccuracy: f32,

    // --- Formation.script (table 0x875908)
    pub formation_travel_slowest_ship: bool,
    pub delta_formation_padding_scale: f32,
    pub broad_formation_padding_scale: f32,
    pub delta3d_formation_padding_scale: f32,
    pub claw_formation_padding_scale: f32,
    pub claw_formation_curvature: f32,
    pub wall_formation_padding_scale: f32,
    pub wall_formation_padding: f32,
    pub sphere_formation_padding_scale: f32,
    pub sphere_minimum_radius: f32,
    pub sphere_maximum_radius: f32,
    pub strikecraft_padding_modifier: f32,
    pub formation_error_calculate_rate: u32,
    /// Not in the table (compiled constant at 0x8a22a0).
    pub formation_error_calculate_frame: u32,
    pub formation_travelvel_maxscale: f32,
    pub formation_travelvel_minscale: f32,
    pub formation_error_big: f32,
    pub formation_error_negligible: f32,

    // --- Tactics.script
    /// `tacticsInfo.formationPadding[TightFormation, LooseFormation]`.
    pub formation_padding: [f32; 2],

    // --- Tweak.script (table 0x8a2330)
    pub ship_crazy_friction: f32,
    pub dont_drift_friction: f32,
    pub considered_still_lo: f32,
    pub considered_still_hi: f32,
    pub autostabilize_frame_kick_in: u32,
    pub verybigship_size: f32,
    pub refresh_collblob_rate: u32,
    pub refresh_collblob_frame: u32,
    pub refresh_collblob_battleping_frame: u32,
    pub blob: BlobProperties,

    /// Combat tunables (Tactics, CommandLayer, FlightMan and the combat parts of Tweak and
    /// AIShip).
    pub combat: crate::combat_tweaks::CombatTweaks,
    /// Base-building tunables (Resource.script and the base entries of Tweak.script).
    pub base: crate::base_tweaks::BaseTweaks,
    /// The military parade tables (`Parade_*.script`).
    pub parade: crate::parade::ParadeTables,
}

impl Default for SimTweaks {
    /// The values compiled into the executable (bit patterns read from its data section; the
    /// blob properties live in uninitialised memory and are all zero until `Tweak.script`).
    fn default() -> Self {
        let b = f32::from_bits;
        Self {
            velocity_scale_factor: b(0x3dcc_cccd),
            avoid_obj_scale_factor: b(0x38d1_b717),
            combat_avoid_obj_scale_factor: b(0x3727_c5ac),
            probe_avoid_obj_scale_factor: b(0x3727_c5ac),
            avoid_obj_padding_scale: b(0x3fcc_cccd),
            avoid_obj_padding_scale_big: 2.0,
            avoid_obj_row_padding_scale: 3.0,
            avoid_obj_formation_padding_scale: b(0x4023_3333),
            avoid_obj_mship_milparade_padding_scale: b(0x3fc6_6666),
            avoid_obj_r2mship_milparade_padding_scale: 0.75,
            avoid_obj_obscuredpoint_scale: b(0x3fc6_6666),
            avoid_panic_factor: b(0x3e4c_cccd),
            avoid_spread_out_dist: 100.0,
            car_turn_power: b(0x3dcc_cccd),
            flyship_heading_accuracy: b(0x3f7f_be77),
            flyship_attack_accuracy: b(0x3f7f_be77),
            descend_pitch4_dist: 0.25,
            descend_pitch3_dist: 0.5,
            descend_pitch2_dist: 0.75,
            min_dist_for_fancy_descend: 100.0,
            min_angle_for_fancy_descend: b(0x3f13_b646),
            min_dist_to_track_heading: 20.0,
            max_angle_for_track_heading: b(0x402f_ced9),
            pass_right_factor: 1.0,
            use_pass_right_angle: b(0x3f78_51ec),
            avoid_min_vel: 1.0,
            neg_avoid_min_vel: -1.0,
            one_over_descend_pitch4_dist: 0.0,
            one_over_descend_3_minus_4_dist: 0.0,
            one_over_descend_2_minus_3_dist: 0.0,
            do_avoid_objs: true,
            error_velocity_squared: b(0x3d4c_cccd),
            still_velocity_lo: -3.0,
            still_velocity_hi: 3.0,
            still_rot_lo: b(0xbe99_999a),
            still_rot_hi: b(0x3e99_999a),
            stabilize_z_lo: b(0xbdcc_cccd),
            stabilize_z_hi: b(0x3dcc_cccd),
            aitrackright_upaccuracy: b(0x3f7c_28f6),
            formation_travel_slowest_ship: true,
            delta_formation_padding_scale: 1.5,
            broad_formation_padding_scale: 1.5,
            delta3d_formation_padding_scale: 1.5,
            claw_formation_padding_scale: 1.5,
            claw_formation_curvature: b(0x3f8c_cccd),
            wall_formation_padding_scale: 2.0,
            wall_formation_padding: 20.0,
            sphere_formation_padding_scale: 4.0,
            sphere_minimum_radius: 500.0,
            sphere_maximum_radius: 3500.0,
            strikecraft_padding_modifier: b(0x3f99_999a),
            formation_error_calculate_rate: 7,
            formation_error_calculate_frame: 2,
            formation_travelvel_maxscale: 1.0,
            formation_travelvel_minscale: b(0x3f33_3333),
            formation_error_big: 10000.0,
            formation_error_negligible: 100.0,
            formation_padding: [0.0; 2],
            ship_crazy_friction: b(0x3f73_3333),
            dont_drift_friction: b(0x3f7d_70a4),
            considered_still_lo: -5.0,
            considered_still_hi: 5.0,
            autostabilize_frame_kick_in: 8,
            verybigship_size: 1500.0,
            refresh_collblob_rate: 7,
            refresh_collblob_frame: 6,
            refresh_collblob_battleping_frame: 4,
            blob: BlobProperties {
                density_low: 0.0,
                density_high: 0.0,
                start_sphere_size: 0.0,
                radius_combine_margin: 0.0,
                smallest_radius: 0.0,
                biggest_radius: 0.0,
                biggest_radius_default: 0.0,
                overlap_factor: 0.0,
                sqrt_overlap_factor: 0.0,
            },
            combat: crate::combat_tweaks::CombatTweaks::default(),
            base: crate::base_tweaks::BaseTweaks::default(),
            parade: crate::parade::ParadeTables::default(),
        }
    }
}

/// How a keyword converts its value (the original's script setters).
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Conv {
    /// `scriptSetReal32CB`.
    Real,
    /// `scriptSetCosAngCB`: `FCOS(deg * DEG_TO_RAD)`.
    CosAng,
    /// `scriptSetCosAngSqrCB`: the cosine squared in a register, then stored.
    CosAngSqr,
    /// `scriptSetTanAngCB`: `FPTAN(deg * DEG_TO_RAD)`.
    TanAng,
    /// `scriptSetBool` / `scriptSetBool8`.
    Bool,
    /// `scriptSetSdwordCB` / `scriptSetUdwordCB`.
    Int,
    /// `scriptSetBlobPropertyOverlap`: the factor and its square root.
    BlobOverlap,
    /// `scriptSetBlobBiggestRadius`: the radius and its default.
    BlobBiggest,
}

enum Target {
    F(fn(&mut SimTweaks) -> &mut f32),
    B(fn(&mut SimTweaks) -> &mut bool),
    U(fn(&mut SimTweaks) -> &mut u32),
    Special,
}

use Conv::*;
use Target::*;

/// (file, keyword, conversion, target) in the order the original reads the files.
const BINDINGS: &[(&str, &str, Conv, Target)] = &[
    ("AIShip.script", "VELOCITY_SCALE_FACTOR", Real, F(|t| &mut t.velocity_scale_factor)),
    ("AIShip.script", "AVOID_OBJ_SCALE_FACTOR", Real, F(|t| &mut t.avoid_obj_scale_factor)),
    ("AIShip.script", "COMBAT_AVOID_OBJ_SCALE_FACTOR", Real, F(|t| &mut t.combat_avoid_obj_scale_factor)),
    ("AIShip.script", "PROBE_AVOID_OBJ_SCALE_FACTOR", Real, F(|t| &mut t.probe_avoid_obj_scale_factor)),
    ("AIShip.script", "AVOID_OBJ_PADDING_SCALE", Real, F(|t| &mut t.avoid_obj_padding_scale)),
    ("AIShip.script", "AVOID_OBJ_PADDING_SCALE_BIG", Real, F(|t| &mut t.avoid_obj_padding_scale_big)),
    ("AIShip.script", "AVOID_OBJ_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.avoid_obj_formation_padding_scale)),
    ("AIShip.script", "AVOID_OBJ_MSHIP_MILPARADE_PADDING_SCALE", Real, F(|t| &mut t.avoid_obj_mship_milparade_padding_scale)),
    ("AIShip.script", "AVOID_OBJ_R2MSHIP_MILPARADE_PADDING_SCALE", Real, F(|t| &mut t.avoid_obj_r2mship_milparade_padding_scale)),
    ("AIShip.script", "AVOID_OBJ_OBSCUREDPOINT_SCALE", Real, F(|t| &mut t.avoid_obj_obscuredpoint_scale)),
    ("AIShip.script", "AVOID_OBJ_ROW_PADDING_SCALE", Real, F(|t| &mut t.avoid_obj_row_padding_scale)),
    ("AIShip.script", "AVOID_PANIC_FACTOR", Real, F(|t| &mut t.avoid_panic_factor)),
    ("AIShip.script", "AVOID_SPREAD_OUT_DIST", Real, F(|t| &mut t.avoid_spread_out_dist)),
    ("AIShip.script", "CAR_TURN_POWER", Real, F(|t| &mut t.car_turn_power)),
    ("AIShip.script", "FLYSHIP_HEADINGACCURACY", CosAng, F(|t| &mut t.flyship_heading_accuracy)),
    ("AIShip.script", "FLYSHIP_ATTACKACCURACY", CosAng, F(|t| &mut t.flyship_attack_accuracy)),
    ("AIShip.script", "DO_AVOID_OBJS", Bool, B(|t| &mut t.do_avoid_objs)),
    ("AIShip.script", "DESCEND_PITCH4_DIST", Real, F(|t| &mut t.descend_pitch4_dist)),
    ("AIShip.script", "DESCEND_PITCH3_DIST", Real, F(|t| &mut t.descend_pitch3_dist)),
    ("AIShip.script", "DESCEND_PITCH2_DIST", Real, F(|t| &mut t.descend_pitch2_dist)),
    ("AIShip.script", "MIN_DIST_FOR_FANCY_DESCEND", Real, F(|t| &mut t.min_dist_for_fancy_descend)),
    ("AIShip.script", "MIN_ANGLE_FOR_FANCY_DESCEND", TanAng, F(|t| &mut t.min_angle_for_fancy_descend)),
    ("AIShip.script", "MIN_DIST_TO_TRACK_HEADING", Real, F(|t| &mut t.min_dist_to_track_heading)),
    ("AIShip.script", "MAX_ANGLE_FOR_TRACK_HEADING", TanAng, F(|t| &mut t.max_angle_for_track_heading)),
    ("AIShip.script", "PASS_RIGHT_FACTOR", Real, F(|t| &mut t.pass_right_factor)),
    ("AIShip.script", "USE_PASS_RIGHT_ANGLE", CosAngSqr, F(|t| &mut t.use_pass_right_angle)),
    ("AIShip.script", "AVOID_MIN_VEL", Real, F(|t| &mut t.avoid_min_vel)),
    ("AITrack.script", "ErrorVelocitySquared", Real, F(|t| &mut t.error_velocity_squared)),
    ("AITrack.script", "StillVelocityLo", Real, F(|t| &mut t.still_velocity_lo)),
    ("AITrack.script", "StillVelocityHi", Real, F(|t| &mut t.still_velocity_hi)),
    ("AITrack.script", "StillRotLo", Real, F(|t| &mut t.still_rot_lo)),
    ("AITrack.script", "StillRotHi", Real, F(|t| &mut t.still_rot_hi)),
    ("AITrack.script", "STABILIZE_Z_LO", Real, F(|t| &mut t.stabilize_z_lo)),
    ("AITrack.script", "STABILIZE_Z_HI", Real, F(|t| &mut t.stabilize_z_hi)),
    ("AITrack.script", "AITRACKRIGHT_UPACCURACY", CosAng, F(|t| &mut t.aitrackright_upaccuracy)),
    ("Formation.script", "FORMATION_TRAVEL_SLOWEST_SHIP", Bool, B(|t| &mut t.formation_travel_slowest_ship)),
    ("Formation.script", "DELTA_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.delta_formation_padding_scale)),
    ("Formation.script", "BROAD_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.broad_formation_padding_scale)),
    ("Formation.script", "DELTA3D_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.delta3d_formation_padding_scale)),
    ("Formation.script", "CLAW_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.claw_formation_padding_scale)),
    ("Formation.script", "CLAW_FORMATION_CURVATURE", Real, F(|t| &mut t.claw_formation_curvature)),
    ("Formation.script", "WALL_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.wall_formation_padding_scale)),
    ("Formation.script", "WALL_FORMATION_PADDING", Real, F(|t| &mut t.wall_formation_padding)),
    ("Formation.script", "SPHERE_FORMATION_PADDING_SCALE", Real, F(|t| &mut t.sphere_formation_padding_scale)),
    ("Formation.script", "SPHERE_MINIMUM_RADIUS", Real, F(|t| &mut t.sphere_minimum_radius)),
    ("Formation.script", "SPHERE_MAXIMUM_RADIUS", Real, F(|t| &mut t.sphere_maximum_radius)),
    ("Formation.script", "STRIKECRAFT_PADDING_MODIFIER", Real, F(|t| &mut t.strikecraft_padding_modifier)),
    ("Formation.script", "FORMATION_ERROR_CALCULATE_RATE", Int, U(|t| &mut t.formation_error_calculate_rate)),
    ("Formation.script", "FORMATION_TRAVELVEL_MAXSCALE", Real, F(|t| &mut t.formation_travelvel_maxscale)),
    ("Formation.script", "FORMATION_TRAVELVEL_MINSCALE", Real, F(|t| &mut t.formation_travelvel_minscale)),
    ("Formation.script", "FORMATION_ERROR_BIG", Real, F(|t| &mut t.formation_error_big)),
    ("Formation.script", "FORMATION_ERROR_NEGLIGIBLE", Real, F(|t| &mut t.formation_error_negligible)),
    ("Tactics.script", "formationPadding[TightFormation]", Real, F(|t| &mut t.formation_padding[0])),
    ("Tactics.script", "formationPadding[LooseFormation]", Real, F(|t| &mut t.formation_padding[1])),
    ("Tweak.script", "AUTOSTABILIZE_FRAME_KICK_IN", Int, U(|t| &mut t.autostabilize_frame_kick_in)),
    ("Tweak.script", "VERYBIGSHIP_SIZE", Real, F(|t| &mut t.verybigship_size)),
    ("Tweak.script", "SHIP_CRAZY_FRICTION", Real, F(|t| &mut t.ship_crazy_friction)),
    ("Tweak.script", "DONT_DRIFT_FRICTION", Real, F(|t| &mut t.dont_drift_friction)),
    ("Tweak.script", "CONSIDERED_STILL_LO", Real, F(|t| &mut t.considered_still_lo)),
    ("Tweak.script", "CONSIDERED_STILL_HI", Real, F(|t| &mut t.considered_still_hi)),
    ("Tweak.script", "univBobDensityLow", Real, F(|t| &mut t.blob.density_low)),
    ("Tweak.script", "univBobDensityHigh", Real, F(|t| &mut t.blob.density_high)),
    ("Tweak.script", "univBobStartSphereSize", Real, F(|t| &mut t.blob.start_sphere_size)),
    ("Tweak.script", "univBobRadiusCombineMargin", Real, F(|t| &mut t.blob.radius_combine_margin)),
    ("Tweak.script", "univBobOverlapFactor", BlobOverlap, Special),
    ("Tweak.script", "univBobSmallestRadius", Real, F(|t| &mut t.blob.smallest_radius)),
    ("Tweak.script", "univBobBiggestRadius", BlobBiggest, Special),
    ("Tweak.script", "REFRESH_COLLBLOB_RATE", Int, U(|t| &mut t.refresh_collblob_rate)),
    ("Tweak.script", "REFRESH_COLLBLOB_FRAME", Int, U(|t| &mut t.refresh_collblob_frame)),
    ("Tweak.script", "REFRESH_COLLBLOB_BATTLEPING_FRAME", Int, U(|t| &mut t.refresh_collblob_battleping_frame)),
];

impl SimTweaks {
    /// Compiled defaults overwritten by the tweak scripts found in `src`, then the derived
    /// values (`AISHIP_OTHER_CALCULATIONS`).
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = SimTweaks::default();
        let mut files: Vec<&str> = BINDINGS.iter().map(|b| b.0).collect();
        files.dedup();
        for file in files {
            let Some(script) = src.read(file).and_then(|b| ScriptFile::parse_bytes(&b).ok()) else {
                continue;
            };
            t.apply(file, &script);
        }
        t.precalculate();
        t.combat = crate::combat_tweaks::CombatTweaks::load(src);
        t.base = crate::base_tweaks::BaseTweaks::load(src);
        t.parade = crate::parade::ParadeTables::load(src);
        t
    }

    /// Defaults plus the derived values, without any script (for tests).
    pub fn compiled() -> Self {
        let mut t = Self::default();
        t.precalculate();
        t
    }

    /// Applies the lines of one tweak file.
    pub fn apply(&mut self, file: &str, script: &ScriptFile) {
        for e in script.entries() {
            for (_, _, conv, target) in BINDINGS.iter().filter(|b| b.0 == file && b.1 == e.key) {
                self.set(*conv, target, &e.raw);
            }
        }
    }

    fn set(&mut self, conv: Conv, target: &Target, raw: &str) {
        let angle = |v: f32| ld(v) * ld(DEG_TO_RAD);
        match (conv, target) {
            (Bool, B(f)) => *f(self) = parse_bool(raw),
            (Int, U(f)) => {
                if let Some(v) = scan_i32(raw) {
                    *f(self) = v as u32;
                }
            }
            (BlobOverlap, _) => {
                if let Some(v) = scan_f32(raw) {
                    self.blob.overlap_factor = v;
                    self.blob.sqrt_overlap_factor = st(ld(v).sqrt());
                }
            }
            (BlobBiggest, _) => {
                if let Some(v) = scan_f32(raw) {
                    self.blob.biggest_radius = v;
                    self.blob.biggest_radius_default = v;
                }
            }
            (conv, F(f)) => {
                if let Some(v) = scan_f32(raw) {
                    *f(self) = match conv {
                        Real => v,
                        CosAng => st(fcos(angle(v))),
                        CosAngSqr => {
                            let c = fcos(angle(v));
                            st(c * c)
                        }
                        // FPTAN is not emulated bit for bit; the double tangent rounds to the
                        // same float except in pathological cases.
                        TanAng => st(angle(v).tan()),
                        _ => v,
                    };
                }
            }
            _ => {}
        }
    }

    /// `scriptAIShipPrecalculate` (`AISHIP_OTHER_CALCULATIONS`).
    pub fn precalculate(&mut self) {
        self.one_over_descend_pitch4_dist = st(1.0 / ld(self.descend_pitch4_dist));
        self.one_over_descend_3_minus_4_dist =
            st(1.0 / (ld(self.descend_pitch3_dist) - ld(self.descend_pitch4_dist)));
        self.one_over_descend_2_minus_3_dist =
            st(1.0 / (ld(self.descend_pitch2_dist) - ld(self.descend_pitch3_dist)));
        self.neg_avoid_min_vel = -self.avoid_min_vel;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angle_setters_match_binary_conventions() {
        let mut t = SimTweaks::default();
        let s = ScriptFile::parse("FLYSHIP_HEADINGACCURACY 2.0\nUSE_PASS_RIGHT_ANGLE 10.0\nMIN_ANGLE_FOR_FANCY_DESCEND 30.0\nBIGSHIP_BANK 3\n");
        t.apply("AIShip.script", &s);
        assert!((t.flyship_heading_accuracy - 0.999_390_8).abs() < 1e-6);
        assert!((t.use_pass_right_angle - 0.969_846_3).abs() < 1e-6);
        assert!((t.min_angle_for_fancy_descend - 0.577_350_3).abs() < 1e-6);
        // The compiled-in defaults equal what the original's HW1-era values give.
        let d = SimTweaks::default();
        assert_eq!(d.min_angle_for_fancy_descend.to_bits(), 0x3f13_b646);
    }

    #[test]
    fn blob_setters() {
        let mut t = SimTweaks::default();
        let s = ScriptFile::parse("univBobOverlapFactor 1.3\nunivBobBiggestRadius 15000\n");
        t.apply("Tweak.script", &s);
        assert_eq!(t.blob.overlap_factor, 1.3);
        assert_eq!(t.blob.sqrt_overlap_factor, (1.3f64).sqrt() as f32);
        assert_eq!(t.blob.biggest_radius_default, 15000.0);
    }
}
