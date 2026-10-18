//! Parses every `.mex`, `.mad` and `.btg` file in an extracted copy of the game archive
//! (`HWC_EXTRACTED_DIR`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hwc_formats::btg::Btg;
use hwc_formats::mad::Mad;
use hwc_formats::mex::{ChunkData, Mex};

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

/// Parsed files of one extension, or `None` when `HWC_EXTRACTED_DIR` is unset.
fn parse_all<T>(
    ext: &str,
    parse: impl Fn(&[u8]) -> Result<T, String>,
) -> Option<Vec<(PathBuf, T)>> {
    let Some(root) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipping");
        return None;
    };
    let mut files = Vec::new();
    files_with_ext(Path::new(&root), ext, &mut files);
    files.sort();
    let mut parsed = Vec::new();
    let mut failures = Vec::new();
    for path in files {
        let data = std::fs::read(&path).unwrap();
        match parse(&data) {
            Ok(v) => parsed.push((path, v)),
            Err(e) => failures.push(format!("{}: {e}", path.display())),
        }
    }
    eprintln!("{ext}: {} parsed, {} failed", parsed.len(), failures.len());
    assert!(failures.is_empty(), "{failures:#?}");
    assert!(
        !parsed.is_empty(),
        "no .{ext} files under HWC_EXTRACTED_DIR"
    );
    Some(parsed)
}

#[test]
fn parse_all_mex() {
    let Some(files) = parse_all("mex", |d| Mex::parse(d).map_err(|e| e.to_string())) else {
        return;
    };
    let mut by_tag: BTreeMap<String, usize> = BTreeMap::new();
    let mut attached = 0;
    let mut negative_boxes = Vec::new();
    for (path, mex) in &files {
        let p = path.display();
        assert_eq!(mex.version, hwc_formats::mex::VERSION, "{p}");
        assert_eq!(mex.trailing_bytes, 0, "{p}");
        // Every mesh has exactly one collision sphere and box.
        let (_, radius) = mex.collision_sphere().unwrap_or_else(|| panic!("{p}"));
        assert!(radius.is_finite() && radius > 0.0, "{p}");
        let (_, extent) = mex.collision_box().unwrap_or_else(|| panic!("{p}"));
        assert!(extent.iter().all(|e| e.is_finite()), "{p}");
        if extent.iter().any(|e| *e < 0.0) {
            negative_boxes.push(p.to_string());
        }
        for c in &mex.chunks {
            *by_tag.entry(c.tag.clone()).or_default() += 1;
            match &c.data {
                ChunkData::Unknown(_) => panic!("{p}: unknown chunk tag {:?}", c.tag),
                ChunkData::Engine { attached_to, .. } | ChunkData::NavLight { attached_to, .. } => {
                    // Cataclysm always writes the long form; the name may be empty.
                    let name = attached_to.as_ref().unwrap_or_else(|| panic!("{p}"));
                    if !name.is_empty() {
                        attached += 1;
                    }
                }
                ChunkData::Dock { name, .. } | ChunkData::Salvage { name, .. } => {
                    assert!(!name.is_empty(), "{p}");
                }
                _ => {}
            }
        }
    }
    eprintln!("mex chunks by tag: {by_tag:?}; {attached} engine/nav chunks attached to objects");
    assert_eq!(files.len(), 967);
    // Only the effect-mesh `etg/meshes/junk.mex` has a degenerate box (z extent -9999993).
    assert_eq!(negative_boxes.len(), 1, "{negative_boxes:?}");
    assert!(negative_boxes[0].ends_with("junk.mex"));
    let expected = [
        ("Col", 967),
        ("Dmg", 5543),
        ("Dok", 5737),
        ("Eng", 463),
        ("Gun", 1275),
        ("Nav", 2951),
        ("Rct", 967),
        ("Sal", 557),
    ];
    let expected: BTreeMap<String, usize> =
        expected.iter().map(|&(t, n)| (t.to_string(), n)).collect();
    assert_eq!(by_tag, expected);
}

#[test]
fn parse_all_mad() {
    let Some(files) = parse_all("mad", |d| Mad::parse(d).map_err(|e| e.to_string())) else {
        return;
    };
    let (mut animations, mut objects, mut keys) = (0, 0, 0);
    for (path, mad) in &files {
        let p = path.display();
        assert_eq!(mad.frames_per_second, 30.0, "{p}");
        assert!(!mad.animations.is_empty() && !mad.objects.is_empty(), "{p}");
        for a in &mad.animations {
            assert!(!a.name.is_empty(), "{p}");
            assert!(a.start_time <= a.end_time, "{p} {}", a.name);
        }
        for o in &mad.objects {
            assert!(!o.name.is_empty(), "{p}");
            assert!(
                o.animation_bits >> mad.animations.len() == 0,
                "{p} {}",
                o.name
            );
            assert!(
                o.keyframes.windows(2).all(|w| w[0].time <= w[1].time),
                "{p} {}: key times not sorted",
                o.name
            );
            keys += o.keyframes.len();
        }
        animations += mad.animations.len();
        objects += mad.objects.len();
    }
    eprintln!("mad: {animations} animations, {objects} object paths, {keys} keyframes");
    assert_eq!(files.len(), 29);
    assert_eq!((animations, objects, keys), (94, 157, 3057));

    let (_, worker) = files
        .iter()
        .find(|(p, _)| p.file_name().is_some_and(|n| n == "bworker.mad"))
        .expect("bworker.mad");
    let names: Vec<&str> = worker.animations.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "JackOut",
            "JackClose",
            "DockClose",
            "DockOpen",
            "BladesSpin",
            "JackSpin",
            "DrillAndBlades"
        ]
    );
    assert_eq!(worker.objects.len(), 12);
    assert_eq!(worker.object("bWorker").unwrap().keyframes.len(), 20);
}

#[test]
fn parse_all_btg() {
    let Some(files) = parse_all("btg", |d| Btg::parse(d).map_err(|e| e.to_string())) else {
        return;
    };
    let (mut verts, mut stars, mut polys) = (0, 0, 0);
    for (path, btg) in &files {
        let p = path.display();
        assert_eq!(btg.trailing_bytes, 0, "{p}");
        assert_eq!(btg.page_size, [2560, 2048], "{p}");
        let [w, h] = btg.page_size.map(f64::from);
        for v in &btg.vertices {
            assert!((0.0..=w).contains(&v.position[0]), "{p}");
            assert!((0.0..=h).contains(&v.position[1]), "{p}");
        }
        for s in &btg.stars {
            assert!(s.texture.to_ascii_lowercase().ends_with(".tga"), "{p}");
        }
        verts += btg.vertices.len();
        stars += btg.stars.len();
        polys += btg.polygons.len();
    }
    eprintln!("btg: {verts} vertices, {stars} stars, {polys} polygons");
    assert_eq!(files.len(), 46);
    assert_eq!((verts, stars, polys), (33318, 8313, 62816));

    let (_, default) = files
        .iter()
        .find(|(p, _)| p.file_name().is_some_and(|n| n == "default.btg"))
        .expect("default.btg");
    assert_eq!(
        (
            default.vertices.len(),
            default.stars.len(),
            default.polygons.len()
        ),
        (770, 97, 1501)
    );
}
