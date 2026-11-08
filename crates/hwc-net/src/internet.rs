//! Internet games: a lobby server in place of the original's WON servers (log in, rooms with
//! their chat and players, the games hosted in each room), and its relay for players who
//! cannot take connections (behind a firewall or NAT).
//!
//! A host lists its game with the server. When the server can connect back to the host's game
//! port the game is joined directly; otherwise a joining player opens a connection to the
//! server asking for the game, the server asks the host to open one too, and from then on the
//! server passes the bytes between the two: the game's own protocol ([`crate::wire::Msg`])
//! runs over that pipe as over a direct connection.

use std::collections::BTreeMap;
use std::io;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::discovery::GameInfo;
use crate::tcp::{Conn, Listener};

/// The lobby server's port.
pub const LOBBY_PORT: u16 = 28472;
/// The room every player starts in.
pub const MAIN_ROOM: &str = "Lobby";

/// A room of the lobby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomInfo {
    pub name: String,
    pub description: String,
    pub users: u32,
    pub locked: bool,
}

/// A game the lobby lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedGame {
    pub id: u32,
    pub info: GameInfo,
    /// Where it can be joined directly (the server reached it), else through the relay.
    pub direct: Option<SocketAddr>,
}

/// The lobby protocol.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LobbyMsg {
    // To the server.
    Login { protocol: u32, build: String, name: String, password: String },
    ListRooms,
    JoinRoom { name: String, password: String },
    CreateRoom { name: String, description: String, password: String },
    Chat { text: String },
    /// A game hosted in our room (on our game port; whether it may be reached directly).
    HostGame { info: GameInfo, try_direct: bool },
    UpdateGame { info: GameInfo },
    EndGame,
    ListGames,
    /// This connection becomes a pipe to the host of the game.
    RelayConnect { game: u32 },
    /// This connection (the host's) becomes the other end of the pipe.
    RelayAccept { pipe: u32 },
    // From the server.
    LoggedIn { you: u32 },
    Refused { reason: String },
    Rooms(Vec<RoomInfo>),
    Room { name: String, users: Vec<String> },
    ChatLine { from: String, text: String },
    Games(Vec<ListedGame>),
    /// To a host: someone joins through the relay; open a pipe with this number.
    RelayIncoming { pipe: u32 },
    /// The pipe is open: the bytes that follow are the game's.
    RelayReady,
}

#[derive(Debug)]
struct Room {
    description: String,
    password: String,
}

#[derive(Debug)]
struct User {
    conn: Conn,
    /// Logged in as.
    name: Option<String>,
    room: String,
    game: Option<u32>,
}

#[derive(Debug)]
struct Game {
    owner: u32,
    info: GameInfo,
    room: String,
    direct: Option<SocketAddr>,
}

/// A connection that has become (half of) a pipe.
#[derive(Debug)]
struct PipeEnd {
    conn: Conn,
    /// Bytes for the other side that came before it was there.
    held: Vec<u8>,
}

#[derive(Debug)]
struct Pipe {
    client: PipeEnd,
    host: Option<PipeEnd>,
    opened: Instant,
}

/// The lobby server. [`LobbyServer::poll`] runs it; the `hwc-lobby` program calls it in a loop.
#[derive(Debug)]
pub struct LobbyServer {
    listener: Listener,
    build: String,
    /// Connections that have not said what they are yet.
    fresh: Vec<Conn>,
    users: BTreeMap<u32, User>,
    rooms: BTreeMap<String, Room>,
    games: BTreeMap<u32, Game>,
    pipes: BTreeMap<u32, Pipe>,
    next_id: u32,
    /// The answers of the checks whether a host can be reached.
    reach_tx: mpsc::Sender<(u32, Option<SocketAddr>)>,
    reach_rx: mpsc::Receiver<(u32, Option<SocketAddr>)>,
}

impl LobbyServer {
    /// A server on `addr` for players running `build`.
    pub fn bind(addr: impl ToSocketAddrs, build: &str) -> io::Result<Self> {
        let (reach_tx, reach_rx) = mpsc::channel();
        let mut rooms = BTreeMap::new();
        rooms.insert(MAIN_ROOM.to_string(), Room { description: "Where everyone starts".into(), password: String::new() });
        Ok(LobbyServer { listener: Listener::bind(addr)?, build: build.to_string(), fresh: Vec::new(), users: BTreeMap::new(), rooms, games: BTreeMap::new(), pipes: BTreeMap::new(), next_id: 1, reach_tx, reach_rx })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.listener.local_addr()
    }

    fn id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }

    /// Takes connections in, answers, and passes the pipes' bytes.
    pub fn poll(&mut self) {
        self.fresh.extend(self.listener.accept());
        // What a new connection is: a login, or a pipe's end.
        let fresh = std::mem::take(&mut self.fresh);
        for mut c in fresh {
            match c.poll_one_as::<LobbyMsg>() {
                Some(LobbyMsg::Login { protocol, build, name, password: _ }) => {
                    let name = name.trim().chars().filter(|c| !c.is_control()).take(24).collect::<String>();
                    if protocol != crate::PROTOCOL || build != self.build {
                        c.send_any(&LobbyMsg::Refused { reason: format!("a different build ({build}; the server is for {})", self.build) });
                        c.flush();
                        continue;
                    }
                    if name.is_empty() || self.users.values().any(|u| u.name.as_deref() == Some(name.as_str())) {
                        c.send_any(&LobbyMsg::Refused { reason: format!("the name {name:?} is taken") });
                        c.flush();
                        continue;
                    }
                    let id = self.id();
                    c.send_any(&LobbyMsg::LoggedIn { you: id });
                    self.users.insert(id, User { conn: c, name: Some(name), room: MAIN_ROOM.to_string(), game: None });
                    self.send_room(MAIN_ROOM);
                    self.send_games_to(id);
                }
                Some(LobbyMsg::RelayConnect { game }) => {
                    let Some(owner) = self.games.get(&game).map(|g| g.owner) else {
                        c.send_any(&LobbyMsg::Refused { reason: "no such game".into() });
                        c.flush();
                        continue;
                    };
                    let pipe = self.id();
                    let held = c.poll_raw();
                    self.pipes.insert(pipe, Pipe { client: PipeEnd { conn: c, held }, host: None, opened: Instant::now() });
                    if let Some(u) = self.users.get_mut(&owner) {
                        u.conn.send_any(&LobbyMsg::RelayIncoming { pipe });
                    }
                }
                Some(LobbyMsg::RelayAccept { pipe }) => {
                    let held = c.poll_raw();
                    match self.pipes.get_mut(&pipe) {
                        Some(p) if p.host.is_none() => {
                            c.send_any(&LobbyMsg::RelayReady);
                            p.client.conn.send_any(&LobbyMsg::RelayReady);
                            // What each side sent early goes over now.
                            c.send_raw(&std::mem::take(&mut p.client.held));
                            p.client.conn.send_raw(&held);
                            p.host = Some(PipeEnd { conn: c, held: Vec::new() });
                        }
                        _ => {}
                    }
                }
                Some(_) => {}
                None if c.is_closed() => {}
                None => self.fresh.push(c),
            }
        }
        // The users.
        let ids: Vec<u32> = self.users.keys().copied().collect();
        let mut gone = Vec::new();
        for id in ids {
            let msgs = match self.users.get_mut(&id) {
                Some(u) => u.conn.poll_as::<LobbyMsg>(),
                None => continue,
            };
            for m in msgs {
                self.handle(id, m);
            }
            if self.users.get(&id).is_some_and(|u| u.conn.is_closed()) {
                gone.push(id);
            }
        }
        for id in gone {
            if let Some(u) = self.users.remove(&id) {
                if let Some(g) = u.game {
                    self.games.remove(&g);
                    self.send_games(&u.room);
                }
                self.send_room(&u.room);
            }
        }
        // Reachability checks.
        while let Ok((game, addr)) = self.reach_rx.try_recv() {
            if let Some(g) = self.games.get_mut(&game) {
                g.direct = addr;
                let room = g.room.clone();
                self.send_games(&room);
            }
        }
        // The pipes.
        let now = Instant::now();
        self.pipes.retain(|_, p| {
            let Some(h) = p.host.as_mut() else {
                // A host that never answers.
                return now.duration_since(p.opened) < Duration::from_secs(15) && !p.client.conn.is_closed();
            };
            let a = p.client.conn.poll_raw();
            if !a.is_empty() {
                h.conn.send_raw(&a);
            }
            let b = h.conn.poll_raw();
            if !b.is_empty() {
                p.client.conn.send_raw(&b);
            }
            h.conn.flush();
            p.client.conn.flush();
            !(p.client.conn.is_closed() && p.client.conn.pending() == 0) && !(h.conn.is_closed() && h.conn.pending() == 0)
        });
        for u in self.users.values_mut() {
            u.conn.flush();
        }
    }

    fn handle(&mut self, id: u32, m: LobbyMsg) {
        let Some(room) = self.users.get(&id).map(|u| u.room.clone()) else { return };
        match m {
            LobbyMsg::ListRooms => {
                let list = self.rooms.iter().map(|(n, r)| RoomInfo { name: n.clone(), description: r.description.clone(), users: self.users.values().filter(|u| &u.room == n).count() as u32, locked: !r.password.is_empty() }).collect();
                self.send_to(id, &LobbyMsg::Rooms(list));
            }
            LobbyMsg::CreateRoom { name, description, password } => {
                let name: String = name.trim().chars().filter(|c| !c.is_control()).take(24).collect();
                if name.is_empty() || self.rooms.contains_key(&name) {
                    self.send_to(id, &LobbyMsg::Refused { reason: "that room exists".into() });
                    return;
                }
                self.rooms.insert(name.clone(), Room { description, password: password.clone() });
                self.handle(id, LobbyMsg::JoinRoom { name, password });
            }
            LobbyMsg::JoinRoom { name, password } => {
                let Some(r) = self.rooms.get(&name) else { return };
                if !r.password.is_empty() && r.password != password {
                    self.send_to(id, &LobbyMsg::Refused { reason: "wrong password".into() });
                    return;
                }
                if let Some(u) = self.users.get_mut(&id) {
                    u.room = name.clone();
                }
                self.send_room(&room);
                self.send_room(&name);
                self.send_games_to(id);
                // Empty rooms go (not the main one).
                let empty: Vec<String> = self.rooms.keys().filter(|n| n.as_str() != MAIN_ROOM && !self.users.values().any(|u| &&u.room == n)).cloned().collect();
                for n in empty {
                    self.rooms.remove(&n);
                }
            }
            LobbyMsg::Chat { text } => {
                let from = self.users.get(&id).and_then(|u| u.name.clone()).unwrap_or_default();
                let text: String = text.chars().filter(|c| !c.is_control()).take(200).collect();
                for u in self.users.values_mut().filter(|u| u.room == room) {
                    u.conn.send_any(&LobbyMsg::ChatLine { from: from.clone(), text: text.clone() });
                }
            }
            LobbyMsg::HostGame { info, try_direct } => {
                let g = self.id();
                let addr = self.users.get(&id).and_then(|u| u.conn.peer()).map(|a| SocketAddr::new(a.ip(), info.port));
                if let Some(old) = self.users.get(&id).and_then(|u| u.game) {
                    self.games.remove(&old);
                }
                self.games.insert(g, Game { owner: id, info, room: room.clone(), direct: None });
                if let Some(u) = self.users.get_mut(&id) {
                    u.game = Some(g);
                }
                if let (true, Some(addr)) = (try_direct, addr) {
                    let tx = self.reach_tx.clone();
                    std::thread::spawn(move || {
                        let ok = TcpStream::connect_timeout(&addr, Duration::from_secs(2)).is_ok();
                        let _ = tx.send((g, ok.then_some(addr)));
                    });
                }
                self.send_games(&room);
            }
            LobbyMsg::UpdateGame { info } => {
                if let Some(g) = self.users.get(&id).and_then(|u| u.game).and_then(|g| self.games.get_mut(&g)) {
                    g.info = info;
                }
                self.send_games(&room);
            }
            LobbyMsg::EndGame => {
                if let Some(g) = self.users.get_mut(&id).and_then(|u| u.game.take()) {
                    self.games.remove(&g);
                }
                self.send_games(&room);
            }
            LobbyMsg::ListGames => self.send_games_to(id),
            _ => {}
        }
    }

    fn send_to(&mut self, id: u32, m: &LobbyMsg) {
        if let Some(u) = self.users.get_mut(&id) {
            u.conn.send_any(m);
        }
    }

    fn send_room(&mut self, room: &str) {
        let users: Vec<String> = self.users.values().filter(|u| u.room == room).filter_map(|u| u.name.clone()).collect();
        let m = LobbyMsg::Room { name: room.to_string(), users };
        for u in self.users.values_mut().filter(|u| u.room == room) {
            u.conn.send_any(&m);
        }
    }

    fn games_in(&self, room: &str) -> Vec<ListedGame> {
        self.games.iter().filter(|(_, g)| g.room == room).map(|(&id, g)| ListedGame { id, info: g.info.clone(), direct: g.direct }).collect()
    }

    fn send_games(&mut self, room: &str) {
        let m = LobbyMsg::Games(self.games_in(room));
        for u in self.users.values_mut().filter(|u| u.room == room) {
            u.conn.send_any(&m);
        }
    }

    fn send_games_to(&mut self, id: u32) {
        let Some(room) = self.users.get(&id).map(|u| u.room.clone()) else { return };
        let m = LobbyMsg::Games(self.games_in(&room));
        self.send_to(id, &m);
    }
}

/// What the lobby said.
#[derive(Debug, Clone, PartialEq)]
pub enum LobbyEvent {
    LoggedIn,
    Refused(String),
    Rooms,
    Room,
    Chat { from: String, text: String },
    Games,
    Disconnected,
}

/// A player's connection to the lobby server.
#[derive(Debug)]
pub struct LobbyClient {
    conn: Conn,
    server: SocketAddr,
    pub logged_in: bool,
    pub name: String,
    pub rooms: Vec<RoomInfo>,
    pub room: String,
    pub users: Vec<String>,
    pub games: Vec<ListedGame>,
    pub events: Vec<LobbyEvent>,
    /// Pipes the server asked us (hosting) to open.
    pub incoming: Vec<u32>,
}

impl LobbyClient {
    /// Connects and logs in.
    pub fn connect(server: impl ToSocketAddrs, name: &str, password: &str, build: &str, timeout: Duration) -> io::Result<Self> {
        let server = server.to_socket_addrs()?.next().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
        let mut conn = Conn::connect(server, timeout)?;
        conn.send_any(&LobbyMsg::Login { protocol: crate::PROTOCOL, build: build.to_string(), name: name.to_string(), password: password.to_string() });
        conn.flush();
        Ok(LobbyClient { conn, server, logged_in: false, name: name.to_string(), rooms: Vec::new(), room: String::new(), users: Vec::new(), games: Vec::new(), events: Vec::new(), incoming: Vec::new() })
    }

    pub fn server(&self) -> SocketAddr {
        self.server
    }

    pub fn send(&mut self, m: LobbyMsg) {
        self.conn.send_any(&m);
        self.conn.flush();
    }

    pub fn poll(&mut self) {
        for m in self.conn.poll_as::<LobbyMsg>() {
            match m {
                LobbyMsg::LoggedIn { .. } => {
                    self.logged_in = true;
                    self.events.push(LobbyEvent::LoggedIn);
                }
                LobbyMsg::Refused { reason } => self.events.push(LobbyEvent::Refused(reason)),
                LobbyMsg::Rooms(r) => {
                    self.rooms = r;
                    self.events.push(LobbyEvent::Rooms);
                }
                LobbyMsg::Room { name, users } => {
                    self.room = name;
                    self.users = users;
                    self.events.push(LobbyEvent::Room);
                }
                LobbyMsg::ChatLine { from, text } => self.events.push(LobbyEvent::Chat { from, text }),
                LobbyMsg::Games(g) => {
                    self.games = g;
                    self.events.push(LobbyEvent::Games);
                }
                LobbyMsg::RelayIncoming { pipe } => self.incoming.push(pipe),
                _ => {}
            }
        }
        if self.conn.is_closed() && !self.events.contains(&LobbyEvent::Disconnected) {
            self.events.push(LobbyEvent::Disconnected);
        }
        self.conn.flush();
    }

    /// A connection to the game's host: direct when the server reached it, else a pipe
    /// through the server. Returns the connection ready for the game's protocol.
    pub fn connect_game(&self, game: &ListedGame, timeout: Duration) -> io::Result<Conn> {
        connect_game(self.server, game, timeout)
    }

    /// The host's end of a pipe the server asked for.
    pub fn accept_pipe(&self, pipe: u32, timeout: Duration) -> io::Result<Conn> {
        accept_pipe(self.server, pipe, timeout)
    }
}

/// [`LobbyClient::connect_game`] with the server's address (for another thread).
pub fn connect_game(server: SocketAddr, game: &ListedGame, timeout: Duration) -> io::Result<Conn> {
    if let Some(a) = game.direct {
        if let Ok(c) = Conn::connect(a, timeout) {
            return Ok(c);
        }
    }
    let mut c = Conn::connect(server, timeout)?;
    c.send_any(&LobbyMsg::RelayConnect { game: game.id });
    c.flush();
    wait_ready(&mut c, timeout)?;
    Ok(c)
}

/// [`LobbyClient::accept_pipe`] with the server's address (for another thread).
pub fn accept_pipe(server: SocketAddr, pipe: u32, timeout: Duration) -> io::Result<Conn> {
    let mut c = Conn::connect(server, timeout)?;
    c.send_any(&LobbyMsg::RelayAccept { pipe });
    c.flush();
    wait_ready(&mut c, timeout)?;
    Ok(c)
}

/// Waits for the server's word that a pipe is open; what follows stays in the connection.
fn wait_ready(c: &mut Conn, timeout: Duration) -> io::Result<()> {
    let end = Instant::now() + timeout;
    loop {
        c.flush();
        match c.poll_one_as::<LobbyMsg>() {
            Some(LobbyMsg::RelayReady) => return Ok(()),
            Some(LobbyMsg::Refused { reason }) => return Err(io::Error::new(io::ErrorKind::ConnectionRefused, reason)),
            Some(_) => {}
            None => {
                if c.is_closed() {
                    return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "the relay closed"));
                }
                if Instant::now() >= end {
                    return Err(io::Error::new(io::ErrorKind::TimedOut, "the host did not answer through the relay"));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }
}
