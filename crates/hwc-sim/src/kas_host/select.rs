//! KAS runtime functions on the simulation: see the parent module.
//!
//! The `kasf*` functions of this group are written as methods of the `KasRuntime` impl in
//! the macro below, which the parent invokes inside that impl.
//!
//! This group holds the selection filters (`ShipsSelect*`, each a `kasSelectionFilter` with
//! one predicate) and the queries over ships, teams, commands, mines and the build queue.
//! Where the simulation lacks the original's data (protect lists of guard orders, mines and
//! resources in ship selections) the functions return what the original would with that
//! data absent and say so in a `LogKind::Flow` entry.

#![allow(unused_imports)]

use hwc_kas::host::{Cx, Ships};
use hwc_math::fpu::{fild_i32, fild_u32, ftol_i32, ld, lt_or_unordered, round_magic, st};
use hwc_math::Vector;

use crate::command::{attr, order, CommandToDo};
use crate::level::distance_squared;
use crate::mission::{self, LogKind, HUMAN_PLAYER};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipType};

use super::SimHost;

/// `SOF_Hide | SOF_Dead`.
pub(super) const HIDDEN_OR_DEAD_FLAGS: u32 = sof::HIDE | sof::DEAD;
/// `SOF_Hyperspace | SOF_Disabled | SOF_Crazy`: ships the "find enemies" scans skip.
pub(super) const AWAY_FLAGS: u32 = sof::HYPERSPACE | sof::DISABLED | sof::CRAZY;
/// Special flags 2 (+0x1cc) bit of ships the "enemy" predicates never count.
const NOT_AN_ENEMY: u32 = 0x0200_0000;
/// Special flags 2 (+0x1cc) bit 3: being built.
pub(super) const UNDER_CONSTRUCTION_FLAG: u32 = 0x0000_0008;
/// Static capability bit (+0x1ec bit 14) of the slipgates.
pub(super) const SLIPGATE_CAPABILITY: u32 = 0x4000;
/// Resource attribute (+0x38) of the asteroids of a meteor storm.
pub(super) const METEOR_ATTRIBUTE: u32 = 0x200;
/// `ranRandomFn` stream of `kasfShipsShuffle` (`RAN_AIPlayer`).
pub(super) const RAN_SHUFFLE: usize = hwc_math::random::RAN_AI_PLAYER;
/// `kasfGetBuildTime`: the build time (in frames) times this is seconds.
const FRAMES_TO_SECONDS: f32 = 0.0625;
/// `kasfTeamHealthLowest` starts above any percentage.
pub(super) const HEALTH_LOWEST: i32 = 110;

/// `kasPredShipMoving`: some velocity component exceeds 2 in magnitude.
pub(super) fn is_moving(v: Vector) -> bool {
    [v.x, v.y, v.z].into_iter().any(|c| !(c.abs() <= 2.0))
}

/// `kasPredShipHealthAtMost`: `health <= pct * maxhealth * 0.01` (the percentage loaded as
/// an unsigned 64-bit integer; false when unordered).
pub(super) fn health_at_most(health: f32, max_health: f32, pct: i32) -> bool {
    let limit = fild_u32(pct as u32) * ld(max_health) * ld(0.01f32);
    !lt_or_unordered(limit, ld(health))
}

/// `kasPredShipHealthAtLeast`: `pct * maxhealth * 0.01 <= health` (true when unordered).
pub(super) fn health_at_least(health: f32, max_health: f32, pct: i32) -> bool {
    let limit = fild_u32(pct as u32) * ld(max_health) * ld(0.01f32);
    let h = ld(health);
    limit <= h || limit.is_nan() || h.is_nan()
}

/// Health percentage of `kasfTeamHealthLowest`: `health * 100 / maxhealth`, rounded by the
/// magic-number trick.
pub(super) fn health_percent(health: f32, max_health: f32) -> i32 {
    round_magic(ld(health) * ld(100.0f32) / ld(max_health))
}

/// `kasfGetBuildTime`: `(int)(buildTime * 0.0625)`, truncated.
pub(super) fn build_time_seconds(build_time: i32) -> i32 {
    ftol_i32(fild_i32(build_time) * ld(FRAMES_TO_SECONDS))
}

/// A radius squared as the "find near" functions keep it: the integer squared on the FPU,
/// stored as a float.
pub(super) fn radius_squared(radius: i32) -> f64 {
    let r = fild_i32(radius);
    ld(st(r * r))
}

/// `kasPredShipEnemyNoStealthy` with argument 0 (the only one `ShipsSelectEnemyNoStealthy`
/// passes): a ship of the human player, without the not-an-enemy flag, that is no swarmer.
/// (Despite the name, nothing here tests cloaking or visibility.)
pub(super) fn enemy_no_stealthy(player: usize, special_flags2: u32, t: ShipType) -> bool {
    player == HUMAN_PLAYER && special_flags2 & NOT_AN_ENEMY == 0 && t != ShipType::sSwarmer
}

/// `kasfShipsShuffle`: every position swaps with the one at `random % count`.
pub(super) fn shuffle(v: &mut [ShipId], mut random: impl FnMut() -> u32) {
    let n = v.len() as u32;
    for i in 0..v.len() {
        let j = (random() % n) as usize;
        v.swap(i, j);
    }
}

/// The protect list of a guard order (`command+0x2b4`), empty unless it is protecting.
pub(super) fn protect_list_of(c: &CommandToDo) -> &[ShipId] {
    if c.attributes & crate::command::attr::PROTECTING != 0 {
        &c.protect
    } else {
        &[]
    }
}

impl SimHost<'_> {
    /// `kasSelectionFilter` with a predicate over a second selection `other`, which may be
    /// the destination or the original selection itself: the predicate sees that list as the
    /// filter leaves it at that moment (the ships added so far, or the list being filtered
    /// in place).
    pub(super) fn filter_with(&mut self, dst: Ships, orig: Ships, other: Ships, pred: impl Fn(&Self, ShipId, &[ShipId]) -> bool) -> i32 {
        let src = self.ships(orig);
        let other_v = self.ships(other);
        if dst == orig {
            let mut v = src;
            let mut i = 0;
            while i < v.len() {
                let id = v[i];
                let pass = if other == dst { pred(self, id, &v) } else { pred(self, id, &other_v) };
                if pass {
                    i += 1;
                } else {
                    mission::swap_remove_all(&mut v, id);
                }
            }
            let n = v.len() as i32;
            self.set_ships(dst, v);
            return n;
        }
        let mut out = Vec::new();
        for &id in &src {
            let pass = if other == dst { pred(self, id, &out) } else { pred(self, id, &other_v) };
            if pass {
                out.push(id);
            }
        }
        let n = out.len() as i32;
        self.set_ships(dst, out);
        n
    }

    /// The ships inside `host` (`ship+0x4b4`): the dock code's list, then the ships KAS put
    /// inside it with the instant dock helpers.
    pub(super) fn inside_of(&self, host: ShipId) -> Vec<ShipId> {
        let mut v: Vec<ShipId> = self.u.ships.get(host).and_then(|h| h.base.inside.as_ref()).map(|i| i.ships.clone()).unwrap_or_default();
        for &(s, h) in &self.g.docked {
            if h == host && !v.contains(&s) {
                v.push(s);
            }
        }
        v
    }

    /// `salvDockedShipIterate` from the start: the ships clamped to `host` that are salvagers
    /// (workers and salvage corvettes), in clamp order.
    pub(super) fn clamped_salvagers(&self, host: ShipId) -> Vec<ShipId> {
        crate::clamp::slaves_of(self.u, host).into_iter().filter(|&id| self.u.ships.get(id).is_some_and(|s| crate::salvage::is_salvager(s.ship_type))).collect()
    }

    /// `kasfUnderAttackElsewhere` on a list: into `attackers` (cleared first), the ships of
    /// every attacking command (attack order, passive attack or attack-and-move) whose
    /// targets include one of `ships`, in command order without duplicates.
    pub(super) fn attacked_by(&mut self, ships: Ships, attackers: Ships) -> i32 {
        if attackers == Ships::Null {
            return 0;
        }
        self.set_ships(attackers, Vec::new());
        // Read after the clear: the two may be the same list.
        let sel = self.ships(ships);
        if sel.is_empty() {
            return 0;
        }
        let mut out: Vec<ShipId> = Vec::new();
        for c in self.u.commands.iter() {
            let attacking = c.order == order::ATTACK || c.attributes & (attr::PASSIVE_ATTACKING | attr::ATTACKING_AND_MOVING) != 0;
            if attacking && c.attack_targets.iter().any(|t| sel.contains(t)) {
                for &s in &c.selection {
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
        }
        let n = out.len() as i32;
        self.set_ships(attackers, out);
        n
    }

    /// The mines (`OBJ_MissileType` objects of type `MISSILE_Mine`) in missile-list order
    /// passing `pred` on their position.
    pub(super) fn mines_where(&self, pred: impl Fn(Vector) -> bool) -> Vec<ShipId> {
        self.u
            .missile_list
            .iter()
            .filter_map(|&m| self.u.missiles.get(m).map(|x| (m, x)))
            .filter(|(_, m)| m.kind == crate::missile::kind::MINE && pred(m.posinfo.position))
            .filter_map(|(id, _)| ShipId::from_mine(id))
            .collect()
    }
}

macro_rules! select_methods {
    () => {
        /// `kasfShipsSelectClass`: ships whose static class is the named one.
        fn ships_select_class(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, ship_class: Str<'_>) -> i32 {
            let class = ship_class.text().and_then(crate::statics::ShipClass::from_name);
            self.filter(new_ships, original_ships, |h, id| {
                class.is_some() && h.u.ships.get(id).and_then(|s| h.u.statics.ship(s.ship_type)).is_some_and(|st_| st_.ship_class == class)
            })
        }

        /// `kasfShipsSelectCapital`: ships whose statics say capital ship.
        fn ships_select_capital(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
            self.filter(new_ships, original_ships, |h, id| h.u.ships.get(id).and_then(|s| h.u.statics.ship(s.ship_type)).is_some_and(|st_| st_.is_capital_ship))
        }

        /// `kasfShipsSelectIndex`: the ship at `index` alone; an index out of range returns 0
        /// and leaves the destination as it was.
        fn ships_select_index(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, index: i32) -> i32 {
            if new_ships == Ships::Null || original_ships == Ships::Null {
                return 0;
            }
            let src = self.ships(original_ships);
            if index < 0 || index as usize >= src.len() {
                return 0;
            }
            let id = src[index as usize];
            self.set_ships(new_ships, vec![id]);
            1
        }

        /// `kasfShipsSelectDamaged`: health at most `max_health_percent` of the maximum.
        fn ships_select_damaged(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, max_health_percent: i32) -> i32 {
            self.filter(new_ships, original_ships, |h, id| {
                h.u.ships.get(id).and_then(|s| h.u.statics.ship(s.ship_type).map(|st_| (s.health, st_.health.max_health))).is_some_and(|(hp, m)| select::health_at_most(hp, m, max_health_percent))
            })
        }

        /// `kasfShipsSelectMoreThanDamaged`: health at least `max_health_percent` of the maximum.
        fn ships_select_more_than_damaged(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships, max_health_percent: i32) -> i32 {
            self.filter(new_ships, original_ships, |h, id| {
                h.u.ships.get(id).and_then(|s| h.u.statics.ship(s.ship_type).map(|st_| (s.health, st_.health.max_health))).is_some_and(|(hp, m)| select::health_at_least(hp, m, max_health_percent))
            })
        }

        /// `kasfShipsSelectMoving`: some velocity component above 2.
        fn ships_select_moving(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
            self.filter(new_ships, original_ships, |h, id| h.u.ships.get(id).is_some_and(|s| select::is_moving(s.posinfo.velocity)))
        }

        /// `kasfShipsSelectNumber`: the first `number` ships (compared unsigned, so a negative
        /// number keeps them all).
        fn ships_select_number(&mut self, _cx: &Cx, new_ships: Ships, old_ships: Ships, number: i32) -> i32 {
            let counter = std::cell::Cell::new(0u32);
            self.filter(new_ships, old_ships, |_, _| {
                let k = counter.get();
                counter.set(k.wrapping_add(1));
                k < number as u32
            })
        }

        /// `kasfShipsSelectShips`: the ships that are also in `compareships`.
        fn ships_select_ships(&mut self, _cx: &Cx, newships: Ships, oldships: Ships, compareships: Ships) -> i32 {
            self.filter_with(newships, oldships, compareships, |_, id, list| list.contains(&id))
        }

        /// `kasfShipsSelectClamped`: the ships that are salvagers clamped to the first ship
        /// of `target`; when that ship has none (or `target` is empty) the destination is
        /// cleared and 0 returned.
        fn ships_select_clamped(&mut self, _cx: &Cx, new_ships: Ships, old_ships: Ships, target: Ships) -> i32 {
            let host = self.ships(target).first().copied();
            let clamped = host.map(|h| self.clamped_salvagers(h)).unwrap_or_default();
            if clamped.is_empty() {
                self.set_ships(new_ships, Vec::new());
                return 0;
            }
            self.filter(new_ships, old_ships, |_, id| clamped.contains(&id))
        }

        /// `kasfShipsSelectGuarding`: ships whose order protects one of `guarding`.
        fn ships_select_guarding(&mut self, _cx: &Cx, new_ships: Ships, old_ships: Ships, guarding: Ships) -> i32 {
            self.filter_with(new_ships, old_ships, guarding, |h, id, list| {
                h.u.ships.get(id).and_then(|s| s.command).and_then(|c| h.u.commands.get(c)).is_some_and(|c| {
                    let p = select::protect_list_of(c);
                    list.iter().any(|g| p.contains(g))
                })
            })
        }

        /// `kasfShipsSelectSalvagable`: ships that some ship of `targets` could salvage
        /// (`salvIsSalvageable` with that ship's player).
        fn ships_select_salvagable(&mut self, _cx: &Cx, new_ships: Ships, old_ships: Ships, targets: Ships) -> i32 {
            self.filter_with(new_ships, old_ships, targets, |h, id, list| {
                list.iter().any(|&s| h.u.ships.get(s).is_some_and(|sv| crate::salvage::can_salvage_target(h.u, sv.player, id)))
            })
        }

        /// `kasfShipsSelectEnemyNoStealthy`.
        fn ships_select_enemy_no_stealthy(&mut self, _cx: &Cx, new_ships: Ships, original_ships: Ships) -> i32 {
            self.filter(new_ships, original_ships, |h, id| h.u.ships.get(id).is_some_and(|s| select::enemy_no_stealthy(s.player, s.special_flags2, s.ship_type)))
        }

        /// `kasfShipsInsideMe`: the ships inside any ship of `me`, without duplicates.
        fn ships_inside_me(&mut self, _cx: &Cx, inside_me: Ships, me: Ships) -> i32 {
            self.set_ships(inside_me, Vec::new());
            let mut out = Vec::new();
            for host in self.ships(me) {
                for s in self.inside_of(host) {
                    if !out.contains(&s) {
                        out.push(s);
                    }
                }
            }
            let n = out.len() as i32;
            self.set_ships(inside_me, out);
            n
        }

        /// `kasfShipUnderConstruction`: the first ship's under-construction bit.
        fn ship_under_construction(&mut self, _cx: &Cx, ships: Ships) -> i32 {
            let first = self.ships(ships).first().copied();
            first.and_then(|id| self.u.ships.get(id)).map_or(0, |s| i32::from(s.special_flags2 & select::UNDER_CONSTRUCTION_FLAG != 0))
        }

        /// `kasfBuildManagerShipTypeInBuildQueue` (`tutBuildManagerShipTypeInBuildQueue`): the
        /// queued count of the first job of that type in the current player's factories (0
        /// when no factory has a job of that type).
        fn build_manager_ship_type_in_build_queue(&mut self, _cx: &Cx, ship_type: Str<'_>) -> i32 {
            let Some(t) = ship_type.text().and_then(ShipType::from_name) else { return 0 };
            for f in &self.u.base.factories {
                let Some(owner) = f.ship.and_then(|s| self.u.ships.get(s)).map(|s| s.player) else { continue };
                if owner != HUMAN_PLAYER {
                    continue;
                }
                if let Some(job) = f.jobs.iter().find(|j| j.info == Some(t)) {
                    return job.n_jobs;
                }
            }
            0
        }

        /// `kasfFindEnemiesNearTeam`: the human player's ships near the team's first ship.
        fn find_enemies_near_team(&mut self, cx: &Cx, ships: Ships, radius: i32) -> i32 {
            if ships == Ships::Null {
                return 0;
            }
            self.set_ships(ships, Vec::new());
            let Some(reference) = self.team_ships(cx).first().copied() else { return 0 };
            let rp = self.pos(reference).unwrap_or(Vector::ZERO);
            let r2 = select::radius_squared(radius);
            let v: Vec<ShipId> = self
                .ship_list()
                .into_iter()
                .filter(|&id| {
                    self.u.ships.get(id).is_some_and(|s| {
                        s.flags & (select::HIDDEN_OR_DEAD_FLAGS | select::AWAY_FLAGS) == 0
                            && s.player == HUMAN_PLAYER
                            && (id == reference || distance_squared(rp, s.posinfo.position) <= r2)
                    })
                })
                .collect();
            let n = v.len() as i32;
            self.set_ships(ships, v);
            n
        }

        /// `kasfFindEnemyShipsInsideWithoutStealthy`: the human player's ships in the volume
        /// that the computer player can see.
        fn find_enemy_ships_inside_without_stealthy(&mut self, _cx: &Cx, volume: Option<Hash>, ships: Ships) -> i32 {
            if ships == Ships::Null {
                return 0;
            }
            let Some(vol) = volume.and_then(|h| self.g.level.volume(h)).copied() else { return 0 };
            let v: Vec<ShipId> = self
                .ship_list()
                .into_iter()
                .filter(|&id| {
                    self.u.ships.get(id).is_some_and(|s| {
                        s.flags & (select::HIDDEN_OR_DEAD_FLAGS | select::AWAY_FLAGS) == 0
                            && s.player == HUMAN_PLAYER
                            && crate::visibility::visible_to_player(self.u, id, 1)
                            && vol.contains(s.posinfo.position)
                    })
                })
                .collect();
            let n = v.len() as i32;
            self.set_ships(ships, v);
            n
        }

        /// `kasfFindMinesInside` (0x48e750): the mines in the volume, in missile-list order.
        fn find_mines_inside(&mut self, _cx: &Cx, volume: Option<Hash>, ships: Ships) -> i32 {
            if ships == Ships::Null {
                return 0;
            }
            self.set_ships(ships, Vec::new());
            let Some(vol) = volume.and_then(|h| self.g.level.volume(h)).copied() else { return 0 };
            let v = self.mines_where(|p| vol.contains(p));
            let n = v.len() as i32;
            self.set_ships(ships, v);
            n
        }

        /// `kasfFindMinesNearTeam`: the mines near the team's first ship (an empty team
        /// returns 0 and leaves the selection alone).
        fn find_mines_near_team(&mut self, cx: &Cx, ships: Ships, radius: i32) -> i32 {
            if ships == Ships::Null {
                return 0;
            }
            let Some(reference) = self.team_ships(cx).first().copied() else { return 0 };
            self.set_ships(ships, Vec::new());
            let rp = self.pos(reference).unwrap_or(Vector::ZERO);
            let r2 = select::radius_squared(radius);
            let v = self.mines_where(|p| distance_squared(rp, p) <= r2);
            let n = v.len() as i32;
            self.set_ships(ships, v);
            n
        }

        /// `kasfFindMeteors` (0x48e380): the asteroids of meteor storms (attribute 0x200), in
        /// resource-list order.
        fn find_meteors(&mut self, _cx: &Cx, meteors: Ships) {
            self.set_ships(meteors, Vec::new());
            let v: Vec<ShipId> = self
                .u
                .base
                .resources
                .list
                .iter()
                .filter(|r| matches!(r.kind, crate::resource::ResourceKind::Asteroid(_)) && r.attributes & select::METEOR_ATTRIBUTE != 0)
                .filter_map(|r| ShipId::from_meteor(r.id))
                .collect();
            self.set_ships(meteors, v);
        }

        /// `kasfFindSlipgate`: the visible ships in the volume, then those that are not
        /// slipgates removed (swap removal).
        fn find_slipgate(&mut self, _cx: &Cx, volume: Option<Hash>, slipgates: Ships) {
            if slipgates == Ships::Null {
                return;
            }
            let vol = volume.and_then(|h| self.g.level.volume(h)).copied();
            let mut v: Vec<ShipId> = self
                .ship_list()
                .into_iter()
                .filter(|&id| self.u.ships.get(id).is_some_and(|s| s.flags & select::HIDDEN_OR_DEAD_FLAGS == 0 && vol.is_some_and(|vol| vol.contains(s.posinfo.position))))
                .collect();
            let mut i = 0;
            while i < v.len() {
                let gate = self.u.ships.get(v[i]).and_then(|s| self.u.statics.ship(s.ship_type)).is_some_and(|st_| st_.capability_flags & select::SLIPGATE_CAPABILITY != 0);
                if gate {
                    i += 1;
                } else {
                    v.swap_remove(i);
                }
            }
            self.set_ships(slipgates, v);
        }

        /// `kasfNearby`: some ship of the team within `distance` of the point.
        fn nearby(&mut self, cx: &Cx, location: Point, distance: i32) -> i32 {
            let p = self.point(location).unwrap_or(Vector::ZERO);
            let r2 = select::radius_squared(distance);
            let near = self.team_ships(cx).into_iter().any(|id| distance_squared(self.pos(id).unwrap_or(Vector::ZERO), p) <= r2);
            i32::from(near)
        }

        /// `kasfFindDistance`: the game's square root of the distance squared, truncated.
        fn find_distance(&mut self, _cx: &Cx, location1: Point, location2: Point) -> i32 {
            let a = self.point(location1).unwrap_or(Vector::ZERO);
            let b = self.point(location2).unwrap_or(Vector::ZERO);
            hwc_math::fpu::ftol_i32(hwc_math::fastmath::fmath_sqrt_double(distance_squared(a, b)))
        }

        /// `kasfUnderAttack`: `UnderAttackElsewhere` for the current team.
        fn under_attack(&mut self, cx: &Cx, attackers: Ships) -> i32 {
            match cx.team {
                Some(t) => self.under_attack_elsewhere(cx, Some(t), attackers),
                None => 0,
            }
        }

        /// `kasfUnderAttackElsewhere`: the ships of every attacking command (attack order,
        /// passive attack or attack-and-move) whose targets include a ship of the team.
        fn under_attack_elsewhere(&mut self, _cx: &Cx, other_team: Team, attackers: Ships) -> i32 {
            let Some(team) = other_team else { return 0 };
            self.attacked_by(Ships::Team(team), attackers)
        }

        /// `kasfShipsUnderAttackElsewhere`: as `UnderAttackElsewhere`, for a selection.
        fn ships_under_attack_elsewhere(&mut self, _cx: &Cx, ships: Ships, attackers: Ships) -> i32 {
            self.attacked_by(ships, attackers)
        }

        /// `kasfTeamHealthLowest`: the lowest health percentage of the team's ships (0 for
        /// no team or an empty one; 110 if no percentage is lower).
        fn team_health_lowest(&mut self, cx: &Cx) -> i32 {
            if self.team(cx).is_none() {
                return 0;
            }
            let sel = self.team_ships(cx);
            if sel.is_empty() {
                return 0;
            }
            let mut lowest = select::HEALTH_LOWEST;
            for id in sel {
                let Some(s) = self.u.ships.get(id) else { continue };
                let m = self.u.statics.ship(s.ship_type).map_or(0.0, |st_| st_.health.max_health);
                let p = select::health_percent(s.health, m);
                if p < lowest {
                    lowest = p;
                }
            }
            lowest
        }

        /// `kasfThisTeamIdle` (0x494ee0): `aitTeamIsIdle` on the current team (no current AI
        /// move, or a done / get-ships move).
        fn this_team_idle(&mut self, cx: &Cx) -> i32 {
            i32::from(self.team(cx).is_none_or(|t| t.moves.is_idle()))
        }

        /// `kasfIsTeamIdle` (0x494ed0): `aitTeamIsIdle` on the team.
        fn is_team_idle(&mut self, _cx: &Cx, team: Team) -> i32 {
            i32::from(team.and_then(|h| self.g.level.team(h)).is_none_or(|t| t.moves.is_idle()))
        }

        /// `kasfIsGuarding`: into `guarded`, the ships protected by guard orders of `ships`.
        fn is_guarding(&mut self, _cx: &Cx, ships: Ships, guarded: Ships) -> i32 {
            if guarded == Ships::Null {
                return 0;
            }
            self.set_ships(guarded, Vec::new());
            let sel = self.ships(ships);
            if sel.is_empty() {
                return 0;
            }
            let mut out = Vec::new();
            for c in self.u.commands.iter() {
                let p = select::protect_list_of(c);
                if c.attributes & crate::command::attr::PROTECTING != 0 && !p.is_empty() && c.selection.iter().any(|s| sel.contains(s)) {
                    for &g in p {
                        if !out.contains(&g) {
                            out.push(g);
                        }
                    }
                }
            }
            let n = out.len() as i32;
            self.set_ships(guarded, out);
            n
        }

        /// `kasfGetTargets`: the targets of the special orders of `ships`, without duplicates.
        fn get_targets(&mut self, _cx: &Cx, targets: Ships, ships: Ships) -> i32 {
            self.set_ships(targets, Vec::new());
            let mut out = Vec::new();
            for id in self.ships(ships) {
                let Some(c) = self.u.ships.get(id).and_then(|s| s.command).and_then(|c| self.u.commands.get(c)) else { continue };
                if c.order == command::order::SPECIAL {
                    for &t in &c.special_targets {
                        if !out.contains(&t) {
                            out.push(t);
                        }
                    }
                }
            }
            let n = out.len() as i32;
            self.set_ships(targets, out);
            n
        }

        /// `kasfGetDockHost`: the ships of `possible_host` with a ship of `ships` inside.
        fn get_dock_host(&mut self, _cx: &Cx, ships: Ships, host: Ships, possible_host: Ships) {
            self.set_ships(host, Vec::new());
            let sel = self.ships(ships);
            let mut out = Vec::new();
            for h in self.ships(possible_host) {
                let inside = self.inside_of(h);
                for s in &sel {
                    if inside.contains(s) && !out.contains(&h) {
                        out.push(h);
                    }
                }
            }
            self.set_ships(host, out);
        }

        /// `kasfGetShipsClampedTo`: everything clamped to the hosts (duplicates kept).
        fn get_ships_clamped_to(&mut self, _cx: &Cx, p_ships_attached: Ships, p_host_ships: Ships) -> i32 {
            if p_ships_attached == Ships::Null || p_host_ships == Ships::Null {
                self.log(LogKind::Warning, "GetShipsClampedTo: got a null selection");
                return 0;
            }
            self.set_ships(p_ships_attached, Vec::new());
            let mut out = Vec::new();
            for h in self.ships(p_host_ships) {
                out.extend(crate::clamp::slaves_of(self.u, h));
            }
            let n = out.len() as i32;
            self.set_ships(p_ships_attached, out);
            n
        }

        /// `kasfGetBuildCost`: the type's build cost (0 for an unknown or unloaded type).
        fn get_build_cost(&mut self, _cx: &Cx, ship_type: Str<'_>) -> i32 {
            ship_type.text().and_then(ShipType::from_name).and_then(|t| self.u.statics.ship(t)).map_or(0, |s| s.build_cost)
        }

        /// `kasfGetBuildTime`: the type's build time in seconds (frames / 16, truncated).
        fn get_build_time(&mut self, _cx: &Cx, ship_type: Str<'_>) -> i32 {
            ship_type.text().and_then(ShipType::from_name).and_then(|t| self.u.statics.ship(t)).map_or(0, |s| select::build_time_seconds(s.build_time))
        }

        /// `TechGet` (`kasfDoesPlayerHave`): the trader's state of the technology is "have"
        /// (set by `TechSet` and by research completed by the human player in single
        /// player); read from the human player's technologies.
        fn tech_get(&mut self, _cx: &Cx, tech_name: Str<'_>) -> i32 {
            let t = Self::str_text(tech_name);
            let Some(tech) = crate::statics::Technology::from_name(&t) else { return 0 };
            i32::from(self.u.base.players.get(HUMAN_PLAYER).is_some_and(|p| p.tech.contains(tech.index())))
        }

        /// `kasfGetShipCountSalvaged`: ships of the type salvaged by the player (all types
        /// for an unknown name, the original's -1).
        fn get_ship_count_salvaged(&mut self, _cx: &Cx, ship_type: Str<'_>, player: i32) -> i32 {
            let stats = &self.u.base.stats;
            let p = player.max(0) as usize;
            match ship_type.text().and_then(ShipType::from_name) {
                Some(t) => stats.ships_salvaged(t, p),
                None => (0..crate::base::NUM_SHIP_TYPES).map(|t| stats.salvaged.get(p * crate::base::NUM_SHIP_TYPES + t).copied().unwrap_or(0)).sum(),
            }
        }

        /// `kasfShipsOrderAttributes`: the attributes of the ships' commands, or-ed.
        fn ships_order_attributes(&mut self, _cx: &Cx, ships: Ships) -> i32 {
            let mut a = 0u16;
            for id in self.ships(ships) {
                if let Some(c) = self.u.ships.get(id).and_then(|s| s.command).and_then(|c| self.u.commands.get(c)) {
                    a |= c.attributes;
                }
            }
            i32::from(a)
        }

        /// `kasfShipsShuffle`: swaps every position with a random one (`RAN_AIPlayer`).
        fn ships_shuffle(&mut self, _cx: &Cx, p_ships: Ships) {
            let mut v = self.ships(p_ships);
            if v.is_empty() {
                return;
            }
            let randy = &mut self.u.randy;
            select::shuffle(&mut v, || randy.random(select::RAN_SHUFFLE));
            self.set_ships(p_ships, v);
        }
    };
}
pub(super) use select_methods;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salvager_and_enemy_type_indices() {
        // The types `salvDockedShipIterate` accepts and the ones the enemy predicates exclude.
        for i in [0x0e, 0x31, 0x48, 0x59, 0x61] {
            assert!(crate::salvage::is_salvager(ShipType::from_index(i).unwrap()), "{i:#x}");
        }
        assert_eq!(ShipType::from_index(9), Some(ShipType::sSwarmer));
        assert_eq!(ShipType::from_index(0x7e), Some(ShipType::dCrystal0));
        assert_eq!(ShipType::from_index(0x7f), Some(ShipType::dCrystal1));
    }

    #[test]
    fn health_predicates() {
        // 0.01f is a little below 0.01: exactly 50% is not "at most 50%".
        assert!(!health_at_most(50.0, 100.0, 50));
        assert!(health_at_most(49.99, 100.0, 50));
        assert!(!health_at_most(50.5, 100.0, 50));
        assert!(health_at_least(50.0, 100.0, 50));
        assert!(!health_at_least(49.5, 100.0, 50));
        // A negative percentage is loaded as a large unsigned number.
        assert!(health_at_most(1e9, 100.0, -1));
        assert!(!health_at_most(f32::NAN, 100.0, 10));
        assert!(health_at_least(f32::NAN, 100.0, 10));
    }

    #[test]
    fn moving_and_percent() {
        assert!(!is_moving(Vector::new(2.0, -2.0, 0.0)));
        assert!(is_moving(Vector::new(0.0, -2.5, 0.0)));
        assert_eq!(health_percent(33.5, 100.0), 34);
        assert_eq!(health_percent(32.5, 100.0), 32); // round half to even
        assert_eq!(build_time_seconds(100), 6);
        assert_eq!(build_time_seconds(-17), -1);
        assert_eq!(radius_squared(3), 9.0);
        assert_eq!(radius_squared(16777217), ld(st(16777217.0f64 * 16777217.0)));
    }

    #[test]
    fn enemy_no_stealthy_rules() {
        assert!(enemy_no_stealthy(HUMAN_PLAYER, 0, ShipType::sRecon));
        assert!(!enemy_no_stealthy(1, 0, ShipType::sRecon));
        assert!(!enemy_no_stealthy(HUMAN_PLAYER, 0x0200_0000, ShipType::sRecon));
        assert!(!enemy_no_stealthy(HUMAN_PLAYER, 0, ShipType::sSwarmer));
    }

    #[test]
    fn shuffle_swaps_each_position_with_random_modulo_count() {
        let mut v: Vec<ShipId> = (0..4).map(ShipId).collect();
        let mut seq = [5u32, 2, 7, 0].into_iter();
        shuffle(&mut v, || seq.next().unwrap());
        // i=0 j=1: 1 0 2 3; i=1 j=2: 1 2 0 3; i=2 j=3: 1 2 3 0; i=3 j=0: 0 2 3 1.
        assert_eq!(v, [0, 2, 3, 1].map(ShipId));
    }
}
