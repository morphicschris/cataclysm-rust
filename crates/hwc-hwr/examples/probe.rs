//! `probe <import dir> <effect or ship>`: plays an effect (or a ship's death) for three seconds
//! headless and prints what it draws.

use hwc_hwr::player::{Anchor, DrawList};
use hwc_hwr::{Library, Player};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let lib = Library::open(std::path::Path::new(&args[0])).expect("import");
    let name = args[1].to_ascii_lowercase();
    let seq = lib.death(&name).cloned();
    let mut pl = Player::new(lib, 1);
    match seq {
        Some(s) => {
            println!(
                "death of {name}: {} events over {} s",
                s.events.len(),
                s.length
            );
            pl.start_death(&s, 1, Anchor::at(glam::Vec3::ZERO), 50.0, 1.0, 1.0);
        }
        None => assert!(
            pl.spawn(
                &name,
                1,
                Anchor::at(glam::Vec3::ZERO),
                glam::Vec3::ZERO,
                1.0
            ),
            "no effect {name}"
        ),
    }
    for step in 0..240 {
        pl.update(1.0 / 30.0, |_| Some(Anchor::at(glam::Vec3::ZERO)));
        if step % 30 == 0 {
            let mut d = DrawList::default();
            pl.draw(&mut d);
            let r: Vec<f32> = d.sprites.iter().take(4).map(|s| s.half_size).collect();
            println!(
                "t {:.2}: active {} sprites {} quads {} strips {} sizes {r:?}",
                (step + 1) as f32 / 30.0,
                pl.active(),
                d.sprites.len(),
                d.quads.len(),
                d.strips.len()
            );
        }
    }
}
