//! The game's sound system: UI sound events, the speech test, the intro and menu music, the
//! audio options applied as the controls move, and in a mission the music, speech (with its
//! subtitles) and the sounds of the simulation's events (guns, hits, explosions).
//!
//! Explosion and hit sounds are chosen by ship class here; the original triggers them from
//! the explosion and hit effects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// Per speech stream, the group event playing now, its variable and when it ends
    /// (`speechqueue[channel].current`).
    fleet_current: [Option<(u32, i32, Instant)>; hwc_audio::mixer::NUM_STREAMS],
    /// The Voice options: commands, status reports, chatter (`bCommandsOn` 0x88b3ec,
    /// `bStatusOn` 0x88b3f0, `bChatterOn` 0x88b3f4).
    voices: Voices,
}

#[derive(Debug, Clone, Copy)]
struct Voices {
    commands: bool,
    status: bool,
    chatter: bool,
}

impl Voices {
    /// `speechEventQueue`'s option checks: an event of a type the player turned off is not
    /// queued.
    fn allow(self, event: u32) -> bool {
        !(event & 0x100_0000 != 0 && !self.chatter
            || event & 0x200_0000 != 0 && !self.status
            || event & 0x400_0000 != 0 && !self.commands)
    }
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
    /// Mixes `blocks` blocks as the output device would (tests without a device).
    #[cfg(test)]
    pub fn pump(&mut self, blocks: usize) {
        let mut out = [0i16; 2 * hwc_audio::codec::BLOCK_FRAMES];
        let mut m = self.system.mixer();
        for _ in 0..blocks {
            m.process_block(&mut out);
        }
    }

    /// Mixes `blocks` blocks and returns them (interleaved stereo at the mixer's rate), for
    /// recordings without a device.
    #[cfg(test)]
    pub fn record(&mut self, blocks: usize) -> Vec<i16> {
        let mut out = [0i16; 2 * hwc_audio::codec::BLOCK_FRAMES];
        let mut all = Vec::with_capacity(blocks * out.len());
        let mut m = self.system.mixer();
        for _ in 0..blocks {
            m.process_block(&mut out);
            all.extend_from_slice(&out);
        }
        all
    }

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
        };
        Ok(Audio {
            system,
            output,
            ship: ShipSounds::default(),
            ships: HashMap::new(),
            fleet_current: [None; hwc_audio::mixer::NUM_STREAMS],
            voices: Voices {
                commands: true,
                status: true,
                chatter: true,
            },
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
        self.voices = Voices {
            commands: o.get("VoiceCommands") != 0,
            status: o.get("VoiceStatus") != 0,
            chatter: o.get("VoiceChatter") != 0,
        };
        let s = &mut self.system;
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

    /// The volume levels (`soundEventGetVolume`).
    pub fn volumes(&self) -> hwc_audio::system::Volumes {
        self.system.volumes()
    }

    /// Restores levels saved with [`Audio::volumes`].
    pub fn set_volumes(&mut self, v: hwc_audio::system::Volumes) {
        self.system.set_volumes(v);
    }

    /// `soundEventMusicVol`, `soundEventSpeechVol`, `soundEventSFXVol` (0..1).
    pub fn set_music_volume(&mut self, level: f32) {
        self.system.set_music_volume(level);
    }

    pub fn set_speech_volume(&mut self, level: f32) {
        self.system.set_speech_volume(level);
    }

    pub fn set_sfx_volume(&mut self, level: f32) {
        self.system.set_sfx_volume(level);
    }

    /// `soundEventPlay(NULL, event)`.
    pub fn event(&mut self, event: u32) {
        self.system.play_event(None, event, &mut self.ship);
    }

    /// `speechEventQueue(NULL, event, -1, -1, ...)` on the first speech stream.
    pub fn speech(&mut self, event: u32) {
        self.system.play_speech(event, -1, -1, 0);
    }

    /// `speechEventQueue(NULL, event, variable, ...)` for a ship-group event (`COMM_AssGrp_*`,
    /// `STAT_F_AssGrp_*`), on the stream of its speaker class. Like `speechEventQueue`, an
    /// event is dropped while the same event with the same variable is playing on its stream,
    /// so pressing a group key twice (select, then focus) says "Standby group n" once.
    pub fn fleet_speech(&mut self, event: u32, variable: i32) {
        let stream = crate::movie_script::speech_stream(event);
        let now = Instant::now();
        if let Some(Some((e, v, end))) = self.fleet_current.get(stream) {
            if (*e, *v) == (event, variable) && now < *end {
                return;
            }
        }
        if let Some(started) = self.system.play_speech(event, variable, -1, stream) {
            let end = now + Duration::from_millis(started.duration_ms.max(0) as u64);
            if let Some(slot) = self.fleet_current.get_mut(stream) {
                *slot = Some((event, variable, end));
            }
        }
    }

    /// `speechEventQueue(ship, event, variable, ...)` for speech the ships and orders asked
    /// for ([`hwc_sim::speech`]), with its checks: the Voice options, only NIS lines while an
    /// NIS plays, and only the local `player`'s ships speak (not one under construction or
    /// flagged silent, `specialFlags2 & 0x100008`). Commands do not need the ship in view.
    pub fn ship_speech(&mut self, u: &Universe, player: usize, s: hwc_sim::speech::Speech) {
        let event = s.event;
        if !self.voices.allow(event) || u.nis_running && event & 0x6000_0000 == 0 {
            return;
        }
        if let Some(id) = s.ship {
            let Some(ship) = u.ship(id) else { return };
            if ship.player != player && !u.nis_running {
                return;
            }
            if ship.special_flags2 & 0x0010_0008 != 0 {
                return;
            }
        }
        self.fleet_speech(event, s.variable.clamp(0, 0x3c));
    }

    /// The event and variable speech stream `stream` is playing (tests).
    #[cfg(test)]
    pub fn speaking(&self, stream: usize) -> Option<(u32, i32)> {
        self.fleet_current
            .get(stream)
            .copied()
            .flatten()
            .filter(|c| Instant::now() < c.2)
            .map(|(e, v, _)| (e, v))
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
        if !in_game {
            self.system.stop_all(0.0, true);
            self.fleet_current = [None; hwc_audio::mixer::NUM_STREAMS];
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
        self.system.play_speech(event, variable, -1, 0)
    }

    /// Sounds for one universe update's events.
    /// A movie script's speech event (`nisAnimaticSpeechEvent`: `speechEventQueue` with no
    /// object or actor) on the stream its speaker class uses. No single-player speech is in
    /// progress while a movie plays, so nothing ducks it.
    pub fn movie_speech(&mut self, event: u32, variable: i32) -> Option<SpeechStarted> {
        let stream = crate::movie_script::speech_stream(event);
        self.system.play_speech(event, variable, -1, stream)
    }

    /// `soundstopallSFX`: every effect stops (streams are left alone).
    pub fn stop_all_sfx(&mut self) {
        self.system.stop_all(0.0, false);
    }

    /// `speechEventCleanup`: every speech stream stops.
    pub fn stop_all_speech(&mut self) {
        self.fleet_current = [None; hwc_audio::mixer::NUM_STREAMS];
        for stream in 0..hwc_audio::mixer::NUM_STREAMS - 1 {
            self.system.stop_speech(stream, 0.0);
        }
    }

    /// Whether the mission speech stream is silent.
    pub fn mission_speech_over(&self) -> bool {
        self.system.speech_over(0)
    }

    /// Stops the mission speech (`speechEventActorStop` when fleet intel is skipped).
    pub fn stop_mission_speech(&mut self, fade: f32) {
        self.system.stop_speech(0, fade);
    }

    /// Sounds the effect scripts asked for in the last update (`playSound`: at the effect),
    /// but those of effects `muted` says are replaced (the optional Remastered deaths).
    pub fn effect_events(&mut self, u: &Universe, muted: impl Fn(u32) -> bool) {
        for e in hwc_sim::effects::events(u) {
            if let hwc_fx::FxEvent::Sound {
                event,
                position,
                effect,
                ..
            } = *e
            {
                if muted(effect) {
                    continue;
                }
                let emitter = Emitter {
                    position: [position.x, position.y, position.z],
                    is_ship: false,
                    ..Default::default()
                };
                self.system
                    .play_event(Some(&emitter), event, &mut ShipSounds::default());
            }
        }
    }

    /// The optional Remastered death sounds.
    pub fn remastered_sounds(&mut self, cues: Vec<crate::hwr_fx::SoundCue>) {
        for c in cues {
            self.system
                .play_external(&c.patch, c.position, |d| c.volume(d));
        }
    }

    /// Sounds for one universe update's events. `stand_ins`: also the hit and explosion
    /// sounds by ship class (without effects; with them the effect scripts play the
    /// original's).
    pub fn sim_events(&mut self, u: &Universe, stand_ins: bool) {
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
                SimEvent::BulletHit { .. } if !stand_ins => {}
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
                    if !stand_ins {
                        continue;
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
        // The default music level (45, lowered from the original's 60 at the user's request).
        let default = hwc_ui::options::Options::default().get("MusicVolume");
        assert!((slider_level(default) - 0.461_25).abs() < 1e-6);
        assert_eq!(slider_level(100), 1.0);
        assert_eq!(slider_level(-5), 0.0);
    }

    /// The order acknowledgements have lines for the ships that give them (the variable is
    /// the speaking ship's type, or the line number the call passes). Some pairs have none in
    /// the data (a carrier docking, a worker attacking, a sentinel's field), and stay silent
    /// as in the original (`SEselectsentence` refuses a variable past the sentence's last).
    #[test]
    fn order_acknowledgements_have_lines() {
        use hwc_sim::speech::event as e;
        use hwc_sim::statics::ShipType as T;
        let Some(Ok(g)) = GameData::from_env() else {
            eprintln!("skipped: no game data");
            return;
        };
        let Ok(mut audio) = Audio::open(g, false) else {
            eprintln!("skipped: no sound data");
            return;
        };
        let by_type = [
            e::SELECTED,
            e::MOVE,
            e::MOVE_CANCELLED,
            e::DOCK,
            e::ATTACK,
            e::ATTACK_CANCELLED,
            e::ATTACK_OWN_SHIPS,
            e::ATTACK_RESOURCES,
            e::TACTICS_EVASIVE,
            e::TACTICS_NEUTRAL,
            e::TACTICS_AGGRESSIVE,
        ];
        for ev in by_type {
            for t in [T::sAcolyte, T::sMimic, T::sMothership] {
                assert!(
                    audio
                        .system
                        .play_speech(ev, t.index() as i32, -1, 0)
                        .is_some(),
                    "no line for {ev:#x} by {t:?}"
                );
            }
        }
        assert!(audio
            .system
            .play_speech(e::DOCK, T::sCarrier.index() as i32, -1, 0)
            .is_none());
        let fixed = [
            (e::HARVEST, T::sWorker.index() as i32),
            (e::SUPPORT, T::sWorker.index() as i32),
            (e::LINK, T::sAcolyte.index() as i32),
            (e::UNLINK, T::sACV.index() as i32),
            (e::DISGUISE, T::sMimic.index() as i32),
            (e::CLOAK_ON, 0),
            (e::HIVE_RECALL, 0),
            (e::GUARD, 0),
            (e::FORMATION_SET, 0),
            (e::FORMATION_SET, 8),
            (e::RETIRE, 0),
        ];
        for (ev, v) in fixed {
            let stream = crate::movie_script::speech_stream(ev);
            assert!(
                audio.system.play_speech(ev, v, -1, stream).is_some(),
                "no line for {ev:#x} variable {v}"
            );
        }
    }

    /// A second press of a group key (select, then focus) queues no second "Standby group n"
    /// while the first plays; another group still answers.
    #[test]
    fn repeated_group_callout_is_dropped_while_it_plays() {
        let Some(Ok(g)) = GameData::from_env() else {
            eprintln!("skipped: no game data");
            return;
        };
        let Ok(mut audio) = Audio::open(g, false) else {
            eprintln!("skipped: no sound data");
            return;
        };
        audio.set_in_game(true);
        let event = crate::hotkeys::SPEECH_SELECT;
        let stream = crate::movie_script::speech_stream(event);
        let queued = |a: &Audio| {
            let m = a.system.mixer();
            m.stream_queued(m.stream_handle(stream))
        };
        audio.fleet_speech(event, 1);
        let once = queued(&audio);
        assert!(once > 0, "the callout was not queued");
        audio.fleet_speech(event, 1);
        assert_eq!(queued(&audio), once, "the repeat was queued");
        audio.fleet_speech(event, 2);
        assert!(queued(&audio) > once, "another group's callout was dropped");
    }
}
