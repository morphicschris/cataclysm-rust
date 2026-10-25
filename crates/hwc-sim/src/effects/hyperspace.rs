//! Hyperspace effects (`HS.c`): the window effect of a ship entering or leaving hyperspace
//! (`hsStart`, `hsOrientEffect`, `hsContinue`, and the end of the collapse in `hsUpdate`).
//!
//! The hyperspace state itself (`hsState`, `clipt`, the rate) belongs to whoever moves the ship
//! through hyperspace (the single-player arrival in [`crate::mission`]); these functions do the
//! effect side and take that state as arguments. The effect is kept in the ship's slot
//! [`SLOT`] (`hsinfo->effect`, ship info +0x1c).

use hwc_fx::runtime::EffectId;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::object::ShipId;
use crate::universe::Universe;

use super::{attach, create, pf, slot, FxOwner};

/// Per-ship effect slot of the hyperspace window.
pub const SLOT: u32 = 1;

/// `hsState` values (ship info +0x40).
pub mod state {
    pub const INACTIVE: u32 = 0;
    pub const POPUP_INTO: u32 = 1;
    pub const POPUP_OUTOF: u32 = 2;
    pub const SLICING_INTO: u32 = 3;
    pub const SLICING_OUTOF: u32 = 4;
    pub const COLLAPSE_INTO: u32 = 5;
    pub const COLLAPSE_OUTOF: u32 = 6;
    pub const FINISHED: u32 = 7;
}

/// `HS_Box_Scale` of `hsOrientEffect` and `hsStart` (0x720b04) and the depth factor (0x720450).
const BOX_SCALE: f32 = 1.08;
const DEPTH_SCALE: f32 = 0.25;

/// `hsShouldDisplayEffect`: ships clamped to another ship (modules on the mothership) come
/// through with their host and show no window of their own.
pub fn should_display(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_none_or(|s| s.base.clamp.as_ref().is_none_or(|c| c.host.is_none()))
}

/// The ship's hyperspace window effect, if one is running.
pub fn effect(u: &Universe, id: ShipId) -> Option<EffectId> {
    slot(u, id, SLOT)
}

/// Mirror of the ship's hyperspace info for the damage effects (see
/// [`super::damage::DamageSlots::hs_state`]). Call whenever `shipHyperspaceState` or
/// `hsState` changes.
pub fn note_state(u: &mut Universe, id: ShipId, ship_state: u32, hs_state: u32) {
    if let Some(fx) = u.effects.as_mut() {
        let d = fx.damage.entry(id).or_default();
        d.hs_ship_state = ship_state;
        d.hs_state = hs_state;
    }
}

/// The effect part of `hsStart` (0x48a7e0), after the caller set `hsState` (`hs_state`) and
/// `clipt` 1: the damage effects stop, the old window is forgotten, and when `display`
/// (`hsShouldDisplayEffect`) a new window starts at the front of the ship's collision box
/// (the hyperspace event's first level; the original also needs the OpenGL clip-plane
/// feature, which every hardware renderer has).
pub fn start(u: &mut Universe, id: ShipId, hs_state: u32, display: bool) -> Option<EffectId> {
    super::damage::stop_effect(u, id, super::damage::ALL);
    super::set_slot(u, id, SLOT, None);
    let fx = u.effects.as_ref()?;
    if !display {
        return None;
    }
    let stat_fx = *fx.tables.hyperspace.as_ref()?.first()?;
    let s = u.ships.get(id)?;
    let stat = u.statics.ship(s.ship_type)?;
    let coll = &stat.header.coll;
    // (Ship scale: `magnitudeSquared`, always 1.)
    let scale = 1.0f32;
    let depth = st(ld(coll.forward_length) * ld(scale) - ld(coll.coll_sphere_offset.z));
    let offset = s.rotinfo.coordsys.mul_vec(Vector::new(0.0, 0.0, depth));
    let cp = s.coll.coll_position;
    let position = Vector::new(st(ld(cp.x) + ld(offset.x)), st(ld(cp.y) + ld(offset.y)), st(ld(cp.z) + ld(offset.z)));
    let width = st(ld(stat.hyperspace_box_scale) * ld(coll.right_length) * ld(BOX_SCALE));
    let thickness = st(ld(coll.forward_length) * ld(DEPTH_SCALE));
    let (velocity, coordsys) = (s.posinfo.velocity, s.rotinfo.coordsys);
    let e = create(u, stat_fx, Some(FxOwner::Ship(id)), Some(position), Some(velocity), Some(coordsys), 1.0, attach::NLIPS, &[0, pf(width), pf(thickness)])?;
    super::set_slot(u, id, SLOT, Some(e));
    orient(u, id, hs_state, 1.0);
    Some(e)
}

/// `hsOrientEffect` (0x48a690): shape the first particle of each of the window's particle
/// systems to the ship's box, opening (popping up) or closing (collapsing) with `clipt`.
pub fn orient(u: &mut Universe, id: ShipId, hs_state: u32, clipt: f32) {
    let Some(e) = effect(u, id) else { return };
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let coll = &stat.header.coll;
    let box_scale = stat.hyperspace_box_scale;
    let tumble = [
        st(ld(box_scale) * ld(coll.up_length) * ld(BOX_SCALE)),
        st(ld(box_scale) * ld(coll.right_length) * ld(BOX_SCALE)),
        st(ld(coll.forward_length) * ld(DEPTH_SCALE)),
    ];
    let open = match hs_state {
        state::POPUP_INTO | state::POPUP_OUTOF => Some(1.0 - ld(clipt)),
        state::COLLAPSE_INTO | state::COLLAPSE_OUTOF => Some(ld(clipt)),
        _ => None,
    };
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(blocks) = fx.world.effect(e).map(|ef| ef.blocks.clone()) else { return };
    for h in blocks {
        let Some(sys) = fx.world.systems.get_mut((h as usize).wrapping_sub(1)).and_then(Option::as_mut) else { continue };
        let Some(p) = sys.particles.first_mut() else { continue };
        p.w_vel = Vector::new(0.0, -1.0, 0.0);
        p.flags |= hwc_fx::particle::pflag::XYZSCALE;
        p.tumble = tumble;
        if let Some(k) = open {
            p.tumble[0] = st(k * ld(p.tumble[0]));
        }
    }
}

/// `hsContinue` (0x48aa20) for a displayed window, done once per update where the original
/// does it each time the ship is drawn: the window is re-shaped ([`orient`]) and placed along
/// the ship's heading where the clip plane currently is. `hyperspace_fails` is the
/// single-player `hyperspaceFails` flag (the window then stays at the box centre while it
/// collapses into hyperspace).
pub fn continue_effect(u: &mut Universe, id: ShipId, hs_state: u32, clipt: f32, hyperspace_fails: bool) {
    let Some(e) = effect(u, id) else { return };
    orient(u, id, hs_state, clipt);
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let coll = &stat.header.coll;
    let fwd = ld(coll.forward_length);
    // Distance along the heading and the plane's sign (`hsGetEquation`'s d and normal z).
    let (sign, d): (f64, f64) = match hs_state {
        state::POPUP_INTO => return,
        state::POPUP_OUTOF => (1.0, -fwd),
        // (The product goes to a double unrounded.)
        state::SLICING_INTO => (-1.0, fwd * ld(clipt)),
        state::SLICING_OUTOF => (1.0, -(fwd * ld(clipt))),
        state::COLLAPSE_INTO if !hyperspace_fails => (-1.0, -fwd),
        state::COLLAPSE_INTO => (1.0, 0.0),
        state::COLLAPSE_OUTOF => (1.0, fwd),
        _ => return,
    };
    let scale = 1.0f32;
    let depth = st(-(d * sign) * ld(scale) - ld(coll.coll_sphere_offset.z));
    let offset = s.rotinfo.coordsys.mul_vec(Vector::new(0.0, 0.0, depth));
    let cp = s.coll.coll_position;
    let position = Vector::new(st(ld(offset.x) + ld(cp.x)), st(ld(offset.y) + ld(cp.y)), st(ld(offset.z) + ld(cp.z)));
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(ef) = fx.world.effects.iter_mut().find(|x| x.id == e) else { return };
    ef.position = position;
    let blocks = ef.blocks.clone();
    for h in blocks {
        let Some(sys) = fx.world.systems.get_mut((h as usize).wrapping_sub(1)).and_then(Option::as_mut) else { continue };
        if let Some(p) = sys.particles.first_mut() {
            p.position = position;
        }
    }
}

/// The end of the collapse in `hsUpdate`: the window effect is deleted.
pub fn finish(u: &mut Universe, id: ShipId) {
    super::delete_slot(u, id, SLOT);
}
