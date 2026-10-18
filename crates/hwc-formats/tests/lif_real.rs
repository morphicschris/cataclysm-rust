//! Parses every `.lif` in an extracted copy of the game archive (`HWC_EXTRACTED_DIR`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hwc_formats::lif::{Lif, PixelFormat};

fn files_with_ext(dir: &Path, ext: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            files_with_ext(&path, ext, out);
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case(ext))
        {
            out.push(path);
        }
    }
}

#[test]
fn parse_all_lif() {
    let Some(root) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipping");
        return;
    };
    let mut files = Vec::new();
    files_with_ext(Path::new(&root), "lif", &mut files);
    let mut failures = Vec::new();
    let mut kinds: BTreeMap<(String, u32), usize> = BTreeMap::new();
    for path in &files {
        let data = std::fs::read(path).unwrap();
        match Lif::parse(&data) {
            Ok(lif) => {
                assert!(lif.width > 0 && lif.height > 0, "{}", path.display());
                let n = lif.pixel_count();
                assert_eq!(lif.to_rgba().len(), n * 4, "{}", path.display());
                assert_eq!(lif.to_rgba_team([255, 0, 0], [0, 0, 255]).len(), n * 4);
                for which in 0..2 {
                    if let Some(mask) = lif.team_mask(which) {
                        assert_eq!(mask.len(), n, "{}", path.display());
                    }
                }
                let fmt = match lif.format {
                    PixelFormat::Indexed => "indexed",
                    PixelFormat::Rgba => "rgba",
                };
                *kinds.entry((fmt.to_string(), lif.flags)).or_default() += 1;
            }
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    eprintln!("lif: {} files, {} failed", files.len(), failures.len());
    for ((fmt, flags), n) in &kinds {
        eprintln!("  {fmt:8} flags {flags:#06x}: {n}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
