//! Effects in the simulation: the ETG effects the original creates from game code
//! (`etgEffectCreate` callers), run inside the universe update as `univUpdate` runs them.
//!
//! [`Effects`] owns an [`FxWorld`] and the event tables. It is optional
//! ([`Universe::effects`]); without it the simulation behaves as before (no effects, dead
//! ships leave the universe at once). With it:
//!
//! * game code creates effects where the original does (guns firing, bullets, hits, missiles,
//!   collisions, damage, deaths, hyperspace, special abilities);
//! * the effect scripts run once per update right after the ships moved (the effect loop of
//!   `univUpdate`, before `univDeleteDeadShips`), with the effects' owners refreshed from the
//!   simulation objects first;
//! * what the scripts ask of the game is applied there: `deleteParentShip` removes a dead ship
//!   (a dead ship stays in the universe as a hulk until its death effect deletes it),
//!   `hideParentShip` hides it, `damageDone` frees a damage-effect slot. Sounds, battle chatter
//!   and the render-list requests are kept in [`Effects::events`] for presentation.
//!
//! Camera independence: the original decides some of this from what is on screen (effects
//! only for objects in the render list, the level of detail, effects lingering four updates
//! when not drawn, and the bolt jitter drawn from the same random stream as damage effects).
//! The simulation must give the same result whatever is drawn, so here every object counts as
//! being in the render list, the level of detail is [`EffectOptions::lod`] (0), every effect
//! is treated as drawn ([`FxWorld::assume_drawn`]) and drawing uses its own generator
//! ([`FxWorld::draw_randy`]). What is seen on screen is what the original shows for on-screen
//! objects.
//!
//! The effect state is not part of `harness::world_hash`; its gameplay consequences (hulks,
//! hidden ships) are, through the ships.

pub mod damage;
pub mod hyperspace;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use hwc_fx::compiler::{FileSource, Library};
use hwc_fx::runtime::{eaf, EffectId, Owner, OwnerId};
use hwc_fx::{EtgTables, FxEvent, FxWorld, RenderList};
use hwc_math::fpu::{ld, st};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Randy, Vector};

use crate::object::{sof, BulletId, MissileId, ObjRef, ShipId};
use crate::resource::ResourceId;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

pub use hwc_fx::scripts::{GUN_BULLET, GUN_BULLET_DESTROYED, GUN_FIRE, GUN_HIT};

/// `EAF_*` attachment flags for [`create`].
pub use hwc_fx::runtime::eaf as attach;

/// `EAF_AllButNLips`.
pub const EAF_ALL_BUT_NLIPS: u32 = eaf::POSITION | eaf::COORDSYS | eaf::VELOCITY;

/// `SOF_ForceVisible` on an effect (set on death effects).
pub const SOF_FORCE_VISIBLE: u32 = 0x1_0000;

/// The compiled effects and their event tables, loaded once and shared by every universe.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct EffectData {
    pub lib: Library,
    pub tables: Arc<EtgTables>,
    /// Compile errors and table warnings (the original ignores most of them).
    pub warnings: Vec<String>,
}

impl EffectData {
    /// `etgStartup`: compile every effect the `ETG\*.script` files name.
    pub fn load(files: &dyn FileSource) -> Self {
        let mut lib = Library::new();
        let load = hwc_fx::load_scripts(&mut lib, files);
        let mut warnings: Vec<String> = load.errors.iter().map(|e| e.to_string()).collect();
        warnings.extend(load.tables.warnings.iter().cloned());
        Self { lib, tables: Arc::new(load.tables), warnings }
    }

    /// [`EffectData::load`] over the simulation's data source.
    pub fn load_from(src: &dyn crate::statics::DataSource) -> Self {
        struct Adapter<'a>(&'a dyn crate::statics::DataSource);
        impl FileSource for Adapter<'_> {
            fn read_file(&self, path: &str) -> Option<Vec<u8>> {
                self.0.read(path)
            }
        }
        Self::load(&Adapter(src))
    }

    /// Whether any effect was compiled (no `ETG` directory: nothing).
    pub fn is_empty(&self) -> bool {
        self.lib.statics.is_empty()
    }
}

/// The original's effect switches (all on in the shipped game).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EffectOptions {
    /// `etgFireEffectsEnabled` (0x870b5c).
    pub fire: bool,
    /// `etgBulletEffectsEnabled` (0x870b60).
    pub bullet: bool,
    /// `etgHitEffectsEnabled` (0x870b58).
    pub hit: bool,
    /// `etgDamageEffectsEnabled` (0x8ae298).
    pub damage: bool,
    /// The level of detail effects are picked at (`ship->currentLOD`; camera dependent in the
    /// original, fixed here).
    pub lod: u8,
}

impl Default for EffectOptions {
    fn default() -> Self {
        Self { fire: true, bullet: true, hit: true, damage: true, lod: 0 }
    }
}

/// What owns an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub enum FxOwner {
    Ship(ShipId),
    Bullet(BulletId),
    Missile(MissileId),
    Resource(ResourceId),
}

impl FxOwner {
    /// The owner id the effect world knows the object by.
    pub fn id(self) -> OwnerId {
        match self {
            FxOwner::Ship(s) => 0x0001_0000 | u32::from(s.0),
            FxOwner::Bullet(b) => 0x2000_0000 | (b.0 & 0x0fff_ffff),
            FxOwner::Missile(m) => 0x4000_0000 | (m.0 & 0x0fff_ffff),
            FxOwner::Resource(r) => 0x6000_0000 | u32::from(r.0),
        }
    }

    /// The object behind an owner id.
    pub fn from_id(id: OwnerId) -> Option<Self> {
        match id >> 28 {
            0 if id & 0xffff_0000 == 0x0001_0000 => Some(FxOwner::Ship(ShipId(id as u16))),
            2 => Some(FxOwner::Bullet(BulletId(id & 0x0fff_ffff))),
            4 => Some(FxOwner::Missile(MissileId(id & 0x0fff_ffff))),
            6 => Some(FxOwner::Resource(ResourceId(id as u16))),
            _ => None,
        }
    }
}

impl From<ObjRef> for FxOwner {
    fn from(o: ObjRef) -> Self {
        match o {
            ObjRef::Ship(s) => FxOwner::Ship(s),
            ObjRef::Bullet(b) => FxOwner::Bullet(b),
            ObjRef::Missile(m) => FxOwner::Missile(m),
        }
    }
}

impl From<ShipId> for FxOwner {
    fn from(s: ShipId) -> Self {
        FxOwner::Ship(s)
    }
}

/// Effects in a universe.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Effects {
    pub world: FxWorld,
    /// Game data: not saved, linked back on load.
    #[serde(skip, default = "crate::save::etg_tables")]
    pub tables: Arc<EtgTables>,
    pub options: EffectOptions,
    /// The effect of each bullet (`bullet->effect`, +0x134).
    pub bullet_effect: BTreeMap<BulletId, EffectId>,
    /// The beam of each ship being consumed (ship +0x594).
    pub consume_beam: BTreeMap<ShipId, EffectId>,
    /// The hit-effect LOD list of each bullet (`bullet->hitEffect`, +0x130): race and bullet
    /// type of the gun that fired it.
    pub bullet_hit: BTreeMap<BulletId, (usize, usize)>,
    /// The hit-effect LOD list of each missile (`missile->hitEffect`, +0x204): race and bullet
    /// type of the launcher.
    pub missile_hit: BTreeMap<MissileId, (usize, usize)>,
    /// Long-lived effects owned by game code, by ship and a per-module slot number (the
    /// pointers the original keeps in ship structures: beams, fields, charge effects...).
    pub slots: BTreeMap<(ShipId, u32), EffectId>,
    /// Damage effects of each ship (`ship->showingDamage[level][i]`), see [`damage`].
    pub damage: BTreeMap<ShipId, damage::DamageSlots>,
    /// Ships the death effect asked to leave out of the render list
    /// (`omitParentShipFromRenderList`: `ship+0x1c`).
    pub omit_from_render: BTreeSet<ShipId>,
    /// Requests of the last update for presentation (sounds, chatter, death cries, ...).
    pub events: Vec<FxEvent>,
    /// `local_5c` of `univUpdateAllPosVelBullets`.
    pub bullet_effect_deleted_this_pass: bool,
    /// The front end draws sentinel field spheres as round bubbles (`hwcModernFields`): hits
    /// on them are shown on the bubble ([`crate::ships::sentinel::bubble_hit`]). Presentation
    /// only, not saved.
    #[serde(skip)]
    pub field_bubble: bool,
}

impl std::fmt::Debug for Effects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Effects").field("world", &self.world).field("options", &self.options).finish()
    }
}

impl Effects {
    /// Effects over the shared data, with the simulation's camera-independent settings.
    pub fn new(data: &EffectData) -> Self {
        let mut world = FxWorld::new(data.lib.clone());
        world.assume_drawn = true;
        world.draw_randy = Some(Randy::default());
        world.history_scalar = 256;
        Self {
            world,
            tables: data.tables.clone(),
            options: EffectOptions::default(),
            bullet_effect: BTreeMap::new(),
            consume_beam: BTreeMap::new(),
            bullet_hit: BTreeMap::new(),
            missile_hit: BTreeMap::new(),
            slots: BTreeMap::new(),
            damage: BTreeMap::new(),
            omit_from_render: BTreeSet::new(),
            events: Vec::new(),
            bullet_effect_deleted_this_pass: false,
            field_bubble: false,
        }
    }

    /// The effect static for `lod` at the simulation's level of detail, the way most callers
    /// pick it: `lod->level[LOD]` if `LOD < nLevels`, else none.
    pub fn pick(&self, lod: Option<&[usize]>) -> Option<usize> {
        lod.and_then(|l| l.get(usize::from(self.options.lod)).copied())
    }

    /// `lod->level[min(nLevels - 1, LOD)]` (deaths).
    pub fn pick_clamped(&self, lod: Option<&[usize]>) -> Option<usize> {
        let l = lod?;
        l.get(usize::from(self.options.lod).min(l.len().checked_sub(1)?)).copied()
    }

    /// The effect static with this file name.
    pub fn named(&self, name: &str) -> Option<usize> {
        self.world.lib.get(name)
    }
}

/// Attach effects to a universe (call before the first update).
pub fn attach(u: &mut Universe, data: &EffectData) {
    let mut fx = Effects::new(data);
    fx.world.universe_time = u.total_time_elapsed;
    u.effects = Some(Box::new(fx));
}

/// Whether the original would treat `o` as being in the render list
/// (`univSpaceObjInRenderList`). Always true here (see the module notes).
pub fn in_render_list(_u: &Universe, _o: FxOwner) -> bool {
    true
}

// -------------------------------------------------------------------------------- owners

/// The state of `o` as an effect owner, `None` when it is gone.
pub fn owner_state(u: &Universe, o: FxOwner) -> Option<Owner> {
    match o {
        FxOwner::Ship(id) => {
            let s = u.ships.get(id)?;
            let stat = u.statics.ship(s.ship_type);
            let c = stat.map_or([0, 0, 0, 255], |st| st.hyperspace_color);
            Some(Owner {
                position: s.posinfo.position,
                velocity: s.posinfo.velocity,
                coordsys: s.rotinfo.coordsys,
                nlips: 1.0,
                color_scheme: s.player as u8,
                is_ship: true,
                is_derelict: false,
                hold_effects: s.ship_type == ShipType::sLeech,
                hyperspace_color: u32::from_le_bytes(c),
                ..Owner::default()
            })
        }
        FxOwner::Bullet(id) => {
            let b = u.bullets.get(id)?;
            Some(Owner {
                position: b.posinfo.position,
                velocity: b.posinfo.velocity,
                coordsys: b.coordsys,
                is_ship: false,
                ..Owner::default()
            })
        }
        FxOwner::Missile(id) => {
            let m = u.missiles.get(id)?;
            Some(Owner {
                position: m.posinfo.position,
                velocity: m.posinfo.velocity,
                coordsys: m.rotinfo.coordsys,
                is_ship: false,
                ..Owner::default()
            })
        }
        FxOwner::Resource(id) => {
            let r = u.base.resources.get(id)?;
            Some(Owner {
                position: r.position,
                velocity: r.velocity,
                coordsys: r.coordsys,
                is_ship: false,
                ..Owner::default()
            })
        }
    }
}

/// Refresh every owner the effects refer to from the simulation; owners that are gone are
/// detached (`etgShipDied`). The NIS's effect anchors ([`crate::nis_support::nis_effect_owner`])
/// are kept up to date by the NIS and left alone.
pub fn sync_owners(u: &mut Universe) {
    let Some(fx) = u.effects.as_ref() else { return };
    let mut ids: Vec<OwnerId> =
        fx.world.effects.iter().filter_map(|e| e.owner).filter(|&id| !crate::nis_support::is_nis_effect_owner(id)).collect();
    ids.sort_unstable();
    ids.dedup();
    let states: Vec<(OwnerId, Option<Owner>)> =
        ids.into_iter().map(|id| (id, FxOwner::from_id(id).and_then(|o| owner_state(u, o)))).collect();
    let fx = u.effects.as_mut().unwrap();
    for (id, st) in states {
        match st {
            Some(o) => fx.world.set_owner(id, o),
            None => fx.world.owner_died(id),
        }
    }
}

// -------------------------------------------------------------------------------- creation

/// `etgEffectCreate`: start effect `stat` owned by `owner`. `position`, `velocity` and
/// `coordsys` default to the owner's; `nlips` 0 means 1. `flags` are [`attach`] flags.
/// Returns the new effect, or `None` without effects or without the owner's state.
#[allow(clippy::too_many_arguments)]
pub fn create(
    u: &mut Universe,
    stat: usize,
    owner: Option<FxOwner>,
    position: Option<Vector>,
    velocity: Option<Vector>,
    coordsys: Option<Matrix>,
    nlips: f32,
    flags: u32,
    params: &[u32],
) -> Option<EffectId> {
    let state = match owner {
        Some(o) => Some((o.id(), owner_state(u, o)?)),
        None => None,
    };
    let fx = u.effects.as_mut()?;
    if let Some((id, st)) = state.clone() {
        fx.world.set_owner(id, st);
    }
    Some(fx.world.create_effect(stat, state.map(|(id, _)| id), position, velocity, coordsys, nlips, flags, params))
}

/// `!etgFrequencyExceeded(stat)`: whether another instance of `stat` may start now.
pub fn frequency_ok(u: &mut Universe, stat: usize) -> bool {
    match u.effects.as_mut() {
        Some(fx) => !fx.world.frequency_exceeded(stat),
        None => false,
    }
}

/// `etgEffectDelete` + `univDeleteEffect`.
pub fn delete(u: &mut Universe, effect: EffectId) {
    if let Some(fx) = u.effects.as_mut() {
        fx.world.delete_effect(effect);
    }
}

/// Whether an effect still exists.
pub fn exists(u: &Universe, effect: EffectId) -> bool {
    u.effects.as_ref().is_some_and(|fx| fx.world.effect(effect).is_some())
}

/// The effect in a per-ship slot (see [`Effects::slots`]).
pub fn slot(u: &Universe, ship: ShipId, slot: u32) -> Option<EffectId> {
    u.effects.as_ref()?.slots.get(&(ship, slot)).copied()
}

/// Put an effect into a per-ship slot (replacing the reference, not deleting the old one).
pub fn set_slot(u: &mut Universe, ship: ShipId, slot: u32, effect: Option<EffectId>) {
    if let Some(fx) = u.effects.as_mut() {
        match effect {
            Some(e) => {
                fx.slots.insert((ship, slot), e);
            }
            None => {
                fx.slots.remove(&(ship, slot));
            }
        }
    }
}

/// Delete the effect in a per-ship slot and clear the slot.
pub fn delete_slot(u: &mut Universe, ship: ShipId, slot: u32) {
    if let Some(fx) = u.effects.as_mut() {
        if let Some(e) = fx.slots.remove(&(ship, slot)) {
            fx.world.delete_effect(e);
        }
    }
}

/// An effect parameter from a float (the scripts receive raw dwords).
pub fn pf(v: f32) -> u32 {
    v.to_bits()
}

// -------------------------------------------------------------------------------- update

/// Start of `univUpdate`: the effects' clock follows the universe clock.
pub fn begin_update(u: &mut Universe) {
    let now = u.total_time_elapsed;
    if let Some(fx) = u.effects.as_mut() {
        fx.world.universe_time = now;
        fx.events.clear();
    }
}

/// The effect loop of `univUpdate` (after the ships moved, before the dead ships are
/// deleted): refresh the owners, run every effect's scripts, then apply what they asked for.
pub fn update(u: &mut Universe) {
    if u.effects.is_none() {
        return;
    }
    sync_owners(u);
    let fx = u.effects.as_mut().unwrap();
    fx.world.run_update(UNIVERSE_UPDATE_PERIOD);
    let events = std::mem::take(&mut fx.world.events);
    for ev in events {
        apply_event(u, &ev);
        if let Some(fx) = u.effects.as_mut() {
            fx.events.push(ev);
        }
    }
}

fn apply_event(u: &mut Universe, ev: &FxEvent) {
    match *ev {
        FxEvent::DeleteParent { owner, .. } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                if u.ships.get(id).is_some_and(|s| s.flags & sof::DEAD != 0) {
                    crate::damage::wipe_ship(u, id);
                }
            }
            if let Some(fx) = u.effects.as_mut() {
                fx.world.owner_died(owner);
            }
        }
        FxEvent::HideParent { owner } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                // `univHideShipFromSpheres`.
                if let Some(s) = u.ships.get_mut(id) {
                    s.flags |= sof::HIDE;
                    u.render_list.retain(|&o| o != ObjRef::Ship(id));
                }
            }
        }
        FxEvent::OmitParentFromRenderList { owner } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                if let Some(fx) = u.effects.as_mut() {
                    fx.omit_from_render.insert(id);
                }
            }
        }
        FxEvent::DamageDone { effect, owner } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                damage::stop_single_effect(u, id, effect);
            }
        }
        // `etgChatterEventPlay` and `etgDeathCriesPlay`.
        FxEvent::Chatter { owner, event, .. } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                crate::battle_chatter::attempt(u, event as usize, id, -1);
            }
        }
        FxEvent::DeathCries { owner } => {
            if let Some(FxOwner::Ship(id)) = FxOwner::from_id(owner) {
                crate::battle_status::ship_dying(u, id);
            }
        }
        FxEvent::Sound { .. } => {}
    }
}

/// A ship left the universe for good (`univWipeShipOutOfExistence` / `univFreeShipContents`):
/// its damage effects stop, its slots are forgotten and its effects lose their owner.
pub fn ship_wiped(u: &mut Universe, id: ShipId) {
    damage::stop_effect(u, id, damage::ALL);
    let Some(fx) = u.effects.as_mut() else { return };
    fx.slots.retain(|&(s, _), _| s != id);
    fx.damage.remove(&id);
    fx.omit_from_render.remove(&id);
    fx.world.owner_died(FxOwner::Ship(id).id());
}

// -------------------------------------------------------------------------------- deaths

/// `DAT_00870b2c`: the explosion type (`EDT_*`) for each bullet type, starting at type -1.
const EXPLOSION_FOR_BULLET: [u8; 21] = [0, 1, 2, 1, 1, 1, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0];

/// `EDT_AccumDamage`.
pub const EDT_ACCUM_DAMAGE: i32 = 0;

/// The `deathBy` that `AddShipToDeleteShipList` records: accumulated damage while the ship's
/// health is above the type's big-death factor, else the explosion type of the bullet type
/// that killed it.
pub fn death_by(u: &Universe, id: ShipId, bullet_type: i32) -> i32 {
    let Some(fx) = u.effects.as_ref() else { return EDT_ACCUM_DAMAGE };
    let Some(s) = u.ships.get(id) else { return EDT_ACCUM_DAMAGE };
    let Some(stat) = u.statics.ship(s.ship_type) else { return EDT_ACCUM_DAMAGE };
    let class = stat.ship_class.map_or(0, |c| c.index());
    let big = fx.tables.big_death.get(stat.ship_race.index() * hwc_fx::scripts::CLASSES.len() + class).copied().unwrap_or(-10);
    if (big as f32) < s.health {
        return EDT_ACCUM_DAMAGE;
    }
    let i = (bullet_type + 1).clamp(0, EXPLOSION_FOR_BULLET.len() as i32 - 1) as usize;
    i32::from(EXPLOSION_FOR_BULLET[i])
}

/// The "big death" groups of `univDeleteDeadShip`: the class slot of the death table used and
/// the scale and size passed to the effect.
fn big_death_kind(t: ShipType) -> Option<(usize, f32, u32)> {
    use ShipType::*;
    Some(match t {
        sDreadnoughtCannon | sMothership | sMothershipOreCanArmour | bMothership | bSectMoship => (0, 0.5, 2),
        sCarrierSupport | sMothershipSpecial | sMothershipEngineering | sMothershipArmour | sMothershipMicro
        | sMothershipSupport | sMothershipCommand | sMothershipEnginesLower | sMothershipLowerSpine
        | sMothershipBigGun | bMothershipSupport => (0, 0.4, 1),
        sMothershipWeapons | sMothershipDockingBay | sMothershipEnginesUpper | bMothershipDockingBay
        | bMothershipEnginesLower | trFloatingCity | trGhostship => (0, 0.75, 3),
        dRKO | dNomadMoon => (9, 0.75, 3),
        _ => return None,
    })
}

/// The death-effect part of `univDeleteDeadShip`: start the explosion owned by the dying ship.
/// Returns the effect (the ship then stays as a hulk until the effect deletes it).
pub fn start_death_effect(u: &mut Universe, id: ShipId, death_by: i32) -> Option<EffectId> {
    if death_by < 0 {
        return None;
    }
    let fx = u.effects.as_ref()?;
    let s = u.ships.get(id)?;
    let stat: &ShipStaticInfo = u.statics.ship(s.ship_type)?;
    let race = stat.ship_race.index();
    let classes = hwc_fx::scripts::CLASSES.len();
    let kind = death_by as usize;
    let big = big_death_kind(s.ship_type);
    let class = match big {
        Some((c, _, _)) => c,
        None => stat.ship_class.map_or(0, |c| c.index()),
    };
    let lod = fx.tables.death.get((race * classes + class) * 5 + kind)?.as_deref();
    let effect = fx.pick_clamped(lod)?;
    let coll = &stat.header.coll;
    // (The software renderer would shrink everything by 0.75.)
    let col_size = st(ld(coll.coll_sphere_size) * 1.0);
    let max_vel = st(crate::tactics::speed_multiplier(s, stat) * ld(stat.header.max_velocity));
    let vel = if max_vel == 0.0 {
        0.0
    } else {
        st(hwc_math::fastmath::fmath_sqrt_double(s.posinfo.velocity.mag_sq_ext()) / ld(max_vel))
    };
    let params: Vec<u32> = match big {
        Some((_, scale, size)) => {
            let o = coll.coll_rect_offset;
            vec![
                pf(scale),
                size,
                pf(col_size),
                pf(vel),
                pf(o.x),
                pf(o.y),
                pf(o.z),
                pf(st(ld(o.x) + ld(coll.up_length))),
                pf(st(ld(o.y) + ld(coll.right_length))),
                pf(st(ld(o.z) + ld(coll.forward_length))),
            ]
        }
        None => vec![pf(col_size), pf(vel)],
    };
    let e = create(u, effect, Some(FxOwner::Ship(id)), None, None, None, 1.0, 0, &params)?;
    if let Some(fx) = u.effects.as_mut() {
        if let Some(ef) = fx.world.effects.iter_mut().find(|x| x.id == e) {
            ef.flags |= SOF_FORCE_VISIBLE;
        }
    }
    Some(e)
}

// -------------------------------------------------------------------------------- guns

/// `gun->bulletType`-indexed gun event of a ship's race.
fn gun_lod(fx: &Effects, race: usize, bullet_type: usize, kind: usize) -> Option<&[usize]> {
    fx.tables.gun_event(race, bullet_type, kind)
}

/// The colour the original gives bullets of this race and bullet type (`etgBulletColor`).
pub fn bullet_color(u: &Universe, race: usize, bullet_type: usize) -> Option<u32> {
    let fx = u.effects.as_ref()?;
    fx.tables.bullet_color.get(race * hwc_fx::scripts::GUN_SOUNDS.len() + bullet_type).copied()
}

/// The effect part of bullet creation (0x487120): the bullet's own effect (attached to the
/// bullet) and the muzzle flash (attached to the ship's velocity), with the parameters the
/// original passes. `target` is the ship fired at.
pub fn bullet_fired(u: &mut Universe, ship: ShipId, gun: usize, bullet: BulletId, target: Option<ShipId>) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(s) = u.ships.get(ship) else { return };
    let Some(b) = u.bullets.get(bullet) else { return };
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(s.ship_type) else { return };
    let Some(gs) = stat.guns.as_ref().and_then(|g| g.get(gun)) else { return };
    let race = stat.ship_race.index();
    let bt = b.bullet_type as usize;
    let lod = fx.options.lod;
    let options = fx.options;
    // Damage passed to the scripts: the upgrade multiplier against armour 0 times the gun's
    // damage multiplier and the ship's scale.
    let dmg = st(ld(crate::damage::weapon_damage(b.bullet_type, 0)) * ld(gs.damage_mult) * 1.0);
    // `param_1` of the bullet effect: the speed it flies at (the burst speed for burst shots).
    // Beams leave it unset in the original (it then holds the ship pointer); 0 here.
    let fly_speed = match b.weapon {
        crate::bullet::weapon::ION | crate::bullet::weapon::LASER | crate::bullet::weapon::BEASTINFECT => 0.0,
        _ if b.burst => u.tweaks.combat.burst_speed,
        _ => crate::gun::bullet_speed(&s.guns[gun], gs),
    };
    let length = if crate::gun::static_bullet_type(gs, false) == 0x13 { 0.0 } else { gs.bullet_length };
    let big_gun = b.bullet_type == 0x12;
    // The bullet's effect: the burst bullet for burst shots, else the gun event; big-gun
    // shells always use the first level.
    let blod: Option<&[usize]> = if b.burst {
        fx.tables.special_effect(3)
    } else {
        gun_lod(fx, race, bt, GUN_BULLET)
    };
    let level = if big_gun { 0 } else { lod };
    let beffect = blod.and_then(|l| l.get(usize::from(level)).copied());
    let fire = gun_lod(fx, race, bt, GUN_FIRE).and_then(|l| l.get(usize::from(lod)).copied());
    let ship_vel = s.posinfo.velocity;
    let ship_type = s.ship_type as u32;
    let bpos = b.posinfo.position;
    let bcs = b.coordsys;
    let weapon = b.weapon;
    let in_view = in_render_list(u, FxOwner::Ship(ship));
    let _ = target;
    u.effects.as_mut().unwrap().bullet_hit.insert(bullet, (race, bt));
    if let Some(c) = bullet_color(u, race, bt) {
        u.bullets.bullet_mut(bullet).color = c;
    }
    if let Some(e) = beffect {
        if (in_view || big_gun) && options.bullet && frequency_ok(u, e) {
            let params: Vec<u32> = if weapon == crate::bullet::weapon::ION {
                // Beams also get the firing ship's type and the gun's barrel offset (+0x104).
                vec![pf(dmg), pf(fly_speed), pf(length), ship_type, pf(gs.barrel_length)]
            } else {
                vec![pf(dmg), pf(fly_speed), pf(length)]
            };
            if let Some(id) = create(u, e, Some(FxOwner::Bullet(bullet)), None, Some(ship_vel), None, 1.0, EAF_ALL_BUT_NLIPS, &params) {
                u.effects.as_mut().unwrap().bullet_effect.insert(bullet, id);
            }
        }
    }
    if let Some(e) = fire {
        if in_view && options.fire && frequency_ok(u, e) {
            create(u, e, Some(FxOwner::Ship(ship)), Some(bpos), Some(ship_vel), Some(bcs), 1.0, eaf::VELOCITY, &[pf(dmg)]);
        }
    }
}

/// The bullet has no hit effect (`bullet->hitEffect = NULL`, as the Beast cascade does for its
/// beams).
pub fn clear_bullet_hit_effect(u: &mut Universe, bullet: BulletId) {
    if let Some(fx) = u.effects.as_mut() {
        fx.bullet_hit.remove(&bullet);
    }
}

/// Set the `length` parameter (the third, `effect->variable + 8`) of the bullet's effect, as
/// `beastCascadeEffectUpdate` does every frame so that the beam reaches its target.
pub fn set_bullet_effect_length(u: &mut Universe, bullet: BulletId, length: f32) {
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(&e) = fx.bullet_effect.get(&bullet) else { return };
    fx.world.set_variable(e, 2, length.to_bits());
}

/// A bullet is gone: its effect goes with it (beams and big-gun shells keep theirs, which
/// end by themselves).
pub fn bullet_removed(u: &mut Universe, bullet: BulletId) {
    let Some(fx) = u.effects.as_mut() else { return };
    fx.bullet_hit.remove(&bullet);
    if let Some(e) = fx.bullet_effect.remove(&bullet) {
        fx.world.delete_effect(e);
    }
    fx.world.owner_died(FxOwner::Bullet(bullet).id());
}

/// The hit effect of `univBulletCollidedWithTarget`: at the hit point (`t` along the bullet's
/// heading; beams always, other bullets only when `t` is not zero), facing back along the
/// bullet, scaled by the target. `damage` and `fatal` are the script parameters (the damage
/// done and `ApplyDamageToTarget`'s result). The bullet's own effect ends unless it is a beam.
pub fn bullet_hit(u: &mut Universe, bullet: BulletId, target: ObjRef, t: f32, damage: f32, fatal: u32) {
    let Some(fx) = u.effects.as_mut() else { return };
    let Some(b) = u.bullets.get(bullet) else { return };
    let beam_like = matches!(b.weapon, crate::bullet::weapon::ION | crate::bullet::weapon::BEASTINFECT);
    if !beam_like {
        if let Some(e) = fx.bullet_effect.remove(&bullet) {
            fx.world.delete_effect(e);
        }
    }
    if !(b.weapon == crate::bullet::weapon::ION || t != 0.0) {
        return;
    }
    let Some(&(race, bt)) = fx.bullet_hit.get(&bullet) else { return };
    let Some(effect) = fx.pick(gun_lod(fx, race, bt, GUN_HIT)) else { return };
    if !fx.options.hit || !in_render_list(u, FxOwner::from(target)) {
        return;
    }
    let b = u.bullets.get(bullet).unwrap().clone();
    let (tpos, tvel) = match target {
        ObjRef::Ship(id) => match u.ships.get(id) {
            Some(s) => (s.posinfo.position, s.posinfo.velocity),
            None => return,
        },
        ObjRef::Missile(id) => match u.missiles.get(id) {
            Some(m) => (m.posinfo.position, m.posinfo.velocity),
            None => return,
        },
        ObjRef::Bullet(_) => return,
    };
    if !frequency_ok(u, effect) {
        return;
    }
    let mut at = if t == 0.0 {
        b.posinfo.position
    } else {
        Vector::new(
            st(ld(t) * ld(b.heading.x) + ld(b.posinfo.position.x)),
            st(ld(t) * ld(b.heading.y) + ld(b.posinfo.position.y)),
            st(ld(t) * ld(b.heading.z) + ld(b.posinfo.position.z)),
        )
    };
    // Ships: the hit point scaled about the ship's centre by its scale (1).
    let scale = 1.0f32;
    if matches!(target, ObjRef::Ship(_)) {
        at = Vector::new(
            st((ld(at.x) - ld(tpos.x)) * ld(scale) + ld(tpos.x)),
            st(ld(scale) * (ld(at.y) - ld(tpos.y)) + ld(tpos.y)),
            st(ld(scale) * (ld(at.z) - ld(tpos.z)) + ld(tpos.z)),
        );
    }
    if let ObjRef::Ship(id) = target {
        let dir = if t == 0.0 { Vector::ZERO } else { b.heading };
        if let Some(q) = crate::ships::sentinel::bubble_hit(u, id, b.posinfo.position, dir, t) {
            at = q;
        }
    }
    let back = Vector::new(-b.heading.x, -b.heading.y, -b.heading.z);
    let cs = Matrix::coord_sys_from_heading(back);
    create(u, effect, None, Some(at), Some(tvel), Some(cs), scale, 0, &[pf(st(ld(damage) * ld(scale))), fatal]);
}

/// A bullet ran out of life or range (`univUpdateAllPosVelBullets`): its effect is deleted,
/// unless the effect is self-deleting (`ESO_SelfDeleting`), in which case it lets go of the
/// bullet and is told when it died (its fourth variable gets its age). Burst bullets then leave
/// the burst explosion (`EGT_BURST_EXPLOSION_EFFECT`) once any bullet effect was deleted in this
/// pass (the original never resets that flag within the pass).
pub fn bullet_expired(u: &mut Universe, bullet: BulletId) {
    let Some(fx) = u.effects.as_mut() else { return };
    if let Some(e) = fx.bullet_effect.remove(&bullet) {
        let self_deleting = fx
            .world
            .effect(e)
            .is_some_and(|ef| fx.world.lib.statics[ef.stat].special_ops & hwc_fx::compiler::ESO_SELF_DELETING != 0);
        if self_deleting {
            if let Some(ef) = fx.world.effects.iter_mut().find(|x| x.id == e) {
                let age = ef.time_elapsed.to_bits();
                if let Some(v) = ef.variables.get_mut(3) {
                    *v = age;
                }
                ef.owner = None;
                ef.flags &= !(eaf::POSITION | eaf::COORDSYS | eaf::VELOCITY);
            }
        } else {
            fx.world.delete_effect(e);
            fx.bullet_effect_deleted_this_pass = true;
        }
    }
    let Some(b) = u.bullets.get(bullet) else { return };
    if !b.burst || !u.effects.as_ref().is_some_and(|fx| fx.bullet_effect_deleted_this_pass) {
        return;
    }
    let (pos, cs) = (b.posinfo.position, b.coordsys);
    let fx = u.effects.as_ref().unwrap();
    let Some(effect) = fx.pick(fx.tables.special_effect(5)) else { return };
    if frequency_ok(u, effect) {
        create(u, effect, None, Some(pos), Some(Vector::ZERO), Some(cs), 1.0, EAF_ALL_BUT_NLIPS, &[]);
    }
}

/// Start of `univUpdateAllPosVelBullets`.
pub fn begin_bullet_pass(u: &mut Universe) {
    if let Some(fx) = u.effects.as_mut() {
        fx.bullet_effect_deleted_this_pass = false;
    }
}

// -------------------------------------------------------------------------------- missiles

/// The effect part of `missileShoot`: the missile remembers its hit effect, and the launcher
/// shows the gun's fire effect at the missile, facing the launch direction.
pub fn missile_fired(u: &mut Universe, ship: ShipId, missile: MissileId, heading: Vector) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(s) = u.ships.get(ship) else { return };
    let Some(stat) = u.statics.ship(s.ship_type) else { return };
    let Some(m) = u.missiles.get(missile) else { return };
    let race = stat.ship_race.index();
    let bt = m.bullet_type as usize;
    let fire = fx.pick(gun_lod(fx, race, bt, GUN_FIRE));
    let fire_on = fx.options.fire;
    let (pos, damage) = (m.posinfo.position, m.damage);
    // The second parameter by launcher type (0x8773f4..0x8773fc).
    let kind: u32 = match s.ship_type {
        ShipType::sDreadnought => pf(40.0),
        ShipType::sDestroyer => pf(50.0),
        ShipType::sAcolyte | ShipType::bAcolyte => pf(-15.0),
        _ => 0,
    };
    u.effects.as_mut().unwrap().missile_hit.insert(missile, (race, bt));
    let Some(e) = fire else { return };
    if !fire_on || !in_render_list(u, FxOwner::Ship(ship)) || !frequency_ok(u, e) {
        return;
    }
    let cs = Matrix::coord_sys_from_heading(heading);
    create(u, e, Some(FxOwner::Ship(ship)), Some(pos), None, Some(cs), 1.0, eaf::VELOCITY | eaf::NLIPS, &[pf(damage), kind]);
}

/// The missile's hit effect (`missile->hitEffect`). Missiles fired by a launcher remember it;
/// level mines (`scriptSetMineCB`) use the mine-launcher row of their player's race.
fn missile_hit_effect(u: &Universe, missile: MissileId) -> Option<usize> {
    let fx = u.effects.as_ref()?;
    if !fx.options.hit {
        return None;
    }
    let (race, bt) = match fx.missile_hit.get(&missile) {
        Some(&k) => k,
        None => {
            let m = u.missiles.get(missile)?;
            let p = m.player?;
            (u.players.get(p)?.race.index(), 9)
        }
    };
    fx.pick(gun_lod(fx, race, bt, GUN_HIT))
}

/// The hit effect of `univMissileCollidedWithTarget`: at the missile (moved `t` along its
/// heading), with the target's velocity and the missile's orientation.
pub fn missile_hit(u: &mut Universe, missile: MissileId, target: ObjRef, t: f32, damage: f32, fatal: u32) {
    let Some(effect) = missile_hit_effect(u, missile) else { return };
    if !in_render_list(u, FxOwner::from(target)) {
        return;
    }
    let Some(m) = u.missiles.get(missile) else { return };
    let (mpos, mcs) = (m.posinfo.position, m.rotinfo.coordsys);
    let h = m.heading();
    let (tpos, tvel) = match target {
        ObjRef::Ship(id) => match u.ships.get(id) {
            Some(s) => (Some(s.posinfo.position), s.posinfo.velocity),
            None => return,
        },
        ObjRef::Missile(id) => match u.missiles.get(id) {
            Some(n) => (None, n.posinfo.velocity),
            None => return,
        },
        ObjRef::Bullet(_) => return,
    };
    if !frequency_ok(u, effect) {
        return;
    }
    let mut at = if t == 0.0 {
        mpos
    } else {
        Vector::new(st(ld(h.x) * ld(t) + ld(mpos.x)), st(ld(h.y) * ld(t) + ld(mpos.y)), st(ld(h.z) * ld(t) + ld(mpos.z)))
    };
    let scale = 1.0f32;
    if let Some(tp) = tpos {
        at = Vector::new(
            st((ld(at.x) - ld(tp.x)) * ld(scale) + ld(tp.x)),
            st(ld(scale) * (ld(at.y) - ld(tp.y)) + ld(tp.y)),
            st(ld(scale) * (ld(at.z) - ld(tp.z)) + ld(tp.z)),
        );
    }
    create(u, effect, None, Some(at), Some(tvel), Some(mcs), scale, 0, &[pf(st(ld(damage) * ld(scale))), fatal]);
}

/// `DeleteDeadMissile`: a missile that blew up (`death_type` >= 0) leaves its hit effect.
pub fn missile_died(u: &mut Universe, missile: MissileId, death_type: i32) {
    if death_type >= 0 {
        if let Some(effect) = missile_hit_effect(u, missile) {
            if let Some(m) = u.missiles.get(missile) {
                let (p, v, cs, d) = (m.posinfo.position, m.posinfo.velocity, m.rotinfo.coordsys, m.damage);
                if in_render_list(u, FxOwner::Missile(missile)) && frequency_ok(u, effect) {
                    create(u, effect, None, Some(p), Some(v), Some(cs), 1.0, 0, &[pf(d), 0]);
                }
            }
        }
    }
    if let Some(fx) = u.effects.as_mut() {
        fx.missile_hit.remove(&missile);
        fx.world.owner_died(FxOwner::Missile(missile).id());
    }
}

// -------------------------------------------------------------------------------- collisions

/// The effect of `ApplyDamageToCollidingObjects` when one of two colliding ships has
/// `KillerCollDamage`: the Somtaaw very large plasma bomb hit, one velocity step ahead of that
/// ship, facing back along its velocity. `damage` is the smaller health.
pub fn killer_collision(u: &mut Universe, killer: ShipId, damage: f32) {
    let Some(fx) = u.effects.as_ref() else { return };
    let Some(effect) = fx.pick(gun_lod(fx, 0, 16, GUN_HIT)) else { return };
    let Some(s) = u.ships.get(killer) else { return };
    let (p, v) = (s.posinfo.position, s.posinfo.velocity);
    if !in_render_list(u, FxOwner::Ship(killer)) || !frequency_ok(u, effect) {
        return;
    }
    let at = Vector::new(p.x + v.x, p.y + v.y, p.z + v.z);
    let mut back = Vector::new(-v.x, -v.y, -v.z);
    back.normalize();
    let cs = Matrix::coord_sys_from_heading(back);
    create(u, effect, None, Some(at), Some(v), Some(cs), 0.0, 0, &[pf(damage), 1]);
}

// -------------------------------------------------------------------------------- presentation

/// One frame of presentation: refresh the owners from the simulation, advance the particles
/// to the universe clock (`etgEffectDraw` for every effect) and append what to draw to `out`.
pub fn draw(u: &mut Universe, out: &mut RenderList) {
    if u.effects.is_none() {
        return;
    }
    sync_owners(u);
    if let Some(fx) = u.effects.as_mut() {
        fx.world.draw(Some(out));
    }
}

/// [`draw`] leaving out what `hide` picks (they still run as if drawn; see
/// [`FxWorld::draw_except`](hwc_fx::FxWorld::draw_except)). Presentation only.
pub fn draw_except(u: &mut Universe, out: &mut RenderList, hide: impl Fn(&hwc_fx::runtime::Effect) -> bool) {
    if u.effects.is_none() {
        return;
    }
    sync_owners(u);
    if let Some(fx) = u.effects.as_mut() {
        fx.world.draw_except(out, hide);
    }
}

/// The requests the effect scripts made during the last update (sounds, chatter...).
pub fn events(u: &Universe) -> &[FxEvent] {
    u.effects.as_ref().map_or(&[], |fx| fx.events.as_slice())
}

/// Whether presentation should draw ship `id` (hidden or omitted by its death effect: no).
pub fn ship_drawn(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ships.get(id) else { return false };
    if s.flags & sof::HIDE != 0 {
        return false;
    }
    !u.effects.as_ref().is_some_and(|fx| fx.omit_from_render.contains(&id))
}
