//! The modern HUD's selected ships (part of the user-requested modern HUD, `--hud`): a
//! top-right panel under the resources panel, in place of the original's selected-ship list
//! (`moDrawShipList`, the "3 x RECON" lines). It is up only while ships are selected.
//!
//! One row per ship type and owner, in the original's order, with the type's name and the
//! number selected; clicking a row works as a click on the original's line (it selects that
//! type, or with the multiple-select key held marks it to leave the selection when the key is
//! released). Under each name a compact health bar: one segment per ship when there are no
//! more than [`SEGMENTS_UP_TO`] of the type, else one bar with their average. The header
//! "SELECTED" shows the number of ships selected and folds the panel (remembered in the
//! options file as `hwcSelectedExpanded`). Rows that do not fit above the bottom of the
//! screen are counted on a last "+N MORE" line.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::ingame::host::{Owner, ShipInfo, ShipType};
use crate::ingame::launchmgr::{HEALTH_GREEN, HEALTH_YELLOW};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_panel::{self as mp, px, Measure};
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcSelectedExpanded";
pub const HEADER_TEXT: &str = "SELECTED";
/// A type with at most this many ships selected gets a health segment per ship.
pub const SEGMENTS_UP_TO: usize = 5;

/// Health bar colours (healthy, damaged, critical), lighter versions of the original's.
pub const HEALTH_OK: Color = [0x5A, 0xDC, 0x6E, 0xFF];
pub const HEALTH_LOW: Color = [0xFF, 0xC8, 0x3C, 0xFF];
pub const HEALTH_CRITICAL: Color = [0xFF, 0x55, 0x3C, 0xFF];
/// Row text: an ally's ship, a ship that cannot take orders, a row marked to leave the
/// selection.
const ALLY: Color = [0x6E, 0xC8, 0xFF, 0xFF];
const DESELECT: Color = [0x96, 0x9C, 0xFF, 0xFF];

/// Reference layout, in pixels at 1080 lines (as wide as the resources panel).
const WIDTH: f32 = 260.0;
const ROW_X: f32 = 16.0;
const ROW_RIGHT: f32 = 14.0;
const ROWS_TOP: f32 = 2.0;
const ROW_H: f32 = 24.0;
const BASELINE: f32 = 13.0;
const BAR_TOP: f32 = 17.0;
const BAR_H: f32 = 3.0;
const ROW_GAP: f32 = 2.0;
const SEGMENT_GAP: f32 = 2.0;
const MORE_H: f32 = 18.0;
const MORE_BASELINE: f32 = 13.0;
const PAD_BOTTOM: f32 = 8.0;
const NAME_PX: f32 = 13.0;
const MORE_PX: f32 = 11.0;

/// Ships of one type and owner.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub ship_type: ShipType,
    pub owner: Owner,
    pub name: String,
    /// Each ship's health, 0..1.
    pub health: Vec<f32>,
}

impl Group {
    pub fn count(&self) -> usize {
        self.health.len()
    }

    pub fn average(&self) -> f32 {
        if self.health.is_empty() {
            return 0.0;
        }
        self.health.iter().sum::<f32>() / self.health.len() as f32
    }
}

/// A ship's health as a fraction (full when its maximum is unknown).
pub fn fraction(s: &ShipInfo) -> f32 {
    if s.max_health > 0.0 {
        (s.health / s.max_health).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// The selection grouped by type and owner, in the original list's order.
pub fn groups(ships: &[ShipInfo]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    for s in ships {
        match out
            .iter_mut()
            .find(|g| g.ship_type == s.ship_type && g.owner == s.owner)
        {
            Some(g) => g.health.push(fraction(s)),
            None => out.push(Group {
                ship_type: s.ship_type,
                owner: s.owner,
                name: s.name.to_uppercase(),
                health: vec![fraction(s)],
            }),
        }
    }
    out.sort_by_key(|g| (g.ship_type, g.owner as u8));
    out
}

/// The health colour of a fraction (the in-game bars' thresholds).
pub fn health_colour(f: f32) -> Color {
    if f > HEALTH_GREEN {
        HEALTH_OK
    } else if f > HEALTH_YELLOW {
        HEALTH_LOW
    } else {
        HEALTH_CRITICAL
    }
}

/// The bar split into `n` segments with a gap between them (one segment for `n` <= 1). A
/// bar too short for its gaps is not split.
pub fn segments(bar: Rect, n: usize, gap: i32) -> Vec<Rect> {
    let w = bar.width();
    let n = n.max(1) as i32;
    if n == 1 || w < n * 2 + (n - 1) * gap {
        return vec![bar];
    }
    let inner = w - (n - 1) * gap;
    (0..n)
        .map(|i| {
            let a = bar.x0 + i * inner / n + i * gap;
            let b = bar.x0 + (i + 1) * inner / n + i * gap;
            Rect::new(a, bar.y0, b, bar.y1)
        })
        .collect()
}

/// One row's place.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    /// Its group.
    pub index: usize,
    pub rect: Rect,
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
    /// Groups left out for want of room, and the baseline of the line that says so.
    pub hidden: usize,
    pub more_baseline: Option<i32>,
    /// The first free line under the panel (with the stacking gap).
    pub below: i32,
}

/// Lays the panel out for a `width` x `height` canvas, its top at `y0` (under the resources
/// panel), the rows ending above `limit`; `total` is the header's ship count. Expanded with
/// no room for a single row, only the header (and `expanded` false).
#[allow(clippy::too_many_arguments)]
pub fn layout(
    width: i32,
    height: i32,
    y0: i32,
    groups: &[Group],
    expanded: bool,
    limit: i32,
    total: &str,
    measure: Measure,
) -> Layout {
    let s = modern_hud::scale_for(height);
    let x1 = width - px(s, mp::MARGIN);
    let x0 = x1 - px(s, WIDTH);
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
    } else if room(groups.len(), false) {
        groups.len()
    } else {
        (0..groups.len())
            .rev()
            .find(|&n| n > 0 && room(n, true))
            .unwrap_or(0)
    };
    let expanded = expanded && n > 0;
    let header = if expanded {
        Rect::new(x0, y0, x1, y0 + header_h)
    } else {
        // Folded: kept to the right edge, like the resources panel's header.
        let w = mp::header_width(measure, s, HEADER_TEXT, Some(total));
        Rect::new((x1 - w).max(x0), y0, x1, y0 + header_h)
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
    let hidden = if expanded { groups.len() - n } else { 0 };
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

/// The panel this frame, or None (nothing selected, or hidden over a manager).
#[allow(clippy::too_many_arguments)]
pub(crate) fn arrange(
    fe: &Fe,
    width: i32,
    height: i32,
    y0: i32,
    ships: &[ShipInfo],
    limit: i32,
    manager: Option<Rect>,
    measure: Measure,
) -> Option<Layout> {
    if ships.is_empty() {
        return None;
    }
    let g = groups(ships);
    let total = ships.len().to_string();
    let saved = fe.game.options.get_bool(EXPANDED_KEY);
    let over = fe.game.ig.overlay.modern_selected.over_manager;
    let full = layout(width, height, y0, &g, true, limit, &total, measure);
    let folded = layout(width, height, y0, &g, false, limit, &total, measure);
    let expanded = mp::shown_state(saved, manager, over, full.panel, folded.panel)?;
    Some(if expanded { full } else { folded })
}

/// The header was clicked: collapse or expand. Over a full-screen manager the choice lasts
/// until the manager closes; otherwise it is remembered in the options file.
pub fn toggle(fe: &mut Fe) {
    let (w, h) = fe.screen_size();
    let view = fe.game.ig.view.clone();
    let shown = mp::arrange(fe, w, h, &view)
        .selected
        .is_some_and(|l| l.expanded);
    if modern_hud::manager_area(fe, w, h).is_some() {
        fe.game.ig.overlay.modern_selected.over_manager = Some(!shown);
    } else {
        fe.game.set_option_now(EXPANDED_KEY, i32::from(!shown));
    }
}

/// Draws the panel laid out as `l` for `groups`; `deselect` are the rows marked to leave the
/// selection.
pub fn paint(
    canvas: &mut Canvas,
    l: &Layout,
    groups: &[Group],
    deselect: &[(ShipType, Owner)],
    pen: &Pen,
    mouse: (i32, i32),
) {
    let s = l.scale;
    if l.expanded {
        mp::paint_frame(canvas, l.panel, s);
    }
    let total: usize = groups.iter().map(Group::count).sum();
    let total = total.to_string();
    let hover = l.header.contains(mouse.0, mouse.1);
    mp::paint_header(
        canvas,
        pen,
        l.header,
        s,
        HEADER_TEXT,
        l.expanded,
        hover,
        Some(&total),
        0.0,
    );
    for row in &l.rows {
        let Some(g) = groups.get(row.index) else {
            continue;
        };
        let col = if deselect.contains(&(g.ship_type, g.owner)) {
            DESELECT
        } else if row.rect.contains(mouse.0, mouse.1) {
            obj::GOLD
        } else {
            match g.owner {
                Owner::Disabled => obj::DONE,
                Owner::Ally => ALLY,
                Owner::Enemy => HEALTH_CRITICAL,
                Owner::Own => WHITE,
            }
        };
        let count = g.count().to_string();
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
        let name = mp::ellipsize(
            &|t| pen.measure(Face::Semibold, NAME_PX * s, 0.0, t),
            &g.name,
            (cx - px(s, 10.0) - row.text_x) as f32,
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
        let fills: Vec<f32> = if g.count() <= SEGMENTS_UP_TO {
            g.health.clone()
        } else {
            vec![g.average()]
        };
        let gap = px(s, SEGMENT_GAP).max(1);
        let segs = segments(row.bar, fills.len(), gap);
        let fills = if segs.len() == fills.len() {
            fills
        } else {
            vec![g.average()]
        };
        for (seg, f) in segs.iter().zip(fills) {
            mp::track(canvas, *seg);
            let w = (seg.width() as f32 * f).round() as i32;
            canvas.blend_rect(
                Rect::new(seg.x0, seg.y0, seg.x0 + w, seg.y1),
                health_colour(f),
            );
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
pub(crate) fn draw(fe: &Fe, canvas: &mut Canvas, l: &Layout, ships: &[ShipInfo]) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let st = &fe.game.ig.overlay;
    paint(
        canvas,
        l,
        &groups(ships),
        &st.deselect,
        &pen,
        st.mouse_pos(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(_: Face, px: f32, spacing: f32, t: &str) -> f32 {
        let n = t.chars().count() as f32;
        n * px * 0.5 + (n - 1.0).max(0.0) * spacing
    }

    fn ship(t: ShipType, name: &str, owner: Owner, health: f32) -> ShipInfo {
        ShipInfo {
            ship_type: t,
            name: name.into(),
            owner,
            health,
            max_health: 100.0,
            ..Default::default()
        }
    }

    fn group(n: usize) -> Group {
        Group {
            ship_type: 1,
            owner: Owner::Own,
            name: "RECON".into(),
            health: vec![1.0; n],
        }
    }

    #[test]
    fn groups_by_type_and_owner_in_the_original_order() {
        let ships = [
            ship(5, "Acolyte", Owner::Own, 100.0),
            ship(2, "Recon", Owner::Own, 50.0),
            ship(5, "Acolyte", Owner::Own, 20.0),
            ship(5, "Acolyte", Owner::Ally, 80.0),
            ship(2, "Recon", Owner::Own, 100.0),
        ];
        let g = groups(&ships);
        assert_eq!(g.len(), 3);
        assert_eq!((g[0].ship_type, g[0].count()), (2, 2));
        assert_eq!(g[0].name, "RECON");
        assert!((g[0].average() - 0.75).abs() < 1e-6);
        assert_eq!(
            (g[1].ship_type, g[1].owner, g[1].count()),
            (5, Owner::Own, 2)
        );
        assert_eq!(g[1].health, [1.0, 0.2]);
        assert_eq!(g[2].owner, Owner::Ally);
        // Unknown maximum: full.
        let mut s = ship(1, "x", Owner::Own, 5.0);
        s.max_health = 0.0;
        assert_eq!(fraction(&s), 1.0);
    }

    #[test]
    fn health_colours_follow_the_thresholds() {
        assert_eq!(health_colour(1.0), HEALTH_OK);
        assert_eq!(health_colour(0.5), HEALTH_LOW);
        assert_eq!(health_colour(0.3), HEALTH_LOW);
        assert_eq!(health_colour(0.25), HEALTH_CRITICAL);
    }

    #[test]
    fn segments_split_the_bar() {
        let bar = Rect::new(100, 10, 200, 13);
        assert_eq!(segments(bar, 1, 2), vec![bar]);
        let s = segments(bar, 3, 2);
        assert_eq!(s.len(), 3);
        assert_eq!(s[0].x0, 100);
        assert_eq!(s[2].x1, 200);
        assert_eq!(s[1].x0 - s[0].x1, 2);
        assert_eq!(s[2].x0 - s[1].x1, 2);
        assert!(s
            .iter()
            .all(|r| r.y0 == 10 && r.y1 == 13 && r.width() >= 31));
        // Too short to split.
        assert_eq!(
            segments(Rect::new(0, 0, 8, 2), 5, 2),
            vec![Rect::new(0, 0, 8, 2)]
        );
    }

    #[test]
    fn layout_at_1080p_under_the_resources_panel() {
        let g = [group(3), group(1)];
        let l = layout(1920, 1080, 142, &g, true, 1064, "4", &mono);
        assert!(l.expanded);
        assert_eq!(l.header, Rect::new(1644, 142, 1904, 172));
        assert_eq!(l.rows.len(), 2);
        let r = l.rows[0];
        assert_eq!(r.rect, Rect::new(1644, 174, 1904, 198));
        assert_eq!((r.text_x, r.right, r.baseline), (1660, 1890, 187));
        assert_eq!(r.bar, Rect::new(1660, 191, 1890, 194));
        assert_eq!(l.rows[1].rect.y0, 200);
        assert_eq!(l.panel, Rect::new(1644, 142, 1904, 224 + 8));
        assert_eq!(l.below, 240);
        // The resources panel ends at 134 (its `bottom` 142 includes the gap): no overlap.
        assert!(!modern_hud::overlaps(
            l.panel,
            modern_hud::layout(1920, 1080).panel
        ));
    }

    #[test]
    fn folded_header_keeps_to_the_right_with_the_count() {
        let g = [group(3)];
        let l = layout(1920, 1080, 142, &g, false, 1064, "3", &mono);
        assert!(!l.expanded && l.rows.is_empty());
        let w = mp::header_width(&mono, 1.0, HEADER_TEXT, Some("3"));
        assert_eq!(l.header, Rect::new(1904 - w, 142, 1904, 172));
        assert_eq!(l.panel, l.header);
    }

    #[test]
    fn rows_that_do_not_fit_are_counted() {
        let g: Vec<Group> = (0..10).map(|_| group(1)).collect();
        // 640x480: scale 0.444; ten rows of 12 fit below the header...
        let l = layout(640, 480, 60, &g, true, 473, "10", &mono);
        assert_eq!((l.rows.len(), l.hidden), (10, 0));
        // ...but not above 150.
        let l = layout(640, 480, 60, &g, true, 150, "10", &mono);
        assert!(l.expanded);
        assert!(l.hidden > 0 && l.rows.len() + l.hidden == 10, "{l:?}");
        assert!(l.panel.y1 <= 150, "{:?}", l.panel);
        assert!(l.more_baseline.is_some());
        assert_eq!(l.panel.x1, 640 - 7);
        // No room at all: the header alone.
        let l = layout(640, 480, 460, &g, true, 473, "10", &mono);
        assert!(!l.expanded);
    }
}
