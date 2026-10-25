//! Music level check: renders music tracks offline at a given music volume and in-game context
//! and reports peak level and how many output samples hit the 16-bit clamp.
//!
//! ```text
//! cargo run --release -p hwc-audio --example music_levels -- [--music 0.615] [--secs 60] [--distance 2000] [tracks...]
//! cargo run --release -p hwc-audio --example music_levels -- --device   # rate the sound card opens at
//! ```
//!
//! Data as for the jukebox example (`HWC_GAME_DIR` and `HWC_EXTRACTED_DIR`, or `HWC_DATA_DIR`).

use std::sync::{Arc, Mutex};

use hwc_audio::mixer::Mixer;
use hwc_audio::offline::{self, Command, Cue};
use hwc_audio::system::{GameContext, SoundSystem, Volumes};
use hwc_audio::{DataSource, SoundData};

fn main() {
    let mut music = Volumes::default().music;
    let mut secs: f64 = 60.0;
    let mut distance = f32::MAX;
    let mut tracks = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--music" => music = it.next().and_then(|s| s.parse().ok()).unwrap_or(music),
            "--secs" => secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(secs),
            "--device" => {
                let o = hwc_audio::output::AudioOutput::start(Arc::new(Mutex::new(Mixer::default())), Default::default());
                match o {
                    Ok(o) => println!("output device {}: {} Hz, {} channels", o.device_name, o.device_rate, o.device_channels),
                    Err(e) => println!("{e}"),
                }
                return;
            }
            "--distance" => distance = it.next().and_then(|s| s.parse().ok()).unwrap_or(distance),
            t => tracks.extend(t.parse::<i32>().ok()),
        }
    }
    let Some(src) = DataSource::from_env() else {
        eprintln!("set HWC_GAME_DIR (and optionally HWC_EXTRACTED_DIR) or HWC_DATA_DIR");
        return;
    };
    let data = Arc::new(SoundData::load(&src).expect("sound data"));
    let Some(m) = &data.music else {
        eprintln!("no music (set HWC_GAME_DIR)");
        return;
    };
    if tracks.is_empty() {
        tracks = (0..m.tracks.len() as i32).collect();
    }
    println!("music level {music:.3}, closest ship {distance}, {secs} s per track");
    println!("track vol  peak dBFS   clipped samples");
    for t in tracks {
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let mut sys = SoundSystem::new(data.clone(), mixer);
        let defaults = Volumes::default();
        let cues = [
            Cue::new(0.0, Command::Volumes { sfx: defaults.sfx, speech: defaults.speech, music }),
            Cue::new(0.0, Command::Context(GameContext { closest_distance: distance, ..Default::default() })),
            Cue::new(0.0, Command::Music(t)),
        ];
        let len = secs.min(m.tracks[t as usize].duration_secs());
        let r = offline::render(&mut sys, &cues, len, 3);
        let peak = r.left.iter().chain(&r.right).fold(0.0f32, |p, &x| p.max(x.abs()));
        let clipped = r.left.iter().chain(&r.right).filter(|x| x.abs() > 32767.0).count();
        println!(
            "{t:>5} {:>3} {:>9.2} {:>9} ({:.3}%)",
            m.tracks[t as usize].volume,
            20.0 * (f64::from(peak) / 32768.0).log10(),
            clipped,
            100.0 * clipped as f64 / (2 * r.frames()).max(1) as f64
        );
    }
}
