//! The single-player hyperspace roll call (`FEMan\Hyperspace_Roll_Call.fib`, screen
//! `HyperspaceRollCall`), started by `HyperspaceRollCallBegin` when the player jumps in a
//! mission with a roll call: "PREPARING FOR HYPERSPACE", the numbers of ships remaining and
//! docked (`ShipsRemainingDraw`, `ShipsDockedDraw`), and the buttons "QUICK DOCKING"
//! (`GoNowHyperspace`), "EMERGENCY HYPERSPACE" (`EmergencyHyperspace`) and "CANCEL DOCKING"
//! (`AbortHyperspace`). Exactly one of the first two is disabled when the screen opens: quick
//! docking while a ship of the player is in combat, otherwise the emergency jump.
//!
//! The game keeps running behind it; the main view takes no keys and no orders while it is up
//! (`mrDisable`), but off the screen the right button and the wheel still move the camera
//! (the main region keeps its mouse events). It closes when a button is pressed, or when the
//! game's roll call ends by itself (every ship docked: the game reports no roll call any more).

use crate::canvas::Rect;
use crate::fer::Fer;
use crate::ingame::{close_manager, draw, Fe, GameRequest, Manager};
use crate::region::AtomRef;

/// The screen.
pub const SCREEN: &str = "HyperspaceRollCall";
/// `selFontName2`, the font the two numbers are printed in.
pub const NUMBER_FONT: &str = "TO_2.hff";
const WHITE: [u8; 4] = [255, 255, 255, 255];

pub fn register(fe: &mut Fe) {
    fe.callback_add("GoNowHyperspace", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        dismiss(fe);
        fe.game.ig.request(GameRequest::HyperspaceGoNow);
    });
    fe.callback_add("EmergencyHyperspace", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        dismiss(fe);
        fe.game.ig.request(GameRequest::EmergencyHyperspace);
    });
    fe.callback_add("AbortHyperspace", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        dismiss(fe);
        fe.game.ig.request(GameRequest::AbortHyperspace);
    });
    fe.draw_callback_add("ShipsRemainingDraw", |fe, fer, a, r| {
        let v = fe.game.ig.view.roll_call.map_or(0, |c| c.remaining);
        draw_number(fe, fer, a, r, v);
    });
    fe.draw_callback_add("ShipsDockedDraw", |fe, fer, a, r| {
        let v = fe.game.ig.view.roll_call.map_or(0, |c| c.docked);
        draw_number(fe, fer, a, r, v);
    });
}

/// A button closed the screen (`feScreenDeleteFlags`); it stays closed until the game's roll
/// call is over.
fn dismiss(fe: &mut Fe) {
    close_manager(fe);
    fe.game.ig.rollcall_dismissed = true;
}

/// `sprintf("%d")` centred in the region in white (`fontPrintCentreCentreRectangle`).
fn draw_number(_fe: &mut Fe, fer: &mut Fer, _a: AtomRef, rect: Rect, value: i32) {
    let Some(font) = fer.res.font(NUMBER_FONT) else {
        return;
    };
    let y = rect.y0 + (rect.height() - crate::text::height(&font)) / 2;
    draw::print_centred(
        fer.canvas,
        &font,
        (rect.x0 + rect.x1) / 2,
        y,
        WHITE,
        &value.to_string(),
    );
}

/// After the screen has started: one of the jump buttons is disabled, decided now.
pub fn started(fe: &mut Fe) {
    fe.game.ig.rollcall_go_now_disabled =
        fe.game.ig.view.roll_call.is_some_and(|c| c.go_now_disabled);
    apply_disabled(fe);
}

/// The buttons' disabled states (again after the screens were rebuilt for a new size).
fn apply_disabled(fe: &mut Fe) {
    let (off, on) = if fe.game.ig.rollcall_go_now_disabled {
        ("GoNowHyperspace", "EmergencyHyperspace")
    } else {
        ("EmergencyHyperspace", "GoNowHyperspace")
    };
    if let Some(a) = fe.find_atom(off) {
        fe.set_disabled(a, true);
    }
    if let Some(a) = fe.find_atom(on) {
        fe.set_disabled(a, false);
    }
}

/// Keeps the screen in step with the game: it opens when the roll call starts and closes
/// when the roll call ended without a button.
pub fn tick(fe: &mut Fe) {
    let up = fe.game.ig.manager == Some(Manager::RollCall);
    let wanted = fe.game.ig.view.roll_call.is_some();
    if !wanted {
        fe.game.ig.rollcall_dismissed = false;
    }
    match (wanted && !fe.game.ig.rollcall_dismissed, up) {
        (true, false) => {
            // The roll call replaces whatever screen was up (`feAllMenusDelete`).
            if fe.game.ig.manager.is_some() {
                close_manager(fe);
            }
            crate::ingame::open_manager(fe, Manager::RollCall);
        }
        (false, true) => close_manager(fe),
        (true, true) => apply_disabled(fe),
        _ => {}
    }
}
