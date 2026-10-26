//! The main overlay (`MainOverlay.c`): the HUD text drawn over the 3D view. What each view
//! shows comes from `MainOverlay.script` (per single/multiplayer and view 1-3, "shown,backdrop"
//! for ten panels). Panels: selected ships (top right), ships in the band box, objectives
//! (bottom left), the mouse-over tooltip, game messages, player names (top left), construction
//! and research progress (left), support units / RUs / game time (bottom right), the
//! taskbar view buttons, the options menu, the key binding pages and "PAUSED".

use crate::canvas::{Canvas, Color, Rect, BLACK, WHITE};
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::icons::Icons;
use crate::ingame::keys::{self, Action};
use crate::ingame::{open_manager, Fe, GameRequest, Manager};

pub const FONT_LIST: &str = "SimplixSSK_13.hff";
pub const FONT_TEXT: &str = "default.hff";
pub const FONT_PAUSED: &str = "Arial_b17.hff";

/// Panels, in script / option order.
pub const PANELS: [&str; 10] = [
    "SELECTED",
    "SELECTING",
    "OBJECTIVES",
    "TOOLTIPS",
    "VIEWOPTIONS",
    "PLAYERINFO",
    "GAMEMESSAGE",
    "PLAYERNAMES",
    "CONSTRUCTION",
    "RESEARCH",
];
pub const SELECTED: usize = 0;
pub const SELECTING: usize = 1;
pub const OBJECTIVES: usize = 2;
pub const TOOLTIPS: usize = 3;
pub const PLAYERINFO: usize = 5;
pub const GAMEMESSAGE: usize = 6;
pub const PLAYERNAMES: usize = 7;
pub const CONSTRUCTION: usize = 8;
pub const RESEARCH: usize = 9;
/// Option menu keys '1'..'9' (VIEWOPTIONS is skipped).
pub const MENU_PANELS: [usize; 9] = [0, 1, 2, 3, 5, 6, 7, 8, 9];

/// Translucent panel backdrop.
pub const BACKDROP: Color = [50, 50, 50, 100];
const SHADOW: Color = [0, 0, 0, 0x40];
const GREEN: Color = [0, 255, 0, 255];
const RED: Color = [255, 0, 0, 255];
const YELLOW: Color = [255, 255, 0, 255];
const GREY_LIST: Color = [120, 120, 120, 255];
const ALLY_BLUE: Color = [0, 192, 255, 255];
const DESELECT: Color = [100, 100, 255, 255];
const LABEL_GREY: Color = [200, 200, 200, 255];
const BUILD_TEXT: Color = [91, 175, 37, 255];
const RESEARCH_TEXT: Color = [106, 175, 191, 255];
/// Messages: lines visible, seconds before the oldest starts fading, fade length.
pub const MESSAGE_LINES: usize = 5;
pub const MESSAGE_HOLD: f32 = 8.0;
pub const MESSAGE_FADE: f32 = 5.0;
const MESSAGE_FADES: [f32; 5] = [1.0, 0.8, 0.7, 0.6, 0.5];
pub const SOUND_OBJECTIVE_CLICK: u32 = 0x109;

/// Key binding page groups: (first action, title).
pub const KEY_GROUPS: [(u16, &str); 10] = [
    (1, "Actions"),
    (17, "S. Actions"),
    (44, "Tactics"),
    (49, "Formations"),
    (59, "Camera"),
    (72, "Selecting"),
    (79, "Views"),
    (86, "Misc."),
    (108, "Groupings"),
    (136, "Sensors"),
];

/// Shown / backdrop flags of one view.
pub type ViewOptions = [(bool, bool); 10];

#[derive(Debug, Clone, Default)]
pub struct State {
    /// [single player, multiplayer][view] options.
    pub options: [[ViewOptions; 3]; 2],
    /// Current view (0..2, script views 1..3).
    pub view: usize,
    /// Number keys edit backdrops instead of panels (`Change_Overlay_Backdrops_Toggle`).
    pub backdrop_mode: bool,
    /// Toggle_Overlay_Options is held: the options menu is up.
    pub options_held: bool,
    /// Show_Keys page held (1-3).
    pub keys_page: Option<u8>,
    pub tactical: bool,
    /// Ship list lines: (type, owner, rect) for clicks and hover.
    lines: Vec<(ShipType, Owner, Rect)>,
    /// Lines marked for deselection.
    pub deselect: Vec<(ShipType, Owner)>,
    /// Objective lines: (index, rect).
    objective_rects: Vec<(usize, Rect)>,
    /// Messages scrolled back by this many lines.
    pub message_scroll: usize,
    message_rects: Vec<(usize, Rect)>,
    /// View buttons: (rect, label, action).
    view_buttons: Vec<(Rect, String, Action)>,
    pub icons: Icons,
    mouse: (i32, i32),
    /// Seconds of real time (`taskTimeElapsed`), advanced by [`tick`].
    clock: f32,
    /// Where the top-left and top-right panels start (`moSetTop`): the bottom of the speaker
    /// portrait while one is up, then sliding back to 0.
    pub top: i32,
    /// When the subtitles last drew (the slide back up starts from here).
    top_stamp: f32,
    /// The modern RU / SU readout's eased bar fills.
    pub modern_hud: crate::ingame::modern_hud::Tween,
    /// The modern objectives panel's flash, and its header (the collapse toggle) as laid out.
    pub modern_objectives: crate::ingame::modern_objectives::PanelState,
    objectives_header: Option<Rect>,
    /// The modern resources panel's header (its collapse toggle) as laid out.
    resources_header: Option<Rect>,
    /// The modern build queue, system log and selected-ships panels: their state, and their
    /// headers as laid out.
    pub modern_build: crate::ingame::modern_build::PanelState,
    pub modern_log: crate::ingame::modern_log::PanelState,
    pub modern_selected: crate::ingame::modern_selected::PanelState,
    build_header: Option<Rect>,
    log_header: Option<Rect>,
    selected_header: Option<Rect>,
    /// Where the modern panels are this frame (they take clicks on them).
    modern_panels: Vec<Rect>,
    /// A cutscene is up (the letterbox, an NIS, fleet intel): the modern panels hide.
    pub cutscene: bool,
}

impl State {
    /// `moReadPrintOption` for every `[SP|MP]_MO_<view>_<PANEL>  shown,backdrop` line.
    pub fn load_script(&mut self, text: &str) {
        for line in text.lines() {
            let line = line.split("//").next().unwrap_or("");
            let mut f = line.split_whitespace();
            let (Some(name), Some(v)) = (f.next(), f.next()) else {
                continue;
            };
            let mut parts = name.splitn(4, '_');
            let (Some(mode), Some(_mo), Some(view), Some(panel)) =
                (parts.next(), parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let mode = match mode {
                "SP" => 0,
                "MP" => 1,
                _ => continue,
            };
            let Some(view) = view.parse::<usize>().ok().filter(|v| (1..=3).contains(v)) else {
                continue;
            };
            let Some(p) = PANELS.iter().position(|&n| n == panel) else {
                continue;
            };
            let Some((a, b)) = v.split_once(',') else {
                continue;
            };
            let num = |s: &str| s.trim().parse::<i32>().unwrap_or(0) != 0;
            self.options[mode][view - 1][p] = (num(a), num(b));
        }
    }

    /// `moSetTop`, called by the subtitle pass on every frame it draws something: a portrait
    /// moves the top below it, and either way the slide back up starts again from now.
    pub fn set_top(&mut self, hud: crate::subtitle::HudTop) {
        if !hud.drawn {
            return;
        }
        if let Some(y) = hud.portrait_bottom.filter(|&y| y != 0) {
            self.top = y;
        }
        self.top_stamp = self.clock;
    }

    /// `moDraw`'s slide back up: each frame the top moves up by the whole seconds (rounded)
    /// since the subtitles last drew, so it holds for half a second and then speeds up.
    fn settle_top(&mut self) {
        if self.top > 0 {
            let step = (self.clock - self.top_stamp).round_ties_even() as i32;
            self.top = (self.top - step).max(0);
        }
    }

    pub fn set_mouse(&mut self, x: i32, y: i32) {
        self.mouse = (x, y);
    }

    /// The pointer as last seen.
    pub fn mouse_pos(&self) -> (i32, i32) {
        self.mouse
    }

    pub fn load_icons(&mut self, text: &str) {
        self.icons = Icons::parse(text);
    }

    fn mode(&self, view: &HostView) -> usize {
        usize::from(!view.player.single_player)
    }

    /// Whether a panel is shown in the current view.
    pub fn shown(&self, view: &HostView, panel: usize) -> bool {
        self.options[self.mode(view)][self.view][panel].0
    }

    pub fn backdrop(&self, view: &HostView, panel: usize) -> bool {
        self.options[self.mode(view)][self.view][panel].1
    }
}

/// `moCyclePrintMode(1)`.
pub fn next_view(fe: &mut Fe) {
    let o = &mut fe.game.ig.overlay;
    o.view = (o.view + 1) % 3;
}

/// `moToggleDebugPrint` (really the backdrop edit mode).
pub fn toggle_backdrops(fe: &mut Fe) {
    let o = &mut fe.game.ig.overlay;
    o.backdrop_mode = !o.backdrop_mode;
}

pub fn toggle_options(fe: &mut Fe) {
    let o = &mut fe.game.ig.overlay;
    o.options_held = !o.options_held;
}

pub fn show_keys(fe: &mut Fe, page: u8) {
    fe.game.ig.overlay.keys_page = Some(page);
}

pub fn toggle_tactical(fe: &mut Fe) {
    let o = &mut fe.game.ig.overlay;
    o.tactical = !o.tactical;
}

/// `moTogglePrintOption`: digit 1-9 while the options key is held.
pub fn toggle_option(fe: &mut Fe, digit: u8) {
    let Some(&panel) = MENU_PANELS.get(usize::from(digit).wrapping_sub(1)) else {
        return;
    };
    let mode = fe.game.ig.overlay.mode(&fe.game.ig.view);
    let o = &mut fe.game.ig.overlay;
    let v = o.view;
    let entry = &mut o.options[mode][v][panel];
    if o.backdrop_mode {
        entry.1 = !entry.1;
    } else {
        entry.0 = !entry.0;
    }
}

pub fn tick(fe: &mut Fe, dt: f32) {
    let max = fe.game.ig.view.messages.len().saturating_sub(MESSAGE_LINES);
    let o = &mut fe.game.ig.overlay;
    o.clock += dt;
    o.message_scroll = o.message_scroll.min(max);
    layout(fe);
}

/// Top of the bottom-of-screen area: objectives and player info push it up (the objectives
/// only in the original layout: the modern HUD has them top left).
fn bar_top(st: &State, view: &HostView, h: i32, th: i32, modern: bool) -> i32 {
    let mut bar = h;
    if st.shown(view, OBJECTIVES) && !view.objectives.is_empty() && !modern {
        bar = h - (view.objectives.len() as i32 * th + th + 24);
    }
    if st.shown(view, PLAYERINFO) {
        bar = bar.min(h - th - 30) - th;
    }
    bar.min(h - 50)
}

/// `moUpdateMouseOver`: where the clickable lines and buttons are this frame, from the
/// state alone (clicks must not depend on a frame having been drawn).
pub fn layout(fe: &mut Fe) {
    let ig = &mut fe.game.ig;
    // The objectives panel keeps the objectives the scripts destroy.
    let v = &ig.view;
    ig.overlay
        .modern_objectives
        .remember(v.player.mission, v.time, &v.objectives);
    // The system log flashes its folded header on a new message.
    let clock = ig.overlay.clock;
    ig.overlay.modern_log.note(clock, &v.messages);
    // A fold chosen over a full-screen manager lasts until it closes.
    if !crate::ingame::full_screen_manager(ig.manager) {
        let o = &mut ig.overlay;
        o.modern_objectives.over_manager = None;
        o.modern_hud.over_manager = None;
        o.modern_build.over_manager = None;
        o.modern_log.over_manager = None;
        o.modern_selected.over_manager = None;
    }
    let (w, h) = fe.screen_size();
    let view = &fe.game.ig.view;
    let st = &fe.game.ig.overlay;
    let (Some(tf), Some(lf)) = (font(fe, FONT_TEXT), font(fe, FONT_LIST)) else {
        return;
    };
    let th = crate::text::height(&tf);
    let lh = crate::text::height(&lf);
    let modern = crate::ingame::modern_hud::enabled(&fe.game.options);
    let bar = bar_top(st, view, h, th, modern);
    // The modern panels (none with the original HUD or in a cutscene).
    let arr = crate::ingame::modern_panel::arrange(fe, w, h, view);
    let mut lines = Vec::new();
    if modern {
        // The selected-ships panel's rows stand for the original's lines.
        if let Some(l) = &arr.selected {
            let g = crate::ingame::modern_selected::groups(&view.selection);
            for r in &l.rows {
                if let Some(g) = g.get(r.index) {
                    lines.push((g.ship_type, g.owner, r.rect));
                }
            }
        }
    } else if st.shown(view, SELECTED) {
        for (i, (t, owner, _, _)) in groups(&view.selection).into_iter().enumerate() {
            let y = st.top + 10 + i as i32 * lh;
            lines.push((t, owner, Rect::new(w - 150, y, w, y + lh)));
        }
    }
    let mut objective_rects = Vec::new();
    let objectives_header = arr.objectives.as_ref().map(|p| p.header);
    let resources_header = arr.resources.as_ref().map(|p| p.header);
    let modern_panels = arr.rects();
    if modern {
        if let Some(p) = &arr.objectives {
            // Destroyed objectives are not the game's any more: no click.
            objective_rects = p
                .items
                .iter()
                .filter_map(|i| i.live.map(|l| (l, i.rect)))
                .collect();
        }
    } else if st.shown(view, OBJECTIVES) && !view.objectives.is_empty() {
        let mut y = bar + th + 4;
        for (i, o) in view.objectives.iter().enumerate() {
            let s = objective_text(o);
            objective_rects.push((i, Rect::new(10, y, 10 + draw::width(&tf, &s), y + th)));
            y += th;
        }
    }
    let mut message_rects = Vec::new();
    if modern {
        // The system log's messages about a ship focus it, as the original's lines do.
        if let Some(l) = &arr.log {
            message_rects = l.items.iter().map(|i| (i.message, i.rect)).collect();
        }
    } else if st.shown(view, GAMEMESSAGE) {
        let base = bar.min(h);
        let scroll = st.message_scroll;
        let newest = view.messages.len().saturating_sub(1 + scroll);
        for k in 0..MESSAGE_LINES {
            let Some(i) = newest.checked_sub(k) else {
                break;
            };
            let Some(m) = view.messages.get(i) else { break };
            if scroll == 0 && view.time - m.time >= MESSAGE_HOLD + MESSAGE_FADE {
                continue;
            }
            let y = base - 2 * th - k as i32 * th;
            message_rects.push((i, Rect::new(17, y, 17 + draw::width(&tf, &m.text), y + th)));
        }
    }
    let view_buttons = view_button_defs(fe, w, bar);
    let o = &mut fe.game.ig.overlay;
    o.lines = lines;
    o.objective_rects = objective_rects;
    o.objectives_header = objectives_header;
    o.resources_header = resources_header;
    o.build_header = arr.build.as_ref().map(|p| p.header);
    o.log_header = arr.log.as_ref().map(|p| p.header);
    o.selected_header = arr.selected.as_ref().map(|p| p.header);
    o.modern_panels = modern_panels;
    o.message_rects = message_rects;
    o.view_buttons = view_buttons;
}

/// The taskbar view buttons (option "Taskbar buttons"): (rect, label, action).
fn view_button_defs(fe: &Fe, width: i32, bar: i32) -> Vec<(Rect, String, Action)> {
    let view = &fe.game.ig.view;
    if !fe.game.options.get_bool("ShowTaskbarButtons") || view.player.tutorial {
        return Vec::new();
    }
    let x0 = (width / 2 - 93).max(0);
    let sensors = if view.player.in_sensors {
        "Main"
    } else {
        "Sensors"
    };
    vec![
        (
            Rect::new(x0, bar + 5, x0 + 90, bar + 25),
            sensors.into(),
            keys::TOGGLE_SENSORS_MANAGER,
        ),
        (
            Rect::new(x0, bar + 28, x0 + 90, bar + 48),
            "Build...".into(),
            keys::OPEN_BUILD_MANAGER,
        ),
        (
            Rect::new(x0 + 96, bar + 5, x0 + 186, bar + 25),
            "Research...".into(),
            keys::OPEN_RESEARCH_MANAGER,
        ),
        (
            Rect::new(x0 + 96, bar + 28, x0 + 186, bar + 48),
            "Systems...".into(),
            keys::OPEN_LAUNCH_MANAGER,
        ),
    ]
}

/// Mouse clicks on the overlay; returns whether one was used.
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool, x: i32, y: i32) -> bool {
    fe.game.ig.overlay.mouse = (x, y);
    layout(fe);
    if !left {
        return false;
    }
    // Sensors pan arrows: held while the button is down.
    if fe.game.ig.view.player.in_sensors {
        for (rect, action) in pan_arrows(fe) {
            if rect.contains(x, y) {
                fe.game.ig.request(GameRequest::Action { action, down });
                return true;
            }
        }
    }
    if !down {
        return false;
    }
    if modern_click(fe, x, y) {
        return true;
    }
    let o = &fe.game.ig.overlay;
    if let Some(&(t, owner, _)) = o.lines.iter().find(|l| l.2.contains(x, y)) {
        click_line(fe, t, owner);
        return true;
    }
    if let Some(&(i, _)) = o.objective_rects.iter().find(|l| l.1.contains(x, y)) {
        click_objective(fe, i);
        return true;
    }
    if let Some(&(i, _)) = o.message_rects.iter().find(|l| l.1.contains(x, y)) {
        if click_message(fe, i) {
            return true;
        }
    }
    let o = &fe.game.ig.overlay;
    if let Some((_, _, a)) = o.view_buttons.iter().find(|b| b.0.contains(x, y)).cloned() {
        crate::ingame::execute_action(fe, a, true);
        return true;
    }
    false
}

/// A click on a line of the selected-ship list (`moProcessClick`): it selects that type, or
/// with the multiple-select key held marks it to leave the selection on the key's release.
fn click_line(fe: &mut Fe, t: ShipType, owner: Owner) {
    let multi = fe.input.down.iter().any(|k| {
        fe.game
            .ig
            .keys
            .of(keys::MULTIPLE_SELECT)
            .iter()
            .any(|b| b.key == *k)
    });
    if multi {
        let d = &mut fe.game.ig.overlay.deselect;
        if !d.contains(&(t, owner)) {
            d.push((t, owner));
        }
    } else {
        fe.game.ig.request(GameRequest::SelectType {
            ship_type: t,
            owner,
        });
    }
}

/// A click on message `i`: one about a ship focuses it. Whether it did.
fn click_message(fe: &mut Fe, i: usize) -> bool {
    let Some(ship) = fe.game.ig.view.messages.get(i).and_then(|m| m.ship) else {
        return false;
    };
    fe.game.ig.request(GameRequest::Sound(0x108));
    fe.game.ig.request(GameRequest::FocusShip(ship));
    true
}

/// `moProcessClick` on objective `i` of the game's list: the hyperspace entry jumps, the
/// others open the objectives screen on it (nothing while the game is paused).
fn click_objective(fe: &mut Fe, i: usize) {
    if fe.game.ig.view.player.paused {
        return;
    }
    fe.game
        .ig
        .request(GameRequest::Sound(SOUND_OBJECTIVE_CLICK));
    let hyper = fe
        .game
        .ig
        .view
        .objectives
        .get(i)
        .is_some_and(|o| o.hyperspace);
    if hyper {
        crate::ingame::execute_action(fe, keys::HYPERSPACE, true);
    } else {
        fe.game.ig.esc.objective = Some(i);
        open_manager(fe, Manager::Objectives);
    }
}

/// Whether `(x, y)` is on one of the modern panels as laid out by [`layout`].
pub fn over_modern_panel(fe: &Fe, x: i32, y: i32) -> bool {
    fe.game
        .ig
        .overlay
        .modern_panels
        .iter()
        .any(|r| r.contains(x, y))
}

/// A left press on the modern panels (from the overlay's clicks, or over a full-screen
/// manager): a header folds its panel, an objective line acts as in the original, anywhere
/// else on a panel the click is used up. Whether it was on a panel.
pub fn modern_click(fe: &mut Fe, x: i32, y: i32) -> bool {
    let o = &fe.game.ig.overlay;
    if o.objectives_header.is_some_and(|r| r.contains(x, y)) {
        crate::ingame::modern_objectives::toggle(fe);
        layout(fe);
        return true;
    }
    if o.resources_header.is_some_and(|r| r.contains(x, y)) {
        crate::ingame::modern_hud::toggle(fe);
        layout(fe);
        return true;
    }
    type Toggle = (Option<Rect>, fn(&mut Fe));
    let toggles: [Toggle; 3] = [
        (o.build_header, crate::ingame::modern_build::toggle),
        (o.log_header, crate::ingame::modern_log::toggle),
        (o.selected_header, crate::ingame::modern_selected::toggle),
    ];
    if let Some((_, toggle)) = toggles
        .iter()
        .find(|(r, _)| r.is_some_and(|r| r.contains(x, y)))
    {
        toggle(fe);
        layout(fe);
        return true;
    }
    if !over_modern_panel(fe, x, y) {
        return false;
    }
    let o = &fe.game.ig.overlay;
    if let Some(&(i, _)) = o.objective_rects.iter().find(|l| l.1.contains(x, y)) {
        click_objective(fe, i);
    } else if let Some(&(t, owner, _)) = o.lines.iter().find(|l| l.2.contains(x, y)) {
        click_line(fe, t, owner);
    } else if let Some(&(i, _)) = o.message_rects.iter().find(|l| l.1.contains(x, y)) {
        click_message(fe, i);
    }
    true
}

/// The modern panels over a full-screen manager (the overlay is not drawn under those).
pub fn draw_modern_panels(fe: &mut Fe, canvas: &mut Canvas) {
    if !crate::ingame::modern_hud::enabled(&fe.game.options) || fe.game.ig.overlay.cutscene {
        return;
    }
    let view = fe.game.ig.view.clone();
    let st = &fe.game.ig.overlay;
    let (clock, mouse) = (st.clock, st.mouse);
    let (objectives, info) = (st.shown(&view, OBJECTIVES), st.shown(&view, PLAYERINFO));
    let arr = crate::ingame::modern_panel::arrange(fe, canvas.width, canvas.height, &view);
    if objectives {
        crate::ingame::modern_objectives::draw(fe, canvas, &view, clock, mouse);
    }
    if info {
        crate::ingame::modern_hud::draw(fe, canvas, &view, clock, mouse);
    }
    draw_modern_lists(fe, canvas, &arr, &view);
}

/// The modern build queue, system log and selected ships, as arranged.
fn draw_modern_lists(
    fe: &Fe,
    canvas: &mut Canvas,
    arr: &crate::ingame::modern_panel::Arrangement,
    view: &HostView,
) {
    let (clock, mouse) = (fe.game.ig.overlay.clock, fe.game.ig.overlay.mouse);
    if let Some(l) = &arr.build {
        crate::ingame::modern_build::draw(fe, canvas, l, view, mouse);
    }
    if let Some(l) = &arr.log {
        crate::ingame::modern_log::draw(fe, canvas, l, view, clock);
    }
    if let Some(l) = &arr.selected {
        crate::ingame::modern_selected::draw(fe, canvas, l, &view.selection);
    }
}

/// An objective's line (`moDrawMissionInfo`): "COMPLETE - " or "INCOMPLETE - " before the
/// text, except the hyperspace entry, which shows its label alone.
fn objective_text(o: &crate::ingame::host::Objective) -> String {
    if o.hyperspace {
        return o.text.clone();
    }
    let state = if o.complete { "COMPLETE" } else { "INCOMPLETE" };
    format!("{state} - {}", o.text)
}

/// Multiple_Select released: the marked lines leave the selection
/// (`moShipListApplyDeselect`).
pub fn apply_deselect(fe: &mut Fe) {
    let d = std::mem::take(&mut fe.game.ig.overlay.deselect);
    if !d.is_empty() {
        fe.game.ig.request(GameRequest::DeselectTypes(d));
    }
}

fn font(fe: &Fe, name: &str) -> Option<std::rc::Rc<hwc_formats::font::Font>> {
    fe.resources().font(name)
}

fn text(c: &mut Canvas, f: &hwc_formats::font::Font, x: i32, y: i32, col: Color, s: &str) -> i32 {
    draw::print_shadow(c, f, x, y, col, SHADOW, s)
}

/// Grows `r` to cover a text box with a margin (`moPrintAndExtendRect`).
fn extend(r: &mut Option<Rect>, x: i32, y: i32, w: i32, h: i32) {
    let t = Rect::new(x - 5, y - 5, x + w + 5, y + h + 5);
    *r = Some(match *r {
        None => t,
        Some(a) => Rect::new(
            a.x0.min(t.x0),
            a.y0.min(t.y0),
            a.x1.max(t.x1),
            a.y1.max(t.y1),
        ),
    });
}

/// Health colour by fraction (ship info and status bars).
pub fn health_colour(f: f32) -> Color {
    if f >= 0.5 {
        GREEN
    } else if f >= 0.25 {
        YELLOW
    } else {
        RED
    }
}

/// The overlay over the 3D view.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let (w, h) = (canvas.width, canvas.height);
    let view = fe.game.ig.view.clone();
    let st = &fe.game.ig.overlay;
    let Some(tf) = font(fe, FONT_TEXT) else {
        return;
    };
    let th = crate::text::height(&tf);
    let modern = crate::ingame::modern_hud::enabled(&fe.game.options);
    let bar = bar_top(st, &view, h, th, modern);
    let options = fe.game.options.clone();
    if options.get_bool("ShowTaskbarBackdrop") && !view.player.tutorial {
        canvas.blend_rect(Rect::new(0, bar, w, h), BACKDROP);
    }
    // The modern panels (none with the original HUD or in a cutscene).
    let arr = crate::ingame::modern_panel::arrange(fe, w, h, &view);
    let list_h = font(fe, FONT_LIST).map_or(0, |f| crate::text::height(&f));
    let mut lines = Vec::new();
    if st.shown(&view, SELECTED) && !modern {
        let bd = st.backdrop(&view, SELECTED);
        draw_ship_list(fe, canvas, &view.selection, false, bd, st.top + 10, &mut lines);
    }
    let st = &fe.game.ig.overlay;
    if st.shown(&view, SELECTING) {
        let bd = st.backdrop(&view, SELECTING);
        // Under the selected ships: the modern panels, or the original's list.
        let y = if modern {
            arr.selected
                .as_ref()
                .map(|l| l.below)
                .or(arr.resources.as_ref().map(|l| l.bottom))
                .unwrap_or(st.top)
                + 10
        } else {
            st.top + 10 + groups(&view.selection).len() as i32 * list_h + 10
        };
        draw_ship_list(fe, canvas, &view.selecting, true, bd, y, &mut Vec::new());
    }
    let _ = lines;
    let st = &fe.game.ig.overlay;
    let mut objective_rects = Vec::new();
    if st.shown(&view, OBJECTIVES) && modern {
        let (clock, mouse) = (st.clock, st.mouse);
        if !st.cutscene {
            crate::ingame::modern_objectives::draw(fe, canvas, &view, clock, mouse);
        }
    } else if st.shown(&view, OBJECTIVES) && !view.objectives.is_empty() {
        let mut y = bar;
        let mut widest = 0;
        let (mx, my) = st.mouse;
        let header = "Objectives:";
        widest = widest.max(draw::width(&tf, header));
        let mut texts = Vec::new();
        for (i, o) in view.objectives.iter().enumerate() {
            let s = objective_text(o);
            widest = widest.max(draw::width(&tf, &s));
            texts.push((i, s, o.complete));
        }
        if !options.get_bool("ShowTaskbarBackdrop") {
            canvas.blend_rect(Rect::new(0, bar - 5, widest + 15, h), BACKDROP);
        }
        text(canvas, &tf, 10, y, WHITE, header);
        y += th + 4;
        for (i, s, complete) in texts {
            let r = Rect::new(10, y, 10 + draw::width(&tf, &s), y + th);
            let c = if complete {
                GREEN
            } else if r.contains(mx, my) {
                WHITE
            } else {
                RED
            };
            text(canvas, &tf, 10, y, c, &s);
            objective_rects.push((i, r));
            y += th;
        }
    }
    let _ = objective_rects;
    let st = &fe.game.ig.overlay;
    if st.shown(&view, TOOLTIPS) {
        if let Some(t) = &view.tooltip {
            draw_tooltip(fe, canvas, t);
        }
    }
    let st = &fe.game.ig.overlay;
    if st.shown(&view, GAMEMESSAGE) && !modern {
        draw_messages(fe, canvas, &view, bar.min(h));
    }
    let st = &fe.game.ig.overlay;
    // The modern objectives panel takes the top-left corner: these columns go under it, the
    // player names under the build queue too.
    let names = st.shown(&view, PLAYERNAMES) && !view.player_names.is_empty();
    let below_objectives = arr.objectives.as_ref().map_or(st.top, |p| p.below.max(st.top));
    let left_top = arr.left_below().map_or(st.top, |y| y.max(st.top));
    let mut left_y = left_top + 20;
    if names {
        left_y = draw_player_names(fe, canvas, &view, left_top);
    }
    let st = &fe.game.ig.overlay;
    let bottom = bar + 5 * th + 10;
    // Where the original's columns would go under the objectives panel alone.
    let natural_y = if names {
        below_objectives + 20 + view.player_names.len() as i32 * (th + 2)
    } else {
        below_objectives + 20
    };
    let mid = natural_y + (bottom - natural_y) / 2;
    if st.shown(&view, CONSTRUCTION) && !modern {
        let bd = st.backdrop(&view, CONSTRUCTION);
        draw_construction(fe, canvas, &view, left_y + 32, mid - 17, bd);
    }
    let st = &fe.game.ig.overlay;
    if st.shown(&view, RESEARCH) {
        let bd = st.backdrop(&view, RESEARCH);
        // The research list keeps its place unless the build queue reaches it, and ends
        // above the system log.
        let top = (mid + 32).max(left_y + 12);
        let end = arr
            .log
            .as_ref()
            .map_or(bottom - 17, |l| (bottom - 17).min(l.panel.y0 - 4));
        draw_research(fe, canvas, &view, top, end, bd);
    }
    draw_modern_lists(fe, canvas, &arr, &view);
    let st = &fe.game.ig.overlay;
    if st.shown(&view, PLAYERINFO) {
        if crate::ingame::modern_hud::enabled(&options) {
            let (clock, mouse) = (st.clock, st.mouse);
            if !st.cutscene {
                crate::ingame::modern_hud::draw(fe, canvas, &view, clock, mouse);
            }
        } else {
            let bd = !options.get_bool("ShowTaskbarBackdrop");
            draw_player_info(fe, canvas, &view, bar, bd);
        }
    }
    draw_view_buttons(fe, canvas, bar);
    if view.player.in_sensors {
        draw_pan_arrows(fe, canvas);
    }
    if fe.game.ig.overlay.options_held && !crate::ingame::shipprint::visible(fe) {
        draw_options_menu(fe, canvas, &view);
    }
    if view.player.paused && fe.game.ig.overlay.view == 0 {
        if let Some(pf) = font(fe, FONT_PAUSED) {
            draw::print_centred(canvas, &pf, w / 2, h / 4, WHITE, "PAUSED");
        }
    }
    if let Some(page) = fe.game.ig.overlay.keys_page {
        draw_key_page(fe, canvas, page);
    }
    fe.game.ig.overlay.settle_top();
}

/// `moDrawShipList` / `moDrawPlayersShipList`: "3 x ACOLYTE" per type, top right.
fn draw_ship_list(
    fe: &Fe,
    canvas: &mut Canvas,
    ships: &[ShipInfo],
    selecting: bool,
    backdrop: bool,
    top: i32,
    lines: &mut Vec<(ShipType, Owner, Rect)>,
) {
    let Some(f) = font(fe, FONT_LIST) else { return };
    let h = crate::text::height(&f);
    let st = &fe.game.ig.overlay;
    let x = canvas.width - 150;
    let mut y = top;
    let groups = groups(ships);
    let mut rect = None;
    for &(_, _, n, ref name) in &groups {
        extend(
            &mut rect,
            x,
            y,
            draw::width(&f, &format!("{n} x {name}")),
            h,
        );
        y += h;
    }
    if backdrop {
        if let Some(r) = rect {
            canvas.blend_rect(r, BACKDROP);
        }
    }
    y -= groups.len() as i32 * h;
    let (mx, my) = st.mouse;
    for (t, owner, n, name) in groups {
        let r = Rect::new(x, y, canvas.width, y + h);
        let c = if selecting {
            match owner {
                Owner::Own => GREEN,
                Owner::Enemy => RED,
                _ => ALLY_BLUE,
            }
        } else if st.deselect.contains(&(t, owner)) {
            DESELECT
        } else if r.contains(mx, my) {
            GREEN
        } else {
            match owner {
                Owner::Disabled => GREY_LIST,
                Owner::Ally => ALLY_BLUE,
                _ => WHITE,
            }
        };
        text(canvas, &f, x, y, c, &format!("{n} x {name}"));
        lines.push((t, owner, r));
        y += h;
    }
}

/// Ships grouped by (type, owner), in type order.
fn groups(ships: &[ShipInfo]) -> Vec<(ShipType, Owner, usize, String)> {
    let mut out: Vec<(ShipType, Owner, usize, String)> = Vec::new();
    for s in ships {
        let key = (s.ship_type, s.owner);
        match out.iter_mut().find(|g| (g.0, g.1) == key) {
            Some(g) => g.2 += 1,
            None => out.push((s.ship_type, s.owner, 1, s.name.to_uppercase())),
        }
    }
    out.sort_by_key(|g| (g.0, g.1 as u8));
    out
}

/// `moDrawSelectedInfo`: the box under the mouse pointer.
fn draw_tooltip(fe: &Fe, canvas: &mut Canvas, t: &Tooltip) {
    let Some(f) = font(fe, FONT_LIST) else { return };
    let h = crate::text::height(&f);
    let (mx, my) = fe.game.ig.overlay.mouse;
    let (tx, ty) = (mx - 5, my + 30);
    let (txt, fill, outline) = match t.kind {
        TooltipKind::Resource => (
            [233, 193, 58, 255],
            [85, 200, 95, 100],
            [160, 160, 160, 255],
        ),
        TooltipKind::Missile => (RED, [100, 100, 100, 100], RED),
        TooltipKind::Enemy => (RED, [150, 100, 75, 100], [160, 160, 160, 255]),
        TooltipKind::Own => (GREEN, [75, 100, 75, 100], [160, 160, 160, 255]),
        TooltipKind::Ally => (ALLY_BLUE, [0, 0, 0, 100], [160, 160, 160, 255]),
    };
    let r = Rect::new(
        mx - 10,
        my + 25,
        tx + draw::width(&f, &t.text) + 5,
        ty + h + 5,
    );
    canvas.blend_rect(r, fill);
    canvas.outline_rect(r, outline);
    canvas.outline_rect(Rect::new(r.x0 + 1, r.y0 + 1, r.x1 - 1, r.y1 - 1), outline);
    text(canvas, &f, tx, ty, txt, &t.text);
}

/// `moDrawMessages`: newest at the bottom, older lines fading, timing out after 13 s.
fn draw_messages(fe: &mut Fe, canvas: &mut Canvas, view: &HostView, base: i32) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    let scroll = fe.game.ig.overlay.message_scroll;
    let n = view.messages.len();
    let newest = n.saturating_sub(1 + scroll);
    let mut rects = Vec::new();
    let (mx, my) = fe.game.ig.overlay.mouse;
    for (k, &line_fade) in MESSAGE_FADES.iter().enumerate().take(MESSAGE_LINES) {
        let Some(i) = newest.checked_sub(k) else {
            break;
        };
        let Some(m) = view.messages.get(i) else { break };
        let mut fade = line_fade;
        if scroll == 0 {
            let age = view.time - m.time;
            if age >= MESSAGE_HOLD + MESSAGE_FADE {
                continue;
            }
            if age > MESSAGE_HOLD {
                fade *= 1.0 - (age - MESSAGE_HOLD) / MESSAGE_FADE;
            }
        }
        let y = base - 2 * h - k as i32 * h;
        let r = Rect::new(17, y, 17 + draw::width(&f, &m.text), y + h);
        let mut c = m.colour;
        if m.ship.is_some() && r.contains(mx, my) {
            c = if c == GREEN { WHITE } else { GREEN };
        }
        let c = [
            (c[0] as f32 * fade) as u8,
            (c[1] as f32 * fade) as u8,
            (c[2] as f32 * fade) as u8,
            255,
        ];
        text(canvas, &f, 17, y, c, &m.text);
        rects.push((i, r));
    }
    // Scroll buttons.
    let icons = &fe.game.ig.overlay.icons;
    let res = fe.resources().clone();
    if n > MESSAGE_LINES {
        let up = Rect::new(
            3,
            base - 2 * h - (MESSAGE_LINES as i32 - 1) * h,
            14,
            base - h * 2 - (MESSAGE_LINES as i32 - 1) * h + 14,
        );
        let name = if up.contains(mx, my) {
            "Buffer_Up_Mouse"
        } else {
            "Buffer_Up"
        };
        icons.draw(&res, canvas, name, up.x0 - 10, up.y0 - 9);
    }
    if scroll > 0 {
        let down = Rect::new(3, base - 2 * h, 14, base - 2 * h + 14);
        let name = if down.contains(mx, my) {
            "Buffer_Down_Mouse"
        } else {
            "Buffer_Down"
        };
        icons.draw(&res, canvas, name, down.x0 - 10, down.y0 - 9);
    }
    let _ = rects;
}

/// `moDrawPlayerStats`: colour swatches and names; returns the next free y.
fn draw_player_names(fe: &Fe, canvas: &mut Canvas, view: &HostView, top: i32) -> i32 {
    let Some(f) = font(fe, FONT_TEXT) else {
        return top + 20;
    };
    let h = crate::text::height(&f);
    let mut y = top + 20;
    for (name, colour) in &view.player_names {
        canvas.fill_rect(Rect::new(10, y, 10 + h, y + h), *colour);
        text(canvas, &f, h + 14, y, *colour, name);
        y += h + 2;
    }
    y
}

/// Construction progress per ship type, one count per factory.
fn draw_construction(
    fe: &Fe,
    canvas: &mut Canvas,
    view: &HostView,
    top: i32,
    bottom: i32,
    bd: bool,
) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    let mut types: Vec<ShipType> = Vec::new();
    for j in &view.jobs {
        if !types.contains(&j.ship_type) {
            types.push(j.ship_type);
        }
    }
    if types.is_empty() {
        return;
    }
    if bd {
        canvas.blend_rect(
            Rect::new(
                5,
                top - 5,
                165,
                (top + types.len() as i32 * h + 5).min(bottom),
            ),
            BACKDROP,
        );
    }
    let factories: Vec<ShipId> = {
        let mut v: Vec<&Factory> = view.factories.iter().collect();
        v.sort_by_key(|f| !f.mothership);
        v.iter().map(|f| f.ship).collect()
    };
    let res = fe.resources().clone();
    let bar_img = fe.game.ig.overlay.icons.image(&res, "Build_ProgressBar");
    let mut y = top;
    for t in types {
        if y + h > bottom {
            break;
        }
        let jobs: Vec<&BuildJob> = view.jobs.iter().filter(|j| j.ship_type == t).collect();
        let counts: Vec<String> = factories
            .iter()
            .map(|fid| {
                jobs.iter()
                    .find(|j| j.factory == *fid)
                    .map_or(0, |j| j.count)
                    .to_string()
            })
            .take(
                jobs.len().max(1).max(
                    factories
                        .iter()
                        .filter(|f| jobs.iter().any(|j| j.factory == **f))
                        .count(),
                ),
            )
            .collect();
        let name = view
            .build_list
            .iter()
            .find_map(|r| match r {
                BuildRow::Ship(o) if o.ship_type == t => Some(o.name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| format!("SHIP {t}"));
        let progress = jobs.iter().map(|j| j.progress).fold(0.0f32, f32::max);
        if let Some(img) = &bar_img {
            draw::textured_rect(canvas, img, Rect::new(10, y - 1, 160, y - 1 + h), WHITE);
            let x = 10 + (progress.clamp(0.0, 1.0) * 150.0) as i32;
            canvas.blend_rect(Rect::new(x, y - 1, 160, y - 1 + h), [0, 0, 0, 75]);
        }
        text(
            canvas,
            &f,
            10,
            y,
            BUILD_TEXT,
            &format!("{name} ({})", counts.join("/")),
        );
        y += h;
    }
}

/// Research in progress.
fn draw_research(fe: &Fe, canvas: &mut Canvas, view: &HostView, top: i32, bottom: i32, bd: bool) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    let topics: Vec<(String, f32)> = view
        .labs
        .iter()
        .filter(|l| l.status == LabStatus::Researching)
        .filter_map(|l| Some((view.tech(l.topic?)?.name.clone(), l.progress)))
        .collect();
    if topics.is_empty() {
        return;
    }
    if bd {
        canvas.blend_rect(
            Rect::new(
                5,
                top - 5,
                165,
                (top + topics.len() as i32 * h + 5).min(bottom),
            ),
            BACKDROP,
        );
    }
    let res = fe.resources().clone();
    let bar_img = fe.game.ig.overlay.icons.image(&res, "Research_ProgressBar");
    let mut y = top;
    for (name, p) in topics {
        if y + h > bottom {
            break;
        }
        if let Some(img) = &bar_img {
            draw::textured_rect(canvas, img, Rect::new(10, y - 1, 160, y + h - 4), WHITE);
            let x = 10 + (p.clamp(0.0, 1.0) * 150.0) as i32;
            canvas.blend_rect(Rect::new(x, y - 1, 160, y + h - 4), [0, 0, 0, 75]);
        }
        text(canvas, &f, 10, y, RESEARCH_TEXT, &name);
        y += h;
    }
}

/// `moDrawRUsAndTime`: SU / RU / game time, bottom right.
fn draw_player_info(fe: &Fe, canvas: &mut Canvas, view: &HostView, bar: i32, bd: bool) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    let w = canvas.width;
    let x = w - 10 - draw::width(&f, "SU: 000 / 000");
    if bd {
        canvas.blend_rect(Rect::new(x - 5, bar - 5, w, canvas.height), BACKDROP);
    }
    let p = &view.player;
    let mut y = bar;
    let mut cx = x + text(canvas, &f, x, y, LABEL_GREY, "SU: ");
    let su_col = if p.support_used <= p.support_capacity {
        GREEN
    } else {
        // Over the cap: red to yellow and back every three seconds.
        let t = (view.time % 3.0) / 1.5;
        let g = if t < 1.0 { t } else { 2.0 - t };
        [255, (255.0 * g) as u8, 0, 255]
    };
    cx += text(canvas, &f, cx, y, su_col, &p.support_used.to_string());
    text(
        canvas,
        &f,
        cx,
        y,
        WHITE,
        &format!(" / {}", p.support_capacity),
    );
    y += h;
    let cx = x + text(canvas, &f, x, y, LABEL_GREY, "RU: ");
    text(canvas, &f, cx, y, WHITE, &p.resource_units.to_string());
    y += h + 10;
    let t = view.time.max(0.0) as u32;
    text(
        canvas,
        &f,
        x,
        y,
        WHITE,
        &format!("{:02}:{:02}:{:02}", t / 3600, t / 60 % 60, t % 60),
    );
}

/// The taskbar view buttons (option "Taskbar buttons").
fn draw_view_buttons(fe: &mut Fe, canvas: &mut Canvas, bar: i32) {
    let res = fe.resources().clone();
    let f = font(fe, FONT_TEXT);
    let (mx, my) = fe.game.ig.overlay.mouse;
    for (r, label, _) in view_button_defs(fe, canvas.width, bar) {
        let m = if r.contains(mx, my) { "_Mouse" } else { "" };
        let icons = &fe.game.ig.overlay.icons;
        if let (Some(l), Some(mid), Some(rt)) = (
            icons.image(&res, &format!("Stats_Exit_Button_Left{m}")),
            icons.image(&res, &format!("Stats_Exit_Button_Mid{m}")),
            icons.image(&res, &format!("Stats_Exit_Button_Right{m}")),
        ) {
            let cap = r.height();
            draw::textured_rect(canvas, &l, Rect::new(r.x0, r.y0, r.x0 + cap, r.y1), WHITE);
            draw::textured_rect(
                canvas,
                &mid,
                Rect::new(r.x0 + cap, r.y0, r.x1 - cap, r.y1),
                WHITE,
            );
            draw::textured_rect(canvas, &rt, Rect::new(r.x1 - cap, r.y0, r.x1, r.y1), WHITE);
        }
        if let Some(f) = &f {
            let y = r.y0 + (r.height() - crate::text::height(f)) / 2;
            draw::print_centred(canvas, f, (r.x0 + r.x1) / 2, y, WHITE, &label);
        }
    }
}

/// Sensors pan arrows: (rect, action).
fn pan_arrows(fe: &Fe) -> Vec<(Rect, Action)> {
    let (w, h) = fe.screen_size();
    let y0 = h;
    vec![
        (
            Rect::from_xywh(w / 2 - 64, y0 - 52, 32, 32),
            keys::PAN_SENSORS_LEFT,
        ),
        (
            Rect::from_xywh(w / 2 + 32, y0 - 52, 32, 32),
            keys::PAN_SENSORS_RIGHT,
        ),
        (
            Rect::from_xywh(w / 2 - 32, y0 - 52, 64, 16),
            keys::PAN_SENSORS_UP,
        ),
        (
            Rect::from_xywh(w / 2 - 32, y0 - 36, 64, 16),
            keys::PAN_SENSORS_DOWN,
        ),
    ]
}

fn draw_pan_arrows(fe: &Fe, canvas: &mut Canvas) {
    let res = fe.resources().clone();
    let (mx, my) = fe.game.ig.overlay.mouse;
    let names = [
        "SensorsScroll_Left",
        "SensorsScroll_Right",
        "SensorsScroll_Up",
        "SensorsScroll_Down",
    ];
    for ((r, _), n) in pan_arrows(fe).into_iter().zip(names) {
        let name = if r.contains(mx, my) {
            format!("{n}_Mouse")
        } else {
            n.to_string()
        };
        fe.game
            .ig
            .overlay
            .icons
            .draw(&res, canvas, &name, r.x0, r.y0);
    }
}

/// `moDrawPrintOptionsMenu`: "(1) SELECTED SHIPS ENABLED" lines while the key is held.
fn draw_options_menu(fe: &Fe, canvas: &mut Canvas, view: &HostView) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    let st = &fe.game.ig.overlay;
    let s = &fe.game.ig.strings;
    let top = st.top;
    let r = Rect::new(180, top + 10, draw::width(&f, &"A".repeat(48)) + 220, top + 9 * h + 40);
    canvas.fill_rect(r, BLACK);
    canvas.outline_rect(r, WHITE);
    canvas.outline_rect(Rect::new(r.x0 + 1, r.y0 + 1, r.x1 - 1, r.y1 - 1), WHITE);
    let labels = [
        "ShowSelectedShips",
        "ShowSelectingShips",
        "ShowObjectives",
        "ShowTooltips",
        "ShowPlayerInfo",
        "ShowGameMessages",
        "ShowPlayerNames",
        "ShowConstruction",
        "ShowResearch",
    ];
    for (i, (&panel, label)) in MENU_PANELS.iter().zip(labels).enumerate() {
        let key = if st.backdrop_mode {
            format!("strMO_Option_{label}Backdrop")
        } else {
            format!("strMO_Option_{label}")
        };
        let name = s
            .get(&key)
            .map_or_else(|| PANELS[panel].to_string(), str::to_string);
        let on = if st.backdrop_mode {
            st.backdrop(view, panel)
        } else {
            st.shown(view, panel)
        };
        let state = if on { "ENABLED" } else { "DISABLED" };
        let c = if on { GREEN } else { RED };
        draw::print(
            canvas,
            &f,
            190,
            top + 20 + i as i32 * h,
            c,
            &format!("({}) {name} {state}", i + 1),
        );
    }
}

/// `moDrawKeyBindingList`: a page of key bindings while Show_Keys is held.
fn draw_key_page(fe: &Fe, canvas: &mut Canvas, page: u8) {
    let Some(f) = font(fe, FONT_TEXT) else { return };
    let h = crate::text::height(&f);
    canvas.blend_rect(Rect::new(0, 0, canvas.width, canvas.height), [5, 5, 5, 210]);
    let groups: &[usize] = match page {
        1 => &[0, 1, 2, 3],
        2 => &[4, 5, 6, 7],
        _ => &[8, 9],
    };
    let mut x = 10;
    let mut y = 10;
    for &g in groups {
        let (first, title) = KEY_GROUPS[g];
        let last = KEY_GROUPS.get(g + 1).map_or(148, |n| n.0);
        let rows = (last - first) as i32;
        if y + (rows + 1) * h > canvas.height - 10 && x == 10 {
            x = canvas.width / 2;
            y = 10;
        }
        draw::print(canvas, &f, x, y, WHITE, title);
        y += h;
        for a in first..last {
            let a = Action(a);
            let name = fe
                .game
                .ig
                .strings
                .key_description(a.name())
                .map_or_else(|| a.name().replace('_', " "), str::to_string);
            let mut cx = x + draw::print(
                canvas,
                &f,
                x,
                y,
                [210, 210, 210, 255],
                &format!("{name}:  "),
            );
            let binds = fe.game.ig.keys.of(a);
            if binds.is_empty() {
                draw::print(canvas, &f, cx, y, RED, "NONE");
            }
            for (i, b) in binds.iter().enumerate() {
                let c = [GREEN, YELLOW, [0, 0, 255, 255]][i.min(2)];
                if i > 0 {
                    cx += draw::print(canvas, &f, cx, y, [210, 210, 210, 255], ", ");
                }
                cx += draw::print(canvas, &f, cx, y, c, &b.display());
            }
            y += h;
        }
        y += 2 * h;
    }
}

/// `selStatusRectCompute`: the rectangle of the status bars over a ship whose projected centre
/// is (cx, cy) with a projected radius of `radius` pixels.
pub fn status_rect(cx: i32, cy: i32, radius: f32) -> Rect {
    let w = (1.6 * radius).max(8.0) as i32;
    let h = (radius / 32.0).max(2.0) as i32;
    let left = cx - w / 2 - 1;
    let right = left + (w - 2 * h) + 4;
    let top = cy - (radius as i32 + h);
    Rect::new(left, top, right, top + h)
}

/// `selHotKeyNumberDraw`: the hot-key group number of a selected ship, at the right end of its
/// status bars and a line of the font above them, in `selHotKeyNumberColor`.
pub fn draw_hotkey_number(canvas: &mut Canvas, font: &hwc_formats::font::Font, bars: Rect, label: &str) {
    let y = bars.y0 - crate::text::height(font);
    crate::text::print(
        canvas,
        font,
        bars.x1,
        y,
        HOTKEY_NUMBER_COLOR,
        label.as_bytes(),
        (crate::text::Shadow::from_bits(0), [0, 0, 0, 255]),
    );
}

/// `selHotKeyNumberColor` (0x8889c0).
pub const HOTKEY_NUMBER_COLOR: Color = [250, 250, 250, 255];

/// `selHotKeyNumberDraw`'s font for a ship's level of detail: `selGroupFont0..2` for levels
/// 0..2, `selGroupFont3` beyond.
pub fn hotkey_font_index(lod: usize) -> usize {
    lod.min(3)
}

/// `selStatusBarsDraw` + `selStatusBarDraw`: the health bar over a ship whose projected
/// centre is (cx, cy) with a projected radius of `radius` pixels (for the game's 3D pass).
pub fn draw_health_bar(canvas: &mut Canvas, cx: i32, cy: i32, radius: f32, fraction: f32) {
    let Rect {
        x0: left,
        y0: top,
        x1: right,
        y1: bottom,
    } = status_rect(cx, cy, radius);
    let h = bottom - top;
    let f = fraction.clamp(0.0, 1.0);
    let (fill, empty) = crate::ingame::launchmgr::health_colours(f);
    // tweak.script colours for the fill.
    let fill = if f > crate::ingame::launchmgr::HEALTH_GREEN {
        [0, 255, 64, 255]
    } else if f > crate::ingame::launchmgr::HEALTH_YELLOW {
        [255, 255, 0, 255]
    } else {
        let _ = fill;
        [255, 0, 0, 255]
    };
    let split = left + ((right - left) as f32 * f) as i32;
    canvas.fill_rect(Rect::new(left, top, split, top + h), fill);
    canvas.fill_rect(Rect::new(split, top, right, top + h), empty);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitle::HudTop;

    #[test]
    fn top_follows_the_portrait_then_slides_back() {
        let mut st = State::default();
        let portrait = HudTop {
            drawn: true,
            portrait_bottom: Some(70),
        };
        st.set_top(portrait);
        assert_eq!(st.top, 70);
        // Text without a portrait keeps the top where it is and holds the slide.
        for _ in 0..20 {
            st.clock += 0.1;
            st.set_top(HudTop {
                drawn: true,
                portrait_bottom: None,
            });
            st.settle_top();
        }
        assert_eq!(st.top, 70);
        // Nothing drawn: nothing moves for half a second, then a pixel a frame (2 from 1.5 s on).
        let mut tops = Vec::new();
        for _ in 0..14 {
            st.clock += 0.1;
            st.set_top(HudTop::default());
            st.settle_top();
            tops.push(st.top);
        }
        assert_eq!(tops, [70, 70, 70, 70, 70, 69, 68, 67, 66, 65, 64, 63, 62, 61]);
        while st.top > 0 {
            st.clock += 0.1;
            st.settle_top();
        }
        assert_eq!(st.top, 0);
    }
}
