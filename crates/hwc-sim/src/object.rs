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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ShipId(pub u16);

/// A reference to any object in the universe lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ObjRef {
    Ship(ShipId),
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
    pub const TARGETABLE: u32 = 0x0010_0000;
    pub const CRAZY: u32 = 0x0020_0000;
    /// Set by the formation code when a wingman is within 100 m of its slot, cleared again in
    /// each ship's post-physics housekeeping (purpose inferred from its uses).
    pub const FORMATION_SETTLED: u32 = 0x0040_0000;
    pub const DISABLED: u32 = 0x0080_0000;
    pub const CLAMPED: u32 = 0x0100_0000;
    pub const HYPERSPACE: u32 = 0x0200_0000;
}

/// `Ship.specialFlags` (+0x1c8) bits used by the movement code.
pub mod special {
    /// Ship is in a formation but has broken away from its slot.
    pub const BROKEN_FORMATION: u32 = 0x0000_0004;
    /// Ship has its own top speed in [`super::Ship::special_max_velocity`].
    pub const CUSTOM_MAX_VELOCITY: u32 = 0x0000_0100;
    pub const ROW_GETTING_OUT_OF_WAY: u32 = 0x0001_0000;
    pub const ATTACK_MOVE_CANCEL: u32 = 0x0002_0000;
    pub const IS_A_SALVAGER: u32 = 0x0008_0000;
    /// Cleared every tick by the post-physics housekeeping.
    pub const TRANSIENT_8000000: u32 = 0x0800_0000;
}

/// `PosInfo` (+0x58).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RotInfo {
    /// Columns are the object's up, right and heading vectors in world space.
    pub coordsys: Matrix,
    pub rotspeed: Vector,
    pub torque: Vector,
}

/// The parts of `CollInfo` (+0xd4) the simulation keeps up to date.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CollInfo {
    /// Collision sphere centre relative to the position, in world orientation.
    pub coll_offset: Vector,
    /// `position + coll_offset`.
    pub coll_position: Vector,
}

/// `NonStats` (+0x17c): the per-instance copy of the guidance statics.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommandId(pub u32);

/// Index into the universe's collision blob list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlobId(pub u32);

/// A ship instance (`Ship`).
#[derive(Debug, Clone, PartialEq)]
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
    pub dont_apply_force_ever: bool,
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
    /// Cataclysm: ramming state (+0x530) and push state (+0x550). Only their "inactive" value 0
    /// is produced by this slice; the physics code keeps their branches.
    pub ram_state: i32,
    pub push_state: i32,
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
            dont_apply_force_ever: false,
            ai_descend: 0.0,
            command: None,
            upgrade_flags: 0,
            color_scheme: 0,
            speed_cap: 0.0,
            speed_upgrade_level: 0,
            ram_state: 0,
            push_state: 0,
        }
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
}

/// A player (`Player`), reduced to what the slice uses.
#[derive(Debug, Clone, PartialEq, Eq)]
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
#[derive(Debug, Clone, Default, PartialEq)]
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

    /// Live ships in number order.
    pub fn iter(&self) -> impl Iterator<Item = &Ship> {
        self.slots.iter().flatten()
    }
}
