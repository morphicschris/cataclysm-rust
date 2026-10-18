//! Command-line utilities for inspecting original game data.

use std::path::{Component, Path, PathBuf};
use std::process::ExitCode;

use hwc_formats::big::BigArchive;

const USAGE: &str = "\
usage: hwc-tools <command> ...

commands:
  big-list <archive.big>                         list entries (name, real size, stored size, compressed)
  big-extract <archive.big> <outdir> [filter]    extract entries whose name contains [filter]
                                                 (case-insensitive, / or \\ accepted)";

type CmdResult = Result<(), Box<dyn std::error::Error>>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["big-list", big] => big_list(Path::new(big)),
        ["big-extract", big, out] => big_extract(Path::new(big), Path::new(out), None),
        ["big-extract", big, out, filter] => big_extract(Path::new(big), Path::new(out), Some(filter)),
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn big_list(path: &Path) -> CmdResult {
    use std::io::Write;
    let big = BigArchive::open(path)?;
    let stdout = std::io::stdout();
    let mut out = std::io::BufWriter::new(stdout.lock());
    let (mut real, mut stored) = (0u64, 0u64);
    for e in big.entries() {
        let flag = if e.compressed { "lzss" } else { "-" };
        // Ignore broken pipes (e.g. piping into `head`).
        if writeln!(out, "{}\t{}\t{}\t{}", e.name, e.real_len, e.stored_len, flag).is_err() {
            return Ok(());
        }
        real += u64::from(e.real_len);
        stored += u64::from(e.stored_len);
    }
    let _ = out.flush();
    eprintln!("{} entries, {real} bytes real, {stored} bytes stored", big.entries().len());
    Ok(())
}

fn big_extract(path: &Path, out_dir: &Path, filter: Option<&str>) -> CmdResult {
    let big = BigArchive::open(path)?;
    let filter = filter.map(hwc_formats::big::normalize_name);
    let mut count = 0usize;
    for e in big.entries() {
        if let Some(f) = &filter {
            if !hwc_formats::big::normalize_name(&e.name).contains(f.as_str()) {
                continue;
            }
        }
        let rel = safe_relative_path(&e.name)
            .ok_or_else(|| format!("refusing to extract unsafe path {:?}", e.name))?;
        let dest = out_dir.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let data = big.read(e)?;
        std::fs::write(&dest, data).map_err(|err| format!("{}: {err}", dest.display()))?;
        count += 1;
    }
    eprintln!("extracted {count} files to {}", out_dir.display());
    Ok(())
}

/// Turns an archive name into a relative path, rejecting absolute paths and `..` components.
fn safe_relative_path(name: &str) -> Option<PathBuf> {
    let mut path = PathBuf::new();
    for part in name.split(['\\', '/']).filter(|p| !p.is_empty() && *p != ".") {
        let mut comps = Path::new(part).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(c)), None) => path.push(c),
            _ => return None,
        }
    }
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::safe_relative_path;
    use std::path::PathBuf;

    #[test]
    fn paths() {
        assert_eq!(safe_relative_path("a\\b/c.txt"), Some(PathBuf::from("a").join("b").join("c.txt")));
        assert_eq!(safe_relative_path("..\\evil"), None);
        assert_eq!(safe_relative_path("C:\\evil"), None);
        assert_eq!(safe_relative_path(""), None);
    }
}
