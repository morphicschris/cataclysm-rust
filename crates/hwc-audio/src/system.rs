//! The game-facing sound API: sound events through the `PKG0` tables, speech sentences, music,
//! volumes and the listener. This is the layer the original's `SoundEvent*`, `SpeechEvent` and
//! `musicEvent*` code provided on top of the mixer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::codec::Eq;
use crate::data::{Race, SoundBank, SoundData};
use crate::mixer::{priority, Mixer, StreamData, StreamEntry, NO_HANDLE};
use crate::tables::{Cardioid, Listener, Vec3};
use hwc_formats::audio::SfxLut;

/// Event numbers: the category flag in bits 8..15, the index into the category's table in the
/// low byte.
pub mod event {
    pub const MASK: u32 = 0xff00;
    pub const INDEX: u32 = 0x00ff;

    pub const UI: u32 = 0x100;
    pub const GUN: u32 = 0x200;
    pub const SHIP_CMN: u32 = 0x400;
    pub const SHIP: u32 = 0x800;
    pub const EXPLOSION: u32 = 0x1000;
    pub const HIT: u32 = 0x4000;
    /// Special gun sounds (`gunspecevents.lut`), only reached through gun events.
    pub const GUN_SPEC: u32 = 0x8000;

    pub const GUN_MOVE: u32 = GUN;
    pub const GUN_SHOT: u32 = GUN + 1;
    pub const GUN_FIRE_LOOPED: u32 = GUN + 2;
    pub const SHIP_ENGINE: u32 = SHIP_CMN;
    pub const SHIP_AMBIENT: u32 = SHIP_CMN + 1;
    /// UI events up to this one are background ambiences (one at a time).
    pub const UI_LAST_AMBIENT: u32 = UI + 7;
    /// Track number that picks one of the first 17 tracks at random.
    pub const MUSIC_RANDOM: i32 = 1102;
}

/// Rows of the distance tables used by each category (Cataclysm's layout).
mod rows {
    pub const SHIP_AMBIENT_OFFSET: usize = 12;
    pub const GUN_SHOT_OFFSET: usize = 24;
    pub const GUN_MOVE_OFFSET: usize = 44;
    pub const EXPLOSION_OFFSET: usize = 68;
    pub const EXPLOSION_LAST: usize = 82;
    pub const EXPLOSION_SECONDARY: usize = 81;
    pub const HIT_OFFSET: usize = 84;
    pub const HIT_LAST: usize = 86;
    pub const SPECIAL_GUN: usize = 9;
    pub const EFFECT_TYPE: usize = 68;
}

/// Size the game assumes for effects when panning them.
const EFFECT_SIZE: f32 = 500.0;
/// Hit events with their own concurrent-voice slots.
const HIT_EVENTS: usize = 8;
/// Music tracks the random-track request picks from.
const RANDOM_TRACKS: u32 = 17;
/// Track played at the master level instead of the music level.
const MASTER_LEVEL_TRACK: i32 = 41;

/// The game's multi-stream random generator (one instance per stream).
#[derive(Debug, Clone)]
pub struct RanStream {
    x: u32,
    y: u32,
    z: u32,
    c: u32,
    n: u32,
}

impl Default for RanStream {
    fn default() -> Self {
        Self { x: 521_288_629, y: 362_436_069, z: 1_613_801, c: 1, n: 1_131_199_209 }
    }
}

impl RanStream {
    pub fn next_u32(&mut self) -> u32 {
        let xc = self.x.wrapping_add(self.c);
        let s = if self.y > xc {
            self.c = 0;
            self.y.wrapping_sub(xc)
        } else {
            self.c = 1;
            xc.wrapping_sub(self.y).wrapping_sub(18)
        };
        self.x = self.y;
        self.y = self.z;
        self.z = s;
        self.n = self.n.wrapping_mul(69069).wrapping_add(1_013_904_243);
        s.wrapping_add(self.n)
    }

    /// Advance a number of steps (the game's time-based "randomize").
    pub fn skip(&mut self, n: u32) {
        for _ in 0..n {
            self.next_u32();
        }
    }
}

/// Which effect bank a patch comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankKind {
    Guns,
    Ships,
    Special,
    Ui,
}

/// Something that makes sound in the world.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emitter {
    pub position: Vec3,
    /// Approximate collision-sphere size (ships); effects use a fixed 500.
    pub radius: f32,
    /// Ship class (row of the distance tables for ship sounds).
    pub ship_class: usize,
    /// Ship type (object index into the common ship event table).
    pub ship_type: usize,
    /// The firing gun's sound type (object index into the gun event table).
    pub gun_sound_type: usize,
    /// 0..1, how much of the ship is hidden; reduces gun and ambient volumes.
    pub coverage: f32,
    /// Whether the object is a ship (common ship events need one).
    pub is_ship: bool,
    /// Whether the object is in the render list; sounds of unseen objects are not played.
    pub visible: bool,
}

impl Default for Emitter {
    fn default() -> Self {
        Self {
            position: [0.0; 3],
            radius: 0.0,
            ship_class: 0,
            ship_type: 0,
            gun_sound_type: 0,
            coverage: 0.0,
            is_ship: true,
            visible: true,
        }
    }
}

/// Per-ship sound handles the game keeps in each ship (the original `SOUNDEVENT` struct).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShipSounds {
    pub engine: i32,
    pub engine_starting: bool,
    pub gun: i32,
    pub burst: i32,
    pub burst_firing: bool,
}

impl Default for ShipSounds {
    fn default() -> Self {
        Self { engine: NO_HANDLE, engine_starting: false, gun: NO_HANDLE, burst: NO_HANDLE, burst_firing: false }
    }
}

/// Game state the sound code reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameContext {
    /// The main 3D view is showing (not a manager screen).
    pub main_screen: bool,
    /// The sensors manager is open.
    pub sensors: bool,
    /// Distance from the camera to the closest ship, for the in-game music level.
    pub closest_distance: f32,
    /// Something is going on (selects the "active" music level curve).
    pub active: bool,
}

impl Default for GameContext {
    fn default() -> Self {
        Self { main_screen: true, sensors: false, closest_distance: f32::MAX, active: false }
    }
}

/// Something in view that the in-game music level takes its distance from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NearbyObject {
    pub position: Vec3,
    /// Distance-table row (ship class) for ships, which only count while within their audible
    /// range; `None` for derelicts, which count at any distance.
    pub class_row: Option<usize>,
    /// Current speed; a moving closest ship selects the "active" music curve.
    pub speed: f32,
}

/// Opt-in behaviour beyond the original. All off by default.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Enhancements {
    /// Scale in-game music by this factor while any speech stream is playing.
    pub duck_music_under_speech: Option<f32>,
}

/// User volume settings, as the game stores them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Volumes {
    pub master: f32,
    pub sfx: f32,
    pub speech: f32,
    pub music: f32,
}

/// The options screen's slider positions (0..100) a fresh install starts with: music 60,
/// effects 75, speech 75.
pub const DEFAULT_SLIDERS: Sliders = Sliders { music: 60, sfx: 75, speech: 75 };

/// Volume slider positions as the options screen stores them (0..100).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sliders {
    pub music: i32,
    pub sfx: i32,
    pub speech: i32,
}

/// A slider position (0..100) as a level 0..1: the game scales by 1.025 so that the top few
/// positions all reach full level.
pub fn slider_level(position: i32) -> f32 {
    (position as f32 * 1.025 * 0.01).clamp(0.0, 1.0)
}

impl Volumes {
    /// Levels for slider positions, with the master (cinematic) scale at 1.
    pub fn from_sliders(s: Sliders) -> Self {
        Self { master: 1.0, sfx: slider_level(s.sfx), speech: slider_level(s.speech), music: slider_level(s.music) }
    }

    /// Everything at full level (louder than any slider setting short of the top).
    pub const FULL: Self = Self { master: 1.0, sfx: 1.0, speech: 1.0, music: 1.0 };
}

/// The levels of a fresh install ([`DEFAULT_SLIDERS`]), which the sound system starts with.
impl Default for Volumes {
    fn default() -> Self {
        Self::from_sliders(DEFAULT_SLIDERS)
    }
}

/// What [`SoundSystem::play_speech`] queued.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechStarted {
    pub stream: usize,
    pub phrase_row: usize,
    pub fragments: usize,
    pub duration_ms: i32,
    pub volume: i16,
    pub subtitles: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MusicState {
    Idle,
    Starting,
    Playing,
    Stopping,
    Switching,
}

const NO_TARGET: i32 = 0xffff;

struct Music {
    state: MusicState,
    track: i32,
    pending: i32,
    target: i32,
    immediate: bool,
    stream: i32,
    /// Second stream used by [`SoundSystem::crossfade_music`].
    spare: i32,
}

/// The sound system. Cheap to drive from the game thread; the mixer inside is shared with the
/// output backend.
pub struct SoundSystem {
    mixer: Arc<Mutex<Mixer>>,
    data: Arc<SoundData>,
    cardioid: Cardioid,
    pub listener: Listener,
    pub context: GameContext,
    pub enhancements: Enhancements,
    volumes: Volumes,
    sfx_level: f32,
    rng_sound: RanStream,
    rng_speech: RanStream,
    rng_music: RanStream,
    race: Race,
    ambient: i32,
    hit_slots: [[i32; 2]; HIT_EVENTS],
    shuffle: HashMap<(Race, usize), u32>,
    music: Music,
    letterbox_saved: f32,
    letterbox: bool,
    sfx_enabled: bool,
}

impl SoundSystem {
    pub fn new(data: Arc<SoundData>, mixer: Arc<Mutex<Mixer>>) -> Self {
        let t = &data.tweaks;
        let cardioid = Cardioid::new(t.sfx_cardioid_factor, t.sfx_cardioid_min, t.random_ambience_minfreq);
        let (stream, spare) = {
            let m = mixer.lock().unwrap();
            (m.stream_handle(crate::mixer::NUM_STREAMS - 1), NO_HANDLE)
        };
        Self {
            mixer,
            data,
            cardioid,
            listener: Listener::default(),
            context: GameContext::default(),
            enhancements: Enhancements::default(),
            volumes: Volumes::default(),
            sfx_level: Volumes::default().master * Volumes::default().sfx,
            rng_sound: RanStream::default(),
            rng_speech: RanStream::default(),
            rng_music: RanStream::default(),
            race: Race::Somtaaw,
            ambient: NO_HANDLE,
            hit_slots: [[NO_HANDLE; 2]; HIT_EVENTS],
            shuffle: HashMap::new(),
            music: Music {
                state: MusicState::Idle,
                track: -1,
                pending: -1,
                target: NO_TARGET,
                immediate: false,
                stream,
                spare,
            },
            letterbox_saved: 10.0,
            letterbox: false,
            sfx_enabled: true,
        }
    }

    pub fn data(&self) -> &Arc<SoundData> {
        &self.data
    }

    pub fn mixer(&self) -> MutexGuard<'_, Mixer> {
        self.mixer.lock().unwrap()
    }

    pub fn mixer_handle(&self) -> Arc<Mutex<Mixer>> {
        self.mixer.clone()
    }

    pub fn cardioid(&self) -> &Cardioid {
        &self.cardioid
    }

    /// Seed all random streams by advancing them `n` steps.
    pub fn randomize(&mut self, n: u32) {
        self.rng_sound.skip(n);
        self.rng_speech.skip(n);
        self.rng_music.skip(n);
    }

    /// Player race: picks the UI table object and the speech voice set.
    pub fn set_race(&mut self, race: Race) {
        self.race = race;
    }

    pub fn race(&self) -> Race {
        self.race
    }

    pub fn set_sfx_enabled(&mut self, on: bool) {
        self.sfx_enabled = on;
    }

    // --------------------------------------------------------------------------------------------
    // Volumes
    // --------------------------------------------------------------------------------------------

    pub fn volumes(&self) -> Volumes {
        self.volumes
    }

    /// Set all four levels at once.
    pub fn set_volumes(&mut self, v: Volumes) {
        self.set_master_volume(v.master);
        self.set_sfx_volume(v.sfx);
        self.set_speech_volume(v.speech);
        self.set_music_volume(v.music);
    }

    /// Effects level 0..1 (scaled by the master level).
    pub fn set_sfx_volume(&mut self, level: f32) {
        self.volumes.sfx = level.clamp(0.0, 1.0);
        self.sfx_level = self.volumes.master * self.volumes.sfx;
    }

    pub fn set_speech_volume(&mut self, level: f32) {
        self.volumes.speech = level.clamp(0.0, 1.0);
    }

    pub fn set_music_volume(&mut self, level: f32) {
        self.volumes.music = level.clamp(0.0, 1.0);
    }

    pub fn set_master_volume(&mut self, level: f32) {
        self.volumes.master = level.clamp(0.0, 1.0);
        self.sfx_level = self.volumes.master * self.volumes.sfx;
    }

    fn speech_level(&self) -> f32 {
        self.volumes.master * self.volumes.speech
    }

    fn music_level(&self) -> f32 {
        (self.volumes.master * self.volumes.music).clamp(0.0, 1.0)
    }

    /// Letterboxed (in-game cinematic) mode: effects drop to 60% (30% with `heavy`) and the
    /// in-game music to 70%, as in the game.
    pub fn set_letterbox(&mut self, on: bool, heavy: bool) {
        self.letterbox = on;
        if on {
            self.letterbox_saved = self.sfx_level;
            self.sfx_level *= if heavy { 0.3 } else { 0.6 };
        } else if self.letterbox_saved < 9.0 {
            self.sfx_level = self.letterbox_saved;
            self.letterbox_saved = 10.0;
        }
    }

    // --------------------------------------------------------------------------------------------
    // Effects
    // --------------------------------------------------------------------------------------------

    fn bank(&self, kind: BankKind) -> &SoundBank {
        match kind {
            BankKind::Guns => &self.data.guns,
            BankKind::Ships => &self.data.ships,
            BankKind::Special => &self.data.special,
            BankKind::Ui => &self.data.ui,
        }
    }

    /// Choose a patch for (object, event), picking a random variation when there are several.
    pub fn pick_patch(&mut self, lut: &SfxLut, object: usize, event: u32) -> i32 {
        if object >= usize::from(lut.num_objects) {
            return NO_HANDLE;
        }
        let Some(row) = lut.row(object, (event & event::INDEX) as usize) else { return NO_HANDLE };
        let count = row[0];
        if count < 2 {
            return i32::from(row[1]);
        }
        let r = self.rng_sound.next_u32() % count as u32;
        i32::from(row[1 + r as usize])
    }

    /// Play a patch directly (the game's `splay`).
    #[allow(clippy::too_many_arguments)]
    pub fn play_patch(
        &mut self,
        bank: BankKind,
        patch: i32,
        eq: Option<&Eq>,
        pan: i16,
        prio: i32,
        volume: i16,
        fade_in: bool,
    ) -> i32 {
        let Some(p) = self.bank(bank).patch(patch).cloned() else { return NO_HANDLE };
        self.mixer.lock().unwrap().play(&p, eq, -1.0, pan, prio, volume, false, fade_in, false)
    }

    fn in_game(&self) -> bool {
        !self.context.sensors && self.context.main_screen
    }

    fn atten(&self, row: usize, distance: f32) -> (i16, Option<Eq>) {
        let a = self.data.tables.volume_eq(row, distance, self.sfx_level, self.data.tweaks.sfx_min_perceptible_vol);
        (a.volume, a.eq)
    }

    /// Play a sound event. `emitter` is the object making the sound (`None` for UI sounds);
    /// `ship` holds the handles the game keeps per ship. Returns the voice handle or
    /// [`NO_HANDLE`].
    pub fn play_event(&mut self, emitter: Option<&Emitter>, event: u32, ship: &mut ShipSounds) -> i32 {
        if !self.sfx_enabled {
            return 0;
        }
        let mut dist = 0.0;
        if let Some(e) = emitter {
            if !e.visible {
                return NO_HANDLE;
            }
            dist = self.listener.distance(e.position);
        }
        let data = self.data.clone();
        match event & event::MASK {
            event::UI => self.play_ui(event),
            event::GUN => {
                let Some(e) = emitter else { return NO_HANDLE };
                if !self.in_game() {
                    return NO_HANDLE;
                }
                let gst = e.gun_sound_type;
                if gst >= usize::from(data.gun_events.num_objects) {
                    return NO_HANDLE;
                }
                let pan = self.listener.pan_angle(e.position, e.radius, dist);
                let single = event == event::GUN_SHOT || (event == event::GUN_FIRE_LOOPED && matches!(gst, 0 | 4 | 10));
                if event == event::GUN_MOVE {
                    let row = gst + rows::GUN_MOVE_OFFSET;
                    if !data.tables.in_range(row, dist) || !self.mixer().is_over(ship.gun) {
                        return NO_HANDLE;
                    }
                    let (vol, eq) = self.atten(row, dist);
                    let vol = round_vol((1.0 - e.coverage) * f32::from(vol));
                    let p = self.pick_patch(&data.gun_events, gst, event);
                    self.play_patch(BankKind::Guns, p, eq.as_ref(), pan, priority::MIN, vol, true)
                } else if single {
                    let (vol, eq) = if gst == 0x13 {
                        if !data.tables.in_range(rows::SPECIAL_GUN, dist) {
                            return NO_HANDLE;
                        }
                        self.atten(rows::SPECIAL_GUN, dist)
                    } else {
                        let row = gst + rows::GUN_SHOT_OFFSET;
                        if !data.tables.in_range(row, dist) || !self.mixer().is_over(ship.gun) {
                            return NO_HANDLE;
                        }
                        let (v, eq) = self.atten(row, dist);
                        (round_vol((1.0 - e.coverage) * f32::from(v)), eq)
                    };
                    let p = if matches!(e.ship_type, 10 | 0x19 | 0x55) {
                        self.pick_patch(&data.gun_spec_events, 0, event::GUN_SPEC + 2)
                    } else {
                        self.pick_patch(&data.gun_events, gst, event::GUN_SHOT)
                    };
                    let h = self.play_patch(BankKind::Guns, p, eq.as_ref(), pan, priority::HIGH, vol, true);
                    if gst != 0x13 {
                        ship.gun = h;
                    }
                    h
                } else {
                    let row = gst + rows::GUN_SHOT_OFFSET;
                    if !data.tables.in_range(row, dist) || ship.burst != NO_HANDLE {
                        return NO_HANDLE;
                    }
                    ship.burst_firing = true;
                    let (v, eq) = self.atten(row, dist);
                    let vol = round_vol((1.0 - e.coverage * 0.5) * f32::from(v));
                    let p = match e.ship_type {
                        7 | 0x29 => self.pick_patch(&data.gun_spec_events, 0, event::GUN_SPEC + 1),
                        0x85 => self.pick_patch(&data.gun_spec_events, 0, event::GUN_SPEC),
                        _ => self.pick_patch(&data.gun_events, gst, event::GUN_FIRE_LOOPED),
                    };
                    let h = self.play_patch(BankKind::Guns, p, eq.as_ref(), pan, priority::MAX, vol, true);
                    ship.burst = h;
                    h
                }
            }
            event::SHIP_CMN => {
                let Some(e) = emitter else { return NO_HANDLE };
                if !self.in_game() || !e.is_ship {
                    return NO_HANDLE;
                }
                let class = e.ship_class;
                if !data.tables.in_range(class, dist) {
                    return NO_HANDLE;
                }
                let (prio, vol, eq);
                if event == event::SHIP_ENGINE {
                    if ship.engine != NO_HANDLE {
                        return NO_HANDLE;
                    }
                    ship.engine_starting = true;
                    prio = match class {
                        5 => priority::MAX - 1,
                        6 => priority::MAX - 2,
                        _ => priority::MAX,
                    };
                    let (v, q) = self.atten(class, dist);
                    eq = q;
                    vol = if matches!(class, 5 | 6) { ((1.0 - e.coverage) * f32::from(v)) as i16 } else { v };
                } else {
                    let (v, q) = self.atten(class + rows::SHIP_AMBIENT_OFFSET, dist);
                    eq = q;
                    vol = ((1.0 - e.coverage) * f32::from(v)) as i16;
                    prio = priority::MIN;
                }
                let pan = self.listener.pan_angle(e.position, e.radius, dist);
                let mut h = NO_HANDLE;
                if e.ship_type < usize::from(data.ship_cmn_events.num_objects) {
                    let p = self.pick_patch(&data.ship_cmn_events, e.ship_type, event);
                    h = self.play_patch(BankKind::Ships, p, eq.as_ref(), pan, prio, vol, true);
                }
                if event == event::SHIP_ENGINE {
                    ship.engine = h;
                    if h == NO_HANDLE {
                        ship.engine_starting = false;
                    }
                }
                h
            }
            event::SHIP => {
                let Some(e) = emitter else { return NO_HANDLE };
                if !self.in_game() || !e.is_ship {
                    return NO_HANDLE;
                }
                let (vol, eq) = self.atten(e.ship_class, dist);
                let pan = self.listener.pan_angle(e.position, e.radius, dist);
                let p = self.pick_patch(&data.ship_events, 0, event);
                self.play_patch(BankKind::Ships, p, eq.as_ref(), pan, priority::NORMAL, vol, true)
            }
            event::EXPLOSION => {
                let Some(e) = emitter else { return NO_HANDLE };
                if !self.in_game() {
                    return NO_HANDLE;
                }
                let index = (event & event::INDEX) as usize;
                let row = if index < 0x10 { rows::EXPLOSION_OFFSET + index } else { rows::EXPLOSION_LAST };
                if !data.tables.in_range(row, dist) {
                    return NO_HANDLE;
                }
                let (vol, eq) = self.atten(row, dist);
                let pan = self.listener.pan_angle(e.position, EFFECT_SIZE, dist);
                let p = self.pick_patch(&data.spec_exp_events, 0, event);
                let h = self.play_patch(BankKind::Special, p, eq.as_ref(), pan, priority::STREAM, vol, true);
                if !matches!(index, 2 | 4 | 7) || !data.tables.in_range(rows::EXPLOSION_SECONDARY, dist) {
                    return h;
                }
                let (vol, eq) = self.atten(rows::EXPLOSION_SECONDARY, dist);
                if vol < 101 {
                    return h;
                }
                let p = self.pick_patch(&data.spec_exp_events, 0, event::EXPLOSION + 0x0d);
                self.play_patch(BankKind::Special, p, eq.as_ref(), pan, priority::STREAM, vol, true)
            }
            event::HIT => {
                let Some(e) = emitter else { return NO_HANDLE };
                if !self.in_game() {
                    return NO_HANDLE;
                }
                let index = (event & event::INDEX) as usize;
                let row = if index == 7 { rows::HIT_LAST } else { rows::HIT_OFFSET + index };
                if index >= HIT_EVENTS || !data.tables.in_range_sqr(row, dist * dist) {
                    return NO_HANDLE;
                }
                for slot in 0..2 {
                    if self.mixer().is_over(self.hit_slots[index][slot]) {
                        let (vol, eq) = self.atten(row, dist);
                        let pan = self.listener.pan_angle(e.position, EFFECT_SIZE, dist);
                        let p = self.pick_patch(&data.spec_hit_events, 0, event);
                        let h = self.play_patch(BankKind::Special, p, eq.as_ref(), pan, priority::HIGH, vol, true);
                        self.hit_slots[index][slot] = h;
                        return h;
                    }
                }
                NO_HANDLE
            }
            _ => NO_HANDLE,
        }
    }

    /// A sound from outside the game's banks (the optional Remastered effects): `patch` at
    /// `position`, fading with distance and muffled as explosion `event` is (its range, volume
    /// and EQ rows), at `gain` (0..1) of that, panned as effects are.
    pub fn play_external(&mut self, patch: &crate::mixer::PatchRef, position: Vec3, event: u32, gain: f32) -> i32 {
        if !self.in_game() {
            return NO_HANDLE;
        }
        let dist = self.listener.distance(position);
        let index = (event & event::INDEX) as usize;
        let row = if index < 0x10 { rows::EXPLOSION_OFFSET + index } else { rows::EXPLOSION_LAST };
        if !self.data.tables.in_range(row, dist) {
            return NO_HANDLE;
        }
        let (vol, eq) = self.atten(row, dist);
        let vol = round_vol(gain.clamp(0.0, 1.0) * f32::from(vol));
        if vol < 1 {
            return NO_HANDLE;
        }
        let pan = self.listener.pan_angle(position, EFFECT_SIZE, dist);
        self.mixer().play(patch, eq.as_ref(), -1.0, pan, priority::NORMAL, vol, false, false, false)
    }

    /// The Cataclysm `soundEffectType`: an effect-driven ship sound (ship event 0x0c).
    pub fn play_effect_type(&mut self, position: Vec3) -> i32 {
        if !self.in_game() {
            return NO_HANDLE;
        }
        let dist = self.listener.distance(position);
        let data = self.data.clone();
        if !data.tables.in_range_sqr(rows::EFFECT_TYPE, dist * dist) {
            return NO_HANDLE;
        }
        let (vol, eq) = self.atten(rows::EFFECT_TYPE, dist);
        let pan = self.listener.pan_angle(position, EFFECT_SIZE, dist);
        let p = self.pick_patch(&data.ship_events, 0, event::SHIP + 0x0c);
        self.play_patch(BankKind::Ships, p, eq.as_ref(), pan, priority::NORMAL, vol, true)
    }

    fn play_ui(&mut self, event: u32) -> i32 {
        let data = self.data.clone();
        if (event::UI..=event::UI_LAST_AMBIENT).contains(&event) {
            let old = self.ambient;
            self.mixer().stop(old, 0.5);
            let vol = round_vol(data.tweaks.sfx_ambient_volume as f32 * self.sfx_level);
            let p = self.pick_patch(&data.ui_events, self.race.index(), event);
            self.ambient = self.play_patch(BankKind::Ui, p, None, 0, priority::STREAM, vol, true);
            return NO_HANDLE;
        }
        let vol = round_vol(self.sfx_level * 255.0);
        if event == 0x1c8 {
            let p = self.pick_patch(&data.ship_events, 0, event::SHIP + 0x21);
            self.play_patch(BankKind::Ships, p, None, 0, priority::MAX, vol, false)
        } else {
            let p = self.pick_patch(&data.ui_events, self.race.index(), event);
            self.play_patch(BankKind::Ui, p, None, 0, priority::MAX, vol, false)
        }
    }

    /// Handle of the current UI background ambience.
    pub fn ambient_handle(&self) -> i32 {
        self.ambient
    }

    /// Stop a sound: immediately-ish (20 blocks) for `quick`, else over half a second.
    pub fn stop_sound(&mut self, handle: i32, quick: bool) -> bool {
        self.mixer().stop(handle, if quick { 0.0 } else { 0.5 })
    }

    /// End a ship's looping burst.
    pub fn stop_burst(&mut self, ship: &mut ShipSounds) {
        if ship.burst_firing {
            ship.burst_firing = false;
            if ship.burst >= 0 {
                self.mixer().stop(ship.burst, 0.5);
                ship.burst = NO_HANDLE;
            }
        }
    }

    /// Re-aim a playing voice at its emitter: recompute volume, EQ and pan for distance-table
    /// `row` (what the game's per-frame update does for engines and ambiences).
    pub fn update_voice(&mut self, handle: i32, row: usize, emitter: &Emitter) {
        let dist = self.listener.distance(emitter.position);
        let (vol, eq) = self.atten(row, dist);
        let pan = self.listener.pan_angle(emitter.position, emitter.radius, dist);
        let mut m = self.mixer.lock().unwrap();
        m.set_volume(handle, vol, 0.0);
        m.set_pan(handle, pan, 0.0);
        if let Some(eq) = eq {
            m.set_eq(handle, &eq);
        }
    }

    /// Apply the fake-doppler EQ from a ship's heading: bands below `low` get the front gain,
    /// bands from `high` up the rear gain, with a ramp between (the game's doppler tweaks).
    #[allow(clippy::too_many_arguments)]
    pub fn set_ship_heading(&mut self, handle: i32, position: Vec3, heading: Vec3, low: usize, high: usize, vel: f32, ship: f32) {
        let angle = self.listener.angle_to_heading(position, heading);
        let c = self.cardioid.clone();
        self.mixer().set_ship_heading(handle, angle, high, low, vel, ship, &c);
    }

    /// Stop all effects with a fade (optionally streams too).
    pub fn stop_all(&mut self, fade: f32, streams: bool) {
        self.mixer().stop_all(fade, streams);
    }

    // --------------------------------------------------------------------------------------------
    // Speech
    // --------------------------------------------------------------------------------------------

    /// The game's variation shuffle: a random order of the `count` variations, weighted by the
    /// phrase table's probabilities, packed one per nibble.
    fn shuffle_sequence(&mut self, probs: &[u8]) -> u32 {
        let n = probs.len();
        let mut order: Vec<u32> = (0..n as u32).collect();
        let mut weight = Vec::with_capacity(n);
        let mut acc = 0u8;
        for &p in probs {
            let w = p.wrapping_sub(acc);
            acc = acc.wrapping_add(w);
            weight.push(w);
        }
        if n > 1 {
            for _ in 0..n - 1 {
                let u = (self.rng_speech.next_u32() % (n as u32 - 1)) as usize;
                if weight[u] <= weight[u + 1] {
                    order.swap(u, u + 1);
                    weight.swap(u, u + 1);
                }
            }
        }
        order.iter().rev().fold(0u32, |seq, &o| (seq << 4) | (o + 1))
    }

    /// Pick the phrase table row for a sentence (`SEselectsentence`). `variable` -1 picks at
    /// random; `variation` -1 (or out of range) uses the shuffled sequence.
    pub fn select_sentence(&mut self, race: Race, event: usize, variable: i32, variation: i32) -> Option<usize> {
        let data = self.data.clone();
        let speech = data.speech(race)?;
        let s = speech.sentences.sentence(0, event)?;
        let nvar = s.num_variations as u16 as i32;
        if nvar == 0 {
            return None;
        }
        let maxv = s.max_variable as u16 as i32;
        let first = s.phrase_index as u16 as i32;
        let var = if variable < 0 {
            if maxv == 0 {
                return None;
            }
            (self.rng_speech.next_u32() % maxv as u32) as i32
        } else if variable >= maxv {
            return None;
        } else {
            variable
        };
        let row = if variation == -1 || variation >= nvar {
            if nvar == 1 {
                first + var
            } else {
                let base = (first + var) as usize;
                let mut seq = self.shuffle.get(&(race, base)).copied().unwrap_or(0);
                if seq == 0 {
                    let probs: Vec<u8> = (0..nvar)
                        .map(|k| speech.phrases.phrases.get(base + (k * maxv) as usize).map_or(0, |p| p.probability))
                        .collect();
                    seq = self.shuffle_sequence(&probs);
                }
                let pick = (seq & 0xf) as i32 - 1;
                self.shuffle.insert((race, base), seq >> 4);
                maxv * pick + first + var
            }
        } else {
            first + var + maxv * variation
        };
        usize::try_from(row).ok()
    }

    /// Speak sentence `event` (the low 11 bits of a speech event; the upper bits are the
    /// game's actor/class flags, which affect the level) on speech stream `stream` (0..3).
    pub fn play_speech(&mut self, event: u32, variable: i32, variation: i32, stream: usize) -> Option<SpeechStarted> {
        if stream >= crate::mixer::NUM_STREAMS - 1 {
            return None;
        }
        let race = self.race;
        let row = self.select_sentence(race, (event & 0x7ff) as usize, variable, variation)?;
        let data = self.data.clone();
        let speech = data.speech(race)?;
        let phrase = speech.phrases.phrases.get(row)?;
        if phrase.placeholder {
            return None;
        }
        let mut vol = self.speech_level() * 255.0;
        if self.single_player_speaking() < 1 || event & 0x1000_0000 != 0 {
            if event & 0x800 != 0 {
                vol *= 0.75;
            }
        } else {
            vol *= data.tweaks.speech_singleplayer_ratio;
        }
        if self.letterbox && event & 0x0300_0000 != 0 {
            vol *= data.tweaks.speech_singleplayer_ratio;
        }
        if vol < data.tweaks.speech_min_perceptible_vol {
            return None;
        }
        let eq = data.eq_intel.as_option();
        let handle = self.mixer().stream_handle(stream);
        let mut subtitles = Vec::new();
        let n = phrase.offsets.len();
        let mut m = self.mixer.lock().unwrap();
        for (i, &off) in phrase.offsets.iter().enumerate() {
            let frag = speech.fragments.get(&(off as usize))?;
            if let Some(s) = &frag.subtitle {
                subtitles.push(s.clone());
            }
            let last = i + 1 == n;
            let v = if last { round_vol(vol) } else { vol as i16 };
            let mut entry = StreamEntry::new(
                StreamData { bytes: speech.bytes.clone(), start: frag.data_offset, len: frag.len },
                v,
                0,
                1,
                speech.bitrate(),
            );
            entry.eq = eq;
            entry.tag = i64::from(event);
            m.stream_queue(handle, entry, !last);
        }
        Some(SpeechStarted {
            stream,
            phrase_row: row,
            fragments: n,
            duration_ms: phrase.duration_ms,
            volume: round_vol(vol),
            subtitles,
        })
    }

    /// `numSinglePlayerEvents`: speech streams playing a single-player (mission) event,
    /// `SPEECH_TYPE_SINGLE_PLAYER` (0x1000_0000). Only while one plays are other voices
    /// (NIS and animatic lines, chatter) turned down by `SPEECH_SINGLEPLAYER_RATIO`.
    fn single_player_speaking(&self) -> usize {
        let m = self.mixer();
        (0..crate::mixer::NUM_STREAMS - 1)
            .map(|i| m.stream_handle(i))
            .filter(|&h| !m.stream_over(h) && m.stream_playing_tag(h).is_some_and(|t| t & 0x1000_0000 != 0))
            .count()
    }

    /// Fade out a speech stream and drop what is queued on it.
    pub fn stop_speech(&mut self, stream: usize, fade: f32) {
        let mut m = self.mixer();
        let h = m.stream_handle(stream);
        m.stream_volume(h, -1, fade);
    }

    /// Whether a speech stream is silent.
    pub fn speech_over(&self, stream: usize) -> bool {
        let m = self.mixer();
        m.stream_over(m.stream_handle(stream))
    }

    // --------------------------------------------------------------------------------------------
    // Music
    // --------------------------------------------------------------------------------------------

    fn track_volume(&self, track: i32) -> i32 {
        let Some(music) = &self.data.music else { return 0 };
        let info = music.tracks[track as usize];
        let level = if track == MASTER_LEVEL_TRACK { self.volumes.master } else { self.music_level() };
        crate::mixer::round_i32(f32::from(info.volume) * level)
    }

    fn queue_track(&mut self, track: i32, fade_in: f32) -> bool {
        let Some(music) = &self.data.music else { return false };
        let Some(info) = music.tracks.get(track as usize).copied() else { return false };
        let vol = self.track_volume(track);
        let mut entry = StreamEntry::new(
            StreamData { bytes: music.bytes.clone(), start: info.data_offset, len: info.len },
            vol as i16,
            0,
            u16::from(info.channels),
            info.bitrate,
        );
        entry.looping = info.looping;
        entry.tag = i64::from(track);
        entry.fade_in = fade_in;
        self.music.target = vol & 0xffff;
        let h = self.music.stream;
        self.mixer().stream_queue(h, entry, false)
    }

    /// Play a music track (0..62, or [`event::MUSIC_RANDOM`]). A playing track is faded out
    /// over half a second first, as in the game.
    pub fn play_music(&mut self, track: i32) -> bool {
        let Some(music) = &self.data.music else { return false };
        if track < 0 {
            return false;
        }
        let track = if track == event::MUSIC_RANDOM { (self.rng_music.next_u32() % RANDOM_TRACKS) as i32 } else { track };
        if track as usize >= music.tracks.len() {
            return false;
        }
        let h = self.music.stream;
        if !self.mixer().stream_over(h) {
            self.music.state = MusicState::Switching;
            self.music.target = NO_TARGET;
            self.music.pending = track;
            self.mixer().stream_volume(h, -1, 0.5);
            return true;
        }
        self.music.pending = -1;
        self.music.track = track;
        let ok = self.queue_track(track, 0.0);
        self.music.state = MusicState::Starting;
        ok
    }

    /// Fade out the music (`track` -1 = whatever is playing).
    pub fn stop_music(&mut self, track: i32, fade: f32) -> bool {
        let mut track = track;
        if track == -1 && matches!(self.music.state, MusicState::Playing | MusicState::Starting) {
            track = self.music.track;
        }
        if track != self.music.track {
            return false;
        }
        if matches!(self.music.state, MusicState::Playing | MusicState::Starting) {
            self.music.target = NO_TARGET;
            let h = self.music.stream;
            self.mixer().stream_volume(h, -1, fade);
            self.music.state = MusicState::Stopping;
            self.music.track = -1;
        }
        true
    }

    /// Opt-in: start `track` while the current one fades out, both over `seconds`, using a
    /// second stream. The original only ever plays one track at a time.
    pub fn crossfade_music(&mut self, track: i32, seconds: f32) -> bool {
        let Some(music) = &self.data.music else { return false };
        if track < 0 || track as usize >= music.tracks.len() {
            return false;
        }
        if self.music.spare == NO_HANDLE {
            let h = self.mixer().add_stream();
            self.music.spare = h;
        }
        let old = self.music.stream;
        if !self.mixer().stream_over(old) {
            self.mixer().stream_volume(old, -1, seconds);
        }
        std::mem::swap(&mut self.music.stream, &mut self.music.spare);
        self.music.pending = -1;
        self.music.track = track;
        let ok = self.queue_track(track, seconds.max(0.0));
        self.music.state = MusicState::Starting;
        ok
    }

    /// Track currently selected (-1 for none).
    pub fn current_track(&self) -> i32 {
        self.music.track
    }

    /// Set the music level immediately on the next update instead of fading.
    pub fn music_level_now(&mut self) {
        self.music.immediate = true;
    }

    /// In-game music level for the current track and context (`musicEventUpdateVolume`).
    fn in_game_music_volume(&self) -> i32 {
        let Some(music) = &self.data.music else { return 0 };
        let track = self.music.track;
        let info = music.tracks[track as usize];
        let tv = f32::from(info.volume);
        let ml = self.music_level();
        let t = &self.data.tweaks;
        let ctx = &self.context;
        let mut vol;
        if !ctx.main_screen {
            vol = if ctx.sensors {
                if track < 19 {
                    (tv * t.music_sensors_vol * ml) as i32
                } else {
                    (tv * t.music_minbattle_vol * ml) as i32
                }
            } else if track < 19 {
                (tv * t.music_managers_vol * ml) as i32
            } else if track == 40 {
                crate::mixer::round_i32(tv * ml)
            } else {
                (tv * t.music_minbattle_vol * ml) as i32
            };
            return vol;
        }
        if track == 18 {
            vol = crate::mixer::round_i32(tv * ml * t.music_tutorial_vol);
        } else if track > 24 {
            vol = crate::mixer::round_i32(tv * ml);
        } else {
            let base = crate::mixer::round_i32(tv * ml) as i16 as f32;
            let (active_slope, inactive_slope, battle_slope) = t.music_slopes();
            let d = ctx.closest_distance;
            let f = if t.music_distance_silent < d {
                if t.music_distance_max < d {
                    if track < 19 {
                        t.music_maxgame_vol
                    } else {
                        t.music_maxbattle_vol
                    }
                } else if track < 19 {
                    if ctx.active {
                        (d - t.music_distance_silent) * active_slope + t.music_minactive_vol
                    } else {
                        (d - t.music_distance_silent) * inactive_slope + t.music_mininactive_vol
                    }
                } else {
                    (d - t.music_distance_silent) * battle_slope + t.music_minbattle_vol
                }
            } else if track < 19 {
                if ctx.active {
                    t.music_minactive_vol
                } else {
                    t.music_mininactive_vol
                }
            } else {
                t.music_minbattle_vol
            };
            let v = crate::mixer::round_i32(base * f) as i16;
            vol = if v > i16::from(info.volume) { i32::from(info.volume) } else { i32::from(v.max(0)) };
        }
        if self.letterbox {
            vol = (f64::from(vol as i16) * 0.7).round_ties_even() as i32;
        }
        if let Some(duck) = self.enhancements.duck_music_under_speech {
            let speaking = {
                let m = self.mixer();
                (0..crate::mixer::NUM_STREAMS - 1).any(|i| !m.stream_over(m.stream_handle(i)))
            };
            if speaking {
                vol = (vol as f32 * duck) as i32;
            }
        }
        vol
    }

    fn update_music(&mut self) {
        if self.data.music.is_none() {
            return;
        }
        let h = self.music.stream;
        match self.music.state {
            MusicState::Starting => {
                let (vol, fading, over) = {
                    let m = self.mixer();
                    (m.stream_get_volume(h).map_or(-1, i32::from), m.stream_fading(h), m.stream_over(h))
                };
                if vol == self.music.target {
                    self.music.state = MusicState::Playing;
                } else if !fading && !over {
                    let t = self.music.target as i16;
                    self.mixer().stream_volume(h, t, 0.5);
                }
            }
            MusicState::Playing => {
                if self.mixer().stream_over(h) {
                    self.music.state = MusicState::Idle;
                    let t = self.music.track;
                    self.play_music(t);
                    return;
                }
                let vol = self.in_game_music_volume() as i16;
                let fade = if self.music.immediate {
                    self.music.immediate = false;
                    0.0
                } else {
                    self.data.tweaks.music_fade_time
                };
                self.mixer().stream_volume(h, vol, fade);
            }
            MusicState::Stopping | MusicState::Switching => {
                let (over, fading, vol) = {
                    let m = self.mixer();
                    (m.stream_over(h), m.stream_fading(h), m.stream_get_volume(h).unwrap_or(0))
                };
                if over {
                    if self.music.state == MusicState::Stopping {
                        self.music.state = MusicState::Idle;
                    } else {
                        self.music.track = self.music.pending;
                        self.music.pending = -1;
                        let t = self.music.track;
                        self.queue_track(t, 0.0);
                        self.music.state = MusicState::Starting;
                    }
                    return;
                }
                if self.music.target == NO_TARGET && (fading || vol < 1) {
                    return;
                }
                self.music.target = NO_TARGET;
                self.mixer().stream_volume(h, -1, 0.5);
            }
            MusicState::Idle => {}
        }
    }

    /// The game's per-frame pass over the objects in view, as far as the music level needs it:
    /// sets [`GameContext::closest_distance`] to the nearest derelict or audible ship and
    /// [`GameContext::active`] when a ship that became the nearest so far was moving faster than
    /// `SFX_FLOAT_VELOCITY`. With nothing near, the music plays at its in-game maximum.
    pub fn note_nearby(&mut self, objects: impl IntoIterator<Item = NearbyObject>) {
        let mut closest = 1e21f32;
        let mut active = false;
        for o in objects {
            let d = self.listener.distance(o.position);
            match o.class_row {
                None => closest = closest.min(d),
                Some(row) => {
                    if self.data.tables.in_range(row, d) && d < closest {
                        closest = d;
                        if o.speed > self.data.tweaks.sfx_float_velocity {
                            active = true;
                        }
                    }
                }
            }
        }
        self.context.closest_distance = closest;
        self.context.active = active;
    }

    /// Per-frame update from the game loop: music levels and the mixer's stall detector.
    pub fn update(&mut self, game_running: bool) {
        self.mixer().note_render_frame(game_running);
        self.update_music();
    }
}

fn round_vol(v: f32) -> i16 {
    crate::mixer::round_i32(v) as i16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_levels_are_a_fresh_installs_sliders() {
        let v = Volumes::default();
        assert!((v.music - 0.615).abs() < 1e-6);
        assert!((v.sfx - 0.76875).abs() < 1e-6);
        assert!((v.speech - 0.76875).abs() < 1e-6);
        assert_eq!(v.master, 1.0);
        assert_eq!(slider_level(100), 1.0);
        assert_eq!(slider_level(98), 1.0);
        assert_eq!(slider_level(-5), 0.0);
    }

    #[test]
    fn ran_stream_is_the_game_generator() {
        let mut r = RanStream::default();
        let a: Vec<u32> = (0..3).map(|_| r.next_u32()).collect();
        // First step by hand: y (362436069) <= x + c (521288630), so s = x + c - y - 18.
        let s = 521_288_630u32 - 362_436_069 - 18;
        let n = 1_131_199_209u32.wrapping_mul(69069).wrapping_add(1_013_904_243);
        assert_eq!(a[0], s.wrapping_add(n));
        assert_ne!(a[1], a[2]);
    }
}
