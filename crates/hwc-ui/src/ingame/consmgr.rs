//! The construction manager ("BUILD MANAGER", `ConsMgr.c`): the list of ships the selected
//! factory can build with the counts to build, queue and cost columns and progress bars, the
//! increment/decrement arrows, Build / Pause batch / Cancel batch, the mothership and two
//! carrier factory icons, the support-units bar, the totals, and the ship view.
//!
//! As in the engine, counts picked in the list stay in the manager until BUILD sends them to the
//! factory; adding to a type the factory is already building goes straight into its queue.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use crate::canvas::{Color, Image, Rect, WHITE};
use crate::consts::*;
use crate::fe::Call;
use crate::fer::Fer;
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::{close_manager, open_manager, Fe, Manager};
use crate::region::{AtomRef, RegionId};

/// Rows the list shows (`0x7499d0`): the scroll bar's page.
pub const VISIBLE_ROWS: usize = 18;
/// Seconds a refused row flashes red.
pub const FLASH_SECONDS: f64 = 0.3;
/// Arrow auto-repeat interval (`MG_WAIT_ACC`).
pub const REPEAT_SECONDS: f64 = 0.25;

pub const FONT_NUMBERS: &str = "default.hff";
pub const FONT_NAMES: &str = "HW_EuroseCond_11.hff";
pub const FONT_FACTORY: &str = "EuroseWideHeavy_i13.hff";

const BACKGROUND: Color = [0, 35, 79, 255];
const DIVIDER: Color = [0, 100, 160, 255];
const PENDING_TEXT: Color = [255, 255, 105, 255];
const BUILDING_TEXT: Color = [0, 255, 0, 255];
const PAUSED_TEXT: Color = [80, 120, 80, 255];
const HEADING_TEXT: Color = [255, 200, 0, 255];
const GREYED_TEXT: Color = [128, 128, 128, 255];
const INVALID_TEXT: Color = [255, 25, 25, 255];
const SHIP_BAR: Color = [145, 168, 249, 255];
const BATCH_BAR: Color = [37, 83, 239, 255];
const ARROW_OUTLINE: Color = [200, 200, 0, 255];
const ARROW_FIRED: Color = [200, 0, 0, 255];

/// Speech events.
pub const SPEECH_BUILD: u32 = 0x400_1035;
pub const SPEECH_BUILD_CAPITAL: u32 = 0x400_103e;
pub const SPEECH_PAUSE: u32 = 0x400_1036;
pub const SPEECH_RESUME: u32 = 0x400_103c;
pub const SPEECH_CANCEL: u32 = 0x400_103b;
pub const SPEECH_FACTORY: u32 = 0x400_101c;
pub const SPEECH_TOTAL_CAP: u32 = 0x200_1039;
pub const SPEECH_TYPE_CAP: u32 = 0x200_1038;
/// UI sounds when a manager opens (`UI_ManagerIntro`, `UI_BuildManager`) and closes
/// (`UI_ManagerExit`, `UI_SoundOfSpace`).
pub const SOUND_MANAGER_INTRO: u32 = 0x12f;
pub const SOUND_BUILD_MANAGER: u32 = 0x104;
pub const SOUND_MANAGER_EXIT: u32 = 0x130;
pub const SOUND_SPACE: u32 = 0x100;

#[derive(Debug, Clone, Default)]
pub struct State {
    /// The factory whose list is shown.
    pub factory: Option<ShipId>,
    /// Last factory slot used (0 mothership, 1-2 carriers), kept between openings.
    pub last_slot: usize,
    /// Ships picked in the list but not yet sent to the factory (`shipavailable.nJobs`).
    pub pending: HashMap<ShipType, i32>,
    /// Selected rows of the current factory (`shipinprogress.selected`).
    pub selected: BTreeSet<ShipType>,
    /// First visible row.
    pub top_row: usize,
    scroll_list: Option<RegionId>,
    /// Last refused row and when (red flash).
    pub flash: Option<(ShipType, f64)>,
    /// An arrow fired this frame: (ship type, right arrow).
    pub arrow_fired: Option<(ShipType, bool)>,
    /// Ship type under the held arrow.
    arrow_row: Option<ShipType>,
    acc_time: f64,
    /// `CM_ExtendedInfo`: the ship view shows the description.
    pub extended_info: bool,
    /// Ship type in the ship view (`svSelectShip`).
    pub ship_view: Option<ShipType>,
    /// Picture of the ship view supplied by the application.
    pub ship_view_image: Option<Rc<Image>>,
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("CM_Close", |fe, call| {
        if guard(fe, call) {
            close(fe);
        }
    });
    fe.callback_add("CM_BuildShips", |fe, call| {
        if guard(fe, call) {
            build_ships(fe);
        }
    });
    fe.callback_add("CM_PauseJobs", |fe, call| {
        if guard(fe, call) {
            pause_jobs(fe);
        }
    });
    fe.callback_add("CM_CancelJobs", |fe, call| {
        if guard(fe, call) {
            cancel_jobs(fe);
        }
    });
    fe.callback_add("CM_Research", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Research);
        }
    });
    fe.callback_add("CM_Launch", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Launch);
        }
    });
    fe.callback_add("CM_ExtendedInfo", |fe, call| {
        if guard(fe, call) {
            fe.game.ig.cm.extended_info = !fe.game.ig.cm.extended_info;
        }
    });
    fe.callback_add("CM_Scroller", |_, _| {});
    fe.draw_callback_add("CM_Ships", draw_ships);
    fe.draw_callback_add("CM_ShipNumbers", draw_numbers);
    fe.draw_callback_add("CM_ShipCost", draw_costs);
    fe.draw_callback_add("CM_LeftArrowDraw", |fe, fer, a, r| {
        draw_arrows(fe, fer, a, r, false)
    });
    fe.draw_callback_add("CM_RightArrowDraw", |fe, fer, a, r| {
        draw_arrows(fe, fer, a, r, true)
    });
    fe.draw_callback_add("CM_RUs", |fe, fer, a, r| {
        let v = fe.game.ig.view.player.resource_units;
        draw_number_box(fe, fer, a, r, v)
    });
    fe.draw_callback_add("CM_TotalCost", |fe, fer, a, r| {
        let v = total_cost(fe);
        draw_number_box(fe, fer, a, r, v)
    });
    fe.draw_callback_add("CM_TotalShips", |fe, fer, a, r| {
        let v = total_ships(fe);
        draw_number_box(fe, fer, a, r, v)
    });
    fe.draw_callback_add("CM_MotherShipDraw", |fe, fer, a, r| {
        draw_factory(fe, fer, a, r, 0)
    });
    fe.draw_callback_add("CM_Carrier1Draw", |fe, fer, a, r| {
        draw_factory(fe, fer, a, r, 1)
    });
    fe.draw_callback_add("CM_Carrier2Draw", |fe, fer, a, r| {
        draw_factory(fe, fer, a, r, 2)
    });
    fe.draw_callback_add("CM_SUBarDraw", draw_support_bar);
    fe.draw_callback_add("CM_FactoryNameDraw", draw_factory_name);
    fe.draw_callback_add("SV_ShipView", draw_ship_view);
    fe.region_function_add("cmSelectAvailable", select_available);
    fe.region_function_add("cmLeftArrowProcess", |fe, id, ev| {
        arrow_process(fe, id, ev, false)
    });
    fe.region_function_add("cmRightArrowProcess", |fe, id, ev| {
        arrow_process(fe, id, ev, true)
    });
    fe.region_function_add("cmSelectMotherShip", |fe, _, ev| select_factory(fe, ev, 0));
    fe.region_function_add("cmSelectCarrier1", |fe, _, ev| select_factory(fe, ev, 1));
    fe.region_function_add("cmSelectCarrier2", |fe, _, ev| select_factory(fe, ev, 2));
}

/// Button callbacks act on the click only (not on screen creation or deletion).
fn guard(fe: &Fe, call: &Call) -> bool {
    !(fe.first_call(call) || fe.last_call(call))
}

// -------------------------------------------------------------------------------------------
// Factories
// -------------------------------------------------------------------------------------------

/// The factory in a slot: 0 the mothership, 1-2 the player's first two carriers.
pub fn factory_in_slot(view: &HostView, slot: usize) -> Option<&Factory> {
    if slot == 0 {
        view.factories.iter().find(|f| f.mothership)
    } else {
        view.factories
            .iter()
            .filter(|f| !f.mothership)
            .nth(slot - 1)
    }
}

fn slot_of(view: &HostView, ship: ShipId) -> Option<usize> {
    (0..3).find(|&s| factory_in_slot(view, s).is_some_and(|f| f.ship == ship))
}

/// `cmConstructionBegin`: picks the factory (a selected mothership/carrier, else the last
/// slot used, else the mothership) and resets the list.
pub fn begin(fe: &mut Fe) {
    let view = &fe.game.ig.view;
    let chosen = match view.selection.as_slice() {
        [one] if view.factories.iter().any(|f| f.ship == one.ship) => Some(one.ship),
        _ => None,
    }
    .or_else(|| factory_in_slot(view, fe.game.ig.cm.last_slot).map(|f| f.ship))
    .or_else(|| factory_in_slot(view, 0).map(|f| f.ship))
    .or_else(|| view.factories.first().map(|f| f.ship));
    let cm = &mut fe.game.ig.cm;
    cm.factory = chosen;
    cm.pending.clear();
    cm.selected.clear();
    cm.top_row = 0;
    cm.flash = None;
    cm.arrow_fired = None;
    cm.arrow_row = None;
    cm.ship_view = None;
    fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_INTRO));
    fe.game.ig.request(GameRequest::Sound(SOUND_BUILD_MANAGER));
}

/// After the screen exists: region handlers, the scroll bar.
pub fn started(fe: &mut Fe) {
    let list = PRESS_RIGHT_ETC;
    fe.region_function_set("CM_Ships", "cmSelectAvailable", list);
    let arrows = RPE_WHEEL_UP | RPE_WHEEL_DOWN | RPE_HOLD_LEFT | RPE_PRESS_LEFT | RPE_RELEASE_LEFT;
    for (atom, f) in [
        ("CM_LeftArrowDraw", "cmLeftArrowProcess"),
        ("CM_RightArrowDraw", "cmRightArrowProcess"),
    ] {
        if let Some(id) = fe.region_function_set(atom, f, arrows) {
            fe.regions.move_to_front(id);
        }
    }
    fe.region_function_set("CM_MotherShipDraw", "cmSelectMotherShip", RPE_PRESS_LEFT);
    fe.region_function_set("CM_Carrier1Draw", "cmSelectCarrier1", RPE_PRESS_LEFT);
    fe.region_function_set("CM_Carrier2Draw", "cmSelectCarrier2", RPE_PRESS_LEFT);
    fe.game.ig.cm.scroll_list = fe.scrollbar_virtual("CM_Scroller", VISIBLE_ROWS);
    sync_scroll(fe, Some(0));
}

/// Mouse events of the ship list (`cmShipInfoDraw` sets them on first draw).
const PRESS_RIGHT_ETC: u32 = RPE_PRESS_LEFT
    | RPE_PRESS_RIGHT
    | RPE_WHEEL_UP
    | RPE_WHEEL_DOWN
    | RPE_HOLD_LEFT
    | RPE_HOLD_RIGHT
    | RPE_DOUBLE_LEFT
    | RPE_DOUBLE_RIGHT;

pub fn tick(fe: &mut Fe, _dt: f32) {
    if fe.game.ig.manager != Some(Manager::Construction) {
        return;
    }
    // The factory went away (destroyed): back to the mothership.
    let view = &fe.game.ig.view;
    if let Some(f) = fe.game.ig.cm.factory {
        if !view.factories.iter().any(|x| x.ship == f) {
            let ms = factory_in_slot(view, 0).map(|f| f.ship);
            let cm = &mut fe.game.ig.cm;
            cm.factory = ms;
            cm.pending.clear();
            cm.selected.clear();
        }
    }
    sync_scroll(fe, None);
}

/// Keeps the scroll bar and the first visible row in step.
fn sync_scroll(fe: &mut Fe, upper: Option<usize>) {
    let total = rows(&fe.game.ig.view).len();
    if let Some(list) = fe.game.ig.cm.scroll_list {
        let top = fe.virtual_scroll(list, total, upper);
        fe.game.ig.cm.top_row = top;
    }
}

fn scroll_by(fe: &mut Fe, delta: isize) {
    let total = rows(&fe.game.ig.view).len();
    let top = fe.game.ig.cm.top_row as isize + delta;
    let max = total.saturating_sub(VISIBLE_ROWS) as isize;
    let top = top.clamp(0, max.max(0)) as usize;
    fe.game.ig.cm.top_row = top;
    sync_scroll(fe, Some(top));
}

/// Scrolls the list so `top` is its first row, `page` rows on screen (the modern build
/// manager shows a different number of rows than the original's 18).
pub(crate) fn scroll_to(fe: &mut Fe, top: usize, page: usize) {
    if let Some(r) = fe
        .game
        .ig
        .cm
        .scroll_list
        .and_then(|id| fe.region_mut(id))
    {
        if let crate::controls::Control::List(l) = &mut r.control {
            l.max_visible = page.max(1);
        }
    }
    let total = rows(&fe.game.ig.view).len();
    let top = top.min(total.saturating_sub(page.max(1)));
    fe.game.ig.cm.top_row = top;
    sync_scroll(fe, Some(top));
}

/// `cmClose`.
pub fn close(fe: &mut Fe) {
    let slot = fe
        .game
        .ig
        .cm
        .factory
        .and_then(|f| slot_of(&fe.game.ig.view, f))
        .unwrap_or(0);
    fe.game.ig.cm.last_slot = slot;
    fe.game.ig.cm.pending.clear();
    fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_EXIT));
    fe.game.ig.request(GameRequest::Sound(SOUND_SPACE));
    close_manager(fe);
}

/// Clicking a factory icon (`cmSelectMotherShip`, `cmSelectCarrier1/2`).
pub(crate) fn select_factory(fe: &mut Fe, ev: u32, slot: usize) {
    if ev != RPE_PRESS_LEFT {
        return;
    }
    let Some(f) = factory_in_slot(&fe.game.ig.view, slot).cloned() else {
        return;
    };
    if fe.game.ig.cm.factory == Some(f.ship) {
        return;
    }
    let cm = &mut fe.game.ig.cm;
    cm.factory = Some(f.ship);
    cm.pending.clear();
    cm.selected.clear();
    fe.game.ig.request(GameRequest::Speech {
        event: SPEECH_FACTORY,
        param: Some(f.ship_type),
    });
}

// -------------------------------------------------------------------------------------------
// The list
// -------------------------------------------------------------------------------------------

/// A visible row of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Heading(String),
    Ship(BuildOption),
}

/// Visible rows: ships not hidden, and the headings with a visible ship below them.
pub fn rows(view: &HostView) -> Vec<Row> {
    let list = &view.build_list;
    let mut out = Vec::new();
    for (i, r) in list.iter().enumerate() {
        match r {
            BuildRow::Class(h) => {
                let shown = list[i + 1..]
                    .iter()
                    .take_while(|r| matches!(r, BuildRow::Ship(_)))
                    .any(|r| matches!(r, BuildRow::Ship(o) if o.status != BuildStatus::Hidden));
                if shown {
                    out.push(Row::Heading(h.clone()));
                }
            }
            BuildRow::Ship(o) if o.status != BuildStatus::Hidden => out.push(Row::Ship(o.clone())),
            _ => {}
        }
    }
    out
}

fn row_height(fe: &Fe, font: &str) -> i32 {
    fe.resources()
        .font(font)
        .map_or(14, |f| crate::text::height(&f) + 2)
}

/// Rows on screen with their y: (row, y).
fn visible(fe: &Fe, rect: Rect, font: &str) -> Vec<(Row, i32)> {
    let h = row_height(fe, font);
    let fh = h - 2;
    let mut y = rect.y0 + 2;
    let mut out = Vec::new();
    for r in rows(&fe.game.ig.view)
        .into_iter()
        .skip(fe.game.ig.cm.top_row)
    {
        if y + fh >= rect.y1 {
            break;
        }
        out.push((r, y));
        y += h;
    }
    out
}

/// `cmSelectShipType`: the ship row at screen y (headings give none).
fn ship_at(fe: &Fe, rect: Rect, y: i32) -> Option<BuildOption> {
    let h = row_height(fe, FONT_NUMBERS);
    visible(fe, rect, FONT_NUMBERS)
        .into_iter()
        .find(|(_, ry)| y < ry + h)
        .and_then(|(r, _)| match r {
            Row::Ship(o) => Some(o),
            Row::Heading(_) => None,
        })
}

pub(crate) fn job(fe: &Fe, ship_type: ShipType) -> Option<&BuildJob> {
    let f = fe.game.ig.cm.factory?;
    fe.game.ig.view.job(f, ship_type)
}

pub(crate) fn pending(fe: &Fe, ship_type: ShipType) -> i32 {
    fe.game.ig.cm.pending.get(&ship_type).copied().unwrap_or(0)
}

/// Support units the manager's own counts need.
pub(crate) fn gui_support_units(fe: &Fe) -> i32 {
    rows(&fe.game.ig.view)
        .iter()
        .filter_map(|r| match r {
            Row::Ship(o) => Some(pending(fe, o.ship_type) * o.support_units),
            _ => None,
        })
        .sum()
}

/// `cmIncrement`: one more ship of a type; false (and a red flash) when refused.
pub fn increment(fe: &mut Fe, opt: &BuildOption, live: &mut i32) -> bool {
    let now = fe.time;
    let gui = pending(fe, opt.ship_type);
    if let Some(left) = opt.cap_left {
        if gui + *live >= left {
            let event = if opt.total_cap {
                SPEECH_TOTAL_CAP
            } else {
                SPEECH_TYPE_CAP
            };
            let param = (!opt.total_cap).then_some(opt.ship_type);
            fe.game.ig.request(GameRequest::Speech { event, param });
            fe.game.ig.cm.flash = Some((opt.ship_type, now));
            return false;
        }
    }
    if opt.support_units > 0 {
        let p = &fe.game.ig.view.player;
        let need = p.support_pending + gui_support_units(fe) + *live * opt.support_units;
        let free = p.support_capacity - p.support_used;
        if p.support_used >= p.support_capacity || free < need + opt.support_units {
            fe.game.ig.cm.flash = Some((opt.ship_type, now));
            return false;
        }
    }
    if opt.status == BuildStatus::Greyed || fe.game.ig.cm.factory.is_none() {
        fe.game.ig.cm.flash = Some((opt.ship_type, now));
        return false;
    }
    let in_production = job(fe, opt.ship_type).is_some_and(|j| j.batch_total >= 1);
    if in_production {
        *live += 1;
    } else {
        *fe.game.ig.cm.pending.entry(opt.ship_type).or_insert(0) += 1;
    }
    fe.game.ig.cm.ship_view = Some(opt.ship_type);
    true
}

/// `cmDecrement`: one ship less: from the manager's count, else from the factory's queue
/// (never the ship in progress).
pub fn decrement(fe: &mut Fe, opt: &BuildOption, live: &mut i32) {
    let gui = pending(fe, opt.ship_type);
    if gui > 0 {
        fe.game.ig.cm.pending.insert(opt.ship_type, gui - 1);
    } else if job(fe, opt.ship_type).is_some_and(|j| j.count + *live > 1) {
        *live -= 1;
    }
    fe.game.ig.cm.ship_view = Some(opt.ship_type);
}

/// Sends what `increment`/`decrement` changed in the factory's live queue.
fn flush_live(fe: &mut Fe, ship_type: ShipType, live: i32) {
    let Some(factory) = fe.game.ig.cm.factory else {
        return;
    };
    if live > 0 {
        fe.game.ig.request(GameRequest::Build {
            factory,
            ship_type,
            count: live,
        });
    } else if live < 0 {
        fe.game.ig.request(GameRequest::Unqueue {
            factory,
            ship_type,
            count: -live,
        });
    }
}

pub(crate) fn ctrl(fe: &Fe) -> bool {
    fe.input.down.contains(&key::CONTROL)
}

fn shift(fe: &Fe) -> bool {
    fe.input.down.contains(&key::SHIFT)
}

/// Adds (`n` times, stopping at the first refusal); returns whether any was added.
pub(crate) fn add(fe: &mut Fe, opt: &BuildOption, n: usize) -> bool {
    let mut live = 0;
    let mut any = false;
    for _ in 0..n {
        if !increment(fe, opt, &mut live) {
            break;
        }
        any = true;
    }
    flush_live(fe, opt.ship_type, live);
    any
}

pub(crate) fn remove(fe: &mut Fe, opt: &BuildOption, n: usize) {
    let mut live = 0;
    for _ in 0..n {
        decrement(fe, opt, &mut live);
    }
    flush_live(fe, opt.ship_type, live);
}

/// `cmSelectAvailable`: clicks on the ship list.
fn select_available(fe: &mut Fe, id: RegionId, ev: u32) {
    let Some(rect) = fe.region(id).map(|r| r.rect) else {
        return;
    };
    match ev {
        RPE_WHEEL_UP => return scroll_by(fe, -3),
        RPE_WHEEL_DOWN => return scroll_by(fe, 3),
        _ => {}
    }
    let (_, my) = fe.mouse_pos();
    let Some(opt) = ship_at(fe, rect, my) else {
        return;
    };
    list_event(fe, &opt, ev);
}

/// A click on a ship row of the list (`cmSelectAvailable` after finding the row): also what
/// the modern build manager's rows do.
pub(crate) fn list_event(fe: &mut Fe, opt: &BuildOption, ev: u32) {
    let opt = opt.clone();
    let t = opt.ship_type;
    let n = if ctrl(fe) { 5 } else { 1 };
    match ev {
        RPE_PRESS_LEFT | RPE_DOUBLE_LEFT => {
            let selected = fe.game.ig.cm.selected.contains(&t);
            if !selected || shift(fe) {
                select(fe, t);
            } else if add(fe, &opt, n) {
                fe.game.ig.cm.arrow_fired = Some((t, true));
            }
        }
        RPE_PRESS_RIGHT | RPE_DOUBLE_RIGHT => {
            remove(fe, &opt, n);
            if n == 1 {
                select(fe, t);
            }
            fe.game.ig.cm.arrow_fired = Some((t, false));
        }
        _ => {}
    }
    fe.game.ig.cm.ship_view = Some(t);
}

/// Selects a row: alone, or toggled with Shift.
fn select(fe: &mut Fe, t: ShipType) {
    let sh = shift(fe);
    let sel = &mut fe.game.ig.cm.selected;
    if sh {
        if !sel.remove(&t) {
            sel.insert(t);
        }
    } else {
        sel.clear();
        sel.insert(t);
    }
}

/// `cmLeftArrowProcess` / `cmRightArrowProcess`: held arrows repeat every quarter second.
fn arrow_process(fe: &mut Fe, id: RegionId, ev: u32, right: bool) {
    let Some(rect) = fe.region(id).map(|r| r.rect) else {
        return;
    };
    match ev {
        RPE_PRESS_LEFT => {
            fe.game.ig.cm.acc_time = fe.time - REPEAT_SECONDS;
            let (_, my) = fe.mouse_pos();
            fe.game.ig.cm.arrow_row = ship_at(fe, rect, my).map(|o| o.ship_type);
        }
        RPE_RELEASE_LEFT => fe.game.ig.cm.arrow_row = None,
        RPE_HOLD_LEFT => {
            let (_, my) = fe.mouse_pos();
            let Some(opt) = ship_at(fe, rect, my) else {
                return;
            };
            fe.game.ig.cm.arrow_row = Some(opt.ship_type);
            if fe.time < fe.game.ig.cm.acc_time {
                return;
            }
            fe.game.ig.cm.acc_time = fe.time + REPEAT_SECONDS;
            let n = if ctrl(fe) { 5 } else { 1 };
            if right {
                if add(fe, &opt, n) {
                    fe.game.ig.cm.arrow_fired = Some((opt.ship_type, true));
                }
            } else {
                remove(fe, &opt, n);
                fe.game.ig.cm.arrow_fired = Some((opt.ship_type, false));
            }
        }
        RPE_WHEEL_UP => scroll_by(fe, -3),
        RPE_WHEEL_DOWN => scroll_by(fe, 3),
        _ => {}
    }
}

// -------------------------------------------------------------------------------------------
// Buttons
// -------------------------------------------------------------------------------------------

/// `cmBuildShips`: every picked count goes to the factory.
pub fn build_ships(fe: &mut Fe) {
    let Some(factory) = fe.game.ig.cm.factory else {
        return;
    };
    let mut queued = Vec::new();
    for r in rows(&fe.game.ig.view) {
        if let Row::Ship(o) = r {
            let n = pending(fe, o.ship_type);
            if n > 0 {
                fe.game.ig.request(GameRequest::Build {
                    factory,
                    ship_type: o.ship_type,
                    count: n,
                });
                queued.push(o);
            }
        }
    }
    fe.game.ig.cm.pending.clear();
    if let Some(first) = queued.first() {
        let single = queued.iter().all(|o| o.ship_type == first.ship_type);
        let speech = if first.class < 5 && single {
            GameRequest::Speech {
                event: SPEECH_BUILD_CAPITAL,
                param: Some(first.ship_type),
            }
        } else {
            GameRequest::Speech {
                event: SPEECH_BUILD,
                param: None,
            }
        };
        fe.game.ig.request(speech);
    }
}

/// `cmPauseJobs`: pauses the selected types, or resumes them when all are paused.
pub fn pause_jobs(fe: &mut Fe) {
    let Some(factory) = fe.game.ig.cm.factory else {
        return;
    };
    let selected: Vec<ShipType> = fe.game.ig.cm.selected.iter().copied().collect();
    let paused_of = |fe: &Fe, t| fe.game.ig.view.job(factory, t).is_some_and(|j| j.paused);
    let all_paused = selected.iter().all(|&t| paused_of(fe, t));
    let mut changed = false;
    for &t in &selected {
        if fe.game.ig.view.job(factory, t).is_some_and(|j| j.count > 0) {
            changed = true;
            fe.game.ig.request(GameRequest::PauseBuild {
                factory,
                ship_type: t,
                paused: !all_paused,
            });
        }
    }
    if changed {
        let event = if all_paused {
            SPEECH_RESUME
        } else {
            SPEECH_PAUSE
        };
        fe.game
            .ig
            .request(GameRequest::Speech { event, param: None });
    }
}

/// `cmCancelJobs`: the selected types lose their picked counts and their factory jobs.
pub fn cancel_jobs(fe: &mut Fe) {
    let Some(factory) = fe.game.ig.cm.factory else {
        return;
    };
    let selected: Vec<ShipType> = fe.game.ig.cm.selected.iter().copied().collect();
    let mut cancelled = false;
    for &t in &selected {
        if fe.game.ig.cm.pending.remove(&t).is_some_and(|n| n > 0) {
            cancelled = true;
        }
        if fe.game.ig.view.job(factory, t).is_some() {
            cancelled = true;
            fe.game.ig.request(GameRequest::CancelBuild {
                factory,
                ship_type: t,
            });
        }
    }
    if cancelled {
        fe.game.ig.request(GameRequest::Speech {
            event: SPEECH_CANCEL,
            param: None,
        });
    }
}

// -------------------------------------------------------------------------------------------
// Totals
// -------------------------------------------------------------------------------------------

/// RUs still owed: picked counts plus every factory's queue less what was paid so far.
pub fn total_cost(fe: &Fe) -> i32 {
    let picked: i32 = rows(&fe.game.ig.view)
        .iter()
        .filter_map(|r| match r {
            Row::Ship(o) => Some(pending(fe, o.ship_type) * o.cost),
            _ => None,
        })
        .sum();
    let queued: i32 = fe
        .game
        .ig
        .view
        .jobs
        .iter()
        .map(|j| j.count * j.cost - j.cost_so_far)
        .sum();
    picked + queued
}

/// Picked plus queued ships of the current factory.
pub fn total_ships(fe: &Fe) -> i32 {
    rows(&fe.game.ig.view)
        .iter()
        .filter_map(|r| match r {
            Row::Ship(o) => {
                Some(pending(fe, o.ship_type) + job(fe, o.ship_type).map_or(0, |j| j.count))
            }
            _ => None,
        })
        .sum()
}

// -------------------------------------------------------------------------------------------
// Drawing
// -------------------------------------------------------------------------------------------

/// `feStaticRectangleDraw` for a user region's atom.
pub fn static_rect(fe: &Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    let a = fe.atom(atom);
    if a.has(FAF_CONTENTS_VISIBLE) {
        fer.canvas.fill_rect(rect, a.content_color);
    }
    if a.has(FAF_BORDER_VISIBLE) {
        let ds = a.drawstyle;
        fer.box_region(rect, ds[0] as i32, ds[1] as i32, &[], false);
    }
}

/// Colour of a ship row's text (names column).
fn name_colour(fe: &Fe, o: &BuildOption) -> Color {
    let mut c = if o.status == BuildStatus::Greyed {
        GREYED_TEXT
    } else if pending(fe, o.ship_type) >= 1 {
        PENDING_TEXT
    } else if fe.game.ig.cm.selected.contains(&o.ship_type) {
        colour::LIST_ITEM_SELECTED
    } else {
        colour::LIST_ITEM_STANDARD
    };
    if let Some(j) = job(fe, o.ship_type).filter(|j| j.count > 0) {
        c = if j.paused { PAUSED_TEXT } else { BUILDING_TEXT };
    }
    if let Some((t, at)) = fe.game.ig.cm.flash {
        if t == o.ship_type && fe.time - at < FLASH_SECONDS {
            c = INVALID_TEXT;
        }
    }
    c
}

/// Colour of the numbers and cost columns.
fn column_colour(fe: &Fe, o: &BuildOption) -> Color {
    let mut c = if o.status == BuildStatus::Greyed {
        GREYED_TEXT
    } else if pending(fe, o.ship_type) < 1 {
        colour::LIST_ITEM_STANDARD
    } else {
        PENDING_TEXT
    };
    if job(fe, o.ship_type).is_some_and(|j| j.count > 0) {
        c = BUILDING_TEXT;
    }
    c
}

/// `cmShipInfoDraw`.
fn draw_ships(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    fer.canvas.fill_rect(
        Rect::new(rect.x0, rect.y0, rect.x1 - 92, rect.y1),
        BACKGROUND,
    );
    let Some(font) = fer.res.font(FONT_NAMES) else {
        return;
    };
    let fh = crate::text::height(&font);
    fer.canvas.set_clip(Some(rect));
    for (row, y) in visible(fe, rect, FONT_NAMES) {
        match row {
            Row::Heading(h) => {
                draw::print(fer.canvas, &font, rect.x0 + 3, y, HEADING_TEXT, &h);
            }
            Row::Ship(o) => {
                if fe.game.ig.cm.selected.contains(&o.ship_type) {
                    draw::translucent_rect(
                        fer.canvas,
                        Rect::new(rect.x0, y, rect.x0 + 190, y + 12),
                        colour::LIST_ITEM_TRANSLUCENT,
                        Some(colour::LIST_ITEM_TRANSLUCENT_BORDER),
                    );
                }
                let mut shadow = None;
                if let Some(j) = job(fe, o.ship_type).filter(|j| j.count > 0) {
                    let w = rect.width() - 94;
                    let pct = (j.progress.clamp(0.0, 1.0) * 100.0) as i32;
                    let x0 = rect.x0 + 2;
                    if pct > 0 {
                        fer.canvas
                            .fill_rect(Rect::new(x0, y, x0 + w * pct / 100, y + fh / 2), SHIP_BAR);
                    }
                    let total = j.batch_total.max(j.count).max(1) as f32;
                    let left = (j.count - 1).max(0) as f32 + (1.0 - j.progress.clamp(0.0, 1.0));
                    let batch = (100.0 - left * 100.0 / total) as i32;
                    if batch > 0 {
                        fer.canvas.fill_rect(
                            Rect::new(x0, y + fh / 2, x0 + w * batch / 100, y + 11),
                            BATCH_BAR,
                        );
                    }
                    shadow = Some([0, 0, 0, 0x40]);
                }
                let c = name_colour(fe, &o);
                match shadow {
                    Some(s) => {
                        draw::print_shadow(fer.canvas, &font, rect.x0 + 7, y, c, s, &o.name);
                    }
                    None => {
                        draw::print(fer.canvas, &font, rect.x0 + 7, y, c, &o.name);
                    }
                }
            }
        }
    }
    fer.canvas.set_clip(None);
}

fn draw_column(fe: &mut Fe, fer: &mut Fer, rect: Rect, right_margin: i32, cost: bool) {
    fer.canvas.fill_rect(rect, BACKGROUND);
    fer.canvas
        .line(rect.x0, rect.y0, rect.x0, rect.y1 - 1, DIVIDER);
    let Some(font) = fer.res.font(FONT_NUMBERS) else {
        return;
    };
    for (row, y) in visible(fe, rect, FONT_NUMBERS) {
        if let Row::Ship(o) = row {
            let v = if cost {
                o.cost
            } else {
                pending(fe, o.ship_type) + job(fe, o.ship_type).map_or(0, |j| j.count)
            };
            let c = column_colour(fe, &o);
            draw::print_right(
                fer.canvas,
                &font,
                rect.x1 - right_margin,
                y,
                c,
                &v.to_string(),
            );
        }
    }
}

/// `cmShipNumbersDraw`.
fn draw_numbers(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    draw_column(fe, fer, rect, 12, false);
}

/// `cmShipCostsDraw`.
fn draw_costs(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    draw_column(fe, fer, rect, 7, true);
}

/// `cmDrawArrow`: outlined triangles on every ship row; the one that just fired is filled.
fn draw_arrows(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, right: bool) {
    let w = rect.width() - 4;
    let fired = fe.game.ig.cm.arrow_fired;
    for (row, y) in visible(fe, rect, FONT_NUMBERS) {
        let Row::Ship(o) = row else { continue };
        let tri = if right {
            [
                (rect.x0 + 2, y),
                (rect.x0 + 2, y + w),
                (rect.x1 - 2, y + w / 2),
            ]
        } else {
            [
                (rect.x1 - 2, y),
                (rect.x0 + 2, y + w / 2),
                (rect.x1 - 2, y + w),
            ]
        };
        if fired == Some((o.ship_type, right)) {
            draw::fill_triangle(fer.canvas, tri, ARROW_FIRED);
            fe.game.ig.cm.arrow_fired = None;
        }
        draw::outline_triangle(fer.canvas, tri, ARROW_OUTLINE);
    }
}

/// RUs / total cost / total ships: right-aligned, centred vertically, in the atom's border
/// colour on the panel colour.
fn draw_number_box(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect, value: i32) {
    fer.canvas.fill_rect(rect, BACKGROUND);
    let Some(font) = fer.res.font(FONT_NAMES) else {
        return;
    };
    let c = fe.atom(atom).border_color;
    let y = rect.y0 + (rect.height() - crate::text::height(&font)) / 2;
    fer.canvas.set_clip(Some(rect));
    draw::print_right(fer.canvas, &font, rect.x1 - 2, y, c, &value.to_string());
    fer.canvas.set_clip(None);
}

/// `cmDrawShipImage`: factory icon (disabled / off / on) with the focus frame; the current
/// factory is joined to the name underline by a white line.
fn draw_factory(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, slot: usize) {
    let view = &fe.game.ig.view;
    let race = view.player.race.file_index();
    let present = factory_in_slot(view, slot).map(|f| f.ship);
    let current = present.is_some() && present == fe.game.ig.cm.factory;
    let icon = if slot == 0 { 1 } else { 2 };
    let state = match (present.is_some(), current) {
        (false, _) => "disabled",
        (true, false) => "off",
        (true, true) => "on",
    };
    let path = format!("FEMan\\Construction_Manager\\Build_race{race}_icon{icon}_{state}.lif");
    fer.focus_window(rect, current);
    let inner = Rect::new(rect.x0 + 2, rect.y0 + 2, rect.x1 - 2, rect.y1 - 2);
    if let Some(img) = fer.res.image(&path) {
        draw::textured_rect(fer.canvas, &img, inner, WHITE);
    }
    if current {
        if let Some(name) = fe.find_atom("CM_FactoryNameDraw") {
            let top = fe.atom(name).rect().y1;
            let cx = (rect.x0 + rect.x1) / 2;
            fer.canvas
                .fill_rect(Rect::new(cx - 1, top, cx + 1, rect.y0), WHITE);
        }
    }
}

/// `cmSelectedFactoryNameDraw`.
fn draw_factory_name(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    if let Some(font) = fer.res.font(FONT_FACTORY) {
        let view = &fe.game.ig.view;
        if let Some(f) = fe
            .game
            .ig
            .cm
            .factory
            .and_then(|s| view.factories.iter().find(|f| f.ship == s))
        {
            let y = rect.y1 - crate::text::height(&font);
            draw::print(fer.canvas, &font, rect.x0, y, WHITE, &f.name);
        }
    }
    fer.canvas
        .fill_rect(Rect::new(rect.x0, rect.y1 - 1, rect.x1, rect.y1 + 1), WHITE);
}

/// `cmSupportUnitsBarDraw`: the support icon, dimmed over the pending part and masked over
/// the free part, read bottom up.
fn draw_support_bar(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    let tex = |n: &str| {
        fer.res
            .image(&format!("FEMan\\TexDecorative\\b_Build_Support{n}.lif"))
    };
    let (Some(icon_top), Some(icon_low), Some(m_top), Some(m_mid), Some(m_low)) = (
        tex("Icon_Top"),
        tex("Icon_Low"),
        tex("Mask_Top"),
        tex("Mask_Mid"),
        tex("Mask_Low"),
    ) else {
        return;
    };
    let h = rect.height();
    let top = rect.y0 + 4;
    let bar_h = h - 22;
    let (x0, x1) = (rect.x0, rect.x1);
    let c = &mut *fer.canvas;
    draw::textured_rect(
        c,
        &icon_top,
        Rect::new(x0, rect.y0, x1, rect.y0 + h / 2 + 1),
        WHITE,
    );
    draw::textured_rect(
        c,
        &icon_low,
        Rect::new(x0, rect.y0 + h / 2 - 1, x1, rect.y1),
        WHITE,
    );
    let p = &fe.game.ig.view.player;
    let cap = p.support_capacity.max(1) as f32;
    let used = (p.support_used as f32 / cap).clamp(0.0, 1.0);
    let pending = ((p.support_pending + gui_support_units(fe)) as f32 / cap).clamp(0.0, 1.0);
    let mask = |c: &mut crate::canvas::Canvas, y: i32, tint: Color| {
        draw::textured_rect(c, &m_mid, Rect::new(x0, rect.y0 + 10, x1, y), tint);
        draw::textured_rect(c, &m_low, Rect::new(x0, y - 1, x1, y + 7), tint);
        draw::textured_rect(c, &m_top, Rect::new(x0, top, x1, rect.y0 + 11), tint);
    };
    let ya = top + 6 + (bar_h as f32 * (1.0 - used)) as i32;
    mask(c, ya, [0, 0, 80, 192]);
    let yb = top + 6 + (bar_h as f32 * (1.0 - used - pending).clamp(0.0, 1.0)) as i32;
    mask(c, yb, WHITE);
}

/// `svShipViewRender`: the application's picture of the ship, the name and cost header, and
/// the description when extended info is on.
fn draw_ship_view(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    let cm = &fe.game.ig.cm;
    if let Some(img) = &cm.ship_view_image {
        let x = rect.x0 + (rect.width() - img.width) / 2;
        let y = rect.y0 + (rect.height() - img.height) / 2;
        fer.canvas.set_clip(Some(rect));
        fer.canvas.blit(img, x, y, crate::canvas::BlitMode::Blend);
        fer.canvas.set_clip(None);
    }
    let Some(t) = cm.ship_view else { return };
    let Some(o) = rows(&fe.game.ig.view).into_iter().find_map(|r| match r {
        Row::Ship(o) if o.ship_type == t => Some(o),
        _ => None,
    }) else {
        return;
    };
    let Some(font) = fer.res.font(FONT_NAMES) else {
        return;
    };
    let fh = crate::text::height(&font);
    if cm.extended_info {
        fer.canvas.blend_rect(
            Rect::new(rect.x0 + 3, rect.y0 + 3, rect.x1 - 3, rect.y1 - 3),
            [0, 0, 0, 128],
        );
    }
    let (x, mut y) = (rect.x0 + 10, rect.y0 + 8);
    draw::print(fer.canvas, &font, x, y, HEADING_TEXT, &o.name);
    y += fh + 1;
    let s = &fe.game.ig.strings;
    let line = format!("{} {} {}", s.text("strCost"), o.cost, s.text("strRUs"));
    let line = line.replace("strCost", "COST:").replace("strRUs", "RUs");
    draw::print(fer.canvas, &font, x, y, HEADING_TEXT, &line);
    if cm.extended_info {
        y += fh * 2;
        for l in draw::wrap(&font, &o.description, rect.width() - 20) {
            if y + fh > rect.y1 - 4 {
                break;
            }
            draw::print(fer.canvas, &font, x, y, colour::LIST_ITEM_STANDARD, &l);
            y += fh + 1;
        }
    }
}

/// Where the application should render the 3D ship and which type (`SV_ShipView`).
pub fn ship_view(fe: &Fe) -> Option<(Rect, ShipType)> {
    if fe.game.ig.manager != Some(Manager::Construction) {
        return None;
    }
    let t = fe.game.ig.cm.ship_view?;
    let a = fe.find_atom("SV_ShipView")?;
    Some((fe.atom(a).rect(), t))
}
