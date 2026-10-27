//! Real-time output through cpal.
//!
//! The mixer produces 22 050 Hz stereo, as the game's DirectSound buffer did. If the device can
//! open a stream at that rate it is used directly; otherwise the output is resampled to the
//! device rate with a windowed-sinc filter (see [`crate::resample`]); the operating system did
//! that for the original.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample, StreamConfig};

use crate::codec::{BLOCK_FRAMES, OUTPUT_RATE};
use crate::mixer::Mixer;
pub use crate::resample::Resampling;
use crate::resample::Resampler;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputOptions {
    pub resampling: Resampling,
    /// Try to open the device at 22 050 Hz before falling back to its default rate.
    pub prefer_native_rate: bool,
}

impl Default for OutputOptions {
    fn default() -> Self {
        Self { resampling: Resampling::Sinc, prefer_native_rate: true }
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
    resampler: Resampler,
}

impl Feeder {
    fn next_frame(&mut self) -> [f32; 2] {
        if self.paused.load(Ordering::Relaxed) {
            return [0.0; 2];
        }
        while self.resampler.needs_input() {
            let mut block = [0i16; 2 * BLOCK_FRAMES];
            if let Ok(mut m) = self.mixer.lock() {
                m.process_block(&mut block);
            }
            self.resampler.push(block.as_chunks::<2>().0.iter().map(|f| [f32::from(f[0]) / 32768.0, f32::from(f[1]) / 32768.0]));
        }
        self.resampler.next_frame()
    }
}

/// A running output stream. Dropping it stops playback.
pub struct AudioOutput {
    _stream: cpal::Stream,
    paused: Arc<AtomicBool>,
    pub device_rate: u32,
    pub device_channels: u16,
    pub device_name: String,
    pub sample_format: SampleFormat,
}

impl AudioOutput {
    /// Open the default output device and start pulling from `mixer`.
    pub fn start(mixer: Arc<Mutex<Mixer>>, opts: OutputOptions) -> Result<Self, OutputError> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| OutputError("no output device".into()))?;
        let device_name = device.id().map(|id| id.to_string()).unwrap_or_else(|_| "default".into());
        let default = device.default_output_config().map_err(|e| OutputError(e.to_string()))?;
        let native = if opts.prefer_native_rate {
            // WASAPI lists every format at every rate, 8-bit first; taking the first match
            // played the game through an 8-bit stream (a constant hiss under the music).
            device.supported_output_configs().ok().and_then(|it| {
                it.filter(|r| r.channels() >= 2 && format_rank(r.sample_format()).is_some())
                    .filter_map(|r| r.try_with_sample_rate(OUTPUT_RATE))
                    .min_by_key(|c| {
                        (c.sample_format() != default.sample_format(), format_rank(c.sample_format()), c.channels() != 2)
                    })
            })
        } else {
            None
        };
        let supported = native.unwrap_or(default);
        let format = supported.sample_format();
        if format_rank(format).is_none_or(|r| r >= 4) {
            eprintln!("audio: {device_name} only offers {format:?} output; expect quantisation noise");
        }
        let config: StreamConfig = supported.into();
        let paused = Arc::new(AtomicBool::new(false));
        let feeder = Feeder {
            mixer,
            paused: paused.clone(),
            resampler: Resampler::new(opts.resampling, OUTPUT_RATE, config.sample_rate),
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
        Ok(Self {
            _stream: stream,
            paused,
            device_rate: config.sample_rate,
            device_channels: config.channels,
            device_name,
            sample_format: format,
        })
    }

    /// Pause output without advancing the mixer (the game's sound pause).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }
}

/// Preference among the sample formats [`build`] handles (lower is better); 8-bit formats last.
fn format_rank(f: SampleFormat) -> Option<u8> {
    Some(match f {
        SampleFormat::F32 => 0,
        SampleFormat::F64 => 1,
        SampleFormat::I32 => 2,
        SampleFormat::I16 => 3,
        SampleFormat::U16 => 4,
        SampleFormat::U8 => 5,
        SampleFormat::I8 => 6,
        _ => return None,
    })
}

fn build<T>(device: &cpal::Device, config: StreamConfig, mut feeder: Feeder) -> Result<cpal::Stream, OutputError>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels);
    let mut failed = false;
    device
        .build_output_stream(
            config,
            move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                // The device calls this from its own thread: a panic must not unwind into the
                // driver. After one, the output falls silent (the panic hook has reported it).
                if failed {
                    data.fill(T::from_sample(0.0));
                    return;
                }
                let fill = std::panic::AssertUnwindSafe(|| {
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
                });
                if std::panic::catch_unwind(fill).is_err() {
                    eprintln!("audio: the mixer panicked; sound is off");
                    // The game thread goes on using the mixer.
                    feeder.mixer.clear_poison();
                    failed = true;
                    data.fill(T::from_sample(0.0));
                }
            },
            |e| eprintln!("audio stream error: {e}"),
            None,
        )
        .map_err(|e| OutputError(e.to_string()))
}
