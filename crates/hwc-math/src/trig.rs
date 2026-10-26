//! Emulation of the x87 `FSIN` / `FCOS` instructions.
//!
//! MSVC6 expands `sin()`/`cos()` inline to `FSIN`/`FCOS` (the binary has ~130 of each in game
//! code, including the physics integrators), and `fsin`/`fcos` in `Types.h` are thin
//! wrappers around the same instructions. Platform `sin` implementations differ from each
//! other and from the x87, so they cannot be used for deterministic simulation.
//!
//! The x87 reduces its argument with a 66-bit approximation of π
//! (`0xC90FDAA22168C234C · 2^-66`) and evaluates to a 64-bit mantissa; precision control does
//! not apply to these instructions. This module reduces with that same constant, exactly
//! (integer arithmetic), and evaluates the reduced argument in double-double arithmetic.
//!
//! The result is the correctly rounded `f64` of what the x87 computes, up to the x87's own
//! sub-ulp error. Two known, rare, sources of mismatch remain:
//! * the hardware result has 64 mantissa bits; when the original feeds it straight into a
//!   further register operation, that operation sees those extra bits, while callers here see
//!   an `f64` (affects only results that land within 2^-64 relative of a rounding boundary);
//! * arguments with `|x| >= 2^50` use a less exact reduction (the hardware handles up to
//!   2^63). Game angles are nowhere near that.

/// π/2 as the x87 knows it: `0xC90FDAA22168C234C · 2^-67`.
const PIO2_X87_MANT: i128 = 0xC_90FD_AA22_168C_234C;
const PIO2_X87_SHIFT: i32 = 67;
/// The x87 leaves arguments at or beyond 2^63 unreduced.
const TWO_POW_63: f64 = 9_223_372_036_854_775_808.0;

#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
struct Dd {
    hi: f64,
    lo: f64,
}

#[inline]
fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    Dd {
        hi: s,
        lo: (a - (s - bb)) + (b - bb),
    }
}

#[inline]
fn quick_two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd {
        hi: s,
        lo: b - (s - a),
    }
}

#[inline]
fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    Dd {
        hi: p,
        lo: a.mul_add(b, -p),
    }
}

impl Dd {
    fn add(self, o: Dd) -> Dd {
        let s = two_sum(self.hi, o.hi);
        let t = two_sum(self.lo, o.lo);
        let s = quick_two_sum(s.hi, s.lo + t.hi);
        quick_two_sum(s.hi, s.lo + t.lo)
    }

    fn mul(self, o: Dd) -> Dd {
        let p = two_prod(self.hi, o.hi);
        quick_two_sum(p.hi, p.lo + (self.hi * o.lo + self.lo * o.hi))
    }

    fn div_f64(self, b: f64) -> Dd {
        let q1 = self.hi / b;
        let r = self.add(neg(two_prod(q1, b)));
        let q2 = r.hi / b;
        let r = r.add(neg(two_prod(q2, b)));
        let q3 = r.hi / b;
        quick_two_sum(q1, q2).add(Dd { hi: q3, lo: 0.0 })
    }
}

#[inline]
fn neg(a: Dd) -> Dd {
    Dd { hi: -a.hi, lo: -a.lo }
}

/// Reduce `x` by the nearest multiple of the x87's π/2. Returns the reduced argument and
/// the quadrant (`k mod 4`).
fn reduce(x: f64) -> (Dd, u32) {
    // |x| < π/4 (any approximation): no reduction.
    if x.abs() < std::f64::consts::FRAC_PI_4 {
        return (Dd { hi: x, lo: 0.0 }, 0);
    }
    if x.abs() < 1.125_899_906_842_624e15 {
        // |x| < 2^50: x · 2^67 is an exact integer below 2^117.
        let scaled = (x * 2f64.powi(PIO2_X87_SHIFT)) as i128;
        let mut k = scaled / PIO2_X87_MANT;
        let mut r = scaled - k * PIO2_X87_MANT;
        // Round the quotient to nearest.
        if 2 * r > PIO2_X87_MANT {
            k += 1;
            r -= PIO2_X87_MANT;
        } else if 2 * r < -PIO2_X87_MANT {
            k -= 1;
            r += PIO2_X87_MANT;
        }
        let hi = r as f64;
        let lo = (r - hi as i128) as f64;
        let scale = 2f64.powi(-PIO2_X87_SHIFT);
        let red = quick_two_sum(hi * scale, lo * scale);
        (red, (k.rem_euclid(4)) as u32)
    } else {
        // Out of game range: approximate reduction in double-double.
        let pio2_hi = (PIO2_X87_MANT >> 15) as f64 * 2f64.powi(15 - PIO2_X87_SHIFT);
        let pio2_lo = (PIO2_X87_MANT & 0x7FFF) as f64 * 2f64.powi(-PIO2_X87_SHIFT);
        let k = (x / pio2_hi).round();
        let p = two_prod(k, pio2_hi).add(two_prod(k, pio2_lo));
        let red = Dd { hi: x, lo: 0.0 }.add(neg(p));
        (red, (k.rem_euclid(4.0)) as u32)
    }
}

fn sin_poly(r: Dd) -> Dd {
    // Taylor series; |r| <= ~0.786 so 15 terms reach well below 2^-106.
    let r2 = r.mul(r);
    let mut term = r;
    let mut sum = r;
    for k in 1..=15u32 {
        let d = f64::from((2 * k) * (2 * k + 1));
        term = neg(term.mul(r2).div_f64(d));
        sum = sum.add(term);
    }
    sum
}

fn cos_poly(r: Dd) -> Dd {
    let r2 = r.mul(r);
    let mut term = Dd { hi: 1.0, lo: 0.0 };
    let mut sum = term;
    for k in 1..=15u32 {
        let d = f64::from((2 * k - 1) * (2 * k));
        term = neg(term.mul(r2).div_f64(d));
        sum = sum.add(term);
    }
    sum
}

/// `FSIN` on a register value.
pub fn fsin(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() >= TWO_POW_63 {
        // Out of range: the x87 sets C2 and leaves the operand unchanged.
        return x;
    }
    let (r, q) = reduce(x);
    let v = match q {
        0 => sin_poly(r),
        1 => cos_poly(r),
        2 => neg(sin_poly(r)),
        _ => neg(cos_poly(r)),
    };
    v.hi + v.lo
}

/// `FCOS` on a register value.
pub fn fcos(x: f64) -> f64 {
    if !x.is_finite() {
        return f64::NAN;
    }
    if x.abs() >= TWO_POW_63 {
        return x;
    }
    let (r, q) = reduce(x);
    let v = match q {
        0 => cos_poly(r),
        1 => neg(sin_poly(r)),
        2 => neg(cos_poly(r)),
        _ => sin_poly(r),
    };
    v.hi + v.lo
}

/// `fsin(float)` from `Types.h`: loads a `float` and executes `FSIN`.
#[inline]
pub fn fsin_f32(x: f32) -> f64 {
    fsin(f64::from(x))
}

/// `fcos(float)` from `Types.h`: loads a `float` and executes `FCOS`.
#[inline]
pub fn fcos_f32(x: f32) -> f64 {
    fcos(f64::from(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values computed at 300-bit precision with the x87's 66-bit π reduction.
    const CASES: &[(f64, u64, u64)] = &[
        (0.1, 0x3fb98eaecb8bcb2c, 0x3fefd712f9a817c1),
        (0.5, 0x3fdeaee8744b05f0, 0x3fec1528065b7d50),
        (std::f64::consts::FRAC_PI_4, 0x3fe6a09e667f3bcc, 0x3fe6a09e667f3bcd),
        (0.785_398_185_253_143_3, 0x3fe6a09e6ecb41fe, 0x3fe6a09e5e333598),
        (1.0, 0x3feaed548f090cee, 0x3fe14a280fb5068c),
        (1.570_796_370_506_286_6, 0x3feffffffffffff7, 0xbe6777a5cf72cffe),
        (2.0, 0x3fed18f6ead1b446, 0xbfdaa22657537205),
        (3.141_592_741_012_573_2, 0xbe7777a5cf72cff8, 0xbfefffffffffffde),
        (-2.5, 0xbfe326af0dcfcab1, 0xbfe9a2f7ef858b7d),
        (10.0, 0xbfe1689ef5f34f52, 0xbfead9ac890c6b1f),
        (100.0, 0xbfe03425b78c4db8, 0x3feb981dbf665fdf),
        (1000.0, 0x3fea75cc150a206b, 0x3fe1ff026793f1bb),
        (6.283_185_482_025_146_5, 0x3e8777a5cf72cfde, 0x3fefffffffffff76),
        (12_345.678_710_937_5, 0xbfe683b269fe900f, 0x3fe6bd659a4d2ee3),
        (1e-8, 0x3e45798ee2308c3a, 0x3ff0000000000000),
        (-0.3, 0xbfd2e9cd95baba33, 0x3fee921dd42f09ba),
    ];

    #[test]
    fn matches_x87_model() {
        for &(x, s, c) in CASES {
            assert_eq!(fsin(x).to_bits(), s, "sin({x}) = {}", fsin(x));
            assert_eq!(fcos(x).to_bits(), c, "cos({x}) = {}", fcos(x));
        }
    }

    #[test]
    fn sin_of_float_pi_shows_x87_pi_error() {
        // True sin(PI_f32) is -8.742278000372482e-8 (0xbe7777a5cf72cffe); the x87 gives a
        // value 6 ulps away because its pi has only 66 bits.
        assert_eq!(fsin_f32(std::f32::consts::PI).to_bits(), 0xbe7777a5cf72cff8);
    }

    #[test]
    fn out_of_range_is_identity() {
        assert_eq!(fsin(1e19), 1e19);
        assert!(fsin(f64::INFINITY).is_nan());
    }
}
