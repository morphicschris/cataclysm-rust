//! A network game from one machine's side: hosting or joining, the lobby, then the running
//! game's turns. The host keeps the lobby, takes clients in, starts the game and relays every
//! client's packets to the others; when a client goes, the host tells everyone the turn the
//! game goes on without them.

use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::time::Duration;

use hwc_formats::multiplayer::GameOptions;
use hwc_sim::orders::Order;
use hwc_sim::ships::mimic::DisguiseTables;
use hwc_sim::skirmish::GameSetup;
use hwc_sim::statics::ShipRace;
use hwc_sim::universe::Universe;

use crate::discovery::{Announcer, GameInfo};
use crate::lobby::{LobbyRequest, LobbyState, SlotKind};
use crate::lockstep::{self, Lockstep};
use crate::tcp::{Conn, Listener};
use crate::wire::Msg;

/// What happened since the last look.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// The lobby changed (someone joined, left or changed their slot).
    LobbyChanged,
    Chat { from: String, text: String },
    /// The game starts with this setup (this machine's player in it).
    Started { setup: GameSetup, names: Vec<String> },
    /// The host turned us away.
    Refused(String),
    /// The connection to the host is gone.
    Disconnected,
    /// A player left the running game.
    PlayerDropped { player: usize, name: String },
    /// A player's simulation no longer matches ours.
    Desync { turn: u32, player: usize },
}

/// The running game's network side.
#[derive(Debug)]
pub struct NetGame {
    pub lockstep: Lockstep,
    pub names: Vec<String>,
    /// The client of each human player.
    pub clients: Vec<Option<u32>>,
    /// The last turn each player sent a packet for.
    last_turn: Vec<u32>,
}

#[derive(Debug)]
struct Peer {
    client: u32,
    conn: Conn,
    hello: bool,
}

#[derive(Debug)]
enum Role {
    Host { listener: Listener, peers: Vec<Peer>, next_client: u32, announcer: Option<Announcer>, port: u16 },
    Client { conn: Conn },
}

/// One machine's side of a network game.
#[derive(Debug)]
pub struct Session {
    pub name: String,
    /// Our client number (the host is 0; a client learns its own from the host).
    pub client: Option<u32>,
    role: Role,
    pub lobby: Option<LobbyState>,
    pub game: Option<NetGame>,
    pub events: Vec<SessionEvent>,
    build: String,
}

impl Session {
    /// Hosts a game on `bind`; with a discovery port, answers the game browsers there.
    pub fn host(name: &str, game_name: &str, map: &str, max_players: usize, options: GameOptions, bind: impl ToSocketAddrs, discovery_port: Option<u16>) -> io::Result<Self> {
        let listener = Listener::bind(bind)?;
        let port = listener.local_addr()?.port();
        let announcer = match discovery_port {
            Some(p) => Some(Announcer::bind(p)?),
            None => None,
        };
        Ok(Session {
            name: name.to_string(),
            client: Some(0),
            role: Role::Host { listener, peers: Vec::new(), next_client: 1, announcer, port },
            lobby: Some(LobbyState::new(game_name, name, map, max_players, options)),
            game: None,
            events: Vec::new(),
            build: crate::build_id(),
        })
    }

    /// Joins the game at `addr`.
    pub fn join(addr: impl ToSocketAddrs, name: &str, timeout: Duration) -> io::Result<Self> {
        let conn = Conn::connect(addr, timeout)?;
        Ok(Self::join_over(conn, name))
    }

    /// Joins the game at the other end of `conn` (a direct connection, or a pipe through the
    /// lobby server's relay).
    pub fn join_over(mut conn: Conn, name: &str) -> Self {
        let build = crate::build_id();
        conn.send(&Msg::Hello { protocol: crate::PROTOCOL, build: build.clone(), name: name.to_string() });
        conn.flush();
        Session { name: name.to_string(), client: None, role: Role::Client { conn }, lobby: None, game: None, events: Vec::new(), build }
    }

    /// The host takes a client that came another way (through the lobby server's relay).
    pub fn adopt(&mut self, conn: Conn) {
        if let Role::Host { peers, next_client, .. } = &mut self.role {
            peers.push(Peer { client: *next_client, conn, hello: false });
            *next_client += 1;
        }
    }

    /// A different build id (the app's, to make sure every machine runs the same program).
    pub fn with_build(mut self, build: &str) -> Self {
        self.build = build.to_string();
        if let Role::Client { conn } = &mut self.role {
            // The hello already went with the library's id; send the app's.
            conn.send(&Msg::Hello { protocol: crate::PROTOCOL, build: build.to_string(), name: self.name.clone() });
            conn.flush();
        }
        self
    }

    pub fn is_host(&self) -> bool {
        matches!(self.role, Role::Host { .. })
    }

    /// Where a host takes clients.
    pub fn local_addr(&self) -> Option<SocketAddr> {
        match &self.role {
            Role::Host { listener, .. } => listener.local_addr().ok(),
            Role::Client { .. } => None,
        }
    }

    /// What the host tells the game browsers.
    pub fn info(&self) -> Option<GameInfo> {
        let (Role::Host { port, .. }, Some(l)) = (&self.role, &self.lobby) else { return None };
        Some(GameInfo {
            name: l.name.clone(),
            host_name: self.name.clone(),
            map: l.map.clone(),
            players: l.players() as u8,
            max_players: l.slots.len() as u8,
            port: *port,
            build: self.build.clone(),
            started: self.game.is_some(),
        })
    }

    /// Reads and answers the network.
    pub fn poll(&mut self) {
        if self.is_host() {
            self.poll_host();
        } else {
            self.poll_client();
        }
    }

    fn broadcast(peers: &mut [Peer], msg: &Msg, except: Option<u32>) {
        for p in peers.iter_mut().filter(|p| p.hello && Some(p.client) != except) {
            p.conn.send(msg);
        }
    }

    fn poll_host(&mut self) {
        let info = self.info();
        let Role::Host { listener, peers, next_client, announcer, .. } = &mut self.role else { return };
        for conn in listener.accept() {
            peers.push(Peer { client: *next_client, conn, hello: false });
            *next_client += 1;
        }
        let mut lobby_changed = false;
        let mut gone: Vec<u32> = Vec::new();
        for i in 0..peers.len() {
            let msgs = peers[i].conn.poll();
            let client = peers[i].client;
            for m in msgs {
                match m {
                    Msg::Hello { protocol, build, name } => {
                        let refuse = if protocol != crate::PROTOCOL || build != self.build {
                            Some(format!("a different build ({build}; the host runs {})", self.build))
                        } else if self.game.is_some() {
                            Some("the game has started".to_string())
                        } else {
                            None
                        };
                        if let Some(reason) = refuse {
                            peers[i].conn.send(&Msg::Refused { reason });
                            gone.push(client);
                            continue;
                        }
                        if peers[i].hello {
                            continue;
                        }
                        let lobby = self.lobby.as_mut().expect("lobby");
                        if lobby.join(client, &name).is_none() {
                            peers[i].conn.send(&Msg::Refused { reason: "the game is full".to_string() });
                            gone.push(client);
                            continue;
                        }
                        peers[i].hello = true;
                        peers[i].conn.send(&Msg::Welcome { client });
                        lobby_changed = true;
                    }
                    Msg::Request(r) if peers[i].hello && self.game.is_none() => {
                        if let Some(l) = self.lobby.as_mut() {
                            l.request(client, &r);
                            lobby_changed = true;
                        }
                    }
                    Msg::Chat { text, .. } if peers[i].hello => {
                        let from = self.lobby.as_ref().and_then(|l| l.slot_of(client)).and_then(|s| match &self.lobby.as_ref().expect("lobby").slots[s].kind {
                            SlotKind::Human { name, .. } => Some(name.clone()),
                            _ => None,
                        });
                        let from = from.or_else(|| self.game.as_ref().and_then(|g| g.clients.iter().position(|&c| c == Some(client)).map(|p| g.names[p].clone()))).unwrap_or_default();
                        let msg = Msg::Chat { from: from.clone(), text: text.clone() };
                        Self::broadcast(peers, &msg, None);
                        self.events.push(SessionEvent::Chat { from, text });
                    }
                    Msg::Turn(pkt) => {
                        let Some(g) = self.game.as_mut() else { continue };
                        // A client only speaks for its own player.
                        if g.clients.get(usize::from(pkt.player)).copied().flatten() != Some(client) {
                            continue;
                        }
                        let p = usize::from(pkt.player);
                        g.last_turn[p] = g.last_turn[p].max(pkt.turn);
                        let relay = Msg::Turn(pkt.clone());
                        g.lockstep.receive(pkt);
                        Self::broadcast(peers, &relay, Some(client));
                    }
                    Msg::Leave => gone.push(client),
                    _ => {}
                }
            }
            if peers[i].conn.is_closed() {
                gone.push(client);
            }
        }
        for client in gone {
            let Some(i) = peers.iter().position(|p| p.client == client) else { continue };
            let mut p = peers.remove(i);
            p.conn.flush();
            if let Some(l) = self.lobby.as_mut().filter(|_| self.game.is_none()) {
                if l.slot_of(client).is_some() {
                    l.leave(client);
                    lobby_changed = true;
                }
            }
            if let Some(g) = self.game.as_mut() {
                if let Some(player) = g.clients.iter().position(|&c| c == Some(client)) {
                    let from_turn = g.last_turn[player] + 1;
                    g.lockstep.drop_player(player, from_turn);
                    g.clients[player] = None;
                    Self::broadcast(peers, &Msg::Dropped { player: player as u8, from_turn }, None);
                    self.events.push(SessionEvent::PlayerDropped { player, name: g.names[player].clone() });
                }
            }
        }
        if lobby_changed {
            if let Some(l) = &self.lobby {
                Self::broadcast(peers, &Msg::Lobby(l.clone()), None);
            }
            self.events.push(SessionEvent::LobbyChanged);
        }
        if let Some(g) = self.game.as_mut() {
            for pkt in g.lockstep.outbox.drain(..) {
                Self::broadcast(peers, &Msg::Turn(pkt), None);
            }
        }
        if let (Some(a), Some(info)) = (announcer.as_ref(), info) {
            a.poll(&info);
        }
        for p in peers.iter_mut() {
            p.conn.flush();
        }
    }

    fn poll_client(&mut self) {
        let Role::Client { conn } = &mut self.role else { return };
        for m in conn.poll() {
            match m {
                Msg::Welcome { client } => self.client = Some(client),
                Msg::Refused { reason } => self.events.push(SessionEvent::Refused(reason)),
                Msg::Lobby(l) => {
                    self.lobby = Some(l);
                    self.events.push(SessionEvent::LobbyChanged);
                }
                Msg::Chat { from, text } => self.events.push(SessionEvent::Chat { from, text }),
                Msg::Start { setup, names, delay } => {
                    let humans: Vec<bool> = setup.players.iter().map(|p| !p.computer).collect();
                    let clients = self.lobby.as_ref().map(|l| l.clients_by_player()).unwrap_or_default();
                    let last = delay.saturating_sub(1);
                    self.game = Some(NetGame { lockstep: Lockstep::new(setup.local_player, &humans, delay), names: names.clone(), clients, last_turn: vec![last; humans.len()] });
                    self.events.push(SessionEvent::Started { setup, names });
                }
                Msg::Turn(pkt) => {
                    if let Some(g) = self.game.as_mut() {
                        g.lockstep.receive(pkt);
                    }
                }
                Msg::Dropped { player, from_turn } => {
                    if let Some(g) = self.game.as_mut() {
                        let p = usize::from(player);
                        g.lockstep.drop_player(p, from_turn);
                        let name = g.names.get(p).cloned().unwrap_or_default();
                        self.events.push(SessionEvent::PlayerDropped { player: p, name });
                    }
                }
                _ => {}
            }
        }
        if let Some(g) = self.game.as_mut() {
            for pkt in g.lockstep.outbox.drain(..) {
                conn.send(&Msg::Turn(pkt));
            }
        }
        if conn.is_closed() {
            if !self.events.contains(&SessionEvent::Disconnected) {
                self.events.push(SessionEvent::Disconnected);
            }
        }
        conn.flush();
    }

    /// A line of chat to everyone.
    pub fn chat(&mut self, text: &str) {
        match &mut self.role {
            Role::Host { peers, .. } => {
                let msg = Msg::Chat { from: self.name.clone(), text: text.to_string() };
                Self::broadcast(peers, &msg, None);
                self.events.push(SessionEvent::Chat { from: self.name.clone(), text: text.to_string() });
            }
            Role::Client { conn } => conn.send(&Msg::Chat { from: self.name.clone(), text: text.to_string() }),
        }
    }

    /// A change to our own slot (race, ready).
    pub fn request(&mut self, r: LobbyRequest) {
        match &mut self.role {
            Role::Host { peers, .. } => {
                if let Some(l) = self.lobby.as_mut() {
                    l.request(0, &r);
                    Self::broadcast(peers, &Msg::Lobby(l.clone()), None);
                    self.events.push(SessionEvent::LobbyChanged);
                }
            }
            Role::Client { conn } => conn.send(&Msg::Request(r)),
        }
    }

    /// The host changes the lobby (a slot, the map, the options); everyone is told.
    pub fn edit_lobby(&mut self, f: impl FnOnce(&mut LobbyState)) {
        let Role::Host { peers, .. } = &mut self.role else { return };
        if self.game.is_some() {
            return;
        }
        if let Some(l) = self.lobby.as_mut() {
            f(l);
            Self::broadcast(peers, &Msg::Lobby(l.clone()), None);
            self.events.push(SessionEvent::LobbyChanged);
        }
    }

    /// The host sets a slot to a computer player (or open, or closed).
    pub fn set_slot(&mut self, i: usize, kind: SlotKind, race: ShipRace) {
        self.edit_lobby(|l| l.set_slot(i, kind, race));
    }

    /// The host starts the game: every client gets its setup, and the host's is returned.
    pub fn start(&mut self, seed: u32, delay: u32) -> Option<GameSetup> {
        let Role::Host { peers, .. } = &mut self.role else { return None };
        let l = self.lobby.as_ref()?;
        if self.game.is_some() || !l.can_start() {
            return None;
        }
        for p in peers.iter_mut().filter(|p| p.hello) {
            let (setup, names) = l.setup(seed, p.client);
            p.conn.send(&Msg::Start { setup, names, delay });
            p.conn.flush();
        }
        let (setup, names) = l.setup(seed, 0);
        let humans: Vec<bool> = setup.players.iter().map(|p| !p.computer).collect();
        let clients = l.clients_by_player();
        self.game = Some(NetGame { lockstep: Lockstep::new(setup.local_player, &humans, delay), names: names.clone(), clients, last_turn: vec![delay.saturating_sub(1); humans.len()] });
        self.events.push(SessionEvent::Started { setup: setup.clone(), names });
        Some(setup)
    }

    /// An order of our player, sent with our next packet.
    pub fn give(&mut self, order: Order) {
        if let Some(g) = self.game.as_mut() {
            g.lockstep.give(order);
        }
    }

    /// Runs the turns that are ready (at most `max`), each with `step` for its updates.
    /// Returns how many ran. Desynchronisations come out as events.
    pub fn run_turns(&mut self, u: &mut Universe, disguises: Option<&DisguiseTables>, max: u32, mut step: impl FnMut(&mut Universe)) -> u32 {
        let Some(g) = self.game.as_mut() else { return 0 };
        let mut n = 0;
        while n < max && g.lockstep.ready() {
            g.lockstep.run_turn(u, disguises, &mut step);
            n += 1;
        }
        for e in g.lockstep.events.drain(..) {
            if let lockstep::Event::Desync { turn, player, .. } = e {
                self.events.push(SessionEvent::Desync { turn, player });
            }
        }
        if n > 0 {
            // Send our packets at once.
            self.poll();
        }
        n
    }

    /// Leaves the game (or closes it, as the host).
    pub fn leave(&mut self) {
        match &mut self.role {
            Role::Host { peers, .. } => {
                for p in peers.iter_mut() {
                    p.conn.send(&Msg::Leave);
                    p.conn.flush_blocking(Duration::from_millis(200));
                }
                peers.clear();
            }
            Role::Client { conn } => {
                conn.send(&Msg::Leave);
                conn.flush_blocking(Duration::from_millis(200));
            }
        }
    }
}
