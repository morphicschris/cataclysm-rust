//! The universe (`Universe.c` / `UnivUpdate.c`): every object, the lists the original keeps
//! them in, players, and the fixed-rate update `univUpdate`.
//!
//! One call to [`Universe::update`] is one `univUpdate(1/16 s)`. The order of work follows
//! Cataclysm's `univUpdate` (0x547410): collision blobs, bullet collisions, the command layer,
//! bump collisions, bullets, ships (state checks, physics, housekeeping), then the dead ships
//! are removed. Parts without a counterpart yet (building, research, missiles, effects) are
//! listed in `docs/sim/runtime.md`.

use std::sync::Arc;

use hwc_math::fpu::{ld, st};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Randy, Vector};

use crate::aitrack;
use crate::blobs::{BlobList, Objects};
use crate::combat_statics::CombatStatics;
use crate::command::{self, CommandLayer};
use crate::formation;
use crate::object::{sof, BulletId, BulletTable, MissileId, ObjRef, Player, Ship, ShipId, ShipTable};
use crate::physics::{self, PhysContext};
use crate::statics::{ShipRace, ShipType, StaticWorld};
use crate::tactics::MaxVelocityContext;
use crate::tunables::SimTweaks;

/// `defaultshipmatrix` (0x8ab820): up = +Z, right = -Y, heading = +X.
pub const DEFAULT_SHIP_MATRIX: Matrix = Matrix { m: [0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0] };

/// Default half-size of the world (0x888c58..0x888c60) before a level sets it.
pub const DEFAULT_WORLD_BOUNDS: Vector = Vector::new(100_000.0, 100_000.0, 100_000.0);

/// Why a ship could not be created.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
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

/// Things that happened during an update, for presentation (effects, sounds). The simulation
/// itself never reads them; they are cleared at the start of every update.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SimEvent {
    /// A gun fired a bullet or beam.
    GunFired { ship: ShipId, gun: usize, bullet: BulletId },
    /// A bullet or beam hit a ship at `position` (a beam hits every frame it touches).
    BulletHit { bullet: BulletId, ship: ShipId, position: Vector, beam: bool, bullet_type: u32 },
    /// A ship died (its explosion; it is removed at the end of the update).
    ShipDied { ship: ShipId, ship_type: ShipType, position: Vector, radius: f32, explosion_type: Option<String> },
    /// A ship gained a veterancy level.
    Upgraded { ship: ShipId, level: u32 },

}

/// Events of the missile, special-ability, cloak and fog code (kept apart from [`SimEvent`] so
/// presentation code that matches every `SimEvent` keeps compiling); cleared every update.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SpecialEvent {
    /// A launcher fired a missile or dropped a mine.
    MissileFired { ship: ShipId, gun: usize, missile: MissileId },
    /// A missile hit a ship or a mine at `position`.
    MissileHit { missile: MissileId, target: ObjRef, position: Vector },
    /// A missile or mine is gone (`exploded`: it blew up rather than running out of life).
    MissileDied { missile: MissileId, position: Vector, exploded: bool, mine: bool },
    /// A cloaker finished cloaking or decloaking.
    Cloaked { ship: ShipId },
    Decloaked { ship: ShipId },
    /// A stealthy ship became visible to a player ("enemy ship sighted").
    EnemyShipSighted { ship: ShipId, player: usize },
    /// The local player's ship `by` discovered an enemy in unexplored space.
    EnemyDiscovered { by: ShipId, ship: ShipId },
    /// A ship-module event for presentation (`ships::Effect`: EMP blasts, field hits, beams...).
    Special { ship: ShipId, effect: crate::ships::Effect },
    /// `soundEventPlay(ship, event, 0)`. (Saved games hold these events: new ones go last.)
    ShipSound { ship: ShipId, event: u32 },
}

/// Game-mode work that runs inside [`Universe::update_with`] (`singlePlayerGameUpdate` and the
/// computer players / KAS in `univUpdate`).
pub trait UpdateHooks {
    /// After the collision blobs, before bullet collisions (`singlePlayerGameUpdate`).
    fn before_bullet_collisions(&mut self, u: &mut Universe) {
        let _ = u;
    }

    /// After bullet collisions, before the command layer (`aiplayerUpdateAll`, `kasExecute`).
    fn computer_players(&mut self, u: &mut Universe) {
        let _ = u;
    }
}

struct NoHooks;

impl UpdateHooks for NoHooks {}

/// The simulation state.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Universe {
    /// Game data: not saved, linked back on load ([`crate::save::Relink`]).
    #[serde(skip, default = "crate::save::statics")]
    pub statics: Arc<StaticWorld>,
    /// Per-type combat routines and their statics (built once from `statics`; not saved).
    #[serde(skip, default = "crate::save::combat")]
    pub combat: Arc<CombatStatics>,
    pub tweaks: SimTweaks,
    pub ships: ShipTable,
    pub bullets: BulletTable,
    /// `universe.SpaceObjList` (every object, creation order).
    pub space_obj_list: Vec<ObjRef>,
    /// `universe.ShipList`.
    pub ship_list: Vec<ShipId>,
    /// `universe.BulletList`.
    pub bullet_list: Vec<BulletId>,
    /// `universe.ImpactableList`.
    pub impactable_list: Vec<ObjRef>,
    /// The missiles and mines.
    pub missiles: crate::missile::MissileTable,
    /// `universe.MissileList`.
    pub missile_list: Vec<MissileId>,
    /// `universe.missileNumber`: the next missile's number.
    pub missile_number: u16,
    /// Slots handed out in the missile table.
    pub missile_slots: u32,
    /// Missiles that died this update (`deleteMissiles`), with the bullet type of what killed
    /// them (-1: no explosion).
    pub dead_missiles: Vec<(MissileId, i32)>,
    /// `universe.ResourceList`, `DerelictList` (resources live in the base layer).
    pub resource_list: Vec<ObjRef>,
    pub derelict_list: Vec<ObjRef>,
    /// `universe.RenderList`, rebuilt by [`Universe::update_render_list`].
    pub render_list: Vec<ObjRef>,
    /// The ships that died this update (`ShipsToDeleteList`), removed at its end.
    pub dead_ships: Vec<ShipId>,
    /// The `deathBy` (`EDT_*` explosion type) of each entry of [`Universe::dead_ships`].
    pub dead_ship_death_by: Vec<i32>,
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
    /// The single-player difficulty switch the damage factor reads (0x88a0d8): 0 doubles all
    /// weapon damage in single-player games.
    pub sp_difficulty: u32,
    /// `aishipTempDisableAvoiding`.
    pub temp_disable_avoiding: bool,
    /// A user-requested change, off by default (`hwcFormationFocusFire`): in a formation attack
    /// the wingmen always take the leader's target, as aggressive tactics do (the original
    /// shares it only when the target is under no command). A setting, not saved.
    #[serde(skip)]
    pub formation_focus_leader: bool,
    /// What happened during the last update.
    pub events: Vec<SimEvent>,
    /// The ships that died in the last update as the battle pings count them (presentation;
    /// not saved).
    #[serde(skip)]
    pub battle_losses: Vec<(ShipId, crate::battle_status::Loss)>,
    /// Where sentinel force fields were hit in the last update, by sentinel (presentation;
    /// not saved).
    #[serde(skip)]
    pub field_hits: Vec<(ShipId, Vector)>,
    /// What the missiles and ship modules did during the last update.
    pub special_events: Vec<SpecialEvent>,
    /// Speech the ships and orders asked for, waiting for the front end (which takes them;
    /// unlike the events they are not cleared by an update). Not saved.
    #[serde(skip)]
    pub speech: Vec<crate::speech::Speech>,
    /// Battle chatter's last times and places, and what the front end tells it (the
    /// original's globals: not saved).
    #[serde(skip)]
    pub chatter: crate::battle_chatter::ChatterState,
    /// What the front end's speech queue refuses (not saved; `None` refuses nothing).
    #[serde(skip)]
    pub speech_gate: Option<crate::speech::SpeechGate>,
    /// The last speech handle given out (`SNDcreatehandle`).
    #[serde(skip)]
    pub speech_handles: u32,
    /// The enemy-sighting warnings waiting to be said (`speechEnemyShipSighted`; the
    /// original's globals, reset with the level: not saved).
    #[serde(skip)]
    pub sightings: crate::speech::Sightings,
    /// The Leech's "nonviable target" note (`LeechChooseTarget` / `LeechHouseKeep` globals
    /// 0x91b67c..0x91b684, reset by `universeReset`: not saved).
    #[serde(skip)]
    pub leech_note: crate::ships::leech::LeechNote,
    /// Status messages for the local player's message panel (`moMessageAdd`), waiting for the
    /// front end (which takes them). Not saved.
    #[serde(skip)]
    pub messages: Vec<crate::speech::Message>,
    /// KAS `PauseUniverse` (0x8f1fc0): orders and ship movement stop, time goes on.
    pub universe_paused: bool,
    /// `nisIsRunning` (0x8f1f14): an NIS is playing (set by `nisStart`, cleared by `nisStop`).
    /// Games are not saved during one.
    #[serde(skip)]
    pub nis_running: bool,
    /// The letterbox bars as the damage rules see them (saved, as the original saves them).
    #[serde(default, deserialize_with = "crate::save::added_in_2")]
    pub letterbox: crate::letterbox::Letterbox,
    /// The base-building layer: players' resources, research and construction, resources,
    /// factories (see [`crate::base`]).
    pub base: crate::base::BaseState,
    /// Fog of war (only the local player has a grid; empty when off).
    pub fog: crate::fow::FogOfWar,
    /// A networked multiplayer game (`multiPlayerGame`, 0x8e9b44).
    pub multiplayer_game: bool,
    /// The KAS variable `G_NaggarokEMP` (EMP hits a flagged ship needs), when a mission set it.
    pub kas_naggarok_emp: Option<i32>,
    /// The mine formations of minelayers (see [`crate::ships::minelayer`]).
    pub mine_formations: Vec<crate::ships::minelayer::MineFormation>,
    /// Shock waves in flight and the big-gun shells they push (see [`crate::shockwave`]).
    pub shockwaves: crate::shockwave::Shockwaves,
    /// Dead ships waiting for their death effect to delete them (`deleteParentShip`). The
    /// original leaves them in `ShipList` flagged `SOF_Dead`; here they move out of
    /// [`Universe::ship_list`] so that no search finds them, still move (careen), and are
    /// drawn until hidden. Only used with [`Universe::effects`].
    pub hulks: Vec<ShipId>,
    /// The effect system (see [`crate::effects`]); `None`: no effects, and dead ships leave at
    /// once.
    pub effects: Option<Box<crate::effects::Effects>>,
    /// The level's nebulae (`nebNebulae`, see [`crate::nebula`]).
    pub nebulae: crate::nebula::Nebulae,
    /// The multiplayer rules (inactive in single player).
    #[serde(default, deserialize_with = "crate::save::added_in_9")]
    pub mp: crate::multiplayer::MpState,
    /// The multiplayer computer players.
    #[serde(default, deserialize_with = "crate::save::added_in_10")]
    pub ai: crate::ai::AiState,
}

impl Universe {
    /// An empty universe with `players` players.
    pub fn new(statics: Arc<StaticWorld>, tweaks: SimTweaks, players: &[ShipRace]) -> Self {
        let combat = Arc::new(CombatStatics::build(&statics));
        let mut u = Self {
            statics,
            combat,
            tweaks,
            ships: ShipTable::default(),
            bullets: BulletTable::default(),
            space_obj_list: Vec::new(),
            ship_list: Vec::new(),
            bullet_list: Vec::new(),
            impactable_list: Vec::new(),
            missiles: Default::default(),
            missile_list: Vec::new(),
            missile_number: 0,
            missile_slots: 0,
            dead_missiles: Vec::new(),
            resource_list: Vec::new(),
            derelict_list: Vec::new(),
            render_list: Vec::new(),
            dead_ships: Vec::new(),
            dead_ship_death_by: Vec::new(),
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
            sp_difficulty: 1,
            temp_disable_avoiding: false,
            formation_focus_leader: false,
            chatter: Default::default(),
            speech_gate: None,
            speech_handles: 0,
            sightings: Default::default(),
            leech_note: Default::default(),
            messages: Vec::new(),
            events: Vec::new(),
            battle_losses: Vec::new(),
            field_hits: Vec::new(),
            special_events: Vec::new(),
            speech: Vec::new(),
            universe_paused: false,
            letterbox: Default::default(),
            nis_running: false,
            base: crate::base::BaseState::new(players),
            fog: Default::default(),
            multiplayer_game: false,
            kas_naggarok_emp: None,
            mine_formations: Vec::new(),
            shockwaves: Default::default(),
            hulks: Vec::new(),
            effects: None,
            nebulae: Default::default(),
            mp: Default::default(),
            ai: Default::default(),
        };
        crate::base::init(&mut u);
        u
    }

    /// The mission clock (`moDrawRUsAndTime` 0x4a3650): `totaltimeelapsed` less `moTimer`
    /// (0x8f1754), which `moResetTimer` sets when the level loads (`univupdateReset`, the end
    /// of `singlePlayerLoadNewLevel`). The clock itself carries over from mission to mission,
    /// but the level load also restarts `univUpdateCounter`, which counts the same steps, so
    /// the time since the load is the counter's steps (and needs nothing more in a save).
    pub fn mission_time(&self) -> f32 {
        self.update_counter as f32 * UNIVERSE_UPDATE_PERIOD
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
        MaxVelocityContext { in_sphere_formation: sphere, special_factor: self.tweaks.combat.kamikaze_speed_burst }
    }

    /// `univCreateShip` (0x53d3c0) + `univAddShip`: a new ship at `position` with the default
    /// orientation (facing +X), owned by `player`. Optionally oriented to `heading`.
    pub fn add_ship(&mut self, ship_type: ShipType, player: usize, position: Vector, heading: Option<Vector>) -> Result<ShipId, CreateError> {
        crate::base::spawn_ship(self, ship_type, player, position, heading)
    }

    /// `univUpdate` (0x547410): one fixed step of `phys_time_elapsed` (1/16 s).
    pub fn update(&mut self) {
        self.update_with(&mut NoHooks);
    }

    /// [`update`](Self::update) with game-mode work (single player, KAS) run in its places.
    pub fn update_with(&mut self, hooks: &mut dyn UpdateHooks) {
        let dt = UNIVERSE_UPDATE_PERIOD;
        self.events.clear();
        self.battle_losses.clear();
        self.field_hits.clear();
        self.special_events.clear();
        self.total_time_elapsed = st(ld(self.total_time_elapsed) + ld(dt));
        self.update_counter = self.update_counter.wrapping_add(1);
        self.phys_time_elapsed = dt;
        // `nisUpdateTask` steps the letterbox at the same rate.
        self.letterbox.tick();
        // The multiplayer winner and game-over clocks.
        crate::multiplayer::update_clocks(self);
        crate::effects::begin_update(self);
        // The multiplayer resource trickle.
        crate::resource::resource_trickle(self);

        // Collision blobs: rebuilt on the first frame and every eighth, refreshed otherwise.
        let statics = self.statics.clone();
        let props = self.tweaks.blob;
        {
            let mut objs = Objects { ships: &mut self.ships, bullets: &mut self.bullets, missiles: &mut self.missiles };
            if self.update_counter == 1
                || (self.tweaks.refresh_collblob_rate & self.update_counter) == self.tweaks.refresh_collblob_frame
            {
                let order = self.space_obj_list.clone();
                let bullets = self.bullet_list.clone();
                let missiles = self.missile_list.clone();
                self.blobs.rebuild(&order, &bullets, &missiles, &mut objs, &statics, &props);
            } else {
                self.blobs.update_objects(&mut objs, &statics);
            }
        }

        // Nebula fog and chunk wobble (`nebSetFog`).
        crate::nebula::set_fog(self);
        // Build manager and research.
        crate::base::after_blobs(self);
        // Multiplayer resource injections and the lump sum.
        crate::multiplayer::update_resources(self);
        hooks.before_bullet_collisions(self);
        crate::collision::check_all_bullet_collisions(self);
        crate::crates::update(self);
        hooks.computer_players(self);
        // aiplayerUpdateAll's multiplayer computer players.
        crate::ai::update_all(self);
        if !self.universe_paused {
            command::cl_process(self);
            command::cl_post_process(self);
        }
        crate::ships::minelayer::update_mine_wall_formations(self);
        crate::collision::check_all_bump_collisions(self);
        crate::bullet::update_all(self);
        crate::missile::update_all(self);
        // (Derelicts would move here.)
        if !self.universe_paused {
            crate::resource::update_all_pos_vel(self);
            self.update_all_pos_vel_ships();
            crate::shockwave::update_all(self);
        } else {
            // While paused only the damage effects are kept up to date.
            for id in self.ship_list.clone().into_iter().chain(self.hulks.clone()) {
                crate::effects::damage::ship_think(self, id);
            }
        }
        crate::effects::update(self);
        crate::damage::delete_dead_ships(self);
        crate::resource::delete_dead_resources(self);
        crate::missile::delete_dead(self);
        // (Resource regrowth and bounties follow.)
        // The sound task's Leech check and the enemy-sighting warnings (`soundEventUpdate`,
        // `speechEnemySightingsUpdate`), run here once an update.
        crate::speech::leeches_in_view(self);
        crate::speech::sightings_update(self);
    }

    /// `univUpdateAllPosVelShips` (0x53ffe0): per ship, the state checks, the physics step and
    /// the post-physics housekeeping (`0x545250`); then the per-ship upkeep (`0x53f850`).
    fn update_all_pos_vel_ships(&mut self) {
        let statics = self.statics.clone();
        let list = self.ship_list.clone();
        // Hosts whose clamped ships move with them (`updateClampedObject`).
        let mut hosts: Vec<ShipId> = Vec::new();
        for &id in &list {
            let Some(ship) = self.ships.get(id) else { continue };
            if ship.flags & sof::DONT_APPLY_PHYSICS != 0 {
                crate::effects::damage::ship_think(self, id);
                if crate::clamp::host_of(self, id).is_some() {
                    crate::clamp::follow_host(self, id);
                }
                continue;
            }
            let Some(stat) = statics.ship(ship.ship_type) else { continue };
            self.check_ship_state(id);
            let Some(ship) = self.ships.get(id) else { continue };
            if let Some(host) = crate::clamp::physics_deferred_to_host(self, id) {
                hosts.push(host);
                self.housekeep(id);
                continue;
            }
            let ctx = PhysContext {
                dt: self.phys_time_elapsed,
                update_counter: self.update_counter,
                single_player: self.single_player,
                world_bounds: self.world_bounds,
                max_vel: self.max_velocity_context(ship),
                now: self.total_time_elapsed,
            };
            let tw = &self.tweaks;
            let old = ship.posinfo.position;
            physics::update_ship(self.ships.ship_mut(id), stat, tw, &ctx);
            if !self.fog.grids.is_empty() {
                let new = self.ships.ship(id).posinfo.position;
                crate::fow::move_viewer(self, id, old, new);
            }
            self.housekeep(id);
        }
        // The hulks drift and spin on until their death effects delete them.
        for id in self.hulks.clone() {
            let Some(ship) = self.ships.get(id) else { continue };
            let Some(stat) = statics.ship(ship.ship_type) else { continue };
            let ctx = PhysContext {
                dt: self.phys_time_elapsed,
                update_counter: self.update_counter,
                single_player: self.single_player,
                world_bounds: self.world_bounds,
                max_vel: self.max_velocity_context(ship),
                now: self.total_time_elapsed,
            };
            let tw = &self.tweaks;
            physics::update_ship(self.ships.ship_mut(id), stat, tw, &ctx);
            self.housekeep(id);
        }
        for host in hosts {
            if self.ships.get(host).is_some_and(|h| h.base.clamp.as_ref().is_some_and(|c| c.host.is_none())) {
                crate::clamp::update_clamped_object(self, host);
            }
        }
        for &id in &list {
            if self.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0) {
                self.upkeep(id);
            }
        }
        for &id in &list {
            if self.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0) {
                crate::ships::housekeep(self, id);
            }
        }
        let dt = self.phys_time_elapsed;
        for &id in &list {
            if self.ships.get(id).is_some_and(|s| s.flags & sof::DEAD == 0) {
                crate::mad::update(self, id, dt);
            }
        }
    }

    /// `univCheckShipState` (0x545670): right-of-way manoeuvres, autostabilisation of ships
    /// with nothing to do, and the tactics reaction to attackers and nearby enemies.
    fn check_ship_state(&mut self, id: ShipId) {
        let statics = self.statics.clone();
        let tw = self.tweaks.clone();
        let dt = self.phys_time_elapsed;
        if self.ships.ship(id).flags & sof::DEAD != 0 {
            return;
        }
        // The self-destruct timer (scuttling, spent gravity wells): the ship blows up.
        let dt_ = self.ships.ship(id).death_time;
        if dt_ != 0.0 && dt_ <= self.total_time_elapsed {
            let s = self.ships.ship(id);
            let blow = match s.ship_type.index() {
                0x40 | 0x4f => crate::ships::gravwell::is_derelict(self, id),
                _ => s.flags2 & crate::object::flags2::DISGUISED == 0 || !crate::ships::mimic::is_disguised(self, id),
            };
            if blow {
                self.ships.ship_mut(id).health = 0.0;
                crate::damage::add_ship_to_delete_list(self, id, -1);
                if self.ships.ship(id).flags & sof::DEAD != 0 {
                    return;
                }
            }
        }
        if self.ships.ship(id).special_flags & crate::object::special::ROW_GETTING_OUT_OF_WAY != 0 {
            crate::aiship::row_fly_out_of_way(self, id);
        }
        let s = self.ships.ship_mut(id);
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
        // (The world-limits recall is not modelled.)
        self.check_retaliation(id);
    }

    /// The reaction part of `univCheckShipState`.
    fn check_retaliation(&mut self, id: ShipId) {
        let tw = self.tweaks.combat.clone();
        let s = self.ships.ship(id);
        if s.flags & sof::DISABLED != 0 {
            return;
        }
        let Some(stat) = self.statics.ship(s.ship_type) else { return };
        let zone = stat.passive_retaliate_zone;
        let num = u32::from(id.0);
        let counter = self.update_counter;
        let cmd = s.command.and_then(|c| self.commands.get(c));
        if !s.ship_idle {
            let moving = cmd.is_some_and(|c| matches!(c.order, command::order::MOVE | command::order::DOCK));
            let rate = tw.check_passive_attack_whilemoving_rate;
            let stealthy = crate::cloak::is_cloaked(s) || crate::ships::mimic::is_disguised(self, id);
            if counter & rate == num & rate && zone != 0.0 && stat.is_capital_ship && moving && !stealthy {
                let enemies = crate::tactics::enemies_within_proximity(self, id, zone);
                crate::tactics::retaliate(self, id, enemies);
            }
            return;
        }
        // (Resource collectors retreat instead; collecting is not modelled.)
        if let Some(att) = s.getting_rocked.filter(|a| self.ships.get(*a).is_some()) {
            let ally = self.players.get(s.player).is_some_and(|p| p.is_ally(self.ships.ship(att).player));
            if !ally {
                let enemies = crate::tactics::ship_and_its_formation(self, att);
                crate::tactics::retaliate(self, id, enemies);
                return;
            }
        }
        let rate = tw.check_passive_attack_rate;
        // Cloaked ships and disguised Mimics do not give themselves away.
        let stealthy = crate::cloak::is_cloaked(s) || crate::ships::mimic::is_disguised(self, id);
        if counter & rate == num & rate && zone != 0.0 && !stealthy && s.flags & sof::SELECTABLE != 0 {
            let enemies = crate::tactics::enemies_within_proximity(self, id, zone);
            crate::tactics::retaliate(self, id, enemies);
        }
    }

    /// Post-physics housekeeping (0x545250): per-frame flags and the "recently attacked"
    /// counters.
    fn housekeep(&mut self, id: ShipId) {
        let s = self.ships.ship_mut(id);
        s.getting_rocked = None;
        if s.flags & sof::DEAD != 0 {
            return;
        }
        s.ship_idle = true;
        s.autostabilize_ship = true;
        s.special_flags &= !crate::object::special::TRANSIENT_8000000;
        if s.recently_attacked != 0 {
            if s.recent_attacker.is_none() {
                s.recently_attacked = 0;
            } else {
                s.recently_attacked -= 1;
                if s.recently_attacked == 0 {
                    s.recent_attacker = None;
                }
            }
        }
        if s.recently_fired_upon != 0 {
            if s.firing_at_us.is_none() {
                s.recently_fired_upon = 0;
            } else {
                s.recently_fired_upon -= 1;
                if s.recently_fired_upon == 0 {
                    s.firing_at_us = None;
                }
            }
        }
        s.flags &= !sof::FORMATION_SETTLED;
        crate::effects::damage::ship_think(self, id);
        // (Engine trails are updated here.)
    }

    /// Per-ship upkeep after all ships moved (`univShipThink` 0x53f850).
    fn upkeep(&mut self, id: ShipId) {
        if !crate::think::before_regeneration(self, id) {
            return;
        }
        crate::think::ship_think(self, id);
        let now = self.total_time_elapsed;
        let s = self.ships.ship_mut(id);
        if s.burst_firing {
            for g in s.guns.iter_mut() {
                if g.level != crate::bullet::weapon::ION
                    && g.burst_state == 1
                    && ld(g.use_burst_fire_time) + ld(0.1f32) < ld(now) - ld(g.burst_start_time)
                {
                    g.burst_state = 2;
                    s.burst_firing = false;
                    g.burst_start_time = now;
                }
            }
        }
        crate::think::after_bursts(self, id);
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
