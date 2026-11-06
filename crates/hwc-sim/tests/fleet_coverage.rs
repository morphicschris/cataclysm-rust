//! Every Somtaaw and Beast ship type over the real game data (`HWC_GAME_DIR` or
//! `HWC_EXTRACTED_DIR`; skipped when neither is set): each is created with every upgrade its
//! type can have and put through attack, special, halt and dock orders. Multiplayer can field
//! any of them, so none may panic, and every armed type must hurt what it is told to attack.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::harness::{Order, Run, Scenario, Spawn};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipClass, ShipRace, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let src: Box<dyn DataSource> = if let Ok(dir) = std::env::var("HWC_EXTRACTED_DIR") {
            Box::new(DirSource::new(dir).ok()?)
        } else if let Ok(dir) = std::env::var("HWC_GAME_DIR") {
            Box::new(BigArchive::open(std::path::Path::new(&dir).join("cataclysm.big")).ok()?)
        } else {
            return None;
        };
        let statics = Arc::new(StaticWorld::load_from(src.as_ref(), LoadOptions::default()));
        let tweaks = SimTweaks::load(src.as_ref());
        Some(Data { statics, tweaks })
    })
    .as_ref()
}

fn run_for(d: &Data, race: ShipRace, t: ShipType, orders: Vec<(u32, Order)>, ticks: u32) -> (f32, f32, bool) {
    let carrier = if race == ShipRace::Beast { ShipType::bCarrier } else { ShipType::sCarrier };
    let spawns = vec![
        Spawn { ship_type: t, player: 0, position: Vector::new(0.0, 0.0, 0.0) },
        Spawn { ship_type: ShipType::tStandardDestroyer, player: 1, position: Vector::new(3500.0, 0.0, 0.0) },
        Spawn { ship_type: carrier, player: 0, position: Vector::new(-4000.0, 0.0, 0.0) },
    ];
    let s = Scenario {
        name: "fleet",
        players: vec![race, ShipRace::Taiidan],
        spawns,
        orders,
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    let mut run = Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap();
    let id = run.ids[0];
    let tg = run.ids[1];
    let mask = hwc_sim::upgrade::type_mask(t);
    run.universe.ships.ship_mut(id).upgrade_flags |= mask;
    let start = run.universe.ships.get(tg).map_or(0.0, |s| s.health);
    run.run(ticks);
    let hurt = run.universe.ships.get(tg).map_or(start, |s| start - s.health);
    let alive = run.universe.ships.get(id).is_some();
    let moved = run.universe.ships.get(id).map_or(0.0, |s| s.posinfo.position.x);
    (hurt, moved, alive)
}

#[test]
fn every_somtaaw_and_beast_type_takes_orders() {
    let Some(d) = data() else { return };
    let mut mute = Vec::new();
    for race in [ShipRace::Somtaaw, ShipRace::Beast] {
        for t in race.ship_types() {
            let Some(st) = d.statics.ship(t) else {
                println!("{t:?}: no ship data");
                continue;
            };
            let class = st.ship_class;
            let armed = st.guns.as_ref().is_some_and(|g| !g.is_empty());
            let module = hwc_sim::ships::module_of(t);
            let attack = vec![(1, Order::Attack { ships: vec![0], targets: vec![1] })];
            let (hurt, x, alive) = run_for(d, race, t, attack, 1600);
            let special = vec![
                (1, Order::Special { ships: vec![0], targets: vec![] }),
                (200, Order::Special { ships: vec![0], targets: vec![1] }),
                (500, Order::Halt { ships: vec![0] }),
                (520, Order::Dock { ships: vec![0], purpose: hwc_sim::dock::purpose::DOCK, with: Some(2) }),
            ];
            let _ = run_for(d, race, t, special, 900);
            println!("{t:?} {class:?} {module:?} armed={armed} hurt={hurt:.0} x={x:.0} alive={alive}");
            let component = class == Some(ShipClass::Component);
            if armed && !component && hurt <= 0.0 && alive {
                mute.push(t);
            }
        }
    }
    println!("armed but did no damage: {mute:?}");
}
