//! Bullets and beams: creation when a gun fires (0x487120), the per-frame update
//! (`univUpdateAllPosVelBullets` 0x53e0c0, `physUpdateBulletPosVel` 0x4dfa50, the homing of
//! energy bolts 0x53dd40) and their removal.
//!
//! The weapon technology decides the kind: projectile, energy and plasma bolts and big-gun
//! shells fly (`WEAPON_PROJECTILE`, `ENERGY`, `PLASMABOMB`, `BIGGUN`); ion beams follow their
//! gun every frame and lasers stay where they were fired (`WEAPON_ION`, `LASER`, and the beast
//! infection, which is a beam too).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::{Matrix, Vector};

use crate::blobs::Objects;
use crate::gun;
use crate::object::{Bullet, BulletId, ObjRef, PosInfo, ShipId};
use crate::universe::Universe;

/// Weapon technologies (`WEAPON_*`).
pub mod weapon {
    pub const PROJECTILE: u32 = 0;
    pub const ENERGY: u32 = 1;
    pub const ION: u32 = 2;
    pub const PLASMABOMB: u32 = 3;
    pub const LASER: u32 = 4;
    pub const MISSILE: u32 = 5;
    pub const MINE: u32 = 6;
    pub const BIGGUN: u32 = 7;
    pub const BEASTINFECT: u32 = 8;
}

/// Whether a weapon technology fires beams.
pub fn is_beam(w: u32) -> bool {
    matches!(w, weapon::ION | weapon::LASER | weapon::BEASTINFECT)
}

/// Bullet creation (0x487120). `source` replaces the gun as the origin (the bullet then starts
/// at the source's position and points from the source to the target). Returns the new
/// bullet, or `None` if the ship or gun do not exist.
pub fn create(u: &mut Universe, id: ShipId, gi: usize, source: Option<ShipId>, target: Option<ShipId>, damage: f32) -> Option<BulletId> {
    let statics = u.statics.clone();
    let s = u.ships.get(id)?;
    let stat = statics.ship(s.ship_type)?;
    let gs = stat.guns.as_ref()?.get(gi)?;
    let g = s.guns[gi];
    let burst = s.special_flags & 0x40 != 0;
    let up = gun::upgraded(&g, gs);
    let target_ship = target.and_then(|t| u.ships.get(t));

    // Heading: the barrel in world space, or source to target.
    let heading = match (source.and_then(|x| u.ships.get(x)), target_ship) {
        (Some(src), Some(t)) => {
            let mut h = Vector::new(t.posinfo.position.x - src.posinfo.position.x, t.posinfo.position.y - src.posinfo.position.y, t.posinfo.position.z - src.posinfo.position.z);
            h.normalize();
            h
        }
        _ => s.rotinfo.coordsys.mul_vec(g.heading),
    };
    let len = gs.bullet_length;
    let length_vec = Vector::new(st(ld(len) * ld(heading.x)), st(ld(len) * ld(heading.y)), st(ld(len) * ld(heading.z)));
    let coordsys = Matrix::coord_sys_from_heading(heading);
    let mut posinfo = PosInfo::default();
    let (mut travel_dist, mut beam_travel_dist) = (0.0, 0.0);
    let mut speed = 0.0f32;
    match g.level {
        weapon::PROJECTILE | weapon::ENERGY | weapon::PLASMABOMB | weapon::BIGGUN => {
            speed = if burst { u.tweaks.combat.burst_speed } else { gun::bullet_speed(&g, gs) };
            posinfo.velocity = Vector::new(st(ld(speed) * ld(heading.x)), st(ld(speed) * ld(heading.y)), st(ld(speed) * ld(heading.z)));
            travel_dist = st(ld(u.phys_time_elapsed) * ld(speed));
        }
        weapon::ION | weapon::LASER | weapon::BEASTINFECT => {
            travel_dist = gs.bullet_length;
            beam_travel_dist = gs.bullet_length;
        }
        _ => {}
    }
    // Position: muzzle (plus the slaved-barrel offset) plus the barrel length.
    let position = if source.is_none() {
        let mut gp = s.rotinfo.coordsys.mul_vec(gs.position);
        if gs.offset.x != 0.0 {
            let o0 = g.cur_orientation.mul_vec(gs.offset);
            let o1 = s.rotinfo.coordsys.mul_vec(o0);
            gp = Vector::new(o1.x + gp.x, o1.y + gp.y, o1.z + gp.z);
        }
        let p = s.posinfo.position;
        let base = Vector::new(gp.x + p.x, gp.y + p.y, gp.z + p.z);
        let b = ld(gs.barrel_length);
        Vector::new(st(ld(heading.x) * b + ld(base.x)), st(ld(heading.y) * b + ld(base.y)), st(ld(heading.z) * b + ld(base.z)))
    } else {
        s.posinfo.position
    };
    posinfo.position = position;
    posinfo.is_moving = 1;
    posinfo.havent_calculated_dist = true;
    let bullet = Bullet {
        id: BulletId(0),
        flags: 0,
        posinfo,
        coordsys,
        coll_optimize_dist: 0.0,
        coll_my_blob: None,
        heading,
        length_vec,
        length_mag: gs.bullet_length,
        travel_dist,
        beam_travel_dist,
        weapon: g.level,
        burst,
        bullet_type: gun::bullet_type(&g, gs),
        time_lived: 0.0,
        total_lifetime: if up { gs.bullet_life_time2 } else { gs.bullet_life_time },
        owner: Some(id),
        gun: Some(gi),
        target: if burst { None } else { target },
        color: 0,
        mass: gs.bullet_mass,
        damage,
        updated_once: false,
        player: s.player,
        max_range: 0.0,
        dist_flown: 0.0,
        prev_position: position,
        dfg_flags: 0,
    };
    let _ = speed;
    let bid = u.bullets.insert(bullet);
    let target_blob = target.and_then(|t| u.ships.get(t)).and_then(|t| t.coll_my_blob);
    let mut objs = Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
    match target_blob {
        Some(b) => u.blobs.add_to_specific(b, ObjRef::Bullet(bid), &mut objs, &statics),
        None => u.blobs.add_to_nearest(ObjRef::Bullet(bid), &mut objs, &statics),
    }
    u.space_obj_list.push(ObjRef::Bullet(bid));
    u.bullet_list.push(bid);
    if let Some(t) = target.and_then(|t| u.ships.get_mut(t)) {
        t.firing_at_us = Some(id);
        t.recently_fired_upon = crate::object::RECENT_ATTACK_DURATION;
    }
    u.events.push(crate::universe::SimEvent::GunFired { ship: id, gun: gi, bullet: bid });
    crate::effects::bullet_fired(u, id, gi, bid, target);
    Some(bid)
}

/// Homing of energy bolts (0x53dd40): after half a second a bolt steers towards its target,
/// leading it when close, and matches its velocity at very short range.
fn energy_homing(u: &mut Universe, bid: BulletId) {
    let b = u.bullets.bullet(bid).clone();
    let Some(t) = b.target.and_then(|t| u.ships.get(t)).cloned() else { return };
    if !(ld(b.time_lived) > ld(0.5f32)) {
        return;
    }
    if matches!(t.ship_type, crate::statics::ShipType::sSwarmer | crate::statics::ShipType::bSwarmer) {
        return;
    }
    let tp = t.coll.coll_position;
    let bp = b.posinfo.position;
    let dx = st(ld(tp.x) - ld(bp.x));
    let dy = st(ld(tp.y) - ld(bp.y));
    let dz_ext = ld(tp.z) - ld(bp.z);
    let dz = st(dz_ext);
    let dist_sq = st((dz_ext * ld(dz) + ld(dy) * ld(dy)) + ld(dx) * ld(dx));
    let d = Vector::new(dx, dy, dz);
    let mut n = d;
    n.normalize();
    let first_n = n;
    let v = b.posinfo.velocity;
    let near = ld(dist_sq) < 40000.0;
    if !near {
        let dot = (ld(d.z) * ld(v.z) + ld(d.y) * ld(v.y)) + ld(d.x) * ld(v.x);
        if dot < ld(f32::from_bits(0xbf5d_b3d0)) {
            return;
        }
    }
    let speed_sq = (ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z);
    let mut speed = if speed_sq < 2_560_000.0 + 10_000.0 && speed_sq > 2_560_000.0 - 10_000.0 { 1600.0 } else { st(fsqrt(speed_sq)) };
    if near && ld(speed) > 0.0 {
        let k = fsqrt(ld(dist_sq)) / ld(speed);
        let tv = t.posinfo.velocity;
        let a = st(k * ld(tv.x));
        let bb = st(k * ld(tv.y));
        let lead = Vector::new(st(ld(a) + ld(d.x)), st(ld(bb) + ld(d.y)), st(k * ld(tv.z) + ld(d.z)));
        n = lead;
        n.normalize();
    }
    let bm = u.bullets.bullet_mut(bid);
    bm.posinfo.velocity = Vector::new(st(ld(n.x) * ld(speed)), st(ld(n.y) * ld(speed)), st(ld(n.z) * ld(speed)));
    bm.heading = n;
    if ld(dist_sq) < 10_000.0 {
        if !matches!(t.ship_type, crate::statics::ShipType::sSwarmer | crate::statics::ShipType::bSwarmer) {
            if ld(speed) > 150.0 {
                speed = st(ld(speed) / 2.0);
            }
            let tv = t.posinfo.velocity;
            let s = ld(speed);
            bm.posinfo.velocity = Vector::new(
                st(ld(tv.x) + ld(first_n.x) * s),
                st(ld(tv.y) + ld(st(ld(first_n.y) * s))),
                st(ld(tv.z) + ld(st(ld(first_n.z) * s))),
            );
        }
        let mut h = bm.posinfo.velocity;
        h.normalize();
        bm.heading = h;
    }
}

/// `physUpdateBulletPosVel` (0x4dfa50): moves the bullet (beams follow their gun); true when
/// its lifetime or range is used up.
pub fn update_pos_vel(u: &mut Universe, bid: BulletId) -> bool {
    let dt = u.phys_time_elapsed;
    let b = u.bullets.bullet(bid).clone();
    let mut d = Vector::ZERO;
    match b.weapon {
        weapon::ION => {
            if let (Some(owner), Some(gi)) = (b.owner, b.gun) {
                if u.ships.get(owner).is_some() {
                    let statics = u.statics.clone();
                    let gt = statics
                        .ship(u.ships.ship(owner).ship_type)
                        .and_then(|st| st.guns.as_ref()?.get(gi).map(|g| g.gun_type));
                    if let Some(t) = b.target {
                        if matches!(gt, Some(Some(crate::statics::GunType::Gimble | crate::statics::GunType::NewGimble))) && u.ships.get(t).is_some() {
                            gun::orient_gimble(u, owner, gi, t);
                        }
                    }
                    let s = u.ships.ship(owner);
                    if let Some(gs) = statics.ship(s.ship_type).and_then(|st| st.guns.as_ref()?.get(gi)) {
                        let g = s.guns[gi];
                        let h = s.rotinfo.coordsys.mul_vec(g.heading);
                        let mut gp = s.rotinfo.coordsys.mul_vec(gs.position);
                        if gs.offset.x != 0.0 {
                            let o0 = g.cur_orientation.mul_vec(gs.offset);
                            let o1 = s.rotinfo.coordsys.mul_vec(o0);
                            gp = Vector::new(o1.x + gp.x, o1.y + gp.y, o1.z + gp.z);
                        }
                        let op = s.posinfo.position;
                        let bm = u.bullets.bullet_mut(bid);
                        bm.heading = h;
                        bm.coordsys = Matrix::coord_sys_from_heading(h);
                        let k = ld(bm.beam_travel_dist);
                        bm.length_vec = Vector::new(st(k * ld(h.x)), st(k * ld(h.y)), st(k * ld(h.z)));
                        bm.posinfo.position = Vector::new(gp.x + op.x, gp.y + op.y, gp.z + op.z);
                    }
                }
            }
        }
        weapon::LASER => {}
        weapon::BEASTINFECT => crate::ships::beast::cascade_effect_update(u, bid),
        _ => {
            let v = b.posinfo.velocity;
            d = Vector::new(st(ld(dt) * ld(v.x)), st(ld(dt) * ld(v.y)), st(ld(dt) * ld(v.z)));
            let bm = u.bullets.bullet_mut(bid);
            bm.prev_position = bm.posinfo.position;
            let p = &mut bm.posinfo.position;
            p.x += d.x;
            p.y += d.y;
            p.z += d.z;
        }
    }
    let bm = u.bullets.bullet_mut(bid);
    let lived = ld(dt) + ld(bm.time_lived);
    bm.posinfo.is_moving = 1;
    bm.time_lived = st(lived);
    if bm.updated_once {
        if lived > ld(bm.total_lifetime) {
            return true;
        }
    } else {
        bm.updated_once = true;
    }
    if bm.bullet_type == 0x12 {
        let flown = fsqrt((ld(d.z) * ld(d.z) + ld(d.y) * ld(d.y)) + ld(d.x) * ld(d.x)) + ld(bm.dist_flown);
        bm.dist_flown = st(flown);
        if flown > ld(bm.max_range) {
            return true;
        }
        // (DFG fields would also end the shell here.)
    }
    false
}

/// `univUpdateAllPosVelBullets` (0x53e0c0).
pub fn update_all(u: &mut Universe) {
    crate::effects::begin_bullet_pass(u);
    let list = u.bullet_list.clone();
    for bid in list {
        let Some(b) = u.bullets.get(bid) else { continue };
        if b.flags & crate::object::sof::DONT_APPLY_PHYSICS != 0 {
            continue;
        }
        if b.weapon == weapon::ENERGY {
            energy_homing(u, bid);
        }
        if update_pos_vel(u, bid) {
            crate::effects::bullet_expired(u, bid);
            // (Burst bullets explode at the end of their flight: pointExplosionInSpace with
            // the burst radius; the burst attack is not modelled.)
            delete(u, bid);
        }
    }
}

/// Removes a bullet from the universe.
pub fn delete(u: &mut Universe, bid: BulletId) {
    crate::effects::bullet_removed(u, bid);
    crate::ships::bullet_freed(u, bid);
    u.blobs.object_died(ObjRef::Bullet(bid));
    u.bullet_list.retain(|&b| b != bid);
    u.space_obj_list.retain(|&o| o != ObjRef::Bullet(bid));
    u.bullets.remove(bid);
}
