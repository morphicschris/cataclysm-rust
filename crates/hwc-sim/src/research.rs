//! `ResearchAPI.c`: Cataclysm's research. A player has five labs, one per research module of
//! the mothership (docking bay, armour, engineering, special, weapons); each technology is
//! researched in the lab of one module type ([`tech_lab`]). Building the module activates its
//! lab, losing it cancels the research there. The Beast has a single lab while its mothership
//! lives and acquires technologies by docking captured ships (`rmAcquireShipTechOnDock`).
//! Research time is counted down once per `REFRESH_RESEARCH_RATE` frames.

use crate::base::{self, lab_status, BaseEvent, ResearchTableKind, ResearchTopic, Techs, NUM_LABS, NUM_TECHS};
use crate::object::ShipId;
use crate::statics::{ResearchTable, ShipRace, ShipType};
use crate::universe::Universe;

/// 0x8a74d4: races (bit per race) each ship type exists for.
const TYPE_RACES: [u16; 145] = [
    0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1,
    0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1, 0x1,
    0x1, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2,
    0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x2, 0x4, 0x4, 0x4, 0x4,
    0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x4, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8,
    0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x8, 0x10, 0x10, 0x10, 0x10,
    0x10, 0x10, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x1, 0x20, 0x20, 0x20, 0x20, 0x40, 0x40, 0x40,
    0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40,
    0x40, 0x40, 0x40, 0x40, 0x40, 0x20, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x40, 0x0, 0x0,
    0x0,
];

/// 0x879bb8: the Somtaaw/Beast stand-in for a type that does not exist for a race (only the
/// first 60 entries are meaningful; the table is followed by unrelated strings).
const TYPE_ALTERNATIVE: [usize; 60] = [
    33, 34, 36, 37, 38, 39, 48, 41, 42, 43, 46, 33, 47, 33, 49, 50, 51, 52, 53, 33, 33, 33, 33, 33, 54, 44, 57, 33, 55, 33,
    33, 33, 33, 0, 1, 1, 2, 1, 4, 5, 4, 7, 8, 9, 25, 7, 10, 12, 6, 14, 15, 16, 17, 18, 24, 28, 0, 26, 0, 0,
];

/// 0x886260: the lab (research module) each technology is researched in (-1: any lab).
const TECH_LAB: [i32; NUM_TECHS] = [
    -1, -1, -1, -1, -1, -1, 2, 2, 0, 0, 0, 0, 0, 0, 1, 1, 2, 3, 4, 4, 4, 4, 4, 4, 0, 2, 2, 4, 3, 1, 1, 1, -1, 3, 3, 2, 3, 3, 3,
    -1, -1,
];

/// 0x721b38: the technology of each tech level (41 = none).
pub const TECH_LEVEL_TECHS: [usize; 3] = [41, 14, 15];

/// Race type ranges (0x87f3f0 first, 0x87f40c last) per race.
const RACE_FIRST: [usize; 7] = [0, 33, 60, 74, 92, 98, 109];
const RACE_LAST: [usize; 7] = [32, 59, 73, 91, 97, 108, 144];

/// `GetAppropriateShipTypeForRace`: the type itself if it exists for the race, else its
/// stand-in.
pub fn appropriate_type_for_race(t: ShipType, race: ShipRace) -> ShipType {
    let i = t.index();
    if (1u32 << (race.index() as u32 & 0x1f)) & u32::from(TYPE_RACES[i]) == 0 {
        TYPE_ALTERNATIVE.get(i).and_then(|&a| ShipType::from_index(a)).unwrap_or(t)
    } else {
        t
    }
}

/// The research table a player uses.
pub fn table_for(u: &Universe, kind: ResearchTableKind) -> &ResearchTable {
    match kind {
        ResearchTableKind::SinglePlayerSomtaaw => &u.statics.research.single_player_somtaaw,
        ResearchTableKind::MultiplayerSomtaaw => &u.statics.research.multiplayer_somtaaw,
        ResearchTableKind::Beast => &u.statics.research.beast,
    }
}

/// The ship-dependency set of a type (empty, mode 0, when the table has no line for it), with
/// the mode `rmEnableShip` gave it (-2: disabled).
pub fn depend_set(table: &ResearchTable, t: ShipType) -> Techs {
    let mut d = table.ship_depend.get(&t).map_or_else(Techs::default, |s| Techs::from_set(*s));
    if let Some(&mode) = table.ship_mode.get(&t) {
        d.mode = mode;
    }
    d
}

/// The lab a technology is researched in (-1: any lab).
pub fn tech_lab(tech: usize) -> i32 {
    TECH_LAB.get(tech).copied().unwrap_or(-1)
}

/// The lab of a Somtaaw research module type.
pub fn module_lab(t: ShipType) -> Option<usize> {
    match t {
        ShipType::sMothershipSpecial => Some(3),
        ShipType::sMothershipArmour => Some(1),
        ShipType::sMothershipMicro => Some(2),
        ShipType::sMothershipWeapons => Some(4),
        ShipType::sMothershipDockingBay => Some(0),
        _ => None,
    }
}

/// `rmInitializeResearchStruct` (0x4eedf0) as `gameStart` calls it for a human player: research
/// allowed, the technology set cleared (mode -1; mode 2 gives every technology), the table chosen
/// by game mode and race, the labs empty (the Beast's single lab ready) and the per-type upgrade
/// technologies.
pub fn init_player(u: &mut Universe, player: usize, mode: i32) {
    let race = u.players[player].race;
    let sp = u.single_player;
    let kind = match (sp, race) {
        (_, ShipRace::Beast) => ResearchTableKind::Beast,
        (true, _) => ResearchTableKind::SinglePlayerSomtaaw,
        (false, _) => ResearchTableKind::MultiplayerSomtaaw,
    };
    let ups: Vec<Techs> = (0..base::NUM_SHIP_TYPES).map(|i| if i < 0x21 { ship_type_upgrade_techs(u, i) } else { Techs::default() }).collect();
    let table = table_for(u, kind).clone();
    let p = &mut u.base.players[player];
    p.can_do_research = true;
    p.tech.set_mode(mode);
    p.research_table = kind;
    p.labs = [base::Lab::default(); NUM_LABS];
    p.topics.clear();
    if race != ShipRace::Beast {
        p.ship_upgrade_techs = ups;
    } else {
        p.labs[0].status = lab_status::IDLE;
        for t in 0x21..0x3c {
            let mut s = Techs::default();
            let known = match t - 0x21 {
                0 => {
                    s.add(0x1c);
                    true
                }
                0xb => {
                    s.add(0);
                    true
                }
                0xd | 0x19 => {
                    s.add(0x20);
                    true
                }
                0xe | 0x14 => {
                    s.add(0x22);
                    s.add(0x20);
                    true
                }
                0x10 => {
                    s.add(7);
                    true
                }
                0x12 => true,
                _ => {
                    // Known from the start when the type depends on nothing.
                    let d = ShipType::from_index(t).map(|st| depend_set(&table, st)).unwrap_or_default();
                    d.mode == Techs::MODE_ALL || d.has.iter().any(|&v| v > 0)
                }
            };
            p.ship_upgrade_techs[t] = s;
            p.acquired[t] = known;
        }
    }
    crate::upgrade::player_update_from_research(u, player);
}

/// `suGetShipTypeUpgradeTechs`: the technologies a type's upgrades need.
fn ship_type_upgrade_techs(u: &Universe, t: usize) -> Techs {
    let mut out = Techs::default();
    let Some(st) = ShipType::from_index(t) else { return out };
    let mut mask = crate::upgrade::type_mask(st);
    let mut bit = 0u32;
    while mask != 0 {
        if mask & 1 != 0 {
            for &(tech, a, b) in &crate::upgrade::UPGRADE_BIT_TECHS {
                if a == bit || b == bit {
                    out.add(tech);
                    break;
                }
            }
        }
        mask >>= 1;
        bit += 1;
    }
    if out.has[0x12] == 0 {
        if let Some(guns) = u.statics.ship(st).and_then(|s| s.guns.as_ref()) {
            if guns.iter().any(|g| g.start_upgrade_level != g.end_upgrade_level && g.end_upgrade_level.index() == 1) {
                out.add(0x12);
            }
        }
    }
    let levels = crate::upgrade::max_tech_level(st).max(0) as usize;
    for &tech in TECH_LEVEL_TECHS.iter().take(levels) {
        if tech != 41 {
            out.add(tech);
        }
    }
    out
}

/// `rmActivateLab` (0x4ef920): a research module was built (or placed).
pub fn activate_lab(u: &mut Universe, player: usize, t: ShipType, _announce: bool) {
    let Some(lab) = module_lab(t) else { return };
    let p = &mut u.base.players[player];
    if p.labs[lab].status == lab_status::ABSENT {
        p.labs[lab].status = lab_status::IDLE;
    }
}

/// `rmDeactivateLab` (0x4efa10): a research module died; its research is lost.
pub fn deactivate_lab(u: &mut Universe, player: usize, t: ShipType) {
    let Some(lab) = module_lab(t) else { return };
    clear_lab_topic(u, player, lab);
    u.base.players[player].labs[lab].status = lab_status::ABSENT;
}

/// `rmDeactivateBeastLab` (0x4efaf0).
pub fn deactivate_beast_lab(u: &mut Universe, player: usize) {
    clear_lab_topic(u, player, 0);
    u.base.players[player].labs[0].status = lab_status::ABSENT;
}

fn clear_lab_topic(u: &mut Universe, player: usize, lab: usize) {
    let p = &mut u.base.players[player];
    match p.labs[lab].status {
        lab_status::RESEARCHING => {
            if let Some(id) = p.labs[lab].topic.take() {
                p.topics.retain(|t| t.id != id);
            }
            p.labs[lab].status = lab_status::IDLE;
        }
        lab_status::PAUSED => p.labs[lab].status = lab_status::IDLE,
        _ => {}
    }
}

/// `rmClearResearchlab` (0x4edca0): stop the research in a lab.
pub fn clear_research_lab(u: &mut Universe, player: usize, lab: usize) {
    clear_lab_topic(u, player, lab);
}

/// `rmFindFreeLab` (0x4edc20): an idle lab that can research `tech` (-1: any idle lab).
pub fn find_free_lab(u: &Universe, player: usize, tech: i32) -> Option<usize> {
    let p = &u.base.players[player];
    if u.players[player].race == ShipRace::Beast {
        return (p.labs[0].status == lab_status::IDLE).then_some(0);
    }
    (0..NUM_LABS).find(|&i| p.labs[i].status == lab_status::IDLE && (tech == -1 || tech_lab(tech as usize) == i as i32))
}

/// `rmAssignPlayersLabToResearch` (0x4edd10): lab `lab` starts researching `tech` (continuing
/// a topic already in progress).
pub fn assign_lab(u: &mut Universe, player: usize, lab: usize, tech: usize) {
    let time = table_for(u, u.base.players[player].research_table).tech_time.get(tech).copied().unwrap_or(0);
    let p = &mut u.base.players[player];
    if let Some(t) = p.topics.iter().find(|t| t.tech == tech) {
        let id = t.id;
        p.labs[lab] = base::Lab { status: lab_status::RESEARCHING, topic: Some(id) };
        return;
    }
    let id = p.next_topic_id;
    p.next_topic_id += 1;
    p.topics.push(ResearchTopic { id, tech, time_left: time as f32 });
    p.labs[lab] = base::Lab { status: lab_status::RESEARCHING, topic: Some(id) };
}

/// `rmResearchTechIfLabFree` (0x4ee6c0): start researching `tech` if the player lacks it and a
/// lab for it is free. True when started.
pub fn research_tech_if_lab_free(u: &mut Universe, player: usize, tech: usize) -> bool {
    let p = &u.base.players[player];
    if !p.can_do_research || p.tech.has.get(tech).copied() == Some(1) {
        return false;
    }
    let Some(lab) = find_free_lab(u, player, tech as i32) else { return false };
    assign_lab(u, player, lab, tech);
    true
}

/// `AllowPlayerToResearch` (0x529a40, KAS `TechSetResearch`): the technology may be
/// researched in single player from now on.
pub fn allow_research(u: &mut Universe, player: usize, tech: usize) {
    if let Some(a) = u.base.players.get_mut(player).and_then(|p| p.research_allowed.get_mut(tech)) {
        *a = true;
    }
}

/// `CanPlayerResearch` (0x529ae0).
pub fn research_allowed(u: &Universe, player: usize, tech: usize) -> bool {
    u.base.players.get(player).and_then(|p| p.research_allowed.get(tech)).copied().unwrap_or(false)
}

/// Whether the player is researching `tech` (`Researching`, 0x4eda30).
pub fn researching(u: &Universe, player: usize, tech: usize) -> bool {
    u.base.players.get(player).is_some_and(|p| p.topics.iter().any(|t| t.tech == tech))
}

/// The research topics the research manager offers the player (`rmUpdateTechList`, 0x4f2170:
/// list entries in state 0), in technology order: not known, not being researched, and its
/// lab present; in single player the mission must also have allowed it (`TechSetResearch`),
/// with no prerequisite test; in multiplayer every technology it depends on must be known
/// instead (and technology 0x22 is left out: it needs a game option the multiplayer setup
/// does not give here).
pub fn available_topics(u: &Universe, player: usize) -> Vec<usize> {
    let Some(p) = u.base.players.get(player) else { return Vec::new() };
    let table = table_for(u, p.research_table);
    (0..NUM_TECHS)
        .filter(|&t| {
            if p.tech.contains(t) || researching(u, player, t) {
                return false;
            }
            let lab = tech_lab(t);
            let lab_here = lab < 0 || p.labs.get(lab as usize).is_some_and(|l| l.status != lab_status::ABSENT);
            if !lab_here {
                return false;
            }
            if u.single_player {
                return research_allowed(u, player, t);
            }
            if t == 0x22 {
                return false;
            }
            let need = table.tech_depend.get(t).map_or(0, |d| d.0);
            (0..NUM_TECHS).filter(|&i| need & (1 << i) != 0).all(|i| p.tech.contains(i))
        })
        .collect()
}

/// `rmCanBuildShip` (0x4edab0): the player has every technology the type depends on.
pub fn can_build_ship(u: &Universe, player: usize, t: ShipType, at_mothership: bool) -> bool {
    let p = &u.base.players[player];
    let table = table_for(u, p.research_table);
    let dep = depend_set(table, t);
    if dep.mode == Techs::MODE_DISABLED {
        return false;
    }
    if !u.single_player && t == ShipType::dAcolyte {
        return false;
    }
    let stat = u.statics.ship(t);
    if u.players[player].race == ShipRace::Somtaaw
        && at_mothership
        && stat.is_some_and(|s| s.external_build && s.ship_class != Some(crate::statics::ShipClass::Component))
        && !base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, player)
    {
        return false;
    }
    let both = Techs::and(&dep, &p.tech);
    if both.mode != dep.mode || both.mode != Techs::MODE_ALL {
        for i in 0..NUM_TECHS {
            if both.has[i] != dep.has[i] {
                return false;
            }
        }
    }
    true
}

/// `rmUpdateResearch` (0x4eddc0): count every topic down by one (eight in the tutorial);
/// finished topics give the technology and free their lab.
pub fn update_research(u: &mut Universe) {
    for player in 0..u.base.players.len() {
        if !u.base.players[player].can_do_research {
            continue;
        }
        let ids: Vec<u32> = u.base.players[player].topics.iter().map(|t| t.id).collect();
        for id in ids {
            // A Beast mothership in its research damage mode researches twice as fast
            // (`MothershipGetDamageModifier4`).
            let beast_ms = u.base.players[player].mothership.filter(|_| u.players.get(player).is_some_and(|p| p.race == crate::statics::ShipRace::Beast));
            let step = if u.base.options.tutorial {
                8.0
            } else if let Some(ms) = beast_ms {
                f64::from(crate::ships::mothership::damage_modifier_research(u, ms))
            } else {
                1.0
            };
            let p = &mut u.base.players[player];
            let Some(t) = p.topics.iter_mut().find(|t| t.id == id) else { continue };
            t.time_left = (f64::from(t.time_left) - step) as f32;
            if t.time_left < 0.0 {
                let tech = t.tech;
                p.tech.add(tech);
                p.topics.retain(|x| x.id != id);
                for lab in p.labs.iter_mut() {
                    if lab.topic == Some(id) {
                        lab.status = lab_status::IDLE;
                        lab.topic = None;
                    }
                }
                crate::upgrade::player_update_from_research(u, player);
                u.base.events.push(BaseEvent::Researched { player, tech });
            }
        }
    }
}

/// `rmAcquireShipTechOnDock` (0x4efc60): a Beast ship docking with its mothership hands over
/// the technologies of its upgrades and makes its type known.
pub fn acquire_ship_tech_on_dock(u: &mut Universe, ship: ShipId) {
    let s = u.ships.ship(ship);
    let (player, t) = (s.player, s.ship_type.index());
    if u.base.players[player].labs[0].status == lab_status::ABSENT {
        return;
    }
    let list = ship_upgrade_list(u, ship);
    let p = &mut u.base.players[player];
    let cur = p.ship_upgrade_techs[t];
    let merged = if list.mode == Techs::MODE_ALL || cur.mode == Techs::MODE_ALL {
        Techs { has: [0; NUM_TECHS], mode: Techs::MODE_ALL }
    } else {
        let mut m = list;
        for i in 0..NUM_TECHS {
            m.has[i] |= cur.has[i];
        }
        m.mode = 0;
        m
    };
    p.ship_upgrade_techs[t] = merged;
    p.acquired[t] = true;
}

/// `suGetShipUpgradeList`: the technologies of a ship's current upgrades.
fn ship_upgrade_list(u: &Universe, ship: ShipId) -> Techs {
    let s = u.ships.ship(ship);
    let mut out = Techs::default();
    for &(tech, a, b) in &crate::upgrade::UPGRADE_BIT_TECHS {
        if s.upgrade_flags & a != 0 || s.upgrade_flags & b != 0 {
            out.add(tech);
        }
    }
    for &(tech, gtype) in &crate::upgrade::GUN_TYPE_TECHS {
        if s.guns.iter().any(|g| g.level as usize == gtype) {
            out.add(tech);
        }
    }
    let level = s.armour_level as usize;
    for &tech in TECH_LEVEL_TECHS.iter().take(level + 1) {
        if tech != 41 {
            out.add(tech);
        }
    }
    out
}

/// Ship types of a race (`RACE_FIRST..=RACE_LAST`).
pub fn race_types(race: ShipRace) -> std::ops::RangeInclusive<usize> {
    RACE_FIRST[race.index()]..=RACE_LAST[race.index()]
}
