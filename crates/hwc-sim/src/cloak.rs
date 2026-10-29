//! Cloaking (`Cloak.c`, 0x431a00..0x431cb0) as the Cataclysm cloakers use it: the cloaked
//! fighter (`bCloakedFighter`, state at +0x650) and the ion array frigates (`bIonArrayFrigate`,
//! `p1IonArrayFrigate`, state at +0x638). A cloak has power (0..1000) that recharges while
//! visible and drains while cloaked; fading in and out takes 20 frames after a short random
//! delay. While cloaked the ship is invisible to every player that has no detector near it
//! (see [`crate::visibility`]).

use hwc_math::fpu::{ld, st};
use hwc_math::random::RAN_ETG;

use crate::object::{sof, Ship, ShipId};
use crate::statics::ShipType;
use crate::universe::{SpecialEvent, Universe};

/// `SOF_Cloakable` / `SOF_Cloaked` (Cataclysm's numbering).
pub const SOF_CLOAKABLE: u32 = 0x0000_0200;
pub const SOF_CLOAKED: u32 = 0x0000_0400;

/// Full cloak power.
pub const MAX_POWER: f32 = 1000.0;
/// Fade step per update.
const FADE_STEP: f32 = 0.05;

/// Cloak states (`CloakStatus`).
pub mod state {
    /// Visible (power recharging).
    pub const VISIBLE: i32 = 0;
    /// Cloaked (power draining).
    pub const CLOAKED: i32 = 1;
    /// Fading in (becoming visible).
    pub const DECLOAKING: i32 = 2;
    /// Fading out (becoming cloaked).
    pub const CLOAKING: i32 = 3;
}

/// A cloak (`CloakStruct`, 24 bytes).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Cloak {
    pub state: i32,
    pub power: f32,
    /// Power regained per update while visible.
    pub recharge: f32,
    /// Power used per update while cloaked.
    pub drain: f32,
    /// When the fade starts.
    pub fade_start: f32,
    /// 0 visible .. 1 cloaked.
    pub fade: f32,
}

/// The types with a cloak.
pub fn is_cloaker(t: ShipType) -> bool {
    matches!(t, ShipType::bCloakedFighter | ShipType::bIonArrayFrigate | ShipType::p1IonArrayFrigate)
}

/// `cloakInit`: full power, the given rates.
pub fn init(s: &mut Ship, drain: f32, recharge: f32) -> Cloak {
    s.flags |= SOF_CLOAKABLE;
    Cloak { state: state::VISIBLE, power: MAX_POWER, recharge, drain, fade_start: 0.0, fade: 0.0 }
}

/// `cloakIsCloaked`.
pub fn is_cloaked(s: &Ship) -> bool {
    s.flags & SOF_CLOAKABLE != 0 && s.flags & SOF_CLOAKED != 0
}

/// The fade delay: `1 - 2^(r * 2^-30 * -0.005 * log2(e))` seconds from now (stream 1), as
/// `cloakToggle`, `cloakUpdate` and the Mimic morphs compute it (the x87 multiplies in this
/// order, the two factors stored as floats).
pub(crate) fn fade_start_time(u: &mut Universe) -> f32 {
    let r = u.randy.random(RAN_ETG);
    let x = f64::from(r) * ld(9.313_226e-8) * ld(-0.005f32) * std::f64::consts::LOG2_E;
    let y = x.exp2();
    st((1.0 - y) + ld(u.total_time_elapsed))
}

fn cloak_mut(s: &mut Ship) -> Option<&mut Cloak> {
    crate::ships::cloak_of(s)
}

/// `cloakToggle` (0x431a40): start cloaking (not while attacking or launching, and only with
/// at least 100 power), or start decloaking.
pub fn toggle(u: &mut Universe, id: ShipId) {
    let order = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).map(|c| c.order);
    let Some(c) = u.ships.get(id).and_then(|s| crate::ships::cloak_ref(s)).copied() else { return };
    match c.state {
        state::VISIBLE => {
            if matches!(order, Some(crate::command::order::ATTACK) | Some(crate::command::order::LAUNCH_SHIP)) {
                return;
            }
            if c.power < 100.0 {
                crate::speech::queue(u, Some(id), crate::speech::event::CLOAK_NO_POWER, 0);
                return;
            }
            let t = fade_start_time(u);
            let s = u.ships.ship_mut(id);
            let c = cloak_mut(s).expect("cloak");
            c.state = state::CLOAKING;
            c.fade = 0.0;
            c.fade_start = t;
            crate::speech::queue(u, Some(id), crate::speech::event::CLOAK_ON, 0);
        }
        state::CLOAKED => {
            let t = fade_start_time(u);
            let s = u.ships.ship_mut(id);
            s.flags &= !SOF_CLOAKED;
            let c = cloak_mut(s).expect("cloak");
            c.state = state::DECLOAKING;
            c.fade = 1.0;
            c.fade_start = t;
            crate::speech::queue(u, Some(id), crate::speech::event::DECLOAK, 0);
        }
        _ => {}
    }
}

/// `cloakGetPowerFraction`.
pub fn power_fraction(s: &Ship) -> f32 {
    crate::ships::cloak_ref(s).map_or(0.0, |c| st(ld(c.power) * 0.001))
}

/// `cloakGetFadeLevel`: the fade while fading.
pub fn fade_level(s: &Ship) -> Option<f32> {
    crate::ships::cloak_ref(s).filter(|c| c.state > 1 && c.state < 4).map(|c| c.fade)
}

/// `cloakUpdate` (0x431cb0), once per update from the cloaker's housekeeping.
pub fn update(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let order = u.ships.ship(id).command.and_then(|c| u.commands.get(c)).map(|c| c.order);
    let Some(c) = u.ships.get(id).and_then(|s| crate::ships::cloak_ref(s)).copied() else { return };
    match c.state {
        state::VISIBLE => {
            if c.power < MAX_POWER {
                let s = u.ships.ship_mut(id);
                let c = cloak_mut(s).expect("cloak");
                // (The comparisons use the unrounded x87 sums.)
                let p = ld(c.recharge) + ld(c.power);
                c.power = if ld(MAX_POWER) <= p { MAX_POWER } else { st(p) };
            }
        }
        state::CLOAKED => {
            let p = st(ld(c.power) - ld(c.drain));
            if p <= 0.0 {
                let t = fade_start_time(u);
                let s = u.ships.ship_mut(id);
                s.flags &= !SOF_CLOAKED;
                let c = cloak_mut(s).expect("cloak");
                c.state = state::DECLOAKING;
                c.fade = 1.0;
                c.power = 0.0;
                c.fade_start = t;
            } else {
                cloak_mut(u.ships.ship_mut(id)).expect("cloak").power = p;
            }
        }
        state::DECLOAKING => {
            if c.fade_start < now {
                let f = st(ld(c.fade) - ld(FADE_STEP));
                let s = u.ships.ship_mut(id);
                let cm = cloak_mut(s).expect("cloak");
                cm.fade = f;
                if f <= 0.0 {
                    cm.state = state::VISIBLE;
                    cm.fade = 0.0;
                    for p in 0..8 {
                        crate::visibility::set_visible(u, id, p, true, false);
                    }
                    u.special_events.push(SpecialEvent::Decloaked { ship: id });
                }
            }
        }
        state::CLOAKING => {
            if matches!(order, Some(crate::command::order::ATTACK) | Some(crate::command::order::LAUNCH_SHIP)) {
                cloak_mut(u.ships.ship_mut(id)).expect("cloak").state = state::DECLOAKING;
                return;
            }
            if c.fade_start < now {
                let f = ld(c.fade) + ld(FADE_STEP);
                let s = u.ships.ship_mut(id);
                let cm = cloak_mut(s).expect("cloak");
                cm.fade = st(f);
                if 1.0 <= f {
                    cm.state = state::CLOAKED;
                    cm.fade = 1.0;
                    s.flags |= SOF_CLOAKED;
                    for p in 0..8 {
                        crate::visibility::set_visible(u, id, p, false, false);
                    }
                    just_cloaked(u, id);
                    u.special_events.push(SpecialEvent::Cloaked { ship: id });
                }
            }
        }
        _ => {}
    }
}

/// `shipHasJustCloaked` (0x440dc0): commands of players that cannot see the ship drop it as
/// a target.
pub fn just_cloaked(u: &mut Universe, id: ShipId) {
    crate::command::remove_invisible_target(u, id);
    let _ = sof::DEAD;
}
