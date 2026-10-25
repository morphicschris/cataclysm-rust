//! The Beast: infection (`beastInfectShip` 0x429aa0, `univCheckBeastInfectionComplete`
//! 0x53e8f0, `univConvertShipToPlayer` 0x53e9e0), the infection cascade of the Beast's capital
//! ships (0x429d70..0x42b7e0) and the module wrappers that run it (`HeavyCruiser`, `Carrier`,
//! `BeastMothership` 0x5aaba0..0x5aad60; the Mothership module calls the same routines).
//!
//! A Beast heavy cruiser, carrier or mothership (and the Beast mothership of the missions) with
//! the cascade upgrade (always in single player) takes the nearest of its special targets,
//! flies at it and, once its infection gun is charged, shoots it. A ship with Beast infection
//! statistics counts the hit; when it has taken `beastAttacksRequired` of them it is infected:
//! it is disabled and spins for `beastAttackTimeSpentStunned` seconds and then becomes a ship of
//! the infecting player (its `beastAttackConvertTo` type). Other ships just lose a fifth of their
//! health. After `beastAttackCascadeDelay` seconds the beam jumps from a hit ship to the nearest
//! ship of the same player within `beastAttackCascadeDistance` whose `beastAttackCascadeCost` is
//! below the attack's remaining points (`beastAttackCascadePoints` at the start), and so on.
//!
//! The tunables come from `RACE_BEAST\beastweapon.script` (see [`crate::base_tweaks`]).

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_ETG;
use hwc_math::Vector;

use crate::object::{sof, BulletId, ShipId};
use crate::statics::{ShipRace, ShipStaticInfo, ShipType};
use crate::universe::Universe;

/// `suShipHasUpgrade(ship, 0x10000)`: the Beast cascade upgrade.
pub const UPGRADE_CASCADE: u32 = 0x1_0000;

/// `specialFlags2` (+0x1cc) bits of the Beast code.
pub mod special2 {
    /// KAS made the ship's infection unlimited (`kasfShipsSetBeastInfectionUnlimited`): the
    /// cascade jumps from it whatever the cost.
    pub const INFECTION_UNLIMITED: u32 = 0x8000;
    /// Newly beastified (`kasfShipsSelectBeastified` reads and clears it).
    pub const BEASTIFIED: u32 = 0x1_0000;
    /// The ship is not counted against support units (a cascade may take it from its own
    /// side).
    pub const NO_SUPPORT_UNITS: u32 = 0x10_0000;
    /// KAS turned the automatic use of the Beast weapon off (`kasfEnableBeastWeaponAutoAttack`).
    pub const NO_AUTO_ATTACK: u32 = 0x80_0000;
}

/// `flags2` (+0x18) bit set at random when a ship is infected (the spin direction of its
/// stun animation).
pub const FLAGS2_STUN_SPIN: u32 = 0x80;

/// The bullet type of the infection gun (`GunStatic +4 == 0x13`).
const BEAST_BULLET_TYPE: u32 = 0x13;

/// A ship's Beast state (+0x4e0..+0x4f8, +0x614, and for the cascade types +0x638..).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Infection {
    /// +0x4e0: infection hits taken.
    pub attacks: i32,
    /// +0x4e4: when the infection completes (0 = not infected).
    pub timer: f32,
    /// +0x4e8: the ship that infected it (a cascade ship sets this to itself when it fires).
    pub infector: Option<ShipId>,
    /// +0x4ec: the player the ship goes to.
    pub player: Option<usize>,
    /// +0x4f0: when it was last hit by the infection beam.
    pub last_attack: f32,
    /// +0x4f4: cascade points left when the beam jumps on from this ship.
    pub cascade_points: f32,
    /// +0x4f8: when the beam jumps on (0: it does not).
    pub cascade_time: f32,
    /// +0x614: infection-missile hits (counted by the cruise missile code).
    pub missile_hits: i8,
    /// +0x638: the cascade of the Beast capital ships (`beastCascadeInit`).
    pub cascade: Option<Box<Cascade>>,
}

/// A cascade (+0x638): the target chosen by the special, the beams in flight, whether the ship
/// was flying at its target.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cascade {
    /// +0x638.
    pub target: Option<ShipId>,
    /// +0x63c: one node per beam (`listAddNode` order).
    pub entries: Vec<CascadeEntry>,
    /// +0x64c.
    pub moving: bool,
}

/// One beam of a cascade (0x1c-byte node: +0x10 bullet, +0x14 source, +0x18 target).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CascadeEntry {
    pub bullet: Option<BulletId>,
    pub source: Option<ShipId>,
    pub target: Option<ShipId>,
}

/// Per-ship state of the Beast mothership's module (`BeastMothershipInit` keeps none beyond
/// the cascade).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BeastMothershipSpec {}

/// Per-type statics (`HeavyCruiserStaticInit`, `CarrierStaticInit`, `BeastMothershipStaticInit`:
/// whether the type has the cascade, which loads `beastweapon.script`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BeastStatics {
    pub has_cascade: bool,
}

impl BeastStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        Self { has_cascade: static_has_cascade(stat) }
    }
}

/// `beastShipHasCascade` / `beastStaticHasCascade` (0x429d70 / 0x429ea0).
pub fn type_has_cascade(t: ShipType) -> bool {
    matches!(t, ShipType::bHeavyCruiser | ShipType::bCarrier | ShipType::bMothership | ShipType::dBeastMothership)
}

/// `beastStaticHasCascade` (0x429ea0).
pub fn static_has_cascade(stat: &ShipStaticInfo) -> bool {
    type_has_cascade(stat.ship_type)
}

/// `beastShipHasCascade` (0x429d70).
pub fn ship_has_cascade(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| type_has_cascade(s.ship_type))
}

/// `beastShipCanCascadeNow` (0x429e00): a cascade type with the upgrade.
pub fn ship_can_cascade_now(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| type_has_cascade(s.ship_type) && crate::upgrade::has_upgrade(s, UPGRADE_CASCADE))
}

/// `beastGetInfectionStats` (0x429d20): `(beastAttackCascadeCost, beastAttacksRequired,
/// beastAttackConvertTo)` of an infectable type.
pub fn infection_stats(stat: &ShipStaticInfo) -> Option<(f32, i32, Option<ShipType>)> {
    (stat.beast_attacks_required >= 0).then_some((stat.beast_attack_cascade_cost, stat.beast_attacks_required, stat.beast_attack_convert_to))
}

fn ship_infection_stats(u: &Universe, id: ShipId) -> Option<(f32, i32, Option<ShipType>)> {
    let s = u.ships.get(id)?;
    infection_stats(u.statics.ship(s.ship_type)?)
}

/// `beastInfectDuration` (0x42b7e0): how long an infected ship stays stunned (the Ecclesiam
/// twice as long). Register value.
pub fn infect_duration(u: &Universe, t: ShipType) -> f64 {
    let d = ld(u.tweaks.base.beast_time_spent_stunned);
    if t == ShipType::dEcclesiam {
        d + d
    } else {
        d
    }
}

/// `beastInfectShip` (0x429aa0): `infector` infects the ship: it is disabled and starts to
/// spin, flickers, and converts to the infector's player when the stun time is over
/// ([`check_infection_complete`]).
pub fn infect_ship(u: &mut Universe, id: ShipId, infector: ShipId) {
    let Some(player) = u.ships.get(infector).map(|s| s.player) else { return };
    let statics = u.statics.clone();
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return };
    let Some(stat) = statics.ship(t) else { return };
    let spin = u.tweaks.base.beast_stunned_spin_rate;
    let dur = infect_duration(u, t);
    let now = u.total_time_elapsed;
    u.ships.ship_mut(id).flags |= sof::DISABLED | sof::CRAZY;
    let r = u.randy.random(RAN_ETG);
    let s = u.ships.ship_mut(id);
    s.flags2 = (s.flags2 & !FLAGS2_STUN_SPIN) | if 50 < r % 100 { FLAGS2_STUN_SPIN } else { 0 };
    let third = st(ld(spin) * ld(0.333_333_34f32));
    let quarter = st(ld(spin) * 0.25);
    let oi = stat.header.one_over_moment_of_inertia;
    let tq = &mut s.rotinfo.torque;
    tq.x = st(ld(spin) * 0.5 / ld(oi.x) + ld(tq.x));
    tq.y = st(ld(third) / ld(oi.y) + ld(tq.y));
    tq.z = st(ld(quarter) / ld(oi.z) + ld(tq.z));
    s.beast.infector = Some(infector);
    s.beast.player = Some(player);
    s.beast.timer = st(ld(now) + dur);
    let next = ld(now) + 1.0;
    s.flicker.next = st(next);
    s.flicker.state = 2;
    s.flicker.end = st(next + dur);
    // (The ship's mesh bindings switch to the infected type's, its animation stops, the
    // "ship infected" speech plays and the computer player writes the ship off; mission 7
    // fires its first-infection trigger. None of that is simulated.)
    ship_just_disabled(u, id);
}

/// `shipHasJustBeenDisabled(ship, 1)`: nobody keeps it as an attack target (crystals excepted).
fn ship_just_disabled(u: &mut Universe, id: ShipId) {
    if !crate::ships::crystal::is_crystal(u, id) {
        crate::command::remove_ship_from_being_targeted(u, id);
    }
    // (The gravity well generators' own shutdown and the mesh link animations are not
    // simulated here.)
}

/// `univCheckBeastInfectionComplete` (0x53e8f0), at the start of `univShipThink`: an infected
/// ship has no orders; when its time is up it is counted and converted to the infecting
/// player (`univConvertShipToPlayer`). True when the ship is gone (it stops thinking).
pub fn check_infection_complete(u: &mut Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    if s.beast.timer == 0.0 {
        return false;
    }
    // (Not during an in-game cinematic, which the simulation does not have.)
    if s.command.is_some() {
        crate::command::remove_ships_from_doing_stuff(u, &[id]);
    }
    let s = u.ships.ship(id);
    if !(s.beast.timer <= u.total_time_elapsed) {
        return false;
    }
    let new_type = ship_infection_stats(u, id).map_or(Some(ShipType::bWorker), |(_, _, to)| to).unwrap_or(ShipType::bWorker);
    let player = s.beast.player.unwrap_or(s.player);
    u.base.stats.add_ship_beastified(new_type, player);
    let s = u.ships.ship_mut(id);
    s.how_did_i_die = crate::damage::how::KILLED_BY_PLAYER;
    s.who_killed_me = player as i32;
    if let Some(n) = convert_ship_to_player(u, id, new_type, player) {
        u.ships.ship_mut(n).special_flags2 |= special2::BEASTIFIED;
    }
    true
}

/// The salvage depot holding a ship (ship +0x3ec).
fn depot_of(u: &Universe, id: ShipId) -> Option<ShipId> {
    u.ships.iter().find(|d| d.base.depot.as_ref().is_some_and(|slots| slots.iter().any(|s| s.ship == Some(id)))).map(|d| d.id)
}

/// `univConvertShipToPlayer` (0x53e9e0): a ship of `new_type` owned by `player` replaces the
/// ship (pose, motion, health, cargo, gun technologies, armour and upgrades carried over) and
/// the old one leaves the game. Without support units for it (and outside a depot) the ship is
/// hit for 5000 instead and its infection ends. Ships going to the Beast flicker; a computer
/// player in a multiplayer game sends its new ship home (one in ten to be retired), and a
/// processor hands over its technologies.
pub fn convert_ship_to_player(u: &mut Universe, old: ShipId, new_type: ShipType, player: usize) -> Option<ShipId> {
    let statics = u.statics.clone();
    {
        let s = u.ships.get_mut(old)?;
        s.beast.timer = 0.0;
    }
    let depot = depot_of(u, old);
    if !crate::construction::player_has_support_units_for_type(u, new_type, player, 0) && depot.is_none() {
        // (The local player is told that there are not enough support units.)
        crate::damage::apply_damage_to_target(u, old, 5000.0, BEAST_BULLET_TYPE as i32, crate::damage::how::KILLED_BY_PLAYER, player as i32, None);
        if let Some(s) = u.ships.get_mut(old) {
            s.beast.timer = 0.0;
            s.flags &= !(sof::DISABLED | sof::CRAZY);
        }
        return None;
    }
    let o = u.ships.ship(old).clone();
    let stat = statics.ship(new_type)?;
    let id = crate::base::create_ship(u, new_type, player, o.posinfo.position, crate::base::CreateOptions::default()).ok()?;
    {
        let n = u.ships.ship_mut(id);
        for (g, og) in n.guns.iter_mut().zip(o.guns.iter()) {
            g.level = og.level;
        }
    }
    crate::construction::unit_cap_create_ship(u, id);
    // (The old type's research-table entry of the old player is copied over the new type's of
    // the new player; the research tables are shared statics here.)
    crate::damage::really_delete_ship(u, old);
    let now = u.total_time_elapsed;
    let n = u.ships.ship_mut(id);
    n.posinfo.velocity = o.posinfo.velocity;
    n.posinfo.force = o.posinfo.force;
    n.rotinfo = o.rotinfo;
    n.health = o.health;
    n.posinfo.is_moving = o.posinfo.is_moving;
    if stat.health.max_health < o.health {
        n.health = stat.health.max_health;
    }
    n.base.resources = o.base.resources;
    if stat.max_resources < o.base.resources {
        n.base.resources = stat.max_resources;
    }
    n.armour_level = o.armour_level;
    n.upgrade_flags = o.upgrade_flags;
    crate::physics::update_obj_rot_info(n, stat);
    if u.players[player].race != ShipRace::Beast {
        return Some(id);
    }
    let beast_depot = depot.and_then(|d| u.ships.get(d)).is_none_or(|d| d.ship_race == ShipRace::Beast);
    if beast_depot {
        let n = u.ships.ship_mut(id);
        let next = ld(now) + 0.5;
        n.flicker.state = 2;
        n.flicker.next = st(next);
        n.flicker.end = st(next + 0.5);
    }
    if u.single_player {
        return Some(id);
    }
    if u.base.players[player].ai && new_type != ShipType::bProcessor {
        let r = u.randy.random(hwc_math::random::RAN_AI_PLAYER) % 100 + 1;
        let purpose = if r < 10 { crate::dock::purpose::RETIRE } else { 0 };
        crate::dock::cl_dock(u, &[id], purpose, None);
        return Some(id);
    }
    if new_type == ShipType::bProcessor {
        crate::research::acquire_ship_tech_on_dock(u, id);
    }
    // (The local player hears which technologies the new ship brings.)
    Some(id)
}

/// `kasfShipsBeastify` (0x493d10): the mission infects the ships that can be infected (by
/// `by`, the first ship of its second team). Returns the others, which the original scuttles
/// (`clScuttle` sets their self-destruct timers; not simulated).
pub fn kas_ships_beastify(u: &mut Universe, ships: &[ShipId], by: ShipId) -> Vec<ShipId> {
    let mut scuttle = Vec::new();
    for &s in ships {
        if u.ships.get(s).is_none() {
            continue;
        }
        if ship_infection_stats(u, s).is_none() {
            scuttle.push(s);
        } else {
            infect_ship(u, s, by);
        }
    }
    scuttle
}

/// `kasfShipsSetBeastInfectionUnlimited` (0x494080): the cascade jumps from these ships
/// regardless of cost, and their infection guns count as fired at 0.01 s.
pub fn kas_set_infection_unlimited(u: &mut Universe, ships: &[ShipId], on: bool) {
    let statics = u.statics.clone();
    for &id in ships {
        let Some(s) = u.ships.get_mut(id) else { continue };
        s.special_flags2 = (s.special_flags2 & !special2::INFECTION_UNLIMITED) | if on { special2::INFECTION_UNLIMITED } else { 0 };
        let Some(gs) = statics.ship(s.ship_type).and_then(|st| st.guns.as_ref()) else { continue };
        for (g, gs) in s.guns.iter_mut().zip(gs.iter()) {
            if crate::gun::static_bullet_type(gs, false) == BEAST_BULLET_TYPE {
                g.last_time_fired = 0.01;
            }
        }
    }
}

/// `kasPredShipBeastifiedClear` (0x494120) over a selection (`kasfShipsSelectBeastified`):
/// the newly beastified ships, whose flag is cleared.
pub fn kas_select_beastified(u: &mut Universe, ships: &[ShipId]) -> Vec<ShipId> {
    let mut out = Vec::new();
    for &id in ships {
        let Some(s) = u.ships.get_mut(id) else { continue };
        if s.special_flags2 & special2::BEASTIFIED != 0 {
            s.special_flags2 &= !special2::BEASTIFIED;
            out.push(id);
        }
    }
    out
}

/// `kasfEnableBeastWeaponAutoAttack` (0x495a20).
pub fn kas_enable_weapon_auto_attack(u: &mut Universe, ships: &[ShipId], on: bool) {
    for &id in ships {
        if let Some(s) = u.ships.get_mut(id) {
            s.special_flags2 = (s.special_flags2 & !special2::NO_AUTO_ATTACK) | if on { 0 } else { special2::NO_AUTO_ATTACK };
        }
    }
}

/// `kasfIsBeastReady` (0x495d60): some ship's infection gun is charged.
pub fn kas_is_beast_ready(u: &Universe, ships: &[ShipId]) -> bool {
    ships.iter().any(|&s| u.ships.get(s).is_some() && 1.0 <= gun_charge_fraction(u, s))
}

/// `kasfShipInfectionTerminal` (0x4942f0... the first ship of the selection is infected).
pub fn kas_infection_terminal(u: &Universe, ships: &[ShipId]) -> bool {
    ships.first().and_then(|&s| u.ships.get(s)).is_some_and(|s| s.beast.timer != 0.0)
}

/// `kasfGetShipCountBeastified` / `gameStatsGetShipsBeastified` (0x53b130).
pub fn ships_beastified(u: &Universe, t: Option<ShipType>, player: usize) -> i32 {
    u.base.stats.ships_beastified(t, player)
}

// --- the cascade

fn cascade(u: &Universe, id: ShipId) -> Option<&Cascade> {
    u.ships.get(id)?.beast.cascade.as_deref()
}

fn cascade_mut(u: &mut Universe, id: ShipId) -> Option<&mut Cascade> {
    u.ships.get_mut(id)?.beast.cascade.as_deref_mut()
}

/// `beastCascadeInit` (0x42a1f0).
pub fn cascade_init(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get_mut(id) else { return };
    s.beast.cascade = Some(Box::default());
    s.special_flags2 &= !special2::INFECTION_UNLIMITED;
}

/// `beastCascadeClose` (0x42a2b0).
pub fn cascade_close(u: &mut Universe, id: ShipId) {
    if let Some(c) = cascade_mut(u, id) {
        c.entries.clear();
        c.target = None;
    }
}

/// `beastCascadeRemoveRefs` (0x42a360) as the cascade types' slot 14: a bullet was freed.
pub fn cascade_remove_refs(u: &mut Universe, id: ShipId, bullet: BulletId) {
    if let Some(c) = cascade_mut(u, id) {
        for e in &mut c.entries {
            if e.bullet == Some(bullet) {
                e.bullet = None;
            }
        }
    }
}

/// `beastCascadeRemoveShip` (0x42a430): `gone` leaves the universe.
pub fn cascade_remove_ship(u: &mut Universe, id: ShipId, gone: ShipId) {
    if let Some(c) = cascade_mut(u, id) {
        if c.target == Some(gone) {
            c.target = None;
        }
        for e in &mut c.entries {
            if e.source == Some(gone) {
                e.source = None;
            }
            if e.target == Some(gone) {
                e.target = None;
            }
        }
    }
}

/// `beastFindNearestTarget` (0x42a580): the nearest of `targets` the ship's player can see
/// (anything in single player), not Leeches or Swarmers.
pub fn find_nearest_target(u: &Universe, id: ShipId, targets: &[ShipId]) -> Option<ShipId> {
    let s = u.ships.get(id)?;
    let p = s.posinfo.position;
    let mut best = 1e21f32;
    let mut found = None;
    for &t in targets {
        let Some(ts) = u.ships.get(t) else { continue };
        if matches!(ts.ship_type, ShipType::sLeech | ShipType::sSwarmer | ShipType::bSwarmer) {
            continue;
        }
        if crate::visibility::visibility_for_player(u, t, s.player) != 0 && !u.single_player {
            continue;
        }
        let q = ts.posinfo.position;
        let (dx, dy, dz) = (ld(q.x) - ld(p.x), ld(q.y) - ld(p.y), ld(q.z) - ld(p.z));
        let d = dx * dx + dy * dy + dz * dz;
        if d < ld(best) {
            best = st(d);
            found = Some(t);
        }
    }
    found
}

/// `beastHasTarget` (0x42a650).
pub fn has_target(u: &Universe, id: ShipId, targets: &[ShipId]) -> bool {
    cascade(u, id).is_some_and(|c| c.target.is_some()) || find_nearest_target(u, id, targets).is_some()
}

/// `MakeTargetsNotIncludeUntargetable` (0x50dd40) and the crystals
/// (`MakeTargetsNotIncludeUntargetableOrCrystals`, 0x50de10): the original swaps the last
/// target into the removed slot.
pub fn remove_untargetable_and_crystals(u: &Universe, targets: &mut Vec<ShipId>) {
    let untargetable = |t: ShipId| {
        let Some(s) = u.ships.get(t) else { return true };
        let caps = u.statics.ship(s.ship_type).map_or(0, |st| st.capability_flags);
        caps & 0x4000 != 0 || s.ship_type == ShipType::sAcolyte || s.special_flags & 0x20_0000 != 0 || s.ship_type == ShipType::dEcclesiamInfected
    };
    let mut i = 0;
    while i < targets.len() {
        if untargetable(targets[i]) {
            targets.swap_remove(i);
        } else {
            i += 1;
        }
    }
    let mut i = 0;
    while i < targets.len() {
        if crate::ships::crystal::is_crystal(u, targets[i]) {
            targets.swap_remove(i);
        } else {
            i += 1;
        }
    }
}

/// `beastChooseTarget` (0x42a700): with the upgrade (or in single player), the nearest
/// target becomes the cascade target unless a beam already holds it or it is infected. True
/// when the special is done (no target).
pub fn choose_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let mut targets = targets.to_vec();
    remove_untargetable_and_crystals(u, &mut targets);
    if targets.is_empty() {
        return true;
    }
    let Some(s) = u.ships.get(id) else { return true };
    if !crate::upgrade::has_upgrade(s, UPGRADE_CASCADE) && !u.single_player {
        return true;
    }
    if let Some(t) = find_nearest_target(u, id, &targets) {
        let held = cascade(u, id).is_some_and(|c| c.entries.iter().any(|e| e.target == Some(t)));
        let infected = u.ships.ship(t).beast.timer != 0.0;
        if !held && !infected {
            if let Some(c) = cascade_mut(u, id) {
                c.target = Some(t);
            }
        }
        // (A new target is announced.)
    }
    cascade(u, id).is_none_or(|c| c.target.is_none())
}

/// `beastClearTarget` (0x42a850).
pub fn clear_target(u: &mut Universe, id: ShipId) {
    if let Some(c) = cascade_mut(u, id) {
        c.target = None;
    }
}

/// `beastCascadeTargetFilter` (0x42a8f0): the ships the beam may jump to from `source`: same
/// player, not infected, not a Leech or Swarmer, not held by a beam of any cascade ship, and
/// cheaper than the points left (unless the source is unlimited).
pub fn cascade_target_filter(u: &Universe, candidate: ShipId, source: ShipId) -> bool {
    let (Some(c), Some(src)) = (u.ships.get(candidate), u.ships.get(source)) else { return false };
    if c.player != src.player || c.beast.timer != 0.0 || matches!(c.ship_type, ShipType::sLeech | ShipType::sSwarmer | ShipType::bSwarmer) {
        return false;
    }
    for &o in &u.ship_list {
        let Some(os) = u.ships.get(o) else { continue };
        if !type_has_cascade(os.ship_type) {
            continue;
        }
        if os.beast.cascade.as_ref().is_some_and(|cc| cc.entries.iter().any(|e| e.target == Some(candidate))) {
            return false;
        }
    }
    let cost = u.statics.ship(c.ship_type).map_or(0.0, |s| s.beast_attack_cascade_cost);
    src.special_flags2 & special2::INFECTION_UNLIMITED != 0 || cost < src.beast.cascade_points
}

/// `univFindNearestShip` (0x53a640) with [`cascade_target_filter`].
fn find_nearest_cascade_target(u: &Universe, source: ShipId) -> Option<ShipId> {
    let p = u.ships.get(source)?.posinfo.position;
    let mut best = 1e20f32;
    let mut found = None;
    for &o in &u.ship_list {
        let Some(os) = u.ships.get(o) else { continue };
        if !cascade_target_filter(u, o, source) {
            continue;
        }
        let q = os.posinfo.position;
        let (dx, dy, dz) = (ld(q.x) - ld(p.x), ld(q.y) - ld(p.y), ld(q.z) - ld(p.z));
        let d = dx * dx + dy * dy + dz * dz;
        if d < ld(best) {
            best = st(d);
            found = Some(o);
        }
    }
    found
}

/// `beastGunChargeFraction` (0x42b720): how far the (last) infection gun has recharged since
/// it fired (1: ready). Register value.
pub fn gun_charge_fraction(u: &Universe, id: ShipId) -> f64 {
    let Some(s) = u.ships.get(id) else { return 0.0 };
    let Some(guns) = u.statics.ship(s.ship_type).and_then(|st| st.guns.as_ref()) else { return 0.0 };
    let now = ld(u.total_time_elapsed);
    let mut f = 0.0;
    for (g, gs) in s.guns.iter().zip(guns.iter()) {
        if crate::gun::static_bullet_type(gs, false) != BEAST_BULLET_TYPE {
            continue;
        }
        let since = now - ld(g.last_time_fired);
        if since <= ld(gs.fire_time) {
            if gs.fire_time <= 0.0 {
                // ("weapon does not have a fire time")
                return 0.0;
            }
            let k = since / ld(gs.fire_time);
            return if k <= 1.0 { k } else { 1.0 };
        }
        f = 1.0;
    }
    f
}

/// Whether a sentinel's force field is up (`SentinelFieldIsUp`).
fn sentinel_field_up(u: &Universe, id: ShipId) -> bool {
    let _ = (u, id);
    // (The sentinels' field state is kept by the sentinel module; until it is reachable from
    // here the field counts as down.)
    false
}

/// `FUN_004366e0`: whether the ray from `from` along `dir` crosses the sentinel's field.
fn sentinel_field_crossed(u: &Universe, sentinel: ShipId, from: Vector, dir: Vector) -> bool {
    let _ = (u, sentinel, from, dir);
    false
}

fn is_sentinel(t: ShipType) -> bool {
    matches!(t, ShipType::sSentinel | ShipType::bSentinel)
}

/// `beastAttackTarget` (0x42b0f0): the infection beam `bullet` from `source` hits `target`
/// (whose infector is already set). Infectable ships count the hit and are infected at the
/// required count (a raised sentinel field takes the hit instead); others lose a fifth of
/// their health (all but one point when under 2000) and the heavy cruiser that fired keeps
/// shooting at them (multiplayer). The beam is recorded with the infector's cascade so that it
/// can jump on. False when the target has no infector.
pub fn attack_target(u: &mut Universe, source: ShipId, target: ShipId, bullet: Option<BulletId>) -> bool {
    let Some(t) = u.ships.get(target) else { return false };
    let Some(infector) = t.beast.infector else { return false };
    let tw = u.tweaks.base.clone();
    let now = u.total_time_elapsed;
    let stats = ship_infection_stats(u, target);
    if let Some((_, required, _)) = stats {
        let t = u.ships.ship_mut(target);
        t.beast.attacks += 1;
        let attacks = t.beast.attacks;
        if is_sentinel(t.ship_type) && sentinel_field_up(u, target) {
            return true;
        }
        if attacks < required {
            let t = u.ships.ship_mut(target);
            t.flicker.delay = -1.0;
            t.flicker.state = 1;
            let next = ld(now) + 1.0;
            t.flicker.next = st(next);
            t.flicker.end = st(next + 2.5);
        } else {
            infect_ship(u, target, infector);
        }
        let t = u.ships.ship_mut(target);
        t.beast.last_attack = now;
        t.beast.cascade_time = st(ld(tw.beast_cascade_delay) + ld(now));
        t.beast.cascade_points = tw.beast_cascade_points;
    } else {
        let health = u.ships.ship(target).health;
        let mut damage = st(ld(health) * ld(0.2f32));
        if damage < 2000.0 {
            damage = st(ld(health) - 1.0);
        }
        let player = u.ships.get(infector).map_or(0, |s| s.player) as i32;
        crate::damage::apply_damage_to_target(u, target, damage, BEAST_BULLET_TYPE as i32, crate::damage::how::KILLED_BY_PLAYER, player, None);
        if let Some(src) = u.ships.get_mut(source) {
            src.queued_attack = None;
        }
        if !u.single_player && u.ships.get(source).is_some_and(|s| s.ship_type == ShipType::bHeavyCruiser) {
            u.ships.ship_mut(source).queued_attack = Some(target);
        }
        if let Some(t) = u.ships.get_mut(target) {
            t.beast.last_attack = now;
            t.beast.cascade_time = st(ld(tw.beast_cascade_delay) + ld(now));
            t.beast.cascade_points = 0.0;
        }
    }
    let entry = CascadeEntry { bullet, source: Some(source), target: Some(target) };
    if let Some(c) = cascade_mut(u, infector) {
        c.entries.push(entry);
    }
    true
}

/// `beastCascadeUpdate` (0x42aab0), from the cascade types' housekeeping: fly at the target
/// (a raised sentinel field in the way becomes the target instead), shoot it when the gun is
/// charged, then let the beams jump on and forget the finished ones.
pub fn cascade_update(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.guns.is_empty() {
        return;
    }
    let Some(c) = s.beast.cascade.as_deref() else { return };
    match c.target {
        None => {
            if c.moving {
                let s = u.ships.ship_mut(id);
                s.autostabilize_ship = true;
                if let Some(c) = s.beast.cascade.as_deref_mut() {
                    c.moving = false;
                }
            }
        }
        Some(t0) => shoot_target(u, id, t0),
    }
    // The beams jump on.
    let entries = cascade(u, id).map(|c| c.entries.clone()).unwrap_or_default();
    for e in entries {
        let (Some(src), Some(bullet)) = (e.target, e.bullet) else { continue };
        let Some(ss) = u.ships.get(src) else { continue };
        let ct = ss.beast.cascade_time;
        if ct == 0.0 || !(ct < u.total_time_elapsed) {
            continue;
        }
        cascade_jump(u, id, src, bullet);
        if let Some(ss) = u.ships.get_mut(src) {
            ss.beast.cascade_time = 0.0;
        }
    }
    if let Some(c) = cascade_mut(u, id) {
        c.entries.retain(|e| e.bullet.is_some());
    }
}

fn shoot_target(u: &mut Universe, id: ShipId, t0: ShipId) {
    let me = u.ships.ship(id).clone();
    let Some(tgt) = u.ships.get(t0) else { return };
    let p = me.posinfo.position;
    let q = tgt.posinfo.position;
    let mut dir = Vector::new(st(ld(q.x) - ld(p.x)), st(ld(q.y) - ld(p.y)), st(ld(q.z) - ld(p.z)));
    let mut best = st(ld(dir.x) * ld(dir.x) + ld(dir.y) * ld(dir.y) + ld(dir.z) * ld(dir.z));
    dir.normalize();
    let mut target = t0;
    // A raised sentinel field between the ship and its target: the nearest sentinel of that
    // group is shot instead.
    let cmds: Vec<_> = u.commands.todo.clone();
    for cid in cmds {
        let Some(c) = u.commands.get(cid) else { continue };
        if c.attributes & crate::command::attr::PROTECTING == 0 {
            continue;
        }
        let sel = c.selection.clone();
        let Some(first) = sel.first().and_then(|&f| u.ships.get(f)) else { continue };
        if first.player == me.player {
            continue;
        }
        let blocked = sel.iter().any(|&x| u.ships.get(x).is_some_and(|xs| is_sentinel(xs.ship_type)) && sentinel_field_up(u, x) && sentinel_field_crossed(u, x, p, dir));
        if !blocked {
            continue;
        }
        for &x in &sel {
            let Some(xs) = u.ships.get(x) else { continue };
            if !is_sentinel(xs.ship_type) {
                continue;
            }
            let r = xs.posinfo.position;
            let (dx, dy, dz) = (ld(p.x) - ld(r.x), ld(p.y) - ld(r.y), ld(p.z) - ld(r.z));
            let d = dx * dx + dy * dy + dz * dz;
            if d < ld(best) {
                best = st(d);
                target = x;
            }
        }
    }
    let tp = u.ships.ship(target).posinfo.position;
    {
        let s = u.ships.ship_mut(id);
        if let Some(c) = s.beast.cascade.as_deref_mut() {
            c.target = Some(target);
        }
        s.autostabilize_ship = false;
    }
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(tp), 0xf, 0.0, None);
    if gun_charge_fraction(u, id) == 1.0 {
        fire_at(u, id, target);
    }
    if let Some(c) = cascade_mut(u, id) {
        c.moving = true;
    }
}

/// The charged infection gun in range and on target fires; the target's infector becomes the
/// ship (and the ship its own), the beam hits ([`attack_target`]) and the special order ends.
fn fire_at(u: &mut Universe, id: ShipId, target: ShipId) {
    let statics = u.statics.clone();
    let me = u.ships.ship(id);
    let Some(guns) = statics.ship(me.ship_type).and_then(|st| st.guns.as_ref()) else { return };
    let p = me.posinfo.position;
    let q = u.ships.ship(target).posinfo.position;
    let (dx, dy, dz) = (ld(p.x) - ld(q.x), ld(p.y) - ld(q.y), ld(p.z) - ld(q.z));
    let dsq = st(dx * dx + dy * dy + dz * dz);
    let mut bullet = None;
    for (gi, gs) in guns.iter().enumerate() {
        if bullet.is_some() {
            break;
        }
        if crate::gun::static_bullet_type(gs, false) != BEAST_BULLET_TYPE {
            continue;
        }
        let r = ld(gs.bullet_length);
        if !(ld(dsq) < r * r) {
            continue;
        }
        if !crate::gun::orient_gimble(u, id, gi, target) {
            continue;
        }
        if u.ships.ship(id).guns.get(gi).map(|g| g.level) != Some(crate::bullet::weapon::BEASTINFECT) {
            continue;
        }
        let before = u.bullet_list.len();
        if crate::gun::shoot(u, id, gi, Some(target)) && u.bullet_list.len() > before {
            bullet = u.bullet_list.last().copied();
        }
    }
    let Some(bullet) = bullet else { return };
    let player = u.ships.ship(id).player;
    let (old_infector, old_player) = {
        let t = u.ships.ship_mut(target);
        let saved = (t.beast.infector, t.beast.player);
        t.beast.infector = Some(id);
        t.beast.player = Some(player);
        saved
    };
    {
        let s = u.ships.ship_mut(id);
        s.beast.infector = Some(id);
        s.beast.player = Some(player);
    }
    if attack_target(u, id, target, Some(bullet)) {
        // (The infection sound plays.)
        u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: crate::ships::Effect::Beam { target } });
    } else if let Some(t) = u.ships.get_mut(target) {
        t.beast.infector = old_infector;
        t.beast.player = old_player;
    }
    if let Some(c) = cascade_mut(u, id) {
        c.target = None;
    }
    crate::command::remove_ships_from_doing_stuff(u, &[id]);
    u.ships.ship_mut(id).ship_is_attacking2 = false;
}

/// The beam jumps from `source` to the nearest eligible ship within the cascade distance (or
/// the source's points are gone).
fn cascade_jump(u: &mut Universe, id: ShipId, source: ShipId, bullet: BulletId) {
    let dist = u.tweaks.base.beast_cascade_distance;
    let Some(next) = find_nearest_cascade_target(u, source) else {
        u.ships.ship_mut(source).beast.cascade_points = 0.0;
        return;
    };
    let p = u.ships.ship(source).posinfo.position;
    let q = u.ships.ship(next).posinfo.position;
    let (dx, dy, dz) = (ld(p.x) - ld(q.x), ld(p.y) - ld(q.y), ld(p.z) - ld(q.z));
    if !(dx * dx + dy * dy + dz * dz < ld(dist) * ld(dist)) {
        u.ships.ship_mut(source).beast.cascade_points = 0.0;
        return;
    }
    let me_player = u.ships.ship(id).player;
    let n = u.ships.ship(next);
    let (old_infector, old_player) = (n.beast.infector, n.beast.player);
    let takeable = old_infector.and_then(|i| u.ships.get(i)).is_none_or(|i| i.player != me_player) || n.special_flags2 & special2::NO_SUPPORT_UNITS != 0;
    if !takeable {
        return;
    }
    let (inf, pl) = {
        let me = u.ships.ship(id);
        (me.beast.infector, me.beast.player)
    };
    {
        let n = u.ships.ship_mut(next);
        n.beast.infector = inf;
        n.beast.player = pl;
    }
    let gun = u.bullets.get(bullet).and_then(|b| b.gun);
    let new_bullet = gun.and_then(|gi| crate::bullet::create(u, id, gi, Some(source), Some(next), 1.0));
    let Some(nb) = new_bullet else { return };
    if attack_target(u, source, next, Some(nb)) {
        let points = u.ships.ship(source).beast.cascade_points;
        let cost = u.statics.ship(u.ships.ship(next).ship_type).map_or(0.0, |s| s.beast_attack_cascade_cost);
        u.ships.ship_mut(next).beast.cascade_points = st(ld(points) - ld(cost));
        u.ships.ship_mut(source).beast.cascade_points = 0.0;
        // (The cascade sound plays.)
        u.special_events.push(crate::universe::SpecialEvent::Special { ship: source, effect: crate::ships::Effect::Beam { target: next } });
    } else {
        u.ships.ship_mut(source).beast.cascade_points = 0.0;
        let n = u.ships.ship_mut(next);
        n.beast.infector = old_infector;
        n.beast.player = old_player;
    }
}

/// `beastCascadeEffectUpdate` (0x429f30), the beam's per-frame update in
/// `physUpdateBulletPosVel`: the beam runs from its source ship (from the gun of the capital
/// ship when it is the source) to its target.
pub fn cascade_effect_update(u: &mut Universe, bid: BulletId) {
    let Some(b) = u.bullets.get_mut(bid) else { return };
    b.length_vec = Vector::new(0.1, 0.1, 0.1);
    let (Some(owner), Some(gi)) = (b.owner, b.gun) else { return };
    let target = b.target;
    let Some(entry) = cascade(u, owner).and_then(|c| c.entries.iter().find(|e| e.bullet == Some(bid)).copied()) else { return };
    let (Some(src), Some(tgt)) = (entry.source, target) else { return };
    let (Some(ss), Some(ts)) = (u.ships.get(src), u.ships.get(tgt)) else { return };
    let pos = if type_has_cascade(ss.ship_type) {
        let o = u.ships.ship(owner);
        let Some(gs) = u.statics.ship(o.ship_type).and_then(|st| st.guns.as_ref()?.get(gi)) else { return };
        let g = o.rotinfo.coordsys.mul_vec(gs.position);
        Vector::new(st(ld(g.x) + ld(o.posinfo.position.x)), st(ld(g.y) + ld(o.posinfo.position.y)), st(ld(g.z) + ld(o.posinfo.position.z)))
    } else {
        ss.posinfo.position
    };
    let tp = ts.posinfo.position;
    let len = Vector::new(st(ld(tp.x) - ld(pos.x)), st(ld(tp.y) - ld(pos.y)), st(ld(tp.z) - ld(pos.z)));
    let mut h = len;
    h.normalize();
    let b = u.bullets.bullet_mut(bid);
    b.posinfo.position = pos;
    b.length_vec = len;
    b.heading = h;
    b.coordsys = hwc_math::Matrix::coord_sys_from_heading(h);
}

// --- the module wrappers

/// `HeavyCruiserInit` / `CarrierInit` / `BeastMothershipInit` (cascade part) and
/// `MothershipInit`'s cascade part ([`mothership_init`]).
pub fn init(u: &mut Universe, id: ShipId) {
    if ship_has_cascade(u, id) {
        cascade_init(u, id);
    }
    // BeastMothershipInit: a Beast mothership starts flickering (the header is shared, the
    // check is for the Beast mothership proper).
    let now = u.total_time_elapsed;
    if let Some(s) = u.ships.get_mut(id) {
        if s.ship_type == ShipType::bMothership {
            s.flicker.delay = -1.0;
            s.flicker.state = 1;
            let next = ld(now) + 1.0;
            s.flicker.next = st(next);
            s.flicker.end = st(next + 2.5);
        }
    }
}

/// The cascade part of `MothershipInit` (for the mothership module).
pub fn mothership_init(u: &mut Universe, id: ShipId) {
    if ship_has_cascade(u, id) {
        cascade_init(u, id);
    }
}

/// `BeastMothershipSpecialTarget` (0x5aacb0), shared by heavy cruisers and motherships.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if ship_has_cascade(u, id) {
        return choose_target(u, id, targets);
    }
    true
}

/// `CarrierSpecialTarget` (0x5a9c10): a Beast carrier with something to infect chooses its
/// target; otherwise carriers refuel and repair the targets (`refuelRepairShips`, which is
/// not simulated: done at once).
pub fn carrier_special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    if ship_has_cascade(u, id) && has_target(u, id, targets) {
        choose_target(u, id, targets);
        return false;
    }
    true
}

/// `BeastMothershipOrderChanged` / `CarrierOrderChanged`.
pub fn order_changed(u: &mut Universe, id: ShipId) {
    if ship_has_cascade(u, id) {
        clear_target(u, id);
    }
}

/// `HeavyCruiserHouseKeep` / `CarrierHouseKeep` / `BeastMothershipHouseKeep` (cascade part;
/// the depot housekeeping runs generically) and `MothershipUpdateDamageMode`'s.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    if ship_has_cascade(u, id) {
        cascade_update(u, id);
    }
}

/// `...RemoveShipReferences` (cascade part).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if ship_has_cascade(u, id) {
        cascade_remove_ship(u, id, gone);
    }
}

/// `...Died` (cascade part).
pub fn died(u: &mut Universe, id: ShipId) {
    if ship_has_cascade(u, id) {
        cascade_close(u, id);
    }
}

/// A ship left the universe: nobody keeps it as their infector
/// (`univRemoveAllShipReferencesFromSpecialShip`, +0x4e8).
pub fn ship_gone(u: &mut Universe, id: ShipId, gone: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        if s.beast.infector == Some(gone) {
            s.beast.infector = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_types() {
        assert!(type_has_cascade(ShipType::bHeavyCruiser));
        assert!(type_has_cascade(ShipType::bCarrier));
        assert!(type_has_cascade(ShipType::bMothership));
        assert!(type_has_cascade(ShipType::dBeastMothership));
        assert!(!type_has_cascade(ShipType::kHeavyCruiser));
        assert!(!type_has_cascade(ShipType::sCarrier));
    }
}
