//! The little vector and matrix arithmetic the static loaders need. Kept local and minimal until
//! the shared `hwc-math` crate settles; the conventions match the original (vectors are `x, y,
//! z`; 3x3 matrices are stored column by column).
//!
//! The original evaluates expressions on the x87 stack (64-bit mantissa) and rounds to single
//! precision only when storing. Multi-term expressions here are evaluated in `f64` and rounded
//! once for the same reason.

pub type Vec3 = [f32; 3];

/// A 3x3 matrix stored column-major: `m[col * 3 + row]`.
pub type Mat3 = [f32; 9];

pub const IDENTITY: Mat3 = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];

/// Scales `v` to unit length; a zero vector is left unchanged.
pub fn normalize(v: &mut Vec3) {
    let [x, y, z] = v.map(f64::from);
    let mag = (x * x + y * y + z * z).sqrt() as f32;
    if mag == 0.0 {
        return;
    }
    let inv = 1.0 / mag;
    for c in v.iter_mut() {
        *c *= inv;
    }
}

/// Rotation about the x axis.
pub fn rot_about_x(cos: f32, sin: f32) -> Mat3 {
    // Rows: [1 0 0], [0 c -s], [0 s c].
    [1.0, 0.0, 0.0, 0.0, cos, sin, 0.0, -sin, cos]
}

/// Rotation about the y axis.
pub fn rot_about_y(cos: f32, sin: f32) -> Mat3 {
    // Rows: [c 0 s], [0 1 0], [-s 0 c].
    [cos, 0.0, -sin, 0.0, 1.0, 0.0, sin, 0.0, cos]
}

/// `a * b`.
pub fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut out = [0.0f32; 9];
    for row in 0..3 {
        for col in 0..3 {
            let ar = |k: usize| f64::from(a[k * 3 + row]);
            let bc = |k: usize| f64::from(b[col * 3 + k]);
            // Same association as the original's hand-written loop.
            out[col * 3 + row] = (ar(2) * bc(2) + bc(0) * ar(0) + bc(1) * ar(1)) as f32;
        }
    }
    out
}

/// `cos` and `sin` of an angle in radians, rounded to single precision.
pub fn cos_sin(angle: f32) -> (f32, f32) {
    let a = f64::from(angle);
    (a.cos() as f32, a.sin() as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotations_compose() {
        let (c, s) = cos_sin(std::f32::consts::FRAC_PI_2);
        let x = rot_about_x(c, s);
        // Rotating +y by 90 degrees about x gives +z.
        let y_col = [x[3], x[4], x[5]];
        assert!((y_col[2] - 1.0).abs() < 1e-6 && y_col[1].abs() < 1e-6);
        assert_eq!(mul(&IDENTITY, &x), x);
        let mut v = [3.0, 0.0, 4.0];
        normalize(&mut v);
        assert!((v[0] - 0.6).abs() < 1e-6 && (v[2] - 0.8).abs() < 1e-6);
    }
}
