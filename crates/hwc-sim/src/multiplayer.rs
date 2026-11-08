//! The rules of a multiplayer game (skirmish or network) while it runs: resource injections
//! and the lump sum (`univUpdate` 0x547410), players losing (`univKillOtherPlayersIfDead`
//! 0x543cc0, `univKillPlayer`, `DisableAllPlayersShips`), backup motherships
//! (`univFindBackupMothership` 0x543b20) and the end of the game (`CheckPlayerWin` 0x543680,
//! `DeclarePlayerWin`).
//!
//! The original's game-over clock is the real-time `universe.totaltimeelapsed` twin at
//! 0xa07e44; the simulation uses its own game clock, which runs the same while the game is not
//! paused.

use hwc_math::random::RAN_GAME;
use hwc_math::fpu::{ld, st};

use crate::base::game_flags;
use crate::object::{flags2, sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// `MG_*` flags of `tpGameCreated.flag` beyond [`game_flags`] (Cataclysm's numbering).
pub mod mg {
    /// `MG_LastMotherShip`: a player is out when the last mothership goes.
    pub const LAST_MOTHERSHIP: u32 = 0x0002;
    /// `MG_CaptureCapitalShip`: every player starts with a capital ship (a Hive Frigate).
    pub const CAPTURE_CAPITAL_SHIP: u32 = 0x0004;
    pub const RESEARCH_ENABLED: u32 = 0x0008;
    /// Cataclysm's "allow allies".
    pub const ALLOW_ALLIES: u32 = 0x0010;
    pub const HARVESTING_ENABLED: u32 = 0x0020;
    pub const RESOURCE_INJECTIONS: u32 = 0x0040;
    pub const RESOURCE_LUMP_SUM: u32 = 0x0080;
    pub const PASSWORD_PROTECTED: u32 = 0x0100;
    pub const UNIT_CAPS_ENABLED: u32 = 0x0200;
    pub const FUEL_BURN_ENABLED: u32 = 0x0400;
    pub const ALLIED_VICTORY: u32 = 0x0800;
    pub const CRATES_ENABLED: u32 = 0x1000;
    /// Cataclysm's start-ship option (`mgStartShip`), HW1's carrier-is-mothership bit.
    pub const CARRIER_IS_MOTHERSHIP: u32 = 0x2000;
    pub const HYPERSPACE: u32 = 0x4000;
}

/// How a player left the game (`typeOfPlayerDeath`).
pub mod death {
    pub const STANDARD: i32 = 0;
    pub const DROPPED_OUT: i32 = 1;
    pub const CAPTURED_SHIP: i32 = 2;
    pub const LOST_MISSION: i32 = 3;
}

/// Speech events (`STAT_F_*`).
mod speech_event {
    pub const RESOURCE_INJECTION_RECEIVED: u32 = 0x200_1044;
    pub const REASSIGNED_TO_CARRIER: u32 = 0x200_101d;
    pub const PLAYER_DIES: u32 = 0x200_102d;
}

/// What happened to the game this update, for the front end (messages, the winner screen).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MpEvent {
    /// Resources were injected (or the lump sum paid out) to every live player.
    ResourcesInjected { amount: i32 },
    /// `DisplayPlayerHasBeenDefeatedMessage`.
    PlayerDefeated { player: usize, how: i32 },
    /// A player's mothership went and another of its ships took over.
    MothershipReassigned { player: usize, ship: ShipId },
    /// `DeclarePlayerWin`: the players still alive won.
    Won { winners: u32 },
    /// The quit clock ran out: the game is over.
    GameOver,
    /// A crate appeared.
    CrateAppeared { position: hwc_math::Vector },
    /// A crate was found; `reward` is what the player got.
    CrateFound { player: usize, position: hwc_math::Vector, reward: crate::crates::Reward },
    /// A crate expired unclaimed.
    CrateExpired { position: hwc_math::Vector },
}

/// The multiplayer state the universe keeps (`tpGameCreated`'s amounts, `lasttimeadded`, the
/// win and quit clocks, the statistics of each player's end).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MpState {
    /// A multiplayer game: the rules here run.
    pub active: bool,
    /// The game is played over the network (`multiPlayerGame`): `CheckPlayerWin` counts the
    /// players left; a skirmish ends when the local player is out or every other one is.
    pub network: bool,
    /// The players in the game (`universe.numPlayers`); the level's own objects belong to
    /// the one after them.
    pub num_players: usize,
    /// The local player (`universe.curPlayerIndex`).
    pub local_player: usize,
    /// `tpGameCreated.resourceInjectionsAmount` (0x87bfa4).
    pub injection_amount: i32,
    /// `tpGameCreated.resourceLumpSumAmount` (0x87bfac).
    pub lump_sum_amount: i32,
    /// `universe.lasttimeadded` (0xa07f44): the update of the last injection.
    pub last_injection: u32,
    /// The time the winner is declared (0xa07f3c), 0 when not set.
    pub win_time: f32,
    /// The time the game ends (0xa07f40), 0 when not set.
    pub quit_time: f32,
    /// `PLAYERLOSE_SHIPDEATHTIMEMIN` / `MAX`: a lost player's ships blow up this long after.
    pub ship_death_time_min: f32,
    pub ship_death_time_max: f32,
    /// `WAIT_TIME_TO_QUIT`, `WAIT_TIME_TO_WIN`.
    pub wait_time_to_quit: f32,
    pub wait_time_to_win: f32,
    /// `playerStats[i].timeOfDeath` and `typeOfPlayerDeath`, per player (`None`: alive).
    pub deaths: Vec<Option<(f32, i32)>>,
    /// The players declared winners (a bit each), once [`MpEvent::Won`] has happened.
    pub winners: Option<u32>,
    /// The game is over (the quit clock ran out).
    pub over: bool,
    /// Each player's bounty (`player->bounty`, `TW_BountySizes[bountySize]`; bounties are
    /// not recalculated during the game yet). Crates are placed with it.
    pub bounties: Vec<u8>,
    /// The crates (`MG_CratesEnabled`).
    pub crates: crate::crates::Crates,
    /// This update's events (not saved).
    #[serde(skip)]
    pub events: Vec<MpEvent>,
}

impl Default for MpState {
    fn default() -> Self {
        Self {
            active: false,
            network: false,
            num_players: 0,
            local_player: 0,
            injection_amount: 0,
            lump_sum_amount: 0,
            last_injection: 0,
            win_time: 0.0,
            quit_time: 0.0,
            ship_death_time_min: 5.0,
            ship_death_time_max: 10.0,
            wait_time_to_quit: 10.0,
            wait_time_to_win: 5.0,
            deaths: Vec::new(),
            winners: None,
            over: false,
            bounties: Vec::new(),
            crates: Default::default(),
            events: Vec::new(),
        }
    }
}

fn alive(u: &Universe, p: usize) -> bool {
    u.base.players.get(p).is_some_and(|pl| pl.state == 0)
}

fn num_players(u: &Universe) -> usize {
    u.mp.num_players.min(u.base.players.len())
}

/// The start of `univUpdate`: the winner is declared and the game ends when their clocks run
/// out.
pub fn update_clocks(u: &mut Universe) {
    if !u.mp.active {
        return;
    }
    u.mp.events.clear();
    if u.mp.quit_time == 0.0 {
        return;
    }
    let now = u.total_time_elapsed;
    if u.mp.win_time != 0.0 && u.mp.win_time <= now {
        declare_player_win(u);
        u.mp.win_time = 0.0;
    }
    if u.mp.quit_time <= now && !u.mp.over {
        u.mp.over = true;
        u.mp.events.push(MpEvent::GameOver);
    }
}

/// `univUpdate` after the research update: the resource injections every
/// `resourceInjectionInterval` updates and the one lump sum at update
/// `resourceLumpSumTime`.
pub fn update_resources(u: &mut Universe) {
    if u.single_player || !u.mp.active {
        return;
    }
    let flags = u.base.options.flags;
    let counter = u.update_counter;
    if flags & mg::RESOURCE_INJECTIONS != 0 && counter.wrapping_sub(u.mp.last_injection) == u.base.options.injection_interval {
        u.mp.last_injection = counter;
        let amount = u.mp.injection_amount;
        give_every_live_player(u, amount);
    }
    if flags & mg::RESOURCE_LUMP_SUM != 0 && counter == u.base.options.lump_sum_frame {
        let amount = u.mp.lump_sum_amount;
        give_every_live_player(u, amount);
    }
}

/// `resourceInjectionTask` (0x53a110) / `resourceLumpSum` (0x53a160), with the fleet's
/// announcement to the local player.
fn give_every_live_player(u: &mut Universe, amount: i32) {
    for p in 0..num_players(u) {
        if !alive(u, p) {
            continue;
        }
        u.base.players[p].resource_units += amount;
        let st_ = &mut u.base.stats;
        if st_.players.len() <= p {
            st_.players.resize(p + 1, crate::base::PlayerStats::default());
        }
        st_.players[p].ru_total += amount;
        st_.players[p].ru_injected += amount;
    }
    u.mp.events.push(MpEvent::ResourcesInjected { amount });
    if alive(u, u.mp.local_player) {
        crate::speech::queue(u, None, speech_event::RESOURCE_INJECTION_RECEIVED, -1);
    }
}

/// Whether `s` can stand in for a player's mothership (`univFindBackupMothership`): the
/// race's carrier, or any ship of the mothership class.
fn backup_candidate(u: &Universe, race: ShipRace, t: ShipType) -> bool {
    let class_mothership = u.statics.ship(t).is_some_and(|s| s.ship_class == Some(ShipClass::Mothership));
    match race {
        ShipRace::Somtaaw => t == ShipType::sCarrier || class_mothership,
        ShipRace::Beast => t == ShipType::bCarrier || class_mothership,
        _ => false,
    }
}

/// `univFindBackupMothership` (0x543b20): another live ship of the player becomes its
/// mothership: a carrier of its race or a mothership-class ship. The fleet says so when the old one was the race's
/// mothership and the game is running.
pub fn find_backup_mothership(u: &mut Universe, player: usize, game_running: bool) -> bool {
    let race = u.players[player].race;
    let current = u.base.players[player].mothership;
    let found = u
        .ship_list
        .iter()
        .filter_map(|&id| u.ships.get(id))
        .find(|s| s.flags & sof::DEAD == 0 && s.player == player && Some(s.id) != current && backup_candidate(u, race, s.ship_type))
        .map(|s| s.id);
    if let Some(id) = found {
        let old_type = current.and_then(|c| u.ships.get(c)).map(|s| s.ship_type);
        let announce = game_running && matches!((race, old_type), (ShipRace::Somtaaw, Some(ShipType::sMothership)) | (ShipRace::Beast, Some(ShipType::bMothership)));
        if announce && player == u.mp.local_player {
            crate::speech::queue(u, None, speech_event::REASSIGNED_TO_CARRIER, -1);
        }
        u.base.players[player].mothership = Some(id);
        u.mp.events.push(MpEvent::MothershipReassigned { player, ship: id });
        return true;
    }
    // (With hyperspace on, the original also searches the ships in mid-level hyperspace,
    // `universe.HsInProgress`; multiplayer hyperspace is not ported yet.)
    false
}

/// `TryToFindMothershipsForPlayers` (0x49eec0), after a level loads: a player the level gave
/// no mothership takes a backup one.
pub fn try_to_find_motherships(u: &mut Universe) {
    for p in 0..num_players(u) {
        if u.base.players[p].mothership.is_none() {
            find_backup_mothership(u, p, false);
        }
    }
}

/// `univKillPlayer`: the player is out. Its ships careen and blow up within
/// `PLAYERLOSE_SHIPDEATHTIMEMIN..MAX` seconds (`DisableAllPlayersShips`).
pub fn kill_player(u: &mut Universe, player: usize, how: i32) {
    if player >= num_players(u) || !alive(u, player) {
        return;
    }
    u.base.players[player].mothership = None;
    u.base.players[player].state = 1;
    if u.mp.deaths.len() < num_players(u) {
        u.mp.deaths.resize(num_players(u), None);
    }
    u.mp.deaths[player] = Some((u.total_time_elapsed, how));
    if player == u.mp.local_player {
        crate::speech::queue(u, None, speech_event::PLAYER_DIES, player as i32);
    }
    u.mp.events.push(MpEvent::PlayerDefeated { player, how });
    disable_all_players_ships(u, player);
}

/// `DisableAllPlayersShips`.
fn disable_all_players_ships(u: &mut Universe, player: usize) {
    let (lo, hi) = (u.mp.ship_death_time_min, u.mp.ship_death_time_max);
    for id in u.ship_list.clone() {
        let Some(s) = u.ships.get(id) else { continue };
        if s.player != player {
            continue;
        }
        if s.flags & sof::DISABLED == 0 {
            u.ships.ship_mut(id).flags |= sof::CRAZY;
            let r = u.randy.random(RAN_GAME);
            let now = u.total_time_elapsed;
            let t = st((ld(hi) - ld(lo)) * (1.0 / 4_294_967_296.0) * f64::from(r as f32) + ld(lo) + ld(now));
            u.ships.ship_mut(id).death_time = t;
            if u.ships.ship(id).flags2 & flags2::DISGUISED != 0 {
                crate::ships::mimic::end_disguise(u, id, false);
            }
            let t = u.ships.ship(id).ship_type;
            if !u.single_player && t != ShipType::sMothership && t != ShipType::bMothership {
                crate::disable::apply_careen_rotation(u, id);
            }
        }
        u.ships.ship_mut(id).flags &= !sof::SELECTABLE;
    }
}

/// `univKillOtherPlayersIfDead` (0x543cc0), for each ship `univDeleteDeadShips` deletes
/// (`was_mothership`: it was its player's mothership when it died): a player who lost the
/// mothership with no backup (with "last mothership" on, or no ships left), or the last ship,
/// is out.
pub fn ship_deleted(u: &mut Universe, player: usize, was_mothership: bool) {
    if !u.mp.active || u.single_player || player >= num_players(u) {
        return;
    }
    if was_mothership {
        u.base.players[player].mothership = None;
        if find_backup_mothership(u, player, true) {
            return;
        }
        if u.base.options.flags & mg::LAST_MOTHERSHIP == 0 && u.base.players[player].total_ships != 0 {
            return;
        }
    } else if u.base.players[player].total_ships != 0 {
        return;
    }
    let was_alive = alive(u, player);
    kill_player(u, player, death::STANDARD);
    if was_alive {
        check_player_win(u);
    }
}

/// `CheckPlayerWin` (0x543680): when the game is decided, the winner is declared
/// `WAIT_TIME_TO_WIN` seconds later and the game ends after `WAIT_TIME_TO_QUIT`.
pub fn check_player_win(u: &mut Universe) {
    let n = num_players(u);
    let decided = if !u.mp.network {
        // A skirmish: decided when the local player is out, or every other player is.
        !alive(u, 0) || (1..n).all(|p| !alive(u, p))
    } else {
        let left: Vec<usize> = (0..n).filter(|&p| alive(u, p)).collect();
        if left.len() < 2 {
            true
        } else if u.base.options.flags & mg::ALLIED_VICTORY != 0 {
            // Allied victory: everyone left is allied with the first of them.
            let first = left[0];
            left.iter().all(|&p| u.players[first].is_ally(p))
        } else {
            false
        }
    };
    if decided {
        let now = u.total_time_elapsed;
        u.mp.quit_time = st(ld(u.mp.wait_time_to_quit) + ld(now));
        u.mp.win_time = st(ld(u.mp.wait_time_to_win) + ld(now));
    }
}

/// `DeclarePlayerWin`: the players still alive are the winners.
fn declare_player_win(u: &mut Universe) {
    let winners = (0..num_players(u)).filter(|&p| alive(u, p)).fold(0u32, |m, p| m | 1 << p);
    u.mp.winners = Some(winners);
    u.mp.events.push(MpEvent::Won { winners });
}

/// Whether harvesting is on (`MG_HarvestinEnabled`, always in single player).
pub fn harvesting_enabled(u: &Universe) -> bool {
    u.single_player || u.base.options.flags & game_flags::HARVESTING_ENABLED != 0
}
