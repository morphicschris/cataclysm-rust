//! Mission extraction from the player's `cataclysm.exe` into `missions/<name>.kasb`.
//!
//! The missions are compiled into the executable, so the importer rebuilds their IR from it
//! with `hwc_kas::extract` and caches it in the compact binary form (`hwc_kas::binary`).

use hwc_kas::extract::{canonical_name, extract_each, find_missions, PeImage};

/// Identifies the extractor; stored in the manifest so that a newer extractor re-extracts.
/// Bump the suffix whenever the produced IR can change.
pub const EXTRACTOR: &str = concat!("hwc-kas-extract/", env!("CARGO_PKG_VERSION"), "+ir1");

/// What the extractor produced.
#[derive(Debug, Default)]
pub struct Extraction {
    /// Every mission found in the executable, in discovery order, with its encoded IR
    /// (`hwc_kas::binary::encode`) or `None` when that mission failed to lower.
    pub missions: Vec<(String, Option<Vec<u8>>)>,
    pub errors: Vec<String>,
}

impl Extraction {
    pub fn extracted(&self) -> usize {
        self.missions.iter().filter(|m| m.1.is_some()).count()
    }
}

/// Locates the missions and lowers each one to IR.
pub fn extract(exe: &[u8]) -> Extraction {
    let mut out = Extraction::default();
    match extract_each(exe) {
        Ok(results) => {
            for r in results {
                match r {
                    Ok(m) => {
                        let bytes = hwc_kas::binary::encode(&m);
                        out.missions.push((m.name.clone(), Some(bytes)));
                    }
                    Err(e) => {
                        out.missions.push((e.mission.clone().unwrap_or_else(|| "?".into()), None));
                        out.errors.push(e.to_string());
                    }
                }
            }
        }
        Err(e) => {
            out.errors.push(format!("cataclysm.exe: {e}"));
            // Still record which missions exist, when the image can be read at all.
            if let Ok(pe) = PeImage::parse(exe) {
                out.missions = find_missions(&pe).into_iter().map(|m| (canonical_name(&m.name), None)).collect();
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_exe_reports_error() {
        let e = extract(b"not an exe");
        assert!(e.missions.is_empty());
        assert!(!e.errors.is_empty());
    }
}
