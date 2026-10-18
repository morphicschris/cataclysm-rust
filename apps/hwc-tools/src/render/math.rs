//! Minimal vector and matrix maths for the software renderer.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn from_array(a: [f32; 3]) -> Self {
        Self::new(a[0], a[1], a[2])
    }

    pub fn dot(self, o: Self) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    pub fn cross(self, o: Self) -> Self {
        Self::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    pub fn length(self) -> f32 {
        self.dot(self).sqrt()
    }

    /// Unit vector in the same direction, or zero for a zero vector.
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len > 0.0 {
            self * (1.0 / len)
        } else {
            self
        }
    }

    pub fn min(self, o: Self) -> Self {
        Self::new(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }

    pub fn max(self, o: Self) -> Self {
        Self::new(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
}

impl Add for Vec3 {
    type Output = Self;
    fn add(self, o: Self) -> Self {
        Self::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}

impl Sub for Vec3 {
    type Output = Self;
    fn sub(self, o: Self) -> Self {
        Self::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    fn mul(self, s: f32) -> Self {
        Self::new(self.x * s, self.y * s, self.z * s)
    }
}

impl Neg for Vec3 {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

/// Column-major 4×4 matrix, laid out like OpenGL (`m[col * 4 + row]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat4(pub [f32; 16]);

impl Mat4 {
    fn at(&self, row: usize, col: usize) -> f32 {
        self.0[col * 4 + row]
    }

    pub fn transform_point(&self, p: Vec3) -> Vec3 {
        let [x, y, z, w] = self.transform4([p.x, p.y, p.z, 1.0]);
        if w != 0.0 && w != 1.0 {
            Vec3::new(x / w, y / w, z / w)
        } else {
            Vec3::new(x, y, z)
        }
    }

    /// Transforms a direction (ignores translation).
    pub fn transform_vector(&self, v: Vec3) -> Vec3 {
        let [x, y, z, _] = self.transform4([v.x, v.y, v.z, 0.0]);
        Vec3::new(x, y, z)
    }

    pub fn transform4(&self, v: [f32; 4]) -> [f32; 4] {
        std::array::from_fn(|row| (0..4).map(|col| self.at(row, col) * v[col]).sum())
    }
}

impl Mul for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 {
        let mut m = [0.0; 16];
        for col in 0..4 {
            for row in 0..4 {
                m[col * 4 + row] = (0..4).map(|k| self.at(row, k) * o.at(k, col)).sum();
            }
        }
        Mat4(m)
    }
}

/// A perspective camera: view basis plus projection scale.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub eye: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3,
    /// Projection scale for x and y (`1 / tan(fov / 2)`, x divided by the aspect ratio).
    scale: [f32; 2],
}

impl Camera {
    /// Camera at `eye` looking along `forward`, with `up` as the approximate screen-up direction.
    /// `fov_y` is the vertical field of view in degrees.
    pub fn new(eye: Vec3, forward: Vec3, up: Vec3, fov_y: f32, aspect: f32) -> Self {
        let forward = forward.normalize();
        let right = forward.cross(up).normalize();
        let up = right.cross(forward);
        let f = 1.0 / (fov_y.to_radians() * 0.5).tan();
        Self {
            eye,
            right,
            up,
            forward,
            scale: [f / aspect, f],
        }
    }

    /// Clip-space position `[x, y, depth, w]` where `w` is the distance along the view axis and
    /// `x / w`, `y / w` are in -1..1 inside the view (y up).
    pub fn project(&self, p: Vec3) -> [f32; 4] {
        let d = p - self.eye;
        let w = d.dot(self.forward);
        [
            d.dot(self.right) * self.scale[0],
            d.dot(self.up) * self.scale[1],
            w,
            w,
        ]
    }

    pub fn forward(&self) -> Vec3 {
        self.forward
    }

    pub fn up(&self) -> Vec3 {
        self.up
    }

    pub fn right(&self) -> Vec3 {
        self.right
    }
}

/// Unit direction for a yaw (around `up`) and pitch (towards `up`), both in degrees, starting
/// from `front`.
pub fn orbit_direction(front: Vec3, up: Vec3, yaw: f32, pitch: f32) -> Vec3 {
    let side = front.cross(up).normalize();
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let horizontal = front * cy + side * sy;
    (horizontal * cp + up * sp).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_multiply_translation() {
        let mut t = Mat4([0.0; 16]);
        for i in 0..4 {
            t.0[i * 5] = 1.0;
        }
        t.0[12] = 5.0;
        let p = (t * t).transform_point(Vec3::new(1.0, 0.0, 0.0));
        assert_eq!(p, Vec3::new(11.0, 0.0, 0.0));
    }

    #[test]
    fn camera_projects_ahead_to_centre() {
        let cam = Camera::new(
            Vec3::default(),
            Vec3::new(0.0, 0.0, -1.0),
            Vec3::new(0.0, 1.0, 0.0),
            90.0,
            1.0,
        );
        let [x, y, _, w] = cam.project(Vec3::new(0.0, 1.0, -2.0));
        assert!(x.abs() < 1e-6 && (y / w - 0.5).abs() < 1e-5 && w == 2.0);
    }
}
