//! Hive frigates and their swarmers (`HiveFrigate` 0x5afef0..0x5b11e0, `Swarmer` 0x5c22b0..): s/bHiveFrigate, s/bSwarmer.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HiveSpec {}

/// Per-swarmer state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SwarmerSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct HiveStatics {}

impl HiveStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn swarmer_init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let _ = (u, id);
    true
}

pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let _ = (u, id, targets);
    true
}

pub fn order_changed(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn swarmer_remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn cancel_special(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

/// `HiveFrigateIsLaunching`.
pub fn is_launching(u: &Universe, id: ShipId) -> bool {
    let _ = (u, id);
    false
}
