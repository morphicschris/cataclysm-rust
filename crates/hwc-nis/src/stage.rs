//! An NIS on the simulation: the objects `nisNewObjectCreate` (0x4c80e0) puts into the
//! universe, the poses written back every step, the object events, and what `nisStop` leaves
//! behind.
//!
//! * Ship paths (races 0..6) become ships of player 0 when their race is the local player's,
//!   of player 1 otherwise, flagged `SOF_DontApplyPhysics | SOF_Selectable | SOF_Targetable |
//!   SOF_NISShip` with health `REALlyBig` (invincible). The Somtaaw mothership comes with its
//!   modules clamped to it, in mission 1 also the docking bay; the NIS deletes the extra
//!   modules it made when it stops.
//! * Asteroid paths become asteroids. Other kinds (dust and gas clouds, derelict-table objects,
//!   effects, generic objects, missiles) are not created yet; their paths still move but have
//!   no object (none of the shipped NIS use them except the asteroid fields).
//! * Every step the NIS pose goes onto the object (`univSetShipPosition`, the clamped modules
//!   follow), with the step's displacement as velocity: capped at the type's top speed for the
//!   first seven ship types (the original tests the type against `NUM_RACES`), and the trails
//!   cut when the object jumped.

use hwc_math::fpu::{ld, st};
use hwc_math::{Matrix, Vector};
use hwc_sim::base::{self, CreateOptions};
use hwc_sim::object::{sof, ShipId};
use hwc_sim::resource::{self, ResourceId, ResourceKind};
use hwc_sim::statics::{ShipRace, ShipType};
use hwc_sim::Universe;

use crate::data::Category;
use crate::events::Action;
use crate::player::{omf, Fired, NisPlayer, StopReport};

/// `REALlyBig`, the health of an invincible NIS object.
pub const REALLY_BIG: f32 = 1e21;

/// The universe object an NIS path drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Ship(ShipId),
    Resource(ResourceId),
    None,
}

/// The NIS objects in a universe.
#[derive(Debug, Clone, Default)]
pub struct Stage {
    pub handles: Vec<Handle>,
    /// Modules the NIS created for its mothership (deleted when it stops).
    pub extra_ships: Vec<ShipId>,
    /// What `hideUniverseToggle` hid, and what was already hidden.
    hidden: Vec<ShipId>,
    hidden_resources: Vec<ResourceId>,
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
    /// starting poses). `mission` decides the mothership's modules.
    pub fn create(u: &mut Universe, player: &NisPlayer, mission: u32) -> Stage {
        let mut stage = Stage::default();
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
                            if t == ShipType::sMothership && mission == 1 {
                                // The docking bay the opening NIS adds to the hull.
                                let pos = state.position;
                                if let Ok(bay) = base::create_ship(u, ShipType::sMothershipDockingBay, 0, pos, CreateOptions::default()) {
                                    let b = u.ships.ship_mut(bay);
                                    b.flags |= sof::DONT_APPLY_PHYSICS | sof::SELECTABLE | sof::TARGETABLE;
                                    hwc_sim::clamp::clamp_obj_to_obj(u, bay, id);
                                    stage.extra_ships.push(bay);
                                }
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
                        }
                        Handle::Resource(id)
                    }
                    None => {
                        stage.log.push(format!("{}: no asteroid statics", path.name));
                        Handle::None
                    }
                },
                other => {
                    stage.log.push(format!("{}: {other:?} objects are not created", path.name));
                    Handle::None
                }
            };
            stage.handles.push(h);
        }
        // The flag the NIS sets on every object, and the starting poses.
        for &h in &stage.handles {
            if let Handle::Resource(id) = h {
                if let Some(r) = u.base.resources.get_mut(id) {
                    r.flags |= sof::NIS_SHIP;
                }
            }
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
                    let Some(s) = u.ships.get_mut(id) else { continue };
                    let t = s.ship_type;
                    s.posinfo.position = o.position;
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

    /// Objects that died or were deleted by the simulation stop being driven
    /// (`nisObjectDied`).
    pub fn forget_dead(&self, u: &Universe, player: &mut NisPlayer) {
        for (i, h) in self.handles.iter().enumerate() {
            let gone = match *h {
                Handle::Ship(id) => u.ships.get(id).is_none_or(|s| s.flags & sof::DEAD != 0),
                Handle::Resource(id) => u.base.resources.get(id).is_none(),
                Handle::None => false,
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
                    set_hidden(u, h, hide);
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
                    self.log.push(format!("{:?}: fire is not implemented", f.object));
                }
                _ => rest.push(f.clone()),
            }
        }
        rest
    }

    /// `univHideJustAboutEverything` / `univUnhideJustAboutEverything`: everything that is
    /// not an NIS object is hidden; what was hidden already stays hidden afterwards.
    pub fn toggle_universe(&mut self, u: &mut Universe) {
        if self.hidden.is_empty() && self.hidden_resources.is_empty() {
            let nis: Vec<ShipId> = self
                .handles
                .iter()
                .filter_map(|h| if let Handle::Ship(id) = h { Some(*id) } else { None })
                .chain(self.extra_ships.iter().copied())
                .collect();
            let ids: Vec<ShipId> = u.ship_list.clone();
            for id in ids {
                let modules_of_nis = u
                    .ships
                    .get(id)
                    .and_then(|s| s.base.clamp.as_ref())
                    .and_then(|c| c.host)
                    .is_some_and(|h| nis.contains(&h));
                let Some(s) = u.ships.get_mut(id) else { continue };
                if s.flags & sof::NIS_SHIP != 0 || nis.contains(&id) || modules_of_nis {
                    continue;
                }
                if s.flags & sof::HIDE == 0 {
                    s.flags |= sof::HIDE;
                    self.hidden.push(id);
                }
            }
            for r in &mut u.base.resources.list {
                if r.flags & sof::NIS_SHIP == 0 && r.flags & sof::HIDE == 0 {
                    r.flags |= sof::HIDE;
                    self.hidden_resources.push(r.id);
                }
            }
        } else {
            for id in self.hidden.drain(..) {
                if let Some(s) = u.ships.get_mut(id) {
                    s.flags &= !sof::HIDE;
                }
            }
            for id in self.hidden_resources.drain(..) {
                if let Some(r) = u.base.resources.get_mut(id) {
                    r.flags &= !sof::HIDE;
                }
            }
        }
    }

    /// `nisStop`'s object part: `remainAtEnd` objects become ordinary objects (stopped unless
    /// they keep moving, vincible, counted by the unit caps), the others and the extra
    /// modules leave the universe. A hidden universe is shown again.
    pub fn stop(&mut self, u: &mut Universe, report: &StopReport) {
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
                Handle::None => {}
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
                Handle::Resource(id) => {
                    u.base.resources.list.retain(|r| r.id != id);
                }
                Handle::None => {}
            }
        }
        if report.unhide_universe && !(self.hidden.is_empty() && self.hidden_resources.is_empty()) {
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
        Handle::None => {}
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
        s.posinfo.position = pos;
        s.rotinfo.coordsys = sys;
        s.posinfo.velocity = hv;
        follow_host(u, m);
    }
}
