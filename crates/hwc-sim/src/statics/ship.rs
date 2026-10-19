//! Per-ship-type statics (`ShipStaticInfo`, 0x458 bytes in Cataclysm), loaded the way
//! `InitStatShipInfo` does it. Field comments give the offset in the original structure and the
//! keyword that sets the field; fields without a keyword are derived (see
//! `docs/sim/static-info.md`).

use hwc_formats::script::ScriptFile;

use super::cscan::{parse_bool, scan_f32, scan_i32, scan_rgb};
use super::dock::DockStaticInfo;
use super::enums::{ShipClass, ShipRace, ShipType};
use super::gun::{self, GunStatic};
use super::header::{GuidanceInfo, HeaderKeys, HealthInfo, StaticHeader};
use hwc_math::Vector;

use super::v3;
use super::navlight::{self, NavLightStatic};
use super::Loader;

/// Engine trail settings for one of the four trails (`trailStyle`, `trailWidth2`, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct TrailSettings {
    /// +0x324 `trailStyle` (default 1).
    pub style: i32,
    /// +0x334.. `trailWidth`, `trailHeight`, `trailAngle`, `trailRibbonAdjust`, `trailLength`,
    /// `trailScaleCap`, `trailSpriteRadius` (default -1 = use the trail module's default).
    pub width: f32,
    pub height: f32,
    pub angle: f32,
    pub ribbon_adjust: f32,
    pub length: f32,
    pub scale_cap: f32,
    pub sprite_radius: f32,
    /// +0x3a4 `trailSpriteOffset` (default 0).
    pub sprite_offset: f32,
}

impl Default for TrailSettings {
    fn default() -> Self {
        Self {
            style: 1,
            width: -1.0,
            height: -1.0,
            angle: -1.0,
            ribbon_adjust: -1.0,
            length: -1.0,
            scale_cap: -1.0,
            sprite_radius: -1.0,
            sprite_offset: 0.0,
        }
    }
}

/// A resource-collection or repair nozzle (`Gun0` / `Rep0` mex chunk).
#[derive(Debug, Clone, PartialEq)]
pub struct Nozzle {
    pub position: Vector,
    /// Normalised direction; `None` for the repair nozzle, which has none.
    pub normal: Option<Vector>,
}

/// Static information for one ship type.
#[derive(Debug, Clone)]
pub struct ShipStaticInfo {
    pub header: StaticHeader,
    pub health: HealthInfo,
    pub guidance: GuidanceInfo,
    /// +0xf8 / +0xfc / +0x100. AI banking and pitching (by class; see `aishipPrecalcInfo`).
    pub sinbank: f32,
    pub pitchturn: f32,
    pub pitchdescend: f32,
    /// +0x104.
    pub nav_lights: Option<Vec<NavLightStatic>>,
    /// +0x198 / +0x1a4 / +0x1b0, per tactic (evasive, neutral, aggressive). With guns these are
    /// the longest and shortest gun range (all three tactics alike); without guns the
    /// `bulletRange[n]`/`minBulletRange[n]` keywords, or `ASTEROID_HARVEST_RANGE` for resource
    /// collectors.
    pub bullet_range_squared: [f32; 3],
    pub bullet_range: [f32; 3],
    pub min_bullet_range: [f32; 3],
    /// +0x1bc `blastRadiusShockWave` (default `BLAST_RADIUS_MULTIPLE_DEFAULT`).
    pub blast_radius_shock_wave: f32,
    /// +0x1c0 `blastRadiusDamage`.
    pub blast_radius_damage: f32,
    /// +0x1c4 `passiveRetaliateZone` (default `RETALIATE_ZONE`; negative means off, stored 0).
    pub passive_retaliate_zone: f32,
    /// +0x1c8 `collSideModifiers[TRANS_*]` (damage multipliers by side; capital ships default to
    /// `capitalSideModifiers`).
    pub coll_side_modifiers: [f32; 6],
    /// +0x1e0.
    pub ship_type: ShipType,
    /// +0x1e4.
    pub ship_race: ShipRace,
    /// +0x1e8 `shipclass` (`None` when missing or unrecognised; the original stores 0 or -1).
    pub ship_class: Option<ShipClass>,
    /// Raw `shipclass` value when it did not name a class.
    pub ship_class_raw: Option<String>,
    /// +0x1ec. Per-type capability bits set from the ship type (meaning of the individual bits
    /// is only partly known; see the docs).
    pub capability_flags: u32,
    /// +0x1f0 `externalBuild`.
    pub external_build: bool,
    /// +0x1f4 `forceFieldProtectionDistanceScale` (values up to 1 are replaced by 2).
    pub force_field_protection_distance_scale: f32,
    /// +0x1f8 `upgradeTimeModifier` (default 1).
    pub upgrade_time_modifier: f32,
    /// +0x1fc..+0x208.
    pub can_build_ships: bool,
    pub can_receive_ships: bool,
    pub can_receive_resources: bool,
    pub can_receive_ships_permanently: bool,
    pub can_receive_ships_for_retire: bool,
    /// Derived: any of the four `canReceive*` above.
    pub can_receive_something: bool,
    pub can_build_big_ships: bool,
    pub can_target_multiple_targets: bool,
    pub rotate_to_retaliate: bool,
    /// `canReceiveFighters`, `canReceiveCorvettes`, `canReceiveResourceCollectors`,
    /// `canReceiveOtherNonCapitalShips`.
    pub can_receive_these_ships: [bool; 4],
    /// +0x20c `passiveAttackPenaltyExempt`.
    pub passive_attack_penalty_exempt: bool,
    /// +0x210 `specialActivateIsContinuous`.
    pub special_activate_is_continuous: bool,
    /// +0x211 `canSpecialBandBoxFriendlies`.
    pub can_special_band_box_friendlies: bool,
    /// +0x212 / +0x213: Cataclysm flags without a keyword, initialised to 1 and 0.
    pub unknown_flag_212: bool,
    pub unknown_flag_213: bool,
    /// +0x214 `canSingleClickSpecialActivate`.
    pub can_single_click_special_activate: bool,
    /// +0x216 / +0x218 `maxDockableFighters`, `maxDockableCorvettes`.
    pub max_dockable_fighters: i16,
    pub max_dockable_corvettes: i16,
    /// +0x21c / +0x220 `canHandleNumShipsDocking`, `canHandleNumShipsDepositingRU`.
    pub can_handle_num_ships_docking: i32,
    pub can_handle_num_ships_depositing_ru: i32,
    /// +0x224 `repairBeamCapable`.
    pub repair_beam_capable: bool,
    /// +0x228..+0x234 `healthPerSecond`, `CapitalDistanceRepairStart2`,
    /// `CapitalDistanceRepairStart`, `AngleDotProdThreshold`.
    pub health_per_second: f32,
    pub capital_distance_repair_start2: f32,
    pub capital_distance_repair_start: f32,
    pub angle_dot_prod_threshold: f32,
    /// +0x238 `buildCost` (resource units).
    pub build_cost: i32,
    /// +0x23c `buildTime` in frames (seconds times 16).
    pub build_time: i32,
    /// +0x240 `fowRadius`.
    pub fow_radius: f32,
    /// +0x244..+0x24c `svManeuverability`, `svCoverage`, `svFirePower` (unused by Cataclysm's
    /// files).
    pub sv_maneuverability: i32,
    pub sv_coverage: i32,
    pub sv_fire_power: i32,
    /// +0x250 `RKOEatMeTime`.
    pub rko_eat_me_time: f32,
    /// +0x254..+0x260 `resourcesAtOneTime`, `maxresources`, `harvestRate`, `harvestAmount`.
    pub resources_at_one_time: i32,
    pub max_resources: i32,
    pub harvest_rate: i32,
    pub harvest_amount: i32,
    /// +0x264 `groupSize`.
    pub group_size: i32,
    /// +0x278..+0x284 `repairTime`, `repairDamage`, `repairCombatTime`, `repairCombatDamage`.
    /// The combat values default to the plain time and to 20% of the plain damage.
    pub repair_time: f32,
    pub repair_damage: f32,
    pub repair_combat_time: f32,
    pub repair_combat_damage: f32,
    /// +0x28c / +0x290 `repairOtherShipRate`, `repairResourceCollectorRate`.
    pub repair_other_ship_rate: f32,
    pub repair_resource_collector_rate: f32,
    /// +0x294 / +0x298 `clearanceDistance`, `clearanceDirection`.
    pub clearance_distance: f32,
    pub clearance_direction: i32,
    /// +0x29c `dockShipRange`.
    pub dock_ship_range: f32,
    /// +0x2a0 `formationPaddingModifier` (default `STRIKECRAFT_PADDING_MODIFIER` for strike
    /// craft, else 1).
    pub formation_padding_modifier: f32,
    /// +0x2a4 `cannotForceAttackIfOwnShip`.
    pub cannot_force_attack_if_own_ship: bool,
    /// +0x2a8 `isCapitalShip`.
    pub is_capital_ship: bool,
    /// +0x2ac `cantMoveAndAttack`.
    pub cant_move_and_attack: bool,
    /// +0x2b0. Engine nozzle positions from the `EngN` mex chunks (zero when absent).
    pub engine_nozzle_offset: [Vector; 4],
    /// Mesh object each nozzle follows (`EngN` chunk, Cataclysm).
    pub engine_nozzle_attached_to: [Option<String>; 4],
    /// +0x308. True when the mex has an `Eng1` chunk.
    pub multiple_engines: bool,
    /// +0x30c `N-LIPS`: minimum size cap.
    pub scale_cap: f32,
    /// +0x318 / +0x31c `madGunOpenDamagedHealthThreshold`, `madWingOpenDamagedHealthThreshold`.
    pub mad_gun_open_damaged_health_threshold: f32,
    pub mad_wing_open_damaged_health_threshold: f32,
    /// +0x320 `repairDroids`.
    pub repair_droids: i32,
    /// +0x324..+0x3b0.
    pub trails: [TrailSettings; 4],
    /// +0x3b4 `minimumZoomDistance`.
    pub minimum_zoom_distance: f32,
    /// +0x3b8 `renderlistFade`.
    pub renderlist_fade: f32,
    /// +0x3bc `renderlistLimit`, squared.
    pub renderlist_limit_sqr: f32,
    /// +0x3c0 / +0x3c4 `dockLightNear` (0), `dockLightFar` (0.1).
    pub dock_light_near: f32,
    pub dock_light_far: f32,
    /// +0x3c8 `dockLightColor` RGBA (default 120, 194, 255).
    pub dock_light_color: [u8; 4],
    /// +0x3cc `hyperspaceColor` RGBA (default black = effect default).
    pub hyperspace_color: [u8; 4],
    /// +0x3d0 `hyperspaceBoxScale` (default 1).
    pub hyperspace_box_scale: f32,
    /// +0x3d4 `leechRUsPerSecond` (default 1).
    pub leech_rus_per_second: i32,
    /// +0x3d8 `leechHPsPerSecond` (default 20).
    pub leech_hps_per_second: f32,
    /// +0x3dc `beastAttackCascadeCost` (default 1).
    pub beast_attack_cascade_cost: f32,
    /// +0x3e0 `beastAttacksRequired` (default -1).
    pub beast_attacks_required: i32,
    /// +0x3e4 `beastAttackConvertTo` (default bWorker; `None` if the name was not a ship type).
    pub beast_attack_convert_to: Option<ShipType>,
    /// +0x3e8 `Overlay_Selection_Scalar` (default 1).
    pub overlay_selection_scalar: f32,
    /// +0x3ec.
    pub guns: Option<Vec<GunStatic>>,
    /// +0x3f0.
    pub dock: Option<DockStaticInfo>,
    /// +0x3f8. For harvesters (class Resource with `harvestAmount` > 0).
    pub res_nozzle: Option<Nozzle>,
    /// +0x3fc. For `repairBeamCapable` ships.
    pub repair_nozzle: Option<Nozzle>,
    /// +0x408 `bUseRayTriangleIntersectionForHitLocation` (default true).
    pub use_ray_triangle_intersection_for_hit_location: bool,
    /// +0x40c `linkDistance`.
    pub link_distance: f32,
    /// +0x410 `supportUnits`.
    pub support_units: i32,
    /// +0x414 `providesSU`.
    pub provides_su: i32,
    /// +0x418..+0x428 `RamDamage`, `cuttingBeamDamagePerSecond`,
    /// `cuttingBeamUpgradedDamagePerSecond`, `RamPushTime`, `RamWaitAfterPushTime`.
    pub ram_damage: f32,
    pub cutting_beam_damage_per_second: f32,
    pub cutting_beam_upgraded_damage_per_second: f32,
    pub ram_push_time: f32,
    pub ram_wait_after_push_time: f32,
    /// +0x430..+0x438 `TimeShockedByEMP`, `EMP_Range`, `EMP_Max_Combine_Distance`.
    pub time_shocked_by_emp: f32,
    pub emp_range: f32,
    pub emp_max_combine_distance: f32,
    /// +0x43c `UpgradeByDocking`.
    pub upgrade_by_docking: bool,
    /// +0x440 `pilotViewOffsetX/Y/Z`.
    pub pilot_view_offset: Vector,
    /// +0x44c. Health after `GLOBAL_SHIP_HEALTH_MODIFIER`, before the single-player factor.
    pub base_max_health: f32,
    /// +0x454 `RKOGetsMissiles`.
    pub rko_gets_missiles: i32,
    /// Source file.
    pub file: String,
    /// Ship-file lines no generic table binds, in file order (outside gun blocks). These are read
    /// by the per-type `CustShipStaticInit` handlers of the original, which are not reproduced
    /// yet.
    pub custom_entries: Vec<(String, String)>,
    /// Gun-block keywords the gun table does not bind.
    pub unbound_gun_keywords: Vec<String>,
}

impl ShipStaticInfo {
    pub(crate) fn new(ship_type: ShipType, file: String) -> Self {
        Self {
            header: StaticHeader::default(),
            health: HealthInfo::default(),
            guidance: GuidanceInfo::default(),
            sinbank: 0.0,
            pitchturn: 0.0,
            pitchdescend: 0.0,
            nav_lights: None,
            bullet_range_squared: [0.0; 3],
            bullet_range: [0.0; 3],
            min_bullet_range: [0.0; 3],
            blast_radius_shock_wave: 0.0,
            blast_radius_damage: 0.0,
            passive_retaliate_zone: 0.0,
            coll_side_modifiers: [0.0; 6],
            ship_type,
            ship_race: ship_type.race(),
            ship_class: Some(ShipClass::Mothership),
            ship_class_raw: None,
            capability_flags: 0,
            external_build: false,
            force_field_protection_distance_scale: -1.0,
            upgrade_time_modifier: 1.0,
            can_build_ships: false,
            can_receive_ships: false,
            can_receive_resources: false,
            can_receive_ships_permanently: false,
            can_receive_ships_for_retire: false,
            can_receive_something: false,
            can_build_big_ships: false,
            can_target_multiple_targets: false,
            rotate_to_retaliate: false,
            can_receive_these_ships: [false; 4],
            passive_attack_penalty_exempt: false,
            special_activate_is_continuous: false,
            can_special_band_box_friendlies: false,
            unknown_flag_212: true,
            unknown_flag_213: false,
            can_single_click_special_activate: false,
            max_dockable_fighters: 0,
            max_dockable_corvettes: 0,
            can_handle_num_ships_docking: 0,
            can_handle_num_ships_depositing_ru: 0,
            repair_beam_capable: false,
            health_per_second: 0.0,
            capital_distance_repair_start2: 0.0,
            capital_distance_repair_start: 0.0,
            angle_dot_prod_threshold: 0.0,
            build_cost: 0,
            build_time: 0,
            fow_radius: 0.0,
            sv_maneuverability: 0,
            sv_coverage: 0,
            sv_fire_power: 0,
            rko_eat_me_time: 0.0,
            resources_at_one_time: 0,
            max_resources: 0,
            harvest_rate: 0,
            harvest_amount: 0,
            group_size: 0,
            repair_time: 0.0,
            repair_damage: 0.0,
            repair_combat_time: 0.0,
            repair_combat_damage: 0.0,
            repair_other_ship_rate: 0.0,
            repair_resource_collector_rate: 0.0,
            clearance_distance: 0.0,
            clearance_direction: 0,
            dock_ship_range: 0.0,
            formation_padding_modifier: 1.0,
            cannot_force_attack_if_own_ship: false,
            is_capital_ship: false,
            cant_move_and_attack: false,
            engine_nozzle_offset: [Vector::ZERO; 4],
            engine_nozzle_attached_to: Default::default(),
            multiple_engines: false,
            scale_cap: 0.0,
            mad_gun_open_damaged_health_threshold: 0.0,
            mad_wing_open_damaged_health_threshold: 0.0,
            repair_droids: 0,
            trails: Default::default(),
            minimum_zoom_distance: 0.0,
            renderlist_fade: 0.0,
            renderlist_limit_sqr: 0.0,
            dock_light_near: 0.0,
            dock_light_far: f32::from_bits(0x3dcc_cccd),
            dock_light_color: [120, 194, 255, 255],
            hyperspace_color: [0, 0, 0, 255],
            hyperspace_box_scale: 1.0,
            leech_rus_per_second: 1,
            leech_hps_per_second: 20.0,
            beast_attack_cascade_cost: 1.0,
            beast_attacks_required: -1,
            beast_attack_convert_to: Some(ShipType::bWorker),
            overlay_selection_scalar: 1.0,
            guns: None,
            dock: None,
            res_nozzle: None,
            repair_nozzle: None,
            use_ray_triangle_intersection_for_hit_location: true,
            link_distance: 0.0,
            support_units: 0,
            provides_su: 0,
            ram_damage: 0.0,
            cutting_beam_damage_per_second: 0.0,
            cutting_beam_upgraded_damage_per_second: 0.0,
            ram_push_time: 0.0,
            ram_wait_after_push_time: 0.0,
            time_shocked_by_emp: 0.0,
            emp_range: 0.0,
            emp_max_combine_distance: 0.0,
            upgrade_by_docking: false,
            pilot_view_offset: Vector::ZERO,
            base_max_health: 0.0,
            rko_gets_missiles: 0,
            file,
            custom_entries: Vec::new(),
            unbound_gun_keywords: Vec::new(),
        }
    }

    /// The ship's own keyword table (everything except the header, health and guidance blocks).
    fn apply(&mut self, key: &str, raw: &str) -> bool {
        let f = |t: &mut f32| {
            if let Some(v) = scan_f32(raw) {
                *t = v;
            }
        };
        let i = |t: &mut i32| {
            if let Some(v) = scan_i32(raw) {
                *t = v;
            }
        };
        let b = |t: &mut bool| *t = parse_bool(raw);
        let rgb = |t: &mut [u8; 4]| {
            if let Some(c) = scan_rgb(raw) {
                *t = c;
            }
        };
        if let Some(rest) = key.strip_prefix("bulletRange[") {
            return match rest {
                "0]" | "1]" | "2]" => {
                    f(&mut self.bullet_range[usize::from(rest.as_bytes()[0] - b'0')]);
                    true
                }
                _ => false,
            };
        }
        if let Some(rest) = key.strip_prefix("minBulletRange[") {
            return match rest {
                "0]" | "1]" | "2]" => {
                    f(&mut self.min_bullet_range[usize::from(rest.as_bytes()[0] - b'0')]);
                    true
                }
                _ => false,
            };
        }
        if let Some(side) = key.strip_prefix("collSideModifiers[") {
            let idx = ["TRANS_UP]", "TRANS_DOWN]", "TRANS_RIGHT]", "TRANS_LEFT]", "TRANS_FORWARD]", "TRANS_BACK]"]
                .iter()
                .position(|s| *s == side);
            return match idx {
                Some(n) => {
                    f(&mut self.coll_side_modifiers[n]);
                    true
                }
                None => false,
            };
        }
        if let Some((field, n)) = trail_key(key) {
            let t = &mut self.trails[n];
            match field {
                "trailStyle" => i(&mut t.style),
                "trailWidth" => f(&mut t.width),
                "trailHeight" => f(&mut t.height),
                "trailAngle" => f(&mut t.angle),
                "trailRibbonAdjust" => f(&mut t.ribbon_adjust),
                "trailLength" => f(&mut t.length),
                "trailScaleCap" => f(&mut t.scale_cap),
                "trailSpriteRadius" => f(&mut t.sprite_radius),
                _ => f(&mut t.sprite_offset),
            }
            return true;
        }
        match key {
            "blastRadiusShockWave" => f(&mut self.blast_radius_shock_wave),
            "blastRadiusDamage" => f(&mut self.blast_radius_damage),
            "passiveRetaliateZone" => f(&mut self.passive_retaliate_zone),
            "shipclass" => {
                self.ship_class = ShipClass::from_name(raw);
                self.ship_class_raw = self.ship_class.is_none().then(|| raw.to_owned());
            }
            "rotateToRetaliate" => b(&mut self.rotate_to_retaliate),
            "canTargetMultipleTargets" => b(&mut self.can_target_multiple_targets),
            "canBuildShips" => b(&mut self.can_build_ships),
            "canReceiveShips" => b(&mut self.can_receive_ships),
            "canReceiveResources" => b(&mut self.can_receive_resources),
            "canReceiveShipsPermanently" => b(&mut self.can_receive_ships_permanently),
            "canReceiveShipsForRetire" => b(&mut self.can_receive_ships_for_retire),
            "canBuildBigShips" => b(&mut self.can_build_big_ships),
            "canReceiveFighters" => b(&mut self.can_receive_these_ships[0]),
            "canReceiveCorvettes" => b(&mut self.can_receive_these_ships[1]),
            "canReceiveResourceCollectors" => b(&mut self.can_receive_these_ships[2]),
            "canReceiveOtherNonCapitalShips" => b(&mut self.can_receive_these_ships[3]),
            "canHandleNumShipsDocking" => i(&mut self.can_handle_num_ships_docking),
            "canHandleNumShipsDepositingRU" => i(&mut self.can_handle_num_ships_depositing_ru),
            "svManeuverability" => i(&mut self.sv_maneuverability),
            "svCoverage" => i(&mut self.sv_coverage),
            "svFirePower" => i(&mut self.sv_fire_power),
            "maxDockableFighters" => {
                if let Some(v) = scan_i32(raw) {
                    self.max_dockable_fighters = v as i16;
                }
            }
            "maxDockableCorvettes" => {
                if let Some(v) = scan_i32(raw) {
                    self.max_dockable_corvettes = v as i16;
                }
            }
            "specialActivateIsContinuous" => b(&mut self.special_activate_is_continuous),
            "canSpecialBandBoxFriendlies" => b(&mut self.can_special_band_box_friendlies),
            "canSingleClickSpecialActivate" => b(&mut self.can_single_click_special_activate),
            "buildCost" => i(&mut self.build_cost),
            "supportUnits" => i(&mut self.support_units),
            "providesSU" => i(&mut self.provides_su),
            "TimeShockedByEMP" => f(&mut self.time_shocked_by_emp),
            "EMP_Range" => f(&mut self.emp_range),
            "EMP_Max_Combine_Distance" => f(&mut self.emp_max_combine_distance),
            "RamDamage" => f(&mut self.ram_damage),
            "cuttingBeamDamagePerSecond" => f(&mut self.cutting_beam_damage_per_second),
            "cuttingBeamUpgradedDamagePerSecond" => f(&mut self.cutting_beam_upgraded_damage_per_second),
            "RamPushTime" => f(&mut self.ram_push_time),
            "RamWaitAfterPushTime" => f(&mut self.ram_wait_after_push_time),
            "UpgradeByDocking" => b(&mut self.upgrade_by_docking),
            "pilotViewOffsetX" => f(&mut self.pilot_view_offset.x),
            "pilotViewOffsetY" => f(&mut self.pilot_view_offset.y),
            "pilotViewOffsetZ" => f(&mut self.pilot_view_offset.z),
            "RKOGetsMissiles" => i(&mut self.rko_gets_missiles),
            "buildTime" => {
                if let Some(v) = scan_i32(raw) {
                    self.build_time = v.wrapping_shl(4);
                }
            }
            "fowRadius" => f(&mut self.fow_radius),
            "resourcesAtOneTime" => i(&mut self.resources_at_one_time),
            "RKOEatMeTime" => f(&mut self.rko_eat_me_time),
            "maxresources" => i(&mut self.max_resources),
            "harvestAmount" => i(&mut self.harvest_amount),
            "harvestRate" => i(&mut self.harvest_rate),
            "groupSize" => i(&mut self.group_size),
            "repairTime" => f(&mut self.repair_time),
            "repairDamage" => f(&mut self.repair_damage),
            "repairDroids" => i(&mut self.repair_droids),
            "repairCombatTime" => f(&mut self.repair_combat_time),
            "repairCombatDamage" => f(&mut self.repair_combat_damage),
            "repairOtherShipRate" => f(&mut self.repair_other_ship_rate),
            "repairResourceCollectorRate" => f(&mut self.repair_resource_collector_rate),
            "dockShipRange" => f(&mut self.dock_ship_range),
            "clearanceDistance" => f(&mut self.clearance_distance),
            "clearanceDirection" => i(&mut self.clearance_direction),
            "formationPaddingModifier" => f(&mut self.formation_padding_modifier),
            "cannotForceAttackIfOwnShip" => b(&mut self.cannot_force_attack_if_own_ship),
            "passiveAttackPenaltyExempt" => b(&mut self.passive_attack_penalty_exempt),
            "isCapitalShip" => b(&mut self.is_capital_ship),
            "cantMoveAndAttack" => b(&mut self.cant_move_and_attack),
            "repairBeamCapable" => b(&mut self.repair_beam_capable),
            "healthPerSecond" => f(&mut self.health_per_second),
            "CapitalDistanceRepairStart2" => f(&mut self.capital_distance_repair_start2),
            "CapitalDistanceRepairStart" => f(&mut self.capital_distance_repair_start),
            "AngleDotProdThreshold" => f(&mut self.angle_dot_prod_threshold),
            "N-LIPS" => f(&mut self.scale_cap),
            "minimumZoomDistance" => f(&mut self.minimum_zoom_distance),
            "renderlistFade" => f(&mut self.renderlist_fade),
            "renderlistLimit" => {
                if let Some(v) = scan_f32(raw) {
                    self.renderlist_limit_sqr = v * v;
                }
            }
            "dockLightNear" => f(&mut self.dock_light_near),
            "dockLightFar" => f(&mut self.dock_light_far),
            "dockLightColor" => rgb(&mut self.dock_light_color),
            "hyperspaceColor" => rgb(&mut self.hyperspace_color),
            "hyperspaceBoxScale" => f(&mut self.hyperspace_box_scale),
            "madGunOpenDamagedHealthThreshold" => f(&mut self.mad_gun_open_damaged_health_threshold),
            "madWingOpenDamagedHealthThreshold" => f(&mut self.mad_wing_open_damaged_health_threshold),
            "externalBuild" => b(&mut self.external_build),
            "leechRUsPerSecond" => i(&mut self.leech_rus_per_second),
            "leechHPsPerSecond" => f(&mut self.leech_hps_per_second),
            "Overlay_Selection_Scalar" => f(&mut self.overlay_selection_scalar),
            "beastAttackCascadeCost" => f(&mut self.beast_attack_cascade_cost),
            "beastAttacksRequired" => i(&mut self.beast_attacks_required),
            "beastAttackConvertTo" => self.beast_attack_convert_to = ShipType::from_name(raw),
            "linkDistance" => f(&mut self.link_distance),
            "forceFieldProtectionDistanceScale" => f(&mut self.force_field_protection_distance_scale),
            "bUseRayTriangleIntersectionForHitLocation" => {
                b(&mut self.use_ray_triangle_intersection_for_hit_location)
            }
            "upgradeTimeModifier" => f(&mut self.upgrade_time_modifier),
            _ => return false,
        }
        true
    }
}

/// `trailWidth3` -> `("trailWidth", 2)`. Only the spellings the original's table lists.
fn trail_key(key: &str) -> Option<(&str, usize)> {
    const FIELDS: [&str; 9] = [
        "trailStyle",
        "trailWidth",
        "trailHeight",
        "trailAngle",
        "trailRibbonAdjust",
        "trailLength",
        "trailScaleCap",
        "trailSpriteRadius",
        "trailSpriteOffset",
    ];
    let field = FIELDS.iter().find(|f| key.starts_with(**f))?;
    match &key[field.len()..] {
        "" => Some((field, 0)),
        "2" => Some((field, 1)),
        "3" => Some((field, 2)),
        "4" => Some((field, 3)),
        _ => None,
    }
}

/// Keywords read by the other ship-file readers (guns, docks, lights, trails, mesh animation
/// bindings). They are not "custom" entries.
const OTHER_READERS: &[&str] = &[
    "NUMBER_OF_GUNS",
    "GUN",
    "{",
    "}",
    "NUMBER_OF_DOCK_POINTS",
    "DockPoint",
    "NUMBER_OF_DOCK_OVERIDES",
    "DockOveride",
    "NUMBER_OF_NAV_LIGHTS",
    "NavLight",
    "trailSegments",
    "trailGranularity",
    "trailColor",
    "BindGun",
    "BindAnimations",
    "MadStartInfo",
    "MadGunAnims",
    "MadDockAnims",
    "MadMothershipDockingAnims",
    "MadSpecialCaseAnims",
    "NUMBER_OF_ANIMS",
    "ANIM",
];

/// Capability bits by ship type (the original's `switch` right after reading the file).
fn capability_flags(t: ShipType) -> u32 {
    use ShipType::*;
    match t {
        sRecon | sSentinel | bRecon | bSentinel | kProximitySensor | tProximitySensor | trFloatingCity
        | trResearchStation | dAcolyte => 0x10000,
        sAcolyte | bAcolyte => 0x20,
        sMimic => 0x20020,
        sLeech => 0x20300,
        sACV | bACV => 0x40,
        sMCV => 0x20040,
        sMultiBeamFrigate | bMultiBeamFrigate => 0x80,
        sHiveFrigate | bHiveFrigate | dRKO => 0x8000,
        sDreadnought | sDestroyer | tMissileDestroyer | p1MissileCorvette => 0x400,
        sCarrier | bCarrier | kCarrier | tCarrier => 0x8012,
        sWorker | bWorker | tResourceCollector => 0x1,
        sMothership | bMothership | dBeastMothership => 0x800e,
        sMothershipSpecial | sMothershipEngineering | sMothershipArmour | sMothershipMicro
        | sMothershipWeapons | sMothershipDockingBay | sMothershipSupport | sMothershipCommand
        | sMothershipEnginesLower | sMothershipEnginesUpper | sMothershipLowerSpine
        | sMothershipBigGun | sMothershipOreCanArmour | bMothershipDockingBay
        | bMothershipEnginesLower | bSectMoship | bMothershipSupport => 0x1000,
        bCloakedFighter | bIonArrayFrigate | p1IonArrayFrigate | p1StandardCorvette => 0x20020,
        tMinelayerCorvette => 0x800,
        dSlipgateSmall | dSlipgateMedium | dSlipgateLarge => 0xc000,
        _ => 0,
    }
}

/// `InitStatShipInfo`. `None` when the race directory has no file for the type (the original
/// then disables the type).
pub(crate) fn load_ship(ld: &mut Loader, ship_type: ShipType) -> Option<ShipStaticInfo> {
    let race = ship_type.race();
    let dir = format!("{}\\", race.name());
    let file = format!("{dir}{}.shp", ship_type.name());
    if !ld.src.exists(&file) || ld.opts.force_default_ship {
        return None;
    }
    let script = ld.read_script(&file)?;
    let tw = ld.tweaks.clone();
    let mut s = ShipStaticInfo::new(ship_type, file.clone());

    // Defaults set before the file is read.
    s.guidance.turn_speed = [1.0; 3];
    if ship_type.is_strike_craft() {
        s.formation_padding_modifier = tw.strikecraft_padding_modifier;
    }
    s.header.coll.precise_selection = 0xff;
    s.header.right_of_way = ld.tweaks.right_of_way[ship_type.index()];
    s.ship_class = Some(ShipClass::Mothership);

    // The ship's keyword table, every line in file order (gun-block lines included; the table
    // has no gun keywords).
    let mut in_block = false;
    for e in script.entries() {
        let (key, raw) = (e.key.as_str(), e.raw.as_str());
        if key.starts_with('{') {
            in_block = true;
        } else if key.starts_with('}') {
            in_block = false;
        }
        let bound = ld.apply_header_files(&mut s.header, key, raw, &dir)
            || s.header.apply(key, raw, HeaderKeys::SHIP)
            || s.health.apply(key, raw)
            || s.guidance.apply(key, raw)
            || s.apply(key, raw);
        if !bound && !in_block && !OTHER_READERS.contains(&key) {
            s.custom_entries.push((key.to_owned(), raw.to_owned()));
        }
    }

    if s.fow_radius == 0.0 {
        ld.warn(format!("{file}: no fowRadius"));
    }
    if s.repair_damage != 0.0 && s.repair_combat_damage == 0.0 {
        s.repair_combat_damage = (f64::from(s.repair_damage) * 0.2) as f32;
    }
    if s.repair_time != 0.0 && s.repair_combat_time == 0.0 {
        s.repair_combat_time = s.repair_time;
    }
    s.capability_flags = capability_flags(ship_type);
    if s.coll_side_modifiers[0] == 0.0 {
        s.coll_side_modifiers = if s.is_capital_ship { tw.capital_side_modifiers } else { [1.0; 6] };
    }
    if ld.opts.single_player {
        // Hard-coded single-player prices.
        match ship_type {
            ShipType::bHeavyCruiser => s.build_cost = 3000,
            ShipType::bIonArrayFrigate => s.build_cost = 600,
            ShipType::sMothershipBigGun => s.build_cost = 5000,
            _ => {}
        }
    }
    if s.blast_radius_shock_wave == 0.0 {
        s.blast_radius_shock_wave = tw.blast_radius_multiple_default;
    }
    if s.passive_retaliate_zone == 0.0 {
        s.passive_retaliate_zone = tw.retaliate_zone;
    } else if s.passive_retaliate_zone < 0.0 {
        s.passive_retaliate_zone = 0.0;
    }
    s.can_receive_something = s.can_receive_ships_for_retire
        || s.can_receive_ships_permanently
        || s.can_receive_resources
        || s.can_receive_ships;
    s.multiple_engines = s.header.mex.as_ref().is_some_and(|m| m.find("Eng", "Eng1").is_some());

    s.header.precalc(&tw);
    if ship_type == ShipType::trMiningBase {
        let c = &mut s.header.coll;
        c.coll_sphere_offset.x += tw.mining_base_vertical_dist_hack;
        c.coll_rect_offset.x += tw.mining_base_vertical_dist_hack;
    }
    s.health.precalc(&tw);
    s.guidance.precalc(&mut s.header, &tw);
    set_ai_banking(&mut s, &tw);

    if let Some(mex) = s.header.mex.clone() {
        for i in 0..4 {
            let name = format!("Eng{i}");
            if let Some(c) = mex.find("Eng", &name) {
                if let hwc_formats::mex::ChunkData::Engine { point, attached_to } = &c.data {
                    s.engine_nozzle_offset[i] = v3(point.position);
                    s.engine_nozzle_attached_to[i] = attached_to.clone().filter(|a| !a.is_empty());
                }
            }
        }
    }

    let mut warnings = Vec::new();
    s.dock = DockStaticInfo::scan(&script, &mut warnings);
    if let (Some(d), Some(mex)) = (s.dock.as_mut(), s.header.mex.as_ref()) {
        d.apply_mex(mex, &mut warnings);
    }
    if s.ship_class == Some(ShipClass::Resource) && s.harvest_amount > 0 {
        s.res_nozzle = s.header.mex.as_ref().and_then(|m| nozzle(m, "Gun0", true));
    }
    if s.repair_beam_capable {
        s.repair_nozzle = s.header.mex.as_ref().and_then(|m| nozzle(m, "Rep0", false));
    }
    s.nav_lights = navlight::scan_nav_lights(&script);
    if let (Some(l), Some(mex)) = (s.nav_lights.as_mut(), s.header.mex.as_ref()) {
        navlight::apply_mex(l, mex, &mut warnings);
    }

    let scan = gun::scan_guns(&script);
    warnings.extend(scan.warnings);
    s.unbound_gun_keywords = scan.unbound;
    s.guns = scan.guns;
    match s.guns.as_mut() {
        Some(guns) => {
            let (max, min) = gun::finish_ranges(guns);
            s.bullet_range = [max; 3];
            s.min_bullet_range = [min; 3];
            for k in 0..3 {
                s.bullet_range_squared[k] = s.bullet_range[k] * s.bullet_range[k];
            }
            if let Some(mex) = s.header.mex.as_ref() {
                gun::apply_mex(guns, mex);
            }
        }
        None if s.ship_class == Some(ShipClass::Resource) => {
            s.bullet_range = [tw.asteroid_harvest_range; 3];
            for k in 0..3 {
                s.bullet_range_squared[k] = s.bullet_range[k] * s.bullet_range[k];
            }
        }
        None => {}
    }
    if s.bullet_range[0] != 0.0 {
        for k in 0..3 {
            s.bullet_range_squared[k] = s.bullet_range[k] * s.bullet_range[k];
        }
    }

    if s.force_field_protection_distance_scale <= 1.0 {
        warnings.push("forceFieldProtectionDistanceScale missing or <= 1; using 2".into());
        s.force_field_protection_distance_scale = 2.0;
    }
    for w in warnings {
        ld.warn(format!("{file}: {w}"));
    }
    Some(s)
}

/// The nozzle reader: position (and normal) of a `Gun` chunk with the given name.
fn nozzle(mex: &hwc_formats::mex::Mex, name: &str, with_normal: bool) -> Option<Nozzle> {
    let c = mex.find("Gun", name)?;
    let hwc_formats::mex::ChunkData::Gun(p) = &c.data else { return None };
    let normal = with_normal.then(|| {
        let mut n = v3(p.normal);
        if n.is_zero() {
            n.x = 1.0;
        } else {
            n.normalize();
        }
        n
    });
    Some(Nozzle { position: v3(p.position), normal })
}

/// `aishipPrecalcInfo`: banking sine and pitch angles by class; class-less special cases use
/// the large-ship values.
fn set_ai_banking(s: &mut ShipStaticInfo, tw: &super::Tweaks) {
    s.sinbank = 0.0;
    s.pitchturn = 0.0;
    s.pitchdescend = 0.0;
    let massive = |s: &mut ShipStaticInfo| {
        s.sinbank = tw.massiveship_bank;
        s.pitchturn = tw.massiveship_turnpitch;
        s.pitchdescend = tw.massiveship_descendpitch;
    };
    match s.ship_class {
        Some(ShipClass::HeavyCruiser | ShipClass::Carrier) => massive(s),
        Some(ShipClass::Destroyer) => {
            s.sinbank = tw.destroyer_bank;
            s.pitchturn = tw.destroyer_turnpitch;
            s.pitchdescend = tw.destroyer_descendpitch;
        }
        Some(ShipClass::Frigate) => {
            s.sinbank = tw.frigate_bank;
            s.pitchturn = tw.frigate_turnpitch;
            s.pitchdescend = tw.frigate_descendpitch;
        }
        Some(ShipClass::Corvette) => s.sinbank = tw.corvette_bank,
        Some(ShipClass::Fighter) => s.sinbank = tw.fighter_bank,
        _ => {
            use ShipType::*;
            if matches!(
                s.ship_type,
                p1Mothership | trFloatingCity | trResearchStation | trGhostship | dRKO | dBeastMothership
            ) {
                massive(s);
            }
        }
    }
}

/// Default `rightOfWay` per ship type (table at 0x8a7a80).
pub(crate) const RIGHT_OF_WAY: [i8; ShipType::COUNT] = [
    0, 0, 0, 0, 0, 0, 1, 1, 1, 0, 2, 2, 3, 3, 1, 0, 2, 0, 4, 4, 4, 4, 4, 4, 4, 2, 4, 4, 4, 4, 4, 4, 4, 0, 0, 0,
    0, 0, 0, 0, 0, 1, 1, 0, 1, 1, 2, 3, 1, 1, 0, 2, 0, 4, 4, 4, 4, 4, 0, 0, 1, 0, 3, 1, 1, 0, 2, 0, 0, 1, 0, 0,
    0, 2, 1, 0, 3, 1, 1, 1, 2, 0, 0, 1, 0, 2, 0, 0, 1, 0, 2, 1, 4, 0, 1, 0, 4, 0, 4, 1, 4, 4, 1, 2, 1, 1, 0, 4,
    4, 4, 2, 2, 0, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 0, 0, 0, 2, 2, 4, 0, 4, 4, 4, 0, 3, 3, 3, 2, 1, 1, 3, 1, 0, 0,
    0,
];

/// Parses a ship file for tools that want the raw script.
pub fn read_ship_script(src: &dyn super::DataSource, ship_type: ShipType) -> Option<ScriptFile> {
    let path = format!("{}\\{}.shp", ship_type.race().name(), ship_type.name());
    ScriptFile::parse_bytes(&src.read(&path)?).ok()
}
