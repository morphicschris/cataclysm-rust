//! Determinism harness: scripted scenarios run for N ticks with a hash of the world state after
//! every tick, so two runs (or two builds) can be compared tick by tick.

use std::sync::Arc;

use hwc_math::Vector;

use crate::command;
use crate::formation;
use crate::object::ShipId;
use crate::statics::{ShipRace, ShipType, StaticWorld};
use crate::tunables::SimTweaks;
use crate::universe::{CreateError, Universe};

/// FNV-1a over 64 bits (fixed, platform independent).
#[derive(Debug, Clone, Copy)]
pub struct Fnv64(u64);

impl Default for Fnv64 {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv64 {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= u64::from(x);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn u32(&mut self, v: u32) {
        self.bytes(&v.to_le_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.u32(v.to_bits());
    }

    pub fn vector(&mut self, v: Vector) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Hash of everything the movement simulation owns (exact bit patterns).
pub fn world_hash(u: &Universe) -> u64 {
    let mut h = Fnv64::default();
    h.u32(u.update_counter);
    h.f32(u.total_time_elapsed);
    for s in u.ships.iter() {
        h.u32(u32::from(s.id.0));
        h.u32(s.flags);
        h.vector(s.posinfo.position);
        h.vector(s.posinfo.velocity);
        h.u32(u32::from(s.posinfo.is_moving));
        for m in s.rotinfo.coordsys.m {
            h.f32(m);
        }
        h.vector(s.rotinfo.rotspeed);
        h.vector(s.coll.coll_position);
        h.f32(s.coll_optimize_dist);
        h.u32(s.coll_my_blob.map_or(u32::MAX, |b| b.0));
        h.u32(u32::from(s.ai_state) | u32::from(s.ai_state_command) << 8 | u32::from(s.autostabilize_state) << 16);
        h.f32(s.ai_descend);
        h.vector(s.move_to);
        h.vector(s.formation_offset);
        h.u32(s.command.map_or(u32::MAX, |c| c.0));
        // Combat state.
        h.f32(s.health);
        h.vector(s.rotinfo.torque);
        h.u32(u32::from(s.ai_state_attack) | (s.tactics as u32) << 8 | s.flightman << 16);
        h.u32(s.attack.target.map_or(u32::MAX, |t| u32::from(t.0)));
        h.u32(s.attack.leader.map_or(u32::MAX, |t| u32::from(t.0)) | s.attack.wingman.map_or(0xffff, |t| u32::from(t.0)) << 16);
        h.u32(u32::from(s.attack.flightmans_left) | u32::from(s.attack.situation) << 16);
        h.u32(s.speed_upgrade_level | u32::from(s.recently_attacked) << 8 | u32::from(s.row_state) << 16);
        h.vector(s.interceptor.ai_vec);
        h.u32(u32::from(s.interceptor.active_gun));
        for g in &s.guns {
            h.f32(g.last_time_fired);
            h.vector(g.heading);
        }
    }
    for &b in &u.bullet_list {
        let b = u.bullets.bullet(b);
        h.u32(b.id.0);
        h.vector(b.posinfo.position);
        h.vector(b.heading);
        h.f32(b.time_lived);
        h.f32(b.beam_travel_dist);
        h.u32(b.owner.map_or(u32::MAX, |o| u32::from(o.0)));
        h.u32(b.target.map_or(u32::MAX, |o| u32::from(o.0)));
    }
    h.u32(u.randy.number_get(hwc_math::random::RAN_GAME));
    for c in u.commands.iter() {
        h.u32(c.id.0);
        h.u32(u32::from(c.order) | u32::from(c.attributes) << 16);
        for id in &c.selection {
            h.u32(u32::from(id.0));
        }
        h.u32(c.waypoint_index as u32);
        h.f32(c.formation.travel_vel);
    }
    h.finish()
}

/// One ship to create at the start.
#[derive(Debug, Clone, PartialEq)]
pub struct Spawn {
    pub ship_type: ShipType,
    pub player: usize,
    pub position: Vector,
}

/// A scripted order.
#[derive(Debug, Clone, PartialEq)]
pub enum Order {
    /// Move the ships (by spawn index) through `waypoints`; `from` is where the order was
    /// given from (the original passes the selection's centre).
    Move { ships: Vec<usize>, mode: u32, waypoints: Vec<Vector> },
    Formation { ships: Vec<usize>, formation_type: u32 },
    Halt { ships: Vec<usize> },
    /// The ships attack the targets (all by spawn index).
    Attack { ships: Vec<usize>, targets: Vec<usize> },
    /// Set the tactics (`object::tactics`) of the ships.
    Tactics { ships: Vec<usize>, tactics: usize },
}

/// A scenario: the races of the players, the ships and the orders by tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: &'static str,
    pub players: Vec<ShipRace>,
    pub spawns: Vec<Spawn>,
    /// `(tick, order)`: the order is issued before tick `tick` runs (tick 0 = first update).
    pub orders: Vec<(u32, Order)>,
}

/// The centre of a group (what the move pie plate uses as "from").
pub fn group_centre(u: &Universe, ships: &[ShipId]) -> Vector {
    let mut c = Vector::ZERO;
    for &id in ships {
        c = c + u.ships.ship(id).posinfo.position;
    }
    let k = 1.0 / ships.len().max(1) as f32;
    Vector::new(c.x * k, c.y * k, c.z * k)
}

/// A running scenario.
pub struct Run {
    pub universe: Universe,
    pub ids: Vec<ShipId>,
    scenario: Scenario,
    tick: u32,
}

impl Run {
    pub fn new(statics: Arc<StaticWorld>, tweaks: SimTweaks, scenario: Scenario) -> Result<Self, CreateError> {
        let mut universe = Universe::new(statics, tweaks, &scenario.players);
        let mut ids = Vec::new();
        for s in &scenario.spawns {
            ids.push(universe.add_ship(s.ship_type, s.player, s.position, None)?);
        }
        Ok(Self { universe, ids, scenario, tick: 0 })
    }

    pub fn tick(&self) -> u32 {
        self.tick
    }

    /// Issues the orders due now and runs one universe update.
    pub fn step(&mut self) -> u64 {
        let due: Vec<Order> = self.scenario.orders.iter().filter(|(t, _)| *t == self.tick).map(|(_, o)| o.clone()).collect();
        for order in due {
            self.issue(&order);
        }
        self.universe.update();
        self.tick += 1;
        world_hash(&self.universe)
    }

    pub fn issue(&mut self, order: &Order) {
        // Ships that have died are left out.
        let sel = |v: &[usize]| v.iter().map(|&i| self.ids[i]).filter(|&id| self.universe.ships.get(id).is_some()).collect::<Vec<_>>();
        match order {
            Order::Move { ships, mode, waypoints } => {
                let s = sel(ships);
                let from = group_centre(&self.universe, &s);
                command::cl_move(&mut self.universe, &s, *mode, from, waypoints);
            }
            Order::Formation { ships, formation_type } => {
                let s = sel(ships);
                command::cl_formation(&mut self.universe, &s, *formation_type);
            }
            Order::Halt { ships } => {
                let s = sel(ships);
                command::cl_halt(&mut self.universe, &s);
            }
            Order::Attack { ships, targets } => {
                let (s, t) = (sel(ships), sel(targets));
                command::cl_attack(&mut self.universe, &s, &t);
            }
            Order::Tactics { ships, tactics } => {
                let s = sel(ships);
                command::cl_set_tactics(&mut self.universe, &s, *tactics);
            }
        }
    }

    /// Runs `ticks` updates and returns the hash after each.
    pub fn run(&mut self, ticks: u32) -> Vec<u64> {
        (0..ticks).map(|_| self.step()).collect()
    }
}

/// Built-in scenarios (also used by the viewer's sandbox).
pub fn scenario(name: &str) -> Option<Scenario> {
    use ShipType::*;
    let v = Vector::new;
    let fleet = |extra: Vec<(u32, Order)>, name: &'static str| {
        let mut spawns = vec![Spawn { ship_type: sMothership, player: 0, position: v(0.0, 0.0, 0.0) }];
        for i in 0..6 {
            let row = (i / 3) as f32;
            let col = (i % 3) as f32 - 1.0;
            spawns.push(Spawn { ship_type: sAcolyte, player: 0, position: v(2500.0 + row * 120.0, col * 150.0, 300.0) });
        }
        for (i, t) in [sMultiBeamFrigate, sHiveFrigate, sRammingFrigate].into_iter().enumerate() {
            spawns.push(Spawn { ship_type: t, player: 0, position: v(1500.0, -1200.0 + i as f32 * 700.0, -200.0) });
        }
        Scenario { name, players: vec![ShipRace::Somtaaw], spawns, orders: extra }
    };
    let acolytes: Vec<usize> = (1..=6).collect();
    let frigates: Vec<usize> = (7..=9).collect();
    match name {
        // Ships sitting still: auto-stabilisation only.
        "idle" => Some(fleet(Vec::new(), "idle")),
        // Acolytes in delta formation fly out and back; frigates move individually.
        "fleet" | "default" => Some(fleet(
            vec![
                (0, Order::Formation { ships: acolytes.clone(), formation_type: formation::kind::DELTA }),
                (2, Order::Move { ships: acolytes.clone(), mode: 0, waypoints: vec![v(6000.0, 2500.0, 600.0)] }),
                (4, Order::Move { ships: frigates.clone(), mode: 0, waypoints: vec![v(-3000.0, -2500.0, -400.0)] }),
                (160, Order::Formation { ships: acolytes.clone(), formation_type: formation::kind::BROAD }),
                (240, Order::Move { ships: acolytes, mode: 0, waypoints: vec![v(1000.0, -3000.0, 0.0)] }),
            ],
            "fleet",
        )),
        // A frigate patrolling between three waypoints.
        "patrol" => Some(fleet(
            vec![(0, Order::Move {
                ships: vec![7],
                mode: command::waypoint_mode::PATROL,
                waypoints: vec![v(3000.0, 0.0, 0.0), v(3000.0, 3000.0, 0.0), v(0.0, 3000.0, 1000.0)],
            })],
            "patrol",
        )),
        // Six Acolytes attack six Taiidan heavy interceptors, which fight back (aggressive).
        "dogfight" => {
            let mut spawns = Vec::new();
            for i in 0..6 {
                let (row, col) = ((i / 3) as f32, (i % 3) as f32 - 1.0);
                spawns.push(Spawn { ship_type: sAcolyte, player: 0, position: v(-2500.0 - row * 150.0, col * 200.0, 100.0) });
            }
            for i in 0..6 {
                let (row, col) = ((i / 3) as f32, (i % 3) as f32 - 1.0);
                spawns.push(Spawn { ship_type: tHeavyInterceptor, player: 1, position: v(2500.0 + row * 150.0, col * 200.0, -100.0) });
            }
            let ours: Vec<usize> = (0..6).collect();
            let theirs: Vec<usize> = (6..12).collect();
            Some(Scenario {
                name: "dogfight",
                players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
                spawns,
                orders: vec![
                    (0, Order::Tactics { ships: theirs.clone(), tactics: crate::object::tactics::AGGRESSIVE }),
                    (0, Order::Tactics { ships: ours.clone(), tactics: crate::object::tactics::AGGRESSIVE }),
                    (1, Order::Attack { ships: ours, targets: theirs }),
                ],
            })
        }
        // Three multi-beam frigates attack a Taiidan destroyer, which fires back (neutral).
        "frigates" => {
            let mut spawns = Vec::new();
            for i in 0..3 {
                spawns.push(Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-3500.0, (i as f32 - 1.0) * 600.0, 0.0) });
            }
            spawns.push(Spawn { ship_type: tStandardDestroyer, player: 1, position: v(2000.0, 0.0, 0.0) });
            Some(Scenario {
                name: "frigates",
                players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
                spawns,
                orders: vec![
                    (0, Order::Tactics { ships: vec![3], tactics: crate::object::tactics::NEUTRAL }),
                    (1, Order::Attack { ships: vec![0, 1, 2], targets: vec![3] }),
                ],
            })
        }
        _ => None,
    }
}

/// Names of the built-in scenarios.
pub const SCENARIOS: [&str; 5] = ["fleet", "idle", "patrol", "dogfight", "frigates"];
