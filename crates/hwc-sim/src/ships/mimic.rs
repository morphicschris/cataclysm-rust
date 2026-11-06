//! Mimic disguises (`Mimic` 0x5b2d70..0x5b4c40, `MCV` 0x5b2b30..0x5b2d50 and
//! `P1StandardCorvette` 0x5b8fc0..0x5b9280): sMimic, sMCV, p1StandardCorvette.
//!
//! A Mimic (and the MCV, the linked pair, and the Turanic standard corvette) can take on the
//! look of another ship type of some player, or of an asteroid. Disguising runs a small state
//! machine (`ship+0x760`): a random delay, a morph out of the current shape (+0x75c from 1 to
//! 0) and the disguise holds; changing disguise morphs back first and then into the pending
//! one (+0x730..+0x758); ending it morphs back. While disguised, the per-player visibility
//! flags ([`crate::visibility`]) hide the ship from the players it fools: the player whose
//! ship it imitates, or everybody for asteroids and derelicts (while its health is above
//! 40%). A disguised ship holds fire, and Mimics and MCVs attack by ramming (kamikaze).
//!
//! An MCV can only look like a fighter by splitting into its two Mimics: it morphs back,
//! orders the unlink and both Mimics take over the disguise (`MimicCopyDisguiseFrom`).
//!
//! A Mimic disguised as an asteroid that sits still drops a real asteroid record into the
//! resource list (`univAddMimicAsteroid`); harvesters may pick it. Mesh bindings, colour
//! schemes and the apparent LOD tables only matter for drawing; they are kept as plain values.

use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{ld, st};
use hwc_math::random::{RAN_AI_PLAYER, RAN_ETG};

use crate::command::{self, attr, order};
use crate::object::{flags2, special, CommandId, ShipId};
use crate::resource::{ResourceId, ResourceKind};
use crate::statics::{ShipRace, ShipStaticInfo, ShipType};
use crate::universe::{SpecialEvent, Universe};

use super::{Effect, ShipSpec};

/// Disguise states (`ship+0x760`).
pub mod state {
    /// Not disguised.
    pub const NONE: u8 = 0;
    /// Disguised.
    pub const DISGUISED: u8 = 1;
    /// Morphing into the disguise (+0x75c falls from 1 to 0).
    pub const MORPH_IN: u8 = 2;
    /// Morphing back into the own shape (+0x75c rises to 1).
    pub const MORPH_OUT: u8 = 3;
    /// Morphing back before taking on the pending disguise.
    pub const MORPH_SWAP: u8 = 4;
    /// Morphing back, then unlinking (MCV).
    pub const MORPH_OUT_UNLINK: u8 = 5;
    /// An MCV about to split so its Mimics can look like fighters.
    pub const SPLIT: u8 = 6;
    /// The same while still disguised: morph back first.
    pub const MORPH_OUT_SPLIT: u8 = 7;
    /// Disguise chosen, morph not started (after the split order, or copied from the MCV).
    pub const PENDING: u8 = 8;
    /// Waiting for the random delay before morphing in.
    pub const WAIT_MORPH_IN: u8 = 9;
    /// Waiting before morphing back.
    pub const WAIT_MORPH_OUT: u8 = 10;
    /// Waiting before morphing back to swap disguises.
    pub const WAIT_SWAP: u8 = 11;
}

/// Morph step per update (0x720614).
const MORPH_STEP: f32 = 0.05;
/// Selection radius factor of a disguise (0x86e00c).
const SELECT_RADIUS_FACTOR: f32 = 1.2;
/// Health fraction above which an asteroid or derelict disguise fools (0x86efb0).
const FOOL_HEALTH_FRACTION: f32 = 0.4;
/// The kamikaze damage factor with upgrade 0x80 (`ship+0x770`).
const KAMIKAZE_SCALE: f32 = 2.0;
/// Upgrade bits.
pub const UPGRADE_DISGUISE: u32 = 0x20;
pub const UPGRADE_SMALL_DISGUISE: u32 = 0x10;
pub const UPGRADE_KAMIKAZE_SCALE: u32 = 0x80;
/// `ship+0x1cc` bit that keeps the disguise when the ship docks.
const KEEP_DISGUISE_WHEN_DOCKED: u32 = 0x0800_0000;
/// Types a disguise into which counts as "a ship of the player" (`<= 0x3b`: Somtaaw and Beast).
const LAST_PLAYER_RACE_TYPE: i32 = 0x3b;
/// Disguise types above which the disguise is a derelict (`> 0x61`).
const LAST_PIRATE_TYPE: i32 = 0x61;

/// What a Mimic looks like (+0x704..+0x72c, the pending one at +0x730..+0x758).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Disguise {
    /// Ship type index (-1: an asteroid or nothing).
    pub ship_type: i32,
    /// Race of the imitated player (-1 none).
    pub race: i32,
    /// A mesh binding was made for the disguise (+0x710 / +0x73c).
    pub mesh: bool,
    /// Colour scheme shown (+0x714).
    pub colour: i32,
    /// Selection radius: the larger of the two collision spheres x 1.2 (+0x718).
    pub select_radius: f32,
    /// Imitated player (-1 none).
    pub player: i32,
    /// Asteroid type for asteroid disguises (-1 none).
    pub asteroid: i32,
    /// Apparent size of the asteroid (+0x724).
    pub asteroid_radius: f32,
    /// Apparent minimum zoom distance (+0x728).
    pub asteroid_zoom: f32,
    /// Apparent resource units x 10 (+0x72c).
    pub asteroid_value: i32,
}

impl Default for Disguise {
    fn default() -> Self {
        Self {
            ship_type: -1,
            race: -1,
            mesh: false,
            colour: 0,
            select_radius: 0.0,
            player: -1,
            asteroid: -1,
            asteroid_radius: 0.0,
            asteroid_zoom: 0.0,
            asteroid_value: 0,
        }
    }
}

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MimicSpec {
    pub state: u8,
    /// Morph progress (+0x75c): 1 own shape, 0 the disguise.
    pub morph: f32,
    /// When the delayed morph starts (+0x764).
    pub morph_start: f32,
    /// Set when the morph into a disguise began (+0x768).
    pub morphed: bool,
    pub current: Disguise,
    pub pending: Disguise,
    /// The asteroid record of a still asteroid disguise (+0x76c).
    pub asteroid: Option<ResourceId>,
    /// Kamikaze damage factor with the upgrade (+0x770).
    pub kamikaze_scale: f32,
    /// Turanic standard corvette: what its special targets are for (+0x79c: 0 nothing, 1
    /// disguise, 2 salvage; set by KAS).
    pub p1_mode: u8,
}

impl Default for MimicSpec {
    fn default() -> Self {
        Self {
            state: state::NONE,
            morph: 0.0,
            morph_start: 0.0,
            morphed: false,
            current: Disguise::default(),
            pending: Disguise::default(),
            asteroid: None,
            kamikaze_scale: KAMIKAZE_SCALE,
            p1_mode: 0,
        }
    }
}

/// Per-type statics (`CustShipStaticInit`). The Mimic takes the interceptor statics, the MCV
/// and the standard corvette the side-step statics (`repositionTime`...), both kept in
/// [`crate::combat_statics::TypeCombat`]; the static flags +0x211..+0x213 they set are not
/// used by the simulation.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct MimicStatics {}

impl MimicStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

/// The disguise order (`FUN_00449520` / `clProcess` case 0xf), command +0x2e8..+0x2f4.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DisguiseOrder {
    /// 0 end the disguise, 1 a ship type, 2 an asteroid.
    pub kind: i32,
    pub ship_type: i32,
    pub player: i32,
    /// Asteroid type (0 random).
    pub asteroid: i32,
}

fn spec(u: &Universe, id: ShipId) -> Option<&MimicSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Mimic(m) => Some(m),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut MimicSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Mimic(m) => Some(m),
        _ => None,
    }
}

fn has_disguise(u: &Universe, id: ShipId) -> bool {
    u.ships
        .get(id)
        .is_some_and(|s| s.flags2 & flags2::DISGUISED != 0)
        && spec(u, id).is_some()
}

fn has_upgrade(u: &Universe, id: ShipId, bit: u32) -> bool {
    u.ships
        .get(id)
        .is_some_and(|s| crate::upgrade::has_upgrade(s, bit))
}

fn set_all_visible(u: &mut Universe, id: ShipId, v: bool) {
    if let Some(s) = u.ships.get_mut(id) {
        s.visible_to = [v; 8];
    }
}

fn effect(u: &mut Universe, id: ShipId, st_: u8) {
    u.special_events.push(SpecialEvent::Special {
        ship: id,
        effect: Effect::State {
            state: u32::from(st_),
        },
    });
}

/// `MimicInit` (0x5b2da0) / `MCVInit` (0x5b2b70) / `P1StandardCorvetteInit` (0x5b9010): no
/// disguise, the disguise flag on (+0x18 & 8).
pub fn init(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    if s.ship_type == ShipType::sMimic {
        s.interceptor.active_gun = 0;
    }
    if matches!(s.ship_type, ShipType::sMCV | ShipType::p1StandardCorvette) {
        s.side_step = Default::default();
    }
    s.flags2 |= flags2::DISGUISED;
    s.spec = ShipSpec::Mimic(MimicSpec::default());
}

fn state_of(u: &Universe, id: ShipId) -> u8 {
    spec(u, id).map_or(state::NONE, |m| m.state)
}

/// Disguised and in a sphere formation: the ship joins the ram (the formation's attack).
fn sphere_ram(u: &mut Universe, id: ShipId) -> bool {
    if state_of(u, id) != state::DISGUISED {
        return false;
    }
    let s = u.ships.ship(id);
    let sphere = s.command.and_then(|c| u.commands.get(c)).is_some_and(|c| {
        c.attributes & attr::FORMATION != 0
            && c.formation.formation_type == crate::formation::kind::SPHERE
    });
    if sphere {
        u.ships.ship_mut(id).special_flags |= special::KAMIKAZE | special::BROKEN_FORMATION;
    }
    sphere
}

/// `MimicAttackPassive` (0x5b2ee0).
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if sphere_ram(u, id) {
        return;
    }
    if state_of(u, id) == state::NONE {
        crate::attack::generic_interceptor_passive(u, id, target, rotate);
    }
}

/// `MCVAttackPassive` (0x5b2c50), also the standard corvette's.
pub fn mcv_attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if sphere_ram(u, id) {
        return;
    }
    if state_of(u, id) == state::NONE {
        if rotate {
            crate::attack::attack_passive_rotate(u, id, target);
        } else {
            crate::attack::attack_simple(u, id, target);
        }
    }
}

/// `MimicAttack` (0x5b2eb0): the interceptor attack, ramming.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    crate::attack::generic_interceptor_attack(u, id, target);
    u.ships.ship_mut(id).special_flags |= special::KAMIKAZE | special::BROKEN_FORMATION;
}

/// The range `CustShipAttack` is called with (`bulletRange[tactics]`).
fn attack_range(u: &Universe, id: ShipId) -> f32 {
    let s = u.ships.ship(id);
    u.statics
        .ship(s.ship_type)
        .map_or(0.0, |st_| st_.bullet_range[s.tactics.min(2)])
}

/// Minimum range of the MCV's and the standard corvette's `attackPassive` (1000).
const MCV_MIN_RANGE: f32 = 1000.0;

/// `MCVAttack` (0x5b2c10): `attackPassive`, ramming.
pub fn mcv_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let r = attack_range(u, id);
    crate::attack::attack_passive(u, id, target, r, MCV_MIN_RANGE);
    u.ships.ship_mut(id).special_flags |= special::KAMIKAZE | special::BROKEN_FORMATION;
}

/// `P1StandardCorvetteAttack` (0x5b9090): disguised, close in with `attackPassive` and ram when
/// facing the target; otherwise the side-step attack.
pub fn p1_attack(u: &mut Universe, id: ShipId, target: ShipId) {
    if state_of(u, id) == state::DISGUISED {
        let r = attack_range(u, id);
        crate::attack::attack_passive(u, id, target, r, MCV_MIN_RANGE);
        if u.ships.get(target).is_none() {
            return;
        }
        let s = u.ships.ship(id);
        let h = s.heading();
        let t = crate::aiship::trajectory(s, u.ships.ship(target));
        if 0.5 < (ld(t.x) * ld(h.x) + ld(t.y) * ld(h.y)) + ld(t.z) * ld(h.z) {
            u.ships.ship_mut(id).special_flags |= special::KAMIKAZE | special::BROKEN_FORMATION;
        }
    } else {
        crate::attack::side_step_attack(u, id, target);
    }
}

/// `MimicSpecialActivate` (0x5b3260), also `P1StandardCorvetteSpecialActivate` (the MCV's):
/// end the disguise after a delay.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    end_disguise(u, id, true);
    true
}

/// The MCV's and the standard corvette's special activate (`P1StandardCorvetteSpecialActivate`
/// 0x5b9150).
pub fn mcv_special_activate(u: &mut Universe, id: ShipId) -> bool {
    special_activate(u, id)
}

/// `MimicSpecialTarget` (0x5b3810): disguise as the first target (a ship of its type and
/// player). Asteroid targets (by their type) are not possible here: special targets are ships.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let Some(&t) = targets.first() else {
        return true;
    };
    if u.ships
        .get(id)
        .is_some_and(|s| s.base.link.partner.is_some())
    {
        return true;
    }
    if !has_upgrade(u, id, UPGRADE_DISGUISE) {
        return true;
    }
    let Some(ts) = u.ships.get(t) else {
        return true;
    };
    let (ty, pl) = (ts.ship_type.index() as i32, ts.player as i32);
    disguise_as_ship_type(u, id, ty, pl);
    true
}

/// `MCVSpecialTarget` (0x5b2ce0).
pub fn mcv_special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    special_target(u, id, targets);
    true
}

/// `P1StandardCorvetteSpecialTarget` (0x5b9170): disguise or salvage, as KAS set it.
pub fn p1_special_target(u: &mut Universe, id: ShipId, cid: CommandId, targets: &[ShipId]) -> bool {
    let mode = spec(u, id).map_or(0, |m| m.p1_mode);
    let done = match mode {
        1 => special_target(u, id, targets),
        2 => crate::salvage::special_target_pub(u, id, cid, targets),
        _ => true,
    };
    if done {
        if let Some(m) = spec_mut(u, id) {
            m.p1_mode = 0;
        }
    }
    done
}

/// `P1StandardCorvetteOrderChanged` (0x5b91e0): the salvage job ends.
pub fn p1_order_changed(u: &mut Universe, id: ShipId) {
    crate::salvage::special_done(u, id);
    if let Some(m) = spec_mut(u, id) {
        m.p1_mode = 0;
    }
}

/// The standard corvette's special mode (`kasfSalvage` sets 2).
pub fn set_p1_mode(u: &mut Universe, id: ShipId, mode: u8) {
    if let Some(m) = spec_mut(u, id) {
        m.p1_mode = mode;
    }
}

/// `MimicChooseColourScheme` (0x5b3280): in single-player games by the disguise's race; in
/// multiplayer the imitated player's own scheme (the original checks the type's per-scheme
/// texture availability, which is not loaded; every scheme counts as available).
fn choose_colour(u: &Universe, stat: &ShipStaticInfo, player: i32) -> i32 {
    if !u.single_player {
        return player;
    }
    match stat.ship_race {
        ShipRace::Somtaaw => 0,
        ShipRace::Beast => 1,
        ShipRace::Kushan => 4,
        ShipRace::Taiidan => 2,
        ShipRace::Turanic | ShipRace::Traders | ShipRace::Derelict => 5,
    }
}

/// `MimicStartMorph` (0x5b3140).
pub fn start_morph(u: &mut Universe, id: ShipId) {
    if let Some(s) = u.ships.get_mut(id) {
        s.base.link.partner = None;
    }
    match state_of(u, id) {
        state::NONE | state::PENDING => {
            let t = crate::cloak::fade_start_time(u);
            let m = spec_mut(u, id).expect("mimic");
            m.morph = 1.0;
            m.state = state::WAIT_MORPH_IN;
            m.morph_start = t;
        }
        state::DISGUISED => {
            spec_mut(u, id).expect("mimic").morph = 0.0;
            let t = crate::cloak::fade_start_time(u);
            let m = spec_mut(u, id).expect("mimic");
            m.morph_start = t;
            m.state = state::WAIT_SWAP;
        }
        state::WAIT_MORPH_OUT => spec_mut(u, id).expect("mimic").state = state::WAIT_SWAP,
        state::MORPH_IN | state::MORPH_OUT => {
            spec_mut(u, id).expect("mimic").state = state::MORPH_SWAP
        }
        _ => {}
    }
}

/// Free the disguise's mesh bindings (the `memFree` pairs) and the asteroid record.
fn free_disguise_data(u: &mut Universe, id: ShipId, both: bool) {
    let Some(m) = spec_mut(u, id) else { return };
    m.current.mesh = false;
    if both {
        m.pending.mesh = false;
    }
    delete_asteroid(u, id);
}

/// `DeleteDeadResource` on the disguise's asteroid record.
fn delete_asteroid(u: &mut Universe, id: ShipId) {
    let Some(r) = spec_mut(u, id).and_then(|m| m.asteroid.take()) else {
        return;
    };
    u.base.resources.list.retain(|x| x.id != r);
    let todo = u.commands.todo.clone();
    for c in todo {
        if let Some(cmd) = u.commands.get_mut(c) {
            if cmd.collect.resource == Some(r) {
                cmd.collect.resource = None;
            }
        }
    }
}

/// `MimicEndDisguise` (0x5b2f30): morph back (after a delay when `delayed`); every player sees
/// the ship again.
pub fn end_disguise(u: &mut Universe, id: ShipId, delayed: bool) {
    if spec(u, id).is_none() {
        return;
    }
    match state_of(u, id) {
        state::DISGUISED => {
            spec_mut(u, id).expect("mimic").morph = 0.0;
            if delayed {
                let t = crate::cloak::fade_start_time(u);
                let m = spec_mut(u, id).expect("mimic");
                m.morph_start = t;
                m.state = state::WAIT_MORPH_OUT;
            } else {
                spec_mut(u, id).expect("mimic").state = state::MORPH_OUT;
            }
        }
        state::WAIT_SWAP => spec_mut(u, id).expect("mimic").state = state::WAIT_MORPH_OUT,
        state::MORPH_IN | state::MORPH_SWAP | state::MORPH_OUT_UNLINK => {
            spec_mut(u, id).expect("mimic").state = state::MORPH_OUT
        }
        state::PENDING | state::WAIT_MORPH_IN => {
            spec_mut(u, id).expect("mimic").state = state::NONE;
            free_disguise_data(u, id, true);
        }
        _ => {}
    }
    set_all_visible(u, id, true);
}

/// The MCV's end of disguise (`MCVEndDisguise` 0x5b2cc0, from `dockUnlinkShipBegin`).
pub fn mcv_end_disguise(u: &mut Universe, id: ShipId, delayed: bool) {
    end_disguise(u, id, delayed);
}

/// `MimicFreeDisguise` (0x5b38e0): the ship docked inside another; the disguise is gone at once.
pub fn free_disguise(u: &mut Universe, id: ShipId) {
    if !has_disguise(u, id) || u.ships.ship(id).special_flags2 & KEEP_DISGUISE_WHEN_DOCKED != 0 {
        return;
    }
    if let Some(m) = spec_mut(u, id) {
        m.state = state::NONE;
        m.morphed = false;
    }
    free_disguise_data(u, id, true);
    set_all_visible(u, id, true);
}

/// `MimicCancelDisguise` (0x5b4420, the MCV's and corvette's slot 27): a pending disguise is
/// dropped.
pub fn cancel_disguise(u: &mut Universe, id: ShipId) {
    if state_of(u, id) == state::PENDING {
        spec_mut(u, id).expect("mimic").state = state::NONE;
        free_disguise_data(u, id, true);
    }
}

/// `MimicCopyDisguiseFrom` (0x5b3060): a Mimic of a splitting MCV takes over its pending
/// disguise (state 8, waiting for the morph).
pub fn copy_disguise_from(u: &mut Universe, id: ShipId, mcv: ShipId) {
    let Some(src) = spec(u, mcv).map(|m| m.pending) else {
        return;
    };
    let Some(m) = spec_mut(u, id) else { return };
    let keep_mesh = m.current.mesh;
    m.current = Disguise {
        mesh: keep_mesh,
        ..src
    };
    m.morph = 1.0;
    if src.ship_type != -1 && src.mesh {
        m.current.mesh = true;
    } else {
        m.pending.mesh = false;
    }
    m.state = state::PENDING;
}

/// Whether the type may take on the look of `t` (`MimicDisguiseAsShipType`'s class checks).
/// Returns `None` when it may not, else whether an MCV must split first.
fn may_disguise_as(u: &Universe, id: ShipId, t: ShipType, stat: &ShipStaticInfo) -> Option<bool> {
    let own = u.ships.ship(id).ship_type;
    if own == ShipType::p1StandardCorvette {
        return Some(false);
    }
    let ti = t.index() as i32;
    match stat.ship_class.map_or(-1, |c| c.index() as i32) {
        0..=3 | 9 | 10 => None,
        4 if own == ShipType::sMimic => None,
        6 | 11 if own == ShipType::sMCV => {
            has_upgrade(u, id, UPGRADE_SMALL_DISGUISE).then_some(true)
        }
        7 if own == ShipType::sMimic || ti == 0x10 || ti == 0x33 => None,
        8 if own == ShipType::sMimic && ti != 0x73 => None,
        _ => Some(false),
    }
}

/// `MimicDisguiseAsShipType` (0x5b3330): look like a ship of type `ship_type` of `player`.
pub fn disguise_as_ship_type(u: &mut Universe, id: ShipId, ship_type: i32, player: i32) {
    let Some(t) = usize::try_from(ship_type)
        .ok()
        .and_then(ShipType::from_index)
    else {
        return;
    };
    let statics = u.statics.clone();
    let Some(stat) = statics.ship(t) else { return };
    let Some(split) = may_disguise_as(u, id, t, stat) else {
        return;
    };
    if split {
        let m = spec_mut(u, id).expect("mimic");
        m.state = u8::from(m.state != 0) + state::SPLIT;
    }
    let own = u.ships.ship(id).ship_type;
    let own_size = statics
        .ship(own)
        .map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let other = stat.header.coll.coll_sphere_size;
    let size = if other <= own_size { own_size } else { other };
    let race = usize::try_from(player)
        .ok()
        .and_then(|p| u.players.get(p))
        .map_or(-1, |p| p.race.index() as i32);
    let colour = choose_colour(u, stat, player);
    let set = |d: &mut Disguise| {
        // (The asteroid size, zoom and value of an earlier disguise stay.)
        d.ship_type = ship_type;
        d.race = race;
        d.mesh = true;
        d.colour = colour;
        d.select_radius = st(ld(size) * ld(SELECT_RADIUS_FACTOR));
        d.player = player;
        d.asteroid = -1;
    };
    let m = spec_mut(u, id).expect("mimic");
    if m.state == state::NONE {
        set(&mut m.current);
    } else {
        set(&mut m.pending);
    }
    start_morph(u, id);
}

/// `MimicDisguiseAsResource` (0x5b3540): look like an asteroid of type 1..3 (0: a random one;
/// Mimics cannot be the biggest, MCVs need the upgrade for the smallest).
pub fn disguise_as_resource(u: &mut Universe, id: ShipId, kind: i32) {
    let own = u.ships.ship(id).ship_type;
    let mut k = kind;
    if k == 0 {
        let r = u.randy.random(RAN_ETG) & 1;
        k = if own == ShipType::sMimic {
            if r == 0 {
                1
            } else {
                2
            }
        } else if r != 0 {
            3
        } else {
            2
        };
    }
    let zoom = match k {
        1 => {
            if own == ShipType::sMCV {
                if !has_upgrade(u, id, UPGRADE_SMALL_DISGUISE) {
                    return;
                }
                let m = spec_mut(u, id).expect("mimic");
                m.state = u8::from(m.state != 0) + state::SPLIT;
            }
            130.0
        }
        2 => 500.0,
        3 => {
            if own == ShipType::sMimic {
                return;
            }
            800.0
        }
        _ => return,
    };
    let statics = u.statics.clone();
    let (radius, value) = match statics.asteroids.get(k as usize).and_then(Option::as_ref) {
        Some(a) => {
            let c = &a.info.header.coll;
            let half = |x: f32| ld(x) * 0.5;
            let sum = half(c.up_length) * half(c.up_length)
                + half(c.right_length) * half(c.right_length)
                + half(c.forward_length) * half(c.forward_length);
            (st(fmath_sqrt_double(sum) * 1.1), a.info.resource_value)
        }
        None => (0.0, 0),
    };
    let own_size = statics
        .ship(own)
        .map_or(0.0, |s| s.header.coll.coll_sphere_size);
    let size = if radius <= own_size { own_size } else { radius };
    let d = Disguise {
        ship_type: -1,
        race: -1,
        mesh: false,
        colour: 0,
        select_radius: st(ld(size) * ld(SELECT_RADIUS_FACTOR)),
        player: -1,
        asteroid: k,
        asteroid_radius: radius,
        asteroid_zoom: zoom,
        asteroid_value: value * 10,
    };
    let m = spec_mut(u, id).expect("mimic");
    if m.state == state::NONE {
        m.current = d;
    } else {
        m.pending = d;
    }
    start_morph(u, id);
}

/// `MimicDoDisguiseCommand` (0x5b37b0): needs the disguise upgrade and no link partner.
pub fn do_disguise_command(u: &mut Universe, id: ShipId, o: DisguiseOrder) {
    if !has_upgrade(u, id, UPGRADE_DISGUISE) || u.ships.ship(id).base.link.partner.is_some() {
        return;
    }
    match o.kind {
        0 => end_disguise(u, id, true),
        1 => disguise_as_ship_type(u, id, o.ship_type, o.player),
        2 => disguise_as_resource(u, id, o.asteroid),
        _ => {}
    }
}

/// `clProcess` for the disguise order: every ship of the command carries it out once.
pub fn process_disguise_to_do(u: &mut Universe, cid: CommandId) {
    let Some(c) = u.commands.get(cid).cloned() else {
        return;
    };
    let Some(o) = c.disguise else { return };
    for &s in &c.selection {
        if spec(u, s).is_some() {
            do_disguise_command(u, s, o);
        }
    }
}

/// The disguise order (`FUN_0044f3a0` -> `FUN_00449520`): the ships stop what they are doing
/// and get a disguise command, carried out by the next `clProcess`.
pub fn cl_disguise(u: &mut Universe, ships: &[ShipId], o: DisguiseOrder) {
    let sel: Vec<ShipId> = ships
        .iter()
        .copied()
        .filter(|&s| u.ships.get(s).is_some())
        .collect();
    if sel.is_empty() {
        return;
    }
    command::remove_ships_from_doing_stuff(u, &sel);
    let mut c = command::new_command_pub(sel.clone());
    c.order = order::DISGUISE;
    c.disguise = Some(o);
    let cid = u.commands.add_pub(c);
    for &s in &sel {
        let sh = u.ships.ship_mut(s);
        sh.command = Some(cid);
        if sh.special_flags & special::ROW_GETTING_OUT_OF_WAY != 0 {
            crate::aiship::row_remove_ship(sh);
        }
    }
}

/// The disguise order for a ship type of `player` (as the computer players give it).
pub fn cl_disguise_as(u: &mut Universe, ships: &[ShipId], disguise: ShipType, player: usize) {
    cl_disguise(
        u,
        ships,
        DisguiseOrder {
            kind: 1,
            ship_type: disguise.index() as i32,
            player: player as i32,
            asteroid: -1,
        },
    );
}

/// `MimicHouseKeep` (0x5b3970), also the MCV's and the standard corvette's: the morph state
/// machine.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(m) = spec(u, id).cloned() else {
        return;
    };
    let now = u.total_time_elapsed;
    match m.state {
        state::DISGUISED => {
            if m.current.ship_type == -1 {
                let s = u.ships.ship(id);
                if s.ship_idle && s.posinfo.is_moving == 0 {
                    if m.asteroid.is_none() {
                        add_asteroid(u, id, m.current.asteroid);
                    }
                } else {
                    delete_asteroid(u, id);
                }
            }
        }
        state::MORPH_IN => {
            if 1.0 <= m.morph {
                effect(u, id, state::MORPH_IN);
                crate::speech::by_ship(u, id, crate::speech::event::DISGUISE);
            }
            let f = ld(m.morph) - ld(MORPH_STEP);
            let mm = spec_mut(u, id).expect("mimic");
            mm.morph = st(f);
            if f <= 0.0 {
                mm.state = state::DISGUISED;
                let d = mm.current;
                if d.player < 0 || d.ship_type < 0 || LAST_PLAYER_RACE_TYPE < d.ship_type {
                    set_all_visible(u, id, false);
                } else if let Some(v) = u.ships.ship_mut(id).visible_to.get_mut(d.player as usize) {
                    *v = false;
                }
            }
        }
        state::MORPH_OUT => {
            if m.morph <= 0.0 {
                effect(u, id, state::MORPH_OUT);
                crate::speech::by_ship(u, id, crate::speech::event::UNDISGUISE);
            }
            let f = ld(m.morph) + ld(MORPH_STEP);
            spec_mut(u, id).expect("mimic").morph = st(f);
            if 1.0 <= f {
                let mm = spec_mut(u, id).expect("mimic");
                mm.state = state::NONE;
                mm.morphed = false;
                free_disguise_data(u, id, true);
            }
        }
        state::MORPH_SWAP => {
            if m.morph <= 0.0 {
                effect(u, id, state::MORPH_SWAP);
            }
            let f = ld(m.morph) + ld(MORPH_STEP);
            let mm = spec_mut(u, id).expect("mimic");
            mm.morph = st(f);
            if 1.0 <= f {
                let p = mm.pending;
                mm.current = p;
                mm.pending.mesh = false;
                mm.state = state::MORPH_IN;
            }
        }
        state::MORPH_OUT_UNLINK => {
            let f = ld(m.morph) + ld(MORPH_STEP);
            spec_mut(u, id).expect("mimic").morph = st(f);
            if 1.0 <= f {
                let mm = spec_mut(u, id).expect("mimic");
                mm.state = state::NONE;
                mm.morphed = false;
                free_disguise_data(u, id, true);
                crate::link::cl_unlink(u, id);
            }
        }
        state::SPLIT => split(u, id),
        state::MORPH_OUT_SPLIT => {
            if m.morph <= 0.0 {
                effect(u, id, state::MORPH_OUT_SPLIT);
                crate::speech::by_ship(u, id, crate::speech::event::UNDISGUISE);
            }
            let f = ld(m.morph) + ld(MORPH_STEP);
            spec_mut(u, id).expect("mimic").morph = st(f);
            if 1.0 <= f {
                let mm = spec_mut(u, id).expect("mimic");
                mm.morphed = false;
                free_disguise_data(u, id, false);
                split(u, id);
            }
        }
        state::WAIT_MORPH_IN => {
            if m.morph_start < now {
                let mm = spec_mut(u, id).expect("mimic");
                mm.state = state::MORPH_IN;
                mm.morphed = true;
            }
        }
        state::WAIT_MORPH_OUT => {
            if m.morph_start < now {
                spec_mut(u, id).expect("mimic").state = state::MORPH_OUT;
            }
        }
        state::WAIT_SWAP if m.morph_start < now => {
            spec_mut(u, id).expect("mimic").state = state::MORPH_SWAP;
        }
        _ => {}
    }
    if state_of(u, id) != state::DISGUISED {
        delete_asteroid(u, id);
    }
}

/// An MCV splits so its two Mimics can take on a small disguise: the unlink order, and the
/// disguise waits to be copied.
fn split(u: &mut Universe, id: ShipId) {
    crate::link::cl_unlink(u, id);
    if let Some(m) = spec_mut(u, id) {
        m.state = state::PENDING;
    }
}

/// `univAddMimicAsteroid` (0x53bce0): an asteroid record of the disguise's type at the ship
/// (resource list only, randomly oriented).
fn add_asteroid(u: &mut Universe, id: ShipId, kind: i32) {
    let Ok(k) = u8::try_from(kind) else { return };
    let p = u.ships.ship(id).coll.coll_position;
    if let Some(r) = crate::resource::add_resource(u, ResourceKind::Asteroid(k), p) {
        spec_mut(u, id).expect("mimic").asteroid = Some(r);
    }
}

/// `MimicDied` (0x5b3ea0) / slot 11 for `id == gone` (`MimicRemoveShipReferences` 0x5b4650):
/// the disguise data goes.
pub fn died(u: &mut Universe, id: ShipId) {
    free_disguise_data(u, id, true);
    if u.ships
        .get(id)
        .is_some_and(|s| s.ship_type == ShipType::p1StandardCorvette)
    {
        crate::salvage::special_done(u, id);
    }
}

/// `MimicRemoveShipReferences` (0x5b4650).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if id == gone {
        free_disguise_data(u, id, true);
    }
}

/// `MimicIsDisguised` (0x5b43f0).
pub fn is_disguised(u: &Universe, id: ShipId) -> bool {
    has_disguise(u, id) && state_of(u, id) != state::NONE
}

/// `MimicIsDisguisedAsPlayer` (0x5b4130): the ship looks like one of `player`'s and is hidden
/// from that player.
pub fn is_disguised_as_player(u: &Universe, id: ShipId, player: usize) -> bool {
    if !has_disguise(u, id) {
        return false;
    }
    let m = spec(u, id).expect("mimic");
    m.state == state::DISGUISED
        && m.current.ship_type != -1
        && !crate::visibility::visible_to_player(u, id, player)
        && m.current.player == player as i32
}

fn healthy_enough(u: &Universe, id: ShipId) -> bool {
    let s = u.ships.ship(id);
    let max = u
        .statics
        .ship(s.ship_type)
        .map_or(1.0, |x| x.health.max_health);
    ld(FOOL_HEALTH_FRACTION) < ld(s.health) / ld(max)
}

/// `MimicFoolsPlayerAsResource` (0x5b42a0).
pub fn fools_player_as_resource(u: &Universe, id: ShipId, player: usize) -> bool {
    if !has_disguise(u, id) || crate::visibility::visible_to_player(u, id, player) {
        return false;
    }
    let m = spec(u, id).expect("mimic");
    m.state == state::DISGUISED && m.current.ship_type == -1 && healthy_enough(u, id)
}

/// `MimicFoolsPlayerAsDerelict` (0x5b4310).
pub fn fools_player_as_derelict(u: &Universe, id: ShipId, player: usize) -> bool {
    if !has_disguise(u, id) || crate::visibility::visible_to_player(u, id, player) {
        return false;
    }
    let m = spec(u, id).expect("mimic");
    m.state == state::DISGUISED && LAST_PIRATE_TYPE < m.current.ship_type && healthy_enough(u, id)
}

/// `MimicGetApparentShipType` (0x5b4220): the disguise type of a Mimic (`None` when it has none
/// set), else the ship's own type.
pub fn apparent_ship_type(u: &Universe, id: ShipId) -> Option<ShipType> {
    let s = u.ships.get(id)?;
    if has_disguise(u, id) {
        let t = spec(u, id)?.current.ship_type;
        return usize::try_from(t).ok().and_then(ShipType::from_index);
    }
    Some(s.ship_type)
}

/// `MimicGetApparentPlayerIndex` (0x5b41a0): single player, the colour scheme shown;
/// multiplayer, the imitated player.
pub fn apparent_player(u: &Universe, id: ShipId) -> i32 {
    let Some(s) = u.ships.get(id) else { return -1 };
    if !has_disguise(u, id) {
        return s.player as i32;
    }
    let m = spec(u, id).expect("mimic");
    let shown = m.state == state::DISGUISED && m.current.ship_type != -1;
    if u.single_player {
        if shown {
            m.current.colour
        } else {
            s.color_scheme as i32
        }
    } else if shown {
        m.current.player
    } else {
        s.player as i32
    }
}

/// The states in which the disguise's shape is shown (`MimicGetApparentStatic*`).
fn shows_disguise(st_: u8) -> bool {
    matches!(st_, 1 | 3 | 4 | 5 | 10 | 11)
}

/// `MimicGetApparentStatic4C` / `MimicGetApparentStatic38` / `MimicGetApparentStatic3B4`: the
/// collision sphere, avoidance radius and minimum zoom distance the ship appears to have.
pub fn apparent_sizes(u: &Universe, id: ShipId) -> Option<(f32, f32, f32)> {
    let s = u.ships.get(id)?;
    let own = u.statics.ship(s.ship_type)?;
    let own_sizes = (
        own.header.coll.coll_sphere_size,
        own.header.coll.avoid_coll_sphere_size,
        own.minimum_zoom_distance,
    );
    if !has_disguise(u, id) {
        return Some(own_sizes);
    }
    let m = spec(u, id)?;
    if !shows_disguise(m.state) {
        return Some(own_sizes);
    }
    match usize::try_from(m.current.ship_type)
        .ok()
        .and_then(ShipType::from_index)
        .and_then(|t| u.statics.ship(t))
    {
        Some(d) => Some((
            d.header.coll.coll_sphere_size,
            d.header.coll.avoid_coll_sphere_size,
            d.minimum_zoom_distance,
        )),
        None => Some((
            m.current.asteroid_radius,
            m.current.asteroid_radius,
            m.current.asteroid_zoom,
        )),
    }
}

/// `MimicGetDisguiseResource` (0x5b43a0): the asteroid record of an asteroid disguise.
pub fn disguise_resource(u: &Universe, id: ShipId) -> Option<ResourceId> {
    if !has_disguise(u, id) {
        return None;
    }
    spec(u, id)?.asteroid
}

/// `MimicGetDisguiseSpeedScale` (0x5b43c0): the kamikaze damage factor (2 with upgrade 0x80).
pub fn kamikaze_damage_scale(u: &Universe, id: ShipId) -> f32 {
    if has_upgrade(u, id, UPGRADE_KAMIKAZE_SCALE) {
        spec(u, id).map_or(1.0, |m| m.kamikaze_scale)
    } else {
        1.0
    }
}

/// The morph progress for drawing (1 own shape .. 0 disguise) and the disguise type shown.
pub fn render_state(u: &Universe, id: ShipId) -> Option<(f32, Option<ShipType>)> {
    let m = spec(u, id)?;
    if m.state == state::NONE {
        return None;
    }
    Some((
        m.morph,
        usize::try_from(m.current.ship_type)
            .ok()
            .and_then(ShipType::from_index),
    ))
}

/// The ship type and colour scheme a Mimic is drawn with: the disguise's once the morph is past
/// half way (morph progress below 0.5), else `None` (its own). The morph itself is drawn as
/// that switch.
pub fn drawn_look(u: &Universe, id: ShipId) -> Option<(ShipType, usize)> {
    let m = spec(u, id)?;
    if m.state == state::NONE || 0.5 <= m.morph {
        return None;
    }
    let t = usize::try_from(m.current.ship_type).ok().and_then(ShipType::from_index)?;
    u.statics.ship(t)?;
    let scheme = usize::try_from(m.current.colour).unwrap_or(u.ships.ship(id).color_scheme);
    Some((t, scheme))
}

/// Disguise types the computer players pick from (0x8bab58..): per mimic type and race.
const MIMIC_SOMTAAW: [usize; 4] = [0, 1, 3, 17];
const MCV_SOMTAAW: [usize; 5] = [4, 14, 7, 6, 8];
const MIMIC_BEAST: [usize; 4] = [33, 52, 35, 37];
const MCV_BEAST: [usize; 6] = [38, 49, 41, 42, 48, 45];

/// `MimicPickRandomDisguiseType` (0x5b2e20): a random type from the table of the mimic type
/// and the player's race (stream 6). Other combinations return the race's number as a type,
/// as the original does.
pub fn pick_random_disguise_type(
    u: &mut Universe,
    mimic_type: ShipType,
    race: ShipRace,
) -> ShipType {
    let pick = |u: &mut Universe, table: &[usize], n: u32| {
        let r = u.randy.random(RAN_AI_PLAYER);
        ShipType::from_index(table[(if n == 4 { r & 3 } else { r % n }) as usize])
            .expect("disguise type")
    };
    match (mimic_type, race) {
        (ShipType::sMimic, ShipRace::Somtaaw) => pick(u, &MIMIC_SOMTAAW, 4),
        (ShipType::sMimic, ShipRace::Beast) => pick(u, &MIMIC_BEAST, 4),
        (ShipType::sMCV, ShipRace::Somtaaw) => pick(u, &MCV_SOMTAAW, 5),
        (ShipType::sMCV, ShipRace::Beast) => pick(u, &MCV_BEAST, 6),
        _ => ShipType::from_index(race.index()).unwrap_or(mimic_type),
    }
}

/// One entry of a player's disguise menu (8 bytes: a ship type, or for the resource list in
/// slot 8 an asteroid type in the second word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct DisguiseEntry {
    pub ship_type: i32,
    pub asteroid: i32,
}

/// The disguise menus (`MimicInitDisguiseTables` 0x5b46c0 and friends, 0x91b6b0): per player
/// (and slot 8, the asteroids) the types offered and the default one.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct DisguiseTables {
    pub entries: [Vec<DisguiseEntry>; 9],
    pub defaults: [i32; 9],
    /// `MimicGetDisguiseTablesFlag` / `MimicSetDisguiseTablesFlag`.
    pub flag: bool,
}

impl DisguiseTables {
    /// `MimicInitDisguiseTables`: Somtaaw and Beast players get their race's list; single-player
    /// games start empty (KAS fills them in with `MimicAddDisguiseType`).
    pub fn init(u: &Universe) -> Self {
        let mut t = Self {
            entries: Default::default(),
            defaults: [0; 9],
            flag: false,
        };
        if u.single_player {
            return t;
        }
        let e = |v: &[i32]| {
            v.iter()
                .map(|&x| DisguiseEntry {
                    ship_type: x,
                    asteroid: 0,
                })
                .collect::<Vec<_>>()
        };
        for (i, p) in u.players.iter().enumerate().take(8) {
            match p.race {
                ShipRace::Somtaaw => {
                    t.entries[i] = e(&[0, 1, 3, 0x11, 4, 0xe, 7, 6, 8]);
                    t.defaults[i] = 5;
                }
                ShipRace::Beast => {
                    t.entries[i] = e(&[0x21, 0x34, 0x23, 0x25, 0x26, 0x31, 0x29, 0x2a, 0x30, 0x2d]);
                    t.defaults[i] = 5;
                }
                _ => {}
            }
        }
        t.reset_asteroids();
        t
    }

    fn reset_asteroids(&mut self) {
        self.entries[8] = (0..4)
            .map(|k| DisguiseEntry {
                ship_type: 0,
                asteroid: k,
            })
            .collect();
        self.defaults[8] = 3;
    }

    /// `MimicAddDisguiseType` (0x5b4b30): append a type to a player's menu (slot 8: reset the
    /// asteroid list).
    pub fn add_type(&mut self, player: usize, ship_type: i32) {
        if player == 8 {
            self.reset_asteroids();
            return;
        }
        if let Some(l) = self.entries.get_mut(player) {
            l.push(DisguiseEntry {
                ship_type,
                asteroid: 0,
            });
        }
    }

    /// `MimicSetDisguiseDefault` (0x5b4bb0).
    pub fn set_default(&mut self, player: usize, v: i32) {
        if let Some(d) = self.defaults.get_mut(player) {
            *d = v;
        }
    }

    /// `MimicIsValidDisguisePlayer` (0x5b49a0): single player, the two players and the slots
    /// 8 and 9; multiplayer, the players in the game and slot 8.
    pub fn is_valid_player(u: &Universe, p: usize) -> bool {
        if u.single_player {
            matches!(p, 0 | 1 | 8 | 9)
        } else {
            p == 8 || p < u.players.len()
        }
    }
}

/// An MCV morphing or disguised other than waiting to split (`+0x760` neither 0 nor 8) holds on
/// before it splits.
pub fn is_mid_disguise(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.ship_type == ShipType::sMCV) && !matches!(state_of(u, id), 0 | state::PENDING)
}

/// A Mimic that has taken a split MCV's waiting disguise (`+0x760 == 8`).
pub fn is_pending_split_part(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.ship_type == ShipType::sMimic) && state_of(u, id) == state::PENDING
}

/// An MCV whose disguise waits for the split (`processUnlinkToDo` copies it to the Mimics).
pub fn is_pending_split(u: &Universe, id: ShipId) -> bool {
    u.ships
        .get(id)
        .is_some_and(|s| s.ship_type == ShipType::sMCV)
        && state_of(u, id) == state::PENDING
}
