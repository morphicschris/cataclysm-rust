//! Remastered battle chatter: the `sound/speech/allships/<voice>/<line>/*.fda` clips of
//! Homeworld Remastered's `EnglishSpeech.big`, by the Cataclysm battle chatter event
//! (`BCE_*`, the table index of the simulation's battle chatter) they can say.
//!
//! Remastered names its lines after Homeworld 1's speech events, which Cataclysm's battle
//! chatter inherited, so most events have a counterpart; a few Cataclysm events ("you've
//! picked up a trailer", the wingman's hit) have no line of Cataclysm's own, and these fill
//! them. Voices are `all_*` (any ship, the group reports), `cap_*` (capital ships), `fighter_*`
//! (strike craft), `support_*` (resourcers and support ships) and `fleet` (fleet command).
//!
//! The import copies only the clips of the mapped lines, to `speech/<voice>/<line>/<clip>.fda`
//! in the imported folder.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::big::Archive;

/// The speech archive in the Remastered `Data` folder.
pub const SPEECH_BIG: &str = "EnglishSpeech.big";

/// Where the clips are, in the imported folder.
pub const SPEECH_DIR: &str = "speech";

/// Per battle chatter event (`BCE_*`), the Remastered lines that say it. Hot-key group lines
/// (`status_group01_inminefield`, `status_hotkeygroup_victory_01`) take the group number
/// for `NN`.
pub const LINES: &[(usize, &[&str])] = &[
    (0x01, &["chatter_genericgrouppositivebattle_report"]),
    (0x02, &["chatter_genericgrouppositivebattle_report"]),
    (0x03, &["chatter_genericgroupnegativebattle_report"]),
    (0x04, &["chatter_genericgroupnegativebattle_report"]),
    (0x05, &["chatter_genericgroupnegativebattle_report"]),
    (0x06, &["chatter_genericgroupstartbattlefair_report"]),
    (0x07, &["chatter_genericgroupstartbattlefair_report"]),
    (0x0a, &["chatter_fighterchased_report"]),
    (0x0c, &["chatter_strikecraftdamaged"]),
    (0x10, &["chatter_fighterchased_report"]),
    (
        0x12,
        &["chatter_genericgroupstartbattledisadvantaged_report"],
    ),
    (
        0x13,
        &["chatter_genericgroupstartbattledisadvantaged_response"],
    ),
    (0x14, &["chatter_genericgroupstartbattlefair_report"]),
    (0x15, &["chatter_genericgroupstartbattlefair_response"]),
    (0x16, &["chatter_genericgroupstartbattleadvantaged_report"]),
    (
        0x17,
        &["chatter_genericgroupstartbattleadvantaged_response"],
    ),
    (0x18, &["command_genericgroupretreat_report"]),
    (0x19, &["command_genericgroupretreat_response"]),
    (0x1a, &["chatter_genericgroupwinningbattle_report"]),
    (0x1b, &["chatter_genericgroupwinningbattle_response"]),
    (
        0x1c,
        &[
            "chatter_genericgroupwinningbattle_report",
            "chatter_genericgrouppositivebattle_report",
        ],
    ),
    (0x1d, &["chatter_genericgroupwinningbattle_response"]),
    (0x1e, &["chatter_genericgroupbattlewon_report"]),
    (0x1f, &["status_hotkeygroup_victory_NN"]),
    (0x20, &["chatter_genericgrouplosingbattle_report"]),
    (0x21, &["chatter_genericgrouplosingbattle_report"]),
    (0x22, &["chatter_genericgrouplosingbattle_response"]),
    (0x23, &["chatter_genericgrouplosingbattle_report"]),
    (0x24, &["chatter_genericgrouplosingbattle_report"]),
    (0x25, &["chatter_genericgrouplosingbattle_response"]),
    (0x29, &["status_attackcomplete"]),
    (0x30, &["status_genericgroupenemyretreated"]),
    (0x34, &["status_genericgroupinminefield"]),
    (0x35, &["status_groupNN_inminefield"]),
    (0x37, &["status_genericgroupunderattackretaliate_defensive"]),
    (0x3f, &["status_capitalship_damaged_report"]),
    (0x40, &["status_capitalshipdies"]),
    (0x43, &["status_mothershipunderlargeattack_report"]),
    (0x45, &["status_carrierormshipcapitalshiplaunched"]),
    (0x46, &["status_anyshipcapitalshipwelcomed"]),
    (0x48, &["status_capitalshipsshipdocked"]),
    (0x4a, &["chatter_anyshipdocking"]),
    (0x4b, &["chatter_resourcecollectorfull"]),
    (0x52, &["status_finishedrepairs"]),
    (0x53, &["chatter_resourcecollectorresourcestransferred"]),
];

/// The lines of event `bce`, with a hot-key group's number filled in (`group` 1..=10).
pub fn lines_of(bce: usize, group: Option<u8>) -> Vec<String> {
    let Some((_, lines)) = LINES.iter().find(|(k, _)| *k == bce) else {
        return Vec::new();
    };
    lines
        .iter()
        .filter_map(|l| match (l.contains("NN"), group) {
            (false, _) => Some(l.to_string()),
            (true, Some(g)) if (1..=10).contains(&g) => Some(l.replace("NN", &format!("{g:02}"))),
            (true, _) => None,
        })
        .collect()
}

fn is_mapped(line: &str) -> bool {
    LINES
        .iter()
        .flat_map(|(_, l)| l.iter())
        .any(|l| match l.split_once("NN") {
            Some((a, b)) => line.len() == l.len() && line.starts_with(a) && line.ends_with(b),
            None => *l == line,
        })
}

/// Copies the clips of the mapped lines from the Remastered `Data` folder to
/// `out/speech/...`. The number of clips and their bytes; nothing when there is no speech
/// archive.
pub fn import(data: &Path, out: &Path) -> io::Result<(usize, u64)> {
    let path = data.join(SPEECH_BIG);
    if !path.is_file() {
        return Ok((0, 0));
    }
    let mut big = Archive::open(path)?;
    let entries: Vec<_> = big
        .entries()
        .iter()
        .filter(|e| {
            let parts: Vec<&str> = e.path.split('/').collect();
            parts.len() == 6
                && parts[..3] == ["sound", "speech", "allships"]
                && parts[5].ends_with(".fda")
                && is_mapped(parts[4])
        })
        .cloned()
        .collect();
    let (mut n, mut bytes) = (0, 0);
    for e in entries {
        let data = big.read_entry(&e)?;
        let rel = e.path.trim_start_matches("sound/speech/allships/");
        let path = out
            .join(SPEECH_DIR)
            .join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&path, &data)?;
        n += 1;
        bytes += data.len() as u64;
    }
    Ok((n, bytes))
}

/// The voice families (the folder name before `_N`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Family {
    All,
    Capital,
    Fighter,
    Support,
    Fleet,
}

impl Family {
    fn of(voice: &str) -> Option<Self> {
        let base = voice.rsplit_once('_').map_or(voice, |(b, _)| b);
        Some(match base {
            "all" => Family::All,
            "cap" => Family::Capital,
            "fighter" => Family::Fighter,
            "support" => Family::Support,
            "fleet" => Family::Fleet,
            _ => return None,
        })
    }
}

/// The imported clips: per line, the voices that say it and their clip files.
#[derive(Debug, Clone, Default)]
pub struct Chatter {
    lines: BTreeMap<String, Vec<(String, Family, Vec<PathBuf>)>>,
}

impl Chatter {
    /// The clips under `dir/speech` (an imported folder); empty when there are none.
    pub fn open(dir: &Path) -> Self {
        let mut c = Chatter::default();
        let Ok(voices) = fs::read_dir(dir.join(SPEECH_DIR)) else {
            return c;
        };
        let mut voices: Vec<_> = voices.flatten().filter(|v| v.path().is_dir()).collect();
        voices.sort_by_key(|v| v.file_name());
        for v in voices {
            let voice = v.file_name().to_string_lossy().to_ascii_lowercase();
            let Some(family) = Family::of(&voice) else {
                continue;
            };
            let Ok(lines) = fs::read_dir(v.path()) else {
                continue;
            };
            for l in lines.flatten() {
                let Ok(clips) = fs::read_dir(l.path()) else {
                    continue;
                };
                let mut clips: Vec<PathBuf> = clips
                    .flatten()
                    .map(|c| c.path())
                    .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("fda")))
                    .collect();
                if clips.is_empty() {
                    continue;
                }
                clips.sort();
                let line = l.file_name().to_string_lossy().to_ascii_lowercase();
                c.lines
                    .entry(line)
                    .or_default()
                    .push((voice.clone(), family, clips));
            }
        }
        c
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    /// Whether event `bce` has a line.
    pub fn has(&self, bce: usize, group: Option<u8>) -> bool {
        lines_of(bce, group)
            .iter()
            .any(|l| self.lines.contains_key(l))
    }

    /// A clip for event `bce` said by a voice of the first of `families` that has it (any
    /// voice otherwise). `seed` picks the voice (the same speaker keeps its voice) and `roll`
    /// the line and clip.
    pub fn pick(
        &self,
        bce: usize,
        group: Option<u8>,
        families: &[Family],
        seed: u32,
        roll: u32,
    ) -> Option<&Path> {
        let lines: Vec<_> = lines_of(bce, group)
            .into_iter()
            .filter_map(|l| self.lines.get(&l))
            .collect();
        if lines.is_empty() {
            return None;
        }
        let voices = &lines[roll as usize % lines.len()];
        let mut choice: Vec<_> = Vec::new();
        for f in families {
            choice = voices.iter().filter(|(_, vf, _)| vf == f).collect();
            if !choice.is_empty() {
                break;
            }
        }
        if choice.is_empty() {
            choice = voices.iter().collect();
        }
        let (_, _, clips) = choice[seed as usize % choice.len()];
        Some(clips[(roll / 7) as usize % clips.len()].as_path())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_lines_take_the_group_number() {
        assert_eq!(
            lines_of(0x35, Some(3)),
            vec!["status_group03_inminefield".to_string()]
        );
        assert!(lines_of(0x35, None).is_empty());
        assert!(is_mapped("status_group10_inminefield"));
        assert!(is_mapped("chatter_fighterchased_report"));
        assert!(!is_mapped("command_move"));
    }

    #[test]
    fn voices_pick_by_family() {
        let mut c = Chatter::default();
        let clip = |n: &str| vec![PathBuf::from(n)];
        c.lines.insert(
            "chatter_fighterchased_report".into(),
            vec![
                ("all_1".into(), Family::All, clip("a")),
                ("fighter_2".into(), Family::Fighter, clip("f")),
            ],
        );
        assert_eq!(
            c.pick(0x0a, None, &[Family::Fighter], 5, 0),
            Some(Path::new("f"))
        );
        assert_eq!(
            c.pick(0x0a, None, &[Family::Capital], 0, 0),
            Some(Path::new("a"))
        );
        assert!(c.has(0x10, None));
        assert!(!c.has(0x0b, None));
    }
}
