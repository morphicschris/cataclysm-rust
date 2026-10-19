//! The launch manager ("SYSTEMS MANAGER", `LaunchMgr.c`): ships docked in the mothership or a
//! carrier grouped under fighter/corvette headings, Launch one / group / all, the auto-launch
//! check boxes, fighter and corvette slot bars, the Somtaaw module panel (health and repair
//! priority) and the Beast mothership's emergency power allocation.

use crate::canvas::{Color, Rect, WHITE};
use crate::consts::*;
use crate::controls::ListItem;
use crate::fe::Call;
use crate::fer::Fer;
use crate::ingame::consmgr::{factory_in_slot, static_rect, FONT_FACTORY};
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::{close_manager, open_manager, Fe, Manager};
use crate::region::{AtomRef, RegionId};

pub const FONT_LIST: &str = "HW_EuroseCond_11.hff";
const HEADING: Color = [255, 200, 0, 255];
const SLOT_FREE: Color = [50, 200, 50, 255];
const SLOT_USED: Color = [200, 50, 50, 255];
const SLOT_TEXT: Color = [128, 0, 0, 255];
pub const SPEECH_LAUNCH: u32 = 0x400_1067;
pub const SOUND_LAUNCH_MANAGER: u32 = 0x102;
/// `dEcclesiam` never leaves with "launch all".
const NEVER_LAUNCHED: ShipType = 0x8c;
/// The Beast mothership (emergency power allocation).
pub const BEAST_MOTHERSHIP: ShipType = 0x35;
/// The Somtaaw mothership (module panel).
pub const SOMTAAW_MOTHERSHIP: ShipType = 0x12;

/// Print order of the list (`0x879200`): fighters, then corvettes (workers list as corvettes).
pub const FIGHTER_ORDER: [ShipType; 12] = [0, 33, 1, 34, 2, 35, 36, 37, 3, 17, 52, 133];
pub const CORVETTE_ORDER: [ShipType; 9] = [4, 38, 5, 58, 39, 40, 59, 14, 49];
/// List item data of the two headings.
const HEADER_FIGHTER: u32 = 0x8000_0000;
const HEADER_CORVETTE: u32 = 0x8000_0001;

/// Health bar thresholds (`selShipHealthGreenFactor`, `selShipHealthYellowFactor`).
pub const HEALTH_GREEN: f32 = 0.5;
pub const HEALTH_YELLOW: f32 = 0.25;

#[derive(Debug, Clone, Default)]
pub struct State {
    /// Ship launched from.
    pub launcher: Option<ShipId>,
    /// Last slot used (0 mothership, 1-2 carriers).
    pub last_slot: usize,
    /// Hovered repair-priority button: (module slot, button).
    pub hover_button: Option<(usize, i32)>,
    list: Option<AtomRef>,
}

/// Health bar colours (`selHealthColoursGet`): (fill, empty).
pub fn health_colours(f: f32) -> (Color, Color) {
    if f > HEALTH_GREEN {
        ([107, 227, 99, 255], [71, 151, 66, 255])
    } else if f > HEALTH_YELLOW {
        ([213, 215, 29, 255], [142, 143, 19, 255])
    } else {
        ([215, 29, 29, 255], [142, 19, 19, 255])
    }
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("LM_Close", |fe, call| {
        if guard(fe, call) {
            close(fe);
        }
    });
    fe.callback_add("LM_Construction", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Construction);
        }
    });
    fe.callback_add("LM_Research", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Research);
        }
    });
    fe.callback_add("LM_Launch", |fe, call| {
        if guard(fe, call) {
            launch_selected(fe, false);
        }
    });
    fe.callback_add("LM_LaunchOne", |fe, call| {
        if guard(fe, call) {
            launch_selected(fe, true);
        }
    });
    fe.callback_add("LM_LaunchAll", |fe, call| {
        if guard(fe, call) {
            launch_all(fe);
        }
    });
    fe.callback_add("LM_AutoLaunchM", |fe, call| auto_launch(fe, call, 0));
    fe.callback_add("LM_AutoLaunchC1", |fe, call| auto_launch(fe, call, 1));
    fe.callback_add("LM_AutoLaunchC2", |fe, call| auto_launch(fe, call, 2));
    fe.callback_add("LM_ShipsToLaunch", ships_to_launch);
    fe.callback_add("LM_BeastSystemsSet", beast_systems);
    fe.list_drawer_add("LM_ShipsToLaunch", ship_item_draw);
    fe.draw_callback_add("LM_FighterUsed", |fe, fer, _a, r| {
        let v = launcher(fe).map_or((0, 0), |l| l.fighters);
        draw_slots(fe, fer, r, v)
    });
    fe.draw_callback_add("LM_CorvetteUsed", |fe, fer, _a, r| {
        let v = launcher(fe).map_or((0, 0), |l| l.corvettes);
        draw_slots(fe, fer, r, v)
    });
    fe.draw_callback_add("LM_MotherShipDraw", |fe, fer, a, r| {
        draw_icon(fe, fer, a, r, 0)
    });
    fe.draw_callback_add("LM_Carrier1Draw", |fe, fer, a, r| {
        draw_icon(fe, fer, a, r, 1)
    });
    fe.draw_callback_add("LM_Carrier2Draw", |fe, fer, a, r| {
        draw_icon(fe, fer, a, r, 2)
    });
    fe.draw_callback_add("LM_FactoryNameDraw", draw_name);
    fe.draw_callback_add("dont_draw", |fe, fer, a, r| {
        static_rect(fe, fer, a, r);
        fer.canvas.fill_rect(r, colour::BACKGROUND);
    });
    for i in 0..7 {
        let name = format!("LM_Module_Slot{}", i + 1);
        let f: crate::fe::FeDrawFunction<crate::cataclysm::Cataclysm> = match i {
            0 => |fe, fer, a, r| draw_module(fe, fer, a, r, 0),
            1 => |fe, fer, a, r| draw_module(fe, fer, a, r, 1),
            2 => |fe, fer, a, r| draw_module(fe, fer, a, r, 2),
            3 => |fe, fer, a, r| draw_module(fe, fer, a, r, 3),
            4 => |fe, fer, a, r| draw_module(fe, fer, a, r, 4),
            5 => |fe, fer, a, r| draw_module(fe, fer, a, r, 5),
            _ => |fe, fer, a, r| draw_module(fe, fer, a, r, 6),
        };
        fe.draw_callback_add(&name, f);
    }
    fe.region_function_add("lmSelectMotherShip", |fe, _, ev| select_slot(fe, ev, 0));
    fe.region_function_add("lmSelectCarrier1", |fe, _, ev| select_slot(fe, ev, 1));
    fe.region_function_add("lmSelectCarrier2", |fe, _, ev| select_slot(fe, ev, 2));
    fe.region_function_add("lmSlotProcess", slot_process);
}

fn guard(fe: &Fe, call: &Call) -> bool {
    !(fe.first_call(call) || fe.last_call(call))
}

pub fn launcher(fe: &Fe) -> Option<&Launcher> {
    let id = fe.game.ig.lm.launcher?;
    fe.game
        .ig
        .view
        .launchers
        .iter()
        .find(|l| l.factory.ship == id)
}

/// `lmLaunchBegin`: the launch ship is a selected mothership/carrier, else the last slot,
/// else the mothership, else the first carrier.
pub fn begin(fe: &mut Fe) {
    let view = &fe.game.ig.view;
    let chosen = match view.selection.as_slice() {
        [one] if view.launchers.iter().any(|l| l.factory.ship == one.ship) => Some(one.ship),
        _ => None,
    }
    .or_else(|| factory_in_slot(view, fe.game.ig.lm.last_slot).map(|f| f.ship))
    .or_else(|| factory_in_slot(view, 0).map(|f| f.ship))
    .or_else(|| view.launchers.first().map(|l| l.factory.ship));
    fe.game.ig.lm.launcher = chosen;
    fe.game.ig.request(GameRequest::Sound(
        crate::ingame::consmgr::SOUND_MANAGER_INTRO,
    ));
    fe.game.ig.request(GameRequest::Sound(SOUND_LAUNCH_MANAGER));
}

pub fn started(fe: &mut Fe) {
    for (atom, f) in [
        ("LM_MotherShipDraw", "lmSelectMotherShip"),
        ("LM_Carrier1Draw", "lmSelectCarrier1"),
        ("LM_Carrier2Draw", "lmSelectCarrier2"),
    ] {
        fe.region_function_set(atom, f, RPE_PRESS_LEFT);
    }
    for i in 1..=7 {
        if let Some(id) = fe.region_function_set(
            &format!("LM_Module_Slot{i}"),
            "lmSlotProcess",
            RPM_MOUSE_EVENTS,
        ) {
            fe.regions.move_to_front(id);
        }
    }
    update_enabled(fe);
    refill(fe);
}

/// Per frame: the launch ship may have died; the list follows the ships docked.
pub fn tick(fe: &mut Fe) {
    if fe.game.ig.manager != Some(Manager::Launch) {
        return;
    }
    if launcher(fe).is_none() {
        let next = factory_in_slot(&fe.game.ig.view, 0)
            .map(|f| f.ship)
            .or_else(|| fe.game.ig.view.launchers.first().map(|l| l.factory.ship));
        match next {
            Some(n) => fe.game.ig.lm.launcher = Some(n),
            None => return close(fe),
        }
    }
    update_enabled(fe);
    refill(fe);
    update_hover(fe);
}

/// Enables the auto-launch boxes of present launchers, and the Beast radio.
fn update_enabled(fe: &mut Fe) {
    for (slot, name) in ["LM_AutoLaunchM", "LM_AutoLaunchC1", "LM_AutoLaunchC2"]
        .into_iter()
        .enumerate()
    {
        let present = factory_in_slot(&fe.game.ig.view, slot).is_some();
        for a in atoms(fe, name) {
            fe.set_disabled(a, !present);
        }
    }
    let beast =
        factory_in_slot(&fe.game.ig.view, 0).is_some_and(|f| f.ship_type == BEAST_MOTHERSHIP);
    for a in atoms(fe, "LM_BeastSystemsSet") {
        fe.set_disabled(a, !beast);
    }
}

fn atoms(fe: &Fe, name: &str) -> Vec<AtomRef> {
    fe.current_screen()
        .map(|s| fe.atoms_named(s, name))
        .unwrap_or_default()
}

/// `lmClose`.
pub fn close(fe: &mut Fe) {
    let slot = fe
        .game
        .ig
        .lm
        .launcher
        .and_then(|l| {
            (0..3).find(|&s| factory_in_slot(&fe.game.ig.view, s).is_some_and(|f| f.ship == l))
        })
        .unwrap_or(0);
    fe.game.ig.lm.last_slot = slot;
    fe.game.ig.lm.list = None;
    fe.game.ig.request(GameRequest::Sound(
        crate::ingame::consmgr::SOUND_MANAGER_EXIT,
    ));
    fe.game
        .ig
        .request(GameRequest::Sound(crate::ingame::consmgr::SOUND_SPACE));
    close_manager(fe);
}

fn select_slot(fe: &mut Fe, ev: u32, slot: usize) {
    if ev != RPE_PRESS_LEFT {
        return;
    }
    let Some(f) = factory_in_slot(&fe.game.ig.view, slot).cloned() else {
        return;
    };
    if fe.game.ig.lm.launcher != Some(f.ship) {
        fe.game.ig.lm.launcher = Some(f.ship);
        refill(fe);
    }
}

// -------------------------------------------------------------------------------------------
// The list
// -------------------------------------------------------------------------------------------

/// Rows: (text, data, selectable). Ship rows hold "count\tname" and the type as data.
fn wanted_rows(fe: &Fe) -> Vec<(String, u32, bool)> {
    let Some(l) = launcher(fe) else {
        return Vec::new();
    };
    let s = &fe.game.ig.strings;
    let heading = |class: &str| {
        format!(
            "{} {}",
            s.get(class).unwrap_or(""),
            s.get("strCLASS_Class").unwrap_or("CLASS")
        )
        .trim()
        .to_string()
    };
    let mut out = Vec::new();
    for (order, header, class, classes) in [
        (
            &FIGHTER_ORDER[..],
            HEADER_FIGHTER,
            "strCLASS_Fighter",
            [6, 0xb],
        ),
        (
            &CORVETTE_ORDER[..],
            HEADER_CORVETTE,
            "strCLASS_Corvette",
            [5, 7],
        ),
    ] {
        let mut rows = Vec::new();
        let mut has_class = false;
        for &t in order {
            let ships: Vec<&DockedShip> = l.docked.iter().filter(|d| d.ship_type == t).collect();
            if let Some(first) = ships.first() {
                has_class |= ships.iter().any(|d| classes.contains(&d.class));
                rows.push((format!("{}\t{}", ships.len(), first.name), t, true));
            }
        }
        if has_class {
            out.push((heading(class), header, false));
        }
        out.extend(rows);
    }
    out
}

/// Rebuilds the list when the docked ships changed, keeping the selected type.
fn refill(fe: &mut Fe) {
    let Some(a) = fe.game.ig.lm.list else { return };
    let rows = wanted_rows(fe);
    let Some(l) = fe.list_mut(a) else { return };
    let same = l.items.len() == rows.len()
        && l.items
            .iter()
            .zip(&rows)
            .all(|(i, r)| i.text == r.0 && i.data == r.1);
    if same {
        return;
    }
    let selected = l.selected.and_then(|i| l.items.get(i)).map(|i| i.data);
    let upper = l.upper;
    l.clear();
    for (text, data, can_select) in rows {
        l.items.push(ListItem {
            text,
            data,
            can_select,
            selected: false,
        });
    }
    if let Some(i) = selected.and_then(|d| l.items.iter().position(|i| i.data == d)) {
        l.select(i);
    }
    l.upper = upper.min(l.items.len().saturating_sub(l.max_visible));
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// `lmShipsToLaunchDraw` (list callback): set up on creation.
fn ships_to_launch(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.last_call(call) {
        fe.game.ig.lm.list = None;
        return;
    }
    if !fe.first_call(call) {
        return;
    }
    let h = fe.resources().font(FONT_LIST).map_or(16, |f| {
        let h = crate::text::height(&f);
        h + h / 2
    });
    fe.list_init(a, FONT_LIST, h);
    fe.game.ig.lm.list = Some(a);
    refill(fe);
}

/// `lmShipItemDraw`: " 12 x ACOLYTE", headings in gold.
fn ship_item_draw(_fe: &Fe, fer: &mut Fer, row: Rect, item: &ListItem) {
    let Some(font) = fer.res.font(FONT_LIST) else {
        return;
    };
    if !item.can_select {
        draw::print(fer.canvas, &font, row.x0, row.y0, HEADING, &item.text);
        return;
    }
    let c = if item.selected {
        colour::LIST_ITEM_SELECTED
    } else {
        colour::LIST_ITEM_STANDARD
    };
    let (count, name) = item.text.split_once('\t').unwrap_or(("", &item.text));
    let col = row.x0 + draw::width(&font, " 55 ");
    draw::print_right(fer.canvas, &font, col, row.y0, c, &format!(" {count}"));
    draw::print(fer.canvas, &font, col, row.y0, c, &format!(" x {name}"));
}

fn selected_type(fe: &Fe) -> Option<ShipType> {
    let a = fe.game.ig.lm.list?;
    let l = fe.list(a)?;
    let item = l.selected.and_then(|i| l.items.get(i))?;
    item.can_select.then_some(item.data)
}

/// `lmLaunch` / `lmLaunchOne`.
pub fn launch_selected(fe: &mut Fe, one: bool) {
    let (Some(t), Some(l)) = (selected_type(fe), launcher(fe)) else {
        return;
    };
    let mut ships: Vec<ShipId> = l
        .docked
        .iter()
        .filter(|d| d.ship_type == t)
        .map(|d| d.ship)
        .collect();
    if one {
        ships.truncate(1);
    }
    send_launch(fe, ships);
}

/// `LM_LaunchAll`.
pub fn launch_all(fe: &mut Fe) {
    let Some(l) = launcher(fe) else { return };
    let ships = l
        .docked
        .iter()
        .filter(|d| d.ship_type != NEVER_LAUNCHED)
        .map(|d| d.ship)
        .collect();
    send_launch(fe, ships);
}

fn send_launch(fe: &mut Fe, ships: Vec<ShipId>) {
    let Some(launcher) = fe.game.ig.lm.launcher else {
        return;
    };
    if ships.is_empty() {
        return;
    }
    fe.game.ig.request(GameRequest::Launch { launcher, ships });
    fe.game.ig.request(GameRequest::Speech {
        event: SPEECH_LAUNCH,
        param: None,
    });
}

/// The auto-launch boxes: checked means "stay docked".
fn auto_launch(fe: &mut Fe, call: &Call, slot: usize) {
    let Some(a) = call.atom else { return };
    let Some(f) = factory_in_slot(&fe.game.ig.view, slot).cloned() else {
        if fe.first_call(call) {
            fe.set_disabled(a, true);
        }
        return;
    };
    let auto = fe
        .game
        .ig
        .view
        .launchers
        .iter()
        .find(|l| l.factory.ship == f.ship)
        .is_some_and(|l| l.auto_launch);
    let name = call.name.clone();
    if fe.first_call(call) {
        fe.toggle_set(&name, !auto);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let stay = fe.atom(a).checked();
    fe.game.ig.request(GameRequest::SetAutoLaunch {
        launcher: f.ship,
        on: !stay,
    });
}

/// `LM_BeastSystemsSet`: the Beast mothership's emergency power allocation.
fn beast_systems(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.ig.view.beast_systems;
        fe.radio_set(&call.name, v);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let v = fe.atom(a).radio_value;
    fe.game.ig.request(GameRequest::BeastSystems(v));
}

// -------------------------------------------------------------------------------------------
// Drawing
// -------------------------------------------------------------------------------------------

/// Slot usage: free capacity green, used red, then the count beside it.
fn draw_slots(_fe: &mut Fe, fer: &mut Fer, rect: Rect, (used, cap): (i32, i32)) {
    fer.canvas.fill_rect(rect, SLOT_FREE);
    let f = if cap > 0 {
        (used as f32 / cap as f32).min(1.0)
    } else {
        0.0
    };
    let w = (rect.width() as f32 * f) as i32;
    fer.canvas
        .fill_rect(Rect::new(rect.x0, rect.y0, rect.x0 + w, rect.y1), SLOT_USED);
    if let Some(font) = fer.res.font(FONT_LIST) {
        let used = used.min(cap.max(0));
        draw::print(
            fer.canvas,
            &font,
            rect.x1,
            rect.y0,
            SLOT_TEXT,
            &format!(" {used}"),
        );
    }
}

/// `lmDrawShipImage`: like the build manager's icons, inset 4.
fn draw_icon(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, slot: usize) {
    let view = &fe.game.ig.view;
    let race = view.player.race.file_index();
    let present = factory_in_slot(view, slot).map(|f| f.ship);
    let current = present.is_some() && present == fe.game.ig.lm.launcher;
    let icon = if slot == 0 { 1 } else { 2 };
    let state = match (present.is_some(), current) {
        (false, _) => "disabled",
        (true, false) => "off",
        (true, true) => "on",
    };
    fer.focus_window(rect, current);
    let path = format!("FEMan\\Construction_Manager\\Build_race{race}_icon{icon}_{state}.lif");
    let inner = Rect::new(rect.x0 + 4, rect.y0 + 4, rect.x1 - 4, rect.y1 - 4);
    if let Some(img) = fer.res.image(&path) {
        draw::textured_rect(fer.canvas, &img, inner, WHITE);
    }
    if current {
        if let Some(name) = fe.find_atom("LM_FactoryNameDraw") {
            let top = fe.atom(name).rect().y1;
            let cx = (rect.x0 + rect.x1) / 2;
            fer.canvas
                .fill_rect(Rect::new(cx - 1, top, cx + 1, rect.y0), WHITE);
        }
    }
}

/// `lmSelectedShipNameDraw`.
fn draw_name(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    if let (Some(font), Some(l)) = (fer.res.font(FONT_FACTORY), launcher(fe)) {
        let y = rect.y1 - crate::text::height(&font);
        draw::print(fer.canvas, &font, rect.x0, y, WHITE, &l.factory.name);
    }
    fer.canvas
        .fill_rect(Rect::new(rect.x0, rect.y1 - 1, rect.x1, rect.y1 + 1), WHITE);
}

/// Repair-priority button rectangles of a module row.
fn button_rects(rect: Rect) -> [Rect; 3] {
    let b = |x0: i32| Rect::new(rect.x1 - x0, rect.y0 + 4, rect.x1 - x0 + 32, rect.y0 + 20);
    [b(108), b(72), b(36)]
}

/// Module panel rows (Somtaaw mothership only).
fn module(fe: &Fe, slot: usize) -> Option<&Module> {
    let ms = factory_in_slot(&fe.game.ig.view, 0)?;
    if ms.ship_type != SOMTAAW_MOTHERSHIP {
        return None;
    }
    fe.game.ig.view.modules.get(slot)
}

/// `lmSlotNDraw`.
fn draw_module(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, slot: usize) {
    let Some(m) = module(fe, slot).cloned() else {
        fer.canvas.fill_rect(rect, colour::BACKGROUND);
        return;
    };
    if let Some(font) = fer.res.font(FONT_LIST) {
        draw::print(
            fer.canvas,
            &font,
            rect.x0 + 6,
            rect.y0,
            colour::LIST_ITEM_STANDARD,
            &m.name,
        );
    }
    let bar = Rect::new(rect.x0 + 6, rect.y1 - 6, rect.x1 - 114, rect.y1);
    let f = m.health.clamp(0.0, 1.0);
    let (fill, empty) = health_colours(f);
    fer.canvas.fill_rect(bar, empty);
    let w = (bar.width() as f32 * f) as i32;
    fer.canvas
        .fill_rect(Rect::new(bar.x0, bar.y0, bar.x0 + w, bar.y1), fill);
    let hover = fe.game.ig.lm.hover_button;
    for (i, r) in button_rects(rect).into_iter().enumerate() {
        let hovered = hover == Some((slot, i as i32));
        let state = match m.repair_priority {
            None => "disabled",
            Some(p) if p == i as i32 => {
                if hovered {
                    "on"
                } else {
                    "off_focus"
                }
            }
            Some(_) => {
                if hovered {
                    "off_mouse"
                } else {
                    "off"
                }
            }
        };
        let path = format!("FEMan\\Construction_Manager\\R1_Sys{}_{state}.lif", i + 1);
        if let Some(img) = fer.res.image(&path) {
            draw::textured_rect(fer.canvas, &img, r, WHITE);
        }
    }
}

/// Which repair button the mouse x is over (the engine ignores y).
fn button_at(rect: Rect, x: i32) -> i32 {
    if x > rect.x1 - 32 {
        2
    } else if x > rect.x1 - 68 {
        1
    } else if x > rect.x1 - 104 {
        0
    } else {
        -1
    }
}

fn slot_of_region(fe: &Fe, id: RegionId) -> Option<(usize, Rect)> {
    let r = fe.region(id)?;
    let name = fe.atom(r.atom?).name.clone()?;
    let n: usize = name.strip_prefix("LM_Module_Slot")?.parse().ok()?;
    Some((n - 1, r.rect))
}

/// `lmSlotNProcess`: hover tracking, release picks a priority.
fn slot_process(fe: &mut Fe, id: RegionId, ev: u32) {
    let Some((slot, rect)) = slot_of_region(fe, id) else {
        return;
    };
    let (mx, _) = fe.mouse_pos();
    let b = button_at(rect, mx);
    fe.game.ig.lm.hover_button = (b >= 0).then_some((slot, b));
    if ev == RPE_RELEASE_LEFT && b >= 0 {
        if let Some(m) = module(fe, slot).cloned() {
            if m.repair_priority.is_some() {
                fe.game.ig.request(GameRequest::SetRepairPriority {
                    module_type: m.ship_type,
                    priority: b,
                });
            }
        }
    }
}

/// Clears the hover once the mouse leaves the module rows.
fn update_hover(fe: &mut Fe) {
    let (mx, my) = fe.mouse_pos();
    let inside = (1..=7).any(|i| {
        fe.find_atom(&format!("LM_Module_Slot{i}"))
            .is_some_and(|a| fe.atom(a).rect().contains(mx, my))
    });
    if !inside {
        fe.game.ig.lm.hover_button = None;
    }
}
