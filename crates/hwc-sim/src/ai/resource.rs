//! The computer player's resource manager (`AIResourceMan.c`, Cataclysm's `air*` 0x4124c0..):
//! resourcers, processors, carriers and support modules, the Somtaaw mothership's modules, the
//! RUs left in the world, and what to do with the ships it is given.

use super::{ai, ai_mut, resource_requests_ships, AiPlayer, Manager};
use crate::object::{sof, ShipId};
use crate::statics::{DataSource, ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// `ResourceFeatures` bits.
pub mod feature {
    /// Carriers and motherships get an active-support team.
    pub const SUPPORT_CARRIERS: u32 = 0x1;
    /// Resourcers work in teams.
    pub const RESOURCER_TEAMS: u32 = 0x2;
    /// Processors (resource controllers) get an active-support team.
    pub const CONTROLLER_TEAMS: u32 = 0x4;
    /// Advance support frigates get an active-support team.
    pub const ASF_TEAMS: u32 = 0x8;
    /// Resourcers under attack call for help.
    pub const DISTRESS: u32 = 0x10;
    /// The number of resourcers follows the RUs left and the drop-off points.
    pub const SMART_COLLECTOR_COUNT: u32 = 0x200;
    /// Processors are built.
    pub const CONTROLLERS: u32 = 0x400;
    /// Resourcers fly aggressive (evasive without).
    pub const AGGRESSIVE: u32 = 0x1000;
}

/// `AIResourceMan.script` (table 0x743310) with the compiled defaults.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tweaks {
    pub update_ru_count_rate: u32,
    pub build_rc_cash_table_base: i32,
    pub build_rc_cash_table_inc: i32,
    pub max_rcollectors_per_dock_point: i32,
    pub max_rcollectors_to_build: i32,
    pub min_worthwhile_rus: i32,
    pub rus_per_rcollector: i32,
    pub min_worthwhile_rus_for_controller: i32,
    pub min_rcollectors_for_controller: i32,
    pub build_rcontroller_cash_base: i32,
    pub build_asf_cash_base: i32,
    pub build_carrier_cash_base: i32,
    pub build_carrier_su_point: i32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Self {
            update_ru_count_rate: 3,
            build_rc_cash_table_base: 400,
            build_rc_cash_table_inc: 200,
            max_rcollectors_per_dock_point: 3,
            max_rcollectors_to_build: 5,
            min_worthwhile_rus: 400,
            rus_per_rcollector: 1000,
            min_worthwhile_rus_for_controller: 3000,
            min_rcollectors_for_controller: 4,
            build_rcontroller_cash_base: 1500,
            build_asf_cash_base: 2000,
            build_carrier_cash_base: 6000,
            build_carrier_su_point: 30,
        }
    }
}

impl Tweaks {
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = Self::default();
        let Some(lines) = crate::level::read_lines(src, "AIResourceMan.script") else { return t };
        for (k, v) in lines {
            let Some(n) = crate::statics::cscan::scan_i32(&v) else { continue };
            match k.as_str() {
                "UPDATE_RU_COUNT_RATE" => t.update_ru_count_rate = n as u32,
                "BUILD_RC_CASH_TABLE_BASE" => t.build_rc_cash_table_base = n,
                "BUILD_RC_CASH_TABLE_INC" => t.build_rc_cash_table_inc = n,
                "MAX_RCOLLECTORS_PER_DOCK_POINT" => t.max_rcollectors_per_dock_point = n,
                "MAX_RCOLLECTORS_TO_BUILD" => t.max_rcollectors_to_build = n,
                "MIN_WORTHWHILE_RUs" => t.min_worthwhile_rus = n,
                "RUs_PER_RCOLLECTOR" => t.rus_per_rcollector = n,
                "MIN_WORTHWHILE_RUs_FOR_CONTROLLER" => t.min_worthwhile_rus_for_controller = n,
                "MIN_RCOLLECTORS_FOR_CONTROLLER" => t.min_rcollectors_for_controller = n,
                "BUILD_RCONTROLLER_CASH_BASE" => t.build_rcontroller_cash_base = n,
                "BUILD_ASF_CASH_BASE" => t.build_asf_cash_base = n,
                "BUILD_CARRIER_CASH_BASE" => t.build_carrier_cash_base = n,
                "BUILD_CARRIER_SU_POINT" => t.build_carrier_su_point = n,
                _ => {}
            }
        }
        t
    }
}

/// `airInit`: the feature bits by level.
pub fn init(a: &mut AiPlayer) {
    a.rus_in_world = -1;
    match a.level {
        0 => a.resource_features |= feature::CONTROLLER_TEAMS,
        1 => a.resource_features |= 0x1c00 | feature::CONTROLLER_TEAMS,
        _ => a.resource_features |= 0x31b | 0x1c00 | feature::CONTROLLER_TEAMS,
    }
}

/// `airShipDied`.
pub fn ship_died(a: &mut AiPlayer, id: ShipId, t: ShipType) {
    let Some(i) = a.resource_reserves.iter().position(|&s| s == id) else { return };
    a.resource_reserves.remove(i);
    match t {
        ShipType::sCarrierSupport | ShipType::sMothershipSupport | ShipType::bMothershipSupport => a.num_support -= 1,
        ShipType::sWorker | ShipType::bWorker | ShipType::tResourceCollector => {
            a.resource_collectors.retain(|&s| s != id);
            a.num_collectors -= 1;
        }
        ShipType::sProcessor | ShipType::bProcessor => a.num_controllers -= 1,
        ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate => a.num_asf -= 1,
        _ => {}
    }
}

/// A team of the resource manager doing `order` (`aitCreate(RESOURCE_TEAM)` + an `aioCreate*`
/// order), with the ship.
fn support_team(u: &mut Universe, p: usize, id: ShipId, order: &str, flag: u32) -> usize {
    let team = super::team_create(u, p, Manager::Resource, order);
    if let Some(Some((_, t))) = u.ai.teams.get_mut(team) {
        t.flags |= flag;
    }
    super::team_add_ship(u, team, id);
    ai_mut(u, p).support_teams.push(team);
    team
}

/// `airAddNewShip` (0x412960): a ship given to the resource manager.
pub fn add_new_ship(u: &mut Universe, p: usize, id: ShipId) {
    ai_mut(u, p).resource_reserves.push(id);
    let t = u.ships.ship(id).ship_type;
    let features = ai(u, p).resource_features;
    match t {
        ShipType::sCarrier | ShipType::sMothership | ShipType::bCarrier | ShipType::bMothership | ShipType::kCarrier | ShipType::tCarrier => {
            if features & feature::SUPPORT_CARRIERS != 0 {
                support_team(u, p, id, "ActiveSupport(mothership)", 0);
            }
        }
        ShipType::sCarrierSupport | ShipType::sMothershipSupport | ShipType::bMothershipSupport => {
            let a = ai_mut(u, p);
            a.num_support += 1;
            a.support_being_built -= 1;
            a.support_module_pending = false;
        }
        ShipType::sWorker | ShipType::bWorker | ShipType::tResourceCollector => added_resource_collector(u, p, id),
        ShipType::sProcessor | ShipType::bProcessor => {
            if features & feature::CONTROLLER_TEAMS != 0 {
                support_team(u, p, id, "ActiveSupport(resource)", 0);
            }
            // (Without the feature: aiuFindBestResourceBlob and a move there, with the moves.)
            let a = ai_mut(u, p);
            a.num_controllers += 1;
            if a.controllers_being_built > 0 {
                a.controllers_being_built -= 1;
            }
        }
        ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate => {
            if features & feature::ASF_TEAMS != 0 {
                support_team(u, p, id, "ActiveSupport(strike craft)", 0);
            }
            let a = ai_mut(u, p);
            a.num_asf += 1;
            if a.asf_being_built > 0 {
                a.asf_being_built -= 1;
            }
        }
        ShipType::kProximitySensor | ShipType::tProximitySensor => ai_mut(u, p).proximity_sensors.push(id),
        _ => {}
    }
}

/// `airAddedResourceCollector` (0x412700): a new resourcer goes harvesting (with harvesting
/// off it docks to be retired). With resourcer teams it joins a team of fewer than three.
fn added_resource_collector(u: &mut Universe, p: usize, id: ShipId) {
    {
        let a = ai_mut(u, p);
        a.num_collectors += 1;
        if a.collectors_being_built > 0 {
            a.collectors_being_built -= 1;
        }
    }
    if !crate::multiplayer::harvesting_enabled(u) {
        crate::dock::cl_dock(u, &[id], 7, None);
        return;
    }
    ai_mut(u, p).resource_collectors.push(id);
    let features = ai(u, p).resource_features;
    let tactics = if features & feature::AGGRESSIVE != 0 { crate::object::tactics::AGGRESSIVE } else { crate::object::tactics::EVASIVE };
    if features & feature::RESOURCER_TEAMS != 0 {
        let team = super::teams_with_flag(u, p, 8).into_iter().find(|&t| u.ai.teams[t].as_ref().is_some_and(|(_, t)| t.ships.len() < 3));
        match team {
            Some(t) => super::team_add_ship(u, t, id),
            None => {
                support_team(u, p, id, "Resourcer", 8);
            }
        }
        // (The team's ActiveResource move gives the collect order; until the moves are
        // ported the order is given here.)
    }
    crate::command::cl_set_tactics(u, &[id], tactics);
    let r = crate::resource::find_nearest_resource(u, id, 0.0, hwc_math::Vector::ZERO);
    crate::resource::cl_collect_resource(u, &[id], r);
}

/// A type can be asked for: the mothership lives, the type is the player's race's (`0x8a74xx`
/// race masks), its statics are loaded, it is not a resourcer with harvesting off, and the
/// mothership can build it.
fn can_request(u: &Universe, p: usize, t: ShipType) -> bool {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)) else { return false };
    if ms.flags & sof::DEAD != 0 {
        return false;
    }
    let race = u.players[p].race;
    if !crate::research::race_types(race).contains(&t.index()) {
        return false;
    }
    let Some(st) = u.statics.ship(t) else { return false };
    if !st.info_needed {
        return false;
    }
    if st.ship_class == Some(ShipClass::Resource) && !crate::multiplayer::harvesting_enabled(u) {
        return false;
    }
    crate::construction::can_build_ship_type(u, ms.id, t, false)
}

/// The race's resourcer (sWorker, bWorker, or the Turanic one for race 3).
fn collector_type(race: ShipRace) -> Option<ShipType> {
    match race {
        ShipRace::Somtaaw => Some(ShipType::sWorker),
        ShipRace::Beast => Some(ShipType::bWorker),
        _ if race.index() == 3 => Some(ShipType::tResourceCollector),
        _ => None,
    }
}

/// `airDumbResourcerRequests`: a random number of resourcers by level (2..4 more), built one
/// request at a time.
fn dumb_resourcer_requests(u: &mut Universe, p: usize) -> bool {
    if ai(u, p).resourcers_to_build == 0 {
        let r = u.randy.random(hwc_math::random::RAN_AI_PLAYER) % 3;
        let a = ai_mut(u, p);
        a.resourcers_to_build = (r + a.level + 2) as u8;
        return false;
    }
    let have = ai(u, p).num_collectors + ai(u, p).collectors_being_built;
    let want = i32::from(ai(u, p).resourcers_to_build);
    if have >= want {
        return false;
    }
    let Some(t) = collector_type(u.players[p].race) else { return false };
    if can_request(u, p, t) {
        resource_requests_ships(u, p, t, want - have, 1);
        return true;
    }
    false
}

/// `airRequestMothershipModules` (Cataclysm): a Somtaaw mothership asks for its Docking Bay,
/// then its Micro module, then the Special, Weapons and Armour modules at random, one every
/// 100 turns, and now and then researches technologies 0x1c and 6. Returns whether it asked.
fn request_mothership_modules(u: &mut Universe, p: usize) -> bool {
    let rnd = |u: &mut Universe| u.randy.random(0);
    let somtaaw_ms = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).is_some_and(|s| s.ship_type == ShipType::sMothership);
    if !somtaaw_ms {
        if rnd(u) % 101 < 5 {
            super::research::research_tech_if_lab_free(u, p, 0x1c);
        }
        return false;
    }
    if ai(u, p).module_cooldown != 0 {
        ai_mut(u, p).module_cooldown -= 1;
        return false;
    }
    let has = |u: &Universe, t: ShipType| crate::base::player_has_ship_of_type(u, t, p);
    let mut asked = false;
    let want = if !has(u, ShipType::sMothershipDockingBay) {
        Some(ShipType::sMothershipDockingBay)
    } else if !has(u, ShipType::sMothershipMicro) {
        Some(ShipType::sMothershipMicro)
    } else {
        match rnd(u) % 3 {
            0 => Some(ShipType::sMothershipSpecial).filter(|&t| !has(u, t)),
            1 => Some(ShipType::sMothershipWeapons).filter(|&t| !has(u, t)),
            _ => Some(ShipType::sMothershipArmour).filter(|&t| !has(u, t)),
        }
    };
    if let Some(t) = want {
        resource_requests_ships(u, p, t, 1, 1);
        ai_mut(u, p).module_cooldown = 100;
        asked = true;
    }
    if ai(u, p).module_cooldown != 100 && rnd(u) % 101 < 5 && !super::research::research_tech_if_lab_free(u, p, 0x1c) {
        super::research::research_tech_if_lab_free(u, p, 6);
    }
    asked
}

/// `airResourceManager` (0x413250).
pub fn manager(u: &mut Universe, p: usize) {
    let tw = u.ai.resource_tweaks;
    // The RUs left in the world, every few turns.
    if ai(u, p).frame_count & tw.update_ru_count_rate == 0 || ai(u, p).rus_in_world == -1 {
        let total: i32 = u.base.resources.list.iter().map(|r| r.resource_value).sum();
        ai_mut(u, p).rus_in_world = total;
    }
    let a = ai(u, p);
    let worth = a.spent[3] + a.spent[2] + a.spent[1] + u.base.players[p].resource_units;
    let flags = u.base.options.flags;
    let research_on = flags & crate::multiplayer::mg::RESEARCH_ENABLED != 0;
    let harvesting = crate::multiplayer::harvesting_enabled(u);
    let mut asked_module = false;
    if !u.single_player && research_on {
        asked_module = request_mothership_modules(u, p);
        if asked_module {
            return;
        }
    }
    let mut asked_collectors = false;
    if harvesting && ai(u, p).rus_in_world > tw.min_worthwhile_rus {
        let collectors = ai(u, p).num_collectors + ai(u, p).collectors_being_built;
        let race = u.players[p].race;
        if ai(u, p).resource_features & feature::SMART_COLLECTOR_COUNT == 0 {
            if dumb_resourcer_requests(u, p) {
                return;
            }
        } else if collectors < tw.max_rcollectors_to_build && collectors < ai(u, p).ru_dock_points * tw.max_rcollectors_per_dock_point {
            if let Some(t) = collector_type(race) {
                if collectors == 0 {
                    if can_request(u, p, t) {
                        resource_requests_ships(u, p, t, 1, 1);
                        return;
                    }
                } else if (collectors + 1) * tw.rus_per_rcollector < ai(u, p).rus_in_world
                    && (collectors - 1) * tw.build_rc_cash_table_inc + tw.build_rc_cash_table_base < worth
                    && can_request(u, p, t)
                {
                    resource_requests_ships(u, p, t, 1, 0);
                    asked_collectors = true;
                }
            }
        }
        // A processor when there are enough resourcers and RUs.
        let a = ai(u, p);
        if a.resource_features & feature::CONTROLLERS != 0
            && collectors >= tw.min_rcollectors_for_controller
            && a.num_controllers + a.controllers_being_built == 0
            && !asked_module
            && (u.mp.num_players as i32) * tw.min_worthwhile_rus_for_controller < a.rus_in_world
            && tw.build_rcontroller_cash_base < worth
        {
            let ms = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.ship_type);
            match (race, ms) {
                (ShipRace::Somtaaw, Some(ShipType::sMothership)) => {
                    if can_request(u, p, ShipType::sProcessor) && crate::base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, p) {
                        resource_requests_ships(u, p, ShipType::sProcessor, 1, 0);
                        return;
                    }
                }
                (ShipRace::Beast, Some(ShipType::bMothership)) => {
                    if can_request(u, p, ShipType::bProcessor) {
                        resource_requests_ships(u, p, ShipType::bProcessor, 1, 0);
                        return;
                    }
                }
                _ => {}
            }
        }
    }
    let _ = asked_collectors;
    // A carrier when support units run short.
    let pl = &u.base.players[p];
    if !u.single_player && tw.build_carrier_cash_base < worth && (pl.su_capacity - tw.build_carrier_su_point) as u32 <= pl.su_used as u32 {
        let ms = pl.mothership.and_then(|m| u.ships.get(m)).map(|s| s.ship_type);
        let carrier = match (u.players[p].race, ms) {
            (ShipRace::Somtaaw, Some(ShipType::sMothership)) => Some(ShipType::sCarrier).filter(|_| crate::base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, p)),
            (ShipRace::Beast, Some(ShipType::bMothership)) => Some(ShipType::bCarrier),
            _ => None,
        };
        if let Some(t) = carrier {
            if can_request(u, p, t) && super::unit_cap_can_build(u, p, t, 1) > 0 {
                resource_requests_ships(u, p, t, 1, 0);
            }
        }
    }
    // (airDivideSupportTasks: with the support teams.)
    // `aidSendDistressSignal`: the resourcers being hit call for help (the distress calls and the
    // ships to rescue the roving defence answers).
    if ai(u, p).resource_features & feature::DISTRESS != 0 {
        let hit: Vec<ShipId> = ai(u, p).resource_collectors.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.getting_rocked.is_some())).collect();
        let d = &mut ai_mut(u, p).defense;
        for id in hit {
            if !d.distress.contains(&id) {
                d.distress.push(id);
            }
            if !d.rescue.contains(&id) {
                d.rescue.push(id);
            }
        }
    }
}
