//! `ConsMgr.c`: building ships. Every ship that can build (motherships, carriers) is a factory
//! with one job per ship type (`shipinprogress`, 0x30 bytes): ships queued, build time, the
//! fixed-point cost paid so far. Each universe update the build task charges the player's
//! resource units a little and counts the time down (`cmBuildTaskFunction`); a finished ship is
//! created inside its factory and launched (`clCreateShip`). Ships with `externalBuild`
//! (modules, destroyers, support units) are built as a hull clamped to the factory whose health
//! grows with the job's progress.
//!
//! Also here: support units (`supportUnits` cost, `providesSU` capacity) and the unit caps.

use hwc_math::fpu::{fild_i32, ld, st};

use crate::base::{self, special, special2, BaseEvent, CreateOptions};
use crate::object::ShipId;
use crate::statics::{ShipClass, ShipRace, ShipType};
use crate::universe::Universe;

/// One job of a factory (`shipinprogress`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct BuildJob {
    /// [0] ships queued.
    pub n_jobs: i32,
    /// [1] ships queued in total (for the GUI).
    pub n_total: i32,
    /// [2] build time of one ship (frames).
    pub time_total: i32,
    /// [3] time left on the current ship.
    pub time_left: i32,
    /// [4] fractional cost carried between frames (16.16 fixed point).
    pub cost_frac: i32,
    /// [6] paused.
    pub paused: bool,
    /// [7] resource units paid so far.
    pub cost_so_far: i32,
    /// [8] the type (its statics).
    pub info: Option<ShipType>,
    /// [9] (+0x24) the external-construction hull is clamped in place.
    pub hull_launched: bool,
    /// [10] (+0x28) an external-construction hull exists.
    pub hull_exists: bool,
    /// [11] (+0x2c) that hull.
    pub hull: Option<ShipId>,
}

/// A factory (`shipsinprogress`, 0x1b48 bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct Factory {
    /// +0x10: the building ship.
    pub ship: Option<ShipId>,
    /// +0x14: can build big ships.
    pub big: bool,
    /// +0x18: one job per ship type.
    pub jobs: Vec<BuildJob>,
}

/// `cmAddFactory` (0x44fb10).
pub fn add_factory(u: &mut Universe, ship: ShipId, big: bool) {
    if u.single_player && u.ships.ship(ship).base.disabled {
        return;
    }
    let class = base::class_of(u, u.ships.ship(ship).ship_type);
    if class == Some(ShipClass::Carrier) {
        let player = u.ships.ship(ship).player;
        u.base.carriers.push((ship, player));
    }
    u.base.factories.push(Factory { ship: Some(ship), big, jobs: vec![BuildJob::default(); base::NUM_SHIP_TYPES] });
}

/// `cmRemoveFactory` (0x44fef0): the factory (and its jobs) go; a carrier gives up its
/// auto-launch bit.
pub fn remove_factory(u: &mut Universe, ship: ShipId) {
    if let Some(i) = u.base.factories.iter().position(|f| f.ship == Some(ship)) {
        u.base.factories.remove(i);
    }
    if let Some(i) = u.base.carriers.iter().position(|&(s, _)| s == ship) {
        let bit = crate::dock::auto_launch_bit(u, ship);
        let player = u.ships.ship(ship).player;
        let al = u.base.players[player].auto_launch;
        // The bits above the carrier's move down one place.
        let low = bit.wrapping_sub(1);
        u.base.players[player].auto_launch = (!(low | bit) & al) >> 1 | al & low;
        u.base.carriers.remove(i);
    }
}

fn factory_index(u: &Universe, ship: ShipId) -> Option<usize> {
    u.base.factories.iter().position(|f| f.ship == Some(ship))
}

/// The SP build-time divisor and cost divisor (`spSkillLevelModifiersGet`, 0x88a0d0/0x88a0d4).
pub fn skill_divisors(u: &Universe) -> (i32, i32) {
    match u.sp_difficulty {
        0 => (4, 4),
        1 => (2, 2),
        _ => (1, 1),
    }
}

fn build_time(u: &Universe, t: ShipType) -> i32 {
    let bt = u.statics.ship(t).map_or(0, |s| s.build_time);
    if u.single_player {
        bt / skill_divisors(u).0
    } else {
        bt
    }
}

/// `cmBuildJobsAdd` (0x453d30): queue `count` ships of type `t` at a factory.
pub fn build_jobs_add(u: &mut Universe, factory: ShipId, t: ShipType, count: i32) {
    let time = build_time(u, t);
    let Some(fi) = factory_index(u, factory) else { return };
    let job = &mut u.base.factories[fi].jobs[t.index()];
    if job.n_jobs == 0 {
        job.time_total = time;
        job.time_left = time;
        job.cost_frac = 0;
        job.paused = false;
        job.cost_so_far = 0;
    }
    job.info = Some(t);
    job.n_jobs += count;
    job.n_total += count;
}

/// The build-manager tables (0x7499e0..): `(flag, type)`, flag 0 = class header, 1 = buildable,
/// 2 = buildable once acquired (Beast). Small factories / big factories per race.
pub fn build_table(race: ShipRace, big: bool) -> &'static [(u8, u8)] {
    const S_SMALL: &[(u8, u8)] = &[(0, 6), (1, 0), (1, 1), (1, 133), (1, 2), (0, 5), (1, 4), (1, 5), (0, 4), (1, 6), (1, 7), (1, 8), (0, 11), (1, 3), (1, 17), (0, 8), (1, 14), (0, 10), (1, 13)];
    const S_BIG: &[(u8, u8)] = &[
        (0, 6), (1, 0), (1, 1), (1, 133), (1, 2), (0, 5), (1, 4), (1, 5), (0, 4), (1, 6), (1, 7), (1, 8), (0, 3), (1, 25), (1, 10),
        (1, 12), (0, 11), (1, 3), (1, 17), (0, 8), (1, 14), (1, 16), (0, 10), (1, 26), (1, 24), (1, 22), (1, 19), (1, 21), (1, 23),
        (1, 31),
    ];
    const B_SMALL: &[(u8, u8)] = &[
        (0, 6), (1, 33), (1, 35), (1, 36), (2, 34), (1, 37), (0, 5), (1, 39), (1, 40), (1, 59), (1, 58), (2, 38), (0, 4), (1, 44),
        (1, 45), (2, 48), (2, 41), (2, 42), (0, 11), (2, 52), (0, 8), (1, 49),
    ];
    const B_BIG: &[(u8, u8)] = &[
        (0, 6), (1, 33), (1, 35), (1, 36), (2, 34), (1, 37), (0, 5), (1, 39), (1, 40), (1, 59), (1, 58), (2, 38), (0, 4), (1, 44),
        (1, 45), (2, 48), (2, 41), (2, 42), (0, 3), (1, 47), (1, 46), (0, 11), (2, 52), (0, 8), (1, 49), (1, 51), (0, 10), (1, 57),
    ];
    const K_SMALL: &[(u8, u8)] = &[(0, 6), (1, 67), (1, 68), (1, 61), (0, 5), (1, 65), (1, 72), (1, 70), (0, 4), (1, 60), (1, 69), (0, 8), (1, 64), (1, 63), (1, 71)];
    const K_BIG: &[(u8, u8)] = &[
        (0, 6), (1, 67), (1, 68), (1, 61), (0, 5), (1, 65), (1, 72), (1, 70), (0, 4), (1, 60), (1, 69), (0, 3), (1, 73), (1, 66),
        (1, 62), (0, 8), (1, 71), (1, 64), (1, 63),
    ];
    const T_SMALL: &[(u8, u8)] = &[(0, 6), (1, 81), (1, 82), (1, 75), (0, 5), (1, 89), (1, 84), (1, 86), (0, 4), (1, 91), (1, 74), (1, 78), (1, 83), (0, 8), (1, 79), (1, 77), (1, 87)];
    const T_BIG: &[(u8, u8)] = &[
        (0, 6), (1, 81), (1, 82), (1, 75), (0, 5), (1, 89), (1, 84), (1, 86), (0, 4), (1, 91), (1, 74), (1, 78), (1, 83), (0, 3),
        (1, 90), (1, 85), (1, 80), (1, 76), (0, 8), (1, 87), (1, 79), (1, 77),
    ];
    const P1: &[(u8, u8)] = &[(0, 6), (1, 93), (0, 5), (1, 97), (1, 95), (0, 4), (1, 94)];
    match race {
        ShipRace::Somtaaw => {
            if big {
                S_BIG
            } else {
                S_SMALL
            }
        }
        ShipRace::Beast => {
            if big {
                B_BIG
            } else {
                B_SMALL
            }
        }
        ShipRace::Kushan => {
            if big {
                K_BIG
            } else {
                K_SMALL
            }
        }
        ShipRace::Taiidan => {
            if big {
                T_BIG
            } else {
                T_SMALL
            }
        }
        ShipRace::Turanic => P1,
        _ => &[],
    }
}

/// `cmCanBuildShipType` (0x4534c0): the factory's table lists the type, an externally built
/// type has a free build point (and is built only by a mothership), and with `check_research`
/// the player has the technologies (and, for acquired Beast types, has acquired it).
pub fn can_build_ship_type(u: &Universe, factory: ShipId, t: ShipType, check_research: bool) -> bool {
    let Some(f) = u.ships.get(factory) else { return false };
    let Some(fstat) = u.statics.ship(f.ship_type) else { return false };
    let table = build_table(fstat.ship_race, fstat.can_build_big_ships);
    let Some(&(flag, _)) = table.iter().find(|&&(flag, ty)| flag != 0 && ty as usize == t.index()) else { return false };
    let Some(stat) = u.statics.ship(t) else { return false };
    if stat.external_build {
        if fstat.ship_class != Some(ShipClass::Mothership) {
            return false;
        }
        if !crate::dock::has_free_build_dock_point_flags(u, factory, t, 1, 0) {
            return false;
        }
    }
    if check_research {
        let at_mothership = f.ship_type == ShipType::sMothership;
        if !crate::research::can_build_ship(u, f.player, t, at_mothership) {
            return false;
        }
        if flag == 2 && !u.base.players[f.player].acquired[t.index()] {
            return false;
        }
    }
    true
}

/// Queue ships the way the build manager does: checks the type can be built here, the unit
/// caps and the support units, then `cmBuildJobsAdd`. Returns how many were queued.
pub fn queue_build(u: &mut Universe, factory: ShipId, t: ShipType, count: i32) -> i32 {
    if !can_build_ship_type(u, factory, t, true) {
        return 0;
    }
    let player = u.ships.ship(factory).player;
    let mut n = 0;
    for _ in 0..count {
        if !player_has_support_units_for_type(u, t, player, n) || !unit_cap_can_create_ship(u, t, factory, n) {
            break;
        }
        n += 1;
    }
    if n > 0 {
        build_jobs_add(u, factory, t, n);
    }
    n
}

/// `cmBuildTaskFunction` (0x450160): one update of the local player's factories.
pub fn build_task(u: &mut Universe) {
    let player = u.base.cur_player;
    if player >= u.base.players.len() {
        return;
    }
    if u.base.players[player].resource_units > 299 {
        u.base.resources_were_above = true;
    }
    let mp = u.base.options.multiplayer_network;
    let sp = u.single_player;
    let (time_div, cost_div) = skill_divisors(u);
    let factories: Vec<Option<ShipId>> = u.base.factories.iter().map(|f| f.ship).collect();
    for (fi_snapshot, fship) in factories.into_iter().enumerate() {
        let Some(fship) = fship else { continue };
        let _ = fi_snapshot;
        if u.ships.get(fship).is_none_or(|s| s.player != player) {
            continue;
        }
        let mut idx = 0usize;
        while idx < base::NUM_SHIP_TYPES && u.base.players[player].resource_units > 0 {
            let Some(fi) = u.base.factories.iter().position(|f| f.ship == Some(fship)) else { break };
            let job = u.base.factories[fi].jobs[idx];
            if !job.paused {
                let info = job.info.and_then(|t| u.statics.ship(t).map(|s| (t, s.external_build, s.build_cost, s.ship_class)));
                if job.n_jobs > 0 && job.time_left > 0 {
                    if let Some((t, ext, cost, _)) = info {
                        if !ext || (job.hull_exists && job.hull_launched) {
                            let step = if u.ships.ship(fship).ship_type == ShipType::bMothership { beast_build_rate(u, fship) } else { 1 };
                            let mut ru = u.base.players[player].resource_units;
                            let mut above = u.base.resources_were_above;
                            let done = charge_frame(&mut u.base.factories[fi].jobs[idx], cost, sp, cost_div, mp, step, &mut ru, &mut above);
                            u.base.players[player].resource_units = ru;
                            u.base.resources_were_above = above;
                            let job = u.base.factories[fi].jobs[idx];
                            if done {
                                ship_build_complete(u, fship, idx, cost_div, time_div);
                            } else if !mp && ext {
                                // Progress = 1 - left / total, rounded to float.
                                let p = external_progress(job.time_left, job.time_total);
                                if let Some(h) = job.hull {
                                    update_external_construction(u, h, p, ExternalUpdate::Progress);
                                }
                            }
                            let _ = t;
                        }
                    }
                }
                // Start the hull of the next externally built ship.
                let Some(fi) = u.base.factories.iter().position(|f| f.ship == Some(fship)) else { break };
                let job = u.base.factories[fi].jobs[idx];
                if let Some((t, true, _, class)) = info {
                    if job.n_jobs > 0 && (job.time_left == job.time_total || job.time_left < 1) && !job.hull_exists {
                        let free = crate::dock::has_free_build_dock_point_flags(u, fship, t, 1, 0);
                        let allowed = class == Some(ShipClass::Component)
                            || u.ships.ship(fship).ship_type == ShipType::bMothership
                            || base::player_has_ship_of_type(u, ShipType::sMothershipDockingBay, player);
                        let time = if sp { build_time_raw(u, t) / time_div } else { build_time_raw(u, t) };
                        if !free || !allowed {
                            let j = &mut u.base.factories[fi].jobs[idx];
                            j.cost_frac = 0;
                            j.time_total = time;
                            j.time_left = time;
                        } else {
                            let mut hull = None;
                            if !mp {
                                let race = u.players[player].race;
                                hull = cl_create_ship(u, t, race, player, fship);
                            }
                            let Some(fi) = u.base.factories.iter().position(|f| f.ship == Some(fship)) else { break };
                            let j = &mut u.base.factories[fi].jobs[idx];
                            j.hull = hull;
                            j.cost_frac = 0;
                            j.time_total = time;
                            j.time_left = time;
                            if !mp {
                                j.hull_exists = true;
                                j.hull_launched = false;
                            }
                        }
                    }
                }
            }
            idx += 1;
        }
    }
}

/// One frame of a build job (`cmBuildTaskFunction`): the 16.16 fixed-point share of the cost
/// (`buildCost << 16 / time`, in single player `<< (17 - divisor)`) is charged, the fraction
/// carried, the time counted down. On the last frame a carried fraction above one half costs one
/// more unit. True when the ship is complete.
#[allow(clippy::too_many_arguments)]
pub fn charge_frame(job: &mut BuildJob, cost: i32, sp: bool, cost_div: i32, mp: bool, step: i32, ru: &mut i32, above: &mut bool) -> bool {
    let shift = if sp { (0x11 - cost_div) as u32 & 0x1f } else { 16 };
    let total = job.time_total.max(1);
    let dec = (cost.wrapping_shl(shift)).wrapping_div(total).wrapping_add(job.cost_frac);
    job.cost_frac = dec & 0xffff;
    let dec = dec >> 16;
    job.cost_so_far += dec;
    if !mp {
        *ru -= dec;
    }
    if *ru < 1 {
        *above = false;
        if !mp {
            *ru = 0;
        }
    }
    job.time_left -= step;
    if job.time_left < 1 {
        if job.cost_frac > 0x7fff {
            if !mp {
                *ru -= 1;
            }
            if *ru < 1 {
                *above = false;
                if !mp {
                    *ru = 0;
                }
            }
        }
        return true;
    }
    false
}

/// The build progress of an external-construction hull: `1 - left / total`.
pub fn external_progress(left: i32, total: i32) -> f32 {
    st(1.0 - fild_i32(left) / fild_i32(total))
}

/// The hull's health after progress moves from `old` to `progress` (capped at `max`).
pub fn grow_health(health: f32, old: f32, progress: f32, max: f32) -> f32 {
    let h = st((ld(progress) - ld(old)) * ld(max) + ld(health));
    if ld(max) < ld(h) {
        max
    } else {
        h
    }
}

fn build_time_raw(u: &Universe, t: ShipType) -> i32 {
    u.statics.ship(t).map_or(0, |s| s.build_time)
}

/// `MothershipGetDamageModifier5`: the Beast mothership builds twice as fast in its build
/// damage mode.
fn beast_build_rate(u: &Universe, ship: ShipId) -> i32 {
    crate::ships::mothership::damage_modifier_build(u, ship)
}

/// `cmShipBuildComplete` (0x44f490): a job finished one ship.
fn ship_build_complete(u: &mut Universe, factory: ShipId, idx: usize, cost_div: i32, time_div: i32) {
    let Some(fi) = factory_index(u, factory) else { return };
    let job = u.base.factories[fi].jobs[idx];
    let Some(t) = job.info else { return };
    let Some(stat) = u.statics.clone().ship(t).cloned() else { return };
    let player = u.ships.ship(factory).player;
    let mp = u.base.options.multiplayer_network;
    let mut built = None;
    if stat.external_build {
        if job.hull_exists && !mp {
            if let Some(h) = job.hull {
                update_external_construction(u, h, 1.0, ExternalUpdate::Complete);
                built = Some(h);
            }
            let Some(fi) = factory_index(u, factory) else { return };
            let j = &mut u.base.factories[fi].jobs[idx];
            j.hull_exists = false;
            j.hull = None;
        }
    } else if !mp {
        let race = u.players[player].race;
        built = cl_create_ship(u, t, race, player, factory);
    }
    u.base.events.push(BaseEvent::ShipBuilt { ship: built, ship_type: t, player });
    let Some(fi) = factory_index(u, factory) else { return };
    let j = &mut u.base.factories[fi].jobs[idx];
    if j.n_jobs > 0 {
        j.n_jobs -= 1;
        if u.single_player {
            j.cost_so_far -= stat.build_cost / cost_div;
        } else {
            j.cost_so_far -= stat.build_cost;
        }
        if j.n_jobs > 0 {
            j.cost_frac = 0;
            let time = if u.single_player { stat.build_time / time_div } else { stat.build_time };
            j.time_total = time;
            j.time_left = time;
            return;
        }
        j.n_total = 0;
    }
}

/// How `processUpdateExternalConstruction` (0x449a60) updates a hull.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalUpdate {
    /// Mode 0: finished.
    Complete,
    /// Mode 1: cancelled (the hull is removed).
    Cancel,
    /// Mode 2: progress.
    Progress,
}

/// `processUpdateExternalConstruction` (0x449a60): the hull's health follows the build
/// progress; when finished it stops being under construction (and a Somtaaw research module
/// activates its lab).
pub fn update_external_construction(u: &mut Universe, hull: ShipId, progress: f32, mode: ExternalUpdate) {
    let Some(s) = u.ships.get(hull) else { return };
    let max = u.statics.ship(s.ship_type).map_or(0.0, |x| x.health.max_health);
    let grow = |s: &mut crate::object::Ship| {
        let old = s.base.build_progress;
        s.base.build_progress = progress;
        s.health = grow_health(s.health, old, progress, max);
    };
    match mode {
        ExternalUpdate::Complete => {
            let s = u.ships.ship_mut(hull);
            if s.base.build_progress < 1.0 {
                grow(s);
            }
            s.special_flags2 &= !special2::UNDER_CONSTRUCTION;
            if s.ship_race == ShipRace::Somtaaw {
                let (p, t) = (s.player, s.ship_type);
                crate::research::activate_lab(u, p, t, true);
            }
        }
        ExternalUpdate::Cancel => {
            u.ships.ship_mut(hull).special_flags2 &= !special2::UNDER_CONSTRUCTION;
            crate::damage::really_delete_ship(u, hull);
        }
        ExternalUpdate::Progress => grow(u.ships.ship_mut(hull)),
    }
}

/// `clCreateShip` (0x4462e0): a finished ship appears inside its factory and, with
/// auto-launch on for the factory (or when it must leave), is launched.
pub fn cl_create_ship(u: &mut Universe, t: ShipType, _race: ShipRace, player: usize, factory: ShipId) -> Option<ShipId> {
    if u.base.players[player].state != 0 {
        return None;
    }
    let pos = u.ships.ship(factory).posinfo.position;
    let id = base::create_ship(
        u,
        t,
        player,
        pos,
        CreateOptions { flags: base::create_flags::BUILT | base::create_flags::NOT_IN_LISTS, builder: Some(factory), ..Default::default() },
    )
    .ok()?;
    unit_cap_create_ship(u, id);
    if t != ShipType::sSwarmer && t != ShipType::bSwarmer {
        if let Some(c) = base::class_of(u, t) {
            let st_ = &mut u.base.stats;
            if st_.players.len() <= player {
                st_.players.resize(player + 1, base::PlayerStats::default());
            }
            st_.players[player].built_per_class[c.index()] += 1;
            if matches!(t, ShipType::sACV | ShipType::sMCV | ShipType::bACV) {
                st_.players[player].built_per_class[c.index()] += 1;
            }
        }
    }
    crate::dock::put_ship_inside(u, id, factory);
    let bit = factory_launch_bit(u, factory, player);
    if bit & u.base.players[player].auto_launch != 0 || crate::dock::ship_has_to_launch(u, factory, id) {
        crate::dock::cl_launch_ship(u, id, factory);
    }
    Some(id)
}

/// The auto-launch bit `clCreateShip` computes: 1 for the player's mothership (or a lone
/// carrier factory), 2 and 4 for the player's carriers.
fn factory_launch_bit(u: &Universe, factory: ShipId, player: usize) -> u32 {
    let ms = u.base.players[player].mothership;
    let (mut a, mut b, mut c) = (ms, None, None);
    if let Some(m) = ms {
        if base::class_of(u, u.ships.get(m).map_or(ShipType::sRecon, |s| s.ship_type)) == Some(ShipClass::Carrier) {
            a = None;
            b = Some(m);
        }
    }
    for &(s, p) in &u.base.carriers {
        if p != player {
            continue;
        }
        if b.is_some() {
            c = Some(s);
            break;
        }
        b = Some(s);
    }
    if Some(factory) == a {
        1
    } else if Some(factory) == b {
        2
    } else if Some(factory) == c {
        4
    } else {
        0
    }
}

/// `cmExternalBuildShipLaunched` (0x451280): the hull is clamped in place; its job may
/// progress.
pub fn external_build_ship_launched(u: &mut Universe, ship: ShipId) {
    let s = u.ships.ship(ship);
    let (player, t, builder) = (s.player, s.ship_type, s.base.builder);
    if player != u.base.cur_player || u.base.players[player].ai {
        return;
    }
    let Some(b) = builder else { return };
    if let Some(fi) = factory_index(u, b) {
        u.base.factories[fi].jobs[t.index()].hull_launched = true;
    }
}

/// `cmCancelExternalBuildJob` (0x451110): an externally built hull died; its job pauses and
/// resets.
pub fn cancel_external_build_job(u: &mut Universe, ship: ShipId) {
    let s = u.ships.ship(ship);
    if s.base.clamp.is_none() {
        return;
    }
    let (player, t, builder) = (s.player, s.ship_type, s.base.builder);
    if player != u.base.cur_player || u.base.players[player].ai {
        return;
    }
    let Some(b) = builder else { return };
    let Some(fi) = factory_index(u, b) else { return };
    let j = &mut u.base.factories[fi].jobs[t.index()];
    if j.n_jobs > 0 && j.hull == Some(ship) {
        j.paused = true;
        j.cost_frac = 0;
        j.time_left = 0;
        j.time_total = 0;
        j.n_total -= 1;
        j.n_jobs -= 1;
        j.hull_exists = false;
        j.hull_launched = false;
        j.hull = None;
    }
}

/// `cmCancelFactoryJobs` (0x4515a0): the factory's docking bay died; externally built jobs are
/// cancelled and refunded (`cmFloatRefundRatio`), hulls removed.
pub fn cancel_factory_jobs(u: &mut Universe, factory: ShipId) {
    if u.base.options.tutorial {
        return;
    }
    let Some(fi) = factory_index(u, factory) else { return };
    let cur = u.base.cur_player;
    let ratio = u.tweaks.base.cm_float_refund_ratio;
    for idx in 0..base::NUM_SHIP_TYPES {
        let job = u.base.factories[fi].jobs[idx];
        let Some(t) = job.info else { continue };
        let Some(stat) = u.statics.ship(t) else { continue };
        if !stat.external_build || stat.ship_class == Some(ShipClass::Component) {
            continue;
        }
        if job.n_total != 0 {
            if !u.base.options.multiplayer_network {
                let refund = hwc_math::fpu::ftol_i32(ld(job.cost_so_far as f32) * ld(ratio));
                u.base.players[cur].resource_units += refund;
            }
            if t != ShipType::sMothershipDockingBay && job.hull_exists {
                if let Some(h) = job.hull {
                    update_external_construction(u, h, 0.0, ExternalUpdate::Cancel);
                }
            }
        }
        let Some(fi) = factory_index(u, factory) else { return };
        let info = u.base.factories[fi].jobs[idx].info;
        u.base.factories[fi].jobs[idx] = BuildJob { info, ..BuildJob::default() };
    }
}

// --- support units

/// `cmAddShipSupportUnits` (0x455020): a new ship uses support units; a finished one adds its
/// capacity.
pub fn add_ship_support_units(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship(id);
    if s.special_flags2 & special2::NO_SUPPORT_UNITS != 0 {
        return;
    }
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let (cost, cap, player, built) = (stat.support_units, stat.provides_su, s.player, s.special_flags2 & special2::UNDER_CONSTRUCTION == 0);
    u.base.players[player].su_used += cost;
    if built {
        u.base.players[player].su_capacity += cap;
        u.ships.ship_mut(id).special_flags2 |= special2::SU_CAPACITY_ADDED;
    }
}

/// `cmAddShipSupportCapacity` (0x454ff0): a hull finished construction.
pub fn add_ship_support_capacity(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship(id);
    if s.special_flags2 & special2::UNDER_CONSTRUCTION != 0 {
        return;
    }
    let cap = u.statics.ship(s.ship_type).map_or(0, |x| x.provides_su);
    let player = s.player;
    u.base.players[player].su_capacity += cap;
    u.ships.ship_mut(id).special_flags2 |= special2::SU_CAPACITY_ADDED;
}

/// `cmRemoveShipSupportUnits` (0x455070).
pub fn remove_ship_support_units(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.special_flags2 & special2::NO_SUPPORT_UNITS != 0 {
        return;
    }
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let (cost, cap, player, added) = (stat.support_units, stat.provides_su, s.player, s.special_flags2 & special2::SU_CAPACITY_ADDED != 0);
    u.base.players[player].su_used -= cost;
    if added {
        u.base.players[player].su_capacity -= cap;
        u.ships.ship_mut(id).special_flags2 &= !special2::SU_CAPACITY_ADDED;
    }
}

/// `cmPlayerHasSupportUnitsForType` (0x4550d0): room for one more ship of the type, counting
/// what is already queued (and `extra` more about to be queued).
pub fn player_has_support_units_for_type(u: &Universe, t: ShipType, player: usize, extra: i32) -> bool {
    let Some(stat) = u.statics.ship(t) else { return false };
    if player != 0 && u.single_player {
        return true;
    }
    if stat.support_units == 0 {
        return true;
    }
    let p = &u.base.players[player];
    let (used, cap) = (p.su_used as u32, p.su_capacity as u32);
    if cap <= used {
        return false;
    }
    let mut queued = 0i32;
    for f in &u.base.factories {
        if f.ship.and_then(|s| u.ships.get(s)).is_none_or(|s| s.player != player) {
            continue;
        }
        for j in &f.jobs {
            let Some(jt) = j.info else { continue };
            let Some(js) = u.statics.ship(jt) else { continue };
            let mut n = j.n_jobs;
            if n > 0 && js.external_build && j.hull_exists {
                n -= 1;
            }
            queued += js.support_units * n;
        }
    }
    (stat.support_units + queued + stat.support_units * extra) as u32 <= cap - used
}

// --- unit caps

/// `unitCapCreateShip` (0x539b60): count a ship (a mothership also counts its starting
/// modules).
pub fn unit_cap_create_ship(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship(id);
    if s.special_flags & special::UNIT_CAP_COUNTED != 0 {
        return;
    }
    let (player, t) = (s.player, s.ship_type);
    let class = base::class_of(u, t);
    let modules = if t == ShipType::sMothership {
        crate::modules::mission_module_types(u)
    } else if t == ShipType::bMothership {
        crate::modules::beast_module_types(u)
    } else {
        Vec::new()
    };
    let p = &mut u.base.players[player];
    p.total_ships += 1;
    p.ship_count[t.index()] += 1;
    if let Some(c) = class {
        p.class_count[c.index()] += 1;
    }
    for m in modules {
        p.total_ships += 1;
        p.ship_count[m.index()] += 1;
        p.module_count += 1;
    }
    u.ships.ship_mut(id).special_flags |= special::UNIT_CAP_COUNTED;
}

/// `unitCapDeleteShip` (0x539cb0).
pub fn unit_cap_delete_ship(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.special_flags & special::UNIT_CAP_COUNTED == 0 {
        return;
    }
    let (player, t) = (s.player, s.ship_type);
    let class = base::class_of(u, t);
    let p = &mut u.base.players[player];
    p.total_ships = (p.total_ships - 1).max(0);
    p.ship_count[t.index()] = (p.ship_count[t.index()] - 1).max(0);
    if let Some(c) = class {
        p.class_count[c.index()] = (p.class_count[c.index()] - 1).max(0);
    }
    u.ships.ship_mut(id).special_flags &= !special::UNIT_CAP_COUNTED;
}

/// `unitCapCanCreateShip` (0x539d90), for one more ship of type `t` at `factory` (plus `extra`
/// about to be queued): the total, per-type and per-class caps, counting jobs queued at the
/// player's factories. Only with unit caps on (or in single player).
pub fn unit_cap_can_create_ship(u: &Universe, t: ShipType, factory: ShipId, extra: i32) -> bool {
    if u.base.options.flags & base::game_flags::UNIT_CAPS_ENABLED == 0 && !u.single_player {
        return true;
    }
    let player = u.ships.ship(factory).player;
    let race = u.players[player].race;
    let caps = &u.statics.unit_caps;
    let class = base::class_of(u, t);
    let mut total = 1 + extra;
    let mut same_type = 1 + extra;
    let mut same_class = 1 + extra;
    for f in &u.base.factories {
        if f.ship.and_then(|s| u.ships.get(s)).is_none_or(|s| s.player != player) {
            continue;
        }
        for i in crate::research::race_types(race) {
            let j = &f.jobs[i];
            total += j.n_jobs;
            if i == t.index() {
                same_type += j.n_jobs;
                if u.statics.ship(t).is_some_and(|s| s.external_build) && j.n_jobs > 0 && j.hull_exists && same_type != 0 {
                    same_type -= 1;
                }
            }
            if ShipType::from_index(i).and_then(|x| base::class_of(u, x)) == class {
                same_class += j.n_jobs;
            }
        }
    }
    let pending = u.base.pending_by_type[t.index()];
    let p = &u.base.players[player];
    if caps.total >= 0 && (caps.total as u32) < (p.total_ships + pending + total) as u32 {
        return false;
    }
    let cap_t = caps.cap(t);
    if cap_t != -1 && (p.ship_count[t.index()] + same_type + pending) as u32 > cap_t as u32 {
        return false;
    }
    if let Some(c) = class {
        let cc = caps.by_class[c.index()];
        if cc != -1 && (cc as u32) < (p.class_count[c.index()] + same_class + pending) as u32 {
            return false;
        }
    }
    true
}

/// The job of a factory for a type.
pub fn job(u: &Universe, factory: ShipId, t: ShipType) -> Option<BuildJob> {
    factory_index(u, factory).map(|fi| u.base.factories[fi].jobs[t.index()])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `cmBuildTaskFunction` (0x450160) emulated frame by frame in cataclysm.exe on one job:
    /// (external, single player, cost divisor, cost, build time, starting RUs, hull max health,
    /// per frame `(time left, fraction, cost so far, RUs, hull health bits, progress bits)`).
    #[allow(clippy::type_complexity)]
    const BUILD_ROWS: &[(i32, i32, i32, i32, i32, i32, f32, &[(i32, i32, i32, i32, u32, u32)])] = &[
    (0, 0, 1, 1074, 686, 13916, f32::from_bits(0x45a4c800), &[(685, 37067, 1, 13915, 0x00000000, 0x00000000), (684, 8598, 3, 13913, 0x00000000, 0x00000000), (683, 45665, 4, 13912, 0x00000000, 0x00000000), (682, 17196, 6, 13910, 0x00000000, 0x00000000), (681, 54263, 7, 13909, 0x00000000, 0x00000000), (680, 25794, 9, 13907, 0x00000000, 0x00000000), (679, 62861, 10, 13906, 0x00000000, 0x00000000), (678, 34392, 12, 13904, 0x00000000, 0x00000000), (677, 5923, 14, 13902, 0x00000000, 0x00000000), (676, 42990, 15, 13901, 0x00000000, 0x00000000), (675, 14521, 17, 13899, 0x00000000, 0x00000000), (674, 51588, 18, 13898, 0x00000000, 0x00000000), (673, 23119, 20, 13896, 0x00000000, 0x00000000), (672, 60186, 21, 13895, 0x00000000, 0x00000000), (671, 31717, 23, 13893, 0x00000000, 0x00000000), (670, 3248, 25, 13891, 0x00000000, 0x00000000)]),
    (1, 0, 1, 2573, 565, 15251, f32::from_bits(0x469c8200), &[(564, 36305, 4, 15247, 0x420dd399, 0x3ae7fc60), (563, 7074, 9, 15242, 0x428dd399, 0x3b67fc60), (562, 43379, 13, 15238, 0x42d4bd65, 0x3badfd48), (561, 14148, 18, 15233, 0x430dd399, 0x3be7fc60), (560, 50453, 22, 15229, 0x4331487f, 0x3c10fdbc), (559, 21222, 27, 15224, 0x4354bd65, 0x3c2dfd48), (558, 57527, 31, 15220, 0x4378324b, 0x3c4afcd4)]),
    (0, 0, 1, 2580, 93, 19890, f32::from_bits(0x46de3400), &[(92, 48623, 27, 19863, 0x00000000, 0x00000000), (91, 31710, 55, 19835, 0x00000000, 0x00000000), (90, 14797, 83, 19807, 0x00000000, 0x00000000), (89, 63420, 110, 19780, 0x00000000, 0x00000000), (88, 46507, 138, 19752, 0x00000000, 0x00000000), (87, 29594, 166, 19724, 0x00000000, 0x00000000), (86, 12681, 194, 19696, 0x00000000, 0x00000000), (85, 61304, 221, 19669, 0x00000000, 0x00000000), (84, 44391, 249, 19641, 0x00000000, 0x00000000), (83, 27478, 277, 19613, 0x00000000, 0x00000000), (82, 10565, 305, 19585, 0x00000000, 0x00000000), (81, 59188, 332, 19558, 0x00000000, 0x00000000), (80, 42275, 360, 19530, 0x00000000, 0x00000000), (79, 25362, 388, 19502, 0x00000000, 0x00000000), (78, 8449, 416, 19474, 0x00000000, 0x00000000), (77, 57072, 443, 19447, 0x00000000, 0x00000000), (76, 40159, 471, 19419, 0x00000000, 0x00000000), (75, 23246, 499, 19391, 0x00000000, 0x00000000), (74, 6333, 527, 19363, 0x00000000, 0x00000000), (73, 54956, 554, 19336, 0x00000000, 0x00000000)]),
    (1, 0, 1, 1162, 644, 8839, f32::from_bits(0x45e39800), &[(643, 52713, 1, 8838, 0x4134f1b1, 0x3acb8728), (642, 39890, 3, 8836, 0x41b4f1b1, 0x3b4b8728), (641, 27067, 5, 8834, 0x4207b545, 0x3b98a55e), (640, 14244, 7, 8832, 0x4234f1b1, 0x3bcb8728), (639, 1421, 9, 8830, 0x42622e1d, 0x3bfe68f2), (638, 54134, 10, 8829, 0x4287b545, 0x3c18a55e), (637, 41311, 12, 8827, 0x429e537b, 0x3c321643), (636, 28488, 14, 8825, 0x42b4f1b1, 0x3c4b8728), (635, 15665, 16, 8823, 0x42cb8fe7, 0x3c64f80d), (634, 2842, 18, 8821, 0x42e22e1d, 0x3c7e68f2), (633, 55555, 19, 8820, 0x42f8cc52, 0x3c8beceb), (632, 42732, 21, 8818, 0x4307b544, 0x3c98a55e), (631, 29909, 23, 8816, 0x4313045f, 0x3ca55dd0), (630, 17086, 25, 8814, 0x431e537a, 0x3cb21643), (629, 4263, 27, 8812, 0x4329a295, 0x3cbeceb5), (628, 56976, 28, 8811, 0x4334f1b0, 0x3ccb8728), (627, 44153, 30, 8809, 0x434040cb, 0x3cd83f9a), (626, 31330, 32, 8807, 0x434b8fe6, 0x3ce4f80d), (625, 18507, 34, 8805, 0x4356df01, 0x3cf1b07f), (624, 5684, 36, 8803, 0x43622e1c, 0x3cfe68f2)]),
    (0, 1, 1, 2315, 642, 12804, f32::from_bits(0x465af400), &[(641, 39709, 3, 12801, 0x00000000, 0x00000000), (640, 13882, 7, 12797, 0x00000000, 0x00000000), (639, 53591, 10, 12794, 0x00000000, 0x00000000), (638, 27764, 14, 12790, 0x00000000, 0x00000000), (637, 1937, 18, 12786, 0x00000000, 0x00000000), (636, 41646, 21, 12783, 0x00000000, 0x00000000), (635, 15819, 25, 12779, 0x00000000, 0x00000000), (634, 55528, 28, 12776, 0x00000000, 0x00000000), (633, 29701, 32, 12772, 0x00000000, 0x00000000), (632, 3874, 36, 12768, 0x00000000, 0x00000000), (631, 43583, 39, 12765, 0x00000000, 0x00000000), (630, 17756, 43, 12761, 0x00000000, 0x00000000), (629, 57465, 46, 12758, 0x00000000, 0x00000000), (628, 31638, 50, 12754, 0x00000000, 0x00000000), (627, 5811, 54, 12750, 0x00000000, 0x00000000), (626, 45520, 57, 12747, 0x00000000, 0x00000000), (625, 19693, 61, 12743, 0x00000000, 0x00000000), (624, 59402, 64, 12740, 0x00000000, 0x00000000), (623, 33575, 68, 12736, 0x00000000, 0x00000000), (622, 7748, 72, 12732, 0x00000000, 0x00000000), (621, 47457, 75, 12729, 0x00000000, 0x00000000), (620, 21630, 79, 12725, 0x00000000, 0x00000000), (619, 61339, 82, 12722, 0x00000000, 0x00000000), (618, 35512, 86, 12718, 0x00000000, 0x00000000), (617, 9685, 90, 12714, 0x00000000, 0x00000000)]),
    (1, 1, 2, 716, 317, 15401, f32::from_bits(0x45ba8000), &[(316, 8476, 1, 15400, 0x41969cab, 0x3b4ebcf9), (315, 16952, 2, 15399, 0x42169cab, 0x3bcebcf9), (314, 25428, 3, 15398, 0x4261eb01, 0x3c1b0dbb), (313, 33904, 4, 15397, 0x42969cab, 0x3c4ebcf9), (312, 42380, 5, 15396, 0x42bc43d5, 0x3c81361b), (311, 50856, 6, 15395, 0x42e1eb01, 0x3c9b0dbb), (310, 59332, 7, 15394, 0x4303c916, 0x3cb4e55a), (309, 2272, 9, 15392, 0x43169cab, 0x3ccebcf9), (308, 10748, 10, 15391, 0x43297040, 0x3ce89498), (307, 19224, 11, 15390, 0x433c43d5, 0x3d01361b), (306, 27700, 12, 15389, 0x434f176b, 0x3d0e21eb), (305, 36176, 13, 15388, 0x4361eb01, 0x3d1b0dbb), (304, 44652, 14, 15387, 0x4374be96, 0x3d27f98a), (303, 53128, 15, 15386, 0x4383c916, 0x3d34e55a), (302, 61604, 16, 15385, 0x438d32e0, 0x3d41d129), (301, 4544, 18, 15383, 0x43969cab, 0x3d4ebcf9), (300, 13020, 19, 15382, 0x43a00675, 0x3d5ba8c8), (299, 21496, 20, 15381, 0x43a97040, 0x3d689498), (298, 29972, 21, 15380, 0x43b2da0a, 0x3d758067), (297, 38448, 22, 15379, 0x43bc43d4, 0x3d81361b), (296, 46924, 23, 15378, 0x43c5ad9f, 0x3d87ac03)]),
    ];

    #[test]
    fn build_progress_matches_emulated_binary() {
        for (c, &(ext, sp, cdiv, cost, total, ru0, maxh, frames)) in BUILD_ROWS.iter().enumerate() {
            let mut job = BuildJob { n_jobs: 1, n_total: 1, time_total: total, time_left: total, ..Default::default() };
            let mut ru = ru0;
            let mut above = false;
            let (mut health, mut prog) = (0.0f32, 0.0f32);
            for (f, &(left, frac, so_far, ru_e, h, p)) in frames.iter().enumerate() {
                charge_frame(&mut job, cost, sp != 0, cdiv, false, 1, &mut ru, &mut above);
                if ext != 0 && job.time_left >= 1 {
                    let np = external_progress(job.time_left, job.time_total);
                    health = grow_health(health, prog, np, maxh);
                    prog = np;
                }
                assert_eq!((job.time_left, job.cost_frac, job.cost_so_far, ru, health.to_bits(), prog.to_bits()), (left, frac, so_far, ru_e, h, p), "case {c} frame {f}");
            }
        }
    }

    #[test]
    fn fixed_point_cost_split() {
        // 1000 RU over 640 frames: 102 400 per frame in 16.16, so 1 RU and a carried fraction.
        let dec = (1000i32 << 16) / 640;
        assert_eq!(dec >> 16, 1);
        assert_eq!(dec & 0xffff, 36_864);
    }
}
