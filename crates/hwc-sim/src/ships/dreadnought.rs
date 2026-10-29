//! The Dreadnought and its cannon (`Dreadnought` module, 0x5ab970..0x5abee0): sDreadnought
//! (the cannon itself is a ship module, sDreadnoughtCannon, clamped to the hull).
//!
//! * Missile regeneration: every `MissileRegenerateTime` seconds the missile launcher with the
//!   fewest missiles (below its `MaxMissiles`) gets one back.
//! * With the cannon upgrade (`suShipHasUpgrade(ship, 0x400)`) the hull grows its cannon: a
//!   `sDreadnoughtCannon` module ship clamped to it (`DreadnoughtCreateCannon`).
//! * The special (`DreadnoughtSpecialActivate`) charges the cannon for a second once its
//!   recharge time has passed; then the cannon releases whatever is clamped to the hull and
//!   fires a shock wave ([`crate::shockwave`] type 4) that pushes, flings and damages every
//!   non-allied ship around.

use hwc_math::fpu::{ld, st};

use crate::disable::Stopwatch;
use crate::object::ShipId;
use crate::shockwave;
use crate::statics::{ShipStaticInfo, ShipType};
use crate::universe::Universe;

use super::ShipSpec;

/// Upgrade bit of the cannon (`suShipHasUpgrade(ship, 0x400)`).
pub const UPGRADE_CANNON: u32 = 0x400;
/// Seconds between cannon shots (0x8b9ea0).
pub const CANNON_RECHARGE_TIME: f32 = 70.0;
/// Seconds the cannon charges before it fires (0x8b9ea4).
pub const CANNON_CHARGE_TIME: f32 = 1.0;
/// Seconds the cannon takes to assemble once created (0x3c23d70a).
const CANNON_BUILD_TIME: f32 = f32::from_bits(0x3c23_d70a);
/// `suUpgradeBitsPercent(0x400)`.
const CANNON_UPGRADE_PERCENT: f32 = 10.0;

/// Per-ship state (+0x638..+0x139c).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DreadnoughtSpec {
    /// +0x638: when a missile was last regenerated.
    pub last_regenerate: f32,
    /// +0x640: when the last missile volley started (never set again: -100000).
    pub last_volley: f32,
    /// +0x1368: the cannon's recharge.
    pub recharge: Stopwatch,
    /// +0x1378: the cannon's assembly.
    pub build: Stopwatch,
    /// +0x1388: the charge before a shot.
    pub charge: Stopwatch,
    /// +0x1398: charging.
    pub charging: bool,
    /// +0x139c: the cannon module.
    pub cannon: Option<ShipId>,
}

impl Default for DreadnoughtSpec {
    fn default() -> Self {
        Self {
            last_regenerate: 0.0,
            last_volley: -100_000.0,
            recharge: Stopwatch::default(),
            build: Stopwatch::default(),
            charge: Stopwatch::default(),
            charging: false,
            cannon: None,
        }
    }
}

/// Per-type statics (`DreadnoughtStaticInit` 0x5ab970). The first six values of the table are
/// the per-tactic bullet ranges (+0x1a4..) and 90 % of the minimum ranges (+0x1b0..), as for
/// the other capital-ship modules.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct DreadnoughtStatics {
    pub range: [f32; 3],
    pub min_range: [f32; 3],
    /// `MissileRegenerateTime`.
    pub missile_regenerate_time: f32,
    /// `MissileVolleyTime`.
    pub missile_volley_time: f32,
    /// `MissileLagVolleyTime`.
    pub missile_lag_volley_time: f32,
}

impl DreadnoughtStatics {
    pub fn parse(stat: &ShipStaticInfo) -> Self {
        let mut s = Self::default();
        for (k, v) in &stat.custom_entries {
            let target = match k.as_str() {
                "MissileRegenerateTime" => &mut s.missile_regenerate_time,
                "MissileVolleyTime" => &mut s.missile_volley_time,
                "MissileLagVolleyTime" => &mut s.missile_lag_volley_time,
                _ => continue,
            };
            if let Some(x) = crate::statics::cscan::scan_f32(v) {
                *target = x;
            }
        }
        for t in 0..3 {
            s.range[t] = stat.bullet_range[t];
            s.min_range[t] = st(ld(stat.min_bullet_range[t]) * ld(0.9f32));
        }
        s
    }
}

fn statics(u: &Universe, t: ShipType) -> DreadnoughtStatics {
    match super::type_statics(u, t) {
        super::TypeStatics::Dreadnought(d) => d.clone(),
        _ => DreadnoughtStatics::default(),
    }
}

fn spec(u: &Universe, id: ShipId) -> Option<&DreadnoughtSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Dreadnought(d) => Some(d),
        _ => None,
    }
}

fn spec_mut(u: &mut Universe, id: ShipId) -> Option<&mut DreadnoughtSpec> {
    match &mut u.ships.get_mut(id)?.spec {
        ShipSpec::Dreadnought(d) => Some(d),
        _ => None,
    }
}

fn has_cannon_upgrade(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| crate::upgrade::has_upgrade(s, UPGRADE_CANNON))
}

/// `DreadnoughtInit` (0x5ab9d0): the cannon starts recharging. (The 40-entry missile table it
/// also resets is never read by Cataclysm.)
pub fn init(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let mut d = DreadnoughtSpec::default();
    d.recharge.start(now, CANNON_RECHARGE_TIME);
    u.ships.ship_mut(id).spec = ShipSpec::Dreadnought(d);
}

/// The missile part of `DreadnoughtHouseKeep`: the launcher with the fewest missiles below its
/// capacity gets one back.
fn regenerate_missile(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let st_ = statics(u, u.ships.ship(id).ship_type);
    let Some(d) = spec(u, id) else { return };
    if !(ld(st_.missile_lag_volley_time) < ld(now) - ld(d.last_volley) && ld(st_.missile_regenerate_time) < ld(now) - ld(d.last_regenerate)) {
        return;
    }
    if let Some(d) = spec_mut(u, id) {
        d.last_regenerate = now;
    }
    let statics = u.statics.clone();
    let s = u.ships.ship(id);
    let Some(gstats) = statics.ship(s.ship_type).and_then(|st| st.guns.clone()) else { return };
    let mut best: Option<usize> = None;
    let mut fewest = 100_000;
    for (i, g) in s.guns.iter().enumerate() {
        let Some(gs) = gstats.get(i) else { continue };
        if g.level == crate::bullet::weapon::MISSILE && g.num_missiles < gs.max_missiles && g.num_missiles < fewest {
            best = Some(i);
            fewest = g.num_missiles;
        }
    }
    if let Some(i) = best {
        u.ships.ship_mut(id).guns[i].num_missiles += 1;
    }
}

/// `RKOReleaseDockedShips` (0x5bead0): everything clamped to the hull lets go (rammers stop
/// pushing, leeches drop off), last first. (`salvCheckDocked` on each is not modelled.)
fn release_docked_ships(u: &mut Universe, id: ShipId) {
    let slaves = crate::clamp::slaves_of(u, id);
    for &sid in slaves.iter().rev() {
        let Some(t) = u.ships.get(sid).map(|s| s.ship_type) else { continue };
        super::ramming::stop_pushing(u, sid);
        if t == ShipType::sLeech {
            super::leech::emp_shocked(u, sid);
        }
    }
}

/// `DreadnoughtHouseKeep` (0x5aba30).
pub fn housekeep(u: &mut Universe, id: ShipId) {
    regenerate_missile(u, id);
    let now = u.total_time_elapsed;
    let Some(d) = spec(u, id).cloned() else { return };
    if d.cannon.is_some() && d.charging && d.charge.expired(now) {
        release_docked_ships(u, id);
        if let Some(d) = spec_mut(u, id) {
            d.charging = false;
        }
        if has_cannon_upgrade(u, id) && spec(u, id).is_some_and(|d| d.recharge.expired(now)) {
            let s = u.ships.ship(id);
            let (pos, player) = (s.posinfo.position, s.player);
            shockwave::create(u, shockwave::kind::DREADNOUGHT, pos, player);
            if let Some(d) = spec_mut(u, id) {
                d.recharge.start(now, CANNON_RECHARGE_TIME);
            }
            u.special_events.push(crate::universe::SpecialEvent::Special { ship: id, effect: super::Effect::BigGunFired });
        }
    }
    let cannon = spec(u, id).and_then(|d| d.cannon);
    if cannon.is_none() {
        // (A ship being consumed, +0x570, would not grow its cannon; consumption is not
        // modelled.)
        if !has_cannon_upgrade(u, id) {
            return;
        }
        create_cannon(u, id);
        if let Some(d) = spec_mut(u, id) {
            d.build.start(now, CANNON_BUILD_TIME);
        }
        if spec(u, id).and_then(|d| d.cannon).is_none() {
            return;
        }
    }
    let Some(d) = spec(u, id).cloned() else { return };
    if !d.build.expired(now) {
        let f = st(d.build.fraction(now));
        if let Some(c) = d.cannon.and_then(|c| u.ships.get_mut(c)) {
            c.base.build_progress = f;
        }
    }
}

/// `DreadnoughtUpdateCannonCharge` (0x5abc40), for presentation: the charge fraction of the
/// arc the renderer draws while the cannon charges.
pub fn cannon_charge_arc(u: &Universe, id: ShipId) -> Option<f32> {
    let s = u.ships.get(id)?;
    let d = spec(u, id)?;
    if s.ship_type != ShipType::sDreadnought || !d.charging || d.cannon.is_none() {
        return None;
    }
    Some(st(d.charge.fraction(u.total_time_elapsed)))
}

/// `DreadnoughtSpecialActivate` (0x5abc90): start charging when the cannon is there and has
/// recharged.
pub fn special_activate(u: &mut Universe, id: ShipId) -> bool {
    let now = u.total_time_elapsed;
    let Some(d) = spec(u, id) else { return true };
    if d.cannon.is_some() && has_cannon_upgrade(u, id) && !d.charging && d.recharge.expired(now) {
        let d = spec_mut(u, id).expect("spec");
        d.charging = true;
        d.charge.start(now, CANNON_CHARGE_TIME);
        crate::speech::queue(u, Some(id), crate::speech::event::CANNON_CHARGING, 0);
    }
    true
}

/// `DreadnoughtDied` (0x5abd10): the cannon goes with the hull.
pub fn died(u: &mut Universe, id: ShipId) {
    let Some(c) = spec(u, id).and_then(|d| d.cannon) else { return };
    if u.ships.get(c).is_none() {
        return;
    }
    if crate::clamp::host_of(u, c).and_then(|h| u.ships.get(h)).is_some_and(|h| h.ship_type == ShipType::sMothership) {
        crate::clamp::unclamp(u, c);
        u.ships.ship_mut(c).flags &= !crate::base::SOF_MODULE;
    }
    crate::damage::really_delete_ship(u, c);
}

/// `DreadnoughtRemoveShipReferences` (0x5abda0).
pub fn remove_ship_refs(u: &mut Universe, id: ShipId, gone: ShipId) {
    if let Some(d) = spec_mut(u, id) {
        if d.cannon == Some(gone) {
            d.charging = false;
            d.cannon = None;
        }
    }
}

/// `DreadnoughtCanFireCannon` (0x5abdc0).
pub fn can_fire_cannon(u: &Universe, id: ShipId) -> bool {
    u.ships.get(id).is_some_and(|s| s.ship_type == ShipType::sDreadnought) && has_cannon_upgrade(u, id)
}

/// `DreadnoughtGetCannonChargeFraction` (0x5abdf0): how far the recharge is (at most 1).
pub fn cannon_charge_fraction(u: &Universe, id: ShipId) -> f64 {
    let Some(d) = spec(u, id) else { return 0.0 };
    let f = d.recharge.fraction(u.total_time_elapsed);
    if f > 1.0 {
        1.0
    } else {
        f
    }
}

/// `DreadnoughtCreateCannon` (0x5abe20): the cannon module, at the hull's position and
/// orientation, clamped to the hull (or to what the hull is clamped to).
pub fn create_cannon(u: &mut Universe, id: ShipId) {
    let now = u.total_time_elapsed;
    let s = u.ships.ship(id);
    let (pos, player, sys, t) = (s.posinfo.position, s.player, s.rotinfo.coordsys, s.ship_type);
    let scale = u.statics.ship(t).map_or(1.0, |st| st.upgrade_time_modifier);
    if let Some(d) = spec_mut(u, id) {
        // suGetUpgradeScaledValue: the upgrade's share (`suUpgradeBitsPercent(0x400)`, 10)
        // times the type's upgrade-time factor.
        d.build.start(now, st(ld(CANNON_UPGRADE_PERCENT) * ld(scale)));
    }
    let opts = crate::base::CreateOptions { builder: Some(id), ..Default::default() };
    let Ok(c) = crate::base::create_ship(u, ShipType::sDreadnoughtCannon, player, pos, opts) else { return };
    {
        let cs = u.ships.ship_mut(c);
        cs.rotinfo.coordsys = sys;
    }
    crate::construction::unit_cap_create_ship(u, c);
    let host = crate::clamp::host_of(u, id).unwrap_or(id);
    crate::clamp::clamp_obj_to_obj(u, c, host);
    u.ships.ship_mut(c).flags |= crate::base::SOF_MODULE;
    if let Some(d) = spec_mut(u, id) {
        d.cannon = Some(c);
    }
}

/// `DreadnoughtReclampCannon` (0x5abee0): clamp the cannon to the hull again (after the hull
/// was released from a dock).
pub fn reclamp_cannon(u: &mut Universe, id: ShipId) {
    let Some(c) = spec(u, id).and_then(|d| d.cannon) else { return };
    if u.ships.get(c).is_none() {
        return;
    }
    crate::clamp::unclamp(u, c);
    crate::clamp::clamp_obj_to_obj(u, c, id);
}
