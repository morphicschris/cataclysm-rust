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
    /// `Camera.upvector` (+0x34) when it is not world +z: the NIS camera's bank
    /// (`nisCamPathUpdate` sets it from the camera path's orientation; the view is
    /// `gluLookAt(eye, lookat, upvector)`). `None` for the game camera, whose up is +z.
    pub up_vector: Option<Vec3>,
    /// The distance the smooth wheel zoom is easing towards ([`wheel_smooth`](Self::wheel_smooth),
    /// [`ease_zoom`](Self::ease_zoom)); `None` when the camera is not easing. A modern option:
    /// the original's wheel steps the distance at once ([`wheel`](Self::wheel)).
    pub zoom_target: Option<f32>,
}

/// Time constant of the smooth wheel zoom, in seconds: each notch's step is 95% done after
/// three of them (0.24 s).
pub const ZOOM_EASE_TIME: f32 = 0.08;

/// One ship the camera is focused on, for the distance that frames it (`NewSetFocusPoint`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FocusShip {
    /// Its collision centre.
    pub centre: Vec3,
    /// Its collision sphere radius.
    pub radius: f32,
    /// Its static `minimumZoomDistance` (0 when it has none).
    pub minimum_zoom: f32,
    /// The radius of everything drawn of it (at least the collision radius), for
    /// [`OrbitCamera::clear_distance`].
    pub drawn_radius: f32,
}

/// `NewSetFocusPoint`'s fit constants: the cotangent of half the 75 degree field of view, and
/// its inverse.
const FIT_COT: f64 = 1.303225;
const FIT_TAN: f64 = 0.767327207;

/// Where `ship` lies seen from the look-at point: its distance along the line to the eye
/// (positive towards the eye) and its distance from that line.
fn along_and_across(look_at: Vec3, toward_eye: Vec3, centre: Vec3) -> (f64, f64) {
    let rel = (centre - look_at).as_dvec3();
    let len = rel.length();
    if len <= 0.0 {
        return (0.0, 0.0);
    }
    let cos = (rel / len).dot(toward_eye.as_dvec3());
    (cos * len, (1.0 - cos * cos).max(0.0).sqrt() * len)
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
            up_vector: None,
            zoom_target: None,
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

    /// The view's up direction: [`up_vector`](Self::up_vector), else world +z.
    pub fn up(&self) -> Vec3 {
        self.up_vector.unwrap_or(Vec3::Z)
    }

    /// Places the camera at `eye` looking at `look_at` (distance, angle and declination are
    /// derived; the declination is not clamped), with `up` as the view's up direction (`None`
    /// for +z). This is how the NIS camera (`nisCamPathUpdate`: eye, look-at point and up
    /// vector) drives the view. Does nothing when `eye` and `look_at` (nearly) coincide or
    /// are not finite.
    pub fn set_eye_look_up(&mut self, eye: Vec3, look_at: Vec3, up: Option<Vec3>) {
        let d = eye - look_at;
        let dist = d.length();
        if !dist.is_finite() || dist <= 1e-3 {
            return;
        }
        self.look_at = look_at;
        self.distance = dist;
        self.angle = d.y.atan2(d.x);
        if self.angle < 0.0 {
            self.angle += TAU;
        }
        self.declination = (-d.z / dist).clamp(-1.0, 1.0).asin();
        self.up_vector = up.filter(|v| v.is_finite() && v.length_squared() > 0.0);
        self.zoom_target = None;
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
        // Any other zoom takes over from an easing wheel zoom where it has got to.
        self.zoom_target = None;
        self.distance = self.limit_zoom(self.distance * factor, min_zoom);
    }

    /// `distance` within the zoom limits (`cameraZoom`'s clamps: the far limit, then
    /// `min_zoom`), unless they are ignored.
    fn limit_zoom(&self, mut distance: f32, min_zoom: f32) -> f32 {
        if !self.ignore_zoom {
            if distance > self.farthest_zoom {
                distance = self.farthest_zoom;
            }
            if distance < min_zoom {
                distance = min_zoom;
            }
        }
        distance
    }

    pub fn zoom(&mut self, factor: f32) {
        self.zoom_with_min(factor, self.closest_zoom);
    }

    /// The distance factor of one wheel notch.
    fn wheel_factor(&self, toward: bool) -> f32 {
        let t = &self.tweaks;
        if toward {
            1.0 - (1.0 - t.wheel_zoom_in) * self.mouse_sens
        } else {
            (t.wheel_zoom_out - 1.0) * self.mouse_sens + 1.0
        }
    }

    /// One mouse-wheel notch towards (`toward = true`) or away from the look-at point: the
    /// original's immediate step (`cameraControl`).
    pub fn wheel(&mut self, toward: bool, min_zoom: f32) {
        self.zoom_with_min(self.wheel_factor(toward), min_zoom);
    }

    /// One wheel notch of the smooth zoom: the same step and limits as [`wheel`](Self::wheel),
    /// applied to the target distance (notches add up), which [`ease_zoom`](Self::ease_zoom)
    /// then moves the camera to.
    pub fn wheel_smooth(&mut self, toward: bool, min_zoom: f32) {
        let from = self.zoom_target.unwrap_or(self.distance);
        let target = self.limit_zoom(from * self.wheel_factor(toward), min_zoom);
        self.zoom_target = Some(target);
    }

    /// Moves the distance towards the smooth zoom's target: `dt` seconds of exponential easing
    /// in log distance, the same at any frame rate (two frames of `dt / 2` end where one frame
    /// of `dt` does). The target is kept within the limits (`min_zoom` may change, e.g. on
    /// focusing a large ship) and dropped once reached.
    pub fn ease_zoom(&mut self, dt: f32, min_zoom: f32) {
        let Some(target) = self.zoom_target else {
            return;
        };
        let target = self.limit_zoom(target, min_zoom);
        let sane = |d: f32| d.is_finite() && d > 0.0;
        if !(sane(target) && sane(self.distance)) {
            self.zoom_target = None;
            return;
        }
        let k = 1.0 - (-dt.max(0.0) / ZOOM_EASE_TIME).exp();
        let (from, to) = (self.distance.ln(), target.ln());
        let d = from + (to - from) * k;
        if (to - d).abs() < 1e-4 {
            self.distance = target;
            self.zoom_target = None;
        } else {
            self.distance = d.exp();
            self.zoom_target = Some(target);
        }
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

    /// The unit direction from the look-at point to the eye.
    fn toward_eye(&self) -> Vec3 {
        let (sd, cd) = self.declination.sin_cos();
        let (sa, ca) = self.angle.sin_cos();
        Vec3::new(cd * ca, cd * sa, -sd)
    }

    /// `NewSetFocusPoint`'s target distance: the closest the camera may come to the focused
    /// ships (after the player zooms, `ccControl` keeps it at least this far) and where a
    /// close-up focus (`zoomInCloseAsPossible`: Alt-focus, or focusing again on what is
    /// already in focus) takes it. Each ship is framed by its collision radius (or by its
    /// minimum zoom distance where it has one) with the field of view the constants assume,
    /// plus how far it lies towards the eye; never nearer than [`closest_zoom`](Self::closest_zoom).
    pub fn focus_distance(&self, ships: &[FocusShip]) -> f32 {
        let close = f64::from(self.closest_zoom);
        if ships.is_empty() {
            return self.closest_zoom;
        }
        let toward = self.toward_eye();
        let mut far = 0.0f64;
        for s in ships {
            let (along, across) = along_and_across(self.look_at, toward, s.centre);
            let reach = if s.minimum_zoom < 0.1 {
                across + f64::from(s.radius)
            } else {
                across + f64::from(s.minimum_zoom) * FIT_TAN
            };
            let d = (reach * FIT_COT).max(close) + along;
            far = far.max(d);
        }
        far as f32
    }

    /// A modern addition to [`focus_distance`](Self::focus_distance): the distance at which the
    /// near clip plane stays outside every focused ship's drawn sphere (with room for the near
    /// plane's corners), so zooming in never cuts into a hull.
    pub fn clear_distance(&self, ships: &[FocusShip]) -> f32 {
        let toward = self.toward_eye();
        let half = (self.tweaks.field_of_view.to_radians() * 0.5).tan();
        // The near plane's corner, for any picture up to twice as wide as it is high.
        let margin = self.tweaks.clip_near * (1.0 + half * half * 5.0).sqrt();
        let mut far = 0.0f64;
        for s in ships {
            let (along, across) = along_and_across(self.look_at, toward, s.centre);
            let r = f64::from(s.drawn_radius.max(s.radius) + margin);
            if across < r {
                far = far.max(along + (r * r - across * across).sqrt());
            }
        }
        far as f32
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
    fn smooth_wheel_zoom_eases_frame_rate_independently() {
        let mut a = OrbitCamera::new(CameraTweaks::default(), 1000.0);
        a.wheel_smooth(true, a.closest_zoom);
        // The target is the original's one-notch step; nothing moves before time passes.
        assert_eq!(a.zoom_target, Some(700.0));
        assert_eq!(a.distance, 1000.0);
        let mut b = a;
        // 0.1 s as one frame or as ten.
        a.ease_zoom(0.1, a.closest_zoom);
        for _ in 0..10 {
            b.ease_zoom(0.01, b.closest_zoom);
        }
        assert!((a.distance - b.distance).abs() < 0.05, "{} vs {}", a.distance, b.distance);
        assert!(a.distance < 1000.0 && a.distance > 700.0);
        // Exponential in log distance: after one time constant, 1 - 1/e of the way.
        let mut c = OrbitCamera::new(CameraTweaks::default(), 1000.0);
        c.wheel_smooth(true, c.closest_zoom);
        c.ease_zoom(ZOOM_EASE_TIME, c.closest_zoom);
        let done = (1000f32.ln() - c.distance.ln()) / (1000f32.ln() - 700f32.ln());
        assert!((done - (1.0 - (-1f32).exp())).abs() < 1e-3, "{done}");
        // It arrives and stops easing.
        for _ in 0..200 {
            a.ease_zoom(1.0 / 60.0, a.closest_zoom);
        }
        assert_eq!(a.distance, 700.0);
        assert_eq!(a.zoom_target, None);
    }

    #[test]
    fn smooth_wheel_zoom_keeps_the_limits_and_adds_notches() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 1000.0);
        c.wheel_smooth(false, c.closest_zoom);
        c.wheel_smooth(false, c.closest_zoom);
        assert!((c.zoom_target.unwrap() - 1000.0 * 1.45 * 1.45).abs() < 0.01);
        for _ in 0..40 {
            c.wheel_smooth(false, c.closest_zoom);
        }
        assert_eq!(c.zoom_target, Some(c.farthest_zoom));
        for _ in 0..80 {
            c.wheel_smooth(true, 300.0);
        }
        assert_eq!(c.zoom_target, Some(300.0));
        // Never past a limit on the way.
        let mut d = OrbitCamera::new(CameraTweaks::default(), 14000.0);
        d.wheel_smooth(false, d.closest_zoom);
        for _ in 0..100 {
            d.ease_zoom(0.05, d.closest_zoom);
            assert!(d.distance <= d.farthest_zoom);
        }
        // A key zoom takes over where the easing has got to.
        let mut e = OrbitCamera::new(CameraTweaks::default(), 1000.0);
        e.wheel_smooth(true, e.closest_zoom);
        e.ease_zoom(0.05, e.closest_zoom);
        let at = e.distance;
        e.key_zoom(true, e.closest_zoom);
        assert_eq!(e.zoom_target, None);
        assert!((e.distance - at * 0.94).abs() < 0.01);
    }

    fn ship(centre: Vec3, radius: f32, minimum_zoom: f32) -> FocusShip {
        FocusShip { centre, radius, minimum_zoom, drawn_radius: radius }
    }

    #[test]
    fn focus_distance_frames_the_ships() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 5000.0);
        c.look_at = Vec3::new(100.0, 200.0, 0.0);
        c.declination = (-30f32).to_radians();
        // Nothing in focus: the camera's own limit.
        assert_eq!(c.focus_distance(&[]), 50.0);
        // One ship at the look-at point: its radius over tan(37.5 degrees).
        let one = [ship(c.look_at, 300.0, 0.0)];
        assert!((c.focus_distance(&one) - 300.0 * 1.303225).abs() < 0.01);
        // A minimum zoom distance replaces the radius, and is the distance itself.
        let mzd = [ship(c.look_at, 300.0, 2000.0)];
        assert!((c.focus_distance(&mzd) - 2000.0).abs() < 0.5);
        // A small ship is held at the closest zoom.
        assert_eq!(c.focus_distance(&[ship(c.look_at, 10.0, 0.0)]), 50.0);
        // A ship off to the side widens the frame; one towards the eye pushes it back by as
        // much as it is nearer.
        let side = c.look_at + Vec3::new(0.0, 0.0, 1.0).cross(c.eye() - c.look_at).normalize() * 400.0;
        assert!((c.focus_distance(&[ship(side, 100.0, 0.0)]) - 500.0 * 1.303225).abs() < 0.05);
        let toward = c.look_at + (c.eye() - c.look_at).normalize() * 400.0;
        let d = c.focus_distance(&[ship(toward, 100.0, 0.0)]);
        assert!((d - (100.0 * 1.303225 + 400.0)).abs() < 0.5, "{d}");
        // The farthest requirement of a group wins.
        let both = [one[0], ship(side, 100.0, 0.0)];
        assert!((c.focus_distance(&both) - 500.0 * 1.303225).abs() < 0.05);
    }

    #[test]
    fn clear_distance_keeps_the_near_plane_outside_the_hull() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 5000.0);
        let s = FocusShip { centre: Vec3::ZERO, radius: 40.0, minimum_zoom: 0.0, drawn_radius: 45.0 };
        let d = c.clear_distance(&[s]);
        // Beyond the drawn radius by more than the near plane's distance.
        assert!(d > 45.0 + c.tweaks.clip_near, "{d}");
        assert!(d < 45.0 + c.tweaks.clip_near * 2.0, "{d}");
        // The original's limit for this ship (the closest zoom of 50) would put the near plane
        // inside it.
        assert!(c.focus_distance(&[s]) < d);
        // A ship well off the line of sight sets no limit.
        let off = FocusShip { centre: Vec3::new(0.0, 500.0, 0.0), ..s };
        c.angle = 0.0;
        assert_eq!(c.clear_distance(&[off]), 0.0);
    }

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
    fn explicit_up_vector_banks_the_view() {
        let mut c = OrbitCamera::new(CameraTweaks::default(), 100.0);
        let eye = Vec3::new(100.0, 0.0, 0.0);
        c.set_eye_look_up(eye, Vec3::ZERO, None);
        assert!((c.eye() - eye).length() < 1e-3);
        assert_eq!(c.up(), Vec3::Z);
        // Banked 90 degrees: the world's +y is the picture's up.
        c.set_eye_look_up(eye, Vec3::ZERO, Some(Vec3::Y));
        let v = c.view();
        let up_in_view = v.transform_vector3(Vec3::Y);
        assert!((up_in_view - Vec3::Y).length() < 1e-5);
        let z_in_view = v.transform_vector3(Vec3::Z);
        assert!(z_in_view.y.abs() < 1e-5);
        // The sky rotation banks the same way.
        let r = c.view_rotation().transform_vector3(Vec3::Y);
        assert!((r - Vec3::Y).length() < 1e-5);
        // A zero or non-finite up falls back to +z; coinciding points change nothing.
        c.set_eye_look_up(eye, Vec3::ZERO, Some(Vec3::ZERO));
        assert_eq!(c.up(), Vec3::Z);
        let before = c;
        c.set_eye_look_up(Vec3::ONE, Vec3::ONE, Some(Vec3::X));
        assert_eq!(c, before);
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
