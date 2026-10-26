//! Kochanek–Bartels (TCB) Hermite spline curves (`B-Spline.c`), Cataclysm version.
//!
//! Cataclysm's `bsCurveUpdate` (0x423060) differs from HW1:
//! * a curve sitting on its last point returns that point's value instead of `REALlyBig`,
//!   and running off the end returns the last point;
//! * a key shorter than ~0.0343 s (`0.034333333f`) whose own tension and the next key's
//!   tension are both non-zero is skipped: time is snapped to the end of the key;
//! * the arithmetic order (traced from the binary) differs from the HW1 source.
//!
//! The original keeps a single global cache (last scaled time, Hermite basis, and the four
//! tangent coefficients keyed on the address of the current key). Because the cache can make
//! the result depend on the previous call (the scaled-time test compares a `float` with a
//! register value), it is reproduced as an explicit [`SplineCache`] that callers share the
//! way the game shares the globals. The key identity is the address of the key in the
//! curve's parameter array, which is the same notion of identity the original uses.

use crate::fpu::{eq_or_unordered, ld, st, Ext};
use crate::REALLY_BIG;

/// `tcb`: tension, continuity, bias of one key.
#[derive(Clone, Copy, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[repr(C)]
pub struct Tcb {
    pub tension: f32,
    pub continuity: f32,
    pub bias: f32,
}

/// `splinecurve`.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SplineCurve {
    pub time_elapsed: f32,
    pub current_point: i32,
    pub n_points: i32,
    pub points: Vec<f32>,
    pub times: Vec<f32>,
    pub params: Vec<Tcb>,
}

/// The function-static state of `bsCurveUpdate`.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SplineCache {
    pub last_time: f32,
    pub hermite: [f32; 4],
    pub old_key: Option<usize>,
    pub dd0a: f32,
    pub dd0b: f32,
    pub ds1a: f32,
    pub ds1b: f32,
}

impl Default for SplineCache {
    fn default() -> Self {
        Self {
            last_time: crate::REALLY_NEGATIVE,
            hermite: [0.0; 4],
            old_key: None,
            dd0a: 0.0,
            dd0b: 0.0,
            ds1a: 0.0,
            ds1b: 0.0,
        }
    }
}

/// Keys shorter than this (seconds) may be skipped.
const SHORT_KEY: f32 = 0.034_333_333;

impl SplineCurve {
    /// `bsCurveStartPrealloced`: arrays already include the extra control points (one before
    /// the first key and two after the last). On the final segment the original reads one
    /// element past the end of `points` and `times`; here those reads continue into `times`
    /// and `params` respectively, so append the values the game's memory would hold if they
    /// matter to the caller.
    pub fn prealloced(points: Vec<f32>, times: Vec<f32>, params: Vec<Tcb>) -> Self {
        let n = points.len() as i32;
        Self {
            time_elapsed: 0.0,
            current_point: 1,
            n_points: n,
            points,
            times,
            params,
        }
    }

    /// `bsCurveStart(..., bAlloc = TRUE)`: copy the keys and add the extra control points.
    ///
    /// `times` must hold at least `points.len() + 1` entries (the original reads one past the
    /// key count; the last value becomes the end of the final segment, plus one second).
    ///
    /// The original's final segment reads one element past the end of `points` and `times`
    /// (see [`SplineCurve::update`]); with this layout that is `times[0]` and the first
    /// key's tension, reproduced by reading through in the same order.
    ///
    /// Deviations from the original, which has memory bugs here: it leaves `times[0]`
    /// uninitialised (only its low byte is written, from bytes past the end of the source
    /// points) and writes 9 bytes past its allocation. `times[0]` is set to `0.0` here.
    pub fn start(points: &[f32], times: &[f32], params: &[Tcb]) -> Self {
        let n = points.len();
        assert!(n >= 1 && times.len() > n && params.len() >= n);
        let total = n + 3;
        let mut p = vec![0.0f32; total];
        p[1..=n].copy_from_slice(points);
        p[0] = p[1];
        p[total - 2] = p[total - 3];
        p[total - 1] = p[total - 2];
        let mut t = vec![0.0f32; total];
        t[1..=n + 1].copy_from_slice(&times[..=n]);
        t[total - 2] += 1.0;
        t[total - 1] = t[total - 2] + 1.0;
        let mut k = vec![Tcb::default(); total];
        k[1..=n].copy_from_slice(&params[..n]);
        k[0] = k[1];
        k[total - 2] = k[total - 3];
        k[total - 1] = k[total - 2];
        Self {
            time_elapsed: 0.0,
            current_point: 1,
            n_points: total as i32,
            points: p,
            times: t,
            params: k,
        }
    }

    /// `points[i]`, continuing into `times` and then `params` past the end, as the single
    /// allocation of `bsCurveStart` lays them out. On the final segment the original reads
    /// `points[cp + 2]` and `times[cp + 2]`, one element past the end of each array.
    fn point_at(&self, i: usize) -> f32 {
        match self.points.get(i) {
            Some(&v) => v,
            None => self.time_at(i - self.points.len()),
        }
    }

    /// `times[i]`, continuing into `params` (as raw floats) past the end.
    fn time_at(&self, i: usize) -> f32 {
        match self.times.get(i) {
            Some(&v) => v,
            None => {
                let j = i - self.times.len();
                self.params
                    .get(j / 3)
                    .map(|k| [k.tension, k.continuity, k.bias][j % 3])
                    .unwrap_or(0.0)
            }
        }
    }

    /// `bsCurveRestart`.
    pub fn restart(&mut self) {
        self.time_elapsed = 0.0;
        self.current_point = 1;
    }

    /// `bsCurveUpdate`: advance by `dt` seconds and return the curve value (a register
    /// value; the caller rounds where it stores). Returns [`REALLY_BIG`] past the end in the
    /// one remaining case the Cataclysm code still does.
    pub fn update(&mut self, cache: &mut SplineCache, dt: f32) -> Ext {
        let te = ld(dt) + ld(self.time_elapsed);
        self.time_elapsed = st(te);
        let n = self.n_points;
        let mut cp = self.current_point;
        if cp == n - 1 {
            return ld(self.point_at(cp as usize));
        }
        while te > ld(self.time_at(cp as usize + 1)) {
            cp += 1;
            self.current_point = cp;
            if cp >= n - 1 {
                return ld(self.point_at((n - 1) as usize));
            }
        }
        let p = |i: i32| ld(self.point_at(i as usize));
        let tm = |i: i32| ld(self.time_at(i as usize));

        let key_len_ext = tm(cp + 1) - tm(cp);
        let key_len = ld(st(key_len_ext));
        let mut t = te;
        // `TEST AH,0x41` pattern: taken unless the key is strictly longer (NaN counts as short).
        if key_len_ext.partial_cmp(&ld(SHORT_KEY)) != Some(std::cmp::Ordering::Greater)
            && !eq_or_unordered(ld(self.params[cp as usize].tension), 0.0)
            && !eq_or_unordered(ld(self.params[cp as usize + 1].tension), 0.0)
        {
            t = tm(cp + 1);
        }
        if cp >= n {
            return ld(REALLY_BIG);
        }

        let s = (t - tm(cp)) / key_len;
        if !eq_or_unordered(ld(cache.last_time), s) {
            let t2 = s * s;
            let t3 = t2 * s;
            let z = (t2 * 3.0 - t3) - t3;
            let a = t3 - t2;
            cache.hermite = [st(1.0 - z), st(z), st((a - t2) + s), st(a)];
            cache.last_time = st(s);
        }

        let adj0 = key_len / (tm(cp + 1) - tm(cp - 1));
        let adj1 = key_len / (tm(cp + 2) - tm(cp));

        let key_id = self.params.as_ptr() as usize + cp as usize * std::mem::size_of::<Tcb>();
        if cache.old_key != Some(key_id) {
            let k0 = self.params[cp as usize];
            let k1 = self.params[cp as usize + 1];
            let (t0, c0, b0) = (ld(k0.tension), ld(k0.continuity), ld(k0.bias));
            let (t1, c1, b1) = (ld(k1.tension), ld(k1.continuity), ld(k1.bias));
            cache.dd0a = st(((b0 + 1.0) * (1.0 - t0)) * (c0 + 1.0));
            cache.dd0b = st(((1.0 - b0) * (1.0 - t0)) * (1.0 - c0));
            cache.ds1a = st(((1.0 - c1) * (1.0 - t1)) * (b1 + 1.0));
            cache.ds1b = st(((1.0 - t1) * (c1 + 1.0)) * (1.0 - b1));
            cache.old_key = Some(key_id);
        }

        let [hx, hy, hz, hw] = cache.hermite.map(ld);
        let delta = p(cp + 1) - p(cp);
        let s1 = (p(cp + 2) - p(cp + 1)) * ld(cache.ds1b) + ld(cache.ds1a) * delta;
        let w_term = (s1 * adj1) * hw;
        let s0 = (p(cp) - p(cp - 1)) * ld(cache.dd0a) + ld(cache.dd0b) * delta;
        let z_term = (s0 * adj0) * hz;
        ((w_term + z_term) + hy * p(cp + 1)) + hx * p(cp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve() -> SplineCurve {
        // HW1's built-in B-Spline test data.
        let points = [0.0, 1.0, -1.0, -0.5, 0.0, -2.0, -1.2];
        let times = [0.0, 0.5, 2.0, 3.0, 4.0, 5.5, 7.0, 7.5, 9.0];
        let z = Tcb::default();
        let tcb = |t, c, b| Tcb {
            tension: t,
            continuity: c,
            bias: b,
        };
        let params = [
            z,
            tcb(0.0, -1.0, 0.0),
            tcb(0.0, 1.0, 0.0),
            tcb(-1.0, 0.0, 0.0),
            z,
            tcb(1.0, 0.0, 0.0),
            z,
        ];
        SplineCurve::start(&points, &times, &params)
    }

    #[test]
    fn interpolates_keys() {
        let mut c = curve();
        let mut cache = SplineCache::default();
        // Times are offset by one slot: segment i runs from times[i] to times[i+1].
        let v = c.update(&mut cache, 0.5);
        assert!((v - 1.0).abs() < 1e-6, "{v}");
        let v = c.update(&mut cache, 1.5);
        assert!((v + 1.0).abs() < 1e-6, "{v}");
    }

    #[test]
    fn ends_on_last_point() {
        let mut c = curve();
        let mut cache = SplineCache::default();
        let mut last = 0.0;
        for _ in 0..1000 {
            last = c.update(&mut cache, 0.02);
        }
        assert_eq!(last as f32, -1.2);
        assert_eq!(c.current_point, c.n_points - 1);
    }

    #[test]
    fn continuous_between_steps() {
        let mut c = curve();
        let mut cache = SplineCache::default();
        let mut prev = c.update(&mut cache, 0.0);
        for _ in 0..300 {
            let v = c.update(&mut cache, 0.02);
            assert!((v - prev).abs() < 0.2, "jump {prev} -> {v}");
            prev = v;
        }
    }
}
