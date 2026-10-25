//! The base-layer parts of `univShipThink` (0x53f850) and the per-type housekeeping the
//! universe update runs for every ship: health regeneration (`univShipRegenerateHealth`, with
//! the Somtaaw mothership sharing its repair among damaged modules by the player's repair
//! priorities), upgrades finishing, module and depot housekeeping, salvager bookkeeping.

use hwc_math::fpu::{ld, st};

use crate::object::ShipId;
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// The first steps of `univShipThink` (0x53f850): the dock notification (every ship drops its
/// references to a ship that docked), the Beast infection check (a converted ship stops
/// here: returns false), the queued attack, the sentinels' field timer and the EMP flicker.
pub fn before_regeneration(u: &mut Universe, id: ShipId) -> bool {
    if u.ships.ship(id).flags2 & crate::object::flags2::DOCK_NOTIFY != 0 {
        let others = u.ship_list.clone();
        for o in others {
            if u.ships.get(o).is_some() {
                crate::ships::remove_ship_refs_docked(u, o, id);
            }
        }
        if let Some(s) = u.ships.get_mut(id) {
            s.flags2 &= !crate::object::flags2::DOCK_NOTIFY;
        }
    }
    if crate::ships::beast::check_infection_complete(u, id) {
        return false;
    }
    let Some(s) = u.ships.get_mut(id) else { return false };
    if let Some(t) = s.queued_attack.take() {
        if u.ships.get(t).is_some() {
            crate::command::cl_attack(u, &[id], &[t]);
        }
    }
    crate::ships::sentinel::think(u, id);
    crate::disable::flicker_update(u, id);
    true
}

/// The steps of `univShipThink` after the gun bursts: disabling, the per-player visibility of
/// stealthy types (every 16 frames) and the automatic behaviour.
pub fn after_bursts(u: &mut Universe, id: ShipId) {
    crate::disable::update(u, id);
    let Some(s) = u.ships.get(id) else { return };
    let caps = u.statics.ship(s.ship_type).map_or(0, |st| st.capability_flags);
    if caps & crate::visibility::CAP_PER_PLAYER_VISIBILITY != 0 && u.update_counter & 15 == u32::from(s.id.0) & 15 {
        crate::visibility::update(u, id);
    }
    if u.ships.get(id).is_some_and(|s| s.flags & crate::object::sof::DEAD == 0) {
        crate::autobehaviour::update(u, id);
    }
}

/// The base-layer steps of `univShipThink` for one ship, in the original's order (after the
/// retaliation queue and before the gun-burst bookkeeping).
pub fn ship_think(u: &mut Universe, id: ShipId) {
    regenerate_health(u, id);
    let now = u.total_time_elapsed;
    let s = u.ships.ship_mut(id);
    if s.base.upgrading && s.base.upgrade_end < now {
        s.base.upgrading = false;
        crate::base::set_disable_reason(s, false, 0);
        crate::upgrade::apply_upgrades(u, id);
    }
}

/// `univShipRegenerateHealth` (0x53e460): ships with the repair upgrade (and all Beast ships)
/// regain `repairDamage` every `repairTime` seconds (the combat values while attacked or
/// attacking). A Somtaaw mothership first repairs its damaged modules, sharing the repair
/// among the modules of the highest priority group.
pub fn regenerate_health(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let combat = s.special_flags & crate::base::special::IN_COMBAT_REPAIR != 0 || s.recently_attacked != 0 || s.ship_is_attacking;
    let (interval, amount) = if combat { (stat.repair_combat_time, stat.repair_combat_damage) } else { (stat.repair_time, stat.repair_damage) };
    let able = crate::upgrade::has_upgrade(s, crate::upgrade::BIT_REPAIR) || s.ship_race == ShipRace::Beast;
    if !able || !(0.0 < amount) || s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0 {
        return;
    }
    if u.base.options.repair_bots && stat.ship_race == ShipRace::Somtaaw {
        crate::ships::repair_droid::update_for_ship(u, id);
    }
    let s = u.ships.ship(id);
    let component = stat.ship_class == Some(crate::statics::ShipClass::Component);
    if component || (stat.health.max_health <= s.health && s.ship_type != ShipType::sMothership) {
        u.ships.ship_mut(id).base.last_regen = now;
        return;
    }
    if !(ld(interval) <= ld(now) - ld(s.base.last_regen)) {
        return;
    }
    let is_ms = s.ship_type == ShipType::sMothership;
    u.ships.ship_mut(id).base.last_regen = now;
    if is_ms {
        repair_modules(u, id, combat);
    }
    let s = u.ships.ship_mut(id);
    if s.health < stat.health.max_health {
        let h = st(ld(amount) * ld(s.base.health_mult) + ld(s.health));
        s.health = if stat.health.max_health < h { stat.health.max_health } else { h };
    }
}

/// The module part of the mothership's repair: per priority group (0 first), the damaged
/// modules in it share seven times the module's own repair amount.
fn repair_modules(u: &mut Universe, ms: ShipId, combat: bool) {
    let statics = u.statics.clone();
    let prio = u.ships.ship(ms).base.repair_priority;
    // Slots: docking bay, weapons, armour, micro, special, support (weakest), big gun.
    let mut slots: [Option<ShipId>; 7] = [None; 7];
    let mut count = [0i32; 3];
    let mut support: Option<ShipId> = None;
    for m in crate::clamp::slaves_of(u, ms) {
        let Some(s) = u.ships.get(m) else { continue };
        let Some(stat) = statics.ship(s.ship_type) else { continue };
        let damaged = s.health < stat.health.max_health;
        let slot = match s.ship_type {
            ShipType::sMothershipDockingBay => 0,
            ShipType::sMothershipWeapons => 1,
            ShipType::sMothershipArmour => 2,
            ShipType::sMothershipMicro => 3,
            ShipType::sMothershipSpecial => 4,
            ShipType::sMothershipBigGun => 6,
            ShipType::sMothershipSupport => {
                match support {
                    None => {
                        if damaged {
                            count[prio[5].clamp(0, 2) as usize] += 1;
                            support = Some(m);
                        }
                    }
                    Some(cur) => {
                        if s.health < u.ships.ship(cur).health {
                            support = Some(m);
                        }
                    }
                }
                continue;
            }
            _ => continue,
        };
        if damaged {
            slots[slot] = Some(m);
            count[prio[slot].clamp(0, 2) as usize] += 1;
        } else {
            slots[slot] = None;
        }
    }
    slots[5] = support;
    for (slot, m) in slots.iter().enumerate() {
        let Some(m) = *m else { continue };
        let s = u.ships.ship(m);
        let Some(stat) = statics.ship(s.ship_type) else { continue };
        let amount = if combat { stat.repair_combat_damage } else { stat.repair_damage };
        if !(0.0 < amount) {
            continue;
        }
        let p = prio[slot];
        let n = if count[0] >= 1 {
            (p == 0).then_some(count[0])
        } else if count[1] >= 1 {
            (p == 1).then_some(count[1])
        } else {
            (p == 2).then_some(count[2])
        };
        let s = u.ships.ship_mut(m);
        if let Some(n) = n {
            s.health = st((7.0 / ld(n as f32)) * ld(s.base.health_mult) * ld(amount) + ld(s.health));
        }
        if stat.health.max_health < s.health {
            s.health = stat.health.max_health;
        }
    }
}
