//! Damage, death and explosions (`Damage.c`/`UnivUpdate.c` as compiled into Cataclysm):
//! what a bullet does to the ship it hits (`univBulletCollidedWithTarget` 0x541ac0),
//! `ApplyDamageToTarget` (0x541310), veterancy (`suAnnounceUpgrade` 0x50f9a0), the death list
//! (`AddShipToDeleteShipList`), the shock wave of an exploding ship (0x540fa0) and the removal
//! of dead ships (`univDeleteDeadShips`).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_u32, ld, st, Ext};
use hwc_math::random::RAN_GAME;
use hwc_math::Vector;

use crate::combat_statics::{class_index, WEAPON_DAMAGE, DAMAGE_UPGRADE_TABLE};
use crate::object::{sof, ObjRef, ShipId};
use crate::statics::{ShipClass, ShipType};
use crate::universe::{SimEvent, Universe};

/// `howDidIDie` / death causes as the original numbers them.
pub mod how {
    /// A bullet or beam fired by a player's ship (`(who, 666)`).
    pub const KILLED_BY_PLAYER: i32 = 666;
    /// The shock wave of an exploding ship of a player.
    pub const KILLED_BY_SHOCKWAVE: i32 = 667;
    /// A kamikaze ship of a player (`(who, 668)`).
    pub const KILLED_BY_KAMIKAZE: i32 = 668;
    /// Collision damage.
    pub const KILLED_BY_COLLISION: i32 = 777;
    /// No player responsible.
    pub const KILLED_BY_UNKNOWN: i32 = 999;
}

/// Veterancy: kill weights per target class (0x721b8c).
const KILL_WEIGHTS: [f32; 12] = [20.0, 8.0, 10.0, 9.0, 5.0, 3.0, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0];
/// Veterancy: the kill score above which a ship of each class reaches levels 1..4 (0x721bbc).
const VETERANCY_THRESHOLDS: [[f32; 4]; 12] = [
    [10.0, 30.0, 60.0, 100.0],
    [10.0, 30.0, 60.0, 100.0],
    [10.0, 30.0, 60.0, 100.0],
    [5.0, 10.0, 20.0, 40.0],
    [3.0, 6.0, 12.0, 16.0],
    [2.0, 4.0, 6.0, 8.0],
    [1.0, 2.0, 3.0, 4.0],
    [10.0, 30.0, 60.0, 100.0],
    [10.0, 30.0, 60.0, 100.0],
    [2.0, 4.0, 6.0, 8.0],
    [10.0, 30.0, 60.0, 100.0],
    [2.0, 4.0, 6.0, 8.0],
];
/// Highest veterancy level.
pub const MAX_UPGRADE_LEVEL: u32 = 4;

/// The weapon damage factor of a ship (0x50fac0): its veterancy step, doubled in single-player
/// games on the easiest setting.
pub fn damage_multiplier(u: &Universe, s: &crate::object::Ship) -> Ext {
    let level = (s.speed_upgrade_level as usize).min(DAMAGE_UPGRADE_TABLE.len() - 1);
    let mut f = ld(DAMAGE_UPGRADE_TABLE[level]);
    // The Beast mothership adds its damage-mode factor (`MothershipGetDamageModifier3`).
    if s.ship_type == ShipType::bMothership {
        f += ld(crate::ships::mothership::damage_modifier_damage(s));
    }
    if u.single_player && u.sp_difficulty == 0 {
        f += f;
    }
    f
}

/// `suGetUpgradeMultiplier`: base damage of a bullet type against an armour level.
pub fn weapon_damage(bullet_type: u32, armour: u32) -> f32 {
    WEAPON_DAMAGE.get(bullet_type as usize).map_or(0.0, |row| row[(armour as usize).min(2)])
}

/// `univBulletCollidedWithTarget` (0x541ac0) for ships: knock-back and spin, side and
/// friendly-fire modifiers, the "under attack" state, then the damage. `apply` is false for
/// hits that only make an effect.
pub fn bullet_collided_with_target(u: &mut Universe, apply: bool, tid: ShipId, bid: crate::object::BulletId, t: f32, side: i32) {
    let hit = bullet_collided_inner(u, apply, tid, bid, t, side);
    // The hit effect, with the damage done and `ApplyDamageToTarget`'s result (1.0f when the
    // hit did no damage).
    if let Some((damage, fatal)) = hit {
        crate::effects::bullet_hit(u, bid, ObjRef::Ship(tid), t, damage, fatal);
    }
}

/// `univBulletCollidedWithTarget` (0x541ac0) for a mine: the hit rocks it unless it is
/// immobile or unbumpable, and it takes the bullet's damage against no armour (no side,
/// friendly-fire or armour-piercing modifiers).
pub fn bullet_collided_with_mine(u: &mut Universe, apply: bool, mid: crate::object::MissileId, bid: crate::object::BulletId, t: f32, _side: i32) {
    const NO_DAMAGE: u32 = 0x3f80_0000;
    let Some(b) = u.bullets.get(bid).cloned() else { return };
    if b.bullet_type == crate::ships::mothership::BIG_GUN_BULLET_TYPE {
        crate::ships::mothership::big_gun_bullet_hit(u, bid, b.posinfo.position);
    }
    let damage = st(ld(weapon_damage(b.bullet_type, 0)) * ld(b.damage));
    let Some(m) = u.missiles.get(mid) else { return };
    let Some(mst) = crate::missile::statics_of(u, m) else { return };
    let immobile = mst.header.immobile;
    let fatal = if apply {
        if !immobile && m.flags & sof::NOT_BUMPABLE == 0 {
            let tw = u.tweaks.combat.clone();
            let k = ld(b.mass) / ld(u.phys_time_elapsed);
            let v = b.posinfo.velocity;
            let push = Vector::new(st(ld(v.x) * k), st(ld(v.y) * k), st(ld(v.z) * k));
            let mass = ld(b.mass);
            let r0 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[0][0], tw.bullet_rock[0][1]);
            let m = u.missiles.missile_mut(mid);
            m.rotinfo.torque.x = st(r0 * mass + ld(m.rotinfo.torque.x));
            let r1 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[1][0], tw.bullet_rock[1][1]);
            let m = u.missiles.missile_mut(mid);
            m.rotinfo.torque.y = st(r1 * mass + ld(m.rotinfo.torque.y));
            let r2 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[2][0], tw.bullet_rock[2][1]);
            let m = u.missiles.missile_mut(mid);
            m.rotinfo.torque.z = st(r2 * mass + ld(m.rotinfo.torque.z));
            let f = &mut m.posinfo.force;
            f.x = st(ld(push.x) + ld(f.x));
            f.y = st(ld(push.y) + ld(f.y));
            f.z = st(ld(push.z) + ld(f.z));
        }
        u32::from(crate::missile::apply_damage(u, mid, damage, b.bullet_type as i32))
    } else {
        NO_DAMAGE
    };
    crate::effects::bullet_hit(u, bid, ObjRef::Missile(mid), t, damage, fatal);
}

/// [`bullet_collided_with_target`] without the effect; returns the effect's damage and
/// "fatal" parameters (`None`: the ship or bullet is gone).
fn bullet_collided_inner(u: &mut Universe, apply: bool, tid: ShipId, bid: crate::object::BulletId, t: f32, side: i32) -> Option<(f32, u32)> {
    const NO_DAMAGE: u32 = 0x3f80_0000;
    let statics = u.statics.clone();
    let b = u.bullets.get(bid).cloned()?;
    // A big-gun shell detonates on whatever it hits (`MothershipBigGunBulletHit2`).
    if b.bullet_type == crate::ships::mothership::BIG_GUN_BULLET_TYPE {
        crate::ships::mothership::big_gun_bullet_hit(u, bid, b.posinfo.position);
    }
    let ship = u.ships.get(tid)?;
    let stat = statics.ship(ship.ship_type)?;
    let mut damage = st(ld(weapon_damage(b.bullet_type, ship.armour_level)) * ld(b.damage));
    if !apply {
        return Some((damage, NO_DAMAGE));
    }
    // A sentinel whose force field is up: the field takes the hit instead.
    if crate::ships::sentinel::absorbs_hit(u, tid, &b) {
        return Some((damage, NO_DAMAGE));
    }
    let ship = u.ships.get(tid)?;
    let tw = u.tweaks.combat.clone();
    if !stat.header.immobile && ship.flags & sof::NOT_BUMPABLE == 0 && ship.special_flags & 0x1000_0000 == 0 {
        let k = ld(b.mass) / ld(u.phys_time_elapsed);
        let v = b.posinfo.velocity;
        let push = Vector::new(st(ld(v.x) * k), st(ld(v.y) * k), st(ld(v.z) * k));
        let mass = ld(b.mass);
        let r0 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[0][0], tw.bullet_rock[0][1]);
        let s = u.ships.ship_mut(tid);
        s.rotinfo.torque.x = st(r0 * mass + ld(s.rotinfo.torque.x));
        let r1 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[1][0], tw.bullet_rock[1][1]);
        let s = u.ships.ship_mut(tid);
        s.rotinfo.torque.y = st(r1 * mass + ld(s.rotinfo.torque.y));
        let r2 = u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[2][0], tw.bullet_rock[2][1]);
        let s = u.ships.ship_mut(tid);
        s.rotinfo.torque.z = st(r2 * mass + ld(s.rotinfo.torque.z));
        let f = &mut s.posinfo.force;
        f.x = st(ld(push.x) + ld(f.x));
        f.y = st(ld(push.y) + ld(f.y));
        f.z = st(ld(push.z) + ld(f.z));
    }
    if side >= 0 {
        damage = st(ld(damage) * ld(stat.coll_side_modifiers[side as usize]));
    }
    let hit_pos = b.posinfo.position.add_scaled(b.heading, t);
    u.events.push(SimEvent::BulletHit { bullet: bid, ship: tid, position: hit_pos, beam: crate::bullet::is_beam(b.weapon), bullet_type: b.bullet_type });
    let owner = b.owner.and_then(|o| u.ships.get(o)).map(|o| (o.id, o.ship_type, o.player));
    match owner {
        Some((oid, otype, oplayer)) => {
            let target_class = stat.ship_class;
            if matches!(otype, ShipType::kHeavyDefender | ShipType::tHeavyDefender) && target_class == Some(ShipClass::Fighter) {
                damage = st(ld(tw.armour_piercing_modifier) * ld(damage));
            }
            let s = u.ships.ship_mut(tid);
            if oplayer == s.player {
                damage = st(ld(tw.friendly_fire_modifier) * ld(damage));
                s.chatter_bits |= 1;
            }
            // (Alliances and the "under attack" speech are not modelled.)
            s.getting_rocked = Some(oid);
            s.recent_attacker = Some(oid);
            s.recently_attacked = crate::object::RECENT_ATTACK_DURATION;
            let fatal = apply_damage_to_target(u, tid, damage, b.bullet_type as i32, how::KILLED_BY_PLAYER, oplayer as i32, Some(oid));
            Some((damage, u32::from(fatal)))
        }
        None => {
            let fatal = apply_damage_to_target(u, tid, damage, b.bullet_type as i32, how::KILLED_BY_UNKNOWN, 99, None);
            Some((damage, u32::from(fatal)))
        }
    }
}

/// `ApplyDamageToTarget` (0x541310) for ships: takes `damage` off the health and, when that
/// kills the ship, credits the killer and puts the ship on the death list. Returns whether the
/// ship died now.
pub fn apply_damage_to_target(u: &mut Universe, tid: ShipId, damage: f32, bullet_type: i32, how: i32, who: i32, killer: Option<ShipId>) -> bool {
    let statics = u.statics.clone();
    // A mothership's own module (a component flagged `SOF_MODULE`) passes the damage to the
    // ship it is clamped to: the modules a mothership starts with never die on their own.
    let tid = match u.ships.get(tid) {
        Some(s) if s.flags & crate::base::SOF_MODULE != 0 && statics.ship(s.ship_type).is_some_and(|st| st.ship_class == Some(ShipClass::Component)) => {
            crate::clamp::host_of(u, tid).filter(|&h| u.ships.get(h).is_some()).unwrap_or(tid)
        }
        _ => tid,
    };
    // No ship is hurt while an NIS plays (`nisIsRunning`).
    if u.nis_running {
        return false;
    }
    let Some(s) = u.ships.get_mut(tid) else { return false };
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let old = s.health;
    let alive = s.flags & sof::DEAD == 0;
    // (Evasive ships retreating to dock when badly damaged need a dock target; not modelled.)
    if s.flags2 & 4 != 0 {
        return false;
    }
    // Nor while the letterbox is in, unless the mission lets this ship be hurt then.
    if crate::letterbox::shields(u, u.ships.ship(tid)) {
        return false;
    }
    let Some(s) = u.ships.get_mut(tid) else { return false };
    s.health = st(ld(s.health) - ld(damage));
    let max = ld(stat.health.max_health);
    if 0.25 <= ld(old) / max && ld(s.health) / max < 0.25 {
        s.chatter_bits |= 8;
    }
    if 0.0 < s.health || !alive {
        return false;
    }
    s.how_did_i_die = how;
    s.who_killed_me = who;
    let victim_player = s.player;
    let victim_class = class_index(stat.ship_class);
    if let Some(k) = killer {
        let hive = crate::ships::hive::swarmer_hive(u, k);
        if let Some(ks) = u.ships.get_mut(k) {
            if ks.flags & sof::DEAD == 0 && ks.player != victim_player {
                if matches!(ks.ship_type, ShipType::sSwarmer | ShipType::bSwarmer) {
                    // Swarmers credit their hive frigate (`+0x644`).
                    if let Some(h) = hive.filter(|&h| u.ships.get(h).is_some()) {
                        u.ships.ship_mut(h).kills[victim_class] += 1;
                        announce_upgrade(u, h);
                    }
                } else {
                    ks.kills[victim_class] += 1;
                    announce_upgrade(u, k);
                }
            }
        }
    }
    add_ship_to_delete_list(u, tid, bullet_type);
    true
}

/// `suAnnounceUpgrade` (0x50f9a0): recomputes a ship's veterancy level from its kills.
pub fn announce_upgrade(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let Some(s) = u.ships.get_mut(id) else { return };
    if s.speed_upgrade_level == MAX_UPGRADE_LEVEL {
        return;
    }
    let mut score = 0f32;
    for (k, w) in s.kills.iter().zip(KILL_WEIGHTS) {
        score = st(fild_u32(*k) * ld(w) + ld(score));
    }
    let class = statics.ship(s.ship_type).map_or(0, |st| class_index(st.ship_class));
    let before = s.speed_upgrade_level;
    s.speed_upgrade_level = 0;
    for (i, &th) in VETERANCY_THRESHOLDS[class.min(11)].iter().enumerate() {
        if ld(th) < ld(score) {
            s.speed_upgrade_level = i as u32 + 1;
        }
    }
    if s.speed_upgrade_level > before {
        let level = s.speed_upgrade_level;
        u.events.push(SimEvent::Upgraded { ship: id, level });
    }
}

/// `AddShipToDeleteShipList`: the ship is dead from now on; it careens, its shock wave goes
/// out, and it is removed at the end of the frame. `bullet_type` -1 marks a scuttle.
pub fn add_ship_to_delete_list(u: &mut Universe, id: ShipId, bullet_type: i32) {
    let statics = u.statics.clone();
    let tw = u.tweaks.combat.clone();
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let Some(stat) = statics.ship(t) else { return };
    // The Somtaaw mothership dies in its own time (`MothershipHouseKeep`).
    if t == ShipType::sMothership && crate::ships::mothership::start_dying(u, id, bullet_type) {
        return;
    }
    u.ships.ship_mut(id).flags |= sof::DEAD;
    if !stat.header.immobile {
        let r0 = u.randy.frandom_between_ext(RAN_GAME, tw.careen_rock[0][0], tw.careen_rock[0][1]);
        let s = u.ships.ship_mut(id);
        s.rotinfo.torque.x = st(r0 + ld(s.rotinfo.torque.x));
        let r1 = u.randy.frandom_between_ext(RAN_GAME, tw.careen_rock[1][0], tw.careen_rock[1][1]);
        let s = u.ships.ship_mut(id);
        s.rotinfo.torque.y = st(r1 + ld(s.rotinfo.torque.y));
        let r2 = u.randy.frandom_between_ext(RAN_GAME, tw.careen_rock[2][0], tw.careen_rock[2][1]);
        let s = u.ships.ship_mut(id);
        s.rotinfo.torque.z = st(r2 + ld(s.rotinfo.torque.z));
    }
    queue_for_deletion(u, id, bullet_type, bullet_type == -1);
}

/// The second half of the Somtaaw mothership's death (`MothershipHouseKeep`, state 2): it is
/// dead now and goes like any other ship, without the careen.
pub fn finish_ship_death(u: &mut Universe, id: ShipId, bullet_type: i32) {
    let Some(s) = u.ships.get_mut(id) else { return };
    s.flags |= sof::DEAD;
    queue_for_deletion(u, id, bullet_type, false);
}

/// The explosion, its shock wave and the place on the delete list.
fn queue_for_deletion(u: &mut Universe, id: ShipId, bullet_type: i32, scuttle: bool) {
    let statics = u.statics.clone();
    let Some(stat) = u.ships.get(id).and_then(|s| statics.ship(s.ship_type)) else { return };
    let s = u.ships.ship(id);
    let explosion_type = stat.custom_entries.iter().find(|(k, _)| k == "explosionType").map(|(_, v)| v.split_whitespace().next().unwrap_or("").to_string());
    u.events.push(SimEvent::ShipDied { ship: id, ship_type: s.ship_type, position: s.posinfo.position, radius: stat.header.coll.coll_sphere_size, explosion_type });
    if let Some(loss) = crate::battle_status::loss_of(u, id) {
        u.battle_losses.push((id, loss));
    }
    shockwave(u, id, scuttle);
    let death_by = crate::effects::death_by(u, id, bullet_type);
    u.dead_ships.push(id);
    u.dead_ship_death_by.push(death_by);
}

/// The shock wave of an exploding ship (0x540fa0): pushes and spins everything impactable
/// within the blast radius and damages targetable ships, falling off linearly with distance.
fn shockwave(u: &mut Universe, id: ShipId, scuttle: bool) {
    let statics = u.statics.clone();
    let tw = u.tweaks.combat.clone();
    let me = u.ships.ship(id).clone();
    let Some(stat) = statics.ship(me.ship_type) else { return };
    let mult = if scuttle { tw.blast_radius_scuttle_bonus } else { 1.0 };
    let radius = st(ld(stat.blast_radius_shock_wave) * ld(stat.header.coll.coll_sphere_size) * ld(mult));
    let r = ld(radius);
    let list = u.impactable_list.clone();
    for obj in list {
        let ObjRef::Ship(oid) = obj else { continue };
        let Some(o) = u.ships.get(oid) else { continue };
        if o.flags & sof::IMPACTABLE == 0 || o.flags & sof::NOT_BUMPABLE != 0 {
            continue;
        }
        let mut d = Vector::new(
            st(ld(o.coll.coll_position.x) - ld(me.coll.coll_position.x)),
            st(ld(o.coll.coll_position.y) - ld(me.coll.coll_position.y)),
            st(ld(o.coll.coll_position.z) - ld(me.coll.coll_position.z)),
        );
        let neg = -radius;
        if !(neg <= d.x && d.x <= radius && neg <= d.y && d.y <= radius && neg <= d.z && d.z <= radius) {
            continue;
        }
        let mut dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
        if !(ld(dsq) <= r * r) || oid == id {
            continue;
        }
        let Some(ostat) = statics.ship(o.ship_type) else { continue };
        if ostat.header.immobile {
            continue;
        }
        if dsq < 1.0 {
            dsq = 1.0;
        }
        let f = st(1.0 - fsqrt(ld(dsq)) / r);
        let c = if scuttle { tw.blast_constant_scuttle } else { tw.blast_constant };
        d.normalize_to_length(st(ld(c) * ld(f)));
        let fe = ld(f);
        for axis in 0..3 {
            let rv = u.randy.frandom_between_ext(RAN_GAME, tw.explosion_rock[axis][0], tw.explosion_rock[axis][1]);
            let s = u.ships.ship_mut(oid);
            let t = match axis {
                0 => &mut s.rotinfo.torque.x,
                1 => &mut s.rotinfo.torque.y,
                _ => &mut s.rotinfo.torque.z,
            };
            *t = st(rv * fe + ld(*t));
        }
        let s = u.ships.ship_mut(oid);
        let fo = &mut s.posinfo.force;
        fo.x = st(ld(d.x) + ld(fo.x));
        fo.y = st(ld(d.y) + ld(fo.y));
        fo.z = st(ld(d.z) + ld(fo.z));
        if s.flags & sof::TARGETABLE != 0 && s.flags & 0x4000_0000 == 0 {
            let base = if stat.blast_radius_damage == 0.0 { stat.health.max_health } else { stat.blast_radius_damage };
            let pct = if scuttle { tw.blast_damage_max_percent_scuttle } else { tw.blast_damage_max_percent };
            let dmg = st(ld(pct) * fe * ld(base));
            if 0.0 < dmg {
                apply_damage_to_target(u, oid, dmg, 0, how::KILLED_BY_SHOCKWAVE, me.player as i32, Some(id));
            }
        }
    }
}

/// `pointExplosionInSpace` (0x541820), a burst bullet ending: every impactable, mobile ship within
/// `radius` of the point is pushed and rocked by how near it is, and takes `damage` if it can be
/// hurt (`SOF_Targetable`, as its own killer).
pub fn point_explosion_in_space(u: &mut Universe, at: Vector, radius: f32, damage: f32, player: i32) {
    let statics = u.statics.clone();
    let tw = u.tweaks.combat.clone();
    let r = ld(radius);
    let list = u.space_obj_list.clone();
    for obj in list {
        let ObjRef::Ship(oid) = obj else { continue };
        let Some(o) = u.ships.get(oid) else { continue };
        if o.flags & sof::IMPACTABLE == 0 || o.flags & sof::NOT_BUMPABLE != 0 {
            continue;
        }
        let mut d = Vector::new(
            st(ld(o.coll.coll_position.x) - ld(at.x)),
            st(ld(o.coll.coll_position.y) - ld(at.y)),
            st(ld(o.coll.coll_position.z) - ld(at.z)),
        );
        let neg = -radius;
        if !(neg <= d.x && d.x <= radius && neg <= d.y && d.y <= radius && neg <= d.z && d.z <= radius) {
            continue;
        }
        let mut dsq = st((ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y)) + ld(d.z) * ld(d.z));
        if !(ld(dsq) <= r * r) {
            continue;
        }
        let Some(ostat) = statics.ship(o.ship_type) else { continue };
        if ostat.header.immobile {
            continue;
        }
        if dsq < 1.0 {
            dsq = 1.0;
        }
        let f = st(1.0 - fsqrt(ld(dsq)) / r);
        d.normalize_to_length(st(ld(tw.blast_constant) * ld(f)));
        let fe = ld(f);
        for axis in 0..3 {
            let rv = u.randy.frandom_between_ext(RAN_GAME, tw.explosion_rock[axis][0], tw.explosion_rock[axis][1]);
            let s = u.ships.ship_mut(oid);
            let t = match axis {
                0 => &mut s.rotinfo.torque.x,
                1 => &mut s.rotinfo.torque.y,
                _ => &mut s.rotinfo.torque.z,
            };
            *t = st(rv * fe + ld(*t));
        }
        let m = ld(tw.burst_force_modifier);
        let s = u.ships.ship_mut(oid);
        let fo = &mut s.posinfo.force;
        fo.x = st(m * ld(d.x) + ld(fo.x));
        fo.y = st(m * ld(d.y) + ld(fo.y));
        fo.z = st(m * ld(d.z) + ld(fo.z));
        if s.flags & sof::TARGETABLE != 0 {
            apply_damage_to_target(u, oid, damage, 0, how::KILLED_BY_SHOCKWAVE, player, Some(oid));
        }
    }
}

/// `univDeleteDeadShips` / `univDeleteDeadShip`: everything that refers to the dead ships lets
/// go of them and their explosions start; then they leave the universe (with effects, they
/// stay as hulks until the explosion deletes them).
pub fn delete_dead_ships(u: &mut Universe) {
    while !u.dead_ships.is_empty() {
        let id = u.dead_ships.remove(0);
        let death_by = if u.dead_ship_death_by.is_empty() { 0 } else { u.dead_ship_death_by.remove(0) };
        delete_dead_ship(u, id, death_by);
    }
}

/// `univReallyDeleteThisShipRightNow`: remove a ship immediately (cancelled hulls), no
/// explosion.
pub fn really_delete_ship(u: &mut Universe, id: ShipId) {
    if let Some(i) = u.dead_ships.iter().position(|&d| d == id) {
        u.dead_ships.remove(i);
        if i < u.dead_ship_death_by.len() {
            u.dead_ship_death_by.remove(i);
        }
    }
    delete_dead_ship(u, id, -1);
}

fn delete_dead_ship(u: &mut Universe, id: ShipId, death_by: i32) {
    if u.ships.get(id).is_none() {
        return;
    }
    if death_by == 0 {
        crate::battle_status::ship_dying(u, id);
    }
    let player = u.ships.ship(id).player;
    let was_mothership = u.base.players.get(player).is_some_and(|p| p.mothership == Some(id));
    let fx = u.effects.is_some();
    retire_dead_ship(u, id, !fx);
    // `univKillOtherPlayersIfDead`.
    crate::multiplayer::ship_deleted(u, player, was_mothership);
    // The explosion; it deletes the hulk when it is done (`deleteParentShip`). Without an
    // explosion the ship goes now (the original would leave it in the universe).
    if fx && crate::effects::start_death_effect(u, id, death_by).is_some() {
        u.hulks.push(id);
        return;
    }
    wipe_ship(u, id);
}

/// The part of `univDeleteDeadShip` that makes everything let go of the ship: it is dead,
/// no longer in the ship or impactable lists or the blobs, and no one refers to it.
fn retire_dead_ship(u: &mut Universe, id: ShipId, fow_now: bool) {
    let Some(s) = u.ships.get_mut(id) else { return };
    s.posinfo.force = Vector::ZERO;
    s.rotinfo.torque = Vector::ZERO;
    s.flags = (s.flags & !sof::SELECTABLE) | sof::DEAD;
    let (tactics, leader) = (s.tactics, s.attack.leader);
    // `stopRepairEffect`: its harvest, salvage or repair beam stops.
    crate::resource::stop_beam_effect(u, id);
    if tactics == crate::object::tactics::EVASIVE {
        if let Some(l) = leader.and_then(|l| u.ships.get_mut(l)) {
            l.chatter_bits |= 2;
        }
    }
    // Bullets let go of their dead owner (beams then fade out) and target.
    let bl = u.bullet_list.clone();
    for bid in bl {
        let b = u.bullets.bullet_mut(bid);
        if b.owner == Some(id) {
            b.owner = None;
            b.gun = None;
            if b.weapon == crate::bullet::weapon::ION {
                b.time_lived = st(ld(b.time_lived) + 10000.0);
            }
        }
        if b.target == Some(id) {
            b.target = None;
        }
    }
    // The ship's module: its died and close callbacks; then every ship drops its references.
    crate::ships::died(u, id);
    crate::ships::close(u, id);
    let others = u.ship_list.clone();
    for o in others {
        if o == id || u.ships.get(o).is_none() {
            continue;
        }
        let s = u.ships.ship_mut(o);
        if s.recent_attacker == Some(id) {
            s.recently_attacked = 0;
            s.recent_attacker = None;
        }
        if s.firing_at_us == Some(id) {
            s.recently_fired_upon = 0;
            s.firing_at_us = None;
        }
        if s.getting_rocked == Some(id) {
            s.getting_rocked = None;
        }
        if s.queued_attack == Some(id) {
            s.queued_attack = None;
        }
        crate::ships::beast::ship_gone(u, o, id);
        crate::ships::remove_ship_refs(u, o, id);
    }
    crate::missile::remove_ship_references(u, id);
    if fow_now {
        crate::fow::remove_viewer(u, id);
    }
    // The base layer lets go (factories, docking, clamping, labs, support units...).
    crate::base::ship_died(u, id);
    u.blobs.object_died(ObjRef::Ship(id));
    u.ships.ship_mut(id).coll_my_blob = None;
    crate::command::cl_ship_died(u, id);
    u.ship_list.retain(|&s| s != id);
    u.impactable_list.retain(|&o| o != ObjRef::Ship(id));
}

/// `univWipeShipOutOfExistence`: the ship leaves the universe for good.
pub fn wipe_ship(u: &mut Universe, id: ShipId) {
    if u.ships.get(id).is_none() {
        return;
    }
    if let Some(i) = u.hulks.iter().position(|&h| h == id) {
        u.hulks.remove(i);
        crate::fow::remove_viewer(u, id);
    }
    crate::effects::ship_wiped(u, id);
    u.ship_list.retain(|&s| s != id);
    u.impactable_list.retain(|&o| o != ObjRef::Ship(id));
    u.space_obj_list.retain(|&o| o != ObjRef::Ship(id));
    u.render_list.retain(|&o| o != ObjRef::Ship(id));
    u.ships.remove(id);
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Arc;

    use super::*;
    use crate::object::{Bullet, BulletId, PosInfo};
    use crate::statics::{ShipRace, ShipStaticInfo, StaticWorld, Tweaks};
    use crate::tunables::SimTweaks;
    use hwc_math::Matrix;

    struct HitCase {
        force: [u32; 3],
        torque: [u32; 3],
        health: u32,
        armour: u32,
        class: usize,
        sides: [u32; 6],
        owner_type: usize,
        friendly: bool,
        vel: [u32; 3],
        btype: u32,
        mass: u32,
        damage: u32,
        rock: [u32; 3],
        side: i32,
        exp_force: [u32; 3],
        exp_torque: [u32; 3],
        exp_health: u32,
        exp_chatter: u8,
    }

    /// Inputs and results of emulating `univBulletCollidedWithTarget` (0x541ac0) with
    /// `ApplyDamageToTarget` in cataclysm.exe (Ghidra p-code emulation; speech, alliance,
    /// crystal and docking helpers stubbed out, rock ranges collapsed so the random numbers do not
    /// matter).
    const HIT_CASES: &[HitCase] = &[
    HitCase { force: [0xc523c5a9, 0x43dd2569, 0xc4a28e59], torque: [0x42cfd70f, 0x42fb70cc, 0xc3d93c4e], health: 0x459de560, armour: 1, class: 6, sides: [0x3fa9c02b, 0x3f49a136, 0x3fc9b143, 0x3fa7ddf3, 0x3fa98779, 0x3f8c3ff5], owner_type: 0, friendly: false, vel: [0xc48609fc, 0xc4ae3056, 0x44d4eaf1], btype: 12, mass: 0x40c1e684, damage: 0x400b6d04, rock: [0xc2ae632e, 0x424e958e, 0x4191c264], side: 1, exp_force: [0xc7d02a9f, 0xc80380b7, 0x481fffbc], exp_torque: [0xc3d435d6, 0x43db54f8, 0xc3a208e7], exp_health: 0x459b6038, exp_chatter: 0 },
    HitCase { force: [0x452f0c3d, 0x454a3b48, 0xc5101aef], torque: [0x42bd7fc1, 0x43d2140c, 0xc2e0c867], health: 0x45fec37c, armour: 1, class: 6, sides: [0x3fcbccd9, 0x3faec444, 0x3ff91b23, 0x3f338365, 0x3f862e06, 0x3f0dc7cd], owner_type: 67, friendly: false, vel: [0xc48d81ac, 0x44e8bd75, 0xc37f5a40], btype: 9, mass: 0x406a26bb, damage: 0x40255b39, rock: [0x416cdf6d, 0x40da427f, 0xc193e0ed], side: 0, exp_force: [0xc777eb1c, 0x47db323d, 0xc686caee], exp_torque: [0x4314e9e4, 0x43de8e29, 0xc3340530], exp_health: 0x45f70ce5, exp_chatter: 0 },
    HitCase { force: [0x457ca049, 0x44e37a49, 0x45860ba6], torque: [0x43b23346, 0x43f57ead, 0x432b4607], health: 0x45b0a330, armour: 1, class: 4, sides: [0x3fc90d89, 0x3f51126e, 0x3fdfab31, 0x3fae1e43, 0x3f6d6c75, 0x3f185e6e], owner_type: 81, friendly: false, vel: [0x44f4e72b, 0xc4cdbdaf, 0x44964c33], btype: 13, mass: 0x40e74d97, damage: 0x3f0ce157, rock: [0xc1692578, 0xc187af2f, 0xc2986ee0], side: 3, exp_force: [0x48613953, 0xc8381d8f, 0x480bfcad], exp_torque: [0x437b12cf, 0x43b83293, 0xc3bdd17b], exp_health: 0x45b08b3c, exp_chatter: 0 },
    HitCase { force: [0x448f2a69, 0xc58e34c8, 0x4508867a], torque: [0xc3290bbd, 0x43be73e1, 0x43f05160], health: 0x45db6d74, armour: 0, class: 6, sides: [0x3f02c702, 0x3f29848f, 0x3fa6d4bd, 0x3ff6301c, 0x3ffa83a9, 0x3f6ffaaa], owner_type: 67, friendly: true, vel: [0xc4abe687, 0xc4e4c837, 0x44b7e3b7], btype: 10, mass: 0x404ce4f5, damage: 0x3f588757, rock: [0x4290aed5, 0xc1c532b4, 0x4293f4e4], side: 2, exp_force: [0xc78758c7, 0xc7bfff76, 0x47977221], exp_torque: [0x427a36cb, 0x4396feb0, 0x44335e6d], exp_health: 0x45dadbef, exp_chatter: 1 },
    HitCase { force: [0x44b3dc64, 0x446f2027, 0x44142714], torque: [0x42f04095, 0x43dc4f85, 0x40e0dbad], health: 0x45d22621, armour: 2, class: 4, sides: [0x3ff3cbb3, 0x3f93fd55, 0x3f632953, 0x3f745970, 0x3f811202, 0x3fd75f43], owner_type: 81, friendly: false, vel: [0xc4391fa5, 0xc3f6f785, 0x43b26803], btype: 4, mass: 0x3f735ac6, damage: 0x40045fe4, rock: [0xc0d7ff4c, 0x420f6cd5, 0xc1ebe078], side: 4, exp_force: [0xc6197f22, 0xc5cce080, 0x45bc1ce0], exp_torque: [0x42e36b56, 0x43ed5a6a, 0xc1a802ca], exp_health: 0x45d03193, exp_chatter: 0 },
    HitCase { force: [0xc50a2c18, 0xc2d1f14f, 0x445f7141], torque: [0x43e36ba7, 0xc3ef5fac, 0xc302d5ce], health: 0x45eabf05, armour: 1, class: 6, sides: [0x3f442132, 0x3f47226d, 0x3fd18e7b, 0x3fe20478, 0x3f656dbf, 0x3fd72977], owner_type: 0, friendly: false, vel: [0x448822ff, 0xc4ec8a16, 0x438a8419], btype: 4, mass: 0x4034ceda, damage: 0x3f87364d, rock: [0x42730bd0, 0xc2510b3a, 0xc27a15a4], side: 2, exp_force: [0x4737aa76, 0xc7a744b4, 0x4651a0b0], exp_torque: [0x441ca000, 0xc41c9909, 0xc399bb8c], exp_health: 0x45e8b148, exp_chatter: 0 },
    HitCase { force: [0x44baec7a, 0xc57bdc6b, 0x447bcf72], torque: [0x43e09315, 0x432eed6a, 0xc389c513], health: 0x4600bc3d, armour: 0, class: 4, sides: [0x3f80a5fb, 0x3fbcd839, 0x3fe9e685, 0x3f969c93, 0x3f566923, 0x3f2e6eda], owner_type: 81, friendly: false, vel: [0x44e4ca5f, 0xc438dbee, 0x44a81a3e], btype: 5, mass: 0x40df4b6d, damage: 0x3facd5c9, rock: [0x42832db7, 0xc2a5c951, 0x41beee67], side: 3, exp_force: [0x4849058a, 0xc7a91cee, 0x48139c32], exp_torque: [0x4462b503, 0xc3c9bfb7, 0xc2da0072], exp_health: 0x46007cb0, exp_chatter: 0 },
    HitCase { force: [0xc56771bd, 0xc5020923, 0x4537a9e8], torque: [0xc364d354, 0xc319a54e, 0xc2a63049], health: 0x45d0b8ae, armour: 1, class: 4, sides: [0x3ff0c1f1, 0x3f3be737, 0x3f01ca46, 0x3ff51b80, 0x3fe8f4b1, 0x3ffd7cc8], owner_type: 67, friendly: true, vel: [0x436b7c6a, 0x44f2e131, 0x4459423b], btype: 1, mass: 0x40c2ecef, damage: 0x4025df2e, rock: [0x420263c7, 0x40736447, 0xc228c43b], side: 1, exp_force: [0x46966003, 0x4836e74c, 0x47ab2a94], exp_torque: [0xc1f215f8, 0xc3027ae6, 0xc3aa0cf9], exp_health: 0x45d04e26, exp_chatter: 1 },
    HitCase { force: [0x457390a8, 0x4561f4fb, 0x455fbcfa], torque: [0x43eb7b69, 0xc3be0078, 0xc37f7750], health: 0x45a0a4a0, armour: 2, class: 4, sides: [0x3fec2412, 0x3fecbcd1, 0x3faec66a, 0x3f050c28, 0x3fcf18e7, 0x3f41fac0], owner_type: 67, friendly: false, vel: [0xc4827a16, 0xc4f013d5, 0x4313fc44], btype: 1, mass: 0x40f15ec5, damage: 0x4001f23c, rock: [0xc1fdd5f4, 0xc2460527, 0x4290aa78], side: 2, exp_force: [0xc7ee6df3, 0xc85ed3b8, 0x46a77ee4], exp_torque: [0x4367a26f, 0xc43c5a50, 0x4391106a], exp_health: 0x459fdd03, exp_chatter: 0 },
    HitCase { force: [0x4542b83a, 0xc589128d, 0xc52e7f7e], torque: [0xc3bce3b5, 0x43c1d19d, 0xc3be650e], health: 0x45ba2e9c, armour: 1, class: 4, sides: [0x3fde1c9c, 0x3f02e1be, 0x3fb8b14d, 0x3fe59c48, 0x3f132c80, 0x3f68376a], owner_type: 67, friendly: false, vel: [0x43ec2d9e, 0x429d604e, 0xc4e08379], btype: 10, mass: 0x40ca5c04, damage: 0x3f012854, rock: [0xc2b2110a, 0xc2954133, 0xc2962646], side: -1, exp_force: [0x4746dc67, 0x455f7520, 0xc834326e], exp_torque: [0xc46b334d, 0xc2a8924c, 0xc455e2b2], exp_health: 0x45b9dde3, exp_chatter: 0 },
    HitCase { force: [0xc32bded5, 0xc591ef1d, 0x4504f315], torque: [0x41767908, 0xc12022a6, 0xc3ab7aab], health: 0x45a538ce, armour: 1, class: 6, sides: [0x3fb0a135, 0x3f8547be, 0x3f496020, 0x3f7e4007, 0x3f2f859d, 0x3faaa936], owner_type: 81, friendly: false, vel: [0x445f0a1a, 0x4493a578, 0x438577e3], btype: 1, mass: 0x40532bfb, damage: 0x4000b5a7, rock: [0x426218f4, 0xc1bf9396, 0x4270edc1], side: 3, exp_force: [0x47374fc6, 0x4761579b, 0x467d6e41], exp_torque: [0x4349e8f9, 0xc2b20be3, 0xc31037fc], exp_health: 0x45a3e940, exp_chatter: 0 },
    HitCase { force: [0x457195eb, 0xc58baad2, 0x449f4e2c], torque: [0x43820595, 0xc3390d28, 0x43e124ff], health: 0x45d0895f, armour: 0, class: 4, sides: [0x3f53f613, 0x3f67b701, 0x3fb142de, 0x3fda33cf, 0x3f562f72, 0x3f320f7d], owner_type: 0, friendly: true, vel: [0x44da3df6, 0xc3fb8753, 0x44c6ed56], btype: 8, mass: 0x3fb497ca, damage: 0x400e7d26, rock: [0x42af9ff2, 0x423938d4, 0x428beaad], side: 3, exp_force: [0x47290e57, 0xc67745b3, 0x47114f44], exp_torque: [0x43bff7e1, 0xc2ef7088, 0x44093f7d], exp_health: 0x45b8d0d5, exp_chatter: 1 },
    ];

    fn world(class: usize, sides: [f32; 6], owner_type: ShipType) -> StaticWorld {
        let mut ships: Vec<Option<ShipStaticInfo>> = vec![None; ShipType::COUNT];
        let mut t = ShipStaticInfo::new(ShipType::sMultiBeamFrigate, String::new());
        t.ship_race = ShipRace::Somtaaw;
        t.health.max_health = 9000.0;
        t.coll_side_modifiers = sides;
        t.ship_class = Some(ShipClass::ALL[class]);
        ships[ShipType::sMultiBeamFrigate.index()] = Some(t);
        let mut o = ShipStaticInfo::new(owner_type, String::new());
        o.ship_race = ShipRace::Taiidan;
        o.health.max_health = 100.0;
        ships[owner_type.index()] = Some(o);
        StaticWorld {
            options: Default::default(),
            tweaks: Tweaks::default(),
            ships,
            missiles: Vec::new(),
            mines: Vec::new(),
            mine_max_diameter: 0.0,
            asteroids: Vec::new(),
            dust_clouds: Vec::new(),
            nebulae: Vec::new(),
            derelicts: Vec::new(),
            research: Default::default(),
            unit_caps: Default::default(),
            unit_cap_presets: BTreeMap::new(),
            team_colors: Default::default(),
            battle_chatter: Default::default(),
            default_lod: None,
            default_mex: None,
            warnings: Vec::new(),
        }
    }

    #[test]
    fn bullet_hit_matches_emulated_binary() {
        let f = f32::from_bits;
        let v = |a: [u32; 3]| Vector::new(f(a[0]), f(a[1]), f(a[2]));
        let bits = |v: Vector| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        for (i, c) in HIT_CASES.iter().enumerate() {
            let owner_type = ShipType::ALL[c.owner_type];
            let statics = Arc::new(world(c.class, c.sides.map(f), owner_type));
            let mut tw = SimTweaks::compiled();
            for k in 0..3 {
                tw.combat.bullet_rock[k] = [f(c.rock[k]); 2];
            }
            let mut u = Universe::new(statics, tw, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
            let tid = u.add_ship(ShipType::sMultiBeamFrigate, 1, Vector::ZERO, None).unwrap();
            let oid = u.add_ship(owner_type, if c.friendly { 1 } else { 0 }, Vector::new(500.0, 0.0, 0.0), None).unwrap();
            {
                let s = u.ships.ship_mut(tid);
                s.posinfo.force = v(c.force);
                s.rotinfo.torque = v(c.torque);
                s.health = f(c.health);
                s.armour_level = c.armour;
            }
            let bid = u.bullets.insert(Bullet {
                id: BulletId(0),
                flags: 0,
                posinfo: PosInfo { velocity: v(c.vel), ..Default::default() },
                coordsys: Matrix::default(),
                coll_optimize_dist: 0.0,
                coll_my_blob: None,
                heading: Vector::new(1.0, 0.0, 0.0),
                length_vec: Vector::ZERO,
                length_mag: 0.0,
                travel_dist: 0.0,
                beam_travel_dist: 0.0,
                weapon: 0,
                burst: false,
                bullet_type: c.btype,
                time_lived: 0.0,
                total_lifetime: 1.0,
                owner: Some(oid),
                gun: None,
                target: None,
                color: 0,
                mass: f(c.mass),
                damage: f(c.damage),
                updated_once: false,
                player: 0,
                max_range: 0.0,
                dist_flown: 0.0,
                prev_position: Vector::ZERO,
                dfg_flags: 0,
            });
            bullet_collided_with_target(&mut u, true, tid, bid, 0.0, c.side);
            let s = u.ships.ship(tid);
            assert_eq!(bits(s.posinfo.force), c.exp_force, "case {i} force");
            assert_eq!(bits(s.rotinfo.torque), c.exp_torque, "case {i} torque");
            assert_eq!(s.health.to_bits(), c.exp_health, "case {i} health");
            assert_eq!(s.chatter_bits, c.exp_chatter, "case {i} chatter");
            assert_eq!(s.recent_attacker, Some(oid));
            assert_eq!(s.recently_attacked, 80);
        }
    }
}
