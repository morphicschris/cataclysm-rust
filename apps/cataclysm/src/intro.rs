//! The start-up movies (`utyGameSystemsInit`): before the front end starts, the engine plays
//! `Movies\sierra.bik`, waits two seconds, then plays `Movies\bdogintro.bik`, each with its own
//! music track. Both go through `binkPlay` with no display callback, so the frames are blitted
//! unscaled with GDI: centred in the window, or at the top-left of the 640x480 display mode the
//! engine switches to for the intro when running full screen (the same place). Releasing
//! Escape stops the movie that is playing (`binkStop` from the window procedure); the pause and
//! the other movie still follow. The hidden `/disableAVI` switch skips the whole sequence.

use std::rc::Rc;

use hwc_formats::gamedata::BinkTablesSource;
use hwc_formats::GameData;
use hwc_ui::canvas::{BlitMode, Image};
use hwc_ui::scale::{Placement, ScaleMode};
use hwc_ui::Canvas;
use hwc_video::{BinkFile, BinkTables, VideoDecoder};

/// One step of the start-up sequence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step {
    /// `binkPlay(Movies\<name>, ..., music)`: the track plays for the length of the movie.
    Movie { file: &'static str, music: i32 },
    /// `Sleep(ms)` between the movies: the last picture stays on screen.
    Pause(f32),
}

/// The retail sequence.
pub const SEQUENCE: [Step; 3] = [
    Step::Movie {
        file: "sierra.bik",
        music: 60,
    },
    Step::Pause(2.0),
    Step::Movie {
        file: "bdogintro.bik",
        music: 61,
    },
];

/// `binkPlay` lets the sound system settle for 150 ms before it starts a movie that has music.
const MUSIC_SETTLE: f32 = 0.15;

/// What the sound system has to do as the sequence advances.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cue {
    /// `soundEventPlayMusic(track)` as a movie starts.
    PlayMusic(i32),
    /// `soundEventStopMusic(0)` when a movie ends or is stopped.
    StopMusic,
}

struct Playing {
    file: BinkFile,
    decoder: VideoDecoder,
    /// Seconds since the movie started (after the settle delay).
    time: f64,
    /// Frames decoded so far.
    decoded: u32,
}

/// Plays [`SEQUENCE`]; drive it with [`Intro::update`] and draw it with [`Intro::render`].
pub struct Intro {
    game: Rc<GameData>,
    tables: BinkTables,
    steps: Vec<Step>,
    next: usize,
    /// Time spent in the current pause or settle delay.
    waited: f32,
    /// A movie step whose settle delay is running.
    pending: Option<Step>,
    playing: Option<Playing>,
    /// The picture on screen (the last frame painted).
    picture: Option<Image>,
    rgba: Vec<u8>,
    done: bool,
}

/// Loads the Bink codec tables the importer extracted (or, for an install, reads them from its
/// `binkw32.dll`).
pub fn load_tables(game: &GameData) -> Result<BinkTables, String> {
    match game.bink_tables() {
        Some(BinkTablesSource::Imported(p)) => {
            BinkTables::load(&p).map_err(|e| format!("{}: {e}", p.display()))
        }
        Some(BinkTablesSource::Dll(p)) => std::fs::read(&p)
            .map_err(|e| e.to_string())
            .and_then(|d| BinkTables::from_binkw32(&d).map_err(|e| e.to_string()))
            .map_err(|e| format!("{}: {e}", p.display())),
        None => Err("no Bink codec tables (re-run hwc-import with the game's binkw32.dll)".into()),
    }
}

impl Intro {
    pub fn new(game: Rc<GameData>, steps: &[Step]) -> Result<Intro, String> {
        let tables = load_tables(&game)?;
        Ok(Intro {
            game,
            tables,
            steps: steps.to_vec(),
            next: 0,
            waited: 0.0,
            pending: None,
            playing: None,
            picture: None,
            rgba: Vec::new(),
            done: false,
        })
    }

    pub fn finished(&self) -> bool {
        self.done
    }

    #[cfg(test)]
    /// Whether a movie is on screen now (as opposed to a pause or the settle delay).
    pub fn movie_playing(&self) -> bool {
        self.playing.is_some()
    }

    /// Name and frame number of the picture on screen, for logs and the tour.
    pub fn position(&self) -> Option<(u32, u32)> {
        self.playing
            .as_ref()
            .map(|p| (p.decoded.saturating_sub(1), p.file.frame_count()))
    }

    /// Escape released: `binkStop` ends the movie that is playing; nothing else is skipped.
    pub fn stop_movie(&mut self, cues: &mut Vec<Cue>) {
        if self.playing.take().is_some() {
            cues.push(Cue::StopMusic);
        }
    }

    /// Advances the sequence by `dt` seconds of wall-clock time.
    pub fn update(&mut self, dt: f32) -> Vec<Cue> {
        let mut cues = Vec::new();
        let mut dt = dt;
        while !self.done {
            if let Some(p) = &mut self.playing {
                p.time += f64::from(dt);
                dt = 0.0;
                let h = p.file.header();
                let due = (p.time * f64::from(h.fps_num) / f64::from(h.fps_den.max(1))) as u32;
                let total = p.file.frame_count();
                if due >= total {
                    // The last frame stays on screen.
                    self.decode_to(total);
                    self.playing = None;
                    cues.push(Cue::StopMusic);
                    continue;
                }
                // Behind: decode the missed frames and paint only the newest one.
                self.decode_to(due + 1);
                break;
            }
            if let Some(step) = self.pending {
                self.waited += dt;
                if self.waited < MUSIC_SETTLE {
                    break;
                }
                dt = self.waited - MUSIC_SETTLE;
                self.waited = 0.0;
                self.pending = None;
                if let Step::Movie { file, music } = step {
                    self.start_movie(file, music, &mut cues);
                }
                continue;
            }
            let Some(&step) = self.steps.get(self.next) else {
                self.done = true;
                break;
            };
            match step {
                Step::Pause(secs) => {
                    self.waited += dt;
                    if self.waited < secs {
                        break;
                    }
                    dt = self.waited - secs;
                    self.waited = 0.0;
                    self.next += 1;
                }
                Step::Movie { music, .. } => {
                    self.next += 1;
                    if music >= 0 {
                        self.pending = Some(step);
                        self.waited = 0.0;
                    } else if let Step::Movie { file, music } = step {
                        self.start_movie(file, music, &mut cues);
                    }
                }
            }
        }
        cues
    }

    fn start_movie(&mut self, name: &str, music: i32, cues: &mut Vec<Cue>) {
        // `binkPlay` returns without playing (or starting the music) when the file is missing.
        let opened = self
            .game
            .movie_path(name)
            .ok_or_else(|| format!("{name} not found"))
            .and_then(|p| BinkFile::open(&p).map_err(|e| format!("{}: {e}", p.display())))
            .and_then(|f| {
                let d = f.video_decoder(&self.tables).map_err(|e| e.to_string())?;
                Ok((f, d))
            });
        match opened {
            Ok((file, decoder)) => {
                if music >= 0 {
                    cues.push(Cue::PlayMusic(music));
                }
                self.playing = Some(Playing {
                    file,
                    decoder,
                    time: 0.0,
                    decoded: 0,
                });
                self.decode_to(1);
            }
            Err(e) => eprintln!("cataclysm: intro movie skipped: {e}"),
        }
    }

    /// Decodes frames until `count` have been decoded, then converts the newest to RGBA.
    fn decode_to(&mut self, count: u32) {
        let Some(p) = &mut self.playing else { return };
        let count = count.min(p.file.frame_count());
        if p.decoded >= count {
            return;
        }
        while p.decoded < count {
            let ok = p
                .file
                .packet(p.decoded)
                .and_then(|pk| p.decoder.decode(pk.video));
            if let Err(e) = ok {
                eprintln!("cataclysm: movie frame {}: {e}", p.decoded);
                p.decoded = p.file.frame_count();
                break;
            }
            p.decoded += 1;
        }
        let (w, h) = (p.decoder.width(), p.decoder.height());
        self.rgba.resize(w * h * 4, 0);
        p.decoder.to_rgba(&mut self.rgba);
        self.picture = Some(Image::from_rgba(w as i32, h as i32, &self.rgba));
    }

    /// Draws the picture on a black screen: unscaled and centred in `Original` mode (canvas =
    /// window), otherwise on a 640x480 canvas the presenter scales.
    pub fn render(&self, canvas: &mut Canvas, mode: ScaleMode, window: (u32, u32)) -> Placement {
        let placement = Placement::new(mode, window);
        let (cw, ch) = placement.canvas;
        canvas.resize(cw, ch);
        canvas.clear(hwc_ui::canvas::BLACK);
        if let Some(img) = &self.picture {
            let x = (cw - img.width) / 2;
            let y = (ch - img.height) / 2;
            canvas.blit(img, x, y, BlitMode::Opaque);
        }
        placement
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retail_sequence() {
        assert_eq!(SEQUENCE.len(), 3);
        assert_eq!(
            SEQUENCE[0],
            Step::Movie {
                file: "sierra.bik",
                music: 60
            }
        );
        assert_eq!(SEQUENCE[1], Step::Pause(2.0));
        assert_eq!(
            SEQUENCE[2],
            Step::Movie {
                file: "bdogintro.bik",
                music: 61
            }
        );
    }

    /// Plays the real movies when game data is available (`HWC_DATA_DIR` / `HWC_GAME_DIR`).
    #[test]
    fn plays_the_installed_movies() {
        let Some(Ok(game)) = GameData::from_env() else {
            eprintln!("skipped: no game data");
            return;
        };
        let game = Rc::new(game);
        let mut intro = match Intro::new(game, &SEQUENCE) {
            Ok(i) => i,
            Err(e) => {
                eprintln!("skipped: {e}");
                return;
            }
        };
        let mut cues = Vec::new();
        // Settle delay, then sierra.bik (240 frames at ~15 fps = 16 s).
        cues.extend(intro.update(0.1));
        assert!(!intro.movie_playing());
        cues.extend(intro.update(0.1));
        assert!(intro.movie_playing());
        assert_eq!(cues, [Cue::PlayMusic(60)]);
        cues.clear();
        cues.extend(intro.update(1.0));
        let (frame, total) = intro.position().unwrap();
        assert_eq!(total, 240);
        assert_eq!(frame, 15);
        // Escape: sierra stops, the two-second pause follows, then bdogintro with its track.
        intro.stop_movie(&mut cues);
        assert_eq!(cues, [Cue::StopMusic]);
        cues.clear();
        cues.extend(intro.update(1.0));
        assert!(!intro.movie_playing());
        cues.extend(intro.update(1.2));
        assert!(intro.movie_playing());
        assert_eq!(cues, [Cue::PlayMusic(61)]);
        cues.clear();
        let mut canvas = Canvas::new(1, 1);
        let p = intro.render(&mut canvas, ScaleMode::Original, (1024, 768));
        assert_eq!(p.canvas, (1024, 768));
        // Centred: black border, picture inside.
        assert_eq!(canvas.get(100, 100), [0, 0, 0, 255]);
        for _ in 0..30 {
            cues.extend(intro.update(1.0));
        }
        assert!(intro.finished());
        assert_eq!(cues, [Cue::StopMusic]);
    }
}
