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
