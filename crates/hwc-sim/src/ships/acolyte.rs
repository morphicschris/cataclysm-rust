//! Acolyte link missiles (`Acolyte` module, 0x5a7db0..0x5a86a0): sAcolyte, bAcolyte, dAcolyte.
//!
//! An Acolyte fights as an interceptor (`GenericInterceptorAttack`), with three additions:
//!
//! * the gun pods unfold (a `.mad` animation) a short random time after the ship starts
//!   attacking and fold back a little later after it stops; the Beast Acolyte only fires its
//!   guns with the pods out;
//! * Somtaaw and Beast Acolytes carry two link missiles (launchers 2 and 3, one missile each,
//!   re-armed when the ship docks). With the link-missile upgrade, a special targeting order
//!   makes the selected Acolytes attack one target together and launch both missiles once they
//!   are within 4 km of it with their pods out;
//! * the link-attack state is dropped when the ship attacks something else, links, or its
//!   target leaves.
//!
//! The pod animation is not timed here (`.mad` animations finish at once in this simulation),
//! so only the random delays before opening and closing matter.

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_ETG;

use crate::object::ShipId;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Upgrade bit of the link missiles (`suShipHasUpgrade(ship, 0x4000000)`).
pub const UPGRADE_LINK_MISSILES: u32 = 0x0400_0000;
/// Squared range within which the link missiles launch (0x8b9010, 4000 m).
const LINK_MISSILE_RANGE_SQ: f32 = 16_000_000.0;
/// The launchers that carry the link missiles.
const LINK_LAUNCHERS: [usize; 2] = [2, 3];

/// Per-ship state (+0x67c..+0x694; +0x638..+0x650 is the interceptor state kept in
/// [`crate::object::Ship::interceptor`], +0x650 the link partner in the base state).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AcolyteSpec {
    /// +0x67c: the pods were opened by the attack logic.
    pub pods_opened: bool,
    /// +0x67d: an open or close is scheduled for [`Self::pod_time`].
    pub pod_change_pending: bool,
    /// +0x680: when the scheduled open or close happens.
    pub pod_time: f32,
    /// +0x684: the pods are folded (1 at creation); guns and link missiles need them out.
    pub pods_folded: bool,
    /// +0x688: the target of the link-missile attack.
    pub link_target: Option<ShipId>,
    /// +0x68c: a link-missile attack is on.
    pub link_attack: bool,
    /// +0x690: the group attack order is still to be given (next housekeeping).
    pub group_attack_pending: bool,
}

impl Default for AcolyteSpec {
    fn default() -> Self {
        Self {
            pods_opened: false,
            pod_change_pending: false,
            pod_time: 0.0,
            pods_folded: true,
            link_target: None,
            link_attack: false,
            group_attack_pending: false,
        }
    }
}

/// Per-type statics (`AcolyteStaticInit`: the interceptor statics, kept in
/// [`crate::combat_statics::TypeCombat::interceptor`]).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AcolyteStatics {}

impl AcolyteStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

/// `AcolyteIsAcolyte` (0x5a7e30): the Somtaaw and Beast Acolytes (the ones with link missiles).
pub fn is_link_acolyte(t: ShipType) -> bool {
    matches!(t, ShipType::sAcolyte | ShipType::bAcolyte)
}

/// `AcolyteIsAnyAcolyte` (0x5a7e90).
pub fn is_any_acolyte(t: ShipType) -> bool {
    matches!(t, ShipType::sAcolyte | ShipType::bAcolyte | ShipType::dAcolyte)
}

fn spec(u: &Universe, id: ShipId) -> Option<&AcolyteSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Acolyte(a) => Some(a),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut AcolyteSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Acolyte(a) => Some(a),
        _ => None,
    }
}

/// Clears the link-missile attack (the reset several entry points share).
fn clear_link_attack(u: &mut Universe, id: ShipId) {
    if let Some(a) = spec_mut(u, id) {
        a.group_attack_pending = false;
        a.link_attack = false;
        a.link_target = None;
    }
}

/// `AcolyteInit` (0x5a7ec0).
pub fn init(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.interceptor.active_gun = 0;
        s.spec = ShipSpec::Acolyte(AcolyteSpec::default());
    }
}

/// Whether launcher `gi` still has its link missile.
fn launcher_loaded(u: &Universe, id: ShipId, gi: usize) -> bool {
    u.ships.get(id).and_then(|s| s.guns.get(gi)).is_some_and(|g| g.num_missiles > 0)
}

/// `AcolyteHasLinkMissiles` (0x5a8580): a Somtaaw or Beast Acolyte with the upgrade and a
/// link missile left.
pub fn has_link_missiles(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    is_link_acolyte(s.ship_type)
        && crate::upgrade::has_upgrade(s, UPGRADE_LINK_MISSILES)
        && (launcher_loaded(u, id, LINK_LAUNCHERS[0]) || launcher_loaded(u, id, LINK_LAUNCHERS[1]))
}

/// `AcolyteLinkMissileState` (0x5a85e0): 2 when [`has_link_missiles`], else 0 (what an ACV
/// remembers of each half).
pub fn link_missile_state(u: &Universe, id: ShipId) -> i32 {
    if has_link_missiles(u, id) {
        2
    } else {
        0
    }
}

/// `AcolyteIsLinked` (0x5a7e50): a Somtaaw or Beast Acolyte with the link-missile upgrade.
pub fn is_linked(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| is_link_acolyte(s.ship_type) && crate::upgrade::has_upgrade(s, UPGRADE_LINK_MISSILES))
}

fn set_link_missiles(u: &mut Universe, id: ShipId, n: i32) {
    let Some(s) = u.ships.get_mut(id) else { return };
    for gi in LINK_LAUNCHERS {
        if let Some(g) = s.guns.get_mut(gi) {
            g.num_missiles = n;
        }
    }
}

/// `AcolyteArmLinkMissiles` (0x5a7db0): after docking (and when an ACV that still had them
/// unlinks) both link missiles are back and the link attack is reset.
pub fn arm_link_missiles(u: &mut Universe, id: ShipId) {
    if !u.ships.get(id).is_some_and(|s| is_link_acolyte(s.ship_type)) {
        return;
    }
    clear_link_attack(u, id);
    set_link_missiles(u, id, 1);
}

/// `AcolyteDisarmLinkMissiles` (0x5a7e00).
pub fn disarm_link_missiles(u: &mut Universe, id: ShipId) {
    if u.ships.get(id).is_some_and(|s| is_link_acolyte(s.ship_type)) {
        set_link_missiles(u, id, 0);
    }
}

/// `processUnlinkToDo`: a half of an unlinked ACV gets its link missiles back when the ACV
/// remembered them (`state` is the ACV's +0x6fc or +0x700).
pub fn restore_after_unlink(u: &mut Universe, id: ShipId, state: i32) {
    if state == 0 {
        disarm_link_missiles(u, id);
    } else {
        arm_link_missiles(u, id);
    }
}

/// Whether the link missiles may launch now: armed for a link attack, pods out, upgrade and
/// missiles present (the condition `AcolyteFire` and `AcolyteHouseKeep` share).
fn link_launch_ready(u: &Universe, id: ShipId) -> bool {
    let Some(a) = spec(u, id) else { return false };
    let Some(s) = u.ships.get(id) else { return false };
    // (The pod animation is always finished here.)
    is_link_acolyte(s.ship_type) && !a.pods_folded && a.link_attack && has_link_missiles(u, id)
}

/// Both launchers fire their link missile at `target` when ready.
fn launch_link_missiles(u: &mut Universe, id: ShipId, target: ShipId) {
    for gi in LINK_LAUNCHERS {
        if gi < u.ships.ship(id).guns.len() && crate::gun::ready(u, id, gi) && launcher_loaded(u, id, gi) {
            crate::missile::shoot(u, id, gi, Some(target), None);
        }
    }
}

/// `AcolyteFire` (0x5a7f30): the interceptor fire (the Beast Acolyte only with its pods out),
/// then the link missiles at the target being fired at when a link attack is on.
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let folded = spec(u, id).is_none_or(|a| a.pods_folded);
    if t != ShipType::dAcolyte || !folded {
        crate::attack::interceptor_fire(u, id, target);
    }
    if link_launch_ready(u, id) {
        launch_link_missiles(u, id, target);
    }
    u.ships.ship_mut(id).ship_is_attacking = true;
}

/// `AcolyteAttack` (0x5a8660), before the interceptor attack: attacking anything but the
/// link target ends the link attack.
pub fn attack_target_changed(u: &mut Universe, id: ShipId, target: ShipId) {
    if spec(u, id).is_some_and(|a| a.link_target != Some(target)) {
        clear_link_attack(u, id);
    }
}

/// `AcolyteSpecialActivate` (0x5a8640; also slot 20): drop the link attack. Always done.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    clear_link_attack(u, id);
    true
}

/// `AcolyteSpecialTarget` (0x5a8040): a link attack on the first target; the attack order
/// follows at the next housekeeping. Always done.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if let (Some(&t), Some(a)) = (targets.first(), spec_mut(u, id)) {
        a.link_attack = true;
        a.group_attack_pending = true;
        a.link_target = Some(t);
    }
    true
}

/// `AcolyteLinkedGroupAttack` (0x5a8070): an Acolyte in a formation of more than one gives the
/// attack order for every Acolyte of it waiting to link-attack the same target.
fn linked_group_attack(u: &mut Universe, id: ShipId) {
    let Some(target) = spec(u, id).and_then(|a| a.link_target) else { return };
    let Some(cmd) = u.ships.ship(id).command.and_then(|c| u.commands.get(c)) else { return };
    if cmd.attributes & crate::command::attr::FORMATION == 0 || cmd.selection.len() < 2 {
        return;
    }
    let selection = cmd.selection.clone();
    let mut group = Vec::new();
    for s in selection {
        let Some(t) = u.ships.get(s).map(|x| x.ship_type) else { continue };
        if !is_link_acolyte(t) {
            continue;
        }
        if let Some(a) = spec_mut(u, s) {
            if a.group_attack_pending && a.link_attack && a.link_target == Some(target) {
                group.push(s);
                a.group_attack_pending = false;
            }
        }
    }
    crate::command::cl_attack(u, &group, &[target]);
}

/// The random delay before the pods open (`base` 0) or close (`base` 2): `base + 3 (1 - e^-x)`
/// with `x` uniform in [0, 2) from the special-ship stream.
fn pod_delay(u: &mut Universe, base: f64) -> f64 {
    let r = u.randy.random(RAN_ETG);
    let x = f64::from(r) * ld(f32::from_bits(0x33c8_0000)) * ld(f32::from_bits(0xbba3_d70a));
    let e = (x * std::f64::consts::LOG2_E).exp2();
    (1.0 - e) * 3.0 + base
}

/// `AcolyteHouseKeep` (0x5a8150): the group attack order of a link attack, the link-missile
/// launch within range, and the pod opening and closing.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    if spec(u, id).is_none() {
        return;
    }
    if spec(u, id).is_some_and(|a| a.group_attack_pending) {
        linked_group_attack(u, id);
        if let Some(target) = spec(u, id).filter(|a| a.group_attack_pending).and_then(|a| a.link_target) {
            if let Some(a) = spec_mut(u, id) {
                a.group_attack_pending = false;
            }
            crate::command::cl_attack(u, &[id], &[target]);
        }
    }
    let target = spec(u, id).filter(|a| a.link_attack).and_then(|a| a.link_target);
    if let Some(t) = target {
        if has_link_missiles(u, id) {
            if let Some(ts) = u.ships.get(t) {
                let p = u.ships.ship(id).posinfo.position;
                let q = ts.posinfo.position;
                let (dx, dy, dz) = (ld(p.x) - ld(q.x), ld(p.y) - ld(q.y), ld(p.z) - ld(q.z));
                if (dx * dx + dy * dy) + dz * dz < ld(LINK_MISSILE_RANGE_SQ) && link_launch_ready(u, id) {
                    launch_link_missiles(u, id, t);
                }
            }
        }
    }
    pods_update(u, id);
}

/// The pod part of `AcolyteHouseKeep`: while attacking, open after a random delay; once no
/// longer attacking, close after a longer one.
fn pods_update(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let attacking = u.ships.ship(id).ship_is_attacking;
    let a = spec(u, id).cloned().expect("acolyte");
    // Open while attacking with the pods folded; close when idle with pods the attack opened.
    let change = if attacking { a.pods_folded } else { a.pods_opened && !a.pods_folded };
    if !change {
        if let Some(a) = spec_mut(u, id) {
            a.pod_change_pending = false;
        }
        return;
    }
    if a.pod_change_pending {
        if !(ld(a.pod_time) < ld(now)) {
            return;
        }
        let a = spec_mut(u, id).expect("acolyte");
        a.pods_folded = !attacking;
        a.pods_opened = attacking;
        a.pod_change_pending = false;
        return;
    }
    let d = pod_delay(u, if attacking { 0.0 } else { 2.0 });
    let a = spec_mut(u, id).expect("acolyte");
    a.pod_change_pending = true;
    a.pod_time = st(ld(now) + d);
}

/// `AcolyteRemoveShipReferences` (0x5a86a0).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if spec(u, id).is_some_and(|a| a.link_target == Some(gone)) {
        clear_link_attack(u, id);
    }
}

/// `AcolyteCancelLinkAttackCheckOrder` (0x5a8540, slot 27): drop the link attack. (The
/// original also answers whether the order may be cancelled: not while linking, nor with a
/// link partner.)
pub fn cancel_link_attack(u: &mut Universe, id: ShipId) {
    clear_link_attack(u, id);
}

/// The answer of `AcolyteCancelLinkAttackCheckOrder`: false while a link order runs or the
/// ship has a link partner.
pub fn may_cancel(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return true };
    if s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::LINK) {
        return false;
    }
    s.base.link.partner.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pod_delay_range() {
        // x = -r / 2^31 (r in [0, 2^32)): the delay runs from 0 to 3 (1 - e^-2).
        let x_max = f64::from(u32::MAX) * ld(f32::from_bits(0x33c8_0000)) * ld(f32::from_bits(0xbba3_d70a));
        assert!((x_max + 2.0).abs() < 1e-6);
        let d = (1.0 - (x_max * std::f64::consts::LOG2_E).exp2()) * 3.0;
        assert!((d - 3.0 * (1.0 - (-2.0f64).exp())).abs() < 1e-6);
    }

    #[test]
    fn link_types() {
        assert!(is_link_acolyte(ShipType::sAcolyte) && is_link_acolyte(ShipType::bAcolyte));
        assert!(!is_link_acolyte(ShipType::dAcolyte) && is_any_acolyte(ShipType::dAcolyte));
    }
}
