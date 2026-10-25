//! Research dependencies and times (`ResearchDepend.script`, read by `rmResetStaticInfo`).
//!
//! Cataclysm keeps three tables: single-player Somtaaw (`SPRS*` keywords), multiplayer Somtaaw
//! (`MPRS*`) and multiplayer Beast (`MPRB*`; single-player Beast players use it too). Each
//! dependency entry is a 42-dword array of flags indexed by [`Technology`] (41 used).

use std::collections::BTreeMap;

use hwc_formats::script::ScriptFile;

use super::cscan::scan_i32;
use super::enums::{ShipType, Technology};

/// A set of technologies (bit n = [`Technology`] with index n).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct TechSet(pub u64);

impl TechSet {
    pub fn contains(self, t: Technology) -> bool {
        self.0 & (1 << t.index()) != 0
    }

    pub fn insert(&mut self, t: Technology) {
        self.0 |= 1 << t.index();
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = Technology> {
        Technology::ALL.iter().copied().filter(move |t| self.contains(*t))
    }
}

/// One research table.
#[derive(Debug, Clone, PartialEq)]
pub struct ResearchTable {
    /// Technologies a ship type needs before it can be built. A later line for the same ship
    /// replaces an earlier one.
    pub ship_depend: BTreeMap<ShipType, TechSet>,
    /// Technologies a technology needs before it can be researched.
    pub tech_depend: [TechSet; Technology::COUNT],
    /// Research time per technology (seconds as given in the file).
    pub tech_time: [i32; Technology::COUNT],
    /// The ship dependency sets' mode (`rmTechSetSetMode`, set by `rmEnableShip`): -2 when the
    /// level or the mission disabled the type; absent types are 0.
    pub ship_mode: BTreeMap<ShipType, i32>,
}

impl Default for ResearchTable {
    fn default() -> Self {
        Self {
            ship_depend: BTreeMap::new(),
            tech_depend: [TechSet::default(); Technology::COUNT],
            tech_time: [0; Technology::COUNT],
            ship_mode: BTreeMap::new(),
        }
    }
}

impl ResearchTable {
    /// Technologies in an order where every technology follows its prerequisites, or the first
    /// technology found on a dependency cycle.
    pub fn topological_order(&self) -> Result<Vec<Technology>, Technology> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Active,
            Done,
        }
        fn visit(
            t: Technology,
            table: &ResearchTable,
            marks: &mut [Mark; Technology::COUNT],
            out: &mut Vec<Technology>,
        ) -> Result<(), Technology> {
            match marks[t.index()] {
                Mark::Done => return Ok(()),
                Mark::Active => return Err(t),
                Mark::New => {}
            }
            marks[t.index()] = Mark::Active;
            for d in table.tech_depend[t.index()].iter() {
                visit(d, table, marks, out)?;
            }
            marks[t.index()] = Mark::Done;
            out.push(t);
            Ok(())
        }
        let mut marks = [Mark::New; Technology::COUNT];
        let mut out = Vec::with_capacity(Technology::COUNT);
        for &t in Technology::ALL {
            visit(t, self, &mut marks, &mut out)?;
        }
        Ok(out)
    }
}

/// The three tables.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ResearchStatics {
    /// `SPRS*`: single-player Somtaaw.
    pub single_player_somtaaw: ResearchTable,
    /// `MPRS*`: multiplayer Somtaaw.
    pub multiplayer_somtaaw: ResearchTable,
    /// `MPRB*`: Beast (both modes). Has no `TECH_RESEARCH_CLEESAN_ATTACK` time keyword.
    pub beast: ResearchTable,
    /// Names the original could not resolve (it writes them out of bounds).
    pub warnings: Vec<String>,
}

/// Splits a dependency value into its first word and the comma/blank separated names after it.
fn split_depend(raw: &str) -> Option<(&str, Vec<String>)> {
    let end = raw.find([' ', '\t']).unwrap_or(raw.len());
    let (head, rest) = raw.split_at(end);
    let rest = rest.trim_start_matches([' ', '\t']);
    if rest.is_empty() {
        return None;
    }
    let names = rest
        .replace(',', " ")
        .replace('?', "0")
        .split([' ', '\t'])
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect();
    Some((head, names))
}

impl ResearchStatics {
    pub fn parse(script: &ScriptFile) -> Self {
        let mut r = ResearchStatics::default();
        for e in script.entries() {
            let key = e.key.as_str();
            let Some(prefix) = ["SPRS", "MPRS", "MPRB"].into_iter().find(|p| key.starts_with(p)) else {
                continue;
            };
            let suffix = &key[4..];
            let table = match prefix {
                "SPRS" => &mut r.single_player_somtaaw,
                "MPRS" => &mut r.multiplayer_somtaaw,
                _ => &mut r.beast,
            };
            match suffix {
                "ShipDepend" | "TechDepend" => {
                    let Some((head, names)) = split_depend(&e.raw) else { continue };
                    let mut set = TechSet::default();
                    for n in &names {
                        match Technology::from_name(n) {
                            Some(t) => set.insert(t),
                            None => r.warnings.push(format!("line {}: unknown technology {n:?}", e.line)),
                        }
                    }
                    if suffix == "ShipDepend" {
                        match ShipType::from_name(head) {
                            Some(s) => {
                                table.ship_depend.insert(s, set);
                            }
                            None => r.warnings.push(format!("line {}: unknown ship {head:?}", e.line)),
                        }
                    } else {
                        match Technology::from_name(head) {
                            Some(t) => table.tech_depend[t.index()] = set,
                            None => r.warnings.push(format!("line {}: unknown technology {head:?}", e.line)),
                        }
                    }
                }
                _ => {
                    // `<prefix>TECH_...`: research time. The keyword must name a technology
                    // exactly (the table is case sensitive) and the Beast table lacks one entry.
                    let Some(t) = Technology::ALL.iter().copied().find(|t| t.name() == suffix) else {
                        continue;
                    };
                    if prefix == "MPRB" && t == Technology::ResearchCleesanAttack {
                        continue;
                    }
                    if let Some(v) = scan_i32(&e.raw) {
                        table.tech_time[t.index()] = v;
                    }
                }
            }
        }
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dependencies() {
        let s = ScriptFile::parse(
            "SPRSShipDepend  sMimic  TECH_DRIVE_FIGHTER, TECH_MIMIC\n\
             SPRSTechDepend  TECH_ADRIVE_FIGHTER TECH_DRIVE_FIGHTER\n\
             SPRSTECH_MIMIC 200\nMPRBTECH_RESEARCH_CLEESAN_ATTACK 5\nSPRSShipDepend sRecon\n",
        );
        let r = ResearchStatics::parse(&s);
        let t = &r.single_player_somtaaw;
        let deps = t.ship_depend[&ShipType::sMimic];
        assert!(deps.contains(Technology::DriveFighter) && deps.contains(Technology::Mimic));
        assert_eq!(deps.iter().count(), 2);
        assert!(!t.ship_depend.contains_key(&ShipType::sRecon));
        assert!(t.tech_depend[Technology::AdriveFighter.index()].contains(Technology::DriveFighter));
        assert_eq!(t.tech_time[Technology::Mimic.index()], 200);
        assert_eq!(r.beast.tech_time[Technology::ResearchCleesanAttack.index()], 0);
        let order = t.topological_order().unwrap();
        let pos = |x| order.iter().position(|&o| o == x).unwrap();
        assert!(pos(Technology::DriveFighter) < pos(Technology::AdriveFighter));
    }
}
