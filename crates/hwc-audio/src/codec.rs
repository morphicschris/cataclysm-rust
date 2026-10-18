//! Spectral building blocks of the game's mixer.
//!
//! The original mixer never converts individual sounds to PCM. Every voice is dequantised into two
//! MDCT frames of coefficients per block, scaled, equalised and summed in the frequency domain, and
//! only the final left/right sums go through the inverse transform. Because the transform is
//! linear this is equivalent to decoding each voice separately, but per-band EQ and the crude
//! "pitch shift" (resampling of the coefficient array) only make sense in this domain, so the
//! recreation keeps the same structure.
//!
//! Format details of the bitstream are documented in `docs/formats/audio.md`.

use std::f64::consts::PI;

/// Size of one coefficient array (the widest frame the codec can describe).
pub const BINS: usize = 256;
/// Output frames produced per mixer block.
pub const BLOCK_FRAMES: usize = 256;
/// The mixer's output rate in Hz.
pub const OUTPUT_RATE: u32 = 22_050;
/// Duration of one mixer block in seconds.
pub const BLOCK_SECONDS: f64 = BLOCK_FRAMES as f64 / OUTPUT_RATE as f64;
/// Mixer blocks per second of fade time, as the game rounds it (1000 / 11.61 ms).
pub const BLOCKS_PER_SECOND: f32 = 86.132_81; // 0x42AC4400, exactly 86.1328125
/// Number of equaliser bands.
pub const EQ_BANDS: usize = 9;
/// First bin of each equaliser band (octaves of the 86 Hz bin grid) plus the end.
pub const EQ_EDGES: [usize; EQ_BANDS + 1] = [0, 1, 2, 4, 8, 16, 32, 64, 128, 256];
/// Largest block the dequantiser looks at, in bytes.
pub const MAX_BLOCK_BYTES: usize = 260;

/// Critical bands addressed by the exponent updates.
const BAND_EDGES: [usize; 27] = [
    0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 15, 17, 20, 23, 27, 31, 37, 43, 51, 62, 74, 89, 110, 139, 180, 256,
];
/// Exponents at or above this carry no coefficient.
const EXPONENT_LIMIT: u8 = 6;

/// An equaliser setting: one gain per band.
pub type Eq = [f32; EQ_BANDS];
/// The neutral equaliser.
pub const FLAT_EQ: Eq = [1.0; EQ_BANDS];

/// Quantiser step for each exponent, computed in single precision the way the game builds its
/// table: `10^(e+1) / (2^(e+1) - 1)`.
fn step_table() -> [f32; EXPONENT_LIMIT as usize] {
    let mut t = [0.0f32; EXPONENT_LIMIT as usize];
    t[0] = 10.0;
    for i in 1..t.len() {
        t[i] = t[i - 1] * 10.0;
    }
    for (i, v) in t.iter_mut().enumerate() {
        *v *= 1.0 / ((1u32 << (i + 1)) - 1) as f32;
    }
    t
}

/// The two coefficient frames of one block.
#[derive(Clone)]
pub struct Frames {
    pub primary: [f32; BINS],
    pub secondary: [f32; BINS],
}

impl Default for Frames {
    fn default() -> Self {
        Self { primary: [0.0; BINS], secondary: [0.0; BINS] }
    }
}

/// Little-endian bit reader over one zero-padded block.
struct Bits {
    buf: [u8; MAX_BLOCK_BYTES + 8],
}

impl Bits {
    fn new(block: &[u8], bitrate_bits: u32) -> Self {
        let mut buf = [0u8; MAX_BLOCK_BYTES + 8];
        let n = block.len().min(bitrate_bits as usize / 8).min(MAX_BLOCK_BYTES);
        buf[..n].copy_from_slice(&block[..n]);
        Self { buf }
    }

    fn get(&self, pos: usize, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let b = pos >> 3;
        let word = u32::from_le_bytes([self.buf[b], self.buf[b + 1], self.buf[b + 2], self.buf[b + 3]]);
        (word >> (pos & 7)) & ((1u32 << n) - 1)
    }

    fn get_signed(&self, pos: usize, n: u32) -> i32 {
        let v = self.get(pos, n);
        let mag = (v & ((1u32 << (n - 1)) - 1)) as i32;
        if v >> (n - 1) != 0 {
            -mag
        } else {
            mag
        }
    }
}

/// Dequantiser with its precomputed step table.
#[derive(Clone)]
pub struct Dequantizer {
    steps: [f32; EXPONENT_LIMIT as usize],
}

impl Default for Dequantizer {
    fn default() -> Self {
        Self { steps: step_table() }
    }
}

impl Dequantizer {
    /// Unpack one block into `out`. `size` limits which bins may be written (the voice's
    /// bandwidth); `exponents` is the channel's persistent exponent state.
    ///
    /// The control flow follows the game's dequantiser exactly, including how malformed streams
    /// are cut short, so the exponent state evolves identically.
    pub fn block(&self, block: &[u8], bitrate_bits: u32, size: usize, exponents: &mut [u8; BINS], out: &mut Frames) {
        out.primary.fill(0.0);
        out.secondary.fill(0.0);
        let bits = Bits::new(block, bitrate_bits);
        let limit = (bitrate_bits as usize).min(MAX_BLOCK_BYTES * 8);
        let header = bits.get(0, 11);
        let flags = header & 3;
        let band_bits = (header >> 2) & 7;
        let exp_bits = (header >> 5) & 3;
        let bin_bits = (header >> 7) & 0xf;
        let mut pos = 11usize;

        if flags & 1 != 0 {
            exponents.fill(0);
        }

        if band_bits != 0 && exp_bits != 0 {
            let mut band = 0u8;
            let mut count = 0;
            while pos < limit {
                let step = bits.get(pos, band_bits) as u8;
                pos += band_bits as usize;
                if count != 0 && step == 0 {
                    break;
                }
                band = band.wrapping_add(step);
                if band > 25 {
                    break;
                }
                let e = bits.get(pos, exp_bits) as u8;
                pos += exp_bits as usize;
                let b = usize::from(band);
                exponents[BAND_EDGES[b]..BAND_EDGES[b + 1]].fill(e);
                count += 1;
                if count > 25 {
                    break;
                }
            }
        }

        if size == 0 || bin_bits == 0 {
            return;
        }
        pos = self.frame(&bits, pos, limit, bin_bits, size, exponents, &mut out.primary);
        if flags & 2 != 0 {
            out.secondary = out.primary;
            return;
        }
        self.frame(&bits, pos, limit, bin_bits, size, exponents, &mut out.secondary);
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        &self,
        bits: &Bits,
        mut pos: usize,
        limit: usize,
        bin_bits: u32,
        size: usize,
        exponents: &[u8; BINS],
        dest: &mut [f32; BINS],
    ) -> usize {
        let mut bin = 0u8;
        for k in 0..BINS {
            if pos >= limit {
                break;
            }
            let step = bits.get(pos, bin_bits) as u8;
            pos += bin_bits as usize;
            if k != 0 && step == 0 {
                break;
            }
            bin = bin.wrapping_add(step);
            let e = exponents[usize::from(bin)];
            let width = u32::from(e) + 2;
            let q = bits.get_signed(pos, width);
            pos += width as usize;
            if q != 0 && usize::from(bin) < size && e < EXPONENT_LIMIT {
                if bin > 127 {
                    break;
                }
                dest[usize::from(bin)] = q as f32 * self.steps[usize::from(e)];
            }
        }
        pos
    }
}

/// Add `src * level` into `dst` over the first `size` bins (a level of exactly zero is skipped,
/// as in the game).
pub fn mix(dst: &mut [f32; BINS], src: &[f32; BINS], level: f32, size: usize) {
    if level == 0.0 {
        return;
    }
    for (d, s) in dst[..size].iter_mut().zip(&src[..size]) {
        *d += level * s;
    }
}

/// Scale each equaliser band; a zero gain clears the band. Bands starting at or past `size` are
/// left alone.
pub fn equalize(c: &mut [f32; BINS], eq: &Eq, size: usize) {
    for (b, &gain) in eq.iter().enumerate() {
        let start = EQ_EDGES[b];
        if size == 0 || start > size - 1 {
            return;
        }
        let end = EQ_EDGES[b + 1].min(size);
        if gain == 0.0 {
            c[start..end].fill(0.0);
        } else if gain != 1.0 {
            for v in &mut c[start..end] {
                *v *= gain;
            }
        }
    }
}

/// The game's pitch shift: resample the coefficient array by linear interpolation, so bin `i`
/// takes the value found at `i * shift`. Bins whose source lies past `size` become silent.
pub fn pitch_shift(c: &mut [f32; BINS], shift: f32, size: usize) {
    if shift == 1.0 || size < 2 {
        return;
    }
    let mut tmp = [0.0f32; BINS];
    for (i, t) in tmp[..size].iter_mut().enumerate() {
        let src = i as f64 * f64::from(shift);
        if src < size as f64 {
            let j = (src as usize).min(size - 2);
            *t = ((src - j as f64) * f64::from(c[j + 1] - c[j]) + f64::from(c[j])) as f32;
        }
    }
    c[..size].copy_from_slice(&tmp[..size]);
}

/// Inverse MDCT of `n / 2` coefficients to `n` samples, through an `n / 4`-point complex FFT,
/// scaled like the game's (`8 / sqrt(n)` after a half-scale pre-rotation).
#[derive(Clone)]
struct Imdct {
    n: usize,
    rot: Vec<(f64, f64)>,
    fft_tw: Vec<(f64, f64)>,
    rev: Vec<usize>,
    scale: f64,
}

impl Imdct {
    fn new(n: usize) -> Self {
        let q = n / 4;
        let rot = (0..q)
            .map(|k| {
                let a = 2.0 * PI * (k as f64 + 0.125) / n as f64;
                (a.cos(), a.sin())
            })
            .collect();
        let fft_tw = (0..q / 2)
            .map(|k| {
                let a = -2.0 * PI * k as f64 / q as f64;
                (a.cos(), a.sin())
            })
            .collect();
        let shift = usize::BITS - q.trailing_zeros();
        let rev = (0..q).map(|i| i.reverse_bits() >> shift).collect();
        Self { n, rot, fft_tw, rev, scale: 8.0 / (n as f64).sqrt() }
    }

    fn fft(&self, re: &mut [f64], im: &mut [f64]) {
        let q = re.len();
        for i in 0..q {
            let j = self.rev[i];
            if j > i {
                re.swap(i, j);
                im.swap(i, j);
            }
        }
        let mut len = 2;
        while len <= q {
            let half = len / 2;
            let stride = q / len;
            for start in (0..q).step_by(len) {
                for k in 0..half {
                    let (wr, wi) = self.fft_tw[k * stride];
                    let (a, b) = (start + k, start + k + half);
                    let tr = re[b] * wr - im[b] * wi;
                    let ti = re[b] * wi + im[b] * wr;
                    re[b] = re[a] - tr;
                    im[b] = im[a] - ti;
                    re[a] += tr;
                    im[a] += ti;
                }
            }
            len *= 2;
        }
    }

    fn run(&self, spec: &[f32], out: &mut [f32]) {
        let n = self.n;
        let (h, q) = (n / 2, n / 4);
        let mut re = [0.0f64; BINS / 2];
        let mut im = [0.0f64; BINS / 2];
        for (k, &(c, s)) in self.rot.iter().enumerate() {
            let a = 0.5 * f64::from(spec[2 * k]);
            let b = 0.5 * f64::from(spec[h - 1 - 2 * k]);
            re[k] = a * c + b * s;
            im[k] = b * c - a * s;
        }
        self.fft(&mut re[..q], &mut im[..q]);
        let mut half = [0.0f64; 2 * BINS];
        for (k, &(c, s)) in self.rot.iter().enumerate() {
            half[2 * k] = (re[k] * c + im[k] * s) * self.scale;
            half[h + 2 * k] = (im[k] * c - re[k] * s) * self.scale;
        }
        for i in (1..n).step_by(2) {
            half[i] = -half[n - 1 - i];
        }
        for (i, o) in out[..n].iter_mut().enumerate() {
            *o = if i < 3 * q { half[q + i] } else { -half[i - 3 * q] } as f32;
        }
    }
}

/// Synthesis window shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Window {
    /// `sin(pi i / n)`: what the game uses.
    #[default]
    Game,
    /// `sin(pi (i + 1/2) / n)`: cancels time-domain aliasing exactly (opt-in improvement).
    Symmetric,
}

/// Transform size used by the master synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Quality {
    /// 256-point transform, 128 bins (11 kHz bandwidth): the game's normal mode.
    #[default]
    Normal,
    /// 128-point transform, 64 bins, each output sample doubled: the game's low-CPU mode.
    Low,
}

impl Quality {
    /// Number of coefficients processed per frame (the game's "fqSize").
    pub fn bins(self) -> usize {
        match self {
            Quality::Normal => 128,
            Quality::Low => 64,
        }
    }
}

/// Overlap-add synthesis of one output channel.
#[derive(Clone)]
pub struct Synth {
    normal: Imdct,
    low: Imdct,
    window_normal: Vec<f32>,
    window_low: Vec<f32>,
    carry: [f32; 2 * BINS],
}

impl Synth {
    pub fn new(window: Window) -> Self {
        let make = |n: usize| -> Vec<f32> {
            let phase = match window {
                Window::Game => 0.0,
                Window::Symmetric => 0.5,
            };
            (0..n).map(|i| (PI * (i as f64 + phase) / n as f64).sin() as f32).collect()
        };
        Self {
            normal: Imdct::new(256),
            low: Imdct::new(128),
            window_normal: make(256),
            window_low: make(128),
            carry: [0.0; 2 * BINS],
        }
    }

    /// Turn one block's two frames into `BLOCK_FRAMES` samples.
    pub fn block(&mut self, primary: &[f32; BINS], secondary: &[f32; BINS], quality: Quality, out: &mut [f32; BLOCK_FRAMES]) {
        match quality {
            Quality::Normal => {
                let mut tmp = [0.0f32; 2 * BINS];
                Self::overlap(&self.normal, &self.window_normal, &mut self.carry, primary, secondary, &mut tmp);
                out.copy_from_slice(&tmp[..BLOCK_FRAMES]);
            }
            Quality::Low => {
                let mut tmp = [0.0f32; 2 * BINS];
                Self::overlap(&self.low, &self.window_low, &mut self.carry, primary, secondary, &mut tmp);
                for (i, o) in out.iter_mut().enumerate() {
                    *o = tmp[i / 2];
                }
            }
        }
    }

    fn overlap(
        imdct: &Imdct,
        w: &[f32],
        carry: &mut [f32; 2 * BINS],
        primary: &[f32; BINS],
        secondary: &[f32; BINS],
        out: &mut [f32; 2 * BINS],
    ) {
        let n = imdct.n;
        let h = n / 2;
        out.copy_from_slice(carry);
        let mut first = [0.0f32; 2 * BINS];
        imdct.run(primary, &mut first);
        imdct.run(secondary, &mut carry[..]);
        for i in 0..h {
            out[h + i] = out[h + i] * w[h + i] + first[i] * w[i];
            carry[i] = carry[i] * w[i] + first[h + i] * w[h + i];
        }
    }
}

/// How mixed samples are converted to 16 bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Limiter {
    /// Round to nearest (ties to even) and clamp: what the game's output stage does.
    #[default]
    Hard,
    /// Logarithmic soft knee above half scale before clamping (opt-in).
    Soft,
}

/// Convert one mixed sample to 16 bits.
pub fn to_i16(x: f32, limiter: Limiter) -> i16 {
    let x = match limiter {
        Limiter::Hard => x,
        Limiter::Soft => hwc_formats::audio::soft_limit(x),
    };
    (f64::from(x).round_ties_even()).clamp(-32768.0, 32767.0) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imdct_matches_direct_formula() {
        for n in [128usize, 256] {
            let h = n / 2;
            let imdct = Imdct::new(n);
            let spec: Vec<f32> = (0..BINS).map(|k| if k < h { ((k * 7 % 11) as f32 - 5.0) * 3.0 } else { 0.0 }).collect();
            let mut fast = vec![0.0f32; n];
            imdct.run(&spec, &mut fast);
            // Direct definition with the game's scale: 0.5 * 8/sqrt(n) * sum X[k] cos(...).
            for (i, &f) in fast.iter().enumerate() {
                let d: f64 = (0..h)
                    .map(|k| {
                        let ph = PI / h as f64 * (i as f64 + 0.5 + h as f64 / 2.0) * (k as f64 + 0.5);
                        f64::from(spec[k]) * ph.cos()
                    })
                    .sum::<f64>()
                    * 0.5
                    * 8.0
                    / (n as f64).sqrt();
                assert!((f64::from(f) - d).abs() < 1e-3 * d.abs().max(1.0), "n {n} sample {i}: {f} vs {d}");
            }
        }
    }

    #[test]
    fn equalizer_bands_and_limits() {
        let mut c = [1.0f32; BINS];
        let mut eq = FLAT_EQ;
        eq[0] = 0.0;
        eq[3] = 0.5;
        eq[8] = 0.0; // starts at bin 128: outside a 128-bin voice
        equalize(&mut c, &eq, 128);
        assert_eq!(c[0], 0.0);
        assert_eq!(c[1], 1.0);
        assert!(c[4..8].iter().all(|&v| v == 0.5));
        assert_eq!(c[128], 1.0);
    }

    #[test]
    fn pitch_shift_interpolates() {
        let mut c = [0.0f32; BINS];
        for (i, v) in c.iter_mut().enumerate().take(128) {
            *v = i as f32;
        }
        pitch_shift(&mut c, 0.5, 128);
        assert_eq!(c[3], 1.5);
        pitch_shift(&mut c, 2.0, 128);
        assert_eq!(c[64], 0.0); // source 128 is past the end
    }

    #[test]
    fn output_rounds_half_even_and_clamps() {
        assert_eq!(to_i16(2.5, Limiter::Hard), 2);
        assert_eq!(to_i16(-3.5, Limiter::Hard), -4);
        assert_eq!(to_i16(40_000.0, Limiter::Hard), 32767);
        assert_eq!(to_i16(-40_000.0, Limiter::Hard), -32768);
        assert!(to_i16(30_000.0, Limiter::Soft) < 30_000);
    }
}
