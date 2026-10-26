//! The modern HUD's objectives panel (part of the user-requested modern HUD, `--hud`): a
//! top-left panel in place of the original's bottom-left "Objectives:" list
//! (`moDrawMissionInfo`). A header bar reads "OBJECTIVES" with a chevron and the number of
//! objectives still open; clicking it collapses or expands the panel (remembered in the
//! options file as `hwcObjectivesExpanded`). Expanded, a translucent dark panel with a gold
//! accent strip lists every objective with a diamond marker: hollow and white while
//! incomplete, filled and greyed out once done. Long text wraps. Lines are clickable as in
//! the original: the others open the objectives screen; the line under the pointer is
//! highlighted. A newly added objective flashes the header (the original draws no attention
//! to it, so the panel does not expand by itself).
//!
//! The hyperspace / end-mission entry (`moObjectiveAddFirst` puts it first) is drawn as a
//! button at the top of the list instead of a line, so it reads as something to press: full
//! width, gold-outlined with a soft glow, its label in bold caps after a hyperspace glyph. It
//! brightens under the pointer, shows pressed while held, and jumps on the release over it
//! (as the managers' buttons do; the original's line acts on the press). It pulses gently
//! when it appears and breathes slowly while it waits. Collapsed, a compact copy sits next to
//! the header so the jump stays a click away. While the game is paused it is dimmed and the
//! click does nothing, as the original ignores it then.
//!
//! The mission scripts mostly complete an objective by destroying it (`ObjectiveDestroy`),
//! which takes it off the original's list; only a few are marked complete
//! (`ObjectiveSet`). The panel remembers the objectives it has seen this mission and keeps a
//! destroyed one in its place, greyed out as complete (it is no longer the game's, so it is
//! not clickable). The order is the game's: the hyperspace entry first, then the rest as
//! they were created.
//!
//! Like the original's top-left text it starts below the speech band and portrait while one
//! is up (the overlay's top, `moSetTop`), and it hides with the rest of the overlay in
//! cutscenes, the letterbox and fleet intelligence. Over the full-screen managers it stays up
//! (see [`modern_hud`]'s notes on folding there).

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::host::{HostView, Objective};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_manager as mm;
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcObjectivesExpanded";

/// Panel background, accent and header text.
pub const PANEL: Color = [8, 10, 14, 153];
pub const ACCENT: Color = [0xFF, 0xC8, 0x00, 0xFF];
pub const GOLD: Color = [0xFF, 0xC8, 0x00, 0xFF];
/// A complete objective's text and marker: 45% white; brighter under the pointer.
pub const DONE: Color = [255, 255, 255, 115];
const DONE_HOVER: Color = [255, 255, 255, 185];
/// How long the header flashes after an objective is added, and one flash's length.
pub const FLASH_SECONDS: f32 = 2.4;
const FLASH_PERIOD: f32 = 0.8;

/// Reference layout, in pixels at 1080 lines (the header's sizes are shared with the
/// resources panel).
const MARGIN: f32 = 16.0;
const WIDTH: f32 = 360.0;
pub(crate) const ACCENT_W: f32 = 3.0;
pub(crate) const HEADER_H: f32 = 30.0;
pub(crate) const HEADER_BASELINE: f32 = 20.0;
pub(crate) const CHEVRON_X: f32 = 15.0;
pub(crate) const CHEVRON_R: f32 = 4.5;
pub(crate) const LABEL_X: f32 = 28.0;
pub(crate) const COUNT_RIGHT: f32 = 12.0;
pub(crate) const HEADER_PX: f32 = 12.0;
pub(crate) const HEADER_SPACING: f32 = 2.0;
pub(crate) const COUNT_PX: f32 = 13.0;
/// Collapsed: the gap between the label and the count.
pub(crate) const COLLAPSED_GAP: f32 = 10.0;
const ITEM_PX: f32 = 14.0;
const MARKER_X: f32 = 17.0;
const MARKER_R: f32 = 4.5;
const MARKER_LINE: f32 = 1.5;
const TEXT_X: f32 = 32.0;
const TEXT_RIGHT: f32 = 14.0;
const LINE_H: f32 = 19.0;
const FIRST_BASELINE: f32 = 14.0;
const ITEM_GAP: f32 = 6.0;
const ITEMS_TOP: f32 = 2.0;
/// The hyperspace button: inset from the panel's sides, its height (a row is 23), the gap
/// under it, its label and glyph.
const BUTTON_X0: f32 = 12.0;
const BUTTON_X1: f32 = 10.0;
const BUTTON_TOP: f32 = 6.0;
const BUTTON_H: f32 = 32.0;
const BUTTON_GAP: f32 = 8.0;
pub(crate) const BUTTON_PX: f32 = 13.0;
pub(crate) const BUTTON_SPACING: f32 = 2.0;
const GLYPH_R: f32 = 5.0;
const GLYPH_GAP: f32 = 8.0;
/// Collapsed: the compact button after the header.
const COMPACT_GAP: f32 = 6.0;
const COMPACT_H: f32 = 22.0;
const COMPACT_PAD: f32 = 10.0;
pub(crate) const COMPACT_PX: f32 = 11.0;
pub(crate) const COMPACT_SPACING: f32 = 1.5;
const COMPACT_GLYPH_R: f32 = 3.5;
const COMPACT_GLYPH_GAP: f32 = 6.0;
/// The pulse: brisker for a few seconds after the button appears, then a slow breath.
pub const INTRO_SECONDS: f32 = 3.0;
const INTRO_PERIOD: f32 = 1.0;
const BREATH_PERIOD: f32 = 2.6;
const BREATH: f32 = 0.35;
const PAD_BOTTOM: f32 = 8.0;
/// Room left under the panel before the original's top-left columns start.
const BELOW_GAP: f32 = 8.0;

/// Which text a width is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    Item,
    /// The header's label and count.
    Header,
    Count,
    /// The hyperspace button's label, full size and compact.
    Button,
    Compact,
}

/// One objective as the panel lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct Listed {
    pub objective: Objective,
    /// Its index in the game's list; None for one the game has destroyed (shown complete).
    pub live: Option<usize>,
}

impl Listed {
    /// Done: marked complete, or gone from the game's list (never the hyperspace entry).
    pub fn complete(&self) -> bool {
        !self.objective.hyperspace && (self.objective.complete || self.live.is_none())
    }
}

fn same(a: &Objective, b: &Objective) -> bool {
    a.text == b.text && a.description == b.description && a.hyperspace == b.hyperspace
}

/// The game's objectives (`live`, in its order) with the ones seen before and destroyed since
/// (`seen`, the previous listing) kept where they were: each goes after the entry it followed
/// last time, or under the hyperspace entry when it was first. A destroyed hyperspace entry
/// is dropped.
pub fn listed(seen: &[Objective], live: &[Objective]) -> Vec<Listed> {
    let mut out: Vec<Listed> = live
        .iter()
        .enumerate()
        .map(|(i, o)| Listed {
            objective: o.clone(),
            live: Some(i),
        })
        .collect();
    let mut used = vec![false; live.len()];
    let mut prev: Option<usize> = None;
    for s in seen {
        if let Some(i) = (0..live.len()).find(|&i| !used[i] && same(&live[i], s)) {
            used[i] = true;
            prev = out.iter().position(|l| l.live == Some(i));
            continue;
        }
        if s.hyperspace {
            continue;
        }
        let at = match prev {
            Some(p) => p + 1,
            None => out.iter().take_while(|l| l.objective.hyperspace).count(),
        };
        out.insert(
            at,
            Listed {
                objective: Objective {
                    complete: true,
                    ..s.clone()
                },
                live: None,
            },
        );
        prev = Some(at);
    }
    out
}

/// One wrapped line of an objective.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub text: String,
    pub x: i32,
    pub baseline: i32,
}

/// One objective's place: its click rectangle, marker centre and lines.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Its index in the listing, and in the game's list (None: destroyed, not clickable).
    pub index: usize,
    pub live: Option<usize>,
    pub complete: bool,
    pub rect: Rect,
    pub marker: (f32, f32),
    pub lines: Vec<Line>,
}

/// The hyperspace / end-mission button.
#[derive(Debug, Clone, PartialEq)]
pub struct Button {
    /// Its index in the listing and in the game's list.
    pub index: usize,
    pub live: usize,
    pub rect: Rect,
    /// The label in caps ("HYPERSPACE", "END MISSION").
    pub label: String,
    /// The small copy next to the collapsed header.
    pub compact: bool,
}

/// The panel's layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub scale: f32,
    pub expanded: bool,
    /// The header bar (the toggle's click area).
    pub header: Rect,
    /// The background (expanded only; the header collapsed).
    pub panel: Rect,
    /// The objectives other than the hyperspace entry.
    pub items: Vec<Item>,
    /// The hyperspace entry, while the game lists one.
    pub button: Option<Button>,
    /// The first line under the panel for the original's top-left columns.
    pub below: i32,
}

/// The objectives still to do (not the hyperspace entry).
pub fn open_count(objectives: &[Listed]) -> usize {
    objectives
        .iter()
        .filter(|o| !o.complete() && !o.objective.hyperspace)
        .count()
}

/// Greedy word wrap: the first line is `first` wide, the rest `rest`; a word wider than a
/// line gets a line to itself.
pub fn wrap(text: &str, first: f32, rest: f32, width: &dyn Fn(&str) -> f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let avail = if lines.is_empty() { first } else { rest };
        let candidate = if cur.is_empty() {
            word.to_string()
        } else {
            format!("{cur} {word}")
        };
        if cur.is_empty() || width(&candidate) <= avail {
            cur = candidate;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
    }
    if !cur.is_empty() || lines.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Lays the panel out for a canvas `height` lines tall, `top` pixels down (the overlay's
/// top), with `width` measuring text.
pub fn layout(
    height: i32,
    top: i32,
    objectives: &[Listed],
    expanded: bool,
    width: &dyn Fn(Text, &str) -> f32,
) -> Panel {
    let s = modern_hud::scale_for(height);
    let px = |v: f32| (v * s).round() as i32;
    let x0 = px(MARGIN);
    let x1 = x0 + px(WIDTH);
    let y0 = top + px(MARGIN);
    let header = if expanded {
        Rect::new(x0, y0, x1, y0 + px(HEADER_H))
    } else {
        // Collapsed: just the chevron, the label and the count (no wide click strip over the
        // 3D view).
        let lw = width(Text::Header, "OBJECTIVES").ceil() as i32;
        let cw = width(Text::Count, &open_count(objectives).to_string()).ceil() as i32;
        let right = x0 + px(LABEL_X) + lw + px(COLLAPSED_GAP) + cw + px(COUNT_RIGHT);
        Rect::new(x0, y0, right.min(x1), y0 + px(HEADER_H))
    };
    let hyper = objectives
        .iter()
        .enumerate()
        .find(|(_, o)| o.objective.hyperspace)
        .and_then(|(i, o)| o.live.map(|l| (i, l, o)));
    let button = hyper.map(|(index, live, o)| {
        let label = o.objective.text.to_uppercase();
        let rect = if expanded {
            let y = header.y1 + px(BUTTON_TOP);
            Rect::new(x0 + px(BUTTON_X0), y, x1 - px(BUTTON_X1), y + px(BUTTON_H))
        } else {
            let lw = width(Text::Compact, &label).ceil() as i32;
            let w = 2 * px(COMPACT_PAD)
                + (2.2 * COMPACT_GLYPH_R * s).ceil() as i32
                + px(COMPACT_GLYPH_GAP)
                + lw;
            let bx = header.x1 + px(COMPACT_GAP);
            let cy = (header.y0 + header.y1) / 2;
            let h = px(COMPACT_H);
            Rect::new(bx, cy - h / 2, bx + w, cy - h / 2 + h)
        };
        Button {
            index,
            live,
            rect,
            label,
            compact: !expanded,
        }
    });
    let mut items = Vec::new();
    let mut y = match &button {
        Some(b) if expanded => b.rect.y1 + px(BUTTON_GAP),
        _ => header.y1 + px(ITEMS_TOP),
    };
    if expanded {
        let text_x = x0 + px(TEXT_X);
        let avail = (x1 - px(TEXT_RIGHT) - text_x) as f32;
        for (index, o) in objectives.iter().enumerate() {
            if o.objective.hyperspace {
                continue;
            }
            let wrapped = wrap(&o.objective.text, avail, avail, &|t| width(Text::Item, t));
            let first = y + px(FIRST_BASELINE);
            let lines: Vec<Line> = wrapped
                .into_iter()
                .enumerate()
                .map(|(i, text)| Line {
                    text,
                    x: text_x,
                    baseline: first + i as i32 * px(LINE_H),
                })
                .collect();
            let h = lines.len() as i32 * px(LINE_H) + px(4.0);
            items.push(Item {
                index,
                live: o.live,
                complete: o.complete(),
                rect: Rect::new(x0, y, x1, y + h),
                marker: ((x0 + px(MARKER_X)) as f32, first as f32 - 5.0 * s),
                lines,
            });
            y += h + px(ITEM_GAP);
        }
        y = if items.is_empty() {
            y - px(BUTTON_GAP) + px(PAD_BOTTOM)
        } else {
            y - px(ITEM_GAP) + px(PAD_BOTTOM)
        };
    }
    let bottom = if expanded && (!items.is_empty() || button.is_some()) {
        y
    } else {
        header.y1
    };

    Panel {
        scale: s,
        expanded,
        header,
        panel: if expanded {
            Rect::new(x0, y0, x1, bottom)
        } else {
            header
        },
        items,
        button,
        below: bottom + px(BELOW_GAP),
    }
}

/// The panel's state kept in the overlay: the objectives seen this mission (so destroyed ones
/// stay listed), how many were last listed and when the header started flashing, and the
/// fold chosen over the open full-screen manager.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    seen: Vec<Objective>,
    mission: Option<i32>,
    last_time: f32,
    known: Option<usize>,
    flash_start: Option<f32>,
    last_clock: f32,
    /// When the hyperspace button appeared (its pulse).
    button_since: Option<f32>,
    /// The left button went down on the hyperspace button and is still held.
    pub pressed: bool,
    /// Expanded (true) or collapsed by a header click while a full-screen manager is open;
    /// None follows [`modern_hud::fold`]'s rule. Cleared when the manager closes.
    pub over_manager: Option<bool>,
}

impl PanelState {
    /// Notes the game's objectives at mission time `time`: the listing keeps the destroyed
    /// ones from now on. Another mission, or a mission clock that went back (a restart or a
    /// load), starts over.
    pub fn remember(&mut self, mission: i32, time: f32, live: &[Objective]) {
        if self.mission != Some(mission) || time + 1.0 < self.last_time {
            self.seen.clear();
        }
        self.mission = Some(mission);
        self.last_time = time;
        self.seen = listed(&self.seen, live)
            .into_iter()
            .map(|l| l.objective)
            .collect();
    }

    /// What the panel lists for the game's objectives `live`.
    pub fn listed(&self, live: &[Objective]) -> Vec<Listed> {
        listed(&self.seen, live)
    }

    /// Notes the objectives now listed: more than before starts the flash. A clock that went
    /// back (a new mission) starts over without one.
    pub fn note(&mut self, clock: f32, count: usize) {
        if clock < self.last_clock {
            self.known = None;
            self.flash_start = None;
        }
        self.last_clock = clock;
        if let Some(k) = self.known {
            if count > k {
                self.flash_start = Some(clock);
            }
        }
        self.known = Some(count);
    }

    /// Notes whether the hyperspace button is up at `clock`: its pulse starts when it
    /// appears (again after a clock that went back).
    pub fn note_button(&mut self, clock: f32, present: bool) {
        if !present {
            self.button_since = None;
            self.pressed = false;
        } else if self.button_since.is_none_or(|t| clock < t) {
            self.button_since = Some(clock);
        }
    }

    /// The button's pulse now, 0..1: a few brisk beats as it appears, fading into a slow,
    /// faint breath while it waits.
    pub fn pulse(&self, clock: f32) -> f32 {
        let Some(start) = self.button_since else {
            return 0.0;
        };
        let t = clock - start;
        if t < 0.0 {
            return 0.0;
        }
        let tau = std::f32::consts::TAU;
        if t < INTRO_SECONDS {
            let wave = 0.5 - 0.5 * (t * tau / INTRO_PERIOD).cos();
            let fade = 1.0 - (1.0 - BREATH) * t / INTRO_SECONDS;
            wave * fade
        } else {
            let u = t - INTRO_SECONDS;
            BREATH * (0.5 - 0.5 * (u * tau / BREATH_PERIOD).cos())
        }
    }

    /// The flash's strength now, 0..1.
    pub fn flash(&self, clock: f32) -> f32 {
        let Some(start) = self.flash_start else {
            return 0.0;
        };
        let t = clock - start;
        if !(0.0..FLASH_SECONDS).contains(&t) {
            return 0.0;
        }
        let wave = 0.5 + 0.5 * (t * std::f32::consts::TAU / FLASH_PERIOD).cos();
        wave * (1.0 - t / FLASH_SECONDS)
    }
}

/// Whether the panel is expanded (the options file).
pub fn expanded(fe: &Fe) -> bool {
    fe.game.options.get_bool(EXPANDED_KEY)
}

/// The header was clicked: collapse or expand. Over a full-screen manager the choice lasts
/// until the manager closes; otherwise it is remembered in the options file.
pub fn toggle(fe: &mut Fe) {
    let (w, h) = fe.screen_size();
    let shown = panel(fe, w, h, &fe.game.ig.view).is_some_and(|p| p.expanded);
    if modern_hud::manager_area(fe, w, h).is_some() {
        fe.game.ig.overlay.modern_objectives.over_manager = Some(!shown);
    } else {
        fe.game.set_option_now(EXPANDED_KEY, i32::from(!shown));
    }
}

/// Measures a kind of text in pixels.
type Measure = Box<dyn Fn(Text, &str) -> f32>;

/// Measures text with Segoe UI, or with the game's font without it.
fn measurer(pen: Option<Pen>, s: f32) -> Measure {
    Box::new(move |kind, t| {
        let Some(p) = &pen else { return 0.0 };
        match kind {
            Text::Item => p.measure(Face::Semibold, ITEM_PX * s, 0.0, t),
            Text::Header => p.measure(Face::Bold, HEADER_PX * s, HEADER_SPACING * s, t),
            Text::Count => p.measure(Face::Bold, COUNT_PX * s, 0.0, t),
            Text::Button => p.measure(Face::Bold, BUTTON_PX * s, BUTTON_SPACING * s, t),
            Text::Compact => p.measure(Face::Bold, COMPACT_PX * s, COMPACT_SPACING * s, t),
        }
    })
}

/// The panel for the current frame, or None when nothing is listed. Folded over a
/// full-screen manager when expanded it would cover the manager.
pub fn panel(fe: &Fe, width: i32, height: i32, view: &HostView) -> Option<Panel> {
    let st = &fe.game.ig.overlay.modern_objectives;
    let objectives = st.listed(&view.objectives);
    if objectives.is_empty() {
        return None;
    }
    let s = modern_hud::scale_for(height);
    let m = measurer(modern_hud::pen(fe), s);
    let top = fe.game.ig.overlay.top;
    let full = layout(height, top, &objectives, true, &*m);
    let saved = expanded(fe);
    let area = modern_hud::manager_area(fe, width, height);
    if modern_hud::fold(saved, area, st.over_manager, full.panel) {
        return Some(full);
    }
    Some(layout(height, top, &objectives, false, &*m))
}

/// Where the original's top-left columns (player names, construction, research) start: the
/// overlay's top, or under the panel while it is up.
pub fn left_top(fe: &Fe, width: i32, height: i32, view: &HostView, shown: bool) -> i32 {
    let top = fe.game.ig.overlay.top;
    if !shown || !modern_hud::enabled(&fe.game.options) {
        return top;
    }
    panel(fe, width, height, view).map_or(top, |p| p.below.max(top))
}

fn blend_cov(canvas: &mut Canvas, x: i32, y: i32, c: Color, cov: f32) {
    if cov > 0.0 {
        canvas.blend(x, y, modern_hud::with_alpha(c, cov * c[3] as f32 / 255.0));
    }
}

/// A diamond marker: filled, or a ring `line` pixels wide.
fn diamond(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, line: Option<f32>, c: Color) {
    let n = (r + 2.0).ceil() as i32;
    for y in (cy as i32 - n)..=(cy as i32 + n) {
        for x in (cx as i32 - n)..=(cx as i32 + n) {
            let d = (x as f32 + 0.5 - cx).abs() + (y as f32 + 0.5 - cy).abs();
            let k = std::f32::consts::FRAC_1_SQRT_2;
            let outer = ((r - d) * k + 0.5).clamp(0.0, 1.0);
            let cov = match line {
                None => outer,
                Some(w) => outer * (1.0 - ((r - w / k - d) * k + 0.5).clamp(0.0, 1.0)),
            };
            blend_cov(canvas, x, y, c, cov);
        }
    }
}

/// The chevron: a small triangle pointing down (expanded) or right (collapsed).
pub(crate) fn chevron(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, down: bool, c: Color) {
    let pts = if down {
        [
            (cx - r, cy - r * 0.5),
            (cx + r, cy - r * 0.5),
            (cx, cy + r * 0.6),
        ]
    } else {
        [
            (cx - r * 0.5, cy - r),
            (cx - r * 0.5, cy + r),
            (cx + r * 0.6, cy),
        ]
    };
    let edge = |a: (f32, f32), b: (f32, f32), p: (f32, f32)| {
        (b.0 - a.0) * (p.1 - a.1) - (b.1 - a.1) * (p.0 - a.0)
    };
    let n = (r + 2.0).ceil() as i32;
    for y in (cy as i32 - n)..=(cy as i32 + n) {
        for x in (cx as i32 - n)..=(cx as i32 + n) {
            let mut hits = 0;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = (
                        x as f32 + (sx as f32 + 0.5) / 4.0,
                        y as f32 + (sy as f32 + 0.5) / 4.0,
                    );
                    let e = [
                        edge(pts[0], pts[1], p),
                        edge(pts[1], pts[2], p),
                        edge(pts[2], pts[0], p),
                    ];
                    if e.iter().all(|&v| v >= 0.0) || e.iter().all(|&v| v <= 0.0) {
                        hits += 1;
                    }
                }
            }
            blend_cov(canvas, x, y, c, hits as f32 / 16.0);
        }
    }
}

/// What the hyperspace button shows this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct ButtonState {
    /// The pointer is on it.
    pub hover: bool,
    /// The left button went down on it and is held.
    pub pressed: bool,
    /// The game is paused: the click does nothing (dimmed, no pulse).
    pub paused: bool,
    /// [`PanelState::pulse`].
    pub pulse: f32,
}

/// A soft gold glow around `r`: rings fading outwards.
fn glow(canvas: &mut Canvas, r: Rect, radius: f32, spread: i32, strength: f32) {
    if strength <= 0.0 {
        return;
    }
    for i in 1..=spread {
        let t = 1.0 - i as f32 / (spread + 1) as f32;
        let ring = Rect::new(r.x0 - i, r.y0 - i, r.x1 + i, r.y1 + i);
        mm::rounded_outline(
            canvas,
            ring,
            radius + i as f32,
            1.0,
            mm::alpha(GOLD, strength * 0.45 * t * t),
        );
    }
}

/// The hyperspace glyph: two arrowheads pointing right, `2.2 r` wide from `x`.
fn hyper_glyph(canvas: &mut Canvas, x: f32, cy: f32, r: f32, c: Color) {
    chevron(canvas, x + 0.5 * r, cy, r, false, c);
    chevron(canvas, x + 1.6 * r, cy, r, false, mm::alpha(c, 0.7));
}

/// Draws the hyperspace / end-mission button.
fn draw_button(canvas: &mut Canvas, pen: &Pen, s: f32, b: &Button, st: &ButtonState) {
    let r = b.rect;
    let px = |v: f32| (v * s).round() as i32;
    let radius = 3.0 * s;
    let live = !st.paused;
    let hover = live && st.hover;
    let down = live && st.pressed && st.hover;
    let pulse = if live { st.pulse } else { 0.0 };
    // The glow: the pulse's, brighter under the pointer.
    let spread = px(if b.compact { 5.0 } else { 8.0 }).max(2);
    let strength = if !live {
        0.0
    } else if hover {
        0.75
    } else {
        0.3 + 0.55 * pulse
    };
    glow(canvas, r, radius, spread, strength);
    let bg = if down {
        [255, 200, 0, 170]
    } else if !live {
        [255, 255, 255, 14]
    } else if hover {
        [255, 200, 0, 70]
    } else {
        mm::lerp([255, 200, 0, 34], [255, 200, 0, 58], pulse)
    };
    mm::rounded(canvas, r, radius, [8, 10, 14, 150]);
    mm::rounded(canvas, r, radius, bg);
    let border = if !live {
        [255, 255, 255, 60]
    } else if hover || down {
        GOLD
    } else {
        mm::lerp(mm::alpha(GOLD, 0.75), GOLD, pulse)
    };
    let line = (1.5 * s).max(1.0);
    mm::rounded_outline(canvas, r, radius, line, border);
    // A light along the top edge.
    if live && !down {
        mm::hline(
            canvas,
            r.x0 + px(6.0),
            r.x1 - px(6.0),
            r.y0 + line.ceil() as i32,
            [255, 255, 255, if hover { 50 } else { 22 }],
        );
    }
    let (tpx, sp, gr, gap) = if b.compact {
        (
            COMPACT_PX * s,
            COMPACT_SPACING * s,
            COMPACT_GLYPH_R * s,
            px(COMPACT_GLYPH_GAP),
        )
    } else {
        (
            BUTTON_PX * s,
            BUTTON_SPACING * s,
            GLYPH_R * s,
            px(GLYPH_GAP),
        )
    };
    let col = if down {
        mm::ON_GOLD
    } else if !live {
        mm::MUTED
    } else if hover {
        WHITE
    } else {
        modern_hud::LABEL
    };
    let lw = pen.advance(Face::Bold, tpx, sp, &b.label);
    let gw = (2.2 * gr).ceil() as i32;
    let total = gw + gap + lw;
    let shift = i32::from(down);
    let x = r.x0 + (r.width() - total) / 2 + shift;
    let baseline = mm::mid_baseline(r, tpx) + shift;
    let cy = (r.y0 + r.y1) as f32 / 2.0 + shift as f32;
    hyper_glyph(canvas, x as f32, cy, gr, col);
    let halo = (hover && !down).then_some((mm::alpha(GOLD, 0.6), 2.0 * s));
    pen.write(
        canvas,
        Face::Bold,
        tpx,
        sp,
        x + gw + gap,
        baseline,
        col,
        halo,
        &b.label,
    );
}

/// Draws the panel (the overlay calls this in place of the original list, and over the
/// full-screen managers). `clock` is the overlay's real-time clock (the flash), `mouse` the
/// pointer (the highlight).
pub fn draw(fe: &mut Fe, canvas: &mut Canvas, view: &HostView, clock: f32, mouse: (i32, i32)) {
    let st = &mut fe.game.ig.overlay.modern_objectives;
    let objectives = st.listed(&view.objectives);
    st.note(clock, objectives.len());
    let hyper = objectives
        .iter()
        .any(|o| o.objective.hyperspace && o.live.is_some());
    st.note_button(clock, hyper);
    let Some(p) = panel(fe, canvas.width, canvas.height, view) else {
        return;
    };
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let s = p.scale;
    let px = |v: f32| (v * s).round() as i32;
    let flash = fe.game.ig.overlay.modern_objectives.flash(clock);
    if p.expanded {
        canvas.blend_rect(p.panel, PANEL);
        canvas.fill_rect(
            Rect::new(
                p.panel.x0,
                p.panel.y0,
                p.panel.x0 + px(ACCENT_W).max(1),
                p.panel.y1,
            ),
            ACCENT,
        );
    }
    if flash > 0.0 {
        canvas.blend_rect(p.header, modern_hud::with_alpha(GOLD, 0.35 * flash));
    }
    let h = p.header;
    let baseline = h.y0 + px(HEADER_BASELINE);
    let hover_header = h.contains(mouse.0, mouse.1);
    let head_col = if hover_header { WHITE } else { GOLD };
    chevron(
        canvas,
        (h.x0 + px(CHEVRON_X)) as f32,
        baseline as f32 - 4.0 * s,
        CHEVRON_R * s,
        p.expanded,
        head_col,
    );
    let (hpx, hsp) = (HEADER_PX * s, HEADER_SPACING * s);
    let lx = h.x0 + px(LABEL_X);
    pen.write(
        canvas,
        Face::Bold,
        hpx,
        hsp,
        lx,
        baseline,
        head_col,
        None,
        "OBJECTIVES",
    );
    let open = open_count(&objectives).to_string();
    let cw = pen.advance(Face::Bold, COUNT_PX * s, 0.0, &open);
    let cx = h.x1 - px(COUNT_RIGHT) - cw;
    pen.write(
        canvas,
        Face::Bold,
        COUNT_PX * s,
        0.0,
        cx,
        baseline,
        head_col,
        None,
        &open,
    );
    if let Some(b) = &p.button {
        let st = &fe.game.ig.overlay.modern_objectives;
        let look = ButtonState {
            hover: b.rect.contains(mouse.0, mouse.1),
            pressed: st.pressed,
            paused: view.player.paused,
            pulse: st.pulse(clock),
        };
        draw_button(canvas, &pen, s, b, &look);
    }
    for item in &p.items {
        // Destroyed objectives are not the game's any more: no highlight, no click.
        let hover = item.live.is_some() && item.rect.contains(mouse.0, mouse.1);
        let col = match (item.complete, hover) {
            (true, false) => DONE,
            (true, true) => DONE_HOVER,
            (false, true) => GOLD,
            (false, false) => WHITE,
        };
        let (mx, my) = item.marker;
        if item.complete {
            diamond(canvas, mx, my, MARKER_R * s, None, col);
        } else {
            let line = (MARKER_LINE * s).max(1.0);
            diamond(canvas, mx, my, MARKER_R * s, Some(line), GOLD);
        }
        for l in &item.lines {
            pen.write(
                canvas,
                Face::Semibold,
                ITEM_PX * s,
                0.0,
                l.x,
                l.baseline,
                col,
                None,
                &l.text,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(text: &str, complete: bool, hyperspace: bool) -> Objective {
        Objective {
            text: text.into(),
            description: String::new(),
            hyperspace,
            primary: true,
            complete,
        }
    }

    fn live(objs: &[Objective]) -> Vec<Listed> {
        listed(&[], objs)
    }

    /// 7 pixels a character.
    fn mono(_: Text, t: &str) -> f32 {
        t.chars().count() as f32 * 7.0
    }

    #[test]
    fn wrap_is_greedy_and_keeps_long_words() {
        let w = |t: &str| t.len() as f32;
        assert_eq!(wrap("aa bb cc", 5.0, 5.0, &w), vec!["aa bb", "cc"]);
        assert_eq!(wrap("aa bb cc", 2.0, 8.0, &w), vec!["aa", "bb cc"]);
        assert_eq!(wrap("abcdefgh x", 4.0, 4.0, &w), vec!["abcdefgh", "x"]);
        assert_eq!(wrap("", 4.0, 4.0, &w), vec![""]);
    }

    #[test]
    fn layout_at_1080p() {
        let objs = [
            obj("Defend the Kuun-Lan", false, false),
            obj(
                "Harvest the asteroid field and bring the resources back",
                true,
                false,
            ),
            obj("Hyperspace", false, true),
        ];
        let p = layout(1080, 0, &live(&objs), true, &mono);
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.header, Rect::new(16, 16, 376, 46));
        // The hyperspace entry is the button at the top, whatever its place in the list.
        let b = p.button.as_ref().expect("button");
        assert_eq!((b.index, b.live, b.compact), (2, 2, false));
        assert_eq!(b.label, "HYPERSPACE");
        assert_eq!(b.rect, Rect::new(28, 52, 366, 84));
        assert!(b.rect.height() > 23, "taller than a row");
        assert_eq!(p.items.len(), 2);
        let a = &p.items[0];
        assert_eq!(a.rect, Rect::new(16, 92, 376, 92 + 23));
        assert_eq!(
            a.lines,
            vec![Line {
                text: "Defend the Kuun-Lan".into(),
                x: 48,
                baseline: 106
            }]
        );
        assert_eq!(a.marker, (33.0, 101.0));
        assert!(!a.complete);
        assert_eq!(a.live, Some(0));
        // The complete one: no tag, its text at the usual place, wrapped to the panel.
        let b = &p.items[1];
        assert!(b.complete);
        assert_eq!(b.rect.y0, a.rect.y1 + 6);
        assert_eq!(b.lines[0].x, 48);
        assert!(b.lines.len() >= 2);
        let avail = (376 - 14 - 48) as f32;
        for (i, l) in b.lines.iter().enumerate() {
            assert!(mono(Text::Item, &l.text) <= avail, "{l:?}");
            assert_eq!(l.baseline, b.lines[0].baseline + i as i32 * 19);
        }
        assert_eq!(b.rect.height(), b.lines.len() as i32 * 19 + 4);
        assert_eq!(p.panel, Rect::new(16, 16, 376, b.rect.y1 + 8));
        assert_eq!(p.below, p.panel.y1 + 8);
        // No hyperspace entry: no button, the list starts under the header.
        let q = layout(1080, 0, &live(&objs[..2]), true, &mono);
        assert!(q.button.is_none());
        assert_eq!(q.items[0].rect.y0, 48);
    }

    #[test]
    fn button_alone_and_end_mission_label() {
        let objs = [obj("End Mission", false, true)];
        let p = layout(1080, 0, &live(&objs), true, &mono);
        assert!(p.items.is_empty());
        let b = p.button.as_ref().unwrap();
        assert_eq!(b.label, "END MISSION");
        assert_eq!(
            p.panel.y1,
            b.rect.y1 + 8,
            "the panel closes under the button"
        );
        // Collapsed: a compact copy right of the header, inside its height, not over it.
        let c = layout(1080, 0, &live(&objs), false, &mono);
        let cb = c.button.as_ref().unwrap();
        assert!(cb.compact);
        assert_eq!(c.header, Rect::new(16, 16, 16 + 28 + 70 + 10 + 7 + 12, 46));
        assert_eq!(cb.rect.x0, c.header.x1 + 6);
        assert!(cb.rect.y0 > c.header.y0 && cb.rect.y1 < c.header.y1);
        assert_eq!(cb.rect.height(), 22);
        // Pad, glyph (2.2 x 3.5 = 7.7 -> 8), gap, label (11 x 7), pad.
        assert_eq!(cb.rect.width(), 10 + 8 + 6 + 77 + 10);
        assert!(!modern_hud::overlaps(cb.rect, c.header));
        assert_eq!(c.below, c.header.y1 + 8);
        // A destroyed hyperspace entry is never listed, so never a button.
        let gone = listed(&objs, &[]);
        assert!(layout(1080, 0, &gone, true, &mono).button.is_none());
    }

    #[test]
    fn the_button_pulses_then_breathes() {
        let mut st = PanelState::default();
        st.note_button(1.0, false);
        assert_eq!(st.pulse(1.5), 0.0, "no button, no pulse");
        st.note_button(2.0, true);
        st.note_button(2.5, true);
        assert!(st.pulse(2.0) < 0.01, "starts from rest");
        assert!(st.pulse(2.5) > 0.85, "the first beat is near full");
        assert!(st.pulse(2.0 + INTRO_SECONDS - 0.5) < 0.7, "the beats fade");
        let breath = (0..200)
            .map(|i| st.pulse(2.0 + INTRO_SECONDS + i as f32 * 0.05))
            .fold(0.0f32, f32::max);
        assert!(breath > 0.3 && breath <= BREATH + 1e-4, "{breath}");
        st.pressed = true;
        st.note_button(20.0, false);
        assert!(!st.pressed, "gone: the press is dropped");
        assert_eq!(st.pulse(20.5), 0.0);
        st.note_button(21.0, true);
        assert!(st.pulse(21.5) > 0.85, "back: it pulses again");
        // A clock that went back (a restart) starts it over.
        st.note_button(0.5, true);
        assert!(st.pulse(1.0) > 0.85);
    }

    #[test]
    fn layout_collapsed_scaled_and_below_the_portrait() {
        let objs = [obj("Defend the Kuun-Lan", false, false)];
        let p = layout(1080, 75, &live(&objs), false, &mono);
        assert!(p.items.is_empty());
        // Chevron and label (28 + 70), gap, count (7), right pad.
        assert_eq!(p.header, Rect::new(16, 91, 16 + 28 + 70 + 10 + 7 + 12, 121));
        assert_eq!(p.panel, p.header, "collapsed: no wide panel to click");
        assert_eq!(p.below, 129);
        let q = layout(768, 0, &live(&objs), true, &mono);
        let s = 768.0 / 1080.0;
        assert!((q.scale - s).abs() < 1e-6);
        assert_eq!(q.header, Rect::new(11, 11, 11 + 256, 11 + 21));
    }

    #[test]
    fn open_count_skips_done_and_hyperspace() {
        let objs = [
            obj("a", false, false),
            obj("b", true, false),
            obj("c", false, false),
            obj("Hyperspace", false, true),
        ];
        assert_eq!(open_count(&live(&objs)), 2);
        // A destroyed one counts as done.
        let seen = [obj("gone", false, false)];
        assert_eq!(open_count(&listed(&seen, &objs)), 2);
    }

    #[test]
    fn destroyed_objectives_stay_listed_in_place() {
        let a = obj("Harvest", false, false);
        let b = obj("Build a processor", false, false);
        let c = obj("Defend", false, false);
        let hyper = obj("Hyperspace", false, true);
        let mut st = PanelState::default();
        st.remember(1, 10.0, &[a.clone(), b.clone(), c.clone()]);
        // The script destroys the middle one: it stays, complete and not the game's.
        let now = [a.clone(), c.clone()];
        st.remember(1, 20.0, &now);
        let l = st.listed(&now);
        let texts: Vec<&str> = l.iter().map(|x| x.objective.text.as_str()).collect();
        assert_eq!(texts, ["Harvest", "Build a processor", "Defend"]);
        assert_eq!(
            l.iter().map(|x| x.live).collect::<Vec<_>>(),
            [Some(0), None, Some(1)]
        );
        assert!(l[1].complete() && !l[0].complete() && !l[2].complete());
        // The first goes too, and the hyperspace entry comes in at the top (the game's
        // order): the destroyed first one goes under it, then the rest keep their places.
        let now = [hyper.clone(), c.clone()];
        st.remember(1, 30.0, &now);
        let l = st.listed(&now);
        let texts: Vec<&str> = l.iter().map(|x| x.objective.text.as_str()).collect();
        assert_eq!(
            texts,
            ["Hyperspace", "Harvest", "Build a processor", "Defend"]
        );
        assert_eq!(
            l.iter().map(|x| x.live).collect::<Vec<_>>(),
            [Some(0), None, None, Some(1)]
        );
        assert!(!l[0].complete(), "the hyperspace entry is never complete");
        // Marked complete by the game (ObjectiveSet): still the game's, so still clickable.
        let done = Objective {
            complete: true,
            ..c.clone()
        };
        let now = [hyper.clone(), done.clone()];
        st.remember(1, 31.0, &now);
        let l = st.listed(&now);
        assert_eq!(l[3].live, Some(1));
        assert!(l[3].complete());
        // A destroyed hyperspace entry is dropped, not shown complete.
        st.remember(1, 32.0, std::slice::from_ref(&done));
        let l = st.listed(std::slice::from_ref(&done));
        assert!(l.iter().all(|x| !x.objective.hyperspace));
        assert_eq!(l.len(), 3);
        // Another mission, or a clock that went back, starts over.
        st.remember(2, 40.0, std::slice::from_ref(&a));
        assert_eq!(st.listed(std::slice::from_ref(&a)).len(), 1);
        st.remember(2, 50.0, &[a.clone(), b.clone()]);
        st.remember(2, 5.0, std::slice::from_ref(&a));
        assert_eq!(st.listed(std::slice::from_ref(&a)).len(), 1);
    }

    #[test]
    fn listing_matches_duplicates_one_for_one() {
        let x = obj("Same", false, false);
        let seen = [x.clone(), x.clone()];
        let l = listed(&seen, std::slice::from_ref(&x));
        assert_eq!(l.len(), 2);
        assert_eq!(l[0].live, Some(0));
        assert_eq!(l[1].live, None);
        // A re-created objective is the game's again.
        let l = listed(&[obj("Back", true, false)], &[obj("Back", false, false)]);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].live, Some(0));
        assert!(!l[0].complete());
    }

    #[test]
    fn a_new_objective_flashes_the_header() {
        let mut st = PanelState::default();
        st.note(1.0, 2);
        assert_eq!(st.flash(1.2), 0.0, "the first sighting does not flash");
        st.note(2.0, 3);
        assert!(st.flash(2.05) > 0.5, "bright at once");
        assert!(st.flash(2.4) < 0.1, "then pulses");
        assert!(st.flash(2.8) > 0.3);
        assert_eq!(st.flash(2.0 + FLASH_SECONDS + 0.1), 0.0);
        st.note(5.0, 2);
        assert_eq!(st.flash(5.4), 0.0, "removing one does not flash");
        // A new mission (the clock went back).
        st.note(0.5, 4);
        assert_eq!(st.flash(0.9), 0.0);
    }
}
