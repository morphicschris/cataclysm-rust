//! Motherships (`Mothership` module, 0x5b6150..0x5b7fb0): sMothership, bMothership,
//! trGhostship.
//!
//! * The Somtaaw mothership's big gun (with the `sMothershipBigGun` module): the player picks a
//!   point (`MothershipBigGunTargetPicked` -> the big-gun order, processed once by the command
//!   layer as `MothershipBigGunAbort`); if the gun has recharged (its `FireTime` less 11
//!   seconds) the mothership turns to the (scattered, in multiplayer) point, charges for 11
//!   seconds and fires one shell (a bullet of weapon `BIGGUN`, bullet type 0x12). The shell
//!   flies as far as the point (`maxRange`) and detonates into a shock wave whose damage falls
//!   off with the distance flown ([`big_gun_damage_falloff`]); hitting a ship or entering a
//!   DFG field detonates it early.
//! * The Somtaaw mothership's death is drawn out: `AddShipToDeleteShipList` starts a sequence
//!   (`FUN_005b7ec0`) that disables it, blows its modules a moment later and then removes it.
//! * The Beast mothership's damage modes (`MothershipUpdateDamageMode`): while a mode is on the
//!   ship burns 20 health points a frame (down to half health) for a speed, damage, research,
//!   building or rate-of-fire bonus (`MothershipGetDamageModifier1..5`).
//! * Every mothership's death sends out a shock wave ([`crate::shockwave`] type 2).
//!
//! The Beast cascade calls inside these callbacks (`beastCascadeInit/Update/Close/RemoveShip`)
//! belong to the Beast module and are not made from here. Presentation-only parts (the charge
//! effect, speech, the health warnings, the launch manager's module slots) are left out.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{ld, st};
use hwc_math::random::{RAN_ETG, RAN_GAME, RAN_PARTICLE_STREAM};
use hwc_math::Vector;

use crate::disable::Stopwatch;
use crate::object::{BulletId, CommandId, ShipId};
use crate::shockwave;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Big-gun states (+0x674).
pub mod big_gun {
    /// Idle.
    pub const IDLE: u8 = 0;
    /// Turning to the target point.
    pub const AIMING: u8 = 1;
    /// Firing (the shell leaves this frame).
    pub const FIRING: u8 = 2;
    /// Done or cancelled: levelling out, then idle.
    pub const RECOVERING: u8 = 3;
    /// Charging (11 seconds).
    pub const CHARGING: u8 = 4;
    /// Waiting for the gun to recharge (restored games).
    pub const WAITING: u8 = 5;
}

/// Bullet type of big-gun shells (and the gun static's start type of the big gun).
pub const BIG_GUN_BULLET_TYPE: u32 = 0x12;
/// The big gun must have been idle for its fire time less this many seconds; it also charges
/// for this long.
const BIG_GUN_CHARGE_TIME: f32 = 11.0;
/// Seconds the mothership turns before it charges anyway (0x8bb178).
const AIM_TIME: f32 = 20.0;
/// Cosine within which the mothership counts as aimed (0x8bb188).
const AIM_COS: f32 = 0.985;
/// `aitrackHeadingFunc` accuracies of the aim.
const AIM_ACCURACY: f32 = f32::from_bits(0x3f7f_be77);
const AIM_UP_ACCURACY: f32 = f32::from_bits(0x3e4c_cccd);
/// Target scatter as a share of the distance: multiplayer (0x8bb17c), single player
/// (0x91ce98).
const SCATTER_MP: f32 = 0.1;
const SCATTER_SP: f32 = 0.0;
/// Multiplayer shells beyond this distance fly a random 10 % further or shorter
/// (0x8bb184, 0x8bb180).
const RANGE_RANDOM_FROM: f32 = 20000.0;
const RANGE_RANDOM_SHARE: f32 = 0.1;
/// Big-gun range and the distance after which its damage falls off: single player
/// (0x8bb18c, 0x8bb190) and multiplayer (0x8bb194, 0x8bb198); the falloff floor (0x8bb19c).
const RANGE_SP: f32 = 60000.0;
const FALLOFF_FROM_SP: f32 = 35000.0;
const RANGE_MP: f32 = 90000.0;
const FALLOFF_FROM_MP: f32 = 30000.0;
const FALLOFF_MIN: f32 = 0.01;
/// The delays of the Somtaaw mothership's death: before its modules blow (0x8bb168..) and a
/// second one that is drawn but never waited for (0x8bb170..).
const DEATH_DELAY: (f32, f32) = (1.0, 2.0);
const DEATH_DELAY2: (f32, f32) = (5.0, 8.0);
/// Disable reason of a dying mothership.
const DISABLE_DYING: usize = 10;
/// `Ship.flags2` bit the dying mothership sets on its component modules.
const FLAGS2_MOTHERSHIP_DYING: u32 = 4;
/// Health a damage mode burns per frame.
const DAMAGE_MODE_COST: f32 = 20.0;

/// Damage modes of the Beast mothership (+0x650).
pub mod damage_mode {
    pub const OFF: u32 = 0;
    /// Speed +0.2.
    pub const SPEED: u32 = 1;
    /// Faster fire of its infection guns.
    pub const FIRE_RATE: u32 = 2;
    /// Weapon damage factor 0.15 instead of 1 (on top of the veterancy factor).
    pub const DAMAGE: u32 = 3;
    /// Research twice as fast.
    pub const RESEARCH: u32 = 4;
    /// Building twice as fast.
    pub const BUILD: u32 = 5;
}

/// Per-ship state (+0x650..+0x6e0).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MothershipSpec {
    /// +0x650: Beast damage mode chosen, +0x654: it is burning health now.
    pub damage_mode: u32,
    pub damage_mode_active: bool,
    /// +0x658: where the big gun aims.
    pub target: Vector,
    /// +0x664: where the mothership was when it started charging.
    pub fire_position: Vector,
    /// +0x670: the last shell fired.
    pub shell: Option<BulletId>,
    /// +0x674: see [`big_gun`].
    pub state: u8,
    /// +0x678: when the charge started.
    pub charge_start: f32,
    /// +0x684: the gun's charge animation (1 charging, 2 cancelled, 0 off).
    pub charge_anim: u8,
    /// +0x688 / +0x68c: the aim timeout.
    pub aim_timer_on: bool,
    pub aim_timer: Stopwatch,
    /// +0x6d4: the death sequence (0 alive, 1 dying, 2 exploding), +0x6d8 when the modules
    /// blow, +0x6dc the unused second delay, +0x6e0 the bullet type that killed it.
    pub death_state: u8,
    pub death_time: f32,
    pub death_time2: f32,
    pub killed_by: i32,
}

/// Per-type statics (`MothershipStaticInit` 0x5b6150): the per-tactic bullet ranges and 90 %
/// of the minimum ranges. (Beast motherships also load the cascade weapon script there; that
/// is the Beast module's.)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MothershipStatics {
    pub range: [f32; 3],
    pub min_range: [f32; 3],
}

impl MothershipStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for t in 0..3 {
            s.range[t] = stat.bullet_range[t];
            s.min_range[t] = st(ld(stat.min_bullet_range[t]) * ld(0.9f32));
        }
        s
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&MothershipSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Mothership(m) => Some(m),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut MothershipSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Mothership(m) => Some(m),
        _ => None,
    }
}

/// The big-gun state of a ship (0 for anything but a mothership).
pub fn state(u: &Universe, id: ShipId) -> u8 {
    spec(u, id).map_or(big_gun::IDLE, |m| m.state)
}

/// `MothershipInit` (0x5b6260).
pub fn init(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let mut m = MothershipSpec::default();
    m.aim_timer.start(now, f32::from_bits(0x3c23_d70a));
    let s = u.ships.ship_mut(id);
    if s.ship_type == ShipType::bMothership {
        // The Beast mothership flickers in (its arrival).
        let t = st(ld(now) + 1.0);
        s.flicker = crate::disable::Flicker { state: 1, next: t, end: st(ld(now) + 1.0 + 2.5), delay: -1.0 };
    }
    s.spec = ShipSpec::Mothership(m);
}

/// `MothershipGetBigGun` (0x5b61b0): the big gun (the first gun whose bullet type is 0x12).
pub fn big_gun_index(u: &Universe, id: ShipId) -> Option<usize> {
    let s = u.ships.get(id)?;
    let guns = u.statics.ship(s.ship_type)?.guns.as_ref()?;
    guns.iter().position(|gs| crate::gun::static_bullet_type(gs, false) == BIG_GUN_BULLET_TYPE)
}

/// The fire time of the big gun and when it last fired.
fn big_gun_timing(u: &Universe, id: ShipId) -> Option<(f32, f32)> {
    let gi = big_gun_index(u, id)?;
    let s = u.ships.get(id)?;
    let gs = u.statics.ship(s.ship_type)?.guns.as_ref()?.get(gi)?;
    Some((gs.fire_time, s.guns.get(gi)?.last_time_fired))
}

/// The big gun has recharged (`FireTime - 11 < now - lastFired`).
fn big_gun_recharged(u: &Universe, id: ShipId) -> bool {
    big_gun_timing(u, id).is_some_and(|(ft, last)| ld(ft) - ld(BIG_GUN_CHARGE_TIME) < ld(u.total_time_elapsed) - ld(last))
}

/// `MothershipBigGunTimerExpired` (0x5b6200).
fn aim_timer_expired(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|m| m.aim_timer_on && m.aim_timer.expired(u.total_time_elapsed))
}

/// `MothershipStartBigGunTimer` (0x5b6230).
fn start_aim_timer(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    if let Some(m) = spec_mut(u, id) {
        if !m.aim_timer_on {
            m.aim_timer_on = true;
            m.aim_timer.start(now, AIM_TIME);
        }
    }
}

/// `MothershipHasBigGunModule` (0x5b7fb0): a Somtaaw mothership with a finished big-gun
/// module clamped to it.
pub fn has_big_gun_module(u: &Universe, id: ShipId) -> bool {
    if u.ships.get(id).is_none_or(|s| s.ship_type != ShipType::sMothership) {
        return false;
    }
    crate::clamp::slaves_of(u, id).iter().any(|&m| {
        u.ships.get(m).is_some_and(|m| m.ship_type == ShipType::sMothershipBigGun && m.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION == 0)
    })
}

/// `MothershipBigGunIsActive` (0x5b6980): the big gun is doing something.
pub fn big_gun_is_active(u: &Universe, id: ShipId) -> bool {
    has_big_gun_module(u, id) && state(u, id) != big_gun::IDLE
}

/// `MothershipBigGunIsCharging` (0x5b69f0): active and not winding down.
pub fn big_gun_is_charging(u: &Universe, id: ShipId) -> bool {
    has_big_gun_module(u, id) && !matches!(state(u, id), big_gun::IDLE | big_gun::RECOVERING)
}

/// `MothershipGetBigGunChargeFraction` (0x5b6550): how far the big gun has recharged.
pub fn big_gun_charge_fraction(u: &Universe, id: ShipId) -> f64 {
    let Some((ft, last)) = big_gun_timing(u, id) else { return 0.0 };
    let f = (ld(u.total_time_elapsed) - ld(last)) / (ld(ft) - ld(BIG_GUN_CHARGE_TIME));
    if f > 1.0 {
        1.0
    } else {
        f
    }
}

/// `MothershipGetBigGunRange` (0x5b7730).
pub fn big_gun_range(u: &Universe) -> f32 {
    if u.single_player {
        RANGE_SP
    } else {
        RANGE_MP
    }
}

/// `MothershipBigGunDamageFalloff` (0x5b7750): full damage up to the falloff distance, then
/// linearly less to the gun's range, never below 1 %.
pub fn big_gun_damage_falloff(u: &Universe, dist: f32) -> f32 {
    let (range, from) = if u.single_player { (RANGE_SP, FALLOFF_FROM_SP) } else { (RANGE_MP, FALLOFF_FROM_MP) };
    let mut f = 1.0;
    if from < dist {
        let span = st(ld(range) - ld(from));
        let mut d = st(ld(dist) - ld(from));
        if span < d {
            d = span;
        }
        if 0.0 < span {
            f = st(1.0 - ld(d) / ld(span));
            if f < 0.0 {
                f = 0.0;
            }
        }
    }
    if f < FALLOFF_MIN {
        f = FALLOFF_MIN;
    }
    f
}

/// `MothershipBigGunScatterTarget` (0x5b6780): the point a shell aims at, scattered by a share
/// of the distance in multiplayer games.
pub fn scatter_target(u: &mut Universe, id: ShipId, point: Vector) -> Vector {
    let pos = u.ships.ship(id).posinfo.position;
    let d = Vector::distance_ext(pos, point);
    let share = if u.single_player { SCATTER_SP } else { SCATTER_MP };
    let spread = ld(share) * d;
    if spread <= 0.0 {
        return point;
    }
    let lo = st(-spread);
    let span = st(spread - ld(lo));
    let mut r = || st(f64::from(u.randy.random(RAN_ETG)) * ld(span) * ld(2.328_306_4e-10) + ld(lo));
    let (dx, dy) = (r(), r());
    let dz = r();
    Vector::new(st(ld(dx) + ld(point.x)), st(ld(dy) + ld(point.y)), st(ld(dz) + ld(point.z)))
}

/// The big-gun order (`COMMAND_BIGGUN`, 0xe).
pub const ORDER_BIG_GUN: u16 = 0xe;

/// `MothershipBigGunTargetPicked` (0x5b6700) / the big-gun order (`FUN_00449810`): the
/// selected Somtaaw mothership gets a big-gun order at `point`.
pub fn cl_big_gun(u: &mut Universe, ships: &[ShipId], point: Vector) {
    let sel: Vec<ShipId> = ships.iter().copied().filter(|&s| u.ships.get(s).is_some_and(|x| x.ship_type == ShipType::sMothership)).collect();
    let Some(&ms) = sel.last() else { return };
    let sel = vec![ms];
    crate::command::remove_ships_from_doing_stuff(u, &sel);
    let mut c = crate::command::new_command_pub(sel.clone());
    c.order = ORDER_BIG_GUN;
    // The order's point (+0x2dc) rides in the waypoint list.
    c.waypoints = vec![point];
    let cid = u.commands.add_pub(c);
    for &s in &sel {
        let sh = u.ships.ship_mut(s);
        sh.command = Some(cid);
        if sh.special_flags & crate::object::special::ROW_GETTING_OUT_OF_WAY != 0 {
            crate::aiship::row_remove_ship(sh);
        }
    }
}

/// `MothershipBigGunAbort` (0x5b68a0), run once for a big-gun order by `clProcess`: when the
/// gun has recharged, take the (scattered) point and start aiming.
pub fn big_gun_order(u: &mut Universe, cid: CommandId) {
    let Some(c) = u.commands.get(cid) else { return };
    let (Some(&ms), Some(&point)) = (c.selection.first(), c.waypoints.first()) else { return };
    if u.ships.get(ms).is_none() || !big_gun_recharged(u, ms) {
        return;
    }
    let target = scatter_target(u, ms, point);
    if let Some(m) = spec_mut(u, ms) {
        m.target = target;
        m.state = big_gun::AIMING;
    }
}

/// `MothershipSpecialActivate` (0x5b6a70): the special key stands the big gun down.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    if has_big_gun_module(u, id) {
        if let Some(m) = spec_mut(u, id) {
            if m.state != big_gun::IDLE {
                m.state = big_gun::IDLE;
            }
        }
    }
    true
}

/// `MothershipBigGunBeginTargeting` (0x5b6ae0) without its interface part: whether the player
/// may pick a big-gun target now (a finished module and a recharged gun).
pub fn can_begin_targeting(u: &Universe, id: ShipId) -> bool {
    has_big_gun_module(u, id) && big_gun_recharged(u, id)
}

/// Unit vector from the mothership to its big-gun target.
fn to_target(u: &Universe, id: ShipId) -> Vector {
    let s = u.ships.ship(id);
    let t = spec(u, id).map_or(Vector::ZERO, |m| m.target);
    let p = s.posinfo.position;
    let mut d = Vector::new(st(ld(t.x) - ld(p.x)), st(ld(t.y) - ld(p.y)), st(ld(t.z) - ld(p.z)));
    d.normalize();
    d
}

/// `MothershipBigGunIsAimed` (0x5b6c20).
pub fn big_gun_is_aimed(u: &Universe, id: ShipId) -> bool {
    let d = to_target(u, id);
    let mut h = u.ships.ship(id).heading();
    h.normalize();
    ld(AIM_COS) < (ld(d.x) * ld(h.x) + ld(d.y) * ld(h.y)) + ld(d.z) * ld(h.z)
}

fn turn_to(u: &mut Universe, id: ShipId, desired: &mut Vector) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    crate::aitrack::heading(s, stat, &tw, dt, desired, AIM_ACCURACY, AIM_UP_ACCURACY, 0.0, 0)
}

fn stabilize(u: &mut Universe, id: ShipId) -> bool {
    let statics = u.statics.clone();
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let s = u.ships.ship_mut(id);
    let Some(stat) = statics.ship(s.ship_type) else { return false };
    crate::aitrack::stabilize(s, stat, &tw, dt)
}

/// `MothershipBigGunAim` (0x5b6cc0): keep turning to the target; true when on it or the aim
/// timeout has run out.
fn big_gun_aim(u: &mut Universe, id: ShipId) -> bool {
    let mut d = to_target(u, id);
    u.ships.ship_mut(id).autostabilize_ship = false;
    if big_gun_is_aimed(u, id) {
        start_aim_timer(u, id);
    }
    aim_timer_expired(u, id) || turn_to(u, id, &mut d)
}

/// `MothershipFireBigGun` (0x5b7b20): the end of the charge (11 seconds): the shell goes out
/// next frame. (The rest moves the charge effect with the ship.)
fn fire_big_gun(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let Some(m) = spec_mut(u, id) else { return };
    if m.state == big_gun::CHARGING && ld(BIG_GUN_CHARGE_TIME) < ld(now) - ld(m.charge_start) {
        m.state = big_gun::FIRING;
        m.charge_anim = 0;
    }
}

/// `MothershipUpdateBigGunEffects` (0x5b7a60): charge if the module is still there.
fn start_charge(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let has = has_big_gun_module(u, id);
    let Some(m) = spec_mut(u, id) else { return };
    if has {
        m.state = big_gun::CHARGING;
        m.charge_anim = 1;
        m.charge_start = now;
        u.ships.ship_mut(id).autostabilize_ship = false;
    } else {
        m.state = big_gun::RECOVERING;
    }
}

/// The shell: the big gun fires with no target; it flies as far as the target point (in
/// multiplayer up to 10 % more or less beyond 20 km).
fn launch_shell(u: &mut Universe, id: ShipId) {
    let Some(gi) = big_gun_index(u, id) else { return };
    let mut d = to_target(u, id);
    u.ships.ship_mut(id).autostabilize_ship = false;
    if big_gun_is_aimed(u, id) {
        let on = spec(u, id).is_some_and(|m| m.aim_timer_on);
        if !on {
            start_aim_timer(u, id);
        }
    }
    if !aim_timer_expired(u, id) {
        turn_to(u, id, &mut d);
    }
    let before = u.bullet_list.len();
    let fired = crate::gun::shoot(u, id, gi, None);
    let shell = if fired && u.bullet_list.len() > before { u.bullet_list.last().copied() } else { None };
    if let Some(m) = spec_mut(u, id) {
        m.shell = shell;
    }
    let Some(b) = shell else { return };
    shockwave::add_special(u, b);
    // (RKOPickSafeLocation2 tells a single-player dRKO where to flee; not modelled.)
    let target = spec(u, id).map_or(Vector::ZERO, |m| m.target);
    let bp = u.bullets.bullet(b).posinfo.position;
    let (dx, dy, dz) = (ld(target.x) - ld(bp.x), ld(target.y) - ld(bp.y), ld(target.z) - ld(bp.z));
    let d_ext = fsqrt((dz * dz + dy * dy) + dx * dx);
    let dist = st(d_ext);
    let range = if !u.single_player && ld(RANGE_RANDOM_FROM) < ld(dist) {
        let a = st(ld(RANGE_RANDOM_SHARE) * ld(dist));
        let lo = st(-ld(a));
        let r = u.randy.random(RAN_ETG);
        st((ld(a) - ld(lo)) * f64::from(r) * ld(2.328_306_4e-10) + ld(lo) + ld(dist))
    } else {
        st(d_ext)
    };
    let bm = u.bullets.bullet_mut(b);
    bm.dist_flown = 0.0;
    bm.max_range = range;
    if let Some(m) = spec_mut(u, id) {
        m.state = big_gun::RECOVERING;
    }
    u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: super::Effect::BigGunFired });
}

/// The big-gun part of `MothershipHouseKeep`.
fn big_gun_housekeep(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let Some(m) = spec_mut(u, id) else { return };
    if matches!(m.state, big_gun::IDLE | big_gun::RECOVERING) {
        m.aim_timer_on = false;
        m.aim_timer.start(now, f32::from_bits(0x3c23_d70a));
    }
    let state = m.state;
    match state {
        big_gun::AIMING => {
            let mut d = to_target(u, id);
            u.ships.ship_mut(id).autostabilize_ship = false;
            if big_gun_is_aimed(u, id) {
                start_aim_timer(u, id);
            }
            if aim_timer_expired(u, id) || turn_to(u, id, &mut d) {
                let p = u.ships.ship(id).posinfo.position;
                if let Some(m) = spec_mut(u, id) {
                    m.fire_position = p;
                }
                if !has_big_gun_module(u, id) {
                    spec_mut(u, id).expect("spec").state = big_gun::RECOVERING;
                } else {
                    let m = spec_mut(u, id).expect("spec");
                    m.state = big_gun::CHARGING;
                    m.charge_anim = 1;
                    m.charge_start = now;
                    u.ships.ship_mut(id).autostabilize_ship = false;
                }
            }
        }
        big_gun::FIRING => launch_shell(u, id),
        big_gun::WAITING => {
            big_gun_aim(u, id);
            if big_gun_recharged(u, id) {
                start_charge(u, id);
            }
        }
        big_gun::CHARGING => {
            big_gun_aim(u, id);
            fire_big_gun(u, id);
        }
        big_gun::RECOVERING => {
            fire_big_gun(u, id);
            if stabilize(u, id) {
                spec_mut(u, id).expect("spec").state = big_gun::IDLE;
            }
        }
        big_gun::IDLE => fire_big_gun(u, id),
        _ => spec_mut(u, id).expect("spec").state = big_gun::RECOVERING,
    }
}

/// `MothershipHouseKeep` (0x5b6fd0): the death sequence, the big gun (Somtaaw) or the damage
/// mode (Beast).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let Some(m) = spec(u, id).cloned() else { return };
    match m.death_state {
        1 => {
            if m.death_time < now {
                for sid in crate::clamp::slaves_of(u, id) {
                    let is_component = u.ships.get(sid).and_then(|s| u.statics.ship(s.ship_type)).and_then(|st| st.ship_class) == Some(crate::statics::ShipClass::Component);
                    if is_component {
                        u.ships.ship_mut(sid).flags2 |= FLAGS2_MOTHERSHIP_DYING;
                    }
                }
                let r = u.randy.random(RAN_PARTICLE_STREAM);
                let m = spec_mut(u, id).expect("spec");
                m.death_state = 2;
                let (lo, hi) = DEATH_DELAY2;
                m.death_time2 = st(f64::from(r) * ((ld(hi) - ld(lo)) * ld(2.328_306_4e-10)) + ld(lo) + ld(now));
            }
            return;
        }
        2 => {
            crate::damage::finish_ship_death(u, id, m.killed_by);
            return;
        }
        3 => return,
        _ => {}
    }
    let t = u.ships.ship(id).ship_type;
    if t == ShipType::sMothership {
        big_gun_housekeep(u, id);
    } else if t == ShipType::bMothership {
        update_damage_mode(u, id);
    }
    // (The low-health warnings of the local player's mothership are interface only.)
}

/// `FUN_005b7ec0`, from `AddShipToDeleteShipList` for the Somtaaw mothership: it does not die
/// at once; it is disabled and its modules blow a second or two later. False when the ship
/// has no mothership state (it then dies as usual).
pub fn start_dying(u: &mut Universe, id: ShipId, bullet_type: i32) -> bool {
    let now = u.total_time_elapsed;
    let Some(state) = spec(u, id).map(|m| m.death_state) else { return false };
    if state != 0 {
        return true;
    }
    let (lo, hi) = DEATH_DELAY;
    let t = st(u.randy.frandom_between_ext(RAN_GAME, lo, hi) + ld(now));
    if let Some(m) = spec_mut(u, id) {
        m.death_state = 1;
        m.killed_by = bullet_type;
        m.death_time = t;
    }
    crate::base::set_disable_reason(u.ships.ship_mut(id), true, DISABLE_DYING);
    crate::salvage::depot_died(u, id);
    true
}

/// Whether the Somtaaw mothership is in its death sequence.
pub fn is_dying(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|m| m.death_state != 0)
}

/// `MothershipUpdateDamageMode` (0x5b6e00).
pub fn update_damage_mode(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let statics = u.statics.clone();
    let Some(mode) = spec(u, id).map(|m| m.damage_mode) else { return };
    if mode == damage_mode::OFF {
        spec_mut(u, id).expect("spec").damage_mode_active = false;
        return;
    }
    let s = u.ships.ship(id);
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let half = st(ld(stat.health.max_health) * 0.5);
    if s.health <= half {
        spec_mut(u, id).expect("spec").damage_mode_active = false;
        // (lmBeastSystemsReset: the interface resets the mode buttons.)
        return;
    }
    spec_mut(u, id).expect("spec").damage_mode_active = true;
    let s = u.ships.ship_mut(id);
    let h = st(ld(s.health) - ld(DAMAGE_MODE_COST));
    s.health = h;
    if h < half {
        s.health = half;
    }
    if mode == damage_mode::FIRE_RATE {
        let Some(guns) = stat.guns.as_ref() else { return };
        for (g, gs) in s.guns.iter_mut().zip(guns.iter()) {
            if crate::gun::static_bullet_type(gs, false) == 0x13 && ld(now) - ld(g.last_time_fired) < ld(gs.fire_time) {
                let l = st(ld(g.last_time_fired) - ld(0.1f32));
                g.last_time_fired = if l < 0.0 { 0.0 } else { l };
            }
        }
    }
}

/// `clProcessSyncPacket`'s damage-mode command (`FUN_00449bc0`): choose a Beast damage mode.
pub fn set_damage_mode(u: &mut Universe, id: ShipId, mode: u32) {
    if let Some(m) = spec_mut(u, id) {
        m.damage_mode = mode;
    }
}

fn mode_active(u: &Universe, id: ShipId, mode: u32) -> bool {
    spec(u, id).is_some_and(|m| m.damage_mode_active && m.damage_mode == mode)
}

/// `MothershipGetDamageModifier1` (0x5b6f00): speed bonus.
pub fn damage_modifier_speed(u: &Universe, id: ShipId) -> f32 {
    if mode_active(u, id, damage_mode::SPEED) {
        0.2
    } else {
        0.0
    }
}

/// [`damage_modifier_speed`] from the ship alone (for code that has no universe).
pub fn speed_modifier_of(s: &crate::object::Ship) -> f32 {
    match &s.spec {
        ShipSpec::Mothership(m) if m.damage_mode_active && m.damage_mode == damage_mode::SPEED => 0.2,
        _ => 0.0,
    }
}

/// `MothershipGetDamageModifier3` (0x5b6f30): the weapon factor added to the veterancy
/// factor (1, or 0.15 in damage mode).
pub fn damage_modifier_damage(s: &crate::object::Ship) -> f32 {
    match &s.spec {
        ShipSpec::Mothership(m) if m.damage_mode_active && m.damage_mode == damage_mode::DAMAGE => 0.15,
        _ => 1.0,
    }
}

/// `MothershipGetDamageModifier4` (0x5b6f60): research speed.
pub fn damage_modifier_research(u: &Universe, id: ShipId) -> f32 {
    if mode_active(u, id, damage_mode::RESEARCH) {
        2.0
    } else {
        1.0
    }
}

/// `MothershipGetDamageModifier5` (0x5b6f90): build rate.
pub fn damage_modifier_build(u: &Universe, id: ShipId) -> i32 {
    if mode_active(u, id, damage_mode::BUILD) {
        2
    } else {
        1
    }
}

/// `MothershipIsDamageModeActive` (0x5b6fc0).
pub fn is_damage_mode_active(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|m| m.damage_mode_active)
}

/// `MothershipAttack` (0x5b6670): an attack order stands the big gun down, then fires what
/// bears (`attackSimple`).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    cancel_big_gun(u, id);
    crate::attack::attack_simple(u, id, target);
}

/// `MothershipCancelBigGun` (0x5b65f0) and `MothershipCancelBigGun2` (0x5b7680).
pub fn cancel_big_gun(u: &mut Universe, id: ShipId) {
    if u.ships.get(id).is_none_or(|s| s.ship_type != ShipType::sMothership) {
        return;
    }
    let Some(m) = spec_mut(u, id) else { return };
    if matches!(m.state, big_gun::RECOVERING | big_gun::IDLE) {
        return;
    }
    if m.state == big_gun::CHARGING {
        m.charge_anim = 2;
    }
    m.state = big_gun::RECOVERING;
    if stabilize(u, id) {
        spec_mut(u, id).expect("spec").state = big_gun::IDLE;
    }
}

/// `MothershipDied` (0x5b63e0): the death wave; modules still being built blow a second later.
pub fn died(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    let (pos, player) = (s.posinfo.position, s.player);
    shockwave::create(u, shockwave::kind::MOTHERSHIP_DEATH, pos, player);
    for sid in crate::clamp::slaves_of(u, id) {
        if let Some(m) = u.ships.get_mut(sid) {
            if m.special_flags2 & crate::base::special2::UNDER_CONSTRUCTION != 0 {
                m.death_time = st(ld(now) + 1.0);
            }
        }
    }
}

/// `MothershipRemoveShipReferences` (0x5b6490): losing the big-gun module ends the big gun's
/// work.
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.ship_type != ShipType::sMothership {
        return;
    }
    let player = s.player;
    if u.ships.get(gone).is_some_and(|g| g.ship_type == ShipType::sMothershipBigGun && g.player == player) {
        if let Some(m) = spec_mut(u, id) {
            m.state = big_gun::RECOVERING;
        }
    }
}

/// `MothershipRemoveBigGunTarget` (0x5b7700): drops the charge effect when it goes (effects
/// are not modelled).
pub fn remove_big_gun_target(u: &mut Universe, id: ShipId, gone: ShipId) {
    let _ = (u, id, gone);
}

/// `MothershipBigGunBulletHit` (0x5b7820) / `MothershipBigGunBulletHit2` (0x5b7940): a shell
/// detonates at `at`: a shock wave scaled by the distance it flew.
pub fn big_gun_bullet_hit(u: &mut Universe, b: BulletId, at: Vector) {
    let Some(bl) = u.bullets.get(b) else { return };
    let (flown, player) = (bl.dist_flown, bl.player);
    shockwave::remove_special(u, b);
    let f = big_gun_damage_falloff(u, flown);
    shockwave::create_custom(u, at, player, f);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn falloff() {
        let statics = std::sync::Arc::new(crate::statics::test_world(Vec::new()));
        let mut u = Universe::new(statics, crate::SimTweaks::compiled(), &[crate::statics::ShipRace::Somtaaw]);
        u.single_player = true;
        assert_eq!(big_gun_damage_falloff(&u, 10000.0), 1.0);
        assert_eq!(big_gun_damage_falloff(&u, 47500.0), 0.5);
        assert_eq!(big_gun_damage_falloff(&u, 80000.0), 0.01);
        u.single_player = false;
        assert_eq!(big_gun_damage_falloff(&u, 60000.0), 0.5);
        assert_eq!(big_gun_range(&u), 90000.0);
    }
}
