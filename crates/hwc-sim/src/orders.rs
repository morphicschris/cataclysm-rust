//! The orders a player gives, as data (the original's command-layer packets: `clWrapMove`,
//! `clWrapAttack`, `clWrapBuildShip`, `clWrapResearch`...). In a single-player game the
//! interface applies them at once; in a network game they travel to every machine and are
//! applied there at the same universe update, so all simulations stay the same
//! (`hwc-net`). [`apply`] is the one place an order changes the universe.
//!
//! An order only moves ships of the player who gave it: a ship that has changed hands or died
//! between the click and the update the order is applied at is left out, the same on every
//! machine.

use hwc_math::Vector;

use crate::object::ShipId;
use crate::player_orders::{self, MimicTarget, SpecialItem, TargetOrder};
use crate::resource::ResourceId;
use crate::ships::mimic::DisguiseTables;
use crate::statics::ShipType;
use crate::universe::Universe;

/// One order of a player.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Order {
    /// `clMove` with a waypoint mode, the point the move plate was centred on and the
    /// waypoints.
    Move { ships: Vec<ShipId>, mode: u32, from: Vector, waypoints: Vec<Vector> },
    /// `clAttack`.
    Attack { ships: Vec<ShipId>, targets: Vec<ShipId> },
    /// `clProtect`: the ships guard the targets.
    Protect { ships: Vec<ShipId>, targets: Vec<ShipId> },
    /// `mrProtectShips`: Sentinels shield the targets (or each other).
    ProtectShips { ships: Vec<ShipId>, targets: Option<Vec<ShipId>> },
    /// `clHalt`.
    Halt { ships: Vec<ShipId> },
    /// `clDock` with a purpose (`dock::purpose`) and the ship to dock with.
    Dock { ships: Vec<ShipId>, purpose: i32, with: Option<ShipId> },
    /// `clCollectResource`: the harvesters among the ships go for the resource (the nearest
    /// without one).
    Harvest { ships: Vec<ShipId>, resource: Option<ResourceId> },
    /// `mrSetTactics`.
    Tactics { ships: Vec<ShipId>, tactics: usize },
    /// `clFormation` (more than one ship).
    Formation { ships: Vec<ShipId>, formation: u32 },
    /// `mrUpgradeShips`.
    Upgrade { ships: Vec<ShipId> },
    /// `mrEnterNearestSlipgate`.
    EnterSlipgate { ships: Vec<ShipId>, mission_15: bool },
    /// `mrDockAtSlipgate`.
    DockAtSlipgate { ships: Vec<ShipId>, gate: ShipId, mission_15: bool },
    /// `MothershipBigGunTargetPicked`: the Siege Cannon fires at the point.
    BigGun { ships: Vec<ShipId>, point: Vector },
    /// `mrBigGunCancel`.
    CancelBigGun { ships: Vec<ShipId> },
    /// `kbcbKamikaze`: the ships that can, ram their targets.
    Kamikaze { ships: Vec<ShipId> },
    /// `mrLeechSet`.
    LeechMode { ships: Vec<ShipId>, mode: u8 },
    /// `mrScuttle` once confirmed (the asking stays with the player who gives it).
    Scuttle { ships: Vec<ShipId> },
    /// `pmExecuteSpecialItem` on the targets.
    Special { ships: Vec<ShipId>, item: SpecialItem, targets: Vec<ShipId> },
    /// The repair key: each ship repairs its nearest ally.
    RepairKey { ships: Vec<ShipId> },
    /// `mrSpecialTargetOrder`.
    SpecialTarget { ships: Vec<ShipId>, targets: Option<Vec<ShipId>>, order: TargetOrder },
    /// `mrMimicDisguiseAsTarget`.
    MimicAsTarget { ships: Vec<ShipId>, target: MimicTarget },
    /// The Mimic menu's disguise `index` of category `slot`.
    MimicAs { ships: Vec<ShipId>, slot: usize, index: usize },
    /// `clWrapBuildShip` / the build manager's queue.
    Build { factory: ShipId, ship_type: ShipType, count: i32 },
    /// `cmDecrement`: queued ships taken off a job (never the one in progress).
    Unqueue { factory: ShipId, ship_type: ShipType, count: i32 },
    /// `cmCancelJobs`.
    CancelBuild { factory: ShipId, ship_type: ShipType },
    /// A build job paused or resumed.
    PauseBuild { factory: ShipId, ship_type: ShipType, paused: bool },
    /// `rmResearchTechIfLabFree`.
    Research { tech: usize },
    /// `rmClearResearchlab`.
    ClearLab { lab: usize },
    /// `clLaunchMultipleShips`.
    Launch { launcher: ShipId, ships: Vec<ShipId> },
    /// `clAutoLaunch` with the player's new auto-launch bits.
    AutoLaunch { bits: u32 },
    /// The mothership's repair priority for a module type.
    RepairPriority { module_type: ShipType, priority: i32 },
    /// The Beast mothership's emergency power setting.
    BeastSystems { mode: u32 },
    /// `clWrapMpHyperspace`: the ships jump from around `from` to around `to` (a multiplayer
    /// game with hyperspace on; the player pays the jump).
    MpHyperspace { ships: Vec<ShipId>, from: Vector, to: Vector },
}

/// What a recorded game holds besides its starting saved game (the original's `.pkts`
/// recordings hold the lockstep packets): every order applied and every player who dropped
/// out, by the universe update it came before.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RecordedEvent {
    Order { player: u8, order: Order },
    PlayerDropped { player: u8 },
}

/// A recording: the player it was made by and the events by update counter (applied when the
/// universe has made that many updates, before the next).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Recording {
    pub local_player: u8,
    pub events: Vec<(u32, RecordedEvent)>,
}

/// The first bytes of a recording file.
const RECORDING_MAGIC: &[u8; 8] = b"HWCREC01";

impl Recording {
    pub fn write(&self, path: &std::path::Path) -> std::io::Result<()> {
        let mut bytes = RECORDING_MAGIC.to_vec();
        bytes.extend(bincode::serialize(self).map_err(std::io::Error::other)?);
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(path, bytes)
    }

    pub fn read(path: &std::path::Path) -> std::io::Result<Self> {
        let bytes = std::fs::read(path)?;
        if bytes.len() < 8 || &bytes[..8] != RECORDING_MAGIC {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a recording"));
        }
        bincode::deserialize(&bytes[8..]).map_err(std::io::Error::other)
    }

    /// Applies what was recorded before the update about to run (the universe's update counter
    /// is the number of updates made); `next` is the first event not yet applied.
    pub fn apply_due(&self, u: &mut Universe, next: &mut usize, disguises: Option<&DisguiseTables>) {
        while let Some((at, e)) = self.events.get(*next) {
            if *at > u.update_counter {
                break;
            }
            match e {
                RecordedEvent::Order { player, order } => apply(u, usize::from(*player), order, disguises),
                RecordedEvent::PlayerDropped { player } => crate::multiplayer::player_dropped(u, usize::from(*player)),
            }
            *next += 1;
        }
    }
}

/// The player's live ships among `ships`, in order.
fn own(u: &Universe, player: usize, ships: &[ShipId]) -> Vec<ShipId> {
    ships.iter().copied().filter(|&id| u.ships.get(id).is_some_and(|s| s.player == player && s.flags & crate::object::sof::DEAD == 0)).collect()
}

/// The targets still there: ships, and live mines (which share the ship numbers' space).
fn live(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    ships.iter().copied().filter(|&id| u.ships.get(id).is_some() || crate::target::exists(u, id)).collect()
}

/// The harvesters among the ships (harvesting allowed, resourcers able to harvest).
pub fn harvesters(u: &Universe, ships: &[ShipId]) -> Vec<ShipId> {
    let enabled = u.single_player || u.base.options.flags & crate::base::game_flags::HARVESTING_ENABLED != 0;
    if !enabled {
        return Vec::new();
    }
    ships
        .iter()
        .copied()
        .filter(|&id| u.ships.get(id).and_then(|s| u.statics.ship(s.ship_type)).is_some_and(|st| st.ship_class == Some(crate::statics::ShipClass::Resource) && 0 < st.harvest_amount))
        .collect()
}

/// Applies `player`'s order to the universe. `disguises` are the Mimic menu's tables.
pub fn apply(u: &mut Universe, player: usize, order: &Order, disguises: Option<&DisguiseTables>) {
    use crate::command;
    match order {
        Order::Move { ships, mode, from, waypoints } => {
            let s = own(u, player, ships);
            if !s.is_empty() && !waypoints.is_empty() {
                command::cl_move(u, &s, *mode, *from, waypoints);
            }
        }
        Order::Attack { ships, targets } => {
            let s = own(u, player, ships);
            let t = live(u, targets);
            if !s.is_empty() && !t.is_empty() {
                command::cl_attack(u, &s, &t);
            }
        }
        Order::Protect { ships, targets } => {
            let s = own(u, player, ships);
            let t = live(u, targets);
            if !s.is_empty() && !t.is_empty() {
                command::cl_protect(u, &s, &t);
            }
        }
        Order::ProtectShips { ships, targets } => {
            let s = own(u, player, ships);
            let t = targets.as_ref().map(|t| live(u, t));
            player_orders::mr_protect_ships(u, &s, t.as_deref());
        }
        Order::Halt { ships } => {
            let s = own(u, player, ships);
            if !s.is_empty() {
                command::cl_halt(u, &s);
            }
        }
        Order::Dock { ships, purpose, with } => {
            let s = own(u, player, ships);
            let w = with.filter(|&w| u.ships.get(w).is_some());
            if !s.is_empty() && (with.is_none() || w.is_some()) {
                crate::dock::cl_dock(u, &s, *purpose, w);
            }
        }
        Order::Harvest { ships, resource } => {
            let s = harvesters(u, &own(u, player, ships));
            if !s.is_empty() {
                crate::resource::cl_collect_resource(u, &s, *resource);
            }
        }
        Order::Tactics { ships, tactics } => {
            let s = own(u, player, ships);
            player_orders::mr_set_tactics(u, &s, *tactics);
        }
        Order::Formation { ships, formation } => {
            let s = own(u, player, ships);
            if s.len() > 1 {
                command::cl_formation(u, &s, *formation);
            }
        }
        Order::Upgrade { ships } => {
            let s = own(u, player, ships);
            player_orders::mr_upgrade_ships(u, &s);
        }
        Order::EnterSlipgate { ships, mission_15 } => {
            let s = own(u, player, ships);
            player_orders::mr_enter_nearest_slipgate(u, player, &s, *mission_15);
        }
        Order::DockAtSlipgate { ships, gate, mission_15 } => {
            let s = own(u, player, ships);
            if u.ships.get(*gate).is_some() {
                player_orders::mr_dock_at_slipgate(u, &s, *gate, *mission_15);
            }
        }
        Order::BigGun { ships, point } => {
            let s = own(u, player, ships);
            if !s.is_empty() {
                crate::ships::mothership::cl_big_gun(u, &s, *point);
            }
        }
        Order::CancelBigGun { ships } => {
            for id in own(u, player, ships) {
                if crate::ships::mothership::big_gun_is_active(u, id) {
                    crate::ships::cl_special(u, &[id], &[]);
                }
            }
        }
        Order::MpHyperspace { ships, from, to } => {
            let s = own(u, player, ships);
            crate::mp_hyperspace::cl_mp_hyperspace(u, &s, *from, *to);
        }
        Order::Kamikaze { ships } => {
            let s = own(u, player, ships);
            player_orders::cl_kamikaze(u, &s);
        }
        Order::LeechMode { ships, mode } => {
            let s = own(u, player, ships);
            player_orders::mr_leech_set(u, &s, *mode);
        }
        Order::Scuttle { ships } => {
            let mut s = own(u, player, ships);
            player_orders::scuttleable(u, &mut s);
            player_orders::retireable(u, &mut s);
            if !s.is_empty() {
                command::cl_scuttle(u, &s);
            }
        }
        Order::Special { ships, item, targets } => {
            let s = own(u, player, ships);
            let t = live(u, targets);
            player_orders::execute_special(u, player, &s, *item, &t);
        }
        Order::RepairKey { ships } => {
            let s = own(u, player, ships);
            player_orders::repair_key(u, player, &s);
        }
        Order::SpecialTarget { ships, targets, order } => {
            let s = own(u, player, ships);
            let t = targets.as_ref().map(|t| live(u, t));
            player_orders::special_target_order(u, player, &s, t.as_deref(), *order);
        }
        Order::MimicAsTarget { ships, target } => {
            let mut s = own(u, player, ships);
            player_orders::with_upgrade(u, &mut s, 0x20);
            if let MimicTarget::Ship(t) = target {
                if u.ships.get(*t).is_none() {
                    return;
                }
            }
            player_orders::mr_mimic_disguise_as_target(u, &s, *target);
        }
        Order::MimicAs { ships, slot, index } => {
            let s = own(u, player, ships);
            if let Some(t) = disguises {
                player_orders::mimic_disguise_as(u, t, (*slot).min(8), &s, *index);
            }
        }
        Order::Build { factory, ship_type, count } => {
            if own(u, player, &[*factory]).is_empty() {
                return;
            }
            crate::construction::queue_build(u, *factory, *ship_type, *count);
        }
        Order::Unqueue { factory, ship_type, count } => {
            if let Some(j) = job_mut(u, player, *factory, *ship_type) {
                let n = (*count).min(j.n_jobs - 1).max(0);
                j.n_jobs -= n;
                j.n_total = (j.n_total - n).max(j.n_jobs);
            }
        }
        Order::CancelBuild { factory, ship_type } => {
            if own(u, player, &[*factory]).is_empty() {
                return;
            }
            crate::construction::cancel_job(u, *factory, *ship_type);
        }
        Order::PauseBuild { factory, ship_type, paused } => {
            if let Some(j) = job_mut(u, player, *factory, *ship_type) {
                j.paused = *paused;
            }
        }
        Order::Research { tech } => {
            crate::research::research_tech_if_lab_free(u, player, *tech);
        }
        Order::ClearLab { lab } => {
            crate::research::clear_research_lab(u, player, *lab);
        }
        Order::Launch { launcher, ships } => {
            let s = own(u, player, ships);
            if !own(u, player, &[*launcher]).is_empty() && !s.is_empty() {
                crate::dock::cl_launch_multiple_ships(u, &s, *launcher);
            }
        }
        Order::AutoLaunch { bits } => {
            crate::dock::cl_auto_launch(u, *bits, player);
        }
        Order::RepairPriority { module_type, priority } => {
            if let Some(ms) = u.base.players.get(player).and_then(|p| p.mothership) {
                crate::ships::mothership::set_repair_priority(u, ms, *module_type, *priority);
            }
        }
        Order::BeastSystems { mode } => {
            if let Some(ms) = u.base.players.get(player).and_then(|p| p.mothership) {
                crate::ships::mothership::set_damage_mode(u, ms, *mode);
            }
        }
    }
}

/// The job building `t` at the player's factory.
fn job_mut(u: &mut Universe, player: usize, factory: ShipId, t: ShipType) -> Option<&mut crate::construction::BuildJob> {
    if u.ships.get(factory).is_none_or(|s| s.player != player) {
        return None;
    }
    u.base.factories.iter_mut().find(|f| f.ship == Some(factory))?.jobs.iter_mut().find(|j| j.n_jobs > 0 && j.info == Some(t))
}

/// A checksum of the state every machine of a network game must agree on (`univGetChecksum`
/// and more): each ship's identity, owner, position, velocity and health, each player's RUs,
/// and the random streams the simulation draws from (the game's and the computer players'; not
/// the battle chatter's or the effects', which depend on what this machine shows).
pub fn sync_checksum(u: &Universe) -> u64 {
    let mut h = Fnv::default();
    for &id in &u.ship_list {
        let Some(s) = u.ships.get(id) else { continue };
        h.u32(u32::from(id.0));
        h.u32(s.ship_type.index() as u32);
        h.u32(s.player as u32);
        let p = s.posinfo.position;
        for v in [p.x, p.y, p.z, s.posinfo.velocity.x, s.posinfo.velocity.y, s.posinfo.velocity.z, s.health] {
            h.u32(v.to_bits());
        }
    }
    for p in &u.base.players {
        h.u32(p.resource_units as u32);
    }
    for i in [
        hwc_math::random::RAN_AI_PLAYER,
        hwc_math::random::RAN_GAME,
    ] {
        let mut s = u.randy.streams[i];
        h.u32(s.next_u32());
    }
    h.u32(u.update_counter);
    h.0
}

/// FNV-1a, 64 bits.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn u32(&mut self, v: u32) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}
