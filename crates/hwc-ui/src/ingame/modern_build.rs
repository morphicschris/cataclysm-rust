//! The modern HUD's build queue (part of the user-requested modern HUD, `--hud`): a top-left
//! panel directly under the objectives panel, in place of the original's construction list
//! (`moDrawConstructionList`, the green "ACOLYTE (2/1)" lines with their progress bars). It
//! moves with the objectives panel's height and fold.
//!
//! Like the original it lists one row per ship type in the player's construction queue, with
//! the ships still to build per factory (the mothership's first; "3 / 1" with two factories),
//! and is not shown at all while the queue is empty. Each row has a thin glowing bar (the
//! resources panel's style) with the progress of the ship being built; a type whose jobs
//! are all paused is greyed, tagged "PAUSED" and its bar dimmed (the original does not show
//! the pause). The header "BUILD QUEUE" folds the panel (remembered in the options file as
//! `hwcBuildQueueExpanded`); folded, the header alone is shown, with no count. Rows that do
//! not fit above the system log are left out and counted on a last "+N MORE" line.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::host::{BuildRow, HostView, ShipType};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_panel::{self as mp, px, Measure};
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcBuildQueueExpanded";
pub const HEADER_TEXT: &str = "BUILD QUEUE";
pub const PAUSED_TEXT: &str = "PAUSED";

/// The "PAUSED" tag.
pub const PAUSED_COL: Color = [0xFF, 0xA8, 0x40, 0xFF];

/// Reference layout, in pixels at 1080 lines (as wide as the objectives panel).
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

/// One ship type of the queue.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub ship_type: ShipType,
    pub name: String,
    /// Ships still to build, per factory with a job of this type (the mothership first).
    pub counts: Vec<i32>,
    /// Progress of the ship being built, 0..1.
    pub progress: f32,
    /// Every job of this type is paused.
    pub paused: bool,
}

impl Entry {
    pub fn total(&self) -> i32 {
        self.counts.iter().sum()
    }

    /// "3", or "3 / 1" with two factories (the original's "(3/1)").
    pub fn count_text(&self) -> String {
        let v: Vec<String> = self.counts.iter().map(i32::to_string).collect();
        v.join(" / ")
    }
}

/// The queue as the original lists it: one entry per ship type in the order the jobs come,
/// the counts per factory, the mothership's first.
pub fn entries(view: &HostView) -> Vec<Entry> {
    let mut factories: Vec<_> = view.factories.iter().collect();
    factories.sort_by_key(|f| !f.mothership);
    let jobs: Vec<_> = view.jobs.iter().filter(|j| j.count > 0).collect();
    let mut types: Vec<ShipType> = Vec::new();
    for j in &jobs {
        if !types.contains(&j.ship_type) {
            types.push(j.ship_type);
        }
    }
    types
        .into_iter()
        .map(|t| {
            let mine: Vec<_> = jobs.iter().filter(|j| j.ship_type == t).collect();
            let mut counts: Vec<i32> = factories
                .iter()
                .map(|f| {
                    mine.iter()
                        .filter(|j| j.factory == f.ship)
                        .map(|j| j.count)
                        .sum()
                })
                .filter(|&n| n > 0)
                .collect();
            let elsewhere: i32 = mine
                .iter()
                .filter(|j| !factories.iter().any(|f| f.ship == j.factory))
                .map(|j| j.count)
                .sum();
            if elsewhere > 0 {
                counts.push(elsewhere);
            }
            let running: Vec<_> = mine.iter().filter(|j| !j.paused).collect();
            let progress = if running.is_empty() {
                mine.iter().map(|j| j.progress).fold(0.0, f32::max)
            } else {
                running.iter().map(|j| j.progress).fold(0.0, f32::max)
            };
            let name = view
                .build_list
                .iter()
                .find_map(|r| match r {
                    BuildRow::Ship(o) if o.ship_type == t => Some(o.name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("Ship {t}"));
            Entry {
                ship_type: t,
                name: name.to_uppercase(),
                counts,
                progress: progress.clamp(0.0, 1.0),
                paused: running.is_empty(),
            }
        })
        .collect()
}

/// One row's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// Its entry.
    pub index: usize,
    pub rect: Rect,
    /// The name's pen x, the count's right edge, their baseline.
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

/// The height of the header and one row, at scale `s` (what the log leaves free above it).
pub fn min_height(s: f32) -> i32 {
    px(s, obj::HEADER_H) + px(s, ROWS_TOP) + px(s, ROW_H) + px(s, PAD_BOTTOM)
}

/// Lays the panel out for a canvas `height` lines tall, its top at `y0`, the rows ending above
/// `limit`. Expanded with no room for a single row, only the header is laid out (and
/// `expanded` is false).
pub fn layout(
    height: i32,
    y0: i32,
    entries: &[Entry],
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
    // Rows that fit: all of them, or as many as leave room for the "+N MORE" line.
    let room = |n: usize, more: bool| {
        let rows = n as i32 * step - px(s, ROW_GAP);
        first + rows + if more { px(s, MORE_H) } else { 0 } + pad <= limit
    };
    let n = if !expanded {
        0
    } else if room(entries.len(), false) {
        entries.len()
    } else {
        (0..entries.len())
            .rev()
            .find(|&n| n > 0 && room(n, true))
            .unwrap_or(0)
    };
    let expanded = expanded && n > 0;
    let full_header = Rect::new(x0, y0, x1, y0 + header_h);
    let header = if expanded {
        full_header
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
    let hidden = if expanded { entries.len() - n } else { 0 };
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

/// The panel's state kept in the overlay.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    /// Expanded (true) or collapsed by a header click while a full-screen manager is open;
    /// None follows the usual rule. Cleared when the manager closes.
    pub over_manager: Option<bool>,
}

/// The panel this frame, or None (empty queue, or hidden over a manager).
pub(crate) fn arrange(
    fe: &Fe,
    height: i32,
    y0: i32,
    entries: &[Entry],
    limit: i32,
    manager: Option<Rect>,
    measure: Measure,
) -> Option<Layout> {
    if entries.is_empty() {
        return None;
    }
    let saved = fe.game.options.get_bool(EXPANDED_KEY);
    let over = fe.game.ig.overlay.modern_build.over_manager;
    let full = layout(height, y0, entries, true, limit, measure);
    let folded = layout(height, y0, entries, false, limit, measure);
    let expanded = mp::shown_state(saved, manager, over, full.panel, folded.panel)?;
    Some(if expanded { full } else { folded })
}

/// The header was clicked: collapse or expand. Over a full-screen manager the choice lasts
/// until the manager closes; otherwise it is remembered in the options file.
pub fn toggle(fe: &mut Fe) {
    let (w, h) = fe.screen_size();
    let view = fe.game.ig.view.clone();
    let shown = mp::arrange(fe, w, h, &view)
        .build
        .is_some_and(|l| l.expanded);
    if modern_hud::manager_area(fe, w, h).is_some() {
        fe.game.ig.overlay.modern_build.over_manager = Some(!shown);
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
    let total: i32 = entries.iter().map(Entry::total).sum();
    let total = total.to_string();
    let hover = l.header.contains(mouse.0, mouse.1);
    let right = l.expanded.then_some(total.as_str());
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
        let col = if e.paused { obj::DONE } else { WHITE };
        let count = e.count_text();
        let cw = pen.advance(Face::Bold, NAME_PX * s, 0.0, &count);
        let cx = row.right - cw;
        pen.write(
            canvas,
            Face::Bold,
            NAME_PX * s,
            0.0,
            cx,
            row.baseline,
            col,
            None,
            &count,
        );
        let mut name_right = cx - px(s, TAG_GAP);
        if e.paused {
            let tw = pen.advance(Face::Bold, TAG_PX * s, TAG_SPACING * s, PAUSED_TEXT);
            let tx = name_right - tw;
            pen.write(
                canvas,
                Face::Bold,
                TAG_PX * s,
                TAG_SPACING * s,
                tx,
                row.baseline,
                PAUSED_COL,
                None,
                PAUSED_TEXT,
            );
            name_right = tx - px(s, TAG_GAP);
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
        let glow = if e.paused { 0.0 } else { 1.0 };
        modern_hud::draw_bar(canvas, row.bar, e.progress, BAR_SIGMA * s, glow);
        if e.paused {
            canvas.blend_rect(row.bar, [8, 10, 14, 150]);
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
pub(crate) fn draw(fe: &Fe, canvas: &mut Canvas, l: &Layout, view: &HostView, mouse: (i32, i32)) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    paint(canvas, l, &entries(view), &pen, mouse);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingame::host::{BuildJob, BuildOption, Factory, ShipId};

    fn mono(_: Face, px: f32, spacing: f32, t: &str) -> f32 {
        let n = t.chars().count() as f32;
        n * px * 0.5 + (n - 1.0).max(0.0) * spacing
    }

    fn entry(name: &str, counts: &[i32], paused: bool) -> Entry {
        Entry {
            ship_type: 1,
            name: name.into(),
            counts: counts.to_vec(),
            progress: 0.4,
            paused,
        }
    }

    fn job(factory: ShipId, t: ShipType, count: i32, paused: bool, progress: f32) -> BuildJob {
        BuildJob {
            factory,
            ship_type: t,
            count,
            batch_total: count,
            paused,
            progress,
            cost: 100,
            cost_so_far: 0,
        }
    }

    #[test]
    fn entries_group_by_type_with_per_factory_counts() {
        let v = HostView {
            factories: vec![
                Factory {
                    ship: 7,
                    mothership: false,
                    ..Default::default()
                },
                Factory {
                    ship: 1,
                    mothership: true,
                    ..Default::default()
                },
            ],
            build_list: vec![
                BuildRow::Class("FIGHTER CLASS".into()),
                BuildRow::Ship(BuildOption {
                    ship_type: 3,
                    name: "Acolyte".into(),
                    ..Default::default()
                }),
            ],
            jobs: vec![
                job(7, 3, 2, false, 0.2),
                job(1, 3, 3, true, 0.9),
                job(1, 5, 1, true, 0.5),
                job(1, 6, 0, false, 0.0),
            ],
            ..Default::default()
        };
        let e = entries(&v);
        assert_eq!(e.len(), 2, "an empty job is not listed");
        assert_eq!(e[0].name, "ACOLYTE");
        assert_eq!(e[0].counts, [3, 2], "the mothership's count first");
        assert_eq!(e[0].count_text(), "3 / 2");
        assert_eq!(e[0].total(), 5);
        assert_eq!(
            e[0].progress, 0.2,
            "the ship being built, not the paused one"
        );
        assert!(!e[0].paused);
        assert_eq!(e[1].name, "SHIP 5");
        assert!(e[1].paused, "all its jobs are paused");
        assert_eq!(e[1].progress, 0.5);
        assert_eq!(e[1].count_text(), "1");
        assert!(entries(&HostView::default()).is_empty());
    }

    #[test]
    fn layout_at_1080p_under_the_objectives() {
        let e = [entry("ACOLYTE", &[3], false), entry("MIMIC", &[1, 2], true)];
        let l = layout(1080, 200, &e, true, 1000, &mono);
        assert!(l.expanded);
        assert_eq!(l.header, Rect::new(16, 200, 376, 230));
        assert_eq!(l.rows.len(), 2);
        let r = l.rows[0];
        assert_eq!(r.rect, Rect::new(16, 232, 376, 260));
        assert_eq!((r.text_x, r.right, r.baseline), (32, 362, 246));
        assert_eq!(r.bar, Rect::new(32, 252, 362, 255));
        assert_eq!(l.rows[1].rect.y0, 262);
        assert_eq!(l.panel, Rect::new(16, 200, 376, 290 + 8));
        assert_eq!(l.below, 306);
        assert_eq!(l.hidden, 0);
        assert_eq!(l.more_baseline, None);
    }

    #[test]
    fn collapsed_is_the_header_alone_without_a_count() {
        let e = [entry("ACOLYTE", &[3], false)];
        let l = layout(1080, 100, &e, false, 1000, &mono);
        assert!(!l.expanded && l.rows.is_empty());
        assert_eq!(
            l.header,
            Rect::new(
                16,
                100,
                16 + mp::header_width(&mono, 1.0, HEADER_TEXT, None),
                130
            )
        );
        assert_eq!(l.panel, l.header);
        assert_eq!(l.below, 138);
    }

    #[test]
    fn rows_that_do_not_fit_are_counted() {
        let e: Vec<Entry> = (0..6)
            .map(|i| entry(&format!("S{i}"), &[1], false))
            .collect();
        // Header 30 + 2, then three rows (88) and the more line (18) and padding (8).
        let l = layout(1080, 0, &e, true, 30 + 2 + 88 + 18 + 8, &mono);
        assert_eq!(l.rows.len(), 3);
        assert_eq!(l.hidden, 3);
        assert_eq!(l.more_baseline, Some(120 + 13));
        assert!(l.panel.y1 <= 146);
        // No room for a single row: the header alone.
        let l = layout(1080, 0, &e, true, 40, &mono);
        assert!(!l.expanded);
        assert_eq!(l.panel, l.header);
        // Everything fits: no more line.
        let l = layout(1080, 0, &e, true, 2000, &mono);
        assert_eq!((l.rows.len(), l.hidden), (6, 0));
        assert_eq!(min_height(1.0), 30 + 2 + 28 + 8);
    }

    #[test]
    fn layout_scales_with_the_height() {
        let e = [entry("ACOLYTE", &[3], false)];
        let l = layout(480, 50, &e, true, 480, &mono);
        let s = 480.0 / 1080.0;
        assert!((l.scale - s).abs() < 1e-6);
        assert_eq!(l.header, Rect::new(7, 50, 7 + 160, 50 + 13));
        assert!(l.rows[0].bar.height() >= 2);
        let k = layout(2160, 0, &e, true, 2160, &mono);
        assert_eq!(k.header, Rect::new(32, 0, 32 + 720, 60));
    }
}
