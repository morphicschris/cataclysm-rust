//! Proximity sensors (`ProximitySensor` module, 0x5b9d80..0x5b9f30): kProximitySensor,
//! tProximitySensor.
//!
//! Every 32 frames a sensor looks through its collision blob for non-allied ships within its
//! `SearchRadius` (`DetectShips`) and marks them as sensed by its player (`ship+0x394`, read
//! by the sensors manager). While something is in range it shows a ping and its blink circle.
//! That is all the module does: what a sensor reveals of cloaked ships comes from its type
//! being a detector (static capability 0x10000; with the sensors technology's upgrade bit
//! 0x800000 every ship within its vision range becomes visible to its player, see
//! [`crate::visibility::update`]). The mesh's open/close animation (open while still, closed
//! when moving faster than 200) is not modelled.

use hwc_math::fpu::{ld, st};

use crate::object::ShipId;
use crate::statics::cscan::{scan_f32, scan_i32, scan_rgb};
use crate::statics::ShipStaticInfo;
use crate::universe::{SpecialEvent, Universe};

use super::{Effect, ShipSpec};

/// Sensor states (`ship+0x638`).
pub mod state {
    pub const START: i32 = 0;
    pub const SEARCHING: i32 = 1;
    /// Something was found (the ping starts).
    pub const FOUND: i32 = 2;
    /// Tracking: the blink circle runs while something is in range.
    pub const TRACKING: i32 = 3;
}

/// The search runs when `frame & 31 == 27` (0x8a22b8 / 0x8a22bc).
const SEARCH_MASK: u32 = 31;
const SEARCH_FRAME: u32 = 27;

/// Per-ship state.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProximitySpec {
    pub state: i32,
    /// Blink circle radius (+0x640).
    pub blink: f32,
    /// +0x644 (cleared only).
    pub found: i32,
}

/// Per-type statics (`ProximitySensorStaticInit`, the `SearchRate` table at 0x8bb9b0).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct ProximityStatics {
    pub search_rate: i32,
    pub search_rate_after_found: i32,
    pub search_radius: f32,
    pub search_radius_sq: f32,
    pub sensor_circle_radius: i32,
    pub sensor_blink_rate: f32,
    pub trigger_speed: f32,
    pub trigger_speed_sq: f32,
    pub blip_color: [u8; 4],
    pub blip_thickness: i32,
}

impl ProximityStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for (key, raw) in &stat.custom_entries {
            match key.as_str() {
                "SearchRate" => s.search_rate = scan_i32(raw).unwrap_or(s.search_rate),
                "SearchRadius" => s.search_radius = scan_f32(raw).unwrap_or(s.search_radius),
                "SensorCircleRadius" => {
                    s.sensor_circle_radius = scan_i32(raw).unwrap_or(s.sensor_circle_radius)
                }
                "SensorBlinkRate" => {
                    s.sensor_blink_rate = scan_f32(raw).unwrap_or(s.sensor_blink_rate)
                }
                "SearchRateAfterFound" => {
                    s.search_rate_after_found = scan_i32(raw).unwrap_or(s.search_rate_after_found)
                }
                "TriggerSpeed" => s.trigger_speed = scan_f32(raw).unwrap_or(s.trigger_speed),
                "blipColor" => s.blip_color = scan_rgb(raw).unwrap_or(s.blip_color),
                "blipThickness" => s.blip_thickness = scan_i32(raw).unwrap_or(s.blip_thickness),
                _ => {}
            }
        }
        s.search_radius_sq = st(ld(s.search_radius) * ld(s.search_radius));
        s.trigger_speed_sq = st(ld(s.trigger_speed) * ld(s.trigger_speed));
        s
    }
}

fn statics(u: &Universe, id: ShipId) -> ProximityStatics {
    match u.ships.get(id).map(|s| super::type_statics(u, s.ship_type)) {
        Some(super::TypeStatics::ProximitySensor(p)) => p.clone(),
        _ => ProximityStatics::default(),
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut ProximitySpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::ProximitySensor(p) => Some(p),
        _ => None,
    }
}

/// `ProximitySensorInit` (0x5b9dd0).
pub fn init(u: &mut Universe, id: ShipId) {
    u.ships.ship_mut(id).spec = ShipSpec::ProximitySensor(ProximitySpec::default());
}

/// `DetectShips` (0x5b9df0): non-allied ships of the sensor's blob within the search radius
/// are marked as sensed by its player. Whether there were any.
pub fn detect_ships(u: &mut Universe, id: ShipId) -> bool {
    let Some(me) = u.ships.get(id) else {
        return false;
    };
    let Some(blob) = me.coll_my_blob.and_then(|b| u.blobs.get(b)) else {
        return false;
    };
    let r2 = statics(u, id).search_radius_sq;
    let (player, pos) = (me.player, me.posinfo.position);
    let mut hits = Vec::new();
    for &o in &blob.ships {
        let Some(os) = u.ships.get(o) else { continue };
        if u.players.get(player).is_some_and(|p| p.is_ally(os.player)) {
            continue;
        }
        let dx = st(ld(os.posinfo.position.x) - ld(pos.x));
        let dy = st(ld(os.posinfo.position.y) - ld(pos.y));
        let dz = st(ld(os.posinfo.position.z) - ld(pos.z));
        if (ld(dx) * ld(dx) + ld(dy) * ld(dy)) + ld(dz) * ld(dz) < ld(r2) {
            hits.push(o);
        }
    }
    for &o in &hits {
        // (The local player's sensor also has the fleet announce a cloaked ship.)
        if player < 32 {
            u.ships.ship_mut(o).sensed_by |= 1 << player;
        }
    }
    !hits.is_empty()
}

/// `FUN_005b9ef0`: the ping's "still valid" test (it lasts while the sensor is found or
/// tracking). Presentation only.
pub fn ping_alive(u: &Universe, id: ShipId) -> bool {
    match u.ships.get(id).map(|s| &s.spec) {
        Some(ShipSpec::ProximitySensor(p)) => p.state != state::FOUND && p.state != state::TRACKING,
        _ => false,
    }
}

/// `ProximitySensorHouseKeep` (0x5b9f30).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let Some(p) = spec_mut(u, id).cloned() else {
        return;
    };
    let st_ = statics(u, id);
    let search = u.update_counter & SEARCH_MASK == SEARCH_FRAME;
    match p.state {
        state::START => spec_mut(u, id).expect("sensor").state = state::SEARCHING,
        state::SEARCHING => {
            if search && detect_ships(u, id) {
                let s = spec_mut(u, id).expect("sensor");
                s.state = state::FOUND;
                s.blink = 0.0;
                s.found = 0;
                // (The local player's sensor starts a ping on the sensors manager.)
                u.special_events.push(SpecialEvent::Special {
                    ship: id,
                    effect: Effect::State {
                        state: state::FOUND as u32,
                    },
                });
            }
        }
        state::FOUND | state::TRACKING => {
            if p.state == state::FOUND && p.found == 0 {
                spec_mut(u, id).expect("sensor").state = state::TRACKING;
            }
            let b = ld(st_.sensor_blink_rate) + ld(p.blink);
            let s = spec_mut(u, id).expect("sensor");
            s.blink = st(b);
            if ld(st_.sensor_circle_radius as f32) <= b {
                s.blink = 0.0;
            }
            if search && !detect_ships(u, id) {
                let s = spec_mut(u, id).expect("sensor");
                s.state = state::SEARCHING;
                s.found = 0;
            }
        }
        _ => {}
    }
}
