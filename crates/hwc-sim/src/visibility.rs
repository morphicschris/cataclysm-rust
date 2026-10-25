//! Per-player visibility of stealthy ships (Cataclysm's `univShipVisibleToPlayer` family,
//! 0x53aaa0..0x53b0e0): Mimics and MCVs (and the P1 standard corvette), the Leech and the
//! cloakers keep one visibility flag per player. Allies always see them; ships with the
//! detector capability and the sensors upgrade (technology 0x1c) reveal them within their
//! vision range. The flags are refreshed every 16 frames for each such ship.

use hwc_math::fpu::ld;

use crate::object::ShipId;
use crate::statics::ShipType;
use crate::universe::{SpecialEvent, Universe};

/// Capability bit: the type detects stealthy ships (static +0x1ec).
pub const CAP_DETECTOR: u32 = 0x10000;
/// Capability bit: the type has per-player visibility.
pub const CAP_PER_PLAYER_VISIBILITY: u32 = 0x20000;
/// The upgrade bit a detector needs (from technology 0x1c).
pub const UPGRADE_DETECTOR: u32 = 0x80_0000;
/// The technology that scales vision ranges.
pub const TECH_SENSORS: usize = 0x1c;
/// Vision range factor with that technology (0x889b74).
pub const SENSORS_RANGE_FACTOR: f32 = 1.3;

/// The kinds of per-player visibility tables.
fn has_table(t: ShipType) -> bool {
    matches!(
        t,
        ShipType::sMimic | ShipType::sMCV | ShipType::p1StandardCorvette | ShipType::sLeech | ShipType::bCloakedFighter
            | ShipType::bIonArrayFrigate | ShipType::p1IonArrayFrigate
    )
}

fn is_mimic(t: ShipType) -> bool {
    matches!(t, ShipType::sMimic | ShipType::sMCV | ShipType::p1StandardCorvette)
}

/// `univShipVisibleToPlayer` (0x53aaa0).
pub fn visible_to_player(u: &Universe, id: ShipId, player: usize) -> bool {
    let Some(s) = u.ships.get(id) else { return true };
    if player >= 8 {
        return true;
    }
    let caps = u.statics.ship(s.ship_type).map_or(0, |st| st.capability_flags);
    if caps & CAP_PER_PLAYER_VISIBILITY == 0 || !has_table(s.ship_type) {
        return true;
    }
    s.visible_to[player]
}

/// `univShipVisibilityForPlayer` (0x53afe0): 1 own ship, 2 hidden from the player, else 0.
pub fn visibility_for_player(u: &Universe, id: ShipId, player: usize) -> u32 {
    let Some(s) = u.ships.get(id) else { return 0 };
    if s.player == player {
        return 1;
    }
    if !visible_to_player(u, id, player) {
        return 2;
    }
    0
}

/// `univShipSetVisibleToPlayer` (0x53ab80). A Mimic hidden from a player stays visible to
/// that player unless its disguise actually fools them. `quiet` suppresses the "enemy
/// sighted" announcement.
pub fn set_visible(u: &mut Universe, id: ShipId, player: usize, visible: bool, quiet: bool) {
    if player >= 8 {
        return;
    }
    let Some(s) = u.ships.get(id) else { return };
    let t = s.ship_type;
    if !has_table(t) {
        return;
    }
    let was = s.visible_to[player];
    let announce = !was && visible && !quiet && match t {
        ShipType::sLeech => true,
        ShipType::bCloakedFighter | ShipType::bIonArrayFrigate | ShipType::p1IonArrayFrigate => crate::cloak::is_cloaked(u.ships.ship(id)),
        _ => crate::ships::mimic::is_disguised(u, id),
    };
    if announce {
        u.special_events.push(SpecialEvent::EnemyShipSighted { ship: id, player });
    }
    u.ships.ship_mut(id).visible_to[player] = visible;
    // The disguise checks read the flag just stored (they need the ship hidden from the player).
    if is_mimic(t) && !visible {
        let fooled = crate::ships::mimic::is_disguised_as_player(u, id, player)
            || crate::ships::mimic::fools_player_as_resource(u, id, player)
            || crate::ships::mimic::fools_player_as_derelict(u, id, player);
        u.ships.ship_mut(id).visible_to[player] = !fooled;
    }
}

/// `univShipInitVisibility` (0x53ad90): Mimics and cloakers start visible, the Leech hidden.
pub fn init(u: &mut Universe, id: ShipId) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    if !has_table(t) {
        return;
    }
    let v = t != ShipType::sLeech;
    for p in 0..8 {
        set_visible(u, id, p, v, true);
    }
}

/// `suGetShipVisionRange` (0x50fc00): the type's `fowRadius`, x1.3 when its player has the
/// sensors technology (x0.125 for non-recons in single-player mission 6).
pub fn vision_range(u: &Universe, id: ShipId) -> f64 {
    let Some(s) = u.ships.get(id) else { return 0.0 };
    let r = u.statics.ship(s.ship_type).map_or(0.0, |st| st.fow_radius);
    let mut v = if u.base.players.get(s.player).is_some_and(|p| p.tech.contains(TECH_SENSORS)) {
        ld(SENSORS_RANGE_FACTOR) * ld(r)
    } else {
        ld(r)
    };
    if u.single_player && u.base.options.mission == 6 && s.ship_type != ShipType::sRecon {
        v *= 0.125;
    }
    v
}

/// `univShipUpdateVisibility` (0x53ae40): allies and nearby detectors see the ship.
pub fn update(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let owner = s.player;
    let pos = s.posinfo.position;
    let n = u.players.len().min(8);
    let mut seen = [false; 8];
    for (p, slot) in seen.iter_mut().enumerate().take(n) {
        let allied = u.players[owner].is_ally(p);
        if allied && (!u.single_player || u.base.options.mission != 14 || owner != 1) {
            *slot = true;
        }
    }
    for &d in &u.ship_list {
        let Some(ds) = u.ships.get(d) else { continue };
        let caps = u.statics.ship(ds.ship_type).map_or(0, |st| st.capability_flags);
        if caps & CAP_DETECTOR == 0 || !crate::upgrade::has_upgrade(ds, UPGRADE_DETECTOR) || ds.player >= 8 || seen[ds.player] {
            continue;
        }
        let r = vision_range(u, d);
        let dx = ld(pos.x) - ld(ds.posinfo.position.x);
        let dy = ld(pos.y) - ld(ds.posinfo.position.y);
        let dz = ld(pos.z) - ld(ds.posinfo.position.z);
        if (dx * dx + dy * dy) + dz * dz < r * r {
            seen[ds.player] = true;
        }
    }
    let leech = u.ships.ship(id).ship_type == ShipType::sLeech;
    for (p, &v) in seen.iter().enumerate().take(n) {
        if v || leech {
            set_visible(u, id, p, v, false);
        }
    }
}
