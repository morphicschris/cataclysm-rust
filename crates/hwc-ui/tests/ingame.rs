//! In-game UI on the real game files with the mock host: manager navigation, build queue and
//! cancel flows, research and launch requests, key dispatch, the popup menu, and screenshots of
//! every screen into `screenshots/ingame/` (when `HWC_SCREENSHOTS` is set). Needs
//! `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped otherwise.

use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::canvas::Canvas;
use hwc_ui::ingame::host::*;
use hwc_ui::ingame::keys;
use hwc_ui::ingame::mock::{MockHost, CARRIER_1, MOTHERSHIP};
use hwc_ui::ingame::{InGameUi, InputEvent, Manager, Setup};
use hwc_ui::options::Options;
use hwc_ui::AssetSource;

const DEFAULT_KEYS: &str = "v 84\n\
key  Move  77\nkey  Dock  68\nkey  Cancel_Orders  192\nkey  Scuttle_Ships  SHIFT+83\n\
key  Tactics_Evasive  113\nkey  Formation_Wall  120\nkey  Open_BuildManager  66\n\
key  Open_ResearchManager  82\nkey  Open_LaunchManager  83\nkey  Escape  27\n\
key  Toggle_Ship_Info  9\nkey  Next_Overlay_View  86\nkey  Zoom_In  187\nkey  Zoom_In  107\n\
key  Enable_Height_Move  16\nkey  Select_ShipGroup_1  49\nkey  Create_ShipGroup_1  CTRL+49\n\
key  Show_Keys_1  35\n";

fn source() -> Option<Rc<dyn AssetSource>> {
    let data = GameData::from_env()?.expect("game data");
    Some(Rc::new(data))
}

fn ui(src: Rc<dyn AssetSource>) -> InGameUi {
    let mut ui = InGameUi::new(
        src,
        Setup {
            options: Options::default(),
            default_keys: Some(DEFAULT_KEYS.into()),
            keys: None,
            keys_path: None,
        },
    )
    .expect("in-game ui");
    ui.resize(640, 480);
    ui
}

/// One frame: the requests are applied to the mock like the game would.
fn frame(ui: &mut InGameUi, host: &mut MockHost) -> Vec<GameRequest> {
    let out = ui.update(host, 1.0 / 30.0);
    for r in &out {
        host.apply(r);
    }
    out
}

fn input(ui: &mut InGameUi, host: &mut MockHost, ev: InputEvent) -> bool {
    ui.handle_input(host, ev)
}

fn click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) -> Vec<GameRequest> {
    input(ui, host, InputEvent::MouseMove { x, y });
    let mut out = frame(ui, host);
    input(
        ui,
        host,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    out.extend(frame(ui, host));
    input(
        ui,
        host,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    out.extend(frame(ui, host));
    out
}

/// Clicks a named control as a player would: not as the second half of a double click
/// (which, as in the original, buttons do not see).
fn click(ui: &mut InGameUi, host: &mut MockHost, name: &str) -> Vec<GameRequest> {
    settle(ui, host);
    let a = ui
        .fe
        .find_atom(name)
        .unwrap_or_else(|| panic!("no {name} on {:?}", ui.fe.stack_names()));
    let r = ui.fe.atom(a).rect();
    click_at(ui, host, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn key(ui: &mut InGameUi, host: &mut MockHost, code: u16) -> Vec<GameRequest> {
    input(ui, host, InputEvent::KeyDown(code));
    let mut out = frame(ui, host);
    input(ui, host, InputEvent::KeyUp(code));
    out.extend(frame(ui, host));
    out
}

/// A stand-in for the 3D view: dark space with a few stars.
fn space(canvas: &mut Canvas) {
    canvas.clear([4, 6, 14, 255]);
    let mut seed = 12345u32;
    for _ in 0..400 {
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let x = (seed >> 8) as i32 % canvas.width;
        seed = seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        let y = (seed >> 8) as i32 % canvas.height;
        let b = 120 + (seed % 120) as u8;
        canvas.put(x, y, [b, b, b, 255]);
    }
}

fn shot(ui: &mut InGameUi, name: &str) {
    let Some(dir) = std::env::var_os("HWC_SCREENSHOTS") else {
        return;
    };
    let dir = std::path::Path::new(&dir).join("ingame");
    std::fs::create_dir_all(&dir).unwrap();
    let mut canvas = Canvas::new(640, 480);
    space(&mut canvas);
    ui.draw(&mut canvas);
    canvas.save_png(&dir.join(format!("{name}.png"))).unwrap();
}

/// Height of a construction-list row (`default.hff` height + 2).
fn cm_row(ui: &InGameUi) -> i32 {
    ui.fe
        .resources()
        .font("default.hff")
        .map(|f| hwc_ui::text::height(&f) + 2)
        .unwrap()
}

/// Screen point of the n-th visible row of the construction list.
fn cm_row_pos(ui: &InGameUi, row: i32) -> (i32, i32) {
    let a = ui.fe.find_atom("CM_Ships").unwrap();
    let r = ui.fe.atom(a).rect();
    (r.x0 + 40, r.y0 + 2 + row * cm_row(ui) + cm_row(ui) / 2)
}

/// Visible row of a ship type in the construction list.
fn cm_row_of(ui: &InGameUi, t: ShipType) -> i32 {
    hwc_ui::ingame::consmgr::rows(&ui.state().view)
        .iter()
        .position(|r| matches!(r, hwc_ui::ingame::consmgr::Row::Ship(o) if o.ship_type == t))
        .unwrap() as i32
}

fn list_row_pos(ui: &InGameUi, atom: &str, index: usize) -> (i32, i32) {
    let a = ui.fe.find_atom(atom).unwrap();
    let r = ui.fe.atom(a).rect();
    let l = ui.fe.list(a).unwrap();
    let y =
        r.y0 + l.title_height + 6 + (index - l.upper) as i32 * l.item_height + l.item_height / 2;
    (r.x0 + 30, y)
}

/// Waits out the double-click time so the next click is a single one.
fn settle(ui: &mut InGameUi, host: &mut MockHost) {
    for _ in 0..20 {
        frame(ui, host);
    }
}

fn right_click_at(ui: &mut InGameUi, host: &mut MockHost, x: i32, y: i32) {
    input(ui, host, InputEvent::MouseMove { x, y });
    input(
        ui,
        host,
        InputEvent::MouseButton {
            left: false,
            down: true,
        },
    );
    frame(ui, host);
    input(
        ui,
        host,
        InputEvent::MouseButton {
            left: false,
            down: false,
        },
    );
    frame(ui, host);
}

#[test]
fn build_manager_queue_build_pause_cancel() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    // B opens the build manager on the mothership.
    let out = key(&mut ui, &mut host, 66);
    assert_eq!(ui.manager(), Some(Manager::Construction));
    assert!(out.contains(&GameRequest::Sound(0x12f)));
    assert_eq!(ui.state().cm.factory, Some(MOTHERSHIP));
    shot(&mut ui, "construction_manager_open");
    // Recon: first click selects, the next two add to the manager's count.
    let (x, y) = cm_row_pos(&ui, cm_row_of(&ui, 0x00));
    click_at(&mut ui, &mut host, x, y);
    assert!(ui.state().cm.selected.contains(&0x00));
    settle(&mut ui, &mut host);
    click_at(&mut ui, &mut host, x, y);
    settle(&mut ui, &mut host);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.pending.get(&0x00), Some(&2));
    settle(&mut ui, &mut host);
    assert!(host.jobs.iter().all(|j| j.ship_type != 0x00));
    // Right click takes one back.
    right_click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.pending.get(&0x00), Some(&1));
    shot(&mut ui, "construction_manager_pending");
    // BUILD sends it to the factory.
    let out = click(&mut ui, &mut host, "CM_BuildShips");
    assert!(out.contains(&GameRequest::Build {
        factory: MOTHERSHIP,
        ship_type: 0x00,
        count: 1
    }));
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Speech {
            event: 0x400_1035,
            ..
        }
    )));
    assert!(ui.state().cm.pending.values().all(|&n| n == 0));
    assert!(host
        .jobs
        .iter()
        .any(|j| j.ship_type == 0x00 && j.count == 1));
    // Adding to a type in production goes straight into its queue.
    let (x, y) = cm_row_pos(&ui, cm_row_of(&ui, 0x01));
    click_at(&mut ui, &mut host, x, y);
    settle(&mut ui, &mut host);
    let out = click_at(&mut ui, &mut host, x, y);
    assert!(out.contains(&GameRequest::Build {
        factory: MOTHERSHIP,
        ship_type: 0x01,
        count: 1
    }));
    // Pause batch pauses the selected type, a second press resumes it.
    let out = click(&mut ui, &mut host, "CM_PauseJobs");
    assert!(out.contains(&GameRequest::PauseBuild {
        factory: MOTHERSHIP,
        ship_type: 0x01,
        paused: true
    }));
    let out = click(&mut ui, &mut host, "CM_PauseJobs");
    assert!(out.contains(&GameRequest::PauseBuild {
        factory: MOTHERSHIP,
        ship_type: 0x01,
        paused: false
    }));
    // Cancel batch removes the job and refunds what it cost so far.
    let ru = host.player.resource_units;
    let out = click(&mut ui, &mut host, "CM_CancelJobs");
    assert!(out.contains(&GameRequest::CancelBuild {
        factory: MOTHERSHIP,
        ship_type: 0x01
    }));
    assert!(host.jobs.iter().all(|j| j.ship_type != 0x01));
    assert_eq!(host.player.resource_units, ru + 34);
    // Greyed rows refuse and flash.
    let (x, y) = cm_row_pos(&ui, cm_row_of(&ui, 0x02));
    click_at(&mut ui, &mut host, x, y);
    settle(&mut ui, &mut host);
    click_at(&mut ui, &mut host, x, y);
    assert!(!ui.state().cm.pending.contains_key(&0x02));
    assert_eq!(ui.state().cm.flash.map(|f| f.0), Some(0x02));
    // The carrier icon switches the factory.
    let out = click(&mut ui, &mut host, "CM_Carrier1Draw");
    assert_eq!(ui.state().cm.factory, Some(CARRIER_1));
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Speech {
            event: 0x400_101c,
            param: Some(0x0c)
        }
    )));
    shot(&mut ui, "construction_manager_carrier");
    // Close.
    let out = click(&mut ui, &mut host, "CM_Close");
    assert_eq!(ui.manager(), None);
    assert!(out.contains(&GameRequest::Sound(0x130)));
}

#[test]
fn managers_link_to_each_other() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 66);
    click(&mut ui, &mut host, "CM_Research");
    assert_eq!(ui.manager(), Some(Manager::Research));
    assert_eq!(ui.fe.current_screen_name(), Some("Research_Manager"));
    click(&mut ui, &mut host, "RM_Launch");
    assert_eq!(ui.fe.current_screen_name(), Some("Launch_Manager"));
    click(&mut ui, &mut host, "LM_Construction");
    assert_eq!(ui.fe.current_screen_name(), Some("Construction_manager"));
    click(&mut ui, &mut host, "CM_Launch");
    click(&mut ui, &mut host, "LM_Research");
    click(&mut ui, &mut host, "RM_Construction");
    assert_eq!(ui.fe.current_screen_name(), Some("Construction_manager"));
    click(&mut ui, &mut host, "CM_Close");
    assert!(ui.fe.stack_names().is_empty());
    // The Beast gets its own screens.
    let mut beast = MockHost::beast();
    frame(&mut ui, &mut beast);
    key(&mut ui, &mut beast, 82);
    assert_eq!(ui.fe.current_screen_name(), Some("Beast_Research_Manager"));
    frame(&mut ui, &mut beast);
    shot(&mut ui, "research_manager_beast");
    click(&mut ui, &mut beast, "RM_Launch");
    assert_eq!(ui.fe.current_screen_name(), Some("Beast_Launch_Manager"));
    shot(&mut ui, "launch_manager_beast");
    click(&mut ui, &mut beast, "LM_Construction");
    assert_eq!(
        ui.fe.current_screen_name(),
        Some("Beast_Construction_manager")
    );
    shot(&mut ui, "construction_manager_beast");
}

#[test]
fn research_and_clear_lab() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 82);
    assert_eq!(ui.fe.current_screen_name(), Some("Research_Manager"));
    shot(&mut ui, "research_manager");
    // Mimic (Engineering lab, idle).
    let rows = hwc_ui::ingame::research::visible_rows(&ui.state().view);
    let i = rows.iter().position(|r| r.1 == 6).unwrap();
    let (x, y) = list_row_pos(&ui, "RM_TechListWindow", i);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().rm.selected_lab, Some(2));
    let out = click(&mut ui, &mut host, "RM_ResearchItem");
    assert!(out.contains(&GameRequest::Research { tech: 6, lab: 2 }));
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::Speech {
            event: 0x400_1031,
            ..
        }
    )));
    assert_eq!(host.labs[2].topic, Some(6));
    host.labs[2].progress = 0.45;
    frame(&mut ui, &mut host);
    shot(&mut ui, "research_manager_researching");
    let out = click(&mut ui, &mut host, "RM_ClearLab");
    assert!(out.contains(&GameRequest::ClearLab { lab: 2 }));
    // Clicking a lab shows its module.
    click(&mut ui, &mut host, "RM_Lab1Draw");
    assert_eq!(ui.state().rm.selected_lab, Some(0));
    click(&mut ui, &mut host, "RM_ExtendedInfo");
    shot(&mut ui, "research_manager_module_info");
    click(&mut ui, &mut host, "RM_ExitMenu");
    assert_eq!(ui.manager(), None);
}

#[test]
fn launch_group_one_all_and_auto_launch() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 83);
    assert_eq!(ui.fe.current_screen_name(), Some("Launch_Manager"));
    let a = ui.fe.find_atom("LM_ShipsToLaunch").unwrap();
    let items: Vec<String> = ui
        .fe
        .list(a)
        .unwrap()
        .items
        .iter()
        .map(|i| i.text.clone())
        .collect();
    assert_eq!(items[0], "FIGHTER CLASS");
    let i = items.iter().position(|t| t.ends_with("ACOLYTE")).unwrap();
    let (x, y) = list_row_pos(&ui, "LM_ShipsToLaunch", i);
    click_at(&mut ui, &mut host, x, y);
    shot(&mut ui, "launch_manager");
    let out = click(&mut ui, &mut host, "LM_LaunchOne");
    assert!(out.contains(&GameRequest::Launch {
        launcher: MOTHERSHIP,
        ships: vec![20]
    }));
    let out = click(&mut ui, &mut host, "LM_Launch");
    assert!(out.contains(&GameRequest::Launch {
        launcher: MOTHERSHIP,
        ships: vec![21, 22]
    }));
    let out = click(&mut ui, &mut host, "LM_AutoLaunchM");
    assert!(out.contains(&GameRequest::SetAutoLaunch {
        launcher: MOTHERSHIP,
        on: true
    }));
    let out = click(&mut ui, &mut host, "LM_LaunchAll");
    assert!(out
        .iter()
        .any(|r| matches!(r, GameRequest::Launch { ships, .. } if ships.len() == 2)));
    assert!(host.launchers[0].docked.is_empty());
    // Repair priority buttons of the first module row.
    let a = ui.fe.find_atom("LM_Module_Slot1").unwrap();
    let r = ui.fe.atom(a).rect();
    let out = click_at(&mut ui, &mut host, r.x1 - 20, r.y0 + 10);
    assert!(out.contains(&GameRequest::SetRepairPriority {
        module_type: 0x18,
        priority: 2
    }));
    click(&mut ui, &mut host, "LM_Carrier1Draw");
    assert_eq!(ui.state().lm.launcher, Some(CARRIER_1));
    shot(&mut ui, "launch_manager_carrier");
}

/// A host whose game has something for Escape to cancel.
struct Busy<'a>(&'a MockHost);

impl GameUiHost for Busy<'_> {
    fn player(&self) -> PlayerInfo {
        self.0.player()
    }
    fn game_time(&self) -> f32 {
        self.0.game_time()
    }
    fn factories(&self) -> Vec<Factory> {
        self.0.factories()
    }
    fn build_list(&self, f: ShipId) -> Vec<BuildRow> {
        self.0.build_list(f)
    }
    fn build_jobs(&self) -> Vec<BuildJob> {
        self.0.build_jobs()
    }
    fn labs(&self) -> Vec<Lab> {
        self.0.labs()
    }
    fn techs(&self) -> Vec<Tech> {
        self.0.techs()
    }
    fn research_rows(&self) -> Vec<ResearchRow> {
        self.0.research_rows()
    }
    fn launchers(&self) -> Vec<Launcher> {
        self.0.launchers()
    }
    fn modules(&self) -> Vec<Module> {
        self.0.modules()
    }
    fn selection(&self) -> Vec<ShipInfo> {
        self.0.selection()
    }
    fn available_commands(&self) -> Vec<Command> {
        self.0.available_commands()
    }
    fn objectives(&self) -> Vec<Objective> {
        self.0.objectives()
    }
    fn messages(&self) -> Vec<GameMessage> {
        self.0.messages()
    }
    fn escape_pending(&self) -> bool {
        true
    }
}

#[test]
fn key_dispatch_to_host_requests() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    assert!(key(&mut ui, &mut host, 68).contains(&GameRequest::Command(Command::Dock)));
    assert!(key(&mut ui, &mut host, 113).contains(&GameRequest::Command(Command::TacticsEvasive)));
    assert!(key(&mut ui, &mut host, 120)
        .contains(&GameRequest::Command(Command::Formation(Formation::Wall))));
    assert!(key(&mut ui, &mut host, 77).contains(&GameRequest::Action {
        action: keys::MOVE,
        down: true
    }));
    // Shift+S scuttles; plain S would open the launch manager.
    input(&mut ui, &mut host, InputEvent::KeyDown(0x10));
    let out = key(&mut ui, &mut host, 83);
    input(&mut ui, &mut host, InputEvent::KeyUp(0x10));
    assert!(out.contains(&GameRequest::Command(Command::Scuttle)));
    assert_eq!(ui.manager(), None);
    frame(&mut ui, &mut host);
    // Ctrl+1 creates group 1; 1 selects it.
    input(&mut ui, &mut host, InputEvent::KeyDown(0x11));
    let out = key(&mut ui, &mut host, 49);
    input(&mut ui, &mut host, InputEvent::KeyUp(0x11));
    assert!(out.contains(&GameRequest::Action {
        action: keys::Action(108),
        down: true
    }));
    assert!(key(&mut ui, &mut host, 49).contains(&GameRequest::Action {
        action: keys::Action(117),
        down: true
    }));
    // Shift alone: height movement down, then up.
    input(&mut ui, &mut host, InputEvent::KeyDown(0x10));
    let down = frame(&mut ui, &mut host);
    assert!(down.contains(&GameRequest::Action {
        action: keys::ENABLE_HEIGHT_MOVE,
        down: true
    }));
    input(&mut ui, &mut host, InputEvent::KeyUp(0x10));
    let up = frame(&mut ui, &mut host);
    assert!(up.contains(&GameRequest::Action {
        action: keys::ENABLE_HEIGHT_MOVE,
        down: false
    }));
    // Zoom is polled, not dispatched.
    input(&mut ui, &mut host, InputEvent::KeyDown(107));
    assert!(ui.action_held(keys::ZOOM_IN));
    assert!(frame(&mut ui, &mut host).is_empty());
    input(&mut ui, &mut host, InputEvent::KeyUp(107));
    assert!(!ui.action_held(keys::ZOOM_IN));
    // V cycles the overlay view; Tab the ship info panel.
    key(&mut ui, &mut host, 86);
    assert_eq!(ui.state().overlay.view, 1);
    key(&mut ui, &mut host, 9);
    assert!(ui.state().ship_print.on);
    // Escape opens the escape menu and pauses single player.
    let out = key(&mut ui, &mut host, 27);
    assert_eq!(ui.manager(), Some(Manager::EscMenu));
    assert_eq!(ui.fe.current_screen_name(), Some("In_game_esc_menu2"));
    assert!(out.contains(&GameRequest::SetPaused(true)));
    shot(&mut ui, "esc_menu");
    let out = click(&mut ui, &mut host, "IG_Cancel");
    assert_eq!(ui.manager(), None);
    assert!(out.contains(&GameRequest::SetPaused(false)));
    // With something to cancel in the game, Escape goes to the game.
    let busy = MockHost::somtaaw();
    let b = Busy(&busy);
    ui.handle_input(&b, InputEvent::KeyDown(27));
    let out = ui.update(&b, 0.03);
    ui.handle_input(&b, InputEvent::KeyUp(27));
    assert!(out.contains(&GameRequest::Action {
        action: keys::ESCAPE,
        down: true
    }));
    assert_eq!(ui.manager(), None);
}

#[test]
fn popup_menu_items_and_submenus() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    input(&mut ui, &mut host, InputEvent::MouseMove { x: 200, y: 120 });
    ui.open_popup(&host, false);
    let labels = |ui: &InGameUi| -> Vec<String> {
        ui.state()
            .popup
            .items
            .iter()
            .map(|i| i.label.clone())
            .collect()
    };
    let items = labels(&ui);
    assert!(items.contains(&"Dock".to_string()) && items.contains(&"Tactics".to_string()));
    assert!(!items.contains(&"Upgrade".to_string()));
    // Hover "Tactics": its submenu opens; click "Evasive".
    let t = ui
        .state()
        .popup
        .items
        .iter()
        .find(|i| i.label == "Tactics")
        .unwrap()
        .rect;
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: t.x0 + 5,
            y: t.y0 + 3,
        },
    );
    let (_, sub) = ui.state().popup.sub.clone().expect("submenu");
    assert_eq!(sub[0].label, "Evasive");
    assert!(sub[1].checked, "neutral is the selection's tactics");
    frame(&mut ui, &mut host);
    shot(&mut ui, "popup_menu_tactics");
    let r = sub[0].rect;
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseMove {
            x: r.x0 + 5,
            y: r.y0 + 3,
        },
    );
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseButton {
            left: true,
            down: true,
        },
    );
    input(
        &mut ui,
        &mut host,
        InputEvent::MouseButton {
            left: true,
            down: false,
        },
    );
    let out = frame(&mut ui, &mut host);
    assert!(out.contains(&GameRequest::Command(Command::TacticsEvasive)));
    assert!(ui.state().popup.open.is_none());
    // Keyboard: Down to "Move", Enter runs it.
    ui.open_popup(&host, false);
    let first = ui.state().popup.highlight.unwrap();
    assert_eq!(ui.state().popup.items[first].label, "Special Action...");
    key(&mut ui, &mut host, 0x28);
    let h = ui.state().popup.highlight.unwrap();
    assert_eq!(ui.state().popup.items[h].label, "Move");
    let out = key(&mut ui, &mut host, 0x0d);
    assert!(out.contains(&GameRequest::Action {
        action: keys::MOVE,
        down: true
    }));
    // "Special Action..." turns into the special menu.
    ui.open_popup(&host, false);
    key(&mut ui, &mut host, 0x0d);
    assert_eq!(labels(&ui), vec!["Link".to_string()]);
    shot(&mut ui, "popup_menu_special");
    key(&mut ui, &mut host, 27);
    assert!(ui.state().popup.open.is_none());
}

#[test]
fn rebinding_in_the_options_keys_screen() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 27);
    click(&mut ui, &mut host, "OP_InGameOptions_Start");
    assert_eq!(ui.fe.current_screen_name(), Some("Game_options"));
    shot(&mut ui, "esc_game_options");
    click(&mut ui, &mut host, "Game_options-TO-Keys");
    assert_eq!(ui.fe.current_screen_name(), Some("Keys"));
    let a = ui.fe.find_atom("OP_KeyBindingsList").unwrap();
    assert_eq!(ui.fe.list(a).unwrap().items.len(), 147);
    shot(&mut ui, "esc_keys");
    // Dock is row 2: select it, Enter, press K.
    let (x, y) = list_row_pos(&ui, "OP_KeyBindingsList", 1);
    click_at(&mut ui, &mut host, x, y);
    key(&mut ui, &mut host, 0x0d);
    assert_eq!(ui.state().binding, Some(keys::DOCK));
    shot(&mut ui, "esc_keys_binding");
    key(&mut ui, &mut host, 75);
    assert_eq!(ui.state().binding, None);
    assert!(ui
        .state()
        .keys
        .of(keys::DOCK)
        .contains(&keys::Binding { key: 75, mods: 0 }));
    // Cancel restores the bindings.
    click(&mut ui, &mut host, "OP_InGameOptions_Cancel");
    assert_eq!(ui.fe.current_screen_name(), Some("Game_options"));
    assert_eq!(
        ui.state().keys.of(keys::DOCK),
        &[keys::Binding { key: 68, mods: 0 }]
    );
    click(&mut ui, &mut host, "IG_StartEscapeMenu");
    assert_eq!(ui.fe.current_screen_name(), Some("In_game_esc_menu2"));
    click(&mut ui, &mut host, "In_game_esc_menu2-TO-Quit_game2");
    assert_eq!(ui.fe.current_screen_name(), Some("Quit_game2"));
    shot(&mut ui, "esc_quit");
    let out = click(&mut ui, &mut host, "QuitToMain");
    assert!(out.contains(&GameRequest::QuitToMain));
    assert_eq!(ui.manager(), None);
}

#[test]
fn overlay_objectives_trader_game_over() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    shot(&mut ui, "overlay_view1");
    // Clicking the selected-ships list narrows the selection.
    let out = click_at(&mut ui, &mut host, 640 - 140, 14);
    assert!(out.iter().any(|r| matches!(
        r,
        GameRequest::SelectType {
            ship_type: 0x01,
            ..
        }
    )));
    // The ship info panel.
    key(&mut ui, &mut host, 9);
    shot(&mut ui, "overlay_ship_info");
    key(&mut ui, &mut host, 9);
    // Key binding page while Show_Keys_1 is held.
    input(&mut ui, &mut host, InputEvent::KeyDown(35));
    frame(&mut ui, &mut host);
    assert_eq!(ui.state().overlay.keys_page, Some(1));
    shot(&mut ui, "overlay_keys_page1");
    input(&mut ui, &mut host, InputEvent::KeyUp(35));
    frame(&mut ui, &mut host);
    // Objectives: clicking one opens fleet intelligence (paused).
    let h = ui
        .fe
        .resources()
        .font("default.hff")
        .map(|f| hwc_ui::text::height(&f))
        .unwrap();
    let bar = (480 - (2 * h + h + 24)).min(480 - h - 30) - h;
    let out = click_at(&mut ui, &mut host, 30, bar + h + 4 + h / 2);
    assert_eq!(ui.manager(), Some(Manager::Objectives));
    assert!(out.contains(&GameRequest::SetPaused(true)));
    shot(&mut ui, "objectives");
    click(&mut ui, &mut host, "PO_CloseObjectiveWindow");
    assert_eq!(ui.manager(), None);
    // Views 2 and 3.
    key(&mut ui, &mut host, 86);
    shot(&mut ui, "overlay_view2");
    key(&mut ui, &mut host, 86);
    shot(&mut ui, "overlay_view3");
    key(&mut ui, &mut host, 86);
    // Trader.
    ui.open(&host, Manager::Trader);
    frame(&mut ui, &mut host);
    assert_eq!(ui.fe.current_screen_name(), Some("Trader_Interface"));
    shot(&mut ui, "trader");
    let out = click(&mut ui, &mut host, "TM_AcceptOffer");
    assert!(out
        .iter()
        .any(|r| matches!(r, GameRequest::TraderAccept(_))));
    click(&mut ui, &mut host, "TM_Leave");
    // Game over.
    ui.game_over(&host);
    frame(&mut ui, &mut host);
    assert_eq!(ui.fe.current_screen_name(), Some("SP_Game_Over"));
    shot(&mut ui, "esc_game_over");
    let out = click(&mut ui, &mut host, "SP_Restart_Level");
    assert!(out.contains(&GameRequest::RestartLevel));
}

#[test]
fn every_in_game_screen_starts_and_draws() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    let names: Vec<String> = ui.fe.screens.iter().map(|s| s.name.clone()).collect();
    let mut canvas = Canvas::new(640, 480);
    for n in names {
        ui.fe.all_screens_delete();
        ui.fe.screen_start(&n);
        ui.fe.process(0.016);
        ui.fe.render_over(&mut canvas);
    }
    ui.fe.all_screens_delete();
    // Larger screens keep the layouts centred.
    ui.resize(1024, 768);
    key(&mut ui, &mut host, 66);
    let mut big = Canvas::new(1024, 768);
    big.clear([4, 6, 14, 255]);
    ui.draw(&mut big);
    if let Some(dir) = std::env::var_os("HWC_SCREENSHOTS") {
        let p = std::path::Path::new(&dir).join("ingame");
        std::fs::create_dir_all(&p).unwrap();
        big.save_png(&p.join("construction_manager_1024x768.png"))
            .unwrap();
    }
}

/// One Escape closes the build and research managers (`feScreenEscape` on the screen's
/// default-back button), not two.
#[test]
fn one_escape_closes_a_manager() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    for open in [66u16, 82] {
        key(&mut ui, &mut host, open);
        assert!(ui.manager().is_some());
        settle(&mut ui, &mut host);
        key(&mut ui, &mut host, 27);
        settle(&mut ui, &mut host);
        assert_eq!(ui.manager(), None, "after one Escape: {:?}", ui.fe.stack_names());
        // The same Escape must not go on to open the escape menu.
        settle(&mut ui, &mut host);
        assert!(ui.fe.stack_names().is_empty(), "{:?}", ui.fe.stack_names());
    }
}

/// A quick second click on the selected row adds one ship, as the slow one does: the double
/// click replaces the second press (`WM_LBUTTONDBLCLK`), it does not come on top of it.
#[test]
fn double_clicking_a_selected_row_adds_one() {
    let Some(src) = source() else { return };
    let mut ui = ui(src);
    let mut host = MockHost::somtaaw();
    frame(&mut ui, &mut host);
    key(&mut ui, &mut host, 66);
    let (x, y) = cm_row_pos(&ui, cm_row_of(&ui, 0x00));
    // Select, then at once click again: one more.
    click_at(&mut ui, &mut host, x, y);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.pending.get(&0x00), Some(&1));
    // A third quick click starts a new pair: one more again.
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.pending.get(&0x00), Some(&2));
    settle(&mut ui, &mut host);
    click_at(&mut ui, &mut host, x, y);
    assert_eq!(ui.state().cm.pending.get(&0x00), Some(&3));
}
