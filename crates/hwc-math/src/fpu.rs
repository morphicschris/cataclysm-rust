//! Model of the x87 FPU as `cataclysm.exe` uses it.
//!
//! # Control word
//!
//! The game executable itself never calls `_controlfp`/`_control87` and contains no `FLDCW`
//! outside the CRT (`__ftol` and the math-library helpers save and restore it). The only
//! precision change in the image is in the copy-protection loader that wraps the real entry
//! point (`.cms_t` section): its CRT start-up calls `_controlfp(_PC_53, _MCW_PC)`. That is also
//! the Win32 default control word (`0x027F`: 53-bit precision, round to nearest, all
//! exceptions masked), so the simulation runs with **53-bit precision control**.
//!
//! The renderer DLLs could in principle change it (Direct3D 6/7 drops the FPU to 24-bit
//! precision unless told to preserve it), but the task scheduler proves the game cannot be
//! running at 24 bits: `taskExecuteAllPending` converts scaled ticks to an integer with the
//! `x + 1.5·2^52` trick (see [`round_magic`]), which only works when additions keep 53 bits.
//! At 24 bits the game clock would never advance.
//!
//! # What that means for a reimplementation
//!
//! With precision control at 53 bits every x87 arithmetic instruction (`FADD`, `FSUB`,
//! `FMUL`, `FDIV`, `FSQRT`, conversions) rounds its result exactly as an IEEE `f64`
//! operation would. The remaining differences between x87@53 and `f64` are confined to the
//! wider exponent range of the registers (no overflow/underflow inside a register) and do not
//! arise for game-scale values. So:
//!
//! * a value held in an FPU register is modelled as `f64` (alias [`Ext`]);
//! * loading a `float` is exact ([`ld`]); storing to a `float` rounds to nearest ([`st`]);
//! * a single `f32` operation whose result is immediately stored (e.g. `a.x = b.x + c.x`) is
//!   identical to Rust `f32` arithmetic, because double rounding through `f64` is innocuous
//!   for `+ - * / sqrt` on `f32` operands;
//! * expressions with more than one operation must be evaluated in `f64` and rounded once at
//!   the store, in the order the binary uses.
//!
//! Transcendental instructions (`FSIN`, `FCOS`, ...) are *not* affected by precision control:
//! they always produce a 64-bit-mantissa result. See [`crate::trig`].
//!
//! # Comparisons
//!
//! MSVC6 compiles float comparisons to `FCOM` + `FNSTSW` + `TEST AH,mask`. For an unordered
//! comparison (a NaN operand) the FPU sets C0, C2 and C3 together, so the outcome depends on
//! the mask: `==` tests (mask `0x40`) and `<` tests (mask `0x01`) come out **true** for NaN,
//! while `>`/`<=` tests that use mask `0x41` come out the same as IEEE. The helpers below make
//! those cases explicit; use the one matching the instruction pattern at the call site.

/// An x87 register value at 53-bit precision control.
pub type Ext = f64;

/// Load a `float` from memory into an FPU register (exact).
#[inline]
pub fn ld(x: f32) -> Ext {
    x as f64
}

/// Store an FPU register to a `float` in memory (round to nearest even).
#[inline]
pub fn st(x: Ext) -> f32 {
    x as f32
}

/// `a == b` as compiled with `TEST AH,0x40`: true when equal **or unordered**.
#[inline]
pub fn eq_or_unordered(a: Ext, b: Ext) -> bool {
    a == b || a.is_nan() || b.is_nan()
}

/// `a < b` as compiled with `TEST AH,0x01` (C0 only): true when less **or unordered**.
#[inline]
pub fn lt_or_unordered(a: Ext, b: Ext) -> bool {
    a < b || a.is_nan() || b.is_nan()
}

/// MSVC6 `__ftol`: truncate toward zero with `FISTP qword` and return all 64 bits.
///
/// NaN and values outside the `i64` range give the x87 "integer indefinite"
/// `0x8000_0000_0000_0000`. C code that converts to a 32-bit integer keeps only the low
/// dword (see [`ftol_i32`] / [`ftol_u32`]).
#[inline]
pub fn ftol(x: Ext) -> i64 {
    // 2^63 is exactly representable; anything at or beyond it (or NaN) is indefinite.
    const LIMIT: f64 = 9_223_372_036_854_775_808.0;
    if x.is_nan() || x >= LIMIT || x < -LIMIT {
        i64::MIN
    } else {
        x.trunc() as i64
    }
}

/// `(sdword)x` / `(int)x`: low 32 bits of [`ftol`].
///
/// Note that NaN and huge values therefore give `0` (low dword of `0x8000_0000_0000_0000`),
/// and values in `[2^31, 2^63)` wrap modulo 2^32 rather than saturating.
#[inline]
pub fn ftol_i32(x: Ext) -> i32 {
    ftol(x) as i32
}

/// `(udword)x`: low 32 bits of [`ftol`] (so `-1.5` gives `0xFFFF_FFFF`).
#[inline]
pub fn ftol_u32(x: Ext) -> u32 {
    ftol(x) as u32
}

/// The `x + 1.5·2^52` rounding trick used by `taskExecuteAllPending` (Cataclysm):
/// the sum is stored as a double and its low dword read back as an integer, i.e. `x`
/// rounded to the nearest integer (ties to even), modulo 2^32.
#[inline]
pub fn round_magic(x: Ext) -> i32 {
    const MAGIC: f64 = 6_755_399_441_055_744.0; // 1.5 * 2^52
    let sum = x + MAGIC;
    sum.to_bits() as u32 as i32
}

/// `FILD dword`: signed 32-bit integer to register (exact).
#[inline]
pub fn fild_i32(x: i32) -> Ext {
    x as f64
}

/// Unsigned 32-bit integer to register as MSVC6 does it (`FILD qword` of the
/// zero-extended value; exact).
#[inline]
pub fn fild_u32(x: u32) -> Ext {
    x as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ftol_truncates_and_wraps_like_fistp_qword() {
        assert_eq!(ftol(1.9), 1);
        assert_eq!(ftol(-1.9), -1);
        assert_eq!(ftol_i32(-0.5), 0);
        assert_eq!(ftol_u32(-1.5), 0xFFFF_FFFF);
        assert_eq!(ftol_u32(3_000_000_000.7), 3_000_000_000);
        assert_eq!(ftol_i32(3_000_000_000.0), 3_000_000_000u32 as i32);
        assert_eq!(ftol(f64::NAN), i64::MIN);
        assert_eq!(ftol(1e300), i64::MIN);
        assert_eq!(ftol(-1e300), i64::MIN);
        assert_eq!(ftol(-9_223_372_036_854_775_808.0), i64::MIN);
        assert_eq!(ftol_i32(f64::NAN), 0);
        assert_eq!(ftol_u32(f64::INFINITY), 0);
        assert_eq!(ftol(4_294_967_296.5), 4_294_967_296);
        assert_eq!(ftol_u32(4_294_967_296.5), 0);
    }

    #[test]
    fn magic_rounding_is_ties_to_even() {
        assert_eq!(round_magic(2.5), 2);
        assert_eq!(round_magic(3.5), 4);
        assert_eq!(round_magic(-2.5), -2);
        assert_eq!(round_magic(15.0 * 0.25), 4);
        assert_eq!(round_magic(7.0 * 0.25), 2);
        assert_eq!(round_magic(0.49), 0);
    }

    #[test]
    fn unordered_compare_helpers() {
        assert!(eq_or_unordered(f64::NAN, 0.0));
        assert!(!eq_or_unordered(1.0, 0.0));
        assert!(lt_or_unordered(f64::NAN, 1.0));
        assert!(!lt_or_unordered(2.0, 1.0));
    }
}
