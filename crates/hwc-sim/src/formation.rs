//! `Formation.c`: formation slots (`FormationCalculateOffsets`), choosing which ship takes
//! which slot (`formationArrageCrazyOptimum`), keeping wingmen on their slots while the leader
//! flies (`processFormationToDo`) and the formation's travel speed.
//!
//! Implemented formation types: delta, broad, delta 3D ("Formation X"), claw, wall and custom.
//! Sphere (5) and Cataclysm's Sentinel force-field wall (9, 10) are not.

use hwc_math::crt::qsort;
use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, fild_i32, ld, round_magic, st, Ext};
use hwc_math::{Matrix, Vector};

use crate::aiship::{self, flags as fly};
use crate::aitrack;
use crate::command::{attr, CommandToDo};
use crate::object::{sof, special, CommandId, ShipId};
use crate::statics::ShipType;
use crate::tactics;
use crate::universe::Universe;

/// Formation types (`TypeOfFormation`).
pub mod kind {
    pub const DELTA: u32 = 0;
    pub const BROAD: u32 = 1;
    pub const DELTA3D: u32 = 2;
    pub const CLAW: u32 = 3;
    pub const WALL: u32 = 4;
    pub const SPHERE: u32 = 5;
    pub const CUSTOM: u32 = 6;
}

/// The formations this slice implements, in the order `F` cycles through them.
pub const CYCLE: [u32; 5] = [kind::DELTA, kind::BROAD, kind::DELTA3D, kind::CLAW, kind::WALL];

/// Display name of a formation type (the original's "nice" names).
pub fn name(t: u32) -> &'static str {
    match t {
        kind::DELTA => "Delta Wing",
        kind::BROAD => "Broad Attack",
        kind::DELTA3D => "Formation X",
        kind::CLAW => "The Claw",
        kind::WALL => "The Wall",
        kind::SPHERE => "Sphere",
        kind::CUSTOM => "Custom",
        _ => "?",
    }
}

/// `FormationCommand` (command +0x230..+0x28c).
#[derive(Debug, Clone, PartialEq)]
pub struct FormationInfo {
    pub formation_type: u32,
    pub locked: bool,
    pub coordsys: Matrix,
    /// Tight (0) or loose (1).
    pub tactical_state: usize,
    pub enders: u32,
    pub travel_vel: f32,
    pub percent_max_speed: f32,
    pub sort_order: u32,
    pub flip_state: u32,
    /// Cataclysm's extra formation parameter (+0x288).
    pub variant: i32,
}

impl Default for FormationInfo {
    fn default() -> Self {
        Self {
            formation_type: 0,
            locked: false,
            coordsys: Matrix::IDENTITY,
            tactical_state: 0,
            enders: 0,
            travel_vel: 0.0,
            percent_max_speed: 0.0,
            sort_order: 0,
            flip_state: 0,
            variant: 0,
        }
    }
}

fn dist_sq(a: Vector, b: Vector) -> Ext {
    let dx = ld(a.x) - ld(b.x);
    let dy = ld(a.y) - ld(b.y);
    let dz = ld(a.z) - ld(b.z);
    (dz * dz + dy * dy) + dx * dx
}

/// `setFormationTravelVelocity` (0x47eae0): the slowest ship's top speed (including
/// Cataclysm's per-ship speed caps) times `percent_max_speed`.
pub fn set_travel_velocity(u: &mut Universe, cid: CommandId) {
    let Some(cmd) = u.commands.get(cid) else { return };
    let mut min = f32::from_bits(0x6258_d727);
    for &id in &cmd.selection {
        let s = u.ships.ship(id);
        let Some(stat) = u.statics.ship(s.ship_type) else { continue };
        let mult = st(tactics::speed_multiplier(s, stat));
        let v = tactics::ships_max_velocity(s, stat, u.max_velocity_context(s)) * ld(mult);
        if v < ld(min) {
            min = st(v);
        }
        if ld(s.speed_cap) > 0.0 && ld(s.speed_cap) < ld(min) {
            min = s.speed_cap;
        }
    }
    let pct = cmd.formation.percent_max_speed;
    if let Some(cmd) = u.commands.get_mut(cid) {
        cmd.formation.travel_vel = st(ld(min) * ld(pct));
    }
}

/// `formationBiggestThenClosestOptimum` (0x47c770): biggest ship first, the rest by size then
/// by distance from the group centre (or from the biggest ship when sizes differ).
pub fn biggest_then_closest(u: &Universe, selection: &mut [ShipId]) {
    let size = |id: ShipId| {
        let s = u.ships.ship(id);
        u.statics.ship(s.ship_type).map_or(0.0, |st| st.header.coll.coll_sphere_size)
    };
    let mut biggest = 0.0f32;
    let mut biggest_index = usize::MAX;
    let mut smallest = 1_000_000.0f32;
    for (i, &id) in selection.iter().enumerate() {
        let c = size(id);
        if ld(c) > ld(biggest) {
            biggest = c;
            biggest_index = i;
        }
        if ld(c) < ld(smallest) {
            smallest = c;
        }
    }
    let avg = if eq_or_unordered(ld(smallest), ld(biggest)) {
        let (mut x, mut y, mut z) = (0.0f64, 0.0f64, 0.0f64);
        for &id in selection.iter() {
            let p = u.ships.ship(id).posinfo.position;
            x += ld(p.x);
            y += ld(p.y);
            z += ld(p.z);
        }
        let (y, z) = (st(y), st(z));
        let inv = st(1.0 / fild_i32(selection.len() as i32));
        Vector::new(st(ld(inv) * x), st(ld(inv) * ld(y)), st(ld(inv) * ld(z)))
    } else {
        u.ships.ship(selection[biggest_index]).posinfo.position
    };
    let mut items: Vec<(f32, ShipId, f32)> = selection
        .iter()
        .map(|&id| (st(dist_sq(u.ships.ship(id).posinfo.position, avg)), id, size(id)))
        .collect();
    qsort(&mut items, |a, b| {
        if eq_or_unordered(ld(a.2), ld(b.2)) {
            if ld(a.0) < ld(b.0) || a.0.is_nan() || b.0.is_nan() {
                -1
            } else {
                1
            }
        } else if ld(a.2) > ld(b.2) {
            -1
        } else {
            1
        }
    });
    for (s, (_, id, _)) in selection.iter_mut().zip(&items) {
        *s = *id;
    }
    // A cloak generator leads so that its field covers the most ships.
    let mut cloak = None;
    for (i, &id) in selection.iter().enumerate() {
        if matches!(u.ships.ship(id).ship_type, ShipType::kCloakGenerator | ShipType::tCloakGenerator) {
            cloak = Some(i);
        }
    }
    if let Some(i) = cloak {
        selection.swap(0, i);
    }
}

/// One wing of a delta/broad/3D/claw formation: the running separation along that wing.
struct Wing {
    sep: f32,
    pos: Vector,
}

/// `FormationCalculateOffsets` (0x47ec90).
pub fn calculate_offsets(u: &mut Universe, cid: CommandId) {
    let Some(cmd) = u.commands.get(cid) else { return };
    let ftype = cmd.formation.formation_type;
    let flip = cmd.formation.flip_state != 0;
    let selection = cmd.selection.clone();
    let tactical = cmd.formation.tactical_state.min(1);
    if selection.is_empty() {
        return;
    }
    let tw = u.tweaks.clone();
    let pad = ld(tw.formation_padding[tactical]);
    let sep_of = |u: &Universe, id: ShipId| -> Ext {
        let s = u.ships.ship(id);
        u.statics.ship(s.ship_type).map_or(0.0, |st| {
            ld(st.formation_padding_modifier) * ld(st.header.coll.coll_sphere_size) * pad
        })
    };
    let leader_sep = st(sep_of(u, selection[0]));
    let n = selection.len();
    let set = |u: &mut Universe, id: ShipId, v: Vector| u.ships.ship_mut(id).formation_offset = v;
    match ftype {
        kind::DELTA | kind::BROAD => {
            let scale = ld(if ftype == kind::DELTA { tw.delta_formation_padding_scale } else { tw.broad_formation_padding_scale });
            let mut right = Wing { sep: leader_sep, pos: Vector::ZERO };
            let mut left = Wing { sep: leader_sep, pos: Vector::ZERO };
            for (i, &id) in selection.iter().enumerate().skip(1) {
                let this_sep = sep_of(u, id);
                let (w, sign) = if i & 1 != 0 { (&mut right, 1.0) } else { (&mut left, -1.0) };
                let r = (this_sep + ld(w.sep)) * scale;
                let side = if flip { -sign } else { sign };
                w.pos.y = if side > 0.0 { st(r + ld(w.pos.y)) } else { st(ld(w.pos.y) - r) };
                if ftype == kind::DELTA {
                    w.pos.z = st(ld(w.pos.z) - r);
                }
                set(u, id, w.pos);
                w.sep = st(this_sep);
            }
        }
        kind::DELTA3D | kind::CLAW => {
            let claw = ftype == kind::CLAW;
            let scale = ld(if claw { tw.claw_formation_padding_scale } else { tw.delta3d_formation_padding_scale });
            // Wings: 1 up-right, 2 up-left, 3 down-right, 0 down-left (index i & 3).
            let mut wings: [Wing; 4] = std::array::from_fn(|_| Wing { sep: leader_sep, pos: Vector::ZERO });
            let mut forward: Ext = 1.0; // kept in a register
            for (i, &id) in selection.iter().enumerate().skip(1) {
                let this_sep = sep_of(u, id);
                let q = i & 3;
                let (up, right) = match q {
                    1 => (1.0, 1.0),
                    2 => (1.0, -1.0),
                    3 => (-1.0, 1.0),
                    _ => (-1.0, -1.0),
                };
                let w = &mut wings[q];
                let r = (this_sep + ld(w.sep)) * scale;
                w.pos.x = if up > 0.0 { st(r + ld(w.pos.x)) } else { st(ld(w.pos.x) - r) };
                let side = if flip { -right } else { right };
                w.pos.y = if side > 0.0 { st(r + ld(w.pos.y)) } else { st(ld(w.pos.y) - r) };
                if claw {
                    let forward_sep = st(r * forward);
                    w.pos.z = st(ld(forward_sep) + ld(w.pos.z));
                }
                set(u, id, w.pos);
                w.sep = st(this_sep);
                if claw && q == 0 {
                    forward *= ld(tw.claw_formation_curvature);
                }
            }
        }
        kind::WALL => {
            let mut square = round_magic(fsqrt(fild_i32(n as i32)));
            if square * square < n as i32 {
                square += 1;
            }
            let leader = u.ships.ship(selection[0]);
            let sep = u.statics.ship(leader.ship_type).map_or(0.0, |si| {
                st(ld(si.formation_padding_modifier) * ld(si.header.coll.coll_sphere_size) * ld(tw.wall_formation_padding_scale) * pad
                    + ld(tw.wall_formation_padding))
            });
            let step = if flip { -sep } else { sep };
            let mut pos = Vector::new(0.0, step, 0.0);
            let mut row_x = 0.0f32;
            let mut col = 1;
            for &id in selection.iter().skip(1) {
                set(u, id, pos);
                col += 1;
                if col < square {
                    pos.y = if flip { pos.y - sep } else { sep + pos.y };
                } else {
                    row_x -= sep;
                    col = 0;
                    pos = Vector::new(row_x, 0.0, 0.0);
                }
            }
        }
        kind::CUSTOM => {
            // Leader = the ship closest (in summed axis distance) to the group centre; the others
            // keep their current offsets from it.
            let mut c = Vector::ZERO;
            for &id in &selection {
                let p = u.ships.ship(id).posinfo.position;
                c = c + p;
            }
            let inv = st(1.0 / fild_i32(n as i32));
            let c = Vector::new(st(ld(inv) * ld(c.x)), st(ld(inv) * ld(c.y)), st(ld(inv) * ld(c.z)));
            let mut best = (f32::from_bits(0x6258_d727), 0usize);
            for (i, &id) in selection.iter().enumerate() {
                let p = u.ships.ship(id).posinfo.position;
                let e = |a: f32, b: f32| {
                    let d = a - b;
                    if d < 0.0 {
                        -d
                    } else {
                        d
                    }
                };
                let total = (e(p.z, c.z) + e(p.y, c.y)) + e(p.x, c.x);
                if total < best.0 {
                    best = (total, i);
                }
            }
            let mut sel = selection.clone();
            sel.swap(0, best.1);
            if let Some(cmd) = u.commands.get_mut(cid) {
                cmd.selection = sel.clone();
            }
            let leader = u.ships.ship(sel[0]).clone();
            for &id in sel.iter().skip(1) {
                let p = u.ships.ship(id).posinfo.position;
                let off = leader.rotinfo.coordsys.vec_mul(p - leader.posinfo.position);
                set(u, id, off);
            }
        }
        _ => {}
    }
}

/// `calculateTravelDistance` (0x47f8a0): summed squared distance of the wingmen from their
/// slots around the current leader.
fn travel_distance(u: &Universe, selection: &[ShipId]) -> f32 {
    let leader = u.ships.ship(selection[0]);
    let mut total = 0.0f32;
    for &id in selection.iter().skip(1) {
        let s = u.ships.ship(id);
        let slot = leader.rotinfo.coordsys.mul_vec(s.formation_offset) + leader.posinfo.position;
        total = st(dist_sq(s.posinfo.position, slot) + ld(total));
    }
    total
}

/// `formationArrageCrazyOptimum` (0x47f980): try leaders (all of them when the ships are of
/// similar size) and give each wingman the free slot nearest to it; keep the arrangement with
/// the least total travel.
pub fn arrange_crazy_optimum(u: &mut Universe, cid: CommandId) {
    let mut selection = u.commands.get(cid).map(|c| c.selection.clone()).unwrap_or_default();
    let n = selection.len();
    if n == 0 {
        return;
    }
    if n != 1 {
        biggest_then_closest(u, &mut selection);
        if let Some(c) = u.commands.get_mut(cid) {
            c.selection = selection.clone();
            c.formation.sort_order = 1;
        }
    }
    let size = |u: &Universe, id: ShipId| {
        let s = u.ships.ship(id);
        u.statics.ship(s.ship_type).map_or(1.0, |st| st.header.coll.coll_sphere_size)
    };
    let redo = ld(size(u, selection[n - 1])) / ld(size(u, selection[0])) > ld(f32::from_bits(0x3f33_3333));
    let original = selection.clone();
    let mut optimum = selection.clone();
    let mut min_leader = f32::from_bits(0x6258_d727);
    for a in 0..n {
        let mut sel = original.clone();
        sel[0] = original[a];
        sel[a] = original[0];
        u.commands.get_mut(cid).expect("command").selection = sel.clone();
        calculate_offsets(u, cid);
        let sel = u.commands.get(cid).expect("command").selection.clone();
        let leader = u.ships.ship(sel[0]).clone();
        let mut positions = vec![Vector::ZERO; n];
        let mut wants = vec![0usize; n];
        let mut gets = vec![0usize; n];
        let mut taken = vec![false; n];
        for i in 1..n {
            let s = u.ships.ship(sel[i]);
            positions[i] = leader.rotinfo.coordsys.mul_vec(s.formation_offset) + leader.posinfo.position;
        }
        for i in 1..n {
            let p = u.ships.ship(sel[i]).posinfo.position;
            let mut min = f32::from_bits(0x6258_d727);
            for (j, pj) in positions.iter().enumerate().skip(1) {
                let d = dist_sq(p, *pj);
                if d < ld(min) {
                    min = st(d);
                    wants[i] = j;
                }
            }
        }
        for i in 1..n {
            let wanted = wants[i];
            if wanted == 0 {
                continue;
            }
            let count = (1..n).filter(|&j| j != i && wants[j] == wanted).count() + 1;
            if count == 1 {
                gets[i] = wanted;
                taken[wanted] = true;
            } else {
                let mut min = f32::from_bits(0x6258_d727);
                let mut winner = 0;
                for j in 1..n {
                    if wants[j] == wanted {
                        let d = dist_sq(u.ships.ship(sel[j]).posinfo.position, positions[wanted]);
                        if d < ld(min) {
                            min = st(d);
                            winner = j;
                        }
                    }
                }
                for j in 1..n {
                    if wants[j] == wanted && j != winner {
                        wants[j] = 0;
                        gets[j] = 0;
                    }
                }
                gets[winner] = wanted;
                taken[wanted] = true;
            }
        }
        let mut last_winner = 0;
        for i in 1..n {
            if wants[i] == 0 {
                let p = u.ships.ship(sel[i]).posinfo.position;
                let mut min = f32::from_bits(0x6258_d727);
                for j in 1..n {
                    if !taken[j] {
                        let d = dist_sq(p, positions[j]);
                        if d < ld(min) {
                            min = st(d);
                            last_winner = j;
                        }
                    }
                }
                wants[i] = last_winner;
                gets[i] = last_winner;
                taken[last_winner] = true;
            }
        }
        let mut test = sel.clone();
        for i in 1..n {
            test[gets[i]] = sel[i];
        }
        u.commands.get_mut(cid).expect("command").selection = test;
        calculate_offsets(u, cid);
        let sel = u.commands.get(cid).expect("command").selection.clone();
        let d = travel_distance(u, &sel);
        if ld(d) < ld(min_leader) {
            min_leader = d;
            optimum = sel;
        }
        if !redo {
            break;
        }
    }
    u.commands.get_mut(cid).expect("command").selection = optimum;
    calculate_offsets(u, cid);
}

/// `FillInFormationSpecifics` (0x47ffc0).
pub fn fill_in_specifics(u: &mut Universe, cid: CommandId, formation_type: u32, variant: i32, new_formation: bool) {
    {
        let tw = u.tweaks.clone();
        let c = u.commands.get_mut(cid).expect("command");
        c.formation.formation_type = formation_type;
        c.formation.variant = variant;
        c.formation.tactical_state = 0;
        c.formation.locked = false;
        c.formation.enders = 0;
        c.formation.percent_max_speed = tw.formation_travelvel_minscale;
        c.formation.sort_order = 0;
        if new_formation {
            c.formation.flip_state = 0;
        }
    }
    set_travel_velocity(u, cid);
    arrange_crazy_optimum(u, cid);
}

/// The formation is now a different type (`formationTypeHasChanged`).
pub fn type_has_changed(u: &mut Universe, cid: CommandId) {
    let c = u.commands.get(cid).expect("command");
    if c.formation.sort_order != 1 && c.selection.len() != 1 {
        let mut sel = c.selection.clone();
        biggest_then_closest(u, &mut sel);
        let c = u.commands.get_mut(cid).expect("command");
        c.selection = sel;
        c.formation.sort_order = 1;
    }
    calculate_offsets(u, cid);
}

/// Cataclysm's formation fly-to helper for steady formations (0x47db90).
fn fly_to_slot_steady(u: &mut Universe, id: ShipId, leader_vel: Vector, slot: Vector, heading: &mut Vector) {
    let dist = st(aitrack::move_left_to_go(u.ships.ship(id), slot));
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let ship_type = u.ships.ship(id).ship_type;
    let stat = statics.ship(ship_type).expect("ship statics");
    let big = tactics::is_capital_ship_static_or_big(stat);
    let far_flags = fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE | fly::DONT_FLY_TO_OBSCURED_POINTS
        | fly::RETURN_IMMED_IF_POINT_OBSCURED | fly::IGNORE_FORMATION_OBSCURED_POINTS;
    let near_flags = far_flags & !fly::POINT_IN_DIRECTION_FLYING;
    let d = ld(dist);
    let fl = if (!big && d > 100.0) || (big && d > 500.0) {
        far_flags
    } else {
        let s = u.ships.ship_mut(id);
        aitrack::heading(s, stat, &tw, dt, heading, tw.flyship_heading_accuracy, tw.flyship_heading_accuracy, stat.sinbank, 0);
        if !(big && d > 100.0) {
            s.flags |= sof::FORMATION_SETTLED;
        }
        near_flags
    };
    if ld(dist) >= 10.0 {
        let r = aiship::fly_to_point_avoiding_objs(u, id, Some(slot), fl, 0.0, Some(leader_vel));
        if r & aiship::FLY_OBJECT_IN_WAY == 0 {
            return;
        }
        if fl & fly::POINT_IN_DIRECTION_FLYING != 0 {
            let s = u.ships.ship_mut(id);
            aitrack::heading(s, stat, &tw, dt, heading, tw.flyship_heading_accuracy, tw.flyship_heading_accuracy, stat.sinbank, 0);
        }
    }
    let s = u.ships.ship_mut(id);
    aitrack::steady_ship_drift_only(s, stat, &tw, dt);
}

/// `processFormationToDo` (0x47e100) for the slot-based formation types.
pub fn process(u: &mut Universe, cid: CommandId, steady: bool, passive_attacked: bool) {
    let Some(cmd) = u.commands.get(cid).cloned() else { return };
    let selection = cmd.selection.clone();
    let n = selection.len();
    if n == 0 {
        return;
    }
    let leader_id = selection[0];
    let leader_cmd = u.ships.ship(leader_id).command.unwrap_or(cid);
    let tw = u.tweaks.clone();
    let calc_error = (tw.formation_error_calculate_rate & u.update_counter) == tw.formation_error_calculate_frame;
    if n == 1 {
        if let Some(c) = u.commands.get_mut(leader_cmd) {
            c.formation.percent_max_speed = tw.formation_travelvel_maxscale;
        }
        set_travel_velocity(u, leader_cmd);
        return;
    }
    let statics = u.statics.clone();
    let dt = u.phys_time_elapsed;
    let leader = u.ships.ship(leader_id).clone();
    let origin = leader.coll.coll_position;
    let (coordsys, mut heading) = if cmd.formation.locked {
        (cmd.formation.coordsys, cmd.formation.coordsys.col(2))
    } else {
        (leader.rotinfo.coordsys, leader.heading())
    };
    let leader_vel = leader.posinfo.velocity;
    let mut error = 0.0f32;
    match cmd.formation.formation_type {
        0 | 1 | 2 | 3 | 4 | 6 | 9 => {
            for &id in selection.iter().skip(1) {
                let ship = u.ships.ship(id).clone();
                let stat = statics.ship(ship.ship_type).expect("ship statics");
                let dont_rotate = stat.rotate_to_retaliate && passive_attacked;
                u.ships.ship_mut(id).ship_idle = false;
                let slot = coordsys.mul_vec(ship.formation_offset) + origin;
                if calc_error {
                    error = st(dist_sq(slot, ship.posinfo.position) + ld(error));
                }
                if ship.special_flags & special::BROKEN_FORMATION != 0 {
                    continue;
                }
                if !dont_rotate {
                    if steady {
                        fly_to_slot_steady(u, id, leader_vel, slot, &mut heading);
                        continue;
                    }
                    let dist = st(aitrack::move_left_to_go(&ship, slot));
                    let big = tactics::is_capital_ship_static_or_big(stat);
                    let mut fl = fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE;
                    let near = if big { ld(dist) <= 500.0 } else { ld(dist) <= 100.0 };
                    if near {
                        let s = u.ships.ship_mut(id);
                        aitrack::heading(s, stat, &tw, dt, &mut heading, tw.flyship_heading_accuracy, tw.flyship_heading_accuracy, stat.sinbank, 0);
                        fl = fly::FAST_AS_POSSIBLE;
                    }
                    if !(ld(dist) < 10.0) {
                        aiship::fly_to_point_avoiding_objs(u, id, Some(slot), fl, 0.0, Some(leader_vel));
                    }
                } else if !steady {
                    if !aitrack::move_reached_destination(&ship, slot, 10.0) {
                        aiship::fly_to_point_avoiding_objs(u, id, Some(slot), fly::FAST_AS_POSSIBLE, 0.0, Some(leader_vel));
                    }
                } else {
                    if aitrack::move_reached_destination(&ship, slot, 100.0) {
                        u.ships.ship_mut(id).flags |= sof::FORMATION_SETTLED;
                    }
                    let reached = aitrack::move_reached_destination(u.ships.ship(id), slot, 10.0);
                    let blocked = !reached
                        && aiship::fly_to_point_avoiding_objs(
                            u,
                            id,
                            Some(slot),
                            fly::FAST_AS_POSSIBLE | fly::DONT_FLY_TO_OBSCURED_POINTS | fly::RETURN_IMMED_IF_POINT_OBSCURED | fly::IGNORE_FORMATION_OBSCURED_POINTS,
                            0.0,
                            Some(leader_vel),
                        ) & aiship::FLY_OBJECT_IN_WAY
                            != 0;
                    if reached || blocked {
                        let s = u.ships.ship_mut(id);
                        aitrack::steady_ship_drift_only(s, stat, &tw, dt);
                    }
                }
            }
        }
        _ => {}
    }
    if calc_error {
        let avg = st(ld(error) / fild_i32(n as i32));
        let pct = if ld(avg) > ld(tw.formation_error_big) {
            tw.formation_travelvel_minscale
        } else if ld(avg) < ld(tw.formation_error_negligible) {
            tw.formation_travelvel_maxscale
        } else {
            st(ld(tw.formation_travelvel_maxscale)
                - ld(avg) / ld(tw.formation_error_big) * (ld(tw.formation_travelvel_maxscale) - ld(tw.formation_travelvel_minscale)))
        };
        if let Some(c) = u.commands.get_mut(leader_cmd) {
            c.formation.percent_max_speed = pct;
        }
        set_travel_velocity(u, leader_cmd);
    }
}

/// Whether a command is a formation of a type this slice can fly.
pub fn is_supported(cmd: &CommandToDo) -> bool {
    cmd.attributes & attr::FORMATION != 0 && matches!(cmd.formation.formation_type, 0..=4 | 6)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::command::CommandToDo;
    use crate::statics::{ShipRace, ShipStaticInfo, StaticWorld, Tweaks};
    use crate::tunables::SimTweaks;

    type Row = (u32, u32, &'static [f32], &'static [f32], &'static [[u32; 3]]);

    /// `FormationCalculateOffsets` (0x47ec90) emulated in cataclysm.exe: formation type, flip
    /// state, each ship's collision size and padding modifier, and the offsets it wrote
    /// (tight formation padding 1, compiled-in formation tweaks).
    const ROWS: &[Row] = &[
        (0, 0, &[f32::from_bits(0x42566998), f32::from_bits(0x4277b34b), f32::from_bits(0x428353bd), f32::from_bits(0x4297f16b), f32::from_bits(0x42772bee), f32::from_bits(0x42952021), f32::from_bits(0x41407c61)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a)], &[[0x00000000, 0x00000000, 0x00000000], [0x00000000, 0x433d5f8e, 0xc33d5f8e], [0x00000000, 0xc342fb1f, 0xc342fb1f], [0x00000000, 0x43c61bed, 0xc3c61bed], [0x00000000, 0xc3c11533, 0xc3c11533], [0x00000000, 0x442118dc, 0xc42118dc], [0x00000000, 0xc3fa413c, 0xc3fa413c]]),
        (0, 1, &[f32::from_bits(0x428d9516), f32::from_bits(0x42129cba), f32::from_bits(0x41891b0d), f32::from_bits(0x41dba00d), f32::from_bits(0x4274bb40), f32::from_bits(0x421a4845), f32::from_bits(0x41b5f65e)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000)], &[[0x00000000, 0x00000000, 0x00000000], [0x00000000, 0xc336675a, 0xc336675a], [0x00000000, 0x431921a7, 0xc31921a7], [0x00000000, 0xc38b4811, 0xc38b4811], [0x00000000, 0x43907be5, 0xc3907be5], [0x00000000, 0xc3bccc9f, 0xc3bccc9f], [0x00000000, 0x43d89b86, 0xc3d89b86]]),
        (1, 0, &[f32::from_bits(0x4121fce3), f32::from_bits(0x428dff25), f32::from_bits(0x41c54bab), f32::from_bits(0x41c8ab61), f32::from_bits(0x429d89f9), f32::from_bits(0x428e2318), f32::from_bits(0x41f202ca), f32::from_bits(0x429a9b5f)], &[f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a)], &[[0x00000000, 0x00000000, 0x00000000], [0x00000000, 0x42f35e22, 0x00000000], [0x00000000, 0xc250b795, 0x00000000], [0x00000000, 0x4388aa7e, 0x00000000], [0x00000000, 0xc366f50f, 0x00000000], [0x00000000, 0x43d48aef, 0x00000000], [0x00000000, 0xc3d598ee, 0x00000000], [0x00000000, 0x4427b568, 0x00000000]]),
        (2, 0, &[f32::from_bits(0x41acef76), f32::from_bits(0x41a197d3), f32::from_bits(0x4168f4dc), f32::from_bits(0x41f8c2d7), f32::from_bits(0x4250deed), f32::from_bits(0x4123ca02), f32::from_bits(0x4265d254)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a)], &[[0x00000000, 0x00000000, 0x00000000], [0x428a6b18, 0x428a6b18, 0x00000000], [0x42823c74, 0xc2823c74, 0x00000000], [0xc2bdc370, 0x42bdc370, 0x00000000], [0xc304e6ff, 0xc304e6ff, 0x00000000], [0x42ebde48, 0x42ebde48, 0x00000000], [0x4342bed2, 0xc342bed2, 0x00000000]]),
        (3, 0, &[f32::from_bits(0x41b77917), f32::from_bits(0x422c4f8d), f32::from_bits(0x41b2a093), f32::from_bits(0x41df763a), f32::from_bits(0x4297762f), f32::from_bits(0x4298e410), f32::from_bits(0x420c23ed), f32::from_bits(0x421933cd)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000)], &[[0x00000000, 0x00000000, 0x00000000], [0x42eda47d, 0x42eda47d, 0x42eda47d], [0x42a2f1f3, 0xc2a2f1f3, 0x42a2f1f3], [0xc2a65c6d, 0x42a65c6d, 0x42a65c6d], [0xc33198d0, 0xc33198d0, 0x433198d0], [0x439b83bf, 0x439b83bf, 0x43a52002], [0x432e3760, 0xc32e3760, 0x43377d9e], [0xc33687ce, 0x433687ce, 0x4340772a]]),
        (3, 1, &[f32::from_bits(0x4192a6c9), f32::from_bits(0x41d9cf01), f32::from_bits(0x4286baa3), f32::from_bits(0x42296ae1), f32::from_bits(0x42377fb7), f32::from_bits(0x4290fac4), f32::from_bits(0x41dc8028), f32::from_bits(0x42299912)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a)], &[[0x00000000, 0x00000000, 0x00000000], [0x42a401cf, 0xc2a401cf, 0x42a401cf], [0x43060b1b, 0x43060b1b, 0x43060b1b], [0xc2da780c, 0xc2da780c, 0x42da780c], [0xc2cb9e0a, 0x42cb9e0a, 0x42cb9e0a], [0x436fbec1, 0xc36fbec1, 0x437f84f0], [0x438e59f6, 0x438e59f6, 0x4395e266], [0xc382e54d, 0xc382e54d, 0x438a8608]]),
        (4, 0, &[f32::from_bits(0x4294883f), f32::from_bits(0x4207304d), f32::from_bits(0x4295633e), f32::from_bits(0x4240a3ed), f32::from_bits(0x41fef8e3), f32::from_bits(0x4200b439), f32::from_bits(0x41b36337)], &[f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000)], &[[0x00000000, 0x00000000, 0x00000000], [0x00000000, 0x4328883f, 0x00000000], [0x00000000, 0x43a8883f, 0x00000000], [0xc328883f, 0x00000000, 0x00000000], [0xc328883f, 0x4328883f, 0x00000000], [0xc328883f, 0x43a8883f, 0x00000000], [0xc3a8883f, 0x00000000, 0x00000000]]),
        (4, 1, &[f32::from_bits(0x429e234c), f32::from_bits(0x423d637c), f32::from_bits(0x4219a60e), f32::from_bits(0x41d4e893), f32::from_bits(0x424e4f12), f32::from_bits(0x4287ae6e), f32::from_bits(0x42279613)], &[f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f800000), f32::from_bits(0x3f99999a), f32::from_bits(0x3f99999a), f32::from_bits(0x3f800000)], &[[0x00000000, 0x00000000, 0x00000000], [0x00000000, 0xc351c3f5, 0x00000000], [0x00000000, 0xc3d1c3f5, 0x00000000], [0xc351c3f5, 0x00000000, 0x00000000], [0xc351c3f5, 0xc351c3f5, 0x00000000], [0xc351c3f5, 0xc3d1c3f5, 0x00000000], [0xc3d1c3f5, 0x00000000, 0x00000000]]),
    ];

    #[test]
    fn offsets_match_emulated_binary() {
        let types = [
            ShipType::sAcolyte,
            ShipType::sMimic,
            ShipType::sLeech,
            ShipType::sACV,
            ShipType::sMCV,
            ShipType::sRecon,
            ShipType::sWorker,
            ShipType::sSwarmer,
        ];
        for (i, (ftype, flip, sizes, mods, expected)) in ROWS.iter().enumerate() {
            let mut ships: Vec<Option<ShipStaticInfo>> = vec![None; ShipType::COUNT];
            for (k, t) in types.iter().enumerate().take(sizes.len()) {
                let mut s = ShipStaticInfo::new(*t, String::new());
                s.ship_race = ShipRace::Somtaaw;
                s.header.coll.coll_sphere_size = sizes[k];
                s.formation_padding_modifier = mods[k];
                s.health.max_health = 1.0;
                ships[t.index()] = Some(s);
            }
            let statics = StaticWorld {
                options: Default::default(),
                tweaks: Tweaks::default(),
                ships,
                missiles: Vec::new(),
                mines: Vec::new(),
                mine_max_diameter: 0.0,
                asteroids: Vec::new(),
                dust_clouds: Vec::new(),
                nebulae: Vec::new(),
                derelicts: Vec::new(),
                research: Default::default(),
                unit_caps: Default::default(),
                unit_cap_presets: BTreeMap::new(),
                team_colors: Default::default(),
                default_lod: None,
                default_mex: None,
                warnings: Vec::new(),
            };
            let mut tw = SimTweaks::compiled();
            tw.formation_padding = [1.0, 2.0];
            let mut u = Universe::new(Arc::new(statics), tw, &[ShipRace::Somtaaw]);
            let ids: Vec<ShipId> = types
                .iter()
                .take(sizes.len())
                .map(|t| u.add_ship(*t, 0, Vector::ZERO, None).unwrap())
                .collect();
            let mut cmd = CommandToDo {
                id: CommandId(0),
                selection: ids.clone(),
                order: 0,
                attributes: crate::command::attr::FORMATION,
                waypoint_mode: 0,
                waypoints: Vec::new(),
                waypoint_index: 0,
                formation: FormationInfo::default(),
            };
            cmd.formation.formation_type = *ftype;
            cmd.formation.flip_state = *flip;
            let cid = u.commands.insert_for_test(cmd);
            calculate_offsets(&mut u, cid);
            for (k, e) in expected.iter().enumerate().skip(1) {
                let o = u.ships.ship(ids[k]).formation_offset;
                assert_eq!([o.x.to_bits(), o.y.to_bits(), o.z.to_bits()], *e, "row {i} (type {ftype}) ship {k}");
            }
        }
    }
}
