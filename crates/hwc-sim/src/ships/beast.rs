//! The Beast: infection (`beastInfectShip` 0x429aa0, `univCheckBeastInfectionComplete` 0x53e8f0), the cascade of Beast capital ships (0x429d70..0x42b720, `HeavyCruiser`, `Carrier`, `Mothership` wrappers) and the Beast mothership (`BeastMothership` 0x5aaba0..0x5aad60).
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BeastMothershipSpec {}

/// A ship being infected by the Beast (+0x4e4 and around); every ship has one.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Infection {
    /// +0x4e4: infection timer (0 = not being infected).
    pub timer: f32,
}

/// `univCheckBeastInfectionComplete` (0x53e8f0): true when the infection converted the ship.
pub fn check_infection_complete(u: &mut Universe, id: ShipId) -> bool {
    let _ = (u, id);
    false
}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BeastStatics {}

impl BeastStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
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

pub fn cascade_remove_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
