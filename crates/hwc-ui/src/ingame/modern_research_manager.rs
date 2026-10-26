//! The research manager (`research_manager.fib` and the Beast's `beast_research_manager.fib`,
//! [`research`]) restyled in the modern HUD's look, the build manager's companion: a
//! user-requested change, on by default; `--research-ui original`,
//! `hwcModernResearchManager = 0` or the original HUD (`--hud original`) bring back the
//! original screen. The look is shared with the build manager ([`super::modern_manager`]).
//!
//! Only the look and the mouse handling are new. The original screen stays open underneath
//! (not drawn, and not given the mouse), so its hot keys (R research, L clear lab, I info,
//! B build, S systems, C and Escape close, the list's arrow keys), its callbacks and every
//! rule of [`research`] (which topics are offered, the lab a topic needs, whether its lab is
//! free, speech and sounds) are the same code. The topic list is the screen's own list
//! window (the same rows, selection and scroll position). The mouse acts through what the
//! original's regions do:
//!
//! * lab tabs: `rmSelectLab1..5` (a press; an absent lab does nothing; the second press of a
//!   double click is not seen, as there);
//! * a topic row: the list window's release (select, with the click sound, and show the
//!   topic); the second press of a double click is the list's double-click message, which
//!   researches it (`RM_TechListWindow`); headings cannot be picked;
//! * buttons: pressed with the click sound, run their callback on release over them (never
//!   on the second press of a double click), like `uicButtonProcess`;
//! * the wheel over the list scrolls three rows, the thin scroll bar drags.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::consts::*;
use crate::controls::ListItem;
use crate::ingame::host::{HostView, Lab, LabStatus, Race, Tech, TechId, TechState};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_manager::{self as mm, *};
use crate::ingame::modern_research;
use crate::ingame::research::{self, Picture};
use crate::ingame::{Fe, Manager};
use crate::options::Options;

/// The options-file key: 1 (default) this research manager, 0 the original's.
pub const OPTION_KEY: &str = "hwcModernResearchManager";

/// `--research-ui`, the options file and the HUD's choice.
static SWITCH: Switch = Switch::new(OPTION_KEY);

/// Sets the command line's choice; `None`: the options file decides.
pub fn set_override(modern: Option<bool>) {
    SWITCH.set_override(modern);
}

/// Whether the modern research manager replaces the original's look.
pub fn enabled(options: &Options) -> bool {
    SWITCH.enabled(options)
}

/// Parses `--research-ui`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    mm::parse_mode(s)
}

/// Whether the modern research manager is up now (enabled, and the research manager open).
pub fn active(fe: &Fe) -> bool {
    fe.game.ig.manager == Some(Manager::Research) && enabled(&fe.game.options)
}

/// A researched topic's mark and status.
const RESEARCHED: Color = modern_research::COMPLETE_COL;
/// Research counts down eight times as fast in the tutorial (`rmUpdateResearch`).
const TUTORIAL_RATE: f32 = 8.0;

// ---- layout (at 1080 lines; the panel, header and footer are [`mm::Frame`]'s) ----

const TABS_Y: f32 = 68.0;
const TAB_H: f32 = 46.0;
const TAB_GAP: f32 = 8.0;
/// Five labs across the panel; the Beast's single lab is two of them wide.
const LAB_TAB_W: f32 = (REF_W - 2.0 * PAD - 4.0 * TAB_GAP) / 5.0;
const BODY_Y: f32 = 126.0;
const LEFT_W: f32 = 532.0;
const RIGHT_X: f32 = 580.0;
const STATS_H: f32 = 38.0;
const STAT_GAP: f32 = 16.0;
const LIST_HEAD_BASELINE: f32 = 190.0;
const LIST_Y: f32 = 202.0;
const ROW_H: f32 = 24.0;
const LIST_ROWS: f32 = 18.0;
const PICTURE_H: f32 = 300.0;

/// The buttons of the original screen, by their atom (and callback) names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    Research,
    Clear,
    Close,
    Build,
    Systems,
    Info,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::Research,
        Button::Clear,
        Button::Close,
        Button::Build,
        Button::Systems,
        Button::Info,
    ];

    /// The atom of `research_manager.fib` the button stands for (its callback's name).
    pub fn atom(self) -> &'static str {
        match self {
            Button::Research => "RM_ResearchItem",
            Button::Clear => "RM_ClearLab",
            Button::Close => "RM_ExitMenu",
            Button::Build => "RM_Construction",
            Button::Systems => "RM_Launch",
            Button::Info => "RM_ExtendedInfo",
        }
    }

    /// The label, as the screen's text (without its `&` hot-key mark).
    pub fn label(self) -> &'static str {
        match self {
            Button::Research => "RESEARCH",
            Button::Clear => "CLEAR LAB",
            Button::Close => "CLOSE",
            Button::Build => "BUILD",
            Button::Systems => "SYSTEMS",
            Button::Info => "INFO",
        }
    }

    /// The hot key shown on the button (the letter the screen's `&` marks).
    pub fn key(self) -> &'static str {
        match self {
            Button::Research => "R",
            Button::Clear => "L",
            Button::Close => "C",
            Button::Build => "B",
            Button::Systems => "S",
            Button::Info => "I",
        }
    }
}

/// Where everything goes on a canvas of a given size.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub scale: f32,
    pub frame: Frame,
    pub panel: Rect,
    /// The lab tabs, in lab order (five, or the Beast's one).
    pub labs: Vec<Rect>,
    /// The readouts: labs in use, topics researched.
    pub stats: [Rect; 2],
    pub list_head_baseline: i32,
    /// The rows' area; `page` rows of `row_h` fit.
    pub list: Rect,
    pub row_h: i32,
    pub page: usize,
    pub scroll_track: Rect,
    /// Columns: a heading's x, a topic's x, the status's right edge.
    pub heading_x: i32,
    pub name_x: i32,
    pub status_right: i32,
    pub picture: Rect,
    pub details: Rect,
    pub buttons: std::collections::BTreeMap<Button, Rect>,
}

/// The layout for a `width` x `height` canvas with `labs` labs (5, or 1 for the Beast).
pub fn layout(width: i32, height: i32, labs: usize) -> Layout {
    let f = Frame::new(width, height);
    let px = |v: f32| f.px(v);
    let (x0, y0) = (f.panel.x0, f.panel.y0);
    let labs = if labs <= 1 {
        vec![f.at(PAD, TABS_Y, 2.0 * LAB_TAB_W + TAB_GAP, TAB_H)]
    } else {
        (0..labs.min(5))
            .map(|i| {
                f.at(
                    PAD + i as f32 * (LAB_TAB_W + TAB_GAP),
                    TABS_Y,
                    LAB_TAB_W,
                    TAB_H,
                )
            })
            .collect()
    };
    let half = (LEFT_W - STAT_GAP) / 2.0;
    let stats = [
        f.at(PAD, BODY_Y, half, STATS_H),
        f.at(PAD + half + STAT_GAP, BODY_Y, half, STATS_H),
    ];
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
    let picture = f.at(RIGHT_X, BODY_Y, REF_W - PAD - RIGHT_X, PICTURE_H);
    let details = Rect::new(
        picture.x0,
        picture.y1 + px(10.0),
        picture.x1,
        f.footer_line - px(8.0),
    );
    let mut buttons = std::collections::BTreeMap::new();
    buttons.insert(Button::Research, f.footer_button(0));
    buttons.insert(Button::Clear, f.footer_button(1));
    buttons.insert(Button::Close, f.footer_right());
    buttons.insert(Button::Systems, f.top_button(0));
    buttons.insert(Button::Build, f.top_button(1));
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
        scale: f.scale,
        frame: f,
        panel: f.panel,
        labs,
        stats,
        list_head_baseline: y0 + px(LIST_HEAD_BASELINE),
        list,
        row_h,
        page: LIST_ROWS as usize,
        scroll_track,
        heading_x: list.x0 + px(4.0),
        name_x: list.x0 + px(22.0),
        status_right: list.x1 - px(10.0),
        picture,
        details,
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
    /// A lab tab, by lab.
    Lab(usize),
    /// The `i`-th row on screen (a heading or a topic).
    Row(usize),
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
    if let Some(i) = l.labs.iter().position(|t| t.contains(x, y)) {
        return Hit::Lab(i);
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
            return Hit::Row(i);
        }
    }
    Hit::Panel
}

/// What eases its hover / press highlight.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Button(Button),
    Lab(usize),
    Row(TechId),
    Thumb,
}

/// The modern manager's own state (the research manager's is [`research::State`]).
#[derive(Debug, Clone, Default)]
pub struct State {
    mouse: (i32, i32),
    /// Left press taken by a button, a list row or the thumb.
    pressed: Option<Hit>,
    clicks: DoubleClick,
    /// Dragging the thumb: the pointer's offset from its top.
    drag: Option<i32>,
    hover: Hover<Key>,
}

/// Opening the research manager: a fresh state.
pub fn reset(fe: &mut Fe) {
    fe.game.ig.mrm = State::default();
}

fn size(fe: &Fe) -> (i32, i32) {
    fe.screen_size()
}

fn layout_now(fe: &Fe) -> Layout {
    let (w, h) = size(fe);
    layout(w, h, fe.game.ig.view.labs.len())
}

/// The list's rows (the screen's list window), and its first row on screen.
fn list_rows(fe: &Fe) -> (Vec<ListItem>, usize) {
    match research::list_atom(fe).and_then(|a| fe.list(a)) {
        Some(l) => (l.items.clone(), l.upper),
        None => (Vec::new(), 0),
    }
}

/// The rows on screen and how many rows the list has.
fn on_screen(fe: &Fe, l: &Layout) -> (Vec<ListItem>, usize, usize) {
    let (all, upper) = list_rows(fe);
    let total = all.len();
    let top = upper.min(total.saturating_sub(l.page));
    let rows = all.into_iter().skip(top).take(l.page).collect();
    (rows, total, top)
}

fn hit_now(fe: &Fe, l: &Layout, x: i32, y: i32) -> (Hit, Vec<ListItem>, usize) {
    let (rows, total, top) = on_screen(fe, l);
    (hit(l, x, y, rows.len(), total, top), rows, top)
}

/// Shows `top` first in the screen's list, with the modern page (as its own scrolling).
fn scroll_to(fe: &mut Fe, top: usize, page: usize) {
    let Some(a) = research::list_atom(fe) else {
        return;
    };
    let Some(l) = fe.list_mut(a) else { return };
    l.max_visible = page;
    let last = l.items.len().saturating_sub(page);
    let top = top.min(last);
    if l.upper == top {
        return;
    }
    l.upper = top;
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

fn scroll_by(fe: &mut Fe, delta: isize) {
    let l = layout_now(fe);
    let (_, upper) = list_rows(fe);
    let top = (upper as isize + delta).max(0) as usize;
    scroll_to(fe, top, l.page);
}

/// The list window's message for an item (`uicListWindowProcess`): it becomes the
/// selection and the screen's callback is told (`CM_NEW_ITEM_SELECTED` with the click
/// sound, or `CM_DOUBLE_CLICK`).
fn list_message(fe: &mut Fe, index: usize, msg: u32) {
    let Some(a) = research::list_atom(fe) else {
        return;
    };
    let Some(l) = fe.list_mut(a) else { return };
    if !l.items.get(index).is_some_and(|i| i.can_select) {
        return;
    }
    l.select(index);
    l.message = msg;
    if msg == CM_NEW_ITEM_SELECTED {
        fe.ui_sound(sound::CLICK);
    }
    fe.function_execute("RM_TechListWindow", Some(a), false);
}

/// The pointer moved.
pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    fe.game.ig.mrm.mouse = (x, y);
    if let Some(grab) = fe.game.ig.mrm.drag {
        let l = layout_now(fe);
        let total = list_rows(fe).0.len();
        let top = l.top_for_thumb(total, y - grab);
        scroll_to(fe, top, l.page);
    }
}

/// A mouse button over the screen (the modern HUD's panels have had their turn).
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool) {
    if !left {
        // The original's regions take no right clicks here.
        return;
    }
    let l = layout_now(fe);
    let (x, y) = fe.game.ig.mrm.mouse;
    let (at, _, top) = hit_now(fe, &l, x, y);
    if !down {
        fe.game.ig.mrm.clicks.release();
        let pressed = fe.game.ig.mrm.pressed.take();
        fe.game.ig.mrm.drag = None;
        match (pressed, at) {
            (Some(Hit::Button(b)), Hit::Button(now)) if b == now => activate(fe, b),
            // The list acts on the release of a press it took.
            (Some(Hit::Row(_)), Hit::Row(i)) => list_message(fe, top + i, CM_NEW_ITEM_SELECTED),
            _ => {}
        }
        return;
    }
    // The second press of a double click (`WM_LBUTTONDBLCLK`): only the list sees it.
    let now = fe.time;
    let st = &mut fe.game.ig.mrm;
    if st.clicks.held() || st.pressed.is_some() {
        return;
    }
    if st.clicks.press(now, x, y) {
        if let Hit::Row(i) = at {
            list_message(fe, top + i, CM_DOUBLE_CLICK);
        }
        return;
    }
    match at {
        Hit::Button(_) => {
            fe.game.ig.mrm.pressed = Some(at);
            fe.ui_sound(sound::CLICK);
        }
        // The Beast's lab button is off its screen (the blob stands there instead).
        Hit::Lab(i) if !beast(fe) => research::select_lab(fe, RPE_PRESS_LEFT, i),
        Hit::Lab(_) => {}
        Hit::Row(_) => fe.game.ig.mrm.pressed = Some(at),
        Hit::Thumb => {
            let total = list_rows(fe).0.len();
            if let Some(th) = l.thumb(total, top) {
                fe.game.ig.mrm.pressed = Some(at);
                fe.game.ig.mrm.drag = Some(y - th.y0);
            }
        }
        Hit::Track(up) => {
            let page = l.page as isize;
            scroll_by(fe, if up { -page } else { page });
        }
        Hit::Panel | Hit::Outside => {}
    }
}

/// The wheel: three rows over the list, as the original's list.
pub fn wheel(fe: &mut Fe, up: bool) {
    let l = layout_now(fe);
    let (x, y) = fe.game.ig.mrm.mouse;
    if l.list.contains(x, y)
        || l.scroll_track.x0 - l.px(6.0) <= x
            && x < l.scroll_track.x1 + l.px(6.0)
            && l.list.y0 <= y
            && y < l.list.y1
    {
        scroll_by(fe, if up { -3 } else { 3 });
    }
}

fn beast(fe: &Fe) -> bool {
    fe.game.ig.view.player.race == Race::Beast
}

/// A button released over itself: its callback, as the screen's button would run it.
fn activate(fe: &mut Fe, b: Button) {
    let atom = fe.find_atom(b.atom());
    fe.function_execute(b.atom(), atom, false);
}

/// Once a frame while open: the list's page size.
pub fn tick(fe: &mut Fe) {
    if !active(fe) {
        return;
    }
    let l = layout_now(fe);
    let (_, upper) = list_rows(fe);
    scroll_to(fe, upper, l.page);
}

/// The panel (for the modern HUD's panels to keep clear of), when up.
pub fn area(fe: &Fe, width: i32, height: i32) -> Option<Rect> {
    active(fe).then(|| layout(width, height, fe.game.ig.view.labs.len()).panel)
}

// ---- what the drawing shows ----

/// How a topic stands, as the list and the details show it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Standing {
    Available,
    /// Offered, but its lab is busy with another topic.
    LabBusy,
    Researching {
        progress: f32,
        paused: bool,
    },
    Researched,
    Unavailable,
}

/// The lab a topic is researched in (the Beast has one for all).
fn lab_of(view: &HostView, t: &Tech) -> usize {
    if view.player.race == Race::Beast {
        0
    } else {
        t.lab
    }
}

pub fn standing(view: &HostView, t: &Tech) -> Standing {
    match t.state {
        TechState::Researched => Standing::Researched,
        TechState::Unavailable => Standing::Unavailable,
        TechState::Researching => {
            let paused = !view
                .labs
                .iter()
                .any(|l| l.topic == Some(t.id) && l.status == LabStatus::Researching);
            Standing::Researching {
                progress: research::progress_of(view, t.id),
                paused,
            }
        }
        TechState::Available => {
            let idle = view
                .labs
                .get(lab_of(view, t))
                .is_some_and(|l| l.status == LabStatus::Idle);
            if idle {
                Standing::Available
            } else {
                Standing::LabBusy
            }
        }
    }
}

fn rate(view: &HostView) -> f32 {
    if view.player.tutorial {
        TUTORIAL_RATE
    } else {
        1.0
    }
}

/// What a lab is called: the Beast's one lab is its research (its screen names no module);
/// the others by their module, for a tab (`short`) without the trailing "MODULE".
fn lab_title(view: &HostView, lab: usize, short: bool) -> String {
    if view.player.race == Race::Beast {
        return "RESEARCH".to_string();
    }
    let name = view.labs.get(lab).map_or("", |l| l.name.as_str());
    if short {
        short_lab_name(name)
    } else {
        name.trim().to_uppercase()
    }
}

/// A lab's name for its tab: without the trailing "MODULE" every lab's name has.
pub fn short_lab_name(name: &str) -> String {
    let n = name.trim().to_uppercase();
    match n.strip_suffix(" MODULE") {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => n,
    }
}

fn percent(p: f32) -> String {
    format!("{}%", (p.clamp(0.0, 1.0) * 100.0).floor() as i32)
}

// ---- drawing ----

fn eased(fe: &Fe, key: Key) -> f32 {
    fe.game.ig.mrm.hover.get(key)
}

/// Draws the manager over the 3D view in `canvas` (the modern HUD's panels come after).
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let l = layout(canvas.width, canvas.height, fe.game.ig.view.labs.len());
    let (mx, my) = fe.game.ig.mrm.mouse;
    let (rows, total, top) = on_screen(fe, &l);
    let at = hit(&l, mx, my, rows.len(), total, top);
    let pressed = fe.game.ig.mrm.pressed;
    // Highlights: what is under the pointer, brighter while pressed.
    let mut targets = Vec::new();
    match at {
        Hit::Button(b) => {
            let v = if pressed == Some(at) { 1.0 } else { 0.6 };
            targets.push((Key::Button(b), v));
        }
        Hit::Lab(i) if !beast(fe) => targets.push((Key::Lab(i), 0.6)),
        Hit::Row(i) => {
            if let Some(r) = rows.get(i).filter(|r| r.can_select) {
                targets.push((Key::Row(r.data), 0.6));
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
    if fe.game.ig.mrm.drag.is_some() {
        targets.push((Key::Thumb, 1.0));
    }
    let now = fe.time;
    fe.game.ig.mrm.hover.step(now, &targets);

    mm::draw_frame(canvas, &l.frame);
    // The selected lab's module, as `RM_ModuleNameDraw` (the Beast's screen has none).
    let view = &fe.game.ig.view;
    let subtitle = fe
        .game
        .ig
        .rm
        .selected_lab
        .filter(|&i| view.player.race != Race::Beast && i < view.labs.len())
        .map(|i| lab_title(view, i, false));
    mm::draw_title(
        canvas,
        &pen,
        &l.frame,
        "RESEARCH MANAGER",
        subtitle.as_deref(),
    );
    draw_labs(fe, canvas, &pen, &l);
    draw_stats(fe, canvas, &pen, &l);
    draw_list(fe, canvas, &pen, &l, &rows, total, top);
    draw_picture(fe, canvas, &pen, &l);
    draw_details(fe, canvas, &pen, &l);
    for b in Button::ALL {
        draw_button(fe, canvas, &pen, &l, b);
    }
}

/// The lab tabs: each lab's name, what it is researching (or idle, or not built) and its
/// progress along the bottom edge; the selected lab is the current tab.
fn draw_labs(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let view = &fe.game.ig.view;
    for (i, &r) in l.labs.iter().enumerate() {
        let lab = view.labs.get(i).cloned().unwrap_or_default();
        let topic = lab.topic.and_then(|t| view.tech(t));
        let busy = matches!(lab.status, LabStatus::Researching | LabStatus::Paused);
        let detail = match lab.status {
            LabStatus::Absent => ("NOT INSTALLED".to_string(), GREYED),
            LabStatus::Idle => ("IDLE".to_string(), DIM),
            LabStatus::Researching | LabStatus::Paused => {
                let name = topic.map_or_else(String::new, |t| t.name.to_uppercase());
                let col = if lab.status == LabStatus::Paused {
                    PAUSED
                } else {
                    TEXT
                };
                (name, col)
            }
        };
        let label = lab_title(view, i, true);
        let mut count = match lab.status {
            LabStatus::Paused => Some("PAUSED".to_string()),
            LabStatus::Researching => Some(percent(lab.progress)),
            _ => None,
        };
        let mut detail = detail;
        // A narrow tab: the count goes on the detail line rather than cut the name.
        if let Some(c) = &count {
            let f = &l.frame;
            let need = pen.advance(
                Face::Bold,
                f.text(BUTTON_PX),
                BUTTON_SPACING * f.scale,
                &label,
            ) + pen.advance(Face::Bold, f.text(VALUE_PX), 0.0, c)
                + f.px(36.0);
            if need > r.width() {
                detail.0 = if detail.0.is_empty() {
                    c.clone()
                } else {
                    format!("{c}  {}", detail.0)
                };
                count = None;
            }
        }
        let look = TabLook {
            label,
            enabled: lab.status != LabStatus::Absent,
            current: fe.game.ig.rm.selected_lab == Some(i),
            hover: eased(fe, Key::Lab(i)),
            count,
            progress: busy.then_some(lab.progress.clamp(0.0, 1.0)),
            paused: lab.status == LabStatus::Paused,
            detail: Some(detail),
        };
        mm::draw_tab(canvas, pen, &l.frame, r, &look);
    }
}

/// Labs in use, and the topics researched of those offered.
fn draw_stats(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let view = &fe.game.ig.view;
    let f = &l.frame;
    let built: Vec<&Lab> = view
        .labs
        .iter()
        .filter(|l| l.status != LabStatus::Absent)
        .collect();
    let working = built
        .iter()
        .filter(|l| matches!(l.status, LabStatus::Researching | LabStatus::Paused))
        .count();
    let value = format!("{working} / {}", built.len());
    let br = mm::readout(
        canvas,
        pen,
        f,
        l.stats[0],
        "LABS IN USE",
        &value,
        WHITE,
        value_glow(f),
    );
    let fill = if built.is_empty() {
        0.0
    } else {
        working as f32 / built.len() as f32
    };
    mm::bar(canvas, f, br, fill, 1.0);
    let offered: Vec<&Tech> = view
        .techs
        .iter()
        .filter(|t| t.state != TechState::Unavailable)
        .collect();
    let done = offered
        .iter()
        .filter(|t| t.state == TechState::Researched)
        .count();
    let value = format!("{done} / {}", offered.len());
    let br = mm::readout(
        canvas,
        pen,
        f,
        l.stats[1],
        "RESEARCHED",
        &value,
        WHITE,
        value_glow(f),
    );
    let fill = if offered.is_empty() {
        0.0
    } else {
        done as f32 / offered.len() as f32
    };
    mm::bar(canvas, f, br, fill, 1.0);
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    fe: &Fe,
    canvas: &mut Canvas,
    pen: &Pen,
    l: &Layout,
    rows: &[ListItem],
    total: usize,
    top: usize,
) {
    let f = &l.frame;
    let lpx = l.text(SMALL_PX - 1.0);
    let lsp = SMALL_SPACING * l.scale;
    let head = l.list_head_baseline;
    let well = Rect::new(
        l.list.x0 - l.px(6.0),
        head - l.px(20.0),
        l.scroll_track.x1 + l.px(6.0),
        l.list.y1 + l.px(8.0),
    );
    mm::well(canvas, f, well);
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
        "TECHNOLOGY",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        lpx,
        lsp,
        l.status_right,
        head,
        DIM,
        None,
        "STATUS",
    );
    hline(canvas, l.list.x0, l.list.x1, head + l.px(8.0), LINE);
    let view = &fe.game.ig.view;
    let npx = l.text(NAME_PX);
    let spx = l.text(SMALL_PX - 1.0);
    if rows.is_empty() {
        write(
            canvas,
            pen,
            Face::Semibold,
            l.text(BODY_PX),
            0.0,
            l.heading_x,
            l.list.y0 + l.px(20.0),
            DIM,
            None,
            "No research is available.",
        );
    }
    for (i, row) in rows.iter().enumerate() {
        let r = l.row(i);
        if !row.can_select {
            mm::list_heading(canvas, pen, f, l.heading_x, r, l.status_right, &row.text);
            continue;
        }
        let Some(t) = view.tech(row.data) else {
            continue;
        };
        let hover = eased(fe, Key::Row(t.id));
        mm::row_band(canvas, f, r, row.selected, hover, 0.0);
        let st = standing(view, t);
        let researching = matches!(st, Standing::Researching { .. });
        let name_col = match st {
            Standing::Researched => MUTED,
            Standing::Unavailable => GREYED,
            Standing::Researching { paused: true, .. } => PAUSED,
            _ => lerp(TEXT, WHITE, hover),
        };
        let base = mid_baseline(r, npx) - if researching { l.px(2.0) } else { 0 };
        // A researched topic's mark, as the original's dot.
        if st == Standing::Researched {
            let (cx, cy) = (l.name_x - l.px(10.0), mid_baseline(r, npx) - l.px(5.0));
            let rad = l.px(3.0).max(2);
            rounded(
                canvas,
                Rect::new(cx - rad, cy - rad, cx + rad, cy + rad),
                rad as f32,
                RESEARCHED,
            );
        }
        // The status on the right.
        let (status, scol, sglow) = match st {
            Standing::Available => ("AVAILABLE".to_string(), LABEL, None),
            Standing::LabBusy => ("LAB BUSY".to_string(), DIM, None),
            Standing::Researching { paused: true, .. } => ("PAUSED".to_string(), PAUSED_TAG, None),
            Standing::Researching { progress, .. } => (percent(progress), WHITE, value_glow(f)),
            Standing::Researched => ("RESEARCHED".to_string(), RESEARCHED, None),
            Standing::Unavailable => ("UNAVAILABLE".to_string(), GREYED, None),
        };
        let (status_px, status_sp, status_face) =
            if matches!(st, Standing::Researching { paused: false, .. }) {
                (l.text(VALUE_PX), 0.0, Face::Bold)
            } else {
                (spx, lsp, Face::Bold)
            };
        let status_w = pen.advance(status_face, status_px, status_sp, &status);
        write_right(
            canvas,
            pen,
            status_face,
            status_px,
            status_sp,
            l.status_right,
            mid_baseline(r, npx),
            scol,
            sglow,
            &status,
        );
        // The name, and an upgrade's tag after it.
        let room = (l.status_right - status_w - l.px(16.0) - l.name_x) as f32;
        let mut name = fit(pen, Face::Semibold, npx, 0.0, &t.name.to_uppercase(), room);
        if name.is_empty() {
            name = t.name.to_uppercase();
        }
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
            &name,
        );
        if t.upgrade {
            let tx = l.name_x + w + l.px(10.0);
            let tag = "UPGRADE";
            let tw = pen.advance(Face::Bold, l.text(SMALL_PX - 2.0), lsp, tag);
            if tx + tw < l.status_right - status_w - l.px(12.0) {
                write(
                    canvas,
                    pen,
                    Face::Bold,
                    l.text(SMALL_PX - 2.0),
                    lsp,
                    tx,
                    base,
                    alpha(LABEL, 0.6),
                    None,
                    tag,
                );
            }
        }
        if let Standing::Researching { progress, paused } = st {
            let y = r.y1 - l.px(6.0);
            let x1 = l.status_right - l.px(64.0);
            let br = Rect::new(l.name_x, y, x1, y + l.px(3.0).max(2));
            if paused {
                mm::paused_bar(canvas, br, progress);
            } else {
                mm::bar(canvas, f, br, progress, 0.9);
            }
        }
    }
    if let Some(th) = l.thumb(total, top) {
        mm::draw_scroll(canvas, l.scroll_track, th, eased(fe, Key::Thumb));
    }
}

/// The picture's file (`RM_TechImageDraw`): the topic's, the module's, or the default one.
fn picture_file(fe: &Fe) -> String {
    match fe.game.ig.rm.picture {
        Picture::Default => "Default_Research_Image".to_string(),
        Picture::Tech(t) => fe
            .game
            .ig
            .view
            .tech(t)
            .map_or_else(|| "Default_Research_Image".into(), |t| t.key.clone()),
        Picture::Module(m) => research::MODULE_IMAGES[m.min(4)].to_string(),
    }
}

/// The picture, large, with the corner brackets; the description over it while Info is on.
fn draw_picture(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let r = l.picture;
    mm::picture_box(canvas, f, r);
    let race = fe.game.ig.view.player.race.file_index();
    let file = picture_file(fe);
    let res = fe.resources();
    let img = res
        .image(&format!("ResearchGUI\\Race{race}\\{file}.lif"))
        .or_else(|| res.image(&format!("ResearchGUI\\Race1\\{file}.lif")));
    let inner = Rect::new(
        r.x0 + l.px(10.0),
        r.y0 + l.px(10.0),
        r.x1 - l.px(10.0),
        r.y1 - l.px(10.0),
    );
    if let Some(img) = img {
        let dst = mm::fit_rect(inner, img.width, img.height);
        mm::blit_smooth(canvas, &img, dst, 1.0);
        // A hairline around the picture.
        mm::rounded_outline(
            canvas,
            Rect::new(dst.x0 - 1, dst.y0 - 1, dst.x1 + 1, dst.y1 + 1),
            1.0,
            1.0,
            [255, 255, 255, 30],
        );
    }
    mm::brackets(canvas, f, r);
    if fe.game.ig.rm.extended_info {
        let rm = &fe.game.ig.rm;
        let view = &fe.game.ig.view;
        // As the original: the topic's description, or the module's name.
        let (heading, text) = match rm.picture {
            Picture::Tech(t) => (
                "TECHNOLOGY INFORMATION",
                view.tech(t)
                    .map(|t| t.description.clone())
                    .unwrap_or_default(),
            ),
            Picture::Module(m) => (
                "RESEARCH MODULE",
                view.labs.get(m).map(|l| l.name.clone()).unwrap_or_default(),
            ),
            Picture::Default => ("TECHNOLOGY INFORMATION", String::new()),
        };
        let text = if text.trim().is_empty() {
            "No information.".to_string()
        } else {
            text.replace("\\n", "\n")
        };
        mm::text_pane(canvas, pen, f, r, heading, &text);
    }
}

/// The pictured topic (or lab): its name, where it is researched and its figures; then the
/// topics being researched with their progress.
fn draw_details(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let d = l.details;
    let view = &fe.game.ig.view;
    let lpx = l.text(SMALL_PX);
    let lsp = SMALL_SPACING * l.scale;
    let x = d.x0 + l.px(2.0);
    let mut y = d.y0 + l.px(24.0);
    let info = l.buttons[&Button::Info];
    let title = |canvas: &mut Canvas, y: i32, name: &str, class: &str| {
        let tpx = l.text(TITLE_BIG_PX);
        let name = fit(
            pen,
            Face::Bold,
            tpx,
            0.0,
            &name.to_uppercase(),
            (info.x0 - x - l.px(10.0)) as f32,
        );
        write(
            canvas,
            pen,
            Face::Bold,
            tpx,
            0.0,
            x,
            y,
            WHITE,
            Some((modern_hud::VALUE_GLOW, 4.0 * l.scale)),
            &name,
        );
        write(
            canvas,
            pen,
            Face::Bold,
            lpx,
            lsp,
            x,
            y + l.px(20.0),
            GOLD,
            None,
            &class.to_uppercase(),
        );
    };
    let dash = || "\u{2014}".to_string();
    let rate = rate(view);
    let cells: Option<Vec<(&str, String)>> = match fe.game.ig.rm.picture {
        Picture::Tech(id) => view.tech(id).map(|t| {
            title(canvas, y, &t.name, &lab_title(view, lab_of(view, t), false));
            let st = standing(view, t);
            let status = match st {
                Standing::Available => "AVAILABLE".to_string(),
                Standing::LabBusy => "LAB BUSY".to_string(),
                Standing::Researching { paused: true, .. } => "PAUSED".to_string(),
                Standing::Researching { progress, .. } => percent(progress),
                Standing::Researched => "RESEARCHED".to_string(),
                Standing::Unavailable => "UNAVAILABLE".to_string(),
            };
            let time = if t.time > 0.0 {
                modern_research::time_text(t.time / rate)
            } else {
                dash()
            };
            let left = match st {
                Standing::Researching { progress, .. } if t.time > 0.0 => {
                    modern_research::time_text((1.0 - progress) * t.time / rate)
                }
                _ => dash(),
            };
            let kind = if t.upgrade { "UPGRADE" } else { "TECHNOLOGY" };
            vec![
                ("STATUS", status),
                ("TYPE", kind.to_string()),
                ("TIME", time),
                ("TIME LEFT", left),
            ]
        }),
        Picture::Module(m) => view.labs.get(m).map(|lab| {
            title(canvas, y, &lab_title(view, m, false), "RESEARCH MODULE");
            let topic = lab.topic.and_then(|t| view.tech(t));
            let status = match lab.status {
                LabStatus::Absent => "NOT INSTALLED",
                LabStatus::Idle => "IDLE",
                LabStatus::Researching => "RESEARCHING",
                LabStatus::Paused => "PAUSED",
            };
            let left = match topic {
                Some(t) if t.time > 0.0 && lab.status == LabStatus::Researching => {
                    modern_research::time_text((1.0 - lab.progress) * t.time / rate)
                }
                _ => dash(),
            };
            let time = match topic {
                Some(t) if t.time > 0.0 => modern_research::time_text(t.time / rate),
                _ => dash(),
            };
            vec![
                ("TOPIC", topic.map_or_else(dash, |t| t.name.to_uppercase())),
                ("STATUS", status.to_string()),
                (
                    "PROGRESS",
                    if topic.is_some() {
                        percent(lab.progress)
                    } else {
                        dash()
                    },
                ),
                ("TIME", time),
                ("TIME LEFT", left),
            ]
        }),
        Picture::Default => None,
    };
    y += l.px(34.0);
    match cells {
        Some(cells) => {
            // A lab's topic gets a whole row, above the figures.
            let (wide, rest): (Vec<_>, Vec<_>) =
                cells.into_iter().partition(|(k, _)| *k == "TOPIC");
            for (k, v) in wide {
                let r = Rect::new(x - l.px(2.0), y, d.x1, y + l.px(22.0));
                let v = fit(
                    pen,
                    Face::Bold,
                    l.text(VALUE_PX - 1.0),
                    0.0,
                    &v,
                    (r.width() - l.px(90.0)) as f32,
                );
                mm::cell(canvas, pen, f, r, k, &v);
                y += l.px(26.0);
            }
            y += mm::cell_grid(canvas, pen, f, x, y, d.width(), &rest);
        }
        None => {
            write(
                canvas,
                pen,
                Face::Bold,
                lpx,
                lsp,
                x,
                d.y0 + l.px(24.0),
                DIM,
                None,
                "NO TECHNOLOGY SELECTED",
            );
            y += 2 * l.px(26.0);
        }
    }
    // What is being researched now.
    y += l.px(20.0);
    mm::section_heading(canvas, pen, f, x, y, d.x1, "IN RESEARCH");
    y += l.px(8.0);
    let topics = modern_research::topics(view);
    let line = l.px(22.0);
    let room = ((d.y1 - y) / line).max(0) as usize;
    if topics.is_empty() {
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
    let shown = if topics.len() > room {
        room.saturating_sub(1)
    } else {
        topics.len()
    };
    let bpx = l.text(BODY_PX);
    for e in topics.iter().take(shown) {
        let base = y + l.px(14.0);
        let col = if e.paused { PAUSED } else { TEXT };
        let right = e
            .seconds_left
            .filter(|_| !e.paused)
            .map_or_else(|| percent(e.progress), modern_research::time_text);
        let name = fit(pen, Face::Semibold, bpx, 0.0, &e.name, l.px(140.0) as f32);
        write(
            canvas,
            pen,
            Face::Semibold,
            bpx,
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
            bpx,
            0.0,
            d.x1,
            base,
            WHITE,
            None,
            &right,
        );
        let br = Rect::new(
            x + l.px(150.0),
            base - l.px(5.0),
            d.x1 - l.px(50.0),
            base - l.px(5.0) + l.px(3.0).max(2),
        );
        if e.paused {
            mm::paused_bar(canvas, br, e.progress);
        } else {
            mm::bar(canvas, f, br, e.progress, 0.8);
        }
        y += line;
    }
    if shown < topics.len() {
        let more = format!("+{} MORE", topics.len() - shown);
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
    let st = &fe.game.ig.mrm;
    let pressed = st.pressed == Some(Hit::Button(b))
        && hit_now(fe, l, st.mouse.0, st.mouse.1).0 == Hit::Button(b);
    let view = &fe.game.ig.view;
    // Research starts the selected topic when it is offered and its lab is free; Clear lab
    // stops the selected lab's research.
    let idle = match b {
        Button::Research => {
            !research::selected_tech(fe).is_some_and(|t| standing(view, t) == Standing::Available)
        }
        Button::Clear => !fe
            .game
            .ig
            .rm
            .selected_lab
            .and_then(|i| view.labs.get(i))
            .is_some_and(|l| l.status == LabStatus::Researching),
        _ => false,
    };
    let look = ButtonLook {
        label: b.label(),
        key: b.key(),
        hover: eased(fe, Key::Button(b)),
        pressed,
        primary: b == Button::Research,
        toggled: b == Button::Info && fe.game.ig.rm.extended_info,
        idle,
        focused: crate::ingame::modern_build_manager::focused(fe, b.atom()),
    };
    mm::draw_button(canvas, pen, &l.frame, r, &look);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingame::host::PlayerInfo;

    const SIZES: [(i32, i32); 6] = [
        (1920, 1080),
        (1024, 768),
        (640, 480),
        (800, 600),
        (1280, 720),
        (3840, 2160),
    ];

    #[test]
    fn panel_matches_the_build_manager() {
        for (w, h) in SIZES {
            let r = layout(w, h, 5);
            let b = crate::ingame::modern_build_manager::layout(w, h);
            assert_eq!(r.panel, b.panel, "{w}x{h}");
            assert_eq!(r.scale, b.scale);
            // The header and footer buttons sit where the build manager's do.
            assert_eq!(
                r.buttons[&Button::Build],
                b.buttons[&crate::ingame::modern_build_manager::Button::Research]
            );
            assert_eq!(
                r.buttons[&Button::Systems],
                b.buttons[&crate::ingame::modern_build_manager::Button::Systems]
            );
            assert_eq!(
                r.buttons[&Button::Close],
                b.buttons[&crate::ingame::modern_build_manager::Button::Close]
            );
        }
    }

    #[test]
    fn everything_sits_inside_the_panel_without_overlaps() {
        for (w, h) in SIZES {
            for labs in [5, 1] {
                let l = layout(w, h, labs);
                let inside = |r: Rect| {
                    r.x0 >= l.panel.x0
                        && r.x1 <= l.panel.x1
                        && r.y0 >= l.panel.y0
                        && r.y1 <= l.panel.y1
                };
                let mut all: Vec<(String, Rect)> = l
                    .buttons
                    .iter()
                    .map(|(b, r)| (format!("{b:?}"), *r))
                    .collect();
                for (i, t) in l.labs.iter().enumerate() {
                    all.push((format!("lab {i}"), *t));
                }
                all.push(("stat 0".into(), l.stats[0]));
                all.push(("stat 1".into(), l.stats[1]));
                all.push(("list".into(), l.list));
                all.push(("picture".into(), l.picture));
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
                assert_eq!(l.labs.len(), labs);
                assert!(l.list.y1 < l.frame.footer_line, "{w}x{h}");
                assert!(l.details.y1 <= l.frame.footer_line);
                assert!(l.list.x1 <= l.scroll_track.x0);
                assert!(l.name_x < l.status_right && l.status_right <= l.list.x1);
                // The info button sits on the details, below the picture.
                assert!(l.buttons[&Button::Info].y0 >= l.picture.y1);
            }
        }
    }

    #[test]
    fn lab_tabs_share_the_width() {
        let l = layout(1920, 1080, 5);
        assert_eq!(l.labs[0].x0, l.list.x0);
        assert_eq!(l.labs[4].x1, l.frame.inner_x1());
        let w0 = l.labs[0].width();
        assert!(l.labs.iter().all(|t| (t.width() - w0).abs() <= 1));
        // The Beast's one lab is two tabs wide.
        let b = layout(1920, 1080, 1);
        assert_eq!(b.labs[0].x0, l.labs[0].x0);
        assert_eq!(b.labs[0].x1, l.labs[1].x1);
    }

    #[test]
    fn hit_testing() {
        for (w, h) in SIZES {
            let l = layout(w, h, 5);
            let centre = |r: Rect| ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
            assert_eq!(hit(&l, 2, 2, 20, 20, 0), Hit::Outside);
            for b in Button::ALL {
                let r = l.buttons[&b];
                let (x, y) = centre(r);
                assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Button(b), "{b:?} {w}x{h}");
                assert_eq!(hit(&l, r.x0, r.y0, 20, 20, 0), Hit::Button(b));
                assert_eq!(hit(&l, r.x1 - 1, r.y1 - 1, 20, 20, 0), Hit::Button(b));
            }
            for (i, t) in l.labs.iter().enumerate() {
                let (x, y) = centre(*t);
                assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Lab(i));
            }
            for i in 0..l.page {
                let r = l.row(i);
                assert_eq!(hit(&l, l.name_x, r.y0, l.page, 30, 0), Hit::Row(i));
                assert_eq!(
                    hit(&l, l.status_right - 2, r.y1 - 1, l.page, 30, 0),
                    Hit::Row(i)
                );
            }
            // Rows past the end of the list are the panel.
            let (_, y) = centre(l.row(12));
            assert_eq!(hit(&l, l.name_x, y, 10, 10, 0), Hit::Panel);
            // The scroll bar only when the list is longer than the page.
            let tx = (l.scroll_track.x0 + l.scroll_track.x1) / 2;
            let ty = l.scroll_track.y1 - 2;
            assert_eq!(hit(&l, tx, ty, 18, 18, 0), Hit::Panel);
            assert_eq!(hit(&l, tx, ty, 18, 30, 0), Hit::Track(false));
            let th = l.thumb(30, 0).unwrap();
            assert_eq!(hit(&l, tx, th.y0 + 1, 18, 30, 0), Hit::Thumb);
            assert_eq!(
                hit(&l, tx, l.scroll_track.y0 + 1, 18, 30, 12),
                Hit::Track(true)
            );
            // The picture and the header are just the panel.
            let (x, y) = centre(l.picture);
            assert_eq!(hit(&l, x, y, 20, 20, 0), Hit::Panel);
            assert_eq!(
                hit(&l, l.panel.x0 + 10, l.panel.y0 + 5, 20, 20, 0),
                Hit::Panel
            );
        }
    }

    #[test]
    fn thumb_drag_round_trips() {
        let l = layout(1920, 1080, 5);
        for top in 0..=12 {
            let th = l.thumb(30, top).unwrap();
            assert_eq!(l.top_for_thumb(30, th.y0), top);
        }
        assert!(l.thumb(18, 0).is_none());
    }

    fn view() -> HostView {
        let tech = |id, lab, state| Tech {
            id,
            name: format!("T{id}"),
            cost: 0,
            time: 100.0,
            lab,
            state,
            upgrade: false,
            description: String::new(),
            key: String::new(),
        };
        HostView {
            labs: vec![
                Lab {
                    name: "HANGAR MODULE".into(),
                    status: LabStatus::Researching,
                    topic: Some(1),
                    progress: 0.25,
                },
                Lab {
                    name: "ARMOR MODULE".into(),
                    status: LabStatus::Idle,
                    topic: None,
                    progress: 0.0,
                },
                Lab {
                    name: "X".into(),
                    status: LabStatus::Paused,
                    topic: Some(4),
                    progress: 0.5,
                },
            ],
            techs: vec![
                tech(1, 0, TechState::Researching),
                tech(2, 0, TechState::Available),
                tech(3, 1, TechState::Available),
                tech(4, 2, TechState::Researching),
                tech(5, 1, TechState::Researched),
                tech(6, 1, TechState::Unavailable),
            ],
            player: PlayerInfo::default(),
            ..HostView::default()
        }
    }

    #[test]
    fn standings() {
        let v = view();
        let st = |id| standing(&v, v.tech(id).unwrap());
        assert_eq!(
            st(1),
            Standing::Researching {
                progress: 0.25,
                paused: false
            }
        );
        assert_eq!(st(2), Standing::LabBusy, "its lab researches another topic");
        assert_eq!(st(3), Standing::Available);
        assert_eq!(
            st(4),
            Standing::Researching {
                progress: 0.5,
                paused: true
            }
        );
        assert_eq!(st(5), Standing::Researched);
        assert_eq!(st(6), Standing::Unavailable);
        // The Beast researches everything in its one lab.
        let mut b = v.clone();
        b.player.race = Race::Beast;
        assert_eq!(standing(&b, b.tech(3).unwrap()), Standing::LabBusy);
    }

    #[test]
    fn names_and_numbers() {
        assert_eq!(short_lab_name("HANGAR MODULE "), "HANGAR");
        assert_eq!(short_lab_name("Adv Engineering Module"), "ADV ENGINEERING");
        assert_eq!(short_lab_name("MODULE"), "MODULE");
        assert_eq!(short_lab_name("LABOR"), "LABOR");
        assert_eq!(percent(0.456), "45%");
        assert_eq!(percent(1.2), "100%");
    }

    #[test]
    fn mode_switch() {
        assert_eq!(parse_mode("MODERN"), Some(true));
        assert_eq!(parse_mode("original"), Some(false));
        assert_eq!(parse_mode("new"), None);
        let o = Options::default();
        assert!(o.get_bool(OPTION_KEY), "on by default");
        assert!(enabled(&o));
        let (off, errors) = Options::from_text("hwcModernResearchManager = 0\n");
        assert!(errors.is_empty());
        assert!(!enabled(&off));
        let (old_hud, _) = Options::from_text("hwcModernHud = 0\n");
        assert!(
            !enabled(&old_hud),
            "the original HUD brings the original back"
        );
    }

    #[test]
    fn button_atoms_are_the_screens() {
        let names: Vec<_> = Button::ALL.iter().map(|b| b.atom()).collect();
        assert_eq!(
            names,
            [
                "RM_ResearchItem",
                "RM_ClearLab",
                "RM_ExitMenu",
                "RM_Construction",
                "RM_Launch",
                "RM_ExtendedInfo"
            ]
        );
    }
}
