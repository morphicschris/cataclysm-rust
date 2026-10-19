//! The minelayer corvette and mine walls (`MinelayerCorvette` module, 0x5b4cc0..0x5b5d80): tMinelayerCorvette.
//!
//! (Stub: to be filled in.)

use crate::object::ShipId;
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MinelayerSpec {}

/// Per-type statics (`CustShipStaticInit`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MinelayerStatics {}

impl MinelayerStatics {
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

pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let _ = (u, id);
    true
}

pub fn order_changed(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn housekeep(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

pub fn died(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}
/// The squared distance a forced mine must get from its layer before it flies to its wall
/// point (the layer type's custom statics +0x68, `MineClearDistance`).
pub fn mine_clear_distance_sq(u: &Universe, layer: crate::statics::ShipType) -> f32 {
    let _ = (u, layer);
    0.0
}

/// A mine of a mine formation died (`listRemoveNode` of the mine's formation link).
pub fn mine_died(u: &mut Universe, formation: u32, mine: crate::object::MissileId) {
    let _ = (u, formation, mine);
}

/// The forced-drop data `missileShoot` takes from a layer laying a wall (+0x658 == 1).
pub fn forced_drop(u: &Universe, layer: ShipId) -> Option<crate::missile::ForcedMine> {
    let _ = (u, layer);
    None
}

/// A mine formation (a wall of mines laid by a minelayer; `MineFormationInfo`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MineFormation {}

/// `univUpdateMineWallFormations` (0x5b5c50), once per update after the command layer.
pub fn update_mine_wall_formations(u: &mut Universe) {
    let _ = u;
}
