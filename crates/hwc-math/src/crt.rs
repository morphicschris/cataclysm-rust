//! MSVC6 C runtime behaviour the game relies on.
//!
//! * [`qsort`] — the VC6 `qsort` (0x66b980) is an unstable quicksort; elements that compare
//!   equal (or comparators that never return 0) end up in an order determined by the exact
//!   algorithm, and the simulation sorts things like collision-blob lists and formation
//!   slots with it. This is a line-for-line port of its control flow.
//! * [`MsvcRand`] — the CRT `rand()` (per-thread LCG). In Cataclysm it is only referenced from
//!   a function-pointer table outside the simulation, but it is cheap to have.
//! * `__ftol` lives in [`crate::fpu::ftol`].

use std::cmp::Ordering;

/// Below this many elements `qsort` switches to its selection sort.
const CUTOFF: usize = 8;

/// MSVC6 `qsort` over a slice, with a C-style comparator (`< 0`, `0`, `> 0`).
///
/// Comparator calls happen in exactly the original order, so stateful or inconsistent
/// comparators (several in the game never return 0) behave identically too.
pub fn qsort<T, F>(base: &mut [T], mut comp: F)
where
    F: FnMut(&T, &T) -> i32,
{
    let num = base.len();
    if num < 2 {
        return;
    }
    // Explicit stack of pending (lo, hi) ranges, inclusive; 30 entries suffice in the CRT.
    let mut lostk: Vec<usize> = Vec::with_capacity(30);
    let mut histk: Vec<usize> = Vec::with_capacity(30);
    let mut lo = 0usize;
    let mut hi = num - 1;

    loop {
        // "recurse:" label.
        let size = hi - lo + 1;
        if size <= CUTOFF {
            shortsort(base, lo, hi, &mut comp);
        } else {
            let mid = lo + size / 2;
            base.swap(mid, lo);
            let mut loguy = lo;
            let mut higuy = hi + 1;
            loop {
                loop {
                    loguy += 1;
                    if loguy > hi || comp(&base[loguy], &base[lo]) > 0 {
                        break;
                    }
                }
                loop {
                    higuy -= 1;
                    if higuy <= lo || comp(&base[higuy], &base[lo]) < 0 {
                        break;
                    }
                }
                if higuy < loguy {
                    break;
                }
                base.swap(loguy, higuy);
            }
            base.swap(lo, higuy);

            // Byte-distance test of the original, `(higuy-1) - lo < hi - loguy`, reduces to
            // this in element units (loguy may be hi + 1, making the right side negative).
            if (higuy as isize - lo as isize) <= (hi as isize - loguy as isize) {
                if loguy < hi {
                    lostk.push(loguy);
                    histk.push(hi);
                }
                if lo + 1 < higuy {
                    hi = higuy - 1;
                    continue;
                }
            } else {
                if lo + 1 < higuy {
                    lostk.push(lo);
                    histk.push(higuy - 1);
                }
                if loguy < hi {
                    lo = loguy;
                    continue;
                }
            }
        }
        // Pop the next pending range.
        match (lostk.pop(), histk.pop()) {
            (Some(l), Some(h)) => {
                lo = l;
                hi = h;
            }
            _ => return,
        }
    }
}

/// The CRT's `shortsort`: repeatedly move the maximum to the end (first maximum wins ties).
fn shortsort<T, F>(base: &mut [T], lo: usize, mut hi: usize, comp: &mut F)
where
    F: FnMut(&T, &T) -> i32,
{
    while hi > lo {
        let mut max = lo;
        for p in lo + 1..=hi {
            if comp(&base[p], &base[max]) > 0 {
                max = p;
            }
        }
        base.swap(max, hi);
        hi -= 1;
    }
}

/// Convenience: [`qsort`] with an [`Ordering`]-returning comparator.
pub fn qsort_by<T, F>(base: &mut [T], mut cmp: F)
where
    F: FnMut(&T, &T) -> Ordering,
{
    qsort(base, |a, b| cmp(a, b) as i32);
}

/// MSVC `rand()`: `state = state * 214013 + 2531011`, returns bits 16..30.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MsvcRand {
    pub state: u32,
}

impl Default for MsvcRand {
    /// The CRT's initial seed is 1.
    fn default() -> Self {
        Self { state: 1 }
    }
}

impl MsvcRand {
    /// `srand(seed)`.
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// `rand()`: 0..=0x7FFF.
    pub fn rand(&mut self) -> i32 {
        self.state = self.state.wrapping_mul(214_013).wrapping_add(2_531_011);
        ((self.state >> 16) & 0x7FFF) as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `smBlobListSort` from the game: descending by key, never returns 0.
    fn blob_cmp(a: &(u32, f32), b: &(u32, f32)) -> i32 {
        if b.1 < a.1 {
            -1
        } else {
            1
        }
    }

    fn run(keys: &[u32]) -> Vec<u32> {
        let mut v: Vec<(u32, f32)> = keys.iter().enumerate().map(|(i, &k)| (i as u32, k as f32)).collect();
        qsort(&mut v, blob_cmp);
        v.iter().map(|e| e.0).collect()
    }

    /// Orders produced by emulating the binary's `qsort` with `smBlobListSort` on records
    /// tagged with their original index.
    #[test]
    fn tie_order_matches_emulated_binary() {
        assert_eq!(run(&[1, 4, 4, 1, 2]), vec![2, 1, 4, 0, 3]);
        assert_eq!(run(&[4, 3, 4, 0, 4, 0, 3, 2, 4]), vec![4, 0, 2, 8, 1, 6, 7, 3, 5]);
        assert_eq!(
            run(&[1, 1, 3, 4, 4, 3, 3, 1, 1, 1, 4, 3, 0, 0, 1, 4, 0, 2, 0, 2, 3, 4, 3]),
            vec![10, 21, 15, 3, 4, 11, 6, 22, 20, 2, 5, 19, 17, 14, 9, 7, 1, 0, 8, 18, 13, 16, 12]
        );
        assert_eq!(
            run(&[
                3, 3, 4, 3, 1, 2, 0, 0, 1, 3, 1, 2, 3, 2, 3, 4, 3, 4, 2, 4, 4, 3, 4, 1, 2, 0, 2, 4, 1, 2, 4, 4, 4,
                0, 1, 4, 2, 2, 0, 0
            ]),
            vec![
                20, 30, 15, 35, 32, 31, 2, 27, 22, 19, 17, 0, 9, 3, 14, 12, 16, 21, 1, 26, 36, 29, 13, 5, 24, 37,
                11, 18, 34, 28, 10, 23, 4, 8, 6, 33, 7, 25, 38, 39
            ]
        );
    }

    #[test]
    fn sorts_large_inputs() {
        let mut r = MsvcRand::default();
        let mut v: Vec<i32> = (0..5000).map(|_| r.rand()).collect();
        let mut expected = v.clone();
        expected.sort();
        qsort(&mut v, |a, b| a - b);
        assert_eq!(v, expected);
        let mut desc: Vec<i32> = (0..1000).rev().collect();
        qsort_by(&mut desc, |a, b| a.cmp(b));
        assert!(desc.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn msvc_rand_sequence() {
        let mut r = MsvcRand::default();
        let v: Vec<i32> = (0..5).map(|_| r.rand()).collect();
        assert_eq!(v, vec![41, 18467, 6334, 26500, 19169]);
    }
}
