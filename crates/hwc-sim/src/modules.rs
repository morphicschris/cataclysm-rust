//! Mothership modules (`ShipModule.c` and `univCreateMothershipModules`). The Somtaaw and
//! Beast motherships are built from module ships clamped to the hull: engines, command,
//! engineering, research modules, docking bay, support. A new mothership gets the starting
//! modules of the game mode; more are built externally (each clamped where its hull was
//! built). A module whose host is gone drifts off and explodes after a type-specific delay.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_ETG;
use hwc_math::Vector;

use crate::base::{self, game_flags, CreateOptions, SOF_MODULE};
use crate::object::{sof, ShipId};
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// The module types a new Somtaaw mothership gets (`univMissionModuleTypesGet`, 0x53a730).
pub fn mission_module_types(u: &Universe) -> Vec<ShipType> {
    use ShipType::*;
    if u.base.options.tutorial {
        return vec![sMothershipEnginesLower, sMothershipEnginesUpper, sMothershipCommand, sMothershipLowerSpine, sMothershipOreCanArmour, sMothershipEngineering, sMothershipDockingBay];
    }
    if u.single_player {
        match u.base.options.mission {
            1 | 2 => return vec![sMothershipEnginesLower, sMothershipEnginesUpper, sMothershipCommand, sMothershipLowerSpine, sMothershipOreCanArmour, sMothershipEngineering],
            3 => return vec![sMothershipEnginesLower, sMothershipEnginesUpper, sMothershipCommand, sMothershipLowerSpine, sMothershipEngineering],
            _ => {}
        }
    }
    // (The DemoBigGun2 / DemoSect2 demo levels have their own lists.)
    vec![sMothershipEnginesUpper, sMothershipCommand, sMothershipEngineering]
}

/// The Beast mothership's modules (`univMissionBeastModuleTypesGet`, 0x53a8f0).
pub fn beast_module_types(u: &Universe) -> Vec<ShipType> {
    if u.single_player && matches!(u.base.options.mission, 1..=3) {
        return Vec::new();
    }
    vec![ShipType::bMothershipDockingBay, ShipType::bMothershipEnginesLower]
}

/// The extra modules a multiplayer mothership gets when research is off (0x8ab8e8).
const ALL_RESEARCH_MODULES: [ShipType; 7] = [
    ShipType::sMothershipSpecial,
    ShipType::sMothershipEngineering,
    ShipType::sMothershipArmour,
    ShipType::sMothershipMicro,
    ShipType::sMothershipWeapons,
    ShipType::sMothershipDockingBay,
    ShipType::sMothershipBigGun,
];

/// `univCreateMothershipModules` (0x53d150): create the starting modules at the mothership's
/// position and clamp them to it.
pub fn create_mothership_modules(u: &mut Universe, ms: ShipId, position: Vector, player: usize, opts: CreateOptions) {
    let t = u.ships.ship(ms).ship_type;
    let list = match t {
        ShipType::sMothership => mission_module_types(u),
        ShipType::bMothership => beast_module_types(u),
        _ => return,
    };
    let race = u.ships.ship(ms).ship_race;
    for m in list {
        let Ok(id) = base::create_ship(u, m, player, position, CreateOptions { heading: None, no_modules: false, ..opts }) else { continue };
        if race == ShipRace::Somtaaw {
            crate::research::activate_lab(u, player, m, false);
        }
        attach(u, id, ms);
        u.ships.ship_mut(id).flags |= SOF_MODULE;
    }
    if !u.single_player && u.base.options.flags & game_flags::RESEARCH_ENABLED == 0 && t == ShipType::sMothership {
        for m in ALL_RESEARCH_MODULES {
            let Ok(id) = base::create_ship(u, m, player, position, CreateOptions { heading: None, no_modules: false, ..opts }) else { continue };
            crate::construction::unit_cap_create_ship(u, id);
            attach(u, id, ms);
        }
    }
}

/// Position, velocity and orientation from the mothership, then clamped.
fn attach(u: &mut Universe, id: ShipId, ms: ShipId) {
    let m = u.ships.ship(ms);
    let (p, v) = (m.posinfo.position, m.posinfo.velocity);
    let s = u.ships.ship_mut(id);
    s.posinfo.position = p;
    s.posinfo.velocity = v;
    crate::clamp::clamp_obj_to_obj(u, id, ms);
}

/// Whether a type is a mothership module (`ShipModule` custom code).
pub fn is_module(t: ShipType) -> bool {
    module_delay(t).is_some()
}

/// `ShipModuleStaticInit`: the delay (seconds) before a module whose host died explodes.
fn module_delay(t: ShipType) -> Option<f32> {
    use ShipType::*;
    Some(match t {
        sDreadnoughtCannon | sCarrierSupport => 1.0,
        sMothershipSpecial => 4.0,
        sMothershipEngineering => 4.5,
        sMothershipArmour | bMothershipDockingBay | bSectMoship => 2.0,
        sMothershipMicro => 3.5,
        sMothershipWeapons => 2.25,
        sMothershipDockingBay => 1.6,
        sMothershipSupport => 1.25,
        sMothershipCommand => 1.75,
        sMothershipEnginesLower => 5.25,
        sMothershipEnginesUpper => 5.0,
        sMothershipLowerSpine => 3.25,
        sMothershipBigGun | bMothershipSupport => 2.75,
        sMothershipOreCanArmour => 2.5,
        bMothershipEnginesLower => 3.0,
        _ => return None,
    })
}

/// `ShipModuleInit` (0x5aa260): the break-away state starts clear.
pub fn custom_init(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    if is_module(s.ship_type) {
        s.base.module = base::ModuleSpec::default();
    }
}

/// `ShipModuleDied` (0x5aa960): losing the docking bay cancels the mothership's externally
/// built jobs.
pub fn ship_died(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship(id);
    if s.ship_type != ShipType::sMothershipDockingBay {
        return;
    }
    if let Some(host) = crate::clamp::host_of(u, id) {
        if u.ships.get(host).is_some_and(|h| h.ship_type == ShipType::sMothership) {
            crate::construction::cancel_factory_jobs(u, host);
        }
    }
}

/// `ShipModuleHouseKeep` (0x5aa340): a module that lost its host waits (drifting when broken
/// away), then explodes.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    if !is_module(s.ship_type) {
        return;
    }
    if s.base.module.broken_away {
        let m = s.base.module;
        if now <= m.drift_end {
            let s = u.ships.ship_mut(id);
            let f = &mut s.posinfo.force;
            *f = Vector::new(st(ld(m.drift.x) + ld(f.x)), st(ld(m.drift.y) + ld(f.y)), st(ld(m.drift.z) + ld(f.z)));
            let spin = st(ld(0.05f32) + ld(m.spin));
            s.base.module.spin = if ld(spin) >= ld(std::f32::consts::TAU) { 0.0 } else { spin };
        } else if !m.explode {
            u.ships.ship_mut(id).flags |= sof::DONT_APPLY_PHYSICS;
        } else {
            let s = u.ships.ship_mut(id);
            s.how_did_i_die = 0x309;
            s.who_killed_me = 99;
            crate::damage::add_ship_to_delete_list(u, id, 5);
            return;
        }
    }
    // Not while an NIS plays: its module paths (n02's hangar, bSectMoship) fly free of a host.
    let s = u.ships.ship(id);
    if !u.nis_running && s.base.clamp.is_none() && !s.base.module.broken_away {
        let delay = module_delay(s.ship_type).unwrap_or(0.0);
        let r = u.randy.random(RAN_ETG);
        let end = st(ld(r as f32) * 4.656_613e-10 + ld(delay) + ld(now));
        let s = u.ships.ship_mut(id);
        s.base.module.broken_away = true;
        s.base.module.drift_end = end;
        s.base.module.start = now;
        s.base.module.explode = true;
    }
}

/// `ShipModuleBreakAway` (0x5aa470): the module leaves its host with a random drift (by
/// module slot) and, after `delay + time`, explodes (`explode`) or stops.
pub fn break_away(u: &mut Universe, id: ShipId, explode: bool, delay: f32, time: f32) {
    let now = u.total_time_elapsed;
    let t = u.ships.ship(id).ship_type;
    let slot = BREAK_SLOT.iter().find(|(ty, _)| *ty == t).map_or(0, |&(_, s)| s);
    let host = crate::clamp::host_of(u, id);
    let hsys = host.and_then(|h| u.ships.get(h)).map(|h| h.rotinfo.coordsys).unwrap_or_default();
    let ranges = BREAK_RANGES[slot as usize];
    let mut comp = |lo: f32, hi: f32| -> f64 {
        let r = u.randy.random(hwc_math::random::RAN_GAME);
        (ld(hi) - ld(lo)) * 2.328_306_4e-10 * f64::from(r as f32) + ld(lo)
    };
    let col = |c: usize| hsys.col(c);
    let mut drift = Vector::ZERO;
    for (axis, c) in [(0usize, 0usize), (1, 1), (2, 1)] {
        let v = col(c);
        let _ = axis;
        let (lo, hi) = ranges[axis];
        let kx = comp(lo, hi);
        let ky = comp(lo, hi);
        let kz = comp(lo, hi);
        drift = Vector::new(st(kx * ld(v.x) + ld(drift.x)), st(ky * ld(v.y) + ld(drift.y)), st(kz * ld(v.z) + ld(drift.z)));
    }
    drift.normalize();
    let (lo, hi) = (50_000.0f32, 70_000.0f32);
    drift.x = st(comp(lo, hi) * ld(drift.x));
    drift.y = st(comp(lo, hi) * ld(drift.y));
    drift.z = st(comp(lo, hi) * ld(drift.z));
    let start = st(ld(now) + ld(delay));
    let up = u.ships.ship(id).up().normalized();
    let s = u.ships.ship_mut(id);
    s.base.module = base::ModuleSpec {
        broken_away: true,
        drift_end: st(ld(start) + ld(time)),
        drift,
        drifting: true,
        spin: 0.0,
        spin_axis: up,
        explode,
        start,
    };
    crate::clamp::unclamp(u, id);
    base::set_disable_reason(u.ships.ship_mut(id), true, 8);
    u.ships.ship_mut(id).flags |= sof::NO_BUMPS;
}

/// 0x8b9468: break-away slot per module type.
const BREAK_SLOT: [(ShipType, u8); 13] = [
    (ShipType::sMothershipSpecial, 0),
    (ShipType::sMothershipEngineering, 4),
    (ShipType::sMothershipArmour, 0),
    (ShipType::sMothershipMicro, 0),
    (ShipType::sMothershipWeapons, 0),
    (ShipType::sMothershipDockingBay, 5),
    (ShipType::sMothershipSupport, 0),
    (ShipType::sMothershipEnginesLower, 3),
    (ShipType::sMothershipEnginesUpper, 2),
    (ShipType::sMothershipLowerSpine, 1),
    (ShipType::sMothershipBigGun, 6),
    (ShipType::sMothershipOreCanArmour, 0),
    (ShipType::sCarrierSupport, 1),
];

/// 0x8b93c0: per slot, the random ranges along the host's up, right and heading.
const BREAK_RANGES: [[(f32, f32); 3]; 7] = [
    [(0.4, 0.5), (-0.3, 0.3), (-0.02, 0.02)],
    [(-0.4, -0.5), (-0.3, 0.3), (-0.02, 0.02)],
    [(0.15, 0.2), (-0.3, 0.3), (-0.2, -0.4)],
    [(-0.15, -0.2), (-0.3, 0.3), (-0.2, -0.4)],
    [(0.1, 0.2), (-0.3, 0.3), (0.1, 0.3)],
    [(-0.1, -0.2), (-0.3, 0.3), (0.1, 0.3)],
    [(-0.4, -0.5), (-0.3, 0.3), (-0.02, 0.02)],
];
