//! The in-game escape menu (`in_game_esc_menu.fib`: the pause menu, save and load, quit, the
//! confirmations, game over), the options and every one of their screens (audio, advanced
//! speech, equalizer, video, custom effects, gameplay, key bindings and their pop-ups), and
//! the objectives screen (`single_player_objective.fib`), restyled in the modern HUD's look:
//! a user-requested change, on by default; `--menus original`, `hwcModernMenus = 0` or the
//! original HUD (`--hud original`) bring back the original screens. The look is the modern
//! managers' ([`super::modern_manager`]); the options hub gets one screen of its own, the
//! modern options ([`super::modern_options`]).
//!
//! Only the look is new. The original screen stays open underneath, undrawn: its callbacks,
//! its keys (the hot keys, Tab and the arrows between its tab stops, Return on its default
//! button, Escape on its back button, the list's keys, typing into the text entry), and
//! every rule of the original's controls stay the same code. Each modern control is built
//! from the screen's own atoms (a button and the label on it, a check box and the label
//! beside it, a radio group and its heading, a slider with its heading and its LO / HI
//! ends, a list window, a text entry) and stands for them: the mouse over a control is
//! handed to the original region under it ([`super::modern_proxy`]), so a press, a drag
//! and a release do there what they always did. What the controls show (checked, the
//! chosen radio button, a slider's level, the list's rows, selection and scroll position,
//! the focus, the typed text) is read back from the original's state every frame.

use crate::canvas::{Canvas, Color, Rect, WHITE};
use crate::consts::*;
use crate::controls::LIST_MARGIN;
use crate::fe::FeAtom;
use crate::ingame::keys::Action;
use crate::ingame::modern_hud::{self, Face, Pen};
use crate::ingame::modern_manager::{self as mm, *};
use crate::ingame::modern_objectives as obj;
use crate::ingame::modern_options as mo;
use crate::ingame::modern_panel;
use crate::ingame::modern_proxy::{Proxy, Target};
use crate::ingame::{keylist, Fe, Manager};
use crate::options::Options;
use crate::region::AtomRef;

/// The options-file key: 1 (default) these menus, 0 the original's.
pub const OPTION_KEY: &str = "hwcModernMenus";

/// `--menus`, the options file and the HUD's choice.
static SWITCH: Switch = Switch::new(OPTION_KEY);

/// Sets the command line's choice; `None`: the options file decides.
pub fn set_override(modern: Option<bool>) {
    SWITCH.set_override(modern);
}

/// The command line's choice, if it made one.
pub fn overridden() -> Option<bool> {
    SWITCH.overridden()
}

/// Whether the modern menus (and the modern systems manager) replace the original's look.
pub fn enabled(options: &Options) -> bool {
    SWITCH.enabled(options)
}

/// Parses `--menus`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    mm::parse_mode(s)
}

/// Whether a modern menu is up now (enabled, and the escape menu or objectives open).
pub fn active(fe: &Fe) -> bool {
    matches!(
        fe.game.ig.manager,
        Some(Manager::EscMenu | Manager::Objectives)
    ) && enabled(&fe.game.options)
        && fe.current_screen().is_some()
}

/// The options hub, where the modern options are added.
pub const HUB: &str = "Game_options";

// ---- the model: what the screen shows ----

/// The modern options' buttons, and the hub's way to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Extra {
    /// The hub's ENHANCEMENTS button.
    Enhancements,
    Accept,
    Defaults,
    Cancel,
}

/// What a control stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Source {
    /// An atom of the original screen (its region gets the mouse).
    Atom(AtomRef),
    /// A button of the modern options or the hub's way there.
    Extra(Extra),
    /// One of the modern options ([`mo::ENTRIES`]).
    Option(usize),
    Nothing,
}

/// A key-binding row (`OP_KeyBindingsList`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeyRow {
    /// The group's name on its first action (`kbListItemDraw`'s "<Group> ----").
    pub group: Option<String>,
    pub action: String,
    pub keys: Vec<String>,
    /// Waiting for a key to bind.
    pub binding: bool,
}

/// A row of a list window.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub text: String,
    pub selectable: bool,
    pub selected: bool,
    pub keys: Option<KeyRow>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Button {
        /// The screen's default (Return) button.
        primary: bool,
        /// The screen's back (Escape) button.
        back: bool,
    },
    Toggle {
        on: bool,
    },
    /// A radio group: segments, or a drop-down when they do not fit.
    Choice {
        labels: Vec<String>,
        sources: Vec<Source>,
        current: Option<usize>,
    },
    Slider {
        fraction: f32,
        readout: String,
        lo: String,
        hi: String,
    },
    VSlider {
        fraction: f32,
    },
    List {
        rows: Vec<Row>,
        /// The first row shown, the rows there are and the page (the original's).
        top: usize,
        total: usize,
        page: usize,
    },
    Entry {
        text: String,
        focused: bool,
    },
    Text {
        paragraphs: Vec<String>,
    },
    Value {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub kind: Kind,
    pub label: String,
    /// The hot key shown in a key cap ("" for none).
    pub key: String,
    pub source: Source,
    /// Index into [`Model::sections`].
    pub section: usize,
    /// A footer button (Accept, Cancel...).
    pub footer: bool,
    pub disabled: bool,
    /// The screen's keyboard focus is on it.
    pub focused: bool,
    /// A remark under the label (the modern options: set on the command line).
    pub note: Option<String>,
    /// Where it is on the original's 640x480 layout.
    pub at: Rect,
}

impl Item {
    fn new(kind: Kind, label: String, source: Source, at: Rect) -> Item {
        Item {
            kind,
            label,
            key: String::new(),
            source,
            section: 0,
            footer: false,
            disabled: false,
            focused: false,
            note: None,
            at,
        }
    }
}

/// A list of buttons (the pause menu, the hub, a confirmation), or a form of settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Style {
    Menu,
    Form,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    /// The original screen's name (or the modern options').
    pub name: String,
    pub style: Style,
    pub title: String,
    pub subtitle: Option<String>,
    /// A confirmation's lines (menus only).
    pub messages: Vec<String>,
    /// Section headings ("" for the first, unnamed one).
    pub sections: Vec<String>,
    /// The controls: the body in its order (sections together), then the footer.
    pub items: Vec<Item>,
}

impl Model {
    /// The first item labelled `label`.
    pub fn find(&self, label: &str) -> Option<usize> {
        self.items.iter().position(|i| i.label == label)
    }

    /// The item standing for the atom named `name` (or one of its radio buttons).
    pub fn by_atom(&self, fe: &Fe, name: &str) -> Option<usize> {
        self.items.iter().position(|i| {
            let named = |s: &Source| match s {
                Source::Atom(a) => fe.atom(*a).name.as_deref() == Some(name),
                _ => false,
            };
            named(&i.source)
                || matches!(&i.kind, Kind::Choice { sources, .. } if sources.iter().any(named))
        })
    }
}

/// The modern screen up now: the modern options over the hub, else the screen on top.
pub fn model(fe: &Fe) -> Option<Model> {
    if let Some(page) = &fe.game.ig.mmenu.page {
        return Some(mo::model(fe, page));
    }
    fe.current_screen().map(|s| screen_model(fe, s))
}

/// A pop-up's screen below it, which is drawn too.
fn popup_below(fe: &Fe) -> Option<usize> {
    if fe.game.ig.mmenu.page.is_some() {
        return None;
    }
    let top = fe.current_screen()?;
    if !fe.screens[top].atoms[0].has(FAF_POPUP) {
        return None;
    }
    let n = fe.stack.len();
    (n >= 3).then(|| fe.stack[n - 2].screen).flatten()
}

/// A label: the atom's text in the current language, without the hot key's '&'.
fn atom_text(fe: &Fe, atom: &FeAtom) -> Option<String> {
    let lang = fe.resources().language().index();
    let t = atom.texts.get(lang).or(atom.text.as_ref())?;
    let s: String = t.iter().map(|&b| b as char).filter(|&c| c != '&').collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    (!s.is_empty()).then_some(s)
}

/// The hot key's name for a key cap.
pub fn hot_key_name(code: u8) -> String {
    match code {
        b'0'..=b'9' | b'A'..=b'Z' => (code as char).to_string(),
        27 => "ESC".into(),
        13 => "ENTER".into(),
        _ => String::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tag {
    Button,
    Toggle,
    Radio,
    Slider,
    VSlider,
    List,
    Entry,
    User,
}

/// A control found on the screen, while its labels are being found.
#[derive(Debug, Clone)]
struct Ctl {
    tag: Tag,
    atoms: Vec<AtomRef>,
    r: Rect,
    label: String,
    /// The radio buttons' own labels.
    options: Vec<String>,
    lo: String,
    hi: String,
    /// A figure drawn beside it by a user region (the channel count).
    value: Option<String>,
}

#[derive(Debug, Clone)]
struct Txt {
    r: Rect,
    text: String,
    big: bool,
    used: bool,
}

fn on_layout(r: Rect) -> bool {
    r.x1 > 0 && r.y1 > 0 && r.x0 < 640 && r.y0 < 480
}

fn mid_y(r: Rect) -> i32 {
    (r.y0 + r.y1) / 2
}

/// The texts to the right of a check box or radio button (one label, perhaps on two lines).
fn right_label(texts: &mut [Txt], c: Rect) -> Option<String> {
    let mut found: Vec<usize> = (0..texts.len())
        .filter(|&i| {
            let t = &texts[i];
            !t.used
                && (8..=34).contains(&(t.r.x0 - c.x0))
                && (c.y0 - 7..=c.y1 + 7).contains(&mid_y(t.r))
        })
        .collect();
    if found.is_empty() {
        return None;
    }
    found.sort_by_key(|&i| (texts[i].r.y0, texts[i].r.x0));
    let s = found
        .iter()
        .map(|&i| texts[i].text.clone())
        .collect::<Vec<_>>()
        .join(" ");
    for i in found {
        texts[i].used = true;
    }
    Some(s)
}

/// The nearest unused text above `c`, at most `gap` over it and starting within `left`
/// before / `right` after its left edge.
fn above_label(texts: &mut [Txt], c: Rect, gap: i32, left: i32, right: i32) -> Option<String> {
    let i = (0..texts.len())
        .filter(|&i| {
            let t = &texts[i];
            !t.used
                && t.r.y0 < c.y0
                && t.r.y1 <= c.y0 + 4
                && c.y0 - t.r.y1 <= gap
                && t.r.x0 >= c.x0 - left
                && t.r.x0 <= c.x0 + right
        })
        .max_by_key(|&i| texts[i].r.y1)?;
    texts[i].used = true;
    Some(texts[i].text.clone())
}

fn region_rect(fe: &Fe, a: AtomRef) -> Option<Rect> {
    fe.region(fe.atom(a).region?).map(|r| r.rect)
}

fn region_has(fe: &Fe, a: AtomRef, status: u32) -> bool {
    fe.atom(a)
        .region
        .and_then(|id| fe.region(id))
        .is_some_and(|r| r.has(status))
}

/// What a user region shows, for the ones a modern screen reads.
fn user_value(fe: &Fe, name: &str) -> Option<String> {
    match name {
        "OP_NumChannels" => {
            Some((fe.game.options.get("NumChannels") + crate::cataclysm::VOICE_MIN).to_string())
        }
        "PO_TextDraw" => {
            let ig = &fe.game.ig;
            Some(
                ig.esc
                    .objective
                    .and_then(|i| ig.view.objectives.get(i))
                    .map_or_else(|| ig.view.fleet_intel.clone(), |o| o.description.clone()),
            )
        }
        _ => None,
    }
}

/// A key-binding row's contents (`kbListItemDraw`'s).
fn key_row(fe: &Fe, data: u32) -> KeyRow {
    let a = Action(data as u16);
    let ig = &fe.game.ig;
    KeyRow {
        group: keylist::group_title(a).map(str::to_uppercase),
        action: ig
            .strings
            .key_description(a.name())
            .map_or_else(|| a.name().replace('_', " "), str::to_string)
            .to_uppercase(),
        keys: ig.keys.of(a).iter().map(|b| b.display()).collect(),
        binding: ig.binding == Some(a),
    }
}

/// The modern screen for an original screen of the escape menu or the objectives.
pub fn screen_model(fe: &Fe, screen: usize) -> Model {
    let s = &fe.screens[screen];
    let mut texts: Vec<Txt> = Vec::new();
    let mut ctls: Vec<Ctl> = Vec::new();
    let mut radios: Vec<(String, Vec<AtomRef>)> = Vec::new();
    for (i, atom) in s.atoms.iter().enumerate().skip(1) {
        let a = AtomRef { screen, atom: i };
        let r = atom.layout_rect();
        if atom.has(FAF_HIDDEN) || !on_layout(r) {
            continue;
        }
        if atom.kind == FA_STATIC_TEXT {
            if let Some(text) = atom_text(fe, atom) {
                texts.push(Txt {
                    r,
                    text,
                    big: r.height() >= 18,
                    used: false,
                });
            }
            continue;
        }
        if atom.region.is_none() {
            continue;
        }
        let tag = match atom.kind {
            FA_BUTTON => Tag::Button,
            FA_CHECK_BOX | FA_TOGGLE_BUTTON => Tag::Toggle,
            FA_RADIO_BUTTON => {
                let name = atom.name.clone().unwrap_or_default();
                match radios.iter_mut().find(|g| g.0 == name) {
                    Some(g) => g.1.push(a),
                    None => radios.push((name, vec![a])),
                }
                continue;
            }
            FA_HORIZ_SLIDER => Tag::Slider,
            FA_VERT_SLIDER => Tag::VSlider,
            FA_LIST_WINDOW => Tag::List,
            FA_TEXT_ENTRY => Tag::Entry,
            FA_USER_REGION
                if atom
                    .name
                    .as_deref()
                    .is_some_and(|n| user_value(fe, n).is_some()) =>
            {
                Tag::User
            }
            _ => continue,
        };
        ctls.push(Ctl {
            tag,
            atoms: vec![a],
            r,
            label: String::new(),
            options: Vec::new(),
            lo: String::new(),
            hi: String::new(),
            value: None,
        });
    }
    for (_, mut atoms) in radios {
        atoms.sort_by_key(|&a| {
            let r = fe.atom(a).layout_rect();
            (r.y0 / 6, r.x0)
        });
        let r = atoms
            .iter()
            .map(|&a| fe.atom(a).layout_rect())
            .reduce(|u, r| {
                Rect::new(
                    u.x0.min(r.x0),
                    u.y0.min(r.y0),
                    u.x1.max(r.x1),
                    u.y1.max(r.y1),
                )
            })
            .unwrap_or_default();
        ctls.push(Ctl {
            tag: Tag::Radio,
            atoms,
            r,
            label: String::new(),
            options: Vec::new(),
            lo: String::new(),
            hi: String::new(),
            value: None,
        });
    }

    let all_texts: Vec<Rect> = texts.iter().map(|t| t.r).collect();

    // The title (the topmost big text) and the subtitle under it.
    let first_ctl = ctls.iter().map(|c| c.r.y0).min().unwrap_or(480);
    let title_i = (0..texts.len())
        .filter(|&i| texts[i].big && texts[i].r.y0 < first_ctl)
        .min_by_key(|&i| (texts[i].r.y0, texts[i].r.x0))
        .or_else(|| {
            (0..texts.len())
                .filter(|&i| texts[i].r.y0 < first_ctl)
                .min_by_key(|&i| (texts[i].r.y0, texts[i].r.x0))
        });
    let mut title = String::new();
    let mut subtitle = None;
    if let Some(t) = title_i {
        texts[t].used = true;
        title = texts[t].text.trim_end_matches(':').trim().to_string();
        let tr = texts[t].r;
        if tr.y0 < 40 {
            if let Some(si) = (0..texts.len())
                .filter(|&i| {
                    let r = texts[i].r;
                    !texts[i].used
                        && (24..=34).contains(&(r.y0 - tr.y0))
                        && (r.x0 - tr.x0).abs() <= 24
                })
                .min_by_key(|&i| texts[i].r.y0)
            {
                texts[si].used = true;
                subtitle = Some(texts[si].text.clone());
            }
        }
    }

    // Labels: on the buttons, beside the check boxes and radio buttons, over the groups,
    // sliders and lists, under the vertical sliders.
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Button) {
        let inside: Vec<usize> = (0..texts.len())
            .filter(|&i| {
                let t = &texts[i];
                !t.used && c.r.contains((t.r.x0 + t.r.x1) / 2, mid_y(t.r))
            })
            .collect();
        c.label = inside
            .iter()
            .map(|&i| texts[i].text.clone())
            .collect::<Vec<_>>()
            .join(" ");
        for i in inside {
            texts[i].used = true;
        }
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Radio) {
        for &a in &c.atoms {
            let r = fe.atom(a).layout_rect();
            c.options
                .push(right_label(&mut texts, r).unwrap_or_default());
        }
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Toggle) {
        c.label = right_label(&mut texts, c.r)
            .or_else(|| above_label(&mut texts, c.r, 16, 12, 12))
            .unwrap_or_default();
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Radio) {
        c.label = above_label(&mut texts, c.r, 36, 32, 40).unwrap_or_default();
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Slider) {
        c.label = above_label(&mut texts, c.r, 26, 24, 24).unwrap_or_default();
        let below: Vec<usize> = (0..texts.len())
            .filter(|&i| {
                let t = &texts[i];
                !t.used
                    && t.r.y0 >= c.r.y1 - 4
                    && t.r.y0 - c.r.y1 <= 12
                    && t.r.x0 >= c.r.x0 - 14
                    && t.r.x0 <= c.r.x1 + 4
            })
            .collect();
        if let (Some(&l), Some(&h)) = (
            below.iter().min_by_key(|&&i| texts[i].r.x0),
            below.iter().max_by_key(|&&i| texts[i].r.x0),
        ) {
            if l != h {
                c.lo = texts[l].text.clone();
                c.hi = texts[h].text.clone();
            }
        }
        for i in below {
            texts[i].used = true;
        }
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::VSlider) {
        if let Some(i) = (0..texts.len()).find(|&i| {
            let t = &texts[i];
            !t.used
                && t.r.y0 >= c.r.y1 - 8
                && t.r.y0 <= c.r.y1 + 12
                && ((t.r.x0 + t.r.x1) / 2 - (c.r.x0 + c.r.x1) / 2).abs() <= 14
        }) {
            texts[i].used = true;
            c.label = texts[i].text.clone();
        }
    }
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::List) {
        c.label = above_label(&mut texts, c.r, 22, 20, 20).unwrap_or_default();
    }
    // A figure drawn by a user region on a slider's heading line goes with the slider.
    let users: Vec<Ctl> = ctls
        .iter()
        .filter(|c| c.tag == Tag::User)
        .cloned()
        .collect();
    let mut taken = Vec::new();
    for u in &users {
        let name = fe.atom(u.atoms[0]).name.clone().unwrap_or_default();
        if let Some(c) = ctls.iter_mut().find(|c| {
            c.tag == Tag::Slider && (c.r.y0 - u.r.y1).abs() <= 14 && u.r.x0 >= c.r.x0 - 10
        }) {
            c.value = user_value(fe, &name);
            taken.push(u.atoms[0]);
        }
    }
    ctls.retain(|c| !(c.tag == Tag::User && taken.contains(&c.atoms[0])));
    // A check box on a slider's heading line (the channels' AUTO) says what it is for.
    let sliders: Vec<(Rect, String)> = ctls
        .iter()
        .filter(|c| c.tag == Tag::Slider)
        .map(|c| (c.r, c.label.clone()))
        .collect();
    for c in ctls.iter_mut().filter(|c| c.tag == Tag::Toggle) {
        if let Some((r, l)) = sliders.iter().find(|(r, l)| {
            !l.is_empty() && (c.r.y1 - r.y0).abs() <= 12 && c.r.x0 > r.x0 && c.label.len() <= 6
        }) {
            c.label = format!("{} {}", l.trim_end_matches(':'), c.label);
            // Listed right after its slider.
            c.r = Rect::new(r.x0, r.y0 + 1, r.x0 + 1, r.y0 + 2);
        }
    }

    let menu = ctls.iter().all(|c| c.tag == Tag::Button);
    let remaining: Vec<Txt> = texts.iter().filter(|t| !t.used).cloned().collect();
    let mut messages = Vec::new();
    let mut headings: Vec<Txt> = Vec::new();
    let mut paragraphs: Vec<String> = Vec::new();
    if menu && remaining.len() >= 4 && ctls.len() <= 2 {
        // A page of text (the texture memory explanation).
        let mut lines = remaining.clone();
        lines.sort_by_key(|t| (t.r.y0, t.r.x0));
        let mut last_y = i32::MIN;
        for t in lines {
            if paragraphs.is_empty() || t.r.y0 - last_y > 20 {
                paragraphs.push(t.text.clone());
            } else if let Some(p) = paragraphs.last_mut() {
                p.push(' ');
                p.push_str(&t.text);
            }
            last_y = t.r.y0;
        }
    } else if menu {
        let mut lines = remaining.clone();
        lines.sort_by_key(|t| (t.r.y0, t.r.x0));
        messages = lines.into_iter().map(|t| t.text).collect();
    } else {
        headings = remaining
            .into_iter()
            .filter(|t| t.text.chars().filter(|c| c.is_alphanumeric()).count() >= 4)
            .collect();
        headings.sort_by_key(|t| (t.r.y0, t.r.x0));
    }

    // The controls as items.
    let lang = fe.resources().language().index();
    let mut items: Vec<Item> = Vec::new();
    for c in &ctls {
        let a = c.atoms[0];
        let atom = fe.atom(a);
        let disabled = c
            .atoms
            .iter()
            .all(|&a| region_has(fe, a, RSF_REGION_DISABLED));
        let focused = c
            .atoms
            .iter()
            .any(|&a| region_has(fe, a, RSF_CURRENT_SELECTED));
        let kind = match c.tag {
            Tag::Button => {
                // A default button that also goes back (the advanced speech screen's
                // ACCEPT!, a link back) is the main one when another button is the way back.
                let pure_back = ctls.iter().any(|o| {
                    o.tag == Tag::Button && o.atoms[0] != a && {
                        let oa = fe.atom(o.atoms[0]);
                        oa.has(FAF_DEFAULT_BACK) && !oa.has(FAF_DEFAULT_OK)
                    }
                });
                let ok = atom.has(FAF_DEFAULT_OK);
                let back = atom.has(FAF_DEFAULT_BACK);
                let primary = ok && (!back || pure_back);
                Kind::Button {
                    primary,
                    back: back && !primary,
                }
            }
            Tag::Toggle => Kind::Toggle { on: atom.checked() },
            Tag::Radio => Kind::Choice {
                labels: c.options.clone(),
                sources: c.atoms.iter().map(|&a| Source::Atom(a)).collect(),
                current: c.atoms.iter().position(|&a| fe.atom(a).checked()),
            },
            Tag::Slider | Tag::VSlider => {
                let (v, max) = fe.slider(a).map_or((0, 100), |s| (s.value, s.max.max(1)));
                let fraction = (v as f32 / max as f32).clamp(0.0, 1.0);
                if c.tag == Tag::VSlider {
                    Kind::VSlider { fraction }
                } else {
                    Kind::Slider {
                        fraction,
                        readout: c
                            .value
                            .clone()
                            .unwrap_or_else(|| format!("{}%", (fraction * 100.0).round() as i32)),
                        lo: c.lo.clone(),
                        hi: c.hi.clone(),
                    }
                }
            }
            Tag::List => {
                let keys = atom.name.as_deref() == Some("OP_KeyBindingsList");
                let (rows, top, total, page) = match fe.list(a) {
                    Some(l) => {
                        let top = l.upper.min(l.items.len());
                        let rows = l.items[top..]
                            .iter()
                            .take(l.max_visible)
                            .map(|it| Row {
                                text: it.text.clone(),
                                selectable: it.can_select,
                                selected: it.selected,
                                keys: keys.then(|| key_row(fe, it.data)),
                            })
                            .collect();
                        (rows, top, l.items.len(), l.max_visible)
                    }
                    None => (Vec::new(), 0, 0, 0),
                };
                Kind::List {
                    rows,
                    top,
                    total,
                    page,
                }
            }
            Tag::Entry => Kind::Entry {
                text: fe.text_entry(a).unwrap_or("").to_string(),
                focused: region_has(fe, a, RSF_KEY_CAPTURE),
            },
            Tag::User => {
                let name = atom.name.clone().unwrap_or_default();
                let v = user_value(fe, &name).unwrap_or_default();
                if name == "PO_TextDraw" {
                    Kind::Text {
                        paragraphs: v
                            .split(['\n', '\r'])
                            .map(str::trim)
                            .filter(|p| !p.is_empty())
                            .map(str::to_string)
                            .collect(),
                    }
                } else {
                    Kind::Value { text: v }
                }
            }
        };
        let mut it = Item::new(kind, c.label.clone(), Source::Atom(a), c.r);
        if c.tag == Tag::Radio {
            it.source = Source::Nothing;
        }
        it.key = hot_key_name(atom.hotkeys[lang]);
        if c.tag != Tag::Button {
            it.key.clear();
        }
        it.disabled = disabled;
        it.focused = focused;
        items.push(it);
    }
    if !paragraphs.is_empty() {
        let at = Rect::new(0, 60, 640, 61);
        items.push(Item::new(
            Kind::Text { paragraphs },
            String::new(),
            Source::Nothing,
            at,
        ));
    }

    // The footer: the bottom row of buttons, below everything else (forms only).
    let style = if items.iter().all(|i| matches!(i.kind, Kind::Button { .. })) {
        Style::Menu
    } else {
        Style::Form
    };
    if style == Style::Form {
        let others = items
            .iter()
            .filter(|i| !matches!(i.kind, Kind::Button { .. }))
            .map(|i| i.at.y1)
            .max()
            .unwrap_or(0);
        let lowest = items
            .iter()
            .filter(|i| matches!(i.kind, Kind::Button { .. }))
            .map(|i| i.at.y0)
            .max();
        if let Some(lowest) = lowest {
            for it in items.iter_mut() {
                if matches!(it.kind, Kind::Button { .. })
                    && it.at.y0 >= lowest - 10
                    && it.at.y0 >= others - 4
                {
                    it.footer = true;
                }
            }
        }
    }

    // Sections: each control goes under the nearest heading above it in its column.
    let mut sections = vec![String::new()];
    sections.extend(headings.iter().map(|h| h.text.clone()));
    for it in items.iter_mut().filter(|i| !i.footer) {
        let best = headings
            .iter()
            .enumerate()
            .filter(|(_, h)| h.r.y1 <= it.at.y0 + 4)
            .filter(|(_, h)| {
                let (left, right) = heading_span(&all_texts, h.r);
                (left - 20..right).contains(&it.at.x0)
            })
            .map(|(k, h)| {
                let dy = (it.at.y0 - h.r.y1) as f32;
                let dx0 = (it.at.x0 - h.r.x0).abs() as f32;
                (k, dy + dx0)
            })
            .min_by(|a, b| a.1.total_cmp(&b.1));
        it.section = best.map_or(0, |(k, _)| k + 1);
    }

    // The order: by section, then by the original's columns, top to bottom.
    let mut order: Vec<usize> = (0..items.len()).collect();
    let columns = column_of(&items);
    if style == Style::Menu {
        order.sort_by_key(|&i| (items[i].at.y0, items[i].at.x0));
    } else {
        order.sort_by_key(|&i| {
            let it = &items[i];
            (it.footer, it.section, columns[i], it.at.y0, it.at.x0)
        });
    }
    let mut items: Vec<Item> = order.into_iter().map(|i| items[i].clone()).collect();
    // The footer: the default button first, the back button on the right.
    let footer_start = items.iter().position(|i| i.footer).unwrap_or(items.len());
    items[footer_start..].sort_by_key(|i| match i.kind {
        Kind::Button { back: true, .. } => (2, i.at.x0),
        Kind::Button { primary: true, .. } => (0, i.at.x0),
        _ => (1, i.at.x0),
    });
    // The hub leads to the modern options too, before its back button.
    if s.name == HUB {
        let back = items
            .iter()
            .position(|i| matches!(i.kind, Kind::Button { back: true, .. }))
            .unwrap_or(items.len());
        let at = items.get(back).map_or(Rect::new(0, 470, 1, 471), |i| i.at);
        let mut e = Item::new(
            Kind::Button {
                primary: false,
                back: false,
            },
            mo::HUB_LABEL.to_string(),
            Source::Extra(Extra::Enhancements),
            at,
        );
        e.key = mo::HUB_KEY.to_string();
        items.insert(back, e);
    }
    Model {
        name: s.name.clone(),
        style,
        title,
        subtitle,
        messages,
        sections,
        items,
    }
}

/// The columns a heading spans on the original's layout: from the end of the text before
/// it on its line to the start of the text after it (the whole width when it is alone).
fn heading_span(texts: &[Rect], h: Rect) -> (i32, i32) {
    let line = texts
        .iter()
        .filter(|t| (t.y0 - h.y0).abs() <= 6 && **t != h);
    let mut left = 0;
    let mut right = 640;
    for t in line {
        if t.x1 <= h.x0 + 4 {
            left = left.max(t.x1);
        } else if t.x0 >= h.x1 - 4 {
            right = right.min(t.x0);
        }
    }
    (left, right)
}

/// The original's column of each item (its left edge, grouped where they are close).
fn column_of(items: &[Item]) -> Vec<usize> {
    let mut xs: Vec<i32> = items.iter().map(|i| i.at.x0).collect();
    xs.sort_unstable();
    let mut starts = Vec::new();
    let mut last = i32::MIN;
    for x in xs {
        if x.saturating_sub(last) > 70 || starts.is_empty() {
            starts.push(x);
        }
        last = x;
    }
    items
        .iter()
        .map(|i| starts.iter().rposition(|&s| s <= i.at.x0).unwrap_or(0))
        .collect()
}

// ---- layout (at 1080 lines) ----

const MENU_W: f32 = 460.0;
/// A confirmation (two buttons side by side) or a notice (one).
const CONFIRM_W: f32 = 520.0;
const FORM_W1: f32 = 700.0;
const FORM_W2: f32 = 1000.0;
const LIST_FORM_W: f32 = 820.0;
const BODY_Y: f32 = HEADER_H + 16.0;
const ROW_H: f32 = 40.0;
const SLIDER_H: f32 = 56.0;
const HEADING_H: f32 = 42.0;
const BUTTON_ROW_H: f32 = 52.0;
const ENTRY_H: f32 = 52.0;
const LIST_ROW_H: f32 = 25.0;
const LIST_HEAD_H: f32 = 34.0;
const VBANK_H: f32 = 250.0;
const TEXT_LINE: f32 = 22.0;
const COL_GAP: f32 = 48.0;
const MENU_BUTTON_H: f32 = 46.0;
const MENU_GAP: f32 = 10.0;
const MESSAGE_H: f32 = 28.0;
/// A form taller than this goes into two columns.
const TWO_COLUMNS_ABOVE: f32 = 540.0;
/// The space under the body: the footer's line and buttons.
const FOOTER_SPACE: f32 = REF_H - FOOTER_LINE + 8.0;
/// The widest control column of a row (switches, sliders, segments).
const CONTROL_W: f32 = 320.0;
const SWITCH_W: f32 = 46.0;
const SWITCH_H: f32 = 24.0;
const READOUT_W: f32 = 54.0;

/// Where an item is drawn.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Placed {
    /// The whole row.
    pub row: Rect,
    /// The control in it (the switch, the slider's track, the segments, the box...).
    pub control: Rect,
    /// Segments; a list's rows (a page of them).
    pub parts: Vec<Rect>,
    /// A list's scroll track.
    pub track: Option<Rect>,
    /// A radio group drawn as a drop-down.
    pub dropdown: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub frame: Frame,
    /// Per item of the model.
    pub placed: Vec<Placed>,
    /// Section headings: (section, its row).
    pub headings: Vec<(usize, Rect)>,
    /// A confirmation's lines.
    pub messages: Vec<Rect>,
}

impl Layout {
    fn px(&self, v: f32) -> i32 {
        self.frame.px(v)
    }

    /// A drop-down's choices when open: under its box, or over it when there is no room.
    pub fn dropdown_rows(&self, i: usize, n: usize, canvas_h: i32) -> Vec<Rect> {
        let Some(p) = self.placed.get(i) else {
            return Vec::new();
        };
        let c = p.control;
        let h = c.height().max(1);
        let gap = self.px(4.0);
        let down = c.y1 + gap + h * n as i32 <= canvas_h - self.px(6.0);
        (0..n as i32)
            .map(|k| {
                let y = if down {
                    c.y1 + gap + k * h
                } else {
                    c.y0 - gap - (n as i32 - k) * h
                };
                Rect::new(c.x0, y, c.x1, y + h)
            })
            .collect()
    }

    /// A list's thumb, when it has more rows than its page.
    pub fn thumb(&self, i: usize, total: usize, page: usize, top: usize) -> Option<Rect> {
        let track = self.placed.get(i)?.track?;
        mm::thumb(track, page, total, top, self.px(24.0))
    }
}

/// A line of the form: a heading, an item, buttons side by side, a bank of vertical
/// sliders.
#[derive(Debug, Clone, PartialEq)]
enum Line {
    Heading(usize),
    One(usize),
    Side(Vec<usize>),
    Bank(Vec<usize>),
}

fn text_height(paragraphs: &[String], width: f32) -> f32 {
    // An estimate: Segoe UI's average letter is about half its size wide, capitals more.
    let letters: usize = paragraphs
        .iter()
        .map(|p| p.chars().filter(|c| c.is_alphabetic()).count())
        .sum();
    let caps: usize = paragraphs
        .iter()
        .map(|p| p.chars().filter(|c| c.is_uppercase()).count())
        .sum();
    let share = caps as f32 / letters.max(1) as f32;
    let per_line = (width / ((BODY_PX + 1.0) * (0.56 + 0.16 * share))).max(10.0);
    let lines: f32 = paragraphs
        .iter()
        .map(|p| (p.chars().count() as f32 / per_line).ceil().max(1.0))
        .sum();
    (lines * TEXT_LINE + paragraphs.len() as f32 * 8.0 + 16.0).clamp(90.0, 420.0)
}

fn line_height(m: &Model, l: &Line, col_w: f32) -> f32 {
    match l {
        Line::Heading(_) => HEADING_H,
        Line::Side(_) => BUTTON_ROW_H,
        Line::Bank(_) => VBANK_H,
        Line::One(i) => match &m.items[*i].kind {
            Kind::Button { .. } => BUTTON_ROW_H,
            Kind::Slider { .. } => SLIDER_H,
            Kind::Entry { .. } => ENTRY_H,
            Kind::List { page, .. } => {
                let head = list_head(&m.items[*i]);
                head + *page.max(&3) as f32 * LIST_ROW_H + 2.0 * 6.0 + 8.0
            }
            Kind::Text { paragraphs } => text_height(paragraphs, col_w - 28.0),
            Kind::VSlider { .. } => VBANK_H,
            _ => {
                ROW_H
                    + if m.items[*i].note.is_some() {
                        10.0
                    } else {
                        0.0
                    }
            }
        },
    }
}

/// The band over a list: its heading, or the key list's column names.
fn list_head(it: &Item) -> f32 {
    let keys = matches!(&it.kind, Kind::List { rows, .. } if rows.iter().any(|r| r.keys.is_some()));
    if it.label.is_empty() && !keys {
        10.0
    } else {
        LIST_HEAD_H
    }
}

/// Lays a modern screen out on a `width` x `height` canvas.
pub fn layout(m: &Model, width: i32, height: i32) -> Layout {
    match m.style {
        Style::Menu => menu_layout(m, width, height),
        Style::Form => form_layout(m, width, height),
    }
}

fn is_confirm(m: &Model) -> bool {
    m.items.len() == 2 && (!m.messages.is_empty() || m.title.ends_with('?'))
}

fn menu_layout(m: &Model, width: i32, height: i32) -> Layout {
    let mut y = BODY_Y + 8.0;
    let mut msg_rows = Vec::new();
    for _ in &m.messages {
        msg_rows.push(y);
        y += MESSAGE_H;
    }
    if !m.messages.is_empty() {
        y += 14.0;
    }
    let menu_w = if is_confirm(m) || m.items.len() <= 1 {
        CONFIRM_W
    } else {
        MENU_W
    };
    let inner = menu_w - 2.0 * PAD;
    let mut rows: Vec<(f32, f32, f32)> = Vec::new();
    if is_confirm(m) {
        let w = (inner - BUTTON_GAP) / 2.0;
        rows.push((PAD, y, w));
        rows.push((PAD + w + BUTTON_GAP, y, w));
        y += MENU_BUTTON_H + MENU_GAP;
    } else {
        for (k, it) in m.items.iter().enumerate() {
            if k > 0 && matches!(it.kind, Kind::Button { back: true, .. }) {
                y += 14.0;
            }
            rows.push((PAD, y, inner));
            y += MENU_BUTTON_H + MENU_GAP;
        }
    }
    let ref_h = y - MENU_GAP + PAD;
    let frame = Frame::sized(width, height, menu_w, ref_h, 0, false);
    let placed = rows
        .iter()
        .map(|&(x, y, w)| {
            let r = frame.at(x, y, w, MENU_BUTTON_H);
            Placed {
                row: r,
                control: r,
                ..Default::default()
            }
        })
        .collect();
    let messages = msg_rows
        .iter()
        .map(|&y| frame.at(PAD, y, inner, MESSAGE_H))
        .collect();
    Layout {
        frame,
        placed,
        headings: Vec::new(),
        messages,
    }
}

/// The form's lines, section by section.
fn form_lines(m: &Model) -> Vec<Vec<Line>> {
    let mut out: Vec<Vec<Line>> = Vec::new();
    let body: Vec<usize> = (0..m.items.len()).filter(|&i| !m.items[i].footer).collect();
    let mut k = 0;
    while k < body.len() {
        let i = body[k];
        let section = m.items[i].section;
        let mut lines = Vec::new();
        if !m.sections[section].is_empty() {
            lines.push(Line::Heading(section));
        }
        while k < body.len() && m.items[body[k]].section == section {
            let i = body[k];
            match m.items[i].kind {
                Kind::Button { .. } => {
                    let mut side = vec![i];
                    while k + 1 < body.len() {
                        let j = body[k + 1];
                        if m.items[j].section == section
                            && matches!(m.items[j].kind, Kind::Button { .. })
                            && (m.items[j].at.y0 - m.items[i].at.y0).abs() <= 8
                            && side.len() < 3
                        {
                            side.push(j);
                            k += 1;
                        } else {
                            break;
                        }
                    }
                    lines.push(if side.len() > 1 {
                        Line::Side(side)
                    } else {
                        Line::One(i)
                    });
                }
                Kind::VSlider { .. } => {
                    let mut bank = vec![i];
                    while k + 1 < body.len()
                        && matches!(m.items[body[k + 1]].kind, Kind::VSlider { .. })
                    {
                        bank.push(body[k + 1]);
                        k += 1;
                    }
                    lines.push(Line::Bank(bank));
                }
                _ => lines.push(Line::One(i)),
            }
            k += 1;
        }
        out.push(lines);
    }
    out
}

/// Whether a radio group's choices fit as segments in `w` reference pixels.
pub fn segments_fit(labels: &[String], w: f32) -> bool {
    if labels.is_empty() || labels.len() > 4 {
        return false;
    }
    let each = w / labels.len() as f32;
    labels
        .iter()
        .all(|l| l.chars().count() as f32 * (BUTTON_PX * 0.72) + 20.0 <= each)
}

fn form_layout(m: &Model, width: i32, height: i32) -> Layout {
    let sections = form_lines(m);
    let wide = m
        .items
        .iter()
        .any(|i| matches!(i.kind, Kind::List { .. } | Kind::Text { .. }));
    let one_col_w = if wide { LIST_FORM_W } else { FORM_W1 };
    let heights = |w: f32| -> Vec<f32> {
        sections
            .iter()
            .map(|s| s.iter().map(|l| line_height(m, l, w)).sum())
            .collect()
    };
    let total: f32 = heights(one_col_w - 2.0 * PAD).iter().sum();
    let cols = if !wide && total > TWO_COLUMNS_ABOVE && sections.len() > 1 {
        2
    } else {
        1
    };
    let ref_w = if cols == 2 { FORM_W2 } else { one_col_w };
    let col_w = (ref_w - 2.0 * PAD - (cols as f32 - 1.0) * COL_GAP) / cols as f32;
    let hs = heights(col_w);
    // Two columns: the split of the sections (in order) with the shorter tallest column.
    let split = if cols == 2 {
        (1..sections.len())
            .min_by(|&a, &b| {
                let h = |k: usize| {
                    let l: f32 = hs[..k].iter().sum();
                    let r: f32 = hs[k..].iter().sum();
                    l.max(r)
                };
                h(a).total_cmp(&h(b))
            })
            .unwrap_or(sections.len())
    } else {
        sections.len()
    };
    let col_h = |range: std::ops::Range<usize>| hs[range].iter().sum::<f32>();
    let body_h = col_h(0..split).max(col_h(split..sections.len()));
    let has_footer = m.items.iter().any(|i| i.footer);
    let ref_h = (BODY_Y + body_h + if has_footer { FOOTER_SPACE } else { PAD }).max(260.0);
    let frame = Frame::sized(width, height, ref_w, ref_h, 0, has_footer);
    let mut placed = vec![Placed::default(); m.items.len()];
    let mut headings = Vec::new();
    for (k, lines) in sections.iter().enumerate() {
        let col = usize::from(k >= split);
        let x = PAD + col as f32 * (col_w + COL_GAP);
        let start = if col == 0 { 0 } else { split };
        let mut y = BODY_Y + col_h(start..k);
        for line in lines {
            let h = line_height(m, line, col_w);
            let row = frame.at(x, y, col_w, h);
            match line {
                Line::Heading(s) => headings.push((*s, row)),
                Line::One(i) => placed[*i] = place(m, &frame, *i, x, y, col_w, h),
                Line::Side(v) => {
                    let gap = BUTTON_GAP;
                    let w = (col_w - gap * (v.len() as f32 - 1.0)) / v.len() as f32;
                    for (n, &i) in v.iter().enumerate() {
                        let bx = x + n as f32 * (w + gap);
                        let b = frame.at(bx, y + 6.0, w, h - 12.0);
                        placed[i] = Placed {
                            row: b,
                            control: b,
                            ..Default::default()
                        };
                    }
                }
                Line::Bank(v) => {
                    let w = (col_w / v.len().max(8) as f32).min(72.0);
                    let x0 = x + (col_w - w * v.len() as f32) / 2.0;
                    for (n, &i) in v.iter().enumerate() {
                        let cx = x0 + n as f32 * w;
                        placed[i] = Placed {
                            row: frame.at(cx, y, w, h),
                            control: frame.at(cx + w / 2.0 - 10.0, y + 18.0, 20.0, h - 64.0),
                            ..Default::default()
                        };
                    }
                }
            }
            y += h;
        }
    }
    // The footer: buttons from the left, the back button on the right.
    let footer: Vec<usize> = (0..m.items.len()).filter(|&i| m.items[i].footer).collect();
    let mut left = 0;
    for &i in &footer {
        let r = if matches!(m.items[i].kind, Kind::Button { back: true, .. }) {
            frame.footer_right()
        } else {
            left += 1;
            frame.footer_button(left - 1)
        };
        placed[i] = Placed {
            row: r,
            control: r,
            ..Default::default()
        };
    }
    Layout {
        frame,
        placed,
        headings,
        messages: Vec::new(),
    }
}

/// One item's row at (`x`, `y`) in reference pixels, `w` x `h`.
fn place(m: &Model, f: &Frame, i: usize, x: f32, y: f32, w: f32, h: f32) -> Placed {
    let it = &m.items[i];
    let row = f.at(x, y, w, h);
    let cw = CONTROL_W.min(w * 0.52);
    let cx = x + w - cw - 8.0;
    let mid = y + h / 2.0;
    let full = it.label.is_empty();
    let (control_x, control_w) = if full { (x + 8.0, w - 16.0) } else { (cx, cw) };
    match &it.kind {
        Kind::Button { .. } => {
            let b = f.at(x, y + 6.0, w, h - 12.0);
            Placed {
                row: b,
                control: b,
                ..Default::default()
            }
        }
        Kind::Toggle { .. } => Placed {
            row,
            control: f.at(
                x + w - SWITCH_W - 10.0,
                mid - SWITCH_H / 2.0,
                SWITCH_W,
                SWITCH_H,
            ),
            ..Default::default()
        },
        Kind::Choice { labels, .. } => {
            let control = f.at(control_x, mid - 15.0, control_w, 30.0);
            let dropdown = !segments_fit(labels, control_w);
            let parts = if dropdown {
                Vec::new()
            } else {
                segment_rects(control, labels.len())
            };
            Placed {
                row,
                control,
                parts,
                dropdown,
                ..Default::default()
            }
        }
        Kind::Slider { .. } => Placed {
            row,
            control: f.at(
                control_x + 8.0,
                mid - 9.0,
                control_w - READOUT_W - 16.0,
                18.0,
            ),
            ..Default::default()
        },
        Kind::Entry { .. } => {
            let b = f.at(x + 4.0, y + 6.0, w - 8.0, h - 12.0);
            Placed {
                row,
                control: b,
                ..Default::default()
            }
        }
        Kind::List { page, .. } => {
            let head = list_head(it);
            let well = f.at(
                x,
                y + head,
                w - 14.0,
                *page.max(&3) as f32 * LIST_ROW_H + 12.0,
            );
            let rows = (0..*page)
                .map(|k| {
                    f.at(
                        x + 4.0,
                        y + head + 6.0 + k as f32 * LIST_ROW_H,
                        w - 22.0,
                        LIST_ROW_H,
                    )
                })
                .collect();
            let track = Rect::new(
                well.x1 + f.px(6.0),
                well.y0 + f.px(4.0),
                well.x1 + f.px(9.0).max(f.px(6.0) + 2),
                well.y1 - f.px(4.0),
            );
            Placed {
                row,
                control: well,
                parts: rows,
                track: Some(track),
                ..Default::default()
            }
        }
        Kind::Text { .. } => Placed {
            row,
            control: f.at(x, y + 4.0, w, h - 8.0),
            ..Default::default()
        },
        Kind::VSlider { .. } => Placed {
            row,
            control: f.at(x + w / 2.0 - 10.0, y + 18.0, 20.0, h - 64.0),
            ..Default::default()
        },
        Kind::Value { .. } => Placed {
            row,
            control: f.at(cx, y, cw, h),
            ..Default::default()
        },
    }
}

// ---- hit-testing ----

/// What is under the pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Hit {
    /// An item's control (a toggle's whole row).
    Item(usize),
    /// A segment of a choice, or a row of a list (on the page).
    Part(usize, usize),
    /// A choice of an open drop-down.
    Choice(usize, usize),
    Thumb(usize),
    /// A list's scroll track above (true) or below its thumb.
    Track(usize, bool),
    Panel,
    Outside,
}

/// A slider's grip zone: its track and a knob's width either side, its row's height.
fn slider_zone(l: &Layout, p: &Placed) -> Rect {
    let k = l.px(10.0);
    Rect::new(p.control.x0 - k, p.row.y0, p.control.x1 + k, p.row.y1)
}

/// What is at (`x`, `y`), with the drop-down `open` (an item) open.
pub fn hit(m: &Model, l: &Layout, open: Option<usize>, canvas_h: i32, x: i32, y: i32) -> Hit {
    if let Some(i) = open {
        if let Some(Kind::Choice { labels, .. }) = m.items.get(i).map(|it| &it.kind) {
            for (k, r) in l
                .dropdown_rows(i, labels.len(), canvas_h)
                .iter()
                .enumerate()
            {
                if r.contains(x, y) {
                    return Hit::Choice(i, k);
                }
            }
        }
    }
    for (i, (it, p)) in m.items.iter().zip(&l.placed).enumerate() {
        match &it.kind {
            Kind::Button { .. } | Kind::Entry { .. } if p.control.contains(x, y) => {
                return Hit::Item(i)
            }
            Kind::Toggle { .. } if p.row.contains(x, y) => return Hit::Item(i),
            Kind::Slider { .. } if slider_zone(l, p).contains(x, y) => return Hit::Item(i),
            Kind::VSlider { .. } if p.row.contains(x, y) => return Hit::Item(i),
            Kind::Choice { .. } => {
                if p.dropdown {
                    if p.control.contains(x, y) {
                        return Hit::Item(i);
                    }
                } else if let Some(k) = p.parts.iter().position(|r| r.contains(x, y)) {
                    return Hit::Part(i, k);
                }
            }
            Kind::List {
                rows,
                top,
                total,
                page,
            } => {
                if let Some(k) = p.parts.iter().position(|r| r.contains(x, y)) {
                    if k < rows.len() {
                        return Hit::Part(i, k);
                    }
                    return Hit::Panel;
                }
                if let Some(track) = p.track {
                    let grip = Rect::new(
                        track.x0 - l.px(6.0),
                        track.y0,
                        track.x1 + l.px(6.0),
                        track.y1,
                    );
                    if grip.contains(x, y) {
                        if let Some(th) = l.thumb(i, *total, *page, *top) {
                            if (th.y0..th.y1).contains(&y) {
                                return Hit::Thumb(i);
                            }
                            return Hit::Track(i, y < th.y0);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if l.frame.panel.contains(x, y) {
        Hit::Panel
    } else {
        Hit::Outside
    }
}

/// The original region a hit stands for: the mouse is handed on through it.
pub fn target(fe: &Fe, m: &Model, l: &Layout, at: Hit) -> Option<Target> {
    let canvas_h = fe.screen_size().1;
    match at {
        Hit::Item(i) => {
            let it = m.items.get(i)?;
            let Source::Atom(a) = it.source else {
                return None;
            };
            let p = &l.placed[i];
            let o = region_rect(fe, a)?;
            Some(match it.kind {
                Kind::Toggle { .. } => Target::new(p.row, o),
                Kind::Slider { .. } => {
                    Target::clamped(Rect::new(p.control.x0, p.row.y0, p.control.x1, p.row.y1), o)
                }
                Kind::VSlider { .. } => {
                    Target::clamped(Rect::new(p.row.x0, p.control.y0, p.row.x1, p.control.y1), o)
                }
                _ => Target::new(p.control, o),
            })
        }
        Hit::Part(i, k) => {
            let it = m.items.get(i)?;
            let p = &l.placed[i];
            match &it.kind {
                Kind::Choice { sources, .. } => {
                    let Source::Atom(a) = *sources.get(k)? else {
                        return None;
                    };
                    Some(Target::new(p.parts[k], region_rect(fe, a)?))
                }
                Kind::List { .. } => {
                    let Source::Atom(a) = it.source else {
                        return None;
                    };
                    let o = region_rect(fe, a)?;
                    let lw = fe.list(a)?;
                    let y0 = o.y0 + lw.title_height + LIST_MARGIN + k as i32 * lw.item_height;
                    let row = Rect::new(
                        o.x0 + LIST_MARGIN,
                        y0,
                        o.x1 - LIST_MARGIN,
                        y0 + lw.item_height,
                    );
                    Some(Target::new(p.parts[k], row))
                }
                _ => None,
            }
        }
        Hit::Choice(i, k) => {
            let Kind::Choice {
                sources, labels, ..
            } = &m.items.get(i)?.kind
            else {
                return None;
            };
            let Source::Atom(a) = *sources.get(k)? else {
                return None;
            };
            let rows = l.dropdown_rows(i, labels.len(), canvas_h);
            Some(Target::new(rows[k], region_rect(fe, a)?))
        }
        _ => None,
    }
}

// ---- state and input ----

/// What is highlighted (eased).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Key {
    Hit(Hit),
    /// A switch's knob position.
    On(usize),
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub mouse: (i32, i32),
    /// The screen the highlights belong to.
    screen: String,
    hover: Hover<Key>,
    /// The control pressed (its look; a local control acts on its release over it).
    pressed: Option<Hit>,
    pub proxy: Proxy,
    /// The open drop-down (an item).
    pub dropdown: Option<usize>,
    /// Dragging a list's thumb: (item, the pointer's offset from its top).
    drag: Option<(usize, i32)>,
    /// Dragging one of the modern options' sliders (an item).
    slide: Option<usize>,
    /// The modern options, over the options hub.
    pub page: Option<mo::Page>,
    /// Escape left a text entry: it goes on to the screen's back button next frame.
    escape_again: bool,
}

/// Opening the escape menu or the objectives: a fresh state.
pub fn reset(fe: &mut Fe) {
    let mouse = fe.game.ig.mmenu.mouse;
    fe.game.ig.mmenu = State {
        mouse,
        ..State::default()
    };
}

fn current(fe: &Fe) -> Option<(Model, Layout)> {
    let m = model(fe)?;
    let (w, h) = fe.screen_size();
    let l = layout(&m, w, h);
    Some((m, l))
}

fn hit_now(fe: &Fe, m: &Model, l: &Layout) -> Hit {
    let (x, y) = fe.game.ig.mmenu.mouse;
    hit(m, l, fe.game.ig.mmenu.dropdown, fe.screen_size().1, x, y)
}

/// The original region under the pointer, if any (nothing while the modern options are up).
fn under(fe: &Fe, m: &Model, l: &Layout, at: Hit) -> Option<Target> {
    if fe.game.ig.mmenu.page.is_some() {
        return None;
    }
    target(fe, m, l, at)
}

/// The pointer moved.
pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    fe.game.ig.mmenu.mouse = (x, y);
    let Some((m, l)) = current(fe) else { return };
    if let Some((i, grab)) = fe.game.ig.mmenu.drag {
        if let Some(Kind::List { total, page, .. }) = m.items.get(i).map(|it| &it.kind) {
            if let Some(track) = l.placed[i].track {
                let top = mm::top_for_thumb(track, *page, *total, y - grab, l.px(24.0));
                scroll_list(fe, &m, i, ScrollTo::Row(top));
            }
        }
    }
    if let Some(i) = fe.game.ig.mmenu.slide {
        mo::slide(fe, &m, &l, i, x);
    }
    let at = hit_now(fe, &m, &l);
    let t = under(fe, &m, &l, at);
    let mut proxy = fe.game.ig.mmenu.proxy;
    proxy.moved(fe, x, y, t);
    fe.game.ig.mmenu.proxy = proxy;
}

enum ScrollTo {
    Row(usize),
    By(isize),
    Wheel(bool),
}

/// Scrolls the original's list window (as its scroll bar and wheel do).
fn scroll_list(fe: &mut Fe, m: &Model, i: usize, to: ScrollTo) {
    let Some(Source::Atom(a)) = m.items.get(i).map(|it| it.source) else {
        return;
    };
    let Some(l) = fe.list_mut(a) else { return };
    let last = l.items.len().saturating_sub(l.max_visible);
    match to {
        ScrollTo::Row(r) => l.upper = r.min(last),
        ScrollTo::By(d) => l.upper = (l.upper as isize + d).clamp(0, last as isize) as usize,
        ScrollTo::Wheel(true) => l.wheel_up(),
        ScrollTo::Wheel(false) => l.wheel_down(),
    }
    if let Some(id) = fe.atom(a).region {
        fe.list_scroll_adjust(id);
    }
}

/// Whether a hit is one of the modern-only controls (handled here, not by the original).
fn local(m: &Model, l: &Layout, at: Hit) -> bool {
    match at {
        Hit::Item(i) => {
            let it = &m.items[i];
            matches!(it.source, Source::Extra(_) | Source::Option(_))
                || (matches!(it.kind, Kind::Choice { .. }) && l.placed[i].dropdown)
        }
        Hit::Part(i, _) | Hit::Choice(i, _) => {
            matches!(&m.items[i].kind, Kind::Choice { sources, .. } if sources.iter().any(|s| matches!(s, Source::Option(_))))
        }
        Hit::Thumb(_) | Hit::Track(..) => true,
        _ => false,
    }
}

/// A mouse button over the screen.
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool) {
    let Some((m, l)) = current(fe) else { return };
    let at = hit_now(fe, &m, &l);
    let pos = fe.game.ig.mmenu.mouse;
    if !left {
        let t = under(fe, &m, &l, at);
        let mut proxy = fe.game.ig.mmenu.proxy;
        proxy.button(fe, false, down, pos, t);
        fe.game.ig.mmenu.proxy = proxy;
        return;
    }
    if down {
        if fe.game.ig.mmenu.pressed.is_some() || fe.game.ig.mmenu.proxy.holding() {
            return;
        }
        // An open drop-down: a choice is pressed through to its radio button; anywhere
        // else closes it, and the press goes no further.
        if fe.game.ig.mmenu.dropdown.is_some() && !matches!(at, Hit::Choice(..)) {
            fe.game.ig.mmenu.dropdown = None;
            return;
        }
        if local(&m, &l, at) {
            fe.game.ig.mmenu.pressed = Some(at);
            match at {
                Hit::Thumb(i) => {
                    if let Kind::List {
                        total, page, top, ..
                    } = &m.items[i].kind
                    {
                        if let Some(th) = l.thumb(i, *total, *page, *top) {
                            fe.game.ig.mmenu.drag = Some((i, pos.1 - th.y0));
                        }
                    }
                }
                Hit::Track(i, up) => {
                    // The original's scroll bar: a press on the track pages, with a click.
                    if let Kind::List { page, .. } = &m.items[i].kind {
                        let d = *page as isize;
                        scroll_list(fe, &m, i, ScrollTo::By(if up { -d } else { d }));
                    }
                    fe.ui_sound(sound::CLICK);
                }
                Hit::Item(i) if l.placed[i].dropdown => {
                    let open = fe.game.ig.mmenu.dropdown;
                    fe.game.ig.mmenu.dropdown = if open == Some(i) { None } else { Some(i) };
                    fe.game.ig.mmenu.pressed = None;
                    fe.ui_sound(sound::CLICK);
                }
                _ => mo::press(fe, &m, &l, at),
            }
            return;
        }
        let t = under(fe, &m, &l, at);
        fe.game.ig.mmenu.pressed = t.map(|_| at);
        let mut proxy = fe.game.ig.mmenu.proxy;
        proxy.button(fe, true, true, pos, t);
        fe.game.ig.mmenu.proxy = proxy;
        return;
    }
    // Released.
    let pressed = fe.game.ig.mmenu.pressed.take();
    fe.game.ig.mmenu.drag = None;
    fe.game.ig.mmenu.slide = None;
    if fe.game.ig.mmenu.proxy.holding() {
        let t = under(fe, &m, &l, at);
        let mut proxy = fe.game.ig.mmenu.proxy;
        proxy.button(fe, true, false, pos, t);
        fe.game.ig.mmenu.proxy = proxy;
        if matches!(pressed, Some(Hit::Choice(..))) {
            fe.game.ig.mmenu.dropdown = None;
        }
        return;
    }
    if let Some(p) = pressed {
        if p == at {
            mo::release(fe, &m, at);
        }
    }
}

/// Starts dragging one of the modern options' sliders.
pub(crate) fn start_slide(fe: &mut Fe, i: usize) {
    fe.game.ig.mmenu.slide = Some(i);
}

/// The wheel: three rows over a list (the original list's own step).
pub fn wheel(fe: &mut Fe, up: bool) {
    let Some((m, l)) = current(fe) else { return };
    let (x, y) = fe.game.ig.mmenu.mouse;
    for (i, (it, p)) in m.items.iter().zip(&l.placed).enumerate() {
        if let Kind::List { .. } = it.kind {
            let zone = Rect::new(
                p.control.x0,
                p.control.y0,
                p.track.map_or(p.control.x1, |t| t.x1 + l.px(6.0)),
                p.control.y1,
            );
            if zone.contains(x, y) {
                scroll_list(fe, &m, i, ScrollTo::Wheel(up));
            }
        }
    }
}

/// A key went down: true when the modern screen used it (the original must not see it).
pub fn key_down(fe: &mut Fe, code: u16) -> bool {
    if fe.game.ig.mmenu.page.is_some() {
        mo::key(fe, code);
        return true;
    }
    if fe.game.ig.mmenu.dropdown.take().is_some() && code == key::ESCAPE {
        return true;
    }
    let typing = fe.game.ig.binding.is_some()
        || fe.current_screen().is_some_and(|s| {
            (1..fe.screens[s].atoms.len())
                .any(|i| region_has(fe, AtomRef { screen: s, atom: i }, RSF_KEY_CAPTURE))
        });
    let mods = [key::SHIFT, key::CONTROL, key::ALT]
        .iter()
        .any(|k| fe.input.down.contains(k));
    // Escape in a text entry: the entry lets go of the keyboard (the original's own
    // handling), and the same press then goes back a screen, so one Escape is enough.
    if code == key::ESCAPE && typing && fe.game.ig.binding.is_none() {
        fe.game.ig.mmenu.escape_again = true;
        return false;
    }
    if fe.current_screen_name() == Some(HUB)
        && !typing
        && !mods
        && code == u16::from(mo::HUB_KEY.as_bytes()[0])
    {
        fe.ui_sound(sound::CLICK);
        mo::open(fe);
        return true;
    }
    false
}

/// Once a frame while up: a new screen starts with fresh highlights.
pub fn tick(fe: &mut Fe) {
    if std::mem::take(&mut fe.game.ig.mmenu.escape_again) && active(fe) {
        fe.key_up(key::ESCAPE);
        fe.key_down(key::ESCAPE);
    }
    if !active(fe) {
        if fe.game.ig.mmenu.page.is_some() || fe.game.ig.mmenu.dropdown.is_some() {
            fe.game.ig.mmenu.page = None;
            fe.game.ig.mmenu.dropdown = None;
        }
        return;
    }
    // The modern options belong to the hub.
    if fe.game.ig.mmenu.page.is_some() && fe.current_screen_name() != Some(HUB) {
        fe.game.ig.mmenu.page = None;
    }
    let name = match &fe.game.ig.mmenu.page {
        Some(_) => mo::NAME.to_string(),
        None => fe.current_screen_name().unwrap_or("").to_string(),
    };
    if fe.game.ig.mmenu.screen != name {
        let st = &mut fe.game.ig.mmenu;
        st.screen = name;
        st.hover = Hover::default();
        st.dropdown = None;
        st.drag = None;
        st.slide = None;
        if !st.proxy.holding() {
            st.pressed = None;
        }
        // The pointer goes where the new screen has it.
        let (x, y) = st.mouse;
        mouse_move(fe, x, y);
    }
}

// ---- drawing ----

fn eased(fe: &Fe, k: Key) -> f32 {
    fe.game.ig.mmenu.hover.get(k)
}

/// Draws the modern screen over the game (the modern HUD under it is dimmed with the scene).
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    let Some(pen) = modern_hud::pen(fe) else {
        return;
    };
    let (w, h) = (canvas.width, canvas.height);
    if let Some(below) = popup_below(fe) {
        let m = screen_model(fe, below);
        let l = layout(&m, w, h);
        draw_model(fe, canvas, &pen, &m, &l, false);
    }
    let Some(m) = model(fe) else { return };
    let l = layout(&m, w, h);
    let (mx, my) = fe.game.ig.mmenu.mouse;
    let at = hit(&m, &l, fe.game.ig.mmenu.dropdown, h, mx, my);
    let pressed = fe.game.ig.mmenu.pressed;
    let mut targets = Vec::new();
    let hot = |at: Hit| -> bool {
        match at {
            Hit::Item(i) | Hit::Part(i, _) | Hit::Choice(i, _) => !m.items[i].disabled,
            Hit::Thumb(_) => true,
            _ => false,
        }
    };
    if hot(at) {
        targets.push((Key::Hit(at), if pressed == Some(at) { 1.0 } else { 0.6 }));
    }
    if let Some(p) = pressed {
        if p != at {
            targets.push((Key::Hit(p), 0.25));
        }
    }
    if let Some((i, _)) = fe.game.ig.mmenu.drag {
        targets.push((Key::Hit(Hit::Thumb(i)), 1.0));
    }
    if let Some(i) = fe.game.ig.mmenu.slide {
        targets.push((Key::Hit(Hit::Item(i)), 1.0));
    }
    for (i, it) in m.items.iter().enumerate() {
        if let Kind::Toggle { on: true } = it.kind {
            targets.push((Key::On(i), 1.0));
        }
    }
    let now = fe.time;
    fe.game.ig.mmenu.hover.step(now, &targets);
    draw_model(fe, canvas, &pen, &m, &l, true);
    if let Some(i) = fe.game.ig.mmenu.dropdown {
        draw_dropdown(fe, canvas, &pen, &m, &l, i);
    }
}

fn draw_model(fe: &Fe, canvas: &mut Canvas, pen: &Pen, m: &Model, l: &Layout, live: bool) {
    let f = &l.frame;
    // Nearly opaque: the menus stand over the HUD's panels (which stay drawn, dimmed with
    // the scene) and a pop-up over the screen under it.
    canvas.blend_rect(f.panel, [6, 8, 12, 170]);
    mm::draw_frame(canvas, f);
    mm::draw_title(
        canvas,
        pen,
        f,
        &m.title.to_uppercase(),
        m.subtitle.as_deref(),
    );
    for (k, r) in l.messages.iter().enumerate() {
        let px = f.text(BODY_PX + 2.0);
        write_centred(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.0,
            (r.x0 + r.x1) / 2,
            mid_baseline(*r, px),
            TEXT,
            None,
            &m.messages[k],
        );
    }
    for &(s, r) in &l.headings {
        section_heading(
            canvas,
            pen,
            f,
            r.x0 + f.px(2.0),
            r.y1 - f.px(12.0),
            r.x1,
            &m.sections[s],
        );
    }
    for i in 0..m.items.len() {
        draw_item(fe, canvas, pen, m, l, i, live);
    }
}

fn label_col(it: &Item, hover: f32) -> Color {
    if it.disabled {
        GREYED
    } else {
        lerp(TEXT, WHITE, hover)
    }
}

/// A row's label on its left (and a note under it); returns its baseline.
fn draw_label(canvas: &mut Canvas, pen: &Pen, f: &Frame, it: &Item, p: &Placed, hover: f32) {
    if it.label.is_empty() {
        return;
    }
    let px = f.text(BODY_PX + 1.0);
    let right = if p.control.x0 > p.row.x0 {
        p.control.x0 - f.px(16.0)
    } else {
        p.row.x1
    };
    let avail = (right - p.row.x0 - f.px(12.0)) as f32;
    let text = fit(pen, Face::Semibold, px, 0.4 * f.scale, &it.label, avail);
    let base = match &it.note {
        Some(_) => mid_baseline(p.row, px) - f.px(6.0),
        None => mid_baseline(p.row, px),
    };
    write(
        canvas,
        pen,
        Face::Semibold,
        px,
        0.4 * f.scale,
        p.row.x0 + f.px(12.0),
        base,
        label_col(it, hover),
        None,
        &text,
    );
    if let Some(n) = &it.note {
        let npx = f.text(SMALL_PX - 1.0);
        let n = fit(
            pen,
            Face::Bold,
            npx,
            SMALL_SPACING * f.scale * 0.6,
            n,
            avail,
        );
        write(
            canvas,
            pen,
            Face::Bold,
            npx,
            SMALL_SPACING * f.scale * 0.6,
            p.row.x0 + f.px(12.0),
            base + f.px(15.0),
            alpha(LABEL, 0.75),
            None,
            &n,
        );
    }
}

fn draw_item(fe: &Fe, canvas: &mut Canvas, pen: &Pen, m: &Model, l: &Layout, i: usize, live: bool) {
    let it = &m.items[i];
    let p = &l.placed[i];
    let f = &l.frame;
    let e = |k: Key| if live { eased(fe, k) } else { 0.0 };
    let hv = e(Key::Hit(Hit::Item(i)));
    let pressed = live && fe.game.ig.mmenu.pressed == Some(Hit::Item(i));
    let focused = live && it.focused;
    match &it.kind {
        Kind::Button { primary, back } => {
            let b = ButtonLook {
                label: &it.label,
                key: &it.key,
                hover: hv,
                pressed: pressed && hv > 0.5,
                primary: *primary,
                idle: it.disabled,
                focused,
                ..Default::default()
            };
            let _ = back;
            draw_button(canvas, pen, f, p.control, &b);
        }
        Kind::Toggle { on } => {
            row_band(canvas, f, p.row, false, hv * 0.8, 0.0);
            draw_label(canvas, pen, f, it, p, hv);
            let knob = if live {
                e(Key::On(i))
            } else {
                f32::from(u8::from(*on))
            };
            draw_switch(canvas, f, p.control, knob, hv, it.disabled);
            if focused {
                focus_row(canvas, f, p.row);
            }
        }
        Kind::Choice {
            labels, current, ..
        } => {
            draw_label(canvas, pen, f, it, p, 0.0);
            if p.dropdown {
                let text = current
                    .and_then(|c| labels.get(c))
                    .map_or("", String::as_str);
                let open = live && fe.game.ig.mmenu.dropdown == Some(i);
                draw_select(canvas, pen, f, p.control, text, open, hv, it.disabled);
            } else {
                let look = ChoiceLook {
                    labels,
                    current: *current,
                    hover: (0..labels.len())
                        .map(|k| e(Key::Hit(Hit::Part(i, k))))
                        .collect(),
                    pressed: (0..labels.len())
                        .find(|&k| live && fe.game.ig.mmenu.pressed == Some(Hit::Part(i, k))),
                    disabled: it.disabled,
                };
                draw_segments(canvas, pen, f, p.control, &look);
            }
            if focused {
                focus_row(canvas, f, p.row);
            }
        }
        Kind::Slider {
            fraction,
            readout,
            lo,
            hi,
        } => {
            draw_label(canvas, pen, f, it, p, hv);
            let held = live
                && (fe.game.ig.mmenu.pressed == Some(Hit::Item(i))
                    || fe.game.ig.mmenu.slide == Some(i));
            draw_slider(
                canvas,
                f,
                p.control,
                *fraction,
                if held { 1.0 } else { hv },
                it.disabled,
            );
            let vpx = f.text(VALUE_PX);
            write_right(
                canvas,
                pen,
                Face::Bold,
                vpx,
                0.0,
                p.row.x1 - f.px(10.0),
                mid_baseline(p.control, vpx),
                if it.disabled { GREYED } else { WHITE },
                (!it.disabled).then_some((modern_hud::VALUE_GLOW, 3.0 * f.scale)),
                readout,
            );
            let spx = f.text(SMALL_PX - 1.5);
            let base = p.control.y1 + f.px(11.0);
            if !lo.is_empty() {
                write(
                    canvas,
                    pen,
                    Face::Bold,
                    spx,
                    SMALL_SPACING * f.scale * 0.6,
                    p.control.x0 - f.px(2.0),
                    base,
                    DIM,
                    None,
                    lo,
                );
            }
            if !hi.is_empty() {
                write_right(
                    canvas,
                    pen,
                    Face::Bold,
                    spx,
                    SMALL_SPACING * f.scale * 0.6,
                    p.control.x1 + f.px(2.0),
                    base,
                    DIM,
                    None,
                    hi,
                );
            }
            if focused {
                focus_row(canvas, f, p.row);
            }
        }
        Kind::VSlider { fraction } => {
            let held = live && fe.game.ig.mmenu.pressed == Some(Hit::Item(i));
            draw_vslider(
                canvas,
                f,
                p.control,
                *fraction,
                if held { 1.0 } else { hv },
                it.disabled,
            );
            let px = f.text(SMALL_PX);
            write_centred(
                canvas,
                pen,
                Face::Bold,
                px,
                SMALL_SPACING * f.scale * 0.5,
                (p.row.x0 + p.row.x1) / 2,
                p.control.y1 + f.px(26.0),
                lerp(LABEL, WHITE, hv),
                None,
                &it.label,
            );
        }
        Kind::List {
            rows,
            top,
            total,
            page,
        } => draw_list(fe, canvas, pen, l, it, i, rows, (*top, *total, *page), live),
        Kind::Entry {
            text,
            focused: typing,
        } => {
            let blink = (fe.time * 2.0).fract() < 0.55;
            let look = FieldLook {
                text,
                placeholder: "",
                focused: *typing,
                caret: *typing && blink,
                hover: hv,
            };
            draw_field(canvas, pen, f, p.control, &look);
        }
        Kind::Text { paragraphs } => draw_text(canvas, pen, f, p.control, paragraphs),
        Kind::Value { text } => {
            draw_label(canvas, pen, f, it, p, 0.0);
            let vpx = f.text(VALUE_PX);
            write_right(
                canvas,
                pen,
                Face::Bold,
                vpx,
                0.0,
                p.row.x1 - f.px(10.0),
                mid_baseline(p.row, vpx),
                WHITE,
                value_glow(f),
                text,
            );
        }
    }
}

/// Paragraphs wrapped into `r`, cut with "..." when they do not fit.
fn draw_text(canvas: &mut Canvas, pen: &Pen, f: &Frame, r: Rect, paragraphs: &[String]) {
    let px = f.text(BODY_PX + 1.0);
    let m = |t: &str| pen.measure(Face::Semibold, px, 0.0, t);
    let width = (r.width() - f.px(28.0)) as f32;
    let line_h = (px * 1.5).round() as i32;
    let bottom = r.y1 - f.px(4.0);
    // Every line, with a paragraph's gap before the next.
    let mut lines: Vec<(String, i32)> = Vec::new();
    for p in paragraphs {
        let wrapped = obj::wrap(p, width, width, &m);
        let n = wrapped.len();
        for (k, line) in wrapped.into_iter().enumerate() {
            lines.push((
                line,
                if k + 1 == n {
                    line_h + line_h / 3
                } else {
                    line_h
                },
            ));
        }
    }
    let mut y = r.y0 + f.px(8.0) + line_h - f.px(4.0);
    for (k, (line, step)) in lines.iter().enumerate() {
        if y > bottom {
            return;
        }
        // The last line that fits says when more is cut.
        let cut = k + 1 < lines.len() && y + step > bottom;
        let line = if cut {
            modern_panel::ellipsize(
                &m,
                &format!("{}...", line.trim_end_matches(['.', ',', ' '])),
                width,
            )
        } else {
            line.clone()
        };
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            0.0,
            r.x0 + f.px(14.0),
            y,
            TEXT,
            None,
            &line,
        );
        y += step;
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_list(
    fe: &Fe,
    canvas: &mut Canvas,
    pen: &Pen,
    l: &Layout,
    it: &Item,
    i: usize,
    rows: &[Row],
    (top, total, page): (usize, usize, usize),
    live: bool,
) {
    let f = &l.frame;
    let p = &l.placed[i];
    let keys = rows.iter().any(|r| r.keys.is_some());
    let base = p.control.y0 - f.px(12.0);
    if !it.label.is_empty() {
        section_heading(
            canvas,
            pen,
            f,
            p.row.x0 + f.px(2.0),
            base,
            p.row.x1,
            &it.label,
        );
    } else if keys {
        // The key list's columns, and how to change a binding.
        let hpx = f.text(SMALL_PX - 1.0);
        let hsp = SMALL_SPACING * f.scale;
        let w = p.control.width() - f.px(18.0);
        let x0 = p.control.x0 + f.px(4.0);
        write(
            canvas,
            pen,
            Face::Bold,
            hpx,
            hsp,
            x0 + f.px(12.0),
            base,
            DIM,
            None,
            "GROUP",
        );
        write(
            canvas,
            pen,
            Face::Bold,
            hpx,
            hsp,
            x0 + (w as f32 * 0.2) as i32,
            base,
            DIM,
            None,
            "ACTION",
        );
        let kx = x0 + (w as f32 * 0.64) as i32;
        let end = kx
            + write(
                canvas,
                pen,
                Face::Bold,
                hpx,
                hsp,
                kx,
                base,
                DIM,
                None,
                "KEYS",
            );
        let hint = "DOUBLE CLICK OR ENTER TO CHANGE";
        if end + f.px(24.0) + pen.advance(Face::Bold, hpx, hsp, hint) <= p.control.x1 {
            write_right(
                canvas,
                pen,
                Face::Bold,
                hpx,
                hsp,
                p.control.x1,
                base,
                alpha(LABEL, 0.7),
                None,
                hint,
            );
        }
    }
    well(canvas, f, p.control);
    if live && it.focused {
        rounded_outline(canvas, p.control, 4.0 * f.scale, 1.0, alpha(GOLD, 0.45));
    }
    let npx = f.text(NAME_PX - 1.0);
    let spx = f.text(SMALL_PX - 1.0);
    let ssp = SMALL_SPACING * f.scale * 0.6;
    let pulse = 0.55 + 0.45 * (fe.time as f32 * 5.0).sin().abs();
    for (k, (row, r)) in p.parts.iter().zip(rows).enumerate() {
        let hv = if live && r.selectable {
            eased(fe, Key::Hit(Hit::Part(i, k)))
        } else {
            0.0
        };
        let binding = r.keys.as_ref().is_some_and(|k| k.binding);
        row_band(canvas, f, *row, r.selected, hv, 0.0);
        if binding {
            rounded(canvas, *row, 2.0 * f.scale, alpha([255, 200, 0, 60], pulse));
        }
        let base = mid_baseline(*row, npx);
        match &r.keys {
            Some(kr) => {
                let w = row.width();
                let gx = row.x0 + f.px(12.0);
                let ax = row.x0 + (w as f32 * 0.2) as i32;
                let kx = row.x0 + (w as f32 * 0.64) as i32;
                if let Some(g) = &kr.group {
                    let g = fit(pen, Face::Bold, spx, ssp, g, (ax - gx - f.px(8.0)) as f32);
                    write(
                        canvas,
                        pen,
                        Face::Bold,
                        spx,
                        ssp,
                        gx,
                        mid_baseline(*row, spx),
                        GOLD,
                        None,
                        &g,
                    );
                }
                let col = if r.selected {
                    lerp(LABEL, WHITE, hv)
                } else {
                    lerp(TEXT, WHITE, hv)
                };
                let a = fit(
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    &kr.action,
                    (kx - ax - f.px(10.0)) as f32,
                );
                write(
                    canvas,
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    ax,
                    base,
                    col,
                    None,
                    &a,
                );
                let cy = (row.y0 + row.y1) / 2;
                if binding {
                    write(
                        canvas,
                        pen,
                        Face::Bold,
                        spx,
                        ssp,
                        kx,
                        mid_baseline(*row, spx),
                        alpha(GOLD, pulse),
                        None,
                        "PRESS A KEY",
                    );
                } else if kr.keys.is_empty() {
                    write(
                        canvas,
                        pen,
                        Face::Bold,
                        spx,
                        ssp,
                        kx,
                        mid_baseline(*row, spx),
                        alpha(REFUSED, 0.85),
                        None,
                        "NONE",
                    );
                } else {
                    let mut x = kx;
                    for k in &kr.keys {
                        let cap = key_cap_rect(pen, f, 0, cy, k);
                        let cap = cap.offset(x - cap.x0, 0);
                        if cap.x1 > row.x1 - f.px(6.0) {
                            break;
                        }
                        draw_key_cap(canvas, pen, f, cap, k, col, 0);
                        x = cap.x1 + f.px(6.0);
                    }
                }
            }
            None => {
                let col = if !r.selectable {
                    GOLD
                } else if r.selected {
                    lerp(LABEL, WHITE, hv)
                } else {
                    lerp(TEXT, WHITE, hv)
                };
                let t = fit(
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    &r.text,
                    (row.width() - f.px(24.0)) as f32,
                );
                write(
                    canvas,
                    pen,
                    Face::Semibold,
                    npx,
                    0.0,
                    row.x0 + f.px(12.0),
                    base,
                    col,
                    None,
                    &t,
                );
            }
        }
    }
    if let Some(th) = l.thumb(i, total, page, top) {
        let hv = if live {
            eased(fe, Key::Hit(Hit::Thumb(i)))
        } else {
            0.0
        };
        draw_scroll(canvas, p.track.unwrap_or(th), th, hv);
    }
}

fn draw_dropdown(fe: &Fe, canvas: &mut Canvas, pen: &Pen, m: &Model, l: &Layout, i: usize) {
    let Some(Kind::Choice {
        labels, current, ..
    }) = m.items.get(i).map(|it| &it.kind)
    else {
        return;
    };
    let rows = l.dropdown_rows(i, labels.len(), canvas.height);
    let look = ChoiceLook {
        labels,
        current: *current,
        hover: (0..labels.len())
            .map(|k| eased(fe, Key::Hit(Hit::Choice(i, k))))
            .collect(),
        pressed: (0..labels.len()).find(|&k| fe.game.ig.mmenu.pressed == Some(Hit::Choice(i, k))),
        disabled: false,
    };
    draw_choices(canvas, pen, &l.frame, &rows, &look);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(kind: Kind, label: &str, section: usize, footer: bool) -> Item {
        let mut it = Item::new(
            kind,
            label.to_string(),
            Source::Nothing,
            Rect::new(0, 0, 10, 10),
        );
        it.section = section;
        it.footer = footer;
        it
    }

    fn button(label: &str, back: bool) -> Kind {
        let _ = label;
        Kind::Button {
            primary: false,
            back,
        }
    }

    fn menu() -> Model {
        Model {
            name: "In_game_esc_menu2".into(),
            style: Style::Menu,
            title: "GAME OPTIONS".into(),
            subtitle: None,
            messages: Vec::new(),
            sections: vec![String::new()],
            items: ["QUIT", "SAVE GAME", "LOAD GAME", "OPTIONS", "BACK TO GAME"]
                .iter()
                .enumerate()
                .map(|(k, l)| item(button(l, k == 4), l, 0, false))
                .collect(),
        }
    }

    fn form(rows: usize) -> Model {
        let mut items = Vec::new();
        for k in 0..rows {
            items.push(item(
                Kind::Toggle { on: k % 2 == 0 },
                &format!("OPTION {k}"),
                k / 6,
                false,
            ));
        }
        items.push(item(
            Kind::Slider {
                fraction: 0.5,
                readout: "50%".into(),
                lo: "LO".into(),
                hi: "HI".into(),
            },
            "MUSIC",
            0,
            false,
        ));
        items.push(item(
            Kind::Choice {
                labels: vec!["LOW".into(), "MEDIUM".into(), "HIGH".into()],
                sources: vec![Source::Nothing; 3],
                current: Some(1),
            },
            "MODEL QUALITY",
            0,
            false,
        ));
        items.push(item(
            Kind::Choice {
                labels: vec![
                    "MULTIMEDIA SPEAKERS".into(),
                    "STEREO/SURROUND SOUND".into(),
                    "HEADPHONES".into(),
                    "CUSTOM".into(),
                ],
                sources: vec![Source::Nothing; 4],
                current: Some(0),
            },
            "SPEAKER SETTINGS",
            0,
            false,
        ));
        items.push(item(
            Kind::Button {
                primary: true,
                back: false,
            },
            "ACCEPT!",
            0,
            true,
        ));
        items.push(item(
            Kind::Button {
                primary: false,
                back: true,
            },
            "CANCEL",
            0,
            true,
        ));
        let n = rows.div_ceil(6).max(1);
        let mut sections = vec![String::new()];
        sections.extend((1..n).map(|k| format!("SECTION {k}")));
        // The items in section order, as the model keeps them.
        items.sort_by_key(|i| (i.footer, i.section));
        Model {
            name: "Test".into(),
            style: Style::Form,
            title: "GAME OPTIONS".into(),
            subtitle: Some("TEST".into()),
            messages: Vec::new(),
            sections,
            items,
        }
    }

    fn inside(outer: Rect, r: Rect) -> bool {
        r.x0 >= outer.x0 && r.y0 >= outer.y0 && r.x1 <= outer.x1 && r.y1 <= outer.y1
    }

    fn overlap(a: Rect, b: Rect) -> bool {
        a.x0 < b.x1 && b.x0 < a.x1 && a.y0 < b.y1 && b.y0 < a.y1
    }

    const SIZES: [(i32, i32); 4] = [(1920, 1080), (1024, 768), (640, 480), (3840, 2160)];

    #[test]
    fn menus_stack_their_buttons_and_fit_every_window() {
        let m = menu();
        for (w, h) in SIZES {
            let l = layout(&m, w, h);
            assert!(l.frame.panel.x0 >= 0 && l.frame.panel.x1 <= w, "{w}x{h}");
            assert!(l.frame.panel.y0 >= 0 && l.frame.panel.y1 <= h, "{w}x{h}");
            for p in &l.placed {
                assert!(inside(l.frame.panel, p.control), "{w}x{h} {:?}", p.control);
            }
            // Top to bottom in the original's order; the back button set apart.
            for k in 1..l.placed.len() {
                assert!(l.placed[k].control.y0 > l.placed[k - 1].control.y1);
            }
            let gap = |k: usize| l.placed[k].control.y0 - l.placed[k - 1].control.y1;
            assert!(gap(4) > gap(3), "the back button sits apart");
        }
        assert_eq!(layout(&m, 1920, 1080).frame.scale, 1.0);
        assert!(
            layout(&m, 640, 480).frame.scale >= 0.8,
            "a menu is readable at 640x480"
        );
    }

    #[test]
    fn a_confirmation_puts_its_two_buttons_side_by_side() {
        let mut m = menu();
        m.title = "QUIT GAME?".into();
        m.items.truncate(2);
        let l = layout(&m, 1920, 1080);
        assert_eq!(l.placed[0].control.y0, l.placed[1].control.y0);
        assert!(l.placed[0].control.x1 < l.placed[1].control.x0);
    }

    #[test]
    fn forms_lay_out_without_overlaps_at_every_size() {
        for rows in [3, 14, 26] {
            let m = form(rows);
            for (w, h) in SIZES {
                let l = layout(&m, w, h);
                let panel = l.frame.panel;
                assert!(panel.x0 >= 0 && panel.x1 <= w && panel.y0 >= 0 && panel.y1 <= h);
                let rects: Vec<Rect> = l.placed.iter().map(|p| p.row).collect();
                for (a, r) in rects.iter().enumerate() {
                    assert!(
                        inside(panel, *r),
                        "{rows} rows {w}x{h}: {r:?} outside {panel:?}"
                    );
                    for b in rects.iter().skip(a + 1) {
                        assert!(!overlap(*r, *b), "{rows} rows {w}x{h}: {r:?} and {b:?}");
                    }
                }
                for p in &l.placed {
                    assert!(inside(p.row, p.control) || p.row == p.control, "{p:?}");
                }
                // The footer sits under the body.
                let body_bottom = m
                    .items
                    .iter()
                    .zip(&l.placed)
                    .filter(|(i, _)| !i.footer)
                    .map(|(_, p)| p.row.y1)
                    .max()
                    .unwrap();
                assert!(l.frame.footer_line >= body_bottom - 1);
            }
        }
        // A long form takes two columns.
        let long = layout(&form(26), 1920, 1080);
        let xs: std::collections::BTreeSet<i32> =
            long.placed.iter().take(26).map(|p| p.row.x0).collect();
        assert_eq!(xs.len(), 2, "two columns");
        let short = layout(&form(3), 1920, 1080);
        assert!(short.frame.panel.width() < long.frame.panel.width());
    }

    #[test]
    fn choices_are_segments_when_they_fit_else_a_drop_down() {
        let m = form(3);
        let l = layout(&m, 1920, 1080);
        let q = m.find("MODEL QUALITY").unwrap();
        let s = m.find("SPEAKER SETTINGS").unwrap();
        assert!(!l.placed[q].dropdown);
        assert_eq!(l.placed[q].parts.len(), 3);
        assert!(l.placed[s].dropdown);
        let rows = l.dropdown_rows(s, 4, 1080);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].y0 >= l.placed[s].control.y1, "opens downwards");
        // Near the bottom of the window it opens upwards.
        let up = l.dropdown_rows(s, 4, l.placed[s].control.y1 + 20);
        assert!(up[3].y1 <= l.placed[s].control.y0);
    }

    #[test]
    fn hit_testing() {
        let m = form(3);
        let l = layout(&m, 1920, 1080);
        let c = |r: Rect| ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
        // A toggle's whole row.
        let t = 0;
        let r = l.placed[t].row;
        assert_eq!(
            hit(&m, &l, None, 1080, r.x0 + 20, (r.y0 + r.y1) / 2),
            Hit::Item(t)
        );
        // A segment.
        let q = m.find("MODEL QUALITY").unwrap();
        let (x, y) = c(l.placed[q].parts[2]);
        assert_eq!(hit(&m, &l, None, 1080, x, y), Hit::Part(q, 2));
        // The slider's grip zone reaches a little past its ends.
        let s = m.find("MUSIC").unwrap();
        let tr = l.placed[s].control;
        assert_eq!(
            hit(&m, &l, None, 1080, tr.x0 - 4, (tr.y0 + tr.y1) / 2),
            Hit::Item(s)
        );
        // A drop-down's box, then its open choices.
        let d = m.find("SPEAKER SETTINGS").unwrap();
        let (x, y) = c(l.placed[d].control);
        assert_eq!(hit(&m, &l, None, 1080, x, y), Hit::Item(d));
        let rows = l.dropdown_rows(d, 4, 1080);
        let (x, y) = c(rows[1]);
        assert_eq!(hit(&m, &l, Some(d), 1080, x, y), Hit::Choice(d, 1));
        // The footer.
        let a = m.find("ACCEPT!").unwrap();
        let (x, y) = c(l.placed[a].control);
        assert_eq!(hit(&m, &l, None, 1080, x, y), Hit::Item(a));
        assert_eq!(l.placed[a].control, l.frame.footer_button(0));
        let b = m.find("CANCEL").unwrap();
        assert_eq!(l.placed[b].control, l.frame.footer_right());
        assert_eq!(hit(&m, &l, None, 1080, 2, 2), Hit::Outside);
    }

    #[test]
    fn lists_hit_rows_thumb_and_track() {
        let rows: Vec<Row> = (0..19)
            .map(|k| Row {
                text: format!("ROW {k}"),
                selectable: true,
                ..Default::default()
            })
            .collect();
        let mut m = form(0);
        m.items.insert(
            0,
            item(
                Kind::List {
                    rows,
                    top: 0,
                    total: 147,
                    page: 19,
                },
                "",
                0,
                false,
            ),
        );
        for (w, h) in SIZES {
            let l = layout(&m, w, h);
            let p = &l.placed[0];
            assert_eq!(p.parts.len(), 19);
            assert!(inside(l.frame.panel, p.control));
            let r = p.parts[18];
            assert_eq!(
                hit(&m, &l, None, h, r.x0 + 10, (r.y0 + r.y1) / 2),
                Hit::Part(0, 18)
            );
            let th = l.thumb(0, 147, 19, 0).expect("a thumb");
            assert_eq!(hit(&m, &l, None, h, th.x0 + 1, th.y0 + 2), Hit::Thumb(0));
            assert_eq!(
                hit(&m, &l, None, h, th.x0 + 1, th.y1 + 10),
                Hit::Track(0, false)
            );
        }
    }

    #[test]
    fn hot_key_names() {
        assert_eq!(hot_key_name(b'Q'), "Q");
        assert_eq!(hot_key_name(27), "ESC");
        assert_eq!(hot_key_name(0), "");
    }

    #[test]
    fn mode_switch() {
        let mut o = Options::default();
        assert!(enabled(&o), "on by default");
        o.set_bool(OPTION_KEY, false);
        assert!(!enabled(&o));
        o.set_bool(OPTION_KEY, true);
        o.set_bool(modern_hud::OPTION_KEY, false);
        assert!(
            !enabled(&o),
            "the original HUD brings back the original menus"
        );
        assert_eq!(parse_mode("Modern"), Some(true));
        assert_eq!(parse_mode("original"), Some(false));
        assert_eq!(parse_mode("x"), None);
    }
}
