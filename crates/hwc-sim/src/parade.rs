//! The military parade (`Formation.c` / `CommandLayer.c`: `GroupShipIntoMilitaryParade`,
//! `CreateMilitaryGroupAroundShip` 0x481750, `PutShipInMilitaryParade` 0x4815e0, `GetShipSlot`
//! 0x481230, `processMilitaryParadeToDo` 0x480980, `setMilitaryParade` 0x480e90): ships grouped
//! around a mothership, carrier, processor or slipgate in slots by type, each slot a row
//! starting at an offset in the host's axes and stepping by a spacing (the `Parade_*.script`
//! tables). Fleets take it at the end of a jump and on arrival, KAS forces it
//! (`ForceParadeFormation`, `ShipsSetFormation(.., 8)`).

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::object::ShipId;
use crate::statics::{DataSource, ShipType};
use crate::universe::Universe;

/// Slots per parade (`MAX_MILITARY_SLOTS`).
pub const NUM_SLOTS: usize = 63;
/// The slot of ships no other slot takes (`SLOT_Misc`).
pub const SLOT_MISC: usize = 62;
/// Subslots are added in batches of this many (`SUBSLOT_BATCH`).
const SUBSLOT_BATCH: usize = 10;
/// `PARADE_CONSIDERED_STEADY_VELSQR` (0x87584c).
pub const CONSIDERED_STEADY_VELSQR: f32 = 2500.0;

/// The slot keywords of the parade scripts, by slot number (table 0x875a28).
pub const SLOT_NAMES: [&str; NUM_SLOTS] = [
    "SRECON", "SMIMIC", "SMCV", "SWORKER", "SPROCESSOR", "SCARRIER0", "SCARRIER1", "SCARRIER2", "SSENTINEL", "SACOLYTE", "SACV",
    "SRAMMINGFRIGATE", "SMULTIBEAMFRIGATE", "SHIVEFRIGATE", "SDREADNOUGHT", "SLEECH", "SDESTROYER", "SMOTHERSHIP", "BRECON",
    "BWORKER", "BPROCESSOR", "BHIVEFRIGATE", "BMULTIBEAMFRIGATE", "BCARRIER0", "BCARRIER1", "BCARRIER2", "BSENTINEL", "BACOLYTE",
    "BACV", "BINTERCEPTOR", "BATTACKBOMBER", "BCLOAKEDFIGHTER", "BMULTIGUNCORVETTE", "BDFGFRIGATE", "BHEAVYCORVETTE",
    "BIONARRAYFRIGATE", "BHEAVYCRUISER", "BMISSILECORVETTE", "BCRUISEMISSILE", "BRAMMINGFRIGATE", "BMOTHERSHIP", "Fighter0",
    "Fighter1", "Fighter2", "Fighter3", "Fighter4", "Fighter5", "Corvette0", "Corvette1", "Corvette2", "Corvette3", "Frigate",
    "ResCollector", "Destroyer", "HeavyCruiser", "NonCombat", "Carrier", "SupportCorvette", "P1Fighter", "P1IonArrayFrigate",
    "P1MissileCorvette", "P1StandardCorvette", "Misc",
];

/// The parade scripts by parade type (`paradeSetTweakables` 0x4807d0).
pub const PARADE_SCRIPTS: [&str; 12] = [
    "Parade_sMothership.script",
    "Parade_bMothership.script",
    "Parade_sCarrier.script",
    "Parade_bCarrier.script",
    "Parade_Slipgate.script",
    "Parade_sProcessor.script",
    "Parade_bProcessor.script",
    "Parade_dPrisonShip.script",
    "Parade_R1Carrier.script",
    "Parade_R2Carrier.script",
    "Parade_P1Mothership.script",
    "Parade_bHeavyCruiser.script",
];

/// One slot of a parade table (`SlotInfo`).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct SlotInfo {
    pub up_offset: f32,
    pub right_offset: f32,
    pub heading_offset: f32,
    pub up_spacing: f32,
    pub right_spacing: f32,
    pub heading_spacing: f32,
    /// 0 forward, 1 back, 2 right, 3 left.
    pub face_direction: u32,
}

/// The twelve parade tables.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ParadeTables {
    #[serde(with = "slot_tables")]
    pub types: Vec<[SlotInfo; NUM_SLOTS]>,
}

/// Serde for the per-script slot tables (arrays longer than serde's own 32).
mod slot_tables {
    use super::{SlotInfo, NUM_SLOTS};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[[SlotInfo; NUM_SLOTS]], s: S) -> Result<S::Ok, S::Error> {
        let rows: Vec<&[SlotInfo]> = v.iter().map(|r| &r[..]).collect();
        rows.serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<[SlotInfo; NUM_SLOTS]>, D::Error> {
        let rows: Vec<Vec<SlotInfo>> = Vec::deserialize(d)?;
        rows.into_iter()
            .map(|r| r.try_into().map_err(|_| serde::de::Error::custom("parade slot table length")))
            .collect()
    }
}

impl Default for ParadeTables {
    fn default() -> Self {
        ParadeTables { types: vec![[SlotInfo::default(); NUM_SLOTS]; PARADE_SCRIPTS.len()] }
    }
}

impl ParadeTables {
    /// `paradeSetTweakables`: every table zeroed, then filled from its script
    /// (`SlotPositionInfo[SLOT_x] up, right, heading, upspacing, rightspacing, headingspacing, face`).
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = ParadeTables::default();
        for (k, file) in PARADE_SCRIPTS.iter().enumerate() {
            let Some(bytes) = src.read(file) else { continue };
            let text = String::from_utf8_lossy(&bytes);
            for line in text.lines() {
                let line = line.split(';').next().unwrap_or("").trim();
                let Some(rest) = line.strip_prefix("SlotPositionInfo[SLOT_") else { continue };
                let Some((name, values)) = rest.split_once(']') else { continue };
                let Some(slot) = SLOT_NAMES.iter().position(|n| *n == name) else { continue };
                let f = crate::statics::cscan::decomma(values);
                let v: Vec<&str> = f.split_whitespace().collect();
                let num = |i: usize| v.get(i).and_then(|s| crate::statics::cscan::scan_f32(s)).unwrap_or(0.0);
                t.types[k][slot] = SlotInfo {
                    up_offset: num(0),
                    right_offset: num(1),
                    heading_offset: num(2),
                    up_spacing: num(3),
                    right_spacing: num(4),
                    heading_spacing: num(5),
                    face_direction: v.get(6).and_then(|s| crate::statics::cscan::scan_i32(s)).unwrap_or(0).max(0) as u32,
                };
            }
        }
        t
    }
}

/// One slot of a parade: its row of subslots and the highest one used (-1: empty).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MilitarySlot {
    pub highest_used: i32,
    pub subslots: Vec<Option<ShipId>>,
}

impl Default for MilitarySlot {
    fn default() -> Self {
        MilitarySlot { highest_used: -1, subslots: vec![None; SUBSLOT_BATCH] }
    }
}

impl MilitarySlot {
    fn recalc(&mut self) {
        self.highest_used = self.subslots.iter().rposition(Option::is_some).map_or(-1, |i| i as i32);
    }
}

/// `MilitaryParadeCommand`: the host and its slots.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MilitaryParade {
    pub around: ShipId,
    /// Parade type (index into [`PARADE_SCRIPTS`], -1 for a host without one).
    pub parade_type: i32,
    pub slots: Vec<MilitarySlot>,
}

/// The parade type of a host (`CreateMilitaryGroupAroundShip`).
pub fn parade_type_of(t: ShipType) -> i32 {
    match t.index() {
        0xc => 2,
        0x10 | 0x8d => 5,
        0x12 | 0x62 | 0x68 => 0,
        0x2e => 11,
        0x2f => 3,
        0x33 => 6,
        0x35 | 0x82 | 0x89 => 1,
        0x3e => 8,
        0x4c | 0x65 => 9,
        0x5c | 0x60 => 10,
        0x6d | 0x86 | 0x87 => 7,
        0x70..=0x72 => 4,
        _ => -1,
    }
}

/// `GetShipSlot` (0x481230): a ship's slot in `parade` (carriers take the first free carrier
/// slot of the three; a Beast heavy cruiser has its own slot only around a Beast mothership).
pub fn ship_slot(u: &Universe, ship: ShipId, parade: &MilitaryParade) -> usize {
    let used = |s: usize| parade.slots[s].highest_used >= 0;
    let t = u.ships.ship(ship).ship_type.index();
    match t {
        0 => 0,
        1 | 0x85 => 9,
        2 => 1,
        3 => 0xf,
        4 => 10,
        5 => 2,
        6 => 0xb,
        7 => 0xc,
        8 => 0xd,
        9 | 0x2b => 0xf,
        10 => 0xe,
        0xc => {
            if used(5) {
                6 + usize::from(used(6))
            } else {
                5
            }
        }
        0xe => 3,
        0x10 => 4,
        0x11 => 8,
        0x12 => 0x11,
        0x19 => 0x10,
        0x21 => 0x12,
        0x22 => 0x1b,
        0x23 => 0x1d,
        0x24 => 0x1f,
        0x25 => 0x1e,
        0x26 => 0x1c,
        0x27 => 0x20,
        0x28 => 0x22,
        0x29 => 0x16,
        0x2a => 0x15,
        0x2c => 0x23,
        0x2d => 0x21,
        0x2e => {
            if u.ships.get(parade.around).is_some_and(|a| a.ship_type.index() == 0x35) {
                0x24
            } else {
                SLOT_MISC
            }
        }
        0x2f => {
            if used(0x17) {
                0x18 + usize::from(used(0x18))
            } else {
                0x17
            }
        }
        0x30 => 0x27,
        0x31 => 0x13,
        0x33 => 0x14,
        0x34 => 0x1a,
        0x35 => 0x28,
        0x3a => 0x26,
        0x3b => 0x25,
        0x3d | 0x4b => 0x2d,
        0x3e => 0x38,
        0x3f | 0x40 | 0x4d | 0x4f => 0x37,
        0x41 => 0x30,
        0x42 | 0x50 => 0x36,
        0x43 | 0x51 => 0x2a,
        0x44 | 0x52 => 0x2c,
        0x46 | 0x56 => 0x31,
        0x48 | 0x59 => 0x39,
        0x49 | 0x5a => 0x35,
        0x54 => 0x32,
        0x5d => 0x3a,
        0x5e => 0x3b,
        0x5f => 0x3c,
        0x61 => 0x3d,
        _ => SLOT_MISC,
    }
}

fn table(u: &Universe, parade_type: i32) -> Option<&[SlotInfo; NUM_SLOTS]> {
    usize::try_from(parade_type).ok().and_then(|t| u.tweaks.parade.types.get(t))
}

/// `PutShipInMilitaryParade` (0x4815e0): the ship takes the first free subslot of its slot
/// (the misc slot when its slot has no position in this table).
pub fn put_ship(u: &Universe, ship: ShipId, parade: &mut MilitaryParade) {
    let mut slot = ship_slot(u, ship, parade);
    if let Some(t) = table(u, parade.parade_type) {
        let i = t[slot];
        if i.up_offset == 0.0 && i.right_offset == 0.0 && i.heading_offset == 0.0 {
            slot = SLOT_MISC;
        }
    }
    let s = &mut parade.slots[slot];
    if let Some(free) = s.subslots.iter().position(Option::is_none) {
        s.subslots[free] = Some(ship);
        s.recalc();
        return;
    }
    let n = s.subslots.len();
    s.subslots.resize(n + SUBSLOT_BATCH, None);
    s.subslots[n] = Some(ship);
    s.highest_used = n as i32;
}

/// `RemoveShipFromMilitaryParade`.
pub fn remove_ship(parade: &mut MilitaryParade, ship: ShipId) {
    for s in &mut parade.slots {
        if let Some(i) = s.subslots.iter().position(|&x| x == Some(ship)) {
            s.subslots[i] = None;
            s.recalc();
            return;
        }
    }
}

/// A new parade around `around`.
pub fn new_parade(u: &Universe, around: ShipId) -> MilitaryParade {
    let parade_type = u.ships.get(around).map_or(-1, |a| parade_type_of(a.ship_type));
    MilitaryParade { around, parade_type, slots: vec![MilitarySlot::default(); NUM_SLOTS] }
}

fn add(a: Vector, b: Vector) -> Vector {
    Vector::new(st(ld(a.x) + ld(b.x)), st(ld(a.y) + ld(b.y)), st(ld(a.z) + ld(b.z)))
}

/// The first position of a slot and its step, in the host's axes (up, right, heading).
fn slot_frame(info: &SlotInfo, pos: Vector, up: Vector, right: Vector, heading: Vector) -> (Vector, Vector) {
    let c = |h: f32, r: f32, u_: f32, p: f32| st(ld(info.heading_offset) * ld(h) + ld(info.right_offset) * ld(r) + ld(u_) * ld(info.up_offset) + ld(p));
    let at = Vector::new(c(heading.x, right.x, up.x, pos.x), c(heading.y, right.y, up.y, pos.y), c(heading.z, right.z, up.z, pos.z));
    let d = |h: f32, u_: f32, r: f32| st(ld(info.heading_spacing) * ld(h) + ld(info.up_spacing) * ld(u_) + ld(info.right_spacing) * ld(r));
    let step = Vector::new(d(heading.x, up.x, right.x), d(heading.y, up.y, right.y), d(heading.z, up.z, right.z));
    (at, step)
}

/// The slot positions of every ship of a parade (`setMilitaryParade`'s walk): `(ship, position,
/// face direction)`.
pub fn slot_positions(u: &Universe, parade: &MilitaryParade) -> Vec<(ShipId, Vector, u32)> {
    let mut out = Vec::new();
    let (Some(t), Some(leader)) = (table(u, parade.parade_type), u.ships.get(parade.around)) else { return out };
    let cs = leader.rotinfo.coordsys;
    let (up, right, heading) = (cs.col(0), cs.col(1), cs.col(2));
    for (k, slot) in parade.slots.iter().enumerate() {
        if slot.highest_used < 0 {
            continue;
        }
        let info = t[k];
        let (mut at, step) = slot_frame(&info, leader.posinfo.position, up, right, heading);
        for sub in slot.subslots.iter().take(slot.highest_used as usize + 1) {
            if let Some(id) = sub {
                out.push((*id, at, info.face_direction));
            }
            at = add(at, step);
        }
    }
    out
}

/// `setMilitaryParade` (0x480e90): every ship is put at its slot, facing as the slot says.
pub fn set_parade(u: &mut Universe, parade: &MilitaryParade) {
    let Some(leader) = u.ships.get(parade.around) else { return };
    let cs = leader.rotinfo.coordsys;
    let (up, right, heading) = (cs.col(0), cs.col(1), cs.col(2));
    let neg = |v: Vector| Vector::new(-v.x, -v.y, -v.z);
    let statics = u.statics.clone();
    for (id, at, face) in slot_positions(u, parade) {
        let Some(s) = u.ships.get_mut(id) else { continue };
        s.posinfo.position = at;
        s.rotinfo.coordsys = match face {
            1 => hwc_math::Matrix::from_cols(up, neg(right), neg(heading)),
            2 => hwc_math::Matrix::from_cols(up, neg(heading), right),
            3 => hwc_math::Matrix::from_cols(up, heading, neg(right)),
            _ => cs,
        };
        if let Some(stat) = statics.ship(s.ship_type) {
            crate::physics::update_obj_rot_info(s, stat);
        }
    }
}

/// `processMilitaryParadeToDo` (0x480980): each ship flies to its slot with the host's velocity
/// and lines up with the slot's facing when close; while the host is steady the ships settle.
pub fn process(u: &mut Universe, parade: &MilitaryParade, passive_attacked: bool) {
    use crate::aiship::{self, flags as fly};
    let Some(leader) = u.ships.get(parade.around) else { return };
    let cs = leader.rotinfo.coordsys;
    let (up, right, heading) = (cs.col(0), cs.col(1), cs.col(2));
    let lv = leader.posinfo.velocity;
    let steady_host = st((ld(lv.z) * ld(lv.z) + ld(lv.y) * ld(lv.y)) + ld(lv.x) * ld(lv.x)) < CONSIDERED_STEADY_VELSQR;
    let Some(t) = table(u, parade.parade_type).copied() else { return };
    let tw = u.tweaks.clone();
    let dt = u.phys_time_elapsed;
    let statics = u.statics.clone();
    let neg = |v: Vector| Vector::new(-v.x, -v.y, -v.z);
    for (k, slot) in parade.slots.iter().enumerate() {
        if slot.highest_used < 0 {
            continue;
        }
        let info = t[k];
        let (mut at, step) = slot_frame(&info, u.ships.ship(parade.around).posinfo.position, up, right, heading);
        let mut face = match info.face_direction {
            1 => neg(heading),
            2 => right,
            3 => neg(right),
            _ => heading,
        };
        let mut first = true;
        for sub in slot.subslots.iter().take(slot.highest_used as usize + 1) {
            if let Some(id) = *sub {
                if u.ships.get(id).is_some() {
                    let stat = statics.ship(u.ships.ship(id).ship_type).expect("statics");
                    if first {
                        first = false;
                    } else {
                        u.ships.ship_mut(id).ship_idle = false;
                    }
                    let dont_rotate = passive_attacked && stat.rotate_to_retaliate;
                    let mut fly_it = false;
                    let mut flags_ = 0u32;
                    if !dont_rotate {
                        let dist = crate::aitrack::move_left_to_go(u.ships.ship(id), at);
                        flags_ = fly::POINT_IN_DIRECTION_FLYING | fly::FAST_AS_POSSIBLE;
                        let capital_like = stat.is_capital_ship || matches!(u.ships.ship(id).ship_type.index(), 0x3f | 0x40 | 0x4d | 0x4f);
                        let track = if capital_like {
                            if dist <= 500.0 {
                                if dist <= 100.0 && steady_host {
                                    u.ships.ship_mut(id).flags |= crate::object::sof::FORMATION_SETTLED;
                                }
                                true
                            } else {
                                info.face_direction != 0
                            }
                        } else if dist > 100.0 {
                            info.face_direction != 0
                        } else {
                            if steady_host {
                                u.ships.ship_mut(id).flags |= crate::object::sof::FORMATION_SETTLED;
                            }
                            true
                        };
                        if track {
                            let s = u.ships.ship_mut(id);
                            crate::aitrack::heading(s, stat, &tw, dt, &mut face, tw.flyship_heading_accuracy, tw.flyship_heading_accuracy, stat.sinbank, 0);
                            flags_ = fly::FAST_AS_POSSIBLE;
                        }
                        fly_it = dist >= 10.0;
                    } else {
                        if steady_host && crate::aitrack::move_reached_destination(u.ships.ship(id), at, 100.0) {
                            u.ships.ship_mut(id).flags |= crate::object::sof::FORMATION_SETTLED;
                        }
                        if !crate::aitrack::move_reached_destination(u.ships.ship(id), at, 10.0) {
                            fly_it = true;
                            flags_ = fly::FAST_AS_POSSIBLE;
                        }
                    }
                    if fly_it {
                        aiship::fly_to_point_avoiding_objs(u, id, Some(at), flags_, 0.0, Some(lv));
                    } else if steady_host {
                        crate::aitrack::steady_ship_drift_only(u.ships.ship_mut(id), stat, &tw, dt);
                    }
                }
            }
            at = add(at, step);
        }
    }
}
