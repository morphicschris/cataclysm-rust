//! Sentinels and their force fields (`Sentinel` module, 0x5c0490..0x5c18d0): sSentinel, bSentinel.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SentinelSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SentinelStatics {}

impl SentinelStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
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

pub fn formation_changed(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
/// The force-field test of `collCheckMissileShipColl`: a sentinel with its field up stops an
/// enemy missile that crosses a field segment (the missile is destroyed).
pub fn missile_hits_field(u: &mut Universe, sentinel: ShipId, missile: &crate::missile::Missile) -> bool {
    let _ = (u, sentinel, missile);
    false
}

/// The sentinels' part of `univShipThink`: every frame but each 32nd, a field hit timer that
/// has run out clears the hit count (+0x97c / +0x980).
pub fn think(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
