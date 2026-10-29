//! The look the modern managers share ([`super::modern_build_manager`],
//! [`super::modern_research_manager`]): the panel's place and scale, its frame, title, tabs,
//! buttons with their key caps, rows, bars, the scroll bar, text panes, eased hover
//! highlights and the double-click rule. Only drawing and geometry live here; what a control
//! does stays with each manager, which drives the original screen's own code.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};

use crate::canvas::{Canvas, Color, Image, Rect, WHITE};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_panel;
use crate::options::Options;

// ---- the switch ----

/// Whether a modern manager replaces the original's look: the command line's choice
/// (`--build-ui`, `--research-ui`), else its options-file key, which only counts while the
/// modern HUD is on (the original HUD, `--hud original` or `hwcModernHud = 0`, is the
/// global way back to the original screens).
#[derive(Debug)]
pub struct Switch {
    /// 0 the options decide, 1 modern, 2 original.
    over: AtomicU8,
    key: &'static str,
}

impl Switch {
    pub const fn new(key: &'static str) -> Switch {
        Switch {
            over: AtomicU8::new(0),
            key,
        }
    }

    /// The options-file key.
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// Sets the command line's choice; `None`: the options file (and the HUD) decide.
    pub fn set_override(&self, modern: Option<bool>) {
        let v = match modern {
            None => 0,
            Some(true) => 1,
            Some(false) => 2,
        };
        self.over.store(v, Ordering::Relaxed);
    }

    /// The command line's choice, if it made one.
    pub fn overridden(&self) -> Option<bool> {
        match self.over.load(Ordering::Relaxed) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        }
    }

    pub fn enabled(&self, options: &Options) -> bool {
        match self.over.load(Ordering::Relaxed) {
            1 => true,
            2 => false,
            _ => modern_hud::enabled(options) && options.get_bool(self.key),
        }
    }
}

/// Parses `--build-ui` / `--research-ui`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "modern" => Some(true),
        "original" => Some(false),
        _ => None,
    }
}

// ---- colours ----

pub const PANEL: Color = [8, 10, 14, 191];
pub const SECTION: Color = [255, 255, 255, 9];
pub const LINE: Color = [255, 255, 255, 24];
pub const GOLD: Color = obj::GOLD;
pub const LABEL: Color = modern_hud::LABEL;
pub const TEXT: Color = [236, 239, 243, 255];
pub const MUTED: Color = [255, 255, 255, 150];
pub const DIM: Color = [255, 255, 255, 95];
pub const GREYED: Color = [255, 255, 255, 70];
pub const PAUSED: Color = [170, 182, 196, 255];
pub const REFUSED: Color = [0xFF, 0x4A, 0x1E, 0xFF];
/// The "PAUSED" tag.
pub const PAUSED_TAG: Color = [255, 160, 60, 220];
/// Text on a control filled gold (pressed, fired).
pub const ON_GOLD: Color = [20, 16, 6, 255];
/// Over the 3D view around the panel.
pub const SCENE_DIM: Color = [0, 0, 0, 92];
const SHADOW: Color = [0, 0, 0, 255];

/// Hover easing, per second.
pub const HOVER_RATE: f32 = 14.0;
/// The second press of a double click: within this time and distance of the first
/// (as [`crate::fe::FrontEnd::mouse_button`]).
pub const DOUBLE_SECONDS: f64 = 0.5;
pub const DOUBLE_PIXELS: i32 = 4;

// ---- geometry, at 1080 lines ----

/// The panel at 1080 lines.
pub const REF_W: f32 = 960.0;
pub const REF_H: f32 = 760.0;
const REF_LINES: f32 = 1080.0;
/// Never smaller than this scale unless the window is (then it fits the window).
pub const MIN_SCALE: f32 = 0.8;
/// Space kept around the panel when it has to fit the window.
pub const FIT_MARGIN: f32 = 8.0;

pub const PAD: f32 = 24.0;
pub const HEADER_H: f32 = 56.0;
const TITLE_BASELINE: f32 = 35.0;
const TITLE_PX: f32 = 16.0;
const TITLE_SPACING: f32 = 3.0;
pub const TOP_BUTTON_W: f32 = 132.0;
pub const TOP_BUTTON_H: f32 = 30.0;
pub const TOP_BUTTON_Y: f32 = 13.0;
pub const TOP_BUTTON_GAP: f32 = 10.0;
pub const FOOTER_LINE: f32 = 680.0;
pub const BUTTON_Y: f32 = 698.0;
pub const BUTTON_H: f32 = 38.0;
pub const BUTTON_W: f32 = 168.0;
pub const BUTTON_GAP: f32 = 12.0;

/// Text sizes at 1080 lines, and the smallest a label may get.
pub const NAME_PX: f32 = 15.0;
pub const VALUE_PX: f32 = 15.0;
pub const BIG_VALUE_PX: f32 = 18.0;
pub const SMALL_PX: f32 = 11.0;
pub const SMALL_SPACING: f32 = 2.0;
pub const BUTTON_PX: f32 = 12.0;
pub const BUTTON_SPACING: f32 = 1.5;
pub const TITLE_BIG_PX: f32 = 21.0;
pub const BODY_PX: f32 = 13.0;
pub const MIN_TEXT: f32 = 8.5;

/// The panel's scale for a `width` x `height` canvas: the height over 1080, at least
/// [`MIN_SCALE`], and never larger than fits the window.
pub fn scale_for(width: i32, height: i32) -> f32 {
    scale_sized(width, height, REF_W, REF_H, top_band(height))
}

/// The scale of a `ref_w` x `ref_h` panel (at 1080 lines) on a `width` x `height` canvas
/// with `band` pixels kept clear along the top: as [`scale_for`].
pub fn scale_sized(width: i32, height: i32, ref_w: f32, ref_h: f32, band: i32) -> f32 {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let fit = ((h - band as f32 - FIT_MARGIN) / ref_h).min((w - 2.0 * FIT_MARGIN) / ref_w);
    (h / REF_LINES).max(MIN_SCALE).min(fit).max(0.2)
}

/// The band along the top kept for the modern HUD's panel headers (objectives, resources),
/// which stay up over the managers: its corner margin and header height, and a little air.
pub fn top_band(height: i32) -> i32 {
    let s = modern_hud::scale_for(height);
    ((modern_panel::MARGIN + obj::HEADER_H) * s).round() as i32 + 4
}

/// The panel, its header and footer line, placed and scaled for a canvas.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub scale: f32,
    pub panel: Rect,
    pub header: Rect,
    pub title_x: i32,
    pub title_baseline: i32,
    pub footer_line: i32,
    /// The panel's size at 1080 lines ([`REF_W`] x [`REF_H`] for the managers).
    pub ref_w: f32,
    pub ref_h: f32,
    /// It has a footer (its hairline and buttons along the bottom).
    pub footer: bool,
}

impl Frame {
    /// The managers' panel on a `width` x `height` canvas: centred, below the HUD's header
    /// band.
    pub fn new(width: i32, height: i32) -> Frame {
        Frame::sized(width, height, REF_W, REF_H, top_band(height), true)
    }

    /// A `ref_w` x `ref_h` panel (at 1080 lines), centred, below `band` pixels along the
    /// top, with or without a footer.
    pub fn sized(
        width: i32,
        height: i32,
        ref_w: f32,
        ref_h: f32,
        band: i32,
        footer: bool,
    ) -> Frame {
        let s = scale_sized(width, height, ref_w, ref_h, band);
        let px = |v: f32| (v * s).round() as i32;
        let (pw, ph) = (px(ref_w), px(ref_h));
        let x0 = (width - pw) / 2;
        let y0 = ((height - ph) / 2).max(band);
        let panel = Rect::new(x0, y0, x0 + pw, y0 + ph);
        Frame {
            scale: s,
            panel,
            header: Rect::new(panel.x0, panel.y0, panel.x1, y0 + px(HEADER_H)),
            title_x: x0 + px(PAD),
            title_baseline: y0 + px(TITLE_BASELINE),
            footer_line: y0 + px(ref_h - (REF_H - FOOTER_LINE)),
            ref_w,
            ref_h,
            footer,
        }
    }

    /// `v` reference pixels at this scale.
    pub fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    /// A text size: `v` at 1080 lines, never under [`MIN_TEXT`].
    pub fn text(&self, v: f32) -> f32 {
        (v * self.scale).max(MIN_TEXT.min(v))
    }

    /// A rectangle in reference pixels from the panel's corner.
    pub fn at(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        let (x0, y0) = (self.panel.x0, self.panel.y0);
        Rect::new(
            x0 + self.px(x),
            y0 + self.px(y),
            x0 + self.px(x + w),
            y0 + self.px(y + h),
        )
    }

    /// The `i`-th header button from the right.
    pub fn top_button(&self, i: usize) -> Rect {
        let x = self.ref_w - PAD - TOP_BUTTON_W - i as f32 * (TOP_BUTTON_W + TOP_BUTTON_GAP);
        self.at(x, TOP_BUTTON_Y, TOP_BUTTON_W, TOP_BUTTON_H)
    }

    /// The footer buttons' top, in reference pixels from the panel's top.
    pub fn footer_y(&self) -> f32 {
        self.ref_h - (REF_H - BUTTON_Y)
    }

    /// The `i`-th footer button from the left.
    pub fn footer_button(&self, i: usize) -> Rect {
        let x = PAD + i as f32 * (BUTTON_W + BUTTON_GAP);
        self.at(x, self.footer_y(), BUTTON_W, BUTTON_H)
    }

    /// The footer button on the right (Close).
    pub fn footer_right(&self) -> Rect {
        self.at(
            self.ref_w - PAD - BUTTON_W,
            self.footer_y(),
            BUTTON_W,
            BUTTON_H,
        )
    }

    /// The panel's inner right edge.
    pub fn inner_x1(&self) -> i32 {
        self.panel.x1 - self.px(PAD)
    }
}

// ---- state shared by the managers ----

/// Eased hover / press highlights, 0..1, by control.
#[derive(Debug, Clone)]
pub struct Hover<K> {
    values: BTreeMap<K, f32>,
    last_time: Option<f64>,
}

impl<K> Default for Hover<K> {
    fn default() -> Self {
        Hover {
            values: BTreeMap::new(),
            last_time: None,
        }
    }
}

impl<K: Ord + Copy> Hover<K> {
    /// Steps every highlight towards its target at `now` (controls not named go to 0).
    pub fn step(&mut self, now: f64, targets: &[(K, f32)]) {
        let dt = self.last_time.map_or(1.0, |t| (now - t).max(0.0) as f32);
        self.last_time = Some(now);
        let k = 1.0 - (-dt * HOVER_RATE).exp();
        let mut keys: Vec<K> = self.values.keys().copied().collect();
        for (key, _) in targets {
            if !keys.contains(key) {
                keys.push(*key);
            }
        }
        for key in keys {
            let target = targets
                .iter()
                .find(|(t, _)| *t == key)
                .map_or(0.0, |(_, v)| *v);
            let cur = self.values.get(&key).copied().unwrap_or(0.0);
            let v = cur + (target - cur) * k;
            if v < 0.01 && target == 0.0 {
                self.values.remove(&key);
            } else {
                self.values.insert(key, v);
            }
        }
    }

    pub fn get(&self, key: K) -> f32 {
        self.values.get(&key).copied().unwrap_or(0.0)
    }
}

/// The double-click rule of the original's window procedure: a second press close in time
/// and place to the first is the double-click message, not a press, and the press after it
/// starts a new pair.
#[derive(Debug, Clone, Copy, Default)]
pub struct DoubleClick {
    last_press: Option<(f64, i32, i32)>,
    held: bool,
}

impl DoubleClick {
    /// The second press of a double click is held down.
    pub fn held(&self) -> bool {
        self.held
    }

    /// A left press at (`x`, `y`): true when it is the second press of a double click.
    pub fn press(&mut self, now: f64, x: i32, y: i32) -> bool {
        let double = self.last_press.is_some_and(|(t, px, py)| {
            now - t < DOUBLE_SECONDS
                && (px - x).abs() <= DOUBLE_PIXELS
                && (py - y).abs() <= DOUBLE_PIXELS
        });
        if double {
            self.held = true;
            self.last_press = None;
        } else {
            self.last_press = Some((now, x, y));
        }
        double
    }

    /// The left button went up.
    pub fn release(&mut self) {
        self.held = false;
    }
}

// ---- the scroll bar ----

/// The thumb on `track` for `total` rows, `page` of them shown from `top`; None when they
/// all fit. It is never shorter than `min_len`.
pub fn thumb(track: Rect, page: usize, total: usize, top: usize, min_len: i32) -> Option<Rect> {
    if total <= page {
        return None;
    }
    let h = track.height() as f32;
    let len = (h * page as f32 / total as f32).max(min_len as f32);
    let max_top = (total - page) as f32;
    let y = track.y0 as f32 + (h - len) * (top as f32 / max_top).clamp(0.0, 1.0);
    Some(Rect::new(
        track.x0 - 2,
        y.round() as i32,
        track.x1 + 2,
        (y + len).round() as i32,
    ))
}

/// The first row for the thumb's top at `y` (dragging).
pub fn top_for_thumb(track: Rect, page: usize, total: usize, y: i32, min_len: i32) -> usize {
    let Some(th) = thumb(track, page, total, 0, min_len) else {
        return 0;
    };
    let free = (track.height() - th.height()).max(1) as f32;
    let f = ((y - track.y0) as f32 / free).clamp(0.0, 1.0);
    (f * (total - page) as f32).round() as usize
}

/// The scroll bar: a faint track and the gold thumb, warmer under the pointer.
pub fn draw_scroll(canvas: &mut Canvas, track: Rect, th: Rect, hover: f32) {
    rounded(canvas, track, 1.5, [255, 255, 255, 16]);
    let inner = Rect::new(track.x0, th.y0, track.x1, th.y1);
    rounded(canvas, inner, 1.5, lerp([255, 200, 0, 140], GOLD, hover));
}

// ---- drawing helpers ----

pub fn lerp(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [m(0), m(1), m(2), m(3)]
}

pub fn alpha(c: Color, a: f32) -> Color {
    modern_hud::with_alpha(c, c[3] as f32 / 255.0 * a)
}

/// Coverage of a rounded rectangle at a pixel centre.
pub fn rounded_cov(r: Rect, radius: f32, px: f32, py: f32) -> f32 {
    let (x0, y0, x1, y1) = (r.x0 as f32, r.y0 as f32, r.x1 as f32, r.y1 as f32);
    let rad = radius.min((x1 - x0) / 2.0).min((y1 - y0) / 2.0).max(0.0);
    let cx = px.clamp(x0 + rad, x1 - rad);
    let cy = py.clamp(y0 + rad, y1 - rad);
    let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt() - rad;
    // Inside the straight parts the distance to the edge counts.
    let inside = (px - x0).min(x1 - px).min(py - y0).min(y1 - py);
    let cov = if d > 0.0 { 0.5 - d } else { inside + 0.5 };
    cov.clamp(0.0, 1.0)
}

/// A filled rounded rectangle, blended.
pub fn rounded(canvas: &mut Canvas, r: Rect, radius: f32, c: Color) {
    if c[3] == 0 {
        return;
    }
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let v = rounded_cov(r, radius, x as f32 + 0.5, y as f32 + 0.5);
            if v > 0.0 {
                canvas.blend(x, y, alpha(c, v));
            }
        }
    }
}

/// A rounded rectangle's outline, `w` pixels wide inside its edge.
pub fn rounded_outline(canvas: &mut Canvas, r: Rect, radius: f32, w: f32, c: Color) {
    if c[3] == 0 {
        return;
    }
    let wi = w.ceil() as i32;
    let inner = Rect::new(r.x0 + wi, r.y0 + wi, r.x1 - wi, r.y1 - wi);
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            if inner.contains(x, y) && x > inner.x0 + radius as i32 && x < inner.x1 - radius as i32
            {
                continue;
            }
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let outer = rounded_cov(r, radius, px, py);
            let inn = if inner.width() > 0 && inner.height() > 0 {
                rounded_cov(inner, (radius - w).max(0.0), px, py)
            } else {
                0.0
            };
            let v = (outer - inn).max(0.0);
            if v > 0.0 {
                canvas.blend(x, y, alpha(c, v));
            }
        }
    }
}

/// A soft shadow around `r`: rings fading outwards (only the border band is touched).
pub fn shadow(canvas: &mut Canvas, r: Rect, spread: i32) {
    for i in 1..=spread {
        let t = 1.0 - i as f32 / (spread + 1) as f32;
        let c = alpha(SHADOW, 0.22 * t * t);
        let o = Rect::new(r.x0 - i, r.y0 - i + 2, r.x1 + i, r.y1 + i + 2);
        canvas.blend_rect(Rect::new(o.x0, o.y0, o.x1, o.y0 + 1), c);
        canvas.blend_rect(Rect::new(o.x0, o.y1 - 1, o.x1, o.y1), c);
        canvas.blend_rect(Rect::new(o.x0, o.y0 + 1, o.x0 + 1, o.y1 - 1), c);
        canvas.blend_rect(Rect::new(o.x1 - 1, o.y0 + 1, o.x1, o.y1 - 1), c);
    }
}

pub fn hline(canvas: &mut Canvas, x0: i32, x1: i32, y: i32, c: Color) {
    canvas.blend_rect(Rect::new(x0, y, x1, y + 1), c);
}

/// Dims everything but `keep`.
pub fn dim_around(canvas: &mut Canvas, keep: Rect) {
    let (w, h) = (canvas.width, canvas.height);
    canvas.blend_rect(Rect::new(0, 0, w, keep.y0), SCENE_DIM);
    canvas.blend_rect(Rect::new(0, keep.y1, w, h), SCENE_DIM);
    canvas.blend_rect(Rect::new(0, keep.y0, keep.x0, keep.y1), SCENE_DIM);
    canvas.blend_rect(Rect::new(keep.x1, keep.y0, w, keep.y1), SCENE_DIM);
}

/// Writes with the pen, left-aligned at `x`; returns the advance.
#[allow(clippy::too_many_arguments)]
pub fn write(
    canvas: &mut Canvas,
    pen: &Pen,
    face: Face,
    px: f32,
    spacing: f32,
    x: i32,
    baseline: i32,
    col: Color,
    glow: Option<(Color, f32)>,
    s: &str,
) -> i32 {
    pen.write(canvas, face, px, spacing, x, baseline, col, glow, s)
}

/// Writes with the pen, its right edge at `right`.
#[allow(clippy::too_many_arguments)]
pub fn write_right(
    canvas: &mut Canvas,
    pen: &Pen,
    face: Face,
    px: f32,
    spacing: f32,
    right: i32,
    baseline: i32,
    col: Color,
    glow: Option<(Color, f32)>,
    s: &str,
) {
    let w = pen.advance(face, px, spacing, s);
    pen.write(canvas, face, px, spacing, right - w, baseline, col, glow, s);
}

/// Writes with the pen, centred on `cx`.
#[allow(clippy::too_many_arguments)]
pub fn write_centred(
    canvas: &mut Canvas,
    pen: &Pen,
    face: Face,
    px: f32,
    spacing: f32,
    cx: i32,
    baseline: i32,
    col: Color,
    glow: Option<(Color, f32)>,
    s: &str,
) {
    let w = pen.advance(face, px, spacing, s);
    pen.write(
        canvas,
        face,
        px,
        spacing,
        cx - w / 2,
        baseline,
        col,
        glow,
        s,
    );
}

/// The baseline that centres caps of `px` in `r` vertically.
pub fn mid_baseline(r: Rect, px: f32) -> i32 {
    ((r.y0 + r.y1) as f32 / 2.0 + px * 0.36).round() as i32
}

/// The glow of a value (white text with a soft gold halo).
pub fn value_glow(f: &Frame) -> Option<(Color, f32)> {
    Some((modern_hud::VALUE_GLOW, 3.0 * f.scale))
}

/// A thin glowing bar (track, glow, gradient fill).
pub fn bar(canvas: &mut Canvas, f: &Frame, r: Rect, fraction: f32, glow: f32) {
    modern_hud::draw_bar(canvas, r, fraction, 3.0 * f.scale, glow);
}

/// A paused bar: grey on the track.
pub fn paused_bar(canvas: &mut Canvas, r: Rect, fraction: f32) {
    canvas.blend_rect(r, modern_hud::TRACK);
    let w = (r.width() as f32 * fraction.clamp(0.0, 1.0)) as i32;
    canvas.blend_rect(Rect::new(r.x0, r.y0, r.x0 + w, r.y1), [170, 182, 196, 160]);
}

/// Shortens `s` with an ellipsis to fit `avail` pixels.
pub fn fit(pen: &Pen, face: Face, px: f32, spacing: f32, s: &str, avail: f32) -> String {
    let m = |t: &str| pen.measure(face, px, spacing, t);
    modern_panel::ellipsize(&m, s, avail)
}

// ---- pieces ----

/// The frame: the dimmed scene around, the shadow, the panel with its accent strip, the gold
/// wash under the header and the header's and footer's hairlines.
pub fn draw_frame(canvas: &mut Canvas, f: &Frame) {
    dim_around(canvas, f.panel);
    shadow(canvas, f.panel, f.px(18.0).max(6));
    canvas.blend_rect(f.panel, PANEL);
    let accent = Rect::new(
        f.panel.x0,
        f.panel.y0,
        f.panel.x0 + f.px(obj::ACCENT_W).max(1),
        f.panel.y1,
    );
    canvas.fill_rect(accent, obj::ACCENT);
    // A faint gold wash along the top.
    for (i, y) in (f.panel.y0..f.header.y1).enumerate() {
        let t = 1.0 - i as f32 / f.header.height().max(1) as f32;
        hline(
            canvas,
            accent.x1,
            f.panel.x1,
            y,
            [255, 200, 0, (14.0 * t) as u8],
        );
    }
    hline(canvas, f.title_x, f.inner_x1(), f.header.y1, LINE);
    if f.footer {
        hline(canvas, f.title_x, f.inner_x1(), f.footer_line, LINE);
    }
}

/// The title in letter-spaced gold caps, and after a thin divider what it is showing
/// (`subtitle`, muted caps) when there is one.
pub fn draw_title(canvas: &mut Canvas, pen: &Pen, f: &Frame, title: &str, subtitle: Option<&str>) {
    let x = write(
        canvas,
        pen,
        Face::Bold,
        f.text(TITLE_PX),
        TITLE_SPACING * f.scale,
        f.title_x,
        f.title_baseline,
        GOLD,
        Some((modern_hud::VALUE_GLOW, 3.0 * f.scale)),
        title,
    );
    let Some(name) = subtitle else { return };
    let dx = f.title_x + x + f.px(16.0);
    let h = f.px(14.0);
    canvas.blend_rect(
        Rect::new(
            dx,
            f.title_baseline - h,
            dx + 1,
            f.title_baseline + f.px(2.0),
        ),
        LINE,
    );
    write(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX + 1.0),
        SMALL_SPACING * f.scale,
        dx + f.px(16.0),
        f.title_baseline - f.px(1.0),
        MUTED,
        None,
        name,
    );
}

/// How a tab looks.
#[derive(Debug, Clone, Default)]
pub struct TabLook {
    pub label: String,
    /// It can be picked (an empty slot cannot).
    pub enabled: bool,
    pub current: bool,
    pub hover: f32,
    /// A number on the right (white, glowing).
    pub count: Option<String>,
    /// What runs there, 0..1: along the bottom edge.
    pub progress: Option<f32>,
    /// It runs paused: the bottom edge is grey.
    pub paused: bool,
    /// A second line under the label (taller tabs).
    pub detail: Option<(String, Color)>,
}

/// A tab: its wash, the current one's gold underline with a glow (filled to the progress),
/// the label (and its detail line), the count, the progress on the others' bottom edge.
pub fn draw_tab(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, t: &TabLook) {
    let hover = if t.enabled { t.hover } else { 0.0 };
    let bg = if t.current {
        [255, 200, 0, 30]
    } else {
        lerp([255, 255, 255, 6], [255, 255, 255, 20], hover)
    };
    rounded(canvas, r, 3.0 * f.scale, bg);
    let u = Rect::new(r.x0, r.y1 - f.px(2.0).max(2), r.x1, r.y1);
    let done = |p: f32| Rect::new(u.x0, u.y0, u.x0 + (u.width() as f32 * p) as i32, u.y1);
    if t.current {
        canvas.blend_rect(u, alpha(GOLD, 0.55));
        canvas.blend_rect(done(t.progress.unwrap_or(1.0)), GOLD);
        // Soft glow over the underline.
        for i in 1..=f.px(6.0).max(2) {
            let a = 0.22 * (1.0 - i as f32 / (f.px(6.0).max(2) + 1) as f32);
            hline(canvas, r.x0 + 2, r.x1 - 2, u.y0 - i, alpha(GOLD, a));
        }
    }
    let col = if !t.enabled {
        GREYED
    } else if t.current {
        LABEL
    } else {
        lerp(MUTED, WHITE, hover)
    };
    let px = f.text(BUTTON_PX);
    let base = match &t.detail {
        None => mid_baseline(r, px) - f.px(1.0),
        Some(_) => r.y0 + (r.height() as f32 * 0.42).round() as i32,
    };
    let right = r.x1 - f.px(12.0);
    let mut avail = (right - r.x0 - f.px(14.0)) as f32;
    if let Some(c) = &t.count {
        avail -= pen.advance(Face::Bold, f.text(VALUE_PX), 0.0, c) as f32 + f.px(10.0) as f32;
    }
    let label = fit(
        pen,
        Face::Bold,
        px,
        BUTTON_SPACING * f.scale,
        &t.label,
        avail,
    );
    write(
        canvas,
        pen,
        Face::Bold,
        px,
        BUTTON_SPACING * f.scale,
        r.x0 + f.px(14.0),
        base,
        col,
        None,
        &label,
    );
    if let Some((d, dcol)) = &t.detail {
        let dpx = f.text(BODY_PX - 1.0);
        let d = fit(
            pen,
            Face::Semibold,
            dpx,
            0.0,
            d,
            (right - r.x0 - f.px(14.0)) as f32,
        );
        let dcol = if t.enabled {
            lerp(*dcol, WHITE, hover * 0.5)
        } else {
            GREYED
        };
        write(
            canvas,
            pen,
            Face::Semibold,
            dpx,
            0.0,
            r.x0 + f.px(14.0),
            base + f.px(17.0),
            dcol,
            None,
            &d,
        );
    }
    if let Some(c) = &t.count {
        write_right(
            canvas,
            pen,
            Face::Bold,
            f.text(VALUE_PX),
            0.0,
            right,
            base + f.px(1.0),
            WHITE,
            value_glow(f),
            c,
        );
    }
    if !t.current {
        if let Some(p) = t.progress {
            canvas.blend_rect(u, modern_hud::TRACK);
            let c = if t.paused {
                [170, 182, 196, 200]
            } else {
                alpha(GOLD, 0.8)
            };
            canvas.blend_rect(done(p), c);
        }
    }
}

/// How a button looks.
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonLook<'a> {
    pub label: &'a str,
    /// The hot key, shown in a small cap.
    pub key: &'a str,
    pub hover: f32,
    /// Held down with the pointer on it.
    pub pressed: bool,
    /// The main action (gold while it would do something).
    pub primary: bool,
    /// A toggle that is on.
    pub toggled: bool,
    /// It would do nothing now (looks inactive; still clickable).
    pub idle: bool,
    /// The screen's keyboard focus is on it.
    pub focused: bool,
}

/// A button: rounded, outlined, a light along its top, the label and its key cap.
pub fn draw_button(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, b: &ButtonLook) {
    let hv = b.hover;
    let (primary, toggled, idle, pressed) = (b.primary, b.toggled, b.idle, b.pressed);
    let radius = 3.0 * f.scale;
    let (bg0, bg1) = if primary && !idle {
        ([255, 200, 0, 48], [255, 200, 0, 96])
    } else if toggled {
        ([255, 200, 0, 40], [255, 200, 0, 80])
    } else {
        ([255, 255, 255, 12], [255, 200, 0, 50])
    };
    let mut bg = lerp(bg0, bg1, hv / 0.6);
    if pressed {
        bg = [255, 200, 0, 130];
    }
    if b.focused {
        let ring = Rect::new(r.x0 - 3, r.y0 - 3, r.x1 + 3, r.y1 + 3);
        rounded_outline(canvas, ring, radius + 3.0, 1.0, alpha(GOLD, 0.7));
    }
    rounded(canvas, r, radius, bg);
    let border = if primary && !idle || toggled {
        lerp(alpha(GOLD, 0.75), GOLD, hv / 0.6)
    } else if idle {
        lerp([255, 255, 255, 30], alpha(GOLD, 0.6), hv / 0.6)
    } else {
        lerp([255, 255, 255, 60], GOLD, hv / 0.6)
    };
    rounded_outline(canvas, r, radius, 1.0, border);
    // A light along the top edge.
    hline(
        canvas,
        r.x0 + f.px(6.0),
        r.x1 - f.px(6.0),
        r.y0 + 1,
        [255, 255, 255, (12.0 + 30.0 * hv) as u8],
    );
    let px = f.text(BUTTON_PX);
    let sp = BUTTON_SPACING * f.scale;
    let col = if pressed {
        ON_GOLD
    } else if primary && !idle || toggled {
        lerp(LABEL, WHITE, hv / 0.6)
    } else if idle {
        lerp(MUTED, WHITE, hv / 0.6)
    } else {
        lerp(TEXT, WHITE, hv / 0.6)
    };
    // The hot key in a small cap on the right (none without a key).
    let shift = i32::from(pressed);
    let cap = if b.key.is_empty() {
        Rect::new(r.x1, r.y0, r.x1, r.y1)
    } else {
        let cap = key_cap_rect(pen, f, r.x1 - f.px(8.0), (r.y0 + r.y1) / 2, b.key);
        let cap = Rect::new(cap.x0, cap.y0.max(r.y0 + 3), cap.x1, cap.y1.min(r.y1 - 3));
        draw_key_cap(canvas, pen, f, cap, b.key, col, shift);
        cap
    };
    let label_room = cap.x0 - r.x0;
    let avail = (label_room - f.px(12.0)) as f32;
    // Tight (a small window): the letter spacing goes before the letters do.
    let sp = if pen.measure(Face::Bold, px, sp, b.label) > avail {
        0.0
    } else {
        sp
    };
    let label = fit(pen, Face::Bold, px, sp, b.label, avail);
    let w = pen.advance(Face::Bold, px, sp, &label);
    let lx = if b.key.is_empty() {
        r.x0 + (r.width() - w) / 2
    } else {
        r.x0 + (label_room - w) / 2 + f.px(4.0)
    };
    write(
        canvas,
        pen,
        Face::Bold,
        px,
        sp,
        lx.max(r.x0 + f.px(8.0)),
        mid_baseline(r, px) + shift,
        col,
        None,
        &label,
    );
}

/// Where a key cap showing `key` goes: its right edge at `right`, centred on `cy`.
pub fn key_cap_rect(pen: &Pen, f: &Frame, right: i32, cy: i32, key: &str) -> Rect {
    let kpx = f.text(SMALL_PX - 1.0);
    let kw = pen.advance(Face::Bold, kpx, 0.0, key).max(f.px(7.0));
    let cap_w = kw + f.px(10.0);
    let cap_h = f.px(16.0);
    Rect::new(right - cap_w, cy - cap_h / 2, right, cy - cap_h / 2 + cap_h)
}

/// A key cap: a thin outline around the key's name, in `col` (`shift` pixels down while
/// its button is held).
pub fn draw_key_cap(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    cap: Rect,
    key: &str,
    col: Color,
    shift: i32,
) {
    let kpx = f.text(SMALL_PX - 1.0);
    rounded_outline(canvas, cap, 2.0 * f.scale, 1.0, alpha(col, 0.45));
    write_centred(
        canvas,
        pen,
        Face::Bold,
        kpx,
        0.0,
        (cap.x0 + cap.x1) / 2,
        mid_baseline(cap, kpx) + shift,
        alpha(col, 0.8),
        None,
        key,
    );
}

/// A darker well under a list, for reading over a busy scene.
pub fn well(canvas: &mut Canvas, f: &Frame, r: Rect) {
    rounded(canvas, r, 4.0 * f.scale, [0, 0, 0, 56]);
}

/// A readout box's background.
pub fn section(canvas: &mut Canvas, f: &Frame, r: Rect) {
    rounded(canvas, r, 3.0 * f.scale, SECTION);
}

/// A readout: a faint box with its label on the left and a large value on the right, in
/// `value_col` with `glow`. Returns the rectangle of the bar under them.
#[allow(clippy::too_many_arguments)]
pub fn readout(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    r: Rect,
    label: &str,
    value: &str,
    value_col: Color,
    glow: Option<(Color, f32)>,
) -> Rect {
    section(canvas, f, r);
    let base = r.y0 + f.px(19.0);
    write(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX),
        SMALL_SPACING * f.scale,
        r.x0 + f.px(12.0),
        base,
        LABEL,
        None,
        label,
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        f.text(BIG_VALUE_PX),
        0.0,
        r.x1 - f.px(12.0),
        base + f.px(1.0),
        value_col,
        glow,
        value,
    );
    let y = r.y1 - f.px(10.0);
    Rect::new(
        r.x0 + f.px(12.0),
        y,
        r.x1 - f.px(12.0),
        y + f.px(4.0).max(2),
    )
}

/// A list row's band: a gold wash and a marker on its left edge when selected, a faint
/// light under the pointer, red while a refusal flashes.
pub fn row_band(
    canvas: &mut Canvas,
    f: &Frame,
    row: Rect,
    selected: bool,
    hover: f32,
    refused: f32,
) {
    let band = Rect::new(row.x0, row.y0 + 1, row.x1, row.y1 - 1);
    if selected {
        rounded(
            canvas,
            band,
            2.0 * f.scale,
            lerp([255, 200, 0, 26], [255, 200, 0, 40], hover),
        );
        canvas.fill_rect(
            Rect::new(
                band.x0,
                band.y0 + 2,
                band.x0 + f.px(2.0).max(2),
                band.y1 - 2,
            ),
            GOLD,
        );
    } else if hover > 0.0 {
        rounded(
            canvas,
            band,
            2.0 * f.scale,
            alpha([255, 255, 255, 18], hover / 0.6),
        );
    }
    if refused > 0.0 {
        rounded(
            canvas,
            band,
            2.0 * f.scale,
            alpha([255, 74, 30, 90], refused),
        );
    }
}

/// A group heading in a list: gold caps and a faint gold hairline to `x1`.
pub fn list_heading(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    x: i32,
    row: Rect,
    x1: i32,
    text: &str,
) {
    let base = row.y1 - f.px(6.0);
    let w = write(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX),
        SMALL_SPACING * f.scale,
        x,
        base,
        GOLD,
        None,
        &text.to_uppercase(),
    );
    let x0 = x + w + f.px(10.0);
    if x0 < x1 {
        hline(canvas, x0, x1, base - f.px(4.0), alpha(GOLD, 0.25));
    }
}

/// A section label (gold-yellow caps) with a faint gold hairline after it to `x1`.
pub fn section_heading(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    x: i32,
    baseline: i32,
    x1: i32,
    text: &str,
) {
    let lpx = f.text(SMALL_PX);
    let lsp = SMALL_SPACING * f.scale;
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        x,
        baseline,
        LABEL,
        None,
        text,
    );
    hline(
        canvas,
        x + pen.advance(Face::Bold, lpx, lsp, text) + f.px(10.0),
        x1,
        baseline - f.px(4.0),
        alpha(GOLD, 0.25),
    );
}

/// A small figure: its label on the left, the value on the right of a faint box.
pub fn cell(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, label: &str, value: &str) {
    rounded(canvas, r, 2.0 * f.scale, SECTION);
    let lpx = f.text(SMALL_PX);
    let lsp = SMALL_SPACING * f.scale;
    let b = mid_baseline(r, lpx);
    write(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX - 1.0),
        lsp,
        r.x0 + f.px(8.0),
        b,
        LABEL,
        None,
        label,
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        f.text(VALUE_PX - 1.0),
        0.0,
        r.x1 - f.px(8.0),
        b + f.px(1.0),
        WHITE,
        value_glow(f),
        value,
    );
}

/// Figures in two columns from (`x`, `y`) across `width`; returns the height used.
#[allow(clippy::too_many_arguments)]
pub fn cell_grid(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    x: i32,
    y: i32,
    width: i32,
    cells: &[(&str, String)],
) -> i32 {
    let colw = (width - f.px(16.0)) / 2;
    // One column when a figure would not fit beside its label in half the width.
    let fits = cells.iter().all(|(label, value)| {
        let lw = pen.advance(
            Face::Bold,
            f.text(SMALL_PX - 1.0),
            SMALL_SPACING * f.scale,
            label,
        );
        let vw = pen.advance(Face::Bold, f.text(VALUE_PX - 1.0), 0.0, value);
        lw + vw + f.px(28.0) <= colw
    });
    let (cols, w) = if fits { (2, colw) } else { (1, width) };
    for (k, (label, value)) in cells.iter().enumerate() {
        let cx0 = x + (k as i32 % cols) * (colw + f.px(16.0));
        let cy = y + (k as i32 / cols) * f.px(26.0);
        let r = Rect::new(cx0 - f.px(2.0), cy, cx0 + w - f.px(2.0), cy + f.px(22.0));
        cell(canvas, pen, f, r, label, value);
    }
    cells.len().div_ceil(cols as usize) as i32 * f.px(26.0)
}

/// A picture box: dark, with a soft warm light in the middle.
pub fn picture_box(canvas: &mut Canvas, f: &Frame, r: Rect) {
    rounded(canvas, r, 4.0 * f.scale, [0, 0, 0, 70]);
    let (cx, cy) = ((r.x0 + r.x1) as f32 / 2.0, (r.y0 + r.y1) as f32 / 2.0);
    let rad = r.width().min(r.height()) as f32 * 0.6;
    for y in r.y0..r.y1 {
        for x in r.x0..r.x1 {
            let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / rad;
            if d < 1.0 {
                let a = (1.0 - d).powi(2) * 0.09;
                canvas.blend(x, y, modern_hud::with_alpha([255, 190, 80, 255], a));
            }
        }
    }
}

/// Gold corner brackets around `r`.
pub fn brackets(canvas: &mut Canvas, f: &Frame, r: Rect) {
    let len = f.px(14.0);
    let c = alpha(GOLD, 0.8);
    let t = 1.max(f.px(1.5));
    for (x, y, dx, dy) in [
        (r.x0, r.y0, 1, 1),
        (r.x1 - 1, r.y0, -1, 1),
        (r.x0, r.y1 - 1, 1, -1),
        (r.x1 - 1, r.y1 - 1, -1, -1),
    ] {
        let hx = if dx > 0 {
            Rect::new(x, y, x + len, y + t)
        } else {
            Rect::new(x - len + 1, y, x + 1, y + t)
        };
        let hx = if dy > 0 { hx } else { hx.offset(0, -t + 1) };
        let vy = if dy > 0 {
            Rect::new(x, y, x + t, y + len)
        } else {
            Rect::new(x, y - len + 1, x + t, y + 1)
        };
        let vy = if dx > 0 { vy } else { vy.offset(-t + 1, 0) };
        canvas.blend_rect(hx, c);
        canvas.blend_rect(vy, c);
    }
}

/// Text over a picture: a dark pane inside `r`, a heading, and `text` wrapped in
/// paragraphs, roomy when it fits and tighter when not, cut with "..." when even that does
/// not fit.
pub fn text_pane(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, heading: &str, text: &str) {
    let inner = Rect::new(
        r.x0 + f.px(6.0),
        r.y0 + f.px(6.0),
        r.x1 - f.px(6.0),
        r.y1 - f.px(6.0),
    );
    rounded(canvas, inner, 3.0 * f.scale, [6, 8, 12, 200]);
    let bpx = f.text(BODY_PX);
    let m = |t: &str| pen.measure(Face::Semibold, bpx, 0.0, t);
    let width = (inner.width() - f.px(28.0)) as f32;
    let mut y = inner.y0 + f.px(26.0);
    write(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX),
        SMALL_SPACING * f.scale,
        inner.x0 + f.px(14.0),
        y,
        LABEL,
        None,
        heading,
    );
    y += f.px(24.0);
    let paras: Vec<Vec<String>> = text
        .split(['\n', '\r'])
        .filter(|p| !p.trim().is_empty())
        .map(|p| obj::wrap(p.trim(), width, width, &m))
        .collect();
    let lines: usize = paras.iter().map(Vec::len).sum();
    let room = inner.y1 - f.px(10.0) - y;
    let roomy = (bpx * 1.45).round() as i32;
    let (line_h, gap) = if lines as i32 * roomy + paras.len() as i32 * roomy / 3 <= room + roomy {
        (roomy, roomy / 3)
    } else {
        ((bpx * 1.2).round() as i32, 0)
    };
    let fits = (room / line_h.max(1) + 1).max(1) as usize;
    let mut shown = 0;
    'out: for para in &paras {
        for line in para {
            if shown + 1 == fits && shown + 1 < lines {
                let cut = format!("{}...", line.trim_end_matches(['.', ',', ' ']));
                let cut = modern_panel::ellipsize(&m, &cut, width);
                write(
                    canvas,
                    pen,
                    Face::Semibold,
                    bpx,
                    0.0,
                    inner.x0 + f.px(14.0),
                    y,
                    TEXT,
                    None,
                    &cut,
                );
                break 'out;
            }
            write(
                canvas,
                pen,
                Face::Semibold,
                bpx,
                0.0,
                inner.x0 + f.px(14.0),
                y,
                TEXT,
                None,
                line,
            );
            y += line_h;
            shown += 1;
        }
        y += gap;
    }
}

/// Where an image of `w` x `h` goes, as large as fits in `r` with its shape kept, centred.
pub fn fit_rect(r: Rect, w: i32, h: i32) -> Rect {
    if w <= 0 || h <= 0 || r.width() <= 0 || r.height() <= 0 {
        return Rect::new(r.x0, r.y0, r.x0, r.y0);
    }
    let s = (r.width() as f32 / w as f32).min(r.height() as f32 / h as f32);
    let (dw, dh) = ((w as f32 * s).round() as i32, (h as f32 * s).round() as i32);
    let x0 = r.x0 + (r.width() - dw) / 2;
    let y0 = r.y0 + (r.height() - dh) / 2;
    Rect::new(x0, y0, x0 + dw, y0 + dh)
}

/// Draws `img` filtered (bilinear) into `dst`, its alpha times `opacity`.
pub fn blit_smooth(canvas: &mut Canvas, img: &Image, dst: Rect, opacity: f32) {
    if dst.width() <= 0 || dst.height() <= 0 || img.width <= 0 || img.height <= 0 {
        return;
    }
    let sx = img.width as f32 / dst.width() as f32;
    let sy = img.height as f32 / dst.height() as f32;
    // Shrinking a lot: average a few taps per axis.
    let taps = sx.max(sy).ceil().clamp(1.0, 4.0) as i32;
    for y in dst.y0..dst.y1 {
        for x in dst.x0..dst.x1 {
            let mut acc = [0.0f32; 4];
            for ty in 0..taps {
                for tx in 0..taps {
                    let fx = ((x - dst.x0) as f32 + (tx as f32 + 0.5) / taps as f32) * sx - 0.5;
                    let fy = ((y - dst.y0) as f32 + (ty as f32 + 0.5) / taps as f32) * sy - 0.5;
                    let p = bilinear(img, fx, fy);
                    for i in 0..4 {
                        acc[i] += p[i];
                    }
                }
            }
            let n = (taps * taps) as f32;
            let a = acc[3] / n * opacity;
            if a <= 0.0 {
                continue;
            }
            // Premultiplied sums back to straight colour.
            let c = |i: usize| (acc[i] / acc[3].max(1e-6)).round().clamp(0.0, 255.0) as u8;
            canvas.blend(x, y, [c(0), c(1), c(2), a.round().clamp(0.0, 255.0) as u8]);
        }
    }
}

/// The image at (`x`, `y`), premultiplied (colour times alpha, alpha 0..255).
fn bilinear(img: &Image, x: f32, y: f32) -> [f32; 4] {
    let x = x.clamp(0.0, (img.width - 1) as f32);
    let y = y.clamp(0.0, (img.height - 1) as f32);
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (x1, y1) = ((x0 + 1).min(img.width - 1), (y0 + 1).min(img.height - 1));
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0.0; 4];
    for (px, py, w) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)),
        (x0, y1, (1.0 - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        let p = img.get(px, py);
        let a = p[3] as f32;
        for i in 0..3 {
            out[i] += p[i] as f32 * a * w;
        }
        out[3] += a * w;
    }
    out
}

// ---- form controls (the modern menus, options and systems manager) ----

/// A filled circle, anti-aliased.
pub fn circle(canvas: &mut Canvas, cx: f32, cy: f32, radius: f32, c: Color) {
    if c[3] == 0 || radius <= 0.0 {
        return;
    }
    let (x0, x1) = ((cx - radius - 1.0) as i32, (cx + radius + 1.0) as i32);
    let (y0, y1) = ((cy - radius - 1.0) as i32, (cy + radius + 1.0) as i32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let v = (radius + 0.5 - d).clamp(0.0, 1.0);
            if v > 0.0 {
                canvas.blend(x, y, alpha(c, v));
            }
        }
    }
}

/// A soft halo around a point (a knob's glow), `strength` at the centre.
pub fn halo(canvas: &mut Canvas, cx: f32, cy: f32, radius: f32, c: Color, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    let r = radius.max(1.0);
    for y in (cy - r) as i32..=(cy + r) as i32 {
        for x in (cx - r) as i32..=(cx + r) as i32 {
            let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt() / r;
            if d < 1.0 {
                canvas.blend(x, y, alpha(c, (1.0 - d).powi(2) * strength));
            }
        }
    }
}

/// A small chevron pointing down (or up), centred on (`cx`, `cy`), `size` pixels wide.
pub fn chevron(canvas: &mut Canvas, cx: i32, cy: i32, size: i32, up: bool, c: Color) {
    let half = (size / 2).max(2);
    for i in 0..=half {
        // The tips at the ends, the point in the middle.
        let dy = if up {
            i / 2 - half / 2
        } else {
            half / 2 - i / 2
        };
        for t in 0..2 {
            canvas.blend(cx - half + i, cy - dy + t, c);
            if i < half {
                canvas.blend(cx + half - i, cy - dy + t, c);
            }
        }
    }
}

/// The gold ring of the keyboard focus around `r`.
pub fn focus_ring(canvas: &mut Canvas, f: &Frame, r: Rect) {
    let ring = Rect::new(r.x0 - 3, r.y0 - 3, r.x1 + 3, r.y1 + 3);
    rounded_outline(canvas, ring, 3.0 * f.scale + 3.0, 1.0, alpha(GOLD, 0.7));
}

/// The keyboard focus on a row of settings: a faint gold wash and a gold mark on its left.
pub fn focus_row(canvas: &mut Canvas, f: &Frame, row: Rect) {
    let band = Rect::new(row.x0, row.y0 + 2, row.x1, row.y1 - 2);
    rounded(canvas, band, 2.0 * f.scale, [255, 200, 0, 16]);
    canvas.fill_rect(
        Rect::new(
            band.x0,
            band.y0 + 3,
            band.x0 + f.px(2.0).max(2),
            band.y1 - 3,
        ),
        alpha(GOLD, 0.9),
    );
}

/// An on / off switch in `r` (a pill): `on` eases the knob across (0 off, 1 on).
pub fn draw_switch(canvas: &mut Canvas, f: &Frame, r: Rect, on: f32, hover: f32, disabled: bool) {
    let on = on.clamp(0.0, 1.0);
    let radius = r.height() as f32 / 2.0;
    let track = if disabled {
        [255, 255, 255, 14]
    } else {
        lerp(
            lerp([255, 255, 255, 22], [255, 255, 255, 40], hover),
            lerp([255, 200, 0, 120], [255, 200, 0, 170], hover),
            on,
        )
    };
    rounded(canvas, r, radius, track);
    let border = if disabled {
        [255, 255, 255, 30]
    } else {
        lerp(lerp([255, 255, 255, 70], alpha(GOLD, 0.8), hover), GOLD, on)
    };
    rounded_outline(canvas, r, radius, 1.0, border);
    let kr = radius - (2.0 * f.scale).max(2.0);
    let x0 = r.x0 as f32 + radius;
    let x1 = r.x1 as f32 - radius;
    let cx = x0 + (x1 - x0) * on;
    let cy = (r.y0 + r.y1) as f32 / 2.0;
    let knob = if disabled {
        [160, 160, 160, 120]
    } else {
        lerp([200, 206, 214, 255], [255, 246, 220, 255], on)
    };
    if !disabled {
        halo(canvas, cx, cy, kr * 2.2, GOLD, 0.25 * on + 0.15 * hover);
    }
    circle(canvas, cx, cy + 1.0, kr, [0, 0, 0, 90]);
    circle(canvas, cx, cy, kr, knob);
}

/// A horizontal slider on `track` (its bar along the middle): the gradient fill to
/// `fraction` and a round knob there, bigger and glowing under the pointer.
pub fn draw_slider(
    canvas: &mut Canvas,
    f: &Frame,
    track: Rect,
    fraction: f32,
    hover: f32,
    disabled: bool,
) {
    let fr = fraction.clamp(0.0, 1.0);
    let h = f.px(4.0).max(3);
    let cy = (track.y0 + track.y1) / 2;
    let bar_r = Rect::new(track.x0, cy - h / 2, track.x1, cy - h / 2 + h);
    if disabled {
        rounded(canvas, bar_r, h as f32 / 2.0, [255, 255, 255, 20]);
    } else {
        bar(canvas, f, bar_r, fr, 0.6 + 0.4 * hover);
    }
    let kx = track.x0 as f32 + track.width() as f32 * fr;
    let kr = f.px(7.0).max(4) as f32 + 1.5 * hover * f.scale;
    if !disabled {
        halo(canvas, kx, cy as f32, kr * 2.4, GOLD, 0.2 + 0.25 * hover);
    }
    circle(canvas, kx, cy as f32 + 1.0, kr, [0, 0, 0, 110]);
    let knob = if disabled {
        [150, 150, 150, 160]
    } else {
        lerp([236, 239, 243, 255], [255, 246, 220, 255], hover)
    };
    circle(canvas, kx, cy as f32, kr, knob);
    if !disabled {
        circle(canvas, kx, cy as f32, kr * 0.45, alpha(GOLD, 0.85));
    }
}

/// A vertical slider on `track` (the bar up its middle; `fraction` 0 at the top, as the
/// original's vertical sliders count), filled from the knob down.
pub fn draw_vslider(
    canvas: &mut Canvas,
    f: &Frame,
    track: Rect,
    fraction: f32,
    hover: f32,
    disabled: bool,
) {
    let fr = fraction.clamp(0.0, 1.0);
    let w = f.px(4.0).max(3);
    let cx = (track.x0 + track.x1) / 2;
    let bar_r = Rect::new(cx - w / 2, track.y0, cx - w / 2 + w, track.y1);
    rounded(canvas, bar_r, w as f32 / 2.0, modern_hud::TRACK);
    let ky = track.y0 as f32 + track.height() as f32 * fr;
    if !disabled {
        for y in ky as i32..bar_r.y1 {
            let t = 1.0 - (y - track.y0) as f32 / track.height().max(1) as f32;
            hline(canvas, bar_r.x0, bar_r.x1, y, modern_hud::gradient(t));
        }
    }
    let kr = f.px(7.0).max(4) as f32 + 1.5 * hover * f.scale;
    let kx = cx as f32 + 0.5;
    if !disabled {
        halo(canvas, kx, ky, kr * 2.4, GOLD, 0.2 + 0.25 * hover);
    }
    circle(canvas, kx, ky + 1.0, kr, [0, 0, 0, 110]);
    circle(canvas, kx, ky, kr, lerp(TEXT, [255, 246, 220, 255], hover));
    circle(canvas, kx, ky, kr * 0.45, alpha(GOLD, 0.85));
}

/// `n` equal segments across `r` (a segmented control's parts).
pub fn segment_rects(r: Rect, n: usize) -> Vec<Rect> {
    let n = n.max(1) as i32;
    (0..n)
        .map(|i| {
            Rect::new(
                r.x0 + r.width() * i / n,
                r.y0,
                r.x0 + r.width() * (i + 1) / n,
                r.y1,
            )
        })
        .collect()
}

/// How a segmented control or a drop-down's list looks.
#[derive(Debug, Clone, Default)]
pub struct ChoiceLook<'a> {
    pub labels: &'a [String],
    pub current: Option<usize>,
    /// Each choice's eased highlight.
    pub hover: Vec<f32>,
    pub pressed: Option<usize>,
    pub disabled: bool,
}

/// A segmented control: one box split into its choices, the current one gold.
pub fn draw_segments(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, c: &ChoiceLook) {
    let radius = 3.0 * f.scale;
    rounded(canvas, r, radius, [255, 255, 255, 10]);
    let parts = segment_rects(r, c.labels.len());
    let px = f.text(BUTTON_PX - 1.0);
    let sp = SMALL_SPACING * f.scale * 0.6;
    for (i, (p, label)) in parts.iter().zip(c.labels).enumerate() {
        let hv = if c.disabled {
            0.0
        } else {
            c.hover.get(i).copied().unwrap_or(0.0)
        };
        let inner = Rect::new(p.x0 + 2, p.y0 + 2, p.x1 - 2, p.y1 - 2);
        let cur = c.current == Some(i);
        let pressed = c.pressed == Some(i) && !c.disabled;
        if pressed {
            rounded(canvas, inner, radius - 1.0, [255, 200, 0, 130]);
        } else if cur {
            let bg = if c.disabled {
                [255, 255, 255, 24]
            } else {
                lerp([255, 200, 0, 60], [255, 200, 0, 90], hv)
            };
            rounded(canvas, inner, radius - 1.0, bg);
            if !c.disabled {
                rounded_outline(canvas, inner, radius - 1.0, 1.0, alpha(GOLD, 0.9));
            }
        } else if hv > 0.0 {
            rounded(
                canvas,
                inner,
                radius - 1.0,
                alpha([255, 255, 255, 22], hv / 0.6),
            );
        }
        if i > 0 && !cur && c.current != Some(i - 1) {
            canvas.blend_rect(
                Rect::new(p.x0, p.y0 + f.px(7.0), p.x0 + 1, p.y1 - f.px(7.0)),
                LINE,
            );
        }
        let col = if c.disabled {
            if cur {
                MUTED
            } else {
                GREYED
            }
        } else if pressed {
            ON_GOLD
        } else if cur {
            lerp(LABEL, WHITE, hv)
        } else {
            lerp(MUTED, WHITE, hv / 0.6)
        };
        let t = fit(
            pen,
            Face::Bold,
            px,
            sp,
            label,
            (p.width() - f.px(8.0)) as f32,
        );
        write_centred(
            canvas,
            pen,
            Face::Bold,
            px,
            sp,
            (p.x0 + p.x1) / 2,
            mid_baseline(*p, px),
            col,
            None,
            &t,
        );
    }
    let edge = if c.disabled {
        [255, 255, 255, 25]
    } else {
        [255, 255, 255, 50]
    };
    rounded_outline(canvas, r, radius, 1.0, edge);
}

/// A drop-down's closed box: the current choice and a chevron (up while open).
#[allow(clippy::too_many_arguments)]
pub fn draw_select(
    canvas: &mut Canvas,
    pen: &Pen,
    f: &Frame,
    r: Rect,
    text: &str,
    open: bool,
    hover: f32,
    disabled: bool,
) {
    let radius = 3.0 * f.scale;
    let bg = if open {
        [255, 200, 0, 40]
    } else {
        lerp([255, 255, 255, 12], [255, 200, 0, 36], hover / 0.6)
    };
    rounded(canvas, r, radius, bg);
    let border = if disabled {
        [255, 255, 255, 25]
    } else if open {
        GOLD
    } else {
        lerp([255, 255, 255, 60], GOLD, hover / 0.6)
    };
    rounded_outline(canvas, r, radius, 1.0, border);
    let px = f.text(BODY_PX);
    let col = if disabled {
        GREYED
    } else {
        lerp(TEXT, WHITE, hover)
    };
    let chev = f.px(10.0).max(6);
    let t = fit(
        pen,
        Face::Semibold,
        px,
        0.0,
        text,
        (r.width() - f.px(24.0) - chev) as f32,
    );
    write(
        canvas,
        pen,
        Face::Semibold,
        px,
        0.0,
        r.x0 + f.px(10.0),
        mid_baseline(r, px),
        col,
        None,
        &t,
    );
    let chev_col = alpha(if disabled { GREYED } else { GOLD }, 0.9);
    chevron(
        canvas,
        r.x1 - f.px(12.0) - chev / 2,
        (r.y0 + r.y1) / 2,
        chev,
        open,
        chev_col,
    );
}

/// The panel a drop-down's list sits on, around its `rows`.
pub fn choices_panel(f: &Frame, rows: &[Rect]) -> Option<Rect> {
    let (first, last) = (rows.first()?, rows.last()?);
    let pad = f.px(4.0);
    Some(Rect::new(
        first.x0 - pad,
        first.y0 - pad,
        first.x1 + pad,
        last.y1 + pad,
    ))
}

/// A drop-down's open list: a solid panel with a row per choice, the current one marked,
/// the one under the pointer warmed.
pub fn draw_choices(canvas: &mut Canvas, pen: &Pen, f: &Frame, rows: &[Rect], c: &ChoiceLook) {
    let Some(r) = choices_panel(f, rows) else {
        return;
    };
    shadow(canvas, r, f.px(10.0).max(4));
    rounded(canvas, r, 4.0 * f.scale, [14, 16, 22, 250]);
    rounded_outline(canvas, r, 4.0 * f.scale, 1.0, alpha(GOLD, 0.6));
    let px = f.text(BODY_PX);
    for (i, (row, label)) in rows.iter().zip(c.labels).enumerate() {
        let hv = c.hover.get(i).copied().unwrap_or(0.0);
        let sel = c.current == Some(i);
        row_band(canvas, f, *row, sel, hv, 0.0);
        if c.pressed == Some(i) {
            rounded(canvas, *row, 2.0 * f.scale, [255, 200, 0, 110]);
        }
        let col = if sel {
            lerp(LABEL, WHITE, hv)
        } else {
            lerp(TEXT, WHITE, hv)
        };
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.0,
            row.x0 + f.px(14.0),
            mid_baseline(*row, px),
            col,
            None,
            label,
        );
    }
}

/// How a text field looks.
#[derive(Debug, Clone, Copy, Default)]
pub struct FieldLook<'a> {
    pub text: &'a str,
    /// Shown muted while the field is empty.
    pub placeholder: &'a str,
    /// It has the keyboard.
    pub focused: bool,
    /// The caret is shown (it blinks).
    pub caret: bool,
    pub hover: f32,
}

/// A text field: a dark box, the text, and a caret at its end while it has the keyboard.
pub fn draw_field(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, t: &FieldLook) {
    let radius = 3.0 * f.scale;
    rounded(canvas, r, radius, [0, 0, 0, 90]);
    let border = if t.focused {
        GOLD
    } else {
        lerp([255, 255, 255, 50], alpha(GOLD, 0.8), t.hover / 0.6)
    };
    rounded_outline(canvas, r, radius, 1.0, border);
    let px = f.text(BODY_PX + 1.0);
    let base = mid_baseline(r, px);
    let x = r.x0 + f.px(12.0);
    if t.text.is_empty() && !t.focused {
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.0,
            x,
            base,
            DIM,
            None,
            t.placeholder,
        );
    }
    let shown = fit(
        pen,
        Face::Semibold,
        px,
        0.0,
        t.text,
        (r.width() - f.px(28.0)) as f32,
    );
    let w = write(
        canvas,
        pen,
        Face::Semibold,
        px,
        0.0,
        x,
        base,
        WHITE,
        None,
        &shown,
    );
    if t.caret {
        let cx = if t.text.is_empty() { x } else { x + w + 1 };
        let h = (px * 1.05).round() as i32;
        canvas.blend_rect(
            Rect::new(
                cx,
                base - h + f.px(2.0),
                cx + f.px(2.0).max(1),
                base + f.px(3.0),
            ),
            GOLD,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sized_frames_keep_the_footer_at_the_bottom() {
        let f = Frame::sized(1920, 1080, 640.0, 420.0, 0, true);
        assert_eq!(f.panel.width(), 640);
        assert_eq!(f.panel.height(), 420);
        assert_eq!(
            f.footer_button(0).y1,
            f.panel.y1 - (REF_H - BUTTON_Y - BUTTON_H) as i32
        );
        assert_eq!(f.footer_right().x1, f.inner_x1());
        assert!(f.footer_line < f.footer_button(0).y0);
        // A tall panel shrinks to fit a small window.
        let s = Frame::sized(640, 480, 960.0, 900.0, 0, true);
        assert!(s.panel.y0 >= 0 && s.panel.y1 <= 480, "{:?}", s.panel);
        // The managers' frame is unchanged.
        assert_eq!(Frame::new(1920, 1080).footer_line, 160 + 680);
    }

    #[test]
    fn segments_split_evenly() {
        let parts = segment_rects(Rect::new(10, 0, 110, 20), 3);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0].x0, 10);
        assert_eq!(parts[2].x1, 110);
        assert!(parts.windows(2).all(|w| w[0].x1 == w[1].x0));
    }

    #[test]
    fn form_controls_draw_inside_their_boxes() {
        let f = Frame::new(1920, 1080);
        let mut c = Canvas::new(200, 60);
        draw_switch(&mut c, &f, Rect::new(10, 10, 54, 32), 1.0, 0.0, false);
        // On: the track is gold, the knob sits on the right.
        let track = c.get(20, 21);
        assert!(track[0] > track[2], "{track:?}");
        assert!(c.get(43, 21)[0] > 200);
        assert_eq!(
            c.get(100, 21),
            Canvas::new(1, 1).get(0, 0),
            "nothing outside"
        );
        let mut c = Canvas::new(200, 40);
        draw_slider(&mut c, &f, Rect::new(20, 10, 180, 30), 0.5, 0.0, false);
        assert!(c.get(100, 20)[0] > 200, "the knob at the middle");
        assert!(c.get(40, 20)[3] > 0, "the filled part");
        circle(&mut c, 10.0, 10.0, 3.0, [255, 0, 0, 255]);
        assert_eq!(c.get(9, 9)[0], 255);
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn frame_is_centred_and_scales_with_the_height() {
        let f = Frame::new(1920, 1080);
        assert_eq!(f.scale, 1.0);
        assert_eq!(f.panel, Rect::new(480, 160, 1440, 920));
        let m = Frame::new(1024, 768);
        assert!((m.scale - MIN_SCALE).abs() < 1e-6);
        assert_eq!(m.panel.width(), 768);
        let s = Frame::new(640, 480);
        assert!(s.panel.x0 >= 8 && s.panel.x1 <= 632, "{:?}", s.panel);
        assert!(s.panel.y0 >= top_band(480) && s.panel.y1 <= 472);
        assert_eq!(Frame::new(3840, 2160).scale, 2.0);
        // The header buttons right to left, the footer's left to right.
        assert!(f.top_button(1).x1 < f.top_button(0).x0);
        assert_eq!(f.top_button(0).x1, f.inner_x1());
        assert!(f.footer_button(0).x1 < f.footer_button(1).x0);
        assert_eq!(f.footer_right().x1, f.inner_x1());
    }

    #[test]
    fn hover_eases_in_and_out() {
        let mut h: Hover<u8> = Hover::default();
        h.step(0.0, &[(1, 1.0)]);
        assert!(h.get(1) > 0.99, "the first step jumps");
        h.step(0.07, &[]);
        let v = h.get(1);
        assert!(v > 0.2 && v < 0.6, "{v}");
        for i in 0..20 {
            h.step(0.07 + i as f64 * 0.1, &[]);
        }
        assert_eq!(h.get(1), 0.0);
    }

    #[test]
    fn double_click_rule() {
        let mut d = DoubleClick::default();
        assert!(!d.press(1.0, 10, 10));
        d.release();
        assert!(d.press(1.3, 12, 11), "close in time and place");
        assert!(d.held());
        d.release();
        assert!(!d.held());
        assert!(
            !d.press(1.4, 12, 11),
            "the press after a double starts a new pair"
        );
        d.release();
        assert!(!d.press(2.5, 12, 11), "too late");
        d.release();
        assert!(!d.press(2.6, 30, 11), "too far");
    }

    #[test]
    fn thumb_round_trips() {
        let track = Rect::new(0, 100, 3, 508);
        for top in 0..=13 {
            let th = thumb(track, 17, 30, top, 24).unwrap();
            assert_eq!(top_for_thumb(track, 17, 30, th.y0, 24), top);
        }
        assert!(thumb(track, 17, 17, 0, 24).is_none());
        assert_eq!(thumb(track, 17, 30, 13, 24).unwrap().y1, track.y1);
    }

    #[test]
    fn rounded_rect_coverage() {
        let r = Rect::new(0, 0, 20, 10);
        assert_eq!(rounded_cov(r, 3.0, 10.5, 5.5), 1.0);
        assert!(rounded_cov(r, 3.0, 0.5, 0.5) < 0.3, "corners are cut");
        assert_eq!(rounded_cov(r, 3.0, 25.0, 5.0), 0.0);
        let mut c = Canvas::new(30, 20);
        rounded(&mut c, Rect::new(2, 2, 28, 18), 4.0, [255, 255, 255, 255]);
        assert_eq!(c.get(15, 10), [255, 255, 255, 255]);
        assert_ne!(c.get(2, 2), [255, 255, 255, 255]);
        let mut c = Canvas::new(30, 20);
        rounded_outline(
            &mut c,
            Rect::new(2, 2, 28, 18),
            4.0,
            1.0,
            [255, 255, 255, 255],
        );
        assert_eq!(c.get(15, 10)[0], 0, "an outline leaves the inside");
        assert!(c.get(15, 2)[0] > 200);
    }

    #[test]
    fn pictures_keep_their_shape() {
        let r = Rect::new(0, 0, 300, 200);
        assert_eq!(fit_rect(r, 256, 256), Rect::new(50, 0, 250, 200));
        assert_eq!(fit_rect(r, 600, 200), Rect::new(0, 50, 300, 150));
        let mut img = Image::new(4, 4, [200, 100, 50, 255]);
        img.pixels[0] = [0, 0, 0, 0];
        let mut c = Canvas::new(8, 8);
        c.clear([255, 255, 255, 255]);
        blit_smooth(&mut c, &img, Rect::new(0, 0, 8, 8), 1.0);
        assert_eq!(c.get(6, 6), [200, 100, 50, 255]);
        // A transparent texel lets the white through without a dark fringe.
        let p = c.get(1, 1);
        assert!(p[0] >= 199 && p[2] >= 50 && p[2] < 255, "{p:?}");
    }
}
