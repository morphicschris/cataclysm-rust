//! The letterbox as far as the game rules see it (`nisScissorFadeIn/Out/Time` and
//! `nisFullyScissored`, 0x8f1f90..0x8f1fb0): while the bars are fully in, `ApplyDamageToTarget`
//! hurts no ship that the mission has not marked vulnerable in wide screen. The bars come in
//! over `frames / 30` seconds and stop counting as in as soon as they start going out. The
//! front end draws its own copy (`hwc_ui::letterbox`); this one steps with the universe.

use crate::universe::Universe;

/// One `nisUpdateTask` step.
const TICK: f32 = 1.0 / 16.0;
/// Script frames per second (`NIS_FrameRate`).
const FRAME_RATE: f32 = 30.0;

#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Letterbox {
    /// 0x8f1f90: seconds the bars take to come in (0 when not coming in).
    pub fade_in: f32,
    /// 0x8f1f94: seconds the bars take to go out (0 when not going out).
    pub fade_out: f32,
    /// 0x8f1f98: time into the current fade.
    pub fade_time: f32,
    /// 0x8f1fb0 `nisFullyScissored`: the bars are fully in.
    pub fully: bool,
    /// 0x8f1f24: the bars stay in when the NIS playing now ends (Mission 1's opening).
    pub keep_after_nis: bool,
}

impl Letterbox {
    /// `kasfWideScreenIn` and its variants (`...InDontCloseSensors`,
    /// `kasWideScreenInHyperspace`): nothing when already in.
    pub fn wide_screen_in(&mut self, frames: i32) {
        if self.fully {
            return;
        }
        if self.fade_out != 0.0 {
            self.fade_time = 0.0;
            self.fade_out = 0.0;
        }
        self.fade_in = frames as f32 / FRAME_RATE;
    }

    /// `kasfWideScreenOut`.
    pub fn wide_screen_out(&mut self, frames: i32) {
        if self.fade_in != 0.0 {
            self.fade_time = 0.0;
            self.fade_in = 0.0;
        }
        self.fade_out = frames as f32 / FRAME_RATE;
    }

    /// The scissor part of one `nisUpdateTask` step (0x4c7a30).
    pub fn tick(&mut self) {
        if self.fade_out != 0.0 {
            self.fade_time += TICK;
            self.fully = false;
            if self.fade_time >= self.fade_out {
                self.fade_out = 0.0;
                self.fade_time = 0.0;
            }
        }
        if self.fade_in > 0.0 {
            if self.fade_in <= self.fade_time {
                self.fully = true;
                self.fade_in = 0.0;
                self.fade_time = 0.0;
            }
            self.fade_time += TICK;
        }
    }

    /// `nisUpdateTask` when the NIS it plays ends: the bars no longer count as in, unless
    /// they were to stay (which is then used up).
    pub fn nis_ended(&mut self) {
        if self.keep_after_nis {
            self.keep_after_nis = false;
        } else {
            self.fully = false;
        }
    }
}

/// The `ApplyDamageToTarget` letterbox guard: the bars are in and the ship is neither marked
/// vulnerable in wide screen (`specialFlags2` 0x1000000, `ShipsSetInvincibleInWidescreen`)
/// nor flagged kamikaze (`specialFlags` 2).
pub fn shields(u: &Universe, s: &crate::object::Ship) -> bool {
    u.letterbox.fully && s.special_flags2 & 0x0100_0000 == 0 && s.special_flags & 2 == 0
}
