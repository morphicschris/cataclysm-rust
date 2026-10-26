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

/// Every modern option, in the screen's order.
pub const ENTRIES: &[Entry] = &[
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
    entry(modern_hud::OPTION_KEY, "MODERN HUD", 3, Control::Toggle),
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

/// The value in effect now.
pub fn current(fe: &Fe, e: &Entry) -> i32 {
    locked(fe, e.key).unwrap_or_else(|| fe.game.options.get(e.key))
}

/// Opens the modern options over the hub.
pub fn open(fe: &mut Fe) {
    let values = ENTRIES.iter().map(|e| current(fe, e)).collect();
    fe.game.ig.mmenu.page = Some(Page { values, focus: 0 });
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
        };
        let mut it = item(kind, e.label, Source::Option(k), k);
        it.section = e.section;
        it.focused = page.focus == k;
        if locked(fe, e.key).is_some() {
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

/// Sets an entry's value (unless the command line set it).
fn set(fe: &mut Fe, k: usize, v: i32) {
    if locked(fe, ENTRIES[k].key).is_some() {
        return;
    }
    if let Some(p) = page_mut(fe) {
        p.values[k] = v;
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
                if let Control::Choice(opts) = ENTRIES[k].control {
                    if let Some(o) = opts.get(n) {
                        set(fe, k, o.1);
                    }
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
                if let Control::Choice(opts) = ENTRIES[focus].control {
                    let v = value(fe, focus);
                    let cur = opts.iter().position(|o| o.1 == v).unwrap_or(0);
                    fe.ui_sound(sound::RADIO);
                    set(fe, focus, opts[(cur + 1) % opts.len()].1);
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
        if locked(fe, e.key).is_none() && fe.game.options.get(e.key) != page.values[k] {
            fe.game.options.set(e.key, page.values[k]);
            changed = true;
        }
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

/// DEFAULTS: every option (not set on the command line) back to its default.
pub fn defaults(fe: &mut Fe) {
    let d = Options::default();
    for (k, e) in ENTRIES.iter().enumerate() {
        set(fe, k, d.get(e.key));
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
                if let Control::Choice(o) = ENTRIES[i].control {
                    assert!(!p.dropdown, "{} fits as segments", ENTRIES[i].label);
                    assert_eq!(p.parts.len(), o.len());
                }
            }
        }
    }
}
