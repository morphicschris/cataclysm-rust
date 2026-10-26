//! Cloaked fighters and ion array frigates (cloaked-fighter part of `GenericInterceptor`
//! 0x5ae200..0x5ae2d0, `P1IonArrayFrigate` 0x5b8a70..0x5b8bc0): bCloakedFighter,
//! bIonArrayFrigate, p1IonArrayFrigate.
//!
//! Both keep a [`crate::cloak::Cloak`] (cloaked fighter +0x650, ion array frigates +0x638),
//! toggle it with their special (`P1IonArrayFrigateSpecialActivate` serves both headers) and
//! run `cloakUpdate` as their housekeeping. A cloaked ship drops its cloak when it is told to
//! attack (the ion array frigates of a single-player game stay cloaked) and holds fire while
//! cloaked.
//!
//! `kCloakGenerator` / `tCloakGenerator` have no header at all (NULL entries in the
//! `CustShipHeader` table) and `CloakGeneratorStaticInit` is never referenced: the cloak
//! generators have no behaviour in Cataclysm.

use hwc_math::fpu::{ld, st};

use crate::cloak;
use crate::object::ShipId;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Power drained per update while cloaked and regained per update while visible
/// (`cloakInit(.., 0.1, 0.1)` in both init routines).
pub const CLOAK_RATE: f32 = f32::from_bits(0x3dcc_cccd);

/// The factor `P1IonArrayFrigateStaticInit` applies to the minimum bullet ranges (0x7205a8).
const MIN_RANGE_FACTOR: f32 = 0.9;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CloakerSpec {
    pub cloak: crate::cloak::Cloak,
}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CloakerStatics {
    /// `P1IonArrayFrigateStaticInit`: the attack range per tactic (the bullet ranges) and the
    /// minimum range per tactic (the minimum bullet ranges x 0.9). The cloaked fighter uses the
    /// interceptor statics instead (`CloakedFighterStaticInit` = `GenericInterceptorStaticInit`).
    pub range: [f32; 3],
    pub min_range: [f32; 3],
}

impl CloakerStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        if matches!(
            stat.ship_type,
            ShipType::bIonArrayFrigate | ShipType::p1IonArrayFrigate
        ) {
            // (The original keeps one table per race: Beast, Taiidan, Turanic pirates.)
            for t in 0..3 {
                s.range[t] = stat.bullet_range[t];
                s.min_range[t] = st(ld(stat.min_bullet_range[t]) * ld(MIN_RANGE_FACTOR));
            }
        }
        s
    }
}

fn statics(u: &Universe, t: ShipType) -> CloakerStatics {
    match super::type_statics(u, t) {
        super::TypeStatics::Cloaker(c) => c.clone(),
        _ => CloakerStatics::default(),
    }
}

/// `CloakedFighterInit` (0x5ae220) / `P1IonArrayFrigateInit` (0x5b8ad0): the cloak, full
/// power, 0.1 per update both ways (and the fighter's active gun).
pub fn init(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    if s.ship_type == ShipType::bCloakedFighter {
        s.interceptor.active_gun = 0;
    }
    let c = cloak::init(s, CLOAK_RATE, CLOAK_RATE);
    s.spec = ShipSpec::Cloaker(CloakerSpec { cloak: c });
}

fn cloaked(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(cloak::is_cloaked)
}

/// `CloakedFighterFire` (0x5ae2d0): `GenericInterceptorFire`, unless cloaked.
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    if cloaked(u, id) {
        return;
    }
    crate::attack::interceptor_fire(u, id, target);
}

/// `CloakedFighterAttackPassive` (0x5ae290): decloak, or the interceptor passive attack.
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if cloaked(u, id) {
        cloak::toggle(u, id);
        return;
    }
    crate::attack::generic_interceptor_passive(u, id, target, rotate);
}

/// `CloakedFighterAttack` (0x5ae250): decloak, or the interceptor attack.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if cloaked(u, id) {
        cloak::toggle(u, id);
        return;
    }
    crate::attack::generic_interceptor_attack(u, id, target);
}

/// `P1IonArrayFrigateAttack` (0x5b8af0): `attackPassive` with the type's ranges; a cloaked
/// frigate decloaks first (not in single-player games, where it just waits).
pub fn ion_array_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if cloaked(u, id) {
        if !u.single_player {
            cloak::toggle(u, id);
        }
        return;
    }
    let (t, tac) = {
        let s = u.ships.ship(id);
        (s.ship_type, s.tactics.min(2))
    };
    let st_ = statics(u, t);
    crate::attack::attack_passive(u, id, target, st_.range[tac], st_.min_range[tac]);
}

/// `P1IonArrayFrigateAttackPassive` (0x5b8b50): decloak (multiplayer only), or turn and fire
/// (`attackPassiveRotate`) / just fire (`attackSimple`).
pub fn ion_array_attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if cloaked(u, id) {
        if !u.single_player {
            cloak::toggle(u, id);
        }
        return;
    }
    if rotate {
        crate::attack::attack_passive_rotate(u, id, target);
    } else {
        crate::attack::attack_simple(u, id, target);
    }
}

/// `P1IonArrayFrigateSpecialActivate` (0x5b8bb0), both headers: toggle the cloak.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    cloak::toggle(u, id);
    true
}

/// `P1IonArrayFrigateHouseKeep` (0x5b8bc0), both headers: `cloakUpdate`.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    cloak::update(u, id);
}
