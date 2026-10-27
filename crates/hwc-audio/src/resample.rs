//! Converting the 22 050 Hz mix to the output device's rate.
//!
//! The mix only carries content up to 11 025 Hz. Played at 44.1 or 48 kHz, every frequency `f`
//! in it has a mirror image at `22 050 - f` (and further up) that the converter must remove.
//! Linear interpolation only damps those images by 10 to 20 dB, which is heard as a faint hiss
//! over bright material such as the music; the windowed-sinc converter takes them below 80 dB,
//! as a media player or the Windows mixer does when it plays the same 22 050 Hz audio.

/// How the 22 050 Hz mix is converted to the device rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resampling {
    /// Kaiser-windowed sinc: images and aliases removed (the default).
    #[default]
    Sinc,
    /// Linear interpolation (comparable to the old Windows kernel mixer; leaves audible images).
    Linear,
    /// Catmull-Rom cubic interpolation.
    Cubic,
}

/// Taps on each side of the read position.
const HALF: usize = 32;
const TAPS: usize = 2 * HALF;
/// Filter phases per input frame (the coefficients are interpolated between them).
const PHASES: usize = 512;
/// Kaiser window shape (about 85 dB stop-band attenuation).
const BETA: f64 = 8.5;
/// Cut-off (the -6 dB point) as a fraction of the lower of the two rates.
const CUTOFF: f64 = 0.475;

/// A stereo sample-rate converter fed with frames at `in_rate`, read at `out_rate`.
pub struct Resampler {
    kind: Resampling,
    step: f64,
    src: Vec<[f32; 2]>,
    pos: f64,
    /// `(PHASES + 1) * TAPS` coefficients; row `p` is the filter for a read position `p /
    /// PHASES` past a frame, tap `k` weighting the frame `k - HALF + 1` from it.
    table: Vec<f32>,
}

impl Resampler {
    pub fn new(kind: Resampling, in_rate: u32, out_rate: u32) -> Self {
        let step = f64::from(in_rate) / f64::from(out_rate);
        // At equal rates every method reads the frames as they are.
        let kind = if in_rate == out_rate { Resampling::Linear } else { kind };
        let table = if kind == Resampling::Sinc { sinc_table(CUTOFF * (1.0f64 / step).min(1.0)) } else { Vec::new() };
        // Silent history so the first frames have their full filter support.
        Self { kind, step, src: vec![[0.0; 2]; HALF], pos: HALF as f64, table }
    }

    /// Whether [`Resampler::next_frame`] needs more input first.
    pub fn needs_input(&self) -> bool {
        self.pos as usize + HALF + 1 >= self.src.len()
    }

    pub fn push(&mut self, frames: impl IntoIterator<Item = [f32; 2]>) {
        self.src.extend(frames);
    }

    /// The next output frame. Call only while [`Resampler::needs_input`] is false.
    pub fn next_frame(&mut self) -> [f32; 2] {
        let i = self.pos as usize;
        let t = self.pos - i as f64;
        let out = match self.kind {
            Resampling::Linear => {
                let t = t as f32;
                let (a, b) = (self.src[i], self.src[i + 1]);
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            }
            Resampling::Cubic => {
                let t = t as f32;
                let (p0, p1, p2, p3) = (self.src[i - 1], self.src[i], self.src[i + 1], self.src[i + 2]);
                let c = |k: usize| {
                    let (a, b, c, d) = (p0[k], p1[k], p2[k], p3[k]);
                    b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
                };
                [c(0), c(1)]
            }
            Resampling::Sinc => {
                let x = t * PHASES as f64;
                let p = x as usize;
                let u = (x - p as f64) as f32;
                let (r0, r1) = (&self.table[p * TAPS..][..TAPS], &self.table[(p + 1) * TAPS..][..TAPS]);
                let frames = &self.src[i + 1 - HALF..i + 1 + HALF];
                let (mut l, mut r) = (0.0f32, 0.0f32);
                for k in 0..TAPS {
                    let w = r0[k] + (r1[k] - r0[k]) * u;
                    l += w * frames[k][0];
                    r += w * frames[k][1];
                }
                [l, r]
            }
        };
        self.pos += self.step;
        // Drop consumed history, keeping the filter's support before the read position.
        let consumed = (self.pos as usize).saturating_sub(HALF);
        if consumed > 4096 {
            self.src.drain(..consumed);
            self.pos -= consumed as f64;
        }
        // Interpolation can overshoot between samples near full scale; keep it in range.
        [out[0].clamp(-1.0, 1.0), out[1].clamp(-1.0, 1.0)]
    }
}

/// Windowed-sinc low-pass with its -6 dB point at `cutoff` (a fraction of the input rate),
/// tabulated for `PHASES + 1` fractional positions; each row sums to 1.
fn sinc_table(cutoff: f64) -> Vec<f32> {
    let mut table = vec![0.0f32; (PHASES + 1) * TAPS];
    let i0_beta = bessel_i0(BETA);
    for p in 0..=PHASES {
        let t = p as f64 / PHASES as f64;
        let mut row = [0.0f64; TAPS];
        for (k, w) in row.iter_mut().enumerate() {
            // Distance from the read position to the frame this tap weights.
            let d = k as f64 - (HALF - 1) as f64 - t;
            let x = 2.0 * cutoff * d;
            let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
            let r = d / HALF as f64;
            let window = if r.abs() >= 1.0 { 0.0 } else { bessel_i0(BETA * (1.0 - r * r).sqrt()) / i0_beta };
            *w = sinc * window;
        }
        let sum: f64 = row.iter().sum();
        for (k, w) in row.iter().enumerate() {
            table[p * TAPS + k] = (w / sum) as f32;
        }
    }
    table
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term) = (1.0, 1.0);
    let q = x * x / 4.0;
    for k in 1..64 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    fn convert(kind: Resampling, input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
        let mut r = Resampler::new(kind, in_rate, out_rate);
        let mut fed = 0;
        let mut out = Vec::new();
        while fed < input.len() || !r.needs_input() {
            if r.needs_input() {
                let end = (fed + 256).min(input.len());
                r.push(input[fed..end].iter().map(|&s| [s, s]));
                fed = end;
                continue;
            }
            out.push(r.next_frame()[0]);
        }
        out
    }

    /// Amplitude of the `hz` component (a single-bin DFT over a Hann window).
    fn amplitude(x: &[f32], rate: f64, hz: f64) -> f64 {
        let n = x.len();
        let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
        for (i, &s) in x.iter().enumerate() {
            let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / n as f64).cos();
            let a = 2.0 * PI * hz * i as f64 / rate;
            re += f64::from(s) * w * a.cos();
            im += f64::from(s) * w * a.sin();
            wsum += w;
        }
        2.0 * (re * re + im * im).sqrt() / wsum
    }

    fn image_db(kind: Resampling, tone: f64) -> (f64, f64) {
        let input: Vec<f32> = (0..22_050).map(|i| (0.5 * (2.0 * PI * tone * i as f64 / 22_050.0).sin()) as f32).collect();
        let out = convert(kind, &input, 22_050, 48_000);
        let body = &out[4800..out.len() - 4800];
        let signal = amplitude(body, 48_000.0, tone);
        let image = amplitude(body, 48_000.0, 22_050.0 - tone);
        (20.0 * (signal / 0.5).log10(), 20.0 * (image / signal).log10())
    }

    #[test]
    fn sinc_removes_the_images_linear_leaves() {
        for tone in [1000.0, 5000.0, 8000.0, 9500.0] {
            let (_, linear) = image_db(Resampling::Linear, tone);
            let (gain, sinc) = image_db(Resampling::Sinc, tone);
            eprintln!("{tone} Hz: image linear {linear:.1} dB, sinc {sinc:.1} dB, sinc passband {gain:.2} dB");
            assert!(gain.abs() < 0.1, "{tone} Hz passband {gain} dB");
            assert!(sinc < -75.0, "{tone} Hz image {sinc} dB");
        }
        // The case behind the hiss: a bright partial mirrored into the top octave.
        assert!(image_db(Resampling::Linear, 8000.0).1 > -20.0);
    }

    #[test]
    fn equal_rates_pass_frames_through() {
        let input: Vec<f32> = (0..2000).map(|i| ((i * 7919) % 200) as f32 / 200.0 - 0.5).collect();
        for kind in [Resampling::Sinc, Resampling::Linear, Resampling::Cubic] {
            let out = convert(kind, &input, 22_050, 22_050);
            assert_eq!(&out[..], &input[..input.len() - HALF - 1]);
        }
    }

    #[test]
    fn downsampling_keeps_the_rate_and_level() {
        let input: Vec<f32> = (0..22_050).map(|i| (0.5 * (2.0 * PI * 1000.0 * i as f64 / 22_050.0).sin()) as f32).collect();
        let out = convert(Resampling::Sinc, &input, 22_050, 16_000);
        assert!((out.len() as i64 - 16_000).abs() < 40, "{}", out.len());
        let a = amplitude(&out[1600..out.len() - 1600], 16_000.0, 1000.0);
        assert!((a - 0.5).abs() < 0.01, "{a}");
    }
}
