//! Setting up a multiplayer game on one machine (a skirmish against the computer, and the
//! simulation side of a network game): the map list (`spTitleListLoad` 0x4ff370, from
//! `multiPlayerMissions.script`), the game types of `gametypes.script`, and `gameStart`'s
//! multiplayer path (0x5a3dc0) with `levelInit` (0x49f070): players and races, the start
//! positions, the level, starting resources and research.

use std::sync::Arc;

use hwc_math::random::Randy;

use crate::level::{self, LevelContext, LevelError, LevelState, MpLevel, ResourceTweaks};
use crate::multiplayer::mg;
use crate::statics::{DataSource, ShipRace, StaticWorld};
use crate::tunables::SimTweaks;
use crate::universe::Universe;

pub use hwc_formats::multiplayer::{game_type_index, parse_game_types, GameOptions, GameType};

/// The original's player limit (`MAX_MULTIPLAYER_PLAYERS`).
pub const MAX_PLAYERS: usize = 8;

/// One map of the multiplayer map list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapInfo {
    /// The directory and level name under `MultiPlayer\`, e.g. `Coldwar2`.
    pub name: String,
    /// The number at the end of the name.
    pub max_players: usize,
    /// Every map takes two players or more.
    pub min_players: usize,
}

impl MapInfo {
    /// `MultiPlayer\<name>\` and `<name>.level`.
    pub fn paths(&self) -> (String, String) {
        (format!("MultiPlayer\\{}\\", self.name), format!("{}.level", self.name))
    }
}

/// The maps of `multiPlayerMissions.script` whose level exists, in file order (`//` and `;`
/// lines are comments). The original then adds the directories under `MultiPlayer\` that
/// are not listed and sorts the list by title; the shipped data lists every map.
pub fn map_list(src: &dyn DataSource) -> Vec<MapInfo> {
    let Some(bytes) = src.read("multiPlayerMissions.script") else { return Vec::new() };
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    hwc_formats::multiplayer::parse_map_list(&text)
        .into_iter()
        .map(|(name, max_players)| MapInfo { name, max_players, min_players: 2 })
        .filter(|m| {
            let (dir, file) = m.paths();
            src.read(&format!("{dir}{file}")).is_some()
        })
        .collect()
}

/// `mgGameTypeScriptInit` (0x522ec0): the game types of `gametypes.script`.
pub fn game_types(src: &dyn DataSource) -> Vec<GameType> {
    let Some(bytes) = src.read("gametypes.script") else { return Vec::new() };
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    parse_game_types(&text)
}

/// The multiplayer entries of `Tweak.script` (0x8a21f4..0x8a222c) with their compiled
/// defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MpTweaks {
    /// `resourceStartSmall`, `resourceStartMedium`, `resourceStartLarge`.
    pub resource_start: [i32; 3],
    pub ship_death_time_min: f32,
    pub ship_death_time_max: f32,
    pub wait_time_to_quit: f32,
    pub wait_time_to_win: f32,
    /// `TW_BountySizes[0..3]` (small, medium, large; off is 0).
    pub bounty_sizes: [u8; 3],
    /// `TW_Bounty_CurrentShipWorth`, `TW_Bounty_ResourcesInPossesion`: the weights of a
    /// player's fleet value and RUs in its bounty.
    pub bounty_ship_worth: f32,
    pub bounty_resources: f32,
}

impl Default for MpTweaks {
    fn default() -> Self {
        Self { resource_start: [400, 1000, 3000], ship_death_time_min: 5.0, ship_death_time_max: 10.0, wait_time_to_quit: 10.0, wait_time_to_win: 5.0, bounty_sizes: [0; 3], bounty_ship_worth: 0.0, bounty_resources: 0.0 }
    }
}

impl MpTweaks {
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = Self::default();
        let Some(lines) = level::read_lines(src, "Tweak.script") else { return t };
        for (k, v) in lines {
            let f = || crate::statics::cscan::scan_f32(&v);
            let i = || crate::statics::cscan::scan_i32(&v);
            match k.as_str() {
                "resourceStartSmall" => t.resource_start[0] = i().unwrap_or(t.resource_start[0]),
                "resourceStartMedium" => t.resource_start[1] = i().unwrap_or(t.resource_start[1]),
                "resourceStartLarge" => t.resource_start[2] = i().unwrap_or(t.resource_start[2]),
                "PLAYERLOSE_SHIPDEATHTIMEMIN" => t.ship_death_time_min = f().unwrap_or(t.ship_death_time_min),
                "PLAYERLOSE_SHIPDEATHTIMEMAX" => t.ship_death_time_max = f().unwrap_or(t.ship_death_time_max),
                "WAIT_TIME_TO_QUIT" => t.wait_time_to_quit = f().unwrap_or(t.wait_time_to_quit),
                "WAIT_TIME_TO_WIN" => t.wait_time_to_win = f().unwrap_or(t.wait_time_to_win),
                "TW_Bounty_CurrentShipWorth" => t.bounty_ship_worth = f().unwrap_or(t.bounty_ship_worth),
                "TW_Bounty_ResourcesInPossesion" => t.bounty_resources = f().unwrap_or(t.bounty_resources),
                "TW_BountySizes[0]" => t.bounty_sizes[0] = i().map_or(t.bounty_sizes[0], |v| v as u8),
                "TW_BountySizes[1]" => t.bounty_sizes[1] = i().map_or(t.bounty_sizes[1], |v| v as u8),
                "TW_BountySizes[2]" => t.bounty_sizes[2] = i().map_or(t.bounty_sizes[2], |v| v as u8),
                _ => {}
            }
        }
        t
    }
}

/// One player of a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlayerSetup {
    /// Somtaaw or Beast.
    pub race: ShipRace,
    /// Played by the computer (`ComputerPlayerEnabled`).
    pub computer: bool,
}

/// The races of a skirmish (`gameStart` 0x5a3dc0): the local player picks one, and the
/// computer players alternate, starting with the other race.
pub fn skirmish_races(local: ShipRace, num_players: usize) -> Vec<ShipRace> {
    let other = if local == ShipRace::Beast { ShipRace::Somtaaw } else { ShipRace::Beast };
    (0..num_players).map(|i| if i % 2 == 0 { local } else { other }).collect()
}

/// A game to start.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GameSetup {
    /// The map's name in the map list (`Coldwar2`).
    pub map: String,
    /// The players, the local one first; computer players last, as the original seats them.
    pub players: Vec<PlayerSetup>,
    pub options: GameOptions,
    /// The start position of each of the map's spheres (`slots[sphere] = player`). `None`:
    /// shuffled from `seed` as `gameStart` does for a skirmish.
    pub slots: Option<[u8; MAX_PLAYERS]>,
    /// Seeds the random numbers (`ranStartup`'s clock count).
    pub seed: u32,
    /// Attach the effect data (explosions and the like), for a game that is drawn.
    pub effects: bool,
    /// The player at this machine (0 in a skirmish).
    #[serde(default)]
    pub local_player: usize,
    /// A network game (`multiPlayerGame`): every machine plays the same setup.
    #[serde(default)]
    pub network: bool,
}

/// Why a game could not start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkirmishError(pub String);

impl std::fmt::Display for SkirmishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SkirmishError {}

impl From<LevelError> for SkirmishError {
    fn from(e: LevelError) -> Self {
        Self(e.to_string())
    }
}

/// `gameStart`'s start-position shuffle (0x95cb60): the identity, then twenty swaps of two
/// random positions below the map's player count, from random stream 0.
pub fn shuffle_slots(randy: &mut Randy, max_players: usize) -> [u8; MAX_PLAYERS] {
    let mut slots: [u8; MAX_PLAYERS] = std::array::from_fn(|i| i as u8);
    let n = max_players.clamp(1, MAX_PLAYERS) as u32;
    for _ in 0..20 {
        let a = (randy.random(0) % n) as usize;
        let b = (randy.random(0) % n) as usize;
        if a != b {
            slots.swap(a, b);
        }
    }
    slots
}

/// A multiplayer game in progress.
#[derive(Debug, Clone)]
pub struct SkirmishRun {
    pub universe: Universe,
    pub level: LevelState,
    pub setup: GameSetup,
    pub map: MapInfo,
    /// The start positions used.
    pub slots: [u8; MAX_PLAYERS],
    /// What the level load reported.
    pub log: Vec<String>,
}

impl SkirmishRun {
    /// `gameStart` for a multiplayer game: the universe with the players, the level, starting
    /// resources and research. `statics` must have been loaded for multiplayer
    /// (`LoadOptions::single_player` off).
    pub fn start(statics: Arc<StaticWorld>, tweaks: SimTweaks, src: &dyn DataSource, setup: GameSetup) -> Result<Self, SkirmishError> {
        let map = map_list(src)
            .into_iter()
            .find(|m| m.name.eq_ignore_ascii_case(&setup.map))
            .ok_or_else(|| SkirmishError(format!("no multiplayer map {}", setup.map)))?;
        let n = setup.players.len();
        if n < map.min_players || n > map.max_players {
            return Err(SkirmishError(format!("{} takes {}..={} players, not {n}", map.name, map.min_players, map.max_players)));
        }
        if let Some(p) = setup.players.iter().find(|p| !matches!(p.race, ShipRace::Somtaaw | ShipRace::Beast)) {
            return Err(SkirmishError(format!("a player must be Somtaaw or Beast, not {:?}", p.race)));
        }
        let mut randy = Randy::startup(setup.seed);
        let slots = setup.slots.unwrap_or_else(|| shuffle_slots(&mut randy, map.max_players));
        let mpt = MpTweaks::load(src);

        // unitCapInitStatics: the caps for this many players.
        let mut statics = statics;
        if let Some(caps) = statics.unit_cap_presets.get(&(n as u32)).cloned() {
            Arc::make_mut(&mut statics).unit_caps = caps;
        }
        // The players, then the extra one that owns the level's own objects.
        let mut races: Vec<ShipRace> = setup.players.iter().map(|p| p.race).collect();
        races.push(ShipRace::Somtaaw);
        let mut u = Universe::new(statics, tweaks, &races);
        u.single_player = false;
        u.multiplayer_game = true;
        let o = &setup.options;
        u.base.options.flags = o.flags;
        u.base.options.mission = 0;
        u.base.options.injection_interval = o.injection_interval;
        u.base.options.lump_sum_frame = o.lump_sum_time;
        for (i, p) in setup.players.iter().enumerate() {
            let pl = &mut u.base.players[i];
            pl.mothership = None;
            pl.resource_units = 0;
            pl.state = 0;
            pl.auto_launch = 0x1f;
            pl.ai = p.computer;
        }
        // The level's own objects' player is never in the game.
        u.base.players[n].state = 1;
        let local = setup.local_player.min(n.saturating_sub(1));
        u.chatter.local_player = local;
        u.mp = crate::multiplayer::MpState {
            active: true,
            network: setup.network,
            num_players: n,
            local_player: local,
            injection_amount: o.injection_amount,
            lump_sum_amount: o.lump_sum_amount,
            ship_death_time_min: mpt.ship_death_time_min,
            ship_death_time_max: mpt.ship_death_time_max,
            wait_time_to_quit: mpt.wait_time_to_quit,
            wait_time_to_win: mpt.wait_time_to_win,
            deaths: vec![None; n],
            bounties: vec![mpt.bounty_sizes.get(usize::from(o.bounty_size)).copied().unwrap_or(0); n],
            bounty_size: o.bounty_size,
            bounty_base: i32::from(mpt.bounty_sizes.get(usize::from(o.bounty_size)).copied().unwrap_or(0)),
            bounty_ship_worth: mpt.bounty_ship_worth,
            bounty_resources: mpt.bounty_resources,
            crates: crate::crates::Crates { tweaks: crate::crates::CrateTweaks::load(src), ..Default::default() },
            hyperspace: crate::mp_hyperspace::MpHyperspace::load(src),
            ..Default::default()
        };
        // univupdateReset, then ranParametersResetAll (stream 0 goes on).
        randy.reset_game_streams();
        u.randy = randy;

        let mut state = LevelState::default();
        let ctx = LevelContext {
            single_player: false,
            mission: 0,
            fleet_modifier: 0.0,
            resources: ResourceTweaks::load(src),
            mp: Some(MpLevel { slots, num_players: n, flags: o.flags }),
        };
        let (dir, file) = map.paths();
        level::load_level(&mut u, src, &dir, &file, &ctx, &mut state)?;
        // levelInit's tail.
        crate::multiplayer::try_to_find_motherships(&mut u);
        level::preset_ships_to_position(&mut u);
        crate::fow::create_for_local_player(&mut u, local);

        // gameStart: the starting RUs. Computer players get five times as much (2000 RUs
        // without a size) when harvesting is on.
        for (i, p) in setup.players.iter().enumerate() {
            let rich = p.computer && o.flags & mg::HARVESTING_ENABLED != 0;
            let (mult, default) = if rich { (5, 2000) } else { (1, 10) };
            let rus = match o.starting_resources {
                s @ 0..=2 => mpt.resource_start[usize::from(s)] * mult,
                _ => default,
            };
            u.base.players[i].resource_units = rus;
        }
        // Research: everything is known when research is off.
        let mode = if o.flags & mg::RESEARCH_ENABLED == 0 { crate::base::Techs::MODE_ALL } else { -1 };
        for i in 0..n {
            crate::fow::with_vision_refresh(&mut u, mode, |u| u.base.players[i].tech.set_mode(mode));
            crate::upgrade::player_update_from_research(&mut u, i);
        }
        // The computer players (aiplayerStartup, aiplayerInit, aiplayerGameStart).
        u.ai.resource_tweaks = crate::ai::resource::Tweaks::load(src);
        u.ai.bigotry = u32::from(o.cpu_bigotry) + 10;
        let humans = setup.players.iter().filter(|p| !p.computer).count();
        crate::ai::choose_enemies(&mut u, n, humans);
        u.ai.players = vec![None; n];
        let level = crate::ai::level_from_slider(o.cpu_difficulty);
        for (i, p) in setup.players.iter().enumerate() {
            if p.computer {
                u.ai.players[i] = Some(crate::ai::AiPlayer::new(&u, i, level));
                crate::ai::attack::init(&mut u, i);
                crate::ai::defense::init(&mut u, i);
            }
        }
        for (i, p) in setup.players.iter().enumerate() {
            if p.computer {
                crate::ai::game_start(&mut u, i);
            }
        }
        // gameStart's last pass: every ship takes its upgrades.
        for id in u.ship_list.clone() {
            crate::upgrade::apply_upgrades(&mut u, id);
        }
        if setup.effects {
            let data = crate::effects::EffectData::load_from(src);
            if !data.is_empty() {
                crate::effects::attach(&mut u, &data);
            }
        }
        let mut log = state.warnings.clone();
        log.insert(0, format!("{}: {} players, start positions {:?}, {} ships", map.name, n, &slots[..map.max_players.min(MAX_PLAYERS)], u.ship_list.len()));
        Ok(Self { universe: u, level: state, setup, map, slots, log })
    }

    /// One universe update.
    pub fn step(&mut self) {
        self.universe.update();
    }

    /// The players declared winners, a bit each, once the game is decided.
    pub fn winners(&self) -> Option<u32> {
        self.universe.mp.winners
    }

    /// The game is over.
    pub fn over(&self) -> bool {
        self.universe.mp.over
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skirmish_computer_races_alternate() {
        use ShipRace::*;
        assert_eq!(skirmish_races(Somtaaw, 4), vec![Somtaaw, Beast, Somtaaw, Beast]);
        assert_eq!(skirmish_races(Beast, 3), vec![Beast, Somtaaw, Beast]);
    }

    #[test]
    fn the_shuffle_only_moves_positions_the_map_has() {
        let mut r = Randy::startup(1234);
        let s = shuffle_slots(&mut r, 3);
        let mut first: Vec<u8> = s[..3].to_vec();
        first.sort_unstable();
        assert_eq!(first, vec![0, 1, 2]);
        assert_eq!(&s[3..], &[3, 4, 5, 6, 7]);
    }
}
