//! The Leech (`Leech` module, 0x5b12d0..0x5b2ac0): sLeech.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LeechSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LeechStatics {}

impl LeechStatics {
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

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn emp_shocked(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn special_order_given(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

/// `LeechIsActive`.
pub fn is_active(u: &Universe, id: ShipId) -> bool {
    let _ = (u, id);
    false
}
