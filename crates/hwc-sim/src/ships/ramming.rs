//! The ramming frigate (`RammingFrigate` module, 0x5ba110..0x5bcab0): sRammingFrigate, bRammingFrigate.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RammingSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RammingStatics {}

impl RammingStatics {
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

pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let _ = (u, id, targets);
    true
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn remove_target_ref(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn cancel_ram(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn stop_pushing(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

/// `RammingFrigateIsLiveRammer`.
pub fn is_live_rammer(u: &Universe, id: ShipId) -> bool {
    let _ = (u, id);
    false
}
