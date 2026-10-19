//! The in-game escape menu (`in_game_esc_menu.fib`, `gp*` / `IG_*` callbacks), its options
//! screens (the front end's option callbacks, with in-game Accept/Cancel), save/load, quit, game
//! over, and the objectives screen (`single_player_objective.fib`, `PO_*`).

use crate::canvas::Rect;
use crate::fer::Fer;
use crate::ingame::draw;
use crate::ingame::{close_manager, Fe, GameRequest, Manager};
use crate::region::AtomRef;
use crate::text::{self, Shadow};

/// `poTextFontName` and the text colour.
pub const TEXT_FONT: &str = "Arial_b12.hff";
const TEXT_COLOUR: [u8; 4] = [0, 200, 200, 255];

#[derive(Debug, Clone, Default)]
pub struct State {
    /// The single-player mission was lost: the menu opens on `SP_Game_Over`.
    pub game_over: bool,
    /// Save-game name typed in `IG_TextEntryWindowInit`.
    pub save_name: String,
    /// The objective whose fleet intelligence is shown (none: the mission briefing).
    pub objective: Option<usize>,
}

/// The escape menu variant for this game (`gpStartInGameEscapeMenu`).
pub fn screen_for(fe: &Fe) -> &'static str {
    let p = &fe.game.ig.view.player;
    if fe.game.ig.esc.game_over && p.single_player {
        "SP_Game_Over"
    } else if p.tutorial {
        "In_game_esc_menu_tutorial"
    } else if p.single_player {
        "In_game_esc_menu2"
    } else if p.multiplayer {
        "In_game_esc_menu_nosave"
    } else {
        "In_game_esc_menu"
    }
}

/// Back to the first page of the escape menu (`gpBackToInGameEscapeMenu`).
fn back_to_menu(fe: &mut Fe) {
    fe.current_screen_delete();
    let name = screen_for(fe);
    fe.screen_start(name);
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("IG_Cancel", |fe, _| close_manager(fe));
    fe.callback_add("IG_StartEscapeMenu", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        back_to_menu(fe);
    });
    fe.callback_add("QuitToMain", |fe, _| {
        close_manager(fe);
        fe.game.ig.request(GameRequest::QuitToMain);
    });
    fe.callback_add("SP_Restart_Level", |fe, _| {
        fe.game.ig.esc.game_over = false;
        close_manager(fe);
        fe.game.ig.request(GameRequest::RestartLevel);
    });
    fe.callback_add("OP_InGameOptions_Start", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.options_snapshot();
        fe.current_screen_delete();
        fe.screen_start("Game_options");
    });
    fe.callback_add("OP_InGameOptions_Accept", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.options_commit();
        fe.game.ig.request(GameRequest::ApplyOptions);
        fe.current_screen_delete();
        fe.screen_start("Game_options");
    });
    fe.callback_add("OP_InGameOptions_Cancel", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.options_restore();
        fe.game.ig.request(GameRequest::AudioSettings);
        fe.current_screen_delete();
        fe.screen_start("Game_options");
    });
    fe.callback_add("OP_Effects_To_InGameVideo_Accept", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.current_screen_delete();
        fe.screen_start("Video");
    });
    fe.callback_add("OP_Effects_To_InGameVideo_Cancel", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.current_screen_delete();
        fe.screen_start("Video");
    });
    fe.callback_add("IG_TextEntryWindowInit", |fe, call| {
        if fe.first_call(call) {
            fe.game.ig.esc.save_name.clear();
        }
    });
    fe.callback_add("IG_SaveGame", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        let name = entered_name(fe);
        close_manager(fe);
        fe.game.ig.request(GameRequest::SaveGame(name));
    });
    fe.callback_add("IG_LoadGame", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        let Some(name) = selected_game(fe) else {
            return;
        };
        fe.game.ig.esc.game_over = false;
        close_manager(fe);
        fe.game.ig.request(GameRequest::LoadGame(name));
    });
    fe.callback_add("PO_CloseObjectiveWindow", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        close_manager(fe);
    });
    fe.draw_callback_add("PO_TextDraw", draw_objective_text);
}

/// Text typed in the save screen's entry (or the selected game's name).
fn entered_name(fe: &Fe) -> String {
    let typed = fe
        .find_atom("IG_TextEntryWindowInit")
        .and_then(|a| fe.text_entry(a))
        .map(|t| t.trim().to_string())
        .unwrap_or_default();
    if typed.is_empty() {
        selected_game(fe).unwrap_or_else(|| "Saved Game".into())
    } else {
        typed
    }
}

fn selected_game(fe: &Fe) -> Option<String> {
    let a = fe.find_atom("FE_GameWindowInit")?;
    let l = fe.list(a)?;
    l.selected
        .and_then(|i| l.items.get(i))
        .map(|i| i.text.clone())
}

/// `poPopupTextDraw`: the objective's fleet intelligence, word wrapped.
fn draw_objective_text(fe: &mut Fe, fer: &mut Fer, _atom: AtomRef, rect: Rect) {
    let Some(font) = fer.res.font(TEXT_FONT) else {
        return;
    };
    let h = text::height(&font);
    let ig = &fe.game.ig;
    let body = ig
        .esc
        .objective
        .and_then(|i| ig.view.objectives.get(i))
        .map_or_else(|| ig.view.fleet_intel.clone(), |o| o.description.clone());
    let mut y = rect.y0;
    fer.canvas.set_clip(Some(rect));
    for l in draw::wrap(&font, &body, rect.width()) {
        if y + h > rect.y1 {
            break;
        }
        text::print(
            fer.canvas,
            &font,
            rect.x0,
            y,
            TEXT_COLOUR,
            &text::to_latin1(&l),
            (Shadow::None, [0, 0, 0, 255]),
        );
        y += h;
    }
    fer.canvas.set_clip(None);
}

/// Opens fleet intelligence for an objective (`poPopupFleetIntelligence`).
pub fn open_objectives(fe: &mut Fe, objective: Option<usize>) {
    fe.game.ig.esc.objective = objective;
    crate::ingame::open_manager(fe, Manager::Objectives);
}
