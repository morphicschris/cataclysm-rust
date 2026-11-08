//! The game's side of a network game: the session ([`hwc_net::session::Session`]) and where
//! the running game is in its lockstep turn. The game screen gives its orders through
//! [`crate::game::Game::give`]; in a network game they go into the next packet, and the
//! universe only updates when every player's orders for the turn have come.

use hwc_net::lockstep::UPDATES_PER_TURN;
use hwc_net::session::{Session, SessionEvent};
use hwc_sim::ships::mimic::DisguiseTables;
use hwc_sim::universe::Universe;

/// The running network game.
#[derive(Debug)]
pub struct NetLink {
    pub session: Session,
    /// Updates of the current turn already run (0: the next update starts a turn).
    pub mid_turn: u32,
    /// The players' names, by player.
    pub names: Vec<String>,
    /// Since when the game has been waiting for packets, and whether it said so.
    stalled: Option<(std::time::Instant, bool)>,
}

impl NetLink {
    pub fn new(session: Session, names: Vec<String>) -> Self {
        NetLink { session, mid_turn: 0, names, stalled: None }
    }

    /// Reads the network (packets, chat, players leaving).
    pub fn poll(&mut self) {
        self.session.poll();
    }

    /// Whether the next universe update may run: inside a turn always; at a turn's start only
    /// when every player's packet for it has come (the turn's orders are then applied).
    pub fn before_update(&mut self, u: &mut Universe, disguises: Option<&DisguiseTables>) -> bool {
        if self.mid_turn == 0 {
            let Some(g) = self.session.game.as_mut() else { return false };
            if !g.lockstep.ready() {
                if self.stalled.is_none() {
                    self.stalled = Some((std::time::Instant::now(), false));
                }
                return false;
            }
            self.stalled = None;
            g.lockstep.begin_turn(u, disguises);
        }
        true
    }

    /// After a universe update: at a turn's end, the checksum and our packet go out.
    pub fn after_update(&mut self, u: &Universe) {
        self.mid_turn += 1;
        if self.mid_turn == UPDATES_PER_TURN {
            self.mid_turn = 0;
            if let Some(g) = self.session.game.as_mut() {
                g.lockstep.end_turn(u);
                for e in g.lockstep.events.drain(..) {
                    if let hwc_net::lockstep::Event::Desync { turn, player, .. } = e {
                        self.session.events.push(SessionEvent::Desync { turn, player });
                    }
                }
            }
            self.session.poll();
        }
    }

    /// The players whose packets the game is waiting for (shown when it stalls).
    pub fn waiting_on(&self) -> Vec<String> {
        let Some(g) = self.session.game.as_ref() else { return Vec::new() };
        g.lockstep.waiting_on().into_iter().map(|p| self.names.get(p).cloned().unwrap_or_else(|| format!("player {}", p + 1))).collect()
    }

    /// After two seconds waiting for other players' orders, once: who the game waits for.
    pub fn stall_note(&mut self) -> Option<String> {
        let (since, said) = self.stalled.as_mut()?;
        if *said || since.elapsed() < std::time::Duration::from_secs(2) {
            return None;
        }
        *said = true;
        Some(format!("Waiting for {}", self.waiting_on().join(", ")))
    }

    /// What happened, as lines for the game's message list.
    pub fn messages(&mut self) -> Vec<String> {
        self.session
            .events
            .drain(..)
            .filter_map(|e| match e {
                SessionEvent::Chat { from, text } => Some(format!("{from}: {text}")),
                SessionEvent::PlayerDropped { name, .. } => Some(format!("{name} has left the game")),
                SessionEvent::Desync { turn, player } => Some(format!("Out of sync with {} at turn {turn}", self.names.get(player).cloned().unwrap_or_default())),
                SessionEvent::Disconnected => Some("Lost the connection to the host".to_string()),
                _ => None,
            })
            .collect()
    }
}
