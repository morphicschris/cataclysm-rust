//! Battle chatter (`battleCanChatterAtThisTime` 0x4238d0, `battleChatterAttempt` 0x4237b0):
//! speech the ships say now and then rather than every time, rolled on the battle chatter
//! random stream (8) so that it never changes the game.
//!
//! Each event of the table at 0x7448d0 (0x58 bytes each) has its tunables, which
//! `BattleChatter.script` overwrites ([`ChatterTable`], loaded with the statics), and the time
//! and place it was last said ([`ChatterState`], the original's globals: not saved).
//!
//! The events are said by the ships (`battleChatterAttempt`) or by fleet command and fleet
//! intelligence (`battleChatterFleetAttempt`). In single-player games the events the script
//! marks `battleStatusSuppress` (fleet intelligence's battle reports) are counted as said but
//! not queued; [`ChatterTable::status_reports`] (an enhancement, off by default) lets them
//! through. The battle-ping evaluation that says most of them is [`crate::battle_status`].

use hwc_formats::script::ScriptFile;
use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::RAN_BATTLE;
use hwc_math::Vector;

use crate::object::ShipId;
use crate::universe::Universe;

/// Battle chatter events (`BCE_*`, the table index).
pub mod bce {
    pub const WINGMAN: usize = 0x00;
    pub const POSITIVE_BATTLE: usize = 0x01;
    pub const POSITIVE_BATTLE_MORE: usize = 0x02;
    pub const NEGATIVE_BATTLE: usize = 0x03;
    pub const NEGATIVE_BATTLE_MORE: usize = 0x04;
    pub const NEGATIVE_BATTLE_MORE2: usize = 0x05;
    pub const NEUTRAL_BATTLE: usize = 0x06;
    pub const NEUTRAL_BATTLE_MORE: usize = 0x07;
    pub const FRIENDLY_FIRE: usize = 0x08;
    pub const GROUP_FRIENDLY_FIRE: usize = 0x09;
    pub const WINGMAN_CHASED: usize = 0x0a;
    pub const WINGMAN_CHASED_RSP: usize = 0x0b;
    pub const WINGMAN_HIT: usize = 0x0c;
    pub const WINGMAN_HIT_RSP: usize = 0x0d;
    pub const BEING_BEASTIFIED: usize = 0x0e;
    pub const WINGMAN_DIES: usize = 0x0f;
    pub const LEADER_CHASED: usize = 0x10;
    /// `COMM_LInt_Kamikaze`: a kamikaze closing in (`doKamikazeAttack` state 5).
    pub const KAMIKAZE: usize = 0x11;
    pub const START_BATTLE_DISADVANT: usize = 0x12;
    pub const START_BATTLE_DISADVANT_RSP: usize = 0x13;
    pub const START_BATTLE_FAIR: usize = 0x14;
    pub const START_BATTLE_FAIR_RSP: usize = 0x15;
    pub const START_BATTLE_ADVANTAGED: usize = 0x16;
    pub const START_BATTLE_ADVANTAGED_RSP: usize = 0x17;
    pub const RETREAT: usize = 0x18;
    pub const RETREAT_RSP: usize = 0x19;
    pub const WINNING_BATTLE: usize = 0x1a;
    pub const WINNING_BATTLE_RSP: usize = 0x1b;
    pub const KICKING_BUTT: usize = 0x1c;
    pub const KICKING_BUTT_RSP: usize = 0x1d;
    pub const BATTLE_WON: usize = 0x1e;
    pub const GROUP_VICTORY: usize = 0x1f;
    pub const LOSING_BATTLE: usize = 0x20;
    pub const LOSING_BATTLE_MORE: usize = 0x21;
    pub const LOSING_BATTLE_RSP: usize = 0x22;
    pub const LOSING_BADLY: usize = 0x23;
    pub const LOSING_BADLY_MORE: usize = 0x24;
    pub const LOSING_BADLY_RSP: usize = 0x25;
    /// `COMM_CannotComply`: an order the selection refuses (scuttling a mothership or carrier).
    pub const CANNOT_COMPLY: usize = 0x26;
    pub const GRAV_WELL_HAS_SHIPS: usize = 0x27;
    pub const GRAV_WELL_OFF: usize = 0x28;
    pub const STRIKE_ATTACK_COMPLETE: usize = 0x29;
    pub const KAMIKAZE_NO_TARGETS: usize = 0x2a;
    pub const FIGHTER_BURST_ATTACK: usize = 0x2b;
    pub const MINE_LAYER_FORCE_DROP: usize = 0x2c;
    pub const HIVE_SWARMERS_UNDERWAY: usize = 0x2d;
    pub const RETREAT_SUCCESSFUL: usize = 0x2e;
    pub const GROUP_RETREAT_SUCCESSFUL: usize = 0x2f;
    pub const ENEMY_RETREATED: usize = 0x30;
    pub const GROUP_ENEMY_RETREATED: usize = 0x31;
    pub const SALVAGE_DROP_OFF: usize = 0x32;
    pub const SHIP_STOLEN: usize = 0x33;
    pub const IN_MINE_FIELD: usize = 0x34;
    pub const GROUP_IN_MINE_FIELD: usize = 0x35;
    pub const CLOAKED_SHIPS_DETECTED: usize = 0x36;
    pub const ATTACK_RETALIATE: usize = 0x37;
    pub const ATTACK_RETALIATE_REPEAT: usize = 0x38;
    pub const GROUP_ATTACK_RETALIATE: usize = 0x39;
    pub const GROUP_ATTACK_RETALIATE_REPEAT: usize = 0x3a;
    pub const UNDER_ATTACK_FROM_ALLY: usize = 0x3b;
    pub const GROUP_ATTACK_FROM_ALLY: usize = 0x3c;
    pub const ENEMY_PROBE_DETECTED: usize = 0x3d;
    pub const GRAV_WELL_DETECTED: usize = 0x3e;
    pub const SHIP_DAMAGED: usize = 0x3f;
    pub const CAPITAL_SHIP_DIES: usize = 0x40;
    pub const SHIP_DIES_REPORT: usize = 0x41;
    pub const RAM_COMPLETE: usize = 0x42;
    pub const MOTHERSHIP_LARGE_ATTACK: usize = 0x43;
    pub const MOTHERSHIP_LARGE_ATTACK_RSP: usize = 0x44;
    pub const CAP_SHIP_LAUNCHED: usize = 0x45;
    pub const CAP_SHIP_WELCOMED: usize = 0x46;
    pub const DAMAGED_SPECIFIC_SHIP: usize = 0x47;
    pub const SHIP_DOCKED: usize = 0x48;
    pub const EMP_PULSE_DETECTED: usize = 0x49;
    pub const DOCKING_MORE: usize = 0x4a;
    pub const WORKER_FULL: usize = 0x4b;
    pub const SALVAGE_TARGET_ACQUIRED: usize = 0x50;
    pub const FINISHED_REPAIRS: usize = 0x52;
    pub const RESOURCES_TRANSFERRED: usize = 0x53;
    pub const RAMMER_STARTING_RAM: usize = 0x54;
    /// `COMM_F_ScuttleReaffirm`: the second scuttle key press (`mrScuttle`).
    pub const SCUTTLE_REAFFIRM: usize = 0x55;
    pub const HYPERSPACE_DETECTED: usize = 0x56;
}

/// The table at 0x7448d0: each event's name in `BattleChatter.script` and its speech event.
const EVENTS: [(&str, u32); 88] = [
    ("CHAT_Fighter_Wingman", 0x10008c4),
    ("CHAT_Intel_Grp_PositiveBattle", 0x100207f),
    ("CHAT_Ship_Grp_PositiveBattleMore", 0x1000816),
    ("CHAT_Intel_Grp_NegativeBattle", 0x1002080),
    ("CHAT_Ship_Grp_NegativeBattleMore", 0x1000817),
    ("CHAT_Intel_Grp_NegativeBattleMore2", 0x100206f),
    ("CHAT_Intel_Grp_NeutralBattle", 0x1002081),
    ("CHAT_Intel_Grp_NeutralBattleMore", 0x1002071),
    ("CHAT_Intel_Grp_FriendlyFire", 0x1002082),
    ("STAT_MoShip_AssGrp_FriendlyFire", 0x200105f),
    ("STAT_Fighter_WingmanChased", 0x20008be),
    ("STAT_Fighter_WingmanChasedRsp", 0x20008bf),
    ("STAT_Fighter_WingmanHit", 0x20008c0),
    ("STAT_Fighter_WingmanHitRsp", 0x20008c1),
    ("STAT_Spec_ShipBeingBeastified", 0x10008a1),
    ("STAT_Fighter_WingmanDies", 0x20008c2),
    ("STAT_Fighter_LeaderChased", 0x20008c3),
    ("COMM_LInt_Kamikaze", 0x40008c5),
    ("STAT_Intel_Grp_StartBattleDisadvant", 0x2002077),
    ("STAT_Intel_Grp_StartBattleDisadvantRsp", 0x200206c),
    ("STAT_Intel_Grp_StartBattleFair", 0x2002078),
    ("STAT_Intel_Grp_StartBattleFairRsp", 0x200206d),
    ("STAT_Intel_Grp_StartBattleAdvantaged", 0x2002079),
    ("STAT_Intel_Grp_StartBattleAdvantagedRsp", 0x200206e),
    ("COMM_Intel_Grp_Retreat", 0x4002072),
    ("COMM_Intel_Grp_RetreatRsp", 0x400206b),
    ("STAT_Intel_Grp_WinningBattle", 0x200207a),
    ("STAT_Ship_Grp_WinningBattleRsp", 0x1000812),
    ("STAT_Intel_Grp_KickingButt", 0x200207b),
    ("STAT_Ship_Grp_KickingButtRsp", 0x1000813),
    ("STAT_Intel_Grp_BattleWon", 0x200207c),
    ("STAT_Intel_AssGrp_Victory", 0x2002075),
    ("STAT_Intel_Grp_LosingBattle", 0x200207d),
    ("STAT_Intel_Grp_LosingBattleMore", 0x2002070),
    ("STAT_Ship_Grp_LosingBattleRsp", 0x1000814),
    ("STAT_Intel_Grp_LosingBadly", 0x200207e),
    ("STAT_Ship_Grp_LosingBadlyMore", 0x1000819),
    ("STAT_Ship_Grp_LosingBadlyRsp", 0x1000815),
    ("COMM_CannotComply", 0x4000810),
    ("STAT_MoShip_GravWellHasShips", 0x2001051),
    ("COMM_Spec_GravWellOff", 0x40008a0),
    ("STAT_MoShip_StrikeAttackComplete", 0x200105a),
    ("COMM_Kamikaze_NoTargets", 0x4000809),
    ("COMM_Spec_FighterBurstAttack", 0x400089a),
    ("COMM_Spec_MineLayerForceDrop", 0x400089b),
    ("STAT_Spec_HiveSwarmersUnderway", 0x20008ab),
    ("STAT_Intel_Grp_RetreatSuccessful", 0x2002083),
    ("STAT_MoShip_AssGrp_RetreatSuccessful", 0x200105d),
    ("STAT_Intel_Grp_EnemyRetreated", 0x2002084),
    ("STAT_MoShip_AssGrp_EnemyRetreated", 0x200105e),
    ("STAT_Spec_SalvageDropOff", 0x2000897),
    ("STAT_MoShip_ShipStolen", 0x2001041),
    ("STAT_Intel_Grp_InMineField", 0x2002087),
    ("STAT_MoShip_AssGrp_InMineField", 0x2001060),
    ("STAT_MoShip_CloakedShipsDetected", 0x200104c),
    ("STAT_Intel_Grp_AttackRetaliate", 0x2002086),
    ("STAT_Ship_Grp_AttackRetaliateRepeat", 0x1000818),
    ("STAT_Intel_AssGrp_AttackRetaliate", 0x2002073),
    ("STAT_Intel_AssGrp_AttackRetaliateRepeat", 0x2002074),
    ("STAT_MoShip_UnderAttackFromAlly", 0x2001069),
    ("STAT_Intel_Grp_AttackFromAlly", 0x2002085),
    ("STAT_MoShip_EnemyProbeDetected", 0x200104b),
    ("STAT_MoShip_GravWellDetected", 0x2001050),
    ("STAT_Ship_Damaged", 0x100080b),
    ("STAT_Cap_Dies", 0x100080c),
    ("STAT_MoShip_ShipDiesReport", 0x2001065),
    ("STAT_Spec_RammerRamComplete", 0x20008ae),
    ("STAT_MoShip_MoShipLargeAttack", 0x2001059),
    ("STAT_MoShip_MoShipUnderLargeAttackRsp", 0x2001057),
    ("STAT_MoShip_CapShipLaunched", 0x200103f),
    ("STAT_MoShip_CapShipWelcomed", 0x200101e),
    ("STAT_MoShip_DamagedSpecificShip", 0x2001066),
    ("STAT_Cap_ShipDocked", 0x81000804),
    ("STAT_MoShip_EMPPulseDetected", 0x200104f),
    ("CHAT_Docking_More", 0x81000804),
    ("STAT_Spec_WorkerFull", 0x20008a4),
    ("STAT_Cloak_CloakingOn_Resp", 0x0),
    ("COMM_Cloak_InsufficientPower", 0x0),
    ("STAT_Spec_CloakCloakPowerLow", 0x200088c),
    ("COMM_Spec_CloakDecloak", 0x400088d),
    ("STAT_Spec_SalvageTargetAcquired", 0x2000899),
    ("STAT_RepVette_StartedRepairs", 0x0),
    ("STAT_Spec_FinishedRepairs", 0x2000893),
    ("STAT_Spec_ResourcesTransferred", 0x200088e),
    ("COMM_Spec_RammerStartingRam", 0x40008ad),
    ("COMM_F_ScuttleReaffirm", 0x400080a),
    ("COMM_MoShip_HyperspaceDetected", 0x400104e),
    ("STAT_MoShip_ProxSensorDetection", 0x0),
];

/// The event `battleChatterAttempt` plays on the camera's position instead of a ship.
const FLEET_EVENT: u32 = 0x100_080c;

/// One event's tunables (`battleevent`, Cataclysm's layout: doubles, distances squared).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatterEvent {
    /// The table index ([`bce`]).
    pub index: usize,
    /// Its name in `BattleChatter.script`.
    pub name: &'static str,
    /// The speech event (+0x04).
    pub event: u32,
    /// `randomWeight` (+0x08): the chance of saying it (x the chatter frequency).
    pub random_weight: f64,
    /// `maxDistance` squared (+0x10): how far from the camera it is heard.
    pub max_distance_sq: f64,
    /// `expDistance` (+0x18; not read by Cataclysm's attempt).
    pub exp_distance: f64,
    /// `minWavelength` (+0x20) and `expWavelength` (+0x28): how soon it may be said again.
    pub min_wavelength: f64,
    pub exp_wavelength: f64,
    /// `minRepeatProximity` squared (+0x30) and its exponent (+0x38): how near the last place
    /// it was said a repeat is held back.
    pub min_repeat_proximity_sq: f64,
    pub exp_repeat_proximity: f64,
    /// `battleStatusUpdate` (+0x50): a battle report; none is said within 30 s of the last
    /// (60 s of the same one).
    pub status_update: bool,
    /// `battleStatusSuppress` (+0x54): not queued in single-player games.
    pub status_suppress: bool,
}

impl ChatterEvent {
    /// The compiled tunables (the same for every event).
    const fn compiled(index: usize, name: &'static str, event: u32) -> Self {
        ChatterEvent {
            index,
            name,
            event,
            random_weight: 1.0,
            max_distance_sq: 1_000_000.0,
            exp_distance: 1.0,
            min_wavelength: 10.1,
            exp_wavelength: 1.0,
            min_repeat_proximity_sq: 0.0,
            exp_repeat_proximity: 1.0,
            status_update: false,
            status_suppress: false,
        }
    }
}

/// The events' tunables after `BattleChatter.script`, and the battle evaluation's (`bat*`).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatterTable {
    pub events: Vec<ChatterEvent>,
    /// `batGlobalFrequencyModifier` (0x74674c; the game's chatter option sets it again).
    pub frequency: f32,
    /// `batBogeyAspect` (0x746744): how far off a ship's tail a chaser may be.
    pub bogey_aspect: f32,
    /// `batStandingFriendlyPower`, `..FriendlyValue`, `..EnemyPower`, `..EnemyValue`
    /// (0x746720..0x74672c).
    pub standing: [f32; 4],
    /// `batExpLossDevalue` (0x746730).
    pub exp_loss_devalue: f32,
    /// The battle status number thresholds: kicking butt, winning, neutral, losing
    /// (0x746734..0x746740).
    pub bsn: [f32; 4],
    /// `batRetreatModifier` (0x746748): leaving `radius * this` of the battle is a retreat.
    pub retreat_modifier: f32,
    /// `batMouthpieceTries` (0x74671c).
    pub mouthpiece_tries: i32,
}

impl Default for ChatterTable {
    fn default() -> Self {
        ChatterTable {
            events: EVENTS.iter().enumerate().map(|(i, &(name, event))| ChatterEvent::compiled(i, name, event)).collect(),
            frequency: 128.0,
            bogey_aspect: 0.5,
            standing: [0.5; 4],
            exp_loss_devalue: 2.0,
            bsn: [4.0, 1.5, 0.75, 0.25],
            retreat_modifier: 1.5,
            mouthpiece_tries: 4,
        }
    }
}

/// `battleClassDependencies` (0x746710), per ship class: 1 = says the capital-ship launch
/// welcome, 2 = reports damage.
pub const CLASS_DEPENDENCIES: [u8; 12] = [0, 3, 3, 3, 3, 0, 0, 2, 0, 0, 0, 0];

impl ChatterTable {
    /// The compiled table with `BattleChatter.script` applied (`bat*SetCB`: each value is read
    /// as a float; the distances are stored squared).
    pub fn parse(script: Option<&ScriptFile>) -> Self {
        let mut t = ChatterTable::default();
        let Some(script) = script else { return t };
        for e in script.entries() {
            let key = e.key.as_str();
            if key.eq_ignore_ascii_case("batMouthpieceTries") {
                if let Some(v) = crate::statics::cscan::scan_i32(e.raw.as_str()) {
                    t.mouthpiece_tries = v;
                }
                continue;
            }
            if let Some(f) = t.global_mut(key) {
                if let Some(v) = crate::statics::cscan::scan_f32(e.raw.as_str()) {
                    *f = v;
                }
                continue;
            }
            let mut words = e.raw.split_whitespace();
            let (Some(name), Some(value)) = (words.next(), words.next()) else { continue };
            let Some(ev) = t.events.iter_mut().find(|ev| ev.name.eq_ignore_ascii_case(name)) else { continue };
            let Some(v) = crate::statics::cscan::scan_f32(value) else { continue };
            let squared = st(ld(v) * ld(v)) as f64;
            match key {
                "randomWeight" => ev.random_weight = f64::from(v),
                "maxDistance" => ev.max_distance_sq = squared,
                "expDistance" => ev.exp_distance = f64::from(v),
                "minWavelength" => ev.min_wavelength = f64::from(v),
                "expWavelength" => ev.exp_wavelength = f64::from(v),
                "minRepeatProximity" => ev.min_repeat_proximity_sq = squared,
                "expRepeatProximity" => ev.exp_repeat_proximity = f64::from(v),
                "battleStatusUpdate" => ev.status_update = v != 0.0,
                "battleStatusSuppress" => ev.status_suppress = v != 0.0,
                _ => {}
            }
        }
        t
    }

    /// The `bat*` value a script key sets.
    fn global_mut(&mut self, key: &str) -> Option<&mut f32> {
        Some(match key.to_ascii_lowercase().as_str() {
            "batglobalfrequencymodifier" => &mut self.frequency,
            "batbogeyaspect" => &mut self.bogey_aspect,
            "batstandingfriendlypower" => &mut self.standing[0],
            "batstandingfriendlyvalue" => &mut self.standing[1],
            "batstandingenemypower" => &mut self.standing[2],
            "batstandingenemyvalue" => &mut self.standing[3],
            "batexplossdevalue" => &mut self.exp_loss_devalue,
            "batbsnkickingbutt" => &mut self.bsn[0],
            "batbsnwinning" => &mut self.bsn[1],
            "batbsnneutral" => &mut self.bsn[2],
            "batbsnlosing" => &mut self.bsn[3],
            "batretreatmodifier" => &mut self.retreat_modifier,
            _ => return None,
        })
    }

    pub fn get(&self, index: usize) -> Option<&ChatterEvent> {
        self.events.get(index)
    }
}

/// When and where each event was last said, and what the front end tells the chatter.
#[derive(Debug, Clone, Default)]
pub struct ChatterState {
    /// Per event of [`ChatterTable::events`]: `lastTimeSpoken` (+0x40) and `lastPlaceSpoken`.
    pub last: Vec<(f32, Vector)>,
    /// The chatter frequency option (`battleChatterFrequencySet`: percent / 100); `None`
    /// keeps the script's.
    pub frequency: Option<f32>,
    /// The camera's eye (`mrCamera->eyeposition`; `None`: no camera, everything is heard).
    pub camera_eye: Option<Vector>,
    /// The local player (`universe.curPlayerIndex`): only its ships chatter.
    pub local_player: usize,
    /// When the last battle report was said (0x746750) and which (0x8d3654).
    pub last_report: Option<(f32, usize)>,
    /// An enhancement (`hwcBattleReports`, off by default): fleet intelligence's battle
    /// reports are said in single-player games too (the original keeps them for skirmish).
    pub reports_in_campaign: bool,
}

impl ChatterState {
    fn last_mut(&mut self, slot: usize) -> &mut (f32, Vector) {
        if self.last.len() <= slot {
            self.last.resize(slot + 1, (0.0, Vector::ZERO));
        }
        &mut self.last[slot]
    }
}

fn slot(u: &Universe, index: usize) -> Option<(usize, ChatterEvent)> {
    let t = &u.statics.battle_chatter;
    let e = t.get(index)?;
    Some((t.events.iter().position(|x| x.index == index)?, e.clone()))
}

fn roll(u: &mut Universe) -> f64 {
    f64::from(u.randy.random(RAN_BATTLE) & 0x7fff)
}

/// `battleCanChatterAtThisTime(event, ship)`: an event said within its `minWavelength` is
/// said again only by chance, and less likely near where it was last said.
pub fn can_chatter_at_this_time(u: &mut Universe, index: usize, ship: Option<ShipId>) -> bool {
    let Some((k, e)) = slot(u, index) else { return false };
    let now = u.total_time_elapsed;
    let (last, place) = *u.chatter.last_mut(k);
    let since = st(ld(now) - ld(last));
    let wave = e.min_wavelength as f32;
    if since > wave {
        return true;
    }
    let mut p = f64::from(st(ld(since) / ld(wave)));
    if e.exp_wavelength != 1.0 {
        p = p.powf(e.exp_wavelength);
    }
    if p * 32768.0 <= roll(u) {
        return false;
    }
    if e.min_repeat_proximity_sq > 0.0 {
        let at = match ship.and_then(|id| u.ships.get(id)) {
            Some(s) => s.posinfo.position,
            None => u.chatter.camera_eye.unwrap_or(place),
        };
        let d = Vector::new(st(ld(at.x) - ld(place.x)), st(ld(at.y) - ld(place.y)), st(ld(at.z) - ld(place.z)));
        let dsq = st(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z));
        if dsq < e.min_repeat_proximity_sq as f32 {
            let mut p = ld(dsq) / fsqrt(e.min_repeat_proximity_sq);
            if e.exp_repeat_proximity != 1.0 {
                p = p.powf(e.exp_repeat_proximity);
            }
            if p * 32768.0 <= roll(u) {
                return false;
            }
        }
    }
    true
}

/// `battleChatterAttempt(linkTo, event, ship, variable)`: one of the local player's ships says
/// the event by chance (its `randomWeight` x the chatter frequency) if the camera is near
/// enough; then it counts as said. The handle of the queued speech, or `None`.
pub fn attempt(u: &mut Universe, index: usize, ship: ShipId, variable: i32) -> Option<u32> {
    attempt_linked(u, None, index, ship, variable)
}

/// [`attempt`] answering the speech `link`.
pub fn attempt_linked(u: &mut Universe, link: Option<u32>, index: usize, ship: ShipId, variable: i32) -> Option<u32> {
    let (k, e) = slot(u, index)?;
    let s = u.ships.get(ship)?;
    if s.player != u.chatter.local_player {
        return None;
    }
    let pos = s.posinfo.position;
    let freq = u.chatter.frequency.unwrap_or(u.statics.battle_chatter.frequency);
    let chance = st(ld(freq) * ld(e.random_weight as f32));
    let r = u.randy.random(RAN_BATTLE);
    if fild_u32(r & 0x7fff) >= ld(chance) * 32768.0 {
        return None;
    }
    // The ship's `cameraDistanceSquared` (+0x40).
    let dsq = u.chatter.camera_eye.map_or(0.0, |eye| {
        let d = Vector::new(st(ld(pos.x) - ld(eye.x)), st(ld(pos.y) - ld(eye.y)), st(ld(pos.z) - ld(eye.z)));
        st(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z))
    });
    if dsq >= e.max_distance_sq as f32 {
        return None;
    }
    *u.chatter.last_mut(k) = (u.total_time_elapsed, pos);
    let speaker = if e.event == FLEET_EVENT { None } else { Some(ship) };
    let h = crate::speech::queue_linked(u, speaker, e.event, variable, link);
    mark(u, h, index);
    h
}

/// Notes on the speech just queued (handle `h`) which battle chatter event it says.
fn mark(u: &mut Universe, h: Option<u32>, index: usize) {
    if let (Some(h), Some(s)) = (h, u.speech.last_mut()) {
        if s.handle == h {
            s.chatter = Some(index);
        }
    }
}

/// `battleChatterFleetAttempt(linkTo, event, variable, where)` (0x423a70): fleet command or
/// intelligence says the event. A battle report waits 30 s after the last one (60 s for the
/// same one); then the wavelength and, with `at`, the repeat-proximity curves, and the random
/// weight. In single-player games a `battleStatusSuppress` event counts as said but is not
/// queued.
pub fn fleet_attempt(u: &mut Universe, link: Option<u32>, index: usize, variable: i32, at: Option<Vector>) -> Option<u32> {
    let (k, e) = slot(u, index)?;
    let now = u.total_time_elapsed;
    if e.status_update {
        if let Some((when, last)) = u.chatter.last_report {
            let wait = if last == index { 60.0 } else { 30.0 };
            if now < st(ld(when) + wait) {
                return None;
            }
        }
    }
    let (last, place) = *u.chatter.last_mut(k);
    let since = st(ld(now) - ld(last));
    let wave = e.min_wavelength as f32;
    if since <= wave {
        let mut p = f64::from(st(ld(since) / ld(wave)));
        if e.exp_wavelength != 1.0 {
            p = p.powf(e.exp_wavelength);
        }
        if p * 32768.0 <= roll(u) {
            return None;
        }
    }
    if let Some(at) = at.filter(|_| e.min_repeat_proximity_sq > 0.0) {
        let d = Vector::new(st(ld(at.x) - ld(place.x)), st(ld(at.y) - ld(place.y)), st(ld(at.z) - ld(place.z)));
        let dsq = st(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z));
        if dsq < e.min_repeat_proximity_sq as f32 {
            let mut p = ld(dsq) / fsqrt(e.min_repeat_proximity_sq);
            if e.exp_repeat_proximity != 1.0 {
                p = p.powf(e.exp_repeat_proximity);
            }
            if p * 32768.0 <= roll(u) {
                return None;
            }
        }
    }
    let freq = u.chatter.frequency.unwrap_or(u.statics.battle_chatter.frequency);
    let chance = st(ld(freq) * ld(e.random_weight as f32));
    if (roll(u) as f32) >= st(ld(chance) * 32768.0) {
        return None;
    }
    let slot = u.chatter.last_mut(k);
    slot.0 = now;
    if let Some(at) = at {
        slot.1 = at;
    }
    if e.status_update {
        u.chatter.last_report = Some((now, index));
    }
    if u.single_player && e.status_suppress && !u.chatter.reports_in_campaign {
        return None;
    }
    let h = crate::speech::queue_linked(u, None, e.event, variable, link);
    mark(u, h, index);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_overrides_the_compiled_tunables() {
        let s = ScriptFile::parse_bytes(
            b"batGlobalFrequencyModifier      1.0\nmaxDistance     COMM_CannotComply                   20000\nminWavelength   COMM_CannotComply                   3   //wait\nexpWavelength   COMM_LInt_Kamikaze 1\n",
        )
        .unwrap();
        let t = ChatterTable::parse(Some(&s));
        assert_eq!(t.frequency, 1.0);
        let c = t.get(bce::CANNOT_COMPLY).unwrap();
        assert_eq!(c.max_distance_sq, 4.0e8);
        assert_eq!(c.min_wavelength, 3.0);
        assert_eq!(t.get(bce::KAMIKAZE).unwrap().max_distance_sq, 1.0e6);
    }
}
