//! `hwc-import <original install dir> <output data dir> [--verify] [--force]`

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use hwc_import::{import, verify, ImportOptions, Progress, Stage, VerifyOptions, VerifyReport};

const USAGE: &str = "\
usage: hwc-import <original install dir> <output data dir> [--verify] [--force] [--hwr <dir>]
       hwc-import --verify-only <output data dir>
       hwc-import --hwr <Homeworld Remastered install dir> <output data dir>

  Builds this game's data directory from your own Homeworld: Cataclysm install. The install is
  only read, never modified. Re-running only copies what changed.

  --verify       after importing, re-hash every output and parse-check every asset
  --force        re-copy everything; allow a non-empty output folder without a manifest
  --verify-only  verify an existing data directory without importing
  --hwr <dir>    also import the optional Homeworld Remastered explosions and engine trails from
                 your own Remastered install (steamapps/common/Homeworld) into hwr/
  -q, --quiet    no progress output";

struct Printer {
    quiet: bool,
    last_line: Instant,
    pending_newline: bool,
}

impl Printer {
    fn line(&mut self, s: &str) {
        if self.pending_newline {
            eprintln!();
            self.pending_newline = false;
        }
        eprintln!("{s}");
    }

    fn handle(&mut self, p: &Progress) {
        if self.quiet {
            if let Progress::Warning(w) = p {
                self.line(&format!("warning: {w}"));
            }
            return;
        }
        match *p {
            Progress::Stage(s) => self.line(match s {
                Stage::Detect => "Checking install...",
                Stage::Copy => "Copying files...",
                Stage::Codecs => "Extracting Bink codec tables...",
                Stage::Missions => "Extracting missions...",
                Stage::Finish => "Writing manifest...",
                Stage::VerifyHashes => "Verifying file hashes...",
                Stage::VerifyAssets => "Parse-checking assets...",
            }),
            Progress::FileStarted { path, size } => self.line(&format!("  {path} ({:.1} MB)", size as f64 / 1e6)),
            Progress::FileSkipped { path } => self.line(&format!("  {path} (up to date)")),
            Progress::Bytes { done, total } if self.last_line.elapsed().as_millis() < 250 && done < total => {}
            Progress::Bytes { done, total } => {
                eprint!("\r    {:.0} / {:.0} MB", done as f64 / 1e6, total as f64 / 1e6);
                let _ = std::io::stderr().flush();
                self.pending_newline = true;
                self.last_line = Instant::now();
            }
            Progress::Assets { done, total } => {
                if self.last_line.elapsed().as_millis() >= 250 || done == total {
                    eprint!("\r    {done} / {total} assets");
                    let _ = std::io::stderr().flush();
                    self.pending_newline = true;
                    self.last_line = Instant::now();
                }
            }
            Progress::Warning(w) => self.line(&format!("warning: {w}")),
        }
    }
}

fn print_verify(r: &VerifyReport) {
    println!(
        "verify: {} files re-hashed ({:.1} MB), {} assets parsed in {:.1}s, {} overrides",
        r.files_hashed,
        r.bytes_hashed as f64 / 1e6,
        r.assets_checked(),
        r.elapsed.as_secs_f64(),
        r.overrides
    );
    let kinds: Vec<String> = r
        .by_kind
        .iter()
        .map(|(k, c)| if c.failed == 0 { format!("{k} {}", c.ok) } else { format!("{k} {}/{} FAILED {}", c.ok, c.ok + c.failed, c.failed) })
        .collect();
    println!("  {}", kinds.join(", "));
    let unchecked: usize = r.unchecked.values().sum();
    if unchecked > 0 {
        let list: Vec<String> = r.unchecked.iter().map(|(k, n)| format!("{k} {n}")).collect();
        println!("  not parse-checked (no parser): {unchecked} ({})", list.join(", "));
    }
    for (p, e) in &r.hash_failures {
        println!("  HASH  {p}: {e}");
    }
    for (p, e) in &r.parse_failures {
        println!("  PARSE {p}: {e}");
    }
    for w in &r.warnings {
        println!("  warning: {w}");
    }
    println!("verify: {}", if r.ok() { "OK" } else { "FAILED" });
}

fn main() -> ExitCode {
    let mut positional: Vec<PathBuf> = Vec::new();
    let (mut do_verify, mut force, mut verify_only, mut quiet) = (false, false, false, false);
    let mut hwr: Option<PathBuf> = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--hwr") => match args.next() {
                Some(v) => hwr = Some(PathBuf::from(v)),
                None => {
                    eprintln!("--hwr wants the Homeworld Remastered install folder\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            Some("--verify") => do_verify = true,
            Some("--force") => force = true,
            Some("--verify-only") => verify_only = true,
            Some("-q" | "--quiet") => quiet = true,
            Some("-h" | "--help") => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            Some(s) if s.starts_with('-') => {
                eprintln!("unknown option {s}\n\n{USAGE}");
                return ExitCode::from(2);
            }
            _ => positional.push(PathBuf::from(arg)),
        }
    }
    let mut printer = Printer { quiet, last_line: Instant::now(), pending_newline: false };

    let out_dir = match (verify_only, positional.as_slice()) {
        (true, [out]) => out.clone(),
        (false, [out]) if hwr.is_some() => out.clone(),
        (false, [install, out]) => {
            match import(install, out, ImportOptions { force }, &mut |p| printer.handle(p)) {
                Ok(r) => {
                    printer.line("");
                    println!(
                        "import: {} copied ({:.1} MB), {} up to date, {} removed; missions: {} ({}/{} extracted{}); \
                         data directory {:.1} MB; {:.1}s",
                        r.copied.len(),
                        r.bytes_copied as f64 / 1e6,
                        r.skipped.len(),
                        r.removed.len(),
                        r.manifest.missions.status,
                        r.manifest.missions.missions.iter().filter(|m| m.path.is_some()).count(),
                        r.manifest.missions.missions.len(),
                        if r.missions_written { ", updated" } else { "" },
                        r.bytes_total as f64 / 1e6,
                        r.elapsed.as_secs_f64()
                    );
                    for e in &r.manifest.missions.errors {
                        println!("  mission error: {e}");
                    }
                    match &r.manifest.exe.known_version {
                        Some(v) => println!("executable: {v}"),
                        None => println!("executable: unrecognised build (sha256 {})", r.manifest.exe.sha256),
                    }
                }
                Err(e) => {
                    printer.line("");
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
            }
            out.clone()
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };

    if let Some(src) = &hwr {
        let dest = out_dir.join(hwc_hwr::import::DATA_DIR);
        printer.line("Importing Homeworld Remastered effects...");
        match hwc_hwr::import::import(src, &dest) {
            Ok(r) => {
                println!(
                    "hwr: {} ships, {} effects, {} textures ({:.1} MB) into {}",
                    r.ships,
                    r.effects,
                    r.textures,
                    r.bytes as f64 / 1e6,
                    dest.display()
                );
                if !r.missing.is_empty() {
                    println!("  effects named but not in the archive: {}", r.missing.join(", "));
                }
            }
            Err(e) => {
                eprintln!("error: Homeworld Remastered import: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    if do_verify || verify_only {
        match verify(&out_dir, VerifyOptions::default(), &mut |p| printer.handle(p)) {
            Ok(r) => {
                printer.line("");
                print_verify(&r);
                if !r.ok() {
                    return ExitCode::FAILURE;
                }
            }
            Err(e) => {
                eprintln!("error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}
