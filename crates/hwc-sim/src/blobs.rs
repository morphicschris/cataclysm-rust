//! Collision blobs (`Blobs.c`, `Collision.c` broad phase): spheres that group nearby objects so
//! that avoidance and collision checks only look at neighbours.
//!
//! `collUpdateCollBlobs` rebuilds the list every `REFRESH_COLLBLOB_RATE + 1` frames (keeping
//! blobs whose objects have not drifted), and `collUpdateObjsInCollBlobs` refreshes each
//! object's distance from its blob centre on the other frames. Objects in a blob are kept
//! sorted by that distance (`collOptimizeDist`) with the CRT `qsort`, and split into the
//! small/big target lists the avoidance code walks.
//!
//! Only ships live in the universe so far, so blobs hold ship ids.

use hwc_math::crt::qsort;
use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st, Ext};
use hwc_math::Vector;

use crate::object::{sof, BlobId, Ship, ShipId, ShipTable};
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
    pub objects: Vec<ShipId>,
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

/// Distance used for blob membership: ships measure from their collision centre.
fn coll_point(ship: &Ship) -> Vector {
    ship.coll.coll_position
}

/// `(dz*dz + dy*dy) + dx*dx` of the difference, in registers.
fn dist_sq_ext(a: Vector, b: Vector) -> Ext {
    let dx = ld(a.x) - ld(b.x);
    let dy = ld(a.y) - ld(b.y);
    let dz = ld(a.z) - ld(b.z);
    (dz * dz + dy * dy) + dx * dx
}

impl Blob {
    fn single(ship: &Ship, mass: f32, props: &BlobProperties) -> Self {
        let (volume, one_over_volume) = volume_and_reciprocal(props.start_sphere_size);
        Blob {
            centre: ship.posinfo.position,
            radius: props.start_sphere_size,
            volume,
            one_over_volume,
            total_mass: mass,
            sort_distance: sort_distance(ship.posinfo.position),
            sqrt_sort_distance: 0.0,
            flags: 0,
            objects: vec![ship.id],
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
        }
    }
}

/// `bobObjectListMedian` (0x426de0): centre of the objects' bounding box (padded by their
/// collision spheres) and half its largest side plus the combine margin.
fn object_list_median(objects: &[ShipId], ships: &ShipTable, statics: &StaticWorld, props: &BlobProperties) -> (Vector, f32) {
    let mut min = [REALLY_BIG; 3];
    let mut max_x = -REALLY_BIG;
    let mut max_y = -REALLY_BIG;
    let mut max_z: Ext = ld(-REALLY_BIG); // kept in a register
    for &id in objects {
        let s = ships.ship(id);
        let p = s.posinfo.position;
        if s.flags & sof::IMPACTABLE != 0 {
            let r = ld(statics.ship(s.ship_type).map_or(0.0, |st| st.header.coll.coll_sphere_size));
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
    fn satisfies(blob: &Blob, ships: &ShipTable, statics: &StaticWorld, props: &BlobProperties) -> bool {
        let r_sq = st(ld(blob.radius) * ld(blob.radius));
        if blob.flags & btf::RECENT_DEATH != 0 {
            return false;
        }
        let (centre, radius) = object_list_median(&blob.objects, ships, statics, props);
        if ld(radius) > ld(blob.radius) && ld(radius) / ld(blob.radius) > ld(UPDATE_RADIUS_THRESHOLD) {
            return false;
        }
        let d = dist_sq_ext(blob.centre, centre);
        !(d / ld(r_sq) > ld(UPDATE_CENTRE_THRESHOLD))
    }

    /// `collUpdateCollBlobs` → `bobListCreate` (0x426970).
    pub fn rebuild(&mut self, order: &[ShipId], ships: &mut ShipTable, statics: &StaticWorld, props: &BlobProperties) {
        if self.update_all {
            self.update_all = false;
            self.blobs.clear();
        }
        // Keep blobs that still fit their objects; flag those objects so they get no blob of
        // their own.
        if !self.blobs.is_empty() {
            let old = std::mem::take(&mut self.blobs);
            for mut blob in old {
                if Self::satisfies(&blob, ships, statics, props) {
                    for &id in &blob.objects {
                        ships.ship_mut(id).flags |= sof::DONT_CREATE_BLOB;
                    }
                    blob.sqrt_sort_distance = 0.0;
                    blob.flags |= btf::DONT_UPDATE | btf::CLEAR_OBJECT_FLAGS;
                    self.blobs.push(blob);
                }
            }
        }
        if order.is_empty() {
            return;
        }
        for &id in order {
            let s = ships.ship_mut(id);
            if s.flags & (sof::DEAD | sof::DONT_CREATE_BLOB) != 0 {
                s.coll_my_blob = None;
                continue;
            }
            let mass = statics.ship(s.ship_type).map_or(0.0, |st| st.header.mass);
            self.blobs.push(Blob::single(s, mass, props));
        }
        merge_sort(&mut self.blobs, &|a: &Blob, b: &Blob| a.sort_distance > b.sort_distance);
        self.combine(ships, statics, props);
        self.update_extra_info(ships, statics);
    }

    /// `bobListUpdate` (0x427480): repeatedly merge overlapping blobs that would not get less
    /// dense (or stay dense and small enough).
    fn combine(&mut self, ships: &mut ShipTable, statics: &StaticWorld, props: &BlobProperties) {
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
                        self.merge(i, j, new_volume, ships, statics, props);
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
        for blob in &mut self.blobs {
            if blob.flags & btf::CLEAR_OBJECT_FLAGS != 0 {
                for &id in &blob.objects {
                    ships.ship_mut(id).flags &= !sof::DONT_CREATE_BLOB;
                }
                blob.flags &= !btf::CLEAR_OBJECT_FLAGS;
            }
            // (bobBlobItemize, the sensors summary of a blob, is not needed for movement.)
        }
    }

    /// `bobBlobCombine` (0x427010): blob `j` is absorbed into blob `i`.
    fn merge(&mut self, i: usize, j: usize, new_volume: f32, ships: &ShipTable, statics: &StaticWorld, props: &BlobProperties) {
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
        let (centre, radius) = object_list_median(&this.objects, ships, statics, props);
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
    fn update_extra_info(&mut self, ships: &mut ShipTable, statics: &StaticWorld) {
        for (bi, blob) in self.blobs.iter_mut().enumerate() {
            for &id in &blob.objects {
                let s = ships.ship_mut(id);
                s.coll_optimize_dist = st(fsqrt(dist_sq_ext(coll_point(s), blob.centre)));
                s.coll_my_blob = Some(BlobId(bi as u32));
            }
            sort_by_optimize_dist(&mut blob.objects, ships);
            if blob.flags & btf::DONT_UPDATE != 0 {
                blob.flags &= !btf::DONT_UPDATE;
                continue;
            }
            fill_specific_lists(blob, ships, statics);
        }
    }

    /// `collUpdateObjsInCollBlobs` → `bobUpdateObjsInBlobCollInfo` (0x426070) for every blob.
    pub fn update_objects(&mut self, ships: &mut ShipTable, statics: &StaticWorld) {
        for blob in &mut self.blobs {
            // (The "getting attacked"/"attacking" flags 0x40/0x80 for the sensors are not
            // modelled.)
            blob.flags &= !0xc0;
            for &id in &blob.objects {
                let s = ships.ship_mut(id);
                s.coll_optimize_dist = st(fsqrt(dist_sq_ext(coll_point(s), blob.centre)));
            }
            sort_by_optimize_dist(&mut blob.objects, ships);
            fill_specific_lists(blob, ships, statics);
        }
    }

    /// `bobFindNearestBlobToObject` (0x425cd0): nearest blob centre and the squared distance.
    pub fn nearest(&self, ship: &Ship) -> Option<(BlobId, f32)> {
        let mut min = ld(REALLY_BIG);
        let mut best = None;
        let mut min_f32 = REALLY_BIG;
        for (i, b) in self.blobs.iter().enumerate() {
            let d = dist_sq_ext(coll_point(ship), b.centre);
            if d < min {
                min = d;
                min_f32 = st(d);
                best = Some(BlobId(i as u32));
            }
        }
        best.map(|b| (b, min_f32))
    }

    /// `collAddSpaceObjToCollBlobs` → `bobAddObjToNearestBlob`: a new object joins the nearest
    /// blob until the next rebuild. (The original inserts it into the specific lists; here
    /// it is appended, which only matters for objects created between rebuilds.)
    pub fn add_to_nearest(&mut self, ship: &mut Ship, statics: &StaticWorld) {
        let Some((id, dsq)) = self.nearest(ship) else {
            return;
        };
        ship.coll_optimize_dist = st(fsqrt(ld(dsq)));
        ship.coll_my_blob = Some(id);
        let blob = &mut self.blobs[id.0 as usize];
        blob.objects.push(ship.id);
        if let Some(stat) = statics.ship(ship.ship_type) {
            let size = stat.header.coll.original_coll_sphere_size;
            if ship.flags & sof::TARGETABLE != 0 {
                if size < BIG_TARGET_SIZE {
                    blob.small_targets.push(ship.id);
                } else {
                    blob.big_targets.push(ship.id);
                }
            }
            blob.ships.push(ship.id);
        }
    }
}

/// `qsort(blobObjects, CompareSpaceObjCollOptimizeDistCB)`: the comparator never returns 0.
fn sort_by_optimize_dist(objects: &mut [ShipId], ships: &ShipTable) {
    let dist: Vec<(ShipId, f32)> = objects.iter().map(|&id| (id, ships.ship(id).coll_optimize_dist)).collect();
    let mut items = dist;
    qsort(&mut items, |a, b| if ld(a.1) > ld(b.1) { 1 } else { -1 });
    for (o, (id, _)) in objects.iter_mut().zip(items) {
        *o = id;
    }
}

/// `FillInCollisionBlobSpecificLists` (0x425db0) for a ship-only blob.
fn fill_specific_lists(blob: &mut Blob, ships: &ShipTable, statics: &StaticWorld) {
    blob.small_targets.clear();
    blob.big_targets.clear();
    blob.ships.clear();
    blob.small_ships.clear();
    blob.big_ships.clear();
    blob.max_small_target_size = 0.0;
    blob.max_big_target_size = 0.0;
    blob.max_ship_size = 0.0;
    blob.max_small_ship_size = 0.0;
    blob.max_big_ship_size = 0.0;
    for &id in &blob.objects {
        let s = ships.ship(id);
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
