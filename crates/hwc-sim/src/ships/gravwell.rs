//! Gravity well generators (`GravWellGenerator` module, 0x5aeab0..0x5af7f0): kGravWellGenerator, tGravWellGenerator.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GravWellSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GravWellStatics {}

impl GravWellStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn close(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
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

pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
