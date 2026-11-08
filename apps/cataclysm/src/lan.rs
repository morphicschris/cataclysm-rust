//! The application's side of the LAN and internet screens ([`hwc_ui::lan`]): the game
//! browser looking for games on the local network, the connection to the lobby server for
//! internet games (`hwcLobbyServer` in the options, `HWC_LOBBY` in the environment, else this
//! machine), the session of the game being hosted or joined, and what the screens show of
//! them. When the game starts, the session goes to the game ([`NetStart`] →
//! [`crate::net_game::NetLink`]).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use hwc_net::discovery::Browser;
use hwc_net::internet::{LobbyClient, LobbyEvent, LobbyMsg, ListedGame};
use hwc_net::tcp::Conn;
use hwc_net::lobby::{LobbyRequest, SlotKind};
use hwc_net::session::{Session, SessionEvent};
use hwc_net::{DISCOVERY_PORT, GAME_PORT};
use hwc_sim::skirmish::GameSetup;
use hwc_sim::statics::ShipRace;
use hwc_ui::cataclysm::Cataclysm;
use hwc_ui::fe::FrontEnd;
use hwc_ui::lan::{GameRow, LanRequest};

type Fe = FrontEnd<Cataclysm>;

/// How often the browser asks the network, and how long a silent game stays listed.
const QUERY_EVERY: Duration = Duration::from_secs(2);
const FORGET_AFTER: Duration = Duration::from_secs(7);

/// A network game about to load: its setup (this machine's player in it), the players' names
/// and the session that plays it.
#[derive(Debug)]
pub struct NetStart {
    pub setup: GameSetup,
    pub names: Vec<String>,
    pub session: Session,
}

/// The LAN screens' network.
#[derive(Debug, Default)]
pub struct LanFront {
    browser: Option<Browser>,
    last_query: Option<Instant>,
    session: Option<Session>,
    /// The lobby server, for internet games.
    lobby: Option<LobbyClient>,
    /// A connection to a game being made (through the relay it can take seconds).
    joining: Option<std::thread::JoinHandle<std::io::Result<Conn>>>,
    /// What the lobby was last told of our game.
    listed: Option<hwc_net::discovery::GameInfo>,
}

/// The lobby server's address.
fn lobby_address(fe: &Fe) -> String {
    let o = fe.game.options.get_str("hwcLobbyServer").trim().to_string();
    if !o.is_empty() {
        return o;
    }
    std::env::var("HWC_LOBBY").unwrap_or_else(|_| format!("127.0.0.1:{}", hwc_net::internet::LOBBY_PORT))
}

fn race_of(r: u8) -> ShipRace {
    if r == 1 {
        ShipRace::Beast
    } else {
        ShipRace::Somtaaw
    }
}

impl LanFront {
    /// A request from the screens.
    pub fn request(&mut self, fe: &mut Fe, r: LanRequest) -> Option<NetStart> {
        match r {
            LanRequest::Browse(on) => {
                if on {
                    if self.browser.is_none() {
                        match Browser::new() {
                            Ok(b) => self.browser = Some(b),
                            Err(e) => eprintln!("cataclysm: the game browser cannot start: {e}"),
                        }
                    }
                    self.last_query = None;
                } else {
                    self.browser = None;
                    fe.game.lan.games.clear();
                }
            }
            LanRequest::Refresh => {
                if let Some(b) = self.browser.as_mut() {
                    b.found.clear();
                }
                self.last_query = None;
            }
            LanRequest::Host { game_name, request, max_players } => {
                self.leave();
                let name = request.name.trim().to_string();
                let bind = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, GAME_PORT);
                // A second game on this machine takes any port, without answering the browser
                // (an internet game is found through the lobby).
                let discovery = (!fe.game.lan.internet).then_some(DISCOVERY_PORT);
                let made = Session::host(&name, &game_name, &request.map, max_players, request.options.clone(), bind, discovery)
                    .or_else(|_| Session::host(&name, &game_name, &request.map, max_players, request.options.clone(), SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0), None));
                match made {
                    Ok(mut s) => {
                        s.request(LobbyRequest::Race(race_of(request.race)));
                        // The computer players take the last seats, alternating races.
                        let n = s.lobby.as_ref().map_or(0, |l| l.slots.len());
                        for i in 0..usize::from(request.options.num_computers).min(n.saturating_sub(1)) {
                            let race = if (i + usize::from(request.race)) % 2 == 0 { ShipRace::Beast } else { ShipRace::Somtaaw };
                            s.set_slot(n - 1 - i, SlotKind::Computer, race);
                        }
                        fe.game.lan.game_name = game_name;
                        if let (Some(l), Some(info)) = (self.lobby.as_mut(), s.info()) {
                            // Not behind a firewall, or finding out: the server tries to reach us.
                            l.send(LobbyMsg::HostGame { info: info.clone(), try_direct: fe.game.lan.firewall != 1 });
                            self.listed = Some(info);
                        }
                        self.session = Some(s);
                    }
                    Err(e) => hwc_ui::lan::back_to_games(fe, &format!("Cannot host a game: {e}")),
                }
            }
            LanRequest::Join(i) if fe.game.lan.internet => {
                self.leave();
                let Some(l) = self.lobby.as_ref() else { return None };
                let Some(g): Option<ListedGame> = l.games.get(i).cloned() else {
                    hwc_ui::lan::back_to_games(fe, "That game is gone");
                    return None;
                };
                let server = l.server();
                fe.game.lan.game_name = g.info.name.clone();
                fe.game.lan.status.push(if g.direct.is_some() { "Connecting to the host...".into() } else { "Connecting through the lobby's relay...".into() });
                fe.game.lan.dirty = true;
                self.joining = Some(std::thread::spawn(move || hwc_net::internet::connect_game(server, &g, Duration::from_secs(10))));
            }
            LanRequest::Join(i) => {
                self.leave();
                let found = self.browser.as_ref().and_then(|b| b.found.get(i)).cloned();
                let Some(f) = found else {
                    hwc_ui::lan::back_to_games(fe, "That game is gone");
                    return None;
                };
                let name = fe.game.skirmish.name.trim().to_string();
                match Session::join(f.game_addr(), &name, Duration::from_secs(3)) {
                    Ok(s) => {
                        fe.game.lan.game_name = f.info.name.clone();
                        fe.game.lan.status.push(format!("Connected to {}", f.game_addr()));
                        fe.game.lan.dirty = true;
                        self.session = Some(s);
                    }
                    Err(e) => hwc_ui::lan::back_to_games(fe, &format!("Cannot reach {}: {e}", f.game_addr())),
                }
            }
            LanRequest::Chat(text) => {
                if let Some(s) = self.session.as_mut() {
                    s.chat(&text);
                }
            }
            LanRequest::Race(r) => {
                if let Some(s) = self.session.as_mut() {
                    s.request(LobbyRequest::Race(race_of(r)));
                }
            }
            LanRequest::Start => {
                let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.subsec_nanos());
                let s = self.session.as_mut()?;
                let setup = s.start(seed, hwc_net::lockstep::DEFAULT_DELAY)?;
                let names = s.game.as_ref().map(|g| g.names.clone()).unwrap_or_default();
                s.poll();
                let session = self.session.take()?;
                self.browser = None;
                if self.listed.take().is_some() {
                    if let Some(l) = self.lobby.as_mut() {
                        l.send(LobbyMsg::EndGame);
                    }
                }
                self.lobby = None;
                return Some(NetStart { setup, names, session });
            }
            LanRequest::Leave => self.leave(),
            LanRequest::Login { name, password, firewall: _ } => {
                self.logout();
                let at = lobby_address(fe);
                match LobbyClient::connect(at.as_str(), &name, &password, &hwc_net::build_id(), Duration::from_secs(5)) {
                    Ok(l) => {
                        fe.game.lan.status.push(format!("Connected to {at}"));
                        fe.game.lan.dirty = true;
                        self.lobby = Some(l);
                    }
                    Err(e) => hwc_ui::lan::back_to_login(fe, &format!("Cannot reach the lobby at {at}: {e}")),
                }
            }
            LanRequest::Logout => self.logout(),
            LanRequest::RoomChat(text) => {
                if let Some(l) = self.lobby.as_mut() {
                    l.send(LobbyMsg::Chat { text });
                }
            }
            LanRequest::ListRooms => {
                if let Some(l) = self.lobby.as_mut() {
                    l.send(LobbyMsg::ListRooms);
                }
            }
            LanRequest::JoinRoom(i) => {
                if let Some(l) = self.lobby.as_mut() {
                    if let Some(r) = l.rooms.get(i).cloned() {
                        l.send(LobbyMsg::JoinRoom { name: r.name, password: String::new() });
                    }
                }
            }
            LanRequest::CreateRoom { name, description, password } => {
                if let Some(l) = self.lobby.as_mut() {
                    l.send(LobbyMsg::CreateRoom { name, description, password });
                }
            }
        }
        None
    }

    fn leave(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.leave();
        }
        if self.listed.take().is_some() {
            if let Some(l) = self.lobby.as_mut() {
                l.send(LobbyMsg::EndGame);
            }
        }
        self.joining = None;
    }

    fn logout(&mut self) {
        self.leave();
        self.lobby = None;
    }

    /// Every frame of the front end: the browser asks and listens, the session reads the
    /// network, and the screens show what changed. A game that starts comes back.
    pub fn update(&mut self, fe: &mut Fe) -> Option<NetStart> {
        if let Some(b) = self.browser.as_mut() {
            if self.last_query.is_none_or(|t| t.elapsed() >= QUERY_EVERY) {
                let here = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, DISCOVERY_PORT));
                b.query(DISCOVERY_PORT, &[here]);
                self.last_query = Some(Instant::now());
            }
            b.poll(FORGET_AFTER);
            let build = hwc_net::build_id();
            let rows: Vec<GameRow> = b
                .found
                .iter()
                .map(|f| GameRow {
                    name: f.info.name.clone(),
                    host: f.info.host_name.clone(),
                    map: f.info.map.clone(),
                    players: f.info.players,
                    max_players: f.info.max_players,
                    started: f.info.started,
                    compatible: f.info.build == build,
                })
                .collect();
            if rows != fe.game.lan.games {
                fe.game.lan.games = rows;
                fe.game.lan.dirty = true;
            }
        }
        let mut started = None;
        // The lobby: logging in, the room, its chat, the rooms and the games.
        if let Some(l) = self.lobby.as_mut() {
            l.poll();
            for e in std::mem::take(&mut l.events) {
                match e {
                    LobbyEvent::LoggedIn => hwc_ui::lan::logged_in(fe),
                    LobbyEvent::Refused(why) if !l.logged_in => {
                        hwc_ui::lan::back_to_login(fe, &format!("Refused: {why}"));
                    }
                    LobbyEvent::Refused(why) => fe.game.lan.room_chat.push(format!("({why})")),
                    LobbyEvent::Chat { from, text } => fe.game.lan.room_chat.push(format!("{from}: {text}")),
                    LobbyEvent::Disconnected => {
                        hwc_ui::lan::back_to_login(fe, "The lobby server has gone");
                    }
                    LobbyEvent::Room | LobbyEvent::Rooms | LobbyEvent::Games => {}
                }
                fe.game.lan.dirty = true;
            }
            if l.room != fe.game.lan.room || l.users != fe.game.lan.room_users {
                fe.game.lan.room = l.room.clone();
                fe.game.lan.room_users = l.users.clone();
                fe.game.lan.dirty = true;
            }
            let rooms: Vec<String> = l.rooms.iter().map(|r| format!("{}  -  {}  ({}){}", r.name, r.description, r.users, if r.locked { "  locked" } else { "" })).collect();
            if rooms != fe.game.lan.rooms {
                fe.game.lan.rooms = rooms;
                fe.game.lan.dirty = true;
            }
            if fe.game.lan.internet {
                let build = hwc_net::build_id();
                let rows: Vec<GameRow> = l
                    .games
                    .iter()
                    .map(|g| GameRow {
                        name: g.info.name.clone(),
                        host: g.info.host_name.clone(),
                        map: g.info.map.clone(),
                        players: g.info.players,
                        max_players: g.info.max_players,
                        started: g.info.started,
                        compatible: g.info.build == build,
                    })
                    .collect();
                if rows != fe.game.lan.games {
                    fe.game.lan.games = rows;
                    fe.game.lan.dirty = true;
                }
            }
            // Players coming through the relay to our game.
            let incoming = std::mem::take(&mut l.incoming);
            let server = l.server();
            for pipe in incoming {
                match hwc_net::internet::accept_pipe(server, pipe, Duration::from_secs(5)) {
                    Ok(c) => {
                        if let Some(s) = self.session.as_mut() {
                            s.adopt(c);
                        }
                    }
                    Err(e) => eprintln!("cataclysm: a player could not come through the relay: {e}"),
                }
            }
            // The lobby hears how our game is doing.
            if let (Some(s), Some(l)) = (self.session.as_ref(), self.lobby.as_mut()) {
                if let Some(info) = s.info().filter(|i| Some(i) != self.listed.as_ref()) {
                    l.send(LobbyMsg::UpdateGame { info: info.clone() });
                    self.listed = Some(info);
                }
            }
        }
        // A connection to an internet game being made.
        if self.joining.as_ref().is_some_and(|h| h.is_finished()) {
            match self.joining.take().expect("joining").join() {
                Ok(Ok(conn)) => {
                    let name = fe.game.skirmish.name.trim().to_string();
                    self.session = Some(Session::join_over(conn, &name));
                }
                Ok(Err(e)) => hwc_ui::lan::back_to_games(fe, &format!("Cannot reach the game: {e}")),
                Err(_) => hwc_ui::lan::back_to_games(fe, "Cannot reach the game"),
            }
        }
        if let Some(s) = self.session.as_mut() {
            s.poll();
            let was_welcomed = s.client.is_some();
            for e in std::mem::take(&mut s.events) {
                match e {
                    SessionEvent::LobbyChanged => {
                        if !s.is_host() && was_welcomed {
                            // A player who joined is ready at once (the host starts when it likes).
                            if let Some(l) = &s.lobby {
                                if l.slot_of(s.client.unwrap_or(u32::MAX)).is_some_and(|i| !l.slots[i].ready) {
                                    s.request(LobbyRequest::Ready(true));
                                    let race = fe.game.skirmish.race;
                                    s.request(LobbyRequest::Race(race_of(race)));
                                }
                            }
                            hwc_ui::lan::joined(fe);
                        }
                    }
                    SessionEvent::Chat { from, text } => {
                        fe.game.lan.chat.push(format!("{from}: {text}"));
                        fe.game.lan.dirty = true;
                    }
                    SessionEvent::Refused(why) => {
                        started = Some(Err(format!("Refused: {why}")));
                    }
                    SessionEvent::Disconnected => {
                        started = Some(Err("The host has gone".to_string()));
                    }
                    SessionEvent::Started { setup, names } => {
                        started = Some(Ok((setup, names)));
                    }
                    _ => {}
                }
            }
            if let Some(l) = &s.lobby {
                let lines: Vec<String> = l
                    .slots
                    .iter()
                    .filter_map(|slot| {
                        let race = if slot.race == ShipRace::Beast { "Beast" } else { "Somtaaw" };
                        match &slot.kind {
                            SlotKind::Human { name, client } => {
                                let tag = if *client == 0 { "  (host)" } else if slot.ready { "  (ready)" } else { "" };
                                Some(format!("{name}  -  {race}{tag}"))
                            }
                            SlotKind::Computer => Some(format!("Computer  -  {race}")),
                            SlotKind::Open => Some("(open)".to_string()),
                            SlotKind::Closed => None,
                        }
                    })
                    .collect();
                let can_start = s.is_host() && l.can_start();
                if lines != fe.game.lan.players || can_start != fe.game.lan.can_start {
                    fe.game.lan.players = lines;
                    fe.game.lan.can_start = can_start;
                    fe.game.lan.dirty = true;
                }
            }
        }
        match started {
            Some(Ok((setup, names))) => {
                let session = self.session.take().expect("session");
                self.browser = None;
                // The game leaves the lobby's list; the lobby connection goes with the game
                // ending (players come back to log in again).
                if self.listed.take().is_some() {
                    if let Some(l) = self.lobby.as_mut() {
                        l.send(LobbyMsg::EndGame);
                    }
                }
                self.lobby = None;
                return Some(NetStart { setup, names, session });
            }
            Some(Err(why)) => {
                self.leave();
                hwc_ui::lan::back_to_games(fe, &why);
                fe.game.lan.dirty = true;
            }
            None => {}
        }
        hwc_ui::lan::refresh(fe);
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::rc::Rc;

    use hwc_formats::GameData;
    use hwc_ui::scale::ScaleMode;
    use hwc_ui::session::{Session as UiSession, Setup};
    use hwc_ui::AssetSource;

    fn ui(data: &Rc<GameData>, name: &str) -> UiSession {
        let source: Rc<dyn AssetSource> = data.clone();
        let mut s = UiSession::new(source, Setup { scale: ScaleMode::Original, ..Default::default() }).expect("session");
        s.resize(1280, 960);
        s.update(1.0 / 60.0);
        s.fe.game.skirmish.name = name.to_string();
        s
    }

    /// Runs the front ends' frames: their requests go to their networks.
    fn pump(uis: &mut [&mut UiSession], nets: &mut [&mut LanFront], starts: &mut [Option<NetStart>]) {
        for i in 0..uis.len() {
            for r in uis[i].update(1.0 / 60.0) {
                if let hwc_ui::cataclysm::Request::Lan(l) = r {
                    if let Some(s) = nets[i].request(&mut uis[i].fe, l) {
                        starts[i] = Some(s);
                    }
                }
            }
            if let Some(s) = nets[i].update(&mut uis[i].fe) {
                starts[i] = Some(s);
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }

    fn press(s: &mut UiSession, name: &str) {
        let a = s.fe.find_atom(name).unwrap_or_else(|| panic!("no {name} on {:?}", s.fe.stack_names()));
        s.fe.function_execute(name, Some(a), false);
    }

    /// Two copies of the game on this machine: one hosts a LAN game with a computer player,
    /// the other finds it in the game list, joins as the Beast, and the host starts: both get
    /// the same game, each with its own player.
    #[test]
    fn two_front_ends_host_find_join_and_start() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else { return };
        let Ok(data) = GameData::open(&dir) else { return };
        let data = Rc::new(data);
        let (mut a, mut b) = (ui(&data, "Hoster"), ui(&data, "Joiner"));
        let (mut na, mut nb) = (LanFront::default(), LanFront::default());
        let mut starts: [Option<NetStart>; 2] = [None, None];
        for s in [&mut a, &mut b] {
            press(s, "FE_Multiplayer_Game");
            s.update(1.0 / 60.0);
            press(s, "MG_LANIPX");
            s.update(1.0 / 60.0);
            press(s, "MG_LaunchLAN");
        }
        pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        // The host makes a game with one computer player.
        press(&mut a, "MG_CreateGame");
        a.update(1.0 / 60.0);
        a.fe.game.skirmish.options.num_computers = 1;
        let three = a.fe.game.skirmish.scenarios.iter().position(|m| m.max_players >= 3).expect("a map for three");
        a.fe.game.skirmish.scenario = three;
        press(&mut a, "MG_CreateGameNow");
        // The other finds it.
        let end = Instant::now() + Duration::from_secs(10);
        while b.fe.game.lan.games.is_empty() && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        let row = b.fe.game.lan.games.first().cloned().expect("the game is listed");
        assert_eq!(row.host, "Hoster");
        assert!(row.compatible);
        b.fe.game.skirmish.race = 1;
        let list = b.fe.find_atom("MG_ListOfGames").expect("list");
        b.fe.list_mut(list).unwrap().select(0);
        press(&mut b, "MG_JoinGame");
        let end = Instant::now() + Duration::from_secs(10);
        while !(a.fe.game.lan.can_start && b.fe.current_screen_name() == Some("Player_Wait")) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert_eq!(b.fe.current_screen_name(), Some("Player_Wait"));
        assert!(a.fe.game.lan.players.iter().any(|l| l.contains("Joiner") && l.contains("Beast")), "{:?}", a.fe.game.lan.players);
        // A line of chat goes across.
        na.request(&mut a.fe, LanRequest::Chat("glhf".into()));
        let end = Instant::now() + Duration::from_secs(5);
        while !b.fe.game.lan.chat.iter().any(|l| l.contains("glhf")) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert!(b.fe.game.lan.chat.iter().any(|l| l == "Hoster: glhf"), "{:?}", b.fe.game.lan.chat);
        press(&mut a, "MG_StartGame");
        let end = Instant::now() + Duration::from_secs(10);
        while starts.iter().any(Option::is_none) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        let [Some(sa), Some(sb)] = starts else { panic!("both start") };
        assert_eq!(sa.setup.local_player, 0);
        assert_eq!(sb.setup.local_player, 1);
        let strip = |mut s: GameSetup| {
            s.local_player = 0;
            s
        };
        assert_eq!(strip(sa.setup.clone()), strip(sb.setup.clone()), "the same game");
        assert_eq!(sa.setup.players.len(), 3);
        assert!(sa.setup.players[2].computer);
        assert_eq!(sb.setup.players[1].race, ShipRace::Beast);
        assert_eq!(sa.names, sb.names);
    }

    /// Internet play on this machine: a lobby server runs on its own; two front ends log in,
    /// meet in the room's chat, one hosts as if behind a firewall (its game is joined through
    /// the server's relay), the other finds it in the room's game list and joins, and both
    /// start the same game.
    #[test]
    fn two_front_ends_meet_in_the_lobby_and_play_through_the_relay() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else { return };
        let Ok(data) = GameData::open(&dir) else { return };
        let data = Rc::new(data);
        let mut server = hwc_net::internet::LobbyServer::bind("127.0.0.1:0", &hwc_net::build_id()).expect("server");
        let at = server.local_addr().unwrap().to_string();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop2 = stop.clone();
        let server_thread = std::thread::spawn(move || {
            while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
                server.poll();
                std::thread::sleep(Duration::from_millis(1));
            }
        });
        let (mut a, mut b) = (ui(&data, "Hoster"), ui(&data, "Joiner"));
        let (mut na, mut nb) = (LanFront::default(), LanFront::default());
        let mut starts: [Option<NetStart>; 2] = [None, None];
        for s in [&mut a, &mut b] {
            s.fe.game.options.set_str("hwcLobbyServer", &at);
            press(s, "FE_Multiplayer_Game");
            s.update(1.0 / 60.0);
            press(s, "MG_InternetWON");
            s.update(1.0 / 60.0);
        }
        // The host is behind a firewall.
        a.fe.game.lan.firewall = 1;
        for s in [&mut a, &mut b] {
            press(s, "MG_LaunchWON");
        }
        let end = Instant::now() + Duration::from_secs(10);
        while !(a.fe.current_screen_name() == Some("Channel_Chat") && b.fe.game.lan.room_users.len() == 2) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert_eq!(b.fe.current_screen_name(), Some("Channel_Chat"));
        assert_eq!(b.fe.game.lan.room, hwc_net::internet::MAIN_ROOM);
        na.request(&mut a.fe, LanRequest::RoomChat("hello room".into()));
        let end = Instant::now() + Duration::from_secs(5);
        while !b.fe.game.lan.room_chat.iter().any(|l| l == "Hoster: hello room") && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert!(b.fe.game.lan.room_chat.iter().any(|l| l == "Hoster: hello room"), "{:?}", b.fe.game.lan.room_chat);
        // Both to the room's games; the host makes one.
        for s in [&mut a, &mut b] {
            press(s, "MG_ViewGames");
            s.update(1.0 / 60.0);
        }
        press(&mut a, "MG_CreateGame");
        a.update(1.0 / 60.0);
        press(&mut a, "MG_CreateGameNow");
        let end = Instant::now() + Duration::from_secs(10);
        while b.fe.game.lan.games.is_empty() && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert_eq!(b.fe.game.lan.games.first().map(|g| g.host.as_str()), Some("Hoster"));
        let list = b.fe.find_atom("MG_ListOfGames").expect("list");
        b.fe.list_mut(list).unwrap().select(0);
        press(&mut b, "MG_JoinGame");
        let end = Instant::now() + Duration::from_secs(15);
        while !(a.fe.game.lan.can_start && b.fe.current_screen_name() == Some("Player_Wait")) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        assert!(a.fe.game.lan.can_start, "the host can start: {:?}", a.fe.game.lan.players);
        press(&mut a, "MG_StartGame");
        let end = Instant::now() + Duration::from_secs(10);
        while starts.iter().any(Option::is_none) && Instant::now() < end {
            pump(&mut [&mut a, &mut b], &mut [&mut na, &mut nb], &mut starts);
        }
        let [Some(sa), Some(sb)] = starts else { panic!("both start") };
        assert_eq!((sa.setup.local_player, sb.setup.local_player), (0, 1));
        assert_eq!(sa.setup.seed, sb.setup.seed);
        assert_eq!(sa.names, sb.names);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        server_thread.join().unwrap();
    }
}
