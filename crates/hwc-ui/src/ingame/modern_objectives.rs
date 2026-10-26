//! The modern HUD's objectives panel (part of the user-requested modern HUD, `--hud`): a
//! top-left panel in place of the original's bottom-left "Objectives:" list
//! (`moDrawMissionInfo`). A header bar reads "OBJECTIVES" with a chevron and the number of
//! objectives still open; clicking it collapses or expands the panel (remembered in the
//! options file as `hwcObjectivesExpanded`). Expanded, a translucent dark panel with a gold
//! accent strip lists every objective with a diamond marker: hollow and white while
//! incomplete, filled and dimmed with a "COMPLETE" tag once done (the original's
//! "COMPLETE - " prefix). Long text wraps. Lines are clickable as in the original: the
//! hyperspace / end-mission entry jumps, the others open the objectives screen; the line
//! under the pointer is highlighted. A newly added objective flashes the header (the
//! original draws no attention to it, so the panel does not expand by itself).
//!
//! Like the original's top-left text it starts below the speech band and portrait while one
//! is up (the overlay's top, `moSetTop`), and it hides with the rest of the overlay.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::draw;
use crate::ingame::host::{HostView, Objective};
use crate::ingame::modern_hud::{self, Face, SHADOW};
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcObjectivesExpanded";

/// Panel background, accent and header text.
pub const PANEL: Color = [8, 10, 14, 153];
pub const ACCENT: Color = [0xFF, 0xC8, 0x00, 0xFF];
pub const GOLD: Color = [0xFF, 0xC8, 0x00, 0xFF];
const DONE_TEXT: Color = [255, 255, 255, 135];
const DONE_TAG: Color = [0xFF, 0xC8, 0x00, 150];
const DONE_MARKER: Color = [0xFF, 0xC8, 0x00, 170];
/// How long the header flashes after an objective is added, and one flash's length.
pub const FLASH_SECONDS: f32 = 2.4;
const FLASH_PERIOD: f32 = 0.8;

/// Reference layout, in pixels at 1080 lines.
const REF_HEIGHT: f32 = 1080.0;
const MARGIN: f32 = 16.0;
const WIDTH: f32 = 360.0;
const ACCENT_W: f32 = 3.0;
const HEADER_H: f32 = 30.0;
const HEADER_BASELINE: f32 = 20.0;
const CHEVRON_X: f32 = 15.0;
const CHEVRON_R: f32 = 4.5;
const LABEL_X: f32 = 28.0;
const COUNT_RIGHT: f32 = 12.0;
const HEADER_PX: f32 = 12.0;
const HEADER_SPACING: f32 = 2.0;
const COUNT_PX: f32 = 13.0;
/// Collapsed: the gap between the label and the count.
const COLLAPSED_GAP: f32 = 10.0;
const ITEM_PX: f32 = 14.0;
const TAG_PX: f32 = 10.0;
const TAG_SPACING: f32 = 1.5;
const TAG_GAP: f32 = 6.0;
const MARKER_X: f32 = 17.0;
const MARKER_R: f32 = 4.5;
const MARKER_LINE: f32 = 1.5;
const TEXT_X: f32 = 32.0;
const TEXT_RIGHT: f32 = 14.0;
const LINE_H: f32 = 19.0;
const FIRST_BASELINE: f32 = 14.0;
const ITEM_GAP: f32 = 6.0;
const ITEMS_TOP: f32 = 2.0;
const PAD_BOTTOM: f32 = 8.0;
/// Room left under the panel before the original's top-left columns start.
const BELOW_GAP: f32 = 8.0;

/// Which text a width is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Text {
    Item,
    Tag,
    /// The header's label and count.
    Header,
    Count,
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
    pub index: usize,
    pub rect: Rect,
    pub marker: (f32, f32),
    pub lines: Vec<Line>,
    /// The "COMPLETE" tag's pen start (complete objectives).
    pub tag: Option<(i32, i32)>,
}

/// The panel's layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Panel {
    pub scale: f32,
    pub expanded: bool,
    /// The header bar (the toggle's click area).
    pub header: Rect,
    /// The background (expanded only).
    pub panel: Rect,
    pub items: Vec<Item>,
    /// The first line under the panel for the original's top-left columns.
    pub below: i32,
}

/// The objectives listed: the hyperspace entry and the rest, in order.
pub fn open_count(objectives: &[Objective]) -> usize {
    objectives
        .iter()
        .filter(|o| !o.complete && !o.hyperspace)
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
    objectives: &[Objective],
    expanded: bool,
    width: &dyn Fn(Text, &str) -> f32,
) -> Panel {
    let s = (height.max(1) as f32 / REF_HEIGHT).max(0.35);
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
    let mut items = Vec::new();
    let mut y = header.y1 + px(ITEMS_TOP);
    if expanded {
        let text_x = x0 + px(TEXT_X);
        let avail = (x1 - px(TEXT_RIGHT) - text_x) as f32;
        for (index, o) in objectives.iter().enumerate() {
            let complete = o.complete && !o.hyperspace;
            let indent = if complete {
                width(Text::Tag, "COMPLETE") + TAG_GAP * s
            } else {
                0.0
            };
            let wrapped = wrap(&o.text, avail - indent, avail, &|t| width(Text::Item, t));
            let first = y + px(FIRST_BASELINE);
            let lines: Vec<Line> = wrapped
                .into_iter()
                .enumerate()
                .map(|(i, text)| Line {
                    text,
                    x: text_x + if i == 0 { indent.round() as i32 } else { 0 },
                    baseline: first + i as i32 * px(LINE_H),
                })
                .collect();
            let h = lines.len() as i32 * px(LINE_H) + px(4.0);
            items.push(Item {
                index,
                rect: Rect::new(x0, y, x1, y + h),
                marker: ((x0 + px(MARKER_X)) as f32, first as f32 - 5.0 * s),
                lines,
                tag: complete.then_some((text_x, first)),
            });
            y += h + px(ITEM_GAP);
        }
        y = y - px(ITEM_GAP) + px(PAD_BOTTOM);
    }
    let bottom = if expanded && !items.is_empty() {
        y
    } else {
        header.y1
    };
    Panel {
        scale: s,
        expanded,
        header,
        panel: Rect::new(x0, y0, x1, bottom),
        items,
        below: bottom + px(BELOW_GAP),
    }
}

/// The animation state kept in the overlay: how many objectives were last seen and when the
/// header started flashing.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    known: Option<usize>,
    flash_start: Option<f32>,
    last_clock: f32,
}

impl PanelState {
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

/// The header was clicked: collapse or expand, and remember it in the options file.
pub fn toggle(fe: &mut Fe) {
    let v = !expanded(fe);
    fe.game.set_option_now(EXPANDED_KEY, i32::from(v));
}

/// Measures text with Segoe UI, or with the game's font without it.
/// Measures a kind of text in pixels.
type Measure = Box<dyn Fn(Text, &str) -> f32>;

fn measurer(fe: &Fe, s: f32) -> Measure {
    if let Some(f) = modern_hud::fonts() {
        return Box::new(move |kind, t| match kind {
            Text::Item => modern_hud::text_width(&f.semibold, ITEM_PX * s, 0.0, t),
            Text::Tag => modern_hud::text_width(&f.bold, TAG_PX * s, TAG_SPACING * s, t),
            Text::Header => modern_hud::text_width(&f.bold, HEADER_PX * s, HEADER_SPACING * s, t),
            Text::Count => modern_hud::text_width(&f.bold, COUNT_PX * s, 0.0, t),
        });
    }
    let font = fe.resources().font(crate::ingame::overlay::FONT_TEXT);
    Box::new(move |_, t| font.as_ref().map_or(0, |f| draw::width(f, t)) as f32)
}

/// The panel for the current frame, or None when there are no objectives.
pub fn panel(fe: &Fe, width: i32, height: i32, view: &HostView) -> Option<Panel> {
    let _ = width;
    if view.objectives.is_empty() {
        return None;
    }
    let s = (height.max(1) as f32 / REF_HEIGHT).max(0.35);
    let m = measurer(fe, s);
    Some(layout(
        height,
        fe.game.ig.overlay.top,
        &view.objectives,
        expanded(fe),
        &*m,
    ))
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
fn chevron(canvas: &mut Canvas, cx: f32, cy: f32, r: f32, down: bool, c: Color) {
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

/// Draws text with its pen at `x` and its baseline at `baseline` (Segoe UI with a drop
/// shadow, or the game's font).
#[allow(clippy::too_many_arguments)]
fn text(
    fe: &Fe,
    canvas: &mut Canvas,
    face: Face,
    px: f32,
    spacing: f32,
    x: i32,
    baseline: i32,
    c: Color,
    s: &str,
) -> i32 {
    if let Some(f) = modern_hud::fonts() {
        let sp = modern_hud::sprite(f, face, px, spacing, s, 0.0);
        modern_hud::draw_sprite(canvas, &sp, x, baseline, c, None, 1.0);
        return sp.advance;
    }
    let Some(f) = fe.resources().font(crate::ingame::overlay::FONT_TEXT) else {
        return 0;
    };
    let h = crate::text::height(&f);
    draw::print_shadow(canvas, &f, x, baseline - h + 2, c, SHADOW, s)
}

fn text_width(fe: &Fe, face: Face, px: f32, spacing: f32, s: &str) -> i32 {
    if let Some(f) = modern_hud::fonts() {
        return modern_hud::sprite(f, face, px, spacing, s, 0.0).advance;
    }
    fe.resources()
        .font(crate::ingame::overlay::FONT_TEXT)
        .map_or(0, |f| draw::width(&f, s))
}

/// Draws the panel (the overlay calls this in place of the original list). `clock` is the
/// overlay's real-time clock (the flash), `mouse` the pointer (the highlight).
pub fn draw(fe: &mut Fe, canvas: &mut Canvas, view: &HostView, clock: f32, mouse: (i32, i32)) {
    let count = view.objectives.len();
    fe.game.ig.overlay.modern_objectives.note(clock, count);
    let Some(p) = panel(fe, canvas.width, canvas.height, view) else {
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
    text(
        fe,
        canvas,
        Face::Bold,
        HEADER_PX * s,
        HEADER_SPACING * s,
        h.x0 + px(LABEL_X),
        baseline,
        head_col,
        "OBJECTIVES",
    );
    let open = open_count(&view.objectives).to_string();
    let cw = text_width(fe, Face::Bold, COUNT_PX * s, 0.0, &open);
    text(
        fe,
        canvas,
        Face::Bold,
        COUNT_PX * s,
        0.0,
        h.x1 - px(COUNT_RIGHT) - cw,
        baseline,
        head_col,
        &open,
    );
    for item in &p.items {
        let Some(o) = view.objectives.get(item.index) else {
            continue;
        };
        let complete = o.complete && !o.hyperspace;
        let hover = item.rect.contains(mouse.0, mouse.1);
        let col = match (complete, o.hyperspace, hover) {
            (_, true, false) => GOLD,
            (_, true, true) => WHITE,
            (true, _, false) => DONE_TEXT,
            (_, _, true) => GOLD,
            _ => WHITE,
        };
        let (mx, my) = item.marker;
        if complete {
            diamond(canvas, mx, my, MARKER_R * s, None, DONE_MARKER);
        } else {
            let line = (MARKER_LINE * s).max(1.0);
            diamond(canvas, mx, my, MARKER_R * s, Some(line), GOLD);
        }
        if let Some((tx, tb)) = item.tag {
            text(
                fe,
                canvas,
                Face::Bold,
                TAG_PX * s,
                TAG_SPACING * s,
                tx,
                tb,
                DONE_TAG,
                "COMPLETE",
            );
        }
        for l in &item.lines {
            text(
                fe,
                canvas,
                Face::Semibold,
                ITEM_PX * s,
                0.0,
                l.x,
                l.baseline,
                col,
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

    /// 7 pixels a character, 50 for the tag.
    fn mono(kind: Text, t: &str) -> f32 {
        match kind {
            Text::Item | Text::Header | Text::Count => t.chars().count() as f32 * 7.0,
            Text::Tag => 50.0,
        }
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
        let p = layout(1080, 0, &objs, true, &mono);
        assert_eq!(p.scale, 1.0);
        assert_eq!(p.header, Rect::new(16, 16, 376, 46));
        assert_eq!(p.items.len(), 3);
        let a = &p.items[0];
        assert_eq!(a.rect, Rect::new(16, 48, 376, 48 + 23));
        assert_eq!(
            a.lines,
            vec![Line {
                text: "Defend the Kuun-Lan".into(),
                x: 48,
                baseline: 62
            }]
        );
        assert_eq!(a.marker, (33.0, 57.0));
        assert_eq!(a.tag, None);
        // The complete one: tag first, text after it, wrapped to the panel.
        let b = &p.items[1];
        assert_eq!(b.rect.y0, a.rect.y1 + 6);
        assert_eq!(b.tag, Some((48, b.rect.y0 + 14)));
        assert_eq!(b.lines[0].x, 48 + 56);
        assert!(b.lines.len() >= 2);
        let avail = (376 - 14 - 48) as f32;
        for (i, l) in b.lines.iter().enumerate() {
            let room = if i == 0 { avail - 56.0 } else { avail };
            assert!(mono(Text::Item, &l.text) <= room, "{l:?}");
            assert_eq!(l.baseline, b.lines[0].baseline + i as i32 * 19);
        }
        assert_eq!(b.rect.height(), b.lines.len() as i32 * 19 + 4);
        let c = &p.items[2];
        assert_eq!(c.tag, None, "the hyperspace entry is never 'complete'");
        assert_eq!(p.panel, Rect::new(16, 16, 376, c.rect.y1 + 8));
        assert_eq!(p.below, p.panel.y1 + 8);
    }

    #[test]
    fn layout_collapsed_scaled_and_below_the_portrait() {
        let objs = [obj("Defend the Kuun-Lan", false, false)];
        let p = layout(1080, 75, &objs, false, &mono);
        assert!(p.items.is_empty());
        // Chevron and label (28 + 70), gap, count (7), right pad.
        assert_eq!(p.header, Rect::new(16, 91, 16 + 28 + 70 + 10 + 7 + 12, 121));
        assert_eq!(p.panel.y1, p.header.y1);
        assert_eq!(p.below, 129);
        let q = layout(768, 0, &objs, true, &mono);
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
        assert_eq!(open_count(&objs), 2);
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
