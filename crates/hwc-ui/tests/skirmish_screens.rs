//! The skirmish screens on the real game files (needs `HWC_DATA_DIR` or `HWC_GAME_DIR`;
//! skipped otherwise): Connection Method's Skirmish, the option tabs, the scenario list and
//! Start Skirmish's request. Pictures go to `screenshots/front_end/skirmish_*.png`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

fn session() -> Option<Session> {
    let data = GameData::from_env()?.expect("game data");
    let src: Rc<dyn AssetSource> = Rc::new(data);
    let mut s = Session::new(
        src,
        Setup {
            scale: ScaleMode::Original,
            ..Default::default()
        },
    )
    .expect("session");
    s.use_original_look();
    s.resize(640, 480);
    Some(s)
}

fn frames(s: &mut Session, n: usize) -> Vec<Request> {
    (0..n).flat_map(|_| s.update(1.0 / 60.0)).collect()
}

fn click_rect(s: &mut Session, x: i32, y: i32) -> Vec<Request> {
    let mut out = frames(s, 32);
    s.fe.mouse_move(x, y);
    out.extend(frames(s, 1));
    s.mouse_button(true, true);
    out.extend(frames(s, 1));
    s.mouse_button(true, false);
    out.extend(frames(s, 1));
    out
}

/// The `n`th atom called `name` on the screens.
fn click(s: &mut Session, name: &str, n: usize) -> Vec<Request> {
    frames(s, 2);
    let first =
        s.fe.find_atom(name)
            .unwrap_or_else(|| panic!("no {name} on {:?}", s.fe.stack_names()));
    let mut atoms = s.fe.atoms_named(first.screen, name);
    // In screen order (top to bottom), as the radio values go.
    atoms.sort_by_key(|&a| (s.fe.atom(a).radio_value, s.fe.atom(a).rect().y0));
    let a = *atoms
        .get(n)
        .unwrap_or_else(|| panic!("no {name} #{n} on {:?}", s.fe.stack_names()));
    let r = s.fe.atom(a).rect();
    click_rect(s, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn shot(s: &mut Session, name: &str) {
    frames(s, 4);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/front_end");
    let _ = std::fs::create_dir_all(&dir);
    s.render()
        .save_png(&dir.join(format!("skirmish_{name}.png")))
        .expect("png");
}

#[test]
fn skirmish_screens_set_up_and_start_a_game() {
    let Some(mut s) = session() else { return };
    frames(&mut s, 10);
    click(&mut s, "FE_Multiplayer_Game", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Connection_Method")
    );
    click(&mut s, "MG_Skirmish", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Skirmish_Basic")
    );
    let sk = &s.fe.game.skirmish;
    assert_eq!(sk.types.len(), 4, "gametypes.script");
    assert_eq!(sk.scenarios.len(), 10, "multiPlayerMissions.script");
    assert_eq!(sk.options.num_computers, 1);
    assert_eq!(sk.type_index, 0, "Default");
    shot(&mut s, "basic");

    // Pick "Carpe Diem" (the fourth game type button).
    click(&mut s, "MG_GameType", 3);
    assert_eq!(s.fe.game.skirmish.type_index, 3);
    assert_eq!(
        s.fe.game.skirmish.options.flags & 0x8,
        0,
        "Carpe Diem turns research off"
    );
    // A win condition off: no game type matches, the custom button lights.
    click(&mut s, "MG_LastMotherShip", 0);
    assert_eq!(s.fe.game.skirmish.type_index, 4, "custom");

    // The scenario list: pick the last one.
    click(&mut s, "MG_ChooseScenario", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Load_a_scenario")
    );
    let list = s.fe.find_atom("CS_ScenarioWindowInit").expect("list");
    let rows = s.fe.list(list).map_or(0, |l| l.items.len());
    assert_eq!(rows, 10);
    let r = s.fe.atom(list).rect();
    // Third row.
    click_rect(&mut s, r.x0 + 30, r.y0 + 4 + 18 * 2 + 9);
    shot(&mut s, "scenarios");
    let picked = s.fe.game.skirmish.picking.expect("a row");
    click(&mut s, "CS_Done", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Skirmish_Basic")
    );
    assert_eq!(s.fe.game.skirmish.scenario, picked);

    // More CPU players, within the map.
    let max = s.fe.game.skirmish.scenarios[picked].max_players;
    for _ in 0..10 {
        click(&mut s, "MG_AddCPUOpponent", 0);
    }
    assert_eq!(
        usize::from(s.fe.game.skirmish.options.num_computers),
        max - 1
    );
    click(&mut s, "MG_RemoveCPUOpponent", 0);
    assert_eq!(
        usize::from(s.fe.game.skirmish.options.num_computers),
        (max - 1).max(2) - 1
    );

    // The other tabs.
    click(&mut s, "Skirmish_Basic-TO-Skirmish_Advanced", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Skirmish_Advanced")
    );
    click(&mut s, "MG_CratesEnabled", 0);
    assert_ne!(s.fe.game.skirmish.options.flags & 0x1000, 0, "crates on");
    shot(&mut s, "advanced");
    click(&mut s, "Skirmish_Advanced-TO-Skirmish_Resource", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Skirmish_Resource")
    );
    click(&mut s, "MG_StartingResources", 2);
    assert_eq!(s.fe.game.skirmish.options.starting_resources, 2);
    shot(&mut s, "resource");

    // Player setup: play the Beast.
    click(&mut s, "Skirmish_Resource-TO-Skirmish_Colour", 0);
    assert_eq!(
        s.fe.current_screen_name().as_deref(),
        Some("Skirmish_Colour")
    );
    click(&mut s, "CP_RaceTwo", 0);
    assert_eq!(s.fe.game.skirmish.race, 1);
    shot(&mut s, "player_setup");

    // Start.
    let out = click(&mut s, "StartNewGame", 0);
    let req = out.iter().find_map(|r| match r {
        Request::StartSkirmish(q) => Some(q.clone()),
        _ => None,
    });
    let q = req.expect("Start Skirmish asks the game to start");
    assert_eq!(q.map, s.fe.game.skirmish.scenarios[picked].name);
    assert_eq!(q.options.starting_resources, 2);
    assert_ne!(q.options.flags & 0x1000, 0);
    assert_eq!(q.race, 1);
    println!("start: {q:?}");
}
