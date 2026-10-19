//! Narrow-phase collision (`Collision.c` as compiled into Cataclysm): the oriented collision
//! box of a ship, line-versus-box tests, bullets and beams against ships, line of sight and
//! the bump collisions between ships.
//!
//! The box tests (`collUpdateCollRectangle` 0x4357a0, `collCheckRectLine` 0x4359e0) and the
//! bullet walk (`collCheckBulletTargetColl` 0x438880) follow the disassembly operation by
//! operation; beams, line of sight and bumps follow the decompiled code.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, lt_or_unordered, st, Ext};
use hwc_math::Vector;

use crate::object::{sof, Bullet, BulletId, Ship, ShipId};
use crate::statics::ShipStaticInfo;
use crate::universe::Universe;

/// `REALlyBig`.
const REALLY_BIG: f32 = f32::from_bits(0x6258_d727);

/// `collUpdateCollRectangle` (0x4357a0): the eight corners of the collision box relative to
/// the ship's position, in world orientation. Corner 0 is the box's minimum corner; corner 6
/// is the opposite one.
pub fn update_coll_rect(ship: &mut Ship, stat: &ShipStaticInfo) {
    let c = &stat.header.coll;
    if c.up_length == 0.0 {
        return;
    }
    let m = ship.rotinfo.coordsys.m;
    let c0 = ship.rotinfo.coordsys.mul_vec(c.coll_rect_offset);
    let (up, right, fwd) = (ld(c.up_length), ld(c.right_length), ld(c.forward_length));
    // Up along column 0 (x stays in a register), right along column 1 (x in a register),
    // forward along column 2 (all stored).
    let a0 = ld(m[0]) * up;
    let u1 = st(up * ld(m[1]));
    let u2 = st(up * ld(m[2]));
    let r0 = right * ld(m[3]);
    let r1 = st(right * ld(m[4]));
    let r2 = st(right * ld(m[5]));
    let f0 = st(fwd * ld(m[6]));
    let f1 = st(fwd * ld(m[7]));
    let f2 = st(fwd * ld(m[8]));

    let p1x = r0 + ld(c0.x);
    let rect1 = Vector::new(st(p1x), st(ld(r1) + ld(c0.y)), st(ld(r2) + ld(c0.z)));
    let rect2 = Vector::new(st(p1x + ld(f0)), st(ld(rect1.y) + ld(f1)), st(ld(rect1.z) + ld(f2)));
    let rect3 = Vector::new(st(ld(f0) + ld(c0.x)), st(ld(f1) + ld(c0.y)), st(ld(f2) + ld(c0.z)));
    let rect4 = Vector::new(st(a0 + ld(c0.x)), st(ld(u1) + ld(c0.y)), st(ld(u2) + ld(c0.z)));
    let rect5 = Vector::new(st(p1x + a0), st(ld(rect1.y) + ld(u1)), st(ld(rect1.z) + ld(u2)));
    let rect6 = Vector::new(st(ld(rect2.x) + a0), st(ld(rect2.y) + ld(u1)), st(ld(rect2.z) + ld(u2)));
    let rect7 = Vector::new(st(ld(rect3.x) + a0), st(ld(rect3.y) + ld(u1)), st(ld(rect3.z) + ld(u2)));
    ship.coll.rect = [c0, rect1, rect2, rect3, rect4, rect5, rect6, rect7];
}

/// `collCheckRectLine` (0x4359e0): where the ray `point + t * dir` (`dir` normalised) enters
/// the ship's collision box. Returns `(t, side)`: `t` is 0 when the point is inside the box and
/// -1 when the ray misses it within `length`; `side` is the face (`TRANS_*` numbering: 0/1 up
/// and down, 2/3 right and left, 4/5 forward and back) or -1.
pub fn rect_line(ship: &Ship, stat: &ShipStaticInfo, point: Vector, dir: Vector, length: f32) -> (f32, i32) {
    let c = &stat.header.coll;
    if c.up_length == 0.0 {
        return (-1.0, -1);
    }
    let lo = [c.coll_rect_offset.x, c.coll_rect_offset.y, c.coll_rect_offset.z];
    let hi = [
        st(ld(lo[0]) + ld(c.up_length)),
        st(ld(lo[1]) + ld(c.right_length)),
        st(ld(lo[2]) + ld(c.forward_length)),
    ];
    let p = ship.posinfo.position;
    let d = Vector::new(st(ld(point.x) - ld(p.x)), st(ld(point.y) - ld(p.y)), st(ld(point.z) - ld(p.z)));
    let lp = ship.rotinfo.coordsys.vec_mul(d);
    let lp = [lp.x, lp.y, lp.z];
    let mut side = [0i32; 3];
    let mut plane = [0f32; 3];
    let mut inside = true;
    for i in 0..3 {
        if lt_or_unordered(ld(lp[i]), ld(lo[i])) {
            side[i] = -1;
            plane[i] = lo[i];
            inside = false;
        } else if !(ld(lp[i]) <= ld(hi[i]) || ld(lp[i]).is_nan()) {
            plane[i] = hi[i];
            side[i] = 1;
            inside = false;
        } else {
            side[i] = 0;
        }
    }
    if inside {
        return (0.0, -1);
    }
    let ld_ = ship.rotinfo.coordsys.vec_mul(dir);
    let ldir = [ld_.x, ld_.y, ld_.z];
    let mut t = [0f32; 3];
    for i in 0..3 {
        t[i] = if side[i] != 0 && !(ld(ldir[i]) == 0.0 || ld(ldir[i]).is_nan()) {
            st((ld(plane[i]) - ld(lp[i])) / ld(ldir[i]))
        } else {
            -1.0
        };
    }
    let mut best = 0;
    for i in 1..3 {
        if ld(t[i]) > ld(t[best]) {
            best = i;
        }
    }
    let tb = t[best];
    if lt_or_unordered(ld(tb), 0.0) {
        return (-1.0, -1);
    }
    if !(ld(tb) <= ld(length) || ld(tb).is_nan()) {
        return (-1.0, -1);
    }
    for i in 0..3 {
        if i == best {
            continue;
        }
        let v = ld(tb) * ld(ldir[i]) + ld(lp[i]);
        if v <= ld(lo[i]) || v.is_nan() {
            return (-1.0, -1);
        }
        if lt_or_unordered(v, ld(hi[i])) {
            continue;
        }
        return (-1.0, -1);
    }
    (tb, best as i32 * 2 + i32::from(side[best] == -1))
}

/// `collCheckRectInRect` (0x435810 area): whether any corner of `b`'s box lies inside `a`'s.
fn rect_in_rect(a: &Ship, b: &Ship) -> bool {
    let m = a.rotinfo.coordsys.m;
    let pa = a.posinfo.position;
    let pb = b.posinfo.position;
    let a0 = a.coll.rect[0];
    let a6 = a.coll.rect[6];
    let eps = 1e-6f32;
    for corner in &b.coll.rect {
        let px = ld(corner.x) + ld(pb.x);
        let py = ld(corner.y) + ld(pb.y);
        let pz = ld(corner.z) + ld(pb.z);
        let d0 = [px - (ld(a0.x) + ld(pa.x)), py - (ld(a0.y) + ld(pa.y)), pz - (ld(a0.z) + ld(pa.z))];
        let d6 = [px - (ld(a6.x) + ld(pa.x)), py - (ld(a6.y) + ld(pa.y)), pz - (ld(a6.z) + ld(pa.z))];
        let dot = |d: [Ext; 3], k: usize| d[0] * ld(m[k]) + d[1] * ld(m[k + 1]) + d[2] * ld(m[k + 2]);
        let e = ld(eps);
        if dot(d6, 6) <= e && -e <= dot(d0, 6) && dot(d6, 3) <= e && -e <= dot(d0, 3) && dot(d6, 0) <= e && -e <= dot(d0, 0) {
            return true;
        }
    }
    false
}

/// `collCheckRectBumpCollision`: the boxes overlap (a corner of one inside the other, or the
/// main diagonal of `a` crossing `b`). Types without a box always count as touching.
fn rect_bump(a: &Ship, astat: &ShipStaticInfo, b: &Ship, bstat: &ShipStaticInfo) -> bool {
    if astat.header.coll.up_length == 0.0 || bstat.header.coll.up_length == 0.0 {
        return true;
    }
    if rect_in_rect(a, b) || rect_in_rect(b, a) {
        return true;
    }
    let p = a.posinfo.position;
    let c0 = a.coll.rect[0];
    let c6 = a.coll.rect[6];
    let start = Vector::new(st(ld(p.x) + ld(c0.x)), st(ld(p.y) + ld(c0.y)), st(ld(p.z) + ld(c0.z)));
    let diag = astat.header.coll.diagonal_length;
    let inv = st(1.0 / ld(diag));
    let dir = Vector::new(
        st(ld(inv) * (ld(c6.x) - ld(c0.x))),
        st(ld(inv) * (ld(c6.y) - ld(c0.y))),
        st(ld(inv) * (ld(c6.z) - ld(c0.z))),
    );
    rect_line(b, bstat, start, dir, diag).0 >= 0.0
}

/// `allianceIsShipAlly`: same player (alliances are not modelled).
fn is_ally(s: &Ship, player: usize) -> bool {
    s.player == player
}

/// `collCheckLineOfSight` (0x437dd0): false when a friendly ship's box lies on the line from
/// `pos` along `dir` before the target (or anywhere along it without a target).
pub fn line_of_sight(u: &Universe, id: ShipId, target: Option<ShipId>, pos: Vector, dir: Vector) -> bool {
    let Some(me) = u.ships.get(id) else { return true };
    let mut length = REALLY_BIG;
    let mut same_blob = true;
    if let Some(t) = target.and_then(|t| u.ships.get(t)) {
        let dx = ld(t.posinfo.position.x) - ld(pos.x);
        let dy = ld(t.posinfo.position.y) - ld(pos.y);
        let dz = ld(t.posinfo.position.z) - ld(pos.z);
        length = st(fsqrt((dz * dz + dy * dy) + dx * dx));
        same_blob = t.coll_my_blob == me.coll_my_blob;
    }
    let blocks = |oid: ShipId| -> bool {
        if oid == id || Some(oid) == target {
            return false;
        }
        let Some(o) = u.ships.get(oid) else { return false };
        if !is_ally(o, me.player) {
            return false;
        }
        let Some(ostat) = u.statics.ship(o.ship_type) else { return false };
        rect_line(o, ostat, pos, dir, length).0 != -1.0
    };
    if same_blob {
        let Some(blob) = me.coll_my_blob.and_then(|b| u.blobs.get(b)) else { return true };
        return !blob.ships.iter().any(|&o| blocks(o));
    }
    for blob in &u.blobs.blobs {
        let dx = ld(blob.centre.x) - ld(pos.x);
        let dy = ld(blob.centre.y) - ld(pos.y);
        let dz = ld(blob.centre.z) - ld(pos.z);
        let proj = (dz * ld(dir.z) + dx * ld(dir.x)) + dy * ld(dir.y);
        if !(proj > 0.0) {
            continue;
        }
        let r = ld(blob.radius);
        let v = r * r - (((dz * dz + dy * dy) + dx * dx) - proj * proj);
        if !(v > 0.0) {
            continue;
        }
        if blob.ships.iter().any(|&o| blocks(o)) {
            return false;
        }
    }
    true
}

/// The sphere pre-test of the bullet walk: the bullet's path this frame passes within the
/// target's collision sphere.
fn bullet_sphere_test(t: &Ship, tstat: &ShipStaticInfo, b: &Bullet) -> bool {
    let tc = t.coll.coll_position;
    let bp = b.posinfo.position;
    let h = b.heading;
    let dx = ld(tc.x) - ld(bp.x);
    let dy = ld(tc.y) - ld(bp.y);
    let dz = ld(tc.z) - ld(bp.z);
    let proj = (dx * ld(h.x) + dy * ld(h.y)) + dz * ld(h.z);
    let proj2 = st(proj * proj);
    let v = ld(tstat.header.coll.coll_sphere_size_sqr) - (((dz * dz + dy * dy) + dx * dx) - ld(proj2));
    if !(v > 0.0) {
        return false;
    }
    let tr = ld(b.travel_dist);
    tr * tr > ld(proj2) - v
}

/// `collCheckAllBulletMissileCollisions` (bullets part): every blob's bullets against the
/// blob's targets.
pub fn check_all_bullet_collisions(u: &mut Universe) {
    for bi in 0..u.blobs.blobs.len() {
        check_bullets_in_blob(u, bi);
    }
}

/// `collCheckBulletTargetColl` (0x438880). A bullet that hits is deleted, which removes it from
/// the list being walked, so the index does not advance.
fn check_bullets_in_blob(u: &mut Universe, bi: usize) {
    let statics = u.statics.clone();
    let mut i = 0;
    while i < u.blobs.blobs[bi].bullets.len() {
        let bid = u.blobs.blobs[bi].bullets[i];
        let Some(b) = u.bullets.get(bid) else {
            i += 1;
            continue;
        };
        match b.weapon {
            crate::bullet::weapon::ION | crate::bullet::weapon::BEASTINFECT => {
                check_beam(u, bi, bid);
                i += 1;
                continue;
            }
            crate::bullet::weapon::LASER => {
                i += 1;
                continue;
            }
            _ => {}
        }
        let b = b.clone();
        let mut hit = None;
        'passes: for pass in 0..2 {
            let blob = &u.blobs.blobs[bi];
            let list = if pass == 0 { &blob.small_targets } else { &blob.big_targets };
            let n = list.len();
            if n == 0 {
                continue;
            }
            let cod = |sid: ShipId| u.ships.ship(sid).coll_optimize_dist;
            let (maxd, start) = if pass == 0 {
                let maxd = st(ld(blob.max_small_target_size) + ld(b.travel_dist));
                let (mut lo, mut hi, mut mid) = (0i32, n as i32 - 1, 0i32);
                while hi >= lo {
                    mid = (hi + lo) >> 1;
                    let d = ld(b.coll_optimize_dist) - ld(cod(list[mid as usize]));
                    if d <= ld(maxd) || d.is_nan() {
                        hi = mid - 1;
                    } else {
                        lo = mid + 1;
                    }
                }
                (maxd, mid as usize)
            } else {
                (st(ld(blob.max_big_target_size) + ld(b.travel_dist)), 0)
            };
            for &tid in list.iter().skip(start) {
                let t = u.ships.ship(tid);
                let diff = ld(t.coll_optimize_dist) - ld(b.coll_optimize_dist);
                if !(diff <= ld(maxd) || diff.is_nan()) {
                    break;
                }
                let Some(tstat) = statics.ship(t.ship_type) else { continue };
                let ad = ld(st(diff)).abs();
                let reach = ld(tstat.header.coll.coll_sphere_size) + ld(b.travel_dist);
                if !(ad <= reach || ad.is_nan() || reach.is_nan()) {
                    continue;
                }
                if b.owner == Some(tid) {
                    continue;
                }
                // (The DFG field effect check that follows is not modelled.)
                if !bullet_sphere_test(t, tstat, &b) {
                    continue;
                }
                let (tt, side) = rect_line(t, tstat, b.posinfo.position, b.heading, b.travel_dist);
                if lt_or_unordered(ld(tt), 0.0) {
                    continue;
                }
                // (The ray/triangle refinement of the hit distance is skipped.)
                hit = Some((tid, tt, side));
                break 'passes;
            }
        }
        match hit {
            Some((tid, tt, side)) => {
                crate::damage::bullet_collided_with_target(u, true, tid, bid, tt, side);
                crate::bullet::delete(u, bid);
            }
            None => i += 1,
        }
    }
}

/// `collCheckBeamCollWithTargetsInBlob` (0x438060): the nearest box a beam meets among one
/// blob's targets.
fn beam_against_blob(u: &Universe, beam: &Bullet, bi: usize, len: f32, best: &mut (f32, Option<ShipId>, i32)) {
    let blob = &u.blobs.blobs[bi];
    let h = beam.heading;
    let bp = beam.posinfo.position;
    for list in [&blob.small_targets, &blob.big_targets] {
        for &tid in list {
            if beam.owner == Some(tid) {
                continue;
            }
            let t = u.ships.ship(tid);
            let Some(tstat) = u.statics.ship(t.ship_type) else { continue };
            let tc = t.coll.coll_position;
            let dx = st(ld(tc.x) - ld(bp.x));
            let dy = st(ld(tc.y) - ld(bp.y));
            let dz = st(ld(tc.z) - ld(bp.z));
            let proj = st((ld(dz) * ld(h.z) + ld(dy) * ld(h.y)) + ld(dx) * ld(h.x));
            if !(ld(proj) > 0.0) {
                continue;
            }
            let lensq = (ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz);
            let v = st(ld(tstat.header.coll.coll_sphere_size_sqr) - (lensq - ld(proj) * ld(proj)));
            if !(ld(v) > 0.0 && ld(proj) * ld(proj) - ld(v) < ld(len) * ld(len)) {
                continue;
            }
            let (mut tt, side) = rect_line(t, tstat, bp, h, len);
            if !(ld(tt) >= 0.0) {
                continue;
            }
            let sq = fsqrt(ld(v));
            if tt == 0.0 {
                tt = st(ld(proj) - sq);
                if tt < 0.0 {
                    tt = 0.0;
                }
            }
            if ld(tt) < ld(best.0) {
                *best = (tt, Some(tid), side);
            }
        }
    }
}

/// `collCheckBeamColl` (0x4383f0): a beam is cut at the first box it meets (in its own blob or
/// any blob its line passes through) and damages that ship every frame.
fn check_beam(u: &mut Universe, own_blob: usize, bid: BulletId) {
    let beam = u.bullets.bullet(bid).clone();
    let len = beam.travel_dist;
    let mut best = (f32::from_bits(0x6258_d727), None, -1);
    beam_against_blob(u, &beam, own_blob, len, &mut best);
    let h = beam.heading;
    let bp = beam.posinfo.position;
    for bi in 0..u.blobs.blobs.len() {
        if bi == own_blob {
            continue;
        }
        let blob = &u.blobs.blobs[bi];
        let dx = st(ld(blob.centre.x) - ld(bp.x));
        let dy = st(ld(blob.centre.y) - ld(bp.y));
        let dz = st(ld(blob.centre.z) - ld(bp.z));
        let proj = st((ld(dx) * ld(h.x) + ld(dy) * ld(h.y)) + ld(dz) * ld(h.z));
        if !(ld(proj) > 0.0) {
            continue;
        }
        let r = ld(blob.radius);
        let lensq = (ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz);
        let v = st(r * r - (lensq - ld(proj) * ld(proj)));
        if ld(v) > 0.0 && ld(proj) * ld(proj) - ld(v) < ld(len) * ld(len) {
            beam_against_blob(u, &beam, bi, len, &mut best);
        }
    }
    match best.1 {
        None => {
            u.bullets.bullet_mut(bid).beam_travel_dist = beam.length_mag;
        }
        Some(tid) => {
            crate::damage::bullet_collided_with_target(u, true, tid, bid, best.0, best.2);
            if let Some(b) = u.bullets.get_mut(bid) {
                b.beam_travel_dist = best.0;
            }
        }
    }
}

/// `collCheckAllBumpCollisions` (0x4398b0): for every blob, big ships among themselves, small
/// ships among themselves, then small against big.
pub fn check_all_bump_collisions(u: &mut Universe) {
    for bi in 0..u.blobs.blobs.len() {
        ship_ship(u, bi, true);
        ship_ship(u, bi, false);
        big_small(u, bi);
    }
}

/// Whether a ship takes part in bump collisions at all.
fn bumpable(s: &Ship) -> bool {
    s.flags & sof::NO_BUMPS == 0 || s.special_flags2 & 8 != 0
}

/// `collCheckShipShipColl` (0x436950): every pair in one list, walking forwards while the
/// distance from the blob centre allows contact.
fn ship_ship(u: &mut Universe, bi: usize, big: bool) {
    let blob = &u.blobs.blobs[bi];
    let (list, max_size) = if big { (blob.big_ships.clone(), blob.max_big_ship_size) } else { (blob.small_ships.clone(), blob.max_small_ship_size) };
    let statics = u.statics.clone();
    for i in 0..list.len() {
        let a = list[i];
        let sa = u.ships.ship(a);
        if !bumpable(sa) {
            continue;
        }
        let Some(astat) = statics.ship(sa.ship_type) else { continue };
        let ra = astat.header.coll.coll_sphere_size;
        let reach = st(ld(ra) + ld(max_size));
        for &b in &list[i + 1..] {
            let sa = u.ships.ship(a);
            let sb = u.ships.ship(b);
            let dist = st(ld(sb.coll_optimize_dist) - ld(sa.coll_optimize_dist));
            if ld(reach) < ld(dist) {
                break;
            }
            let Some(bstat) = statics.ship(sb.ship_type) else { continue };
            if pair_touches(sa, astat, sb, bstat, dist) {
                bump(u, a, b, ra, bstat.header.coll.coll_sphere_size);
            }
        }
    }
}

/// `collCheckBigShipSmallShipColl` (0x436f70): each small ship against the big ships near it in
/// the blob (found by binary search).
fn big_small(u: &mut Universe, bi: usize) {
    let blob = &u.blobs.blobs[bi];
    let smalls = blob.small_ships.clone();
    let bigs = blob.big_ships.clone();
    let max_big = blob.max_big_ship_size;
    if bigs.is_empty() {
        return;
    }
    let statics = u.statics.clone();
    let mut mid = 0i32;
    for &a in &smalls {
        let sa = u.ships.ship(a);
        if !bumpable(sa) {
            continue;
        }
        let Some(astat) = statics.ship(sa.ship_type) else { continue };
        let ra = astat.header.coll.coll_sphere_size;
        let reach = st(ld(ra) + ld(max_big));
        let (mut lo, mut hi) = (0i32, bigs.len() as i32 - 1);
        while hi >= lo {
            mid = (hi + lo) >> 1;
            let d = ld(sa.coll_optimize_dist) - ld(u.ships.ship(bigs[mid as usize]).coll_optimize_dist);
            if d <= ld(reach) {
                hi = mid - 1;
            } else {
                lo = mid + 1;
            }
        }
        for &b in bigs.iter().skip(mid.max(0) as usize) {
            let sa = u.ships.ship(a);
            let sb = u.ships.ship(b);
            let dist = st(ld(sb.coll_optimize_dist) - ld(sa.coll_optimize_dist));
            if ld(reach) < ld(dist) {
                break;
            }
            let Some(bstat) = statics.ship(sb.ship_type) else { continue };
            if pair_touches(sa, astat, sb, bstat, dist) {
                bump(u, a, b, ra, bstat.header.coll.coll_sphere_size);
            }
        }
    }
}

/// The pair filters and contact tests shared by both bump walks (docking, salvage and
/// sentinel exceptions are not modelled: no such ships take part yet).
fn pair_touches(a: &Ship, astat: &ShipStaticInfo, b: &Ship, bstat: &ShipStaticInfo, dist: f32) -> bool {
    if !bumpable(b) {
        return false;
    }
    if a.posinfo.is_moving == 0 && b.posinfo.is_moving == 0 {
        return false;
    }
    let sum = st(ld(astat.header.coll.coll_sphere_size) + ld(bstat.header.coll.coll_sphere_size));
    if !(ld(dist) <= ld(sum)) {
        return false;
    }
    if a.docking_ship == Some(b.id) || b.docking_ship == Some(a.id) || (a.docking_ship.is_some() && b.docking_ship.is_some()) {
        return false;
    }
    // Salvagers latching onto their target do not bump it.
    let latching = |s: &Ship, t: &Ship| s.special_flags & crate::base::special::SALVAGER != 0 && s.base.salvage.target == Some(t.id) && s.base.salvage.latching;
    if latching(a, b) || latching(b, a) {
        return false;
    }
    // A ship and the host it is clamped to never bump.
    let host = |s: &Ship| s.base.clamp.as_ref().and_then(|c| c.host);
    if (a.flags & sof::CLAMPED != 0 && host(a) == Some(b.id)) || (b.flags & sof::CLAMPED != 0 && host(b) == Some(a.id)) {
        return false;
    }
    if (a.flags | b.flags) & sof::HIDE != 0 {
        return false;
    }
    let d = Vector::new(
        st(ld(b.coll.coll_position.x) - ld(a.coll.coll_position.x)),
        st(ld(b.coll.coll_position.y) - ld(a.coll.coll_position.y)),
        st(ld(b.coll.coll_position.z) - ld(a.coll.coll_position.z)),
    );
    let neg = -sum;
    if !(neg <= d.x && d.x <= sum && neg <= d.y && d.y <= sum && neg <= d.z && d.z <= sum) {
        return false;
    }
    let dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
    if !(ld(dsq) < ld(sum) * ld(sum)) {
        return false;
    }
    rect_bump(a, astat, b, bstat)
}

/// The response to a bump: repulsion, or a velocity exchange when the ships have not collided
/// for a while (`FUN_00540770`).
fn bump(u: &mut Universe, a: ShipId, b: ShipId, ra: f32, rb: f32) {
    let (sa, sb) = (u.ships.ship(a), u.ships.ship(b));
    if (sa.flags | sb.flags) & sof::NOT_BUMPABLE != 0 {
        return;
    }
    let mut d = Vector::new(
        st(ld(sb.coll.coll_position.x) - ld(sa.coll.coll_position.x)),
        st(ld(sb.coll.coll_position.y) - ld(sa.coll.coll_position.y)),
        st(ld(sb.coll.coll_position.z) - ld(sa.coll.coll_position.z)),
    );
    let mut dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
    if dsq < 1.0 {
        d.x = 10.0;
        dsq = 100.0;
    }
    let dist = st(fsqrt(ld(dsq)));
    // (Kamikaze, ramming, salvage, leech and the killer-collision damage are not modelled;
    // `FUN_005405e0` does nothing for ordinary ships.)
    let sum = st(ld(ra) + ld(rb));
    response(u, a, b, sum, d, dist);
}

fn response(u: &mut Universe, a: ShipId, b: ShipId, sum: f32, d: Vector, dist: f32) {
    let tw = u.tweaks.combat.clone();
    let now = u.total_time_elapsed;
    let statics = u.statics.clone();
    let k = st(((ld(sum) - ld(dist)) * ld(tw.collision_repulse_factor)) / ld(dist));
    let (sa, sb) = (u.ships.ship(a).clone(), u.ships.ship(b).clone());
    let (Some(astat), Some(bstat)) = (statics.ship(sa.ship_type), statics.ship(sb.ship_type)) else { return };
    let can_push = |s: &Ship, stat: &ShipStaticInfo| !stat.header.immobile && s.attributes & 0x10 == 0;
    let clear = ld(tw.collision_clear_time);
    if clear <= ld(now) - ld(sa.last_time_collided) || clear <= ld(now) - ld(sb.last_time_collided) {
        u.ships.ship_mut(a).last_time_collided = now;
        u.ships.ship_mut(b).last_time_collided = now;
        let vsq = |v: Vector| st((ld(v.z) * ld(v.z) + ld(v.y) * ld(v.y)) + ld(v.x) * ld(v.x));
        let va2 = vsq(sa.posinfo.velocity);
        let vb2 = vsq(sb.posinfo.velocity);
        // `fast` is the faster ship.
        let (fast, slow, fast2, slow2) = if vb2 < va2 { (&sa, &sb, va2, vb2) } else { (&sb, &sa, vb2, va2) };
        let (ma, mb) = (astat.header.mass, bstat.header.mass);
        let inv_dt = st(1.0 / ld(u.phys_time_elapsed));
        let (pa, pb) = (sa.posinfo.velocity, sb.posinfo.velocity);
        let mut f = Vector::new(
            st(ld(inv_dt) * (ld(mb) * ld(pb.x) - ld(ma) * ld(pa.x))),
            st(ld(inv_dt) * (ld(st(ld(mb) * ld(pb.y))) - ld(st(ld(ma) * ld(pa.y))))),
            st(ld(inv_dt) * (ld(st(ld(mb) * ld(pb.z))) - ld(st(ld(ma) * ld(pa.z))))),
        );
        let glancing = ld(tw.glance_collision_vel_ratio) <= ld(fast2) / ld(slow2)
            || (ld(fast.posinfo.velocity.x) * ld(slow.posinfo.velocity.x) + ld(fast.posinfo.velocity.y) * ld(slow.posinfo.velocity.y))
                + ld(fast.posinfo.velocity.z) * ld(slow.posinfo.velocity.z)
                < 0.0;
        if glancing {
            if f.x == 0.0 && f.y == 0.0 {
                f.x = 0.1;
            }
            let mag = st(fsqrt((ld(f.x) * ld(f.x) + ld(f.y) * ld(f.y)) + ld(f.z) * ld(f.z)));
            let mut side = Vector::new(st(ld(f.y) - 0.0), st(0.0 - ld(f.x)), 0.0);
            side.normalize_to_length(mag);
            let rel = Vector::new(
                st(ld(slow.coll.coll_position.x) - ld(fast.coll.coll_position.x)),
                st(ld(slow.coll.coll_position.y) - ld(fast.coll.coll_position.y)),
                0.0,
            );
            let cross = ld(rel.x) * ld(fast.posinfo.velocity.y) - ld(rel.y) * ld(fast.posinfo.velocity.x);
            f = if cross <= 0.0 { side } else { Vector::new(-side.x, -side.y, -side.z) };
        }
        let (fast_id, slow_id) = (fast.id, slow.id);
        let fstat = if fast_id == a { astat } else { bstat };
        let sstat = if slow_id == a { astat } else { bstat };
        let fs = u.ships.ship(fast_id);
        if can_push(fs, fstat) {
            let fs = u.ships.ship_mut(fast_id);
            fs.posinfo.force = Vector::new(st(ld(f.x) + ld(fs.posinfo.force.x)), st(ld(f.y) + ld(fs.posinfo.force.y)), st(ld(f.z) + ld(fs.posinfo.force.z)));
        }
        let ss = u.ships.ship(slow_id);
        if can_push(ss, sstat) {
            let ss = u.ships.ship_mut(slow_id);
            ss.posinfo.force = Vector::new(st(ld(ss.posinfo.force.x) - ld(f.x)), st(ld(ss.posinfo.force.y) - ld(f.y)), st(ld(ss.posinfo.force.z) - ld(f.z)));
        }
    } else {
        if can_push(&sb, bstat) {
            let s = u.ships.ship_mut(b);
            let p = &mut s.posinfo.force;
            *p = Vector::new(st(ld(k) * ld(d.x) + ld(p.x)), st(ld(k) * ld(d.y) + ld(p.y)), st(ld(k) * ld(d.z) + ld(p.z)));
        }
        if can_push(&sa, astat) {
            let s = u.ships.ship_mut(a);
            let p = &mut s.posinfo.force;
            *p = Vector::new(st(ld(p.x) - ld(k) * ld(d.x)), st(ld(p.y) - ld(k) * ld(d.y)), st(ld(p.z) - ld(k) * ld(d.z)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::{ShipId, ShipTable};
    use crate::statics::{ShipRace, ShipType};
    use hwc_math::Matrix;

    fn boxed_ship() -> (Ship, ShipStaticInfo) {
        let mut stat = ShipStaticInfo::new(ShipType::sAcolyte, String::new());
        let c = &mut stat.header.coll;
        c.coll_rect_offset = Vector::new(-5.0, -10.0, -20.0);
        c.up_length = 10.0;
        c.right_length = 20.0;
        c.forward_length = 40.0;
        c.diagonal_length = 45.825_756;
        let mut s = Ship::new(ShipId(0), ShipType::sAcolyte, ShipRace::Somtaaw, 0);
        s.rotinfo.coordsys = Matrix { m: [0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0] };
        s.posinfo.position = Vector::new(100.0, 50.0, -30.0);
        let _ = ShipTable::default();
        (s, stat)
    }

    #[test]
    fn rect_line_hits_front_face() {
        let (mut s, stat) = boxed_ship();
        update_coll_rect(&mut s, &stat);
        // Heading is world +X; the front face (local z = +20) is at world x = 120.
        let (t, side) = rect_line(&s, &stat, Vector::new(200.0, 50.0, -30.0), Vector::new(-1.0, 0.0, 0.0), 1000.0);
        assert_eq!(t, 80.0);
        assert_eq!(side, 4);
        // Inside the box.
        assert_eq!(rect_line(&s, &stat, s.posinfo.position, Vector::new(1.0, 0.0, 0.0), 10.0).0, 0.0);
        // Too short.
        assert_eq!(rect_line(&s, &stat, Vector::new(200.0, 50.0, -30.0), Vector::new(-1.0, 0.0, 0.0), 50.0).0, -1.0);
        // Pointing away.
        assert_eq!(rect_line(&s, &stat, Vector::new(200.0, 50.0, -30.0), Vector::new(1.0, 0.0, 0.0), 1000.0).0, -1.0);
    }

    #[test]
    fn coll_rect_corners() {
        let (mut s, stat) = boxed_ship();
        update_coll_rect(&mut s, &stat);
        // Corner 0 = M * offset; corner 6 = the opposite corner.
        assert_eq!(s.coll.rect[0], Vector::new(-20.0, 10.0, -5.0));
        assert_eq!(s.coll.rect[6], Vector::new(20.0, -10.0, 5.0));
    }
    /// Ray start, direction, length; expected `t` and side.
    type Ray = ([u32; 3], [u32; 3], u32, u32, i32);

    struct CollCase {
        pos: [u32; 3],
        m: [u32; 9],
        off: [u32; 3],
        lens: [u32; 3],
        rect: [u32; 24],
        rays: &'static [Ray],
    }

    /// Inputs and results of emulating `collUpdateCollRectangle` (0x4357a0) and
    /// `collCheckRectLine` (0x4359e0) in cataclysm.exe (Ghidra p-code emulation, random
    /// orientations and rays; see the report for the script).
    const COLL_CASES: &[CollCase] = &[
    CollCase { pos: [0xc38edc81, 0x43b35131, 0x451f1437], m: [0x3f7228d5, 0x3ea5dd99, 0x3c83a60e, 0xbea5cd89, 0x3f723135, 0xbc20691c, 0xbc9687ee, 0x3b84f272, 0x3f7ff465], off: [0xc196cbd4, 0xc22d9295, 0xc250f6fe], lens: [0x4320aa65, 0x41df1152, 0x42874ae0], rect: [0xc0345f5d, 0xc23d7bf2, 0xc25070aa, 0xc13d9119, 0xc1a7eea4, 0xc2518837, 0xc1517487, 0xc1a5bc8e, 0x41740514, 0xc081f689, 0xc23c62e7, 0x41786348, 0x43152936, 0x4095aed0, 0xc2461cbd, 0x430c21a2, 0x41f874f4, 0xc247344a, 0x430ae36b, 0x41faa70a, 0x418eaa64, 0x4313eaff, 0x409e7728, 0x4190d97e], rays: &[([0xc4194bec, 0x44179665, 0x4528c03a], [0x3f42e2d5, 0xbf0f15cf, 0xbea84f52], 0x438c05aa, 0xbf800000, -1), ([0xc33b384d, 0x441bffb8, 0x45093cba], [0xbe8af3f0, 0xbf0ca1f2, 0x3f4a50ba], 0x44fa0000, 0xbf800000, -1), ([0xc3d45811, 0x43d7b370, 0x450fd854], [0x3ef20dfb, 0xbea002ea, 0x3f52ebaa], 0x43c7546a, 0x439030ac, 2), ([0xc39feded, 0x43352a25, 0x4537f637], [0x3e09d3a3, 0x3ee376a1, 0xbf62bf07], 0x44fa0000, 0xbf800000, -1)] },
    CollCase { pos: [0x44c17e17, 0x429a9d9e, 0xc53057d1], m: [0xbe3b1bea, 0xbf413f75, 0x3f214142, 0x3f23f65b, 0xbf13d5e1, 0xbf019a76, 0x3f3ef4c5, 0x3e9f327a, 0x3f16c8b5], off: [0xc1d68aaf, 0xc120311a, 0xc0c88982], lens: [0x430c216a, 0x424a42e3, 0x433a2355], rect: [0xc0c5f9f1, 0x41c09e22, 0xc1783d51, 0x41d197f9, 0xc0a3f298, 0xc2247512, 0x43250b31, 0x425302f8, 0x42890abd, 0x4304a862, 0x42a3e82e, 0x42bc3d9c, 0xc1fe55dc, 0xc2a36852, 0x429181cf, 0x3f181323, 0xc2ddcf04, 0x423c9de0, 0x430b7045, 0xc2541cbc, 0x431cca1b, 0x42d61aec, 0xc1be9eb0, 0x4336638a], rays: &[([0x4494b98c, 0xc1b2929c, 0xc525e5e8], [0x3f5c6549, 0x3e8021e5, 0xbee2c7a8], 0x44fa0000, 0x43d2a6ed, 5), ([0x44b12e8c, 0xc29372eb, 0xc54895e7], [0x3e9726a0, 0x3ece8fce, 0x3f5db6f8], 0x4420097f, 0x43d620bd, 5), ([0x4490968a, 0x424509f5, 0xc531007a], [0x3f7fc7a0, 0x3cfee565, 0x3ce09432], 0x438c3d55, 0xbf800000, -1), ([0x44b96a64, 0xc17b0d34, 0xc53599b1], [0x3f164c46, 0x3ee57050, 0x3f2c9611], 0x44fa0000, 0x42f71fba, 5)] },
    CollCase { pos: [0x453ab6c2, 0xc5342374, 0xc4ea84fd], m: [0xbf5f7c50, 0xbee031ef, 0x3e5bf9c8, 0x3ed8b9f5, 0xbf66252e, 0xbde600df, 0x3e781dc4, 0xbbe8fe8b, 0x3f785d65], off: [0xc1028786, 0xc17fa15d, 0xc2187667], lens: [0x413d1bd5, 0x42fbed36, 0x4327c148], rect: [0xc10e05e9, 0x4191a71b, 0xc213bf89, 0x4231c5ab, 0xc2be11ee, 0xc24c5588, 0x42aa2e35, 0xc2c074a6, 0x42df561e, 0x41fe2a8a, 0x41881c3c, 0x42fba11e, 0xc1998e7e, 0x41507f98, 0xc2099794, 0x42087fe6, 0xc2c86bc2, 0xc2422d93, 0x42958b53, 0xc2cace7a, 0x42e46a19, 0x41ab9f00, 0x413d69da, 0x43005a8c], rays: &[([0x453505bc, 0xc5496ca8, 0xc503d2b5], [0x3e79dfec, 0x3f49df73, 0x3f1080af], 0x44fa0000, 0x43c81932, 0), ([0x45281330, 0xc52fc662, 0xc4c94898], [0x3f383055, 0xbe46af72, 0xbf2ab6c4], 0x44fa0000, 0x43ce5136, 0), ([0x454f2295, 0xc5243f97, 0xc501c8ea], [0xbf389643, 0xbf02aa36, 0x3eefeb66], 0x43a62163, 0xbf800000, -1), ([0x45441006, 0xc539ba5e, 0xc4ec4493], [0xbf6e46e2, 0x3eba8bc1, 0xbcf87292], 0x44fa0000, 0x43057017, 1)] },
    CollCase { pos: [0xc4c431d2, 0x44996f42, 0xc4b64392], m: [0x3f3f257f, 0x3f298eed, 0x3d7c7ab4, 0xbf284721, 0x3f386fee, 0x3e625392, 0x3dd0dbc4, 0xbe527b2c, 0x3f792b6e], off: [0xc1914292, 0xc26c1b23, 0xc1228806], lens: [0x425594cb, 0x42e88f28, 0x432bf4bb], rect: [0x41c1a740, 0xc251dba6, 0xc1c06ba7, 0xc250e980, 0x41fa7b00, 0x3fd2ea50, 0xc20ac445, 0xc0811ff0, 0x4329040b, 0x4226f8db, 0xc2af9e92, 0x430f50c1, 0x42802676, 0xc188ca5c, 0xc1a6170c, 0xc145c0d0, 0x428559fc, 0x409e0d00, 0x40a5a837, 0x41faa4f4, 0x432c4e9e, 0x42a33913, 0xc251c6ac, 0x43129b54], rays: &[([0xc4b8c3aa, 0x448374e0, 0xc48c8726], [0xbe8acae7, 0x3ec61d41, 0xbf61a0ae], 0x44fa0000, 0xbf800000, -1), ([0xc4dd49c8, 0x44583057, 0xc4cc1159], [0x3edfeef6, 0x3f55d8f1, 0x3eaa8381], 0x43a1e324, 0xbf800000, -1), ([0xc49d19c0, 0x44c97be4, 0xc4a6921c], [0xbf19169d, 0xbf3ff54b, 0xbe90f10f], 0x443e20b7, 0xbf800000, -1), ([0xc499d5fc, 0x4496e9f5, 0xc4c48449], [0xbf7240d2, 0xbc0c8cda, 0x3ea576ba], 0x440a0420, 0x43940365, 3)] },
    CollCase { pos: [0xc2d52bdc, 0x44acdf9d, 0xc487d256], m: [0xbf011f04, 0xbe8f8271, 0xbf5114ce, 0x3e113a3d, 0xbf75bbaf, 0x3e77a565, 0xbf5a0ca5, 0x3bc97fe9, 0x3f061e8a], off: [0xc2711a06, 0xc2910786, 0xc271205e], lens: [0x430fb383, 0x4320b87e, 0x4337d9ba], rect: [0x428eecd6, 0x42ac42d5, 0x3de04e93, 0x42bc835e, 0xc28849f5, 0x421be9ff, 0xc2795b39, 0xc286071e, 0x43074c66, 0xc2aa4424, 0x42ae85ac, 0x42c0dbe0, 0xbf82397f, 0x423768ac, 0xc2ea8263, 0x41ae3687, 0xc2d8d874, 0xc29cc578, 0xc306d1ac, 0xc2d6959d, 0x418f7954, 0xc31d9cf0, 0x423bee5a, 0xc1a77a5c], rays: &[([0xc3611e4f, 0x44bf635a, 0xc43f79df], [0x3ebe899b, 0xbed39fc7, 0xbf54c0ab], 0x44fa0000, 0x43970e78, 1), ([0xc2415bd1, 0x44b95e9a, 0xc4939699], [0xbed32be5, 0xbf3870f6, 0x3f0eb863], 0x4344cc36, 0x426e16ed, 5), ([0x4390093a, 0x44d2da49, 0xc4620078], [0xbf428c50, 0xbf0df9de, 0xbead8ca2], 0x44fa0000, 0x43e4b2d0, 1), ([0xc3087ce6, 0x44b0ffa9, 0xc485f3b0], [0x3ef7ae46, 0xbf2bcc1b, 0xbf0fd2dd], 0x42bdf204, 0x0, -1)] },
    CollCase { pos: [0xc4ca5572, 0x4494c042, 0xc183f8e7], m: [0x3f2be3ac, 0xbf3d8d3e, 0x3cf84879, 0x3ec1477e, 0x3ec0feeb, 0x3f588780, 0xbf234037, 0xbf0e7544, 0x3f08592b], off: [0xc0bb2326, 0xc1dc9e9d, 0xc25f70ec], lens: [0x4241f4e1, 0x4228e6c5, 0x4336164c], rect: [0x41aa4792, 0x41c828b4, 0xc255049d, 0x4214e65c, 0x4223befc, 0xc18c5102, 0xc29dc890, 0xc271901e, 0x429ee248, 0xc2bda9da, 0xc2989d60, 0x422ee875, 0x42575eda, 0xc12e21cc, 0xc24f23bc, 0x428b90b6, 0x40a11178, 0xc1808f40, 0xc239560f, 0xc2c09676, 0x42a1d2b9, 0xc27918a3, 0xc2e06bc6, 0x4234c956], rays: &[([0xc4ba55b4, 0x448ef1bb, 0x43947148], [0xbec25fa6, 0x3e1bc243, 0xbf699c98], 0x44fa0000, 0x43a3ea83, 2), ([0xc4abf660, 0x44ae36c7, 0xc38021ab], [0xbf1a8ada, 0xbf02caba, 0x3f1cac0b], 0x43952773, 0xbf800000, -1), ([0xc4eeb755, 0x449465d0, 0x437d2e12], [0x3f47aaee, 0x3d0aa28e, 0xbf1ffafd], 0x44fa0000, 0xbf800000, -1), ([0xc4aa25e4, 0x445c1161, 0xc21da927], [0xbf15fb54, 0x3f4f2e8d, 0x3d2d2eaa], 0x44fa0000, 0x43b503c9, 0)] },
    CollCase { pos: [0x44811c48, 0x44a2351b, 0x45003699], m: [0xbf6cf4b3, 0x3ea553e5, 0x3e4a2c33, 0xbe70d97e, 0xbf6641a6, 0x3ebc9977, 0x3e97d223, 0x3e96cb19, 0x3f689200], off: [0xc1cedfcd, 0xc24a4430, 0xc25f79f6], lens: [0x427d9d0c, 0x429a9a02, 0x43055d4a], rect: [0x419a1990, 0x41a56a7c, 0xc294f9d7, 0x3f8a5d58, 0xc24366c8, 0xc2380de6, 0x422281ea, 0xc1192764, 0x42964a42, 0x426b3bc7, 0x426fd22d, 0x423aaebc, 0xc21db26a, 0x422499e9, 0xc277e141, 0xc2666c47, 0xc1e3043a, 0xc205fb79, 0xc1907a90, 0x412e6b48, 0x42af5379, 0x3df929d8, 0x42a0db6c, 0x426cc129], rays: &[([0x448783ae, 0x44b04e56, 0x44f6f209], [0xbefba361, 0xbf3b446c, 0x3ef1f686], 0x42be1904, 0xbf800000, -1), ([0x449ecb93, 0x44d29bd4, 0x44d9f500], [0xbeeaf5c7, 0xbf302ca7, 0x3f0fdd55], 0x44fa0000, 0x43f9fb3f, 3), ([0x44aef4e0, 0x44b4a81f, 0x44e25c56], [0xbf4e2006, 0xbeb75562, 0x3ef209d7], 0x44fa0000, 0x43d8c6e8, 1), ([0x44849867, 0x4467a65f, 0x44e5127c], [0xbd008fea, 0x3f56e0d0, 0x3f0aeb8e], 0x439b8d60, 0xbf800000, -1)] },
    CollCase { pos: [0xc41fb9c6, 0x44daf2fd, 0x451856c0], m: [0x3edfa90c, 0xbf6306d7, 0x3e1a5824, 0x3f0493e4, 0x3de33156, 0xbf592588, 0x3f3c4a45, 0x3ee5ae64, 0x3f01fa8e], off: [0xc16be454, 0xc21c0f94, 0xc24faa1a], lens: [0x4336e4b7, 0x42a32f60, 0x42ec263b], rect: [0xc281a928, 0xc168af6e, 0x409064b1, 0xc1b499d4, 0xc0afb9f2, 0xc28164ca, 0x42808a21, 0x423de7d3, 0xc097e660, 0x41b01db8, 0x4219b336, 0x4280ecaf, 0x417106c8, 0xc330bca8, 0x42005125, 0x42654718, 0xc327af81, 0xc2148506, 0x43102a11, 0xc2e56f78, 0x41b68f85, 0x42cbd16f, 0xc2f789c7, 0x42b80ef6], rays: &[([0xc3a7b918, 0x44f8a03b, 0x452e8d41], [0xbf12e4ab, 0xbedd3351, 0xbf321e2b], 0x44478177, 0x43e5123b, 4), ([0xc45910fd, 0x45017949, 0x45305d10], [0x3eebe0eb, 0xbf142b6f, 0xbf2c417e], 0x44fa0000, 0x43fddafb, 1), ([0xc44f595b, 0x44f0c6a8, 0x45001f0c], [0x3ed066d3, 0xbed814d2, 0x3f4f60a0], 0x44fa0000, 0x43d0f12b, 1), ([0xc3d42870, 0x44d9b1a5, 0x4500c267], [0xbee7ad63, 0xbcdad312, 0x3f6430a0], 0x439a065c, 0xbf800000, -1)] },
    CollCase { pos: [0x4346bb58, 0x448bf006, 0x451c00b1], m: [0xbe37fb16, 0x3f21a86f, 0xbf41194c, 0xbeb8007a, 0xbf4182ff, 0xbf0c16e5, 0xbf6a6d73, 0x3e30e751, 0x3eb9ba95], off: [0xc22f76b7, 0xc25f5dab, 0xc2180c89], lens: [0x43313038, 0x42f0005e, 0x430cec08], rect: [0x427b08d3, 0x40fe3274, 0x42476d1a, 0x419d103a, 0xc2a587ef, 0xc17cf8e8, 0xc2dad3ef, 0xc269ae43, 0x420d3c55, 0xc2849394, 0x420127ea, 0x42c9f3d4, 0x41f76323, 0x42efaaf6, 0xc2a7972d, 0xc1433c92, 0x41e8ff80, 0xc315766c, 0xc30d3fc8, 0x4255e15b, 0xc2c4af90, 0xc2c43f35, 0x43102de2, 0xc202b3cc], rays: &[([0x43ca8067, 0x4485e13b, 0x451ef103], [0xbf6afaf9, 0x3e7f5876, 0xbe9e0d71], 0x44fa0000, 0x433845d9, 5), ([0x42405c05, 0x44800f2e, 0x452d4407], [0x3f08766a, 0x3e80a22b, 0xbf4ed3aa], 0x44fa0000, 0x4392d37d, 1), ([0x43808fc1, 0x445c1192, 0x451dcc42], [0xbe684759, 0x3f73cdc0, 0xbe50c144], 0x43c089df, 0x432f16b1, 1), ([0x43538bc9, 0x4481fef9, 0x452b0e5e], [0xbd21eae3, 0x3ea58332, 0xbf720b1a], 0x43b77b6e, 0x434de618, 1)] },
    CollCase { pos: [0x45218f48, 0xc3766410, 0x451dcb2b], m: [0x3f75443a, 0x3e92acac, 0x3bb49563, 0xbe8e3319, 0x3f6ee7f9, 0xbe6965f1, 0xbd9041f9, 0x3e5e0b88, 0x3f7941f0], off: [0xc227e935, 0xc1f3cef9, 0xc291225c], lens: [0x433bc7f0, 0x4338cdaf, 0x42db7e3a], rect: [0xc1d522c6, 0xc260cef8, 0xc27fc370, 0xc29befac, 0x42e885e0, 0xc2d4201e, 0xc2ab659e, 0x430c0f1b, 0x3f4b1880, 0xc2097d47, 0xc2019e4c, 0x422ba92e, 0x4319440c, 0xc01a1990, 0xc27b9fbf, 0x42cbe11d, 0x432a0e48, 0xc2d20e45, 0x42bc6b2b, 0x4341da73, 0x3fea0269, 0x43118913, 0x41ab1e26, 0x422fccdf], rays: &[([0x450dfd89, 0xc39b6136, 0x450643ac], [0x3f183636, 0x3e1694f0, 0x3f4a5cb0], 0x443deaa9, 0x43e20596, 1), ([0x451243d9, 0xc3e84aad, 0x45272534], [0x3f2dbb7a, 0x3f1b6b19, 0xbed3a4ab], 0x44024ead, 0x439aa135, 1), ([0x45218b24, 0xc33499fd, 0x452f63ba], [0x3cbe4906, 0xbe4af0ce, 0xbf7ada0a], 0x44fa0000, 0x437b8db6, 4), ([0x4509b8e2, 0xc30bfc4f, 0x452e0d78], [0x3f53a047, 0xbe647524, 0xbf043e58], 0x43a3ee7a, 0xbf800000, -1)] },
    CollCase { pos: [0x4331b6d0, 0x4503b8c9, 0x44279114], m: [0xbf5041fe, 0xbf149753, 0xbd13d692, 0x3ee5660c, 0xbf2a444c, 0x3f18ef82, 0xbebdd43a, 0x3ef08c6f, 0x3f4d16ae], off: [0xc2839384, 0xc29166d1, 0xc1e58637], lens: [0x428bb213, 0x43395363, 0x424dca28], rect: [0x41fcaa6a, 0x42921ea9, 0xc2801589, 0x42e53c12, 0xc248cdf2, 0x423ab084, 0x42bf15db, 0xc1d03d7b, 0x42afc6d0, 0x41482320, 0x42c27643, 0xc1b69bea, 0xc1c9e85f, 0x42021245, 0xc285204e, 0x42672ebf, 0xc2b57c80, 0x42309af9, 0x421ae251, 0xc28524e5, 0x42aabc0b, 0xc231409d, 0x4262c179, 0xc1cac700], rays: &[([0x4410196b, 0x45171952, 0x43bcb4d1], [0xbf356655, 0xbf039b90, 0x3ef776af], 0x4459e328, 0x43fec0ac, 5), ([0x42fb1a92, 0x4519c805, 0x443038d9], [0x3e2ac03c, 0xbf7bad30, 0xbd9a70fd], 0x440c5711, 0x43a2c266, 4), ([0xc31170bf, 0x45073781, 0x440b1590], [0x3f6cd762, 0xbe2f8b8c, 0x3ead644c], 0x44fa0000, 0x43ad34b5, 0), ([0x4333f145, 0x450d0186, 0x43d2bc81], [0xbdabd62e, 0xbf187884, 0x3f4c8468], 0x44fa0000, 0xbf800000, -1)] },
    CollCase { pos: [0xc5048abd, 0xc4ec9908, 0xc4de1082], m: [0xbe45c7f8, 0xbf0764a0, 0xbf5390b8, 0x3f591003, 0x3eaaf053, 0xbed2da96, 0x3efcc8dd, 0xbf47bfc5, 0x3ec49569], off: [0xc1424bb4, 0xc11a5587, 0xc298b968], lens: [0x42c3963e, 0x431b4c01, 0x429031c2], rect: [0xc22e23a0, 0x427b23ec, 0xc174fb25, 0x42b04877, 0x42e54438, 0xc29e8857, 0x42f77986, 0x4269830c, 0xc24e566e, 0xc0fe0c10, 0x40d0f440, 0x4145eddd, 0xc279b152, 0x413106d4, 0xc2c042cb, 0x428a819e, 0x427ba639, 0xc32015de, 0x42d1b2ad, 0x40d506a8, 0xc304674e, 0xc1d69e69, 0xc234c3af, 0xc288e5aa], rays: &[([0xc50631a7, 0xc4eb11ae, 0xc4e50e6e], [0x3ec52961, 0xbee508e1, 0x3f4ea604], 0x4264e317, 0x0, -1), ([0xc5147a7e, 0xc4f133e3, 0xc4c6218d], [0x3f445c1d, 0x3d878f76, 0xbf23603c], 0x43f4b182, 0xbf800000, -1), ([0xc50d5b6c, 0xc50b0ec1, 0xc4bbdfb5], [0x3eb14070, 0x3f4382ec, 0xbf0b7cc9], 0x43a194b0, 0xbf800000, -1), ([0xc50453d8, 0xc4bc546b, 0xc4bf9514], [0xbd12c4fb, 0xbf581b89, 0xbf08edaa], 0x4397927c, 0xbf800000, -1)] },
    ];

    #[test]
    fn rect_and_rect_line_match_emulated_binary() {
        let f = f32::from_bits;
        let v = |a: [u32; 3]| Vector::new(f(a[0]), f(a[1]), f(a[2]));
        for (i, c) in COLL_CASES.iter().enumerate() {
            let (mut s, mut stat) = boxed_ship();
            s.posinfo.position = v(c.pos);
            s.rotinfo.coordsys = Matrix { m: c.m.map(f) };
            let cc = &mut stat.header.coll;
            cc.coll_rect_offset = v(c.off);
            cc.up_length = f(c.lens[0]);
            cc.right_length = f(c.lens[1]);
            cc.forward_length = f(c.lens[2]);
            update_coll_rect(&mut s, &stat);
            let got: Vec<u32> = s.coll.rect.iter().flat_map(|r| [r.x.to_bits(), r.y.to_bits(), r.z.to_bits()]).collect();
            assert_eq!(got, c.rect.to_vec(), "case {i} rectangle");
            for (k, (start, dir, len, t, side)) in c.rays.iter().enumerate() {
                let (gt, gs) = rect_line(&s, &stat, v(*start), v(*dir), f(*len));
                assert_eq!(gt.to_bits(), *t, "case {i} ray {k} t");
                if *t != (-1.0f32).to_bits() && *t != 0 {
                    assert_eq!(gs, *side, "case {i} ray {k} side");
                }
            }
        }
    }
}
