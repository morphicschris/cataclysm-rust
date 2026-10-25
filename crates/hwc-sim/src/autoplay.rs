//! Scripted test orders that play the campaign headless: a stand-in for the player that gives
//! the orders a mission needs (attack, explore, harvest), presses the hyperspace button when the
//! mission offers the jump, answers the roll call, and, when a mission cannot be finished with
//! these orders within its time budget, ends it with the original's debug override (the C key,
//! `hyperspaceOverride`). Used by the campaign example, the chain test and the viewer.
//!
//! Nothing here is part of the original game; the orders only use what a player can do.

use hwc_math::Vector;

use crate::mission::{sp_state, MissionRun, HUMAN_PLAYER};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipType};

/// How a mission ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The mission offered the jump ("Hyperspace" objective) and the fleet took it.
    Hyperspace,
    /// The time budget ran out and the jump was forced with the debug override.
    Forced,
    /// The mission failed (`MissionFailed`) or the mothership was lost.
    Failed,
    /// Neither within the update limit.
    Timeout,
}

/// The player stand-in for one mission.
#[derive(Debug, Clone)]
pub struct Autoplay {
    /// Updates after the fleet arrived before the jump is forced.
    pub budget: u32,
    /// Give attack and explore orders (otherwise only the hyperspace button is pressed).
    pub orders: bool,
    arrived_at: Option<u32>,
    rollcall_at: Option<u32>,
    pub forced: bool,
    pub pressed: bool,
    last_orders: u32,
    explore: usize,
}

impl Autoplay {
    pub fn new(budget: u32) -> Self {
        Autoplay { budget, orders: true, arrived_at: None, rollcall_at: None, forced: false, pressed: false, last_orders: 0, explore: 0 }
    }

    /// The orders for this update (call before [`MissionRun::step`]).
    pub fn tick(&mut self, run: &mut MissionRun) {
        let now = run.universe.update_counter;
        if !run.game.ships_have_arrived {
            return;
        }
        let arrived = *self.arrived_at.get_or_insert(now);
        match run.game.sp_state {
            sp_state::NONE => {
                if run.game.hyper.player_can_hyperspace && run.hyperspace_button().is_ok() {
                    self.pressed = true;
                    return;
                }
                if now.saturating_sub(arrived) >= self.budget && !self.forced && run.force_hyperspace().is_ok() {
                    self.forced = true;
                    return;
                }
                if self.orders && now >= self.last_orders + 160 {
                    self.last_orders = now;
                    self.give_orders(run);
                }
            }
            sp_state::WAITING_ROLLCALL => {
                let t = *self.rollcall_at.get_or_insert(now);
                // A player waits a minute for the fleet to dock, then goes.
                if now.saturating_sub(t) >= 16 * 60 {
                    run.hyperspace_go_now();
                }
            }
            _ => {}
        }
    }

    /// Plays the mission until the fleet has jumped (or `limit` updates).
    pub fn play(&mut self, run: &mut MissionRun, limit: u32) -> Ending {
        for _ in 0..limit {
            self.tick(run);
            run.step();
            if run.mission_complete() {
                return if self.forced { Ending::Forced } else { Ending::Hyperspace };
            }
            if run.game.mission_failed && run.game.sp_state == sp_state::NONE {
                return Ending::Failed;
            }
        }
        Ending::Timeout
    }

    /// Combat ships attack the enemy nearest the mothership; recons explore the level's
    /// volumes and points in turn; workers harvest.
    fn give_orders(&mut self, run: &mut MissionRun) {
        let u = &run.universe;
        let Some(ms) = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| u.ships.get(m).is_some()) else { return };
        let mp = u.ships.ship(ms).posinfo.position;
        let mine = |id: ShipId| u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER && s.flags & (sof::HIDE | sof::DEAD) == 0);
        let class = |id: ShipId| u.ships.get(id).and_then(|s| crate::base::class_of(u, s.ship_type));
        let mut fighters = Vec::new();
        let mut recons = Vec::new();
        let mut workers = Vec::new();
        for &id in &u.ship_list {
            if !mine(id) {
                continue;
            }
            let t = u.ships.ship(id).ship_type;
            match (t, class(id)) {
                (ShipType::sRecon, _) => recons.push(id),
                (ShipType::sWorker, _) => workers.push(id),
                (_, Some(ShipClass::Mothership | ShipClass::Component | ShipClass::NonCombat | ShipClass::Resource)) => {}
                (ShipType::sProcessor | ShipType::sRepairDroid | ShipType::sMimic | ShipType::sMCV, _) => {}
                _ => {
                    if u.statics.ship(t).is_some_and(|s| s.guns.as_ref().is_some_and(|g| !g.is_empty()) || t == ShipType::sRammingFrigate) {
                        fighters.push(id);
                    }
                }
            }
        }
        let enemy = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| u.ships.get(id).is_some_and(|s| s.player != HUMAN_PLAYER && !u.players[HUMAN_PLAYER].is_ally(s.player) && s.flags & (sof::HIDE | sof::DEAD) == 0 && s.flags & sof::TARGETABLE != 0))
            .filter(|&id| crate::visibility::visible_to_player(u, id, HUMAN_PLAYER))
            .min_by(|&a, &b| (u.ships.ship(a).posinfo.position - mp).mag_sq().total_cmp(&(u.ships.ship(b).posinfo.position - mp).mag_sq()));
        let mut spots: Vec<Vector> = run.game.level.volumes.iter().map(|(_, v)| match *v {
            crate::level::Volume::Sphere { center, .. } => center,
            crate::level::Volume::Box { min, max } => Vector::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5, (min.z + max.z) * 0.5),
        }).collect();
        spots.extend(run.game.level.points.iter().map(|p| Vector::new(p.1[0], p.1[1], p.1[2])));
        let u = &mut run.universe;
        if let Some(e) = enemy {
            if !fighters.is_empty() {
                crate::command::cl_attack(u, &fighters, &[e]);
            }
        }
        if !recons.is_empty() && !spots.is_empty() {
            let to = spots[self.explore % spots.len()];
            self.explore += 1;
            crate::command::cl_move(u, &recons, 0, mp, &[to]);
        }
        let idle: Vec<ShipId> = workers.into_iter().filter(|&w| u.ships.ship(w).command.is_none()).collect();
        if !idle.is_empty() {
            crate::resource::cl_collect_resource(u, &idle, None);
        }
    }
}
