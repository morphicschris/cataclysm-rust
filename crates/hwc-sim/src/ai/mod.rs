//! The computer player of a multiplayer game (`AIPlayer.c`, `AIFleetMan.c`; see
//! `docs/ai-player-plan.md`): per computer player its state ([`AiPlayer`], `struct AIPlayer`
//! 0x1d18 bytes), the choice of enemies, the turn every `aiplayerUpdateRate` updates
//! (`aiplayerUpdateAll` → `aifFleetCommand`), the build requests of its managers and the
//! building they lead to.
//!
//! Ported so far: the player, fleet command, the build requests, the resource manager
//! ([`resource`]), the attack manager ([`attack`]) and the teams with the moves it uses
//! ([`team`]). The defence manager and the resource manager's team moves come next.

pub mod attack;
pub mod research;
pub mod resource;
pub mod team;

use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// Skill levels (`AIPlayerLevel`).
pub mod level {
    pub const BEGINNER: u32 = 0;
    pub const INTERMEDIATE: u32 = 1;
    pub const ADVANCED: u32 = 2;
}

/// `aiplayerUpdateRate` by level (0x740868): the player's turn comes when
/// `univUpdateCounter & rate == player & rate`.
const UPDATE_RATE: [u32; 3] = [63, 31, 15];
/// `aifResearchDelay` by level (0x740884): turns between research checks.
const RESEARCH_DELAY: [u8; 3] = [2, 1, 0];
/// The managers' shares of the spending (0x74087c, 0x740874, 0x740878).
const RESOURCE_RATIO: f32 = 0.4;
const ATTACK_RATIO: f32 = 0.3;
const DEFENSE_RATIO: f32 = 0.3;
/// `AIF_HYPER_SKIM` (`aifHyperspaceInit`).
const HYPER_SKIM: f32 = 0.08;

/// `ConvertSliderToAIPlayerDifficulty`: the CPU difficulty slider (0..100) as a level.
pub fn level_from_slider(slider: u8) -> u32 {
    if slider < 0x22 {
        level::BEGINNER
    } else if slider > 0x42 {
        level::ADVANCED
    } else {
        level::INTERMEDIATE
    }
}

/// Which manager a team or request belongs to (`AITeamType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Manager {
    Attack = 0,
    Defense = 1,
    Resource = 2,
    Script = 3,
}

/// A request for ships (`RequestShips`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Request {
    pub ship_type: ShipType,
    /// Ships still to build (-1: refused).
    pub num: i32,
    /// `REQUESTSHIPS_HIPRI` and the like.
    pub priority: u32,
    /// Where it is built.
    pub creator: Option<ShipId>,
}

/// A team waiting for ships it asked for (`TeamWaitingForTheseShips`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeamWaiting {
    pub ship_type: ShipType,
    pub num: i32,
    pub team: usize,
    /// The AI variable set when the ships have all come.
    pub done_var: String,
}

/// A computer player's team (`AITeam`); its moves come with the teams step of the port.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AiTeam {
    pub manager: Manager,
    /// `teamFlags`.
    pub flags: u32,
    pub ships: Vec<ShipId>,
    /// What the team was created to do (the `aioCreate*` order), for the resource manager's
    /// teams whose moves are not ported yet.
    pub order: String,
    /// +0x0c `teamFeatures` (by team level: 0, 8, 0x1f), +0x10 `teamDelay`, +0x2c team level.
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub features: u32,
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub delay: u8,
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub level: u8,
    /// +0x30 the moves, +0x3c the current one.
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub moves: Vec<team::AiMove>,
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub current: Option<usize>,
    /// +0x74 `kasTactics`.
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub tactics: i32,
}

/// `struct AIPlayer`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AiPlayer {
    pub player: usize,
    /// +0x04 `aiplayerDifficultyLevel`.
    pub level: u32,
    /// +0x08 `ResourceFeatures`, +0x0c `AttackFeatures`, +0x10 `DefenseFeatures`.
    pub resource_features: u32,
    pub attack_features: u32,
    pub defense_features: u32,
    /// +0x14 `aiplayerUpdateRate`, +0x18 `aiplayerFrameCount`.
    pub update_rate: u32,
    pub frame_count: u32,
    /// +0x1c `enemyPlayerCount`, +0x20 `primaryEnemyPlayer`.
    pub enemy_player_count: u32,
    pub primary_enemy: usize,
    /// +0x24 `newships`: ships not yet handed to a manager.
    pub new_ships: Vec<ShipId>,
    /// +0x93c `NumRUDockPoints`.
    pub ru_dock_points: i32,
    /// +0x940.. ships being built: resource collectors, controllers (processors), advance
    /// support frigates, research ships, support (modules and carrier support).
    pub collectors_being_built: i32,
    pub controllers_being_built: i32,
    pub asf_being_built: i32,
    pub research_being_built: i32,
    pub support_being_built: i32,
    /// +0x954 `ScriptCreator`, +0x958 `AICreator`: where the ships are built.
    pub script_creator: Option<ShipId>,
    pub ai_creator: Option<ShipId>,
    /// +0x95c.. RUs spent by the resource, attack, defence and script managers.
    pub spent: [i32; 4],
    /// +0x96c `ResourceManRequestShips`.
    pub resource_request: Option<Request>,
    /// +0x98c.. the attack, defence and script managers' request queues.
    pub requests: [Vec<Request>; 3],
    /// +0x9b0.. the teams waiting for ships, by manager (attack, defence, script).
    pub waiting: [Vec<TeamWaiting>; 3],
    /// +0x9d4.. the ships of each type being built for each of those managers.
    pub being_built: [Vec<u8>; 3],
    /// +0xb88 `TechnologyDeficit`.
    pub technology_deficit: i32,
    /// +0xb8c the resourcers the "dumb" count wants (0: pick one).
    pub resourcers_to_build: u8,
    /// +0xb8d turns before the next Somtaaw mothership module is asked for.
    pub module_cooldown: u8,
    /// +0xb8e a support module has been asked for.
    pub support_module_pending: bool,
    /// +0xb8f turns before the next research check.
    pub research_delay: u8,
    /// +0xb90 `firstTurn`, +0xb94 `recalculateAllies`.
    pub first_turn: bool,
    pub recalculate_allies: bool,
    /// +0xb98 `aifHyperSavings`, +0xb9c `aifHyperSkimming`, +0xba0 `aifLastRUCount`.
    pub hyper_savings: i32,
    pub hyper_skimming: f32,
    pub last_ru_count: i32,
    /// +0xba4 `airResourceReserves`: the resource manager's ships.
    pub resource_reserves: Vec<ShipId>,
    /// +0xbac `airResourceCollectors`.
    pub resource_collectors: Vec<ShipId>,
    /// +0xbb4 `airEasilyAccesibleRUsInWorld` (-1: count again).
    pub rus_in_world: i32,
    /// +0xbb8.. numbers of resource collectors, controllers, advance support frigates,
    /// research ships, support ships.
    pub num_collectors: i32,
    pub num_controllers: i32,
    pub num_asf: i32,
    pub num_research: i32,
    pub num_support: i32,
    /// +0xbcc `numSupportTeams`, +0xbd0 `supportTeam[12]`.
    pub support_teams: Vec<usize>,
    /// +0x1cc0: the proximity sensors.
    pub proximity_sensors: Vec<ShipId>,
    /// The attack manager's state.
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub attack: attack::AttackState,
}

/// Everything of the computer players.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AiState {
    /// By player number.
    pub players: Vec<Option<AiPlayer>>,
    /// The enemy group of each player (`aiplayerChooseEnemies`, 0xa0806a; -1 none).
    pub enemy_of: Vec<i8>,
    /// `aiplayerChangeBigotry` (0x8d3268): the chance (0..100) a computer player goes after a
    /// human rather than another computer.
    pub bigotry: u32,
    /// Every computer player's teams (`team->aiplayerowner` is the player).
    pub teams: Vec<Option<(usize, AiTeam)>>,
    /// `AIResourceMan.script`.
    pub resource_tweaks: resource::Tweaks,
    /// The AI variables (`AIVar.c`) and the last generated label number.
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub vars: Vec<(String, i32)>,
    #[serde(default, deserialize_with = "crate::save::added_in_11")]
    pub next_label: u32,
}

impl AiPlayer {
    /// `aiplayerInit` (0x411020) with `aifInit`, `airInit` and the attack and defence
    /// managers' feature bits.
    pub fn new(u: &Universe, player: usize, level: u32) -> Self {
        let level = level.min(2);
        let mut a = AiPlayer {
            player,
            level,
            resource_features: 0,
            attack_features: 0,
            defense_features: 0,
            update_rate: UPDATE_RATE[level as usize],
            frame_count: 0,
            enemy_player_count: 0,
            primary_enemy: 0,
            new_ships: Vec::new(),
            ru_dock_points: 0,
            collectors_being_built: 0,
            controllers_being_built: 0,
            asf_being_built: 0,
            research_being_built: 0,
            support_being_built: 0,
            script_creator: None,
            ai_creator: None,
            spent: [0; 4],
            resource_request: None,
            requests: Default::default(),
            waiting: Default::default(),
            being_built: [vec![0; crate::base::NUM_SHIP_TYPES], vec![0; crate::base::NUM_SHIP_TYPES], vec![0; crate::base::NUM_SHIP_TYPES]],
            technology_deficit: 0,
            resourcers_to_build: 0,
            module_cooldown: 0,
            support_module_pending: false,
            research_delay: 0,
            first_turn: true,
            recalculate_allies: true,
            hyper_savings: 0,
            hyper_skimming: 0.0,
            last_ru_count: u.base.players[player].resource_units,
            resource_reserves: Vec::new(),
            resource_collectors: Vec::new(),
            rus_in_world: -1,
            num_collectors: 0,
            num_controllers: 0,
            num_asf: 0,
            num_research: 0,
            num_support: 0,
            support_teams: Vec::new(),
            proximity_sensors: Vec::new(),
            attack: attack::AttackState::default(),
        };
        // aifInit: the creators are the mothership; advanced players save for hyperspace.
        let ms = u.base.players[player].mothership;
        a.script_creator = ms;
        a.ai_creator = ms;
        if level == level::ADVANCED && (u.single_player || u.base.options.flags & crate::multiplayer::mg::HYPERSPACE != 0) {
            a.resource_features |= 0x0100_0000;
        }
        resource::init(&mut a);
        a
    }
}

/// `aiplayerChooseEnemies` (0x410d30) for `total` players of whom the first `humans` are human:
/// each computer player goes after a human (with the bigotry's chance) or after another
/// computer player, spread out as evenly as the random numbers allow.
pub fn choose_enemies(u: &mut Universe, total: usize, humans: usize) {
    let computers = total.saturating_sub(humans);
    u.ai.enemy_of = vec![-1; 8];
    if computers == 0 {
        if !u.ai.enemy_of.is_empty() {
            u.ai.enemy_of[0] = 1;
        }
        return;
    }
    let rnd = |u: &mut Universe| u.randy.random(hwc_math::random::RAN_AI_PLAYER);
    let bigotry = u.ai.bigotry;
    let mut versus_ai = Vec::new();
    let mut versus_humans = Vec::new();
    for c in humans..total {
        let r = rnd(u) % 101;
        if humans == 0 || bigotry <= r {
            versus_ai.push(c);
        } else {
            versus_humans.push(c);
        }
    }
    // groups[g] lists the players whose enemy is g.
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); 8];
    for &c in &versus_ai {
        let mut g = (rnd(u) as usize) % total;
        for _ in 0..101 {
            if g != c && groups[g].is_empty() {
                groups[g].push(c);
                break;
            }
            g += 1;
            if g == total {
                g = 0;
            }
        }
    }
    if humans != 0 {
        let per = versus_humans.len() / humans + usize::from(!versus_ai.is_empty());
        let rest = versus_humans.len() % humans;
        let even = versus_humans.len() - rest;
        for &c in &versus_humans[..even] {
            let mut g = (rnd(u) as usize) % humans;
            let mut slot = 0;
            for _ in 0..101 {
                if groups[g].len() <= slot {
                    groups[g].push(c);
                    break;
                }
                slot += 1;
                if slot == per {
                    g = (g + 1) % humans;
                    slot = 0;
                }
            }
        }
        for &c in &versus_humans[even..] {
            let mut g = (rnd(u) as usize) % humans;
            let mut slot = 0;
            for _ in 0..101 {
                if groups[g].len() <= slot {
                    groups[g].push(c);
                    break;
                }
                slot += 1;
                if slot == per + 1 {
                    g = (g + 1) % humans;
                    slot = 0;
                }
            }
        }
    }
    for (g, list) in groups.iter().enumerate() {
        for &c in list {
            if c < 8 {
                u.ai.enemy_of[c] = g as i8;
            }
        }
    }
}

/// `aiplayerGameStart` (0x411860): the enemy, and the player's ships to hand out (with the
/// resource drop-off points they give).
pub fn game_start(u: &mut Universe, player: usize) {
    let total = u.mp.num_players.max(1);
    let enemy = u.ai.enemy_of.get(player).copied().unwrap_or(-1);
    let enemy = if enemy < 0 { 0 } else { enemy as usize };
    let ships: Vec<ShipId> = u.ship_list.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player == player && s.flags & sof::HIDE == 0)).collect();
    let mut dock_points = 0;
    for &id in &ships {
        let s = u.ships.ship(id);
        if u.statics.ship(s.ship_type).is_some_and(|st| st.can_receive_resources) {
            dock_points += ru_dock_points_on(s.ship_type);
        }
    }
    let Some(Some(a)) = u.ai.players.get_mut(player) else { return };
    a.enemy_player_count = (total - 1) as u32;
    a.primary_enemy = enemy;
    a.new_ships = ships;
    a.ru_dock_points += dock_points;
    a.hyper_skimming = HYPER_SKIM;
}

/// `NumRUDockPointsOnThisShip`.
pub fn ru_dock_points_on(t: ShipType) -> i32 {
    if matches!(t, ShipType::kCarrier | ShipType::tCarrier) {
        2
    } else {
        1
    }
}

/// `aiplayerUpdateAll` (0x411250), the multiplayer part: each computer player whose turn it is
/// runs its fleet command.
pub fn update_all(u: &mut Universe) {
    if !u.mp.active || u.nis_running {
        return;
    }
    for p in 0..u.ai.players.len() {
        let Some(Some(a)) = u.ai.players.get(p) else { continue };
        let rate = a.update_rate;
        if rate & u.update_counter != rate & p as u32 || u.base.players.get(p).is_none_or(|pl| pl.state == 1) {
            continue;
        }
        fleet_command(u, p);
        if let Some(Some(a)) = u.ai.players.get_mut(p) {
            a.frame_count += 1;
        }
    }
}

fn ai(u: &Universe, p: usize) -> &AiPlayer {
    u.ai.players[p].as_ref().expect("computer player")
}

fn ai_mut(u: &mut Universe, p: usize) -> &mut AiPlayer {
    u.ai.players[p].as_mut().expect("computer player")
}

/// `aifFleetCommand` (0x405ba0).
fn fleet_command(u: &mut Universe, p: usize) {
    // (aifFindAllies, aiuCreateBlobArrays, aiuUpdateKnowledgeOfEnemyShips, aidClearDistressSignal:
    // with the attack and defence managers.)
    assign_new_ships(u, p);
    if ai(u, p).first_turn {
        // aiaProcessSpecialTeams (attack manager).
        ai_mut(u, p).first_turn = false;
    } else {
        if ai(u, p).resource_features & 0x0100_0000 != 0 {
            skim_hyperspace_savings(u, p);
        }
        ai_mut(u, p).resource_request.take();
        attack::manager(u, p);
        // (aidDefenseManager: with its step of the port.)
        resource::manager(u, p);
    }
    team::execute(u, p);
    process_ship_build_requests(u, p);
}

/// `aifFleetCommand`'s hyperspace savings: a share of what came in goes aside, or a debt is
/// paid back.
fn skim_hyperspace_savings(u: &mut Universe, p: usize) {
    let mut ru = u.base.players[p].resource_units;
    let a = ai_mut(u, p);
    if a.hyper_savings < 0 {
        if ru != 0 {
            if ru < -a.hyper_savings {
                a.hyper_savings += ru;
                ru = 0;
            } else {
                ru += a.hyper_savings;
                a.hyper_savings = 0;
            }
        }
    } else if a.last_ru_count < ru {
        let take = ((ru - a.last_ru_count) as f32 * a.hyper_skimming) as i32;
        a.hyper_savings += take;
        ru -= take;
    }
    u.base.players[p].resource_units = ru;
    let ru = u.base.players[p].resource_units;
    ai_mut(u, p).last_ru_count = ru;
}

/// `aifAssignNewShips` (0x405860): each new ship goes to a team waiting for its type (script,
/// attack, defence in that order), else, for resourcers, carriers, motherships, non-combat and
/// support ships and the advance support frigates, to the resource manager. The rest wait for
/// the attack and defence managers.
fn assign_new_ships(u: &mut Universe, p: usize) {
    let mut i = 0;
    while i < ai(u, p).new_ships.len() {
        let id = ai(u, p).new_ships[i];
        let Some(s) = u.ships.get(id) else {
            ai_mut(u, p).new_ships.remove(i);
            continue;
        };
        let t = s.ship_type;
        let mut given = false;
        for (m, q) in [(2usize, 2usize), (0, 0), (1, 1)] {
            if ai(u, p).being_built[q][t.index()] == 0 {
                continue;
            }
            let a = ai_mut(u, p);
            if let Some(w) = a.waiting[m].iter().position(|w| w.ship_type == t) {
                a.waiting[m][w].num -= 1;
                let team = a.waiting[m][w].team;
                // The last of them sets the team's variable.
                let done_var = (a.waiting[m][w].num == 0).then(|| a.waiting[m].remove(w).done_var);
                a.being_built[q][t.index()] = a.being_built[q][t.index()].saturating_sub(1);
                a.new_ships.remove(i);
                team_add_ship(u, team, id);
                if let Some(v) = done_var {
                    team::var_set(u, &v, 1);
                }
                given = true;
                break;
            }
        }
        if given {
            continue;
        }
        let class = u.statics.ship(t).and_then(|st| st.ship_class);
        let to_resources = matches!(class, Some(ShipClass::Resource | ShipClass::NonCombat | ShipClass::Mothership | ShipClass::Carrier | ShipClass::Component))
            || matches!(t, ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate);
        if to_resources {
            ai_mut(u, p).new_ships.remove(i);
            resource::add_new_ship(u, p, id);
        } else {
            i += 1;
        }
    }
}

/// `aitCreate`: a new team of the player.
pub fn team_create(u: &mut Universe, p: usize, manager: Manager, order: &str) -> usize {
    // aitCreate: the team level is twice the player's; it gives the features and the delay.
    let level = (ai(u, p).level * 2).min(4) as u8;
    let features = match level {
        2 => 8,
        3 => 0x1e,
        4 => 0x1f,
        _ => 0,
    };
    let delay = [2u8, 1, 1, 0, 0][usize::from(level)];
    let tactics = if u.single_player { 1 } else { 2 };
    let t = AiTeam { manager, flags: 0, ships: Vec::new(), order: order.to_string(), features, delay, level, moves: Vec::new(), current: None, tactics };
    u.ai.teams.push(Some((p, t)));
    u.ai.teams.len() - 1
}

/// `aitAddShip`.
pub fn team_add_ship(u: &mut Universe, team: usize, id: ShipId) {
    if let Some(Some((_, t))) = u.ai.teams.get_mut(team) {
        if !t.ships.contains(&id) {
            t.ships.push(id);
        }
    }
}

/// `aitFindNextTeamWithFlag`: the player's teams with the flag, after `after`.
pub fn teams_with_flag(u: &Universe, p: usize, flag: u32) -> Vec<usize> {
    u.ai.teams.iter().enumerate().filter_map(|(i, t)| t.as_ref().filter(|(o, t)| *o == p && t.flags & flag != 0).map(|_| i)).collect()
}

/// `aifResourceManRequestsShipsCB`.
pub fn resource_requests_ships(u: &mut Universe, p: usize, t: ShipType, num: i32, priority: u32) {
    let a = ai_mut(u, p);
    a.resource_request = Some(Request { ship_type: t, num, priority, creator: a.ai_creator });
}

/// Whether a request can be taken up now: the type is the player's race's (`0x8a74d4`), its
/// statics are loaded, and an external build has a docking bay (or is a component); otherwise
/// the request is refused (`num` -1).
fn usable(u: &Universe, p: usize, r: &Request) -> Option<i32> {
    if r.num <= 0 {
        return None;
    }
    let race = u.players[p].race;
    let own = crate::research::race_types(race).contains(&r.ship_type.index());
    let st = u.statics.ship(r.ship_type)?;
    if !own || !st.info_needed {
        return None;
    }
    if st.external_build && race != ShipRace::Beast && st.ship_class != Some(ShipClass::Component) && !crate::base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, p) {
        return None;
    }
    Some(st.build_cost)
}

/// `BuildShip` (0x404470): one ship of a request, if support units allow. Normal ships go into
/// the build queue at the creator (`clWrapBuildShip`: paid now, made after the build time);
/// externally built ones are made at once (`clWrapCreateShip`) and only counted against the
/// budget.
fn build_ship(u: &mut Universe, p: usize, t: ShipType, manager: usize, budget: &mut i32, creator: Option<ShipId>) {
    let Some(st) = u.statics.ship(t) else { return };
    let (cost, su, external) = (st.build_cost, st.support_units, st.external_build);
    let pl = &u.base.players[p];
    if !u.single_player && su != 0 && pl.su_used >= pl.su_capacity {
        return;
    }
    {
        let a = ai_mut(u, p);
        match t {
            ShipType::sCarrierSupport | ShipType::sMothershipSupport | ShipType::bMothershipSupport => a.support_being_built += 1,
            ShipType::sWorker | ShipType::bWorker | ShipType::tResourceCollector => a.collectors_being_built += 1,
            ShipType::sProcessor | ShipType::bProcessor => a.controllers_being_built += 1,
            ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate => a.asf_being_built += 1,
            _ => {}
        }
    }
    let creator = creator.or(u.base.players[p].mothership);
    let Some(creator) = creator.filter(|&c| u.ships.get(c).is_some()) else { return };
    if external {
        let race = u.players[p].race;
        crate::construction::cl_create_ship(u, t, race, p, creator);
    } else {
        crate::command::cl_build_ship(u, t, p, creator);
    }
    ai_mut(u, p).spent[manager] += cost;
    *budget -= cost;
}

/// `aifRequestSupportModule` (Cataclysm): when no support module is on order, the race's one
/// (sMothershipSupport at a Somtaaw mothership, sCarrierSupport at a carrier,
/// bMothershipSupport at the Naggarok) is asked for while under its cap, or its technology
/// researched.
fn request_support_module(u: &mut Universe, p: usize) -> bool {
    let Some(ms) = u.base.players[p].mothership.and_then(|m| u.ships.get(m)).map(|s| s.ship_type) else { return false };
    if ai(u, p).support_module_pending {
        return false;
    }
    let t = match (u.players[p].race, ms) {
        (ShipRace::Somtaaw, ShipType::sMothership) => ShipType::sMothershipSupport,
        (ShipRace::Somtaaw, _) => ShipType::sCarrierSupport,
        (ShipRace::Beast, ShipType::bMothership) => ShipType::bMothershipSupport,
        _ => return false,
    };
    let have = u.base.players[p].ship_count[t.index()];
    if have >= u.statics.unit_caps.cap(t) && u.statics.unit_caps.cap(t) != -1 {
        return false;
    }
    if have != 0 || research::tech_required_for_ship(u, p, t) == 0 {
        let creator = ai(u, p).ai_creator;
        let a = ai_mut(u, p);
        a.resource_request = Some(Request { ship_type: t, num: 1, priority: 1, creator });
        a.support_module_pending = true;
        return true;
    }
    research::research_tech_for_ship(u, p, t);
    false
}

/// Whether a type can be built now for a request (`aifProcessShipBuildRequests`' test), with
/// the research it leads to. Returns (buildable, needs a hull).
fn check_request(u: &mut Universe, p: usize, t: ShipType, research_now: bool) -> (bool, bool) {
    if u.players[p].race == ShipRace::Beast && !research::ship_type_acquired(u, p, t) {
        return (false, false);
    }
    let Some(st) = u.statics.ship(t) else { return (false, false) };
    let component = st.ship_class == Some(ShipClass::Component);
    let external = st.external_build;
    let factory = u.base.players[p].mothership;
    let can = component || factory.is_some_and(|f| crate::construction::can_build_ship_type(u, f, t, false));
    if !can {
        return (false, false);
    }
    if research::tech_required_for_ship(u, p, t) != 0 {
        if research_now {
            research::research_tech_for_ship(u, p, t);
        }
        return (false, false);
    }
    if research::tech_required_for_ship_upgrades(u, p, t) != 0 && research_now {
        research::research_tech_for_ship(u, p, t);
    }
    (true, !component && external)
}

/// `aifProcessShipBuildRequests` (0x4052f0).
fn process_ship_build_requests(u: &mut Universe, p: usize) {
    let research_now = {
        let a = ai_mut(u, p);
        if a.research_delay == 0 {
            a.research_delay = RESEARCH_DELAY[a.level as usize];
            true
        } else {
            a.research_delay -= 1;
            a.research_delay == 0
        }
    };
    ai_mut(u, p).technology_deficit = 0;
    if research_now {
        request_support_module(u, p);
    }
    let mut hull = false;
    // The resource manager's request.
    let mut res = ai(u, p).resource_request;
    if let Some(r) = res.filter(|r| r.num > 0) {
        let (ok, h) = check_request(u, p, r.ship_type, research_now);
        if ok {
            hull |= h;
        } else {
            res = None;
        }
    } else {
        res = None;
    }
    // The first buildable request of each queue.
    let mut firsts: [Option<usize>; 3] = [None; 3];
    for q in [0usize, 1] {
        if hull {
            break;
        }
        let n = ai(u, p).requests[q].len();
        for i in 0..n {
            let t = ai(u, p).requests[q][i].ship_type;
            let (ok, h) = check_request(u, p, t, research_now);
            if ok {
                hull |= h;
                firsts[q] = Some(i);
                break;
            }
        }
    }
    if !hull && !ai(u, p).requests[2].is_empty() {
        firsts[2] = Some(0);
    }
    // A free lab keeps researching: the Somtaaw mothership's and carrier's upgrades, the
    // Beast's technology 0x20.
    if crate::research::find_free_lab(u, p, -1).is_some() {
        match u.players[p].race {
            ShipRace::Somtaaw => {
                for t in [ShipType::sMothership, ShipType::sCarrier] {
                    if research::tech_required_for_ship_upgrades(u, p, t) != 0 {
                        research::research_tech_for_ship(u, p, t);
                        break;
                    }
                }
            }
            ShipRace::Beast => {
                if !u.base.players[p].tech.contains(0x20) {
                    if let Some(lab) = crate::research::find_free_lab(u, p, -1) {
                        crate::research::assign_lab(u, p, lab, 0x20);
                    }
                }
            }
            _ => {}
        }
    }
    build_requested_ships(u, p, res, firsts);
}

/// `aifBuildRequestedShips` (0x4045b0): when the RUs cover every request, all are built
/// (resource, script, attack, defence). Otherwise the script's request goes first; else the
/// resource, attack and defence requests are weighed by each manager's share of the spending
/// against its ratio (a high-priority request weighs -1, a missing one 1e21) and the lightest
/// is built as far as the RUs go.
fn build_requested_ships(u: &mut Universe, p: usize, res: Option<Request>, firsts: [Option<usize>; 3]) {
    if u.base.players[p].mothership.is_none() {
        return;
    }
    let mut budget = u.base.players[p].resource_units;
    let take = |u: &Universe, q: usize| firsts[q].and_then(|i| ai(u, p).requests[q].get(i).copied());
    let mut script = take(u, 2);
    let mut attack = take(u, 0);
    let mut defense = take(u, 1);
    let mut resource = res;
    // Refused requests (wrong race, statics not loaded, no docking bay for an external build).
    let mut costs = [0i32; 4];
    for (slot, q) in [(&mut script, Some(2usize)), (&mut attack, Some(0)), (&mut defense, Some(1)), (&mut resource, None)] {
        if let Some(r) = *slot {
            match usable(u, p, &r) {
                Some(c) => {
                    let k = match q {
                        Some(2) => 3,
                        Some(0) => 1,
                        Some(1) => 2,
                        _ => 0,
                    };
                    costs[k] = c * r.num.max(0);
                }
                None => {
                    refuse(u, p, q, firsts);
                    *slot = None;
                }
            }
        }
    }
    let unit = |r: &Option<Request>| r.and_then(|r| u.statics.ship(r.ship_type)).map_or(0, |s| s.build_cost);
    let (cs, ca, cd, cr) = (unit(&script), unit(&attack), unit(&defense), unit(&resource));
    let mut built = [0i32; 3];
    // Builds up to `n` while the RUs last (or all of them when `always`).
    let run = |u: &mut Universe, t: ShipType, n: i32, cost: i32, m: usize, creator, budget: &mut i32, always: bool| -> i32 {
        let mut k = 0;
        while k < n && (always || cost <= *budget) {
            build_ship(u, p, t, m, budget, creator);
            k += 1;
        }
        k
    };
    let support = |t: ShipType| matches!(t, ShipType::sMothershipSupport | ShipType::sCarrierSupport | ShipType::bMothershipSupport);
    if costs.iter().sum::<i32>() <= budget {
        if let Some(r) = resource {
            run(u, r.ship_type, r.num, cr, 0, r.creator, &mut budget, true);
        }
        if let Some(r) = script {
            built[2] = run(u, r.ship_type, r.num, cs, 3, r.creator, &mut budget, true);
        }
        if let Some(r) = attack {
            built[0] = run(u, r.ship_type, r.num, ca, 1, r.creator, &mut budget, true);
        }
        if let Some(r) = defense {
            built[1] = run(u, r.ship_type, r.num, cd, 2, r.creator, &mut budget, true);
        }
    } else if let Some(r) = script {
        built[2] = run(u, r.ship_type, r.num, cs, 3, r.creator, &mut budget, false);
    } else {
        let spent = ai(u, p).spent;
        let total = (spent[0] + spent[1] + spent[2]) as f32;
        let key = |r: &Option<Request>, s: i32, ratio: f32| match r {
            None => 1e21f32,
            Some(r) if r.priority & 1 != 0 => -1.0,
            Some(_) => (s as f32 / total) / ratio,
        };
        let kr = key(&resource, spent[0], RESOURCE_RATIO);
        let ka = key(&attack, spent[1], ATTACK_RATIO);
        let kd = key(&defense, spent[2], DEFENSE_RATIO);
        let resource_first = if kd < ka { kr < kd } else { kr < ka };
        if resource_first {
            if let Some(r) = resource {
                if cr <= budget {
                    run(u, r.ship_type, r.num, cr, 0, r.creator, &mut budget, false);
                } else if support(r.ship_type) {
                    ai_mut(u, p).support_module_pending = false;
                }
            }
        } else {
            if kd < ka {
                if let Some(r) = defense {
                    built[1] = run(u, r.ship_type, r.num, cd, 2, r.creator, &mut budget, false);
                }
            } else if let Some(r) = attack {
                built[0] = run(u, r.ship_type, r.num, ca, 1, r.creator, &mut budget, false);
            }
            if resource.is_some_and(|r| r.num > 0 && support(r.ship_type)) {
                ai_mut(u, p).support_module_pending = false;
            }
        }
    }
    // What was built comes off the queued requests and is expected as new ships.
    for q in 0..3 {
        let Some(i) = firsts[q] else { continue };
        let a = ai_mut(u, p);
        let Some(r) = a.requests[q].get_mut(i) else { continue };
        r.num -= built[q];
        let t = r.ship_type.index();
        a.being_built[q][t] = a.being_built[q][t].saturating_add(built[q].clamp(0, 255) as u8);
        if r.num < 1 {
            a.requests[q].remove(i);
        }
    }
}

/// A request that cannot be built is refused (its ships set to -1).
fn refuse(u: &mut Universe, p: usize, q: Option<usize>, firsts: [Option<usize>; 3]) {
    let a = ai_mut(u, p);
    match q {
        None => {
            if let Some(r) = a.resource_request.as_mut() {
                if matches!(r.ship_type, ShipType::sMothershipSupport | ShipType::sCarrierSupport | ShipType::bMothershipSupport) {
                    a.support_module_pending = false;
                }
                r.num = -1;
            }
        }
        Some(q) => {
            if let Some(i) = firsts[q] {
                if i < a.requests[q].len() {
                    a.requests[q].remove(i);
                }
            }
        }
    }
}

/// `aiuUnitCapCanBuildShip` (0x41e510): how many of `n` ships of type `t` the unit caps still
/// allow, counting what is on order and requested.
pub fn unit_cap_can_build(u: &Universe, p: usize, t: ShipType, n: i32) -> i32 {
    if !u.base.unit_caps_maintained() || u.single_player {
        return n;
    }
    let a = ai(u, p);
    let class = u.statics.ship(t).and_then(|s| s.ship_class);
    let mut total = 0;
    let mut same_class = 0;
    let mut same_type = 0;
    for ty in crate::research::race_types(u.players[p].race) {
        let k: i32 = (0..3).map(|q| i32::from(a.being_built[q][ty])).sum();
        total += k;
        let c = ShipType::from_index(ty).and_then(|x| u.statics.ship(x)).and_then(|s| s.ship_class);
        if c == class {
            same_class += k;
            if ty == t.index() {
                same_type += k;
            }
        }
    }
    for q in &a.requests {
        for r in q {
            total += r.num;
            if u.statics.ship(r.ship_type).and_then(|s| s.ship_class) == class {
                same_class += r.num;
                if r.ship_type == t {
                    same_type += r.num;
                }
            }
        }
    }
    for &id in &a.new_ships {
        total += 1;
        if let Some(s) = u.ships.get(id) {
            if u.statics.ship(s.ship_type).and_then(|st| st.ship_class) == class {
                same_class += 1;
                if s.ship_type == t {
                    same_type += 1;
                }
            }
        }
    }
    let building = a.collectors_being_built + a.controllers_being_built + a.asf_being_built + a.research_being_built;
    match t {
        ShipType::sWorker | ShipType::bWorker | ShipType::tResourceCollector => same_type += a.collectors_being_built,
        ShipType::sProcessor | ShipType::bProcessor => same_type += a.controllers_being_built,
        ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate => same_type += a.asf_being_built,
        _ => {}
    }
    same_class += building;
    let caps = &u.statics.unit_caps;
    let pl = &u.base.players[p];
    let left = caps.total - (total + pl.total_ships + building);
    if left < n {
        return left.max(0);
    }
    let cap_t = caps.cap(t);
    if cap_t != -1 {
        let left = cap_t - pl.ship_count[t.index()] - same_type;
        if left < n {
            return left.max(0);
        }
    }
    if let Some(c) = class {
        let cap_c = caps.by_class[c.index()];
        if cap_c != -1 {
            let left = cap_c - pl.class_count[c.index()] - same_class;
            if left < n {
                return left.max(0);
            }
        }
    }
    n
}

/// `aiplayerShipLaunchedCallback` (0x4113a0): a ship launched from a computer player's
/// carrier or mothership waits to be handed to a manager (and is a drop-off point when it
/// takes resources).
pub fn ship_launched(u: &mut Universe, id: ShipId) {
    if !u.mp.active || u.nis_running {
        return;
    }
    let Some(s) = u.ships.get(id) else { return };
    let (p, t) = (s.player, s.ship_type);
    if u.ai.players.get(p).is_none_or(Option::is_none) {
        return;
    }
    let receives = u.statics.ship(t).is_some_and(|st| st.can_receive_resources);
    let a = ai_mut(u, p);
    if !a.new_ships.contains(&id) {
        a.new_ships.push(id);
    }
    if receives {
        a.ru_dock_points += ru_dock_points_on(t);
    }
}

/// `aiplayerShipDied` (0x411980) for the multiplayer computer players: the ship leaves their
/// lists and the managers' counts.
pub fn ship_died(u: &mut Universe, id: ShipId, owner: usize, ship_type: ShipType) {
    let receives = u.statics.ship(ship_type).is_some_and(|s| s.can_receive_resources);
    for p in 0..u.ai.players.len() {
        let Some(Some(a)) = u.ai.players.get_mut(p) else { continue };
        if owner == p && receives {
            a.ru_dock_points -= ru_dock_points_on(ship_type);
        }
        a.new_ships.retain(|&s| s != id);
        resource::ship_died(a, id, ship_type);
        attack::ship_died(a, id);
    }
    team::ship_died(u, id);
    for t in u.ai.teams.iter_mut().flatten() {
        t.1.ships.retain(|&s| s != id);
    }
}

/// A ship a computer player got (`clCreateShip` → `aiplayerShipLaunchedCallback` and the
/// like): it waits to be handed to a manager.
pub fn ship_new(u: &mut Universe, id: ShipId) {
    let Some(p) = u.ships.get(id).map(|s| s.player) else { return };
    if let Some(Some(a)) = u.ai.players.get_mut(p) {
        if !a.new_ships.contains(&id) {
            a.new_ships.push(id);
        }
    }
}
