//! `Gun.c` as compiled into Cataclysm: fire-rate checks (with Cataclysm's damage slow-down and
//! the second fire mode of upgraded guns), turret aiming, the fixed-gun aim check, and firing
//! guns at one or several targets.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, ld, lt_or_unordered, st, Ext};
use hwc_math::random::RAN_GAME;
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::aiship;
use crate::combat_statics::is_acolyte;
use crate::command::attr;
use crate::object::{sof, Gun, ShipId};
use crate::statics::{GunStatic, GunType, ShipClass, ShipStaticInfo};
use crate::universe::Universe;

/// `NIS_TriggerHappyAngle` (cos 45 degrees) used for ships in a cinematic.
const NIS_TRIGGER_HAPPY: f32 = f32::from_bits(0x3f35_04f3);

/// `GUNBURST_STATE_*`.
pub mod burst {
    pub const READY: u32 = 0;
    pub const BURST: u32 = 1;
    pub const COOLOFF: u32 = 2;
}

/// The guns a new ship starts with (`univCreateShip` and `suShipInit`): barrels along the
/// muzzle normal (turrets at their default orientation), launchers full, the weapon
/// technology the gun starts at.
pub fn init_guns(stat: &ShipStaticInfo) -> Vec<Gun> {
    let Some(statics) = stat.guns.as_ref() else { return Vec::new() };
    statics
        .iter()
        .map(|gs| {
            let mut g = Gun {
                level: gs.start_upgrade_level.index() as u32,
                heading: Vector::ZERO,
                cur_orientation: Matrix::IDENTITY,
                cur_orientation_non_concat: Matrix::IDENTITY,
                angle: 0.0,
                declination: 0.0,
                angle_speed: 0.0,
                declination_speed: 0.0,
                num_missiles: 0,
                last_time_fired: 0.0,
                burst_start_time: 0.0,
                use_burst_fire_time: 0.0,
                use_burst_wait_time: 0.0,
                burst_state: 0,
            };
            match gs.gun_type {
                Some(GunType::NewGimble) => {
                    g.cur_orientation = gs.default_gun_orientation;
                    if gs.flags & 1 != 0 {
                        g.cur_orientation_non_concat = gs.default_gun_orientation_non_concat;
                    }
                    g.heading = g.cur_orientation.col(2);
                }
                Some(GunType::MissileLauncher) | Some(GunType::MineLauncher) => {
                    g.num_missiles = gs.max_missiles;
                    g.heading = gs.gun_normal;
                }
                Some(GunType::Fixed) | Some(GunType::Gimble) => g.heading = gs.gun_normal,
                None => {}
            }
            g
        })
        .collect()
}

/// The gun has been upgraded to its end technology (and that differs from the start).
pub fn upgraded(gun: &Gun, gs: &GunStatic) -> bool {
    gun.level == gs.end_upgrade_level.index() as u32 && gs.start_upgrade_level != gs.end_upgrade_level
}

/// The gun static's bullet type for its start (`+4`) and end (`+8`) technology.
pub fn static_bullet_type(gs: &GunStatic, end: bool) -> u32 {
    let w = if end { gs.end_upgrade_level } else { gs.start_upgrade_level };
    crate::combat_statics::bullet_type(w.index() as u32, gs.gun_size.index() as u32)
}

/// Bullet type the gun fires now (0x486ee0).
pub fn bullet_type(gun: &Gun, gs: &GunStatic) -> u32 {
    static_bullet_type(gs, upgraded(gun, gs))
}

/// Bullet speed of the gun (0x486f00): upgraded guns use `BulletSpeed2` unless it is unset
/// while the first speed is positive (the original prints a warning then).
pub fn bullet_speed(gun: &Gun, gs: &GunStatic) -> f32 {
    if upgraded(gun, gs) {
        if ld(gs.bullet_speed2) > 0.0 || !(ld(gs.bullet_speed) > 0.0) {
            gs.bullet_speed2
        } else {
            gs.bullet_speed
        }
    } else {
        gs.bullet_speed
    }
}

fn gun_static(stat: &ShipStaticInfo, i: usize) -> Option<&GunStatic> {
    stat.guns.as_ref().and_then(|g| g.get(i))
}

/// The fire-time multiplier of a damaged ship, applied to a register value.
fn damage_slowdown(health: f32, max_health: f32, base: Ext) -> Ext {
    let ratio_ext = ld(health) / ld(max_health);
    let ratio = st(ratio_ext);
    if ratio_ext < ld(f32::from_bits(0x3dcc_cccd)) {
        base * 4.0
    } else if ld(ratio) < 0.25 {
        base * 2.5
    } else if ld(ratio) < 0.5 {
        base * 1.75
    } else if ld(ratio) < 0.75 {
        base * 1.25
    } else {
        base
    }
}

/// `gunCanShoot` (0x485920).
pub fn can_shoot(u: &mut Universe, id: ShipId, gi: usize) -> bool {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let cmd = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).map(|c| (c.attributes, c.formation.formation_type));
    let tw_lo = u.tweaks.combat.passive_retaliation_fire_time_modifier_lo;
    let tw_hi = u.tweaks.combat.passive_retaliation_fire_time_modifier_hi;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(gs) = gun_static(stat, gi) else { return false };
    let max_health = stat.health.max_health;
    let health = s.health;
    let gun = &mut s.guns[gi];
    if eq_or_unordered(ld(gs.burst_fire_time), 0.0) {
        let mut fire_time = ld(gs.fire_time);
        let bt = static_bullet_type(gs, false);
        if bt != 0x13 && bt != 0x12 {
            fire_time = damage_slowdown(health, max_health, fire_time);
        }
        return (ld(now) - ld(gun.last_time_fired)) > fire_time;
    }
    // Burst-firing guns: burst, cool off, ready again.
    let mut restart = false;
    match gun.burst_state {
        burst::READY => restart = true,
        burst::BURST => {
            s.burst_firing = true;
            if ld(now) - ld(gun.burst_start_time) > ld(gun.use_burst_fire_time) {
                gun.burst_state = burst::COOLOFF;
                gun.burst_start_time = now;
                s.burst_firing = false;
                if ld(now) - ld(gun.burst_start_time) > ld(gun.use_burst_wait_time) {
                    gun.burst_state = burst::READY;
                    restart = true;
                } else {
                    return false;
                }
            }
        }
        burst::COOLOFF => {
            s.burst_firing = false;
            if ld(now) - ld(gun.burst_start_time) > ld(gun.use_burst_wait_time) {
                gun.burst_state = burst::READY;
                restart = true;
            } else {
                return false;
            }
        }
        _ => {}
    }
    if restart {
        gun.burst_start_time = now;
        gun.burst_state = burst::BURST;
        gun.use_burst_fire_time = gs.burst_fire_time;
        gun.use_burst_wait_time = gs.burst_wait_time;
        let w = damage_slowdown(health, max_health, ld(gs.burst_wait_time));
        gun.use_burst_wait_time = st(w);
    }
    let passive = cmd.is_some_and(|(a, ft)| {
        a & attr::PASSIVE_ATTACKING != 0
            && !stat.passive_attack_penalty_exempt
            && !(a & attr::PROTECTING != 0 && a & attr::FORMATION != 0 && ft == crate::formation::kind::SPHERE)
    });
    let since = ld(now) - ld(gun.last_time_fired);
    if !passive {
        return ld(gs.fire_time) < since;
    }
    let base = st(ld(tw_lo) + ld(gs.fire_time));
    let since_f = st(since);
    if !(since > ld(base)) {
        return false;
    }
    let r = u.randy.frandom_between_ext(RAN_GAME, tw_lo, tw_hi);
    r + ld(base) < ld(since_f)
}

/// Ready-to-fire check (0x4857f0): upgraded guns use the second fire time, shared between
/// the ship's upgraded guns; other guns use [`can_shoot`].
pub fn ready(u: &mut Universe, id: ShipId, gi: usize) -> bool {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(gs) = gun_static(stat, gi) else { return false };
    let gun = &s.guns[gi];
    if !upgraded(gun, gs) {
        return can_shoot(u, id, gi);
    }
    let since = ld(now) - ld(gun.last_time_fired);
    if ld(gs.fire_time2) > 0.0 {
        if !(ld(gs.fire_time2) < since) {
            return false;
        }
        // The latest shot of all upgraded guns, and how many there are.
        let mut latest = gun.last_time_fired;
        let mut n = 0;
        for (k, g) in s.guns.iter().enumerate() {
            if let Some(ks) = gun_static(stat, k) {
                if upgraded(g, ks) {
                    n += 1;
                    if ld(latest) < ld(g.last_time_fired) {
                        latest = g.last_time_fired;
                    }
                }
            }
        }
        if n > 1 {
            return !(ld(now) - ld(gs.fire_time2) / f64::from(n as f32) <= ld(latest));
        }
        true
    } else {
        // (The original prints "Hobbes call Mike: upgraded weapon" here.)
        ld(gs.fire_time) < since
    }
}

/// `gunOrientGimbleGun` (0x485c10): aim the gun at `target`; true when it points close
/// enough to fire. Gimbal guns only check the muzzle cone; visible turrets turn towards the
/// target at their tracking rates.
pub fn orient_gimble(u: &mut Universe, id: ShipId, gi: usize, target: ShipId) -> bool {
    let statics = u.statics.clone();
    let dt = u.phys_time_elapsed;
    let below = u.tweaks.combat.dont_use_velocity_pred_if_below;
    let Some(t) = crate::target::get(u, target) else { return false };
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(gs) = gun_static(stat, gi) else { return false };
    let (trigger_happy, cos_max) = if s.flags & sof::NIS_SHIP != 0 {
        (NIS_TRIGGER_HAPPY, NIS_TRIGGER_HAPPY)
    } else {
        (gs.trigger_happy, gs.cos_max_angle_from_norm)
    };
    let max_angle_speed = gs.max_angle_speed;
    let speed = bullet_speed(&s.guns[gi], gs);
    let coordsys = s.rotinfo.coordsys;
    match gs.gun_type {
        Some(GunType::Gimble) => {
            let traj = aiship::trajectory_with_vel_prediction(s, &t, speed, below);
            let mut local = coordsys.vec_mul(traj);
            local = Vector::new(local.x - gs.position.x, local.y - gs.position.y, local.z - gs.position.z);
            local.normalize();
            let n = gs.gun_normal;
            let mut dot = (ld(local.z) * ld(n.z) + ld(local.y) * ld(n.y)) + ld(local.x) * ld(n.x);
            if dot > 1.0 {
                dot = 1.0;
            }
            if lt_or_unordered(dot, ld(cos_max)) {
                return false;
            }
            if dot <= ld(gs.cos_min_angle_from_norm) {
                u.ships.ship_mut(id).guns[gi].heading = local;
                return true;
            }
            false
        }
        Some(GunType::NewGimble) => {
            let slave = gs.slave_driver;
            if slave >= 0 {
                let src = s.guns.get(slave as usize).copied();
                if let Some(src) = src {
                    let g = &mut u.ships.ship_mut(id).guns[gi];
                    g.heading = src.heading;
                    g.cur_orientation = src.cur_orientation;
                    g.declination = src.declination;
                    g.angle = src.angle;
                }
            }
            let s = u.ships.ship(id);
            let traj = aiship::trajectory_with_vel_prediction(s, &t, speed, below);
            let mut local = coordsys.vec_mul(traj);
            local = Vector::new(local.x - gs.position.x, local.y - gs.position.y, local.z - gs.position.z);
            local.normalize();
            let g = s.guns[gi].cur_orientation.vec_mul(local);
            let gun = &mut u.ships.ship_mut(id).guns[gi];
            gun.angle_speed = 0.0;
            gun.declination_speed = 0.0;
            let beam = matches!(gs.start_upgrade_level.index(), 2 | 4);
            if !lt_or_unordered(ld(g.z), ld(trigger_happy)) && !beam {
                gun.heading = local;
                return true;
            }
            if slave < 0 {
                let mut changed = false;
                let clamp = |v: Ext| -> Ext {
                    if v > ld(max_angle_speed) {
                        ld(max_angle_speed)
                    } else {
                        let neg = st(-ld(max_angle_speed));
                        if v < ld(neg) {
                            ld(neg)
                        } else {
                            v
                        }
                    }
                };
                if !eq_or_unordered(ld(g.y), 0.0) {
                    let v = clamp(-(ld(g.y) * ld(gs.angle_tracking)));
                    let tryv = ld(dt) * v + ld(gun.angle);
                    if tryv < ld(gs.max_turn_angle) && tryv > ld(gs.min_turn_angle) {
                        gun.angle = st(tryv);
                        gun.angle_speed = st(v);
                        changed = true;
                    }
                }
                if !eq_or_unordered(ld(g.x), 0.0) {
                    let v = clamp(ld(g.x) * ld(gs.declination_tracking));
                    let tryv = ld(dt) * v + ld(gun.declination);
                    if tryv < ld(gs.max_declination) && tryv > ld(gs.min_declination) {
                        gun.declination = st(tryv);
                        gun.declination_speed = st(v);
                        changed = true;
                    }
                }
                if changed {
                    let roty = Matrix::rot_about_y(st(fcos(ld(gun.declination))), st(fsin(ld(gun.declination))));
                    let rotx = Matrix::rot_about_x(st(fcos(ld(gun.angle))), st(fsin(ld(gun.angle))));
                    let tmp = gs.default_gun_orientation.mul_mat(&rotx);
                    gun.cur_orientation = tmp.mul_mat(&roty);
                    if gs.flags & 1 != 0 {
                        let tmp = gs.default_gun_orientation_non_concat.mul_mat(&roty);
                        gun.cur_orientation_non_concat = tmp.mul_mat(&rotx);
                    }
                    gun.heading = gun.cur_orientation.col(2);
                }
            }
            if !lt_or_unordered(ld(g.z), ld(trigger_happy)) {
                gun.heading = local;
                return true;
            }
            false
        }
        _ => false,
    }
}

/// Fixed-gun aim check (0x488360): would a bullet along the barrel (`use_gun_heading`) or the
/// ship's heading pass within 3/4 of the target's radius of the predicted aim point?
pub fn fixed_can_hit(u: &Universe, id: ShipId, gi: usize, target: ShipId, use_gun_heading: bool) -> bool {
    let statics = &u.statics;
    let below = u.tweaks.combat.dont_use_velocity_pred_if_below;
    let (Some(s), Some(t)) = (u.ships.get(id), crate::target::get(u, target)) else { return false };
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(gs) = gun_static(stat, gi) else { return false };
    let speed = bullet_speed(&s.guns[gi], gs);
    let traj = aiship::trajectory_with_vel_prediction(s, &t, speed, below);
    let gp = s.rotinfo.coordsys.mul_vec(gs.position);
    let d = Vector::new(traj.x - gp.x, traj.y - gp.y, traj.z - gp.z);
    let dir_local = if use_gun_heading { s.guns[gi].heading } else { s.heading() };
    let dir = s.rotinfo.coordsys.mul_vec(dir_local);
    let mag = st(fsqrt((ld(d.z) * ld(d.z) + ld(d.y) * ld(d.y)) + ld(d.x) * ld(d.x)));
    let inv = 1.0 / ld(mag);
    let nx = st(inv * ld(d.x));
    let ny = st(inv * ld(d.y));
    let mut dot = ((inv * ld(d.z)) * ld(dir.z) + ld(dir.y) * ld(ny)) + ld(dir.x) * ld(nx);
    if !(dot > 0.0) {
        return false;
    }
    if dot > 1.0 {
        dot = 1.0;
    }
    let perp = fsqrt(1.0 - dot * dot) * ld(mag);
    perp < ld(t.coll_header.coll_sphere_size) * ld(0.75f32)
}

/// `gunShoot` (0x486bd0): line-of-sight check for capital ships, then a bullet with the ship's
/// damage. Returns whether a bullet was created.
pub fn shoot(u: &mut Universe, id: ShipId, gi: usize, target: Option<ShipId>) -> bool {
    let statics = u.statics.clone();
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(gs) = gun_static(stat, gi) else { return false };
    if let Some(tid) = target {
        if !crate::target::exists(u, tid) {
            return false;
        }
        let class = stat.ship_class.map_or(0, |c| c.index());
        if class < ShipClass::Corvette.index() && static_bullet_type(gs, false) != 0x12 {
            let cs = s.rotinfo.coordsys;
            let dir = cs.mul_vec(s.guns[gi].heading);
            let gp = cs.mul_vec(gs.position);
            let pos = Vector::new(gp.x + s.posinfo.position.x, gp.y + s.posinfo.position.y, gp.z + s.posinfo.position.z);
            if !crate::collision::line_of_sight(u, id, Some(tid), pos, dir) {
                return false;
            }
        }
    }
    {
        let s = u.ships.ship_mut(id);
        if !s.ship_is_attacking {
            s.ship_is_attacking = true;
            s.ship_is_attacking2 = true;
        }
        if let Some(w) = s.attack.wingman {
            if let Some(ws) = u.ships.get_mut(w) {
                if !ws.ship_is_attacking {
                    ws.ship_is_attacking = true;
                    u.ships.ship_mut(id).ship_is_attacking2 = true;
                }
            }
        }
    }
    let s = u.ships.ship(id);
    if s.special_flags & crate::object::special::CANT_FIRE != 0 {
        return false;
    }
    let now = u.total_time_elapsed;
    u.ships.ship_mut(id).guns[gi].last_time_fired = now;
    let s = u.ships.ship(id);
    let factor = crate::damage::damage_multiplier(u, s);
    let mut damage = st(factor * ld(gs.damage_mult));
    if s.special_flags & crate::object::special::KAS_DAMAGE_MODIFIER != 0 {
        damage = st(ld(damage) * ld(s.damage_modifier));
    }
    // Shooting a Leech also costs it 10 points (the latch damage).
    if let Some(tid) = target {
        if u.ships.get(tid).is_some_and(|t| t.ship_type == crate::statics::ShipType::sLeech) {
            let player = u.ships.ship(id).player as i32;
            crate::damage::apply_damage_to_target(u, tid, 10.0, static_bullet_type(gs, false) as i32, crate::damage::how::KILLED_BY_PLAYER, player, Some(id));
        }
    }
    crate::bullet::create(u, id, gi, None, target, damage).is_some()
}

/// `gunShootGunsAtTarget` (0x487900): fire every ready gun that is in range and aims at the
/// target. `trajectory` (normalised) is used by launchers.
pub fn shoot_guns_at_target(u: &mut Universe, id: ShipId, target: ShipId, range: f32, trajectory: Vector) -> bool {
    let statics = u.statics.clone();
    let Some(s) = u.ships.get(id) else { return false };
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(guns) = stat.guns.as_ref() else { return false };
    let mut shot = false;
    for (gi, gs) in guns.iter().enumerate() {
        let s = u.ships.ship(id);
        let trigger = if s.flags & sof::NIS_SHIP != 0 { NIS_TRIGGER_HAPPY } else { gs.trigger_happy };
        let start_type = static_bullet_type(gs, false);
        if !(ld(range) < ld(gs.bullet_range)) || start_type == 0x13 || start_type == 0x12 {
            continue;
        }
        if !ready(u, id, gi) {
            continue;
        }
        match gs.gun_type {
            Some(GunType::Fixed) if fixed_can_hit(u, id, gi, target, true) => {
                shoot(u, id, gi, Some(target));
                shot = true;
            }
            Some(GunType::Gimble) | Some(GunType::NewGimble) if orient_gimble(u, id, gi, target) => {
                shoot(u, id, gi, Some(target));
                shot = true;
            }
            Some(GunType::MissileLauncher) | Some(GunType::MineLauncher) => {
                let s = u.ships.ship(id);
                let h = s.heading();
                let dot = (ld(h.x) * ld(trajectory.x) + ld(h.y) * ld(trajectory.y)) + ld(h.z) * ld(trajectory.z);
                let acolyte = gs.gun_type == Some(GunType::MissileLauncher) && is_acolyte(s.ship_type);
                if s.guns[gi].num_missiles > 0 && ld(trigger) <= dot && !acolyte {
                    shoot_missile(u, id, gi, Some(target));
                    shot = true;
                }
            }
            Some(GunType::Fixed | GunType::Gimble | GunType::NewGimble) => {}
            None => {}
        }
    }
    shot
}

/// `gunShootGunsAtMultipleTargets` (0x487cb0): each gun fires at its own target.
pub fn shoot_guns_at_multiple_targets(u: &mut Universe, id: ShipId) -> bool {
    let statics = u.statics.clone();
    let Some(s) = u.ships.get(id) else { return false };
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    let Some(guns) = stat.guns.as_ref() else { return false };
    let targets = s.attack.multiple_targets.clone().unwrap_or_default();
    let mut shot = false;
    for (gi, gs) in guns.iter().enumerate() {
        let start_type = static_bullet_type(gs, false);
        if start_type == 0x13 || start_type == 0x12 {
            continue;
        }
        if !ready(u, id, gi) {
            // A slaved barrel keeps following its driver.
            if gs.slave_driver >= 0 {
                let s = u.ships.ship_mut(id);
                if let Some(src) = s.guns.get(gs.slave_driver as usize).copied() {
                    let g = &mut s.guns[gi];
                    g.heading = src.heading;
                    g.cur_orientation = src.cur_orientation;
                    g.declination = src.declination;
                    g.angle = src.angle;
                }
            }
            continue;
        }
        let Some(Some(target)) = targets.get(gi).copied() else { continue };
        if !crate::target::exists(u, target) {
            continue;
        }
        match gs.gun_type {
            Some(GunType::Fixed) if fixed_can_hit(u, id, gi, target, true) => {
                shoot(u, id, gi, Some(target));
                shot = true;
            }
            Some(GunType::Gimble) | Some(GunType::NewGimble) if orient_gimble(u, id, gi, target) => {
                shoot(u, id, gi, Some(target));
                shot = true;
            }
            _ => {}
        }
    }
    shot
}

/// `DefaultShipFire` (0x5aad80) and `GenericCapitalFire`: every ready gun fires at the target.
pub fn default_fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let statics = u.statics.clone();
    let Some(s) = u.ships.get(id) else { return };
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let Some(guns) = stat.guns.as_ref() else { return };
    for (gi, gs) in guns.iter().enumerate() {
        if !ready(u, id, gi) {
            continue;
        }
        match gs.gun_type {
            Some(GunType::Fixed) | Some(GunType::Gimble) | Some(GunType::NewGimble) => {
                shoot(u, id, gi, Some(target));
            }
            Some(GunType::MissileLauncher) | Some(GunType::MineLauncher) if u.ships.ship(id).guns[gi].num_missiles > 0 => {
                shoot_missile(u, id, gi, Some(target));
            }
            _ => {}
        }
    }
}

/// `missileShoot` for a launcher (a wall-laying minelayer drops its mine into the wall).
pub fn shoot_missile(u: &mut Universe, id: ShipId, gi: usize, target: Option<ShipId>) -> bool {
    let forced = crate::ships::minelayer::forced_drop(u, id);
    crate::missile::shoot(u, id, gi, target, forced).is_some()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::statics::{ShipRace, ShipType};
    use crate::tunables::SimTweaks;

    struct GunCase {
        gimble: bool,
        pos: [u32; 3],
        m: [u32; 9],
        tcoll: [u32; 3],
        tvel: [u32; 3],
        tsize: u32,
        heading: [u32; 3],
        gpos: [u32; 3],
        gnorm: [u32; 3],
        cosmin: u32,
        cosmax: u32,
        speed: u32,
        exp_ret: bool,
        exp_heading: [u32; 3],
    }

    /// Inputs and results of emulating `gunOrientGimbleGun` (0x485c10, gimbal guns) and the
    /// fixed-gun hit check (0x488360) in cataclysm.exe (Ghidra p-code emulation, with the `fmathSqrtDouble` table the game builds at start-up
    /// written into the emulator's memory).
    const GUN_CASES: &[GunCase] = &[
    GunCase { gimble: true, pos: [0x44385a40, 0x44b5571a, 0x44dd652a], m: [0xbe87cf9a, 0x3dcf55bc, 0x3f757717, 0xbeb690c7, 0xbf6f2c25, 0x0, 0x3f655483, 0xbeaf0d7c, 0x3e915dc3], tcoll: [0x4524eb7a, 0x43a35a5a, 0x455943b9], tvel: [0x42ee5bfa, 0x43a05c36, 0xc39ab7b6], tsize: 0x42c63f09, heading: [0x0, 0x0, 0x0], gpos: [0xc1223182, 0x3fe00e3c, 0x403d4a19], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f800000, cosmax: 0x3f3d1c1d, speed: 0x43fa0000, exp_ret: true, exp_heading: [0x3ebc62ca, 0x3e07e866, 0x3f6b9bf9] },
    GunCase { gimble: true, pos: [0xc4a56363, 0x451c2128, 0x44c74b49], m: [0xbe406e9b, 0xbe969cf9, 0x3f6fe59d, 0x3f57bc73, 0xbf09d186, 0x0, 0x3f012639, 0x3f4a2a6f, 0x3eb2b90b], tcoll: [0xc452c7cb, 0x4539a599, 0x44cc8689], tvel: [0x43948fd9, 0xc3686f55, 0xc3639d75], tsize: 0x4344a8f6, heading: [0x0, 0x0, 0x0], gpos: [0x416e574a, 0xc106d83e, 0x4193ac47], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f800000, cosmax: 0x3ed1689b, speed: 0x44bb8000, exp_ret: true, exp_heading: [0xbed3e829, 0x3eeb3890, 0x3f493360] },
    GunCase { gimble: true, pos: [0x452ef62d, 0x4513a733, 0xc496e888], m: [0xbe80518b, 0x3e9920b3, 0x3f6bb4de, 0x3f443715, 0x3f246cd3, 0x0, 0xbf176411, 0x3f34a931, 0xbec7c8c5], tcoll: [0x450cef05, 0x452b11ff, 0xc4b1205c], tvel: [0xc3c6a592, 0x430e58ef, 0xc301aeb9], tsize: 0x4289c8b5, heading: [0x0, 0x0, 0x0], gpos: [0x414bda00, 0xbf452b57, 0xc0ebc8ea], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f7851ec, cosmax: 0x3f1561c4, speed: 0x44bb8000, exp_ret: true, exp_heading: [0x3d725395, 0xbe9c5722, 0x3f734f34] },
    GunCase { gimble: true, pos: [0xc5261fe9, 0x4532298f, 0xc532ece8], m: [0x3ac1c5fe, 0x3ecde56e, 0x3f6a6339, 0xbf7fff8f, 0x3b70ecf1, 0x0, 0xbb5c95f6, 0xbf6a62d2, 0x3ecde5c9], tcoll: [0xc522c970, 0x451e86b0, 0xc52a112c], tvel: [0xc3c45e60, 0xc3b54f25, 0xc37f43b0], tsize: 0x433f7bf3, heading: [0x0, 0x0, 0x0], gpos: [0xc14239e4, 0x4123abdb, 0x41897d5d], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f7851ec, cosmax: 0x3f01b272, speed: 0x453b8000, exp_ret: false, exp_heading: [0x0, 0x0, 0x0] },
    GunCase { gimble: true, pos: [0x45134444, 0xc4bad45c, 0xc35ee6c5], m: [0x3e8f525e, 0x3e348f28, 0x3f7195a8, 0xbf087187, 0x3f589bbd, 0x0, 0xbf4c693e, 0xbf00c2a4, 0x3ea962c0], tcoll: [0x44ae1dc9, 0xc500ecfe, 0x43d3ae45], tvel: [0xc369369a, 0x4124dd5d, 0xc2dbcc9a], tsize: 0x4218a0f1, heading: [0x0, 0x0, 0x0], gpos: [0xc10f912f, 0x4101ab16, 0xc182895a], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f7fbe77, cosmax: 0x3ef7a7df, speed: 0x44480000, exp_ret: true, exp_heading: [0x3e42c0c2, 0x3c99da3a, 0x3f7b48e9] },
    GunCase { gimble: true, pos: [0x443e356a, 0x44a5d9ee, 0xc48f05b6], m: [0x3e84f75a, 0xbcdcb282, 0x3f771eca, 0x3dd35204, 0x3f7ea230, 0x22800000, 0xbf75cd1d, 0x3dcbfd8c, 0x3e85ae04], tcoll: [0x438b27ff, 0x44bd90d9, 0xc49059ae], tvel: [0x43407261, 0xc2f9de0b, 0xc347d53a], tsize: 0x4305af02, heading: [0x0, 0x0, 0x0], gpos: [0xc162cc24, 0x41994bd4, 0x40b10e21], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f7851ec, cosmax: 0x3f002180, speed: 0x44bb8000, exp_ret: true, exp_heading: [0xbece7534, 0x3e45aba9, 0x3f6500ad] },
    GunCase { gimble: true, pos: [0xc50a891b, 0x44adbf92, 0x4528aca9], m: [0x3e6d582f, 0x3e7cffff, 0x3f70dcc2, 0xbf3ab426, 0x3f2f268c, 0x0, 0xbf24cb2d, 0xbf2fa9e4, 0x3ead7388], tcoll: [0xc51e937d, 0x44724246, 0x4538af19], tvel: [0xc3242650, 0xc215e77f, 0x43c7b851], tsize: 0x432bed92, heading: [0x0, 0x0, 0x0], gpos: [0x4198528a, 0xbfede011, 0xbef280f3], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f7fbe77, cosmax: 0x3f166184, speed: 0x453b8000, exp_ret: true, exp_heading: [0x3e2b8b1d, 0xbd571a15, 0x3f7c06cc] },
    GunCase { gimble: true, pos: [0xc33383da, 0x4516b29b, 0xc5114a35], m: [0xbc87c465, 0x3d0c7ce9, 0x3f7fd06f, 0xbf668062, 0xbedec18c, 0x0, 0x3ede9828, 0xbf66558e, 0x3d1c076a], tcoll: [0x43bbb665, 0x44e9d123, 0xc518f1c8], tvel: [0xc0d0550b, 0x41424a21, 0x4219ebf4], tsize: 0x42d312c4, heading: [0x0, 0x0, 0x0], gpos: [0xc18bce21, 0x413ce5ab, 0xc0b8b49d], gnorm: [0x0, 0x0, 0x3f800000], cosmin: 0x3f800000, cosmax: 0x3f417b03, speed: 0x453b8000, exp_ret: true, exp_heading: [0xbe1f8e26, 0xbeb3e07f, 0x3f6c58cf] },
    GunCase { gimble: false, pos: [0x43b1f168, 0x447c7619, 0x44fb6ca3], m: [0x3d4e77f9, 0xbf0b15e3, 0x3f5688a0, 0x3f7ee7bd, 0x3dbd3331, 0x0, 0xbd9e8dc4, 0x3f559dc3, 0x3f0baece], tcoll: [0x445b70fa, 0x453990b4, 0x45383719], tvel: [0x438cba93, 0xc1a47494, 0x435af93f], tsize: 0x428fd808, heading: [0x3f7ee727, 0xbdbd65b9, 0x3920519a], gpos: [0x40de41bf, 0xc0b796ef, 0xc110b837], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7fbe77, cosmax: 0x3f3e215a, speed: 0x43fa0000, exp_ret: false, exp_heading: [0x3f7ee727, 0xbdbd65b9, 0x3920519a] },
    GunCase { gimble: false, pos: [0xc460eea9, 0x45009471, 0x450ab77b], m: [0x3e5256c1, 0x3f000f58, 0x3f575726, 0xbf6ccffc, 0x3ec27b8c, 0x24000000, 0xbea397f8, 0xbf47334a, 0x3f0a6f98], tcoll: [0xc4ed9eb2, 0xc3ccf343, 0x454fbf7d], tvel: [0x4386a5eb, 0x43aef3e3, 0xc191abbb], tsize: 0x430d5ef1, heading: [0x3f7d4338, 0x3dab9b94, 0xbdf47b2b], gpos: [0x41337016, 0x404ef6c7, 0x40d3e977], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7851ec, cosmax: 0x3ef6943b, speed: 0x453b8000, exp_ret: false, exp_heading: [0x3f7d4338, 0x3dab9b94, 0xbdf47b2b] },
    GunCase { gimble: false, pos: [0x44a54f94, 0xc5312449, 0xc4fefb92], m: [0x3e2af0ae, 0xbd8543e1, 0x3f7bdb88, 0x3eb9f342, 0x3f6e8508, 0xa3000000, 0xbf6aa8fa, 0x3eb6f0f7, 0x3e3777c2], tcoll: [0x4407b529, 0xc516c329, 0xc50548e4], tvel: [0xc1b5da4f, 0xc35b0184, 0xc3b257c1], tsize: 0x420d7afe, heading: [0x3f7b7709, 0xbe0a39f1, 0xbe051950], gpos: [0xc19496ef, 0xbfb18273, 0xc09937ac], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f800000, cosmax: 0x3f277302, speed: 0x44bb8000, exp_ret: false, exp_heading: [0x3f7b7709, 0xbe0a39f1, 0xbe051950] },
    GunCase { gimble: false, pos: [0x44a382ed, 0xc43cfbce, 0xc460382c], m: [0xbeca550a, 0x3dfdddf1, 0x3f6902fa, 0xbe993d76, 0xbf7443d4, 0xa3800000, 0x3f5e5493, 0xbe8b7ab8, 0x3ed40d7d], tcoll: [0x4555d652, 0xc4a029c3, 0x44149630], tvel: [0x421fc90f, 0xc31ee4e0, 0xc323c662], tsize: 0x42d6ce5e, heading: [0x3f7f0d97, 0xbd8efd4b, 0xbd4d2f84], gpos: [0xc16d5677, 0xc06856a5, 0x4023a2b2], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7fbe77, cosmax: 0x3f149a8f, speed: 0x44bb8000, exp_ret: false, exp_heading: [0x3f7f0d97, 0xbd8efd4b, 0xbd4d2f84] },
    GunCase { gimble: false, pos: [0x440fe80e, 0x43bf93d1, 0x452b7469], m: [0xbe02517c, 0x3cd7dbd5, 0x3f7dd42d, 0xbe4f9d7f, 0xbf7aaedd, 0x22800000, 0x3f788e95, 0xbe4ddaba, 0x3e051513], tcoll: [0x44d797dc, 0xc2ba9c30, 0x452530a1], tvel: [0x42529934, 0x42b844d4, 0xc38fb845], tsize: 0x430198c1, heading: [0x3f7fb16e, 0xbd2fabe4, 0xbcc154d3], gpos: [0xc12f2522, 0xc1057187, 0x41972f7c], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7851ec, cosmax: 0x3f07c408, speed: 0x44bb8000, exp_ret: false, exp_heading: [0x3f7fb16e, 0xbd2fabe4, 0xbcc154d3] },
    GunCase { gimble: false, pos: [0x440fb708, 0xc4b421ab, 0x45345e22], m: [0xbdcf449e, 0x3b247b7d, 0x3f7eaf4e, 0x3ccb17a2, 0x3f7febdb, 0x21000000, 0xbf7e9b44, 0x3cca0c86, 0xbdcf54ef], tcoll: [0xc4045326, 0xc4b04414, 0x451de172], tvel: [0xc1939c8d, 0xc3973edf, 0x42c2b2d7], tsize: 0x42bc8488, heading: [0x3f7b95d6, 0x3df741bd, 0x3e0f6b9c], gpos: [0xc19bc663, 0x3fa6bb24, 0xc110c6a3], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7851ec, cosmax: 0x3f44e6a5, speed: 0x44bb8000, exp_ret: false, exp_heading: [0x3f7b95d6, 0x3df741bd, 0x3e0f6b9c] },
    GunCase { gimble: false, pos: [0xc26d2606, 0x4500f7be, 0x44a6179c], m: [0xbb06e3f1, 0x3c144f13, 0x3f7ffd2d, 0x3f79a0d0, 0x3e630aec, 0x0, 0xbe63086b, 0x3f799e0f, 0xbc181831], tcoll: [0xc40e57fe, 0x459ab75b, 0x44e85fad], tvel: [0x439e0049, 0xc3c187e9, 0xc395964a], tsize: 0x427e9585, heading: [0x3f7ed95c, 0x3d2b025a, 0x3dae1ee9], gpos: [0xc1977ba2, 0xc0c04d17, 0x40e8a799], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f800000, cosmax: 0x3ea4ca7d, speed: 0x453b8000, exp_ret: false, exp_heading: [0x3f7ed95c, 0x3d2b025a, 0x3dae1ee9] },
    GunCase { gimble: false, pos: [0x439faf45, 0x448b4cb7, 0xc5132657], m: [0xbe0ed6f3, 0xbeab1ec1, 0x3f6e9f22, 0xbf6c3fbb, 0x3ec53470, 0xa3800000, 0xbeb7d156, 0xbf5c361b, 0xbeb96d1e], tcoll: [0x41b58b89, 0xc37b457d, 0xc52d94ee], tvel: [0x43b54ea0, 0x42ee2ee0, 0x438f7a27], tsize: 0x4329d3fd, heading: [0x3f7e50ba, 0x3d95d5e8, 0x3db47912], gpos: [0x417f4b50, 0xbfb2c43b, 0x41253ef4], gnorm: [0x3f800000, 0x0, 0x0], cosmin: 0x3f7fbe77, cosmax: 0x3f4d409d, speed: 0x43fa0000, exp_ret: false, exp_heading: [0x3f7e50ba, 0x3d95d5e8, 0x3db47912] },
    ];

    #[test]
    fn aiming_matches_emulated_binary() {
        let f = f32::from_bits;
        let v = |a: [u32; 3]| Vector::new(f(a[0]), f(a[1]), f(a[2]));
        let bits = |v: Vector| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        for (i, c) in GUN_CASES.iter().enumerate() {
            let mut a = ShipStaticInfo::new(ShipType::sAcolyte, String::new());
            a.ship_race = ShipRace::Somtaaw;
            a.health.max_health = 100.0;
            let mut gs = GunStatic { gun_type: Some(if c.gimble { GunType::Gimble } else { GunType::Fixed }), ..Default::default() };
            gs.position = v(c.gpos);
            gs.gun_normal = v(c.gnorm);
            gs.cos_min_angle_from_norm = f(c.cosmin);
            gs.cos_max_angle_from_norm = f(c.cosmax);
            gs.trigger_happy = 0.99;
            gs.bullet_speed = f(c.speed);
            a.guns = Some(vec![gs]);
            let mut t = ShipStaticInfo::new(ShipType::tHeavyInterceptor, String::new());
            t.ship_race = ShipRace::Taiidan;
            t.health.max_health = 100.0;
            t.header.coll.coll_sphere_size = f(c.tsize);
            let statics = Arc::new(crate::statics::test_world(vec![a, t]));
            let mut u = Universe::new(statics, SimTweaks::compiled(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
            let id = u.add_ship(ShipType::sAcolyte, 0, v(c.pos), None).unwrap();
            let tid = u.add_ship(ShipType::tHeavyInterceptor, 1, Vector::ZERO, None).unwrap();
            {
                let s = u.ships.ship_mut(id);
                s.rotinfo.coordsys = Matrix { m: c.m.map(f) };
                s.guns[0].heading = v(c.heading);
                s.guns[0].level = 0;
                let t = u.ships.ship_mut(tid);
                t.coll.coll_position = v(c.tcoll);
                t.posinfo.velocity = v(c.tvel);
            }
            let ret = if c.gimble { orient_gimble(&mut u, id, 0, tid) } else { fixed_can_hit(&u, id, 0, tid, true) };
            assert_eq!(ret, c.exp_ret, "case {i} result");
            if c.gimble && c.exp_ret {
                assert_eq!(bits(u.ships.ship(id).guns[0].heading), c.exp_heading, "case {i} heading");
            }
        }
    }
}
