//! The per-type ship modules (`CustShipHeader`, the 145-entry table at 0x8a75f8): the
//! callbacks each ship type plugs into the universe update, and the state each keeps
//! (`ShipSpecifics`, +0x638 onwards).
//!
//! Cataclysm's header has 35 slots (see `recovered/naming/s0-modules.md` for the layout);
//! the ones the simulation calls are:
//!
//! | slot | callback | called from |
//! | --- | --- | --- |
//! | 1 | static init | [`crate::combat_statics::CombatStatics::build`] ([`parse_statics`]) |
//! | 3 | init | `univCreateShip` ([`init`]) |
//! | 4 | close | `univDeleteDeadShip`, after died ([`close`]) |
//! | 5 | fire | [`crate::attack::ship_fire`] ([`fire`]) |
//! | 6 | passive attack | [`crate::attack::ship_attack_passive`] ([`attack_passive`]) |
//! | 7 | special activate | `clSpecial` / `processSpecialToDo` ([`special_activate`]) |
//! | 8 | special target | `processSpecialToDo` ([`special_target`]) |
//! | 9 | order changed | `FreeLastOrder`, `RemoveShipsFromDoingStuff` ([`order_changed`]) |
//! | 10 | housekeeping | `univUpdateAllPosVelShips` ([`housekeep`]) |
//! | 11 | remove ship references | `univRemoveAllShipReferencesFromSpecialShip` ([`remove_ship_refs`]) |
//! | 12 | the same when a ship docks | `univShipThink`'s dock notification ([`remove_ship_refs_docked`]) |
//! | 13 | remove effect / target references | `univRemoveEffectRefsFromSpecialShips` ([`remove_target_ref`]) |
//! | 14 | Beast cascade references | the cascade code ([`cascade_remove_refs`]) |
//! | 15 | died | `univDeleteDeadShip` ([`died`]) |
//! | 20, 22, 27 | cancel the special | orders that replace it ([`cancel_special`]) |
//! | 21 | attack | [`crate::attack::ship_attack`] ([`attack`]) |
//! | 34 | formation changed | sentinels ([`formation_changed`]) |
//!
//! Save/load/fix slots have no counterpart (there is no save game yet).

use crate::object::{Ship, ShipId};
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

pub mod acolyte;
pub mod acv;
pub mod beast;
pub mod cloaker;
pub mod crystal;
pub mod dfg;
pub mod dreadnought;
pub mod gravwell;
pub mod heavy_corvette;
pub mod hive;
pub mod leech;
pub mod mimic;
pub mod minelayer;
pub mod missile_ships;
pub mod mothership;
pub mod nomad;
pub mod processor;
pub mod proximity;
pub mod ramming;
pub mod repair_droid;
pub mod sentinel;
pub mod slipgate;
pub mod worker_repair;

/// The ship module a type uses (which `CustShipHeader`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Module {
    Acolyte,
    Acv,
    Mimic,
    Mcv,
    P1StandardCorvette,
    Leech,
    Ramming,
    Hive,
    Swarmer,
    Dreadnought,
    ShipModule,
    Carrier,
    Processor,
    RepairDroid,
    Sentinel,
    Mothership,
    CloakedFighter,
    IonArray,
    Dfg,
    HeavyCruiser,
    BeastMothership,
    CruiseMissile,
    MissileCorvette,
    MissileDestroyer,
    Minelayer,
    GravWell,
    ProximitySensor,
    NomadMoon,
    Crystal,
    Worker,
    SalCap,
    /// The slipgates (`dSlipgateSmall/Medium/Large`).
    Slipgate,
    HeavyCorvette,
    /// Everything else (no ship-specific callbacks beyond the combat routines).
    Generic,
}

/// The header of a type.
pub fn module_of(t: ShipType) -> Module {
    use ShipType::*;
    match t {
        sAcolyte | bAcolyte | dAcolyte => Module::Acolyte,
        sACV | bACV => Module::Acv,
        sMimic => Module::Mimic,
        sMCV => Module::Mcv,
        p1StandardCorvette => Module::P1StandardCorvette,
        sLeech => Module::Leech,
        sRammingFrigate | bRammingFrigate => Module::Ramming,
        sHiveFrigate | bHiveFrigate => Module::Hive,
        sSwarmer | bSwarmer => Module::Swarmer,
        sDreadnought => Module::Dreadnought,
        sDreadnoughtCannon | sCarrierSupport | sMothershipSpecial | sMothershipEngineering | sMothershipArmour
        | sMothershipMicro | sMothershipWeapons | sMothershipDockingBay | sMothershipSupport | sMothershipCommand
        | sMothershipEnginesLower | sMothershipEnginesUpper | sMothershipLowerSpine | sMothershipBigGun
        | sMothershipOreCanArmour | bMothershipDockingBay | bMothershipEnginesLower | bSectMoship | bMothershipSupport => {
            Module::ShipModule
        }
        sCarrier | bCarrier | kCarrier | tCarrier | dKushanCarrier => Module::Carrier,
        sProcessor | bProcessor => Module::Processor,
        sRepairDroid | bRepairDroid => Module::RepairDroid,
        sSentinel | bSentinel => Module::Sentinel,
        sMothership | bMothership | trGhostship => Module::Mothership,
        bCloakedFighter => Module::CloakedFighter,
        bIonArrayFrigate | p1IonArrayFrigate => Module::IonArray,
        bDFGFrigate | tDFGFrigate => Module::Dfg,
        bHeavyCruiser | kHeavyCruiser | tHeavyCruiser => Module::HeavyCruiser,
        dBeastMothership => Module::BeastMothership,
        bCruiseMissile => Module::CruiseMissile,
        bMissileCorvette | p1MissileCorvette => Module::MissileCorvette,
        tMissileDestroyer => Module::MissileDestroyer,
        tMinelayerCorvette => Module::Minelayer,
        kGravWellGenerator | tGravWellGenerator => Module::GravWell,
        kProximitySensor | tProximitySensor => Module::ProximitySensor,
        dNomadMoon => Module::NomadMoon,
        dCrystal0 | dCrystal1 => Module::Crystal,
        sWorker | bWorker => Module::Worker,
        kSalCapCorvette | tSalCapCorvette => Module::SalCap,
        dSlipgateSmall | dSlipgateMedium | dSlipgateLarge => Module::Slipgate,
        bHeavyCorvette | kHeavyCorvette => Module::HeavyCorvette,
        _ => Module::Generic,
    }
}

/// Per-ship state of the modules (`ShipSpecifics`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum ShipSpec {
    #[default]
    None,
    Acolyte(acolyte::AcolyteSpec),
    Acv(acv::AcvSpec),
    Mimic(mimic::MimicSpec),
    Leech(leech::LeechSpec),
    Ramming(ramming::RammingSpec),
    Hive(hive::HiveSpec),
    Swarmer(hive::SwarmerSpec),
    Dreadnought(dreadnought::DreadnoughtSpec),
    Processor(processor::ProcessorSpec),
    RepairDroid(repair_droid::RepairDroidSpec),
    Sentinel(sentinel::SentinelSpec),
    Mothership(mothership::MothershipSpec),
    Cloaker(cloaker::CloakerSpec),
    Dfg(dfg::DfgSpec),
    GravWell(gravwell::GravWellSpec),
    Minelayer(minelayer::MinelayerSpec),
    MissileShip(missile_ships::MissileShipSpec),
    ProximitySensor(proximity::ProximitySpec),
    NomadMoon(nomad::NomadSpec),
    Crystal(crystal::CrystalSpec),
    BeastMothership(beast::BeastMothershipSpec),
    Slipgate(slipgate::SlipgateSpec),
    HeavyCorvette(heavy_corvette::HeavyCorvetteSpec),
}

/// Per-type statics the modules' static initialisers read from the ship files
/// (`CustShipStaticInit`, kept at static +0x194).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub enum TypeStatics {
    #[default]
    None,
    Acolyte(acolyte::AcolyteStatics),
    Acv(acv::AcvStatics),
    Mimic(mimic::MimicStatics),
    Leech(leech::LeechStatics),
    Ramming(ramming::RammingStatics),
    Hive(hive::HiveStatics),
    Dreadnought(dreadnought::DreadnoughtStatics),
    Processor(processor::ProcessorStatics),
    RepairDroid(repair_droid::RepairDroidStatics),
    Sentinel(sentinel::SentinelStatics),
    Mothership(mothership::MothershipStatics),
    Cloaker(cloaker::CloakerStatics),
    Dfg(dfg::DfgStatics),
    GravWell(gravwell::GravWellStatics),
    Minelayer(minelayer::MinelayerStatics),
    MissileShip(missile_ships::MissileShipStatics),
    ProximitySensor(proximity::ProximityStatics),
    NomadMoon(nomad::NomadStatics),
    Crystal(crystal::CrystalStatics),
    Beast(beast::BeastStatics),
}

/// `CustShipStaticInit` of the type's module.
pub fn parse_statics(stat: &ShipStaticInfo) -> TypeStatics {
    match module_of(stat.ship_type) {
        Module::Acolyte => TypeStatics::Acolyte(acolyte::AcolyteStatics::parse(stat)),
        Module::Acv => TypeStatics::Acv(acv::AcvStatics::parse(stat)),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => TypeStatics::Mimic(mimic::MimicStatics::parse(stat)),
        Module::Leech => TypeStatics::Leech(leech::LeechStatics::parse(stat)),
        Module::Ramming => TypeStatics::Ramming(ramming::RammingStatics::parse(stat)),
        Module::Hive | Module::Swarmer => TypeStatics::Hive(hive::HiveStatics::parse(stat)),
        Module::Dreadnought => TypeStatics::Dreadnought(dreadnought::DreadnoughtStatics::parse(stat)),
        Module::Processor => TypeStatics::Processor(processor::ProcessorStatics::parse(stat)),
        Module::RepairDroid => TypeStatics::RepairDroid(repair_droid::RepairDroidStatics::parse(stat)),
        Module::Sentinel => TypeStatics::Sentinel(sentinel::SentinelStatics::parse(stat)),
        Module::Mothership => TypeStatics::Mothership(mothership::MothershipStatics::parse(stat)),
        Module::CloakedFighter | Module::IonArray => TypeStatics::Cloaker(cloaker::CloakerStatics::parse(stat)),
        Module::Dfg => TypeStatics::Dfg(dfg::DfgStatics::parse(stat)),
        Module::GravWell => TypeStatics::GravWell(gravwell::GravWellStatics::parse(stat)),
        Module::Minelayer => TypeStatics::Minelayer(minelayer::MinelayerStatics::parse(stat)),
        Module::MissileCorvette | Module::MissileDestroyer | Module::CruiseMissile => {
            TypeStatics::MissileShip(missile_ships::MissileShipStatics::parse(stat))
        }
        Module::ProximitySensor => TypeStatics::ProximitySensor(proximity::ProximityStatics::parse(stat)),
        Module::NomadMoon => TypeStatics::NomadMoon(nomad::NomadStatics::parse(stat)),
        Module::Crystal => TypeStatics::Crystal(crystal::CrystalStatics::parse(stat)),
        Module::HeavyCruiser | Module::Carrier | Module::BeastMothership => TypeStatics::Beast(beast::BeastStatics::parse(stat)),
        _ => TypeStatics::None,
    }
}

/// The type statics of a ship's type.
pub fn type_statics(u: &Universe, t: ShipType) -> &TypeStatics {
    static NONE: TypeStatics = TypeStatics::None;
    u.combat.get(t).map_or(&NONE, |c| &c.module)
}

fn module(u: &Universe, id: ShipId) -> Option<Module> {
    u.ships.get(id).map(|s| module_of(s.ship_type))
}

/// The cloak of a cloaker (cloaked fighter +0x650, ion array frigates +0x638).
pub fn cloak_ref(s: &Ship) -> Option<&crate::cloak::Cloak> {
    match &s.spec {
        ShipSpec::Cloaker(c) => Some(&c.cloak),
        _ => None,
    }
}

/// Mutable [`cloak_ref`].
pub fn cloak_of(s: &mut Ship) -> Option<&mut crate::cloak::Cloak> {
    match &mut s.spec {
        ShipSpec::Cloaker(c) => Some(&mut c.cloak),
        _ => None,
    }
}

/// Slot 3, `CustShipInit`: called when the ship is created.
pub fn init(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acolyte => acolyte::init(u, id),
        Module::Acv => acv::init(u, id),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => mimic::init(u, id),
        Module::Leech => leech::init(u, id),
        Module::Ramming => ramming::init(u, id),
        Module::Hive => hive::init(u, id),
        Module::Swarmer => hive::swarmer_init(u, id),
        Module::Dreadnought => dreadnought::init(u, id),
        Module::Processor => processor::init(u, id),
        Module::RepairDroid => repair_droid::init(u, id),
        Module::Sentinel => sentinel::init(u, id),
        Module::Mothership => {
            mothership::init(u, id);
            beast::mothership_init(u, id);
        }
        Module::CloakedFighter | Module::IonArray => cloaker::init(u, id),
        Module::Dfg => dfg::init(u, id),
        Module::GravWell => gravwell::init(u, id),
        Module::Minelayer => minelayer::init(u, id),
        Module::MissileCorvette | Module::MissileDestroyer | Module::CruiseMissile => missile_ships::init(u, id),
        Module::ProximitySensor => proximity::init(u, id),
        Module::NomadMoon => nomad::init(u, id),
        Module::Crystal => crystal::init(u, id),
        Module::HeavyCruiser | Module::Carrier | Module::BeastMothership => beast::init(u, id),
        Module::Worker => worker_repair::init(u, id),
        Module::Slipgate => slipgate::init(u, id),
        Module::HeavyCorvette => heavy_corvette::init(u, id),
        _ => {}
    }
}

/// Slot 4, `CustShipClose`.
pub fn close(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acv => acv::close(u, id),
        Module::Leech => leech::close(u, id),
        Module::Ramming => ramming::close(u, id),
        Module::GravWell => gravwell::close(u, id),
        Module::Slipgate => slipgate::close(u, id),
        _ => {}
    }
}

/// Slot 5, `CustShipFire`, for the types whose fire routine lives in a module. Returns false
/// when the type uses one of the shared routines in [`crate::attack`].
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) -> bool {
    let Some(m) = module(u, id) else { return false };
    match m {
        Module::Acolyte => acolyte::fire(u, id, target),
        Module::Acv => acv::fire(u, id, target),
        Module::Hive => hive::fire(u, id, target),
        Module::CloakedFighter => cloaker::fire(u, id, target),
        Module::IonArray => crate::gun::default_fire(u, id, target),
        Module::Mimic | Module::Mcv => crate::attack::interceptor_fire(u, id, target),
        Module::P1StandardCorvette => crate::gun::default_fire(u, id, target),
        Module::Minelayer => minelayer::fire(u, id, target),
        Module::NomadMoon => nomad::fire(u, id, target),
        Module::Mothership => crate::gun::default_fire(u, id, target),
        _ => return false,
    }
    true
}

/// Slot 6, `CustShipAttackPassive`, for module routines (false: shared routine). `rotate`:
/// the ship may turn to retaliate (the caller's flag and the type's `rotateToRetaliate`).
pub fn attack_passive(u: &mut Universe, id: ShipId, target: ShipId, rotate: bool) -> bool {
    let Some(m) = module(u, id) else { return false };
    match m {
        Module::Mimic => mimic::attack_passive(u, id, target, rotate),
        Module::Mcv | Module::P1StandardCorvette => mimic::mcv_attack_passive(u, id, target, rotate),
        Module::Ramming => {}
        Module::Hive => hive::attack_passive(u, id, target, rotate),
        Module::Sentinel => sentinel::attack_passive(u, id, target, rotate),
        Module::CloakedFighter => cloaker::attack_passive(u, id, target, rotate),
        Module::IonArray => cloaker::ion_array_attack_passive(u, id, target, rotate),
        Module::Processor => processor::attack_passive(u, id, target),
        Module::Carrier => crate::attack::attack_simple(u, id, target),
        // RKOAttackPassive.
        Module::Mothership => crate::attack::attack_simple(u, id, target),
        _ => return false,
    }
    true
}

/// Slot 21, `CustShipAttack`, for module routines (false: shared routine).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) -> bool {
    let Some(m) = module(u, id) else { return false };
    match m {
        Module::Acv => acv::attack(u, id, target),
        Module::Mimic => mimic::attack(u, id, target),
        Module::Mcv => mimic::mcv_attack(u, id, target),
        Module::P1StandardCorvette => mimic::p1_attack(u, id, target),
        Module::Ramming => ramming::attack(u, id, target),
        Module::Hive => hive::attack(u, id, target),
        Module::Processor => processor::attack(u, id, target),
        Module::Sentinel => sentinel::attack(u, id, target),
        Module::Mothership => mothership::attack(u, id, target),
        Module::CloakedFighter => cloaker::attack(u, id, target),
        Module::IonArray => cloaker::ion_array_attack(u, id, target),
        Module::Dfg => dfg::attack(u, id, target),
        Module::CruiseMissile => missile_ships::cruise_missile_attack(u, id, target),
        Module::MissileCorvette => missile_ships::corvette_attack(u, id, target),
        Module::Minelayer => minelayer::attack(u, id, target),
        Module::GravWell => gravwell::attack(u, id, target),
        Module::NomadMoon => nomad::attack(u, id, target),
        _ => return false,
    }
    true
}

/// Whether the type has a special-activate routine (`CustShipSpecialActivate` set).
pub fn has_special_activate(t: ShipType) -> bool {
    use ShipType::*;
    match module_of(t) {
        Module::Acolyte | Module::Acv | Module::Mimic | Module::Mcv | Module::P1StandardCorvette | Module::Ramming
        | Module::Hive | Module::Dreadnought | Module::Sentinel | Module::Mothership | Module::CloakedFighter
        | Module::IonArray | Module::Minelayer | Module::GravWell | Module::Carrier => true,
        _ => matches!(t, sRecon | bRecon | bInterceptor | bAttackBomber | kAttackBomber | kHeavyInterceptor | tAttackBomber | tHeavyInterceptor | p1Fighter),
    }
}

/// Whether the type has a special-target routine (`CustShipSpecialTarget` set).
pub fn has_special_target(t: ShipType) -> bool {
    matches!(
        module_of(t),
        Module::Acolyte
            | Module::Acv
            | Module::Mimic
            | Module::Mcv
            | Module::P1StandardCorvette
            | Module::Leech
            | Module::Ramming
            | Module::Hive
            | Module::Dreadnought
            | Module::Carrier
            | Module::Worker
            | Module::Sentinel
            | Module::Mothership
            | Module::HeavyCruiser
            | Module::MissileCorvette
            | Module::MissileDestroyer
            | Module::SalCap
            | Module::BeastMothership
            | Module::HeavyCorvette
    ) || matches!(t, ShipType::kAdvanceSupportFrigate | ShipType::tAdvanceSupportFrigate | ShipType::tResourceCollector)
}

/// Slot 7, `CustShipSpecialActivate`. Returns whether the special is done (types without the
/// routine are done at once).
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let Some(m) = module(u, id) else { return true };
    match m {
        Module::Acolyte => acolyte::special_activate(u, id),
        Module::Acv => acv::special_activate(u, id),
        Module::Mimic => mimic::special_activate(u, id),
        Module::Mcv | Module::P1StandardCorvette => mimic::mcv_special_activate(u, id),
        Module::Ramming => ramming::special_activate(u, id),
        Module::Hive => hive::special_activate(u, id),
        Module::Dreadnought => dreadnought::special_activate(u, id),
        Module::Sentinel => true,
        Module::Mothership => mothership::special_activate(u, id),
        Module::CloakedFighter | Module::IonArray => cloaker::special_activate(u, id),
        Module::Minelayer => minelayer::special_activate(u, id),
        Module::GravWell => gravwell::special_activate(u, id),
        _ => true,
    }
}

/// Slot 8, `CustShipSpecialTarget`. Returns whether the special is done.
pub fn special_target(u: &mut Universe, id: ShipId, cid: crate::object::CommandId, targets: &[ShipId]) -> bool {
    let Some(m) = module(u, id) else { return true };
    match m {
        Module::Acolyte => acolyte::special_target(u, id, targets),
        Module::HeavyCorvette => heavy_corvette::special_target(u, id, targets),
        Module::Acv => acv::special_target(u, id, targets),
        Module::Mimic => mimic::special_target(u, id, targets),
        Module::Mcv => mimic::mcv_special_target(u, id, targets),
        Module::P1StandardCorvette => mimic::p1_special_target(u, id, cid, targets),
        Module::Leech => leech::special_target(u, id, targets),
        Module::Ramming => ramming::special_target(u, id, targets),
        Module::Hive => hive::special_target(u, id, targets),
        Module::Dreadnought => true,
        Module::Sentinel => true,
        Module::Mothership | Module::HeavyCruiser | Module::BeastMothership => beast::special_target(u, id, targets),
        Module::Carrier => beast::carrier_special_target(u, id, targets),
        Module::MissileCorvette | Module::MissileDestroyer => missile_ships::special_target(u, id, targets),
        Module::Worker if worker_repair::is_repairing(u, id) => worker_repair::special_target(u, id, cid, targets),
        Module::Worker | Module::SalCap => crate::salvage::special_target_pub(u, id, cid, targets),
        _ => true,
    }
}

/// Slot 9, `CustShipOrderChanged`: the ship's order was replaced.
pub fn order_changed(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Leech => leech::order_changed(u, id),
        Module::HeavyCorvette => heavy_corvette::order_changed(u, id),
        Module::P1StandardCorvette => mimic::p1_order_changed(u, id),
        Module::Hive => hive::order_changed(u, id),
        Module::Minelayer => minelayer::order_changed(u, id),
        Module::Mothership | Module::HeavyCruiser | Module::BeastMothership | Module::Carrier => beast::order_changed(u, id),
        Module::Worker => worker_repair::order_changed(u, id),
        _ => {}
    }
}

/// Slot 10, `CustShipHousekeep`, once per update after the physics of all ships.
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acolyte => acolyte::housekeep(u, id),
        Module::Acv => acv::housekeep(u, id),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => mimic::housekeep(u, id),
        Module::Leech => leech::housekeep(u, id),
        Module::Ramming => ramming::housekeep(u, id),
        Module::Hive => hive::housekeep(u, id),
        Module::Dreadnought => dreadnought::housekeep(u, id),
        Module::ShipModule => crate::modules::housekeep(u, id),
        Module::Processor => processor::housekeep(u, id),
        Module::Sentinel => sentinel::housekeep(u, id),
        Module::Mothership => mothership::housekeep(u, id),
        Module::CloakedFighter | Module::IonArray => cloaker::housekeep(u, id),
        Module::Dfg => dfg::housekeep(u, id),
        Module::HeavyCruiser | Module::Carrier | Module::BeastMothership => beast::housekeep(u, id),
        Module::Worker => worker_repair::housekeep(u, id),
        Module::MissileCorvette | Module::MissileDestroyer => missile_ships::housekeep(u, id),
        Module::Minelayer => minelayer::housekeep(u, id),
        Module::GravWell => gravwell::housekeep(u, id),
        Module::ProximitySensor => proximity::housekeep(u, id),
        Module::NomadMoon => nomad::housekeep(u, id),
        Module::Slipgate => slipgate::housekeep(u, id),
        Module::HeavyCorvette => heavy_corvette::housekeep(u, id),
        _ => {}
    }
    if u.ships.get(id).is_some_and(|s| s.base.depot.is_some()) {
        crate::salvage::depot_housekeep(u, id);
    }
}

/// Slot 11, `CustShipRemoveShipReferences`: `gone` is leaving the universe.
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acolyte => acolyte::remove_ship_refs(u, id, gone),
        Module::Acv => acv::remove_ship_refs(u, id, gone),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => mimic::remove_ship_refs(u, id, gone),
        Module::Leech => leech::remove_ship_refs(u, id, gone),
        Module::Ramming => ramming::remove_ship_refs(u, id, gone),
        Module::Hive => hive::remove_ship_refs(u, id, gone),
        Module::Swarmer => hive::swarmer_remove_ship_refs(u, id, gone),
        Module::Dreadnought => dreadnought::remove_ship_refs(u, id, gone),
        Module::Processor => processor::remove_ship_refs(u, id, gone),
        Module::Sentinel => sentinel::remove_ship_refs(u, id, gone),
        Module::Mothership => {
            mothership::remove_ship_refs(u, id, gone);
            beast::remove_ship_refs(u, id, gone);
        }
        Module::HeavyCruiser | Module::Carrier | Module::BeastMothership => beast::remove_ship_refs(u, id, gone),
        Module::Worker => worker_repair::remove_ship_refs(u, id, gone),
        Module::GravWell => gravwell::remove_ship_refs(u, id, gone),
        Module::NomadMoon => nomad::remove_ship_refs(u, id, gone),
        Module::Slipgate => slipgate::remove_ship_refs(u, id, gone),
        _ => {}
    }
}

/// Slot 12: the references dropped when `gone` docks (most modules reuse slot 11).
pub fn remove_ship_refs_docked(u: &mut Universe, id: ShipId, gone: ShipId) {
    remove_ship_refs(u, id, gone);
}

/// Slot 13: target and effect references (ramming target, processor beams, big-gun target).
pub fn remove_target_ref(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Ramming => ramming::remove_target_ref(u, id, gone),
        Module::Processor => processor::stop_beam_on_ship(u, id, gone),
        Module::Mothership => mothership::remove_big_gun_target(u, id, gone),
        _ => {}
    }
}

/// Slot 14: a bullet is freed; the Beast cascades drop their references to it.
pub fn cascade_remove_refs(u: &mut Universe, id: ShipId, bullet: crate::object::BulletId) {
    if matches!(module(u, id), Some(Module::HeavyCruiser | Module::Carrier | Module::Mothership | Module::BeastMothership)) {
        beast::cascade_remove_refs(u, id, bullet);
    }
}

/// `univRemoveBulletRefsFromSpecialShips` / the slot-14 loop of `univFreeBulletContents`:
/// every ship hears that `bullet` is going.
pub fn bullet_freed(u: &mut Universe, bullet: crate::object::BulletId) {
    let ships = u.ship_list.clone();
    for s in ships {
        if u.ships.get(s).is_some_and(|x| x.beast.cascade.is_some()) {
            cascade_remove_refs(u, s, bullet);
        }
    }
}

/// Slot 15, `CustShipDied`.
pub fn died(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acv => acv::close(u, id),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => mimic::died(u, id),
        Module::Leech => leech::died(u, id),
        Module::Ramming => ramming::died(u, id),
        Module::Hive => hive::died(u, id),
        Module::Dreadnought => dreadnought::died(u, id),
        Module::Processor => processor::died(u, id),
        Module::Mothership => {
            mothership::died(u, id);
            beast::died(u, id);
        }
        Module::HeavyCruiser | Module::Carrier | Module::BeastMothership => beast::died(u, id),
        Module::Worker => worker_repair::died(u, id),
        Module::Minelayer => minelayer::died(u, id),
        Module::GravWell => gravwell::died(u, id),
        Module::Crystal => crystal::died(u, id),
        _ => {}
    }
}

/// Slots 20, 22 and 27: the special is cancelled (a new order, the cancel key).
pub fn cancel_special(u: &mut Universe, id: ShipId) {
    let Some(m) = module(u, id) else { return };
    match m {
        Module::Acolyte => acolyte::cancel_link_attack(u, id),
        Module::Acv => acv::cancel_emp_target(u, id),
        Module::Mimic | Module::Mcv | Module::P1StandardCorvette => mimic::cancel_disguise(u, id),
        Module::Ramming => {
            ramming::cancel_ram(u, id);
        }
        Module::Hive => hive::cancel_special(u, id),
        Module::Mothership => mothership::cancel_big_gun(u, id),
        _ => {}
    }
}

/// Slot 34, `clFormation`'s per-ship hook: the ship is put into a formation of
/// `formation_type`. Returns whether it joins (types without the hook always do).
pub fn formation_changed(u: &mut Universe, id: ShipId, formation_type: u32) -> bool {
    if module(u, id) == Some(Module::Sentinel) {
        return sentinel::formation_changed(u, id, formation_type);
    }
    true
}

/// Slot 20, `clMove`'s per-ship hook. Returns whether the ship takes the move order (types
/// without the hook always do).
pub fn move_hook(u: &mut Universe, id: ShipId) -> bool {
    match module(u, id) {
        Some(Module::Ramming) => ramming::cancel_ram(u, id),
        Some(Module::Hive) => hive::move_hook(u, id),
        _ => true,
    }
}

/// Slot 22, `clDock`'s per-ship hook (`dw`: the ship to dock with, if given). Returns whether
/// the ship takes the dock order.
pub fn dock_hook(u: &mut Universe, id: ShipId, dw: Option<ShipId>) -> bool {
    match module(u, id) {
        Some(Module::Ramming) => ramming::cancel_ram(u, id),
        Some(Module::Hive) => hive::dock_hook(u, id, dw),
        _ => true,
    }
}

/// Slot 27, `clHalt`'s per-ship hook. Returns whether the ship takes the halt order.
pub fn halt_hook(u: &mut Universe, id: ShipId) -> bool {
    match module(u, id) {
        Some(Module::Ramming) => ramming::cancel_ram(u, id),
        Some(Module::Hive) => hive::halt_hook(u, id),
        _ => true,
    }
}

/// What a ship does when an EMP first shocks it (`ShipApplyEMP`): a leech lets go, a rammer
/// stops pushing, a salvager checks whether it was docked.
pub fn emp_shocked(u: &mut Universe, id: ShipId) {
    match module(u, id) {
        Some(Module::Leech) => leech::emp_shocked(u, id),
        Some(Module::Ramming) => ramming::stop_pushing(u, id),
        _ => {}
    }
}

/// `processSpecialToDo` (0x43d3a0): without targets every ship's special-activate routine,
/// with targets every ship's special-target routine; done when all are.
pub fn process_special_to_do(u: &mut Universe, cid: crate::object::CommandId) -> bool {
    let Some(cmd) = u.commands.get(cid).cloned() else { return true };
    let mut done = true;
    for &s in &cmd.selection {
        let Some(t) = u.ships.get(s).map(|x| x.ship_type) else { continue };
        let targets = u.commands.get(cid).map(|c| c.special_targets.clone()).unwrap_or_default();
        if targets.is_empty() {
            if has_special_activate(t) {
                done &= special_activate(u, s);
            }
        } else if has_special_target(t) {
            done &= special_target(u, s, cid, &targets);
        }
    }
    done
}

/// `clSpecial` (0x444d90) via `clWrapSpecial`: without targets, ships whose special is not
/// continuous trigger it at once and leave the selection; the rest (or every ship, with
/// targets) get a special order.
pub fn cl_special(u: &mut Universe, ships: &[ShipId], targets: &[ShipId]) {
    let statics = u.statics.clone();
    let mut sel: Vec<ShipId> = ships.iter().copied().filter(|&s| u.ships.get(s).is_some()).collect();
    if sel.is_empty() {
        return;
    }
    if sel.iter().all(|&s| crate::salvage::is_salvager(u.ships.ship(s).ship_type)) && !targets.is_empty() {
        crate::salvage::cl_salvage(u, &sel, targets);
        return;
    }
    if targets.is_empty() {
        for &s in ships {
            let Some(t) = u.ships.get(s).map(|x| x.ship_type) else { continue };
            let continuous = statics.ship(t).is_some_and(|st| st.special_activate_is_continuous);
            if !continuous {
                sel.retain(|&x| x != s);
                if has_special_activate(t) {
                    special_activate(u, s);
                }
            }
        }
        if sel.is_empty() {
            return;
        }
    }
    crate::command::remove_ships_from_doing_stuff(u, &sel);
    let mut c = crate::command::new_command_pub(sel.clone());
    c.order = crate::command::order::SPECIAL;
    c.special_targets = targets.to_vec();
    let cid = u.commands.add_pub(c);
    for &s in &sel {
        let sh = u.ships.ship_mut(s);
        sh.command = Some(cid);
        if sh.special_flags & crate::object::special::ROW_GETTING_OUT_OF_WAY != 0 {
            crate::aiship::row_remove_ship(sh);
        }
    }
    for &s in &sel {
        if u.ships.ship(s).ship_type == ShipType::sLeech {
            leech::special_order_given(u, s);
        }
    }
}

/// Presentation events of the ship modules (for the renderer and sound).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Effect {
    /// An ACV's EMP went off with this radius.
    EmpBlast { radius: f32 },
    /// A force field took a hit.
    FieldHit,
    /// A beam (repair, leech drain, cutting beam) is on towards `target`.
    Beam { target: ShipId },
    /// A ship's special visual state changed (disguise, cascade, charge...).
    State { state: u32 },
    /// The Dreadnought cannon or the big gun fired.
    BigGunFired,
}
