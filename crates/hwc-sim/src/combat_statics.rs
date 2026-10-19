//! Per-type combat data the ship modules read: which attack, passive-attack and fire routines
//! a ship type uses (the `CustShipHeader` table at 0x8a75f8), and the custom statics those
//! routines keep (`GenericInterceptorStatics`, the capital ships' attack ranges, the side-step
//! and multi-beam parameters). Built once per [`StaticWorld`] from the keywords the ship files
//! carry for the per-type static initialisers.

use crate::statics::cscan::{decomma, scan, Scanned};
use crate::statics::{ShipClass, ShipStaticInfo, ShipType, StaticWorld};

/// Number of ship classes plus the "Default" column of the per-class tables.
pub const CLASS_COLUMNS: usize = 13;
/// The "Default" column.
pub const DEFAULT_CLASS: usize = 12;

/// `CustShipAttack` routine of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttackKind {
    /// `GenericInterceptorAttack` (fighters).
    Interceptor,
    /// `AcolyteAttack`: resets the link state when the target changes, then interceptor.
    Acolyte,
    /// `P1MothershipAttack`: `attackPassive` with the type's range table (capital ships).
    Capital,
    /// `DefaultShipAttack`: `attackPassive` with the bullet ranges.
    Default,
    /// `RepairCorvetteAttack` (shared corvette routine): `attackSideStep`.
    SideStep,
    /// `P2MultiBeamFrigateAttackDoAttack`.
    MultiBeam,
    /// A routine this reimplementation does not have yet (the ship holds still).
    Unsupported,
    /// No attack routine.
    None,
}

/// `CustShipAttackPassive` routine of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PassiveKind {
    /// `GenericInterceptorPassiveAttack`.
    Interceptor,
    /// The shared `DefaultShipAttackPassive` (`attackPassiveRotate` / `attackSimple`).
    Default,
    /// `P2MultiBeamFrigateAttackPassive`: just fires.
    MultiBeam,
    Unsupported,
    None,
}

/// `CustShipFire` routine of a type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FireKind {
    /// `GenericInterceptorFire`: the active gun only.
    Interceptor,
    /// `AcolyteFire`.
    Acolyte,
    /// `DefaultShipFire`: every ready gun.
    Default,
    /// `GenericCapitalFire`: like the default, with the Cataclysm gun types.
    GenericCapital,
    /// `P2MultiBeamFrigateFire`.
    MultiBeam,
    Unsupported,
    None,
}

/// `FlightManProb` (0x148 bytes): probabilities of the flight manoeuvres by situation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FlightManProb {
    pub prob_turnaround: [u32; 7],
    pub cum_turnaround: [u32; 7],
    pub sum_turnaround: u32,
    pub prob_aip: [u32; 7],
    pub cum_aip: [u32; 7],
    pub sum_aip: u32,
    pub prob_evasive_behind: [u32; 8],
    pub cum_evasive_behind: [u32; 8],
    pub sum_evasive_behind: u32,
    pub prob_evasive_front: [u32; 8],
    pub cum_evasive_front: [u32; 8],
    pub sum_evasive_front: u32,
    pub prob_evasive_pure: [u32; 8],
    pub cum_evasive_pure: [u32; 8],
    pub sum_evasive_pure: u32,
    pub valid: u32,
}

/// `GenericInterceptorStatics`: per tactic (3) and target class (12 + default).
#[derive(Debug, Clone, PartialEq)]
pub struct InterceptorStatics {
    /// `gunsFireTime`.
    pub fire_time: f32,
    pub max_fly_away_dist: [[f32; CLASS_COLUMNS]; 3],
    /// Squares, computed before the per-class defaults are filled in (so classes that take
    /// the default keep 0 here, as in the original).
    pub max_fly_away_dist_sq: [[f32; CLASS_COLUMNS]; 3],
    pub break_range: [[f32; CLASS_COLUMNS]; 3],
    pub fly_past_dist: [[f32; CLASS_COLUMNS]; 3],
    /// Cosines.
    pub trigger_happy: [[f32; CLASS_COLUMNS]; 3],
    pub face_target_accuracy: [[f32; CLASS_COLUMNS]; 3],
    pub max_attack_time: [[f32; CLASS_COLUMNS]; 3],
    pub flightman_prob: Box<[[FlightManProb; CLASS_COLUMNS]; 3]>,
}

/// Side-step parameters (`repositionTime`, `circleRange`, `fullMovementFreedom`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct SideStepStatics {
    pub reposition_time: f32,
    pub circle_range: f32,
    pub full_movement_freedom: bool,
}

/// Combat data of one ship type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeCombat {
    pub attack: AttackKind,
    pub passive: PassiveKind,
    pub fire: FireKind,
    pub interceptor: Option<InterceptorStatics>,
    /// The capital ships' attack ranges (custom statics +0x194): bullet range per tactic, and
    /// the minimum range per tactic (x0.9; ion cannon frigates use 0.725 and 0.5).
    pub capital_range: [f32; 3],
    pub capital_min_range: [f32; 3],
    pub side_step: SideStepStatics,
    /// `MultiBeamRangeRatio`.
    pub multi_beam_range_ratio: f32,
}

/// Combat data of every ship type.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CombatStatics {
    pub types: Vec<Option<TypeCombat>>,
}

impl CombatStatics {
    pub fn build(statics: &StaticWorld) -> Self {
        let types = ShipType::ALL.iter().map(|&t| statics.ship(t).map(TypeCombat::build)).collect();
        Self { types }
    }

    pub fn get(&self, t: ShipType) -> Option<&TypeCombat> {
        self.types.get(t.index()).and_then(Option::as_ref)
    }
}

/// The routines of each type, from the `CustShipHeader` table.
fn routines(t: ShipType) -> (AttackKind, PassiveKind, FireKind) {
    use AttackKind as A;
    use FireKind as F;
    use PassiveKind as P;
    use ShipType::*;
    match t {
        sRecon | bRecon | bInterceptor | bAttackBomber | kAttackBomber | kHeavyInterceptor | tAttackBomber
        | tHeavyInterceptor | p1Fighter => (A::Interceptor, P::Interceptor, F::Interceptor),
        sAcolyte | bAcolyte | dAcolyte => (A::Acolyte, P::Interceptor, F::Acolyte),
        sMultiBeamFrigate | bMultiBeamFrigate => (A::MultiBeam, P::MultiBeam, F::MultiBeam),
        sDestroyer | kStandardDestroyer | tStandardDestroyer | tStandardFrigate | kIonCannonFrigate
        | tIonCannonFrigate | kAdvanceSupportFrigate | tAdvanceSupportFrigate | bHeavyCruiser | kHeavyCruiser
        | tHeavyCruiser | tMissileDestroyer | p1Mothership | trFloatingCity | dIonArrayFrigate | dIonCannonFrigate => {
            (A::Capital, P::Default, F::Default)
        }
        sDreadnought => (A::Capital, P::Default, F::GenericCapital),
        sCarrier | bCarrier | kCarrier | tCarrier | dKushanCarrier => (A::Capital, P::Unsupported, F::Default),
        trCargoBarge | trResearchStation => (A::Default, P::Default, F::Default),
        bMultiGunCorvette | kMultiGunCorvette | tMultiGunCorvette | bHeavyCorvette | kHeavyCorvette => {
            (A::SideStep, P::Default, F::Default)
        }
        sSwarmer | bSwarmer => (A::SideStep, P::None, F::Default),
        trJunk_LGun | trJunk_SGun => (A::None, P::Default, F::Default),
        // Attack routines of their own that are not reproduced yet: Mimic, ACV, MCV, ramming and
        // hive frigates, processors, sentinels, motherships, ion array and DFG frigates, gravity
        // wells, heavy defenders, missile and standard corvettes, minelayers, cloaked fighters,
        // cruise missiles, the Nomad moon.
        sMimic | sACV | sMCV | sRammingFrigate | sHiveFrigate | sProcessor | sSentinel | sMothership | bCloakedFighter
        | bACV | bHiveFrigate | bIonArrayFrigate | bDFGFrigate | bRammingFrigate | bProcessor | bSentinel
        | bMothership | bCruiseMissile | bMissileCorvette | kGravWellGenerator | kHeavyDefender | tDFGFrigate
        | tGravWellGenerator | tHeavyDefender | tMinelayerCorvette | p1IonArrayFrigate | p1MissileCorvette
        | p1StandardCorvette | trGhostship | dBeastMothership | dNomadMoon => (A::Unsupported, P::Unsupported, F::Unsupported),
        _ => (A::None, P::None, F::None),
    }
}

impl TypeCombat {
    fn build(stat: &ShipStaticInfo) -> Self {
        let (attack, passive, fire) = routines(stat.ship_type);
        let interceptor = matches!(attack, AttackKind::Interceptor | AttackKind::Acolyte).then(|| InterceptorStatics::parse(stat));
        let (range_k, min_k) = match stat.ship_type {
            ShipType::kIonCannonFrigate | ShipType::tIonCannonFrigate | ShipType::dIonArrayFrigate | ShipType::dIonCannonFrigate => {
                (0.725f32, 0.5f32)
            }
            _ => (1.0, 0.9),
        };
        let mut capital_range = [0.0; 3];
        let mut capital_min_range = [0.0; 3];
        for t in 0..3 {
            capital_range[t] = if range_k == 1.0 { stat.bullet_range[t] } else { (f64::from(stat.bullet_range[t]) * f64::from(range_k)) as f32 };
            capital_min_range[t] = (f64::from(stat.min_bullet_range[t]) * f64::from(min_k)) as f32;
        }
        let mut side_step = SideStepStatics::default();
        let mut multi_beam_range_ratio = 0.0;
        for (key, raw) in &stat.custom_entries {
            let f = || crate::statics::cscan::scan_f32(raw);
            match key.as_str() {
                "repositionTime" => side_step.reposition_time = f().unwrap_or(side_step.reposition_time),
                "circleRange" => side_step.circle_range = f().unwrap_or(side_step.circle_range),
                "fullMovementFreedom" => side_step.full_movement_freedom = crate::statics::cscan::parse_bool(raw),
                "MultiBeamRangeRatio" => multi_beam_range_ratio = f().unwrap_or(multi_beam_range_ratio),
                _ => {}
            }
        }
        Self { attack, passive, fire, interceptor, capital_range, capital_min_range, side_step, multi_beam_range_ratio }
    }
}

/// `StrToTacticsType`.
fn tactics_from_name(s: &str) -> Option<usize> {
    ["Evasive", "Neutral", "Aggressive"].iter().position(|n| n.eq_ignore_ascii_case(s))
}

/// The column of a `tactic class value` line: `Default` (compared exactly for the real-valued
/// tables) or a class name.
fn class_column(name: &str, exact_default: bool) -> Option<usize> {
    let is_default = if exact_default { name == "Default" } else { name.eq_ignore_ascii_case("Default") };
    if is_default {
        Some(DEFAULT_CLASS)
    } else {
        ShipClass::from_name(name).map(|c| c.index())
    }
}

impl InterceptorStatics {
    /// `GenericInterceptorStaticInit` (0x5ac9a0).
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let z = [[0.0f32; CLASS_COLUMNS]; 3];
        let mut s = Self {
            fire_time: 0.0,
            max_fly_away_dist: z,
            max_fly_away_dist_sq: z,
            break_range: z,
            fly_past_dist: z,
            trigger_happy: z,
            face_target_accuracy: z,
            max_attack_time: z,
            flightman_prob: Box::new([[FlightManProb::default(); CLASS_COLUMNS]; 3]),
        };
        for (key, raw) in &stat.custom_entries {
            let array = |cos: bool| -> Option<(usize, usize, f32)> {
                let v = scan(raw, "%s %s %f");
                let (Some(Scanned::Str(t)), Some(Scanned::Str(c)), Some(Scanned::Float(x))) = (v.first(), v.get(1), v.get(2)) else {
                    return None;
                };
                let x = if cos { hwc_math::fpu::st(hwc_math::trig::fcos(f64::from(*x) * f64::from(0.017_453_292f32))) } else { *x };
                Some((tactics_from_name(t)?, class_column(c, true)?, x))
            };
            match key.as_str() {
                "gunsFireTime" => {
                    if let Some(v) = crate::statics::cscan::scan_f32(raw) {
                        s.fire_time = v;
                    }
                }
                "maxFlyAwayDist" | "breakRange" | "flyPastDist" | "maxAttackTime" => {
                    if let Some((t, c, x)) = array(false) {
                        let table = match key.as_str() {
                            "maxFlyAwayDist" => &mut s.max_fly_away_dist,
                            "breakRange" => &mut s.break_range,
                            "flyPastDist" => &mut s.fly_past_dist,
                            _ => &mut s.max_attack_time,
                        };
                        table[t][c] = x;
                    }
                }
                "triggerHappy" | "faceTargetAccuracy" => {
                    if let Some((t, c, x)) = array(true) {
                        let table = if key == "triggerHappy" { &mut s.trigger_happy } else { &mut s.face_target_accuracy };
                        table[t][c] = x;
                    }
                }
                "flightmanTurnaround" | "flightmanAIP" | "flightmanEvasiveBehind" | "flightmanEvasiveFront"
                | "flightmanEvasivePure" => {
                    let v = scan(raw, "%s %s %s");
                    let (Some(Scanned::Str(t)), Some(Scanned::Str(c)), Some(Scanned::Str(list))) = (v.first(), v.get(1), v.get(2)) else {
                        continue;
                    };
                    let (Some(t), Some(c)) = (tactics_from_name(t), class_column(c, false)) else { continue };
                    let p = &mut s.flightman_prob[t][c];
                    let values = parse_prob_list(list);
                    match key.as_str() {
                        "flightmanTurnaround" => set_probs(&values, &mut p.prob_turnaround, &mut p.cum_turnaround, &mut p.sum_turnaround),
                        "flightmanAIP" => set_probs(&values, &mut p.prob_aip, &mut p.cum_aip, &mut p.sum_aip),
                        "flightmanEvasiveBehind" => {
                            set_probs(&values, &mut p.prob_evasive_behind, &mut p.cum_evasive_behind, &mut p.sum_evasive_behind)
                        }
                        "flightmanEvasiveFront" => {
                            set_probs(&values, &mut p.prob_evasive_front, &mut p.cum_evasive_front, &mut p.sum_evasive_front)
                        }
                        _ => set_probs(&values, &mut p.prob_evasive_pure, &mut p.cum_evasive_pure, &mut p.sum_evasive_pure),
                    }
                    p.valid = 0xcc;
                }
                _ => {}
            }
        }
        // Squares of every column, then the classes without a fly-past distance take the
        // default column (the squares are not copied).
        for t in 0..3 {
            for c in 0..CLASS_COLUMNS {
                let d = s.max_fly_away_dist[t][c];
                s.max_fly_away_dist_sq[t][c] = d * d;
            }
        }
        for t in 0..3 {
            for c in 0..DEFAULT_CLASS {
                if s.fly_past_dist[t][c] == 0.0 {
                    s.max_fly_away_dist[t][c] = s.max_fly_away_dist[t][DEFAULT_CLASS];
                    s.break_range[t][c] = s.break_range[t][DEFAULT_CLASS];
                    s.fly_past_dist[t][c] = s.fly_past_dist[t][DEFAULT_CLASS];
                    s.trigger_happy[t][c] = s.trigger_happy[t][DEFAULT_CLASS];
                    s.face_target_accuracy[t][c] = s.face_target_accuracy[t][DEFAULT_CLASS];
                    s.max_attack_time[t][c] = s.max_attack_time[t][DEFAULT_CLASS];
                    s.flightman_prob[t][c] = s.flightman_prob[t][DEFAULT_CLASS];
                }
            }
        }
        s
    }
}

/// The comma-separated integers of a flight-manoeuvre probability list (`%d` per value).
fn parse_prob_list(list: &str) -> Vec<u32> {
    decomma(list)
        .split_whitespace()
        .map(|v| crate::statics::cscan::scan_i32(v).unwrap_or(0) as u32)
        .collect()
}

/// `setFlightManProb`: the values (as many as given, up to the table size) and their running
/// sums over the whole table.
fn set_probs<const N: usize>(values: &[u32], prob: &mut [u32; N], cum: &mut [u32; N], sum: &mut u32) {
    for (p, v) in prob.iter_mut().zip(values) {
        *p = *v;
    }
    let mut acc = 0u32;
    for i in 0..N {
        acc = acc.wrapping_add(prob[i]);
        cum[i] = acc;
    }
    *sum = acc;
}

/// Index of a target's class column (`NUM_CLASSES` for non-ships or unknown classes).
pub fn class_index(class: Option<ShipClass>) -> usize {
    class.map_or(DEFAULT_CLASS, |c| c.index())
}

/// Whether a type is an Acolyte (`AcolyteIsAcolyte`).
pub fn is_acolyte(t: ShipType) -> bool {
    matches!(t, ShipType::sAcolyte | ShipType::bAcolyte | ShipType::dAcolyte)
}

/// Bullet-type index for a weapon technology and gun size (0x486de0).
pub fn bullet_type(weapon: u32, size: u32) -> u32 {
    match (weapon, size) {
        (1, 2) => 0,
        (1, 1) => 4,
        (1, 0) => 10,
        (1, 3) => 14,
        (2, 2) => 1,
        (2, 1) => 5,
        (2, 0) => 11,
        (2, 3) => 15,
        (3, 2) => 2,
        (3, 1) => 6,
        (3, 0) => 12,
        (3, 3) => 16,
        (0, 2) => 3,
        (0, 1) => 7,
        (0, 0) => 13,
        (6, _) => 8,
        (5, _) => 9,
        (4, _) => 17,
        (7, _) => 18,
        (8, _) => 19,
        // Projectile very-large and unknown pairs fall through to the size value.
        (_, s) => s,
    }
}

/// Damage per bullet type and armour level (`suGetUpgradeMultiplier`, table at 0x721200).
pub const WEAPON_DAMAGE: [[f32; 3]; 20] = [
    [80.0, 75.0, 50.0],
    [16.0, 14.0, 9.0],
    [100.0, 95.0, 65.0],
    [40.0, 28.0, 26.0],
    [40.0, 38.0, 30.0],
    [10.0, 9.0, 6.0],
    [70.0, 65.0, 45.0],
    [20.0, 15.0, 14.0],
    [400.0, 300.0, 295.0],
    [60.0, 40.0, 38.0],
    [22.0, 20.0, 14.0],
    [5.0, 4.0, 3.0],
    [50.0, 47.0, 35.0],
    [7.0, 4.0, 4.0],
    [125.0, 115.0, 90.0],
    [36.0, 33.0, 25.0],
    [150.0, 140.0, 100.0],
    [10.0, 10.0, 8.0],
    [0.0, 0.0, 0.0],
    [0.0, 0.0, 0.0],
];

/// Weapon damage factor per upgrade level (0x721b50), the same steps as the speed table.
pub const DAMAGE_UPGRADE_TABLE: [f32; 5] = [1.0, 1.1, 1.2, 1.3, 1.4];
