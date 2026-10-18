//! Distance attenuation, EQ and panning: the game's `VOL1`/`RNG1`/`FRQ1` tables, the cardioid
//! "doppler" table, the listener geometry and the `soundevent.script` tweaks.

use hwc_formats::audio::{FloatTableLut, TableLut};
use hwc_formats::script::ScriptFile;

use crate::codec::{Eq, FLAT_EQ};

/// Distance tables with the game's precomputed slopes.
///
/// Each row (an object class) has three range breakpoints with matching volumes, plus a shared
/// per-band EQ for each breakpoint. Between breakpoints volume and EQ are interpolated linearly;
/// in front of the first breakpoint the volume rises towards 255 and the EQ towards flat.
#[derive(Debug, Clone)]
pub struct DistanceTables {
    cols: usize,
    rows: usize,
    range: Vec<i32>,
    volume: Vec<i32>,
    range_inv: Vec<f32>,
    vol_step: Vec<f32>,
    eq_bands: usize,
    eq_steps: usize,
    eq: Vec<f32>,
    eq_step: Vec<f32>,
}

/// Result of [`DistanceTables::volume_eq`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attenuation {
    /// 0..255; 0 means "do not play".
    pub volume: i16,
    /// The EQ for this distance, or `None` where the game leaves its EQ buffer unwritten (the
    /// volume is then 0 and nothing plays).
    pub eq: Option<Eq>,
}

impl DistanceTables {
    pub fn new(volume: &TableLut, range: &TableLut, freq: &FloatTableLut) -> Self {
        let cols = usize::from(range.columns);
        let rows = usize::from(range.rows).min(usize::from(volume.rows));
        let mut range_inv = vec![0.0f32; range.values.len()];
        let mut vol_step = vec![0.0f32; volume.values.len()];
        let vcols = usize::from(volume.columns);
        for row in 0..usize::from(volume.rows) {
            let j = row * vcols;
            if j >= range.values.len() {
                break;
            }
            range_inv[j] = 1.0 / range.values[j] as f32;
            vol_step[j] = (255 - volume.values[j]) as f32;
            for i in 1..cols {
                if j + i >= range.values.len() || j + i >= volume.values.len() {
                    break;
                }
                vol_step[j + i] = (volume.values[j + i - 1] - volume.values[j + i]) as f32;
                range_inv[j + i] = 1.0 / (range.values[j + i] - range.values[j + i - 1]) as f32;
            }
        }
        let eq_bands = usize::from(freq.columns);
        let eq_steps = usize::from(freq.rows);
        let mut eq_step = vec![0.0f32; freq.values.len()];
        for band in 0..eq_bands {
            eq_step[band] = FLAT_EQ.get(band).copied().unwrap_or(1.0) - freq.values[band];
            for step in 1..eq_steps {
                let k = step * eq_bands + band;
                eq_step[k] = freq.values[k - eq_bands] - freq.values[k];
            }
        }
        Self {
            cols,
            rows,
            range: range.values.clone(),
            volume: volume.values.clone(),
            range_inv,
            vol_step,
            eq_bands,
            eq_steps,
            eq: freq.values.clone(),
            eq_step,
        }
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The audible range of a row (its last breakpoint).
    pub fn max_range(&self, row: usize) -> Option<f32> {
        (row < self.rows).then(|| self.range[row * self.cols + 2] as f32)
    }

    /// The game's range check: within the row's third breakpoint.
    pub fn in_range(&self, row: usize, distance: f32) -> bool {
        self.max_range(row).is_some_and(|r| distance <= r)
    }

    /// Same, against a squared distance.
    pub fn in_range_sqr(&self, row: usize, distance_sq: f32) -> bool {
        self.max_range(row).is_some_and(|r| distance_sq <= r * r)
    }

    /// Volume and EQ for an object of class `row` at `distance` with the effects level `sfx`
    /// (0..1), dropping anything quieter than `min_volume`.
    pub fn volume_eq(&self, row: usize, distance: f32, sfx: f32, min_volume: f32) -> Attenuation {
        if row >= self.rows {
            return Attenuation { volume: 0, eq: None };
        }
        let base = row * self.cols;
        let Some(i) = (0..self.cols).find(|&i| distance <= self.range[base + i] as f32) else {
            let mut eq = FLAT_EQ;
            for b in eq.iter_mut().take(self.eq_bands) {
                *b = 1.0;
            }
            return Attenuation { volume: 0, eq: Some(eq) };
        };
        let idx = base + i;
        let mult = (self.range[idx] as f32 - distance) * self.range_inv[idx];
        let mut vol = (mult * self.vol_step[idx] + self.volume[idx] as f32) * sfx;
        if !vol.is_finite() || vol < min_volume {
            return Attenuation { volume: 0, eq: None };
        }
        if vol > 255.0 {
            vol = 255.0;
        }
        let mut eq = FLAT_EQ;
        if i < self.eq_steps {
            for (b, e) in eq.iter_mut().enumerate().take(self.eq_bands) {
                let k = i * self.eq_bands + b;
                *e = mult * self.eq_step[k] + self.eq[k];
            }
        }
        Attenuation { volume: crate::mixer::round_i32(vol) as i16, eq: Some(eq) }
    }
}

/// The cardioid table used for the fake doppler EQ, indexed by angle in degrees.
///
/// The game fills 180 entries; reading index 180 (which it does for headings of 0 and 180
/// degrees) returns whatever follows the table in memory, the `RANDOM_AMBIENCE_MINFREQ` tweak.
/// That is reproduced here.
#[derive(Debug, Clone)]
pub struct Cardioid {
    values: [f32; 181],
}

impl Cardioid {
    pub fn new(factor: f32, min: f32, past_end: f32) -> Self {
        let mut values = [0.0f32; 181];
        for (i, v) in values.iter_mut().take(180).enumerate() {
            let angle = (180 - i) as f64 * 0.017_453_296;
            let x = (angle.cos() + 1.0) * f64::from(factor);
            *v = if x < f64::from(min) { min } else { x as f32 };
        }
        values[180] = past_end;
        Self { values }
    }

    pub fn at(&self, index: i32) -> f32 {
        self.values[index.clamp(0, 180) as usize]
    }
}

/// A 3D vector as used by the camera.
pub type Vec3 = [f32; 3];

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn scale(a: Vec3, s: f32) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn normalize(a: Vec3) -> Vec3 {
    let m = dot(a, a).sqrt();
    if m == 0.0 {
        a
    } else {
        scale(a, 1.0 / m)
    }
}

/// The listener: the game camera's eye, look-at point and up vector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub eye: Vec3,
    pub look_at: Vec3,
    pub up: Vec3,
}

impl Default for Listener {
    fn default() -> Self {
        Self { eye: [0.0; 3], look_at: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0] }
    }
}

impl Listener {
    /// Distance from the eye.
    pub fn distance(&self, pos: Vec3) -> f32 {
        let d = sub(pos, self.eye);
        dot(d, d).sqrt()
    }

    /// The game's pan angle: the horizontal angle between the view direction and the object,
    /// 0 ahead, 90 right, -90 left, +-179 behind. Objects closer than twice their size are
    /// centred.
    pub fn pan_angle(&self, pos: Vec3, size: f32, distance: f32) -> i16 {
        if distance < size + size {
            return 0;
        }
        let to_point = sub(pos, self.eye);
        let look = sub(self.look_at, self.eye);
        let right = cross(look, self.up);
        let look = normalize(look);
        let along = dot(look, to_point);
        let side = dot(right, to_point) / dot(right, right);
        let proj = normalize(add(scale(look, along), scale(right, side)));
        let d = dot(proj, look).clamp(-1.0, 1.0);
        let mut angle = f64::from(d).acos();
        if side < 0.0 {
            angle = -angle;
        }
        (angle * 57.295_77).round_ties_even() as i16
    }

    /// Angle between a ship's heading and the direction to the camera, in whole degrees
    /// (`SEgetAngleToCamera`; 0 means flying straight at the camera).
    pub fn angle_to_heading(&self, pos: Vec3, heading: Vec3) -> i16 {
        let to_cam = normalize(sub(self.eye, pos));
        let h = normalize(heading);
        let d = dot(to_cam, h).clamp(-1.0, 1.0);
        (f64::from(d).acos() * 57.295_77) as i16
    }
}

/// The `soundevent.script` tweaks the audio code uses. Defaults are the shipped values.
#[derive(Debug, Clone, PartialEq)]
pub struct Tweaks {
    pub sfx_ambient_volume: i32,
    pub sfx_min_perceptible_vol: f32,
    pub sfx_hyperspace_volume: i32,
    pub sfx_cardioid_factor: f32,
    pub sfx_cardioid_min: f32,
    pub speech_min_perceptible_vol: f32,
    pub speech_ambient_level: f32,
    pub speech_singleplayer_ratio: f32,
    pub music_distance_silent: f32,
    pub music_distance_max: f32,
    pub music_maxgame_vol: f32,
    pub music_minactive_vol: f32,
    pub music_mininactive_vol: f32,
    pub music_sensors_vol: f32,
    pub music_managers_vol: f32,
    pub music_tutorial_vol: f32,
    pub music_fade_time: f32,
    pub music_maxbattle_vol: f32,
    pub music_minbattle_vol: f32,
    pub random_ambience_minfreq: f32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Self {
            sfx_ambient_volume: 77,
            sfx_min_perceptible_vol: 10.0,
            sfx_hyperspace_volume: 100,
            sfx_cardioid_factor: 1.0,
            sfx_cardioid_min: 0.43,
            speech_min_perceptible_vol: 0.0,
            speech_ambient_level: 0.6,
            speech_singleplayer_ratio: 0.5,
            music_distance_silent: 1200.0,
            music_distance_max: 3300.0,
            music_maxgame_vol: 0.9,
            music_minactive_vol: 0.3,
            music_mininactive_vol: 0.5,
            music_sensors_vol: 0.2,
            music_managers_vol: 0.2,
            music_tutorial_vol: 0.6,
            music_fade_time: 2.0,
            music_maxbattle_vol: 0.9,
            music_minbattle_vol: 0.3,
            random_ambience_minfreq: 1.5,
        }
    }
}

impl Tweaks {
    /// Read the values present in a parsed `soundevent.script`; missing keys keep defaults.
    pub fn from_script(script: &ScriptFile) -> Self {
        let mut t = Self::default();
        let f = |key: &str, v: &mut f32| {
            if let Some(x) = script.get(key).and_then(|e| e.f32()) {
                *v = x;
            }
        };
        let i = |key: &str, v: &mut i32| {
            if let Some(x) = script.get(key).and_then(|e| e.i32().or_else(|| e.f32().map(|f| f as i32))) {
                *v = x;
            }
        };
        i("SFX_AMBIENT_VOLUME", &mut t.sfx_ambient_volume);
        f("SFX_MIN_PERCEPTABLE_VOL", &mut t.sfx_min_perceptible_vol);
        i("SFX_HYPERSPACE_VOLUME", &mut t.sfx_hyperspace_volume);
        f("SFX_CARDIOD_FACTOR", &mut t.sfx_cardioid_factor);
        f("SFX_CARDIOD_MIN", &mut t.sfx_cardioid_min);
        f("SPEECH_MIN_PERCEPTABLE_VOL", &mut t.speech_min_perceptible_vol);
        f("SPEECH_AMBIENT_LEVEL", &mut t.speech_ambient_level);
        f("SPEECH_SINGLEPLAYER_RATIO", &mut t.speech_singleplayer_ratio);
        f("MUSIC_DISTANCE_SILENT", &mut t.music_distance_silent);
        f("MUSIC_DISTANCE_MAX", &mut t.music_distance_max);
        f("MUSIC_MAXGAME_VOL", &mut t.music_maxgame_vol);
        f("MUSIC_MINACTIVE_VOL", &mut t.music_minactive_vol);
        f("MUSIC_MININACTIVE_VOL", &mut t.music_mininactive_vol);
        f("MUSIC_SENSORS_VOL", &mut t.music_sensors_vol);
        f("MUSIC_MANAGERS_VOL", &mut t.music_managers_vol);
        f("MUSIC_TUTORIAL_VOL", &mut t.music_tutorial_vol);
        f("MUSIC_FADE_TIME", &mut t.music_fade_time);
        f("MUSIC_MAXBATTLE_VOL", &mut t.music_maxbattle_vol);
        f("MUSIC_MINBATTLE_VOL", &mut t.music_minbattle_vol);
        f("RANDOM_AMBIENCE_MINFREQ", &mut t.random_ambience_minfreq);
        t
    }

    /// Slopes the music volume code derives at start-up: (active, inactive, battle).
    pub fn music_slopes(&self) -> (f32, f32, f32) {
        let span = self.music_distance_max - self.music_distance_silent;
        (
            (self.music_maxgame_vol - self.music_minactive_vol) / span,
            (self.music_maxgame_vol - self.music_mininactive_vol) / span,
            (self.music_maxbattle_vol - self.music_minbattle_vol) / span,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tables() -> DistanceTables {
        let volume = TableLut { columns: 3, rows: 2, values: vec![170, 40, 10, 177, 55, 1] };
        let range = TableLut { columns: 3, rows: 2, values: vec![2000, 6000, 12000, 1600, 5000, 5000] };
        let freq = FloatTableLut {
            columns: 8,
            rows: 3,
            values: [vec![1.0; 8], vec![0.5; 8], vec![0.0; 8]].concat(),
        };
        DistanceTables::new(&volume, &range, &freq)
    }

    #[test]
    fn volume_interpolates_between_breakpoints() {
        let t = tables();
        assert_eq!(t.volume_eq(0, 0.0, 1.0, 10.0).volume, 255);
        assert_eq!(t.volume_eq(0, 2000.0, 1.0, 10.0).volume, 170);
        let a = t.volume_eq(0, 4000.0, 1.0, 10.0);
        assert_eq!(a.volume, 105);
        let eq = a.eq.unwrap();
        assert!((eq[0] - 0.75).abs() < 1e-6 && eq[8] == 1.0);
        // Below the perceptible minimum: nothing plays.
        assert_eq!(t.volume_eq(0, 11_000.0, 1.0, 10.0).volume, 15);
        assert_eq!(t.volume_eq(0, 11_000.0, 1.0, 20.0), Attenuation { volume: 0, eq: None });
        // Out of range: flat EQ, silent.
        assert_eq!(t.volume_eq(0, 13_000.0, 1.0, 10.0).volume, 0);
        assert!(t.in_range(0, 12_000.0) && !t.in_range(0, 12_001.0));
        // Effects volume scales the result.
        assert_eq!(t.volume_eq(0, 2000.0, 0.5, 10.0).volume, 85);
    }

    #[test]
    fn degenerate_breakpoints_do_not_produce_garbage() {
        let t = tables();
        assert_eq!(t.volume_eq(1, 5000.0, 1.0, 0.0).volume, 55);
    }

    #[test]
    fn pan_angle_quadrants() {
        let l = Listener { eye: [0.0; 3], look_at: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0] };
        assert_eq!(l.pan_angle([100.0, 0.0, 0.0], 1.0, 100.0), 0);
        // look x up = (1,0,0) x (0,0,1) = (0,-1,0): the game's "right" is -y here.
        assert_eq!(l.pan_angle([0.0, -100.0, 0.0], 1.0, 100.0), 90);
        assert_eq!(l.pan_angle([0.0, 100.0, 0.0], 1.0, 100.0), -90);
        assert_eq!(l.pan_angle([-100.0, -1.0, 0.0], 1.0, 100.0).abs(), 179);
        assert_eq!(l.pan_angle([0.0, -100.0, 0.0], 60.0, 100.0), 0);
    }

    #[test]
    fn cardioid_shape() {
        let c = Cardioid::new(1.0, 0.43, 1.5);
        assert_eq!(c.at(0), 0.43);
        assert!((c.at(90) - 1.0).abs() < 1e-3);
        assert!((c.at(179) - 1.99985).abs() < 1e-3);
        assert_eq!(c.at(180), 1.5);
    }
}
