//! Proximity sensors (`ProximitySensor` module, 0x5b9d80..0x5b9f30): kProximitySensor, tProximitySensor.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProximitySpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProximityStatics {}

impl ProximityStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
