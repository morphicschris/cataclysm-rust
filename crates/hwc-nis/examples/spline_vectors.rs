//! Writes spline curves (n01's camera and one ship path, plus a synthetic curve with tension,
//! continuity, bias and a short key) as hex words, the input of the Ghidra emulator run that
//! produced `tests/data/spline_emulated.txt`.
//! `cargo run -p hwc-nis --example spline_vectors -- extracted/nis/n01.nis out.txt`
use hwc_math::spline::Tcb;
use hwc_nis::data::Curves;
use hwc_nis::NisData;
use std::fmt::Write;

fn hex(v: &[f32]) -> String {
    v.iter().map(|f| format!("{:08x}", f.to_bits())).collect::<Vec<_>>().join(" ")
}

pub fn synthetic() -> Curves {
    let t = |tension, continuity, bias| Tcb { tension, continuity, bias };
    let points = vec![0.0, 0.0, 1.0, -1.0, -0.5, 0.0, -2.0, 3.5, -1.2, -1.2, -1.2, 7.0];
    let times = vec![0.0, 0.0, 0.5, 2.0, 2.03, 3.0, 4.0, 5.5, 7.0, 7.5, 9.0, 0.25];
    let params = vec![
        t(0.0, 0.0, 0.0), t(0.0, 0.0, 0.0), t(0.0, -1.0, 0.0), t(0.5, 1.0, 0.0), t(0.25, 0.0, 0.5),
        t(-1.0, 0.0, 0.0), t(0.0, 0.3, -0.7), t(1.0, 0.0, 0.0), t(0.2, 0.2, 0.2), t(0.0, 0.0, 0.0),
        t(0.0, 0.0, 0.0),
    ];
    Curves { samples: 11, times, params, channels: std::array::from_fn(|_| points.clone()) }
}

fn main() {
    let mut a = std::env::args().skip(1);
    let nis = std::fs::read(a.next().expect("nis")).expect("read");
    let out = a.next().expect("out");
    let d = NisData::load(&nis, None).expect("load");
    let mut s = String::new();
    let mut sets: Vec<(&str, Curves)> = vec![("camera0", d.cameras[0].curves.clone()), ("recon1", d.objects[1].curves.clone())];
    sets.push(("synthetic", synthetic()));
    for (name, c) in &sets {
        let channels = if *name == "synthetic" { 1 } else { 6 };
        for ch in 0..channels {
            let tcb: Vec<f32> = c.params.iter().flat_map(|k| [k.tension, k.continuity, k.bias]).collect();
            writeln!(s, "curve {name}.{ch} {}", c.samples).unwrap();
            writeln!(s, "points {}", hex(&c.channels[ch])).unwrap();
            writeln!(s, "times {}", hex(&c.times)).unwrap();
            writeln!(s, "params {}", hex(&tcb)).unwrap();
        }
    }
    std::fs::write(out, s).unwrap();
}
