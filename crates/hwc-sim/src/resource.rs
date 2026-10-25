//! Resources and harvesting (`ResCollect.c`, the resource parts of `Universe.c` /
//! `UnivUpdate.c`). Asteroids and dust clouds hold resource units; workers and resource
//! collectors harvest them (`clCollectResource`, `processCollectResource`), carry up to
//! `maxresources` and dock with the nearest ship that takes resources to unload (dock purpose
//! 3), then go back to harvesting. `harvestableByMultipleShips` caps the harvesters per
//! resource. Multiplayer games also trickle resource units to players below
//! `regenerateRUsIfUnder`.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::aiship::fly_to_point_avoiding_objs;
use crate::aitrack;
use crate::base::{self, special2, HarvestFn};
use crate::command::{self, order};
use crate::object::CommandId;
use crate::object::{sof, ShipId};
use crate::statics::{ResourceInfo, ShipType};
use crate::universe::{Universe, DEFAULT_SHIP_MATRIX};

/// A resource's identity (`resourceID`), handed out in creation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ResourceId(pub u16);

/// Resource kinds (`ObjType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// Asteroid of type 0..4.
    Asteroid(u8),
    /// Dust cloud of type 0..3.
    DustCloud(u8),
}

/// A resource object (`Resource` / `Asteroid` / `DustCloud`, 0x1b4 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Resource {
    pub id: ResourceId,
    pub kind: ResourceKind,
    /// +0x14 flags (`SOF_*`).
    pub flags: u32,
    /// +0x38 attributes.
    pub attributes: u32,
    /// +0x3c attributes parameter.
    pub attributes_param: i16,
    /// +0x194: the regrowing resource volume it belongs to (index into the level's volumes).
    pub volume: Option<usize>,
    pub position: Vector,
    pub velocity: Vector,
    pub force: Vector,
    pub coordsys: Matrix,
    pub rotspeed: Vector,
    pub torque: Vector,
    /// +0x7c.
    pub is_moving: bool,
    /// Collision centre.
    pub coll_position: Vector,
    /// +0x170 health.
    pub health: f32,
    /// +0x18e: frames the resource may not be chosen (it was out of reach).
    pub not_accessible: u16,
    /// +0x190 resource units left.
    pub resource_value: i32,
    /// +0x19c dust cloud scale (1).
    pub scale: f32,
}

/// The resources (`universe.ResourceList`) by id, in list order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResourceTable {
    pub list: Vec<Resource>,
    /// `universe.resourceNumber`.
    pub next_id: u16,
    /// `ResourcesToDeleteList`.
    pub dead: Vec<ResourceId>,
}

impl ResourceTable {
    pub fn get(&self, id: ResourceId) -> Option<&Resource> {
        self.list.iter().find(|r| r.id == id)
    }

    pub fn get_mut(&mut self, id: ResourceId) -> Option<&mut Resource> {
        self.list.iter_mut().find(|r| r.id == id)
    }
}

/// The statics of a resource.
pub fn info(u: &Universe, kind: ResourceKind) -> Option<&ResourceInfo> {
    match kind {
        ResourceKind::Asteroid(t) => u.statics.asteroids.get(t as usize)?.as_ref().map(|a| &a.info),
        ResourceKind::DustCloud(t) => u.statics.dust_clouds.get(t as usize)?.as_ref().map(|a| &a.info),
    }
}

/// `univInitSpaceObjPosRot` with a random orientation: three game random numbers, turning the
/// default orientation about x, then y, then z.
pub fn random_orientation(u: &mut Universe) -> Matrix {
    let mut coordsys = DEFAULT_SHIP_MATRIX;
    let a = u.randy.random(RAN_GAME);
    let b = u.randy.random(RAN_GAME);
    let c = u.randy.random(RAN_GAME);
    let rot = |r: u32| {
        let ang = ld(st(ld(r as f32) * ld(1.462_918_1e-9)));
        (st(fcos(ang)), st(fsin(ang)))
    };
    let (cx, sx) = rot(a);
    let (cy, sy) = rot(b);
    let (cz, sz) = rot(c);
    coordsys = coordsys.mul_mat(&Matrix::rot_about_x(cx, sx));
    coordsys = coordsys.mul_mat(&Matrix::rot_about_y(cy, sy));
    coordsys.mul_mat(&Matrix::rot_about_z(cz, sz))
}

/// `univAddAsteroid` (0x53bae0) / `univAddDustCloud` (0x53c3a0): a resource at `position` with
/// a random orientation and its full resource value. Without statics for `kind` nothing is
/// created (and no random numbers are drawn).
pub fn add_resource(u: &mut Universe, kind: ResourceKind, position: Vector) -> Option<ResourceId> {
    let stat = info(u, kind)?.clone();
    let id = ResourceId(u.base.resources.next_id);
    u.base.resources.next_id = u.base.resources.next_id.wrapping_add(1);
    let coordsys = random_orientation(u);
    let off = coordsys.mul_vec(stat.header.coll.coll_sphere_offset);
    let flags = match kind {
        // The smallest asteroids are minor objects (`SOF_...4`, kept out of the blobs).
        ResourceKind::Asteroid(0) => 0x0010_4007,
        ResourceKind::Asteroid(_) => 0x0010_4003,
        ResourceKind::DustCloud(_) => 0x0018_4003,
    };
    let r = Resource {
        id,
        kind,
        flags,
        attributes: 0,
        attributes_param: 0,
        volume: None,
        position,
        velocity: Vector::ZERO,
        force: Vector::ZERO,
        coordsys,
        rotspeed: Vector::ZERO,
        torque: Vector::ZERO,
        is_moving: false,
        coll_position: position + off,
        health: stat.health.max_health,
        not_accessible: 0,
        resource_value: stat.resource_value,
        scale: 1.0,
    };
    u.base.resources.list.push(r);
    Some(id)
}

/// `univUpdateAllPosVelResources` (0x53e360): basic physics for every resource, and the
/// not-accessible counters run down.
pub fn update_all_pos_vel(u: &mut Universe) {
    let dt = u.phys_time_elapsed;
    let statics = u.statics.clone();
    for i in 0..u.base.resources.list.len() {
        let kind = u.base.resources.list[i].kind;
        let stat = match kind {
            ResourceKind::Asteroid(t) => statics.asteroids.get(t as usize).and_then(|a| a.as_ref()).map(|a| a.info.clone()),
            ResourceKind::DustCloud(t) => statics.dust_clouds.get(t as usize).and_then(|a| a.as_ref()).map(|a| a.info.clone()),
        };
        let r = &mut u.base.resources.list[i];
        if r.flags & 0x84 == 0 {
            if let Some(stat) = stat {
                phys_basic(r, &stat, dt);
            }
        }
        if r.not_accessible != 0 {
            r.not_accessible -= 1;
        }
    }
}

/// `physUpdateObjPosVelBasic` for a resource (force, capped velocity, position, spin).
fn phys_basic(r: &mut Resource, stat: &ResourceInfo, dt: f32) {
    let h = &stat.header;
    let k = ld(h.one_over_mass);
    let f = r.force;
    let dv = Vector::new(st(k * ld(f.x) * ld(dt)), st(k * ld(f.y) * ld(dt)), st(k * ld(f.z) * ld(dt)));
    r.velocity = Vector::new(st(ld(dv.x) + ld(r.velocity.x)), st(ld(dv.y) + ld(r.velocity.y)), st(ld(dv.z) + ld(r.velocity.z)));
    if r.velocity.x == 0.0 && r.velocity.y == 0.0 && r.velocity.z == 0.0 {
        r.is_moving = false;
    } else {
        r.is_moving = true;
        r.velocity.cap_sloppy(h.max_velocity);
        r.position = Vector::new(
            st(ld(dt) * ld(r.velocity.x) + ld(r.position.x)),
            st(ld(dt) * ld(r.velocity.y) + ld(r.position.y)),
            st(ld(dt) * ld(r.velocity.z) + ld(r.position.z)),
        );
    }
    let mut turned = false;
    let spin = |axis: u8, w: f32, m: &mut Matrix| {
        let a = ld(dt) * ld(w);
        let (c, s) = (st(fcos(a)), st(fsin(a)));
        let rm = match axis {
            0 => Matrix::rot_about_x(c, s),
            1 => Matrix::rot_about_y(c, s),
            _ => Matrix::rot_about_z(c, s),
        };
        *m = m.mul_mat(&rm);
    };
    if r.rotspeed.x != 0.0 {
        spin(0, r.rotspeed.x, &mut r.coordsys);
        turned = true;
    }
    if r.rotspeed.y != 0.0 {
        spin(1, r.rotspeed.y, &mut r.coordsys);
        turned = true;
    }
    if r.rotspeed.z != 0.0 {
        spin(2, r.rotspeed.z, &mut r.coordsys);
        turned = true;
    }
    let off = if turned { r.coordsys.mul_vec(h.coll.coll_sphere_offset) } else { r.coll_position - r.position };
    r.coll_position = r.position + off;
    r.force = Vector::ZERO;
}

/// Workers harvest with a beam (`WorkerIsWorker`).
pub fn is_worker(t: ShipType) -> bool {
    t == ShipType::sWorker || t == ShipType::bWorker
}

/// `ResourceMovingTooFast` (0x4ec580).
fn moving_too_fast(u: &Universe, r: &Resource) -> bool {
    let m = u.tweaks.base.max_resource_velocity_to_chase;
    ld(r.velocity.x).abs() > ld(m) || ld(r.velocity.y).abs() > ld(m) || ld(r.velocity.z).abs() > ld(m)
}

/// `ResourceAlreadyBeingHarvested` (0x4ec600): the resource has as many harvesters as it allows
/// (`harvestableByMultipleShips`; 0 = one), not counting command `except`.
pub fn already_being_harvested(u: &Universe, except: Option<CommandId>, r: ResourceId) -> bool {
    let cap = u.base.resources.get(r).and_then(|x| info(u, x.kind)).map_or(0, |i| i.harvestable_by_multiple_ships) as u32;
    let n = u.commands.iter().filter(|c| c.order == order::COLLECT_RESOURCE && Some(c.id) != except && c.collect.resource == Some(r)).count() as u32;
    if cap != 0 {
        cap <= n
    } else {
        n > 0
    }
}

/// Inside the world bounds (`pieMovePointClipToLimits` leaves the point alone).
fn inside_world(u: &Universe, p: Vector) -> bool {
    let b = u.world_bounds;
    p.x >= -b.x && p.x <= b.x && p.y >= -b.y && p.y <= b.y && p.z >= -b.z && p.z <= b.z
}

/// `univFindNearestResource` (0x544c70): the nearest resource available to `ship` (within the
/// box of half-size `radius` around `centre` when `radius` > 0).
pub fn find_nearest_resource(u: &Universe, ship: ShipId, radius: f32, centre: Vector) -> Option<ResourceId> {
    let me = u.ships.ship(ship).posinfo.position;
    let mut best = 1e20f32;
    let mut found = None;
    for r in &u.base.resources.list {
        if r.attributes & base::attributes::RESOURCE_UNHARVESTABLE != 0 || r.flags & sof::DEAD != 0 {
            continue;
        }
        if moving_too_fast(u, r) || already_being_harvested(u, None, r.id) || r.not_accessible != 0 {
            continue;
        }
        if radius > 0.0 {
            let d = r.position - centre;
            let neg = -radius;
            if !(neg <= d.x && d.x <= radius && neg <= d.y && d.y <= radius && neg <= d.z && d.z <= radius) {
                continue;
            }
        }
        if !inside_world(u, r.position) {
            continue;
        }
        let dx = st(ld(r.position.x) - ld(me.x));
        let dy = st(ld(r.position.y) - ld(me.y));
        let dz = st(ld(r.position.z) - ld(me.z));
        let d2 = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
        if ld(d2) < ld(best) {
            best = d2;
            found = Some(r.id);
        }
    }
    found
}

/// The harvest routine of a type.
fn harvest_fn_for(t: ShipType) -> HarvestFn {
    if is_worker(t) {
        HarvestFn::Worker
    } else {
        HarvestFn::Resourcer
    }
}

/// `clCollectResource` (0x4ec7e0): each ship gets its own collect command, harvesting
/// `resource` (or the nearest one) and staying in the zone around it.
pub fn cl_collect_resource(u: &mut Universe, ships: &[ShipId], resource: Option<ResourceId>) {
    let ships: Vec<ShipId> = ships.iter().copied().filter(|&s| u.ships.get(s).is_some()).collect();
    if ships.is_empty() {
        return;
    }
    command::remove_ships_from_doing_stuff(u, &ships);
    let zone = u.tweaks.base.collect_resource_zone;
    for s in ships {
        command::init_ship_ai_pub(u, s);
        let sh = u.ships.ship_mut(s);
        sh.base.harvest_state = 0;
        sh.base.harvest_state2 = 0;
        sh.base.harvest_fn = harvest_fn_for(sh.ship_type);
        let mut c = command::new_command_pub(vec![s]);
        c.order = order::COLLECT_RESOURCE;
        c.collect.resource = resource;
        if let Some(r) = resource.and_then(|r| u.base.resources.get(r)) {
            c.collect.zone_radius = zone;
            c.collect.zone_centre = r.position;
        }
        let cid = u.commands.add_pub(c);
        command::prepare_ships_pub(u, cid);
    }
}

/// `ChangeSingleShipToCollectResource` (0x4ec750): the command turns into a collect order
/// (after unloading), harvesting `resource` or the nearest one.
pub fn change_single_ship_to_collect_resource(u: &mut Universe, cid: CommandId, resource: Option<ResourceId>) {
    let Some(&ship) = u.commands.get(cid).and_then(|c| c.selection.first()) else { return };
    let res = resource.or_else(|| find_nearest_resource_for(u, ship, cid));
    command::init_ship_ai_pub(u, ship);
    let sh = u.ships.ship_mut(ship);
    sh.base.harvest_state = 0;
    sh.base.harvest_state2 = 0;
    sh.base.harvest_fn = harvest_fn_for(sh.ship_type);
    if let Some(c) = u.commands.get_mut(cid) {
        c.order = order::COLLECT_RESOURCE;
        c.attributes = 0;
        c.collect.resource = res;
    }
    u.ships.ship_mut(ship).command = Some(cid);
}

/// `rescollectFindNearestResource` (0x4ec680): first in the command's zone, then anywhere
/// (only for the local player and the computer players).
fn find_nearest_resource_for(u: &mut Universe, ship: ShipId, cid: CommandId) -> Option<ResourceId> {
    let zone = u.commands.get(cid).map_or(0.0, |c| c.collect.zone_radius);
    let look_in_zone = u.ships.ship(ship).special_flags2 & special2::HARVEST_LOOK_IN_ZONE != 0;
    if zone != 0.0 || look_in_zone {
        if look_in_zone {
            let z = u.tweaks.base.collect_resource_zone;
            if let Some(c) = u.commands.get_mut(cid) {
                c.collect.zone_radius = z;
            }
        }
        u.ships.ship_mut(ship).special_flags2 &= !special2::HARVEST_LOOK_IN_ZONE;
        let (r, c) = u.commands.get(cid).map_or((0.0, Vector::ZERO), |c| (c.collect.zone_radius, c.collect.zone_centre));
        if let Some(found) = find_nearest_resource(u, ship, r, c) {
            return Some(found);
        }
        if let Some(c) = u.commands.get_mut(cid) {
            c.collect.zone_radius = 0.0;
        }
    }
    let player = u.ships.ship(ship).player;
    if player != u.base.cur_player && !u.base.players[player].ai {
        return None;
    }
    find_nearest_resource(u, ship, 0.0, Vector::ZERO)
}

/// `rescollectStopHarvesting` (0x4ed8d0).
fn stop_harvesting(u: &mut Universe, ship: ShipId) {
    let s = u.ships.ship_mut(ship);
    s.base.res_harvesting = false;
    s.special_flags2 &= !special2::HARVESTING;
    harvest_step(u, ship, None);
}

/// Calls the ship's harvest routine (`ship+0x390`).
fn harvest_step(u: &mut Universe, ship: ShipId, res: Option<ResourceId>) {
    match u.ships.ship(ship).base.harvest_fn {
        HarvestFn::None => {}
        HarvestFn::Resourcer => resourcer_harvests(u, ship, res),
        HarvestFn::Worker => worker_update_beam(u, ship, res),
    }
}

/// `processCollectResource` (0x4ed5c0). True when the command is over.
pub fn process_collect_resource(u: &mut Universe, cid: CommandId) -> bool {
    let Some(&ship) = u.commands.get(cid).and_then(|c| c.selection.first()) else { return true };
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(u.ships.ship(ship).ship_type) else { return true };
    let carried = u.ships.ship(ship).base.resources;
    if carried < stat.max_resources {
        let target = u.commands.get(cid).and_then(|c| c.collect.resource).filter(|r| u.base.resources.get(*r).is_some());
        if let Some(r) = target {
            let res = u.base.resources.get(r).expect("resource").clone();
            let rate = u.tweaks.base.check_harv_out_of_world_rate;
            let dropped = moving_too_fast(u, &res)
                || already_being_harvested(u, Some(cid), r)
                || (u.update_counter & rate == u32::from(res.id.0) & rate && !inside_world(u, res.position));
            if !dropped {
                harvest_step(u, ship, Some(r));
                if u.base.resources.get(r).is_some_and(|x| x.not_accessible != 0) {
                    u.ships.ship_mut(ship).base.harvest_effect = false;
                    if let Some(c) = u.commands.get_mut(cid) {
                        c.collect.resource = None;
                    }
                    stop_harvesting(u, ship);
                }
                return false;
            }
            u.ships.ship_mut(ship).base.harvest_effect = false;
            if let Some(c) = u.commands.get_mut(cid) {
                c.collect.resource = None;
            }
            let s = u.ships.ship_mut(ship);
            if s.base.res_harvesting {
                s.base.res_harvesting = false;
                s.special_flags2 &= !special2::HARVESTING;
                s.base.res_gone = true;
            }
            harvest_step(u, ship, None);
            return false;
        }
        if let Some(c) = u.commands.get_mut(cid) {
            c.collect.resource = None;
        }
        match find_nearest_resource_for(u, ship, cid) {
            None => {
                if u.base.options.multiplayer_network {
                    return true;
                }
            }
            Some(r) => {
                let s = u.ships.ship_mut(ship);
                if s.base.res_harvesting {
                    s.base.res_harvesting = false;
                    s.special_flags2 &= !special2::HARVESTING;
                    s.base.res_gone = true;
                }
                harvest_step(u, ship, None);
                if u.ships.ship(ship).base.resources < stat.resources_at_one_time {
                    command::init_ship_ai_pub(u, ship);
                    let s = u.ships.ship_mut(ship);
                    s.base.harvest_state = 0;
                    s.base.harvest_state2 = 0;
                    s.base.harvest_fn = harvest_fn_for(s.ship_type);
                    if let Some(c) = u.commands.get_mut(cid) {
                        c.collect.resource = Some(r);
                    }
                    return false;
                }
            }
        }
    } else {
        u.ships.ship_mut(ship).base.harvest_effect = false;
        if let Some(c) = u.commands.get_mut(cid) {
            c.collect.resource = None;
        }
        u.ships.ship_mut(ship).special_flags2 |= special2::HARVEST_LOOK_IN_ZONE;
    }
    // Full (or nothing left): unload at the nearest ship that takes resources.
    let dw = crate::dock::find_nearest_ship_to_dock_at(u, ship, crate::dock::purpose::DEPOSIT_RESOURCES);
    stop_harvesting(u, ship);
    let done = match dw {
        Some(dw) if u.ships.ship(ship).base.resources >= 1 => {
            command::free_last_order_pub(u, cid);
            change_single_ship_to_dock(u, cid, ship, dw);
            false
        }
        _ => true,
    };
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(ship);
    aitrack::steady_ship(s, stat, &tw, dt);
    done
}

/// `dockChangeSingleShipToDock(command, ship, dw, TRUE, 3)`.
fn change_single_ship_to_dock(u: &mut Universe, cid: CommandId, ship: ShipId, dw: ShipId) {
    command::clear_protecting_and_passive(u, cid);
    if let Some(c) = u.commands.get_mut(cid) {
        c.order = order::DOCK;
        c.dock = command::DockInfo { purpose: crate::dock::purpose::DEPOSIT_RESOURCES, collect_after: true, same_dockwith: true };
    }
    crate::dock::initialize_dock_vars(u, ship, dw, crate::dock::purpose::DEPOSIT_RESOURCES);
    command::prepare_ships_pub(u, cid);
    command::init_ship_ai_pub(u, ship);
}

/// `CollectResources` (0x4ecc30): one harvest tick (every `harvestRate` mask frames).
pub fn collect_resources(u: &mut Universe, ship: ShipId, r: ResourceId) -> bool {
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(u.ships.ship(ship).ship_type) else { return false };
    let max = stat.max_resources;
    if max == 0 || max <= u.ships.ship(ship).base.resources {
        return false;
    }
    if (stat.harvest_rate as u32) & u.update_counter != 0 {
        return true;
    }
    let tutorial = u.base.options.tutorial;
    let Some(res) = u.base.resources.get(r).cloned() else { return false };
    let mut amount = stat.harvest_amount.min(res.resource_value);
    if stat.harvest_amount > res.resource_value {
        amount = res.resource_value;
    }
    if tutorial {
        amount *= 5;
    }
    let s = u.ships.ship_mut(ship);
    if s.special_flags2 & special2::NO_RU_GAIN == 0 {
        s.base.resources += amount;
    }
    if max < s.base.resources {
        amount += max - s.base.resources;
        s.base.resources = max;
    }
    let rinfo = info(u, res.kind).cloned();
    let rr = u.base.resources.get_mut(r).expect("resource");
    rr.resource_value -= amount;
    if let (ResourceKind::Asteroid(_), Some(ri)) = (rr.kind, rinfo.as_ref()) {
        let v = (rr.resource_value as f32).max(0.0);
        rr.health = st((ld(v) / ld(ri.resource_value as f32)) * ld(ri.health.max_health));
    }
    if rr.resource_value < 1 && rr.flags & sof::DEAD == 0 {
        rr.flags |= sof::DEAD;
        u.base.resources.dead.push(r);
        let s = u.ships.ship_mut(ship);
        s.base.res_gone = true;
        s.base.res_harvesting = false;
        return true;
    }
    if let (ResourceKind::DustCloud(_), Some(ri)) = (rr.kind, rinfo.as_ref()) {
        let f = st(ld(rr.resource_value as f32) / ld(ri.resource_value as f32));
        rr.scale = f.min(1.0);
    }
    true
}

/// `WorkerIsFacingTarget`: within 45 degrees; with `turn` the worker turns towards the target
/// (or stops turning once facing it).
fn worker_facing(u: &mut Universe, ship: ShipId, target: Vector, check_only: bool) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship(ship);
    let mut d = target - s.posinfo.position;
    d.normalize();
    let h = s.heading();
    let c = st(fcos(ld(90.0f32) * 0.5 * ld(0.017_453_292f32)));
    let facing = ld(c) <= (ld(h.x) * ld(d.x) + ld(h.y) * ld(d.y)) + ld(h.z) * ld(d.z);
    if !check_only {
        let s = u.ships.ship_mut(ship);
        if facing {
            s.rotinfo.rotspeed = Vector::ZERO;
            s.rotinfo.torque = Vector::ZERO;
            return true;
        }
        if let Some(stat) = statics.ship(s.ship_type) {
            let a = f32::from_bits(0x3f7d_70a4);
            aitrack::heading(s, stat, &tw, dt, &mut d, a, a, 0.0, 0);
        }
    }
    facing
}

/// `WorkerUpdateBeam` (0x5c3a80): a worker flies to the resource, faces it, opens its grinder
/// (mesh animation, immediate here) and harvests with its beam.
fn worker_update_beam(u: &mut Universe, ship: ShipId, res: Option<ResourceId>) {
    let Some(r) = res.and_then(|r| u.base.resources.get(r).cloned()) else {
        let s = u.ships.ship_mut(ship);
        s.base.harvest_effect = false;
        s.base.harvest_state = 0;
        return;
    };
    let statics = u.statics.clone();
    let st_ = u.ships.ship(ship).base.harvest_state;
    if st_ != 7 {
        let far = !aitrack::within_distance(u.ships.ship(ship), r.position, 3000.0);
        if matches!(st_, 4..=6) && (!worker_facing(u, ship, r.position, true) || far) {
            let s = u.ships.ship_mut(ship);
            s.base.harvest_effect = false;
            s.base.harvest_state = 7;
        } else {
            u.ships.ship_mut(ship).ship_idle = false;
        }
    }
    let now = u.total_time_elapsed;
    match u.ships.ship(ship).base.harvest_state {
        0 => u.ships.ship_mut(ship).base.harvest_state = 1,
        1 => {
            if aitrack::within_distance(u.ships.ship(ship), r.position, 2000.0)
                || fly_to_point_avoiding_objs(u, ship, Some(r.position), 0x17, 0.0, Some(r.velocity)) == 1
            {
                u.ships.ship_mut(ship).base.harvest_state = 2;
            }
        }
        2 => {
            if worker_facing(u, ship, r.position, false) {
                let s = u.ships.ship_mut(ship);
                s.posinfo.velocity = Vector::ZERO;
                s.posinfo.force = Vector::ZERO;
                s.rotinfo.rotspeed = Vector::ZERO;
                s.rotinfo.torque = Vector::ZERO;
                s.base.harvest_state = 3;
            }
        }
        3 => {
            let s = u.ships.ship_mut(ship);
            if s.base.mad_anim == 0 {
                s.base.worker_timer = now;
                if s.base.mad_done {
                    s.base.harvest_state = match r.kind {
                        ResourceKind::Asteroid(_) => 4,
                        ResourceKind::DustCloud(_) => 5,
                    };
                    s.base.harvest_effect = true;
                }
            } else if s.base.mad_anim == -1 || s.base.mad_done {
                s.base.mad_anim = 0;
                s.base.mad_done = true;
                s.base.worker_timer = now;
            }
        }
        4..=6 => {
            u.ships.ship_mut(ship).base.harvest_effect = true;
            collect_resources(u, ship, r.id);
        }
        7 => {
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(ship);
            if let Some(stat) = statics.ship(s.ship_type) {
                if aitrack::stabilize(s, stat, &tw, dt) {
                    s.base.harvest_state = 0;
                }
            }
        }
        _ => {}
    }
}

/// `R1ResourcerHarvestsAsteroid` (0x4ed1d0): resource collectors close in, turn their harvest
/// effect on and collect while holding position (circling bigger asteroids).
fn resourcer_harvests(u: &mut Universe, ship: ShipId, res: Option<ResourceId>) {
    let Some(r) = res.and_then(|r| u.base.resources.get(r).cloned()) else {
        u.ships.ship_mut(ship).base.harvest_state = 0;
        return;
    };
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let Some(stat) = statics.ship(u.ships.ship(ship).ship_type) else { return };
    let rinfo = info(u, r.kind).cloned();
    let approx = rinfo.as_ref().map_or(0.0, |i| i.header.coll.approx_coll_sphere_size);
    let range_of = |u: &Universe| {
        let p = u.ships.ship(ship).posinfo.position;
        let t = Vector::new(st(ld(r.coll_position.x) - ld(p.x)), st(ld(r.coll_position.y) - ld(p.y)), st(ld(r.coll_position.z) - ld(p.z)));
        let d = st(hwc_math::fastmath::fmath_sqrt_double((ld(t.x) * ld(t.x) + ld(t.y) * ld(t.y)) + ld(t.z) * ld(t.z)));
        let rr = ld(d) - ld(approx);
        (t, d, if rr <= 0.0 { 0.0 } else { rr })
    };
    let mut state = u.ships.ship(ship).base.harvest_state;
    if state == 0 {
        state = 1;
        u.ships.ship_mut(ship).base.harvest_state = 1;
    }
    match state {
        1 | 2 => {
            let (_, _, range) = range_of(u);
            if state == 2 || range <= ld(tw.base.asteroid_harvest_range) {
                let s = u.ships.ship_mut(ship);
                s.base.harvest_effect = true;
                s.base.harvest_state = 3;
                return;
            }
            let res_ = fly_to_point_avoiding_objs(u, ship, Some(r.position), 0x17, 0.0, Some(r.velocity));
            if res_ & 1 != 0 {
                if let Some(x) = u.base.resources.get_mut(r.id) {
                    x.not_accessible = 60 << 4;
                }
            }
        }
        3 => {
            let (t, d, range) = range_of(u);
            u.ships.ship_mut(ship).base.harvest_effect = true;
            if ld(tw.base.asteroid_max_harvest_range) < range {
                let s = u.ships.ship_mut(ship);
                s.base.harvest_effect = false;
                s.base.harvest_state = 1;
                return;
            }
            collect_resources(u, ship, r.id);
            let inv = st(1.0 / ld(d));
            let mut dir = Vector::new(st(ld(inv) * ld(t.x)), st(ld(inv) * ld(t.y)), st(ld(inv) * ld(t.z)));
            let acc = f32::from_bits(0x3f7f_bce7);
            let small = matches!(r.kind, ResourceKind::Asteroid(k) if (k as i32) < tw.base.circle_harvest_asteroids_biggerequal_than);
            let s = u.ships.ship_mut(ship);
            aitrack::heading(s, stat, &tw, dt, &mut dir, acc, acc, 0.0, 0);
            if small {
                aitrack::zero_velocity(s, stat, &tw, dt);
                return;
            }
            let right = s.right();
            let v = s.posinfo.velocity;
            if (ld(right.x) * ld(v.x) + ld(right.y) * ld(v.y)) + ld(right.z) * ld(v.z) < ld(tw.base.circle_right_velocity) {
                let f = st(ld(tw.base.circle_right_thrust) * ld(s.nonstat.thrust_strength[2]));
                crate::physics::apply_force(s, f, 2);
            }
            if ld(tw.base.asteroid_harvest_range) < range {
                fly_to_point_avoiding_objs(u, ship, Some(r.position), 0, 0.0, Some(r.velocity));
                return;
            }
            aitrack::zero_forward_velocity(u.ships.ship_mut(ship), stat, dt);
        }
        _ => {}
    }
}

/// `univDeleteDeadResources`: remove exhausted resources; commands harvesting them forget
/// them (`clResourceDied`).
pub fn delete_dead_resources(u: &mut Universe) {
    let dead = std::mem::take(&mut u.base.resources.dead);
    for r in dead {
        u.base.resources.list.retain(|x| x.id != r);
        let todo = u.commands.todo.clone();
        for c in todo {
            if let Some(cmd) = u.commands.get_mut(c) {
                if cmd.collect.resource == Some(r) {
                    cmd.collect.resource = None;
                }
            }
        }
    }
}

/// The multiplayer resource trickle at the start of `univUpdate`: every `regenerateRUrate`
/// mask frames each live player below `regenerateRUsIfUnder` gains `regenerateRUs`.
pub fn resource_trickle(u: &mut Universe) {
    if u.single_player {
        return;
    }
    let tw = &u.tweaks.base;
    if tw.regenerate_ru_rate & u.update_counter != 0 {
        return;
    }
    let (under, amount) = (tw.regenerate_rus_if_under, tw.regenerate_rus);
    for p in 0..u.base.players.len() {
        let pl = &mut u.base.players[p];
        if pl.state == 1 || pl.resource_units >= under {
            continue;
        }
        let before = pl.resource_units;
        pl.resource_units = (before + amount).min(under);
        let gained = pl.resource_units - before;
        let st_ = &mut u.base.stats;
        if st_.players.len() <= p {
            st_.players.resize(p + 1, base::PlayerStats::default());
        }
        st_.players[p].ru_other += gained;
    }
}
