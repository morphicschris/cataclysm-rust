//! The build manager (`construction_manager.fib`, [`consmgr`]) restyled in the modern HUD's
//! look: a user-requested change, on by default; `--build-ui original`,
//! `hwcModernBuildManager = 0` or the original HUD (`--hud original`) bring back the
//! original screen. The shared look is [`super::modern_manager`].
//!
//! Only the look and the mouse handling are new. The original screen stays open underneath
//! (not drawn, and not given the mouse), so its hot keys (B, P, L, C, I, R, S, Return,
//! Escape, Tab / arrow focus), its callbacks and every rule of [`consmgr`] (picked counts,
//! live queue edits, caps, support units, speech, sounds) are the same code. The mouse acts
//! through the same functions the original's regions call:
//!
//! * factory tabs: `cmSelectMotherShip` / `cmSelectCarrier1/2` (a press; the second press of
//!   a double click is not seen, as there);
//! * a ship row: `cmSelectAvailable` (left: select, or add when selected, Shift toggles,
//!   Ctrl adds five; right: remove; the second press of a double click acts once);
//! * the - / + controls: `cmLeftArrowProcess` / `cmRightArrowProcess` (held, they repeat
//!   every quarter second; doubles are not seen);
//! * buttons: pressed with the click sound, run their callback on release over them (never
//!   on the second press of a double click), like `uicButtonProcess`;
//! * the wheel over the list scrolls three rows, the thin scroll bar drags.
//!
//! The panel is centred and scales with the window height (laid out for 1080 lines, never
//! larger than the window); the 3D view around it is dimmed a little and the modern HUD's
//! panels stay up, folding or hiding where they would cover it.

use std::collections::BTreeMap;

use crate::canvas::{BlitMode, Canvas, Rect, WHITE};
use crate::consts::*;
use crate::ingame::consmgr::{self, Row};
use crate::ingame::host::{BuildOption, ShipType};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::{Fe, Manager};
use crate::options::Options;

/// The options-file key: 1 (default) this build manager, 0 the original's.
pub const OPTION_KEY: &str = "hwcModernBuildManager";

/// `--build-ui`, the options file and the HUD's choice.
static SWITCH: Switch = Switch::new(OPTION_KEY);

/// Sets the command line's choice; `None`: the options file decides.
pub fn set_override(modern: Option<bool>) {
    SWITCH.set_override(modern);
}

/// Whether the modern build manager replaces the original's look.
pub fn enabled(options: &Options) -> bool {
    SWITCH.enabled(options)
}

/// Parses `--build-ui`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    mm::parse_mode(s)
}

/// Whether the modern build manager is up now (enabled, and the build manager open).
pub fn active(fe: &Fe) -> bool {
    fe.game.ig.manager == Some(Manager::Construction) && enabled(&fe.game.options)
}

use crate::ingame::modern_manager::{self as mm, *};

/// How long a fired - / + stays lit.
const FIRED_SECONDS: f64 = 0.18;

// ---- layout (at 1080 lines; the panel, header and footer are [`mm::Frame`]'s) ----

pub use crate::ingame::modern_manager::{REF_H, REF_W};
const TABS_Y: f32 = 68.0;
const TAB_H: f32 = 36.0;
const TAB_W: f32 = 200.0;
const TAB_GAP: f32 = 8.0;
const BODY_Y: f32 = 118.0;
const LEFT_W: f32 = 532.0;
const RIGHT_X: f32 = 580.0;
const STATS_H: f32 = 38.0;
const STAT_GAP: f32 = 16.0;
const LIST_HEAD_BASELINE: f32 = 182.0;
const LIST_Y: f32 = 194.0;
const ROW_H: f32 = 24.0;
const LIST_ROWS: f32 = 17.0;
const TOTALS_Y: f32 = 612.0;
const TOTALS_H: f32 = 30.0;
const PREVIEW_H: f32 = 300.0;
const STEP_BOX: f32 = 20.0;
const SHIP_TITLE_PX: f32 = mm::TITLE_BIG_PX;

/// The buttons of the original screen, by their atom (and callback) names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Build,
    Pause,
    Cancel,
    Close,
    Research,
    Systems,
    Info,
}

impl Button {
    pub const ALL: [Button; 7] = [
        Button::Build,
        Button::Pause,
        Button::Cancel,
        Button::Close,
        Button::Research,
        Button::Systems,
        Button::Info,
    ];

    /// The atom of `construction_manager.fib` the button stands for (its callback's name).
    pub fn atom(self) -> &'static str {
        match self {
            Button::Build => "CM_BuildShips",
            Button::Pause => "CM_PauseJobs",
            Button::Cancel => "CM_CancelJobs",
            Button::Close => "CM_Close",
            Button::Research => "CM_Research",
            Button::Systems => "CM_Launch",
            Button::Info => "CM_ExtendedInfo",
        }
    }

    /// The label, as the screen's text (without its `&` hot-key mark).
    pub fn label(self) -> &'static str {
        match self {
            Button::Build => "BUILD",
            Button::Pause => "PAUSE BATCH",
            Button::Cancel => "CANCEL BATCH",
            Button::Close => "CLOSE",
            Button::Research => "RESEARCH",
            Button::Systems => "SYSTEMS",
            Button::Info => "INFO",
        }
    }

    /// The hot key shown on the button (the letter the screen's `&` marks).
    pub fn key(self) -> &'static str {
        match self {
            Button::Build => "B",
            Button::Pause => "P",
            Button::Cancel => "L",
            Button::Close => "C",
            Button::Research => "R",
            Button::Systems => "S",
            Button::Info => "I",
        }
    }
}

/// Where everything goes on a canvas of a given size.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub scale: f32,
    /// The panel, header and footer line (shared with the other modern managers).
    pub frame: Frame,
    pub panel: Rect,
    pub header: Rect,
    pub title_x: i32,
    pub title_baseline: i32,
    /// The factory tabs: mothership, first and second carrier.
    pub tabs: [Rect; 3],
    pub ru: Rect,
    pub su: Rect,
    pub list_head_baseline: i32,
    /// The rows' area; `page` rows of `row_h` fit.
    pub list: Rect,
    pub row_h: i32,
    pub page: usize,
    pub scroll_track: Rect,
    /// Columns: a heading's x, a ship name's x, the - / count / + centres, the cost's right
    /// edge, the - / + boxes' size.
    pub heading_x: i32,
    pub name_x: i32,
    pub minus_cx: i32,
    pub count_cx: i32,
    pub plus_cx: i32,
    pub cost_right: i32,
    pub step_box: i32,
    pub totals: Rect,
    pub preview: Rect,
    pub details: Rect,
    pub footer_line: i32,
    pub buttons: BTreeMap<Button, Rect>,
}

/// The panel's scale for a `width` x `height` canvas (see [`mm::scale_for`]).
pub fn scale_for(width: i32, height: i32) -> f32 {
    mm::scale_for(width, height)
}

/// The band along the top kept for the modern HUD's panel headers (see [`mm::top_band`]).
pub fn top_band(height: i32) -> i32 {
    mm::top_band(height)
}

/// The layout for a `width` x `height` canvas.
pub fn layout(width: i32, height: i32) -> Layout {
    let f = Frame::new(width, height);
    let s = f.scale;
    let px = |v: f32| f.px(v);
    let (x0, y0) = (f.panel.x0, f.panel.y0);
    let at = |x: f32, y: f32, w: f32, h: f32| f.at(x, y, w, h);
    let tabs = [0.0, 1.0, 2.0].map(|i| at(PAD + i * (TAB_W + TAB_GAP), TABS_Y, TAB_W, TAB_H));
    let half = (LEFT_W - STAT_GAP) / 2.0;
    let ru = at(PAD, BODY_Y, half, STATS_H);
    let su = at(PAD + half + STAT_GAP, BODY_Y, half, STATS_H);
    let row_h = px(ROW_H).max(10);
    let list = Rect::new(
        x0 + px(PAD),
        y0 + px(LIST_Y),
        x0 + px(PAD + LEFT_W - 10.0),
        y0 + px(LIST_Y) + row_h * LIST_ROWS as i32,
    );
    let scroll_track = Rect::new(
        x0 + px(PAD + LEFT_W - 4.0),
        list.y0,
        x0 + px(PAD + LEFT_W - 4.0) + px(3.0).max(2),
        list.y1,
    );
    let cost_right = list.x1 - px(10.0);
    let plus_cx = cost_right - px(70.0);
    let count_cx = plus_cx - px(30.0);
    let minus_cx = count_cx - px(30.0);
    let preview = at(RIGHT_X, BODY_Y, REF_W - PAD - RIGHT_X, PREVIEW_H);
    let details = Rect::new(
        preview.x0,
        preview.y1 + px(10.0),
        preview.x1,
        y0 + px(TOTALS_Y + TOTALS_H),
    );
    let mut buttons = BTreeMap::new();
    for (i, b) in [Button::Build, Button::Pause, Button::Cancel]
        .into_iter()
        .enumerate()
    {
        buttons.insert(b, f.footer_button(i));
    }
    buttons.insert(Button::Close, f.footer_right());
    buttons.insert(Button::Systems, f.top_button(0));
    buttons.insert(Button::Research, f.top_button(1));
    buttons.insert(
        Button::Info,
        Rect::new(
            details.x1 - px(76.0),
            details.y0 + px(4.0),
            details.x1,
            details.y0 + px(30.0),
        ),
    );
    Layout {
        scale: s,
        frame: f,
        panel: f.panel,
        header: f.header,
        title_x: f.title_x,
        title_baseline: f.title_baseline,
        tabs,
        ru,
        su,
        list_head_baseline: y0 + px(LIST_HEAD_BASELINE),
        list,
        row_h,
        page: LIST_ROWS as usize,
        scroll_track,
        heading_x: list.x0 + px(4.0),
        name_x: list.x0 + px(14.0),
        minus_cx,
        count_cx,
        plus_cx,
        cost_right,
        step_box: px(STEP_BOX).max(9),
        totals: Rect::new(
            list.x0,
            y0 + px(TOTALS_Y),
            list.x1,
            y0 + px(TOTALS_Y + TOTALS_H),
        ),
        preview,
        details,
        footer_line: f.footer_line,
        buttons,
    }
}

impl Layout {
    fn px(&self, v: f32) -> i32 {
        self.frame.px(v)
    }

    fn text(&self, v: f32) -> f32 {
        self.frame.text(v)
    }

    /// The rectangle of the `i`-th row on screen.
    pub fn row(&self, i: usize) -> Rect {
        let y = self.list.y0 + i as i32 * self.row_h;
        Rect::new(self.list.x0, y, self.list.x1, y + self.row_h)
    }

    fn step(&self, i: usize, cx: i32) -> Rect {
        let r = self.row(i);
        let cy = (r.y0 + r.y1) / 2;
        let h = self.step_box;
        Rect::new(cx - h / 2, cy - h / 2, cx - h / 2 + h, cy - h / 2 + h)
    }

    /// The - control of the `i`-th row on screen.
    pub fn minus(&self, i: usize) -> Rect {
        self.step(i, self.minus_cx)
    }

    /// The + control of the `i`-th row on screen.
    pub fn plus(&self, i: usize) -> Rect {
        self.step(i, self.plus_cx)
    }

    /// The scroll bar's thumb for `total` rows from `top`; None when every row fits.
    pub fn thumb(&self, total: usize, top: usize) -> Option<Rect> {
        mm::thumb(self.scroll_track, self.page, total, top, self.px(24.0))
    }

    /// The first row for the thumb's top at `y` (dragging).
    pub fn top_for_thumb(&self, total: usize, y: i32) -> usize {
        mm::top_for_thumb(self.scroll_track, self.page, total, y, self.px(24.0))
    }
}

/// What is under a point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hit {
    Button(Button),
    Tab(usize),
    /// The `i`-th row on screen (a heading or a ship).
    Row(usize),
    Minus(usize),
    Plus(usize),
    Thumb,
    /// The scroll track above (true) or below the thumb.
    Track(bool),
    /// Elsewhere on the panel.
    Panel,
    Outside,
}

/// Hit-testing: what is at (`x`, `y`) with `rows` rows on screen (`total` in the list, the
/// first at `top`).
pub fn hit(l: &Layout, x: i32, y: i32, rows: usize, total: usize, top: usize) -> Hit {
    if !l.panel.contains(x, y) {
        return Hit::Outside;
    }
    for (&b, r) in &l.buttons {
        if r.contains(x, y) {
            return Hit::Button(b);
        }
    }
    if let Some(i) = l.tabs.iter().position(|t| t.contains(x, y)) {
        return Hit::Tab(i);
    }
    let track = Rect::new(
        l.scroll_track.x0 - l.px(5.0),
        l.scroll_track.y0,
        l.scroll_track.x1 + l.px(5.0),
        l.scroll_track.y1,
    );
    if track.contains(x, y) {
        if let Some(th) = l.thumb(total, top) {
            if y >= th.y0 && y < th.y1 {
                return Hit::Thumb;
            }
            return Hit::Track(y < th.y0);
        }
    }
    if l.list.contains(x, y) {
        let i = ((y - l.list.y0) / l.row_h) as usize;
        if i < rows {
            // A little slack around the - / + boxes.
            let grow = |r: Rect| {
                let g = l.px(3.0);
                Rect::new(r.x0 - g, l.row(i).y0, r.x1 + g, l.row(i).y1)
            };
            if grow(l.minus(i)).contains(x, y) {
                return Hit::Minus(i);
            }
            if grow(l.plus(i)).contains(x, y) {
                return Hit::Plus(i);
            }
            return Hit::Row(i);
        }
    }
    Hit::Panel
}

/// What eases its hover / press highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Button(Button),
    Tab(usize),
    Row(ShipType),
    Minus(ShipType),
    Plus(ShipType),
    Thumb,
}

/// The modern manager's own state (the build manager's is [`consmgr::State`]).
#[derive(Debug, Clone, Default)]
pub struct State {
    mouse: (i32, i32),
    /// Left press taken by a button, a - / + or the thumb.
    pressed: Option<Hit>,
    /// The double-click rule.
    clicks: DoubleClick,
    /// A held - / +: (right, next repeat time).
    hold: Option<(bool, f64)>,
    /// Dragging the thumb: the pointer's offset from its top.
    drag: Option<i32>,
    /// Eased highlights.
    hover: Hover<Key>,
    /// The - / + that last fired, and when.
    fired: Option<(ShipType, bool, f64)>,
}

/// Opening the build manager: a fresh state.
pub fn reset(fe: &mut Fe) {
    fe.game.ig.mbm = State::default();
}

/// The canvas size (the screens' size).
fn size(fe: &Fe) -> (i32, i32) {
    fe.screen_size()
}

/// The rows on screen and how many rows the list has.
fn on_screen(fe: &Fe, l: &Layout) -> (Vec<Row>, usize, usize) {
    let all = consmgr::rows(&fe.game.ig.view);
    let total = all.len();
    let top = fe.game.ig.cm.top_row.min(total.saturating_sub(l.page));
    let rows = all.into_iter().skip(top).take(l.page).collect();
    (rows, total, top)
}

fn hit_now(fe: &Fe, l: &Layout, x: i32, y: i32) -> (Hit, Vec<Row>) {
    let (rows, total, top) = on_screen(fe, l);
    (hit(l, x, y, rows.len(), total, top), rows)
}

fn ship_of(rows: &[Row], i: usize) -> Option<BuildOption> {
    match rows.get(i) {
        Some(Row::Ship(o)) => Some(o.clone()),
        _ => None,
    }
}

/// The pointer moved.
pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    fe.game.ig.mbm.mouse = (x, y);
    if let Some(grab) = fe.game.ig.mbm.drag {
        let (w, h) = size(fe);
        let l = layout(w, h);
        let total = consmgr::rows(&fe.game.ig.view).len();
        let top = l.top_for_thumb(total, y - grab);
        consmgr::scroll_to(fe, top, l.page);
    }
}

/// A mouse button over the screen (the modern HUD's panels have had their turn).
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool) {
    let (w, h) = size(fe);
    let l = layout(w, h);
    let (x, y) = fe.game.ig.mbm.mouse;
    let (at, rows) = hit_now(fe, &l, x, y);
    if !left {
        if down {
            if let Hit::Row(i) | Hit::Minus(i) | Hit::Plus(i) = at {
                if let Some(o) = ship_of(&rows, i) {
                    consmgr::list_event(fe, &o, RPE_PRESS_RIGHT);
                    note_fired(fe);
                }
            }
        }
        return;
    }
    if !down {
        fe.game.ig.mbm.clicks.release();
        let pressed = fe.game.ig.mbm.pressed.take();
        fe.game.ig.mbm.hold = None;
        fe.game.ig.mbm.drag = None;
        if let (Some(Hit::Button(b)), Hit::Button(now)) = (pressed, at) {
            if b == now {
                activate(fe, b);
            }
        }
        return;
    }
    // The second press of a double click (`WM_LBUTTONDBLCLK`): only the list sees it.
    let now = fe.time;
    let st = &mut fe.game.ig.mbm;
    if st.clicks.held() || st.pressed.is_some() {
        return;
    }
    if st.clicks.press(now, x, y) {
        if let Hit::Row(i) = at {
            if let Some(o) = ship_of(&rows, i) {
                consmgr::list_event(fe, &o, RPE_DOUBLE_LEFT);
                note_fired(fe);
            }
        }
        return;
    }
    match at {
        Hit::Button(_) => {
            fe.game.ig.mbm.pressed = Some(at);
            fe.ui_sound(sound::CLICK);
        }
        Hit::Tab(slot) => consmgr::select_factory(fe, RPE_PRESS_LEFT, slot),
        Hit::Row(i) => {
            if let Some(o) = ship_of(&rows, i) {
                consmgr::list_event(fe, &o, RPE_PRESS_LEFT);
                note_fired(fe);
            }
        }
        Hit::Minus(_) | Hit::Plus(_) => {
            let right = matches!(at, Hit::Plus(_));
            fe.game.ig.mbm.pressed = Some(at);
            fe.game.ig.mbm.hold = Some((right, now - consmgr::REPEAT_SECONDS));
            repeat(fe);
        }
        Hit::Thumb => {
            let total = rows_total(fe);
            if let Some(th) = l.thumb(total, fe.game.ig.cm.top_row) {
                fe.game.ig.mbm.pressed = Some(at);
                fe.game.ig.mbm.drag = Some(y - th.y0);
            }
        }
        Hit::Track(up) => {
            let page = l.page as isize;
            scroll_by(fe, if up { -page } else { page });
        }
        Hit::Panel | Hit::Outside => {}
    }
}

fn rows_total(fe: &Fe) -> usize {
    consmgr::rows(&fe.game.ig.view).len()
}

fn scroll_by(fe: &mut Fe, delta: isize) {
    let (w, h) = size(fe);
    let l = layout(w, h);
    let top = (fe.game.ig.cm.top_row as isize + delta).max(0) as usize;
    consmgr::scroll_to(fe, top, l.page);
}

/// The wheel: three rows over the list, as the original's list and arrows.
pub fn wheel(fe: &mut Fe, up: bool) {
    let (w, h) = size(fe);
    let l = layout(w, h);
    let (x, y) = fe.game.ig.mbm.mouse;
    if l.list.contains(x, y)
        || l.scroll_track.x0 - l.px(6.0) <= x
            && x < l.scroll_track.x1 + l.px(6.0)
            && l.list.y0 <= y
            && y < l.list.y1
    {
        scroll_by(fe, if up { -3 } else { 3 });
    }
}

/// Takes the "an arrow fired" mark the build manager leaves for its arrows' drawing.
fn note_fired(fe: &mut Fe) {
    if let Some((t, right)) = fe.game.ig.cm.arrow_fired.take() {
        fe.game.ig.mbm.fired = Some((t, right, fe.time));
    }
}

/// A held - / +: fires at once and then every quarter second while the pointer stays on
/// that side of a ship row (the row under it then, as the original's arrows).
fn repeat(fe: &mut Fe) {
    let Some((right, next)) = fe.game.ig.mbm.hold else {
        return;
    };
    if fe.time < next {
        return;
    }
    let (w, h) = size(fe);
    let l = layout(w, h);
    let (x, y) = fe.game.ig.mbm.mouse;
    let (at, rows) = hit_now(fe, &l, x, y);
    let i = match (at, right) {
        (Hit::Plus(i), true) | (Hit::Minus(i), false) => i,
        _ => return,
    };
    let Some(o) = ship_of(&rows, i) else { return };
    fe.game.ig.mbm.hold = Some((right, fe.time + consmgr::REPEAT_SECONDS));
    let n = if consmgr::ctrl(fe) { 5 } else { 1 };
    if right {
        if consmgr::add(fe, &o, n) {
            fe.game.ig.cm.arrow_fired = Some((o.ship_type, true));
        }
    } else {
        consmgr::remove(fe, &o, n);
        fe.game.ig.cm.arrow_fired = Some((o.ship_type, false));
    }
    fe.game.ig.cm.ship_view = Some(o.ship_type);
    note_fired(fe);
}

/// A button released over itself: its callback, as the screen's button would run it.
fn activate(fe: &mut Fe, b: Button) {
    let atom = fe.find_atom(b.atom());
    fe.function_execute(b.atom(), atom, false);
}

/// Once a frame while open: held - / + repeat, the list's page size.
pub fn tick(fe: &mut Fe) {
    if !active(fe) {
        return;
    }
    repeat(fe);
    let (w, h) = size(fe);
    let l = layout(w, h);
    let top = fe.game.ig.cm.top_row;
    consmgr::scroll_to(fe, top, l.page);
}

/// The panel (for the modern HUD's panels to keep clear of), when up.
pub fn area(fe: &Fe, width: i32, height: i32) -> Option<Rect> {
    active(fe).then(|| layout(width, height).panel)
}

/// Where the application should render the 3D ship and which type: the preview box.
pub fn ship_view(fe: &Fe) -> Option<(Rect, ShipType)> {
    if !active(fe) {
        return None;
    }
    let t = fe.game.ig.cm.ship_view?;
    let (w, h) = size(fe);
    let p = layout(w, h).preview;
    Some((Rect::new(p.x0 + 1, p.y0 + 1, p.x1 - 1, p.y1 - 1), t))
}

/// A thin glowing bar (track, glow, gradient fill).
fn bar(canvas: &mut Canvas, l: &Layout, r: Rect, fraction: f32, glow: f32) {
    mm::bar(canvas, &l.frame, r, fraction, glow);
}

// ---- drawing ----

/// Everything the drawing needs to know about one ship row.
struct ShipRow {
    pending: i32,
    queued: i32,
    building: bool,
    paused: bool,
    progress: f32,
    batch: f32,
    selected: bool,
    refused: f32,
}

fn ship_row(fe: &Fe, o: &BuildOption) -> ShipRow {
    let pending = consmgr::pending(fe, o.ship_type);
    let job = consmgr::job(fe, o.ship_type).filter(|j| j.count > 0);
    let (queued, paused, progress, batch) = job.map_or((0, false, 0.0, 0.0), |j| {
        let p = j.progress.clamp(0.0, 1.0);
        let total = j.batch_total.max(j.count).max(1) as f32;
        let left = (j.count - 1).max(0) as f32 + (1.0 - p);
        (j.count, j.paused, p, (1.0 - left / total).clamp(0.0, 1.0))
    });
    let refused = match fe.game.ig.cm.flash {
        Some((t, at)) if t == o.ship_type => {
            let age = fe.time - at;
            if (0.0..consmgr::FLASH_SECONDS).contains(&age) {
                1.0 - (age / consmgr::FLASH_SECONDS) as f32
            } else {
                0.0
            }
        }
        _ => 0.0,
    };
    ShipRow {
        pending,
        queued,
        building: job.is_some(),
        paused,
        progress,
        batch,
        selected: fe.game.ig.cm.selected.contains(&o.ship_type),
        refused,
    }
}

/// Steps the eased highlights towards what is under the pointer (and pressed) now.
fn step_hover(fe: &mut Fe, targets: &[(Key, f32)]) {
    let now = fe.time;
    fe.game.ig.mbm.hover.step(now, targets);
}

fn eased(fe: &Fe, key: Key) -> f32 {
    fe.game.ig.mbm.hover.get(key)
}

/// Whether the screen's keyboard focus (Tab / arrows) is on a button's atom.
pub(crate) fn focused(fe: &Fe, atom: &str) -> bool {
    fe.find_atom(atom)
        .and_then(|a| fe.atom(a).region)
        .and_then(|id| fe.region(id))
        .is_some_and(|r| r.status & RSF_CURRENT_SELECTED != 0)
}

/// Draws the manager over the 3D view in `canvas` (the modern HUD's panels come after).
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let l = layout(canvas.width, canvas.height);
    let (mx, my) = fe.game.ig.mbm.mouse;
    let (rows, total, top) = on_screen(fe, &l);
    let at = hit(&l, mx, my, rows.len(), total, top);
    let pressed = fe.game.ig.mbm.pressed;
    // Highlights: what is under the pointer, brighter while pressed.
    let mut targets = Vec::new();
    let strength = |h: Hit| if pressed == Some(h) { 1.0 } else { 0.6 };
    match at {
        Hit::Button(b) => targets.push((Key::Button(b), strength(at))),
        Hit::Tab(i) => targets.push((Key::Tab(i), 0.6)),
        Hit::Row(i) | Hit::Minus(i) | Hit::Plus(i) => {
            if let Some(o) = ship_of(&rows, i) {
                targets.push((Key::Row(o.ship_type), 0.6));
                if let Hit::Minus(_) = at {
                    targets.push((Key::Minus(o.ship_type), strength(at)));
                }
                if let Hit::Plus(_) = at {
                    targets.push((Key::Plus(o.ship_type), strength(at)));
                }
            }
        }
        Hit::Thumb => targets.push((Key::Thumb, 0.6)),
        _ => {}
    }
    if let Some(Hit::Button(b)) = pressed {
        if at != Hit::Button(b) {
            targets.push((Key::Button(b), 0.25));
        }
    }
    if fe.game.ig.mbm.drag.is_some() {
        targets.push((Key::Thumb, 1.0));
    }
    step_hover(fe, &targets);

    // The scene around, the frame.
    mm::draw_frame(canvas, &l.frame);

    draw_header(fe, canvas, &pen, &l);
    draw_tabs(fe, canvas, &pen, &l);
    draw_stats(fe, canvas, &pen, &l);
    draw_list(fe, canvas, &pen, &l, &rows, total, top);
    draw_totals(fe, canvas, &pen, &l);
    draw_preview(fe, canvas, &pen, &l);
    draw_details(fe, canvas, &pen, &l);
    for b in Button::ALL {
        draw_button(fe, canvas, &pen, &l, b);
    }
}

fn draw_header(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    // The current factory after a thin divider.
    let view = &fe.game.ig.view;
    let name = fe
        .game
        .ig
        .cm
        .factory
        .and_then(|s| view.factories.iter().find(|f| f.ship == s))
        .map(|f| f.name.to_uppercase());
    mm::draw_title(canvas, pen, &l.frame, "BUILD MANAGER", name.as_deref());
}

/// The factory tabs: mothership and two carriers, each with its queue count and the ship
/// it is building; an empty slot is shown disabled.
fn draw_tabs(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let view = &fe.game.ig.view;
    for (slot, &r) in l.tabs.iter().enumerate() {
        let f = consmgr::factory_in_slot(view, slot);
        let current = f.is_some_and(|f| Some(f.ship) == fe.game.ig.cm.factory);
        let jobs: Vec<_> = f
            .map(|f| view.jobs.iter().filter(|j| j.factory == f.ship).collect())
            .unwrap_or_default();
        // The ship in production at that factory, as a share of the tab's bottom edge.
        let progress = jobs
            .iter()
            .find(|j| j.count > 0 && !j.paused)
            .map(|j| j.progress.clamp(0.0, 1.0));
        let label = match f {
            Some(f) => f.name.to_uppercase(),
            None if slot == 0 => "NO COMMAND SHIP".to_string(),
            None => format!("CARRIER {slot} \u{2014} NONE"),
        };
        let count: i32 = jobs.iter().map(|j| j.count).sum();
        let look = TabLook {
            label,
            enabled: f.is_some(),
            current,
            hover: eased(fe, Key::Tab(slot)),
            count: (count > 0).then(|| count.to_string()),
            progress,
            paused: false,
            detail: None,
        };
        mm::draw_tab(canvas, pen, &l.frame, r, &look);
    }
}

/// Current RUs and the support units (used, plus what is queued and picked).
fn draw_stats(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let p = &fe.game.ig.view.player;
    let f = &l.frame;
    let glow = value_glow(f);
    // RUs.
    let br = mm::readout(
        canvas,
        pen,
        f,
        l.ru,
        "CURRENT RUS",
        &modern_hud::thousands(p.resource_units),
        WHITE,
        glow,
    );
    bar(canvas, l, br, modern_hud::ru_fill(p.resource_units), 1.0);
    // Support units: used (gold), then what the queues and picked counts need (paler).
    let pending = p.support_pending + consmgr::gui_support_units(fe);
    let cap = p.support_capacity;
    let at_cap = p.support_used + pending >= cap && cap > 0 && pending + p.support_used > 0;
    let value = format!("{} / {}", p.support_used + pending, cap);
    let (vcol, vglow) = if at_cap {
        (
            modern_hud::AT_CAP,
            Some(([0xFF, 0x4A, 0x1E, 140], 3.0 * l.scale)),
        )
    } else {
        (WHITE, glow)
    };
    let br = mm::readout(canvas, pen, f, l.su, "SUPPORT UNITS", &value, vcol, vglow);
    let used = modern_hud::su_fill(p.support_used, cap);
    bar(canvas, l, br, used, 1.0);
    let both = modern_hud::su_fill(p.support_used + pending, cap);
    if both > used {
        let x0 = br.x0 + (br.width() as f32 * used).round() as i32;
        let x1 = br.x0 + (br.width() as f32 * both).round() as i32;
        rounded(
            canvas,
            Rect::new(x0, br.y0, x1, br.y1),
            1.5,
            [255, 230, 160, 120],
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    fe: &Fe,
    canvas: &mut Canvas,
    pen: &Pen,
    l: &Layout,
    rows: &[Row],
    total: usize,
    top: usize,
) {
    let lpx = l.text(SMALL_PX - 1.0);
    let lsp = SMALL_SPACING * l.scale;
    let head = l.list_head_baseline;
    // A darker well under the list and its totals, for reading over a busy scene.
    let well = Rect::new(
        l.list.x0 - l.px(6.0),
        head - l.px(20.0),
        l.scroll_track.x1 + l.px(6.0),
        l.totals.y1 + l.px(4.0),
    );
    mm::well(canvas, &l.frame, well);
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.heading_x,
        head,
        DIM,
        None,
        "AVAILABLE SHIPS",
    );
    write_centred(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.count_cx,
        head,
        DIM,
        None,
        "QUEUE",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.cost_right,
        head,
        DIM,
        None,
        "COST",
    );
    hline(canvas, l.list.x0, l.list.x1, head + l.px(8.0), LINE);
    let now = fe.time;
    let fired = fe.game.ig.mbm.fired;
    let npx = l.text(NAME_PX);
    for (i, row) in rows.iter().enumerate() {
        let r = l.row(i);
        match row {
            Row::Heading(h) => {
                mm::list_heading(canvas, pen, &l.frame, l.heading_x, r, l.cost_right, h);
            }
            Row::Ship(o) => {
                let sr = ship_row(fe, o);
                let t = o.ship_type;
                let hover = eased(fe, Key::Row(t));
                let greyed = o.status == crate::ingame::host::BuildStatus::Greyed;
                mm::row_band(canvas, &l.frame, r, sr.selected, hover, sr.refused);
                let name_col = if sr.refused > 0.5 {
                    REFUSED
                } else if greyed {
                    GREYED
                } else if sr.paused {
                    PAUSED
                } else if sr.pending > 0 && !sr.building {
                    LABEL
                } else {
                    lerp(TEXT, WHITE, hover)
                };
                let base = mid_baseline(r, npx) - if sr.building { l.px(2.0) } else { 0 };
                let w = write(
                    canvas,
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    l.name_x,
                    base,
                    name_col,
                    None,
                    &o.name.to_uppercase(),
                );
                if sr.paused {
                    let tx = (l.name_x + w + l.px(10.0)).min(l.minus_cx - l.px(70.0));
                    write(
                        canvas,
                        pen,
                        Face::Bold,
                        l.text(SMALL_PX - 2.0),
                        lsp,
                        tx,
                        base,
                        [255, 160, 60, 220],
                        None,
                        "PAUSED",
                    );
                }
                if sr.building {
                    let y = r.y1 - l.px(6.0);
                    let x1 = l.minus_cx - l.step_box / 2 - l.px(18.0);
                    let br = Rect::new(l.name_x, y, x1, y + l.px(3.0).max(2));
                    if sr.paused {
                        mm::paused_bar(canvas, br, sr.progress);
                    } else {
                        bar(canvas, l, br, sr.progress, 0.9);
                    }
                    // The batch under it, a hairline.
                    let by = br.y1 + 1;
                    let bw = (br.width() as f32 * sr.batch) as i32;
                    canvas.blend_rect(
                        Rect::new(br.x0, by, br.x0 + bw, by + 1),
                        [255, 216, 74, 150],
                    );
                }
                // - count +
                let count = sr.pending + sr.queued;
                for (right, rect) in [(false, l.minus(i)), (true, l.plus(i))] {
                    let key = if right { Key::Plus(t) } else { Key::Minus(t) };
                    let hv = eased(fe, key);
                    let lit = fired.is_some_and(|(ft, fr, at)| {
                        ft == t && fr == right && now - at < FIRED_SECONDS
                    });
                    let usable = if right { !greyed } else { count > 0 };
                    let bg = if lit {
                        [255, 200, 0, 150]
                    } else {
                        lerp([255, 255, 255, 12], [255, 200, 0, 70], hv)
                    };
                    rounded(canvas, rect, 3.0 * l.scale, bg);
                    let border = if usable {
                        lerp([255, 255, 255, 50], GOLD, hv)
                    } else {
                        [255, 255, 255, 25]
                    };
                    rounded_outline(canvas, rect, 3.0 * l.scale, 1.0, border);
                    let glyph = if lit {
                        [20, 16, 6, 255]
                    } else if usable {
                        lerp(TEXT, WHITE, hv)
                    } else {
                        GREYED
                    };
                    let (cx, cy) = ((rect.x0 + rect.x1) / 2, (rect.y0 + rect.y1) / 2);
                    let arm = (rect.width() as f32 * 0.27).round() as i32;
                    let th = (l.scale * 1.6).round().max(1.0) as i32;
                    canvas.blend_rect(
                        Rect::new(cx - arm, cy - th / 2, cx + arm + 1, cy - th / 2 + th),
                        glyph,
                    );
                    if right {
                        canvas.blend_rect(
                            Rect::new(cx - th / 2, cy - arm, cx - th / 2 + th, cy + arm + 1),
                            glyph,
                        );
                    }
                }
                let (ccol, cglow) = if greyed {
                    (GREYED, None)
                } else if count == 0 {
                    (DIM, None)
                } else if sr.pending > 0 && !sr.building {
                    (LABEL, Some((modern_hud::VALUE_GLOW, 3.0 * l.scale)))
                } else {
                    (WHITE, Some((modern_hud::VALUE_GLOW, 3.0 * l.scale)))
                };
                write_centred(
                    canvas,
                    pen,
                    Face::Bold,
                    l.text(VALUE_PX),
                    0.0,
                    l.count_cx,
                    mid_baseline(r, npx),
                    ccol,
                    cglow,
                    &count.to_string(),
                );
                let cost_col = if greyed { GREYED } else { MUTED };
                write_right(
                    canvas,
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    l.cost_right,
                    mid_baseline(r, npx),
                    cost_col,
                    None,
                    &modern_hud::thousands(o.cost),
                );
            }
        }
    }
    // The scroll bar, when the list is longer than the page.
    if let Some(th) = l.thumb(total, top) {
        mm::draw_scroll(canvas, l.scroll_track, th, eased(fe, Key::Thumb));
    }
}

fn draw_totals(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let r = l.totals;
    hline(canvas, r.x0, r.x1, r.y0, LINE);
    let lpx = l.text(SMALL_PX);
    let lsp = SMALL_SPACING * l.scale;
    let base = r.y1 - l.px(7.0);
    let glow = Some((modern_hud::VALUE_GLOW, 3.0 * l.scale));
    let mid = (r.x0 + r.x1) / 2;
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.heading_x,
        base,
        LABEL,
        None,
        "TOTAL SHIPS",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        l.text(BIG_VALUE_PX),
        0.0,
        mid - l.px(20.0),
        base + l.px(1.0),
        WHITE,
        glow,
        &consmgr::total_ships(fe).to_string(),
    );
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        mid + l.px(12.0),
        base,
        LABEL,
        None,
        "TOTAL COST",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        l.text(BIG_VALUE_PX),
        0.0,
        l.cost_right,
        base + l.px(1.0),
        WHITE,
        glow,
        &modern_hud::thousands(consmgr::total_cost(fe)),
    );
}

/// The ship view: the application's render of the ship, with corner brackets; the
/// description over it while Info is on.
fn draw_preview(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let r = l.preview;
    // A soft light behind the ship.
    mm::picture_box(canvas, &l.frame, r);
    let cm = &fe.game.ig.cm;
    if let (Some(img), Some(_)) = (&cm.ship_view_image, cm.ship_view) {
        let x = r.x0 + (r.width() - img.width) / 2;
        let y = r.y0 + (r.height() - img.height) / 2;
        canvas.set_clip(Some(r));
        canvas.blit(img, x, y, BlitMode::Blend);
        canvas.set_clip(None);
    } else if cm.ship_view.is_none() {
        write_centred(
            canvas,
            pen,
            Face::Bold,
            l.text(SMALL_PX),
            SMALL_SPACING * l.scale,
            (r.x0 + r.x1) / 2,
            (r.y0 + r.y1) / 2,
            DIM,
            None,
            "SELECT A SHIP",
        );
    }
    mm::brackets(canvas, &l.frame, r);
    // Extended info: the description over the picture.
    if cm.extended_info {
        if let Some(o) = viewed(fe) {
            let text = if o.description.trim().is_empty() {
                "No information."
            } else {
                o.description.as_str()
            };
            mm::text_pane(canvas, pen, &l.frame, r, "SHIP INFORMATION", text);
        }
    }
}

/// The ship in the view, as its list row.
fn viewed(fe: &Fe) -> Option<BuildOption> {
    let t = fe.game.ig.cm.ship_view?;
    consmgr::rows(&fe.game.ig.view)
        .into_iter()
        .find_map(|r| match r {
            Row::Ship(o) if o.ship_type == t => Some(o),
            _ => None,
        })
}

/// The class heading a ship sits under.
fn class_of(fe: &Fe, t: ShipType) -> Option<String> {
    let mut heading = None;
    for r in consmgr::rows(&fe.game.ig.view) {
        match r {
            Row::Heading(h) => heading = Some(h),
            Row::Ship(o) if o.ship_type == t => return heading,
            _ => {}
        }
    }
    None
}

/// Name, class and figures of the ship in the view; the current factory's production.
fn draw_details(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let d = l.details;
    let lpx = l.text(SMALL_PX);
    let lsp = SMALL_SPACING * l.scale;
    let x = d.x0 + l.px(2.0);
    let mut y = d.y0 + l.px(24.0);
    if let Some(o) = viewed(fe) {
        let title_px = l.text(SHIP_TITLE_PX);
        let info = l.buttons[&Button::Info];
        let m = |t: &str| pen.measure(Face::Bold, title_px, 0.0, t);
        let name = crate::ingame::modern_panel::ellipsize(
            &m,
            &o.name.to_uppercase(),
            (info.x0 - x - l.px(10.0)) as f32,
        );
        write(
            canvas,
            pen,
            Face::Bold,
            title_px,
            0.0,
            x,
            y,
            WHITE,
            Some((modern_hud::VALUE_GLOW, 4.0 * l.scale)),
            &name,
        );
        y += l.px(20.0);
        if let Some(c) = class_of(fe, o.ship_type) {
            write(
                canvas,
                pen,
                Face::Bold,
                lpx,
                lsp,
                x,
                y,
                GOLD,
                None,
                &c.to_uppercase(),
            );
        }
        y += l.px(14.0);
        // Two columns of figures.
        let sr = ship_row(fe, &o);
        let status = if sr.paused {
            "PAUSED".to_string()
        } else if sr.building {
            format!("{}%", (sr.progress * 100.0).round() as i32)
        } else if o.status == crate::ingame::host::BuildStatus::Greyed {
            "UNAVAILABLE".to_string()
        } else {
            "\u{2014}".to_string()
        };
        let cells = [
            ("COST", format!("{} RU", modern_hud::thousands(o.cost))),
            ("SUPPORT", format!("{} SU", o.support_units)),
            ("IN QUEUE", (sr.pending + sr.queued).to_string()),
            ("BUILDING", status),
        ];
        mm::cell_grid(canvas, pen, &l.frame, x, y, d.width(), &cells);
        y += 2 * l.px(26.0) + l.px(8.0);
    } else {
        write(
            canvas,
            pen,
            Face::Bold,
            lpx,
            lsp,
            x,
            y,
            DIM,
            None,
            "NO SHIP SELECTED",
        );
        y += l.px(34.0) + 2 * l.px(26.0) + l.px(8.0);
    }
    // The current factory's production line.
    let view = &fe.game.ig.view;
    let jobs: Vec<_> = match fe.game.ig.cm.factory {
        Some(f) => view
            .jobs
            .iter()
            .filter(|j| j.factory == f && j.count > 0)
            .collect(),
        None => Vec::new(),
    };
    y += l.px(12.0);
    mm::section_heading(canvas, pen, &l.frame, x, y, d.x1, "IN PRODUCTION");
    y += l.px(8.0);
    let line = l.px(22.0);
    let room = ((d.y1 - y) / line).max(0) as usize;
    if jobs.is_empty() {
        write(
            canvas,
            pen,
            Face::Semibold,
            l.text(BODY_PX),
            0.0,
            x,
            y + l.px(16.0),
            DIM,
            None,
            "Idle",
        );
        return;
    }
    let names = consmgr::rows(view);
    let name_of = |t: ShipType| {
        names.iter().find_map(|r| match r {
            Row::Ship(o) if o.ship_type == t => Some(o.name.to_uppercase()),
            _ => None,
        })
    };
    let shown = if jobs.len() > room {
        room.saturating_sub(1)
    } else {
        jobs.len()
    };
    for j in jobs.iter().take(shown) {
        let base = y + l.px(14.0);
        let name = name_of(j.ship_type).unwrap_or_default();
        let col = if j.paused { PAUSED } else { TEXT };
        write(
            canvas,
            pen,
            Face::Semibold,
            l.text(BODY_PX),
            0.0,
            x,
            base,
            col,
            None,
            &name,
        );
        write_right(
            canvas,
            pen,
            Face::Bold,
            l.text(BODY_PX),
            0.0,
            d.x1,
            base,
            WHITE,
            None,
            &format!("\u{00d7}{}", j.count),
        );
        let br = Rect::new(
            x + l.px(150.0),
            base - l.px(5.0),
            d.x1 - l.px(40.0),
            base - l.px(5.0) + l.px(3.0).max(2),
        );
        if j.paused {
            mm::paused_bar(canvas, br, j.progress);
        } else {
            bar(canvas, l, br, j.progress, 0.8);
        }
        y += line;
    }
    if shown < jobs.len() {
        let more = format!("+{} MORE", jobs.len() - shown);
        write(
            canvas,
            pen,
            Face::Bold,
            l.text(SMALL_PX - 1.0),
            lsp,
            x,
            y + l.px(14.0),
            DIM,
            None,
            &more,
        );
    }
}

fn draw_button(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout, b: Button) {
    let r = l.buttons[&b];
    let pressed = fe.game.ig.mbm.pressed == Some(Hit::Button(b))
        && hit_now(fe, l, fe.game.ig.mbm.mouse.0, fe.game.ig.mbm.mouse.1).0 == Hit::Button(b);
    // Pause and Cancel act on the selected rows; Build on the picked counts.
    let idle = match b {
        Button::Build => fe.game.ig.cm.pending.values().all(|&n| n <= 0),
        Button::Pause | Button::Cancel => fe.game.ig.cm.selected.is_empty(),
        _ => false,
    };
    let look = ButtonLook {
        label: b.label(),
        key: b.key(),
        hover: eased(fe, Key::Button(b)),
        pressed,
        primary: b == Button::Build,
        toggled: b == Button::Info && fe.game.ig.cm.extended_info,
        idle,
        focused: focused(fe, b.atom()),
    };
    mm::draw_button(canvas, pen, &l.frame, r, &look);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_is_centred_and_scales_with_the_height() {
        let l = layout(1920, 1080);
        assert_eq!(l.scale, 1.0);
        assert_eq!(l.panel, Rect::new(480, 160, 1440, 920));
        // 768 lines: never under the minimum scale, centred.
        let m = layout(1024, 768);
        assert!((m.scale - MIN_SCALE).abs() < 1e-6);
        assert_eq!(m.panel.width(), 768);
        assert_eq!(m.panel.x0, (1024 - 768) / 2);
        assert_eq!(m.panel.y0, (768 - m.panel.height()) / 2);
        // 640x480: fits the window with its margin, below the HUD's header band.
        let s = layout(640, 480);
        assert!(s.panel.x0 >= 8 && s.panel.x1 <= 632, "{:?}", s.panel);
        assert!(
            s.panel.y0 >= top_band(480) && s.panel.y1 <= 472,
            "{:?}",
            s.panel
        );
        for (w, h) in [(1920, 1080), (1024, 768), (640, 480), (800, 600)] {
            assert!(layout(w, h).panel.y0 >= top_band(h), "{w}x{h}");
        }
        // 4K doubles.
        let k = layout(3840, 2160);
        assert_eq!(k.scale, 2.0);
        assert_eq!(k.panel.width(), 1920);
        // Wide screens keep the height's scale.
        assert_eq!(layout(2560, 1080).scale, 1.0);
    }

    #[test]
    fn at_1080_lines_the_side_panels_stay_clear() {
        // The modern HUD's columns: 16..416 on the left (the log is the widest), the right
        // ones from 1644.
        let l = layout(1920, 1080);
        assert!(l.panel.x0 > 416 && l.panel.x1 < 1644);
    }

    #[test]
    fn everything_sits_inside_the_panel_without_overlaps() {
        for (w, h) in [
            (1920, 1080),
            (1024, 768),
            (640, 480),
            (800, 600),
            (1280, 720),
            (3840, 2160),
        ] {
            let l = layout(w, h);
            let inside = |r: Rect| {
                r.x0 >= l.panel.x0 && r.x1 <= l.panel.x1 && r.y0 >= l.panel.y0 && r.y1 <= l.panel.y1
            };
            let mut all: Vec<(String, Rect)> = l
                .buttons
                .iter()
                .map(|(b, r)| (format!("{b:?}"), *r))
                .collect();
            for (i, t) in l.tabs.iter().enumerate() {
                all.push((format!("tab {i}"), *t));
            }
            all.push(("ru".into(), l.ru));
            all.push(("su".into(), l.su));
            all.push(("list".into(), l.list));
            all.push(("totals".into(), l.totals));
            all.push(("preview".into(), l.preview));
            for (n, r) in &all {
                assert!(inside(*r), "{n} {r:?} outside {:?} at {w}x{h}", l.panel);
                assert!(r.width() > 0 && r.height() > 0, "{n} empty at {w}x{h}");
            }
            for i in 0..all.len() {
                for j in i + 1..all.len() {
                    assert!(
                        !modern_hud::overlaps(all[i].1, all[j].1),
                        "{} / {} at {w}x{h}",
                        all[i].0,
                        all[j].0
                    );
                }
            }
            // The list's rows end above the totals; the preview's details above the footer.
            assert!(l.row(l.page - 1).y1 <= l.totals.y0, "{w}x{h}");
            assert!(l.details.y1 <= l.footer_line);
            // - count + cost read left to right inside the list, clear of the scroll bar.
            assert!(l.name_x < l.minus(0).x0);
            assert!(l.minus(0).x1 < l.count_cx && l.count_cx < l.plus(0).x0);
            assert!(l.plus(0).x1 < l.cost_right && l.cost_right <= l.list.x1);
            assert!(l.list.x1 <= l.scroll_track.x0);
            // The - / + boxes are big enough to hit.
            assert!(l.step_box >= 9, "{w}x{h}");
        }
    }

    #[test]
    fn hit_testing() {
        let l = layout(1920, 1080);
        let centre = |r: Rect| ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
        assert_eq!(hit(&l, 5, 5, 20, 20, 0), Hit::Outside);
        for b in Button::ALL {
            let (x, y) = centre(l.buttons[&b]);
            assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Button(b), "{b:?}");
        }
        for i in 0..3 {
            let (x, y) = centre(l.tabs[i]);
            assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Tab(i));
        }
        let (x, y) = centre(l.row(3));
        assert_eq!(hit(&l, l.name_x + 5, y, 20, 20, 0), Hit::Row(3));
        let _ = x;
        let (x, y) = centre(l.minus(4));
        assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Minus(4));
        let (x, y) = centre(l.plus(5));
        assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Plus(5));
        // A little slack around the boxes, but the count between them is the row.
        let p = l.plus(5);
        assert_eq!(hit(&l, p.x1 + 1, y, 20, 20, 0), Hit::Plus(5));
        assert_eq!(hit(&l, l.count_cx, y, 20, 20, 0), Hit::Row(5));
        // Rows past the end of the list are just the panel.
        let (_, y) = centre(l.row(12));
        assert_eq!(hit(&l, l.name_x + 5, y, 10, 10, 0), Hit::Panel);
        // The scroll bar only when the list is longer than the page.
        let tx = (l.scroll_track.x0 + l.scroll_track.x1) / 2;
        let ty = l.scroll_track.y1 - 2;
        assert_eq!(hit(&l, tx, ty, 17, 17, 0), Hit::Panel);
        assert_eq!(hit(&l, tx, ty, 17, 30, 0), Hit::Track(false));
        let th = l.thumb(30, 0).unwrap();
        assert_eq!(hit(&l, tx, th.y0 + 1, 17, 30, 0), Hit::Thumb);
        let th = l.thumb(30, 13).unwrap();
        assert_eq!(
            th.y1, l.scroll_track.y1,
            "the last page puts the thumb at the bottom"
        );
        assert_eq!(
            hit(&l, tx, l.scroll_track.y0 + 1, 17, 30, 13),
            Hit::Track(true)
        );
        // The header and the gaps hit the panel.
        assert_eq!(
            hit(&l, l.panel.x0 + 10, l.panel.y0 + 5, 20, 20, 0),
            Hit::Panel
        );
    }

    #[test]
    fn hit_testing_at_every_size() {
        for (w, h) in [(1024, 768), (640, 480)] {
            let l = layout(w, h);
            for b in Button::ALL {
                let r = l.buttons[&b];
                assert_eq!(hit(&l, r.x0 + 1, r.y0 + 1, 20, 20, 0), Hit::Button(b));
                assert_eq!(hit(&l, r.x1 - 1, r.y1 - 1, 20, 20, 0), Hit::Button(b));
            }
            for i in 0..l.page {
                let m = l.minus(i);
                let p = l.plus(i);
                assert_eq!(
                    hit(&l, (m.x0 + m.x1) / 2, (m.y0 + m.y1) / 2, l.page, 30, 0),
                    Hit::Minus(i)
                );
                assert_eq!(
                    hit(&l, (p.x0 + p.x1) / 2, (p.y0 + p.y1) / 2, l.page, 30, 0),
                    Hit::Plus(i)
                );
                let r = l.row(i);
                assert_eq!(hit(&l, l.name_x, r.y0, l.page, 30, 0), Hit::Row(i));
                assert_eq!(hit(&l, l.name_x, r.y1 - 1, l.page, 30, 0), Hit::Row(i));
            }
        }
    }

    #[test]
    fn thumb_drag_round_trips() {
        let l = layout(1920, 1080);
        for top in 0..=13 {
            let th = l.thumb(30, top).unwrap();
            assert_eq!(l.top_for_thumb(30, th.y0), top);
        }
        assert_eq!(l.top_for_thumb(30, -500), 0);
        assert_eq!(l.top_for_thumb(30, 5000), 13);
        assert!(l.thumb(17, 0).is_none());
    }

    #[test]
    fn mode_switch() {
        assert_eq!(parse_mode("Modern"), Some(true));
        assert_eq!(parse_mode("original"), Some(false));
        assert_eq!(parse_mode("x"), None);
        let o = Options::default();
        assert!(o.get_bool(OPTION_KEY), "on by default");
        assert!(enabled(&o));
        let (off, errors) = Options::from_text("hwcModernBuildManager = 0\n");
        assert!(errors.is_empty());
        assert!(!enabled(&off));
        // The original HUD brings the original screen back.
        let (old_hud, _) = Options::from_text("hwcModernHud = 0\n");
        assert!(!enabled(&old_hud));
    }

    #[test]
    fn button_atoms_are_the_screens() {
        let names: Vec<_> = Button::ALL.iter().map(|b| b.atom()).collect();
        assert_eq!(
            names,
            [
                "CM_BuildShips",
                "CM_PauseJobs",
                "CM_CancelJobs",
                "CM_Close",
                "CM_Research",
                "CM_Launch",
                "CM_ExtendedInfo"
            ]
        );
    }
}
