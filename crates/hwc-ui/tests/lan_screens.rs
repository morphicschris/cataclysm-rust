//! The LAN screens on the real game files (needs `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped
//! otherwise): Connection Method's LAN, the login, the game list filled from what the
//! application found, joining (Connecting, then the player's waiting room with its players and
//! chat), and creating a game (the option tabs, then the host's waiting room and Start).
//! Pictures go to `screenshots/front_end/lan_*.png`, in the original and the modern look.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::lan::{GameRow, LanRequest};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

fn session(modern: bool) -> Option<Session> {
    let data = GameData::from_env()?.expect("game data");
    let src: Rc<dyn AssetSource> = Rc::new(data);
    let mut s = Session::new(src, Setup { scale: ScaleMode::Original, ..Default::default() }).expect("session");
    if !modern {
        s.use_original_look();
    }
    s.resize(1280, 960);
    Some(s)
}

fn frames(s: &mut Session, n: usize) -> Vec<Request> {
    (0..n).flat_map(|_| s.update(1.0 / 60.0)).collect()
}

/// Runs a named callback as a click on its button would.
fn press(s: &mut Session, name: &str) -> Vec<Request> {
    frames(s, 2);
    let a = s.fe.find_atom(name).unwrap_or_else(|| panic!("no {name} on {:?}", s.fe.stack_names()));
    s.fe.function_execute(name, Some(a), false);
    frames(s, 2)
}

fn lan(reqs: &[Request]) -> Vec<LanRequest> {
    reqs.iter()
        .filter_map(|r| match r {
            Request::Lan(l) => Some(l.clone()),
            _ => None,
        })
        .collect()
}

fn shot(s: &mut Session, name: &str, modern: bool) {
    frames(s, 4);
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/front_end");
    let _ = std::fs::create_dir_all(&dir);
    let look = if modern { "modern" } else { "original" };
    s.render().save_png(&dir.join(format!("lan_{name}_{look}.png"))).expect("png");
}

fn screen(s: &Session) -> Option<String> {
    s.fe.current_screen_name().map(str::to_string)
}

fn run(modern: bool) {
    let Some(mut s) = session(modern) else { return };
    frames(&mut s, 10);
    press(&mut s, "FE_Multiplayer_Game");
    assert_eq!(screen(&s).as_deref(), Some("Connection_Method"));
    press(&mut s, "MG_LANIPX");
    assert_eq!(screen(&s).as_deref(), Some("LAN_Login"));
    s.fe.game.skirmish.name = "Tester".into();
    shot(&mut s, "login", modern);
    let r = press(&mut s, "MG_LaunchLAN");
    assert_eq!(lan(&r), vec![LanRequest::Browse(true)]);
    assert_eq!(screen(&s).as_deref(), Some("Available_Games"));
    // The application found two games.
    s.fe.game.lan.games = vec![
        GameRow { name: "Kiith Somtaaw".into(), host: "Alice".into(), map: "Coldwar3".into(), players: 1, max_players: 3, started: false, compatible: true },
        GameRow { name: "Old build".into(), host: "Bob".into(), map: "Coldwar2".into(), players: 1, max_players: 2, started: false, compatible: false },
    ];
    s.fe.game.lan.dirty = true;
    hwc_ui::lan::refresh(&mut s.fe);
    let a = s.fe.find_atom("MG_ListOfGames").expect("game list");
    assert_eq!(s.fe.list(a).map(|l| l.items.len()), Some(2));
    assert!(s.fe.list(a).unwrap().items[0].text.contains("Alice"));
    shot(&mut s, "games", modern);
    // Join the first.
    s.fe.list_mut(a).unwrap().select(0);
    let r = press(&mut s, "MG_JoinGame");
    assert_eq!(lan(&r), vec![LanRequest::Join(0)]);
    assert_eq!(screen(&s).as_deref(), Some("Connecting"));
    // The host takes us in.
    s.fe.game.lan.players = vec!["Alice  -  Somtaaw  (host)".into(), "Tester  -  Beast  (ready)".into(), "Computer  -  Beast".into()];
    s.fe.game.lan.chat = vec!["Alice: welcome".into()];
    hwc_ui::lan::joined(&mut s.fe);
    s.fe.game.lan.dirty = true;
    hwc_ui::lan::refresh(&mut s.fe);
    assert_eq!(screen(&s).as_deref(), Some("Player_Wait"));
    let p = s.fe.find_atom("MG_GameUserName").expect("players");
    assert_eq!(s.fe.list(p).map(|l| l.items.len()), Some(3));
    shot(&mut s, "player_wait", modern);
    let r = press(&mut s, "MG_LeaveGame");
    assert_eq!(lan(&r), vec![LanRequest::Leave]);
    assert_eq!(screen(&s).as_deref(), Some("Available_Games"));
    // Create a game.
    press(&mut s, "MG_CreateGame");
    assert_eq!(screen(&s).as_deref(), Some("Basic_Options"));
    shot(&mut s, "create", modern);
    let r = press(&mut s, "MG_CreateGameNow");
    let reqs = lan(&r);
    assert!(matches!(reqs.as_slice(), [LanRequest::Host { game_name, .. }] if game_name == "Tester's game"), "{reqs:?}");
    assert_eq!(screen(&s).as_deref(), Some("Captain_Wait"));
    s.fe.game.lan.players = vec!["Tester  -  Somtaaw  (host)".into(), "Alice  -  Beast  (ready)".into()];
    s.fe.game.lan.can_start = true;
    s.fe.game.lan.dirty = true;
    hwc_ui::lan::refresh(&mut s.fe);
    shot(&mut s, "captain_wait", modern);
    let r = press(&mut s, "MG_StartGame");
    assert_eq!(lan(&r), vec![LanRequest::Start]);
}

#[test]
fn lan_screens_original_look() {
    run(false);
}

#[test]
fn lan_screens_modern_look() {
    run(true);
}
