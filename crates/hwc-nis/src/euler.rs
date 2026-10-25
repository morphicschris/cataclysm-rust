//! Orientation of NIS objects and cameras from their heading, pitch and bank curves
//! (`nisRotateAboutVector` 0x4c7db0, `nisShipEulerToMatrix` 0x4c7ec0).
//!
//! The arithmetic follows the binary's x87 sequence: sine and cosine are rounded to `float`, so
//! are the six cross products, and each matrix element is one register expression rounded once
//! when it is stored (one of the intermediate products is stored as a `float` and reloaded).

use hwc_math::fpu::{ld, st};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

/// `HPBIdentityShip` (0x87e8c8): the orientation of a ship with zero heading, pitch and bank,
/// columns (0,0,1), (-1,0,0), (0,-1,0).
pub const HPB_IDENTITY_SHIP: Matrix = Matrix {
    m: [0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0, -1.0, 0.0],
};

/// `nisRotateAboutVector`: rotation by `radians` about the unit `axis`.
pub fn rotate_about_vector(axis: Vector, radians: f32) -> Matrix {
    let (x, y, z) = (axis.x, axis.y, axis.z);
    let a = ld(radians);
    let s = st(fsin(a));
    let c = st(fcos(a));
    let xy = st(ld(y) * ld(x));
    let yz = st(ld(z) * ld(y));
    let zx = st(ld(z) * ld(x));
    let xs = st(ld(s) * ld(x));
    let ys = st(ld(s) * ld(y));
    let zs = st(ld(s) * ld(z));
    let one_c = st(1.0 - ld(c));
    let (one_c, c) = (ld(one_c), ld(c));
    let mut m = [0.0f32; 9];
    m[0] = st(ld(x) * ld(x) * one_c + c);
    let t = one_c * ld(xy);
    m[3] = st(t - ld(zs));
    let u = one_c * ld(zx);
    let u_stored = st(u);
    m[6] = st(u + ld(ys));
    m[1] = st(t + ld(zs));
    m[4] = st(ld(y) * ld(y) * one_c + c);
    let v = one_c * ld(yz);
    m[7] = st(v - ld(xs));
    m[2] = st(ld(u_stored) - ld(ys));
    m[5] = st(v + ld(xs));
    m[8] = st(ld(z) * ld(z) * one_c + c);
    Matrix { m }
}

/// `nisShipEulerToMatrix`: heading about the world up axis, then the curve's "pitch" about the
/// ship's third column and its "bank" about the second, starting from [`HPB_IDENTITY_SHIP`].
/// Angles are in radians (the loader converts the file's degrees).
pub fn ship_euler_to_matrix(heading: f32, pitch: f32, bank: f32) -> Matrix {
    let mut m = HPB_IDENTITY_SHIP;
    let r = rotate_about_vector(Vector::new(0.0, 0.0, 1.0), -heading);
    m = r.mul_mat(&m);
    let r = rotate_about_vector(m.col(2), pitch);
    m = r.mul_mat(&m);
    let r = rotate_about_vector(m.col(1), -bank);
    r.mul_mat(&m)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: &Matrix, b: &Matrix) -> bool {
        a.m.iter().zip(b.m.iter()).all(|(x, y)| (x - y).abs() < 1e-6)
    }

    #[test]
    fn zero_angles_give_the_ship_identity() {
        assert!(close(&ship_euler_to_matrix(0.0, 0.0, 0.0), &HPB_IDENTITY_SHIP));
    }

    #[test]
    fn rotation_about_z() {
        let r = rotate_about_vector(Vector::new(0.0, 0.0, 1.0), std::f32::consts::FRAC_PI_2);
        // Column-major: the x axis goes to +y.
        assert!((r.get(1, 0) - 1.0).abs() < 1e-6);
        assert!((r.get(0, 1) + 1.0).abs() < 1e-6);
        assert!((r.get(2, 2) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn heading_turns_the_forward_axis() {
        // The ship's forward axis is column 2 (0,-1,0 at rest); heading 90 degrees turns it
        // about +z by -90 degrees, to (-1, 0, 0).
        let m = ship_euler_to_matrix(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        let f = m.col(2);
        assert!((f.x + 1.0).abs() < 1e-6 && f.y.abs() < 1e-6, "{f:?}");
    }
}
