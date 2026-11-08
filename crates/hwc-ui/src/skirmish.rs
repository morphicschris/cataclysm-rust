//! The skirmish screens of `multiplayer_game.fib` (`MultiplayerGame.c` and `ScenPick.c` in the
//! original): Connection Method's Skirmish button, the Basic / Advanced / Resource / Player
//! Setup tabs over `tpGameCreated`, the scenario list, and Start Skirmish.
//!
//! The game types and map list come from the game files through
//! [`hwc_formats::multiplayer`]; what the player sets goes to the game in
//! [`Request::StartSkirmish`](crate::cataclysm::Request::StartSkirmish).

use hwc_formats::multiplayer::{
    game_type_index, parse_game_types, parse_map_list, GameOptions, GameType,
};

use crate::canvas::{Color, Rect};
use crate::cataclysm::{Cataclysm, Request};
use crate::consts::colour;
use crate::consts::{
    CM_ACCEPT_TEXT, CM_DOUBLE_CLICK, CM_KEY_PRESSED, RPE_ENTER, RPE_EXIT, RPM_PRESS_RELEASE,
    RSF_LEFT_PRESSED, RSF_MOUSE_INSIDE,
};
use crate::fe::{Call, FrontEnd};
use crate::fer::Fer;
use crate::region::{AtomRef, RegionId};
use crate::text::{self, Shadow};

type Fe = FrontEnd<Cataclysm>;

/// `MG_*` flag bits the check boxes set (Cataclysm's numbering).
mod bit {
    pub const LAST_MOTHERSHIP: u32 = 0x0002;
    pub const RESEARCH: u32 = 0x0008;
    pub const ALLOW_ALLIES: u32 = 0x0010;
    pub const HARVESTING: u32 = 0x0020;
    pub const INJECTIONS: u32 = 0x0040;
    pub const LUMP_SUM: u32 = 0x0080;
    pub const ALLIED_VICTORY: u32 = 0x0800;
    pub const CRATES: u32 = 0x1000;
    pub const HYPERSPACE: u32 = 0x4000;
}

/// The check boxes and their bits. "Disable harvesting" (`MG_HarvestingEnabled`) is ticked
/// when the bit is clear.
const CHECKS: [(&str, u32); 9] = [
    ("MG_LastMotherShip", bit::LAST_MOTHERSHIP),
    ("MG_AlliedVictory", bit::ALLIED_VICTORY),
    ("MG_AllowAllies", bit::ALLOW_ALLIES),
    ("MG_ResearchEnabled", bit::RESEARCH),
    ("MG_CratesEnabled", bit::CRATES),
    ("MG_Hyperspace", bit::HYPERSPACE),
    ("MG_HarvestingEnabled", bit::HARVESTING),
    ("MG_ResourceInjections", bit::INJECTIONS),
    ("MG_ResourceLumpSum", bit::LUMP_SUM),
];

/// One map of the scenario list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    /// Directory and level name (`Coldwar2`).
    pub name: String,
    /// The level's `[title]` line (`spTitleFind`), or the name.
    pub title: String,
    pub max_players: usize,
    /// `description.txt`'s `Description` lines.
    pub description: Vec<String>,
    /// The game type the description sets (`type.*` lines; none in the shipped maps).
    pub settings: Option<GameType>,
}

/// The skirmish screens' state (`tpGameCreated`, the picked scenario and race).
#[derive(Debug, Clone, Default)]
pub struct SkirmishUi {
    pub loaded: bool,
    pub types: Vec<GameType>,
    pub scenarios: Vec<Scenario>,
    /// The picked scenario (`spCurrentSelected`).
    pub scenario: usize,
    /// The row of the scenario list while it is open.
    pub picking: Option<usize>,
    pub options: GameOptions,
    /// The game type button (`types.len()` is "custom").
    pub type_index: usize,
    /// The player's name (`utyName`).
    pub name: String,
    /// 0 Somtaaw, 1 Beast (`whichRaceSelected`).
    pub race: u8,
    /// The add / remove arrows were just pressed (drawn red once).
    add_pressed: bool,
    remove_pressed: bool,
}

/// What the game needs to start a skirmish.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkirmishRequest {
    pub map: String,
    pub options: GameOptions,
    /// The local player's race: 0 Somtaaw, 1 Beast.
    pub race: u8,
    pub name: String,
}

impl SkirmishUi {
    /// `spTitleListLoad` and `mgGameTypeScriptInit`, once.
    pub fn load(&mut self, read: &dyn Fn(&str) -> Option<Vec<u8>>) {
        if self.loaded {
            return;
        }
        self.loaded = true;
        let latin = |b: Vec<u8>| -> String { b.iter().map(|&c| char::from(c)).collect() };
        if let Some(t) = read("gametypes.script").map(latin) {
            self.types = parse_game_types(&t);
        }
        if let Some(t) = read("multiPlayerMissions.script").map(latin) {
            for (name, max_players) in parse_map_list(&t) {
                let dir = format!("MultiPlayer\\{name}\\");
                let Some(level) = read(&format!("{dir}{name}.level")).map(latin) else {
                    continue;
                };
                let title = level_title(&level).unwrap_or_else(|| name.clone());
                let desc = read(&format!("{dir}description.txt"))
                    .map(latin)
                    .unwrap_or_default();
                let (description, settings) = parse_description(&desc);
                self.scenarios.push(Scenario {
                    name,
                    title,
                    max_players,
                    description,
                    settings,
                });
            }
        }
        // compareScenariosCB: by title.
        self.scenarios.sort_by(|a, b| {
            a.title
                .to_ascii_lowercase()
                .cmp(&b.title.to_ascii_lowercase())
        });
        if let Some(g) = self.types.first() {
            g.apply(&mut self.options);
            self.type_index = 0;
        }
    }

    pub fn max_players(&self) -> usize {
        self.scenarios
            .get(self.scenario)
            .map_or(2, |s| s.max_players)
    }

    /// The request for the picked game.
    pub fn request(&self) -> Option<SkirmishRequest> {
        let s = self.scenarios.get(self.scenario)?;
        Some(SkirmishRequest {
            map: s.name.clone(),
            options: self.options.clone(),
            race: self.race,
            name: self.name.clone(),
        })
    }

    /// `mgGameTypesOtherButtonPressed`: the type button the options now match.
    fn retype(&mut self) {
        self.type_index = game_type_index(&self.types, &self.options);
    }
}

/// `spTitleFind`: the first `[...]` line of a level.
fn level_title(level: &str) -> Option<String> {
    level
        .lines()
        .map(|l| l.trim_end_matches(['\r', '\n']))
        .find(|l| l.len() > 2 && l.starts_with('[') && l.ends_with(']'))
        .map(|l| l[1..l.len() - 1].to_string())
}

/// A map's `description.txt`: the English `Description` lines, and the `type.*` settings it
/// gives (with only the bits it names).
fn parse_description(text: &str) -> (Vec<String>, Option<GameType>) {
    let mut lines = Vec::new();
    let mut settings: Option<GameType> = None;
    for raw in text.lines() {
        let l = raw.trim_end_matches('\r');
        if l.starts_with(';') || l.starts_with("//") {
            continue;
        }
        let mut it = l.splitn(2, char::is_whitespace);
        let key = it.next().unwrap_or("");
        let value = it.next().unwrap_or("").trim();
        if key == "Description" {
            lines.push(value.to_string());
        } else if key.starts_with("type.") {
            let g = settings.get_or_insert_with(|| {
                let mut g = parse_game_types("GAME map\n{\n}\n")
                    .pop()
                    .expect("one type");
                g.flag_needed = 0;
                g
            });
            let one = parse_game_types(&format!("GAME x\n{{\n{key} {value}\n}}\n")).pop();
            if let Some(o) = one {
                if key == "type.flag" {
                    g.flags |= o.flags;
                } else if key == "type.flagNeeded" {
                    if let Some(b) = value
                        .strip_prefix("BIT")
                        .and_then(|b| b.split_whitespace().next())
                        .and_then(|b| b.parse::<u32>().ok())
                    {
                        g.flag_needed |= 1 << b;
                    }
                }
                g.starting_fleet = o.starting_fleet.or(g.starting_fleet);
                g.bounty_size = o.bounty_size.or(g.bounty_size);
                g.starting_resources = o.starting_resources.or(g.starting_resources);
                g.injection_interval = o.injection_interval.or(g.injection_interval);
                g.injection_amount = o.injection_amount.or(g.injection_amount);
                g.lump_sum_time = o.lump_sum_time.or(g.lump_sum_time);
                g.lump_sum_amount = o.lump_sum_amount.or(g.lump_sum_amount);
            }
        }
    }
    (lines, settings)
}

/// Registers the skirmish callbacks.
pub fn register(fe: &mut Fe) {
    fe.callback_add("MG_Skirmish", skirmish);
    fe.callback_add("MG_BackFromOptions", |fe, _| {
        fe.all_screens_delete();
        // Cancel on a LAN game's option tabs goes back to the game list.
        if std::mem::take(&mut fe.game.lan.creating) {
            fe.screen_start("Available_Games");
        } else {
            fe.screen_start("Connection_Method");
        }
    });
    fe.callback_add("MG_GameType", game_type);
    for (name, _) in CHECKS {
        fe.callback_add(name, check);
    }
    fe.callback_add("MG_BountySize", |fe, call| {
        radio(fe, call, |o| &mut o.bounty_size)
    });
    fe.callback_add("MG_StartingResources", |fe, call| {
        radio(fe, call, |o| &mut o.starting_resources)
    });
    fe.callback_add("MG_ResourceInjectionInterval", |fe, call| {
        number(fe, call, Field::InjectionInterval)
    });
    fe.callback_add("MG_ResourceInjectionAmount", |fe, call| {
        number(fe, call, Field::InjectionAmount)
    });
    fe.callback_add("MG_ResourceLumpSumInterval", |fe, call| {
        number(fe, call, Field::LumpSumTime)
    });
    fe.callback_add("MG_ResourceLumpSumAmount", |fe, call| {
        number(fe, call, Field::LumpSumAmount)
    });
    fe.callback_add("MG_NameEntry", name_entry);
    fe.callback_add("OP_CPU_Difficulty", |fe, call| cpu_slider(fe, call, false));
    fe.callback_add("OP_CPU_Attacks", |fe, call| cpu_slider(fe, call, true));
    fe.slider_handler_add("mgCpuSlider", cpu_slider_moved);
    fe.callback_add("MG_ChooseScenario", choose_scenario);
    fe.callback_add("CS_ScenarioWindowInit", scenario_window);
    fe.callback_add("CS_ScenarioBitmap", scenario_description);
    fe.callback_add("CS_Done", |fe, _| done_picking(fe));
    fe.callback_add("CS_Back", |fe, _| {
        fe.game.skirmish.picking = None;
        fe.current_screen_delete();
    });
    fe.callback_add("StartNewGame", start);
    fe.callback_add("CP_RaceOne", race);
    fe.callback_add("CP_RaceTwo", race);
    fe.draw_callback_add("CS_ScenarioName", draw_scenario_name);
    fe.list_drawer_add("CS_ScenarioBitmap", draw_description_line);
    fe.draw_callback_add("MG_NumberOfComputers", draw_computers);
    fe.draw_callback_add("MG_AddCPUOpponent", |fe, fer, a, r| {
        draw_arrow(fe, fer, a, r, false)
    });
    fe.draw_callback_add("MG_RemoveCPUOpponent", |fe, fer, a, r| {
        draw_arrow(fe, fer, a, r, true)
    });
    fe.region_function_add("mgAddCPU", |fe, id, event| cpu_arrow(fe, id, event, true));
    fe.region_function_add("mgRemoveCPU", |fe, id, event| {
        cpu_arrow(fe, id, event, false)
    });
}

fn load(fe: &mut Fe) {
    let res = fe.resources().clone();
    fe.game.skirmish.load(&|p| res.read(p));
}

/// `mgSkirmish`: one CPU opponent, the Basic tab.
fn skirmish(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    load(fe);
    let s = &mut fe.game.skirmish;
    s.options.num_computers = 1;
    if s.name.is_empty() {
        s.name = fe.game.options.get_str("PlayerName").to_string();
    }
    s.race = fe.game.options.get("PlayerRace").clamp(0, 1) as u8;
    fe.all_screens_delete();
    fe.screen_start("Skirmish_Basic");
}

/// Re-reads every skirmish control on the current screens (`feAllCallOnCreate` after a game
/// type is picked).
fn sync(fe: &mut Fe) {
    let o = fe.game.skirmish.options.clone();
    for (name, b) in CHECKS {
        if fe.find_atom(name).is_some() {
            fe.toggle_set(name, ticked(o.flags, name, b));
        }
    }
    if fe.find_atom("MG_GameType").is_some() {
        fe.radio_set("MG_GameType", fe.game.skirmish.type_index as u32);
    }
    if fe.find_atom("MG_BountySize").is_some() {
        fe.radio_set("MG_BountySize", u32::from(o.bounty_size));
    }
    if fe.find_atom("MG_StartingResources").is_some() {
        fe.radio_set("MG_StartingResources", u32::from(o.starting_resources));
    }
    for f in [
        Field::InjectionInterval,
        Field::InjectionAmount,
        Field::LumpSumTime,
        Field::LumpSumAmount,
    ] {
        if let Some(a) = fe.find_atom(f.atom()) {
            let (v, len) = f.get(&o);
            fe.text_entry_set(a, &v.to_string(), len);
        }
    }
}

/// `mgGameType`.
fn game_type(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.radio_set(&call.name, fe.game.skirmish.type_index as u32);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let v = fe.atom(a).radio_value as usize;
    let s = &mut fe.game.skirmish;
    s.type_index = v;
    if let Some(g) = s.types.get(v).cloned() {
        g.apply(&mut s.options);
    }
    sync(fe);
}

fn ticked(flags: u32, name: &str, b: u32) -> bool {
    (flags & b != 0) != (name == "MG_HarvestingEnabled")
}

/// `mgLastMotherShip` and the other flag check boxes.
fn check(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    let Some(&(_, b)) = CHECKS.iter().find(|(n, _)| *n == call.name) else {
        return;
    };
    if fe.first_call(call) {
        fe.toggle_set(
            &call.name,
            ticked(fe.game.skirmish.options.flags, &call.name, b),
        );
    } else if fe.last_call(call) {
        return;
    } else {
        let on = fe.atom(a).checked() != (call.name == "MG_HarvestingEnabled");
        let o = &mut fe.game.skirmish.options;
        o.flags = if on { o.flags | b } else { o.flags & !b };
    }
    fe.game.skirmish.retype();
    if fe.find_atom("MG_GameType").is_some() {
        fe.radio_set("MG_GameType", fe.game.skirmish.type_index as u32);
    }
}

/// `mgBountySize`, `mgStartingResources`.
fn radio(fe: &mut Fe, call: &Call, field: fn(&mut GameOptions) -> &mut u8) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = *field(&mut fe.game.skirmish.options);
        fe.radio_set(&call.name, u32::from(v));
    } else if fe.last_call(call) {
        return;
    } else {
        *field(&mut fe.game.skirmish.options) = fe.atom(a).radio_value as u8;
    }
    fe.game.skirmish.retype();
    if fe.find_atom("MG_GameType").is_some() {
        fe.radio_set("MG_GameType", fe.game.skirmish.type_index as u32);
    }
}

/// The resource text boxes.
#[derive(Clone, Copy)]
enum Field {
    InjectionInterval,
    InjectionAmount,
    LumpSumTime,
    LumpSumAmount,
}

impl Field {
    fn atom(self) -> &'static str {
        match self {
            Field::InjectionInterval => "MG_ResourceInjectionInterval",
            Field::InjectionAmount => "MG_ResourceInjectionAmount",
            Field::LumpSumTime => "MG_ResourceLumpSumInterval",
            Field::LumpSumAmount => "MG_ResourceLumpSumAmount",
        }
    }

    /// The shown value (times in minutes: 960 updates) and the box's length.
    fn get(self, o: &GameOptions) -> (i64, usize) {
        match self {
            Field::InjectionInterval => (i64::from(o.injection_interval / 960), 10),
            Field::InjectionAmount => (i64::from(o.injection_amount), 6),
            Field::LumpSumTime => (i64::from(o.lump_sum_time / 960), 10),
            Field::LumpSumAmount => (i64::from(o.lump_sum_amount), 6),
        }
    }

    fn set(self, o: &mut GameOptions, v: i64) {
        match self {
            Field::InjectionInterval => o.injection_interval = (v.max(0) as u32).wrapping_mul(960),
            Field::InjectionAmount => o.injection_amount = v as i32,
            Field::LumpSumTime => o.lump_sum_time = (v.max(0) as u32).wrapping_mul(960),
            Field::LumpSumAmount => o.lump_sum_amount = v as i32,
        }
    }

    /// Accepting the box turns its option on.
    fn flag(self) -> u32 {
        match self {
            Field::InjectionInterval | Field::InjectionAmount => bit::INJECTIONS,
            Field::LumpSumTime | Field::LumpSumAmount => bit::LUMP_SUM,
        }
    }
}

/// `mgResourceInjectionInterval` and the other number boxes: the value goes in when the box
/// loses focus or is accepted; accepting also ticks its option.
fn number(fe: &mut Fe, call: &Call, f: Field) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let (v, len) = f.get(&fe.game.skirmish.options);
        fe.text_entry_set(a, &v.to_string(), len);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let msg = fe.text_message(a);
    if (2..4).contains(&msg) {
        let v = fe
            .text_entry(a)
            .and_then(|t| t.trim().parse::<i64>().ok())
            .unwrap_or(0);
        f.set(&mut fe.game.skirmish.options, v);
        if msg == CM_ACCEPT_TEXT {
            fe.game.skirmish.options.flags |= f.flag();
            let name = match f.flag() {
                bit::INJECTIONS => "MG_ResourceInjections",
                _ => "MG_ResourceLumpSum",
            };
            fe.toggle_set(name, true);
        }
    }
    fe.game.skirmish.retype();
    if fe.find_atom("MG_GameType").is_some() {
        fe.radio_set("MG_GameType", fe.game.skirmish.type_index as u32);
    }
}

/// `mgNameEntry`: up to 18 characters; control characters and quotes are refused.
fn name_entry(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let clean: String = fe
            .game
            .skirmish
            .name
            .chars()
            .filter(|&c| !(c.is_control() || c == '"' || c == '\''))
            .collect();
        fe.game.skirmish.name = clean.clone();
        fe.text_entry_set(a, &clean, 0x12);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    match fe.text_message(a) {
        2 | 3 => fe.game.skirmish.name = fe.text_entry(a).unwrap_or_default().to_string(),
        CM_KEY_PRESSED => {
            let last = fe.text_entry(a).and_then(|t| t.chars().last());
            if last.is_some_and(|c| c.is_control() || c == '"' || c == '\'') {
                fe.text_entry_backspace(a);
            }
        }
        _ => {}
    }
}

/// `opCPUDifficulty` / `opCPUAttacks`: sliders 0..100.
fn cpu_slider(fe: &mut Fe, call: &Call, attacks: bool) {
    let Some(a) = call.atom else { return };
    if !fe.first_call(call) {
        return;
    }
    let o = &fe.game.skirmish.options;
    let v = if attacks {
        o.cpu_bigotry
    } else {
        o.cpu_difficulty
    };
    fe.slider_setup(a, 101, u32::from(v), Some("mgCpuSlider"));
}

fn cpu_slider_moved(fe: &mut Fe, region: RegionId) {
    let Some(a) = fe.region(region).and_then(|r| r.atom) else {
        return;
    };
    let v = fe.slider(a).map_or(0, |s| s.value).min(255) as u8;
    let attacks = fe.atom(a).name.as_deref() == Some("OP_CPU_Attacks");
    let o = &mut fe.game.skirmish.options;
    if attacks {
        o.cpu_bigotry = v;
    } else {
        o.cpu_difficulty = v;
    }
}

/// `mgAddCPUProcess` / `mgRemoveCPUProcess`: one more or one fewer CPU player, within the
/// map's players and never below one.
fn cpu_arrow(fe: &mut Fe, _id: RegionId, event: u32, add: bool) {
    if event != crate::consts::RPE_PRESS_LEFT {
        return;
    }
    let max = fe.game.skirmish.max_players();
    let s = &mut fe.game.skirmish;
    let n = usize::from(s.options.num_computers);
    if add {
        if n + 1 < max {
            s.options.num_computers += 1;
        }
        s.add_pressed = true;
    } else {
        if n > 1 {
            s.options.num_computers -= 1;
        }
        s.remove_pressed = true;
    }
    fe.ui_sound(crate::consts::sound::CLICK);
}

/// `mgNumberOfComputers`.
fn draw_computers(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, r: Rect) {
    fer.canvas.fill_rect(r, [0x5a, 0x63, 0x7a, 255]);
    let Some(font) = fer.res.font("HW_EuroseCond_11.hff") else {
        return;
    };
    let t = fe.game.skirmish.options.num_computers.to_string();
    text::print(
        fer.canvas,
        &font,
        r.x0,
        r.y0,
        [255, 255, 255, 255],
        t.as_bytes(),
        (Shadow::None, [0, 0, 0, 255]),
    );
}

/// `mgDrawArrow`: a triangle pointing right (add) or left (remove), white under the mouse,
/// red just after a press. The region is made clickable on first draw (`regFunctionSet`).
fn draw_arrow(fe: &mut Fe, fer: &mut Fer, a: AtomRef, r: Rect, left: bool) {
    let (atom, function) = if left {
        ("MG_RemoveCPUOpponent", "mgRemoveCPU")
    } else {
        ("MG_AddCPUOpponent", "mgAddCPU")
    };
    fe.region_function_set(atom, function, RPM_PRESS_RELEASE | RPE_ENTER | RPE_EXIT);
    let status = fe
        .atom(a)
        .region
        .and_then(|id| fe.region(id))
        .map_or(0, |reg| reg.status);
    let s = &mut fe.game.skirmish;
    let pressed = if left {
        std::mem::take(&mut s.remove_pressed)
    } else {
        std::mem::take(&mut s.add_pressed)
    };
    let fill: Color = if pressed || status & RSF_LEFT_PRESSED != 0 {
        [0xc8, 0, 0, 255]
    } else if status & RSF_MOUSE_INSIDE != 0 {
        [255, 255, 255, 255]
    } else {
        [0x5a, 0x63, 0x7a, 255]
    };
    let w = r.x1 - r.x0 - 4;
    let top = r.y0 + 4;
    let (tip, base) = if left {
        (r.x0 + 2, r.x1 - 2)
    } else {
        (r.x1 - 2, r.x0 + 2)
    };
    let pts = [(tip, top + w / 2), (base, top), (base, top + w)];
    fill_triangle(fer, pts, fill);
    for i in 0..3 {
        let (p, q) = (pts[i], pts[(i + 1) % 3]);
        fer.canvas.line(p.0, p.1, q.0, q.1, [255, 255, 255, 255]);
    }
}

fn fill_triangle(fer: &mut Fer, p: [(i32, i32); 3], c: Color) {
    let (y0, y1) = (
        p.iter().map(|q| q.1).min().unwrap_or(0),
        p.iter().map(|q| q.1).max().unwrap_or(0),
    );
    for y in y0..=y1 {
        let mut xs = Vec::new();
        for i in 0..3 {
            let (a, b) = (p[i], p[(i + 1) % 3]);
            if (a.1 <= y && y < b.1) || (b.1 <= y && y < a.1) {
                xs.push(a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1));
            }
        }
        if let (Some(&l), Some(&r)) = (xs.iter().min(), xs.iter().max()) {
            fer.canvas.fill_rect(
                Rect {
                    x0: l,
                    y0: y,
                    x1: r + 1,
                    y1: y + 1,
                },
                c,
            );
        }
    }
}

/// `spScenarioNameDraw`: the picked scenario's title, upper case.
fn draw_scenario_name(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, r: Rect) {
    let Some(font) = fer.res.font("HW_EuroseCond_11.hff") else {
        return;
    };
    let title = fe
        .game
        .skirmish
        .scenarios
        .get(fe.game.skirmish.scenario)
        .map(|s| s.title.to_ascii_uppercase())
        .unwrap_or_default();
    let t: String = title.chars().take(0x1f).collect();
    text::print(
        fer.canvas,
        &font,
        r.x0,
        r.y0,
        colour::LIST_ITEM_SELECTED,
        &text::to_latin1(&t),
        (Shadow::None, [0, 0, 0, 255]),
    );
}

/// `mgChooseScenario` → `spScenarioPick`.
fn choose_scenario(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    load(fe);
    fe.game.skirmish.picking = Some(fe.game.skirmish.scenario);
    fe.screen_start("Load_a_scenario");
}

/// `spScenarioWindowInit`: the titles, the picked one selected; a new selection shows its
/// description; a double click or Return picks it.
fn scenario_window(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let titles: Vec<String> = fe
            .game
            .skirmish
            .scenarios
            .iter()
            .map(|s| s.title.clone())
            .collect();
        let cur = fe.game.skirmish.scenario;
        fe.list_init(a, "HW_EuroseCond_11.hff", 18);
        if let Some(l) = fe.list_mut(a) {
            l.items.clear();
            l.selected = None;
            l.upper = 0;
            for (i, t) in titles.iter().enumerate() {
                l.add(t.chars().take(0x1f).collect::<String>(), i as u32, i == cur);
            }
        }
        if let Some(id) = fe.atom(a).region {
            fe.list_scroll_adjust(id);
        }
        fill_description(fe);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let sel = fe.list(a).and_then(|l| {
        l.selected
            .and_then(|i| l.items.get(i))
            .map(|it| it.data as usize)
    });
    let msg = fe.list(a).map_or(0, |l| l.message);
    if let Some(i) = sel {
        if fe.game.skirmish.picking != Some(i) {
            fe.game.skirmish.picking = Some(i);
            fill_description(fe);
        }
    }
    if msg == CM_ACCEPT_TEXT || msg == CM_DOUBLE_CLICK {
        done_picking(fe);
    }
}

/// `spScenarioBitmap`: the description list.
fn scenario_description(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) {
        fill_description(fe);
    }
}

/// `spNewItem`: the picked row's description, wrapped to the list.
fn fill_description(fe: &mut Fe) {
    let Some(a) = fe.find_atom("CS_ScenarioBitmap") else {
        return;
    };
    let i = fe
        .game
        .skirmish
        .picking
        .unwrap_or(fe.game.skirmish.scenario);
    let lines = fe
        .game
        .skirmish
        .scenarios
        .get(i)
        .map(|s| s.description.clone())
        .unwrap_or_default();
    let width = fe.atom(a).rect().width() - 18;
    let font = fe
        .resources()
        .font("Arial_b12.hff")
        .or_else(|| fe.resources().font("HW_EuroseCond_11.hff"));
    let mut rows = Vec::new();
    for l in lines {
        if l.is_empty() {
            rows.push(String::new());
            continue;
        }
        let mut cur = String::new();
        for w in l.split_whitespace() {
            let next = if cur.is_empty() {
                w.to_string()
            } else {
                format!("{cur} {w}")
            };
            let fits = font
                .as_ref()
                .is_none_or(|f| text::width(f, &text::to_latin1(&next)) <= width);
            if fits || cur.is_empty() {
                cur = next;
            } else {
                rows.push(std::mem::take(&mut cur));
                cur = w.to_string();
            }
        }
        rows.push(cur);
    }
    fe.list_init(a, "Arial_b12.hff", 14);
    if let Some(l) = fe.list_mut(a) {
        l.items.clear();
        l.selected = None;
        l.upper = 0;
        for (i, r) in rows.into_iter().enumerate() {
            l.add(r, i as u32, false);
        }
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// `spDescriptionItemDraw`: a description line in the description's colour (`Color 255, 200,
/// 0` and `DropShadow Yes` in every shipped `description.txt`), 6 pixels in.
fn draw_description_line(_fe: &Fe, fer: &mut Fer, r: Rect, item: &crate::controls::ListItem) {
    let Some(font) = fer
        .res
        .font("Arial_b12.hff")
        .or_else(|| fer.res.font("HW_EuroseCond_11.hff"))
    else {
        return;
    };
    text::print(
        fer.canvas,
        &font,
        r.x0 + 6,
        r.y0,
        [255, 200, 0, 255],
        &text::to_latin1(&item.text),
        (Shadow::SE, [0, 0, 0, 255]),
    );
}

/// `spDonePicking`: the row becomes the scenario, its settings apply, and the CPU count is cut
/// to what the map takes.
fn done_picking(fe: &mut Fe) {
    let s = &mut fe.game.skirmish;
    if let Some(i) = s.picking.take() {
        if i < s.scenarios.len() {
            s.scenario = i;
        }
    }
    if let Some(g) = s.scenarios.get(s.scenario).and_then(|m| m.settings.clone()) {
        g.apply(&mut s.options);
        s.retype();
    }
    let max = s.max_players();
    if usize::from(s.options.num_computers) + 1 > max {
        s.options.num_computers = max.saturating_sub(1) as u8;
    }
    fe.current_screen_delete();
    sync(fe);
}

/// `cpNewRace`: Race One is the Somtaaw, Race Two the Beast; the buttons show the pick.
fn race(fe: &mut Fe, call: &Call) {
    if fe.last_call(call) {
        return;
    }
    if !fe.first_call(call) {
        fe.game.skirmish.race = u8::from(call.name != "CP_RaceOne");
    }
    let beast = fe.game.skirmish.race == 1;
    fe.toggle_set("CP_RaceOne", !beast);
    fe.toggle_set("CP_RaceTwo", beast);
}

/// `utyStartSkirmish` (`StartNewGame` on the skirmish screens).
fn start(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    if !fe.game.skirmish.loaded {
        return;
    }
    fe.game.colors.accept(&mut fe.game.options);
    let name = fe.game.skirmish.name.clone();
    fe.game.options.set_str("PlayerName", &name);
    fe.game
        .options
        .set("PlayerRace", i32::from(fe.game.skirmish.race));
    let Some(req) = fe.game.skirmish.request() else {
        fe.log_once("no multiplayer maps".into());
        return;
    };
    fe.all_screens_delete();
    fe.game.requests.push(Request::StartSkirmish(req));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_level_title_is_its_bracket_line() {
        let l = "; MissionMan script\n; Info: Coldwar (2)\n\n[Coldwar (2)]\n\nMissionSphere 0\n";
        assert_eq!(level_title(l).as_deref(), Some("Coldwar (2)"));
        assert_eq!(level_title("nothing"), None);
    }

    #[test]
    fn descriptions_keep_english_lines_and_skip_comments() {
        let (lines, settings) = parse_description("Description         One.\nDescription\nDescriptionFrench Un.\n;type.flag BIT1\nFont Arial_b12.hff\n");
        assert_eq!(lines, ["One.", ""]);
        assert!(settings.is_none());
        let (_, s) = parse_description("type.flag BIT3\ntype.flagNeeded BIT3\ntype.bountySize 2\n");
        let s = s.expect("settings");
        let mut o = GameOptions {
            flags: 0x22,
            ..Default::default()
        };
        s.apply(&mut o);
        assert_eq!((o.flags, o.bounty_size), (0x2a, 2));
    }
}
