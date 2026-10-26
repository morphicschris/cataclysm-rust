//! A preview of the build manager (`construction_manager.fib`, [`consmgr`]) restyled in the
//! modern HUD's look: a user-requested change, **off by default** (`--build-ui modern` or
//! `hwcModernBuildManager = 1` turn it on).
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
use std::sync::atomic::{AtomicU8, Ordering};

use crate::canvas::{BlitMode, Canvas, Color, Rect, WHITE};
use crate::consts::*;
use crate::ingame::consmgr::{self, Row};
use crate::ingame::host::{BuildOption, ShipType};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_objectives as obj;
use crate::ingame::{Fe, Manager};
use crate::options::Options;

/// The options-file key: 0 (default) the original build manager, 1 this one.
pub const OPTION_KEY: &str = "hwcModernBuildManager";

/// `--build-ui`: 0 the options file decides, 1 modern, 2 original.
static MODE_OVERRIDE: AtomicU8 = AtomicU8::new(0);

/// Sets the command line's (or the build's) choice; `None`: the options file decides.
pub fn set_override(modern: Option<bool>) {
    let v = match modern {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    MODE_OVERRIDE.store(v, Ordering::Relaxed);
}

/// Whether the modern build manager replaces the original's look.
pub fn enabled(options: &Options) -> bool {
    match MODE_OVERRIDE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => options.get_bool(OPTION_KEY),
    }
}

/// Parses `--build-ui`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "modern" => Some(true),
        "original" => Some(false),
        _ => None,
    }
}

/// Whether the modern build manager is up now (enabled, and the build manager open).
pub fn active(fe: &Fe) -> bool {
    fe.game.ig.manager == Some(Manager::Construction) && enabled(&fe.game.options)
}

// ---- colours ----

const PANEL: Color = [8, 10, 14, 191];
const SECTION: Color = [255, 255, 255, 9];
const LINE: Color = [255, 255, 255, 24];
const GOLD: Color = obj::GOLD;
const LABEL: Color = modern_hud::LABEL;
const TEXT: Color = [236, 239, 243, 255];
const MUTED: Color = [255, 255, 255, 150];
const DIM: Color = [255, 255, 255, 95];
const GREYED: Color = [255, 255, 255, 70];
const PAUSED: Color = [170, 182, 196, 255];
const REFUSED: Color = [0xFF, 0x4A, 0x1E, 0xFF];
/// Over the 3D view around the panel.
const SCENE_DIM: Color = [0, 0, 0, 92];
const SHADOW: Color = [0, 0, 0, 255];

/// Hover easing, per second.
const HOVER_RATE: f32 = 14.0;
/// How long a fired - / + stays lit.
const FIRED_SECONDS: f64 = 0.18;
/// The second press of a double click: within this time and distance of the first
/// (as [`crate::fe::FrontEnd::mouse_button`]).
const DOUBLE_SECONDS: f64 = 0.5;
const DOUBLE_PIXELS: i32 = 4;

// ---- layout ----

/// The panel at 1080 lines.
pub const REF_W: f32 = 960.0;
pub const REF_H: f32 = 760.0;
const REF_LINES: f32 = 1080.0;
/// Never smaller than this scale unless the window is (then it fits the window).
const MIN_SCALE: f32 = 0.8;
/// Space kept around the panel when it has to fit the window.
const FIT_MARGIN: f32 = 8.0;

const PAD: f32 = 24.0;
const HEADER_H: f32 = 56.0;
const TITLE_BASELINE: f32 = 35.0;
const TITLE_PX: f32 = 16.0;
const TITLE_SPACING: f32 = 3.0;
const TOP_BUTTON_W: f32 = 132.0;
const TOP_BUTTON_H: f32 = 30.0;
const TOP_BUTTON_Y: f32 = 13.0;
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
const FOOTER_LINE: f32 = 680.0;
const BUTTON_Y: f32 = 698.0;
const BUTTON_H: f32 = 38.0;
const BUTTON_W: f32 = 168.0;
const BUTTON_GAP: f32 = 12.0;
const STEP_BOX: f32 = 20.0;

/// Text sizes at 1080 lines, and the smallest a label may get.
const NAME_PX: f32 = 15.0;
const VALUE_PX: f32 = 15.0;
const BIG_VALUE_PX: f32 = 18.0;
const SMALL_PX: f32 = 11.0;
const SMALL_SPACING: f32 = 2.0;
const BUTTON_PX: f32 = 12.0;
const BUTTON_SPACING: f32 = 1.5;
const SHIP_TITLE_PX: f32 = 21.0;
const BODY_PX: f32 = 13.0;
const MIN_TEXT: f32 = 8.5;

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

/// The panel's scale for a `width` x `height` canvas: the height over 1080, at least
/// [`MIN_SCALE`], and never larger than fits the window.
pub fn scale_for(width: i32, height: i32) -> f32 {
    let (w, h) = (width.max(1) as f32, height.max(1) as f32);
    let band = top_band(height) as f32;
    let fit = ((h - band - FIT_MARGIN) / REF_H).min((w - 2.0 * FIT_MARGIN) / REF_W);
    (h / REF_LINES).max(MIN_SCALE).min(fit).max(0.2)
}

/// The band along the top kept for the modern HUD's panel headers (objectives, resources),
/// which stay up over the manager: its corner margin and header height, and a little air.
pub fn top_band(height: i32) -> i32 {
    let s = modern_hud::scale_for(height);
    ((obj_margin() + obj::HEADER_H) * s).round() as i32 + 4
}

fn obj_margin() -> f32 {
    crate::ingame::modern_panel::MARGIN
}

/// The layout for a `width` x `height` canvas.
pub fn layout(width: i32, height: i32) -> Layout {
    let s = scale_for(width, height);
    let px = |v: f32| (v * s).round() as i32;
    let (pw, ph) = (px(REF_W), px(REF_H));
    let x0 = (width - pw) / 2;
    let y0 = ((height - ph) / 2).max(top_band(height));
    let panel = Rect::new(x0, y0, x0 + pw, y0 + ph);
    let at = |x: f32, y: f32, w: f32, h: f32| {
        Rect::new(x0 + px(x), y0 + px(y), x0 + px(x + w), y0 + px(y + h))
    };
    let right_edge = REF_W - PAD;
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
    let preview = at(RIGHT_X, BODY_Y, right_edge - RIGHT_X, PREVIEW_H);
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
        let x = PAD + i as f32 * (BUTTON_W + BUTTON_GAP);
        buttons.insert(b, at(x, BUTTON_Y, BUTTON_W, BUTTON_H));
    }
    buttons.insert(
        Button::Close,
        at(right_edge - BUTTON_W, BUTTON_Y, BUTTON_W, BUTTON_H),
    );
    buttons.insert(
        Button::Systems,
        at(
            right_edge - TOP_BUTTON_W,
            TOP_BUTTON_Y,
            TOP_BUTTON_W,
            TOP_BUTTON_H,
        ),
    );
    buttons.insert(
        Button::Research,
        at(
            right_edge - 2.0 * TOP_BUTTON_W - 10.0,
            TOP_BUTTON_Y,
            TOP_BUTTON_W,
            TOP_BUTTON_H,
        ),
    );
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
        panel,
        header: Rect::new(panel.x0, panel.y0, panel.x1, y0 + px(HEADER_H)),
        title_x: x0 + px(PAD),
        title_baseline: y0 + px(TITLE_BASELINE),
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
        footer_line: y0 + px(FOOTER_LINE),
        buttons,
    }
}

impl Layout {
    fn px(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }

    /// A text size: `v` at 1080 lines, never under [`MIN_TEXT`].
    fn text(&self, v: f32) -> f32 {
        (v * self.scale).max(MIN_TEXT.min(v))
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
        if total <= self.page {
            return None;
        }
        let t = self.scroll_track;
        let h = t.height() as f32;
        let len = (h * self.page as f32 / total as f32).max(self.px(24.0) as f32);
        let max_top = (total - self.page) as f32;
        let y = t.y0 as f32 + (h - len) * (top as f32 / max_top).clamp(0.0, 1.0);
        Some(Rect::new(
            t.x0 - 2,
            y.round() as i32,
            t.x1 + 2,
            (y + len).round() as i32,
        ))
    }

    /// The first row for the thumb's top at `y` (dragging).
    pub fn top_for_thumb(&self, total: usize, y: i32) -> usize {
        let Some(th) = self.thumb(total, 0) else {
            return 0;
        };
        let free = (self.scroll_track.height() - th.height()).max(1) as f32;
        let f = ((y - self.scroll_track.y0) as f32 / free).clamp(0.0, 1.0);
        (f * (total - self.page) as f32).round() as usize
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
    /// The first press of a possible double click, and whether the second is held.
    last_press: Option<(f64, i32, i32)>,
    double_held: bool,
    /// A held - / +: (right, next repeat time).
    hold: Option<(bool, f64)>,
    /// Dragging the thumb: the pointer's offset from its top.
    drag: Option<i32>,
    /// Eased highlights, 0..1, and the time they were last stepped.
    hover: BTreeMap<Key, f32>,
    last_time: Option<f64>,
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
        fe.game.ig.mbm.double_held = false;
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
    if st.double_held || st.pressed.is_some() {
        return;
    }
    let double = st.last_press.is_some_and(|(t, px, py)| {
        now - t < DOUBLE_SECONDS
            && (px - x).abs() <= DOUBLE_PIXELS
            && (py - y).abs() <= DOUBLE_PIXELS
    });
    if double {
        st.double_held = true;
        st.last_press = None;
        if let Hit::Row(i) = at {
            if let Some(o) = ship_of(&rows, i) {
                consmgr::list_event(fe, &o, RPE_DOUBLE_LEFT);
                note_fired(fe);
            }
        }
        return;
    }
    st.last_press = Some((now, x, y));
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

// ---- drawing helpers ----

fn lerp(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    let m = |i: usize| (a[i] as f32 + (b[i] as f32 - a[i] as f32) * t).round() as u8;
    [m(0), m(1), m(2), m(3)]
}

fn alpha(c: Color, a: f32) -> Color {
    modern_hud::with_alpha(c, c[3] as f32 / 255.0 * a)
}

/// Coverage of a rounded rectangle at a pixel centre.
fn rounded_cov(r: Rect, radius: f32, px: f32, py: f32) -> f32 {
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
fn rounded(canvas: &mut Canvas, r: Rect, radius: f32, c: Color) {
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
fn rounded_outline(canvas: &mut Canvas, r: Rect, radius: f32, w: f32, c: Color) {
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
fn shadow(canvas: &mut Canvas, r: Rect, spread: i32) {
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

fn hline(canvas: &mut Canvas, x0: i32, x1: i32, y: i32, c: Color) {
    canvas.blend_rect(Rect::new(x0, y, x1, y + 1), c);
}

/// Dims everything but `keep`.
fn dim_around(canvas: &mut Canvas, keep: Rect) {
    let (w, h) = (canvas.width, canvas.height);
    canvas.blend_rect(Rect::new(0, 0, w, keep.y0), SCENE_DIM);
    canvas.blend_rect(Rect::new(0, keep.y1, w, h), SCENE_DIM);
    canvas.blend_rect(Rect::new(0, keep.y0, keep.x0, keep.y1), SCENE_DIM);
    canvas.blend_rect(Rect::new(keep.x1, keep.y0, w, keep.y1), SCENE_DIM);
}

/// Writes with the pen: left-aligned at `x` (Bold).
#[allow(clippy::too_many_arguments)]
fn write(
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

#[allow(clippy::too_many_arguments)]
fn write_right(
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

#[allow(clippy::too_many_arguments)]
fn write_centred(
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
fn mid_baseline(r: Rect, px: f32) -> i32 {
    ((r.y0 + r.y1) as f32 / 2.0 + px * 0.36).round() as i32
}

/// A thin glowing bar (track, glow, gradient fill).
fn bar(canvas: &mut Canvas, l: &Layout, r: Rect, fraction: f32, glow: f32) {
    modern_hud::draw_bar(canvas, r, fraction, 3.0 * l.scale, glow);
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
    let st = &mut fe.game.ig.mbm;
    let dt = st.last_time.map_or(1.0, |t| (now - t).max(0.0) as f32);
    st.last_time = Some(now);
    let k = 1.0 - (-dt * HOVER_RATE).exp();
    let mut keys: Vec<Key> = st.hover.keys().copied().collect();
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
        let cur = st.hover.get(&key).copied().unwrap_or(0.0);
        let v = cur + (target - cur) * k;
        if v < 0.01 && target == 0.0 {
            st.hover.remove(&key);
        } else {
            st.hover.insert(key, v);
        }
    }
}

fn eased(fe: &Fe, key: Key) -> f32 {
    fe.game.ig.mbm.hover.get(&key).copied().unwrap_or(0.0)
}

/// Whether the screen's keyboard focus (Tab / arrows) is on a button's atom.
fn focused(fe: &Fe, b: Button) -> bool {
    fe.find_atom(b.atom())
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
    dim_around(canvas, l.panel);
    shadow(canvas, l.panel, l.px(18.0).max(6));
    canvas.blend_rect(l.panel, PANEL);
    let accent = Rect::new(
        l.panel.x0,
        l.panel.y0,
        l.panel.x0 + l.px(obj::ACCENT_W).max(1),
        l.panel.y1,
    );
    canvas.fill_rect(accent, obj::ACCENT);
    // A faint gold wash along the top.
    for (i, y) in (l.panel.y0..l.header.y1).enumerate() {
        let t = 1.0 - i as f32 / l.header.height().max(1) as f32;
        hline(
            canvas,
            accent.x1,
            l.panel.x1,
            y,
            [255, 200, 0, (14.0 * t) as u8],
        );
    }
    let inner_x0 = l.title_x;
    let inner_x1 = l.panel.x1 - l.px(PAD);
    hline(canvas, inner_x0, inner_x1, l.header.y1, LINE);
    hline(canvas, inner_x0, inner_x1, l.footer_line, LINE);

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
    let x = write(
        canvas,
        pen,
        Face::Bold,
        l.text(TITLE_PX),
        TITLE_SPACING * l.scale,
        l.title_x,
        l.title_baseline,
        GOLD,
        Some((modern_hud::VALUE_GLOW, 3.0 * l.scale)),
        "BUILD MANAGER",
    );
    // The current factory after a thin divider.
    let view = &fe.game.ig.view;
    let name = fe
        .game
        .ig
        .cm
        .factory
        .and_then(|s| view.factories.iter().find(|f| f.ship == s))
        .map(|f| f.name.to_uppercase());
    if let Some(name) = name {
        let dx = l.title_x + x + l.px(16.0);
        let h = l.px(14.0);
        canvas.blend_rect(
            Rect::new(
                dx,
                l.title_baseline - h,
                dx + 1,
                l.title_baseline + l.px(2.0),
            ),
            LINE,
        );
        write(
            canvas,
            pen,
            Face::Bold,
            l.text(SMALL_PX + 1.0),
            SMALL_SPACING * l.scale,
            dx + l.px(16.0),
            l.title_baseline - l.px(1.0),
            MUTED,
            None,
            &name,
        );
    }
}

/// The factory tabs: mothership and two carriers, each with its queue count and the ship
/// it is building; an empty slot is shown disabled.
fn draw_tabs(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let view = &fe.game.ig.view;
    for (slot, &r) in l.tabs.iter().enumerate() {
        let f = consmgr::factory_in_slot(view, slot);
        let current = f.is_some_and(|f| Some(f.ship) == fe.game.ig.cm.factory);
        let hover = if f.is_some() {
            eased(fe, Key::Tab(slot))
        } else {
            0.0
        };
        let bg = if current {
            [255, 200, 0, 30]
        } else {
            lerp([255, 255, 255, 6], [255, 255, 255, 20], hover)
        };
        rounded(canvas, r, 3.0 * l.scale, bg);
        let jobs: Vec<_> = f
            .map(|f| view.jobs.iter().filter(|j| j.factory == f.ship).collect())
            .unwrap_or_default();
        // The ship in production at that factory, as a share of the tab's bottom edge.
        let progress = jobs
            .iter()
            .find(|j| j.count > 0 && !j.paused)
            .map(|j| j.progress.clamp(0.0, 1.0));
        let u = Rect::new(r.x0, r.y1 - l.px(2.0).max(2), r.x1, r.y1);
        let done = |p: f32| Rect::new(u.x0, u.y0, u.x0 + (u.width() as f32 * p) as i32, u.y1);
        if current {
            canvas.blend_rect(u, alpha(GOLD, 0.55));
            canvas.blend_rect(done(progress.unwrap_or(1.0)), GOLD);
            // Soft glow over the underline.
            for i in 1..=l.px(6.0).max(2) {
                let a = 0.22 * (1.0 - i as f32 / (l.px(6.0).max(2) + 1) as f32);
                hline(canvas, r.x0 + 2, r.x1 - 2, u.y0 - i, alpha(GOLD, a));
            }
        }
        let (label, col) = match f {
            Some(f) => (
                f.name.to_uppercase(),
                if current {
                    LABEL
                } else {
                    lerp(MUTED, WHITE, hover)
                },
            ),
            None if slot == 0 => ("NO COMMAND SHIP".to_string(), GREYED),
            None => (format!("CARRIER {slot} \u{2014} NONE"), GREYED),
        };
        let px = l.text(BUTTON_PX);
        let base = mid_baseline(r, px) - l.px(1.0);
        let count: i32 = jobs.iter().map(|j| j.count).sum();
        let count_s = (count > 0).then(|| count.to_string());
        let right = r.x1 - l.px(12.0);
        let mut avail = (right - r.x0 - l.px(14.0)) as f32;
        if let Some(c) = &count_s {
            avail -= pen.advance(Face::Bold, l.text(VALUE_PX), 0.0, c) as f32 + l.px(10.0) as f32;
        }
        let m = |t: &str| pen.measure(Face::Bold, px, BUTTON_SPACING * l.scale, t);
        let label = crate::ingame::modern_panel::ellipsize(&m, &label, avail);
        write(
            canvas,
            pen,
            Face::Bold,
            px,
            BUTTON_SPACING * l.scale,
            r.x0 + l.px(14.0),
            base,
            col,
            None,
            &label,
        );
        if let Some(c) = count_s {
            write_right(
                canvas,
                pen,
                Face::Bold,
                l.text(VALUE_PX),
                0.0,
                right,
                base + l.px(1.0),
                WHITE,
                Some((modern_hud::VALUE_GLOW, 3.0 * l.scale)),
                &c,
            );
        }
        if !current {
            if let Some(p) = progress {
                canvas.blend_rect(u, modern_hud::TRACK);
                canvas.blend_rect(done(p), alpha(GOLD, 0.8));
            }
        }
    }
}

/// Current RUs and the support units (used, plus what is queued and picked).
fn draw_stats(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let p = &fe.game.ig.view.player;
    let lpx = l.text(SMALL_PX);
    let lsp = SMALL_SPACING * l.scale;
    let vpx = l.text(BIG_VALUE_PX);
    let glow = Some((modern_hud::VALUE_GLOW, 3.0 * l.scale));
    let bar_h = l.px(4.0).max(2);
    for r in [l.ru, l.su] {
        rounded(canvas, r, 3.0 * l.scale, SECTION);
    }
    let base = |r: Rect| r.y0 + l.px(19.0);
    let bar_r = |r: Rect| {
        let y = r.y1 - l.px(10.0);
        Rect::new(r.x0 + l.px(12.0), y, r.x1 - l.px(12.0), y + bar_h)
    };
    // RUs.
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.ru.x0 + l.px(12.0),
        base(l.ru),
        LABEL,
        None,
        "CURRENT RUS",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        vpx,
        0.0,
        l.ru.x1 - l.px(12.0),
        base(l.ru) + l.px(1.0),
        WHITE,
        glow,
        &modern_hud::thousands(p.resource_units),
    );
    bar(
        canvas,
        l,
        bar_r(l.ru),
        modern_hud::ru_fill(p.resource_units),
        1.0,
    );
    // Support units: used (gold), then what the queues and picked counts need (paler).
    let pending = p.support_pending + consmgr::gui_support_units(fe);
    let cap = p.support_capacity;
    let at_cap = p.support_used + pending >= cap && cap > 0 && pending + p.support_used > 0;
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.su.x0 + l.px(12.0),
        base(l.su),
        LABEL,
        None,
        "SUPPORT UNITS",
    );
    let value = format!("{} / {}", p.support_used + pending, cap);
    let (vcol, vglow) = if at_cap {
        (
            modern_hud::AT_CAP,
            Some(([0xFF, 0x4A, 0x1E, 140], 3.0 * l.scale)),
        )
    } else {
        (WHITE, glow)
    };
    write_right(
        canvas,
        pen,
        Face::Bold,
        vpx,
        0.0,
        l.su.x1 - l.px(12.0),
        base(l.su) + l.px(1.0),
        vcol,
        vglow,
        &value,
    );
    let br = bar_r(l.su);
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
    rounded(canvas, well, 4.0 * l.scale, [0, 0, 0, 56]);
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
                let base = r.y1 - l.px(6.0);
                let w = write(
                    canvas,
                    pen,
                    Face::Bold,
                    l.text(SMALL_PX),
                    lsp,
                    l.heading_x,
                    base,
                    GOLD,
                    None,
                    &h.to_uppercase(),
                );
                let x0 = l.heading_x + w + l.px(10.0);
                if x0 < l.cost_right {
                    hline(
                        canvas,
                        x0,
                        l.cost_right,
                        base - l.px(4.0),
                        alpha(GOLD, 0.25),
                    );
                }
            }
            Row::Ship(o) => {
                let sr = ship_row(fe, o);
                let t = o.ship_type;
                let hover = eased(fe, Key::Row(t));
                let greyed = o.status == crate::ingame::host::BuildStatus::Greyed;
                let band = Rect::new(r.x0, r.y0 + 1, r.x1, r.y1 - 1);
                if sr.selected {
                    rounded(
                        canvas,
                        band,
                        2.0 * l.scale,
                        lerp([255, 200, 0, 26], [255, 200, 0, 40], hover),
                    );
                    canvas.fill_rect(
                        Rect::new(
                            band.x0,
                            band.y0 + 2,
                            band.x0 + l.px(2.0).max(2),
                            band.y1 - 2,
                        ),
                        GOLD,
                    );
                } else if hover > 0.0 {
                    rounded(
                        canvas,
                        band,
                        2.0 * l.scale,
                        alpha([255, 255, 255, 18], hover / 0.6),
                    );
                }
                if sr.refused > 0.0 {
                    rounded(
                        canvas,
                        band,
                        2.0 * l.scale,
                        alpha([255, 74, 30, 90], sr.refused),
                    );
                }
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
                        canvas.blend_rect(br, modern_hud::TRACK);
                        let w = (br.width() as f32 * sr.progress) as i32;
                        canvas.blend_rect(
                            Rect::new(br.x0, br.y0, br.x0 + w, br.y1),
                            [170, 182, 196, 160],
                        );
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
        let tr = l.scroll_track;
        rounded(canvas, tr, 1.5, [255, 255, 255, 16]);
        let hv = eased(fe, Key::Thumb);
        let inner = Rect::new(tr.x0, th.y0, tr.x1, th.y1);
        rounded(canvas, inner, 1.5, lerp([255, 200, 0, 140], GOLD, hv));
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
    rounded(canvas, r, 4.0 * l.scale, [0, 0, 0, 70]);
    // A soft light behind the ship.
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
    // Corner brackets.
    let len = l.px(14.0);
    let c = alpha(GOLD, 0.8);
    let t = 1.max(l.px(1.5));
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
    // Extended info: the description over the picture.
    if cm.extended_info {
        if let Some(o) = viewed(fe) {
            let inner = Rect::new(
                r.x0 + l.px(6.0),
                r.y0 + l.px(6.0),
                r.x1 - l.px(6.0),
                r.y1 - l.px(6.0),
            );
            rounded(canvas, inner, 3.0 * l.scale, [6, 8, 12, 200]);
            let bpx = l.text(BODY_PX);
            let m = |t: &str| pen.measure(Face::Semibold, bpx, 0.0, t);
            let width = (inner.width() - l.px(28.0)) as f32;
            let mut y = inner.y0 + l.px(26.0);
            write(
                canvas,
                pen,
                Face::Bold,
                l.text(SMALL_PX),
                SMALL_SPACING * l.scale,
                inner.x0 + l.px(14.0),
                y,
                LABEL,
                None,
                "SHIP INFORMATION",
            );
            y += l.px(24.0);
            let text = if o.description.trim().is_empty() {
                "No information."
            } else {
                o.description.as_str()
            };
            let paras: Vec<Vec<String>> = text
                .split(['\n', '\r'])
                .filter(|p| !p.trim().is_empty())
                .map(|p| obj::wrap(p.trim(), width, width, &m))
                .collect();
            let lines: usize = paras.iter().map(Vec::len).sum();
            let room = inner.y1 - l.px(10.0) - y;
            // Roomy when it fits, tighter (no gaps between paragraphs) when not.
            let roomy = (bpx * 1.45).round() as i32;
            let (line_h, gap) =
                if lines as i32 * roomy + paras.len() as i32 * roomy / 3 <= room + roomy {
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
                        let cut = crate::ingame::modern_panel::ellipsize(&m, &cut, width);
                        write(
                            canvas,
                            pen,
                            Face::Semibold,
                            bpx,
                            0.0,
                            inner.x0 + l.px(14.0),
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
                        inner.x0 + l.px(14.0),
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
    let glow = Some((modern_hud::VALUE_GLOW, 3.0 * l.scale));
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
        let colw = (d.width() - l.px(16.0)) / 2;
        for (k, (label, value)) in cells.iter().enumerate() {
            let cx0 = x + (k as i32 % 2) * (colw + l.px(16.0));
            let cy = y + (k as i32 / 2) * l.px(26.0);
            let cell = Rect::new(cx0 - l.px(2.0), cy, cx0 + colw - l.px(2.0), cy + l.px(22.0));
            rounded(canvas, cell, 2.0 * l.scale, SECTION);
            let b = mid_baseline(cell, lpx);
            write(
                canvas,
                pen,
                Face::Bold,
                l.text(SMALL_PX - 1.0),
                lsp,
                cell.x0 + l.px(8.0),
                b,
                LABEL,
                None,
                label,
            );
            write_right(
                canvas,
                pen,
                Face::Bold,
                l.text(VALUE_PX - 1.0),
                0.0,
                cell.x1 - l.px(8.0),
                b + l.px(1.0),
                WHITE,
                glow,
                value,
            );
        }
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
    write(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        x,
        y,
        LABEL,
        None,
        "IN PRODUCTION",
    );
    hline(
        canvas,
        x + pen.advance(Face::Bold, lpx, lsp, "IN PRODUCTION") + l.px(10.0),
        d.x1,
        y - l.px(4.0),
        alpha(GOLD, 0.25),
    );
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
            canvas.blend_rect(br, modern_hud::TRACK);
            let w = (br.width() as f32 * j.progress.clamp(0.0, 1.0)) as i32;
            canvas.blend_rect(
                Rect::new(br.x0, br.y0, br.x0 + w, br.y1),
                [170, 182, 196, 160],
            );
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
    let hv = eased(fe, Key::Button(b));
    let pressed = fe.game.ig.mbm.pressed == Some(Hit::Button(b))
        && hit_now(fe, l, fe.game.ig.mbm.mouse.0, fe.game.ig.mbm.mouse.1).0 == Hit::Button(b);
    let primary = b == Button::Build;
    let toggled = b == Button::Info && fe.game.ig.cm.extended_info;
    // Pause and Cancel act on the selected rows; Build on the picked counts.
    let idle = match b {
        Button::Build => fe.game.ig.cm.pending.values().all(|&n| n <= 0),
        Button::Pause | Button::Cancel => fe.game.ig.cm.selected.is_empty(),
        _ => false,
    };
    let radius = 3.0 * l.scale;
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
    if focused(fe, b) {
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
        r.x0 + l.px(6.0),
        r.x1 - l.px(6.0),
        r.y0 + 1,
        [255, 255, 255, (12.0 + 30.0 * hv) as u8],
    );
    let px = l.text(BUTTON_PX);
    let sp = BUTTON_SPACING * l.scale;
    let col = if pressed {
        [20, 16, 6, 255]
    } else if primary && !idle || toggled {
        lerp(LABEL, WHITE, hv / 0.6)
    } else if idle {
        lerp(MUTED, WHITE, hv / 0.6)
    } else {
        lerp(TEXT, WHITE, hv / 0.6)
    };
    // The hot key in a small cap on the right.
    let kpx = l.text(SMALL_PX - 1.0);
    let kw = pen.advance(Face::Bold, kpx, 0.0, b.key()).max(l.px(7.0));
    let cap_w = kw + l.px(10.0);
    let cap_h = l.px(16.0).min(r.height() - 6);
    let cap = Rect::new(
        r.x1 - l.px(8.0) - cap_w,
        (r.y0 + r.y1) / 2 - cap_h / 2,
        r.x1 - l.px(8.0),
        (r.y0 + r.y1) / 2 - cap_h / 2 + cap_h,
    );
    let shift = i32::from(pressed);
    let label_room = cap.x0 - r.x0;
    let w = pen.advance(Face::Bold, px, sp, b.label());
    let lx = r.x0 + (label_room - w) / 2 + l.px(4.0);
    write(
        canvas,
        pen,
        Face::Bold,
        px,
        sp,
        lx.max(r.x0 + l.px(8.0)),
        mid_baseline(r, px) + shift,
        col,
        None,
        b.label(),
    );
    rounded_outline(canvas, cap, 2.0 * l.scale, 1.0, alpha(col, 0.45));
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
        b.key(),
    );
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
    fn mode_switch() {
        assert_eq!(parse_mode("Modern"), Some(true));
        assert_eq!(parse_mode("original"), Some(false));
        assert_eq!(parse_mode("x"), None);
        let o = Options::default();
        assert!(!o.get_bool(OPTION_KEY), "off by default");
        let (on, errors) = Options::from_text("hwcModernBuildManager = 1\n");
        assert!(errors.is_empty());
        assert!(on.get_bool(OPTION_KEY));
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
