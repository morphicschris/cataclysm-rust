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

/// The map's player count: `atoi` of the name from its last digit run, as `spTitleListLoad`
/// reads it (it tries every digit and keeps the last).
fn trailing_players(name: &str) -> usize {
    let b = name.as_bytes();
    let mut n = 0usize;
    for i in 0..b.len() {
        if b[i].is_ascii_digit() {
            let digits: String = name[i..].chars().take_while(char::is_ascii_digit).collect();
            n = digits.parse().unwrap_or(0);
        }
    }
    n
}

/// The maps of `multiPlayerMissions.script` whose level exists, in file order (`//` and `;`
/// lines are comments). The original then adds the directories under `MultiPlayer\` that
/// are not listed and sorts the list by title; the shipped data lists every map.
pub fn map_list(src: &dyn DataSource) -> Vec<MapInfo> {
    let Some(bytes) = src.read("multiPlayerMissions.script") else { return Vec::new() };
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
            continue;
        }
        let max_players = trailing_players(line);
        if max_players == 0 {
            continue;
        }
        let m = MapInfo { name: line.to_string(), max_players, min_players: 2 };
        let (dir, file) = m.paths();
        if src.read(&format!("{dir}{file}")).is_some() {
            out.push(m);
        }
    }
    out
}

/// The options a multiplayer game starts with (`tpGameCreated`, 0x87bf9c..0x87bfb4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameOptions {
    /// `flag`, see [`mg`].
    pub flags: u32,
    /// Computer players (the screens' CPU count; [`GameSetup::players`] says who they are).
    pub num_computers: u8,
    /// 0 low, 1 medium, 2 high (kept for the screens; the game does not use it).
    pub starting_fleet: u8,
    /// 0..2; 3 is off.
    pub bounty_size: u8,
    /// 0 small, 1 medium, 2 large (`resourceStart*`); anything else gives 10 RUs.
    pub starting_resources: u8,
    /// In universe updates (16 a second).
    pub injection_interval: u32,
    pub injection_amount: i32,
    pub lump_sum_time: u32,
    pub lump_sum_amount: i32,
    /// `aiplayerDifficultyLevel` and `aiplayerBigotry` (the CPU sliders).
    pub cpu_difficulty: u8,
    pub cpu_bigotry: u8,
}

impl Default for GameOptions {
    /// Before any game type is picked (the values `gametypes.script` documents).
    fn default() -> Self {
        Self {
            flags: 0,
            num_computers: 0,
            starting_fleet: 1,
            bounty_size: 1,
            starting_resources: 2,
            injection_interval: 9600,
            injection_amount: 1500,
            lump_sum_time: 19200,
            lump_sum_amount: 2000,
            cpu_difficulty: 1,
            cpu_bigotry: 1,
        }
    }
}

/// A game type of `gametypes.script` (`GAME name { type.* }`): the options it sets. A value
/// the block leaves out keeps what was set before (the original fills the table with 0xff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameType {
    pub name: String,
    /// `type.flag` bits, see [`mg`].
    pub flags: u16,
    /// `type.flagNeeded`: the bits the type sets (all when the block names none).
    pub flag_needed: u16,
    pub num_computers: Option<u8>,
    pub starting_fleet: Option<u8>,
    pub bounty_size: Option<u8>,
    pub starting_resources: Option<u8>,
    pub injection_interval: Option<u32>,
    pub injection_amount: Option<i32>,
    pub lump_sum_time: Option<u32>,
    pub lump_sum_amount: Option<i32>,
}

impl GameType {
    fn named(name: &str) -> Self {
        Self {
            name: name.to_string(),
            flags: 0,
            flag_needed: 0xffff,
            num_computers: None,
            starting_fleet: None,
            bounty_size: None,
            starting_resources: None,
            injection_interval: None,
            injection_amount: None,
            lump_sum_time: None,
            lump_sum_amount: None,
        }
    }

    /// `mgSetGameTypeByStruct` (0x4bd7c0): the type's bits and values over the options (the
    /// computer count stays).
    pub fn apply(&self, o: &mut GameOptions) {
        let f = (o.flags as u16 & !self.flag_needed) | (self.flags & self.flag_needed);
        o.flags = (o.flags & !0xffff) | u32::from(f);
        if let Some(v) = self.starting_fleet {
            o.starting_fleet = v;
        }
        if let Some(v) = self.bounty_size {
            o.bounty_size = v;
        }
        if let Some(v) = self.starting_resources {
            o.starting_resources = v;
        }
        if let Some(v) = self.injection_interval {
            o.injection_interval = v;
        }
        if let Some(v) = self.injection_amount {
            o.injection_amount = v;
        }
        if let Some(v) = self.lump_sum_time {
            o.lump_sum_time = v;
        }
        if let Some(v) = self.lump_sum_amount {
            o.lump_sum_amount = v;
        }
    }

    /// `mgGameTypesOtherButtonPressed`'s test: the options are this type's.
    pub fn matches(&self, o: &GameOptions) -> bool {
        (o.flags as u16 & self.flag_needed) == self.flags
            && self.starting_fleet.is_none_or(|v| v == o.starting_fleet)
            && self.bounty_size.is_none_or(|v| v == o.bounty_size)
            && self.starting_resources.is_none_or(|v| v == o.starting_resources)
            && self.injection_interval.is_none_or(|v| v == o.injection_interval)
            && self.injection_amount.is_none_or(|v| v == o.injection_amount)
            && self.lump_sum_time.is_none_or(|v| v == o.lump_sum_time)
            && self.lump_sum_amount.is_none_or(|v| v == o.lump_sum_amount)
    }

    /// The options of this type from the defaults.
    pub fn options(&self) -> GameOptions {
        let mut o = GameOptions::default();
        self.apply(&mut o);
        o
    }
}

/// The radio button of `MG_GameType` the options select: the first type they match, or the
/// last ("custom") button.
pub fn game_type_index(types: &[GameType], o: &GameOptions) -> usize {
    types.iter().position(|g| g.matches(o)).unwrap_or(types.len())
}

/// `mgGameTypeScriptInit` (0x522ec0): the game types of `gametypes.script`. `type.flag BITn`
/// sets bit n.
pub fn game_types(src: &dyn DataSource) -> Vec<GameType> {
    let Some(bytes) = src.read("gametypes.script") else { return Vec::new() };
    let text: String = bytes.iter().map(|&b| char::from(b)).collect();
    let mut out: Vec<GameType> = Vec::new();
    let mut cur: Option<GameType> = None;
    for raw in text.lines() {
        let line = raw.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let mut it = line.splitn(2, char::is_whitespace);
        let key = it.next().unwrap_or("");
        let value = it.next().unwrap_or("").trim();
        if key == "GAME" {
            if let Some(g) = cur.take() {
                out.push(g);
            }
            cur = Some(GameType::named(value));
            continue;
        }
        if key.starts_with('}') {
            if let Some(g) = cur.take() {
                out.push(g);
            }
            continue;
        }
        let Some(g) = cur.as_mut() else { continue };
        let num = || value.split_whitespace().next().and_then(|v| v.parse::<i64>().ok()).unwrap_or(0);
        let bit = || value.strip_prefix("BIT").and_then(|b| b.split_whitespace().next()).and_then(|b| b.parse::<u32>().ok());
        match key {
            "type.flag" => {
                if let Some(b) = bit() {
                    g.flags |= 1 << b;
                }
            }
            "type.flagNeeded" => {
                if let Some(b) = bit() {
                    // The table starts at 0xffff, so a named bit leaves it as it is.
                    g.flag_needed |= 1 << b;
                }
            }
            "type.numComputers" => g.num_computers = Some(num() as u8),
            "type.startingFleet" => g.starting_fleet = Some(num() as u8),
            "type.bountySize" => g.bounty_size = Some(num() as u8),
            "type.startingResources" => g.starting_resources = Some(num() as u8),
            "type.resourceInjectionInterval" => g.injection_interval = Some(num() as u32),
            "type.resourceInjectionsAmount" => g.injection_amount = Some(num() as i32),
            "type.resourceLumpSumTime" => g.lump_sum_time = Some(num() as u32),
            "type.resourceLumpSumAmount" => g.lump_sum_amount = Some(num() as i32),
            _ => {}
        }
    }
    if let Some(g) = cur.take() {
        out.push(g);
    }
    out
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
}

impl Default for MpTweaks {
    fn default() -> Self {
        Self { resource_start: [400, 1000, 3000], ship_death_time_min: 5.0, ship_death_time_max: 10.0, wait_time_to_quit: 10.0, wait_time_to_win: 5.0, bounty_sizes: [0; 3] }
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, PartialEq)]
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
        u.chatter.local_player = 0;
        u.mp = crate::multiplayer::MpState {
            active: true,
            network: false,
            num_players: n,
            local_player: 0,
            injection_amount: o.injection_amount,
            lump_sum_amount: o.lump_sum_amount,
            ship_death_time_min: mpt.ship_death_time_min,
            ship_death_time_max: mpt.ship_death_time_max,
            wait_time_to_quit: mpt.wait_time_to_quit,
            wait_time_to_win: mpt.wait_time_to_win,
            deaths: vec![None; n],
            bounties: vec![mpt.bounty_sizes.get(usize::from(o.bounty_size)).copied().unwrap_or(0); n],
            crates: crate::crates::Crates { tweaks: crate::crates::CrateTweaks::load(src), ..Default::default() },
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
        crate::fow::create_for_local_player(&mut u, 0);

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
    fn the_player_count_is_the_last_number_of_the_name() {
        assert_eq!(trailing_players("Coldwar2"), 2);
        assert_eq!(trailing_players("KiithKnox6"), 6);
        assert_eq!(trailing_players("ArmedDeux8"), 8);
        assert_eq!(trailing_players("Nothing"), 0);
    }

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
