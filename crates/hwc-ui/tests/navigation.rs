//! Front-end manager behaviour on hand-built screens (no game data needed): links, callbacks,
//! hot keys, mouse states, toggles, radio groups, sliders, keyboard focus, popups.

use std::rc::Rc;

use hwc_formats::fib::{self, Atom, AtomData, AtomKind, Fib, Link, Screen};
use hwc_ui::assets::{MemorySource, Resources};
use hwc_ui::consts::*;
use hwc_ui::fe::{Call, FrontEnd};
use hwc_ui::Canvas;

#[derive(Default)]
struct Log {
    calls: Vec<String>,
}

type Fe = FrontEnd<Log>;

fn atom(kind: u8, name: Option<&str>, flags: u32, rect: (i32, i32, i32, i32)) -> Atom {
    Atom {
        name: name.map(String::from),
        flags,
        raw_kind: kind,
        kind: AtomKind::from_raw(kind),
        border_width: 1,
        tab_stop: 0,
        border_color: [0, 50, 100, 255],
        content_color: [0, 0, 0, 255],
        rect: fib::Rect {
            left: rect.0,
            top: rect.1,
            right: rect.2,
            bottom: rect.3,
        },
        data: AtomData::None,
        bitmap: None,
        hotkey_modifiers: 0,
        hotkeys: [0; 5],
        draw_style: [1, 0xcdcd_cdcd],
    }
}

fn base(flags: u32) -> Atom {
    atom(0, None, flags, (0, 0, 640, 480))
}

fn label(text: &str, rect: (i32, i32, i32, i32)) -> Atom {
    let mut a = atom(0, None, 0x30840, rect);
    a.data = AtomData::Text {
        text: text.into(),
        font: Some("HW_EuroseCond_11.hff".into()),
    };
    a
}

fn link(from: &str, to: &str, flags: u32) -> Link {
    Link {
        name: Some(format!("{from}-TO-{to}")),
        flags,
        target: Some(to.into()),
    }
}

fn hot(mut a: Atom, key: u8) -> Atom {
    a.hotkeys[0] = key;
    a
}

fn tab(mut a: Atom, t: u16) -> Atom {
    a.tab_stop = t;
    a
}

fn radio(value: u32, rect: (i32, i32, i32, i32)) -> Atom {
    let mut a = atom(
        FA_RADIO_BUTTON,
        Some("Difficulty"),
        FAF_FUNCTION | FAF_CALL_ON_CREATE,
        rect,
    );
    a.data = AtomData::RadioValue(value);
    a
}

/// Two screens linked both ways, a popup and a modal popup.
fn test_fib() -> Fib {
    let main = Screen {
        name: "Main".into(),
        flags: 0,
        links: vec![link("Main", "Options", 1)],
        atoms: vec![
            base(0x848),
            tab(
                hot(
                    atom(
                        FA_BUTTON,
                        Some("Main-TO-Options"),
                        FAF_LINK,
                        (100, 100, 300, 132),
                    ),
                    b'O',
                ),
                1,
            ),
            label("&OPTIONS", (150, 108, 250, 122)),
            tab(
                hot(
                    atom(
                        FA_BUTTON,
                        Some("DoThing"),
                        FAF_FUNCTION | FAF_DEFAULT_OK,
                        (100, 140, 300, 172),
                    ),
                    b'D',
                ),
                2,
            ),
            {
                let mut q = hot(
                    atom(
                        FA_BUTTON,
                        Some("OpenQuit"),
                        FAF_FUNCTION | FAF_DEFAULT_BACK,
                        (100, 180, 300, 212),
                    ),
                    b'Q',
                );
                q.hotkey_modifiers = HKM_CONTROL;
                tab(q, 3)
            },
            atom(
                FA_BUTTON,
                Some("Disabled"),
                FAF_FUNCTION | FAF_DISABLED,
                (100, 220, 300, 252),
            ),
            // Entirely off the 640x480 layout: hidden, but still a region (hot key works).
            hot(
                atom(
                    FA_BUTTON,
                    Some("Main-TO-Options"),
                    FAF_LINK,
                    (20, 620, 50, 640),
                ),
                b'X',
            ),
        ],
    };
    let options = Screen {
        name: "Options".into(),
        flags: 0,
        links: vec![link("Options", "Main", 1)],
        atoms: vec![
            base(0x848),
            tab(
                atom(
                    FA_BUTTON,
                    Some("Options-TO-Main"),
                    FAF_LINK | FAF_DEFAULT_BACK,
                    (10, 400, 120, 432),
                ),
                1,
            ),
            atom(
                FA_CHECK_BOX,
                Some("Check"),
                FAF_FUNCTION | FAF_CALL_ON_CREATE,
                (10, 10, 30, 26),
            ),
            radio(0, (10, 50, 30, 66)),
            radio(1, (40, 50, 60, 66)),
            radio(2, (70, 50, 90, 66)),
            atom(
                FA_HORIZ_SLIDER,
                Some("Volume"),
                FAF_FUNCTION | FAF_CALL_ON_CREATE,
                (100, 100, 300, 116),
            ),
            atom(
                FA_TOGGLE_BUTTON,
                Some("Toggle"),
                FAF_FUNCTION,
                (10, 200, 120, 232),
            ),
        ],
    };
    let mut popup_base = atom(0, None, 0x848 | FAF_POPUP | FAF_MODAL, (200, 150, 440, 330));
    popup_base.draw_style = [1, 0];
    let quit = Screen {
        name: "Quit".into(),
        flags: 0,
        links: vec![],
        atoms: vec![
            popup_base,
            hot(
                atom(
                    FA_BUTTON,
                    Some("QuitYes"),
                    FAF_FUNCTION,
                    (250, 200, 390, 232),
                ),
                b'Y',
            ),
            hot(
                atom(
                    FA_BUTTON,
                    Some("SCREEN_Disappear"),
                    FAF_FUNCTION | FAF_DEFAULT_BACK,
                    (250, 250, 390, 282),
                ),
                b'N',
            ),
        ],
    };
    Fib {
        version: fib::VERSION,
        screens: vec![main, options, quit],
    }
}

fn logger(fe: &mut Fe, call: &Call) {
    let first = if fe.first_call(call) { " (create)" } else { "" };
    let msg = format!("{}{first}", call.name);
    fe.game.calls.push(msg);
}

fn setup() -> Fe {
    let res = Rc::new(Resources::new(Rc::new(MemorySource::default())));
    let mut fe = FrontEnd::new(Log::default(), res);
    for name in ["DoThing", "Disabled", "Check", "Toggle", "QuitYes"] {
        fe.callback_add(name, logger);
    }
    fe.callback_add("OpenQuit", |fe, call| {
        logger(fe, call);
        let b = fe.current_base();
        fe.screen_start_from(b, "Quit");
    });
    fe.callback_add("Difficulty", |fe, call| {
        if fe.first_call(call) {
            fe.radio_set(&call.name, 1);
        }
        logger(fe, call);
    });
    fe.callback_add("Volume", |fe, call| {
        if fe.first_call(call) {
            fe.slider_setup(call.atom.unwrap(), 100, 40, Some("volume"));
        }
    });
    fe.slider_handler_add("volume", |fe, id| {
        let v = match &fe.region(id).unwrap().control {
            hwc_ui::controls::Control::Slider(s) => s.value,
            _ => 0,
        };
        fe.game.calls.push(format!("volume={v}"));
    });
    fe.load_fib(&test_fib());
    fe.screen_start("Main");
    fe.process(0.016);
    fe
}

fn frame(fe: &mut Fe) {
    fe.process(1.0 / 60.0);
}

fn click(fe: &mut Fe, x: i32, y: i32) {
    fe.mouse_move(x, y);
    frame(fe);
    fe.mouse_button(true, true);
    frame(fe);
    fe.mouse_button(true, false);
    frame(fe);
}

fn press_key(fe: &mut Fe, k: u16) {
    fe.key_down(k);
    frame(fe);
    fe.key_up(k);
    frame(fe);
}

fn region_status(fe: &Fe, name: &str) -> u32 {
    let a = fe.find_atom(name).unwrap();
    fe.region(fe.atom(a).region.unwrap()).unwrap().status
}

#[test]
fn link_buttons_switch_screens() {
    let mut fe = setup();
    assert_eq!(fe.stack_names(), ["Main"]);
    click(&mut fe, 200, 116);
    assert_eq!(fe.stack_names(), ["Options"]);
    // Default-back button returns.
    click(&mut fe, 60, 416);
    assert_eq!(fe.stack_names(), ["Main"]);
}

#[test]
fn clicks_need_press_and_release_inside() {
    let mut fe = setup();
    fe.mouse_move(200, 156);
    frame(&mut fe);
    assert!(region_status(&fe, "DoThing") & RSF_MOUSE_INSIDE != 0);
    fe.mouse_button(true, true);
    frame(&mut fe);
    assert!(region_status(&fe, "DoThing") & RSF_LEFT_PRESSED != 0);
    // Dragging off and releasing outside does nothing.
    fe.mouse_move(500, 400);
    frame(&mut fe);
    fe.mouse_button(true, false);
    frame(&mut fe);
    assert!(fe.game.calls.is_empty());
    assert_eq!(
        region_status(&fe, "DoThing") & (RSF_LEFT_PRESSED | RSF_MOUSE_INSIDE),
        0
    );
    click(&mut fe, 200, 156);
    assert_eq!(fe.game.calls, ["DoThing"]);
}

#[test]
fn hot_keys_with_modifiers() {
    let mut fe = setup();
    press_key(&mut fe, u16::from(b'D'));
    assert_eq!(fe.game.calls, ["DoThing"]);
    // Ctrl+Q needed for OpenQuit.
    press_key(&mut fe, u16::from(b'Q'));
    assert_eq!(fe.game.calls.len(), 1);
    fe.key_down(key::CONTROL);
    press_key(&mut fe, u16::from(b'Q'));
    fe.key_up(key::CONTROL);
    assert_eq!(fe.game.calls.last().map(String::as_str), Some("OpenQuit"));
    assert_eq!(fe.stack_names(), ["Main", "Quit"]);
}

#[test]
fn hidden_off_screen_atom_keeps_its_hot_key() {
    let mut fe = setup();
    let main = fe.find_screen("Main").unwrap();
    assert!(fe.screens[main].atoms[6].has(FAF_HIDDEN));
    press_key(&mut fe, u16::from(b'X'));
    assert_eq!(fe.stack_names(), ["Options"]);
}

#[test]
fn disabled_buttons_ignore_input() {
    let mut fe = setup();
    click(&mut fe, 200, 236);
    assert!(fe.game.calls.is_empty());
    assert!(region_status(&fe, "Disabled") & RSF_REGION_DISABLED != 0);
}

#[test]
fn labels_become_children_of_their_buttons() {
    let fe = setup();
    let main = fe.find_screen("Main").unwrap();
    let label = fe.screens[main].atoms[2].region.unwrap();
    let button = fe.screens[main].atoms[1].region.unwrap();
    assert_eq!(fe.region(label).unwrap().parent, Some(button));
}

#[test]
fn modal_popup_blocks_the_screen_below() {
    let mut fe = setup();
    click(&mut fe, 200, 196);
    assert_eq!(fe.stack_names(), ["Main", "Quit"]);
    fe.game.calls.clear();
    // The Main button under the popup's area does not get the click...
    click(&mut fe, 200, 156);
    assert!(fe.game.calls.is_empty());
    // ...nor its hot key.
    press_key(&mut fe, u16::from(b'D'));
    assert!(fe.game.calls.is_empty());
    press_key(&mut fe, u16::from(b'Y'));
    assert_eq!(fe.game.calls, ["QuitYes"]);
    // Without tab stops a screen gets no keyboard-navigation handlers, so Escape does nothing
    // (the shipped Quit_game screen has a hidden Escape hot-key button for this).
    press_key(&mut fe, key::ESCAPE);
    assert_eq!(fe.stack_names(), ["Main", "Quit"]);
    press_key(&mut fe, u16::from(b'N'));
    assert_eq!(fe.stack_names(), ["Main"]);
}

#[test]
fn check_boxes_toggles_and_radio_groups() {
    let mut fe = setup();
    click(&mut fe, 200, 116);
    // Create callbacks ran with the first-call flag, last atom first.
    assert!(fe.game.calls.contains(&"Check (create)".to_string()));
    assert!(fe.game.calls.contains(&"Difficulty (create)".to_string()));
    let check = fe.find_atom("Check").unwrap();
    assert!(!fe.atom(check).checked());
    click(&mut fe, 20, 18);
    assert!(fe.atom(check).checked());
    click(&mut fe, 20, 18);
    assert!(!fe.atom(check).checked());
    let toggle = fe.find_atom("Toggle").unwrap();
    click(&mut fe, 60, 216);
    assert!(fe.atom(toggle).checked());
    // Radio group: value 1 set on create; clicking value 2 moves the check.
    let screen = fe.current_screen().unwrap();
    let group = fe.atoms_named(screen, "Difficulty");
    let checked = |fe: &Fe| {
        group
            .iter()
            .filter(|&&a| fe.atom(a).checked())
            .map(|&a| fe.atom(a).radio_value)
            .collect::<Vec<_>>()
    };
    assert_eq!(checked(&fe), [1]);
    click(&mut fe, 80, 58);
    assert_eq!(checked(&fe), [2]);
    // Arrow keys on the focused radio button step through the group.
    press_key(&mut fe, key::LEFT);
    assert_eq!(checked(&fe), [1]);
}

#[test]
fn slider_drag_is_clamped_to_the_slider() {
    let mut fe = setup();
    click(&mut fe, 200, 116);
    let vol = fe.find_atom("Volume").unwrap();
    assert_eq!(fe.slider(vol).unwrap().value, 40);
    fe.mouse_move(150, 108);
    frame(&mut fe);
    fe.mouse_button(true, true);
    frame(&mut fe);
    assert_eq!(fe.slider(vol).unwrap().value, 25);
    // Dragging past the end stays inside (the cursor is clipped to the slider).
    fe.mouse_move(600, 300);
    frame(&mut fe);
    assert_eq!(fe.slider(vol).unwrap().value, 99);
    fe.mouse_button(true, false);
    frame(&mut fe);
    assert_eq!(fe.game.calls.last().map(String::as_str), Some("volume=99"));
}

#[test]
fn keyboard_focus_and_default_buttons() {
    let mut fe = setup();
    // Tab stop 1 starts focused.
    assert!(region_status(&fe, "Main-TO-Options") & RSF_CURRENT_SELECTED != 0);
    press_key(&mut fe, key::TAB);
    assert!(region_status(&fe, "DoThing") & RSF_CURRENT_SELECTED != 0);
    assert_eq!(
        region_status(&fe, "Main-TO-Options") & RSF_CURRENT_SELECTED,
        0
    );
    press_key(&mut fe, key::SPACE);
    assert_eq!(fe.game.calls, ["DoThing"]);
    press_key(&mut fe, key::END);
    assert!(region_status(&fe, "OpenQuit") & RSF_CURRENT_SELECTED != 0);
    // Tab wraps around.
    press_key(&mut fe, key::TAB);
    assert!(region_status(&fe, "Main-TO-Options") & RSF_CURRENT_SELECTED != 0);
    // Return activates the default-OK button wherever the focus is.
    press_key(&mut fe, key::RETURN);
    assert_eq!(fe.game.calls, ["DoThing", "DoThing"]);
}

#[test]
fn unknown_callbacks_are_logged_once() {
    let mut fe = setup();
    fe.function_execute("NoSuchCallback", None, false);
    fe.function_execute("NoSuchCallback", None, false);
    assert_eq!(
        fe.log
            .iter()
            .filter(|m| m.contains("NoSuchCallback"))
            .count(),
        1
    );
}

#[test]
fn repositioning_centres_and_keeps_the_stack() {
    let mut fe = setup();
    click(&mut fe, 200, 116);
    fe.set_screen_size(1024, 768);
    assert_eq!(fe.stack_names(), ["Options"]);
    let a = fe.find_atom("Options-TO-Main").unwrap();
    assert_eq!((fe.atom(a).x, fe.atom(a).y), (10 + 192, 400 + 144));
    // The base region touches every edge and keeps its size (no background flag).
    let main = fe.find_screen("Main").unwrap();
    assert_eq!(fe.screens[main].atoms[1].x, 100 + 192);
    // Rendering at the new size works without game files (missing bitmaps are skipped).
    let mut canvas = Canvas::new(1024, 768);
    fe.render(&mut canvas);
}
