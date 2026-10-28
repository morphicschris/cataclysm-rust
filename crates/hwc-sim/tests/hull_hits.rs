//! The hull-mesh refinement of the hit tests (`meshShipSegmentCollide`) over the real game
//! data (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`; skipped when neither is set): every ship type
//! with a level-of-detail table gets a hull, a ray at a ship's middle meets it, and of a grid
//! of parallel rays only some of those that enter the collision box meet the hull.

use std::sync::{Arc, OnceLock};

use hwc_formats::big::BigArchive;
use hwc_math::Vector;
use hwc_sim::collision::{hull_ray, rect_line};
use hwc_sim::harness::{Run, Scenario, Spawn};
use hwc_sim::statics::{DataSource, DirSource, LoadOptions, ShipRace, ShipType, StaticWorld};
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

#[test]
fn ship_types_have_hulls() {
    let Some(d) = data() else { return };
    let mut with_lod = 0;
    let mut missing = Vec::new();
    for &t in ShipType::ALL {
        let Some(st) = d.statics.ship(t) else { continue };
        if st.header.lod.is_none() {
            continue;
        }
        with_lod += 1;
        match &st.hull {
            Some(h) => assert!(h.objects.iter().any(|o| !o.polygons.is_empty()), "{t:?}: an empty hull"),
            None => missing.push(t),
        }
    }
    println!("{with_lod} ship types with a LOD table; without a hull: {missing:?}");
    assert!(with_lod > 50);
    assert!(missing.len() * 10 < with_lod, "too many without a hull: {missing:?}");
}

fn one_ship(d: &Data, ship_type: ShipType, race: ShipRace) -> Run {
    let s = Scenario {
        name: "hull",
        players: vec![race],
        spawns: vec![Spawn { ship_type, player: 0, position: Vector::new(0.0, 0.0, 0.0) }],
        orders: Vec::new(),
        resources: Vec::new(),
        resource_units: Vec::new(),
    };
    Run::new(d.statics.clone(), d.tweaks.clone(), s).unwrap()
}

/// The fighters' files turn the test off (`bUseRayTriangleIntersectionForHitLocation FALSE`):
/// an Acolyte keeps its box.
#[test]
fn acolyte_keeps_its_box() {
    let Some(d) = data() else { return };
    let run = one_ship(d, ShipType::sAcolyte, ShipRace::Somtaaw);
    let st = d.statics.ship(ShipType::sAcolyte).unwrap();
    assert!(!st.use_ray_triangle_intersection_for_hit_location);
    let ship = run.universe.ships.ship(run.ids[0]);
    let p = ship.coll.coll_position;
    assert_eq!(hull_ray(ship, st, Vector::new(p.x - 500.0, p.y, p.z), p), None);
}

/// Rays along +x through a grid in front of the Beast carrier: the one through its middle
/// meets the hull, beyond where it enters the box; a good part of the rays that enter the box
/// miss the hull. (The hull pokes out of the box here and there; only box hits are refined.)
#[test]
fn carrier_hull_is_inside_its_box() {
    let Some(d) = data() else { return };
    let run = one_ship(d, ShipType::bCarrier, ShipRace::Beast);
    let ship = run.universe.ships.ship(run.ids[0]);
    let st = d.statics.ship(ShipType::bCarrier).unwrap();
    assert!(st.use_ray_triangle_intersection_for_hit_location && st.hull.is_some());
    let r = st.header.coll.coll_sphere_size;
    let centre = ship.coll.coll_position;
    let dir = Vector::new(1.0, 0.0, 0.0);
    let from = |y: f32, z: f32| Vector::new(centre.x - 4.0 * r, centre.y + y, centre.z + z);
    let ahead = |p: Vector| Vector::new(p.x + 1.0, p.y, p.z);
    let p = hull_ray(ship, st, from(0.0, 0.0), ahead(from(0.0, 0.0))).flatten().expect("the middle ray meets the hull");
    assert!((p.x - centre.x).abs() < r, "{p:?} near {centre:?}");
    let (mut boxed, mut both) = (0, 0);
    let n = 24;
    for i in 0..=n {
        for j in 0..=n {
            let y = (i as f32 / n as f32 * 2.0 - 1.0) * r;
            let z = (j as f32 / n as f32 * 2.0 - 1.0) * r;
            let p = from(y, z);
            let (t, _) = rect_line(ship, st, p, dir, 8.0 * r);
            let hull = hull_ray(ship, st, p, ahead(p)).flatten();
            if t < 0.0 {
                continue;
            }
            boxed += 1;
            if let Some(h) = hull {
                both += 1;
                assert!(h.x >= p.x + t - 1.0, "the hull is met beyond the box entry at ({y}, {z})");
            }
        }
    }
    println!("of {} rays: {boxed} enter the box, {both} of those meet the hull", (n + 1) * (n + 1));
    assert!(both > 0 && both < boxed);
}
