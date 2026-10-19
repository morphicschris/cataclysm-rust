//! Cloaked fighters and ion array frigates (cloaked-fighter part of `GenericInterceptor` 0x5ae200..0x5ae2d0, `P1IonArrayFrigate` 0x5b8a70..0x5b8bc0): bCloakedFighter, bIonArrayFrigate, p1IonArrayFrigate.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloakerSpec {
    pub cloak: crate::cloak::Cloak,
}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CloakerStatics {}

impl CloakerStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
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

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
