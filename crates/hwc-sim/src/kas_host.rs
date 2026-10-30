//! The KAS runtime functions on the simulation ([`hwc_kas::KasRuntime`] / [`hwc_kas::KasHost`]).
//!
//! [`SimHost`] borrows the universe and the mission's game-side state for one `kasExecute` (or
//! `kasMissionStart`). Each implemented method follows the original `kasf*` function of the
//! same name (the kas2c table names them): selections are the original grow-selections with
//! their quirks (removal swaps the last ship into the hole, `ShipsAdd` skips duplicates,
//! `growSelectAddShip` does not), positions of selections and teams are the centres of their
//! bounding boxes (`selCentrePointComputeGeneral`), team orders go through the command layer.
//! Functions without a game-side model yet (UI, sound, research, docking AI moves) record what
//! they were asked; everything not implemented at all falls through to
//! [`KasRuntime::unhandled`], which logs the call.

use hwc_kas::host::{Cx, KasHost, KasRuntime, Point, Ships, Str, Team, Value};
use hwc_kas::{FuncId, Hash};
use hwc_math::fpu::{fild_i32, ld, round_magic, st};
use hwc_math::Vector;

use crate::command;
use crate::level::{self, distance_squared, KasTeam};
use crate::mission::{self, Game, LogKind, Objective, HUMAN_PLAYER};
use crate::object::{sof, special, ShipId};
use crate::statics::{ShipRace, ShipType};
use crate::universe::Universe;

mod ai_ui;
mod moves;
mod orders;
mod select;

/// Flags that keep a ship out of `FindShipsInside` (`SOF_Hide | SOF_Dead`).
const HIDDEN_OR_DEAD: u32 = sof::HIDE | sof::DEAD;
/// `kasfSpawnEffect` (0x4932d0): the named effect on every ship of the selection, attached
/// to the ship in every way (the scale passed is 0, the ship's own is used), with the ship's
/// bounding radius and `param` (as a float) for parameters. No frequency limit applies. An
/// effect name that was never loaded starts nothing.
pub fn spawn_effect(u: &mut Universe, ships: &[ShipId], name: &str, param: i32) {
    let Some(stat) = u.effects.as_ref().and_then(|fx| fx.named(name)) else { return };
    let p = crate::effects::pf(st(fild_i32(param)));
    for &id in ships {
        let Some(size) = u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).map(|st_| st_.header.coll.coll_sphere_size) else { continue };
        crate::effects::create(u, stat, Some(crate::effects::FxOwner::Ship(id)), None, None, None, 0.0, crate::effects::attach::FULL, &[crate::effects::pf(size), p]);
    }
}

/// Marker for a ship hidden by KAS while it was already hidden (`HideShips` / `UnhideShips`).
const HIDDEN_TWICE: u32 = 0x0800_0000;
/// Special flags 2 (+0x1cc) bit: not under the player's control.
const NOT_PLAYER_CONTROLLED: u32 = 0x0010_0000;
/// Special flags (+0x1c8): launching, and about to launch.
const LAUNCHING: u32 = 0x0100_0000;
const LAUNCH_PENDING: u32 = 0x0080_0000;

/// The log lines of `SetCurrentMimicItem` and `SetMCVItemIndex`: `<prefix><item type>, <value>)`.
pub(crate) const MIMIC_ITEM_LOG: &str = "SetCurrentMimicItem(";
pub(crate) const MCV_INDEX_LOG: &str = "SetMCVItemIndex(";

/// Replays one log line onto the disguise menus (a saved game rebuilds them from its log).
pub(crate) fn replay_disguise_log(t: &mut crate::ships::mimic::DisguiseTables, line: &str) {
    let parse = |rest: &str| -> Option<(i32, i32)> {
        let (a, b) = rest.split_once(", ")?;
        let b = b.split(')').next()?;
        Some((a.parse().ok()?, b.parse().ok()?))
    };
    // Saves made before the menus were kept logged the slot instead (8 for the asteroids):
    // `SetCurrentMimicItem: disguise menu <slot> += "<name>" (<type>) ...` and
    // `SetMCVItemIndex: disguise menu <slot> default = <index> ...`.
    let old_item = |slot: i32| if slot == 8 { 2 } else { slot };
    if let Some((item, ty)) = line.strip_prefix(MIMIC_ITEM_LOG).and_then(parse) {
        ai_ui::add_mimic_item(t, item, ty);
    } else if let Some((item, index)) = line.strip_prefix(MCV_INDEX_LOG).and_then(parse) {
        ai_ui::set_mcv_item_index(t, item, index);
    } else if let Some(rest) = line.strip_prefix("SetCurrentMimicItem: disguise menu ") {
        let slot = rest.split(' ').next().and_then(|x| x.parse().ok());
        let ty = rest.rsplit_once(" (no disguise").and_then(|(a, _)| a.rsplit_once('(')).and_then(|(_, b)| b.trim_end_matches(')').parse().ok());
        if let (Some(slot), Some(ty)) = (slot, ty) {
            ai_ui::add_mimic_item(t, old_item(slot), ty);
        }
    } else if let Some(rest) = line.strip_prefix("SetMCVItemIndex: disguise menu ") {
        let mut w = rest.split(' ');
        let slot = w.next().and_then(|x| x.parse().ok());
        let index = w.nth(2).and_then(|x| x.parse().ok());
        if let (Some(slot), Some(index)) = (slot, index) {
            ai_ui::set_mcv_item_index(t, old_item(slot), index);
        }
    }
}

#[cfg(test)]
mod disguise_log_tests {
    use super::replay_disguise_log;
    use crate::ships::mimic::DisguiseTables;

    #[test]
    fn old_and_new_log_lines_rebuild_the_menus() {
        let mut a = DisguiseTables::default();
        for l in [
            "SetCurrentMimicItem(0, 4) \"sRecon\"",
            "SetMCVItemIndex(0, 1)",
            "SetCurrentMimicItem(2, -1) \"Asteroids\"",
        ] {
            replay_disguise_log(&mut a, l);
        }
        let mut b = DisguiseTables::default();
        for l in [
            "SetCurrentMimicItem: disguise menu 0 += \"sRecon\" (4) (no disguise tables)",
            "SetMCVItemIndex: disguise menu 0 default = 1 (no disguise tables)",
            "SetCurrentMimicItem: disguise menu 8 += \"Asteroids\" (-1) (no disguise tables)",
        ] {
            replay_disguise_log(&mut b, l);
        }
        assert_eq!(a, b);
        assert_eq!(a.entries[0].len(), 1);
        assert_eq!(a.defaults[0], 1);
        assert_eq!(a.entries[8].len(), 4);
    }
}

/// The game side of one KAS run.
pub struct SimHost<'a> {
    pub u: &'a mut Universe,
    pub g: &'a mut Game,
}

impl SimHost<'_> {
    fn log(&mut self, kind: LogKind, text: impl Into<String>) {
        self.g.log(kind, text);
    }

    /// Ships of a selection argument (a missing named selection reads as empty).
    fn ships(&self, s: Ships) -> Vec<ShipId> {
        match s {
            Ships::Null => Vec::new(),
            Ships::Selection(h) => self.g.selection(h).cloned().unwrap_or_default(),
            Ships::Team(h) => self.g.level.team(h).map(|t| t.ships.clone()).unwrap_or_default(),
        }
    }

    /// The list behind a selection argument, for writing (named selections are created on
    /// first use; a team's list is the team itself).
    fn ships_mut(&mut self, s: Ships) -> Option<&mut Vec<ShipId>> {
        match s {
            Ships::Null => None,
            Ships::Selection(h) => Some(self.g.selection_mut(h)),
            Ships::Team(h) => self.g.level.team_mut(h).map(|t| &mut t.ships),
        }
    }

    fn set_ships(&mut self, s: Ships, v: Vec<ShipId>) {
        if let Some(dst) = self.ships_mut(s) {
            *dst = v;
        }
    }

    fn team(&self, cx: &Cx) -> Option<&KasTeam> {
        cx.team.and_then(|h| self.g.level.team(h))
    }

    fn team_ships(&self, cx: &Cx) -> Vec<ShipId> {
        self.team(cx).map(|t| t.ships.clone()).unwrap_or_default()
    }

    /// The position of a selection entry (a ship, mine or meteor).
    fn pos(&self, id: ShipId) -> Option<Vector> {
        if let Some(m) = id.mine() {
            return self.u.missiles.get(m).map(|m| m.posinfo.position);
        }
        if let Some(r) = id.meteor() {
            return self.u.base.resources.get(r).map(|r| r.position);
        }
        self.u.ships.get(id).map(|s| s.posinfo.position)
    }

    /// The position and the hidden flag of a selection entry still in the universe.
    fn object_state(&self, id: ShipId) -> Option<(Vector, bool)> {
        if let Some(m) = id.mine() {
            return self.u.missiles.get(m).map(|m| (m.posinfo.position, m.flags & sof::HIDE != 0));
        }
        if let Some(r) = id.meteor() {
            return self.u.base.resources.get(r).map(|r| (r.position, r.flags & sof::HIDE != 0));
        }
        self.u.ships.get(id).map(|s| (s.posinfo.position, s.flags & sof::HIDE != 0))
    }

    /// `selCentrePointComputeGeneral`: the centre of the bounding box of the ships' positions.
    fn centre(&self, ships: &[ShipId]) -> Vector {
        let mut lo = [1e21f32; 3];
        let mut hi = [-1e21f32; 3];
        for p in ships.iter().filter_map(|&id| self.pos(id)) {
            for (k, c) in [p.x, p.y, p.z].into_iter().enumerate() {
                if c <= lo[k] {
                    lo[k] = c;
                }
                if hi[k] <= c {
                    hi[k] = c;
                }
            }
        }
        Vector::new(st((ld(hi[0]) + ld(lo[0])) * 0.5), st((ld(hi[1]) + ld(lo[1])) * 0.5), st((ld(hi[2]) + ld(lo[2])) * 0.5))
    }

    /// A position argument: labelled points, centres of selections, teams and volumes.
    fn point(&self, p: Point) -> Option<Vector> {
        match p {
            Point::Null => None,
            Point::Label(h) => self.g.level.point(h).map(|v| Vector::new(v[0], v[1], v[2])),
            Point::Ships(h) => {
                let s = self.g.selection(h).cloned().unwrap_or_default();
                Some(if s.is_empty() { Vector::ZERO } else { self.centre(&s) })
            }
            Point::Team(h) => {
                let s = self.g.level.team(h).map(|t| t.ships.clone()).unwrap_or_default();
                Some(if s.is_empty() { Vector::ZERO } else { self.centre(&s) })
            }
            Point::Volume(h) => self.g.level.volume(h).map(|v| v.center()),
        }
    }

    fn live(&self, id: ShipId) -> bool {
        self.u.ships.get(id).is_some()
    }

    /// The ship list in universe order (`universe.ShipList`).
    fn ship_list(&self) -> Vec<ShipId> {
        self.u.ship_list.clone()
    }

    /// `kasSelectionFilter`: the ships of `orig` passing `pred` into `dst` (or `orig` filtered
    /// in place when they are the same selection). Returns the count.
    fn filter(&mut self, dst: Ships, orig: Ships, pred: impl Fn(&Self, ShipId) -> bool) -> i32 {
        let src = self.ships(orig);
        if dst == orig {
            let mut v = src;
            let mut i = 0;
            while i < v.len() {
                if pred(self, v[i]) {
                    i += 1;
                } else {
                    let id = v[i];
                    mission::swap_remove_all(&mut v, id);
                }
            }
            let n = v.len() as i32;
            self.set_ships(dst, v);
            return n;
        }
        let v: Vec<ShipId> = src.into_iter().filter(|&id| pred(self, id)).collect();
        let n = v.len() as i32;
        self.set_ships(dst, v);
        n
    }

    /// `kasPredShipEnemy`: ship of the human player (enemy of the computer) when `friendly` is
    /// false, of anyone else otherwise; swarmers and the Nomad moon parts never count.
    fn pred_enemy(&self, id: ShipId, friendly: bool) -> bool {
        let Some(s) = self.u.ships.get(id) else { return false };
        if s.special_flags2 & 0x0200_0000 != 0 {
            return false;
        }
        if friendly {
            s.player != HUMAN_PLAYER && !matches!(s.ship_type, ShipType::sSwarmer | ShipType::dNomadMoon | ShipType::dNomadFieldEmitter)
        } else {
            // `kasPredShipEnemy`: the human player's ships the computer player (1) sees; a
            // disguised Mimic, cloaked ship or undetected Leech is left out. (The original also
            // drops Mimics disguised as player 4's ships, which a single-player game has none of.)
            s.player == HUMAN_PLAYER
                && crate::visibility::visible_to_player(self.u, id, 1)
                && !crate::ships::mimic::is_disguised_as_player(self.u, id, 4)
                && s.ship_type != ShipType::sSwarmer
        }
    }

    fn str_text(s: Str<'_>) -> String {
        match s {
            Str::Null => String::new(),
            Str::Int(v) => v.to_string(),
            Str::Text(t) | Str::Local { text: t, .. } => t.to_string(),
        }
    }

    /// `univRemoveShipFromOutside` + `dockPutShipInside` for each ship: the instant dock
    /// helpers and `ShipsCreateLaunch` put ships straight into the host's inside list.
    fn put_inside(&mut self, ships: &[ShipId], host: ShipId) {
        if self.u.ships.get(host).is_none() {
            return;
        }
        for &id in ships {
            if self.u.ships.get(id).is_none() {
                continue;
            }
            if self.u.ship_list.contains(&id) {
                crate::base::remove_ship_from_outside(self.u, id, true);
            }
            crate::dock::put_ship_inside(self.u, id, host);
        }
    }

    /// Marks ships as launched by KAS (`specialFlags |= 0x800000 | 0x1000000`); the dock code
    /// clears 0x1000000 once a ship has flown out of its launch cone.
    fn mark_launching(&mut self, ships: &[ShipId]) {
        for &id in ships {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.special_flags |= LAUNCH_PENDING | LAUNCHING;
            }
        }
    }

    /// `kasfTeamFinishedLaunching` / `kasfShipsFinishedLaunching` on a list: every ship is out
    /// of its launch cone and in the universe.
    fn finished_launching(&self, sel: &[ShipId]) -> bool {
        sel.iter().all(|&id| self.u.ships.get(id).is_some_and(|s| s.special_flags & LAUNCHING == 0) && self.u.ship_list.contains(&id))
    }
}

impl KasRuntime for SimHost<'_> {
    select::select_methods!();
    orders::orders_methods!();
    ai_ui::ai_ui_methods!();

    fn unhandled(&mut self, cx: &Cx, func: FuncId) -> i32 {
        let team = cx.team.map(|t| self.g.name(t)).unwrap_or_else(|| "-".into());
        self.log(LogKind::Unimplemented, format!("{} (team {team})", func.name()));
        0
    }

    // ---- orders ------------------------------------------------------------------------

    fn attack(&mut self, cx: &Cx, targets: Ships) {
        let t = self.ships(targets);
        if t.is_empty() {
            return;
        }
        self.delete_all_team_moves(cx);
        let sel = self.team_ships(cx);
        command::cl_attack(self.u, &sel, &t);
    }

    fn move_to(&mut self, cx: &Cx, destination: Point) {
        let Some(dest) = self.point(destination) else {
            self.log(LogKind::Warning, "MoveTo: NULL destination");
            return;
        };
        // kasfMoveTo (0x48f7c0): the team's moves are replaced by a move-team move (same
        // formation, no waiting, removed when done) that the next AI pass carries out.
        self.delete_all_team_moves(cx);
        self.add_team_move(cx, crate::aimove::TeamMove::move_team(dest, crate::aimove::SAME_FORMATION, false, true));
    }

    fn patrol_path(&mut self, cx: &Cx, path: Option<Hash>) {
        let Some(p) = path.and_then(|h| self.g.level.path(h)).cloned() else {
            self.log(LogKind::Warning, "PatrolPath: unknown path");
            return;
        };
        self.delete_all_team_moves(cx);
        let sel = self.team_ships(cx);
        if sel.is_empty() || p.points.is_empty() {
            return;
        }
        let from = self.centre(&sel);
        let mode = if p.closed { command::waypoint_mode::LOOP } else { command::waypoint_mode::PATROL };
        command::cl_move(self.u, &sel, mode, from, &p.points);
    }

    fn stop(&mut self, cx: &Cx) {
        self.delete_all_team_moves(cx);
        let sel = self.team_ships(cx);
        command::cl_halt(self.u, &sel);
    }

    /// `kasfGuardShips` (0x48df80): the team drops its AI moves and guards the ships
    /// (`clWrapProtect` with the team's own ship list, which loses its docking ships).
    fn guard(&mut self, cx: &Cx, ships: Ships) {
        let s = self.ships(ships);
        if s.is_empty() {
            return;
        }
        self.delete_all_team_moves(cx);
        if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
            let mut list = std::mem::take(&mut t.ships);
            command::make_ships_controllable(self.u, &mut list);
            if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
                t.ships = list.clone();
            }
            if !list.is_empty() {
                command::cl_wrap_protect(self.u, &list, &s);
            }
        }
    }

    fn formation_delta(&mut self, cx: &Cx) {
        self.team_formation(cx, crate::formation::kind::DELTA);
    }

    fn formation_claw(&mut self, cx: &Cx) {
        self.team_formation(cx, crate::formation::kind::CLAW);
    }

    fn formation_broad(&mut self, cx: &Cx) {
        self.team_formation(cx, crate::formation::kind::BROAD);
    }

    fn formation_delta3d(&mut self, cx: &Cx) {
        self.team_formation(cx, crate::formation::kind::DELTA3D);
    }

    fn formation_wall(&mut self, cx: &Cx) {
        self.team_formation(cx, crate::formation::kind::WALL);
    }

    /// `kasfDockInstant`: a dock move (stay, instant) with the team's first ship when it is fit
    /// for orders, otherwise with the nearest host.
    fn dock_instant(&mut self, cx: &Cx, with_team: Team) {
        self.team_dock(cx, with_team, crate::aimove::dock_flags::STAY | crate::aimove::dock_flags::INSTANT);
    }

    /// `kasfLaunch`: the team is marked as launching and gets a launch move.
    fn launch(&mut self, cx: &Cx) {
        let sel = self.team_ships(cx);
        self.mark_launching(&sel);
        self.delete_all_team_moves(cx);
        self.add_team_move(cx, crate::aimove::TeamMove::launch(false, true));
    }

    fn team_docked_ready_for_launch(&mut self, cx: &Cx) -> i32 {
        let sel = self.team_ships(cx);
        if sel.is_empty() {
            return 0;
        }
        // `kasfTeamDockedReadyForLaunch`: every ship is inside a host (hidden, not in
        // hyperspace) and not waiting for a soft launch from a carrier or mothership, or it
        // stays docked at a carrier or mothership's refuelling point.
        let soft_launch_host = |u: &Universe, h: Option<ShipId>| h.and_then(|h| u.ships.get(h)).is_some_and(|h| matches!(h.ship_type.index(), 0xc | 0x2f | 0x4c | 0x3e));
        for &id in &sel {
            let Some(s) = self.u.ships.get(id) else { return 0 };
            let refuelling = s.base.dock_with.is_some() && matches!(s.base.dock_state, 8 | 9);
            if s.flags & sof::HIDE != 0 && s.flags & sof::HYPERSPACE == 0 {
                if refuelling && soft_launch_host(self.u, s.base.dock_with) {
                    return 0;
                }
            } else if s.special_flags & crate::base::special::DOCK_STAY == 0 || !refuelling {
                return 0;
            }
        }
        1
    }

    fn team_finished_launching(&mut self, cx: &Cx) -> i32 {
        let sel = self.team_ships(cx);
        if sel.is_empty() {
            return 0;
        }
        i32::from(self.finished_launching(&sel))
    }

    /// `kasfShipsCreateLaunch` (0x495290): the ships are created, put inside the host's first
    /// ship and ordered to launch from it (`aiuWrapDock` purpose launch), so they come out
    /// through its launch points one after another. Ships for the human player take the host's
    /// "not player controlled" state (and disable reason 1 with it).
    fn ships_create_launch(&mut self, cx: &Cx, created_ships: Ships, shiptype: Str<'_>, count: i32, host: Ships, owner: i32) {
        let _ = cx;
        let Some(h) = self.ships(host).first().copied() else { return };
        self.set_ships(created_ships, Vec::new());
        let mut v = Vec::new();
        if let Some(t) = shiptype.text().and_then(ShipType::from_name) {
            let mission = self.g.config.mission;
            for _ in 0..count.max(0) {
                match level::create_ship(self.u, &mut self.g.level, mission, t, owner.clamp(0, 1) as usize, Vector::ZERO) {
                    Ok(id) => v.push(id),
                    Err(e) => self.g.log(LogKind::Warning, format!("ShipsCreateLaunch: {e}")),
                }
            }
        }
        self.set_ships(created_ships, v.clone());
        if owner == 0 {
            let not_controlled = self.u.ships.get(h).is_some_and(|x| x.special_flags2 & NOT_PLAYER_CONTROLLED != 0);
            for &id in &v {
                self.set_uncontrollable(id, not_controlled);
                if let Some(s) = self.u.ships.get_mut(id) {
                    crate::base::set_disable_reason(s, not_controlled, 1);
                }
            }
        }
        self.put_inside(&v, h);
        if self.u.ships.get(h).is_some_and(|x| matches!(x.ship_type.index(), 8 | 0x2a)) {
            crate::ships::hive::reset_swarmers(self.u, h);
        }
        self.mark_launching(&v);
        self.wrap_dock(&v, crate::dock::purpose::LAUNCH, Some(h));
    }

    fn reinforce_team_with_ships(&mut self, cx: &Cx, teamtoreinforce: Team, shipstoadd: Ships) {
        let _ = cx;
        let add = self.ships(shipstoadd);
        if let Some(t) = teamtoreinforce.and_then(|h| self.g.level.team_mut(h)) {
            t.ships.extend(add);
        }
    }

    fn team_switch_player_owner(&mut self, cx: &Cx, b_make_controllable: i32) {
        let sel = self.team_ships(cx);
        if sel.is_empty() && cx.team.is_none() {
            return;
        }
        command::remove_ships_from_doing_stuff(self.u, &sel);
        for &id in &sel {
            // The human player's ships stop being its fog-of-war viewers as they change hands;
            // ships coming over become viewers once they are out in the universe.
            if self.u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER) {
                crate::fow::remove_viewer(self.u, id);
            }
            let Some(s) = self.u.ships.get_mut(id) else { continue };
            if s.player == HUMAN_PLAYER {
                s.player = level::COMPUTER_PLAYER;
                s.attributes |= 0x100;
                s.flags &= !sof::SELECTABLE;
            } else {
                // `SelectionSwitchPlayerOwner` (0x48f5a0): without control the ship also gets
                // disable reason 1, so the player's orders leave it out.
                if b_make_controllable == 0 {
                    crate::base::set_disable_reason(s, true, 1);
                }
                s.attributes &= !0x100;
                s.player = HUMAN_PLAYER;
                s.flags = (s.flags & !sof::DISABLED) | sof::SELECTABLE;
                if b_make_controllable == 0 {
                    s.special_flags2 |= NOT_PLAYER_CONTROLLED;
                } else {
                    s.special_flags2 &= !NOT_PLAYER_CONTROLLED;
                }
                if self.u.ship_list.contains(&id) {
                    crate::fow::add_viewer(self.u, id);
                }
            }
        }
    }

    // ---- selections --------------------------------------------------------------------

    fn ships_clear(&mut self, _cx: &Cx, ships: Ships) {
        if let Some(v) = self.ships_mut(ships) {
            v.clear();
        }
    }

    fn ships_count(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        self.ships(ships).len() as i32
    }

    fn ships_add(&mut self, _cx: &Cx, ships_a: Ships, ships_b: Ships) -> i32 {
        if ships_a == Ships::Null || ships_b == Ships::Null {
            return 0;
        }
        let b = self.ships(ships_b);
        let Some(a) = self.ships_mut(ships_a) else { return 0 };
        if ships_a != ships_b {
            for id in b {
                if !a.contains(&id) {
                    a.push(id);
                }
            }
        }
        a.len() as i32
    }

    fn ships_remove(&mut self, _cx: &Cx, ships_a: Ships, ships_b: Ships) -> i32 {
        if ships_a == Ships::Null || ships_b == Ships::Null {
            return 0;
        }
        let b = self.ships(ships_b);
        let same = ships_a == ships_b;
        let Some(a) = self.ships_mut(ships_a) else { return 0 };
        if same {
            a.clear();
            return 0;
        }
        for id in b {
            mission::swap_remove_all(a, id);
        }
        a.len() as i32
    }

    fn first_ship(&mut self, _cx: &Cx, p_dst: Ships, p_src: Ships) {
        let first = self.ships(p_src).first().copied();
        self.set_ships(p_dst, first.into_iter().collect());
    }

    fn find_ships_inside(&mut self, _cx: &Cx, volume: Option<Hash>, ships: Ships) -> i32 {
        let Some(vol) = volume.and_then(|h| self.g.level.volume(h)).copied() else {
            self.log(LogKind::Warning, "FindShipsInside: unknown volume");
            return 0;
        };
        if ships == Ships::Null {
            return 0;
        }
        let v: Vec<ShipId> = self
            .ship_list()
            .into_iter()
            .filter(|&id| self.u.ships.get(id).is_some_and(|s| s.flags & HIDDEN_OR_DEAD == 0 && vol.contains(s.posinfo.position)))
            .collect();
        let n = v.len() as i32;
        self.set_ships(ships, v);
        n
    }

    fn find_ships_near_point(&mut self, _cx: &Cx, ships: Ships, location: Point, radius: i32) -> i32 {
        let Some(p) = self.point(location) else { return 0 };
        if ships == Ships::Null {
            return 0;
        }
        let r = ld(fild_i32(radius) as f32);
        let v: Vec<ShipId> = self
            .ship_list()
            .into_iter()
            .filter(|&id| {
                self.u.ships.get(id).is_some_and(|s| {
                    s.flags & (HIDDEN_OR_DEAD | sof::HYPERSPACE | sof::DISABLED | sof::CRAZY) == 0 && distance_squared(p, s.posinfo.position) <= r * r
                })
            })
            .collect();
        let n = v.len() as i32;
        self.set_ships(ships, v);
        n
    }

    fn find_enemies_nearby(&mut self, cx: &Cx, ships: Ships, radius: i32) -> i32 {
        let _ = cx;
        let cur = self.ships(ships);
        if ships == Ships::Null || cur.is_empty() {
            return 0;
        }
        // The reference ship: the first ship of the human player in the selection, else the
        // last one looked at.
        let mut reference = cur[0];
        for &id in &cur {
            reference = id;
            if self.u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER) {
                break;
            }
        }
        let rp = self.pos(reference).unwrap_or(Vector::ZERO);
        let r = ld(fild_i32(radius) as f32);
        let v: Vec<ShipId> = self
            .ship_list()
            .into_iter()
            .filter(|&id| {
                self.u.ships.get(id).is_some_and(|s| {
                    s.flags & (HIDDEN_OR_DEAD | sof::HYPERSPACE | sof::DISABLED | sof::CRAZY) == 0
                        && s.player == HUMAN_PLAYER
                        && (id == reference || distance_squared(rp, s.posinfo.position) <= r * r)
                })
            })
            .collect();
        let n = v.len() as i32;
        self.set_ships(ships, v);
        n
    }

    fn ships_select_enemy(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
        self.filter(new_ships, original_ships, |h, id| h.pred_enemy(id, false))
    }

    fn ships_select_friendly(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
        self.filter(new_ships, original_ships, |h, id| h.pred_enemy(id, true))
    }

    fn ships_select_player(&mut self, _cx: &Cx, new_ships: Ships, old_ships: Ships) -> i32 {
        self.filter(new_ships, old_ships, |h, id| h.u.ships.get(id).is_some_and(|s| s.special_flags2 & NOT_PLAYER_CONTROLLED == 0))
    }

    fn ships_select_race(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, ship_type: i32) -> i32 {
        self.filter(new_ships, original_ships, |h, id| h.u.ships.get(id).is_some_and(|s| s.ship_type.race().index() as i32 == ship_type))
    }

    fn ships_select_type(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, ship_type: Str<'_>) -> i32 {
        let t = ship_type.text().and_then(ShipType::from_name);
        self.filter(new_ships, original_ships, |h, id| h.u.ships.get(id).is_some_and(|s| Some(s.ship_type) == t))
    }

    fn ships_select_nearby(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, location: Point, index: i32) -> i32 {
        let p = self.point(location).unwrap_or(Vector::ZERO);
        let r = ld(fild_i32(index) as f32);
        let r2 = ld(st(r * r));
        self.filter(new_ships, original_ships, |h, id| h.pos(id).is_some_and(|q| distance_squared(q, p) < r2))
    }

    fn ships_sort(&mut self, _cx: &Cx, ships: Ships, origin: Point) {
        let Some(o) = self.point(origin) else { return };
        let mut v = self.ships(ships);
        let d = |h: &Self, id: ShipId| {
            let q = h.pos(id).unwrap_or(Vector::ZERO);
            let (x, y, z) = (ld(st(ld(o.x) - ld(q.x))), ld(st(ld(o.y) - ld(q.y))), ld(st(ld(o.z) - ld(q.z))));
            x * x + y * y + z * z
        };
        // kasfShipsSort: for each ship, compare it with every one before it and swap when nearer.
        for i in 1..v.len() {
            for j in 0..i {
                if d(self, v[i]) < d(self, v[j]) {
                    v.swap(i, j);
                }
            }
        }
        self.set_ships(ships, v);
    }

    fn get_player_mothership(&mut self, _cx: &Cx, ships: Ships) {
        let ms = self.g.level.motherships.get(HUMAN_PLAYER).copied().flatten();
        self.set_ships(ships, ms.into_iter().collect());
    }

    fn is_idle(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        let v = self.ships(ships);
        let idle = v.iter().any(|&id| {
            self.u.ships.get(id).is_some_and(|s| s.command.and_then(|c| self.u.commands.get(c)).is_none_or(|c| c.order == command::order::NULL))
        });
        i32::from(idle)
    }

    /// `kasfHasShipBeenDiscovered` (0x494b90): a ship of the selection that is not hidden lies in
    /// a cell of the fog grid the human player's ships see now (`fowPointGetViewerCount > 0`),
    /// the same test the sensors use to show it. Without a grid nothing is discovered.
    fn has_ship_been_discovered(&mut self, _cx: &Cx, ship: Ships) -> i32 {
        let v = self.ships(ship);
        let Some(grid) = self.u.fog.grids.get(HUMAN_PLAYER).and_then(Option::as_ref) else { return 0 };
        for id in v {
            let Some((pos, hidden)) = self.object_state(id) else { continue };
            if !hidden && grid.point_viewer_count(pos) > 0 {
                return 1;
            }
        }
        0
    }

    // ---- teams -------------------------------------------------------------------------

    fn team_count(&mut self, cx: &Cx) -> i32 {
        self.team(cx).map_or(0, |t| t.ships.len() as i32)
    }

    fn team_count_original(&mut self, cx: &Cx) -> i32 {
        self.team(cx).map_or(0, |t| t.orig_count)
    }

    fn team_health_average(&mut self, cx: &Cx) -> i32 {
        let sel = self.team_ships(cx);
        if sel.is_empty() {
            return 0;
        }
        let (mut max, mut cur) = (0.0f32, 0.0f32);
        for &id in &sel {
            let Some(s) = self.u.ships.get(id) else { continue };
            let m = self.u.statics.ship(s.ship_type).map_or(0.0, |st| st.health.max_health);
            max = st(ld(max) + ld(m));
            cur = st(ld(cur) + ld(s.health));
        }
        round_magic(ld(cur) * 100.0 / ld(max))
    }

    fn team_health_set(&mut self, cx: &Cx, percent_health: i32) {
        let sel = self.team_ships(cx);
        let statics = self.u.statics.clone();
        for id in sel {
            if let Some(s) = self.u.ships.get_mut(id) {
                let m = statics.ship(s.ship_type).map_or(0.0, |st| st.health.max_health);
                s.health = st(fild_i32(percent_health) * ld(m) * ld(0.01f32));
            }
        }
    }

    fn ships_damage_modifier_set(&mut self, _cx: &Cx, ships: Ships, damage_modifier: i32) {
        for id in self.ships(ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.damage_modifier = st(fild_i32(damage_modifier) * ld(0.01f32));
                s.special_flags |= special::KAS_DAMAGE_MODIFIER;
            }
        }
    }

    fn ships_damage_modifier_clear(&mut self, _cx: &Cx, ships: Ships) {
        for id in self.ships(ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.special_flags &= !special::KAS_DAMAGE_MODIFIER;
            }
        }
    }

    /// `kasfShipsSetRepairFactor` (0x48f030): the self-repair multiplier (+0x3dc) that
    /// `univShipRegenerateHealth` applies, and `SPECIAL_KasSpecialRepairModifier`.
    fn ships_repair_modifier_set(&mut self, _cx: &Cx, ships: Ships, repair_modifier: i32) {
        for id in self.ships(ships) {
            let m = st(fild_i32(repair_modifier) * ld(0.01f32));
            if let Some(s) = self.u.ships.get_mut(id) {
                s.base.health_mult = m;
                s.special_flags |= special::KAS_REPAIR_MODIFIER;
            }
            self.g.repair_modifiers.retain(|r| r.0 != id);
            self.g.repair_modifiers.push((id, m));
        }
    }

    /// `kasfShipsClearRepairFactor` (0x48f080) only clears the flag: the multiplier stays.
    fn ships_repair_modifier_clear(&mut self, _cx: &Cx, ships: Ships) {
        for id in self.ships(ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.special_flags &= !special::KAS_REPAIR_MODIFIER;
            }
        }
    }

    fn ships_set_invincible(&mut self, _cx: &Cx, ships: Ships, invincible: i32) {
        for id in self.ships(ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.flags2 = (s.flags2 & !4) | ((invincible as u32 & 1) * 4);
            }
        }
    }

    fn ships_set_color(&mut self, _cx: &Cx, ships: Ships, color: i32) {
        for id in self.ships(ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.color_scheme = color.clamp(0, 7) as usize;
            }
        }
    }

    fn hide_ships(&mut self, _cx: &Cx, shipstohide: Ships) {
        for id in self.ships(shipstohide) {
            if let Some(s) = self.u.ships.get_mut(id) {
                if s.flags & sof::HIDE == 0 {
                    s.flags |= sof::HIDE;
                } else {
                    s.flags |= HIDDEN_TWICE;
                }
            }
        }
    }

    fn unhide_ships(&mut self, _cx: &Cx, shipstounhide: Ships) {
        for id in self.ships(shipstounhide) {
            if let Some(s) = self.u.ships.get_mut(id) {
                if s.flags & HIDDEN_TWICE == 0 {
                    s.flags &= !sof::HIDE;
                } else {
                    s.flags &= !HIDDEN_TWICE;
                }
            }
        }
    }

    /// `kasfShipsUpgrade` (0x494290): `suShipApplyUpgrades` on every ship of the selection
    /// (Mission 2's Turanic corvettes take the disguise upgrade from `TECH_MIMIC` this way).
    fn upgrade_ships(&mut self, _cx: &Cx, ships: Ships) {
        for id in self.ships(ships) {
            if self.u.ships.get(id).is_some() {
                crate::upgrade::apply_upgrades(self.u, id);
            }
        }
    }

    fn delete_ships_when_mission_ends(&mut self, _cx: &Cx, shipstodelete: Ships) {
        for id in self.ships(shipstodelete) {
            if !self.g.delete_when_mission_ends.contains(&id) {
                self.g.delete_when_mission_ends.push(id);
            }
        }
    }

    // ---- game state --------------------------------------------------------------------

    // ---- mid-level hyperspace -----------------------------------------------------------

    fn team_hyperspace_out(&mut self, cx: &Cx) {
        if cx.team.is_none() {
            return;
        }
        let sel = crate::campaign::filter_disabled(self.u, &self.team_ships(cx));
        command::remove_ships_from_doing_stuff(self.u, &sel);
        crate::campaign::hyperspace_selection_out(self.u, self.g, &sel);
        self.g.hyper.kas_delay = 0.0;
    }

    fn ships_hyperspace_out(&mut self, _cx: &Cx, ships: Ships) {
        let v = self.ships(ships);
        if v.is_empty() {
            return;
        }
        let sel = crate::campaign::filter_disabled(self.u, &v);
        command::remove_ships_from_doing_stuff(self.u, &sel);
        crate::campaign::hyperspace_selection_out(self.u, self.g, &sel);
        self.g.hyper.kas_delay = 0.0;
    }

    fn team_hyperspace_in(&mut self, cx: &Cx, destination: Point) {
        if cx.team.is_none() {
            return;
        }
        let d = self.point4(destination).unwrap_or([0.0, 0.0, 0.0, 1.0]);
        let sel = self.team_ships(cx);
        crate::campaign::hyperspace_selection_in(self.u, self.g, &sel, d);
        self.g.hyper.kas_delay = 0.0;
    }

    fn team_hyperspace_in_near(&mut self, cx: &Cx, destination: Point, distance: i32) {
        if cx.team.is_none() {
            return;
        }
        let near = self.point(destination).unwrap_or(Vector::ZERO);
        let draws = [self.u.randy.random(6), self.u.randy.random(6), self.u.randy.random(6)];
        let p = orders::random_standoff(draws, near, st(fild_i32(distance)));
        let mut d = [p.x, p.y, p.z, 1.0];
        orders::clamp_to_world(&mut d, self.u.world_bounds);
        let sel = self.team_ships(cx);
        crate::campaign::hyperspace_selection_in(self.u, self.g, &sel, d);
        self.g.hyper.kas_delay = 0.0;
    }

    fn ships_hyperspace_in(&mut self, _cx: &Cx, ships: Ships, point: Point) {
        let v = self.ships(ships);
        if v.is_empty() {
            return;
        }
        let d = self.point4(point).unwrap_or([0.0, 0.0, 0.0, 1.0]);
        crate::campaign::hyperspace_selection_in(self.u, self.g, &v, d);
        self.g.hyper.kas_delay = 0.0;
    }

    fn ships_hyperspace_in_default(&mut self, _cx: &Cx, ships: Ships) {
        // Each ship comes back where it is (its position when it left).
        for id in self.ships(ships) {
            let Some(p) = self.pos(id) else { continue };
            crate::campaign::hyperspace_selection_in(self.u, self.g, &[id], [p.x, p.y, p.z, 1.0]);
            self.g.hyper.kas_delay = 0.0;
        }
    }

    fn ships_create_hyperspace(&mut self, cx: &Cx, created_ships: Ships, shiptype: Str<'_>, count: i32, dest: Point, owner: i32) {
        // kasfShipsCreateHyperspace: create, mark to start in hyperspace, send to hyperspace
        // (at once, because of the mark) and bring in at `dest`.
        self.ships_create(cx, created_ships, shiptype, count, owner);
        let v = self.ships(created_ships);
        for &id in &v {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.attributes |= crate::campaign::START_IN_HYPERSPACE;
            }
        }
        if !v.is_empty() {
            let sel = crate::campaign::filter_disabled(self.u, &v);
            command::remove_ships_from_doing_stuff(self.u, &sel);
            crate::campaign::hyperspace_selection_out(self.u, self.g, &sel);
            self.g.hyper.kas_delay = 0.0;
            let d = self.point4(dest).unwrap_or([0.0, 0.0, 0.0, 1.0]);
            crate::campaign::hyperspace_selection_in(self.u, self.g, &v, d);
            self.g.hyper.kas_delay = 0.0;
        }
        for &id in &v {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.attributes &= !crate::campaign::START_IN_HYPERSPACE;
            }
        }
    }

    fn hyperspace_delay(&mut self, _cx: &Cx, milliseconds: i32) {
        self.g.hyper.kas_delay = st(fild_i32(milliseconds) * 0.001);
    }

    fn start_level_by_hyperspacing_ships(&mut self, _cx: &Cx) {
        mission::begin_arrival(self.g);
    }

    fn mission_skill_get(&mut self, _cx: &Cx) -> i32 {
        self.g.config.skill
    }

    fn mission_failed(&mut self, _cx: &Cx) {
        self.g.mission_failed = true;
        self.log(LogKind::Flow, "MissionFailed");
    }

    /// `kasfMissionCompleted` (0x48d290): in the tutorial the tutorial is over; otherwise the
    /// game is won (`singlePlayerMissionCompleteCB`: the game ends, the closing movie plays).
    fn mission_completed(&mut self, _cx: &Cx) {
        self.g.mission_completed = true;
        self.log(LogKind::Flow, "MissionCompleted");
    }

    fn r_us_set(&mut self, _cx: &Cx, player: i32, r_us: i32) {
        if player >= 0 {
            let p = player as usize;
            if self.g.resource_units.len() <= p {
                self.g.resource_units.resize(p + 1, 0);
            }
            self.g.resource_units[p] = r_us;
            // kasfRUsSet writes the player's resourceUnits.
            if let Some(pl) = self.u.base.players.get_mut(p) {
                pl.resource_units = r_us;
            }
        }
    }

    fn r_us_get(&mut self, _cx: &Cx, player: i32) -> i32 {
        usize::try_from(player).ok().and_then(|p| self.u.base.players.get(p)).map_or(0, |p| p.resource_units)
    }

    fn get_world_resources(&mut self, _cx: &Cx) -> i32 {
        crate::resource::world_resources(self.u)
    }

    fn tech_set(&mut self, _cx: &Cx, tech_name: Str<'_>) {
        // PlayerAcquiredTechnology: the trader marks it known and the current player has it
        // (rmAddTechToPlayer: add, then suPlayerUpdateFromResearch).
        let t = Self::str_text(tech_name);
        if let Some(tech) = crate::statics::Technology::from_name(&t) {
            crate::fow::tech_add(self.u, HUMAN_PLAYER, tech.index());
            crate::upgrade::player_update_from_research(self.u, HUMAN_PLAYER);
        } else {
            self.log(LogKind::Warning, format!("TechSet: unknown technology {t:?}"));
        }
        self.g.techs.push(t);
    }

    /// `kasfAllowPlayerToResearch` (0x492310, `AllowPlayerToResearch`): the research manager
    /// offers the technology from now on ([`crate::research::available_topics`]).
    fn tech_set_research(&mut self, _cx: &Cx, tech_name: Str<'_>) {
        let t = Self::str_text(tech_name);
        if let Some(tech) = crate::statics::Technology::from_name(&t) {
            crate::research::allow_research(self.u, HUMAN_PLAYER, tech.index());
        }
        self.g.research_allowed.push(t);
    }

    fn enable_ship_construction(&mut self, _cx: &Cx, ship_type: Str<'_>, ship_race: Str<'_>, b_enabled: i32) {
        // kasfEnableShipConstruction: rmEnableShip(StrToShipRace, StrToShipType, enabled), then
        // the construction manager refreshes (it reads the tables when drawn).
        let (t, r) = (Self::str_text(ship_type), Self::str_text(ship_race));
        match (ShipType::from_name(&t), ShipRace::from_name(&r)) {
            (Some(ty), Some(race)) => level::enable_ship(self.u, race.index(), ty, b_enabled != 0),
            _ => self.log(LogKind::Warning, format!("EnableShipConstruction: unknown {t:?} / {r:?}")),
        }
        self.g.construction.push((t, r, b_enabled));
    }

    fn save_level(&mut self, _cx: &Cx, level_number: i32, level_name: Str<'_>) {
        let name = Self::str_text(level_name);
        self.log(LogKind::Flow, format!("SaveLevel({level_number}, {name:?}) requested"));
        self.g.pending_save = Some((level_number, name));
    }

    // ---- objectives --------------------------------------------------------------------

    fn objective_create(&mut self, _cx: &Cx, label: Str<'_>, brief_text: Str<'_>, full_text: Str<'_>) {
        let label = Self::str_text(label);
        let brief = Self::str_text(brief_text);
        let mut full = Self::str_text(full_text);
        if full.is_empty() {
            full = brief.clone();
        }
        // objectiveAndFleetIntelligenceCreate: a label that exists is not created again; the
        // "hyperspace" objective is what makes the hyperspace button work (0xaaf94c).
        if self.g.objectives.iter().any(|o| o.label.eq_ignore_ascii_case(&label)) {
            return;
        }
        if label.eq_ignore_ascii_case("hyperspace") {
            self.g.hyper.player_can_hyperspace = true;
        }
        self.log(LogKind::Objective, format!("ObjectiveCreate {label:?}: {brief}"));
        self.g.objectives.push(Objective { label, brief, full, primary: true, complete: false });
    }

    fn objective_destroy(&mut self, _cx: &Cx, label: Str<'_>) {
        let label = Self::str_text(label);
        let before = self.g.objectives.len();
        self.g.objectives.retain(|o| !o.label.eq_ignore_ascii_case(&label));
        if self.g.objectives.len() != before {
            self.log(LogKind::Objective, format!("ObjectiveDestroy {label:?}"));
        }
    }

    fn objectives_destroy_all(&mut self, _cx: &Cx) {
        self.g.objectives.clear();
        self.log(LogKind::Objective, "ObjectivesDestroyAll");
    }

    // ---- presentation (recorded) -------------------------------------------------------

    fn speech_event(&mut self, _cx: &Cx, event: i32, variable: i32) {
        self.g.intel_ended = false;
        self.g.speech_end = self.g.config.speech_seconds.map(|s| st(ld(self.u.total_time_elapsed) + ld(s)));
        let time = self.u.total_time_elapsed;
        self.g.presentation.push(crate::mission::Presentation::Speech { event, variable, time });
        if !self.g.config.log_calls {
            self.log(LogKind::Flow, format!("SpeechEvent {event:#x}"));
        }
    }

    fn intel_event_not_ended(&mut self, _cx: &Cx) {
        self.g.intel_ended = false;
    }

    /// `kasfIntelEventEnded`: the end the front end reported (`subMessageEnded`, cleared by
    /// asking), else `SUBMESSAGE_SAFETY_TIMEOUT` after the first ask once no subtitle is on
    /// screen.
    fn intel_event_ended(&mut self, _cx: &Cx) -> i32 {
        use crate::mission::{IntelEnd, IntelEndReason};
        let now = self.u.total_time_elapsed;
        let mut reason = IntelEndReason::Reported;
        if let Some(end) = self.g.speech_end {
            if end < now {
                self.g.speech_end = None;
                self.g.intel_ended = true;
                reason = IntelEndReason::SpeechTimer;
            }
        }
        let ended = std::mem::take(&mut self.g.intel_ended);
        if ended {
            self.g.intel_stamp = 0.0;
            self.g.intel_ends.push(IntelEnd { time: now, reason });
            return 1;
        }
        if self.g.intel_stamp == 0.0 {
            self.g.intel_stamp = now;
            return 0;
        }
        if ld(self.g.sp_tweaks.submessage_safety_timeout) < ld(now) - ld(self.g.intel_stamp)
            && !self.g.subtitles_on_screen
        {
            self.g.intel_stamp = 0.0;
            self.g.intel_ends.push(IntelEnd { time: now, reason: IntelEndReason::Timeout });
            return 1;
        }
        0
    }

    fn location_card(&mut self, _cx: &Cx, milliseconds: i32, location: Str<'_>) {
        let time = self.u.total_time_elapsed;
        self.g.presentation.push(crate::mission::Presentation::LocationCard { text: location.text().unwrap_or("").to_string(), milliseconds, time });
        self.g.intel_ended = false;
    }

    // ---- special abilities -------------------------------------------------------------

    /// `kasfSpecialToggle`: the team uses its special.
    fn special_toggle(&mut self, cx: &Cx) {
        let sel = self.team_ships(cx);
        if !sel.is_empty() {
            crate::ships::cl_special(self.u, &sel, &[]);
        }
    }

    /// `kasfShipsSpecialToggle`.
    fn ships_special_toggle(&mut self, _cx: &Cx, ships: Ships) {
        let sel = self.ships(ships);
        if !sel.is_empty() {
            crate::ships::cl_special(self.u, &sel, &[]);
        }
    }

    /// `kasfSpecialOn`: the team uses its special on the ships.
    fn special_on(&mut self, cx: &Cx, ships: Ships) -> i32 {
        let sel = self.team_ships(cx);
        let t = self.ships(ships);
        if !sel.is_empty() && ships != Ships::Null {
            crate::ships::cl_special(self.u, &sel, &t);
        }
        0
    }

    /// `kasfSpecialOff` (does nothing in Cataclysm).
    fn special_off(&mut self, _cx: &Cx) -> i32 {
        0
    }

    /// `kasfShipsSpecialOn`.
    fn ships_special_on(&mut self, _cx: &Cx, src_ships: Ships, dest_ships: Ships) -> i32 {
        let sel = self.ships(src_ships);
        let t = self.ships(dest_ships);
        if src_ships != Ships::Null && dest_ships != Ships::Null {
            crate::ships::cl_special(self.u, &sel, &t);
        }
        0
    }

    /// `kasfShipsSpecialOff`: the special toggled off (clWrapSpecial without targets).
    fn ships_special_off(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        let sel = self.ships(ships);
        if ships != Ships::Null {
            crate::ships::cl_special(self.u, &sel, &[]);
        }
        0
    }

    /// `kasfMimic` (0x4935f0): disguise as a ship type, or as asteroid `A<n>`.
    fn mimic(&mut self, _cx: &Cx, ships: Ships, ship_name: Str<'_>, player_index: i32) {
        let name = ship_name.text().unwrap_or("");
        let o = if let Some(n) = name.strip_prefix(['A', 'a']) {
            let k = n.bytes().next().map_or(-0x30, |c| i32::from(c) - 0x30);
            crate::ships::mimic::DisguiseOrder { kind: 2, ship_type: 0, player: player_index, asteroid: k }
        } else {
            let t = ShipType::from_name(name).map_or(-1, |t| t.index() as i32);
            crate::ships::mimic::DisguiseOrder { kind: 1, ship_type: t, player: player_index, asteroid: 0 }
        };
        for id in self.ships(ships) {
            if self.u.ships.get(id).is_some() {
                crate::ships::mimic::do_disguise_command(self.u, id, o);
            }
        }
    }

    /// `kasfUnmimic` (0x4936a0).
    fn unmimic(&mut self, _cx: &Cx, ships: Ships) {
        for id in self.ships(ships) {
            if self.u.ships.get(id).is_some() {
                crate::ships::mimic::end_disguise(self.u, id, true);
            }
        }
    }

    /// `kasfSetStayMimicked` (0x495e70): keep the disguise when docking.
    fn set_stay_mimicked(&mut self, _cx: &Cx, p_ships: Ships, b_stay_mimicked: i32) {
        for id in self.ships(p_ships) {
            if let Some(s) = self.u.ships.get_mut(id) {
                s.special_flags2 = (s.special_flags2 & !0x0800_0000) | ((b_stay_mimicked as u32 & 1) << 27);
            }
        }
    }

    /// `kasfShipsBeastify` (0x493d10): infect the ships (by the originator's first ship); the
    /// others are scuttled (self-destruct timers are not simulated).
    fn ships_beastify(&mut self, _cx: &Cx, ships: Ships, originator: Ships) {
        let sel = self.ships(ships);
        let Some(&by) = self.ships(originator).first() else { return };
        let rest = crate::ships::beast::kas_ships_beastify(self.u, &sel, by);
        if !rest.is_empty() {
            self.log(LogKind::Flow, format!("ShipsBeastify: {} ships scuttled (not simulated)", rest.len()));
        }
    }

    /// `kasfShipsSetBeastInfectionUnlimited`.
    fn ships_set_beast_infection_unlimited(&mut self, _cx: &Cx, ships: Ships, enable: i32) {
        let sel = self.ships(ships);
        crate::ships::beast::kas_set_infection_unlimited(self.u, &sel, enable != 0);
    }

    /// `kasfShipsSelectBeastified`: the newly beastified ships of the original selection.
    fn ships_select_beastified(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
        let sel = self.ships(original_ships);
        let out = crate::ships::beast::kas_select_beastified(self.u, &sel);
        let n = out.len() as i32;
        self.set_ships(new_ships, out);
        n
    }

    /// `kasfGetShipCountBeastified`.
    fn get_ship_count_beastified(&mut self, _cx: &Cx, ship_type: Str<'_>, player: i32) -> i32 {
        let t = ship_type.text().and_then(ShipType::from_name);
        crate::ships::beast::ships_beastified(self.u, t, player.max(0) as usize)
    }

    /// `kasfShipInfectionTerminal`.
    fn ship_infection_terminal(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        let sel = self.ships(ships);
        i32::from(crate::ships::beast::kas_infection_terminal(self.u, &sel))
    }

    /// `kasfEnableBeastWeaponAutoAttack`.
    fn enable_beast_weapon_auto_attack(&mut self, _cx: &Cx, p_beast_ships: Ships, b_enable: i32) {
        let sel = self.ships(p_beast_ships);
        crate::ships::beast::kas_enable_weapon_auto_attack(self.u, &sel, b_enable != 0);
    }

    /// `kasfIsBeastReady`.
    fn is_beast_ready(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        let sel = self.ships(ships);
        i32::from(crate::ships::beast::kas_is_beast_ready(self.u, &sel))
    }

    /// `kasfTimesHitByEMP`: EMP hits taken by the ships.
    fn times_hit_by_emp(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        self.ships(ships).iter().filter_map(|&id| self.u.ships.get(id)).map(|s| s.disable.emp_count).sum()
    }

    /// `kasfIsForceFieldActive`: some sentinel's field is up.
    fn is_force_field_active(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        i32::from(self.ships(ships).iter().any(|&id| crate::ships::sentinel::field_is_up(self.u, id)))
    }

    /// `kasfIsEMPReady`: some ACV's EMP is charged.
    fn is_emp_ready(&mut self, _cx: &Cx, ships: Ships) -> i32 {
        i32::from(self.ships(ships).iter().any(|&id| 1.0 <= crate::ships::acv::emp_charge_fraction(self.u, id)))
    }

    /// `kasfSalvage` (0x492d60 area): the team's salvage-capable ships salvage the targets.
    fn salvage(&mut self, cx: &Cx, ships: Ships) {
        let sel: Vec<ShipId> = self.team_ships(cx).into_iter().filter(|&id| self.u.ships.get(id).is_some_and(|s| crate::salvage::is_salvager(s.ship_type))).collect();
        let t = self.ships(ships);
        if sel.is_empty() {
            return;
        }
        crate::ships::cl_special(self.u, &sel, &t);
    }

    fn spawn_effect(&mut self, _cx: &Cx, ships: Ships, effect: Str<'_>, effect_param: i32) {
        let list = self.ships(ships);
        spawn_effect(self.u, &list, &Self::str_text(effect), effect_param);
    }

    fn music_play(&mut self, _cx: &Cx, track_num: i32) {
        self.g.music = Some(track_num);
        let time = self.u.total_time_elapsed;
        self.g.presentation.push(crate::mission::Presentation::Music { track: track_num, time });
    }

    fn open_sensors(&mut self, _cx: &Cx, flags: i32) {
        self.g.sensors_kas_flag = if flags < 2 { flags } else { 1 };
        self.g.sensors_open = true;
    }

    fn close_sensors(&mut self, _cx: &Cx, _flags: i32) {
        self.g.sensors_open = false;
        self.g.sensors_kas_flag = 0;
    }

    fn sensors_is_open(&mut self, _cx: &Cx, flags: i32) -> i32 {
        if flags != 0 {
            return self.g.sensors_kas_flag;
        }
        i32::from(self.g.sensors_open && self.g.sensors_kas_flag == 0)
    }

    fn set_sensors_look_at_point(&mut self, _cx: &Cx, point: Point) {
        if self.g.sensors_open {
            if let Some(p) = self.point(point) {
                self.g.sensors_look_at = p;
            }
        }
    }

    fn pause_universe(&mut self, _cx: &Cx) {
        self.u.universe_paused = true;
    }

    fn unpause_universe(&mut self, _cx: &Cx) {
        self.u.universe_paused = false;
    }

    fn wide_screen_in_dont_close_sensors(&mut self, _cx: &Cx, frames: i32) {
        self.g.wide_screen_call(self.u, crate::mission::WideScreenKind::InDontCloseSensors, frames);
    }

    fn wide_screen_out(&mut self, _cx: &Cx, frames: i32) {
        self.g.wide_screen_call(self.u, crate::mission::WideScreenKind::Out, frames);
    }

    /// `kasfCameraFocus` (0x491e90): nothing when the camera already focuses on these ships;
    /// otherwise a clean main screen and `ccFocusScripted` (a cut to them). An empty selection
    /// focuses the player's.
    fn camera_focus(&mut self, _cx: &Cx, ships: Ships) {
        let v = self.ships(ships);
        if !v.is_empty() {
            self.main_screen();
        }
        self.g.camera.focus = v.clone();
        self.g.camera.scripted = true;
        self.g.camera_orders.push(crate::mission::CameraOrder::Focus { ships: v, close: false, cut: true, unless_same: true });
    }

    /// `kasfCameraGetAngleDeg` (0x4917b0): the remembered camera's angle in whole degrees
    /// (truncated, 0..360).
    fn camera_get_angle_deg(&mut self, _cx: &Cx) -> i32 {
        self.g.camera.angle as i32
    }

    /// `kasfCameraGetDeclinationDeg` (0x491810): -90 (above) to +90 (below), truncated.
    fn camera_get_declination_deg(&mut self, _cx: &Cx) -> i32 {
        self.g.camera.declination as i32
    }

    /// `kasfCameraGetDistance` (0x491870), truncated.
    fn camera_get_distance(&mut self, _cx: &Cx) -> i32 {
        self.g.camera.distance as i32
    }

    /// `kasfCameraSetAngleDeg` (0x4917d0): `ccChangeAngleDeclination` with the current
    /// declination, then `cameraSetEyePosition` on the actual camera (no blend).
    fn camera_set_angle_deg(&mut self, _cx: &Cx, angle: i32) {
        self.g.camera.angle = angle as f32;
        self.camera_angles();
    }

    /// `kasfCameraSetDeclinationDeg` (0x491830): as `CameraSetAngleDeg`, for the declination.
    fn camera_set_declination_deg(&mut self, _cx: &Cx, declination: i32) {
        self.g.camera.declination = declination as f32;
        self.camera_angles();
    }

    /// `kasfCameraSetDistance` (0x491880): `ccSetDistance` + `cameraSetEyePosition`.
    fn camera_set_distance(&mut self, _cx: &Cx, distance: i32) {
        self.g.camera.distance = distance as f32;
        self.g.camera_orders.push(crate::mission::CameraOrder::Distance { distance: distance as f32, scripted: false });
    }

    /// `kasfTutCameraFocusCancel` (0x492100): `ccCancelFocus`, back to the previous focus.
    fn tut_camera_focus_cancel(&mut self, _cx: &Cx) {
        self.g.camera.scripted = false;
        self.g.camera.focus.clear();
        self.g.camera_orders.push(crate::mission::CameraOrder::CancelFocus);
    }

    fn tut_enable_flags(&mut self, _cx: &Cx, flags_list: Str<'_>) {
        let f = Self::str_text(flags_list);
        self.g.tut_flags.push(f);
    }

    fn tut_remove_all_pointers(&mut self, _cx: &Cx) {
        self.g.pointers.clear();
    }

    fn tut_remove_pointer(&mut self, _cx: &Cx, name: Str<'_>) {
        let n = Self::str_text(name);
        self.g.pointers.retain(|p| !p.eq_ignore_ascii_case(&n));
    }

    fn tut_set_pointer_target_ship_selection(&mut self, _cx: &Cx, name: Str<'_>, p_ship_list: Ships) {
        if !self.ships(p_ship_list).is_empty() {
            let n = Self::str_text(name);
            self.g.pointers.push(n);
        }
    }

    fn tut_set_obj_pointer_target_ship_selection(&mut self, _cx: &Cx, name: Str<'_>, p_ship_list: Ships) {
        if !self.ships(p_ship_list).is_empty() {
            let n = Self::str_text(name);
            self.g.pointers.push(n);
        }
    }

    fn tut_set_pointer_target_ai_volume(&mut self, _cx: &Cx, name: Str<'_>, _volume: Option<Hash>) {
        let n = Self::str_text(name);
        self.g.pointers.push(n);
    }

    fn tut_set_obj_pointer_target_ai_volume(&mut self, _cx: &Cx, name: Str<'_>, _volume: Option<Hash>) {
        let n = Self::str_text(name);
        self.g.pointers.push(n);
    }

    fn ping_add_ships(&mut self, _cx: &Cx, ships: Ships, label: Str<'_>) {
        let v = self.ships(ships);
        if !v.is_empty() {
            let n = Self::str_text(label);
            self.g.pings.push(n.clone());
            self.g.ping_orders.push(mission::PingOrder::Ships { label: n, ships: v });
        }
    }

    fn ping_add_point(&mut self, _cx: &Cx, vector: Point, label: Str<'_>) {
        let n = Self::str_text(label);
        self.g.pings.push(n.clone());
        if let Some(at) = self.point(vector) {
            self.g.ping_orders.push(mission::PingOrder::Point { label: n, at });
        }
    }

    fn ping_remove(&mut self, _cx: &Cx, label: Str<'_>) {
        let n = Self::str_text(label);
        self.g.pings.retain(|p| !p.eq_ignore_ascii_case(&n));
        self.g.ping_orders.push(mission::PingOrder::Remove { label: n });
    }
}

impl SimHost<'_> {
    /// `aiuKasWrapFormation` for the current team, remembered as the team's KAS formation.
    fn team_formation(&mut self, cx: &Cx, kind: u32) {
        let sel = self.team_ships(cx);
        if !sel.is_empty() {
            command::cl_formation(self.u, &sel, kind);
        }
        if let Some(t) = cx.team.and_then(|h| self.g.level.team_mut(h)) {
            t.kas_formation = kind as i32;
        }
    }
}

impl KasHost for SimHost<'_> {
    fn universe_time(&mut self) -> f32 {
        self.u.total_time_elapsed
    }

    fn random(&mut self, stream: u32) -> u32 {
        self.u.randy.random(stream as usize)
    }

    fn trace_call(&mut self, cx: &Cx, func: FuncId, args: &[Value<'_>], result: i32) {
        if !self.g.config.log_calls {
            return;
        }
        let team = cx.team.map(|t| self.g.name(t)).unwrap_or_else(|| "mission".into());
        let args: Vec<String> = args
            .iter()
            .map(|a| match *a {
                Value::Ships(Ships::Selection(h)) => format!("SHIPS_{}", self.g.name(h)),
                Value::Ships(Ships::Team(h)) | Value::Team(Some(h)) => format!("TEAM_{}", self.g.name(h)),
                Value::Volume(Some(h)) => format!("VOLUME_{}", self.g.name(h)),
                Value::Path(Some(h)) => format!("PATH_{}", self.g.name(h)),
                Value::Hash(h) => self.g.name(h),
                _ => a.to_string(),
            })
            .collect();
        self.g.log(LogKind::Call, format!("{team}: {}({}) = {result}", func.name(), args.join(", ")));
    }

    fn fsm_created(&mut self, team: Hash, fsm: Hash) {
        let t = self.g.level.team_or_create(team);
        t.kas_tactics = 1;
        t.kas_formation = 7;
        let (tn, fname) = (self.g.name(team), self.g.name(fsm));
        self.g.log(LogKind::State, format!("FSMCreate {fname} for team {tn}"));
    }

    fn state_changed(&mut self, team: Hash, state: Hash) {
        let (tn, sn) = (self.g.name(team), self.g.name(state));
        self.g.log(LogKind::State, format!("team {tn} -> state {sn}"));
    }

    fn warning(&mut self, _cx: &Cx, message: &str) {
        self.g.log(LogKind::Warning, message.to_string());
    }
}
