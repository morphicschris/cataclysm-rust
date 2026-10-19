//! The Dreadnought and its cannon (`Dreadnought` module, 0x5ab970..0x5abee0): sDreadnought (the cannon itself is a ship module, sDreadnoughtCannon).
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DreadnoughtSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DreadnoughtStatics {}

impl DreadnoughtStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
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
