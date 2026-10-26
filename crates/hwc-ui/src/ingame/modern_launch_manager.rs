//! The systems manager (the launch manager, `launch_manager.fib` and the Beast's
//! `beast_launch_manager.fib`, [`launchmgr`]): the build and research managers' "Systems"
//! screen, restyled in their look. Part of the modern menus (`--menus`, `hwcModernMenus`,
//! on by default; the original HUD brings back the original screen too).
//!
//! As with the modern menus ([`super::modern_menu`]), the original screen stays open
//! underneath, undrawn, and keeps the keyboard (L launch one, G launch group, A launch all,
//! B build, R research, C and Escape close); the mouse over each modern control is handed
//! to the original region it stands for ([`super::modern_proxy`]): the launcher tabs to
//! `LM_MotherShipDraw` / `LM_Carrier1Draw` / `LM_Carrier2Draw` (`lmSelect*`, a press), the
//! REMAIN DOCKED switches to the auto-launch check boxes, the docked-ship rows to the list
//! window's rows, the repair-priority buttons to the module rows at the button's place
//! (`lmSlotNProcess`), the Beast's emergency power choices to its radio buttons, and the
//! buttons to the screen's buttons. What they show is read from the original's state and
//! [`launchmgr`]'s.

use crate::canvas::{Canvas, Rect, WHITE};
use crate::consts::*;
use crate::controls::LIST_MARGIN;
use crate::ingame::consmgr::factory_in_slot;
use crate::ingame::host::Launcher;
use crate::ingame::launchmgr::{self, BEAST_MOTHERSHIP, SOMTAAW_MOTHERSHIP};
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_manager::{self as mm, *};
use crate::ingame::modern_menu;
use crate::ingame::modern_proxy::{Proxy, Target};
use crate::ingame::modern_research;
use crate::ingame::{Fe, Manager};
use crate::region::AtomRef;

/// Whether the modern systems manager is up now (the modern menus on, the launch manager
/// open).
pub fn active(fe: &Fe) -> bool {
    fe.game.ig.manager == Some(Manager::Launch)
        && modern_menu::enabled(&fe.game.options)
        && fe.current_screen().is_some()
}

// ---- layout (at 1080 lines; the panel, header and footer are [`mm::Frame`]'s) ----

const TABS_Y: f32 = 68.0;
const TAB_H: f32 = 46.0;
const TAB_GAP: f32 = 8.0;
const TAB_W: f32 = (REF_W - 2.0 * PAD - 2.0 * TAB_GAP) / 3.0;
const BODY_Y: f32 = 126.0;
const LEFT_W: f32 = 532.0;
const RIGHT_X: f32 = 580.0;
const RIGHT_W: f32 = REF_W - PAD - RIGHT_X;
const STATS_H: f32 = 38.0;
const STAT_GAP: f32 = 16.0;
const LIST_HEAD_BASELINE: f32 = 190.0;
const LIST_Y: f32 = 202.0;
const ROW_H: f32 = 26.0;
const STAY_ROW_H: f32 = 36.0;
const MODULE_Y: f32 = 160.0;
const MODULE_H: f32 = 64.0;
const POWER_H: f32 = 44.0;
const PRIORITY_W: f32 = 34.0;

/// The original screen's buttons, by their atoms (and callbacks).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Button {
    LaunchOne,
    Launch,
    LaunchAll,
    Close,
    Build,
    Research,
}

impl Button {
    pub const ALL: [Button; 6] = [
        Button::LaunchOne,
        Button::Launch,
        Button::LaunchAll,
        Button::Close,
        Button::Build,
        Button::Research,
    ];

    pub fn atom(self) -> &'static str {
        match self {
            Button::LaunchOne => "LM_LaunchOne",
            Button::Launch => "LM_Launch",
            Button::LaunchAll => "LM_LaunchAll",
            Button::Close => "LM_Close",
            Button::Build => "LM_Construction",
            Button::Research => "LM_Research",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Button::LaunchOne => "LAUNCH ONE",
            Button::Launch => "LAUNCH GROUP",
            Button::LaunchAll => "LAUNCH ALL",
            Button::Close => "CLOSE",
            Button::Build => "BUILD",
            Button::Research => "RESEARCH",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Button::LaunchOne => "L",
            Button::Launch => "G",
            Button::LaunchAll => "A",
            Button::Close => "C",
            Button::Build => "B",
            Button::Research => "R",
        }
    }
}

/// The launcher tabs' regions and the auto-launch check boxes, by slot.
pub const TAB_ATOMS: [&str; 3] = ["LM_MotherShipDraw", "LM_Carrier1Draw", "LM_Carrier2Draw"];
pub const STAY_ATOMS: [&str; 3] = ["LM_AutoLaunchM", "LM_AutoLaunchC1", "LM_AutoLaunchC2"];

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub frame: Frame,
    pub tabs: [Rect; 3],
    pub slots: [Rect; 2],
    /// The list's well and its rows (a page of them).
    pub list: Rect,
    pub rows: Vec<Rect>,
    pub scroll_track: Rect,
    pub stay_heading: i32,
    pub stay: [Rect; 3],
    /// The right column.
    pub right: Rect,
    pub right_heading: i32,
    /// Module rows and each one's three repair-priority buttons.
    pub modules: Vec<(Rect, [Rect; 3])>,
    /// The Beast's emergency power choices.
    pub power: Vec<Rect>,
    pub buttons: Vec<(Button, Rect)>,
}

impl Layout {
    pub fn px(&self, v: f32) -> i32 {
        self.frame.px(v)
    }

    pub fn button(&self, b: Button) -> Rect {
        self.buttons
            .iter()
            .find(|(x, _)| *x == b)
            .map_or(Rect::default(), |(_, r)| *r)
    }

    pub fn thumb(&self, total: usize, top: usize) -> Option<Rect> {
        mm::thumb(
            self.scroll_track,
            self.rows.len(),
            total,
            top,
            self.px(24.0),
        )
    }
}

/// The layout on a `width` x `height` canvas, with a list `page` rows long, `modules`
/// module rows and `power` emergency power choices.
pub fn layout(width: i32, height: i32, page: usize, modules: usize, power: usize) -> Layout {
    let f = Frame::new(width, height);
    let tabs = [0, 1, 2].map(|i| f.at(PAD + i as f32 * (TAB_W + TAB_GAP), TABS_Y, TAB_W, TAB_H));
    let sw = (LEFT_W - STAT_GAP) / 2.0;
    let slots = [0, 1].map(|i| f.at(PAD + i as f32 * (sw + STAT_GAP), BODY_Y, sw, STATS_H));
    let page = page.max(1);
    let list_h = page as f32 * ROW_H + 12.0;
    let list = f.at(PAD, LIST_Y, LEFT_W - 16.0, list_h);
    let rows = (0..page)
        .map(|k| {
            f.at(
                PAD + 4.0,
                LIST_Y + 6.0 + k as f32 * ROW_H,
                LEFT_W - 24.0,
                ROW_H,
            )
        })
        .collect();
    let scroll_track = Rect::new(
        list.x1 + f.px(6.0),
        list.y0 + f.px(4.0),
        list.x1 + f.px(9.0).max(f.px(6.0) + 2),
        list.y1 - f.px(4.0),
    );
    let stay_y = LIST_Y + list_h + 34.0;
    let stay = [0, 1, 2].map(|i| {
        f.at(
            PAD,
            stay_y + 8.0 + i as f32 * STAY_ROW_H,
            LEFT_W,
            STAY_ROW_H,
        )
    });
    let right = f.at(RIGHT_X, BODY_Y, RIGHT_W, FOOTER_LINE - 16.0 - BODY_Y);
    let modules = (0..modules)
        .map(|k| {
            let y = MODULE_Y + k as f32 * MODULE_H;
            let row = f.at(RIGHT_X, y, RIGHT_W, MODULE_H - 6.0);
            let seg = |n: usize| {
                f.at(
                    RIGHT_X + RIGHT_W - 8.0 - (3 - n) as f32 * PRIORITY_W,
                    y + 13.0,
                    PRIORITY_W,
                    30.0,
                )
            };
            (row, [seg(0), seg(1), seg(2)])
        })
        .collect();
    let power = (0..power)
        .map(|k| f.at(RIGHT_X, MODULE_Y + k as f32 * POWER_H, RIGHT_W, POWER_H))
        .collect();
    let mut buttons = vec![
        (Button::Research, f.top_button(0)),
        (Button::Build, f.top_button(1)),
        (Button::Close, f.footer_right()),
    ];
    for (i, b) in [Button::LaunchOne, Button::Launch, Button::LaunchAll]
        .into_iter()
        .enumerate()
    {
        buttons.push((b, f.footer_button(i)));
    }
    Layout {
        tabs,
        slots,
        list,
        rows,
        scroll_track,
        stay_heading: f.panel.y0 + f.px(stay_y - 4.0),
        stay,
        right,
        right_heading: f.panel.y0 + f.px(BODY_Y + 20.0),
        modules,
        power,
        buttons,
        frame: f,
    }
}

// ---- what the screen shows ----

/// The list window of the screen.
fn list_atom(fe: &Fe) -> Option<AtomRef> {
    fe.find_atom("LM_ShipsToLaunch")
}

/// The list's rows on the page, its first row on screen, how many rows it has, its page.
fn list_rows(fe: &Fe) -> (Vec<crate::controls::ListItem>, usize, usize, usize) {
    match list_atom(fe).and_then(|a| fe.list(a)) {
        Some(l) => {
            let top = l.upper.min(l.items.len());
            let rows = l.items[top..].iter().take(l.max_visible).cloned().collect();
            (rows, top, l.items.len(), l.max_visible)
        }
        None => (Vec::new(), 0, 0, 1),
    }
}

/// The module panel's rows (the Somtaaw mothership's modules), as many as it shows.
fn module_count(fe: &Fe) -> usize {
    let view = &fe.game.ig.view;
    match factory_in_slot(view, 0) {
        Some(ms) if ms.ship_type == SOMTAAW_MOTHERSHIP => view.modules.len().min(7),
        _ => 0,
    }
}

/// The Beast's emergency power radio buttons with their labels, top to bottom.
fn power_choices(fe: &Fe) -> Vec<(AtomRef, String)> {
    let view = &fe.game.ig.view;
    if !factory_in_slot(view, 0).is_some_and(|f| f.ship_type == BEAST_MOTHERSHIP) {
        return Vec::new();
    }
    let Some(screen) = fe.current_screen() else {
        return Vec::new();
    };
    let lang = fe.resources().language().index();
    let s = &fe.screens[screen];
    let mut out: Vec<(AtomRef, String, i32)> = fe
        .atoms_named(screen, "LM_BeastSystemsSet")
        .into_iter()
        .filter(|&a| fe.atom(a).region.is_some())
        .map(|a| {
            let r = fe.atom(a).layout_rect();
            let cy = (r.y0 + r.y1) / 2;
            // Its label is on its left, on its line.
            let label = s
                .atoms
                .iter()
                .filter(|t| t.kind == FA_STATIC_TEXT)
                .filter(|t| {
                    let tr = t.layout_rect();
                    tr.x1 <= r.x0 + 4 && ((tr.y0 + tr.y1) / 2 - cy).abs() <= 8
                })
                .max_by_key(|t| t.layout_rect().x1)
                .and_then(|t| t.texts.get(lang).or(t.text.as_ref()).cloned())
                .map(|b| {
                    b.iter()
                        .map(|&c| c as char)
                        .filter(|&c| c != '&')
                        .collect::<String>()
                        .trim()
                        .to_string()
                })
                .unwrap_or_default();
            (a, label, r.y0)
        })
        .collect();
    out.sort_by_key(|c| c.2);
    out.into_iter().map(|(a, l, _)| (a, l)).collect()
}

fn launcher(fe: &Fe) -> Option<&Launcher> {
    launchmgr::launcher(fe)
}

/// The layout for the screen as it is now.
pub fn layout_now(fe: &Fe) -> Layout {
    let (w, h) = fe.screen_size();
    let (_, _, _, page) = list_rows(fe);
    layout(w, h, page, module_count(fe), power_choices(fe).len())
}

// ---- hit-testing ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hit {
    Tab(usize),
    Stay(usize),
    Row(usize),
    Thumb,
    Track(bool),
    /// A module's repair-priority button: (module, priority).
    Priority(usize, usize),
    Power(usize),
    Button(Button),
    Panel,
    Outside,
}

pub fn hit(l: &Layout, x: i32, y: i32, rows: usize, total: usize, top: usize) -> Hit {
    for (b, r) in &l.buttons {
        if r.contains(x, y) {
            return Hit::Button(*b);
        }
    }
    if let Some(i) = l.tabs.iter().position(|r| r.contains(x, y)) {
        return Hit::Tab(i);
    }
    if let Some(i) = l.stay.iter().position(|r| r.contains(x, y)) {
        return Hit::Stay(i);
    }
    if let Some(i) = l.rows.iter().position(|r| r.contains(x, y)) {
        if i < rows {
            return Hit::Row(i);
        }
        return Hit::Panel;
    }
    let grip = Rect::new(
        l.scroll_track.x0 - l.px(6.0),
        l.scroll_track.y0,
        l.scroll_track.x1 + l.px(6.0),
        l.scroll_track.y1,
    );
    if grip.contains(x, y) {
        if let Some(th) = l.thumb(total, top) {
            if (th.y0..th.y1).contains(&y) {
                return Hit::Thumb;
            }
            return Hit::Track(y < th.y0);
        }
    }
    for (m, (_, segs)) in l.modules.iter().enumerate() {
        if let Some(p) = segs.iter().position(|r| r.contains(x, y)) {
            return Hit::Priority(m, p);
        }
    }
    if let Some(i) = l.power.iter().position(|r| r.contains(x, y)) {
        return Hit::Power(i);
    }
    if l.frame.panel.contains(x, y) {
        Hit::Panel
    } else {
        Hit::Outside
    }
}

fn region_rect(fe: &Fe, a: AtomRef) -> Option<Rect> {
    fe.region(fe.atom(a).region?).map(|r| r.rect)
}

fn named_rect(fe: &Fe, name: &str) -> Option<Rect> {
    let a = fe.find_atom(name)?;
    region_rect(fe, a)
}

/// The original region a hit stands for.
pub fn target(fe: &Fe, l: &Layout, at: Hit) -> Option<Target> {
    match at {
        Hit::Button(b) => Some(Target::new(l.button(b), named_rect(fe, b.atom())?)),
        Hit::Tab(i) => Some(Target::new(l.tabs[i], named_rect(fe, TAB_ATOMS[i])?)),
        Hit::Stay(i) => Some(Target::new(l.stay[i], named_rect(fe, STAY_ATOMS[i])?)),
        Hit::Row(k) => {
            let a = list_atom(fe)?;
            let o = region_rect(fe, a)?;
            let lw = fe.list(a)?;
            let y0 = o.y0 + lw.title_height + LIST_MARGIN + k as i32 * lw.item_height;
            let row = Rect::new(
                o.x0 + LIST_MARGIN,
                y0,
                o.x1 - LIST_MARGIN,
                y0 + lw.item_height,
            );
            Some(Target::new(l.rows[k], row))
        }
        Hit::Priority(m, p) => {
            // `lmSlotNProcess` picks the button by the pointer's place from the row's right.
            let o = named_rect(fe, &format!("LM_Module_Slot{}", m + 1))?;
            let x1 = o.x1 - 104 + p as i32 * 36;
            let r = Rect::new(x1 + 2, o.y0, x1 + 34, o.y1);
            Some(Target::new(l.modules[m].1[p], r))
        }
        Hit::Power(i) => {
            let (a, _) = power_choices(fe).into_iter().nth(i)?;
            Some(Target::new(l.power[i], region_rect(fe, a)?))
        }
        _ => None,
    }
}

// ---- state and input ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Key {
    Hit(Hit),
    On(usize),
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub mouse: (i32, i32),
    pressed: Option<Hit>,
    pub proxy: Proxy,
    /// Dragging the list's thumb: the pointer's offset from its top.
    drag: Option<i32>,
    hover: Hover<Key>,
}

/// Opening the launch manager: a fresh state.
pub fn reset(fe: &mut Fe) {
    let mouse = fe.game.ig.mlm.mouse;
    fe.game.ig.mlm = State {
        mouse,
        ..State::default()
    };
}

fn hit_now(fe: &Fe, l: &Layout) -> Hit {
    let (rows, top, total, _) = list_rows(fe);
    let (x, y) = fe.game.ig.mlm.mouse;
    hit(l, x, y, rows.len(), total, top)
}

fn scroll_to(fe: &mut Fe, top: usize) {
    let Some(a) = list_atom(fe) else { return };
    let Some(l) = fe.list_mut(a) else { return };
    l.upper = top.min(l.items.len().saturating_sub(l.max_visible));
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// The pointer moved.
pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    fe.game.ig.mlm.mouse = (x, y);
    let l = layout_now(fe);
    if let Some(grab) = fe.game.ig.mlm.drag {
        let (_, _, total, page) = list_rows(fe);
        let top = mm::top_for_thumb(l.scroll_track, page, total, y - grab, l.px(24.0));
        scroll_to(fe, top);
    }
    let at = hit_now(fe, &l);
    let t = target(fe, &l, at);
    let mut proxy = fe.game.ig.mlm.proxy;
    proxy.moved(fe, x, y, t);
    fe.game.ig.mlm.proxy = proxy;
}

/// A mouse button over the screen (the modern HUD's panels have had their turn).
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool) {
    let l = layout_now(fe);
    let at = hit_now(fe, &l);
    let pos = fe.game.ig.mlm.mouse;
    if left && down {
        if fe.game.ig.mlm.pressed.is_some() || fe.game.ig.mlm.proxy.holding() {
            return;
        }
        match at {
            Hit::Thumb => {
                let (_, top, total, _) = list_rows(fe);
                if let Some(th) = l.thumb(total, top) {
                    fe.game.ig.mlm.drag = Some(pos.1 - th.y0);
                    fe.game.ig.mlm.pressed = Some(at);
                }
                return;
            }
            Hit::Track(up) => {
                let (_, top, _, page) = list_rows(fe);
                let top = if up {
                    top.saturating_sub(page)
                } else {
                    top + page
                };
                scroll_to(fe, top);
                fe.ui_sound(sound::CLICK);
                return;
            }
            _ => {}
        }
    }
    if left && !down {
        fe.game.ig.mlm.drag = None;
        if fe.game.ig.mlm.pressed.take() == Some(Hit::Thumb) {
            return;
        }
    }
    let t = target(fe, &l, at);
    if left && down {
        fe.game.ig.mlm.pressed = t.map(|_| at);
    }
    let mut proxy = fe.game.ig.mlm.proxy;
    proxy.button(fe, left, down, pos, t);
    fe.game.ig.mlm.proxy = proxy;
}

/// The wheel over the list: the original list's own step.
pub fn wheel(fe: &mut Fe, up: bool) {
    let l = layout_now(fe);
    let (x, y) = fe.game.ig.mlm.mouse;
    let zone = Rect::new(
        l.list.x0,
        l.list.y0,
        l.scroll_track.x1 + l.px(6.0),
        l.list.y1,
    );
    if !zone.contains(x, y) {
        return;
    }
    let Some(a) = list_atom(fe) else { return };
    if let Some(lw) = fe.list_mut(a) {
        if up {
            lw.wheel_up();
        } else {
            lw.wheel_down();
        }
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// The panel (for the modern HUD's panels to keep clear of), when up.
pub fn area(fe: &Fe, width: i32, height: i32) -> Option<Rect> {
    active(fe).then(|| Frame::new(width, height).panel)
}

// ---- drawing ----

fn eased(fe: &Fe, k: Key) -> f32 {
    fe.game.ig.mlm.hover.get(k)
}

/// Whether the slot's "remain docked" box is checked.
fn stays(fe: &Fe, slot: usize) -> bool {
    fe.find_atom(STAY_ATOMS[slot])
        .is_some_and(|a| fe.atom(a).checked())
}

pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let l = layout_now(fe);
    let at = hit_now(fe, &l);
    let pressed = fe.game.ig.mlm.pressed;
    let mut targets = Vec::new();
    if !matches!(at, Hit::Panel | Hit::Outside) {
        let v = if pressed == Some(at) { 1.0 } else { 0.6 };
        targets.push((Key::Hit(at), v));
    }
    if fe.game.ig.mlm.drag.is_some() {
        targets.push((Key::Hit(Hit::Thumb), 1.0));
    }
    for slot in 0..3 {
        if stays(fe, slot) {
            targets.push((Key::On(slot), 1.0));
        }
    }
    let now = fe.time;
    fe.game.ig.mlm.hover.step(now, &targets);

    let f = &l.frame;
    mm::draw_frame(canvas, f);
    let name = launcher(fe).map(|l| l.factory.name.to_uppercase());
    let title = screen_title(fe);
    mm::draw_title(canvas, &pen, f, &title, name.as_deref());
    draw_tabs(fe, canvas, &pen, &l);
    draw_slots(fe, canvas, &pen, &l);
    draw_list(fe, canvas, &pen, &l);
    draw_stay(fe, canvas, &pen, &l);
    if !l.modules.is_empty() {
        draw_modules(fe, canvas, &pen, &l);
    } else if !l.power.is_empty() {
        draw_power(fe, canvas, &pen, &l);
    } else {
        section_heading(
            canvas,
            &pen,
            f,
            l.right.x0,
            l.right_heading,
            l.right.x1,
            "MODULE STATUS",
        );
        write(
            canvas,
            &pen,
            Face::Semibold,
            f.text(BODY_PX),
            0.0,
            l.right.x0 + f.px(2.0),
            l.right_heading + f.px(30.0),
            DIM,
            None,
            "NO MODULES",
        );
    }
    for (b, r) in &l.buttons {
        let focused = fe
            .find_atom(b.atom())
            .and_then(|a| fe.atom(a).region)
            .and_then(|id| fe.region(id))
            .is_some_and(|r| r.has(RSF_CURRENT_SELECTED));
        let hv = eased(fe, Key::Hit(Hit::Button(*b)));
        let idle = match b {
            Button::LaunchOne | Button::Launch => selected_row(fe).is_none(),
            Button::LaunchAll => launcher(fe).is_none_or(|l| l.docked.is_empty()),
            _ => false,
        };
        let look = ButtonLook {
            label: b.label(),
            key: b.key(),
            hover: hv,
            pressed: pressed == Some(Hit::Button(*b)) && hv > 0.5,
            primary: *b == Button::Launch && !idle,
            idle,
            focused,
            ..Default::default()
        };
        draw_button(canvas, &pen, f, *r, &look);
    }
}

/// The screen's own title (its static text), else SYSTEMS MANAGER.
fn screen_title(fe: &Fe) -> String {
    let lang = fe.resources().language().index();
    fe.current_screen()
        .and_then(|s| {
            fe.screens[s]
                .atoms
                .iter()
                .filter(|a| a.kind == FA_STATIC_TEXT && a.layout_rect().height() >= 18)
                .min_by_key(|a| a.layout_rect().y0)
                .and_then(|a| a.texts.get(lang).or(a.text.as_ref()).cloned())
        })
        .map(|b| b.iter().map(|&c| c as char).collect::<String>())
        .map(|s| s.trim().trim_end_matches(':').to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "SYSTEMS MANAGER".to_string())
}

/// The selected ship row's type, if a ship row is selected.
fn selected_row(fe: &Fe) -> Option<u32> {
    let l = fe.list(list_atom(fe)?)?;
    let it = l.selected.and_then(|i| l.items.get(i))?;
    it.can_select.then_some(it.data)
}

fn draw_tabs(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let view = &fe.game.ig.view;
    for (slot, &r) in l.tabs.iter().enumerate() {
        let f = factory_in_slot(view, slot);
        let current = f.is_some_and(|f| Some(f.ship) == fe.game.ig.lm.launcher);
        let docked = f
            .and_then(|f| view.launchers.iter().find(|l| l.factory.ship == f.ship))
            .map_or(0, |l| l.docked.len());
        let label = match f {
            Some(f) => f.name.to_uppercase(),
            None if slot == 0 => "NO COMMAND SHIP".to_string(),
            None => format!("CARRIER {slot} \u{2014} NONE"),
        };
        let look = TabLook {
            label,
            enabled: f.is_some(),
            current,
            hover: eased(fe, Key::Hit(Hit::Tab(slot))),
            count: (f.is_some()).then(|| docked.to_string()),
            progress: None,
            paused: false,
            detail: None,
        };
        mm::draw_tab(canvas, pen, &l.frame, r, &look);
    }
}

/// Fighter and corvette slots: used of available.
fn draw_slots(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let (fighters, corvettes) =
        launcher(fe).map_or(((0, 0), (0, 0)), |l| (l.fighters, l.corvettes));
    for (r, label, (used, cap)) in [
        (l.slots[0], "FIGHTER SLOTS", fighters),
        (l.slots[1], "CORVETTE SLOTS", corvettes),
    ] {
        let full = cap > 0 && used >= cap;
        let col = if full { modern_hud::AT_CAP } else { WHITE };
        let b = mm::readout(
            canvas,
            pen,
            f,
            r,
            label,
            &format!("{} / {}", used.min(cap.max(0)), cap),
            col,
            value_glow(f),
        );
        let fr = if cap > 0 {
            (used as f32 / cap as f32).clamp(0.0, 1.0)
        } else {
            0.0
        };
        bar(canvas, f, b, fr, 1.0);
    }
}

fn draw_list(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let (rows, top, total, _) = list_rows(fe);
    section_heading(
        canvas,
        pen,
        f,
        l.list.x0 + f.px(2.0),
        f.panel.y0 + f.px(LIST_HEAD_BASELINE),
        l.list.x1,
        "DOCKED SHIPS",
    );
    well(canvas, f, l.list);
    let npx = f.text(NAME_PX);
    for (k, (row, it)) in l.rows.iter().zip(&rows).enumerate() {
        if !it.can_select {
            list_heading(
                canvas,
                pen,
                f,
                row.x0 + f.px(4.0),
                *row,
                row.x1 - f.px(8.0),
                &it.text,
            );
            continue;
        }
        let hv = eased(fe, Key::Hit(Hit::Row(k)));
        row_band(canvas, f, *row, it.selected, hv, 0.0);
        let (count, name) = it.text.split_once('\t').unwrap_or(("", &it.text));
        let col = if it.selected {
            lerp(LABEL, WHITE, hv)
        } else {
            lerp(TEXT, WHITE, hv)
        };
        let base = mid_baseline(*row, npx);
        write(
            canvas,
            pen,
            Face::Semibold,
            npx,
            0.0,
            row.x0 + f.px(22.0),
            base,
            col,
            None,
            &name.to_uppercase(),
        );
        write_right(
            canvas,
            pen,
            Face::Bold,
            f.text(VALUE_PX),
            0.0,
            row.x1 - f.px(12.0),
            base,
            WHITE,
            value_glow(f),
            &format!("\u{d7} {count}"),
        );
    }
    if rows.is_empty() {
        write(
            canvas,
            pen,
            Face::Semibold,
            f.text(BODY_PX),
            0.0,
            l.list.x0 + f.px(16.0),
            l.list.y0 + f.px(28.0),
            DIM,
            None,
            "NO SHIPS DOCKED",
        );
    }
    if let Some(th) = l.thumb(total, top) {
        draw_scroll(canvas, l.scroll_track, th, eased(fe, Key::Hit(Hit::Thumb)));
    }
}

/// REMAIN DOCKED: the auto-launch check boxes (checked: ships stay docked).
fn draw_stay(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let view = &fe.game.ig.view;
    section_heading(
        canvas,
        pen,
        f,
        l.stay[0].x0 + f.px(2.0),
        l.stay_heading,
        l.stay[0].x1,
        &stay_heading(fe),
    );
    for (slot, &row) in l.stay.iter().enumerate() {
        let fac = factory_in_slot(view, slot);
        let disabled = fe
            .find_atom(STAY_ATOMS[slot])
            .and_then(|a| fe.atom(a).region)
            .and_then(|id| fe.region(id))
            .is_none_or(|r| r.has(RSF_REGION_DISABLED));
        let hv = if disabled {
            0.0
        } else {
            eased(fe, Key::Hit(Hit::Stay(slot)))
        };
        row_band(canvas, f, row, false, hv * 0.8, 0.0);
        let label = match fac {
            Some(fac) => fac.name.to_uppercase(),
            None if slot == 0 => "NO COMMAND SHIP".into(),
            None => format!("CARRIER {slot}"),
        };
        let px = f.text(BODY_PX + 1.0);
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.4 * f.scale,
            row.x0 + f.px(12.0),
            mid_baseline(row, px),
            if disabled {
                GREYED
            } else {
                lerp(TEXT, WHITE, hv)
            },
            None,
            &label,
        );
        let sw = Rect::new(
            row.x1 - f.px(56.0),
            (row.y0 + row.y1) / 2 - f.px(12.0),
            row.x1 - f.px(10.0),
            (row.y0 + row.y1) / 2 + f.px(12.0),
        );
        draw_switch(canvas, f, sw, eased(fe, Key::On(slot)), hv, disabled);
    }
}

/// The heading over the auto-launch boxes: the screen's own words ("REMAIN DOCKED").
fn stay_heading(fe: &Fe) -> String {
    let lang = fe.resources().language().index();
    let Some(s) = fe.current_screen() else {
        return "REMAIN DOCKED".into();
    };
    let words: Vec<String> = fe.screens[s]
        .atoms
        .iter()
        .filter(|a| a.kind == FA_STATIC_TEXT)
        .filter(|a| {
            let r = a.layout_rect();
            r.x0 >= 240 && r.x0 <= 260 && (150..=180).contains(&r.y0)
        })
        .filter_map(|a| a.texts.get(lang).or(a.text.as_ref()).cloned())
        .map(|b| {
            b.iter()
                .map(|&c| c as char)
                .collect::<String>()
                .trim()
                .to_string()
        })
        .collect();
    if words.is_empty() {
        "REMAIN DOCKED".into()
    } else {
        words.join(" ")
    }
}

/// The Somtaaw module panel: each module's health and its repair priority (1, 2, 3).
fn draw_modules(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let view = &fe.game.ig.view;
    let rp = pen.advance(
        Face::Bold,
        f.text(SMALL_PX),
        SMALL_SPACING * f.scale,
        "REPAIR PRIORITY",
    );
    section_heading(
        canvas,
        pen,
        f,
        l.right.x0,
        l.right_heading,
        l.right.x1 - f.px(8.0) - rp - f.px(12.0),
        "MODULE STATUS",
    );
    write_right(
        canvas,
        pen,
        Face::Bold,
        f.text(SMALL_PX),
        SMALL_SPACING * f.scale,
        l.right.x1 - f.px(8.0),
        l.right_heading,
        LABEL,
        None,
        "REPAIR PRIORITY",
    );
    let labels = ["1".to_string(), "2".to_string(), "3".to_string()];
    for (m, (row, segs)) in l.modules.iter().enumerate() {
        let Some(module) = view.modules.get(m) else {
            continue;
        };
        mm::section(canvas, f, *row);
        let npx = f.text(BODY_PX + 1.0);
        let name_w = (segs[0].x0 - row.x0 - f.px(24.0)) as f32;
        let name = fit(
            pen,
            Face::Semibold,
            npx,
            0.0,
            &module.name.to_uppercase(),
            name_w,
        );
        write(
            canvas,
            pen,
            Face::Semibold,
            npx,
            0.0,
            row.x0 + f.px(12.0),
            row.y0 + f.px(23.0),
            TEXT,
            None,
            &name,
        );
        let h = module.health.clamp(0.0, 1.0);
        let bar_r = Rect::new(
            row.x0 + f.px(12.0),
            row.y1 - f.px(16.0),
            segs[0].x0 - f.px(60.0),
            row.y1 - f.px(16.0) + f.px(4.0).max(2),
        );
        let (fill, _) = launchmgr::health_colours(h);
        canvas.blend_rect(bar_r, modern_hud::TRACK);
        let w = (bar_r.width() as f32 * h) as i32;
        rounded(
            canvas,
            Rect::new(bar_r.x0, bar_r.y0, bar_r.x0 + w, bar_r.y1),
            1.5,
            alpha(fill, 0.95),
        );
        write_right(
            canvas,
            pen,
            Face::Bold,
            f.text(SMALL_PX + 1.0),
            0.0,
            segs[0].x0 - f.px(14.0),
            bar_r.y1 + f.px(1.0),
            if h > launchmgr::HEALTH_GREEN {
                modern_research::COMPLETE_COL
            } else {
                WHITE
            },
            None,
            &format!("{}%", (h * 100.0).round() as i32),
        );
        let seg_box = Rect::new(segs[0].x0, segs[0].y0, segs[2].x1, segs[0].y1);
        let look = ChoiceLook {
            labels: &labels,
            current: module.repair_priority.map(|p| p.clamp(0, 2) as usize),
            hover: (0..3)
                .map(|p| eased(fe, Key::Hit(Hit::Priority(m, p))))
                .collect(),
            pressed: (0..3).find(|&p| fe.game.ig.mlm.pressed == Some(Hit::Priority(m, p))),
            disabled: module.repair_priority.is_none(),
        };
        draw_segments(canvas, pen, f, seg_box, &look);
    }
}

/// The Beast mothership's emergency power allocation: one choice of its systems.
fn draw_power(fe: &Fe, canvas: &mut Canvas, pen: &Pen, l: &Layout) {
    let f = &l.frame;
    let choices = power_choices(fe);
    let heading = power_heading(fe);
    section_heading(
        canvas,
        pen,
        f,
        l.right.x0,
        l.right_heading,
        l.right.x1,
        &heading,
    );
    for (i, (row, (a, label))) in l.power.iter().zip(&choices).enumerate() {
        let on = fe.atom(*a).checked();
        let disabled = fe
            .atom(*a)
            .region
            .and_then(|id| fe.region(id))
            .is_none_or(|r| r.has(RSF_REGION_DISABLED));
        let hv = if disabled {
            0.0
        } else {
            eased(fe, Key::Hit(Hit::Power(i)))
        };
        let band = Rect::new(row.x0, row.y0 + 3, row.x1, row.y1 - 3);
        row_band(canvas, f, band, on, hv, 0.0);
        let px = f.text(BODY_PX + 1.0);
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.4 * f.scale,
            row.x0 + f.px(14.0),
            mid_baseline(*row, px),
            if disabled {
                GREYED
            } else if on {
                lerp(LABEL, WHITE, hv)
            } else {
                lerp(TEXT, WHITE, hv)
            },
            None,
            label,
        );
        // A radio mark on the right.
        let cx = (row.x1 - f.px(22.0)) as f32;
        let cy = (row.y0 + row.y1) as f32 / 2.0;
        let r = f.px(8.0).max(4) as f32;
        circle(
            canvas,
            cx,
            cy,
            r,
            lerp([255, 255, 255, 70], GOLD, if on { 1.0 } else { hv }),
        );
        circle(canvas, cx, cy, r - 1.5, [14, 16, 22, 255]);
        if on {
            halo(canvas, cx, cy, r * 2.0, GOLD, 0.3);
            circle(canvas, cx, cy, r * 0.5, GOLD);
        }
    }
}

fn power_heading(fe: &Fe) -> String {
    let lang = fe.resources().language().index();
    fe.current_screen()
        .and_then(|s| {
            fe.screens[s]
                .atoms
                .iter()
                .filter(|a| a.kind == FA_STATIC_TEXT)
                .find(|a| {
                    let r = a.layout_rect();
                    r.x0 > 340 && (120..=140).contains(&r.y0)
                })
                .and_then(|a| a.texts.get(lang).or(a.text.as_ref()).cloned())
        })
        .map(|b| b.iter().map(|&c| c as char).collect::<String>())
        .map(|s| s.trim().trim_end_matches(':').to_string())
        .unwrap_or_else(|| "EMERGENCY POWER ALLOCATION".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inside(outer: Rect, r: Rect) -> bool {
        r.x0 >= outer.x0 && r.y0 >= outer.y0 && r.x1 <= outer.x1 && r.y1 <= outer.y1
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
    }

    #[test]
    fn everything_sits_inside_the_panel_without_overlaps() {
        for (w, h) in [(1920, 1080), (1024, 768), (640, 480), (3840, 2160)] {
            for (modules, power) in [(7, 0), (0, 6), (0, 0)] {
                let l = layout(w, h, 11, modules, power);
                let p = l.frame.panel;
                let mut all: Vec<Rect> = l.tabs.to_vec();
                all.extend(l.slots);
                all.push(l.list);
                all.extend(l.stay);
                all.extend(l.modules.iter().map(|m| m.0));
                all.extend(l.power.iter().copied());
                all.extend(l.buttons.iter().map(|b| b.1));
                for (i, a) in all.iter().enumerate() {
                    assert!(inside(p, *a), "{w}x{h}: {a:?} outside {p:?}");
                    for b in all.iter().skip(i + 1) {
                        assert!(!overlap(*a, *b), "{w}x{h}: {a:?} and {b:?}");
                    }
                }
                assert!(l.stay[2].y1 <= l.frame.footer_line, "{w}x{h}");
                assert!(l.list.y1 < l.stay[0].y0);
                for (row, segs) in &l.modules {
                    for s in segs {
                        assert!(inside(*row, *s));
                    }
                }
            }
        }
    }

    #[test]
    fn same_frame_and_header_buttons_as_the_other_managers() {
        let l = layout(1920, 1080, 11, 7, 0);
        assert_eq!(l.frame, Frame::new(1920, 1080));
        assert_eq!(l.button(Button::Research), l.frame.top_button(0));
        assert_eq!(l.button(Button::Build), l.frame.top_button(1));
        assert_eq!(l.button(Button::Close), l.frame.footer_right());
    }

    #[test]
    fn hit_testing() {
        let l = layout(1920, 1080, 11, 7, 0);
        let c = |r: Rect| ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
        let at = |r: Rect| {
            let (x, y) = c(r);
            hit(&l, x, y, 5, 30, 0)
        };
        assert_eq!(at(l.tabs[1]), Hit::Tab(1));
        assert_eq!(at(l.stay[2]), Hit::Stay(2));
        assert_eq!(at(l.rows[4]), Hit::Row(4));
        assert_eq!(at(l.rows[7]), Hit::Panel, "no row there");
        assert_eq!(at(l.modules[3].1[2]), Hit::Priority(3, 2));
        assert_eq!(
            at(l.button(Button::LaunchAll)),
            Hit::Button(Button::LaunchAll)
        );
        let th = l.thumb(30, 0).unwrap();
        assert_eq!(hit(&l, th.x0 + 1, th.y0 + 2, 5, 30, 0), Hit::Thumb);
        assert_eq!(hit(&l, th.x0 + 1, th.y1 + 8, 5, 30, 0), Hit::Track(false));
        assert_eq!(hit(&l, 2, 2, 5, 30, 0), Hit::Outside);
        let b = layout(1920, 1080, 11, 0, 6);
        let (x, y) = c(b.power[5]);
        assert_eq!(hit(&b, x, y, 5, 30, 0), Hit::Power(5));
    }

    #[test]
    fn button_atoms_are_the_screens() {
        assert_eq!(Button::Launch.atom(), "LM_Launch");
        assert_eq!(Button::Close.key(), "C");
    }
}
