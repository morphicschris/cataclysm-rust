//! Which NIS plays when in the single-player campaign (tables in `cataclysm.exe` used by
//! `singlePlayerStartGame`, `singlePlayerGameUpdate` and `singlePlayerNisStoppedCB`).

/// Highest mission number.
pub const MISSIONS: u32 = 17;

/// `nis\nNN.nis` and its script for NIS `n` (1..=11; table 0x88a488).
pub fn nis_files(n: u32) -> Option<(String, String)> {
    (1..=11)
        .contains(&n)
        .then(|| (format!("nis\\n{n:02}.nis"), format!("nis\\n{n:02}.script")))
}

/// The NISlet `n` (1..=3) of `mission` (table 0x88a4d8, indexed `n + mission * 3`; the game
/// appends `.nis` and `.script`). Only three are defined: mission 1 NISlet 1, mission 9
/// NISlet 1 and mission 10 NISlet 1.
pub fn nislet_files(mission: u32, n: u32) -> Option<(String, String)> {
    let base = match (mission, n) {
        (1, 1) => "nis\\nislet01",
        (9, 1) => "nis\\nislet03",
        (10, 1) => "nis\\nislet02",
        _ => return None,
    };
    Some((format!("{base}.nis"), format!("{base}.script")))
}

/// Table 0x88a3f4: the mission's NIS plays in a level of its own (`MissionNN_NISMM`), which
/// is loaded before it; when the NIS stops the mission level is loaded and KAS starts.
pub const NIS_LEVEL_FIRST: [bool; 19] = [
    false, true, false, false, true, false, true, true, false, false, true, false, true, false,
    true, true, true, true, false,
];

/// Table 0x88a440: the NIS that belongs to a mission (-1 none). For missions whose NIS has no
/// level of its own it plays when the fleet has arrived; the entry for mission `m + 1` is the
/// NIS that plays when mission `m` ends.
pub const MISSION_NIS: [i32; 19] = [0, 1, -1, -1, 2, -1, 4, 5, -1, -1, 6, -1, 7, -1, 8, 9, 10, 11, 0];

/// The NIS of `mission`, if any.
pub fn mission_nis(mission: u32) -> Option<u32> {
    MISSION_NIS
        .get(mission as usize)
        .and_then(|&n| (n > 0).then_some(n as u32))
}

/// Whether `mission`'s NIS has a level of its own loaded before it.
pub fn nis_level_first(mission: u32) -> bool {
    NIS_LEVEL_FIRST.get(mission as usize).copied().unwrap_or(false)
}

/// The NIS played after the fleet arrives in `mission` (`singlePlayerGameUpdate`, state 9).
pub fn nis_after_arrival(mission: u32) -> Option<u32> {
    mission_nis(mission).filter(|_| !nis_level_first(mission))
}

/// The `strNIS..` string a `subtitle n` event shows (`nisSubtitle` 0x4cab70). Index 1 has no
/// case in the original's jump table and shows nothing.
pub fn subtitle_string(index: i32) -> Option<&'static str> {
    const NAMES: [&str; 12] = [
        "strNIS01", "", "strNISLET1", "strNIS02", "strNIS04", "strNIS05", "strNIS06", "strNIS07",
        "strNIS08", "strNIS09", "strNIS10", "strNIS11",
    ];
    NAMES
        .get(usize::try_from(index).ok()?)
        .copied()
        .filter(|s| !s.is_empty())
}

/// Variable the NIS-stopped callback sets for a NIS or NISlet.
pub fn complete_variable(nis: Option<u32>, nislet: Option<u32>) -> Option<String> {
    match (nislet, nis) {
        (Some(n), _) => Some(format!("G_Nislet{n}Complete")),
        (None, Some(n)) => Some(format!("G_Nis{n}Complete")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn campaign_tables() {
        assert_eq!(nis_files(1).unwrap().0, "nis\\n01.nis");
        assert!(nis_files(12).is_none());
        assert_eq!(nislet_files(1, 1).unwrap().1, "nis\\nislet01.script");
        assert_eq!(nislet_files(10, 1).unwrap().0, "nis\\nislet02.nis");
        assert!(nislet_files(2, 1).is_none());
        assert_eq!(mission_nis(1), Some(1));
        assert!(nis_level_first(1));
        assert_eq!(nis_after_arrival(1), None);
        assert_eq!(mission_nis(17), Some(11));
        assert_eq!(subtitle_string(0), Some("strNIS01"));
        assert_eq!(subtitle_string(1), None);
        assert_eq!(subtitle_string(2), Some("strNISLET1"));
        assert_eq!(complete_variable(Some(1), None).unwrap(), "G_Nis1Complete");
    }
}
