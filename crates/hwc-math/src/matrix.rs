//! 3x3 and 4x4 matrices (`Matrix.h`, `Matrix.c`, `Matrix-mult.c`).
//!
//! Storage is column-major exactly as in the game (`m11, m21, m31, m12, ...`), so element
//! `(row, col)` (1-based `m<row><col>` in the original) lives at index `row + 3*col`
//! (`row + 4*col` for [`HMatrix`]), both 0-based here.
//!
//! The multiply routines reproduce the summation order of the code in `cataclysm.exe`:
//! the 3x3 routines are hand-written x87 assembly whose pairing differs per output
//! component, and the 4x4 routines are compiler output with re-associated sums. Like the
//! originals, results must not alias inputs (the routines here take inputs by value, so that
//! hazard does not exist, but note that the original `matMultiplyMatByMat` writes each
//! output row before reading the next row of `first`).

use crate::fpu::{eq_or_unordered, ld, st};
use crate::vector::{HVector, Vector};

/// `matrix`: 3x3, column-major.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct Matrix {
    pub m: [f32; 9],
}

/// `hmatrix`: 4x4 homogeneous, column-major.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct HMatrix {
    pub m: [f32; 16],
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Default for HMatrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Matrix {
    /// `IdentityMatrix`.
    pub const IDENTITY: Matrix = Matrix {
        m: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
    };

    /// Element at 0-based `(row, col)`.
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.m[row + 3 * col]
    }

    /// Set element at 0-based `(row, col)`.
    #[inline]
    pub fn set(&mut self, row: usize, col: usize, v: f32) {
        self.m[row + 3 * col] = v;
    }

    /// Column `col` as a vector (`matGetVectFromMatrixCol1..3`).
    #[inline]
    pub fn col(&self, col: usize) -> Vector {
        Vector::new(self.m[3 * col], self.m[3 * col + 1], self.m[3 * col + 2])
    }

    /// Replace column `col` (`matPutVectIntoMatrixCol1..3`).
    #[inline]
    pub fn set_col(&mut self, col: usize, v: Vector) {
        self.m[3 * col] = v.x;
        self.m[3 * col + 1] = v.y;
        self.m[3 * col + 2] = v.z;
    }

    /// `matCreateMatFromVecs`: build from three columns.
    pub fn from_cols(c1: Vector, c2: Vector, c3: Vector) -> Matrix {
        let mut r = Matrix::IDENTITY;
        r.set_col(0, c1);
        r.set_col(1, c2);
        r.set_col(2, c3);
        r
    }

    /// `matGetMatFromHMat`: upper-left 3x3.
    pub fn from_hmatrix(h: &HMatrix) -> Matrix {
        let mut r = Matrix::IDENTITY;
        for col in 0..3 {
            for row in 0..3 {
                r.m[row + 3 * col] = h.m[row + 4 * col];
            }
        }
        r
    }

    /// `matMultiplyMatByMat` (0x4a87f0): `first * second`.
    pub fn mul_mat(&self, second: &Matrix) -> Matrix {
        let b = |row: usize, col: usize| ld(second.m[row + 3 * col]);
        let mut c = Matrix::IDENTITY;
        for i in 0..3 {
            let a0 = ld(self.m[i]);
            let a1 = ld(self.m[i + 3]);
            let a2 = ld(self.m[i + 6]);
            c.m[i] = st((a2 * b(2, 0) + a0 * b(0, 0)) + a1 * b(1, 0));
            c.m[i + 3] = st((a2 * b(2, 1) + a1 * b(1, 1)) + a0 * b(0, 1));
            c.m[i + 6] = st((a1 * b(1, 2) + a2 * b(2, 2)) + a0 * b(0, 2));
        }
        c
    }

    /// `matMultiplyMatByVec` (0x4a88b0): `matrix * v`.
    pub fn mul_vec(&self, v: Vector) -> Vector {
        let m = |k: usize| ld(self.m[k]);
        let (x, y, z) = (ld(v.x), ld(v.y), ld(v.z));
        Vector::new(
            st((x * m(0) + y * m(3)) + z * m(6)),
            st(x * m(1) + (y * m(4) + z * m(7))),
            st(x * m(2) + (y * m(5) + z * m(8))),
        )
    }

    /// `matMultiplyVecByMat` (0x4a8940): `v * matrix` (i.e. `transpose(matrix) * v`).
    pub fn vec_mul(&self, v: Vector) -> Vector {
        let m = |k: usize| ld(self.m[k]);
        let (x, y, z) = (ld(v.x), ld(v.y), ld(v.z));
        Vector::new(
            st((x * m(0) + y * m(1)) + z * m(2)),
            st(x * m(3) + (y * m(4) + z * m(5))),
            st(x * m(6) + (y * m(7) + z * m(8))),
        )
    }

    /// `matTranspose` / `matCopyAndTranspose`.
    pub fn transpose(&self) -> Matrix {
        let mut r = *self;
        for row in 0..3 {
            for col in 0..3 {
                r.m[row + 3 * col] = self.m[col + 3 * row];
            }
        }
        r
    }

    /// `matMakeRotAboutZ`.
    pub fn rot_about_z(cos: f32, sin: f32) -> Matrix {
        Matrix {
            m: [cos, sin, 0.0, -sin, cos, 0.0, 0.0, 0.0, 1.0],
        }
    }

    /// `matMakeRotAboutX`.
    pub fn rot_about_x(cos: f32, sin: f32) -> Matrix {
        Matrix {
            m: [1.0, 0.0, 0.0, 0.0, cos, sin, 0.0, -sin, cos],
        }
    }

    /// `matMakeRotAboutY`.
    pub fn rot_about_y(cos: f32, sin: f32) -> Matrix {
        Matrix {
            m: [cos, 0.0, -sin, 0.0, 1.0, 0.0, sin, 0.0, cos],
        }
    }

    /// `matCopyAndScale` (0x4a8b00): `input * diag(scale)` via a full matrix multiply (so
    /// infinities/NaNs propagate through the zero terms as in the original).
    pub fn copy_and_scale(input: &Matrix, scale: f32) -> Matrix {
        let mut s = Matrix::IDENTITY;
        s.m[0] *= scale;
        s.m[4] *= scale;
        s.m[8] *= scale;
        input.mul_mat(&s)
    }

    /// `matCreateCoordSysFromHeading` (0x4a8660): columns are `(up, right, heading)` with
    /// the world up `(0,0,1)` (nudged when the heading is exactly `(0,0,1)`). The cross
    /// products are specialised for `up.y == 0` as the compiler did, so the zero terms are
    /// absent (this matters for signed zeros).
    pub fn coord_sys_from_heading(heading: Vector) -> Matrix {
        let lof = heading;
        // The three equality tests use the unordered-is-equal pattern.
        let same = eq_or_unordered(0.0, ld(lof.x))
            && eq_or_unordered(0.0, ld(lof.y))
            && eq_or_unordered(1.0, ld(lof.z));
        let (ux, uz) = if same {
            (ld(0.119_145_22), ld(0.992_876_8))
        } else {
            (0.0, 1.0)
        };
        let (lx, ly, lz) = (ld(lof.x), ld(lof.y), ld(lof.z));
        let mut right = Vector::new(st(ly * uz), st(lz * ux - lx * uz), st(-(ly * ux)));
        let (rx, ry, rz) = (ld(right.x), ld(right.y), ld(right.z));
        let mut up = Vector::new(st(ry * lz - rz * ly), st(rz * lx - rx * lz), st(rx * ly - ry * lx));
        right.normalize();
        up.normalize();
        Matrix::from_cols(up, right, lof)
    }
}

impl HMatrix {
    /// `IdentityHMatrix`.
    pub const IDENTITY: HMatrix = HMatrix {
        m: [
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ],
    };

    /// Element at 0-based `(row, col)`.
    #[inline]
    pub fn get(&self, row: usize, col: usize) -> f32 {
        self.m[row + 4 * col]
    }

    /// Set element at 0-based `(row, col)`.
    #[inline]
    pub fn set(&mut self, row: usize, col: usize, v: f32) {
        self.m[row + 4 * col] = v;
    }

    /// Column `col` (`hmatGetHVectFromHMatrixCol1..4`).
    pub fn col(&self, col: usize) -> HVector {
        HVector::new(self.m[4 * col], self.m[4 * col + 1], self.m[4 * col + 2], self.m[4 * col + 3])
    }

    /// `hmatCreateHMatFromHVecs`.
    pub fn from_cols(c: [HVector; 4]) -> HMatrix {
        let mut r = HMatrix::IDENTITY;
        for (i, v) in c.iter().enumerate() {
            r.m[4 * i] = v.x;
            r.m[4 * i + 1] = v.y;
            r.m[4 * i + 2] = v.z;
            r.m[4 * i + 3] = v.w;
        }
        r
    }

    /// `hmatMakeHMatFromMat`.
    pub fn from_matrix(mat: &Matrix) -> HMatrix {
        Self::from_matrix_and_pos(mat, Vector::ZERO)
    }

    /// `hmatMakeHMatFromMatAndVec`.
    pub fn from_matrix_and_pos(mat: &Matrix, pos: Vector) -> HMatrix {
        let mut r = HMatrix::IDENTITY;
        for col in 0..3 {
            for row in 0..3 {
                r.m[row + 4 * col] = mat.m[row + 3 * col];
            }
        }
        r.m[12] = pos.x;
        r.m[13] = pos.y;
        r.m[14] = pos.z;
        r
    }

    /// `hmatMultiplyHMatByHMat` (0x4a8300): `first * second`.
    pub fn mul_hmat(&self, second: &HMatrix) -> HMatrix {
        let b = |row: usize, col: usize| ld(second.m[row + 4 * col]);
        let mut c = HMatrix::IDENTITY;
        for i in 0..4 {
            let a0 = ld(self.m[i]);
            let a1 = ld(self.m[i + 4]);
            let a2 = ld(self.m[i + 8]);
            let a3 = ld(self.m[i + 12]);
            c.m[i] = st(((a3 * b(3, 0) + a1 * b(1, 0)) + a2 * b(2, 0)) + a0 * b(0, 0));
            c.m[i + 4] = st(((a0 * b(0, 1) + a3 * b(3, 1)) + a1 * b(1, 1)) + a2 * b(2, 1));
            c.m[i + 8] = st(((a2 * b(2, 2) + a0 * b(0, 2)) + a3 * b(3, 2)) + a1 * b(1, 2));
            c.m[i + 12] = st(((a0 * b(0, 3) + a3 * b(3, 3)) + a1 * b(1, 3)) + a2 * b(2, 3));
        }
        c
    }

    /// `hmatMultiplyHMatByHVec` (0x4a83b0): `matrix * v`.
    pub fn mul_hvec(&self, v: HVector) -> HVector {
        let m = |k: usize| ld(self.m[k]);
        let (x, y, z, w) = (ld(v.x), ld(v.y), ld(v.z), ld(v.w));
        let row = |r: usize| st(((m(4 + r) * y + m(12 + r) * w) + m(8 + r) * z) + m(r) * x);
        HVector::new(row(0), row(1), row(2), row(3))
    }

    /// `hmatMultiplyHVecByHMat` (0x4a84a0): `v * matrix`.
    pub fn hvec_mul(&self, v: HVector) -> HVector {
        let m = |k: usize| ld(self.m[k]);
        let (x, y, z, w) = (ld(v.x), ld(v.y), ld(v.z), ld(v.w));
        let col = |c: usize| {
            let b = 4 * c;
            st(((m(b + 1) * y + m(b + 3) * w) + m(b + 2) * z) + m(b) * x)
        };
        HVector::new(col(0), col(1), col(2), col(3))
    }

    /// Cataclysm addition at 0x4a8440: affine transform of a point (`w` taken as 1, only
    /// x, y, z produced).
    pub fn transform_point(&self, v: Vector) -> Vector {
        let m = |k: usize| ld(self.m[k]);
        let (x, y, z) = (ld(v.x), ld(v.y), ld(v.z));
        Vector::new(
            st(((m(8) * z + m(4) * y) + m(0) * x) + m(12)),
            st(((m(9) * z + m(1) * x) + m(5) * y) + m(13)),
            st(((m(10) * z + m(2) * x) + m(6) * y) + m(14)),
        )
    }

    /// `hmatTranspose` / `hmatCopyAndTranspose`.
    pub fn transpose(&self) -> HMatrix {
        let mut r = *self;
        for row in 0..4 {
            for col in 0..4 {
                r.m[row + 4 * col] = self.m[col + 4 * row];
            }
        }
        r
    }

    /// `hmatMakeRotAboutZ`.
    pub fn rot_about_z(cos: f32, sin: f32) -> HMatrix {
        HMatrix::from_matrix(&Matrix::rot_about_z(cos, sin))
    }

    /// `hmatMakeRotAboutX`.
    pub fn rot_about_x(cos: f32, sin: f32) -> HMatrix {
        HMatrix::from_matrix(&Matrix::rot_about_x(cos, sin))
    }

    /// `hmatMakeRotAboutY`.
    pub fn rot_about_y(cos: f32, sin: f32) -> HMatrix {
        HMatrix::from_matrix(&Matrix::rot_about_y(cos, sin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(bits: &[u32]) -> Vec<f32> {
        bits.iter().map(|&b| f32::from_bits(b)).collect()
    }

    fn mat(bits: &[u32]) -> Matrix {
        Matrix {
            m: f(bits).try_into().unwrap(),
        }
    }

    fn hmat(bits: &[u32]) -> HMatrix {
        HMatrix {
            m: f(bits).try_into().unwrap(),
        }
    }

    fn vbits(v: Vector) -> [u32; 3] {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
    }

    fn hvbits(v: HVector) -> [u32; 4] {
        [v.x.to_bits(), v.y.to_bits(), v.z.to_bits(), v.w.to_bits()]
    }

    struct Case3 {
        m: [u32; 9],
        v: [u32; 3],
        m2: [u32; 9],
        mat_vec: [u32; 3],
        vec_mat: [u32; 3],
        mat_mat: [u32; 9],
    }

    /// Inputs and outputs from emulating the cataclysm.exe routines (Ghidra p-code).
    const CASES3: &[Case3] = &[
        Case3 {
            m: [0xc0617e7b, 0xc0df74df, 0x40413235, 0xc108d205, 0x3f37b743, 0xc02beb0e, 0xc10d70ba, 0x3e1848a8, 0xc114005b],
            v: [0xbfa9ddf7, 0xc109a572, 0xc102f8ca],
            m2: [0xbfc13b1b, 0x40d12f74, 0xc0f0c449, 0xc0b12087, 0x40231d51, 0x410f4451, 0x3fc56230, 0xc0043fbe, 0x411866d1],
            mat_vec: [0x43169a84, 0x3ff01f00, 0x42bda4df],
            vec_mat: [0x422028ab, 0x41d94e41, 0x42ac5780],
            mat_mat: [0x417ee4a8, 0x4161d2ed, 0x423debcf, 0xc2a2e4d9, 0x422740b2, 0xc2d4c3bf, 0xc28fee19, 0xc12d5aa0, 0xc29bcdf2],
        },
        Case3 {
            m: [0xc11117f2, 0x40e56b79, 0xc086a66a, 0xc0e3ad3f, 0xc0f49ceb, 0xc07524ad, 0x40ca5225, 0xc0cc55ca, 0x3fd0e57c],
            v: [0x4031cf2a, 0xc02354c6, 0x3f7473a0],
            m2: [0xc10be854, 0xc10ced79, 0xc0bc2fb9, 0x4066e977, 0xbfb95d1b, 0xc06de441, 0x3fdb09d3, 0xbf6fb22d, 0xc080262d],
            mat_vec: [0xbf7fda39, 0x4205512e, 0xbeb6ac34],
            vec_mat: [0xc23e091a, 0xc07aa693, 0x420dae30],
            mat_mat: [0x42d19048, 0x4228c36c, 0x4273bcb4, 0xc237aa4e, 0x4272b049, 0xc17b37a5, 0xc208b41e, 0x4233fd65, 0xc1226575],
        },
        Case3 {
            m: [0x40bc6722, 0x407eb67f, 0xc0a3c73a, 0x3fbe8653, 0x3f010190, 0x40f01687, 0x4092d851, 0xc087b847, 0x4119a7ec],
            v: [0xc0f4701a, 0xbfd19b07, 0x40a491f8],
            m2: [0xc0debadb, 0xbe620924, 0xc1137424, 0x405750f9, 0x40a9534a, 0x3fbaf248, 0x40f04e49, 0xc06e6737, 0x4079fa63],
            mat_vec: [0xc1be7cdd, 0xc2542731, 0x42986585],
            vec_mat: [0xc29b9fbc, 0x41d31ee7, 0x41aa3f9c],
            mat_mat: [0xc2a73288, 0x41346322, 0xc25a261d, 0x42098b4e, 0x411dc980, 0x421207a3, 0x42625e9a, 0x41371b22, 0xc1e6fa3a],
        },
    ];

    #[test]
    fn matrix3_routines_match_emulated_binary() {
        for c in CASES3 {
            let m = mat(&c.m);
            let m2 = mat(&c.m2);
            let v = f(&c.v);
            let v = Vector::new(v[0], v[1], v[2]);
            assert_eq!(vbits(m.mul_vec(v)), c.mat_vec);
            assert_eq!(vbits(m.vec_mul(v)), c.vec_mat);
            let p = m.mul_mat(&m2);
            let pb: Vec<u32> = p.m.iter().map(|x| x.to_bits()).collect();
            assert_eq!(pb, c.mat_mat);
        }
    }

    struct Case4 {
        h: [u32; 16],
        h2: [u32; 16],
        hv: [u32; 4],
        hmat_hmat: [u32; 16],
        hmat_hvec: [u32; 4],
        hvec_hmat: [u32; 4],
        affine: [u32; 3],
    }

    const CASES4: &[Case4] = &[
        Case4 {
            h: [0x3ff1963e, 0x3fcc881f, 0xbf603a8c, 0x40d99452, 0x410e4c46, 0xbf049dd4, 0x40521d65, 0xc10c95f6, 0x4080f474, 0x403c532f, 0x411dca6b, 0x40ce0828, 0xc089dbde, 0xc0122fdc, 0x4057e01f, 0xc118c7a5],
            h2: [0xbf441ec1, 0xc0d472f4, 0xc0f50f06, 0xc10d2274, 0x40abab4b, 0xc0ed38e6, 0xc0a186c9, 0xc00b959a, 0x40edb5c7, 0xc10636c8, 0xbf82148b, 0x3f7d21e1, 0x40f55d9b, 0x40cc56cf, 0x40e8f337, 0xc08dcf7e],
            hv: [0xbfd8d746, 0xc034c5dd, 0x40f5e227, 0x41127957],
            hmat_hmat: [0xc25566f6, 0xbe2f0a25, 0xc2fccdd7, 0x42b0186e, 0xc2858032, 0x4022a035, 0xc2ac58c9, 0x42b3e0f2, 0xc289e024, 0x412f7667, 0xc222ee70, 0x42d8754a, 0x42ef619e, 0x4221f380, 0x428e2f0a, 0x42aa6d70],
            hmat_hvec: [0xc2132b56, 0x3ee968a9, 0x42c5bc05, 0xc1c5246c],
            hvec_hmat: [0x423f37d4, 0xc289a137, 0x42ef2994, 0xc23efcfb],
            affine: [0xbfd4ab18, 0x4198a8d3, 0x428eb8f4],
        },
        Case4 {
            h: [0xc0df691e, 0xc0cf387d, 0xc0ab8c30, 0xc0aaaa37, 0xbe99fb4d, 0x3fe427fb, 0xc097d798, 0xc11eb0a7, 0xbfcf7f39, 0xc0275afd, 0x3fa9d562, 0x4110fdc8, 0x4073d4f6, 0x3e9ea1dd, 0x401684cb, 0x4061893e],
            h2: [0xc10eb8e7, 0x40ffb37d, 0x40b32e34, 0x40efb03d, 0x40bea388, 0xc009c148, 0xc0014e9e, 0xc0fdbc7c, 0x402be401, 0xc10c14a9, 0xc10a72e2, 0xc0ba643d, 0xc0d8203f, 0xc04cbb38, 0xc10f2d02, 0xc11fece4],
            hv: [0xc0df30c1, 0xc0ff1014, 0xc02e944d, 0xc117d6f8],
            hmat_hmat: [0x429eaac0, 0x426ebdf0, 0x420bcba1, 0x4235d559, 0xc287c241, 0xc21e5a9d, 0xc22c363c, 0xc262b4c1, 0xc1c2423c, 0xc142ce77, 0x3ffb88a4, 0xc1d37161, 0x41c452b8, 0x42695902, 0x41800b0f, 0xc2424402],
            hmat_hvec: [0x419add0b, 0x420c9a7f, 0x4245182d, 0x42685ccf],
            hvec_hmat: [0x43258bca, 0x42bde9bd, 0xc265df0c, 0xc289cd00],
            affine: [0x426d4b32, 0x42199a98, 0x4293e399],
        },
    ];

    #[test]
    fn matrix4_routines_match_emulated_binary() {
        for c in CASES4 {
            let h = hmat(&c.h);
            let h2 = hmat(&c.h2);
            let v = f(&c.hv);
            let hv = HVector::new(v[0], v[1], v[2], v[3]);
            let p = h.mul_hmat(&h2);
            let pb: Vec<u32> = p.m.iter().map(|x| x.to_bits()).collect();
            assert_eq!(pb, c.hmat_hmat);
            assert_eq!(hvbits(h.mul_hvec(hv)), c.hmat_hvec);
            assert_eq!(hvbits(h.hvec_mul(hv)), c.hvec_hmat);
            assert_eq!(vbits(h.transform_point(hv.xyz())), c.affine);
        }
    }

    #[test]
    fn coord_sys_is_orthonormal() {
        let mut h = Vector::new(0.3, -0.5, 0.8);
        h.normalize();
        let m = Matrix::coord_sys_from_heading(h);
        assert_eq!(m.col(2), h);
        let up = m.col(0);
        let right = m.col(1);
        assert!(up.dot_ext(right).abs() < 1e-6);
        assert!((up.mag_ext() - 1.0).abs() < 1e-3);
        // Straight up uses the nudged up vector instead of producing NaNs.
        let z = Matrix::coord_sys_from_heading(Vector::new(0.0, 0.0, 1.0));
        assert!(z.m.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn identity_and_rotation_layout() {
        let v = Vector::new(1.0, 2.0, 3.0);
        assert_eq!(Matrix::IDENTITY.mul_vec(v), v);
        let r = Matrix::rot_about_z(0.0, 1.0);
        assert_eq!(r.get(0, 1), -1.0);
        assert_eq!(r.mul_vec(Vector::new(1.0, 0.0, 0.0)), Vector::new(0.0, 1.0, 0.0));
        assert_eq!(HMatrix::from_matrix(&r).transform_point(Vector::new(1.0, 0.0, 0.0)), Vector::new(0.0, 1.0, 0.0));
    }
}
