//! Mission01 on the simulation (needs the game: `HWC_DATA_DIR` with an imported data directory
//! or `HWC_GAME_DIR` with an install; skipped when neither is set).

use std::sync::{Arc, OnceLock};

use hwc_formats::gamedata::GameData;
use hwc_kas::{kas_hash, Mission};
use hwc_sim::harness::world_hash;
use hwc_sim::mission::{load_kas_mission, sp_state, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    gd: GameData,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    script: Mission,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let gd = match GameData::from_env() {
            Some(Ok(gd)) => gd,
            _ => {
                eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
                return None;
            }
        };
        let script = match load_kas_mission(&gd, "Mission01") {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{e}; skipping");
                return None;
            }
        };
        let src = GameDataSource(&gd);
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
        let tweaks = SimTweaks::load(&src);
        Some(Data { gd, statics, tweaks, script })
    })
    .as_ref()
}

fn start(d: &Data) -> MissionRun {
    let mut config = MissionConfig::new(1);
    config.log_calls = false;
    // Intel speech ends after 3 s (standing in for the subtitles) to keep the run short.
    config.speech_seconds = Some(3.0);
    MissionRun::start(d.statics.clone(), d.tweaks.clone(), &GameDataSource(&d.gd), d.script.clone(), config).expect("mission starts")
}

/// Runs until the first objective exists (at most `limit` updates); returns the update it
/// appeared on and a fingerprint of the state then.
fn run_to_first_objective(d: &Data, limit: u32) -> (u32, u64, Vec<(u32, Option<u32>)>, MissionRun) {
    let mut run = start(d);
    for _ in 0..limit {
        run.step();
        if !run.game.objectives.is_empty() {
            break;
        }
    }
    let states = run.kas.teams().map(|t| (t.label, t.state)).collect();
    (run.universe.update_counter, world_hash(&run.universe), states, run)
}

#[test]
fn mission01_level_loads_in_original_order() {
    let Some(d) = data() else { return };
    let run = start(d);
    let teams: Vec<u32> = run.game.level.teams.iter().map(|t| t.label).collect();
    // Teams are created in the order their labels first appear in the sphere files.
    let first = ["FleetIntel", "Objectives", "KushanFlankFrigates", "TaiidanBombers", "TaiidanCarrier", "TaiidanResourcers"];
    for (i, n) in first.iter().enumerate() {
        assert_eq!(teams[i], kas_hash(n), "team {i}");
    }
    assert_eq!(*teams.last().unwrap(), kas_hash("KushanProximityArray"));
    assert_eq!(teams.len(), 36);
    // FleetIntel names a ship line with 0..1 recons: none at fleet strength 0.
    assert_eq!(run.game.level.team(kas_hash("FleetIntel")).unwrap().ships.len(), 0);
    // TaiidanBombers: 11 bombers and 3 heavy interceptors from two lines.
    let bombers = run.game.level.team(kas_hash("TaiidanBombers")).unwrap();
    assert_eq!((bombers.ships.len(), bombers.orig_count), (14, 14));
    assert_eq!(run.universe.ship_list.len(), 154);
    // The first ship line of the level is ship 0.
    assert_eq!(bombers.ships[0].0, 5);
    assert!(run.game.level.point(kas_hash("StartPointPlayer")).is_some());
    assert!(run.game.level.volume(kas_hash("BattleZone")).is_some());
    assert_eq!(run.game.level.resources.len(), 16);
    // The asteroids are harvestable simulation objects, numbered in creation order.
    let ids: Vec<u16> = run.universe.base.resources.list.iter().map(|r| r.id.0).collect();
    assert_eq!(ids, (0..16).collect::<Vec<u16>>());
    assert!(run.universe.base.resources.list.iter().all(|r| r.resource_value > 0));
    assert_eq!(run.game.sp_state, sp_state::START_KAS);
}

#[test]
fn mission01_reaches_first_objective_deterministically() {
    let Some(d) = data() else { return };
    let (frame_a, hash_a, states_a, run) = run_to_first_objective(d, 2000);
    assert_eq!(run.game.objectives.len(), 1, "first objective after {frame_a} updates");
    assert_eq!(run.game.objectives[0].label, "DestroyBombers");
    assert_eq!(run.game.objectives[0].brief, "Destroy Taiidani Bombers");
    assert_eq!(run.var(None, "G_ShipsHaveArrived"), Some(1));
    assert_eq!(run.var(None, "G_Nislet1Complete"), Some(1));
    assert!(run.game.ships_have_arrived);
    assert_eq!(run.game.sp_state, sp_state::NONE);
    // The player's mothership arrived and is what GetPlayerMothership returns.
    let ms = run.game.level.motherships[0].expect("player mothership");
    assert_eq!(run.universe.ship(ms).unwrap().player, 0);
    // FleetIntel has started its intel cycle; the Kushan guards belong to the player now.
    assert!(run.team_state("FleetIntel").is_some());
    let guard = run.game.level.team(kas_hash("KushanMainGuardFrigates")).unwrap();
    assert!(guard.ships.iter().all(|&s| run.universe.ship(s).is_some_and(|s| s.player == 0)));

    let (frame_b, hash_b, states_b, _) = run_to_first_objective(d, 2000);
    assert_eq!(frame_a, frame_b);
    assert_eq!(hash_a, hash_b);
    assert_eq!(states_a, states_b);
}

/// The construction manager's list in Mission 1 is the original's: the level's IncludeShips
/// (no Mimic, no Super Acolyte, which `ExcludeShips RACE_DERELICT,All` leaves unloaded), the
/// research the mission gives (`TechSet TECH_DRIVE_FIGHTER` makes Acolytes buildable), and the
/// modules KAS switches off (`EnableShipConstruction`).
#[test]
fn mission01_construction_list_matches_the_original() {
    use hwc_sim::statics::ShipType as T;
    let Some(d) = data() else { return };
    let mut run = start(d);
    while !run.game.ships_have_arrived && run.universe.update_counter < 2000 {
        run.step();
    }
    let ms = run.game.level.motherships[0].expect("player mothership");
    let list = hwc_sim::level::construction_list(&run.universe, ms);
    assert_eq!(list, vec![T::sRecon, T::sAcolyte, T::sRammingFrigate, T::sWorker, T::sProcessor, T::sMothershipDockingBay]);
    assert!(!hwc_sim::level::construction_offers(&run.universe, ms, T::dAcolyte));
    let fighter = hwc_sim::statics::Technology::from_name("TECH_DRIVE_FIGHTER").unwrap();
    assert_eq!(run.universe.base.players[0].tech.has[fighter.index()], 1);
}

/// `GetWorldResources` is the resource units left in the harvestable resources, and drops as
/// the player's workers harvest them.
#[test]
fn mission01_world_resources_drop_as_asteroids_are_harvested() {
    use hwc_sim::resource::world_resources;
    use hwc_sim::statics::ShipType as T;
    let Some(d) = data() else { return };
    let mut run = start(d);
    while !run.game.ships_have_arrived && run.universe.update_counter < 2000 {
        run.step();
    }
    let before = world_resources(&run.universe);
    assert!(before > 0);
    let workers: Vec<_> = run
        .universe
        .ship_list
        .iter()
        .copied()
        .filter(|&id| run.universe.ship(id).is_some_and(|s| s.player == 0 && s.ship_type == T::sWorker))
        .collect();
    assert_eq!(workers.len(), 2);
    hwc_sim::resource::cl_collect_resource(&mut run.universe, &workers, None);
    for _ in 0..16 * 240 {
        run.step();
        if world_resources(&run.universe) < before {
            break;
        }
    }
    let after = world_resources(&run.universe);
    assert!(after < before, "no resource units harvested ({before} left)");
}

/// The single-player research gating: the research manager does not offer linking until the
/// mission's script allows it (`TechSetResearch("TECH_LINK")`, when the bombers' objective is
/// done), and offers it from then on (once its lab is there).
#[test]
fn mission01_linking_is_unavailable_until_the_script_allows_it() {
    let Some(d) = data() else { return };
    let link = hwc_sim::statics::Technology::from_name("TECH_LINK").expect("TECH_LINK").index();
    let mut run = start(d);
    let mut ap = hwc_sim::autoplay::Autoplay::new(16 * 4500);
    let mut allowed_at = None;
    for _ in 0..16 * 3000 {
        ap.tick(&mut run);
        run.step();
        let u = &run.universe;
        let allowed = hwc_sim::research::research_allowed(u, 0, link);
        let offered = hwc_sim::research::available_topics(u, 0).contains(&link);
        if !allowed {
            assert!(!offered, "linking offered at update {} before the script allowed it", u.update_counter);
        } else {
            allowed_at.get_or_insert(u.update_counter);
            let lab = hwc_sim::research::tech_lab(link);
            let lab_here = lab < 0 || u.base.players[0].labs[lab as usize].status != hwc_sim::base::lab_status::ABSENT;
            if lab_here && !u.base.players[0].tech.contains(link) && !hwc_sim::research::researching(u, 0, link) {
                assert!(offered, "linking allowed but not offered at update {}", u.update_counter);
            }
        }
        if run.mission_complete() {
            break;
        }
    }
    assert!(allowed_at.is_some(), "the script allows linking during the mission");
    // The KAS pings came through as orders: the resourcers' ping and its removal.
    use hwc_sim::mission::PingOrder;
    let pings = &run.game.ping_orders;
    assert!(pings.iter().any(|p| matches!(p, PingOrder::Ships { label, ships } if label == "ResourcerPing" && !ships.is_empty())), "{pings:?}");
    assert!(pings.iter().any(|p| matches!(p, PingOrder::Remove { label } if label == "ResourcerPing")), "{pings:?}");
    // Nothing the script never allowed is offered.
    let u = &run.universe;
    for t in hwc_sim::research::available_topics(u, 0) {
        assert!(hwc_sim::research::research_allowed(u, 0, t), "technology {t} offered without TechSetResearch");
    }
}

/// The Taiidan resource collectors fly to the unexplored region and, once discovered, harvest:
/// the computer player has an AI player, so they find resources (they sat still before).
#[test]
fn mission01_taiidan_resourcers_move_and_harvest() {
    let Some(d) = data() else { return };
    let mut run = start(d);
    let mut ap = hwc_sim::autoplay::Autoplay::new(16 * 4500);
    let team = kas_hash("TaiidanResourcers");
    let first: Vec<_> = run.game.level.team(team).map(|t| t.ships.clone()).unwrap_or_default();
    assert!(!first.is_empty(), "the level has the resource collectors");
    let p0: Vec<_> = first.iter().map(|&id| run.universe.ships.ship(id).posinfo.position).collect();
    let (mut moved, mut harvested) = (false, false);
    for _ in 0..16 * 2000 {
        ap.tick(&mut run);
        run.step();
        let u = &run.universe;
        for (k, &id) in first.iter().enumerate() {
            let Some(s) = u.ships.get(id) else { continue };
            moved |= (s.posinfo.position - p0[k]).mag_sq() > 1000.0 * 1000.0;
            harvested |= s.base.resources > 0;
        }
        if moved && harvested {
            break;
        }
    }
    assert!(moved, "the resource collectors move");
    assert!(harvested, "the resource collectors harvest");
}

/// `singlePlayerLoadNewLevel` ends with `levelFogOfWarCreate`: the human player gets a fog
/// grid with its own ships as viewers, and the Taiidan harvesters and carriers far away are
/// not seen through it at the start (`fowShipVisibleToPlayer`).
#[test]
fn mission01_starts_with_the_players_fog_of_war() {
    let Some(d) = data() else { return };
    let mut run = start(d);
    assert!(run.universe.fog.grids.first().is_some_and(Option::is_some), "the human player's fog grid");
    assert!(run.universe.fog.grids.iter().skip(1).all(Option::is_none), "only the local player has a grid");
    // The fleet comes out of hyperspace and its ships become viewers.
    let own_ships = |u: &hwc_sim::Universe| -> Vec<hwc_sim::ShipId> {
        u.ship_list.iter().copied().filter(|&id| u.ship(id).is_some_and(|s| s.player == 0)).collect()
    };
    for _ in 0..3000 {
        if !own_ships(&run.universe).is_empty() {
            break;
        }
        run.step();
    }
    let u = &run.universe;
    let grid = u.fog.grids[0].as_ref().unwrap();
    let own = own_ships(u);
    assert!(!own.is_empty(), "the fleet arrives");
    for &id in &own {
        if hwc_sim::fow::provides_vision(u, id) {
            let s = u.ship(id).unwrap();
            let n = grid.point_viewer_count(s.posinfo.position);
            assert!(n > 0, "an own ship sees its own cell: {:?} at {:?} count {n} (update {}, grid {:?} {:?})", s.ship_type, s.posinfo.position, u.update_counter, grid.origin, grid.dims);
        }
    }
    let enemies: Vec<_> = u.ship_list.iter().copied().filter(|&id| u.ship(id).is_some_and(|s| s.player == 1)).collect();
    assert!(!enemies.is_empty(), "mission 1 has Taiidan ships");
    let hidden = enemies.iter().filter(|&&id| !hwc_sim::fow::ship_visible_to_player(u, id, 0)).count();
    assert!(hidden > 0, "some enemy ships start in the fog");
}
