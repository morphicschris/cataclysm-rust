//! The DFG frigate and its field (`DFGFrigate` module, 0x5ab420..0x5ab910): bDFGFrigate,
//! tDFGFrigate.
//!
//! The frigate carries a spherical field around it (`fieldRadius`). Enemy bullets that cross
//! a target inside the field get one roll (`univDFGFieldEffect`): `percentageThatGetThrough`
//! of them are unaffected, the rest are reflected (back along the line from the frigate,
//! turned by up to `angleVariance` degrees, keeping their speed) and live
//! `BulletLifeExtension` seconds longer. Big-gun shells detonate when they meet a field
//! (here, and in their flight test `DFGFrigateIsPointInAnyField`). Enemy sentinel field
//! formations near the frigate are disrupted (`DFGFrigateHouseKeep`).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::object::{BulletId, ShipId};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

/// Per-ship state (the header reserves four unused bytes).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DfgSpec {}

/// Per-type statics (`DFGFrigateStaticInit` 0x5ab420).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DfgStatics {
    /// `fieldRadius`, and its square.
    pub field_radius: f32,
    pub field_radius_sq: f32,
    /// `BulletDamageMultiplier` (read but not used by the field code).
    pub bullet_damage_multiplier: f32,
    /// `BulletLifeExtension`: seconds added to a reflected bullet's life.
    pub bullet_life_extension: f32,
    /// `percentageThatGetThrough`.
    pub percentage_through: u32,
    /// `angleVariance` in degrees.
    pub angle_variance: u32,
}

impl DfgStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        use crate::statics::cscan::{scan_f32, scan_i32};
        let mut s = Self::default();
        for (k, v) in &stat.custom_entries {
            match k.as_str() {
                "fieldRadius" => s.field_radius = scan_f32(v).unwrap_or(s.field_radius),
                "BulletDamageMultiplier" => s.bullet_damage_multiplier = scan_f32(v).unwrap_or(s.bullet_damage_multiplier),
                "BulletLifeExtension" => s.bullet_life_extension = scan_f32(v).unwrap_or(s.bullet_life_extension),
                "percentageThatGetThrough" => s.percentage_through = scan_i32(v).map_or(s.percentage_through, |x| x as u32),
                "angleVariance" => s.angle_variance = scan_i32(v).map_or(s.angle_variance, |x| x as u32),
                _ => {}
            }
        }
        s.field_radius_sq = st(ld(s.field_radius) * ld(s.field_radius));
        s
    }
}

/// Whether a type is a DFG frigate.
pub fn is_dfg(t: ShipType) -> bool {
    matches!(t, ShipType::bDFGFrigate | ShipType::tDFGFrigate)
}

fn statics(u: &Universe, t: ShipType) -> DfgStatics {
    match super::type_statics(u, t) {
        super::TypeStatics::Dfg(d) => d.clone(),
        _ => DfgStatics::default(),
    }
}

/// The frigate has no init callback.
pub fn init(u: &mut Universe, id: ShipId) {
    let _ = (u, id);
}

/// `DFGFrigateAttack` (0x5ab7e0): a passive attack that holds within its field.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let r = statics(u, u.ships.ship(id).ship_type).field_radius;
    crate::attack::attack_passive(u, id, target, r, 1000.0);
}

/// Sentinels whose field formation an enemy DFG frigate disrupts, with the time until which
/// their field stays down (`DFGFrigateHouseKeep` 0x5ab810: every frame whose update counter
/// has bit 2 or 3 set, each formation of type 9 or 10 whose leader is an enemy inside the
/// field gets each member's field-hit count set to 1 and its timer to now + 2 s).
pub fn disrupted_sentinels(u: &Universe, id: ShipId) -> Vec<(ShipId, f32)> {
    let mut out = Vec::new();
    if u.update_counter & 0xc == 0 {
        return out;
    }
    let Some(me) = u.ships.get(id) else { return out };
    let st_ = statics(u, me.ship_type);
    let until = st(ld(u.total_time_elapsed) + 2.0);
    for c in u.commands.iter() {
        if c.attributes & crate::command::attr::FORMATION == 0 || !matches!(c.formation.formation_type, 9 | 10) {
            continue;
        }
        let Some(leader) = c.selection.first().and_then(|&l| u.ships.get(l)) else { continue };
        if leader.player == me.player {
            continue;
        }
        let p = leader.posinfo.position;
        let q = me.posinfo.position;
        let (dx, dy, dz) = (st(ld(p.x) - ld(q.x)), st(ld(p.y) - ld(q.y)), st(ld(p.z) - ld(q.z)));
        if ((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz)) < ld(st_.field_radius_sq) {
            out.extend(c.selection.iter().map(|&s| (s, until)));
        }
    }
    out
}

/// `DFGFrigateHouseKeep` (0x5ab810): enemy sentinel fields inside the field go down for 2 s.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    for (s, until) in disrupted_sentinels(u, id) {
        crate::ships::sentinel::suppress_field(u, s, until);
    }
}

/// `DFGFrigateIsPointInAnyField` (0x5ab910).
pub fn is_point_in_any_field(u: &Universe, p: Vector) -> bool {
    u.ship_list.iter().any(|&sid| {
        u.ships.get(sid).is_some_and(|s| is_dfg(s.ship_type) && crate::shockwave::within_range_sq(p, s.posinfo.position, ld(statics(u, s.ship_type).field_radius_sq)))
    })
}

/// The DFG test of `collCheckBulletTargetColl` (`FUN_00438800`): a bullet that comes near a
/// DFG frigate (within its field, measured from the frigate's collision centre) feels the
/// field. Returns true when the bullet was destroyed.
pub fn bullet_near_ship(u: &mut Universe, ship: ShipId, b: BulletId) -> bool {
    let Some(s) = u.ships.get(ship) else { return false };
    if !is_dfg(s.ship_type) {
        return false;
    }
    let Some(bl) = u.bullets.get(b) else { return false };
    let c = s.coll.coll_position;
    let p = bl.posinfo.position;
    let (dx, dy, dz) = (st(ld(c.x) - ld(p.x)), st(ld(c.y) - ld(p.y)), st(ld(c.z) - ld(p.z)));
    let r2 = statics(u, s.ship_type).field_radius_sq;
    if !(((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz)) < ld(r2)) {
        return false;
    }
    field_effect(u, ship, b)
}

/// `univDFGFieldEffect` (0x5ab470): an enemy bullet in the field. Returns true when it was a
/// big-gun shell (which detonates and is removed).
pub fn field_effect(u: &mut Universe, dfg: ShipId, b: BulletId) -> bool {
    let Some(s) = u.ships.get(dfg) else { return false };
    let (dpos, dplayer, dtype) = (s.posinfo.position, s.player, s.ship_type);
    let Some(bl) = u.bullets.get(b) else { return false };
    if bl.player == dplayer || bl.dfg_flags & 1 != 0 {
        return false;
    }
    if crate::shockwave::is_big_gun_bullet(u, b) {
        let at = bl.posinfo.position;
        super::mothership::big_gun_bullet_hit(u, b, at);
        crate::bullet::delete(u, b);
        return true;
    }
    let st_ = statics(u, dtype);
    u.bullets.bullet_mut(b).dfg_flags |= 1;
    if u.randy.random(RAN_GAME) % 101 < st_.percentage_through {
        return false;
    }
    let bl = u.bullets.bullet(b);
    let v = bl.posinfo.velocity;
    let speed = st(fsqrt((ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z)));
    let bp = bl.posinfo.position;
    let mut away = Vector::new(st(ld(bp.x) - ld(dpos.x)), st(ld(bp.y) - ld(dpos.y)), st(ld(bp.z) - ld(dpos.z)));
    away.normalize();
    let frame = Matrix::coord_sys_from_heading(away);
    // (The deflection effect is presentation.)
    let r = u.randy.random(RAN_GAME) % (st_.angle_variance.wrapping_add(1));
    let deg = st(f64::from(r) * ld(0.017_453_292f32));
    let flip = u.randy.random(RAN_GAME) & 1 != 0;
    let a = if flip { -ld(deg) } else { ld(deg) };
    let (sn, cs) = (st(fsin(a)), st(fcos(a)));
    let m = Matrix::rot_about_z(cs, sn).mul_mat(&frame);
    let mut dir = m.col(0);
    if u.randy.random(RAN_GAME) & 1 != 0 {
        dir = Vector::new(st(-ld(dir.x)), st(-ld(dir.y)), st(-ld(dir.z)));
    }
    dir.normalize();
    let bm = u.bullets.bullet_mut(b);
    bm.coordsys = Matrix::coord_sys_from_heading(dir);
    bm.posinfo.velocity = Vector::new(st(ld(dir.x) * ld(speed)), st(ld(dir.y) * ld(speed)), st(ld(dir.z) * ld(speed)));
    bm.heading = dir;
    let len = ld(bm.length_mag);
    bm.length_vec = Vector::new(st(len * ld(dir.x)), st(len * ld(dir.y)), st(len * ld(dir.z)));
    bm.total_lifetime = st(ld(st_.bullet_life_extension) + ld(bm.total_lifetime));
    u.special_events.push(crate::universe::SpecialEvent::Special { ship: dfg, effect: super::Effect::FieldHit });
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_field() {
        let mut stat = ShipStaticInfo::new(ShipType::bDFGFrigate, String::new());
        for (k, v) in [("fieldRadius", "6000"), ("BulletLifeExtension", "1.0"), ("percentageThatGetThrough", "7"), ("angleVariance", "30")] {
            stat.custom_entries.push((k.into(), v.into()));
        }
        let s = DfgStatics::parse(&stat);
        assert_eq!((s.field_radius, s.field_radius_sq, s.percentage_through, s.angle_variance), (6000.0, 36_000_000.0, 7, 30));
    }
}
