//! 3- and 4-component vectors (`Vector.h` / `Vector.c`).
//!
//! The `vec*` macros of the original are inlined into their callers, where the compiler may
//! keep intermediates in registers or re-associate sums. The helpers ending in `_ext` return
//! the register (`f64`) value of the macro's expression in source order, so a caller can
//! reproduce what its own call site does; check the disassembly of that site for the order.
//! The out-of-line functions (`vecNormalize`, `vecCapVector`, ...) are reproduced exactly as
//! compiled in `cataclysm.exe`.

use crate::fastmath::fsqrt;
use crate::fpu::{eq_or_unordered, ld, lt_or_unordered, st, Ext};

/// `vector`: three `real32`s.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// `hvector`: homogeneous four-component vector.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct HVector {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl HVector {
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// `vecMakeHVecFromVec`: `w = 1`.
    pub const fn from_vector(v: Vector) -> Self {
        Self::new(v.x, v.y, v.z, 1.0)
    }

    /// `vecGrabVecFromHVec`.
    pub const fn xyz(self) -> Vector {
        Vector::new(self.x, self.y, self.z)
    }

    /// `vecHomogenize`: divide by `w` unless `w` is 0 (or NaN) or exactly 1.
    pub fn homogenize(self) -> Vector {
        let mut dst = self.xyz();
        // `w == 0.0f` uses the unordered-is-equal pattern; `w == 1.0f` is an integer compare
        // of the bit pattern 0x3F800000.
        if eq_or_unordered(ld(self.w), 0.0) || self.w.to_bits() == 0x3F80_0000 {
            return dst;
        }
        let one_over_w = 1.0 / ld(self.w);
        dst.x = st(one_over_w * ld(dst.x));
        dst.y = st(one_over_w * ld(dst.y));
        dst.z = st(one_over_w * ld(dst.z));
        dst
    }
}

impl Vector {
    pub const ZERO: Vector = Vector::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// `vecScalarMultiply` / `vecMultiplyByScalar`.
    #[inline]
    pub fn scale(self, k: f32) -> Vector {
        Vector::new(self.x * k, self.y * k, self.z * k)
    }

    /// `vecAddToScalarMultiply`: `dst += v * k`, product and sum in a register.
    #[inline]
    pub fn add_scaled(self, v: Vector, k: f32) -> Vector {
        Vector::new(
            st(ld(self.x) + ld(v.x) * ld(k)),
            st(ld(self.y) + ld(v.y) * ld(k)),
            st(ld(self.z) + ld(v.z) * ld(k)),
        )
    }

    /// `vecSubFromScalarMultiply`: `dst -= v * k`.
    #[inline]
    pub fn sub_scaled(self, v: Vector, k: f32) -> Vector {
        Vector::new(
            st(ld(self.x) - ld(v.x) * ld(k)),
            st(ld(self.y) - ld(v.y) * ld(k)),
            st(ld(self.z) - ld(v.z) * ld(k)),
        )
    }

    /// `vecScalarDivide` / `vecDivideByScalar`: `tmp = 1/k` is a `real32` local in the
    /// macro; pass whether the call site actually stored it (`true` = rounded to `f32`).
    pub fn div_scalar(self, k: f32, tmp_stored: bool) -> Vector {
        let r = 1.0 / ld(k);
        let r = if tmp_stored { ld(st(r)) } else { r };
        Vector::new(st(ld(self.x) * r), st(ld(self.y) * r), st(ld(self.z) * r))
    }

    /// `vecDotProduct` in source order: `(ax*bx + ay*by) + az*bz`, as a register value.
    #[inline]
    pub fn dot_ext(self, b: Vector) -> Ext {
        (ld(self.x) * ld(b.x) + ld(self.y) * ld(b.y)) + ld(self.z) * ld(b.z)
    }

    /// `vecDotProduct` stored to a `real32`.
    #[inline]
    pub fn dot(self, b: Vector) -> f32 {
        st(self.dot_ext(b))
    }

    /// `vecMagnitudeSquared` in source order: `(x*x + y*y) + z*z` (also the order every
    /// out-of-line `Vector.c` function uses).
    #[inline]
    pub fn mag_sq_ext(self) -> Ext {
        (ld(self.x) * ld(self.x) + ld(self.y) * ld(self.y)) + ld(self.z) * ld(self.z)
    }

    /// `vecMagnitudeSquared` stored to a `real32`.
    #[inline]
    pub fn mag_sq(self) -> f32 {
        st(self.mag_sq_ext())
    }

    /// `fsqrt(vecMagnitudeSquared(v))` as a register value (the usual length idiom).
    #[inline]
    pub fn mag_ext(self) -> Ext {
        fsqrt(self.mag_sq_ext())
    }

    /// `vecCrossProduct` with each component computed in a register and stored.
    #[inline]
    pub fn cross(self, b: Vector) -> Vector {
        let a = self;
        Vector::new(
            st(ld(a.y) * ld(b.z) - ld(a.z) * ld(b.y)),
            st(ld(a.z) * ld(b.x) - ld(a.x) * ld(b.z)),
            st(ld(a.x) * ld(b.y) - ld(a.y) * ld(b.x)),
        )
    }

    /// `vecAreEqual` (IEEE equality; the `&&` chain uses the ordered compare).
    #[inline]
    pub fn are_equal(self, b: Vector) -> bool {
        self.x == b.x && self.y == b.y && self.z == b.z
    }

    /// `vecIsZero`.
    #[inline]
    pub fn is_zero(self) -> bool {
        self.x == 0.0 && self.y == 0.0 && self.z == 0.0
    }

    /// Multiply every component by a register value and store.
    #[inline]
    fn scale_ext(&mut self, k: Ext) {
        self.x = st(k * ld(self.x));
        self.y = st(k * ld(self.y));
        self.z = st(k * ld(self.z));
    }

    /// `vecNormalize` (0x54b110). A zero-length (or NaN-length) vector is left unchanged.
    pub fn normalize(&mut self) {
        let mag = self.mag_ext();
        if eq_or_unordered(mag, 0.0) {
            return;
        }
        self.scale_ext(1.0 / mag);
    }

    /// Returns a normalised copy (see [`Vector::normalize`]).
    pub fn normalized(self) -> Vector {
        let mut v = self;
        v.normalize();
        v
    }

    /// `vecCopyAndNormalize` (0x54b1c0): no zero check (a zero vector becomes NaN).
    pub fn copy_and_normalize(self) -> Vector {
        let r = 1.0 / self.mag_ext();
        let mut v = self;
        v.scale_ext(r);
        v
    }

    /// `vecNormalizeToLength` (0x54b220).
    pub fn normalize_to_length(&mut self, length: f32) {
        let ratio = ld(length) / self.mag_ext();
        self.scale_ext(ratio);
    }

    /// `vecCapVectorSloppy` (0x54b270): clamp each component to `[-max, max]`.
    pub fn cap_sloppy(&mut self, max_magnitude: f32) {
        let neg_max = -max_magnitude;
        for c in [&mut self.x, &mut self.y, &mut self.z] {
            if *c > max_magnitude {
                *c = max_magnitude;
            } else if *c < neg_max {
                *c = neg_max;
            }
        }
    }

    /// `vecCapVector` (0x54b2f0): scale down to `max_magnitude` if longer.
    pub fn cap(&mut self, max_magnitude: f32) {
        let mag = self.mag_ext();
        if mag > ld(max_magnitude) {
            self.scale_ext(ld(max_magnitude) / mag);
        }
    }

    /// `vecCapVectorWithMag`: as [`Vector::cap`] with a caller-supplied magnitude.
    ///
    /// Not present out of line in `cataclysm.exe` (the function at 0x54b350 that the naming
    /// pass called `vecCapVectorWithMag` is really `vecCapMinVector`); kept for callers
    /// that inline the HW1 logic.
    pub fn cap_with_mag(&mut self, max_magnitude: f32, actual_mag: f32) {
        if actual_mag > max_magnitude {
            self.scale_ext(ld(max_magnitude) / ld(actual_mag));
        }
    }

    /// `vecCapMinVector` (0x54b350): scale up to `min_magnitude` if shorter. The `<` test
    /// is the unordered-is-true pattern.
    pub fn cap_min(&mut self, min_magnitude: f32) {
        let mag = self.mag_ext();
        if lt_or_unordered(mag, ld(min_magnitude)) {
            self.scale_ext(ld(min_magnitude) / mag);
        }
    }

    /// `vecCapMinMaxVector` (0x54b3b0). Both tests use the magnitude measured on entry.
    pub fn cap_min_max(&mut self, min_magnitude: f32, max_magnitude: f32) {
        let mag = self.mag_ext();
        if lt_or_unordered(mag, ld(min_magnitude)) {
            self.scale_ext(ld(min_magnitude) / mag);
        }
        if mag > ld(max_magnitude) {
            self.scale_ext(ld(max_magnitude) / mag);
        }
    }

    /// `getVectDistSloppy`: largest absolute component (HW1; inlined in Cataclysm).
    pub fn dist_sloppy(self) -> f32 {
        let a = |v: f32| if v < 0.0 { -v } else { v };
        let (x, y, z) = (a(self.x), a(self.y), a(self.z));
        if x > y {
            if x > z {
                x
            } else {
                z
            }
        } else if y > z {
            y
        } else {
            z
        }
    }

    /// `vecLineIntersectWithXYPlane` (0x54b4b0): `k` stays in a register.
    pub fn line_intersect_xy_plane(p1: Vector, p2: Vector, zp: f32) -> Vector {
        let k = (ld(zp) - ld(p1.z)) / (ld(p2.z) - ld(p1.z));
        Vector::new(
            st((ld(p2.x) - ld(p1.x)) * k + ld(p1.x)),
            st((ld(p2.y) - ld(p1.y)) * k + ld(p1.y)),
            zp,
        )
    }

    /// `vecLineIntersectWithYZPlane` (0x54b4f0).
    pub fn line_intersect_yz_plane(p1: Vector, p2: Vector, xp: f32) -> Vector {
        let k = (ld(xp) - ld(p1.x)) / (ld(p2.x) - ld(p1.x));
        Vector::new(
            xp,
            st((ld(p2.y) - ld(p1.y)) * k + ld(p1.y)),
            st((ld(p2.z) - ld(p1.z)) * k + ld(p1.z)),
        )
    }

    /// `vecLineIntersectWithXZPlane` (0x54b530).
    pub fn line_intersect_xz_plane(p1: Vector, p2: Vector, yp: f32) -> Vector {
        let k = (ld(yp) - ld(p1.y)) / (ld(p2.y) - ld(p1.y));
        Vector::new(
            st((ld(p2.x) - ld(p1.x)) * k + ld(p1.x)),
            yp,
            st((ld(p2.z) - ld(p1.z)) * k + ld(p1.z)),
        )
    }

    /// `vecLineIntersectWithPlane` (0x54b570): intersection of the line through `line` with
    /// direction `dir` and the plane through `plane` with normal `normal`. The difference
    /// vector stays in registers and the dot products are summed z, x, y.
    pub fn line_intersect_with_plane(plane: Vector, normal: Vector, line: Vector, dir: Vector) -> Vector {
        let value = (ld(normal.z) * ld(dir.z) + ld(normal.x) * ld(dir.x)) + ld(dir.y) * ld(normal.y);
        let tx = ld(line.x) - ld(plane.x);
        let ty = ld(line.y) - ld(plane.y);
        let tz = ld(line.z) - ld(plane.z);
        let num = (tz * ld(normal.z) + tx * ld(normal.x)) + ty * ld(normal.y);
        let t = -(num / value);
        Vector::new(
            st(t * ld(dir.x) + ld(line.x)),
            st(t * ld(dir.y) + ld(line.y)),
            st(t * ld(dir.z) + ld(line.z)),
        )
    }

    /// `vecVectorsBlend` (0x54b5e0): `start*(1-f) + end*f`.
    pub fn blend(start: Vector, end: Vector, factor: f32) -> Vector {
        let omf = 1.0 - ld(factor);
        Vector::new(
            st(omf * ld(start.x) + ld(factor) * ld(end.x)),
            st(omf * ld(start.y) + ld(factor) * ld(end.y)),
            st(omf * ld(start.z) + ld(factor) * ld(end.z)),
        )
    }

    /// Cataclysm addition at 0x54b700: distance between two points, `fsqrt` of the squared
    /// difference summed z, y, x (register result).
    pub fn distance_ext(a: Vector, b: Vector) -> Ext {
        let dx = ld(a.x) - ld(b.x);
        let dy = ld(a.y) - ld(b.y);
        let dz = ld(a.z) - ld(b.z);
        fsqrt((dz * dz + dy * dy) + dx * dx)
    }

    /// Cataclysm addition at 0x54b630: unit normal of the triangle `(a, b, c)`,
    /// `normalize((a - b) × (c - b))`. The magnitude uses the *unrounded* z component of
    /// the cross product (it is still in a register when the length is computed).
    pub fn triangle_normal(a: Vector, b: Vector, c: Vector) -> Vector {
        let ux = ld(a.x) - ld(b.x);
        let uy = ld(a.y) - ld(b.y);
        let uz = ld(a.z) - ld(b.z);
        // The original stores (c - b).x and .y to float locals; .z stays in a register.
        let vx = ld(st(ld(c.x) - ld(b.x)));
        let vy = ld(st(ld(c.y) - ld(b.y)));
        let vz = ld(c.z) - ld(b.z);
        let nx = st(vz * uy - vy * uz);
        let ny = st(uz * vx - vz * ux);
        let nz_ext = ux * vy - vx * uy;
        let mut out = Vector::new(nx, ny, st(nz_ext));
        let mag = fsqrt((nz_ext * nz_ext + ld(ny) * ld(ny)) + ld(nx) * ld(nx));
        if eq_or_unordered(mag, 0.0) {
            return out;
        }
        out.scale_ext(1.0 / mag);
        out
    }
}

/// `vecAdd` (each component is one operation, so plain `f32` arithmetic is exact).
impl std::ops::Add for Vector {
    type Output = Vector;
    #[inline]
    fn add(self, o: Vector) -> Vector {
        Vector::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

/// `vecSub`.
impl std::ops::Sub for Vector {
    type Output = Vector;
    #[inline]
    fn sub(self, o: Vector) -> Vector {
        Vector::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

/// `vecNegate` / `vecCopyAndNegate`.
impl std::ops::Neg for Vector {
    type Output = Vector;
    #[inline]
    fn neg(self) -> Vector {
        Vector::new(-self.x, -self.y, -self.z)
    }
}

/// `vecScalarMultiply` / `vecMultiplyByScalar`.
impl std::ops::Mul<f32> for Vector {
    type Output = Vector;
    #[inline]
    fn mul(self, k: f32) -> Vector {
        self.scale(k)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(v: Vector) -> [u32; 3] {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
    }

    /// `vecNormalize` results from emulating cataclysm.exe (p-code emulation, which models
    /// the FPU at 64-bit precision; these cases agree at 53 bits too).
    #[test]
    fn normalize_matches_emulated_binary() {
        let cases: &[([f32; 3], [u32; 3])] = &[
            ([1.0, 1.0, 0.0], [0x3f3504f6, 0x3f3504f6, 0x00000000]),
            ([0.3, 0.4, 0.0], [0x3f19999a, 0x3f4ccccc, 0x00000000]),
            ([1.0, 2.0, 2.0], [0x3eaaaaab, 0x3f2aaaab, 0x3f2aaaab]),
            ([3.0, 4.0, 12.0], [0x3e6c4ec5, 0x3e9d89d9, 0x3f6c4ec5]),
            ([0.1, 0.2, 0.3], [0x3e88d68c, 0x3f08d68c, 0x3f4d41d3]),
        ];
        for (input, expected) in cases {
            let mut v = Vector::new(input[0], input[1], input[2]);
            v.normalize();
            assert_eq!(bits(v), *expected, "normalize({input:?})");
        }
    }

    #[test]
    fn normalize_zero_and_nan_are_untouched() {
        let mut z = Vector::ZERO;
        z.normalize();
        assert_eq!(z, Vector::ZERO);
        let mut n = Vector::new(f32::NAN, 1.0, 2.0);
        n.normalize();
        assert!(n.x.is_nan());
        assert_eq!((n.y, n.z), (1.0, 2.0));
    }

    #[test]
    fn cap_family() {
        let mut v = Vector::new(30.0, 40.0, 0.0);
        v.cap(5.0);
        assert!((v.x - 3.0).abs() < 1e-3 && (v.y - 4.0).abs() < 1e-3);
        let mut w = Vector::new(0.3, 0.4, 0.0);
        w.cap_min(5.0);
        assert!((w.x - 3.0).abs() < 1e-3);
        let mut s = Vector::new(10.0, -10.0, 0.5);
        s.cap_sloppy(2.0);
        assert_eq!(s, Vector::new(2.0, -2.0, 0.5));
    }

    #[test]
    fn homogenize_skips_w_one_and_zero() {
        assert_eq!(HVector::new(2.0, 4.0, 6.0, 1.0).homogenize(), Vector::new(2.0, 4.0, 6.0));
        assert_eq!(HVector::new(2.0, 4.0, 6.0, 0.0).homogenize(), Vector::new(2.0, 4.0, 6.0));
        assert_eq!(HVector::new(2.0, 4.0, 6.0, 2.0).homogenize(), Vector::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn triangle_normal_is_unit_and_oriented() {
        let n = Vector::triangle_normal(
            Vector::new(1.0, 0.0, 0.0),
            Vector::new(0.0, 0.0, 0.0),
            Vector::new(0.0, 1.0, 0.0),
        );
        assert_eq!(n, Vector::new(0.0, 0.0, 1.0));
    }
}
