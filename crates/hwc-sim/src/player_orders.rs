//! The orders the local player gives the selection from the special-action menu and the
//! ship-order keys (Cataclysm's `mr*` order functions: `mrLinkShips`, `mrUnlinkShips`,
//! `mrUpgradeShips`, `mrSpecialTargetOrder`, `mrCloakOn`...), what that menu offers
//! (`selGetCommandAvailability` and `pmBuildMenuItems`) and how its items run
//! (`pmExecuteSpecialItem`).
//!
//! The selection filters keep the original's in-place swap removal, so the order of the
//! ships left (which decides, for example, which Acolyte of a pair flies to the other) is the
//! original's.

use crate::object::{flags2, sof, CommandId, ShipId};
use crate::ships::{acolyte, acv, beast, crystal, hive, leech, mimic, mothership};
use crate::statics::{ShipClass, ShipType};
use crate::universe::Universe;
use crate::upgrade::has_upgrade;

/// Indices of the 41-entry availability table (`selGetCommandAvailability`). For a selection
/// an entry says some ship can do it, the `ALL_*` entries that every ship can; for targets
/// the same table says what they are.
pub mod avail {
    /// A worker able to harvest.
    pub const HARVEST: usize = 0;
    /// Salvage capable (upgrade 0x80000).
    pub const SALVAGE: usize = 1;
    /// Repair capable (upgrade 0x40000, not the processors).
    pub const REPAIR: usize = 2;
    /// Armed and not holding a sentinel field.
    pub const ATTACK: usize = 3;
    /// A ship of a player not allied with the local one.
    pub const ENEMY: usize = 4;
    /// A ship of the local player or an ally.
    pub const ALLY: usize = 5;
    /// Looks like a resource.
    pub const RESOURCE: usize = 6;
    /// Something that can be salvaged.
    pub const SALVAGEABLE: usize = 7;
    pub const SLIPGATE_SMALL: usize = 8;
    pub const SLIPGATE_MEDIUM: usize = 9;
    pub const SLIPGATE_LARGE: usize = 10;
    /// Swarmer launch (upgrade 0x800).
    pub const HIVE: usize = 0xb;
    pub const HIVE_LAUNCHING: usize = 0xc;
    /// EMP (upgrade 0x200).
    pub const EMP: usize = 0xd;
    /// Leech (upgrade 0x40).
    pub const LEECH: usize = 0xe;
    pub const LEECH_ACTIVE: usize = 0xf;
    pub const BIG_GUN: usize = 0x10;
    pub const BIG_GUN_CHARGING: usize = 0x11;
    /// Can link (upgrade 0x8: Acolytes and Mimics with `TECH_LINK`).
    pub const LINK: usize = 0x12;
    /// Can split (upgrade 0x10: ACVs and MCVs).
    pub const UNLINK: usize = 0x13;
    /// Beast infection cascade ready.
    pub const BEAST_CASCADE: usize = 0x14;
    /// Ramming frigates.
    pub const RAMMING: usize = 0x15;
    /// Mimic (upgrade 0x20).
    pub const MIMIC: usize = 0x16;
    /// An MCV among the Mimics.
    pub const MIMIC_MCV: usize = 0x17;
    pub const MIMIC_DISGUISED: usize = 0x18;
    /// Repulse (upgrade 0x400).
    pub const REPULSE: usize = 0x19;
    /// Force field (upgrades 0x1000, 0x2000, 0x4000).
    pub const FORCE_FIELD: usize = 0x1a;
    pub const FORCE_FIELD_ON: usize = 0x1b;
    pub const CLOAK: usize = 0x1c;
    pub const CLOAKED: usize = 0x1d;
    pub const LINK_MISSILES: usize = 0x1f;
    pub const ALL_LEECH: usize = 0x20;
    pub const ALL_EMP: usize = 0x21;
    pub const ALL_MIMIC: usize = 0x22;
    pub const ALL_HIVE: usize = 0x23;
    pub const ALL_RAMMING: usize = 0x24;
    pub const ALL_BEAST_CASCADE: usize = 0x25;
    pub const ALL_LINK_MISSILES: usize = 0x26;
    pub const ALL_FORCE_FIELD: usize = 0x27;
    pub const LEN: usize = 0x29;
}

/// One availability table.
pub type Availability = [bool; avail::LEN];

/// The original's in-place filter: a ship that fails is replaced by the last one.
fn retain_swap(list: &mut Vec<ShipId>, mut keep: impl FnMut(ShipId) -> bool) {
    let mut i = 0;
    while i < list.len() {
        if keep(list[i]) {
            i += 1;
        } else {
            list.swap_remove(i);
        }
    }
}

/// `makeShipsNotIncludeDisabled` (0x50d800): disabled ships leave (Leeches and Sentinels
/// stay).
pub fn not_disabled(u: &Universe, list: &mut Vec<ShipId>) {
    retain_swap(list, |id| {
        u.ships.get(id).is_some_and(|s| !s.base.disabled || matches!(s.ship_type, ShipType::sLeech | ShipType::sSentinel | ShipType::bSentinel))
    });
}

/// `makeShipsHaveUpgrade` (0x50d6f0).
pub fn with_upgrade(u: &Universe, list: &mut Vec<ShipId>, bit: u32) {
    retain_swap(list, |id| u.ships.get(id).is_some_and(|s| has_upgrade(s, bit)));
}

fn is_ally(u: &Universe, id: ShipId, player: usize) -> bool {
    crate::shockwave::is_ally(u, id, player)
}

fn untargetable_type(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.capability_flags & 0x4000 != 0)
}

/// `selFowPointHidden(point, FALSE)`: no ship of the player sees the point.
fn fow_hidden(u: &Universe, player: usize, id: ShipId) -> bool {
    let Some(Some(g)) = u.fog.grids.get(player) else { return false };
    let Some(s) = u.ships.get(id) else { return true };
    let n = g.point_viewer_count(s.posinfo.position);
    n < 1 && n != crate::fow::OUTSIDE
}

/// `selGetCommandAvailability` (0x50ebd0) for `player`.
pub fn command_availability(u: &Universe, player: usize, ships: &[ShipId]) -> Availability {
    use avail::*;
    let mut t = [false; LEN];
    if !ships.is_empty() {
        t[ALL_LEECH..=ALL_FORCE_FIELD].fill(true);
    }
    for &id in ships {
        let Some(s) = u.ships.get(id) else { continue };
        let st = u.statics.ship(s.ship_type);
        let ti = s.ship_type.index();
        if !is_ally(u, id, player) {
            if mimic::fools_player_as_resource(u, id, player) {
                t[RESOURCE] = true;
            } else if mimic::is_disguised_as_player(u, id, player) {
                t[ALLY] = true;
            } else if !crystal::is_crystal(u, id) && !untargetable_type(u, id) && s.flags2 & flags2::PERMANENTLY_DISABLED == 0 && ti != 0x8d {
                t[ENEMY] = true;
            }
        } else {
            t[ALLY] = true;
        }
        if has_upgrade(s, 0x4_0000) && !matches!(s.ship_type, ShipType::sProcessor | ShipType::bProcessor) {
            t[REPAIR] = true;
        }
        if has_upgrade(s, crate::upgrade::BIT_SALVAGE) {
            t[SALVAGE] = true;
        }
        if st.is_some_and(|st| st.ship_class == Some(ShipClass::Resource) && st.harvest_amount > 0)
            && !s.base.disabled
            && matches!(s.ship_type, ShipType::sWorker | ShipType::bWorker)
        {
            t[HARVEST] = true;
        }
        if !crate::command::attack_capable(u, &[id]).is_empty() && !s.base.disabled {
            t[ATTACK] = true;
        }
        if crate::salvage::can_salvage_target(u, player, id) {
            t[SALVAGEABLE] = true;
        }
        match ti {
            0x70 => t[SLIPGATE_SMALL] = true,
            0x71 => t[SLIPGATE_MEDIUM] = true,
            0x72 => t[SLIPGATE_LARGE] = true,
            _ => {}
        }
        if has_upgrade(s, 0x200) {
            t[EMP] = true;
        } else {
            t[ALL_EMP] = false;
        }
        if has_upgrade(s, 0x800) {
            t[HIVE] = true;
            if hive::is_launching(u, id) {
                t[HIVE_LAUNCHING] = true;
            }
        } else {
            t[ALL_HIVE] = false;
        }
        if mothership::has_big_gun_module(u, id) {
            t[BIG_GUN] = true;
            if mothership::big_gun_is_charging(u, id) {
                t[BIG_GUN_CHARGING] = true;
            }
        }
        if has_upgrade(s, crate::link::UPGRADE_LINK) {
            t[LINK] = true;
        } else if has_upgrade(s, crate::link::UPGRADE_UNLINK) {
            t[UNLINK] = true;
        }
        if has_upgrade(s, 0x40) {
            t[LEECH] = true;
            if leech::is_active(u, id) {
                t[LEECH_ACTIVE] = true;
            }
        } else {
            t[ALL_LEECH] = false;
        }
        if has_upgrade(s, 0x20) {
            t[MIMIC] = true;
            if mimic::is_disguised(u, id) {
                t[MIMIC_DISGUISED] = true;
            }
            if s.ship_type == ShipType::sMCV {
                t[MIMIC_MCV] = true;
            }
        } else {
            t[ALL_MIMIC] = false;
        }
        if has_upgrade(s, 0x400) {
            t[REPULSE] = true;
        }
        if beast::ship_can_cascade_now(u, id) {
            t[BEAST_CASCADE] = true;
        } else {
            t[ALL_BEAST_CASCADE] = false;
        }
        if matches!(s.ship_type, ShipType::sRammingFrigate | ShipType::bRammingFrigate) {
            t[RAMMING] = true;
        } else {
            t[ALL_RAMMING] = false;
        }
        if has_upgrade(s, 0x1000) || has_upgrade(s, 0x2000) || has_upgrade(s, 0x4000) {
            t[FORCE_FIELD] = true;
            let on = s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| {
                c.attributes & crate::command::attr::FORMATION != 0 && matches!(c.formation.formation_type, 9 | 10)
            });
            if on {
                t[FORCE_FIELD_ON] = true;
            }
        } else {
            t[ALL_FORCE_FIELD] = false;
        }
        if has_upgrade(s, 0x2_0000) {
            if crate::cloak::is_cloaked(s) {
                t[CLOAKED] = true;
            } else {
                t[CLOAK] = true;
            }
        }
        if acolyte::has_link_missiles(u, id) {
            t[LINK_MISSILES] = true;
        } else {
            t[ALL_LINK_MISSILES] = false;
        }
    }
    t
}

/// The items of the special-action menu (`PopupMenu` items 25..48), in menu order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum SpecialItem {
    Harvest,
    Repair,
    Salvage,
    SelectLeech,
    Leech,
    Unleech,
    Link,
    Unlink,
    SelectMimic,
    MimicItem,
    UnMimic,
    ForceField,
    UnlinkForceField,
    Cloak,
    UnCloak,
    Swarm,
    CancelSwarm,
    BigGun,
    CancelBigGun,
    Infect,
    Emp,
    Ram,
    Repulse,
    FireMissiles,
}

impl SpecialItem {
    pub const ALL: [SpecialItem; 24] = [
        Self::Harvest,
        Self::Repair,
        Self::Salvage,
        Self::SelectLeech,
        Self::Leech,
        Self::Unleech,
        Self::Link,
        Self::Unlink,
        Self::SelectMimic,
        Self::MimicItem,
        Self::UnMimic,
        Self::ForceField,
        Self::UnlinkForceField,
        Self::Cloak,
        Self::UnCloak,
        Self::Swarm,
        Self::CancelSwarm,
        Self::BigGun,
        Self::CancelBigGun,
        Self::Infect,
        Self::Emp,
        Self::Ram,
        Self::Repulse,
        Self::FireMissiles,
    ];

    /// The menu item number (the `pmExecuteSpecialItem` case).
    pub fn index(self) -> usize {
        25 + self as usize
    }

    /// The item's text (`english.script`'s `strMenuSpecial*Desc`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Harvest => "Harvest",
            Self::Repair => "Repair",
            Self::Salvage => "Salvage",
            Self::SelectLeech => "Select Leech",
            Self::Leech => "Leech",
            Self::Unleech => "Unleech",
            Self::Link => "Link",
            Self::Unlink => "Unlink",
            Self::SelectMimic => "Select Mimic",
            Self::MimicItem => "Mimic",
            Self::UnMimic => "UnMimic",
            Self::ForceField => "Force Field",
            Self::UnlinkForceField => "Unlink Force Field",
            Self::Cloak => "Cloak",
            Self::UnCloak => "UnCloak",
            Self::Swarm => "Swarm",
            Self::CancelSwarm => "Cancel Swarm",
            Self::BigGun => "Siege Cannon",
            Self::CancelBigGun => "Cancel Siege Cannon",
            Self::Infect => "Infect",
            Self::Emp => "EMP",
            Self::Ram => "Ram",
            Self::Repulse => "Repulse",
            Self::FireMissiles => "Fire Missiles",
        }
    }
}

/// `pmBuildMenuItems` (0x4e60d0): the special actions the selection (less its disabled ships)
/// offers, in menu order. With band-boxed `targets` only the targeted actions that suit them
/// stay. Empty means "No Special Action Available...".
pub fn special_menu(u: &Universe, player: usize, selection: &[ShipId], targets: &[ShipId]) -> Vec<SpecialItem> {
    use avail::*;
    use SpecialItem as I;
    let mut sel = selection.to_vec();
    not_disabled(u, &mut sel);
    let a = command_availability(u, player, &sel);
    let tg = command_availability(u, player, targets);
    let untargeted = targets.is_empty();
    let fits = |i: usize| untargeted || tg[i];
    let mut on = Vec::new();
    let mut set = |c: bool, i: I| {
        if c {
            on.push(i);
        }
    };
    set(a[HARVEST] && fits(RESOURCE), I::Harvest);
    set(a[REPAIR] && fits(ALLY), I::Repair);
    set(a[SALVAGE] && fits(SALVAGEABLE), I::Salvage);
    set(a[HIVE] && fits(ENEMY), I::Swarm);
    set(a[EMP] && fits(ENEMY), I::Emp);
    set(a[BEAST_CASCADE] && fits(ENEMY), I::Infect);
    set(a[LEECH] && fits(ENEMY), I::Leech);
    set(a[BIG_GUN] && untargeted, I::BigGun);
    set(a[BIG_GUN_CHARGING] && untargeted, I::CancelBigGun);
    set(a[RAMMING], I::Ram);
    set(a[LINK_MISSILES], I::FireMissiles);
    set(a[LEECH], I::SelectLeech);
    set(a[HIVE_LAUNCHING] && untargeted, I::CancelSwarm);
    set(a[LEECH_ACTIVE] && untargeted, I::Unleech);
    set(a[LINK] && untargeted, I::Link);
    set(a[UNLINK] && untargeted, I::Unlink);
    if a[MIMIC] && untargeted {
        set(true, I::SelectMimic);
        set(true, I::MimicItem);
        set(a[MIMIC_DISGUISED], I::UnMimic);
    }
    set(a[REPULSE] && untargeted, I::Repulse);
    set(a[CLOAKED] && untargeted, I::UnCloak);
    set(a[CLOAK] && untargeted, I::Cloak);
    if a[FORCE_FIELD_ON] {
        set(true, I::UnlinkForceField);
    } else if a[FORCE_FIELD] {
        set(true, I::ForceField);
    }
    SpecialItem::ALL.into_iter().filter(|i| on.contains(i)).collect()
}

/// `clWrapSpecial` (0x44ead0): controllable ships get the special order.
fn wrap_special(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let mut sel = ships.to_vec();
    crate::command::make_ships_controllable(u, &mut sel);
    if !sel.is_empty() {
        crate::ships::cl_special(u, &sel, targets);
    }
}

/// `mrLinkShips` (`mrUnlinkShips` in the symbol names, 0x55ff20): the selected ships with the
/// link upgrade that are not disabled pair up and link (`clLink`).
pub fn mr_link_ships(u: &mut Universe, selection: &[ShipId]) -> Option<CommandId> {
    let mut sel = selection.to_vec();
    with_upgrade(u, &mut sel, crate::link::UPGRADE_LINK);
    not_disabled(u, &mut sel);
    crate::link::cl_link_ships(u, &sel)
}

/// `mrUnlinkShips` (`mrLinkShips` in the symbol names, 0x55ff90): the linked ships of the
/// selection split.
pub fn mr_unlink_ships(u: &mut Universe, selection: &[ShipId]) -> Option<CommandId> {
    let mut sel = selection.to_vec();
    with_upgrade(u, &mut sel, crate::link::UPGRADE_UNLINK);
    not_disabled(u, &mut sel);
    crate::link::cl_unlink_ships(u, &sel)
}

/// `mrUpgradeShips` (0x560000): the selected ships that are not disabled and have upgrades
/// waiting take them, in the field or, for types upgraded by docking, by docking.
pub fn mr_upgrade_ships(u: &mut Universe, selection: &[ShipId]) {
    let mut field = Vec::new();
    let mut by_dock = Vec::new();
    for &id in selection {
        let Some(s) = u.ships.get(id) else { continue };
        if s.base.disabled || !crate::upgrade::can_upgrade(u, id) {
            continue;
        }
        if u.statics.ship(s.ship_type).is_some_and(|st| st.upgrade_by_docking) {
            by_dock.push(id);
        } else {
            field.push(id);
        }
    }
    if !field.is_empty() {
        crate::upgrade::cl_upgrade(u, &field);
    }
    if !by_dock.is_empty() {
        crate::command::make_ships_controllable(u, &mut by_dock);
        if !by_dock.is_empty() {
            crate::dock::cl_dock(u, &by_dock, crate::dock::purpose::DOCK, None);
        }
    }
}

/// Whether some selected ship can take an upgrade (the main menu's flashing "Upgrade").
pub fn any_can_upgrade(u: &Universe, player: usize, selection: &[ShipId]) -> bool {
    selection.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.player == player && !s.base.disabled) && crate::upgrade::can_upgrade(u, id))
}

/// What `mrFindNearestSpecialTarget` looks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TargetKind {
    Salvageable,
    Ally,
    Enemy,
    Empable,
}

/// Whether `id` is an object of the kind for `player` (the test `mrFindNearestSpecialTarget`
/// makes of each candidate, its distance aside).
pub fn is_special_target(u: &Universe, player: usize, id: ShipId, kind: TargetKind) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    if s.flags & (sof::DEAD | sof::HIDE) != 0 {
        return false;
    }
    let enemy = || !is_ally(u, id, player) && !crystal::is_crystal(u, id) && !untargetable_type(u, id);
    match kind {
        TargetKind::Salvageable => crate::salvage::can_salvage_target(u, player, id) && !fow_hidden(u, player, id),
        TargetKind::Ally => is_ally(u, id, player),
        TargetKind::Enemy => enemy() && !fow_hidden(u, player, id),
        TargetKind::Empable => enemy() && acv::can_be_emped(u, id, None) && !fow_hidden(u, player, id),
    }
}

/// `WorkerGetLowestModuleHealth` (0x5c45b0): the health fraction of the most damaged of the
/// ship and its modules.
fn lowest_module_health(u: &Universe, id: ShipId) -> f32 {
    if u.ships.get(id).is_none() {
        return 1.0;
    }
    let m = crate::ships::processor::most_damaged_module(u, id);
    let s = u.ships.ship(m);
    let max = u.statics.ship(s.ship_type).map_or(1.0, |st| st.health.max_health);
    s.health / max
}

/// `mrFindNearestSpecialTarget` (0x55e7d0): the nearest object of the kind to `ship` that
/// `player` can see (for allies, a farther one whose most damaged module is worse off than
/// the nearest so far's wins when it is under 10^6 farther in squared distance).
pub fn find_nearest_special_target(u: &Universe, player: usize, ship: ShipId, kind: TargetKind) -> Option<ShipId> {
    let me = u.ships.get(ship)?.posinfo.position;
    let mut best = 1e21f32;
    let mut found: Option<ShipId> = None;
    for &id in &u.ship_list {
        if id == ship || !is_special_target(u, player, id, kind) {
            continue;
        }
        let s = u.ships.ship(id);
        let p = s.posinfo.position;
        let (dx, dy, dz) = (p.x - me.x, p.y - me.y, p.z - me.z);
        let d = dx * dx + dy * dy + dz * dz;
        if d < best {
            best = d;
            found = Some(id);
        } else if kind == TargetKind::Ally {
            if let Some(f) = found {
                if d - best < 1e6 && lowest_module_health(u, id) < lowest_module_health(u, f) {
                    best = d;
                    found = Some(id);
                }
            }
        }
    }
    found
}

/// The special orders of `mrSpecialTargetOrder` (0x55ea10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TargetOrder {
    Salvage,
    Repair,
    Leech,
    Swarm,
    Emp,
    Infect,
    Ram,
    LinkMissiles,
}

/// `mrSpecialTargetOrder` (0x55ea10): the capable ships of the selection get the special
/// order on `targets`, or, without targets, each on the nearest suitable target of its own.
pub fn special_target_order(u: &mut Universe, player: usize, selection: &[ShipId], targets: Option<&[ShipId]>, order: TargetOrder) {
    let mut sel = selection.to_vec();
    not_disabled(u, &mut sel);
    match order {
        TargetOrder::Salvage => with_upgrade(u, &mut sel, crate::upgrade::BIT_SALVAGE),
        TargetOrder::Repair => retain_swap(&mut sel, |id| {
            u.ships.get(id).is_some_and(|s| has_upgrade(s, 0x4_0000) && !matches!(s.ship_type, ShipType::sProcessor | ShipType::bProcessor))
        }),
        TargetOrder::Leech => retain_swap(&mut sel, |id| u.ships.get(id).is_some_and(|s| has_upgrade(s, 0x40) && (!s.base.disabled || leech::is_active(u, id)))),
        TargetOrder::Swarm => with_upgrade(u, &mut sel, 0x800),
        TargetOrder::Emp => {
            with_upgrade(u, &mut sel, 0x200);
            if sel.is_empty() {
                return;
            }
            retain_swap(&mut sel, |id| !(acv::is_moving_to_emp_target(u, id) || acv::is_firing_emp(u, id)));
        }
        TargetOrder::Infect => with_upgrade(u, &mut sel, 0x1_0000),
        TargetOrder::Ram => retain_swap(&mut sel, |id| {
            u.ships.get(id).is_some_and(|s| matches!(s.ship_type, ShipType::sRammingFrigate | ShipType::bRammingFrigate) && !s.base.disabled)
        }),
        TargetOrder::LinkMissiles => retain_swap(&mut sel, |id| acolyte::has_link_missiles(u, id)),
    }
    if sel.is_empty() {
        return;
    }
    let run = |u: &mut Universe, ships: &[ShipId], t: &[ShipId]| match order {
        TargetOrder::Salvage => crate::salvage::cl_salvage(u, ships, t),
        TargetOrder::Repair => crate::ships::worker_repair::cl_repair(u, ships, t),
        _ => wrap_special(u, ships, t),
    };
    match targets {
        Some(t) => {
            let t: Vec<ShipId> = match order {
                TargetOrder::Salvage => t.iter().copied().filter(|&x| crate::salvage::can_salvage_target(u, player, x)).collect(),
                _ => t.to_vec(),
            };
            if !t.is_empty() {
                run(u, &sel, &t);
            }
        }
        None => {
            let kind = match order {
                TargetOrder::Salvage => TargetKind::Salvageable,
                TargetOrder::Repair => TargetKind::Ally,
                TargetOrder::Emp => TargetKind::Empable,
                _ => TargetKind::Enemy,
            };
            for &ship in &sel {
                if let Some(t) = find_nearest_special_target(u, player, ship, kind) {
                    run(u, &[ship], &[t]);
                }
            }
        }
    }
}

/// `mrSpecialActivate400` (0x560da0): the repulse.
pub fn mr_repulse(u: &mut Universe, selection: &[ShipId]) {
    let mut sel = selection.to_vec();
    with_upgrade(u, &mut sel, 0x400);
    not_disabled(u, &mut sel);
    if !sel.is_empty() {
        wrap_special(u, &sel, &[]);
    }
}

/// `mrCloakOn` / `mrCloakOff` (0x55f240 / 0x55f2d0): each cloak-capable ship not yet cloaked
/// (or cloaked) toggles, one special order each.
pub fn mr_cloak(u: &mut Universe, selection: &[ShipId], on: bool) {
    for &id in selection {
        let Some(s) = u.ships.get(id) else { continue };
        if has_upgrade(s, 0x2_0000) && crate::cloak::is_cloaked(s) != on {
            wrap_special(u, &[id], &[]);
        }
    }
}

/// `mrHiveStopLaunch` (0x55f130): each launching hive frigate stops.
pub fn mr_hive_stop_launch(u: &mut Universe, selection: &[ShipId]) {
    for &id in selection {
        if u.ships.get(id).is_some_and(|s| has_upgrade(s, 0x800)) && hive::is_launching(u, id) {
            wrap_special(u, &[id], &[]);
        }
    }
}

impl SpecialItem {
    /// What the item's context target is (`pmSetContextTarget`'s kinds); `None` for the
    /// items without one.
    pub fn target_kind(self) -> Option<TargetKind> {
        Some(match self {
            Self::Repair => TargetKind::Ally,
            Self::Salvage => TargetKind::Salvageable,
            Self::Leech | Self::Swarm | Self::Infect | Self::Ram | Self::Repulse | Self::FireMissiles => TargetKind::Enemy,
            Self::Emp => TargetKind::Empable,
            _ => return None,
        })
    }
}

/// `pmSetContextTarget` (0x4e5f00): the target a highlighted menu item acts on when no band
/// box gave the menu targets, the nearest suitable object to the first selected ship.
pub fn context_target(u: &Universe, player: usize, selection: &[ShipId], item: SpecialItem) -> Option<ShipId> {
    let kind = item.target_kind()?;
    find_nearest_special_target(u, player, *selection.first()?, kind)
}

/// `kbcbRepair` (0x549a40): the repair key, each repair-capable ship on the ally nearest it
/// (`mrSpecialTargetRepair(NULL)`).
pub fn repair_key(u: &mut Universe, player: usize, selection: &[ShipId]) {
    special_target_order(u, player, selection, None, TargetOrder::Repair);
}

/// `pmExecuteSpecialItem` (0x4e8480) for the items the simulation carries out. `targets` are
/// the band-boxed targets (or the menu's context target); returns false for items it does not
/// handle (the submenus, the siege cannon's targeting, the Mimic and leech toggles, the force
/// field).
pub fn execute_special(u: &mut Universe, player: usize, selection: &[ShipId], item: SpecialItem, targets: &[ShipId]) -> bool {
    use SpecialItem as I;
    let t = (!targets.is_empty()).then_some(targets);
    match item {
        I::Harvest => crate::resource::cl_collect_resource(u, selection, None),
        I::Repair => {
            // Only with a target: the band-boxed allies, else the menu's context target (with
            // neither the item does nothing; the repair key is [`repair_key`]).
            let ally: Vec<ShipId> = targets.iter().copied().filter(|&x| is_ally(u, x, player)).collect();
            if !ally.is_empty() {
                special_target_order(u, player, selection, Some(&ally), TargetOrder::Repair);
            }
        }
        I::Salvage => special_target_order(u, player, selection, t, TargetOrder::Salvage),
        I::Leech => special_target_order(u, player, selection, t, TargetOrder::Leech),
        I::Link => {
            mr_link_ships(u, selection);
        }
        I::Unlink => {
            mr_unlink_ships(u, selection);
        }
        I::Cloak => mr_cloak(u, selection, true),
        I::UnCloak => mr_cloak(u, selection, false),
        I::Swarm => special_target_order(u, player, selection, t, TargetOrder::Swarm),
        I::CancelSwarm => mr_hive_stop_launch(u, selection),
        I::Infect => special_target_order(u, player, selection, t, TargetOrder::Infect),
        I::Emp => special_target_order(u, player, selection, t, TargetOrder::Emp),
        I::Ram => special_target_order(u, player, selection, t, TargetOrder::Ram),
        I::Repulse => mr_repulse(u, selection),
        I::FireMissiles => special_target_order(u, player, selection, t, TargetOrder::LinkMissiles),
        I::UnMimic => mr_mimic_undisguise(u, selection),
        I::SelectLeech | I::Unleech | I::SelectMimic | I::MimicItem | I::ForceField | I::UnlinkForceField | I::BigGun | I::CancelBigGun => {
            return false;
        }
    }
    true
}

/// How many entries of disguise menu `slot` the selection may pick (`MimicGetDisguiseTypeCount`):
/// all of them with an MCV selected, else the first `defaults[slot]` (`SetMCVItemIndex`: the
/// larger shapes need two linked Mimics).
pub fn mimic_entry_count(t: &mimic::DisguiseTables, slot: usize, mcv: bool) -> usize {
    let n = t.entries.get(slot).map_or(0, Vec::len);
    if mcv {
        n
    } else {
        usize::try_from(t.defaults.get(slot).copied().unwrap_or(0)).unwrap_or(0).min(n)
    }
}

/// Whether the selection holds an MCV (`selGetCommandAvailability`'s 0x17 entry).
pub fn selection_has_mcv(u: &Universe, selection: &[ShipId]) -> bool {
    selection.iter().any(|&id| u.ships.get(id).is_some_and(|s| s.ship_type == ShipType::sMCV && has_upgrade(s, 0x20)))
}

/// `pmMimicDisguiseAs` (0x4e8d40): the selection's Mimics and MCVs that are not disabled take
/// entry `index` of disguise menu `slot` (a ship type of that player, or for slot 8 an
/// asteroid); single Mimics only the entries below the menu's MCV index.
pub fn mimic_disguise_as(u: &mut Universe, t: &mimic::DisguiseTables, slot: usize, selection: &[ShipId], index: usize) {
    let Some(&e) = t.entries.get(slot).and_then(|l| l.get(index)) else { return };
    let single = mimic_entry_count(t, slot, false);
    let sel: Vec<ShipId> = selection
        .iter()
        .copied()
        .filter(|&id| {
            u.ships.get(id).is_some_and(|s| {
                !s.base.disabled && (s.ship_type == ShipType::sMCV || (s.ship_type == ShipType::sMimic && index < single))
            })
        })
        .collect();
    if sel.is_empty() {
        return;
    }
    let o = mimic::DisguiseOrder {
        kind: if slot == 8 { 2 } else { 1 },
        ship_type: e.ship_type,
        player: slot as i32,
        asteroid: e.asteroid,
    };
    mimic::cl_disguise(u, &sel, o);
}

/// `mrMimicUndisguise` (0x560120): the disguised ships of the selection with the disguise
/// upgrade, not disabled, drop their disguise.
pub fn mr_mimic_undisguise(u: &mut Universe, selection: &[ShipId]) {
    let mut sel: Vec<ShipId> = selection
        .iter()
        .copied()
        .filter(|&id| u.ships.get(id).is_some_and(|s| has_upgrade(s, 0x20)) && mimic::is_disguised(u, id))
        .collect();
    not_disabled(u, &mut sel);
    if !sel.is_empty() {
        mimic::cl_disguise(u, &sel, mimic::DisguiseOrder { kind: 0, ship_type: 0, player: 0, asteroid: 0 });
    }
}

/// What a Mimic order clicked on takes the shape of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MimicTarget {
    Ship(ShipId),
    Asteroid,
}

/// Whether the selection's Mimics can take the shape of the object under the pointer
/// (`mouseCursorActionClassify`'s Mimic case): an asteroid, or a ship whose type its owner's
/// disguise menu offers them.
pub fn mimic_can_take_shape(u: &Universe, t: &mimic::DisguiseTables, selection: &[ShipId], target: MimicTarget) -> bool {
    match target {
        MimicTarget::Asteroid => true,
        MimicTarget::Ship(id) => {
            let Some(s) = u.ships.get(id) else { return false };
            let n = mimic_entry_count(t, s.player, selection_has_mcv(u, selection));
            t.entries.get(s.player).is_some_and(|l| l.iter().take(n).any(|e| e.ship_type == s.ship_type.index() as i32))
        }
    }
}

/// `mrMimicDisguiseAsTarget` (0x562200): the selection takes the shape of the clicked ship (its
/// type, as one of its owner's), or each ship that of an asteroid (an MCV a large one, a Mimic a
/// small one).
pub fn mr_mimic_disguise_as_target(u: &mut Universe, selection: &[ShipId], target: MimicTarget) {
    let sel: Vec<ShipId> = selection.iter().copied().filter(|&id| u.ships.get(id).is_some()).collect();
    if sel.is_empty() {
        return;
    }
    match target {
        MimicTarget::Ship(t) => {
            let Some(s) = u.ships.get(t) else { return };
            let o = mimic::DisguiseOrder { kind: 1, ship_type: s.ship_type.index() as i32, player: s.player as i32, asteroid: 0 };
            mimic::cl_disguise(u, &sel, o);
        }
        MimicTarget::Asteroid => {
            for id in sel {
                let k = if u.ships.ship(id).ship_type == ShipType::sMCV { 3 } else { 1 };
                mimic::cl_disguise(u, &[id], mimic::DisguiseOrder { kind: 2, ship_type: 0, player: 8, asteroid: k });
            }
        }
    }
}

/// `MakeShipsSpecialTargetCapable(selection, 0, TRUE)` (0x50d4d0): ships with a special-target
/// routine that are not disabled.
pub fn special_target_capable(u: &Universe, list: &mut Vec<ShipId>) {
    retain_swap(list, |id| u.ships.get(id).is_some_and(|s| crate::ships::has_special_target(s.ship_type) && !s.base.disabled));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_number_like_the_original_menu() {
        assert_eq!(SpecialItem::Harvest.index(), 25);
        assert_eq!(SpecialItem::Link.index(), 31);
        assert_eq!(SpecialItem::Unlink.index(), 32);
        assert_eq!(SpecialItem::Emp.index(), 45);
        assert_eq!(SpecialItem::FireMissiles.index(), 48);
    }

    #[test]
    fn context_target_kinds_follow_the_menu_items() {
        use SpecialItem as I;
        // `pmSetContextTarget`: item 0x1a repair (allies), 0x1b salvage, 0x2d EMP, the
        // other targeted ones enemies.
        let kinds: Vec<(usize, TargetKind)> = SpecialItem::ALL.into_iter().filter_map(|i| Some((i.index(), i.target_kind()?))).collect();
        assert_eq!(
            kinds,
            vec![
                (0x1a, TargetKind::Ally),
                (0x1b, TargetKind::Salvageable),
                (0x1d, TargetKind::Enemy),
                (0x28, TargetKind::Enemy),
                (0x2c, TargetKind::Enemy),
                (0x2d, TargetKind::Empable),
                (0x2e, TargetKind::Enemy),
                (0x2f, TargetKind::Enemy),
                (0x30, TargetKind::Enemy),
            ]
        );
        assert_eq!(I::Repair.index(), 0x1a);
    }

    #[test]
    fn swap_removal_keeps_the_original_order() {
        let mut v: Vec<ShipId> = (0..5).map(ShipId).collect();
        retain_swap(&mut v, |id| id.0 % 2 == 1);
        assert_eq!(v, vec![ShipId(3), ShipId(1)]);
    }
}
