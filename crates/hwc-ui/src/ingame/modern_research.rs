//! The modern HUD's research panel (part of the user-requested modern HUD, `--hud`): a
//! top-left panel directly under the build queue (under the objectives panel while the queue
//! is empty), in place of the original's research list (`moDrawResearchList`, the blue topic
//! names over their progress bars). It moves with the panels above it.
//!
//! Like the original it lists each technology being researched once, in technology order
//! (`rmGetResearchProgressList` walks the technologies and picks the player's topics), and is
//! not shown at all while nothing is being researched. Each row has the topic's name, the
//! time left (the list carries it, the original does not print it) and a thin glowing bar
//! with the progress. A topic whose lab is paused is greyed and tagged "PAUSED". A topic that
//! has just finished stays a few seconds as a "COMPLETE" row and fades out (the original
//! drops it at once). The original's list does nothing when clicked (only its scroll arrows
//! do), so the rows take the click without acting on it. The header "RESEARCH" folds the
//! panel (remembered in the options file as `hwcResearchExpanded`); folded, the header alone
//! is shown. Rows that do not fit above the system log are left out and counted on a last
//! "+N MORE" line.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::host::{HostView, LabStatus, TechId, TechState};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_panel::{self as mp, px, Measure};
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcResearchExpanded";
pub const HEADER_TEXT: &str = "RESEARCH";
pub const PAUSED_TEXT: &str = "PAUSED";
pub const COMPLETE_TEXT: &str = "COMPLETE";

/// The "PAUSED" tag (as the build queue's).
pub const PAUSED_COL: Color = [0xFF, 0xA8, 0x40, 0xFF];
/// The "COMPLETE" tag and the finished topic's bar.
pub const COMPLETE_COL: Color = [0x8C, 0xE8, 0x6A, 0xFF];
/// A finished topic's row: shown this long, then fading out over [`COMPLETE_FADE`].
pub const COMPLETE_HOLD: f32 = 2.5;
pub const COMPLETE_FADE: f32 = 1.5;
/// Research counts down eight times as fast in the tutorial (`rmUpdateResearch`).
const TUTORIAL_RATE: f32 = 8.0;

/// Reference layout, in pixels at 1080 lines (the build queue's).
const WIDTH: f32 = 360.0;
const ROW_X: f32 = 16.0;
const ROW_RIGHT: f32 = 14.0;
const ROWS_TOP: f32 = 2.0;
const ROW_H: f32 = 28.0;
const BASELINE: f32 = 14.0;
const BAR_TOP: f32 = 20.0;
const BAR_H: f32 = 3.0;
const ROW_GAP: f32 = 2.0;
const MORE_H: f32 = 18.0;
const MORE_BASELINE: f32 = 13.0;
const PAD_BOTTOM: f32 = 8.0;
const NAME_PX: f32 = 13.0;
const TAG_PX: f32 = 9.5;
const TAG_SPACING: f32 = 1.5;
const TAG_GAP: f32 = 10.0;
const MORE_PX: f32 = 11.0;
/// The bar glow's sigma.
const BAR_SIGMA: f32 = 3.0;

/// One row: a topic being researched, or one just finished.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub tech: TechId,
    pub name: String,
    /// 0..1.
    pub progress: f32,
    /// Its lab is paused.
    pub paused: bool,
    /// Seconds of game time left, if the technology's time is known.
    pub seconds_left: Option<f32>,
    /// A finished topic: the row's opacity, 0..1.
    pub complete: Option<f32>,
}

/// The topics being researched, as the original lists them: each technology once, in
/// technology order, with its labs' best progress; paused when none of its labs is running.
pub fn topics(view: &HostView) -> Vec<Entry> {
    let mut techs: Vec<TechId> = view
        .labs
        .iter()
        .filter(|l| matches!(l.status, LabStatus::Researching | LabStatus::Paused))
        .filter_map(|l| l.topic)
        .collect();
    techs.sort_unstable();
    techs.dedup();
    let rate = if view.player.tutorial {
        TUTORIAL_RATE
    } else {
        1.0
    };
    techs
        .into_iter()
        .map(|t| {
            let labs: Vec<_> = view.labs.iter().filter(|l| l.topic == Some(t)).collect();
            let progress = labs
                .iter()
                .map(|l| l.progress)
                .fold(0.0, f32::max)
                .clamp(0.0, 1.0);
            let paused = !labs.iter().any(|l| l.status == LabStatus::Researching);
            let tech = view.tech(t);
            let seconds_left = tech
                .filter(|k| k.time > 0.0)
                .map(|k| (1.0 - progress) * k.time / rate);
            let name = tech.map_or_else(|| format!("Topic {t}"), |k| k.name.clone());
            Entry {
                tech: t,
                name: name.to_uppercase(),
                progress,
                paused,
                seconds_left,
                complete: None,
            }
        })
        .collect()
}

/// "1:05" (minutes and seconds left, rounded up), "1:02:03" past the hour.
pub fn time_text(seconds: f32) -> String {
    let t = seconds.max(0.0).ceil() as u32;
    if t >= 3600 {
        format!("{}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60)
    } else {
        format!("{}:{:02}", t / 60, t % 60)
    }
}

/// A topic that has just finished.
#[derive(Debug, Clone, PartialEq)]
struct Done {
    tech: TechId,
    name: String,
    start: f32,
}

/// The panel's state kept in the overlay.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    /// Expanded (true) or collapsed by a header click while a full-screen manager is open;
    /// None follows the usual rule. Cleared when the manager closes.
    pub over_manager: Option<bool>,
    /// The topics seen last frame, and the mission they were in.
    last: Vec<(TechId, String)>,
    mission: Option<i32>,
    done: Vec<Done>,
}

impl PanelState {
    /// Called each frame with the real-time clock: a topic gone from the list with its
    /// technology now researched becomes a "COMPLETE" row (not one stopped by the player, nor
    /// across a change of mission).
    pub fn note(&mut self, clock: f32, view: &HostView) {
        let now: Vec<(TechId, String)> =
            topics(view).into_iter().map(|e| (e.tech, e.name)).collect();
        if self.mission != Some(view.player.mission) {
            self.mission = Some(view.player.mission);
            self.done.clear();
        } else {
            for (t, name) in &self.last {
                let finished = view
                    .tech(*t)
                    .is_some_and(|k| k.state == TechState::Researched);
                if finished
                    && !now.iter().any(|n| n.0 == *t)
                    && !self.done.iter().any(|d| d.tech == *t)
                {
                    self.done.push(Done {
                        tech: *t,
                        name: name.clone(),
                        start: clock,
                    });
                }
            }
        }
        self.done.retain(|d| {
            let age = clock - d.start;
            (0.0..COMPLETE_HOLD + COMPLETE_FADE).contains(&age)
                && !now.iter().any(|n| n.0 == d.tech)
        });
        self.last = now;
    }

    /// The finished topics' rows at `clock`, oldest first.
    fn complete_rows(&self, clock: f32) -> Vec<Entry> {
        self.done
            .iter()
            .filter_map(|d| {
                let a = complete_alpha(clock - d.start);
                (a > 0.0).then(|| Entry {
                    tech: d.tech,
                    name: d.name.clone(),
                    progress: 1.0,
                    paused: false,
                    seconds_left: None,
                    complete: Some(a),
                })
            })
            .collect()
    }
}

/// A "COMPLETE" row's opacity `age` seconds after the topic finished.
pub fn complete_alpha(age: f32) -> f32 {
    if !(0.0..COMPLETE_HOLD + COMPLETE_FADE).contains(&age) {
        0.0
    } else if age < COMPLETE_HOLD {
        1.0
    } else {
        1.0 - (age - COMPLETE_HOLD) / COMPLETE_FADE
    }
}

/// The rows this frame: the topics being researched, then the ones just finished.
pub fn entries(view: &HostView, state: &PanelState, clock: f32) -> Vec<Entry> {
    let mut v = topics(view);
    v.extend(state.complete_rows(clock));
    v
}

/// One row's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// Its entry.
    pub index: usize,
    pub rect: Rect,
    /// The name's pen x, the time's right edge, their baseline.
    pub text_x: i32,
    pub right: i32,
    pub baseline: i32,
    pub bar: Rect,
}

/// The panel's layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub expanded: bool,
    pub header: Rect,
    /// The background (the header when collapsed).
    pub panel: Rect,
    pub rows: Vec<Row>,
    /// Entries left out for want of room, and the baseline of the line that says so.
    pub hidden: usize,
    pub more_baseline: Option<i32>,
    /// The first free line under the panel (with the stacking gap).
    pub below: i32,
}

/// What a point of the panel is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The header (folds the panel).
    Header,
    /// A row, by entry index.
    Row(usize),
    /// Elsewhere on the panel's background.
    Panel,
}

/// What `(x, y)` is on, if on the panel.
pub fn hit(l: &Layout, x: i32, y: i32) -> Option<Hit> {
    if l.header.contains(x, y) {
        return Some(Hit::Header);
    }
    if let Some(r) = l.rows.iter().find(|r| r.rect.contains(x, y)) {
        return Some(Hit::Row(r.index));
    }
    l.panel.contains(x, y).then_some(Hit::Panel)
}

/// The header's height at scale `s` (the room kept for the panel folded).
pub fn header_height(s: f32) -> i32 {
    px(s, obj::HEADER_H)
}

/// The height of the header and one row, at scale `s`.
pub fn min_height(s: f32) -> i32 {
    px(s, obj::HEADER_H) + px(s, ROWS_TOP) + px(s, ROW_H) + px(s, PAD_BOTTOM)
}

/// Lays the panel out with `n` rows for a canvas `height` lines tall, its top at `y0`, the
/// rows ending above `limit`. Expanded with no room for a single row, only the header is
/// laid out (and `expanded` is false).
pub fn layout(
    height: i32,
    y0: i32,
    n_entries: usize,
    expanded: bool,
    limit: i32,
    measure: Measure,
) -> Layout {
    let s = modern_hud::scale_for(height);
    let x0 = px(s, mp::MARGIN);
    let x1 = x0 + px(s, WIDTH);
    let header_h = px(s, obj::HEADER_H);
    let row_h = px(s, ROW_H);
    let step = row_h + px(s, ROW_GAP);
    let pad = px(s, PAD_BOTTOM);
    let first = y0 + header_h + px(s, ROWS_TOP);
    let room = |n: usize, more: bool| {
        let rows = n as i32 * step - px(s, ROW_GAP);
        first + rows + if more { px(s, MORE_H) } else { 0 } + pad <= limit
    };
    let n = if !expanded {
        0
    } else if room(n_entries, false) {
        n_entries
    } else {
        (0..n_entries)
            .rev()
            .find(|&n| n > 0 && room(n, true))
            .unwrap_or(0)
    };
    let expanded = expanded && n > 0;
    let header = if expanded {
        Rect::new(x0, y0, x1, y0 + header_h)
    } else {
        let w = mp::header_width(measure, s, HEADER_TEXT, None);
        Rect::new(x0, y0, (x0 + w).min(x1), y0 + header_h)
    };
    let (text_x, right) = (x0 + px(s, ROW_X), x1 - px(s, ROW_RIGHT));
    let bar_h = px(s, BAR_H).max(2);
    let rows: Vec<Row> = (0..n)
        .map(|i| {
            let top = first + i as i32 * step;
            let bar_top = top + px(s, BAR_TOP);
            Row {
                index: i,
                rect: Rect::new(x0, top, x1, top + row_h),
                text_x,
                right,
                baseline: top + px(s, BASELINE),
                bar: Rect::new(text_x, bar_top, right, bar_top + bar_h),
            }
        })
        .collect();
    let hidden = if expanded { n_entries - n } else { 0 };
    let mut bottom = rows.last().map_or(header.y1, |r| r.rect.y1);
    let more_baseline = (hidden > 0).then(|| {
        let b = bottom + px(s, MORE_BASELINE);
        bottom += px(s, MORE_H);
        b
    });
    if expanded {
        bottom += pad;
    }
    Layout {
        scale: s,
        expanded,
        header,
        panel: if expanded {
            Rect::new(x0, y0, x1, bottom)
        } else {
            header
        },
        rows,
        hidden,
        more_baseline,
        below: bottom + px(s, mp::STACK_GAP),
    }
}

/// The panel this frame, or None (nothing researched, or hidden over a manager).
pub(crate) fn arrange(
    fe: &Fe,
    height: i32,
    y0: i32,
    n_entries: usize,
    limit: i32,
    manager: Option<Rect>,
    measure: Measure,
) -> Option<Layout> {
    if n_entries == 0 {
        return None;
    }
    let saved = fe.game.options.get_bool(EXPANDED_KEY);
    let over = fe.game.ig.overlay.modern_research.over_manager;
    let full = layout(height, y0, n_entries, true, limit, measure);
    let folded = layout(height, y0, n_entries, false, limit, measure);
    let expanded = mp::shown_state(saved, manager, over, full.panel, folded.panel)?;
    Some(if expanded { full } else { folded })
}

/// The header was clicked: collapse or expand. Over a full-screen manager the choice lasts
/// until the manager closes; otherwise it is remembered in the options file.
pub fn toggle(fe: &mut Fe) {
    let (w, h) = fe.screen_size();
    let view = fe.game.ig.view.clone();
    let shown = mp::arrange(fe, w, h, &view)
        .research
        .is_some_and(|l| l.expanded);
    if modern_hud::manager_area(fe, w, h).is_some() {
        fe.game.ig.overlay.modern_research.over_manager = Some(!shown);
    } else {
        fe.game.set_option_now(EXPANDED_KEY, i32::from(!shown));
    }
}

/// Draws the panel laid out as `l` for `entries`.
pub fn paint(canvas: &mut Canvas, l: &Layout, entries: &[Entry], pen: &Pen, mouse: (i32, i32)) {
    let s = l.scale;
    if l.expanded {
        mp::paint_frame(canvas, l.panel, s);
    }
    let active = entries.iter().filter(|e| e.complete.is_none()).count();
    let count = active.to_string();
    let hover = hit(l, mouse.0, mouse.1) == Some(Hit::Header);
    let right = (l.expanded && active > 0).then_some(count.as_str());
    mp::paint_header(
        canvas,
        pen,
        l.header,
        s,
        HEADER_TEXT,
        l.expanded,
        hover,
        right,
        0.0,
    );
    for row in &l.rows {
        let Some(e) = entries.get(row.index) else {
            continue;
        };
        let fade = e.complete.unwrap_or(1.0);
        let col = if e.paused {
            obj::DONE
        } else {
            modern_hud::with_alpha(WHITE, fade)
        };
        let mut name_right = row.right;
        let tag = |canvas: &mut Canvas, right: i32, text: &str, c: Color| -> i32 {
            let tw = pen.advance(Face::Bold, TAG_PX * s, TAG_SPACING * s, text);
            let tx = right - tw;
            pen.write(
                canvas,
                Face::Bold,
                TAG_PX * s,
                TAG_SPACING * s,
                tx,
                row.baseline,
                c,
                None,
                text,
            );
            tx - px(s, TAG_GAP)
        };
        if e.complete.is_some() {
            name_right = tag(
                canvas,
                name_right,
                COMPLETE_TEXT,
                modern_hud::with_alpha(COMPLETE_COL, fade),
            );
        } else {
            if let Some(t) = e.seconds_left {
                let text = time_text(t);
                let w = pen.advance(Face::Bold, NAME_PX * s, 0.0, &text);
                let x = name_right - w;
                pen.write(
                    canvas,
                    Face::Bold,
                    NAME_PX * s,
                    0.0,
                    x,
                    row.baseline,
                    col,
                    None,
                    &text,
                );
                name_right = x - px(s, TAG_GAP);
            }
            if e.paused {
                name_right = tag(canvas, name_right, PAUSED_TEXT, PAUSED_COL);
            }
        }
        let name = mp::ellipsize(
            &|t| pen.measure(Face::Semibold, NAME_PX * s, 0.0, t),
            &e.name,
            (name_right - row.text_x) as f32,
        );
        pen.write(
            canvas,
            Face::Semibold,
            NAME_PX * s,
            0.0,
            row.text_x,
            row.baseline,
            col,
            None,
            &name,
        );
        if e.complete.is_some() {
            canvas.blend_rect(row.bar, modern_hud::with_alpha(COMPLETE_COL, 0.85 * fade));
        } else {
            let glow = if e.paused { 0.0 } else { 1.0 };
            modern_hud::draw_bar(canvas, row.bar, e.progress, BAR_SIGMA * s, glow);
            if e.paused {
                canvas.blend_rect(row.bar, [8, 10, 14, 150]);
            }
        }
    }
    if let Some(b) = l.more_baseline {
        let text = format!("+{} MORE", l.hidden);
        let x = l.rows.first().map_or(l.panel.x0, |r| r.text_x);
        pen.write(
            canvas,
            Face::Bold,
            MORE_PX * s,
            obj::HEADER_SPACING * s * 0.5,
            x,
            b,
            obj::DONE,
            None,
            &text,
        );
    }
}

/// Draws the panel as arranged this frame.
pub(crate) fn draw(fe: &Fe, canvas: &mut Canvas, l: &Layout, entries: &[Entry], mouse: (i32, i32)) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    paint(canvas, l, entries, &pen, mouse);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingame::host::{Lab, Tech};

    fn mono(_: Face, px: f32, spacing: f32, t: &str) -> f32 {
        let n = t.chars().count() as f32;
        n * px * 0.5 + (n - 1.0).max(0.0) * spacing
    }

    fn lab(status: LabStatus, topic: Option<TechId>, progress: f32) -> Lab {
        Lab {
            name: "MODULE".into(),
            status,
            topic,
            progress,
        }
    }

    fn tech(id: TechId, name: &str, time: f32, state: TechState) -> Tech {
        Tech {
            id,
            name: name.into(),
            time,
            state,
            ..Default::default()
        }
    }

    fn view() -> HostView {
        HostView {
            labs: vec![
                lab(LabStatus::Researching, Some(7), 0.25),
                lab(LabStatus::Idle, None, 0.0),
                lab(LabStatus::Paused, Some(3), 0.5),
                lab(LabStatus::Absent, Some(9), 0.9),
            ],
            techs: vec![
                tech(3, "Cloaking", 100.0, TechState::Researching),
                tech(7, "Heavy Armor", 120.0, TechState::Researching),
                tech(9, "Gone", 10.0, TechState::Researching),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn topics_in_technology_order_with_time_left() {
        let e = topics(&view());
        assert_eq!(e.len(), 2, "idle and absent labs are not listed");
        assert_eq!((e[0].tech, e[0].name.as_str()), (3, "CLOAKING"));
        assert!(e[0].paused);
        assert_eq!(e[0].seconds_left, Some(50.0));
        assert_eq!((e[1].tech, e[1].name.as_str()), (7, "HEAVY ARMOR"));
        assert!(!e[1].paused);
        assert_eq!(e[1].progress, 0.25);
        assert_eq!(e[1].seconds_left, Some(90.0));
        let mut v = view();
        v.player.tutorial = true;
        assert_eq!(topics(&v)[1].seconds_left, Some(90.0 / 8.0));
        assert!(topics(&HostView::default()).is_empty());
    }

    #[test]
    fn time_text_rounds_up() {
        assert_eq!(time_text(65.0), "1:05");
        assert_eq!(time_text(0.2), "0:01");
        assert_eq!(time_text(0.0), "0:00");
        assert_eq!(time_text(3723.0), "1:02:03");
    }

    #[test]
    fn a_finished_topic_fades_out_as_complete() {
        let mut st = PanelState::default();
        let mut v = view();
        st.note(0.0, &v);
        assert!(entries(&v, &st, 0.0).iter().all(|e| e.complete.is_none()));
        // Heavy Armor finishes: its lab goes idle and the technology is the player's.
        v.labs[0] = lab(LabStatus::Idle, None, 0.0);
        v.techs[1].state = TechState::Researched;
        // Cloaking is stopped by the player: no COMPLETE row.
        v.labs[2] = lab(LabStatus::Idle, None, 0.0);
        v.techs[0].state = TechState::Available;
        st.note(1.0, &v);
        let e = entries(&v, &st, 1.0);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].name, "HEAVY ARMOR");
        assert_eq!(e[0].complete, Some(1.0));
        assert_eq!(e[0].progress, 1.0);
        let mid = entries(&v, &st, 1.0 + COMPLETE_HOLD + COMPLETE_FADE / 2.0);
        assert!((mid[0].complete.unwrap() - 0.5).abs() < 1e-5);
        st.note(1.0 + COMPLETE_HOLD + COMPLETE_FADE, &v);
        assert!(entries(&v, &st, 1.0 + COMPLETE_HOLD + COMPLETE_FADE).is_empty());
        assert_eq!(complete_alpha(-1.0), 0.0);
    }

    #[test]
    fn a_new_mission_does_not_report_completions() {
        let mut st = PanelState::default();
        let mut v = view();
        st.note(0.0, &v);
        v.player.mission += 1;
        v.labs.clear();
        v.techs[1].state = TechState::Researched;
        st.note(1.0, &v);
        assert!(entries(&v, &st, 1.0).is_empty());
    }

    #[test]
    fn layout_at_1080p_under_the_build_queue() {
        let l = layout(1080, 306, 2, true, 1000, &mono);
        assert!(l.expanded);
        assert_eq!(l.header, Rect::new(16, 306, 376, 336));
        assert_eq!(l.rows.len(), 2);
        let r = l.rows[0];
        assert_eq!(r.rect, Rect::new(16, 338, 376, 366));
        assert_eq!((r.text_x, r.right, r.baseline), (32, 362, 352));
        assert_eq!(r.bar, Rect::new(32, 358, 362, 361));
        assert_eq!(l.rows[1].rect.y0, 368);
        assert_eq!(l.panel, Rect::new(16, 306, 376, 396 + 8));
        assert_eq!(l.below, 412);
        assert_eq!((l.hidden, l.more_baseline), (0, None));
        assert_eq!(min_height(1.0), 30 + 2 + 28 + 8);
        assert_eq!(header_height(1.0), 30);
    }

    #[test]
    fn collapsed_is_the_header_alone() {
        let l = layout(1080, 100, 3, false, 1000, &mono);
        assert!(!l.expanded && l.rows.is_empty());
        let w = mp::header_width(&mono, 1.0, HEADER_TEXT, None);
        assert_eq!(l.header, Rect::new(16, 100, 16 + w, 130));
        assert_eq!(l.panel, l.header);
        assert_eq!(l.below, 138);
    }

    #[test]
    fn rows_that_do_not_fit_are_counted() {
        let l = layout(1080, 0, 6, true, 30 + 2 + 88 + 18 + 8, &mono);
        assert_eq!((l.rows.len(), l.hidden), (3, 3));
        assert_eq!(l.more_baseline, Some(120 + 13));
        // No room for a row: the header alone.
        let l = layout(1080, 0, 6, true, 40, &mono);
        assert!(!l.expanded);
        assert_eq!(l.panel, l.header);
    }

    #[test]
    fn layout_scales_with_the_height() {
        let l = layout(480, 50, 1, true, 480, &mono);
        assert!((l.scale - 480.0 / 1080.0).abs() < 1e-6);
        assert_eq!(l.header, Rect::new(7, 50, 7 + 160, 50 + 13));
        assert!(l.rows[0].bar.height() >= 2);
        let k = layout(2160, 0, 1, true, 2160, &mono);
        assert_eq!(k.header, Rect::new(32, 0, 32 + 720, 60));
    }

    #[test]
    fn hit_test() {
        let l = layout(1080, 306, 2, true, 1000, &mono);
        assert_eq!(hit(&l, 20, 310), Some(Hit::Header));
        assert_eq!(hit(&l, 370, 335), Some(Hit::Header));
        assert_eq!(hit(&l, 100, 340), Some(Hit::Row(0)));
        assert_eq!(hit(&l, 100, 370), Some(Hit::Row(1)));
        // The gap between rows and the bottom padding are the panel's.
        assert_eq!(hit(&l, 100, 367), Some(Hit::Panel));
        assert_eq!(hit(&l, 100, 400), Some(Hit::Panel));
        assert_eq!(hit(&l, 100, 404), None);
        assert_eq!(hit(&l, 400, 340), None);
        // Folded: the header only.
        let f = layout(1080, 306, 2, false, 1000, &mono);
        assert_eq!(hit(&f, 20, 310), Some(Hit::Header));
        assert_eq!(hit(&f, 20, 340), None);
        assert_eq!(hit(&f, f.header.x1 + 1, 310), None);
    }
}
