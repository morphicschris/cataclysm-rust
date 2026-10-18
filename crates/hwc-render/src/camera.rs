//! Homeworld's orbit camera: an eye that circles a look-at point at a given distance, azimuth
//! ("angle", about the world Z axis) and declination (elevation; negative values lift the eye
//! above the look-at point). Zoom is multiplicative and clamped to per-camera limits.
//!
//! Tuning values come from `camera.script` (`[Camera]` section); [`CameraTweaks::default`] holds
//! the values Cataclysm ships with.

use glam::{Mat4, Vec3};
use hwc_formats::script::ScriptFile;
use std::f32::consts::TAU;

/// Tuning constants read from `camera.script`. Angles are stored in radians.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CameraTweaks {
    pub angle_rot: f32,
    pub declination_rot: f32,
    pub max_declination: f32,
    pub min_declination: f32,
    pub zoom_in: f32,
    pub zoom_out: f32,
    pub key_zoom_in: f32,
    pub key_zoom_out: f32,
    pub clip_near: f32,
    pub clip_far: f32,
    /// Vertical field of view in degrees.
    pub field_of_view: f32,
    pub wheel_zoom_in: f32,
    pub wheel_zoom_out: f32,
    pub max_zoomout_distance: f32,
    pub min_zoomout_distance: f32,
    pub mouse_sens_min: f32,
    pub mouse_sens_max: f32,
}

impl Default for CameraTweaks {
    fn default() -> Self {
        Self {
            angle_rot: 0.1f32.to_radians(),
            declination_rot: 0.1f32.to_radians(),
            max_declination: 85.0f32.to_radians(),
            min_declination: (-85.0f32).to_radians(),
            zoom_in: 0.995,
            zoom_out: 1.005,
            key_zoom_in: 0.94,
            key_zoom_out: 1.06,
            clip_near: 20.0,
            clip_far: 35000.0,
            field_of_view: 75.0,
            wheel_zoom_in: 0.70,
            wheel_zoom_out: 1.45,
            max_zoomout_distance: 14500.0,
            min_zoomout_distance: 50.0,
            mouse_sens_min: -1.0,
            mouse_sens_max: 1.0,
        }
    }
}

impl CameraTweaks {
    /// Applies any `CAMERA_*` keys present in a parsed `camera.script` over the defaults.
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut t = Self::default();
        let angles: [(&str, &mut f32); 4] = [
            ("CAMERA_ANGLE_ROT", &mut t.angle_rot),
            ("CAMERA_DECLINATION_ROT", &mut t.declination_rot),
            ("CAMERA_MAX_DECLINATION", &mut t.max_declination),
            ("CAMERA_MIN_DECLINATION", &mut t.min_declination),
        ];
        for (key, slot) in angles {
            if let Some(v) = script.get(key).and_then(|e| e.f32()) {
                *slot = v.to_radians();
            }
        }
        let reals: [(&str, &mut f32); 13] = [
            ("CAMERA_ZOOM_IN", &mut t.zoom_in),
            ("CAMERA_ZOOM_OUT", &mut t.zoom_out),
            ("CAMERA_KEY_ZOOM_IN", &mut t.key_zoom_in),
            ("CAMERA_KEY_ZOOM_OUT", &mut t.key_zoom_out),
            ("CAMERA_CLIP_NEAR", &mut t.clip_near),
            ("CAMERA_CLIP_FAR", &mut t.clip_far),
            ("CAMERA_FIELD_OF_VIEW", &mut t.field_of_view),
            ("CAMERA_WHEEL_ZOOM_IN", &mut t.wheel_zoom_in),
            ("CAMERA_WHEEL_ZOOM_OUT", &mut t.wheel_zoom_out),
            ("CAMERA_MAX_ZOOMOUT_DISTANCE", &mut t.max_zoomout_distance),
            ("CAMERA_MIN_ZOOMOUT_DISTANCE", &mut t.min_zoomout_distance),
            ("CAMERA_MOUSE_SENS_MIN", &mut t.mouse_sens_min),
            ("CAMERA_MOUSE_SENS_MAX", &mut t.mouse_sens_max),
        ];
        for (key, slot) in reals {
            if let Some(v) = script.get(key).and_then(|e| e.f32()) {
                *slot = v;
            }
        }
        t
    }

    /// Mouse sensitivity multiplier for an options-screen setting of 0..=100 (50 gives 1.0).
    pub fn mouse_sensitivity(&self, setting: u8) -> f32 {
        let s = f32::from(setting.min(100)) / 100.0;
        2f32.powf(s * (self.mouse_sens_max - self.mouse_sens_min) + self.mouse_sens_min)
    }
}

/// An orbit camera in the engine's Z-up world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OrbitCamera {
    pub tweaks: CameraTweaks,
    pub look_at: Vec3,
    /// Azimuth in radians, kept within 0..2π.
    pub angle: f32,
    /// Elevation in radians; negative puts the eye above the look-at point.
    pub declination: f32,
    pub distance: f32,
    /// Zoom limits applied by [`zoom`](Self::zoom).
    pub closest_zoom: f32,
    pub farthest_zoom: f32,
    /// Disables the zoom limits (used by scripted cameras).
    pub ignore_zoom: bool,
    /// Mouse sensitivity multiplier (1.0 at the default options setting).
    pub mouse_sens: f32,
}

impl OrbitCamera {
    /// A camera at `distance` looking at the origin with zero angle and declination.
    pub fn new(tweaks: CameraTweaks, distance: f32) -> Self {
        Self {
            tweaks,
            look_at: Vec3::ZERO,
            angle: 0.0,
            declination: 0.0,
            distance,
            closest_zoom: tweaks.min_zoomout_distance,
            farthest_zoom: tweaks.max_zoomout_distance,
            ignore_zoom: false,
            mouse_sens: 1.0,
        }
    }

    fn wrap_angle(&mut self) {
        if self.angle < 0.0 {
            self.angle += TAU;
        } else if self.angle > TAU {
            self.angle -= TAU;
        }
    }

    fn clamp_declination(&mut self) {
        self.declination = self
            .declination
            .clamp(self.tweaks.min_declination, self.tweaks.max_declination);
    }

    /// Eye position: the offset (distance, 0, 0) turned about Y by the declination, then about Z
    /// by the angle, added to the look-at point.
    pub fn eye(&self) -> Vec3 {
        let (sd, cd) = self.declination.sin_cos();
        let (sa, ca) = self.angle.sin_cos();
        let d = self.distance;
        self.look_at + Vec3::new(d * cd * ca, d * cd * sa, -d * sd)
    }

    pub fn up(&self) -> Vec3 {
        Vec3::Z
    }

    pub fn rotate_angle(&mut self, radians: f32) {
        self.angle += radians;
        self.wrap_angle();
    }

    pub fn rotate_declination(&mut self, radians: f32) {
        self.declination += radians;
        self.clamp_declination();
    }

    /// Multiplies the distance by `factor`, then clamps to the far limit and to `min_zoom`
    /// (normally [`closest_zoom`](Self::closest_zoom), or a focused ship's minimum zoom distance
    /// when that is larger) unless zoom limits are ignored.
    pub fn zoom_with_min(&mut self, factor: f32, min_zoom: f32) {
        self.distance *= factor;
        if !self.ignore_zoom {
            if self.distance > self.farthest_zoom {
                self.distance = self.farthest_zoom;
            }
            if self.distance < min_zoom {
                self.distance = min_zoom;
            }
        }
    }

    pub fn zoom(&mut self, factor: f32) {
        self.zoom_with_min(factor, self.closest_zoom);
    }

    /// One mouse-wheel notch towards (`toward = true`) or away from the look-at point.
    pub fn wheel(&mut self, toward: bool, min_zoom: f32) {
        let t = &self.tweaks;
        let factor = if toward {
            1.0 - (1.0 - t.wheel_zoom_in) * self.mouse_sens
        } else {
            (t.wheel_zoom_out - 1.0) * self.mouse_sens + 1.0
        };
        self.zoom_with_min(factor, min_zoom);
    }

    /// One frame of the zoom keys being held.
    pub fn key_zoom(&mut self, toward: bool, min_zoom: f32) {
        let t = &self.tweaks;
        let factor = if toward {
            1.0 - (1.0 - t.key_zoom_in) * self.mouse_sens
        } else {
            (t.key_zoom_out - 1.0) * self.mouse_sens + 1.0
        };
        self.zoom_with_min(factor, min_zoom);
    }

    /// Mouse movement while the right button is held. `dx`/`dy` are the cursor's movement in
    /// pixels (right and down positive). With both buttons held vertical movement zooms instead.
    pub fn mouse_drag(&mut self, dx: f32, dy: f32, both_buttons: bool, min_zoom: f32) {
        // The engine measures movement as (centre - cursor), so moving right or down is negative.
        let (mx, my) = (-dx, -dy);
        if both_buttons {
            let t = self.tweaks;
            if my < 0.0 {
                self.zoom_with_min(1.0 + (1.0 - t.zoom_out) * my * self.mouse_sens, min_zoom);
            } else if my > 0.0 {
                self.zoom_with_min(1.0 - (1.0 - t.zoom_in) * my * self.mouse_sens, min_zoom);
            }
        } else {
            self.rotate_declination(self.tweaks.declination_rot * my * self.mouse_sens);
            self.rotate_angle(self.tweaks.angle_rot * mx * self.mouse_sens);
        }
    }

    /// World-to-eye matrix (right-handed, Z up).
    pub fn view(&self) -> Mat4 {
        Mat4::look_at_rh(self.eye(), self.look_at, self.up())
    }

    /// Eye rotation only, for drawing the sky sphere around the eye.
    pub fn view_rotation(&self) -> Mat4 {
        Mat4::look_at_rh(Vec3::ZERO, self.look_at - self.eye(), self.up())
    }

    /// Perspective projection with wgpu's 0..1 depth range.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        Mat4::perspective_rh(
            self.tweaks.field_of_view.to_radians(),
            aspect,
            self.tweaks.clip_near,
            self.tweaks.clip_far,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_declination_lifts_the_eye() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 100.0);
        c.declination = (-30.0f32).to_radians();
        let eye = c.eye();
        assert!((eye.z - 50.0).abs() < 1e-3);
        assert!((eye.length() - 100.0).abs() < 1e-3);
        c.declination = 0.0;
        c.angle = 90f32.to_radians();
        assert!((c.eye() - Vec3::new(0.0, 100.0, 0.0)).length() < 1e-3);
    }

    #[test]
    fn declination_and_angle_limits() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 100.0);
        c.rotate_declination(10.0);
        assert_eq!(c.declination, 85f32.to_radians());
        c.rotate_angle(-0.5);
        assert!((c.angle - (TAU - 0.5)).abs() < 1e-5);
    }

    #[test]
    fn zoom_clamps() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 1000.0);
        c.wheel(false, 50.0);
        assert!((c.distance - 1450.0).abs() < 1e-2);
        c.zoom_with_min(100.0, 50.0);
        assert_eq!(c.distance, 14500.0);
        c.zoom_with_min(0.0001, 300.0);
        assert_eq!(c.distance, 300.0);
        c.ignore_zoom = true;
        c.zoom(0.5);
        assert_eq!(c.distance, 150.0);
    }

    #[test]
    fn script_overrides() {
        let s =
            ScriptFile::parse("[Camera]\nCAMERA_FIELD_OF_VIEW 60.0\nCAMERA_MAX_DECLINATION 45\n");
        let t = CameraTweaks::from_script(&s);
        assert_eq!(t.field_of_view, 60.0);
        assert!((t.max_declination - 45f32.to_radians()).abs() < 1e-6);
        assert!((t.mouse_sensitivity(50) - 1.0).abs() < 1e-6);
    }
}
