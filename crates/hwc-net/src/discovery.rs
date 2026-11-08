//! Finding games on the local network: a browser broadcasts a query on the discovery port, and
//! each host answers with what its game is (name, map, players, whether it has started).

use std::io::ErrorKind;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

const QUERY: &[u8; 4] = b"HWCQ";
const REPLY: &[u8; 4] = b"HWCR";

/// What a host says of its game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GameInfo {
    pub name: String,
    pub host_name: String,
    pub map: String,
    pub players: u8,
    pub max_players: u8,
    /// The TCP port the game takes clients on.
    pub port: u16,
    /// The build every machine must run ([`crate::build_id`]).
    pub build: String,
    pub started: bool,
}

/// A host's answering socket.
#[derive(Debug)]
pub struct Announcer {
    socket: UdpSocket,
}

impl Announcer {
    /// Listens on `port` of every interface.
    pub fn bind(port: u16) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))?;
        socket.set_nonblocking(true)?;
        Ok(Announcer { socket })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Answers the queries that have come.
    pub fn poll(&self, info: &GameInfo) {
        let mut buf = [0u8; 64];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if n >= 4 && &buf[..4] == QUERY {
                        let mut reply = REPLY.to_vec();
                        reply.extend(bincode::serialize(info).expect("game info serialises"));
                        let _ = self.socket.send_to(&reply, from);
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
    }
}

/// A game found: where to join it, what it is, when it last answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub addr: SocketAddr,
    pub info: GameInfo,
    pub seen: Instant,
}

impl Found {
    /// Where to connect: the answering host's address with the game's port.
    pub fn game_addr(&self) -> SocketAddr {
        SocketAddr::new(self.addr.ip(), self.info.port)
    }
}

/// The game browser's socket and the games it has found.
#[derive(Debug)]
pub struct Browser {
    socket: UdpSocket,
    pub found: Vec<Found>,
}

impl Browser {
    pub fn new() -> std::io::Result<Self> {
        let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0))?;
        socket.set_nonblocking(true)?;
        socket.set_broadcast(true)?;
        Ok(Browser { socket, found: Vec::new() })
    }

    /// Asks the local network (broadcast on `port`) and any other addresses given.
    pub fn query(&self, port: u16, also: &[SocketAddr]) {
        let _ = self.socket.send_to(QUERY, SocketAddrV4::new(Ipv4Addr::BROADCAST, port));
        for a in also {
            let _ = self.socket.send_to(QUERY, a);
        }
    }

    /// Takes in the answers; games silent for `forget` are dropped from the list.
    pub fn poll(&mut self, forget: Duration) {
        let mut buf = [0u8; 2048];
        loop {
            match self.socket.recv_from(&mut buf) {
                Ok((n, from)) => {
                    if n < 4 || &buf[..4] != REPLY {
                        continue;
                    }
                    let Ok(info) = bincode::deserialize::<GameInfo>(&buf[4..n]) else { continue };
                    let now = Instant::now();
                    match self.found.iter_mut().find(|f| f.addr == from) {
                        Some(f) => {
                            f.info = info;
                            f.seen = now;
                        }
                        None => self.found.push(Found { addr: from, info, seen: now }),
                    }
                }
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        let now = Instant::now();
        self.found.retain(|f| now.duration_since(f.seen) < forget);
    }
}
