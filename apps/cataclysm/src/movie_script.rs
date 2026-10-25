//! The soundtrack of a movie: `Movies\<name>.script` (`animLoadNISScript`).
//!
//! None of the shipped `.bik` files carries an audio track. What the player hears during an
//! animatic comes from the movie's script, an NIS event script read with the NIS keyword table
//! and sorted by time: the music, speech and effects levels to use (absolute levels that replace
//! the player's settings while the movie runs, restored afterwards), the music track to start,
//! and the speech events with their subtitles (`animaticSpeechEvent`). Each time the movie
//! player has decoded frames it calls `animBinkDecode(frames decoded)`, which fires every event
//! whose time is at or before `frames / fps` and updates the subtitles on that same clock.
//!
//! Keywords that need a ship of an NIS (`show`, `attack`...) mean nothing here and are
//! ignored, as are the ones no movie uses.

use hwc_formats::nis::NisScript;
use hwc_nis::events::{evaluate, script_seconds};

/// What an event of a movie script does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MovieAction {
    /// `musicVolume`, `speechVolume`, `SFXVolume` (`soundEvent*Vol`): 0..1.
    MusicVolume(f32),
    SpeechVolume(f32),
    SfxVolume(f32),
    /// `startMusic`: the track starts (whatever was playing stops at once).
    StartMusic(i32),
    /// `stopMusic`: fade out over this many seconds.
    StopMusic(f32),
    /// `animaticSpeechEvent` / `speechEvent`: `speechEventQueue` with no object or actor.
    Speech {
        event: u32,
        variable: i32,
    },
    /// `soundEvent`.
    Sound(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MovieEvent {
    /// Seconds from the start of the movie.
    pub time: f32,
    pub action: MovieAction,
}

/// A movie's event list and how far it has been played.
#[derive(Debug, Clone, Default)]
pub struct MovieScript {
    pub events: Vec<MovieEvent>,
    /// The first event not fired yet.
    next: usize,
    /// (line, keyword) of lines that were skipped.
    pub ignored: Vec<(usize, String)>,
}

/// `sscanf("%f")`: the leading number, 0 when there is none.
fn leading_f32(s: &str) -> f32 {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || c == '.' || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().unwrap_or(0.0)
}

/// `sscanf("%d")`.
fn leading_i32(s: &str) -> Option<i32> {
    let s = s.trim_start();
    let end = s
        .char_indices()
        .find(|&(i, c)| !(c.is_ascii_digit() || (i == 0 && (c == '-' || c == '+'))))
        .map_or(s.len(), |(i, _)| i);
    s[..end].parse().ok()
}

/// The speech parsers' arguments: an expression up to the first separator outside brackets
/// (e.g. `(198 + 1048576 + 1073741824)`), evaluated and truncated, then an optional `, var`.
fn speech_args(args: &str) -> (i64, i32) {
    let mut depth = 0;
    let mut end = args.len();
    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = i + 1;
                    break;
                }
            }
            ',' | ' ' | '\t' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    let value = evaluate(&args[..end]).map_or(0, |v| v.trunc() as i64);
    let variable = args[end..].get(1..).and_then(leading_i32).unwrap_or(0);
    (value, variable)
}

impl MovieScript {
    /// Parses a script (Latin-1 bytes).
    pub fn parse(data: &[u8]) -> Result<MovieScript, String> {
        let script = NisScript::parse_bytes(data).map_err(|e| e.to_string())?;
        let mut events = Vec::new();
        let mut ignored = Vec::new();
        for e in &script.events {
            let args = e.args.as_str();
            let action = match e.keyword.as_str() {
                "musicVolume" => MovieAction::MusicVolume(leading_f32(args)),
                "speechVolume" => MovieAction::SpeechVolume(leading_f32(args)),
                "SFXVolume" => MovieAction::SfxVolume(leading_f32(args)),
                "startMusic" => MovieAction::StartMusic(leading_i32(args).unwrap_or(0)),
                "stopMusic" => MovieAction::StopMusic(leading_f32(args)),
                "animaticSpeechEvent" | "speechEvent" => {
                    let (event, variable) = speech_args(args);
                    MovieAction::Speech {
                        event: event as u32,
                        variable,
                    }
                }
                "soundEvent" => MovieAction::Sound(speech_args(args).0 as u32),
                other => {
                    ignored.push((e.line, other.to_string()));
                    continue;
                }
            };
            events.push(MovieEvent {
                time: script_seconds(e.time),
                action,
            });
        }
        // `nisEventSortCB`: by time, then in script order (the sort is stable).
        events.sort_by(|a, b| a.time.total_cmp(&b.time));
        Ok(MovieScript {
            events,
            next: 0,
            ignored,
        })
    }

    /// `animBinkDecode`: the events due by `seconds` into the movie, in order.
    pub fn due(&mut self, seconds: f32) -> Vec<MovieAction> {
        let mut out = Vec::new();
        while let Some(e) = self.events.get(self.next) {
            if seconds < e.time {
                break;
            }
            out.push(e.action);
            self.next += 1;
        }
        out
    }

    /// Time of the last event.
    #[cfg(test)]
    pub fn last_time(&self) -> f32 {
        self.events.last().map_or(0.0, |e| e.time)
    }
}

/// The speech stream a speech event goes to (`speechEventQueue`): Fleet Command and the
/// Tactical Officer on stream 1, the generic voice and the 0x0100_0000 class on stream 2,
/// everyone else on stream 0.
pub fn speech_stream(event: u32) -> usize {
    match event & 0x3f_f800 {
        0x1000 | 0x2000 => 1,
        0x20_0000 => 2,
        _ if event & 0x100_0000 != 0 => 2,
        _ => 0,
    }
}

/// Carries out a script event during a movie (`nisScriptTable` handlers): the sound system
/// gets the levels, music and speech, the subtitle system each subtitled fragment of the
/// speech with its actor (the speech event's actor flags, as in a mission).
pub fn present(
    action: MovieAction,
    audio: &mut Option<crate::audio::Audio>,
    subtitles: Option<&mut hwc_ui::subtitle::Subtitles>,
    mission: u32,
) {
    let Some(a) = audio.as_mut() else { return };
    match action {
        MovieAction::MusicVolume(v) => a.set_music_volume(v),
        MovieAction::SpeechVolume(v) => a.set_speech_volume(v),
        MovieAction::SfxVolume(v) => a.set_sfx_volume(v),
        MovieAction::StartMusic(track) => {
            a.stop_music(0.0);
            a.play_music(track);
        }
        MovieAction::StopMusic(fade) => a.stop_music(fade),
        MovieAction::Sound(event) => a.event(event),
        MovieAction::Speech { event, variable } => {
            let spoken = a.movie_speech(event, variable);
            let actor = hwc_ui::subtitle::actor_for_speech(0, event, true, mission);
            if let (Some(s), Some(actor), Some(subs)) = (spoken, actor, subtitles) {
                let n = s.subtitles.len().max(1) as f32;
                let each = s.duration_ms.max(0) as f32 / 1000.0 / n;
                for t in &s.subtitles {
                    subs.add(actor, event, &hwc_ui::text::to_latin1(t), each);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCRIPT: &str = "#line 1 \"Animatic01.sc\"\n\n\nat frame 0\n      musicVolume     0.7\n        speechVolume    0.9\n        SFXVolume       0.6\n\nat 0.81\n        startMusic 42\n\nat 2.0\n\t\tanimaticSpeechEvent (198 + 1048576 + 1073741824)\n\nat 35.0\n\t\tanimaticSpeechEvent (199 + 8192 + 1073741824), 3\nat 1.0\n        stopMusic 2.5\n        show\n";

    #[test]
    fn parses_and_fires_in_time_order() {
        let mut s = MovieScript::parse(SCRIPT.as_bytes()).unwrap();
        assert_eq!(s.events.len(), 7);
        assert_eq!(s.ignored, vec![(19, "show".to_string())]);
        assert_eq!(
            s.due(1.0 / 30.0),
            vec![
                MovieAction::MusicVolume(0.7),
                MovieAction::SpeechVolume(0.9),
                MovieAction::SfxVolume(0.6)
            ]
        );
        // 0.81 s falls between frames 24 and 25 at 30 fps.
        assert!(s.due(24.0 / 30.0).is_empty());
        assert_eq!(s.due(25.0 / 30.0), vec![MovieAction::StartMusic(42)]);
        // The later `at 1.0` block sorts before `at 2.0`.
        assert_eq!(s.due(1.0), vec![MovieAction::StopMusic(2.5)]);
        assert_eq!(
            s.due(100.0),
            vec![
                MovieAction::Speech {
                    event: 198 + 0x10_0000 + 0x4000_0000,
                    variable: 0
                },
                MovieAction::Speech {
                    event: 199 + 0x2000 + 0x4000_0000,
                    variable: 3
                }
            ]
        );
        assert!(s.due(1000.0).is_empty());
        assert_eq!(s.last_time(), 35.0);
    }

    #[test]
    fn streams() {
        assert_eq!(speech_stream(198 | 0x10_0000 | 0x4000_0000), 0);
        assert_eq!(speech_stream(652 | 0x2000 | 0x4000_0000), 1);
        assert_eq!(speech_stream(520 | 0x1000 | 0x4000_0000), 1);
        assert_eq!(speech_stream(626 | 0x20_0000 | 0x4000_0000), 2);
    }

    use std::rc::Rc;

    use hwc_formats::GameData;
    use hwc_ui::subtitle::{region, Subtitles};
    use hwc_ui::{AssetSource, Canvas, Resources};
    use hwc_video::BinkFile;

    use crate::app::{draw_movie_subtitles, script_name};
    use crate::intro::{Cue, Intro, Step};

    const MOVIES: [&str; 20] = [
        "Animatic01.bik",
        "Animatic02.bik",
        "Animatic03.bik",
        "Animatic04.bik",
        "Animatic05.bik",
        "Animatic06.bik",
        "Animatic07.bik",
        "Animatic08.bik",
        "Animatic09.bik",
        "Animatic10.bik",
        "Animatic11.bik",
        "Animatic12.bik",
        "Animatic13.bik",
        "Animatic14.bik",
        "Animatic15.bik",
        "Animatic16.bik",
        "Animatic17.bik",
        "Animatic18.bik",
        "bdogintro.bik",
        "sierra.bik",
    ];

    fn game() -> Option<Rc<GameData>> {
        match GameData::from_env() {
            Some(Ok(g)) => Some(Rc::new(g)),
            _ => {
                eprintln!("skipped: no game data");
                None
            }
        }
    }

    fn sound(game: &Rc<GameData>) -> Option<crate::audio::Audio> {
        let g = GameData::open(game.root()?).ok()?;
        crate::audio::Audio::open(g, false).ok()
    }

    /// Every movie's sound, checked against its picture: no movie carries a Bink audio track,
    /// so the soundtrack of each animatic is its script. Every script parses, sets its levels
    /// and starts its music at the start, and every speech event in it starts inside the
    /// movie, resolves to a phrase with subtitles and ends by the end of the movie.
    #[test]
    fn every_movie_soundtrack_fits_its_movie() {
        let Some(game) = game() else { return };
        let Some(mut audio) = sound(&game) else {
            eprintln!("skipped: no sound data");
            return;
        };
        let mut speeches = 0;
        for name in MOVIES {
            let path = game
                .movie_path(name)
                .unwrap_or_else(|| panic!("{name} missing"));
            let movie = BinkFile::open(&path).unwrap();
            assert!(
                movie.audio_tracks().is_empty(),
                "{name} has a Bink audio track"
            );
            let length = movie.header().duration() as f32;
            let script = AssetSource::read(&*game, &script_name(name));
            if !name.starts_with("Animatic") {
                // The start-up movies play a music track instead (`binkPlay`'s music).
                assert!(script.is_none(), "{name} has a script");
                continue;
            }
            let data = script.unwrap_or_else(|| panic!("{name}: no script"));
            let script = MovieScript::parse(&data).unwrap();
            assert!(script.ignored.is_empty(), "{name}: {:?}", script.ignored);
            let start: Vec<_> = script
                .events
                .iter()
                .take(4)
                .map(|e| (e.time, e.action))
                .collect();
            assert!(
                matches!(start[0], (0.0, MovieAction::MusicVolume(_))),
                "{name}: {start:?}"
            );
            assert!(
                matches!(start[1], (0.0, MovieAction::SpeechVolume(_))),
                "{name}: {start:?}"
            );
            assert!(
                matches!(start[2], (0.0, MovieAction::SfxVolume(_))),
                "{name}: {start:?}"
            );
            assert!(
                matches!(start[3].1, MovieAction::StartMusic(_)),
                "{name}: {start:?}"
            );
            assert!(
                script.last_time() < length,
                "{name}: last event after the end"
            );
            let mut last_end = 0.0f32;
            for e in &script.events {
                if let MovieAction::Speech { event, variable } = e.action {
                    let s = audio.movie_speech(event, variable).unwrap_or_else(|| {
                        panic!("{name}: speech {event:#x} at {} does not play", e.time)
                    });
                    assert!(
                        !s.subtitles.is_empty(),
                        "{name}: speech {event:#x} has no subtitles"
                    );
                    last_end = last_end.max(e.time + s.duration_ms as f32 / 1000.0);
                    speeches += 1;
                }
            }
            audio.stop_all_speech();
            eprintln!(
                "{name}: {length:.1} s, {} events, speech ends at {last_end:.1} s",
                script.events.len()
            );
            assert!(
                last_end <= length + 1.0,
                "{name}: speech runs {:.1} s past the end",
                last_end - length
            );
        }
        assert_eq!(speeches, 81);
    }

    /// Animatic01 as the game plays it: the levels and music at the start, the first speech at
    /// two seconds with its subtitle over the movie (no band behind it), on the movie clock.
    #[test]
    fn animatic_plays_its_script_with_subtitles() {
        let Some(game) = game() else { return };
        let Some(a) = sound(&game) else {
            eprintln!("skipped: no sound data");
            return;
        };
        let mut audio = Some(a);
        let steps = [Step::Movie {
            file: "Animatic01.bik",
            music: -1,
        }];
        let mut intro = match Intro::new(game.clone(), &steps) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let data = AssetSource::read(&*game, &script_name("Animatic01.bik")).unwrap();
        intro.set_script(MovieScript::parse(&data).unwrap());
        let res = Resources::new(game.clone());
        let mut subs = Subtitles::load(&res, true, &[]).expect("Subtitle.script");
        let mut fired = Vec::new();
        let step = 1.0 / 30.0;
        let mut t = 0.0;
        while t < 3.0 {
            for c in intro.update(step) {
                if let Cue::Script(a) = c {
                    fired.push((intro.script_clock().unwrap(), a));
                    present(a, &mut audio, Some(&mut subs), 0);
                }
            }
            subs.update(intro.script_clock().unwrap());
            t += step;
        }
        let actions: Vec<_> = fired.iter().map(|f| f.1).collect();
        assert_eq!(
            actions,
            [
                MovieAction::MusicVolume(0.7),
                MovieAction::SpeechVolume(0.9),
                MovieAction::SfxVolume(0.6),
                MovieAction::StartMusic(42),
                MovieAction::Speech {
                    event: 198 + 0x10_0000 + 0x4000_0000,
                    variable: 0
                }
            ]
        );
        // On the first frames, then once frame 25 (0.81 s) and frame 60 (2 s) are decoded; the
        // clock is read after the update that fired them (within a frame).
        let frame = 1.0 / 30.0 + 1e-5;
        assert!(fired[0].0 <= 2.0 * frame, "{fired:?}");
        assert!(
            fired[3].0 >= 25.0 / 30.0 && fired[3].0 - 25.0 / 30.0 <= frame,
            "{fired:?}"
        );
        assert!(
            fired[4].0 >= 2.0 - 1e-5 && fired[4].0 - 2.0 <= frame,
            "{fired:?}"
        );
        let volumes = audio.as_ref().unwrap().volumes();
        assert!((volumes.music - 0.7).abs() < 1e-6 && (volumes.speech - 0.9).abs() < 1e-6);
        // The subtitle is up in the letterbox region, without a portrait.
        let r = &subs.regions[region::LETTERBOX];
        assert!(!r.cards.is_empty(), "no subtitle for the first speech");
        assert_eq!(r.picture, None);
        let clock = intro.script_clock().unwrap();
        let mut frame = Canvas::new(1, 1);
        intro.render(&mut frame, hwc_ui::scale::ScaleMode::Original, (1024, 768));
        let mut with = frame.clone();
        draw_movie_subtitles(&mut subs, &mut with, clock);
        // Text was drawn into the region...
        let rect = subs.regions[region::LETTERBOX].rect;
        let changed = (rect.y0..rect.y1)
            .flat_map(|y| (rect.x0..rect.x1).map(move |x| (x, y)))
            .filter(|&(x, y)| with.get(x, y) != frame.get(x, y))
            .count();
        assert!(changed > 50, "no subtitle text drawn ({changed} pixels)");
        // ...and nothing outside it: no band across the top.
        let band = subs.band_rect(with.width);
        let y = (band.y0 + band.y1) / 2;
        for x in [2, with.width - 3] {
            assert_eq!(
                with.get(x, y),
                frame.get(x, y),
                "band drawn over the movie at {x},{y}"
            );
        }
        // In a mission the band is there.
        let mut game_view = frame.clone();
        subs.draw(
            &mut game_view,
            clock,
            None,
            hwc_ui::subtitle::DrawState::default(),
        );
        assert_ne!(game_view.get(2, y), frame.get(2, y));
    }
}
