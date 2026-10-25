//! `Salvage.c`: Cataclysm's salvaging. Salvagers (workers with the salvage upgrade, salvage
//! corvettes) latch onto the salvage points of a damaged ship or derelict; once every point
//! has a salvager the group tows the target to the nearest depot (carrier, processor or
//! mothership of the salvagers' player), which takes it in, waits a retrofit time and converts
//! it to its player (`univConvertShipToPlayer`). Only the generic depot process is modelled
//! (not crystals, the Beast infection, the big gun or message pods).

use hwc_math::fpu::{ftol_i32, ld, st};
use hwc_math::Vector;

use crate::aiship::fly_to_point_avoiding_objs;
use crate::aitrack;
use crate::base::{self, special, special2, BaseEvent, CreateOptions};
use crate::command::{self, order};
use crate::object::CommandId;
use crate::dock;
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// Salvage-point dock types.
const DPT_SALVAGE: i32 = 27;
const DPT_SALVAGE_LATCH: i32 = 28;

/// A depot slot (5 dwords at the depot's specifics).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct DepotSlot {
    pub ship: Option<ShipId>,
    /// Start and end of the retrofit (integer seconds, as the original stores them).
    pub start: i32,
    pub end: i32,
    /// 0 new, 1 retrofitting.
    pub state: i32,
    /// Just added: release the salvagers first.
    pub fresh: bool,
}

/// Depot capability (`ship+0x1cc & 0x2000`).
pub const DEPOT_FLAG: u32 = 0x2000;

/// Whether the type keeps a depot (carriers, processors, motherships, the Beast heavy
/// cruiser).
pub fn is_depot_type(t: ShipType) -> bool {
    use ShipType::*;
    matches!(t, sCarrier | bCarrier | kCarrier | tCarrier | dKushanCarrier | sProcessor | bProcessor | p1Mothership | sMothership | bMothership | trGhostship | bHeavyCruiser)
}

/// `salvDepotInit` (0x4f7a80).
pub fn depot_init(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    if is_depot_type(s.ship_type) {
        s.special_flags2 |= DEPOT_FLAG;
        s.base.depot = Some(vec![DepotSlot::default(); 16]);
    }
}

/// `salvIsSalvager`: a ship with a salvage specification.
pub fn is_salvager(t: ShipType) -> bool {
    use ShipType::*;
    matches!(t, sWorker | bWorker | kSalCapCorvette | tSalCapCorvette | p1StandardCorvette)
}

/// Salvage points a target needs filled (`count[Salvage] + count[SalvageLatch]`; -1 if none).
fn points_needed(u: &Universe, target: ShipId) -> i32 {
    let n = u.ships.get(target).and_then(|t| u.statics.ship(t.ship_type)).and_then(|s| s.dock.as_ref()).map_or(0, |d| {
        d.count_of_type[DPT_SALVAGE as usize] as i8 as i32 + d.count_of_type[DPT_SALVAGE_LATCH as usize] as i8 as i32
    });
    if n < 1 {
        -1
    } else {
        n
    }
}

/// `salvTargetDamageOk` (0x4f3190): damaged enough (health at most the class's limit).
pub fn target_damage_ok(u: &Universe, target: ShipId) -> bool {
    let t = u.ships.ship(target);
    if matches!(t.ship_type, ShipType::dMessagePod | ShipType::dBuoy | ShipType::dBigGunUnit | ShipType::dCrystal0 | ShipType::dCrystal1) {
        return true;
    }
    let Some(stat) = u.statics.ship(t.ship_type) else { return false };
    let class = stat.ship_class.map_or(0, |c| c.index());
    let limit = if u.single_player && u.base.options.mission == 8 && u.sp_difficulty == 2 {
        0.75
    } else {
        u.tweaks.base.salvage_damage_limit.get(class).copied().unwrap_or(0.0)
    };
    ld(t.health) <= ld(limit) * ld(stat.health.max_health)
}

/// `salvCanSalvageTarget` (0x4f3290): the reason counters reduced to the answer.
pub fn can_salvage_target(u: &Universe, player: usize, target: ShipId) -> bool {
    let Some(t) = u.ships.get(target) else { return false };
    let Some(stat) = u.statics.ship(t.ship_type) else { return false };
    let ally = t.attributes & base::attributes::DOCK_WITH_ANYONE == 0 && u.players.get(player).is_some_and(|p| p.is_ally(t.player));
    if ally || t.special_flags2 & special2::UNDER_CONSTRUCTION != 0 {
        return false;
    }
    if points_needed(u, target) < 1 {
        return false;
    }
    if t.flags2 & 4 != 0 && !u.base.options.tutorial {
        return false;
    }
    if !crate::construction::player_has_support_units_for_type(u, t.ship_type, player, 0) {
        return false;
    }
    if is_salvager(t.ship_type) && t.base.salvage.target.is_some() {
        return false;
    }
    use ShipType::*;
    if matches!(t.ship_type, sMothership | bMothership | trFloatingCity | trCargoBarge | trMiningBase | trResearchStation | trJunkYardDawg | trJunkYardHQ | trResearchStationBridge | trResearchStationTower | dBeastMothership) {
        return false;
    }
    if u.players[player].race == ShipRace::Somtaaw && !u.single_player && (0x21..0x3c).contains(&t.ship_type.index()) {
        // A Beast type needs a Somtaaw type that converts to it.
        let ok = (0..base::NUM_SHIP_TYPES).filter(|&i| !(0x21..0x3c).contains(&i)).any(|i| {
            ShipType::from_index(i).and_then(|x| u.statics.ship(x)).is_some_and(|s| s.beast_attacks_required != -1 && s.beast_attack_convert_to == Some(t.ship_type))
        });
        if !ok {
            return false;
        }
    }
    let _ = stat;
    true
}

/// `salvFindNearestTarget` (0x4f3810): the target to go for and why there is none
/// (1 = all have their salvagers, 2 = none damaged enough).
fn find_nearest_target(u: &Universe, ship: ShipId, targets: &[ShipId]) -> (Option<ShipId>, u8) {
    let s = u.ships.ship(ship);
    let player = s.player;
    let me = s.posinfo.position;
    let mut status = 0u8;
    let mut status_idx = None;
    let mut best = 1e21f32;
    let mut best_idx = None;
    for (i, &t) in targets.iter().enumerate() {
        if u.ships.get(t).is_none() || !can_salvage_target(u, player, t) {
            continue;
        }
        let needed = points_needed(u, t).max(0);
        let attached = u.ships.ship(t).base.salvagers[player.min(7)];
        if needed == attached {
            if status == 0 {
                status = 1;
                status_idx = Some(i);
            }
            continue;
        }
        if !target_damage_ok(u, t) && !(u.single_player && player == 1) {
            if status == 0 {
                status = 2;
                status_idx = Some(i);
            }
            continue;
        }
        if attached > 0 && attached < needed {
            best_idx = Some(i);
            break;
        }
        let p = u.ships.ship(t).posinfo.position;
        let dx = st(ld(p.x) - ld(me.x));
        let dy = st(ld(p.y) - ld(me.y));
        let dz = st(ld(p.z) - ld(me.z));
        let d2 = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
        if ld(d2) < ld(best) {
            best = d2;
            best_idx = Some(i);
        }
    }
    if let Some(i) = best_idx {
        return (Some(targets[i]), 0);
    }
    if status != 0 {
        return (status_idx.map(|i| targets[i]).filter(|_| false), status);
    }
    (None, 0)
}

/// `salvReserveDockPoint` (0x4f3b10): the first free latch point, else the last free salvage
/// point.
fn reserve_dock_point(u: &mut Universe, ship: ShipId, target: ShipId) -> bool {
    let tt = u.ships.ship(target).ship_type;
    let Some(info) = u.statics.ship(tt).and_then(|s| s.dock.clone()) else { return false };
    let reserved = u.ships.ship(target).base.occupancy.as_ref().map(|o| o.reserved.clone()).unwrap_or_default();
    let mut pick = None;
    let mut last_salvage = None;
    for (i, p) in info.points.iter().enumerate() {
        let t = p.dock_type.map_or(-1, |t| t.index() as i32);
        let free = !reserved.get(i).copied().unwrap_or(false);
        if t == DPT_SALVAGE_LATCH && free {
            pick = Some(i);
            break;
        }
        if t == DPT_SALVAGE && free {
            last_salvage = Some(i);
        }
        pick = last_salvage;
    }
    let Some(i) = pick else { return false };
    let t = info.points[i].dock_type.map(|t| t.index());
    if let Some(o) = u.ships.ship_mut(target).base.occupancy.as_mut() {
        o.reserved[i] = true;
        if let Some(t) = t {
            o.used_per_type[t] = o.used_per_type[t].wrapping_add(1);
        }
    }
    u.ships.ship_mut(ship).base.salvage.dock_point = Some(i);
    true
}

fn release_point(u: &mut Universe, ship: ShipId) {
    let spec = u.ships.ship(ship).base.salvage;
    if let (Some(i), Some(t)) = (spec.dock_point, spec.target) {
        let pt = u.ships.get(t).and_then(|x| u.statics.ship(x.ship_type)).and_then(|s| s.dock.as_ref()).and_then(|d| d.points.get(i)).and_then(|p| p.dock_type).map(|t| t.index());
        if let Some(o) = u.ships.get_mut(t).and_then(|x| x.base.occupancy.as_mut()) {
            if let Some(r) = o.reserved.get_mut(i) {
                *r = false;
            }
            if let Some(pt) = pt {
                o.used_per_type[pt] = o.used_per_type[pt].wrapping_sub(1);
            }
        }
    }
    u.ships.ship_mut(ship).base.salvage.dock_point = None;
}

/// The salvagers clamped to a target (`salvDockedShipIterate`).
fn docked_salvagers(u: &Universe, target: ShipId) -> Vec<ShipId> {
    crate::clamp::slaves_of(u, target).into_iter().filter(|&s| u.ships.get(s).is_some_and(|x| is_salvager(x.ship_type))).collect()
}

/// `salvReset` (0x4f3c90): the salvager lets go of its target.
pub fn reset(u: &mut Universe, ship: ShipId) {
    let Some(s) = u.ships.get(ship) else { return };
    let player = s.player.min(7);
    base::set_disable_reason(u.ships.ship_mut(ship), false, 2);
    u.ships.ship_mut(ship).special_flags2 &= !special2::DOCKING;
    if u.ships.ship(ship).base.clamp.as_ref().is_some_and(|c| c.host.is_some()) {
        crate::clamp::unclamp(u, ship);
    }
    // The beam effect is deleted outright here, self-deleting or not.
    if let Some(e) = crate::effects::slot(u, ship, crate::resource::FX_SLOT_BEAM) {
        crate::effects::delete(u, e);
        crate::effects::set_slot(u, ship, crate::resource::FX_SLOT_BEAM, None);
    }
    u.ships.ship_mut(ship).base.harvest_effect = false;
    if u.ships.ship(ship).base.salvage.dock_point.is_some() {
        release_point(u, ship);
    }
    let spec = u.ships.ship(ship).base.salvage;
    if let Some(t) = spec.target.filter(|t| u.ships.get(*t).is_some()) {
        let tg = u.ships.ship_mut(t);
        tg.base.salvagers[player] = (tg.base.salvagers[player] - 1).max(0);
        if tg.base.salvagers[player] == 0 {
            tg.base.salvage_owner = -1;
        }
        let none_left = docked_salvagers(u, t).is_empty();
        let tg = u.ships.ship_mut(t);
        if tg.base.depot_point != -1 && spec.depot.is_some() && tg.base.builder.is_none() {
            // (The depot slot point is released with the depot.)
            tg.base.depot_point = -1;
        }
        if none_left {
            tg.base.salvage_notify_time = f32::from_bits(0x6258_d727);
            tg.special_flags2 &= !special2::BEING_SALVAGED;
            tg.flags &= !(sof::DISABLED | sof::FORMATION_SETTLED);
        }
        tg.flags |= sof::SELECTABLE;
    }
    let s = u.ships.ship_mut(ship);
    s.base.salvage = base::SalvageSpec::default();
    s.special_flags &= !4;
}

/// `salvStartTractorBeam` (0x4f2ef0): the salvager's tractor beam (its race's tractor-beam
/// effect), owned by the salvager and following its position, orientation and scale but not
/// its velocity (the effect's own velocity is cleared). Kept in the beam slot the harvest
/// effects use (+0x38c).
pub fn start_tractor_beam(u: &mut Universe, ship: ShipId) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(s) = u.ships.get(ship) else { return };
    let lod = fx.tables.tractor_beam.get(s.ship_race.index()).and_then(|l| l.as_deref());
    let mut id = None;
    if let Some(e) = fx.pick(lod) {
        if crate::effects::frequency_ok(u, e) {
            use crate::effects::attach;
            // The ship's scale (+0x33c) is 1.
            id = crate::effects::create(u, e, Some(crate::effects::FxOwner::Ship(ship)), None, None, None, 1.0, attach::POSITION | attach::COORDSYS | attach::NLIPS, &[]);
            if let Some(ef) = id.and_then(|e| u.effects.as_mut().and_then(|fx| fx.world.effects.iter_mut().find(|x| x.id == e))) {
                ef.velocity = Vector::ZERO;
            }
        }
    }
    crate::effects::set_slot(u, ship, crate::resource::FX_SLOT_BEAM, id);
}

/// The command callback when a salvager leaves its special order.
pub fn special_done(u: &mut Universe, ship: ShipId) {
    if u.ships.get(ship).is_some_and(|s| is_salvager(s.ship_type)) {
        reset(u, ship);
    }
}

/// `clSpecial` for salvagers: salvage the targets.
pub fn cl_salvage(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let ships: Vec<ShipId> = ships.iter().copied().filter(|&s| u.ships.get(s).is_some_and(|x| crate::upgrade::has_upgrade(x, crate::upgrade::BIT_SALVAGE))).collect();
    if ships.is_empty() || targets.is_empty() {
        return;
    }
    command::remove_ships_from_doing_stuff(u, &ships);
    for &s in &ships {
        reset(u, s);
        u.ships.ship_mut(s).special_flags |= special::SALVAGER;
    }
    let mut c = command::new_command_pub(ships.clone());
    c.order = order::SPECIAL;
    c.special_targets = targets.to_vec();
    let cid = u.commands.add_pub(c);
    command::init_ships_for_ai(u, &ships);
    command::prepare_ships_pub(u, cid);
}

/// `processSpecialToDo` (0x445010): every ship's special-target routine; done when all are.
pub fn process_special_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let Some(cmd) = u.commands.get(cid).cloned() else { return true };
    let mut done = true;
    for &s in &cmd.selection {
        if u.ships.get(s).is_none() {
            continue;
        }
        let targets: Vec<ShipId> = u.commands.get(cid).map(|c| c.special_targets.clone()).unwrap_or_default();
        done &= if targets.is_empty() { true } else { special_target(u, s, cid, &targets) };
    }
    done
}

/// `salvFindDepot` (0x4f39d0): the player's nearest depot that takes the target.
fn find_depot(u: &Universe, ship: ShipId, target: ShipId) -> Option<ShipId> {
    let player = u.ships.ship(ship).player;
    let tp = u.ships.ship(target).posinfo.position;
    let mut best = f32::MAX;
    let mut found = None;
    for &d in u.ship_list.iter().filter(|&&d| u.ships.get(d).is_some_and(|x| x.player == player && x.special_flags2 & DEPOT_FLAG != 0)).take(16) {
        if !depot_can_accept(u, d, target) {
            continue;
        }
        let p = u.ships.ship(d).posinfo.position;
        let dx = st(ld(tp.x) - ld(p.x));
        let dy = st(ld(tp.y) - ld(p.y));
        let dz = st(ld(tp.z) - ld(p.z));
        let d2 = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
        if ld(d2) < ld(best) {
            best = d2;
            found = Some(d);
        }
    }
    found
}

/// `salvDepotCanAccept` (0x4f3040).
fn depot_can_accept(u: &Universe, depot: ShipId, _target: ShipId) -> bool {
    let d = u.ships.ship(depot);
    if d.special_flags2 & DEPOT_FLAG == 0 || d.special_flags2 & special2::UNDER_CONSTRUCTION != 0 {
        return false;
    }
    if u.single_player && d.ship_type == ShipType::kCarrier {
        return false;
    }
    !(d.ship_race == ShipRace::Beast && !u.single_player && !crate::upgrade::has_upgrade(d, 0x10000))
}

/// `salvFlyToTarget` (0x4f56a0): the towed ship flies itself to the depot.
fn fly_to_depot(u: &mut Universe, target: ShipId, depot: ShipId) -> bool {
    let dp = u.ships.ship(depot).posinfo.position;
    if aitrack::move_reached_destination(u.ships.ship(target), dp, 6000.0) {
        return true;
    }
    let statics = u.statics.clone();
    let t = u.ships.ship(target);
    let Some(stat) = statics.ship(t.ship_type) else { return true };
    let mult = crate::tactics::speed_multiplier(t, stat);
    let max = crate::tactics::ships_max_velocity(t, stat, u.max_velocity_context(t));
    fly_to_point_avoiding_objs(u, target, Some(dp), 0, st(max * ld(st(mult))), None);
    // salvTurnToVelocity.
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(target);
    let mut v = s.posinfo.velocity;
    v.normalize();
    let a = f32::from_bits(0x3f7f_be77);
    aitrack::heading(s, stat, &tw, dt, &mut v, a, a, 0.0, 0);
    false
}

/// `salvSpecialTarget` (0x4f5dd0): one step of a salvager's job. True when it is over.
fn special_target(u: &mut Universe, ship: ShipId, _cid: CommandId, targets: &[ShipId]) -> bool {
    if !crate::upgrade::has_upgrade(u.ships.ship(ship), crate::upgrade::BIT_SALVAGE) {
        return true;
    }
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    // A target that got clamped to something else is given up.
    let spec = u.ships.ship(ship).base.salvage;
    if let Some(t) = spec.target {
        let gone = u.ships.get(t).is_none();
        if gone || (crate::clamp::host_of(u, t).is_some() && spec.state != 0 && spec.state != 1) {
            reset(u, ship);
        }
    }
    let fly_dist = 1000.0f32;
    let player = u.ships.ship(ship).player;
    let spec = u.ships.ship(ship).base.salvage;
    match spec.state {
        0 => {
            if targets.is_empty() && spec.target.is_none() {
                u.ships.ship_mut(ship).base.salvage.state = 0xe;
                return false;
            }
            let (t, status) = find_nearest_target(u, ship, targets);
            match t {
                Some(t) if status == 0 => {
                    let tg = u.ships.ship_mut(t);
                    tg.base.salvagers[player.min(7)] += 1;
                    let s = u.ships.ship_mut(ship);
                    s.base.salvage.target = Some(t);
                    s.base.salvage.state = 1;
                }
                _ => {
                    if status == 0 {
                        u.ships.ship_mut(ship).base.salvage.state = 0xe;
                        return false;
                    }
                    // Wait near the targets.
                    if let Some(&t) = targets.iter().find(|&&t| u.ships.get(t).is_some()) {
                        let p = u.ships.ship(t).posinfo.position;
                        if aitrack::within_distance(u.ships.ship(ship), p, st(ld(fly_dist) + ld(fly_dist))) {
                            let tw = u.tweaks.clone();
                            let dt = u.phys_time_elapsed;
                            let s = u.ships.ship_mut(ship);
                            if let Some(stat) = statics.ship(s.ship_type) {
                                aitrack::stabilize(s, stat, &tw, dt);
                                aitrack::zero_velocity(s, stat, &tw, dt);
                            }
                        } else {
                            fly_to_point_avoiding_objs(u, ship, Some(p), 2, 0.0, None);
                        }
                    }
                }
            }
            false
        }
        1 => {
            let t = spec.target.expect("target");
            let p = u.ships.ship(t).posinfo.position;
            fly_to_point_avoiding_objs(u, ship, Some(p), 2, 0.0, None);
            if !aitrack::within_distance(u.ships.ship(ship), p, fly_dist) {
                return false;
            }
            let owner = u.ships.ship(t).base.salvage_owner;
            if owner != -1 && owner != player as i32 {
                let tw = u.tweaks.clone();
                let dt = u.phys_time_elapsed;
                let s = u.ships.ship_mut(ship);
                if let Some(stat) = statics.ship(s.ship_type) {
                    aitrack::stabilize(s, stat, &tw, dt);
                    aitrack::zero_velocity(s, stat, &tw, dt);
                }
                return false;
            }
            u.ships.ship_mut(t).base.salvage_owner = player as i32;
            u.ships.ship_mut(ship).base.salvage.state = 2;
            false
        }
        2 => {
            let t = spec.target.expect("target");
            if reserve_dock_point(u, ship, t) {
                u.ships.ship_mut(ship).base.salvage.state = 3;
            } else {
                reset(u, ship);
            }
            false
        }
        3..=5 => {
            let t = spec.target.expect("target");
            let Some(pi) = spec.dock_point else { return false };
            let Some(p) = statics.ship(u.ships.ship(t).ship_type).and_then(|s| s.dock.as_ref()).and_then(|d| d.points.get(pi)).cloned() else { return false };
            let step_done = match spec.state {
                3 => dock::fly_to_cone_inside_pub(u, ship, t, &p),
                4 => dock::fly_to_latch_standoff_pub(u, ship, t, &p),
                _ => {
                    let own = statics.ship(u.ships.ship(ship).ship_type).and_then(|s| s.dock.as_ref()).and_then(|d| d.center.and_then(|c| d.points.get(c))).cloned().unwrap_or_default();
                    dock::fly_to_cone_origin_latch_pub(u, ship, &own, t, &p)
                }
            };
            if step_done {
                let s = u.ships.ship_mut(ship);
                match spec.state {
                    3 => {
                        s.special_flags2 |= special2::DOCKING;
                        s.base.salvage.latching = true;
                        s.base.salvage.state = 4;
                    }
                    4 => {
                        if crate::resource::is_worker(s.ship_type) && s.base.mad_anim != 3 {
                            s.base.mad_anim = 3;
                            s.base.mad_done = true;
                        }
                        s.base.salvage.state = 5;
                    }
                    _ => s.base.salvage.state = 7,
                }
            }
            // The target slows down while salvagers approach.
            {
                let tg = u.ships.ship_mut(t);
                tg.posinfo.velocity = Vector::new(st(ld(tg.posinfo.velocity.x) * 0.3), st(ld(tg.posinfo.velocity.y) * 0.3), st(ld(tg.posinfo.velocity.z) * 0.3));
                tg.rotinfo.rotspeed =
                    Vector::new(st(ld(tg.rotinfo.rotspeed.x) * 0.3), st(ld(tg.rotinfo.rotspeed.y) * 0.3), st(ld(tg.rotinfo.rotspeed.z) * 0.3));
            }
            false
        }
        7 | 8 => {
            let t = spec.target.expect("target");
            if spec.state == 7 {
                crate::clamp::clamp_obj_to_obj(u, ship, t);
                let pt = spec.dock_point.and_then(|i| statics.ship(u.ships.ship(t).ship_type).and_then(|s| s.dock.as_ref()).and_then(|d| d.points.get(i)).and_then(|p| p.dock_type)).map_or(-1, |x| x.index() as i32);
                if pt != DPT_SALVAGE_LATCH {
                    u.ships.ship_mut(ship).base.harvest_effect = true;
                    start_tractor_beam(u, ship);
                }
                u.ships.ship_mut(ship).base.salvage.latching = false;
                command::cl_halt(u, &[t]);
            }
            let depot = find_depot(u, ship, t);
            for s in docked_salvagers(u, t) {
                u.ships.ship_mut(s).base.salvage.depot = depot;
            }
            u.ships.ship_mut(ship).base.salvage.depot = depot;
            if depot.is_none() {
                reset(u, ship);
                u.ships.ship_mut(ship).base.salvage.state = 0;
                return false;
            }
            u.ships.ship_mut(t).base.salvage_depot_check = now;
            u.ships.ship_mut(ship).base.salvage.state = 9;
            false
        }
        9 | 10 => {
            let t = spec.target.expect("target");
            let needed = points_needed(u, t);
            let attached = crate::clamp::slaves_of(u, t).iter().filter(|&&s| u.ships.get(s).is_some_and(|x| x.player == player && is_salvager(x.ship_type))).count() as i32;
            if attached < needed {
                let tg = u.ships.ship_mut(t);
                if spec.state == 10 {
                    u.ships.ship_mut(ship).base.salvage.state = 9;
                    return false;
                }
                tg.flags |= sof::SELECTABLE;
                tg.special_flags2 &= !special2::BEING_SALVAGED;
                return false;
            }
            if spec.state == 9 {
                u.ships.ship_mut(t).flags &= !sof::SELECTABLE;
                u.ships.ship_mut(ship).base.salvage.state = 10;
                return false;
            }
            // Every point is taken: tow the target to the depot.
            let Some(depot) = spec.depot.filter(|d| u.ships.get(*d).is_some()) else {
                reset(u, ship);
                return false;
            };
            command::remove_ships_from_doing_stuff(u, &[t]);
            let tg = u.ships.ship_mut(t);
            tg.flags &= !sof::SELECTABLE;
            tg.flags |= sof::DISABLED | sof::FORMATION_SETTLED;
            tg.special_flags &= !0x100;
            tg.special_flags2 |= special2::BEING_SALVAGED;
            if tg.special_flags & special::SALVAGE_TOWING == 0 {
                if fly_to_depot(u, t, depot) {
                    for s in docked_salvagers(u, t) {
                        u.ships.ship_mut(s).base.salvage.state = 0xb;
                    }
                    u.ships.ship_mut(ship).base.salvage.state = 0xb;
                }
                u.ships.ship_mut(t).special_flags |= special::SALVAGE_TOWING;
            }
            false
        }
        0xb => {
            let t = spec.target.expect("target");
            let tg = u.ships.ship_mut(t);
            tg.posinfo.velocity = Vector::ZERO;
            tg.rotinfo.rotspeed = Vector::ZERO;
            u.ships.ship_mut(ship).base.salvage.state = 0xc;
            false
        }
        0xc => {
            if let Some(d) = spec.depot {
                depot_add_ship(u, ship, d);
            }
            u.ships.ship_mut(ship).base.salvage.state = 0xd;
            false
        }
        0xd => false,
        0xe => {
            reset(u, ship);
            true
        }
        _ => true,
    }
}

/// `salvDepotAddShip` (0x4f5350): the target takes a depot slot; the retrofit time is its
/// length over `twSalTransSpeed` (capped by `twSalTransMaxTime`), in whole seconds.
fn depot_add_ship(u: &mut Universe, ship: ShipId, depot: ShipId) {
    let Some(t) = u.ships.ship(ship).base.salvage.target else { return };
    if u.ships.get(depot).and_then(|d| d.base.depot.as_ref()).is_none_or(|d| d.iter().any(|s| s.ship == Some(t))) {
        return;
    }
    let len = u.ships.get(t).and_then(|x| u.statics.ship(x.ship_type)).map_or(0.0, |s| s.header.coll.forward_length);
    let tw = &u.tweaks.base;
    let mut time = st(ld(len) / ld(tw.sal_trans_speed));
    if ld(tw.sal_trans_max_time) <= ld(time) {
        time = tw.sal_trans_max_time;
    }
    let now = u.total_time_elapsed;
    let Some(slots) = u.ships.ship_mut(depot).base.depot.as_mut() else { return };
    let Some(slot) = slots.iter_mut().find(|s| s.ship.is_none()) else { return };
    *slot = DepotSlot { ship: Some(t), start: ftol_i32(ld(now)), end: ftol_i32(ld(now) + ld(time)), state: 0, fresh: true };
    u.ships.ship_mut(t).base.builder = Some(depot);
    // No salvage order goes for it any more.
    let todo = u.commands.todo.clone();
    for c in todo {
        if let Some(cmd) = u.commands.get_mut(c) {
            if cmd.order == order::SPECIAL {
                cmd.special_targets.retain(|&x| x != t);
            }
        }
    }
    for s in docked_salvagers(u, t) {
        base::set_disable_reason(u.ships.ship_mut(s), true, 2);
    }
}

/// `salvDepotHouseKeep` (0x4f7c90): release the salvagers of new arrivals, then retrofit.
pub fn depot_housekeep(u: &mut Universe, depot: ShipId) {
    let Some(slots) = u.ships.get(depot).and_then(|d| d.base.depot.clone()) else { return };
    for (i, slot) in slots.iter().enumerate() {
        let Some(t) = slot.ship else { continue };
        if slot.fresh {
            // salvDepotReleaseSlot: salvagers with nothing left to salvage stop.
            for s in docked_salvagers(u, t).into_iter().rev() {
                let targets = u.ships.ship(s).command.and_then(|c| u.commands.get(c)).map(|c| c.special_targets.clone()).unwrap_or_default();
                if find_nearest_target(u, s, &targets).0.is_none() {
                    command::remove_ships_from_doing_stuff(u, &[s]);
                }
                reset(u, s);
            }
            if let Some(sl) = u.ships.ship_mut(depot).base.depot.as_mut() {
                sl[i].fresh = false;
            }
            continue;
        }
        depot_process_generic(u, depot, i);
    }
}

/// `salvDepotProcessGeneric` (0x4f4800): after the retrofit time the ship changes owner.
fn depot_process_generic(u: &mut Universe, depot: ShipId, i: usize) {
    let now = u.total_time_elapsed;
    let Some(slot) = u.ships.ship(depot).base.depot.as_ref().map(|d| d[i]) else { return };
    let Some(t) = slot.ship else { return };
    if u.ships.get(t).is_none() {
        u.ships.ship_mut(depot).base.depot.as_mut().expect("depot")[i] = DepotSlot::default();
        return;
    }
    if slot.state == 0 {
        u.ships.ship_mut(depot).base.depot.as_mut().expect("depot")[i].state = 1;
        return;
    }
    if !(ld(slot.end as f32) <= ld(now)) {
        return;
    }
    let player = u.ships.ship(depot).player;
    let new_type = converted_ship_type(u, t, depot);
    if let Some(nt) = new_type {
        if let Some(n) = convert_ship_to_player(u, t, nt, player) {
            u.ships.ship_mut(n).base.builder = None;
            u.base.stats.add_ship_salvaged(nt, player);
            u.base.events.push(BaseEvent::Salvaged { ship: n, player });
        }
    }
    u.ships.ship_mut(depot).base.depot.as_mut().expect("depot")[i] = DepotSlot::default();
}

/// `salvGetConvertedShipType` (0x4f7b80): the type the salvaged ship becomes for the depot's
/// race.
fn converted_ship_type(u: &Universe, target: ShipId, depot: ShipId) -> Option<ShipType> {
    let tt = u.ships.ship(target).ship_type;
    if tt == ShipType::dBigGunUnit {
        return Some(tt);
    }
    let d = u.ships.ship(depot);
    let beast_type = (0x21..0x3c).contains(&d.ship_type.index());
    match d.ship_race {
        ShipRace::Beast if beast_type => u.statics.ship(tt).and_then(|s| if s.beast_attacks_required != -1 { s.beast_attack_convert_to } else { None }),
        _ => {
            if !(0x21..0x3c).contains(&tt.index()) {
                return Some(tt);
            }
            (0..base::NUM_SHIP_TYPES)
                .filter(|&i| !(0x21..0x3c).contains(&i))
                .filter_map(ShipType::from_index)
                .find(|&x| u.statics.ship(x).is_some_and(|s| s.beast_attacks_required != -1 && s.beast_attack_convert_to == Some(tt)))
        }
    }
}

/// `univConvertShipToPlayer` (0x53e9e0): a new ship of `new_type` owned by `player` takes the
/// old one's place, pose, health, cargo and upgrades; the old one leaves the game.
pub fn convert_ship_to_player(u: &mut Universe, old: ShipId, new_type: ShipType, player: usize) -> Option<ShipId> {
    let o = u.ships.ship(old).clone();
    let id = base::create_ship(u, new_type, player, o.posinfo.position, CreateOptions::default()).ok()?;
    let n = u.ships.ship_mut(id);
    for (g, og) in n.guns.iter_mut().zip(o.guns.iter()) {
        g.level = og.level;
    }
    crate::construction::unit_cap_create_ship(u, id);
    crate::damage::really_delete_ship(u, old);
    let statics = u.statics.clone();
    let stat = statics.ship(new_type)?;
    let n = u.ships.ship_mut(id);
    n.posinfo.velocity = o.posinfo.velocity;
    n.posinfo.force = o.posinfo.force;
    n.rotinfo = o.rotinfo;
    n.health = if stat.health.max_health < o.health { stat.health.max_health } else { o.health };
    n.posinfo.is_moving = o.posinfo.is_moving;
    n.base.resources = o.base.resources.min(stat.max_resources);
    n.armour_level = o.armour_level;
    n.upgrade_flags = o.upgrade_flags;
    crate::physics::update_obj_rot_info(n, stat);
    Some(id)
}

/// `salvShipDied`: a salvager's target died.
pub fn ship_died(u: &mut Universe, id: ShipId) {
    let salvagers: Vec<ShipId> = u.ships.iter().filter(|s| s.base.salvage.target == Some(id) || s.base.salvage.depot == Some(id)).map(|s| s.id).collect();
    for s in salvagers {
        if u.ships.ship(s).base.salvage.target == Some(id) {
            u.ships.ship_mut(s).base.salvage.dock_point = None;
            reset(u, s);
        } else {
            u.ships.ship_mut(s).base.salvage.depot = None;
        }
    }
}

/// `salvDepotDied`: ships in the depot's slots are released.
pub fn depot_died(u: &mut Universe, id: ShipId) {
    let Some(slots) = u.ships.get(id).and_then(|d| d.base.depot.clone()) else { return };
    for s in slots.into_iter().filter_map(|s| s.ship) {
        if let Some(t) = u.ships.get_mut(s) {
            t.base.builder = None;
            t.flags &= !(sof::DISABLED | sof::FORMATION_SETTLED);
            t.flags |= sof::SELECTABLE;
            t.special_flags2 &= !special2::BEING_SALVAGED;
        }
    }
}

/// Whether a class can be salvaged by default limits (for callers choosing targets).
pub fn class_limit(u: &Universe, c: ShipClass) -> f32 {
    u.tweaks.base.salvage_damage_limit[c.index()]
}

/// Whether the ship is held in a salvage depot (ship +0x3ec): it neither fires nor can be
/// fired at with missiles.
pub fn is_in_depot(u: &Universe, id: ShipId) -> bool {
    u.ships.iter().any(|d| d.base.depot.as_ref().is_some_and(|slots| slots.iter().any(|s| s.ship == Some(id))))
}

/// The salvagers' special-target routine for the ship-module dispatch.
pub fn special_target_pub(u: &mut Universe, ship: ShipId, cid: CommandId, targets: &[ShipId]) -> bool {
    special_target(u, ship, cid, targets)
}
