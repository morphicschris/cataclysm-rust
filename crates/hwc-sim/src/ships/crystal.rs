//! Crystals (`Crystal` module, 0x5aaa70..0x5aab60): dCrystal0, dCrystal1. Salvageable
//! resource crystals that cannot be selected; one that is destroyed rather than processed at
//! a depot goes up in a shock wave.

use crate::object::{sof, ShipId};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

/// The disable reason of a crystal (`ShipSetDisableReason(ship, 1, 0xc)`).
pub const DISABLE_REASON_CRYSTAL: usize = 0xc;
/// `specialFlags` (+0x1c8) bit a crystal sets at creation.
pub const SPECIAL_CRYSTAL: u32 = 0x4_0000;

/// Per-ship state (+0x638..+0x640).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CrystalSpec {
    /// +0x638: resource units it still holds (handed out while a depot processes it).
    pub resources: i32,
    /// +0x63c: it was processed (no explosion when it goes).
    pub processed: bool,
    /// +0x640: a depot is processing it.
    pub processing: bool,
}

/// Per-type statics (`CrystalStaticInit`, 0x5aaaa0: `resourceValue`, at 0x91b670 / 0x91b674).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CrystalStatics {
    pub resource_value: i32,
}

impl CrystalStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for (k, v) in &stat.custom_entries {
            if k == "resourceValue" {
                if let Some(x) = crate::statics::cscan::scan_i32(v) {
                    s.resource_value = x;
                }
            }
        }
        // (A crystal without a resource value only draws a warning.)
        s
    }
}

/// `CrystalIsCrystal` (0x5aaa70).
pub fn is_crystal(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| is_crystal_type(s.ship_type))
}

pub fn is_crystal_type(t: ShipType) -> bool {
    matches!(t, ShipType::dCrystal0 | ShipType::dCrystal1)
}

fn statics(u: &Universe, t: ShipType) -> CrystalStatics {
    match crate::ships::type_statics(u, t) {
        crate::ships::TypeStatics::Crystal(c) => c.clone(),
        _ => CrystalStatics::default(),
    }
}

/// `CrystalInit` (0x5aab10): disabled for good, not selectable, holding its resource value.
pub fn init(u: &mut Universe, id: ShipId) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let value = statics(u, t).resource_value;
    let s = u.ships.ship_mut(id);
    crate::base::set_disable_reason(s, true, DISABLE_REASON_CRYSTAL);
    s.flags &= !sof::SELECTABLE;
    s.special_flags |= SPECIAL_CRYSTAL;
    s.spec = crate::ships::ShipSpec::Crystal(CrystalSpec { resources: value, processed: false, processing: false });
}

/// The crystal's state.
pub fn spec(u: &Universe, id: ShipId) -> Option<&CrystalSpec> {
    match &u.ships.get(id)?.spec {
        crate::ships::ShipSpec::Crystal(c) => Some(c),
        _ => None,
    }
}

/// Mutable [`spec`] (for the depot that processes it).
pub fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut CrystalSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        crate::ships::ShipSpec::Crystal(c) => Some(c),
        _ => None,
    }
}

/// `CrystalDied` (0x5aab60): an unprocessed crystal explodes (`swCreate(3, ...)`), credited
/// to the player that destroyed it (ship +0x60c: `whoKilledMe` of a player kill, else -1).
pub fn died(u: &mut Universe, id: ShipId) {
    if !spec(u, id).is_some_and(|c| !c.processed) {
        return;
    }
    let s = u.ships.ship(id);
    let killer = if (crate::damage::how::KILLED_BY_PLAYER..=crate::damage::how::KILLED_BY_PLAYER + 2).contains(&s.how_did_i_die) { s.who_killed_me } else { -1 };
    let pos = s.posinfo.position;
    crate::shockwave::create(u, crate::shockwave::kind::CRYSTAL, pos, killer as usize);
}
