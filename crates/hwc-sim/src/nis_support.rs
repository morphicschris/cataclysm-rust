//! What the NIS player (`NIS.c`, played by `hwc-nis`) needs from the universe beyond the
//! ordinary ship code: the universe hide toggle, the position setter the NIS moves ships
//! with, the untargeted fire of the `fire` event, the Somtaaw mothership's modules for an
//! NIS ship, and the NIS's missile and effect objects.
//!
//! * [`hide_just_about_everything`] / [`unhide_just_about_everything`]
//!   (`univHideJustAboutEverything` 0x542cf0 / `univUnhideJustAboutEverything` 0x542d70);
//! * [`set_ship_position`] (`univSetShipPosition` 0x548180);
//! * [`fire`] (`nisFire` 0x4ca870: the type's `CustShipFire` without a target);
//! * [`hyperspace_ship_types_attached_to_mothership`] (`spGetHyperspaceShipTypesAttachedToType12`
//!   0x5171e0) and [`create_nis_mothership_modules`] (the mothership part of
//!   `nisNewObjectCreate` 0x4c80e0);
//! * [`create_nis_missile`] (the `Missile` paths of `nisNewObjectCreate`, `univAddMissile`);
//! * NIS effects: [`nis_effect_owner`], [`set_nis_effect_pose`], [`nis_effect_show`],
//!   [`nis_effect_hide`], [`nis_effect_remove`] (`nisObjectShow` / `nisObjectHide` /
//!   `nisUpdate` / `nisStop` for effect paths).
//!
//! The derelict-table objects (`univAddDerelict`, `univAddDerelictByStatInfo`) and gas clouds
//! (`univAddGasCloud`) of `nisNewObjectCreate` have no counterpart: the simulation has no
//! derelict or gas-cloud objects yet (and none of the shipped NIS files has such paths; their
//! objects are ships, asteroids and the derelict-race ships).

use hwc_fx::runtime::{eaf, EffectId, Owner, OwnerId};
use hwc_math::fpu::{ld, st};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

use crate::base::{self, CreateOptions};
use crate::combat_statics::{is_acolyte, FireKind};
use crate::object::{sof, MissileId, ObjRef, ShipId};
use crate::ships::{self, Module};
use crate::statics::{GunType, ShipRace, ShipType};
use crate::universe::Universe;

/// `REALlyBig` (0x6258d727): the health of an invincible NIS object.
pub const REALLY_BIG: f32 = 1e21;

/// `SOF_HideSave2` (bit 28): the object was hidden already when
/// [`hide_just_about_everything`] ran, so [`unhide_just_about_everything`] leaves it hidden.
pub const HIDE_SAVE2: u32 = 0x1000_0000;

/// `SOF_ForceVisible`: set on NIS effects (always drawn).
pub const FORCE_VISIBLE: u32 = 0x0001_0000;

// ----------------------------------------------------------------------------- hide toggle

/// `univHideJustAboutEverything` (0x542cf0): every ship, bullet, missile and effect that is
/// not an NIS object (`SOF_NISShip`) is hidden and leaves the render list. Objects hidden
/// already are marked [`HIDE_SAVE2`] instead, so the matching unhide keeps them hidden.
/// Resources (asteroids, dust clouds, nebulae) are left alone.
pub fn hide_just_about_everything(u: &mut Universe) {
    fn hide(flags: &mut u32) -> bool {
        if *flags & sof::NIS_SHIP != 0 {
            return false;
        }
        if *flags & sof::HIDE != 0 {
            *flags |= HIDE_SAVE2;
            false
        } else {
            *flags |= sof::HIDE;
            true
        }
    }
    let list = u.space_obj_list.clone();
    for o in list {
        let newly_hidden = match o {
            ObjRef::Ship(id) => u.ships.get_mut(id).is_some_and(|s| hide(&mut s.flags)),
            ObjRef::Bullet(id) => u.bullets.get_mut(id).is_some_and(|b| hide(&mut b.flags)),
            ObjRef::Missile(id) => u.missiles.get_mut(id).is_some_and(|m| hide(&mut m.flags)),
        };
        if newly_hidden {
            u.render_list.retain(|&r| r != o);
        }
    }
    if let Some(fx) = u.effects.as_mut() {
        for e in &mut fx.world.effects {
            hide(&mut e.flags);
        }
    }
}

/// `univUnhideJustAboutEverything` (0x542d70): every object (resources included) is shown,
/// except those marked [`HIDE_SAVE2`], which only lose the mark. Like the original this also
/// shows objects that were hidden after [`hide_just_about_everything`] ran.
pub fn unhide_just_about_everything(u: &mut Universe) {
    fn unhide(flags: &mut u32) {
        if *flags & HIDE_SAVE2 != 0 {
            *flags &= !HIDE_SAVE2;
        } else {
            *flags &= !sof::HIDE;
        }
    }
    let list = u.space_obj_list.clone();
    for o in list {
        match o {
            ObjRef::Ship(id) => {
                if let Some(s) = u.ships.get_mut(id) {
                    unhide(&mut s.flags);
                }
            }
            ObjRef::Bullet(id) => {
                if let Some(b) = u.bullets.get_mut(id) {
                    unhide(&mut b.flags);
                }
            }
            ObjRef::Missile(id) => {
                if let Some(m) = u.missiles.get_mut(id) {
                    unhide(&mut m.flags);
                }
            }
        }
    }
    for r in &mut u.base.resources.list {
        unhide(&mut r.flags);
    }
    if let Some(fx) = u.effects.as_mut() {
        for e in &mut fx.world.effects {
            unhide(&mut e.flags);
        }
    }
}

// ----------------------------------------------------------------------------- position

/// `univSetShipPosition` (0x548180): moves the ship, moving its fog-of-war viewer first when
/// it provides vision (`fowShipProvidesVision`, `univShipFowMoveViewer` from the old position
/// to the new). The original does nothing else here: the collision blob is not touched (the
/// blobs are rebuilt by the universe update).
pub fn set_ship_position(u: &mut Universe, id: ShipId, position: Vector) {
    let Some(old) = u.ships.get(id).map(|s| s.posinfo.position) else { return };
    if crate::fow::provides_vision(u, id) {
        crate::fow::move_viewer(u, id, old, position);
    }
    u.ships.ship_mut(id).posinfo.position = position;
}

// ----------------------------------------------------------------------------- fire

/// `nisFire` (0x4ca870): the ship's `CustShipFire` with no target. Returns whether a fire
/// routine ran.
///
/// Supported are the routines that work without a target: `GenericInterceptorFire` (the
/// active gun, then the next), `DefaultShipFire` and `GenericCapitalFire` (every ready gun;
/// bullets fly along the guns), the minelayers' fire and `NomadMoonFire` (which pulses only
/// when called without a target). Types whose routine needs a target (Acolyte link missiles,
/// ACV, Hive, cloaked fighter, multi-beam frigates) return false without firing; so do types
/// without a fire routine. The shipped NIS files fire only a Taiidan heavy interceptor
/// (`nislet01`) and the Nomad moon (`n11`).
pub fn fire(u: &mut Universe, id: ShipId) -> bool {
    let Some(t) = u.ships.get(id).map(|s| s.ship_type) else { return false };
    let Some(kind) = u.combat.get(t).map(|c| c.fire) else { return false };
    match kind {
        FireKind::Interceptor => interceptor_fire_untargeted(u, id),
        FireKind::Default => default_fire_untargeted(u, id),
        FireKind::GenericCapital => generic_capital_fire_untargeted(u, id),
        FireKind::Unsupported => match ships::module_of(t) {
            Module::NomadMoon => ships::nomad::fire_untargeted(u, id),
            Module::Mimic | Module::Mcv => interceptor_fire_untargeted(u, id),
            Module::IonArray | Module::P1StandardCorvette | Module::Mothership => default_fire_untargeted(u, id),
            Module::Minelayer => ships::minelayer::fire_at(u, id, None),
            _ => return false,
        },
        FireKind::Acolyte | FireKind::MultiBeam | FireKind::None => return false,
    }
    true
}

/// `GenericInterceptorFire` with no target.
fn interceptor_fire_untargeted(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let g = {
        let s = u.ships.ship_mut(id);
        s.interceptor.last_time_fired = now;
        s.interceptor.active_gun as usize
    };
    if g < u.ships.ship(id).guns.len() {
        crate::gun::shoot(u, id, g, None);
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

/// `DefaultShipFire` with no target.
fn default_fire_untargeted(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(u.ships.ship(id).ship_type) else { return };
    let Some(guns) = stat.guns.as_ref() else { return };
    for (gi, gs) in guns.iter().enumerate() {
        if !crate::gun::ready(u, id, gi) {
            continue;
        }
        match gs.gun_type {
            Some(GunType::Fixed) | Some(GunType::Gimble) | Some(GunType::NewGimble) => {
                crate::gun::shoot(u, id, gi, None);
            }
            Some(GunType::MissileLauncher) | Some(GunType::MineLauncher) if u.ships.ship(id).guns[gi].num_missiles > 0 => {
                crate::gun::shoot_missile(u, id, gi, None);
            }
            _ => {}
        }
    }
}

/// `GenericCapitalFire` with no target.
fn generic_capital_fire_untargeted(u: &mut Universe, id: ShipId) {
    let n = u.ships.ship(id).guns.len();
    for gi in 0..n {
        if !crate::gun::ready(u, id, gi) {
            continue;
        }
        let level = u.ships.ship(id).guns[gi].level;
        if matches!(level, 0 | 1 | 2 | 7) {
            crate::gun::shoot(u, id, gi, None);
        } else if matches!(level, 5 | 6) && u.ships.ship(id).guns[gi].num_missiles > 0 {
            crate::gun::shoot_missile(u, id, gi, None);
        }
    }
}

// ----------------------------------------------------------------------------- mothership modules

/// `spGetHyperspaceShipTypesAttachedToType12` (0x5171e0): the types of the ships in the
/// campaign's hyperspace list (`ShipsInHyperspace`, in list order: pass
/// `game.hyper.in_hyperspace`) that are clamped to a Somtaaw mothership (type 18), i.e. the
/// modules the fleet carried into this mission. `nisNewObjectCreate` builds an NIS
/// mothership's modules from it.
pub fn hyperspace_ship_types_attached_to_mothership(u: &Universe, in_hyperspace: &[ShipId]) -> Vec<ShipType> {
    in_hyperspace
        .iter()
        .filter_map(|&id| u.ships.get(id))
        .filter(|s| {
            s.base
                .clamp
                .as_ref()
                .and_then(|c| c.host)
                .and_then(|h| u.ships.get(h))
                .is_some_and(|h| h.ship_type == ShipType::sMothership)
        })
        .map(|s| s.ship_type)
        .collect()
}

/// The module part of `nisNewObjectCreate` (0x4c80e0) for an NIS Somtaaw mothership `host`:
/// a module of every type in `module_types` (from
/// [`hyperspace_ship_types_attached_to_mothership`]), taken from the end of the list, for
/// player 0 at the mothership's position, invincible (health [`REALLY_BIG`]), selectable and
/// targetable, clamped to the mothership. Mission 4 leaves out the docking bay. A support
/// module goes onto the dock point its counterpart on the player's own mothership occupies
/// (reserving it on the NIS mothership), turned half a turn about its x axis when the point
/// is an odd one of its kind. Mission 1 then adds a docking bay counted by the unit caps and
/// flagged `SOF_DontApplyPhysics`. Returns the modules in creation order (the NIS deletes
/// them when it stops).
pub fn create_nis_mothership_modules(u: &mut Universe, host: ShipId, module_types: &[ShipType], mission: i32) -> Vec<ShipId> {
    let mut made = Vec::new();
    let Some(position) = u.ships.get(host).map(|s| s.posinfo.position) else { return made };
    for &t in module_types.iter().rev() {
        if mission == 4 && t == ShipType::sMothershipDockingBay {
            continue;
        }
        let Ok(id) = base::create_ship(u, t, 0, position, CreateOptions { no_modules: true, ..Default::default() }) else {
            continue;
        };
        {
            let s = u.ships.ship_mut(id);
            s.health = REALLY_BIG;
            s.flags |= sof::SELECTABLE | sof::TARGETABLE;
        }
        if t == ShipType::sMothershipSupport {
            place_support_module(u, id, host);
        }
        crate::clamp::clamp_obj_to_obj(u, id, host);
        made.push(id);
    }
    if mission == 1 {
        if let Ok(bay) = base::create_ship(u, ShipType::sMothershipDockingBay, 0, position, CreateOptions { no_modules: true, ..Default::default() }) {
            crate::construction::unit_cap_create_ship(u, bay);
            u.ships.ship_mut(bay).flags |= sof::SELECTABLE | sof::TARGETABLE | sof::DONT_APPLY_PHYSICS;
            crate::clamp::clamp_obj_to_obj(u, bay, host);
            made.push(bay);
        }
    }
    made
}

/// The support module's dock point: the one the first support module clamped to the local
/// player's mothership has reserved.
fn place_support_module(u: &mut Universe, module: ShipId, host: ShipId) {
    let own = u.base.players.get(u.base.cur_player).and_then(|p| p.mothership);
    let Some(own) = own.filter(|&m| u.ships.get(m).is_some()) else { return };
    let Some(theirs) = crate::clamp::slaves_of(u, own).into_iter().find(|&m| u.ships.get(m).is_some_and(|s| s.ship_type == ShipType::sMothershipSupport))
    else {
        return;
    };
    let idx = u.ships.ship(theirs).base.dock_point;
    if idx == -1 {
        return;
    }
    let idx = idx as usize;
    crate::dock::reserve_specific_dock_point(u, module, host, idx);
    let h = u.ships.ship(host);
    let (hpos, hsys) = (h.posinfo.position, h.rotinfo.coordsys);
    let statics = u.statics.clone();
    let Some(dock) = statics.ship(h.ship_type).and_then(|s| s.dock.as_ref()) else { return };
    let Some(point) = dock.points.get(idx) else { return };
    let off = hsys.mul_vec(point.position);
    let pos = Vector::new(st(ld(off.x) + ld(hpos.x)), st(ld(off.y) + ld(hpos.y)), st(ld(off.z) + ld(hpos.z)));
    let first = point.dock_type.map(|t| dock.first_of_type[t.index()]);
    let s = u.ships.ship_mut(module);
    s.posinfo.position = pos;
    if let Some(first) = first {
        if odd_point(s.base.dock_point, first) {
            s.rotinfo.coordsys = s.rotinfo.coordsys.mul_mat(&half_turn_about_x());
        }
    }
}

/// Whether dock point `idx` is an odd one of its kind (`(idx - first) & 1` on the bytes).
fn odd_point(idx: i32, first_of_type: u8) -> bool {
    (idx as u8).wrapping_sub(first_of_type) & 1 != 0
}

/// `matMakeRotAboutX` by π as single-precision sine and cosine of the float π.
fn half_turn_about_x() -> Matrix {
    let a = ld(std::f32::consts::PI);
    Matrix::rot_about_x(st(fcos(a)), st(fsin(a)))
}

// ----------------------------------------------------------------------------- missiles

/// The `Missile` paths of `nisNewObjectCreate`: [`crate::missile::add_missile`] of `race`,
/// living as long as the NIS (`length`, seconds), at `position`, flagged `SOF_NISShip`.
pub fn create_nis_missile(u: &mut Universe, race: ShipRace, position: Vector, length: f32) -> Option<MissileId> {
    let id = crate::missile::add_missile(u, race)?;
    let m = u.missiles.missile_mut(id);
    m.total_lifetime = length;
    m.posinfo.position = position;
    m.flags |= sof::NIS_SHIP;
    Some(id)
}

// ----------------------------------------------------------------------------- effects

/// High nibble of the owner ids of NIS effect anchors (the zeroed stand-in objects the
/// original creates for effect paths); the other owner kinds use 0, 2, 4 and 6.
const NIS_OWNER_NIBBLE: u32 = 7;

/// The owner id of the anchor of NIS effect path `path` (its index among the NIS's object
/// paths, or any number unique among the NIS effects playing).
pub fn nis_effect_owner(path: usize) -> OwnerId {
    (NIS_OWNER_NIBBLE << 28) | (path as u32 & 0x0fff_ffff)
}

/// Whether `id` is an NIS effect anchor ([`crate::effects::sync_owners`] leaves them alone).
pub fn is_nis_effect_owner(id: OwnerId) -> bool {
    id >> 28 == NIS_OWNER_NIBBLE
}

/// The effect static an effect path names (`nisLoad` resolves the path's type to an
/// `etgeffectstatic` by file name).
pub fn nis_effect_static(u: &Universe, name: &str) -> Option<usize> {
    u.effects.as_ref()?.named(name)
}

/// `nisUpdate` for an effect path: the anchor takes the NIS pose (its effect, created with
/// position, orientation and velocity attached, follows it). Call before
/// [`nis_effect_show`] and every step after.
pub fn set_nis_effect_pose(u: &mut Universe, owner: OwnerId, position: Vector, velocity: Vector, coordsys: Matrix) {
    let Some(fx) = u.effects.as_mut() else { return };
    fx.world.set_owner(owner, Owner { position, velocity, coordsys, nlips: 1.0, is_ship: false, ..Owner::default() });
}

/// `nisObjectShow` for an effect path (not while seeking): `etgEffectCreate` of `stat` on the
/// anchor with `EAF_AllButNLips`, N-Lips 1, made always visible ([`FORCE_VISIBLE`]). `None`
/// without effects in the universe.
pub fn nis_effect_show(u: &mut Universe, owner: OwnerId, stat: usize) -> Option<EffectId> {
    let fx = u.effects.as_mut()?;
    if !fx.world.owners.contains_key(&owner) {
        fx.world.set_owner(owner, Owner { is_ship: false, ..Owner::default() });
    }
    let id = fx.world.create_effect(stat, Some(owner), None, None, None, 1.0, eaf::POSITION | eaf::COORDSYS | eaf::VELOCITY, &[]);
    if let Some(e) = fx.world.effects.iter_mut().find(|e| e.id == id) {
        e.flags |= FORCE_VISIBLE;
    }
    Some(id)
}

/// `nisObjectHide` for an effect path: the effect is deleted (`etgEffectDelete`).
pub fn nis_effect_hide(u: &mut Universe, effect: EffectId) {
    crate::effects::delete(u, effect);
}

/// `nisStop` for an effect path: its effects are deleted (`etgDeleteEffectsOwnedBy`) and the
/// anchor is freed.
pub fn nis_effect_remove(u: &mut Universe, owner: OwnerId) {
    let Some(fx) = u.effects.as_mut() else { return };
    fx.world.delete_effects_owned_by(owner);
    fx.world.owners.remove(&owner);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::resource::{Resource, ResourceId, ResourceKind};
    use crate::statics::{ShipStaticInfo, StaticWorld};
    use crate::tunables::SimTweaks;

    fn ship(t: ShipType, race: ShipRace) -> ShipStaticInfo {
        let mut s = ShipStaticInfo::new(t, String::new());
        s.ship_race = race;
        s.health.max_health = 100.0;
        s.header.coll.coll_sphere_size = 10.0;
        s
    }

    fn world(list: Vec<ShipStaticInfo>) -> StaticWorld {
        crate::statics::test_world(list)
    }

    fn universe() -> Universe {
        let statics = Arc::new(world(vec![
            ship(ShipType::sMothership, ShipRace::Somtaaw),
            ship(ShipType::sMothershipSupport, ShipRace::Somtaaw),
            ship(ShipType::sMothershipDockingBay, ShipRace::Somtaaw),
            ship(ShipType::sMothershipEngineering, ShipRace::Somtaaw),
            ship(ShipType::sRecon, ShipRace::Somtaaw),
            ship(ShipType::tHeavyInterceptor, ShipRace::Taiidan),
        ]));
        let mut u = Universe::new(statics, SimTweaks::compiled(), &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        base::init(&mut u);
        u
    }

    fn add(u: &mut Universe, t: ShipType, player: usize) -> ShipId {
        base::create_ship(u, t, player, Vector::new(10.0, 20.0, 30.0), CreateOptions { no_modules: true, ..Default::default() }).unwrap()
    }

    fn resource(u: &mut Universe, id: u16) -> ResourceId {
        let r = Resource {
            id: ResourceId(id),
            kind: ResourceKind::Asteroid(1),
            flags: 0x0010_4003,
            attributes: 0,
            attributes_param: 0,
            volume: None,
            position: Vector::ZERO,
            velocity: Vector::ZERO,
            force: Vector::ZERO,
            coordsys: Matrix::IDENTITY,
            rotspeed: Vector::ZERO,
            torque: Vector::ZERO,
            is_moving: false,
            coll_position: Vector::ZERO,
            health: 1.0,
            not_accessible: 0,
            resource_value: 100,
            scale: 1.0,
        };
        u.base.resources.list.push(r);
        ResourceId(id)
    }

    #[test]
    fn hide_keeps_nis_objects_and_resources_and_remembers_what_was_hidden() {
        let mut u = universe();
        let plain = add(&mut u, ShipType::sRecon, 0);
        let nis = add(&mut u, ShipType::sRecon, 0);
        let hidden = add(&mut u, ShipType::sRecon, 1);
        u.ships.ship_mut(nis).flags |= sof::NIS_SHIP;
        u.ships.ship_mut(hidden).flags |= sof::HIDE;
        let r = resource(&mut u, 0);
        u.render_list = vec![ObjRef::Ship(plain), ObjRef::Ship(nis)];

        hide_just_about_everything(&mut u);
        let f = |u: &Universe, id| u.ships.ship(id).flags;
        assert_ne!(f(&u, plain) & sof::HIDE, 0);
        assert_eq!(f(&u, plain) & HIDE_SAVE2, 0);
        assert_eq!(f(&u, nis) & (sof::HIDE | HIDE_SAVE2), 0);
        assert_eq!(f(&u, hidden) & (sof::HIDE | HIDE_SAVE2), sof::HIDE | HIDE_SAVE2);
        assert_eq!(u.base.resources.get(r).unwrap().flags & sof::HIDE, 0);
        assert_eq!(u.render_list, vec![ObjRef::Ship(nis)]);

        // Hidden in between: shown by the unhide, as in the original.
        u.ships.ship_mut(nis).flags |= sof::HIDE;
        u.base.resources.get_mut(r).unwrap().flags |= sof::HIDE;
        unhide_just_about_everything(&mut u);
        assert_eq!(f(&u, plain) & sof::HIDE, 0);
        assert_eq!(f(&u, nis) & sof::HIDE, 0);
        assert_eq!(f(&u, hidden) & (sof::HIDE | HIDE_SAVE2), sof::HIDE);
        assert_eq!(u.base.resources.get(r).unwrap().flags & sof::HIDE, 0);
    }

    #[test]
    fn set_ship_position_moves_the_ship() {
        let mut u = universe();
        let id = add(&mut u, ShipType::sRecon, 0);
        let p = Vector::new(-5.0, 6.0, 7.5);
        set_ship_position(&mut u, id, p);
        assert_eq!(u.ships.ship(id).posinfo.position, p);
    }

    #[test]
    fn hyperspace_types_are_the_ships_clamped_to_a_somtaaw_mothership() {
        let mut u = universe();
        let ms = add(&mut u, ShipType::sMothership, 0);
        let a = add(&mut u, ShipType::sMothershipEngineering, 0);
        let b = add(&mut u, ShipType::sMothershipSupport, 0);
        let free = add(&mut u, ShipType::sRecon, 0);
        let other_host = add(&mut u, ShipType::sMothershipDockingBay, 0);
        let on_other = add(&mut u, ShipType::sRecon, 0);
        crate::clamp::clamp_obj_to_obj(&mut u, a, ms);
        crate::clamp::clamp_obj_to_obj(&mut u, b, ms);
        crate::clamp::clamp_obj_to_obj(&mut u, on_other, other_host);
        let list = [free, a, on_other, b, ms];
        assert_eq!(
            hyperspace_ship_types_attached_to_mothership(&u, &list),
            vec![ShipType::sMothershipEngineering, ShipType::sMothershipSupport]
        );
    }

    #[test]
    fn nis_modules_are_made_from_the_end_of_the_list_and_clamped() {
        let mut u = universe();
        let ms = add(&mut u, ShipType::sMothership, 0);
        let types = [ShipType::sMothershipEngineering, ShipType::sMothershipDockingBay];
        let made = create_nis_mothership_modules(&mut u, ms, &types, 2);
        let made_types: Vec<ShipType> = made.iter().map(|&id| u.ships.ship(id).ship_type).collect();
        assert_eq!(made_types, vec![ShipType::sMothershipDockingBay, ShipType::sMothershipEngineering]);
        for &m in &made {
            let s = u.ships.ship(m);
            assert_eq!(s.health, REALLY_BIG);
            assert_eq!(s.flags & (sof::SELECTABLE | sof::TARGETABLE), sof::SELECTABLE | sof::TARGETABLE);
            assert_eq!(crate::clamp::host_of(&u, m), Some(ms));
        }
        // Mission 4 leaves out the bay; mission 1 adds one at the end.
        let ms4 = add(&mut u, ShipType::sMothership, 0);
        let made4 = create_nis_mothership_modules(&mut u, ms4, &types, 4);
        assert_eq!(made4.len(), 1);
        let ms1 = add(&mut u, ShipType::sMothership, 0);
        let made1 = create_nis_mothership_modules(&mut u, ms1, &[], 1);
        assert_eq!(made1.len(), 1);
        let bay = u.ships.ship(made1[0]);
        assert_eq!(bay.ship_type, ShipType::sMothershipDockingBay);
        assert_ne!(bay.flags & sof::DONT_APPLY_PHYSICS, 0);
    }

    #[test]
    fn odd_dock_points_and_the_half_turn() {
        assert!(!odd_point(4, 4));
        assert!(odd_point(5, 4));
        assert!(odd_point(0, 255));
        let m = half_turn_about_x();
        assert_eq!(m.m[0], 1.0);
        assert_eq!(m.m[4], -1.0);
        assert_eq!(m.m[8], -1.0);
        assert!(m.m[5].abs() < 1e-6 && m.m[5] != 0.0);
    }

    #[test]
    fn nis_effect_owner_ids_have_their_own_range() {
        let o = nis_effect_owner(3);
        assert!(is_nis_effect_owner(o));
        for other in [
            crate::effects::FxOwner::Ship(ShipId(3)).id(),
            crate::effects::FxOwner::Missile(MissileId(3)).id(),
            crate::effects::FxOwner::Resource(ResourceId(3)).id(),
        ] {
            assert!(!is_nis_effect_owner(other));
        }
        assert_eq!(crate::effects::FxOwner::from_id(o), None);
    }

    #[test]
    fn really_delete_resource_forgets_it_everywhere() {
        let mut u = universe();
        let a = resource(&mut u, 0);
        let b = resource(&mut u, 1);
        u.base.resources.dead.push(a);
        crate::resource::really_delete_resource(&mut u, a);
        assert!(u.base.resources.get(a).is_none());
        assert!(u.base.resources.get(b).is_some());
        assert!(u.base.resources.dead.is_empty());
        // A second delete does nothing.
        crate::resource::really_delete_resource(&mut u, a);
        assert_eq!(u.base.resources.list.len(), 1);
    }

    #[test]
    fn fire_without_combat_statics_does_nothing() {
        let mut u = universe();
        let id = add(&mut u, ShipType::sRecon, 0);
        let fired = fire(&mut u, id);
        assert_eq!(fired, u.combat.get(ShipType::sRecon).is_some_and(|c| !matches!(c.fire, FireKind::None | FireKind::Acolyte | FireKind::MultiBeam)));
        assert!(!fire(&mut u, ShipId(999)));
    }
}
