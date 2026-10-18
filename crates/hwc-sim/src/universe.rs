//! The universe (`Universe.c` / `UnivUpdate.c`): every object, the lists the original keeps
//! them in, players, and the fixed-rate update `univUpdate`.
//!
//! One call to [`Universe::update`] is one `univUpdate(1/16 s)`. The order of work follows
//! Cataclysm's `univUpdate` (0x547410); parts of it that have no counterpart yet (building,
//! research, bullets, missiles, collisions, effects, damage) are skipped and listed in
//! `docs/sim/runtime.md`.

use std::sync::Arc;

use hwc_math::fpu::{ld, st};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Randy, Vector};

use crate::aitrack;
use crate::blobs::BlobList;
use crate::command::{self, CommandLayer};
use crate::formation;
use crate::object::{sof, NonStats, ObjRef, Player, PosInfo, RotInfo, Ship, ShipId, ShipTable};
use crate::physics::{self, PhysContext};
use crate::statics::{ShipRace, ShipType, StaticWorld};
use crate::tactics::MaxVelocityContext;
use crate::tunables::SimTweaks;

/// `defaultshipmatrix` (0x8ab820): up = +Z, right = -Y, heading = +X.
pub const DEFAULT_SHIP_MATRIX: Matrix = Matrix { m: [0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0] };

/// Default half-size of the world (0x888c58..0x888c60) before a level sets it.
pub const DEFAULT_WORLD_BOUNDS: Vector = Vector::new(100_000.0, 100_000.0, 100_000.0);

/// Why a ship could not be created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CreateError {
    /// The race has no statics for this ship type (the original disables such types).
    NoStatics(ShipType),
    NoSuchPlayer(usize),
}

impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CreateError::NoStatics(t) => write!(f, "no statics loaded for {t}"),
            CreateError::NoSuchPlayer(p) => write!(f, "no player {p}"),
        }
    }
}

impl std::error::Error for CreateError {}

/// The simulation state.
#[derive(Debug, Clone)]
pub struct Universe {
    pub statics: Arc<StaticWorld>,
    pub tweaks: SimTweaks,
    pub ships: ShipTable,
    /// `universe.SpaceObjList` (every object, creation order).
    pub space_obj_list: Vec<ObjRef>,
    /// `universe.ShipList`.
    pub ship_list: Vec<ShipId>,
    /// `universe.ImpactableList`.
    pub impactable_list: Vec<ObjRef>,
    /// `universe.MissileList`, `ResourceList`, `DerelictList` (no such objects yet).
    pub missile_list: Vec<ObjRef>,
    pub resource_list: Vec<ObjRef>,
    pub derelict_list: Vec<ObjRef>,
    /// `universe.RenderList`, rebuilt by [`Universe::update_render_list`].
    pub render_list: Vec<ObjRef>,
    pub players: Vec<Player>,
    /// `universe.shipNumber`: the next ship id.
    pub ship_number: u16,
    /// `universe.univUpdateCounter`.
    pub update_counter: u32,
    /// `universe.totaltimeelapsed`.
    pub total_time_elapsed: f32,
    /// `universe.phystimeelapsed`.
    pub phys_time_elapsed: f32,
    pub blobs: BlobList,
    pub commands: CommandLayer,
    pub randy: Randy,
    pub world_bounds: Vector,
    /// Single-player games skip the world-bounds velocity clamp.
    pub single_player: bool,
    /// `aishipTempDisableAvoiding`.
    pub temp_disable_avoiding: bool,
}

impl Universe {
    /// An empty universe with `players` players.
    pub fn new(statics: Arc<StaticWorld>, tweaks: SimTweaks, players: &[ShipRace]) -> Self {
        Self {
            statics,
            tweaks,
            ships: ShipTable::default(),
            space_obj_list: Vec::new(),
            ship_list: Vec::new(),
            impactable_list: Vec::new(),
            missile_list: Vec::new(),
            resource_list: Vec::new(),
            derelict_list: Vec::new(),
            render_list: Vec::new(),
            players: players.iter().enumerate().map(|(i, &r)| Player::new(i, r)).collect(),
            ship_number: 0,
            update_counter: 0,
            total_time_elapsed: 0.0,
            phys_time_elapsed: UNIVERSE_UPDATE_PERIOD,
            blobs: BlobList::default(),
            commands: CommandLayer::default(),
            randy: Randy::default(),
            world_bounds: DEFAULT_WORLD_BOUNDS,
            single_player: false,
            temp_disable_avoiding: false,
        }
    }

    /// `ShipIDtoShip`.
    pub fn ship(&self, id: ShipId) -> Option<&Ship> {
        self.ships.get(id)
    }

    /// The speed-limit inputs for a ship (sphere formation, special factor).
    pub fn max_velocity_context(&self, ship: &Ship) -> MaxVelocityContext {
        let sphere = ship
            .command
            .and_then(|c| self.commands.get(c))
            .is_some_and(|c| c.attributes & command::attr::FORMATION != 0 && c.formation.formation_type == formation::kind::SPHERE);
        MaxVelocityContext { in_sphere_formation: sphere, special_factor: 0.0 }
    }

    /// `univCreateShip` (0x53d3c0) + `univAddShip`: a new ship at `position` with the default
    /// orientation (facing +X), owned by `player`. Optionally oriented to `heading`.
    pub fn add_ship(&mut self, ship_type: ShipType, player: usize, position: Vector, heading: Option<Vector>) -> Result<ShipId, CreateError> {
        let stat = self.statics.ship(ship_type).ok_or(CreateError::NoStatics(ship_type))?;
        if player >= self.players.len() {
            return Err(CreateError::NoSuchPlayer(player));
        }
        let id = ShipId(self.ship_number);
        self.ship_number = self.ship_number.wrapping_add(1);
        let g = &stat.guidance;
        let mut flags = sof::ROTATABLE | sof::IMPACTABLE | sof::SELECTABLE | sof::TARGETABLE;
        if ld(self.tweaks.verybigship_size) < ld(stat.header.coll.coll_sphere_size) {
            flags |= sof::BIG_OBJECT;
        }
        let mut coordsys = DEFAULT_SHIP_MATRIX;
        if let Some(h) = heading {
            coordsys = Matrix::coord_sys_from_heading(h.normalized());
        }
        let mut ship = Ship::new(id, ship_type, stat.ship_race, player);
        ship.flags = flags;
        ship.posinfo = PosInfo { position, havent_calculated_dist: true, ..Default::default() };
        ship.rotinfo = RotInfo { coordsys, ..Default::default() };
        ship.health = stat.health.max_health;
        ship.nonstat = NonStats { thrust_strength: g.thrust_strength, rot_strength: g.rot_strength, turn_speed: g.turn_speed };
        ship.max_velocity_copy = stat.header.max_velocity;
        ship.time_created = self.total_time_elapsed;
        ship.color_scheme = self.players[player].color_scheme;
        physics::update_obj_rot_info(&mut ship, stat);
        self.blobs.add_to_nearest(&mut ship, &self.statics);
        self.ships.insert(ship);
        self.space_obj_list.push(ObjRef::Ship(id));
        self.ship_list.push(id);
        self.impactable_list.push(ObjRef::Ship(id));
        Ok(id)
    }

    /// `univUpdate` (0x547410): one fixed step of `phys_time_elapsed` (1/16 s).
    pub fn update(&mut self) {
        let dt = UNIVERSE_UPDATE_PERIOD;
        self.total_time_elapsed = st(ld(self.total_time_elapsed) + ld(dt));
        self.update_counter = self.update_counter.wrapping_add(1);
        self.phys_time_elapsed = dt;

        // Collision blobs: rebuilt on the first frame and every eighth, refreshed otherwise.
        let statics = self.statics.clone();
        let props = self.tweaks.blob;
        if self.update_counter == 1
            || (self.tweaks.refresh_collblob_rate & self.update_counter) == self.tweaks.refresh_collblob_frame
        {
            let order: Vec<ShipId> = self.space_obj_list.iter().map(|ObjRef::Ship(id)| *id).collect();
            self.blobs.rebuild(&order, &mut self.ships, &statics, &props);
        } else {
            self.blobs.update_objects(&mut self.ships, &statics);
        }

        // (Build manager, research, resource injection, bullets/missiles, crates and the
        // computer players run here in the original.)
        command::cl_process(self);
        // (clPostProcess handles guard/protect retaliation; collision bumps follow.)

        self.update_all_pos_vel_ships();
        // (Effects, dead-object deletion, resource regrowth and bounties follow.)
    }

    /// `univUpdateAllPosVelShips` (0x53ffe0): per ship, the state checks, the physics step and
    /// the post-physics housekeeping (`0x545250`).
    fn update_all_pos_vel_ships(&mut self) {
        let statics = self.statics.clone();
        let list = self.ship_list.clone();
        for id in list {
            let Some(ship) = self.ships.get(id) else { continue };
            if ship.flags & sof::DONT_APPLY_PHYSICS != 0 {
                continue;
            }
            let Some(stat) = statics.ship(ship.ship_type) else { continue };
            self.check_ship_state(id);
            let ctx = PhysContext {
                dt: self.phys_time_elapsed,
                update_counter: self.update_counter,
                single_player: self.single_player,
                world_bounds: self.world_bounds,
                max_vel: self.max_velocity_context(self.ships.ship(id)),
            };
            let tw = &self.tweaks;
            physics::update_ship(self.ships.ship_mut(id), stat, tw, &ctx);
            self.housekeep(id);
        }
    }

    /// The autostabilisation part of `univCheckShipState` (0x545670): ships with nothing to
    /// do level out and stop after `AUTOSTABILIZE_FRAME_KICK_IN` frames.
    fn check_ship_state(&mut self, id: ShipId) {
        let statics = self.statics.clone();
        let tw = self.tweaks.clone();
        let dt = self.phys_time_elapsed;
        let s = self.ships.ship_mut(id);
        if s.flags & sof::DEAD != 0 {
            return;
        }
        let Some(stat) = statics.ship(s.ship_type) else { return };
        if s.autostabilize_ship
            && s.flags & (sof::HIDE | sof::CRAZY | sof::DISABLED | sof::HYPERSPACE) == 0
            && s.flags & sof::NIS_SHIP == 0
        {
            if aitrack::is_ship_steady(s, &tw) {
                aitrack::force_steady_ship(s);
                s.autostabilize_state = 0;
            } else {
                let mut stabilize = false;
                match s.autostabilize_state {
                    0 => {
                        s.autostabilize_wait = tw.autostabilize_frame_kick_in as u8;
                        s.autostabilize_state = 1;
                        if s.autostabilize_wait != 0 {
                            s.autostabilize_wait -= 1;
                        } else {
                            s.autostabilize_state = 2;
                            stabilize = true;
                        }
                    }
                    1 => {
                        if s.autostabilize_wait != 0 {
                            s.autostabilize_wait -= 1;
                        } else {
                            s.autostabilize_state = 2;
                            stabilize = true;
                        }
                    }
                    2 => stabilize = true,
                    _ => {}
                }
                if stabilize {
                    aitrack::steady_ship(s, stat, &tw, dt);
                }
            }
        }
        // (The world-limits recall, proximity retaliation and the death timer are not
        // modelled.)
    }

    /// Post-physics housekeeping (0x545250).
    fn housekeep(&mut self, id: ShipId) {
        let s = self.ships.ship_mut(id);
        if s.flags & sof::DEAD != 0 {
            return;
        }
        s.ship_idle = true;
        s.autostabilize_ship = true;
        s.special_flags &= !crate::object::special::TRANSIENT_8000000;
        s.flags &= !sof::FORMATION_SETTLED;
        // (Recent-attack counters, damage effects and engine trails are updated here.)
    }

    /// `univUpdateRenderList` simplified: every visible ship within `max_dist` of `eye`, in
    /// ship-list order.
    pub fn update_render_list(&mut self, eye: Vector, max_dist: f32) {
        let max_sq = ld(max_dist) * ld(max_dist);
        self.render_list = self
            .ship_list
            .iter()
            .filter(|&&id| {
                self.ships.get(id).is_some_and(|s| {
                    let d = s.posinfo.position - eye;
                    s.flags & sof::HIDE == 0 && d.mag_sq_ext() <= max_sq
                })
            })
            .map(|&id| ObjRef::Ship(id))
            .collect();
    }
}
