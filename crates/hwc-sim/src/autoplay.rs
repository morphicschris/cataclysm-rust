//! Scripted test orders that play the campaign headless: a stand-in for the player that reads
//! the mission's objectives and gives the orders a player would to complete them (build ships
//! and modules, harvest, research, explore, attack, guard, repair, salvage), presses the
//! hyperspace button when the mission offers the jump and answers the roll call. When a
//! mission cannot be finished within the time budget the jump is forced with the original's
//! debug override (the C key, `hyperspaceOverride`), which [`Ending::Forced`] reports.
//! Used by the campaign example, the chain test and the viewer.
//!
//! Nothing here is part of the original game; the orders only use what a player can do (the
//! command layer, the build manager, the research manager), and the per-mission plans only use
//! what the mission tells the player (objectives and what their texts point at).

use hwc_kas::kas_hash;
use hwc_math::Vector;

use crate::mission::{sp_state, MissionRun, HUMAN_PLAYER};
use crate::object::{sof, ShipId};
use crate::statics::{ShipClass, ShipType};
use crate::universe::Universe;

/// How a mission ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Autoplay {
    /// Updates after the fleet arrived before the jump is forced.
    pub budget: u32,
    /// Give orders (otherwise only the hyperspace button is pressed).
    pub orders: bool,
    arrived_at: Option<u32>,
    rollcall_at: Option<u32>,
    pub forced: bool,
    pub pressed: bool,
    last_orders: u32,
    explore: usize,
    /// What the plan did, for logs and the viewer (`(update, text)`).
    pub notes: Vec<(u32, String)>,
    /// Per-mission plan marks (what was done once).
    marks: Vec<String>,
    /// Mission 3: since when the Clee-San has hung at the mouth of its dock.
    m3_stuck_since: Option<u32>,
    /// Since when the jump waits for tilted capital ships to right themselves.
    tilt_since: Option<u32>,
}

/// How often the orders are reviewed (every 4 s).
const ORDER_PERIOD: u32 = 64;

impl Autoplay {
    pub fn new(budget: u32) -> Self {
        Autoplay {
            budget,
            orders: true,
            arrived_at: None,
            rollcall_at: None,
            forced: false,
            pressed: false,
            last_orders: 0,
            explore: 0,
            notes: Vec::new(),
            marks: Vec::new(),
            m3_stuck_since: None,
            tilt_since: None,
        }
    }

    fn note(&mut self, now: u32, text: impl Into<String>) {
        self.notes.push((now, text.into()));
    }

    /// Records `mark` once; true the first time.
    fn once(&mut self, mark: &str) -> bool {
        if self.marks.iter().any(|m| m == mark) {
            return false;
        }
        self.marks.push(mark.to_string());
        true
    }

    /// The orders for this update (call before [`MissionRun::step`]).
    pub fn tick(&mut self, run: &mut MissionRun) {
        let now = run.universe.update_counter;
        if let Some(t) = std::env::var_os("HWC_WATCH_TEAM") {
            if now.is_multiple_of(16) || std::env::var_os("HWC_WATCH_EVERY").is_some() {
                let t = t.to_string_lossy().to_string();
                let v: Vec<String> = run.game.level.team(kas_hash(&t)).map(|x| x.ships.clone()).unwrap_or_default().iter().filter_map(|&id| {
                    let s = run.universe.ships.get(id)?;
                    let att = s.recent_attacker.and_then(|a| run.universe.ships.get(a)).map(|a| format!("{:?}/p{}", a.ship_type, a.player));
                    Some(format!("{:?} {} dock {:?}/{}/{} hp {:.0} p{} att {:?} flags {:x} flags2 {:x} pos {:?} vel {:?} cmd {:?} tac {} dead {}", s.ship_type, id.0, s.base.dock_with, s.base.dock_state, s.base.dock_point, s.health, s.player, att, s.flags, s.flags2, s.posinfo.position, s.posinfo.velocity, s.command.and_then(|c| run.universe.commands.get(c)).map(|c| (c.order, c.attributes, c.selection.len(), c.waypoints.clone())), s.tactics, run.universe.ship_list.contains(&id)))
                }).collect();
                let ids = run.game.level.team(kas_hash(&t)).map(|x| x.ships.clone()).unwrap_or_default();
                let homing: Vec<String> = run.universe.missiles.iter().filter(|m| m.target.is_some_and(|x| ids.contains(&x))).map(|m| {
                    let tp = run.universe.ships.ship(m.target.unwrap()).posinfo.position;
                    let owner = m.owner.and_then(|o| run.universe.ships.get(o)).map(|o| {
                        let c = o.command.and_then(|c| run.universe.commands.get(c));
                        (o.id, o.ship_type, o.tactics, o.attack.target, c.map(|c| (c.order, c.attributes, c.attack_targets.clone())))
                    });
                    format!("kind {} p{:?} owner {:?} d {:.0} life {:.1}", m.kind, m.player, owner, (m.posinfo.position - tp).mag_sq().sqrt(), m.time_lived)
                }).collect();
                self.note(now, format!("watch {t}: {v:?} homing {homing:?}"));
            }
        }
        if let Some(list) = std::env::var_os("HWC_WATCH_SHIPS") {
            let list = list.to_string_lossy().to_string();
            for id in list.split(',').filter_map(|x| x.trim().parse::<u16>().ok()).map(ShipId) {
                if let Some(s) = run.universe.ships.get(id) {
                    let c = s.command.and_then(|c| run.universe.commands.get(c));
                    let line = format!(
                        "ship {} {:?} p{} hp {:.0} pos ({:.0},{:.0},{:.0}) idle {} target {:?} cmd {:?} flags {:x} dock {:?}/{}",
                        id.0, s.ship_type, s.player, s.health, s.posinfo.position.x, s.posinfo.position.y, s.posinfo.position.z, s.ship_idle, s.attack.target,
                        c.map(|c| (c.order, c.attributes, c.attack_targets.clone(), c.waypoints.clone())), s.flags, s.base.dock_with, s.base.dock_state
                    );
                    if now.is_multiple_of(32) {
                        self.note(now, line);
                    }
                }
            }
        }
        if !run.game.ships_have_arrived {
            return;
        }
        let arrived = *self.arrived_at.get_or_insert(now);
        match run.game.sp_state {
            sp_state::NONE => {
                // A player cannot press anything while the universe is paused for an intel
                // event; the forced jump waits for it too (a jump during the pause leaves the
                // small ships outside: nothing processes their dock orders).
                if run.universe.universe_paused {
                    return;
                }
                if run.game.hyper.player_can_hyperspace && self.ready_to_jump(run) && run.hyperspace_button().is_ok() {
                    self.pressed = true;
                    self.note(now, "hyperspace button");
                    return;
                }
                if now.saturating_sub(arrived) >= self.budget && !self.forced && run.force_hyperspace().is_ok() {
                    self.forced = true;
                    self.note(now, "budget spent: jump forced");
                    return;
                }
                if self.orders && now >= self.last_orders + ORDER_PERIOD {
                    self.last_orders = now;
                    self.give_orders(run);
                }
            }
            sp_state::WAITING_ROLLCALL => {
                let t = *self.rollcall_at.get_or_insert(now);
                if std::env::var_os("HWC_DEBUG_GUARDS").is_some() && now.is_multiple_of(160) {
                    let u = &run.universe;
                    let out: Vec<String> = u.ship_list.iter().filter(|&&id| u.ships.ship(id).player == HUMAN_PLAYER && crate::campaign::is_fleet_ship(u, run.game.config.mission, id)).filter(|&&id| !crate::campaign::can_hyperspace(u, u.ships.ship(id).ship_type)).map(|&id| {
                        let s = u.ships.ship(id);
                        format!("{}:{:?} dock {:?}/{} cmd {:?}", id.0, s.ship_type, s.base.dock_with, s.base.dock_state, s.command.and_then(|c| u.commands.get(c)).map(|c| (c.order, c.dock.purpose)))
                    }).collect();
                    let mh = run.universe.base.players[HUMAN_PLAYER].mothership.map(|m| u.ships.ship(m).heading()).unwrap_or(Vector::ZERO);
                    let caps: Vec<String> = u.ship_list.iter().filter(|&&id| u.ships.ship(id).player == HUMAN_PLAYER && crate::campaign::is_fleet_ship(u, run.game.config.mission, id) && crate::campaign::can_hyperspace(u, u.ships.ship(id).ship_type)).map(|&id| {
                        let s = u.ships.ship(id);
                        let h = s.heading();
                        format!("{}:{:?} dot {:.3} upz {:.3} h {:?} mh {:?} rot {:?} cmd {:?} flags {:x}", id.0, s.ship_type, h.x * mh.x + h.y * mh.y + h.z * mh.z, s.rotinfo.coordsys.m[2], h, mh, s.rotinfo.rotspeed, s.command.and_then(|c| u.commands.get(c)).map(|c| c.order), s.flags)
                    }).collect();
                    self.note(now, format!("rollcall remaining {} outside {out:?} caps {caps:?}", run.game.hyper.rollcall_remaining));
                }
                // A player gives the fleet three minutes to dock and line up, then presses
                // "go now" once (the stragglers dock at once and join the parade).
                if now.saturating_sub(t) >= 16 * 180 && self.once("go now") {
                    run.hyperspace_go_now();
                    self.note(now, "roll call: go now");
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

    /// Before jumping a player brings the strike craft home (they dock in the roll call or
    /// instantly when the mission has none) and waits for builds to finish.
    fn ready_to_jump(&mut self, run: &mut MissionRun) -> bool {
        let u = &run.universe;
        if under_construction_any(u) {
            return false;
        }
        // The capital ships must be able to line up with the mothership for the jump: a player
        // waits until none is still tumbling on its side (idle ships right themselves).
        let m = run.game.config.mission;
        let acc = run.game.sp_tweaks.heading_accuracy;
        let tilted: Vec<ShipId> = u
            .ship_list
            .iter()
            .copied()
            .filter(|&id| u.ships.ship(id).player == HUMAN_PLAYER && crate::campaign::is_fleet_ship(u, m, id) && crate::campaign::can_hyperspace(u, u.ships.ship(id).ship_type))
            .filter(|&id| !matches!(crate::base::class_of(u, u.ships.ship(id).ship_type), Some(ShipClass::Mothership | ShipClass::Component)))
            .filter(|&id| u.ships.ship(id).rotinfo.coordsys.m[2] < acc)
            .collect();
        if tilted.is_empty() {
            return true;
        }
        // Stop them so that they right themselves; give up waiting after two minutes.
        let since = *self.tilt_since.get_or_insert(u.update_counter);
        let waited = u.update_counter.saturating_sub(since) > 16 * 120;
        if !waited {
            let busy: Vec<ShipId> = tilted.iter().copied().filter(|&id| !idle(u, id)).collect();
            if !busy.is_empty() {
                crate::command::cl_halt(&mut run.universe, &busy);
            }
        }
        waited
    }

    fn give_orders(&mut self, run: &mut MissionRun) {
        let now = run.universe.update_counter;
        let mission = run.game.config.mission;
        let fleet = Fleet::of(&run.universe);
        let Some(ms) = fleet.mothership else { return };
        // Common to every mission: harvesting, keeping a few workers and a defence force.
        harvest(&mut run.universe, &fleet);
        if std::env::var_os("HWC_DEBUG_RU").is_some() {
            let u = &run.universe;
            let orders: Vec<String> = fleet.workers.iter().map(|&w| format!("{:?}", u.ships.ship(w).command.and_then(|c| u.commands.get(c)).map(|c| c.order))).collect();
            let jobs: Vec<String> = u.base.factories.iter().flat_map(|f| f.jobs.iter().filter(|j| j.n_jobs > 0).map(|j| format!("{:?}x{}", j.info, j.n_jobs))).collect();
            let world: i64 = u.base.resources.list.iter().map(|r| i64::from(r.resource_value)).sum();
            self.note(now, format!("RU {} workers {orders:?} jobs {jobs:?} world {world}", u.base.players[HUMAN_PLAYER].resource_units));
            if let Some(&w) = fleet.workers.first() {
                let wp = u.ships.ship(w).posinfo.position;
                let near = crate::resource::find_nearest_resource(u, w, 0.0, Vector::ZERO);
                let unh = u.base.resources.list.iter().filter(|r| r.attributes & crate::base::attributes::RESOURCE_UNHARVESTABLE != 0).count();
                let busy = u.base.resources.list.iter().filter(|r| crate::resource::already_being_harvested(u, None, r.id)).count();
                let b = u.world_bounds;
                let out = u.base.resources.list.iter().filter(|r| !(r.position.x.abs() <= b.x && r.position.y.abs() <= b.y && r.position.z.abs() <= b.z)).count();
                let na = u.base.resources.list.iter().filter(|r| r.not_accessible != 0).count();
                let cid = u.ships.ship(w).command;
                let st_ = u.statics.ship(ShipType::sWorker).unwrap();
                let cr = cid.and_then(|c| u.commands.get(c)).and_then(|c| c.collect.resource);
                let rr = cr.and_then(|r| u.base.resources.get(r)).map(|r| format!("{:?} {:?} v{:?} val {}", r.kind, r.position, r.velocity, r.resource_value));
                self.note(now, format!("collect {cr:?} {rr:?} hs {} {} fn {:?}", u.ships.ship(w).base.harvest_state, u.ships.ship(w).base.harvest_state2, u.ships.ship(w).base.harvest_fn));
                self.note(now, format!("cmd {cid:?} sel {:?} me {w:?} in todo {} carried {} max {} at_once {}", cid.and_then(|c| u.commands.get(c)).map(|c| c.selection.clone()), cid.is_some_and(|c| u.commands.todo.contains(&c)), u.ships.ship(w).base.resources, st_.max_resources, st_.resources_at_one_time));
                self.note(now, format!("worker at {wp:?} nearest {near:?} n {} unharvestable {unh} busy {busy} outside {out} not_accessible {na} bounds {b:?} cur_player {} ai {}", u.base.resources.list.len(), u.base.cur_player, u.base.players[0].ai));
            }
        }
        if std::env::var_os("HWC_DEBUG_BUILD").is_some() && (now / ORDER_PERIOD).is_multiple_of(16) {
            let list = crate::level::construction_list(&run.universe, ms);
            let p = &run.universe.base.players[HUMAN_PLAYER];
            let techs: Vec<String> = (0..crate::base::NUM_TECHS).filter(|&t| p.tech.contains(t)).filter_map(crate::statics::Technology::from_index).map(|t| format!("{t:?}")).collect();
            self.note(now, format!("offers {list:?} tech {techs:?} RU {}", p.resource_units));
        }
        match mission {
            1 => self.mission1(run, &fleet, ms),
            2 => self.mission2(run, &fleet, ms),
            3 => self.mission3(run, &fleet, ms),
            4 => self.mission4(run, &fleet, ms),
            5 => self.mission5(run, &fleet, ms),
            6 => self.mission6(run, &fleet, ms),
            _ => self.generic(run, &fleet, ms),
        }
        let _ = now;
    }

    // ---- the generic player ------------------------------------------------------------

    /// Combat ships attack the enemy nearest the mothership; recons explore the level's
    /// volumes and points in turn.
    fn generic(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let mp = run.universe.ships.ship(ms).posinfo.position;
        defend(&mut run.universe, fleet, mp, f32::MAX);
        let spots = level_spots(run);
        if !fleet.recons.is_empty() && !spots.is_empty() {
            let to = spots[self.explore % spots.len()];
            self.explore += 1;
            move_ships(&mut run.universe, &fleet.recons, to);
        }
    }

    // ---- Mission 1: Hiigara -------------------------------------------------------------

    fn mission1(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sRammingFrigate, 4);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 15);
        let mp = run.universe.ships.ship(ms).posinfo.position;
        let target = if run.game.objective("DestroyBombers").is_some() {
            Some("TaiidanBombers")
        } else if run.game.objective("DestroyResourcers").is_some() || {
            // Enemy harvesters in sight are taken out before they strip the few asteroids.
            let seen = enemies(&run.universe);
            team_ships(run, "TaiidanResourcers").iter().any(|s| seen.contains(s))
        } {
            Some("TaiidanResourcers")
        } else if run.game.objective("DestroyProxAttackers").is_some() {
            // The attackers nearest the proximity sensors first (they must not lose them all).
            let sensors: Vec<Vector> = team_ships(run, "KushanProximityArray").iter().map(|&id| run.universe.ships.ship(id).posinfo.position).collect();
            let near = |run: &MissionRun, t: &str| -> f32 {
                team_ships(run, t).iter().map(|&id| {
                    let p = run.universe.ships.ship(id).posinfo.position;
                    sensors.iter().map(|&q| (p - q).mag_sq()).fold(f32::MAX, f32::min)
                }).fold(f32::MAX, f32::min)
            };
            ["TaiidanProxAttackAbomb1", "TaiidanProxAttackMGCorv1", "TaiidanProxAttackHInt1"]
                .into_iter()
                .filter(|t| !team_ships(run, t).is_empty())
                .min_by(|a, b| near(run, a).total_cmp(&near(run, b)))
        } else if run.game.objective("DestroyDestroyer").is_some() {
            Some("TaiidanDestroyer")
        } else {
            None
        };
        if run.game.objective("ExploreRegion").is_some() {
            if let Some(c) = volume_centre(run, "UNEXPLORED") {
                move_ships(&mut run.universe, &fleet.recons, c);
                if self.once("m1 explore") {
                    self.note(now, "M1: recons sent to the unexplored region");
                }
            }
        }
        match target.map(|t| (t, team_ships(run, t))) {
            Some((t, ships)) if !ships.is_empty() => {
                let group = if run.game.objective("DestroyProxAttackers").is_some() { 4 } else { 8 };
                strike(&mut run.universe, &fleet.combat, &ships, group);
                if self.once(t) {
                    self.note(now, format!("M1: fleet attacks {t}"));
                }
            }
            _ => defend(&mut run.universe, fleet, mp, 20000.0),
        }
    }

    // ---- Mission 2: outskirts of the Hiigaran system -----------------------------------

    fn mission2(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        let micro_done = count_type(&run.universe, ShipType::sMothershipMicro) > 0 && !under_construction_any(&run.universe);
        build_up_to(&mut run.universe, ms, ShipType::sRecon, 2);
        let worker_target = if micro_done { 8 } else { 5 };
        build_up_to(&mut run.universe, ms, ShipType::sWorker, worker_target);
        if run.game.objective("Build").is_some() {
            queue_up_to(&mut run.universe, ms, ShipType::sMothershipMicro, 1);
        }
        for tech in ["TECH_REPAIR", "TECH_SALVAGE"] {
            research(&mut run.universe, tech);
        }
        // Acolytes one at a time (a long queue charges RUs the workers and the repair need).
        let pending = crate::construction::job(&run.universe, ms, ShipType::sAcolyte).map_or(0, |j| j.n_jobs.max(0));
        if micro_done && pending == 0 {
            build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 16);
        }
        // Once salvage is researched, new workers come with the upgrade: build two.
        let salvage_tech = crate::statics::Technology::from_name("TECH_SALVAGE").is_some_and(|t| run.universe.base.players[HUMAN_PLAYER].tech.contains(t.index()));
        let upgraded = fleet.workers.iter().filter(|&&w| crate::upgrade::has_upgrade(run.universe.ships.ship(w), crate::upgrade::BIT_SALVAGE)).count();
        if salvage_tech && upgraded < 2 {
            let n = fleet.workers.len() + 2 - upgraded;
            queue_up_to(&mut run.universe, ms, ShipType::sWorker, n.min(9));
        }
        // Find the destroyer: the recons search the four quadrants in turn.
        if run.game.objective("FindDestroyer").is_some() {
            let r = 75000.0;
            let spots = [Vector::new(r, r, 0.0), Vector::new(-r, r, 0.0), Vector::new(-r, -r, 0.0), Vector::new(r, -r, 0.0)];
            for (k, &id) in fleet.recons.iter().enumerate() {
                if idle(&run.universe, id) {
                    let to = spots[(self.explore + k) % spots.len()];
                    move_ships(&mut run.universe, &[id], to);
                }
            }
            self.explore += 1;
        }
        let destroyer = team_ships(run, "kDestroyer");
        if run.game.objective("RepairDestroyer").is_some() && !destroyer.is_empty() {
            // Two workers repair once there are workers enough to keep harvesting. Past 40 %
            // the Bushan-Re draws an ambush, so a player first gathers a guard for it.
            let dmax = run.universe.statics.ship(run.universe.ships.ship(destroyer[0]).ship_type).map_or(1.0, |s| s.health.max_health);
            let pct = run.universe.ships.ship(destroyer[0]).health / dmax;
            let ready = fleet.combat.len() >= 14 || pct < 0.36;
            let n = if ready { fleet.workers.len().saturating_sub(3).min(3) } else { 0 };
            let repairers: Vec<ShipId> = fleet.workers.iter().copied().take(n).collect();
            if !repairers.is_empty() {
                crate::ships::worker_repair::cl_repair(&mut run.universe, &repairers, &destroyer);
                if self.once("m2 repair") {
                    self.note(now, "M2: workers repair the Bushan-Re");
                }
            }
            // The fleet guards the Bushan-Re (guards hit back at whatever attacks it).
            guard(&mut run.universe, &fleet.combat, &destroyer);
            if self.once("m2 guard") {
                self.note(now, "M2: the fleet guards the Bushan-Re");
            }
        } else {
            defend(&mut run.universe, fleet, mp, 25000.0);
        }
        let pod = team_ships(run, "dMessagePod");
        if run.game.objective("SalvagePod").is_some() && !pod.is_empty() {
            let p = run.universe.ships.ship(pod[0]).posinfo.position;
            // Workers with the salvage upgrade (new ones have it; old ones get it by docking).
            let can = |u: &Universe, w: ShipId| crate::upgrade::has_upgrade(u.ships.ship(w), crate::upgrade::BIT_SALVAGE);
            let salvaging = |u: &Universe, w: ShipId| u.ships.ship(w).command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::SPECIAL && c.special_targets.contains(&pod[0]));
            let busy = fleet.workers.iter().filter(|&&w| salvaging(&run.universe, w)).count();
            let fresh: Vec<ShipId> = fleet.workers.iter().copied().filter(|&w| can(&run.universe, w) && !salvaging(&run.universe, w)).take(2usize.saturating_sub(busy)).collect();
            if !fresh.is_empty() {
                crate::salvage::cl_salvage(&mut run.universe, &fresh, &pod);
                if self.once("m2 salvage") {
                    self.note(now, "M2: workers salvage the message pod");
                }
            } else if busy == 0 && self.once("m2 upgrade") {
                let w: Vec<ShipId> = fleet.workers.iter().copied().take(2).collect();
                crate::dock::cl_dock(&mut run.universe, &w, crate::dock::purpose::DOCK, Some(ms));
                self.note(now, "M2: workers dock for the salvage upgrade (late)");
            }
            // Escort the salvage.
            guard(&mut run.universe, &fleet.combat, &pod);
            let _ = p;
        }
        if run.game.objective("DestroyPirates").is_some() {
            let at = pod.first().map(|&p| run.universe.ships.ship(p).posinfo.position).unwrap_or(mp);
            attack_near(&mut run.universe, fleet, at, 40000.0);
            attack_near(&mut run.universe, fleet, mp, f32::MAX);
        }
    }

    // ---- Mission 3: Coruc-Tel -----------------------------------------------------------

    fn mission3(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        build_up_to(&mut run.universe, ms, ShipType::sMothershipDockingBay, 1);
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sRecon, 2);
        build_up_to(&mut run.universe, ms, ShipType::sMimic, 1);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 10);
        research(&mut run.universe, "TECH_MIMIC");
        let clee = team_ships(run, "CleeSan");
        let Some(&c) = clee.first() else {
            defend(&mut run.universe, fleet, mp, 20000.0);
            return;
        };
        let cp = run.universe.ships.ship(c).posinfo.position;
        if run.game.objective("Find").is_some() {
            // The Clee-San is open to mines for a moment once found: mines lying near it are
            // set off by Acolyte decoys first, while the recons wait out of sight.
            let mines: Vec<Vector> = run
                .universe
                .missiles
                .iter()
                .filter(|m| m.kind == crate::missile::kind::MINE && !m.is_dead() && (m.posinfo.position - cp).mag_sq() < 6000.0 * 6000.0)
                .map(|m| m.posinfo.position)
                .collect();
            if mines.is_empty() {
                move_ships(&mut run.universe, &fleet.recons, cp);
                if self.once("m3 find") {
                    self.note(now, "M3: recons fly to the Clee-San");
                }
            } else {
                let decoys: Vec<ShipId> = fleet.combat.iter().copied().filter(|&id| run.universe.ships.ship(id).ship_type == ShipType::sAcolyte).take(mines.len().min(4)).collect();
                for (k, &d) in decoys.iter().enumerate() {
                    move_ships(&mut run.universe, &[d], mines[k]);
                }
                for &r in &fleet.recons {
                    let rp = run.universe.ships.ship(r).posinfo.position;
                    let fow = run.universe.statics.ship(ShipType::sRecon).map_or(0.0, |s| s.fow_radius);
                    if (rp - cp).mag_sq() < (fow + 4000.0) * (fow + 4000.0) {
                        let mut away = rp - cp;
                        away.normalize();
                        move_ships(&mut run.universe, &[r], cp + away.scale(fow + 6000.0));
                    }
                }
                if !decoys.is_empty() && self.once("m3 find mines") {
                    self.note(now, format!("M3: decoys clear {} mines by the Clee-San before it is found", mines.len()));
                }
            }
        }
        if run.game.objective("Mimic").is_some() {
            let mimics: Vec<ShipId> = fleet.all.iter().copied().filter(|&id| matches!(run.universe.ships.ship(id).ship_type, ShipType::sMimic | ShipType::sMCV)).collect();
            if !mimics.is_empty() {
                move_ships(&mut run.universe, &mimics, cp);
                if self.once("m3 mimic") {
                    self.note(now, "M3: Mimic sent to the Clee-San");
                }
            }
        }
        if run.game.objective("Lure").is_some() {
            // The Clee-San will not set off while mines are within 14 km of it: Acolytes fly
            // at them as decoys and set them off.
            let mines: Vec<Vector> = run
                .universe
                .missiles
                .iter()
                .filter(|m| m.kind == crate::missile::kind::MINE && !m.is_dead() && (m.posinfo.position - cp).mag_sq() < 15000.0 * 15000.0)
                .map(|m| m.posinfo.position)
                .collect();
            let decoys: Vec<ShipId> = fleet.combat.iter().copied().filter(|&id| run.universe.ships.ship(id).ship_type == ShipType::sAcolyte).rev().take(mines.len().min(3)).collect();
            for (k, &d) in decoys.iter().enumerate() {
                move_ships(&mut run.universe, &[d], mines[k]);
            }
            if std::env::var_os("HWC_DEBUG_GUARDS").is_some() {
                let st_ = run.kas.team(kas_hash("CleeSan")).and_then(|t| t.state).map(|h| format!("{h:08x}"));
                let dpos: Vec<String> = decoys.iter().map(|&d| format!("{:.0}", (run.universe.ships.ship(d).posinfo.position - mines[0]).mag_sq().sqrt())).collect();
                self.note(now, format!("M3 lure: mines<15k {} state {st_:?} decoy d {dpos:?} ms d {:.0} Mimic var {:?}", mines.len(), (mp - cp).mag_sq().sqrt(), run.var(Some("CleeSan"), "Mimic")));
            }
            if !decoys.is_empty() && self.once("m3 mines") {
                self.note(now, format!("M3: decoys set off the mines near the Clee-San ({} mines)", mines.len()));
            }
        }
        if run.game.objective("Lure").is_some() {
            // The Clee-San comes once the mothership is near it: bring the mothership over.
            let d = (cp - mp).mag_sq();
            // Once it is docking with the mothership, the mothership holds still for it.
            let docking = run.universe.ships.ship(c).base.dock_with == Some(ms);
            let ms_moving = run.universe.ships.ship(ms).command.and_then(|k| run.universe.commands.get(k)).is_some_and(|k| k.order == crate::command::order::MOVE);
            if docking && std::env::var_os("HWC_DEBUG_GUARDS").is_some() {
                let u = &run.universe;
                let users: Vec<String> = u.ships.iter().filter(|s| s.base.dock_with.is_some_and(|w| w == ms || crate::clamp::slaves_of(u, ms).contains(&w))).map(|s| format!("{}:{:?} st{} pt{} w{:?}", s.id.0, s.ship_type, s.base.dock_state, s.base.dock_point, s.base.dock_with.map(|w| u.ships.ship(w).ship_type))).collect();
                let t = crate::dock::point_type_for(u, ShipType::dEcclesiam, 0, crate::dock::purpose::DOCK_STAY, ms);
                let rp = crate::dock::reserved_point(u, c, ms).map(|p| format!("type {:?} pos {:?} normal {:?} up {} heading {}", p.dock_type, p.position, p.cone_normal, p.up_direction, p.heading_direction));
                let cs = u.ships.ship(c);
                self.note(now, format!("M3 dock users {users:?} ecc point type {t} d {:.0} point {rp:?} ecc heading {:?} up {:?} rotspeed {:?} torque {:?} flags {:x} drot {}", (cp - mp).mag_sq().sqrt(), cs.rotinfo.coordsys.col(2), cs.rotinfo.coordsys.col(0), cs.rotinfo.rotspeed, cs.rotinfo.torque, cs.flags, cs.dont_rotate_ever));
            }
            if docking && d < 6000.0 * 6000.0 {
                // A Clee-San that has hung at the mouth of its dock for half a minute gets
                // the mothership moved a little under it (the cone turns with it).
                let stuck = run.universe.ships.ship(c).base.dock_state == crate::dock::state::FLY_TO_ORIGIN;
                let since = if stuck { *self.m3_stuck_since.get_or_insert(now) } else { self.m3_stuck_since.take().map_or(now, |_| now) };
                if stuck && now.saturating_sub(since) > 16 * 30 {
                    let side = run.universe.ships.ship(ms).rotinfo.coordsys.col(1).scale(1500.0);
                    move_ships(&mut run.universe, &[ms], mp + side);
                    self.m3_stuck_since = Some(now);
                    self.note(now, "M3: the mothership shifts under the docking Clee-San");
                } else if ms_moving && !stuck {
                    crate::command::cl_halt(&mut run.universe, &[ms]);
                }
            } else if d > 5000.0 * 5000.0 && idle(&run.universe, ms) {
                move_ships(&mut run.universe, &[ms], cp);
                if self.once("m3 lure") {
                    self.note(now, "M3: mothership moves to the Clee-San");
                }
            }
        }
        defend(&mut run.universe, fleet, mp, 20000.0);
    }

    // ---- Mission 4: deep space ----------------------------------------------------------

    fn mission4(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        if run.game.objective("Replace").is_some() || count_type(&run.universe, ShipType::sMothershipDockingBay) == 0 {
            build_up_to(&mut run.universe, ms, ShipType::sMothershipDockingBay, 1);
        }
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 14);
        if run.game.objective("Research").is_some() {
            research(&mut run.universe, "TECH_RESEARCH_CLEESAN_ATTACK");
        }
        if run.game.objective("Escort").is_some() {
            let clee = team_ships(run, "Ecclesiam");
            let acolytes: Vec<ShipId> = fleet.all.iter().copied().filter(|&id| run.universe.ships.ship(id).ship_type == ShipType::sAcolyte).take(12).collect();
            if !clee.is_empty() && acolytes.len() >= 10 {
                let guarding = acolytes.iter().all(|&a| crate::command::protect_list(&run.universe, a) == clee.as_slice());
                if !guarding {
                    crate::command::cl_wrap_protect(&mut run.universe, &acolytes, &clee);
                    if self.once("m4 escort") {
                        self.note(now, format!("M4: {} acolytes guard the Clee-San", acolytes.len()));
                    }
                }
                return;
            }
        }
        defend(&mut run.universe, fleet, mp, 15000.0);
    }

    // ---- Mission 5: Aiowa ---------------------------------------------------------------

    fn mission5(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        build_up_to(&mut run.universe, ms, ShipType::sRecon, 2);
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 7);
        if run.game.objective("BuildEngineering").is_some() {
            queue_up_to(&mut run.universe, ms, ShipType::sMothershipSpecial, 1);
        }
        // The fleet: Acolytes first, then ACVs (EMP and missiles against fighters).
        if count_type(&run.universe, ShipType::sMothershipSpecial) > 0 || run.game.objective("BuildEngineering").is_none() {
            build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 30);
            build_up_to(&mut run.universe, ms, ShipType::sACV, 6);
        }
        if run.game.objective("InterceptBentusi").is_some() {
            if let Some(&city) = team_ships(run, "trCity").first() {
                let p = run.universe.ships.ship(city).posinfo.position;
                move_ships(&mut run.universe, &fleet.recons, p);
                if self.once("m5 intercept") {
                    self.note(now, "M5: recons intercept the Bentusi");
                }
            }
        }
        // The Caal-Shto is helpless: once it is ours to protect the fleet stays with it and
        // fights whatever comes near; with nothing near, a third stays and the rest hunt the
        // Beast ships nearest to it.
        let caal = team_ships(run, "CaalShto");
        let protecting = !caal.is_empty() && (run.game.objective("ProtectCaalShto").is_some() || run.game.objective("DestroyBeast").is_some());
        if !protecting {
            // Until then the fleet stays home (the Bentusi's end clears 22.5 km around it).
            defend(&mut run.universe, fleet, mp, 15000.0);
            return;
        }
        let cp = run.universe.ships.ship(caal[0]).posinfo.position;
        if std::env::var_os("HWC_DEBUG_FLEET").is_some() {
            let u = &run.universe;
            let n_near = enemies(u).into_iter().filter(|&e| (u.ships.ship(e).posinfo.position - cp).mag_sq() < 10000.0 * 10000.0).count();
            let ours = fleet.combat.iter().filter(|&&id| (u.ships.ship(id).posinfo.position - cp).mag_sq() < 6000.0 * 6000.0).count();
            self.note(now, format!("M5 caal hp {:.0} combat {} near caal {ours} enemies near {n_near} RU {} su {}/{} workers {}", u.ships.ship(caal[0]).health, fleet.combat.len(), u.base.players[HUMAN_PLAYER].resource_units, u.base.players[HUMAN_PLAYER].su_used, u.base.players[HUMAN_PLAYER].su_capacity, fleet.workers.len()));
        }
        if self.once("m5 protect") {
            self.note(now, "M5: the fleet protects the Caal-Shto");
        }
        let u = &mut run.universe;
        let mut near: Vec<ShipId> = enemies(u).into_iter().filter(|&e| (u.ships.ship(e).posinfo.position - cp).mag_sq() < 10000.0 * 10000.0).collect();
        near.sort_by(|&a, &b| (u.ships.ship(a).posinfo.position - cp).mag_sq().total_cmp(&(u.ships.ship(b).posinfo.position - cp).mag_sq()));
        if !near.is_empty() {
            near.truncate(6);
            let ready: Vec<ShipId> = fleet.combat.iter().copied().filter(|&id| !attacking_any(u, id, &near)).collect();
            attack(u, &ready, &near);
            return;
        }
        let keep = fleet.combat.len() / 3;
        let (home, hunters) = fleet.combat.split_at(keep);
        for &id in home {
            if idle(u, id) && (u.ships.ship(id).posinfo.position - cp).mag_sq() > 4000.0 * 4000.0 {
                move_ships(u, &[id], cp);
            }
        }
        if run.game.objective("DestroyBeast").is_some() && hunters.len() >= 12 {
            let f = Fleet { combat: hunters.to_vec(), ..fleet.clone() };
            attack_near(&mut run.universe, &f, cp, f32::MAX);
            if self.once("m5 hunt") {
                self.note(now, "M5: hunting the rest of the Beast fleet");
            }
        } else {
            for &id in hunters {
                if idle(u, id) && (u.ships.ship(id).posinfo.position - cp).mag_sq() > 4000.0 * 4000.0 {
                    move_ships(u, &[id], cp);
                }
            }
        }
    }

    // ---- Mission 6: Kadiir nebula -------------------------------------------------------

    /// The Caal-Shto goes from nav point to nav point while the mothership is within 12 km
    /// of it and fewer than five mines are within 16 km; it stops for a single mine. The
    /// player scouts the nav points with recons (the last one by finding the heavy cruiser
    /// near it), keeps the mothership and the fleet with the Caal-Shto, sets the mines off
    /// with decoys, hunts the Taiidan scouts before they get home to their station (that
    /// would wake the Death fleet), and clears the heavy cruiser's group ahead of the
    /// Caal-Shto at the last nav point.
    fn mission6(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        build_up_to(&mut run.universe, ms, ShipType::sRecon, 8);
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 24);
        build_up_to(&mut run.universe, ms, ShipType::sACV, 6);
        let point = |run: &MissionRun, n: &str| run.game.level.point(kas_hash(n)).map(|p| Vector::new(p[0], p[1], p[2]));
        let navs: Vec<Vector> = ["Nav1", "Nav2", "Nav3", "Nav4", "Nav5"].iter().filter_map(|n| point(run, n)).collect();
        let caal = team_ships(run, "CaalShto").first().copied();
        let station = team_ships(run, "TStation").first().map(|&s| run.universe.ships.ship(s).posinfo.position);
        let mut recons = fleet.recons.clone();
        // A recon explores the station, then the rest scout.
        if run.game.objective("Explore").is_some() && !recons.is_empty() {
            let r = recons.remove(0);
            if let Some(p) = station {
                move_ships(&mut run.universe, &[r], p);
                if self.once("m6 explore") {
                    self.note(now, "M6: recon explores the Taiidan base");
                }
            }
        }
        // One recon to each nav point.
        if run.var(None, "G_discovered") == Some(1) || run.game.objective("Scout").is_some() {
            for (k, &to) in navs.iter().enumerate() {
                let Some(&r) = recons.get(k) else { break };
                if (run.universe.ships.ship(r).posinfo.position - to).mag_sq() > 3000.0 * 3000.0 && idle(&run.universe, r) {
                    move_ships(&mut run.universe, &[r], to);
                }
            }
            if self.once("m6 scout") {
                self.note(now, "M6: recons scout the nav points");
            }
        }
        let spare: Vec<ShipId> = recons.iter().copied().skip(navs.len()).collect();
        let Some(c) = caal else {
            defend(&mut run.universe, fleet, mp, 12000.0);
            return;
        };
        let cp = run.universe.ships.ship(c).posinfo.position;
        let bits = run.kas.var(Some(kas_hash("CaalShto")), 0x0005_26b3).unwrap_or(0);
        if std::env::var_os("HWC_DEBUG_GUARDS").is_some() {
            let u = &run.universe;
            let mines = u.missiles.iter().filter(|m| m.kind == crate::missile::kind::MINE && (m.posinfo.position - cp).mag_sq() < 16000.0 * 16000.0).count();
            let state = run.kas.team(kas_hash("CaalShto")).and_then(|t| t.state).map(|h| format!("{h:08x}"));
            let near = |p: usize, r: f32| u.ship_list.iter().filter(|&&e| u.ships.ship(e).player == p && (u.ships.ship(e).posinfo.position - cp).mag_sq() < r * r).count();
            self.note(
                now,
                format!(
                    "M6 caal hp {:.0} at ({:.0},{:.0}) bits {bits} mines<16k {mines} state {state:?} death {:?} combat {} ours<6k {} theirs<10k {} ms d {:.0} recons {}",
                    u.ships.ship(c).health,
                    cp.x,
                    cp.y,
                    run.var(None, "G_Death"),
                    fleet.combat.len(),
                    near(HUMAN_PLAYER, 6000.0),
                    near(1, 10000.0),
                    (mp - cp).mag_sq().sqrt(),
                    fleet.recons.len()
                ),
            );
            let close: Vec<String> = u.ship_list.iter().filter(|&&e| u.ships.ship(e).player == 1 && (u.ships.ship(e).posinfo.position - cp).mag_sq() < 12000.0f32 * 12000.0).map(|&e| format!("{}:{:?} hp{:.0} vis {}", e.0, u.ships.ship(e).ship_type, u.ships.ship(e).health, crate::visibility::visible_to_player(u, e, HUMAN_PLAYER))).collect();
            if !close.is_empty() {
                self.note(now, format!("M6 near caal: {close:?}"));
            }
        }
        // Before the last leg the heavy cruiser's group waits near the last nav point; it
        // calls the Death fleet when it sees the Caal-Shto or the mothership. The player holds
        // the mothership back (the Caal-Shto waits for it) until the group is gone.
        let visible = enemies(&run.universe);
        let cruiser_group: Vec<ShipId> = match (navs.get(3), navs.get(4)) {
            (Some(_), Some(&n5)) if bits & 8 != 0 && (cp - n5).mag_sq() < 50000.0 * 50000.0 => {
                let cruiser = team_ships(run, "HvCruiser");
                let mut v: Vec<ShipId> = visible.iter().copied().filter(|&e| (run.universe.ships.ship(e).posinfo.position - n5).mag_sq() < 25000.0 * 25000.0 || cruiser.contains(&e)).collect();
                v.sort_by(|&a, &b| (run.universe.ships.ship(a).posinfo.position - cp).mag_sq().total_cmp(&(run.universe.ships.ship(b).posinfo.position - cp).mag_sq()));
                v
            }
            _ => Vec::new(),
        };
        let hold = !cruiser_group.is_empty();
        if hold {
            let n5 = navs[4];
            let mut away = cp - n5;
            away.normalize();
            let back = cp + away.scale(15000.0);
            if (mp - back).mag_sq() > 3000.0 * 3000.0 && (mp - cp).mag_sq() < 13000.0 * 13000.0 {
                move_ships(&mut run.universe, &[ms], back);
            }
            if self.once("m6 hold") {
                self.note(now, "M6: the mothership holds back until the heavy cruiser's group is gone");
            }
        }
        // The mothership keeps with the Caal-Shto: it flies a little ahead of it towards the
        // point the Caal-Shto is heading for (the Caal-Shto waits whenever the mothership is
        // more than 12 km away, and only goes on once it is within 6 km).
        if run.game.objective("Escort").is_some() && !hold {
            let goal = point(run, "ESCORT").unwrap_or(cp);
            let mut dir = goal - cp;
            let left = dir.mag_sq().sqrt();
            let dest = if left > 3000.0 {
                dir.normalize();
                cp + dir.scale(3000.0)
            } else {
                goal
            };
            let going = run.universe.ships.ship(ms).command.and_then(|k| run.universe.commands.get(k)).is_some_and(|k| k.order == crate::command::order::MOVE && k.waypoints.last().is_some_and(|w| (*w - dest).mag_sq() < 1500.0 * 1500.0));
            if !going && (mp - dest).mag_sq() > 1500.0 * 1500.0 {
                move_ships(&mut run.universe, &[ms], dest);
            }
            if self.once("m6 escort") {
                self.note(now, "M6: the mothership escorts the Caal-Shto");
            }
        }
        // Mines near the Caal-Shto: a decoy flies at each until it goes off.
        let mines: Vec<Vector> = run
            .universe
            .missiles
            .iter()
            .filter(|m| m.kind == crate::missile::kind::MINE && !m.is_dead() && (m.posinfo.position - cp).mag_sq() < 17000.0 * 17000.0)
            .map(|m| m.posinfo.position)
            .collect();
        let mut decoys: Vec<ShipId> = spare;
        decoys.extend(fleet.combat.iter().copied().filter(|&id| run.universe.ships.ship(id).ship_type == ShipType::sAcolyte).rev().take(2));
        let used = mines.len().min(decoys.len());
        for (k, &m) in mines.iter().take(used).enumerate() {
            let d = decoys[k];
            if idle(&run.universe, d) || (run.universe.ships.ship(d).posinfo.position - m).mag_sq() > 2000.0 * 2000.0 {
                move_ships(&mut run.universe, &[d], m);
            }
        }
        if !mines.is_empty() && self.once("m6 mines") {
            self.note(now, format!("M6: decoys set off {} mines near the Caal-Shto", mines.len()));
        }
        let combat: Vec<ShipId> = fleet.combat.iter().copied().filter(|id| !decoys[..used].contains(id)).collect();
        // The Taiidan scouts: a hunting group goes for any it can see.
        let scouts: Vec<ShipId> = ["Scouts", "Scouts2", "Scouts3", "Scouts4"].iter().flat_map(|t| team_ships(run, t)).collect();
        let seen: Vec<ShipId> = scouts.into_iter().filter(|s| visible.contains(s)).collect();
        let (hunters, escort) = combat.split_at(combat.len() / 2);
        if !seen.is_empty() {
            let ready: Vec<ShipId> = hunters.iter().copied().filter(|&id| !attacking_any(&run.universe, id, &seen)).collect();
            attack(&mut run.universe, &ready, &seen);
            if self.once("m6 scouts") {
                self.note(now, "M6: hunting the Taiidan scouts");
            }
        }
        // The heavy cruiser's group is cleared by the whole fleet, nearest first.
        if hold {
            let targets: Vec<ShipId> = cruiser_group.iter().copied().take(6).collect();
            let ready: Vec<ShipId> = combat.iter().copied().filter(|&id| !attacking_any(&run.universe, id, &targets)).collect();
            attack(&mut run.universe, &ready, &targets);
            if self.once("m6 cruiser") {
                self.note(now, "M6: the fleet clears the heavy cruiser's group at the last nav point");
            }
            return;
        }
        // The rest stay with the Caal-Shto and fight what comes within 10 km of it (once the
        // Death fleet is out, everyone and within 16 km).
        let death = run.var(None, "G_Death") == Some(1);
        let (escort, range) = if death { (&combat[..], 16000.0f32) } else { (escort, 10000.0f32) };
        let mut near: Vec<ShipId> = visible.iter().copied().filter(|&e| (run.universe.ships.ship(e).posinfo.position - cp).mag_sq() < range * range).collect();
        near.sort_by(|&a, &b| (run.universe.ships.ship(a).posinfo.position - cp).mag_sq().total_cmp(&(run.universe.ships.ship(b).posinfo.position - cp).mag_sq()));
        if !near.is_empty() {
            near.truncate(6);
            let ready: Vec<ShipId> = escort.iter().copied().filter(|&id| !attacking_any(&run.universe, id, &near)).collect();
            attack(&mut run.universe, &ready, &near);
            return;
        }
        let home: Vec<ShipId> = if seen.is_empty() { combat.clone() } else { escort.to_vec() };
        for id in home {
            if idle(&run.universe, id) && (run.universe.ships.ship(id).posinfo.position - cp).mag_sq() > 4000.0 * 4000.0 {
                move_ships(&mut run.universe, &[id], cp);
            }
        }
    }
}

/// The player's ships by role.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct Fleet {
    mothership: Option<ShipId>,
    all: Vec<ShipId>,
    combat: Vec<ShipId>,
    recons: Vec<ShipId>,
    workers: Vec<ShipId>,
}

impl Fleet {
    fn of(u: &Universe) -> Fleet {
        let mothership = u.base.players[HUMAN_PLAYER].mothership.filter(|&m| u.ships.get(m).is_some());
        let mut f = Fleet { mothership, ..Fleet::default() };
        for &id in &u.ship_list {
            let s = u.ships.ship(id);
            if s.player != HUMAN_PLAYER || s.flags & (sof::HIDE | sof::DEAD) != 0 || under_construction(u, id) {
                continue;
            }
            // Ships the mission controls (escorted ships, allies handed over) are not ours to order;
            // nor are the story ships the mission hands over (the Clee-San, the Caal-Shto).
            if s.special_flags2 & 0x0010_0000 != 0 || s.ship_race == crate::statics::ShipRace::Derelict || s.ship_type == ShipType::kCarrier {
                continue;
            }
            f.all.push(id);
            let class = crate::base::class_of(u, s.ship_type);
            match (s.ship_type, class) {
                (ShipType::sRecon, _) => f.recons.push(id),
                (ShipType::sWorker, _) => f.workers.push(id),
                (_, Some(ShipClass::Mothership | ShipClass::Component | ShipClass::NonCombat | ShipClass::Resource)) => {}
                (ShipType::sProcessor | ShipType::sRepairDroid | ShipType::sMimic | ShipType::sMCV, _) => {}
                (t, _) => {
                    if u.statics.ship(t).is_some_and(|st_| st_.guns.as_ref().is_some_and(|g| !g.is_empty()) || t == ShipType::sRammingFrigate) {
                        f.combat.push(id);
                    }
                }
            }
        }
        f
    }
}

/// A ship with nothing to do: no order, or only holding its place in the fleet's military
/// parade (where the arrival and the jump leave the ships until the player orders them).
fn idle(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| {
        s.command.and_then(|c| u.commands.get(c)).is_none_or(|c| {
            (c.order == crate::command::order::NULL && c.attributes & crate::command::attr::PROTECTING == 0) || c.order == crate::command::order::MILITARY_PARADE
        })
    })
}

/// The live, visible ships of a KAS team (by its label in the level files).
fn team_ships(run: &MissionRun, label: &str) -> Vec<ShipId> {
    let u = &run.universe;
    run.game
        .level
        .team(kas_hash(label))
        .map(|t| t.ships.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.flags & (sof::HIDE | sof::DEAD) == 0) && u.ship_list.contains(&id)).collect())
        .unwrap_or_default()
}

fn volume_centre(run: &MissionRun, label: &str) -> Option<Vector> {
    run.game.level.volume(kas_hash(label)).map(|v| v.center())
}

fn level_spots(run: &MissionRun) -> Vec<Vector> {
    let mut spots: Vec<Vector> = run.game.level.volumes.iter().map(|(_, v)| v.center()).collect();
    spots.extend(run.game.level.points.iter().map(|p| Vector::new(p.1[0], p.1[1], p.1[2])));
    spots
}

fn move_ships(u: &mut Universe, ships: &[ShipId], to: Vector) {
    let sel: Vec<ShipId> = ships.iter().copied().filter(|&id| u.ships.get(id).is_some()).collect();
    if let Some(&first) = sel.first() {
        let from = u.ships.ship(first).posinfo.position;
        crate::command::cl_move(u, &sel, crate::command::waypoint_mode::ONCE, from, &[to]);
    }
}

/// A strike on `targets`: ships already on it carry on; the idle ones at home join once there
/// are at least `group` of them together (a player does not send ships in one at a time).
fn strike(u: &mut Universe, ships: &[ShipId], targets: &[ShipId], group: usize) {
    let on_it = |u: &Universe, id: ShipId| {
        u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::ATTACK && c.attack_targets.iter().any(|t| targets.contains(t)))
    };
    let busy = ships.iter().filter(|&&id| on_it(u, id)).count();
    let free: Vec<ShipId> = ships.iter().copied().filter(|&id| !on_it(u, id) && idle(u, id)).collect();
    if busy > 0 && !targets.iter().any(|t| u.ships.get(*t).is_some()) {
        return;
    }
    if free.len() >= group || (busy >= group && !free.is_empty()) {
        attack(u, &free, targets);
    }
}

/// Whether `id` is attacking one of `targets`.
fn attacking_any(u: &Universe, id: ShipId, targets: &[ShipId]) -> bool {
    u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::ATTACK && c.attack_targets.iter().any(|t| targets.contains(t)))
}

/// Ships not already attacking one of `targets` attack them.
fn attack(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let fresh: Vec<ShipId> = ships
        .iter()
        .copied()
        .filter(|&id| {
            u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)).is_none_or(|c| c.order != crate::command::order::ATTACK || !c.attack_targets.iter().any(|t| targets.contains(t)))
        })
        .collect();
    if !fresh.is_empty() && !targets.is_empty() {
        crate::command::cl_attack(u, &fresh, targets);
    }
}

fn enemies(u: &Universe) -> Vec<ShipId> {
    u.ship_list
        .iter()
        .copied()
        .filter(|&id| {
            u.ships.get(id).is_some_and(|s| {
                s.player != HUMAN_PLAYER && !u.players[HUMAN_PLAYER].is_ally(s.player) && s.flags & (sof::HIDE | sof::DEAD) == 0 && s.flags & sof::TARGETABLE != 0
            })
        })
        .filter(|&id| crate::visibility::visible_to_player(u, id, HUMAN_PLAYER))
        .collect()
}

/// The combat ships attack the enemy nearest `at` within `range`.
fn attack_near(u: &mut Universe, fleet: &Fleet, at: Vector, range: f32) {
    let e = enemies(u)
        .into_iter()
        .filter(|&id| range == f32::MAX || (u.ships.ship(id).posinfo.position - at).mag_sq() <= range * range)
        .min_by(|&a, &b| (u.ships.ship(a).posinfo.position - at).mag_sq().total_cmp(&(u.ships.ship(b).posinfo.position - at).mag_sq()));
    if let Some(e) = e {
        let ready: Vec<ShipId> = fleet.combat.iter().copied().filter(|&id| idle(u, id) || u.ships.ship(id).command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::MOVE)).collect();
        attack(u, &ready, &[e]);
    }
}

/// `ships` guard `protect` (the player's protect order), unless they already do or are
/// fighting for it.
fn guard(u: &mut Universe, ships: &[ShipId], protect: &[ShipId]) {
    let fresh: Vec<ShipId> = ships
        .iter()
        .copied()
        .filter(|&id| {
            let c = u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c));
            c.is_none_or(|c| c.attributes & crate::command::attr::PROTECTING == 0 || c.protect.as_slice() != protect)
        })
        .collect();
    if !fresh.is_empty() {
        crate::command::cl_wrap_protect(u, &fresh, protect);
    }
}

/// Idle combat ships defend `at`.
fn defend(u: &mut Universe, fleet: &Fleet, at: Vector, range: f32) {
    attack_near(u, fleet, at, range);
}

/// Idle workers harvest.
fn harvest(u: &mut Universe, fleet: &Fleet) {
    let idle_workers: Vec<ShipId> = fleet
        .workers
        .iter()
        .copied()
        .filter(|&w| u.ships.ship(w).command.and_then(|c| u.commands.get(c)).is_none_or(|c| c.order == crate::command::order::MILITARY_PARADE))
        .collect();
    if !idle_workers.is_empty() {
        crate::resource::cl_collect_resource(u, &idle_workers, None);
    }
}

/// Queues ships of `t` at the mothership until the player has (or is building) `n`, whatever
/// the RUs (the build manager charges as it builds).
fn queue_up_to(u: &mut Universe, ms: ShipId, t: ShipType, n: usize) {
    let have = count_type(u, t) + crate::construction::job(u, ms, t).map_or(0, |j| j.n_jobs.max(0) as usize);
    if have < n && crate::level::construction_offers(u, ms, t) {
        crate::construction::queue_build(u, ms, t, 1);
    }
}

/// Queues ships of `t` at the mothership until the player has (or is building) `n`.
fn build_up_to(u: &mut Universe, ms: ShipId, t: ShipType, n: usize) {
    let have = count_type(u, t) + crate::construction::job(u, ms, t).map_or(0, |j| j.n_jobs.max(0) as usize);
    if have >= n || !crate::level::construction_offers(u, ms, t) {
        return;
    }
    let cost = u.statics.ship(t).map_or(0, |s| s.build_cost);
    if u.base.players[HUMAN_PLAYER].resource_units < cost {
        return;
    }
    crate::construction::queue_build(u, ms, t, 1);
}

/// Starts researching `tech` when the research manager offers it and a lab is free.
fn research(u: &mut Universe, tech: &str) {
    if let Some(t) = crate::statics::Technology::from_name(tech) {
        if crate::research::available_topics(u, HUMAN_PLAYER).contains(&t.index()) {
            crate::research::research_tech_if_lab_free(u, HUMAN_PLAYER, t.index());
        }
    }
}

fn under_construction(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0)
}

fn under_construction_any(u: &Universe) -> bool {
    u.ship_list.iter().any(|&id| u.ships.ship(id).player == HUMAN_PLAYER && under_construction(u, id))
}

/// The player's ships of a type in the universe (finished or being built).
fn count_type(u: &Universe, t: ShipType) -> usize {
    u.ship_list.iter().filter(|&&id| u.ships.get(id).is_some_and(|s| s.player == HUMAN_PLAYER && s.ship_type == t)).count()
}
