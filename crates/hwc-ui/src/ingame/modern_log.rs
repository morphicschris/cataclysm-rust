//! The modern HUD's system log (part of the user-requested modern HUD, `--hud`): a
//! bottom-left panel in place of the original's game message lines (`moDrawMessages`: "ACOLYTE
//! built", "Research complete", "under attack"...). The panel grows upward from the bottom-left
//! corner: its header ("SYSTEM LOG") on top, the messages under it, the newest at the bottom.
//! Folded, the header alone sits in the bottom-left corner; a click there expands the panel
//! upward again.
//!
//! Expanded it keeps the last [`HISTORY`] messages: as in the original a message holds for
//! [`overlay::MESSAGE_HOLD`] seconds and then fades over [`overlay::MESSAGE_FADE`] seconds, but
//! instead of disappearing it stays as a dimmed history line. Scrolled back (Page Up game chat)
//! every line is full, as in the original. Message colours keep their meaning, lifted towards
//! white so that the original's pure red, green and blue read on the dark panel. A message
//! about a ship is clickable as in the original (it focuses the ship). Long messages wrap.
//! Folded, the header alone is shown, and it flashes when a message comes in; a warning (a red
//! message, such as the refused jump's "Complete or Cancel External Construction") still shows
//! above the folded header while the original would show it, then goes. The fold is
//! remembered in the options file as `hwcSystemLogExpanded`.

use crate::canvas::{Canvas, Color, Rect};
use crate::ingame::host::{GameMessage, HostView};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_panel::{self as mp, px, Flash, Measure};
use crate::ingame::overlay::{MESSAGE_FADE, MESSAGE_HOLD};
use crate::ingame::Fe;

/// The options-file key: 1 (default) expanded, 0 collapsed.
pub const EXPANDED_KEY: &str = "hwcSystemLogExpanded";
pub const HEADER_TEXT: &str = "SYSTEM LOG";
/// Messages kept on the expanded panel.
pub const HISTORY: usize = 8;
/// How bright a message stays once it has faded (the original lets it go).
pub const HISTORY_ALPHA: f32 = 0.5;
/// The colour of a warning, which shows even with the panel folded.
pub const WARNING: [u8; 4] = [255, 0, 0, 255];
/// Warnings shown at most above the folded header.
pub const FOLDED_WARNINGS: usize = 3;

/// Reference layout, in pixels at 1080 lines.
const WIDTH: f32 = 400.0;
const TEXT_X: f32 = 16.0;
const TEXT_RIGHT: f32 = 14.0;
const LINE_H: f32 = 18.0;
const BASELINE: f32 = 13.0;
const ITEM_GAP: f32 = 2.0;
/// Header bottom to the first message, and the last message to the panel's bottom.
const ITEMS_TOP: f32 = 2.0;
const PAD_BOTTOM: f32 = 8.0;
const TEXT_PX: f32 = 13.0;

/// A message's brightness by age (seconds since it came in): full while the original holds
/// it, then fading to [`HISTORY_ALPHA`] over the original's fade; full while scrolled back.
pub fn age_alpha(age: f32, scrolled: bool) -> f32 {
    if scrolled || age <= MESSAGE_HOLD {
        1.0
    } else if age >= MESSAGE_HOLD + MESSAGE_FADE {
        HISTORY_ALPHA
    } else {
        1.0 - (1.0 - HISTORY_ALPHA) * (age - MESSAGE_HOLD) / MESSAGE_FADE
    }
}

/// A folded warning's brightness by age: the original's hold and fade, then gone.
pub fn warning_alpha(age: f32) -> f32 {
    if age <= MESSAGE_HOLD {
        1.0
    } else {
        (1.0 - (age - MESSAGE_HOLD) / MESSAGE_FADE).clamp(0.0, 1.0)
    }
}

/// The warnings a folded panel shows at `time`: the newest red messages still held or fading
/// as in the original (none while scrolled back), oldest first, as (index, text).
pub fn live_warnings(messages: &[GameMessage], time: f32, scroll: usize) -> Vec<(usize, &str)> {
    if scroll > 0 {
        return Vec::new();
    }
    let mut v: Vec<(usize, &str)> = messages
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, m)| m.colour == WARNING && time - m.time < MESSAGE_HOLD + MESSAGE_FADE)
        .take(FOLDED_WARNINGS)
        .map(|(i, m)| (i, m.text.as_str()))
        .collect();
    v.reverse();
    v
}

/// A message colour made readable on the dark panel: mixed towards white, more so the darker
/// it is (pure red becomes a light red, pure blue a light blue; white stays white).
pub fn readable(c: [u8; 4]) -> Color {
    let lum = 0.299 * c[0] as f32 + 0.587 * c[1] as f32 + 0.114 * c[2] as f32;
    let k = 0.2 + 0.45 * ((160.0 - lum) / 160.0).max(0.0);
    let m = |v: u8| (v as f32 + (255.0 - v as f32) * k).round() as u8;
    [m(c[0]), m(c[1]), m(c[2]), 255]
}

/// One message's place: its click rectangle and wrapped lines (pen x, baseline, text).
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Its index in the game's messages.
    pub message: usize,
    pub rect: Rect,
    pub lines: Vec<(i32, i32, String)>,
}

/// The panel's layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub expanded: bool,
    /// The header, on the panel's bottom edge.
    pub header: Rect,
    /// The background (the header when collapsed).
    pub panel: Rect,
    /// Oldest first (folded: the live warnings, see [`fold_warnings`]).
    pub items: Vec<Item>,
    /// Folded with warnings: the backdrop behind them, above the header.
    pub warnings: Option<Rect>,
}

/// The messages the panel lists: up to [`HISTORY`] ending at the newest (less the lines
/// scrolled back), oldest first, as (index, text).
pub fn listed(messages: &[GameMessage], scroll: usize) -> Vec<(usize, &str)> {
    let Some(newest) = messages.len().checked_sub(1 + scroll) else {
        return Vec::new();
    };
    let first = (newest + 1).saturating_sub(HISTORY);
    (first..=newest)
        .map(|i| (i, messages[i].text.as_str()))
        .collect()
}

/// Lays the panel out on a canvas `height` lines tall, its bottom on the bottom margin: the
/// messages (oldest first) stacked up from there with the header above them, the oldest
/// dropped while the header would reach above `top`. Folded, the header on the bottom margin.
/// Expanded with no room for one message, the header alone (and `expanded` false).
pub fn layout(
    height: i32,
    top: i32,
    messages: &[(usize, &str)],
    expanded: bool,
    measure: Measure,
) -> Layout {
    let s = modern_hud::scale_for(height);
    let x0 = px(s, mp::MARGIN);
    let x1 = x0 + px(s, WIDTH);
    let y1 = height - px(s, mp::MARGIN);
    let header_h = px(s, obj::HEADER_H);
    let mut items = Vec::new();
    if expanded {
        let text_x = x0 + px(s, TEXT_X);
        let avail = (x1 - px(s, TEXT_RIGHT) - text_x) as f32;
        let width = |t: &str| measure(Face::Semibold, TEXT_PX * s, 0.0, t);
        let line_h = px(s, LINE_H);
        let mut bottom = y1 - px(s, PAD_BOTTOM);
        for &(index, text) in messages.iter().rev() {
            let wrapped = obj::wrap(text, avail, avail, &width);
            let h = wrapped.len() as i32 * line_h;
            let y = bottom - h;
            if y - px(s, ITEMS_TOP) - header_h < top {
                break;
            }
            let lines = wrapped
                .into_iter()
                .enumerate()
                .map(|(i, t)| (text_x, y + i as i32 * line_h + px(s, BASELINE), t))
                .collect();
            items.push(Item {
                message: index,
                rect: Rect::new(x0, y, x1, y + h),
                lines,
            });
            bottom = y - px(s, ITEM_GAP);
        }
        items.reverse();
    }
    let expanded = expanded && !items.is_empty();
    let (header, panel) = match items.first() {
        Some(first) if expanded => {
            let hy1 = first.rect.y0 - px(s, ITEMS_TOP);
            let header = Rect::new(x0, hy1 - header_h, x1, hy1);
            (header, Rect::new(x0, header.y0, x1, y1))
        }
        _ => {
            let w = mp::header_width(measure, s, HEADER_TEXT, None);
            let header = Rect::new(x0, y1 - header_h, (x0 + w).min(x1), y1);
            (header, header)
        }
    };
    Layout {
        scale: s,
        expanded,
        header,
        panel,
        items,
        warnings: None,
    }
}

/// Puts `warnings` (oldest first) on a folded layout, stacked up from just above its header,
/// the oldest dropped while they would reach above `top`. The header and the panel stay as
/// they are, so the fold still hides and shows by the header alone.
pub fn fold_warnings(l: &mut Layout, warnings: &[(usize, &str)], top: i32, measure: Measure) {
    if l.expanded || warnings.is_empty() {
        return;
    }
    let s = l.scale;
    let x0 = l.header.x0;
    let x1 = x0 + px(s, WIDTH);
    let text_x = x0 + px(s, TEXT_X);
    let avail = (x1 - px(s, TEXT_RIGHT) - text_x) as f32;
    let width = |t: &str| measure(Face::Semibold, TEXT_PX * s, 0.0, t);
    let line_h = px(s, LINE_H);
    let y1 = l.header.y0 - px(s, ITEMS_TOP);
    let mut bottom = y1 - px(s, PAD_BOTTOM);
    let mut items = Vec::new();
    for &(index, text) in warnings.iter().rev() {
        let wrapped = obj::wrap(text, avail, avail, &width);
        let y = bottom - wrapped.len() as i32 * line_h;
        if y - px(s, PAD_BOTTOM) < top {
            break;
        }
        let lines = wrapped
            .into_iter()
            .enumerate()
            .map(|(i, t)| (text_x, y + i as i32 * line_h + px(s, BASELINE), t))
            .collect();
        items.push(Item {
            message: index,
            rect: Rect::new(x0, y, x1, bottom),
            lines,
        });
        bottom = y - px(s, ITEM_GAP);
    }
    items.reverse();
    if let Some(first) = items.first() {
        l.warnings = Some(Rect::new(x0, first.rect.y0 - px(s, PAD_BOTTOM), x1, y1));
    }
    l.items = items;
}

/// The panel's state kept in the overlay: the newest message seen (for the flash) and the
/// fold chosen over the open full-screen manager.
#[derive(Debug, Clone, Default)]
pub struct PanelState {
    newest: Option<(u32, String)>,
    last_clock: f32,
    pub flash: Flash,
    /// Expanded (true) or collapsed by a header click while a full-screen manager is open;
    /// None follows the usual rule. Cleared when the manager closes.
    pub over_manager: Option<bool>,
}

impl PanelState {
    /// Notes the newest message at `clock`: a different one than last time starts the flash
    /// (not the first one seen, and not after the clock went back: a new mission).
    pub fn note(&mut self, clock: f32, messages: &[GameMessage]) {
        if clock < self.last_clock {
            self.newest = None;
            self.flash.clear();
        }
        self.last_clock = clock;
        let now = messages.last().map(|m| (m.time.to_bits(), m.text.clone()));
        if now.is_some() && self.newest.is_some() && now != self.newest {
            self.flash.trigger(clock);
        }
        if now.is_some() {
            self.newest = now;
        }
    }
}

/// The panel this frame, or None (no messages yet, or hidden over a manager). `top` is the
/// highest line the messages may reach.
pub(crate) fn arrange(
    fe: &Fe,
    _width: i32,
    height: i32,
    view: &HostView,
    top: i32,
    manager: Option<Rect>,
    measure: Measure,
) -> Option<Layout> {
    let list = listed(&view.messages, fe.game.ig.overlay.message_scroll);
    if list.is_empty() {
        return None;
    }
    let saved = fe.game.options.get_bool(EXPANDED_KEY);
    let over = fe.game.ig.overlay.modern_log.over_manager;
    let full = layout(height, top, &list, true, measure);
    let folded = layout(height, top, &list, false, measure);
    let expanded = mp::shown_state(saved, manager, over, full.panel, folded.panel)?;
    if expanded {
        return Some(full);
    }
    let mut folded = folded;
    let warnings = live_warnings(&view.messages, view.time, fe.game.ig.overlay.message_scroll);
    fold_warnings(&mut folded, &warnings, top, measure);
    // A warning that would reach into a full-screen manager waits for it to close.
    if let (Some(w), Some(m)) = (folded.warnings, manager) {
        if modern_hud::overlaps(w, m) {
            folded.items.clear();
            folded.warnings = None;
        }
    }
    Some(folded)
}

/// The header was clicked: collapse or expand. Over a full-screen manager the choice lasts
/// until the manager closes; otherwise it is remembered in the options file.
pub fn toggle(fe: &mut Fe) {
    let (w, h) = fe.screen_size();
    let view = fe.game.ig.view.clone();
    let shown = mp::arrange(fe, w, h, &view).log.is_some_and(|l| l.expanded);
    if modern_hud::manager_area(fe, w, h).is_some() {
        fe.game.ig.overlay.modern_log.over_manager = Some(!shown);
    } else {
        fe.game.set_option_now(EXPANDED_KEY, i32::from(!shown));
    }
}

/// Draws the panel laid out as `l` for the game's `messages` at mission time `time`;
/// `flash` is the header flash (drawn folded only), `scrolled` the chat scrolled back.
#[allow(clippy::too_many_arguments)]
pub fn paint(
    canvas: &mut Canvas,
    l: &Layout,
    messages: &[GameMessage],
    time: f32,
    scrolled: bool,
    pen: &Pen,
    mouse: (i32, i32),
    flash: f32,
) {
    let s = l.scale;
    if l.expanded {
        mp::paint_frame(canvas, l.panel, s);
    } else if let Some(w) = l.warnings {
        mp::paint_frame(canvas, w, s);
    }
    let hover = l.header.contains(mouse.0, mouse.1);
    let flash = if l.expanded { 0.0 } else { flash };
    mp::paint_header(
        canvas,
        pen,
        l.header,
        s,
        HEADER_TEXT,
        l.expanded,
        hover,
        None,
        flash,
    );
    for item in &l.items {
        let Some(m) = messages.get(item.message) else {
            continue;
        };
        let a = if l.expanded {
            age_alpha(time - m.time, scrolled)
        } else {
            warning_alpha(time - m.time)
        };
        let over = m.ship.is_some() && item.rect.contains(mouse.0, mouse.1);
        let col = if over { obj::GOLD } else { readable(m.colour) };
        let col = modern_hud::with_alpha(col, a);
        for (x, baseline, text) in &item.lines {
            pen.write(
                canvas,
                Face::Semibold,
                TEXT_PX * s,
                0.0,
                *x,
                *baseline,
                col,
                None,
                text,
            );
        }
    }
}

/// Draws the panel as arranged this frame.
pub(crate) fn draw(fe: &Fe, canvas: &mut Canvas, l: &Layout, view: &HostView, clock: f32) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let st = &fe.game.ig.overlay;
    let flash = st.modern_log.flash.strength(clock);
    let scrolled = st.message_scroll > 0;
    let mouse = st.mouse_pos();
    paint(
        canvas,
        l,
        &view.messages,
        view.time,
        scrolled,
        &pen,
        mouse,
        flash,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(_: Face, px: f32, spacing: f32, t: &str) -> f32 {
        let n = t.chars().count() as f32;
        n * px * 0.5 + (n - 1.0).max(0.0) * spacing
    }

    fn msg(text: &str, time: f32) -> GameMessage {
        GameMessage {
            text: text.into(),
            colour: [255, 255, 255, 255],
            ship: None,
            time,
        }
    }

    #[test]
    fn fades_like_the_original_then_stays_dimmed() {
        assert_eq!(age_alpha(0.0, false), 1.0);
        assert_eq!(age_alpha(MESSAGE_HOLD, false), 1.0);
        let mid = age_alpha(MESSAGE_HOLD + MESSAGE_FADE / 2.0, false);
        assert!((mid - 0.75).abs() < 1e-5, "{mid}");
        assert_eq!(age_alpha(MESSAGE_HOLD + MESSAGE_FADE, false), HISTORY_ALPHA);
        assert_eq!(age_alpha(600.0, false), HISTORY_ALPHA);
        assert_eq!(age_alpha(600.0, true), 1.0, "scrolled back: full");
    }

    #[test]
    fn colours_keep_their_meaning_and_read() {
        assert_eq!(readable([255, 255, 255, 255]), [255, 255, 255, 255]);
        let red = readable([255, 0, 0, 255]);
        assert_eq!(red[0], 255);
        assert!(red[1] > 60 && red[1] < 140 && red[1] == red[2], "{red:?}");
        let green = readable([0, 255, 0, 255]);
        assert!(
            green[1] == 255 && green[0] > 40 && green[0] < 120,
            "{green:?}"
        );
        let blue = readable([0, 0, 255, 255]);
        assert!(blue[2] == 255 && blue[0] > 120, "{blue:?}");
        // Still clearly a hue: the dominant channel stays the largest.
        assert!(red[0] > red[1] + 100 && blue[2] > blue[0] + 50);
    }

    #[test]
    fn listing_keeps_the_last_eight_and_scrolls() {
        let m: Vec<GameMessage> = (0..12).map(|i| msg(&format!("m{i}"), i as f32)).collect();
        let l = listed(&m, 0);
        assert_eq!(l.len(), HISTORY);
        assert_eq!(l.first().unwrap(), &(4, "m4"));
        assert_eq!(l.last().unwrap(), &(11, "m11"), "the newest last");
        let l = listed(&m, 3);
        assert_eq!(l.last().unwrap().0, 8);
        assert_eq!(l.len(), HISTORY);
        assert_eq!(listed(&m, 11), vec![(0, "m0")]);
        assert!(listed(&m, 12).is_empty());
        assert!(listed(&[], 0).is_empty());
    }

    #[test]
    fn layout_grows_up_from_the_corner_with_the_header_on_top() {
        let list = [
            (0, "ACOLYTE built"),
            (1, "Research complete: Fighter Drive"),
        ];
        let l = layout(1080, 0, &list, true, &mono);
        assert!(l.expanded);
        assert_eq!(l.items.len(), 2);
        // From the bottom margin up: the newest, the older one, then the header on top.
        let newest = &l.items[1];
        assert_eq!(newest.message, 1, "the newest at the bottom");
        assert_eq!(newest.rect, Rect::new(16, 1056 - 18, 416, 1056));
        assert_eq!(newest.lines, vec![(32, 1038 + 13, list[1].1.to_string())]);
        assert_eq!(l.items[0].rect, Rect::new(16, 1018, 416, 1036));
        assert_eq!(l.header, Rect::new(16, 986, 416, 1016));
        assert_eq!(l.panel, Rect::new(16, 986, 416, 1064));
        // Folded: the header alone, in the bottom-left corner.
        let f = layout(1080, 0, &list, false, &mono);
        assert!(!f.expanded && f.items.is_empty());
        assert_eq!(
            f.header,
            Rect::new(
                16,
                1034,
                16 + mp::header_width(&mono, 1.0, HEADER_TEXT, None),
                1064
            )
        );
        assert_eq!(f.panel.y1, l.panel.y1, "both end on the bottom margin");
        assert_eq!(f.panel, f.header);
    }

    #[test]
    fn a_folded_log_still_shows_live_warnings() {
        let warn = |text: &str, time: f32| GameMessage {
            colour: WARNING,
            ..msg(text, time)
        };
        let messages = [
            warn(
                "Complete or Cancel External Construction Before Continuing",
                10.0,
            ),
            msg("ACOLYTE built", 11.0),
            warn("cannot save", 12.0),
        ];
        // Red only, oldest first, while the original holds or fades them; none scrolled back.
        assert_eq!(
            live_warnings(&messages, 12.0, 0),
            vec![(0, messages[0].text.as_str()), (2, "cannot save")]
        );
        assert_eq!(
            live_warnings(&messages, 10.0 + MESSAGE_HOLD + MESSAGE_FADE, 0),
            vec![(2, "cannot save")]
        );
        assert!(live_warnings(&messages, 12.0, 1).is_empty());
        assert_eq!(warning_alpha(MESSAGE_HOLD), 1.0);
        assert_eq!(warning_alpha(MESSAGE_HOLD + MESSAGE_FADE), 0.0);
        // Above the folded header, which stays where it was.
        let list = listed(&messages, 0);
        let mut f = layout(1080, 0, &list, false, &mono);
        let header = f.header;
        let w = live_warnings(&messages, 12.0, 0);
        fold_warnings(&mut f, &w, 0, &mono);
        assert!(!f.expanded);
        assert_eq!((f.header, f.panel), (header, header));
        assert_eq!(
            f.items.iter().map(|i| i.message).collect::<Vec<_>>(),
            vec![0, 2]
        );
        let back = f.warnings.expect("a backdrop");
        assert!(
            back.y1 <= header.y0
                && f.items
                    .iter()
                    .all(|i| i.rect.y0 >= back.y0 && i.rect.y1 <= back.y1)
        );
        // No room above the header: nothing.
        let mut g = layout(1080, 0, &list, false, &mono);
        fold_warnings(&mut g, &w, header.y0 - 4, &mono);
        assert!(g.items.is_empty() && g.warnings.is_none());
        // Expanded layouts are left alone.
        let mut e = layout(1080, 0, &list, true, &mono);
        let before = e.clone();
        fold_warnings(&mut e, &w, 0, &mono);
        assert_eq!(e, before);
    }

    #[test]
    fn long_messages_wrap_and_the_oldest_give_way() {
        let long = "Complete or Cancel External Construction Before Continuing with the jump";
        let list = [(0, "old"), (1, long)];
        let l = layout(1080, 0, &list, true, &mono);
        assert!(l.items[1].lines.len() >= 2, "wraps");
        for (_, _, t) in &l.items[1].lines {
            assert!(mono(Face::Semibold, 13.0, 0.0, t) <= (416 - 14 - 32) as f32);
        }
        // Room for only the newest.
        let top = l.items[1].rect.y0 - 2 - 30;
        let l2 = layout(1080, top, &list, true, &mono);
        assert_eq!(l2.items.len(), 1);
        assert_eq!(l2.items[0].message, 1);
        // No room for any: folded.
        let l3 = layout(1080, 1030, &list, true, &mono);
        assert!(!l3.expanded);
        // 640x480 scale.
        let l4 = layout(480, 0, &list, true, &mono);
        assert_eq!(l4.panel.y1, 480 - 7);
        assert_eq!(l4.header.y0, l4.panel.y0);
        assert_eq!(l4.header.width(), 178);
    }

    #[test]
    fn a_new_message_flashes() {
        let mut st = PanelState::default();
        let mut m = vec![msg("a", 1.0)];
        st.note(1.0, &m);
        assert_eq!(
            st.flash.strength(1.1),
            0.0,
            "the first sighting does not flash"
        );
        st.note(2.0, &m);
        assert_eq!(st.flash.strength(2.1), 0.0);
        m.push(msg("b", 2.5));
        st.note(3.0, &m);
        assert!(st.flash.strength(3.05) > 0.5);
        // The list capped at 50 drops the oldest: still flashes on a new newest only.
        m.remove(0);
        st.note(6.0, &m);
        assert_eq!(st.flash.strength(6.05), 0.0);
        // A new mission: no flash.
        st.note(0.5, &[msg("c", 0.1)]);
        assert_eq!(st.flash.strength(0.55), 0.0);
    }
}
