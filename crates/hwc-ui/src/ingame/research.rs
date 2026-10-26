//! The research manager (`ResearchGUI.c`): technologies grouped by research module with their
//! state colours and progress, the five lab buttons (one animated blob for the Beast), the tech
//! picture with its description, Research and Clear lab.

use crate::canvas::{Color, Rect, BLACK, WHITE};
use crate::consts::*;
use crate::controls::ListItem;
use crate::fe::Call;
use crate::fer::Fer;
use crate::ingame::consmgr::{static_rect, SOUND_MANAGER_EXIT, SOUND_MANAGER_INTRO, SOUND_SPACE};
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::{close_manager, open_manager, Fe, Manager};
use crate::region::AtomRef;

pub const FONT_LIST: &str = "HW_EuroseCond_11.hff";
pub const FONT_INFO: &str = "default.hff";
pub const FONT_MODULE: &str = "EuroseWideHeavy_i8.hff";
pub const SOUND_RESEARCH_MANAGER: u32 = 0x106;
pub const SOUND_BLOB: u32 = 0x13c;
pub const SPEECH_RESEARCH: u32 = 0x400_1031;
pub const SPEECH_CLEAR_LAB: u32 = 0x400_1032;

const GOLD: Color = [255, 200, 0, 255];
const UPGRADE_AVAILABLE: Color = [0, 215, 155, 255];
const UNAVAILABLE: Color = [100, 100, 100, 255];
const OWNED_UPGRADE: Color = [0, 130, 94, 255];
const OWNED_DOT: Color = [0, 139, 0, 255];
const BAR_BACK: Color = [9, 79, 117, 255];
const BAR_LEFT: Color = [37, 83, 239, 255];
const BAR_RIGHT: Color = [145, 168, 249, 255];
const INFO_TEXT: Color = [121, 255, 238, 255];
const LAB_OUTLINE: Color = [255, 0, 0, 255];
const LAB_BAR_DARK: Color = [50, 50, 27, 255];
const LAB_BAR_LIGHT: Color = [160, 160, 115, 255];
const SCAN_LINE: Color = [200, 200, 200, 255];

/// Lab icon names, in lab order (`Research_icon_<name>_{disabled,off,on}.lif`).
pub const LAB_ICONS: [&str; 5] = ["Hangar", "Armour", "Engineering", "Special", "Weapons"];
/// Module pictures shown when a lab is clicked.
pub const MODULE_IMAGES: [&str; 5] = [
    "MODULE_Hangar",
    "MODULE_Armour",
    "MODULE_Engineering",
    "MODULE_Advanced_Engineering",
    "MODULE_Weapon",
];
/// Beast blob animation (ping-pong over six frames).
const BLOB_FRAMES: [usize; 10] = [1, 2, 3, 4, 5, 4, 3, 2, 1, 0];

/// What the picture shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Picture {
    #[default]
    Default,
    Tech(TechId),
    Module(usize),
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub selected_lab: Option<usize>,
    pub picture: Picture,
    pub extended_info: bool,
    /// Scan-line positions of the lab progress bars (0..1).
    scan: [f32; 5],
    /// Beast blob frame counter and its clock.
    blob_frame: usize,
    blob_time: f64,
    list: Option<AtomRef>,
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("RM_ExitMenu", |fe, call| {
        if guard(fe, call) {
            close(fe);
        }
    });
    fe.callback_add("RM_Construction", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Construction);
        }
    });
    fe.callback_add("RM_Launch", |fe, call| {
        if guard(fe, call) {
            close(fe);
            open_manager(fe, Manager::Launch);
        }
    });
    fe.callback_add("RM_ResearchItem", |fe, call| {
        if guard(fe, call) {
            research_item(fe);
        }
    });
    fe.callback_add("RM_ClearLab", |fe, call| {
        if guard(fe, call) {
            clear_lab(fe);
        }
    });
    fe.callback_add("RM_ClearAllLabs", |fe, _| fe.game.ig.rm.selected_lab = None);
    fe.callback_add("RM_ExtendedInfo", |fe, call| {
        if guard(fe, call) {
            fe.game.ig.rm.extended_info = !fe.game.ig.rm.extended_info;
        }
    });
    fe.callback_add("RM_TechListWindow", tech_list);
    fe.list_drawer_add("RM_TechListWindow", tech_item_draw);
    for (i, name) in [
        "RM_Lab1Draw",
        "RM_Lab2Draw",
        "RM_Lab3Draw",
        "RM_Lab4Draw",
        "RM_Lab5Draw",
    ]
    .into_iter()
    .enumerate()
    {
        let f: crate::fe::FeDrawFunction<crate::cataclysm::Cataclysm> = match i {
            0 => |fe, fer, a, r| draw_lab(fe, fer, a, r, 0),
            1 => |fe, fer, a, r| draw_lab(fe, fer, a, r, 1),
            2 => |fe, fer, a, r| draw_lab(fe, fer, a, r, 2),
            3 => |fe, fer, a, r| draw_lab(fe, fer, a, r, 3),
            _ => |fe, fer, a, r| draw_lab(fe, fer, a, r, 4),
        };
        fe.draw_callback_add(name, f);
    }
    fe.draw_callback_add("RM_TechImageDraw", draw_picture);
    fe.draw_callback_add("RM_ModuleNameDraw", draw_module_name);
    fe.draw_callback_add("RM_PussyBlob", draw_blob);
    fe.region_function_add("rmSelectLab1", |fe, _, ev| select_lab(fe, ev, 0));
    fe.region_function_add("rmSelectLab2", |fe, _, ev| select_lab(fe, ev, 1));
    fe.region_function_add("rmSelectLab3", |fe, _, ev| select_lab(fe, ev, 2));
    fe.region_function_add("rmSelectLab4", |fe, _, ev| select_lab(fe, ev, 3));
    fe.region_function_add("rmSelectLab5", |fe, _, ev| select_lab(fe, ev, 4));
}

fn guard(fe: &Fe, call: &Call) -> bool {
    !(fe.first_call(call) || fe.last_call(call))
}

/// `rmResearchGUIBegin`.
pub fn begin(fe: &mut Fe) {
    fe.game.ig.rm.picture = Picture::Default;
    fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_INTRO));
    fe.game
        .ig
        .request(GameRequest::Sound(SOUND_RESEARCH_MANAGER));
}

pub fn started(fe: &mut Fe) {
    for i in 1..=5 {
        let f: &'static str = match i {
            1 => "rmSelectLab1",
            2 => "rmSelectLab2",
            3 => "rmSelectLab3",
            4 => "rmSelectLab4",
            _ => "rmSelectLab5",
        };
        fe.region_function_set(&format!("RM_Lab{i}Draw"), f, RPE_PRESS_LEFT);
    }
    refill(fe);
}

pub fn tick(fe: &mut Fe) {
    if fe.game.ig.manager != Some(Manager::Research) {
        return;
    }
    refill(fe);
    // Beast blob: a frame every tenth of a second; a sound each cycle while researching.
    let rm = &mut fe.game.ig.rm;
    if fe.time - rm.blob_time >= 0.1 {
        rm.blob_time = fe.time;
        let researching = fe
            .game
            .ig
            .view
            .labs
            .first()
            .is_some_and(|l| l.status == LabStatus::Researching);
        let rm = &mut fe.game.ig.rm;
        if researching {
            rm.blob_frame = (rm.blob_frame + 1) % BLOB_FRAMES.len();
            if rm.blob_frame == 0 && fe.game.ig.view.player.race == Race::Beast {
                fe.game.ig.request(GameRequest::Sound(SOUND_BLOB));
            }
        } else {
            rm.blob_frame = 0;
        }
    }
}

/// `RM_ExitMenu`.
pub fn close(fe: &mut Fe) {
    fe.game.ig.rm.picture = Picture::Default;
    fe.game.ig.rm.list = None;
    fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_EXIT));
    fe.game.ig.request(GameRequest::Sound(SOUND_SPACE));
    close_manager(fe);
}

// -------------------------------------------------------------------------------------------
// The list
// -------------------------------------------------------------------------------------------

const HEADER: u32 = 0x8000_0000;

/// Visible rows: techs that are not unavailable, headings with such techs below them.
pub fn visible_rows(view: &HostView) -> Vec<(String, u32, bool)> {
    let rows = &view.research_rows;
    let shown = |id: &TechId| {
        view.tech(*id)
            .is_some_and(|t| t.state != TechState::Unavailable)
    };
    let mut out = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        match r {
            ResearchRow::Header { name } => {
                let any = rows[i + 1..]
                    .iter()
                    .take_while(|r| matches!(r, ResearchRow::Tech(_)))
                    .any(|r| matches!(r, ResearchRow::Tech(id) if shown(id)));
                if any {
                    out.push((name.clone(), HEADER, false));
                }
            }
            ResearchRow::Tech(id) if shown(id) => {
                let name = view.tech(*id).map(|t| t.name.clone()).unwrap_or_default();
                out.push((name, *id, true));
            }
            _ => {}
        }
    }
    out
}

/// `rmUpdateTechList`: rebuilt when the visible rows change; the pictured tech stays
/// selected.
fn refill(fe: &mut Fe) {
    let Some(a) = fe.game.ig.rm.list else { return };
    let rows = visible_rows(&fe.game.ig.view);
    let picture = fe.game.ig.rm.picture;
    let Some(l) = fe.list_mut(a) else { return };
    let same = l.items.len() == rows.len()
        && l.items
            .iter()
            .zip(&rows)
            .all(|(i, r)| i.data == r.1 && i.text == r.0);
    if same {
        return;
    }
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
    if let Picture::Tech(t) = picture {
        if let Some(i) = l.items.iter().position(|i| i.can_select && i.data == t) {
            l.select(i);
        }
    }
    l.upper = upper.min(l.items.len().saturating_sub(l.max_visible));
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// `RM_TechListWindow`: set up on creation; a click shows the tech, a double click researches.
fn tech_list(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.last_call(call) {
        fe.game.ig.rm.list = None;
        return;
    }
    if fe.first_call(call) {
        let h = fe.resources().font(FONT_LIST).map_or(16, |f| {
            let h = crate::text::height(&f);
            h + h / 2
        });
        fe.list_init(a, FONT_LIST, h);
        fe.game.ig.rm.list = Some(a);
        refill(fe);
        return;
    }
    let Some(l) = fe.list(a) else { return };
    let msg = l.message;
    let Some(tech) = l
        .selected
        .and_then(|i| l.items.get(i))
        .filter(|i| i.can_select)
        .map(|i| i.data)
    else {
        return;
    };
    match msg {
        CM_NEW_ITEM_SELECTED => {
            fe.game.ig.rm.picture = Picture::Tech(tech);
            fe.game.ig.rm.selected_lab = fe.game.ig.view.tech(tech).map(|t| t.lab);
        }
        CM_DOUBLE_CLICK => research_item(fe),
        _ => {}
    }
}

/// The screen's technology list (`RM_TechListWindow`), while the screen is up.
pub(crate) fn list_atom(fe: &Fe) -> Option<AtomRef> {
    fe.game.ig.rm.list
}

/// The technology selected in the list (what Research acts on).
pub(crate) fn selected_tech(fe: &Fe) -> Option<&Tech> {
    let a = fe.game.ig.rm.list?;
    let l = fe.list(a)?;
    let item = l.selected.and_then(|i| l.items.get(i))?;
    if !item.can_select {
        return None;
    }
    fe.game.ig.view.tech(item.data)
}

/// Progress (0..1) of a tech being researched.
pub(crate) fn progress_of(view: &HostView, tech: TechId) -> f32 {
    view.labs
        .iter()
        .find(|l| l.topic == Some(tech))
        .map_or(0.0, |l| l.progress.clamp(0.0, 1.0))
}

/// `rmDrawTechListItem`.
fn tech_item_draw(fe: &Fe, fer: &mut Fer, row: Rect, item: &ListItem) {
    let Some(font) = fer.res.font(FONT_LIST) else {
        return;
    };
    if !item.can_select {
        draw::print(fer.canvas, &font, row.x0, row.y0, GOLD, &item.text);
        return;
    }
    let Some(t) = fe.game.ig.view.tech(item.data) else {
        return;
    };
    let x = row.x0 + 4 * draw::width(&font, " ");
    let fh = crate::text::height(&font);
    let available = if t.upgrade {
        UPGRADE_AVAILABLE
    } else {
        colour::LIST_ITEM_STANDARD
    };
    let mut shadow = None;
    let mut c = match t.state {
        TechState::Available => available,
        TechState::Unavailable => UNAVAILABLE,
        TechState::Researching => {
            let done = progress_of(&fe.game.ig.view, t.id);
            let bar = Rect::new(x - 2, row.y0, row.x1 - 2, row.y0 + fh + 2);
            fer.canvas.fill_rect(bar, BAR_BACK);
            let right = (row.x1 - 2) - ((row.x1 - x) as f32 * (1.0 - done)) as i32;
            gradient_h(
                fer.canvas,
                Rect::new(x - 2, bar.y0, right, bar.y1),
                BAR_LEFT,
                BAR_RIGHT,
            );
            shadow = Some([0, 0, 0, 0x40]);
            GOLD
        }
        TechState::Researched => {
            fill_circle(fer.canvas, x - 7, row.y0 + 5, 3, OWNED_DOT);
            if t.upgrade {
                OWNED_UPGRADE
            } else {
                colour::LIST_ITEM_INACTIVE
            }
        }
    };
    if item.selected && t.state != TechState::Researching {
        c = available;
    }
    match shadow {
        Some(s) => draw::print_shadow(fer.canvas, &font, x, row.y0, c, s, &item.text),
        None => draw::print(fer.canvas, &font, x, row.y0, c, &item.text),
    };
}

/// A left-to-right colour ramp (`primRectShaded2`).
fn gradient_h(c: &mut crate::canvas::Canvas, r: Rect, left: Color, right: Color) {
    let w = r.width().max(1);
    for x in r.x0..r.x1 {
        let t = (x - r.x0) as f32 / w as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        let col = [
            mix(left[0], right[0]),
            mix(left[1], right[1]),
            mix(left[2], right[2]),
            255,
        ];
        c.fill_rect(Rect::new(x, r.y0, x + 1, r.y1), col);
    }
}

/// A top-to-bottom colour ramp.
fn gradient_v(c: &mut crate::canvas::Canvas, r: Rect, top: Color, bottom: Color) {
    let h = r.height().max(1);
    for y in r.y0..r.y1 {
        let t = (y - r.y0) as f32 / h as f32;
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t) as u8;
        let col = [
            mix(top[0], bottom[0]),
            mix(top[1], bottom[1]),
            mix(top[2], bottom[2]),
            255,
        ];
        c.fill_rect(Rect::new(r.x0, y, r.x1, y + 1), col);
    }
}

fn fill_circle(c: &mut crate::canvas::Canvas, cx: i32, cy: i32, r: i32, col: Color) {
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                c.put(cx + x, cy + y, col);
            }
        }
    }
}

// -------------------------------------------------------------------------------------------
// Buttons
// -------------------------------------------------------------------------------------------

/// `RM_ResearchItem`: research the selected tech in its lab if that lab is idle; the lab
/// becomes selected either way.
pub fn research_item(fe: &mut Fe) {
    let Some(t) = selected_tech(fe).cloned() else {
        return;
    };
    if t.state == TechState::Researched {
        return;
    }
    let lab = if fe.game.ig.view.player.race == Race::Beast {
        0
    } else {
        t.lab
    };
    let idle = fe
        .game
        .ig
        .view
        .labs
        .get(lab)
        .is_some_and(|l| l.status == LabStatus::Idle);
    if idle && t.state == TechState::Available {
        fe.game
            .ig
            .request(GameRequest::Research { tech: t.id, lab });
        fe.game.ig.rm.scan[lab] = 0.0;
        fe.game.ig.request(GameRequest::Speech {
            event: SPEECH_RESEARCH,
            param: None,
        });
    }
    fe.game.ig.rm.selected_lab = Some(lab);
}

/// `RM_ClearLab`: stops the selected lab's research (its progress is lost).
pub fn clear_lab(fe: &mut Fe) {
    let Some(lab) = fe.game.ig.rm.selected_lab else {
        return;
    };
    if fe
        .game
        .ig
        .view
        .labs
        .get(lab)
        .is_some_and(|l| l.status == LabStatus::Researching)
    {
        fe.game.ig.request(GameRequest::ClearLab { lab });
        fe.game.ig.request(GameRequest::Speech {
            event: SPEECH_CLEAR_LAB,
            param: None,
        });
    }
}

pub(crate) fn select_lab(fe: &mut Fe, ev: u32, lab: usize) {
    if ev != RPE_PRESS_LEFT {
        return;
    }
    let present = fe
        .game
        .ig
        .view
        .labs
        .get(lab)
        .is_some_and(|l| l.status != LabStatus::Absent);
    if present {
        fe.game.ig.rm.selected_lab = Some(lab);
        fe.game.ig.rm.picture = Picture::Module(lab);
    }
}

// -------------------------------------------------------------------------------------------
// Drawing
// -------------------------------------------------------------------------------------------

fn race_dir(fe: &Fe) -> u32 {
    fe.game.ig.view.player.race.file_index()
}

/// `rmDrawLabButton`.
fn draw_lab(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, lab: usize) {
    let selected = fe.game.ig.rm.selected_lab == Some(lab);
    fer.focus_window(rect, selected);
    let l = fe.game.ig.view.labs.get(lab).cloned().unwrap_or_default();
    let inner = Rect::new(rect.x0 + 5, rect.y0 + 5, rect.x1 - 5, rect.y1 - 5);
    if l.status != LabStatus::Researching {
        fer.canvas.fill_rect(inner, BLACK);
    }
    fer.canvas.outline_rect(inner, LAB_OUTLINE);
    if l.status == LabStatus::Absent {
        return;
    }
    let state = if selected { "on" } else { "off" };
    let path = format!(
        "ResearchGUI\\Race1\\Research_icon_{}_{state}.lif",
        LAB_ICONS[lab]
    );
    if let Some(img) = fer.res.image(&path) {
        let icon = Rect::new(rect.x0 + 10, rect.y0 + 5, rect.x1 - 10, rect.y1 - 18);
        draw::textured_rect(fer.canvas, &img, icon, WHITE);
    }
    if l.status == LabStatus::Researching {
        let bar = Rect::new(rect.x0 + 6, rect.y1 - 14, rect.x1 - 6, rect.y1 - 7);
        let w = (bar.width() as f32 * l.progress.clamp(0.0, 1.0)) as i32;
        gradient_v(
            fer.canvas,
            Rect::new(bar.x0, bar.y0, bar.x0 + w, bar.y1),
            LAB_BAR_DARK,
            LAB_BAR_LIGHT,
        );
        if selected {
            let s = &mut fe.game.ig.rm.scan[lab];
            *s = (*s + 1.0 / 32.0) % 1.0;
            let x = bar.x0 + (bar.width() as f32 * *s) as i32;
            fer.canvas.line(x, bar.y0, x, bar.y1 - 1, SCAN_LINE);
        }
    }
}

/// `RM_TechImageDraw`: the picture, the title, and the description when extended info is on.
fn draw_picture(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    let race = race_dir(fe);
    let picture = fe.game.ig.rm.picture;
    let file = match picture {
        Picture::Default => "Default_Research_Image".to_string(),
        Picture::Tech(t) => fe
            .game
            .ig
            .view
            .tech(t)
            .map_or_else(|| "Default_Research_Image".into(), |t| t.key.clone()),
        Picture::Module(m) => MODULE_IMAGES[m.min(4)].to_string(),
    };
    let inner = Rect::new(rect.x0 + 3, rect.y0 + 3, rect.x1 - 3, rect.y1 - 3);
    let img = fer
        .res
        .image(&format!("ResearchGUI\\Race{race}\\{file}.lif"))
        .or_else(|| fer.res.image(&format!("ResearchGUI\\Race1\\{file}.lif")));
    if let Some(img) = img {
        draw::textured_rect(fer.canvas, &img, inner, WHITE);
    }
    let Some(font) = fer.res.font(FONT_INFO) else {
        return;
    };
    let fh = crate::text::height(&font);
    let tech = match picture {
        Picture::Tech(t) => fe.game.ig.view.tech(t).cloned(),
        _ => None,
    };
    if fe.game.ig.rm.extended_info {
        fer.canvas.blend_rect(inner, [0, 0, 0, 128]);
        let text = match (&tech, picture) {
            (Some(t), _) => t.description.clone(),
            (None, Picture::Module(m)) => fe
                .game
                .ig
                .view
                .labs
                .get(m)
                .map(|l| l.name.clone())
                .unwrap_or_default(),
            _ => String::new(),
        };
        let mut y = rect.y0 + 16 + fh * 3 / 2;
        for line in draw::wrap(&font, &text, rect.width() - 15 - 15) {
            if y > rect.y1 - fh {
                break;
            }
            draw::print(fer.canvas, &font, rect.x0 + 15, y, INFO_TEXT, &line);
            y += fh;
        }
    }
    if let Some(t) = tech {
        draw::print(fer.canvas, &font, rect.x0 + 15, rect.y0 + 11, GOLD, &t.name);
    }
}

/// `RM_ModuleNameDraw`: the selected lab's module, white with a black shadow.
fn draw_module_name(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    let Some(lab) = fe.game.ig.rm.selected_lab else {
        return;
    };
    let Some(name) = fe.game.ig.view.labs.get(lab).map(|l| l.name.clone()) else {
        return;
    };
    if let Some(font) = fer.res.font(FONT_MODULE) {
        draw::print(fer.canvas, &font, rect.x0 + 2, rect.y0 + 2, BLACK, &name);
        draw::print(fer.canvas, &font, rect.x0, rect.y0, WHITE, &name);
    }
}

/// `RM_PussyBlob`: the Beast's single lab, an animation that pulses while researching.
fn draw_blob(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect) {
    let researching = fe
        .game
        .ig
        .view
        .labs
        .first()
        .is_some_and(|l| l.status == LabStatus::Researching);
    let frame = if researching {
        BLOB_FRAMES[fe.game.ig.rm.blob_frame % BLOB_FRAMES.len()]
    } else {
        0
    };
    let path = format!("ResearchGUI\\Race2\\bresearch_ani_{:02}.lif", frame + 1);
    if let Some(img) = fer.res.image(&path) {
        draw::textured_rect(fer.canvas, &img, rect, WHITE);
    }
}
