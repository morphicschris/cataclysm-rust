//! The key binding list of the options (`OP_KeyBindingsList`, `kbListWindowCB`), shared by the
//! front end and the in-game options: every action with its bindings; Enter or a double click
//! on a row waits for a key and binds it (with the held Shift/Alt/Ctrl), taking it from
//! actions that may not share it; keys without a name open the "invalid key" box.

use crate::canvas::{Color, Rect};
use crate::consts::*;
use crate::controls::ListItem;
use crate::fe::Call;
use crate::fer::Fer;
use crate::ingame::draw;
use crate::ingame::keys::{self, Action, Binding, ACTION_NAMES};
use crate::ingame::overlay::KEY_GROUPS;
use crate::ingame::Fe;

pub const FONT: &str = "hw_eurosecond_11.hff";
const UNBOUND: Color = [255, 20, 20, 255];
const BINDING: Color = [255, 255, 20, 255];

pub fn register(fe: &mut Fe) {
    fe.callback_add("OP_KeyBindingsList", key_list);
    fe.list_drawer_add("OP_KeyBindingsList", item_draw);
    // The pool list has no handler in the game: it stays empty.
    fe.callback_add("OP_KeyBindingsPoolList", |_, _| {});
    fe.callback_add("OP_KeyResetToDefault", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        // The engine reloads keys.cfg: the bindings saved when the options were entered.
        fe.game.ig.keys = fe.game.ig.saved_keys.clone();
        fe.game.ig.binding = None;
    });
}

fn row_height(fe: &Fe) -> i32 {
    fe.resources()
        .font(FONT)
        .map_or(14, |f| crate::text::height(&f) + 2)
}

fn key_list(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.last_call(call) {
        fe.game.ig.binding = None;
        return;
    }
    if fe.first_call(call) {
        let h = row_height(fe);
        fe.list_init(a, FONT, h);
        fe.list_focusable(a, true);
        if let Some(l) = fe.list_mut(a) {
            for i in 0..ACTION_NAMES.len() {
                l.add(String::new(), i as u32 + 1, i == 0);
            }
        }
        if let Some(id) = fe.atom(a).region {
            fe.list_scroll_adjust(id);
        }
        fe.game.ig.binding = None;
        return;
    }
    let Some(l) = fe.list(a) else { return };
    let msg = l.message;
    let selected = l
        .selected
        .and_then(|i| l.items.get(i))
        .map(|i| Action(i.data as u16));
    let captured = l.captured_key;
    match msg {
        CM_ACCEPT_TEXT | CM_DOUBLE_CLICK => {
            fe.game.ig.binding = selected;
            if let Some(l) = fe.list_mut(a) {
                l.key_capture = selected.is_some();
                l.captured_key = None;
            }
        }
        CM_KEY_PRESSED => {
            let Some(action) = fe.game.ig.binding.take() else {
                return;
            };
            let Some(k) = captured else { return };
            if keys::key_has_name(k) {
                let mut mods = 0;
                for (vk, m) in [
                    (key::SHIFT, keys::MOD_SHIFT),
                    (key::ALT, keys::MOD_ALT),
                    (key::CONTROL, keys::MOD_CTRL),
                ] {
                    if fe.input.down.contains(&vk) {
                        mods |= m;
                    }
                }
                fe.game
                    .ig
                    .keys
                    .set_from_list(action, Binding { key: k, mods });
            } else {
                let name = if fe.find_screen("In_Game_Invalid_Key").is_some() {
                    "In_Game_Invalid_Key"
                } else {
                    "Invalid_Key"
                };
                let base = fe.current_base();
                fe.screen_start_from(base, name);
            }
        }
        CM_NEW_ITEM_SELECTED | CM_LOSE_FOCUS if fe.game.ig.binding.is_some() => {
            fe.game.ig.binding = None;
            if let Some(l) = fe.list_mut(a) {
                l.key_capture = false;
            }
        }
        _ => {}
    }
}

/// Group title of the first action of each group.
pub(crate) fn group_title(a: Action) -> Option<&'static str> {
    KEY_GROUPS.iter().find(|g| g.0 == a.0).map(|g| g.1)
}

/// `kbListItemDraw`: "<Group> ---- Description" and the bound keys at 3/5 of the width.
fn item_draw(fe: &Fe, fer: &mut Fer, row: Rect, item: &ListItem) {
    let Some(font) = fer.res.font(FONT) else {
        return;
    };
    let a = Action(item.data as u16);
    let ig = &fe.game.ig;
    let binds = ig.keys.of(a);
    let binding = ig.binding == Some(a);
    let c = if binding {
        BINDING
    } else if binds.is_empty() {
        UNBOUND
    } else if item.selected {
        colour::LIST_ITEM_SELECTED
    } else {
        colour::LIST_ITEM_STANDARD
    };
    let desc = ig
        .strings
        .key_description(a.name())
        .map_or_else(|| a.name().replace('_', " "), str::to_string);
    let pad = draw::width(&font, "Formations - ");
    let x = row.x0 + 2;
    if let Some(t) = group_title(a) {
        let dashes = "-".repeat(12usize.saturating_sub(t.len()));
        draw::print(fer.canvas, &font, x, row.y0, c, &format!("{t} {dashes}"));
    }
    draw::print(fer.canvas, &font, x + pad, row.y0, c, &desc);
    let keys_text = if binding {
        "??????".to_string()
    } else if binds.is_empty() {
        "NONE".to_string()
    } else {
        binds
            .iter()
            .map(|b| b.display())
            .collect::<Vec<_>>()
            .join(",")
    };
    let kx = row.x0 + row.width() * 3 / 5;
    draw::print(fer.canvas, &font, kx, row.y0, c, &keys_text);
}
