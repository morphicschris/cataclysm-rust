//! Pulls the Remastered death explosions and engine-trail art out of the player's own Homeworld
//! Remastered install into a folder of loose files the game reads ([`crate::Library::open`]).
//!
//! The install is only read. What is copied: the `.events` and `.ship` files of the Kushan and
//! Taiidan ships (`HW1Ships.big`), every effect their death sequences start (followed through
//! combos, spray particles and spawns), the textures and animations those effects draw with, and
//! the engine-trail textures (`Homeworld2.big`). Layout of the output:
//!
//! ```text
//! index.txt                 format line and counts
//! ships/<ship>.events       ships/<ship>.ship
//! fx/<effect>.lua
//! <texture path>            (art/fx/..., effect/trails/...)
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::big::Archive;
use crate::fx::{Anim, FxDef};
use crate::ships::DeathSequence;

/// First line of `index.txt`.
pub const INDEX_FORMAT: &str = "hwc-hwr 1";

/// The folder of a game data directory the effects are imported into.
pub const DATA_DIR: &str = "hwr";

/// Ship name prefixes imported (the Homeworld 1 races of the Remastered collection).
pub const SHIP_PREFIXES: [&str; 2] = ["kus_", "tai_"];

/// What an import wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub ships: usize,
    pub effects: usize,
    pub textures: usize,
    pub bytes: u64,
    /// Effects named somewhere but not found in the archive.
    pub missing: Vec<String>,
}

/// The `Data` folder of a Remastered install given the install (`.../Homeworld`), its
/// `HomeworldRM` folder or the `Data` folder itself.
pub fn find_data_dir(root: &Path) -> Option<PathBuf> {
    [
        root.join("HomeworldRM").join("Data"),
        root.join("Data"),
        root.to_path_buf(),
    ]
    .into_iter()
    .find(|d| d.join("Homeworld2.big").is_file() && d.join("HW1Ships.big").is_file())
}

fn write(out: &Path, rel: &str, bytes: &[u8], report: &mut Report) -> io::Result<()> {
    let path = out.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&path, bytes)?;
    report.bytes += bytes.len() as u64;
    Ok(())
}

fn stem(path: &str) -> &str {
    let file = path.rsplit('/').next().unwrap_or(path);
    file.strip_suffix(".lua").unwrap_or(file)
}

/// Imports from the install at `root` into `out` (created if needed).
pub fn import(root: &Path, out: &Path) -> io::Result<Report> {
    let data = find_data_dir(root).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "no Homeworld Remastered data (Homeworld2.big, HW1Ships.big) under {}",
                root.display()
            ),
        )
    })?;
    let mut main = Archive::open(data.join("Homeworld2.big"))?;
    let mut ships = Archive::open(data.join("HW1Ships.big"))?;
    fs::create_dir_all(out)?;
    let mut report = Report::default();

    // Ships: their events and tuning; the death effects seed the effect closure.
    let mut seeds: BTreeSet<String> = BTreeSet::new();
    let ship_files: Vec<_> = ships
        .entries()
        .iter()
        .filter(|e| {
            let file = e.path.rsplit('/').next().unwrap_or("");
            e.path.starts_with("ship/")
                && SHIP_PREFIXES.iter().any(|p| file.starts_with(p))
                && (file.ends_with(".events") || file.ends_with(".ship"))
                // Only the ship's own files (`ship/kus_scout/kus_scout.ship`), not variants.
                && e.path.split('/').nth(1).is_some_and(|dir| file.starts_with(dir) && e.path.split('/').count() == 3)
        })
        .cloned()
        .collect();
    let mut ship_names = BTreeSet::new();
    for e in &ship_files {
        let bytes = ships.read_entry(e)?;
        let file = e.path.rsplit('/').next().unwrap_or("");
        if file.ends_with(".events") {
            seeds.extend(DeathSequence::parse(&bytes).effects().map(str::to_string));
        }
        ship_names.insert(file.rsplit_once('.').map_or(file, |(s, _)| s).to_string());
        write(out, &format!("ships/{file}"), &bytes, &mut report)?;
    }
    report.ships = ship_names.len();

    // Effects by stem (the first file of a name wins, as names are global).
    let mut by_stem: BTreeMap<String, crate::big::Entry> = BTreeMap::new();
    for e in main.entries() {
        if e.path.starts_with("art/fx/") && e.path.ends_with(".lua") {
            by_stem
                .entry(stem(&e.path).to_string())
                .or_insert_with(|| e.clone());
        }
    }
    let mut done: BTreeSet<String> = BTreeSet::new();
    let mut textures: BTreeSet<String> = BTreeSet::new();
    let mut stack: Vec<String> = seeds.into_iter().collect();
    while let Some(name) = stack.pop() {
        if !done.insert(name.clone()) {
            continue;
        }
        let Some(e) = by_stem.get(&name).cloned() else {
            report.missing.push(name);
            continue;
        };
        let bytes = main.read_entry(&e)?;
        if let Some(def) = FxDef::parse(&name, &bytes) {
            stack.extend(def.children().into_iter().filter(|c| !done.contains(c)));
            textures.extend(def.textures());
        }
        write(out, &format!("fx/{name}.lua"), &bytes, &mut report)?;
        report.effects += 1;
    }
    report.missing.sort();

    // Trail textures.
    let trail: Vec<String> = main
        .entries()
        .iter()
        .filter(|e| e.path.starts_with("effect/trails/") && e.path.ends_with(".tga"))
        .map(|e| e.path.clone())
        .collect();
    textures.extend(trail);

    // Textures, and the frames of animations.
    let mut written = BTreeSet::new();
    let mut queue: Vec<String> = textures.into_iter().collect();
    while let Some(t) = queue.pop() {
        if !written.insert(t.clone()) {
            continue;
        }
        let Some(bytes) = main.read(&t)? else {
            continue;
        };
        if t.ends_with(".anim") {
            queue.extend(Anim::parse(&bytes).frames.into_iter().map(|f| f.path));
        } else if !t.ends_with(".tga") {
            continue;
        } else {
            report.textures += 1;
        }
        write(out, &t, &bytes, &mut report)?;
    }

    let index = format!(
        "{INDEX_FORMAT}\nships {}\neffects {}\ntextures {}\nsource {}\n",
        report.ships,
        report.effects,
        report.textures,
        data.display()
    );
    fs::write(out.join("index.txt"), index)?;
    Ok(report)
}
