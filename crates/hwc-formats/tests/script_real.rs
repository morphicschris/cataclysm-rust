//! Parses every text tweak file of an extracted `cataclysm.big`. Point `HWC_EXTRACTED_DIR` at the
//! extraction root; the tests skip when it is unset. Run with `--nocapture` to see the survey.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hwc_formats::script::{is_text, Dialect, ScriptFile, ShipKeywordSurvey, ShipStats};

fn extracted_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("HWC_EXTRACTED_DIR")?);
    if dir.is_dir() {
        Some(dir)
    } else {
        eprintln!(
            "HWC_EXTRACTED_DIR={} is not a directory; skipping",
            dir.display()
        );
        None
    }
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

fn extension(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| "(none)".into())
}

fn rel(root: &Path, p: &Path) -> String {
    p.strip_prefix(root)
        .unwrap_or(p)
        .to_string_lossy()
        .replace('\\', "/")
}

#[derive(Default)]
struct ExtStats {
    text: usize,
    binary: usize,
    entries: usize,
    preprocessed: usize,
}

#[test]
fn parse_every_text_file() {
    let Some(root) = extracted_dir() else { return };
    let mut files = Vec::new();
    walk(&root, &mut files);
    let mut by_ext: BTreeMap<String, ExtStats> = BTreeMap::new();
    let mut failures = Vec::new();
    for path in &files {
        let bytes = std::fs::read(path).unwrap();
        let st = by_ext.entry(extension(path)).or_default();
        if !is_text(&bytes) {
            st.binary += 1;
            continue;
        }
        st.text += 1;
        match ScriptFile::parse_bytes(&bytes) {
            Ok(f) => {
                st.entries += f.entry_count();
                if f.dialect() == Dialect::Preprocessed {
                    st.preprocessed += 1;
                }
                if !f.overlong_lines.is_empty() {
                    println!(
                        "overlong lines in {}: {:?}",
                        rel(&root, path),
                        f.overlong_lines
                    );
                }
            }
            Err(e) => failures.push(format!("{}: {e}", rel(&root, path))),
        }
    }
    println!(
        "{:<10} {:>6} {:>6} {:>8} {:>6}",
        "ext", "text", "binary", "entries", "#line"
    );
    for (ext, s) in &by_ext {
        println!(
            "{ext:<10} {:>6} {:>6} {:>8} {:>6}",
            s.text, s.binary, s.entries, s.preprocessed
        );
    }
    assert!(failures.is_empty(), "{failures:#?}");
    // Every tweak-file type must come out as text in full.
    for ext in [
        "script",
        "shp",
        "lod",
        "dist",
        "mission",
        "level",
        "missphere",
        "cred",
        "plug",
        "mif",
    ] {
        let s = by_ext
            .get(ext)
            .unwrap_or_else(|| panic!("no .{ext} files extracted"));
        assert_eq!(s.binary, 0, ".{ext} files detected as binary");
        assert!(s.entries > 0, ".{ext} files produced no entries");
    }
}

#[test]
fn ship_stats_and_keyword_survey() {
    let Some(root) = extracted_dir() else { return };
    let mut files = Vec::new();
    walk(&root, &mut files);
    let shp: Vec<_> = files.iter().filter(|p| extension(p) == "shp").collect();
    assert!(!shp.is_empty());

    let mut survey = ShipKeywordSurvey::default();
    let mut gun_count_mismatch = Vec::new();
    let mut total_guns = 0;
    for path in &shp {
        let name = rel(&root, path);
        let file = ScriptFile::load(path).unwrap_or_else(|e| panic!("{name}: {e}"));
        survey.add(&name, &file);
        let stats = ShipStats::from_script(&file);
        if stats.name.is_empty() {
            println!("no [name] header (harmless to the engine): {name}");
        }
        assert!(stats.lod_file.is_some(), "{name}: no LODFile");
        assert!(stats.maxhealth.is_some(), "{name}: no maxhealth");
        total_guns += stats.guns.len();
        let declared = stats.number_of_guns.unwrap_or(0).max(0) as usize;
        if declared != stats.guns.len() {
            gun_count_mismatch.push(format!(
                "{name}: NUMBER_OF_GUNS {declared}, blocks {}",
                stats.guns.len()
            ));
        }
        if name.ends_with("race_beast/bacolyte.shp") {
            assert_eq!(stats.name, "bAcolyte");
            assert_eq!(stats.mass, Some(10.0));
            assert_eq!(stats.maxvelocity, Some(800.0));
            assert_eq!(
                stats.thruststrength,
                [
                    Some(850.0),
                    Some(850.0),
                    Some(650.0),
                    Some(650.0),
                    Some(1000.0),
                    Some(450.0)
                ]
            );
            assert_eq!(stats.shipclass.as_deref(), Some("CLASS_Fighter"));
            assert_eq!(stats.is_capital_ship, Some(false));
            assert_eq!(stats.build_cost, Some(90));
            assert_eq!(stats.support_units, Some(2));
            assert_eq!(stats.guns.len(), 4);
            assert_eq!(
                stats.guns[2].gun_type.as_deref(),
                Some("GUN_MissileLauncher")
            );
            assert_eq!(stats.guns[0].fire_time2, Some(2.8));
            assert!(!stats.tactics.is_empty());
        }
    }
    println!("{} .shp files, {total_guns} gun blocks", shp.len());
    for m in &gun_count_mismatch {
        println!("gun count mismatch: {m}");
    }

    let print = |title: &str, list: Vec<hwc_formats::script::UnboundKeyword<'_>>| {
        println!("\n{title} ({}):", list.len());
        for u in list {
            let files: Vec<_> = u
                .files
                .iter()
                .map(|f| f.rsplit('/').next().unwrap())
                .collect();
            let shown = if files.len() > 6 {
                format!("{} files, e.g. {}", files.len(), files[..4].join(", "))
            } else {
                files.join(", ")
            };
            let case = u
                .case_variant_of
                .map(|h| format!(" [case variant of {h}]"))
                .unwrap_or_default();
            println!("  {}{case}: {shown}", u.key);
        }
    };
    print(
        "Cataclysm-only top-level ship keywords",
        survey.cataclysm_only_top_level(),
    );
    print("Cataclysm-only gun keywords", survey.cataclysm_only_gun());
    let unused = survey.hw1_unused();
    println!(
        "\nHW1 keywords unused by Cataclysm ({}): {}",
        unused.len(),
        unused.join(" ")
    );
    assert_eq!(survey.files, shp.len());
}
