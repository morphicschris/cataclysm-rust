//! Deterministic rendering without a sound card, plus the level/spectrum measurements the tests
//! use to check mixes.

use crate::codec::{BLOCK_FRAMES, BLOCK_SECONDS, OUTPUT_RATE};
use crate::system::{BankKind, Emitter, GameContext, ShipSounds, SoundSystem};
use crate::tables::Listener;

/// One scripted action.
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    /// A sound event (see [`crate::system::event`]).
    Event { event: u32, emitter: Option<Emitter> },
    /// A patch played directly.
    Patch { bank: BankKind, patch: i32, volume: i16, pan: i16, priority: i32 },
    /// A speech sentence on a speech stream.
    Speech { event: u32, variable: i32, variation: i32, stream: usize },
    StopSpeech { stream: usize, fade: f32 },
    Music(i32),
    StopMusic(f32),
    /// Opt-in crossfade to a track.
    Crossfade(i32, f32),
    /// Stop a handle returned by an earlier cue (by cue index).
    StopCue { cue: usize, fade: f32 },
    Listener(Listener),
    Context(GameContext),
    Volumes { sfx: f32, speech: f32, music: f32 },
    StopAll(f32),
}

/// A command at a time (seconds from the start; applied at the next block boundary).
#[derive(Debug, Clone, PartialEq)]
pub struct Cue {
    pub at: f64,
    pub command: Command,
}

impl Cue {
    pub fn new(at: f64, command: Command) -> Self {
        Self { at, command }
    }
}

/// Interleaved 16-bit stereo at 22 050 Hz, plus the same mix before quantisation.
#[derive(Debug, Clone, Default)]
pub struct Rendered {
    pub pcm: Vec<i16>,
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    /// Handle returned by each cue (`-1` for cues without one).
    pub handles: Vec<i32>,
}

impl Rendered {
    pub fn frames(&self) -> usize {
        self.left.len()
    }

    /// A RIFF/WAVE file of the 16-bit output.
    pub fn to_wav(&self) -> Vec<u8> {
        wav(&self.pcm, 2, OUTPUT_RATE)
    }

    /// Frame index of a time.
    pub fn frame_at(&self, seconds: f64) -> usize {
        ((seconds * f64::from(OUTPUT_RATE)) as usize).min(self.frames())
    }
}

/// Build a 16-bit PCM WAV file.
pub fn wav(samples: &[i16], channels: u16, rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
    out.extend_from_slice(&(channels * 2).to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Run a cue list through the sound system for `seconds` and collect the output. The game
/// update ([`SoundSystem::update`]) runs every `blocks_per_update` blocks (3 = ~29 Hz).
pub fn render(system: &mut SoundSystem, cues: &[Cue], seconds: f64, blocks_per_update: u64) -> Rendered {
    let blocks = (seconds / BLOCK_SECONDS).ceil() as u64;
    let mut order: Vec<usize> = (0..cues.len()).collect();
    order.sort_by(|&a, &b| cues[a].at.total_cmp(&cues[b].at));
    let mut next = 0;
    let mut out = Rendered { handles: vec![-1; cues.len()], ..Default::default() };
    let mut ship = ShipSounds::default();
    let mixer = system.mixer_handle();
    for b in 0..blocks {
        let now = b as f64 * BLOCK_SECONDS;
        while next < order.len() && cues[order[next]].at <= now + 1e-9 {
            let i = order[next];
            out.handles[i] = apply(system, &cues[i].command, &mut ship, &out.handles);
            next += 1;
        }
        if b % blocks_per_update.max(1) == 0 {
            system.update(true);
        }
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        let mut m = mixer.lock().unwrap();
        m.process_block_f32(&mut f);
        let limiter = m.options().limiter;
        drop(m);
        for (&l, &r) in f[0].iter().zip(&f[1]) {
            out.pcm.push(crate::codec::to_i16(l, limiter));
            out.pcm.push(crate::codec::to_i16(r, limiter));
        }
        out.left.extend_from_slice(&f[0]);
        out.right.extend_from_slice(&f[1]);
    }
    out
}

fn apply(system: &mut SoundSystem, cmd: &Command, ship: &mut ShipSounds, handles: &[i32]) -> i32 {
    match cmd {
        Command::Event { event, emitter } => {
            *ship = ShipSounds::default();
            system.play_event(emitter.as_ref(), *event, ship)
        }
        Command::Patch { bank, patch, volume, pan, priority } => system.play_patch(*bank, *patch, None, *pan, *priority, *volume, true),
        Command::Speech { event, variable, variation, stream } => {
            system.play_speech(*event, *variable, *variation, *stream).map_or(-1, |s| s.stream as i32)
        }
        Command::StopSpeech { stream, fade } => {
            system.stop_speech(*stream, *fade);
            -1
        }
        Command::Music(t) => i32::from(system.play_music(*t)) - 1,
        Command::StopMusic(f) => {
            system.stop_music(-1, *f);
            -1
        }
        Command::Crossfade(t, s) => i32::from(system.crossfade_music(*t, *s)) - 1,
        Command::StopCue { cue, fade } => {
            if let Some(&h) = handles.get(*cue) {
                system.mixer().stop(h, *fade);
            }
            -1
        }
        Command::Listener(l) => {
            system.listener = *l;
            -1
        }
        Command::Context(c) => {
            system.context = *c;
            -1
        }
        Command::Volumes { sfx, speech, music } => {
            system.set_sfx_volume(*sfx);
            system.set_speech_volume(*speech);
            system.set_music_volume(*music);
            -1
        }
        Command::StopAll(f) => {
            system.stop_all(*f, true);
            -1
        }
    }
}

/// Simple level measurements of a stretch of samples.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Levels {
    pub rms: f64,
    pub peak: f64,
}

pub fn levels(x: &[f32]) -> Levels {
    if x.is_empty() {
        return Levels { rms: 0.0, peak: 0.0 };
    }
    let e: f64 = x.iter().map(|&v| f64::from(v) * f64::from(v)).sum();
    let peak = x.iter().fold(0.0f64, |m, &v| m.max(f64::from(v).abs()));
    Levels { rms: (e / x.len() as f64).sqrt(), peak }
}

/// Root-mean-square in decibels relative to 16-bit full scale.
pub fn dbfs(rms: f64) -> f64 {
    20.0 * (rms.max(1e-9) / 32768.0).log10()
}

/// Average power spectrum over Hann-windowed frames of `n` (a power of two) samples; bin `k`
/// is `k * rate / n` Hz.
pub fn power_spectrum(x: &[f32], n: usize) -> Vec<f64> {
    assert!(n.is_power_of_two());
    let mut acc = vec![0.0f64; n / 2];
    let mut frames = 0;
    let window: Vec<f64> = (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()).collect();
    let mut start = 0;
    while start + n <= x.len() {
        let mut re: Vec<f64> = (0..n).map(|i| f64::from(x[start + i]) * window[i]).collect();
        let mut im = vec![0.0f64; n];
        fft(&mut re, &mut im);
        for k in 0..n / 2 {
            acc[k] += re[k] * re[k] + im[k] * im[k];
        }
        frames += 1;
        start += n / 2;
    }
    if frames > 0 {
        for a in &mut acc {
            *a /= frames as f64;
        }
    }
    acc
}

/// Fraction of the spectrum's energy at or above `hz`.
pub fn energy_above(spectrum: &[f64], n: usize, hz: f64) -> f64 {
    let k0 = (hz * n as f64 / f64::from(OUTPUT_RATE)).ceil() as usize;
    let total: f64 = spectrum.iter().sum();
    if total == 0.0 {
        return 0.0;
    }
    spectrum[k0.min(spectrum.len())..].iter().sum::<f64>() / total
}

/// Spectral centroid in Hz.
pub fn centroid(spectrum: &[f64], n: usize) -> f64 {
    let total: f64 = spectrum.iter().sum();
    if total == 0.0 {
        return 0.0;
    }
    spectrum.iter().enumerate().map(|(k, &p)| k as f64 * p).sum::<f64>() / total * f64::from(OUTPUT_RATE) / n as f64
}

/// Pearson correlation of two equally long signals.
pub fn correlation(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len());
    if n == 0 {
        return 0.0;
    }
    let ma = a[..n].iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let mb = b[..n].iter().map(|&v| f64::from(v)).sum::<f64>() / n as f64;
    let (mut sab, mut saa, mut sbb) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let (x, y) = (f64::from(a[i]) - ma, f64::from(b[i]) - mb);
        sab += x * y;
        saa += x * x;
        sbb += y * y;
    }
    if saa == 0.0 || sbb == 0.0 {
        0.0
    } else {
        sab / (saa * sbb).sqrt()
    }
}

fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let bits = n.trailing_zeros();
    for i in 0..n {
        let j = i.reverse_bits() >> (usize::BITS - bits);
        if j > i {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let a = -2.0 * std::f64::consts::PI / len as f64;
        for start in (0..n).step_by(len) {
            for k in 0..len / 2 {
                let (wr, wi) = ((a * k as f64).cos(), (a * k as f64).sin());
                let (p, q) = (start + k, start + k + len / 2);
                let tr = re[q] * wr - im[q] * wi;
                let ti = re[q] * wi + im[q] * wr;
                re[q] = re[p] - tr;
                im[q] = im[p] - ti;
                re[p] += tr;
                im[p] += ti;
            }
        }
        len *= 2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_finds_a_tone() {
        let n = 1024;
        let f = 1000.0;
        let x: Vec<f32> = (0..8192).map(|i| (2.0 * std::f64::consts::PI * f * i as f64 / 22050.0).sin() as f32).collect();
        let s = power_spectrum(&x, n);
        let c = centroid(&s, n);
        assert!((c - f).abs() < 30.0, "{c}");
        assert!(energy_above(&s, n, 2000.0) < 1e-3);
        assert!((levels(&x).rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 1e-3);
    }
}
