//! The "modern HUD" resource readout: a user-requested change, on by default, in the style of
//! the later Homeworld games ("sleek glow"). Two rows in the top-right corner with no panel
//! behind them: a small spaced-out caps label ("RESOURCES", "SUPPORT UNITS") with the value
//! right-aligned on the same line, and under it a thin rounded bar with a gradient fill and a
//! soft glow. The mission clock sits underneath. RU changes float up as "+150" / "−400";
//! the SU bar pulses while support is at the cap. It replaces the original's SU / RU / time
//! box (bottom right), which comes back with `--hud original` or `hwcModernHud = 0`.
//!
//! It is drawn from the overlay at the point where the original box would be, under the same
//! panel switch, so it shows and hides exactly when that box would (cutscenes, the letterbox,
//! full-screen managers, the per-view overlay options).
//!
//! Text is rendered at the canvas's own resolution from the system's Segoe UI (loaded at run
//! time from the Windows font folder, never shipped) with `ab_glyph`; without it the game's
//! bitmap font is used instead. Glows are CPU blurs: text is rasterised once per string and
//! size into a cached sprite with its blurred mask; a bar's glow is the product of two 1-D
//! Gaussian profiles of its fill (a few multiplications per pixel). Sizes follow the window
//! height, laid out for 1080 lines.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use ab_glyph::{Font as _, FontVec, PxScale, ScaleFont as _};

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::draw;
use crate::ingame::host::HostView;
use crate::ingame::Fe;
use crate::options::Options;

/// The options-file key: 1 (default) the modern readout, 0 the original box.
pub const OPTION_KEY: &str = "hwcModernHud";

/// The RU bar is full at this many RUs and dips below it (RUs have no cap of their own).
pub const RU_BAR_MAX: i32 = 2000;

/// Label colour.
pub const LABEL: Color = [0xFF, 0xD5, 0x4A, 0xFF];
/// Bar fill gradient, left to right end of the fill.
pub const FILL_LEFT: Color = [0xFF, 0x9D, 0x00, 0xFF];
pub const FILL_RIGHT: Color = [0xFF, 0xD8, 0x4A, 0xFF];
/// Bar glow colour.
pub const BAR_GLOW: Color = [0xFF, 0xC8, 0x00, 0xFF];
/// The empty part of a bar.
pub const TRACK: Color = [0, 0, 0, 115];
/// Soft glow around the value text.
pub const VALUE_GLOW: Color = [255, 200, 0, 153];
/// Value text (and its glow) while the support units are at (or over) the cap.
pub const AT_CAP: Color = [0xFF, 0x4A, 0x1E, 0xFF];
const AT_CAP_GLOW: Color = [0xFF, 0x4A, 0x1E, 140];
/// Floating RU changes.
pub const GAIN: Color = [0xC0, 0xFF, 0x48, 0xFF];
pub const LOSS: Color = [0xFF, 0x5A, 0x2A, 0xFF];
const CLOCK: Color = [255, 255, 255, 217];
pub(crate) const SHADOW: Color = [0, 0, 0, 170];

/// How fast a bar eases towards its value (per second; larger is quicker).
pub const TWEEN_RATE: f32 = 10.0;
/// How long an RU change floats, and the window in which further changes of the same sign
/// add to it instead of starting another.
pub const POPUP_SECONDS: f32 = 1.2;
pub const POPUP_MERGE_SECONDS: f32 = 0.5;
const POPUP_MAX: usize = 3;
/// The SU bar's pulse at the cap, seconds per cycle.
pub const PULSE_SECONDS: f32 = 1.6;

/// Reference layout, in pixels at 1080 lines.
const REF_HEIGHT: f32 = 1080.0;
const MARGIN: f32 = 16.0;
const BAR_W: f32 = 230.0;
const BAR_H: f32 = 4.0;
/// Row top to the value's baseline, and to the bar's top.
const BASELINE: f32 = 14.0;
const BAR_TOP: f32 = 21.0;
const ROW_GAP: f32 = 10.0;
/// Bar bottom to the clock's baseline.
const CLOCK_BASELINE: f32 = 16.0;
const LABEL_PX: f32 = 11.0;
const LABEL_SPACING: f32 = 2.0;
const VALUE_PX: f32 = 16.0;
const CLOCK_PX: f32 = 11.0;
const CLOCK_SPACING: f32 = 1.0;
const POPUP_PX: f32 = 13.0;
const POPUP_GAP: f32 = 8.0;
const POPUP_RISE: f32 = 12.0;
/// CSS-style blur sizes (twice the Gaussian's sigma).
const BAR_GLOW_BLUR: f32 = 8.0;
const VALUE_GLOW_BLUR: f32 = 6.0;
/// The bar glow's strength (the 1-D profiles of a 4-pixel bar peak well under 1).
const BAR_GLOW_GAIN: f32 = 1.5;

/// `--hud`: 0 the options file decides, 1 modern, 2 original.
static MODE_OVERRIDE: AtomicU8 = AtomicU8::new(0);

thread_local! {
    /// Frames this thread has drawn the readout (for tests).
    static DRAWN: Cell<u64> = const { Cell::new(0) };
    /// Rasterised text by (string, font, size, spacing, glow).
    static SPRITES: RefCell<HashMap<SpriteKey, Rc<Sprite>>> = RefCell::new(HashMap::new());
}

/// Sets the command line's choice (`None`: the options file decides).
pub fn set_override(modern: Option<bool>) {
    let v = match modern {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    MODE_OVERRIDE.store(v, Ordering::Relaxed);
}

/// Whether the modern readout replaces the original box.
pub fn enabled(options: &Options) -> bool {
    match MODE_OVERRIDE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => options.get_bool(OPTION_KEY),
    }
}

/// Parses `--hud`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "modern" => Some(true),
        "original" => Some(false),
        _ => None,
    }
}

/// How many frames this thread has drawn the readout.
pub fn drawn_count() -> u64 {
    DRAWN.with(Cell::get)
}

/// The RU bar's fill: the RUs held against [`RU_BAR_MAX`], 0..1.
pub fn ru_fill(ru: i32) -> f32 {
    (ru.clamp(0, RU_BAR_MAX) as f32) / RU_BAR_MAX as f32
}

/// The SU bar's fill: units used against the cap, 0..1 (full when there is no cap but
/// something is using support).
pub fn su_fill(used: i32, cap: i32) -> f32 {
    if cap <= 0 {
        return if used > 0 { 1.0 } else { 0.0 };
    }
    (used.max(0) as f32 / cap as f32).clamp(0.0, 1.0)
}

/// Whether the SU value is drawn in the warning colour and the bar pulses.
pub fn su_at_cap(used: i32, cap: i32) -> bool {
    used > 0 && used >= cap
}

/// The pulse's strength at the cap, 0..1, a gentle sine.
pub fn pulse(clock: f32) -> f32 {
    0.5 + 0.5 * (clock * std::f32::consts::TAU / PULSE_SECONDS).sin()
}

/// "1,950": thousands separated by commas.
pub fn thousands(v: i32) -> String {
    let digits = v.unsigned_abs().to_string();
    let mut out = String::new();
    if v < 0 {
        out.push('-');
    }
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// A floating RU change's text: "+150", or "−400" with a real minus sign.
pub fn delta_text(d: i32) -> String {
    if d >= 0 {
        format!("+{}", thousands(d))
    } else {
        format!("\u{2212}{}", thousands(-d))
    }
}

/// One row: the label line and the bar under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// The label's left edge and the value's right edge.
    pub left: i32,
    pub right: i32,
    /// Baseline of the label and the value.
    pub baseline: i32,
    pub bar: Rect,
}

/// Where everything goes on a canvas of a given size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub ru: Row,
    pub su: Row,
    /// The clock's right edge and baseline.
    pub clock_right: i32,
    pub clock_baseline: i32,
    /// Text sizes in pixels and letter spacings.
    pub label_px: f32,
    pub label_spacing: f32,
    pub value_px: f32,
    pub clock_px: f32,
    pub clock_spacing: f32,
    pub popup_px: f32,
    pub popup_gap: i32,
    pub popup_rise: f32,
    /// Gaussian sigmas of the bar and value glows.
    pub bar_sigma: f32,
    pub value_sigma: f32,
    /// First free line under the readout (with a margin), for the panels below it.
    pub bottom: i32,
}

/// Lays the readout out for a `width` x `height` canvas: sizes scale with the height, the
/// corner margin is 16 pixels at 1080 lines.
pub fn layout(width: i32, height: i32) -> Layout {
    let s = (height.max(1) as f32 / REF_HEIGHT).max(0.35);
    let px = |v: f32| (v * s).round() as i32;
    let right = width - px(MARGIN);
    let left = right - px(BAR_W);
    let bar_h = px(BAR_H).max(2);
    let row = |top: i32| {
        let bar_top = top + px(BAR_TOP);
        Row {
            left,
            right,
            baseline: top + px(BASELINE),
            bar: Rect::new(left, bar_top, right, bar_top + bar_h),
        }
    };
    let ru = row(px(MARGIN));
    let su = row(ru.bar.y1 + px(ROW_GAP));
    let clock_baseline = su.bar.y1 + px(CLOCK_BASELINE);
    Layout {
        scale: s,
        ru,
        su,
        clock_right: right,
        clock_baseline,
        label_px: LABEL_PX * s,
        label_spacing: LABEL_SPACING * s,
        value_px: VALUE_PX * s,
        clock_px: CLOCK_PX * s,
        clock_spacing: CLOCK_SPACING * s,
        popup_px: POPUP_PX * s,
        popup_gap: px(POPUP_GAP),
        popup_rise: POPUP_RISE * s,
        bar_sigma: BAR_GLOW_BLUR * s / 2.0,
        value_sigma: VALUE_GLOW_BLUR * s / 2.0,
        bottom: clock_baseline + px(4.0) + px(MARGIN) / 2,
    }
}

/// [`layout`] moved down by the overlay's top (`moSetTop`: below the speech portrait while
/// one is up, sliding back up after), like the original's top-right panels.
pub fn layout_at(width: i32, height: i32, top: i32) -> Layout {
    let mut l = layout(width, height);
    let shift = |r: &mut Row| {
        r.baseline += top;
        r.bar = r.bar.offset(0, top);
    };
    shift(&mut l.ru);
    shift(&mut l.su);
    l.clock_baseline += top;
    l.bottom += top;
    l
}

/// The filled part of a bar for a fraction.
pub fn fill_rect(bar: Rect, fraction: f32) -> Rect {
    let w = (bar.width() as f32 * fraction.clamp(0.0, 1.0)).round() as i32;
    Rect::new(bar.x0, bar.y0, bar.x0 + w, bar.y1)
}

/// "hh:mm:ss", like the original box.
pub fn clock_text(time: f32) -> String {
    let t = time.max(0.0) as u32;
    format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
}

/// A floating RU change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Popup {
    pub amount: i32,
    pub start: f32,
}

/// The readout's animation state, kept in the overlay state: the eased bar fills and the
/// floating RU changes.
#[derive(Debug, Clone, Default)]
pub struct Tween {
    ru: Option<f32>,
    su: Option<f32>,
    last_clock: f32,
    last_ru: Option<i32>,
    pub popups: Vec<Popup>,
}

impl Tween {
    /// Moves the shown fills towards the targets; the first call (or a clock that went back,
    /// i.e. a new mission) jumps straight there.
    pub fn step(&mut self, clock: f32, ru: f32, su: f32) -> (f32, f32) {
        let dt = clock - self.last_clock;
        self.last_clock = clock;
        let k = if dt > 0.0 {
            1.0 - (-dt * TWEEN_RATE).exp()
        } else {
            0.0
        };
        let ease = |cur: &mut Option<f32>, target: f32| -> f32 {
            let v = match *cur {
                Some(c) if dt >= 0.0 => c + (target - c) * k,
                _ => target,
            };
            let v = if (v - target).abs() < 0.001 {
                target
            } else {
                v
            };
            *cur = Some(v);
            v
        };
        (ease(&mut self.ru, ru), ease(&mut self.su, su))
    }

    /// Notes the RUs held now: a change starts a floating "+n" / "−n", or adds to the newest
    /// one when it has the same sign and is under [`POPUP_MERGE_SECONDS`] old. Old ones go.
    pub fn note_ru(&mut self, clock: f32, ru: i32) {
        if self.popups.iter().any(|p| p.start > clock) {
            // The clock went back: a new mission.
            self.popups.clear();
            self.last_ru = None;
        }
        self.popups.retain(|p| clock - p.start < POPUP_SECONDS);
        let Some(last) = self.last_ru.replace(ru) else {
            return;
        };
        let d = ru - last;
        if d == 0 {
            return;
        }
        if let Some(p) = self.popups.last_mut() {
            if clock - p.start < POPUP_MERGE_SECONDS && (p.amount > 0) == (d > 0) {
                p.amount += d;
                p.start = clock;
                return;
            }
        }
        self.popups.push(Popup {
            amount: d,
            start: clock,
        });
        if self.popups.len() > POPUP_MAX {
            self.popups.remove(0);
        }
    }
}

/// How far down the top-right panels (the selected-ship list) start while the readout is
/// up: 0 with the original box.
pub fn top_reserve(fe: &Fe, width: i32, height: i32) -> i32 {
    if enabled(&fe.game.options) {
        layout_at(width, height, fe.game.ig.overlay.top).bottom
    } else {
        0
    }
}

// ---- fonts and cached text sprites ----

pub(crate) struct Fonts {
    pub(crate) bold: FontVec,
    pub(crate) semibold: FontVec,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Face {
    Bold,
    Semibold,
}

fn font_dir() -> std::path::PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    std::path::PathBuf::from(windir).join("Fonts")
}

fn load(name: &str) -> Option<FontVec> {
    let bytes = std::fs::read(font_dir().join(name)).ok()?;
    FontVec::try_from_vec(bytes).ok()
}

/// Segoe UI Bold for the labels and values, Semibold (or Bold) for the clock; None when the
/// system has no Segoe UI.
pub(crate) fn fonts() -> Option<&'static Fonts> {
    static FONTS: OnceLock<Option<Fonts>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let bold = load("segoeuib.ttf")?;
            let semibold = load("seguisb.ttf")
                .or_else(|| load("segoeui.ttf"))
                .or_else(|| load("segoeuib.ttf"))?;
            Some(Fonts { bold, semibold })
        })
        .as_ref()
}

impl Fonts {
    fn face(&self, f: Face) -> &FontVec {
        match f {
            Face::Bold => &self.bold,
            Face::Semibold => &self.semibold,
        }
    }
}

/// `ab_glyph` scales by the line height (ascent to descent); the sizes here are em sizes,
/// like a point size at 96 dpi.
fn em_scale(font: &FontVec, em_px: f32) -> PxScale {
    let upem = font.units_per_em().unwrap_or(2048.0);
    PxScale::from(em_px * font.height_unscaled() / upem)
}

/// Width of `s` in pixels at `px` with `spacing` between letters.
pub(crate) fn text_width(font: &FontVec, px: f32, spacing: f32, s: &str) -> f32 {
    let f = font.as_scaled(em_scale(font, px));
    let mut w = 0.0;
    let mut prev = None;
    for c in s.chars() {
        let g = f.glyph_id(c);
        if let Some(p) = prev {
            w += f.kern(p, g) + spacing;
        }
        w += f.h_advance(g);
        prev = Some(g);
    }
    w
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct SpriteKey {
    text: String,
    face: Face,
    px: u32,
    spacing: u32,
    sigma: u32,
}

/// A string rasterised once: its coverage and, for glowing text, the blurred coverage.
pub(crate) struct Sprite {
    w: i32,
    h: i32,
    /// Where the pen started and the baseline, inside the sprite.
    origin_x: i32,
    baseline: i32,
    pub(crate) advance: i32,
    cov: Vec<f32>,
    glow: Option<Vec<f32>>,
}

impl Sprite {
    fn at(buf: &[f32], w: i32, h: i32, x: i32, y: i32) -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            buf[(y * w + x) as usize]
        }
    }
}

/// A Gaussian kernel of `sigma`, radius 3 sigma, summing to 1.
fn kernel(sigma: f32) -> Vec<f32> {
    let r = (sigma * 3.0).ceil().max(1.0) as i32;
    let k: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    k.into_iter().map(|v| v / sum).collect()
}

/// Separable Gaussian blur of a `w` x `h` mask.
fn blur(src: &[f32], w: i32, h: i32, sigma: f32) -> Vec<f32> {
    let k = kernel(sigma);
    let r = (k.len() / 2) as i32;
    let mut tmp = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (i, kv) in k.iter().enumerate() {
                acc += kv * Sprite::at(src, w, h, x + i as i32 - r, y);
            }
            tmp[(y * w + x) as usize] = acc;
        }
    }
    let mut out = vec![0.0; src.len()];
    for y in 0..h {
        for x in 0..w {
            let mut acc = 0.0;
            for (i, kv) in k.iter().enumerate() {
                acc += kv * Sprite::at(&tmp, w, h, x, y + i as i32 - r);
            }
            out[(y * w + x) as usize] = acc;
        }
    }
    out
}

fn rasterise(font: &FontVec, px: f32, spacing: f32, s: &str, sigma: f32) -> Sprite {
    let scale = em_scale(font, px);
    let f = font.as_scaled(scale);
    let advance = text_width(font, px, spacing, s);
    let pad = if sigma > 0.0 {
        (sigma * 3.0).ceil() as i32 + 2
    } else {
        2
    };
    let asc = f.ascent().ceil() as i32;
    let desc = (-f.descent()).ceil() as i32;
    let w = advance.ceil() as i32 + 2 * pad;
    let h = asc + desc + 2 * pad;
    let baseline = pad + asc;
    let mut cov = vec![0.0f32; (w * h).max(0) as usize];
    let mut pen = pad as f32;
    let mut prev = None;
    for c in s.chars() {
        let id = f.glyph_id(c);
        if let Some(p) = prev {
            pen += f.kern(p, id) + spacing;
        }
        let glyph = id.with_scale_and_position(scale, ab_glyph::point(pen, baseline as f32));
        if let Some(o) = font.outline_glyph(glyph) {
            let b = o.px_bounds();
            let (bx, by) = (b.min.x as i32, b.min.y as i32);
            o.draw(|gx, gy, c| {
                let (x, y) = (bx + gx as i32, by + gy as i32);
                if x >= 0 && y >= 0 && x < w && y < h {
                    let p = &mut cov[(y * w + x) as usize];
                    *p = (*p + c).min(1.0);
                }
            });
        }
        pen += f.h_advance(id);
        prev = Some(id);
    }
    let glow = (sigma > 0.0).then(|| blur(&cov, w, h, sigma));
    Sprite {
        w,
        h,
        origin_x: pad,
        baseline,
        advance: advance.round() as i32,
        cov,
        glow,
    }
}

pub(crate) fn sprite(
    fonts: &Fonts,
    face: Face,
    px: f32,
    spacing: f32,
    s: &str,
    sigma: f32,
) -> Rc<Sprite> {
    let key = SpriteKey {
        text: s.to_string(),
        face,
        px: px.to_bits(),
        spacing: spacing.to_bits(),
        sigma: sigma.to_bits(),
    };
    SPRITES.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(sp) = c.get(&key) {
            return sp.clone();
        }
        // Values and clock ticks come and go: keep the cache small.
        if c.len() > 256 {
            c.clear();
        }
        let sp = Rc::new(rasterise(fonts.face(face), px, spacing, s, sigma));
        c.insert(key, sp.clone());
        sp
    })
}

pub(crate) fn with_alpha(c: Color, a: f32) -> Color {
    [c[0], c[1], c[2], (a.clamp(0.0, 1.0) * 255.0).round() as u8]
}

/// Draws a sprite with its pen start at `x` and its baseline at `baseline`: the glow (if
/// any), a 1-pixel drop shadow, then the text; `fade` scales every layer.
pub(crate) fn draw_sprite(
    canvas: &mut Canvas,
    sp: &Sprite,
    x: i32,
    baseline: i32,
    col: Color,
    glow: Option<Color>,
    fade: f32,
) {
    let (ox, oy) = (x - sp.origin_x, baseline - sp.baseline);
    let alpha = |c: Color| c[3] as f32 / 255.0 * fade;
    for y in 0..sp.h {
        for x in 0..sp.w {
            let (cx, cy) = (ox + x, oy + y);
            if let (Some(g), Some(gc)) = (&sp.glow, glow) {
                let v = g[(y * sp.w + x) as usize];
                if v > 0.004 {
                    canvas.blend(cx, cy, with_alpha(gc, (v * 1.6).min(1.0) * alpha(gc)));
                }
            }
            let sh = Sprite::at(&sp.cov, sp.w, sp.h, x - 1, y - 1);
            if sh > 0.0 {
                canvas.blend(cx, cy, with_alpha(SHADOW, sh * alpha(SHADOW)));
            }
            let v = sp.cov[(y * sp.w + x) as usize];
            if v > 0.0 {
                canvas.blend(cx, cy, with_alpha(col, v * alpha(col)));
            }
        }
    }
}

// ---- bars ----

/// Coverage of a horizontal capsule (rounded ends) spanning `x0..x1`, `y0..y1`, at the
/// pixel centre `(px, py)`.
fn capsule(x0: f32, x1: f32, y0: f32, y1: f32, px: f32, py: f32) -> f32 {
    let r = ((y1 - y0) / 2.0).min((x1 - x0) / 2.0);
    let cy = (y0 + y1) / 2.0;
    let (a, b) = (x0 + r, x1 - r);
    let dx = if px < a {
        a - px
    } else if px > b {
        px - b
    } else {
        0.0
    };
    let dy = (py - cy).abs();
    let d = (dx * dx + dy * dy).sqrt() - r;
    (0.5 - d).clamp(0.0, 1.0)
}

fn lerp(a: Color, b: Color, t: f32) -> Color {
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [m(0), m(1), m(2), 255]
}

/// The bar's fill colour `t` (0..1) along the fill.
pub fn gradient(t: f32) -> Color {
    lerp(FILL_LEFT, FILL_RIGHT, t.clamp(0.0, 1.0))
}

/// The Gaussian-blurred indicator of `a..b`, sampled at the pixel centres `from..to`.
fn profile(a: f32, b: f32, from: i32, to: i32, sigma: f32) -> Vec<f32> {
    // The blurred step is the Gaussian's CDF; erf from Abramowitz & Stegun 7.1.26.
    fn erf(x: f32) -> f32 {
        let s = x.signum();
        let x = x.abs();
        let t = 1.0 / (1.0 + 0.327_591_1 * x);
        let y = 1.0
            - (((((1.061_405_4 * t - 1.453_152_1) * t) + 1.421_413_7) * t - 0.284_496_74) * t
                + 0.254_829_6)
                * t
                * (-x * x).exp();
        s * y
    }
    let k = 1.0 / (sigma * std::f32::consts::SQRT_2);
    (from..to)
        .map(|p| {
            let c = p as f32 + 0.5;
            0.5 * (erf((b - c) * k) - erf((a - c) * k))
        })
        .collect()
}

/// Track, glow and fill of one bar. `glow` scales the glow (the SU pulse).
fn draw_bar(canvas: &mut Canvas, bar: Rect, fraction: f32, sigma: f32, glow: f32) {
    let (x0, x1) = (bar.x0 as f32, bar.x1 as f32);
    let (y0, y1) = (bar.y0 as f32, bar.y1 as f32);
    for y in bar.y0..bar.y1 {
        for x in bar.x0..bar.x1 {
            let c = capsule(x0, x1, y0, y1, x as f32 + 0.5, y as f32 + 0.5);
            if c > 0.0 {
                canvas.blend(x, y, with_alpha(TRACK, c * TRACK[3] as f32 / 255.0));
            }
        }
    }
    let fx1 = x0 + (x1 - x0) * fraction.clamp(0.0, 1.0);
    if fx1 - x0 < 0.5 {
        return;
    }
    // Glow: the product of the fill's blurred horizontal and vertical extents.
    let r = (sigma * 3.0).ceil() as i32;
    let (gx0, gx1) = (bar.x0 - r, fx1.ceil() as i32 + r);
    let (gy0, gy1) = (bar.y0 - r, bar.y1 + r);
    let px = profile(x0, fx1, gx0, gx1, sigma);
    let py = profile(y0, y1, gy0, gy1, sigma);
    for (j, vy) in py.iter().enumerate() {
        for (i, vx) in px.iter().enumerate() {
            let a = (vx * vy * BAR_GLOW_GAIN * glow).min(1.0);
            if a > 0.004 {
                canvas.blend(gx0 + i as i32, gy0 + j as i32, with_alpha(BAR_GLOW, a));
            }
        }
    }
    let len = (fx1 - x0).max(1.0);
    for y in bar.y0..bar.y1 {
        for x in bar.x0..fx1.ceil() as i32 {
            let cx = x as f32 + 0.5;
            let c = capsule(x0, fx1, y0, y1, cx, y as f32 + 0.5);
            if c > 0.0 {
                canvas.blend(x, y, with_alpha(gradient((cx - x0) / len), c));
            }
        }
    }
}

/// Draws the readout (the overlay calls this in place of the original box). `clock` is the
/// overlay's real-time clock, for the easing, the floating changes and the pulse.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas, view: &HostView, clock: f32) {
    DRAWN.with(|c| c.set(c.get() + 1));
    let p = &view.player;
    let r = Readout {
        ru: p.resource_units,
        su_used: p.support_used,
        su_cap: p.support_capacity,
        time: view.time,
    };
    let top = fe.game.ig.overlay.top;
    if paint(canvas, &mut fe.game.ig.overlay.modern_hud, &r, clock, top) {
        return;
    }
    // No Segoe UI: the game's own font, same places, without glows.
    let Some(f) = fe.resources().font(crate::ingame::overlay::FONT_TEXT) else {
        return;
    };
    let l = layout_at(canvas.width, canvas.height, top);
    let h = crate::text::height(&f);
    let su_col = if su_at_cap(r.su_used, r.su_cap) {
        AT_CAP
    } else {
        WHITE
    };
    let rows = [
        (l.ru, "RESOURCES", thousands(r.ru), WHITE),
        (
            l.su,
            "SUPPORT UNITS",
            format!("{} / {}", r.su_used, r.su_cap),
            su_col,
        ),
    ];
    for (row, label, value, col) in &rows {
        let y = row.baseline - h + 2;
        draw::print_shadow(canvas, &f, row.left, y, LABEL, SHADOW, label);
        let w = draw::width(&f, value);
        draw::print_shadow(canvas, &f, row.right - w, y, *col, SHADOW, value);
    }
    let clock_s = clock_text(r.time);
    let cw = draw::width(&f, &clock_s);
    let y = l.clock_baseline - h + 2;
    draw::print_shadow(canvas, &f, l.clock_right - cw, y, WHITE, SHADOW, &clock_s);
}

/// The numbers the readout shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Readout {
    pub ru: i32,
    pub su_used: i32,
    pub su_cap: i32,
    /// Mission time, seconds.
    pub time: f32,
}

/// Draws the bars and, with Segoe UI available, the text, `top` pixels down; false when the
/// text still has to be drawn with the game's font.
pub fn paint(canvas: &mut Canvas, hud: &mut Tween, r: &Readout, clock: f32, top: i32) -> bool {
    let l = layout_at(canvas.width, canvas.height, top);
    let at_cap = su_at_cap(r.su_used, r.su_cap);
    let (ru_f, su_f) = hud.step(clock, ru_fill(r.ru), su_fill(r.su_used, r.su_cap));
    hud.note_ru(clock, r.ru);
    let popups = hud.popups.clone();
    let su_glow = if at_cap {
        0.45 + 0.75 * pulse(clock)
    } else {
        1.0
    };
    draw_bar(canvas, l.ru.bar, ru_f, l.bar_sigma, 1.0);
    draw_bar(canvas, l.su.bar, su_f, l.bar_sigma, su_glow);

    let ru_value = thousands(r.ru);
    let su_value = format!("{} / {}", r.su_used, r.su_cap);
    let (su_col, su_vglow) = if at_cap {
        (AT_CAP, AT_CAP_GLOW)
    } else {
        (WHITE, VALUE_GLOW)
    };
    let clock_s = clock_text(r.time);
    let rows = [
        (l.ru, "RESOURCES", ru_value, WHITE, VALUE_GLOW),
        (l.su, "SUPPORT UNITS", su_value, su_col, su_vglow),
    ];
    if let Some(fonts) = fonts() {
        let mut ru_value_left = l.ru.right;
        for (i, (row, label, value, col, vglow)) in rows.iter().enumerate() {
            let lab = sprite(fonts, Face::Bold, l.label_px, l.label_spacing, label, 0.0);
            draw_sprite(canvas, &lab, row.left, row.baseline, LABEL, None, 1.0);
            let v = sprite(fonts, Face::Bold, l.value_px, 0.0, value, l.value_sigma);
            let vx = row.right - v.advance;
            draw_sprite(canvas, &v, vx, row.baseline, *col, Some(*vglow), 1.0);
            if i == 0 {
                ru_value_left = vx;
            }
        }
        for pop in &popups {
            let t = ((clock - pop.start) / POPUP_SECONDS).clamp(0.0, 1.0);
            let col = if pop.amount >= 0 { GAIN } else { LOSS };
            let sp = sprite(
                fonts,
                Face::Bold,
                l.popup_px,
                0.0,
                &delta_text(pop.amount),
                0.0,
            );
            let x = ru_value_left - l.popup_gap - sp.advance;
            let y = l.ru.baseline - (l.popup_rise * t).round() as i32;
            draw_sprite(canvas, &sp, x, y, col, None, (1.0 - t) * (1.0 - t * 0.3));
        }
        let c = sprite(
            fonts,
            Face::Semibold,
            l.clock_px,
            l.clock_spacing,
            &clock_s,
            0.0,
        );
        draw_sprite(
            canvas,
            &c,
            l.clock_right - c.advance,
            l.clock_baseline,
            CLOCK,
            None,
            1.0,
        );
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layout_at_1080p_keeps_16_px_margins() {
        let l = layout(1920, 1080);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.ru.right, 1904);
        assert_eq!(l.ru.left, 1904 - 230);
        assert_eq!(l.ru.baseline, 30);
        assert_eq!(l.ru.bar, Rect::new(1674, 37, 1904, 41));
        assert_eq!(l.su.bar, Rect::new(1674, 72, 1904, 76));
        assert_eq!(l.su.bar.y0 - l.ru.bar.y1, 31);
        assert_eq!(l.clock_right, 1904);
        assert_eq!(l.clock_baseline, 92);
        assert_eq!((l.label_px, l.label_spacing, l.value_px), (11.0, 2.0, 16.0));
        assert_eq!((l.bar_sigma, l.value_sigma), (4.0, 3.0));
        assert!(
            l.bottom > l.clock_baseline && l.bottom < 120,
            "{}",
            l.bottom
        );
    }

    #[test]
    fn layout_scales_with_the_height() {
        let l = layout(1024, 768);
        let s = 768.0 / 1080.0;
        assert!((l.scale - s).abs() < 1e-6);
        assert_eq!(l.ru.right, 1024 - 11);
        assert_eq!(l.ru.bar.width(), 164);
        assert_eq!(l.ru.bar.height(), 3);
        assert!((l.value_px - 16.0 * s).abs() < 1e-4);
        // 4K doubles everything.
        let k = layout(3840, 2160);
        assert_eq!(k.ru.bar, Rect::new(3840 - 32 - 460, 74, 3840 - 32, 82));
        // A tiny canvas still lays out inside itself, bars at least 2 pixels tall.
        let t = layout(320, 200);
        assert!(t.ru.left > 0 && t.su.bar.y1 < 200 && t.ru.bar.height() >= 2);
    }

    #[test]
    fn layout_moves_below_the_speech_portrait() {
        let a = layout(1920, 1080);
        let b = layout_at(1920, 1080, 75);
        assert_eq!(b.ru.bar, a.ru.bar.offset(0, 75));
        assert_eq!(b.su.baseline, a.su.baseline + 75);
        assert_eq!(b.clock_baseline, a.clock_baseline + 75);
        assert_eq!(b.bottom, a.bottom + 75);
        assert_eq!(layout_at(1920, 1080, 0), a);
    }

    #[test]
    fn ru_fill_is_against_a_fixed_2000() {
        assert_eq!(RU_BAR_MAX, 2000);
        assert_eq!(ru_fill(0), 0.0);
        assert_eq!(ru_fill(1000), 0.5);
        assert_eq!(ru_fill(1950), 0.975);
        assert_eq!(ru_fill(2000), 1.0);
        assert_eq!(ru_fill(5000), 1.0);
        assert_eq!(ru_fill(-10), 0.0);
    }

    #[test]
    fn su_fill_and_cap_warning() {
        assert_eq!(su_fill(8, 20), 0.4);
        assert_eq!(su_fill(20, 20), 1.0);
        assert_eq!(su_fill(30, 20), 1.0);
        assert_eq!(su_fill(0, 0), 0.0);
        assert_eq!(su_fill(3, 0), 1.0);
        assert!(!su_at_cap(8, 20));
        assert!(su_at_cap(20, 20));
        assert!(su_at_cap(21, 20));
        assert!(!su_at_cap(0, 0));
        // The pulse stays within 0..1 and moves.
        let v: Vec<f32> = (0..16).map(|i| pulse(i as f32 * 0.1)).collect();
        assert!(v.iter().all(|p| (0.0..=1.0).contains(p)));
        assert!(
            v.iter().cloned().fold(0.0, f32::max) - v.iter().cloned().fold(1.0, f32::min) > 0.8
        );
    }

    #[test]
    fn number_formats() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(950), "950");
        assert_eq!(thousands(1950), "1,950");
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(-2500), "-2,500");
        assert_eq!(delta_text(150), "+150");
        assert_eq!(delta_text(-400), "\u{2212}400");
        assert_eq!(delta_text(-1500), "\u{2212}1,500");
    }

    #[test]
    fn fill_rect_and_gradient() {
        let bar = Rect::new(100, 10, 300, 14);
        assert_eq!(fill_rect(bar, 0.0), Rect::new(100, 10, 100, 14));
        assert_eq!(fill_rect(bar, 0.4), Rect::new(100, 10, 180, 14));
        assert_eq!(fill_rect(bar, 1.5), bar);
        assert_eq!(gradient(0.0), FILL_LEFT);
        assert_eq!(gradient(1.0), FILL_RIGHT);
        assert_eq!(gradient(0.5), [0xFF, 0xBB, 0x25, 0xFF]);
    }

    #[test]
    fn capsule_has_rounded_ends() {
        // A 4-pixel bar from 0 to 20: solid in the middle, cut at the corners.
        assert_eq!(capsule(0.0, 20.0, 0.0, 4.0, 10.5, 2.5), 1.0);
        assert!(capsule(0.0, 20.0, 0.0, 4.0, 0.5, 0.5) < 0.6);
        assert_eq!(capsule(0.0, 20.0, 0.0, 4.0, 25.0, 2.0), 0.0);
    }

    #[test]
    fn glow_profile_is_a_blurred_step() {
        let p = profile(10.0, 20.0, 0, 30, 4.0);
        assert!(p[15] > 0.7 && p[15] < 1.0);
        assert!((p[10] - 0.5).abs() < 0.1, "{}", p[10]);
        assert!(p[0] < 0.01 && p[29] < 0.01);
        let k = kernel(3.0);
        assert_eq!(k.len(), 19);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn tween_eases_and_snaps_on_a_new_mission() {
        let mut t = Tween::default();
        assert_eq!(t.step(1.0, 1.0, 0.5), (1.0, 0.5));
        let (r, s) = t.step(1.1, 0.0, 0.5);
        assert!(r > 0.0 && r < 1.0, "{r}");
        assert_eq!(s, 0.5);
        for i in 0..100 {
            t.step(1.1 + i as f32 * 0.1, 0.0, 0.5);
        }
        assert_eq!(t.step(12.0, 0.0, 0.5).0, 0.0);
        // The clock went back: straight to the new value.
        assert_eq!(t.step(0.5, 1.0, 0.25), (1.0, 0.25));
    }

    #[test]
    fn ru_changes_float_and_merge() {
        let mut t = Tween::default();
        t.note_ru(1.0, 2000);
        assert!(t.popups.is_empty(), "the first value is not a change");
        t.note_ru(1.1, 1600);
        assert_eq!(
            t.popups,
            vec![Popup {
                amount: -400,
                start: 1.1
            }]
        );
        // Another loss soon after adds to it.
        t.note_ru(1.3, 1500);
        assert_eq!(
            t.popups,
            vec![Popup {
                amount: -500,
                start: 1.3
            }]
        );
        // A gain starts its own.
        t.note_ru(1.4, 1650);
        assert_eq!(t.popups.len(), 2);
        assert_eq!(t.popups[1].amount, 150);
        // Gone after their time.
        t.note_ru(1.4 + POPUP_SECONDS + 0.01, 1650);
        assert!(t.popups.is_empty());
        // Later gains do not merge with an expired one.
        t.note_ru(5.0, 1700);
        t.note_ru(5.0 + POPUP_MERGE_SECONDS + 0.1, 1800);
        assert_eq!(t.popups.len(), 2);
        // A new mission (the clock went back) forgets everything.
        t.note_ru(0.5, 300);
        assert!(t.popups.is_empty());
    }

    #[test]
    fn clock_and_mode_parsing() {
        assert_eq!(clock_text(71.9), "00:01:11");
        assert_eq!(clock_text(3725.0), "01:02:05");
        assert_eq!(parse_mode("Modern"), Some(true));
        assert_eq!(parse_mode("original"), Some(false));
        assert_eq!(parse_mode("x"), None);
    }

    #[test]
    fn modern_hud_is_the_default_option() {
        let o = Options::default();
        assert!(o.get_bool(OPTION_KEY));
        let (back, errors) = Options::from_text("hwcModernHud = 0\n");
        assert!(errors.is_empty());
        assert!(!back.get_bool(OPTION_KEY));
    }

    /// Paints the readout over a bright background: at the cap with a floating loss, and
    /// with a gain; the pictures go to `screenshots/hud/sample_*.png`.
    #[test]
    fn paint_samples() {
        if fonts().is_none() {
            return;
        }
        let dir =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/hud");
        let background = |c: &mut Canvas| {
            for y in 0..c.height {
                for x in 0..c.width {
                    let t = x as f32 / c.width as f32;
                    let v = [(200.0 - 60.0 * t) as u8, (120.0 + 40.0 * t) as u8, 40, 255];
                    c.pixels[(y * c.width + x) as usize] = v;
                }
            }
        };
        let mut c = Canvas::new(1920, 1080);
        background(&mut c);
        let mut t = Tween::default();
        let r = Readout {
            ru: 2000,
            su_used: 20,
            su_cap: 20,
            time: 71.0,
        };
        assert!(paint(&mut c, &mut t, &r, 1.0, 0));
        let r = Readout { ru: 1600, ..r };
        background(&mut c);
        assert!(paint(&mut c, &mut t, &r, 1.3, 0));
        assert_eq!(t.popups.len(), 1);
        let l = layout(1920, 1080);
        // The fill is opaque gradient at the bar's left end.
        let p = c.get(l.ru.bar.x0 + 4, (l.ru.bar.y0 + l.ru.bar.y1) / 2);
        assert_eq!(p[0], 255);
        assert!((150..175).contains(&p[1]) && p[2] < 20, "{p:?}");
        // The track is darker than the background.
        let q = c.get(l.ru.bar.x1 - 3, (l.ru.bar.y0 + l.ru.bar.y1) / 2);
        assert!(q[0] < 160, "{q:?}");
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = c.save_png(&dir.join("sample_cap_and_loss_1920x1080.png"));
            let mut c = Canvas::new(1024, 768);
            background(&mut c);
            let mut t = Tween::default();
            let r = Readout {
                ru: 1800,
                su_used: 8,
                su_cap: 20,
                time: 3725.0,
            };
            paint(&mut c, &mut t, &r, 1.0, 0);
            background(&mut c);
            paint(&mut c, &mut t, &Readout { ru: 1950, ..r }, 1.4, 0);
            let _ = c.save_png(&dir.join("sample_gain_1024x768.png"));
        }
    }

    #[test]
    fn segoe_text_is_cached_with_its_glow() {
        let Some(f) = fonts() else { return };
        let w = text_width(&f.bold, 16.0, 0.0, "1,950");
        assert!(w > 25.0 && w < 50.0, "{w}");
        let spaced = text_width(&f.bold, 11.0, 2.0, "RESOURCES");
        let tight = text_width(&f.bold, 11.0, 0.0, "RESOURCES");
        assert!((spaced - tight - 16.0).abs() < 0.01);
        let a = sprite(f, Face::Bold, 16.0, 0.0, "1,950", 3.0);
        let b = sprite(f, Face::Bold, 16.0, 0.0, "1,950", 3.0);
        assert!(
            Rc::ptr_eq(&a, &b),
            "the second request comes from the cache"
        );
        let g = a.glow.as_ref().expect("glow");
        let cov: f32 = a.cov.iter().sum();
        let glow: f32 = g.iter().sum();
        assert!((cov - glow).abs() / cov < 0.02, "a blur keeps the mass");
        let mut c = Canvas::new(200, 60);
        draw_sprite(&mut c, &a, 20, 30, WHITE, Some(VALUE_GLOW), 1.0);
        assert!(c
            .pixels
            .iter()
            .any(|p| p[0] > 200 && p[1] > 200 && p[2] > 200));
        // The glow tints pixels away from the glyphs gold.
        assert!(c.pixels.iter().any(|p| p[0] > 20 && p[2] < p[0] / 4));
    }
}
