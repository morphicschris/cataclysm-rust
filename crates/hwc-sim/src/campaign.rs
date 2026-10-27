//! The single-player campaign between missions: the player's fleet hyperspacing out at the end
//! of a mission, what of it (and of the player) reaches the next mission, and the next
//! mission's start from that.
//!
//! The original never takes a snapshot: the ships that jump stay ship objects in the
//! `ShipsInHyperspace` list while `singlePlayerLoadNewLevel` deletes everything else and resets
//! the universe, and the player structure (RUs, technology, research) is kept through the
//! reset. Here the end of the jump produces a [`Carry`] (the list, the player and the random
//! streams) and [`crate::mission::MissionRun::start_campaign`] starts the next mission from it.
//!
//! The hyperspace-out half of `singlePlayerGameUpdate` (0x5148f0) is [`update`]:
//!
//! * `HyperspaceRollCallBegin` (0x514070, [`hyperspace_button`]): allowed once the mission
//!   created its "Hyperspace" objective (or with the debug override), not while a ship is
//!   being built. Missions whose entry in 0x88a35c is set (1, 2, 5, 7, 15, 17) skip the roll
//!   call: after five seconds every ship that can dock docks instantly and the hyperspace-
//!   capable ships (capital ships and modules) go straight into hyperspace. The other missions
//!   order every small ship to dock and the capital ships to the mothership, and wait for the
//!   roll call ([`go_now`], [`emergency_hyperspace`] are its two buttons).
//! * State 3 waits 2.5 s, state 4 turns the fleet to the mothership's heading and runs each
//!   ship through `UpdateHyperspacingShip` (the window effect into hyperspace); state 5 is the
//!   whiteout and the between-missions movie, after which the ships the mission marked with
//!   `DeleteShipsWhenMissionEnds` are wiped and the next level is due.
//!
//! See `docs/sim/missions.md` (Campaign).

use std::collections::BTreeMap;

use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::{Randy, RAN_GAME};
use hwc_math::Vector;

use crate::base::PlayerBase;
use crate::mission::{hs_effect, sp_state, FleetShip, Game, LogKind, HUMAN_PLAYER};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipType};
use crate::universe::Universe;

/// 0x88a35c: missions that end without a roll call (the fleet docks and jumps at once).
const NO_ROLL_CALL: [bool; 19] = table([1, 2, 5, 7, 15, 17]);
/// 0x88a3a8: missions whose fleet fades in at the start point instead of coming through
/// hyperspace windows one ship at a time.
const FADE_IN_ARRIVAL: [bool; 19] = table([4, 5, 6, 7, 8, 10, 12, 14, 15, 17]);

const fn table<const N: usize>(missions: [usize; N]) -> [bool; 19] {
    let mut t = [false; 19];
    let mut i = 0;
    while i < N {
        t[missions[i]] = true;
        i += 1;
    }
    t
}

/// Whether mission `m` ends without a roll call.
pub fn skips_roll_call(m: u32) -> bool {
    NO_ROLL_CALL.get(m as usize).copied().unwrap_or(false)
}

/// Whether mission `m`'s fleet fades in at the start point.
pub fn fades_in(m: u32) -> bool {
    FADE_IN_ARRIVAL.get(m as usize).copied().unwrap_or(false)
}

/// `shipHyperspaceState` values of a ship leaving (ship info +0x20).
pub mod leave_state {
    pub const NONE: u32 = 0;
    pub const WAITING: u32 = 1;
    pub const ACCELERATING: u32 = 2;
    pub const GONE: u32 = 4;
}

/// A ship's single-player hyperspace record (`ShipSinglePlayerGameInfo`, ship +0x4c4) and the
/// roll-call bits of its special flags.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ShipHs {
    /// `shipHyperspaceState` (+0x20).
    pub state: u32,
    /// `timeToHyperspace` (+0x24) and the delay it was set from (+0x28).
    pub time: f32,
    pub delay: f32,
    /// (+0x2c..+0x34) the offset from the mothership along its heading, right and up axes.
    pub offset: Vector,
    pub clipt: f32,
    pub rate: f32,
    /// `hsState` (+0x40), `hsFlags` (+0x44).
    pub hs_state: u32,
    pub hs_flags: u32,
    /// Special flag 0x80: within hyperspace range of the mothership.
    pub in_range: bool,
    /// Special flag 0x8000: in the military parade around the mothership.
    pub in_parade: bool,
    /// Flags2 0x40: left behind by the emergency jump.
    pub abandoned: bool,
    /// (+0x0) coming in mid-level to `dest` (+0xc..+0x18); (+0x4) went out mid-level.
    pub midlevel_in: bool,
    pub midlevel_out: bool,
    pub dest: [f32; 4],
}

/// The single-player state of the jump between missions (`singlePlayerGameInfo` and the few
/// globals around it).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct HyperState {
    /// 0xaaf94c `playerCanHyperspace`: the mission created its "Hyperspace" objective.
    pub player_can_hyperspace: bool,
    /// 0x9101d8: the debug override (the original's C key) that allows the jump anyway.
    pub hyperspace_override: bool,
    /// 0xaaf914 `oldAutoLaunch`, restored when the fleet arrives.
    pub old_auto_launch: u32,
    /// 0xaaf91c / 0xaaf920.
    pub rollcall_docked: i32,
    pub rollcall_remaining: i32,
    /// 0x9101f8: the roll-call screen is up.
    pub rollcall_screen: bool,
    /// 0xaaf924 (state 4: 0 turning to the mothership's heading, 1 jumping).
    pub substate: u32,
    /// 0xaaf928.
    pub timestamp: f32,
    /// 0x90c8e0: when state 3 ends.
    pub fleet_command_time: f32,
    /// 0x91020c: the end of the skipped roll call's five seconds.
    pub no_roll_call_until: f32,
    /// 0xaaf950 `hyperspaceFails`.
    pub hyperspace_fails: bool,
    /// 0xaaf958: the between-missions movie has yet to play.
    pub bink_pending: bool,
    /// 0x91a61c / 0x91a620 `mrWhiteOut` / `mrWhiteOutT`.
    pub whiteout: bool,
    pub whiteout_t: f32,
    /// 0x9100c8 / 0x9100c4 / 0x910218: the screen fade (1 black .. 0 clear), the flag that
    /// restarts its clock, and the time of the last decrement.
    pub fade: f32,
    pub fade_reset: bool,
    pub fade_last: f32,
    /// 0x910214: the fade-in arrival places the fleet on the next update.
    pub fade_arrival_pending: bool,
    /// `ShipsInHyperspace` (0xaaf92c), in list order: ships out of the universe lists.
    pub in_hyperspace: Vec<ShipId>,
    /// Per-ship hyperspace records.
    pub ships: BTreeMap<ShipId, ShipHs>,
    /// 0xaaf8ec: the mission is over and the next level is due ([`Carry::take`]).
    pub level_complete: bool,
    /// The carried fleet's ships, created in hyperspace when the level loads, by fleet entry.
    pub fleet_ids: Vec<Option<ShipId>>,
    /// `ShipsToHyperspace` (0xaaf93c): ships KAS sent out of the level that are on their way.
    pub to_hyperspace: Vec<ShipId>,
    /// 0x9101f0 `kasfHyperspaceDelay`: seconds added to the next mid-level jump.
    pub kas_delay: f32,
    /// 0x91a624: the white flash over the view when mission 1's NISlet ends (1 when set,
    /// held for three seconds of the arrival, then falling by a half per second; drawn
    /// additively while above 0), and 0x91021c / 0x910220, the time of its last step and the
    /// end of the hold.
    pub arrival_flash: f32,
    pub arrival_flash_last: f32,
    pub arrival_flash_hold: f32,
}

impl Default for HyperState {
    fn default() -> Self {
        HyperState {
            player_can_hyperspace: false,
            hyperspace_override: false,
            // singlePlayerInit.
            old_auto_launch: 0xf,
            arrival_flash: 0.0,
            arrival_flash_last: 0.0,
            arrival_flash_hold: 0.0,
            rollcall_docked: 0,
            rollcall_remaining: 0,
            rollcall_screen: false,
            substate: 0,
            timestamp: 0.0,
            fleet_command_time: 0.0,
            no_roll_call_until: 0.0,
            hyperspace_fails: false,
            bink_pending: false,
            whiteout: false,
            whiteout_t: 0.0,
            fade: 0.0,
            fade_reset: false,
            fade_last: 0.0,
            fade_arrival_pending: false,
            in_hyperspace: Vec::new(),
            ships: BTreeMap::new(),
            level_complete: false,
            fleet_ids: Vec::new(),
            to_hyperspace: Vec::new(),
            kas_delay: 0.0,
        }
    }
}

impl HyperState {
    fn rec(&mut self, id: ShipId) -> &mut ShipHs {
        self.ships.entry(id).or_default()
    }

    fn get(&self, id: ShipId) -> ShipHs {
        self.ships.get(&id).copied().unwrap_or_default()
    }
}

/// What reaches the next mission (`singlePlayerLoadNewLevel`'s view of the world): the ships in
/// hyperspace with what is docked inside them, the player (RUs, technology, research, labs),
/// the game clock and the random streams.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Carry {
    /// The mission the fleet left.
    pub mission: u32,
    pub fleet: Vec<FleetShip>,
    pub resource_units: i32,
    pub player: PlayerBase,
    pub total_time: f32,
    pub randy: Randy,
    pub old_auto_launch: u32,
}

impl Carry {
    /// The carry-over of a mission whose fleet has jumped ([`HyperState::level_complete`]).
    pub fn take(u: &Universe, game: &Game) -> Option<Carry> {
        if !game.hyper.level_complete {
            return None;
        }
        let ms = u.base.players[HUMAN_PLAYER].mothership;
        // singlePlayerLoadNewLevel wipes the ships that went out mid-level (the computer
        // player's, sent away by KAS).
        let fleet = game
            .hyper
            .in_hyperspace
            .iter()
            .filter(|&&id| !game.hyper.get(id).midlevel_out && u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER))
            .filter_map(|&id| fleet_entry(u, game, id, ms))
            .collect();
        Some(Carry {
            mission: game.config.mission,
            fleet,
            resource_units: u.base.players[HUMAN_PLAYER].resource_units,
            player: u.base.players[HUMAN_PLAYER].clone(),
            total_time: u.total_time_elapsed,
            randy: u.randy.clone(),
            old_auto_launch: game.hyper.old_auto_launch,
        })
    }
}

fn fleet_entry(u: &Universe, game: &Game, id: ShipId, ms: Option<ShipId>) -> Option<FleetShip> {
    let s = u.ships.get(id)?;
    let module = crate::base::class_of(u, s.ship_type) == Some(ShipClass::Component) && ms.is_some_and(|m| m != id);
    let docked = s
        .base
        .inside
        .as_ref()
        .map(|i| i.ships.iter().filter_map(|&d| fleet_entry(u, game, d, None)).collect())
        .unwrap_or_default();
    Some(FleetShip {
        ship_type: s.ship_type,
        offset: game.hyper.get(id).offset,
        docked,
        module,
        carried: true,
        color_scheme: Some(s.color_scheme),
        tactics: Some(s.tactics),
        attributes: s.attributes,
    })
}

/// `spWipeUnavailableModulesInHyperspace` (0x515cd0) for mission `mission`: the mission
/// modules the new mission's mothership does not have are wiped (the research and support
/// modules the player built are kept), and mission 4 loses the docking bay.
pub fn wipe_unavailable_modules(fleet: &[FleetShip], mission: u32) -> Vec<FleetShip> {
    use ShipType::*;
    let list = crate::level::mission_module_types(mission);
    fleet
        .iter()
        .filter(|f| {
            if !f.module {
                return true;
            }
            let built = matches!(f.ship_type, sMothershipSpecial | sMothershipEngineering | sMothershipArmour | sMothershipMicro | sMothershipWeapons | sMothershipSupport | sMothershipBigGun | sMothershipDockingBay | sCarrierSupport);
            if built {
                return !(mission == 4 && f.ship_type == sMothershipDockingBay);
            }
            list.contains(&f.ship_type)
        })
        .cloned()
        .collect()
}

/// The player structure across `universeReset`: everything is kept but the pointers to ships
/// and the unit-cap counts (the carried ships are counted again as they are created), and
/// `suPlayerInit` clears the upgrade state that `suPlayerUpdateFromResearch` rebuilds.
pub(crate) fn restore_player(u: &mut Universe, c: &Carry) {
    let mut p = c.player.clone();
    p.mothership = None;
    p.su_used = 0;
    p.total_ships = 0;
    p.ship_count.iter_mut().for_each(|n| *n = 0);
    p.class_count = Default::default();
    p.module_count = 0;
    p.resource_units = c.resource_units;
    // suPlayerInit.
    p.gun_types = [0; 9];
    p.gun_types[0] = 1;
    p.max_tech_level = 0;
    p.upgrade_bits = 0;
    u.base.players[HUMAN_PLAYER] = p;
    u.total_time_elapsed = c.total_time;
    u.randy = c.randy.clone();
}

// ---------------------------------------------------------------------------------------------
// The player's side: the hyperspace button and the roll call's buttons.
// ---------------------------------------------------------------------------------------------

/// Why the hyperspace button did nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HyperspaceRefused {
    /// The mission has not offered the jump yet (no "Hyperspace" objective).
    NotAvailable,
    /// A ship is being built (the original shows a message).
    ShipUnderConstruction,
    /// A jump is already under way, or the player has no mothership.
    Busy,
}

/// `HyperspaceRollCallBegin` (0x514070): the hyperspace button.
pub fn hyperspace_button(u: &mut Universe, game: &mut Game) -> Result<(), HyperspaceRefused> {
    let h = &mut game.hyper;
    if !(h.player_can_hyperspace || h.hyperspace_override) {
        return Err(HyperspaceRefused::NotAvailable);
    }
    if ship_under_construction(u) {
        return Err(HyperspaceRefused::ShipUnderConstruction);
    }
    h.hyperspace_override = false;
    h.player_can_hyperspace = false;
    let Some(ms) = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| u.ships.get(m).is_some()) else {
        return Err(HyperspaceRefused::Busy);
    };
    if game.sp_state != sp_state::NONE {
        return Err(HyperspaceRefused::Busy);
    }
    let _ = ms;
    game.sp_state = sp_state::WAITING_ROLLCALL;
    game.hyper.old_auto_launch = u.base.players[HUMAN_PLAYER].auto_launch;
    crate::dock::cl_auto_launch(u, 0, HUMAN_PLAYER);
    let now = u.total_time_elapsed;
    if skips_roll_call(game.config.mission) {
        // kasWideScreenInHyperspace(90): five seconds of wide screen first.
        game.wide_screen_call(crate::mission::WideScreenKind::InHyperspace, 90, u.total_time_elapsed);
        game.hyper.no_roll_call_until = st(ld(now) + 5.0);
        game.log(LogKind::Flow, "hyperspace: no roll call in this mission (the fleet jumps in 5 s)");
        return Ok(());
    }
    let small = non_hyperspacing_ships(u);
    cancel_launch_orders(u);
    if crate::dock::cl_dock(u, &small, crate::dock::purpose::DOCK_STAY, None) > 0 {
        game.log(LogKind::Flow, "hyperspace: fleet ordered to dock");
        speech(u, game, SPEECH_ROLLCALL_DOCK);
    }
    tell_hyperspacing_ships_to_ready(u, game);
    game.hyper.rollcall_screen = true;
    calculate_roll_call(u, game);
    game.log(LogKind::Flow, format!("hyperspace roll call: {} docked, {} remaining", game.hyper.rollcall_docked, game.hyper.rollcall_remaining));
    Ok(())
}

/// Roll-call speech events (`speechEventQueue`): the small ships are told to dock
/// (`HyperspaceRollCallBegin`), the roll call is complete (state 2 to 3), the jump is called
/// off (`spAbortHyperspaceCB`).
pub const SPEECH_ROLLCALL_DOCK: i32 = 0x0200_101f;
pub const SPEECH_ROLLCALL_COMPLETE: i32 = 0x0200_1020;
pub const SPEECH_HYPERSPACE_ABORTED: i32 = 0x0400_1021;

/// Queues a speech event for the front end (as KAS `SpeechEvent` does).
fn speech(u: &Universe, game: &mut Game, event: i32) {
    let time = u.total_time_elapsed;
    game.presentation.push(crate::mission::Presentation::Speech { event, variable: 0, time });
}

/// `spAbortHyperspaceCB` (0x5124f0): the roll call's cancel button: the jump is off, the small
/// ships stop docking, and the hyperspace button is offered again.
pub fn abort_hyperspace(u: &mut Universe, game: &mut Game) -> bool {
    if game.sp_state != sp_state::WAITING_ROLLCALL {
        return false;
    }
    speech(u, game, SPEECH_HYPERSPACE_ABORTED);
    game.sp_state = sp_state::NONE;
    let small = non_hyperspacing_ships(u);
    crate::command::cl_halt(u, &small);
    game.hyper.rollcall_screen = false;
    game.hyper.player_can_hyperspace = true;
    game.log(LogKind::Flow, "hyperspace roll call: cancelled");
    true
}

/// Which roll-call jump button the original disables (`HyperspaceRollCallBegin`): while any of
/// the player's ships was recently attacked or fired upon (+0x35a / +0x35b), "go now" is off
/// and only the emergency jump is offered; otherwise the emergency jump is off.
pub fn rollcall_go_now_disabled(u: &Universe) -> bool {
    u.ship_list.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER && (s.recently_attacked != 0 || s.recently_fired_upon != 0)))
}

/// `spGoNowHyperspaceCB` (0x512570): the roll call's "go now": the small ships dock instantly
/// and the capital ships join the mothership.
pub fn go_now(u: &mut Universe, game: &mut Game) -> bool {
    if game.sp_state != sp_state::WAITING_ROLLCALL || skips_roll_call(game.config.mission) {
        return false;
    }
    game.wide_screen_call(crate::mission::WideScreenKind::InHyperspace, 90, u.total_time_elapsed);
    let small = non_hyperspacing_ships(u);
    crate::dock::cl_dock(u, &small, crate::dock::purpose::INSTANT_STAY, None);
    for id in hyperspacing_ships(u) {
        if crate::base::class_of(u, u.ships.ship(id).ship_type) != Some(ShipClass::Mothership) {
            crate::command::remove_ships_from_doing_stuff(u, &[id]);
            crate::aitrack::force_steady_ship(u.ships.ship_mut(id));
            let r = game.hyper.rec(id);
            r.in_range = true;
            r.in_parade = true;
            join_parade(u, id);
        }
    }
    // The capital ships are put at their parade slots at once (`GetMilitaryGroupAroundShip`,
    // `setMilitaryParade`).
    if let Some(ms) = u.base.players[HUMAN_PLAYER].mothership {
        if let Some(p) = crate::command::military_group_around(u, ms).and_then(|c| u.commands.get(c)).and_then(|c| c.parade.clone()) {
            crate::parade::set_parade(u, &p);
        }
    }
    game.hyper.rollcall_screen = false;
    game.log(LogKind::Flow, "hyperspace roll call: go now");
    true
}

/// `spEmergencyHyperspaceCB` (0x512680): jump with the capital ships and leave the rest behind.
pub fn emergency_hyperspace(u: &mut Universe, game: &mut Game) -> bool {
    if game.sp_state != sp_state::WAITING_ROLLCALL || skips_roll_call(game.config.mission) {
        return false;
    }
    game.wide_screen_call(crate::mission::WideScreenKind::InHyperspace, 90, u.total_time_elapsed);
    for id in non_hyperspacing_ships(u) {
        crate::command::remove_ships_from_doing_stuff(u, &[id]);
        game.hyper.rec(id).abandoned = true;
    }
    for id in hyperspacing_ships(u) {
        if crate::base::class_of(u, u.ships.ship(id).ship_type) != Some(ShipClass::Mothership) {
            crate::command::remove_ships_from_doing_stuff(u, &[id]);
            crate::aitrack::force_steady_ship(u.ships.ship_mut(id));
            let r = game.hyper.rec(id);
            r.in_range = true;
            r.in_parade = true;
            join_parade(u, id);
        }
    }
    game.hyper.rollcall_screen = false;
    game.log(LogKind::Flow, "hyperspace roll call: emergency hyperspace");
    true
}

/// `univPlayerHasShipUnderConstruction` (0x548200).
fn ship_under_construction(u: &Universe) -> bool {
    u.ship_list.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER && s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0))
}

/// `clCancelAllLaunchOrdersFromPlayer`.
fn cancel_launch_orders(u: &mut Universe) {
    let launching: Vec<ShipId> = u
        .commands
        .iter()
        .filter(|c| c.order == crate::command::order::LAUNCH_SHIP)
        .flat_map(|c| c.selection.iter().copied())
        .filter(|&s| u.ships.get(s).is_some_and(|x| x.player == HUMAN_PLAYER))
        .collect();
    if !launching.is_empty() {
        crate::command::remove_ships_from_doing_stuff(u, &launching);
    }
}

// ---------------------------------------------------------------------------------------------
// Which ships go.
// ---------------------------------------------------------------------------------------------

/// `spIsPlayerFleetShipForHyperspace` (0x512820).
pub fn is_fleet_ship(u: &Universe, mission: u32, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let ally = u.players.get(HUMAN_PLAYER).is_some_and(|p| p.is_ally(s.player));
    let mut ok = ally && s.flags & (sof::DEAD | sof::DISABLED) == 0 && !matches!(s.ship_type, ShipType::sSwarmer | ShipType::bSwarmer);
    if s.base.disabled && crate::base::class_of(u, s.ship_type) != Some(ShipClass::Component) {
        if s.ship_type != ShipType::dEcclesiam {
            ok = false;
        }
        if mission == 5 && s.ship_type == ShipType::kCarrier {
            ok = true;
        }
        if s.ship_type == ShipType::sSentinel {
            return true;
        }
    }
    ok
}

/// Capital ships and modules make the jump themselves (`isCapitalShip` or class Component).
pub fn can_hyperspace(u: &Universe, t: ShipType) -> bool {
    u.statics.ship(t).is_some_and(|s| s.is_capital_ship || s.ship_class == Some(ShipClass::Component))
}

/// `selectAllCurrentPlayersNonHyperspacingShips` (0x50e0d0).
pub fn non_hyperspacing_ships(u: &Universe) -> Vec<ShipId> {
    let m = u.base.options.mission.max(0) as u32;
    u.ship_list.iter().copied().filter(|&id| is_fleet_ship(u, m, id) && !can_hyperspace(u, u.ships.ship(id).ship_type)).collect()
}

/// `selectAllCurrentPlayersHyperspacingShips` (0x50e150).
pub fn hyperspacing_ships(u: &Universe) -> Vec<ShipId> {
    let m = u.base.options.mission.max(0) as u32;
    u.ship_list.iter().copied().filter(|&id| is_fleet_ship(u, m, id) && can_hyperspace(u, u.ships.ship(id).ship_type)).collect()
}

fn class(u: &Universe, id: ShipId) -> Option<ShipClass> {
    u.ships.get(id).and_then(|s| crate::base::class_of(u, s.ship_type))
}

/// `TellHyperspacingShipsToReady` (0x512940): the mothership is in range by definition; the
/// others are in range when they already fly in its parade.
fn tell_hyperspacing_ships_to_ready(u: &mut Universe, game: &mut Game) {
    let mut sel = hyperspacing_ships(u);
    for &id in &sel {
        if class(u, id) == Some(ShipClass::Mothership) {
            game.hyper.rec(id).in_range = true;
        } else {
            // (sResourceCollector-style refuelling of type 0x58, tResourceCollector, is not
            // needed: fuel is not modelled.) The parade is not modelled, so no ship is in it.
            let r = game.hyper.rec(id);
            r.in_range = r.in_parade;
        }
    }
    // MakeShipsNotIncludeLaunchingShips: capital ships being launched keep launching.
    sel.retain(|&id| {
        let launching = u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::LAUNCH_SHIP);
        !launching
    });
    crate::command::remove_ships_from_doing_stuff(u, &sel);
}

/// `CalculateRollCall` (0x5128b0).
fn calculate_roll_call(u: &Universe, game: &mut Game) {
    let m = game.config.mission;
    let (mut docked, mut remaining) = (0, 0);
    for &id in &u.ship_list {
        if !is_fleet_ship(u, m, id) {
            continue;
        }
        let s = u.ships.ship(id);
        if let Some(i) = &s.base.inside {
            docked += i.ships.len() as i32;
        }
        let r = game.hyper.get(id);
        if !can_hyperspace(u, s.ship_type) {
            if !r.abandoned {
                remaining += 1;
            }
        } else if !matches!(class(u, id), Some(ShipClass::Mothership) | Some(ShipClass::Component)) {
            if r.in_range {
                docked += 1;
            } else {
                remaining += 1;
            }
        }
    }
    game.hyper.rollcall_docked = docked;
    game.hyper.rollcall_remaining = remaining;
}

/// `OrientHyperspacingShips` (0x513100): capital ships out of range fly to the mothership; in
/// range they turn to its heading. True when every one in range is on heading.
fn orient_hyperspacing_ships(u: &mut Universe, game: &mut Game) -> bool {
    let Some(ms) = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| u.ships.get(m).is_some()) else { return true };
    let mp = u.ships.ship(ms).posinfo.position;
    let heading = u.ships.ship(ms).heading();
    let m = game.config.mission;
    let (range_sq, accuracy) = (game.sp_tweaks.hyperspace_range_sq, game.sp_tweaks.heading_accuracy);
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let mut all = true;
    for id in u.ship_list.clone() {
        if !is_fleet_ship(u, m, id) || !can_hyperspace(u, u.ships.ship(id).ship_type) {
            continue;
        }
        let c = class(u, id);
        if matches!(c, Some(ShipClass::Mothership) | Some(ShipClass::Component)) {
            continue;
        }
        u.ships.ship_mut(id).autostabilize_ship = false;
        u.ships.ship_mut(id).ship_idle = false;
        if !game.hyper.get(id).in_range {
            let p = u.ships.ship(id).posinfo.position;
            if ld(range_sq) <= crate::level::distance_squared(p, mp) {
                crate::aiship::fly_to_point_avoiding_objs(u, id, Some(mp), 0x33, 0.0, None);
            } else {
                let launching = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::LAUNCH_SHIP);
                if !launching {
                    game.hyper.rec(id).in_range = true;
                }
            }
        } else {
            let Some(stat) = statics.ship(u.ships.ship(id).ship_type) else { continue };
            let mut h = heading;
            let on = crate::aitrack::heading(u.ships.ship_mut(id), stat, &tw, dt, &mut h, accuracy, accuracy, stat.sinbank, 0);
            if !on {
                game.hyper.rollcall_remaining += 1;
            }
            all &= on;
        }
    }
    all
}

/// The ships that take part in the jump: fleet ships that can hyperspace and are in range.
fn jumping(u: &Universe, game: &Game, id: ShipId) -> bool {
    is_fleet_ship(u, game.config.mission, id) && can_hyperspace(u, u.ships.ship(id).ship_type) && game.hyper.get(id).in_range
}

/// `CalculateHyperspaceTimeForShip` (0x512a70).
fn calculate_hyperspace_time(u: &mut Universe, game: &mut Game, id: ShipId, ms: Option<ShipId>) {
    let tw = game.sp_tweaks.clone();
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id).clone();
    let other = ms.filter(|&m| m != id).and_then(|m| u.ships.get(m));
    let offset = match other {
        Some(m) => {
            let d = Vector::new(st(ld(s.posinfo.position.x) - ld(m.posinfo.position.x)), st(ld(s.posinfo.position.y) - ld(m.posinfo.position.y)), st(ld(s.posinfo.position.z) - ld(m.posinfo.position.z)));
            let c = &m.rotinfo.coordsys;
            let along = |k: usize| {
                let a = c.col(k);
                st(ld(d.x) * ld(a.x) + ld(d.y) * ld(a.y) + ld(d.z) * ld(a.z))
            };
            Vector::new(along(2), along(1), along(0))
        }
        None => Vector::ZERO,
    };
    let delay = if other.is_none() {
        0.0
    } else {
        let r = u.randy.random(RAN_GAME);
        st(ld(tw.random_waittime) * 2.328_306_4e-10 * fild_u32(r) + ld(tw.base_waittime))
    };
    let r = game.hyper.rec(id);
    r.offset = offset;
    r.state = leave_state::WAITING;
    r.hs_state = hs_effect::INACTIVE;
    r.hs_flags = 0;
    r.delay = delay;
    r.time = st(ld(now) + ld(delay));
    // shipHasJustDisappeared: nobody keeps it as a target.
    crate::command::ship_has_just_disappeared(u, id);
}

/// `AddShipToHyperspace` (0x512be0): into the hyperspace list; in single player the ship, its
/// modules and the ships inside it are healed.
fn add_ship_to_hyperspace(u: &mut Universe, game: &mut Game, id: ShipId) {
    if !game.hyper.in_hyperspace.contains(&id) {
        game.hyper.in_hyperspace.push(id);
    }
    let statics = u.statics.clone();
    let heal = |u: &mut Universe, s: ShipId| {
        if let Some(sh) = u.ships.get_mut(s) {
            if let Some(stat) = statics.ship(sh.ship_type) {
                sh.health = stat.health.max_health;
            }
        }
    };
    u.ships.ship_mut(id).flags |= sof::HYPERSPACE;
    heal(u, id);
    let modules: Vec<ShipId> = u.ships.iter().filter(|m| m.base.clamp.as_ref().and_then(|c| c.host) == Some(id)).map(|m| m.id).collect();
    for m in modules {
        heal(u, m);
    }
    let inside: Vec<ShipId> = u.ships.ship(id).base.inside.as_ref().map(|i| i.ships.clone()).unwrap_or_default();
    for s in inside {
        heal(u, s);
    }
}

/// `GroupShipIntoMilitaryParade` around the player's mothership (the ship's own modules,
/// clamped to it, stay where they are).
fn join_parade(u: &mut Universe, id: ShipId) {
    let Some(ms) = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| m != id && u.ships.get(m).is_some()) else { return };
    let Some(s) = u.ships.get(id) else { return };
    if s.flags & crate::base::SOF_MODULE != 0 || s.base.clamp.as_ref().is_some_and(|c| c.host.is_some()) {
        return;
    }
    crate::command::group_ship_into_military_parade(u, id, ms);
    if let Some(s) = u.ships.get_mut(id) {
        s.special_flags |= 0x8000;
    }
}

/// Into hyperspace at once (the skipped roll call, `InstantlyHyperspaceOut`'s effect).
fn jump_now(u: &mut Universe, game: &mut Game, id: ShipId) {
    let ms = u.base.players[HUMAN_PLAYER].mothership;
    crate::command::remove_ships_from_doing_stuff(u, &[id]);
    crate::aitrack::force_steady_ship(u.ships.ship_mut(id));
    if Some(id) != ms {
        game.hyper.rec(id).in_parade = true;
        join_parade(u, id);
    }
    crate::level::preset_ships_to_position(u);
    calculate_hyperspace_time(u, game, id, ms);
    game.hyper.rec(id).state = leave_state::GONE;
    if u.ships.ship(id).flags & sof::HYPERSPACE == 0 {
        crate::base::remove_ship_from_outside(u, id, false);
        add_ship_to_hyperspace(u, game, id);
    }
}

/// `UpdateHyperspacingShip` (0x512cc0), end-of-mission jump: true once the ship is in
/// hyperspace.
fn update_hyperspacing_ship(u: &mut Universe, game: &mut Game, id: ShipId, mid: bool) -> bool {
    let now = u.total_time_elapsed;
    let tw = game.sp_tweaks.clone();
    let ms = u.base.players[HUMAN_PLAYER].mothership;
    game.hyper.rec(id).midlevel_out = mid;
    let r = game.hyper.get(id);
    match r.hs_state {
        hs_effect::INACTIVE => {
            if r.time <= now {
                let display = crate::effects::hyperspace::should_display(u, id);
                let rec = game.hyper.rec(id);
                rec.state = leave_state::ACCELERATING;
                rec.time = st(ld(tw.accelerate_time) + ld(now));
                // hsStart(ship, HYPERSPACE_SLICE_RATE, into, display).
                rec.hs_state = hs_effect::POPUP_INTO;
                rec.clipt = 1.0;
                rec.rate = tw.slice_rate;
                crate::effects::hyperspace::start(u, id, hs_effect::POPUP_INTO, display);
                crate::effects::hyperspace::note_state(u, id, leave_state::ACCELERATING, hs_effect::POPUP_INTO);
            }
            false
        }
        hs_effect::SLICING_INTO | hs_effect::COLLAPSE_INTO if mid => {
            step(u, game, id);
            false
        }
        hs_effect::SLICING_INTO => {
            let others = u.ship_list.iter().any(|&o| jumping(u, game, o) && matches!(game.hyper.get(o).hs_state, hs_effect::POPUP_INTO | hs_effect::INACTIVE));
            if !others {
                step(u, game, id);
            }
            false
        }
        hs_effect::COLLAPSE_INTO => {
            if Some(id) == ms {
                let others = !game.hyper.whiteout && u.ship_list.iter().any(|&o| o != id && jumping(u, game, o) && game.hyper.get(o).hs_state != hs_effect::FINISHED);
                game.hyper.whiteout = false;
                if others {
                    return false;
                }
                game.hyper.whiteout = true;
                game.hyper.whiteout_t = 1.0 - r.clipt;
            }
            step(u, game, id);
            false
        }
        hs_effect::FINISHED => {
            game.hyper.rec(id).state = leave_state::GONE;
            if u.ships.ship(id).flags & sof::HYPERSPACE != 0 {
                return true;
            }
            crate::base::remove_ship_from_outside(u, id, true);
            if !mid && u.ships.ship(id).player != HUMAN_PLAYER {
                // Ships of other players that went along are left behind.
                crate::base::wipe_inside_ship(u, id);
                return true;
            }
            add_ship_to_hyperspace(u, game, id);
            true
        }
        _ => {
            step(u, game, id);
            false
        }
    }
}

/// `hsUpdate` on a leaving ship, with the window effect following the clip plane.
fn step(u: &mut Universe, game: &mut Game, id: ShipId) {
    let tw = game.sp_tweaks.clone();
    let r = game.hyper.rec(id);
    let ended = crate::mission::hs_update_fields(&mut r.hs_state, &mut r.clipt, &mut r.rate, r.hs_flags, &tw);
    let (state, hs_state, clipt) = (r.state, r.hs_state, r.clipt);
    if crate::effects::hyperspace::should_display(u, id) && hs_state != hs_effect::INACTIVE && hs_state != hs_effect::FINISHED {
        crate::effects::hyperspace::continue_effect(u, id, hs_state, clipt, game.hyper.hyperspace_fails);
    }
    if ended {
        crate::effects::hyperspace::finish(u, id);
    }
    crate::effects::hyperspace::note_state(u, id, state, hs_state);
}

/// The screen fade at the top of `singlePlayerGameUpdate`: half of it per second of game time.
pub(crate) fn fade_update(u: &Universe, game: &mut Game) {
    let h = &mut game.hyper;
    if 0.0 < h.fade {
        let now = u.total_time_elapsed;
        let mut prev = h.fade_last;
        if h.fade_reset {
            h.fade_reset = false;
            prev = now;
        }
        h.fade_last = now;
        h.fade = st(ld(h.fade) - (ld(now) - ld(prev)) * 0.5);
    }
}

/// States 2 to 5 and 10 of `singlePlayerGameUpdate` (the jump out).
pub(crate) fn update(u: &mut Universe, game: &mut Game) {
    let ms = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| u.ships.get(m).is_some());
    if ms.is_none() {
        if game.sp_state == sp_state::WAITING_ROLLCALL {
            // The mothership died during the roll call: the jump is off.
            speech(u, game, SPEECH_HYPERSPACE_ABORTED);
            game.sp_state = sp_state::NONE;
            let small = non_hyperspacing_ships(u);
            crate::command::cl_halt(u, &small);
            game.hyper.rollcall_screen = false;
            game.hyper.player_can_hyperspace = true;
            game.log(LogKind::Flow, "hyperspace aborted: no mothership");
        }
        return;
    }
    let now = u.total_time_elapsed;
    let m = game.config.mission;
    match game.sp_state {
        sp_state::WAITING_ROLLCALL if skips_roll_call(m) => {
            if now <= game.hyper.no_roll_call_until {
                game.hyper.fade = st(1.0 - (ld(game.hyper.no_roll_call_until) - ld(now)) / 5.0);
                return;
            }
            for id in std::mem::take(&mut game.hyper.in_hyperspace) {
                crate::base::wipe_inside_ship(u, id);
            }
            let small = non_hyperspacing_ships(u);
            let n = crate::dock::cl_dock(u, &small, crate::dock::purpose::INSTANT_STAY, None);
            game.log(LogKind::Flow, format!("hyperspace: {n} of {} small ships dock at once", small.len()));
            let ms = u.base.players[HUMAN_PLAYER].mothership;
            for id in u.ship_list.clone() {
                game.hyper.rec(id).in_range = true;
                if is_fleet_ship(u, m, id) && can_hyperspace(u, u.ships.ship(id).ship_type) {
                    let _ = ms;
                    jump_now(u, game, id);
                }
            }
            enter_whiteout(u, game, 2.5);
            game.log(LogKind::Flow, format!("fleet jumped to hyperspace: {} ships", game.hyper.in_hyperspace.len()));
        }
        sp_state::WAITING_ROLLCALL => {
            calculate_roll_call(u, game);
            orient_hyperspacing_ships(u, game);
            if game.hyper.rollcall_remaining == 0 {
                if game.hyper.rollcall_screen {
                    go_now(u, game);
                    game.hyper.rollcall_screen = false;
                }
                game.sp_state = sp_state::WAITING_FLEET_COMMAND;
                game.hyper.substate = 0;
                game.hyper.timestamp = now;
                game.hyper.fleet_command_time = st(ld(now) + 2.5);
                speech(u, game, SPEECH_ROLLCALL_COMPLETE);
                game.log(LogKind::Flow, "hyperspace roll call complete (state 3)");
            }
        }
        sp_state::WAITING_FLEET_COMMAND => {
            if game.hyper.fleet_command_time <= now {
                game.sp_state = sp_state::SHIPS_LEAVING;
            }
        }
        sp_state::SHIPS_LEAVING => {
            if game.hyper.substate == 0 {
                let oriented = orient_hyperspacing_ships(u, game);
                if now < game.hyper.timestamp || !oriented {
                    return;
                }
                let ms = u.base.players[HUMAN_PLAYER].mothership;
                for id in u.ship_list.clone() {
                    if jumping(u, game, id) {
                        calculate_hyperspace_time(u, game, id, ms);
                    }
                }
                game.hyper.substate = 1;
                for id in std::mem::take(&mut game.hyper.in_hyperspace) {
                    crate::base::wipe_inside_ship(u, id);
                }
                game.log(LogKind::Flow, "fleet leaving through hyperspace (state 4)");
                return;
            }
            let mut all = true;
            for id in u.ship_list.clone() {
                if u.ships.get(id).is_none() || !jumping(u, game, id) {
                    continue;
                }
                let done = update_hyperspacing_ship(u, game, id, false);
                if !game.hyper.hyperspace_fails || !done {
                    all = false;
                }
            }
            if !all {
                return;
            }
            enter_whiteout(u, game, 0.0);
            game.log(LogKind::Flow, format!("fleet in hyperspace: {} ships", game.hyper.in_hyperspace.len()));
        }
        sp_state::WHITEOUT => {
            if now < game.hyper.timestamp {
                return;
            }
            if game.hyper.bink_pending {
                // animBinkPlay(mission, mission + 1): the between-missions movie is not played.
                game.hyper.bink_pending = false;
                game.hyper.whiteout = false;
                game.hyper.fade = 1.0;
                game.log(LogKind::Flow, format!("between-missions movie {m} -> {} not played", m + 1));
                return;
            }
            finish_jump(u, game);
        }
        _ => {}
    }
}

fn enter_whiteout(u: &Universe, game: &mut Game, extra: f32) {
    let now = u.total_time_elapsed;
    game.sp_state = sp_state::WHITEOUT;
    game.hyper.timestamp = st(ld(game.sp_tweaks.whiteout_time) + ld(now) + ld(extra));
    game.hyper.whiteout = true;
    game.hyper.whiteout_t = 1.0;
    game.hyper.bink_pending = true;
}

/// The end of state 5: damage effects forgotten, the ships the mission marked to be deleted at
/// its end wiped (with any of them inside other ships), and the next level due.
fn finish_jump(u: &mut Universe, game: &mut Game) {
    let marked = |u: &Universe, game: &Game, id: ShipId| game.delete_when_mission_ends.contains(&id) || u.ships.get(id).is_some_and(|s| s.attributes & 0x40 != 0);
    for id in game.hyper.in_hyperspace.clone() {
        crate::effects::damage::stop_effect(u, id, crate::effects::damage::ALL);
        if marked(u, game, id) {
            crate::base::wipe_inside_ship(u, id);
            game.hyper.in_hyperspace.retain(|&s| s != id);
            continue;
        }
        let inside: Vec<ShipId> = u.ships.get(id).and_then(|s| s.base.inside.as_ref()).map(|i| i.ships.clone()).unwrap_or_default();
        for s in inside {
            if marked(u, game, s) {
                crate::dock::remove_ship_from_inside(u, s, id);
            }
        }
    }
    if std::env::var_os("HWC_DEBUG_JUMP").is_some() {
        for s in u.ships.iter().filter(|s| s.player == 0 && matches!(s.ship_type, ShipType::sRecon | ShipType::sWorker)) {
            let host = crate::base::find_ship_i_am_inside(u, s.id);
            eprintln!("{:?} {} dock_with {:?} state {} inside {:?} in_list {} cmd {:?} {:?} paused {} flags {:x}", s.ship_type, s.id.0, s.base.dock_with, s.base.dock_state, host, u.ship_list.contains(&s.id), s.command, s.command.and_then(|c| u.commands.get(c)).map(|c| (c.order, c.attributes, c.dock.purpose, c.selection.len())), u.universe_paused, s.flags);
        }
    }
    game.hyper.level_complete = true;
    game.sp_state = sp_state::START_KAS;
    game.hyper.fade = 1.0;
    game.log(LogKind::Flow, format!("mission {} complete: {} ships in hyperspace, next level due", game.config.mission, game.hyper.in_hyperspace.len()));
}

/// The fade-in arrival of state 9 (0x88a3a8 set): the whole fleet appears at the start point
/// at once, the docked ships are put outside, and the arrival ends when the fade has cleared.
/// Returns true when the arrival is complete.
pub(crate) fn fade_in_arrival(u: &mut Universe, game: &mut Game, sp: [f32; 4]) -> bool {
    if !game.hyper.fade_arrival_pending {
        return game.hyper.fade <= 0.0;
    }
    game.hyper.fade_arrival_pending = false;
    let rot = st(ld(sp[3]) * ld(crate::level::DEG_TO_RAD));
    let m = hwc_math::Matrix::rot_about_z(st(hwc_math::trig::fcos(ld(rot))), st(hwc_math::trig::fsin(ld(rot))));
    let mut created = Vec::new();
    for i in 0..game.arriving.len() {
        let Some(id) = crate::mission::create_arriving_ship(u, game, i) else { continue };
        let a = &game.arriving[i];
        let o = m.mul_vec(Vector::new(a.offset.x, -a.offset.y, a.offset.z));
        let p = Vector::new(st(ld(o.x) + ld(sp[0])), st(ld(o.y) + ld(sp[1])), st(ld(o.z) + ld(sp[2])));
        crate::mission::put_ship_outside_at_pub(u, id, p, rot);
        if let Some(s) = u.ships.get_mut(id) {
            s.flags &= !(sof::HYPERSPACE | sof::HIDE);
        }
        game.arriving[i].state = leave_state::NONE;
        created.push(id);
    }
    let ms = u.base.players[HUMAN_PLAYER].mothership;
    for id in u.ship_list.clone() {
        let Some(s) = u.ships.get(id) else { continue };
        if s.player != HUMAN_PLAYER || !(s.ship_type == ShipType::dEcclesiam || !s.base.disabled) {
            continue;
        }
        crate::command::remove_ships_from_doing_stuff(u, &[id]);
        crate::aitrack::force_steady_ship(u.ships.ship_mut(id));
        if Some(id) != ms {
            game.hyper.rec(id).in_parade = true;
            join_parade(u, id);
        }
        if u.ships.ship(id).base.inside.as_ref().is_some_and(|i| !i.ships.is_empty()) {
            launch_docked_on_arrival(u, game, id);
        }
    }
    crate::level::preset_ships_to_position(u);
    game.hyper.fade_reset = true;
    game.hyper.fade = 1.0;
    game.log(LogKind::Flow, format!("fleet faded in at the start point: {} ships", created.len()));
    false
}

/// `spLaunchDockedShipsOnArrival` (0x5142c0): with the mothership's auto-launch on, the ships
/// inside come out at once (not from Hive frigates); in mission 4 the Ecclesiam always does.
fn launch_docked_on_arrival(u: &mut Universe, game: &mut Game, host: ShipId) {
    let t = u.ships.ship(host).ship_type;
    let put_out = |u: &mut Universe, s: ShipId| {
        crate::dock::remove_ship_from_inside(u, s, host);
        crate::dock::put_ship_outside(u, s, host, Vector::ZERO, 0, 1);
        let p = u.ships.ship(host).posinfo.position;
        u.ships.ship_mut(s).posinfo.position = p;
        crate::ships::acolyte::arm_link_missiles(u, s);
    };
    if !matches!(t, ShipType::sHiveFrigate | ShipType::bHiveFrigate) && game.hyper.old_auto_launch & 1 != 0 {
        while let Some(s) = u.ships.ship(host).base.inside.as_ref().and_then(|i| i.ships.first().copied()) {
            put_out(u, s);
        }
    }
    if game.config.mission == 4 {
        let ecc: Vec<ShipId> = u.ships.ship(host).base.inside.as_ref().map(|i| i.ships.iter().copied().filter(|&s| u.ships.get(s).is_some_and(|x| x.ship_type == ShipType::dEcclesiam)).collect()).unwrap_or_default();
        for s in ecc {
            put_out(u, s);
        }
    }
}

/// The end of the arrival (both kinds) for a carried fleet: the labs of the ships present,
/// the auto-launch the player had, and the ships that must be launched.
pub(crate) fn arrival_complete(u: &mut Universe, game: &mut Game) {
    for id in u.ship_list.clone() {
        let Some(s) = u.ships.get(id) else { continue };
        let (p, t) = (s.player, s.ship_type);
        if u.players[p].race == crate::statics::ShipRace::Somtaaw {
            crate::research::activate_lab(u, p, t, false);
        }
    }
    let bits = game.hyper.old_auto_launch;
    crate::dock::cl_auto_launch(u, bits, HUMAN_PLAYER);
    for &id in &u.ship_list {
        if let Some(r) = game.hyper.ships.get_mut(&id) {
            r.state = leave_state::NONE;
            r.in_range = false;
        }
    }
    // Only the player's (and allied) ships leave the hyperspace list: ships the mission sent
    // to hyperspace at its start (`TeamHyperspaceOut` in an FSM's INIT) stay there until KAS
    // brings them in.
    let keep: Vec<ShipId> = game
        .hyper
        .in_hyperspace
        .iter()
        .copied()
        .filter(|&id| u.ships.get(id).is_some() && !crate::shockwave::is_ally(u, id, HUMAN_PLAYER))
        .collect();
    game.hyper.in_hyperspace = keep;
}

/// A carried ship back in the universe (`univCreateShip` without the mission's starting
/// modules; the modules it had travel as entries of their own), with its own state and the
/// ships that travelled inside it docked inside it again.
pub(crate) fn create_carried_ship(u: &mut Universe, f: &FleetShip) -> Result<ShipId, crate::universe::CreateError> {
    let id = spawn(u, f, true)?;
    u.ships.ship_mut(id).flags |= sof::HYPERSPACE | sof::HIDE;
    for d in &f.docked {
        let Ok(inner) = spawn(u, d, false) else { continue };
        crate::dock::put_ship_inside(u, inner, id);
        for dd in &d.docked {
            if let Ok(x) = spawn(u, dd, false) {
                crate::dock::put_ship_inside(u, x, inner);
            }
        }
    }
    Ok(id)
}

/// A ship of the fleet docked inside `host` (`dockPutShipInside`), with the ships that travel
/// inside it.
pub(crate) fn create_docked_inside(u: &mut Universe, f: &FleetShip, host: ShipId) -> Result<ShipId, crate::universe::CreateError> {
    let inner = spawn(u, f, false)?;
    crate::dock::put_ship_inside(u, inner, host);
    for d in &f.docked {
        if let Ok(x) = spawn(u, d, false) {
            crate::dock::put_ship_inside(u, x, inner);
        }
    }
    Ok(inner)
}

fn spawn(u: &mut Universe, f: &FleetShip, outside: bool) -> Result<ShipId, crate::universe::CreateError> {
    use crate::base::{create_flags, CreateOptions};
    let _ = outside;
    let flags = create_flags::NOT_IN_LISTS;
    let id = crate::base::create_ship(u, f.ship_type, HUMAN_PLAYER, Vector::ZERO, CreateOptions { flags, no_modules: true, ..Default::default() })?;
    crate::upgrade::init_static_upgrade_bits(u, id);
    crate::construction::unit_cap_create_ship(u, id);
    if u.players[HUMAN_PLAYER].race == crate::statics::ShipRace::Somtaaw {
        crate::research::activate_lab(u, HUMAN_PLAYER, f.ship_type, false);
    }
    if crate::level::is_mothership_type(f.ship_type) && outside && f.ship_type == ShipType::sMothership {
        u.base.players[HUMAN_PLAYER].mothership = Some(id);
    }
    let s = u.ships.ship_mut(id);
    if let Some(c) = f.color_scheme {
        s.color_scheme = c;
    }
    if let Some(t) = f.tactics {
        s.tactics = t;
    }
    s.attributes = f.attributes;
    s.flags |= sof::HIDE;
    Ok(id)
}

impl Carry {
    /// The fleet `warp.script` gives for starting at mission `mission` (`WarpInFleet`,
    /// 0x516ae0): a mothership with the mission's modules, the script's ships, its RUs and its
    /// technologies, as if the previous mission had just ended. The original puts the ships in
    /// a military parade around the mothership before they jump; the parade is not modelled, so
    /// the ships are lined up beside it.
    pub fn warp(statics: std::sync::Arc<crate::statics::StaticWorld>, tweaks: crate::tunables::SimTweaks, src: &dyn crate::statics::DataSource, mission: u32) -> Option<Carry> {
        let lines = crate::level::read_lines(src, "warp.script")?;
        let (mut rus, mut ships, mut techs) = (0, Vec::<(ShipType, i32)>::new(), Vec::new());
        let mut level = 0u32;
        for (k, v) in lines {
            let v = v.split(';').next().unwrap_or("").trim().to_string();
            match k.to_ascii_lowercase().as_str() {
                "level" => level = v.parse().unwrap_or(0),
                "resources" if level == mission => rus = v.parse().unwrap_or(0),
                "ships" if level == mission => {
                    let mut it = v.split(',').map(str::trim);
                    if let (Some(t), Some(n)) = (it.next().and_then(ShipType::from_name), it.next().and_then(|n| n.parse::<i32>().ok())) {
                        ships.push((t, n));
                    }
                }
                "tech" if level == mission => techs.push(v),
                _ => {}
            }
        }
        let mut u = Universe::new(statics, tweaks, &[crate::statics::ShipRace::Somtaaw, crate::statics::ShipRace::Taiidan]);
        u.single_player = true;
        crate::base::init(&mut u);
        for t in &techs {
            if let Some(tech) = crate::statics::Technology::from_name(t) {
                u.base.players[HUMAN_PLAYER].tech.add(tech.index());
            }
        }
        u.base.players[HUMAN_PLAYER].resource_units = rus;
        let mut fleet = vec![FleetShip { carried: true, ..FleetShip::new(ShipType::sMothership) }];
        for &m in crate::level::mission_module_types(mission) {
            fleet.push(FleetShip { module: true, carried: true, ..FleetShip::new(m) });
        }
        let mut k = 0;
        for (t, n) in ships {
            let module = u.statics.ship(t).is_some_and(|s| s.ship_class == Some(ShipClass::Component));
            for _ in 0..n {
                let mut f = FleetShip { module, carried: true, ..FleetShip::new(t) };
                if !module {
                    k += 1;
                    let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                    f.offset = Vector::new(-600.0 * ((k / 2) as f32), side * 2500.0, 0.0);
                }
                fleet.push(f);
            }
        }
        Some(Carry {
            mission: mission.saturating_sub(1),
            fleet,
            resource_units: rus,
            player: u.base.players[HUMAN_PLAYER].clone(),
            total_time: 0.0,
            randy: Randy::default(),
            old_auto_launch: 0xf,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// A carry-over as text (the viewer's --carry file). Floats are written as their bits so that a
// mission started from a file behaves exactly like one started from the carry in memory.
// ---------------------------------------------------------------------------------------------

const CARRY_MAGIC: &str = "hwc-carry 1";

impl Carry {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let f = |x: f32| format!("{:08x}", x.to_bits());
        let p = &self.player;
        let mut o = String::new();
        let _ = writeln!(o, "{CARRY_MAGIC}");
        let _ = writeln!(o, "mission {}", self.mission);
        let _ = writeln!(o, "rus {}", self.resource_units);
        let _ = writeln!(o, "time {}", f(self.total_time));
        let _ = writeln!(o, "autolaunch {}", self.old_auto_launch);
        for s in &self.randy.streams {
            let _ = writeln!(o, "random {} {} {} {} {}", s.x, s.y, s.z, s.c, s.n);
        }
        let techs: Vec<String> = p.tech.has.iter().map(|t| t.to_string()).collect();
        let _ = writeln!(o, "tech {} {}", p.tech.mode, techs.join(" "));
        let allowed: Vec<String> = p.research_allowed.iter().map(|&a| u8::from(a).to_string()).collect();
        let _ = writeln!(o, "allowed {}", allowed.join(" "));
        let _ = writeln!(o, "player {} {} {} {} {} {}", p.su_capacity, p.auto_launch, u8::from(p.can_do_research), p.next_topic_id, p.max_tech_level, p.upgrade_bits);
        for t in &p.topics {
            let _ = writeln!(o, "topic {} {} {}", t.id, t.tech, f(t.time_left));
        }
        for l in &p.labs {
            let _ = writeln!(o, "lab {} {}", l.status, l.topic.map_or(-1, |t| t as i64));
        }
        fn ship(o: &mut String, depth: usize, s: &FleetShip, f: &dyn Fn(f32) -> String) {
            let _ = writeln!(
                o,
                "ship {depth} {} {} {} {} {} {} {} {}",
                s.ship_type,
                f(s.offset.x),
                f(s.offset.y),
                f(s.offset.z),
                u8::from(s.module),
                s.color_scheme.map_or(-1, |c| c as i64),
                s.tactics.map_or(-1, |c| c as i64),
                s.attributes
            );
            for d in &s.docked {
                ship(o, depth + 1, d, f);
            }
        }
        for s in &self.fleet {
            ship(&mut o, 0, s, &f);
        }
        o
    }

    /// Reads [`to_text`](Self::to_text)'s format; the player is rebuilt from a fresh
    /// single-player research state with the saved values on top.
    pub fn from_text(text: &str, statics: std::sync::Arc<crate::statics::StaticWorld>, tweaks: crate::tunables::SimTweaks) -> Result<Carry, String> {
        let mut lines = text.lines();
        if lines.next().map(str::trim) != Some(CARRY_MAGIC) {
            return Err("not a carry file".into());
        }
        let mut u = Universe::new(statics, tweaks, &[crate::statics::ShipRace::Somtaaw, crate::statics::ShipRace::Taiidan]);
        u.single_player = true;
        crate::base::init(&mut u);
        let mut c = Carry { mission: 0, fleet: Vec::new(), resource_units: 0, player: u.base.players[HUMAN_PLAYER].clone(), total_time: 0.0, randy: Randy::default(), old_auto_launch: 0xf };
        c.player.labs = Default::default();
        let (mut rnd, mut lab) = (0, 0);
        let fl = |s: &str| u32::from_str_radix(s, 16).map(f32::from_bits).map_err(|e| format!("{s}: {e}"));
        let mut stack: Vec<FleetShip> = Vec::new();
        let mut depths: Vec<usize> = Vec::new();
        let pop_to = |stack: &mut Vec<FleetShip>, depths: &mut Vec<usize>, fleet: &mut Vec<FleetShip>, d: usize| {
            while depths.last().is_some_and(|&x| x >= d) {
                let s = stack.pop().unwrap();
                depths.pop();
                match stack.last_mut() {
                    Some(parent) => parent.docked.push(s),
                    None => fleet.push(s),
                }
            }
        };
        for line in lines {
            let w: Vec<&str> = line.split_whitespace().collect();
            let Some(&k) = w.first() else { continue };
            let n = |i: usize| -> Result<i64, String> { w.get(i).ok_or(format!("short line: {line}"))?.parse::<i64>().map_err(|e| format!("{line}: {e}")) };
            match k {
                "mission" => c.mission = n(1)? as u32,
                "rus" => c.resource_units = n(1)? as i32,
                "time" => c.total_time = fl(w[1])?,
                "autolaunch" => c.old_auto_launch = n(1)? as u32,
                "random" => {
                    if let Some(s) = c.randy.streams.get_mut(rnd) {
                        *s = hwc_math::random::RanStream { x: n(1)? as u32, y: n(2)? as u32, z: n(3)? as u32, c: n(4)? as u32, n: n(5)? as u32 };
                    }
                    rnd += 1;
                }
                "tech" => {
                    c.player.tech.mode = n(1)? as i32;
                    for i in 0..crate::base::NUM_TECHS {
                        c.player.tech.has[i] = n(i + 2)? as i32;
                    }
                }
                "allowed" => {
                    for i in 0..c.player.research_allowed.len() {
                        c.player.research_allowed[i] = n(i + 1)? != 0;
                    }
                }
                "player" => {
                    c.player.su_capacity = n(1)? as i32;
                    c.player.auto_launch = n(2)? as u32;
                    c.player.can_do_research = n(3)? != 0;
                    c.player.next_topic_id = n(4)? as u32;
                    c.player.max_tech_level = n(5)? as i32;
                    c.player.upgrade_bits = n(6)? as u32;
                }
                "topic" => c.player.topics.push(crate::base::ResearchTopic { id: n(1)? as u32, tech: n(2)? as usize, time_left: fl(w[3])? }),
                "lab" => {
                    if let Some(l) = c.player.labs.get_mut(lab) {
                        l.status = n(1)? as i32;
                        l.topic = u32::try_from(n(2)?).ok();
                    }
                    lab += 1;
                }
                "ship" => {
                    let d = n(1)? as usize;
                    pop_to(&mut stack, &mut depths, &mut c.fleet, d);
                    let t = ShipType::from_name(w.get(2).copied().unwrap_or("")).ok_or(format!("unknown ship type in {line}"))?;
                    let s = FleetShip {
                        ship_type: t,
                        offset: Vector::new(fl(w[3])?, fl(w[4])?, fl(w[5])?),
                        docked: Vec::new(),
                        module: n(6)? != 0,
                        carried: true,
                        color_scheme: usize::try_from(n(7)?).ok(),
                        tactics: usize::try_from(n(8)?).ok(),
                        attributes: n(9)? as u32,
                    };
                    stack.push(s);
                    depths.push(d);
                }
                _ => return Err(format!("unknown line: {line}")),
            }
        }
        pop_to(&mut stack, &mut depths, &mut c.fleet, 0);
        c.player.resource_units = c.resource_units;
        Ok(c)
    }
}

/// The carried fleet as the level loads (`singlePlayerLoadNewLevel`): the ships exist, in
/// hyperspace and out of the universe lists, so that the mission's start finds the player's
/// mothership (`GetPlayerMothership`) before the fleet arrives.
pub(crate) fn create_fleet_in_hyperspace(u: &mut Universe, game: &mut Game) {
    let fleet = game.fleet.clone();
    let mut ids = Vec::new();
    let mut ms = None;
    for f in &fleet {
        match create_carried_ship(u, f) {
            Ok(id) => {
                if f.ship_type == ShipType::sMothership && ms.is_none() {
                    ms = Some(id);
                    u.base.players[HUMAN_PLAYER].mothership = Some(id);
                    if game.level.motherships.is_empty() {
                        game.level.motherships.push(None);
                    }
                    game.level.motherships[HUMAN_PLAYER] = Some(id);
                }
                game.hyper.in_hyperspace.push(id);
                ids.push(Some(id));
            }
            Err(e) => {
                game.log(LogKind::Warning, format!("{}: {e}", f.ship_type));
                ids.push(None);
            }
        }
    }
    // The modules ride on the mothership wherever they are in the list.
    if let Some(m) = ms {
        for (f, id) in fleet.iter().zip(&ids) {
            if let (true, Some(id)) = (f.module, *id) {
                crate::level::attach_module(u, &mut game.level, id, m);
            }
        }
    }
    game.hyper.fleet_ids = ids;
}

/// The list part of `PutShipOutsideAt` for a ship coming out of hyperspace.
pub(crate) fn bring_outside(u: &mut Universe, id: ShipId) {
    if u.ship_list.contains(&id) {
        return;
    }
    crate::base::add_ship_to_outside(u, id, id);
    crate::fow::add_viewer(u, id);
}

// ---------------------------------------------------------------------------------------------
// Mid-level hyperspace: ships KAS sends out of the level and brings back in
// (`spHyperspaceSelectionOut` / `spHyperspaceSelectionIn`, `UpdateMidLevelHyperspacingShips`).
// ---------------------------------------------------------------------------------------------

/// Starts in hyperspace (`+0x38` 0x80, set by the level or `ShipsCreateHyperspace`).
pub const START_IN_HYPERSPACE: u32 = 0x80;

/// `aiuFilterDisabledShips`: ships that are not disabled.
pub fn filter_disabled(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    ships.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & sof::DISABLED == 0 && !s.base.disabled)).collect()
}

/// `spHyperspaceSelectionOut` (0x513a60): each ship (and the ships clamped to it) goes to
/// hyperspace: those marked to start in hyperspace at once, the others through the window.
pub(crate) fn hyperspace_selection_out(u: &mut Universe, game: &mut Game, ships: &[ShipId]) {
    let Some(&lead) = ships.first() else { return };
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        if s.flags & sof::HYPERSPACE != 0 || game.hyper.to_hyperspace.contains(&id) {
            continue;
        }
        if s.attributes & START_IN_HYPERSPACE == 0 {
            detach_clamped_for_hyperspace(u, id);
            calculate_hyperspace_time(u, game, id, Some(lead));
            let delay = game.hyper.kas_delay;
            let host = u.ships.ship(id).base.clamp.as_ref().and_then(|c| c.host);
            let r = game.hyper.rec(id);
            r.time = st(ld(delay) + ld(r.time));
            // A ship clamped to one that is already on its way leaves with it.
            if let Some(h) = host {
                if game.hyper.to_hyperspace.contains(&h) {
                    let t = game.hyper.get(h).time;
                    game.hyper.rec(id).time = t;
                }
            }
            game.hyper.to_hyperspace.push(id);
        } else {
            calculate_hyperspace_time(u, game, id, Some(lead));
            game.hyper.rec(id).state = leave_state::GONE;
            if u.ships.ship(id).flags & sof::HYPERSPACE == 0 {
                crate::base::remove_ship_from_outside(u, id, false);
                add_ship_to_hyperspace(u, game, id);
                game.hyper.rec(id).midlevel_out = false;
            }
            u.ships.ship_mut(id).attributes &= !START_IN_HYPERSPACE;
        }
        let clamped: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&c| u.ships.get(c).and_then(|x| x.base.clamp.as_ref()).and_then(|k| k.host) == Some(id) && !game.hyper.to_hyperspace.contains(&c))
            .collect();
        if !clamped.is_empty() {
            hyperspace_selection_out(u, game, &clamped);
        }
    }
}

/// `spDetachShipsClampedToForHyperspace` (0x5139a0), the part the simulation models: what is
/// latched onto the ship (not its modules) lets go.
fn detach_clamped_for_hyperspace(u: &mut Universe, id: ShipId) {
    let latched: Vec<ShipId> = u
        .ships
        .iter()
        .filter(|s| s.base.clamp.as_ref().and_then(|c| c.host) == Some(id) && crate::base::class_of(u, s.ship_type) != Some(ShipClass::Component))
        .map(|s| s.id)
        .collect();
    for l in latched {
        crate::clamp::unclamp(u, l);
    }
}

/// `spHyperspaceSelectionIn` (0x5137e0): ships in hyperspace (and those clamped to them) are
/// marked to come in at `dest`.
pub(crate) fn hyperspace_selection_in(u: &mut Universe, game: &mut Game, ships: &[ShipId], dest: [f32; 4]) {
    for &id in ships {
        let clamped: Vec<ShipId> = game
            .hyper
            .in_hyperspace
            .iter()
            .copied()
            .filter(|&c| u.ships.get(c).and_then(|x| x.base.clamp.as_ref()).and_then(|k| k.host) == Some(id) && !game.hyper.get(c).midlevel_in)
            .collect();
        if !clamped.is_empty() {
            hyperspace_selection_in(u, game, &clamped, dest);
        }
        let delay = game.hyper.kas_delay;
        let r = game.hyper.rec(id);
        r.midlevel_in = true;
        r.dest = dest;
        r.time = st(ld(delay) + ld(r.time));
    }
}

/// `UpdateMidLevelHyperspacingShips` (0x514820), at the start of every single-player update.
pub(crate) fn update_midlevel(u: &mut Universe, game: &mut Game) {
    let mut i = 0;
    while i < game.hyper.to_hyperspace.len() {
        let id = game.hyper.to_hyperspace[i];
        let done = u.ships.get(id).is_none() || update_hyperspacing_ship(u, game, id, true);
        if done {
            game.hyper.to_hyperspace.remove(i);
        } else {
            i += 1;
        }
    }
    for id in game.hyper.in_hyperspace.clone() {
        if !game.hyper.get(id).midlevel_in || u.ships.get(id).is_none() {
            continue;
        }
        if update_arriving_midlevel(u, game, id) {
            let r = game.hyper.rec(id);
            r.midlevel_in = false;
            r.state = leave_state::NONE;
            r.in_range = false;
            game.hyper.in_hyperspace.retain(|&s| s != id);
        }
    }
}

/// `UpdateArrivingShip` (0x513340) for a mid-level arrival: no waiting, the window opens at
/// once and the ship is done when its deceleration time is over.
fn update_arriving_midlevel(u: &mut Universe, game: &mut Game, id: ShipId) -> bool {
    use crate::mission::ship_hs;
    let now = u.total_time_elapsed;
    let tw = game.sp_tweaks.clone();
    let r = game.hyper.get(id);
    if let Some(s) = u.ships.get_mut(id) {
        s.autostabilize_ship = false;
        s.ship_idle = false;
    }
    if r.state == leave_state::GONE {
        let d = r.dest;
        let rot = st(ld(d[3]) * ld(crate::level::DEG_TO_RAD));
        let m = hwc_math::Matrix::rot_about_z(st(hwc_math::trig::fcos(ld(rot))), st(hwc_math::trig::fsin(ld(rot))));
        let o = m.mul_vec(Vector::new(r.offset.x, -r.offset.y, r.offset.z));
        let p = Vector::new(st(ld(o.x) + ld(d[0])), st(ld(o.y) + ld(d[1])), st(ld(o.z) + ld(d[2])));
        crate::mission::put_ship_outside_at_pub(u, id, p, rot);
        let s = u.ships.ship_mut(id);
        s.flags &= !sof::HYPERSPACE;
        s.flags |= sof::HIDE;
        let rec = game.hyper.rec(id);
        rec.state = ship_hs::WAITING;
        rec.time = 0.0;
    }
    let r = game.hyper.get(id);
    match r.state {
        ship_hs::WAITING => {
            if now <= r.time {
                return false;
            }
            if let Some(s) = u.ships.get_mut(id) {
                s.posinfo.velocity = Vector::ZERO;
                s.flags &= !sof::HIDE;
            }
            let display = crate::effects::hyperspace::should_display(u, id);
            let rec = game.hyper.rec(id);
            rec.state = ship_hs::DECELERATING;
            rec.time = st(ld(tw.decelerate_time) + ld(now));
            rec.rate = tw.slice_rate;
            rec.clipt = 1.0;
            rec.hs_state = hs_effect::POPUP_OUTOF;
            crate::effects::hyperspace::start(u, id, hs_effect::POPUP_OUTOF, display);
            crate::effects::hyperspace::note_state(u, id, ship_hs::DECELERATING, hs_effect::POPUP_OUTOF);
            false
        }
        ship_hs::DECELERATING => {
            if r.hs_state == hs_effect::INACTIVE {
                return r.time < now;
            }
            step(u, game, id);
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_mission_tables_match_the_original() {
        let no_roll: Vec<u32> = (0..19).filter(|&m| skips_roll_call(m)).collect();
        assert_eq!(no_roll, vec![1, 2, 5, 7, 15, 17]);
        let fade: Vec<u32> = (0..19).filter(|&m| fades_in(m)).collect();
        assert_eq!(fade, vec![4, 5, 6, 7, 8, 10, 12, 14, 15, 17]);
        assert!(!skips_roll_call(40) && !fades_in(40));
    }

    #[test]
    fn unavailable_modules_are_wiped() {
        use ShipType::*;
        let m = |t| FleetShip { module: true, carried: true, ..FleetShip::new(t) };
        let fleet = vec![FleetShip::new(sMothership), m(sMothershipEnginesLower), m(sMothershipLowerSpine), m(sMothershipCommand), m(sMothershipDockingBay), m(sMothershipMicro)];
        let types = |v: Vec<FleetShip>| v.into_iter().map(|f| f.ship_type).collect::<Vec<_>>();
        // Mission 3 keeps the lower engines and spine; the built modules stay.
        assert_eq!(types(wipe_unavailable_modules(&fleet, 3)), vec![sMothership, sMothershipEnginesLower, sMothershipLowerSpine, sMothershipCommand, sMothershipDockingBay, sMothershipMicro]);
        // Mission 4 has neither, and loses the docking bay.
        assert_eq!(types(wipe_unavailable_modules(&fleet, 4)), vec![sMothership, sMothershipCommand, sMothershipMicro]);
        // Mission 5 keeps the docking bay.
        assert_eq!(types(wipe_unavailable_modules(&fleet, 5)), vec![sMothership, sMothershipCommand, sMothershipDockingBay, sMothershipMicro]);
    }
}
