//! Determinism harness: scripted scenarios run for N ticks with a hash of the world state after
//! every tick, so two runs (or two builds) can be compared tick by tick.

use std::sync::Arc;

use hwc_math::Vector;

use crate::command;
use crate::formation;
use crate::object::ShipId;
use crate::resource::ResourceKind;
use crate::statics::{ShipRace, ShipType, StaticWorld, Technology};
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
    world_hash_with(u, &HashView::default())
}

/// What [`world_hash_with`] leaves out: some ships (with the numbers of later ships closed up,
/// as if those ships had never been created) and the special-ability state. Used to show that a
/// change of the hash comes only from new state.
#[derive(Debug, Clone, Default)]
pub struct HashView {
    /// Ships not hashed.
    pub skip: Vec<ShipId>,
    /// Leave out missiles, `Ship::spec`, the generic special state and the fog of war.
    pub no_specials: bool,
}

impl HashView {
    fn keep(&self, id: ShipId) -> bool {
        !self.skip.contains(&id)
    }

    fn id(&self, id: ShipId) -> u32 {
        u32::from(id.0) - self.skip.iter().filter(|s| s.0 < id.0).count() as u32
    }

    fn opt(&self, id: Option<ShipId>) -> u32 {
        id.map_or(u32::MAX, |s| self.id(s))
    }
}

/// [`world_hash`] seen through a [`HashView`].
pub fn world_hash_with(u: &Universe, view: &HashView) -> u64 {
    let mut h = Fnv64::default();
    h.u32(u.update_counter);
    h.f32(u.total_time_elapsed);
    for s in u.ships.iter().filter(|s| view.keep(s.id)) {
        h.u32(view.id(s.id));
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
        h.u32(view.opt(s.attack.target));
        h.u32(view.opt(s.attack.leader) | s.attack.wingman.map_or(0xffff, |t| view.id(t)) << 16);
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
        h.u32(view.opt(b.owner));
        h.u32(view.opt(b.target));
    }
    hash_base(&mut h, u, view);
    if !view.no_specials {
        hash_specials(&mut h, u);
    }
    h.u32(u.randy.number_get(hwc_math::random::RAN_GAME));
    for c in u.commands.iter() {
        h.u32(c.id.0);
        h.u32(u32::from(c.order) | u32::from(c.attributes) << 16);
        for &id in &c.selection {
            h.u32(view.id(id));
        }
        h.u32(c.waypoint_index as u32);
        h.f32(c.formation.travel_vel);
    }
    h.finish()
}

/// Missiles, mines and the special-ability state (only what exists is hashed, so scenarios
/// without them hash as before).
fn hash_specials(h: &mut Fnv64, u: &Universe) {
    for &mid in &u.missile_list {
        let m = u.missiles.missile(mid);
        h.u32(mid.0 | m.kind << 31);
        h.u32(m.flags);
        h.vector(m.posinfo.position);
        h.vector(m.posinfo.velocity);
        h.f32(m.time_lived);
        h.f32(m.total_lifetime);
        h.f32(m.health);
        h.u32(m.target.map_or(u32::MAX, |t| u32::from(t.0)) | m.mine_state << 16);
    }
    for s in u.ships.iter() {
        let vis = s.visible_to.iter().enumerate().fold(0u32, |a, (i, &v)| a | (u32::from(!v) << i));
        let generic = s.disable.disabled || s.disable.slowed || s.disable.emp_count != 0 || vis != 0 || s.beast.timer != 0.0 || s.queued_attack.is_some() || s.flicker.state != 0;
        if generic {
            h.u32(u32::from(s.id.0));
            h.u32(u32::from(s.disable.disabled) | u32::from(s.disable.slowed) << 1 | vis << 8);
            h.f32(s.disable.timer.duration);
            h.f32(s.beast.timer);
        }
        if s.spec != crate::ships::ShipSpec::None {
            h.u32(u32::from(s.id.0));
            h.bytes(format!("{:?}", s.spec).as_bytes());
        }
    }
    for g in u.fog.grids.iter().flatten() {
        for c in &g.cells {
            h.u32(c.viewers as u32 | u32::from(c.explored) << 31);
        }
    }
}

/// The base layer's state: docking, clamping, cargo and construction per ship, players'
/// resources and research, factory jobs and the resources.
fn hash_base(h: &mut Fnv64, u: &Universe, view: &HashView) {
    let opt = |s: Option<ShipId>| view.opt(s);
    for s in u.ships.iter().filter(|s| view.keep(s.id)) {
        let b = &s.base;
        h.u32(view.id(s.id));
        h.u32(b.dock_state | (b.dock_point as u32) << 8);
        h.u32(opt(b.dock_with));
        h.f32(b.dock_dist);
        h.f32(b.dock_timer);
        h.u32(opt(b.clamp.as_ref().and_then(|c| c.host)));
        h.u32(b.resources as u32);
        h.u32(u32::from(b.harvest_state) | b.salvage.state << 8 | b.link.state << 16);
        h.f32(b.build_progress);
        h.u32(s.special_flags2);
    }
    for p in &u.base.players {
        h.u32(p.resource_units as u32);
        h.u32(p.su_used as u32 | (p.su_capacity as u32) << 16);
        for t in &p.topics {
            h.u32(t.tech as u32);
            h.f32(t.time_left);
        }
        for l in &p.labs {
            h.u32(l.status as u32);
        }
        h.u32(p.tech.has.iter().enumerate().fold(0u32, |a, (i, &v)| a ^ ((v as u32) << (i % 32))));
    }
    for f in &u.base.factories {
        h.u32(opt(f.ship));
        for j in f.jobs.iter().filter(|j| j.n_jobs > 0) {
            h.u32(j.n_jobs as u32);
            h.u32(j.time_left as u32);
            h.u32(j.cost_frac as u32);
        }
    }
    for r in &u.base.resources.list {
        h.u32(u32::from(r.id.0));
        h.vector(r.position);
        h.u32(r.resource_value as u32);
    }
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
    /// Queue `count` ships of a type at a factory (as the build manager does).
    Build { factory: usize, ship_type: ShipType, count: i32 },
    /// The ships harvest (the nearest resource).
    Collect { ships: Vec<usize> },
    /// The player starts researching a technology.
    Research { player: usize, tech: Technology },
    /// Ship `a` links with ship `b`.
    Link { a: usize, b: usize },
    /// The salvagers salvage the targets.
    Salvage { ships: Vec<usize>, targets: Vec<usize> },
    /// The ships dock (with a given ship, or the nearest) for a purpose (`dock::purpose`).
    Dock { ships: Vec<usize>, purpose: i32, with: Option<usize> },
    /// The ships use their special ability (`clWrapSpecial`), on the targets if any.
    Special { ships: Vec<usize>, targets: Vec<usize> },
    /// A level mine (`Mine` keyword) at a position.
    Mine { position: Vector },
    /// Make a player a computer player (auto behaviour), with a difficulty and primary enemy.
    ComputerPlayer { player: usize, difficulty: i32, primary_enemy: Option<usize> },
    /// Turn on fog of war for a (local) player.
    FogOfWar { player: usize },
    /// Set the ships' health to a fraction of their maximum (as KAS `SetHealth` does), to stage
    /// deaths quickly.
    SetHealth { ships: Vec<usize>, fraction: f32 },
}

/// A resource object to place.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSpawn {
    pub kind: ResourceKind,
    pub position: Vector,
}

/// A scenario: the races of the players, the ships and the orders by tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Scenario {
    pub name: &'static str,
    pub players: Vec<ShipRace>,
    pub spawns: Vec<Spawn>,
    /// `(tick, order)`: the order is issued before tick `tick` runs (tick 0 = first update).
    pub orders: Vec<(u32, Order)>,
    /// Resources placed after the ships.
    pub resources: Vec<ResourceSpawn>,
    /// Starting resource units per player (missing entries: 0).
    pub resource_units: Vec<i32>,
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
        for (p, &ru) in scenario.resource_units.iter().enumerate() {
            if let Some(pl) = universe.base.players.get_mut(p) {
                pl.resource_units = ru;
            }
        }
        let mut ids = Vec::new();
        for s in &scenario.spawns {
            ids.push(universe.add_ship(s.ship_type, s.player, s.position, None)?);
        }
        for r in &scenario.resources {
            crate::resource::add_resource(&mut universe, r.kind, r.position);
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
            Order::Build { factory, ship_type, count } => {
                if let Some(&f) = sel(&[*factory]).first() {
                    crate::construction::queue_build(&mut self.universe, f, *ship_type, *count);
                }
            }
            Order::Collect { ships } => {
                let s = sel(ships);
                crate::resource::cl_collect_resource(&mut self.universe, &s, None);
            }
            Order::Research { player, tech } => {
                crate::research::research_tech_if_lab_free(&mut self.universe, *player, tech.index());
            }
            Order::Link { a, b } => {
                if let (Some(&a), Some(&b)) = (sel(&[*a]).first(), sel(&[*b]).first()) {
                    crate::link::cl_link(&mut self.universe, a, b);
                }
            }
            Order::Salvage { ships, targets } => {
                let (s, t) = (sel(ships), sel(targets));
                crate::salvage::cl_salvage(&mut self.universe, &s, &t);
            }
            Order::Dock { ships, purpose, with } => {
                let s = sel(ships);
                let w = with.and_then(|w| sel(&[w]).first().copied());
                crate::dock::cl_dock(&mut self.universe, &s, *purpose, w);
            }
            Order::Special { ships, targets } => {
                let (s, t) = (sel(ships), sel(targets));
                crate::ships::cl_special(&mut self.universe, &s, &t);
            }
            Order::Mine { position } => {
                crate::missile::add_level_mine(&mut self.universe, *position);
            }
            Order::ComputerPlayer { player, difficulty, primary_enemy } => {
                if let Some(p) = self.universe.base.players.get_mut(*player) {
                    p.ai = true;
                    p.ai_difficulty = *difficulty;
                    p.ai_primary_enemy = *primary_enemy;
                }
            }
            Order::SetHealth { ships, fraction } => {
                for id in sel(ships) {
                    let t = self.universe.ships.ship(id).ship_type;
                    let max = self.universe.statics.ship(t).map_or(0.0, |st| st.health.max_health);
                    self.universe.ships.ship_mut(id).health = hwc_math::fpu::st(hwc_math::fpu::ld(max) * hwc_math::fpu::ld(*fraction));
                }
            }
            Order::FogOfWar { player } => {
                crate::fow::create_for_local_player(&mut self.universe, *player);
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
        Scenario { name, players: vec![ShipRace::Somtaaw], spawns, orders: extra, resources: Vec::new(), resource_units: Vec::new() }
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
                resources: Vec::new(),
                resource_units: Vec::new(),
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
                resources: Vec::new(),
                resource_units: Vec::new(),
            })
        }
        // Three multi-beam frigates finish off a badly damaged Taiidan frigate.
        "frigate_death" => Some(Scenario {
            name: "frigate_death",
            players: vec![ShipRace::Somtaaw, ShipRace::Taiidan],
            spawns: vec![
                Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-1800.0, -500.0, 0.0) },
                Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-1800.0, 500.0, 0.0) },
                Spawn { ship_type: tStandardFrigate, player: 1, position: v(600.0, 0.0, 0.0) },
            ],
            orders: vec![
                (0, Order::Tactics { ships: vec![2], tactics: crate::object::tactics::NEUTRAL }),
                (0, Order::SetHealth { ships: vec![2], fraction: 0.12 }),
                (1, Order::Attack { ships: vec![0, 1], targets: vec![2] }),
            ],
            resources: Vec::new(),
            resource_units: Vec::new(),
        }),
        // Frigates finish off a nearly dead Beast mothership: a big death.
        "big_death" => Some(Scenario {
            name: "big_death",
            players: vec![ShipRace::Somtaaw, ShipRace::Beast],
            spawns: vec![
                Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-4000.0, -600.0, 0.0) },
                Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-4000.0, 600.0, 0.0) },
                Spawn { ship_type: sMultiBeamFrigate, player: 0, position: v(-4200.0, 0.0, 300.0) },
                Spawn { ship_type: bMothership, player: 1, position: v(1500.0, 0.0, 0.0) },
            ],
            orders: vec![
                (0, Order::Tactics { ships: vec![3], tactics: crate::object::tactics::NEUTRAL }),
                (0, Order::SetHealth { ships: vec![3], fraction: 0.004 }),
                (1, Order::Attack { ships: vec![0, 1, 2], targets: vec![3] }),
            ],
            resources: Vec::new(),
            resource_units: Vec::new(),
        }),
        // A Somtaaw base: the mothership (with its starting modules), four workers harvesting an
        // asteroid field, a docking bay and two recon ships queued at the mothership.
        "base" => {
            let mut spawns = vec![Spawn { ship_type: sMothership, player: 0, position: v(0.0, 0.0, 0.0) }];
            for i in 0..4 {
                spawns.push(Spawn { ship_type: sWorker, player: 0, position: v(1500.0, -900.0 + i as f32 * 600.0, 400.0) });
            }
            let mut resources = Vec::new();
            for i in 0..10 {
                let k = i as f32;
                resources.push(ResourceSpawn {
                    kind: ResourceKind::Asteroid(1 + (i % 3) as u8),
                    position: v(9000.0 + (k * 1.7).sin() * 1500.0, -2500.0 + k * 550.0, (k * 2.3).cos() * 800.0),
                });
            }
            Some(Scenario {
                name: "base",
                players: vec![ShipRace::Somtaaw],
                spawns,
                orders: vec![
                    (0, Order::Collect { ships: vec![1, 2, 3, 4] }),
                    (1, Order::Build { factory: 0, ship_type: sMothershipDockingBay, count: 1 }),
                    (2, Order::Build { factory: 0, ship_type: sRecon, count: 2 }),
                ],
                resources,
                resource_units: vec![3000],
            })
        }
        _ => None,
    }
}

/// Names of the built-in scenarios.
pub const SCENARIOS: [&str; 6] = ["fleet", "idle", "patrol", "dogfight", "frigates", "base"];
