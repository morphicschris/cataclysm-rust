//! Building and refreshing a data directory.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::hash;
use crate::install::{self, FileKind, Install};
use crate::manifest::{
    now_unix, ExeRecord, FileRecord, Manifest, MissionRecord, MissionsRecord, Mtime, SourceRecord, FORMAT,
    MANIFEST_FILE, MANIFEST_VERSION,
};
use crate::missions;
use crate::{ImportError, Progress, Stage};

pub use hwc_formats::datadir::{MISSIONS_DIR, OVERRIDES_DIR};
/// Top-level folders and files the importer owns (and may delete stale outputs from).
const MANAGED: [&str; 6] = ["base", "audio", "movies", "config", MISSIONS_DIR, hwc_video::TABLES_FILE_NAME];

#[derive(Debug, Clone, Copy, Default)]
pub struct ImportOptions {
    /// Re-copy and re-extract everything, and allow importing into a non-empty folder that has
    /// no manifest.
    pub force: bool,
}

#[derive(Debug, Clone)]
pub struct ImportReport {
    pub manifest: Manifest,
    /// Data-directory paths written in this run.
    pub copied: Vec<String>,
    /// Paths that were already up to date.
    pub skipped: Vec<String>,
    /// Stale paths from an earlier import that were deleted.
    pub removed: Vec<String>,
    pub bytes_copied: u64,
    /// Size of every file in the data directory listed by the manifest.
    pub bytes_total: u64,
    /// Whether the mission IR was (re)written in this run.
    pub missions_written: bool,
    pub warnings: Vec<String>,
    pub elapsed: Duration,
}

/// Absolute, normalised form of a path that may not exist yet: the deepest existing ancestor is
/// canonicalised and the rest appended.
fn absolute(path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().map(|d| d.join(path)).unwrap_or_else(|_| path.to_path_buf())
    };
    let mut existing = path.as_path();
    let mut rest = Vec::new();
    loop {
        if let Ok(c) = existing.canonicalize() {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return out;
        }
        match (existing.parent(), existing.file_name()) {
            (Some(p), Some(n)) => {
                rest.push(n.to_os_string());
                existing = p;
            }
            _ => return path,
        }
    }
}

/// A path for humans: without Windows' `\\?\` verbatim prefix that canonicalisation adds.
fn display_path(p: &Path) -> String {
    let s = p.to_string_lossy();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else {
        s.strip_prefix(r"\\?\").unwrap_or(&s).to_owned()
    }
}

fn lower_components(p: &Path) -> Vec<String> {
    p.components()
        .filter(|c| !matches!(c, Component::CurDir))
        .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

/// True when `inner` is `outer` or inside it (case-insensitively, which errs on the safe side).
fn contains(outer: &Path, inner: &Path) -> bool {
    let (o, i) = (lower_components(outer), lower_components(inner));
    i.len() >= o.len() && i[..o.len()] == o[..]
}

fn check_output_location(install: &Path, out: &Path) -> Result<(), ImportError> {
    let (install, out_abs) = (absolute(install), absolute(out));
    if contains(&install, &out_abs) {
        return Err(ImportError::UnsafeOutput {
            dir: out.to_path_buf(),
            reason: "it is inside the original install, which is never written to".into(),
        });
    }
    if contains(&out_abs, &install) {
        return Err(ImportError::UnsafeOutput { dir: out.to_path_buf(), reason: "it contains the original install".into() });
    }
    Ok(())
}

fn to_native(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

fn partial_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    path.with_file_name(name)
}

/// Writes `bytes` to `path` via a temporary file and a rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), ImportError> {
    let tmp = partial_path(path);
    fs::write(&tmp, bytes).map_err(ImportError::io(&tmp))?;
    fs::rename(&tmp, path).map_err(ImportError::io(path))
}

/// Builds `out_dir` from the install at `install_dir`, or brings an earlier import up to date.
///
/// The install is only read. Files whose source size and mtime match the manifest, and whose
/// output still has the recorded size and mtime, are skipped; everything else is copied through
/// a `.partial` file and renamed into place, so an interrupted import never leaves a truncated
/// file under its final name. The manifest is written last.
pub fn import(
    install_dir: &Path,
    out_dir: &Path,
    opts: ImportOptions,
    progress: &mut dyn FnMut(&Progress),
) -> Result<ImportReport, ImportError> {
    let start = Instant::now();
    progress(&Progress::Stage(Stage::Detect));
    let install = install::detect(install_dir)?;
    check_output_location(install_dir, out_dir)?;
    let mut warnings = install.warnings.clone();
    for w in &warnings {
        progress(&Progress::Warning(w));
    }

    // Previous state.
    let previous = if out_dir.exists() {
        match Manifest::load(out_dir) {
            Ok(Some(m)) if m.manifest_version == MANIFEST_VERSION => Some(m),
            Ok(Some(m)) => {
                let w = format!("manifest version {} is not {MANIFEST_VERSION}; re-importing everything", m.manifest_version);
                progress(&Progress::Warning(&w));
                warnings.push(w);
                None
            }
            Ok(None) => {
                let non_empty = fs::read_dir(out_dir).map_err(ImportError::io(out_dir))?.next().is_some();
                if non_empty && !opts.force {
                    return Err(ImportError::OutputNotEmpty(out_dir.to_path_buf()));
                }
                None
            }
            Err(e) if opts.force => {
                let w = format!("ignoring unreadable manifest: {e}");
                progress(&Progress::Warning(&w));
                warnings.push(w);
                None
            }
            Err(e) => return Err(ImportError::Io { path: out_dir.join(MANIFEST_FILE), source: e }),
        }
    } else {
        None
    };
    let previous = if opts.force { None } else { previous };
    fs::create_dir_all(out_dir).map_err(ImportError::io(out_dir))?;

    // Copy.
    progress(&Progress::Stage(Stage::Copy));
    let total = install.total_bytes();
    let mut done = 0u64;
    let mut records = Vec::new();
    let (mut copied, mut skipped) = (Vec::new(), Vec::new());
    let mut bytes_copied = 0u64;
    for src in &install.files {
        let meta = src.path.metadata().map_err(ImportError::io(&src.path))?;
        let src_mtime = Mtime::of(&meta);
        let dest = to_native(out_dir, &src.dest);
        let old = previous.as_ref().and_then(|m| m.file(&src.dest));
        let up_to_date = old.is_some_and(|o| {
            o.source.size == meta.len()
                && o.source.mtime == src_mtime
                && o.source.path.eq_ignore_ascii_case(&src.rel)
                && dest.metadata().is_ok_and(|d| d.len() == o.size && Mtime::of(&d) == o.mtime)
        });
        if let (true, Some(old)) = (up_to_date, old) {
            progress(&Progress::FileSkipped { path: &src.dest });
            done += meta.len();
            progress(&Progress::Bytes { done, total });
            skipped.push(src.dest.clone());
            records.push(old.clone());
            continue;
        }
        progress(&Progress::FileStarted { path: &src.dest, size: meta.len() });
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent).map_err(ImportError::io(parent))?;
        }
        let tmp = partial_path(&dest);
        let (size, sha256) = hash::copy_hashed(&src.path, &tmp, &mut |n| {
            done += n;
            progress(&Progress::Bytes { done, total });
        })
        .map_err(ImportError::io(&src.path))?;
        fs::rename(&tmp, &dest).map_err(ImportError::io(&dest))?;
        let out_meta = dest.metadata().map_err(ImportError::io(&dest))?;
        bytes_copied += size;
        copied.push(src.dest.clone());
        records.push(FileRecord {
            path: src.dest.clone(),
            kind: src.kind,
            size,
            sha256: sha256.clone(),
            mtime: Mtime::of(&out_meta),
            source: SourceRecord { path: src.rel.clone(), size: meta.len(), mtime: src_mtime, sha256 },
        });
    }

    // Bink codec tables, from the install's binkw32.dll.
    progress(&Progress::Stage(Stage::Codecs));
    if let Some((record, written)) = import_bink_tables(&install, out_dir, previous.as_ref(), &mut warnings, progress)? {
        if written {
            bytes_copied += record.size;
            copied.push(record.path.clone());
        } else {
            skipped.push(record.path.clone());
        }
        records.push(record);
    }
    records.sort_by(|a, b| a.path.cmp(&b.path));

    // Missions.
    progress(&Progress::Stage(Stage::Missions));
    let (missions, missions_written) = import_missions(&install, out_dir, previous.as_ref(), &mut warnings, progress)?;

    // Remove outputs of an earlier import that are no longer produced.
    progress(&Progress::Stage(Stage::Finish));
    let mut removed = Vec::new();
    if let Some(prev) = &previous {
        let keep: Vec<&str> = records
            .iter()
            .map(|r| r.path.as_str())
            .chain(missions.missions.iter().filter_map(|m| m.path.as_deref()))
            .collect();
        let old_paths = prev.files.iter().map(|f| f.path.as_str()).chain(prev.missions.missions.iter().filter_map(|m| m.path.as_deref()));
        for old in old_paths {
            let managed = MANAGED.iter().any(|d| old.split('/').next() == Some(d)) && !old.split('/').any(|p| p == "..");
            if managed && !keep.contains(&old) {
                let p = to_native(out_dir, old);
                if p.is_file() {
                    fs::remove_file(&p).map_err(ImportError::io(&p))?;
                }
                removed.push(old.to_owned());
            }
        }
    }
    let overrides = out_dir.join(OVERRIDES_DIR);
    fs::create_dir_all(&overrides).map_err(ImportError::io(&overrides))?;

    let exe = ExeRecord {
        path: install.exe_path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        size: install.exe.size,
        sha256: install.exe.sha256.clone(),
        known_version: install.exe.known.map(|k| k.id.to_owned()),
    };
    let changed = !copied.is_empty() || !removed.is_empty() || missions_written;
    let manifest = Manifest {
        format: FORMAT.into(),
        manifest_version: MANIFEST_VERSION,
        importer_version: env!("CARGO_PKG_VERSION").into(),
        updated_unix: match (&previous, changed) {
            (Some(p), false) => p.updated_unix,
            _ => now_unix(),
        },
        install_dir: display_path(&absolute(install_dir)),
        exe,
        files: records,
        missions,
        overrides_dir: OVERRIDES_DIR.into(),
    };
    if previous.as_ref() != Some(&manifest) {
        manifest.save(out_dir).map_err(ImportError::io(out_dir.join(MANIFEST_FILE)))?;
    }
    let bytes_total = manifest.files.iter().map(|f| f.size).sum::<u64>()
        + manifest.missions.missions.iter().filter_map(|m| m.size).sum::<u64>();
    Ok(ImportReport {
        manifest,
        copied,
        skipped,
        removed,
        bytes_copied,
        bytes_total,
        missions_written,
        warnings,
        elapsed: start.elapsed(),
    })
}

fn import_missions(
    install: &Install,
    out_dir: &Path,
    previous: Option<&Manifest>,
    warnings: &mut Vec<String>,
    progress: &mut dyn FnMut(&Progress),
) -> Result<(MissionsRecord, bool), ImportError> {
    // Reference digests are re-checked on every run (cheap), so the warning is not lost when the
    // extraction itself is skipped.
    let check = |record: &mut MissionsRecord, warnings: &mut Vec<String>, progress: &mut dyn FnMut(&Progress)| {
        match missions::check_reference(install.exe.known, record) {
            Ok(()) => record.verified_exe = true,
            Err(problems) => {
                record.verified_exe = false;
                // An unknown build was already reported by `detect`.
                if install.exe.known.is_some() {
                    for w in problems {
                        progress(&Progress::Warning(&w));
                        warnings.push(w);
                    }
                }
            }
        }
    };
    if let Some(prev) = previous.map(|m| &m.missions) {
        let outputs_intact = prev.missions.iter().all(|m| match (&m.path, m.size) {
            (Some(p), Some(size)) => to_native(out_dir, p).metadata().is_ok_and(|d| d.len() == size),
            _ => true,
        });
        if prev.extractor == missions::EXTRACTOR && prev.exe_sha256 == install.exe.sha256 && outputs_intact {
            for m in &prev.missions {
                if let Some(p) = &m.path {
                    progress(&Progress::FileSkipped { path: p });
                }
            }
            let mut record = prev.clone();
            check(&mut record, warnings, progress);
            return Ok((record, false));
        }
    }

    let exe_bytes = fs::read(&install.exe_path).map_err(ImportError::io(&install.exe_path))?;
    let extraction = missions::extract(&exe_bytes);
    for e in &extraction.errors {
        let w = format!("mission extraction: {e}");
        progress(&Progress::Warning(&w));
        warnings.push(w);
    }
    let dir = out_dir.join(MISSIONS_DIR);
    fs::create_dir_all(&dir).map_err(ImportError::io(&dir))?;
    let mut records = Vec::new();
    for (name, ir) in &extraction.missions {
        let Some(bytes) = ir else {
            records.push(MissionRecord { name: name.clone(), path: None, size: None, sha256: None });
            continue;
        };
        let rel = format!("{MISSIONS_DIR}/{}.kasb", name.to_ascii_lowercase());
        progress(&Progress::FileStarted { path: &rel, size: bytes.len() as u64 });
        write_atomic(&to_native(out_dir, &rel), bytes)?;
        records.push(MissionRecord {
            name: name.clone(),
            path: Some(rel),
            size: Some(bytes.len() as u64),
            sha256: Some(hash::sha256_bytes(bytes)),
        });
    }
    let status = match (extraction.extracted(), extraction.missions.len()) {
        (0, _) => "failed",
        (n, total) if n == total && extraction.errors.is_empty() => "extracted",
        _ => "partial",
    };
    let mut record = MissionsRecord {
        status: status.into(),
        extractor: missions::EXTRACTOR.into(),
        exe_sha256: install.exe.sha256.clone(),
        verified_exe: false,
        missions: records,
        errors: extraction.errors,
    };
    check(&mut record, warnings, progress);
    Ok((record, true))
}

/// Extracts the Bink codec tables from `binkw32.dll` into `bink_tables.bin`. Returns the
/// manifest record and whether it was (re)written, or `None` (after a warning) when the tables
/// are unavailable: the rest of the import does not depend on them, only movie playback does.
fn import_bink_tables(
    install: &Install,
    out_dir: &Path,
    previous: Option<&Manifest>,
    warnings: &mut Vec<String>,
    progress: &mut dyn FnMut(&Progress),
) -> Result<Option<(FileRecord, bool)>, ImportError> {
    let Some(src) = &install.binkw32 else { return Ok(None) };
    let mut warn = |w: String, progress: &mut dyn FnMut(&Progress)| {
        progress(&Progress::Warning(&w));
        warnings.push(w);
    };
    let dll = match fs::read(&src.path) {
        Ok(b) => b,
        Err(e) => {
            warn(format!("{}: {e}; movies will be unavailable", src.rel), progress);
            return Ok(None);
        }
    };
    let meta = src.path.metadata().map_err(ImportError::io(&src.path))?;
    let source = SourceRecord { path: src.rel.clone(), size: dll.len() as u64, mtime: Mtime::of(&meta), sha256: hash::sha256_bytes(&dll) };
    let dest = to_native(out_dir, &src.dest);

    let old = previous.and_then(|m| m.file(&src.dest)).filter(|o| o.kind == FileKind::CodecTables);
    if let Some(old) = old {
        let up_to_date = old.source.sha256 == source.sha256
            && old.source.path.eq_ignore_ascii_case(&source.path)
            && dest.metadata().is_ok_and(|d| d.len() == old.size && Mtime::of(&d) == old.mtime)
            && hwc_video::BinkTables::load(&dest).is_ok();
        if up_to_date {
            progress(&Progress::FileSkipped { path: &src.dest });
            // The record (and so the manifest) stays unchanged even if only the DLL's mtime moved.
            return Ok(Some((old.clone(), false)));
        }
    }

    let tables = match hwc_video::import_tables(&dll) {
        Ok(t) => t,
        Err(e) => {
            warn(format!("{}: could not extract the Bink codec tables ({e}); movies will be unavailable", src.rel), progress);
            return Ok(None);
        }
    };
    progress(&Progress::FileStarted { path: &src.dest, size: tables.len() as u64 });
    write_atomic(&dest, &tables)?;
    let out_meta = dest.metadata().map_err(ImportError::io(&dest))?;
    Ok(Some((
        FileRecord {
            path: src.dest.clone(),
            kind: FileKind::CodecTables,
            size: tables.len() as u64,
            sha256: hash::sha256_bytes(&tables),
            mtime: Mtime::of(&out_meta),
            source,
        },
        true,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn containment() {
        assert!(contains(Path::new("C:/Games/HWC"), Path::new("C:/Games/HWC/data")));
        assert!(contains(Path::new("C:/Games/HWC"), Path::new("c:/games/hwc")));
        assert!(!contains(Path::new("C:/Games/HWC"), Path::new("C:/Games/HWC2")));
        assert!(!contains(Path::new("C:/Games/HWC/data"), Path::new("C:/Games/HWC")));
    }

    #[test]
    fn refuses_output_inside_install() {
        let base = std::env::temp_dir().join(format!("hwc-import-loc-{}", std::process::id()));
        fs::create_dir_all(&base).unwrap();
        let err = check_output_location(&base, &base.join("sub/data")).unwrap_err();
        assert!(matches!(err, ImportError::UnsafeOutput { .. }));
        let err = check_output_location(&base.join("inst"), &base).unwrap_err();
        assert!(matches!(err, ImportError::UnsafeOutput { .. }));
        assert!(check_output_location(&base.join("inst"), &base.join("out")).is_ok());
        fs::remove_dir_all(&base).ok();
    }
}
