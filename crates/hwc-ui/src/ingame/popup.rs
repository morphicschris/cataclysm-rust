//! The right-click menu (`PopupMenu.c`, Cataclysm-only): the main menu (orders, tactics,
//! formations, focus, views, sensors toggles) and the special-actions menu, each item running
//! its key action; submenus open beside the hovered item. Drawn with the `PopupMenu_*` overlay
//! icons; the ships an item applies to are outlined.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::consts::key;
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::keys::{self, Action};
use crate::ingame::{execute_action, Fe};

pub const FONT: &str = "arial_12.hff";
pub const FONT_LARGE: &str = "arial_15.hff";
const TITLE: Color = [0, 200, 0, 255];
const GREYED: Color = [120, 120, 120, 255];
const FILL: Color = [0, 0, 60, 150];
const TARGET_OUTLINE: Color = [150, 180, 230, 255];
/// The boxes around the highlighted item's targets: allies, enemies.
const ALLY_TARGET: Color = [0, 150, 0, 255];
const ENEMY_TARGET: Color = [150, 0, 0, 255];

/// Which menu is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    Main,
    Special,
}

/// Submenus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Submenu {
    Tactics,
    Formations,
    Sensors,
    Leech,
    /// "Select Mimic": the disguise menus.
    MimicType,
    /// "Mimic": the entries of the picked disguise menu.
    MimicItem,
    Focus,
    View,
    WaypointMode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub label: String,
    pub action: Option<Action>,
    pub submenu: Option<Submenu>,
    /// What the item asks of the game when it has no key action (the Mimic submenus' picks).
    pub request: Option<GameRequest>,
    pub group: u8,
    pub greyed: bool,
    pub checked: bool,
    pub flash: bool,
    /// The title line (not selectable).
    pub title: bool,
    /// Placed rectangle (text area).
    pub rect: Rect,
}

fn item(label: &str, action: Option<Action>, group: u8) -> Item {
    Item {
        label: label.into(),
        action,
        submenu: None,
        request: None,
        group,
        greyed: false,
        checked: false,
        flash: false,
        title: false,
        rect: Rect::default(),
    }
}

fn sub(label: &str, s: Submenu, group: u8) -> Item {
    Item {
        submenu: Some(s),
        ..item(label, None, group)
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub open: Option<MenuKind>,
    pub origin: (i32, i32),
    pub items: Vec<Item>,
    pub highlight: Option<usize>,
    /// Open submenu: (parent item, items).
    pub sub: Option<(usize, Vec<Item>)>,
    pub sub_highlight: Option<usize>,
    /// The action of the item the menu last carried out, until the game takes it (it tells
    /// a menu pick from the same order's key).
    pub executed: Option<Action>,
    /// The targets of the highlighted special action (the band-boxed ones, else its context
    /// target), as the game placed them on screen: centre, radius, ally
    /// (`pmDrawItemTargetHighlights`).
    pub target_boxes: Vec<(i32, i32, i32, bool)>,
    /// Metrics of the last layout: space width, font height.
    metrics: (i32, i32),
}

pub fn is_open(fe: &Fe) -> bool {
    fe.game.ig.popup.open.is_some()
}

pub fn close(fe: &mut Fe) {
    let p = &mut fe.game.ig.popup;
    p.open = None;
    p.items.clear();
    p.sub = None;
    p.highlight = None;
    p.sub_highlight = None;
    p.target_boxes.clear();
}

/// `pmOpenForTargets(0)` at the mouse (Open_Menu, right click on empty space).
pub fn open_at_mouse(fe: &mut Fe) {
    let (x, y) = fe.mouse_pos();
    open(fe, MenuKind::Main, x, y);
}

/// Opens a menu with its top-left at the mouse + (5, 5).
pub fn open(fe: &mut Fe, kind: MenuKind, x: i32, y: i32) {
    if fe.game.ig.popup.open == Some(kind) {
        return;
    }
    let items = match kind {
        MenuKind::Main => main_items(fe),
        MenuKind::Special => special_items(fe),
    };
    let p = &mut fe.game.ig.popup;
    p.open = Some(kind);
    p.origin = (x + 5, y + 5);
    p.items = items;
    p.sub = None;
    p.sub_highlight = None;
    p.highlight = p.items.iter().position(selectable);
    layout(fe);
}

fn selectable(i: &Item) -> bool {
    !i.title && !i.greyed
}

fn has(view: &HostView, c: Command) -> bool {
    view.commands.contains(&c)
}

/// `pmEnableMenuItems`: the main menu for the selection.
fn main_items(fe: &Fe) -> Vec<Item> {
    let view = &fe.game.ig.view;
    let mut v = Vec::new();
    if let [one] = view.selection.as_slice() {
        v.push(Item {
            title: true,
            ..item(&one.name, None, 0)
        });
    }
    // While waypoints are being placed only the waypoint items and the view menus
    // (`pmEnableMenuItems` with `piePointSpecMode` 4).
    if view.waypoints.is_some() {
        v.push(item("End Waypoints", Some(keys::WAYPOINTS), 3));
        v.push(sub("Waypoint Mode", Submenu::WaypointMode, 3));
        v.push(item("Cancel Waypoints", Some(keys::ESCAPE), 3));
        v.push(item(
            "Remove Last Waypoint",
            Some(keys::CANCEL_LAST_WAYPOINT),
            3,
        ));
        v.push(sub("Focus", Submenu::Focus, 5));
        v.push(sub("View", Submenu::View, 6));
        v.push(sub("Sensors", Submenu::Sensors, 7));
        return v;
    }
    if !view.selection.is_empty() {
        v.push(item("Special Action...", Some(keys::SHIP_SPECIAL), 1));
    }
    if has(view, Command::Upgrade) {
        let label = if view.player.race == Race::Beast {
            "Mutate"
        } else {
            "Upgrade"
        };
        v.push(Item {
            flash: true,
            ..item(label, Some(keys::UPGRADE), 2)
        });
    }
    if has(view, Command::Move) {
        v.push(item("Move", Some(keys::MOVE), 3));
    }
    if has(view, Command::Hyperspace) && view.player.multiplayer {
        v.push(item("Hyperspace", Some(keys::HYPERSPACE), 3));
    }
    // Single player (`pmEnableMenuItems`): the jump, labelled "Hyperspace" or "End Mission",
    // while the mission allows it.
    if let (Some(label), false) = (&view.hyperspace_label, view.player.multiplayer) {
        v.push(item(label, Some(keys::HYPERSPACE), 3));
    }
    if has(view, Command::Move) {
        v.push(item("Start Waypoints", Some(keys::WAYPOINTS), 3));
    }
    if has(view, Command::Dock) {
        v.push(item("Dock", Some(keys::DOCK), 3));
        v.push(item("Retire", Some(keys::RETIRE), 3));
    }
    if has(view, Command::Cancel) {
        v.push(item("Cancel", Some(keys::CANCEL_ORDERS), 3));
    }
    let tactics = [
        Command::TacticsEvasive,
        Command::TacticsNeutral,
        Command::TacticsAggressive,
    ];
    if tactics.iter().any(|c| has(view, *c)) {
        v.push(sub("Tactics", Submenu::Tactics, 4));
    }
    if view
        .commands
        .iter()
        .any(|c| matches!(c, Command::Formation(_)))
    {
        v.push(sub("Formations", Submenu::Formations, 4));
    }
    v.push(sub("Focus", Submenu::Focus, 5));
    v.push(sub("View", Submenu::View, 6));
    v.push(sub("Sensors", Submenu::Sensors, 7));
    v
}

/// `pmBuildMenuItems`: the special actions the selection can do.
fn special_items(fe: &Fe) -> Vec<Item> {
    let view = &fe.game.ig.view;
    let mut v = Vec::new();
    let specials: [(Command, &str, Action, u8); 17] = [
        (Command::Harvest, "Harvest", keys::HARVEST, 12),
        (Command::Repair, "Repair", keys::REPAIR, 12),
        (Command::Salvage, "Salvage", keys::SALVAGE, 12),
        (Command::Leech, "Leech", keys::LEECH, 13),
        (Command::Link, "Link", keys::LINK, 14),
        (Command::Unlink, "Unlink", keys::UNLINK, 14),
        (
            Command::ForceField,
            "Force Field",
            keys::FORCE_FIELD_OPTIONS,
            16,
        ),
        (
            Command::UnlinkForceField,
            "Unlink Force Field",
            keys::FORCE_FIELD_UNLINK,
            16,
        ),
        (Command::Cloak, "Cloak", keys::CLOAK, 17),
        (Command::Swarmers, "Swarm", keys::LAUNCH_SWARMERS, 18),
        (Command::BigGun, "Siege Cannon", keys::BIG_GUN_ATTACK, 18),
        (Command::BeastAttack, "Infect", keys::BEAST_ATTACK, 18),
        (Command::Emp, "EMP", keys::EMP, 18),
        (Command::Ram, "Ram", keys::RAM, 18),
        (Command::Repulse, "Repulse", keys::REPULSE, 18),
        (
            Command::FireMissiles,
            "Fire Missiles",
            keys::FIRE_MISSILES,
            18,
        ),
        (Command::Kamikaze, "Kamikaze", keys::KAMIKAZE, 18),
    ];
    for (c, label, a, g) in specials {
        if has(view, c) {
            if c == Command::Leech {
                v.push(sub("Select Leech", Submenu::Leech, g));
            }
            v.push(item(label, Some(a), g));
        }
        // `pmBuildMenuItems`: the Mimic items come after the link items.
        if c == Command::Unlink {
            if has(view, Command::Mimic) {
                v.push(sub("Select Mimic", Submenu::MimicType, 15));
                v.push(sub("Mimic", Submenu::MimicItem, 15));
            }
            if has(view, Command::UnMimic) {
                v.push(item("UnMimic", Some(keys::CANCEL_MIMIC), 15));
            }
        }
    }
    if v.is_empty() {
        v.push(Item {
            greyed: true,
            ..item("No Special Action Available...", None, 11)
        });
    }
    v
}

fn submenu_items(fe: &Fe, s: Submenu) -> Vec<Item> {
    let view = &fe.game.ig.view;
    let checked = |mut i: Item, c: bool| {
        i.checked = c;
        i
    };
    match s {
        Submenu::Tactics => {
            let t = view.selection.first().map(|s| s.tactics);
            let all = view.selection.iter().all(|s| Some(s.tactics) == t);
            [
                ("Evasive", keys::TACTICS_EVASIVE),
                ("Neutral", keys::TACTICS_NEUTRAL),
                ("Aggressive", keys::TACTICS_AGGRESSIVE),
            ]
            .iter()
            .enumerate()
            .map(|(i, (l, a))| checked(item(l, Some(*a), 0), all && t == Some(i as i32)))
            .collect()
        }
        Submenu::Formations => [
            ("Delta", keys::FORMATION_DELTA),
            ("Broad", keys::FORMATION_BROAD),
            ("X", keys::FORMATION_DELTA3D),
            ("Claw", keys::FORMATION_CLAW),
            ("Wall", keys::FORMATION_WALL),
            ("Sphere", keys::FORMATION_SPHERE),
            ("Custom", keys::FORMATION_CUSTOM),
            ("Parade", keys::FORMATION_PARADE),
        ]
        .iter()
        .map(|(l, a)| item(l, Some(*a), 0))
        .collect(),
        Submenu::Sensors => [
            ("Area Explored", 138),
            ("Sensors (Sphere)", 137),
            ("Sensors (Wireframe)", 139),
            ("Resources", 136),
            ("Dust", 144),
            ("Background", 143),
            ("Compass", 145),
            ("Objective Pointer", 146),
            ("Large Ship Overlay", 140),
            ("Slipgate Paths", 147),
            ("Ship Meshes", 141),
            ("Pieplate Lines", 142),
        ]
        .iter()
        .map(|(l, a)| item(l, Some(Action(*a)), 0))
        .collect(),
        Submenu::Leech => [
            ("Spy", keys::LEECH_SPY),
            ("Leech", keys::LEECH_LEECH),
            ("Vent", keys::LEECH_VECT),
        ]
        .iter()
        .map(|(l, a)| item(l, Some(*a), 0))
        .collect(),
        // `"%d. %s"` of the slot or the entry and its name (`pmBuildMenuItems`).
        Submenu::MimicType => view
            .mimic
            .types
            .iter()
            .map(|(slot, name)| Item {
                request: Some(GameRequest::MimicType(*slot)),
                ..checked(
                    item(&format!("{slot}. {name}"), None, 0),
                    *slot == view.mimic.current,
                )
            })
            .collect(),
        Submenu::MimicItem => view
            .mimic
            .items
            .iter()
            .enumerate()
            .map(|(i, name)| Item {
                request: Some(GameRequest::MimicItem(i)),
                ..item(&format!("{i}. {name}"), None, 0)
            })
            .collect(),
        Submenu::Focus => vec![
            Item {
                greyed: view.selection.is_empty(),
                ..item("On Selected", Some(keys::FOCUS), 0)
            },
            item("On Command Ship", Some(keys::FOCUS_ON_MOTHERSHIP), 0),
            item("On Event", Some(keys::FOCUS_ON_LAST_SPEAKER), 0),
            item("On Ships in View", Some(keys::FLEET_VIEW), 0),
        ],
        Submenu::View => {
            let sensors = view.player.in_sensors;
            vec![
                Item {
                    greyed: !sensors,
                    ..item("Main Region...", Some(keys::TOGGLE_SENSORS_MANAGER), 0)
                },
                Item {
                    greyed: sensors,
                    ..item("Sensors...", Some(keys::TOGGLE_SENSORS_MANAGER), 0)
                },
                item("Build Manager...", Some(keys::OPEN_BUILD_MANAGER), 0),
                item("Research Manager...", Some(keys::OPEN_RESEARCH_MANAGER), 0),
                item("Systems Manager...", Some(keys::OPEN_LAUNCH_MANAGER), 0),
                checked(
                    item("Ship Overlay", Some(keys::TOGGLE_SHIP_INFO), 0),
                    fe.game.ig.ship_print.on,
                ),
                checked(
                    item("Tactical Overlay", Some(keys::TOGGLE_TACTICAL_DISPLAY), 0),
                    fe.game.ig.overlay.tactical,
                ),
                item("Pilot View", Some(keys::TOGGLE_CHASE_MODE), 0),
            ]
        }
        Submenu::WaypointMode => {
            let mode = fe.game.ig.view.waypoints;
            vec![
                checked(
                    item("Linear", Some(keys::WAYPOINT_MODE_NORMAL), 0),
                    mode == Some(0),
                ),
                checked(
                    item("End To End", Some(keys::WAYPOINT_MODE_END_TO_END), 0),
                    mode == Some(2),
                ),
                checked(
                    item("Circular", Some(keys::WAYPOINT_MODE_CIRCULAR), 0),
                    mode == Some(1),
                ),
            ]
        }
    }
}

/// Submenus that always reserve the check column.
fn reserves_checks(s: Option<Submenu>) -> bool {
    matches!(
        s,
        Some(
            Submenu::Tactics
                | Submenu::Formations
                | Submenu::Leech
                | Submenu::MimicType
                | Submenu::Sensors
                | Submenu::View
                | Submenu::WaypointMode
        )
    )
}

fn font(fe: &Fe) -> Option<std::rc::Rc<hwc_formats::font::Font>> {
    let name = if fe.game.options.get_bool("LargePopupMenuFonts") {
        FONT_LARGE
    } else {
        FONT
    };
    fe.resources().font(name)
}

/// Bound keys of an action, as the key column shows them.
fn key_names(fe: &Fe, a: Action) -> String {
    fe.game
        .ig
        .keys
        .of(a)
        .iter()
        .map(|b| b.display())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Column widths of a menu: (check column, text width, key column).
fn columns(
    fe: &Fe,
    f: &hwc_formats::font::Font,
    items: &[Item],
    sub: Option<Submenu>,
) -> (i32, i32, i32) {
    let w = draw::width(f, " ");
    let text_w = items
        .iter()
        .map(|i| draw::width(f, &i.label))
        .max()
        .unwrap_or(0);
    let check = if items.iter().any(|i| i.checked) || reserves_checks(sub) {
        3 * w
    } else {
        0
    };
    let keys = items
        .iter()
        .filter(|i| i.action.is_some() || i.submenu.is_some())
        .map(|i| i.action.map_or(0, |a| draw::width(f, &key_names(fe, a))))
        .max();
    let key_col = keys.map_or(0, |k| 4 * w + k.max(3 * w));
    (check, text_w, key_col)
}

/// `pmLayoutMenu`: places the items of a menu at (x, y); returns its width.
fn place(
    fe: &Fe,
    f: &hwc_formats::font::Font,
    items: &mut [Item],
    x: i32,
    y: i32,
    groups: bool,
    sub: Option<Submenu>,
) -> i32 {
    let w = draw::width(f, " ");
    let h = crate::text::height(f);
    let (check, text_w, key_col) = columns(fe, f, items, sub);
    let width = text_w + (2 * w * 4) / 10 + check + key_col;
    let mut cy = y + h / 2;
    let mut group = None;
    for it in items.iter_mut() {
        if groups && group.is_some() && group != Some(it.group) {
            cy += h * 6 / 10;
        }
        group = Some(it.group);
        it.rect = Rect::new(x, cy, x + width, cy + h);
        cy += h + h / 2;
    }
    width
}

/// Lays out the open menu and submenu, keeping them on screen.
fn layout(fe: &mut Fe) {
    let Some(f) = font(fe) else { return };
    let w = draw::width(&f, " ");
    let h = crate::text::height(&f);
    let (sw, sh) = fe.screen_size();
    let border = 3 * w;
    let mut p = std::mem::take(&mut fe.game.ig.popup);
    let (x, y) = p.origin;
    let groups = p.open == Some(MenuKind::Main);
    let width = place(fe, &f, &mut p.items, x, y, groups, None);
    let bottom = p.items.last().map_or(y, |i| i.rect.y1 + h / 2);
    let dx = (sw - border - (x + width)).min(0);
    let dy = (sh - border - bottom).min(0);
    for it in &mut p.items {
        it.rect = it.rect.offset(dx, dy);
    }
    let main_left = x + dx;
    if let Some((parent, items)) = &mut p.sub {
        let pr = p.items.get(*parent).map_or(Rect::default(), |i| i.rect);
        let kind = p.items.get(*parent).and_then(|i| i.submenu);
        let sx = pr.x1 + 10 + border;
        let sy = pr.y0 - h / 2;
        let swidth = place(fe, &f, items, sx, sy, false, kind);
        let sbottom = items.last().map_or(sy, |i| i.rect.y1 + h / 2);
        let dx = if sx + swidth + border > sw {
            main_left - border - 10 - (sx + swidth)
        } else {
            0
        };
        let dy = (sh - border - sbottom).min(0);
        for it in items.iter_mut() {
            it.rect = it.rect.offset(dx, dy);
        }
    }
    p.metrics = (w, h);
    fe.game.ig.popup = p;
}

/// The item under the mouse: (in submenu, index).
fn hit(fe: &Fe, x: i32, y: i32) -> Option<(bool, usize)> {
    let p = &fe.game.ig.popup;
    let (w, h) = p.metrics;
    let border = 3 * w;
    let inside = |r: Rect| {
        Rect::new(r.x0 - border, r.y0 - h / 2, r.x1 + border, r.y1 + h / 2).contains(x, y)
    };
    if let Some((_, items)) = &p.sub {
        if let Some(i) = items.iter().position(|it| inside(it.rect)) {
            return Some((true, i));
        }
    }
    p.items
        .iter()
        .position(|it| inside(it.rect))
        .map(|i| (false, i))
}

/// `pmUpdateMouse`: hover moves the highlight and opens submenus.
pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    if !is_open(fe) {
        return;
    }
    match hit(fe, x, y) {
        Some((true, i)) => {
            let ok = fe
                .game
                .ig
                .popup
                .sub
                .as_ref()
                .is_some_and(|(_, items)| selectable(&items[i]));
            if ok {
                fe.game.ig.popup.sub_highlight = Some(i);
            }
        }
        Some((false, i))
            if selectable(&fe.game.ig.popup.items[i]) && fe.game.ig.popup.highlight != Some(i) =>
        {
            highlight_main(fe, i);
        }
        _ => {}
    }
}

fn highlight_main(fe: &mut Fe, i: usize) {
    let s = fe.game.ig.popup.items[i].submenu;
    fe.game.ig.popup.highlight = Some(i);
    fe.game.ig.popup.sub_highlight = None;
    fe.game.ig.popup.sub = s.map(|s| (i, submenu_items(fe, s)));
    layout(fe);
}

/// Mouse buttons while the menu is open; returns whether it was used.
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool, x: i32, y: i32) -> bool {
    if !is_open(fe) {
        return false;
    }
    let h = hit(fe, x, y);
    if down {
        if h.is_none() {
            close(fe);
        }
        return true;
    }
    if !left {
        return true;
    }
    match h {
        Some((true, i)) => {
            fe.game.ig.popup.sub_highlight = Some(i);
            execute_highlighted(fe);
        }
        Some((false, i)) => {
            if selectable(&fe.game.ig.popup.items[i]) {
                fe.game.ig.popup.highlight = Some(i);
                fe.game.ig.popup.sub_highlight = None;
                execute_highlighted(fe);
            }
        }
        None => close(fe),
    }
    true
}

/// `pmExecuteHighlighted`.
pub fn execute_highlighted(fe: &mut Fe) {
    let p = &fe.game.ig.popup;
    let (it, in_sub) = match (&p.sub, p.sub_highlight, p.highlight) {
        (Some((_, items)), Some(i), _) => (items[i].clone(), true),
        (_, _, Some(i)) => (p.items[i].clone(), false),
        _ => return,
    };
    if it.greyed || it.title {
        return;
    }
    let sensors_toggle = in_sub
        && p.sub
            .as_ref()
            .and_then(|(parent, _)| p.items.get(*parent))
            .is_some_and(|i| i.submenu == Some(Submenu::Sensors));
    if let Some(r) = it.request.clone() {
        close(fe);
        fe.game.ig.request(r);
        return;
    }
    let Some(a) = it.action else {
        return;
    };
    if a == keys::SHIP_SPECIAL && !in_sub {
        // "Special Action...": the menu turns into the special-actions menu.
        let (x, y) = fe.game.ig.popup.origin;
        fe.game.ig.popup.open = None;
        open(fe, MenuKind::Special, x - 5, y - 5);
        return;
    }
    if !sensors_toggle {
        close(fe);
    }
    fe.game.ig.popup.executed = Some(a);
    execute_action(fe, a, true);
    if a.has_up() {
        execute_action(fe, a, false);
    }
}

/// `pmProcessKey`: Enter, arrows; returns whether the key was used.
pub fn process_key(fe: &mut Fe, code: u16) -> bool {
    if !is_open(fe) {
        return false;
    }
    let step = |items: &[Item], cur: Option<usize>, d: isize| -> Option<usize> {
        let n = items.len() as isize;
        if n == 0 {
            return None;
        }
        let mut i = cur.map_or(if d > 0 { -1 } else { n }, |c| c as isize);
        for _ in 0..n {
            i = (i + d).rem_euclid(n);
            if selectable(&items[i as usize]) {
                return Some(i as usize);
            }
        }
        cur
    };
    match code {
        key::RETURN => execute_highlighted(fe),
        key::ESCAPE => close(fe),
        key::UP | key::DOWN => {
            let d = if code == key::UP { -1 } else { 1 };
            let p = &mut fe.game.ig.popup;
            if let (Some((_, items)), Some(_)) = (&p.sub, p.sub_highlight) {
                p.sub_highlight = step(items, p.sub_highlight, d);
            } else if let Some(i) = step(&p.items, p.highlight, d) {
                highlight_main(fe, i);
            }
        }
        key::RIGHT => {
            let p = &mut fe.game.ig.popup;
            if let Some((_, items)) = &p.sub {
                p.sub_highlight = step(items, None, 1);
            }
        }
        key::LEFT => fe.game.ig.popup.sub_highlight = None,
        _ => return false,
    }
    true
}

/// `pmDraw`: target outlines, the menu, then the open submenu.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    if !is_open(fe) {
        return;
    }
    let Some(f) = font(fe) else { return };
    // Ships the highlighted order applies to.
    if fe.game.ig.popup.highlight.is_some() {
        for &(_, x, y, r) in &fe.game.ig.view.ship_positions {
            canvas.outline_rect(Rect::new(x - r, y - r, x + r, y + r), TARGET_OUTLINE);
        }
    }
    if fe.game.ig.popup.open == Some(MenuKind::Special) {
        for &(x, y, r, ally) in &fe.game.ig.popup.target_boxes {
            let c = if ally { ALLY_TARGET } else { ENEMY_TARGET };
            canvas.outline_rect(Rect::new(x - r, y - r, x + r, y + r), c);
        }
    }
    let p = fe.game.ig.popup.clone();
    draw_menu(fe, canvas, &f, &p.items, p.highlight, None);
    if let Some((parent, items)) = &p.sub {
        let kind = p.items.get(*parent).and_then(|i| i.submenu);
        draw_menu(fe, canvas, &f, items, p.sub_highlight, kind);
    }
}

fn draw_menu(
    fe: &Fe,
    canvas: &mut Canvas,
    f: &hwc_formats::font::Font,
    items: &[Item],
    highlight: Option<usize>,
    sub: Option<Submenu>,
) {
    let Some(first) = items.first() else { return };
    let w = draw::width(f, " ");
    let h = crate::text::height(f);
    let border = 3 * w;
    let x0 = first.rect.x0;
    let x1 = first.rect.x1;
    let y0 = first.rect.y0 - h / 2;
    let y1 = items.last().map_or(y0, |i| i.rect.y1 + h / 2);
    let res = fe.resources().clone();
    let icons = &fe.game.ig.overlay.icons;
    // Box: fill, edges, corners outside the rectangle.
    canvas.blend_rect(
        Rect::new(x0 - border, y0 - border, x1 + border, y1 + border),
        FILL,
    );
    let tex = |n: &str| icons.image(&res, n);
    let pieces = [
        (
            "PopupMenu_BorderEdge_Top",
            Rect::new(x0, y0 - border, x1, y0),
        ),
        (
            "PopupMenu_BorderEdge_Bottom",
            Rect::new(x0, y1, x1, y1 + border),
        ),
        (
            "PopupMenu_BorderEdge_Left",
            Rect::new(x0 - border, y0, x0, y1),
        ),
        (
            "PopupMenu_BorderEdge_Right",
            Rect::new(x1, y0, x1 + border, y1),
        ),
        (
            "PopupMenu_BorderCorner_TopLeft",
            Rect::new(x0 - border, y0 - border, x0, y0),
        ),
        (
            "PopupMenu_BorderCorner_TopRight",
            Rect::new(x1, y0 - border, x1 + border, y0),
        ),
        (
            "PopupMenu_BorderCorner_BottomLeft",
            Rect::new(x0 - border, y1, x0, y1 + border),
        ),
        (
            "PopupMenu_BorderCorner_BottomRight",
            Rect::new(x1, y1, x1 + border, y1 + border),
        ),
    ];
    for (n, r) in pieces {
        if let Some(img) = tex(n) {
            draw::textured_rect(canvas, &img, r, WHITE);
        }
    }
    let (check, text_w, _) = columns(fe, f, items, sub);
    let cap = 3 * w;
    let over = w * 8 / 10;
    let t = (fe.time % 5.0) as f32 / 2.5;
    let flash_g = if t < 1.0 { t } else { 2.0 - t };
    for (i, it) in items.iter().enumerate() {
        let r = it.rect;
        if highlight == Some(i) {
            let (top, bot) = (r.y0 - h / 2, r.y1 + h / 2);
            if let (Some(l), Some(m), Some(rc)) = (
                tex("PopupMenu_MouseOverLeftEnd"),
                tex("PopupMenu_MouseOverMiddle"),
                tex("PopupMenu_MouseOverRightEnd"),
            ) {
                draw::textured_rect(
                    canvas,
                    &l,
                    Rect::new(r.x0 - over, top, r.x0 + cap, bot),
                    WHITE,
                );
                draw::textured_rect(
                    canvas,
                    &m,
                    Rect::new(r.x0 + cap, top, r.x1 - cap, bot),
                    WHITE,
                );
                draw::textured_rect(
                    canvas,
                    &rc,
                    Rect::new(r.x1 - cap, top, r.x1 + over, bot),
                    WHITE,
                );
            }
        }
        if it.checked {
            if let Some(img) = tex("PopupMenu_SelectedIndicator") {
                let s = check.min(h);
                draw::textured_rect(
                    canvas,
                    &img,
                    Rect::new(r.x0, r.y0, r.x0 + s, r.y0 + s),
                    WHITE,
                );
            }
        }
        let c = if it.title {
            TITLE
        } else if it.greyed {
            GREYED
        } else if it.flash {
            [255, (255.0 * flash_g) as u8, 0, 255]
        } else {
            WHITE
        };
        let tx = r.x0 + (2 * w * 4) / 10 / 2 + check;
        draw::print(canvas, f, tx, r.y0, c, &it.label);
        let kx = tx + text_w + 4 * w;
        if it.submenu.is_some() {
            let n = if it.greyed {
                "PopupMenu_SubMenuIndicator_Disabled"
            } else {
                "PopupMenu_SubMenuIndicator"
            };
            if let Some(img) = tex(n) {
                draw::textured_rect(canvas, &img, Rect::new(kx, r.y0, kx + h, r.y0 + h), WHITE);
            }
        } else if let Some(a) = it.action {
            draw::print(canvas, f, kx, r.y0, c, &key_names(fe, a));
        }
    }
}
