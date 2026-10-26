//! Speed limits (`tacticsGetShipsMaxVelocity` and Cataclysm's speed multiplier, 0x50fb10) and
//! the tactics-driven retaliation of ships that see enemies or are shot at.

use hwc_math::fpu::{ld, st, Ext};

use hwc_math::Vector;

use crate::command::{self, attr, order};
use crate::object::{sof, special, CommandId, Ship, ShipId};
use crate::universe::Universe;
use crate::statics::{ShipStaticInfo, ShipType};

/// Speed multiplier per engine upgrade level (table at 0x721b64).
pub const SPEED_UPGRADE_TABLE: [f32; 5] = [1.0, 1.1, 1.2, 1.3, 1.4];

/// Cataclysm's speed multiplier (0x50fb10): the engine upgrade factor (+0.15 for some upgrade
/// bits), plus the Ramming Frigate's ram bonus, minus a damage penalty from the health ratio.
/// Returned as a register value.
///
/// The Beast mothership adds its speed damage mode (`MothershipGetDamageModifier1`); the ram
/// bonus is `RammingFrigateGetRamSpeedBonus`.
pub fn speed_multiplier(ship: &Ship, stat: &ShipStaticInfo) -> Ext {
    let level = (ship.speed_upgrade_level as usize).min(SPEED_UPGRADE_TABLE.len() - 1);
    let mut base = SPEED_UPGRADE_TABLE[level];
    if ship.upgrade_flags & 7 != 0 {
        base = st(ld(base) + ld(f32::from_bits(0x3e19_999a)));
    }
    if ship.ship_type == ShipType::bMothership {
        base = st(ld(crate::ships::mothership::speed_modifier_of(ship)) + ld(base));
    }
    let r = crate::ships::ramming::ram_speed_bonus(ship) + ld(base);
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
#[derive(Debug, Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
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

// --- retaliation (Tactics.c / univCheckShipState)

/// Most enemies `getEnemiesWithinProximity` reports.
const MAX_PROXIMITY_ENEMIES: usize = 6;

/// `FUN_00525120`: a ship counts as a target for `player` (visible, not a special object, inside
/// the world). Visibility is not modelled (always visible).
pub fn is_valid_target(u: &Universe, t: ShipId) -> bool {
    let Some(s) = u.ships.get(t) else { return false };
    let Some(stat) = u.statics.ship(s.ship_type) else { return false };
    // Crystals, ships being converted by the Beast (+0x4e4) and the other untargetables.
    // (+0x3ec, a reference the dying-ship cleanup clears, is not modelled.)
    if crate::ships::crystal::is_crystal_type(s.ship_type) || s.beast.timer != 0.0 {
        return false;
    }
    if stat.capability_flags & 0x4000 != 0 || s.special_flags2 & 0x0200_0000 != 0 {
        return false;
    }
    if matches!(s.ship_type.index(), 0x6e | 0x6f | 0x73..=0x7b | 0x8d) {
        return false;
    }
    let b = u.world_bounds;
    let p = s.posinfo.position;
    !(p.x < -b.x || b.x < p.x || p.y < -b.y || b.y < p.y || p.z < -b.z || b.z < p.z)
}

fn is_enemy(u: &Universe, me: &Ship, other: &Ship) -> bool {
    !u.players.get(me.player).is_some_and(|p| other.player == me.player || p.is_ally(other.player))
}

/// `getEnemiesWithinProximity` (0x545430): up to six enemies of the ship's blob inside the box
/// of half-size `dist` around it. (The detection-time gates of `TW_*_SEE_*` let everything
/// through: detection times are not modelled.)
pub fn enemies_within_proximity(u: &Universe, id: ShipId, dist: f32) -> Vec<ShipId> {
    let Some(me) = u.ships.get(id) else { return Vec::new() };
    let Some(blob) = me.coll_my_blob.and_then(|b| u.blobs.get(b)) else { return Vec::new() };
    if me.ship_type == ShipType::trFloatingCity || me.special_flags & 0x4_0000 != 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for &o in &blob.ships {
        if out.len() >= MAX_PROXIMITY_ENEMIES {
            break;
        }
        let Some(os) = u.ships.get(o) else { continue };
        if !is_valid_target(u, o) || !is_enemy(u, me, os) || os.special_flags & 0x24_0000 != 0 {
            continue;
        }
        // Stealthy ships hidden from the player (cloaked, undetected Leeches, disguised Mimics)
        // are not seen (`FUN_00525120` and the explicit `univShipVisibleToPlayer` test).
        if !crate::visibility::visible_to_player(u, o, me.player) {
            continue;
        }
        if os.flags & (sof::HIDE | sof::DISABLED) != 0 || matches!(os.ship_type, ShipType::sMothership | ShipType::bMothership) {
            continue;
        }
        let d = [
            ld(os.posinfo.position.x) - ld(me.posinfo.position.x),
            ld(os.posinfo.position.y) - ld(me.posinfo.position.y),
            ld(os.posinfo.position.z) - ld(me.posinfo.position.z),
        ];
        let r = ld(dist);
        if d.iter().all(|&c| -r < c && c < r) {
            out.push(o);
        }
    }
    out
}

/// `getShipAndItsFormation`: the ships of a ship's formation, or the ship alone.
pub fn ship_and_its_formation(u: &Universe, id: ShipId) -> Vec<ShipId> {
    match u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)) {
        Some(c) if c.attributes & attr::FORMATION != 0 => c.selection.clone(),
        _ => vec![id],
    }
}

/// The ship that answers for a ship in formation: its leader (`FUN_00524580`).
fn responsible_ship(u: &Universe, id: ShipId) -> ShipId {
    match u.ships.get(id).and_then(|s| s.command).and_then(|c| u.commands.get(c)) {
        Some(c) if c.attributes & attr::FORMATION != 0 && !c.selection.is_empty() => c.selection[0],
        _ => id,
    }
}

/// `FUN_00524cc0`: a ship (or its formation leader) reacts to `enemies`.
pub fn retaliate(u: &mut Universe, id: ShipId, enemies: Vec<ShipId>) {
    let me = responsible_ship(u, id);
    let cmd = u.ships.get(me).and_then(|s| s.command);
    react_to_enemies(u, me, cmd, enemies);
}

/// Ships that may not be pulled into a retaliation (`FUN_005252b0`).
fn can_join_retaliation(u: &Universe, id: ShipId) -> bool {
    let s = u.ships.ship(id);
    s.special_flags2 & 8 == 0 && s.special_flags & 0x80 == 0 && !crate::ships::mothership::big_gun_is_active(u, id)
}

/// `FUN_00524d50`: the reaction by tactics: evasive ships flee (and fire back), neutral ships
/// fire back, aggressive ships attack (fire back only while guarding).
fn react_to_enemies(u: &mut Universe, id: ShipId, cmd: Option<CommandId>, enemies: Vec<ShipId>) {
    if enemies.is_empty() {
        return;
    }
    let s = u.ships.ship(id);
    if s.special_flags & 0x80 != 0 || s.flags & sof::HIDE != 0 {
        return;
    }
    let mut protecting = false;
    if let Some(c) = cmd.and_then(|c| u.commands.get(c)) {
        if c.selection.len() > 1 && c.selection.iter().any(|&o| u.ships.get(o).is_some_and(|os| os.special_flags & 0x80 != 0)) {
            return;
        }
        if matches!(c.order, 2 | 3 | 4 | 5 | 7 | 8 | 10 | 11 | 12 | 15 | 17) {
            return;
        }
        protecting = c.attributes & attr::PROTECTING != 0;
    }
    // An Acolyte that is linking with a partner (+0x650) does not react. (So does a Mimic
    // with +0x650 set, a field of its own that is not modelled; Mimics have no guns.)
    let s = u.ships.ship(id);
    if crate::ships::acolyte::is_link_acolyte(s.ship_type) && s.base.link.partner.is_some() {
        return;
    }
    match u.ships.ship(id).tactics {
        crate::object::tactics::EVASIVE => evasive_reaction(u, id, cmd, enemies),
        crate::object::tactics::NEUTRAL => passive_reaction(u, id, cmd, enemies),
        _ => {
            if protecting {
                passive_reaction(u, id, cmd, enemies);
            } else {
                aggressive_reaction(u, id, cmd, enemies);
            }
        }
    }
}

fn selection_of(u: &Universe, id: ShipId, cmd: Option<CommandId>) -> Vec<ShipId> {
    match cmd.and_then(|c| u.commands.get(c)) {
        Some(c) => c.selection.clone(),
        None => vec![id],
    }
}

/// `FUN_005247b0`: fire back while carrying on.
fn passive_reaction(u: &mut Universe, id: ShipId, cmd: Option<CommandId>, mut enemies: Vec<ShipId>) {
    enemies.retain(|&e| is_valid_target(u, e));
    if enemies.is_empty() || !crate::attack::can_attack_passive(u, id) {
        return;
    }
    // `FUN_00524680`: the command's targets plus the enemies it does not have yet; when it
    // already has targets and none is new, nothing changes.
    let mut targets: Vec<ShipId> = cmd.and_then(|c| u.commands.get(c)).map(|c| c.attack_targets.clone()).unwrap_or_default();
    let had = targets.len();
    for e in enemies {
        if !targets[..had].contains(&e) {
            targets.push(e);
        }
    }
    if had > 0 && targets.len() == had {
        return;
    }
    let sel = selection_of(u, id, cmd);
    if command::ships_in_passive_attack_range(u, &sel, &targets) {
        command::cl_passive_attack(u, &sel, &targets);
    }
}

/// `FUN_00524980`: move away from the enemies (by the ship's sensor radius), and fire back.
fn evasive_reaction(u: &mut Universe, id: ShipId, cmd: Option<CommandId>, mut enemies: Vec<ShipId>) {
    let docking = cmd.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == order::DOCK);
    if !docking {
        enemies.retain(|&e| is_valid_target(u, e));
        if !enemies.is_empty() {
            let mut sel = selection_of(u, id, cmd);
            sel.retain(|&s| !matches!(u.ships.ship(s).ship_type, ShipType::sMothership | ShipType::p1Mothership) && can_join_retaliation(u, s));
            let player = sel.first().map(|&s| u.ships.ship(s).player);
            sel.retain(|&s| Some(u.ships.ship(s).player) == player);
            if !sel.is_empty() {
                let (mut ax, mut ay, mut az) = (0f32, 0f32, 0f32);
                for &e in &enemies {
                    let p = u.ships.ship(e).posinfo.position;
                    ax = st(ld(p.x) + ld(ax));
                    ay = st(ld(p.y) + ld(ay));
                    az = st(ld(p.z) + ld(az));
                }
                if enemies.len() > 1 {
                    let k = st(1.0 / ld(enemies.len() as i32 as f32));
                    ax = st(ld(k) * ld(ax));
                    ay = st(ld(k) * ld(ay));
                    az = st(ld(k) * ld(az));
                }
                let s = u.ships.ship(id);
                let dist = u.statics.ship(s.ship_type).map_or(0.0, |st| st.fow_radius);
                let p = s.posinfo.position;
                let mut away = Vector::new(st(ld(p.x) - ld(ax)), st(ld(p.y) - ld(ay)), st(ld(p.z) - ld(az)));
                away.normalize();
                let b = u.world_bounds;
                let clamp = |v: f32, m: f32| if m < v { m } else if v < -m { -m } else { v };
                let to = Vector::new(
                    clamp(st(ld(away.x) * ld(dist) + ld(p.x)), b.x),
                    clamp(st(ld(st(ld(away.y) * ld(dist))) + ld(p.y)), b.y),
                    clamp(st(ld(st(ld(away.z) * ld(dist))) + ld(p.z)), b.z),
                );
                command::cl_move(u, &sel, 0, p, &[to]);
            }
        }
    }
    passive_reaction(u, id, cmd, enemies);
}

/// The aggressive reaction: the selection attacks the enemies.
fn aggressive_reaction(u: &mut Universe, id: ShipId, cmd: Option<CommandId>, mut enemies: Vec<ShipId>) {
    enemies.retain(|&e| is_valid_target(u, e));
    if enemies.is_empty() {
        return;
    }
    let mut sel = command::attack_capable(u, &selection_of(u, id, cmd));
    sel.retain(|&s| can_join_retaliation(u, s));
    let player = sel.first().map(|&s| u.ships.ship(s).player);
    sel.retain(|&s| Some(u.ships.ship(s).player) == player);
    if sel.is_empty() {
        return;
    }
    let attacking = cmd.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == order::ATTACK);
    if !attacking {
        command::cl_attack(u, &sel, &enemies);
    }
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
