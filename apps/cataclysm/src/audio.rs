//! The game's sound system as the front end uses it: UI sound events, the speech test, the
//! intro and menu music, and the audio options applied as the controls move.

use std::sync::{Arc, Mutex};

use hwc_audio::codec::Quality;
use hwc_audio::mixer::AutoMode;
use hwc_audio::output::{AudioOutput, OutputOptions};
use hwc_audio::system::GameContext;
use hwc_audio::{DataSource, Mixer, MixerOptions, ShipSounds, SoundData, SoundSystem};
use hwc_formats::GameData;
use hwc_ui::options::Options;

/// Music track the front end starts with (`soundEventPlayMusic(0x11)` at the end of
/// `utyGameSystemsInit`).
pub const FRONT_END_TRACK: i32 = 17;
/// `utyGameQuit` fades the music over half a second.
pub const QUIT_FADE: f32 = 0.5;
/// Voices below the channel slider's range (`soundGetVoiceLimits`).
const MIN_VOICES: i32 = 8;

pub struct Audio {
    system: SoundSystem,
    output: Option<AudioOutput>,
    ship: ShipSounds,
}

/// `normalizeSmoothie`: a 0..100 volume slider value as a 0..1 level.
pub fn slider_level(value: i32) -> f32 {
    (value as f32 * 1.025 * 0.01).clamp(0.0, 1.0)
}

impl Audio {
    /// Loads the sound data and opens the default output device (`play` false: no device, for
    /// the screenshot tour and tests).
    pub fn open(game: GameData, play: bool) -> Result<Audio, String> {
        let data = SoundData::load(&DataSource::from_game_data(game)).map_err(|e| e.to_string())?;
        let mixer = Arc::new(Mutex::new(Mixer::new(MixerOptions::default())));
        let output = if play {
            match AudioOutput::start(mixer.clone(), OutputOptions::default()) {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("cataclysm: {e}; continuing without sound");
                    None
                }
            }
        } else {
            None
        };
        let mut system = SoundSystem::new(Arc::new(data), mixer);
        // Front end: no game, so the music level uses the "inactive, nothing near" curve the
        // engine falls into before any game has run.
        system.context = GameContext {
            main_screen: true,
            sensors: false,
            closest_distance: 0.0,
            active: false,
            mission_speech: 0,
        };
        Ok(Audio {
            system,
            output,
            ship: ShipSounds::default(),
        })
    }

    /// Volumes, voices, mixer quality and master EQ from the options (`opOptionsInit`, and
    /// the `op*Process` handlers while the audio screens are open).
    pub fn apply_options(&mut self, o: &Options) {
        let s = &mut self.system;
        s.set_master_volume(1.0);
        s.set_sfx_volume(slider_level(o.get("SFXVolume")));
        s.set_speech_volume(slider_level(o.get("SpeechVolume")));
        s.set_music_volume(slider_level(o.get("MusicVolume")));
        s.music_level_now();
        let mut m = s.mixer();
        let auto = |on: bool| if on { AutoMode::Auto } else { AutoMode::Off };
        m.set_num_voices(
            (o.get("NumChannels") + MIN_VOICES).max(MIN_VOICES) as usize,
            auto(o.get("AutoChannel") != 0),
        );
        // `soundMixerSetMode`: 0 normal, 1 automatic, 2 low.
        match o.get("SoundQuality") {
            2 => m.set_quality(Quality::Low, AutoMode::Off),
            1 => m.set_quality(Quality::Normal, AutoMode::Auto),
            _ => m.set_quality(Quality::Normal, AutoMode::Off),
        }
        let eq: Vec<f32> = (0..8)
            .map(|i| 1.0 - o.get(&format!("Equalizer{i}")) as f32 / 100.0)
            .collect();
        m.set_master_eq(&eq);
    }

    /// `soundEventPlay(NULL, event)`.
    pub fn event(&mut self, event: u32) {
        self.system.play_event(None, event, &mut self.ship);
    }

    /// `speechEventQueue(NULL, event, -1, -1, ...)` on the first speech stream.
    pub fn speech(&mut self, event: u32) {
        self.system.play_speech(event, -1, -1, 0);
    }

    pub fn play_music(&mut self, track: i32) {
        self.system.play_music(track);
    }

    pub fn stop_music(&mut self, fade: f32) {
        self.system.stop_music(-1, fade);
    }

    /// Per-frame update (`soundEventUpdate`).
    pub fn update(&mut self) {
        self.system.update(false);
    }

    /// `soundEventPause(TRUE)` as the game quits.
    pub fn pause(&mut self) {
        if let Some(o) = &self.output {
            o.set_paused(true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_levels() {
        assert_eq!(slider_level(0), 0.0);
        assert!((slider_level(60) - 0.615).abs() < 1e-6);
        assert_eq!(slider_level(100), 1.0);
        assert_eq!(slider_level(-5), 0.0);
    }
}
