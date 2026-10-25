//! The game's sound system: UI sound events, the speech test, the intro and menu music, the
//! audio options applied as the controls move, and in a mission the music, speech (with its
//! subtitles) and the sounds of the simulation's events (guns, hits, explosions).
//!
//! Explosion and hit sounds are chosen by ship class here; the original triggers them from
//! the explosion and hit effects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use hwc_audio::codec::Quality;
use hwc_audio::mixer::AutoMode;
use hwc_audio::output::{AudioOutput, OutputOptions};
use hwc_audio::system::{event, Emitter, GameContext, NearbyObject, SpeechStarted};
use hwc_audio::tables::Listener;
use hwc_audio::{DataSource, Mixer, MixerOptions, ShipSounds, SoundData, SoundSystem};
use hwc_formats::GameData;
use hwc_sim::statics::ShipClass;
use hwc_sim::universe::SimEvent;
use hwc_sim::{ShipId, Universe};
use hwc_ui::options::Options;

/// Music track the front end starts with (`soundEventPlayMusic(0x11)` at the end of
/// `utyGameSystemsInit`).
pub const FRONT_END_TRACK: i32 = 17;
/// `utyGameQuit` fades the music over half a second.
pub const QUIT_FADE: f32 = 0.5;
/// Voices below the channel slider's range (`soundGetVoiceLimits`).
const MIN_VOICES: i32 = 8;

/// Hit events (`Hit_Nick`, `Hit_Strike`) and explosions (`Exp_DestDamageSmallFast`,
/// `Exp_DestMothership`, `Exp_DestDamageLarge`).
const HIT_NICK: u32 = event::HIT;
const HIT_STRIKE: u32 = event::HIT + 1;
const EXP_SMALL: u32 = event::EXPLOSION;
const EXP_MOTHERSHIP: u32 = event::EXPLOSION + 1;
const EXP_LARGE: u32 = event::EXPLOSION + 2;

pub struct Audio {
    system: SoundSystem,
    output: Option<AudioOutput>,
    ship: ShipSounds,
    ships: HashMap<ShipId, ShipSounds>,
}

fn explosion_event(class: Option<ShipClass>) -> u32 {
    match class {
        Some(ShipClass::Fighter | ShipClass::Corvette | ShipClass::Micro) => EXP_SMALL,
        Some(ShipClass::Mothership) => EXP_MOTHERSHIP,
        _ => EXP_LARGE,
    }
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
            ships: HashMap::new(),
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

    /// Game or front end: the music level curve and the "game running" state.
    pub fn set_in_game(&mut self, in_game: bool) {
        self.system.context.main_screen = true;
        self.system.context.active = in_game;
        self.system.context.mission_speech = 0;
        if !in_game {
            self.system.stop_all(0.0, true);
            self.ships.clear();
        }
    }

    /// Per-frame update in a game (`soundEventUpdate`), with the camera as the listener.
    pub fn game_update(&mut self, eye: [f32; 3], look_at: [f32; 3], running: bool, u: &Universe) {
        self.system.listener = Listener {
            eye,
            look_at,
            up: [0.0, 0.0, 1.0],
        };
        // The in-game music level follows the distance to the nearest audible ship.
        let objects = u.ship_list.iter().filter_map(|&id| {
            let s = u.ship(id)?;
            let st = u.statics.ship(s.ship_type)?;
            let (p, v) = (s.posinfo.position, s.posinfo.velocity);
            Some(NearbyObject {
                position: [p.x, p.y, p.z],
                class_row: Some(st.ship_class.map_or(0, |c| c.index())),
                speed: (v.x * v.x + v.y * v.y + v.z * v.z).sqrt(),
            })
        });
        self.system.note_nearby(objects);
        self.system.update(running);
    }

    /// A mission speech event on the intel stream; its subtitles and length come back.
    pub fn mission_speech(&mut self, event: u32, variable: i32) -> Option<SpeechStarted> {
        self.system.context.mission_speech = 1;
        self.system.play_speech(event, variable, -1, 0)
    }

    /// Sounds for one universe update's events.
    pub fn sim_events(&mut self, u: &Universe) {
        for e in &u.events {
            match e {
                SimEvent::GunFired { ship, gun, .. } => {
                    let Some(s) = u.ship(*ship) else { continue };
                    let Some(st) = u.statics.ship(s.ship_type) else {
                        continue;
                    };
                    let Some(gs) = st.guns.as_ref().and_then(|g| g.get(*gun)) else {
                        continue;
                    };
                    let Some(g) = s.guns.get(*gun) else { continue };
                    let p = s.posinfo.position;
                    let emitter = Emitter {
                        position: [p.x, p.y, p.z],
                        radius: st.header.coll.coll_sphere_size,
                        ship_class: st.ship_class.map_or(0, |c| c.index()),
                        ship_type: s.ship_type.index(),
                        gun_sound_type: hwc_sim::gun::bullet_type(g, gs) as usize,
                        ..Default::default()
                    };
                    let slot = self.ships.entry(*ship).or_default();
                    self.system
                        .play_event(Some(&emitter), event::GUN_SHOT, slot);
                }
                SimEvent::BulletHit {
                    ship,
                    position,
                    beam,
                    ..
                } => {
                    let small = u
                        .ship(*ship)
                        .and_then(|s| u.statics.ship(s.ship_type))
                        .is_some_and(|st| {
                            matches!(
                                st.ship_class,
                                Some(ShipClass::Fighter | ShipClass::Corvette)
                            )
                        });
                    let emitter = Emitter {
                        position: [position.x, position.y, position.z],
                        is_ship: false,
                        ..Default::default()
                    };
                    let ev = if small || *beam { HIT_NICK } else { HIT_STRIKE };
                    self.system
                        .play_event(Some(&emitter), ev, &mut ShipSounds::default());
                }
                SimEvent::ShipDied {
                    ship,
                    ship_type,
                    position,
                    ..
                } => {
                    if let Some(mut slot) = self.ships.remove(ship) {
                        self.system.stop_burst(&mut slot);
                    }
                    let class = u.statics.ship(*ship_type).and_then(|st| st.ship_class);
                    let emitter = Emitter {
                        position: [position.x, position.y, position.z],
                        is_ship: false,
                        ..Default::default()
                    };
                    self.system.play_event(
                        Some(&emitter),
                        explosion_event(class),
                        &mut ShipSounds::default(),
                    );
                }
                _ => {}
            }
        }
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
    fn explosions_by_class() {
        assert_eq!(explosion_event(Some(ShipClass::Fighter)), EXP_SMALL);
        assert_eq!(explosion_event(Some(ShipClass::Mothership)), EXP_MOTHERSHIP);
        assert_eq!(explosion_event(Some(ShipClass::Destroyer)), EXP_LARGE);
    }

    #[test]
    fn slider_levels() {
        assert_eq!(slider_level(0), 0.0);
        assert!((slider_level(60) - 0.615).abs() < 1e-6);
        assert_eq!(slider_level(100), 1.0);
        assert_eq!(slider_level(-5), 0.0);
    }
}
