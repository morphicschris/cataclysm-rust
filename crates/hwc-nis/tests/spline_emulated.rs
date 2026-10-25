//! The spline evaluation against Cataclysm's `bsCurveUpdate` (0x423060) run in Ghidra's
//! emulator.
//!
//! `tests/data/spline_emulated.txt` holds the emulated results for a synthetic curve with
//! non-zero tension, continuity and bias and a key shorter than the skip threshold, stepped by
//! 0.01 s. When `HWC_EXTRACTED_DIR` is set and the private fixture
//! `recovered/nis/spline_n01_emulated.txt` exists, the six camera curves and the six curves of
//! `sRecon(1)` of `nis\n01.nis` are checked too, stepped by 1/16 s through the whole NIS
//! with one shared spline cache, as the NIS task runs them. The emulator computes with 64-bit
//! mantissas where the game runs at 53 bits; both round to the same `float` here.
//!
//! The emulator input was written by `examples/spline_vectors.rs`.

use hwc_math::spline::{SplineCache, SplineCurve, Tcb};
use hwc_nis::data::Curves;
use hwc_nis::NisData;

fn synthetic() -> Curves {
    let t = |tension, continuity, bias| Tcb { tension, continuity, bias };
    let points = vec![0.0, 0.0, 1.0, -1.0, -0.5, 0.0, -2.0, 3.5, -1.2, -1.2, -1.2, 7.0];
    let times = vec![0.0, 0.0, 0.5, 2.0, 2.03, 3.0, 4.0, 5.5, 7.0, 7.5, 9.0, 0.25];
    let params = vec![
        t(0.0, 0.0, 0.0),
        t(0.0, 0.0, 0.0),
        t(0.0, -1.0, 0.0),
        t(0.5, 1.0, 0.0),
        t(0.25, 0.0, 0.5),
        t(-1.0, 0.0, 0.0),
        t(0.0, 0.3, -0.7),
        t(1.0, 0.0, 0.0),
        t(0.2, 0.2, 0.2),
        t(0.0, 0.0, 0.0),
        t(0.0, 0.0, 0.0),
    ];
    Curves {
        samples: 11,
        times,
        params,
        channels: std::array::from_fn(|_| points.clone()),
    }
}

/// Expected (value bits, current point) per step and curve.
fn expected(text: &str) -> Vec<Vec<(u32, i32)>> {
    text.lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            l.split_whitespace()
                .skip(1)
                .map(|e| {
                    let (v, cp) = e.split_once(':').unwrap();
                    (u32::from_str_radix(v, 16).unwrap(), cp.parse().unwrap())
                })
                .collect()
        })
        .collect()
}

/// Runs `curves` in order every step with one cache; returns the mismatches.
fn run(curves: &mut [SplineCurve], dt: f32, want: &[Vec<(u32, i32)>]) -> Vec<String> {
    let mut cache = SplineCache::default();
    let mut bad = Vec::new();
    for (k, row) in want.iter().enumerate() {
        assert_eq!(row.len(), curves.len(), "step {k}");
        for (c, (curve, &(bits, cp))) in curves.iter_mut().zip(row).enumerate() {
            let v = curve.update(&mut cache, dt) as f32;
            if v.to_bits() != bits || curve.current_point != cp {
                bad.push(format!(
                    "step {k} curve {c}: got {v} ({:08x}) at point {}, want {} ({bits:08x}) at {cp}",
                    v.to_bits(),
                    curve.current_point,
                    f32::from_bits(bits)
                ));
            }
        }
    }
    bad
}

#[test]
fn synthetic_curve_matches_the_emulator() {
    let want = expected(include_str!("data/spline_emulated.txt"));
    assert_eq!(want.len(), 1000);
    let mut curves = vec![synthetic().curve(0)];
    let bad = run(&mut curves, 0.01, &want);
    assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad[..bad.len().min(20)].join("\n"));
}

#[test]
fn n01_curves_match_the_emulator() {
    let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipped");
        return;
    };
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../recovered/nis/spline_n01_emulated.txt");
    let Ok(text) = std::fs::read_to_string(&fixture) else {
        eprintln!("{} not found; skipped", fixture.display());
        return;
    };
    let nis = std::fs::read(std::path::Path::new(&dir).join("nis").join("n01.nis")).expect("n01.nis");
    let d = NisData::load(&nis, None).expect("load");
    let mut curves: Vec<SplineCurve> = (0..6)
        .map(|c| d.cameras[0].curves.curve(c))
        .chain((0..6).map(|c| d.objects[1].curves.curve(c)))
        .collect();
    let want = expected(&text);
    assert_eq!(want.len(), 1780);
    let bad = run(&mut curves, 0.0625, &want);
    assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad[..bad.len().min(20)].join("\n"));
}

#[test]
fn ship_euler_matches_the_emulator() {
    let text = include_str!("data/euler_emulated.txt");
    let mut checked = 0;
    let mut bad = Vec::new();
    for line in text.lines().filter(|l| !l.starts_with('#')) {
        let mut w = line.split_whitespace();
        let i: i32 = w.next().unwrap().parse().unwrap();
        let want: Vec<u32> = w.map(|h| u32::from_str_radix(h, 16).unwrap()).collect();
        let f = i as f32;
        let (h, p, b) = (f * 0.37 - 3.0, f * 0.21 - 2.0, f * 0.13 - 1.0);
        let m = hwc_nis::euler::ship_euler_to_matrix(h, p, b);
        for (k, (&got, &w)) in m.m.iter().zip(&want).enumerate() {
            // The emulator's FSIN/FCOS are double sin/cos; allow one unit in the last place.
            let d = (got.to_bits() as i64 - w as i64).abs();
            if d > 1 {
                bad.push(format!("{i}[{k}]: {got} vs {}", f32::from_bits(w)));
            }
        }
        checked += 1;
    }
    assert_eq!(checked, 64);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// `nis\n01.nis` and its script load as the game holds them (needs `HWC_EXTRACTED_DIR`).
#[test]
fn n01_loads() {
    let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") else {
        eprintln!("HWC_EXTRACTED_DIR not set; skipped");
        return;
    };
    let dir = std::path::Path::new(&dir).join("nis");
    let nis = std::fs::read(dir.join("n01.nis")).expect("n01.nis");
    let script = std::fs::read(dir.join("n01.script")).expect("n01.script");
    let d = NisData::load(&nis, Some(&script)).expect("load");
    assert_eq!(d.objects.len(), 46);
    assert_eq!(d.cameras.len(), 1);
    assert_eq!(d.scissor_fade_in, Some(1.0 * hwc_nis::data::FRAME_TO_SECONDS));
    assert_eq!(d.find_object(r"RACE_SECT\sRecon (2)"), Some(2));
    assert_eq!(d.find_object(r"RACE_DERELICT\dbuoy (14)"), Some(19));
    assert_eq!(d.find_object(r"Asteroids\asteroid3 (7)"), Some(44));
    // The first event after time 0 is the fade in at frame 1.
    let first = d.events.iter().find(|e| e.time > 0.0).unwrap();
    assert_eq!(first.time, 1.0 * hwc_nis::data::FRAME_TO_SECONDS);
    assert!(matches!(first.action, hwc_nis::Action::FadeTo { level, .. } if level == 0.0));
    // Events are in time order, script order within a time.
    assert!(d.events.windows(2).all(|w| w[0].time < w[1].time || (w[0].time == w[1].time && w[0].order < w[1].order)));
    // Every keyword of the shipped scripts is known or ignored as by the original.
    for name in ["n02", "n03", "n04", "n05", "n06", "n07", "n08", "n09", "n10", "n11", "nislet01", "nislet02", "nislet03"] {
        let nis = std::fs::read(dir.join(format!("{name}.nis"))).expect("nis");
        let script = std::fs::read(dir.join(format!("{name}.script"))).expect("script");
        let d = NisData::load(&nis, Some(&script)).unwrap_or_else(|e| panic!("{name}: {e}"));
        for (line, what) in &d.ignored {
            let known = ["motionCueScale", "minimumLODSet", "stretch", "widescreenOut", "closeslipgate"];
            assert!(known.iter().any(|k| what.starts_with(k)), "{name}:{line}: {what}");
        }
        assert!(!d.events.is_empty(), "{name}");
    }
}
