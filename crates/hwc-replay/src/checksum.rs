//! The original's sync checksums, recomputed from a [`StateFrame`].
//!
//! When the original processes a sync packet it first computes (`netcheckShow`):
//!
//! * `univcheck`: `univGetChecksum`, the sum of the positions of every ship in the ship list
//!   without `SOF_Dead`. Three single-precision running sums (x, y, z) are kept, each addition
//!   done at 53-bit precision and rounded back to `f32`; the result is `(z + y) + x`, rounded
//!   to `f32` by the caller;
//! * `randcheck`: the low 16 bits of the next `RAN_Game` (stream 10) value. This *consumes* a
//!   value from the game stream once per packet, so a replay must draw it too;
//! * `statcheck`: `statsGetStatChecksum` (per-player statistics CRCs plus bounty values);
//!   not reproducible from a state frame.
//!
//! The frame to use is the one recorded after the last universe update before that packet.

use hwc_math::random::RAN_GAME;

use crate::state::StateFrame;

/// `univGetChecksum` over a frame: the checksum and the number of live ships summed.
pub fn univ_checksum(frame: &StateFrame) -> (f32, u32) {
    let (mut x, mut y, mut z) = (0f32, 0f32, 0f32);
    let mut count = 0;
    for s in frame.ships.iter().filter(|s| !s.is_dead()) {
        x = (x as f64 + s.position[0] as f64) as f32;
        y = (y as f64 + s.position[1] as f64) as f32;
        z = (z as f64 + s.position[2] as f64) as f32;
        count += 1;
    }
    let sum = (z as f64 + y as f64) + x as f64;
    (sum as f32, count)
}

/// The `randcheck` the original would draw next from this frame's game stream.
pub fn next_rand_check(frame: &StateFrame) -> Option<u16> {
    let mut s = *frame.streams.get(RAN_GAME)?;
    Some(s.next_u32() as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::frame;
    use crate::state::SHIP_FLAG_DEAD;
    use hwc_math::random::RanStream;

    #[test]
    fn sums_live_ships_in_order() {
        let mut f = frame(1, &[1.0, 2.0, 3.0]);
        // positions are (x, 2x, -x): x = 6, y = 12, z = -6 -> (z + y) + x = 12
        assert_eq!(univ_checksum(&f), (12.0, 3));
        f.ships[1].flags |= SHIP_FLAG_DEAD;
        assert_eq!(univ_checksum(&f), (8.0, 2));
    }

    #[test]
    fn running_sums_round_to_single() {
        // Each +1 (and +2 in y) is rounded away by the single-precision running sums; summing
        // in double would give 2e8 + 32 instead.
        let mut xs = vec![1.0e8];
        xs.extend([1.0; 16]);
        let f = frame(1, &xs);
        assert_eq!(univ_checksum(&f), (2.0e8, 17));
    }

    #[test]
    fn rand_check_does_not_advance_the_frame() {
        let f = frame(1, &[]);
        let mut s = RanStream::DEFAULT;
        assert_eq!(next_rand_check(&f), Some(s.next_u32() as u16));
        assert_eq!(f.streams[RAN_GAME], RanStream::DEFAULT);
    }
}
