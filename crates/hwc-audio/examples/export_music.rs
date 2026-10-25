//! Export every music track from the player's install to WAV files, decoded through the
//! game's own music path at 40% music volume (full volume clips in the mixer).
//!
//! ```text
//! cargo run --release -p hwc-audio --example export_music -- <out-dir>
//! ```
//!
//! Data comes from `HWC_DATA_DIR`, or `HWC_EXTRACTED_DIR` plus `HWC_GAME_DIR`, as for `jukebox`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use hwc_audio::mixer::Mixer;
use hwc_audio::offline;
use hwc_audio::system::SoundSystem;
use hwc_audio::{DataSource, SoundData};

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: export_music <out-dir>"));
    std::fs::create_dir_all(&out).expect("creating the output directory");
    let src = DataSource::from_env().expect("set HWC_DATA_DIR, or HWC_EXTRACTED_DIR and HWC_GAME_DIR");
    let data = Arc::new(SoundData::load(&src).expect("loading the sound data"));
    let tracks = data.music.as_ref().map(|m| m.tracks.clone()).unwrap_or_default();
    for (i, t) in tracks.iter().enumerate() {
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        let mut sys = SoundSystem::new(data.clone(), mixer);
        sys.set_music_volume(0.4);
        if !sys.play_music(i as i32) {
            eprintln!("track {i}: could not start");
            continue;
        }
        // A short tail so the last block and any fade are kept.
        let secs = t.duration_secs() + 0.5;
        let r = offline::render(&mut sys, &[], secs, 3);
        let path = out.join(format!("track{i:02}.wav"));
        std::fs::write(&path, r.to_wav()).expect("writing wav");
        println!("track {i:02}: {secs:.1} s{} -> {}", if t.looping { " (looping)" } else { "" }, path.display());
    }
}
