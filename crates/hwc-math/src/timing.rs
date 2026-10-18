//! Fixed-step timing: the task scheduler's tick accounting and the universe update rate.
//!
//! Facts from `cataclysm.exe`:
//! * `universeUpdateTask` is started with a period of `0x3D800000` = `1/16` s and calls
//!   `univUpdate(1/16)` per step: `UNIVERSE_UPDATE_RATE` is **16 Hz**, as in HW1.
//! * The task timer runs at `UTY_TimerResloutionMax` = **240 Hz**: start-up computes
//!   `divisor = perfFreq / 240` and passes `perfFreq / divisor` to `taskStartup`
//!   (64-bit integer divisions), so the frequency is 240 for any realistic counter rate.
//! * A task's `ticksPerCall` is `__ftol(period * frequency)`; the universe task gets 15.
//! * Each frame, every task accumulates elapsed ticks; it runs `ticks / ticksPerCall` times,
//!   capped at `TSK_MaxTicks` (8), keeping only `ticks % ticksPerCall` as carry (time beyond
//!   the cap is dropped).
//! * New in Cataclysm: a game-speed factor scales the frame's ticks before they are handed
//!   out: `scaled = round_to_nearest_even(ticks * factor)` (the 1.5·2^52 trick), with the
//!   factor taken from `{0.25, 0.5, 1.0, 2.0}` (index 2, normal speed, by default).
//!   `taskTimeDelta` is the scaled ticks over the frequency; a second, unscaled
//!   delta/elapsed pair is kept for real time.

use crate::fpu::{fild_i32, ftol_i32, ld, round_magic, st};

/// `UNIVERSE_UPDATE_RATE`.
pub const UNIVERSE_UPDATE_RATE: u32 = 16;
/// `UNIVERSE_UPDATE_PERIOD` (`1.0f / 16`).
pub const UNIVERSE_UPDATE_PERIOD: f32 = 1.0 / UNIVERSE_UPDATE_RATE as f32;
/// `UTY_TimerResloutionMax`: the task timer's nominal frequency in Hz.
pub const TIMER_RESOLUTION: u32 = 240;
/// `TSK_MaxTicks`: most times a task runs in one frame.
pub const TSK_MAX_TICKS: u32 = 8;
/// Cataclysm game-speed factors (table at 0x721cec).
pub const GAME_SPEED_FACTORS: [f32; 4] = [0.25, 0.5, 1.0, 2.0];
/// Default game-speed index (normal speed).
pub const GAME_SPEED_DEFAULT: usize = 2;

/// The task frequency the game derives from the performance-counter frequency.
pub fn task_frequency(perf_freq: u64) -> u32 {
    let divisor = perf_freq / u64::from(TIMER_RESOLUTION);
    if divisor == 0 {
        // Counters slower than 240 Hz: the original passes the raw frequency through.
        return perf_freq as u32;
    }
    (perf_freq / divisor) as u32
}

/// `taskStart`'s `ticksPerCall = (udword)(period * taskFrequency)`.
pub fn ticks_per_call(period: f32, frequency: f32) -> u32 {
    ftol_i32(ld(period) * ld(frequency)) as u32
}

/// Cataclysm's game-speed tick scaling in `taskExecuteAllPending`.
pub fn scale_ticks(ticks: i32, speed_factor: f32) -> i32 {
    round_magic(fild_i32(ticks) * ld(speed_factor))
}

/// Per-task tick accounting (`taskdata.ticksLeft` / `ticksPerCall`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskTicker {
    pub ticks_per_call: u32,
    pub carry: u32,
    /// `TF_OncePerFrame`: run exactly once per frame regardless of ticks.
    pub once_per_frame: bool,
}

impl TaskTicker {
    pub fn new(ticks_per_call: u32) -> Self {
        Self {
            ticks_per_call,
            carry: 0,
            once_per_frame: false,
        }
    }

    /// Feed one frame's (scaled) ticks; returns how many times the task runs this frame.
    pub fn advance(&mut self, ticks: u32) -> u32 {
        if self.once_per_frame {
            self.carry = self.ticks_per_call.wrapping_sub(1);
            return 1;
        }
        let total = ticks.wrapping_add(self.carry);
        self.carry = total % self.ticks_per_call;
        let calls = total / self.ticks_per_call;
        if (calls as i32) < TSK_MAX_TICKS as i32 {
            calls
        } else {
            TSK_MAX_TICKS
        }
    }
}

/// The scheduler's frame clock: game-speed scaling plus the time accumulators.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TaskClock {
    pub frequency: f32,
    pub speed_index: usize,
    /// `taskTimeDelta`: scaled ticks of the last frame, in seconds.
    pub time_delta: f32,
    /// `taskTimeElapsed`: running sum of `time_delta`.
    pub time_elapsed: f32,
    /// Unscaled (real-time) delta of the last frame.
    pub real_time_delta: f32,
    /// Unscaled (real-time) elapsed sum.
    pub real_time_elapsed: f32,
}

impl TaskClock {
    pub fn new(frequency: u32) -> Self {
        Self {
            frequency: frequency as f32,
            speed_index: GAME_SPEED_DEFAULT,
            time_delta: 0.0,
            time_elapsed: 0.0,
            real_time_delta: 0.0,
            real_time_elapsed: 0.0,
        }
    }

    /// Start of `taskExecuteAllPending(ticks)`: returns the scaled tick count that the
    /// tasks will consume.
    pub fn begin_frame(&mut self, ticks: i32) -> i32 {
        let scaled = scale_ticks(ticks, GAME_SPEED_FACTORS[self.speed_index]);
        self.time_delta = st(fild_i32(scaled) / ld(self.frequency));
        self.time_elapsed = st(ld(self.time_elapsed) + ld(self.time_delta));
        self.real_time_delta = st(fild_i32(ticks) / ld(self.frequency));
        self.real_time_elapsed = st(ld(self.real_time_elapsed) + ld(self.real_time_delta));
        scaled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universe_task_runs_every_15_ticks() {
        assert_eq!(UNIVERSE_UPDATE_PERIOD.to_bits(), 0x3D80_0000);
        assert_eq!(task_frequency(3_579_545), 240);
        assert_eq!(task_frequency(10_000_000), 240);
        assert_eq!(ticks_per_call(UNIVERSE_UPDATE_PERIOD, 240.0), 15);
    }

    #[test]
    fn ticker_caps_and_carries() {
        let mut t = TaskTicker::new(15);
        assert_eq!(t.advance(14), 0);
        assert_eq!(t.advance(1), 1);
        assert_eq!(t.advance(31), 2);
        assert_eq!(t.carry, 1);
        assert_eq!(t.advance(200), 8);
        assert_eq!(t.carry, 201 % 15);
    }

    #[test]
    fn game_speed_scaling_rounds_to_even() {
        assert_eq!(scale_ticks(10, 0.25), 2); // 2.5 -> 2
        assert_eq!(scale_ticks(14, 0.25), 4); // 3.5 -> 4
        assert_eq!(scale_ticks(7, 2.0), 14);
        let mut c = TaskClock::new(240);
        assert_eq!(c.begin_frame(24), 24);
        assert_eq!(c.time_delta, 0.1);
    }
}
