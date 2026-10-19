//! Mesh animation from `.mad` files.
//!
//! Each animated mesh object follows six keyframed channels (x, y, z, heading, pitch, bank)
//! interpolated with Kochanek-Bartels (tension/continuity/bias) Hermite splines. An animation is
//! a time window of the file's shared timeline. Cataclysm adds a per-ship-type remap from the
//! animation numbers the game uses (the `.shp` file's `ANIM n name` lines) to the file's
//! animations, and lets the caller choose looping and what happens at the end instead of using
//! the file's loop flag.
//!
//! [`ShipAnimator`] is the per-instance playback state; [`MadClip::pose`] turns it into local
//! object matrices that [`crate::mesh::Skeleton::resolve`] composes down the hierarchy.

use glam::{Mat3, Mat4, Vec3};
use hwc_formats::mad::{Keyframe, Mad, ObjectPath};
use hwc_formats::script::ShipStats;

use crate::mesh::Skeleton;

/// Playback flag: wrap around at the end of the animation.
pub const ANIM_LOOP: u32 = 0x1;
/// Playback flag: when a non-looping animation ends, stop it and return to the rest pose
/// instead of holding the last frame.
pub const ANIM_STOP_AT_END: u32 = 0x2;

/// One evaluated spline channel value at time `t`, starting from key `start`.
///
/// The keys carry one padding key before the first real one and after the last, as in the
/// files; the segment `[k, k+1]` containing `t` is interpolated with tangents from its
/// neighbours. Returns `None` past the last segment.
fn channel_value(
    keys: &[Keyframe],
    value: impl Fn(&Keyframe) -> f32,
    start: usize,
    t: f32,
) -> Option<f32> {
    let n = keys.len();
    if n < 2 {
        return keys.first().map(&value);
    }
    let mut cp = start.max(1).min(n - 2);
    while t > keys.get(cp + 1)?.time {
        cp += 1;
        if cp >= n - 1 {
            return None;
        }
    }
    let at = |i: usize| keys[i.min(n - 1)].time;
    let val = |i: usize| value(&keys[i.min(n - 1)]);
    let key_len = at(cp + 1) - at(cp);
    let s = if key_len > 0.0 {
        (t - at(cp)) / key_len
    } else {
        0.0
    };
    let s2 = s * s;
    let s3 = s2 * s;
    let z = 3.0 * s2 - 2.0 * s3;
    let h = [1.0 - z, z, s3 - 2.0 * s2 + s, s3 - s2];
    let prev = cp.saturating_sub(1);
    let adj0 = key_len / (at(cp + 1) - at(prev)).max(f32::EPSILON);
    let adj1 = key_len / (at(cp + 2) - at(cp)).max(f32::EPSILON);
    let [t0, c0, b0] = keys[cp].tcb;
    let [t1, c1, b1] = keys[(cp + 1).min(n - 1)].tcb;
    let dd0a = (1.0 - t0) * (1.0 + c0) * (1.0 + b0);
    let dd0b = (1.0 - t0) * (1.0 - c0) * (1.0 - b0);
    let ds1a = (1.0 - t1) * (1.0 - c1) * (1.0 + b1);
    let ds1b = (1.0 - t1) * (1.0 + c1) * (1.0 - b1);
    let delta = val(cp + 1) - val(cp);
    Some(
        h[0] * val(cp)
            + h[1] * val(cp + 1)
            + h[2] * (adj0 * (dd0a * (val(cp) - val(prev)) + dd0b * delta))
            + h[3] * (adj1 * (ds1a * delta + ds1b * (val(cp + 2) - val(cp + 1)))),
    )
}

/// Heading/pitch/bank to a rotation: heading about +X (negated), then pitch about the turned
/// +Z, then bank about the turned +Y (negated).
pub fn euler_to_matrix(heading: f32, pitch: f32, bank: f32) -> Mat3 {
    Mat3::from_rotation_x(-heading) * Mat3::from_rotation_z(pitch) * Mat3::from_rotation_y(-bank)
}

/// An object's local matrix on its path at time `t` (path channels map to the mesh's axes as
/// x = path y, y = path z, z = -path x).
pub fn path_matrix(path: &ObjectPath, start_key: usize, t: f32) -> Option<Mat4> {
    let k = &path.keyframes;
    let c = |f: fn(&Keyframe) -> f32| channel_value(k, f, start_key, t);
    let px = c(|k| k.position[0])?;
    let py = c(|k| k.position[1])?;
    let pz = c(|k| k.position[2])?;
    let h = c(|k| k.rotation[0])?;
    let p = c(|k| k.rotation[1])?;
    let b = c(|k| k.rotation[2])?;
    let rot = euler_to_matrix(h, p, b);
    Some(Mat4::from_mat3_translation(rot, Vec3::new(py, pz, -px)))
}

/// A ship type's animation data: the `.mad` file plus the ship's animation remap.
#[derive(Debug, Clone)]
pub struct MadClip {
    pub mad: Mad,
    /// Ship animation number to file animation index (from `ANIM n name`); empty when the ship
    /// type defines none, in which case numbers index the file directly.
    pub remap: Vec<usize>,
}

impl MadClip {
    pub fn new(mad: Mad) -> Self {
        Self {
            mad,
            remap: Vec::new(),
        }
    }

    /// Builds the remap from the `.shp` file's `ANIM n name` lines.
    pub fn with_ship_stats(mad: Mad, stats: &ShipStats) -> Self {
        let mut remap = Vec::new();
        if let Some(lines) = stats.other.get("ANIM") {
            for e in lines {
                let (Some(n), Some(name)) = (
                    e.values.first().and_then(|v| v.as_i64()),
                    e.values.get(1).map(|v| v.to_string()),
                ) else {
                    continue;
                };
                let Some(file_index) = mad.animation_index(&name) else {
                    continue;
                };
                let n = n.max(0) as usize;
                if remap.len() <= n {
                    remap.resize(n + 1, 0);
                }
                remap[n] = file_index;
            }
        }
        Self { mad, remap }
    }

    /// File animation index for a ship animation number.
    pub fn file_animation(&self, anim: usize) -> Option<usize> {
        let i = if self.remap.is_empty() {
            anim
        } else {
            *self.remap.get(anim)?
        };
        (i < self.mad.animations.len()).then_some(i)
    }

    /// Ship animation number of a named file animation.
    pub fn find(&self, name: &str) -> Option<usize> {
        let file = self.mad.animation_index(name)?;
        if self.remap.is_empty() {
            Some(file)
        } else {
            self.remap.iter().position(|&f| f == file)
        }
    }

    /// For each skeleton object, the index of the path that drives it (by name).
    pub fn bind(&self, skeleton: &Skeleton) -> Vec<Option<usize>> {
        skeleton
            .objects
            .iter()
            .map(|o| {
                self.mad
                    .objects
                    .iter()
                    .position(|p| p.name.eq_ignore_ascii_case(&o.name))
            })
            .collect()
    }

    /// Local matrices for the skeleton at the animator's current time: `None` entries keep
    /// their rest matrix. Returns `None` when nothing is playing.
    pub fn pose(
        &self,
        animator: &ShipAnimator,
        binding: &[Option<usize>],
    ) -> Option<Vec<Option<Mat4>>> {
        let anim = animator.current?;
        let file = self.file_animation(anim)?;
        let a = &self.mad.animations[file];
        let t = animator.time.min(a.end_time);
        Some(
            binding
                .iter()
                .map(|path| {
                    let path = &self.mad.objects[(*path)?];
                    if !path.in_animation(file) {
                        return None;
                    }
                    let start = path
                        .keyframes
                        .iter()
                        .position(|k| k.time == a.start_time)
                        .unwrap_or(1);
                    path_matrix(path, start, t)
                })
                .collect(),
        )
    }
}

/// Per-instance animation playback state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShipAnimator {
    /// Ship animation number playing, if any.
    pub current: Option<usize>,
    /// The animation that played before the current one.
    pub previous: Option<usize>,
    pub flags: u32,
    pub paused: bool,
    /// Position on the file timeline, seconds.
    pub time: f32,
}

impl ShipAnimator {
    /// Starts ship animation `anim` from its first frame with `flags` ([`ANIM_LOOP`],
    /// [`ANIM_STOP_AT_END`]).
    pub fn play(&mut self, clip: &MadClip, anim: usize, flags: u32) -> bool {
        let Some(file) = clip.file_animation(anim) else {
            return false;
        };
        if self.current.is_some() {
            self.previous = self.current;
        }
        self.paused = false;
        self.current = Some(anim);
        self.flags = flags;
        self.time = clip.mad.animations[file].start_time;
        true
    }

    /// Stops playback; the mesh returns to its rest pose.
    pub fn stop(&mut self) {
        if self.current.is_some() {
            self.previous = self.current;
        }
        self.current = None;
        self.paused = false;
    }

    /// Freezes (or resumes) the current pose.
    pub fn pause(&mut self, freeze: bool) {
        self.paused = freeze;
    }

    pub fn is_playing(&self) -> bool {
        self.current.is_some() && !self.paused
    }

    /// Advances by `dt` seconds. Returns true when a non-looping animation reaches its end:
    /// it then holds its last frame (paused), or stops if [`ANIM_STOP_AT_END`] was given.
    pub fn update(&mut self, clip: &MadClip, dt: f32) -> bool {
        let Some(anim) = self.current else {
            return false;
        };
        if self.paused {
            return false;
        }
        let Some(file) = clip.file_animation(anim) else {
            self.stop();
            return true;
        };
        let a = &clip.mad.animations[file];
        if self.time + dt > a.end_time {
            if self.flags & ANIM_LOOP != 0 {
                let len = a.end_time - a.start_time;
                if len <= 0.0 {
                    self.time = a.start_time;
                    return false;
                }
                // Wrap the way the engine does: subtract whole animation lengths until the next
                // step fits.
                loop {
                    self.time = self.time + dt - a.end_time + a.start_time;
                    if self.time + dt <= a.end_time {
                        break;
                    }
                }
                return false;
            }
            if self.flags & ANIM_STOP_AT_END == 0 {
                self.time = a.end_time;
                self.paused = true;
            } else {
                self.stop();
            }
            return true;
        }
        self.time += dt;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwc_formats::mad::Animation;

    fn key(time: f32, x: f32) -> Keyframe {
        Keyframe {
            time,
            tcb: [0.0; 3],
            position: [x, 0.0, 0.0],
            rotation: [0.0; 3],
        }
    }

    fn clip() -> MadClip {
        let keys = vec![
            key(-1.0, 0.0),
            key(0.0, 0.0),
            key(1.0, 10.0),
            key(2.0, 10.0),
            key(3.0, 10.0),
        ];
        MadClip::new(Mad {
            version: 0.91,
            length: 2.0,
            frames_per_second: 30.0,
            animations: vec![
                Animation {
                    name: "Open".into(),
                    start_time: 0.0,
                    end_time: 1.0,
                    flags: 0,
                },
                Animation {
                    name: "Spin".into(),
                    start_time: 1.0,
                    end_time: 2.0,
                    flags: 1,
                },
            ],
            objects: vec![ObjectPath {
                name: "Arm".into(),
                animation_bits: 3,
                keyframes: keys,
            }],
        })
    }

    #[test]
    fn spline_passes_through_keys() {
        let c = clip();
        let k = &c.mad.objects[0].keyframes;
        let v = |t| channel_value(k, |k| k.position[0], 1, t).unwrap();
        assert!(v(0.0).abs() < 1e-6);
        assert!((v(1.0) - 10.0).abs() < 1e-5);
        let mid = v(0.5);
        assert!(mid > 0.0 && mid < 10.0, "{mid}");
        assert!(channel_value(k, |k| k.position[0], 1, 10.0).is_none());
    }

    #[test]
    fn path_axes_map_to_mesh_axes() {
        let c = clip();
        let m = path_matrix(&c.mad.objects[0], 1, 1.0).unwrap();
        // Path x becomes mesh -z.
        assert!((m.transform_point3(Vec3::ZERO) - Vec3::new(0.0, 0.0, -10.0)).length() < 1e-4);
    }

    #[test]
    fn euler_order() {
        let m = euler_to_matrix(0.0, std::f32::consts::FRAC_PI_2, 0.0);
        assert!((m * Vec3::X - Vec3::Y).length() < 1e-6);
        let m = euler_to_matrix(std::f32::consts::FRAC_PI_2, 0.0, 0.0);
        assert!((m * Vec3::Y - -Vec3::Z).length() < 1e-6);
    }

    #[test]
    fn non_looping_holds_last_frame() {
        let c = clip();
        let mut a = ShipAnimator::default();
        assert!(a.play(&c, 0, 0));
        assert!(!a.update(&c, 0.5));
        assert!(a.update(&c, 0.6));
        assert!(a.paused);
        assert_eq!(a.time, 1.0);
        assert!(c.pose(&a, &[Some(0)]).unwrap()[0].is_some());
        let mut s = ShipAnimator::default();
        s.play(&c, 0, ANIM_STOP_AT_END);
        assert!(s.update(&c, 2.0));
        assert_eq!(s.current, None);
        assert!(c.pose(&s, &[Some(0)]).is_none());
    }

    #[test]
    fn looping_wraps() {
        let c = clip();
        let mut a = ShipAnimator::default();
        a.play(&c, 1, ANIM_LOOP);
        a.update(&c, 0.75);
        assert!(!a.update(&c, 0.5));
        assert!((a.time - 1.25).abs() < 1e-5, "{}", a.time);
    }

    #[test]
    fn remap_from_ship_lines() {
        let script = hwc_formats::script::ScriptFile::parse("ANIM 0 Spin\nANIM 1 Open\n");
        let stats = ShipStats::from_script(&script);
        let c = MadClip::with_ship_stats(clip().mad, &stats);
        assert_eq!(c.remap, vec![1, 0]);
        assert_eq!(c.file_animation(0), Some(1));
        assert_eq!(c.find("Open"), Some(1));
    }
}
