//! Screen modes and resolutions (a modern addition, user-requested): a window, a borderless
//! window over the whole monitor (the default) or exclusive full screen, with the resolution
//! lists the menus offer for each.
//!
//! The original only knows full screen (`/fullscreen`) or a window (`/window`) at the
//! resolution picked from the display driver's modes (`screenWidth` / `screenHeight` in the
//! registry). Here the mode is the options key [`MODE_KEY`]; the size stays in `screenWidth` /
//! `screenHeight` and the full-screen refresh rate is [`REFRESH_KEY`] (millihertz, 0: the
//! monitor's current rate). A borderless window always covers the monitor at its own
//! resolution: the size is shown there but cannot be picked (the 3D view's internal
//! resolution is the render-scale option's job).
//!
//! The monitor is described by [`Monitor`] (filled by the application from the windowing
//! system); without one (tests, the screenshot tour) every listed size is offered.

use crate::options::Options;

/// The options key of the mode: 0 window, 1 borderless window (default), 2 full screen.
pub const MODE_KEY: &str = "hwcDisplayMode";
/// The options key of the full-screen refresh rate, in millihertz (0: the monitor's current).
pub const REFRESH_KEY: &str = "hwcRefreshRate";

/// The smallest size offered (the original's lowest mode).
pub const MIN_SIZE: (i32, i32) = (640, 480);

/// How the game's window covers the screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// A resizable window with decorations, at the chosen size, centred.
    Windowed,
    /// A borderless window covering the monitor at its own resolution.
    #[default]
    Borderless,
    /// Exclusive full screen at the chosen size and refresh rate.
    Fullscreen,
}

impl Mode {
    pub const ALL: [Mode; 3] = [Mode::Windowed, Mode::Borderless, Mode::Fullscreen];

    /// The options value (an unknown one is the default, borderless).
    pub fn from_option(v: i32) -> Mode {
        match v {
            0 => Mode::Windowed,
            2 => Mode::Fullscreen,
            _ => Mode::Borderless,
        }
    }

    pub fn option_value(self) -> i32 {
        match self {
            Mode::Windowed => 0,
            Mode::Borderless => 1,
            Mode::Fullscreen => 2,
        }
    }

    /// `--display-mode`'s value.
    pub fn parse(s: &str) -> Option<Mode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "windowed" | "window" => Mode::Windowed,
            "borderless" | "borderless-windowed" | "desktop" => Mode::Borderless,
            "fullscreen" | "full-screen" | "exclusive" => Mode::Fullscreen,
            _ => return None,
        })
    }

    /// The row in the main menu's mode list.
    pub fn label(self) -> &'static str {
        match self {
            Mode::Windowed => "Window",
            Mode::Borderless => "Borderless window",
            Mode::Fullscreen => "Full screen",
        }
    }

    /// Covers the whole monitor (borderless or exclusive).
    pub fn is_full(self) -> bool {
        self != Mode::Windowed
    }
}

/// A size, with a refresh rate in millihertz for a full-screen mode (0: none or any).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct VideoMode {
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

impl VideoMode {
    pub const fn new(width: i32, height: i32, refresh_mhz: i32) -> VideoMode {
        VideoMode {
            width,
            height,
            refresh_mhz,
        }
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The refresh rate in whole hertz (0: none).
    pub fn hz(&self) -> i32 {
        (self.refresh_mhz + 500) / 1000
    }

    /// The main menu's narrow list: "1920 x 1080", or with a refresh rate "1920x1080 @ 144Hz".
    pub fn short_label(&self) -> String {
        if self.refresh_mhz > 0 {
            format!("{}x{} @ {}Hz", self.width, self.height, self.hz())
        } else {
            self.label()
        }
    }

    /// "1920 x 1080", or with a refresh rate "1920 x 1080 @ 144 Hz".
    pub fn label(&self) -> String {
        if self.refresh_mhz > 0 {
            format!("{} x {} @ {} Hz", self.width, self.height, self.hz())
        } else {
            format!("{} x {}", self.width, self.height)
        }
    }
}

/// The original's resolutions (its 4:3 and 5:4 modes).
pub const ORIGINAL_RESOLUTIONS: [(i32, i32); 6] = [
    (640, 480),
    (800, 600),
    (1024, 768),
    (1280, 960),
    (1280, 1024),
    (1600, 1200),
];

/// The modern ones: 16:9, 16:10, 21:9 and 32:9, up to 5120x1440.
pub const MODERN_RESOLUTIONS: [(i32, i32); 13] = [
    (1280, 720),
    (1366, 768),
    (1600, 900),
    (1920, 1080),
    (1920, 1200),
    (2560, 1080),
    (2560, 1440),
    (2560, 1600),
    (3440, 1440),
    (3840, 1080),
    (3840, 1600),
    (3840, 2160),
    (5120, 1440),
];

/// Every listed size, by width then height.
pub fn known_resolutions() -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = ORIGINAL_RESOLUTIONS
        .iter()
        .chain(MODERN_RESOLUTIONS.iter())
        .copied()
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The monitor the window is on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Monitor {
    /// Its current (desktop) resolution.
    pub size: (i32, i32),
    /// Its current refresh rate in millihertz (0: unknown).
    pub refresh_mhz: i32,
    /// Its exclusive full-screen modes (any order, duplicates allowed).
    pub modes: Vec<VideoMode>,
}

impl Monitor {
    /// The monitor offers this size in exclusive full screen (at any refresh rate).
    pub fn has_size(&self, size: (i32, i32)) -> bool {
        self.modes.iter().any(|m| m.size() == size)
    }

    /// The best full-screen mode of a size: the refresh rate asked for, else the monitor's
    /// current one, else the highest.
    pub fn best_mode(&self, size: (i32, i32), refresh_mhz: i32) -> Option<VideoMode> {
        let at = |r: i32| {
            self.modes
                .iter()
                .find(|m| m.size() == size && r > 0 && (m.refresh_mhz - r).abs() <= 500)
                .copied()
        };
        at(refresh_mhz)
            .or_else(|| at(self.refresh_mhz))
            .or_else(|| {
                self.modes
                    .iter()
                    .filter(|m| m.size() == size)
                    .max_by_key(|m| m.refresh_mhz)
                    .copied()
            })
    }
}

/// Whether a window of `size` fits on a desktop of `desktop`.
pub fn fits(size: (i32, i32), desktop: (i32, i32)) -> bool {
    size.0 <= desktop.0 && size.1 <= desktop.1
}

/// The resolutions offered in a mode, in list order.
///
/// - Window: every listed size that fits the desktop.
/// - Borderless: the monitor's own resolution only.
/// - Full screen: the listed sizes the monitor has, and its other modes from 640x480 up, each
///   at every refresh rate it has (one per whole hertz, the exact rate kept).
///
/// Without a monitor every listed size, without refresh rates (and nothing for borderless).
pub fn choices(mode: Mode, monitor: Option<&Monitor>) -> Vec<VideoMode> {
    let known = known_resolutions();
    let Some(m) = monitor else {
        return match mode {
            Mode::Borderless => Vec::new(),
            _ => known
                .iter()
                .map(|&(w, h)| VideoMode::new(w, h, 0))
                .collect(),
        };
    };
    match mode {
        Mode::Windowed => known
            .iter()
            .filter(|&&s| fits(s, m.size))
            .map(|&(w, h)| VideoMode::new(w, h, 0))
            .collect(),
        Mode::Borderless => vec![VideoMode::new(m.size.0, m.size.1, m.refresh_mhz.max(0))],
        Mode::Fullscreen => {
            let mut v: Vec<VideoMode> = m
                .modes
                .iter()
                .filter(|v| v.width >= MIN_SIZE.0 && v.height >= MIN_SIZE.1)
                .copied()
                .collect();
            // Highest exact rate first within each whole hertz, then one of each.
            v.sort_unstable_by_key(|v| (v.width, v.height, v.hz(), -v.refresh_mhz));
            v.dedup_by_key(|v| (v.width, v.height, v.hz()));
            v
        }
    }
}

/// The display set up: the mode, the size (of the window, or of the full-screen mode) and the
/// full-screen refresh rate (millihertz, 0: the monitor's current).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Setting {
    pub mode: Mode,
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
}

impl Default for Setting {
    fn default() -> Self {
        Setting::from_options(&Options::default())
    }
}

impl Setting {
    pub fn from_options(o: &Options) -> Setting {
        Setting {
            mode: Mode::from_option(o.get(MODE_KEY)),
            width: o.get("screenWidth"),
            height: o.get("screenHeight"),
            refresh_mhz: o.get(REFRESH_KEY).max(0),
        }
    }

    /// Stores the setting in the options.
    pub fn store(&self, o: &mut Options) {
        o.set(MODE_KEY, self.mode.option_value());
        o.set("screenWidth", self.width);
        o.set("screenHeight", self.height);
        o.set(REFRESH_KEY, self.refresh_mhz);
    }

    pub fn size(&self) -> (i32, i32) {
        (self.width, self.height)
    }

    /// The same display, as the player sees it: a borderless window is the same whatever
    /// size is remembered, a window whatever refresh rate.
    pub fn same_display(&self, other: &Setting) -> bool {
        self.mode == other.mode
            && match self.mode {
                Mode::Borderless => true,
                Mode::Windowed => self.size() == other.size(),
                Mode::Fullscreen => {
                    self.size() == other.size()
                        && (self.refresh_mhz - other.refresh_mhz).abs() <= 500
                }
            }
    }

    /// The row of `list` (from [`choices`]) this setting picks.
    pub fn index_in(&self, list: &[VideoMode]) -> Option<usize> {
        match self.mode {
            Mode::Borderless => (!list.is_empty()).then_some(0),
            Mode::Windowed => list.iter().position(|m| m.size() == self.size()),
            Mode::Fullscreen => list
                .iter()
                .position(|m| {
                    m.size() == self.size()
                        && (self.refresh_mhz == 0
                            || (m.refresh_mhz - self.refresh_mhz).abs() <= 500)
                })
                .or_else(|| list.iter().position(|m| m.size() == self.size())),
        }
    }

    /// Takes a row picked from the list of its mode (a borderless window keeps its size).
    pub fn pick(&mut self, choice: VideoMode) {
        match self.mode {
            Mode::Borderless => {}
            Mode::Windowed => {
                self.width = choice.width;
                self.height = choice.height;
            }
            Mode::Fullscreen => {
                self.width = choice.width;
                self.height = choice.height;
                self.refresh_mhz = choice.refresh_mhz;
            }
        }
    }

    /// What can really be used on this monitor, with a note when it had to change (at start-up
    /// the monitor may not be the one the options were saved on):
    /// - full screen in a size the monitor does not have: a borderless window;
    /// - full screen at a refresh rate it does not have: the size at its current (or best) rate;
    /// - a window larger than the desktop: the largest listed size that fits;
    /// - a size below 640x480: 640x480.
    pub fn validated(&self, monitor: Option<&Monitor>) -> (Setting, Option<String>) {
        let mut s = *self;
        let mut notes = Vec::new();
        if s.width < MIN_SIZE.0 || s.height < MIN_SIZE.1 {
            if s.mode != Mode::Borderless {
                notes.push(format!(
                    "{} x {} is below {} x {}",
                    s.width, s.height, MIN_SIZE.0, MIN_SIZE.1
                ));
            }
            s.width = s.width.max(MIN_SIZE.0);
            s.height = s.height.max(MIN_SIZE.1);
        }
        let Some(m) = monitor else {
            return (s, (!notes.is_empty()).then(|| notes.join("; ")));
        };
        match s.mode {
            Mode::Borderless => {}
            Mode::Windowed => {
                if !fits(s.size(), m.size) {
                    let best = known_resolutions()
                        .into_iter()
                        .filter(|&r| fits(r, m.size))
                        .max_by_key(|&(w, h)| (w * h, w))
                        .unwrap_or(MIN_SIZE);
                    notes.push(format!(
                        "a {} x {} window does not fit the {} x {} desktop: {} x {}",
                        s.width, s.height, m.size.0, m.size.1, best.0, best.1
                    ));
                    s.width = best.0;
                    s.height = best.1;
                }
            }
            Mode::Fullscreen => match m.best_mode(s.size(), s.refresh_mhz) {
                None => {
                    notes.push(format!(
                        "the monitor has no {} x {} full-screen mode: a borderless window instead",
                        s.width, s.height
                    ));
                    s.mode = Mode::Borderless;
                }
                Some(v) => {
                    if s.refresh_mhz > 0 && (v.refresh_mhz - s.refresh_mhz).abs() > 500 {
                        notes.push(format!(
                            "the monitor has no {} Hz at {} x {}: {} Hz",
                            (s.refresh_mhz + 500) / 1000,
                            s.width,
                            s.height,
                            v.hz()
                        ));
                        s.refresh_mhz = v.refresh_mhz;
                    }
                }
            },
        }
        (s, (!notes.is_empty()).then(|| notes.join("; ")))
    }
}

/// Parses `WxH` (also `W x H`, `W*H`).
pub fn parse_size(s: &str) -> Option<(i32, i32)> {
    let s = s.trim().to_ascii_lowercase();
    let (w, h) = s.split_once('x').or_else(|| s.split_once('*'))?;
    let (w, h): (i32, i32) = (w.trim().parse().ok()?, h.trim().parse().ok()?);
    (w > 0 && h > 0 && w <= 16_384 && h <= 16_384).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn monitor() -> Monitor {
        let mut modes = Vec::new();
        for &(w, h) in &[
            (640, 480),
            (800, 600),
            (1024, 768),
            (1280, 720),
            (1920, 1080),
            (2560, 1440),
            (320, 200),
            (1152, 864),
        ] {
            for r in [59_940, 60_000, 143_998, 32_000] {
                modes.push(VideoMode::new(w, h, r));
            }
        }
        Monitor {
            size: (2560, 1440),
            refresh_mhz: 143_998,
            modes,
        }
    }

    #[test]
    fn modes_parse_and_round_trip_through_the_options() {
        assert_eq!(Mode::parse("Windowed"), Some(Mode::Windowed));
        assert_eq!(Mode::parse("window"), Some(Mode::Windowed));
        assert_eq!(Mode::parse("BORDERLESS"), Some(Mode::Borderless));
        assert_eq!(Mode::parse("fullscreen"), Some(Mode::Fullscreen));
        assert_eq!(Mode::parse("exclusive"), Some(Mode::Fullscreen));
        assert_eq!(Mode::parse("tv"), None);
        for m in Mode::ALL {
            assert_eq!(Mode::from_option(m.option_value()), m);
        }
        assert_eq!(Mode::from_option(7), Mode::Borderless);
        // Borderless is the default (a user-requested change).
        let d = Setting::default();
        assert_eq!(d.mode, Mode::Borderless);
        assert_eq!((d.size(), d.refresh_mhz), ((640, 480), 0));
        let mut o = Options::default();
        let s = Setting {
            mode: Mode::Fullscreen,
            width: 5120,
            height: 1440,
            refresh_mhz: 239_760,
        };
        s.store(&mut o);
        assert_eq!(Setting::from_options(&o), s);
        let text = o.to_text();
        assert_eq!(Setting::from_options(&Options::from_text(&text).0), s);
    }

    #[test]
    fn sizes_parse() {
        assert_eq!(parse_size("5120x1440"), Some((5120, 1440)));
        assert_eq!(parse_size("1920 X 1080"), Some((1920, 1080)));
        assert_eq!(parse_size("800*600"), Some((800, 600)));
        for bad in ["", "1920", "x1080", "0x480", "-640x480", "axb", "99999x1"] {
            assert_eq!(parse_size(bad), None, "{bad}");
        }
    }

    #[test]
    fn the_lists_cover_the_original_and_modern_shapes() {
        let k = known_resolutions();
        assert_eq!(k.len(), 19);
        assert_eq!(k.first(), Some(&(640, 480)));
        assert!(k.windows(2).all(|w| w[0] < w[1]));
        for s in [(640, 480), (800, 600), (1024, 768), (1280, 1024)] {
            assert!(k.contains(&s), "{s:?}");
        }
        let aspect = |&(w, h): &(i32, i32)| w as f32 / h as f32;
        for want in [16.0 / 9.0, 16.0 / 10.0, 21.0 / 9.0, 32.0 / 9.0] {
            assert!(k.iter().any(|s| (aspect(s) - want).abs() < 0.06), "{want}");
        }
        assert_eq!(k.iter().map(|s| s.0).max(), Some(5120));
    }

    #[test]
    fn window_sizes_fit_the_desktop() {
        let m = monitor();
        let w = choices(Mode::Windowed, Some(&m));
        assert!(w
            .iter()
            .all(|v| v.width <= 2560 && v.height <= 1440 && v.refresh_mhz == 0));
        assert!(w.iter().any(|v| v.size() == (2560, 1440)));
        assert!(!w.iter().any(|v| v.size() == (2560, 1600)));
        assert!(!w.iter().any(|v| v.size() == (3440, 1440)));
        // Without a monitor, everything.
        assert_eq!(choices(Mode::Windowed, None).len(), 19);
        assert_eq!(choices(Mode::Windowed, None)[1].label(), "800 x 600");
    }

    #[test]
    fn borderless_is_the_desktop() {
        let m = monitor();
        assert_eq!(
            choices(Mode::Borderless, Some(&m)),
            vec![VideoMode::new(2560, 1440, 143_998)]
        );
        assert!(choices(Mode::Borderless, None).is_empty());
        let s = Setting {
            mode: Mode::Borderless,
            width: 800,
            height: 600,
            refresh_mhz: 0,
        };
        assert_eq!(s.index_in(&choices(Mode::Borderless, Some(&m))), Some(0));
        let mut p = s;
        p.pick(VideoMode::new(1920, 1080, 0));
        assert_eq!(p, s, "a borderless window keeps its remembered size");
    }

    #[test]
    fn full_screen_lists_the_monitor_modes_with_refresh_rates() {
        let m = monitor();
        let f = choices(Mode::Fullscreen, Some(&m));
        // 7 sizes from 640x480 up (not 320x200), at 32, 60 and 144 Hz (59.94 and 60 are one).
        assert_eq!(f.len(), 7 * 3);
        assert!(!f.iter().any(|v| v.width < 640));
        // The monitor's own odd size is listed too.
        assert!(f.iter().any(|v| v.size() == (1152, 864)));
        // Not what it lacks.
        assert!(!f.iter().any(|v| v.size() == (5120, 1440)));
        let at60 = f
            .iter()
            .find(|v| v.size() == (1920, 1080) && v.hz() == 60)
            .unwrap();
        assert_eq!(at60.refresh_mhz, 60_000, "the higher exact rate is kept");
        assert_eq!(at60.label(), "1920 x 1080 @ 60 Hz");
        assert_eq!(at60.short_label(), "1920x1080 @ 60Hz");
        let s = Setting {
            mode: Mode::Fullscreen,
            width: 1920,
            height: 1080,
            refresh_mhz: 143_998,
        };
        let i = s.index_in(&f).unwrap();
        assert_eq!(f[i], VideoMode::new(1920, 1080, 143_998));
        let mut p = s;
        p.pick(VideoMode::new(1280, 720, 60_000));
        assert_eq!((p.size(), p.refresh_mhz), ((1280, 720), 60_000));
    }

    #[test]
    fn startup_falls_back_safely() {
        let m = monitor();
        let fs = |w, h, r| Setting {
            mode: Mode::Fullscreen,
            width: w,
            height: h,
            refresh_mhz: r,
        };
        // Fine as it is.
        let (s, note) = fs(1920, 1080, 60_000).validated(Some(&m));
        assert_eq!((s, note), (fs(1920, 1080, 60_000), None));
        // Any rate: the monitor's current.
        let (s, note) = fs(1920, 1080, 0).validated(Some(&m));
        assert_eq!((s, note), (fs(1920, 1080, 0), None));
        assert_eq!(
            m.best_mode((1920, 1080), 0),
            Some(VideoMode::new(1920, 1080, 143_998))
        );
        // A rate it lacks: the current.
        let (s, note) = fs(1920, 1080, 240_000).validated(Some(&m));
        assert_eq!(s.refresh_mhz, 143_998);
        assert!(note.unwrap().contains("240 Hz"));
        // A size it lacks (another monitor): borderless.
        let (s, note) = fs(5120, 1440, 0).validated(Some(&m));
        assert_eq!(s.mode, Mode::Borderless);
        assert!(note.unwrap().contains("borderless"));
        // A window too big for the desktop: the largest listed size that fits.
        let win = Setting {
            mode: Mode::Windowed,
            width: 3840,
            height: 2160,
            refresh_mhz: 0,
        };
        let (s, note) = win.validated(Some(&m));
        assert_eq!(s.size(), (2560, 1440));
        assert!(note.is_some());
        // Nonsense sizes.
        let (s, note) = Setting {
            width: 0,
            height: -5,
            ..win
        }
        .validated(None);
        assert_eq!(s.size(), (640, 480));
        assert!(note.is_some());
        // Borderless never needs a note.
        let (s, note) = Setting::default().validated(Some(&m));
        assert_eq!((s.mode, note), (Mode::Borderless, None));
    }

    #[test]
    fn same_display_ignores_what_a_mode_does_not_use() {
        let a = Setting {
            mode: Mode::Borderless,
            width: 640,
            height: 480,
            refresh_mhz: 0,
        };
        assert!(a.same_display(&Setting {
            width: 1920,
            height: 1080,
            ..a
        }));
        let w = Setting {
            mode: Mode::Windowed,
            ..a
        };
        assert!(w.same_display(&Setting {
            refresh_mhz: 60_000,
            ..w
        }));
        assert!(!w.same_display(&Setting { width: 800, ..w }));
        assert!(!w.same_display(&a));
        let f = Setting {
            mode: Mode::Fullscreen,
            refresh_mhz: 60_000,
            ..a
        };
        assert!(f.same_display(&Setting {
            refresh_mhz: 59_940,
            ..f
        }));
        assert!(!f.same_display(&Setting {
            refresh_mhz: 144_000,
            ..f
        }));
    }
}
