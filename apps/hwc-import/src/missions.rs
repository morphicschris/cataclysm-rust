//! Mission extraction from the player's `cataclysm.exe` into `missions/<name>.kasb`.
//!
//! The missions are compiled into the executable, so the importer rebuilds their IR from it
//! with `hwc_kas::extract` and caches it in the compact binary form (`hwc_kas::binary`).

use hwc_kas::extract::{canonical_name, extract_each, find_missions, PeImage};

use crate::install::KnownExe;
use crate::manifest::MissionsRecord;

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

/// Checks extracted missions against the reference digests of a known executable.
///
/// `Ok(())` means every mission of the reference was extracted with the same IR digest and no
/// other mission appeared. `Err` lists what could not be confirmed (unknown build, no reference
/// for this extractor version, missing, extra or differing missions); callers report these as
/// warnings and record the missions as unverified.
pub fn check_reference(known: Option<&KnownExe>, missions: &MissionsRecord) -> Result<(), Vec<String>> {
    let Some(known) = known else {
        return Err(vec!["missions: the executable is not a known build, so the extracted IR is unverified".into()]);
    };
    let Some(reference) = known.missions else {
        return Err(vec![format!("missions: no reference digests for {}; the extracted IR is unverified", known.id)]);
    };
    if reference.extractor != missions.extractor {
        return Err(vec![format!(
            "missions: the reference digests for {} were taken with extractor {}, these missions with {}; the extracted IR is unverified",
            known.id, reference.extractor, missions.extractor
        )]);
    }
    let mut problems = Vec::new();
    for (name, digest) in reference.missions {
        match missions.missions.iter().find(|m| m.name.eq_ignore_ascii_case(name)) {
            None => problems.push(format!("missions: {name} was not found in the executable")),
            Some(m) => match &m.sha256 {
                None => problems.push(format!("missions: {name} failed to extract")),
                Some(h) if !h.eq_ignore_ascii_case(digest) => {
                    problems.push(format!("missions: {name} IR sha256 {h} differs from the reference {digest} for {}", known.id))
                }
                Some(_) => {}
            },
        }
    }
    for m in &missions.missions {
        if !reference.missions.iter().any(|(n, _)| n.eq_ignore_ascii_case(&m.name)) {
            problems.push(format!("missions: {} is not in the reference list for {}", m.name, known.id));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::install::MissionDigests;
    use crate::manifest::MissionRecord;

    fn record(missions: &[(&str, Option<&str>)]) -> MissionsRecord {
        MissionsRecord {
            status: "extracted".into(),
            extractor: EXTRACTOR.into(),
            exe_sha256: "00".into(),
            verified_exe: false,
            missions: missions
                .iter()
                .map(|(n, h)| MissionRecord {
                    name: n.to_string(),
                    path: h.map(|_| format!("missions/{n}.kasb")),
                    size: h.map(|_| 1),
                    sha256: h.map(str::to_owned),
                })
                .collect(),
            errors: vec![],
        }
    }

    #[test]
    fn reference_digests() {
        let known = KnownExe {
            id: "test",
            description: "",
            sha256: "00",
            size: 0,
            missions: Some(MissionDigests { extractor: EXTRACTOR, missions: &[("Mission01", "aa"), ("Mission02", "bb")] }),
        };
        assert!(check_reference(Some(&known), &record(&[("mission01", Some("AA")), ("Mission02", Some("bb"))])).is_ok());
        let e = check_reference(Some(&known), &record(&[("Mission01", Some("ab")), ("Mission03", Some("cc"))])).unwrap_err();
        assert_eq!(e.len(), 3, "{e:?}");
        assert!(e[0].contains("Mission01") && e[0].contains("differs"));
        assert!(e[1].contains("Mission02") && e[1].contains("not found"));
        assert!(e[2].contains("Mission03"));
        let e = check_reference(Some(&known), &record(&[("Mission01", None), ("Mission02", Some("bb"))])).unwrap_err();
        assert_eq!(e, vec!["missions: Mission01 failed to extract".to_string()]);
        let mut other = record(&[("Mission01", Some("aa")), ("Mission02", Some("bb"))]);
        other.extractor = "older".into();
        assert!(check_reference(Some(&known), &other).unwrap_err()[0].contains("extractor"));
        assert!(check_reference(None, &other).is_err());
    }

    #[test]
    fn garbage_exe_reports_error() {
        let e = extract(b"not an exe");
        assert!(e.missions.is_empty());
        assert!(!e.errors.is_empty());
    }
}
