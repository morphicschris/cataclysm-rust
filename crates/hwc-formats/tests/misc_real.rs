//! Parses every `.hff` font, `.fib` screen layout, `.hsf` lighting file and `.nis` cinematic (with
//! its `.script`) in an extracted copy of the game archive (`HWC_EXTRACTED_DIR`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use hwc_formats::fib::{AtomData, AtomKind, Fib, ATOM_LINK, LINK_ENABLED};
use hwc_formats::hsf::{Hsf, LightKind};
use hwc_formats::nis::{Nis, NisScript, ObjectCategory};
use hwc_formats::font::Font;

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

fn collect(ext: &str) -> Option<Vec<PathBuf>> {
    let Some(root) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipping");
        return None;
    };
    let mut files = Vec::new();
    files_with_ext(Path::new(&root), ext, &mut files);
    files.sort();
    Some(files)
}

fn report(ext: &str, files: &[PathBuf], failures: &[String]) {
    eprintln!("{ext}: {} files, {} failed", files.len(), failures.len());
    assert!(
        failures.is_empty(),
        "{ext} failures:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_font() {
    let Some(files) = collect("hff") else { return };
    assert_eq!(files.len(), 102);
    let mut failures = Vec::new();
    let mut glyphs = 0usize;
    for path in &files {
        let font = match Font::parse(&std::fs::read(path).unwrap()) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let name = path.display();
        assert_eq!(font.flags, 0, "{name}");
        assert_eq!(font.glyphs().count() as u32, font.declared_count, "{name}");
        assert!(
            font.declared_count == 94 || font.declared_count == 213,
            "{name}"
        );
        assert_eq!(font.palette.len(), 2, "{name}");
        assert!(
            font.image.iter().all(|&c| c <= 15),
            "{name}: coverage above 15"
        );
        // Printable ASCII is always there and 'A' has ink.
        for c in (b'0'..=b'9').chain(b'A'..=b'Z').chain(b'a'..=b'z') {
            assert!(font.glyph(c).is_some(), "{name}: missing {:?}", c as char);
        }
        let a = font.bitmap(b'A').unwrap();
        assert!(a.alpha().contains(&255), "{name}: 'A' has no solid pixel");
        for g in font.glyphs() {
            assert_eq!(i32::from(g.height), font.full_height, "{name}");
            let bm = font.bitmap(g.code).unwrap();
            assert_eq!(bm.coverage.len() as u32, bm.width * bm.height);
            glyphs += 1;
        }
        assert!(font.text_width(b"Homeworld") > 0, "{name}");
    }
    eprintln!("hff: {glyphs} glyphs");
    report("hff", &files, &failures);
}

#[test]
fn every_screen_layout() {
    let Some(files) = collect("fib") else { return };
    assert_eq!(files.len(), 21);
    let mut failures = Vec::new();
    let (mut screens, mut atoms, mut links) = (0usize, 0usize, 0usize);
    let mut dangling = Vec::new();
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    let mut fonts: BTreeMap<String, usize> = BTreeMap::new();
    let font_dir =
        std::env::var_os("HWC_EXTRACTED_DIR").map(|d| Path::new(&d).join("fonts").join("english"));
    for path in &files {
        let fib = match Fib::parse(&std::fs::read(path).unwrap()) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let name = path.display();
        assert!(!fib.screens.is_empty(), "{name}");
        for s in &fib.screens {
            screens += 1;
            for l in &s.links {
                links += 1;
                assert!(l.flags & LINK_ENABLED != 0, "{name}: disabled link");
                let target = l.target.as_deref().expect("link target");
                // Links only lead to screens of the same file.
                assert!(
                    fib.screen(target).is_some(),
                    "{name}: link to unknown screen {target}"
                );
            }
            for a in &s.atoms {
                atoms += 1;
                *kinds.entry(format!("{:?}", a.kind)).or_default() += 1;
                assert!(
                    !matches!(a.kind, AtomKind::Other(_)),
                    "{name}: kind {}",
                    a.raw_kind
                );
                assert!(
                    !matches!(a.data, AtomData::Raw(_)),
                    "{name}: raw data on {:?}",
                    a.kind
                );
                if let Some(font) = a.font() {
                    *fonts.entry(font.to_ascii_lowercase()).or_default() += 1;
                    if let Some(dir) = &font_dir {
                        assert!(
                            dir.join(font.to_ascii_lowercase()).exists(),
                            "{name}: font {font} missing"
                        );
                    }
                }
                if a.kind == AtomKind::DecorativeRegion {
                    let bmp = a.bitmap_path().expect("decorative bitmap");
                    assert!(bmp.to_ascii_lowercase().ends_with(".lif"), "{name}: {bmp}");
                }
                if a.kind == AtomKind::BitmapButton {
                    assert!(
                        a.bitmap_path().is_some(),
                        "{name}: bitmap button without bitmap"
                    );
                }
                if a.kind == AtomKind::Button && a.name.is_some() && a.has(ATOM_LINK) {
                    let n = a.name.as_deref().unwrap();
                    if s.link(n).is_none() {
                        dangling.push(format!("{}/{}", s.name, n));
                    }
                }
            }
        }
    }
    let front = Fib::parse(
        &std::fs::read(files.iter().find(|p| p.ends_with("front_end.fib")).unwrap()).unwrap(),
    )
    .unwrap();
    let main = front.screen("Main_game_screen").unwrap();
    assert!(main.atoms.iter().any(|a| a.text() == Some("&QUIT")));
    eprintln!("fib: {screens} screens, {links} links, {atoms} atoms");
    eprintln!("fib kinds: {kinds:?}");
    eprintln!("fib link buttons without a link in their screen: {dangling:?}");
    eprintln!("fib fonts: {fonts:?}");
    report("fib", &files, &failures);
}

#[test]
fn every_lighting_file() {
    let Some(files) = collect("hsf") else { return };
    assert_eq!(files.len(), 34);
    let mut failures = Vec::new();
    for path in &files {
        let hsf = match Hsf::parse(&std::fs::read(path).unwrap()) {
            Ok(h) => h,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let name = path.display();
        assert!(hsf.ambient().is_some(), "{name}");
        assert!((1..=2).contains(&hsf.directional().count()), "{name}");
        for l in &hsf.lights {
            assert!(
                matches!(
                    l.kind,
                    LightKind::Ambient | LightKind::Distant | LightKind::Spot
                ),
                "{name}"
            );
            assert!(l.intensity.is_finite() && l.intensity >= 0.0, "{name}");
            assert!(l.position.iter().all(|v| v.is_finite()), "{name}");
            if l.kind == LightKind::Spot {
                assert!(l.cone_angle.is_some() && l.edge_angle.is_some(), "{name}");
            } else if l.kind == LightKind::Distant {
                assert!(l.cone_angle.is_none(), "{name}");
            }
        }
    }
    report("hsf", &files, &failures);
}

#[test]
fn every_cinematic() {
    let Some(files) = collect("nis") else { return };
    assert_eq!(files.len(), 14);
    let mut failures = Vec::new();
    let (mut paths, mut keys, mut events, mut resolved, mut unresolved) = (0, 0, 0, 0, 0);
    let mut bad_durations = 0usize;
    let mut overrun = 0f32;
    for path in &files {
        let nis = match Nis::parse(&std::fs::read(path).unwrap()) {
            Ok(n) => n,
            Err(e) => {
                failures.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let name = path.display();
        assert!(nis.length > 0.0, "{name}");
        assert_eq!(nis.cameras.len(), 1, "{name}");
        assert!(nis.events.is_empty(), "{name}");
        let all_keys = nis
            .objects
            .iter()
            .map(|o| (&o.keys, o.time_offset))
            .chain(nis.cameras.iter().map(|c| (&c.keys, c.time_offset)));
        for (ks, _) in all_keys {
            paths += 1;
            keys += ks.len();
            assert!(ks.len() >= 4, "{name}: only {} keys", ks.len());
            // The trailing end-condition keys may sit a frame before the last real key.
            let body = &ks[..ks.len() - 2];
            assert!(
                body.windows(2).all(|w| w[0].time <= w[1].time),
                "{name}: times not sorted"
            );
            for (i, k) in ks.iter().enumerate() {
                assert!(
                    k.position.iter().chain(&k.rotation).all(|v| v.is_finite()),
                    "{name}"
                );
                // Some actor paths run on past the sequence length (up to ~17 s in n01).
                assert!(k.time >= 0.0, "{name}: negative key time");
                overrun = overrun.max(k.time - nis.length);
                if i + 1 < body.len() && (ks[i + 1].time - k.time - k.duration).abs() > 1e-3 {
                    bad_durations += 1;
                }
            }
        }
        for o in &nis.objects {
            assert!(
                !matches!(o.category(), ObjectCategory::Other(_)),
                "{name}: race {}",
                o.race
            );
            if let Some(p) = &o.parent {
                assert!(
                    nis.objects.iter().any(|q| &q.name == p),
                    "{name}: parent {p}"
                );
            }
        }

        let script_path = path.with_extension("script");
        let script = NisScript::parse_bytes(&std::fs::read(&script_path).unwrap())
            .unwrap_or_else(|e| panic!("{}: {e}", script_path.display()));
        assert!(!script.events.is_empty(), "{name}");
        for e in &script.events {
            assert!(
                e.time.seconds() >= 0.0 && e.time.seconds() <= nis.length,
                "{}: line {}",
                script_path.display(),
                e.line
            );
        }
        events += script.events.len();
        for obj in &script.objects {
            if nis.find_object(obj).is_some() {
                resolved += 1;
            } else {
                unresolved += 1;
                eprintln!("{}: no path for {obj}", script_path.display());
            }
        }
    }
    eprintln!(
        "nis: {paths} paths, {keys} keys, {events} script events, {resolved} `with` objects resolved, {unresolved} not; {bad_durations} keys whose duration != next time - time; paths overrun the length by up to {overrun:.2} s"
    );
    assert_eq!(bad_durations, 0);
    assert_eq!(unresolved, 0);
    report("nis", &files, &failures);
}
