//! `Dock.c`: docking with and launching from other ships, Cataclysm's way.
//!
//! Every dock point of a ship has one of 35 types (`DPT_*`). [`dock_point_type`] maps a docking
//! ship (class, type, external-build flag, capability flags), the purpose of the dock and the
//! ship docked with to the type of point it uses; the host keeps a per-type count of points in
//! use and a reservation flag per point (`ship+0x4a8`). The state machine `ShipDocksAtShip`
//! ([`ship_docks_at_ship`]) flies the ship to the point's cone, inside or onto a latch, and
//! back out when it launches. Purposes: 0 dock (refuel/repair, and wait inside), 1 dock and
//! stay, 2 build/launch, 3 deposit resources, 4 leech, 5 tow, 7 retire, 8/9 instant versions
//! of 0/1, 10 slipgate.

use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{ld, st, Ext};
use hwc_math::{Matrix, Vector};

use crate::aiship::{self, fly_to_point_avoiding_objs};
use crate::aitrack;
use crate::base::{self, flags2, special, special2, InsideShips};
use crate::command::{self, order};
use crate::object::CommandId;
use crate::object::{sof, Ship, ShipId};
use crate::statics::{DockStaticPoint, ShipClass, ShipRace, ShipStaticInfo, ShipType};
use crate::universe::Universe;

/// Dock purposes (`ShipDocksAtShip`'s third argument, the dock command's +0x290).
pub mod purpose {
    pub const DOCK: i32 = 0;
    pub const DOCK_STAY: i32 = 1;
    pub const LAUNCH: i32 = 2;
    pub const DEPOSIT_RESOURCES: i32 = 3;
    pub const LEECH: i32 = 4;
    pub const TOW: i32 = 5;
    pub const RETIRE: i32 = 7;
    pub const INSTANT: i32 = 8;
    pub const INSTANT_STAY: i32 = 9;
    pub const SLIPGATE: i32 = 10;
}

/// `ShipDocksAtShip` states (`ship+0x49c`).
pub mod state {
    pub const RESERVE: u32 = 0;
    pub const FLY_TO_FRONT: u32 = 1;
    pub const FLY_TO_CONE: u32 = 2;
    pub const FLY_TO_ORIGIN: u32 = 5;
    pub const LATCH_STANDOFF: u32 = 6;
    pub const LATCHING: u32 = 7;
    pub const INSIDE: u32 = 8;
    pub const LATCHED: u32 = 9;
    pub const LAUNCH: u32 = 10;
    pub const UNDER_CONSTRUCTION: u32 = 11;
    pub const FLY_OUT: u32 = 12;
}

/// Dock-point type indices (`DPT_*`, see [`crate::statics::DockPointType`]).
pub mod dpt {
    pub const SMALL_IN: i32 = 0;
    pub const SMALL_OUT: i32 = 1;
    pub const SMALL_IN_ENGINEERING: i32 = 2;
    pub const SMALL_OUT_ENGINEERING: i32 = 3;
    pub const FIGHTER_LATCH: i32 = 4;
    pub const CORVETTE_LATCH: i32 = 5;
    pub const MED_IN: i32 = 6;
    pub const MED_OUT: i32 = 7;
    pub const MED_IN_ENGINEERING: i32 = 8;
    pub const MED_OUT_ENGINEERING: i32 = 9;
    pub const LARGE_IN: i32 = 10;
    pub const LARGE_OUT: i32 = 11;
    pub const LEECH: i32 = 12;
    pub const LEECH_RU_TRANSFER: i32 = 13;
    pub const WORKER_RU_TRANSFER: i32 = 14;
    pub const TOW: i32 = 15;
    pub const DESTROYER_EB: i32 = 18;
    pub const MODULE_EB: i32 = 19;
    pub const SUPPORT_EB: i32 = 20;
    pub const SWARMER_IN: i32 = 21;
    pub const SWARMER_OUT: i32 = 22;
    pub const CRYSTAL_PROCESS: i32 = 23;
    pub const RESOURCER_RU_TRANSFER: i32 = 26;
    pub const SALVAGE: i32 = 27;
    pub const SALVAGE_LATCH: i32 = 28;
    pub const SALVAGE_PROCESS: i32 = 29;
    pub const CLEESAN_BOARDING: i32 = 32;
    pub const MOTHERSHIP_IN: i32 = 33;
    pub const MOTHERSHIP_OUT: i32 = 34;
}

/// Static capability bit of slipgates (`static+0x1ec & 0x4000`).
pub const CAP_SLIPGATE: u32 = 0x4000;

/// What [`dock_point_type`] needs to know about the ship docked with.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DockWith {
    /// The ship has dock points at all.
    pub has_dock: bool,
    pub ship_type: i32,
    /// Its static capability flags (+0x1ec).
    pub static_flags: u32,
    /// First `DPT_Small_In` point (0xff when it has none).
    pub first_small_in: u8,
}

impl DockWith {
    pub fn of(stat: &ShipStaticInfo) -> Self {
        Self {
            has_dock: stat.dock.is_some(),
            ship_type: stat.ship_type.index() as i32,
            static_flags: stat.capability_flags,
            first_small_in: stat.dock.as_ref().map_or(0, |d| d.first_of_type[0]),
        }
    }
}

/// `dockGetDockPointType` (0x45f310): the type of dock point a ship of `class`/`ship_type`
/// (with external-build flag `build` and static capability `flags`) uses on `dw` for
/// `purpose`, or -1. `sp_mission` is the single-player mission (0 outside single player).
pub fn dock_point_type(class: i32, ship_type: i32, build: i32, flags: u32, purpose: i32, dw: &DockWith, sp_mission: i32) -> i32 {
    if !dw.has_dock {
        return -1;
    }
    let sp16 = sp_mission == 0x10;
    if sp16 && ship_type == 0xe && dw.ship_type == 0x8d {
        return 0x20;
    }
    // The shared tail: swarmers use their own points, everything else small ones.
    let tail = |t: i32| if t == 9 || t == 0x2b { 0x15 } else { 0 };
    let deposit = |t: i32| {
        if t != 0x58 {
            if flags & 1 == 0 {
                if flags & 0x200 != 0 {
                    0xd
                } else {
                    -1
                }
            } else {
                0xe
            }
        } else {
            0x1a
        }
    };
    match purpose {
        0 | 8 => match class {
            0..=3 => 10,
            4 => 6,
            5 => {
                if dw.first_small_in == 0xff {
                    5
                } else {
                    0
                }
            }
            6 => {
                if dw.first_small_in == 0xff {
                    4
                } else {
                    0
                }
            }
            7 => match ship_type {
                0xe => {
                    if dw.ship_type == 0x8d && sp16 {
                        0xf
                    } else {
                        6
                    }
                }
                0x31 => 6,
                0x58 => 0x1a,
                _ => 10,
            },
            8 => {
                if ship_type > 0x7d && ship_type < 0x80 {
                    0x17
                } else {
                    -1
                }
            }
            0xb => {
                if ship_type == 3 {
                    0
                } else {
                    tail(ship_type)
                }
            }
            _ => -1,
        },
        1 | 9 => {
            if class != 0xb {
                if class == 8 {
                    6
                } else {
                    0
                }
            } else {
                tail(ship_type)
            }
        }
        2 => {
            if dw.static_flags & CAP_SLIPGATE != 0 {
                return match class {
                    0 => 0x22,
                    4 => 7,
                    5 | 6 | 8 | 10 | 0xb => 1,
                    7 if ship_type != 0x10 && ship_type != 0x33 => 7,
                    _ => 0xb,
                };
            }
            if build != 0 {
                match ship_type {
                    10 | 0xc | 0x10 | 0x19 | 0x2e | 0x2f | 0x33 => return 0x12,
                    0xd | 0x1a | 0x39 => return 0x14,
                    0x13..=0x18 | 0x1f => return 0x13,
                    _ => {}
                }
            }
            match class {
                0..=3 => 0xb,
                4 | 8 => 7,
                5 | 6 => 1,
                7 => {
                    if ship_type == 0x10 || ship_type == 0x33 {
                        0xb
                    } else {
                        7
                    }
                }
                0xb => {
                    if ship_type != 9 && ship_type != 0x2b {
                        1
                    } else {
                        0x16
                    }
                }
                // Classes without a launch point fall through to the deposit case.
                _ => deposit(ship_type),
            }
        }
        3 => deposit(ship_type),
        4 => 0xc,
        5 => 0xf,
        7 => match ship_type {
            0x6f => {
                if dw.ship_type != 0x12 && dw.ship_type != 0x35 {
                    0x1d
                } else {
                    0x12
                }
            }
            0x7a => 6,
            0x7e | 0x7f => 0x17,
            _ => {
                if class > 1 {
                    if class < 4 {
                        return 10;
                    }
                    if class == 7 && (ship_type == 0x10 || ship_type == 0x33) {
                        return 10;
                    }
                }
                6
            }
        },
        10 => {
            if dw.static_flags & CAP_SLIPGATE == 0 {
                return -1;
            }
            match class {
                0 => 0x21,
                4 => 6,
                5 | 6 | 8 | 10 | 0xb => 0,
                7 => {
                    if ship_type == 0x10 || ship_type == 0x33 {
                        10
                    } else {
                        6
                    }
                }
                _ => 10,
            }
        }
        _ => -1,
    }
}

fn sp_mission(u: &Universe) -> i32 {
    if u.single_player {
        u.base.options.mission
    } else {
        0
    }
}

/// [`dock_point_type`] for a ship of `ship_type` docking with `dw` (external-build flag as
/// given, the type's own capability flags).
pub fn point_type_for(u: &Universe, ship_type: ShipType, build: i32, purpose: i32, dw: ShipId) -> i32 {
    let flags = u.statics.ship(ship_type).map_or(0, |s| s.capability_flags);
    point_type_for_flags(u, ship_type, build, flags, purpose, dw)
}

/// [`dock_point_type`] with explicit capability flags.
pub fn point_type_for_flags(u: &Universe, ship_type: ShipType, build: i32, flags: u32, purpose: i32, dw: ShipId) -> i32 {
    let Some(stat) = u.statics.ship(ship_type) else { return -1 };
    let Some(dws) = u.ships.get(dw).and_then(|d| u.statics.ship(d.ship_type)) else { return -1 };
    let class = stat.ship_class.map_or(-1, |c| c.index() as i32);
    dock_point_type(class, ship_type.index() as i32, build, flags, purpose, &DockWith::of(dws), sp_mission(u))
}

/// The dock points of a ship type.
fn points(u: &Universe, t: ShipType) -> Option<&crate::statics::DockStaticInfo> {
    u.statics.ship(t).and_then(|s| s.dock.as_ref())
}

/// The static point `ship.dock_point` of `dw`.
pub fn reserved_point(u: &Universe, ship: ShipId, dw: ShipId) -> Option<&DockStaticPoint> {
    let idx = u.ships.get(ship)?.base.dock_point;
    if idx < 0 {
        return None;
    }
    points(u, u.ships.get(dw)?.ship_type)?.points.get(idx as usize)
}

fn point_type_index(p: &DockStaticPoint) -> Option<usize> {
    p.dock_type.map(|t| t.index())
}

/// Points of type `t` in use on `dw` and how many it has.
fn used_and_count(u: &Universe, dw: ShipId, t: i32) -> (u8, u8) {
    if !(0..35).contains(&t) {
        return (0, 0);
    }
    let s = u.ships.ship(dw);
    let used = s.base.occupancy.as_ref().map_or(0, |o| o.used_per_type[t as usize]);
    let count = points(u, s.ship_type).map_or(0, |d| d.count_of_type[t as usize]);
    (used, count)
}

fn first_of_type(u: &Universe, dw: ShipId, t: i32) -> i32 {
    let s = u.ships.ship(dw);
    points(u, s.ship_type).map_or(-1, |d| d.first_of_type[t as usize] as i8 as i32)
}

/// Marks point `idx` of `dw` reserved by `ship`.
fn reserve_point(u: &mut Universe, ship: ShipId, dw: ShipId, idx: usize) {
    let t = u.ships.get(dw).and_then(|d| points(u, d.ship_type)).and_then(|d| d.points.get(idx)).and_then(point_type_index);
    u.ships.ship_mut(ship).base.dock_point = idx as i32;
    let d = u.ships.ship_mut(dw);
    if let Some(o) = d.base.occupancy.as_mut() {
        if let Some(r) = o.reserved.get_mut(idx) {
            *r = true;
        }
        if let Some(t) = t {
            o.used_per_type[t] = o.used_per_type[t].wrapping_add(1);
        }
    }
}

/// Releases `ship`'s reserved point on `dw` (the inline code repeated through `Dock.c`).
pub fn free_point(u: &mut Universe, ship: ShipId, dw: ShipId) {
    let idx = u.ships.ship(ship).base.dock_point;
    if idx != -1 {
        let t = u.ships.get(dw).and_then(|d| points(u, d.ship_type)).and_then(|d| d.points.get(idx as usize)).and_then(point_type_index);
        if let Some(d) = u.ships.get_mut(dw) {
            if let Some(o) = d.base.occupancy.as_mut() {
                if let Some(r) = o.reserved.get_mut(idx as usize) {
                    *r = false;
                }
                if let Some(t) = t {
                    o.used_per_type[t] = o.used_per_type[t].wrapping_sub(1);
                }
            }
        }
    }
    u.ships.ship_mut(ship).base.dock_point = -1;
}

/// World position of a dock point of `dw` (`matMultiplyMatByVec` then the position added and
/// stored).
pub fn point_world_position(dw: &Ship, local: Vector) -> Vector {
    let m = dw.rotinfo.coordsys.mul_vec(local);
    let p = dw.posinfo.position;
    Vector::new(st(ld(m.x) + ld(p.x)), st(ld(m.y) + ld(p.y)), st(ld(m.z) + ld(p.z)))
}

/// The engineering remap of a mothership without a docking bay (or whose normal points are
/// full): small/medium in/out points become the engineering ones. Returns `None` when the
/// remapped medium point is blocked by traffic on the other medium point.
fn mothership_remap(u: &Universe, dw: ShipId, t: i32) -> Option<i32> {
    let d = u.ships.ship(dw);
    if d.ship_type != ShipType::sMothership && d.ship_type != ShipType::trGhostship {
        return Some(t);
    }
    let (used, count) = used_and_count(u, dw, t);
    let has_bay = base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, d.player);
    if has_bay && used != count {
        return Some(t);
    }
    let o = d.base.occupancy.as_ref();
    let used_of = |k: usize| o.map_or(0, |o| o.used_per_type[k]);
    Some(match t {
        0 => 2,
        1 => 3,
        6 => {
            if used_of(7) != 0 {
                return None;
            }
            8
        }
        7 => {
            if used_of(6) != 0 {
                return None;
            }
            9
        }
        _ => t,
    })
}

/// `dockReserveDockPoint` (0x45fc00): reserve the free point of the right type nearest to the
/// ship and set the cone distance (+0x498) from it.
pub fn reserve_dock_point(u: &mut Universe, ship: ShipId, dw: ShipId, purpose: i32) -> bool {
    let st_ = u.ships.ship(ship).ship_type;
    let mut t = point_type_for(u, st_, 0, purpose, dw);
    if t < 0 {
        return false;
    }
    match mothership_remap(u, dw, t) {
        Some(r) => t = r,
        None => return false,
    }
    if u.ships.ship(dw).ship_type == ShipType::sCarrier && (t == 7 || t == 6) {
        t = 6;
        if u.ships.ship(ship).base.dock_point != -1 {
            return true;
        }
    }
    let (used, count) = used_and_count(u, dw, t);
    if used as i8 as i32 == count as i8 as i32 {
        return false;
    }
    let Some(info) = points(u, u.ships.ship(dw).ship_type) else { return false };
    let first = info.first_of_type[t as usize] as i8 as i32;
    if first < 0 || first as usize >= info.points.len() {
        return false;
    }
    let dws = u.ships.ship(dw);
    let me = u.ships.ship(ship).posinfo.position;
    let reserved = dws.base.occupancy.as_ref().map(|o| o.reserved.clone()).unwrap_or_default();
    let Some((idx, dist)) = nearest_free_point(me, dws.posinfo.position, &dws.rotinfo.coordsys, &info.points, &reserved, first as usize, t as usize) else {
        return false;
    };
    u.ships.ship_mut(ship).base.dock_dist = dist;
    reserve_point(u, ship, dw, idx);
    true
}

/// The search of `dockReserveDockPoint`: among the run of points of type `t` starting at
/// `first`, the free one nearest to `me`, and the cone distance (the distance clamped to the
/// point's minimum and maximum).
pub fn nearest_free_point(me: Vector, host_pos: Vector, host_sys: &Matrix, points: &[DockStaticPoint], reserved: &[bool], first: usize, t: usize) -> Option<(usize, f32)> {
    let mut best = st(1e21);
    let mut best_idx: Option<usize> = None;
    let mut i = first;
    while i < points.len() {
        let p = &points[i];
        if point_type_index(p) != Some(t) {
            break;
        }
        if !reserved.get(i).copied().unwrap_or(false) {
            let m = host_sys.mul_vec(p.position);
            let w = Vector::new(st(ld(m.x) + ld(host_pos.x)), st(ld(m.y) + ld(host_pos.y)), st(ld(m.z) + ld(host_pos.z)));
            let dx = ld(me.x) - ld(w.x);
            let dy = ld(me.y) - ld(w.y);
            let dz = ld(me.z) - ld(w.z);
            let d2 = (dz * dz + dy * dy) + dx * dx;
            if d2 < ld(best) {
                best = st(d2);
                best_idx = Some(i);
            }
        }
        i += 1;
    }
    let idx = best_idx?;
    let p = &points[idx];
    let (min, max) = (p.min_dist, p.max_dist);
    let d: Ext = fmath_sqrt_double(f64::from(best));
    let dist = if d < ld(min) {
        min
    } else if !(d > ld(max)) {
        st(d)
    } else {
        max
    };
    Some((idx, dist))
}

/// `dockReserveBuildLaunchPoint` (0x45ff50): reserve the first free point of the ship's
/// build/launch type and take the cone distance from its minimum.
pub fn reserve_build_launch_point(u: &mut Universe, ship: ShipId, dw: ShipId) -> bool {
    let s = u.ships.ship(ship);
    let build = u.statics.ship(s.ship_type).map_or(0, |st| st.external_build as i32);
    let mut t = point_type_for(u, s.ship_type, build, purpose::LAUNCH, dw);
    let d = u.ships.ship(dw);
    if (d.ship_type == ShipType::sMothership && !base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, d.player))
        || d.ship_type == ShipType::trGhostship
    {
        t = match t {
            0 => 2,
            1 => 3,
            6 => 8,
            7 => 9,
            x => x,
        };
    }
    if d.ship_type == ShipType::sCarrier && (t == 7 || t == 6) {
        t = 6;
        if u.ships.ship(ship).base.dock_point != -1 {
            return true;
        }
    }
    if t < 0 {
        return false;
    }
    let Some(info) = points(u, d.ship_type) else { return false };
    let reserved = d.base.occupancy.as_ref().map(|o| o.reserved.clone()).unwrap_or_default();
    let mut i = info.first_of_type[t as usize] as i8 as i32;
    while i >= 0 && (i as usize) < info.points.len() {
        let p = &info.points[i as usize];
        if point_type_index(p) != Some(t as usize) {
            return false;
        }
        if !reserved.get(i as usize).copied().unwrap_or(false) {
            let min = p.min_dist;
            u.ships.ship_mut(ship).base.dock_dist = min;
            reserve_point(u, ship, dw, i as usize);
            return true;
        }
        i = (i as i8).wrapping_add(1) as i32;
    }
    false
}

/// `dockHasFreeBuildDockPoint` (0x45feb0): `dw` has a free point of the type a new ship of
/// this kind would be built at.
pub fn has_free_build_dock_point_flags(u: &Universe, dw: ShipId, ship_type: ShipType, build: i32, flags: u32) -> bool {
    let mut t = point_type_for_flags(u, ship_type, build, flags, purpose::LAUNCH, dw);
    let d = u.ships.ship(dw);
    let remap = (d.ship_type == ShipType::sMothership && !base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, d.player))
        || d.ship_type == ShipType::trGhostship;
    if remap {
        if t == 1 {
            t = 3;
        } else if t == 7 {
            t = 9;
            let used6 = d.base.occupancy.as_ref().map_or(0, |o| o.used_per_type[6]);
            if used6 != 0 {
                return false;
            }
        }
    }
    if t < 0 {
        return false;
    }
    let (used, count) = used_and_count(u, dw, t);
    used as i8 as i32 != count as i8 as i32
}

/// `dockReserveSpecificDockPoint` (0x45fbb0).
pub fn reserve_specific_dock_point(u: &mut Universe, ship: ShipId, dw: ShipId, idx: usize) {
    reserve_point(u, ship, dw, idx);
}

/// `dockShipLatchesToDockPoint` (0x45f010): the ship clamps onto its point instead of flying
/// inside.
pub fn latches_to_dock_point(u: &Universe, ship: ShipId, purpose: i32) -> bool {
    let s = u.ships.ship(ship);
    if sp_mission(u) == 0x10 && s.ship_type == ShipType::sWorker && s.base.dock_with.and_then(|d| u.ships.get(d)).is_some_and(|d| d.ship_type == ShipType::dEcclesiamInfected) {
        return true;
    }
    if let Some(dw) = s.base.dock_with {
        if let Some(p) = reserved_point(u, ship, dw) {
            let t = p.dock_type.map_or(-1, |t| t.index() as i32);
            if t > 3 && t < 6 {
                return true;
            }
        }
    }
    if purpose == 0 {
        matches!(s.ship_type, ShipType::tResourceCollector | ShipType::dCrystal0 | ShipType::dCrystal1)
    } else {
        purpose > 2 && purpose < 6
    }
}

/// `GetDirectionVectorOfShip` (0x45e7a0): column `dir & 3` of `m`, negated when `dir & 4`.
pub fn direction_vector(dir: i32, m: &Matrix) -> Vector {
    let mut v = match dir & 3 {
        0 => m.col(0),
        1 => m.col(1),
        2 => m.col(2),
        _ => Vector::ZERO,
    };
    if dir & 4 != 0 {
        v = Vector::new(-v.x, -v.y, -v.z);
    }
    v
}

/// `dockPutShipInside` (0x460860).
pub fn put_ship_inside(u: &mut Universe, ship: ShipId, dw: ShipId) {
    let class = base::class_of(u, u.ships.ship(ship).ship_type);
    let d = u.ships.ship_mut(dw);
    let inside = d.base.inside.get_or_insert_with(InsideShips::default);
    if inside.ships.contains(&ship) {
        return;
    }
    inside.ships.push(ship);
    match class {
        Some(ShipClass::Fighter) | Some(ShipClass::Micro) => inside.fighters += 1,
        Some(ShipClass::Corvette) | Some(ShipClass::Resource) => inside.corvettes += 1,
        _ => {}
    }
}

/// `dockRemoveShipFromInside` (0x45e880).
pub fn remove_ship_from_inside(u: &mut Universe, ship: ShipId, dw: ShipId) {
    let class = u.ships.get(ship).and_then(|s| base::class_of(u, s.ship_type));
    let Some(d) = u.ships.get_mut(dw) else { return };
    let Some(inside) = d.base.inside.as_mut() else { return };
    let Some(i) = inside.ships.iter().position(|&s| s == ship) else { return };
    inside.ships.remove(i);
    match class {
        Some(ShipClass::Fighter) | Some(ShipClass::Micro) => inside.fighters -= 1,
        Some(ShipClass::Corvette) | Some(ShipClass::Resource) => inside.corvettes -= 1,
        _ => {}
    }
}

/// `dockPutShipOutside` (0x45e8f0): back into the universe at `position`, oriented by the
/// host's `heading`/`up` direction codes (support units and engineering pairs get flipped).
pub fn put_ship_outside(u: &mut Universe, ship: ShipId, dw: ShipId, position: Vector, heading_dir: i32, up_dir: i32) {
    base::add_ship_to_outside(u, ship, dw);
    let d = u.ships.ship(dw);
    let (dvel, dsys) = (d.posinfo.velocity, d.rotinfo.coordsys);
    let point = u.ships.ship(ship).base.dock_point;
    let first = if point >= 0 {
        reserved_point(u, ship, dw).and_then(|p| p.dock_type).map_or(0, |t| first_of_type(u, dw, t.index() as i32))
    } else {
        0
    };
    let parity = (point as i8).wrapping_sub(first as i8) as u8;
    let s = u.ships.ship_mut(ship);
    s.posinfo.position = position;
    s.posinfo.velocity = dvel;
    let heading = direction_vector(heading_dir, &dsys);
    let up = direction_vector(up_dir, &dsys);
    // heading x up.
    let right = Vector::new(
        st(ld(up.z) * ld(heading.y) - ld(up.y) * ld(heading.z)),
        st(ld(up.x) * ld(heading.z) - ld(heading.x) * ld(up.z)),
        st(ld(heading.x) * ld(up.y) - ld(up.x) * ld(heading.y)),
    );
    s.rotinfo.coordsys = Matrix::from_cols(up, right, heading);
    let pi = std::f64::consts::PI as f32;
    let flip_x = || Matrix::rot_about_x(st(hwc_math::trig::fcos(ld(pi))), st(hwc_math::trig::fsin(ld(pi))));
    match s.ship_type {
        ShipType::sMothershipSupport | ShipType::sCarrierSupport => {
            if parity & 1 != 0 {
                s.rotinfo.coordsys = s.rotinfo.coordsys.mul_mat(&flip_x());
            }
        }
        ShipType::bMothershipSupport => {
            if parity & 2 != 0 {
                let ry = Matrix::rot_about_y(st(hwc_math::trig::fcos(ld(pi))), st(hwc_math::trig::fsin(ld(pi))));
                s.rotinfo.coordsys = s.rotinfo.coordsys.mul_mat(&ry);
                let a: f32 = if parity & 1 == 0 { 0.122_173_05 } else { -0.122_173_05 };
                let rz = Matrix::rot_about_z(st(hwc_math::trig::fcos(ld(a))), st(hwc_math::trig::fsin(ld(a))));
                s.rotinfo.coordsys = s.rotinfo.coordsys.mul_mat(&rz);
            }
            if parity & 1 != 0 {
                s.rotinfo.coordsys = s.rotinfo.coordsys.mul_mat(&flip_x());
            }
        }
        _ => {}
    }
    // Out in the open at its new position, it sees again.
    crate::fow::add_viewer(u, ship);
    if let Some(stat) = u.statics.clone().ship(u.ships.ship(ship).ship_type) {
        crate::physics::update_obj_rot_info(u.ships.ship_mut(ship), stat);
    }
}

fn steady(u: &mut Universe, id: ShipId) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return true };
    aitrack::steady_ship(s, stat, &tw, dt)
}

fn steady_drift_only(u: &mut Universe, id: ShipId) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return true };
    aitrack::steady_ship_drift_only(s, stat, &tw, dt)
}

/// `dockFlyToFrontOfDockwith` (0x460bd0): fly to a point ahead of the host.
fn fly_to_front_of_dockwith(u: &mut Universe, ship: ShipId, dw: ShipId) -> bool {
    let d = u.ships.ship(dw);
    let Some(dstat) = u.statics.ship(d.ship_type) else { return true };
    let mut f = d.rotinfo.coordsys.mul_vec(Vector::new(1.0, 0.0, 0.0));
    let r = dstat.header.coll.coll_sphere_size;
    f = Vector::new(st(ld(r) * ld(f.x)), st(ld(r) * ld(f.y)), st(ld(r) * ld(f.z)));
    if d.ship_type == ShipType::sMothership || d.ship_type == ShipType::trGhostship {
        let k = 2.0f32;
        f = Vector::new(st(ld(k) * ld(f.x)), st(ld(k) * ld(f.y)), st(ld(k) * ld(f.z)));
    }
    let p = d.posinfo.position;
    let target = Vector::new(st(ld(f.x) + ld(p.x)), st(ld(f.y) + ld(p.y)), st(ld(f.z) + ld(p.z)));
    let me = u.ships.ship(ship).posinfo.position;
    let dx = st(ld(target.x) - ld(me.x));
    let dy = st(ld(target.y) - ld(me.y));
    let dz = st(ld(target.z) - ld(me.z));
    let dist = fmath_sqrt_double(f64::from(st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz))));
    if 100.0 <= dist {
        let vel = d.posinfo.velocity;
        fly_to_point_avoiding_objs(u, ship, Some(target), 6, -80.0, Some(vel));
        return false;
    }
    true
}

/// `dockFlyToConeInside` (0x462770): fly onto the point's cone axis; true once inside the
/// cone.
fn fly_to_cone_inside(u: &mut Universe, ship: ShipId, dw: ShipId, p: &DockStaticPoint) -> bool {
    let d = u.ships.ship(dw);
    let w = point_world_position(d, p.position);
    let me = u.ships.ship(ship).posinfo.position;
    let rel = Vector::new(st(ld(me.x) - ld(w.x)), st(ld(me.y) - ld(w.y)), st(ld(me.z) - ld(w.z)));
    let dist = st(fmath_sqrt_double(f64::from(st((ld(rel.x) * ld(rel.x) + ld(rel.y) * ld(rel.y)) + ld(rel.z) * ld(rel.z)))));
    let clamped = if ld(p.min_dist) <= ld(dist) {
        if ld(p.max_dist) < ld(dist) {
            p.max_dist
        } else {
            dist
        }
    } else {
        p.min_dist
    };
    let n = d.rotinfo.coordsys.mul_vec(p.cone_normal);
    let target = Vector::new(
        st(ld(p.min_dist) * ld(n.x) + ld(w.x)),
        st(ld(p.min_dist) * ld(n.y) + ld(w.y)),
        st(ld(p.min_dist) * ld(n.z) + ld(w.z)),
    );
    let dvel = d.posinfo.velocity;
    let dslip = u.statics.ship(d.ship_type).is_some_and(|s| s.capability_flags & CAP_SLIPGATE != 0);
    let dot = (ld(n.x) * ld(rel.x) + ld(n.y) * ld(rel.y)) + ld(n.z) * ld(rel.z) + 5.0;
    if ld(dist) * ld(p.cone_angle) < dot {
        let class = base::class_of(u, u.ships.ship(ship).ship_type);
        let small = matches!(class, Some(ShipClass::Fighter) | Some(ShipClass::Corvette) | Some(ShipClass::Micro));
        if !dslip || !small {
            aitrack::force_steady_ship(u.ships.ship_mut(ship));
        }
        return true;
    }
    if ld(dist) <= ld(clamped) + 500.0 {
        fly_to_point_avoiding_objs(u, ship, Some(target), 3, -10.0, Some(dvel));
    } else {
        fly_to_point_avoiding_objs(u, ship, Some(target), 7, -80.0, Some(dvel));
    }
    false
}

/// `dockFlyToConeOrigin` (0x4600d0): slide down the cone to the point, aligned with the
/// point's heading and up directions.
fn fly_to_cone_origin(u: &mut Universe, ship: ShipId, dw: ShipId, tolerance: f32) -> bool {
    let statics = u.statics.clone();
    let d = u.ships.ship(dw).clone();
    let Some(dstat) = statics.ship(d.ship_type) else { return true };
    let s = u.ships.ship(ship).clone();
    let Some(sstat) = statics.ship(s.ship_type) else { return true };
    let Some(p) = reserved_point(u, ship, dw).cloned() else { return true };
    let class = sstat.ship_class;
    if dstat.capability_flags & CAP_SLIPGATE != 0 && matches!(class, Some(ShipClass::Fighter) | Some(ShipClass::Corvette) | Some(ShipClass::Micro)) {
        let w = point_world_position(&d, p.cone_normal);
        if aitrack::move_reached_destination(&s, w, sstat.header.coll.coll_sphere_size) {
            return true;
        }
        fly_to_point_avoiding_objs(u, ship, Some(w), 7, -100.0, Some(d.posinfo.velocity));
        return false;
    }
    let w = point_world_position(&d, p.position);
    if aitrack::move_reached_destination(&s, w, tolerance) {
        return true;
    }
    let n = d.rotinfo.coordsys.mul_vec(p.cone_normal);
    let mut up = direction_vector(p.up_direction, &d.rotinfo.coordsys);
    let heading = if s.ship_type == ShipType::dEcclesiam {
        let h = Vector::new(-n.x, -n.y, -n.z);
        let a = Vector::new(
            st(ld(up.y) * ld(h.z) - ld(up.z) * ld(h.y)),
            st(ld(up.z) * ld(h.x) - ld(up.x) * ld(h.z)),
            st(ld(up.x) * ld(h.y) - ld(up.y) * ld(h.x)),
        );
        up = Vector::new(
            st(ld(a.z) * ld(h.y) - ld(a.y) * ld(h.z)),
            st(ld(a.x) * ld(h.z) - ld(a.z) * ld(h.x)),
            st(ld(a.y) * ld(h.x) - ld(a.x) * ld(h.y)),
        );
        h
    } else {
        direction_vector(p.heading_direction, &d.rotinfo.coordsys)
    };
    let rel = Vector::new(st(ld(s.posinfo.position.x) - ld(w.x)), st(ld(s.posinfo.position.y) - ld(w.y)), st(ld(s.posinfo.position.z) - ld(w.z)));
    let dist = st(fmath_sqrt_double(f64::from(st((ld(rel.x) * ld(rel.x) + ld(rel.y) * ld(rel.y)) + ld(rel.z) * ld(rel.z)))));
    let inv = st(1.0 / ld(dist));
    let rn = st(ld(inv) * ld(rel.x));
    let cosang = st(ld(n.x) * ld(rn) + ld(n.y) * (ld(inv) * ld(rel.y)) + ld(n.z) * (ld(inv) * ld(rel.z)));
    // Follow the cone as the host turns.
    let prev = s.base.prev_cone_normal;
    let shift = Vector::new(
        st((ld(n.x) - ld(prev.x)) * ld(dist) + ld(s.posinfo.position.x)),
        st(ld(st((ld(n.y) - ld(prev.y)) * ld(dist))) + ld(s.posinfo.position.y)),
        st(ld(st((ld(n.z) - ld(prev.z)) * ld(dist))) + ld(s.posinfo.position.z)),
    );
    base::set_ship_position(u, ship, shift);
    let dt = u.phys_time_elapsed;
    let aligned = {
        let sh = u.ships.ship_mut(ship);
        if sh.ship_type == ShipType::dEcclesiam {
            // The Clee-San goes in once its heading is within 0.995 and its up within 0.78 of
            // the point's (taken before this turn), whatever `aitrackHeadingAndUp` says: its
            // `up` is not unit length, so at a point whose up is not square to the cone it could
            // never reach 0.995 (Mission 3's Kuun-Lan dock, 0.805).
            let (h, su) = (sh.heading(), sh.up());
            let hd = st(ld(h.x) * ld(heading.x) + ld(h.y) * ld(heading.y) + ld(h.z) * ld(heading.z));
            let ud = st(ld(su.x) * ld(up.x) + ld(su.y) * ld(up.y) + ld(su.z) * ld(up.z));
            aitrack::heading_and_up(sh, sstat, dt, heading, up, f32::from_bits(0x3f7e_b852));
            !(ld(hd) < 0.995 || ld(ud) < 0.78)
        } else {
            aitrack::heading_and_up(sh, sstat, dt, heading, up, f32::from_bits(0x3f7e_b852))
        }
    };
    if ld(cosang) < 0.0 {
        return true;
    }
    if ld(cosang) < 0.995 && 100.0 < ld(dist) {
        let along = st((ld(n.x) * ld(rel.x) + ld(n.y) * ld(rel.y)) + ld(n.z) * ld(rel.z));
        let t = Vector::new(st(ld(along) * ld(n.x) + ld(w.x)), st(ld(along) * ld(n.y) + ld(w.y)), st(ld(along) * ld(n.z) + ld(w.z)));
        fly_to_point_avoiding_objs(u, ship, Some(t), 0, -20.0, Some(d.posinfo.velocity));
    }
    if aligned {
        let slip = dstat.capability_flags & CAP_SLIPGATE != 0;
        let (lim, flags) = if slip { (100.0f32, 4u32) } else { (120.0f32, 0u32) };
        fly_to_point_avoiding_objs(u, ship, Some(w), flags, -lim, Some(d.posinfo.velocity));
        if slip {
            let r = sstat.header.coll.coll_sphere_size;
            let tol = if r <= 1000.0 { 1000.0 } else { r };
            if aitrack::move_reached_destination(u.ships.ship(ship), w, tol) {
                return true;
            }
        }
    }
    u.ships.ship_mut(ship).base.prev_cone_normal = n;
    false
}

/// `dockFlyOutOfCone` (0x460670): after launching, fly out along the cone until clear.
fn fly_out_of_cone(u: &mut Universe, ship: ShipId, dw: Option<ShipId>) -> bool {
    if let Some(dw) = dw.filter(|d| u.ships.get(*d).is_some()) {
        let Some(p) = reserved_point(u, ship, dw).cloned() else {
            u.ships.ship_mut(ship).special_flags &= !special::LEAVING_DOCK;
            return true;
        };
        let d = u.ships.ship(dw);
        let fly = p.fly_away_dist;
        let mut n = d.rotinfo.coordsys.mul_vec(p.cone_normal);
        let w = point_world_position(d, p.position);
        let k = st(ld(fly) * 4.0);
        let out = Vector::new(st(ld(k) * ld(n.x) + ld(w.x)), st(ld(k) * ld(n.y) + ld(w.y)), st(ld(k) * ld(n.z) + ld(w.z)));
        let me = u.ships.ship(ship).posinfo.position;
        let rel = Vector::new(st(ld(me.x) - ld(w.x)), st(ld(me.y) - ld(w.y)), st(ld(me.z) - ld(w.z)));
        let along = st(((ld(rel.x) * ld(n.x) + ld(rel.y) * ld(n.y)) + ld(rel.z) * ld(n.z)) / ((ld(n.x) * ld(n.x) + ld(n.y) * ld(n.y)) + ld(n.z) * ld(n.z)));
        n = Vector::new(st(ld(along) * ld(n.x)), st(ld(along) * ld(n.y)), st(ld(along) * ld(n.z)));
        let on_axis = Vector::new(st(ld(n.x) + ld(w.x)), st(ld(n.y) + ld(w.y)), st(ld(n.z) + ld(w.z)));
        base::set_ship_position(u, ship, on_axis);
        // (CarrierDockDoorCheck: the carrier's doors are always open here.)
        if (ld(rel.x) * ld(rel.x) + ld(rel.y) * ld(rel.y)) + ld(rel.z) * ld(rel.z) < ld(fly) * ld(fly) {
            fly_to_point_avoiding_objs(u, ship, Some(out), 0, 0.0, None);
            return false;
        }
    }
    u.ships.ship_mut(ship).special_flags &= !special::LEAVING_DOCK;
    true
}

/// `dockFlyToLatchStandoff` (0x462960): fly onto the cone axis before latching.
fn fly_to_latch_standoff(u: &mut Universe, ship: ShipId, dw: ShipId, p: &DockStaticPoint) -> bool {
    let statics = u.statics.clone();
    let d = u.ships.ship(dw).clone();
    let dslip = statics.ship(d.ship_type).is_some_and(|s| s.capability_flags & CAP_SLIPGATE != 0);
    let sstat = statics.ship(u.ships.ship(ship).ship_type);
    let class = sstat.and_then(|s| s.ship_class);
    if dslip && matches!(class, Some(ShipClass::Fighter) | Some(ShipClass::Corvette) | Some(ShipClass::Micro)) {
        return true;
    }
    let w = point_world_position(&d, p.position);
    let me = u.ships.ship(ship).posinfo.position;
    let rx = st(ld(me.x) - ld(w.x));
    let ry = st(ld(me.y) - ld(w.y));
    let rz = st(ld(me.z) - ld(w.z));
    let mut dist = st(fmath_sqrt_double(f64::from(st((ld(rx) * ld(rx) + ld(ry) * ld(ry)) + ld(rz) * ld(rz)))));
    if ld(p.min_dist) <= ld(dist) {
        if ld(p.max_dist) < ld(dist) {
            dist = p.max_dist;
        }
    } else {
        dist = p.min_dist;
    }
    let n = d.rotinfo.coordsys.mul_vec(p.cone_normal);
    let target = Vector::new(st(ld(n.x) * ld(dist) + ld(w.x)), st(ld(n.y) * ld(dist) + ld(w.y)), st(ld(n.z) * ld(dist) + ld(w.z)));
    let tol = if dslip { st(ld(sstat.map_or(0.0, |s| s.header.coll.coll_sphere_size)) * 0.125 + 25.0) } else { 10.0 };
    if aitrack::move_reached_destination(u.ships.ship(ship), target, tol) {
        return true;
    }
    let (flags, lim) = if dslip { (0xb7u32, 100.0f32) } else { (0xb3u32, 80.0f32) };
    fly_to_point_avoiding_objs(u, ship, Some(target), flags, -lim, Some(d.posinfo.velocity));
    false
}

/// `dockFlyToConeInside` for the salvage code.
pub fn fly_to_cone_inside_pub(u: &mut Universe, ship: ShipId, dw: ShipId, p: &DockStaticPoint) -> bool {
    fly_to_cone_inside(u, ship, dw, p)
}

/// `dockFlyToConeOrigin` for the salvage code.
pub fn fly_to_cone_origin_pub(u: &mut Universe, ship: ShipId, dw: ShipId, tolerance: f32) -> bool {
    fly_to_cone_origin(u, ship, dw, tolerance)
}

/// `dockFlyToLatchStandoff` for the salvage code.
pub fn fly_to_latch_standoff_pub(u: &mut Universe, ship: ShipId, dw: ShipId, p: &DockStaticPoint) -> bool {
    fly_to_latch_standoff(u, ship, dw, p)
}

/// `dockFlyToConeOriginLatch` for the salvage code.
pub fn fly_to_cone_origin_latch_pub(u: &mut Universe, ship: ShipId, own: &DockStaticPoint, dw: ShipId, p: &DockStaticPoint) -> bool {
    fly_to_cone_origin_latch(u, ship, own, dw, p)
}

/// `dockCalcLatchCoordSys` (0x45ec20): the orientation the ship takes when its own link point
/// `own` lies on the host's dock point `p`.
fn latch_coord_sys(own: &DockStaticPoint, host_sys: &Matrix, p: &DockStaticPoint) -> Matrix {
    let pt = Matrix::coord_sys_from_heading(p.cone_normal);
    let h = direction_vector(p.heading_direction, &pt);
    let up = direction_vector(p.up_direction, &pt);
    // heading x up.
    let mut right = Vector::new(
        st(ld(up.z) * ld(h.y) - ld(up.y) * ld(h.z)),
        st(ld(up.x) * ld(h.z) - ld(h.x) * ld(up.z)),
        st(ld(h.x) * ld(up.y) - ld(up.x) * ld(h.y)),
    );
    right.normalize();
    let point_sys = Matrix::from_cols(up, right, h);
    let world = host_sys.mul_mat(&point_sys);
    // The ship's own point: heading = -normal, up from (0,0,1) crossed in.
    let a = Vector::new(-own.cone_normal.x, -own.cone_normal.y, -own.cone_normal.z);
    let mut b = Vector::new(st(0.0 - ld(a.y)), st(ld(a.x) - 0.0), 0.0);
    b.normalize();
    let mut c = Vector::new(
        st(ld(b.z) * ld(a.y) - ld(b.y) * ld(a.z)),
        st(ld(b.x) * ld(a.z) - ld(b.z) * ld(a.x)),
        st(ld(b.y) * ld(a.x) - ld(b.x) * ld(a.y)),
    );
    c.normalize();
    let own_sys = Matrix::from_cols(a, b, c).transpose();
    world.mul_mat(&own_sys)
}

/// `matCoordSysAlignedWithin` (0x45ede0).
fn aligned_within(a: &Matrix, b: &Matrix, threshold: f32) -> bool {
    let d = |i: usize| (ld(b.m[i]) * ld(a.m[i]) + ld(b.m[i + 1]) * ld(a.m[i + 1])) + ld(b.m[i + 2]) * ld(a.m[i + 2]);
    !(d(0) < ld(threshold)) && !(d(6) < ld(threshold)) && !(d(3) < ld(threshold))
}

/// `dockFlyToConeOriginLatch` (0x462b60): bring the ship's own link point onto the dock point
/// and snap to it.
fn fly_to_cone_origin_latch(u: &mut Universe, ship: ShipId, own: &DockStaticPoint, dw: ShipId, p: &DockStaticPoint) -> bool {
    let statics = u.statics.clone();
    let s = u.ships.ship(ship).clone();
    let Some(sstat) = statics.ship(s.ship_type) else { return true };
    let d = u.ships.ship(dw).clone();
    let dslip = statics.ship(d.ship_type).is_some_and(|x| x.capability_flags & CAP_SLIPGATE != 0);
    let avoid = sstat.header.coll.avoid_coll_sphere_size;
    let avoid2 = st(ld(avoid) * ld(avoid));
    let radius = sstat.header.coll.coll_sphere_size;
    let want = latch_coord_sys(own, &d.rotinfo.coordsys, p);
    let w = point_world_position(&d, p.position);
    let off = s.rotinfo.coordsys.mul_vec(own.position);
    let target = Vector::new(st(ld(w.x) - ld(off.x)), st(ld(w.y) - ld(off.y)), st(ld(w.z) - ld(off.z)));
    let me = s.posinfo.position;
    let (close, threshold) = if dslip {
        let t = st(ld(radius) * 0.25);
        let c = !(ld(t) <= (ld(me.x) - ld(target.x)).abs()) && !(ld(t) <= (ld(me.y) - ld(target.y)).abs()) && !(ld(t) <= (ld(me.z) - ld(target.z)).abs());
        (c, f32::from_bits(0x3f7c_28f6))
    } else {
        let c = !(5.0 <= (ld(me.x) - ld(target.x)).abs()) && !(5.0 <= (ld(me.y) - ld(target.y)).abs()) && !(5.0 <= (ld(me.z) - ld(target.z)).abs());
        (c, f32::from_bits(0x3f7e_f9db))
    };
    let oriented = aligned_within(&s.rotinfo.coordsys, &want, threshold);
    let dx = st(ld(target.x) - ld(me.x));
    let dy = st(ld(target.y) - ld(me.y));
    let dz = st(ld(target.z) - ld(me.z));
    let d2 = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
    // Mesh animations finish immediately here, so both sides are always ready.
    let anims_ready = true;
    if oriented && close && anims_ready {
        let sh = u.ships.ship_mut(ship);
        sh.rotinfo.coordsys = want;
        sh.posinfo.velocity = d.posinfo.velocity;
        base::set_ship_position(u, ship, target);
        return true;
    }
    let dt = u.phys_time_elapsed;
    if !oriented {
        let h = direction_vector(2, &want);
        let up = direction_vector(0, &want);
        let accuracy = f32::from_bits(0x3f7f_bce7);
        aitrack::heading_and_up(u.ships.ship_mut(ship), sstat, dt, h, up, accuracy);
        if close {
            return false;
        }
    } else if close {
        return false;
    } else if anims_ready {
        fly_to_point_avoiding_objs(u, ship, Some(target), 0, -120.0, Some(d.posinfo.velocity));
        return false;
    }
    if ld(d2) <= ld(radius) * ld(radius) {
        let tw = u.tweaks.clone();
        aitrack::velocity_vector(u.ships.ship_mut(ship), sstat, &tw, dt, d.posinfo.velocity);
        let _ = avoid2;
        return false;
    }
    fly_to_point_avoiding_objs(u, ship, Some(target), 0, -120.0, Some(d.posinfo.velocity));
    false
}

/// Unloads a resourcer's cargo into its player's stock (the inline copies of
/// `dockAddResourcesToPlayer` in `ShipDocksAtShip`).
fn unload_resources(u: &mut Universe, ship: ShipId, purpose_: i32) {
    if !matches!(purpose_, 0 | 1 | 2 | 3 | 7 | 8 | 9) {
        return;
    }
    let s = u.ships.ship_mut(ship);
    if !matches!(s.ship_type, ShipType::sLeech | ShipType::sWorker | ShipType::bWorker | ShipType::tResourceCollector) {
        return;
    }
    let amount = s.base.resources;
    s.base.resources = 0;
    let player = s.player;
    if amount > 0 {
        base::add_resources_to_player(u, player, amount);
        u.base.events.push(base::BaseEvent::ResourcesDelivered { ship, player, amount });
    }
}

/// The dock timer extras: longer stays to unload and to repair.
fn dock_timer_extras(u: &mut Universe, ship: ShipId) {
    let max = u.ships.get(ship).and_then(|s| u.statics.ship(s.ship_type)).map_or(0.0, |s| s.health.max_health);
    let s = u.ships.ship_mut(ship);
    if 0.0 < s.base.resources as f32 {
        s.base.dock_timer = st(ld(s.base.dock_timer) + 4.0);
    }
    if s.health < max {
        s.base.dock_timer = st(ld(s.base.dock_timer) + 6.0);
    }
}

/// `ShipHasToLaunch` (0x...): whether a ship inside `dw` must leave (other player, no room).
pub fn ship_has_to_launch(u: &Universe, dw: ShipId, ship: ShipId) -> bool {
    let (d, s) = (u.ships.ship(dw), u.ships.ship(ship));
    if u.single_player && u.base.options.mission != 0 && false {
        return false;
    }
    if s.player != d.player {
        return true;
    }
    if matches!(s.ship_type, ShipType::bSwarmer | ShipType::sSwarmer) {
        return false;
    }
    if s.special_flags & special::DOCK_STAY != 0 {
        return false;
    }
    let Some(dstat) = u.statics.ship(d.ship_type) else { return false };
    let class = base::class_of(u, s.ship_type);
    let inside = d.base.inside.as_ref();
    if matches!(class, Some(ShipClass::Fighter) | Some(ShipClass::Micro)) && inside.map_or(0, |i| i.fighters) <= dstat.max_dockable_fighters {
        return false;
    }
    if class != Some(ShipClass::Corvette) && !crate::resource::is_worker(s.ship_type) {
        return true;
    }
    dstat.max_dockable_corvettes < inside.map_or(0, |i| i.corvettes)
}

/// `lmCarrierAutoLaunchEnabled` (0x49b160).
pub fn carrier_auto_launch_enabled(u: &Universe, dw: ShipId) -> bool {
    let d = u.ships.ship(dw);
    let class = base::class_of(u, d.ship_type);
    if u.single_player && (d.player != 0 || d.special_flags2 & special2::DOCK_ANYWHERE != 0) {
        return true;
    }
    if !matches!(class, Some(ShipClass::Mothership) | Some(ShipClass::Carrier)) {
        return true;
    }
    let bit = auto_launch_bit(u, dw);
    if bit == 0 {
        return true;
    }
    u.base.players[d.player].auto_launch & bit != 0
}

/// `lmGetCarrierAutoLaunchBit` (0x49b0c0): 1 for the mothership, 2/4/8 for the carriers.
pub fn auto_launch_bit(u: &Universe, dw: ShipId) -> u32 {
    let d = u.ships.ship(dw);
    if u.single_player && (d.player != 0 || d.special_flags2 & special2::DOCK_ANYWHERE != 0) {
        return 0;
    }
    let class = base::class_of(u, d.ship_type);
    if !matches!(class, Some(ShipClass::Mothership) | Some(ShipClass::Carrier)) {
        return 0;
    }
    if u.base.players[d.player].mothership == Some(dw) {
        return 1;
    }
    let carriers: Vec<ShipId> = u.base.carriers.iter().filter(|(_, p)| *p == d.player).map(|(s, _)| *s).take(3).collect();
    let mut bit = 2;
    for c in carriers {
        if c == dw {
            return bit;
        }
        bit <<= 1;
    }
    0
}

/// `ShipDocksAtShip`'s chatter as a ship gets inside or latches on: it says it docked, or
/// that it is unloading its resources.
fn docked_chatter(u: &mut Universe, ship: ShipId) {
    use crate::battle_chatter::{attempt, bce, can_chatter_at_this_time};
    if can_chatter_at_this_time(u, bce::DOCKING_MORE, Some(ship)) {
        let s = u.ships.ship(ship);
        let k = if s.base.resources as f32 <= 0.0 { bce::DOCKING_MORE } else { bce::RESOURCES_TRANSFERRED };
        let t = s.ship_type.index() as i32;
        attempt(u, k, ship, t);
    }
}

/// `ShipDocksAtShip` (0x460e00): one step of `ship` docking with `dw` for `purpose_`. True when
/// this dock (or launch) is complete.
pub fn ship_docks_at_ship(u: &mut Universe, ship: ShipId, dw: ShipId, cmd: Option<CommandId>, purpose_: i32) -> bool {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let Some(dstat) = u.ships.get(dw).and_then(|d| statics.ship(d.ship_type)) else { return false };
    let dslip = dstat.capability_flags & CAP_SLIPGATE != 0;
    let st_now = u.ships.ship(ship).base.dock_state;
    match st_now {
        state::RESERVE => {
            if purpose_ == purpose::INSTANT || purpose_ == purpose::INSTANT_STAY {
                base::remove_ship_from_outside(u, ship, true);
                put_ship_inside(u, ship, dw);
                if u.ships.ship(dw).ship_type != ShipType::dRKO {
                    crate::ships::mimic::free_disguise(u, ship);
                }
                return true;
            }
            let range_limit = ld(dstat.dock_ship_range) + ld(dstat.header.coll.avoid_coll_sphere_size);
            let range = |u: &Universe| {
                let t = aiship::trajectory(u.ships.ship(ship), u.ships.ship(dw));
                aiship::range_to_target(dstat, t)
            };
            if range_limit <= range(u) || !reserve_dock_point(u, ship, dw, purpose_) {
                if range(u) < range_limit {
                    aitrack::force_steady_ship(u.ships.ship_mut(ship));
                    return false;
                }
                let p = u.ships.ship(dw).posinfo.position;
                fly_to_point_avoiding_objs(u, ship, Some(p), 0x3a, 0.0, None);
                return false;
            }
            // Slipgates remember the ship, so its escorts follow it through.
            if purpose_ == purpose::SLIPGATE && crate::ships::slipgate::is_slipgate_type(u.ships.ship(dw).ship_type) {
                crate::ships::slipgate::add_ship_to_queue(u, dw, ship);
            }
            u.ships.ship_mut(ship).base.dock_state = state::FLY_TO_CONE;
        }
        state::FLY_TO_FRONT => {
            if fly_to_front_of_dockwith(u, ship, dw) {
                u.ships.ship_mut(ship).base.dock_state = state::FLY_TO_CONE;
                return false;
            }
        }
        state::FLY_TO_CONE => {
            let Some(p) = reserved_point(u, ship, dw).cloned() else { return false };
            if crate::ships::slipgate::is_slipgate_type(u.ships.ship(dw).ship_type) {
                crate::ships::slipgate::open(u, dw);
            }
            if fly_to_cone_inside(u, ship, dw, &p) {
                let s = u.ships.ship_mut(ship);
                s.special_flags2 |= special2::DOCKING;
                s.flags |= sof::NO_BUMPS;
                if latches_to_dock_point(u, ship, purpose_) {
                    u.ships.ship_mut(ship).base.dock_state = state::LATCH_STANDOFF;
                    return false;
                }
                let n = u.ships.ship(dw).rotinfo.coordsys.mul_vec(p.cone_normal);
                let s = u.ships.ship_mut(ship);
                s.base.dock_state = state::FLY_TO_ORIGIN;
                s.base.prev_cone_normal = n;
                return false;
            }
        }
        state::FLY_TO_ORIGIN => {
            let gate = crate::ships::slipgate::is_slipgate_type(u.ships.ship(dw).ship_type);
            if gate {
                crate::ships::slipgate::open(u, dw);
            }
            if fly_to_cone_origin(u, ship, dw, 4.0) {
                if gate {
                    // At the open gate the ship waits to go through; its dock point is free.
                    if crate::ships::slipgate::open(u, dw) {
                        u.ships.ship_mut(ship).base.dock_state = 0x10;
                        u.ships.ship_mut(ship).base.slipgate_timer = now;
                        free_point(u, ship, dw);
                    } else {
                        steady(u, ship);
                    }
                    return false;
                }
                let _ = dslip;
                let point_type = reserved_point(u, ship, dw).and_then(|p| p.dock_type).map_or(-1, |t| t.index() as i32);
                base::remove_ship_from_outside(u, ship, true);
                if purpose_ == purpose::RETIRE {
                    // Fleet command reports the retired ship (`STAT_MoShip_ShipRetired`).
                    let (t, player) = (u.ships.ship(ship).ship_type.index() as i32, u.ships.ship(ship).player);
                    if player == u.chatter.local_player {
                        crate::speech::queue(u, None, 0x200_1040, t);
                    }
                    if u.ships.ship(ship).ship_race == ShipRace::Beast {
                        crate::research::acquire_ship_tech_on_dock(u, ship);
                    }
                } else {
                    docked_chatter(u, ship);
                    put_ship_inside(u, ship, dw);
                    let s = u.ships.ship_mut(ship);
                    s.base.dock_state = state::INSIDE;
                    s.base.dock_timer = now;
                    unload_resources(u, ship, purpose_);
                    dock_timer_extras(u, ship);
                    u.base.events.push(base::BaseEvent::ShipDocked { ship, with: dw });
                }
                let keep = u.ships.ship(dw).ship_type == ShipType::sCarrier && (point_type == 6 || point_type == 7);
                if !keep {
                    free_point(u, ship, dw);
                }
                if purpose_ == purpose::RETIRE {
                    set_collect_after(u, cmd, false);
                    return true;
                }
            }
        }
        state::LATCH_STANDOFF => {
            let Some(p) = reserved_point(u, ship, dw).cloned() else { return false };
            if fly_to_latch_standoff(u, ship, dw, &p) {
                let s = u.ships.ship_mut(ship);
                if crate::resource::is_worker(s.ship_type) && s.base.mad_anim != 3 {
                    s.base.mad_anim = 3;
                    s.base.mad_done = true;
                    s.base.mad_paused = true;
                }
                s.base.dock_state = state::LATCHING;
                return false;
            }
        }
        state::LATCHING => {
            let sstat = statics.ship(u.ships.ship(ship).ship_type);
            let own = sstat.and_then(|s| s.dock.as_ref()).and_then(|d| d.center.and_then(|c| d.points.get(c))).cloned();
            let Some(p) = reserved_point(u, ship, dw).cloned() else { return false };
            let own = own.unwrap_or_default();
            if fly_to_cone_origin_latch(u, ship, &own, dw, &p) {
                u.ships.ship_mut(ship).base.dock_timer = now;
                dock_timer_extras(u, ship);
                crate::clamp::clamp_obj_to_obj(u, ship, dw);
                u.ships.ship_mut(ship).base.dock_state = state::LATCHED;
                docked_chatter(u, ship);
                u.base.events.push(base::BaseEvent::ShipDocked { ship, with: dw });
                return false;
            }
        }
        state::INSIDE => {
            if u.ships.ship(ship).base.dock_timer < now {
                let launch = (carrier_auto_launch_enabled(u, dw) || ship_has_to_launch(u, dw, ship))
                    && u.ships.ship(ship).special_flags & special::DOCK_STAY == 0;
                if launch {
                    if purpose_ != purpose::DEPOSIT_RESOURCES {
                        set_collect_after(u, cmd, false);
                    }
                    u.ships.ship_mut(ship).base.dock_state = state::LAUNCH;
                    return false;
                }
                set_collect_after(u, cmd, false);
                return true;
            }
        }
        state::LATCHED => {
            let s = u.ships.ship(ship);
            if s.base.dock_timer < now && s.special_flags & 1 == 0 {
                unload_resources(u, ship, purpose_);
                crate::clamp::unclamp(u, ship);
                u.ships.ship_mut(ship).base.dock_state = state::FLY_OUT;
                return false;
            }
        }
        state::LAUNCH => {
            if u.single_player && u.base.options.mission == 3 && u.ships.ship(ship).ship_type == ShipType::dEcclesiam {
                return true;
            }
            if u.ships.ship(dw).special_flags & special::HYPERSPACING == 0 && reserve_build_launch_point(u, ship, dw) {
                let Some(p) = reserved_point(u, ship, dw).cloned() else { return false };
                let s = u.ships.ship_mut(ship);
                s.special_flags2 |= special2::DOCKING;
                s.flags |= sof::NO_BUMPS;
                let pos = point_world_position(u.ships.ship(dw), p.position);
                remove_ship_from_inside(u, ship, dw);
                put_ship_outside(u, ship, dw, pos, p.heading_direction, p.up_direction);
                u.base.events.push(base::BaseEvent::ShipLaunched { ship, from: dw });
                if dslip {
                    let s = u.ships.ship_mut(ship);
                    s.base.dock_state = 0x11;
                    s.base.slipgate_timer = now;
                    return false;
                }
                if u.ships.ship(ship).special_flags2 & special2::UNDER_CONSTRUCTION == 0 {
                    u.ships.ship_mut(ship).base.dock_state = state::FLY_OUT;
                    let upgrade = u.ships.ship(ship).ship_type;
                    let by_dock = statics.ship(upgrade).is_some_and(|s| s.upgrade_by_docking) || crate::resource::is_worker(upgrade);
                    if by_dock {
                        crate::upgrade::apply_upgrades(u, ship);
                    }
                    crate::ships::acolyte::arm_link_missiles(u, ship);
                    crate::ships::acv::reset_emp_counters(u, ship);
                    let max = statics.ship(upgrade).map_or(0.0, |s| s.health.max_health);
                    let s = u.ships.ship_mut(ship);
                    s.special_flags &= !0x80;
                    s.health = max;
                    return false;
                }
                // External construction: the hull is clamped to its builder.
                u.ships.ship_mut(ship).base.dock_state = state::UNDER_CONSTRUCTION;
                if let Some(b) = u.ships.ship(ship).base.builder {
                    crate::clamp::clamp_obj_to_obj(u, ship, b);
                }
                base::set_disable_reason(u.ships.ship_mut(ship), true, 4);
                crate::construction::external_build_ship_launched(u, ship);
                return false;
            }
        }
        state::UNDER_CONSTRUCTION => {
            if u.ships.ship(ship).special_flags2 & special2::UNDER_CONSTRUCTION == 0 {
                crate::construction::add_ship_support_capacity(u, ship);
                let class = base::class_of(u, u.ships.ship(ship).ship_type);
                if class == Some(ShipClass::Component) {
                    return true;
                }
                crate::clamp::unclamp(u, ship);
                base::set_disable_reason(u.ships.ship_mut(ship), false, 4);
                if u.ships.ship(ship).ship_type == crate::statics::ShipType::sDreadnought {
                    crate::ships::dreadnought::reclamp_cannon(u, ship);
                }
                u.ships.ship_mut(ship).base.dock_state = state::FLY_OUT;
                let caps = statics.ship(u.ships.ship(ship).ship_type).map_or(0, |s| s.capability_flags);
                if caps & 6 != 0 {
                    crate::construction::add_factory(u, ship, caps >> 2 & 1 != 0);
                    return false;
                }
            }
        }
        state::FLY_OUT => {
            if fly_out_of_cone(u, ship, Some(dw)) {
                if latches_to_dock_point(u, ship, purpose_) && crate::resource::is_worker(u.ships.ship(ship).ship_type) {
                    let s = u.ships.ship_mut(ship);
                    if s.base.mad_anim != 2 {
                        s.base.mad_anim = 2;
                        s.base.mad_done = true;
                    }
                }
                return true;
            }
        }
        0x10 | 0x11 => {
            steady_drift_only(u, ship);
            if crate::ships::slipgate::is_slipgate_type(u.ships.ship(dw).ship_type) {
                crate::ships::slipgate::open(u, dw);
            }
            let s = u.ships.ship_mut(ship);
            if st(ld(s.base.slipgate_timer) + 1.5) < now {
                s.base.dock_state = if st_now != 0x10 { 0xe } else { 0xd };
                return false;
            }
        }
        0xd => {
            // Through the gate: the ship is put inside the partner gate and launched from it.
            steady_drift_only(u, ship);
            crate::ships::slipgate::open(u, dw);
            if u.ships.ship(ship).base.dock_timer < now {
                if let Some(partner) = crate::ships::slipgate::partner(u, dw).filter(|&p| u.ships.get(p).is_some()) {
                    base::remove_ship_from_outside(u, ship, true);
                    put_ship_inside(u, ship, partner);
                    let s = u.ships.ship_mut(ship);
                    s.base.dock_with = Some(partner);
                    s.base.dock_state = state::LAUNCH;
                } else {
                    u.ships.ship_mut(ship).base.dock_state = state::FLY_OUT;
                }
                return false;
            }
        }
        0xe => {
            steady_drift_only(u, ship);
            crate::ships::slipgate::open(u, dw);
            if u.ships.ship(ship).base.dock_timer < now {
                u.ships.ship_mut(ship).base.dock_state = state::FLY_OUT;
                return false;
            }
        }
        _ => {}
    }
    false
}

fn set_collect_after(u: &mut Universe, cmd: Option<CommandId>, v: bool) {
    if let Some(c) = cmd.and_then(|c| u.commands.get_mut(c)) {
        c.dock.collect_after = v;
    }
}

/// `ThisShipCanDockWith` (0x45f830).
pub fn ship_can_dock_with(u: &Universe, ship: ShipId, dw: ShipId, purpose_: i32, need_free: bool) -> bool {
    let (Some(s), Some(d)) = (u.ships.get(ship), u.ships.get(dw)) else { return false };
    let t = point_type_for(u, s.ship_type, 0, purpose_, dw);
    if d.ship_type == ShipType::dRKO && !matches!(s.ship_type, ShipType::sMimic | ShipType::sMCV | ShipType::bCruiseMissile) {
        return false;
    }
    if d.ship_type == ShipType::dBeastMothership && s.player == 0 {
        return false;
    }
    let Some(info) = points(u, d.ship_type) else { return false };
    if t == -1 {
        return false;
    }
    if info.first_of_type[t as usize] == 0xff {
        return false;
    }
    if s.ship_type == d.ship_type {
        return false;
    }
    if d.flags & sof::DEAD != 0 || s.flags & sof::DEAD != 0 {
        return false;
    }
    if d.attributes & base::attributes::LATCH_ONLY != 0 && !latches_to_dock_point(u, ship, purpose_) {
        return false;
    }
    let dslip = u.statics.ship(d.ship_type).is_some_and(|x| x.capability_flags & CAP_SLIPGATE != 0);
    if (!u.single_player || (s.player != 1 && s.base.dock_state == 0)) && purpose_ != purpose::SLIPGATE {
        if dslip {
            return false;
        }
        if u.single_player && d.base.disabled {
            return false;
        }
    } else if u.single_player && matches!(u.base.options.mission, 7 | 0xf | 6) && s.player == 0 && s.special_flags2 & special2::DOCK_ANYWHERE == 0 && dslip {
        return false;
    }
    let anywhere = s.special_flags2 & special2::DOCK_ANYWHERE != 0;
    if !anywhere && d.special_flags2 & special2::DOCK_ANYWHERE != 0 {
        return false;
    }
    let ally = u.players.get(s.player).is_some_and(|p| p.is_ally(d.player));
    if !ally && s.attributes & base::attributes::DOCK_WITH_ANYONE == 0 && d.special_flags2 & special2::ALLY_DOCKABLE == 0 {
        // Crystals carried by salvagers may dock anywhere.
        return false;
    }
    if d.special_flags2 & special2::UNDER_CONSTRUCTION != 0 {
        return false;
    }
    if (4..=5).contains(&t) {
        let has_own_link = u.statics.ship(s.ship_type).and_then(|x| x.dock.as_ref()).is_some_and(|x| x.center.is_some());
        if !has_own_link {
            return false;
        }
    }
    if need_free {
        let (used, count) = used_and_count(u, dw, t);
        return (used as i32) < count as i8 as i32;
    }
    true
}

/// `FindNearestShipToDockAt` (0x460cf0): the nearest ship `ship` can dock with for the purpose
/// (for launches, the ship it is inside).
pub fn find_nearest_ship_to_dock_at(u: &Universe, ship: ShipId, purpose_: i32) -> Option<ShipId> {
    if purpose_ == purpose::LAUNCH {
        return u.ship_list.iter().copied().find(|&d| u.ships.get(d).and_then(|x| x.base.inside.as_ref()).is_some_and(|i| i.ships.contains(&ship)));
    }
    let me = u.ships.ship(ship).posinfo.position;
    let mut best = st(1e20);
    let mut found = None;
    for &d in &u.ship_list {
        if !ship_can_dock_with(u, ship, d, purpose_, false) {
            continue;
        }
        let p = u.ships.ship(d).posinfo.position;
        let dx = ld(p.x) - ld(me.x);
        let dy = ld(p.y) - ld(me.y);
        let dz = ld(p.z) - ld(me.z);
        let dist = fmath_sqrt_double((dx * dx + dy * dy) + dz * dz);
        if dist < ld(best) {
            best = st(dist);
            found = Some(d);
        }
    }
    found
}

/// `RemoveShipFromDocking` (0x460900): free the point, clear the docking flags and let go of
/// the host.
pub fn remove_ship_from_docking(u: &mut Universe, ship: ShipId) {
    let Some(s) = u.ships.get(ship) else { return };
    if s.base.dock_point != -1 {
        if let Some(dw) = s.base.dock_with.filter(|d| u.ships.get(*d).is_some()) {
            free_point(u, ship, dw);
        } else {
            u.ships.ship_mut(ship).base.dock_point = -1;
        }
    }
    let s = u.ships.ship_mut(ship);
    s.special_flags &= !special::DOCK_STAY;
    s.flags &= !sof::NO_BUMPS;
    s.special_flags2 &= !special2::DOCKING;
    if crate::clamp::host_of(u, ship).is_some() {
        crate::clamp::unclamp(u, ship);
    }
    let s = u.ships.ship_mut(ship);
    s.base.dock_with = None;
    if crate::resource::is_worker(s.ship_type) && s.base.mad_anim == 3 {
        s.base.mad_anim = 2;
        s.base.mad_done = true;
    }
}

/// `RemoveShipFromLaunching` (0x460b30). It also clears the KAS "still launching" bit
/// (`kasfLaunch`'s 0x1000000, [`special::LEAVING_DOCK`]) as HW1's version did
/// (`SPECIAL_KasCheckDoneLaunching`); Cataclysm dropped that, so a ship whose launch was cut
/// short by a new order kept `TeamFinishedLaunching` false for good. Mission 5's last wave
/// hit it every time: its third heavy corvette waits for one of the Beast carrier's two launch
/// points, the first two go idle, the team's own `IsIdle` -> `Attack` cancels the third's fly
/// out, and the carrier never got its "leave" (it could be killed instead of warping away).
pub fn remove_ship_from_launching(u: &mut Universe, ship: ShipId) {
    let Some(s) = u.ships.get(ship) else { return };
    let class = base::class_of(u, s.ship_type);
    if s.base.dock_point != -1 && class != Some(ShipClass::Component) {
        if let Some(dw) = s.base.dock_with.filter(|d| u.ships.get(*d).is_some()) {
            free_point(u, ship, dw);
        } else {
            u.ships.ship_mut(ship).base.dock_point = -1;
        }
        u.ships.ship_mut(ship).base.dock_with = None;
    }
    let s = u.ships.ship_mut(ship);
    s.special_flags &= !(special::DOCK_STAY | special::LEAVING_DOCK);
    s.flags2 &= !flags2::LAUNCHING;
    s.flags &= !sof::NO_BUMPS;
    s.special_flags2 &= !special2::DOCKING;
}

/// `InitializeDockVars` (0x45f1e0) / the per-ship part of `clDock`.
pub fn initialize_dock_vars(u: &mut Universe, ship: ShipId, dw: ShipId, purpose_: i32) {
    if purpose_ == purpose::DOCK_STAY || purpose_ == purpose::INSTANT_STAY {
        u.ships.ship_mut(ship).special_flags |= special::DOCK_STAY;
        if purpose_ == purpose::INSTANT_STAY && u.ships.get(dw).is_some_and(|d| crate::ships::hive::is_hive_type(d.ship_type)) {
            crate::ships::hive::reset_swarmers(u, dw);
        }
    }
    let s = u.ships.ship_mut(ship);
    s.base.dock_with = Some(dw);
    if purpose_ == purpose::LAUNCH {
        s.flags2 |= flags2::LAUNCHING;
        s.base.dock_state = state::LAUNCH;
    } else {
        s.base.dock_state = state::RESERVE;
    }
    s.base.dock_point = -1;
}

/// `clDock` (0x462210): order `selection` to dock (with `dw`, or each with the nearest ship it
/// can dock with). Returns the number of ships ordered.
pub fn cl_dock(u: &mut Universe, selection: &[ShipId], purpose_: i32, dw: Option<ShipId>) -> usize {
    let mut sel: Vec<ShipId> = selection.iter().copied().filter(|&s| u.ships.get(s).is_some()).collect();
    if sel.is_empty() {
        return 0;
    }
    if let Some(d) = dw {
        let mut i = 0;
        while i < sel.len() {
            if ship_can_dock_with(u, sel[i], d, purpose_, false) {
                i += 1;
            } else {
                // The original moves the last entry into the hole.
                let last = sel.len() - 1;
                sel.swap(i, last);
                sel.pop();
            }
        }
        if sel.is_empty() {
            return 0;
        }
    }
    // The ship types' dock hooks (slot 22) may keep ships out of the order.
    sel.retain(|&s| crate::ships::dock_hook(u, s, dw));
    if sel.is_empty() {
        return 0;
    }
    let existing = command::selection_already_doing_something(u, &sel);
    let mut collect_after = false;
    if let Some(cid) = existing {
        command::clear_protecting_and_passive(u, cid);
        if u.commands.get(cid).is_some_and(|c| c.order != order::COLLECT_RESOURCE) {
            // Reuse the command the selection is already carrying out.
            command::free_last_order_pub(u, cid);
            let c = u.commands.get_mut(cid).expect("command");
            c.order = order::DOCK;
            c.dock = command::DockInfo { purpose: purpose_, collect_after: false, same_dockwith: false };
            return finish_cl_dock(u, cid, &sel, purpose_, dw);
        }
        // Harvesters going to unload return to harvesting afterwards.
        if purpose_ == purpose::DOCK || purpose_ == purpose::DEPOSIT_RESOURCES || purpose_ == purpose::SLIPGATE {
            collect_after = true;
        }
    }
    command::remove_ships_from_doing_stuff(u, &sel);
    let mut c = command::new_command_pub(sel.clone());
    c.order = order::DOCK;
    c.dock = command::DockInfo { purpose: purpose_, collect_after, same_dockwith: false };
    let cid = u.commands.add_pub(c);
    finish_cl_dock(u, cid, &sel, purpose_, dw)
}

fn finish_cl_dock(u: &mut Universe, cid: CommandId, sel: &[ShipId], purpose_: i32, dw: Option<ShipId>) -> usize {
    for &s in sel {
        let target = match dw {
            Some(d) => Some(d),
            None => find_nearest_ship_to_dock_at(u, s, purpose_),
        };
        match target {
            Some(d) => initialize_dock_vars(u, s, d, purpose_),
            None => {
                let sh = u.ships.ship_mut(s);
                sh.base.dock_with = None;
                sh.base.dock_state = state::RESERVE;
                sh.base.dock_point = -1;
            }
        }
    }
    let first = u.ships.ship(sel[0]).base.dock_with;
    let same = sel.iter().all(|&s| u.ships.ship(s).base.dock_with == first);
    if let Some(c) = u.commands.get_mut(cid) {
        c.dock.same_dockwith = same;
    }
    command::init_ships_for_ai(u, sel);
    command::prepare_ships_pub(u, cid);
    sel.len()
}

/// `processDockToDo` (0x461ef0): every ship of a dock command takes its dock step. True when
/// all of them are done.
pub fn process_dock_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let Some(cmd) = u.commands.get(cid).cloned() else { return true };
    let purpose_ = cmd.dock.purpose;
    if cmd.selection.is_empty() {
        return true;
    }
    let mut all_done = true;
    let mut i = 0;
    loop {
        let Some(sel) = u.commands.get(cid).map(|c| c.selection.clone()) else { return all_done };
        if i >= sel.len() {
            return all_done;
        }
        let ship = sel[i];
        let dw = u.ships.get(ship).and_then(|s| s.base.dock_with).filter(|d| u.ships.get(*d).is_some());
        match dw {
            None => {
                if let Some(d) = find_nearest_ship_to_dock_at(u, ship, purpose_) {
                    initialize_dock_vars(u, ship, d, purpose_);
                    command::init_ship_ai_pub(u, ship);
                    all_done = false;
                }
                i += 1;
            }
            Some(d) => {
                if !ship_docks_at_ship(u, ship, d, Some(cid), purpose_) {
                    all_done = false;
                    i += 1;
                    continue;
                }
                let attrs = u.commands.get(cid).map_or(0, |c| c.attributes);
                if attrs & command::attr::FORMATION != 0 {
                    command::remove_ship_from_formation_pub(u, cid, ship);
                }
                if attrs & command::attr::PASSIVE_ATTACKING != 0 {
                    command::remove_ship_from_attacking(u, ship);
                }
                let collect_after = u.commands.get(cid).is_some_and(|c| c.dock.collect_after);
                if collect_after && purpose_ != purpose::RETIRE {
                    i += 1;
                    continue;
                }
                ship_finished_docking(u, ship, d, cid);
            }
        }
    }
}

/// `dockShipFinishedDocking` (0x461d90): the ship leaves the dock command; retiring ships are
/// refunded and removed.
pub fn ship_finished_docking(u: &mut Universe, ship: ShipId, dw: ShipId, cid: CommandId) {
    remove_ship_from_docking(u, ship);
    command::remove_ship_from_command_selection(u, cid, ship);
    u.ships.ship_mut(ship).command = None;
    let purpose_ = u.commands.get(cid).map_or(0, |c| c.dock.purpose);
    if purpose_ == purpose::RETIRE {
        let statics = u.statics.clone();
        let s = u.ships.ship(ship);
        let cost = statics.ship(s.ship_type).map_or(0, |x| x.build_cost);
        let health_frac = statics.ship(s.ship_type).map_or(0.0, |x| x.health.one_over_max_health);
        // Refund: build cost scaled by the health left (`__ftol`), plus the cargo.
        let refund = hwc_math::fpu::ftol_i32(ld(cost as f32) * (ld(s.health) * ld(health_frac))) + s.base.resources;
        let player = u.ships.ship(dw).player;
        u.base.players[player].resource_units += refund;
        let stp = &mut u.base.stats;
        if stp.players.len() <= player {
            stp.players.resize(player + 1, base::PlayerStats::default());
        }
        stp.players[player].ru_other += refund;
        stp.players[player].ru_total += refund;
        base::wipe_inside_ship(u, ship);
        return;
    }
    let s = u.ships.ship(ship);
    let collect_after = u.commands.get(cid).is_some_and(|c| c.dock.collect_after);
    if s.base.dock_state == state::FLY_OUT && !collect_after && !matches!(s.ship_type, ShipType::sSwarmer | ShipType::bSwarmer) {
        // (GroupShip: join the host's military parade, not modelled.)
    }
}

/// `clLaunchShip` (0x445cc0): a launch command for one ship inside `from`, put at the front of
/// the command list.
pub fn cl_launch_ship(u: &mut Universe, ship: ShipId, from: ShipId) {
    let Some(s) = u.ships.get(ship) else { return };
    let ext = u.statics.ship(s.ship_type).is_some_and(|x| x.external_build);
    let latch_only = !ext && u.ships.get(from).is_some_and(|f| f.attributes & base::attributes::LATCH_ONLY != 0);
    if latch_only || s.special_flags & 0x0200_0000 != 0 {
        return;
    }
    // (The +0x244 list-node check: the ship must not be in the universe lists.)
    if u.ship_list.contains(&ship) {
        return;
    }
    command::remove_ships_from_doing_stuff(u, &[ship]);
    let mut c = command::new_command_pub(vec![ship]);
    c.order = order::LAUNCH_SHIP;
    c.launch_from = Some(from);
    initialize_dock_vars(u, ship, from, purpose::LAUNCH);
    let sh = u.ships.ship_mut(ship);
    sh.ai_state = 0;
    sh.ai_state_attack = 0;
    sh.ai_state_command = 0;
    let cid = u.commands.add_front_pub(c);
    let sh = u.ships.ship_mut(ship);
    sh.command = Some(cid);
    crate::aiship::row_remove_ship(sh);
}

/// `clLaunchMultipleShips` (0x445e40).
pub fn cl_launch_multiple_ships(u: &mut Universe, ships: &[ShipId], from: ShipId) {
    for &s in ships {
        if u.ships.get(s).is_some_and(|x| x.attributes & base::attributes::DONT_LAUNCH == 0) {
            cl_launch_ship(u, s, from);
        }
    }
}

/// `LaunchAllInternalShipsOfPlayer` for one auto-launch bit: launch everything inside the
/// player's ship with that bit. Returns how many were launched.
fn launch_all_internal_ships(u: &mut Universe, player: usize, bit: u32) -> usize {
    let hosts: Vec<ShipId> = u.ship_list.iter().copied().filter(|&d| u.ships.get(d).is_some_and(|x| x.player == player) && auto_launch_bit(u, d) == bit).collect();
    let mut n = 0;
    for h in hosts {
        let inside: Vec<ShipId> = u.ships.ship(h).base.inside.as_ref().map(|i| i.ships.clone()).unwrap_or_default();
        for s in inside {
            if u.ships.get(s).is_some_and(|x| x.command.is_none()) {
                cl_launch_ship(u, s, h);
                n += 1;
            }
        }
    }
    n
}

/// `clAutoLaunch` (0x4460e0): set the player's auto-launch bits and launch from every host
/// whose bit is on.
pub fn cl_auto_launch(u: &mut Universe, bits: u32, player: usize) {
    let mut bit = 1u32;
    for _ in 0..5 {
        let cur = u.base.players[player].auto_launch;
        if cur & bit != bits & bit {
            if bits & bit == 0 {
                u.base.players[player].auto_launch = !bit & cur;
            } else {
                u.base.players[player].auto_launch = cur | bit;
            }
        }
        if u.base.players[player].auto_launch & bit != 0 {
            launch_all_internal_ships(u, player, bit);
        }
        bit <<= 1;
    }
}

/// `processLaunchShipToDo` (0x4620a0): one launch step; frees the launch point when the ship is
/// out.
pub fn process_launch_ship_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let Some(cmd) = u.commands.get(cid).cloned() else { return true };
    let Some(&ship) = cmd.selection.first() else { return true };
    let Some(from) = cmd.launch_from.filter(|f| u.ships.get(*f).is_some()) else { return true };
    let st_ = u.ships.ship(ship).ship_type;
    // Launching the engineering, special, armour or weapons module breaks away the ore can.
    if matches!(st_, ShipType::sMothershipArmour | ShipType::sMothershipWeapons | ShipType::sMothershipSpecial | ShipType::sMothershipMicro) {
        for sl in crate::clamp::slaves_of(u, from) {
            if u.ships.get(sl).is_some_and(|x| x.ship_type == ShipType::sMothershipOreCanArmour) {
                crate::modules::break_away(u, sl, true, 7.0, 10.0);
            }
        }
    }
    if ship_docks_at_ship(u, ship, from, Some(cid), purpose::LAUNCH) {
        let s = u.ships.ship(ship);
        let class = base::class_of(u, s.ship_type);
        if s.base.dock_point != -1 && class != Some(ShipClass::Component) {
            if let Some(dw) = s.base.dock_with {
                free_point(u, ship, dw);
            }
            u.ships.ship_mut(ship).base.dock_with = None;
        }
        let s = u.ships.ship_mut(ship);
        s.special_flags &= !special::DOCK_STAY;
        s.special_flags2 &= !special2::DOCKING;
        s.flags2 &= !flags2::LAUNCHING;
        s.flags &= !sof::NO_BUMPS;
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `dockGetDockPointType` results from emulating 0x45f310 in cataclysm.exe over every class,
    /// purpose and a set of ship types, dock hosts and flags (see `docs/sim/runtime.md`).
    #[test]
    fn dock_point_type_matches_emulated_binary() {
        for &(class, ty, build, flags, purpose_, dw_type, dw_flags, first_small_in, mission, expected) in crate::dock_rows::DOCK_POINT_TYPE_ROWS {
            let dw = DockWith { has_dock: true, ship_type: dw_type, static_flags: dw_flags, first_small_in };
            let got = dock_point_type(class, ty, build, flags, purpose_, &dw, mission);
            assert_eq!(got, expected, "class {class} type {ty:#x} build {build} flags {flags:#x} purpose {purpose_} dw {dw_type:#x}/{dw_flags:#x}/{first_small_in} mission {mission}");
        }
    }

    /// The nearest free point and its cone distance against `dockReserveDockPoint` (0x45fc00)
    /// emulated in cataclysm.exe.
    #[test]
    fn reserve_dock_point_matches_emulated_binary() {
        for (i, &(me, host, m, pts, reserved, result, idx, dist)) in crate::dock_rows::RESERVE_ROWS.iter().enumerate() {
            let points: Vec<DockStaticPoint> = pts
                .iter()
                .map(|&(t, p, min, max)| DockStaticPoint {
                    dock_type: crate::statics::DockPointType::from_index(t as usize),
                    position: p,
                    min_dist: min,
                    max_dist: max,
                    ..Default::default()
                })
                .collect();
            let first = points.iter().position(|p| p.dock_type.map(|d| d.index()) == Some(0)).unwrap();
            let got = nearest_free_point(me, host, &Matrix { m }, &points, reserved, first, 0);
            assert_eq!(result, 1, "row {i}");
            let (gi, gd) = got.expect("a point");
            assert_eq!((gi as i32, gd.to_bits()), (idx, dist), "row {i}");
        }
    }
}
