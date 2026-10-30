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

/// `SlipgateInit` (0x5c1bd0): the gate cannot be selected, is invulnerable, never
/// retaliates and lets allies dock (its effect sizes and colour, +0x648..0x650, are
/// [`effect_params`]).
pub fn init(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.spec = crate::ships::ShipSpec::Slipgate(SlipgateSpec::default());
        apply_init_flags(s);
    }
}

/// The flags `SlipgateInit` sets. Saves made before they were ported get them on load.
pub fn apply_init_flags(s: &mut crate::object::Ship) {
    s.special_flags |= 0x0004_0000;
    s.flags2 |= crate::object::flags2::INVULNERABLE;
    s.flags &= !crate::object::sof::SELECTABLE;
    s.special_flags2 |= crate::base::special2::ALLY_DOCKABLE;
}

/// `SlipgateInit`'s per-type effect parameters: the radius of the opening and closing
/// effects (+0x64c), the radius of the permanent one (+0x648) and the colour (+0x650, RGBA).
pub fn effect_params(t: ShipType) -> Option<(f32, f32, [u8; 4])> {
    match t {
        ShipType::dSlipgateSmall => Some((1250.0, 5000.0, [255, 0, 0, 255])),
        ShipType::dSlipgateMedium => Some((2600.0, 10400.0, [255, 255, 0, 255])),
        ShipType::dSlipgateLarge => Some((11100.0, 44400.0, [0, 0, 255, 255])),
        _ => None,
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

/// The gate's effect (+0x644) among the per-ship effect slots.
const GATE_EFFECT: u32 = 0x644;
/// `EGT_SLIPGATE_OPENING_EFFECT`, `EGT_SLIPGATE_CLOSING_EFFECT` and
/// `EGT_SLIPGATE_PERMANENT_EFFECT` in the special-purpose effect table.
const OPENING_EFFECT_SLOT: usize = 29;
const CLOSING_EFFECT_SLOT: usize = 30;
const PERMANENT_EFFECT_SLOT: usize = 31;

/// `SlipgateSetEffectState` (0x5c1920): the state and the end time of its effect. The old
/// effect is deleted and the state's own started on the gate: the permanent glow while
/// closed, the opening vortex while opening or open, the closing one while closing.
pub fn set_effect_state(u: &mut Universe, id: ShipId, state: u8) {
    let now = u.total_time_elapsed;
    if spec(u, id).is_none() {
        return;
    }
    let slot = match state {
        gate_state::CLOSED => PERMANENT_EFFECT_SLOT,
        gate_state::OPENING | gate_state::OPEN => OPENING_EFFECT_SLOT,
        gate_state::CLOSING => CLOSING_EFFECT_SLOT,
        _ => return,
    };
    crate::effects::delete_slot(u, id, GATE_EFFECT);
    if let Some(s) = spec_mut(u, id) {
        s.state_end = match state {
            gate_state::CLOSED => 0.0,
            gate_state::CLOSING => st(ld(now) + 0.625),
            _ => st(ld(now) + 2.25),
        };
    }
    start_effect(u, id, slot, state);
    if let Some(s) = spec_mut(u, id) {
        s.state = state;
    }
}

/// `etgEffectCreate(effect, gate, ..., EAF_Full, 3, radius, 0, colour)`: the closed gate's
/// effect gets the permanent radius (+0x648), the others the opening radius (+0x64c); the
/// line-of-fire offset is 0.
fn start_effect(u: &mut Universe, id: ShipId, slot: usize, state: u8) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let Some((open_radius, permanent_radius, colour)) = effect_params(t) else { return };
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(effect) = fx.pick_clamped(fx.tables.special_effect(slot)) else { return };
    let radius = if state == gate_state::CLOSED { permanent_radius } else { open_radius };
    let colour = u32::from_le_bytes(colour);
    let params = [crate::effects::pf(radius), 0, colour];
    let e = crate::effects::create(u, effect, Some(id.into()), None, None, None, 0.0, crate::effects::attach::FULL, &params);
    crate::effects::set_slot(u, id, GATE_EFFECT, e);
}

/// Whether the gate has its effect (+0x644 set). Without an effect system, or without the
/// slipgate effects in its tables, every gate counts as having one, so the gates still open
/// and close on time.
fn has_effect(u: &Universe, id: ShipId) -> bool {
    let Some(fx) = u.effects.as_ref() else { return true };
    if fx.pick_clamped(fx.tables.special_effect(PERMANENT_EFFECT_SLOT)).is_none() {
        return true;
    }
    crate::effects::slot(u, id, GATE_EFFECT).is_some_and(|e| crate::effects::exists(u, e))
}

/// `SlipgateClose`: the gate's effect goes with it.
pub fn close(u: &mut Universe, id: ShipId) {
    crate::effects::delete_slot(u, id, GATE_EFFECT);
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
    if has_effect(u, id) && s.last_open != 0.0 && st(ld(s.last_open) + 4.0) < now {
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
        // A gate that has closed loses the closing effect (the permanent one starts below).
        if s.state == gate_state::CLOSING {
            crate::effects::delete_slot(u, id, GATE_EFFECT);
        }
    }
    // A gate without its effect gets its state's (at level start the permanent one).
    if !has_effect(u, id) {
        let state = state(u, id);
        set_effect_state(u, id, state);
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
