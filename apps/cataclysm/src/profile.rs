//! Frame profile (`--profile` or `HWC_PROFILE=1`): CPU time per named section, GPU time per
//! pass (from the renderer's timestamp queries, when the GPU has them) and per-frame counts
//! (pixels, draws, canvas copies, bytes uploaded), averaged and printed once a second.
//!
//! Sections are timed with [`span`] (a guard that adds its time when dropped) or [`add_ms`];
//! counts with [`count`]; the frame ends with [`frame_end`], which prints the report when a
//! second has passed. Everything is a no-op unless profiling is on.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Instant;

/// 0: not decided yet, 1: off, 2: on.
static STATE: AtomicU8 = AtomicU8::new(0);

/// Turns profiling on (`--profile`).
pub fn enable() {
    STATE.store(2, Ordering::Relaxed);
}

/// Turns profiling off (tests: frames that should not count).
#[cfg(test)]
pub fn disable() {
    STATE.store(1, Ordering::Relaxed);
}

/// Whether profiling is on (`--profile`, or `HWC_PROFILE` set to anything but `0`).
pub fn enabled() -> bool {
    match STATE.load(Ordering::Relaxed) {
        0 => {
            let on = std::env::var_os("HWC_PROFILE").is_some_and(|v| v != "0" && !v.is_empty());
            STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        s => s == 2,
    }
}

#[derive(Debug, Clone, Default)]
struct Stat {
    name: &'static str,
    total: f64,
    max: f64,
    samples: u32,
}

#[derive(Debug, Default)]
struct Profile {
    cpu: Vec<Stat>,
    gpu: Vec<Stat>,
    counts: Vec<(&'static str, u64)>,
    frames: u32,
    since: Option<Instant>,
    frame_start: Option<Instant>,
}

fn add(list: &mut Vec<Stat>, name: &'static str, ms: f64) {
    let s = match list.iter_mut().position(|s| s.name == name) {
        Some(i) => &mut list[i],
        None => {
            list.push(Stat {
                name,
                ..Stat::default()
            });
            list.last_mut().expect("pushed")
        }
    };
    s.total += ms;
    s.max = s.max.max(ms);
    s.samples += 1;
}

thread_local! {
    static PROFILE: RefCell<Profile> = RefCell::new(Profile::default());
}

/// Times a section until dropped.
pub struct Span {
    name: &'static str,
    start: Option<Instant>,
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some(t) = self.start {
            add_ms(self.name, t.elapsed().as_secs_f64() * 1e3);
        }
    }
}

/// Starts timing section `name` (CPU).
pub fn span(name: &'static str) -> Span {
    Span {
        name,
        start: enabled().then(Instant::now),
    }
}

/// Adds `ms` of CPU time to section `name`.
pub fn add_ms(name: &'static str, ms: f64) {
    if !enabled() {
        return;
    }
    PROFILE.with(|p| add(&mut p.borrow_mut().cpu, name, ms));
}

/// Adds `ms` of GPU time to pass `name`.
pub fn gpu_ms(name: &'static str, ms: f64) {
    if !enabled() {
        return;
    }
    PROFILE.with(|p| add(&mut p.borrow_mut().gpu, name, ms));
}

/// Adds `n` to counter `name` (pixels, draws, copies, bytes).
pub fn count(name: &'static str, n: u64) {
    if !enabled() {
        return;
    }
    PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        match p.counts.iter_mut().find(|c| c.0 == name) {
            Some(c) => c.1 += n,
            None => p.counts.push((name, n)),
        }
    });
}

/// Marks the start of a frame (for the whole-frame time).
pub fn frame_start() {
    if !enabled() {
        return;
    }
    PROFILE.with(|p| p.borrow_mut().frame_start = Some(Instant::now()));
}

/// One second's averages: CPU and GPU milliseconds per frame by section, and counts per
/// frame.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub frames: u32,
    /// (section, mean ms per frame, worst single sample in ms).
    pub cpu: Vec<(&'static str, f64, f64)>,
    pub gpu: Vec<(&'static str, f64, f64)>,
    pub counts: Vec<(&'static str, f64)>,
}

impl Report {
    #[cfg(test)]
    pub fn cpu_ms(&self, name: &str) -> Option<f64> {
        self.cpu.iter().find(|c| c.0 == name).map(|c| c.1)
    }

    #[cfg(test)]
    pub fn gpu_ms(&self, name: &str) -> Option<f64> {
        self.gpu.iter().find(|c| c.0 == name).map(|c| c.1)
    }

    #[cfg(test)]
    pub fn count(&self, name: &str) -> Option<f64> {
        self.counts.iter().find(|c| c.0 == name).map(|c| c.1)
    }

    /// One line per kind, `name mean/max` in milliseconds.
    pub fn text(&self) -> String {
        let ms = |v: &[(&str, f64, f64)]| {
            v.iter()
                .map(|(n, m, x)| format!("{n} {m:.2}/{x:.1}"))
                .collect::<Vec<_>>()
                .join("  ")
        };
        let counts = self
            .counts
            .iter()
            .map(|(n, v)| {
                if *v >= 10_000.0 {
                    format!("{n} {:.0}k", v / 1000.0)
                } else {
                    format!("{n} {v:.0}")
                }
            })
            .collect::<Vec<_>>()
            .join("  ");
        let gpu = if self.gpu.is_empty() {
            "(no timestamp queries)".to_string()
        } else {
            ms(&self.gpu)
        };
        format!(
            "profile: {} frames; mean/max ms per frame\n  cpu: {}\n  gpu: {}\n  per frame: {}",
            self.frames,
            ms(&self.cpu),
            gpu,
            counts
        )
    }
}

fn take(p: &mut Profile) -> Report {
    let frames = p.frames.max(1);
    let f = f64::from(frames);
    let means = |v: &[Stat]| v.iter().map(|s| (s.name, s.total / f, s.max)).collect();
    let r = Report {
        frames: p.frames,
        cpu: means(&p.cpu),
        gpu: means(&p.gpu),
        counts: p.counts.iter().map(|&(n, v)| (n, v as f64 / f)).collect(),
    };
    p.cpu.clear();
    p.gpu.clear();
    p.counts.clear();
    p.frames = 0;
    p.since = Some(Instant::now());
    r
}

/// Ends a frame: adds the whole frame's time (from [`frame_start`]) as `frame`, and once a
/// second prints and returns the report.
pub fn frame_end() -> Option<Report> {
    if !enabled() {
        return None;
    }
    PROFILE.with(|p| {
        let mut p = p.borrow_mut();
        if let Some(t) = p.frame_start.take() {
            add(&mut p.cpu, "frame", t.elapsed().as_secs_f64() * 1e3);
        }
        p.frames += 1;
        let since = *p.since.get_or_insert_with(Instant::now);
        if since.elapsed().as_secs_f64() < 1.0 {
            return None;
        }
        let r = take(&mut p);
        eprintln!("{}", r.text());
        Some(r)
    })
}

/// The averages since the last report, without waiting for the second (tests); starts a new
/// window.
#[cfg(test)]
pub fn report_now() -> Report {
    PROFILE.with(|p| take(&mut p.borrow_mut()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sections_average_over_the_frames() {
        enable();
        let _ = report_now();
        for i in 0..4 {
            add_ms("fog", 2.0);
            add_ms("discs", f64::from(i));
            gpu_ms("scene", 1.5);
            count("pixels", 100);
            frame_start();
            let _ = frame_end();
        }
        let r = report_now();
        assert_eq!(r.frames, 4);
        assert_eq!(r.cpu_ms("fog"), Some(2.0));
        assert_eq!(r.cpu_ms("discs"), Some(1.5));
        assert_eq!(r.cpu.iter().find(|c| c.0 == "discs").map(|c| c.2), Some(3.0));
        assert_eq!(r.gpu_ms("scene"), Some(1.5));
        assert_eq!(r.count("pixels"), Some(100.0));
        assert!(r.cpu_ms("frame").is_some());
        assert!(r.text().contains("fog 2.00/2.0"));
        let empty = report_now();
        assert_eq!(empty.frames, 0);
        assert!(empty.cpu.is_empty());
    }
}
