//! The computer player's pass over its teams' AI moves (`aitExecute`, 0x417c00), on the KAS
//! host: see [`crate::aimove`].

use hwc_kas::host::Cx;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use super::SimHost;
use crate::aimove::{self, MoveKind, TeamMove};
use crate::command;
use crate::level::distance_squared;

impl SimHost<'_> {
    /// `aitDeleteAllTeamMoves` on the current team.
    pub(super) fn delete_all_team_moves(&mut self, cx: &Cx) {
        if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
            t.moves.delete_all();
        }
    }

    /// Adds a move to the current team (`listAddNode(&team->moves)`).
    pub(super) fn add_team_move(&mut self, cx: &Cx, m: TeamMove) {
        if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
            t.moves.add(m);
        }
    }

    /// `aitExecute`: every team's current move, in team order.
    pub fn ai_teams_execute(&mut self) {
        for ti in 0..self.g.level.teams.len() {
            self.ai_team_execute(ti);
        }
    }

    fn ai_team_execute(&mut self, ti: usize) {
        let t = &mut self.g.level.teams[ti];
        if t.moves.current.is_none() {
            if t.moves.moves.is_empty() {
                return;
            }
            t.moves.current = Some(0);
        }
        let cur = t.moves.current.expect("current move");
        if t.moves.moves[cur].move_type() == aimove::move_type::DONE {
            return;
        }
        if t.moves.delay > 0 {
            t.moves.delay -= 1;
            return;
        }
        if !self.process_move(ti) {
            return;
        }
        // The move is done: on to the next one.
        let ships = self.g.level.teams[ti].ships.clone();
        let Some(last) = self.g.level.teams[ti].moves.current else { return };
        let moves = &self.g.level.teams[ti].moves;
        let last_move = moves.moves[last].clone();
        if last + 1 >= moves.moves.len() {
            self.g.level.teams[ti].moves.current = None;
        } else {
            let next = moves.moves[last + 1].clone();
            self.g.level.teams[ti].moves.current = Some(last + 1);
            // Advanced teams have the tactics feature.
            if next.tactics != last_move.tactics && !ships.is_empty() {
                self.wrap_set_tactics(&ships, next.tactics.clamp(0, 2) as usize);
            }
            if next.formation == aimove::SAME_FORMATION {
                self.g.level.teams[ti].moves.moves[last + 1].formation = last_move.formation;
            } else if next.formation != aimove::NO_FORMATION && next.formation != last_move.formation && ships.len() > 1 {
                let ok = self.wrap_filter(&ships);
                if !ok.is_empty() {
                    command::cl_formation(self.u, &ok, next.formation as u32);
                }
            }
            self.g.level.teams[ti].moves.delay = aimove::KAS_TEAM_MOVE_DELAY;
        }
        if last_move.remove {
            let m = &mut self.g.level.teams[ti].moves;
            m.moves.remove(last);
            if let Some(c) = m.current {
                if c > last {
                    m.current = Some(c - 1);
                }
            }
        }
    }

    /// The current move's process function; true when it is done.
    fn process_move(&mut self, ti: usize) -> bool {
        let cur = self.g.level.teams[ti].moves.current.expect("current move");
        match self.g.level.teams[ti].moves.moves[cur].kind.clone() {
            MoveKind::MoveTeam { destination } => self.process_move_team(ti, cur, destination),
            MoveKind::Intercept { .. } => self.process_intercept(ti, cur),
            MoveKind::Dock { .. } => self.process_dock(ti, cur),
            MoveKind::Launch => self.process_launch(ti, cur),
        }
    }

    /// `aimProcessDock` (Cataclysm's: no health test, the purpose from the move's flags). The
    /// first pass gives the dock order and keeps a copy of the team; later passes drop the
    /// ships that are out again with no host, or that stay at a refuelling point; the move is
    /// done when none is left.
    fn process_dock(&mut self, ti: usize, cur: usize) -> bool {
        let ships = self.g.level.teams[ti].ships.clone();
        if ships.is_empty() {
            return true;
        }
        let m = &self.g.level.teams[ti].moves.moves[cur];
        let MoveKind::Dock { flags, host, .. } = m.kind.clone() else { return true };
        if !m.processing {
            let (purpose, with_host) = super::orders::aim_dock_purpose(flags);
            let host = if with_host { host } else { None };
            self.wrap_dock(&ships, purpose, host);
            let t = &mut self.g.level.teams[ti];
            if let MoveKind::Dock { docking, .. } = &mut t.moves.moves[cur].kind {
                *docking = ships;
            }
            t.moves.moves[cur].processing = true;
            t.moves.docking = true;
            return false;
        }
        let done: Vec<ShipId> = ships
            .iter()
            .copied()
            .filter(|&id| {
                self.u.ships.get(id).is_some_and(|s| {
                    let out_free = s.base.dock_with.is_none() && s.flags & crate::object::sof::HIDE == 0;
                    let staying = s.special_flags & crate::base::special::DOCK_STAY != 0 && refuelling_at_carrier_mother(s);
                    out_free || staying
                })
            })
            .collect();
        let t = &mut self.g.level.teams[ti];
        let MoveKind::Dock { docking, .. } = &mut t.moves.moves[cur].kind else { return true };
        for id in done {
            if let Some(i) = docking.iter().position(|&d| d == id) {
                docking.swap_remove(i);
            }
        }
        if docking.is_empty() {
            t.moves.docking = false;
            return true;
        }
        false
    }

    /// `aimProcessLaunch`: the team's ships inside the host of its first ship are launched
    /// (`clWrapLaunchMultipleShips`); ships staying at a refuelling point are let go instead.
    /// Until one of the two applies the move waits.
    fn process_launch(&mut self, ti: usize, cur: usize) -> bool {
        let ships = self.g.level.teams[ti].ships.clone();
        let Some(&first) = ships.first() else { return false };
        if let Some(host) = crate::base::find_ship_i_am_inside(self.u, first) {
            let inside: Vec<ShipId> = ships.iter().copied().filter(|&id| self.u.ships.ship(host).base.inside.as_ref().is_some_and(|i| i.ships.contains(&id))).collect();
            crate::dock::cl_launch_multiple_ships(self.u, &inside, host);
            self.g.level.teams[ti].moves.moves[cur].processing = true;
            return true;
        }
        let stays = self.u.ships.get(first).is_some_and(|s| s.special_flags & crate::base::special::DOCK_STAY != 0 && refuelling_at_carrier_mother(s));
        if stays {
            for &id in &ships {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.special_flags &= !crate::base::special::DOCK_STAY;
                }
            }
            self.g.level.teams[ti].moves.moves[cur].processing = true;
            return true;
        }
        false
    }

    /// `aiuWrapMove` (0x41eeb0): the fit, mobile ships move from the position of the
    /// selection's first ship. False when no ship could be ordered.
    pub(super) fn wrap_move(&mut self, sel: &[ShipId], dest: Vector) -> bool {
        let Some(&first) = sel.first() else { return false };
        let mut ok = self.wrap_filter(sel);
        if ok.is_empty() {
            return false;
        }
        self.make_mobile(&mut ok);
        let from = self.pos(first).unwrap_or(Vector::ZERO);
        if !ok.is_empty() {
            command::cl_move(self.u, &ok, command::waypoint_mode::ONCE, from, &[dest]);
        }
        true
    }

    /// `aitTeamIsFinishedMoving` (0x417320): the centre of the team is within `range` of the
    /// destination, or its first ship has nowhere to go.
    fn team_finished_moving(&self, ships: &[ShipId], dest: Vector, range: f32) -> bool {
        if ships.is_empty() {
            return true;
        }
        let c = self.centre(ships);
        let d = distance_squared(c, dest);
        let r = ld(range) * ld(range);
        let first_still = self.u.ships.get(ships[0]).is_none_or(|s| s.move_to == Vector::ZERO);
        !(r <= d) || first_still
    }

    /// `aimProcessMove` (0x407cd0).
    fn process_move_team(&mut self, ti: usize, cur: usize, dest: Vector) -> bool {
        let ships = self.g.level.teams[ti].ships.clone();
        if ships.is_empty() {
            return true;
        }
        let m = &self.g.level.teams[ti].moves.moves[cur];
        let wait = m.wait;
        if !m.processing {
            if !self.wrap_move(&ships, dest) {
                return false;
            }
            self.g.level.teams[ti].moves.moves[cur].processing = true;
        } else if self.team_finished_moving(&ships, dest, aimove::MOVE_FINISHED_RANGE) {
            return true;
        }
        !wait
    }

    /// `aimProcessIntercept` (0x408090): the first time (and after each re-aim) the team gets a
    /// move to where the target is, or halfway between it and where it is going; the intercept
    /// is done when the team's first ship is close to the target; otherwise it is re-aimed every
    /// `interval` seconds when the target has moved off.
    fn process_intercept(&mut self, ti: usize, cur: usize) -> bool {
        let MoveKind::Intercept { target, interval, next_time, destination, target_moving } = self.g.level.teams[ti].moves.moves[cur].kind.clone() else {
            return true;
        };
        let Some(ts) = self.u.ships.get(target) else { return true };
        let ships = self.g.level.teams[ti].ships.clone();
        if ships.is_empty() {
            return true;
        }
        let (tp, tm) = (ts.posinfo.position, ts.move_to);
        let now = self.u.total_time_elapsed;
        if !self.g.level.teams[ti].moves.moves[cur].processing {
            let (dest, moving) = if tm == Vector::ZERO {
                (tp, false)
            } else {
                let h = |a: f32, b: f32| st(ld(st(ld(a) + ld(b))) * 0.5);
                (Vector::new(h(tp.x, tm.x), h(tm.y, tp.y), h(tm.z, tp.z)), true)
            };
            let formation = self.g.level.teams[ti].moves.moves[cur].formation;
            {
                let m = &mut self.g.level.teams[ti].moves.moves[cur];
                m.kind = MoveKind::Intercept { target, interval, next_time: st(ld(now) + ld(interval)), destination: dest, target_moving: moving };
                m.processing = true;
            }
            let mv = TeamMove::move_team(dest, formation, false, true);
            self.g.level.teams[ti].moves.add_before_and_make_current(cur, mv);
            return false;
        }
        let first = self.pos(ships[0]).unwrap_or(Vector::ZERO);
        if distance_squared(tp, first) < ld(aimove::INTERCEPT_FINISH_DISTSQ) {
            return true;
        }
        if now < next_time {
            return false;
        }
        if !target_moving {
            if distance_squared(tp, destination) > ld(aimove::INTERCEPT_RECALC_DISTSQ) {
                self.g.level.teams[ti].moves.moves[cur].processing = false;
            }
            return false;
        }
        if tm != destination {
            self.g.level.teams[ti].moves.moves[cur].processing = false;
        }
        false
    }
}

use crate::object::ShipId;

/// `ShipIsRefuelingAtCarrierMother`: docked with a host and inside it or latched to it.
pub(super) fn refuelling_at_carrier_mother(s: &crate::object::Ship) -> bool {
    s.base.dock_with.is_some() && matches!(s.base.dock_state, crate::dock::state::INSIDE | crate::dock::state::LATCHED)
}
