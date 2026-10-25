//! `Clamp.c`: ships fixed to another ship. A clamped ship (the slave) keeps its position and
//! orientation relative to its host; a host can carry any number of slaves (Cataclysm's change
//! from HW1's single clamped object: the host's `clampInfo` holds a slave list). Mothership
//! modules, latched workers, salvagers, external-construction hulls and docked fighters on
//! latch points are all clamped.

use hwc_math::fpu::{ld, st};
use hwc_math::{Matrix, Vector};

use crate::base::ClampInfo;
use crate::object::{sof, ShipId};
use crate::universe::Universe;

/// `clampObjToObj` (0x4301d0): clamp `slave` to `host` where it is now.
pub fn clamp_obj_to_obj(u: &mut Universe, slave: ShipId, host: ShipId) {
    let h = u.ships.ship(host);
    let (hpos, hsys) = (h.posinfo.position, h.rotinfo.coordsys);
    let s = u.ships.ship(slave);
    let d = Vector::new(
        st(ld(s.posinfo.position.x) - ld(hpos.x)),
        st(ld(s.posinfo.position.y) - ld(hpos.y)),
        st(ld(s.posinfo.position.z) - ld(hpos.z)),
    );
    // Position in host coordinates (vector times matrix), orientation (slave^T x host)^T.
    let rel_position = hsys.vec_mul(d);
    let rel_coordsys = s.rotinfo.coordsys.transpose().mul_mat(&hsys).transpose();
    let s = u.ships.ship_mut(slave);
    let info = s.base.clamp.get_or_insert_with(ClampInfo::default);
    info.host = Some(host);
    info.rel_position = rel_position;
    info.rel_coordsys = rel_coordsys;
    s.flags |= sof::CLAMPED;
    let h = u.ships.ship_mut(host);
    let hinfo = h.base.clamp.get_or_insert_with(ClampInfo::default);
    hinfo.slaves.get_or_insert_with(Vec::new).push(slave);
}

/// `unClampObj` (0x430320): release `slave` from its host.
pub fn unclamp(u: &mut Universe, slave: ShipId) {
    let Some(info) = u.ships.get_mut(slave).and_then(|s| s.base.clamp.as_mut()) else { return };
    let host = info.host.take();
    if let Some(host) = host {
        if let Some(h) = u.ships.get_mut(host) {
            if let Some(hinfo) = h.base.clamp.as_mut() {
                if let Some(list) = hinfo.slaves.as_mut() {
                    // selSelectionRemoveSingleShip: swap-remove.
                    if let Some(i) = list.iter().position(|&x| x == slave) {
                        list.swap_remove(i);
                    }
                    if list.is_empty() {
                        hinfo.slaves = None;
                        if hinfo.host.is_none() {
                            h.base.clamp = None;
                            h.flags &= !sof::CLAMPED;
                        }
                    }
                }
            }
        }
    }
    let s = u.ships.ship_mut(slave);
    if let Some(info) = s.base.clamp.as_mut() {
        if info.slaves.as_ref().is_some_and(|l| l.is_empty()) {
            info.slaves = None;
        }
        if info.slaves.is_none() && info.host.is_none() {
            s.base.clamp = None;
            s.flags &= !sof::CLAMPED;
        }
    }
}

/// `unClampAllSlaves` (0x4303f0): release everything clamped to `host` (the first slave each
/// time, until the host's clamp info is gone).
pub fn unclamp_all_slaves(u: &mut Universe, host: ShipId) {
    loop {
        let Some(first) = u.ships.get(host).and_then(|h| h.base.clamp.as_ref()).and_then(|c| c.slaves.as_ref()).and_then(|l| l.first().copied())
        else {
            return;
        };
        unclamp(u, first);
    }
}

/// The host of a clamped ship.
pub fn host_of(u: &Universe, id: ShipId) -> Option<ShipId> {
    u.ships.get(id).and_then(|s| s.base.clamp.as_ref()).and_then(|c| c.host)
}

/// The slaves clamped to a ship.
pub fn slaves_of(u: &Universe, id: ShipId) -> Vec<ShipId> {
    u.ships.get(id).and_then(|s| s.base.clamp.as_ref()).and_then(|c| c.slaves.clone()).unwrap_or_default()
}

/// The pose a slave takes from its host: position, orientation.
fn pose_from_host(host_pos: Vector, host_sys: &Matrix, info: &ClampInfo) -> (Vector, Matrix) {
    let off = host_sys.mul_vec(info.rel_position);
    let pos = Vector::new(st(ld(off.x) + ld(host_pos.x)), st(ld(off.y) + ld(host_pos.y)), st(ld(off.z) + ld(host_pos.z)));
    (pos, host_sys.mul_mat(&info.rel_coordsys))
}

/// `updateClampedObject` (0x430430): every slave of `host` runs its physics step, then takes
/// its position, orientation and velocity from the host; slaves of slaves follow.
pub fn update_clamped_object(u: &mut Universe, host: ShipId) {
    let statics = u.statics.clone();
    let mut i = 0;
    loop {
        let slaves = slaves_of(u, host);
        let Some(&sid) = slaves.get(i) else { return };
        i += 1;
        let Some(stat) = u.ships.get(sid).and_then(|s| statics.ship(s.ship_type)) else { continue };
        let ctx = crate::physics::PhysContext {
            dt: u.phys_time_elapsed,
            update_counter: u.update_counter,
            single_player: u.single_player,
            world_bounds: u.world_bounds,
            max_vel: u.max_velocity_context(u.ships.ship(sid)),
            now: u.total_time_elapsed,
        };
        let tw = u.tweaks.clone();
        crate::physics::update_ship(u.ships.ship_mut(sid), stat, &tw, &ctx);
        follow_host(u, sid);
        if u.ships.get(sid).and_then(|s| s.base.clamp.as_ref()).is_some_and(|c| c.slaves.is_some()) {
            update_clamped_object(u, sid);
        }
    }
}

/// `updateClampedObjectNoPhys` (0x4305a0) and the tail of `updateClampedObject`: pose and
/// velocity from the host.
pub fn follow_host(u: &mut Universe, sid: ShipId) {
    let statics = u.statics.clone();
    let Some(info) = u.ships.get(sid).and_then(|s| s.base.clamp.clone()) else { return };
    let Some(host) = info.host.and_then(|h| u.ships.get(h)) else { return };
    let (hpos, hsys, hvel) = (host.posinfo.position, host.rotinfo.coordsys, host.posinfo.velocity);
    let (pos, sys) = pose_from_host(hpos, &hsys, &info);
    let s = u.ships.ship_mut(sid);
    s.posinfo.position = pos;
    s.rotinfo.coordsys = sys;
    s.posinfo.velocity = hvel;
    if let Some(stat) = statics.ship(s.ship_type) {
        crate::physics::update_obj_rot_info(s, stat);
    }
}

/// `updateClampSlavesFromHost` (0x430640): snap every slave to its host (no velocity, no
/// recursion).
pub fn update_slaves_from_host(u: &mut Universe, host: ShipId) {
    let (hpos, hsys) = {
        let h = u.ships.ship(host);
        (h.posinfo.position, h.rotinfo.coordsys)
    };
    for sid in slaves_of(u, host) {
        let Some(info) = u.ships.get(sid).and_then(|s| s.base.clamp.clone()) else { continue };
        let (pos, sys) = pose_from_host(hpos, &hsys, &info);
        let s = u.ships.ship_mut(sid);
        s.posinfo.position = pos;
        s.rotinfo.coordsys = sys;
    }
}

/// Whether the per-ship physics step of `univUpdateAllPosVelShips` is replaced by the host's
/// `updateClampedObject`: the ship is the first slave of a ship host that is not clamped
/// itself. The host is then queued (returned) for the clamped update.
pub fn physics_deferred_to_host(u: &Universe, id: ShipId) -> Option<ShipId> {
    let host = host_of(u, id)?;
    let h = u.ships.get(host)?;
    let hinfo = h.base.clamp.as_ref()?;
    if hinfo.host.is_some() {
        return None;
    }
    if hinfo.slaves.as_ref()?.first() == Some(&id) {
        Some(host)
    } else {
        None
    }
}
