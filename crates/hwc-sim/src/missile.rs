//! Missiles and mines (`OBJ_MissileType`): launching from missile and mine launchers
//! (`missileShoot` 0x4860f0), level mines (`scriptSetMineCB` 0x49f580), guidance
//! (`aishipGuideMissile` 0x415e70, `aishipFlyMissileToTarget` 0x415b70,
//! `shouldMissileLoseTarget` 0x415c90, `aishipGuideMine` 0x416420, `aishipFlyMineToTarget`
//! 0x415fb0, `aishipmineaquiretarget` 0x4162d0), their physics (`physUpdateObjPosVelMissile`
//! 0x4e0820), collisions with ships and mines (`collCheckMissileShipColl` 0x438e10,
//! `collCheckMissileMineColl` 0x439690), the hit (`univMissileCollidedWithTarget` 0x542120)
//! and their removal (`AddMissileToDeleteMissileList`, `DeleteDeadMissile` 0x5448a0), with
//! Cataclysm's capped remaining life when a missile loses its target
//! (`univMissileCapRemainingLife` 0x5425d0).
//!
//! A missile and a mine are the same object; `kind` tells them apart (`missileType` +0x1e0).
//! Field docs give the offset in the original 600-byte object.

use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{eq_or_unordered, ld, lt_or_unordered, st, Ext};
use hwc_math::random::RAN_GAME;
use hwc_math::{Matrix, Vector};

use crate::blobs::Objects;
use crate::object::{sof, CollInfo, MissileId, NonStats, ObjRef, PosInfo, RotInfo, ShipId};
use crate::statics::{GunType, MissileStaticInfo, ShipRace};
use crate::universe::{SpecialEvent, Universe, DEFAULT_SHIP_MATRIX};

/// `missileType` (+0x1e0).
pub mod kind {
    pub const MISSILE: u32 = 0;
    pub const MINE: u32 = 1;
}

/// The remaining life a missile keeps when it loses its target (0x8ab928).
pub const CAPPED_REMAINING_LIFE: f32 = 5.0;
/// Squared distance below which a missile leads its target (0x7436b0, 250 m).
const LEAD_DIST_SQ: f32 = 62_500.0;
/// Squared speed above which a missile leads its target (0x7204d8, 200 m/s).
const LEAD_MIN_SPEED_SQ: f32 = 40_000.0;
/// Frame mask of a mine's target search (0x8a2284).
const MINE_ACQUIRE_RATE: u32 = 15;
/// Damage of a level mine (`scriptSetMineCB`).
const LEVEL_MINE_DAMAGE: f32 = 5000.0;
/// `REALlyBig`.
const REALLY_BIG: f32 = f32::from_bits(0x6258_d727);

/// Where a missile launcher's mine goes when its layer drops a wall (the minelayer's custom
/// state, +0x658.. in the layer; see `ships::minelayer`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForcedMine {
    /// The layer's mine formation (+0x6b0 in the layer).
    pub formation: u32,
    /// Wall position (+0x65c..+0x664).
    pub wall_point: Vector,
    /// +0x6b4 / +0x6b8, copied to the mine (+0x248 / +0x24c).
    pub extra: [f32; 2],
    /// The layer's forced-drop damage range (custom statics +0x74 lo, +0x70 hi) and the
    /// forced mine's lifetime (+0x7c).
    pub damage_lo: f32,
    pub damage_hi: f32,
    pub lifetime: f32,
}

/// A missile or mine.
#[derive(Debug, Clone, PartialEq)]
pub struct Missile {
    pub id: MissileId,
    /// +0x14 (`ROTATABLE | IMPACTABLE | NOT_BUMPABLE | TARGETABLE` at creation; `DEAD` 0x800).
    pub flags: u32,
    /// +0x1e0.
    pub kind: u32,
    /// +0x1e8: the race whose missile or mine statics it uses.
    pub race: ShipRace,
    pub posinfo: PosInfo,
    pub rotinfo: RotInfo,
    pub coll: CollInfo,
    /// +0x50 / +0x54.
    pub coll_optimize_dist: f32,
    pub coll_my_blob: Option<crate::object::BlobId>,
    /// +0x170.
    pub health: f32,
    /// +0x17c..: the guidance statics.
    pub nonstat: NonStats,
    /// +0x1b8.
    pub max_velocity_copy: f32,
    /// +0x1c4: position at the end of the last physics step (the trail's head).
    pub trail_position: Vector,
    /// +0x1e4: bullet type (damage table row).
    pub bullet_type: u32,
    /// +0x1ec: distance it can travel in one frame.
    pub travel_dist: f32,
    /// +0x1f0 / +0x1f4.
    pub time_lived: f32,
    pub total_lifetime: f32,
    /// +0x1f8: owning player (`None` for neutral level mines in multiplayer without player 7).
    pub player: Option<usize>,
    /// +0x1fc / +0x200.
    pub owner: Option<ShipId>,
    pub target: Option<ShipId>,
    /// +0x208.
    pub damage: f32,
    /// +0x20c: how often `shouldMissileLoseTarget` has run.
    pub lose_target_checks: i32,
    /// +0x210: `missileID`.
    pub number: u16,
    /// +0x214 (6 for level mines; written, not read by the simulation).
    pub level_mine_tag: u32,
    /// +0x220: the mine's state (0 new, 1 leaving its layer, 2 drifting to a stop, 3 chasing,
    /// 4 flying to its wall point).
    pub mine_state: u32,
    /// +0x224: the wall point a forced mine flies to.
    pub wall_point: Vector,
    /// +0x230: dropped as part of a wall (`maxvelocity_FORCED`, forced ranges).
    pub forced: bool,
    /// +0x244: the mine formation it belongs to.
    pub formation: Option<u32>,
    /// +0x248 / +0x24c.
    pub formation_extra: [f32; 2],
    /// +0x250: the owner's colour scheme.
    pub color_scheme: u8,
}

impl Missile {
    pub fn is_dead(&self) -> bool {
        self.flags & sof::DEAD != 0
    }

    pub fn heading(&self) -> Vector {
        self.rotinfo.coordsys.col(2)
    }

    fn blank(id: MissileId, race: ShipRace, kind: u32) -> Self {
        Missile {
            id,
            flags: sof::ROTATABLE | sof::IMPACTABLE | sof::NOT_BUMPABLE | sof::TARGETABLE,
            kind,
            race,
            posinfo: PosInfo::default(),
            rotinfo: RotInfo::default(),
            coll: CollInfo::default(),
            coll_optimize_dist: 0.0,
            coll_my_blob: None,
            health: 0.0,
            nonstat: NonStats::default(),
            max_velocity_copy: 0.0,
            trail_position: Vector::ZERO,
            bullet_type: 0,
            travel_dist: 0.0,
            time_lived: 0.0,
            total_lifetime: 0.0,
            player: None,
            owner: None,
            target: None,
            damage: 0.0,
            lose_target_checks: 0,
            number: 0,
            level_mine_tag: 0,
            mine_state: 0,
            wall_point: Vector::ZERO,
            forced: false,
            formation: None,
            formation_extra: [0.0; 2],
            color_scheme: 0,
        }
    }
}

/// Missiles by id (ids are never reused).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MissileTable {
    slots: Vec<Option<Missile>>,
}

impl MissileTable {
    pub fn get(&self, id: MissileId) -> Option<&Missile> {
        self.slots.get(id.0 as usize).and_then(Option::as_ref)
    }

    pub fn get_mut(&mut self, id: MissileId) -> Option<&mut Missile> {
        self.slots.get_mut(id.0 as usize).and_then(Option::as_mut)
    }

    pub fn missile(&self, id: MissileId) -> &Missile {
        self.get(id).expect("missile id refers to a live missile")
    }

    pub fn missile_mut(&mut self, id: MissileId) -> &mut Missile {
        self.get_mut(id).expect("missile id refers to a live missile")
    }

    fn insert(&mut self, m: Missile) {
        let i = m.id.0 as usize;
        if self.slots.len() <= i {
            self.slots.resize(i + 1, None);
        }
        self.slots[i] = Some(m);
    }

    fn remove(&mut self, id: MissileId) -> Option<Missile> {
        self.slots.get_mut(id.0 as usize).and_then(Option::take)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Missile> {
        self.slots.iter().flatten()
    }
}

/// The statics of a missile or mine.
pub fn statics_of<'a>(u: &'a Universe, m: &Missile) -> Option<&'a MissileStaticInfo> {
    let table = if m.kind == kind::MINE { &u.statics.mines } else { &u.statics.missiles };
    table.get(m.race.index()).and_then(Option::as_ref)
}

/// `univUpdateObjRotInfo` for a missile: collision centre and box.
fn update_rot_info(m: &mut Missile, st_: &MissileStaticInfo) {
    if m.flags & sof::IMPACTABLE == 0 {
        return;
    }
    m.coll.coll_offset = m.rotinfo.coordsys.mul_vec(st_.header.coll.coll_sphere_offset);
    m.coll.coll_position = Vector::new(
        st(ld(m.posinfo.position.x) + ld(m.coll.coll_offset.x)),
        st(ld(m.coll.coll_offset.y) + ld(m.posinfo.position.y)),
        st(ld(m.coll.coll_offset.z) + ld(m.posinfo.position.z)),
    );
    if let Some(r) = crate::collision::coll_rect_corners(&m.rotinfo.coordsys, &st_.header.coll) {
        m.coll.rect = r;
    }
}

/// Copies the guidance statics into a new object (the shared part of every creator).
fn init_from_statics(m: &mut Missile, st_: &MissileStaticInfo) {
    m.health = st_.health.max_health;
    m.nonstat = NonStats { thrust_strength: st_.guidance.thrust_strength, rot_strength: st_.guidance.rot_strength, turn_speed: st_.guidance.turn_speed };
    m.max_velocity_copy = st_.header.max_velocity;
    m.posinfo.is_moving = 1;
    m.posinfo.havent_calculated_dist = true;
}

/// Hands out the next `missileID` (`universe.missileNumber`).
fn next_id(u: &mut Universe) -> (MissileId, u16) {
    let n = u.missile_number;
    u.missile_number = u.missile_number.wrapping_add(1);
    let id = MissileId(u.missile_slots);
    u.missile_slots += 1;
    (id, n)
}

/// Adds a new missile to the blobs and the universe lists.
fn link(u: &mut Universe, id: MissileId, target_blob: Option<crate::object::BlobId>) {
    let statics = u.statics.clone();
    let mut objs = Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
    match target_blob {
        Some(b) if u.blobs.get(b).is_some() => u.blobs.add_to_specific(b, ObjRef::Missile(id), &mut objs, &statics),
        _ => u.blobs.add_to_nearest(ObjRef::Missile(id), &mut objs, &statics),
    }
    u.space_obj_list.push(ObjRef::Missile(id));
    u.impactable_list.push(ObjRef::Missile(id));
    u.missile_list.push(id);
}

/// `missileShoot` (0x4860f0): a launcher fires a missile (missile launchers) or drops a mine
/// (mine launchers) at `target`. `forced` describes a mine dropped into a wall.
pub fn shoot(u: &mut Universe, id: ShipId, gi: usize, target: Option<ShipId>, forced: Option<ForcedMine>) -> Option<MissileId> {
    let statics = u.statics.clone();
    let s = u.ships.get(id)?;
    let stat = statics.ship(s.ship_type)?;
    let gs = stat.guns.as_ref()?.get(gi)?;
    let launcher = gs.gun_type;
    if crate::salvage::is_in_depot(u, id) {
        return None;
    }
    if let Some(t) = target {
        let t_ship = u.ships.get(t)?;
        let _ = t_ship;
        if crate::salvage::is_in_depot(u, t) {
            return None;
        }
        if launcher == Some(GunType::MissileLauncher) {
            let cs = s.rotinfo.coordsys;
            let dir = cs.mul_vec(s.guns[gi].heading);
            let gp = cs.mul_vec(gs.position);
            let p = s.posinfo.position;
            let pos = Vector::new(st(ld(gp.x) + ld(p.x)), st(ld(gp.y) + ld(p.y)), st(ld(gp.z) + ld(p.z)));
            if !crate::collision::line_of_sight(u, id, Some(t), pos, dir) {
                return None;
            }
        }
    }
    let now = u.total_time_elapsed;
    let dt = u.phys_time_elapsed;
    {
        let g = &mut u.ships.ship_mut(id).guns[gi];
        g.num_missiles -= 1;
        g.last_time_fired = now;
    }
    let s = u.ships.ship(id).clone();
    let race = s.ship_race;
    let is_missile = launcher == Some(GunType::MissileLauncher);
    let k = if is_missile { kind::MISSILE } else { kind::MINE };
    let table = if is_missile { &statics.missiles } else { &statics.mines };
    let mst = table.get(race.index()).and_then(Option::as_ref)?;
    let (mid, number) = next_id(u);
    let mut m = Missile::blank(mid, race, k);
    m.number = number;
    let gun = s.guns[gi];
    let upgraded = crate::gun::upgraded(&gun, gs);
    if is_missile {
        m.travel_dist = st(ld(dt) * ld(mst.header.max_velocity));
    } else if let Some(f) = forced {
        m.mine_state = 1;
        m.formation = Some(f.formation);
        m.wall_point = f.wall_point;
        m.formation_extra = f.extra;
        m.travel_dist = st(ld(dt) * ld(mst.max_velocity_forced));
        m.forced = true;
    } else {
        m.travel_dist = st(ld(dt) * ld(mst.header.max_velocity));
    }
    m.color_scheme = s.color_scheme as u8;
    init_from_statics(&mut m, mst);
    m.bullet_type = crate::gun::static_bullet_type(gs, upgraded);
    m.player = Some(s.player);
    m.owner = Some(id);
    m.target = target;
    if m.forced {
        let f = forced.expect("forced mine");
        let r = u.randy.random(RAN_GAME);
        // (The random number is loaded exactly, as a 64-bit integer.)
        m.damage = st((ld(f.damage_hi) - ld(f.damage_lo)) * ld(2.328_306_4e-10) * f64::from(r) + ld(f.damage_lo));
        m.total_lifetime = f.lifetime;
    } else {
        let armour = target.and_then(|t| u.ships.get(t)).map_or(0, |t| t.armour_level);
        let table_dmg = crate::damage::weapon_damage(m.bullet_type, armour);
        let mult = crate::damage::damage_multiplier(u, &s);
        m.damage = st(mult * ld(table_dmg) * ld(gs.damage_mult));
        m.total_lifetime = gs.bullet_life_time;
    }
    if s.special_flags & crate::object::special::KAS_DAMAGE_MODIFIER != 0 {
        m.damage = st(ld(s.damage_modifier) * ld(m.damage));
    }
    // Position: muzzle plus the barrel along the launch direction.
    let cs = s.rotinfo.coordsys;
    let h = cs.mul_vec(gun.heading);
    let gp = cs.mul_vec(gs.position);
    let p = s.posinfo.position;
    let barrel = ld(gs.barrel_length);
    let px = st(ld(gp.x) + ld(p.x));
    let py = st(ld(gp.y) + ld(p.y));
    let pz = st(ld(gp.z) + ld(p.z));
    m.posinfo.position = Vector::new(st(ld(h.x) * barrel + ld(px)), st(ld(h.y) * barrel + ld(py)), st(ld(h.z) * barrel + ld(pz)));
    let speed = crate::gun::bullet_speed(&gun, gs);
    let v = s.posinfo.velocity;
    m.posinfo.velocity = Vector::new(
        st(ld(v.x) + ld(st(ld(speed) * ld(h.x)))),
        st(ld(v.y) + ld(st(ld(speed) * ld(h.y)))),
        st(ld(v.z) + ld(st(ld(speed) * ld(h.z)))),
    );
    m.rotinfo.coordsys = if !is_missile {
        // Mines leave sideways: with two launchers, each points its own way.
        let c = cs.m;
        if s.guns.len() < 2 {
            cs
        } else if gi == 0 {
            Matrix { m: [-c[3], -c[4], -c[5], c[0], c[1], c[2], c[6], c[7], c[8]] }
        } else {
            Matrix { m: [c[3], c[4], c[5], -c[0], -c[1], -c[2], c[6], c[7], c[8]] }
        }
    } else {
        Matrix::coord_sys_from_heading(h)
    };
    update_rot_info(&mut m, mst);
    m.trail_position = m.posinfo.position;
    let target_blob = target.and_then(|t| u.ships.get(t)).and_then(|t| t.coll_my_blob);
    u.missiles.insert(m);
    link(u, mid, target_blob);
    if let Some(f) = forced.filter(|_| !is_missile) {
        crate::ships::minelayer::mine_added(u, f.formation, mid);
    }
    if let Some(t) = target.and_then(|t| u.ships.get_mut(t)) {
        t.firing_at_us = Some(id);
        t.recently_fired_upon = crate::object::RECENT_ATTACK_DURATION;
    }
    u.special_events.push(SpecialEvent::MissileFired { ship: id, gun: gi, missile: mid });
    Some(mid)
}

/// `scriptSetMineCB` (0x49f580): a mine placed by a level (`Mine x,y,z,angle`). It uses the
/// derelict race's mine statics and belongs to player 1 in single player (player 7
/// otherwise). The original computes a sphere-relative position for levels without
/// absolute coordinates but then places the mine at the raw coordinates; so does this.
pub fn add_level_mine(u: &mut Universe, position: Vector) -> Option<MissileId> {
    let statics = u.statics.clone();
    let race = ShipRace::Derelict;
    let mst = statics.mines.get(race.index()).and_then(Option::as_ref)?;
    let (mid, number) = next_id(u);
    let mut m = Missile::blank(mid, race, kind::MINE);
    m.number = number;
    m.level_mine_tag = 6;
    let owner = if u.single_player { 1 } else { 7 };
    m.player = (owner < u.players.len()).then_some(owner);
    m.travel_dist = st(ld(mst.header.max_velocity) * 0.0625);
    init_from_statics(&mut m, mst);
    m.posinfo.is_moving = 0;
    m.total_lifetime = REALLY_BIG;
    m.bullet_type = 8;
    m.damage = LEVEL_MINE_DAMAGE;
    m.posinfo.position = position;
    m.rotinfo.coordsys = DEFAULT_SHIP_MATRIX;
    update_rot_info(&mut m, mst);
    m.posinfo.is_moving = 1;
    m.trail_position = position;
    u.missiles.insert(m);
    link(u, mid, None);
    Some(mid)
}

/// The per-frame missile loop of `univUpdate`: guidance (missiles or mines), physics, and the
/// ones whose life is over go on the delete list.
pub fn update_all(u: &mut Universe) {
    let list = u.missile_list.clone();
    for mid in list {
        let Some(m) = u.missiles.get(mid) else { continue };
        if m.flags & sof::DONT_APPLY_PHYSICS != 0 {
            continue;
        }
        let expired = match m.kind {
            kind::MISSILE => guide_missile(u, mid),
            kind::MINE => guide_mine(u, mid),
            _ => false,
        };
        physics(u, mid);
        if expired && !u.missiles.missile(mid).is_dead() {
            add_to_delete_list(u, mid, -1);
        }
    }
}

/// `aishipGuideMissile` (0x415e70): steer at the target (dropping it when it cloaks or, near
/// it, by chance), point along the velocity, age. True when the missile's life is over.
fn guide_missile(u: &mut Universe, mid: MissileId) -> bool {
    let m = u.missiles.missile(mid);
    if m.is_dead() {
        return false;
    }
    if let Some(t) = m.target {
        fly_missile_to_target(u, mid, t);
        let cloaked = u.ships.get(t).is_some_and(crate::cloak::is_cloaked);
        let tw = &u.tweaks.combat;
        let m = u.missiles.missile(mid);
        let lose = cloaked
            || (m.lose_target_checks < tw.missile_num_times_check
                && tw.missile_target_loss_rate & u.update_counter == u32::from(m.number) & u.update_counter
                && should_lose_target(u, mid));
        if lose {
            u.missiles.missile_mut(mid).target = None;
        }
    }
    let dt = u.phys_time_elapsed;
    let m = u.missiles.missile_mut(mid);
    let v = m.posinfo.velocity;
    let z = |x: f32| eq_or_unordered(ld(x), 0.0);
    if !(z(v.x) && z(v.y) && z(v.z)) {
        let mut h = v;
        h.normalize();
        m.rotinfo.coordsys = Matrix::coord_sys_from_heading(h);
        m.rotinfo.rotspeed = Vector::ZERO;
        m.rotinfo.torque = Vector::ZERO;
    }
    m.posinfo.is_moving = 1;
    let lived = ld(dt) + ld(m.time_lived);
    m.time_lived = st(lived);
    !(lived <= ld(m.total_lifetime) || lived.is_nan())
}

/// `aishipFlyMissileToTarget` (0x415b70): fly at the target's collision centre, leading it
/// once within 250 m when going faster than 200 m/s, at full speed.
fn fly_missile_to_target(u: &mut Universe, mid: MissileId, tid: ShipId) {
    let Some(t) = u.ships.get(tid) else { return };
    let (tc, tv) = (t.coll.coll_position, t.posinfo.velocity);
    let m = u.missiles.missile(mid);
    let Some(mst) = statics_of(u, m) else { return };
    let max_vel = mst.header.max_velocity;
    let mass = mst.header.mass;
    let p = m.posinfo.position;
    let dx = st(ld(tc.x) - ld(p.x));
    let dy = st(ld(tc.y) - ld(p.y));
    let dz_ext = ld(tc.z) - ld(p.z);
    let dz = st(dz_ext);
    let dist_sq_ext = (dz_ext * ld(dz) + ld(dy) * ld(dy)) + ld(dx) * ld(dx);
    let dist_sq = st(dist_sq_ext);
    let mut d = Vector::new(dx, dy, dz);
    if !(dist_sq_ext > ld(LEAD_DIST_SQ)) {
        let v = m.posinfo.velocity;
        let speed_sq = (ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z);
        if !lt_or_unordered(speed_sq, ld(LEAD_MIN_SPEED_SQ)) {
            let speed = st(fsqrt(speed_sq));
            let k = fsqrt(ld(dist_sq)) / ld(speed);
            let lx = st(k * ld(tv.x));
            let ly = st(k * ld(tv.y));
            let lz = k * ld(tv.z);
            d = Vector::new(st(ld(lx) + ld(dx)), st(ld(ly) + ld(dy)), st(lz + ld(dz)));
        }
    }
    cap_min_max(&mut d, max_vel, max_vel);
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let m = u.missiles.missile_mut(mid);
    let (v, cs, caps) = (m.posinfo.velocity, m.rotinfo.coordsys, m.nonstat.thrust_strength);
    crate::aitrack::velocity_vector_on(&mut m.posinfo.force, &cs, v, mass, &caps, true, &tw, dt, d);
}

/// `vecCapMinMaxVector` (0x54b3b0) with the original's register use.
fn cap_min_max(v: &mut Vector, min: f32, max: f32) {
    let mag = fsqrt((ld(v.x) * ld(v.x) + ld(v.y) * ld(v.y)) + ld(v.z) * ld(v.z));
    if mag < ld(min) {
        let k = ld(min) / mag;
        v.x = st(k * ld(v.x));
        v.y = st(k * ld(v.y));
        v.z = st(k * ld(v.z));
    }
    if mag > ld(max) {
        let k = ld(max) / mag;
        v.x = st(k * ld(v.x));
        v.y = st(k * ld(v.y));
        v.z = st(k * ld(v.z));
    }
}

/// `shouldMissileLoseTarget` (0x415c90): close to a small ship the missile may miss, more
/// likely against evasive, dodging or manoeuvring targets.
fn should_lose_target(u: &mut Universe, mid: MissileId) -> bool {
    let m = u.missiles.missile(mid);
    let Some(t) = m.target.and_then(|t| u.ships.get(t)) else { return false };
    let Some(tstat) = u.statics.ship(t.ship_type) else { return false };
    if tstat.is_capital_ship {
        return false;
    }
    let tw = &u.tweaks.combat;
    let p = m.posinfo.position;
    let tp = t.posinfo.position;
    let dx = st(ld(tp.x) - ld(p.x));
    let dy = st(ld(tp.y) - ld(p.y));
    let dz = st(ld(tp.z) - ld(p.z));
    let lim = tw.missile_min_dist_before_target_loss;
    let neg = -lim;
    if !(neg <= dx && dx <= lim && neg <= dy && dy <= lim && neg <= dz && dz <= lim) {
        return false;
    }
    let mut chance = tw.missile_randomness[t.tactics.min(2)];
    if t.is_dodging {
        chance = st(ld(tw.missile_boost_if_dodging) + ld(chance));
    }
    if t.flightman != 0 {
        chance = st(ld(tw.missile_boost_if_flightman) + ld(chance));
    }
    u.missiles.missile_mut(mid).lose_target_checks += 1;
    let r = u.randy.random(RAN_GAME);
    ld(st(f64::from(r as f32) * ld(2.328_306_4e-10))) < ld(chance)
}

/// `aishipGuideMine` (0x416420): the mine state machine. True when its life is over.
fn guide_mine(u: &mut Universe, mid: MissileId) -> bool {
    let m = u.missiles.missile(mid).clone();
    if m.is_dead() {
        return false;
    }
    let Some(mst) = statics_of(u, &m).cloned() else { return false };
    // A mine's target can only be a ship here (resources, nebulae and missiles are dropped).
    let counter = u.update_counter;
    let my_frame = MINE_ACQUIRE_RATE & counter == u32::from(m.number) & MINE_ACQUIRE_RATE;
    match m.mine_state {
        1 => {
            let leave = match m.owner.and_then(|o| u.ships.get(o)) {
                None => true,
                Some(o) => {
                    let p = m.posinfo.position;
                    let op = o.posinfo.position;
                    // The differences stay in registers; summed z, y, then x.
                    let dx = ld(p.x) - ld(op.x);
                    let dy = ld(p.y) - ld(op.y);
                    let dz = ld(p.z) - ld(op.z);
                    let clear = crate::ships::minelayer::mine_clear_distance_sq(u, o.ship_type);
                    ld(clear) < (dz * dz + dy * dy) + dx * dx
                }
            };
            if leave {
                u.missiles.missile_mut(mid).mine_state = 4;
            }
        }
        2 => {
            stop_drift(u.missiles.missile_mut(mid), mst.mine_stop_friction);
            if my_frame {
                if u.missiles.missile(mid).target.is_none() {
                    let t = acquire_target(u, mid);
                    u.missiles.missile_mut(mid).target = t;
                    if t.is_some() {
                        u.missiles.missile_mut(mid).mine_state = 3;
                    }
                } else {
                    u.missiles.missile_mut(mid).mine_state = 3;
                }
            }
        }
        0 => u.missiles.missile_mut(mid).mine_state = 3,
        3 => {
            let back = if m.formation.is_some() { 4 } else { 2 };
            match m.target {
                None => u.missiles.missile_mut(mid).mine_state = back,
                Some(t) => {
                    if !fly_mine_to_target(u, mid, MineGoal::Ship(t)) {
                        u.missiles.missile_mut(mid).mine_state = back;
                    }
                }
            }
        }
        4 => {
            if my_frame {
                let mut t = u.missiles.missile(mid).target;
                if t.is_none() {
                    t = acquire_target(u, mid);
                    u.missiles.missile_mut(mid).target = t;
                }
                if t.is_some() {
                    u.missiles.missile_mut(mid).mine_state = 3;
                }
            }
            let mm = u.missiles.missile(mid);
            if !crate::aitrack::reached_destination_at(mm.posinfo.position, mm.wall_point, 10.0) {
                let wp = mm.wall_point;
                fly_mine_to_target(u, mid, MineGoal::Point(wp));
            } else {
                stop_drift(u.missiles.missile_mut(mid), mst.mine_stop_friction);
            }
        }
        _ => {}
    }
    let dt = u.phys_time_elapsed;
    let m = u.missiles.missile_mut(mid);
    m.posinfo.is_moving = 1;
    let lived = ld(dt) + ld(m.time_lived);
    m.time_lived = st(lived);
    !(lived <= ld(m.total_lifetime) || lived.is_nan())
}

/// The friction of a drifting mine: slow down, then stop once every component is below 1.
fn stop_drift(m: &mut Missile, friction: f32) {
    let v = m.posinfo.velocity;
    let small = |x: f32| !(x < -1.0) && !(1.0 < x);
    if small(v.x) && small(v.y) && small(v.z) {
        m.posinfo.velocity = Vector::ZERO;
        m.rotinfo.rotspeed = Vector::ZERO;
    } else {
        let f = ld(friction);
        m.posinfo.velocity = Vector::new(st(f * ld(v.x)), st(f * ld(v.y)), st(f * ld(v.z)));
        let r = m.rotinfo.rotspeed;
        m.rotinfo.rotspeed = Vector::new(st(f * ld(r.x)), st(f * ld(r.y)), st(f * ld(r.z)));
    }
}

/// What a mine flies at: a ship's collision centre or a point (the wall position).
#[derive(Debug, Clone, Copy)]
enum MineGoal {
    Ship(ShipId),
    Point(Vector),
}

/// `aishipFlyMineToTarget` (0x415fb0). False when the target is lost (out of range, cloaked,
/// disabled, or running away faster than the mine).
fn fly_mine_to_target(u: &mut Universe, mid: MissileId, goal: MineGoal) -> bool {
    let m = u.missiles.missile(mid);
    let Some(mst) = statics_of(u, m).cloned() else { return false };
    let speed = if m.forced { mst.max_velocity_forced } else { mst.header.max_velocity };
    let (gp, gv, gblob) = match goal {
        MineGoal::Ship(t) => match u.ships.get(t) {
            Some(s) => (s.coll.coll_position, s.posinfo.velocity, s.coll_my_blob),
            None => {
                u.missiles.missile_mut(mid).target = None;
                return false;
            }
        },
        MineGoal::Point(p) => (p, Vector::ZERO, None),
    };
    let p = m.posinfo.position;
    let mut d = Vector::new(st(ld(gp.x) - ld(p.x)), st(ld(gp.y) - ld(p.y)), st(ld(gp.z) - ld(p.z)));
    if !crate::aitrack::reached_destination_at(p, gp, mst.mine_range_forced) {
        u.missiles.missile_mut(mid).target = None;
        return false;
    }
    if let Some(t) = m.target.and_then(|t| u.ships.get(t)) {
        if crate::cloak::is_cloaked(t) || t.flags & sof::DISABLED != 0 {
            u.missiles.missile_mut(mid).target = None;
            return false;
        }
    }
    let ahead = (ld(d.x) * ld(gv.x) + ld(d.z) * ld(gv.z)) + ld(d.y) * ld(gv.y);
    let gv_sq = (ld(gv.z) * ld(gv.z) + ld(gv.y) * ld(gv.y)) + ld(gv.x) * ld(gv.x);
    if 0.0 < ahead && ld(speed) * ld(speed) < gv_sq {
        u.missiles.missile_mut(mid).target = None;
        return false;
    }
    // A mine chasing a ship moves into the ship's blob.
    if let Some(b) = gblob {
        let own = u.missiles.missile(mid).coll_my_blob;
        if own != Some(b) {
            if own.is_some() {
                u.blobs.object_died(ObjRef::Missile(mid));
                u.missiles.missile_mut(mid).coll_my_blob = None;
            }
            let statics = u.statics.clone();
            let mut objs = Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
            if u.blobs.get(b).is_some() {
                u.blobs.add_to_specific(b, ObjRef::Missile(mid), &mut objs, &statics);
            }
        }
    }
    d.normalize_to_length(speed);
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let m = u.missiles.missile_mut(mid);
    let (v, cs, caps) = (m.posinfo.velocity, m.rotinfo.coordsys, m.nonstat.thrust_strength);
    crate::aitrack::velocity_vector_on(&mut m.posinfo.force, &cs, v, mst.header.mass, &caps, true, &tw, dt, d);
    true
}

/// `getShipNearObjTok`: the ships within `range` (per axis) of the object's collision centre,
/// walking its blob, then the blobs after it and before it in the blob list.
fn ships_near(u: &Universe, point: Vector, own_blob: usize, own_dist: f32, range: f32) -> Vec<ShipId> {
    let mut out = Vec::new();
    let blobs = &u.blobs.blobs;
    let biggest = u.tweaks.blob.biggest_radius;
    let scan = |bi: usize, out: &mut Vec<ShipId>| {
        for &sid in &blobs[bi].ships {
            if let Some(s) = u.ships.get(sid) {
                if crate::aitrack::reached_destination_at(s.posinfo.position, point, range) {
                    out.push(sid);
                }
            }
        }
    };
    scan(own_blob, &mut out);
    let own = &blobs[own_blob];
    let own_reach = ld(own.sqrt_sort_distance) + ld(own_dist);
    for (bi, b) in blobs.iter().enumerate().skip(own_blob + 1) {
        if ld(range) < (ld(b.sqrt_sort_distance) - ld(biggest)) - own_reach {
            break;
        }
        if crate::aitrack::reached_destination_at(point, b.centre, st(ld(biggest) + ld(range))) {
            scan(bi, &mut out);
        }
    }
    for bi in (0..own_blob).rev() {
        let b = &blobs[bi];
        if ld(range) < own_reach - (ld(biggest) + ld(b.sqrt_sort_distance)) {
            break;
        }
        if crate::aitrack::reached_destination_at(point, b.centre, st(ld(range) + ld(b.radius))) {
            scan(bi, &mut out);
        }
    }
    out
}

/// `aishipmineaquiretarget` (0x4162d0): the heaviest enemy ship in range that the mine's
/// owner can see (ties broken at random).
fn acquire_target(u: &mut Universe, mid: MissileId) -> Option<ShipId> {
    let m = u.missiles.missile(mid);
    let mst = statics_of(u, m)?;
    let range = if m.forced { mst.mine_range_forced } else { mst.mine_range };
    let blob = m.coll_my_blob?;
    let player = m.player;
    let cand = ships_near(u, m.coll.coll_position, blob.0 as usize, m.coll_optimize_dist, range);
    let mut best: Option<ShipId> = None;
    for sid in cand {
        let s = u.ships.ship(sid);
        if !u.single_player && s.ship_race == ShipRace::Derelict {
            continue;
        }
        if Some(s.player) == player {
            continue;
        }
        if player.is_some_and(|p| u.players.get(p).is_some_and(|pl| pl.is_ally(s.player))) {
            continue;
        }
        if s.flags2 & crate::object::flags2::IGNORED_BY_MINES != 0 {
            continue;
        }
        let pidx = player.unwrap_or(usize::MAX);
        let ok = if crate::cloak::is_cloaked(s) {
            crate::visibility::visible_to_player(u, sid, pidx)
        } else {
            s.flags & sof::DISABLED == 0
                && (s.ship_type != crate::statics::ShipType::sLeech || crate::visibility::visible_to_player(u, sid, pidx))
                && (s.flags2 & crate::object::flags2::DISGUISED == 0 || !crate::ships::mimic::is_disguised_as_player(u, sid, pidx))
        };
        if !ok {
            continue;
        }
        let mass = |id: ShipId| u.statics.ship(u.ships.ship(id).ship_type).map_or(0.0, |st| st.header.mass);
        match best {
            None => best = Some(sid),
            Some(b) => {
                let (ms, mb) = (mass(sid), mass(b));
                let replace = if ms <= mb {
                    if ms == mb {
                        let r = u.randy.random(RAN_GAME);
                        r.wrapping_sub(1) & 1 != 0
                    } else {
                        false
                    }
                } else {
                    true
                };
                if replace {
                    best = Some(sid);
                }
            }
        }
    }
    best
}

/// `physUpdateObjPosVelMissile` (0x4e0820).
pub fn physics(u: &mut Universe, mid: MissileId) {
    let dt = u.phys_time_elapsed;
    let counter = u.update_counter;
    let m = u.missiles.missile(mid);
    let Some(mst) = statics_of(u, m).cloned() else { return };
    let h = &mst.header;
    let m = u.missiles.missile_mut(mid);
    let dte = ld(dt);
    let f = m.posinfo.force;
    let oom = ld(h.one_over_mass);
    let ax = oom * ld(f.x);
    let ay = st(oom * ld(f.y));
    let az = st(oom * ld(f.z));
    let v = &mut m.posinfo.velocity;
    v.x = st(ax * dte + ld(v.x));
    let ay2 = st(ld(ay) * dte);
    let az2 = st(ld(az) * dte);
    v.y = st(ld(ay2) + ld(v.y));
    v.z = st(ld(az2) + ld(v.z));
    if v.x.to_bits() | v.y.to_bits() | v.z.to_bits() == 0 {
        m.posinfo.is_moving = 0;
    } else {
        m.posinfo.is_moving = 1;
        let cap = if m.forced { mst.max_velocity_forced } else { h.max_velocity };
        m.posinfo.velocity.cap_sloppy(cap);
        let v = m.posinfo.velocity;
        let py = st(dte * ld(v.y));
        let pz = st(dte * ld(v.z));
        let p = &mut m.posinfo.position;
        p.x = st(dte * ld(v.x) + ld(p.x));
        p.y = st(ld(py) + ld(p.y));
        p.z = st(ld(pz) + ld(p.z));
    }
    let tq = m.rotinfo.torque;
    let z = |x: f32| eq_or_unordered(ld(x), 0.0);
    if !(z(tq.x) && z(tq.y) && z(tq.z)) {
        let moi = h.one_over_moment_of_inertia;
        let rx = ld(moi.x) * ld(tq.x);
        let ry = st(ld(moi.y) * ld(tq.y));
        let rz = st(ld(moi.z) * ld(tq.z));
        let ry2 = st(ld(ry) * dte);
        let rz2 = st(ld(rz) * dte);
        let r = &mut m.rotinfo.rotspeed;
        r.x = st(rx * dte + ld(r.x));
        r.y = st(ld(ry2) + ld(r.y));
        r.z = st(ld(rz2) + ld(r.z));
        m.rotinfo.rotspeed.cap_sloppy(h.max_rot);
    }
    let mut rotated = false;
    let rs = m.rotinfo.rotspeed;
    if !z(rs.x) {
        crate::physics::rotate(&mut m.rotinfo.coordsys, rs.x, dt, Matrix::rot_about_x);
        rotated = true;
    }
    if !z(rs.y) {
        crate::physics::rotate(&mut m.rotinfo.coordsys, rs.y, dt, Matrix::rot_about_y);
        rotated = true;
    }
    if !z(rs.z) {
        crate::physics::rotate(&mut m.rotinfo.coordsys, rs.z, dt, Matrix::rot_about_z);
        rotated = true;
    }
    if m.flags & sof::IMPACTABLE != 0 {
        if rotated {
            m.coll.coll_offset = m.rotinfo.coordsys.mul_vec(h.coll.coll_sphere_offset);
            if counter & 3 == 3 {
                if let Some(r) = crate::collision::coll_rect_corners(&m.rotinfo.coordsys, &h.coll) {
                    m.coll.rect = r;
                }
            }
        }
        let p = m.posinfo.position;
        m.trail_position = p;
        m.coll.coll_position = Vector::new(
            st(ld(m.coll.coll_offset.x) + ld(p.x)),
            st(ld(m.coll.coll_offset.y) + ld(p.y)),
            st(ld(m.coll.coll_offset.z) + ld(p.z)),
        );
    }
    m.rotinfo.torque = Vector::ZERO;
    m.posinfo.force = Vector::ZERO;
}

// --- collisions

/// The sphere-then-box test both missile walks make: does the missile's path this frame
/// enter the object's box? Returns the distance along the path and the face.
fn path_hits(missile: &Missile, centre: Vector, size_sqr: f32, coll: &crate::statics::header::StaticCollInfo, pos: Vector, cs: &Matrix) -> Option<(f32, i32)> {
    let h = missile.heading();
    let p = missile.posinfo.position;
    let dx = st(ld(centre.x) - ld(p.x));
    let dy = st(ld(centre.y) - ld(p.y));
    let dz = st(ld(centre.z) - ld(p.z));
    let proj = st((ld(h.x) * ld(dx) + ld(h.y) * ld(dy)) + ld(h.z) * ld(dz));
    let proj2 = st(ld(proj) * ld(proj));
    let v = st(ld(size_sqr) - (((ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz)) - ld(proj2)));
    let tr = missile.travel_dist;
    if !(0.0 < v && ld(proj2) - ld(v) < ld(tr) * ld(tr)) {
        return None;
    }
    let (t, side) = crate::collision::rect_line_at(coll, pos, cs, p, h, tr);
    (ld(t) >= 0.0).then_some((t, side))
}

/// The missile collisions of one blob, in the original's order: missiles against ships,
/// mines against ships, missiles against mines (resources and derelicts are not objects in
/// this simulation).
pub fn check_blob(u: &mut Universe, bi: usize) {
    check_against_ships(u, bi, false);
    check_against_ships(u, bi, true);
    check_against_mines(u, bi);
}

/// `collCheckMissileShipColl` (0x438e10): the blob's missiles (or mines) against its small,
/// then big ships, binary-searching the sorted distances like the bullet walk.
fn check_against_ships(u: &mut Universe, bi: usize, mines: bool) {
    let list = if mines { u.blobs.blobs[bi].mines.clone() } else { u.blobs.blobs[bi].missiles.clone() };
    let statics = u.statics.clone();
    for mid in list {
        let Some(m) = u.missiles.get(mid) else { continue };
        if m.is_dead() {
            continue;
        }
        let m = m.clone();
        let mut hit: Option<(ShipId, f32, i32)> = None;
        let mut field_hit: Option<ShipId> = None;
        'passes: for pass in 0..2 {
            let blob = &u.blobs.blobs[bi];
            let ships = if pass == 0 { blob.small_ships.clone() } else { blob.big_ships.clone() };
            let (max_small, max_big) = (blob.max_small_ship_size, blob.max_big_ship_size);
            let n = ships.len();
            if n == 0 {
                continue;
            }
            let reach = |max: f32| st(ld(max) + ld(m.travel_dist));
            let (maxd, start) = if pass == 0 {
                let maxd = reach(max_small);
                let (mut lo, mut hi, mut mid_i) = (0i32, n as i32 - 1, 0i32);
                while lo <= hi {
                    mid_i = (hi + lo) >> 1;
                    let d = ld(m.coll_optimize_dist) - ld(u.ships.ship(ships[mid_i as usize]).coll_optimize_dist);
                    if d <= ld(maxd) {
                        hi = mid_i - 1;
                    } else {
                        lo = mid_i + 1;
                    }
                }
                (maxd, mid_i as usize)
            } else {
                (reach(max_big), 0)
            };
            for &sid in ships.iter().skip(start) {
                let s = u.ships.ship(sid);
                let diff = st(ld(s.coll_optimize_dist) - ld(m.coll_optimize_dist));
                if ld(maxd) < ld(diff) {
                    break;
                }
                let Some(sst) = statics.ship(s.ship_type) else { continue };
                if !(ld(diff.abs()) <= ld(sst.header.coll.coll_sphere_size) + ld(m.travel_dist)) || m.owner == Some(sid) {
                    continue;
                }
                let (cp, pos, cs) = (s.coll.coll_position, s.posinfo.position, s.rotinfo.coordsys);
                if crate::ships::sentinel::missile_hits_field(u, sid, &m) {
                    field_hit = Some(sid);
                    break 'passes;
                }
                if let Some((t, side)) = path_hits(&m, cp, sst.header.coll.coll_sphere_size_sqr, &sst.header.coll, pos, &cs) {
                    // (The ray/mesh refinement of the hit is skipped, as for bullets.)
                    hit = Some((sid, t, side));
                    break 'passes;
                }
            }
        }
        if field_hit.is_some() {
            add_to_delete_list(u, mid, -1);
        } else if let Some((sid, t, side)) = hit {
            collided_with_ship(u, sid, mid, t, side);
            add_to_delete_list(u, mid, -1);
        }
    }
}

/// `collCheckMissileMineColl` (0x439690): missiles against the blob's mines.
fn check_against_mines(u: &mut Universe, bi: usize) {
    let missiles = u.blobs.blobs[bi].missiles.clone();
    let mines = u.blobs.blobs[bi].mines.clone();
    if mines.is_empty() {
        return;
    }
    let max_mine = u.statics.mine_max_diameter;
    for mid in missiles {
        let Some(m) = u.missiles.get(mid) else { continue };
        if m.is_dead() {
            continue;
        }
        let m = m.clone();
        let maxd = st(ld(max_mine) + ld(m.travel_dist));
        let n = mines.len();
        let (mut lo, mut hi, mut start) = (0i32, n as i32 - 1, 0i32);
        while lo <= hi {
            start = (hi + lo) >> 1;
            let d = ld(m.coll_optimize_dist) - ld(u.missiles.get(mines[start as usize]).map_or(0.0, |x| x.coll_optimize_dist));
            if d <= ld(maxd) {
                hi = start - 1;
            } else {
                lo = start + 1;
            }
        }
        for &nid in mines.iter().skip(start as usize) {
            let Some(mine) = u.missiles.get(nid) else { continue };
            let diff = st(ld(mine.coll_optimize_dist) - ld(m.coll_optimize_dist));
            if ld(maxd) < ld(diff) {
                break;
            }
            let Some(nst) = statics_of(u, mine) else { continue };
            if !(ld(diff.abs()) <= ld(nst.header.coll.coll_sphere_size) + ld(m.travel_dist)) {
                continue;
            }
            if let Some((t, side)) = path_hits(&m, mine.coll.coll_position, nst.header.coll.coll_sphere_size_sqr, &nst.header.coll, mine.posinfo.position, &mine.rotinfo.coordsys) {
                collided_with_mine(u, nid, mid, t, side);
                add_to_delete_list(u, mid, -1);
                break;
            }
        }
    }
}

/// The knock-back a missile gives what it hits (mass over a frame, random spin).
fn knock_back(u: &mut Universe, mid: MissileId) -> Option<(Vector, [Ext; 3], f32)> {
    let m = u.missiles.missile(mid);
    let mst = statics_of(u, m)?;
    let mass = mst.header.mass;
    let k = ld(mass) / ld(u.phys_time_elapsed);
    let v = m.posinfo.velocity;
    let push = Vector::new(st(ld(v.x) * k), st(ld(v.y) * k), st(ld(v.z) * k));
    let tw = u.tweaks.combat.clone();
    let r: [Ext; 3] = std::array::from_fn(|i| u.randy.frandom_between_ext(RAN_GAME, tw.bullet_rock[i][0], tw.bullet_rock[i][1]));
    Some((push, r, mass))
}

/// `univMissileCollidedWithTarget` (0x542120) for a ship: knock-back, side and friendly-fire
/// modifiers, the "under attack" state, then the damage.
pub fn collided_with_ship(u: &mut Universe, tid: ShipId, mid: MissileId, t: f32, side: i32) {
    let statics = u.statics.clone();
    let Some(ship) = u.ships.get(tid) else { return };
    let Some(sst) = statics.ship(ship.ship_type) else { return };
    let m = u.missiles.missile(mid).clone();
    if !sst.header.immobile && ship.flags & sof::NOT_BUMPABLE == 0 {
        if let Some((push, r, mass)) = knock_back(u, mid) {
            let s = u.ships.ship_mut(tid);
            s.rotinfo.torque.x = st(r[0] * ld(mass) + ld(s.rotinfo.torque.x));
            s.rotinfo.torque.y = st(r[1] * ld(mass) + ld(s.rotinfo.torque.y));
            s.rotinfo.torque.z = st(r[2] * ld(mass) + ld(s.rotinfo.torque.z));
            let f = &mut s.posinfo.force;
            f.x = st(ld(push.x) + ld(f.x));
            f.y = st(ld(push.y) + ld(f.y));
            f.z = st(ld(push.z) + ld(f.z));
        }
    }
    let mut damage = m.damage;
    if side >= 0 {
        damage = st(ld(damage) * ld(sst.coll_side_modifiers[side as usize]));
    }
    let position = m.posinfo.position.add_scaled(m.heading(), t);
    u.special_events.push(SpecialEvent::MissileHit { missile: mid, target: ObjRef::Ship(tid), position });
    let owner = m.owner.and_then(|o| u.ships.get(o)).map(|o| (o.id, o.player));
    match owner {
        Some((oid, oplayer)) => {
            let ff = u.tweaks.combat.friendly_fire_modifier;
            let ally = u.players.get(oplayer).is_some_and(|p| p.is_ally(u.ships.ship(tid).player));
            let s = u.ships.ship_mut(tid);
            if oplayer == s.player {
                damage = st(ld(ff) * ld(damage));
                s.chatter_bits |= 1;
            } else if ally {
                s.chatter_bits |= 4;
            }
            s.getting_rocked = Some(oid);
            s.recent_attacker = Some(oid);
            s.recently_attacked = crate::object::RECENT_ATTACK_DURATION;
            crate::damage::apply_damage_to_target(u, tid, damage, m.bullet_type as i32, crate::damage::how::KILLED_BY_PLAYER, oplayer as i32, Some(oid));
        }
        None => {
            crate::damage::apply_damage_to_target(u, tid, damage, m.bullet_type as i32, crate::damage::how::KILLED_BY_UNKNOWN, 99, None);
        }
    }
}

/// `univMissileCollidedWithTarget` for a mine hit by a missile.
fn collided_with_mine(u: &mut Universe, nid: MissileId, mid: MissileId, t: f32, _side: i32) {
    let m = u.missiles.missile(mid).clone();
    let immobile = u.missiles.get(nid).and_then(|n| statics_of(u, n)).is_some_and(|s| s.header.immobile);
    if !immobile {
        if let Some((push, r, mass)) = knock_back(u, mid) {
            let n = u.missiles.missile_mut(nid);
            n.rotinfo.torque.x = st(r[0] * ld(mass) + ld(n.rotinfo.torque.x));
            n.rotinfo.torque.y = st(r[1] * ld(mass) + ld(n.rotinfo.torque.y));
            n.rotinfo.torque.z = st(r[2] * ld(mass) + ld(n.rotinfo.torque.z));
            let f = &mut n.posinfo.force;
            f.x = st(ld(push.x) + ld(f.x));
            f.y = st(ld(push.y) + ld(f.y));
            f.z = st(ld(push.z) + ld(f.z));
        }
    }
    let position = m.posinfo.position.add_scaled(m.heading(), t);
    u.special_events.push(SpecialEvent::MissileHit { missile: mid, target: ObjRef::Missile(nid), position });
    apply_damage(u, nid, m.damage, m.bullet_type as i32);
}

/// `ApplyDamageToTarget` for a missile or mine: it dies when its health runs out.
pub fn apply_damage(u: &mut Universe, mid: MissileId, damage: f32, bullet_type: i32) -> bool {
    let Some(m) = u.missiles.get_mut(mid) else { return false };
    let alive = !m.is_dead();
    m.health = st(ld(m.health) - ld(damage));
    if 0.0 < m.health || !alive {
        return false;
    }
    add_to_delete_list(u, mid, bullet_type);
    true
}

/// `AddMissileToDeleteMissileList`: dead from now on, removed at the end of the update.
/// `bullet_type` -1 means no explosion.
pub fn add_to_delete_list(u: &mut Universe, mid: MissileId, bullet_type: i32) {
    let m = u.missiles.missile_mut(mid);
    m.flags |= sof::DEAD;
    u.dead_missiles.push((mid, bullet_type));
}

/// `univMissileCapRemainingLife`: a missile that lost its target lives at most 5 more seconds.
pub fn cap_remaining_life(m: &mut Missile) {
    if ld(CAPPED_REMAINING_LIFE) < ld(m.total_lifetime) - ld(m.time_lived) {
        m.total_lifetime = st(ld(CAPPED_REMAINING_LIFE) + ld(m.time_lived));
    }
}

/// `univRemoveAllTargetReferencesFromMissiles`: missiles let go of a ship that is gone.
pub fn remove_ship_references(u: &mut Universe, id: ShipId) {
    for &mid in &u.missile_list {
        let Some(m) = u.missiles.get_mut(mid) else { continue };
        if m.owner == Some(id) {
            m.owner = None;
        }
        if m.target == Some(id) {
            m.target = None;
            cap_remaining_life(m);
        }
    }
}

/// `univDeleteDeadMissiles`: the missiles that died this update leave the universe
/// (`DeleteDeadMissile` 0x5448a0 for each).
pub fn delete_dead(u: &mut Universe) {
    let dead = std::mem::take(&mut u.dead_missiles);
    for (mid, bullet_type) in dead {
        let Some(m) = u.missiles.get(mid) else { continue };
        u.special_events.push(SpecialEvent::MissileDied { missile: mid, position: m.posinfo.position, exploded: bullet_type >= 0, mine: m.kind == kind::MINE });
        u.blobs.object_died(ObjRef::Missile(mid));
        // (Bullets and other missiles never aim at missiles in this simulation, so there are
        // no references to clear.)
        if let Some(f) = u.missiles.missile(mid).formation {
            crate::ships::minelayer::mine_died(u, f, mid);
        }
        crate::command::cl_missile_died(u, mid);
        u.missile_list.retain(|&x| x != mid);
        u.impactable_list.retain(|&o| o != ObjRef::Missile(mid));
        u.space_obj_list.retain(|&o| o != ObjRef::Missile(mid));
        u.render_list.retain(|&o| o != ObjRef::Missile(mid));
        u.missiles.remove(mid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capped_life() {
        let mut m = Missile::blank(MissileId(0), ShipRace::Somtaaw, kind::MISSILE);
        m.total_lifetime = 20.0;
        m.time_lived = 3.0;
        cap_remaining_life(&mut m);
        assert_eq!(m.total_lifetime, 8.0);
        m.total_lifetime = 9.0;
        cap_remaining_life(&mut m);
        assert_eq!(m.total_lifetime, 8.0);
    }

    #[test]
    fn cap_min_max_scales_to_speed() {
        let mut v = Vector::new(3.0, 4.0, 0.0);
        cap_min_max(&mut v, 10.0, 10.0);
        assert!((v.x - 6.0).abs() < 1e-3 && (v.y - 8.0).abs() < 1e-3);
    }

    struct GuideCase {
        pos: [u32; 3],
        vel: [u32; 3],
        cs: [u32; 9],
        tc: [u32; 3],
        tv: [u32; 3],
        mass: u32,
        maxvel: u32,
        thrust: [u32; 6],
        lived: u32,
        life: u32,
        exp_force: [u32; 3],
        exp_cs: [u32; 15],
        exp_lived: u32,
        exp_ret: bool,
    }

    /// Inputs and results of emulating `aishipGuideMissile` (0x415e70, with
    /// `aishipFlyMissileToTarget`, `vecCapMinMaxVector`, `aitrackVelocityVectorGuidance` and
    /// `physApplyForceToObj` below it) in cataclysm.exe (Ghidra p-code emulation with the
    /// `fmathSqrtDouble` table written into memory; `ErrorVelocitySquared` = 1, no target loss
    /// checks): force, orientation, rotation and the life counter.
    const GUIDE_CASES: &[GuideCase] = &[
    GuideCase { pos: [0x4002ae54, 0xc18d15ab, 0x40278fbc], vel: [0xc2425124, 0xc1199bdb, 0xc1f36a8c], cs: [0xbf037a8d, 0xbdcfde9a, 0x3f5a1d77, 0xbe4687a6, 0x3f7b2483, 0x00000000, 0xbf55fa00, 0xbe29265d, 0xbf06058b], tc: [0xc2e0fc53, 0x41170ccb, 0xc20d599a], tv: [0x43863c23, 0x429cc048, 0x4247314c], mass: 0x3f07654f, maxvel: 0x449c4000, thrust: [0x4537323d, 0x436b16fb, 0x4521d8f0, 0x446af77c, 0x440195be, 0x43dccc7b], lived: 0x40456da9, life: 0x412b67fa, exp_force: [0xc4f74495, 0x450c8b5c, 0x44b65468], exp_cs: [0xbf037b91, 0xbdcfe036, 0x3f5a1f26, 0xbe4687f4, 0x3f7b24e6, 0x00000000, 0xbf55fb58, 0xbe29276d, 0xbf060663, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40496da9, exp_ret: false },
    GuideCase { pos: [0xc0b12cb2, 0xc12101ca, 0xc14cf304], vel: [0x43792d70, 0xc1f75730, 0x43be4c3e], cs: [0xbf540f98, 0x3dd27f64, 0x3f0cf97f, 0xbdfc2dd3, 0xbf7e0d46, 0x00000000, 0x3f0be6db, 0xbd8aded5, 0x3f55afe4], tc: [0xc1ce4329, 0x411a5a44, 0xc2b0ae90], tv: [0x4327e5d5, 0xc37ae312, 0xc2efb379], mass: 0x4008718d, maxvel: 0x44480000, thrust: [0x4469c140, 0x4537e81d, 0x43dd3204, 0x44a411cc, 0x450f7b57, 0x44073054], lived: 0x409c77db, life: 0x40a8c84d, exp_force: [0x4502778f, 0xc43032cb, 0xc5017c32], exp_cs: [0xbf541208, 0x3dd281cf, 0x3f0cfb1e, 0xbdfc2f2c, 0xbf7e0ea2, 0x00000000, 0x3f0be7bd, 0xbd8adfb5, 0x3f55b13d, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x409e77db, exp_ret: false },
    GuideCase { pos: [0xc18c95f6, 0x4100f474, 0x40bc532f], vel: [0x43175783, 0xc10d0b8b, 0x425f7809], cs: [0xbeb0be74, 0x3ca4b80d, 0x3f703541, 0xbd6e2dd2, 0xbf7f911b, 0x00000000, 0x3f6fcd34, 0xbd5f7c81, 0x3eb10b25], tc: [0xc4d7a09d, 0xc4ca1097, 0xc20f82c3], tv: [0x4393edc5, 0x434127a6, 0xc3013e20], mass: 0x3fdd1c5f, maxvel: 0x449c4000, thrust: [0x4530be18, 0x448d5b12, 0x44e9f555, 0x44bf76b6, 0x4437335d, 0x4469635e], lived: 0x40ec46ba, life: 0x40f9210b, exp_force: [0xc4f4f05a, 0xc4dc1675, 0x4511a9ff], exp_cs: [0xbeb0bef9, 0x3ca4b887, 0x3f7035f5, 0xbd6e2f35, 0xbf7f929a, 0x00000000, 0x3f6fcea5, 0xbd5f7dd8, 0x3eb10c36, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40ee46ba, exp_ret: false },
    GuideCase { pos: [0xc09908ff, 0xc12c51ac, 0xc18571e5], vel: [0x43cd9a99, 0x44724c9b, 0x43b6056c], cs: [0xbe02b377, 0xbe9a072d, 0x3f71f1cf, 0x3f6baa80, 0xbec7f9aa, 0x00000000, 0x3ebcfeed, 0x3f5eba1d, 0x3ea7516a], tc: [0xc1ea9e1e, 0x41e4478e, 0x435d467e], tv: [0xc3513891, 0x42be3853, 0xc392618f], mass: 0x405800c2, maxvel: 0x44480000, thrust: [0x42dfbe2f, 0x44a45e3c, 0x44925abb, 0x44d9cc77, 0x4532ffbe, 0x450366e8], lived: 0x40a4f50f, life: 0x41152b9f, exp_force: [0xc515a58d, 0xc493c953, 0xc411552b], exp_cs: [0xbe02b3d4, 0xbe9a0799, 0x3f71f279, 0x3f6bab1f, 0xbec7fa31, 0x80000000, 0x3ebcfef4, 0x3f5eba25, 0x3ea75171, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40a6f50f, exp_ret: false },
    GuideCase { pos: [0xc17e7a20, 0x402af74e, 0x3fbb7cd9], vel: [0xc1776c8e, 0xc2cb3120, 0x41d1e106], cs: [0x3d187558, 0x3e7a6817, 0x3f780b7a, 0xbf7d15c5, 0x3e1a16d1, 0x00000000, 0xbe154d03, 0xbf75386f, 0x3e7d4a6d], tc: [0xc3996068, 0xc39f51d4, 0xc25e6c7c], tv: [0x4386af44, 0x42887c16, 0xc380e7c4], mass: 0x3f88e2ea, maxvel: 0x444b768c, thrust: [0x444fe30a, 0x448a6dc0, 0x4490825e, 0x43e41f09, 0x45201eac, 0x453a3ff7], lived: 0x40951ddb, life: 0x41063082, exp_force: [0xc4c2b52e, 0xc51f6e2c, 0xc3dba862], exp_cs: [0x3d187684, 0x3e7a6a05, 0x3f780d63, 0xbf7d1750, 0x3e1a17c1, 0x00000000, 0xbe154d3c, 0xbf7538ce, 0x3e7d4acf, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40971ddb, exp_ret: false },
    GuideCase { pos: [0x3f8feb9b, 0x41991ed2, 0x4168872c], vel: [0xc387e02b, 0x4204caf8, 0xc3b5d836], cs: [0xbf4b04e6, 0x3dc669b7, 0x3f19f61e, 0x3df85870, 0x3f7e1c60, 0x80000000, 0xbf18d342, 0x3d955b9d, 0xbf4c8749], tc: [0x43e06a3a, 0xc21faf51, 0x43b35297], tv: [0x42eb6fa7, 0xc30f54b5, 0xc29ff5d3], mass: 0x3f6b05b0, maxvel: 0x449c2437, thrust: [0x44d0d16e, 0x44c2ba49, 0x44f335d0, 0x44eacb94, 0x451525ba, 0x450fb229], lived: 0x3ff9c974, life: 0x40d59f72, exp_force: [0x4521ef3c, 0xc3e4d6d8, 0x4460fe8b], exp_cs: [0xbf4b0635, 0x3dc66afe, 0x3f19f71b, 0x3df85955, 0x3f7e1d4a, 0x80000000, 0xbf18d3b3, 0x3d955c0b, 0xbf4c87e1, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x4000e4ba, exp_ret: false },
    GuideCase { pos: [0x41457b69, 0x410ecd49, 0xc0c09d66], vel: [0xc3e1a698, 0x431ae23a, 0xc3656511], cs: [0xbed1d581, 0x3e1006fc, 0x3f66b870, 0x3ea63323, 0x3f72231e, 0x80000000, 0xbf5a3a00, 0x3e95c9af, 0xbeddd8f0], tc: [0x40974614, 0xc21ad752, 0xc2fc7930], tv: [0x438e5ac0, 0xc37bad58, 0xc36eb4aa], mass: 0x400283d3, maxvel: 0x44480000, thrust: [0x44eeb95d, 0x45296e4f, 0x451e9435, 0x44ba4f22, 0x44f9345d, 0x45172f78], lived: 0x3f59086e, life: 0x4119fc50, exp_force: [0x452813ba, 0xc51c2bb8, 0xc4ae6240], exp_cs: [0xbed1d6b6, 0x3e1007d0, 0x3f66b9c2, 0x3ea6339b, 0x3f7223cc, 0x80000000, 0xbf5a3cbe, 0x3e95cb91, 0xbedddbba, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x3f69086e, exp_ret: false },
    GuideCase { pos: [0xc15a5506, 0x419dcbc4, 0xc1972f33], vel: [0xc2a07102, 0x44047ad1, 0xc461f7ab], cs: [0xbe03d5ee, 0x3f59b802, 0x3f0291a5, 0x3f7d1dba, 0x3e194518, 0x80000000, 0xbd9c5892, 0x3f011919, 0xbf5c3309], tc: [0xc135a8da, 0xc32627c0, 0x4315705f], tv: [0x4259f314, 0xc1a64d26, 0x42bb07a3], mass: 0x40240544, maxvel: 0x44480000, thrust: [0x44fac27f, 0x448b85d1, 0x44d363a7, 0x43efed35, 0x430d4df5, 0x4536394e], lived: 0x40cfe559, life: 0x410afa22, exp_force: [0xc2d85504, 0xc51bb646, 0x44f251d1], exp_cs: [0xbe03d75c, 0x3f59ba5d, 0x3f02930f, 0x3f7d2140, 0x3e194739, 0x80000000, 0xbd9c5a6f, 0x3f011aa4, 0xbf5c35aa, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40d1e559, exp_ret: false },
    GuideCase { pos: [0x41833499, 0xc0bb280d, 0xbfd6373b], vel: [0xc383c934, 0xc2b169f5, 0xc3ca0bb2], cs: [0xbf47de72, 0xbe8688d8, 0x3f1121fe, 0xbea350d6, 0x3f72a094, 0x00000000, 0xbf098d2c, 0xbe392cf2, 0xbf52e28e], tc: [0x43145420, 0xc3036973, 0xc2e1b880], tv: [0x4248097c, 0x437293fc, 0xc23e7dfe], mass: 0x406c8430, maxvel: 0x446df416, thrust: [0x440714d5, 0x44c592c3, 0x4524722e, 0x4512fde3, 0x44e919df, 0x4512e835], lived: 0x3fbfbf47, life: 0x40bfb58f, exp_force: [0x454af2b2, 0xc4ad9464, 0x4482069f], exp_cs: [0xbf47deef, 0xbe86892c, 0x3f11225a, 0xbea352d0, 0x3f72a384, 0x00000000, 0xbf098d8b, 0xbe392d72, 0xbf52e31f, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x3fc7bf47, exp_ret: false },
    GuideCase { pos: [0x3e9dfb89, 0x401e06f9, 0x41266547], vel: [0xc2b9ce21, 0xc2a4ceaa, 0x42c92382], cs: [0x3ef11052, 0x3ed5d230, 0x3f46f156, 0xbf29dfb5, 0x3f3f846d, 0x00000000, 0xbf14d4f9, 0xbf04032b, 0x3f211d3e], tc: [0xc32287b0, 0x4189196f, 0x424f20a3], tv: [0x43777e2a, 0xc2083430, 0x42870891], mass: 0x400aea3c, maxvel: 0x4470d82b, thrust: [0x4461f5a3, 0x44c4b4e4, 0x4518959a, 0x44c48f5f, 0x444c8ceb, 0x44ca29db], lived: 0x410c2801, life: 0x4137ea26, exp_force: [0xc53149b7, 0x443ad2fb, 0xc430feb5], exp_cs: [0x3ef1126b, 0x3ed5d40c, 0x3f46f311, 0xbf29dff1, 0x3f3f84b1, 0x00000000, 0xbf14d60b, 0xbf04041e, 0x3f211e67, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x410d2801, exp_ret: false },
    GuideCase { pos: [0xc171b232, 0x41313ca3, 0x418ca439], vel: [0xc1b44e87, 0xc2b4b999, 0xc2781cb3], cs: [0xbe095f59, 0xbf09b0ed, 0x3f551140, 0xbf7863e8, 0x3e77d0bf, 0x00000000, 0xbe4e4162, 0xbf4ebbdd, 0xbf0de8d2], tc: [0x42f60e48, 0xc3034d43, 0xc308ea3b], tv: [0x42ac264c, 0xc2a0947e, 0xc31422a3], mass: 0x3f4ecebc, maxvel: 0x4464a09a, thrust: [0x4532e431, 0x449cde3d, 0x44bd21cd, 0x4539aa09, 0x451d216f, 0x440e101a], lived: 0x408a1644, life: 0x4109bf6e, exp_force: [0x451f0497, 0xc501a327, 0xc518620f], exp_cs: [0xbe0960cd, 0xbf09b260, 0x3f551380, 0xbf786467, 0x3e77d13e, 0x00000000, 0xbe4e4324, 0xbf4ebda0, 0xbf0dea07, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x408c1644, exp_ret: false },
    GuideCase { pos: [0xc18b6d4c, 0x419b3a05, 0x41388d67], vel: [0xc43fa1e1, 0x440cf042, 0x425f20de], cs: [0x3d411c2c, 0xbd0e067a, 0x3f7f8fac, 0x3f17acb1, 0x3f4e3ae3, 0x80000000, 0xbf4de066, 0x3f176a23, 0x3d6fb6b7], tc: [0xc4381815, 0xc3720cc5, 0xc39247be], tv: [0x438d8240, 0xc36d21db, 0xc30ca954], mass: 0x3ee4988b, maxvel: 0x44480000, thrust: [0x450f3c08, 0x451ad5aa, 0x45203ce0, 0x4500c52d, 0x4531b679, 0x449fa7f5], lived: 0x40abb62e, life: 0x4109a7d9, exp_force: [0xc3968184, 0xc512029c, 0xc512ea74], exp_cs: [0x3d411cf7, 0xbd0e0710, 0x3f7f90ba, 0x3f17acef, 0x3f4e3b37, 0x80000000, 0xbf4de0e9, 0x3f176a84, 0x3d6fb750, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000, 0x00000000], exp_lived: 0x40adb62e, exp_ret: false },

    ];

    #[test]
    fn guidance_matches_emulated_binary() {
        use std::sync::Arc;
        let f = f32::from_bits;
        let v = |a: [u32; 3]| Vector::new(f(a[0]), f(a[1]), f(a[2]));
        let bits = |v: Vector| [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()];
        for (i, c) in GUIDE_CASES.iter().enumerate() {
            let mut t = crate::statics::ShipStaticInfo::new(crate::statics::ShipType::tHeavyInterceptor, String::new());
            t.ship_race = ShipRace::Taiidan;
            t.health.max_health = 100.0;
            let mut world = crate::statics::test_world(vec![t]);
            let mut mst = MissileStaticInfo::default();
            mst.header.mass = f(c.mass);
            mst.header.max_velocity = f(c.maxvel);
            mst.health.max_health = 30.0;
            world.missiles = vec![None; ShipRace::ALL.len()];
            world.missiles[ShipRace::Somtaaw.index()] = Some(mst);
            let mut tw = crate::tunables::SimTweaks::compiled();
            tw.error_velocity_squared = 1.0;
            tw.combat.missile_num_times_check = 0;
            let mut u = Universe::new(Arc::new(world), tw, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
            let tid = u.add_ship(crate::statics::ShipType::tHeavyInterceptor, 1, Vector::ZERO, None).unwrap();
            {
                let s = u.ships.ship_mut(tid);
                s.coll.coll_position = v(c.tc);
                s.posinfo.velocity = v(c.tv);
            }
            let mut m = Missile::blank(MissileId(0), ShipRace::Somtaaw, kind::MISSILE);
            m.posinfo.position = v(c.pos);
            m.posinfo.velocity = v(c.vel);
            m.rotinfo.coordsys = Matrix { m: c.cs.map(f) };
            m.nonstat.thrust_strength = c.thrust.map(f);
            m.time_lived = f(c.lived);
            m.total_lifetime = f(c.life);
            m.target = Some(tid);
            u.missiles.insert(m);
            let ret = guide_missile(&mut u, MissileId(0));
            let m = u.missiles.missile(MissileId(0));
            assert_eq!(bits(m.posinfo.force), c.exp_force, "case {i} force");
            let cs: Vec<u32> = m.rotinfo.coordsys.m.iter().map(|x| x.to_bits()).collect();
            assert_eq!(cs[..], c.exp_cs[..9], "case {i} orientation");
            assert_eq!(bits(m.rotinfo.rotspeed), c.exp_cs[9..12], "case {i} rotation");
            assert_eq!(m.time_lived.to_bits(), c.exp_lived, "case {i} life");
            assert_eq!(ret, c.exp_ret, "case {i} result");
        }
    }
}
