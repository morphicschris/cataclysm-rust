//! Loads every static from the real game data and checks the result. Uses `HWC_GAME_DIR`
//! (`cataclysm.big` inside it) or, failing that, `HWC_EXTRACTED_DIR` (an extracted archive);
//! skipped when neither is set. Run with `--nocapture` for a per-race summary.

use hwc_formats::big::BigArchive;
use hwc_sim::statics::*;

macro_rules! check {
    ($v:expr, $cond:expr, $($fmt:tt)+) => {
        let ok: bool = $cond;
        if !ok {
            $v.push(format!($($fmt)+));
        }
    };
}

fn load() -> Option<StaticWorld> {
    if let Some(dir) = std::env::var_os("HWC_GAME_DIR") {
        let path = std::path::Path::new(&dir).join("cataclysm.big");
        let big = BigArchive::open(&path).unwrap_or_else(|e| panic!("opening {}: {e}", path.display()));
        return Some(StaticWorld::load(&big));
    }
    if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
        let src = DirSource::new(&dir).expect("indexing extracted directory");
        return Some(StaticWorld::load_from(&src, LoadOptions::default()));
    }
    eprintln!("HWC_GAME_DIR / HWC_EXTRACTED_DIR not set; skipping");
    None
}

#[test]
fn loads_everything() {
    let Some(w) = load() else { return };

    // Ships per race: (types in the race, types with statics).
    let expected = [
        (ShipRace::Somtaaw, 33, 33),
        (ShipRace::Beast, 27, 27),
        (ShipRace::Kushan, 14, 14),
        (ShipRace::Taiidan, 18, 18),
        (ShipRace::Turanic, 6, 6),
        (ShipRace::Traders, 11, 9),
        (ShipRace::Derelict, 36, 36),
    ];
    for (race, types, loaded) in expected {
        let n = w.ships_of_race(race).count();
        println!("{race}: {n}/{} ship types", race.ship_types().count());
        let missing: Vec<_> = race.ship_types().filter(|t| w.ship(*t).is_none()).collect();
        println!("  without statics: {missing:?}");
        assert_eq!(race.ship_types().count(), types);
        assert_eq!(n, loaded, "{race}");
    }

    let mut gun_count = 0;
    let mut problems: Vec<String> = Vec::new();
    // Oddities of the shipped data that the original tolerates.
    let mut quirks: Vec<String> = Vec::new();
    for s in w.ships.iter().flatten() {
        let name = s.ship_type.name();
        check!(problems, s.ship_type.race() == s.ship_race, "{name}: race");
        // Mesh and LOD.
        let Some(lod) = s.header.lod.as_ref() else {
            problems.push(format!("{name}: no LOD"));
            continue;
        };
        check!(problems, !lod.levels.is_empty(), "{name}: empty LOD");
        for (i, l) in lod.levels.iter().enumerate() {
            if l.kind == LodType::Mesh && !l.mesh_exists {
                quirks.push(format!("{name}: LOD {i} mesh missing"));
            }
        }
        check!(problems, lod.levels[0].kind == LodType::Mesh, "{name}: LOD 0 not a mesh");
        check!(problems, s.header.mex.is_some(), "{name}: no mex");
        if s.header.mex_is_default {
            quirks.push(format!("{name}: default mex"));
        }
        // Physics and collision.
        check!(problems, s.header.mass > 0.0, "{name}: mass");
        check!(problems, s.header.coll.coll_sphere_size > 0.0, "{name}: no collision sphere");
        check!(problems, s.header.coll.avoid_coll_sphere_pad >= s.header.coll.avoid_coll_sphere_size, "{name}");
        check!(problems, s.health.max_health > 0.0 && s.health.one_over_max_health > 0.0, "{name}: health");
        check!(problems, s.health.min_coll_damage > 0.0, "{name}: no collision damage");
        check!(problems, s.ship_class.is_some(), "{name}: class {:?}", s.ship_class_raw);
        check!(problems, s.build_cost >= 0 && s.build_time >= 0, "{name}");
        check!(problems, s.force_field_protection_distance_scale > 1.0, "{name}");
        // Guns.
        if let Some(guns) = &s.guns {
            for g in guns {
                gun_count += 1;
                check!(problems, g.gun_type.is_some(), "{name}: gun {} type", g.gun_index);
                check!(problems, g.mex_point_found, "{name}: gun {} not in mex", g.gun_index);
                check!(problems, g.bullet_range > 0.0, "{name}: gun {} range", g.gun_index);
                check!(problems, g.start_upgrade_level <= g.end_upgrade_level || g.end_upgrade_level == WeaponType::Projectile,
                    "{name}: gun {} upgrade levels", g.gun_index);
                check!(problems, g.obsolete_keywords.is_empty(), "{name}: obsolete gun keywords");
                check!(problems, (g.gun_normal.mag_sq() - 1.0).abs() < 1e-4, "{name}");
                let mut lr = g.bullet_range;
                lr *= lr;
                check!(problems, s.bullet_range_squared[0] >= lr * 0.999, "{name}: range squared");
            }
            check!(problems, s.bullet_range[0] >= s.min_bullet_range[0], "{name}: ranges");
        }
        if let Some(dock) = &s.dock {
            for p in &dock.points {
                if p.dock_type.is_none() {
                    quirks.push(format!("{name}: dock point {} has no valid type", p.name));
                }
            }
        }
        if let Some(lights) = &s.nav_lights {
            for l in lights {
                check!(problems, l.light_type.is_some(), "{name}: nav light {}", l.name);
            }
        }
        if let Some(t) = s.beast_attack_convert_to {
            check!(problems, w.ship(t).is_some() || t == ShipType::bWorker, "{name}: converts to unloaded {t}");
        }
    }
    for p in &problems {
        println!("PROBLEM {p}");
    }
    assert!(problems.is_empty(), "{} problems", problems.len());
    let mut expected_quirks: Vec<String> = [
        "sWorker: dock point Grinder has no valid type",
        "sWorker: dock point gun0 has no valid type",
        "bWorker: dock point Grinder has no valid type",
        "bWorker: dock point gun0 has no valid type",
        "trJunkYardHQ: dock point dawg has no valid type",
        "dCrystal0: dock point center has no valid type",
        "dCrystal1: dock point center has no valid type",
        "sRepairDroid: LOD 1 mesh missing",
        "sRepairDroid: LOD 2 mesh missing",
        "sRepairDroid: LOD 3 mesh missing",
        "sRepairDroid: LOD 4 mesh missing",
        "nisLightSource1: default mex",
        "nisLightSource2: default mex",
        "nisLensflareSource1: default mex",
    ]
    .map(String::from)
    .to_vec();
    expected_quirks.sort();
    quirks.sort();
    assert_eq!(quirks, expected_quirks);
    println!("{gun_count} guns");
    assert!(gun_count > 250);

    // Spot checks against the files.
    let acolyte = w.ship(ShipType::sAcolyte).unwrap();
    assert_eq!(acolyte.ship_class, Some(ShipClass::Fighter));
    assert_eq!(acolyte.build_cost, 75);
    assert_eq!(acolyte.build_time, 12 * 16);
    assert_eq!(acolyte.health.max_health, 220.0);
    assert_eq!(acolyte.header.max_velocity, 825.0);
    assert_eq!(acolyte.guidance.thrust_strength[4], 1000.0 * 10.0);
    assert_eq!(acolyte.beast_attack_convert_to, Some(ShipType::bAcolyte));
    let guns = acolyte.guns.as_ref().unwrap();
    assert_eq!(guns.len(), 4);
    assert_eq!(guns[0].gun_type, Some(GunType::Gimble));
    assert_eq!(guns[2].gun_type, Some(GunType::MissileLauncher));
    assert_eq!(guns[0].end_upgrade_level, WeaponType::Energy);
    assert_eq!(acolyte.bullet_range, [5000.0; 3]);
    assert_eq!(acolyte.min_bullet_range, [4500.0; 3]);
    assert!((guns[0].bullet_life_time - 4500.0 / 4000.0).abs() < 1e-6);
    assert_eq!(acolyte.formation_padding_modifier, w.tweaks.strikecraft_padding_modifier);

    // Missiles and mines per race.
    let have = |v: &Vec<Option<MissileStaticInfo>>| -> Vec<ShipRace> {
        v.iter().flatten().filter_map(|m| m.race).collect()
    };
    use ShipRace::*;
    assert_eq!(have(&w.missiles), vec![Somtaaw, Beast, Kushan, Taiidan, Turanic]);
    assert_eq!(have(&w.mines), vec![Somtaaw, Beast, Kushan, Taiidan, Derelict]);
    for m in w.mines.iter().flatten() {
        assert!(m.mine_range > 0.0 && (m.mine_range * m.mine_range - m.mine_range_sqr).abs() <= m.mine_range_sqr * 1e-5);
    }
    assert!(w.mine_max_diameter > 0.0);

    // Resources and derelicts.
    assert_eq!(w.asteroids.iter().flatten().count(), 5);
    assert_eq!(w.dust_clouds.iter().flatten().count(), 4);
    assert_eq!(w.nebulae.iter().flatten().count(), 1);
    assert_eq!(w.derelicts.iter().flatten().count(), 8);
    let a2 = w.asteroids[2].as_ref().unwrap();
    assert_eq!(a2.info.resource_value, 400);
    assert_eq!(a2.break_pieces.len(), 2);
    assert_eq!(a2.break_pieces[0].asteroid_type, 1);
    for d in w.dust_clouds.iter().flatten() {
        let c = &d.info.header.coll;
        assert_eq!(c.coll_sphere_size, w.tweaks.dustcloud_radial_scale * w.tweaks.dustcloud_radii[d.kind.index()]);
    }
    let gate = w.derelicts[DerelictType::HyperspaceGate.index()].as_ref().unwrap();
    assert_eq!(gate.health.max_health, w.tweaks.hyperspace_gate_health);

    // Tweaks the scripts change.
    assert_eq!(w.tweaks.retaliate_zone, 7000.0);
    assert_eq!(w.tweaks.coll_damage_default_min_fraction, 0.1);
    assert_eq!(w.tweaks.strikecraft_padding_modifier, 1.3);
    assert!((w.tweaks.corvette_bank - 70f32.to_radians().sin()).abs() < 1e-6);
    assert_eq!(w.tweaks.fighter_bank, Tweaks::default().fighter_bank);

    // Research: dependencies acyclic and ships belong to the table's race.
    for (label, table, race) in [
        ("SP Somtaaw", &w.research.single_player_somtaaw, Somtaaw),
        ("MP Somtaaw", &w.research.multiplayer_somtaaw, Somtaaw),
        ("Beast", &w.research.beast, Beast),
    ] {
        let order = table.topological_order().unwrap_or_else(|t| panic!("{label}: cycle through {t}"));
        assert_eq!(order.len(), Technology::COUNT);
        assert!(!table.ship_depend.is_empty(), "{label}");
        for (ship, deps) in &table.ship_depend {
            assert_eq!(ship.race(), race, "{label}: {ship}");
            assert!(!deps.is_empty(), "{label}: {ship}");
        }
        let timed = table.tech_time.iter().filter(|&&t| t > 0).count();
        println!("{label}: {} ship deps, {timed} timed techs", table.ship_depend.len());
        assert!(timed >= 39, "{label}");
    }
    assert!(w.research.warnings.is_empty(), "{:?}", w.research.warnings);

    // Unit caps and team colours.
    assert_eq!(w.unit_caps.total, 297);
    assert_eq!(w.unit_caps.cap(ShipType::sCarrier), 2);
    assert!(!w.unit_cap_presets.is_empty());
    assert_eq!(w.team_colors.schemes[0].base, [66, 108, 202, 255]);
    assert_eq!(w.team_colors.resource, [157, 118, 91, 255]);

    println!("{} warnings", w.warnings.len());
    for wn in w.warnings.iter().take(40) {
        println!("  {wn}");
    }
}

#[test]
fn custom_keywords_survey() {
    let Some(w) = load() else { return };
    let mut keys: std::collections::BTreeMap<&str, usize> = Default::default();
    for s in w.ships.iter().flatten() {
        for (k, _) in &s.custom_entries {
            *keys.entry(k.as_str()).or_default() += 1;
        }
    }
    println!("{} keywords left for per-type static init:", keys.len());
    for (k, n) in &keys {
        println!("  {k} {n}");
    }
}

