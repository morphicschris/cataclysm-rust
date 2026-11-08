//! Lockstep turns. A turn is two universe updates (`UNIV_UPDATES_PER_PACKET`). Every human
//! player sends one [`TurnPacket`] for each turn, `delay` turns ahead, holding the orders given
//! since its last packet; a machine runs a turn only once it has every player's packet for it,
//! applying the orders in player order before the updates. Computer players need no packets:
//! their decisions are part of the simulation every machine runs.
//!
//! Each packet also carries the checksum of the state its sender reached after the last turn
//! it ran; each machine compares it with its own for that turn and reports a
//! desynchronisation the moment one appears.

use std::collections::{BTreeMap, VecDeque};

use hwc_sim::orders::{self, Order};
use hwc_sim::ships::mimic::DisguiseTables;
use hwc_sim::universe::Universe;

/// Universe updates in a turn.
pub const UPDATES_PER_TURN: u32 = 2;
/// Turns between giving an order and its turn (3 turns: 0.375 s at 16 updates a second).
pub const DEFAULT_DELAY: u32 = 3;
/// Our checksums kept for comparing with late packets.
const CHECKSUMS_KEPT: usize = 512;

/// One player's orders for a turn.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TurnPacket {
    pub turn: u32,
    pub player: u8,
    pub orders: Vec<Order>,
    /// The sender's checksum after a turn it ran (the turn, the checksum).
    pub checksum: Option<(u32, u64)>,
}

/// What a turn turned up.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A player's state differs from ours after `turn`.
    Desync { turn: u32, player: usize, ours: u64, theirs: u64 },
    /// A player left; from `turn` on the game goes on without them.
    PlayerDropped { player: usize, turn: u32 },
}

/// The lockstep state of one machine.
#[derive(Debug, Clone)]
pub struct Lockstep {
    pub local: usize,
    /// The players who send packets (the humans still in the game).
    waiting_for: Vec<bool>,
    pub delay: u32,
    /// The next turn to run.
    pub turn: u32,
    /// Orders by turn, by player (`None`: not come yet).
    pending: BTreeMap<u32, Vec<Option<Vec<Order>>>>,
    /// The local player's orders since its last packet.
    local_orders: Vec<Order>,
    /// Our checksums after each turn run.
    checksums: VecDeque<(u32, u64)>,
    /// Others' checksums for turns we have not run yet.
    remote_checks: Vec<(u32, usize, u64)>,
    /// Packets to send.
    pub outbox: Vec<TurnPacket>,
    /// Players dropping out, by the turn they go at.
    drops: BTreeMap<u32, Vec<usize>>,
    /// Desynchronisations and drops found.
    pub events: Vec<Event>,
    /// When recording: the orders and drops applied, by update counter.
    pub record: Option<Vec<(u32, orders::RecordedEvent)>>,
}

impl Lockstep {
    /// `humans[p]`: player `p` sends packets. The first `delay` turns have no orders.
    pub fn new(local: usize, humans: &[bool], delay: u32) -> Self {
        let n = humans.len();
        let mut pending = BTreeMap::new();
        for t in 0..delay {
            pending.insert(t, (0..n).map(|p| humans[p].then(Vec::new)).collect());
        }
        Lockstep {
            local,
            waiting_for: humans.to_vec(),
            delay,
            turn: 0,
            pending,
            local_orders: Vec::new(),
            checksums: VecDeque::new(),
            remote_checks: Vec::new(),
            outbox: Vec::new(),
            drops: BTreeMap::new(),
            events: Vec::new(),
            record: None,
        }
    }

    /// An order of the local player, for its next packet.
    pub fn give(&mut self, order: Order) {
        self.local_orders.push(order);
    }

    /// A packet from another machine (or our own, which is ignored).
    pub fn receive(&mut self, pkt: TurnPacket) {
        let p = usize::from(pkt.player);
        if p == self.local || p >= self.waiting_for.len() {
            return;
        }
        if let Some((turn, sum)) = pkt.checksum {
            self.check(turn, p, sum);
        }
        if pkt.turn < self.turn {
            return;
        }
        let n = self.waiting_for.len();
        let slot = self.pending.entry(pkt.turn).or_insert_with(|| vec![None; n]);
        if slot[p].is_none() {
            slot[p] = Some(pkt.orders);
        }
    }

    fn check(&mut self, turn: u32, player: usize, theirs: u64) {
        match self.checksums.iter().find(|c| c.0 == turn) {
            Some(&(_, ours)) => {
                if ours != theirs {
                    self.events.push(Event::Desync { turn, player, ours, theirs });
                }
            }
            None if turn >= self.turn => self.remote_checks.push((turn, player, theirs)),
            None => {}
        }
    }

    /// `player` leaves from `turn` on (a turn no machine can have run yet: the host picks the
    /// one after the last packet it had from them).
    pub fn drop_player(&mut self, player: usize, turn: u32) {
        let turn = turn.max(self.turn);
        self.drops.entry(turn).or_default().push(player);
    }

    /// Whether every player's packet for the next turn has come.
    pub fn ready(&self) -> bool {
        let Some(slot) = self.pending.get(&self.turn) else { return self.waiting_for.iter().all(|&w| !w) };
        (0..self.waiting_for.len()).all(|p| !self.waiting_for[p] || slot[p].is_some() || self.dropping_by(p, self.turn))
    }

    fn dropping_by(&self, p: usize, turn: u32) -> bool {
        self.drops.range(..=turn).any(|(_, v)| v.contains(&p))
    }

    /// The players whose packets the next turn waits for.
    pub fn waiting_on(&self) -> Vec<usize> {
        let slot = self.pending.get(&self.turn);
        (0..self.waiting_for.len()).filter(|&p| self.waiting_for[p] && slot.is_none_or(|s| s[p].is_none()) && !self.dropping_by(p, self.turn)).collect()
    }

    /// Runs the next turn: [`Lockstep::begin_turn`], `step` for each of its updates, then
    /// [`Lockstep::end_turn`].
    pub fn run_turn(&mut self, u: &mut Universe, disguises: Option<&DisguiseTables>, mut step: impl FnMut(&mut Universe)) {
        self.begin_turn(u, disguises);
        for _ in 0..UPDATES_PER_TURN {
            step(u);
        }
        self.end_turn(u);
    }

    /// The start of the next turn: the players dropping out go, then the orders are applied
    /// in player order. The turn's [`UPDATES_PER_TURN`] universe updates come next.
    pub fn begin_turn(&mut self, u: &mut Universe, disguises: Option<&DisguiseTables>) {
        assert!(self.ready(), "turn {} is not ready", self.turn);
        let t = self.turn;
        if let Some(ps) = self.drops.remove(&t) {
            for p in ps {
                if p < self.waiting_for.len() && self.waiting_for[p] {
                    self.waiting_for[p] = false;
                    hwc_sim::multiplayer::player_dropped(u, p);
                    if let Some(r) = self.record.as_mut() {
                        r.push((u.update_counter, orders::RecordedEvent::PlayerDropped { player: p as u8 }));
                    }
                    self.events.push(Event::PlayerDropped { player: p, turn: t });
                }
            }
        }
        let slot = self.pending.remove(&t).unwrap_or_default();
        for (p, orders) in slot.into_iter().enumerate() {
            for o in orders.into_iter().flatten() {
                orders::apply(u, p, &o, disguises);
                if let Some(r) = self.record.as_mut() {
                    r.push((u.update_counter, orders::RecordedEvent::Order { player: p as u8, order: o }));
                }
            }
        }
    }

    /// The end of the turn after its updates: the checksum is kept and compared, and the local
    /// packet for the turn `delay` ahead goes in the outbox with it.
    pub fn end_turn(&mut self, u: &Universe) {
        let t = self.turn;
        let sum = orders::sync_checksum(u);
        self.checksums.push_back((t, sum));
        while self.checksums.len() > CHECKSUMS_KEPT {
            self.checksums.pop_front();
        }
        let due: Vec<(u32, usize, u64)> = self.remote_checks.iter().copied().filter(|c| c.0 == t).collect();
        self.remote_checks.retain(|c| c.0 > t);
        for (turn, p, theirs) in due {
            if theirs != sum {
                self.events.push(Event::Desync { turn, player: p, ours: sum, theirs });
            }
        }
        self.turn += 1;
        let orders = std::mem::take(&mut self.local_orders);
        let target = t + self.delay;
        if self.waiting_for.get(self.local).copied().unwrap_or(false) {
            let n = self.waiting_for.len();
            self.pending.entry(target).or_insert_with(|| vec![None; n])[self.local] = Some(orders.clone());
            self.outbox.push(TurnPacket { turn: target, player: self.local as u8, orders, checksum: Some((t, sum)) });
        }
    }

    /// Our checksum after `turn`, if still kept.
    pub fn checksum(&self, turn: u32) -> Option<u64> {
        self.checksums.iter().find(|c| c.0 == turn).map(|c| c.1)
    }
}
