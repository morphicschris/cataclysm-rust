//! The Nomad moon and its field emitters (`NomadMoon` module, 0x5b8480..0x5b89f0): dNomadMoon
//! (the emitters, dNomadFieldEmitter, have no callbacks of their own).
//!
//! The moon is created with two field emitters 3925 m above and below it. When it pulses
//! (KAS `NomadMoonFire`, or by itself whenever a big-gun shell is in flight) it charges for 6
//! seconds, then sends out a shock wave ([`crate::shockwave`] type 5) that pushes and flings
//! every ship but the Beast's and its own (`ShipApplyFieldSlowdown`: the ship drifts with the
//! push for a second), taking a fifth of the health of each ship it first catches; then it
//! rests for 11 seconds. Losing both emitters stops it.

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::disable::Stopwatch;
use crate::object::ShipId;
use crate::shockwave;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Height of the emitters above and below the moon (0x8bb490).
pub const EMITTER_OFFSET: f32 = 3925.0;
/// Charge before a pulse (0x8bb494), and during an NIS (0x8bb498).
pub const PULSE_CHARGE_TIME: f32 = 6.0;
pub const PULSE_CHARGE_TIME_NIS: f32 = 5.0;
/// Rest after a pulse (0x8bb49c).
pub const PULSE_REST_TIME: f32 = 11.0;

/// Pulse states (+0x640).
pub mod pulse {
    pub const IDLE: u8 = 0;
    pub const CHARGING: u8 = 1;
    pub const RESTING: u8 = 2;
}

/// Per-ship state (+0x638..+0x650).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NomadSpec {
    /// +0x638 / +0x63c: the upper and lower field emitters.
    pub emitters: [Option<ShipId>; 2],
    /// +0x640: see [`pulse`].
    pub pulse: u8,
    /// +0x644: the charge or rest.
    pub timer: Stopwatch,
}

/// Per-type statics (the moon's static init is empty).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NomadStatics {}

impl NomadStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let _ = stat;
        Self {}
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&NomadSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::NomadMoon(n) => Some(n),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut NomadSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::NomadMoon(n) => Some(n),
        _ => None,
    }
}

/// Whether an NIS is playing (`nisIsRunning`); this simulation plays none.
fn nis_running(_u: &Universe) -> bool {
    false
}

/// `NomadMoonInit` (0x5b8480): the two field emitters.
pub fn init(u: &mut Universe, id: ShipId) {
    u.ships.ship_mut(id).spec = ShipSpec::NomadMoon(NomadSpec::default());
    let s = u.ships.ship(id);
    let (p, player) = (s.posinfo.position, s.player);
    let opts = crate::base::CreateOptions { builder: Some(id), ..Default::default() };
    let up = Vector::new(p.x, p.y, st(ld(EMITTER_OFFSET) + ld(p.z)));
    let a = crate::base::create_ship(u, ShipType::dNomadFieldEmitter, player, up, opts).ok();
    let down = Vector::new(p.x, p.y, st(ld(p.z) - ld(EMITTER_OFFSET)));
    let b = crate::base::create_ship(u, ShipType::dNomadFieldEmitter, player, down, opts).ok();
    if let Some(n) = spec_mut(u, id) {
        n.emitters = [a, b];
        n.pulse = pulse::IDLE;
    }
}

/// `NomadMoonFind` (0x5b8550): the (last) Nomad moon in the universe.
pub fn find(u: &Universe) -> Option<ShipId> {
    u.ship_list.iter().copied().rfind(|&s| u.ships.get(s).is_some_and(|x| x.ship_type == ShipType::dNomadMoon))
}

/// `NomadMoonNumEmitters` (0x5b8650).
pub fn num_emitters(u: &Universe, id: ShipId) -> u32 {
    spec(u, id).map_or(0, |n| n.emitters.iter().filter(|e| e.is_some()).count() as u32)
}

/// `NomadMoonCanPulse` (0x5b8580).
pub fn can_pulse(u: &Universe, id: ShipId) -> bool {
    num_emitters(u, id) != 0 && spec(u, id).is_some_and(|n| n.pulse == pulse::IDLE)
}

/// `NomadMoonStartPulse` (0x5b85c0): start charging a pulse if it can.
pub fn start_pulse(u: &mut Universe, id: ShipId) -> bool {
    if !can_pulse(u, id) {
        return false;
    }
    let now = u.total_time_elapsed;
    let t = if nis_running(u) { PULSE_CHARGE_TIME_NIS } else { PULSE_CHARGE_TIME };
    let n = spec_mut(u, id).expect("spec");
    n.timer.start(now, t);
    n.pulse = pulse::CHARGING;
    true
}

/// `NomadMoonRemoveShipReferences` (0x5b8680).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    let Some(n) = spec_mut(u, id) else { return };
    for e in &mut n.emitters {
        if *e == Some(gone) {
            *e = None;
        }
    }
    if n.emitters.iter().all(Option::is_none) && n.pulse != pulse::IDLE {
        n.pulse = pulse::IDLE;
    }
}

/// `NomadMoonHouseKeep` (0x5b8740).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let Some(n) = spec(u, id).cloned() else { return };
    if n.pulse == pulse::CHARGING {
        if n.timer.expired(now) {
            let m = spec_mut(u, id).expect("spec");
            m.timer.start(now, PULSE_REST_TIME);
            m.pulse = pulse::RESTING;
            let s = u.ships.ship(id);
            let (p, player) = (s.posinfo.position, s.player);
            shockwave::create(u, shockwave::kind::NOMAD_PULSE, p, player);
        }
    } else if n.pulse == pulse::RESTING && n.timer.expired(now) {
        spec_mut(u, id).expect("spec").pulse = pulse::IDLE;
    }
    if nis_running(u) {
        // (During an NIS the emitters are kept above and below the moon.)
        return;
    }
    if shockwave::any_active(u) {
        start_pulse(u, id);
    }
}

/// `NomadMoonAttack` (0x5b8920).
pub fn attack(u: &mut Universe, id: ShipId, target: ShipId) {
    crate::attack::attack_simple(u, id, target);
}

/// `NomadMoonFire` (0x5b8940): only a fire call without a target pulses; the combat code always
/// passes one.
pub fn fire(u: &mut Universe, id: ShipId, target: ShipId) {
    let _ = (u, id, target);
}

/// `NomadMoonFire` with no target (the KAS path).
pub fn fire_untargeted(u: &mut Universe, id: ShipId) {
    start_pulse(u, id);
}

/// `NomadMoonUpdateFieldEffect` (0x5b89f0), for presentation: the charge fraction of the arcs
/// to the two emitters while charging.
pub fn field_arc_fraction(u: &Universe, id: ShipId) -> Option<f32> {
    let n = spec(u, id)?;
    (n.pulse == pulse::CHARGING).then(|| st(n.timer.fraction(u.total_time_elapsed)))
}
