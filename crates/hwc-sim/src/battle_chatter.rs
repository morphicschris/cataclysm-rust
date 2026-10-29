//! Battle chatter (`battleCanChatterAtThisTime` 0x4238d0, `battleChatterAttempt` 0x4237b0):
//! speech the ships say now and then rather than every time, rolled on the battle chatter
//! random stream (8) so that it never changes the game.
//!
//! Each event of the table at 0x7448d0 (0x58 bytes each) has its tunables, which
//! `BattleChatter.script` overwrites ([`ChatterTable`], loaded with the statics), and the time
//! and place it was last said ([`ChatterState`], the original's globals: not saved).
//!
//! Only the events the player's orders use are here so far: the kamikaze yell, "cannot
//! comply" and the scuttle confirmation.

use hwc_formats::script::ScriptFile;
use hwc_math::fastmath::fsqrt;
use hwc_math::fpu::{fild_u32, ld, st};
use hwc_math::random::RAN_BATTLE;
use hwc_math::Vector;

use crate::object::ShipId;
use crate::universe::Universe;

/// Battle chatter events (`BCE_*`, the table index).
pub mod bce {
    /// `COMM_LInt_Kamikaze`: a kamikaze closing in (`doKamikazeAttack` state 5).
    pub const KAMIKAZE: usize = 0x11;
    /// `COMM_CannotComply`: an order the selection refuses (scuttling a mothership or carrier).
    pub const CANNOT_COMPLY: usize = 0x26;
    /// `COMM_F_ScuttleReaffirm`: the second scuttle key press (`mrScuttle`).
    pub const SCUTTLE_REAFFIRM: usize = 0x55;
}

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
}

impl ChatterEvent {
    /// The compiled tunables (the same for these events).
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
        }
    }
}

/// The events' tunables after `BattleChatter.script`.
#[derive(Debug, Clone, PartialEq)]
pub struct ChatterTable {
    pub events: Vec<ChatterEvent>,
    /// `batGlobalFrequencyModifier` (0x74674c; the game's chatter option sets it again).
    pub frequency: f32,
}

impl Default for ChatterTable {
    fn default() -> Self {
        ChatterTable {
            events: vec![
                ChatterEvent::compiled(bce::KAMIKAZE, "COMM_LInt_Kamikaze", 0x400_08c5),
                ChatterEvent::compiled(bce::CANNOT_COMPLY, "COMM_CannotComply", 0x400_0810),
                ChatterEvent::compiled(bce::SCUTTLE_REAFFIRM, "COMM_F_ScuttleReaffirm", 0x400_080a),
            ],
            frequency: 128.0,
        }
    }
}

impl ChatterTable {
    /// The compiled table with `BattleChatter.script` applied (`bat*SetCB`: each value is read
    /// as a float; the distances are stored squared).
    pub fn parse(script: Option<&ScriptFile>) -> Self {
        let mut t = ChatterTable::default();
        let Some(script) = script else { return t };
        for e in script.entries() {
            let key = e.key.as_str();
            if key.eq_ignore_ascii_case("batGlobalFrequencyModifier") {
                if let Some(v) = crate::statics::cscan::scan_f32(e.raw.as_str()) {
                    t.frequency = v;
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
                _ => {}
            }
        }
        t
    }

    fn get(&self, index: usize) -> Option<&ChatterEvent> {
        self.events.iter().find(|e| e.index == index)
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
/// enough; then it counts as said.
pub fn attempt(u: &mut Universe, index: usize, ship: ShipId, variable: i32) {
    let Some((k, e)) = slot(u, index) else { return };
    let Some(s) = u.ships.get(ship) else { return };
    if s.player != u.chatter.local_player {
        return;
    }
    let pos = s.posinfo.position;
    let freq = u.chatter.frequency.unwrap_or(u.statics.battle_chatter.frequency);
    let chance = st(ld(freq) * ld(e.random_weight as f32));
    let r = u.randy.random(RAN_BATTLE);
    if fild_u32(r & 0x7fff) >= ld(chance) * 32768.0 {
        return;
    }
    // The ship's `cameraDistanceSquared` (+0x40).
    let dsq = u.chatter.camera_eye.map_or(0.0, |eye| {
        let d = Vector::new(st(ld(pos.x) - ld(eye.x)), st(ld(pos.y) - ld(eye.y)), st(ld(pos.z) - ld(eye.z)));
        st(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z))
    });
    if dsq >= e.max_distance_sq as f32 {
        return;
    }
    *u.chatter.last_mut(k) = (u.total_time_elapsed, pos);
    let speaker = if e.event == FLEET_EVENT { None } else { Some(ship) };
    crate::speech::queue(u, speaker, e.event, variable);
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
