//! KAS runtime functions on the simulation: see the parent module.
//!
//! The `kasf*` functions of this group are written as methods of the `KasRuntime` impl in
//! the macro below, which the parent invokes inside that impl.
//!
//! This group holds the labelled-point arithmetic (`PointAdd` .. `PointScalarMult`, the
//! original `kasfVector*`) and the orders and ship-state switches KAS gives to teams and
//! selections. Team orders that the original routes through the AI move layer
//! (`aimCreate*` followed by `aimProcess*` on the team's next AI pass) are issued at once
//! through the command layer, as the parent's `MoveTo` does.

#![allow(unused_imports)]

use hwc_kas::host::{Cx, Point, Ships, Team};
use hwc_math::fpu::{fild_i32, fild_u32, ld, st, Ext};
use hwc_math::Vector;

use hwc_kas::Hash;

use crate::command;
use crate::mission::LogKind;
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipType};

/// `ShipDisableForTime` duration used by `ShipsSetHelpless` (a practically endless time).
pub(super) const HELPLESS_DURATION_BITS: u32 = 0x6258_d727;
/// Special flags (+0x1c8): KAS asked the ship not to retaliate.
pub(super) const NON_RETALIATION: u32 = 0x0004_0000;
/// Special flags 2 (+0x1cc): the ship may be hurt while the letterbox is up (cleared by
/// `ShipsSetInvincibleInWidescreen(ships, TRUE)`).
pub(super) const VULNERABLE_IN_WIDESCREEN: u32 = 0x0100_0000;
/// Special flags 2 (+0x1cc): not under the player's control (no support units charged).
pub(super) const NOT_PLAYER_CONTROLLED: u32 = 0x0010_0000;
/// Special flags 2 (+0x1cc) bit 3: under construction (left out of AI orders).
pub(super) const UNDER_CONSTRUCTION: u32 = 0x0000_0008;
/// `flags2` (+0x18) bit 5, set by `ShipsEffectedByMines(ships, 1)`.
pub(super) const MINES_FLAG: u32 = 0x0000_0020;
/// The disable reason `MakeShipsControllable` sets.
pub(super) const REASON_NOT_CONTROLLABLE: usize = 1;
/// `suShipHasUpgrade` bit that makes a ship repair-capable.
pub(super) const REPAIR_UPGRADE: u32 = 0x0004_0000;
/// Formation number `ShipsSetFormation` treats as the military parade.
pub(super) const PARADE_FORMATION: i32 = 8;

/// Clamps a point into the world box (`kasfVector*` after writing their result): above the
/// bound becomes the bound, below its negation becomes the negation (unordered values stay).
pub(super) fn clamp_to_world(v: &mut [f32; 4], bounds: Vector) {
    for (c, b) in v.iter_mut().take(3).zip([bounds.x, bounds.y, bounds.z]) {
        if *c > b {
            *c = b;
        } else if -b > *c {
            *c = -b;
        }
    }
}

/// `kasfVectorParametric`: `result = (maxt - t) / maxt * start`, then `+= t / maxt * end`,
/// component by component. `start` / `end` of `None` mean the operand is the result itself
/// (the original reads through the same pointer, so it sees the partly written result).
pub(super) fn parametric(r: &mut [f32; 4], start: Option<[f32; 4]>, end: Option<[f32; 4]>, t: i32, maxt: i32) {
    let a: Ext = fild_i32(maxt.wrapping_sub(t)) / fild_i32(maxt);
    for k in 0..3 {
        let s = start.map_or(r[k], |s| s[k]);
        r[k] = st(a * ld(s));
    }
    let b: Ext = fild_i32(t) / fild_i32(maxt);
    for k in 0..3 {
        let e = end.map_or(r[k], |e| e[k]);
        r[k] = st(b * ld(e) + ld(r[k]));
    }
}

/// `kasfVectorScalarMult`: the point times `numerator / denominator` (no clamping).
pub(super) fn scalar_mult(r: &mut [f32; 4], numerator: i32, denominator: i32) {
    let f: Ext = fild_i32(numerator) / fild_i32(denominator);
    for c in r.iter_mut().take(3) {
        *c = st(f * ld(*c));
    }
}

/// `aiuGenerateRandomStandoffPoint` with no side preference (as `kasfVectorNear` calls it):
/// three draws of random stream 6 make a direction in [-10, 10)^3, which is normalised and
/// scaled by `distance` around `near`. The first component keeps its product in the register.
pub(super) fn random_standoff(draws: [u32; 3], near: Vector, distance: f32) -> Vector {
    let unit = f32::from_bits(0x31a0_0000); // 4.656613e-09 (20 / 2^32)
    let c = |r: u32| st(fild_u32(r) * ld(unit) - 10.0);
    let n = Vector::new(c(draws[0]), c(draws[1]), c(draws[2])).copy_and_normalize();
    let d = ld(distance);
    let y = st(ld(n.y) * d);
    let z = st(ld(n.z) * d);
    Vector::new(st(ld(near.x) + ld(n.x) * d), st(ld(near.y) + ld(y)), st(ld(near.z) + ld(z)))
}

/// `aimProcessDock`'s dock purpose for the kinds `kasfDock*` create: bit 1 picks the
/// dock-and-stay purposes, bit 2 the instant ones; bit 0 says the host is given.
pub(super) fn aim_dock_purpose(kind: u32) -> (i32, bool) {
    let mut p = i32::from(kind & 2 != 0);
    if kind & 4 != 0 {
        p += 8;
    }
    (p, kind & 1 != 0)
}

impl super::SimHost<'_> {
    /// `ShipAlreadyHyperspaceOut`: hyperspacing, or waiting in the hyperspace list.
    pub(super) fn hyperspaced_out(&self, id: ShipId) -> bool {
        self.u.ships.get(id).is_some_and(|s| s.flags & sof::HYPERSPACE != 0) || self.g.hyper.in_hyperspace.contains(&id)
    }

    /// The test every `aiuWrap*` applies before giving an order: not crazy or disabled, not
    /// gone to hyperspace, not under construction.
    pub(super) fn wrap_ok(&self, id: ShipId) -> bool {
        self.u.ships.get(id).is_some_and(|s| s.flags & (sof::CRAZY | sof::DISABLED) == 0 && s.special_flags2 & UNDER_CONSTRUCTION == 0)
            && !self.hyperspaced_out(id)
    }

    pub(super) fn wrap_filter(&self, sel: &[ShipId]) -> Vec<ShipId> {
        sel.iter().copied().filter(|&id| self.wrap_ok(id)).collect()
    }

    /// `aiuWrapDock` + `clWrapDock`: the ships fit for orders (less those already docking or
    /// launching with a host) dock with `host` (when it is fit too), or each with the nearest
    /// ship it can dock at. Returns whether an order was given.
    pub(super) fn wrap_dock(&mut self, sel: &[ShipId], purpose: i32, host: Option<ShipId>) -> bool {
        let mut ok = self.wrap_filter(sel);
        if ok.is_empty() {
            return false;
        }
        if let Some(h) = host {
            if !self.wrap_ok(h) {
                return false;
            }
        }
        command::make_ships_controllable(self.u, &mut ok);
        if !ok.is_empty() {
            crate::dock::cl_dock(self.u, &ok, purpose, host);
        }
        true
    }

    /// `aiuWrapSetTactics`.
    pub(super) fn wrap_set_tactics(&mut self, sel: &[ShipId], tactics: usize) -> bool {
        let ok = self.wrap_filter(sel);
        if ok.is_empty() {
            return false;
        }
        command::cl_set_tactics(self.u, &ok, tactics);
        true
    }

    /// The team's dock move (`aitDeleteAllTeamMoves` + `aimCreateDock`): `kind` is the flags
    /// without a host; with a fit first ship of `with_team` the move docks with it
    /// (`kind | AT_SHIP`). The next computer-player pass gives the order ([`crate::aimove`]).
    pub(super) fn team_dock(&mut self, cx: &Cx, with_team: Team, kind: u32) {
        let first = with_team.and_then(|h| self.g.level.team(h)).and_then(|t| t.ships.first().copied());
        let host = first.filter(|&h| self.wrap_ok(h));
        let kind = if host.is_some() { kind | crate::aimove::dock_flags::AT_SHIP } else { kind };
        self.delete_all_team_moves(cx);
        self.add_team_move(cx, crate::aimove::TeamMove::dock(kind, host, false, true));
    }

    /// A point argument with its fourth component: labelled points as stored, the centres of
    /// selections, teams and volumes with 1.
    pub(super) fn point4(&self, p: Point) -> Option<[f32; 4]> {
        if let Point::Label(h) = p {
            return self.g.level.point(h);
        }
        self.point(p).map(|v| [v.x, v.y, v.z, 1.0])
    }

    /// Writes a point result. Only labelled points keep it: the centres of selections, teams
    /// and volumes are temporaries in the original.
    pub(super) fn store_point(&mut self, p: Point, v: [f32; 4], func: &str) {
        match p {
            Point::Label(h) => match self.g.level.points.iter_mut().find(|q| q.0 == h) {
                Some(q) => q.1 = v,
                // (The original writes through a NULL vector here.)
                None => {
                    let n = self.g.name(h);
                    self.log(LogKind::Warning, format!("{func}: unknown point {n}"));
                }
            },
            Point::Null => self.log(LogKind::Warning, format!("{func}: NULL result")),
            _ => {}
        }
    }

    /// `kasLabelledVectorAdd` or an overwrite of the existing point of that label.
    pub(super) fn set_labelled_point(&mut self, label: Hash, v: [f32; 4]) {
        match self.g.level.points.iter_mut().find(|q| q.0 == label) {
            Some(q) => q.1 = v,
            None => self.g.level.points.push((label, v)),
        }
    }

    /// The `kasfVectorAddTo` / `kasfVectorSubFrom` body.
    pub(super) fn point_add_sub(&mut self, result: Point, other: Point, sign: f32, func: &str) {
        let (Some(mut r), Some(o)) = (self.point4(result), self.point4(other)) else {
            self.log(LogKind::Warning, format!("{func}: NULL point"));
            return;
        };
        for k in 0..3 {
            r[k] = if sign > 0.0 { st(ld(r[k]) + ld(o[k])) } else { st(ld(r[k]) - ld(o[k])) };
        }
        clamp_to_world(&mut r, self.u.world_bounds);
        self.store_point(result, r, func);
    }

    /// `FUN_005aea30` (the controllable switch of `kasfMakeShipsControllable`): the support
    /// units follow the not-player-controlled bit.
    pub(super) fn set_uncontrollable(&mut self, id: ShipId, uncontrollable: bool) {
        let Some(bit) = self.u.ships.get(id).map(|s| s.special_flags2 & NOT_PLAYER_CONTROLLED != 0) else { return };
        if uncontrollable && !bit {
            crate::construction::remove_ship_support_units(self.u, id);
        } else if !uncontrollable && bit {
            self.u.ships.ship_mut(id).special_flags2 &= !NOT_PLAYER_CONTROLLED;
            crate::construction::add_ship_support_units(self.u, id);
        }
        let s = self.u.ships.ship_mut(id);
        s.special_flags2 = (s.special_flags2 & !NOT_PLAYER_CONTROLLED) | if uncontrollable { NOT_PLAYER_CONTROLLED } else { 0 };
    }

    /// `isShipRepairCapable && WorkerIsWorker`: a worker with the repair upgrade.
    pub(super) fn repair_worker(&self, id: ShipId) -> bool {
        self.u.ships.get(id).is_some_and(|s| {
            let t = s.ship_type.index();
            crate::upgrade::has_upgrade(s, REPAIR_UPGRADE) && t != 0x10 && t != 0x33 && (t == 0xe || t == 0x31)
        })
    }

    /// `MakeShipsHarvestCapable(.., FALSE)`: resource-class ships that harvest, when
    /// harvesting is on (always in single player).
    pub(super) fn harvest_capable(&self, id: ShipId) -> bool {
        let enabled = self.u.single_player || self.u.base.options.flags & crate::base::game_flags::HARVESTING_ENABLED != 0;
        enabled
            && self.u.ships.get(id).and_then(|s| self.u.statics.ship(s.ship_type)).is_some_and(|st_| st_.ship_class == Some(ShipClass::Resource) && 0 < st_.harvest_amount)
    }

    /// `MakeShipsMobileFlagged(sel, FALSE)`: immobile ships leave the selection (the last
    /// entry moves into the hole and is looked at next).
    pub(super) fn make_mobile(&self, sel: &mut Vec<ShipId>) {
        let mut i = 0;
        while i < sel.len() {
            let immobile = self.u.ships.get(sel[i]).and_then(|s| self.u.statics.ship(s.ship_type)).is_some_and(|st_| st_.header.immobile);
            if immobile {
                sel.swap_remove(i);
            } else {
                i += 1;
            }
        }
    }
}

macro_rules! orders_methods {
    () => {
        // ---- labelled points (kasfVector*) -------------------------------------------------

        /// `kasfVectorAdd`: sets (or creates) the labelled point, fourth component 1.
        fn point_add(&mut self, _cx: &Cx, label: Hash, x: i32, y: i32, z: i32) {
            let v = [st(fild_i32(x)), st(fild_i32(y)), st(fild_i32(z)), 1.0];
            self.set_labelled_point(label, v);
        }

        /// `kasfVectorCopy`: all four components into (a new) labelled point.
        fn point_copy(&mut self, _cx: &Cx, label: Hash, vec: Point) {
            let Some(v) = self.point4(vec) else {
                self.log(LogKind::Warning, "PointCopy: NULL source");
                return;
            };
            self.set_labelled_point(label, v);
        }

        /// `kasfVectorParametric`.
        fn point_parametric(&mut self, _cx: &Cx, result: Point, start: Point, end: Point, t: i32, maxt: i32) {
            let (Some(mut r), Some(s), Some(e)) = (self.point4(result), self.point4(start), self.point4(end)) else {
                self.log(LogKind::Warning, "PointParametric: NULL point");
                return;
            };
            let alias = |p: Point, v: [f32; 4]| if matches!(p, Point::Label(_)) && p == result { None } else { Some(v) };
            orders::parametric(&mut r, alias(start, s), alias(end, e), t, maxt);
            orders::clamp_to_world(&mut r, self.u.world_bounds);
            self.store_point(result, r, "PointParametric");
        }

        /// `kasfVectorNear`: a random point `distance` away from `near` (fourth component 1).
        fn point_near(&mut self, _cx: &Cx, result: Point, near: Point, distance: i32) {
            let Some(n) = self.point(near) else {
                self.log(LogKind::Warning, "PointNear: NULL point");
                return;
            };
            let draws = [self.u.randy.random(6), self.u.randy.random(6), self.u.randy.random(6)];
            let p = orders::random_standoff(draws, n, st(fild_i32(distance)));
            let mut r = [p.x, p.y, p.z, 1.0];
            orders::clamp_to_world(&mut r, self.u.world_bounds);
            self.store_point(result, r, "PointNear");
        }

        /// `kasfVectorAddTo`.
        fn point_add_to(&mut self, _cx: &Cx, result: Point, add: Point) {
            self.point_add_sub(result, add, 1.0, "PointAddTo");
        }

        /// `kasfVectorSubFrom`.
        fn point_sub_from(&mut self, _cx: &Cx, result: Point, sub: Point) {
            self.point_add_sub(result, sub, -1.0, "PointSubFrom");
        }

        /// `kasfVectorScalarMult` (the only one that does not clamp to the world).
        fn point_scalar_mult(&mut self, _cx: &Cx, result: Point, numerator: i32, denominator: i32) {
            let Some(mut r) = self.point4(result) else {
                self.log(LogKind::Warning, "PointScalarMult: NULL point");
                return;
            };
            orders::scalar_mult(&mut r, numerator, denominator);
            self.store_point(result, r, "PointScalarMult");
        }

        // ---- orders ------------------------------------------------------------------------

        /// `kasfShipsMoveTo` (`aiuWrapMove`): the fit, mobile ships move from the position of
        /// the selection's first ship.
        fn ships_move_to(&mut self, _cx: &Cx, ships: Ships, destination: Point) {
            let Some(dest) = self.point(destination) else {
                self.log(LogKind::Warning, "ShipsMoveTo: NULL destination");
                return;
            };
            let sel = self.ships(ships);
            let mut ok = self.wrap_filter(&sel);
            if ok.is_empty() {
                return;
            }
            self.make_mobile(&mut ok);
            let from = self.pos(sel[0]).unwrap_or(Vector::ZERO);
            command::cl_move(self.u, &ok, command::waypoint_mode::ONCE, from, &[dest]);
        }

        /// `kasfShipsStop` (`clWrapHalt`).
        fn ships_stop(&mut self, _cx: &Cx, ship: Ships) {
            let sel = self.ships(ship);
            command::cl_halt(self.u, &sel);
        }

        /// `kasfStopInstant`: the team halts and loses all its motion at once.
        fn stop_instant(&mut self, cx: &Cx) {
            self.delete_all_team_moves(cx);
            let sel = self.team_ships(cx);
            command::cl_halt(self.u, &sel);
            for id in sel {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.posinfo.velocity = Vector::ZERO;
                    s.posinfo.force = Vector::ZERO;
                    s.rotinfo.rotspeed = Vector::ZERO;
                    s.rotinfo.torque = Vector::ZERO;
                }
            }
        }

        /// `kasfIntercept` (0x48db00): the team's moves are replaced by an intercept of the
        /// first target (re-aimed every 2 s, same formation, the team's KAS tactics), which
        /// the AI passes carry out until the team is close (`aimProcessIntercept`).
        fn intercept(&mut self, cx: &Cx, targets: Ships) {
            let Some(&t) = self.ships(targets).first() else { return };
            self.delete_all_team_moves(cx);
            let tactics = self.team(cx).map_or(1, |tm| tm.kas_tactics);
            self.add_team_move(cx, crate::aimove::TeamMove::intercept(t, 2.0, crate::aimove::SAME_FORMATION, tactics, true, true));
        }

        /// `kasfKamikaze` (`aimProcessKamikaze`): aggressive tactics, attack the targets, then
        /// the attackers turn kamikaze.
        fn kamikaze(&mut self, cx: &Cx, targets: Ships) {
            let t = self.ships(targets);
            if t.is_empty() {
                return;
            }
            self.delete_all_team_moves(cx);
            let sel = self.team_ships(cx);
            if sel.is_empty() || !self.wrap_set_tactics(&sel, 2) {
                return;
            }
            let ok = self.wrap_filter(&sel);
            if ok.is_empty() {
                return;
            }
            command::cl_attack(self.u, &ok, &t);
            let ok = self.wrap_filter(&sel);
            command::cl_set_kamikaze(self.u, &ok);
        }

        /// `kasfHarvest`: the team's harvesters collect resources.
        fn harvest(&mut self, cx: &Cx) {
            let sel = self.team_ships(cx);
            if sel.is_empty() {
                return;
            }
            let h: Vec<ShipId> = sel.into_iter().filter(|&id| self.harvest_capable(id)).collect();
            if !h.is_empty() {
                crate::resource::cl_collect_resource(self.u, &h, None);
            }
        }

        /// `kasfRepair`: the team's repair-capable workers repair the ships.
        fn repair(&mut self, cx: &Cx, ships: Ships) {
            let sel = self.team_ships(cx);
            if sel.is_empty() {
                return;
            }
            let workers: Vec<ShipId> = sel.into_iter().filter(|&id| self.repair_worker(id)).collect();
            let targets = self.ships(ships);
            crate::ships::worker_repair::cl_repair(self.u, &workers, &targets);
        }

        /// `kasfShipsGuardShips` (0x48dfc0): `clWrapProtect` (the guard selection loses its
        /// docking and launching ships, in place).
        fn ships_guard_ships(&mut self, _cx: &Cx, guard: Ships, ships: Ships) {
            let protect = self.ships(ships);
            let mut sel = self.ships(guard);
            if sel.is_empty() || protect.is_empty() {
                return;
            }
            command::make_ships_controllable(self.u, &mut sel);
            self.set_ships(guard, sel.clone());
            if !sel.is_empty() {
                command::cl_wrap_protect(self.u, &sel, &protect);
            }
        }

        // ---- docking -----------------------------------------------------------------------

        /// `kasfDockSupportWith`: dock (for support) with the team's first ship, or the
        /// nearest.
        fn dock_support_with(&mut self, cx: &Cx, with_team: Team) {
            self.team_dock(cx, with_team, 0);
        }

        /// `kasfDock` / `kasfDockSupport` (one function, 0x48fb90): a dock move with the
        /// nearest host (the team argument is not read).
        fn dock(&mut self, cx: &Cx, _with_team: Team) {
            self.team_dock(cx, None, 0);
        }

        fn dock_support(&mut self, cx: &Cx) {
            self.team_dock(cx, None, 0);
        }

        /// `kasfShipsDockSupportWith`: the ships dock (refuel/repair) with the first host.
        fn ships_dock_support_with(&mut self, _cx: &Cx, ships: Ships, with_ships: Ships) {
            let sel = self.ships(ships);
            let host = self.ships(with_ships).first().copied();
            self.wrap_dock(&sel, crate::dock::purpose::DOCK, host);
        }

        /// `kasfDockStayMothership`: a dock-and-stay move with the mothership of the first
        /// ship's player, when it is fit for orders.
        fn dock_stay_mothership(&mut self, cx: &Cx) {
            let first = self.team_ships(cx).first().copied();
            let ms = first.and_then(|id| self.u.ships.get(id)).and_then(|s| self.u.base.players.get(s.player)).and_then(|p| p.mothership);
            if let Some(ms) = ms.filter(|&m| self.wrap_ok(m)) {
                self.delete_all_team_moves(cx);
                self.add_team_move(cx, crate::aimove::TeamMove::dock(crate::aimove::dock_flags::AT_SHIP | crate::aimove::dock_flags::STAY, Some(ms), false, true));
            }
        }

        /// `kasfTeamDocking`: the team's `TEAM_DOCKING` flag.
        fn team_docking(&mut self, cx: &Cx) -> i32 {
            i32::from(cx.team.and_then(|h| self.g.level.team(h)).is_some_and(|t| t.moves.docking))
        }

        /// `kasfShipsLaunch`: the ships are marked as launching and ordered to launch from the
        /// host (`aiuWrapDock` purpose launch).
        fn ships_launch(&mut self, _cx: &Cx, ships: Ships, host: Ships) {
            let sel = self.ships(ships);
            let h = self.ships(host).first().copied();
            self.mark_launching(&sel);
            self.wrap_dock(&sel, crate::dock::purpose::LAUNCH, h);
        }

        /// `kasfShipsFinishedLaunching`: every ship is out of its launch cone and in the
        /// universe (true for an empty list).
        fn ships_finished_launching(&mut self, _cx: &Cx, ships: Ships) -> i32 {
            let sel = self.ships(ships);
            i32::from(self.finished_launching(&sel))
        }

        /// `kasfSlipgateDockInstant`: the team is put inside the slipgate at once.
        fn slipgate_dock_instant(&mut self, cx: &Cx, slipgate: Ships) {
            let sel = self.team_ships(cx);
            if sel.is_empty() {
                return;
            }
            let Some(gate) = self.ships(slipgate).first().copied() else { return };
            self.put_inside(&sel, gate);
            if self.u.ships.get(gate).is_some_and(|h| matches!(h.ship_type.index(), 8 | 0x2a)) {
                crate::ships::hive::reset_swarmers(self.u, gate);
            }
        }

        /// `kasfDockStay`: dock and stay with the team's first ship, or the nearest.
        fn dock_stay(&mut self, cx: &Cx, with_team: Team) {
            self.team_dock(cx, with_team, crate::aimove::dock_flags::STAY);
        }

        /// `kasfShipsDockStay`: dock and stay (in mission 9 the selection led by ship type
        /// 0x58 retires instead).
        fn ships_dock_stay(&mut self, _cx: &Cx, ships: Ships, with_ships: Ships) {
            let sel = self.ships(ships);
            let host = self.ships(with_ships).first().copied();
            let retire = sel.first().and_then(|&id| self.u.ships.get(id)).is_some_and(|s| s.ship_type.index() == 0x58) && self.u.base.options.mission == 9;
            let purpose = if retire { crate::dock::purpose::RETIRE } else { crate::dock::purpose::DOCK_STAY };
            self.wrap_dock(&sel, purpose, host);
        }

        /// `kasfShipsDockInstant`: the ships are put inside the host's first ship at once
        /// (`univRemoveShipFromOutside`, `dockPutShipInside`; the hive frigates reset their
        /// swarmers).
        fn ships_dock_instant(&mut self, _cx: &Cx, ship_sel: Ships, host_sel: Ships) {
            let Some(&host) = self.ships(host_sel).first() else {
                self.log(LogKind::Warning, "ShipsDockInstant: no host");
                return;
            };
            let sel = self.ships(ship_sel);
            self.put_inside(&sel, host);
            if self.u.ships.get(host).is_some_and(|h| matches!(h.ship_type.index(), 8 | 0x2a)) {
                crate::ships::hive::reset_swarmers(self.u, host);
            }
        }

        /// `kasfSlipgateDock`: the team docks with the slipgate (purpose slipgate).
        fn slipgate_dock(&mut self, cx: &Cx, slipgate: Ships) {
            let sel = self.team_ships(cx);
            if sel.is_empty() {
                return;
            }
            let gate = self.ships(slipgate).first().copied();
            self.wrap_dock(&sel, crate::dock::purpose::SLIPGATE, gate);
        }

        /// `kasfEnteringSlipgate`: a ship of the team has a slipgate dock order.
        fn entering_slipgate(&mut self, cx: &Cx) -> i32 {
            let sel = self.team_ships(cx);
            let entering = sel.iter().any(|&id| {
                self.u.ships.get(id).and_then(|s| s.command).and_then(|c| self.u.commands.get(c)).is_some_and(|c| {
                    c.order == command::order::DOCK && c.dock.purpose == crate::dock::purpose::SLIPGATE
                })
            });
            i32::from(entering)
        }

        // ---- formations --------------------------------------------------------------------

        fn formation_sphere(&mut self, cx: &Cx) {
            self.team_formation(cx, crate::formation::kind::SPHERE);
        }

        /// `kasfShipsSetFormation` (0x48dd60): formation 8 is the military parade
        /// (`clWrapSetMilitaryParade`), any other a formation.
        fn ships_set_formation(&mut self, _cx: &Cx, ships: Ships, formation: i32) {
            let sel = self.ships(ships);
            if formation == orders::PARADE_FORMATION {
                let ok = self.wrap_filter(&sel);
                command::cl_set_military_parade(self.u, &ok);
                return;
            }
            command::cl_formation(self.u, &sel, formation as u32);
        }

        /// `kasfForceParadeFormation` (0x495c10): every ship of the fleet stops what it is doing,
        /// joins the parade around the first ship of `mothership` and is put at its slot at once
        /// (`setMilitaryParade`).
        fn force_parade_formation(&mut self, _cx: &Cx, mothership: Ships, fleet: Ships) {
            let (ms, fl) = (self.ships(mothership), self.ships(fleet));
            let (Some(&around), false) = (ms.first(), fl.is_empty()) else { return };
            let live: Vec<ShipId> = fl.into_iter().filter(|&id| self.live(id)).collect();
            crate::parade::force_parade(self.u, around, &live);
        }

        // ---- tactics -----------------------------------------------------------------------

        fn tactics_aggressive(&mut self, cx: &Cx) {
            self.team_tactics(cx, 2);
        }

        fn tactics_neutral(&mut self, cx: &Cx) {
            self.team_tactics(cx, 1);
        }

        fn tactics_evasive(&mut self, cx: &Cx) {
            self.team_tactics(cx, 0);
        }

        /// `kasfShipsSetNonRetaliation`.
        fn ships_set_non_retaliation(&mut self, _cx: &Cx, ships: Ships) {
            for id in self.ships(ships) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.special_flags |= orders::NON_RETALIATION;
                }
            }
        }

        /// `kasfShipsSetRetaliation`.
        fn ships_set_retaliation(&mut self, _cx: &Cx, ships: Ships) {
            for id in self.ships(ships) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.special_flags &= !orders::NON_RETALIATION;
                }
            }
        }

        // ---- ship state --------------------------------------------------------------------

        /// `kasfShipsSetMaxVelocity`.
        fn ships_velocity_max_set(&mut self, _cx: &Cx, ships: Ships, max_velocity: i32) {
            let sel = self.ships(ships);
            self.set_max_velocity(&sel, Some(max_velocity));
        }

        /// `kasfShipsClearMaxVelocity`.
        fn ships_velocity_max_clear(&mut self, _cx: &Cx, ships: Ships) {
            let sel = self.ships(ships);
            self.set_max_velocity(&sel, None);
        }

        /// `kasfTeamSetMaxVelocity`.
        fn team_velocity_max_set(&mut self, cx: &Cx, max_velocity: i32) {
            let sel = self.team_ships(cx);
            self.set_max_velocity(&sel, Some(max_velocity));
        }

        /// `kasfTeamClearMaxVelocity`.
        fn team_velocity_max_clear(&mut self, cx: &Cx) {
            let sel = self.team_ships(cx);
            self.set_max_velocity(&sel, None);
        }

        /// `kasfTeamAttributesBitSet`.
        fn team_attributes_bit_set(&mut self, cx: &Cx, attributes: i32) {
            for id in self.team_ships(cx) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.attributes |= attributes as u32;
                }
            }
        }

        /// `kasfShipsSetHelpless`: disabled for (practically) ever, or re-enabled.
        fn ships_set_helpless(&mut self, _cx: &Cx, ships: Ships, helpless: i32) {
            for id in self.ships(ships) {
                if !self.live(id) {
                    continue;
                }
                if helpless == 0 {
                    crate::disable::re_enable(self.u.ships.ship_mut(id));
                } else {
                    crate::disable::disable_for_time(self.u, id, f32::from_bits(orders::HELPLESS_DURATION_BITS));
                }
            }
        }

        /// `kasfMakeShipsControllable`.
        fn make_ships_controllable(&mut self, _cx: &Cx, ships: Ships, controllable: i32) {
            let off = controllable == 0;
            for id in self.ships(ships) {
                if !self.live(id) {
                    continue;
                }
                self.set_uncontrollable(id, off);
                crate::base::set_disable_reason(self.u.ships.ship_mut(id), off, orders::REASON_NOT_CONTROLLABLE);
            }
        }

        /// `kasfShipsEffectedByMines`: bit 5 of `flags2` follows the argument's low bit.
        fn ships_effected_by_mines(&mut self, _cx: &Cx, ships: Ships, effected_by_mines: i32) {
            for id in self.ships(ships) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    s.flags2 = (s.flags2 & !orders::MINES_FLAG) | ((effected_by_mines as u32 & 1) << 5);
                }
            }
        }

        /// `kasfShipsSetInvincibleInWidescreen`: TRUE clears the vulnerable-in-widescreen bit.
        fn ships_set_invincible_in_widescreen(&mut self, _cx: &Cx, p_ships: Ships, b_invincible_in_widescreen: i32) {
            for id in self.ships(p_ships) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    let bit = if b_invincible_in_widescreen == 0 { orders::VULNERABLE_IN_WIDESCREEN } else { 0 };
                    s.special_flags2 = (s.special_flags2 & !orders::VULNERABLE_IN_WIDESCREEN) | bit;
                }
            }
        }

        /// `kasfShipsSetPercentDamaged`: health to a percentage of the maximum.
        fn ships_health_set(&mut self, _cx: &Cx, ships: Ships, percent_health: i32) {
            let statics = self.u.statics.clone();
            for id in self.ships(ships) {
                if let Some(s) = self.u.ships.get_mut(id) {
                    let m = statics.ship(s.ship_type).map_or(0.0, |st_| st_.health.max_health);
                    s.health = st(fild_i32(percent_health) * ld(m) * ld(0.01f32));
                }
            }
        }

        /// `kasfShipsDamage`: `ApplyDamageToTarget` with no attacker.
        fn ships_damage(&mut self, _cx: &Cx, ships: Ships, points: i32) {
            let dmg = st(fild_i32(points));
            for id in self.ships(ships) {
                if self.live(id) {
                    crate::damage::apply_damage_to_target(self.u, id, dmg, 0, 0, 0, None);
                }
            }
        }

        /// `kasfShipsScuttle` (0x493890): [`command::cl_scuttle`]. (In the tutorial only with
        /// its scuttle flag, not modelled.)
        fn ships_scuttle(&mut self, _cx: &Cx, ships: Ships) {
            let sel = self.ships(ships);
            if sel.is_empty() {
                return;
            }
            if self.u.base.options.tutorial {
                self.log(LogKind::Flow, "ShipsScuttle: ignored in the tutorial");
                return;
            }
            let live: Vec<ShipId> = sel.into_iter().filter(|&id| self.live(id)).collect();
            command::cl_scuttle(self.u, &live);
        }

        /// `kasfDeleteShips`: the ships leave the universe at once (no explosion) and every
        /// selection and team.
        fn delete_ships(&mut self, _cx: &Cx, shipstodelete: Ships) {
            let sel = self.ships(shipstodelete);
            for id in sel {
                if self.live(id) {
                    if crate::ships::mimic::is_disguised(self.u, id) {
                        crate::ships::mimic::end_disguise(self.u, id, false);
                    }
                    crate::damage::really_delete_ship(self.u, id);
                }
                self.g.ship_removed(id);
            }
            if let Some(v) = self.ships_mut(shipstodelete) {
                v.clear();
            }
        }

        /// `kasfShipsCreate`: `count` ships of the type for player `owner` at the origin.
        fn ships_create(&mut self, _cx: &Cx, created_ships: Ships, shiptype: Str<'_>, count: i32, owner: i32) {
            self.set_ships(created_ships, Vec::new());
            let Some(t) = shiptype.text().and_then(ShipType::from_name) else { return };
            let mission = self.g.config.mission;
            let mut v = Vec::new();
            for _ in 0..count.max(0) {
                match level::create_ship(self.u, &mut self.g.level, mission, t, owner.max(0) as usize, Vector::ZERO) {
                    Ok(id) => v.push(id),
                    Err(e) => self.g.log(LogKind::Warning, format!("ShipsCreate: {e}")),
                }
            }
            if let Some(dst) = self.ships_mut(created_ships) {
                dst.extend(v);
            }
        }
    };
}
pub(super) use orders_methods;

impl super::SimHost<'_> {
    /// `kasfTactics*`: the team's KAS tactics and `aiuWrapSetTactics`.
    pub(super) fn team_tactics(&mut self, cx: &Cx, tactics: i32) {
        if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
            t.kas_tactics = tactics;
        }
        let sel = self.team_ships(cx);
        self.wrap_set_tactics(&sel, tactics as usize);
    }

    /// `kasf*SetMaxVelocity` / `kasf*ClearMaxVelocity`.
    pub(super) fn set_max_velocity(&mut self, sel: &[ShipId], v: Option<i32>) {
        for &id in sel {
            let Some(s) = self.u.ships.get_mut(id) else { continue };
            match v {
                Some(v) => {
                    s.special_max_velocity = st(fild_i32(v));
                    s.special_flags |= crate::object::special::CUSTOM_MAX_VELOCITY;
                }
                None => s.special_flags &= !crate::object::special::CUSTOM_MAX_VELOCITY,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_keeps_inside_and_clips_outside() {
        let b = Vector::new(100.0, 200.0, 300.0);
        let mut v = [150.0, -250.0, 10.0, 1.0];
        clamp_to_world(&mut v, b);
        assert_eq!(v, [100.0, -200.0, 10.0, 1.0]);
        let mut n = [f32::NAN, 200.0, -300.0, 7.0];
        clamp_to_world(&mut n, b);
        assert!(n[0].is_nan());
        assert_eq!(&n[1..], &[200.0, -300.0, 7.0]);
    }

    #[test]
    fn parametric_interpolates() {
        let mut r = [0.0; 4];
        parametric(&mut r, Some([0.0, 10.0, -10.0, 1.0]), Some([100.0, 20.0, 10.0, 1.0]), 1, 4);
        assert_eq!(&r[..3], &[25.0, 12.5, -5.0]);
        assert_eq!(r[3], 0.0);
    }

    #[test]
    fn parametric_aliased_end_reads_partial_result() {
        // result == end: the second pass reads the already scaled start.
        let mut r = [8.0, 8.0, 8.0, 1.0];
        parametric(&mut r, Some([4.0, 4.0, 4.0, 1.0]), None, 1, 2);
        // first pass: 0.5 * 4 = 2; second: 0.5 * 2 + 2 = 3.
        assert_eq!(&r[..3], &[3.0, 3.0, 3.0]);
    }

    #[test]
    fn scalar_mult_scales_xyz_only() {
        let mut r = [3.0, -6.0, 9.0, 2.0];
        scalar_mult(&mut r, 2, 3);
        assert_eq!(r, [2.0, -4.0, 6.0, 2.0]);
    }

    #[test]
    fn dock_purposes() {
        assert_eq!(aim_dock_purpose(0), (crate::dock::purpose::DOCK, false));
        assert_eq!(aim_dock_purpose(1), (crate::dock::purpose::DOCK, true));
        assert_eq!(aim_dock_purpose(2), (crate::dock::purpose::DOCK_STAY, false));
        assert_eq!(aim_dock_purpose(3), (crate::dock::purpose::DOCK_STAY, true));
        assert_eq!(aim_dock_purpose(6), (crate::dock::purpose::INSTANT_STAY, false));
        assert_eq!(aim_dock_purpose(7), (crate::dock::purpose::INSTANT_STAY, true));
    }

    #[test]
    fn standoff_is_at_distance() {
        let near = Vector::new(1000.0, -500.0, 250.0);
        let p = random_standoff([0x1234_5678, 0x9abc_def0, 0x0fed_cba9], near, 300.0);
        let d = ((p.x - near.x).powi(2) + (p.y - near.y).powi(2) + (p.z - near.z).powi(2)).sqrt();
        assert!((d - 300.0).abs() < 0.5, "{d}");
    }
}
