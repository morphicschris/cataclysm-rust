//! Shared pieces of the modern HUD's list panels (the build queue, the system log and the
//! selected ships, next to the objectives and resources panels): the frame (translucent dark
//! background with the gold accent strip), the collapsible header with its chevron, the fold
//! and hide rule over the full-screen managers, a header flash, and [`arrange`], which places
//! every modern panel for a frame so that they stack without overlapping:
//!
//! * top left: objectives, then the build queue directly under it (it moves up and down with
//!   the objectives panel's height and fold), then research under the build queue (under the
//!   objectives while the queue is empty);
//! * bottom left: the system log, its header on the bottom edge;
//! * top right: resources, then the selected ships under it.
//!
//! The build queue, research and the selected ships drop the rows that do not fit above the next panel
//! (or the bottom of the screen) and say how many are left out; the log drops its oldest
//! lines. A panel with no room for a single row shows its header only.

use crate::canvas::{Canvas, Rect, WHITE};
use crate::ingame::host::HostView;
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::overlay;
use crate::ingame::{modern_build, modern_log, modern_research, modern_selected, Fe};

/// Gap between panels stacked in one corner, at 1080 lines (the objectives' and resources'
/// gap below them).
pub(crate) const STACK_GAP: f32 = 8.0;
/// Corner margin at 1080 lines.
pub(crate) const MARGIN: f32 = 16.0;

/// Measures text: (face, size in pixels, letter spacing, text) to a width in pixels.
pub type Measure<'a> = &'a dyn Fn(Face, f32, f32, &str) -> f32;

/// Measures with a pen (Segoe UI, or the game's font without it).
pub fn measure_with(pen: &Option<Pen>) -> impl Fn(Face, f32, f32, &str) -> f32 + '_ {
    move |face, px, spacing, t| {
        pen.as_ref()
            .map_or(0.0, |p| p.measure(face, px, spacing, t))
    }
}

/// Pixels at scale `s` for a length at 1080 lines.
pub(crate) fn px(s: f32, v: f32) -> i32 {
    (v * s).round() as i32
}

/// Width of a collapsed header at scale `s`: chevron and label, then (with `right`, the width
/// of a value on the header's right) a gap and the value, then the right padding.
pub fn header_width(measure: Measure, s: f32, label: &str, right: Option<&str>) -> i32 {
    let lw = measure(
        Face::Bold,
        obj::HEADER_PX * s,
        obj::HEADER_SPACING * s,
        label,
    );
    let mut w = px(s, obj::LABEL_X) + lw.ceil() as i32 + px(s, obj::COUNT_RIGHT);
    if let Some(r) = right {
        w += px(s, obj::COLLAPSED_GAP)
            + measure(Face::Bold, obj::COUNT_PX * s, 0.0, r).ceil() as i32;
    }
    w
}

/// Whether a panel shows expanded (`Some(true)`), as its header only (`Some(false)`), or not
/// at all (`None`): as the options file says (`saved`), except over a full-screen manager
/// (`manager`, the part of it to keep clear), where it folds when expanded it would cover the
/// manager (unless its header was clicked while the manager is open, `over_manager`), and
/// hides when even its header would.
pub fn shown_state(
    saved: bool,
    manager: Option<Rect>,
    over_manager: Option<bool>,
    expanded: Rect,
    collapsed: Rect,
) -> Option<bool> {
    if modern_hud::fold(saved, manager, over_manager, expanded) {
        return Some(true);
    }
    match manager {
        Some(m) if modern_hud::overlaps(collapsed, m) => None,
        _ => Some(false),
    }
}

/// A header flash (a new entry while the panel is folded): bright at once, pulsing, gone
/// after [`obj::FLASH_SECONDS`].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Flash {
    start: Option<f32>,
}

impl Flash {
    pub fn trigger(&mut self, clock: f32) {
        self.start = Some(clock);
    }

    pub fn clear(&mut self) {
        self.start = None;
    }

    /// The flash's strength at `clock`, 0..1.
    pub fn strength(&self, clock: f32) -> f32 {
        let Some(start) = self.start else {
            return 0.0;
        };
        let t = clock - start;
        if !(0.0..obj::FLASH_SECONDS).contains(&t) {
            return 0.0;
        }
        let wave = 0.5 + 0.5 * (t * std::f32::consts::TAU / 0.8).cos();
        wave * (1.0 - t / obj::FLASH_SECONDS)
    }
}

/// The background and accent strip of an expanded panel.
pub(crate) fn paint_frame(canvas: &mut Canvas, panel: Rect, s: f32) {
    canvas.blend_rect(panel, obj::PANEL);
    let accent = Rect::new(
        panel.x0,
        panel.y0,
        panel.x0 + px(s, obj::ACCENT_W).max(1),
        panel.y1,
    );
    canvas.fill_rect(accent, obj::ACCENT);
}

/// A header: the flash (if any), the chevron, the label and, with `right`, a value on the
/// right. Gold, white under the pointer.
#[allow(clippy::too_many_arguments)]
pub(crate) fn paint_header(
    canvas: &mut Canvas,
    pen: &Pen,
    header: Rect,
    s: f32,
    label: &str,
    expanded: bool,
    hover: bool,
    right: Option<&str>,
    flash: f32,
) {
    if flash > 0.0 {
        canvas.blend_rect(header, modern_hud::with_alpha(obj::GOLD, 0.35 * flash));
    }
    let col = if hover { WHITE } else { obj::GOLD };
    let baseline = header.y0 + px(s, obj::HEADER_BASELINE);
    obj::chevron(
        canvas,
        (header.x0 + px(s, obj::CHEVRON_X)) as f32,
        baseline as f32 - 4.0 * s,
        obj::CHEVRON_R * s,
        expanded,
        col,
    );
    pen.write(
        canvas,
        Face::Bold,
        obj::HEADER_PX * s,
        obj::HEADER_SPACING * s,
        header.x0 + px(s, obj::LABEL_X),
        baseline,
        col,
        None,
        label,
    );
    if let Some(r) = right {
        let w = pen.advance(Face::Bold, obj::COUNT_PX * s, 0.0, r);
        let x = header.x1 - px(s, obj::COUNT_RIGHT) - w;
        pen.write(
            canvas,
            Face::Bold,
            obj::COUNT_PX * s,
            0.0,
            x,
            baseline,
            col,
            None,
            r,
        );
    }
}

/// `s` shortened with "..." to fit `avail` pixels.
pub fn ellipsize(measure: &dyn Fn(&str) -> f32, s: &str, avail: f32) -> String {
    if measure(s) <= avail {
        return s.to_string();
    }
    let chars: Vec<char> = s.chars().collect();
    for n in (0..chars.len()).rev() {
        let t: String = chars[..n].iter().collect::<String>().trim_end().to_string() + "...";
        if measure(&t) <= avail {
            return t;
        }
    }
    String::new()
}

/// A thin bar's track, with nothing filled (the health bars' background).
pub(crate) fn track(canvas: &mut Canvas, bar: Rect) {
    canvas.blend_rect(bar, modern_hud::TRACK);
}

/// Every modern panel's place this frame (None: not shown).
#[derive(Debug, Clone, Default)]
pub struct Arrangement {
    pub objectives: Option<obj::Panel>,
    pub resources: Option<modern_hud::Layout>,
    pub build: Option<modern_build::Layout>,
    pub research: Option<modern_research::Layout>,
    /// The research panel's rows (the topics, then the ones just finished).
    pub research_entries: Vec<modern_research::Entry>,
    pub log: Option<modern_log::Layout>,
    pub selected: Option<modern_selected::Layout>,
}

impl Arrangement {
    /// Every panel's rectangle (for taking the clicks on them).
    pub fn rects(&self) -> Vec<Rect> {
        let mut v = Vec::new();
        v.extend(self.objectives.as_ref().map(|p| p.panel));
        // Collapsed, the hyperspace button sits outside the header.
        v.extend(
            self.objectives
                .as_ref()
                .and_then(|p| p.button.as_ref())
                .filter(|b| b.compact)
                .map(|b| b.rect),
        );
        v.extend(self.resources.as_ref().map(|p| p.panel));
        v.extend(self.build.as_ref().map(|p| p.panel));
        v.extend(self.research.as_ref().map(|p| p.panel));
        v.extend(self.log.as_ref().map(|p| p.panel));
        v.extend(self.selected.as_ref().map(|p| p.panel));
        v
    }

    /// The first free line under the top-left panels (objectives, build queue and research),
    /// if any is up.
    pub fn left_below(&self) -> Option<i32> {
        self.research
            .as_ref()
            .map(|r| r.below)
            .or(self.build.as_ref().map(|b| b.below))
            .or(self.objectives.as_ref().map(|o| o.below))
    }
}

/// Places the modern panels for a `width` x `height` frame: none with `--hud original` or in
/// a cutscene; each one only while its overlay panel is shown in the current view
/// (`MainOverlay.script`: OBJECTIVES, PLAYERINFO, CONSTRUCTION, RESEARCH, GAMEMESSAGE,
/// SELECTED).
pub fn arrange(fe: &Fe, width: i32, height: i32, view: &HostView) -> Arrangement {
    let st = &fe.game.ig.overlay;
    if !modern_hud::enabled(&fe.game.options) || st.cutscene {
        return Arrangement::default();
    }
    let s = modern_hud::scale_for(height);
    let pen = modern_hud::pen(fe);
    let m = measure_with(&pen);
    let manager = modern_hud::manager_area(fe, width, height);
    let mut a = Arrangement::default();
    if st.shown(view, overlay::OBJECTIVES) {
        a.objectives = obj::panel(fe, width, height, view);
    }
    if st.shown(view, overlay::PLAYERINFO) {
        a.resources = Some(modern_hud::panel(fe, width, height, view));
    }
    let left_top = a
        .objectives
        .as_ref()
        .map_or(st.top + px(s, MARGIN), |p| p.below);
    let entries = if st.shown(view, overlay::CONSTRUCTION) {
        modern_build::entries(view)
    } else {
        Vec::new()
    };
    let research = if st.shown(view, overlay::RESEARCH) {
        modern_research::entries(view, &st.modern_research, st.clock())
    } else {
        Vec::new()
    };
    // Room kept under the build queue for the research panel's header.
    let research_reserve = if research.is_empty() {
        0
    } else {
        modern_research::header_height(s) + px(s, STACK_GAP)
    };
    // The log first, keeping room above it for the build queue's header and one row, and the
    // research panel's header.
    if st.shown(view, overlay::GAMEMESSAGE) {
        let reserve = if entries.is_empty() {
            0
        } else {
            modern_build::min_height(s) + px(s, STACK_GAP)
        } + research_reserve;
        a.log = modern_log::arrange(fe, width, height, view, left_top + reserve, manager, &m);
    }
    let limit = a
        .log
        .as_ref()
        .map_or(height - px(s, MARGIN), |l| l.panel.y0 - px(s, STACK_GAP));
    if !entries.is_empty() {
        let build_limit = limit - research_reserve;
        a.build = modern_build::arrange(fe, height, left_top, &entries, build_limit, manager, &m);
    }
    if !research.is_empty() {
        let top = a.build.as_ref().map_or(left_top, |b| b.below);
        a.research = modern_research::arrange(fe, height, top, research.len(), limit, manager, &m);
        a.research_entries = research;
    }
    if st.shown(view, overlay::SELECTED) && !view.selection.is_empty() {
        let top = a
            .resources
            .as_ref()
            .map_or(st.top + px(s, MARGIN), |r| r.bottom);
        let limit = height - px(s, MARGIN);
        a.selected =
            modern_selected::arrange(fe, width, height, top, &view.selection, limit, manager, &m);
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn mono(_: Face, px: f32, spacing: f32, t: &str) -> f32 {
        let n = t.chars().count() as f32;
        n * px * 0.5 + (n - 1.0).max(0.0) * spacing
    }

    #[test]
    fn collapsed_header_width() {
        // 1080 lines: 28 to the label, 11 letters of 6 px with 10 gaps of 2, 12 padding.
        assert_eq!(header_width(&mono, 1.0, "BUILD QUEUE", None), 28 + 86 + 12);
        // A value adds the gap and its width.
        assert_eq!(
            header_width(&mono, 1.0, "SELECTED", Some("12")),
            28 + 62 + 12 + 10 + 13
        );
    }

    #[test]
    fn shown_state_folds_then_hides_over_a_manager() {
        let e = Rect::new(0, 0, 100, 100);
        let c = Rect::new(0, 0, 100, 20);
        assert_eq!(shown_state(true, None, None, e, c), Some(true));
        assert_eq!(shown_state(false, None, None, e, c), Some(false));
        // Expanded would reach the manager: folded; the header too: hidden.
        let m = Rect::new(0, 50, 200, 200);
        assert_eq!(shown_state(true, Some(m), None, e, c), Some(false));
        let m2 = Rect::new(0, 10, 200, 200);
        assert_eq!(shown_state(true, Some(m2), None, e, c), None);
        // Unfolded on demand over the manager.
        assert_eq!(shown_state(true, Some(m), Some(true), e, c), Some(true));
        // Clear of the manager: the options file.
        let far = Rect::new(500, 500, 600, 600);
        assert_eq!(shown_state(true, Some(far), None, e, c), Some(true));
    }

    #[test]
    fn flash_pulses_and_ends() {
        let mut f = Flash::default();
        assert_eq!(f.strength(1.0), 0.0);
        f.trigger(1.0);
        assert!(f.strength(1.05) > 0.5);
        assert!(f.strength(1.4) < 0.1);
        assert_eq!(f.strength(1.0 + obj::FLASH_SECONDS + 0.1), 0.0);
        f.clear();
        assert_eq!(f.strength(1.05), 0.0);
    }

    #[test]
    fn ellipsize_fits() {
        let w = |t: &str| t.chars().count() as f32;
        assert_eq!(ellipsize(&w, "short", 10.0), "short");
        assert_eq!(ellipsize(&w, "a long line of text", 10.0), "a long...");
        assert_eq!(ellipsize(&w, "abc", 1.0), "");
    }
}
