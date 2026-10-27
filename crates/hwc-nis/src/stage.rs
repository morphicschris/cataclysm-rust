//! An NIS on the simulation: the objects `nisNewObjectCreate` (0x4c80e0) puts into the
//! universe, the poses written back every step, the object events, and what `nisStop` leaves
//! behind. The universe side is `hwc_sim::nis_support`.
//!
//! * Ship paths (races 0..6) become ships of player 0 when their race is the local player's,
//!   of player 1 otherwise, flagged `SOF_DontApplyPhysics | SOF_Selectable | SOF_Targetable |
//!   SOF_NISShip` with health `REALlyBig` (invincible). A Somtaaw mothership gets the modules
//!   the fleet carried into the mission (`spGetHyperspaceShipTypesAttachedToType12`,
//!   [`nis_support::create_nis_mothership_modules`]; in mission 1 the docking bay); the NIS
//!   deletes them when it stops.
//! * Asteroid paths become asteroids, deleted at once when the NIS stops
//!   (`univReallyDeleteThisResourceRightNow`) unless they remain.
//! * Missile paths become missiles of their race living as long as the NIS; effect paths get an
//!   anchor whose effect `nisObjectShow` creates and `nisObjectHide` deletes.
//! * Dust and gas clouds, nebulae, derelict-table and generic objects are not created (the
//!   simulation has no such objects; none of the shipped NIS files uses them).
//! * Every step the NIS pose goes onto the object (`univSetShipPosition`, the clamped modules
//!   follow), with the step's displacement as velocity: capped at the type's top speed for the
//!   first seven ship types (the original tests the type against `NUM_RACES`), and the trails
//!   cut when the object jumped.
//! * `hideUniverseToggle` is `univHideJustAboutEverything` / `univUnhideJustAboutEverything`.
//! * The `NISlensflareSource1` object carries the NIS sun flare ([`Stage::sun`]).

use hwc_math::fpu::{ld, st};
use hwc_math::{Matrix, Vector};
use hwc_sim::base::{self, CreateOptions};
use hwc_sim::nis_support::{self, REALLY_BIG as SIM_REALLY_BIG};
use hwc_sim::object::{sof, MissileId, ShipId};
use hwc_sim::resource::{self, ResourceId, ResourceKind};
use hwc_sim::statics::{ShipRace, ShipType};
use hwc_sim::Universe;

use crate::data::Category;
use crate::events::Action;
use crate::player::{omf, Fired, NisPlayer, StopReport};

/// `REALlyBig`, the health of an invincible NIS object.
pub const REALLY_BIG: f32 = SIM_REALLY_BIG;

/// The type name of the object that carries the NIS sun flare.
pub const SUN_SOURCE: &str = "NISlensflareSource1";

/// The universe object an NIS path drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Ship(ShipId),
    Resource(ResourceId),
    Missile(MissileId),
    /// An effect path: its anchor, the effect static, and the effect while shown.
    Effect {
        owner: u32,
        stat: Option<usize>,
        effect: Option<u32>,
    },
    None,
}

/// What the NIS sun flare does after a step (the presentation layer owns the flare).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SunEvent {
    Show,
    Hide,
}

/// The NIS objects in a universe.
#[derive(Debug, Clone, Default)]
pub struct Stage {
    pub handles: Vec<Handle>,
    /// Modules the NIS created for its motherships (deleted when it stops).
    pub extra_ships: Vec<ShipId>,
    /// `hideUniverseToggle` has hidden the universe.
    universe_hidden: bool,
    /// The path that carries the NIS sun flare, if any.
    pub sun: Option<usize>,
    /// Show and hide events of the sun's path since the last call of
    /// [`take_sun_events`](Self::take_sun_events).
    sun_events: Vec<SunEvent>,
    /// Lines describing objects or events the stage could not handle.
    pub log: Vec<String>,
}

/// The ship type of a path name.
pub fn ship_type(name: &str) -> Option<ShipType> {
    ShipType::from_name(name)
}

/// Asteroid kind of a path name (`asteroid1` .. `asteroid4`; `asteroid0` exists too).
pub fn asteroid_kind(name: &str) -> Option<u8> {
    let l = name.to_ascii_lowercase();
    l.strip_prefix("asteroid")?.parse().ok()
}

fn is_ship(u: &Universe, h: Handle) -> Option<ShipId> {
    match h {
        Handle::Ship(id) if u.ships.get(id).is_some() => Some(id),
        _ => None,
    }
}

impl Stage {
    /// `nisNewObjectCreate` for every path, after `nisStart` (the player already holds the
    /// starting poses). `mission` decides the mothership's modules; `module_types` are the
    /// module types the fleet carried into the mission
    /// ([`nis_support::hyperspace_ship_types_attached_to_mothership`], empty for the opening).
    pub fn create(u: &mut Universe, player: &NisPlayer, mission: u32, module_types: &[ShipType]) -> Stage {
        let mut stage = Stage::default();
        // `nisStart` sets `nisIsRunning` before it creates the objects.
        u.nis_running = true;
        let local_race = u.players.first().map(|p| p.race);
        for (i, path) in player.data.objects.iter().enumerate() {
            let state = &player.objects[i];
            let h = match path.category {
                Category::Ship(race) => {
                    let Some(t) = ship_type(&path.type_name) else {
                        stage.log.push(format!("{}: unknown ship type", path.name));
                        stage.handles.push(Handle::None);
                        continue;
                    };
                    if path.type_name.eq_ignore_ascii_case(SUN_SOURCE) && stage.sun.is_none() {
                        stage.sun = Some(i);
                    }
                    let race = ShipRace::from_index(race as usize);
                    let owner = if race == local_race { 0 } else { 1.min(u.players.len() - 1) };
                    let opts = CreateOptions {
                        no_modules: true,
                        ..Default::default()
                    };
                    match base::create_ship(u, t, owner, state.position, opts) {
                        Ok(id) => {
                            let s = u.ships.ship_mut(id);
                            s.health = REALLY_BIG;
                            s.flags |= sof::DONT_APPLY_PHYSICS | sof::SELECTABLE | sof::TARGETABLE | sof::NIS_SHIP;
                            if t == ShipType::sMothership {
                                let made = nis_support::create_nis_mothership_modules(u, id, module_types, mission as i32);
                                for &m in &made {
                                    if let Some(s) = u.ships.get_mut(m) {
                                        // Shown with their host when the universe is hidden.
                                        s.flags |= sof::NIS_SHIP;
                                    }
                                }
                                stage.extra_ships.extend(made);
                            }
                            for m in hwc_sim::clamp::slaves_of(u, id) {
                                if let Some(s) = u.ships.get_mut(m) {
                                    s.flags |= sof::DONT_APPLY_PHYSICS;
                                }
                            }
                            Handle::Ship(id)
                        }
                        Err(e) => {
                            stage.log.push(format!("{}: {e:?}", path.name));
                            Handle::None
                        }
                    }
                }
                Category::Asteroid => match asteroid_kind(&path.type_name)
                    .and_then(|k| resource::add_resource(u, ResourceKind::Asteroid(k), state.position))
                {
                    Some(id) => {
                        if let Some(r) = u.base.resources.get_mut(id) {
                            r.health = REALLY_BIG;
                            r.flags |= sof::NIS_SHIP;
                        }
                        Handle::Resource(id)
                    }
                    None => {
                        stage.log.push(format!("{}: no asteroid statics", path.name));
                        Handle::None
                    }
                },
                Category::Missile(race) => {
                    let race = ShipRace::from_index(race as usize).unwrap_or(ShipRace::Somtaaw);
                    match nis_support::create_nis_missile(u, race, state.position, player.data.length) {
                        Some(id) => Handle::Missile(id),
                        None => {
                            stage.log.push(format!("{}: no missile for {race:?}", path.name));
                            Handle::None
                        }
                    }
                }
                Category::Effect => {
                    let owner = nis_support::nis_effect_owner(i);
                    let stat = nis_support::nis_effect_static(u, &path.type_name);
                    if stat.is_none() {
                        stage.log.push(format!("{}: no effect {}", path.name, path.type_name));
                    }
                    nis_support::set_nis_effect_pose(u, owner, state.position, state.velocity, state.coordsys);
                    Handle::Effect { owner, stat, effect: None }
                }
                other => {
                    stage.log.push(format!("{}: {other:?} objects are not created", path.name));
                    Handle::None
                }
            };
            stage.handles.push(h);
        }
        stage.write_poses(u, player, true);
        stage
    }

    /// Puts the player's poses on the universe objects (`nisUpdate`'s second half). Call after
    /// each [`NisPlayer::tick`]. Objects that left the universe stop being driven.
    pub fn write_poses(&mut self, u: &mut Universe, player: &NisPlayer, all: bool) {
        let statics = u.statics.clone();
        for (i, o) in player.objects.iter().enumerate() {
            if !(o.posed || all) || o.flags & omf::CURVES_DELETED != 0 {
                continue;
            }
            match self.handles.get(i).copied().unwrap_or(Handle::None) {
                Handle::Ship(id) => {
                    if u.ships.get(id).is_none() {
                        continue;
                    }
                    nis_support::set_ship_position(u, id, o.position);
                    let s = u.ships.ship_mut(id);
                    let t = s.ship_type;
                    s.rotinfo.coordsys = o.coordsys;
                    let mut v = o.velocity;
                    if o.moving && t.index() < 7 {
                        if let Some(st) = statics.ship(t) {
                            v.cap(st.header.max_velocity);
                        }
                    }
                    s.posinfo.velocity = v;
                    s.posinfo.is_moving = u16::from(o.moving);
                    if let Some(st) = statics.ship(t) {
                        hwc_sim::physics::update_obj_rot_info(s, st);
                    }
                    follow_host(u, id);
                }
                Handle::Resource(id) => {
                    if let Some(r) = u.base.resources.get_mut(id) {
                        r.position = o.position;
                        r.coordsys = o.coordsys;
                        r.velocity = o.velocity;
                        r.is_moving = o.moving;
                    }
                }
                Handle::Missile(id) => {
                    if let Some(m) = u.missiles.get_mut(id) {
                        m.posinfo.position = o.position;
                        m.posinfo.velocity = o.velocity;
                        m.rotinfo.coordsys = o.coordsys;
                    }
                }
                Handle::Effect { owner, .. } => {
                    nis_support::set_nis_effect_pose(u, owner, o.position, o.velocity, o.coordsys);
                }
                Handle::None => {}
            }
        }
    }

    /// Ships whose trails the last step cut (they jumped [`crate::player::JUMP_DISTANCE`]).
    pub fn jumped(&self, u: &Universe, player: &NisPlayer) -> Vec<ShipId> {
        player
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| o.jumped)
            .filter_map(|(i, _)| is_ship(u, self.handles[i]))
            .collect()
    }

    /// The universe object of path `i`, if it still exists.
    pub fn ship(&self, u: &Universe, i: usize) -> Option<ShipId> {
        self.handles.get(i).and_then(|&h| is_ship(u, h))
    }

    /// Where the NIS puts the sun flare's source now.
    pub fn sun_position(&self, player: &NisPlayer) -> Option<Vector> {
        self.sun.map(|i| player.objects[i].position)
    }

    /// The sun source's show and hide events since the last call.
    pub fn take_sun_events(&mut self) -> Vec<SunEvent> {
        std::mem::take(&mut self.sun_events)
    }

    /// Objects that died or were deleted by the simulation stop being driven
    /// (`nisObjectDied`).
    pub fn forget_dead(&self, u: &Universe, player: &mut NisPlayer) {
        for (i, h) in self.handles.iter().enumerate() {
            let gone = match *h {
                Handle::Ship(id) => u.ships.get(id).is_none_or(|s| s.flags & sof::DEAD != 0),
                Handle::Resource(id) => u.base.resources.get(id).is_none(),
                Handle::Missile(id) => u.missiles.get(id).is_none(),
                Handle::Effect { .. } | Handle::None => false,
            };
            if gone {
                player.objects[i].flags |= omf::OBJECT_DIED | omf::CURVES_DELETED;
            }
        }
    }

    /// Applies the object events of a step; returns the events for the presentation layer
    /// (speech, music, fades, lights, subtitles...).
    pub fn apply(&mut self, u: &mut Universe, fired: &[Fired]) -> Vec<Fired> {
        let mut rest = Vec::new();
        for f in fired {
            let h = f.object.and_then(|i| self.handles.get(i).copied()).unwrap_or(Handle::None);
            let ship = is_ship(u, h);
            match &f.action {
                Action::Show | Action::Hide => {
                    let hide = f.action == Action::Hide;
                    if f.object.is_some() && f.object == self.sun {
                        self.sun_events.push(if hide { SunEvent::Hide } else { SunEvent::Show });
                    }
                    if let (Handle::Effect { owner, stat, effect }, Some(i)) = (h, f.object) {
                        // Effects are created when shown (not while seeking) and deleted when
                        // hidden.
                        let effect = match (hide, effect, stat) {
                            (true, Some(e), _) => {
                                nis_support::nis_effect_hide(u, e);
                                None
                            }
                            (false, None, Some(stat)) if !f.seeking => nis_support::nis_effect_show(u, owner, stat),
                            (_, e, _) => e,
                        };
                        self.handles[i] = Handle::Effect { owner, stat, effect };
                    } else {
                        set_hidden(u, h, hide);
                    }
                    rest.push(f.clone());
                }
                Action::ColorScheme(n) => {
                    if let Some(id) = ship {
                        u.ships.ship_mut(id).color_scheme = (*n).max(0) as usize;
                        for m in hwc_sim::clamp::slaves_of(u, id) {
                            if let Some(s) = u.ships.get_mut(m) {
                                s.color_scheme = (*n).max(0) as usize;
                            }
                        }
                    }
                }
                Action::DamageLevel(frac) => {
                    if let Some(id) = ship {
                        let t = u.ships.ship(id).ship_type;
                        if let Some(info) = u.statics.clone().ship(t) {
                            u.ships.ship_mut(id).health = st(ld(*frac) * ld(info.health.max_health));
                        }
                    }
                }
                Action::Invincible => {
                    if let Some(id) = ship {
                        u.ships.ship_mut(id).health = REALLY_BIG;
                    }
                }
                Action::Vincible => {
                    if let Some(id) = ship {
                        let t = u.ships.ship(id).ship_type;
                        if let Some(st) = u.statics.clone().ship(t) {
                            u.ships.ship_mut(id).health = st.health.max_health;
                        }
                    }
                }
                Action::Attack { target } => {
                    let tgt = self.handles.get(*target).and_then(|&t| is_ship(u, t));
                    if let (Some(a), Some(t)) = (ship, tgt) {
                        let same = u.ships.ship(a).player == u.ships.ship(t).player;
                        if u.ships.ship(a).ship_type == ShipType::bMothership {
                            hwc_sim::ships::cl_special(u, &[a], &[t]);
                        } else if same {
                            hwc_sim::command::cl_attack(u, &[a], &[t]);
                        } else {
                            hwc_sim::command::cl_passive_attack(u, &[a], &[t]);
                        }
                    }
                }
                Action::Repair { target } => {
                    let tgt = self.handles.get(*target).and_then(|&t| is_ship(u, t));
                    if let (Some(a), Some(t)) = (ship, tgt) {
                        hwc_sim::ships::cl_special(u, &[a], &[t]);
                    }
                }
                Action::HaltAttack => {
                    if let Some(id) = ship {
                        hwc_sim::command::remove_ships_from_doing_stuff(u, &[id]);
                    }
                }
                Action::Death { kind } => {
                    if f.seeking {
                        continue;
                    }
                    match h {
                        Handle::Ship(id) if u.ships.get(id).is_some() => {
                            hwc_sim::damage::finish_ship_death(u, id, *kind);
                        }
                        Handle::Resource(id) => u.base.resources.dead.push(id),
                        _ => {}
                    }
                }
                Action::HideUniverseToggle => {
                    self.toggle_universe(u);
                    rest.push(f.clone());
                }
                Action::Fire => {
                    if f.seeking {
                        continue;
                    }
                    if let Some(id) = ship {
                        if !nis_support::fire(u, id) {
                            self.log.push(format!("{:?}: {:?} cannot fire without a target", f.object, u.ships.ship(id).ship_type));
                        }
                    }
                }
                _ => rest.push(f.clone()),
            }
        }
        rest
    }

    /// `univHideJustAboutEverything` / `univUnhideJustAboutEverything`.
    pub fn toggle_universe(&mut self, u: &mut Universe) {
        if self.universe_hidden {
            nis_support::unhide_just_about_everything(u);
        } else {
            nis_support::hide_just_about_everything(u);
        }
        self.universe_hidden = !self.universe_hidden;
    }

    /// `nisStop`'s object part: `remainAtEnd` objects become ordinary objects (stopped unless
    /// they keep moving, vincible, counted by the unit caps), the others, the extra modules and
    /// the effects leave the universe. A hidden universe is shown again.
    pub fn stop(&mut self, u: &mut Universe, report: &StopReport) {
        u.nis_running = false;
        for id in self.extra_ships.drain(..) {
            if u.ships.get(id).is_some() {
                hwc_sim::damage::really_delete_ship(u, id);
            }
        }
        for &(i, keep_moving) in &report.remain {
            match self.handles[i] {
                Handle::Ship(id) => {
                    let Some(s) = u.ships.get_mut(id) else { continue };
                    s.flags &= !sof::DONT_APPLY_PHYSICS;
                    if !keep_moving {
                        s.posinfo.velocity = Vector::ZERO;
                        s.posinfo.force = Vector::ZERO;
                        s.rotinfo.torque = Vector::ZERO;
                        s.rotinfo.rotspeed = Vector::ZERO;
                        s.posinfo.is_moving = 0;
                    }
                    let t = s.ship_type;
                    if let Some(st) = u.statics.clone().ship(t) {
                        u.ships.ship_mut(id).health = st.health.max_health;
                    }
                    hwc_sim::construction::unit_cap_create_ship(u, id);
                    if matches!(t, ShipType::sMothership | ShipType::bMothership) {
                        for m in hwc_sim::clamp::slaves_of(u, id) {
                            if let Some(s) = u.ships.get_mut(m) {
                                s.flags &= !sof::DONT_APPLY_PHYSICS;
                            }
                        }
                    }
                    if let Some(s) = u.ships.get_mut(id) {
                        s.flags &= !sof::NIS_SHIP;
                    }
                }
                Handle::Resource(id) => {
                    if let Some(r) = u.base.resources.get_mut(id) {
                        r.flags &= !(sof::DONT_APPLY_PHYSICS | sof::NIS_SHIP);
                        r.velocity = Vector::ZERO;
                        r.is_moving = false;
                    }
                }
                _ => {}
            }
        }
        for &i in &report.delete {
            match self.handles[i] {
                Handle::Ship(id) => {
                    if u.ships.get(id).is_some() {
                        for m in hwc_sim::clamp::slaves_of(u, id) {
                            if u.ships.get(m).is_some() {
                                hwc_sim::damage::really_delete_ship(u, m);
                            }
                        }
                        hwc_sim::damage::really_delete_ship(u, id);
                    }
                }
                Handle::Resource(id) => resource::really_delete_resource(u, id),
                _ => {}
            }
        }
        for h in &self.handles {
            if let Handle::Effect { owner, .. } = *h {
                nis_support::nis_effect_remove(u, owner);
            }
        }
        if report.unhide_universe && self.universe_hidden {
            self.toggle_universe(u);
        }
    }
}

/// `nisObjectShow` / `nisObjectHide`: the object and the ships clamped to it.
fn set_hidden(u: &mut Universe, h: Handle, hide: bool) {
    let apply = |f: &mut u32| {
        if hide {
            *f |= sof::HIDE;
        } else {
            *f &= !sof::HIDE;
        }
    };
    match h {
        Handle::Ship(id) => {
            if let Some(s) = u.ships.get_mut(id) {
                apply(&mut s.flags);
            }
            for m in hwc_sim::clamp::slaves_of(u, id) {
                if let Some(s) = u.ships.get_mut(m) {
                    apply(&mut s.flags);
                }
            }
        }
        Handle::Resource(id) => {
            if let Some(r) = u.base.resources.get_mut(id) {
                apply(&mut r.flags);
            }
        }
        Handle::Missile(id) => {
            if let Some(m) = u.missiles.get_mut(id) {
                apply(&mut m.flags);
            }
        }
        Handle::Effect { .. } | Handle::None => {}
    }
}

/// `updateClampSlavesFromHost`: the clamped ships take their pose from the host.
fn follow_host(u: &mut Universe, host: ShipId) {
    let Some(h) = u.ships.get(host) else { return };
    let (hp, hs, hv) = (h.posinfo.position, h.rotinfo.coordsys, h.posinfo.velocity);
    for m in hwc_sim::clamp::slaves_of(u, host) {
        let Some(s) = u.ships.get_mut(m) else { continue };
        let Some(info) = s.base.clamp.as_ref() else { continue };
        let off = hs.mul_vec(info.rel_position);
        let pos = Vector::new(
            st(ld(off.x) + ld(hp.x)),
            st(ld(off.y) + ld(hp.y)),
            st(ld(off.z) + ld(hp.z)),
        );
        let sys: Matrix = hs.mul_mat(&info.rel_coordsys);
        nis_support::set_ship_position(u, m, pos);
        let s = u.ships.ship_mut(m);
        s.rotinfo.coordsys = sys;
        s.posinfo.velocity = hv;
        follow_host(u, m);
    }
}
