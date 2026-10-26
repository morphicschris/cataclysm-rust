//! Battle pings for the sensors manager (`pingBattlePingsCreate` 0x4e4b40,
//! `pingAttackPingsCreate` 0x4e47f0): every collision blob with a battle flag (0x40 a ship was
//! recently attacked, 0x80 a ship is attacking; set by `bobUpdateObjsInBlobCollInfo`) is split
//! into sub-blobs (`bobSubBlobListCreate`) of the ships that pass `pingBattleBlobCriteria`, with
//! the `pingBob*` blob properties of `ping.script`; each sub-blob is one battle ping. The
//! original does this on the `REFRESH_COLLBLOB_BATTLEPING_FRAME` frames of the collision blob
//! refresh cycle. Presentation only: nothing here changes the simulation.

use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::blobs::{btf, BlobList, Objects};
use crate::object::{sof, ShipId};
use crate::statics::DataSource;
use crate::tunables::BlobProperties;
use crate::universe::Universe;

/// One battle ping: where, how big, and the ships in it.
#[derive(Debug, Clone, PartialEq)]
pub struct BattlePing {
    pub centre: Vector,
    pub radius: f32,
    pub ships: Vec<ShipId>,
}

/// 0x888ae0: per ship type, bit 4 = left out of the sensors' battle pings (types 0..=144).
const SENSORS_TYPE_FLAGS: [u8; 145] = [
    0, 0, 0, 0, 0, 0, 1, 1, 1, 4, 3, 4, 3, 4, 1, 4, 3, 0, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 6, 0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 4, 1, 1, 3, 3, 1,
    1, 4, 3, 0, 6, 6, 6, 6, 6, 0, 0, 1, 0, 3, 0, 0, 0, 3, 0, 0, 1, 0, 4, 0, 1, 1, 0, 3, 0, 1, 0, 3, 0, 0, 1, 0, 1, 0, 4, 1, 0, 1, 1, 1, 0, 1, 0, 3,
    0, 3, 1, 3, 3, 0, 0, 3, 0, 0, 0, 0, 6, 6, 6, 0, 0, 0, 6, 6, 6, 6, 6, 6, 6, 0, 0, 0, 0, 0, 0, 6, 0, 6, 6, 0, 0, 0, 0, 0, 6, 0, 0, 0, 0, 0, 0,
    0,
];

/// The `pingBob*` blob properties of `ping.script` (the values shipped with the game by
/// default).
pub fn ping_blob_properties(src: Option<&dyn DataSource>) -> BlobProperties {
    let mut p = BlobProperties {
        density_low: 2.0e-12,
        density_high: 4.0e-12,
        start_sphere_size: 4000.0,
        radius_combine_margin: 3000.0,
        smallest_radius: 5000.0,
        biggest_radius: 10000.0,
        biggest_radius_default: 10000.0,
        overlap_factor: 0.5,
        sqrt_overlap_factor: st(ld(0.5f32).sqrt()),
    };
    if let Some(lines) = src.and_then(|s| crate::level::read_lines(s, "ping.script")) {
        for (k, v) in lines {
            let Some(x) = crate::statics::cscan::scan_f32(&v) else { continue };
            match k.to_ascii_lowercase().as_str() {
                "pingbobdensitylow" => p.density_low = x,
                "pingbobdensityhigh" => p.density_high = x,
                "pingbobsmallestradius" => p.smallest_radius = x,
                "pingbobbiggestradius" => {
                    p.biggest_radius = x;
                    p.biggest_radius_default = x;
                }
                "pingboboverlapfactor" => {
                    p.overlap_factor = x;
                    p.sqrt_overlap_factor = st(ld(x).sqrt());
                }
                "pingbobradiuscombinemargin" => p.radius_combine_margin = x,
                "pingbobstartspheresize" => p.start_sphere_size = x,
                _ => {}
            }
        }
    }
    p
}

/// `pingBattleBlobCriteria` (0x4e46b0): a ship (not dead, hidden, crazy or disabled) that was
/// recently attacked by someone other than the local player, or that is attacking, and whose
/// type the sensors do not leave out.
pub fn battle_ship(u_cur_player: usize, objs: &Objects, id: ShipId) -> bool {
    let s = objs.ships.ship(id);
    if s.flags & (sof::DISABLED | sof::CRAZY | sof::HIDE | sof::DEAD) != 0 {
        return false;
    }
    let attacked = s.recently_attacked != 0 && s.recent_attacker.and_then(|a| objs.ships.get(a)).is_some_and(|a| a.player != u_cur_player);
    if !attacked && !s.ship_is_attacking {
        return false;
    }
    SENSORS_TYPE_FLAGS.get(s.ship_type.index()).is_none_or(|f| f & 4 == 0)
}

/// Whether this update is a battle-ping frame (`univUpdateCounter & REFRESH_COLLBLOB_RATE ==
/// REFRESH_COLLBLOB_BATTLEPING_FRAME`).
pub fn is_battle_ping_frame(u: &Universe) -> bool {
    u.update_counter & u.tweaks.refresh_collblob_rate == u.tweaks.refresh_collblob_battleping_frame
}

/// The battle pings of the current collision blobs: one per sub-blob of every blob flagged
/// 0x40 or 0x80. Call it on [`is_battle_ping_frame`] frames, as the original does. The
/// universe is borrowed mutably only to walk the blobs' objects; it is not changed.
pub fn battle_pings(u: &mut Universe, props: &BlobProperties) -> Vec<BattlePing> {
    let statics = u.statics.clone();
    let cur = u.base.cur_player;
    let parents: Vec<_> = u.blobs.blobs.iter().filter(|b| b.flags & (btf::GETTING_ATTACKED | btf::ATTACKING) != 0).cloned().collect();
    let mut objs = Objects { ships: &mut u.ships, bullets: &mut u.bullets, missiles: &mut u.missiles };
    let mut out = Vec::new();
    for parent in &parents {
        for b in BlobList::sub_blobs(parent, &mut objs, &statics, props, &|o, id| battle_ship(cur, o, id)) {
            let ships = b.objects.iter().filter_map(|r| if let crate::object::ObjRef::Ship(id) = r { Some(*id) } else { None }).collect();
            out.push(BattlePing { centre: b.centre, radius: b.radius, ships });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensors_type_table_matches_the_binary_bits() {
        let left_out: Vec<usize> = (0..SENSORS_TYPE_FLAGS.len()).filter(|&i| SENSORS_TYPE_FLAGS[i] & 4 != 0).collect();
        assert_eq!(left_out[..5], [9, 11, 13, 15, 18]);
        assert_eq!(left_out.len(), 42);
    }

    #[test]
    fn ping_blob_properties_default_to_ping_script() {
        let p = ping_blob_properties(None);
        assert_eq!(p.start_sphere_size, 4000.0);
        assert_eq!(p.biggest_radius, 10000.0);
        assert!((p.sqrt_overlap_factor - 0.70710677).abs() < 1e-6);
    }
}
