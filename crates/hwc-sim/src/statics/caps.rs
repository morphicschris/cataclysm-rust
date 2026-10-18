//! Unit caps (`UnitLimitCaps.script` at startup, then `UnitLimitCaps<n>.script` on top of it
//! when a game starts; `unitCapInitStatics`).

use hwc_formats::script::ScriptFile;

use super::cscan::scan_i32;
use super::enums::{ShipClass, ShipRace, ShipType};

/// Caps by ship type and by class. -1 disables the cap (support units limit the type instead).
#[derive(Debug, Clone, PartialEq)]
pub struct UnitCaps {
    /// `ShipTotalCaps`.
    pub total: i32,
    /// `Cap<shiptype>` (e.g. `CapsRecon`, `CapbCarrier`). Derelict-race types other than
    /// dAcolyte have no keyword.
    pub by_type: [i32; ShipType::COUNT],
    /// `CapCLASS_<class>`.
    pub by_class: [i32; ShipClass::COUNT],
}

impl Default for UnitCaps {
    fn default() -> Self {
        Self { total: 0, by_type: [0; ShipType::COUNT], by_class: [0; ShipClass::COUNT] }
    }
}

/// The keyword that sets a type's cap, if it has one.
pub fn cap_keyword(t: ShipType) -> Option<String> {
    if t.race() == ShipRace::Derelict && t != ShipType::dAcolyte {
        return None;
    }
    if t == ShipType::trGhostship {
        return Some("CaptrGhostShip".to_owned());
    }
    Some(format!("Cap{}", t.name()))
}

impl UnitCaps {
    /// Applies a caps script on top of the current values (lines that are absent keep their
    /// value, as with the original's `scriptSet`).
    pub fn apply(&mut self, script: &ScriptFile) {
        let keys: Vec<(String, ShipType)> =
            ShipType::ALL.iter().filter_map(|&t| cap_keyword(t).map(|k| (k, t))).collect();
        for e in script.entries() {
            let Some(v) = scan_i32(&e.raw) else { continue };
            if e.key == "ShipTotalCaps" {
                self.total = v;
            } else if let Some(c) = e.key.strip_prefix("Cap").and_then(class_by_exact_name) {
                self.by_class[c.index()] = v;
            } else if let Some((_, t)) = keys.iter().find(|(k, _)| *k == e.key) {
                self.by_type[t.index()] = v;
            }
        }
    }

    pub fn cap(&self, t: ShipType) -> i32 {
        self.by_type[t.index()]
    }
}

/// Exact (case-sensitive) class name lookup, for keyword suffixes.
fn class_by_exact_name(s: &str) -> Option<ShipClass> {
    ShipClass::ALL.iter().copied().find(|c| c.name() == s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords() {
        assert_eq!(cap_keyword(ShipType::sRecon).as_deref(), Some("CapsRecon"));
        assert_eq!(cap_keyword(ShipType::trGhostship).as_deref(), Some("CaptrGhostShip"));
        assert_eq!(cap_keyword(ShipType::dRKO), None);
        let mut caps = UnitCaps::default();
        caps.apply(&ScriptFile::parse("ShipTotalCaps 297\nCapsCarrier 2\nCapCLASS_Frigate 12\ncapsRecon 4\n"));
        assert_eq!(caps.total, 297);
        assert_eq!(caps.cap(ShipType::sCarrier), 2);
        assert_eq!(caps.by_class[ShipClass::Frigate.index()], 12);
        assert_eq!(caps.cap(ShipType::sRecon), 0);
    }
}
