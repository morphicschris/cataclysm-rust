//! The LAN game screens of `multiplayer_game.fib` (`MultiplayerGame.c` and the Titan screens
//! of the original, here over the port's own network games, `hwc-net`): Connection Method's
//! LAN button, LAN Login (the player's name and setup), the game list of Available Games (the
//! games the browser finds on the local network), Create Game on the Basic / Advanced /
//! Resource option tabs, Connecting, and the waiting rooms: Captain Wait for the game's host
//! (start or dissolve the game) and Player Wait for a player who joined, each with the game's
//! chat and its players.
//!
//! The rooms of the original's lobby server (the Channel Chat screen) have no LAN equivalent:
//! after the login the game list opens at once. The internet screens (Connection Method's
//! Internet: Internet Login with its firewall choice, the room's chat and players, Available
//! Rooms, Create Room, and the same game list and waiting rooms with the room's chat) talk to
//! the port's lobby server (`hwc-lobby`) in place of WON's.
//!
//! The screens only show what the application tells them ([`LanUi`]) and ask it to act
//! ([`LanRequest`] in [`Request::Lan`]); the application owns the network.

use crate::canvas::Rect;
use crate::cataclysm::{Cataclysm, Request};
use crate::consts::colour;
use crate::consts::{CM_ACCEPT_TEXT, CM_DOUBLE_CLICK, CM_KEY_PRESSED};
use crate::fe::{Call, FrontEnd};
use crate::fer::Fer;
use crate::region::AtomRef;
use crate::skirmish::SkirmishRequest;
use crate::text::{self, Shadow};

type Fe = FrontEnd<Cataclysm>;

/// One game the browser found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GameRow {
    pub name: String,
    pub host: String,
    pub map: String,
    pub players: u8,
    pub max_players: u8,
    pub started: bool,
    /// The host runs the same build (a game of another build cannot be joined).
    pub compatible: bool,
}

/// What the screens show of the network.
#[derive(Debug, Clone, Default)]
pub struct LanUi {
    /// The games found.
    pub games: Vec<GameRow>,
    /// The game being made on the option tabs (the Basic tab came from Create Game).
    pub creating: bool,
    /// We host the game we are in.
    pub hosting: bool,
    /// The game we are in: its name and its players' lines.
    pub game_name: String,
    pub players: Vec<String>,
    /// The game can start (the host's Start button).
    pub can_start: bool,
    /// The game's chat.
    pub chat: Vec<String>,
    /// What connecting has said.
    pub status: Vec<String>,
    /// The screen the player setup returns to.
    pub setup_from: Option<String>,
    /// The lists need filling again.
    pub dirty: bool,
    /// The internet screens (the lobby server) rather than the LAN's.
    pub internet: bool,
    /// The lobby's room: its name, chat and players; the rooms there are.
    pub room: String,
    pub room_chat: Vec<String>,
    pub room_users: Vec<String>,
    pub rooms: Vec<String>,
    /// The internet login's password and firewall choice (0 not behind, 1 behind, 2 detect).
    pub password: String,
    pub firewall: u8,
    /// The room being created.
    pub new_room: (String, String, String),
}

/// What the screens ask the application to do.
#[derive(Debug, Clone, PartialEq)]
pub enum LanRequest {
    /// Look for games on the local network (or stop).
    Browse(bool),
    /// Ask the network again ("Ping all").
    Refresh,
    /// Host a game with this name and these settings.
    Host { game_name: String, request: SkirmishRequest, max_players: usize },
    /// Join the game of that row.
    Join(usize),
    /// A line of the game's chat.
    Chat(String),
    /// The host starts the game.
    Start,
    /// Leave (or, hosting, dissolve) the game.
    Leave,
    /// The player's race changed (0 Somtaaw, 1 Beast).
    Race(u8),
    /// Log in to the lobby server.
    Login { name: String, password: String, firewall: u8 },
    /// Leave the lobby server.
    Logout,
    /// A line of the room's chat.
    RoomChat(String),
    /// The rooms there are.
    ListRooms,
    /// Go to the room of that row.
    JoinRoom(usize),
    /// Make a room and go to it.
    CreateRoom { name: String, description: String, password: String },
}

pub fn register(fe: &mut Fe) {
    fe.callback_add("MG_LANIPX", lan_login);
    fe.callback_add("MG_InternetWON", internet_login);
    fe.callback_add("MG_PasswordEntry", |fe, call| entry(fe, call, |l| &mut l.password, 0x1f));
    fe.callback_add("MG_FirewallButton", firewall);
    fe.callback_add("MG_LaunchWON", launch_won);
    for name in ["MG_NewAccount", "MG_ChangePassword"] {
        // The lobby server has no accounts: any free name logs in.
        fe.callback_add(name, |_, _| {});
    }
    fe.callback_add("MG_ChatHistoryWindow", |fe, call| {
        if fe.first_call(call) {
            fill_room(fe);
        }
    });
    fe.callback_add("MG_UserNameWindow", |fe, call| {
        if fe.first_call(call) {
            fill_room(fe);
        }
    });
    fe.callback_add("MG_ChatTextEntry", room_chat_entry);
    fe.callback_add("MG_ViewChannels", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.requests.push(Request::Lan(LanRequest::ListRooms));
        fe.all_screens_delete();
        fe.screen_start("Available_Channels");
    });
    fe.callback_add("MG_ListOfChannels", list_of_rooms);
    fe.callback_add("MG_JoinChannel", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            join_room(fe);
        }
    });
    fe.callback_add("MG_CreateChannel", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.lan.new_room = Default::default();
        fe.all_screens_delete();
        fe.screen_start("Create_Channel");
    });
    fe.callback_add("MG_ChannelNameEntry", |fe, call| entry(fe, call, |l| &mut l.new_room.0, 0x17));
    fe.callback_add("MG_ChannelDescriptionEntry", |fe, call| entry(fe, call, |l| &mut l.new_room.1, 0x3f));
    fe.callback_add("MG_ChannelPasswordEntry", |fe, call| entry(fe, call, |l| &mut l.new_room.2, 0x1f));
    fe.callback_add("MG_CreateChannelNow", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        let (name, description, password) = fe.game.lan.new_room.clone();
        if name.trim().is_empty() {
            return;
        }
        fe.game.requests.push(Request::Lan(LanRequest::CreateRoom { name, description, password }));
        fe.all_screens_delete();
        fe.screen_start("Channel_Chat");
    });
    for name in ["MG_BackToAvailableChannels", "MG_BackToChannelChat"] {
        fe.callback_add(name, |fe, call| {
            if fe.first_call(call) || fe.last_call(call) {
                return;
            }
            let to = if call.name == "MG_BackToAvailableChannels" { "Available_Channels" } else { "Channel_Chat" };
            fe.all_screens_delete();
            fe.screen_start(to);
        });
    }
    fe.callback_add("MG_BackToConnection", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.all_screens_delete();
        fe.screen_start("Connection_Method");
    });
    fe.callback_add("MG_LaunchLAN", launch);
    fe.callback_add("MG_ListOfGamesBack", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.all_screens_delete();
        if fe.game.lan.internet {
            fe.screen_start("Channel_Chat");
        } else {
            fe.game.requests.push(Request::Lan(LanRequest::Browse(false)));
            fe.screen_start("LAN_Login");
        }
    });
    fe.callback_add("MG_ListOfGames", list_of_games);
    fe.callback_add("MG_PingAllGames", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            fe.game.requests.push(Request::Lan(LanRequest::Refresh));
        }
    });
    fe.callback_add("MG_JoinGame", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            join_selected(fe);
        }
    });
    fe.callback_add("MG_CreateGame", create_game);
    fe.callback_add("MG_GameNameTextEntry", game_name_entry);
    fe.callback_add("MG_CreateGameNow", create_now);
    fe.callback_add("MG_GameChatWindow", |fe, call| {
        if fe.first_call(call) {
            fill_chat(fe);
        }
    });
    fe.callback_add("MG_GameUserName", |fe, call| {
        if fe.first_call(call) {
            fill_players(fe);
        }
    });
    fe.callback_add("MG_GameChatTextEntry", chat_entry);
    fe.callback_add("MG_StartGame", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) && fe.game.lan.hosting && fe.game.lan.can_start {
            fe.game.requests.push(Request::Lan(LanRequest::Start));
        }
    });
    for name in ["MG_LeaveGame", "MG_ViewGames", "MG_ConnectingCancel"] {
        fe.callback_add(name, |fe, call| {
            if fe.first_call(call) || fe.last_call(call) {
                return;
            }
            // Connecting to the lobby itself: cancel goes back to the login.
            if call.name == "MG_ConnectingCancel" && fe.game.lan.internet && fe.game.lan.room.is_empty() {
                fe.game.requests.push(Request::Lan(LanRequest::Logout));
                fe.all_screens_delete();
                fe.screen_start("Internet_Login");
                return;
            }
            leave(fe, "Available_Games");
        });
    }
    fe.callback_add("MG_BackToLogin", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        if fe.game.lan.internet {
            fe.game.requests.push(Request::Lan(LanRequest::Leave));
            fe.game.requests.push(Request::Lan(LanRequest::Logout));
            fe.all_screens_delete();
            fe.screen_start("Internet_Login");
        } else {
            leave(fe, "LAN_Login");
        }
    });
    fe.callback_add("MG_ChangeColors", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.lan.setup_from = fe.current_screen_name().map(str::to_string);
        fe.screen_start("Player_Options");
    });
    fe.callback_add("MG_SetColorsNow", |fe, call| {
        if fe.first_call(call) || fe.last_call(call) {
            return;
        }
        fe.game.colors.accept(&mut fe.game.options);
        let race = fe.game.skirmish.race;
        fe.game.options.set("PlayerRace", i32::from(race));
        fe.game.requests.push(Request::Lan(LanRequest::Race(race)));
        fe.current_screen_delete();
    });
    fe.callback_add("MG_BackFromPlayerOptions", |fe, call| {
        if !fe.first_call(call) && !fe.last_call(call) {
            fe.current_screen_delete();
        }
    });
    for name in ["MG_SetupGame", "MG_ViewDetails", "MG_ConnectingStatus"] {
        fe.callback_add(name, |fe, call| {
            if name_is(call, "MG_ConnectingStatus") && fe.first_call(call) {
                fill_status(fe);
            }
        });
    }
    fe.draw_callback_add("MG_CurrentGame", draw_current_game);
    fe.draw_callback_add("MG_CurrentChannel", draw_current_room);
}

fn name_is(call: &Call, name: &str) -> bool {
    call.name == name
}

/// `mgLANIPX`: the login, with the player's name.
fn lan_login(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    fe.game.lan.internet = false;
    if fe.game.skirmish.name.is_empty() {
        fe.game.skirmish.name = fe.game.options.get_str("PlayerName").to_string();
    }
    fe.game.skirmish.race = fe.game.options.get("PlayerRace").clamp(0, 1) as u8;
    fe.all_screens_delete();
    fe.screen_start("LAN_Login");
}

/// `mgLaunchLAN` (GO!): the name is kept and the game list opens, looking for games.
fn launch(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let name = fe.game.skirmish.name.trim().to_string();
    if name.is_empty() {
        return;
    }
    fe.game.options.set_str("PlayerName", &name);
    fe.game.requests.push(Request::Lan(LanRequest::Browse(true)));
    fe.all_screens_delete();
    fe.screen_start("Available_Games");
}

/// `mgInternetWON`: the internet login, with the player's name.
fn internet_login(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    fe.game.lan.internet = true;
    if fe.game.skirmish.name.is_empty() {
        fe.game.skirmish.name = fe.game.options.get_str("PlayerName").to_string();
    }
    fe.game.skirmish.race = fe.game.options.get("PlayerRace").clamp(0, 1) as u8;
    fe.game.lan.firewall = fe.game.options.get("hwcFirewall").clamp(0, 2) as u8;
    fe.all_screens_delete();
    fe.screen_start("Internet_Login");
}

/// A text box kept in [`LanUi`].
fn entry(fe: &mut Fe, call: &Call, field: fn(&mut LanUi) -> &mut String, max: usize) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = field(&mut fe.game.lan).clone();
        fe.text_entry_set(a, &v, max);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let t = fe.text_entry(a).unwrap_or_default().to_string();
    *field(&mut fe.game.lan) = t;
}

/// The firewall choice: 0 not behind one, 1 behind one (games go through the relay), 2 find
/// out (the server tries to reach the game).
fn firewall(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let v = fe.game.lan.firewall;
        fe.radio_set("MG_FirewallButton", u32::from(v));
        return;
    }
    if fe.last_call(call) {
        return;
    }
    fe.game.lan.firewall = (fe.atom(a).radio_value as u8).min(2);
    fe.game.options.set("hwcFirewall", i32::from(fe.game.lan.firewall));
}

/// `mgLaunchWON`: log in to the lobby server.
fn launch_won(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let name = fe.game.skirmish.name.trim().to_string();
    if name.is_empty() {
        return;
    }
    fe.game.options.set_str("PlayerName", &name);
    fe.game.lan.room.clear();
    fe.game.lan.status = vec!["Logging in...".to_string()];
    let (password, firewall) = (fe.game.lan.password.clone(), fe.game.lan.firewall);
    fe.game.requests.push(Request::Lan(LanRequest::Login { name, password, firewall }));
    fe.all_screens_delete();
    fe.screen_start("Connecting");
}

/// Logged in: the room's screen.
pub fn logged_in(fe: &mut Fe) {
    fe.all_screens_delete();
    fe.screen_start("Channel_Chat");
}

/// The lobby server turned us away or went: back to the login with why.
pub fn back_to_login(fe: &mut Fe, why: &str) {
    fe.game.lan.status = vec![why.to_string()];
    fe.game.lan.room.clear();
    fe.all_screens_delete();
    fe.screen_start(if fe.game.lan.internet { "Internet_Login" } else { "LAN_Login" });
}

fn room_chat_entry(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.text_entry_set(a, "", 0x7f);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    if fe.text_message(a) == CM_ACCEPT_TEXT {
        let line = fe.text_entry(a).unwrap_or_default().trim().to_string();
        if !line.is_empty() {
            fe.game.requests.push(Request::Lan(LanRequest::RoomChat(line)));
        }
        fe.text_entry_set(a, "", 0x7f);
    }
}

fn fill_room(fe: &mut Fe) {
    let chat = fe.game.lan.room_chat.clone();
    fill_lines(fe, "MG_ChatHistoryWindow", &chat, true);
    let users = fe.game.lan.room_users.clone();
    fill_lines(fe, "MG_UserNameWindow", &users, false);
}

fn list_of_rooms(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.list_init(a, "HW_EuroseCond_11.hff", 18);
        fill_rooms(fe);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let msg = fe.list(a).map_or(0, |l| l.message);
    if msg == CM_DOUBLE_CLICK || msg == CM_ACCEPT_TEXT {
        join_room(fe);
    }
}

fn fill_rooms(fe: &mut Fe) {
    let rows = fe.game.lan.rooms.clone();
    let Some(a) = fe.find_atom("MG_ListOfChannels") else { return };
    let keep = fe.list(a).and_then(|l| l.selected);
    if let Some(l) = fe.list_mut(a) {
        l.items.clear();
        l.selected = None;
        for (i, t) in rows.iter().enumerate() {
            l.add(t.clone(), i as u32, keep == Some(i));
        }
    }
}

fn join_room(fe: &mut Fe) {
    let Some(a) = fe.find_atom("MG_ListOfChannels") else { return };
    let Some(i) = fe.list(a).and_then(|l| l.selected).and_then(|i| fe.list(a)?.items.get(i)).map(|it| it.data as usize) else { return };
    fe.game.requests.push(Request::Lan(LanRequest::JoinRoom(i)));
    fe.all_screens_delete();
    fe.screen_start("Channel_Chat");
}

fn draw_current_room(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, r: Rect) {
    let Some(font) = fer.res.font("HW_EuroseCond_11.hff") else { return };
    let t: String = fe.game.lan.room.to_ascii_uppercase().chars().take(0x1f).collect();
    text::print(fer.canvas, &font, r.x0, r.y0, colour::LIST_ITEM_SELECTED, &text::to_latin1(&t), (Shadow::None, [0, 0, 0, 255]));
}

/// The game list: a row for each game found, with its map and players.
fn list_of_games(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.list_init(a, "HW_EuroseCond_11.hff", 18);
        fill_games(fe);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    let msg = fe.list(a).map_or(0, |l| l.message);
    if msg == CM_DOUBLE_CLICK || msg == CM_ACCEPT_TEXT {
        join_selected(fe);
    }
}

/// A game's row in the list.
pub fn game_line(g: &GameRow) -> String {
    let state = if !g.compatible {
        "  (other version)"
    } else if g.started {
        "  (playing)"
    } else {
        ""
    };
    format!("{}  -  {}  -  {}  {}/{}{state}", g.name, g.host, g.map, g.players, g.max_players)
}

fn fill_games(fe: &mut Fe) {
    let Some(a) = fe.find_atom("MG_ListOfGames") else { return };
    let rows: Vec<String> = fe.game.lan.games.iter().map(game_line).collect();
    let keep = fe.list(a).and_then(|l| l.selected);
    if let Some(l) = fe.list_mut(a) {
        l.items.clear();
        l.selected = None;
        for (i, t) in rows.iter().enumerate() {
            l.add(t.clone(), i as u32, keep == Some(i));
        }
        if l.upper >= l.items.len() {
            l.upper = 0;
        }
    }
}

fn join_selected(fe: &mut Fe) {
    let Some(a) = fe.find_atom("MG_ListOfGames") else { return };
    let Some(i) = fe.list(a).and_then(|l| l.selected).and_then(|i| fe.list(a)?.items.get(i)).map(|it| it.data as usize) else { return };
    let ok = fe.game.lan.games.get(i).is_some_and(|g| g.compatible && !g.started);
    if !ok {
        return;
    }
    fe.game.lan.status = vec![format!("Joining {}...", fe.game.lan.games[i].name)];
    fe.game.lan.hosting = false;
    fe.game.requests.push(Request::Lan(LanRequest::Join(i)));
    fe.screen_start("Connecting");
}

/// `mgCreateGame`: the option tabs, for a game of ours.
fn create_game(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let res = fe.resources().clone();
    fe.game.skirmish.load(&|p| res.read(p));
    let s = &mut fe.game.skirmish;
    s.options.num_computers = 0;
    if fe.game.lan.game_name.is_empty() {
        let who = fe.game.skirmish.name.clone();
        fe.game.lan.game_name = format!("{who}'s game");
    }
    fe.game.lan.creating = true;
    fe.all_screens_delete();
    fe.screen_start("Basic_Options");
}

fn game_name_entry(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        let n = fe.game.lan.game_name.clone();
        fe.text_entry_set(a, &n, 0x1f);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    match fe.text_message(a) {
        2 | 3 => fe.game.lan.game_name = fe.text_entry(a).unwrap_or_default().to_string(),
        CM_KEY_PRESSED => {
            let last = fe.text_entry(a).and_then(|t| t.chars().last());
            if last.is_some_and(|c| c.is_control() || c == '"' || c == '\'') {
                fe.text_entry_backspace(a);
            } else {
                fe.game.lan.game_name = fe.text_entry(a).unwrap_or_default().to_string();
            }
        }
        _ => {}
    }
}

/// `mgCreateGameNow`: the game is hosted and the host's waiting room opens.
fn create_now(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) || fe.last_call(call) {
        return;
    }
    let Some(request) = fe.game.skirmish.request() else { return };
    let max_players = fe.game.skirmish.max_players();
    let game_name = fe.game.lan.game_name.trim().to_string();
    if game_name.is_empty() {
        return;
    }
    fe.game.lan.creating = false;
    fe.game.lan.hosting = true;
    fe.game.lan.chat.clear();
    fe.game.requests.push(Request::Lan(LanRequest::Host { game_name, request, max_players }));
    fe.all_screens_delete();
    fe.screen_start("Captain_Wait");
}

/// The chat line typed in the waiting room goes when Return is pressed.
fn chat_entry(fe: &mut Fe, call: &Call) {
    let Some(a) = call.atom else { return };
    if fe.first_call(call) {
        fe.text_entry_set(a, "", 0x7f);
        return;
    }
    if fe.last_call(call) {
        return;
    }
    if fe.text_message(a) == CM_ACCEPT_TEXT {
        let line = fe.text_entry(a).unwrap_or_default().trim().to_string();
        if !line.is_empty() {
            fe.game.requests.push(Request::Lan(LanRequest::Chat(line)));
        }
        fe.text_entry_set(a, "", 0x7f);
    }
}

fn leave(fe: &mut Fe, to: &str) {
    fe.game.requests.push(Request::Lan(LanRequest::Leave));
    fe.game.lan.hosting = false;
    fe.game.lan.players.clear();
    fe.game.lan.chat.clear();
    fe.all_screens_delete();
    fe.screen_start(to);
}

fn fill_lines(fe: &mut Fe, atom: &str, lines: &[String], last_visible: bool) {
    let Some(a) = fe.find_atom(atom) else { return };
    if fe.list(a).is_some_and(|l| l.item_height <= 1) {
        fe.list_init(a, "HW_EuroseCond_11.hff", 14);
    }
    if let Some(l) = fe.list_mut(a) {
        l.items.clear();
        l.selected = None;
        for (i, t) in lines.iter().enumerate() {
            l.add(t.clone(), i as u32, false);
        }
        l.upper = if last_visible { l.items.len().saturating_sub(l.max_visible.max(1)) } else { 0 };
    }
}

fn fill_chat(fe: &mut Fe) {
    let lines = fe.game.lan.chat.clone();
    fill_lines(fe, "MG_GameChatWindow", &lines, true);
}

fn fill_players(fe: &mut Fe) {
    let lines = fe.game.lan.players.clone();
    fill_lines(fe, "MG_GameUserName", &lines, false);
}

fn fill_status(fe: &mut Fe) {
    let lines = fe.game.lan.status.clone();
    fill_lines(fe, "MG_ConnectingStatus", &lines, true);
}

fn draw_current_game(fe: &mut Fe, fer: &mut Fer, _a: AtomRef, r: Rect) {
    let Some(font) = fer.res.font("HW_EuroseCond_11.hff") else { return };
    let t: String = fe.game.lan.game_name.to_ascii_uppercase().chars().take(0x1f).collect();
    text::print(fer.canvas, &font, r.x0, r.y0, colour::LIST_ITEM_SELECTED, &text::to_latin1(&t), (Shadow::None, [0, 0, 0, 255]));
}

/// After the application changed [`LanUi`]: the open screen's lists show it.
pub fn refresh(fe: &mut Fe) {
    if !std::mem::take(&mut fe.game.lan.dirty) {
        return;
    }
    fill_games(fe);
    fill_chat(fe);
    fill_players(fe);
    fill_status(fe);
    fill_room(fe);
    fill_rooms(fe);
}

/// The host has taken us in: the player's waiting room.
pub fn joined(fe: &mut Fe) {
    if fe.current_screen_name() == Some("Connecting") || fe.find_screen("Player_Wait").is_none_or(|_| fe.current_screen_name() != Some("Player_Wait")) {
        fe.all_screens_delete();
        fe.screen_start("Player_Wait");
    }
}

/// The game could not be joined, or it ended: back to the game list with why.
pub fn back_to_games(fe: &mut Fe, why: &str) {
    fe.game.lan.status = vec![why.to_string()];
    fe.game.lan.room_chat.push(why.to_string());
    fe.game.lan.hosting = false;
    fe.game.lan.players.clear();
    fe.all_screens_delete();
    fe.screen_start("Available_Games");
}
