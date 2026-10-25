//! The minelayer corvette and mine walls (`MinelayerCorvette` module, 0x5b4cc0..0x5b5d80):
//! tMinelayerCorvette.
//!
//! In an attack the minelayer makes passes over its target, dropping mines while it is within
//! the drop range and veering off to come round again (`MineLayerAttackRun`); next to a
//! mothership-class target it stops and drops a wall instead. Its special ("force drop") stops
//! the ship and lays a square wall of `NumMinesInSide`² forced mines, spiralling out from a
//! point behind it: each forced mine first clears the layer, then flies to its wall point
//! (`missileShoot` / `aishipGuideMine`). The mines of a wall form a mine formation; once the
//! wall is complete, mines that wander more than 4 km from their wall point leave it.
//! Spent mines regenerate one at a time (`mineRegenerateTime`).

use hwc_math::fpu::{ld, st, Ext};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::aiship::flags as fly;
use crate::object::{MissileId, ShipId, ISMOVING_MOVING};
use crate::statics::{GunType, ShipClass, ShipStaticInfo};
use crate::universe::Universe;

use super::ShipSpec;

/// `INTERCEPTORBREAK_MINVELOCITY` (0x74372c).
const BREAK_MIN_VELOCITY: f32 = -200.0;
/// Squared distances to a mothership-class target to start and to give up dropping a wall
/// there (set by `MinelayerCorvetteInit`, 6 km and 8 km).
const MOTHERSHIP_DIST_SQ: f32 = 36_000_000.0;
const MOTHERSHIP_DIST_SQ2: f32 = 64_000_000.0;
/// Squared distance a mine may stray from its wall point before it leaves the formation.
const MINE_GONE_TOO_FAR_SQ: f32 = 16_000_000.0;
/// Heading accuracy before dropping next to a mothership (0x3f7851ec).
const DROP_HEADING_ACCURACY: f32 = f32::from_bits(0x3f78_51ec);
/// `DEG_TO_RAD` as stored (0x7204f8).
const DEG_TO_RAD: f32 = f32::from_bits(0x3c8e_fa35);
/// Wait time a new formation gets (29 s; read by the wall effect only).
const FORMATION_WAIT_TIME: f32 = 29.0;

/// Attack-run states (`ship->aistateattack`).
pub mod run {
    pub const ATTACK_INIT: u8 = 0;
    pub const APPROACH: u8 = 1;
    pub const BREAK1: u8 = 2;
    pub const BREAK_POSITION: u8 = 3;
    pub const KILL: u8 = 4;
    pub const BREAK2: u8 = 5;
    pub const DROP_MOTHERSHIP: u8 = 7;
}

/// Wall-drop states (+0x640).
pub mod mining {
    pub const FIRST_OFF: i32 = 99;
    pub const FIRST_OFF2: i32 = 100;
    pub const DROP_RIGHT: i32 = 101;
    pub const DROP_UP: i32 = 102;
    pub const DROP_DOWN: i32 = 103;
    pub const DROP_LEFT: i32 = 104;
    pub const DONE_WAIT: i32 = 105;
}

/// What the next mine is (+0x658): an attack mine or a wall mine.
pub mod mine_ai {
    pub const DROP_ATTACK: i32 = 0;
    pub const DROP_FORMATION: i32 = 1;
}

/// Per-ship state (+0x638..+0x6c0; the side-step state at +0x638 is the generic
/// [`crate::object::Ship::side_step`], unused by the minelayer's own attack).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MinelayerSpec {
    /// +0x640: see [`mining`].
    pub mining_status: i32,
    /// +0x648: the point an attack pass flies to.
    pub ai_vec: Vector,
    /// +0x654: time spent in the drop pass.
    pub ai_sphere_time: f32,
    /// +0x658: see [`mine_ai`].
    pub mine_ai_state: i32,
    /// +0x65c: the wall point of the next mine.
    pub formation_position: Vector,
    /// +0x668..+0x6a0: the wall's steps (ship up, right and their opposites, times the mine
    /// spacing) and the offset of its first point (behind the ship).
    pub formation_up: Vector,
    pub formation_right: Vector,
    pub formation_left: Vector,
    pub formation_down: Vector,
    pub formation_heading: Vector,
    /// +0x6a4: mines dropped into the wall so far.
    pub mine_drop_number: i32,
    /// +0x6a8 / +0x6ac: the spiral's side length and the progress along the side.
    pub mining_side_max: i32,
    pub mining_side_count: i32,
    /// +0x6b0: the formation being laid.
    pub formation: Option<u32>,
    /// +0x6b4 / +0x6b8: grid coordinates of the next wall point.
    pub formation_x: i32,
    pub formation_y: i32,
    /// +0x6bc: last mine regeneration.
    pub last_regen: f32,
}

/// Per-type statics (`MinelayerCorvetteStaticInit`, the struct at custom statics +0).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MinelayerStatics {
    /// +0xc / +0x64.
    pub num_mines_in_side: i32,
    pub num_mines_in_side_sq: i32,
    /// +0x10 (read by the mines' own statics, `MINE_STOP_FRICTION`).
    pub mine_stop_friction: f32,
    pub mine_spacing: f32,
    pub mine_drop_distance: f32,
    /// +0x1c.
    pub drop_stop_radius_sq: f32,
    pub break_in_away_dist: f32,
    pub drop_range: f32,
    pub drop_stop_range: f32,
    /// +0x2c: per target class (12 classes).
    pub fly_away_dist: [f32; 12],
    pub fly_away_tolerance: f32,
    pub break2_sphereize_freq: f32,
    /// +0x68 / +0x6c.
    pub mine_clear_distance_sq: f32,
    pub mine_clear_distance: f32,
    /// +0x70 / +0x74.
    pub forced_drop_damage_hi: f32,
    pub forced_drop_damage_lo: f32,
    /// +0x78.
    pub gun_refire_time: f32,
    /// +0x7c.
    pub forced_drop_lifetime: f32,
    /// +0x80.
    pub mine_regenerate_time: f32,
}

/// The `FlyAwayDist[...]` keywords and their class columns.
const FLY_AWAY_KEYS: [(&str, ShipClass); 9] = [
    ("FlyAwayDist[CLASS_Mothership]", ShipClass::Mothership),
    ("FlyAwayDist[CLASS_HeavyCruiser]", ShipClass::HeavyCruiser),
    ("FlyAwayDist[CLASS_Carrier]", ShipClass::Carrier),
    ("FlyAwayDist[CLASS_Destroyer]", ShipClass::Destroyer),
    ("FlyAwayDist[CLASS_Frigate]", ShipClass::Frigate),
    ("FlyAwayDist[CLASS_Corvette]", ShipClass::Corvette),
    ("FlyAwayDist[CLASS_Fighter]", ShipClass::Fighter),
    ("FlyAwayDist[CLASS_Resource]", ShipClass::Resource),
    ("FlyAwayDist[CLASS_NonCombat]", ShipClass::NonCombat),
];

impl MinelayerStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        use crate::statics::cscan::{scan_f32, scan_i32};
        let mut s = Self::default();
        for (key, raw) in &stat.custom_entries {
            let f = |v: &mut f32| {
                if let Some(x) = scan_f32(raw) {
                    *v = x;
                }
            };
            match key.as_str() {
                "NumMinesInSide" => {
                    if let Some(x) = scan_i32(raw) {
                        s.num_mines_in_side = x;
                    }
                }
                "MINE_STOP_FRICTION" => f(&mut s.mine_stop_friction),
                "MineSpacing" => f(&mut s.mine_spacing),
                "MineDropDistance" => f(&mut s.mine_drop_distance),
                "breakInAwayDist" => f(&mut s.break_in_away_dist),
                "DropRange" => f(&mut s.drop_range),
                "DropStopRange" => f(&mut s.drop_stop_range),
                "FlyAwayTolerance" => f(&mut s.fly_away_tolerance),
                "Break2SphereizeFreq" => f(&mut s.break2_sphereize_freq),
                "MineClearDistance" => f(&mut s.mine_clear_distance),
                "forced_drop_damage_lo" => f(&mut s.forced_drop_damage_lo),
                "forced_drop_damage_hi" => f(&mut s.forced_drop_damage_hi),
                "forced_drop_lifetime" => f(&mut s.forced_drop_lifetime),
                "gunReFireTime" => f(&mut s.gun_refire_time),
                "mineRegenerateTime" => f(&mut s.mine_regenerate_time),
                k => {
                    if let Some((_, c)) = FLY_AWAY_KEYS.iter().find(|(n, _)| *n == k) {
                        f(&mut s.fly_away_dist[c.index()]);
                    }
                }
            }
        }
        s.drop_stop_radius_sq = st(ld(s.mine_drop_distance) * ld(s.mine_drop_distance));
        s.num_mines_in_side_sq = s.num_mines_in_side.wrapping_mul(s.num_mines_in_side);
        s.mine_clear_distance_sq = st(ld(s.mine_clear_distance) * ld(s.mine_clear_distance));
        s
    }
}

/// A mine formation (a wall of mines laid by a minelayer; `MineFormationInfo`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MineFormation {
    pub id: u32,
    /// The mines of the wall, in the order they were dropped (`MineList`).
    pub mines: Vec<MissileId>,
    /// `playerowner`.
    pub player: usize,
    /// `FULL`: no more mines will be added (the layer finished, died or was given another
    /// order).
    pub full: bool,
    /// `waittime` (the wall effect's pulse delay).
    pub wait_time: f32,
}

fn spec(u: &Universe, id: ShipId) -> Option<&MinelayerSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Minelayer(m) => Some(m),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut MinelayerSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Minelayer(m) => Some(m),
        _ => None,
    }
}

fn statics_of_type(u: &Universe, t: crate::statics::ShipType) -> Option<&MinelayerStatics> {
    match super::type_statics(u, t) {
        super::TypeStatics::Minelayer(s) => Some(s),
        _ => None,
    }
}

fn statics(u: &Universe, id: ShipId) -> MinelayerStatics {
    statics_of_type(u, u.ships.ship(id).ship_type).cloned().unwrap_or_default()
}

/// `MinelayerCorvetteInit` (0x5b4d40).
pub fn init(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.side_step = Default::default();
        s.ai_state_attack = run::ATTACK_INIT;
        s.spec = ShipSpec::Minelayer(MinelayerSpec { mining_status: mining::FIRST_OFF, ..Default::default() });
    }
}

/// Fires launcher `gi` (`missileShoot`; a wall mine when the layer is laying a wall).
fn shoot(u: &mut Universe, id: ShipId, gi: usize, target: Option<ShipId>) {
    crate::gun::shoot_missile(u, id, gi, target);
}

fn loaded(u: &Universe, id: ShipId, gi: usize) -> i32 {
    u.ships.ship(id).guns.get(gi).map_or(0, |g| g.num_missiles)
}

/// `MinelayerCorvetteFire` (0x5b4d90): with two launchers, the one that fired longer ago
/// drops (the other when it is empty).
pub fn fire_at(u: &mut Universe, id: ShipId, target: Option<ShipId>) {
    let n = u.ships.ship(id).guns.len();
    if n == 0 {
        return;
    }
    if n < 2 {
        if loaded(u, id, 0) > 0 && crate::gun::ready(u, id, 0) {
            shoot(u, id, 0, target);
        }
        return;
    }
    let (t0, t1) = {
        let g = &u.ships.ship(id).guns;
        (g[0].last_time_fired, g[1].last_time_fired)
    };
    let (first, second) = if t1 < t0 { (1, 0) } else { (0, 1) };
    if loaded(u, id, first) < 1 {
        if loaded(u, id, second) > 0 && crate::gun::ready(u, id, second) {
            shoot(u, id, second, target);
        }
    } else if crate::gun::ready(u, id, first) {
        shoot(u, id, first, target);
    }
}

/// The fire slot (`CustShipFire`).
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    fire_at(u, id, Some(target));
}

/// The target-class column of the fly-away table.
fn target_class(u: &Universe, target: ShipId) -> usize {
    let c = u.ships.get(target).and_then(|t| u.statics.ship(t.ship_type)).and_then(|s| s.ship_class);
    crate::combat_statics::class_index(c).min(11)
}

/// `SetAIVecHeading` (0x5b4ea0): the point to fly to for the next pass: across the target's
/// bow when approaching from ahead of it, else off to a random side; turned a little more
/// when the target stands still.
fn set_ai_vec_heading(u: &mut Universe, id: ShipId, target: ShipId, traj: Vector) {
    let fa = statics(u, id).fly_away_dist[target_class(u, target)];
    let t = u.ships.ship(target);
    let th = t.heading();
    let (tpos, moving) = (t.posinfo.position, t.posinfo.is_moving & ISMOVING_MOVING != 0);
    let tr = traj;
    let dot = (ld(th.z) * ld(tr.z) + ld(th.x) * ld(tr.x)) + ld(th.y) * ld(tr.y);
    let mut v = spec(u, id).map(|s| s.ai_vec).unwrap_or_default();
    if dot > 0.0 {
        let x = ld(th.x) + ld(v.x);
        let y = st(ld(th.y) + ld(v.y));
        let z = st(ld(th.z) + ld(v.z));
        v = Vector::new(st(x + ld(tr.x)), st(ld(y) + ld(tr.y)), st(ld(z) + ld(tr.z)));
    } else {
        let r = u.randy.random(RAN_GAME);
        let c = if r & 1 == 1 {
            Vector::new(
                st(ld(th.y) * ld(tr.z) - ld(th.z) * ld(tr.y)),
                st(ld(th.z) * ld(tr.x) - ld(th.x) * ld(tr.z)),
                st(ld(th.x) * ld(tr.y) - ld(th.y) * ld(tr.x)),
            )
        } else {
            Vector::new(
                st(ld(th.z) * ld(tr.y) - ld(th.y) * ld(tr.z)),
                st(ld(th.x) * ld(tr.z) - ld(th.z) * ld(tr.x)),
                st(ld(th.y) * ld(tr.x) - ld(th.x) * ld(tr.y)),
            )
        };
        v = Vector::new(st(ld(c.x) + ld(tr.x)), st(ld(c.y) + ld(tr.y)), st(ld(tr.z) + ld(c.z)));
    }
    v = Vector::new(st(ld(fa) * ld(v.x)), st(ld(fa) * ld(v.y)), st(ld(fa) * ld(v.z)));
    if !moving {
        let deg = u.randy.random(RAN_GAME) % 26 + 5;
        let a: Ext = f64::from(deg) * ld(DEG_TO_RAD);
        let (sin, cos) = (st(fsin(a)), st(fcos(a)));
        v = Matrix::rot_about_z(cos, sin).mul_vec(v);
    }
    v = Vector::new(st(ld(tpos.x) + ld(v.x)), st(ld(tpos.y) + ld(v.y)), st(ld(tpos.z) + ld(v.z)));
    if let Some(s) = spec_mut(u, id) {
        s.ai_vec = v;
    }
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(v), fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE, BREAK_MIN_VELOCITY, None);
}

fn trajectory(u: &Universe, id: ShipId, target: ShipId) -> Vector {
    crate::aiship::trajectory(u.ships.ship(id), u.ships.ship(target))
}

fn range(u: &Universe, target: ShipId, traj: Vector) -> Ext {
    let tstat = u.statics.ship(u.ships.ship(target).ship_type).expect("statics");
    crate::aiship::range_to_target(tstat, traj)
}

fn fly_fast(u: &mut Universe, id: ShipId, point: Vector) {
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(point), fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE, BREAK_MIN_VELOCITY, None);
}

fn sq_dist(a: Vector, b: Vector) -> Ext {
    let (x, y, z) = (ld(a.x) - ld(b.x), ld(a.y) - ld(b.y), ld(a.z) - ld(b.z));
    (x * x + y * y) + z * z
}

/// `MinelayerCorvetteAttack` (0x5b5550) / `MineLayerAttackRun` (0x5b5140): approach, break to
/// one side, make a dropping pass across the target and fly off to come round again; next to
/// a mothership-class target, stop and drop a wall.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if spec(u, id).is_none() || u.ships.get(target).is_none() {
        return;
    }
    let ms = statics(u, id);
    let class = target_class(u, target);
    let tpos = u.ships.ship(target).posinfo.position;
    let mut state = u.ships.ship(id).ai_state_attack;
    match state {
        run::ATTACK_INIT | run::APPROACH => {
            let traj = trajectory(u, id, target);
            crate::aiship::fly_to_point_avoiding_objs(u, id, Some(tpos), fly::POINT_IN_DIRECTION_FLYING, 0.0, None);
            let r = range(u, target, traj);
            if class == ShipClass::Mothership.index() {
                let d = sq_dist(u.ships.ship(target).coll.coll_position, u.ships.ship(id).coll.coll_position);
                if d < ld(MOTHERSHIP_DIST_SQ) {
                    u.ships.ship_mut(id).ai_state_attack = run::DROP_MOTHERSHIP;
                }
            } else if r < ld(ms.break_in_away_dist) {
                u.ships.ship_mut(id).ai_state_attack = run::BREAK_POSITION;
                spec_mut(u, id).expect("minelayer").ai_vec = Vector::ZERO;
            }
            return;
        }
        run::BREAK_POSITION => {
            let mut traj = trajectory(u, id, target);
            traj.normalize();
            set_ai_vec_heading(u, id, target, traj);
            u.ships.ship_mut(id).ai_state_attack = run::BREAK1;
            state = run::BREAK1;
        }
        _ => {}
    }
    match state {
        run::BREAK1 => {
            let mut traj = trajectory(u, id, target);
            let r = range(u, target, traj);
            fly_fast(u, id, tpos);
            if st(r) < ms.drop_range {
                traj.normalize();
                set_ai_vec_heading(u, id, target, traj);
                u.ships.ship_mut(id).ai_state_attack = run::KILL;
                spec_mut(u, id).expect("minelayer").ai_sphere_time = 0.0;
            }
        }
        run::KILL => {
            let mut traj = trajectory(u, id, target);
            let r = range(u, target, traj);
            let dt = u.phys_time_elapsed;
            {
                let s = spec_mut(u, id).expect("minelayer");
                s.ai_sphere_time = st(ld(dt) + ld(s.ai_sphere_time));
            }
            if !u.ships.ship(id).guns.is_empty() && crate::gun::ready(u, id, 0) && loaded(u, id, 0) > 0 {
                spec_mut(u, id).expect("minelayer").mine_ai_state = mine_ai::DROP_ATTACK;
                fire_at(u, id, Some(target));
            }
            if ms.drop_stop_range < st(r) {
                u.ships.ship_mut(id).ai_state_attack = run::BREAK2;
            }
            if ms.break2_sphereize_freq < spec(u, id).expect("minelayer").ai_sphere_time {
                let s = spec_mut(u, id).expect("minelayer");
                s.ai_vec = Vector::ZERO;
                s.ai_sphere_time = -1_000_000.0;
                traj.normalize();
                set_ai_vec_heading(u, id, target, traj);
            }
            let v = spec(u, id).expect("minelayer").ai_vec;
            fly_fast(u, id, v);
        }
        run::BREAK2 => {
            let traj = trajectory(u, id, target);
            let r = range(u, target, traj);
            let v = spec(u, id).expect("minelayer").ai_vec;
            fly_fast(u, id, v);
            if ms.fly_away_dist[class] < st(r) || crate::aitrack::move_reached_destination(u.ships.ship(id), v, ms.fly_away_tolerance) {
                u.ships.ship_mut(id).ai_state_attack = run::APPROACH;
            }
        }
        run::DROP_MOTHERSHIP => {
            let a = u.ships.ship(id).coll.coll_position;
            let b = u.ships.ship(target).coll.coll_position;
            let mut d = Vector::new(st(ld(a.x) - ld(b.x)), st(ld(a.y) - ld(b.y)), st(ld(a.z) - ld(b.z)));
            let m = (ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z);
            d.normalize();
            if ld(MOTHERSHIP_DIST_SQ2) < m {
                u.ships.ship_mut(id).ai_state_attack = run::ATTACK_INIT;
            }
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(id);
            let stat = statics.ship(s.ship_type).expect("statics");
            let on = crate::aitrack::heading(s, stat, &tw, dt, &mut d, DROP_HEADING_ACCURACY, DROP_HEADING_ACCURACY, 0.0, crate::aitrack::IGNORE_UP_VEC);
            if on && static_mine_drop(u, id, Some(target)) {
                spec_mut(u, id).expect("minelayer").mining_status = mining::FIRST_OFF;
                close_formation(u, id);
            }
        }
        _ => {}
    }
}

/// The layer lets go of its formation: no more mines will join it.
fn close_formation(u: &mut Universe, id: ShipId) {
    let Some(f) = spec_mut(u, id).and_then(|s| s.formation.take()) else { return };
    if let Some(mf) = u.mine_formations.iter_mut().find(|m| m.id == f) {
        mf.full = true;
    }
}

/// Drops the next wall mine after moving the wall point by `step` and advancing the spiral.
fn drop_wall_mine(u: &mut Universe, id: ShipId, target: Option<ShipId>, step: fn(&MinelayerSpec) -> Vector) {
    {
        let s = spec_mut(u, id).expect("minelayer");
        s.mine_ai_state = mine_ai::DROP_FORMATION;
        let d = step(s);
        let p = s.formation_position;
        s.formation_position = Vector::new(st(ld(d.x) + ld(p.x)), st(ld(d.y) + ld(p.y)), st(ld(d.z) + ld(p.z)));
    }
    fire_at(u, id, target);
}

/// `MinelayerCorvetteStaticMineDrop` (0x5b5580): one step of laying a wall: stop, then drop
/// one mine per call, spiralling right, up, left, down with growing sides. True when the wall
/// is complete (or was already).
pub fn static_mine_drop(u: &mut Universe, id: ShipId, target: Option<ShipId>) -> bool {
    if spec(u, id).is_none() {
        return true;
    }
    let ms = statics(u, id);
    let now = u.total_time_elapsed;
    let guns = u.ships.ship(id).guns.clone();
    let Some(g0) = guns.first().copied() else { return false };
    let recent = |g: &crate::object::Gun| ld(now) - ld(g.last_time_fired) < ld(ms.gun_refire_time);
    if guns.len() < 2 {
        if g0.num_missiles == 0 || recent(&g0) {
            return false;
        }
    } else {
        let g1 = guns[1];
        if (g0.num_missiles == 0 && g1.num_missiles == 0) || (recent(&g1) && recent(&g0)) {
            return false;
        }
    }
    let mut status = spec(u, id).expect("minelayer").mining_status;
    if status == mining::FIRST_OFF {
        // (Battle chatter "force dropping" for the local player.)
        spec_mut(u, id).expect("minelayer").mining_status = mining::FIRST_OFF2;
        status = mining::FIRST_OFF2;
    }
    match status {
        mining::FIRST_OFF2 => {
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(id);
            let stat = statics.ship(s.ship_type).expect("statics");
            let still_rot = crate::aitrack::zero_rotation_anywhere(s, stat, &tw, dt);
            let still = crate::aitrack::zero_velocity(s, stat, &tw, dt);
            if still && still_rot {
                start_wall(u, id, &ms, target);
            }
        }
        mining::DROP_RIGHT => {
            drop_wall_mine(u, id, target, |s| s.formation_right);
            let s = spec_mut(u, id).expect("minelayer");
            s.mine_drop_number += 1;
            s.formation_x += 1;
            s.mining_side_count += 1;
            if s.mining_side_count == s.mining_side_max {
                s.mining_side_count = 0;
                s.mining_status = mining::DROP_UP;
            }
        }
        mining::DROP_UP => {
            drop_wall_mine(u, id, target, |s| s.formation_up);
            let s = spec_mut(u, id).expect("minelayer");
            s.mining_side_count += 1;
            s.formation_y += 1;
            s.mine_drop_number += 1;
            if s.mining_side_count == s.mining_side_max {
                s.mining_side_max += 1;
                s.mining_side_count = 0;
                s.mining_status = mining::DROP_LEFT;
            }
        }
        mining::DROP_DOWN => {
            drop_wall_mine(u, id, target, |s| s.formation_down);
            let s = spec_mut(u, id).expect("minelayer");
            s.mining_side_count += 1;
            s.formation_y -= 1;
            s.mine_drop_number += 1;
            if s.mining_side_count == s.mining_side_max {
                s.mining_side_max += 1;
                s.mining_side_count = 0;
                s.mining_status = mining::DROP_RIGHT;
            }
        }
        mining::DROP_LEFT => {
            drop_wall_mine(u, id, target, |s| s.formation_left);
            let s = spec_mut(u, id).expect("minelayer");
            s.mining_side_count += 1;
            s.mine_drop_number += 1;
            s.formation_x -= 1;
            if s.mining_side_count == s.mining_side_max {
                s.mining_side_count = 0;
                s.mining_status = mining::DROP_DOWN;
            }
        }
        mining::DONE_WAIT => {
            spec_mut(u, id).expect("minelayer").mine_ai_state = mine_ai::DROP_ATTACK;
            return true;
        }
        _ => {}
    }
    if spec(u, id).expect("minelayer").mine_drop_number != ms.num_mines_in_side.wrapping_mul(ms.num_mines_in_side) {
        return false;
    }
    close_formation(u, id);
    let s = spec_mut(u, id).expect("minelayer");
    s.mine_ai_state = mine_ai::DROP_ATTACK;
    s.mine_drop_number = 0;
    s.mining_status = mining::DONE_WAIT;
    true
}

/// The stopped layer sets up the wall (its steps along the ship's up and right axes, the first
/// point behind it), opens a mine formation and drops the first mine.
fn start_wall(u: &mut Universe, id: ShipId, ms: &MinelayerStatics, target: Option<ShipId>) {
    crate::aitrack::force_steady_ship(u.ships.ship_mut(id));
    let (cs, pos, player) = {
        let s = u.ships.ship(id);
        (s.rotinfo.coordsys, s.posinfo.position, s.player)
    };
    let fid = u.mine_formations.iter().map(|f| f.id).max().map_or(1, |m| m.wrapping_add(1));
    u.mine_formations.push(MineFormation { id: fid, mines: Vec::new(), player, full: false, wait_time: FORMATION_WAIT_TIME });
    let scale = |v: Vector, k: Ext| Vector::new(st(k * ld(v.x)), st(k * ld(v.y)), st(k * ld(v.z)));
    let sp = ld(ms.mine_spacing);
    let up = scale(cs.col(0), sp);
    let right = scale(cs.col(1), sp);
    let heading = scale(cs.col(2), -ld(ms.mine_drop_distance));
    let s = spec_mut(u, id).expect("minelayer");
    s.mining_status = mining::DROP_RIGHT;
    s.mining_side_max = 1;
    s.mining_side_count = 0;
    s.formation_up = up;
    s.formation_down = Vector::new(-up.x, -up.y, -up.z);
    s.formation_right = right;
    s.formation_left = Vector::new(-right.x, -right.y, -right.z);
    s.formation_heading = heading;
    s.formation = Some(fid);
    s.formation_x = 0;
    s.formation_y = 0;
    s.formation_position = Vector::new(st(ld(pos.x) + ld(heading.x)), st(ld(pos.y) + ld(heading.y)), st(ld(pos.z) + ld(heading.z)));
    s.mine_ai_state = mine_ai::DROP_FORMATION;
    fire_at(u, id, target);
    spec_mut(u, id).expect("minelayer").mine_drop_number = 1;
}

/// `MinelayerCorvetteSpecialActivate` (0x5b5b80): the force drop (one wall step per update).
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    static_mine_drop(u, id, None)
}

/// `MinelayerCorvetteOrderChangedCleanUp` (0x5b5d80).
pub fn order_changed(u: &mut Universe, id: ShipId) {
    if let Some(s) = spec_mut(u, id) {
        s.mining_status = mining::FIRST_OFF;
    }
    close_formation(u, id);
}

/// `MinelayerCorvetteHousekeep` (0x5b5bb0): every `mineRegenerateTime` the mine launcher with
/// the fewest mines (below its maximum) gets one back.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(last) = spec(u, id).map(|s| s.last_regen) else { return };
    let ms = statics(u, id);
    let now = u.total_time_elapsed;
    if !(ld(ms.mine_regenerate_time) < ld(now) - ld(last)) {
        return;
    }
    spec_mut(u, id).expect("minelayer").last_regen = now;
    regenerate_one(u, id, GunType::MineLauncher);
}

/// The launcher of `kind` with the fewest missiles below its maximum gets one more (the
/// regeneration loop of the minelayer and missile-ship housekeeping).
pub(crate) fn regenerate_one(u: &mut Universe, id: ShipId, kind: GunType) {
    let statics = u.statics.clone();
    let s = u.ships.ship_mut(id);
    let Some(gstats) = statics.ship(s.ship_type).and_then(|st| st.guns.as_ref()) else { return };
    let mut best: Option<usize> = None;
    let mut least = 100_000;
    for (i, (g, gs)) in s.guns.iter().zip(gstats).enumerate() {
        if gs.gun_type == Some(kind) && g.num_missiles < gs.max_missiles && g.num_missiles < least {
            least = g.num_missiles;
            best = Some(i);
        }
    }
    if let Some(i) = best {
        s.guns[i].num_missiles += 1;
    }
}

/// `MineLayerCorvetteDied` (0x5b5b90).
pub fn died(u: &mut Universe, id: ShipId) {
    close_formation(u, id);
}

/// The squared distance a forced mine must get from its layer before it flies to its wall
/// point (the layer type's custom statics +0x68, `MineClearDistance`²).
pub fn mine_clear_distance_sq(u: &Universe, layer: crate::statics::ShipType) -> f32 {
    statics_of_type(u, layer).map_or(0.0, |s| s.mine_clear_distance_sq)
}

/// A mine joined a formation (`listAddNode` in `missileShoot`).
pub fn mine_added(u: &mut Universe, formation: u32, mine: MissileId) {
    if let Some(f) = u.mine_formations.iter_mut().find(|f| f.id == formation) {
        f.mines.push(mine);
    }
}

/// A mine of a mine formation died (`listRemoveNode` of the mine's formation link).
pub fn mine_died(u: &mut Universe, formation: u32, mine: MissileId) {
    if let Some(f) = u.mine_formations.iter_mut().find(|f| f.id == formation) {
        f.mines.retain(|&m| m != mine);
    }
}

/// The forced-drop data `missileShoot` takes from a layer laying a wall (+0x658 == 1).
pub fn forced_drop(u: &Universe, layer: ShipId) -> Option<crate::missile::ForcedMine> {
    let s = spec(u, layer)?;
    if s.mine_ai_state != mine_ai::DROP_FORMATION {
        return None;
    }
    let formation = s.formation?;
    let ms = statics_of_type(u, u.ships.ship(layer).ship_type)?;
    Some(crate::missile::ForcedMine {
        formation,
        wall_point: s.formation_position,
        extra: [s.formation_x as f32, s.formation_y as f32],
        damage_lo: ms.forced_drop_damage_lo,
        damage_hi: ms.forced_drop_damage_hi,
        lifetime: ms.forced_drop_lifetime,
    })
}

/// `univUpdateMineWallFormations` (0x5b5c50), once per update after the command layer: a
/// finished formation down to nine mines dissolves (its mines go free); in other finished
/// formations, mines more than 4 km from their wall point leave it.
pub fn update_mine_wall_formations(u: &mut Universe) {
    let mut i = 0;
    while i < u.mine_formations.len() {
        let f = &u.mine_formations[i];
        if !f.full {
            i += 1;
            continue;
        }
        if f.mines.len() == 9 {
            let f = u.mine_formations.remove(i);
            for m in f.mines {
                if let Some(m) = u.missiles.get_mut(m) {
                    m.formation = None;
                }
            }
            continue;
        }
        let mines = f.mines.clone();
        let mut keep = Vec::with_capacity(mines.len());
        for mid in mines {
            let gone = u.missiles.get(mid).is_some_and(|m| ld(MINE_GONE_TOO_FAR_SQ) < sq_dist(m.posinfo.position, m.wall_point));
            if gone {
                if let Some(m) = u.missiles.get_mut(mid) {
                    m.formation = None;
                }
            } else {
                keep.push(mid);
            }
        }
        u.mine_formations[i].mines = keep;
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statics_parse() {
        let mut s = ShipStaticInfo::new(crate::statics::ShipType::tMinelayerCorvette, String::new());
        for (k, v) in [
            ("NumMinesInSide", "3"),
            ("MineDropDistance", "400.0"),
            ("MineClearDistance", "100.0"),
            ("FlyAwayDist[CLASS_Frigate]", "1200.0"),
            ("FlyAwayDist[CLASS_NonCombat]", "800.0"),
        ] {
            s.custom_entries.push((k.to_owned(), v.to_owned()));
        }
        let m = MinelayerStatics::parse(&s);
        assert_eq!(m.num_mines_in_side_sq, 9);
        assert_eq!(m.drop_stop_radius_sq, 160_000.0);
        assert_eq!(m.mine_clear_distance_sq, 10_000.0);
        assert_eq!(m.fly_away_dist[ShipClass::Frigate.index()], 1200.0);
        assert_eq!(m.fly_away_dist[8], 800.0);
    }
}
