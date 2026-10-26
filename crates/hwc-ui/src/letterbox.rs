//! The NIS letterbox ("scissor window", `NIS.c` / `rndDrawScissorBars`): black bars above and
//! below a 1.85:1 picture during cinematics and fleet intel events.
//!
//! `wideScreenIn(frames)` grows the bars over `frames / 30` seconds: while it moves the bars are
//! `fade * height` tall, grey (0.5) under black of alpha `fade`, so they darken as they grow;
//! once in, the picture is scissored to the middle and the bars are plain black.
//! `wideScreenOut(frames)` shrinks them again. The fade advances in steps of 1/16 s, on the
//! NIS task's clock (real time, not paused with the game).

use crate::canvas::{Canvas, Rect};

/// One NIS task step (`UNIVERSE_UPDATE_PERIOD`).
pub const TICK: f32 = 1.0 / 16.0;
/// Script frames per second (`NIS_FrameRate`).
pub const FRAME_RATE: f32 = 30.0;
/// Bar colour while moving (`MR_LetterboxGrey`).
pub const GREY: u8 = 128;
/// UI sound events of the bars coming in and going out (`UI_Letterbox`, `UI_Unletterbox`).
pub const SOUND_IN: u32 = 0x123;
pub const SOUND_OUT: u32 = 0x124;

/// Height of each bar at a screen size (`NIS_LetterHeight`): what is left of a 1.85:1
/// picture the full width of the screen (none on a screen wider than that, 21:9 and 32:9,
/// where the original's sum goes negative).
pub fn bar_height(width: i32, height: i32) -> i32 {
    ((height - width * 20 / 37) / 2).max(0)
}

/// Height of each bar with [`Letterbox::tall`]: never less than on a 4:3 screen of the same
/// height (14% of it, as the original looks in its 4:3 modes), so the bars still show on a
/// wide screen. The same as [`bar_height`] on 4:3 and narrower screens.
pub fn tall_bar_height(width: i32, height: i32) -> i32 {
    bar_height(width, height).max(bar_height(height * 4 / 3, height))
}

/// The letterbox state (`nisScissorFadeIn/Out/Time`, `nisScissorFade`, `nisFullyScissored`).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Letterbox {
    pub fade_in: f32,
    pub fade_out: f32,
    pub fade_time: f32,
    /// 0..1 while moving, 0 otherwise.
    pub fade: f32,
    /// The bars are fully in.
    pub fully: bool,
    /// The bars are as tall as on a 4:3 screen of the same height ([`tall_bar_height`]; a
    /// user-requested change, the default in the game: on a 16:9 screen the original's 1.85:1
    /// picture leaves bars of only 2% of the height). Off: [`bar_height`], as the original.
    pub tall: bool,
    clock: f32,
}

impl Letterbox {
    /// `kasfWideScreenIn`: returns whether it starts moving (it does nothing when already in;
    /// the caller plays [`SOUND_IN`] when it does).
    pub fn wide_screen_in(&mut self, frames: i32) -> bool {
        if self.fully {
            return false;
        }
        if self.fade_out != 0.0 {
            self.fade_time = 0.0;
            self.fade_out = 0.0;
            self.fade = 0.0;
        }
        self.fade_in = frames as f32 / FRAME_RATE;
        true
    }

    /// `kasfWideScreenOut` (the caller plays [`SOUND_OUT`]).
    pub fn wide_screen_out(&mut self, frames: i32) {
        if self.fade_in != 0.0 {
            self.fade_time = 0.0;
            self.fade_in = 0.0;
            self.fade = 0.0;
        }
        self.fade_out = frames as f32 / FRAME_RATE;
    }

    /// One mission wide-screen call, with its own frame count: `kasfWideScreenIn` and its
    /// Cataclysm variants (`...InDontCloseSensors`, `kasWideScreenInHyperspace`, all the same
    /// body) when `coming_in`, else `kasfWideScreenOut`. Returns the UI sound the call plays:
    /// [`SOUND_IN`] unless the bars are already fully in, [`SOUND_OUT`] always.
    pub fn wide_screen_call(&mut self, coming_in: bool, frames: i32) -> Option<u32> {
        if coming_in {
            self.wide_screen_in(frames).then_some(SOUND_IN)
        } else {
            self.wide_screen_out(frames);
            Some(SOUND_OUT)
        }
    }

    /// `nisScissorBarsReset` (the bar shape stays).
    pub fn reset(&mut self) {
        *self = Letterbox {
            tall: self.tall,
            ..Letterbox::default()
        };
    }

    /// Height of each bar fully in on a `width` x `height` screen.
    pub fn full_height(&self, width: i32, height: i32) -> i32 {
        if self.tall {
            tall_bar_height(width, height)
        } else {
            bar_height(width, height)
        }
    }

    /// One step of `nisUpdateTask`'s scissor part. Returns true on the step the bars are fully in.
    pub fn tick(&mut self) -> bool {
        let mut arrived = false;
        if self.fade_out != 0.0 {
            self.fade_time += TICK;
            self.fully = false;
            if self.fade_time >= self.fade_out {
                self.fade = 0.0;
                self.fade_out = 0.0;
                self.fade_time = 0.0;
            } else {
                self.fade = 1.0 - self.fade_time / self.fade_out;
            }
        }
        if self.fade_in > 0.0 {
            self.fade = self.fade_time / self.fade_in;
            if self.fade_time >= self.fade_in {
                self.fully = true;
                self.fade_in = 0.0;
                self.fade_time = 0.0;
                self.fade = 0.0;
                arrived = true;
            }
            self.fade_time += TICK;
        }
        arrived
    }

    /// Advances by `dt` seconds of real time in whole steps.
    pub fn update(&mut self, dt: f32) -> bool {
        self.clock += dt;
        let mut arrived = false;
        while self.clock >= TICK {
            self.clock -= TICK;
            arrived |= self.tick();
        }
        arrived
    }

    /// Anything to draw (moving or in).
    pub fn active(&self) -> bool {
        self.fade != 0.0 || self.fully
    }

    /// Height of each bar [`draw`](Self::draw) draws on a `width` x `height` screen (0 when
    /// none).
    pub fn drawn_height(&self, width: i32, height: i32) -> i32 {
        let full = self.full_height(width, height);
        if self.fade != 0.0 {
            (full as f32 * self.fade) as i32
        } else if self.fully {
            full
        } else {
            0
        }
    }

    /// The bars over the finished picture.
    pub fn draw(&self, canvas: &mut Canvas) {
        let (w, h) = (canvas.width, canvas.height);
        let full = self.full_height(w, h);
        if self.fade != 0.0 {
            let y = self.drawn_height(w, h);
            let a = (self.fade * 255.0).round().clamp(0.0, 255.0) as u8;
            for r in [Rect::new(0, 0, w, y), Rect::new(0, h - y, w, h)] {
                canvas.fill_rect(r, [GREY, GREY, GREY, 255]);
                canvas.blend_rect(r, [0, 0, 0, a]);
            }
        } else if self.fully {
            for r in [Rect::new(0, 0, w, full), Rect::new(0, h - full, w, h)] {
                canvas.fill_rect(r, [0, 0, 0, 255]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights() {
        assert_eq!(bar_height(640, 480), 67);
        assert_eq!(bar_height(1024, 768), 107);
        // 16:9: the original's bars are thin; the tall ones keep the 4:3 share.
        assert_eq!(bar_height(1600, 900), 18);
        assert_eq!(bar_height(1920, 1080), 21);
        // Wider than 1.85:1: no bars (not a negative height).
        assert_eq!(bar_height(2560, 1080), 0);
        assert_eq!(bar_height(5120, 1440), 0);
        assert_eq!(tall_bar_height(5120, 1440), 201);
        assert_eq!(tall_bar_height(1600, 900), 126);
        assert_eq!(tall_bar_height(1920, 1080), 151);
        assert_eq!(tall_bar_height(640, 480), 67);
        assert_eq!(tall_bar_height(1280, 1024), bar_height(1280, 1024));
        // Any shape and size: the tall bars stay about 14% of the height each (more on
        // screens narrower than 4:3), and the picture between them never vanishes.
        for (w, h) in [
            (1280, 720),
            (1680, 1050),
            (2560, 1080),
            (3440, 1440),
            (3840, 2160),
            (5120, 1440),
            (1080, 1920),
        ] {
            let b = tall_bar_height(w, h);
            assert!(b >= h * 13 / 100 && b < h / 2, "{w}x{h}: {b}");
        }
        let mut l = Letterbox {
            tall: true,
            fully: true,
            ..Letterbox::default()
        };
        assert_eq!(l.drawn_height(1600, 900), 126);
        l.reset();
        assert!(l.tall && !l.fully);
        assert_eq!(Letterbox::default().full_height(1600, 900), 18);
    }

    #[test]
    fn fades_in_and_out() {
        let mut l = Letterbox::default();
        assert!(l.wide_screen_in(90));
        assert_eq!(l.fade_in, 3.0);
        let mut steps = 0;
        while !l.tick() {
            steps += 1;
            assert!(l.fade >= 0.0 && l.fade < 1.0);
        }
        // 3 s at 16 steps a second, plus the step that notices the end.
        assert_eq!(steps, 48);
        assert!(l.fully && l.fade == 0.0);
        assert!(!l.wide_screen_in(90));
        l.wide_screen_out(90);
        l.tick();
        assert!(!l.fully);
        // The step that finished the fade in still advanced the fade time, so the way out
        // starts one step along (as in the original).
        assert!((l.fade - (1.0 - 2.0 / 48.0)).abs() < 1e-5);
        for _ in 0..60 {
            l.tick();
        }
        assert!(!l.active());
    }

    /// Mission calls with their own frame counts: the bars take `frames / 30` s each way, a
    /// call while fully in is silent, and a call out in the middle of the way in turns back
    /// from nothing.
    #[test]
    fn wide_screen_calls_use_their_frame_counts() {
        let mut l = Letterbox::default();
        assert_eq!(l.wide_screen_call(true, 45), Some(SOUND_IN));
        let mut steps = 0;
        while !l.tick() {
            steps += 1;
        }
        // 1.5 s at 16 steps a second.
        assert_eq!(steps, 24);
        assert_eq!(l.wide_screen_call(true, 90), None, "already in: no sound");
        assert!(l.fully);
        assert_eq!(l.wide_screen_call(false, 15), Some(SOUND_OUT));
        let mut steps = 0;
        while l.active() {
            l.tick();
            steps += 1;
        }
        assert!(steps <= 9, "0.5 s out took {steps} steps");
        assert_eq!(l.wide_screen_call(true, 90), Some(SOUND_IN));
        for _ in 0..10 {
            l.tick();
        }
        assert!(l.fade > 0.0 && !l.fully);
        assert_eq!(l.wide_screen_call(false, 90), Some(SOUND_OUT));
        assert_eq!((l.fade_in, l.fade), (0.0, 0.0));
        assert_eq!(l.fade_out, 3.0);
    }

    #[test]
    fn draws_bars() {
        let mut c = Canvas::new(640, 480);
        c.clear([200, 200, 200, 255]);
        let l = Letterbox {
            fully: true,
            ..Letterbox::default()
        };
        l.draw(&mut c);
        assert_eq!(c.get(5, 66), [0, 0, 0, 255]);
        assert_eq!(c.get(5, 67), [200, 200, 200, 255]);
        assert_eq!(c.get(5, 480 - 67), [0, 0, 0, 255]);
        let l = Letterbox {
            fade: 0.5,
            ..Letterbox::default()
        };
        let mut c = Canvas::new(640, 480);
        l.draw(&mut c);
        let p = c.get(5, 10);
        assert!(p[0] > 50 && p[0] < 80, "{p:?}");
    }
}
