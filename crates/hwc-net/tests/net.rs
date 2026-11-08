//! Network games on one machine (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; the tests
//! that run a game are skipped when neither is set): two machines in lockstep stay the same
//! under orders and latency, a desynchronisation is caught, and a host and a client play over
//! TCP on this machine, found by the game browser, until the client drops out.

use std::collections::VecDeque;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use hwc_formats::gamedata::GameData;
use hwc_math::Vector;
use hwc_net::discovery::{Announcer, Browser, GameInfo};
use hwc_net::lobby::{LobbyRequest, SlotKind};
use hwc_net::lockstep::{Event, Lockstep, TurnPacket, DEFAULT_DELAY};
use hwc_net::session::{Session, SessionEvent};
use hwc_sim::mission::GameDataSource;
use hwc_sim::multiplayer::mg;
use hwc_sim::orders::{sync_checksum, Order};
use hwc_sim::skirmish::{game_types, map_list, GameOptions, GameSetup, PlayerSetup, SkirmishRun};
use hwc_sim::statics::{LoadOptions, ShipRace, ShipType, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    gd: GameData,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
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
        let src = GameDataSource(&gd);
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = SimTweaks::load(&src);
        Some(Data { gd, statics, tweaks })
    })
    .as_ref()
}

fn options(d: &Data) -> GameOptions {
    let src = GameDataSource(&d.gd);
    let mut o = game_types(&src).into_iter().find(|g| g.name == "Default").expect("the Default game type").options();
    o.flags &= !mg::RESEARCH_ENABLED;
    o
}

/// A map for `n` players.
fn map_for(d: &Data, n: usize) -> String {
    let src = GameDataSource(&d.gd);
    map_list(&src).into_iter().find(|m| m.min_players <= n && n <= m.max_players).expect("a map").name
}

fn start(d: &Data, setup: GameSetup) -> SkirmishRun {
    let src = GameDataSource(&d.gd);
    SkirmishRun::start(d.statics.clone(), d.tweaks.clone(), &src, setup).expect("start")
}

/// A small deterministic random source for the players' clicks.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 33) as u32
    }
}

/// What a player might click: a move, an attack on an enemy, building at the mothership.
fn some_order(run: &SkirmishRun, player: usize, rng: &mut Lcg) -> Option<Order> {
    let u = &run.universe;
    let own: Vec<_> = u.ship_list.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player == player)).collect();
    let enemies: Vec<_> = u.ship_list.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player != player && s.player < 2)).collect();
    match rng.next() % 4 {
        0 => {
            let ms = u.base.players[player].mothership?;
            let t = [ShipType::sAcolyte, ShipType::bInterceptor, ShipType::sACV, ShipType::bAttackBomber][(rng.next() % 4) as usize];
            Some(Order::Build { factory: ms, ship_type: t, count: 1 + (rng.next() % 3) as i32 })
        }
        1 if !own.is_empty() && !enemies.is_empty() => {
            let ships: Vec<_> = own.iter().copied().filter(|_| rng.next() % 3 == 0).collect();
            Some(Order::Attack { ships, targets: vec![enemies[(rng.next() as usize) % enemies.len()]] })
        }
        _ if !own.is_empty() => {
            let ships: Vec<_> = own.iter().copied().filter(|_| rng.next() % 2 == 0).collect();
            let r = |rng: &mut Lcg| (rng.next() % 20000) as f32 - 10000.0;
            let to = Vector::new(r(rng), r(rng), r(rng) * 0.2);
            Some(Order::Move { ships, mode: hwc_sim::command::waypoint_mode::ONCE, from: Vector::ZERO, waypoints: vec![to] })
        }
        _ => None,
    }
}

/// Two machines, one player each (and a computer player), packets delivered late and out of
/// step: they run the same game.
#[test]
fn two_machines_in_lockstep_stay_the_same() {
    let Some(d) = data() else { return };
    let map = map_for(d, 3);
    let mk = |local| GameSetup {
        map: map.clone(),
        players: vec![PlayerSetup { race: ShipRace::Somtaaw, computer: false }, PlayerSetup { race: ShipRace::Beast, computer: false }, PlayerSetup { race: ShipRace::Beast, computer: true }],
        options: options(d),
        slots: None,
        seed: 21,
        effects: false,
        local_player: local,
        network: true,
    };
    let mut runs = [start(d, mk(0)), start(d, mk(1))];
    assert_eq!(sync_checksum(&runs[0].universe), sync_checksum(&runs[1].universe), "the same start");
    let humans = [true, true, false];
    let mut ls = [Lockstep::new(0, &humans, DEFAULT_DELAY), Lockstep::new(1, &humans, DEFAULT_DELAY)];
    let mut rngs = [Lcg(1), Lcg(2)];
    // In flight: (deliver at step, to, packet).
    let mut wire: VecDeque<(u32, usize, TurnPacket)> = VecDeque::new();
    let mut net_rng = Lcg(3);
    let turns = 1500;
    let mut step = 0u32;
    let mut stalls = 0;
    while ls[0].turn < turns || ls[1].turn < turns {
        step += 1;
        for m in 0..2 {
            if ls[m].turn >= turns || !ls[m].ready() {
                if ls[m].turn < turns {
                    stalls += 1;
                }
                continue;
            }
            if rngs[m].next() % 6 == 0 {
                if let Some(o) = some_order(&runs[m], m, &mut rngs[m]) {
                    ls[m].give(o);
                }
            }
            let run = &mut runs[m];
            ls[m].run_turn(&mut run.universe, None, |u| u.update());
            for pkt in ls[m].outbox.drain(..) {
                let lag = net_rng.next() % 4;
                wire.push_back((step + lag, 1 - m, pkt));
            }
        }
        // Deliver what is due (in order per sender: TCP keeps order).
        let mut keep = VecDeque::new();
        let mut blocked = [false; 2];
        while let Some((at, to, pkt)) = wire.pop_front() {
            if at <= step && !blocked[to] {
                ls[to].receive(pkt);
            } else {
                blocked[to] = true;
                keep.push_back((at, to, pkt));
            }
        }
        wire = keep;
        assert!(step < turns * 20, "the machines stalled");
    }
    for m in 0..2 {
        assert!(ls[m].events.is_empty(), "machine {m}: {:?}", ls[m].events);
    }
    let (a, b) = (sync_checksum(&runs[0].universe), sync_checksum(&runs[1].universe));
    let ships = runs[0].universe.ship_list.len();
    println!("{turns} turns, {step} steps, {stalls} stalls waiting for packets, {ships} ships, checksum {a:x}");
    assert_eq!(a, b, "both machines end the same");
    for t in [10, 500, turns - 1] {
        assert_eq!(ls[0].checksum(t), ls[1].checksum(t), "turn {t}");
    }
}

/// One machine's world is changed behind the game's back: the other reports it within a few
/// turns.
#[test]
fn a_desynchronisation_is_caught() {
    let Some(d) = data() else { return };
    let mk = |local| GameSetup {
        map: "Coldwar2".into(),
        players: vec![PlayerSetup { race: ShipRace::Somtaaw, computer: false }, PlayerSetup { race: ShipRace::Beast, computer: false }],
        options: options(d),
        slots: None,
        seed: 4,
        effects: false,
        local_player: local,
        network: true,
    };
    let mut runs = [start(d, mk(0)), start(d, mk(1))];
    let humans = [true, true];
    let mut ls = [Lockstep::new(0, &humans, DEFAULT_DELAY), Lockstep::new(1, &humans, DEFAULT_DELAY)];
    for turn in 0..60u32 {
        if turn == 30 {
            runs[1].universe.base.players[0].resource_units += 1;
        }
        for m in 0..2 {
            let run = &mut runs[m];
            ls[m].run_turn(&mut run.universe, None, |u| u.update());
        }
        let out: Vec<Vec<TurnPacket>> = ls.iter_mut().map(|l| l.outbox.drain(..).collect()).collect();
        for (m, pkts) in out.into_iter().enumerate() {
            for p in pkts {
                ls[1 - m].receive(p);
            }
        }
    }
    let found: Vec<&Event> = ls[0].events.iter().chain(ls[1].events.iter()).collect();
    println!("{found:?}");
    assert!(matches!(found.first(), Some(Event::Desync { turn: 30, .. })), "the turn it happened: {found:?}");
}

/// The game browser finds a host answering on this machine.
#[test]
fn the_game_browser_finds_a_game() {
    let a = Announcer::bind(0).expect("announcer");
    let port = a.local_addr().expect("addr").port();
    let info = GameInfo { name: "Test game".into(), host_name: "Host".into(), map: "Coldwar2".into(), players: 1, max_players: 4, port: 28470, build: hwc_net::build_id(), started: false };
    let mut b = Browser::new().expect("browser");
    b.query(port, &[format!("127.0.0.1:{port}").parse().unwrap()]);
    let end = Instant::now() + Duration::from_secs(3);
    while b.found.is_empty() && Instant::now() < end {
        a.poll(&info);
        std::thread::sleep(Duration::from_millis(5));
        b.poll(Duration::from_secs(10));
    }
    assert_eq!(b.found.first().map(|f| &f.info), Some(&info));
}

/// A host and a client over TCP on this machine: the client joins and picks the Beast, the
/// host adds a computer player and starts, both play in lockstep giving orders, then the
/// client leaves and the host plays on without them.
#[test]
fn a_lan_game_over_tcp() {
    let Some(d) = data() else { return };
    let mut host = Session::host("Host", "Test game", &map_for(d, 3), 3, options(d), "127.0.0.1:0", None).expect("host");
    let addr = host.local_addr().expect("address");
    let mut client = Session::join(addr, "Client", Duration::from_secs(2)).expect("join");
    let pump = |host: &mut Session, client: &mut Session| {
        host.poll();
        client.poll();
        std::thread::sleep(Duration::from_millis(1));
    };
    let end = Instant::now() + Duration::from_secs(5);
    while client.lobby.as_ref().is_none_or(|l| l.humans() < 2) && Instant::now() < end {
        pump(&mut host, &mut client);
    }
    assert_eq!(client.client, Some(1), "the client was welcomed");
    client.request(LobbyRequest::Race(ShipRace::Beast));
    client.request(LobbyRequest::Ready(true));
    host.set_slot(2, SlotKind::Computer, ShipRace::Somtaaw);
    let end = Instant::now() + Duration::from_secs(5);
    while !host.lobby.as_ref().unwrap().can_start() && Instant::now() < end {
        pump(&mut host, &mut client);
    }
    let host_setup = host.start(77, DEFAULT_DELAY).expect("the game starts");
    let end = Instant::now() + Duration::from_secs(5);
    let mut client_setup = None;
    while client_setup.is_none() && Instant::now() < end {
        pump(&mut host, &mut client);
        for e in client.events.drain(..) {
            if let SessionEvent::Started { setup, .. } = e {
                client_setup = Some(setup);
            }
        }
    }
    let mut client_setup = client_setup.expect("the client starts");
    let mut host_setup = host_setup;
    assert_eq!(host_setup.local_player, 0);
    assert_eq!(client_setup.local_player, 1);
    assert_eq!(client_setup.players[1].race, ShipRace::Beast);
    assert!(client_setup.players[2].computer);
    host_setup.effects = false;
    client_setup.effects = false;
    // The game's chat: the host to the client only, the client to everyone.
    host.game_chat(1 << 1, "gl hf");
    client.game_chat(0b111, "you too");
    let end = Instant::now() + Duration::from_secs(5);
    let heard = |s: &Session, text: &str| s.events.iter().any(|e| matches!(e, SessionEvent::GameChat { text: t, .. } if t == text));
    while !(heard(&client, "gl hf") && heard(&host, "you too")) && Instant::now() < end {
        pump(&mut host, &mut client);
    }
    assert!(client.events.iter().any(|e| matches!(e, SessionEvent::GameChat { from: 0, text, private: true } if text == "gl hf")), "{:?}", client.events);
    assert!(host.events.iter().any(|e| matches!(e, SessionEvent::GameChat { from: 1, text, private: false } if text == "you too")), "{:?}", host.events);
    let mut runs = [start(d, host_setup), start(d, client_setup)];
    let mut rngs = [Lcg(5), Lcg(6)];
    let turns = 400;
    let end = Instant::now() + Duration::from_secs(120);
    while (host.game.as_ref().unwrap().lockstep.turn < turns || client.game.as_ref().unwrap().lockstep.turn < turns) && Instant::now() < end {
        for (m, s) in [&mut host, &mut client].into_iter().enumerate() {
            if s.game.as_ref().unwrap().lockstep.turn >= turns {
                s.poll();
                continue;
            }
            if s.game.as_ref().unwrap().lockstep.ready() && rngs[m].next() % 6 == 0 {
                if let Some(o) = some_order(&runs[m], m, &mut rngs[m]) {
                    s.give(o);
                }
            }
            s.run_turns(&mut runs[m].universe, None, 1, |u| u.update());
            s.poll();
        }
    }
    let (a, b) = (sync_checksum(&runs[0].universe), sync_checksum(&runs[1].universe));
    let desyncs: Vec<_> = host.events.iter().chain(client.events.iter()).filter(|e| matches!(e, SessionEvent::Desync { .. })).cloned().collect();
    println!("host turn {}, client turn {}, desyncs {desyncs:?}", host.game.as_ref().unwrap().lockstep.turn, client.game.as_ref().unwrap().lockstep.turn);
    assert!(desyncs.is_empty());
    assert_eq!(a, b, "host and client end the same");
    // The client leaves; the host goes on alone.
    client.leave();
    let t0 = host.game.as_ref().unwrap().lockstep.turn;
    let end = Instant::now() + Duration::from_secs(10);
    while host.game.as_ref().unwrap().lockstep.turn < t0 + 50 && Instant::now() < end {
        host.run_turns(&mut runs[0].universe, None, 1, |u| u.update());
        host.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(host.events.iter().any(|e| matches!(e, SessionEvent::PlayerDropped { player: 1, .. })), "the host saw the client go: {:?}", host.events);
    assert!(host.game.as_ref().unwrap().lockstep.turn >= t0 + 50, "the host plays on");
    assert!(runs[0].universe.mp.deaths.get(1).copied().flatten().is_some(), "the dropped player is out of the game");
}

/// Debug: the same game seen from two players, the same orders: where do they part?
#[test]
#[ignore]
fn find_local_player_divergence() {
    let Some(d) = data() else { return };
    let map = map_for(d, 3);
    let mk = |local| GameSetup {
        map: map.clone(),
        players: vec![PlayerSetup { race: ShipRace::Somtaaw, computer: false }, PlayerSetup { race: ShipRace::Beast, computer: false }, PlayerSetup { race: ShipRace::Beast, computer: true }],
        options: options(d),
        slots: None,
        seed: 21,
        effects: false,
        local_player: local,
        network: true,
    };
    let mut runs = [start(d, mk(0)), start(d, mk(1))];
    let mut rngs = [Lcg(1), Lcg(2)];
    let with_orders = std::env::var_os("ORDERS").is_some();
    for step in 0..8000u32 {
        if with_orders && step % 12 == 0 {
            for p in 0..2 {
                if let Some(o) = some_order(&runs[0], p, &mut rngs[p]) {
                    for r in runs.iter_mut() {
                        hwc_sim::orders::apply(&mut r.universe, p, &o, None);
                    }
                }
            }
        }
        for r in runs.iter_mut() {
            r.universe.update();
        }
        let (a, b) = (&runs[0].universe, &runs[1].universe);
        if sync_checksum(a) != sync_checksum(b) {
            println!("diverged at update {step}");
            for (i, s) in a.randy.streams.iter().enumerate() {
                if *s != b.randy.streams[i] {
                    println!("  random stream {i} differs");
                }
            }
            for &id in &a.ship_list {
                let (x, y) = (a.ships.get(id), b.ships.get(id));
                match (x, y) {
                    (Some(x), Some(y)) => {
                        if x.posinfo.position != y.posinfo.position || x.health != y.health || x.posinfo.velocity != y.posinfo.velocity {
                            println!("  ship {:?} {:?} p{}: pos {:?} vs {:?}, health {} vs {}", id, x.ship_type, x.player, x.posinfo.position, y.posinfo.position, x.health, y.health);
                        }
                    }
                    _ => println!("  ship {:?} only on one side", id),
                }
            }
            if a.ship_list.len() != b.ship_list.len() {
                println!("  ship counts {} vs {}", a.ship_list.len(), b.ship_list.len());
            }
            for p in 0..3 {
                if a.base.players[p].resource_units != b.base.players[p].resource_units {
                    println!("  player {p} RUs {} vs {}", a.base.players[p].resource_units, b.base.players[p].resource_units);
                }
            }
            return;
        }
    }
    println!("no divergence");
}

/// The lobby server on this machine: two players log in, chat in the main room, one hosts a
/// game that the server cannot reach (so the other joins through the relay), and they play
/// in lockstep through the server.
#[test]
fn an_internet_game_through_the_lobby_and_its_relay() {
    use hwc_net::internet::{LobbyClient, LobbyEvent, LobbyMsg, LobbyServer};
    let Some(d) = data() else { return };
    let build = hwc_net::build_id();
    let mut server = LobbyServer::bind("127.0.0.1:0", &build).expect("server");
    let at = server.local_addr().unwrap();
    // The server runs on its own, as it would on another machine.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop2 = stop.clone();
    let server_thread = std::thread::spawn(move || {
        while !stop2.load(std::sync::atomic::Ordering::Relaxed) {
            server.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let mut alice = LobbyClient::connect(at, "Alice", "", &build, Duration::from_secs(2)).expect("alice");
    let mut bob = LobbyClient::connect(at, "Bob", "", &build, Duration::from_secs(2)).expect("bob");
    let run_until = |a: &mut LobbyClient, b: &mut LobbyClient, done: &dyn Fn(&LobbyClient, &LobbyClient) -> bool| {
        let end = Instant::now() + Duration::from_secs(10);
        while !done(a, b) && Instant::now() < end {
            a.poll();
            b.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(done(a, b), "timed out");
    };
    run_until(&mut alice, &mut bob, &|a, b| a.logged_in && b.logged_in && a.users.len() == 2);
    // A name in use is refused.
    let mut again = LobbyClient::connect(at, "Alice", "", &build, Duration::from_secs(2)).expect("again");
    let end = Instant::now() + Duration::from_secs(5);
    while again.events.is_empty() && Instant::now() < end {
        again.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(matches!(again.events.first(), Some(LobbyEvent::Refused(_))), "{:?}", again.events);
    alice.send(LobbyMsg::Chat { text: "anyone for a game?".into() });
    run_until(&mut alice, &mut bob, &|_, b| b.events.iter().any(|e| matches!(e, LobbyEvent::Chat { from, .. } if from == "Alice")));
    // Alice hosts; the server is not asked to reach her, so the game goes through the relay.
    let mut host = Session::host("Alice", "Relay test", &map_for(d, 2), 2, options(d), "127.0.0.1:0", None).expect("host");
    let info = host.info().expect("info");
    alice.send(LobbyMsg::HostGame { info, try_direct: false });
    run_until(&mut alice, &mut bob, &|_, b| !b.games.is_empty());
    let listed = bob.games[0].clone();
    assert_eq!(listed.direct, None);
    // Bob's connection goes through the server; the server asks Alice for her end.
    let bob_conn = std::thread::scope(|s| {
        let h = s.spawn(|| bob.connect_game(&listed, Duration::from_secs(5)));
        let end = Instant::now() + Duration::from_secs(5);
        while !h.is_finished() && Instant::now() < end {
            alice.poll();
            for pipe in std::mem::take(&mut alice.incoming) {
                let c = alice.accept_pipe(pipe, Duration::from_secs(5)).expect("pipe");
                host.adopt(c);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        h.join().unwrap()
    })
    .expect("bob reaches the game through the relay");
    let mut client = Session::join_over(bob_conn, "Bob");
    let pump = |host: &mut Session, client: &mut Session| {
        host.poll();
        client.poll();
        std::thread::sleep(Duration::from_millis(1));
    };
    let end = Instant::now() + Duration::from_secs(5);
    while host.lobby.as_ref().is_none_or(|l| l.humans() < 2) && Instant::now() < end {
        pump(&mut host, &mut client);
    }
    client.request(LobbyRequest::Ready(true));
    let end = Instant::now() + Duration::from_secs(5);
    while !host.lobby.as_ref().unwrap().can_start() && Instant::now() < end {
        pump(&mut host, &mut client);
    }
    let mut host_setup = host.start(9, DEFAULT_DELAY).expect("the game starts");
    let mut client_setup = None;
    let end = Instant::now() + Duration::from_secs(5);
    while client_setup.is_none() && Instant::now() < end {
        pump(&mut host, &mut client);
        for e in client.events.drain(..) {
            if let SessionEvent::Started { setup, .. } = e {
                client_setup = Some(setup);
            }
        }
    }
    let mut client_setup = client_setup.expect("the client starts");
    host_setup.effects = false;
    client_setup.effects = false;
    let mut runs = [start(d, host_setup), start(d, client_setup)];
    let mut rngs = [Lcg(8), Lcg(9)];
    let turns = 200;
    let end = Instant::now() + Duration::from_secs(60);
    while (host.game.as_ref().unwrap().lockstep.turn < turns || client.game.as_ref().unwrap().lockstep.turn < turns) && Instant::now() < end {
        for (m, s) in [&mut host, &mut client].into_iter().enumerate() {
            if s.game.as_ref().unwrap().lockstep.turn < turns {
                if s.game.as_ref().unwrap().lockstep.ready() && rngs[m].next() % 5 == 0 {
                    if let Some(o) = some_order(&runs[m], m, &mut rngs[m]) {
                        s.give(o);
                    }
                }
                s.run_turns(&mut runs[m].universe, None, 1, |u| u.update());
            }
            s.poll();
        }
    }
    assert!(host.game.as_ref().unwrap().lockstep.turn >= turns && client.game.as_ref().unwrap().lockstep.turn >= turns, "both played through the relay");
    assert_eq!(sync_checksum(&runs[0].universe), sync_checksum(&runs[1].universe), "the same game");
    assert!(!host.events.iter().chain(client.events.iter()).any(|e| matches!(e, SessionEvent::Desync { .. })));
    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    server_thread.join().unwrap();
}
