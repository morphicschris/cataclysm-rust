//! Hive frigates and their swarmers (`HiveFrigate` 0x5afef0..0x5b11e0, `Swarmer`
//! 0x5c22b0..0x5c23f0): s/bHiveFrigate, s/bSwarmer.
//!
//! A hive frigate carries six swarmers, created inside it with the frigate and replaced one at
//! a time (every 15 s) when lost. Given targets (special target, or found by an aggressive hive
//! nearby), it flies within launch range, opens up, launches its swarmers one by one after a
//! short random delay, sends them at its current target, and keeps launching the ones that come
//! back. The special without a target recalls them: they dock back one by one, are refuelled
//! (repaired 1 HP per second) inside, and the hive closes.
//!
//! Hive states (`+0x63c`): 0 idle, 7 flying to the target, 1 preparing the launch, 2 launching,
//! 3 sending the swarmers to attack, 4 attacking, 5 preparing the recall, 6 recalling.
//!
//! Swarmer states (`+0x648`, as [`swarmer_state`] reads them): 0 docked, 1 launching, 2 out,
//! 3 docking, 4 attacking.
//!
//! Not reproduced: the open/close animations, speech and chatter (presentation).

use hwc_math::fpu::{fild_u32, ld, st};

use crate::base::{create_flags, CreateOptions};
use crate::object::{sof, ShipId};
use crate::ships::{Effect, ShipSpec, TypeStatics};
use crate::statics::{ShipRace, ShipStaticInfo, ShipType};
use crate::universe::{SpecialEvent, Universe};

/// Swarmers a hive carries.
pub const SWARMERS: usize = 6;
/// Most targets a hive keeps.
const MAX_TARGETS: usize = 1000;
/// `EnemyNearByCheckRate` / `EnemyNearByCheckFrame` (tweak.script 15 / 14, 0x8a22a4 /
/// 0x8a22a8): how often an aggressive idle hive looks for enemies.
const ENEMY_NEARBY_CHECK_RATE: u32 = 15;
const ENEMY_NEARBY_CHECK_FRAME: u32 = 14;
/// Range of the hive's own enemy search (7000, the same value as the launch range).
const ENEMY_SEARCH_RANGE: f32 = 7000.0;
/// `aishipFlyToPointAvoidingObjsFunc` flags and speed limit the hive flies to its target with.
const FLY_FLAGS: u32 = 7;
const FLY_LIMIT: f32 = -400.0;

/// Hive states (`+0x63c`).
pub mod state {
    pub const IDLE: u32 = 0;
    pub const PREPARE_LAUNCH: u32 = 1;
    pub const LAUNCHING: u32 = 2;
    pub const SEND: u32 = 3;
    pub const ATTACKING: u32 = 4;
    pub const PREPARE_RECALL: u32 = 5;
    pub const RECALLING: u32 = 6;
    pub const FLYING: u32 = 7;
}

/// Swarmer states (`+0x648`).
pub mod swarmer {
    pub const DOCKED: i32 = 0;
    pub const LAUNCHING: i32 = 1;
    pub const OUT: i32 = 2;
    pub const DOCKING: i32 = 3;
    pub const ATTACKING: i32 = 4;
}

/// Per-ship state of a hive frigate (`ShipSpecifics`, +0x638).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct HiveSpec {
    /// +0x638: when the next lost swarmer is replaced.
    pub regen_time: f32,
    /// +0x63c: see [`state`].
    pub state: u32,
    /// +0x640: the swarmers.
    pub swarmers: [Option<ShipId>; SWARMERS],
    /// +0x658: how many it has.
    pub count: i32,
    /// +0x65c / +0x15fc: the targets (cleared entries stay as `None`).
    pub targets: Vec<Option<ShipId>>,
    /// +0x1600: the current target.
    pub target: Option<ShipId>,
    /// +0x1604: 1 while flying to the target, 2 when a dock order waits for the recall.
    pub fly: u32,
    /// +0x1608: the ship to dock with afterwards.
    pub dock_with: Option<ShipId>,
    /// +0x160c: the hive is open.
    pub open: bool,
}

/// Per-swarmer state (`ShipSpecifics` from +0x640; the side-step state at +0x638 is
/// [`crate::object::Ship::side_step`]).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SwarmerSpec {
    /// +0x640: slot in the hive.
    pub index: i32,
    /// +0x644: the hive.
    pub hive: Option<ShipId>,
    /// +0x648: see [`swarmer`] (the stored value; [`swarmer_state`] adds what its order says).
    pub state: i32,
    /// +0x64c: when it launches (or docks) next.
    pub launch_at: f32,
    /// +0x650: its random launch delay.
    pub launch_delay: f32,
    /// +0x654: last refuel.
    pub refuel_at: f32,
}

/// Per-type statics (`HiveFrigateStaticInit`, 0x5afef0).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct HiveStatics {
    /// Time to replace a lost swarmer (15 s).
    pub regen_time: f32,
    /// Distance from the target's surface within which the swarmers launch (7000).
    pub launch_range: f32,
    /// The hive's own attack range per tactics: 80% of the gun range, 70% of the minimum.
    pub attack_range: [f32; 3],
    pub attack_min_range: [f32; 3],
}

impl HiveStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let k_max = ld(f32::from_bits(0x3f4c_cccd));
        let k_min = ld(f32::from_bits(0x3f33_3333));
        Self {
            regen_time: 15.0,
            launch_range: 7000.0,
            attack_range: stat.bullet_range.map(|r| st(ld(r) * k_max)),
            attack_min_range: stat.min_bullet_range.map(|r| st(ld(r) * k_min)),
        }
    }
}

fn statics(u: &Universe, t: ShipType) -> HiveStatics {
    match crate::ships::type_statics(u, t) {
        TypeStatics::Hive(s) => s.clone(),
        _ => HiveStatics::default(),
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&HiveSpec> {
    match u.ships.get(id).map(|s| &s.spec) {
        Some(ShipSpec::Hive(h)) => Some(h),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut HiveSpec> {
    match u.ships.get_mut(id).map(|s| &mut s.spec) {
        Some(ShipSpec::Hive(h)) => Some(h),
        _ => None,
    }
}

fn sw_spec(u: &Universe, id: ShipId) -> Option<&SwarmerSpec> {
    match u.ships.get(id).map(|s| &s.spec) {
        Some(ShipSpec::Swarmer(s)) => Some(s),
        _ => None,
    }
}

fn sw_spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut SwarmerSpec> {
    match u.ships.get_mut(id).map(|s| &mut s.spec) {
        Some(ShipSpec::Swarmer(s)) => Some(s),
        _ => None,
    }
}

/// Whether a type is a swarmer.
pub fn is_swarmer_type(t: ShipType) -> bool {
    matches!(t, ShipType::sSwarmer | ShipType::bSwarmer)
}

/// Whether a type is a hive frigate.
pub fn is_hive_type(t: ShipType) -> bool {
    matches!(t, ShipType::sHiveFrigate | ShipType::bHiveFrigate)
}

/// The hive a swarmer belongs to (`+0x644`): ApplyDamageToTarget credits a swarmer's kills
/// to it.
pub fn swarmer_hive(u: &Universe, id: ShipId) -> Option<ShipId> {
    sw_spec(u, id).and_then(|s| s.hive)
}

fn hive_effect(u: &mut Universe, id: ShipId, open: bool) {
    // madAnimationStart(hive, 0 open / 1 close)
    u.special_events.push(SpecialEvent::Special { ship: id, effect: Effect::State { state: u32::from(!open) } });
}

/// `SwarmerGetState` (0x5c2330): the stored state, corrected by where the swarmer is and what
/// its order is (docked when inside, attacking under an attack order, launching or docking
/// under a dock order, out otherwise).
pub fn swarmer_state(u: &Universe, id: ShipId) -> i32 {
    let Some(s) = u.ships.get(id) else { return swarmer::DOCKED };
    let stored = sw_spec(u, id).map_or(0, |w| w.state);
    if !u.ship_list.contains(&id) {
        return swarmer::DOCKED;
    }
    let Some(cmd) = s.command.and_then(|c| u.commands.get(c)) else {
        return if matches!(stored, swarmer::LAUNCHING | swarmer::DOCKING | swarmer::ATTACKING) { swarmer::OUT } else { stored };
    };
    match cmd.order {
        crate::command::order::ATTACK => swarmer::ATTACKING,
        crate::command::order::DOCK => match stored {
            swarmer::DOCKED => swarmer::LAUNCHING,
            swarmer::OUT | swarmer::ATTACKING => swarmer::DOCKING,
            other => other,
        },
        _ => swarmer::OUT,
    }
}

/// `SwarmerSetState` (0x5c23a0).
pub fn set_swarmer_state(u: &mut Universe, id: ShipId, st: i32) {
    if let Some(s) = sw_spec_mut(u, id) {
        s.state = st;
    }
}

/// `P2SwarmerInit` (0x5c2300).
pub fn swarmer_init(u: &mut Universe, id: ShipId) {
    let s = u.ships.ship_mut(id);
    s.side_step = Default::default();
    s.special_flags |= crate::base::special::DOCK_STAY;
    s.spec = ShipSpec::Swarmer(SwarmerSpec::default());
}

/// `SwarmerRemoveShipReferences` (0x5c23b0).
pub fn swarmer_remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if let Some(s) = sw_spec_mut(u, id) {
        if s.hive == Some(gone) {
            s.hive = None;
        }
    }
}

/// Creates the swarmer for slot `index` inside the hive (the loop body of `HiveFrigateInit`
/// and the replacement in `HiveFrigateHouseKeep`).
fn create_swarmer(u: &mut Universe, id: ShipId, index: usize) {
    let h = u.ships.ship(id);
    let t = if h.ship_race == ShipRace::Beast { ShipType::bSwarmer } else { ShipType::sSwarmer };
    let (pos, player) = (h.posinfo.position, h.player);
    let opts = CreateOptions { flags: create_flags::BUILT | create_flags::NOT_IN_LISTS, builder: Some(id), ..Default::default() };
    let Ok(sw) = crate::base::create_ship(u, t, player, pos, opts) else { return };
    let s = u.ships.ship_mut(sw);
    s.flags &= !sof::SELECTABLE;
    if let ShipSpec::Swarmer(w) = &mut s.spec {
        w.index = index as i32;
        w.hive = Some(id);
    }
    if let Some(h) = spec_mut(u, id) {
        h.count += 1;
    }
    crate::dock::put_ship_inside(u, sw, id);
    if let Some(h) = spec_mut(u, id) {
        h.swarmers[index] = Some(sw);
    }
}

/// Replaces the first lost swarmer once the regeneration time has passed.
fn regenerate(u: &mut Universe, id: ShipId, regen: f32) {
    let now = u.total_time_elapsed;
    let Some(h) = spec_mut(u, id) else { return };
    if !(h.regen_time < now) {
        return;
    }
    h.regen_time = st(ld(now) + ld(regen));
    if let Some(i) = h.swarmers.iter().position(|s| s.is_none()) {
        create_swarmer(u, id, i);
    }
}

/// `HiveFrigateInit` (0x5aff60): six swarmers, inside.
pub fn init(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    u.ships.ship_mut(id).spec = ShipSpec::Hive(HiveSpec { regen_time: now, ..Default::default() });
    for i in 0..SWARMERS {
        create_swarmer(u, id, i);
    }
}

/// `HiveFrigateFire` (0x5b0390): every ready gun fires (`DefaultShipFire`'s loop).
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    if u.ships.get(id).is_some_and(|s| s.guns.is_empty()) {
        return;
    }
    crate::gun::default_fire(u, id, target);
    if let Some(h) = spec_mut(u, id) {
        h.fly = 0;
    }
}

/// `HiveFrigateAttackPassive` (0x5b01b0).
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) {
    if rotate {
        crate::attack::attack_passive_rotate(u, id, target);
    } else {
        crate::attack::attack_simple(u, id, target);
    }
    if let Some(h) = spec_mut(u, id) {
        h.fly = 0;
    }
}

/// `HiveFrigateAttack` (0x5b0170): stand off at 80% of the gun range.
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    let hs = statics(u, s.ship_type);
    let tac = s.tactics.min(2);
    crate::attack::attack_passive(u, id, target, hs.attack_range[tac], hs.attack_min_range[tac]);
    if let Some(h) = spec_mut(u, id) {
        h.fly = 0;
    }
}

/// Starts the recall (state 5) unless idle or already recalling, and drops the targets (the
/// body shared by the special activate and the move/dock hooks).
fn recall(h: &mut HiveSpec) {
    if h.state != state::IDLE && h.state != state::RECALLING {
        h.state = state::PREPARE_RECALL;
    }
    for t in h.targets.iter_mut() {
        *t = None;
    }
    h.target = None;
    h.targets.clear();
    h.fly = 0;
}

/// `HiveFrigateSpecialActivate` (0x5b0200): recall the swarmers.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    if let Some(h) = spec_mut(u, id) {
        recall(h);
    }
    true
}

/// `HiveFrigateSpecialTarget` (0x5b0290): new targets (its own swarmers excepted); the first
/// becomes the current one. A launch in progress sends the swarmers at once.
pub fn special_target(u: &mut Universe, id: ShipId, targets: &[ShipId]) -> bool {
    let me = u.ships.ship(id).player;
    let list: Vec<Option<ShipId>> = targets
        .iter()
        .take(MAX_TARGETS)
        .map(|&t| match u.ships.get(t) {
            Some(s) if s.player == me && is_swarmer_type(s.ship_type) => None,
            _ => Some(t),
        })
        .collect();
    let Some(h) = spec_mut(u, id) else { return true };
    if let Some(first) = list.iter().flatten().next() {
        h.target = Some(*first);
    }
    h.targets = list;
    if !matches!(h.state, state::IDLE | state::FLYING | state::PREPARE_LAUNCH | state::LAUNCHING) {
        h.state = state::SEND;
    }
    h.fly = 0;
    true
}

/// `HiveFrigateOrderChanged` (0x5b0360).
pub fn order_changed(u: &mut Universe, id: ShipId) {
    if let Some(h) = spec_mut(u, id) {
        h.fly = 0;
    }
}

/// `HiveFrigateSlot20` (0x5b1000), the move hook: a hive flying to its target forgets it.
/// Keeps the ship in the move order.
pub fn move_hook(u: &mut Universe, id: ShipId) -> bool {
    if let Some(h) = spec_mut(u, id) {
        if h.state == state::IDLE || h.state == state::FLYING {
            recall(h);
        }
        h.fly = 0;
    }
    true
}

/// `HiveFrigateSlot22` (0x5b10b0), the dock hook: a hive with swarmers out recalls them first
/// and docks with `dw` afterwards (false: not docking now).
pub fn dock_hook(u: &mut Universe, id: ShipId, dw: Option<ShipId>) -> bool {
    let Some(h) = spec_mut(u, id) else { return true };
    if h.state == state::IDLE {
        return true;
    }
    recall(h);
    h.dock_with = dw;
    h.fly = 2;
    false
}

/// `HiveFrigateSlot27` (0x5b1090), the halt hook.
pub fn halt_hook(u: &mut Universe, id: ShipId) -> bool {
    if let Some(h) = spec_mut(u, id) {
        h.fly = 0;
    }
    true
}

/// `HiveFrigateIsLaunching` (0x5b0500): it has a target and is doing something about it.
pub fn is_launching(u: &Universe, id: ShipId) -> bool {
    spec(u, id).is_some_and(|h| h.target.is_some() && h.state != state::IDLE)
}

/// `HiveFrigateResetSwarmers` (0x5b0530): back to idle with every swarmer docked (an instant
/// dock of the hive, and so on).
pub fn reset_swarmers(u: &mut Universe, id: ShipId) {
    let Some(h) = spec_mut(u, id) else { return };
    h.state = state::IDLE;
    for sw in h.swarmers.into_iter().flatten() {
        set_swarmer_state(u, sw, swarmer::DOCKED);
    }
}

/// `HiveFrigateRefuelSwarmers` (0x5b0430): docked swarmers regain one hit point per second.
fn refuel_swarmers(u: &mut Universe, id: ShipId) {
    let statics = u.statics.clone();
    let now = u.total_time_elapsed;
    let Some(h) = spec(u, id) else { return };
    for sw in h.swarmers.into_iter().flatten() {
        let Some(s) = u.ships.get(sw) else { continue };
        let Some(max) = statics.ship(s.ship_type).map(|x| x.health.max_health) else { continue };
        if swarmer_state(u, sw) != swarmer::DOCKED || !(s.health < max) {
            continue;
        }
        let refuel = sw_spec(u, sw).map_or(0.0, |w| w.refuel_at);
        if 0.0 < refuel {
            let since = st(ld(now) - ld(refuel));
            if 1.0 < since {
                let s = u.ships.ship_mut(sw);
                let hp = st(ld(since) + ld(s.health));
                s.health = if max < hp { max } else { hp };
                if let Some(w) = sw_spec_mut(u, sw) {
                    w.refuel_at = now;
                }
            }
        } else if let Some(w) = sw_spec_mut(u, sw) {
            w.refuel_at = now;
        }
    }
}

/// Distance from the hive to its target's surface (`aishipGetTrajectory` + `RangeToTarget`).
fn range_to(u: &Universe, id: ShipId, target: ShipId) -> Option<f64> {
    let (me, t) = (u.ships.get(id)?, u.ships.get(target)?);
    let tstat = u.statics.ship(t.ship_type)?;
    Some(crate::aiship::range_to_target(tstat, crate::aiship::trajectory(me, t)))
}

/// Flies towards the target (state 7 when it was idle).
fn fly_to_target(u: &mut Universe, id: ShipId, target: ShipId) {
    if let Some(h) = spec_mut(u, id) {
        h.fly = 1;
        if h.state == state::IDLE {
            h.state = state::FLYING;
        }
    }
    let Some(t) = u.ships.get(target) else { return };
    let (tp, tv) = (t.posinfo.position, t.posinfo.velocity);
    crate::aiship::fly_to_point_avoiding_objs(u, id, Some(tp), FLY_FLAGS, FLY_LIMIT, Some(tv));
    let s = u.ships.ship_mut(id);
    s.ship_idle = false;
    s.autostabilize_ship = false;
}

/// The swarmers in a state (by [`swarmer_state`]).
fn swarmers_in(u: &Universe, id: ShipId) -> Vec<(ShipId, i32)> {
    spec(u, id).map_or_else(Vec::new, |h| h.swarmers.into_iter().flatten().map(|s| (s, swarmer_state(u, s))).collect())
}

fn close(u: &mut Universe, id: ShipId) {
    if let Some(h) = spec_mut(u, id) {
        if h.open {
            h.open = false;
            hive_effect(u, id, false);
        }
    }
}

/// `HiveFrigateHouseKeep` (0x5b0570).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(s) = u.ships.get(id) else { return };
    if s.special_flags & 0x80 != 0 {
        return;
    }
    let (player, tactics, ship_type) = (s.player, s.tactics, s.ship_type);
    let hs = statics(u, ship_type);
    let now = u.total_time_elapsed;
    let Some(h) = spec(u, id) else { return };
    if let Some(t) = h.target {
        if !crate::fow::ship_visible_to_player(u, t, player) {
            crate::ships::cl_special(u, &[id], &[]);
        }
    }
    let Some(h) = spec(u, id) else { return };
    let cur = h.state;
    match cur {
        state::IDLE | state::FLYING => {
            if cur == state::IDLE {
                let target = spec(u, id).and_then(|h| h.target);
                if tactics == crate::object::tactics::AGGRESSIVE
                    && target.is_none()
                    && ENEMY_NEARBY_CHECK_RATE & u.update_counter == ENEMY_NEARBY_CHECK_FRAME
                {
                    let enemies = crate::tactics::enemies_within_proximity(u, id, ENEMY_SEARCH_RANGE);
                    if !enemies.is_empty() {
                        crate::ships::cl_special(u, &[id], &enemies);
                    }
                }
                close(u, id);
            }
            if let Some(h) = spec(u, id).filter(|h| h.fly == 2) {
                let dw = h.dock_with;
                crate::dock::cl_dock(u, &[id], crate::dock::purpose::DOCK, dw);
                if let Some(h) = spec_mut(u, id) {
                    h.fly = 0;
                }
            }
            regenerate(u, id, hs.regen_time);
            refuel_swarmers(u, id);
            let Some(t) = spec(u, id).and_then(|h| h.target) else { return };
            if range_to(u, id, t).is_some_and(|r| r < ld(hs.launch_range)) {
                if let Some(h) = spec_mut(u, id) {
                    h.state = state::PREPARE_LAUNCH;
                    h.fly = 0;
                }
                return;
            }
            fly_to_target(u, id, t);
        }
        state::PREPARE_LAUNCH => {
            let mut n = 0;
            for (sw, st_) in swarmers_in(u, id) {
                if st_ != swarmer::DOCKED {
                    continue;
                }
                let r = u.randy.random(hwc_math::random::RAN_ETG);
                n += 1;
                let x = fild_u32(r) * ld(f32::from_bits(0x33c8_0000)) * ld(f32::from_bits(0xbba3_d70a));
                let delay = (1.0 - x.exp()) + (1.0 - x.exp());
                if let Some(w) = sw_spec_mut(u, sw) {
                    w.launch_delay = st(delay);
                    w.launch_at = st(delay + ld(now));
                }
            }
            if n < 1 {
                if let Some(h) = spec_mut(u, id) {
                    h.state = state::IDLE;
                }
                return;
            }
            if tactics == crate::object::tactics::AGGRESSIVE {
                let enemies = crate::tactics::enemies_within_proximity(u, id, ENEMY_SEARCH_RANGE);
                if !enemies.is_empty() {
                    crate::command::cl_attack(u, &[id], &enemies);
                }
            }
            if let Some(h) = spec_mut(u, id) {
                h.state = state::LAUNCHING;
                if !h.open {
                    h.open = true;
                    hive_effect(u, id, true);
                }
            }
        }
        state::LAUNCHING => {
            let mut all_out = true;
            let mut launch = Vec::new();
            for (sw, st_) in swarmers_in(u, id) {
                match st_ {
                    swarmer::DOCKED => {
                        if sw_spec(u, sw).is_some_and(|w| w.launch_at < now) {
                            set_swarmer_state(u, sw, swarmer::LAUNCHING);
                            launch.push(sw);
                        }
                        all_out = false;
                    }
                    swarmer::LAUNCHING => all_out = false,
                    _ => {}
                }
            }
            if !launch.is_empty() {
                crate::dock::cl_dock(u, &launch, crate::dock::purpose::LAUNCH, Some(id));
            }
            if all_out {
                if let Some(h) = spec_mut(u, id) {
                    h.state = state::SEND;
                }
            }
        }
        state::SEND => {
            let mut attackers = Vec::new();
            for (sw, st_) in swarmers_in(u, id) {
                if st_ != swarmer::DOCKED && st_ != swarmer::LAUNCHING {
                    set_swarmer_state(u, sw, swarmer::ATTACKING);
                    attackers.push(sw);
                }
            }
            let target = spec(u, id).and_then(|h| h.target);
            if let Some(t) = target {
                if !attackers.is_empty() {
                    crate::command::cl_attack(u, &attackers, &[t]);
                }
            }
            if let Some(h) = spec_mut(u, id) {
                h.state = state::ATTACKING;
            }
            if let Some(t) = target {
                if range_to(u, id, t).is_some_and(|r| ld(hs.launch_range) <= r) {
                    fly_to_target(u, id, t);
                }
            }
        }
        state::ATTACKING => {
            let target = spec(u, id).and_then(|h| h.target);
            if let Some(t) = target.filter(|_| spec(u, id).is_some_and(|h| h.fly == 1)) {
                if range_to(u, id, t).is_some_and(|r| r < ld(hs.launch_range)) {
                    if let Some(h) = spec_mut(u, id) {
                        h.fly = 0;
                    }
                } else {
                    fly_to_target(u, id, t);
                }
            }
            let mut attackers = Vec::new();
            for (sw, st_) in swarmers_in(u, id) {
                if st_ == swarmer::OUT {
                    set_swarmer_state(u, sw, swarmer::ATTACKING);
                    attackers.push(sw);
                }
            }
            if let Some(t) = target {
                if !attackers.is_empty() {
                    crate::command::cl_attack(u, &attackers, &[t]);
                }
                if range_to(u, id, t).is_some_and(|r| r < ld(hs.launch_range)) {
                    let mut launch = Vec::new();
                    for (sw, st_) in swarmers_in(u, id) {
                        if st_ == swarmer::DOCKED {
                            set_swarmer_state(u, sw, swarmer::LAUNCHING);
                            launch.push(sw);
                        }
                    }
                    if !launch.is_empty() {
                        crate::dock::cl_dock(u, &launch, crate::dock::purpose::LAUNCH, Some(id));
                    }
                }
            }
            regenerate(u, id, hs.regen_time);
        }
        state::PREPARE_RECALL => {
            let mut n = 0;
            for (sw, st_) in swarmers_in(u, id) {
                if st_ != swarmer::DOCKED && st_ != swarmer::DOCKING {
                    n += 1;
                    if let Some(w) = sw_spec_mut(u, sw) {
                        w.launch_at = st(ld(now) + ld(w.launch_delay));
                    }
                }
            }
            if let Some(h) = spec_mut(u, id) {
                h.state = if n > 0 { state::RECALLING } else { state::IDLE };
            }
            if n == 0 {
                close(u, id);
            }
        }
        state::RECALLING => {
            let mut all_in = true;
            let mut dock = Vec::new();
            for (sw, st_) in swarmers_in(u, id) {
                match st_ {
                    swarmer::OUT | swarmer::ATTACKING => {
                        if sw_spec(u, sw).is_some_and(|w| w.launch_at < now) {
                            set_swarmer_state(u, sw, swarmer::DOCKING);
                            dock.push(sw);
                        }
                        all_in = false;
                    }
                    swarmer::DOCKED => {}
                    _ => all_in = false,
                }
            }
            if !dock.is_empty() {
                crate::dock::cl_dock(u, &dock, crate::dock::purpose::DOCK_STAY, Some(id));
            }
            if all_in {
                if let Some(h) = spec_mut(u, id) {
                    h.state = state::IDLE;
                }
                close(u, id);
            }
        }
        _ => {}
    }
}

/// `HiveFrigateRemoveShipReferences` (0x5b0080): a lost swarmer frees its slot; a lost target
/// is replaced by the next one (or the swarmers are recalled).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let is_sw = u.ships.get(gone).is_some_and(|s| is_swarmer_type(s.ship_type));
    let Some(h) = spec_mut(u, id) else { return };
    if is_sw {
        if let Some(i) = h.swarmers.iter().position(|&s| s == Some(gone)) {
            h.swarmers[i] = None;
            h.count -= 1;
        }
    }
    for t in h.targets.iter_mut() {
        if *t == Some(gone) {
            *t = None;
        }
    }
    if h.target != Some(gone) {
        return;
    }
    if let Some(next) = h.targets.iter().flatten().next().copied() {
        h.target = Some(next);
        if h.state == state::ATTACKING {
            h.state = state::SEND;
        }
        return;
    }
    h.target = None;
    h.targets.clear();
    if h.state != state::IDLE {
        h.state = state::PREPARE_RECALL;
    }
}

/// `HiveFrigateDied` (0x5b0020): the swarmers that are out die with it (those inside go with
/// the ships inside).
pub fn died(u: &mut Universe, id: ShipId) {
    let Some(h) = spec(u, id) else { return };
    let swarmers = h.swarmers;
    let (how, who) = {
        let s = u.ships.ship(id);
        (s.how_did_i_die, s.who_killed_me)
    };
    for sw in swarmers.into_iter().flatten() {
        let alive_outside = u.ships.get(sw).is_some_and(|s| s.flags & sof::DEAD == 0) && u.ship_list.contains(&sw);
        if alive_outside {
            let s = u.ships.ship_mut(sw);
            s.how_did_i_die = how;
            s.who_killed_me = who;
            crate::damage::really_delete_ship(u, sw);
        }
    }
    if let Some(h) = spec_mut(u, id) {
        h.swarmers = [None; SWARMERS];
    }
}

/// Slots 20/22/27 for the hive (kept for the shared `cancel_special` dispatch: a recall).
pub fn cancel_special(u: &mut Universe, id: ShipId) {
    move_hook(u, id);
}
