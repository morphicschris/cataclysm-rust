//! The multiplayer setup files: `gametypes.script` (the game types the skirmish and game
//! screens offer, over `tpGameCreated`) and `multiPlayerMissions.script` (the map list).

/// The options a multiplayer game starts with (`tpGameCreated`, 0x87bf9c..0x87bfb4).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GameOptions {
    /// `flag`, see `hwc_sim::multiplayer::mg`.
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
    /// `aiplayerDifficultyLevel` and `aiplayerBigotry`: the CPU sliders, 0..100 (difficulty
    /// below 34 is easy, above 66 hard).
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
            cpu_difficulty: 50,
            cpu_bigotry: 50,
        }
    }
}

/// A game type of `gametypes.script` (`GAME name { type.* }`): the options it sets. A value
/// the block leaves out keeps what was set before (the original fills the table with 0xff).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameType {
    pub name: String,
    /// `type.flag` bits, see `hwc_sim::multiplayer::mg`.
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
pub fn parse_game_types(text: &str) -> Vec<GameType> {
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

/// The map's player count: `atoi` of the name from its last digit run, as `spTitleListLoad`
/// reads it (it tries every digit and keeps the last).
pub fn trailing_players(name: &str) -> usize {
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

/// The map names of `multiPlayerMissions.script` with their player counts, in file order
/// (`//` and `;` lines are comments; a name without a number is skipped).
pub fn parse_map_list(text: &str) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with("//") {
            continue;
        }
        let n = trailing_players(line);
        if n > 0 {
            out.push((line.to_string(), n));
        }
    }
    out
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
    fn a_type_sets_only_what_it_names() {
        let types = parse_game_types("numGames 2\nGAME A\n{\n type.flag BIT1\n type.bountySize 2\n}\nGAME B\n{\n type.flag BIT3\n type.resourceLumpSumAmount 7\n}\n");
        assert_eq!(types.len(), 2);
        let mut o = types[0].options();
        assert_eq!((o.flags, o.bounty_size), (2, 2));
        types[1].apply(&mut o);
        assert_eq!((o.flags, o.bounty_size, o.lump_sum_amount), (8, 2, 7));
        assert_eq!(game_type_index(&types, &o), 1);
        o.flags |= 1;
        assert_eq!(game_type_index(&types, &o), 2);
    }
}
