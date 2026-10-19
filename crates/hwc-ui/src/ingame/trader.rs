//! The Bentusi trader ("BENTUSI EXCHANGE", `Trader.c`): offered technologies with prices,
//! the trader's dialogue, the picture and description of the selected tech, and the player's RUs.

use crate::canvas::{Color, Rect, BLACK, WHITE};
use crate::consts::*;
use crate::fe::Call;
use crate::fer::Fer;
use crate::ingame::consmgr::{static_rect, SOUND_MANAGER_EXIT, SOUND_MANAGER_INTRO, SOUND_SPACE};
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::{close_manager, Fe, Manager};
use crate::region::{AtomRef, RegionId};

pub const FONT: &str = "HW_EuroseCond_11.hff";
pub const SOUND_TRADER: u32 = 0x103;
const STANDARD: Color = [0, 100, 160, 255];
const CANT_AFFORD: Color = [255, 25, 25, 255];
const SELECTED: Color = [255, 255, 105, 255];
const SELECTION_RECT: Color = [50, 120, 20, 255];

/// The trader's lines by dialogue state (greeting, selected, can't afford, purchased).
pub const DIALOG: [&str; 4] = [
    "Hello there, fellow space traveller. We have technology to trade.",
    "An excellent choice.",
    "You cannot afford this technology.",
    "The technology is yours. Trade well.",
];

#[derive(Debug, Clone, Default)]
pub struct State {
    pub selected: Option<TechId>,
    pub hover: Option<TechId>,
    pub dialog: usize,
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("TM_Leave", |fe, call| {
        if !(fe.first_call(call) || fe.last_call(call)) {
            fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_EXIT));
            fe.game.ig.request(GameRequest::Sound(SOUND_SPACE));
            fe.game.ig.request(GameRequest::TraderLeave);
            close_manager(fe);
        }
    });
    fe.callback_add("TM_AcceptOffer", accept);
    fe.draw_callback_add("TM_TechList", |fe, fer, a, r| {
        draw_list(fe, fer, a, r, false)
    });
    fe.draw_callback_add("TM_CostList", |fe, fer, a, r| {
        draw_list(fe, fer, a, r, true)
    });
    fe.draw_callback_add("TM_XRUs", |fe, fer, _a, r| {
        fer.canvas.fill_rect(r, BLACK);
        if let Some(font) = fer.res.font(FONT) {
            let v = fe.game.ig.view.player.resource_units.to_string();
            let y = r.y0 + (r.height() - crate::text::height(&font)) / 2;
            draw::print_right(fer.canvas, &font, r.x1 - 2, y, WHITE, &v);
        }
    });
    fe.draw_callback_add("TM_TechInfo", draw_info);
    fe.draw_callback_add("TM_TechImage", draw_image);
    fe.draw_callback_add("TM_Dialog", draw_dialog);
    fe.region_function_add("tmSelectAvailable", select_available);
}

/// `tmTradeBegin`.
pub fn open(fe: &mut Fe) {
    fe.game.ig.tm = State::default();
    fe.game.ig.request(GameRequest::Sound(SOUND_MANAGER_INTRO));
    fe.game.ig.request(GameRequest::Sound(SOUND_TRADER));
    crate::ingame::open_manager(fe, Manager::Trader);
    let filter = RPE_PRESS_LEFT | RPE_ENTER | RPE_EXIT | RPE_HOLD_LEFT;
    fe.region_function_set("TM_TechList", "tmSelectAvailable", filter);
    let first = fe
        .game
        .ig
        .view
        .trader_offers
        .iter()
        .find(|t| t.cost <= fe.game.ig.view.player.resource_units)
        .map(|t| t.id);
    fe.game.ig.tm.selected = first;
}

fn row_height(fe: &Fe) -> i32 {
    fe.resources()
        .font(FONT)
        .map_or(14, |f| crate::text::height(&f) + 2)
}

fn offer_at(fe: &Fe, rect: Rect, y: i32) -> Option<TechId> {
    let i = (y - rect.y0 - 2) / row_height(fe);
    (i >= 0)
        .then(|| fe.game.ig.view.trader_offers.get(i as usize).map(|t| t.id))
        .flatten()
}

fn select_available(fe: &mut Fe, id: RegionId, ev: u32) {
    let Some(rect) = fe.region(id).map(|r| r.rect) else {
        return;
    };
    let (_, my) = fe.mouse_pos();
    let hit = offer_at(fe, rect, my);
    fe.game.ig.tm.hover = hit;
    if ev == RPE_PRESS_LEFT {
        if let Some(t) = hit {
            let afford = fe
                .game
                .ig
                .view
                .trader_offers
                .iter()
                .find(|o| o.id == t)
                .is_some_and(|o| o.cost <= fe.game.ig.view.player.resource_units);
            if afford {
                fe.game.ig.tm.selected = Some(t);
                fe.game.ig.tm.dialog = 1;
            } else {
                fe.game.ig.tm.dialog = 2;
            }
        }
    }
}

/// `tmAcceptOffer`.
fn accept(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let Some(t) = fe.game.ig.tm.selected else {
        return;
    };
    let cost = fe
        .game
        .ig
        .view
        .trader_offers
        .iter()
        .find(|o| o.id == t)
        .map_or(i32::MAX, |o| o.cost);
    if cost <= fe.game.ig.view.player.resource_units {
        fe.game.ig.request(GameRequest::TraderAccept(t));
        fe.game.ig.tm.selected = None;
        fe.game.ig.tm.dialog = 3;
    } else {
        fe.game.ig.tm.dialog = 2;
    }
}

fn draw_list(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect, cost: bool) {
    static_rect(fe, fer, atom, rect);
    let Some(font) = fer.res.font(FONT) else {
        return;
    };
    let h = row_height(fe);
    let rus = fe.game.ig.view.player.resource_units;
    let mut y = rect.y0 + 2;
    for t in &fe.game.ig.view.trader_offers {
        if y + h > rect.y1 {
            break;
        }
        let selected = fe.game.ig.tm.selected == Some(t.id);
        if selected && !cost {
            fer.canvas.outline_rect(
                Rect::new(rect.x0 + 1, y - 1, rect.x1 - 1, y + h - 1),
                SELECTION_RECT,
            );
        }
        let c = if t.cost > rus {
            CANT_AFFORD
        } else if selected {
            SELECTED
        } else {
            STANDARD
        };
        if cost {
            draw::print_right(fer.canvas, &font, rect.x1 - 4, y, c, &t.cost.to_string());
        } else {
            draw::print(fer.canvas, &font, rect.x0 + 4, y, c, &t.name);
        }
        y += h;
    }
}

fn shown_tech(fe: &Fe) -> Option<&Tech> {
    let id = fe.game.ig.tm.hover.or(fe.game.ig.tm.selected)?;
    fe.game.ig.view.trader_offers.iter().find(|t| t.id == id)
}

fn draw_info(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    let (Some(font), Some(t)) = (fer.res.font(FONT), shown_tech(fe)) else {
        return;
    };
    let h = crate::text::height(&font);
    let mut y = rect.y0 + 3;
    for l in draw::wrap(&font, &t.description, rect.width() - 8) {
        if y + h > rect.y1 {
            break;
        }
        draw::print(fer.canvas, &font, rect.x0 + 4, y, STANDARD, &l);
        y += h;
    }
}

fn draw_image(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    let Some(t) = shown_tech(fe) else { return };
    let race = fe.game.ig.view.player.race.file_index();
    if let Some(img) = fer
        .res
        .image(&format!("ResearchGUI\\Race{race}\\{}.lif", t.key))
    {
        let inner = Rect::new(rect.x0 + 3, rect.y0 + 3, rect.x1 - 3, rect.y1 - 3);
        draw::textured_rect(fer.canvas, &img, inner, WHITE);
    }
}

fn draw_dialog(fe: &mut Fe, fer: &mut Fer, atom: AtomRef, rect: Rect) {
    static_rect(fe, fer, atom, rect);
    let Some(font) = fer.res.font(FONT) else {
        return;
    };
    let text = DIALOG[fe.game.ig.tm.dialog.min(3)];
    let h = crate::text::height(&font);
    let mut y = rect.y0 + 4;
    for l in draw::wrap(&font, text, rect.width() - 8) {
        draw::print(fer.canvas, &font, rect.x0 + 4, y, WHITE, &l);
        y += h;
    }
}
