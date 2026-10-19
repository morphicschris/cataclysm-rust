//! The DFG frigate and its field (`DFGFrigate` module, 0x5ab420..0x5ab910): bDFGFrigate, tDFGFrigate.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DfgSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DfgStatics {}

impl DfgStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
