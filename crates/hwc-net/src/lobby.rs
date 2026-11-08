//! The game being set up before it starts: the host's name for it, the map and options, and
//! the slots the players take (humans as they join, computer players the host adds). The host
//! keeps the state and sends it to every client whenever it changes.

use serde::{Deserialize, Serialize};

use hwc_formats::multiplayer::GameOptions;
use hwc_sim::skirmish::{GameSetup, PlayerSetup};
use hwc_sim::statics::ShipRace;

/// Who a slot is for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SlotKind {
    Open,
    Closed,
    Human { client: u32, name: String },
    Computer,
}

/// One seat of the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub kind: SlotKind,
    pub race: ShipRace,
    pub ready: bool,
}

/// A client's change to its own slot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LobbyRequest {
    Race(ShipRace),
    Ready(bool),
}

/// The game being set up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LobbyState {
    pub name: String,
    pub map: String,
    pub options: GameOptions,
    pub slots: Vec<Slot>,
}

impl LobbyState {
    /// A new game hosted by `host_name` (client 0), in the first of `max_players` slots.
    pub fn new(name: &str, host_name: &str, map: &str, max_players: usize, options: GameOptions) -> Self {
        let mut slots: Vec<Slot> = (0..max_players.max(2)).map(|_| Slot { kind: SlotKind::Open, race: ShipRace::Somtaaw, ready: false }).collect();
        slots[0].kind = SlotKind::Human { client: 0, name: host_name.to_string() };
        LobbyState { name: name.to_string(), map: map.to_string(), options, slots }
    }

    /// A client takes the first open slot; `None` when the game is full.
    pub fn join(&mut self, client: u32, name: &str) -> Option<usize> {
        let i = self.slots.iter().position(|s| s.kind == SlotKind::Open)?;
        self.slots[i] = Slot { kind: SlotKind::Human { client, name: name.to_string() }, race: ShipRace::Somtaaw, ready: false };
        Some(i)
    }

    /// The client's slot opens again.
    pub fn leave(&mut self, client: u32) {
        for s in &mut self.slots {
            if matches!(&s.kind, SlotKind::Human { client: c, .. } if *c == client) {
                *s = Slot { kind: SlotKind::Open, race: ShipRace::Somtaaw, ready: false };
            }
        }
    }

    pub fn slot_of(&self, client: u32) -> Option<usize> {
        self.slots.iter().position(|s| matches!(&s.kind, SlotKind::Human { client: c, .. } if *c == client))
    }

    /// A client's request for its own slot.
    pub fn request(&mut self, client: u32, r: &LobbyRequest) {
        let Some(i) = self.slot_of(client) else { return };
        match r {
            LobbyRequest::Race(race) if matches!(race, ShipRace::Somtaaw | ShipRace::Beast) => self.slots[i].race = *race,
            LobbyRequest::Ready(v) => self.slots[i].ready = *v,
            _ => {}
        }
    }

    /// The host sets a slot that is not a human's: open, closed or a computer player.
    pub fn set_slot(&mut self, i: usize, kind: SlotKind, race: ShipRace) {
        let Some(s) = self.slots.get_mut(i) else { return };
        if matches!(s.kind, SlotKind::Human { .. }) || matches!(kind, SlotKind::Human { .. }) {
            return;
        }
        *s = Slot { kind, race, ready: true };
    }

    pub fn humans(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.kind, SlotKind::Human { .. })).count()
    }

    pub fn players(&self) -> usize {
        self.slots.iter().filter(|s| matches!(s.kind, SlotKind::Human { .. } | SlotKind::Computer)).count()
    }

    /// The seats in player order: the humans first, then the computer players, as the
    /// original seats them.
    pub fn seating(&self) -> Vec<usize> {
        let humans = (0..self.slots.len()).filter(|&i| matches!(self.slots[i].kind, SlotKind::Human { .. }));
        let cpus = (0..self.slots.len()).filter(|&i| self.slots[i].kind == SlotKind::Computer);
        humans.chain(cpus).collect()
    }

    /// Every human but the host is ready, and there are at least two players.
    pub fn can_start(&self) -> bool {
        self.players() >= 2 && self.slots.iter().all(|s| !matches!(&s.kind, SlotKind::Human { client, .. } if *client != 0) || s.ready)
    }

    /// The setup `client` plays, and the players' names in player order.
    pub fn setup(&self, seed: u32, client: u32) -> (GameSetup, Vec<String>) {
        let seats = self.seating();
        let players = seats.iter().map(|&i| PlayerSetup { race: self.slots[i].race, computer: self.slots[i].kind == SlotKind::Computer }).collect();
        let names = seats
            .iter()
            .map(|&i| match &self.slots[i].kind {
                SlotKind::Human { name, .. } => name.clone(),
                _ => format!("Computer {}", i + 1),
            })
            .collect();
        let local = self.slot_of(client).and_then(|s| seats.iter().position(|&x| x == s)).unwrap_or(0);
        let mut options = self.options.clone();
        options.num_computers = seats.iter().filter(|&&i| self.slots[i].kind == SlotKind::Computer).count() as u8;
        let setup = GameSetup { map: self.map.clone(), players, options, slots: None, seed, effects: true, local_player: local, network: true };
        (setup, names)
    }

    /// The client numbers of the human players, by player.
    pub fn clients_by_player(&self) -> Vec<Option<u32>> {
        self.seating()
            .iter()
            .map(|&i| match &self.slots[i].kind {
                SlotKind::Human { client, .. } => Some(*client),
                _ => None,
            })
            .collect()
    }
}
