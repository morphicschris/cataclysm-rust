//! Prints a ship's engine nozzles (`Eng` chunks of its `.mex`) and the objects of each level
//! of detail they attach to: `cargo run -p hwc-render --example nozzles -- <extracted dir> <ship>...`.

use hwc_render::assets::engine_nozzles;
use hwc_render::{DirSource, ShipDef, ShipModel, Skeleton};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("extracted directory");
    let src = DirSource::new(dir);
    let all = ShipDef::list_all(&src);
    for name in args {
        let Some(def) = all.iter().find(|d| d.name.eq_ignore_ascii_case(&name)) else {
            eprintln!("{name}: not found");
            continue;
        };
        println!("== {}", def.name);
        for n in engine_nozzles(&src, def) {
            println!(
                "  Eng{} at {:?} normal {:?} attached to {:?}",
                n.index, n.position, n.normal, n.attached_to
            );
        }
        let (levels, _) = ShipModel::load_lods(&src, def).expect("model");
        for (l, m) in levels.iter().enumerate() {
            let sk = Skeleton::from_model(m);
            let world = sk.rest_matrices();
            for (i, o) in sk.objects.iter().enumerate() {
                let t = world[i].w_axis.truncate();
                let r = world[i].x_axis.truncate();
                println!(
                    "  lod {l} object {i} {:12} parent {:?} at {:?} x-axis {:?}",
                    o.name, o.parent, t, r
                );
            }
        }
    }
}
