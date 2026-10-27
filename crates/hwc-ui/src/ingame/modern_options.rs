//! The modern options ("ENHANCEMENTS"): the recreation's own options (not in the original)
//! on one screen of the in-game options, reached from the options hub of the modern menus
//! ([`super::modern_menu`]). Each is a key of the options store ([`crate::options`]) that the
//! command line can also set (`--msaa`, `--zoom`, `--hud`...); one set on the command line is
//! shown as it is and cannot be changed here (the command line wins for this run).
//!
//! It works like the original's option screens: the changes wait until ACCEPT, which keeps
//! and saves them (`options_commit`) and asks the game to apply them
//! ([`GameRequest::ApplyOptions`]); CANCEL (or Escape) forgets them. Every one applies at
//! once, in the mission being played.

use crate::consts::*;
use crate::display::{self, Mode, Setting, VideoMode};
use crate::ingame::modern_hud;

use crate::ingame::modern_menu::{
    self as menu, Extra, Hit, Item, Kind, Layout, Model, Source, Style,
};
use crate::ingame::{modern_build_manager, modern_research_manager, Fe, GameRequest};
use crate::options::Options;

/// The modern screen's name (in place of an original screen's).
pub const NAME: &str = "hwc_Modern_options";
/// The options hub's button, and its hot key.
pub const HUB_LABEL: &str = "ENHANCEMENTS";
pub const HUB_KEY: &str = "E";
pub const TITLE: &str = "GAME OPTIONS";

/// How an option is set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Control {
    Toggle,
    /// One of these (label, value).
    Choice(&'static [(&'static str, i32)]),
    Slider {
        min: i32,
        max: i32,
        step: i32,
        unit: &'static str,
    },
    /// The resolution of the screen mode chosen above it (its value is the row of that
    /// mode's list, [`display::choices`]).
    Resolution,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    /// The options-file key.
    pub key: &'static str,
    pub label: &'static str,
    /// Index into [`SECTIONS`].
    pub section: usize,
    pub control: Control,
    /// It applies only after the game is started again.
    pub restart: bool,
    /// It only counts with the modern HUD on.
    pub needs_hud: bool,
}

const fn entry(key: &'static str, label: &'static str, section: usize, control: Control) -> Entry {
    Entry {
        key,
        label,
        section,
        control,
        restart: false,
        needs_hud: false,
    }
}

pub const SECTIONS: [&str; 4] = ["DISPLAY", "CAMERA", "CONTROLS", "INTERFACE"];

/// The resolution entry's key (not an options key: it stands for `screenWidth`,
/// `screenHeight` and [`display::REFRESH_KEY`]).
pub const RESOLUTION_KEY: &str = "hwcResolution";

/// Every modern option, in the screen's order.
pub const ENTRIES: &[Entry] = &[
    entry(
        display::MODE_KEY,
        "SCREEN MODE",
        0,
        Control::Choice(&[("WINDOW", 0), ("BORDERLESS", 1), ("FULL SCREEN", 2)]),
    ),
    entry(RESOLUTION_KEY, "RESOLUTION", 0, Control::Resolution),
    entry(
        "hwcRenderScale",
        "RENDER SCALE",
        0,
        Control::Slider {
            min: 50,
            max: 200,
            step: 5,
            unit: "%",
        },
    ),
    entry(
        "hwcMsaa",
        "ANTI-ALIASING (MSAA)",
        0,
        Control::Choice(&[("OFF", 1), ("2X", 2), ("4X", 4), ("8X", 8)]),
    ),
    entry("hwcFxaa", "FXAA EDGE SMOOTHING", 0, Control::Toggle),
    entry(
        "hwcSceneAtWindow",
        "SCALED MODES: 3D AT WINDOW RESOLUTION",
        0,
        Control::Toggle,
    ),
    entry(
        "hwcRemasteredFx",
        "REMASTERED EXPLOSIONS AND TRAILS",
        0,
        Control::Toggle,
    ),
    entry(
        "hwcFullDetailShips",
        "FULL-DETAIL SHIPS AT ANY DISTANCE",
        1,
        Control::Toggle,
    ),
    entry(
        "hwcRenderList",
        "DRAW DISTANCE",
        1,
        Control::Choice(&[("ORIGINAL", 1), ("FAR", 0)]),
    ),
    entry("hwcSmoothZoom", "SMOOTH WHEEL ZOOM", 1, Control::Toggle),
    entry("hwcFocusZoom", "FOCUS AGAIN ZOOMS IN", 1, Control::Toggle),
    entry(
        "hwcZoomClear",
        "KEEP THE CAMERA OUT OF HULLS",
        1,
        Control::Toggle,
    ),
    entry(
        "hwcRightClickMove",
        "RIGHT CLICK OPENS THE MOVE PLATE",
        2,
        Control::Toggle,
    ),
    entry(
        "hwcDoubleClickSelectType",
        "DOUBLE CLICK SELECTS ALL OF A TYPE",
        2,
        Control::Toggle,
    ),
    entry(
        "hwcTabShipIcons",
        "TAB CYCLES SHIP ICONS",
        2,
        Control::Toggle,
    ),
    entry(modern_hud::OPTION_KEY, "MODERN HUD", 3, Control::Toggle),
    entry("hwcOrderLines", "ORDER LINES", 3, Control::Toggle),
    entry(
        "hwcShipIcons",
        "SHIP ICONS",
        3,
        Control::Choice(&[("ALL", 2), ("NEARBY", 1), ("OFF", 0)]),
    ),
    entry(
        "hwcHealthBrackets",
        "BRACKETED HEALTH BARS",
        3,
        Control::Toggle,
    ),
    Entry {
        needs_hud: true,
        ..entry(
            modern_build_manager::OPTION_KEY,
            "MODERN BUILD MANAGER",
            3,
            Control::Toggle,
        )
    },
    Entry {
        needs_hud: true,
        ..entry(
            modern_research_manager::OPTION_KEY,
            "MODERN RESEARCH MANAGER",
            3,
            Control::Toggle,
        )
    },
    Entry {
        needs_hud: true,
        ..entry(
            menu::OPTION_KEY,
            "MODERN MENUS AND SYSTEMS MANAGER",
            3,
            Control::Toggle,
        )
    },
    entry(
        "hwcOriginalLetterbox",
        "CUTSCENE LETTERBOX",
        3,
        Control::Choice(&[("TALL", 0), ("ORIGINAL", 1)]),
    ),
];

/// The footer: ACCEPT, DEFAULTS, CANCEL.
const FOOTER: [(Extra, &str, &str); 3] = [
    (Extra::Accept, "ACCEPT", "A"),
    (Extra::Defaults, "DEFAULTS", "D"),
    (Extra::Cancel, "CANCEL", "C"),
];

/// The values being edited, and the keyboard focus (an entry, then the footer's buttons).
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub values: Vec<i32>,
    pub focus: usize,
    /// The screen mode and resolution being edited (the mode and resolution entries show it).
    pub display: Setting,
}

fn index_of(key: &str) -> usize {
    ENTRIES
        .iter()
        .position(|e| e.key == key)
        .expect("a modern option")
}

/// The resolutions offered for a mode on the monitor in use.
pub fn resolutions(fe: &Fe, mode: Mode) -> Vec<VideoMode> {
    display::choices(mode, fe.game.monitor.as_ref())
}

/// The resolution row of a setting (the nearest size that fits when its own is not listed).
fn resolution_row(fe: &Fe, s: &Setting) -> i32 {
    let list = resolutions(fe, s.mode);
    s.index_in(&list)
        .or_else(|| {
            list.iter()
                .enumerate()
                .filter(|(_, v)| display::fits(v.size(), s.size()))
                .max_by_key(|(_, v)| (v.width * v.height, v.refresh_mhz))
                .map(|(i, _)| i)
        })
        .unwrap_or(0) as i32
}

/// The value the command line set for this run, if it did.
pub fn locked(fe: &Fe, key: &str) -> Option<i32> {
    let switch = match key {
        modern_hud::OPTION_KEY => modern_hud::overridden(),
        modern_build_manager::OPTION_KEY => modern_build_manager::overridden(),
        modern_research_manager::OPTION_KEY => modern_research_manager::overridden(),
        menu::OPTION_KEY => menu::overridden(),
        _ => None,
    };
    switch.map(i32::from).or_else(|| {
        fe.game
            .ig
            .locked_options
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| *v)
    })
}

/// The value in effect now (the screen mode and resolution: the display in use).
pub fn current(fe: &Fe, e: &Entry) -> i32 {
    match e.key {
        display::MODE_KEY => fe.game.display.mode.option_value(),
        RESOLUTION_KEY => resolution_row(fe, &fe.game.display),
        _ => locked(fe, e.key).unwrap_or_else(|| fe.game.options.get(e.key)),
    }
}

/// Opens the modern options over the hub.
pub fn open(fe: &mut Fe) {
    let values = ENTRIES.iter().map(|e| current(fe, e)).collect();
    let display = fe.game.display;
    fe.game.ig.mmenu.page = Some(Page {
        values,
        focus: 0,
        display,
    });
    fe.game.ig.mmenu.dropdown = None;
}

/// The screen.
pub fn model(fe: &Fe, page: &Page) -> Model {
    let hud = ENTRIES
        .iter()
        .position(|e| e.key == modern_hud::OPTION_KEY)
        .is_none_or(|k| page.values[k] != 0);
    let mut items = Vec::new();
    for (k, e) in ENTRIES.iter().enumerate() {
        let v = page.values[k];
        let kind = match e.control {
            Control::Toggle => Kind::Toggle { on: v != 0 },
            Control::Choice(opts) => Kind::Choice {
                labels: opts.iter().map(|o| o.0.to_string()).collect(),
                sources: vec![Source::Option(k); opts.len()],
                current: opts.iter().position(|o| o.1 == v),
            },
            Control::Slider { min, max, unit, .. } => Kind::Slider {
                fraction: (v - min) as f32 / (max - min).max(1) as f32,
                readout: format!("{v}{unit}"),
                lo: format!("{min}{unit}"),
                hi: format!("{max}{unit}"),
            },
            Control::Resolution => {
                let list = resolutions(fe, page.display.mode);
                let labels: Vec<String> = if page.display.mode == Mode::Borderless {
                    vec![list
                        .first()
                        .map_or("DESKTOP".into(), |m| format!("{} x {}", m.width, m.height))]
                } else {
                    list.iter().map(VideoMode::label).collect()
                };
                Kind::Choice {
                    sources: vec![Source::Option(k); labels.len()],
                    current: if page.display.mode == Mode::Borderless {
                        Some(0)
                    } else {
                        (v >= 0 && (v as usize) < labels.len()).then_some(v as usize)
                    },
                    labels,
                }
            }
        };
        let mut it = item(kind, e.label, Source::Option(k), k);
        it.section = e.section;
        it.focused = page.focus == k;
        if e.control == Control::Resolution && page.display.mode == Mode::Borderless {
            // The desktop's own resolution; the 3D view's is the render scale's.
            it.disabled = true;
            it.note = Some("THE DESKTOP'S OWN".into());
        } else if locked(fe, e.key).is_some() {
            it.disabled = true;
            it.note = Some("SET ON THE COMMAND LINE".into());
        } else if e.restart {
            it.note = Some("APPLIES AFTER A RESTART".into());
        } else if e.needs_hud && !hud {
            it.note = Some("WITH THE MODERN HUD ONLY".into());
        }
        items.push(it);
    }
    for (n, (x, label, key)) in FOOTER.iter().enumerate() {
        let mut it = item(
            Kind::Button {
                primary: *x == Extra::Accept,
                back: *x == Extra::Cancel,
            },
            label,
            Source::Extra(*x),
            ENTRIES.len() + n,
        );
        it.key = key.to_string();
        it.footer = true;
        it.focused = page.focus == ENTRIES.len() + n;
        items.push(it);
    }
    Model {
        name: NAME.into(),
        style: Style::Form,
        title: TITLE.into(),
        subtitle: Some(HUB_LABEL.into()),
        messages: Vec::new(),
        sections: SECTIONS.iter().map(|s| s.to_string()).collect(),
        items,
    }
}

fn item(kind: Kind, label: &str, source: Source, k: usize) -> Item {
    Item {
        kind,
        label: label.to_string(),
        key: String::new(),
        source,
        section: 0,
        footer: false,
        disabled: false,
        focused: false,
        note: None,
        at: crate::canvas::Rect::new(0, k as i32 * 10, 640, k as i32 * 10 + 10),
    }
}

fn page_mut(fe: &mut Fe) -> Option<&mut Page> {
    fe.game.ig.mmenu.page.as_mut()
}

fn entry_of(m: &Model, i: usize) -> Option<usize> {
    match m.items.get(i)?.source {
        Source::Option(k) => Some(k),
        _ => None,
    }
}

/// Sets an entry's value (unless the command line set it). The screen mode and resolution
/// edit the page's display too: a new mode keeps the size when it has it (else the nearest
/// smaller), a resolution row is picked from the mode's list.
fn set(fe: &mut Fe, k: usize, v: i32) {
    if locked(fe, ENTRIES[k].key).is_some() {
        return;
    }
    let Some(mut s) = fe.game.ig.mmenu.page.as_ref().map(|p| p.display) else {
        return;
    };
    let res = index_of(RESOLUTION_KEY);
    let mut row = None;
    match ENTRIES[k].key {
        display::MODE_KEY => {
            s.mode = Mode::from_option(v);
            let r = resolution_row(fe, &s);
            if s.mode != Mode::Borderless {
                if let Some(&m) = resolutions(fe, s.mode).get(r as usize) {
                    s.pick(m);
                }
            }
            row = Some(r);
        }
        RESOLUTION_KEY => {
            let list = resolutions(fe, s.mode);
            let Some(&m) = list.get(v.max(0) as usize) else {
                return;
            };
            s.pick(m);
        }
        _ => {}
    }
    if let Some(p) = page_mut(fe) {
        p.values[k] = v;
        p.display = s;
        if let Some(r) = row {
            p.values[res] = r;
        }
    }
}

/// The number of rows of a choice or of the resolution list.
fn choice_len(fe: &Fe, k: usize) -> usize {
    match ENTRIES[k].control {
        Control::Choice(opts) => opts.len(),
        Control::Resolution => fe
            .game
            .ig
            .mmenu
            .page
            .as_ref()
            .filter(|p| p.display.mode != Mode::Borderless)
            .map_or(0, |p| resolutions(fe, p.display.mode).len()),
        _ => 0,
    }
}

fn value(fe: &Fe, k: usize) -> i32 {
    fe.game.ig.mmenu.page.as_ref().map_or(0, |p| p.values[k])
}

fn toggle(fe: &mut Fe, k: usize) {
    let v = value(fe, k);
    set(fe, k, i32::from(v == 0));
}

/// A step along a choice or a slider (or a toggle flipped).
fn step(fe: &mut Fe, k: usize, delta: i32) {
    if locked(fe, ENTRIES[k].key).is_some() {
        return;
    }
    let v = value(fe, k);
    match ENTRIES[k].control {
        Control::Toggle => {
            fe.ui_sound(if v != 0 {
                sound::TOGGLE_CHECKED
            } else {
                sound::TOGGLE_UNCHECKED
            });
            toggle(fe, k);
        }
        Control::Choice(opts) => {
            let cur = opts.iter().position(|o| o.1 == v).unwrap_or(0) as i32;
            let next = (cur + delta).clamp(0, opts.len() as i32 - 1) as usize;
            if next as i32 != cur {
                fe.ui_sound(sound::RADIO);
            }
            set(fe, k, opts[next].1);
        }
        Control::Slider { min, max, step, .. } => {
            set(fe, k, (v + delta * step).clamp(min, max));
        }
        Control::Resolution => {
            let n = choice_len(fe, k) as i32;
            if n == 0 {
                return;
            }
            let next = (v + delta).clamp(0, n - 1);
            if next != v {
                fe.ui_sound(sound::RADIO);
            }
            set(fe, k, next);
        }
    }
}

/// A press on one of the screen's controls (it acts on the release over it; a slider
/// follows the pointer at once).
pub fn press(fe: &mut Fe, m: &Model, l: &Layout, at: Hit) {
    let i = match at {
        Hit::Item(i) | Hit::Part(i, _) | Hit::Choice(i, _) => i,
        _ => return,
    };
    let Some(it) = m.items.get(i) else { return };
    if it.disabled {
        return;
    }
    if let Some(p) = page_mut(fe) {
        p.focus = i;
    }
    match (&it.kind, it.source) {
        (Kind::Button { .. }, _) => fe.ui_sound(sound::CLICK),
        (Kind::Toggle { on }, _) => fe.ui_sound(if *on {
            sound::TOGGLE_CHECKED
        } else {
            sound::TOGGLE_UNCHECKED
        }),
        (Kind::Choice { .. }, _) => fe.ui_sound(sound::RADIO),
        (Kind::Slider { .. }, Source::Option(_)) => {
            menu::start_slide(fe, i);
            let x = fe.game.ig.mmenu.mouse.0;
            slide(fe, m, l, i, x);
        }
        _ => {}
    }
}

/// A release over the control pressed.
pub fn release(fe: &mut Fe, m: &Model, at: Hit) {
    match at {
        Hit::Item(i) => match m.items.get(i).map(|it| (it.source, &it.kind)) {
            Some((Source::Extra(Extra::Enhancements), _)) => open(fe),
            Some((Source::Extra(Extra::Accept), _)) => accept(fe),
            Some((Source::Extra(Extra::Cancel), _)) => cancel(fe),
            Some((Source::Extra(Extra::Defaults), _)) => defaults(fe),
            Some((Source::Option(k), Kind::Toggle { .. })) => toggle(fe, k),
            _ => {}
        },
        Hit::Part(i, n) | Hit::Choice(i, n) => {
            if let Some(k) = entry_of(m, i) {
                match ENTRIES[k].control {
                    Control::Choice(opts) => {
                        if let Some(o) = opts.get(n) {
                            set(fe, k, o.1);
                        }
                    }
                    Control::Resolution if n < choice_len(fe, k) => set(fe, k, n as i32),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

/// Dragging a slider: its value under the pointer, on its step.
pub fn slide(fe: &mut Fe, m: &Model, l: &Layout, i: usize, x: i32) {
    let Some(k) = entry_of(m, i) else { return };
    let Control::Slider { min, max, step, .. } = ENTRIES[k].control else {
        return;
    };
    let t = l.placed[i].control;
    let f = ((x - t.x0) as f32 / t.width().max(1) as f32).clamp(0.0, 1.0);
    let raw = min as f32 + f * (max - min) as f32;
    let v = ((raw / step as f32).round() as i32 * step).clamp(min, max);
    set(fe, k, v);
}

/// A key while the screen is up (it takes every key).
pub fn key(fe: &mut Fe, code: u16) {
    let n = ENTRIES.len() + FOOTER.len();
    let Some(focus) = fe.game.ig.mmenu.page.as_ref().map(|p| p.focus) else {
        return;
    };
    let shift = fe.input.down.contains(&key::SHIFT);
    let move_to = |fe: &mut Fe, f: usize| {
        if let Some(p) = page_mut(fe) {
            p.focus = f % n;
        }
    };
    match code {
        key::ESCAPE => {
            fe.ui_sound(sound::CLICK);
            cancel(fe);
        }
        key::TAB if shift => move_to(fe, focus + n - 1),
        key::TAB | key::DOWN => move_to(fe, focus + 1),
        key::UP => move_to(fe, focus + n - 1),
        key::HOME => move_to(fe, 0),
        key::END => move_to(fe, n - 1),
        key::LEFT | key::RIGHT if focus < ENTRIES.len() => {
            let d = if code == key::RIGHT { 1 } else { -1 };
            if ENTRIES[focus].control == Control::Toggle {
                let on = value(fe, focus) != 0;
                if on != (d > 0) {
                    step(fe, focus, d);
                }
            } else {
                step(fe, focus, d);
            }
        }
        key::SPACE if focus < ENTRIES.len() => {
            if ENTRIES[focus].control == Control::Toggle {
                step(fe, focus, 1);
            } else {
                // A choice steps on, round to its first.
                match ENTRIES[focus].control {
                    Control::Choice(opts) => {
                        let v = value(fe, focus);
                        let cur = opts.iter().position(|o| o.1 == v).unwrap_or(0);
                        fe.ui_sound(sound::RADIO);
                        set(fe, focus, opts[(cur + 1) % opts.len()].1);
                    }
                    Control::Resolution => {
                        let n = choice_len(fe, focus) as i32;
                        if n > 0 {
                            fe.ui_sound(sound::RADIO);
                            set(fe, focus, (value(fe, focus) + 1) % n);
                        }
                    }
                    _ => {}
                }
            }
        }
        key::SPACE | key::RETURN if focus >= ENTRIES.len() => {
            fe.ui_sound(sound::CLICK);
            match FOOTER[focus - ENTRIES.len()].0 {
                Extra::Accept => accept(fe),
                Extra::Defaults => defaults(fe),
                _ => cancel(fe),
            }
        }
        key::RETURN => {
            fe.ui_sound(sound::CLICK);
            accept(fe);
        }
        c => {
            let hot = FOOTER.iter().find(|f| u16::from(f.2.as_bytes()[0]) == c);
            if let Some((x, _, _)) = hot {
                fe.ui_sound(sound::CLICK);
                match x {
                    Extra::Accept => accept(fe),
                    Extra::Defaults => defaults(fe),
                    _ => cancel(fe),
                }
            }
        }
    }
}

/// ACCEPT: the changed values are kept and saved, and applied.
pub fn accept(fe: &mut Fe) {
    let Some(page) = fe.game.ig.mmenu.page.take() else {
        return;
    };
    let mut changed = false;
    for (k, e) in ENTRIES.iter().enumerate() {
        if matches!(e.key, display::MODE_KEY | RESOLUTION_KEY) {
            continue;
        }
        if locked(fe, e.key).is_none() && fe.game.options.get(e.key) != page.values[k] {
            fe.game.options.set(e.key, page.values[k]);
            changed = true;
        }
    }
    // The screen mode and resolution: kept when the player changed them from the display in
    // use (the game switches to them). A display the command line asked for is not written to
    // the options by an ACCEPT that left it alone.
    if page.display != fe.game.display {
        page.display.store(&mut fe.game.options);
        changed = true;
    }
    if changed {
        fe.game.options_commit();
        fe.game.ig.request(GameRequest::ApplyOptions);
    }
}

/// CANCEL: back to the hub, nothing changed.
pub fn cancel(fe: &mut Fe) {
    fe.game.ig.mmenu.page = None;
}

/// DEFAULTS: every option (not set on the command line) back to its default (the screen
/// mode: a borderless window).
pub fn defaults(fe: &mut Fe) {
    let d = Options::default();
    for (k, e) in ENTRIES.iter().enumerate() {
        if e.key != RESOLUTION_KEY {
            set(fe, k, d.get(e.key));
        }
    }
}

/// The screen's layout on a canvas (for tests).
pub fn layout(fe: &Fe, width: i32, height: i32) -> Option<(Model, Layout)> {
    let m = model(fe, fe.game.ig.mmenu.page.as_ref()?);
    let l = menu::layout(&m, width, height);
    Some((m, l))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_is_a_known_option_with_a_valid_default() {
        let d = Options::default();
        for e in ENTRIES {
            let v = d.get(e.key);
            match e.control {
                Control::Resolution => assert_eq!(e.key, RESOLUTION_KEY),
                Control::Toggle => assert!(v == 0 || v == 1, "{}", e.key),
                Control::Choice(opts) => {
                    assert!(opts.iter().any(|o| o.1 == v), "{} = {v}", e.key)
                }
                Control::Slider { min, max, .. } => assert!((min..=max).contains(&v), "{}", e.key),
            }
            assert!(e.section < SECTIONS.len());
        }
        // In section order (the screen keeps sections together).
        assert!(ENTRIES.windows(2).all(|w| w[0].section <= w[1].section));
    }

    #[test]
    fn the_screen_lays_out_in_two_columns_and_fits() {
        let page = Page {
            values: ENTRIES
                .iter()
                .map(|e| Options::default().get(e.key))
                .collect(),
            focus: 0,
            display: Setting::default(),
        };
        // Without a front end: build the items as the model does.
        let items: Vec<Item> = ENTRIES
            .iter()
            .enumerate()
            .map(|(k, e)| {
                let mut it = item(
                    match e.control {
                        Control::Toggle => Kind::Toggle {
                            on: page.values[k] != 0,
                        },
                        Control::Choice(o) => Kind::Choice {
                            labels: o.iter().map(|o| o.0.to_string()).collect(),
                            sources: vec![Source::Option(k); o.len()],
                            current: Some(0),
                        },
                        Control::Slider { .. } => Kind::Slider {
                            fraction: 0.3,
                            readout: "100%".into(),
                            lo: String::new(),
                            hi: String::new(),
                        },
                        Control::Resolution => Kind::Choice {
                            labels: display::choices(Mode::Windowed, None)
                                .iter()
                                .map(VideoMode::label)
                                .collect(),
                            sources: vec![Source::Option(k); 19],
                            current: Some(0),
                        },
                    },
                    e.label,
                    Source::Option(k),
                    k,
                );
                it.section = e.section;
                it
            })
            .collect();
        let m = Model {
            name: NAME.into(),
            style: Style::Form,
            title: TITLE.into(),
            subtitle: None,
            messages: Vec::new(),
            sections: SECTIONS.iter().map(|s| s.to_string()).collect(),
            items,
        };
        for (w, h) in [(1920, 1080), (1024, 768), (640, 480)] {
            let l = menu::layout(&m, w, h);
            assert!(l.frame.panel.y1 <= h && l.frame.panel.x1 <= w);
            let xs: std::collections::BTreeSet<i32> = l.placed.iter().map(|p| p.row.x0).collect();
            assert_eq!(xs.len(), 2, "two columns at {w}x{h}");
            for (i, p) in l.placed.iter().enumerate() {
                match ENTRIES[i].control {
                    // The screen mode's three long names make a drop-down.
                    Control::Choice(_) if ENTRIES[i].key == display::MODE_KEY => {
                        assert!(p.dropdown);
                    }
                    Control::Choice(o) => {
                        assert!(!p.dropdown, "{} fits as segments", ENTRIES[i].label);
                        assert_eq!(p.parts.len(), o.len());
                    }
                    // The resolutions are a drop-down, every row of it on the canvas.
                    Control::Resolution => {
                        assert!(p.dropdown);
                        for r in l.dropdown_rows(i, 19, h) {
                            assert!(
                                r.y0 >= 0 && r.y1 <= h && r.x0 >= 0 && r.x1 <= w,
                                "{r:?} at {w}x{h}"
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}
