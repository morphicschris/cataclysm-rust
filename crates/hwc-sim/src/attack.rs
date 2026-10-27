//! The ship-type attack routines (`Attack.c` and the ship modules as compiled into Cataclysm):
//! what a ship does each frame while it attacks a target (`CustShipAttack`), while it only
//! fires at what comes into range (`CustShipAttackPassive`) and how it fires
//! (`CustShipFire`).
//!
//! * fighters: `GenericInterceptorAttack` (0x5ad150), a state machine of attack runs with
//!   flight manoeuvres, breaking off and turning around (Acolytes use it too);
//! * capital ships and frigates: `attackPassive` with the ship's ranges (close in, hold at
//!   the minimum range, keep the target in front and fire), `attackPassiveRotate` /
//!   `attackSimple` when only retaliating;
//! * corvettes and swarmers: `attackSideStep`;
//! * the multi-beam frigate: its own close-in-and-strafe routine with ion beams.
//!
//! These follow the decompiled code; the floating-point steps are kept in the original order
//! but not emulator-verified (the shared gun code they call is).

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st, Ext};
use hwc_math::random::RAN_GAME;
use hwc_math::{Matrix, Vector};

use crate::aiship::{self, flags as fly};
use crate::aitrack;
use crate::combat_statics::{class_index, is_acolyte, AttackKind, FireKind, PassiveKind, TypeCombat, CLASS_COLUMNS};
use crate::flightman::{self, family, kind as fm, tactalk};
use crate::gun;
use crate::object::{special, ShipId};
use crate::physics::apply_force;
use crate::statics::{GunType, ShipClass};
use crate::target::Target;
use crate::universe::Universe;

/// The heading accuracy the attack routines track targets with (0x7436e8, 0x7436ec).
pub const HEADING_ACCURACY: f32 = f32::from_bits(0x3f7f_be77);
/// The tighter accuracy of the whip strafe run (0x3f7fff58).
const WHIP_HEADING_ACCURACY: f32 = f32::from_bits(0x3f7f_ff58);
/// `DEG_TO_RAD` as the compiler stored it.
const DEG_TO_RAD: f32 = 0.017_453_292;

fn combat(u: &Universe, id: ShipId) -> Option<TypeCombat> {
    let s = u.ships.get(id)?;
    u.combat.get(s.ship_type).cloned()
}

/// Whether the type has an attack routine (`CustShipAttack` set).
pub fn can_attack(u: &Universe, id: ShipId) -> bool {
    combat(u, id).is_some_and(|c| !matches!(c.attack, AttackKind::None))
}

/// Whether the type has a passive-attack routine.
pub fn can_attack_passive(u: &Universe, id: ShipId) -> bool {
    combat(u, id).is_some_and(|c| !matches!(c.passive, PassiveKind::None))
}

/// `CustShipAttack`: one frame of attacking `target`.
pub fn ship_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(c) = combat(u, id) else { return };
    if !crate::target::exists(u, target) {
        return;
    }
    match c.attack {
        AttackKind::Interceptor => interceptor_attack(u, id, target, &c),
        AttackKind::Acolyte => {
            crate::ships::acolyte::attack_target_changed(u, id, target);
            interceptor_attack(u, id, target, &c);
        }
        AttackKind::Capital => {
            let tac = u.ships.ship(id).tactics;
            attack_passive(u, id, target, c.capital_range[tac], c.capital_min_range[tac]);
        }
        AttackKind::Default => {
            let s = u.ships.ship(id);
            if s.guns.is_empty() {
                return;
            }
            let stat = u.statics.ship(s.ship_type).expect("statics");
            let tac = s.tactics;
            let (max, min) = (stat.bullet_range[tac], st(ld(stat.min_bullet_range[tac]) * 0.9));
            attack_passive(u, id, target, max, min);
        }
        AttackKind::SideStep => attack_side_step(u, id, target, &c),
        AttackKind::MultiBeam => multibeam_attack(u, id, target, &c),
        AttackKind::Unsupported => {
            crate::ships::attack(u, id, target);
        }
        AttackKind::None => {}
    }
}

/// `CustShipAttackPassive`: fire at `target` without chasing it; `rotate` lets ships that may
/// turn to retaliate (`rotateToRetaliate`) do so.
pub fn ship_attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    let Some(c) = combat(u, id) else { return };
    if !crate::target::exists(u, target) {
        return;
    }
    let stat = u.statics.ship(u.ships.ship(id).ship_type).expect("statics").clone();
    let rotate = rotate && stat.rotate_to_retaliate;
    match c.passive {
        PassiveKind::Interceptor => interceptor_passive(u, id, target, rotate, &c),
        PassiveKind::Default => {
            if rotate {
                attack_passive_rotate(u, id, target);
            } else {
                attack_simple(u, id, target);
            }
        }
        PassiveKind::MultiBeam => ship_fire(u, id, target),
        PassiveKind::Unsupported => {
            crate::ships::attack_passive(u, id, target, rotate);
        }
        PassiveKind::None => {}
    }
}

/// `CustShipFire`.
pub fn ship_fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(c) = combat(u, id) else { return };
    match c.fire {
        FireKind::Interceptor => interceptor_fire(u, id, target),
        FireKind::Acolyte => crate::ships::acolyte::fire(u, id, target),
        FireKind::Default => gun::default_fire(u, id, target),
        FireKind::GenericCapital => generic_capital_fire(u, id, target),
        FireKind::MultiBeam => multibeam_fire(u, id, target, &c),
        FireKind::Unsupported => {
            crate::ships::fire(u, id, target);
        }
        FireKind::None => {}
    }
}

/// `GenericCapitalFire`: every ready gun whose weapon fires bullets or beams.
fn generic_capital_fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let n = u.ships.ship(id).guns.len();
    for gi in 0..n {
        if !gun::ready(u, id, gi) {
            continue;
        }
        let level = u.ships.ship(id).guns[gi].level;
        if matches!(level, 0 | 1 | 2 | 7) {
            gun::shoot(u, id, gi, Some(target));
        } else if matches!(level, 5 | 6) && u.ships.ship(id).guns[gi].num_missiles > 0 {
            gun::shoot_missile(u, id, gi, Some(target));
        }
    }
}

/// Normalises `traj` by its `fmathSqrtDouble` length (stored as a float, then `1/len` in a
/// register); returns the stored length.
fn normalize_by_dist(traj: &mut Vector) -> f32 {
    let dist = st(aiship::traj_dist(*traj));
    let inv = 1.0 / ld(dist);
    traj.x = st(inv * ld(traj.x));
    traj.y = st(inv * ld(traj.y));
    traj.z = st(inv * ld(traj.z));
    dist
}

fn heading(u: &mut Universe, id: ShipId, desired: &mut Vector, acc: f32, sinbank: f32, flags: u32) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    aitrack::heading(s, stat, &tw, dt, desired, acc, acc, sinbank, flags)
}

/// The target (a ship or a mine) as the attack routines read it.
fn target_stat(u: &Universe, target: ShipId) -> Target {
    crate::target::get(u, target).expect("attack target")
}

/// The target-class column of the interceptor tables (`NUM_CLASSES` for a mine).
fn target_class(u: &Universe, target: ShipId) -> usize {
    class_index(target_stat(u, target).ship_class)
}

/// `GenericInterceptorCanFire` (0x5acc50): the fire interval has passed, a gun is ready (the
/// search starts at the active gun) and it can hit.
fn interceptor_can_fire(u: &mut Universe, id: ShipId, target: ShipId, c: &TypeCombat) -> bool {
    let Some(ic) = c.interceptor.as_ref() else { return false };
    let s = u.ships.ship(id);
    // A disguised Mimic holds fire. (The gun-opening animation is not modelled.)
    if s.flags2 & crate::object::flags2::DISGUISED != 0 && crate::ships::mimic::is_disguised(u, id) {
        return false;
    }
    if !(ld(ic.fire_time) <= ld(u.total_time_elapsed) - ld(s.interceptor.last_time_fired)) {
        return false;
    }
    let mut n = s.guns.len() as i32;
    if is_acolyte(s.ship_type) {
        n /= 2;
    }
    if n <= 0 {
        return false;
    }
    let mut g = s.interceptor.active_gun as i32;
    let mut tries = 0;
    while !gun::ready(u, id, g as usize) {
        g += 1;
        if n <= g {
            g = 0;
        }
        tries += 1;
        if n <= tries {
            return false;
        }
    }
    u.ships.ship_mut(id).interceptor.active_gun = g as u16;
    let statics = u.statics.clone();
    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics");
    let guns = stat.guns.as_ref().expect("guns");
    match guns[g as usize].gun_type {
        Some(GunType::Fixed) => gun::fixed_can_hit(u, id, g as usize, target, true),
        Some(GunType::Gimble) => gun::orient_gimble(u, id, g as usize, target),
        Some(GunType::NewGimble) => {
            let mut result = false;
            for gi in 0..guns.len() {
                let r = gun::orient_gimble(u, id, gi, target);
                if gi == g as usize {
                    result = r;
                }
            }
            result
        }
        _ => false,
    }
}

/// `GenericInterceptorFire` (0x5ace80): fire the active gun and move on to the next.
pub fn interceptor_fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let now = u.total_time_elapsed;
    let g = {
        let s = u.ships.ship_mut(id);
        s.interceptor.last_time_fired = now;
        s.interceptor.active_gun as usize
    };
    if g < u.ships.ship(id).guns.len() {
        gun::shoot(u, id, g, Some(target));
    }
    let s = u.ships.ship_mut(id);
    s.interceptor.active_gun += 1;
    if s.guns.len() <= s.interceptor.active_gun as usize {
        s.interceptor.active_gun = 0;
    }
    if is_acolyte(s.ship_type) && 1 < s.interceptor.active_gun {
        s.interceptor.active_gun = 0;
    }
}

/// The interceptor trajectory: led by the first gun's bullet speed when the ship has guns.
fn interceptor_trajectory(u: &Universe, id: ShipId, target: ShipId) -> Vector {
    let s = u.ships.ship(id);
    let t = &target_stat(u, target);
    let stat = u.statics.ship(s.ship_type).expect("statics");
    match stat.guns.as_ref().and_then(|g| g.first()) {
        Some(gs) if !s.guns.is_empty() => {
            let speed = gun::bullet_speed(&s.guns[0], gs);
            aiship::trajectory_with_vel_prediction(s, t, speed, u.tweaks.combat.dont_use_velocity_pred_if_below)
        }
        _ => aiship::trajectory(s, t),
    }
}

/// `GenericInterceptorPassiveAttack` (0x5aceb0).
fn interceptor_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool, c: &TypeCombat) {
    let Some(ic) = c.interceptor.as_ref() else { return };
    let mut traj = interceptor_trajectory(u, id, target);
    let dist = normalize_by_dist(&mut traj);
    let tstat = target_stat(u, target);
    let r = st(aiship::range_given_coll(&tstat.coll_header, ld(dist)));
    if rotate {
        heading(u, id, &mut traj, HEADING_ACCURACY, 0.0, 0);
    }
    let s = u.ships.ship(id);
    let stat = u.statics.ship(s.ship_type).expect("statics");
    let tac = s.tactics;
    if ld(r) < ld(stat.bullet_range[tac]) {
        let _ = ic.trigger_happy[tac][target_class(u, target).min(CLASS_COLUMNS - 1)];
        if interceptor_can_fire(u, id, target, c) {
            ship_fire(u, id, target);
        }
    }
}

/// `setupWhipStrafe` (0x5ad0b0): against capital ships, dive past the target from above or
/// below when the angle allows it.
fn setup_whip_strafe(u: &mut Universe, id: ShipId, target: ShipId) {
    let t = u.tweaks.combat.flightman.clone();
    let s = u.ships.ship(id);
    let tg = target_stat(u, target);
    let stat = u.statics.ship(s.ship_type).expect("statics");
    let tz = tg.posinfo.position.z;
    let d = ld(t.whipstrafe_bulletrangeoptimum) * ld(stat.bullet_range[s.tactics]) + ld(tg.coll_header.approx_coll_sphere_size);
    let mz = s.posinfo.position.z;
    let z = if mz <= tz { ld(tz) - d } else { d + ld(tz) };
    let dx = ld(tg.posinfo.position.x) - ld(s.posinfo.position.x);
    let dy = ld(tg.posinfo.position.y) - ld(s.posinfo.position.y);
    let dz = z - ld(s.posinfo.position.z);
    if dz * dz <= (dx * dx + dy * dy) * ld(t.whipstrafe_maxangcomparefactor) {
        let k = ld(t.whipstrafe_flybyovershoot);
        let p = s.posinfo.position;
        let v = Vector::new(st(k * dx + ld(p.x)), st(k * dy + ld(p.y)), st(k * dz + ld(p.z)));
        let now = u.total_time_elapsed;
        let sm = u.ships.ship_mut(id);
        sm.interceptor.ai_vec = v;
        sm.interceptor.aip_flightman = 1;
        flightman::init(u, id, fm::WHIP_STRAFE, -1, None);
        let sm = u.ships.ship_mut(id);
        sm.flightman_state2 = u8::from(tz < mz);
        sm.ai_state_attack = 10;
        sm.interceptor.ai_time_kill = now;
    }
}

/// A random whole number of degrees in `[lo, hi]`.
fn random_degrees(u: &mut Universe, lo: i32, hi: i32) -> u32 {
    let r = u.randy.random(RAN_GAME);
    r % ((hi - lo) as u32).wrapping_add(1) + lo as u32
}

/// `GenericInterceptorAttack` (0x5ad150).
fn interceptor_attack(u: &mut Universe, id: ShipId, target: ShipId, c: &TypeCombat) {
    let Some(ic) = c.interceptor.clone() else { return };
    let statics = u.statics.clone();
    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics").clone();
    let tstat = target_stat(u, target);
    let tclass = class_index(tstat.ship_class).min(CLASS_COLUMNS - 1);
    let tac = u.ships.ship(id).tactics;
    let range = stat.bullet_range[tac];
    let prob = ic.flightman_prob[tac][tclass];
    let tw = u.tweaks.combat.clone();
    let now = u.total_time_elapsed;
    let target_pos = target_stat(u, target).posinfo.position;
    let attack_state = u.ships.ship(id).ai_state_attack;
    match attack_state {
        0 | 1 | 5 => {
            let state = u.ships.ship(id).ai_state_attack;
            if state == 0 {
                // (The bombing run some bombers make on non-ship targets is not modelled.)
                let s = u.ships.ship_mut(id);
                s.interceptor.aip_flightman = 0;
                s.ai_state_attack = 1;
            }
            let traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let f = if state == 5 { fly::POINT_IN_DIRECTION_FLYING } else { fly::POINT_IN_DIRECTION_FLYING | fly::CAR_TURN };
            aiship::fly_to_point_avoiding_objs(u, id, Some(target_pos), f, 0.0, None);
            let r = aiship::range_given_coll(&tstat.coll_header, aiship::traj_dist(traj));
            if r < ld(range) {
                let s = u.ships.ship_mut(id);
                s.ai_state_attack = 2;
                s.interceptor.ai_time_kill = now;
            }
        }
        2 => {
            let mut traj = interceptor_trajectory(u, id, target);
            let dist = normalize_by_dist(&mut traj);
            let tv = target_stat(u, target).posinfo.velocity;
            let dot = (ld(traj.z) * ld(tv.z) + ld(traj.y) * ld(tv.y)) + ld(traj.x) * ld(tv.x);
            let with = if dot < 0.0 { None } else { Some(tv) };
            aiship::fly_to_point_avoiding_objs(u, id, Some(target_pos), fly::FAST_AS_POSSIBLE, -stat.header.max_velocity, with);
            let running = u.ships.ship(id).flightman;
            let mut track_flags = 0;
            if matches!(running, fm::BARREL_ROLL | fm::SWARMER_BARRELROLL | fm::ROLL180) {
                track_flags = aitrack::DONT_ROLL;
                if flightman::execute(u, id) {
                    u.ships.ship_mut(id).tactics_talk = 0;
                }
            } else if matches!(running, fm::CELEB_FLIP | fm::BARRELROLL_OUT) {
                flightman::execute(u, id);
                return;
            }
            let acc = ic.face_target_accuracy[tac][tclass];
            heading(u, id, &mut traj, acc, stat.sinbank, track_flags);
            let r = st(aiship::range_given_coll(&tstat.coll_header, ld(dist)));
            if ld(range) <= ld(r) {
                return;
            }
            if interceptor_can_fire(u, id, target, c) {
                ship_fire(u, id, target);
                let s = u.ships.ship(id);
                if s.flightman == fm::NULL && s.interceptor.aip_flightman == 0 {
                    u.ships.ship_mut(id).interceptor.aip_flightman = 1;
                    let pick = if flightman::test_random(u, &prob, family::AIP, fm::BARREL_ROLL) {
                        u.ships.ship_mut(id).tactics_talk |= tactalk::BARRELROLL;
                        Some(fm::BARREL_ROLL)
                    } else if flightman::test_random(u, &prob, family::AIP, fm::CELEB_FLIP) {
                        Some(fm::CELEB_FLIP)
                    } else if flightman::test_random(u, &prob, family::AIP, fm::SWARMER_BARRELROLL) {
                        Some(fm::SWARMER_BARRELROLL)
                    } else if flightman::test_random(u, &prob, family::AIP, fm::ROLL180) {
                        Some(fm::ROLL180)
                    } else {
                        None
                    };
                    if let Some(p) = pick {
                        flightman::init(u, id, p, -1, None);
                    }
                }
            }
            if ld(ic.break_range[tac][tclass]) <= ld(r) {
                return;
            }
            // Break off: pick a point to fly away to, rotated off the line to the target.
            let deg = random_degrees(u, tw.fighter_break_angle_min, tw.fighter_break_angle_max);
            let angle = if !flightman::test_random(u, &prob, family::AIP, fm::BARRELROLL_OUT) {
                let s = u.ships.ship_mut(id);
                s.ai_state_attack = 3;
                s.tactics_talk = 0;
                if tclass == ShipClass::Fighter.index() {
                    deg as i32 as f32
                } else if deg & 1 == 0 {
                    (deg as i32).wrapping_neg() as f32
                } else {
                    deg as i32 as f32
                }
            } else {
                u.ships.ship_mut(id).ai_state_attack = 12;
                flightman::init(u, id, fm::BARRELROLL_OUT, -1, None);
                deg as i32 as f32
            };
            let mut rad = st(ld(angle) * ld(DEG_TO_RAD));
            let s = u.ships.ship(id);
            let form = match s.command.and_then(|c| u.commands.get(c)) {
                Some(cmd) if cmd.attributes & crate::command::attr::FORMATION != 0 && s.tactics != 0 => cmd.formation.formation_type,
                _ => 7,
            };
            let tz = if traj.z < 0.0 { -traj.z } else { traj.z };
            let mut v = Vector::new(st(ld(traj.x) * 15000.0), st(ld(traj.y) * 15000.0), st(ld(traj.z) * 15000.0));
            let rot = if tz < 0.75 {
                if form == 0 || form == 1 {
                    let vdeg = random_degrees(u, tw.fighter_break_vertical_angle_min, tw.fighter_break_vertical_angle_max);
                    rad = st(ld(vdeg as i32 as f32) * ld(DEG_TO_RAD));
                    let mut flat = Vector::new(traj.x, traj.y, 0.0);
                    flat.normalize_to_length(15000.0);
                    let ax = if traj.x < 0.0 { -traj.x } else { traj.x };
                    let ay = if traj.y < 0.0 { -traj.y } else { traj.y };
                    let (sn, cs) = (st(hwc_math::trig::fsin(ld(rad))), st(hwc_math::trig::fcos(ld(rad))));
                    let m = if ax <= ay { Matrix::rot_about_x(cs, sn) } else { Matrix::rot_about_y(cs, sn) };
                    let mut av = m.mul_vec(flat);
                    if traj.z <= 0.0 {
                        if av.z < 0.0 {
                            av.z = -av.z;
                        }
                    } else if 0.0 < av.z {
                        av.z = -av.z;
                    }
                    finish_ai_vec(u, id, av, target_pos);
                    return;
                }
                let (sn, cs) = (st(hwc_math::trig::fsin(ld(rad))), st(hwc_math::trig::fcos(ld(rad))));
                Matrix::rot_about_z(cs, sn)
            } else {
                let (sn, cs) = (st(hwc_math::trig::fsin(ld(rad))), st(hwc_math::trig::fcos(ld(rad))));
                Matrix::rot_about_y(cs, sn)
            };
            v = rot.mul_vec(v);
            finish_ai_vec(u, id, v, target_pos);
        }
        3 => {
            let traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let far = !(tstat.ship_class.is_some_and(|c| c.index() < ShipClass::Corvette.index()));
            let d2 = (ld(traj.x) * ld(traj.x) + ld(traj.y) * ld(traj.y)) + ld(traj.z) * ld(traj.z);
            if d2 <= ld(ic.max_fly_away_dist_sq[tac][tclass]) {
                let aiv = u.ships.ship(id).interceptor.ai_vec;
                let reached = aitrack::move_reached_destination(u.ships.ship(id), aiv, tw.interceptor_break_tolerance);
                let flags = fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE | fly::DONT_FLY_TO_OBSCURED_POINTS | fly::RETURN_IMMED_IF_POINT_OBSCURED;
                if !far {
                    // Fly away from the capital ship along the line through the break point.
                    let mut away = Vector::new(st(ld(aiv.x) - ld(target_pos.x)), st(ld(aiv.y) - ld(target_pos.y)), st(ld(aiv.z) - ld(target_pos.z)));
                    away.normalize();
                    let p = Vector::new(
                        st(ld(away.x) * 20000.0 + ld(target_pos.x)),
                        st(ld(away.y) * 20000.0 + ld(target_pos.y)),
                        st(ld(away.z) * 20000.0 + ld(target_pos.z)),
                    );
                    if aiship::fly_to_point_avoiding_objs(u, id, Some(p), flags, tw.interceptor_break_min_velocity, None) & aiship::FLY_OBJECT_IN_WAY == 0 {
                        return;
                    }
                } else if !reached && aiship::fly_to_point_avoiding_objs(u, id, Some(aiv), flags, tw.interceptor_break_min_velocity, None) & aiship::FLY_OBJECT_IN_WAY == 0 {
                    return;
                }
            }
            let h = u.ships.ship(id).heading();
            let next = if (ld(traj.x) * ld(h.x) + ld(traj.y) * ld(h.y)) + ld(traj.z) * ld(h.z) <= 0.0 {
                u.ships.ship_mut(id).tactics_talk |= tactalk::FLIPTURN;
                flightman::get_random(u, &prob, family::TURNAROUND)
            } else {
                fm::DONOTHING
            };
            flightman::init(u, id, next, 0, None);
            u.ships.ship_mut(id).ai_state_attack = 4;
        }
        4 => {
            if flightman::execute(u, id) {
                let s = u.ships.ship_mut(id);
                s.tactics_talk = 0;
                s.ai_state_attack = 5;
                s.interceptor.aip_flightman = 0;
                s.ai_state = 0;
                let idle = s.flightman == fm::NULL;
                if idle
                    && tstat.is_capital_ship
                    && tstat.ship_class.is_some_and(|c| c != ShipClass::Mothership)
                    && flightman::test_random(u, &prob, family::AIP, fm::WHIP_STRAFE)
                {
                    setup_whip_strafe(u, id, target);
                }
            }
        }
        10 => {
            if u.ships.ship(id).flightman != fm::WHIP_STRAFE {
                u.ships.ship_mut(id).ai_state_attack = 0;
                return;
            }
            let mut traj = aiship::trajectory_with_angle_correction(u.ships.ship(id), &target_stat(u, target));
            let dist = normalize_by_dist(&mut traj);
            let r = aiship::range_given_coll(&tstat.coll_header, ld(dist));
            let aiv = u.ships.ship(id).interceptor.ai_vec;
            let minv = tw.flightman.whipstrafe_minvelocity;
            if ld(range) * 0.7 <= r {
                aiship::fly_to_point_avoiding_objs(u, id, Some(aiv), fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE, minv, None);
            } else {
                aiship::fly_to_point_avoiding_objs(u, id, Some(aiv), fly::FAST_AS_POSSIBLE, minv, None);
                heading(u, id, &mut traj, WHIP_HEADING_ACCURACY, stat.sinbank, 0);
                if interceptor_can_fire(u, id, target, c) {
                    ship_fire(u, id, target);
                }
            }
            let reached = aitrack::move_reached_destination(u.ships.ship(id), aiv, tw.flightman.whipstrafe_reachdestinationrange);
            let kill = u.ships.ship(id).interceptor.ai_time_kill;
            if reached || ld(ic.max_attack_time[tac][tclass]) < ld(now) - ld(kill) {
                u.ships.ship_mut(id).ai_state_attack = 11;
            }
        }
        11 => {
            if u.ships.ship(id).flightman != fm::WHIP_STRAFE {
                u.ships.ship_mut(id).ai_state_attack = 0;
                return;
            }
            if !flightman::execute(u, id) {
                return;
            }
            u.ships.ship_mut(id).ai_state_attack = 3;
            let p = u.ships.ship(id).posinfo.position;
            let mut v = Vector::new(st(ld(p.x) - ld(target_pos.x)), st(ld(p.y) - ld(target_pos.y)), 0.0);
            v.normalize_to_length(ic.fly_past_dist[tac][tclass]);
            let deg = random_degrees(u, tw.fighter_break_angle_min, tw.fighter_break_angle_max);
            let d = if deg & 1 == 0 { (deg as i32).wrapping_neg() } else { deg as i32 };
            let a = f64::from(d) * f64::from(DEG_TO_RAD);
            let (sn, cs) = (st(hwc_math::trig::fsin(a)), st(hwc_math::trig::fcos(a)));
            let av = Matrix::rot_about_z(cs, sn).mul_vec(v);
            finish_ai_vec(u, id, av, target_pos);
        }
        12 if flightman::execute(u, id) => {
            u.ships.ship_mut(id).ai_state_attack = 3;
        }
        _ => {}
    }
}

/// The break-off point is relative to the target.
fn finish_ai_vec(u: &mut Universe, id: ShipId, v: Vector, target_pos: Vector) {
    let s = u.ships.ship_mut(id);
    s.interceptor.ai_vec = Vector::new(st(ld(target_pos.x) + ld(v.x)), st(ld(target_pos.y) + ld(v.y)), st(ld(target_pos.z) + ld(v.z)));
}

/// `GenericInterceptorAttack` for the ship modules that call it (Mimic, cloaked fighter).
pub fn generic_interceptor_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if let Some(c) = combat(u, id) {
        interceptor_attack(u, id, target, &c);
    }
}

/// `GenericInterceptorPassiveAttack` for the ship modules that call it.
pub fn generic_interceptor_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if let Some(c) = combat(u, id) {
        interceptor_passive(u, id, target, rotate, &c);
    }
}

/// `attackSideStep` with the type's side-step statics, for the ship modules that call it.
pub fn side_step_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if let Some(c) = combat(u, id) {
        attack_side_step(u, id, target, &c);
    }
}

/// `attackSimple` (0x421b70): fire whatever guns bear, without moving.
pub fn attack_simple(u: &mut Universe, id: ShipId, target: ShipId) {
    let s = u.ships.ship(id);
    if s.special_flags & 0x20_0000 != 0 {
        return;
    }
    if s.attack.multiple_targets.is_some() {
        gun::shoot_guns_at_multiple_targets(u, id);
        return;
    }
    let mut traj = aiship::trajectory(s, &target_stat(u, target));
    let dist = normalize_by_dist(&mut traj);
    let r = st(aiship::range_given_coll(&target_stat(u, target).coll_header, ld(dist)));
    gun::shoot_guns_at_target(u, id, target, r, traj);
}

/// `attackPassiveRotate` (0x421c40): turn towards the target (level with it for carriers and
/// fast targets of capital ships) and fire.
pub fn attack_passive_rotate(u: &mut Universe, id: ShipId, target: ShipId) {
    let s = u.ships.ship(id);
    if s.special_flags & 0x20_0000 != 0 {
        return;
    }
    let stat = u.statics.ship(s.ship_type).expect("statics").clone();
    let mut traj = aiship::trajectory(s, &target_stat(u, target));
    let class = class_index(stat.ship_class);
    let tv = target_stat(u, target).posinfo.velocity;
    let tsq = (ld(tv.z) * ld(tv.z) + ld(tv.y) * ld(tv.y)) + ld(tv.x) * ld(tv.x);
    let lie_flat = class < 12 && stat.is_capital_ship && ld(u.tweaks.combat.cap_ship_lie_flat_speed_sq[class]) < tsq;
    if stat.ship_class == Some(ShipClass::Carrier) || lie_flat || matches!(s.ship_type, crate::statics::ShipType::sSwarmer) {
        traj.z = 0.0;
    }
    let dist = st(fsqrt((ld(traj.z) * ld(traj.z) + ld(traj.x) * ld(traj.x)) + ld(traj.y) * ld(traj.y)));
    let inv = 1.0 / ld(dist);
    traj = Vector::new(st(inv * ld(traj.x)), st(inv * ld(traj.y)), st(inv * ld(traj.z)));
    heading(u, id, &mut traj, HEADING_ACCURACY, 0.0, 0);
    if u.ships.ship(id).attack.multiple_targets.is_some() {
        gun::shoot_guns_at_multiple_targets(u, id);
        return;
    }
    let r = st(aiship::range_given_coll(&target_stat(u, target).coll_header, ld(dist)));
    gun::shoot_guns_at_target(u, id, target, r, traj);
}

/// `needToGoToSameVerticalPlane`: carriers and some big ships always fight level with their
/// target; other capital ships do when a fast target is far above or below.
fn need_same_vertical_plane(u: &Universe, id: ShipId, target: ShipId, dist: f32, speed_sq: f32) -> bool {
    use crate::statics::ShipType::*;
    let s = u.ships.ship(id);
    let stat = u.statics.ship(s.ship_type).expect("statics");
    if stat.ship_class == Some(ShipClass::Carrier) || matches!(s.ship_type, sSwarmer) {
        return true;
    }
    let t = target_stat(u, target);
    let v = t.posinfo.velocity;
    if ld(speed_sq) < (ld(v.z) * ld(v.z) + ld(v.y) * ld(v.y)) + ld(v.x) * ld(v.x) {
        let dz = ld(s.posinfo.position.z) - ld(t.posinfo.position.z);
        let dz = if dz < 0.0 { -dz } else { dz };
        if ld(dist) < dz {
            return true;
        }
    }
    false
}

/// `attackPassive` (0x421d60): close to `max` range (matching the target's velocity), hold at
/// `min`, keep the target in front and fire when in range. Capital ships stay level with fast
/// targets (`capShipLieFlat*`).
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, max: f32, min: f32) {
    let stat = u.statics.ship(u.ships.ship(id).ship_type).expect("statics").clone();
    let mut traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
    let raw = traj;
    let dist = normalize_by_dist(&mut traj);
    let r = st(aiship::range_given_coll(&target_stat(u, target).coll_header, ld(dist)));
    let class = class_index(stat.ship_class).min(11);
    let lie_speed = u.tweaks.combat.cap_ship_lie_flat_speed_sq[class];
    let lie_dist = u.tweaks.combat.cap_ship_lie_flat_distance[class];
    let tp = target_stat(u, target).posinfo.position;
    let tv = target_stat(u, target).posinfo.velocity;
    let mut in_range = false;
    if lie_speed == 0.0 || !need_same_vertical_plane(u, id, target, lie_dist, lie_speed) {
        if ld(r) <= ld(max) {
            if ld(r) <= ld(min) {
                let tw = u.tweaks.clone();
                let dt = u.phys_time_elapsed;
                aitrack::zero_velocity(u.ships.ship_mut(id), &stat, &tw, dt);
            } else {
                aiship::fly_to_point_avoiding_objs(u, id, Some(tp), 0, 0.0, Some(tv));
            }
            in_range = true;
        } else {
            aiship::fly_to_point_avoiding_objs(u, id, Some(tp), 0, 0.0, Some(tv));
        }
        heading(u, id, &mut traj, HEADING_ACCURACY, 0.0, 0);
        if !in_range {
            return;
        }
    } else {
        let p = u.ships.ship(id).posinfo.position;
        let mut point = Vector::new(p.x, p.y, tp.z);
        if ld(min) < ld(r) {
            point = Vector::new(st((ld(point.x) + ld(tp.x)) * 0.5), st((ld(point.y) + ld(tp.y)) * 0.5), st((ld(point.z) + ld(tp.z)) * 0.5));
        }
        let tclass = target_class(u, target);
        let attacking_me = (tclass == ShipClass::Fighter.index() || tclass == ShipClass::Corvette.index())
            && u.ships.get(target).is_some_and(|t| {
                t.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.order == crate::command::order::ATTACK)
                    && t.attack.target == Some(id)
            });
        if !attacking_me {
            aiship::fly_to_point_avoiding_objs(u, id, Some(point), 0, 0.0, Some(tv));
        }
        let mut flat = Vector::new(raw.x, raw.y, 0.0);
        flat.normalize();
        heading(u, id, &mut flat, HEADING_ACCURACY, 0.0, 0);
        if ld(max) <= ld(r) {
            return;
        }
    }
    // A disguised Mimic holds fire.
    if u.ships.ship(id).flags2 & crate::object::flags2::DISGUISED != 0 && crate::ships::mimic::is_disguised(u, id) {
        return;
    }
    if u.ships.ship(id).attack.multiple_targets.is_some() {
        gun::shoot_guns_at_multiple_targets(u, id);
        return;
    }
    gun::shoot_guns_at_target(u, id, target, r, traj);
}

/// The side-step direction after a burst: any of the four for ships with full freedom of
/// movement, else left/right (up/down when attacking from above).
fn side_step_direction(u: &mut Universe, id: ShipId, full: bool) -> f32 {
    let r = u.randy.random(RAN_GAME);
    if full {
        (r & 3) as f32
    } else if u.ships.ship(id).special_flags & special::ATTACKING_FROM_ABOVE == 0 {
        (3 - u32::from(r & 3 != 0)) as f32
    } else {
        u32::from(r & 3 == 0) as f32
    }
}

fn fire_guns(u: &mut Universe, id: ShipId, target: ShipId, range: f32, traj: Vector) -> bool {
    if u.ships.ship(id).attack.multiple_targets.is_some() {
        gun::shoot_guns_at_multiple_targets(u, id)
    } else {
        gun::shoot_guns_at_target(u, id, target, range, traj)
    }
}

/// `attackSideStep` (0x421fb0): close in and fire, then slide sideways while turning to keep
/// the target in front, and repeat.
pub fn attack_side_step(u: &mut Universe, id: ShipId, target: ShipId, c: &TypeCombat) {
    let p = c.side_step;
    let stat = u.statics.ship(u.ships.ship(id).ship_type).expect("statics").clone();
    let tstat = target_stat(u, target);
    let tac = u.ships.ship(id).tactics;
    let range = stat.bullet_range[tac];
    let now = u.total_time_elapsed;
    let tp = target_stat(u, target).posinfo.position;
    let full = p.full_movement_freedom;
    let state = u.ships.ship(id).ai_state_attack;
    let own_vel = |u: &Universe| u.ships.ship(id).posinfo.velocity;
    let mut back_to_start = false;
    match state {
        0 => {
            let traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            aiship::fly_to_point_avoiding_objs(u, id, Some(tp), fly::POINT_IN_DIRECTION_FLYING | fly::CAR_TURN, 0.0, None);
            let r = st(aiship::range_given_coll(&tstat.coll_header, aiship::traj_dist(traj)));
            if ld(r) < ld(range) && fire_guns(u, id, target, r, traj) {
                let dir = side_step_direction(u, id, full);
                let s = u.ships.ship_mut(id);
                s.ai_state_attack = 3;
                s.side_step.time = now;
                s.side_step.direction = dir;
                return;
            }
            if ld(r) < ld(p.circle_range) {
                u.ships.ship_mut(id).ai_state_attack = 1;
            }
        }
        1 => {
            let mut traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let dist = normalize_by_dist(&mut traj);
            let v = own_vel(u);
            aiship::fly_to_point_avoiding_objs(u, id, None, 0, 0.0, Some(v));
            heading(u, id, &mut traj, HEADING_ACCURACY, stat.sinbank, 0);
            let r = st(aiship::range_given_coll(&tstat.coll_header, ld(dist)));
            if ld(r) < ld(range) && fire_guns(u, id, target, r, traj) {
                let dir = side_step_direction(u, id, full);
                let s = u.ships.ship_mut(id);
                s.ai_state_attack = 2;
                s.side_step.time = now;
                s.side_step.direction = dir;
                return;
            }
            if !(ld(r) <= ld(p.circle_range)) {
                back_to_start = true;
            }
        }
        2 | 3 => {
            let mut traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let dist = normalize_by_dist(&mut traj);
            heading(u, id, &mut traj, HEADING_ACCURACY, stat.sinbank, 0);
            let r = st(aiship::range_given_coll(&tstat.coll_header, ld(dist)));
            let dir = u.ships.ship(id).side_step.direction as i32 as usize;
            let side = |u: &mut Universe| {
                let s = u.ships.ship_mut(id);
                let f = s.nonstat.thrust_strength[dir.min(5)];
                apply_force(s, f, dir as u16);
            };
            let forward = |u: &mut Universe| {
                let s = u.ships.ship_mut(id);
                let f = st(ld(s.nonstat.thrust_strength[4]) * 0.5);
                apply_force(s, f, 4);
            };
            if state == 2 {
                side(u);
                forward(u);
            } else {
                forward(u);
                side(u);
            }
            let v = own_vel(u);
            aiship::fly_to_point_avoiding_objs(u, id, None, 0, 0.0, Some(v));
            if u.ships.ship(id).attack.multiple_targets.is_some() {
                gun::shoot_guns_at_multiple_targets(u, id);
            }
            let started = u.ships.ship(id).side_step.time;
            if state == 2 && !(ld(r) <= ld(p.circle_range)) {
                back_to_start = true;
            } else if ld(now) - ld(started) <= ld(p.reposition_time) {
                return;
            } else if ld(r) < ld(p.circle_range) {
                u.ships.ship_mut(id).ai_state_attack = 1;
                return;
            } else {
                back_to_start = true;
            }
        }
        _ => {}
    }
    if back_to_start {
        u.ships.ship_mut(id).ai_state_attack = 0;
    }
}

/// The multi-beam frigate's beam range: its first gun's range times `MultiBeamRangeRatio`.
fn multibeam_range(u: &Universe, id: ShipId, c: &TypeCombat) -> Ext {
    let stat = u.statics.ship(u.ships.ship(id).ship_type).expect("statics");
    let r = stat.guns.as_ref().and_then(|g| g.first()).map_or(0.0, |g| g.bullet_range);
    ld(r) * ld(c.multi_beam_range_ratio)
}

/// `P2MultiBeamFrigateAttackDoAttack` (0x5b8100): close to three quarters of the beam range,
/// then strafe sideways firing until the target gets away.
fn multibeam_attack(u: &mut Universe, id: ShipId, target: ShipId, c: &TypeCombat) {
    let tstat = target_stat(u, target);
    let near = multibeam_range(u, id, c) * 0.75;
    let state = u.ships.ship(id).ai_state_attack;
    match state {
        0 | 1 => {
            if state == 0 {
                u.ships.ship_mut(id).ai_state_attack = 1;
            }
            let traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let r = aiship::range_given_coll(&tstat.coll_header, aiship::traj_dist(traj));
            let tp = target_stat(u, target).posinfo.position;
            let tv = target_stat(u, target).posinfo.velocity;
            if r < near || aiship::fly_to_point_avoiding_objs(u, id, Some(tp), fly::POINT_IN_DIRECTION_FLYING | fly::CAR_TURN, -10.0, Some(tv)) != 0 {
                let statics = u.statics.clone();
                let tw = u.tweaks.clone();
                let dt = u.phys_time_elapsed;
                let s = u.ships.ship_mut(id);
                aitrack::steady_ship(s, statics.ship(s.ship_type).expect("statics"), &tw, dt);
                s.ai_state_attack = 3;
                let rnd = u.randy.random(RAN_GAME);
                u.ships.ship_mut(id).side_step.direction = (3 - u32::from(rnd & 3 != 0)) as f32;
            }
            ship_fire(u, id, target);
        }
        3 => {
            let traj = aiship::trajectory(u.ships.ship(id), &target_stat(u, target));
            let dir = u.ships.ship(id).side_step.direction as i32 as usize;
            {
                let s = u.ships.ship_mut(id);
                let f = s.nonstat.thrust_strength[dir.min(5)];
                apply_force(s, f, dir as u16);
            }
            let v = u.ships.ship(id).posinfo.velocity;
            aiship::fly_to_point_avoiding_objs(u, id, None, 0, 0.0, Some(v));
            ship_fire(u, id, target);
            if !crate::target::exists(u, target) {
                return;
            }
            let r = aiship::range_given_coll(&tstat.coll_header, aiship::traj_dist(traj));
            if !(r <= near) {
                u.ships.ship_mut(id).ai_state_attack = 1;
            }
        }
        _ => {}
    }
}

/// `P2MultiBeamFrigateFire` (0x5b81e0): one beam at a time, at the target when a gun can
/// reach it, otherwise at any enemy in reach.
fn multibeam_fire(u: &mut Universe, id: ShipId, target: ShipId, c: &TypeCombat) {
    let range = multibeam_range(u, id, c);
    let statics = u.statics.clone();
    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics");
    let Some(guns) = stat.guns.as_ref() else { return };
    let r = crate::target::get(u, target).map(|t| {
        let traj = aiship::trajectory(u.ships.ship(id), &t);
        st(aiship::range_given_coll(&target_stat(u, target).coll_header, aiship::traj_dist(traj)))
    });
    let mut chosen: Option<(usize, ShipId)> = None;
    if let Some(r) = r {
        for (gi, gs) in guns.iter().enumerate() {
            if gun::ready(u, id, gi) && ld(r) < ld(gs.bullet_range) * ld(c.multi_beam_range_ratio) && gun::orient_gimble(u, id, gi, target) {
                chosen = Some((gi, target));
                break;
            }
        }
    }
    if chosen.is_none() {
        let enemies = crate::tactics::enemies_within_proximity(u, id, st(range));
        if !enemies.is_empty() {
            'guns: for gi in 0..guns.len() {
                if !gun::ready(u, id, gi) {
                    continue;
                }
                for &e in &enemies {
                    if gun::orient_gimble(u, id, gi, e) {
                        chosen = Some((gi, e));
                        break 'guns;
                    }
                }
            }
        }
    }
    let Some((gi, t)) = chosen else { return };
    gun::shoot(u, id, gi, Some(t));
    if u.ships.ship(id).ai_state_attack != 3 && t == target {
        let s = u.ships.ship(id);
        let moving_attack = s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| c.attributes & crate::command::attr::ATTACKING_AND_MOVING != 0);
        if s.command.is_some() && !moving_attack {
            let tw = u.tweaks.clone();
            let dt = u.phys_time_elapsed;
            let s = u.ships.ship_mut(id);
            aitrack::steady_ship(s, stat, &tw, dt);
        }
        u.ships.ship_mut(id).ai_state_attack = 3;
    }
}
