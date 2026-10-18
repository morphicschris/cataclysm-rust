//! Per-race missile and mine statics (`RACE_X\Missile.shp`, `RACE_X\Mine.shp`;
//! `MissileStaticInfo`, 300 bytes).

use super::cscan::scan_f32;
use super::enums::ShipRace;
use super::header::{GuidanceInfo, HeaderKeys, HealthInfo, StaticHeader};
use super::Loader;

#[derive(Debug, Clone, Default)]
pub struct MissileStaticInfo {
    pub race: Option<ShipRace>,
    pub header: StaticHeader,
    pub health: HealthInfo,
    pub guidance: GuidanceInfo,
    /// +0xf8. Mines: trigger range, `sqrt(MINE_RANGESQR)`.
    pub mine_range: f32,
    /// +0xfc. Mines: `sqrt(MINE_RANGESQR_FORCED)`.
    pub mine_range_forced: f32,
    /// +0x100 `MINE_STOP_FRICTION`.
    pub mine_stop_friction: f32,
    /// +0x104 `MINE_RANGESQR` (the file gives the range; the setter squares it).
    pub mine_range_sqr: f32,
    /// +0x108 `MINE_RANGESQR_FORCED`.
    pub mine_range_sqr_forced: f32,
    /// +0x10c `maxvelocity_FORCED`.
    pub max_velocity_forced: f32,
    /// +0x110 `N-LIPS` (mines).
    pub scale_cap: f32,
    /// +0x124 / +0x128. Missiles: trail width and height, -1 (= trail default).
    pub trail_width: f32,
    pub trail_height: f32,
    /// Source file.
    pub file: String,
    /// Lines no table binds.
    pub unbound: Vec<String>,
}

const MISSILE_KEYS: HeaderKeys = HeaderKeys {
    coll_sphere_size: false,
    immobile: false,
    right_of_way: false,
    coll_modifiers: false,
    coll_box_scale: true,
};

/// The mine table additionally lists the box scales, but with offsets that miss the mine's
/// statics, so they have no effect on it.
const MINE_KEYS: HeaderKeys = HeaderKeys { coll_box_scale: false, ..MISSILE_KEYS };

fn load(ld: &mut Loader, race: ShipRace, mine: bool) -> Option<MissileStaticInfo> {
    let dir = format!("{}\\", race.name());
    let file = format!("{dir}{}", if mine { "Mine.shp" } else { "Missile.shp" });
    if !ld.src.exists(&file) {
        return None;
    }
    let script = ld.read_script(&file)?;
    let mut m = MissileStaticInfo { race: Some(race), file, ..Default::default() };
    m.guidance.turn_speed = [1.0; 3];
    m.header.right_of_way = -1;
    let keys = if mine { MINE_KEYS } else { MISSILE_KEYS };
    for e in script.entries() {
        let (key, raw) = (e.key.as_str(), e.raw.as_str());
        let mut bound = ld.apply_header_files(&mut m.header, key, raw, &dir)
            || m.header.apply(key, raw, keys)
            || m.health.apply(key, raw)
            || m.guidance.apply(key, raw);
        if !bound && mine {
            let v = scan_f32(raw);
            bound = true;
            match (key, v) {
                ("maxvelocity_FORCED", Some(v)) => m.max_velocity_forced = v,
                ("MINE_STOP_FRICTION", Some(v)) => m.mine_stop_friction = v,
                ("MINE_RANGESQR", Some(v)) => m.mine_range_sqr = v * v,
                ("MINE_RANGESQR_FORCED", Some(v)) => m.mine_range_sqr_forced = v * v,
                ("N-LIPS", Some(v)) => m.scale_cap = v,
                ("maxvelocity_FORCED" | "MINE_STOP_FRICTION" | "MINE_RANGESQR" | "MINE_RANGESQR_FORCED" | "N-LIPS", None) => {}
                _ => bound = false,
            }
        }
        if !bound {
            m.unbound.push(key.to_owned());
        }
    }
    if mine {
        m.mine_range = f64::from(m.mine_range_sqr).sqrt() as f32;
        m.mine_range_forced = f64::from(m.mine_range_sqr_forced).sqrt() as f32;
    }
    let tw = ld.tweaks.clone();
    m.header.precalc(&tw);
    m.health.precalc(&tw);
    m.guidance.precalc(&mut m.header, &tw);
    if !mine {
        m.trail_width = -1.0;
        m.trail_height = -1.0;
    }
    Some(m)
}

/// `InitStatMissileInfo`.
pub(crate) fn load_missile(ld: &mut Loader, race: ShipRace) -> Option<MissileStaticInfo> {
    load(ld, race, false)
}

/// `InitStatMineInfo`.
pub(crate) fn load_mine(ld: &mut Loader, race: ShipRace) -> Option<MissileStaticInfo> {
    load(ld, race, true)
}
