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

/// Height of each bar at a screen size (`NIS_LetterHeight`).
pub fn bar_height(width: i32, height: i32) -> i32 {
    (height - width * 20 / 37) / 2
}

/// The letterbox state (`nisScissorFadeIn/Out/Time`, `nisScissorFade`, `nisFullyScissored`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Letterbox {
    pub fade_in: f32,
    pub fade_out: f32,
    pub fade_time: f32,
    /// 0..1 while moving, 0 otherwise.
    pub fade: f32,
    /// The bars are fully in.
    pub fully: bool,
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

    /// `nisScissorBarsReset`.
    pub fn reset(&mut self) {
        *self = Letterbox::default();
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

    /// The bars over the finished picture.
    pub fn draw(&self, canvas: &mut Canvas) {
        let (w, h) = (canvas.width, canvas.height);
        let full = bar_height(w, h);
        if self.fade != 0.0 {
            let y = (full as f32 * self.fade) as i32;
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
