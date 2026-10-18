//! Speed limits: `tacticsGetShipsMaxVelocity` and Cataclysm's speed multiplier (0x50fb10,
//! engine upgrades and damage).

use hwc_math::fpu::{ld, st, Ext};

use crate::object::{special, Ship};
use crate::statics::{ShipStaticInfo, ShipType};

/// Speed multiplier per engine upgrade level (table at 0x721b64).
pub const SPEED_UPGRADE_TABLE: [f32; 5] = [1.0, 1.1, 1.2, 1.3, 1.4];

/// Cataclysm's speed multiplier (0x50fb10): the engine upgrade factor (+0.15 for some upgrade
/// bits), plus the Ramming Frigate's ram bonus, minus a damage penalty from the health ratio.
/// Returned as a register value.
///
/// The Beast mothership's damage modifier and the ram bonus are not modelled (both are 0 for
/// every ship this slice creates).
pub fn speed_multiplier(ship: &Ship, stat: &ShipStaticInfo) -> Ext {
    let level = (ship.speed_upgrade_level as usize).min(SPEED_UPGRADE_TABLE.len() - 1);
    let mut base = SPEED_UPGRADE_TABLE[level];
    if ship.upgrade_flags & 7 != 0 {
        base = st(ld(base) + ld(f32::from_bits(0x3e19_999a)));
    }
    // + RammingFrigateGetRamSpeedBonus (0.0 when not ramming)
    let r = ld(base) + 0.0;
    // The ratio is compared once straight from the register, then reloaded as a float.
    let ratio_ext = ld(ship.health) / ld(stat.health.max_health);
    let ratio = st(ratio_ext);
    if ratio_ext < ld(f32::from_bits(0x3dcc_cccd)) {
        return r - ld(f32::from_bits(0x3f19_999a));
    }
    if ld(ratio) < 0.25 {
        return r - ld(f32::from_bits(0x3ecc_cccd));
    }
    if ld(ratio) < 0.5 {
        return r - ld(f32::from_bits(0x3e4c_cccd));
    }
    if ld(ratio) < 0.75 {
        return r - ld(f32::from_bits(0x3dcc_cccd));
    }
    r
}

/// Inputs from outside the ship that `tacticsGetShipsMaxVelocity` looks at.
#[derive(Debug, Clone, Copy, Default)]
pub struct MaxVelocityContext {
    /// The ship's command is a formation of type sphere.
    pub in_sphere_formation: bool,
    /// Global factor used when special flags 2 and 8 are both set (0xa58888).
    pub special_factor: f32,
}

/// `tacticsGetShipsMaxVelocity` (0x524510), as a register value.
pub fn ships_max_velocity(ship: &Ship, stat: &ShipStaticInfo, ctx: MaxVelocityContext) -> Ext {
    let mut factor = 1.0;
    if ctx.in_sphere_formation {
        factor = ld(f32::from_bits(0x3f33_3333));
    }
    let sf = ship.special_flags;
    if sf & 2 != 0 && sf & 8 != 0 {
        factor = ld(ctx.special_factor);
    }
    if sf & special::CUSTOM_MAX_VELOCITY != 0 {
        return ld(ship.special_max_velocity);
    }
    factor * ld(stat.header.max_velocity)
}

/// The capped speed used by the physics and the fly-to code:
/// `(float)(maxvel * (float)speed_multiplier)`.
pub fn capped_speed(ship: &Ship, stat: &ShipStaticInfo, ctx: MaxVelocityContext) -> f32 {
    let mult = st(speed_multiplier(ship, stat));
    st(ships_max_velocity(ship, stat, ctx) * ld(mult))
}

/// `isStrikeCraft` (Cataclysm's list).
pub fn is_strike_craft(t: ShipType) -> bool {
    matches!(t.index(), 0..=5 | 0x21..=0x26 | 0x3d | 0x41 | 0x43 | 0x44 | 0x46 | 0x48 | 0x4b | 0x51 | 0x52
        | 0x54 | 0x56 | 0x59 | 0x5d | 0x5f | 0x61)
}

/// `isCapitalShipStaticOrBig`: `isCapitalShip`, or a cloak or gravity-well generator.
pub fn is_capital_ship_static_or_big(stat: &ShipStaticInfo) -> bool {
    stat.is_capital_ship
        || matches!(
            stat.ship_type,
            ShipType::kCloakGenerator | ShipType::kGravWellGenerator | ShipType::tCloakGenerator | ShipType::tGravWellGenerator
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strike_craft_matches_static_list() {
        // The tactics list and the statics' strike-craft list are the same 27 types.
        for t in ShipType::ALL {
            assert_eq!(is_strike_craft(*t), t.is_strike_craft(), "{t}");
        }
    }
}
