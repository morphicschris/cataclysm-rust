//! The ACV and its EMP (`ACV` module, 0x5a86d0..0x5a99f0): sACV, bACV.
//!
//! The ACV (two linked Acolytes) fights like a corvette (`attackSideStep`) and, with the EMP
//! upgrade, sends out an electromagnetic pulse that disables nearby ships (`ShipApplyEMP`):
//!
//! * activated without a target it fires at once around itself;
//! * given a target it flies there (state "moving to target"), with every ACV of its
//!   formation that was given the same target, and fires when within 2.5 km (4 km for an RKO);
//!   ACVs in a formation that are close enough to the group's centre combine their pulses
//!   (each victim takes one charge per ACV);
//! * a pulse leaves the ACV unable to fire again for 77 seconds (2 s firing, 75 recharging).
//!
//! The ACV also remembers whether each of its two Acolytes still had link missiles, so they
//! get them back when it unlinks.

use hwc_math::fpu::{ld, st, Ext};
use hwc_math::Vector;

use crate::aiship::flags as fly;
use crate::object::{sof, ShipId};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::{SpecialEvent, Universe};

use super::{Effect, ShipSpec};

/// Upgrade bit of the EMP (`suShipHasUpgrade(ship, 0x200)`).
pub const UPGRADE_EMP: u32 = 0x200;
/// Squared distance to its target at which a moving ACV fires (2500 m).
const FIRE_DIST_SQ: f32 = 6_250_000.0;
/// Distance at which an RKO counts as in range (0x8b90d8, both for firing and as a victim).
const RKO_RANGE: f32 = 4000.0;
/// Seconds the pulse is firing, and firing plus recharging.
const FIRING_TIME: f32 = 2.0;
const RECHARGE_END: f32 = 77.0;
const RECHARGE_TIME: f64 = 75.0;

/// EMP states (+0x6f0).
pub mod emp {
    pub const READY: u8 = 0;
    pub const FIRING: u8 = 1;
    pub const RECHARGING: u8 = 2;
    pub const MOVING_TO_TARGET: u8 = 3;
}

/// Per-ship state (+0x638..+0x70c; the side-step state is the generic
/// [`crate::object::Ship::side_step`]).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AcvSpec {
    /// +0x6e4: the ships the last pulse hit and the ACVs that joined it (drawn as arcs).
    pub emp_targets: Vec<ShipId>,
    /// +0x6ec: when the last pulse went off.
    pub emp_fire_time: f32,
    /// +0x6f0: see [`emp`].
    pub emp_state: u8,
    /// +0x6f4: the ship it flies to before firing.
    pub emp_target: Option<ShipId>,
    /// +0x6f8: the attack order towards the target is still to be given.
    pub group_attack_pending: bool,
    /// +0x6fc / +0x700: whether each Acolyte had link missiles (2) or not (0).
    pub link_missiles: [i32; 2],
    /// +0x704 / +0x708: whether each Acolyte had the link-missile upgrade.
    pub acolytes_linked: [bool; 2],
}

/// Per-type statics (`ACVStaticInit`: +4 `EMP_Max_Combine_Distance`, +8 `EMP_Range`; the
/// side-step parameters are [`crate::combat_statics::TypeCombat::side_step`]).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct AcvStatics {
    pub max_combine_distance: f32,
    pub emp_range: f32,
}

impl AcvStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        Self { max_combine_distance: stat.emp_max_combine_distance, emp_range: stat.emp_range }
    }
}

/// `ACVIsACV` (0x5a8ce0).
pub fn is_acv(t: ShipType) -> bool {
    matches!(t, ShipType::sACV | ShipType::bACV)
}

fn spec(u: &Universe, id: ShipId) -> Option<&AcvSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Acv(a) => Some(a),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut AcvSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Acv(a) => Some(a),
        _ => None,
    }
}

fn statics(u: &Universe, id: ShipId) -> AcvStatics {
    let t = u.ships.ship(id).ship_type;
    match super::type_statics(u, t) {
        super::TypeStatics::Acv(s) => s.clone(),
        _ => AcvStatics::default(),
    }
}

/// `ACVInit` (0x5a8f00).
pub fn init(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.side_step = Default::default();
        s.spec = ShipSpec::Acv(AcvSpec::default());
    }
}

/// `ACVClose` (0x5a8770; also the died slot): the target list goes.
pub fn close(u: &mut Universe, id: ShipId) {
    if let Some(a) = spec_mut(u, id) {
        a.emp_targets.clear();
    }
}

/// `ACVResetEMPCounters` (0x5a8740): after docking both link-missile counters are full.
pub fn reset_emp_counters(u: &mut Universe, id: ShipId) {
    if let Some(a) = spec_mut(u, id) {
        a.link_missiles = [2, 2];
    }
}

/// `processLinkToDo`: a new ACV remembers its Acolytes' link missiles (`a` the initiator).
pub fn remember_acolytes(u: &mut Universe, id: ShipId, a: ShipId, b: ShipId) {
    if !u.ships.get(a).is_some_and(|s| super::acolyte::is_link_acolyte(s.ship_type)) {
        return;
    }
    let missiles = [super::acolyte::link_missile_state(u, a), super::acolyte::link_missile_state(u, b)];
    let linked = [super::acolyte::is_linked(u, a), super::acolyte::is_linked(u, b)];
    if let Some(s) = spec_mut(u, id) {
        s.link_missiles = missiles;
        s.acolytes_linked = linked;
    }
}

/// The link-missile counter of the ACV's first (`half` 0) or second Acolyte, for unlinking.
pub fn link_missile_counter(u: &Universe, id: ShipId, half: usize) -> i32 {
    spec(u, id).map_or(0, |a| a.link_missiles[half])
}

/// `ACVIsMovingToEMPTarget` (0x5a8790).
pub fn is_moving_to_emp_target(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|a| a.emp_state == emp::MOVING_TO_TARGET)
}

/// `ACVIsFiringEMP` (0x5a8840).
pub fn is_firing_emp(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|a| a.emp_state == emp::FIRING)
}

/// `ACVGetEMPTargets` (0x5a9880): the arcs to draw.
pub fn emp_targets(u: &Universe, id: ShipId) -> &[ShipId] {
    spec(u, id).map_or(&[][..], |a| a.emp_targets.as_slice())
}

/// The ship holding `id` in its salvage depot (ship +0x3ec).
fn depot_holding(u: &Universe, id: ShipId) -> Option<ShipId> {
    u.ships.iter().find(|d| d.base.depot.as_ref().is_some_and(|slots| slots.iter().any(|s| s.ship == Some(id)))).map(|d| d.id)
}

/// `ACVShipOwnedByPlayer` (0x5a86d0): `id` is being salvaged by one of `player`'s salvagers
/// (the first salvager clamped to it), or sits in one of `player`'s depots.
fn owned_by_player(u: &Universe, player: usize, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let salvager = crate::clamp::slaves_of(u, id).into_iter().find(|&x| u.ships.get(x).is_some_and(|x| crate::salvage::is_salvager(x.ship_type)));
    let salvaged = salvager.is_some_and(|x| s.special_flags2 & crate::base::special2::BEING_SALVAGED != 0 && u.ships.ship(x).player == player);
    salvaged || depot_holding(u, id).is_some_and(|d| u.ships.ship(d).player == player)
}

/// `ACVCanBeEMPed` (0x5a8860): a ship an EMP disables (or a KAS-flagged one that flickers)
/// and that is not the ACV's own, nor held or being salvaged by the ACV's player. (Ships being
/// consumed by the Beast, +0x570, are also immune; consuming is not modelled.)
pub fn can_be_emped(u: &Universe, id: ShipId, acv: Option<ShipId>) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let Some(stat) = u.statics.ship(s.ship_type) else { return false };
    let acv_player = acv.and_then(|a| u.ships.get(a)).map(|a| a.player);
    let mut ok = (0.0 < stat.time_shocked_by_emp || s.special_flags2 & crate::base::special2::KAS_EMP_FLICKER != 0) && acv_player != Some(s.player);
    if depot_holding(u, id).is_some() {
        ok = false;
    }
    if let Some(p) = acv_player {
        if owned_by_player(u, p, id) {
            ok = false;
        }
    }
    ok
}

/// `RKOIsRKO`.
fn is_rko(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.ship_type == ShipType::dRKO)
}

/// Whether the ACV may fire its EMP now (the check before `ACVGetEMPGroupCentreShip`).
fn emp_ready(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    let Some(a) = spec(u, id) else { return false };
    matches!(a.emp_state, emp::READY | emp::MOVING_TO_TARGET) && crate::upgrade::has_upgrade(s, UPGRADE_EMP) && s.flags & sof::CRAZY == 0
}

/// `ACVGetEMPGroupCentreShip` (0x5a8d00): the ACVs firing together (the ship alone when
/// `single` or not in a formation, else every ready ACV of its formation) and the one nearest
/// their centre.
fn group_centre_ship(u: &Universe, id: ShipId, single: bool) -> (Vec<ShipId>, Option<ShipId>) {
    let mut group = Vec::new();
    let cmd = u.ships.ship(id).command.and_then(|c| u.commands.get(c));
    match cmd {
        Some(c) if !single && c.attributes & crate::command::attr::FORMATION != 0 => {
            for &s in &c.selection {
                if u.ships.get(s).is_some_and(|x| is_acv(x.ship_type)) && emp_ready(u, s) {
                    group.push(s);
                }
            }
        }
        _ => group.push(id),
    }
    let (mut x, mut y, mut z) = (0.0f32, 0.0f32, 0.0f32);
    for &s in &group {
        let p = u.ships.ship(s).posinfo.position;
        x = st(ld(x) + ld(p.x));
        y = st(ld(y) + ld(p.y));
        z = st(ld(z) + ld(p.z));
    }
    let k = st(1.0 / ld(group.len() as f32));
    let centre = Vector::new(st(ld(k) * ld(x)), st(ld(k) * ld(y)), st(ld(k) * ld(z)));
    let mut best: Option<ShipId> = None;
    let mut best_dist = 1e21f32;
    for &s in &group {
        let d = Vector::distance_ext(centre, u.ships.ship(s).posinfo.position);
        if d < ld(best_dist) {
            best_dist = st(d);
            best = Some(s);
        }
    }
    (group, best)
}

/// The firing part of `ACVSpecialActivate` and `ACVHouseKeep`: the group ACVs close enough to
/// the centre fire together around the centre ship, else the ACV fires alone.
fn fire_group(u: &mut Universe, id: ShipId, single: bool) {
    let (mut group, centre) = group_centre_ship(u, id, single);
    let Some(centre) = centre else { return };
    let cp = u.ships.ship(centre).posinfo.position;
    let mut i = 0;
    while i < group.len() {
        let s = group[i];
        let max = statics(u, s).max_combine_distance;
        if ld(max) < Vector::distance_ext(cp, u.ships.ship(s).posinfo.position) {
            group.swap_remove(i);
        }
        i += 1;
    }
    if group.len() < 2 || !group.contains(&id) {
        fire_emp(u, &[id], id);
    } else {
        fire_emp(u, &group, centre);
    }
}

/// `ACVFireEMP` (0x5a8940): every ship within an ACV's range that an EMP can hit takes one
/// charge per ACV of the group; the ACVs start recharging and the centre ACV keeps the list.
pub fn fire_emp(u: &mut Universe, group: &[ShipId], centre: ShipId) {
    let mut victims: Vec<ShipId> = Vec::new();
    let ships = u.ship_list.clone();
    for &acv in group {
        let range = statics(u, acv).emp_range;
        let ap = u.ships.ship(acv).posinfo.position;
        for &s in &ships {
            if s == acv || !can_be_emped(u, s, Some(acv)) {
                continue;
            }
            let d: Ext = Vector::distance_ext(ap, u.ships.ship(s).posinfo.position);
            if (d < ld(range) || (is_rko(u, s) && st(d) < RKO_RANGE)) && !victims.contains(&s) {
                victims.push(s);
            }
        }
    }
    if victims.is_empty() {
        // "No targets in range".
        crate::speech::by_ship(u, centre, crate::speech::event::NOT_ENOUGH);
        return;
    }
    crate::speech::queue(u, Some(centre), crate::speech::event::EMP_FIRED, 0);
    let now = u.total_time_elapsed;
    for &acv in group {
        let radius = statics(u, acv).emp_range;
        if let Some(a) = spec_mut(u, acv) {
            a.emp_targets.clear();
            a.emp_fire_time = now;
            a.emp_state = emp::FIRING;
            a.emp_target = None;
        }
        u.special_events.push(SpecialEvent::Special { ship: acv, effect: Effect::EmpBlast { radius } });
        emp_effect(u, acv);
    }
    let joined: Vec<ShipId> = group.iter().copied().filter(|&s| s != centre).collect();
    if let Some(a) = spec_mut(u, centre) {
        a.emp_targets.extend(joined);
    }
    let charges = group.len() as i32;
    for v in victims {
        crate::disable::apply_emp(u, v, charges);
        if let Some(a) = spec_mut(u, centre) {
            a.emp_targets.push(v);
        }
    }
}

/// The pulse effect of one firing ACV in `ACVFireEMP` (`EGT_ELECTRO_MAGNETIC_PULSE`, always its
/// first level): at the ACV, oriented like it, standing still but attached to it in every
/// respect, with no parameters.
fn emp_effect(u: &mut Universe, acv: ShipId) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(effect) = fx.tables.special_effect(EMP_EFFECT_SLOT).and_then(|l| l.first().copied()) else { return };
    if !crate::effects::frequency_ok(u, effect) {
        return;
    }
    let s = u.ships.ship(acv);
    let (pos, cs) = (s.posinfo.position, s.rotinfo.coordsys);
    let flags = crate::effects::EAF_ALL_BUT_NLIPS | crate::effects::attach::NLIPS;
    crate::effects::create(u, effect, Some(acv.into()), Some(pos), Some(Vector::ZERO), Some(cs), 1.0, flags, &[]);
}

/// `EGT_ELECTRO_MAGNETIC_PULSE` in the special-purpose effect table.
const EMP_EFFECT_SLOT: usize = 23;

/// `ACVSpecialActivate` (0x5a8f70): a ready ACV fires its EMP alone; one that is busy drops
/// its EMP target. Always done.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let Some(state) = spec(u, id).map(|a| a.emp_state) else { return true };
    if state == emp::READY {
        if emp_ready(u, id) {
            fire_group(u, id, true);
        }
    } else if let Some(a) = spec_mut(u, id) {
        a.emp_state = emp::READY;
        a.emp_target = None;
    }
    true
}

/// `ACVSpecialTarget` (0x5a90c0): fly to the first target an EMP can hit and fire there.
/// Always done (the flight is driven by the housekeeping and the attack routine).
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if !spec(u, id).is_some_and(|a| matches!(a.emp_state, emp::READY | emp::MOVING_TO_TARGET)) {
        return true;
    }
    for &t in targets {
        if can_be_emped(u, t, Some(id)) {
            let a = spec_mut(u, id).expect("acv");
            a.emp_target = Some(t);
            a.emp_state = emp::MOVING_TO_TARGET;
            a.group_attack_pending = true;
            return true;
        }
    }
    true
}

/// `ACVGroupAttackEMPTarget` (0x5a9240): the attack order towards the EMP target, for every
/// ACV of the formation waiting with the same target.
fn group_attack_emp_target(u: &mut Universe, id: ShipId) {
    let Some(target) = spec(u, id).and_then(|a| a.emp_target) else { return };
    let cmd = u.ships.ship(id).command.and_then(|c| u.commands.get(c));
    let formation = cmd.filter(|c| c.attributes & crate::command::attr::FORMATION != 0 && c.selection.len() > 1).map(|c| c.selection.clone());
    match formation {
        Some(sel) => {
            let mut group = Vec::new();
            for s in sel {
                if !u.ships.get(s).is_some_and(|x| is_acv(x.ship_type)) {
                    continue;
                }
                if let Some(a) = spec_mut(u, s) {
                    if a.group_attack_pending && a.emp_state == emp::MOVING_TO_TARGET && a.emp_target == Some(target) {
                        group.push(s);
                        a.group_attack_pending = false;
                    }
                }
            }
            crate::command::cl_attack(u, &group, &[target]);
        }
        None => {
            crate::command::cl_attack(u, &[id], &[target]);
            if let Some(a) = spec_mut(u, id) {
                a.group_attack_pending = false;
            }
        }
    }
}

/// `ACVGetEMPChargeFraction` (0x5a9360): 0 while firing, the recharge progress after, 1 when
/// ready (0 without the upgrade).
pub fn emp_charge_fraction(u: &Universe, id: ShipId) -> f32 {
    let Some(a) = spec(u, id) else { return 0.0 };
    let f: Ext = match a.emp_state {
        emp::FIRING => 0.0,
        emp::RECHARGING => (ld(u.total_time_elapsed) - ld(a.emp_fire_time) - 2.0) / RECHARGE_TIME,
        _ => {
            let up = u.ships.get(id).is_some_and(|s| crate::upgrade::has_upgrade(s, UPGRADE_EMP));
            return if up { 1.0 } else { 0.0 };
        }
    };
    if f <= 1.0 {
        st(f)
    } else {
        1.0
    }
}

/// `ACVHouseKeep` (0x5a93e0): the firing and recharge timers, and an ACV moving to its target
/// fires when close enough (or gives up when the target is no longer visible).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(a) = spec(u, id).cloned() else { return };
    let elapsed = st(ld(u.total_time_elapsed) - ld(a.emp_fire_time));
    let mut state = a.emp_state;
    if state == emp::FIRING && (FIRING_TIME < elapsed || elapsed < 0.0) {
        let s = spec_mut(u, id).expect("acv");
        s.emp_targets.clear();
        s.emp_state = emp::RECHARGING;
        state = emp::RECHARGING;
    }
    if state == emp::RECHARGING && (RECHARGE_END < elapsed || elapsed < 0.0) {
        let s = spec_mut(u, id).expect("acv");
        s.emp_state = emp::READY;
        s.emp_fire_time = 0.0;
        state = emp::READY;
    }
    if state != emp::MOVING_TO_TARGET {
        return;
    }
    let target = a.emp_target;
    if spec(u, id).is_some_and(|a| a.group_attack_pending) {
        group_attack_emp_target(u, id);
        if let Some(a) = spec_mut(u, id) {
            a.group_attack_pending = false;
        }
    }
    let player = u.ships.ship(id).player;
    let visible = target.is_some_and(|t| crate::fow::ship_visible_to_player(u, t, player));
    let Some(t) = target.filter(|_| visible) else {
        super::cl_special(u, &[id], &[]);
        return;
    };
    let p = u.ships.ship(id).posinfo.position;
    let q = u.ships.ship(t).posinfo.position;
    let (dx, dy, dz) = (st(ld(p.x) - ld(q.x)), st(ld(p.y) - ld(q.y)), st(ld(p.z) - ld(q.z)));
    let d2 = st((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz));
    let rko_close = is_rko(u, t) && ld(d2) < ld(RKO_RANGE) * ld(RKO_RANGE);
    if rko_close || d2 < FIRE_DIST_SQ {
        spec_mut(u, id).expect("acv").emp_state = emp::READY;
        if emp_ready(u, id) {
            fire_group(u, id, false);
        }
        if let Some(a) = spec_mut(u, id) {
            a.emp_target = None;
        }
    }
}

/// `ACVFire` (0x5a96b0): every ready gun fires (bullet and beam weapons, missiles while there
/// are any).
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let n = u.ships.ship(id).guns.len();
    for gi in 0..n {
        if !crate::gun::ready(u, id, gi) {
            continue;
        }
        let g = u.ships.ship(id).guns[gi];
        match g.level {
            0 | 1 | 2 | 7 => {
                crate::gun::shoot(u, id, gi, Some(target));
            }
            5 | 6 if g.num_missiles > 0 => {
                crate::missile::shoot(u, id, gi, Some(target), None);
            }
            _ => {}
        }
    }
}

/// `ACVAttack` (0x5a9740): side-step attack, except towards its EMP target, which it flies to
/// (a formation follows its leader there).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(a) = spec(u, id).cloned() else { return };
    if a.emp_state != emp::MOVING_TO_TARGET || a.emp_target != Some(target) {
        if a.emp_state == emp::MOVING_TO_TARGET {
            let s = spec_mut(u, id).expect("acv");
            s.emp_state = emp::READY;
            s.emp_target = None;
        } else if let Some(c) = u.combat.get(u.ships.ship(id).ship_type).cloned() {
            crate::attack::attack_side_step(u, id, target, &c);
        }
        return;
    }
    let Some(tp) = crate::target::get(u, target).map(|t| t.posinfo.position) else { return };
    let cid = u.ships.ship(id).command;
    let formation = cid.and_then(|c| u.commands.get(c)).filter(|c| c.attributes & crate::command::attr::FORMATION != 0).cloned();
    match formation {
        Some(cmd) => {
            let leader = cmd.selection.first().copied();
            if leader == Some(id) {
                fly_to_target(u, id, tp, cmd.formation.travel_vel);
                u.ships.ship_mut(id).ship_idle = false;
                crate::formation::process(u, cid.expect("command"), false, false);
            } else if let Some(l) = leader {
                // A follower keeps its slot while the leader flies to the same target.
                let same = u.ships.get(l).is_some_and(|x| is_acv(x.ship_type)) && spec(u, l).is_some_and(|la| la.emp_target == a.emp_target);
                if same {
                    return;
                }
            }
        }
        None => {
            let s = u.ships.ship(id);
            let stat = u.statics.ship(s.ship_type).expect("statics");
            let ctx = u.max_velocity_context(s);
            let max = st(crate::tactics::ships_max_velocity(s, stat, ctx));
            let limit = st(crate::tactics::speed_multiplier(s, stat) * ld(max));
            fly_to_target(u, id, tp, limit);
        }
    }
    u.ships.ship_mut(id).ship_idle = false;
}

fn fly_to_target(u: &mut Universe, id: ShipId, point: Vector, limit: f32) {
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(point), fly::POINT_IN_DIRECTION_FLYING | fly::CAR_TURN, limit, None);
}

/// `ACVRemoveShipReferences` (0x5a87c0).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(a) = spec_mut(u, id) else { return };
    let mut i = 0;
    while i < a.emp_targets.len() {
        if a.emp_targets[i] == gone {
            a.emp_targets.swap_remove(i);
        } else {
            i += 1;
        }
    }
    if a.emp_target == Some(gone) {
        a.emp_target = None;
        if a.emp_state == emp::MOVING_TO_TARGET {
            a.emp_state = emp::READY;
        }
    }
}

/// `ACVCancelEMPTarget` (0x5a99f0; slots 20 and 22, and `ACVCancelEMPTargetCheckOrder` slot
/// 27): stop moving to the EMP target.
pub fn cancel_emp_target(u: &mut Universe, id: ShipId) {
    if let Some(a) = spec_mut(u, id) {
        if a.emp_state == emp::MOVING_TO_TARGET {
            a.emp_state = emp::READY;
            a.emp_target = None;
        }
    }
}

/// The answer of `ACVCancelEMPTargetCheckOrder` (0x5a99a0): false while unlinking or with a
/// link partner.
pub fn may_cancel(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return true };
    if s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::UNLINK) {
        return false;
    }
    s.base.link.partner.is_none()
}
