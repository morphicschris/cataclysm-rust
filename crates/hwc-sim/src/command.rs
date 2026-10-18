//! `CommandLayer.c`: the orders ships are carrying out (the "to do" list) and how a move order
//! is executed, with Cataclysm's waypoint lists.
//!
//! In Cataclysm a move carries up to 20 waypoints and a mode (0 = once, 1 = loop, 2/3 = patrol
//! back and forth). A group that is not in formation is split into one command per ship, each
//! with the waypoints shifted by the ship's offset from the group (so the group keeps its shape,
//! shrunk according to the distance flown).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, ld, st, Ext};
use hwc_math::Vector;

use crate::aiship::{self, flags as fly};
use crate::aitrack;
use crate::formation::{self, FormationInfo};
use crate::object::{sof, special, CommandId, ShipId, ShipTable};
use crate::universe::Universe;

/// Orders (`COMMAND_*`).
pub mod order {
    pub const NULL: u16 = 0;
    pub const MOVE: u16 = 1;
    pub const ATTACK: u16 = 2;
    pub const DOCK: u16 = 3;
    pub const HALT: u16 = 8;
    pub const MILITARY_PARADE: u16 = 9;
}

/// Order attributes (`COMMAND_IS_*`).
pub mod attr {
    pub const FORMATION: u16 = 0x01;
    pub const PROTECTING: u16 = 0x02;
    pub const PASSIVE_ATTACKING: u16 = 0x04;
    pub const HOLDING_PATTERN: u16 = 0x08;
    pub const ATTACKING_AND_MOVING: u16 = 0x10;
}

/// Most waypoints a move carries.
pub const MAX_WAYPOINTS: usize = 20;

/// Waypoint list modes (command +0x18).
pub mod waypoint_mode {
    pub const ONCE: u32 = 0;
    pub const LOOP: u32 = 1;
    pub const PATROL: u32 = 2;
    pub const PATROL_BACK: u32 = 3;
}

/// `CommandToDo`, reduced to the parts the move and formation orders use.
#[derive(Debug, Clone, PartialEq)]
pub struct CommandToDo {
    pub id: CommandId,
    pub selection: Vec<ShipId>,
    pub order: u16,
    pub attributes: u16,
    pub waypoint_mode: u32,
    pub waypoints: Vec<Vector>,
    /// Index of the waypoint being flown to (+0x11c).
    pub waypoint_index: i32,
    pub formation: FormationInfo,
}

/// `CommandLayer`: the to-do list in order, over a store indexed by [`CommandId`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CommandLayer {
    slots: Vec<Option<CommandToDo>>,
    /// The to-do list (new commands are appended).
    pub todo: Vec<CommandId>,
}

impl CommandLayer {
    pub fn get(&self, id: CommandId) -> Option<&CommandToDo> {
        self.slots.get(id.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: CommandId) -> Option<&mut CommandToDo> {
        self.slots.get_mut(id.0 as usize).and_then(Option::as_mut)
    }

    /// Commands in to-do order.
    pub fn iter(&self) -> impl Iterator<Item = &CommandToDo> {
        self.todo.iter().filter_map(|&id| self.get(id))
    }

    fn add(&mut self, mut cmd: CommandToDo) -> CommandId {
        let id = CommandId(self.slots.len() as u32);
        cmd.id = id;
        self.slots.push(Some(cmd));
        self.todo.push(id);
        id
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, cmd: CommandToDo) -> CommandId {
        self.add(cmd)
    }

    fn delete(&mut self, id: CommandId) {
        self.todo.retain(|&c| c != id);
        if let Some(s) = self.slots.get_mut(id.0 as usize) {
            *s = None;
        }
    }
}

fn new_command(selection: Vec<ShipId>) -> CommandToDo {
    CommandToDo {
        id: CommandId(0),
        selection,
        order: order::NULL,
        attributes: 0,
        waypoint_mode: 0,
        waypoints: Vec::new(),
        waypoint_index: 0,
        formation: FormationInfo::default(),
    }
}

/// `clRemoveShipFromSelection`: swap-remove every occurrence.
fn remove_from_selection(sel: &mut Vec<ShipId>, id: ShipId) -> bool {
    let mut removed = false;
    let mut i = 0;
    while i < sel.len() {
        if sel[i] == id {
            sel.swap_remove(i);
            removed = true;
            continue;
        }
        i += 1;
    }
    removed
}

/// `SelectionsAreEquivalent`.
fn selections_equivalent(a: &[ShipId], b: &[ShipId]) -> bool {
    a.len() == b.len() && a.iter().all(|s| b.contains(s))
}

/// The scale `CalculateMoveToPoints` applies to the group's offsets: 0.1 when the move is
/// shorter than the group's radius, 1 when it is five or more radii, linear in between.
fn move_offsets(ships: &ShipTable, sel: &[ShipId], from: Vector, to: Vector) -> Vec<Vector> {
    let n = sel.len();
    if n == 1 {
        return vec![Vector::ZERO];
    }
    let dx = ld(to.x) - ld(from.x);
    let dy = ld(to.y) - ld(from.y);
    let dz = ld(to.z) - ld(from.z);
    let dmag = st(fsqrt((dz * dz + dy * dy) + dx * dx));
    let mut max_rsq: Ext = -1.0;
    let mut offsets = Vec::with_capacity(n);
    for &id in sel {
        let p = ships.ship(id).posinfo.position;
        let r = Vector::new(p.x - from.x, p.y - from.y, p.z - from.z);
        let rsq = st((ld(r.z) * ld(r.z) + ld(r.y) * ld(r.y)) + ld(r.x) * ld(r.x));
        if ld(rsq) > max_rsq {
            max_rsq = ld(rsq);
        }
        offsets.push(r);
    }
    let max_r = fsqrt(max_rsq);
    let (shrink_min, shrink_max, dist_max) = (ld(0.1f32), 1.0, 5.0);
    let scale = if ld(dmag) < max_r {
        shrink_min
    } else {
        let d_over_r = ld(dmag) / max_r;
        if d_over_r < dist_max {
            ((d_over_r - 1.0) * (shrink_max - shrink_min)) / (dist_max - 1.0) + shrink_min
        } else {
            shrink_max
        }
    };
    if !eq_or_unordered(scale, 1.0) {
        for o in &mut offsets {
            *o = Vector::new(st(scale * ld(o.x)), st(scale * ld(o.y)), st(scale * ld(o.z)));
        }
    }
    offsets
}

/// `CalculateMoveToPoints` (0x4416c0): each ship's `move_to` is `to` plus its scaled offset.
pub fn calculate_move_to_points(u: &mut Universe, sel: &[ShipId], from: Vector, to: Vector) {
    move_to_points(&mut u.ships, sel, from, to);
}

fn move_to_points(ships: &mut ShipTable, sel: &[ShipId], from: Vector, to: Vector) {
    let offsets = move_offsets(ships, sel, from, to);
    if sel.len() == 1 {
        ships.ship_mut(sel[0]).move_to = to;
        return;
    }
    for (&id, o) in sel.iter().zip(offsets) {
        ships.ship_mut(id).move_to = to + o;
    }
}

/// `InitShipAI(ship, fresh)`.
fn init_ship_ai(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.ai_state = 0;
    s.ai_state_attack = 0;
    s.ai_state_command = 0;
}

/// `PrepareShipsForCommand`.
fn prepare_ships(u: &mut Universe, cid: CommandId) {
    let sel = u.commands.get(cid).map(|c| c.selection.clone()).unwrap_or_default();
    for id in sel {
        let s = u.ships.ship_mut(id);
        s.command = Some(cid);
        s.special_flags &= !special::ROW_GETTING_OUT_OF_WAY;
    }
}

/// `RemoveShipsFromDoingStuff` for the orders this slice creates (null, move, halt and
/// formations).
pub fn remove_ships_from_doing_stuff(u: &mut Universe, ships: &[ShipId]) {
    for &id in ships {
        u.ships.ship_mut(id).special_flags &= !special::ROW_GETTING_OUT_OF_WAY;
    }
    let todo = u.commands.todo.clone();
    for cid in todo {
        for &id in ships {
            let Some(cmd) = u.commands.get_mut(cid) else { break };
            if cmd.attributes & attr::FORMATION != 0 {
                let Some(index) = remove_from_formation(cmd, id) else { continue };
                u.ships.ship_mut(id).command = None;
                let cmd = u.commands.get(cid).expect("command");
                if cmd.selection.len() < 2 {
                    for &s in &cmd.selection.clone() {
                        u.ships.ship_mut(s).command = None;
                    }
                    u.commands.delete(cid);
                    break;
                }
                formation::set_travel_velocity(u, cid);
                let t = u.commands.get(cid).expect("command").formation.formation_type;
                if t == formation::kind::CUSTOM {
                    if index == 0 {
                        formation::calculate_offsets(u, cid);
                    }
                } else if t != formation::kind::WALL && t != formation::kind::SPHERE {
                    formation::calculate_offsets(u, cid);
                }
            } else if remove_from_selection(&mut cmd.selection, id) {
                u.ships.ship_mut(id).command = None;
                if u.commands.get(cid).expect("command").selection.is_empty() {
                    u.commands.delete(cid);
                    break;
                }
            }
        }
    }
}

/// `formationRemoveShipFromSelection`: wings close up behind the removed ship.
fn remove_from_formation(cmd: &mut CommandToDo, id: ShipId) -> Option<usize> {
    let sel = &mut cmd.selection;
    let i = sel.iter().position(|&s| s == id)?;
    let n = sel.len();
    match cmd.formation.formation_type {
        formation::kind::DELTA | formation::kind::BROAD if n > 2 => {
            let mut j = i;
            while j < n {
                if j + 2 < n {
                    sel[j] = sel[j + 2];
                } else if j + 1 < n {
                    sel[j] = sel[j + 1];
                }
                j += 2;
            }
            sel.truncate(n - 1);
        }
        formation::kind::DELTA3D | formation::kind::CLAW => {
            let mut j = i;
            while j < n {
                for k in (1..=4).rev() {
                    if j + k < n {
                        sel[j] = sel[j + k];
                        break;
                    }
                }
                j += 4;
            }
            sel.truncate(n - 1);
        }
        _ => {
            sel.swap_remove(i);
        }
    }
    Some(i)
}

/// `clMove` (0x442860): order `selection` to fly from `from` through `waypoints`. Ships already
/// in a command that the selection covers completely keep it (so formations keep flying in
/// formation); the rest get new move commands.
pub fn cl_move(u: &mut Universe, selection: &[ShipId], mode: u32, from: Vector, waypoints: &[Vector]) {
    if selection.is_empty() || waypoints.is_empty() {
        return;
    }
    let waypoints = &waypoints[..waypoints.len().min(MAX_WAYPOINTS)];
    let mut copy: Vec<ShipId> = selection.to_vec();
    let todo = u.commands.todo.clone();
    for cid in todo {
        let Some(cmd) = u.commands.get(cid) else { continue };
        let csel = cmd.selection.clone();
        if !csel.is_empty() && csel.iter().all(|s| copy.contains(s)) {
            for s in &csel {
                remove_from_selection(&mut copy, *s);
            }
            if csel.len() == 1 {
                let offsets = move_offsets(&u.ships, selection, from, waypoints[0]);
                let idx = selection.iter().position(|&s| s == csel[0]).unwrap_or(0);
                let o = offsets[idx];
                let wps: Vec<Vector> = waypoints.iter().map(|w| *w + o).collect();
                change_order_to_move(u, cid, mode, from + o, &wps);
            } else {
                change_order_to_move(u, cid, mode, from, waypoints);
            }
        }
        if copy.is_empty() {
            return;
        }
    }
    if !copy.is_empty() {
        cl_move_these(u, &copy, mode, from, waypoints, false);
    }
}

/// `clMoveThese` (0x442290).
pub fn cl_move_these(u: &mut Universe, selection: &[ShipId], mode: u32, from: Vector, waypoints: &[Vector], skip_checks: bool) {
    if selection.is_empty() {
        return;
    }
    if !skip_checks {
        let found = u.commands.iter().find(|c| selections_equivalent(selection, &c.selection)).map(|c| c.id);
        if let Some(cid) = found {
            change_order_to_move(u, cid, mode, from, waypoints);
            return;
        }
        // (Ships attacking with a single waypoint keep attacking while moving; attacks are not
        // modelled.)
    }
    remove_ships_from_doing_stuff(u, selection);
    if selection.len() < 2 {
        let mut cmd = new_command(selection.to_vec());
        cmd.order = order::MOVE;
        cmd.waypoint_mode = mode;
        cmd.waypoints = waypoints.to_vec();
        cmd.waypoint_index = 0;
        let cid = u.commands.add(cmd);
        for &id in selection {
            init_ship_ai(u, id);
        }
        calculate_move_to_points(u, selection, from, waypoints[0]);
        for &id in selection {
            let s = u.ships.ship_mut(id);
            s.move_from = s.posinfo.position;
        }
        prepare_ships(u, cid);
    } else {
        let offsets = move_offsets(&u.ships, selection, from, waypoints[0]);
        for (&id, o) in selection.iter().zip(offsets) {
            let wps: Vec<Vector> = waypoints.iter().map(|w| *w + o).collect();
            cl_move_these(u, &[id], mode, from, &wps, true);
        }
    }
}

/// `ChangeOrderToMove` (0x441ba0).
pub fn change_order_to_move(u: &mut Universe, cid: CommandId, mode: u32, from: Vector, waypoints: &[Vector]) {
    let Some(cmd) = u.commands.get_mut(cid) else { return };
    cmd.attributes &= !(attr::PROTECTING | attr::PASSIVE_ATTACKING);
    let is_formation = cmd.attributes & attr::FORMATION != 0;
    let sel = cmd.selection.clone();
    if sel.len() > 1 && !is_formation {
        // Split the group: every ship but the last gets its own command with shifted waypoints.
        let offsets = move_offsets(&u.ships, &sel, from, waypoints[0]);
        let mut k = 0;
        while u.commands.get(cid).map_or(0, |c| c.selection.len()) > 1 && k < offsets.len() {
            let first = u.commands.get(cid).expect("command").selection[0];
            let wps: Vec<Vector> = waypoints.iter().map(|w| *w + offsets[k]).collect();
            cl_move_these(u, &[first], mode, from, &wps, true);
            k += 1;
        }
    }
    let Some(cmd) = u.commands.get_mut(cid) else { return };
    if cmd.order != order::MOVE {
        cmd.order = order::MOVE;
    }
    cmd.waypoint_index = 0;
    cmd.waypoint_mode = mode;
    cmd.waypoints = waypoints[..waypoints.len().min(MAX_WAYPOINTS)].to_vec();
    let sel = cmd.selection.clone();
    if is_formation {
        init_ship_ai(u, sel[0]);
    } else {
        for &id in &sel {
            init_ship_ai(u, id);
        }
    }
    calculate_move_to_points(u, &sel, from, waypoints[0]);
    for &id in &sel {
        let s = u.ships.ship_mut(id);
        s.move_from = s.posinfo.position;
    }
    prepare_ships(u, cid);
}

/// `processMoveToDo` (0x43f780) for one ship of a move command. Returns true when the ship
/// has arrived at the last waypoint and stopped.
pub fn process_move_to_do(u: &mut Universe, cid: CommandId, passive_attacked: bool, index: usize, is_leader: bool) -> bool {
    let cmd = u.commands.get(cid).expect("command").clone();
    let id = cmd.selection[index];
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let ship = u.ships.ship(id).clone();
    let stat = statics.ship(ship.ship_type).expect("ship statics");
    let dont_rotate = if cmd.attributes & attr::ATTACKING_AND_MOVING != 0 {
        true
    } else {
        // (Sentinels in a force field always count as "don't rotate"; not modelled.)
        stat.rotate_to_retaliate && passive_attacked
    };
    let count = cmd.waypoints.len() as i32;
    let mut steady = false;
    match ship.ai_state_command {
        0 => {
            let tolerance = if ship.is_dodging || ld(u.total_time_elapsed) < ld(ship.dodge_time) {
                250.0
            } else if cmd.waypoint_mode == waypoint_mode::ONCE && cmd.waypoint_index == count - 1 {
                50.0
            } else {
                800.0
            };
            if aitrack::move_reached_destination(&ship, ship.move_to, tolerance) {
                let c = u.commands.get_mut(cid).expect("command");
                let (prev, next) = match c.waypoint_mode {
                    waypoint_mode::ONCE => {
                        c.waypoint_index += 1;
                        if c.waypoint_index < count {
                            (c.waypoint_index - 1, c.waypoint_index)
                        } else {
                            arrived(u, cid, id);
                            steady = true;
                            (0, 0)
                        }
                    }
                    waypoint_mode::LOOP => {
                        c.waypoint_index += 1;
                        if c.waypoint_index >= count {
                            c.waypoint_index = 0;
                            (count - 1, 0)
                        } else if c.waypoint_index == 0 {
                            (count - 1, 0)
                        } else {
                            (c.waypoint_index - 1, c.waypoint_index)
                        }
                    }
                    waypoint_mode::PATROL => {
                        c.waypoint_index += 1;
                        if c.waypoint_index >= count {
                            c.waypoint_mode = waypoint_mode::PATROL_BACK;
                            c.waypoint_index = count - 2;
                            (count - 1, c.waypoint_index)
                        } else {
                            (c.waypoint_index - 1, c.waypoint_index)
                        }
                    }
                    waypoint_mode::PATROL_BACK => {
                        let old = c.waypoint_index;
                        c.waypoint_index -= 1;
                        if c.waypoint_index < 0 {
                            c.waypoint_mode = waypoint_mode::PATROL;
                            c.waypoint_index = 1;
                            (0, 1)
                        } else {
                            (old, c.waypoint_index)
                        }
                    }
                    _ => return false,
                };
                if !steady {
                    let c = u.commands.get(cid).expect("command");
                    let (sel, wps) = (c.selection.clone(), c.waypoints.clone());
                    let w = |i: i32| wps[i.clamp(0, count - 1) as usize];
                    calculate_move_to_points(u, &sel, w(prev), w(next));
                    return false;
                }
            } else {
                let limit = if is_leader { cmd.formation.travel_vel } else { -20.0 };
                let fl = if dont_rotate {
                    fly::DONT_FLY_TO_OBSCURED_POINTS | fly::RETURN_IMMED_IF_POINT_OBSCURED
                } else {
                    fly::DONT_FLY_TO_OBSCURED_POINTS
                        | fly::RETURN_IMMED_IF_POINT_OBSCURED
                        | fly::FIRST_POINT_IN_DIRECTION_FLYING
                        | fly::POINT_IN_DIRECTION_FLYING
                        | fly::CAR_TURN
                };
                if aiship::fly_to_point_avoiding_objs(u, id, Some(ship.move_to), fl, limit, None) & aiship::FLY_OBJECT_IN_WAY == 0 {
                    return false;
                }
                arrived(u, cid, id);
            }
        }
        1 => {}
        _ => return false,
    }
    let s = u.ships.ship_mut(id);
    let done = aitrack::steady_ship_drift_only(s, stat, &tw, dt);
    let c = u.commands.get(cid).expect("command");
    done && c.waypoint_mode == waypoint_mode::ONCE && c.waypoint_index >= count - 1
}

/// A ship of a move command reached its last waypoint (or found it obscured).
fn arrived(u: &mut Universe, cid: CommandId, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.ai_state_command = 1;
    s.move_to = Vector::ZERO;
    s.move_from = Vector::ZERO;
    let c = u.commands.get(cid).expect("command").clone();
    if c.attributes & attr::ATTACKING_AND_MOVING != 0 {
        for &o in c.selection.iter().skip(1) {
            let s = u.ships.ship_mut(o);
            s.move_to = Vector::ZERO;
            s.move_from = Vector::ZERO;
        }
    }
}

/// `MajorityShipsAreDoing` for moves: the command most of the selection is moving with.
fn majority_moving(u: &Universe, selection: &[ShipId]) -> Option<CommandId> {
    let mut moving = 0;
    let mut freq: Vec<(CommandId, usize)> = Vec::new();
    for &id in selection {
        if let Some(cid) = u.ships.ship(id).command {
            if let Some(c) = u.commands.get(cid) {
                if c.order == order::ATTACK {
                    return None;
                }
                if c.order == order::MOVE {
                    moving += 1;
                    match freq.iter_mut().find(|(c, _)| *c == cid) {
                        Some(f) => f.1 += 1,
                        None => freq.push((cid, 1)),
                    }
                }
            }
        }
    }
    if moving <= selection.len() - moving {
        return None;
    }
    let mut best: Option<(CommandId, usize)> = None;
    for (c, f) in freq {
        if best.is_none_or(|b| f > b.1) {
            best = Some((c, f));
        }
    }
    best.map(|b| b.0)
}

/// `clFormation` (0x480380): put the selection into a formation (or change its type).
pub fn cl_formation(u: &mut Universe, selection: &[ShipId], formation_type: u32) {
    if selection.is_empty() {
        return;
    }
    let existing = u.commands.iter().find(|c| selections_equivalent(selection, &c.selection)).map(|c| c.id);
    if let Some(cid) = existing {
        let c = u.commands.get_mut(cid).expect("command");
        if c.attributes & attr::FORMATION != 0 {
            if c.formation.formation_type == formation_type && c.formation.variant == 0 {
                return;
            }
            c.formation.variant = 0;
            c.formation.formation_type = formation_type;
            formation::type_has_changed(u, cid);
            prepare_ships(u, cid);
            return;
        }
        if c.order == order::MOVE || c.attributes & attr::PROTECTING != 0 || c.order == order::ATTACK {
            c.attributes |= attr::FORMATION;
            formation::fill_in_specifics(u, cid, formation_type, 0, true);
            prepare_ships(u, cid);
            return;
        }
    }
    // A new formation command; it joins the move most of the ships were on.
    let majority = majority_moving(u, selection).and_then(|c| u.commands.get(c).cloned());
    remove_ships_from_doing_stuff(u, selection);
    let mut cmd = new_command(selection.to_vec());
    cmd.attributes = attr::FORMATION;
    let cid = u.commands.add(cmd);
    for &id in selection {
        u.ships.ship_mut(id).command = Some(cid);
    }
    formation::fill_in_specifics(u, cid, formation_type, 0, true);
    let sel = u.commands.get(cid).expect("command").selection.clone();
    for &id in &sel {
        init_ship_ai(u, id);
    }
    prepare_ships(u, cid);
    if let Some(m) = majority {
        let i = if m.waypoint_index - 1 < 0 { m.waypoints.len() as i32 - 1 } else { m.waypoint_index - 1 };
        let from = m.waypoints[i.max(0) as usize];
        change_order_to_move(u, cid, m.waypoint_mode, from, &m.waypoints);
    }
}

/// `clHalt`: stop the selection where it is (the command is replaced by a halt order that
/// ends once the ships are steady).
pub fn cl_halt(u: &mut Universe, selection: &[ShipId]) {
    if selection.is_empty() {
        return;
    }
    let existing = u.commands.iter().find(|c| selections_equivalent(selection, &c.selection)).map(|c| c.id);
    if let Some(cid) = existing {
        let c = u.commands.get_mut(cid).expect("command");
        c.order = order::HALT;
        prepare_ships(u, cid);
        return;
    }
    remove_ships_from_doing_stuff(u, selection);
    let mut cmd = new_command(selection.to_vec());
    cmd.order = order::HALT;
    let cid = u.commands.add(cmd);
    for &id in selection {
        init_ship_ai(u, id);
    }
    prepare_ships(u, cid);
}

/// `clProcess` (0x447400) for the orders of this slice.
pub fn cl_process(u: &mut Universe) {
    let todo = u.commands.todo.clone();
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    for cid in todo {
        let Some(cmd) = u.commands.get(cid).cloned() else { continue };
        // Commands with crazy or dead ships are skipped this frame.
        if cmd.selection.iter().any(|&id| u.ships.ship(id).flags & (sof::CRAZY | sof::DEAD) != 0) {
            continue;
        }
        for &id in &cmd.selection {
            u.ships.ship_mut(id).autostabilize_ship = false;
        }
        if cmd.attributes & attr::FORMATION != 0 {
            if !formation::is_supported(&cmd) {
                continue;
            }
            match cmd.order {
                order::NULL => {
                    if cmd.attributes & attr::PROTECTING == 0 {
                        if cmd.attributes & attr::HOLDING_PATTERN == 0 {
                            let leader = cmd.selection[0];
                            let stat = statics.ship(u.ships.ship(leader).ship_type).expect("statics");
                            aitrack::steady_ship(u.ships.ship_mut(leader), stat, &tw, dt);
                        }
                        let steady_formation = cmd.attributes & attr::HOLDING_PATTERN == 0;
                        formation::process(u, cid, steady_formation, false);
                    }
                }
                order::MOVE => {
                    if process_move_to_do(u, cid, false, 0, true) {
                        let c = u.commands.get_mut(cid).expect("command");
                        c.order = order::NULL;
                    }
                    formation::process(u, cid, false, false);
                }
                order::HALT => {
                    let leader = cmd.selection[0];
                    let stat = statics.ship(u.ships.ship(leader).ship_type).expect("statics");
                    let done = aitrack::steady_ship(u.ships.ship_mut(leader), stat, &tw, dt);
                    formation::process(u, cid, true, false);
                    if done {
                        u.commands.get_mut(cid).expect("command").order = order::NULL;
                    }
                }
                _ => {}
            }
            continue;
        }
        match cmd.order {
            order::NULL => {
                if cmd.attributes == 0 {
                    for &id in &cmd.selection {
                        u.ships.ship_mut(id).command = None;
                    }
                    u.commands.delete(cid);
                }
            }
            order::MOVE => {
                let mut all_done = true;
                for i in 0..cmd.selection.len() {
                    all_done &= process_move_to_do(u, cid, false, i, false);
                }
                if all_done {
                    if cmd.attributes & (attr::PROTECTING | attr::PASSIVE_ATTACKING) == 0 {
                        for &id in &cmd.selection {
                            u.ships.ship_mut(id).command = None;
                        }
                        u.commands.delete(cid);
                    } else {
                        u.commands.get_mut(cid).expect("command").order = order::NULL;
                    }
                }
            }
            order::HALT => {
                let mut all = true;
                for &id in &cmd.selection {
                    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics");
                    all &= aitrack::steady_ship(u.ships.ship_mut(id), stat, &tw, dt);
                }
                if all {
                    if cmd.attributes == 0 {
                        for &id in &cmd.selection {
                            u.ships.ship_mut(id).command = None;
                        }
                        u.commands.delete(cid);
                    } else {
                        u.commands.get_mut(cid).expect("command").order = order::NULL;
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::Ship;
    use crate::statics::{ShipRace, ShipType};

    type Row = (&'static [[f32; 3]], [f32; 3], [f32; 3], &'static [[u32; 3]]);

    /// Ship positions, from, to and the resulting `moveTo` of emulating `CalculateMoveToPoints`
    /// (0x4416c0) in cataclysm.exe (with the `fmathSqrtDouble` table, which the game builds at
    /// start-up, written into the emulator's memory).
    const ROWS: &[Row] = &[
        (&[[f32::from_bits(0xc35df4b3), f32::from_bits(0xc43e0838), f32::from_bits(0xc5078c37)]], [f32::from_bits(0x43b747eb), f32::from_bits(0xc3f6c850), f32::from_bits(0x40320d99)], [f32::from_bits(0x44876ff1), f32::from_bits(0xc49c0328), f32::from_bits(0x42c8f017)], &[[0x44876ff1, 0xc49c0328, 0x42c8f017]]),
        (&[[f32::from_bits(0x442ef99e), f32::from_bits(0xc52c2a02), f32::from_bits(0xc4357879)], [f32::from_bits(0x44989c3c), f32::from_bits(0xc38fefef), f32::from_bits(0x44a8cc8d)]], [f32::from_bits(0xc3ab6be2), f32::from_bits(0xc382fe70), f32::from_bits(0xc3c286b7)], [f32::from_bits(0xc385ce9f), f32::from_bits(0x4596bfc2), f32::from_bits(0x442e05b3)], &[[0x4248c658, 0x457e0431, 0x44145b82], [0x4350fa9a, 0x459680a4, 0x4499479e]]),
        (&[[f32::from_bits(0x44cda838), f32::from_bits(0xc42e80b3), f32::from_bits(0x44b89248)], [f32::from_bits(0xc5155fbc), f32::from_bits(0xc49c9dcf), f32::from_bits(0x4482ad50)], [f32::from_bits(0x44a9479f), f32::from_bits(0xc3eabbdb), f32::from_bits(0xc51a9b9c)]], [f32::from_bits(0xc3694435), f32::from_bits(0xc3910e10), f32::from_bits(0xc35ad0ca)], [f32::from_bits(0x45e0d7ec), f32::from_bits(0xc5ea741a), f32::from_bits(0x460d7b1d)], &[[0x460dc60e, 0xc5f7334f, 0x4627f8a9], [0x459d7230, 0xc6044556, 0x46213c0a], [0x460939fb, 0xc5f00ef7, 0x45d47ef2]]),
        (&[[f32::from_bits(0x450e43d3), f32::from_bits(0xc526f439), f32::from_bits(0xc435c680)], [f32::from_bits(0xc246eb17), f32::from_bits(0xc532b19e), f32::from_bits(0xc3e1d2ee)], [f32::from_bits(0x4518676c), f32::from_bits(0xc5117b8f), f32::from_bits(0x441144ba)], [f32::from_bits(0xc50e09b0), f32::from_bits(0x43ec19d4), f32::from_bits(0x45143d22)], [f32::from_bits(0xc4deb5c9), f32::from_bits(0xc53867c1), f32::from_bits(0xc51c2fa8)]], [f32::from_bits(0x421f13e8), f32::from_bits(0xc3f14480), f32::from_bits(0xc3cf94ec)], [f32::from_bits(0xc3082e78), f32::from_bits(0x46adcef9), f32::from_bits(0xc593cae6)], &[[0x4503449c, 0x469cb584, 0xc59d8a67], [0xc361ae38, 0x469b3dd7, 0xc594eec6], [0x450d6834, 0x469f6499, 0xc5695200], [0xc51908e8, 0x46b54472, 0xc4f2cc18], [0xc4f4b437, 0x469a8713, 0xc5d4e96b]]),
        (&[[f32::from_bits(0xc418cc3a), f32::from_bits(0x44501389), f32::from_bits(0xc51877e4)], [f32::from_bits(0x43ef6684), f32::from_bits(0xc4f59567), f32::from_bits(0x44235520)], [f32::from_bits(0x452bdf4a), f32::from_bits(0xc5272f5f), f32::from_bits(0x43a52ea2)], [f32::from_bits(0x441f923c), f32::from_bits(0xc50382c5), f32::from_bits(0xc4adc460)], [f32::from_bits(0x453994db), f32::from_bits(0x453abc92), f32::from_bits(0xc50dffc8)], [f32::from_bits(0x449a19e8), f32::from_bits(0x45291896), f32::from_bits(0xc4c56912)], [f32::from_bits(0x4426b0f1), f32::from_bits(0xc52b5d0c), f32::from_bits(0xc4491468)]], [f32::from_bits(0x432e1fef), f32::from_bits(0x42b484a1), f32::from_bits(0x43895000)], [f32::from_bits(0xc55d9392), f32::from_bits(0xc4a09ea5), f32::from_bits(0x455def31)], &[[0xc5635d8d, 0xc495adfd, 0x4549ed56], [0xc55b549c, 0xc4beea6b, 0x4560b9da], [0xc54a96ae, 0xc4c9633a, 0x455e5861], [0xc55a27a5, 0xc4c0f8f1, 0x4551a998], [0xc548f8be, 0xc46bcd6a, 0x454b2971], [0xc555c5b6, 0xc4741ff6, 0x455044a7], [0xc559f1e7, 0xc4ca5f8d, 0x4555fb19]]),
        (&[[f32::from_bits(0x43fc6b63), f32::from_bits(0xc3921999), f32::from_bits(0xc412be9b)], [f32::from_bits(0x453646e1), f32::from_bits(0x43df4eca), f32::from_bits(0xc5349cce)], [f32::from_bits(0x44e08703), f32::from_bits(0xc4807744), f32::from_bits(0xc3c74853)], [f32::from_bits(0xc4d6edfa), f32::from_bits(0xc3a7f4e3), f32::from_bits(0xc48377c4)]], [f32::from_bits(0xc3cd977b), f32::from_bits(0x4301829a), f32::from_bits(0xc3c6771d)], [f32::from_bits(0x4619617d), f32::from_bits(0xc68478d2), f32::from_bits(0x4617b426)], &[[0x4627b194, 0xc687c43e, 0x4614bbf5], [0x464d5ff1, 0xc681fe9c, 0x45e18157], [0x463bdf19, 0xc68d834b, 0x4617ad9c], [0x4604f07a, 0xc6881bab, 0x460d78e6]]),
    ];

    #[test]
    fn move_to_points_match_emulated_binary() {
        let v = |a: [f32; 3]| Vector::new(a[0], a[1], a[2]);
        for (i, (pos, from, to, expected)) in ROWS.iter().enumerate() {
            let mut ships = ShipTable::default();
            let mut sel = Vec::new();
            for (k, p) in pos.iter().enumerate() {
                let mut s = Ship::new(ShipId(k as u16), ShipType::sAcolyte, ShipRace::Somtaaw, 0);
                s.posinfo.position = v(*p);
                ships.insert(s);
                sel.push(ShipId(k as u16));
            }
            move_to_points(&mut ships, &sel, v(*from), v(*to));
            for (k, e) in expected.iter().enumerate() {
                let m = ships.ship(sel[k]).move_to;
                assert_eq!([m.x.to_bits(), m.y.to_bits(), m.z.to_bits()], *e, "row {i} ship {k}");
            }
        }
    }
}
