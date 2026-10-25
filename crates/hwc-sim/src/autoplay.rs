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
    marks: Vec<&'static str>,
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
        }
    }

    fn note(&mut self, now: u32, text: impl Into<String>) {
        self.notes.push((now, text.into()));
    }

    /// Records `mark` once; true the first time.
    fn once(&mut self, mark: &'static str) -> bool {
        if self.marks.contains(&mark) {
            return false;
        }
        self.marks.push(mark);
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
                    Some(format!("{:?} hp {:.0} p{} att {:?} flags {:x} flags2 {:x} pos {:?} vel {:?} cmd {:?} tac {} dead {}", s.ship_type, s.health, s.player, att, s.flags, s.flags2, s.posinfo.position, s.posinfo.velocity, s.command.and_then(|c| run.universe.commands.get(c)).map(|c| (c.order, c.attributes, c.selection.len(), c.waypoints.clone())), s.tactics, run.universe.ship_list.contains(&id)))
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
                        "ship {} {:?} p{} hp {:.0} pos ({:.0},{:.0},{:.0}) idle {} target {:?} cmd {:?}",
                        id.0, s.ship_type, s.player, s.health, s.posinfo.position.x, s.posinfo.position.y, s.posinfo.position.z, s.ship_idle, s.attack.target,
                        c.map(|c| (c.order, c.attributes, c.attack_targets.clone(), c.waypoints.clone()))
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
                // A player gives the fleet a minute to dock, then goes.
                if run.game.hyper.rollcall_remaining == 0 || now.saturating_sub(t) >= 16 * 60 {
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

    /// Before jumping a player brings the strike craft home (they dock in the roll call or
    /// instantly when the mission has none) and waits for builds to finish.
    fn ready_to_jump(&mut self, run: &mut MissionRun) -> bool {
        let u = &run.universe;
        !under_construction_any(u)
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
        } else if run.game.objective("DestroyResourcers").is_some() {
            Some("TaiidanResourcers")
        } else if run.game.objective("DestroyProxAttackers").is_some() {
            ["TaiidanProxAttackAbomb1", "TaiidanProxAttackMGCorv1", "TaiidanProxAttackHInt1"].into_iter().find(|t| !team_ships(run, t).is_empty())
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
                strike(&mut run.universe, &fleet.combat, &ships, 8);
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
            move_ships(&mut run.universe, &fleet.recons, cp);
            if self.once("m3 find") {
                self.note(now, "M3: recons fly to the Clee-San");
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
            // The Clee-San comes once the mothership is near it: bring the mothership over.
            let d = (cp - mp).mag_sq();
            if d > 5000.0 * 5000.0 && idle(&run.universe, ms) {
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
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 16);
        if run.game.objective("BuildEngineering").is_some() {
            build_up_to(&mut run.universe, ms, ShipType::sMothershipSpecial, 1);
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
        // The Caal-Shto is helpless: half the fleet guards it; the rest goes for the heavy
        // cruiser first (it infects what it reaches), then for the rest of the Beast.
        let caal = team_ships(run, "CaalShto");
        let protecting = !caal.is_empty() && (run.game.objective("ProtectCaalShto").is_some() || run.game.objective("DestroyBeast").is_some());
        // While the Beast is near the Caal-Shto the whole fleet guards it.
        let threatened = protecting && {
            let cp = run.universe.ships.ship(caal[0]).posinfo.position;
            enemies(&run.universe).iter().any(|&e| (run.universe.ships.ship(e).posinfo.position - cp).mag_sq() < 12000.0 * 12000.0)
        };
        if protecting {
            let half = if threatened { fleet.combat.len() } else { fleet.combat.len() / 2 };
            guard(&mut run.universe, &fleet.combat[..half], &caal);
            if std::env::var_os("HWC_DEBUG_GUARDS").is_some() {
                let u = &run.universe;
                let cp = u.ships.ship(caal[0]).posinfo.position;
                let g: Vec<String> = fleet.combat[..half].iter().map(|&id| {
                    let s = u.ships.ship(id);
                    let c = s.command.and_then(|c| u.commands.get(c));
                    format!("{}:{:?} d{:.0} {:?}", id.0, s.ship_type, (s.posinfo.position - cp).mag_sq().sqrt(), c.map(|c| (c.order, c.attributes, c.attack_targets.len())))
                }).collect();
                let near: Vec<String> = u.ship_list.iter().filter(|&&e| u.ships.ship(e).player != HUMAN_PLAYER && (u.ships.ship(e).posinfo.position - cp).mag_sq() < 6000.0 * 6000.0).map(|&e| format!("{}:{:?}", e.0, u.ships.ship(e).ship_type)).collect();
                let rocked = u.ships.ship(caal[0]).getting_rocked;
                self.note(now, format!("M5 guards {g:?} enemies near {near:?} rocked {rocked:?}"));
            }
        }
        if threatened {
            return;
        }
        if run.game.objective("DestroyBeast").is_some() {
            let rest: Vec<ShipId> = fleet.combat[fleet.combat.len() / 2..].to_vec();
            let cruiser = team_ships(run, "bHCruise1");
            if !cruiser.is_empty() {
                strike(&mut run.universe, &rest, &cruiser, 4);
                if self.once("m5 cruiser") {
                    self.note(now, "M5: strike on the Beast heavy cruiser");
                }
            } else {
                let f = Fleet { combat: rest, ..fleet.clone() };
                attack_near(&mut run.universe, &f, mp, f32::MAX);
            }
            return;
        }
        // Until the Bentusi is gone, the fleet stays home (the blast clears 22.5 km around it).
        defend(&mut run.universe, fleet, mp, 15000.0);
    }

    // ---- Mission 6: Kadiir nebula -------------------------------------------------------

    fn mission6(&mut self, run: &mut MissionRun, fleet: &Fleet, ms: ShipId) {
        let now = run.universe.update_counter;
        let mp = run.universe.ships.ship(ms).posinfo.position;
        build_up_to(&mut run.universe, ms, ShipType::sRecon, 5);
        build_up_to(&mut run.universe, ms, ShipType::sWorker, 3);
        build_up_to(&mut run.universe, ms, ShipType::sAcolyte, 14);
        let point = |run: &MissionRun, n: &str| run.game.level.point(kas_hash(n)).map(|p| Vector::new(p[0], p[1], p[2]));
        // Explore the station with a recon; then hold a recon at each nav point.
        if run.game.objective("Explore").is_some() {
            if let (Some(&r), Some(&st_)) = (fleet.recons.first(), team_ships(run, "TStation").first()) {
                let p = run.universe.ships.ship(st_).posinfo.position;
                move_ships(&mut run.universe, &[r], p);
                if self.once("m6 explore") {
                    self.note(now, "M6: recon explores the Taiidan base");
                }
            }
        }
        let navs: Vec<Vector> = ["Nav1", "Nav2", "Nav3", "Nav4", "Nav5"].iter().filter_map(|n| point(run, n)).collect();
        if run.var(None, "G_discovered") == Some(1) || run.game.objective("Scout").is_some() {
            for (k, &r) in fleet.recons.iter().skip(1).enumerate().take(4) {
                if let Some(&to) = navs.get(k) {
                    if (run.universe.ships.ship(r).posinfo.position - to).mag_sq() > 3000.0 * 3000.0 && idle(&run.universe, r) {
                        move_ships(&mut run.universe, &[r], to);
                    }
                }
            }
        }
        if run.game.objective("Escort").is_some() {
            if let Some(&c) = team_ships(run, "CaalShto").first() {
                let cp = run.universe.ships.ship(c).posinfo.position;
                if std::env::var_os("HWC_DEBUG_GUARDS").is_some() {
                    let u = &run.universe;
                    let near = |p: usize, r: f32| u.ship_list.iter().filter(|&&e| u.ships.ship(e).player == p && (u.ships.ship(e).posinfo.position - cp).mag_sq() < r * r).count();
                    let ms_d = (mp - cp).mag_sq().sqrt();
                    let busy = fleet.combat.iter().filter(|&&id| !idle(u, id)).count();
                    self.note(now, format!("M6 caal hp {:.0} combat {} busy {busy} ours<6k {} theirs<6k {} ms d {ms_d:.0} recons {}", u.ships.ship(c).health, fleet.combat.len(), near(HUMAN_PLAYER, 6000.0), near(1, 6000.0), fleet.recons.len()));
                }
                if (cp - mp).mag_sq() > 4000.0 * 4000.0 {
                    move_ships(&mut run.universe, &[ms], cp);
                }
                attack_near(&mut run.universe, fleet, cp, 18000.0);
                return;
            }
        }
        defend(&mut run.universe, fleet, mp, 18000.0);
    }
}

/// The player's ships by role.
#[derive(Debug, Clone, Default)]
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

fn idle(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.command.and_then(|c| u.commands.get(c)).is_none_or(|c| c.order == crate::command::order::NULL && c.attributes & crate::command::attr::PROTECTING == 0))
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
    let idle_workers: Vec<ShipId> = fleet.workers.iter().copied().filter(|&w| u.ships.ship(w).command.is_none()).collect();
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

/// Starts researching `tech` when a lab is free.
fn research(u: &mut Universe, tech: &str) {
    if let Some(t) = crate::statics::Technology::from_name(tech) {
        if !u.base.players[HUMAN_PLAYER].tech.contains(t.index()) {
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
