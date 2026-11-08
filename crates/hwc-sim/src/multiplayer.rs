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
    /// Each player's bounty (`player->bounty`, from `TW_BountySizes[bountySize]`, then
    /// [`calculate_player_bounties`]). Killing its ships pays it; crates are placed with it.
    pub bounties: Vec<u8>,
    /// `tpGameCreated.bountySize` (3: off), `BountyInit`'s base (0xa08084) and the weights of
    /// the bounties' fleet value and RUs.
    #[serde(default, deserialize_with = "crate::save::added_in_13")]
    pub bounty_size: u8,
    #[serde(default, deserialize_with = "crate::save::added_in_13")]
    pub bounty_base: i32,
    #[serde(default, deserialize_with = "crate::save::added_in_13")]
    pub bounty_ship_worth: f32,
    #[serde(default, deserialize_with = "crate::save::added_in_13")]
    pub bounty_resources: f32,
    /// 0xa08080: the bounties have been worked out once.
    #[serde(default, deserialize_with = "crate::save::added_in_13")]
    pub bounties_done: bool,
    /// The crates (`MG_CratesEnabled`).
    pub crates: crate::crates::Crates,
    /// Hyperspace jumps (`MG_Hyperspace`).
    #[serde(default, deserialize_with = "crate::save::added_in_14")]
    pub hyperspace: crate::mp_hyperspace::MpHyperspace,
    /// What happened since the front end last took them (not saved).
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
            bounty_size: 3,
            bounty_base: 0,
            bounty_ship_worth: 0.0,
            bounty_resources: 0.0,
            bounties_done: false,
            crates: Default::default(),
            hyperspace: Default::default(),
            events: Vec::new(),
        }
    }
}

/// `univUpdate`'s bounty step: [`calculate_player_bounties`] the first time and then every
/// 128 updates (when `univUpdateCounter & 0x7f == 0x65`).
pub fn update_bounties(u: &mut Universe) {
    if !u.mp.active || u.single_player {
        return;
    }
    if !u.mp.bounties_done || u.update_counter & 0x7f == 0x65 {
        u.mp.bounties_done = true;
        calculate_player_bounties(u);
    }
}

/// `calculatePlayerBounties` (0x4280a0): the bounties shared out of `players * base` by each
/// player's share of the fleets' value (`TW_Bounty_CurrentShipWorth`) and of the RUs held
/// (`TW_Bounty_ResourcesInPossesion`; a computer player counts the humans' average), at most
/// 250 each.
pub fn calculate_player_bounties(u: &mut Universe) {
    let n = num_players(u);
    if n == 0 {
        return;
    }
    let pool = (n as i32).wrapping_mul(u.mp.bounty_base);
    let mut worth = vec![0f32; n];
    let counts = |s: &crate::object::Ship| s.flags & (sof::DEAD | sof::DISABLED) == 0;
    for &id in &u.ship_list {
        let Some(s) = u.ships.get(id) else { continue };
        if !counts(s) || s.player >= n {
            continue;
        }
        let cost = |t| u.statics.ship(t).map_or(0, |st| st.build_cost) as f32;
        worth[s.player] = st(ld(cost(s.ship_type)) + ld(worth[s.player]));
        // The ships inside count for their owners.
        for &i in s.base.inside.as_ref().map(|x| x.ships.as_slice()).unwrap_or(&[]) {
            let Some(c) = u.ships.get(i) else { continue };
            if counts(c) && c.player < n {
                worth[c.player] = st(ld(cost(c.ship_type)) + ld(worth[c.player]));
            }
        }
    }
    // The humans' RUs; computer players count their average.
    let humans = (0..n).filter(|&p| !u.base.players[p].ai).count();
    let mut rus = vec![0f32; n];
    let mut sum = 0f32;
    for p in 0..humans.min(n) {
        rus[p] = u.base.players[p].resource_units as f32;
        sum = st(ld(rus[p]) + ld(sum));
    }
    let avg = if humans != 0 { st(ld(sum) / humans as f64) } else { sum };
    if avg != 0.0 {
        for r in rus.iter_mut().skip(humans) {
            *r = avg;
        }
    }
    let (mut total_worth, mut total_rus) = (0f32, 0f32);
    for p in 0..n {
        total_worth = st(ld(total_worth) + ld(worth[p]));
        total_rus = st(ld(total_rus) + ld(rus[p]));
    }
    let mut share = vec![0f32; n];
    let mut total = 0f32;
    for p in 0..n {
        let a = if total_worth == 0.0 { 0.0 } else { st(ld(st(ld(worth[p]) / ld(total_worth))) * ld(u.mp.bounty_ship_worth)) };
        let b = if total_rus == 0.0 { 0.0 } else { st(ld(st(ld(u.mp.bounty_resources) / ld(total_rus))) * ld(rus[p])) };
        share[p] = st(ld(b) + ld(a));
        total = st(ld(share[p]) + ld(total));
    }
    if u.mp.bounties.len() < n {
        u.mp.bounties.resize(n, 0);
    }
    for p in 0..n {
        let v = st(ld(st(ld(share[p]) / ld(total))) * f64::from(pool as f32));
        // The FPU's rounding to the nearest (the 6.7553994e15 trick); nothing to share is 0.
        let b = if v.is_finite() { (f64::from(v)).round_ties_even() as i64 } else { 0 };
        u.mp.bounties[p] = b.clamp(0, 0xfa) as u8;
    }
}

/// `bountyShipWasKilled` (0x428310): a ship destroyed by another player's weapons pays that
/// player its owner's bounty in hundredths of its cost.
pub fn bounty_ship_was_killed(u: &mut Universe, owner: usize, ship_type: crate::statics::ShipType, how: i32, who: i32) {
    if u.single_player || !u.mp.active || u.mp.bounty_size == 3 {
        return;
    }
    if !(0x29a..=0x29c).contains(&how) || !(0..8).contains(&who) || owner >= 8 || owner as i32 == who {
        return;
    }
    let bounty = u.mp.bounties.get(owner).copied().unwrap_or(0);
    let cost = u.statics.ship(ship_type).map_or(0, |s| s.build_cost);
    let pay = (f64::from(bounty) * f64::from(f32::from_bits(0x3c23_d70a)) * f64::from(cost)) as i32;
    let k = who as usize;
    if let Some(p) = u.base.players.get_mut(k) {
        p.resource_units += pay;
    }
    if let Some(s) = u.base.stats.players.get_mut(k) {
        s.ru_bounty += pay;
        s.ru_total += pay;
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

/// A player left a network game (the `PLAYERDROPPED` sync command): the player is out, as
/// `univKillPlayer(PLAYERKILLED_DROPPEDOUT)`, and the game may be decided.
pub fn player_dropped(u: &mut Universe, player: usize) {
    let was_alive = alive(u, player);
    kill_player(u, player, death::DROPPED_OUT);
    if was_alive {
        check_player_win(u);
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
    let now = u.total_time_elapsed;
    if let Some(s) = u.base.stats.players.get_mut(player) {
        s.died_at = now;
    }
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
pub fn ship_deleted(u: &mut Universe, player: usize, was_mothership: bool, killer: Option<usize>) {
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
    // The player whose weapons took the last ship has put this one out.
    if let Some(k) = killer.filter(|&k| k != player && was_alive) {
        if let Some(s) = u.base.stats.players.get_mut(k) {
            s.players_killed += 1;
        }
    }
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
