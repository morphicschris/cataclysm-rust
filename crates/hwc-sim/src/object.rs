//! Run-time objects: the `SpaceObj` family (`SpaceObj.h`) as Cataclysm lays it out.
//!
//! The original builds every object from one C struct prefix (`SpaceObj` → `SpaceObjRot` →
//! `SpaceObjRotImp` → `SpaceObjRotImpTarg` → ... → `Ship`). Here the shared parts are plain
//! structs ([`PosInfo`], [`RotInfo`], [`CollInfo`]) embedded in each object kind. Field docs
//! give the offset in Cataclysm's `Ship` (0x63c bytes plus ship-specific data) where it was
//! traced from the binary.

use hwc_math::{Matrix, Vector};

use crate::statics::{ShipRace, ShipType};

/// Object kinds (`ObjType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ObjType {
    Ship = 0,
    Bullet = 1,
    Asteroid = 2,
    Nebula = 3,
    Gas = 4,
    Dust = 5,
    Derelict = 6,
    Effect = 7,
    Missile = 8,
}

/// A ship's identity: `shipID.shipNumber`, handed out in creation order and never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct ShipId(pub u16);

impl ShipId {
    /// KAS selections hold space objects, not only ships: mines (`FindMinesInside`) and
    /// meteors (`FindMeteors`) are kept in a selection under ids of these two ranges, which
    /// ship numbers never reach.
    pub const MINE_RANGE: u16 = 0xC000;
    pub const METEOR_RANGE: u16 = 0x8000;
    const INDEX_MASK: u16 = 0x3FFF;

    /// The selection entry of a mine (`None` past the range).
    pub fn from_mine(m: MissileId) -> Option<ShipId> {
        (m.0 <= u32::from(Self::INDEX_MASK)).then_some(ShipId(Self::MINE_RANGE | m.0 as u16))
    }

    /// The selection entry of a meteor (an asteroid of a meteor storm).
    pub fn from_meteor(r: crate::resource::ResourceId) -> Option<ShipId> {
        (r.0 <= Self::INDEX_MASK).then_some(ShipId(Self::METEOR_RANGE | r.0))
    }

    /// The mine a selection entry stands for.
    pub fn mine(self) -> Option<MissileId> {
        (self.0 & 0xC000 == Self::MINE_RANGE).then_some(MissileId(u32::from(self.0 & Self::INDEX_MASK)))
    }

    /// The meteor a selection entry stands for.
    pub fn meteor(self) -> Option<crate::resource::ResourceId> {
        (self.0 & 0xC000 == Self::METEOR_RANGE).then_some(crate::resource::ResourceId(self.0 & Self::INDEX_MASK))
    }

    /// The entry is a ship (not a mine or meteor).
    pub fn is_ship(self) -> bool {
        self.0 & 0x8000 == 0
    }
}

/// A bullet's identity (slot in the bullet store; slots are reused once a bullet is gone).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct BulletId(pub u32);

/// A missile's or mine's identity (`missileID`, handed out in creation order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct MissileId(pub u32);

/// A reference to any object in the universe lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ObjRef {
    Ship(ShipId),
    Bullet(BulletId),
    /// A missile or a mine (`OBJ_MissileType`).
    Missile(MissileId),
}

impl ObjRef {
    pub fn ship(self) -> Option<ShipId> {
        match self {
            ObjRef::Ship(id) => Some(id),
            _ => None,
        }
    }

    pub fn missile(self) -> Option<MissileId> {
        match self {
            ObjRef::Missile(id) => Some(id),
            _ => None,
        }
    }
}

/// Tactics settings (`TacticsType`).
pub mod tactics {
    pub const EVASIVE: usize = 0;
    pub const NEUTRAL: usize = 1;
    pub const AGGRESSIVE: usize = 2;
}

/// `ATTACKEVENT_*` bits.
pub mod attack_event {
    pub const ENEMY_TARGET_DIED: u16 = 1;
    pub const LEADER_DIED: u16 = 2;
    pub const WINGMAN_DIED: u16 = 4;
}

/// `ATTACKSITUATION_*` values.
pub mod attack_situation {
    pub const EVADING_REPOS: u16 = 1;
    pub const JUST_EVADING: u16 = 2;
    pub const SPLITTING: u16 = 3;
    pub const SANDWICH: u16 = 4;
}

/// `RECENT_ATTACK_DURATION`: 5 seconds of frames.
pub const RECENT_ATTACK_DURATION: u8 = 80;

/// `SpecificAttackVars` (+0x374).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AttackVars {
    /// One target per gun, for ships that can target several at once (+0x374).
    pub multiple_targets: Option<Vec<Option<ShipId>>>,
    /// +0x378.
    pub target: Option<ShipId>,
    /// +0x37c / +0x380: fighter pairs.
    pub leader: Option<ShipId>,
    pub wingman: Option<ShipId>,
    /// +0x384 / +0x386 / +0x388.
    pub flightmans_left: u16,
    pub situation: u16,
    pub events: u16,
}

/// A gun's run-time state (`Gun`, 0x88 bytes in Cataclysm).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Gun {
    /// +0x00. Weapon technology currently fitted (index into `WeaponType`).
    pub level: u32,
    /// +0x04. Barrel direction in ship coordinates.
    pub heading: Vector,
    /// +0x10 / +0x34. Barrel orientation for `GUN_NewGimble` guns.
    pub cur_orientation: Matrix,
    pub cur_orientation_non_concat: Matrix,
    /// +0x58 azimuth, +0x5c declination, +0x60 / +0x64 their rates.
    pub angle: f32,
    pub declination: f32,
    pub angle_speed: f32,
    pub declination_speed: f32,
    /// +0x68.
    pub num_missiles: i32,
    /// +0x70.
    pub last_time_fired: f32,
    /// +0x74..+0x80: burst-fire state (`GUNBURST_STATE_*`).
    pub burst_start_time: f32,
    pub use_burst_fire_time: f32,
    pub use_burst_wait_time: f32,
    pub burst_state: u32,
}

/// Per-type ship state kept in `ShipSpecifics` (+0x638 onwards) by the combat code.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct InterceptorSpec {
    /// +0x638 `lasttimefired`.
    pub last_time_fired: f32,
    /// +0x63c `aitimekill`.
    pub ai_time_kill: f32,
    /// +0x640 `aivec`.
    pub ai_vec: Vector,
    /// +0x64c `aiAIPflightman`.
    pub aip_flightman: u16,
    /// +0x64e `activeGun`.
    pub active_gun: u16,
    /// +0x650.. bombing-run rally point, original point, angle and delay.
    pub rally_point: Vector,
    pub orig_point: Vector,
    pub angle: f32,
    pub bombing_delay: i32,
}

/// Attack-side-step state (`attackSideStep` parameters) for corvettes and the multi-beam
/// frigate.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SideStepSpec {
    /// Time the side step started.
    pub time: f32,
    /// Direction of the side step (a `TRANS_*` value, kept as the float the original stores).
    pub direction: f32,
}

/// `SpaceObj.flags` bits as Cataclysm numbers them (several moved relative to HW1).
pub mod sof {
    pub const ROTATABLE: u32 = 0x0000_0001;
    pub const IMPACTABLE: u32 = 0x0000_0002;
    pub const DONT_APPLY_PHYSICS: u32 = 0x0000_0004;
    pub const NIS_SHIP: u32 = 0x0000_0080;
    pub const DEAD: u32 = 0x0000_0800;
    /// The object's collision blob is fine as it is (moved from 0x2000 in HW1).
    pub const DONT_CREATE_BLOB: u32 = 0x0000_1000;
    pub const SELECTABLE: u32 = 0x0000_2000;
    pub const HIDE: u32 = 0x0000_8000;
    pub const BIG_OBJECT: u32 = 0x0002_0000;
    pub const NOT_BUMPABLE: u32 = 0x0008_0000;
    /// Bump collisions are ignored (bit 31; cleared for ships by `specialFlags2 & 8`).
    pub const NO_BUMPS: u32 = 0x8000_0000;
    pub const TARGETABLE: u32 = 0x0010_0000;
    pub const CRAZY: u32 = 0x0020_0000;
    /// Set by the formation code when a wingman is within 100 m of its slot, cleared again in
    /// each ship's post-physics housekeeping (purpose inferred from its uses).
    pub const FORMATION_SETTLED: u32 = 0x0040_0000;
    pub const DISABLED: u32 = 0x0080_0000;
    pub const CLAMPED: u32 = 0x0100_0000;
    pub const HYPERSPACE: u32 = 0x0200_0000;
}

/// `SpaceObj.flags2` (+0x18) bits.
pub mod flags2 {
    /// Forces the impactable update.
    pub const FORCE_IMPACTABLE: u32 = 0x0000_0002;
    /// Takes no damage (`ApplyDamageToTarget` returns at once).
    pub const INVULNERABLE: u32 = 0x0000_0004;
    /// A Mimic in disguise.
    pub const DISGUISED: u32 = 0x0000_0008;
    /// Disabled for good (multiplayer derelicts).
    pub const PERMANENTLY_DISABLED: u32 = 0x0000_0010;
    /// Mines and enemy targeting ignore the ship.
    pub const IGNORED_BY_MINES: u32 = 0x0000_0020;
    /// Set while a ship's references are being removed from every ship after it docked
    /// (`univShipThink`'s dock notification).
    pub const DOCK_NOTIFY: u32 = 0x0000_0100;
}

/// `Ship.specialFlags` (+0x1c8) bits used by the movement code.
pub mod special {
    /// Ship is in a formation but has broken away from its slot.
    pub const BROKEN_FORMATION: u32 = 0x0000_0004;
    /// Ship has its own top speed in [`super::Ship::special_max_velocity`].
    pub const CUSTOM_MAX_VELOCITY: u32 = 0x0000_0100;
    /// `SPECIAL_Kamikaze`.
    pub const KAMIKAZE: u32 = 0x0000_0002;
    /// `SPECIAL_Attacking...` side-stepping flags cleared with the attack.
    pub const ATTACK_SIDESTEP: u32 = 0x0000_0004;
    /// `SPECIAL_KasSpecialDamageModifier`: [`super::Ship::damage_modifier`] applies.
    pub const KAS_DAMAGE_MODIFIER: u32 = 0x0000_4000;
    /// `SPECIAL_KasSpecialRepairModifier`: KAS set the ship's repair multiplier
    /// ([`crate::base::ShipBase::health_mult`]).
    pub const KAS_REPAIR_MODIFIER: u32 = 0x0000_2000;
    pub const ROW_GETTING_OUT_OF_WAY: u32 = 0x0001_0000;
    /// Set when the attack started from well above the target (`ATTACKING_FROM_ABOVE`).
    pub const ATTACKING_FROM_ABOVE: u32 = 0x0010_0000;
    /// Gun fire suppressed (`SPECIAL_...`; `gunShoot` refuses to fire).
    pub const CANT_FIRE: u32 = 0x1000_0000;
    pub const ATTACK_MOVE_CANCEL: u32 = 0x0002_0000;
    pub const IS_A_SALVAGER: u32 = 0x0008_0000;
    /// Cleared every tick by the post-physics housekeeping.
    pub const TRANSIENT_8000000: u32 = 0x0800_0000;
    /// The ship's guard chain comes back to itself (`FindShipsWereGuarding`): it does not
    /// try to keep pace with the ship it protects.
    pub const CIRCULAR_GUARD: u32 = 0x4000_0000;
}

/// `PosInfo` (+0x58).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct PosInfo {
    pub position: Vector,
    pub velocity: Vector,
    pub force: Vector,
    /// `ISMOVING_MOVING` (1) and `ISMOVING_ROTATING` (2).
    pub is_moving: u16,
    pub havent_calculated_dist: bool,
}

pub const ISMOVING_MOVING: u16 = 1;
pub const ISMOVING_ROTATING: u16 = 2;

/// `RotInfo` (+0x98).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct RotInfo {
    /// Columns are the object's up, right and heading vectors in world space.
    pub coordsys: Matrix,
    pub rotspeed: Vector,
    pub torque: Vector,
}

/// The parts of `CollInfo` (+0xd4) the simulation keeps up to date.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct CollInfo {
    /// Collision sphere centre relative to the position, in world orientation.
    pub coll_offset: Vector,
    /// `position + coll_offset`.
    pub coll_position: Vector,
    /// `collrectoffset` (+0xec): the eight corners of the collision box relative to the
    /// position, in world orientation (`collUpdateCollRectangle`; zero when the type has no
    /// box).
    pub rect: [Vector; 8],
}

/// `NonStats` (+0x17c): the per-instance copy of the guidance statics.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NonStats {
    pub thrust_strength: [f32; 6],
    pub rot_strength: [f32; 6],
    pub turn_speed: [f32; 3],
}

/// Translation directions for [`crate::physics::apply_force`] (`TRANS_*`).
pub mod trans {
    pub const UP: u16 = 0;
    pub const DOWN: u16 = 1;
    pub const RIGHT: u16 = 2;
    pub const LEFT: u16 = 3;
    pub const FORWARD: u16 = 4;
    pub const BACK: u16 = 5;
}

/// Rotation directions for [`crate::physics::apply_rot`] (`ROT_*`).
pub mod rot {
    pub const ABOUT_X_CCW: u16 = 0;
    pub const ABOUT_X_CW: u16 = 1;
    pub const ABOUT_Y_CCW: u16 = 2;
    pub const ABOUT_Y_CW: u16 = 3;
    pub const ABOUT_Z_CCW: u16 = 4;
    pub const ABOUT_Z_CW: u16 = 5;
}

/// Index into the command layer's command store.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct CommandId(pub u32);

/// Index into the universe's collision blob list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct BlobId(pub u32);

/// A ship instance (`Ship`).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ship {
    pub id: ShipId,
    pub ship_type: ShipType,
    pub ship_race: ShipRace,
    /// `flags` (+0x14), see [`sof`].
    pub flags: u32,
    /// Second flag word (+0x18; bit 1 forces the impactable update, bit 3 marks a Mimic in
    /// disguise).
    pub flags2: u32,
    /// Index of the owning player (`playerowner`, +0x32c).
    pub player: usize,
    pub posinfo: PosInfo,
    pub rotinfo: RotInfo,
    pub coll: CollInfo,
    /// `collOptimizeDist` (+0x50): distance from the centre of the object's collision blob.
    pub coll_optimize_dist: f32,
    /// `collMyBlob` (+0x54).
    pub coll_my_blob: Option<BlobId>,
    /// `health` (+0x170).
    pub health: f32,
    pub nonstat: NonStats,
    /// Current top speed copy (+0x1b8), initialised from the statics.
    pub max_velocity_copy: f32,
    /// `aistate` (+0x1c0): the fly-to state machine.
    pub ai_state: u8,
    /// `dockingship` (+0x1c4): no avoidance while set.
    pub docking_ship: Option<ShipId>,
    /// `attributes` (+0x38, set by missions: bit 4 = never pushed by collisions).
    pub attributes: u32,
    /// `specialFlags` (+0x1c8), see [`special`].
    pub special_flags: u32,
    /// `specialFlags2` (+0x1cc).
    pub special_flags2: u32,
    /// `moveTo` (+0x24c) and `moveFrom` (+0x258).
    pub move_to: Vector,
    pub move_from: Vector,
    /// `formationOffset` (+0x264) in leader (up, right, heading) coordinates.
    pub formation_offset: Vector,
    /// `formationHeading` (+0x270).
    pub formation_heading: Vector,
    /// `engineOffset` (+0x27c) and `enginePosition` (+0x288).
    pub engine_offset: Vector,
    pub engine_position: Vector,
    /// `isDodging` (+0x2bc) and `DodgeTime` (+0x2c0).
    pub is_dodging: bool,
    pub dodge_time: f32,
    /// `timeCreated` (+0x2e0).
    pub time_created: f32,
    /// Ship-specific top speed (+0x2f8), used when [`special::CUSTOM_MAX_VELOCITY`] is set.
    pub special_max_velocity: f32,
    /// `shipidle` (+0x34c).
    pub ship_idle: bool,
    /// `autostabilizeship` (+0x34d), `autostabilizestate` (+0x34e), `autostabilizewait` (+0x34f).
    pub autostabilize_ship: bool,
    pub autostabilize_state: u8,
    pub autostabilize_wait: u8,
    /// `aistateattack` (+0x350).
    pub ai_state_attack: u8,
    /// `aistatecommand` (+0x355).
    pub ai_state_command: u8,
    /// `dontrotateever` (+0x356) and `dontapplyforceever` (+0x357).
    pub dont_rotate_ever: bool,
    /// Bit 4 of `dontrotateever`: a wingman of a formation attack is firing at a target ahead
    /// (`delegateCommand`), so the formation keeps its heading on the target this frame.
    pub dont_rotate_formation_fire: bool,
    /// Bit 0: never apply forces; bit 1: a right-of-way manoeuvre owns the thrusters.
    pub dont_apply_force_ever: u8,
    /// `aidescend` (+0x360): 0, or ±1/(height to descend) for the fancy capital-ship descent.
    pub ai_descend: f32,
    /// The command the ship belongs to (`command`, +0x370; in Cataclysm also its formation).
    pub command: Option<CommandId>,
    /// Weapon/engine upgrade bits (+0x3f4); bits 0..2 add 0.15 to the speed multiplier.
    pub upgrade_flags: u32,
    /// `colorScheme`.
    pub color_scheme: usize,
    /// Cataclysm: per-ship speed cap (+0x5c4); ignored when not positive.
    pub speed_cap: f32,
    /// Cataclysm: engine upgrade level (+0x5f8), index into the speed table (1.0 .. 1.4).
    pub speed_upgrade_level: u32,

    // --- combat state
    /// `lasttimecollided` (+0x178): last bump collision.
    pub last_time_collided: f32,
    /// `rowGetOutOfWay` (+0x294), `rowOriginalPoint` (+0x298), `rowOutOfWayPoint` (+0x2a4),
    /// `rowState` (+0x35d).
    pub row_get_out_of_way: Option<ShipId>,
    pub row_original_point: Vector,
    pub row_out_of_way_point: Vector,
    pub row_state: u8,
    /// `tacticstype` (+0x2b8), see [`tactics`].
    pub tactics: usize,
    /// `tacticsTalk` (+0x2d4) and `tacticsNeedToFlightMan` (+0x2d8).
    pub tactics_talk: u32,
    pub tactics_need_to_flightman: u32,
    /// `gettingrocked` (+0x340, cleared every frame), `recentAttacker` (+0x344),
    /// `firingAtUs` (+0x348).
    pub getting_rocked: Option<ShipId>,
    pub recent_attacker: Option<ShipId>,
    pub firing_at_us: Option<ShipId>,
    /// `flightmanState1` (+0x351), `flightmanState2` (+0x352).
    pub flightman_state1: u8,
    pub flightman_state2: u8,
    /// `shipisattacking` (+0x358) and Cataclysm's companion flag (+0x616).
    pub ship_is_attacking: bool,
    pub ship_is_attacking2: bool,
    /// `recentlyAttacked` (+0x35a), `recentlyFiredUpon` (+0x35b), `chatterBits` (+0x35c).
    pub recently_attacked: u8,
    pub recently_fired_upon: u8,
    pub chatter_bits: u8,
    /// `flightman` (+0x364) and its private data (+0x368).
    pub flightman: u32,
    pub flightman_info: Option<crate::flightman::FlightmanInfo>,
    /// `attackvars` (+0x374).
    pub attack: AttackVars,
    /// `howDidIDie` (+0x39c) and `whoKilledMe` (+0x3a0).
    pub how_did_i_die: i32,
    pub who_killed_me: i32,
    /// `holdingPatternPos` (+0x3d0).
    pub holding_pattern_pos: Vector,
    /// `damageModifier` (+0x3e0), used with [`special::KAS_DAMAGE_MODIFIER`].
    pub damage_modifier: f32,
    /// Armour upgrade level (+0x3f0), the column of the weapon damage table.
    pub armour_level: u32,
    /// Kills per target class (+0x5c8), which drive Cataclysm's veterancy upgrades.
    pub kills: [u32; 12],
    /// Burst fire in progress on some gun (+0x610).
    pub burst_firing: bool,
    /// The ship's guns (`gunInfo`, +0x490).
    pub guns: Vec<Gun>,
    /// Interceptor-family specifics (+0x638..).
    pub interceptor: InterceptorSpec,
    /// Side-step specifics of corvettes (+0x638) and the multi-beam frigate (+0x658).
    pub side_step: SideStepSpec,
    /// Docking, clamping, construction, resourcing and salvage state (see [`crate::base`]).
    pub base: crate::base::ShipBase,
    /// The type's own state (`ShipSpecifics`, +0x638 onwards), see [`crate::ships`].
    pub spec: crate::ships::ShipSpec,
    /// Per-player visibility of stealthy types (cloakers +0x650, Leech +0x654, cloaked
    /// fighter +0x668, Mimics +0x6e4): `univShipVisibleToPlayer`.
    pub visible_to: [bool; 8],
    /// Disabling (EMP, timed disable, field slow-down), +0x528..+0x570.
    pub disable: crate::disable::DisableState,
    /// A target to attack at the next think (+0x4b0, set by retaliation code).
    pub queued_attack: Option<ShipId>,
    /// The flicker effect after an EMP on a KAS-flagged ship (+0x4d0..+0x4dc).
    pub flicker: crate::disable::Flicker,
    /// Beast infection of this ship (+0x4e4 and around), see [`crate::ships::beast`].
    pub beast: crate::ships::beast::Infection,
    /// The repair droids crawling over the hull (+0x4c0), see [`crate::ships::repair_droid`].
    pub droids: Option<Box<crate::ships::repair_droid::DroidArray>>,
    /// Worker repair: the counts of workers that picked this ship (+0x600 / +0x604) and, for a
    /// worker, its repair state (+0x63c..+0x67c), see [`crate::ships::worker_repair`].
    pub worker_repair: crate::ships::worker_repair::WorkerRepairState,
    /// `+0x2fc`: when set, the time at which `univCheckShipState` blows the ship up (a spent
    /// gravity well, modules of a dead mothership).
    pub death_time: f32,
    /// `+0x394`: players whose proximity sensors have sensed the ship (bit per player).
    pub sensed_by: u32,
}

impl Ship {
    /// A ship with every run-time field at its zero/initial value (what `univCreateShip`
    /// starts from before filling in the specifics).
    pub fn new(id: ShipId, ship_type: ShipType, ship_race: ShipRace, player: usize) -> Self {
        Ship {
            id,
            ship_type,
            ship_race,
            flags: 0,
            flags2: 0,
            player,
            posinfo: PosInfo::default(),
            rotinfo: RotInfo::default(),
            coll: CollInfo::default(),
            coll_optimize_dist: 0.0,
            coll_my_blob: None,
            health: 0.0,
            nonstat: NonStats::default(),
            max_velocity_copy: 0.0,
            ai_state: 0,
            docking_ship: None,
            attributes: 0,
            special_flags: 0,
            special_flags2: 0,
            move_to: Vector::ZERO,
            move_from: Vector::ZERO,
            formation_offset: Vector::ZERO,
            formation_heading: Vector::ZERO,
            engine_offset: Vector::ZERO,
            engine_position: Vector::ZERO,
            is_dodging: false,
            dodge_time: 0.0,
            time_created: 0.0,
            special_max_velocity: 0.0,
            ship_idle: false,
            autostabilize_ship: false,
            autostabilize_state: 0,
            autostabilize_wait: 0,
            ai_state_attack: 0,
            ai_state_command: 0,
            dont_rotate_ever: false,
            dont_rotate_formation_fire: false,
            dont_apply_force_ever: 0,
            ai_descend: 0.0,
            command: None,
            upgrade_flags: 0,
            color_scheme: 0,
            speed_cap: 0.0,
            speed_upgrade_level: 0,
            last_time_collided: 0.0,
            row_get_out_of_way: None,
            row_original_point: Vector::ZERO,
            row_out_of_way_point: Vector::ZERO,
            row_state: 0,
            tactics: tactics::NEUTRAL,
            tactics_talk: 0,
            tactics_need_to_flightman: 0,
            getting_rocked: None,
            recent_attacker: None,
            firing_at_us: None,
            flightman_state1: 0,
            flightman_state2: 0,
            ship_is_attacking: false,
            ship_is_attacking2: false,
            recently_attacked: 0,
            recently_fired_upon: 0,
            chatter_bits: 0,
            flightman: 0,
            flightman_info: None,
            attack: AttackVars::default(),
            how_did_i_die: 0,
            who_killed_me: 0,
            holding_pattern_pos: Vector::ZERO,
            damage_modifier: 0.0,
            armour_level: 0,
            kills: [0; 12],
            burst_firing: false,
            guns: Vec::new(),
            interceptor: InterceptorSpec::default(),
            side_step: SideStepSpec::default(),
            base: crate::base::ShipBase::default(),
            spec: crate::ships::ShipSpec::None,
            visible_to: [true; 8],
            disable: crate::disable::DisableState::default(),
            queued_attack: None,
            flicker: crate::disable::Flicker::default(),
            beast: crate::ships::beast::Infection::default(),
            droids: None,
            worker_repair: Default::default(),
            death_time: 0.0,
            sensed_by: 0,
        }
    }

    /// Whether any `dontrotateever` bit is set (the ship's rotation is left alone).
    pub fn rotation_frozen(&self) -> bool {
        self.dont_rotate_ever || self.dont_rotate_formation_fire
    }

    /// Heading (third column of the coordinate system).
    pub fn heading(&self) -> Vector {
        self.rotinfo.coordsys.col(2)
    }

    /// Up vector (first column).
    pub fn up(&self) -> Vector {
        self.rotinfo.coordsys.col(0)
    }

    /// Right vector (second column).
    pub fn right(&self) -> Vector {
        self.rotinfo.coordsys.col(1)
    }

    pub fn is_dead(&self) -> bool {
        self.flags & sof::DEAD != 0
    }

    /// `ship->posinfo.position` of the collision centre (`collInfo.collPosition`).
    pub fn coll_pos(&self) -> Vector {
        self.coll.coll_position
    }
}

/// A player (`Player`), reduced to what the slice uses.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Player {
    pub index: usize,
    pub race: ShipRace,
    /// Alliance bit mask (`Allies`); a player is always allied with itself.
    pub allies: u32,
    /// Team colour scheme.
    pub color_scheme: usize,
}

impl Player {
    pub fn new(index: usize, race: ShipRace) -> Self {
        Self { index, race, allies: 1 << index, color_scheme: index }
    }

    pub fn is_ally(&self, other: usize) -> bool {
        self.allies & (1 << other) != 0
    }
}

/// Ships by number (`ShipIDToPtr`): numbers are handed out sequentially and never reused.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShipTable {
    slots: Vec<Option<Ship>>,
}

impl ShipTable {
    pub fn get(&self, id: ShipId) -> Option<&Ship> {
        self.slots.get(id.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: ShipId) -> Option<&mut Ship> {
        self.slots.get_mut(id.0 as usize).and_then(Option::as_mut)
    }

    /// The ship with this number; panics if it does not exist (an internal invariant).
    pub fn ship(&self, id: ShipId) -> &Ship {
        self.get(id).expect("ship id refers to a live ship")
    }

    pub fn ship_mut(&mut self, id: ShipId) -> &mut Ship {
        self.get_mut(id).expect("ship id refers to a live ship")
    }

    pub(crate) fn insert(&mut self, ship: Ship) {
        let i = ship.id.0 as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, None);
        }
        self.slots[i] = Some(ship);
    }

    /// Removes a ship (it has been deleted from the universe).
    pub(crate) fn remove(&mut self, id: ShipId) -> Option<Ship> {
        self.slots.get_mut(id.0 as usize).and_then(Option::take)
    }

    /// Live ships in number order.
    pub fn iter(&self) -> impl Iterator<Item = &Ship> {
        self.slots.iter().flatten()
    }
}

/// A bullet or beam (`Bullet`, 0x164 bytes in Cataclysm).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Bullet {
    pub id: BulletId,
    /// +0x14.
    pub flags: u32,
    pub posinfo: PosInfo,
    /// +0x98. Built from the heading.
    pub coordsys: Matrix,
    /// +0x50 / +0x54.
    pub coll_optimize_dist: f32,
    pub coll_my_blob: Option<BlobId>,
    /// +0xe4 `bulletheading` (world, normalised).
    pub heading: Vector,
    /// +0xf0 `lengthvec`, +0xfc `lengthmag`.
    pub length_vec: Vector,
    pub length_mag: f32,
    /// +0x100 `traveldist`: distance covered per frame (beams: their length).
    pub travel_dist: f32,
    /// +0x104 `beamtraveldist`: length of a beam up to what it hit.
    pub beam_travel_dist: f32,
    /// +0x108. Weapon technology of the gun that fired it (`WeaponType` index).
    pub weapon: u32,
    /// +0x10c. Fired in the burst attack (`SPECIAL_BurstFiring`).
    pub burst: bool,
    /// +0x110. Bullet type (0..19): the weapon technology combined with the gun size.
    pub bullet_type: u32,
    /// +0x114 `timelived`, +0x118 `totallifetime`.
    pub time_lived: f32,
    pub total_lifetime: f32,
    /// +0x11c owner, +0x120 the gun that fired it (index), +0x124 target.
    pub owner: Option<ShipId>,
    pub gun: Option<usize>,
    pub target: Option<ShipId>,
    /// +0x128 colour (packed RGB from the gun-event table; 0 when unknown).
    pub color: u32,
    /// +0x12c `bulletmass`.
    pub mass: f32,
    /// +0x138 damage.
    pub damage: f32,
    /// +0x13c: the first physics update has happened (the lifetime check starts after it).
    pub updated_once: bool,
    /// +0x14c `playerowner`.
    pub player: usize,
    /// +0x150 / +0x154: maximum range and distance flown (Cataclysm, big-gun shells).
    pub max_range: f32,
    pub dist_flown: f32,
    /// +0x158: position at the start of the frame.
    pub prev_position: Vector,
    /// +0x140: bit 0 set once a DFG field has had its go at the bullet.
    pub dfg_flags: u32,
}

/// Bullets by slot; slots are reused (in a fixed order, so runs stay deterministic).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BulletTable {
    slots: Vec<Option<Bullet>>,
    free: Vec<u32>,
}

impl BulletTable {
    pub fn get(&self, id: BulletId) -> Option<&Bullet> {
        self.slots.get(id.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: BulletId) -> Option<&mut Bullet> {
        self.slots.get_mut(id.0 as usize).and_then(Option::as_mut)
    }

    pub fn bullet(&self, id: BulletId) -> &Bullet {
        self.get(id).expect("bullet id refers to a live bullet")
    }

    pub fn bullet_mut(&mut self, id: BulletId) -> &mut Bullet {
        self.get_mut(id).expect("bullet id refers to a live bullet")
    }

    /// The id the next bullet will get.
    pub(crate) fn next_id(&self) -> BulletId {
        BulletId(self.free.last().copied().unwrap_or(self.slots.len() as u32))
    }

    pub(crate) fn insert(&mut self, mut bullet: Bullet) -> BulletId {
        let id = self.next_id();
        bullet.id = id;
        if self.free.pop().is_none() {
            self.slots.push(None);
        }
        self.slots[id.0 as usize] = Some(bullet);
        id
    }

    pub(crate) fn remove(&mut self, id: BulletId) -> Option<Bullet> {
        let b = self.slots.get_mut(id.0 as usize).and_then(Option::take);
        if b.is_some() {
            self.free.push(id.0);
        }
        b
    }

    pub fn iter(&self) -> impl Iterator<Item = &Bullet> {
        self.slots.iter().flatten()
    }
}
