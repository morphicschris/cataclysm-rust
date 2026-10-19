//! Mimic disguises (`Mimic` 0x5b2d70..0x5b4c40, `MCV` 0x5b2b30..0x5b2d50 and the P1 standard corvette): sMimic, sMCV, p1StandardCorvette.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MimicSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MimicStatics {}

impl MimicStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn mcv_attack_passive(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn mcv_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn p1_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let _ = (u, id);
    true
}

pub fn mcv_special_activate(u: &mut Universe, id: ShipId) -> bool {
    let _ = (u, id);
    true
}

pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let _ = (u, id, targets);
    true
}

pub fn mcv_special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let _ = (u, id, targets);
    true
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn cancel_disguise(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
/// `MimicIsDisguised`.
pub fn is_disguised(u: &Universe, id: ShipId) -> bool {
    let _ = (u, id);
    false
}

/// `MimicIsDisguisedAsPlayer`: whether the disguise makes `player` take the ship for one of
/// their own (or an ally's).
pub fn is_disguised_as_player(u: &Universe, id: ShipId, player: usize) -> bool {
    let _ = (u, id, player);
    false
}

/// `MimicFoolsPlayerAsResource`.
pub fn fools_player_as_resource(u: &Universe, id: ShipId, player: usize) -> bool {
    let _ = (u, id, player);
    false
}

/// `MimicFoolsPlayerAsDerelict`.
pub fn fools_player_as_derelict(u: &Universe, id: ShipId, player: usize) -> bool {
    let _ = (u, id, player);
    false
}

/// `MimicPickRandomDisguiseType` (0x5b2e20): a random type from the disguise table of the
/// mimic type and the player's race (stream 6).
pub fn pick_random_disguise_type(u: &mut Universe, mimic_type: crate::statics::ShipType, race: crate::statics::ShipRace) -> crate::statics::ShipType {
    let _ = (u, race);
    mimic_type
}

/// The disguise order (`FUN_0044f3a0` / `FUN_00449520`): the mimics disguise as `disguise` of
/// player `player`.
pub fn cl_disguise_as(u: &mut Universe, ships: &[ShipId], disguise: crate::statics::ShipType, player: usize) {
    let _ = (u, ships, disguise, player);
}
