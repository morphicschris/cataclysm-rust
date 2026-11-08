//! In-game chat of a multiplayer game (`GameChat.c`): the "start global message" and "start
//! ally message" keys open the chat line of `Game_Chat.fib` at the top of the screen, with
//! "Say:" or "To allies:" before it (`gcChatTextDraw`); Return sends the line to the game
//! ([`GameRequest::Chat`]) and Escape drops it (`gcInGameChatEntry`). A line starting with
//! `/` and a player's name goes to that player only (`gcParseChatEntry`); Tab completes the
//! name. What the players say comes back as messages of the message log, which the chat page
//! keys scroll.

use crate::canvas::Canvas;
use crate::consts::{CM_ACCEPT_TEXT, CM_KEY_PRESSED, CM_REJECT_TEXT};
use crate::fe::Call;

use super::host::GameRequest;
use super::Fe;

/// The screens of `Game_Chat.fib`.
pub const SAY_SCREEN: &str = "Say_Chatting_Screen";
pub const ALLIES_SCREEN: &str = "Allies_Chatting_Screen";

/// The chat line's state.
#[derive(Debug, Clone, Default)]
pub struct State {
    /// `gcChatEntryMode` (0x00ae1a90): 0 closed, 1 to everyone, 2 to the allies.
    pub mode: u8,
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("InGameChatEntry", entry);
}

/// `gcChatEntryStart`: opens the chat line (multiplayer only).
pub fn start(fe: &mut Fe, allies: bool) {
    if !fe.game.ig.view.player.multiplayer {
        return;
    }
    if fe.find_screen(SAY_SCREEN).is_none() {
        let data = fe.resources().read(r"FEMan\Game_Chat.fib");
        match data.map(|d| fe.load_fib_data(&d)) {
            Some(Ok(())) => {}
            _ => {
                fe.log_once("Game_Chat.fib unavailable".into());
                return;
            }
        }
    }
    if fe.game.ig.chat.mode != 0 {
        return;
    }
    fe.game.ig.chat.mode = if allies { 2 } else { 1 };
    fe.screen_start(if allies { ALLIES_SCREEN } else { SAY_SCREEN });
}

fn close(fe: &mut Fe) {
    fe.game.ig.chat.mode = 0;
    fe.current_screen_delete();
}

/// `gcInGameChatEntry`.
fn entry(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.text_entry_set(a, "", 0xff);
        // The line takes the keys at once (`keyBufferClear`, the entry's focus).
        fe.text_entry_focus(a);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    match fe.text_message(a) {
        CM_ACCEPT_TEXT => {
            let text = fe.text_entry(a).unwrap_or_default().trim().to_string();
            let allies = fe.game.ig.chat.mode == 2;
            close(fe);
            if !text.is_empty() {
                fe.game.ig.request(GameRequest::Chat { allies, text });
            }
        }
        CM_REJECT_TEXT => close(fe),
        CM_KEY_PRESSED => {
            // Tab after "/" and part of a name completes it (`gcParseChatEntry`).
            let t = fe.text_entry(a).unwrap_or_default().to_string();
            if let Some(rest) = t.strip_suffix('\t') {
                let names: Vec<String> = fe.game.ig.view.player_names.iter().map(|n| n.0.clone()).collect();
                let done = match rest.strip_prefix('/').and_then(|p| complete(&names, p)) {
                    Some(n) => format!("/{n} "),
                    None => rest.to_string(),
                };
                fe.text_entry_set(a, &done, 0xff);
            }
        }
        _ => {}
    }
}

/// The one player whose name starts with `prefix` (ignoring case), if only one does.
pub fn complete(names: &[String], prefix: &str) -> Option<String> {
    let p = prefix.to_ascii_lowercase();
    let mut hits = names.iter().filter(|n| n.to_ascii_lowercase().starts_with(&p));
    let first = hits.next()?;
    hits.next().is_none().then(|| first.clone())
}

/// `gcParseChatEntry`: the player a line beginning `/name` is for, and the rest of the line.
pub fn whisper_to(names: &[String], line: &str) -> Option<(usize, String)> {
    let rest = line.strip_prefix('/')?;
    let lower = rest.to_ascii_lowercase();
    // The longest name the line starts with (names may have spaces).
    let (i, n) = names.iter().enumerate().filter(|(_, n)| !n.is_empty() && lower.starts_with(&n.to_ascii_lowercase())).max_by_key(|(_, n)| n.len())?;
    Some((i, rest[n.len()..].trim().to_string()))
}

/// The chat line's prompt (`gcChatTextDraw`), left of the line.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let mode = fe.game.ig.chat.mode;
    if mode == 0 {
        return;
    }
    let screen = if mode == 2 { ALLIES_SCREEN } else { SAY_SCREEN };
    let Some(r) = fe.find_atom("InGameChatEntry").filter(|a| fe.current_screen_name() == Some(screen) || a.screen == fe.find_screen(screen).unwrap_or(usize::MAX)).map(|a| fe.atom(a).rect()) else { return };
    let Some(f) = fe.resources().font("default.hff").or_else(|| fe.resources().font("HW_EuroseCond_11.hff")) else { return };
    let prompt = if mode == 2 { "To allies:" } else { "Say:" };
    let w = crate::text::width(&f, &crate::text::to_latin1(prompt)[..]);
    crate::text::print(canvas, &f, r.x0 - w - 4, r.y0 + 4, [255, 255, 255, 255], &crate::text::to_latin1(prompt), (crate::text::Shadow::None, [0, 0, 0, 255]));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["Alice".into(), "Bob".into(), "Bobby Tables".into()]
    }

    #[test]
    fn a_slash_and_a_name_whispers() {
        assert_eq!(whisper_to(&names(), "/alice hi there"), Some((0, "hi there".into())));
        // The longest name that fits.
        assert_eq!(whisper_to(&names(), "/Bobby Tables gg"), Some((2, "gg".into())));
        assert_eq!(whisper_to(&names(), "/Bob gg"), Some((1, "gg".into())));
        assert_eq!(whisper_to(&names(), "hello"), None);
        assert_eq!(whisper_to(&names(), "/Carol hi"), None);
    }

    #[test]
    fn tab_completes_one_name() {
        assert_eq!(complete(&names(), "al"), Some("Alice".into()));
        assert_eq!(complete(&names(), "bob"), None, "two names start so");
        assert_eq!(complete(&names(), "bobby"), Some("Bobby Tables".into()));
    }
}
