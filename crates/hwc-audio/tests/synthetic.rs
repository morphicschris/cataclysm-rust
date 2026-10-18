//! Mixer checks against the reference decoder on synthetic codec data (no game files needed).

use std::sync::Arc;

use hwc_audio::codec::{Window, BLOCK_FRAMES};
use hwc_audio::mixer::{Mixer, MixerOptions, PatchRef, StreamData, StreamEntry};
use hwc_formats::audio::{decode_fquant_with, OutputRate, SynthesisWindow};

struct BitWriter {
    bytes: Vec<u8>,
    pos: usize,
}

impl BitWriter {
    fn new(len: usize) -> Self {
        Self { bytes: vec![0; len], pos: 0 }
    }
    fn put(&mut self, v: u32, n: u32) {
        for i in 0..n {
            if (v >> i) & 1 != 0 {
                self.bytes[(self.pos + i as usize) / 8] |= 1 << ((self.pos + i as usize) % 8);
            }
        }
        self.pos += n as usize;
    }
}

struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) % u64::from(n)) as u32
    }
}

const BAND_EDGES: [usize; 27] =
    [0, 1, 2, 3, 4, 5, 6, 7, 9, 11, 13, 15, 17, 20, 23, 27, 31, 37, 43, 51, 62, 74, 89, 110, 139, 180, 256];

/// Encode a block with random exponents and coefficients within `max_bin`, fitting 512 bits.
fn random_block(rng: &mut Lcg, first: bool, max_bin: usize, exps: &mut [u8; 256]) -> Vec<u8> {
    let bytes = 64;
    let mut w = BitWriter::new(bytes);
    let copy = rng.next(4) == 0;
    let flags = u32::from(first) | if copy { 2 } else { 0 };
    let (b, e, s) = (3u32, 2u32, 4u32);
    w.put(flags, 2);
    w.put(b, 3);
    w.put(e, 2);
    w.put(s, 4);
    if first {
        exps.fill(0);
    }
    // Exponent updates for a few bands.
    let mut band = 0usize;
    let mut count = 0;
    loop {
        let step = if count == 0 { rng.next(3) as usize } else { 1 + rng.next(4) as usize };
        if band + step > 25 || count > 6 {
            w.put(0, b);
            break;
        }
        band += step;
        let ex = rng.next(4) as u8;
        w.put(step as u32, b);
        w.put(u32::from(ex), e);
        exps[BAND_EDGES[band]..BAND_EDGES[band + 1]].fill(ex);
        count += 1;
    }
    let frames = if copy { 1 } else { 2 };
    for f in 0..frames {
        let budget = if f == 0 { 230 } else { 490 };
        let mut bin = 0usize;
        let mut k = 0;
        loop {
            let step = if k == 0 { rng.next(4) as usize } else { 1 + rng.next(12) as usize };
            let ex = exps[(bin + step).min(255)];
            let width = u32::from(ex) + 2;
            if bin + step >= max_bin || w.pos + (s + width + s) as usize > budget {
                w.put(0, s);
                break;
            }
            bin += step;
            let mag = rng.next(1 << (width - 1));
            let sign = rng.next(2);
            w.put(step as u32, s);
            w.put(mag | (sign << (width - 1)), width);
            k += 1;
        }
    }
    w.bytes
}

fn patch(data: Vec<u8>, rate: u32) -> PatchRef {
    let len = data.len();
    PatchRef {
        data: data.into(),
        start: 0,
        end: len,
        loop_start: 0,
        loop_end: len,
        bitrate: 512,
        flags: 0,
        sample_rate: rate,
        volume: 255,
        pan: 0,
        priority: 50,
    }
}

fn synthetic_payload(seed: u64, blocks: usize, max_bin: usize) -> Vec<u8> {
    let mut rng = Lcg(seed);
    let mut exps = [0u8; 256];
    (0..blocks).flat_map(|i| random_block(&mut rng, i == 0, max_bin, &mut exps)).collect()
}

fn mix_patch(p: &PatchRef, blocks: usize, opts: MixerOptions) -> (Vec<f32>, Vec<f32>) {
    let mut m = Mixer::new(opts);
    let h = m.play(p, None, -1.0, 0, 50, 255, false, true, false);
    assert!(h >= 0);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for _ in 0..blocks + 2 {
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        m.process_block_f32(&mut f);
        l.extend_from_slice(&f[0]);
        r.extend_from_slice(&f[1]);
    }
    (l, r)
}

#[test]
fn full_volume_voice_equals_reference_decoder() {
    let blocks = 60;
    let data = synthetic_payload(7, blocks, 128);
    let reference = decode_fquant_with(&data, 512, 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let p = patch(data, 44_100);
    let (l, r) = mix_patch(&p, blocks, MixerOptions::default());
    let peak = reference.samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 100.0, "synthetic data too quiet: {peak}");
    let mut worst = 0.0f32;
    for i in 0..reference.samples.len() {
        worst = worst.max((l[i] - reference.samples[i]).abs()).max((r[i] - reference.samples[i]).abs());
    }
    assert!(worst < 1e-3 * peak, "worst difference {worst} (peak {peak})");
    // After the last block the voice is released and the overlap tail dies away.
    let tail = &l[(blocks + 1) * BLOCK_FRAMES..];
    assert!(tail.iter().all(|&v| v == 0.0));
}

#[test]
fn symmetric_window_option_matches_reference() {
    let blocks = 20;
    let data = synthetic_payload(11, blocks, 128);
    let reference = decode_fquant_with(&data, 512, 1, OutputRate::Hz22050, SynthesisWindow::Symmetric).unwrap();
    let p = patch(data, 44_100);
    let opts = MixerOptions { window: Window::Symmetric, ..Default::default() };
    let (l, _) = mix_patch(&p, blocks, opts);
    let peak = reference.samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let worst = reference.samples.iter().zip(&l).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    assert!(worst < 1e-3 * peak, "{worst} vs {peak}");
}

#[test]
fn only_sub_22khz_patches_are_band_limited_to_64_bins() {
    // The game decodes 128 bins for sources above 22 049 Hz (all shipped patches) and 64 below.
    let blocks = 40;
    let data = synthetic_payload(3, blocks, 128);
    let p = patch(data.clone(), 11_025);
    let (l, _) = mix_patch(&p, blocks, MixerOptions::default());
    let full = patch(data, 22_050);
    let (lf, _) = mix_patch(&full, blocks, MixerOptions::default());
    let n = 512;
    let s = hwc_audio::offline::power_spectrum(&l, n);
    let sf = hwc_audio::offline::power_spectrum(&lf, n);
    // 64 bins of 86 Hz = 5.5 kHz; leave a little room for window leakage.
    let above = hwc_audio::offline::energy_above(&s, n, 6000.0);
    let above_full = hwc_audio::offline::energy_above(&sf, n, 6000.0);
    assert!(above < 1e-3, "{above}");
    assert!(above_full > 0.05, "{above_full}");
}

#[test]
fn volume_and_pan_scale_the_reference() {
    let blocks = 30;
    let data = synthetic_payload(5, blocks, 128);
    let reference = decode_fquant_with(&data, 512, 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let mut m = Mixer::default();
    let p = patch(data, 44_100);
    // Volume 128, panned fully left: L = 128/255, R = 0 (once the 20-block pan sweep is over).
    let h = m.play(&p, None, -1.0, -90, 50, 128, false, true, false);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for _ in 0..blocks {
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        m.process_block_f32(&mut f);
        l.extend_from_slice(&f[0]);
        r.extend_from_slice(&f[1]);
    }
    assert!(m.voice_state(h).is_some());
    let g = 128.0 / 255.0;
    // Blocks 22.. are past the sweep and its overlap.
    let from = 22 * BLOCK_FRAMES;
    let to = (blocks - 1) * BLOCK_FRAMES;
    let peak = reference.samples[from..to].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for i in from..to {
        assert!((l[i] - g * reference.samples[i]).abs() < 1e-3 * peak, "L {i}");
        assert!(r[i].abs() < 1e-3 * peak, "R {i}");
    }
}

#[test]
fn stereo_stream_entries_are_gapless_and_loop() {
    // Two mono "channels" interleaved block by block; the stream must reproduce both
    // decodes, and looping must restart without a gap.
    let blocks = 20;
    let left = synthetic_payload(21, blocks, 128);
    let right = synthetic_payload(22, blocks, 128);
    let mut inter = Vec::new();
    for i in 0..blocks {
        inter.extend_from_slice(&left[i * 64..(i + 1) * 64]);
        inter.extend_from_slice(&right[i * 64..(i + 1) * 64]);
    }
    let ref_l = decode_fquant_with(&left, 512, 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let ref_r = decode_fquant_with(&right, 512, 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let bytes: Arc<[u8]> = inter.into();
    let mut m = Mixer::default();
    let h = m.stream_handle(3);
    let mut e = StreamEntry::new(StreamData { bytes: bytes.clone(), start: 0, len: bytes.len() }, 255, 0, 2, 512);
    e.looping = true;
    m.stream_queue(h, e, false);
    let (mut l, mut r) = (Vec::new(), Vec::new());
    for _ in 0..blocks + 5 {
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        m.process_block_f32(&mut f);
        l.extend_from_slice(&f[0]);
        r.extend_from_slice(&f[1]);
    }
    let peak = ref_l.samples.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for i in 0..blocks * BLOCK_FRAMES {
        assert!((l[i] - ref_l.samples[i]).abs() < 1e-3 * peak, "L {i}");
        assert!((r[i] - ref_r.samples[i]).abs() < 1e-3 * peak, "R {i}");
    }
    // The loop restarts on the first block (which resets exponents), so block `blocks + 1`
    // equals block 1 of the reference.
    for i in 0..BLOCK_FRAMES {
        let a = l[(blocks + 1) * BLOCK_FRAMES + i];
        let b = ref_l.samples[BLOCK_FRAMES + i];
        assert!((a - b).abs() < 1e-3 * peak, "loop {i}: {a} vs {b}");
    }
    assert!(!m.stream_over(h));
}

