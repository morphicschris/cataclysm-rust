//! Repair droids (`RepairDroid` module, 0x5bd350..0x5be690): the little robots that crawl over
//! the hull of a damaged Somtaaw ship whose type has `repairDroids`, throwing sparks.
//!
//! In the original they are purely visual: `univShipRegenerateHealth` updates a ship's droid
//! array (`droidUpdateArray`) only while the "repair bots" video option is on, and nothing in
//! the array touches the ship's health (the health comes back through the regeneration itself).
//! The droids draw their random numbers from the particle stream (`ranRandomFn(0)`), which the
//! simulation does not otherwise use. The array is created for every ship with droids
//! (`droidCreateArray` from `univCreateShip`); [`update_array`] is meant to be driven with the
//! ship's level-0 hull mesh ([`HullMesh`]) by whoever presents the droids. Without a mesh the
//! droids still fly between their dock points but never land on the hull.
//!
//! With effects attached each spark is the original's welding effect (`EGT_WELDING_EFFECT`) on
//! the host, at the hull point, facing the droid.

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::object::ShipId;
use crate::statics::{DockPointType, ShipRace, ShipStaticInfo};
use crate::universe::Universe;

/// The particle stream the droids use (`ranRandomFn(0)`).
const RAN_DROIDS: usize = 0;
/// "No vertex / no polygon": the original stores -1 in a float slot.
const NONE: i32 = -1;

/// Per-ship state (`sRepairDroid` and `bRepairDroid` ships have none of their own).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepairDroidSpec {}

/// Per-type statics of the repair-droid ship types (`RepairDroidStaticInit`, 0x5bd350:
/// `repositionTime`, `circleRange`, `fullMovementFreedom`, at 0x8d35f8).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RepairDroidStatics {
    pub reposition_time: f32,
    pub circle_range: f32,
    pub full_movement_freedom: bool,
}

impl RepairDroidStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for (k, v) in &stat.custom_entries {
            let first = v.split_whitespace().next().unwrap_or("");
            match k.as_str() {
                "repositionTime" => s.reposition_time = crate::statics::cscan::scan_f32(first).unwrap_or(s.reposition_time),
                "circleRange" => s.circle_range = crate::statics::cscan::scan_f32(first).unwrap_or(s.circle_range),
                "fullMovementFreedom" => s.full_movement_freedom = matches!(first.to_ascii_uppercase().as_str(), "TRUE" | "1"),
                _ => {}
            }
        }
        s
    }
}

pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

/// What a droid is doing (the state byte of its slot).
pub mod state {
    /// Inside the ship.
    pub const DOCKED: u8 = 0;
    /// Flying out from its dock point.
    pub const LAUNCHING: u8 = 1;
    /// Crawling to a hull vertex.
    pub const MOVING: u8 = 2;
    /// Welding (sparks).
    pub const REPAIRING: u8 = 3;
    /// Moving on to the next vertex.
    pub const REPOSITIONING: u8 = 4;
    /// Heading back to the dock point.
    pub const RETURNING: u8 = 5;
    /// Flying in.
    pub const DOCKING: u8 = 6;
}

/// One droid (a 0x2c-byte slot plus its 0x63c-byte stand-in ship object). Positions are in the
/// host ship's coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Droid {
    /// +0x04: see [`state`].
    pub state: u8,
    /// +0x08: where it is heading.
    pub dest: Vector,
    /// +0x14: the hull vertex it sits on, +0x18 the polygon it walks, +0x1c the vertex
    /// before the last edge step on the way home ([`NONE`]: none).
    pub vertex: i32,
    pub poly: i32,
    pub prev_vertex: i32,
    /// +0x20 / +0x24: when it stops crawling or welding, the next spark; +0x28 is only ever
    /// reset.
    pub timer: f32,
    pub spark_time: f32,
    pub spare: f32,
    /// The droid object: position (+0x58), velocity (+0x64), heading column (+0xb0) and the
    /// slot after it the original abuses for its turn towards the hull normal (+0xbc).
    pub position: Vector,
    pub velocity: Vector,
    pub heading: Vector,
    pub turn: Vector,
}

impl Default for Droid {
    fn default() -> Self {
        Self {
            state: state::DOCKED,
            dest: Vector::ZERO,
            vertex: 0,
            poly: 0,
            prev_vertex: 0,
            timer: -1.0,
            spark_time: -1.0,
            spare: 0.0,
            position: Vector::ZERO,
            velocity: Vector::ZERO,
            // univInitSpaceObjPosRot: the default ship orientation (heading +X).
            heading: crate::universe::DEFAULT_SHIP_MATRIX.col(2),
            turn: Vector::ZERO,
        }
    }
}

/// A ship's droids (`droidCreateArray`, 0x18 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct DroidArray {
    pub droids: Vec<Droid>,
    /// +0x0c: the dock point the last droid left from.
    pub dock_index: i32,
    /// +0x10: when the next droid may leave.
    pub next_launch: f32,
    /// +0x14: the host's health at the last update.
    pub last_health: f32,
    /// A spark went off this update at this hull position (ship coordinates) — for the
    /// presentation; the original creates its spark effect here.
    pub sparks: Vec<Vector>,
}

/// The level-0 hull mesh of the host, which the droids crawl over (`meshObject`: vertices
/// with a normal index, normals, triangles).
pub trait HullMesh {
    fn num_vertices(&self) -> usize;
    /// Vertex position and the index of its normal.
    fn vertex(&self, i: usize) -> (Vector, usize);
    fn normal(&self, i: usize) -> Vector;
    fn num_polys(&self) -> usize;
    /// The three vertex indices of a triangle.
    fn poly(&self, i: usize) -> [u16; 3];
    /// `meshObjectNearestVertexIndex`.
    fn nearest_vertex(&self, p: Vector) -> usize;
}

/// `droidCreateArray` (0x5bda10): `count` droids, all inside. The Beast has none (and other
/// races beyond the Traders are unsupported).
pub fn create_array(u: &Universe, id: ShipId, count: usize) -> Option<DroidArray> {
    let s = u.ships.get(id)?;
    if matches!(s.ship_race, ShipRace::Beast | ShipRace::Derelict) {
        return None;
    }
    Some(DroidArray { droids: vec![Droid::default(); count], dock_index: -1, next_launch: -1.0, last_health: s.health, sparks: Vec::new() })
}

/// What `univCreateShip` does: Somtaaw ships with `repairDroids` get an array.
pub fn create_for_ship(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    if stat.repair_droids == 0 || stat.ship_race != ShipRace::Somtaaw {
        return;
    }
    let arr = create_array(u, id, stat.repair_droids.max(0) as usize).map(Box::new);
    u.ships.ship_mut(id).droids = arr;
}

/// The next repair-bot dock point (`droidGetNextDock`, inlined in `droidUpdateArray`).
fn next_dock(stat: &ShipStaticInfo, arr: &mut DroidArray) -> Option<crate::statics::DockStaticPoint> {
    let dock = stat.dock.as_ref()?;
    let t = DockPointType::Repairbot.index();
    let count = dock.count_of_type[t] as i8 as i32;
    if count < 1 {
        return None;
    }
    arr.dock_index += 1;
    if count <= arr.dock_index {
        arr.dock_index = 0;
    }
    dock.points.get((dock.first_of_type[t] as i8 as i32 + arr.dock_index) as usize).cloned()
}

fn wrap(v: i32, n: i32) -> i32 {
    if n <= v {
        v % n
    } else {
        v
    }
}

/// `droidNextDockIndex` (0x5bd380): walk to a random neighbour of the vertex over the next
/// polygons that contain it.
pub fn next_vertex(u: &mut Universe, mesh: &dyn HullMesh, poly: &mut i32, vertex: &mut i32) {
    let nv = mesh.num_vertices() as i32;
    let np = mesh.num_polys() as i32;
    *vertex = wrap(*vertex, nv.max(1));
    let mut n = 0;
    while n <= np {
        *poly = wrap(*poly + 1, np.max(1));
        let p = mesh.poly(*poly as usize).map(i32::from);
        let r = u.randy.random(RAN_DROIDS) & 3;
        let v = *vertex;
        match r {
            0 => {
                if v == p[0] {
                    *vertex = p[1];
                    return;
                } else if v == p[1] {
                    *vertex = p[2];
                    return;
                } else if v == p[2] {
                    *vertex = p[0];
                    return;
                }
            }
            1 => {
                if v == p[0] {
                    *vertex = p[2];
                    return;
                } else if v == p[1] {
                    *vertex = p[1];
                    return;
                } else if v == p[2] {
                    *vertex = p[0];
                    return;
                }
            }
            2 => {
                if v == p[0] {
                    *vertex = p[2];
                    return;
                } else if v == p[1] {
                    *vertex = p[0];
                    return;
                } else if v == p[2] {
                    *vertex = p[1];
                    return;
                }
            }
            _ => {
                *vertex = if nv < 1 { 0 } else { (u.randy.random(RAN_DROIDS) % nv as u32) as i32 };
            }
        }
        n += 1;
    }
    *poly = 0;
    *vertex = 0;
}

/// `droidPickNextDockEdge` (0x5bd520): of the edges leaving the vertex, the one pointing most
/// along `dir`.
pub fn pick_next_edge(mesh: &dyn HullMesh, dir: Vector, poly: &mut i32, vertex: &mut i32) {
    let nv = mesh.num_vertices() as i32;
    let np = mesh.num_polys() as i32;
    *vertex = wrap(*vertex, nv.max(1));
    *poly = wrap(*poly, np.max(1));
    let mut best_poly = *poly;
    let mut best_vertex = *vertex;
    let mut best = -2.0f32;
    *poly -= 1;
    if np < 0 {
        return;
    }
    let pos = |i: i32| mesh.vertex(i as usize).0;
    let mut n = 0;
    while n <= np {
        *poly = wrap(*poly + 1, np.max(1));
        let p = mesh.poly(*poly as usize).map(i32::from);
        for (k, &pv) in p.iter().enumerate() {
            if *vertex != pv {
                continue;
            }
            let others = match k {
                0 => [p[1], p[2]],
                1 => [p[0], p[2]],
                _ => [p[0], p[1]],
            };
            for other in others {
                let a = pos(*vertex);
                let b = pos(other);
                let mut e = Vector::new(b.x - a.x, b.y - a.y, b.z - a.z);
                e.normalize();
                let d = st(ld(e.z) * ld(dir.z) + ld(e.x) * ld(dir.x) + ld(e.y) * ld(dir.y));
                if best <= d {
                    best_poly = *poly;
                    best_vertex = other;
                    best = d;
                }
            }
        }
        n += 1;
    }
    *vertex = best_vertex;
    *poly = best_poly;
}

/// `droidIsValidPredockPos` (0x5bd970): whether `p` lies inside the dock point's cone.
pub fn is_valid_predock_pos(point: &crate::statics::DockStaticPoint, p: Vector) -> bool {
    let mut d = Vector::new(point.position.x - p.x, point.position.y - p.y, point.position.z - p.z);
    d.normalize();
    let mut lim = st(ld(point.cone_angle) * 0.318_309_87);
    if lim < 0.0 {
        lim = -lim;
    }
    let dot = ld(d.x) * ld(point.cone_normal.x) + ld(d.y) * ld(point.cone_normal.y) + ld(d.z) * ld(point.cone_normal.z);
    dot >= ld(lim)
}

/// Special-purpose slot of the sparks (`EGT_WELDING_EFFECT`, 0xae7050).
const WELDING_EFFECT: usize = 20;

/// The spark of a welding droid (in `droidUpdateArray`): on the host at the hull point `at`,
/// facing the droid at `droid` (both in the host's coordinates), told how far away the droid
/// is.
fn weld_effect(u: &mut Universe, id: ShipId, at: Vector, droid: Vector) {
    if u.effects.is_none() {
        return;
    }
    let s = u.ships.ship(id);
    let cs = s.rotinfo.coordsys;
    let p = s.posinfo.position;
    let world = |v: Vector| {
        let o = cs.mul_vec(v);
        Vector::new(st(ld(o.x) + ld(p.x)), st(ld(o.y) + ld(p.y)), st(ld(o.z) + ld(p.z)))
    };
    let spark = world(at);
    let to = world(droid);
    let mut dir = Vector::new(st(ld(to.x) - ld(spark.x)), st(ld(to.y) - ld(spark.y)), st(ld(to.z) - ld(spark.z)));
    let dist = st(hwc_math::fastmath::fmath_sqrt_double((ld(dir.z) * ld(dir.z) + ld(dir.y) * ld(dir.y)) + ld(dir.x) * ld(dir.x)));
    dir.normalize();
    let heading = hwc_math::Matrix::coord_sys_from_heading(dir);
    let Some(effect) = crate::shockwave::fx::special_clamped(u, WELDING_EFFECT) else { return };
    if !crate::shockwave::fx::hit_enabled(u) || !crate::effects::frequency_ok(u, effect) || !crate::effects::in_render_list(u, id.into()) {
        return;
    }
    let params = [0x20, crate::effects::pf(5.0), crate::effects::pf(dist)];
    crate::effects::create(u, effect, Some(id.into()), Some(spark), None, Some(heading), 1.0, crate::effects::attach::POSITION, &params);
}

fn hull_full(u: &Universe, id: ShipId) -> bool {
    let s = u.ships.ship(id);
    let max = u.statics.ship(s.ship_type).map_or(0.0, |st| st.health.max_health);
    max <= s.health
}

fn dist_sq(v: Vector) -> f32 {
    st(ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y) + ld(v.z) * ld(v.z))
}

/// `droidUpdateArray` (0x5bdc90): one update of a ship's droids. `far` is the original's level
/// of detail test (the ship drawn at LOD 3 or coarser): far away the droids go home.
pub fn update_array(u: &mut Universe, id: ShipId, mesh: Option<&dyn HullMesh>, far: bool) {
    let Some(mut arr) = u.ships.get_mut(id).and_then(|s| s.droids.take()) else { return };
    arr.sparks.clear();
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(u.ships.ship(id).ship_type) else {
        u.ships.ship_mut(id).droids = Some(arr);
        return;
    };
    let dt = u.phys_time_elapsed;
    for i in 0..arr.droids.len() {
        let now = u.total_time_elapsed;
        let done = hull_full(u, id) || far;
        let mut d = arr.droids[i];
        if d.state == state::DOCKED {
            if done || (now <= arr.next_launch && i != 0) {
                continue;
            }
            arr.next_launch = st(ld(now) + 2.5);
            match next_dock(stat, &mut arr) {
                Some(p) => {
                    let k = ld(p.fly_away_dist);
                    d.dest = Vector::new(
                        st(ld(st(k * ld(p.cone_normal.x))) + ld(p.position.x)),
                        st(ld(p.position.y) + ld(st(k * ld(p.cone_normal.y)))),
                        st(ld(p.position.z) + ld(st(k * ld(p.cone_normal.z)))),
                    );
                    d.position = p.position;
                }
                None => {
                    d.dest = Vector::new(1000.0, 0.0, 0.0);
                    d.position = Vector::ZERO;
                }
            }
            d.vertex = NONE;
            d.poly = NONE;
            d.timer = -1.0;
            d.spark_time = -1.0;
            d.spare = -1.0;
            d.state = state::LAUNCHING;
            arr.droids[i] = d;
            continue;
        }
        // Steer towards the destination: the vertex (lifted 20 along its normal) when on one.
        let mut on_vertex = None;
        if d.vertex != NONE {
            if let Some(m) = mesh {
                d.vertex = wrap(d.vertex, (m.num_vertices() as i32).max(1));
                let (vp, ni) = m.vertex(d.vertex as usize);
                let n = m.normal(ni);
                d.dest = Vector::new(st(ld(st(ld(n.x) * 20.0)) + ld(vp.x)), st(ld(vp.y) + ld(st(ld(n.y) * 20.0))), st(ld(vp.z) + ld(st(ld(n.z) * 20.0))));
                on_vertex = Some((vp, n));
            }
        }
        let v = Vector::new(st(ld(st(ld(d.dest.x) - ld(d.position.x))) * 0.7), st(ld(st(ld(d.dest.y) - ld(d.position.y))) * 0.7), st(ld(st(ld(d.dest.z) - ld(d.position.z))) * 0.7));
        d.velocity = v;
        if let Some((_, n)) = on_vertex {
            d.turn = Vector::new(st(ld(st(ld(d.heading.x) - ld(n.x))) * 0.7), st(ld(st(ld(d.heading.y) - ld(n.y))) * 0.7), st(ld(st(ld(d.heading.z) - ld(n.z))) * 0.7));
        }
        d.position = Vector::new(st(ld(v.x) * ld(dt) + ld(d.position.x)), st(ld(v.y) * ld(dt) + ld(d.position.y)), st(ld(v.z) * ld(dt) + ld(d.position.z)));
        let speed_sq = dist_sq(v);
        match d.state {
            state::LAUNCHING => {
                if done {
                    d.state = state::RETURNING;
                } else if speed_sq <= 90.0 {
                    if let Some(m) = mesh {
                        d.vertex = m.nearest_vertex(d.position) as i32;
                    }
                    let r = u.randy.random(RAN_DROIDS) % 0x708;
                    d.state = state::MOVING;
                    d.timer = st(ld(st(f64::from(r) * 0.01 + ld(u.total_time_elapsed))) + 2.0);
                }
            }
            state::MOVING => {
                if speed_sq <= 25.0 {
                    if done {
                        d.state = state::RETURNING;
                    } else if now <= d.timer {
                        if let Some(m) = mesh {
                            next_vertex(u, m, &mut d.poly, &mut d.vertex);
                        }
                    } else {
                        d.state = state::REPAIRING;
                    }
                }
            }
            state::REPAIRING => {
                if now <= d.timer {
                    if d.spark_time < now {
                        if speed_sq <= 50.0 {
                            let at = on_vertex.map_or(Vector::ZERO, |(p, _)| p);
                            arr.sparks.push(at);
                            weld_effect(u, id, at, d.position);
                        }
                        let r = u.randy.random(RAN_DROIDS) % 100;
                        d.spark_time = st(f64::from(r) * 0.02 + ld(now) + 0.5);
                    }
                } else {
                    if let Some(m) = mesh {
                        next_vertex(u, m, &mut d.poly, &mut d.vertex);
                    }
                    d.state = state::REPOSITIONING;
                }
                if hull_full(u, id) || far {
                    d.state = state::RETURNING;
                }
            }
            state::REPOSITIONING => {
                if speed_sq <= 50.0 {
                    let r = u.randy.random(RAN_DROIDS) % 1000;
                    d.state = state::REPAIRING;
                    d.timer = st(f64::from(r) * 0.01 + ld(now));
                }
            }
            state::RETURNING => {
                if !done {
                    d.state = state::REPAIRING;
                } else if speed_sq <= 50.0 {
                    let point = next_dock(stat, &mut arr);
                    let valid = point.as_ref().is_none_or(|p| is_valid_predock_pos(p, d.position));
                    if !valid && d.vertex != NONE {
                        if let (Some(p), Some(m)) = (point.as_ref(), mesh) {
                            let mut dir = Vector::new(p.position.x - d.position.x, p.position.y - d.position.y, p.position.z - d.position.z);
                            dir.normalize();
                            let before = d.vertex;
                            pick_next_edge(m, dir, &mut d.poly, &mut d.vertex);
                            if d.prev_vertex == d.vertex {
                                let k = ld(p.max_dist);
                                d.dest = Vector::new(
                                    st(ld(st(k * ld(p.cone_normal.x))) + ld(p.position.x)),
                                    st(ld(p.position.y) + ld(st(k * ld(p.cone_normal.y)))),
                                    st(ld(p.position.z) + ld(st(k * ld(p.cone_normal.z)))),
                                );
                                d.vertex = NONE;
                            }
                            d.prev_vertex = before;
                            arr.droids[i] = d;
                            continue;
                        }
                        d.dest = Vector::ZERO;
                    } else {
                        d.dest = point.map_or(Vector::ZERO, |p| p.position);
                    }
                    d.vertex = NONE;
                    d.poly = NONE;
                    d.state = state::DOCKING;
                }
            }
            state::DOCKING => {
                d.vertex = NONE;
                d.poly = NONE;
                if speed_sq <= 0.1 {
                    d.state = state::DOCKED;
                }
            }
            _ => {}
        }
        arr.droids[i] = d;
    }
    let s = u.ships.ship_mut(id);
    arr.last_health = s.health;
    s.droids = Some(arr);
}

/// The droid update of `univShipRegenerateHealth` (with the "repair bots" option): the ship's
/// array (created on first use, as `univCreateShip` would have) moves on. The simulation has
/// neither the hull mesh nor a level of detail, so the droids stay off the hull.
pub fn update_for_ship(u: &mut Universe, id: ShipId) {
    if u.ships.get(id).is_some_and(|s| s.droids.is_none()) {
        create_for_ship(u, id);
    }
    update_array(u, id, None, false);
}

/// `droidDestroyArray` (0x5bdc50).
pub fn destroy_array(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.droids = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Square;

    impl HullMesh for Square {
        fn num_vertices(&self) -> usize {
            4
        }
        fn vertex(&self, i: usize) -> (Vector, usize) {
            let p = [Vector::new(0.0, 0.0, 0.0), Vector::new(10.0, 0.0, 0.0), Vector::new(10.0, 10.0, 0.0), Vector::new(0.0, 10.0, 0.0)];
            (p[i], 0)
        }
        fn normal(&self, _i: usize) -> Vector {
            Vector::new(0.0, 0.0, 1.0)
        }
        fn num_polys(&self) -> usize {
            2
        }
        fn poly(&self, i: usize) -> [u16; 3] {
            [[0, 1, 2], [0, 2, 3]][i]
        }
        fn nearest_vertex(&self, _p: Vector) -> usize {
            0
        }
    }

    #[test]
    fn edge_towards_direction() {
        let mut poly = 0;
        let mut vertex = 0;
        pick_next_edge(&Square, Vector::new(0.0, 1.0, 0.0), &mut poly, &mut vertex);
        assert_eq!(vertex, 3);
        assert_eq!(poly, 1);
    }

    #[test]
    fn predock_cone() {
        let p = crate::statics::DockStaticPoint { position: Vector::new(0.0, 0.0, 0.0), cone_normal: Vector::new(-1.0, 0.0, 0.0), cone_angle: 1.0, ..Default::default() };
        assert!(is_valid_predock_pos(&p, Vector::new(10.0, 0.0, 0.0)));
        assert!(!is_valid_predock_pos(&p, Vector::new(-10.0, 0.0, 0.0)));
    }
}
