//! `ShipUpgrade.c`: Cataclysm's ship upgrades. Researched technologies give a player a tech
//! level (0..2), upgrade bits (engines, cloaking, repair, salvage...) and better gun types; a
//! ship takes them over when it is created and, for most types, when it docks
//! (`suShipApplyUpgrades`).

use crate::base::{BaseState, Techs, NUM_SHIP_TYPES};
use crate::object::{Ship, ShipId};
use crate::statics::{ShipRace, ShipStaticInfo, ShipType, WeaponType};
use crate::universe::Universe;

/// 0x7212f0: per ship type `{initial tech level, maximum tech level}` (every initial level is
/// 0).
const MAX_TECH_LEVEL: [i32; NUM_SHIP_TYPES] = [
    1, 1, 1, 1, 1, 1, 2, 2, 2, 0, 2, 2, 2, 2, 2, 0, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 0, 2, 2, 2, 2,
    2, 2, 0, 2, 2, 2, 2, 2, 2, 2, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0,
];

/// 0x721778: upgrade bits valid per ship type.
const TYPE_MASK: [u32; NUM_SHIP_TYPES] = [
    0x800001, 0x4000009, 0xa9, 0x40, 0x211, 0xb1, 0x2600100, 0x200002, 0x200802, 0x0, 0x200404, 0x0,
    0x300004, 0x0, 0x6c0000, 0x0, 0x340000, 0x807000, 0x300000, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x200004, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x800001, 0x4000009, 0x1,
    0x20001, 0x1, 0x211, 0x1, 0x1, 0x2, 0x802, 0x0, 0x20002, 0x8002, 0x10004, 0x110004,
    0x2000100, 0xc0000, 0x0, 0x140000, 0x807000, 0x110000, 0x0, 0x0, 0x0, 0x0, 0x10001, 0x1,
    0x0, 0x0, 0x0, 0x0, 0x1000000, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x80000, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x1000000, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x800000, 0x0, 0x80000, 0x0, 0x0, 0x800000, 0x0, 0x20000, 0x0,
    0x800000, 0x80020, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x10000, 0x0,
    0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0, 0x0,
    0x0,
];

/// 0x7219c0: `{technology, bit, second bit}` for each upgrade bit.
pub const UPGRADE_BIT_TECHS: [(usize, u32, u32); 25] = [
    (0x9, 0x1, 0x0),
    (0xb, 0x2, 0x0),
    (0xd, 0x4, 0x0),
    (0x18, 0x8, 0x10),
    (0x19, 0x20, 0x0),
    (0x11, 0x40, 0x0),
    (0x17, 0x80, 0x0),
    (0x1a, 0x100, 0x0),
    (0x16, 0x200, 0x0),
    (0x15, 0x400, 0x0),
    (0x11, 0x800, 0x0),
    (0x1d, 0x1000, 0x0),
    (0x1e, 0x2000, 0x0),
    (0x1f, 0x4000, 0x0),
    (0x4, 0x8000, 0x0),
    (0x20, 0x10000, 0x0),
    (0x0, 0x20000, 0x0),
    (0x21, 0x200000, 0x0),
    (0x7, 0x80000, 0x0),
    (0x22, 0x100000, 0x0),
    (0x6, 0x40000, 0x0),
    (0x24, 0x400000, 0x0),
    (0x1c, 0x800000, 0x0),
    (0x13, 0x2000000, 0x0),
    (0x14, 0x4000000, 0x0),
];

/// 0x721af0: `{technology, gun type}`; technology 0x29 means always available.
pub const GUN_TYPE_TECHS: [(usize, usize); 9] = [(41, 0), (18, 1), (19, 2), (2, 3), (41, 4), (20, 5), (41, 6), (27, 7), (32, 8)];

/// Upgrade bit: the ship repairs itself (`suShipHasUpgrade(ship, 0x200000)`).
pub const BIT_REPAIR: u32 = 0x20_0000;
/// Upgrade bit: salvage capable.
pub const BIT_SALVAGE: u32 = 0x8_0000;

/// `suShipInit`: the initial tech level of a type.
pub fn initial_tech_level(_t: ShipType) -> u32 {
    0
}

/// The maximum tech level of a type.
pub fn max_tech_level(t: ShipType) -> i32 {
    MAX_TECH_LEVEL[t.index()]
}

/// The upgrade bits a type can have.
pub fn type_mask(t: ShipType) -> u32 {
    TYPE_MASK[t.index()]
}

/// `suShipApplyUpgrades` (0x50f660) on a ship not yet in the universe.
pub fn apply_to_ship(ship: &mut Ship, base: &BaseState, stat: &ShipStaticInfo) {
    let Some(p) = base.players.get(ship.player) else { return };
    let level = p.max_tech_level.min(max_tech_level(ship.ship_type));
    ship.armour_level = level.max(0) as u32;
    ship.upgrade_flags = type_mask(ship.ship_type) & p.upgrade_bits | base.static_upgrade_bits.get(ship.ship_type.index()).copied().unwrap_or(0);
    if let Some(statics) = stat.guns.as_ref() {
        for (g, gs) in ship.guns.iter_mut().zip(statics) {
            if g.level == 0 && gs.end_upgrade_level == WeaponType::Energy && p.gun_types[1] != 0 {
                g.level = 1;
            }
        }
    }
}

/// `suShipApplyUpgrades` on a ship in the universe.
pub fn apply_upgrades(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let Some(stat) = statics.ship(t) else { return };
    let base = std::mem::take(&mut u.base);
    apply_to_ship(u.ships.ship_mut(id), &base, stat);
    u.base = base;
}

/// `suShipHasUpgrade`.
pub fn has_upgrade(s: &Ship, bit: u32) -> bool {
    s.upgrade_flags & bit != 0
}

/// `suPlayerUpdateFromResearch` (0x50f310): tech level, gun types and upgrade bits from the
/// player's technologies.
pub fn player_update_from_research(u: &mut Universe, player: usize) {
    let p = &mut u.base.players[player];
    let tech = p.tech;
    p.max_tech_level = if tech.contains(0xf) {
        2
    } else if tech.contains(0xe) {
        1
    } else {
        0
    };
    for (i, &(t, _)) in GUN_TYPE_TECHS.iter().enumerate() {
        p.gun_types[i] = i32::from(t == 41 || tech.contains(t));
    }
    for &(t, a, b) in &UPGRADE_BIT_TECHS {
        if tech.contains(t) {
            p.upgrade_bits |= a | b;
        }
    }
}

/// `suInitStaticUpgradeBits` (0x50fcc0): the default upgrade bits of a mission ship's type,
/// from the technologies its type depends on.
pub fn init_static_upgrade_bits(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship(id);
    let t = s.ship_type;
    if t.index() >= 0x3c {
        return;
    }
    let depend = ship_depend(u, s.player, t, s.ship_race);
    let mut bits = 0u32;
    for &(tech, a, b) in &UPGRADE_BIT_TECHS {
        if depend.contains(tech) {
            bits |= a | b;
        }
    }
    u.base.static_upgrade_bits[t.index()] = type_mask(t) & bits;
}

/// The ship-dependency technology set of a type in the player's research table (the Somtaaw
/// multiplayer table for Beast types not yet acquired).
pub fn ship_depend(u: &Universe, player: usize, t: ShipType, race: ShipRace) -> Techs {
    let p = &u.base.players[player];
    let table = crate::research::table_for(u, p.research_table);
    if race == ShipRace::Beast && !p.acquired[t.index()] {
        let mapped = crate::research::appropriate_type_for_race(t, ShipRace::Somtaaw);
        return crate::research::depend_set(&u.statics.research.multiplayer_somtaaw, mapped);
    }
    crate::research::depend_set(table, t)
}
