//! Table-driven square root (`FastMath.c`: `fmathInitSqrt`, `fmathSqrtDouble`).
//!
//! The game uses this approximation for `fsqrt()`/`fmathSqrt()` everywhere (vector lengths,
//! normalisation, capping), so reproducing it exactly matters far more than accuracy.
//!
//! How it works: the 16384-entry table holds the top 20 mantissa bits of `sqrt(m)` for
//! `m` in `[1,2)` (first half) and `[2,4)` (second half), indexed by the exponent parity and
//! the top 13 mantissa bits of the input. The result keeps the input's **low 32 bits of
//! mantissa unchanged** (only the high dword is rebuilt), which is a quirk of the original
//! and is reproduced here. Other quirks reproduced:
//! * the zero test is the x87 `== 0.0` pattern, so **NaN returns `0.0`** (as does `-0.0`);
//! * negative inputs are not rejected: the sign bit flows through the exponent arithmetic.

use std::sync::OnceLock;

const TABLE_SIZE: usize = 16384;
const MANT_SHIFTS: u32 = 7;
const EXP_BIAS: u32 = 1023;
const EXP_SHIFTS: u32 = 20;
const EXP_LSB: u32 = 0x0010_0000;
const MANT_MASK: u32 = 0x000F_FFFF;

fn table() -> &'static [u32; TABLE_SIZE] {
    static TABLE: OnceLock<Box<[u32; TABLE_SIZE]>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([0u32; TABLE_SIZE]);
        for i in 0..(TABLE_SIZE / 2) as u32 {
            // High dword only; the low dword of the input double is zero.
            // `FSQRT` at 53-bit precision is the correctly rounded IEEE square root.
            let even = f64::from_bits(u64::from((i << MANT_SHIFTS) | (EXP_BIAS << EXP_SHIFTS)) << 32);
            t[i as usize] = ((even.sqrt().to_bits() >> 32) as u32) & MANT_MASK;
            let odd =
                f64::from_bits(u64::from((i << MANT_SHIFTS) | ((EXP_BIAS + 1) << EXP_SHIFTS)) << 32);
            t[i as usize + TABLE_SIZE / 2] = ((odd.sqrt().to_bits() >> 32) as u32) & MANT_MASK;
        }
        t
    })
}

/// One raw table entry (exposed for verification against the original).
pub fn sqrt_table_entry(index: usize) -> u32 {
    table()[index]
}

/// `fmathSqrtDouble`: the game's approximate square root, on a register value.
pub fn fmath_sqrt_double(f: f64) -> f64 {
    if crate::fpu::eq_or_unordered(f, 0.0) {
        return 0.0;
    }
    let bits = f.to_bits();
    let lo = bits as u32;
    let hi = (bits >> 32) as u32;
    let e = (hi >> EXP_SHIFTS).wrapping_sub(EXP_BIAS);
    let mut m = hi & MANT_MASK;
    if e & 1 != 0 {
        m |= EXP_LSB;
    }
    let e = e >> 1;
    let new_hi = table()[(m >> MANT_SHIFTS) as usize] | (e.wrapping_add(EXP_BIAS) << EXP_SHIFTS);
    f64::from_bits((u64::from(new_hi) << 32) | u64::from(lo))
}

/// `fsqrt(x)` as the game writes it. The `(real32)` casts in the macro are not honoured by
/// MSVC6 for register values, so the argument is the full-precision register value and the
/// result stays in a register; round with [`crate::fpu::st`] where the original stores it.
#[inline]
pub fn fsqrt(x: f64) -> f64 {
    fmath_sqrt_double(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table entries read back after emulating `fmathInitSqrt` in cataclysm.exe (Ghidra
    /// p-code emulation; the run covered indices 0..=3572 of each half).
    #[test]
    fn table_matches_emulated_binary() {
        let samples: &[(usize, u32)] = &[
            (0, 0x0),
            (1, 0x3f),
            (2, 0x7f),
            (3, 0xbf),
            (100, 0x18ec),
            (511, 0x7dd1),
            (1000, 0xf2cd),
            (1234, 0x129ae),
            (2047, 0x1e33e),
            (3000, 0x2b39c),
            (3500, 0x31d62),
            (8192, 0x6a09e),
            (8193, 0x6a0f8),
            (8300, 0x6c6ad),
            (9000, 0x7b792),
            (10000, 0x90000),
            (11000, 0xa3860),
            (11700, 0xb0aa8),
        ];
        for &(i, v) in samples {
            assert_eq!(sqrt_table_entry(i), v, "table[{i}]");
        }
    }

    /// Results of emulating `fmathSqrtDouble` in cataclysm.exe with the emulated table.
    #[test]
    fn sqrt_matches_emulated_binary() {
        let cases: &[(f64, u64)] = &[
            (2.0, 0x3ff6a09e00000000),
            (1.2, 0x3ff186da33333333),
            (4.8, 0x400186da33333333),
            (0.3, 0x3fe186da33333333),
            (2.4, 0x3ff8c95d33333333),
            (9.0, 0x4008000000000000),
            (0.6, 0x3fe8c95d33333333),
            (1.0000001, 0x3ff000001ad7f29b),
            (0.001, 0x3fa030b5d2f1a9fc),
            (-4.0, 0x8000000000000000),
            (0.0, 0x0000000000000000),
        ];
        for &(x, bits) in cases {
            assert_eq!(fmath_sqrt_double(x).to_bits(), bits, "sqrt({x})");
        }
    }

    #[test]
    fn nan_counts_as_zero() {
        assert_eq!(fmath_sqrt_double(f64::NAN).to_bits(), 0);
        assert_eq!(fmath_sqrt_double(-0.0).to_bits(), 0);
    }

    #[test]
    fn approximation_is_close() {
        for &x in &[1.0f64, 2.0, 3.0, 10.0, 12345.678, 1e-6, 7.0 / 3.0] {
            let rel = (fmath_sqrt_double(x) - x.sqrt()).abs() / x.sqrt();
            assert!(rel < 1e-4, "{x}: {rel}");
        }
    }
}
