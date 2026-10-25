//! Mission 6's nebulae built by the level loader (needs the game: `HWC_DATA_DIR` with an
//! imported data directory or `HWC_GAME_DIR` with an install; skipped when neither is set).

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_math::Vector;
use hwc_sim::level::{self, LevelContext, LevelState, ResourceTweaks};
use hwc_sim::mission::GameDataSource;
use hwc_sim::nebula::{self, chunk_flags, tendril_flags, NebulaCamera};
use hwc_sim::resource::ResourceKind;
use hwc_sim::statics::{LoadOptions, ShipRace, StaticWorld};
use hwc_sim::{SimTweaks, Universe};

/// The `Resources Nebula` lines of `SinglePlayer\Mission06\General`: centre and tendrils.
const LINES: [([f32; 3], usize); 8] = [
    ([-39270.9, 48227.9, -12593.6], 30),
    ([35165.4, -63483.9, 14597.7], 30),
    ([-18473.8, -31679.4, 9662.7], 30),
    ([2920.0, 27210.6, 2304.6], 30),
    ([54560.5, -24648.6, 1813.0], 30),
    ([-61266.4, 57059.9, -1588.0], 30),
    ([20564.4, -29511.1, 9748.7], 30),
    ([-28892.3, 22853.4, -5147.0], 30),
];
const DIMENSION: [f32; 3] = [37971.9, 37971.9, 113915.7];

fn load(gd: &GameData, statics: &Arc<StaticWorld>, tweaks: &SimTweaks) -> (Universe, LevelState) {
    let src = GameDataSource(gd);
    let mut u = Universe::new(statics.clone(), tweaks.clone(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
    u.single_player = true;
    let ctx = LevelContext { single_player: true, mission: 6, fleet_modifier: 0.0, resources: ResourceTweaks::load(&src) };
    let mut st = LevelState::default();
    let (dir, file) = level::mission_paths(6, None);
    level::load_level(&mut u, &src, &dir, &file, &ctx, &mut st).expect("mission 6 loads");
    (u, st)
}

#[test]
fn mission06_nebulae() {
    let gd = match GameData::from_env() {
        Some(Ok(gd)) => gd,
        _ => {
            eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
            return;
        }
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let (u, st) = load(&gd, &statics, &tweaks);
    assert!(!st.warnings.iter().any(|w| w.contains("unknown resource type")), "{:?}", st.warnings);
    assert!(!st.unmodelled.iter().any(|(k, v)| k == "Resources" && v.contains("Nebula")));

    let tw = &u.nebulae.tweaks;
    assert_eq!((tw.fog_red, tw.fog_green, tw.fog_blue), (1.0, 0.25, 0.4), "Nebulae.script read");
    assert_eq!(tw.pick_distance_minthresh, 2300.0);
    assert_eq!(u.nebulae.list.len(), LINES.len());

    let mut total = 0;
    for (neb, (centre, tendrils)) in u.nebulae.list.iter().zip(LINES) {
        assert_eq!(neb.tendrils.len(), tendrils);
        assert!(neb.chunks.len() > tendrils, "{} chunks", neb.chunks.len());
        for c in &neb.chunks {
            for k in 0..3 {
                let p = [c.position.x, c.position.y, c.position.z][k];
                assert!((p - centre[k]).abs() <= DIMENSION[k] * 0.5 + 1.0, "chunk at {:?} outside the box of {centre:?}", c.position);
            }
            assert_eq!(c.flags & (chunk_flags::INACTIVE | chunk_flags::DEAD), 0);
            let r = u.base.resources.get(c.resource.expect("chunk resource")).expect("resource exists");
            assert_eq!(r.kind, ResourceKind::Nebula(0));
            assert_eq!(r.position, c.position);
            assert_eq!(r.resource_value, 100);
        }
        for t in &neb.tendrils {
            let (a, b) = t.ends().expect("every tendril is used");
            assert_ne!(a, b);
            assert!(!t.has_geometry(), "no camera, no geometry");
        }
        // Chains of more than one segment are drawn.
        assert!(neb.tendrils.iter().any(|t| t.flags & tendril_flags::INACTIVE == 0));
        total += neb.chunks.len();
    }
    let nebula_resources = u.base.resources.list.iter().filter(|r| matches!(r.kind, ResourceKind::Nebula(_))).count();
    assert_eq!(nebula_resources, total);
    eprintln!(
        "mission 6: {total} nebula chunks ({:?}); first chunk {:?}",
        u.nebulae.list.iter().map(|n| n.chunks.len()).collect::<Vec<_>>(),
        u.nebulae.list[0].chunks[0].position
    );

    // Regression golden of this implementation (not a capture from the original game).
    assert_eq!(u.nebulae.list.iter().map(|n| n.chunks.len()).collect::<Vec<_>>(), [33, 32, 32, 32, 33, 33, 32, 32]);
    assert_eq!(u.nebulae.list[0].chunks[0].position, Vector::new(-31453.477, 62997.79, -8439.027));

    // Deterministic.
    let (again, _) = load(&gd, &statics, &tweaks);
    assert_eq!(again.nebulae, u.nebulae);
    assert_eq!(again.randy, u.randy);
}

#[test]
fn mission06_fog_and_geometry_follow_the_camera() {
    let gd = match GameData::from_env() {
        Some(Ok(gd)) => gd,
        _ => {
            eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
            return;
        }
    };
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let (mut u, _) = load(&gd, &statics, &tweaks);
    let c = u.nebulae.list[3].chunks[0].position;
    let game_stream = u.randy.number_get(hwc_math::random::RAN_GAME);
    nebula::set_fog(&mut u);
    assert!(!u.nebulae.fog.on, "no camera, no fog");
    u.nebulae.camera = Some(NebulaCamera { eye: Vector::new(c.x + 3000.0, c.y, c.z), look_at: c, clip_far: 35_000.0 });
    nebula::set_fog(&mut u);
    assert!(u.nebulae.fog.sum > 0.0);
    let made: usize = u.nebulae.list.iter().map(|n| n.tendrils.iter().filter(|t| t.has_geometry()).count()).sum();
    assert!(made > 0, "the visible tendrils get cylinders");
    for n in &u.nebulae.list {
        for t in n.tendrils.iter().filter(|t| t.has_geometry()) {
            assert_eq!(t.lods[0].vertices.len(), 36);
            assert_eq!(t.lods[1].vertices.len(), 12);
            assert!(t.radius > 0.0);
        }
    }
    assert_eq!(u.randy.number_get(hwc_math::random::RAN_GAME), game_stream, "the game stream is untouched");
}
