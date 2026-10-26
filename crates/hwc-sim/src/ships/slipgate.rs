//! Slipgates (`Slipgate.c`, Cataclysm only): the gates the level files place (`Slipgate`
//! lines, `scriptSetSlipgateCB` 0x49c680), paired two by two, through which ships docking with
//! purpose "slipgate" travel to the partner gate.
//!
//! A gate opens when a ship flies into its cone (`SlipgateOpen`: the opening effect runs for
//! 2.25 s, the gate is then open), stays open while ships keep using it, and closes 4 s after
//! the last use (0.625 s closing). The ship waits 1.5 s at the open gate, is put inside the
//! partner gate and launched from it (`ShipDocksAtShip` states 0x10, 0xd, 10, 0x11, 0xe). Ships
//! guarding a ship that enters, or following it in a military parade, are sent after it
//! (`SlipgateDockShip`).

use hwc_math::fpu::{ld, st};

use crate::object::ShipId;
use crate::statics::ShipType;
use crate::universe::Universe;

/// Effect states (`+0x63c`).
pub mod gate_state {
    pub const CLOSED: u8 = 0;
    pub const OPENING: u8 = 1;
    pub const OPEN: u8 = 2;
    pub const CLOSING: u8 = 3;
}

/// `SlipgateSpec` (+0x638..).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SlipgateSpec {
    /// +0x638: the paired gate.
    pub partner: Option<ShipId>,
    /// +0x63c: effect state.
    pub state: u8,
    /// +0x640: when the gate was last asked to open (0: not since it closed).
    pub last_open: f32,
    /// +0x664: when the opening / closing effect ends (0: none running).
    pub state_end: f32,
    /// +0x65c: ships entering since the last housekeeping, whose escorts follow them.
    pub queue: Vec<ShipId>,
}

/// The gate types (`0x70..0x72`).
pub fn is_slipgate_type(t: ShipType) -> bool {
    matches!(t, ShipType::dSlipgateSmall | ShipType::dSlipgateMedium | ShipType::dSlipgateLarge)
}

fn spec(u: &Universe, id: ShipId) -> Option<&SlipgateSpec> {
    match &u.ships.get(id)?.spec {
        crate::ships::ShipSpec::Slipgate(s) => Some(s),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut SlipgateSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        crate::ships::ShipSpec::Slipgate(s) => Some(s),
        _ => None,
    }
}

/// `SlipgateInit`.
pub fn init(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.spec = crate::ships::ShipSpec::Slipgate(SlipgateSpec::default());
    }
}

/// The paired gate.
pub fn partner(u: &Universe, id: ShipId) -> Option<ShipId> {
    spec(u, id).and_then(|s| s.partner)
}

/// `SlipgateSetPartner`.
pub fn set_partner(u: &mut Universe, id: ShipId, partner: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.partner = Some(partner);
    }
}

/// The gate's effect state.
pub fn state(u: &Universe, id: ShipId) -> u8 {
    spec(u, id).map_or(gate_state::CLOSED, |s| s.state)
}

/// `SlipgateSetEffectState` (0x5c1920): the state and the end time of its effect.
pub fn set_effect_state(u: &mut Universe, id: ShipId, state: u8) {
    let now = u.total_time_elapsed;
    let Some(s) = spec_mut(u, id) else { return };
    match state {
        gate_state::CLOSED => s.state_end = 0.0,
        gate_state::OPENING | gate_state::OPEN => s.state_end = st(ld(now) + 2.25),
        gate_state::CLOSING => s.state_end = st(ld(now) + 0.625),
        _ => return,
    }
    s.state = state;
}

/// `SlipgateOpen` (0x5c1a60): asks the gate (and its partner, unless the gate is hidden) to
/// open; true once both are open.
pub fn open(u: &mut Universe, id: ShipId) -> bool {
    let now = u.total_time_elapsed;
    let hidden = u.ships.get(id).is_some_and(|s| s.flags & crate::object::sof::NIS_SHIP != 0);
    let partner = partner(u, id);
    let paired = !hidden;
    if paired {
        if let Some(s) = spec_mut(u, id) {
            s.last_open = now;
        }
        if let Some(p) = partner {
            if let Some(ps) = spec_mut(u, p) {
                ps.last_open = now;
            }
        }
    } else if let Some(s) = spec_mut(u, id) {
        s.last_open = 0.0;
    }
    if state(u, id) == gate_state::CLOSED {
        set_effect_state(u, id, gate_state::OPENING);
    }
    if paired {
        if let Some(p) = partner {
            if state(u, p) == gate_state::CLOSED {
                set_effect_state(u, p, gate_state::OPENING);
            }
        }
    }
    let partner_open = !paired || partner.is_none_or(|p| state(u, p) == gate_state::OPEN);
    state(u, id) == gate_state::OPEN && partner_open
}

/// `SlipgateAddShipToQueue` (0x5c1d30).
pub fn add_ship_to_queue(u: &mut Universe, gate: ShipId, ship: ShipId) {
    if let Some(s) = spec_mut(u, gate) {
        if !s.queue.contains(&ship) {
            s.queue.push(ship);
        }
    }
}

/// `SlipgateDockShip` (0x5c1d50): the ships guarding `ship`, or in the military parade it
/// leads, that can use the gate are sent through it too.
fn dock_followers(u: &mut Universe, gate: ShipId, ship: ShipId) {
    let mut followers: Vec<ShipId> = Vec::new();
    for c in u.commands.iter() {
        let guarding = c.attributes & crate::command::attr::PROTECTING != 0 && c.protect.contains(&ship);
        let parade = c.order == crate::command::order::MILITARY_PARADE && c.selection.first() == Some(&ship);
        if guarding || parade {
            followers.extend(c.selection.iter().copied().filter(|&s| s != ship));
        }
    }
    followers.truncate(1000);
    let ok: Vec<ShipId> = followers.into_iter().filter(|&f| crate::dock::ship_can_dock_with(u, f, gate, crate::dock::purpose::SLIPGATE, false)).collect();
    if !ok.is_empty() {
        crate::dock::cl_dock(u, &ok, crate::dock::purpose::SLIPGATE, Some(gate));
    }
}

/// `SlipgateHouseKeep` (0x5c1e50).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let queue = spec_mut(u, id).map(|s| std::mem::take(&mut s.queue)).unwrap_or_default();
    for ship in queue {
        if u.ships.get(ship).is_some() {
            dock_followers(u, id, ship);
        }
    }
    let now = u.total_time_elapsed;
    let Some(s) = spec(u, id).cloned() else { return };
    // Four seconds after the last use the gate (and its partner) close.
    if s.last_open != 0.0 && st(ld(s.last_open) + 4.0) < now {
        set_effect_state(u, id, gate_state::CLOSING);
        let hidden = u.ships.get(id).is_some_and(|sh| sh.flags & crate::object::sof::NIS_SHIP != 0);
        if !hidden {
            if let Some(p) = s.partner {
                set_effect_state(u, p, gate_state::CLOSING);
            }
        }
        if let Some(sp) = spec_mut(u, id) {
            sp.last_open = 0.0;
        }
    }
    let Some(s) = spec(u, id).cloned() else { return };
    if matches!(s.state, gate_state::OPENING | gate_state::CLOSING) && s.state_end != 0.0 && s.state_end < now {
        if let Some(sp) = spec_mut(u, id) {
            sp.state = if s.state == gate_state::OPENING { gate_state::OPEN } else { gate_state::CLOSED };
            sp.state_end = 0.0;
        }
    }
}

/// `SlipgateRemoveShipReferences`.
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.queue.retain(|&q| q != gone);
        if s.partner == Some(gone) {
            s.partner = None;
        }
    }
}
