//! Speech the ships and the command layer ask for (`speechEventQueue` 0x51d680 calls made
//! with a ship, or with none for the mothership's voice). The simulation only notes them in
//! [`Universe::speech`]; the front end, which owns the speech queue, the voice options and
//! the streams, plays them and decides which to drop.
//!
//! Cataclysm's ship voice events carry the speaker flag 0x800 and, for orders, the command
//! type flag 0x4000000; the variable is the speaking ship's type (`ship+0x2b0`), so the line
//! names the ship ("Acolytes moving").

use hwc_math::fpu::{ld, st};

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
    /// A kamikaze order with no ship attacking (`kbcbKamikaze`: `COMM_Kamikaze_NoTargets`).
    pub const KAMIKAZE_NO_TARGETS: u32 = 0x400_0809;
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
    /// Ships told to kamikaze (`kbcbKamikaze`).
    pub const KAMIKAZE: u32 = 0x400_08a7;
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
    /// Fleet command (speaker flag 0x2000) asking to confirm a scuttle order (`mrScuttle`'s
    /// first press: `COMM_F_ScuttleRequest`).
    pub const SCUTTLE_REQUEST: u32 = 0x400_2088;
}

/// A speech event to queue: `ship` speaks (`None`: the mothership's voice with no ship),
/// with `variable` choosing the line. `link` is the handle of the event it answers: it plays
/// after that one, on its stream, and not at all if that one was dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Speech {
    pub ship: Option<ShipId>,
    pub event: u32,
    pub variable: i32,
    pub handle: u32,
    pub link: Option<u32>,
    /// The battle chatter event (`BCE_*`) it says, for battle chatter.
    pub chatter: Option<usize>,
}

/// What `speechEventQueue`'s refusals need from the front end, handed over before each
/// update ([`Universe::speech_gate`]; `None`, as in tests, refuses nothing).
#[derive(Debug, Clone, Default)]
pub struct SpeechGate {
    /// The Voice options: chatter (0x88b3f4), status (0x88b3f0), commands (0x88b3ec).
    pub chatter: bool,
    pub status: bool,
    pub commands: bool,
    /// Speech is off or the sound paused (`!enableSpeech` 0x8ae2c0, 0x910254).
    pub off: bool,
    /// `numSinglePlayerEvents` (0xaaeb50) > 0: a mission line plays, so no chatter.
    pub single_player_speaking: bool,
    /// `soundEventDuckSFX` (0x9102ec): no status or chatter while the effects are ducked.
    pub ducked: bool,
    /// The ships in the render list (`univSpaceObjInRenderList`): only they report status or
    /// chatter.
    pub in_view: std::collections::BTreeSet<ShipId>,
    /// The hot-key group of each grouped ship (`ship+0x43e`, 1..=9).
    pub groups: std::collections::BTreeMap<ShipId, u8>,
    /// The main screen is drawn and no ship is within 5000 of the camera (`mrRenderMainScreen`
    /// 0x8afb4c, the sound's closest distance 0x9102a4).
    pub far_from_camera: bool,
}

/// `SPEECH_TYPE_*` bits.
pub mod kind {
    pub const CHATTER: u32 = 0x100_0000;
    pub const STATUS: u32 = 0x200_0000;
    pub const COMMAND: u32 = 0x400_0000;
    pub const ALWAYS_PLAY: u32 = 0x8000_0000;
}

/// `speechEventQueue(ship, event, variable, ...)`.
pub fn queue(u: &mut Universe, ship: Option<ShipId>, event: u32, variable: i32) {
    queue_linked(u, ship, event, variable, None);
}

/// `speechEventQueue(ship, event, variable, ..., linkTo, ...)` with the refusals the
/// simulation can see ([`SpeechGate`]): the handle of the queued event, or `None` (`ERROR`).
/// The front end may still drop it (a full queue, the same line already playing).
pub fn queue_linked(u: &mut Universe, ship: Option<ShipId>, event: u32, variable: i32, link: Option<u32>) -> Option<u32> {
    if !accepts(u, ship, event) {
        return None;
    }
    u.speech_handles = u.speech_handles.wrapping_add(1);
    let handle = u.speech_handles;
    u.speech.push(Speech { ship, event, variable, handle, link, chatter: None });
    Some(handle)
}

fn accepts(u: &Universe, ship: Option<ShipId>, event: u32) -> bool {
    use kind::*;
    if (event & 0x7ff) > 0x35e || u.nis_running && event & 0x6000_0000 == 0 {
        return false;
    }
    let Some(g) = &u.speech_gate else { return true };
    let refused = g.off
        || g.single_player_speaking && event & CHATTER != 0
        || g.ducked && event & (CHATTER | STATUS) != 0
        || event & CHATTER != 0 && !g.chatter
        || event & STATUS != 0 && !g.status
        || event & COMMAND != 0 && !g.commands;
    if refused {
        return false;
    }
    let Some(id) = ship else { return true };
    if event & 0x1000 != 0 {
        return true;
    }
    let Some(s) = u.ships.get(id) else { return false };
    if !g.in_view.contains(&id) && event & (STATUS | CHATTER) != 0 && event & ALWAYS_PLAY == 0 {
        return false;
    }
    // Other players' ships speak only through a Mimic's disguise (selection), which the
    // order acknowledgements handle; silent ships (`specialFlags2 & 0x100008`) never.
    (s.player == u.chatter.local_player || u.nis_running) && s.special_flags2 & 0x0010_0008 == 0
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

/// A line for the local player's message panel (`moMessageAdd(string, colour)`): `key` names
/// the string in the language's string table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub key: &'static str,
    pub colour: [u8; 4],
}

/// Fleet command's enemy-sighting warnings (status, speaker flag 0x1000).
pub mod sighting {
    /// A cloaked ship sighted (0xaaeb60).
    pub const CLOAKED: u32 = 0x200_104c;
    /// A disguised Mimic sighted (0xaaeb64).
    pub const MIMIC: u32 = 0x200_10ba;
    /// An enemy Leech on one of the player's ships (0xaaeb54).
    pub const LEECH: u32 = 0x200_10bb;
}

/// The sighting flags waiting to be said and when the last warning was (0xaaeb54..0xaaeb68,
/// cleared by `speechEnemySightingsReset` at the level reset).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sightings {
    pub cloaked: bool,
    pub mimic: bool,
    pub leech: bool,
    pub last: f32,
}

/// `speechEnemyShipSighted` (0x51dce0): `player` (only the local player counts) sees an
/// enemy stealth ship: a disguised Mimic or MCV (or the P1 standard corvette), a cloaker, or
/// a Leech that is leeching something.
pub fn enemy_ship_sighted(u: &mut Universe, ship: ShipId, player: usize) {
    use crate::statics::ShipType as T;
    if player != u.chatter.local_player {
        return;
    }
    let Some(s) = u.ships.get(ship) else { return };
    if u.players.get(player).is_some_and(|p| p.is_ally(s.player)) {
        return;
    }
    match s.ship_type {
        T::sMimic | T::sMCV | T::p1StandardCorvette => u.sightings.mimic = true,
        T::sLeech => {
            if crate::ships::leech::is_active(u, ship) && crate::ships::leech::target(u, ship).is_some() {
                u.sightings.leech = true;
            }
        }
        T::bCloakedFighter | T::bIonArrayFrigate | T::p1IonArrayFrigate => u.sightings.cloaked = true,
        _ => {}
    }
}

/// The Leech check of `soundEventUpdate` (0x517e30): an active Leech in the render list whose
/// target is below a fifth of its health is reported to the target's owner. It runs only with
/// a front end that plays sound (a [`SpeechGate`] that is not off).
pub fn leeches_in_view(u: &mut Universe) {
    let Some(g) = &u.speech_gate else { return };
    if g.off {
        return;
    }
    let leeches: Vec<ShipId> = g
        .in_view
        .iter()
        .copied()
        .filter(|&id| u.ships.get(id).is_some_and(|s| s.ship_type == crate::statics::ShipType::sLeech))
        .collect();
    for id in leeches {
        if !crate::ships::leech::is_active(u, id) {
            continue;
        }
        let Some(t) = crate::ships::leech::target(u, id).and_then(|t| u.ships.get(t)) else { continue };
        let max = u.statics.ship(t.ship_type).map_or(0.0, |x| x.health.max_health);
        if st(ld(t.health) / ld(max)) < 0.2 {
            let owner = t.player;
            enemy_ship_sighted(u, id, owner);
        }
    }
}

/// `speechEnemySightingsUpdate` (0x51de00): at most one round of warnings a minute, each
/// pending flag said once by fleet command.
pub fn sightings_update(u: &mut Universe) {
    let now = u.total_time_elapsed;
    if !(u.sightings.last + 60.0 < now) {
        return;
    }
    for (pending, event) in [(u.sightings.cloaked, sighting::CLOAKED), (u.sightings.mimic, sighting::MIMIC), (u.sightings.leech, sighting::LEECH)] {
        if pending {
            queue(u, None, event, -1);
            u.sightings.last = now;
        }
    }
    let s = &mut u.sightings;
    if s.last == now {
        s.cloaked = false;
        s.mimic = false;
        s.leech = false;
    }
}
