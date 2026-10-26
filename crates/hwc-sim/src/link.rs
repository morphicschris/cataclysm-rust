//! Ship linking (`Dock.c`, Cataclysm only): two Acolytes become an ACV, two Mimics an MCV, two
//! Beast Acolytes a Beast ACV. The initiator flies under its partner, turns upside down against
//! it, both play their link animation and the merged ship replaces them (`processLinkToDo`).
//! Unlinking splits the merged ship again.

use hwc_math::fpu::{ld, st};
use hwc_math::{Matrix, Vector};

use crate::aiship::fly_to_point_avoiding_objs;
use crate::aitrack;
use crate::base::{self, BaseEvent, CreateOptions};
use crate::command::{self, order};
use crate::object::CommandId;
use crate::object::{sof, ShipId};
use crate::statics::ShipType;
use crate::universe::Universe;

/// Link states (`ship+0x654`).
pub mod state {
    pub const NONE: u32 = 0;
    pub const APPROACH: u32 = 2;
    pub const ALIGN: u32 = 3;
    pub const MERGE: u32 = 4;
    pub const PARTNER: u32 = 5;
}

/// How long the link animation may take before the merge (`stopwatchStart(.., 2.0)`).
const LINK_TIME: f32 = 2.0;

/// The merged type of a pair.
pub fn linked_type(t: ShipType) -> Option<ShipType> {
    match t {
        ShipType::sAcolyte => Some(ShipType::sACV),
        ShipType::sMimic => Some(ShipType::sMCV),
        ShipType::bAcolyte => Some(ShipType::bACV),
        _ => None,
    }
}

/// `dockLinkShipsBegin` (0x463080) with the link order: `a` flies to `b` and links with it.
pub fn cl_link(u: &mut Universe, a: ShipId, b: ShipId) -> bool {
    let (Some(sa), Some(sb)) = (u.ships.get(a), u.ships.get(b)) else { return false };
    if sa.ship_type != sb.ship_type || linked_type(sa.ship_type).is_none() || sa.player != sb.player || a == b {
        return false;
    }
    command::remove_ships_from_doing_stuff(u, &[a, b]);
    if u.ships.ship(a).ship_type == ShipType::sMimic {
        crate::ships::mimic::end_disguise(u, a, true);
        crate::ships::mimic::end_disguise(u, b, true);
    }
    let sa = u.ships.ship_mut(a);
    sa.base.link = base::LinkSpec { partner: Some(b), state: state::APPROACH, timer: 0.0 };
    let sb = u.ships.ship_mut(b);
    sb.base.link = base::LinkSpec { partner: Some(a), state: state::PARTNER, timer: 0.0 };
    let mut c = command::new_command_pub(vec![a]);
    c.order = order::LINK;
    c.link_with = Some(b);
    let cid = u.commands.add_pub(c);
    command::init_ships_for_ai(u, &[a]);
    command::prepare_ships_pub(u, cid);
    true
}

/// Upgrade bit of the ships that link (Acolytes, Mimics: `TECH_LINK`).
pub const UPGRADE_LINK: u32 = 0x8;
/// Upgrade bit of the linked ships that split again (ACVs, MCVs: `TECH_LINK`).
pub const UPGRADE_UNLINK: u32 = 0x10;
/// Static capability bit of the linked types (ACV, MCV, Beast ACV).
const CAP_LINKED: u32 = 0x40;

/// `dockLinkShipsBegin` (0x463080): `a` becomes the initiator (it flies under `b`), `b` the
/// passive partner. A Mimic pair drops its disguises.
pub fn dock_link_ships_begin(u: &mut Universe, a: ShipId, b: ShipId) {
    u.ships.ship_mut(a).base.link = base::LinkSpec { partner: Some(b), state: state::APPROACH, timer: 0.0 };
    u.ships.ship_mut(b).base.link = base::LinkSpec { partner: Some(a), state: state::PARTNER, timer: 0.0 };
    if u.ships.ship(a).ship_type == ShipType::sMimic {
        crate::ships::mimic::end_disguise(u, a, true);
        crate::ships::mimic::end_disguise(u, b, true);
    }
}

/// The per-ship reset before a link order (0x43fb30): a linkable ship lets go of any partner.
fn release_partner(u: &mut Universe, id: ShipId) {
    let t = u.ships.ship(id).ship_type;
    if !matches!(t, ShipType::sAcolyte | ShipType::sACV | ShipType::sMimic | ShipType::sMCV | ShipType::bAcolyte | ShipType::bACV) {
        return;
    }
    if let Some(p) = u.ships.ship(id).base.link.partner {
        if let Some(ps) = u.ships.get_mut(p) {
            ps.special_flags2 &= !base::special2::DOCKING;
            ps.base.link.partner = None;
            base::set_disable_reason(ps, false, 3);
        }
        let s = u.ships.ship_mut(id);
        s.special_flags2 &= !base::special2::DOCKING;
        s.base.link.partner = None;
    }
    base::set_disable_reason(u.ships.ship_mut(id), false, 3);
}

/// The link partner search of `clLink` (0x448d80): the nearest ship of `ship`'s type in
/// `sel` that has the link upgrade and no partner yet (universe ship-list order on ties).
fn nearest_link_partner(u: &Universe, sel: &[ShipId], ship: ShipId) -> Option<ShipId> {
    let me = u.ships.ship(ship);
    let mut best = 1e20f32;
    let mut found = None;
    for &id in &u.ship_list {
        let Some(s) = u.ships.get(id) else { continue };
        if !crate::upgrade::has_upgrade(s, UPGRADE_LINK) || !sel.contains(&id) || s.ship_type != me.ship_type || id == ship || s.base.link.partner.is_some() {
            continue;
        }
        let (p, q) = (s.posinfo.position, me.posinfo.position);
        let (dx, dy, dz) = (ld(p.x) - ld(q.x), ld(p.y) - ld(q.y), ld(p.z) - ld(q.z));
        let d = (dz * dz + dy * dy + dx * dx).sqrt();
        if d < f64::from(best) {
            best = d as f32;
            found = Some(id);
        }
    }
    found
}

/// A ship dropped from a link or unlink order before it starts.
fn drop_from_order(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.command = None;
    if s.base.disable_reasons[5] {
        base::set_disable_reason(s, false, 5);
    }
}

/// `clLink` (0x448e50): the ships pair up, each with the nearest unpaired ship of its type in
/// the selection; of each pair the lower one (smaller z) flies under the other. One link
/// order holds the initiators; the partners and the ships left without a partner leave the
/// order. Returns the order, if any ship got one.
pub fn cl_link_ships(u: &mut Universe, selection: &[ShipId]) -> Option<CommandId> {
    let mut list: Vec<ShipId> = selection.iter().copied().filter(|&s| u.ships.get(s).is_some()).collect();
    if list.is_empty() {
        return None;
    }
    command::remove_ships_from_doing_stuff(u, &list);
    for &s in &list {
        release_partner(u, s);
    }
    let mut i = 0;
    while i < list.len() {
        let ship = list[i];
        if u.ships.ship(ship).base.link.partner.is_some() {
            list.swap_remove(i);
            drop_from_order(u, ship);
            continue;
        }
        let Some(p) = nearest_link_partner(u, &list, ship) else {
            list.swap_remove(i);
            drop_from_order(u, ship);
            continue;
        };
        let (a, b) = if u.ships.ship(p).posinfo.position.z < u.ships.ship(ship).posinfo.position.z {
            let j = list.iter().position(|&x| x == p).expect("partner in the selection");
            list.swap(i, j);
            (p, ship)
        } else {
            (ship, p)
        };
        dock_link_ships_begin(u, a, b);
        i += 1;
    }
    if list.is_empty() {
        return None;
    }
    let mut c = command::new_command_pub(list.clone());
    c.order = order::LINK;
    let cid = u.commands.add_pub(c);
    command::prepare_ships_pub(u, cid);
    command::init_ships_for_ai(u, &list);
    Some(cid)
}

/// `dockUnlinkShipBegin` (0x463b00): the linked ship's split starts (an MCV drops its
/// disguise).
fn dock_unlink_ship_begin(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.base.link = base::LinkSpec { partner: Some(id), state: state::NONE, timer: 0.0 };
    if s.ship_type == ShipType::sMCV {
        crate::ships::mimic::mcv_end_disguise(u, id, true);
    }
}

/// `clUnlink` (0x449240): every linked ship of the selection (static capability 0x40) gets
/// one unlink order.
pub fn cl_unlink_ships(u: &mut Universe, selection: &[ShipId]) -> Option<CommandId> {
    let list: Vec<ShipId> = selection
        .iter()
        .copied()
        .filter(|&s| u.ships.get(s).is_some_and(|x| u.statics.ship(x.ship_type).is_some_and(|st| st.capability_flags & CAP_LINKED != 0)))
        .collect();
    if list.is_empty() {
        return None;
    }
    command::remove_ships_from_doing_stuff(u, &list);
    let mut c = command::new_command_pub(list.clone());
    c.order = order::UNLINK;
    let cid = u.commands.add_pub(c);
    for &s in &list {
        dock_unlink_ship_begin(u, s);
    }
    command::prepare_ships_pub(u, cid);
    command::init_ships_for_ai(u, &list);
    Some(cid)
}

fn neg(v: Vector) -> Vector {
    Vector::new(-v.x, -v.y, -v.z)
}

/// `processLinkToDo` (0x463120). True when every ship of the command is done.
pub fn process_link_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let Some(sel) = u.commands.get(cid).map(|c| c.selection.clone()) else { return true };
    let mut done = true;
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let dt = u.phys_time_elapsed;
    for ship in sel {
        let Some(s) = u.ships.get(ship) else { continue };
        let Some(p) = s.base.link.partner.filter(|p| u.ships.get(*p).is_some()) else { continue };
        let link_dist = statics.ship(s.ship_type).map_or(0.0, |x| x.link_distance);
        let pt = u.ships.ship(p).clone();
        let (pup, pright, ph) = (pt.up(), pt.right(), pt.heading());
        match s.base.link.state {
            state::APPROACH => {
                let k = st(ld(link_dist) + ld(link_dist));
                let target = Vector::new(st(ld(k) * -ld(pup.x) + ld(pt.posinfo.position.x)), st(ld(k) * -ld(pup.y) + ld(pt.posinfo.position.y)), st(ld(k) * -ld(pup.z) + ld(pt.posinfo.position.z)));
                fly_to_point_avoiding_objs(u, ship, Some(target), 0xb3, -80.0, Some(pt.posinfo.velocity));
                let me = u.ships.ship(ship).posinfo.position;
                if (ld(me.x) - ld(target.x)).abs() < 5.0 && (ld(me.y) - ld(target.y)).abs() < 5.0 && (ld(me.z) - ld(target.z)).abs() < 5.0 {
                    let sh = u.ships.ship_mut(ship);
                    sh.posinfo.position = target;
                    sh.posinfo.velocity = pt.posinfo.velocity;
                    sh.base.link.state = state::ALIGN;
                }
            }
            state::ALIGN => {
                let q = Vector::new(st(ld(link_dist) * -ld(pup.x)), st(ld(link_dist) * -ld(pup.y)), st(ld(link_dist) * -ld(pup.z)));
                let meet = Vector::new(st(ld(pt.posinfo.position.x) + ld(q.x)), st(ld(pt.posinfo.position.y) + ld(q.y)), st(ld(pt.posinfo.position.z) + ld(q.z)));
                let below = Vector::new(st(ld(meet.x) + ld(q.x)), st(ld(meet.y) + ld(q.y)), st(ld(meet.z) + ld(q.z)));
                let want = Matrix::from_cols(neg(pup), neg(pright), ph);
                if let Some(stat) = statics.ship(s.ship_type) {
                    aitrack::heading_and_up(u.ships.ship_mut(ship), stat, dt, ph, neg(pup), f32::from_bits(0x3f7f_bce7));
                }
                let sh = u.ships.ship_mut(ship);
                sh.posinfo.position = below;
                sh.posinfo.velocity = pt.posinfo.velocity;
                let aligned = {
                    let a = &sh.rotinfo.coordsys;
                    let d = |i: usize| (ld(want.m[i]) * ld(a.m[i]) + ld(want.m[i + 1]) * ld(a.m[i + 1])) + ld(want.m[i + 2]) * ld(a.m[i + 2]);
                    let th = ld(f32::from_bits(0x3f7f_0069));
                    !(d(0) < th) && !(d(6) < th) && !(d(3) < th)
                };
                if aligned {
                    sh.posinfo.position = meet;
                    sh.rotinfo.coordsys = want;
                    sh.flags |= sof::NO_BUMPS;
                    sh.base.link.state = state::MERGE;
                    sh.base.link.timer = st(ld(now) + ld(LINK_TIME));
                    base::set_disable_reason(sh, true, 3);
                    let ps = u.ships.ship_mut(p);
                    ps.posinfo.position = meet;
                    ps.flags |= sof::NO_BUMPS;
                    base::set_disable_reason(ps, true, 3);
                }
            }
            state::MERGE => {
                let sh = u.ships.ship_mut(ship);
                sh.posinfo.position = pt.posinfo.position;
                sh.rotinfo.coordsys = Matrix::from_cols(neg(pup), neg(pright), ph);
                sh.posinfo.velocity = pt.posinfo.velocity;
                // The link animations are not modelled; the merge happens when the time the
                // original allows them runs out.
                if !(now >= sh.base.link.timer) {
                    done = false;
                    continue;
                }
                merge(u, ship, p);
                continue;
            }
            _ => continue,
        }
        done = false;
    }
    done
}

/// The merge: the linked ship replaces both (health the mean of their health fractions, kills
/// added up).
fn merge(u: &mut Universe, a: ShipId, b: ShipId) {
    let statics = u.statics.clone();
    let (sa, sb) = (u.ships.ship(a).clone(), u.ships.ship(b).clone());
    let Some(t) = linked_type(sa.ship_type) else { return };
    let Ok(id) = base::create_ship(u, t, sa.player, sb.posinfo.position, CreateOptions::default()) else { return };
    crate::construction::unit_cap_create_ship(u, id);
    let (ma, mb) = (statics.ship(sa.ship_type).map_or(1.0, |x| x.health.max_health), statics.ship(sb.ship_type).map_or(1.0, |x| x.health.max_health));
    let mn = statics.ship(t).map_or(1.0, |x| x.health.max_health);
    let n = u.ships.ship_mut(id);
    n.rotinfo.coordsys = if t == ShipType::sMCV {
        Matrix::from_cols(sb.right(), neg(sb.up()), sb.heading())
    } else {
        sb.rotinfo.coordsys
    };
    let h = st((ld(st(ld(sa.health) / ld(ma))) + ld(st(ld(sb.health) / ld(mb)))) * 0.5 * ld(mn));
    n.health = h;
    n.tactics = if sa.tactics == sb.tactics { sa.tactics } else { crate::object::tactics::NEUTRAL };
    for k in 0..12 {
        n.kills[k] = sa.kills[k] + sb.kills[k];
    }
    if let Some(stat) = statics.ship(t) {
        crate::physics::update_obj_rot_info(n, stat);
    }
    crate::ships::acv::remember_acolytes(u, id, a, b);
    crate::damage::really_delete_ship(u, a);
    crate::damage::really_delete_ship(u, b);
    u.base.events.push(BaseEvent::Linked { new_ship: id });
}

/// `dockUnlinkShipBegin` + order: split a linked ship back into its two parts.
pub fn cl_unlink(u: &mut Universe, ship: ShipId) -> bool {
    let Some(s) = u.ships.get(ship) else { return false };
    if !matches!(s.ship_type, ShipType::sACV | ShipType::sMCV | ShipType::bACV) {
        return false;
    }
    command::remove_ships_from_doing_stuff(u, &[ship]);
    if u.ships.ship(ship).ship_type == ShipType::sMCV {
        crate::ships::mimic::mcv_end_disguise(u, ship, true);
    }
    let mut c = command::new_command_pub(vec![ship]);
    c.order = order::UNLINK;
    let cid = u.commands.add_pub(c);
    command::prepare_ships_pub(u, cid);
    true
}

/// `processUnlinkToDo` (0x463bd0), reduced: the two parts appear either side of the linked
/// ship (the split animation and turn are not modelled).
pub fn process_unlink_to_do(u: &mut Universe, cid: CommandId) -> bool {
    let Some(sel) = u.commands.get(cid).map(|c| c.selection.clone()) else { return true };
    for ship in sel {
        let Some(s) = u.ships.get(ship).cloned() else { continue };
        let part = match s.ship_type {
            ShipType::sACV => ShipType::sAcolyte,
            ShipType::sMCV => ShipType::sMimic,
            ShipType::bACV => ShipType::bAcolyte,
            _ => continue,
        };
        let d = u.statics.ship(part).map_or(50.0, |x| x.link_distance);
        let up = s.up();
        let mut parts = Vec::new();
        for (half, sign) in [1.0f32, -1.0].into_iter().enumerate() {
            let p = Vector::new(st(ld(s.posinfo.position.x) + ld(sign * d) * ld(up.x)), st(ld(s.posinfo.position.y) + ld(sign * d) * ld(up.y)), st(ld(s.posinfo.position.z) + ld(sign * d) * ld(up.z)));
            if let Ok(id) = base::create_ship(u, part, s.player, p, CreateOptions::default()) {
                crate::construction::unit_cap_create_ship(u, id);
                let n = u.ships.ship_mut(id);
                n.rotinfo.coordsys = s.rotinfo.coordsys;
                n.posinfo.velocity = s.posinfo.velocity;
                if crate::ships::acv::is_acv(s.ship_type) {
                    let state = crate::ships::acv::link_missile_counter(u, ship, half);
                    crate::ships::acolyte::restore_after_unlink(u, id, state);
                }
                parts.push(id);
            }
        }
        // A splitting MCV hands its pending disguise to both Mimics, which morph into it (at
        // the end of the split animation in the original).
        if crate::ships::mimic::is_pending_split(u, ship) {
            for &p in &parts {
                crate::ships::mimic::copy_disguise_from(u, p, ship);
            }
            for &p in &parts {
                crate::ships::mimic::start_morph(u, p);
            }
        }
        crate::damage::really_delete_ship(u, ship);
    }
    true
}

/// `FreeLastOrder` for link orders: the pair lets go of each other.
pub fn free_link_order(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if let Some(p) = s.base.link.partner {
        if let Some(ps) = u.ships.get_mut(p) {
            ps.base.link = base::LinkSpec::default();
            ps.special_flags2 &= !base::special2::DOCKING;
            base::set_disable_reason(ps, false, 3);
        }
    }
    let s = u.ships.ship_mut(id);
    s.base.link = base::LinkSpec::default();
    s.special_flags2 &= !base::special2::DOCKING;
    base::set_disable_reason(s, false, 3);
}

/// A dying ship's link partner lets go.
pub fn ship_died(u: &mut Universe, id: ShipId) {
    if let Some(p) = u.ships.get(id).and_then(|s| s.base.link.partner) {
        if let Some(ps) = u.ships.get_mut(p) {
            ps.base.link = base::LinkSpec::default();
            base::set_disable_reason(ps, false, 3);
        }
    }
}
