//! Processors and their repair beams (`Processor` module, 0x5b92a0..0x5b9d50): sProcessor, bProcessor.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessorSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProcessorStatics {}

impl ProcessorStatics {
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

pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn stop_beam_on_ship(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
