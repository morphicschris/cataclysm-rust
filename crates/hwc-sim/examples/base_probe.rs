//! Prints what happens in the `base` scenario (development probe).
use std::sync::Arc;

use hwc_sim::harness::{scenario, Run};
use hwc_sim::statics::{DirSource, LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

fn main() {
    let dir = std::env::var("HWC_EXTRACTED_DIR").expect("HWC_EXTRACTED_DIR");
    let src = DirSource::new(dir).unwrap();
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
    let tweaks = SimTweaks::load(&src);
    let mut run = Run::new(statics, tweaks, scenario("base").unwrap()).unwrap();
    let ticks: u32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(3000);
    for t in 0..ticks {
        run.step();
        let u = &run.universe;
        for e in &u.base.events {
            println!("{t}: {e:?}");
        }
        if t == ticks - 1 {
            for s in u.ships.iter() {
                println!("ship {} {:?} pos {:?} clamp {:?} hide {}", s.id.0, s.ship_type, s.posinfo.position, s.base.clamp.as_ref().map(|c| (c.host, c.rel_position)), s.flags & 0x8000 != 0);
            }
        }
        if t % 200 == 0 {
            let p = &u.base.players[0];
            print!("{t}: RU {} su {}/{} ships {} |", p.resource_units, p.su_used, p.su_capacity, u.ship_list.len());
            for s in u.ships.iter().filter(|s| s.ship_type == hwc_sim::statics::ShipType::sWorker) {
                print!(" w{} st{} h{} d{} ru{} pos({:.0},{:.0},{:.0})", s.id.0, s.base.dock_state, s.base.harvest_state, s.base.dock_with.map_or(-1, |d| d.0 as i32), s.base.resources, s.posinfo.position.x, s.posinfo.position.y, s.posinfo.position.z);
                if let Some(c) = s.command.and_then(|c| u.commands.get(c)) { print!(" o{}", c.order); }
            }
            println!();
        }
    }
}
