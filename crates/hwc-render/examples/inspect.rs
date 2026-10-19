//! Dumps mesh objects, engine points and animations of a ship (development aid).
use hwc_render::{AssetSource, DirSource, ShipDef};
fn main() {
    let dir = std::env::var("HWC_EXTRACTED_DIR").unwrap_or_else(|_| "extracted".into());
    let src = DirSource::new(dir);
    let mut args = std::env::args().skip(1);
    let race = args.next().unwrap_or("race_sect".into());
    let name = args.next().unwrap_or("sacolyte".into());
    let def = ShipDef::load(&src, &race, &name).unwrap();
    println!("lod {:?} mex {:?}", def.stats.lod_file, def.mex_path());
    let mesh = def.mesh_path(&src);
    let geo = hwc_formats::geo::Geo::parse(&src.read(&mesh).unwrap()).unwrap();
    for (i, o) in geo.objects.iter().enumerate() {
        println!(
            "obj {i} {} parent {:?} polys {} m {:?}",
            o.name,
            o.parent,
            o.polygons.len(),
            &o.local_matrix[12..15]
        );
        let fl: std::collections::BTreeSet<_> =
            o.vertex_attrs.iter().map(|a| (a.flags, a.color)).collect();
        if fl.len() > 1 {
            println!("   attrs {:?}", fl);
        }
    }
    if let Some(m) = def.mex_path().and_then(|p| src.read(&p)) {
        let mex = hwc_formats::mex::Mex::parse(&m).unwrap();
        for c in &mex.chunks {
            if c.tag == "Eng" {
                println!("{} {:?}", c.name, c.data);
            }
        }
    }
    if let Some(d) = src.read(&format!("{race}\\{name}.mad")) {
        let mad = hwc_formats::mad::Mad::parse(&d).unwrap();
        println!("mad len {} fps {}", mad.length, mad.frames_per_second);
        for a in &mad.animations {
            println!("anim {:?}", a);
        }
        for o in &mad.objects {
            println!(
                "path {} bits {:x} keys {} times {:?}",
                o.name,
                o.animation_bits,
                o.keyframes.len(),
                o.keyframes.iter().map(|k| k.time).collect::<Vec<_>>()
            );
        }
    }
    for e in def.stats.other.iter().filter(|(k, _)| {
        k.starts_with("ANIM") || k.starts_with("trail") || k.starts_with("NUMBER_OF_ANIMS")
    }) {
        println!("{:?}", e);
    }
}
