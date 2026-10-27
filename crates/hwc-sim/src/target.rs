//! Attack targets (`SpaceObjRotImpTarg`): a ship, or a mine the player ordered an attack on
//! (`selRectDragFunction` gathers mines for the attack band box). A mine stands in an attack
//! selection under its [`ShipId::from_mine`] id, as in the KAS selections.
//!
//! The combat code reads a target through [`Target`]: where it is, how it moves, its
//! collision sphere and box, and, for ships only, its class (`targetIndex = NUM_CLASSES` for
//! anything else, as `GenericInterceptorAttack` does).

use hwc_math::Vector;

use crate::missile::kind;
use crate::object::{BlobId, CollInfo, MissileId, PosInfo, RotInfo, Ship, ShipId};
use crate::statics::{ShipClass, ShipType, StaticCollInfo};
use crate::universe::Universe;

/// What the combat code reads of an attack target.
#[derive(Debug, Clone)]
pub struct Target {
    pub id: ShipId,
    pub posinfo: PosInfo,
    pub rotinfo: RotInfo,
    pub coll: CollInfo,
    pub coll_optimize_dist: f32,
    pub coll_my_blob: Option<BlobId>,
    pub flags: u32,
    /// The statics' collision sizes (`staticinfo->staticheader.staticCollInfo`).
    pub coll_header: StaticCollInfo,
    /// `staticheader.immobile`.
    pub immobile: bool,
    /// Ships only: the type, its class and whether it is a capital ship.
    pub ship_type: Option<ShipType>,
    pub ship_class: Option<ShipClass>,
    pub is_capital_ship: bool,
}

impl Target {
    pub fn is_ship(&self) -> bool {
        self.ship_type.is_some()
    }

    /// The mine this target is (`None`: a ship).
    pub fn mine(&self) -> Option<MissileId> {
        self.id.mine()
    }
}

/// A live mine standing for `id` (`None` for ships, gone mines and dead ones).
pub fn live_mine(u: &Universe, id: ShipId) -> Option<MissileId> {
    let mid = id.mine()?;
    u.missiles.get(mid).filter(|m| m.kind == kind::MINE && !m.is_dead()).map(|_| mid)
}

/// Whether `id` is a target that is still there: a ship, or a live mine.
pub fn exists(u: &Universe, id: ShipId) -> bool {
    if id.is_ship() {
        u.ships.get(id).is_some()
    } else {
        live_mine(u, id).is_some()
    }
}

/// The target `id` (a ship, or a live mine), as the combat code reads it.
pub fn get(u: &Universe, id: ShipId) -> Option<Target> {
    if id.is_ship() {
        let s = u.ships.get(id)?;
        let stat = u.statics.ship(s.ship_type)?;
        return Some(Target {
            id,
            posinfo: s.posinfo,
            rotinfo: s.rotinfo,
            coll: s.coll,
            coll_optimize_dist: s.coll_optimize_dist,
            coll_my_blob: s.coll_my_blob,
            flags: s.flags,
            coll_header: stat.header.coll.clone(),
            immobile: stat.header.immobile,
            ship_type: Some(s.ship_type),
            ship_class: stat.ship_class,
            is_capital_ship: stat.is_capital_ship,
        });
    }
    let mid = live_mine(u, id)?;
    let m = u.missiles.get(mid)?;
    let st = crate::missile::statics_of(u, m)?;
    Some(Target {
        id,
        posinfo: m.posinfo,
        rotinfo: m.rotinfo,
        coll: m.coll,
        coll_optimize_dist: m.coll_optimize_dist,
        coll_my_blob: m.coll_my_blob,
        flags: m.flags,
        coll_header: st.header.coll.clone(),
        immobile: st.header.immobile,
        ship_type: None,
        ship_class: None,
        is_capital_ship: false,
    })
}

/// What aiming at an object reads: its collision centre and its velocity.
pub trait Aim {
    fn aim_point(&self) -> Vector;
    fn aim_velocity(&self) -> Vector;
}

impl Aim for Ship {
    fn aim_point(&self) -> Vector {
        self.coll.coll_position
    }

    fn aim_velocity(&self) -> Vector {
        self.posinfo.velocity
    }
}

impl Aim for Target {
    fn aim_point(&self) -> Vector {
        self.coll.coll_position
    }

    fn aim_velocity(&self) -> Vector {
        self.posinfo.velocity
    }
}
