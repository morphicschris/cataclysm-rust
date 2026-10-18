//! `--verify`: re-hash a data directory against its manifest and parse-check every asset with
//! `hwc-formats`.

use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use hwc_formats::big::{normalize_name, BigArchive};
use hwc_formats::{audio, btg, fib, font, geo, hsf, lif, mad, mex, nis, script};

use crate::datadir::scan_overrides;
use crate::hash;
use crate::import::OVERRIDES_DIR;
use crate::install::{self, FileKind};
use crate::manifest::Manifest;
use crate::{Progress, Stage};

#[derive(Debug, Clone, Copy)]
pub struct VerifyOptions {
    /// Re-hash every file listed in the manifest.
    pub hashes: bool,
    /// Parse every recognised asset in the archive, the overrides and the audio banks.
    pub assets: bool,
    /// Worker threads for the parse check (0 = available parallelism).
    pub threads: usize,
}

impl Default for VerifyOptions {
    fn default() -> Self {
        VerifyOptions { hashes: true, assets: true, threads: 0 }
    }
}

/// Parse-check counts for one file type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KindCount {
    pub ok: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Default)]
pub struct VerifyReport {
    /// Files re-hashed.
    pub files_hashed: usize,
    pub bytes_hashed: u64,
    /// (path, problem) for every missing or mismatching file.
    pub hash_failures: Vec<(String, String)>,
    /// Per extension (`lif`, `geo`, ..., plus `wxd`, `vce`, `kasb`).
    pub by_kind: BTreeMap<String, KindCount>,
    /// (name, error) for every asset that failed to parse.
    pub parse_failures: Vec<(String, String)>,
    /// Archive or override files with no parser (images, demos, ...), by extension.
    pub unchecked: BTreeMap<String, usize>,
    pub overrides: usize,
    pub elapsed: Duration,
}

impl VerifyReport {
    pub fn ok(&self) -> bool {
        self.hash_failures.is_empty() && self.parse_failures.is_empty()
    }

    pub fn assets_checked(&self) -> usize {
        self.by_kind.values().map(|c| c.ok + c.failed).sum()
    }

    fn record(&mut self, kind: &str, name: &str, result: Result<(), String>) {
        let c = self.by_kind.entry(kind.to_owned()).or_default();
        match result {
            Ok(()) => c.ok += 1,
            Err(e) => {
                c.failed += 1;
                self.parse_failures.push((name.to_owned(), e));
            }
        }
    }
}

/// Extensions of the keyword/value text files.
const TEXT_EXTS: [&str; 14] = ["script", "shp", "dist", "missphere", "level", "mission", "ebg", "plug", "mif", "lod", "cred", "lst", "list", "txt"];

fn extension(name: &str) -> String {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default()
}

fn stem_path(name: &str) -> &str {
    name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name)
}

/// Whether `ext` is handled by [`check_asset`].
pub fn is_checked(ext: &str) -> bool {
    matches!(ext, "lif" | "geo" | "peo" | "mex" | "mad" | "btg" | "hff" | "fib" | "hsf" | "nis" | "bnk" | "lut" | "wxh")
        || TEXT_EXTS.contains(&ext)
}

/// Parses one asset by extension. `nis_script` says the file is the event list of a `.nis`.
pub fn check_asset(ext: &str, bytes: &[u8], nis_script: bool) -> Result<(), String> {
    fn s<E: std::fmt::Display>(r: Result<impl Sized, E>) -> Result<(), String> {
        r.map(|_| ()).map_err(|e| e.to_string())
    }
    match ext {
        "lif" => s(lif::Lif::parse(bytes)),
        "geo" | "peo" => s(geo::Geo::parse(bytes)),
        "mex" => s(mex::Mex::parse(bytes)),
        "mad" => s(mad::Mad::parse(bytes)),
        "btg" => s(btg::Btg::parse(bytes)),
        "hff" => s(font::Font::parse(bytes)),
        "fib" => s(fib::Fib::parse(bytes)),
        "hsf" => s(hsf::Hsf::parse(bytes)),
        "nis" => s(nis::Nis::parse(bytes)),
        "bnk" => s(audio::Bank::parse(bytes)),
        "lut" => s(audio::Lut::parse(bytes)),
        "wxh" => s(audio::MusicIndex::parse(bytes)),
        "script" if nis_script => {
            s(script::ScriptFile::parse_bytes(bytes))?;
            s(nis::NisScript::parse_bytes(bytes))
        }
        e if TEXT_EXTS.contains(&e) => s(script::ScriptFile::parse_bytes(bytes)),
        _ => Ok(()),
    }
}

enum Source {
    Archive(usize),
    Loose(PathBuf),
}

struct Job {
    name: String,
    ext: String,
    nis_script: bool,
    source: Source,
}

/// Verifies the data directory at `dir`.
pub fn verify(dir: &Path, opts: VerifyOptions, progress: &mut dyn FnMut(&Progress)) -> std::io::Result<VerifyReport> {
    let start = Instant::now();
    let manifest = Manifest::load(dir)?
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{}: no manifest", dir.display())))?;
    let mut report = VerifyReport::default();
    let path_of = |rel: &str| rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));

    if opts.hashes {
        progress(&Progress::Stage(Stage::VerifyHashes));
        let mut list: Vec<(&str, u64, &str)> = manifest.files.iter().map(|f| (f.path.as_str(), f.size, f.sha256.as_str())).collect();
        for m in &manifest.missions.missions {
            if let (Some(p), Some(size), Some(h)) = (&m.path, m.size, &m.sha256) {
                list.push((p, size, h));
            }
        }
        let total: u64 = list.iter().map(|l| l.1).sum();
        let mut done = 0u64;
        for (rel, size, expected) in list {
            let path = path_of(rel);
            progress(&Progress::FileStarted { path: rel, size });
            report.files_hashed += 1;
            match path.metadata() {
                Err(e) => {
                    report.hash_failures.push((rel.to_owned(), format!("missing: {e}")));
                    continue;
                }
                Ok(m) if m.len() != size => {
                    report.hash_failures.push((rel.to_owned(), format!("size {} differs from manifest {size}", m.len())));
                    continue;
                }
                Ok(_) => {}
            }
            match hash::sha256_file(&path, &mut |n| {
                done += n;
                progress(&Progress::Bytes { done, total });
            }) {
                Ok(h) if h == expected => report.bytes_hashed += size,
                Ok(h) => report.hash_failures.push((rel.to_owned(), format!("sha256 {h} differs from manifest {expected}"))),
                Err(e) => report.hash_failures.push((rel.to_owned(), e.to_string())),
            }
        }
    }

    if opts.assets {
        progress(&Progress::Stage(Stage::VerifyAssets));
        check_assets(dir, &manifest, opts.threads, &mut report, progress)?;
    }
    report.elapsed = start.elapsed();
    Ok(report)
}

fn check_assets(
    dir: &Path,
    manifest: &Manifest,
    threads: usize,
    report: &mut VerifyReport,
    progress: &mut dyn FnMut(&Progress),
) -> std::io::Result<()> {
    let path_of = |rel: &str| rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
    let big_rec = manifest.files.iter().find(|f| f.kind == FileKind::Archive);
    let archive = match big_rec.map(|r| BigArchive::open(path_of(&r.path))) {
        Some(Ok(a)) => Some(a),
        Some(Err(e)) => {
            report.parse_failures.push((big_rec.unwrap().path.clone(), e.to_string()));
            None
        }
        None => {
            report.parse_failures.push(("manifest".into(), "no archive listed".into()));
            None
        }
    };
    let overrides = scan_overrides(&dir.join(OVERRIDES_DIR))?;
    report.overrides = overrides.len();

    // Every name visible to the engine: archive entries shadowed by overrides.
    let mut names: HashSet<String> = overrides.keys().cloned().collect();
    let mut jobs = Vec::new();
    for (key, path) in &overrides {
        jobs.push((key.clone(), Source::Loose(path.clone())));
    }
    if let Some(a) = &archive {
        for (i, e) in a.entries().iter().enumerate() {
            let key = normalize_name(&e.name);
            if names.insert(key.clone()) {
                jobs.push((key, Source::Archive(i)));
            }
        }
    }
    let mut checked = Vec::new();
    for (name, source) in jobs {
        let ext = extension(&name);
        if !is_checked(&ext) {
            *report.unchecked.entry(if ext.is_empty() { "(none)".into() } else { ext }).or_default() += 1;
            continue;
        }
        let nis_script = ext == "script" && names.contains(&format!("{}.nis", stem_path(&name)));
        checked.push(Job { name, ext, nis_script, source });
    }
    checked.sort_by(|a, b| a.name.cmp(&b.name));

    let total = checked.len();
    let threads = if threads == 0 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4) } else { threads };
    let next = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<(usize, Result<(), String>)>();
    let mut results: Vec<Option<Result<(), String>>> = vec![None; total];
    std::thread::scope(|scope| {
        for _ in 0..threads.min(total.max(1)) {
            let tx = tx.clone();
            let (jobs, next, archive) = (&checked, &next, archive.as_ref());
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(job) = jobs.get(i) else { break };
                let bytes = match &job.source {
                    Source::Loose(p) => fs::read(p).map_err(|e| e.to_string()),
                    Source::Archive(idx) => {
                        let a = archive.expect("archive jobs need an archive");
                        a.read(&a.entries()[*idx]).map_err(|e| e.to_string())
                    }
                };
                let r = bytes.and_then(|b| check_asset(&job.ext, &b, job.nis_script));
                if tx.send((i, r)).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        for (n, (i, r)) in rx.iter().enumerate() {
            results[i] = Some(r);
            if n % 256 == 0 || n + 1 == total {
                progress(&Progress::Assets { done: n + 1, total });
            }
        }
    });
    for (job, r) in checked.iter().zip(results) {
        let label = match job.source {
            Source::Loose(_) => format!("{OVERRIDES_DIR}\\{}", job.name),
            Source::Archive(_) => job.name.clone(),
        };
        report.record(&job.ext, &label, r.unwrap_or_else(|| Err("not checked (worker stopped)".into())));
    }

    check_streams(dir, manifest, archive.as_ref(), &overrides, report);
    check_missions(dir, manifest, report);
    Ok(())
}

/// Reads an archive-namespace file the way the engine would (override first).
fn read_named(archive: Option<&BigArchive>, overrides: &std::collections::HashMap<String, PathBuf>, name: &str) -> Result<Vec<u8>, String> {
    if let Some(p) = overrides.get(&normalize_name(name)) {
        return fs::read(p).map_err(|e| e.to_string());
    }
    match archive.map(|a| a.read_file(name)) {
        Some(Ok(Some(b))) => Ok(b),
        Some(Ok(None)) | None => Err(format!("{name} not found")),
        Some(Err(e)) => Err(e.to_string()),
    }
}

/// The music and speech banks, resolved against their indices in the archive.
fn check_streams(
    dir: &Path,
    manifest: &Manifest,
    archive: Option<&BigArchive>,
    overrides: &std::collections::HashMap<String, PathBuf>,
    report: &mut VerifyReport,
) {
    for f in manifest.files.iter().filter(|f| matches!(f.kind, FileKind::Music | FileKind::Speech)) {
        let path = f.path.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
        let file_name = f.path.rsplit('/').next().unwrap_or(&f.path);
        let result = fs::read(&path).map_err(|e| e.to_string()).and_then(|bytes| {
            if f.kind == FileKind::Music {
                let index = read_named(archive, overrides, "soundfx\\music4cat.wxh")?;
                let index = audio::MusicIndex::parse(&index).map_err(|e| format!("music4cat.wxh: {e}"))?;
                let tracks = audio::music_tracks(&index, &bytes).map_err(|e| e.to_string())?;
                if tracks.is_empty() {
                    return Err("no music tracks".into());
                }
                Ok(())
            } else {
                let bank = audio::SpeechBank::parse(&bytes).map_err(|e| e.to_string())?;
                let race = if file_name.eq_ignore_ascii_case(install::BEAST_SPEECH) { "beast" } else { "sect" };
                let lut = read_named(archive, overrides, &format!("soundfx\\{race}phrase.lut"))?;
                let audio::Lut::Phrase(phrases) = audio::Lut::parse(&lut).map_err(|e| format!("{race}phrase.lut: {e}"))? else {
                    return Err(format!("{race}phrase.lut is not a phrase table"));
                };
                let missing = phrases
                    .phrases
                    .iter()
                    .filter(|p| !p.placeholder)
                    .flat_map(|p| p.offsets.iter())
                    .filter(|&&o| bank.find(o as usize).is_none())
                    .count();
                if missing > 0 {
                    return Err(format!("{missing} phrase offsets from {race}phrase.lut are not fragments in the bank"));
                }
                Ok(())
            }
        });
        report.record(&extension(file_name), &f.path, result);
    }
}

fn check_missions(dir: &Path, manifest: &Manifest, report: &mut VerifyReport) {
    for m in &manifest.missions.missions {
        let Some(rel) = &m.path else { continue };
        let path = rel.split('/').fold(dir.to_path_buf(), |p, part| p.join(part));
        let result = fs::read(&path).map_err(|e| e.to_string()).and_then(|b| {
            let mission = hwc_kas::binary::decode(&b).map_err(|e| e.message)?;
            if !mission.name.eq_ignore_ascii_case(&m.name) {
                return Err(format!("contains mission {:?}", mission.name));
            }
            Ok(())
        });
        report.record("kasb", rel, result);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions() {
        assert_eq!(extension("race_sect\\smothership\\rl0\\lod0\\smothership.GEO"), "geo");
        assert_eq!(extension("dir.d\\noext"), "");
        assert_eq!(stem_path("nis\\n01.script"), "nis\\n01");
        assert!(is_checked("peo") && is_checked("shp") && !is_checked("jpg"));
    }

    #[test]
    fn garbage_fails_binary_parsers() {
        for ext in ["lif", "geo", "mex", "mad", "btg", "hff", "fib", "hsf", "nis", "bnk", "lut", "wxh"] {
            assert!(check_asset(ext, b"garbage!garbage!", false).is_err(), "{ext} accepted garbage");
        }
        assert!(check_asset("shp", b"\x01\x02binary", false).is_err());
        assert!(check_asset("shp", b"Key 1\n", false).is_ok());
        assert!(check_asset("jpg", b"anything", false).is_ok());
    }
}
