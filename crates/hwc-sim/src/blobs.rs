//! Collision blobs (`Blobs.c`, `Collision.c` broad phase): spheres that group nearby objects so
//! that avoidance and collision checks only look at neighbours.
//!
//! `collUpdateCollBlobs` rebuilds the list every `REFRESH_COLLBLOB_RATE + 1` frames (keeping
//! blobs whose objects have not drifted), and `collUpdateObjsInCollBlobs` refreshes each
//! object's distance from its blob centre on the other frames. Objects in a blob are kept
//! sorted by that distance (`collOptimizeDist`) with the CRT `qsort`, and split into the
//! small/big target lists the avoidance code walks.
//!
//! Blobs hold ships and bullets; bullets lose their blob at each rebuild and are put back into
//! their target's blob (or the nearest one) afterwards, as in the original.

use hwc_math::crt::qsort;
use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st, Ext};
use hwc_math::Vector;

use crate::missile::MissileTable;
use crate::object::{sof, BlobId, BulletId, BulletTable, MissileId, ObjRef, Ship, ShipId, ShipTable};
use crate::statics::StaticWorld;
use crate::tunables::BlobProperties;

/// `BIG_TARGET_SIZE` / `BIG_SHIP_SIZE`.
pub const BIG_TARGET_SIZE: f32 = 500.0;
/// `4/3 * PI` as the compiler folded it (0x720570).
const SPHERE_VOLUME_FACTOR: f32 = f32::from_bits(0x4086_0a92);
/// `REALlyBig` (1e21).
const REALLY_BIG: f32 = f32::from_bits(0x6258_d727);
/// `bobUpdateRadiusThreshold` and `bobUpdateCentreThreshold` (0x747638, 0x74763c).
const UPDATE_RADIUS_THRESHOLD: f32 = 1.0;
const UPDATE_CENTRE_THRESHOLD: f32 = f32::from_bits(0x3dcc_cccd);

/// Blob flags (`BTF_*`) as Cataclysm numbers them.
pub mod btf {
    pub const DONT_UPDATE: u32 = 0x0100;
    pub const CLEAR_OBJECT_FLAGS: u32 = 0x0200;
    pub const RECENT_DEATH: u32 = 0x1000;
}

/// One blob.
#[derive(Debug, Clone, PartialEq)]
pub struct Blob {
    pub centre: Vector,
    pub radius: f32,
    pub volume: f32,
    pub one_over_volume: f32,
    pub total_mass: f32,
    /// Squared distance of the centre from the origin (the list is sorted by it).
    pub sort_distance: f32,
    /// Its square root, computed lazily (0 = not yet).
    pub sqrt_sort_distance: f32,
    pub flags: u32,
    /// All objects, sorted by `collOptimizeDist` (`blobObjects`).
    pub objects: Vec<ObjRef>,
    /// Targets smaller and not smaller than [`BIG_TARGET_SIZE`] (`blobSmallTargets`,
    /// `blobBigTargets`), in `objects` order.
    pub small_targets: Vec<ShipId>,
    pub big_targets: Vec<ShipId>,
    pub max_small_target_size: f32,
    pub max_big_target_size: f32,
    /// Ships (`blobShips`, `blobSmallShips`, `blobBigShips`).
    pub ships: Vec<ShipId>,
    pub small_ships: Vec<ShipId>,
    pub big_ships: Vec<ShipId>,
    pub max_ship_size: f32,
    pub max_small_ship_size: f32,
    pub max_big_ship_size: f32,
    /// Bullets and beams (`blobBullets`).
    pub bullets: Vec<BulletId>,
    /// Missiles (`blobMissiles`) and mines (`blobMines`).
    pub missiles: Vec<MissileId>,
    pub mines: Vec<MissileId>,
}

/// The objects the blobs refer to.
pub struct Objects<'a> {
    pub ships: &'a mut ShipTable,
    pub bullets: &'a mut BulletTable,
    pub missiles: &'a mut MissileTable,
}

impl Objects<'_> {
    fn position(&self, r: ObjRef) -> Vector {
        match r {
            ObjRef::Ship(id) => self.ships.ship(id).posinfo.position,
            ObjRef::Bullet(id) => self.bullets.bullet(id).posinfo.position,
            ObjRef::Missile(id) => self.missiles.missile(id).posinfo.position,
        }
    }

    /// The point blob distances are measured from: ships use their collision centre.
    fn coll_point(&self, r: ObjRef) -> Vector {
        match r {
            ObjRef::Ship(id) => self.ships.ship(id).coll.coll_position,
            ObjRef::Bullet(id) => self.bullets.bullet(id).posinfo.position,
            ObjRef::Missile(id) => self.missiles.missile(id).posinfo.position,
        }
    }

    fn flags(&self, r: ObjRef) -> u32 {
        match r {
            ObjRef::Ship(id) => self.ships.ship(id).flags,
            ObjRef::Bullet(id) => self.bullets.bullet(id).flags,
            ObjRef::Missile(id) => self.missiles.missile(id).flags,
        }
    }

    fn set_flags(&mut self, r: ObjRef, f: impl FnOnce(u32) -> u32) {
        match r {
            ObjRef::Ship(id) => {
                let s = self.ships.ship_mut(id);
                s.flags = f(s.flags);
            }
            ObjRef::Bullet(id) => {
                let b = self.bullets.bullet_mut(id);
                b.flags = f(b.flags);
            }
            ObjRef::Missile(id) => {
                let m = self.missiles.missile_mut(id);
                m.flags = f(m.flags);
            }
        }
    }

    fn dist(&self, r: ObjRef) -> f32 {
        match r {
            ObjRef::Ship(id) => self.ships.ship(id).coll_optimize_dist,
            ObjRef::Bullet(id) => self.bullets.bullet(id).coll_optimize_dist,
            ObjRef::Missile(id) => self.missiles.missile(id).coll_optimize_dist,
        }
    }

    fn set_dist_blob(&mut self, r: ObjRef, d: f32, blob: Option<BlobId>) {
        match r {
            ObjRef::Ship(id) => {
                let s = self.ships.ship_mut(id);
                s.coll_optimize_dist = d;
                if blob.is_some() {
                    s.coll_my_blob = blob;
                }
            }
            ObjRef::Bullet(id) => {
                let b = self.bullets.bullet_mut(id);
                b.coll_optimize_dist = d;
                if blob.is_some() {
                    b.coll_my_blob = blob;
                }
            }
            ObjRef::Missile(id) => {
                let m = self.missiles.missile_mut(id);
                m.coll_optimize_dist = d;
                if blob.is_some() {
                    m.coll_my_blob = blob;
                }
            }
        }
    }

    /// The collision size and mass of an object (statics +0x48 and +8).
    fn size_and_mass(&self, r: ObjRef, statics: &StaticWorld) -> (f32, f32) {
        match r {
            ObjRef::Ship(id) => statics.ship(self.ships.ship(id).ship_type).map_or((0.0, 0.0), |st| (st.header.coll.coll_sphere_size, st.header.mass)),
            ObjRef::Bullet(_) => (0.0, 0.0),
            ObjRef::Missile(id) => {
                let m = self.missiles.missile(id);
                let table = if m.kind == crate::missile::kind::MINE { &statics.mines } else { &statics.missiles };
                table.get(m.race.index()).and_then(Option::as_ref).map_or((0.0, 0.0), |st| (st.header.coll.coll_sphere_size, st.header.mass))
            }
        }
    }
}

/// `sphereVolume(r)` and its reciprocal as the creation code computes them.
fn volume_and_reciprocal(radius: f32) -> (f32, f32) {
    let r = ld(radius);
    let v = ((r * r) * r) * ld(SPHERE_VOLUME_FACTOR);
    (st(v), st(1.0 / v))
}

/// `(x*x + y*y) + z*z` of a point.
fn sort_distance(p: Vector) -> f32 {
    let (x, y, z) = (ld(p.x), ld(p.y), ld(p.z));
    st((x * x + y * y) + z * z)
}

/// `(dz*dz + dy*dy) + dx*dx` of the difference, in registers.
fn dist_sq_ext(a: Vector, b: Vector) -> Ext {
    let dx = ld(a.x) - ld(b.x);
    let dy = ld(a.y) - ld(b.y);
    let dz = ld(a.z) - ld(b.z);
    (dz * dz + dy * dy) + dx * dx
}

impl Blob {
    fn single(r: ObjRef, position: Vector, mass: f32, props: &BlobProperties) -> Self {
        let (volume, one_over_volume) = volume_and_reciprocal(props.start_sphere_size);
        Blob {
            centre: position,
            radius: props.start_sphere_size,
            volume,
            one_over_volume,
            total_mass: mass,
            sort_distance: sort_distance(position),
            sqrt_sort_distance: 0.0,
            flags: 0,
            objects: vec![r],
            small_targets: Vec::new(),
            big_targets: Vec::new(),
            max_small_target_size: 0.0,
            max_big_target_size: 0.0,
            ships: Vec::new(),
            small_ships: Vec::new(),
            big_ships: Vec::new(),
            max_ship_size: 0.0,
            max_small_ship_size: 0.0,
            max_big_ship_size: 0.0,
            bullets: Vec::new(),
            missiles: Vec::new(),
            mines: Vec::new(),
        }
    }
}

/// `bobObjectListMedian` (0x426de0): centre of the objects' bounding box (padded by their
/// collision spheres) and half its largest side plus the combine margin.
fn object_list_median(objects: &[ObjRef], objs: &Objects, statics: &StaticWorld, props: &BlobProperties) -> (Vector, f32) {
    let mut min = [REALLY_BIG; 3];
    let mut max_x = -REALLY_BIG;
    let mut max_y = -REALLY_BIG;
    let mut max_z: Ext = ld(-REALLY_BIG); // kept in a register
    for &o in objects {
        let p = objs.position(o);
        if objs.flags(o) & sof::IMPACTABLE != 0 {
            let r = ld(objs.size_and_mass(o, statics).0);
            for (m, c) in min.iter_mut().zip([p.x, p.y, p.z]) {
                let v = ld(c) - r;
                if !(ld(*m) < v) {
                    *m = st(v);
                }
            }
            let vx = r + ld(p.x);
            if !(ld(max_x) > vx) {
                max_x = st(vx);
            }
            let vy = r + ld(p.y);
            if !(ld(max_y) > vy) {
                max_y = st(vy);
            }
            let vz = st(r + ld(p.z));
            if !(max_z > ld(vz)) {
                max_z = ld(vz);
            }
        } else {
            for (m, c) in min.iter_mut().zip([p.x, p.y, p.z]) {
                if !(ld(c) > ld(*m)) {
                    *m = c;
                }
            }
            if !(ld(p.x) < ld(max_x)) {
                max_x = p.x;
            }
            if !(ld(p.y) < ld(max_y)) {
                max_y = p.y;
            }
            if !(max_z > ld(p.z)) {
                max_z = ld(p.z);
            }
        }
    }
    let centre = Vector::new(
        st((ld(max_x) + ld(min[0])) * 0.5),
        st((ld(max_y) + ld(min[1])) * 0.5),
        st((max_z + ld(min[2])) * 0.5),
    );
    let dx = st(ld(max_x) - ld(min[0]));
    let dy = st(ld(max_y) - ld(min[1]));
    let dz = max_z - ld(min[2]);
    let dxy = if ld(dx) > ld(dy) { dx } else { dy };
    let biggest = if ld(dxy) > dz { ld(dxy) } else { dz };
    (centre, st(biggest * 0.5 + ld(props.radius_combine_margin)))
}

/// The universe's collision blob list (`universe.collBlobList`), in list order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct BlobList {
    pub blobs: Vec<Blob>,
    /// `universe.collUpdateAllBlobs`: drop every blob on the next rebuild.
    pub update_all: bool,
}

impl BlobList {
    pub fn get(&self, id: BlobId) -> Option<&Blob> {
        self.blobs.get(id.0 as usize)
    }

    /// `bobAllObjectsSatisfyBlob` (0x426810).
    fn satisfies(blob: &Blob, objs: &Objects, statics: &StaticWorld, props: &BlobProperties) -> bool {
        let r_sq = st(ld(blob.radius) * ld(blob.radius));
        if blob.flags & btf::RECENT_DEATH != 0 {
            return false;
        }
        let (centre, radius) = object_list_median(&blob.objects, objs, statics, props);
        if ld(radius) > ld(blob.radius) && ld(radius) / ld(blob.radius) > ld(UPDATE_RADIUS_THRESHOLD) {
            return false;
        }
        let d = dist_sq_ext(blob.centre, centre);
        !(d / ld(r_sq) > ld(UPDATE_CENTRE_THRESHOLD))
    }

    /// `bobAllObjectsFlagDontUpdate` (0x4268d0): a kept blob drops its bullets (they are put
    /// back after the rebuild) and flags its other objects.
    fn flag_dont_update(blob: &mut Blob, objs: &mut Objects) {
        let mut i = 0;
        while i < blob.objects.len() {
            let r = blob.objects[i];
            let flying_missile = matches!(r, ObjRef::Missile(m) if objs.missiles.missile(m).kind != crate::missile::kind::MINE);
            if matches!(r, ObjRef::Bullet(_)) || flying_missile {
                blob.objects.swap_remove(i);
                continue;
            }
            objs.set_flags(r, |f| f | sof::DONT_CREATE_BLOB);
            i += 1;
        }
        blob.sqrt_sort_distance = 0.0;
        blob.bullets.clear();
        blob.missiles.clear();
        blob.flags |= btf::DONT_UPDATE | btf::CLEAR_OBJECT_FLAGS;
    }

    /// `collUpdateCollBlobs` → `bobListCreate` (0x426970). `order` is the universe object list;
    /// the bullets of `bullet_list` are put back into blobs at the end.
    pub fn rebuild(
        &mut self,
        order: &[ObjRef],
        bullet_list: &[BulletId],
        missile_list: &[MissileId],
        objs: &mut Objects,
        statics: &StaticWorld,
        props: &BlobProperties,
    ) {
        if self.update_all {
            self.update_all = false;
            self.blobs.clear();
        }
        // Keep blobs that still fit their objects; flag those objects so they get no blob of
        // their own.
        if !self.blobs.is_empty() {
            let old = std::mem::take(&mut self.blobs);
            for mut blob in old {
                if Self::satisfies(&blob, objs, statics, props) {
                    Self::flag_dont_update(&mut blob, objs);
                    self.blobs.push(blob);
                }
            }
        }
        if order.is_empty() {
            return;
        }
        for &r in order {
            match r {
                ObjRef::Bullet(id) => {
                    objs.bullets.bullet_mut(id).coll_my_blob = None;
                }
                ObjRef::Ship(id) => {
                    let s = objs.ships.ship_mut(id);
                    if s.flags & (sof::DEAD | sof::DONT_CREATE_BLOB) != 0 {
                        s.coll_my_blob = None;
                        continue;
                    }
                    let mass = statics.ship(s.ship_type).map_or(0.0, |st| st.header.mass);
                    let p = s.posinfo.position;
                    self.blobs.push(Blob::single(r, p, mass, props));
                }
                ObjRef::Missile(id) => {
                    let m = objs.missiles.missile_mut(id);
                    if m.flags & (sof::DEAD | sof::DONT_CREATE_BLOB) != 0 || m.kind != crate::missile::kind::MINE {
                        m.coll_my_blob = None;
                        continue;
                    }
                    let p = m.posinfo.position;
                    self.blobs.push(Blob::single(r, p, f32::from_bits(0x3dcc_cccd), props));
                }
            }
        }
        merge_sort(&mut self.blobs, &|a: &Blob, b: &Blob| a.sort_distance > b.sort_distance);
        self.combine(objs, statics, props);
        self.update_extra_info(objs, statics);
        // Bullets join the blob of their target, or the nearest one.
        for &bid in bullet_list {
            let target_blob = objs
                .bullets
                .bullet(bid)
                .target
                .and_then(|t| objs.ships.get(t))
                .and_then(|t| t.coll_my_blob);
            match target_blob {
                Some(b) => self.add_to_specific(b, ObjRef::Bullet(bid), objs, statics),
                None => self.add_to_nearest(ObjRef::Bullet(bid), objs, statics),
            }
        }
        // Then the flying missiles (mines have blobs of their own).
        for &mid in missile_list {
            let m = objs.missiles.missile(mid);
            if m.flags & sof::DEAD != 0 || m.kind == crate::missile::kind::MINE {
                continue;
            }
            let target_blob = m.target.and_then(|t| objs.ships.get(t)).and_then(|t| t.coll_my_blob);
            match target_blob {
                Some(b) => self.add_to_specific(b, ObjRef::Missile(mid), objs, statics),
                None => self.add_to_nearest(ObjRef::Missile(mid), objs, statics),
            }
        }
    }

    /// `bobListUpdate` (0x427480): repeatedly merge overlapping blobs that would not get less
    /// dense (or stay dense and small enough).
    fn combine(&mut self, objs: &mut Objects, statics: &StaticWorld, props: &BlobProperties) {
        let biggest = ld(props.biggest_radius);
        loop {
            let mut hits = 0;
            let mut i = 0;
            while i < self.blobs.len() {
                if self.blobs[i].sqrt_sort_distance == 0.0 {
                    self.blobs[i].sqrt_sort_distance = st(fsqrt(ld(self.blobs[i].sort_distance)));
                }
                let this_sort = ld(self.blobs[i].sqrt_sort_distance);
                let mut j = i + 1;
                while j < self.blobs.len() {
                    if self.blobs[j].sqrt_sort_distance == 0.0 {
                        self.blobs[j].sqrt_sort_distance = st(fsqrt(ld(self.blobs[j].sort_distance)));
                    }
                    let (this, other) = (&self.blobs[i], &self.blobs[j]);
                    let check_dist = ld(other.sqrt_sort_distance) - this_sort;
                    if check_dist > biggest + ld(this.radius) {
                        break;
                    }
                    let radius = st(ld(this.radius) + ld(other.radius));
                    let check_radius = st(ld(radius) * ld(props.sqrt_overlap_factor));
                    if check_dist > ld(check_radius) {
                        j += 1;
                        continue;
                    }
                    let diff = Vector::new(
                        this.centre.x - other.centre.x,
                        this.centre.y - other.centre.y,
                        this.centre.z - other.centre.z,
                    );
                    let neg = -check_radius;
                    let outside = |c: f32| ld(c) < ld(neg) || c.is_nan() || ld(c) > ld(check_radius);
                    if outside(diff.x) || outside(diff.y) || outside(diff.z) {
                        j += 1;
                        continue;
                    }
                    let distance = (ld(diff.z) * ld(diff.z) + ld(diff.y) * ld(diff.y)) + ld(diff.x) * ld(diff.x);
                    if distance > ld(radius) * ld(radius) * ld(props.overlap_factor) {
                        j += 1;
                        continue;
                    }
                    let nr_ext = ((fsqrt(distance) + ld(this.radius)) + ld(other.radius)) * 0.5;
                    let new_radius = st(nr_ext);
                    let new_volume =
                        st(((nr_ext * ld(new_radius)) * ld(new_radius)) * ld(SPHERE_VOLUME_FACTOR));
                    let new_density = (ld(this.total_mass) + ld(other.total_mass)) / ld(new_volume);
                    let this_density = ld(this.one_over_volume) * ld(this.total_mass);
                    let other_density = st(ld(other.one_over_volume) * ld(other.total_mass));
                    let max_density = if this_density > ld(other_density) { this_density } else { ld(other_density) };
                    let merge = new_density > max_density
                        || (new_density > ld(props.density_high) && ld(new_radius) < ld(props.biggest_radius));
                    if merge {
                        self.merge(i, j, new_volume, objs, statics, props);
                        hits += 1;
                        // `j` now indexes the node after the absorbed one.
                    } else {
                        j += 1;
                    }
                }
                i += 1;
            }
            if hits == 0 {
                break;
            }
        }
        for bi in 0..self.blobs.len() {
            if self.blobs[bi].flags & btf::CLEAR_OBJECT_FLAGS != 0 {
                for k in 0..self.blobs[bi].objects.len() {
                    let r = self.blobs[bi].objects[k];
                    objs.set_flags(r, |f| f & !sof::DONT_CREATE_BLOB);
                }
                self.blobs[bi].flags &= !btf::CLEAR_OBJECT_FLAGS;
            }
            // (bobBlobItemize, the sensors summary of a blob, is not needed for movement.)
        }
    }

    /// `bobBlobCombine` (0x427010): blob `j` is absorbed into blob `i`.
    fn merge(&mut self, i: usize, j: usize, new_volume: f32, objs: &Objects, statics: &StaticWorld, props: &BlobProperties) {
        let other = self.blobs.remove(j);
        let this = &mut self.blobs[i];
        this.flags &= !btf::DONT_UPDATE;
        this.flags |= other.flags & btf::CLEAR_OBJECT_FLAGS;
        this.objects.extend_from_slice(&other.objects);
        this.small_targets.clear();
        this.big_targets.clear();
        this.ships.clear();
        this.small_ships.clear();
        this.big_ships.clear();
        this.bullets.clear();
        let (centre, radius) = object_list_median(&this.objects, objs, statics, props);
        this.centre = centre;
        this.radius = radius;
        this.volume = new_volume;
        this.one_over_volume = st(1.0 / ld(new_volume));
        this.total_mass = st(ld(this.total_mass) + ld(other.total_mass));
        this.sort_distance = sort_distance(centre);
        this.sqrt_sort_distance = 0.0;
    }

    /// `bobUpdateExtraCollBobInfo` (0x426180): distances, sort and per-kind lists after a
    /// rebuild (blobs kept unchanged keep their old lists).
    fn update_extra_info(&mut self, objs: &mut Objects, statics: &StaticWorld) {
        for (bi, blob) in self.blobs.iter_mut().enumerate() {
            for &r in &blob.objects {
                let d = st(fsqrt(dist_sq_ext(objs.coll_point(r), blob.centre)));
                objs.set_dist_blob(r, d, Some(BlobId(bi as u32)));
            }
            sort_by_optimize_dist(&mut blob.objects, objs);
            if blob.flags & btf::DONT_UPDATE != 0 {
                blob.flags &= !btf::DONT_UPDATE;
                continue;
            }
            fill_specific_lists(blob, objs, statics);
        }
    }

    /// `collUpdateObjsInCollBlobs` → `bobUpdateObjsInBlobCollInfo` (0x426070) for every blob.
    pub fn update_objects(&mut self, objs: &mut Objects, statics: &StaticWorld) {
        for blob in &mut self.blobs {
            // (The "getting attacked"/"attacking" flags 0x40/0x80 for the sensors are not
            // modelled.)
            blob.flags &= !0xc0;
            for &r in &blob.objects {
                let d = st(fsqrt(dist_sq_ext(objs.coll_point(r), blob.centre)));
                objs.set_dist_blob(r, d, None);
            }
            sort_by_optimize_dist(&mut blob.objects, objs);
            fill_specific_lists(blob, objs, statics);
        }
    }

    /// `bobFindNearestBlobToObject` (0x425cd0): nearest blob centre to `point` and the squared
    /// distance.
    pub fn nearest_to(&self, point: Vector) -> Option<(BlobId, f32)> {
        let mut min = ld(REALLY_BIG);
        let mut best = None;
        let mut min_f32 = REALLY_BIG;
        for (i, b) in self.blobs.iter().enumerate() {
            let d = dist_sq_ext(point, b.centre);
            if d < min {
                min = d;
                min_f32 = st(d);
                best = Some(BlobId(i as u32));
            }
        }
        best.map(|b| (b, min_f32))
    }

    /// `bobFindNearestBlobToObject` for a ship (its collision centre).
    pub fn nearest(&self, ship: &Ship) -> Option<(BlobId, f32)> {
        self.nearest_to(ship.coll.coll_position)
    }

    /// `bobAddObjToNearestBlob`.
    pub fn add_to_nearest(&mut self, r: ObjRef, objs: &mut Objects, statics: &StaticWorld) {
        let Some((id, dsq)) = self.nearest_to(objs.coll_point(r)) else {
            return;
        };
        objs.set_dist_blob(r, st(fsqrt(ld(dsq))), Some(id));
        self.add_to_lists(id, r, objs, statics);
    }

    /// `bobAddObjToSpecificBlob`.
    pub fn add_to_specific(&mut self, id: BlobId, r: ObjRef, objs: &mut Objects, statics: &StaticWorld) {
        let c = self.blobs[id.0 as usize].centre;
        let d = st(fsqrt(dist_sq_ext(objs.coll_point(r), c)));
        objs.set_dist_blob(r, d, Some(id));
        self.add_to_lists(id, r, objs, statics);
    }

    /// `AddObjToObjectListsOfBlob` (0x427b40): sorted insertion into every list the object
    /// belongs to (the maximum sizes are not updated, as in the original).
    fn add_to_lists(&mut self, id: BlobId, r: ObjRef, objs: &Objects, statics: &StaticWorld) {
        let blob = &mut self.blobs[id.0 as usize];
        let dist = |x: ObjRef| objs.dist(x);
        match r {
            ObjRef::Ship(sid) => {
                let s = objs.ships.ship(sid);
                let orig = statics.ship(s.ship_type).map_or(0.0, |st| st.header.coll.original_coll_sphere_size);
                let ship_dist = |x: &ShipId| dist(ObjRef::Ship(*x));
                if s.flags & sof::TARGETABLE != 0 {
                    let list = if orig < BIG_TARGET_SIZE { &mut blob.small_targets } else { &mut blob.big_targets };
                    insert_preserve_order(list, sid, &ship_dist);
                }
                let list = if orig < BIG_TARGET_SIZE { &mut blob.small_ships } else { &mut blob.big_ships };
                insert_preserve_order(list, sid, &ship_dist);
                insert_preserve_order(&mut blob.ships, sid, &ship_dist);
            }
            ObjRef::Bullet(bid) => {
                insert_preserve_order(&mut blob.bullets, bid, &|x: &BulletId| dist(ObjRef::Bullet(*x)));
            }
            ObjRef::Missile(mid) => {
                let list = if objs.missiles.missile(mid).kind == crate::missile::kind::MINE { &mut blob.mines } else { &mut blob.missiles };
                insert_preserve_order(list, mid, &|x: &MissileId| dist(ObjRef::Missile(*x)));
            }
        }
        insert_preserve_order(&mut blob.objects, r, &|x: &ObjRef| dist(*x));
    }

    /// `bobObjectDied`: removes an object from every blob list, keeping order; a dying ship
    /// marks its blob as recently changed.
    pub fn object_died(&mut self, r: ObjRef) {
        for blob in &mut self.blobs {
            let Some(i) = blob.objects.iter().position(|&o| o == r) else { continue };
            blob.objects.remove(i);
            match r {
                ObjRef::Ship(id) => {
                    for list in [&mut blob.small_targets, &mut blob.big_targets, &mut blob.small_ships, &mut blob.big_ships, &mut blob.ships] {
                        if let Some(k) = list.iter().position(|&s| s == id) {
                            list.remove(k);
                        }
                    }
                    blob.flags |= btf::RECENT_DEATH;
                }
                ObjRef::Bullet(id) => {
                    if let Some(k) = blob.bullets.iter().position(|&b| b == id) {
                        blob.bullets.remove(k);
                    }
                }
                ObjRef::Missile(id) => {
                    for list in [&mut blob.missiles, &mut blob.mines] {
                        if let Some(k) = list.iter().position(|&m| m == id) {
                            list.remove(k);
                        }
                    }
                }
            }
            return;
        }
    }
}

/// `AddSpaceObjToSelectionPreserveOrder` (0x427ac0): binary search on the distance, then
/// insert after an element whose distance is not larger, else before it.
fn insert_preserve_order<T: Copy>(list: &mut Vec<T>, item: T, dist: &dyn Fn(&T) -> f32) {
    if list.is_empty() {
        list.push(item);
        return;
    }
    let d = ld(dist(&item));
    let (mut bottom, mut top) = (0i32, list.len() as i32 - 1);
    let mut mid = 0i32;
    while bottom <= top {
        mid = (top + bottom) >> 1;
        if d <= ld(dist(&list[mid as usize])) {
            top = mid - 1;
        } else {
            bottom = mid + 1;
        }
    }
    let m = mid as usize;
    if ld(dist(&list[m])) <= d {
        list.insert(m + 1, item);
    } else {
        list.insert(m, item);
    }
}

/// `qsort(blobObjects, CompareSpaceObjCollOptimizeDistCB)`: the comparator never returns 0.
fn sort_by_optimize_dist(objects: &mut [ObjRef], objs: &Objects) {
    let mut items: Vec<(ObjRef, f32)> = objects.iter().map(|&r| (r, objs.dist(r))).collect();
    qsort(&mut items, |a, b| if ld(a.1) > ld(b.1) { 1 } else { -1 });
    for (o, (r, _)) in objects.iter_mut().zip(items) {
        *o = r;
    }
}

/// `FillInCollisionBlobSpecificLists` (0x425db0).
fn fill_specific_lists(blob: &mut Blob, objs: &Objects, statics: &StaticWorld) {
    blob.small_targets.clear();
    blob.big_targets.clear();
    blob.ships.clear();
    blob.small_ships.clear();
    blob.big_ships.clear();
    blob.bullets.clear();
    blob.missiles.clear();
    blob.mines.clear();
    blob.max_small_target_size = 0.0;
    blob.max_big_target_size = 0.0;
    blob.max_ship_size = 0.0;
    blob.max_small_ship_size = 0.0;
    blob.max_big_ship_size = 0.0;
    for &r in &blob.objects {
        let id = match r {
            ObjRef::Ship(id) => id,
            ObjRef::Bullet(b) => {
                blob.bullets.push(b);
                continue;
            }
            ObjRef::Missile(m) => {
                if objs.missiles.missile(m).kind == crate::missile::kind::MINE {
                    blob.mines.push(m);
                } else {
                    blob.missiles.push(m);
                }
                continue;
            }
        };
        let s = objs.ships.ship(id);
        let Some(stat) = statics.ship(s.ship_type) else { continue };
        let orig = stat.header.coll.original_coll_sphere_size;
        if s.flags & sof::TARGETABLE != 0 {
            if orig < BIG_TARGET_SIZE {
                blob.small_targets.push(id);
                if blob.max_small_target_size < orig {
                    blob.max_small_target_size = orig;
                }
            } else {
                blob.big_targets.push(id);
                if blob.max_big_target_size < orig {
                    blob.max_big_target_size = orig;
                }
            }
        }
        if orig < BIG_TARGET_SIZE {
            blob.small_ships.push(id);
            if blob.max_small_ship_size < orig {
                blob.max_small_ship_size = orig;
            }
        } else {
            blob.big_ships.push(id);
            if blob.max_big_ship_size < orig {
                blob.max_big_ship_size = orig;
            }
        }
        blob.ships.push(id);
        let size = stat.header.coll.coll_sphere_size;
        if blob.max_ship_size < size {
            blob.max_ship_size = size;
        }
    }
}

/// `listMergeSortGeneral`: the linked-list merge sort (stable; `greater(a, b)` true moves `b`
/// ahead of `a`).
fn merge_sort<T, F: Fn(&T, &T) -> bool>(items: &mut Vec<T>, greater: &F) {
    let n = items.len();
    if n <= 1 {
        return;
    }
    if n == 2 {
        if greater(&items[0], &items[1]) {
            items.swap(0, 1);
        }
        return;
    }
    let half = n >> 1;
    let mut right = items.split_off(half);
    let mut left = std::mem::take(items);
    merge_sort(&mut left, greater);
    merge_sort(&mut right, greater);
    let mut l = left.into_iter().peekable();
    let mut r = right.into_iter().peekable();
    loop {
        let take_right = match (l.peek(), r.peek()) {
            (None, None) => break,
            (None, Some(_)) => true,
            (Some(_), None) => false,
            (Some(a), Some(b)) => greater(a, b),
        };
        let next = if take_right { r.next() } else { l.next() };
        items.push(next.expect("peeked"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_sort_is_stable_on_ties() {
        let mut v = vec![(3, 'a'), (1, 'b'), (3, 'c'), (2, 'd'), (1, 'e')];
        merge_sort(&mut v, &|a: &(i32, char), b: &(i32, char)| a.0 > b.0);
        assert_eq!(v, vec![(1, 'b'), (1, 'e'), (2, 'd'), (3, 'a'), (3, 'c')]);
    }

    #[test]
    fn sphere_volume_uses_folded_constant() {
        let (v, r) = volume_and_reciprocal(3000.0);
        assert!((v - 1.130_973_4e11).abs() / v < 1e-6);
        assert!((r * v - 1.0).abs() < 1e-6);
    }
}
