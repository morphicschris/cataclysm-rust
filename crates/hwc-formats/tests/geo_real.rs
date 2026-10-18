//! Parses every `.geo` and `.peo` mesh in an extracted copy of the game archive
//! (`HWC_EXTRACTED_DIR`).

use std::path::{Path, PathBuf};

use hwc_formats::geo::Geo;

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

fn check_all(ext: &str) {
    let Some(root) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipping");
        return;
    };
    let mut files = Vec::new();
    files_with_ext(Path::new(&root), ext, &mut files);
    let mut failures = Vec::new();
    let (mut objects, mut polygons, mut vertices) = (0usize, 0usize, 0usize);
    for path in &files {
        let data = std::fs::read(path).unwrap();
        let geo = match Geo::parse(&data) {
            Ok(g) => g,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        // Only .peo files use the paged identifier (one old-version .peo still uses RMF97ba).
        assert!(!geo.paged || ext == "peo", "{}", path.display());
        assert!(!geo.objects.is_empty(), "{}", path.display());
        assert!(geo.roots().count() >= 1, "{}", path.display());
        for (i, obj) in geo.objects.iter().enumerate() {
            assert_eq!(obj.vertex_attrs.len(), obj.vertices.len());
            assert_eq!(
                obj.normals.len(),
                (obj.face_normal_count + obj.vertex_normal_count) as usize
            );
            for v in &obj.vertices {
                assert!(
                    v.position.iter().all(|c| c.is_finite()),
                    "{}",
                    path.display()
                );
            }
            for p in &obj.polygons {
                assert!(p
                    .vertices
                    .iter()
                    .all(|&v| (v as usize) < obj.vertices.len()));
                assert!((p.material as usize) < geo.materials.len());
            }
            if let Some(parent) = obj.parent {
                assert!(
                    geo.children(parent).contains(&i),
                    "{} object {i}",
                    path.display()
                );
            }
            objects += 1;
            polygons += obj.polygons.len();
            vertices += obj.vertices.len();
        }
    }
    eprintln!(
        "{ext}: {} files, {} failed; {objects} objects, {vertices} vertices, {polygons} polygons",
        files.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{failures:#?}");
}

#[test]
fn parse_all_geo() {
    check_all("geo");
}

#[test]
fn parse_all_peo() {
    check_all("peo");
}
