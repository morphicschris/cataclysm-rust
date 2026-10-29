//! Speech the ships and the command layer ask for (`speechEventQueue` 0x51d680 calls made
//! with a ship, or with none for the mothership's voice). The simulation only notes them in
//! [`Universe::speech`]; the front end, which owns the speech queue, the voice options and
//! the streams, plays them and decides which to drop.
//!
//! Cataclysm's ship voice events carry the speaker flag 0x800 and, for orders, the command
//! type flag 0x4000000; the variable is the speaking ship's type (`ship+0x2b0`), so the line
//! names the ship ("Acolytes moving").

use crate::object::ShipId;
use crate::universe::Universe;

/// The ship voice events the orders use (`speechEventQueue(ship, event, ship->shiptype)`).
pub mod event {
    /// Ships selected (`mrObjectClick`, `mrBandBoxRelease`, `moProcessClick`; with no ship,
    /// the mothership's voice).
    pub const SELECTED: u32 = 0x400_0800;
    /// Move order (`mrMoveShipsExecute`, `mrWaypointMoveExecute`, `clWrapDock` to a slipgate).
    pub const MOVE: u32 = 0x400_0801;
    /// A move order cancelled (`clHalt` on a moving command).
    pub const MOVE_CANCELLED: u32 = 0x400_0802;
    /// Dock order (`clWrapDock`).
    pub const DOCK: u32 = 0x400_0803;
    /// Attack order on enemies (`speechEventAttack`).
    pub const ATTACK: u32 = 0x400_0805;
    /// An attack order cancelled (`clHalt` on an attacking command).
    pub const ATTACK_CANCELLED: u32 = 0x400_0806;
    /// A forced attack on the player's own ships (`speechEventAttack`).
    pub const ATTACK_OWN_SHIPS: u32 = 0x400_0807;
    /// A forced attack on resources (`speechEventAttack`).
    pub const ATTACK_RESOURCES: u32 = 0x400_0808;
    /// Tactics set to evasive, neutral, aggressive (`mrSetTactics`).
    pub const TACTICS_EVASIVE: u32 = 0x400_080d;
    pub const TACTICS_NEUTRAL: u32 = 0x400_080e;
    pub const TACTICS_AGGRESSIVE: u32 = 0x400_080f;
    /// Too few sentinels for a field, or an EMP with nothing in range (`mrProtectShips`,
    /// `ACVFireEMP`).
    pub const NOT_ENOUGH: u32 = 0x400_0810;
    /// Cloaking on, not enough power to cloak, decloaking (`cloakToggle`).
    pub const CLOAK_ON: u32 = 0x400_0889;
    pub const CLOAK_NO_POWER: u32 = 0x400_088b;
    pub const DECLOAK: u32 = 0x400_088d;
    /// Sentinels forming a field (`mrProtectShips`).
    pub const FORCE_FIELD: u32 = 0x400_088f;
    /// A worker sent to salvage (`clPlaySpeechEventsForSupportCraft`).
    pub const SUPPORT: u32 = 0x400_0896;
    /// Ships linking, unlinking (`dockLinkShipsBegin`, `dockUnlinkShipBegin`).
    pub const LINK: u32 = 0x400_089c;
    pub const UNLINK: u32 = 0x400_089d;
    /// A gravity well switched on (`GravWellGeneratorSpecialActivate`).
    pub const GRAVWELL_ON: u32 = 0x400_089e;
    /// Harvest ordered with nothing to harvest, harvest ordered (`FUN_00445260`,
    /// `mrHarvestResources`).
    pub const HARVEST_NOTHING: u32 = 0x400_08a2;
    pub const HARVEST: u32 = 0x400_08a3;
    /// A mimic taking on, dropping its disguise (`MimicHouseKeep`).
    pub const DISGUISE: u32 = 0x400_08a5;
    pub const UNDISGUISE: u32 = 0x400_08a6;
    /// A hive frigate's new order, its swarmers recalled (`HiveFrigateOrderChanged`,
    /// `HiveFrigateSpecialActivate` and its move and dock hooks).
    pub const HIVE_ORDER: u32 = 0x400_08a9;
    pub const HIVE_RECALL: u32 = 0x400_08aa;
    /// The dreadnought's cannon charging (`DreadnoughtSpecialActivate`).
    pub const CANNON_CHARGING: u32 = 0x400_08ac;
    /// A leech's drain mode: resources, health, hold on (`LeechSetState` 7, 8, 6).
    pub const LEECH_DRAIN_RUS: u32 = 0x400_08af;
    pub const LEECH_DRAIN_HEALTH: u32 = 0x400_08b0;
    pub const LEECH_HOLD: u32 = 0x400_08b1;
    /// ACVs firing their EMP (`ACVFireEMP`).
    pub const EMP_FIRED: u32 = 0x400_08bd;
    /// The mothership's voice (speaker flag 0x1000): a guard order (`mrBandBoxRelease`), a
    /// formation (`mrSetTheFormation`; the variable is the formation), retiring ships
    /// (`clWrapDock`).
    pub const GUARD: u32 = 0x400_1061;
    pub const FORMATION_SET: u32 = 0x400_1062;
    pub const RETIRE: u32 = 0x400_106a;
}

/// A speech event to queue: `ship` speaks (`None`: the mothership's voice with no ship),
/// with `variable` choosing the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Speech {
    pub ship: Option<ShipId>,
    pub event: u32,
    pub variable: i32,
}

/// `speechEventQueue(ship, event, variable, ...)`.
pub fn queue(u: &mut Universe, ship: Option<ShipId>, event: u32, variable: i32) {
    u.speech.push(Speech { ship, event, variable });
}

/// `speechEventQueue(ship, event, ship->shiptype, ...)`: the ship names its type.
pub fn by_ship(u: &mut Universe, ship: ShipId, event: u32) {
    let Some(t) = u.ships.get(ship).map(|s| s.ship_type.index() as i32) else { return };
    queue(u, Some(ship), event, t);
}

/// The selection's first ship speaks for it (most orders: `selection->ShipPtr[0]`).
pub fn by_first(u: &mut Universe, selection: &[ShipId], event: u32) {
    if let Some(&id) = selection.first() {
        by_ship(u, id, event);
    }
}
