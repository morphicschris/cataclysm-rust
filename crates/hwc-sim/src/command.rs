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
    pub const LAUNCH_SHIP: u16 = 4;
    pub const COLLECT_RESOURCE: u16 = 5;
    pub const BUILDING_SHIP: u16 = 6;
    pub const SPECIAL: u16 = 7;
    pub const HALT: u16 = 8;
    pub const MILITARY_PARADE: u16 = 9;
    pub const LINK: u16 = 0xb;
    pub const UNLINK: u16 = 0xc;
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
    /// Targets of an attack or passive attack (+0x22c).
    pub attack_targets: Vec<ShipId>,
    /// +0x27a: a formation's attack has reached its targets (until then it flies to them).
    pub attack_in_range: bool,
    /// The move a command was carrying out when it was ordered to attack (+0x120, +0x124):
    /// waypoint mode, waypoints and index, resumed afterwards.
    pub resume_move: Option<(u32, Vec<Vector>, i32)>,
    /// Dock orders (+0x290..+0x29b).
    pub dock: DockInfo,
    /// Launch orders: the ship launched from (+0x29c).
    pub launch_from: Option<ShipId>,
    /// Resource collection (+0x2a0..+0x2b0).
    pub collect: CollectInfo,
    /// Special orders (salvage): the targets (+0x2bc).
    pub special_targets: Vec<ShipId>,
    /// Link orders: the partner ship.
    pub link_with: Option<ShipId>,
}

/// The dock part of a command.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DockInfo {
    /// +0x290: dock purpose (see [`crate::dock::purpose`]).
    pub purpose: i32,
    /// +0x298: go back to harvesting after docking.
    pub collect_after: bool,
    /// +0x299: every ship docks with the same ship.
    pub same_dockwith: bool,
}

/// The resource-collection part of a command.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollectInfo {
    /// +0x2a0: the resource being harvested.
    pub resource: Option<crate::resource::ResourceId>,
    /// +0x2a4: radius of the zone to look for resources in (0 = anywhere).
    pub zone_radius: f32,
    /// +0x2a8: centre of that zone.
    pub zone_centre: Vector,
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

    /// Appends a command (`listAddNode`).
    pub fn add_pub(&mut self, cmd: CommandToDo) -> CommandId {
        self.add(cmd)
    }

    /// Puts a command at the front of the to-do list (`listAddNodeBeginning`, used for
    /// launches).
    pub fn add_front_pub(&mut self, mut cmd: CommandToDo) -> CommandId {
        let id = CommandId(self.slots.len() as u32);
        cmd.id = id;
        self.slots.push(Some(cmd));
        self.todo.insert(0, id);
        id
    }

    /// Removes a command (`listDeleteNode`).
    pub fn delete_pub(&mut self, id: CommandId) {
        self.delete(id)
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
        attack_targets: Vec::new(),
        attack_in_range: false,
        resume_move: None,
        dock: DockInfo::default(),
        launch_from: None,
        collect: CollectInfo::default(),
        special_targets: Vec::new(),
        link_with: None,
    }
}

/// A fresh command for `selection` (no order yet).
pub fn new_command_pub(selection: Vec<ShipId>) -> CommandToDo {
    new_command(selection)
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
        aiship::row_remove_ship(s);
    }
}

/// `RemoveShipsFromDoingStuff` for the orders this slice creates (null, move, halt and
/// formations).
pub fn remove_ships_from_doing_stuff(u: &mut Universe, ships: &[ShipId]) {
    for &id in ships {
        aiship::row_remove_ship(u.ships.ship_mut(id));
    }
    let todo = u.commands.todo.clone();
    for cid in todo {
        for &id in ships {
            let Some(cmd) = u.commands.get_mut(cid) else { break };
            let attacking = cmd.order == order::ATTACK || cmd.attributes & attr::PASSIVE_ATTACKING != 0;
            let moving = cmd.order == order::MOVE;
            if cmd.attributes & attr::FORMATION != 0 {
                let Some(index) = remove_from_formation(cmd, id) else { continue };
                u.ships.ship_mut(id).command = None;
                let sel = u.commands.get(cid).expect("command").selection.clone();
                if attacking {
                    unpair(u, &sel, id);
                }
                if moving {
                    let s = u.ships.ship_mut(id);
                    s.move_to = Vector::ZERO;
                    s.move_from = Vector::ZERO;
                }
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
                let order_ = cmd.order;
                u.ships.ship_mut(id).command = None;
                let sel = cmd.selection.clone();
                if attacking {
                    unpair(u, &sel, id);
                }
                if moving {
                    let s = u.ships.ship_mut(id);
                    s.move_to = Vector::ZERO;
                    s.move_from = Vector::ZERO;
                }
                match order_ {
                    order::DOCK => crate::dock::remove_ship_from_docking(u, id),
                    order::LAUNCH_SHIP => crate::dock::remove_ship_from_launching(u, id),
                    order::COLLECT_RESOURCE => {
                        let s = u.ships.ship_mut(id);
                        s.base.harvest_effect = false;
                        s.base.harvest_state = 0;
                        s.base.harvest_state2 = 0;
                    }
                    order::SPECIAL => crate::salvage::special_done(u, id),
                    _ => {}
                }
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
    let Some(cmd) = u.commands.get(cid) else { return };
    let (was_attack, was_passive) = (cmd.order == order::ATTACK, cmd.attributes & attr::PASSIVE_ATTACKING != 0);
    if was_passive {
        clear_passive_attacking(u, cid);
    }
    if was_attack {
        free_last_order(u, cid);
    }
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

/// The end of an attack order: a command that was moving resumes its waypoints, the rest
/// fall idle.
fn finish_attack(u: &mut Universe, cid: CommandId) {
    let resume = u.commands.get_mut(cid).and_then(|c| c.resume_move.take());
    match resume {
        Some((mode, wps, index)) if !wps.is_empty() => {
            let lead = u.commands.get(cid).expect("command").selection[0];
            let from = u.ships.ship(lead).posinfo.position;
            let i = (index.max(0) as usize).min(wps.len() - 1);
            free_last_order(u, cid);
            change_order_to_move(u, cid, mode, from, &wps[i..]);
        }
        _ => {
            free_last_order(u, cid);
            if let Some(c) = u.commands.get_mut(cid) {
                c.order = order::NULL;
            }
        }
    }
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
            let passive = cmd.attributes & attr::PASSIVE_ATTACKING != 0;
            match cmd.order {
                order::NULL => {
                    if cmd.attributes & attr::PROTECTING == 0 {
                        run_passive_attack(u, cid);
                        let attrs = u.commands.get(cid).map_or(0, |c| c.attributes);
                        if attrs & attr::HOLDING_PATTERN == 0 {
                            let leader = cmd.selection[0];
                            let stat = statics.ship(u.ships.ship(leader).ship_type).expect("statics");
                            if passive && stat.rotate_to_retaliate {
                                aitrack::steady_ship_drift_only(u.ships.ship_mut(leader), stat, &tw, dt);
                            } else {
                                aitrack::steady_ship(u.ships.ship_mut(leader), stat, &tw, dt);
                            }
                        }
                        if passive {
                            formation::process(u, cid, false, true);
                        } else {
                            formation::process(u, cid, attrs & attr::HOLDING_PATTERN == 0, false);
                        }
                    }
                }
                order::MOVE => {
                    run_passive_attack(u, cid);
                    if process_move_to_do(u, cid, false, 0, true) {
                        free_last_order(u, cid);
                        let c = u.commands.get_mut(cid).expect("command");
                        c.order = order::NULL;
                    }
                    formation::process(u, cid, false, false);
                }
                order::ATTACK => {
                    // (Formations attack ship by ship here; the in-formation attack of
                    // holdFormationDuringBattle is not modelled.)
                    if process_attack_to_do(u, cid) {
                        finish_attack(u, cid);
                    }
                }
                order::DOCK => {
                    if crate::dock::process_dock_to_do(u, cid) {
                        let empty = u.commands.get(cid).is_none_or(|c| c.selection.is_empty());
                        if empty {
                            delete_command(u, cid);
                        } else {
                            free_last_order(u, cid);
                            u.commands.get_mut(cid).expect("command").order = order::NULL;
                        }
                    }
                }
                order::LINK => {
                    if crate::link::process_link_to_do(u, cid) {
                        delete_command(u, cid);
                    }
                }
                order::UNLINK => {
                    if crate::link::process_unlink_to_do(u, cid) {
                        delete_command(u, cid);
                    }
                }
                order::HALT => {
                    run_passive_attack(u, cid);
                    let leader = cmd.selection[0];
                    let stat = statics.ship(u.ships.ship(leader).ship_type).expect("statics");
                    let done = if passive && stat.rotate_to_retaliate {
                        aitrack::steady_ship_drift_only(u.ships.ship_mut(leader), stat, &tw, dt)
                    } else {
                        aitrack::steady_ship(u.ships.ship_mut(leader), stat, &tw, dt)
                    };
                    formation::process(u, cid, true, passive);
                    if done {
                        free_last_order(u, cid);
                        u.commands.get_mut(cid).expect("command").order = order::NULL;
                    }
                }
                _ => {}
            }
            continue;
        }
        match cmd.order {
            order::NULL => {
                if cmd.attributes & attr::HOLDING_PATTERN != 0 {
                    // (clHoldingPattern is not modelled: the ships just stop.)
                    u.commands.get_mut(cid).expect("command").attributes &= !attr::HOLDING_PATTERN;
                }
                let attrs = u.commands.get(cid).map_or(0, |c| c.attributes);
                if attrs == 0 {
                    for &id in &cmd.selection {
                        u.ships.ship_mut(id).command = None;
                    }
                    u.commands.delete(cid);
                } else if attrs & attr::PROTECTING == 0 && attrs & attr::PASSIVE_ATTACKING != 0 {
                    run_passive_attack(u, cid);
                    let leader = cmd.selection[0];
                    let stat = statics.ship(u.ships.ship(leader).ship_type).expect("statics");
                    if !stat.rotate_to_retaliate {
                        aitrack::steady_ship(u.ships.ship_mut(leader), stat, &tw, dt);
                    } else if u.ships.ship(leader).tactics != crate::object::tactics::EVASIVE {
                        aitrack::steady_ship_drift_only(u.ships.ship_mut(leader), stat, &tw, dt);
                    }
                }
            }
            order::ATTACK => {
                if process_attack_to_do(u, cid) {
                    finish_attack(u, cid);
                }
            }
            order::MOVE => {
                run_passive_attack(u, cid);
                let cmd = u.commands.get(cid).expect("command").clone();
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
            order::DOCK => {
                if crate::dock::process_dock_to_do(u, cid) {
                    let collect_after = u.commands.get(cid).is_some_and(|c| c.dock.collect_after);
                    if !collect_after {
                        delete_command(u, cid);
                    } else {
                        free_last_order(u, cid);
                        crate::resource::change_single_ship_to_collect_resource(u, cid, None);
                    }
                }
            }
            order::LAUNCH_SHIP => {
                if crate::dock::process_launch_ship_to_do(u, cid) {
                    // (The launched ship would join the host's formation or military parade.)
                    delete_command(u, cid);
                }
            }
            order::COLLECT_RESOURCE => {
                if crate::resource::process_collect_resource(u, cid) {
                    // (CreateMilitaryGroupAroundShip: the harvester would join the parade.)
                    delete_command(u, cid);
                }
            }
            order::SPECIAL => {
                if crate::salvage::process_special_to_do(u, cid) {
                    delete_command(u, cid);
                }
            }
            order::LINK => {
                if crate::link::process_link_to_do(u, cid) {
                    delete_command(u, cid);
                }
            }
            order::UNLINK => {
                if crate::link::process_unlink_to_do(u, cid) {
                    delete_command(u, cid);
                }
            }
            order::HALT => {
                run_passive_attack(u, cid);
                let cmd = u.commands.get(cid).expect("command").clone();
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

// --- attack orders

/// `RemoveShipFromAttacking`: the ship drops its targets, pairing and manoeuvre.
pub fn remove_ship_from_attacking(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get_mut(id) else { return };
    s.attack.multiple_targets = None;
    s.attack.target = None;
    s.attack.leader = None;
    s.attack.wingman = None;
    s.ship_is_attacking = false;
    s.ship_is_attacking2 = false;
    s.special_flags &= !(special::ATTACKING_FROM_ABOVE | special::KAMIKAZE | special::ATTACK_SIDESTEP);
    if s.flightman != 0 {
        crate::flightman::close(u, id);
    }
}

/// The pairing bookkeeping when `gone` leaves an attacking command: its leader and wingman
/// learn about it.
fn unpair(u: &mut Universe, sel: &[ShipId], gone: ShipId) {
    remove_ship_from_attacking(u, gone);
    for &o in sel {
        if o == gone {
            continue;
        }
        let s = u.ships.ship_mut(o);
        if s.attack.leader == Some(gone) {
            s.attack.events |= crate::object::attack_event::LEADER_DIED;
            s.attack.leader = None;
        }
        if s.attack.wingman == Some(gone) {
            s.attack.events |= crate::object::attack_event::WINGMAN_DIED;
            s.attack.wingman = None;
        }
    }
}

/// `AttackCleanup`.
fn attack_cleanup(u: &mut Universe, cid: CommandId) {
    let Some(cmd) = u.commands.get(cid).cloned() else { return };
    if let Some(c) = u.commands.get_mut(cid) {
        c.attributes &= !attr::ATTACKING_AND_MOVING;
    }
    for &id in &cmd.selection {
        remove_ship_from_attacking(u, id);
        let s = u.ships.ship_mut(id);
        if cmd.order != order::MOVE {
            s.move_to = Vector::ZERO;
            s.move_from = Vector::ZERO;
        }
        s.special_flags &= !special::ATTACK_SIDESTEP;
    }
}

/// `ClearPassiveAttacking`.
fn clear_passive_attacking(u: &mut Universe, cid: CommandId) {
    if let Some(c) = u.commands.get_mut(cid) {
        c.attributes &= !attr::PASSIVE_ATTACKING;
    }
    attack_cleanup(u, cid);
    if let Some(c) = u.commands.get_mut(cid) {
        c.attack_targets.clear();
    }
}

/// `FreeLastOrder` for the orders this layer has.
fn free_last_order(u: &mut Universe, cid: CommandId) {
    let Some(cmd) = u.commands.get(cid).cloned() else { return };
    match cmd.order {
        order::MOVE => {
            for &id in &cmd.selection {
                let s = u.ships.ship_mut(id);
                s.move_to = Vector::ZERO;
                s.move_from = Vector::ZERO;
            }
        }
        order::ATTACK => {
            attack_cleanup(u, cid);
            if let Some(c) = u.commands.get_mut(cid) {
                c.attack_targets.clear();
            }
        }
        order::DOCK => {
            // DockCleanup.
            for &id in &cmd.selection {
                crate::dock::remove_ship_from_docking(u, id);
            }
        }
        order::LAUNCH_SHIP => {
            // LaunchCleanup.
            if let Some(&id) = cmd.selection.first() {
                crate::dock::remove_ship_from_launching(u, id);
            }
        }
        order::COLLECT_RESOURCE => {
            for &id in &cmd.selection {
                let s = u.ships.ship_mut(id);
                s.base.harvest_effect = false;
                s.base.harvest_state = 0;
                s.base.harvest_state2 = 0;
            }
        }
        order::SPECIAL => {
            for &id in &cmd.selection {
                crate::salvage::special_done(u, id);
            }
            if let Some(c) = u.commands.get_mut(cid) {
                c.special_targets.clear();
            }
        }
        order::LINK | order::UNLINK => {
            for &id in &cmd.selection {
                crate::link::free_link_order(u, id);
            }
        }
        _ => {}
    }
    if let Some(c) = u.commands.get_mut(cid) {
        c.attributes &= !attr::HOLDING_PATTERN;
    }
}

/// `FreeCommandToDoContents` + `listDeleteNode`: the ships leave the command and it is
/// removed.
pub fn delete_command(u: &mut Universe, cid: CommandId) {
    let Some(cmd) = u.commands.get(cid).cloned() else { return };
    for &id in &cmd.selection {
        if let Some(s) = u.ships.get_mut(id) {
            if s.command == Some(cid) {
                s.command = None;
            }
        }
    }
    if cmd.attributes & attr::PASSIVE_ATTACKING != 0 || cmd.order == order::ATTACK {
        attack_cleanup(u, cid);
    }
    u.commands.delete(cid);
}

/// `FreeLastOrder` for other modules.
pub fn free_last_order_pub(u: &mut Universe, cid: CommandId) {
    free_last_order(u, cid);
}

/// `IsSelectionAlreadyDoingSomething`: the command whose selection is the same set of ships.
pub fn selection_already_doing_something(u: &Universe, sel: &[ShipId]) -> Option<CommandId> {
    u.commands.todo.iter().copied().find(|&c| u.commands.get(c).is_some_and(|cmd| selections_equivalent(sel, &cmd.selection)))
}

/// `ClearProtecting` and `ClearPassiveAttacking` when set.
pub fn clear_protecting_and_passive(u: &mut Universe, cid: CommandId) {
    let attrs = u.commands.get(cid).map_or(0, |c| c.attributes);
    if attrs & attr::PROTECTING != 0 {
        if let Some(c) = u.commands.get_mut(cid) {
            c.attributes &= !attr::PROTECTING;
        }
    }
    if attrs & attr::PASSIVE_ATTACKING != 0 {
        clear_passive_attacking(u, cid);
    }
}

/// `InitShipsForAI(selection, TRUE)`.
pub fn init_ships_for_ai(u: &mut Universe, sel: &[ShipId]) {
    for &id in sel {
        init_ship_ai(u, id);
    }
}

/// `InitShipAI(ship, TRUE)`.
pub fn init_ship_ai_pub(u: &mut Universe, id: ShipId) {
    init_ship_ai(u, id);
}

/// `PrepareShipsForCommand(command, TRUE)`.
pub fn prepare_ships_pub(u: &mut Universe, cid: CommandId) {
    prepare_ships(u, cid);
}

/// `clRemoveTargetFromSelection` on a command's selection; a formation recalculates its
/// offsets (`formationContentHasChanged`).
pub fn remove_ship_from_command_selection(u: &mut Universe, cid: CommandId, id: ShipId) {
    let Some(c) = u.commands.get_mut(cid) else { return };
    let formation_cmd = c.attributes & attr::FORMATION != 0;
    if formation_cmd {
        if remove_from_formation(c, id).is_some() && !c.selection.is_empty() {
            formation::calculate_offsets(u, cid);
        }
    } else {
        remove_from_selection(&mut c.selection, id);
    }
}

/// `RemoveShipFromFormation` + `formationContentHasChanged`.
pub fn remove_ship_from_formation_pub(u: &mut Universe, cid: CommandId, id: ShipId) {
    remove_ship_from_command_selection(u, cid, id);
}

/// `BreakupIntoLeaderWingman` (0x43c8b0): fighters are paired with the nearest unpaired
/// fighter within `wingmanJoinMaxDist`, at random by `FIGHTER_PAIRUP_PROB`.
fn breakup_into_leader_wingman(u: &mut Universe, sel: &[ShipId]) {
    let n = sel.len();
    let pairup = u.tweaks.combat.fighter_pairup_prob;
    let max = u.tweaks.combat.wingman_join_max_dist;
    let mut state = vec![2u32; n];
    for (i, &id) in sel.iter().enumerate() {
        let interceptor = u.combat.get(u.ships.ship(id).ship_type).is_some_and(|c| matches!(c.attack, crate::combat_statics::AttackKind::Interceptor));
        // (Ships in the "special" order class 4 are not paired either.)
        if interceptor && (u.randy.random(hwc_math::random::RAN_GAME) & 0xff) <= pairup {
            state[i] = 0;
        }
    }
    for i in 0..n {
        if state[i] != 0 {
            continue;
        }
        let a = u.ships.ship(sel[i]).posinfo.position;
        let mut best = f32::from_bits(0x6258_d727);
        let mut pick = 0usize;
        for j in i + 1..n {
            if state[j] != 0 {
                continue;
            }
            let b = u.ships.ship(sel[j]).posinfo.position;
            let d = Vector::new(st(ld(a.x) - ld(b.x)), st(ld(a.y) - ld(b.y)), st(ld(a.z) - ld(b.z)));
            let dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
            if dsq < max && dsq < best {
                pick = j;
                best = dsq;
            }
        }
        if pick != 0 {
            state[pick] |= 1;
            let (leader, wing) = (sel[i], sel[pick]);
            let l = u.ships.ship_mut(leader);
            l.attack.leader = None;
            l.attack.wingman = Some(wing);
            let w = u.ships.ship_mut(wing);
            w.attack.leader = Some(leader);
            w.attack.wingman = None;
        }
    }
}

/// `InitExtraAttackInfo` (0x43cad0): clears the ships' attack state, decides whether the group
/// attacks from above, pairs fighters (`pair_up`) and hands out first targets (nearest
/// unclaimed target per ship, round-robin; aggressive groups share the first ship's target).
fn init_extra_attack_info(u: &mut Universe, sel: &[ShipId], targets: &[ShipId], pair_up: bool) {
    let n = sel.len();
    let nt = targets.len();
    if n == 0 || nt == 0 {
        return;
    }
    let (mut sx, mut sy, mut sz) = (0f32, 0f32, 0f32);
    for &id in sel {
        let s = u.ships.ship_mut(id);
        let p = s.posinfo.position;
        sx = st(ld(sx) + ld(p.x));
        sy = st(ld(sy) + ld(p.y));
        sz = st(ld(sz) + ld(p.z));
        s.special_flags &= !(special::ATTACKING_FROM_ABOVE | special::KAMIKAZE | special::ATTACK_SIDESTEP);
        s.attack.multiple_targets = None;
        s.attack.target = None;
        s.attack.leader = None;
        s.attack.wingman = None;
        s.attack.flightmans_left = 0;
        s.attack.situation = 0;
        s.attack.events = 0;
        s.tactics_need_to_flightman = 0;
        s.tactics_talk = 0;
    }
    let (mut tx, mut ty, mut tz) = (0f32, 0f32, 0f32);
    for &t in targets {
        let p = u.ships.ship(t).posinfo.position;
        tx = st(ld(tx) + ld(p.x));
        ty = st(ld(ty) + ld(p.y));
        tz = st(ld(tz) + ld(p.z));
    }
    let a = st(1.0 / ld(n as i32 as f32));
    let b = st(1.0 / ld(nt as i32 as f32));
    let dx = st(ld(a) * ld(sx) - ld(b) * ld(tx));
    let dy = st(ld(a) * ld(sy) - ld(b) * ld(ty));
    let mut dz = st(ld(a) * ld(sz) - ld(b) * ld(tz));
    if dz < 0.0 {
        dz = -dz;
    }
    let tw = &u.tweaks.combat;
    if ld(tw.attacking_from_above_min_dist) <= ld(dz) && fsqrt(ld(dx) * ld(dx) + ld(dy) * ld(dy)) * ld(tw.attacking_from_above_min_ratio) <= ld(dz) {
        for &id in sel {
            u.ships.ship_mut(id).special_flags |= special::ATTACKING_FROM_ABOVE;
        }
    }
    if pair_up {
        breakup_into_leader_wingman(u, sel);
    }
    if nt == 1 {
        for &id in sel {
            let s = u.ships.ship_mut(id);
            if s.attack.leader.is_none() {
                s.attack.target = Some(targets[0]);
            }
        }
        return;
    }
    let first = sel[0];
    let aggressive = u.ships.ship(first).tactics == crate::object::tactics::AGGRESSIVE;
    let mut claimed = vec![false; nt];
    let mut count = 0;
    for (i, &id) in sel.iter().enumerate() {
        if u.ships.ship(id).attack.leader.is_some() {
            continue;
        }
        let p = u.ships.ship(id).posinfo.position;
        let mut best = f32::from_bits(0x6258_d727);
        let mut pick: i32 = -1;
        for (k, &t) in targets.iter().enumerate() {
            if claimed[k] {
                continue;
            }
            let q = u.ships.ship(t).posinfo.position;
            let d = Vector::new(st(ld(p.x) - ld(q.x)), st(ld(p.y) - ld(q.y)), st(ld(p.z) - ld(q.z)));
            let dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
            if dsq < best {
                pick = k as i32;
                best = dsq;
            }
        }
        let pick = pick.max(0) as usize;
        claimed[pick] = true;
        count += 1;
        if count == nt {
            claimed.iter_mut().for_each(|c| *c = false);
            count = 0;
        }
        u.ships.ship_mut(id).attack.target = Some(targets[pick]);
        if aggressive && i > 0 {
            if let Some(t) = u.ships.ship(first).attack.target {
                u.ships.ship_mut(id).attack.target = Some(t);
            }
        }
        let stat = u.statics.ship(u.ships.ship(id).ship_type).expect("statics");
        if stat.can_target_multiple_targets {
            let guns = u.ships.ship(id).guns.len();
            u.ships.ship_mut(id).attack.multiple_targets = Some(vec![None; guns]);
            pick_multiple_attack_targets(u, id, targets);
        }
    }
}

/// `pickMultipleAttackTargets` (0x443400): each gun takes the target in its range that needs
/// the least turning (spread out: a picked target counts 1.2 times further for the next gun).
fn pick_multiple_attack_targets(u: &mut Universe, id: ShipId, targets: &[ShipId]) {
    let statics = u.statics.clone();
    let s = u.ships.ship(id).clone();
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let Some(guns) = stat.guns.as_ref() else { return };
    let mut local: Vec<(Vector, f32)> = targets
        .iter()
        .map(|&t| (s.rotinfo.coordsys.vec_mul(aiship::trajectory(&s, u.ships.ship(t))), 1.0f32))
        .collect();
    let mut picks = vec![None; guns.len()];
    for (gi, gs) in guns.iter().enumerate() {
        let mut best = f32::from_bits(0x6258_d727);
        let mut pick: Option<usize> = None;
        for (k, &t) in targets.iter().enumerate() {
            let (lv, weight) = local[k];
            let d = Vector::new(st(ld(lv.x) - ld(gs.position.x)), st(ld(lv.y) - ld(gs.position.y)), st(ld(lv.z) - ld(gs.position.z)));
            let dist = st(fsqrt((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z)));
            let tstat = statics.ship(u.ships.ship(t).ship_type).expect("statics");
            if aiship::range_given_dist(tstat, ld(dist)) < ld(gs.bullet_range) {
                let inv = 1.0 / ld(dist);
                let n = gs.gun_normal;
                let cos = (inv * ld(d.x) * ld(n.x) + inv * ld(d.y) * ld(n.y)) + inv * ld(d.z) * ld(n.z);
                let score = st((2.0 - cos) * ld(dist) * ld(weight));
                if score < best {
                    pick = Some(k);
                    best = score;
                }
            }
        }
        if let Some(k) = pick {
            picks[gi] = Some(targets[k]);
            local[k].1 = st(ld(local[k].1) * ld(1.2f32));
        }
    }
    // (Frigates attacking a corvette also consider a second target; not modelled.)
    u.ships.ship_mut(id).attack.multiple_targets = Some(picks);
}

/// The target filter of the attack orders (0x443dc0): ships pass, motherships stand for their
/// attached components (none exist in this slice).
fn attack_target_list(targets: &[ShipId]) -> Vec<ShipId> {
    targets.to_vec()
}

/// Ships in `sel` that have an attack routine (`MakeShipsAttackCapable`).
pub fn attack_capable(u: &Universe, sel: &[ShipId]) -> Vec<ShipId> {
    sel.iter().copied().filter(|&id| crate::attack::can_attack(u, id)).collect()
}

/// `clAttack` (0x4444b0): order `selection` to attack `targets`. Commands covered by the
/// selection change their order (a moving command remembers its waypoints and resumes them
/// afterwards); the rest get a new command.
pub fn cl_attack(u: &mut Universe, selection: &[ShipId], targets: &[ShipId]) {
    let mut sel: Vec<ShipId> = selection.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.coll_my_blob.is_some() && s.flags & sof::HIDE == 0)).collect();
    sel = attack_capable(u, &sel);
    if targets.is_empty() || sel.is_empty() {
        return;
    }
    let todo = u.commands.todo.clone();
    for cid in todo {
        let Some(cmd) = u.commands.get(cid) else { continue };
        let csel = cmd.selection.clone();
        if !csel.is_empty() && csel.iter().all(|s| sel.contains(s)) {
            for s in &csel {
                remove_from_selection(&mut sel, *s);
            }
            let c = u.commands.get_mut(cid).expect("command");
            c.attributes &= !attr::PROTECTING;
            if c.order == order::MOVE && !c.waypoints.is_empty() {
                c.resume_move = Some((c.waypoint_mode, c.waypoints.clone(), c.waypoint_index));
            }
            change_order_to_attack(u, cid, targets);
        }
        if sel.is_empty() {
            break;
        }
    }
    if !sel.is_empty() {
        cl_attack_these(u, &sel, targets);
    }
}

/// `clAttackThese` (0x444110).
fn cl_attack_these(u: &mut Universe, sel: &[ShipId], targets: &[ShipId]) {
    let found = u.commands.iter().find(|c| selections_equivalent(sel, &c.selection)).map(|c| c.id);
    if let Some(cid) = found {
        let c = u.commands.get_mut(cid).expect("command");
        c.attributes &= !attr::PROTECTING;
        change_order_to_attack(u, cid, targets);
        return;
    }
    remove_ships_from_doing_stuff(u, sel);
    let list = attack_target_list(targets);
    init_extra_attack_info(u, sel, &list, true);
    let mut cmd = new_command(sel.to_vec());
    cmd.order = order::ATTACK;
    cmd.attack_targets = list;
    for &id in sel {
        let s = u.ships.ship_mut(id);
        s.ai_state = 0;
        s.ai_state_attack = 0;
        s.ai_state_command = 0;
    }
    let cid = u.commands.add(cmd);
    prepare_ships(u, cid);
}

/// `ChangeOrderToAttack` (0x443f60).
fn change_order_to_attack(u: &mut Universe, cid: CommandId, targets: &[ShipId]) {
    let cmd = u.commands.get(cid).expect("command").clone();
    if cmd.attributes & attr::PASSIVE_ATTACKING != 0 {
        clear_passive_attacking(u, cid);
    }
    let cmd = u.commands.get(cid).expect("command").clone();
    let mut was_attacking = false;
    if cmd.order == order::ATTACK {
        if cmd.attack_targets.len() == targets.len() && cmd.attack_targets.iter().zip(targets).all(|(a, b)| a == b) {
            if let Some(c) = u.commands.get_mut(cid) {
                c.attributes &= !attr::ATTACKING_AND_MOVING;
            }
            return;
        }
        was_attacking = true;
    }
    free_last_order(u, cid);
    let list = attack_target_list(targets);
    init_extra_attack_info(u, &cmd.selection, &list, true);
    let c = u.commands.get_mut(cid).expect("command");
    c.order = order::ATTACK;
    c.attack_targets = list;
    if c.attributes & attr::FORMATION != 0 {
        c.attack_in_range = false;
    }
    prepare_ships(u, cid);
    for &id in &cmd.selection {
        let s = u.ships.ship_mut(id);
        s.ai_state = 0;
        if !was_attacking {
            s.ai_state_attack = 0;
        }
        s.ai_state_command = 0;
    }
}

/// `AttackClosestOpponent`: the nearest target.
fn attack_closest_opponent(u: &Universe, id: ShipId, targets: &[ShipId]) -> Option<ShipId> {
    let p = u.ships.ship(id).posinfo.position;
    let mut best = f32::from_bits(0x6258_d727);
    let mut pick = None;
    for &t in targets {
        let Some(ts) = u.ships.get(t) else { continue };
        let q = ts.posinfo.position;
        let d = Vector::new(st(ld(p.x) - ld(q.x)), st(ld(p.y) - ld(q.y)), st(ld(p.z) - ld(q.z)));
        let dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
        if dsq < best {
            pick = Some(t);
            best = dsq;
        }
    }
    pick
}

/// `isThisShipThreateningMe`: `other` (or its pair) is attacking `me` and is not manoeuvring.
fn is_threatening(u: &Universe, other: ShipId, me: ShipId) -> bool {
    let o = u.ships.ship(other);
    let quiet = o.attack.situation == 0;
    if o.attack.target == Some(me) {
        return quiet;
    }
    match o.attack.leader {
        None => {
            if let Some(w) = o.attack.wingman.and_then(|w| u.ships.get(w)) {
                if w.attack.target == Some(me) {
                    return quiet;
                }
            }
        }
        Some(l) => {
            if u.ships.get(l).is_some_and(|l| l.attack.target == Some(me)) {
                return quiet;
            }
        }
    }
    false
}

/// `FindNewThreateningTarget`: the nearest target within `FIGHTER_MAX_THREAT_DIST` that is
/// attacking us.
fn find_new_threatening_target(u: &Universe, id: ShipId, targets: &[ShipId], except: Option<ShipId>) -> Option<ShipId> {
    let me = u.ships.ship(id);
    let max = u.tweaks.combat.fighter_max_threat_dist[me.tactics];
    let mut best = f32::from_bits(0x6258_d727);
    let mut pick = None;
    for &t in targets {
        if Some(t) == except || u.ships.get(t).is_none() {
            continue;
        }
        if !is_threatening(u, t, id) {
            continue;
        }
        let q = u.ships.ship(t).posinfo.position;
        let p = me.posinfo.position;
        let d = Vector::new(st(ld(q.x) - ld(p.x)), st(ld(q.y) - ld(p.y)), st(ld(q.z) - ld(p.z)));
        let dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
        if dsq < max && dsq < best {
            best = dsq;
            pick = Some(t);
        }
    }
    pick
}

/// The flight-manoeuvre probabilities of an interceptor against `target`.
fn flightman_prob(u: &Universe, id: ShipId, target: Option<ShipId>) -> Option<crate::combat_statics::FlightManProb> {
    let s = u.ships.ship(id);
    let ic = u.combat.get(s.ship_type)?.interceptor.as_ref()?;
    let class = target
        .and_then(|t| u.ships.get(t))
        .map_or(crate::combat_statics::DEFAULT_CLASS, |t| crate::combat_statics::class_index(u.statics.ship(t.ship_type).and_then(|st| st.ship_class)));
    Some(ic.flightman_prob[s.tactics][class.min(crate::combat_statics::CLASS_COLUMNS - 1)])
}

/// The evasive manoeuvre a fighter picks against `threat`: a run of pure evasive manoeuvres
/// (`evasivePureProb`), or one manoeuvre chosen by whether the threat is in front or behind.
fn evasive_manoeuvre(u: &mut Universe, id: ShipId, threat: ShipId) -> u32 {
    use crate::flightman::{family, get_random};
    let tac = u.ships.ship(id).tactics;
    let prob = flightman_prob(u, id, Some(threat)).unwrap_or_default();
    let ev = u.tweaks.combat.flightman.evasivepure_prob[tac];
    if (u.randy.random(hwc_math::random::RAN_GAME) & 0xff) < ev {
        let fm = get_random(u, &prob, family::EVASIVE_PURE);
        let s = u.ships.ship_mut(id);
        s.attack.flightmans_left = u.tweaks.combat.flightman.num_evasivepure_maneuvers[tac];
        s.attack.situation = crate::object::attack_situation::JUST_EVADING;
        fm
    } else {
        let s = u.ships.ship(id);
        let t = u.ships.ship(threat);
        let h = s.heading();
        let d = [
            ld(s.posinfo.position.x) - ld(t.posinfo.position.x),
            ld(s.posinfo.position.y) - ld(t.posinfo.position.y),
            ld(s.posinfo.position.z) - ld(t.posinfo.position.z),
        ];
        let fam = if (d[0] * ld(h.x) + d[1] * ld(h.y)) + d[2] * ld(h.z) <= 0.0 { family::EVASIVE_FRONT } else { family::EVASIVE_BEHIND };
        let fm = get_random(u, &prob, fam);
        let s = u.ships.ship_mut(id);
        s.attack.flightmans_left = 1;
        s.attack.situation = crate::object::attack_situation::EVADING_REPOS;
        fm
    }
}

fn set_target(u: &mut Universe, id: ShipId, t: Option<ShipId>) {
    let s = u.ships.ship_mut(id);
    s.attack.target = t;
    s.ai_state = 0;
    s.ai_state_attack = 0;
}

/// `formationWingmanTrackLeader` (0x47de00): a wingman keeps station behind and to the side
/// of its leader.
fn wingman_track_leader(u: &mut Universe, id: ShipId, leader: ShipId, turn: bool) {
    let statics = u.statics.clone();
    let l = u.ships.ship(leader).clone();
    let s = u.ships.ship(id);
    let (Some(ls), Some(ss)) = (statics.ship(l.ship_type), statics.ship(s.ship_type)) else { return };
    let mut head = l.heading();
    let right = l.rotinfo.coordsys.col(1);
    let k = (ld(ls.header.coll.coll_sphere_size) + ld(ss.header.coll.coll_sphere_size)) * ld(1.5f32);
    let p = l.posinfo.position;
    let dest = Vector::new(
        st((ld(right.x) - ld(head.x)) * k + ld(p.x)),
        st((ld(right.y) - ld(head.y)) * k + ld(p.y)),
        st((ld(right.z) - ld(head.z)) * k + ld(p.z)),
    );
    if !turn {
        if !aitrack::move_reached_destination(s, dest, 10.0) {
            aiship::fly_to_point_avoiding_objs(u, id, Some(dest), fly::FAST_AS_POSSIBLE, 0.0, Some(l.posinfo.velocity));
        }
        return;
    }
    let left = st(aitrack::move_left_to_go(s, dest));
    let mut flags = fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE;
    if left <= 100.0 {
        let tw = u.tweaks.clone();
        let dt = u.phys_time_elapsed;
        let sm = u.ships.ship_mut(id);
        aitrack::heading(sm, ss, &tw, dt, &mut head, crate::attack::HEADING_ACCURACY, crate::attack::HEADING_ACCURACY, ss.sinbank, 0);
        flags = fly::FAST_AS_POSSIBLE;
    }
    if 10.0 <= left {
        aiship::fly_to_point_avoiding_objs(u, id, Some(dest), flags, 0.0, Some(l.posinfo.velocity));
    }
}

/// `processAttackToDo` (0x43d6d0) for a command that is not a formation. Returns true when
/// the attack is over (no targets left).
fn process_attack_to_do(u: &mut Universe, cid: CommandId) -> bool {
    use crate::flightman::{execute, get_random, init, kind};
    use crate::object::{attack_event, attack_situation, tactics};
    let cmd = u.commands.get(cid).expect("command").clone();
    // (Fog of war: every target counts as visible.)
    if cmd.attack_targets.is_empty() {
        for &id in &cmd.selection {
            let s = u.ships.ship_mut(id);
            s.holding_pattern_pos = s.posinfo.position;
        }
        let all_small = cmd.selection.iter().all(|&id| {
            let c = u.statics.ship(u.ships.ship(id).ship_type).and_then(|s| s.ship_class);
            matches!(c, Some(crate::statics::ShipClass::Fighter) | Some(crate::statics::ShipClass::Corvette))
        });
        if !all_small && cmd.attributes == 0 {
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let mut all = true;
            for &id in &cmd.selection {
                let s = u.ships.ship_mut(id);
                all &= aitrack::steady_ship(s, statics.ship(s.ship_type).expect("statics"), &tw, dt);
            }
            return all;
        }
        // (Strike craft go into the holding pattern here; it is not modelled, they stop.)
        return true;
    }
    // (Guard conditions during an attack and attacking while moving are not modelled.)
    let tw = u.tweaks.combat.clone();
    let counter = u.update_counter;
    let rnd = |u: &mut Universe| u.randy.random(hwc_math::random::RAN_GAME) & 0xff;
    for &id in &cmd.selection {
        if u.ships.ship(id).coll_my_blob.is_none() {
            return true;
        }
        let interceptor = u
            .combat
            .get(u.ships.ship(id).ship_type)
            .is_some_and(|c| matches!(c.attack, crate::combat_statics::AttackKind::Interceptor | crate::combat_statics::AttackKind::Acolyte));
        let mut target = u.ships.ship(id).attack.target.filter(|t| u.ships.get(*t).is_some());
        // `attack` = fall through to the ship's attack routine with `target`.
        let mut attack = true;
        if !interceptor {
            if target.is_none() {
                target = attack_closest_opponent(u, id, &cmd.attack_targets);
                set_target(u, id, target);
            }
            if u.ships.ship(id).attack.multiple_targets.is_some() && counter & tw.multipletargets_retarget_rate == u32::from(id.0) & tw.multipletargets_retarget_rate {
                pick_multiple_attack_targets(u, id, &cmd.attack_targets);
            }
        } else if u.ships.ship(id).attack.flightmans_left != 0 {
            attack = false;
            if execute(u, id) {
                let s = u.ships.ship_mut(id);
                s.attack.flightmans_left -= 1;
                if s.attack.flightmans_left == 0 {
                    s.attack.situation = 0;
                } else {
                    let prob = flightman_prob(u, id, target).unwrap_or_default();
                    let fm = get_random(u, &prob, crate::flightman::family::EVASIVE_PURE);
                    init(u, id, fm, -1, None);
                }
            }
        } else {
            let s = u.ships.ship(id);
            let tac = s.tactics;
            let (leader, wingman) = (s.attack.leader, s.attack.wingman);
            if tac == tactics::AGGRESSIVE || leader.is_none() {
                let single = (leader.is_none() && wingman.is_none()) || tac == tactics::AGGRESSIVE;
                let rate = if single { tw.fighter_single_check_rate[tac] } else { tw.fighter_pair_check_rate[tac] };
                let paired = !single;
                if target.is_none() {
                    match find_new_threatening_target(u, id, &cmd.attack_targets, None) {
                        None => {
                            target = attack_closest_opponent(u, id, &cmd.attack_targets);
                            set_target(u, id, target);
                            let lone_target = target.and_then(|t| u.ships.get(t)).is_some_and(|ts| ts.attack.wingman.is_none() && ts.attack.leader.is_none());
                            if paired && lone_target && rnd(u) < tw.flightman.sandwich_prob[tac] {
                                let t = target.expect("target");
                                if let Some(w) = wingman {
                                    set_target(u, w, Some(t));
                                    u.ships.ship_mut(w).flightman_state2 = 1;
                                    init(u, w, kind::SANDWICH, 0, Some(t));
                                    let ws = u.ships.ship_mut(w);
                                    ws.attack.flightmans_left = 1;
                                    ws.attack.situation = attack_situation::SANDWICH;
                                }
                                u.ships.ship_mut(id).flightman_state2 = 0;
                                init(u, id, kind::SANDWICH, 0, Some(t));
                                let s = u.ships.ship_mut(id);
                                s.attack.flightmans_left = 1;
                                s.attack.situation = attack_situation::SANDWICH;
                                attack = false;
                            } else if u.ships.ship(id).attack.events & attack_event::ENEMY_TARGET_DIED != 0 {
                                init(u, id, kind::BARREL_ROLL, -1, None);
                                u.ships.ship_mut(id).attack.flightmans_left = 1;
                                attack = false;
                            }
                        }
                        Some(threat) => {
                            let fm = evasive_manoeuvre(u, id, threat);
                            init(u, id, fm, -1, None);
                            set_target(u, id, Some(threat));
                            attack = false;
                        }
                    }
                } else if let Some(t) = target.filter(|_| counter & rate == tw.checkbogey_frame & rate) {
                    if !is_threatening(u, t, id) {
                        if let Some(threat) = find_new_threatening_target(u, id, &cmd.attack_targets, Some(t)) {
                            let (fm, flag) = if paired && rnd(u) < tw.flightman.split_prob[tac] {
                                if let Some(w) = wingman {
                                    let ws = u.ships.ship_mut(w);
                                    ws.attack.flightmans_left = 1;
                                    ws.attack.situation = attack_situation::SPLITTING;
                                    let (wf, wflag) = if u.randy.random(hwc_math::random::RAN_GAME) & 1 == 0 { (kind::HARDBANK, -1) } else { (kind::IMMELMAN, 1) };
                                    init(u, w, wf, wflag, None);
                                }
                                let s = u.ships.ship_mut(id);
                                s.attack.flightmans_left = 1;
                                s.attack.situation = attack_situation::SPLITTING;
                                (kind::IMMELMAN, 0)
                            } else {
                                (evasive_manoeuvre(u, id, threat), -1)
                            };
                            init(u, id, fm, flag, None);
                            set_target(u, id, Some(threat));
                            attack = false;
                        }
                    } else if tac == tactics::EVASIVE
                        && counter & tw.flightman.token_evasivepure_checkrate == tw.flightman.token_evasivepure_checkframe
                        && rnd(u) < tw.flightman.token_evasivepure_prob
                    {
                        let prob = flightman_prob(u, id, Some(t)).unwrap_or_default();
                        let fm = get_random(u, &prob, crate::flightman::family::EVASIVE_PURE);
                        let s = u.ships.ship_mut(id);
                        s.attack.flightmans_left = tw.flightman.num_evasivepure_maneuvers[tac];
                        s.attack.situation = attack_situation::JUST_EVADING;
                        init(u, id, fm, -1, None);
                        attack = false;
                    }
                }
            } else if let (None, Some(l)) = (target, leader) {
                // A wingman without a target of its own sticks with its leader, firing at the
                // leader's target when it is ahead of the leader.
                attack = false;
                let lt = u.ships.get(l).and_then(|ls| ls.attack.target).filter(|t| u.ships.get(*t).is_some());
                let mut turn = true;
                if let Some(t2) = lt {
                    let ls = u.ships.ship(l);
                    let h = ls.heading();
                    let p = u.ships.ship(id).posinfo.position;
                    let q = u.ships.ship(t2).posinfo.position;
                    let ahead = (ld(p.x) - ld(q.x)) * ld(h.x) + (ld(p.y) - ld(q.y)) * ld(h.y) + (ld(p.z) - ld(q.z)) * ld(h.z) < 0.0;
                    if crate::attack::can_attack_passive(u, id) && ahead {
                        crate::attack::ship_attack_passive(u, id, t2, true);
                        turn = false;
                    }
                }
                wingman_track_leader(u, id, l, turn);
            }
        }
        if attack {
            if let Some(t) = target.filter(|t| u.ships.get(*t).is_some()) {
                // (Kamikaze attacks are not modelled.)
                crate::attack::ship_attack(u, id, t);
            }
        }
        let s = u.ships.ship_mut(id);
        s.ship_idle = false;
        s.attack.events = 0;
    }
    false
}

/// `clPassiveAttack` (0x4447b0): the selection fires at `targets` while it carries on with
/// what it is doing (a new command when its ships are not exactly one command).
pub fn cl_passive_attack(u: &mut Universe, selection: &[ShipId], targets: &[ShipId]) {
    if selection.is_empty() || targets.is_empty() {
        return;
    }
    let found = u.commands.iter().find(|c| selections_equivalent(selection, &c.selection)).map(|c| c.id);
    if let Some(cid) = found {
        let c = u.commands.get(cid).expect("command");
        if c.attributes & attr::HOLDING_PATTERN != 0 {
            return;
        }
        let same = c.attributes & attr::PASSIVE_ATTACKING != 0 && selections_equivalent(&c.attack_targets, targets);
        if same {
            return;
        }
        if c.attributes & attr::PASSIVE_ATTACKING == 0 && !matches!(c.order, order::NULL | order::MOVE | order::HALT | order::MILITARY_PARADE) {
            return;
        }
        change_order_to_passive_attack(u, cid, targets);
        return;
    }
    remove_ships_from_doing_stuff(u, selection);
    init_extra_attack_info(u, selection, targets, false);
    let mut cmd = new_command(selection.to_vec());
    cmd.order = order::NULL;
    cmd.attributes = attr::PASSIVE_ATTACKING;
    cmd.attack_targets = targets.to_vec();
    for &id in selection {
        let s = u.ships.ship_mut(id);
        s.ai_state = 0;
        s.ai_state_attack = 0;
    }
    let cid = u.commands.add(cmd);
    prepare_ships(u, cid);
}

/// `ChangeOrderToPassiveAttack` (0x444600).
fn change_order_to_passive_attack(u: &mut Universe, cid: CommandId, targets: &[ShipId]) {
    let c = u.commands.get(cid).expect("command").clone();
    if c.attributes & attr::PASSIVE_ATTACKING != 0 {
        clear_passive_attacking(u, cid);
    } else if c.order == order::ATTACK {
        attack_cleanup(u, cid);
        u.commands.get_mut(cid).expect("command").attack_targets.clear();
    }
    init_extra_attack_info(u, &c.selection, targets, false);
    let cm = u.commands.get_mut(cid).expect("command");
    cm.attack_targets = targets.to_vec();
    cm.attributes = (cm.attributes & !attr::HOLDING_PATTERN) | attr::PASSIVE_ATTACKING;
    for &id in &c.selection {
        let s = u.ships.ship_mut(id);
        s.ai_state = 0;
        s.ai_state_attack = 0;
    }
    prepare_ships(u, cid);
}

/// `ShipsInPassiveAttackRange`: any ship of `sel` within its `passiveRetaliateZone` box of
/// the targets' average position.
pub fn ships_in_passive_attack_range(u: &Universe, sel: &[ShipId], targets: &[ShipId]) -> bool {
    let live: Vec<_> = targets.iter().filter_map(|&t| u.ships.get(t)).collect();
    if live.is_empty() {
        return false;
    }
    let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    for t in &live {
        x = st(ld(x) + ld(t.posinfo.position.x));
        y = st(ld(y) + ld(t.posinfo.position.y));
        z = st(ld(z) + ld(t.posinfo.position.z));
    }
    let k = ld(st(1.0 / ld(live.len() as i32 as f32)));
    let mut any = false;
    for &id in sel {
        let s = u.ships.ship(id);
        let zone = ld(u.statics.ship(s.ship_type).map_or(0.0, |st| st.passive_retaliate_zone));
        let p = s.posinfo.position;
        let d = [ld(p.x) - k * ld(x), ld(p.y) - k * ld(y), ld(p.z) - k * ld(z)];
        if d.iter().all(|&c| -zone < c && c < zone) {
            any = true;
        }
    }
    any
}

/// `processPassiveAttackToDo` (0x43e180): every ship fires at its target while targets are in
/// range. Returns true when the passive attack is over.
fn process_passive_attack_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let cmd = u.commands.get(cid).expect("command").clone();
    let targets: Vec<ShipId> = cmd.attack_targets.iter().copied().filter(|t| u.ships.get(*t).is_some()).collect();
    if targets.is_empty() || !ships_in_passive_attack_range(u, &cmd.selection, &targets) {
        return true;
    }
    let rotate = !matches!(cmd.order, order::MOVE | order::DOCK);
    let tw = u.tweaks.combat.clone();
    for &id in &cmd.selection {
        let mut t = u.ships.ship(id).attack.target.filter(|t| u.ships.get(*t).is_some());
        if t.is_none() {
            t = attack_closest_opponent(u, id, &targets);
            let s = u.ships.ship_mut(id);
            s.attack.target = t;
            s.ai_state = 0;
            s.ai_state_attack = 0;
        }
        if u.ships.ship(id).attack.multiple_targets.is_some() && u.update_counter & tw.multipletargets_retarget_rate == u32::from(id.0) & tw.multipletargets_retarget_rate {
            pick_multiple_attack_targets(u, id, &targets);
        }
        if let Some(t) = t {
            crate::attack::ship_attack_passive(u, id, t, rotate);
        }
        u.ships.ship_mut(id).attack.events = 0;
    }
    false
}

/// The passive attack that runs alongside a command's order.
fn run_passive_attack(u: &mut Universe, cid: CommandId) {
    let has = u.commands.get(cid).is_some_and(|c| c.attributes & attr::PASSIVE_ATTACKING != 0);
    if has {
        if process_passive_attack_to_do(u, cid) {
            clear_passive_attacking(u, cid);
        }
        if let Some(c) = u.commands.get_mut(cid) {
            c.attributes &= !attr::HOLDING_PATTERN;
        }
    }
}

/// `RemoveShipFromBeingTargeted`: a dead ship leaves every target list.
pub(crate) fn remove_ship_from_being_targeted(u: &mut Universe, dead: ShipId) {
    let todo = u.commands.todo.clone();
    for cid in todo {
        let Some(c) = u.commands.get_mut(cid) else { continue };
        if (c.attributes & attr::PASSIVE_ATTACKING != 0 || c.order == order::ATTACK) && remove_from_selection(&mut c.attack_targets, dead) {
            {
                let sel = c.selection.clone();
                for id in sel {
                    let s = u.ships.ship_mut(id);
                    if s.attack.target == Some(dead) {
                        s.attack.events |= crate::object::attack_event::ENEMY_TARGET_DIED;
                        s.attack.target = None;
                    }
                    if let Some(m) = s.attack.multiple_targets.as_mut() {
                        for t in m.iter_mut() {
                            if *t == Some(dead) {
                                *t = None;
                            }
                        }
                    }
                }
            }
        }
    }
}

/// `clShipDied`: a dead ship leaves its command and every target list.
pub fn cl_ship_died(u: &mut Universe, dead: ShipId) {
    remove_ships_from_doing_stuff(u, &[dead]);
    remove_ship_from_being_targeted(u, dead);
}

/// `clSetTactics`: set the tactics of the selection (the ships' commands keep running).
pub fn cl_set_tactics(u: &mut Universe, selection: &[ShipId], tactics: usize) {
    for &id in selection {
        if let Some(s) = u.ships.get_mut(id) {
            s.tactics = tactics.min(2);
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
