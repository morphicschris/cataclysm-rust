//! Real-time output through cpal.
//!
//! The mixer produces 22 050 Hz stereo, as the game's DirectSound buffer did. If the device can
//! open a stream at that rate it is used directly; otherwise the output is resampled to the
//! device rate (the operating system did that for the original).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::codec::{BLOCK_FRAMES, OUTPUT_RATE};
use crate::mixer::Mixer;

/// How the 22 050 Hz mix is converted to the device rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Resampling {
    /// Linear interpolation (comparable to the old Windows kernel mixer).
    #[default]
    Linear,
    /// Catmull-Rom cubic interpolation (opt-in, less aliasing).
    Cubic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputOptions {
    pub resampling: Resampling,
    /// Try to open the device at 22 050 Hz before falling back to its default rate.
    pub prefer_native_rate: bool,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self { resampling: Resampling::Linear, prefer_native_rate: true }
    }
}

#[derive(Debug)]
pub struct OutputError(pub String);

impl std::fmt::Display for OutputError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "audio output: {}", self.0)
    }
}

impl std::error::Error for OutputError {}

/// Pulls blocks from the mixer and converts them to the device rate.
struct Feeder {
    mixer: Arc<Mutex<Mixer>>,
    paused: Arc<AtomicBool>,
    step: f64,
    resampling: Resampling,
    src: Vec<[f32; 2]>,
    pos: f64,
}

impl Feeder {
    fn refill(&mut self) {
        let mut block = [0i16; 2 * BLOCK_FRAMES];
        if let Ok(mut m) = self.mixer.lock() {
            m.process_block(&mut block);
        }
        for f in block.as_chunks::<2>().0 {
            self.src.push([f32::from(f[0]) / 32768.0, f32::from(f[1]) / 32768.0]);
        }
    }

    fn next_frame(&mut self) -> [f32; 2] {
        if self.paused.load(Ordering::Relaxed) {
            return [0.0; 2];
        }
        while self.pos as usize + 3 >= self.src.len() {
            self.refill();
        }
        let i = self.pos as usize;
        let t = (self.pos - i as f64) as f32;
        let out = match self.resampling {
            Resampling::Linear => {
                let (a, b) = (self.src[i], self.src[i + 1]);
                [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t]
            }
            Resampling::Cubic => {
                let p0 = self.src[i.saturating_sub(1)];
                let (p1, p2, p3) = (self.src[i], self.src[i + 1], self.src[i + 2]);
                let c = |k: usize| {
                    let (a, b, c, d) = (p0[k], p1[k], p2[k], p3[k]);
                    b + 0.5 * t * (c - a + t * (2.0 * a - 5.0 * b + 4.0 * c - d + t * (3.0 * (b - c) + d - a)))
                };
                [c(0), c(1)]
            }
        };
        self.pos += self.step;
        // Drop consumed history, keeping one frame before the read position.
        let consumed = (self.pos as usize).saturating_sub(1);
        if consumed > 4096 {
            self.src.drain(..consumed);
            self.pos -= consumed as f64;
        }
        out
    }
}

/// A running output stream. Dropping it stops playback.
pub struct AudioOutput {
    _stream: cpal::Stream,
    paused: Arc<AtomicBool>,
    pub device_rate: u32,
    pub device_channels: u16,
    pub device_name: String,
}

impl AudioOutput {
    /// Open the default output device and start pulling from `mixer`.
    pub fn start(mixer: Arc<Mutex<Mixer>>, opts: OutputOptions) -> Result<Self, OutputError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| OutputError("no output device".into()))?;
        let device_name = device.id().map(|id| id.to_string()).unwrap_or_else(|_| "default".into());
        let native = if opts.prefer_native_rate {
            device
                .supported_output_configs()
                .ok()
                .and_then(|mut it| it.find_map(|r| if r.channels() >= 2 { r.try_with_sample_rate(OUTPUT_RATE) } else { None }))
        } else {
            None
        };
        let supported = match native {
            Some(c) => c,
            None => device.default_output_config().map_err(|e| OutputError(e.to_string()))?,
        };
        let format = supported.sample_format();
        let config: StreamConfig = supported.into();
        let paused = Arc::new(AtomicBool::new(false));
        let feeder = Feeder {
            mixer,
            paused: paused.clone(),
            step: f64::from(OUTPUT_RATE) / f64::from(config.sample_rate),
            resampling: opts.resampling,
            src: Vec::new(),
            pos: 0.0,
        };
        let stream = match format {
            SampleFormat::F32 => build::<f32>(&device, config, feeder),
            SampleFormat::F64 => build::<f64>(&device, config, feeder),
            SampleFormat::I16 => build::<i16>(&device, config, feeder),
            SampleFormat::I32 => build::<i32>(&device, config, feeder),
            SampleFormat::U16 => build::<u16>(&device, config, feeder),
            SampleFormat::U8 => build::<u8>(&device, config, feeder),
            SampleFormat::I8 => build::<i8>(&device, config, feeder),
            other => return Err(OutputError(format!("unsupported sample format {other:?}"))),
        }?;
        stream.play().map_err(|e| OutputError(e.to_string()))?;
        Ok(Self { _stream: stream, paused, device_rate: config.sample_rate, device_channels: config.channels, device_name })
    }

    /// Pause output without advancing the mixer (the game's sound pause).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

fn build<T>(device: &cpal::Device, config: StreamConfig, mut feeder: Feeder) -> Result<cpal::Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                for frame in data.chunks_mut(channels) {
                    let [l, r] = feeder.next_frame();
                    match frame.len() {
                        1 => frame[0] = T::from_sample(0.5 * (l + r)),
                        _ => {
                            frame[0] = T::from_sample(l);
                            frame[1] = T::from_sample(r);
                            for s in &mut frame[2..] {
                                *s = T::from_sample(0.0);
                            }
                        }
                    }
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| OutputError(e.to_string()))
}
