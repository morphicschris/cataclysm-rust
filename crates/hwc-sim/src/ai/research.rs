//! The research questions the computer player asks (`ResearchAPI.c`: `rmTechRequiredForShip`
//! 0x4ee3a0, `rmTechRequiredForShipUpgrades`, `rmResearchTechForShip`, `rmShipTypeAcquired`,
//! `rmResearchTechIfLabFree`).

use crate::base::{Techs, NUM_TECHS};
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

/// The technologies a set needs, closed over the technologies' own dependencies
/// (`rmAllTechRequiredforTech` for each), as a set; `None` when it needs every technology.
fn closure(u: &Universe, player: usize, start: &Techs) -> Option<[bool; NUM_TECHS]> {
    if start.mode == Techs::MODE_ALL {
        return None;
    }
    let table = crate::research::table_for(u, u.base.players[player].research_table);
    let mut need = [false; NUM_TECHS];
    let mut stack: Vec<usize> = (0..NUM_TECHS).filter(|&i| start.has[i] == 1).collect();
    while let Some(t) = stack.pop() {
        if need[t] {
            continue;
        }
        need[t] = true;
        if let Some(deps) = table.tech_depend.get(t) {
            for d in deps.iter() {
                if !need[d.index()] {
                    stack.push(d.index());
                }
            }
        }
    }
    Some(need)
}

/// The technologies of `set` (closed over their dependencies) the player does not have.
fn missing(u: &Universe, player: usize, set: &Techs) -> Vec<usize> {
    let have = &u.base.players[player].tech;
    if have.mode == Techs::MODE_ALL {
        return Vec::new();
    }
    match closure(u, player, set) {
        None => (0..NUM_TECHS).filter(|&i| have.has[i] != 1).collect(),
        Some(need) => (0..NUM_TECHS).filter(|&i| need[i] && have.has[i] != 1).collect(),
    }
}

fn ship_depend(u: &Universe, player: usize, t: ShipType) -> Techs {
    let table = crate::research::table_for(u, u.base.players[player].research_table);
    crate::research::depend_set(table, t)
}

fn upgrade_depend(u: &Universe, player: usize, t: ShipType) -> Techs {
    u.base.players[player].ship_upgrade_techs.get(t.index()).copied().unwrap_or_default()
}

/// `rmTechRequiredForShip`: how many technologies the player still needs to build `t`.
pub fn tech_required_for_ship(u: &Universe, player: usize, t: ShipType) -> usize {
    missing(u, player, &ship_depend(u, player, t)).len()
}

/// `rmTechRequiredForShipUpgrades`: how many technologies the player still needs for `t`'s
/// upgrades.
pub fn tech_required_for_ship_upgrades(u: &Universe, player: usize, t: ShipType) -> usize {
    missing(u, player, &upgrade_depend(u, player, t)).len()
}

/// `rmShipTypeAcquired` (Beast: the type is known from docking or capture).
pub fn ship_type_acquired(u: &Universe, player: usize, t: ShipType) -> bool {
    u.base.players[player].acquired.get(t.index()).copied().unwrap_or(false)
}

/// Starts every missing technology whose own dependencies are met and that has a free lab.
fn research_ready(u: &mut Universe, player: usize, missing: &[usize]) -> bool {
    let mut started = false;
    for &tech in missing {
        let table = crate::research::table_for(u, u.base.players[player].research_table);
        let ready = table.tech_depend.get(tech).is_none_or(|deps| deps.iter().all(|d| u.base.players[player].tech.contains(d.index())));
        if !ready {
            continue;
        }
        if let Some(lab) = crate::research::find_free_lab(u, player, tech as i32) {
            crate::research::assign_lab(u, player, lab, tech);
            started = true;
        }
    }
    started
}

/// `rmResearchTechForShip` (0x4ee840): the technologies `t` needs, else those its upgrades
/// need, whose own dependencies are met, are researched in the free labs. A Beast player only
/// researches for types it has acquired.
pub fn research_tech_for_ship(u: &mut Universe, player: usize, t: ShipType) -> bool {
    if u.players[player].race == ShipRace::Beast && !ship_type_acquired(u, player, t) {
        return false;
    }
    let m = missing(u, player, &ship_depend(u, player, t));
    if !m.is_empty() {
        return research_ready(u, player, &m);
    }
    let m = missing(u, player, &upgrade_depend(u, player, t));
    if !m.is_empty() {
        return research_ready(u, player, &m);
    }
    false
}

/// `rmResearchTechIfLabFree`: researches `tech` when the player lacks it and a lab is free.
pub fn research_tech_if_lab_free(u: &mut Universe, player: usize, tech: usize) -> bool {
    let p = &u.base.players[player];
    if p.tech.mode == Techs::MODE_ALL || p.tech.has.get(tech).copied() == Some(1) {
        return false;
    }
    match crate::research::find_free_lab(u, player, tech as i32) {
        Some(lab) => {
            crate::research::assign_lab(u, player, lab, tech);
            true
        }
        None => false,
    }
}
