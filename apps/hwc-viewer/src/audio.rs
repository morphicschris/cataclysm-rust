//! Sound for the sandbox and mission modes through `hwc-audio`: gunfire, hits and explosions from
//! the simulation's events, UI clicks, the mission's music and its speech (whose subtitles and
//! length come back to the mission overlay).
//!
//! The simulation's events say which ship fired which gun; the sound tables need the gun's
//! sound type, which Cataclysm keeps in two gun-static fields `hwc-sim` does not expose yet
//! (+0x04/+0x08, start and fully upgraded). Until it does, the type is derived the Homeworld 1
//! way from the gun size and the weapon technology fitted. Explosion and hit sounds are chosen
//! by ship class here; the original triggers them from the explosion and hit effects.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use glam::Vec3;
use hwc_audio::codec::Limiter;
use hwc_audio::mixer::{Mixer, MixerOptions};
use hwc_audio::system::{event, Emitter, NearbyObject, ShipSounds, SoundSystem, SpeechStarted};
use hwc_audio::tables::Listener;
use hwc_audio::{DataSource, SoundData};
use hwc_formats::gamedata::GameData;
use hwc_sim::statics::{GunSize, ShipClass, WeaponType};
use hwc_sim::universe::SimEvent;
use hwc_sim::{ShipId, Universe};

/// UI events (Homeworld numbering, unchanged in Cataclysm).
pub mod ui {
    pub const CLICK: u32 = 0x108;
    pub const MOVEMENT_GUI_ON: u32 = 0x109;
    pub const MOVEMENT_GUI_OFF: u32 = 0x10a;
    pub const MOVEMENT_GUI_ACCEPT: u32 = 0x116;
}

/// Explosion events (`Exp_DestDamageSmallFast`, `Exp_DestMothership`, `Exp_DestDamageLarge`).
const EXP_SMALL: u32 = event::EXPLOSION;
const EXP_MOTHERSHIP: u32 = event::EXPLOSION + 1;
const EXP_LARGE: u32 = event::EXPLOSION + 2;
/// Hit events (`Hit_Nick`, `Hit_Strike`).
const HIT_NICK: u32 = event::HIT;
const HIT_STRIKE: u32 = event::HIT + 1;

pub struct Audio {
    sys: SoundSystem,
    _output: Option<hwc_audio::output::AudioOutput>,
    ships: HashMap<ShipId, ShipSounds>,
}

/// Homeworld's `GunSoundType` from gun size and weapon technology.
pub fn gun_sound_type(size: GunSize, weapon: Option<WeaponType>) -> usize {
    let by_size = |large: usize, medium: usize, small: usize, very_large: usize| match size {
        GunSize::Large => large,
        GunSize::Medium => medium,
        GunSize::Small => small,
        GunSize::VeryLarge => very_large,
    };
    match weapon {
        Some(WeaponType::Energy) => by_size(0, 4, 10, 14),
        Some(WeaponType::Ion) => by_size(1, 5, 11, 15),
        Some(WeaponType::PlasmaBomb) => by_size(2, 6, 12, 16),
        Some(WeaponType::Mine) => 8,
        Some(WeaponType::Missile) => 9,
        Some(WeaponType::Laser) => 17,
        _ => by_size(3, 7, 13, 3),
    }
}

fn explosion_event(class: Option<ShipClass>) -> u32 {
    match class {
        Some(ShipClass::Fighter | ShipClass::Corvette | ShipClass::Micro) => EXP_SMALL,
        Some(ShipClass::Mothership) => EXP_MOTHERSHIP,
        _ => EXP_LARGE,
    }
}

fn to3(v: Vec3) -> [f32; 3] {
    v.to_array()
}

impl Audio {
    /// Loads the sound data from `game` and, unless `mute`, opens the sound card. With `mute` the
    /// engine still runs (speech subtitles and lengths are needed) but nothing is heard.
    /// `soft_limit` swaps the original's hard 16-bit clamp for a soft knee (opt-in).
    pub fn new(game: GameData, mute: bool, soft_limit: bool) -> Result<Self, String> {
        let data = SoundData::load(&DataSource::from_game_data(game)).map_err(|e| e.to_string())?;
        let opts = MixerOptions {
            limiter: if soft_limit { Limiter::Soft } else { Limiter::Hard },
            ..Default::default()
        };
        let mixer = Arc::new(Mutex::new(Mixer::new(opts)));
        let mut sys = SoundSystem::new(Arc::new(data), mixer.clone());
        if mute {
            // Nothing pulls the mixer, so nothing is heard; speech still reports its subtitles.
            sys.set_sfx_enabled(false);
        }
        let output = if mute {
            None
        } else {
            match hwc_audio::output::AudioOutput::start(mixer, Default::default()) {
                Ok(o) => Some(o),
                Err(e) => {
                    eprintln!("warning: {e}; running without sound");
                    None
                }
            }
        };
        Ok(Self {
            sys,
            _output: output,
            ships: HashMap::new(),
        })
    }

    /// The camera is the listener.
    pub fn set_listener(&mut self, eye: Vec3, look_at: Vec3) {
        self.sys.listener = Listener {
            eye: to3(eye),
            look_at: to3(look_at),
            up: [0.0, 0.0, 1.0],
        };
    }

    /// The in-game music level follows the distance from the camera to the nearest audible
    /// ship (set the listener first).
    pub fn nearby_ships(&mut self, u: &Universe) {
        let objects = u.ship_list.iter().filter_map(|&id| {
            let s = u.ship(id)?;
            let st = u.statics.ship(s.ship_type)?;
            let p = s.posinfo.position;
            let v = s.posinfo.velocity;
            Some(NearbyObject {
                position: [p.x, p.y, p.z],
                class_row: Some(st.ship_class.map_or(0, |c| c.index())),
                speed: (v.x * v.x + v.y * v.y + v.z * v.z).sqrt(),
            })
        });
        self.sys.note_nearby(objects);
    }

    /// Per-frame upkeep (music levels, the mixer's stall detector).
    pub fn frame(&mut self, running: bool) {
        self.sys.update(running);
    }

    pub fn ui(&mut self, ev: u32) {
        let mut none = ShipSounds::default();
        self.sys.play_event(None, ev, &mut none);
    }

    pub fn music(&mut self, track: i32) {
        self.sys.play_music(track);
    }

    /// Speaks a mission speech event on the intel stream.
    pub fn speech(&mut self, ev: u32, variable: i32) -> Option<SpeechStarted> {
        self.sys.context.mission_speech = 1;
        self.sys.play_speech(ev, variable, -1, 0)
    }

    /// Sounds the effect scripts asked for in the last update (`playSound`: the sound plays at
    /// the effect).
    pub fn effect_events(&mut self, u: &Universe) {
        for e in hwc_sim::effects::events(u) {
            if let hwc_fx::FxEvent::Sound { event, position, .. } = *e {
                let emitter = Emitter {
                    position: [position.x, position.y, position.z],
                    is_ship: false,
                    ..Default::default()
                };
                self.sys.play_event(Some(&emitter), event, &mut ShipSounds::default());
            }
        }
    }

    /// Sounds for one universe update's events. `stand_ins`: also play hit and explosion
    /// sounds by ship class (without effects, whose scripts play the original's).
    pub fn sim_events(&mut self, u: &Universe, stand_ins: bool) {
        for e in &u.events {
            match *e {
                SimEvent::GunFired { ship, gun, .. } => {
                    let Some(s) = u.ship(ship) else { continue };
                    let Some(st) = u.statics.ship(s.ship_type) else { continue };
                    let Some(gs) = st.guns.as_ref().and_then(|g| g.get(gun)) else { continue };
                    let weapon = s
                        .guns
                        .get(gun)
                        .and_then(|g| WeaponType::from_index(g.level as usize));
                    let emitter = Emitter {
                        position: [s.posinfo.position.x, s.posinfo.position.y, s.posinfo.position.z],
                        radius: st.header.coll.coll_sphere_size,
                        ship_class: st.ship_class.map_or(0, |c| c.index()),
                        ship_type: s.ship_type.index(),
                        gun_sound_type: gun_sound_type(gs.gun_size, weapon),
                        ..Default::default()
                    };
                    let slot = self.ships.entry(ship).or_default();
                    self.sys.play_event(Some(&emitter), event::GUN_SHOT, slot);
                }
                SimEvent::BulletHit { .. } if !stand_ins => {}
                SimEvent::BulletHit { ship, position, beam, .. } => {
                    let small = u
                        .ship(ship)
                        .and_then(|s| u.statics.ship(s.ship_type))
                        .is_some_and(|st| matches!(st.ship_class, Some(ShipClass::Fighter | ShipClass::Corvette)));
                    let emitter = Emitter {
                        position: [position.x, position.y, position.z],
                        is_ship: false,
                        ..Default::default()
                    };
                    let ev = if small || beam { HIT_NICK } else { HIT_STRIKE };
                    self.sys.play_event(Some(&emitter), ev, &mut ShipSounds::default());
                }
                SimEvent::ShipDied { ship, ship_type, position, .. } => {
                    if let Some(mut slot) = self.ships.remove(&ship) {
                        self.sys.stop_burst(&mut slot);
                    }
                    if !stand_ins {
                        continue;
                    }
                    let class = u.statics.ship(ship_type).and_then(|st| st.ship_class);
                    let emitter = Emitter {
                        position: [position.x, position.y, position.z],
                        is_ship: false,
                        ..Default::default()
                    };
                    self.sys.play_event(Some(&emitter), explosion_event(class), &mut ShipSounds::default());
                }
                SimEvent::Upgraded { .. } => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gun_sound_types_follow_homeworld() {
        assert_eq!(gun_sound_type(GunSize::Small, Some(WeaponType::Projectile)), 13);
        assert_eq!(gun_sound_type(GunSize::Medium, Some(WeaponType::Ion)), 5);
        assert_eq!(gun_sound_type(GunSize::Large, Some(WeaponType::Energy)), 0);
        assert_eq!(gun_sound_type(GunSize::VeryLarge, Some(WeaponType::PlasmaBomb)), 16);
        assert_eq!(gun_sound_type(GunSize::Small, Some(WeaponType::Missile)), 9);
        assert_eq!(gun_sound_type(GunSize::Small, Some(WeaponType::Laser)), 17);
    }

    #[test]
    fn explosions_by_class() {
        assert_eq!(explosion_event(Some(ShipClass::Fighter)), EXP_SMALL);
        assert_eq!(explosion_event(Some(ShipClass::Mothership)), EXP_MOTHERSHIP);
        assert_eq!(explosion_event(Some(ShipClass::Destroyer)), EXP_LARGE);
    }
}
