//! Hyperspace jumps in a multiplayer game (`MG_Hyperspace`): the player pays a jump cost by
//! ship and distance (`hyperspaceCost`, the `hyperspace` lines of `Tweak.script`), the ships
//! stop, leave through the window, wait in the ether at least
//! `TW_MULTIPLAYER_HYPERSPACE_WAIT_IN_ETHER_TIME` seconds and come out at the destination
//! (`clMpHyperspace`, `clMpHyperspaceThese`, `ChangeOrderToHyperSpace`,
//! `processMpHyperspaceingToDo`). The leaving and arriving are the campaign's mid-level jumps
//! (`spHyperspaceSelectionOut` / `spHyperspaceSelectionIn`, `UpdateMidLevelHyperspacingShips`),
//! run every update of a game with hyperspace on.
//!
//! The sensors-manager pings the original raises for the ships going and coming
//! (`hyperspaceOutPingTimeOut`, `hyperspaceInPingTimeOut`) and the arrival's battle chatter are
//! presentation and are not modelled.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::campaign::HyperState;
use crate::command::{self, order};
use crate::object::{sof, CommandId, ShipId};
use crate::statics::{ShipClass, ShipType};
use crate::universe::Universe;

/// `COMMAND_MP_HYPERSPACEING`.
pub const ORDER: u16 = 10;

/// The disable reason a ship carries from the order until it has arrived.
pub const DISABLE_REASON: usize = 6;

/// One ship type's jump cost (`hyperspace <ship> mincost mindistance RUperMeter maxcost`,
/// 0xa47340, 0x14 bytes a type): a type without a line cannot jump (`+0x10`).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct JumpCost {
    pub min_cost: f32,
    pub ru_per_meter: f32,
    pub min_distance: f32,
    pub max_cost: f32,
}

/// The multiplayer hyperspace rules and the ships on their way.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MpHyperspace {
    /// The cost per ship type (by [`ShipType::index`]); `None`: the type cannot jump.
    pub costs: Vec<Option<JumpCost>>,
    /// `TW_MULTIPLAYER_HYPERSPACE_WAIT_IN_ETHER_TIME` (0xa4729c).
    pub ether_wait: f32,
    /// The hyperspace window's timings (`singleplayer.script`, shared with the campaign).
    pub tweaks: crate::mission::SpTweaks,
    /// The ships leaving and in hyperspace (`ShipsToHyperspace`, `ShipsInHyperspace`).
    pub hyper: HyperState,
}

impl MpHyperspace {
    /// The `hyperspace` lines and the ether wait of `Tweak.script`, the window timings of
    /// `singleplayer.script`.
    pub fn load(src: &dyn crate::statics::DataSource) -> Self {
        let mut h = MpHyperspace { costs: vec![None; ShipType::COUNT], ether_wait: 20.0, tweaks: crate::mission::SpTweaks::load(src), hyper: HyperState::default() };
        let Some(lines) = crate::level::read_lines(src, "Tweak.script") else { return h };
        for (k, v) in lines {
            match k.as_str() {
                "TW_MULTIPLAYER_HYPERSPACE_WAIT_IN_ETHER_TIME" => {
                    h.ether_wait = crate::statics::cscan::scan_f32(&v).unwrap_or(h.ether_wait);
                }
                "hyperspace" => {
                    // `scriptSetHyperspaceCostCB`: "%s %f %f %f %f" = name, mincost,
                    // mindistance, RUperMeter, maxcost.
                    let mut w = v.split_whitespace();
                    let Some(t) = w.next().and_then(ShipType::from_name) else { continue };
                    let f: Vec<f32> = w.take(4).filter_map(crate::statics::cscan::scan_f32).collect();
                    if f.len() == 4 {
                        h.costs[t.index()] = Some(JumpCost { min_cost: f[0], min_distance: f[1], ru_per_meter: f[2], max_cost: f[3] });
                    }
                }
                _ => {}
            }
        }
        h
    }

    fn cost_of(&self, t: ShipType) -> Option<JumpCost> {
        self.costs.get(t.index()).copied().flatten()
    }
}

/// The state of a jump command (`+0x314`) and its clock (`+0x310`).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct JumpOrder {
    /// 0 stopping, 1 leaving, 2 in the ether, 3 arriving.
    pub state: u32,
    pub time: f32,
}

/// `hyperspaceCost` (0x50d4a0): the ships' costs summed. The distance left over the first
/// ship's free distance is what the next ship is charged by, as the original does.
pub fn hyperspace_cost(u: &Universe, distance: f32, ships: &[ShipId]) -> f64 {
    let mut total = 0.0f64;
    let mut d = ld(distance);
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        let c = u.mp.hyperspace.cost_of(s.ship_type).unwrap_or_default();
        let mut cost = if ld(c.min_distance) <= d {
            d -= ld(c.min_distance);
            d * ld(c.ru_per_meter) + ld(c.min_cost)
        } else {
            ld(c.min_cost)
        };
        if ld(c.max_cost) < cost {
            cost = ld(c.max_cost);
        }
        total += cost;
    }
    total
}

/// `makeSelectionHyperspaceCapable` (0x50d3c0): the ships that can jump: their type has a
/// cost line and nothing clamped to them is still being built.
pub fn hyperspace_capable(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    ships
        .iter()
        .copied()
        .filter(|&id| {
            let Some(s) = u.ships.get(id) else { return false };
            let building = s.base.clamp.as_ref().and_then(|c| c.slaves.as_ref()).is_some_and(|sl| {
                sl.iter().any(|&c| u.ships.get(c).is_some_and(|c| c.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0))
            });
            !building && u.mp.hyperspace.cost_of(s.ship_type).is_some()
        })
        .collect()
}

/// The distance `from` → `to` as `clMpHyperspace` measures it.
fn distance(from: Vector, to: Vector) -> f32 {
    let (dx, dy, dz) = (ld(from.x) - ld(to.x), ld(from.y) - ld(to.y), ld(from.z) - ld(to.z));
    st(fsqrt(dx * dx + dy * dy + dz * dz))
}

/// What the jump costs the player, or `None` when no ship of the selection can jump.
pub fn jump_cost(u: &Universe, ships: &[ShipId], from: Vector, to: Vector) -> Option<i32> {
    let s = hyperspace_capable(u, ships);
    (!s.is_empty()).then(|| hyperspace_cost(u, distance(from, to), &s) as i32)
}

/// `clWrapMpHyperspace` + `clMpHyperspace` (0x443000): the capable ships of `ships` jump from
/// around `from` to around `to` if their player can pay; true when they went. The ships keep
/// their places relative to `from`.
pub fn cl_mp_hyperspace(u: &mut Universe, ships: &[ShipId], from: Vector, to: Vector) -> bool {
    if u.base.options.flags & crate::multiplayer::mg::HYPERSPACE == 0 {
        return false;
    }
    let ships = hyperspace_capable(u, ships);
    let Some(&first) = ships.first() else { return false };
    let cost = hyperspace_cost(u, distance(from, to), &ships) as i32;
    let player = u.ships.ship(first).player;
    if player < u.mp.num_players {
        if u.base.players[player].resource_units < cost {
            return false;
        }
        u.base.players[player].resource_units -= cost;
        if let Some(s) = u.base.stats.players.get_mut(player) {
            s.ru_spent += cost;
        }
    }
    for &id in &ships {
        command::ship_has_just_disappeared(u, id);
        // `clWrapMpHyperspace`: the ships cannot be ordered until they are out.
        crate::base::set_disable_reason(u.ships.ship_mut(id), true, DISABLE_REASON);
    }
    let dest_of = |u: &Universe, lead: ShipId| {
        let p = u.ships.ship(lead).posinfo.position;
        Vector::new(st(ld(p.x) - ld(from.x) + ld(to.x)), st(ld(p.y) - ld(from.y) + ld(to.y)), st(ld(p.z) - ld(from.z) + ld(to.z)))
    };
    // Commands whose ships all jump change to the jump (`TheseShipsAreInSelection`).
    let mut left = ships.clone();
    for cid in u.commands.todo.clone() {
        let Some(c) = u.commands.get(cid) else { continue };
        if c.selection.is_empty() || c.order == order::BUILDING_SHIP || !c.selection.iter().all(|s| left.contains(s)) {
            continue;
        }
        let sel = c.selection.clone();
        left.retain(|s| !sel.contains(s));
        let dest = dest_of(u, sel[0]);
        change_order_to_hyperspace(u, cid, dest);
        if left.is_empty() {
            return true;
        }
    }
    let dest = dest_of(u, left[0]);
    cl_mp_hyperspace_these(u, &left, dest);
    true
}

/// `clMpHyperspaceThese` (0x442ea0): a command for the ships (their own if they are one).
fn cl_mp_hyperspace_these(u: &mut Universe, ships: &[ShipId], dest: Vector) {
    let same = u.commands.iter().find(|c| c.selection.len() == ships.len() && c.selection.iter().all(|s| ships.contains(s))).map(|c| c.id);
    if let Some(cid) = same {
        change_order_to_hyperspace(u, cid, dest);
        return;
    }
    command::remove_ships_from_doing_stuff(u, ships);
    let mut cmd = command::new_command_pub(ships.to_vec());
    cmd.order = ORDER;
    cmd.waypoint_mode = 0;
    cmd.waypoints = vec![dest];
    cmd.waypoint_index = 0;
    cmd.mp_hyperspace = Some(JumpOrder::default());
    let cid = u.commands.add_pub(cmd);
    for &id in ships {
        command::init_ship_ai_pub(u, id);
    }
    command::prepare_ships_pub(u, cid);
}

/// `ChangeOrderToHyperSpace` (0x442bb0).
fn change_order_to_hyperspace(u: &mut Universe, cid: CommandId, dest: Vector) {
    if u.commands.get(cid).is_some_and(|c| c.order == ORDER) {
        // "A little wierd: ships that were hyperspacing were told to hyperspace again."
        return;
    }
    command::free_last_order_pub(u, cid);
    let Some(c) = u.commands.get_mut(cid) else { return };
    c.attributes &= !(command::attr::PROTECTING | command::attr::PASSIVE_ATTACKING);
    c.protect.clear();
    c.attack_targets.clear();
    c.order = ORDER;
    c.waypoint_index = 0;
    c.waypoint_mode = 0;
    c.waypoints = vec![dest];
    c.mp_hyperspace = Some(JumpOrder::default());
    let sel = c.selection.clone();
    for id in sel {
        command::init_ship_ai_pub(u, id);
    }
    command::prepare_ships_pub(u, cid);
}

/// `needToWaitForThisShip` (0x43f0a0): a mothership waits for a ship it is launching that is
/// already outside. (The original's Advance Support Frigate rules are for Homeworld's ships.)
fn need_to_wait(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    if u.statics.ship(s.ship_type).and_then(|st| st.ship_class) != Some(ShipClass::Mothership) {
        return false;
    }
    u.commands.iter().any(|c| c.order == order::LAUNCH_SHIP && c.launch_from == Some(id) && c.selection.first().and_then(|&l| u.ships.get(l)).is_some_and(|l| l.coll_my_blob.is_some()))
}

/// `processMpHyperspaceingToDo` (0x43f170): true once every ship has arrived.
pub fn process(u: &mut Universe, cid: CommandId) -> bool {
    let Some(c) = u.commands.get(cid).cloned() else { return false };
    let mut jo = c.mp_hyperspace.unwrap_or_default();
    let sel = c.selection.clone();
    let now = u.total_time_elapsed;
    for &id in &sel {
        u.ships.ship_mut(id).posinfo.is_moving = crate::object::ISMOVING_MOVING;
    }
    let done = match jo.state {
        0 => {
            // Stop, wait for the ships being launched, then leave.
            let statics = u.statics.clone();
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let mut moving = false;
            for &id in &sel {
                let s = u.ships.ship_mut(id);
                let Some(stat) = statics.ship(s.ship_type) else { continue };
                if crate::aitrack::zero_rotation_anywhere(s, stat, &tw, dt) && crate::aitrack::zero_velocity(s, stat, &tw, dt) {
                    s.rotinfo.rotspeed = Vector::ZERO;
                    s.posinfo.velocity = Vector::ZERO;
                } else {
                    moving = true;
                }
            }
            if sel.iter().any(|&id| need_to_wait(u, id)) {
                return false;
            }
            if !moving {
                let mh = &mut u.mp.hyperspace;
                let (mut hy, tw) = (std::mem::take(&mut mh.hyper), mh.tweaks.clone());
                crate::campaign::hyperspace_selection_out_hs(u, &mut hy, &tw, &sel);
                u.mp.hyperspace.hyper = hy;
                jo = JumpOrder { state: 1, time: now };
            }
            false
        }
        1 => {
            // Out once the first ship is in hyperspace.
            if sel.first().is_some_and(|&id| u.ships.ship(id).flags & sof::HYPERSPACE != 0) {
                jo = JumpOrder { state: 2, time: st(ld(u.mp.hyperspace.ether_wait) + ld(now)) };
            }
            false
        }
        2 => {
            if jo.time <= now {
                let d = c.waypoints.get(c.waypoint_index.max(0) as usize).copied().unwrap_or(Vector::ZERO);
                // The heading of the first ship around z, in degrees.
                let h = u.ships.ship(sel[0]).rotinfo.coordsys.col(2);
                let x = st(ld(h.x) + 0.0);
                let x = if 1.0 <= f64::from(x) {
                    1.0
                } else if f64::from(x) <= -1.0 {
                    -1.0
                } else {
                    x
                };
                let mut deg = st(ld(x).acos() * 57.295_777_918_682_05);
                if ld(h.y) + 0.0 < 0.0 {
                    deg = st(360.0 - ld(deg));
                }
                let mut hy = std::mem::take(&mut u.mp.hyperspace.hyper);
                crate::campaign::hyperspace_selection_in_hs(u, &mut hy, &sel, [d.x, d.y, d.z, deg]);
                u.mp.hyperspace.hyper = hy;
                jo = JumpOrder { state: 3, time: now };
            }
            false
        }
        3 => {
            // Done when every ship is out of the window.
            let mut all = true;
            for &id in &sel {
                if u.mp.hyperspace.hyper.get(id).midlevel_in {
                    all = false;
                } else {
                    crate::base::set_disable_reason(u.ships.ship_mut(id), false, DISABLE_REASON);
                }
            }
            if all {
                for &id in &sel {
                    crate::base::set_disable_reason(u.ships.ship_mut(id), false, DISABLE_REASON);
                }
            }
            all
        }
        _ => false,
    };
    if let Some(c) = u.commands.get_mut(cid) {
        c.mp_hyperspace = Some(jo);
    }
    done
}

/// `UpdateMidLevelHyperspacingShips` in a multiplayer game with hyperspace on (`univUpdate`).
pub fn update(u: &mut Universe) {
    if u.single_player || !u.mp.active || u.base.options.flags & crate::multiplayer::mg::HYPERSPACE == 0 {
        return;
    }
    let mh = &mut u.mp.hyperspace;
    if mh.hyper.to_hyperspace.is_empty() && !mh.hyper.in_hyperspace.iter().any(|&id| mh.hyper.get(id).midlevel_in) {
        return;
    }
    let (mut hy, tw) = (std::mem::take(&mut mh.hyper), mh.tweaks.clone());
    crate::campaign::update_midlevel_hs(u, &mut hy, &tw, 0);
    u.mp.hyperspace.hyper = hy;
}
