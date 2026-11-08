//! The application's side of the LAN screens ([`hwc_ui::lan`]): the game browser looking for
//! games on the local network, the session of the game being hosted or joined, and what the
//! screens show of them. When the game starts, the session goes to the game
//! ([`NetStart`] → [`crate::net_game::NetLink`]).

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::{Duration, Instant};

use hwc_net::discovery::Browser;
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
                // A second game on this machine takes any port, without answering the browser.
                let made = Session::host(&name, &game_name, &request.map, max_players, request.options.clone(), bind, Some(DISCOVERY_PORT))
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
                        self.session = Some(s);
                    }
                    Err(e) => hwc_ui::lan::back_to_games(fe, &format!("Cannot host a game: {e}")),
                }
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
                return Some(NetStart { setup, names, session });
            }
            LanRequest::Leave => self.leave(),
        }
        None
    }

    fn leave(&mut self) {
        if let Some(mut s) = self.session.take() {
            s.leave();
        }
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
}
