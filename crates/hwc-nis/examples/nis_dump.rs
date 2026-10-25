//! Prints a loaded NIS: paths, cameras and events. `cargo run -p hwc-nis --example nis_dump -- <dir> n01`
use hwc_nis::NisData;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("dir");
    let name = args.next().unwrap_or_else(|| "n01".into());
    let nis = std::fs::read(format!("{dir}/{name}.nis")).expect("nis");
    let script = std::fs::read(format!("{dir}/{name}.script")).ok();
    let d = NisData::load(&nis, script.as_deref()).expect("load");
    println!("length {} loop {} fade-in {:?}", d.length, d.loop_point, d.scissor_fade_in);
    for (i, o) in d.objects.iter().enumerate() {
        let c = &o.curves;
        let n = c.samples as usize;
        println!(
            "{i:3} {:24} {:?} inst {} parent {:?} n {} t0 {:.3} tN {:.3} first ({:.0},{:.0},{:.0}) hpb ({:.3},{:.3},{:.3})",
            o.name, o.category, o.instance, o.parent, n, c.times[0], c.times[n.saturating_sub(1)],
            c.value(0, 0), c.value(1, 0), c.value(2, 0), c.value(3, 0), c.value(4, 0), c.value(5, 0)
        );
    }
    for (i, cam) in d.cameras.iter().enumerate() {
        let c = &cam.curves;
        let n = c.samples as usize;
        let bank: Vec<f32> = (0..n).map(|k| c.value(5, k)).collect();
        let maxb = bank.iter().fold(0f32, |a, b| a.max(b.abs()));
        println!("camera {i}: n {n} times {:.3}..{:.3} max |bank| {maxb:.4} rad", c.times[0], c.times[n - 1]);
        for k in 0..n {
            println!("   key {k} t {:.3} pos ({:.1},{:.1},{:.1}) hpb ({:.3},{:.3},{:.3}) tcb {:?}", c.times[k], c.value(0,k), c.value(1,k), c.value(2,k), c.value(3,k), c.value(4,k), c.value(5,k), c.params[k]);
        }
    }
    for e in &d.events {
        println!("{:8.3} {:?} {:?}", e.time, e.object.map(|i| d.objects[i].name.clone()), e.action);
    }
    for (l, s) in &d.ignored {
        println!("ignored line {l}: {s}");
    }
}
