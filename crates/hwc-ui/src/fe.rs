//! The front-end manager (`FEFlow.c` + the control half of `UIControls.c` + `Region.c`
//! processing): screens loaded from `.fib` files, the screen stack, region creation for every atom
//! type, mouse/keyboard event processing, keyboard navigation, links and named callbacks.
//!
//! `FrontEnd<G>` is generic over the game state `G` that callbacks work on. Callbacks are plain
//! function pointers registered by name, like `feCallbackAdd`: the `.fib` files refer to them by
//! the atom names.

use std::collections::{HashMap, HashSet, VecDeque};
use std::rc::Rc;

use hwc_formats::fib::{AtomData, Fib};

use crate::assets::Resources;
use crate::canvas::{Canvas, Color, Rect};
use crate::consts::*;
use crate::controls::{
    Control, ListItem, ListWindow, ScrollBar, Slider, SliderHandler, TextEntry, LIST_MARGIN,
};
use crate::fer::{BitmapLook, ButtonLook, Cutout, Fer, MarkLook, SliderLook};
use crate::region::{region_inside, AtomRef, Draw, KeyAction, Region, RegionId, Regions, Role};
use crate::text::{self, Shadow};

/// Native size of front-end layouts.
pub const FE_WIDTH: i32 = 640;
pub const FE_HEIGHT: i32 = 480;

/// Width of a list window's scroll bar and the gap before it (`LW_BarWidth`, `LW_WindowXBarSpace`).
const LW_BAR_WIDTH: i32 = 16;
const LW_BAR_SPACE: i32 = 2;

/// Which atom kinds may adopt regions lying on top of them (`feCanStack`).
const CAN_STACK: [bool; 22] = [
    false, true, true, true, true, true, false, false, true, false, false, true, true, false,
    false, false, false, true, false, false, true, false,
];

/// Decorative regions are looked up in [`CAN_STACK`] by the 13th character of their bitmap path
/// in Cataclysm (the region's ID is the name, not the atom), reading past the table into the
/// bytes that follow it. This mask says which character codes land on a non-zero byte.
const DECORATIVE_STACK_MASK: [u64; 4] = [
    0x3fff_ffff_ff12_193e,
    0xffff_ff7f_ffff_ffff,
    0xff3f_fff1_ff0f_fff7,
    0x3fff_ffff_f3ff_ffff,
];

fn decorative_stackable(path: &str) -> bool {
    let c = path.as_bytes().get(12).copied().unwrap_or(0) as usize;
    DECORATIVE_STACK_MASK[c / 64] >> (c % 64) & 1 != 0
}

/// A screen link (`felink`).
#[derive(Debug, Clone, PartialEq)]
pub struct FeLink {
    pub name: Option<String>,
    pub flags: u32,
    pub target: Option<String>,
}

/// An atom with its runtime state (`featom`).
#[derive(Debug, Clone, PartialEq)]
pub struct FeAtom {
    pub name: Option<String>,
    pub flags: u32,
    /// Runtime status (`FAS_*`): checked, first/last call.
    pub status: u32,
    /// Atom type after loading (labels become `FA_STATIC_TEXT`).
    pub kind: u8,
    pub border_color: Color,
    pub content_color: Color,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    loaded: (i32, i32, i32, i32),
    /// Label text in the engine's 8-bit encoding (English).
    pub text: Option<Vec<u8>>,
    /// The label in every language the file has (English, French, German, ...).
    pub texts: Vec<Vec<u8>>,
    pub font: Option<String>,
    /// Radio button value.
    pub radio_value: u32,
    /// Bitmap button base name.
    pub bitmap: Option<String>,
    pub hotkey_modifiers: u8,
    pub hotkeys: [u8; 5],
    pub tabstop: u16,
    pub drawstyle: [u32; 2],
    pub region: Option<RegionId>,
}

impl FeAtom {
    pub fn rect(&self) -> Rect {
        Rect::from_xywh(self.x, self.y, self.width, self.height)
    }

    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }

    /// The rectangle as loaded, on the 640x480 layout (before the screen was placed on a
    /// screen of another size).
    pub fn layout_rect(&self) -> Rect {
        let (x, y, w, h) = self.loaded;
        Rect::from_xywh(x, y, w, h)
    }

    pub fn checked(&self) -> bool {
        self.status & FAS_CHECKED != 0
    }

    /// Whether the atom lies at least partly on the 640x480 layout (`feAtomOnScreen`).
    fn on_screen(&self) -> bool {
        let on = |x: i32, y: i32| (0..FE_WIDTH).contains(&x) && (0..FE_HEIGHT).contains(&y);
        on(self.x, self.y)
            || on(self.x + self.width, self.y)
            || on(self.x + self.width, self.y + self.height)
            || on(self.x, self.y + self.height)
    }
}

/// A loaded screen (`fescreen`).
#[derive(Debug, Clone, PartialEq)]
pub struct FeScreen {
    pub name: String,
    pub flags: u32,
    pub links: Vec<FeLink>,
    pub atoms: Vec<FeAtom>,
    /// Screens with menu items keep their 640x480 coordinates at any resolution.
    has_menus: bool,
    /// An extra displacement of a screen that keeps its 640x480 coordinates (an opt-in
    /// layout change, see [`FrontEnd::set_screen_offset`]); (0, 0) as loaded.
    offset: (i32, i32),
}

impl FeScreen {
    pub fn link(&self, name: &str) -> Option<&FeLink> {
        self.links.iter().find(|l| l.name.as_deref() == Some(name))
    }

    pub fn atom_index(&self, name: &str) -> Option<usize> {
        self.atoms
            .iter()
            .position(|a| a.name.as_deref() == Some(name))
    }
}

/// One entry of the screen stack (`festackentry`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackEntry {
    pub screen: Option<usize>,
    pub base: RegionId,
    pub parent: Option<RegionId>,
}

/// What a callback was called for.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub name: String,
    pub atom: Option<AtomRef>,
}

/// A named function callback (`fefunction`).
pub type FeFunction<G> = fn(&mut FrontEnd<G>, &Call);
/// A named user-region draw callback (`fedrawfunction`).
pub type FeDrawFunction<G> = fn(&mut FrontEnd<G>, &mut Fer, AtomRef, Rect);
/// A slider value handler installed by an option callback.
pub type SliderFunction<G> = fn(&mut FrontEnd<G>, RegionId);
/// A region process function installed by name (`regFunctionSet` on a user region).
pub type RegionFunction<G> = fn(&mut FrontEnd<G>, RegionId, u32);
/// A list window item drawer.
pub type ListItemDraw<G> = fn(&FrontEnd<G>, &mut Fer, Rect, &ListItem);

/// Keyboard and mouse state as the region processor sees it.
#[derive(Debug, Clone, Default)]
pub struct Input {
    pub mouse_x: i32,
    pub mouse_y: i32,
    pub left: bool,
    pub right: bool,
    /// Keys currently held.
    pub down: HashSet<u16>,
    /// Keys pressed since last consumed ("stuck" keys).
    pub stuck: HashSet<u16>,
    pub wheel_up: bool,
    pub wheel_down: bool,
    pub double_left: bool,
    /// Buffered key presses for focused text entries and list windows: code | shift bit.
    pub typed: VecDeque<u32>,
    /// Characters typed (for text entries).
    pub chars: VecDeque<char>,
}

impl Input {
    fn all_stuck(&self, keys: &[u16]) -> bool {
        !keys.is_empty() && keys.iter().all(|k| self.stuck.contains(k))
    }

    fn all_down(&self, keys: &[u16]) -> bool {
        !keys.is_empty() && keys.iter().all(|k| self.down.contains(k))
    }
}

/// The front-end manager.
pub struct FrontEnd<G> {
    /// Game state the callbacks operate on.
    pub game: G,
    res: Rc<Resources>,
    pub screens: Vec<FeScreen>,
    pub regions: Regions,
    root: RegionId,
    main: RegionId,
    pub stack: Vec<StackEntry>,
    callbacks: HashMap<String, FeFunction<G>>,
    draw_callbacks: HashMap<String, FeDrawFunction<G>>,
    slider_handlers: HashMap<&'static str, SliderFunction<G>>,
    list_drawers: HashMap<String, ListItemDraw<G>>,
    region_functions: HashMap<&'static str, RegionFunction<G>>,
    pub tab_stop: u32,
    clicked_left: Option<RegionId>,
    clicked_left_last: Option<RegionId>,
    keys_focussed: bool,
    pub input: Input,
    /// Size of the screen the layouts are positioned on.
    screen_size: (i32, i32),
    mouse_clip: Option<Rect>,
    /// Messages for the log (unknown callbacks, stubs).
    pub log: Vec<String>,
    logged: HashSet<String>,
    /// Time in seconds since start, advanced by [`FrontEnd::process`].
    pub time: f64,
    last_press: Option<(f64, i32, i32)>,
    /// The left button is down as the second press of a double click (no press of its own).
    left_double_held: bool,
    /// UI sound events fired by the controls since the application last took them
    /// ([`crate::consts::sound`]).
    pub sounds: Vec<u32>,
}

impl<G> FrontEnd<G> {
    pub fn new(game: G, res: Rc<Resources>) -> Self {
        let mut regions = Regions::default();
        let full = Rect::new(0, 0, FE_WIDTH, FE_HEIGHT);
        let root = regions.alloc(Region::new(full, 0, Role::Root));
        let main = regions.alloc(Region::new(full, 0, Role::Root));
        regions.insert_child(main, root);
        let mut fe = Self {
            game,
            res,
            screens: Vec::new(),
            regions,
            root,
            main,
            stack: vec![StackEntry {
                screen: None,
                base: main,
                parent: None,
            }],
            callbacks: HashMap::new(),
            draw_callbacks: HashMap::new(),
            slider_handlers: HashMap::new(),
            list_drawers: HashMap::new(),
            region_functions: HashMap::new(),
            tab_stop: 1,
            clicked_left: None,
            clicked_left_last: None,
            keys_focussed: false,
            input: Input::default(),
            screen_size: (FE_WIDTH, FE_HEIGHT),
            mouse_clip: None,
            log: Vec::new(),
            logged: HashSet::new(),
            time: 0.0,
            last_press: None,
            left_double_held: false,
            sounds: Vec::new(),
        };
        fe.callback_add("SCREEN_Disappear", |fe, _| fe.current_screen_delete());
        fe
    }

    pub fn resources(&self) -> &Rc<Resources> {
        &self.res
    }

    /// The main region new screens are parented to (`ghMainRegion`).
    pub fn main_region(&self) -> RegionId {
        self.main
    }

    pub fn screen_size(&self) -> (i32, i32) {
        self.screen_size
    }

    /// Records a message once per distinct text and prints it.
    pub fn log_once(&mut self, msg: String) {
        if self.logged.insert(msg.clone()) {
            eprintln!("hwc-ui: {msg}");
            self.log.push(msg);
        }
    }

    /// Records a diagnostic the engine only printed in debug builds; printed when the
    /// `HWC_UI_DEBUG` environment variable is set.
    pub fn debug_once(&mut self, msg: String) {
        if self.logged.insert(msg.clone()) {
            if std::env::var_os("HWC_UI_DEBUG").is_some() {
                eprintln!("hwc-ui: {msg}");
            }
            self.log.push(msg);
        }
    }

    // ---------------------------------------------------------------------------------------
    // Registration
    // ---------------------------------------------------------------------------------------

    /// `feCallbackAdd`: binds a function to an atom name (replacing an earlier binding).
    pub fn callback_add(&mut self, name: &str, f: FeFunction<G>) {
        self.callbacks.insert(name.to_string(), f);
    }

    /// `feDrawCallbackAdd`.
    pub fn draw_callback_add(&mut self, name: &str, f: FeDrawFunction<G>) {
        self.draw_callbacks.insert(name.to_string(), f);
    }

    pub fn slider_handler_add(&mut self, name: &'static str, f: SliderFunction<G>) {
        self.slider_handlers.insert(name, f);
    }

    pub fn list_drawer_add(&mut self, name: &str, f: ListItemDraw<G>) {
        self.list_drawers.insert(name.to_string(), f);
    }

    pub fn region_function_add(&mut self, name: &'static str, f: RegionFunction<G>) {
        self.region_functions.insert(name, f);
    }

    /// `regFunctionSet` + `regFilterSet` on a named atom's region of the current screens.
    pub fn region_function_set(
        &mut self,
        atom: &str,
        function: &'static str,
        filter: u32,
    ) -> Option<RegionId> {
        let a = self.find_atom(atom)?;
        let id = self.atom(a).region?;
        let r = self.regions.get_mut(id)?;
        r.role = Role::Custom(function);
        r.filter = filter;
        Some(id)
    }

    /// Confines the mouse to a rectangle (`mouseClipToRect`), or frees it.
    pub fn mouse_clip(&mut self, rect: Option<Rect>) {
        self.mouse_clip = rect;
    }

    /// The mouse position as regions see it.
    pub fn mouse_pos(&self) -> (i32, i32) {
        self.mouse()
    }

    pub fn has_callback(&self, name: &str) -> bool {
        self.callbacks.contains_key(name)
    }

    pub fn has_draw_callback(&self, name: &str) -> bool {
        self.draw_callbacks.contains_key(name)
    }

    // ---------------------------------------------------------------------------------------
    // Loading
    // ---------------------------------------------------------------------------------------

    /// `feScreensLoad`: adds every screen of a `.fib` (positioned for the current screen size).
    pub fn load_fib(&mut self, fib: &Fib) {
        for s in &fib.screens {
            let has_menus = s.atoms.iter().any(|a| a.raw_kind == FA_MENU_ITEM)
                || s.name == "HyperspaceRollCall";
            let atoms = s
                .atoms
                .iter()
                .map(|a| {
                    let mut kind = a.raw_kind;
                    let (mut text, mut font, mut radio_value) = (None, None, 0);
                    match &a.data {
                        AtomData::Text { text: t, font: f } if kind != FA_RADIO_BUTTON => {
                            kind = FA_STATIC_TEXT;
                            text = Some(text::to_latin1(t));
                            font = f.clone();
                        }
                        AtomData::RadioValue(v) => radio_value = *v,
                        AtomData::Raw(_)
                            if kind != FA_RADIO_BUTTON && a.flags & FAF_BITMAP == 0 =>
                        {
                            kind = FA_STATIC_TEXT;
                        }
                        _ => {}
                    }
                    let r = a.rect;
                    let mut width = r.right - r.left;
                    if kind == FA_LIST_WINDOW {
                        width -= LW_BAR_WIDTH + LW_BAR_SPACE;
                    }
                    let height = r.bottom - r.top;
                    FeAtom {
                        name: a.name.clone(),
                        flags: a.flags,
                        status: 0,
                        kind,
                        border_color: a.border_color,
                        content_color: a.content_color,
                        x: r.left,
                        y: r.top,
                        width,
                        height,
                        loaded: (r.left, r.top, width, height),
                        texts: text.iter().cloned().collect(),
                        text,
                        font,
                        radio_value,
                        bitmap: a.bitmap.clone(),
                        hotkey_modifiers: a.hotkey_modifiers,
                        hotkeys: a.hotkeys,
                        tabstop: a.tab_stop,
                        drawstyle: a.draw_style,
                        region: None,
                    }
                })
                .collect();
            let links = s
                .links
                .iter()
                .map(|l| FeLink {
                    name: l.name.clone(),
                    flags: l.flags,
                    target: l.target.clone(),
                })
                .collect();
            let mut screen = FeScreen {
                name: s.name.clone(),
                flags: s.flags,
                links,
                atoms,
                has_menus,
                offset: (0, 0),
            };
            Self::position_screen(&mut screen, self.screen_size);
            self.screens.push(screen);
        }
    }

    /// `feScreensLoad` from the file bytes: like [`FrontEnd::load_fib`], plus every language's
    /// version of each label (stored one after another, NUL-separated).
    pub fn load_fib_data(&mut self, data: &[u8]) -> Result<(), String> {
        let fib = Fib::parse(data).map_err(|e| e.to_string())?;
        let first = self.screens.len();
        self.load_fib(&fib);
        for (si, labels) in label_strings(data).into_iter().enumerate() {
            let Some(screen) = self.screens.get_mut(first + si) else {
                break;
            };
            for (ai, texts) in labels.into_iter().enumerate() {
                if let (Some(atom), Some(texts)) = (screen.atoms.get_mut(ai), texts) {
                    if atom.text.is_some() && !texts.is_empty() {
                        atom.texts = texts;
                    }
                }
            }
        }
        Ok(())
    }

    /// Loads only the screens named `keep` of a `.fib` (screens another file's links reach).
    pub fn load_fib_screens(&mut self, data: &[u8], keep: &[&str]) -> Result<(), String> {
        let first = self.screens.len();
        self.load_fib_data(data)?;
        let mut i = first;
        while i < self.screens.len() {
            if keep.contains(&self.screens[i].name.as_str())
                && self.screens[..i]
                    .iter()
                    .all(|s| s.name != self.screens[i].name)
            {
                i += 1;
            } else {
                self.screens.remove(i);
            }
        }
        Ok(())
    }

    /// Applies the resolution rules to one screen: background atoms stretch to the screen
    /// edges they touch, everything else is centred, and atoms entirely off the 640x480 layout
    /// are hidden.
    fn position_screen(screen: &mut FeScreen, size: (i32, i32)) {
        if screen.has_menus {
            let (ox, oy) = screen.offset;
            for atom in &mut screen.atoms {
                atom.x = atom.loaded.0 + ox;
                atom.y = atom.loaded.1 + oy;
            }
            return;
        }
        let (dx, dy) = ((size.0 - FE_WIDTH) / 2, (size.1 - FE_HEIGHT) / 2);
        for atom in &mut screen.atoms {
            let (x, y, w, h) = atom.loaded;
            atom.x = x;
            atom.y = y;
            atom.width = w;
            atom.height = h;
            if atom.has(FAF_BACKGROUND) {
                rescale_background(atom, size);
            } else {
                if !atom.on_screen() {
                    atom.flags |= FAF_HIDDEN;
                }
                atom.x += dx;
                atom.y += dy;
            }
        }
    }

    /// Changes the screen size (`feAllScreensReposition`): every screen is repositioned and the
    /// stack is rebuilt. The engine drops back to the main menu here; this keeps the screens
    /// that were open (their create callbacks run again).
    pub fn set_screen_size(&mut self, width: i32, height: i32) {
        if (width, height) == self.screen_size {
            return;
        }
        self.screen_size = (width, height);
        let full = Rect::new(0, 0, width, height);
        for id in [self.root, self.main] {
            if let Some(r) = self.regions.get_mut(id) {
                r.rect = full;
            }
        }
        for s in &mut self.screens {
            Self::position_screen(s, (width, height));
        }
        let names: Vec<(String, bool)> = self
            .stack
            .iter()
            .filter_map(|e| e.screen)
            .map(|s| {
                (
                    self.screens[s].name.clone(),
                    self.screens[s].atoms[0].has(FAF_POPUP),
                )
            })
            .collect();
        while self.stack.len() > 1 {
            self.current_screen_delete();
        }
        for (name, popup) in names {
            let parent = if popup {
                self.current_base()
            } else {
                self.main
            };
            self.screen_start_from(parent, &name);
        }
    }

    pub fn find_screen(&self, name: &str) -> Option<usize> {
        self.screens.iter().position(|s| s.name == name)
    }

    /// Whether (`x`, `y`) is on a started screen named `name`: inside its base region or one
    /// of the regions under it (what the region system would hand the event to).
    pub fn screen_contains(&self, name: &str, x: i32, y: i32) -> bool {
        let Some(screen) = self.find_screen(name) else {
            return false;
        };
        let mut todo: Vec<RegionId> = self
            .stack
            .iter()
            .filter(|e| e.screen == Some(screen))
            .map(|e| e.base)
            .collect();
        while let Some(id) = todo.pop() {
            let Some(r) = self.regions.get(id) else {
                continue;
            };
            if r.rect.contains(x, y) {
                return true;
            }
            todo.extend(r.children.iter().copied());
        }
        false
    }

    /// Moves a screen that keeps its 640x480 coordinates (the menus and the roll call) by
    /// (`dx`, `dy`) from where the file puts it. Not something the original does; used for
    /// opt-in layout changes. Takes effect the next time the screen starts.
    pub fn set_screen_offset(&mut self, name: &str, dx: i32, dy: i32) {
        let size = self.screen_size;
        if let Some(i) = self.find_screen(name) {
            let screen = &mut self.screens[i];
            if screen.has_menus && screen.offset != (dx, dy) {
                screen.offset = (dx, dy);
                Self::position_screen(screen, size);
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Accessors used by callbacks
    // ---------------------------------------------------------------------------------------

    pub fn atom(&self, r: AtomRef) -> &FeAtom {
        &self.screens[r.screen].atoms[r.atom]
    }

    pub fn atom_mut(&mut self, r: AtomRef) -> &mut FeAtom {
        &mut self.screens[r.screen].atoms[r.atom]
    }

    pub fn region(&self, id: RegionId) -> Option<&Region> {
        self.regions.get(id)
    }

    pub fn region_mut(&mut self, id: RegionId) -> Option<&mut Region> {
        self.regions.get_mut(id)
    }

    /// The screen on top of the stack.
    pub fn current_screen(&self) -> Option<usize> {
        self.stack.last().and_then(|e| e.screen)
    }

    pub fn current_screen_name(&self) -> Option<&str> {
        self.current_screen().map(|s| self.screens[s].name.as_str())
    }

    /// Names of the screens on the stack, bottom first.
    pub fn stack_names(&self) -> Vec<&str> {
        self.stack
            .iter()
            .filter_map(|e| e.screen)
            .map(|s| self.screens[s].name.as_str())
            .collect()
    }

    pub fn current_base(&self) -> RegionId {
        self.stack.last().map_or(self.main, |e| e.base)
    }

    /// Whether a callback runs because its screen was just created (`FEFIRSTCALL`).
    pub fn first_call(&self, call: &Call) -> bool {
        call.atom
            .is_some_and(|a| self.atom(a).status & FAS_ON_CREATE != 0)
    }

    /// Whether a callback runs because its screen is being deleted (`FELASTCALL`).
    pub fn last_call(&self, call: &Call) -> bool {
        call.atom
            .is_some_and(|a| self.atom(a).status & FAS_ON_DELETE != 0)
    }

    /// Finds a named atom on the topmost screen of the stack that has it.
    pub fn find_atom(&self, name: &str) -> Option<AtomRef> {
        self.stack
            .iter()
            .rev()
            .filter_map(|e| e.screen)
            .find_map(|s| {
                self.screens[s]
                    .atom_index(name)
                    .map(|atom| AtomRef { screen: s, atom })
            })
    }

    /// All atoms with this name on that screen (radio groups).
    pub fn atoms_named(&self, screen: usize, name: &str) -> Vec<AtomRef> {
        self.screens[screen]
            .atoms
            .iter()
            .enumerate()
            .filter(|(_, a)| a.name.as_deref() == Some(name))
            .map(|(atom, _)| AtomRef { screen, atom })
            .collect()
    }

    /// The slider state of an atom's region.
    pub fn slider(&self, atom: AtomRef) -> Option<&Slider> {
        match &self.regions.get(self.atom(atom).region?)?.control {
            Control::Slider(s) => Some(s),
            _ => None,
        }
    }

    pub fn slider_mut(&mut self, atom: AtomRef) -> Option<&mut Slider> {
        let id = self.atom(atom).region?;
        match &mut self.regions.get_mut(id)?.control {
            Control::Slider(s) => Some(s),
            _ => None,
        }
    }

    /// Sets up a slider from an option callback: range, value, and a value handler; held
    /// clicks are added to its events so dragging updates it.
    pub fn slider_setup(
        &mut self,
        atom: AtomRef,
        max: u32,
        value: u32,
        handler: Option<&'static str>,
    ) {
        let Some(id) = self.atom(atom).region else {
            return;
        };
        if let Some(r) = self.regions.get_mut(id) {
            r.filter |= RPE_HOLD_LEFT;
            if let Control::Slider(s) = &mut r.control {
                s.max = max;
                s.value = value;
                if let Some(h) = handler {
                    s.handler = SliderHandler::Named(h);
                }
            }
        }
    }

    pub fn list(&self, atom: AtomRef) -> Option<&ListWindow> {
        match &self.regions.get(self.atom(atom).region?)?.control {
            Control::List(l) => Some(l),
            _ => None,
        }
    }

    /// Text of a text entry atom.
    pub fn text_entry(&self, atom: AtomRef) -> Option<&str> {
        match &self.regions.get(self.atom(atom).region?)?.control {
            Control::Text(t) => Some(&t.text),
            _ => None,
        }
    }

    /// Whether a text entry has the keyboard (`regKeysFocussed`).
    pub fn keys_focussed(&self) -> bool {
        self.keys_focussed
    }

    pub fn list_mut(&mut self, atom: AtomRef) -> Option<&mut ListWindow> {
        let id = self.atom(atom).region?;
        match &mut self.regions.get_mut(id)?.control {
            Control::List(l) => Some(l),
            _ => None,
        }
    }

    /// `uicListWindowInit` for an atom's list: rows of `item_height` drawn by the named drawer
    /// (or the default text drawer with `font`).
    pub fn list_init(&mut self, atom: AtomRef, font: &str, item_height: i32) {
        let Some(id) = self.atom(atom).region else {
            return;
        };
        let Some(r) = self.regions.get_mut(id) else {
            return;
        };
        let rect = r.rect;
        r.filter |= RPE_DOUBLE_LEFT;
        if let Control::List(l) = &mut r.control {
            l.font = Some(font.to_string());
            l.init(rect, item_height, 0);
        }
        self.list_scroll_adjust(id);
    }

    /// Re-fits a list's scroll bar thumb after items changed.
    pub fn list_scroll_adjust(&mut self, list_id: RegionId) {
        let (bar, upper, visible, total) = match self.regions.get(list_id).map(|r| &r.control) {
            Some(Control::List(l)) => (l.scrollbar, l.upper, l.max_visible, l.items.len()),
            _ => return,
        };
        if let Some(Control::ScrollBar(sb)) = bar
            .and_then(|b| self.regions.get_mut(b))
            .map(|r| &mut r.control)
        {
            sb.adjust(upper, visible, total);
        }
    }

    /// Gives a stand-alone scroll bar (one not belonging to a list window, like the build
    /// manager's `CM_Scroller` thumbwheel) an invisible list of rows to scroll; the caller reads
    /// and sets the position with [`FrontEnd::virtual_scroll`]. Returns the list's region.
    pub fn scrollbar_virtual(&mut self, scrollbar_atom: &str, visible: usize) -> Option<RegionId> {
        let a = self.find_atom(scrollbar_atom)?;
        let bar = self.atom(a).region?;
        let parent = self.regions.get(bar)?.parent?;
        let mut r = Region::new(Rect::default(), 0, Role::Static);
        r.control = Control::List(ListWindow {
            max_visible: visible,
            scrollbar: Some(bar),
            ..Default::default()
        });
        let id = self.regions.alloc(r);
        self.regions.insert_child(id, parent);
        if let Some(Control::ScrollBar(sb)) = self.regions.get_mut(bar).map(|r| &mut r.control) {
            sb.list = Some(id);
        }
        Some(id)
    }

    /// Sets the row count of a virtual scroll list and, when `upper` is given, its first
    /// visible row; returns the first visible row (moved by the scroll bar since last time).
    pub fn virtual_scroll(&mut self, list: RegionId, total: usize, upper: Option<usize>) -> usize {
        let pos = match self.regions.get_mut(list).map(|r| &mut r.control) {
            Some(Control::List(l)) => {
                l.items.resize(
                    total,
                    ListItem {
                        text: String::new(),
                        data: 0,
                        can_select: false,
                        selected: false,
                    },
                );
                if let Some(u) = upper {
                    l.upper = u;
                }
                let last = total.saturating_sub(l.max_visible);
                l.upper = l.upper.min(last);
                l.upper
            }
            _ => return 0,
        };
        self.list_scroll_adjust(list);
        pos
    }

    /// Makes a list window focusable (keyboard goes to it once clicked).
    pub fn list_focusable(&mut self, atom: AtomRef, focus: bool) {
        let Some(r) = self
            .atom(atom)
            .region
            .and_then(|id| self.regions.get_mut(id))
        else {
            return;
        };
        if focus {
            r.filter |= RPE_KEY_DOWN;
        } else {
            r.filter &= !RPE_KEY_DOWN;
        }
        if let Control::List(l) = &mut r.control {
            l.can_have_focus = focus;
        }
    }

    /// Enables or disables an atom's region (`RSF_RegionDisabled`).
    pub fn set_disabled(&mut self, atom: AtomRef, disabled: bool) {
        if let Some(r) = self
            .atom(atom)
            .region
            .and_then(|id| self.regions.get_mut(id))
        {
            if disabled {
                r.status |= RSF_REGION_DISABLED;
            } else {
                r.status &= !RSF_REGION_DISABLED;
            }
        }
    }

    /// Requests a redraw of an atom's region (no-op: every frame is drawn in full).
    pub fn dirty(&mut self, _atom: AtomRef) {}

    /// `feToggleButtonSet`.
    pub fn toggle_set(&mut self, name: &str, pressed: bool) {
        if let Some(a) = self.find_atom(name) {
            let atom = self.atom_mut(a);
            if pressed {
                atom.status |= FAS_CHECKED;
            } else {
                atom.status &= !FAS_CHECKED;
            }
        }
    }

    /// `feRadioButtonSet`: checks the button of the group whose value is `index`.
    pub fn radio_set(&mut self, name: &str, index: u32) {
        let Some(first) = self.find_atom(name) else {
            self.log_once(format!("feRadioButtonSet: no radio group '{name}'"));
            return;
        };
        for a in self.atoms_named(first.screen, name) {
            let atom = self.atom_mut(a);
            if atom.radio_value == index {
                atom.status |= FAS_CHECKED;
            } else {
                atom.status &= !FAS_CHECKED;
            }
        }
    }

    // ---------------------------------------------------------------------------------------
    // Screen stack
    // ---------------------------------------------------------------------------------------

    /// `feScreenStart` with the main region as parent.
    pub fn screen_start(&mut self, name: &str) -> Option<RegionId> {
        let main = self.main;
        self.screen_start_from(main, name)
    }

    /// `feScreenStart(parent, name)`.
    pub fn screen_start_from(&mut self, parent: RegionId, name: &str) -> Option<RegionId> {
        self.tab_stop = 1;
        let Some(screen) = self.find_screen(name) else {
            self.log_once(format!("feScreenStart: can't find screen '{name}'"));
            return None;
        };
        self.keys_focussed = false;
        self.input.stuck.clear();
        let mut parent = parent;
        if !self.screens[screen].atoms[0].has(FAF_POPUP) {
            if self.stack.last().and_then(|e| e.parent) == Some(parent) {
                self.current_screen_delete();
            }
        } else {
            parent = self.current_base();
        }
        if let Some(top) = self.current_screen() {
            if self.screens[top].name == name {
                return Some(self.current_base());
            }
        }
        // The stack entry exists before the regions so that controls see their own screen.
        let placeholder = parent;
        self.stack.push(StackEntry {
            screen: Some(screen),
            base: placeholder,
            parent: Some(parent),
        });
        let base = self.regions_add(parent, screen, false);
        if let Some(e) = self.stack.last_mut() {
            e.base = base;
        }
        if self.stack.len() > 1 {
            let siblings = self
                .regions
                .get(parent)
                .map(|p| p.children.clone())
                .unwrap_or_default();
            for c in siblings {
                if let Some(r) = self.regions.get_mut(c) {
                    r.status &= !RSF_MOUSE_INSIDE;
                }
            }
        }
        let mut buttons = 0;
        for atom in &mut self.screens[screen].atoms {
            if atom.tabstop != 0 && (atom.kind != FA_RADIO_BUTTON || atom.radio_value == 1) {
                buttons += 1;
            }
            atom.status = 0;
        }
        // Call-on-create, last atom first.
        let count = self.screens[screen].atoms.len();
        for index in (1..count).rev() {
            let atom = &self.screens[screen].atoms[index];
            if atom.has(FAF_CALL_ON_CREATE) {
                if let Some(n) = atom.name.clone() {
                    self.function_execute(
                        &n,
                        Some(AtomRef {
                            screen,
                            atom: index,
                        }),
                        true,
                    );
                }
            }
        }
        if buttons > 0 {
            use KeyAction::*;
            for (action, k) in [
                (Tab, key::TAB),
                (Right, key::RIGHT),
                (Left, key::LEFT),
                (Up, key::UP),
                (Down, key::DOWN),
                (Space, key::SPACE),
                (Return, key::RETURN),
                (Escape, key::ESCAPE),
                (Home, key::HOME),
                (End, key::END),
            ] {
                let mut r =
                    Region::new(Rect::default(), RPE_KEY_DOWN, Role::Key { action, buttons });
                r.keys = vec![k];
                let id = self.regions.alloc(r);
                self.regions.insert_child(id, base);
            }
        }
        self.clicked_left = None;
        self.clicked_left_last = None;
        Some(base)
    }

    /// `feCurrentScreenDelete`: runs call-on-delete callbacks and removes the top screen.
    pub fn current_screen_delete(&mut self) {
        self.keys_focussed = false;
        self.input.stuck.clear();
        let Some(&StackEntry {
            screen: Some(screen),
            base,
            ..
        }) = self.stack.last()
        else {
            self.log_once("feCurrentScreenDelete: NULL screen".into());
            return;
        };
        let count = self.screens[screen].atoms.len();
        for index in (1..count).rev() {
            let atom = &self.screens[screen].atoms[index];
            if atom.has(FAF_CALL_ON_DELETE) && atom.kind != FA_USER_REGION {
                if let Some(n) = atom.name.clone() {
                    let r = AtomRef {
                        screen,
                        atom: index,
                    };
                    self.atom_mut(r).status |= FAS_ON_DELETE;
                    self.function_execute(&n, Some(r), false);
                    self.atom_mut(r).status &= !FAS_ON_DELETE;
                }
            }
        }
        self.region_delete(base);
        for atom in &mut self.screens[screen].atoms {
            atom.region = None;
        }
        self.stack.pop();
    }

    /// Deletes every screen on the stack.
    pub fn all_screens_delete(&mut self) {
        while self.stack.len() > 1 {
            self.current_screen_delete();
        }
    }

    /// `feScreenAllHotKeysUpdate` for every screen on the stack (after a language change).
    pub fn hotkeys_update_all(&mut self) {
        let screens: Vec<usize> = self.stack.iter().filter_map(|e| e.screen).collect();
        for s in screens {
            for atom in 1..self.screens[s].atoms.len() {
                let aref = AtomRef { screen: s, atom };
                if let Some(id) = self.atom(aref).region {
                    if self
                        .regions
                        .get(id)
                        .is_some_and(|r| r.role == Role::Control)
                    {
                        self.accelerator_set(id, aref);
                    }
                }
            }
        }
    }

    /// `feRegionsAdd`: builds the region tree of a screen under `parent`.
    fn regions_add(&mut self, parent: RegionId, screen: usize, move_to_front: bool) -> RegionId {
        let a0 = self.screens[screen].atoms[0].clone();
        let mut base = Region::new(a0.rect(), RPE_PRESS_LEFT, Role::Base);
        if move_to_front {
            base.status |= RSF_PRIORITY_REGION;
        }
        if a0.has(FAF_MODAL) {
            base.filter |= RPE_MODAL_BREAK;
        }
        base.draw = Draw::Base;
        base.drawstyle = a0.drawstyle;
        base.atom = Some(AtomRef { screen, atom: 0 });
        let base_id = self.regions.alloc(base);
        self.regions.insert_child(base_id, parent);
        self.regions.move_to_front(base_id);
        self.screens[screen].atoms[0].region = Some(base_id);

        let mut cutouts: Vec<(Rect, RegionId, bool)> = Vec::new();
        let count = self.screens[screen].atoms.len();
        for index in (1..count).rev() {
            let aref = AtomRef {
                screen,
                atom: index,
            };
            let atom = self.screens[screen].atoms[index].clone();
            let hidden = atom.has(FAF_HIDDEN);
            let rect = atom.rect();
            let stack_ok = |k: u8| CAN_STACK.get(k as usize).copied().unwrap_or(false);
            let plain = |fe: &mut Self, draw: Draw, stackable: bool| {
                let mut r = Region::new(rect, 0, Role::Static);
                r.atom = Some(aref);
                r.draw = if hidden { Draw::None } else { draw };
                r.drawstyle = atom.drawstyle;
                r.stackable = stackable;
                let id = fe.regions.alloc(r);
                fe.regions.insert_child(id, base_id);
                fe.atom_mut(aref).region = Some(id);
                id
            };
            match atom.kind {
                FA_USER_REGION => {
                    let known = atom
                        .name
                        .as_deref()
                        .is_some_and(|n| self.draw_callbacks.contains_key(n));
                    if known {
                        plain(self, Draw::User, stack_ok(atom.kind));
                    } else {
                        self.atom_mut(aref).region = None;
                        self.debug_once(format!(
                            "feRegionsAdd: no user region handler for '{}'",
                            atom.name.as_deref().unwrap_or("")
                        ));
                    }
                }
                FA_STATIC_TEXT => {
                    plain(self, Draw::StaticText, stack_ok(atom.kind));
                }
                FA_DECORATIVE_REGION | FA_OPAQUE_DECORATIVE_REGION => {
                    let st = atom.name.as_deref().is_some_and(decorative_stackable);
                    plain(self, Draw::Decorative, st);
                }
                FA_BUTTON | FA_CHECK_BOX | FA_TOGGLE_BUTTON | FA_RADIO_BUTTON
                | FA_BITMAP_BUTTON | FA_HORIZ_SLIDER | FA_VERT_SLIDER | FA_DRAG_BUTTON => {
                    let filter = if atom.kind == FA_DRAG_BUTTON {
                        UIF_DRAG_BUTTON
                    } else {
                        UIF_BUTTON
                    };
                    let mut r = Region::new(rect, filter, Role::Control);
                    r.atom = Some(aref);
                    if atom.has(FAF_DISABLED) {
                        r.status |= RSF_REGION_DISABLED;
                    }
                    r.draw = if hidden {
                        Draw::None
                    } else {
                        match atom.kind {
                            FA_CHECK_BOX => Draw::CheckBox,
                            FA_TOGGLE_BUTTON => Draw::Toggle,
                            FA_RADIO_BUTTON => Draw::Radio,
                            FA_BITMAP_BUTTON => Draw::BitmapButton,
                            FA_HORIZ_SLIDER => Draw::HorizSlider,
                            FA_VERT_SLIDER => Draw::VertSlider,
                            _ => Draw::Button,
                        }
                    };
                    r.control = match atom.kind {
                        FA_HORIZ_SLIDER | FA_VERT_SLIDER => Control::Slider(Slider::default()),
                        _ => Control::Button,
                    };
                    r.stackable = stack_ok(atom.kind);
                    r.tabstop = atom.tabstop;
                    let id = self.regions.alloc(r);
                    self.regions.insert_child(id, base_id);
                    self.atom_mut(aref).region = Some(id);
                    self.accelerator_set(id, aref);
                    if atom.tabstop == 1 {
                        self.set_current(Some(id), false);
                    }
                }
                FA_SCROLL_BAR => {
                    let id = self.scrollbar_alloc(base_id, aref, rect, None);
                    self.atom_mut(aref).region = Some(id);
                }
                FA_TEXT_ENTRY => {
                    let mut r = Region::new(rect, UIF_TEXT_ENTRY, Role::Control);
                    r.atom = Some(aref);
                    if atom.has(FAF_DISABLED) {
                        r.status |= RSF_REGION_DISABLED;
                    }
                    r.draw = if hidden { Draw::None } else { Draw::TextEntry };
                    r.control = Control::Text(TextEntry {
                        max_len: 64,
                        ..Default::default()
                    });
                    r.stackable = stack_ok(atom.kind);
                    r.tabstop = atom.tabstop;
                    let id = self.regions.alloc(r);
                    self.regions.insert_child(id, base_id);
                    self.atom_mut(aref).region = Some(id);
                    if atom.tabstop == 1 {
                        self.set_current(Some(id), false);
                    }
                }
                FA_LIST_WINDOW => {
                    let mut r = Region::new(rect, UIF_SCROLLING, Role::Control);
                    r.atom = Some(aref);
                    if atom.has(FAF_DISABLED) {
                        r.status |= RSF_REGION_DISABLED;
                    }
                    r.draw = if hidden { Draw::None } else { Draw::ListWindow };
                    r.control = Control::List(ListWindow::default());
                    r.tabstop = atom.tabstop;
                    let id = self.regions.alloc(r);
                    self.regions.insert_child(id, base_id);
                    self.atom_mut(aref).region = Some(id);
                    let bar_rect = Rect::from_xywh(
                        rect.x1 + LW_BAR_SPACE,
                        rect.y0,
                        LW_BAR_WIDTH,
                        rect.height(),
                    );
                    let bar = self.scrollbar_alloc(id, aref, bar_rect, Some(id));
                    if let Some(Control::List(l)) = self.regions.get_mut(id).map(|r| &mut r.control)
                    {
                        l.scrollbar = Some(bar);
                    }
                    if atom.tabstop == 1 {
                        self.set_current(Some(id), false);
                    }
                }
                FA_CUTOUT_REGION => {
                    let id = plain(self, Draw::None, false);
                    cutouts.push((rect, id, atom.has(FAF_DONT_CUTOUT_BASE)));
                }
                _ => {
                    plain(self, Draw::StaticRect, stack_ok(atom.kind));
                }
            }
        }
        self.find_cutout_regions(&cutouts, base_id);
        self.find_children(base_id);
        base_id
    }

    /// `uicChildScrollBarAlloc` (vertical): the tab region plus up/down arrow regions.
    fn scrollbar_alloc(
        &mut self,
        parent: RegionId,
        aref: AtomRef,
        rect: Rect,
        list: Option<RegionId>,
    ) -> RegionId {
        let tab_rect = Rect::new(rect.x0, rect.y0 + 20, rect.x1, rect.y1 - 20);
        let filter = UIF_SCROLLING;
        let mut tab = Region::new(tab_rect, filter, Role::Control);
        tab.atom = Some(aref);
        tab.draw = Draw::ScrollBar;
        let tab_id = self.regions.alloc(tab);
        let mut up = Region::new(
            Rect::from_xywh(rect.x0, rect.y0, rect.width(), 20),
            filter,
            Role::ScrollButton {
                scrollbar: tab_id,
                up: true,
            },
        );
        up.draw = Draw::ScrollButton;
        let mut down = Region::new(
            Rect::from_xywh(rect.x0, rect.y1 - 20, rect.width(), 20),
            filter,
            Role::ScrollButton {
                scrollbar: tab_id,
                up: false,
            },
        );
        down.draw = Draw::ScrollButton;
        // The engine allocates the tab first, then the buttons, all under the same parent.
        self.regions.insert_child(tab_id, parent);
        let up_id = self.regions.alloc(up);
        self.regions.insert_child(up_id, parent);
        let down_id = self.regions.alloc(down);
        self.regions.insert_child(down_id, parent);
        let mut thumb_region = tab_rect;
        thumb_region.y0 += 4;
        thumb_region.y1 -= 4;
        let mut sb = ScrollBar {
            thumb_region,
            thumb: Rect::new(rect.x0 + 4, rect.y0 + 20, rect.x1 - 4, rect.y1 - 40),
            up: Some(up_id),
            down: Some(down_id),
            list,
            ..Default::default()
        };
        sb.adjust(0, 0, 0);
        sb.thumb.x0 = rect.x0 + 4;
        if let Some(r) = self.regions.get_mut(tab_id) {
            r.control = Control::ScrollBar(sb);
        }
        tab_id
    }

    /// `feFindCutoutRegion`: records each cutout on the base region and on every earlier
    /// sibling (lower atom index) one of its corners pokes into.
    fn find_cutout_regions(&mut self, cutouts: &[(Rect, RegionId, bool)], base: RegionId) {
        let corners = |c: Rect| [(c.x0, c.y0), (c.x1, c.y1), (c.x1, c.y0), (c.x0, c.y1)];
        let base_rect = self.regions.get(base).map(|r| r.rect).unwrap_or_default();
        let siblings = self
            .regions
            .get(base)
            .map(|r| r.children.clone())
            .unwrap_or_default();
        for &(c, id, dont_cut_base) in cutouts {
            if !dont_cut_base
                && corners(c)
                    .iter()
                    .any(|&(x, y)| base_rect.strictly_contains(x, y))
            {
                if let Some(b) = self.regions.get_mut(base) {
                    b.cutouts.push(Cutout { rect: c });
                }
            }
            let Some(pos) = siblings.iter().position(|&s| s == id) else {
                continue;
            };
            for &other in &siblings[pos + 1..] {
                if let Some(r) = self.regions.get_mut(other) {
                    if corners(c)
                        .iter()
                        .any(|&(x, y)| r.rect.strictly_contains(x, y))
                    {
                        r.cutouts.push(Cutout { rect: c });
                    }
                }
            }
        }
    }

    /// `feFindChildren`: regions lying on top of a stackable region (label on a button,
    /// anything on a decoration) become its children so they draw and hit-test with it.
    fn find_children(&mut self, base: RegionId) {
        'restart: loop {
            let children = self
                .regions
                .get(base)
                .map(|r| r.children.clone())
                .unwrap_or_default();
            for (i, &region) in children.iter().enumerate() {
                let Some(r) = self.regions.get(region) else {
                    continue;
                };
                if !r.stackable || r.atom.is_none() {
                    continue;
                }
                let hay = r.rect;
                for j in (0..i).rev() {
                    let child = children[j];
                    let Some(c) = self.regions.get(child) else {
                        continue;
                    };
                    if region_inside(c.rect, hay) {
                        self.regions.unlink(child);
                        self.regions.insert_child(child, region);
                        continue 'restart;
                    }
                }
            }
            break;
        }
    }

    /// `feAcceleratorSet`: the atom's hot key (with modifiers) for the current language.
    fn accelerator_set(&mut self, id: RegionId, aref: AtomRef) {
        let lang = self.res.language().index();
        let atom = self.atom(aref);
        let mut keys = Vec::new();
        if atom.hotkey_modifiers & HKM_CONTROL != 0 {
            keys.push(key::CONTROL);
        }
        if atom.hotkey_modifiers & HKM_SHIFT != 0 {
            keys.push(key::SHIFT);
        }
        if atom.hotkey_modifiers & HKM_ALT != 0 {
            keys.push(key::ALT);
        }
        let hk = atom.hotkeys[lang];
        if hk != 0 {
            keys.push(u16::from(hk));
        }
        if let Some(r) = self.regions.get_mut(id) {
            if keys.is_empty() {
                r.filter &= !RPE_KEY_DOWN;
            } else {
                r.filter |= RPE_KEY_DOWN;
            }
            r.keys = keys;
        }
    }

    /// `regRegionDelete`: deferred while the region is being processed.
    pub fn region_delete(&mut self, id: RegionId) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        if r.has(RSF_PROCESSING) {
            let has_keys = r.filter & RPM_KEY_EVENT != 0;
            let has_children = !r.children.is_empty();
            if let Some(r) = self.regions.get_mut(id) {
                r.status |= RSF_TO_BE_DELETED;
            }
            if has_keys {
                self.keys_focussed = false;
            }
            if has_children {
                self.regions.disable_recursive(id);
            }
            return;
        }
        let children = r.children.clone();
        for c in children {
            self.region_delete(c);
        }
        self.regions.unlink(id);
        if self.clicked_left == Some(id) {
            self.clicked_left = None;
        }
        self.regions.free(id);
    }

    // ---------------------------------------------------------------------------------------
    // Callbacks
    // ---------------------------------------------------------------------------------------

    /// `feFunctionExecute`.
    pub fn function_execute(&mut self, name: &str, atom: Option<AtomRef>, first_call: bool) {
        let Some(&f) = self.callbacks.get(name) else {
            self.debug_once(format!("feFunctionExecute: function '{name}' not found"));
            return;
        };
        if first_call {
            if let Some(a) = atom {
                self.atom_mut(a).status |= FAS_ON_CREATE;
            }
        }
        f(
            self,
            &Call {
                name: name.to_string(),
                atom,
            },
        );
        if let Some(a) = atom {
            if let Some(atom) = self
                .screens
                .get_mut(a.screen)
                .and_then(|s| s.atoms.get_mut(a.atom))
            {
                atom.status &= !FAS_ON_CREATE;
            }
        }
    }

    /// `feButtonProcess`: a button was clicked; follow its link or run its function.
    fn button_activate(&mut self, aref: AtomRef) {
        let atom = self.atom(aref).clone();
        if atom.has(FAF_LINK) {
            let Some(name) = atom.name else {
                self.log_once("feButtonProcess: NULL link button hit".into());
                return;
            };
            let Some(screen) = self.current_screen() else {
                return;
            };
            let Some(target) = self.screens[screen]
                .link(&name)
                .and_then(|l| l.target.clone())
            else {
                let sname = self.screens[screen].name.clone();
                self.debug_once(format!(
                    "feButtonProcess: no link found in screen '{sname}' for link button '{name}'"
                ));
                return;
            };
            let parent = self
                .stack
                .last()
                .and_then(|e| e.parent)
                .unwrap_or(self.main);
            self.screen_start_from(parent, &target);
        } else if atom.has(FAF_FUNCTION) {
            if let Some(name) = atom.name {
                self.function_execute(&name, Some(aref), false);
            }
        }
    }

    /// Queues a UI sound event (`soundEventPlay(NULL, event)`).
    pub fn ui_sound(&mut self, event: u32) {
        self.sounds.push(event);
    }

    /// The sound `uicToggleProcess` plays when a toggle is pressed: by its current state, or
    /// the race buttons' own sounds when pressed with the mouse.
    fn toggle_sound(&self, aref: AtomRef, mouse: bool) -> u32 {
        let atom = self.atom(aref);
        match atom.name.as_deref() {
            Some("CP_RaceOne") if mouse => sound::RACE_ONE,
            Some("CP_RaceTwo") if mouse => sound::RACE_TWO,
            _ if atom.checked() => sound::TOGGLE_CHECKED,
            _ => sound::TOGGLE_UNCHECKED,
        }
    }

    /// The control's "click" (its `processFunction` with `CM_ButtonClick`).
    fn control_click(&mut self, id: RegionId) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        let Some(aref) = r.atom else { return };
        if let Control::Slider(s) = &r.control {
            if let SliderHandler::Named(h) = s.handler {
                if let Some(&f) = self.slider_handlers.get(h) {
                    f(self, id);
                }
                return;
            }
        }
        self.button_activate(aref);
    }

    // ---------------------------------------------------------------------------------------
    // Focus and keyboard navigation (UIControls)
    // ---------------------------------------------------------------------------------------

    /// First atom of the current screen whose region has `status` set.
    fn find_region_flag(&self, status: u32) -> Option<AtomRef> {
        let screen = self.current_screen()?;
        (1..self.screens[screen].atoms.len())
            .map(|atom| AtomRef { screen, atom })
            .find(|&a| {
                self.atom(a)
                    .region
                    .and_then(|id| self.regions.get(id))
                    .is_some_and(|r| r.has(status))
            })
    }

    /// First atom of the current screen with flag `flag` set.
    fn find_atom_flag(&self, flag: u32) -> Option<AtomRef> {
        let screen = self.current_screen()?;
        (1..self.screens[screen].atoms.len())
            .map(|atom| AtomRef { screen, atom })
            .find(|&a| self.atom(a).has(flag))
    }

    /// The checked (or unchecked) radio button region of `aref`'s group.
    fn radio_region(&self, aref: AtomRef, selected: bool) -> Option<RegionId> {
        let name = self.atom(aref).name.clone()?;
        let screen = self.current_screen()?;
        self.atoms_named(screen, &name)
            .into_iter()
            .find(|&a| self.atom(a).checked() == selected)
            .and_then(|a| self.atom(a).region)
    }

    /// `uicFindTabstop`.
    fn find_tabstop(&self) -> Option<RegionId> {
        let screen = self.current_screen()?;
        let index = self.screens[screen]
            .atoms
            .iter()
            .position(|a| u32::from(a.tabstop) == self.tab_stop)?;
        let aref = AtomRef {
            screen,
            atom: index,
        };
        if self.atom(aref).kind == FA_RADIO_BUTTON {
            self.radio_region(aref, true)
        } else {
            self.atom(aref).region
        }
    }

    /// `uicClearCurrent`: returns whether the cleared control had a tab stop.
    fn clear_current(&mut self) -> bool {
        let Some(aref) = self.find_region_flag(RSF_CURRENT_SELECTED) else {
            return false;
        };
        let Some(id) = self.atom(aref).region else {
            return false;
        };
        let kind = self.atom(aref).kind;
        if kind == FA_RADIO_BUTTON {
            let name = self.atom(aref).name.clone().unwrap_or_default();
            for a in self.atoms_named(aref.screen, &name) {
                if let Some(r) = self.atom(a).region.and_then(|i| self.regions.get_mut(i)) {
                    r.status &= !RSF_CURRENT_SELECTED;
                }
            }
        }
        let Some(r) = self.regions.get_mut(id) else {
            return false;
        };
        r.status &= !RSF_CURRENT_SELECTED;
        let tabstop = r.tabstop;
        let lose_focus = match &mut r.control {
            Control::Text(t) => {
                t.message = CM_LOSE_FOCUS;
                true
            }
            Control::List(l) if l.can_have_focus => {
                l.message = CM_LOSE_FOCUS;
                true
            }
            _ => false,
        };
        if lose_focus {
            r.status &= !RSF_KEY_CAPTURE;
            self.keys_focussed = false;
            if let Some(n) = self.atom(aref).name.clone() {
                self.function_execute(&n, Some(aref), false);
            }
        }
        tabstop != 0
    }

    /// `uicSetCurrent`.
    fn set_current(&mut self, id: Option<RegionId>, _user_input: bool) {
        let Some(id) = id else { return };
        let Some(r) = self.regions.get_mut(id) else {
            return;
        };
        if r.has(RSF_CANT_FOCUS_TO) {
            return;
        }
        if !r.has(RSF_CURRENT_SELECTED) {
            r.status |= RSF_CURRENT_SELECTED;
            if r.tabstop != 0 {
                self.tab_stop = u32::from(r.tabstop);
            }
        }
        let tabstop = r.tabstop;
        let aref = r.atom;
        let capture = match &r.control {
            Control::Text(_) => true,
            Control::List(l) => l.can_have_focus,
            _ => false,
        };
        if capture {
            r.status |= RSF_KEY_CAPTURE;
            self.input.typed.clear();
            self.input.chars.clear();
            self.keys_focussed = true;
        }
        if let Some(aref) = aref {
            if self.atom(aref).kind == FA_RADIO_BUTTON {
                let name = self.atom(aref).name.clone().unwrap_or_default();
                if let Some(screen) = self.current_screen() {
                    for a in self.atoms_named(screen, &name) {
                        if self.atom(a).tabstop != 0 {
                            self.tab_stop = u32::from(tabstop);
                        }
                        if let Some(r) = self.atom(a).region.and_then(|i| self.regions.get_mut(i)) {
                            r.status |= RSF_CURRENT_SELECTED;
                        }
                    }
                }
            }
        }
    }

    fn limit_tabstop(&mut self, buttons: u32) {
        if self.tab_stop > buttons {
            self.tab_stop = 1;
        } else if self.tab_stop == 0 {
            self.tab_stop = buttons;
        }
    }

    fn move_focus(&mut self, delta: i32, buttons: u32) {
        self.clear_current();
        self.tab_stop = (self.tab_stop as i32 + delta).max(0) as u32;
        self.limit_tabstop(buttons);
        let t = self.find_tabstop();
        self.set_current(t, true);
    }

    /// Steps a radio group to the neighbouring value (arrow keys on a radio button).
    fn radio_step(&mut self, delta: i32) -> bool {
        let Some(sel) = self.find_region_flag(RSF_CURRENT_SELECTED) else {
            return false;
        };
        if self.atom(sel).kind != FA_RADIO_BUTTON {
            return false;
        }
        let Some(checked) = self.radio_region(sel, true) else {
            return true;
        };
        let Some(cur) = self.regions.get(checked).and_then(|r| r.atom) else {
            return true;
        };
        let index = self.atom(cur).radio_value as i64 + i64::from(delta);
        if index < 0 {
            return true;
        }
        let name = self.atom(cur).name.clone().unwrap_or_default();
        if let Some(target) = self
            .atoms_named(cur.screen, &name)
            .into_iter()
            .find(|&a| i64::from(self.atom(a).radio_value) == index)
        {
            self.clear_current();
            let tid = self.atom(target).region;
            self.set_current(tid, true);
            if let Some(tid) = tid {
                self.radio_activate(tid);
                self.ui_sound(sound::RADIO);
            }
        }
        true
    }

    /// A radio button was chosen: run its callback, then check it in its group.
    fn radio_activate(&mut self, id: RegionId) {
        self.control_click(id);
        if let Some(aref) = self.regions.get(id).and_then(|r| r.atom) {
            let (name, value) = (self.atom(aref).name.clone(), self.atom(aref).radio_value);
            if let Some(name) = name {
                self.radio_set(&name, value);
            }
        }
    }

    fn key_action(&mut self, action: KeyAction, buttons: u32) {
        use KeyAction::*;
        match action {
            Tab => {
                if self.clear_current() {
                    if self.input.down.contains(&key::SHIFT) {
                        self.tab_stop = self.tab_stop.saturating_sub(1);
                    } else {
                        self.tab_stop += 1;
                    }
                    self.limit_tabstop(buttons);
                }
                let t = self.find_tabstop();
                self.set_current(t, true);
            }
            Right | Left => {
                let delta = if action == Right { 1 } else { -1 };
                if !self.radio_step(delta) {
                    self.move_focus(delta, buttons);
                }
            }
            Up => self.move_focus(-1, buttons),
            Down => self.move_focus(1, buttons),
            Space => self.space_activate(),
            Return => {
                let target = self
                    .find_atom_flag(FAF_DEFAULT_OK)
                    .and_then(|a| self.atom(a).region)
                    .or_else(|| self.find_tabstop());
                if let Some(id) = target {
                    let kind = self
                        .regions
                        .get(id)
                        .and_then(|r| r.atom)
                        .map(|a| self.atom(a).kind);
                    match kind {
                        Some(FA_BUTTON | FA_CHECK_BOX | FA_TOGGLE_BUTTON) => self.control_click(id),
                        Some(FA_TEXT_ENTRY) => {
                            if let Some(r) = self.regions.get_mut(id) {
                                r.status |= RSF_KEY_CAPTURE;
                                if let Control::Text(t) = &mut r.control {
                                    t.message = CM_GAIN_FOCUS;
                                }
                            }
                            self.input.typed.clear();
                            self.keys_focussed = true;
                            self.control_click(id);
                        }
                        _ => {}
                    }
                }
            }
            Escape => {
                if let Some(id) = self
                    .find_atom_flag(FAF_DEFAULT_BACK)
                    .and_then(|a| self.atom(a).region)
                {
                    self.control_click(id);
                    self.ui_sound(sound::CLICK);
                }
            }
            Home => {
                self.clear_current();
                self.tab_stop = 1;
                let t = self.find_tabstop();
                self.set_current(t, true);
            }
            End => {
                self.clear_current();
                self.tab_stop = buttons;
                let t = self.find_tabstop();
                self.set_current(t, true);
            }
        }
    }

    /// `uicSpacebarProcess`: activates the focused control (radio groups advance, wrapping).
    fn space_activate(&mut self) {
        let Some(aref) = self.find_region_flag(RSF_CURRENT_SELECTED) else {
            return;
        };
        let Some(id) = self.atom(aref).region else {
            return;
        };
        match self.atom(aref).kind {
            FA_BUTTON => {
                self.ui_sound(sound::CLICK);
                self.control_click(id);
            }
            FA_CHECK_BOX | FA_TOGGLE_BUTTON => {
                let s = self.toggle_sound(aref, false);
                self.ui_sound(s);
                self.atom_mut(aref).status ^= FAS_CHECKED;
                self.control_click(id);
            }
            FA_RADIO_BUTTON => {
                let Some(checked) = self.radio_region(aref, true) else {
                    return;
                };
                let Some(cur) = self.regions.get(checked).and_then(|r| r.atom) else {
                    return;
                };
                let name = self.atom(cur).name.clone().unwrap_or_default();
                let group = self.atoms_named(cur.screen, &name);
                let next = self.atom(cur).radio_value + 1;
                let target = group
                    .iter()
                    .copied()
                    .find(|&a| self.atom(a).radio_value == next)
                    .or_else(|| {
                        group
                            .iter()
                            .copied()
                            .find(|&a| self.atom(a).radio_value == 0)
                    });
                if let Some(t) = target.and_then(|a| self.atom(a).region) {
                    self.radio_activate(t);
                    self.ui_sound(sound::RADIO);
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------------------
    // Input and processing
    // ---------------------------------------------------------------------------------------

    pub fn mouse_move(&mut self, x: i32, y: i32) {
        self.input.mouse_x = x;
        self.input.mouse_y = y;
    }

    pub fn mouse_button(&mut self, left: bool, down: bool) {
        if left {
            // As the original's window procedure: the second press of a double click is
            // `WM_LBUTTONDBLCLK`, which sets only the double-click key (no left-button press),
            // and the press after it starts a new pair.
            if down && !self.input.left && !self.left_double_held {
                let (x, y) = (self.input.mouse_x, self.input.mouse_y);
                let double = self.last_press.is_some_and(|(t, px, py)| {
                    self.time - t < 0.5 && (px - x).abs() <= 4 && (py - y).abs() <= 4
                });
                if double {
                    self.input.double_left = true;
                    self.left_double_held = true;
                    self.last_press = None;
                    return;
                }
                self.last_press = Some((self.time, x, y));
            }
            if !down {
                self.left_double_held = false;
            }
            self.input.left = down;
        } else {
            self.input.right = down;
        }
    }

    pub fn key_down(&mut self, code: u16) {
        if !self.input.down.contains(&code) {
            self.input.stuck.insert(code);
        }
        self.input.down.insert(code);
        let shift = if self.input.down.contains(&key::SHIFT) {
            key::SHIFT_BIT
        } else {
            0
        };
        self.input.typed.push_back(u32::from(code) | shift);
    }

    pub fn key_up(&mut self, code: u16) {
        self.input.down.remove(&code);
    }

    pub fn char_typed(&mut self, c: char) {
        self.input.chars.push_back(c);
    }

    pub fn wheel(&mut self, up: bool) {
        if up {
            self.input.wheel_up = true;
        } else {
            self.input.wheel_down = true;
        }
    }

    /// Mouse position as the regions see it (clamped while a slider holds the cursor).
    fn mouse(&self) -> (i32, i32) {
        let (x, y) = (self.input.mouse_x, self.input.mouse_y);
        match self.mouse_clip {
            Some(c) => (x.clamp(c.x0, c.x1 - 1), y.clamp(c.y0, c.y1 - 1)),
            None => (x, y),
        }
    }

    /// One pass of the region processor (`regProcessTask`), then frame bookkeeping.
    pub fn process(&mut self, dt: f32) {
        self.time += f64::from(dt);
        let root = self.root;
        self.process_region(root, 0xffff_ffff);
        self.input.wheel_up = false;
        self.input.wheel_down = false;
        self.input.double_left = false;
        // Buffered keys only matter to a focused control in this frame.
        self.input.typed.clear();
        self.input.chars.clear();
    }

    /// `regFunctionCall`: delivers `event` if the region is alive, enabled and filters it in.
    fn call(&mut self, id: RegionId, event: u32, data: u32, mask: u32) -> bool {
        let Some(r) = self.regions.get(id) else {
            return false;
        };
        if r.has(RSF_TO_BE_DELETED | RSF_REGION_DISABLED) || (mask & r.filter & event) != event {
            return false;
        }
        self.dispatch(id, event, data);
        true
    }

    /// `regRegionProcess`: children first, then this region's mouse and key logic.
    fn process_region(&mut self, id: RegionId, mut mask: u32) -> u32 {
        let Some(r) = self.regions.get_mut(id) else {
            return mask;
        };
        r.status |= RSF_PROCESSING;
        let children = r.children.clone();
        for child in children {
            if self.regions.get(child).is_none_or(|c| c.parent != Some(id)) {
                continue;
            }
            mask = self.process_region(child, mask);
            if self
                .regions
                .get(child)
                .is_some_and(|c| c.has(RSF_TO_BE_DELETED))
            {
                self.region_delete(child);
            }
        }
        let Some(r) = self.regions.get(id) else {
            return mask;
        };
        let filter = r.filter;
        if (mask & filter & RPE_MODAL_BREAK) == RPE_MODAL_BREAK {
            mask &= !RPM_ALL_EVENTS;
        }
        let mut ret = mask;
        let (mx, my) = self.mouse();
        if (self.clicked_left.is_none() || self.clicked_left == Some(id))
            && filter & RPM_MOUSE_EVENTS != 0
        {
            let rect = r.rect;
            let status = r.status;
            if rect.contains(mx, my) {
                if self.input.wheel_up {
                    self.input.wheel_up = false;
                    self.call(id, RPE_WHEEL_UP, 0, mask);
                }
                if self.input.wheel_down {
                    self.input.wheel_down = false;
                    self.call(id, RPE_WHEEL_DOWN, 0, mask);
                }
                if (mask & filter & RPE_DOUBLE_LEFT) == RPE_DOUBLE_LEFT && self.input.double_left {
                    if self.clicked_left_last == Some(id) {
                        self.call(id, RPE_DOUBLE_LEFT, 0, mask);
                    }
                    self.input.double_left = false;
                    ret &= !RPE_DOUBLE_LEFT;
                }
                if status & RSF_MOUSE_INSIDE == 0 {
                    if status & RSF_LEFT_PRESSED != 0 {
                        self.call(id, RPE_ENTER_HOLD_LEFT, 0, mask);
                        self.set_status(id, RSF_LEFT_PRESSED, true);
                        self.clicked_left = Some(id);
                        self.clicked_left_last = Some(id);
                    }
                    if status & RSF_RIGHT_PRESSED != 0 {
                        self.call(id, RPE_ENTER_HOLD_RIGHT, 0, mask);
                    }
                    self.call(id, RPE_ENTER, 0, mask);
                    self.set_status(id, RSF_MOUSE_INSIDE, true);
                } else {
                    if self.input.left {
                        if status & RSF_LEFT_PRESSED == 0 {
                            if self.call(id, RPE_PRESS_LEFT, 0, mask) {
                                self.set_status(id, RSF_LEFT_PRESSED, true);
                                self.clicked_left = Some(id);
                                self.clicked_left_last = Some(id);
                            }
                            ret &= !RPM_PRESS_RELEASE;
                        } else {
                            self.call(id, RPE_HOLD_LEFT, 0, mask);
                        }
                    } else if status & RSF_LEFT_PRESSED != 0 {
                        self.call(id, RPE_RELEASE_LEFT, 0, mask);
                        self.clicked_left = None;
                        ret &= !RPM_PRESS_RELEASE;
                        self.set_status(id, RSF_LEFT_PRESSED, false);
                    }
                    if self.input.right {
                        if status & RSF_RIGHT_PRESSED == 0 {
                            if self.call(id, RPE_PRESS_RIGHT, 0, mask) {
                                self.set_status(id, RSF_RIGHT_PRESSED, true);
                            }
                            ret &= !RPM_PRESS_RELEASE;
                        } else {
                            self.call(id, RPE_HOLD_RIGHT, 0, mask);
                        }
                    } else if status & RSF_RIGHT_PRESSED != 0 {
                        self.call(id, RPE_RELEASE_RIGHT, 0, mask);
                        ret &= !RPM_PRESS_RELEASE;
                        self.set_status(id, RSF_RIGHT_PRESSED, false);
                    }
                }
            } else {
                if status & RSF_MOUSE_INSIDE != 0 {
                    self.set_status(id, RSF_MOUSE_INSIDE, false);
                    if status & RSF_LEFT_PRESSED != 0 {
                        self.call(id, RPE_EXIT_HOLD_LEFT, 0, mask);
                    }
                    if status & RSF_RIGHT_PRESSED != 0 {
                        self.call(id, RPE_EXIT_HOLD_RIGHT, 0, mask);
                    }
                    self.call(id, RPE_EXIT, 0, mask);
                }
                if !self.input.left {
                    self.set_status(id, RSF_LEFT_PRESSED, false);
                    self.clicked_left = None;
                }
            }
        }
        // Key logic.
        if let Some(r) = self.regions.get(id) {
            let (status, filter, keys) = (r.status, r.filter, r.keys.clone());
            if status & RSF_KEY_CAPTURE != 0 {
                let typed: Vec<u32> = self.input.typed.drain(..).collect();
                for k in typed {
                    // A focused control eats the key: it must not fire a hot key later.
                    self.input.stuck.remove(&((k & 0xffff) as u16));
                    self.call(id, RPE_KEY_DOWN, k, mask);
                }
            } else if filter & RPM_KEY_EVENT != 0 && !self.keys_focussed {
                if status & RSF_KEY_PRESSED == 0 {
                    if self.input.all_stuck(&keys) {
                        if self.call(id, RPE_KEY_DOWN, 0, mask) {
                            for k in &keys {
                                self.input.stuck.remove(k);
                            }
                            self.set_status(id, RSF_KEY_PRESSED, true);
                        }
                    } else {
                        self.set_status(id, RSF_KEY_PRESSED, false);
                    }
                } else if !self.input.all_down(&keys) {
                    self.call(id, RPE_KEY_UP, 0, mask);
                    self.set_status(id, RSF_KEY_PRESSED, false);
                }
            }
        }
        if let Some(r) = self.regions.get_mut(id) {
            r.status &= !RSF_PROCESSING;
        }
        ret
    }

    fn set_status(&mut self, id: RegionId, bit: u32, on: bool) {
        if let Some(r) = self.regions.get_mut(id) {
            if on {
                r.status |= bit;
            } else {
                r.status &= !bit;
            }
        }
    }

    /// The region's process function.
    fn dispatch(&mut self, id: RegionId, event: u32, data: u32) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        match r.role.clone() {
            Role::Key { action, buttons } => {
                if event == RPE_KEY_DOWN {
                    self.key_action(action, buttons);
                }
            }
            Role::ScrollButton { scrollbar, up } => self.scroll_button_event(scrollbar, up, event),
            Role::Control => {
                let kind = r.atom.map(|a| self.atom(a).kind);
                let control = match &r.control {
                    Control::ScrollBar(_) => 1,
                    Control::List(_) => 2,
                    Control::Text(_) => 3,
                    Control::Slider(_) => 4,
                    _ => 0,
                };
                match (control, kind) {
                    (1, _) => self.scrollbar_event(id, event),
                    (2, _) => self.list_event(id, event, data),
                    (3, _) => self.text_event(id, event, data),
                    (4, _) => self.slider_event(id, event),
                    (_, Some(FA_CHECK_BOX | FA_TOGGLE_BUTTON | FA_BITMAP_BUTTON)) => {
                        self.toggle_event(id, event)
                    }
                    (_, Some(FA_RADIO_BUTTON)) => self.radio_event(id, event),
                    _ => self.button_event(id, event),
                }
            }
            Role::Custom(name) => {
                if let Some(&f) = self.region_functions.get(name) {
                    f(self, id, event);
                }
            }
            Role::Base | Role::Root | Role::Static => {}
        }
    }

    /// `uicButtonProcess`.
    fn button_event(&mut self, id: RegionId, event: u32) {
        match event {
            RPE_PRESS_LEFT => {
                self.clear_current();
                self.set_current(Some(id), false);
                self.ui_sound(sound::CLICK);
            }
            RPE_KEY_DOWN => {
                self.ui_sound(sound::CLICK);
                self.control_click(id);
            }
            RPE_RELEASE_LEFT => self.control_click(id),
            _ => {}
        }
    }

    /// `uicToggleProcess` (check boxes, toggle and bitmap buttons).
    fn toggle_event(&mut self, id: RegionId, event: u32) {
        let aref = self.regions.get(id).and_then(|r| r.atom);
        match event {
            RPE_PRESS_LEFT => {
                self.clear_current();
                self.set_current(Some(id), false);
                if let Some(a) = aref {
                    let s = self.toggle_sound(a, true);
                    self.ui_sound(s);
                }
            }
            RPE_RELEASE_LEFT | RPE_KEY_DOWN => {
                if let Some(a) = aref {
                    if event == RPE_KEY_DOWN {
                        let s = self.toggle_sound(a, false);
                        self.ui_sound(s);
                    }
                    self.atom_mut(a).status ^= FAS_CHECKED;
                }
                self.control_click(id);
            }
            _ => {}
        }
    }

    /// `uicRadioButtonProcess`.
    fn radio_event(&mut self, id: RegionId, event: u32) {
        match event {
            RPE_PRESS_LEFT => {
                self.clear_current();
                self.set_current(Some(id), false);
                self.ui_sound(sound::RADIO);
            }
            RPE_KEY_DOWN => {
                self.radio_activate(id);
                self.ui_sound(sound::RADIO);
            }
            RPE_RELEASE_LEFT => self.radio_activate(id),
            _ => {}
        }
    }

    /// `uicHorizSliderProcess` / `uicVertSliderProcess`.
    fn slider_event(&mut self, id: RegionId, event: u32) {
        match event {
            RPE_RELEASE_LEFT => self.mouse_clip = None,
            RPE_PRESS_LEFT | RPE_HOLD_LEFT => {
                if event == RPE_PRESS_LEFT {
                    self.mouse_clip = self.regions.get(id).map(|r| r.rect);
                }
                let (mx, my) = self.mouse();
                let vertical = self
                    .regions
                    .get(id)
                    .and_then(|r| r.atom)
                    .is_some_and(|a| self.atom(a).kind == FA_VERT_SLIDER);
                let Some(r) = self.regions.get_mut(id) else {
                    return;
                };
                let rect = r.rect;
                if let Control::Slider(s) = &mut r.control {
                    let v = if vertical {
                        (my - rect.y0) as f32 * s.max as f32 / rect.height().max(1) as f32
                    } else {
                        (mx - rect.x0) as f32 * s.max as f32 / rect.width().max(1) as f32
                    };
                    s.value = v.max(0.0) as u32;
                }
                self.control_click(id);
            }
            _ => {}
        }
    }

    /// `uicScrollBarProcess` for the tab region.
    fn scrollbar_event(&mut self, id: RegionId, event: u32) {
        let (_, my) = self.mouse();
        let Some(Control::ScrollBar(sb)) = self.regions.get(id).map(|r| r.control.clone()) else {
            return;
        };
        let Some(list) = sb.list else { return };
        match event {
            RPE_WHEEL_UP => self.list_scroll(list, |l| l.wheel_up()),
            RPE_WHEEL_DOWN => self.list_scroll(list, |l| l.wheel_down()),
            RPE_PRESS_LEFT => {
                let drag = if sb.thumb.contains(self.input.mouse_x, my) {
                    Some(my)
                } else {
                    self.ui_sound(sound::CLICK);
                    None
                };
                if let Some(Control::ScrollBar(s)) =
                    self.regions.get_mut(id).map(|r| &mut r.control)
                {
                    s.drag_y = drag;
                }
            }
            RPE_HOLD_LEFT => {
                if let Some(start) = sb.drag_y {
                    if sb.div_size > 0.0 {
                        let rows = ((my - start) as f32 / sb.div_size) as isize;
                        if rows != 0 {
                            self.list_scroll(list, |l| {
                                let last = l.items.len().saturating_sub(l.max_visible) as isize;
                                l.upper = (l.upper as isize + rows).clamp(0, last.max(0)) as usize;
                            });
                            if let Some(Control::ScrollBar(s)) =
                                self.regions.get_mut(id).map(|r| &mut r.control)
                            {
                                s.drag_y = Some(start + (rows as f32 * s.div_size) as i32);
                            }
                        }
                    }
                }
            }
            RPE_RELEASE_LEFT => {
                if sb.drag_y.is_none() {
                    // Click on the track: page up or down (`SC_Other`).
                    let above = my < sb.thumb.y0;
                    self.list_scroll(list, |l| {
                        let page = l.max_visible;
                        let last = l.items.len().saturating_sub(page);
                        l.upper = if above {
                            l.upper.saturating_sub(page)
                        } else {
                            (l.upper + page).min(last)
                        };
                    });
                }
                if let Some(Control::ScrollBar(s)) =
                    self.regions.get_mut(id).map(|r| &mut r.control)
                {
                    s.drag_y = None;
                }
            }
            _ => {}
        }
    }

    fn scroll_button_event(&mut self, scrollbar: RegionId, up: bool, event: u32) {
        let list = match self.regions.get(scrollbar).map(|r| &r.control) {
            Some(Control::ScrollBar(sb)) => sb.list,
            _ => None,
        };
        let Some(list) = list else { return };
        match event {
            RPE_RELEASE_LEFT => self.list_scroll(list, |l| {
                if up {
                    l.upper = l.upper.saturating_sub(1);
                } else {
                    let last = l.items.len().saturating_sub(l.max_visible);
                    l.upper = (l.upper + 1).min(last);
                }
            }),
            RPE_WHEEL_UP => self.list_scroll(list, |l| l.wheel_up()),
            RPE_WHEEL_DOWN => self.list_scroll(list, |l| l.wheel_down()),
            _ => {}
        }
    }

    fn list_scroll(&mut self, list: RegionId, f: impl FnOnce(&mut ListWindow)) {
        if let Some(Control::List(l)) = self.regions.get_mut(list).map(|r| &mut r.control) {
            f(l);
        }
        self.list_scroll_adjust(list);
    }

    /// `uicListWindowProcess`.
    fn list_event(&mut self, id: RegionId, event: u32, data: u32) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        let (rect, aref) = (r.rect, r.atom);
        let (_, my) = self.mouse();
        let notify = |fe: &mut Self, msg: u32| {
            if let Some(Control::List(l)) = fe.regions.get_mut(id).map(|r| &mut r.control) {
                l.message = msg;
            }
            if let Some(a) = aref {
                if let Some(n) = fe.atom(a).name.clone() {
                    fe.function_execute(&n, Some(a), false);
                }
            }
        };
        match event {
            RPE_KEY_DOWN => {
                let k = (data & 0xffff) as u16;
                if let Some(Control::List(l)) = self.regions.get_mut(id).map(|r| &mut r.control) {
                    if l.key_capture {
                        if !matches!(k, key::SHIFT | key::CONTROL | key::ALT) {
                            l.key_capture = false;
                            l.captured_key = Some(k);
                            notify(self, CM_KEY_PRESSED);
                        }
                        return;
                    }
                }
                let step = match k {
                    key::LEFT | key::UP => Some(-1),
                    key::RIGHT | key::DOWN => Some(1),
                    key::PAGE_UP => Some(-10),
                    key::PAGE_DOWN => Some(10),
                    key::HOME => Some(isize::MIN / 2),
                    key::END => Some(isize::MAX / 2),
                    _ => None,
                };
                if let Some(d) = step {
                    self.list_scroll(id, |l| l.step(d));
                    let has_selection = matches!(
                        self.regions.get(id).map(|r| &r.control),
                        Some(Control::List(l)) if l.selected.is_some()
                    );
                    if has_selection {
                        self.ui_sound(sound::CLICK);
                    }
                    notify(self, CM_NEW_ITEM_SELECTED);
                } else if k == key::ESCAPE {
                    self.key_action(KeyAction::Escape, 0);
                } else if k == key::SPACE || k == key::RETURN {
                    notify(self, CM_ACCEPT_TEXT);
                }
            }
            RPE_DOUBLE_LEFT | RPE_RELEASE_LEFT => {
                let hit = match self.regions.get(id).map(|r| &r.control) {
                    Some(Control::List(l)) if !l.items.is_empty() => l.item_at(rect, my),
                    _ => None,
                };
                if let Some(i) = hit {
                    if let Some(Control::List(l)) = self.regions.get_mut(id).map(|r| &mut r.control)
                    {
                        l.select(i);
                    }
                    let msg = if event == RPE_DOUBLE_LEFT {
                        CM_DOUBLE_CLICK
                    } else {
                        self.ui_sound(sound::CLICK);
                        CM_NEW_ITEM_SELECTED
                    };
                    notify(self, msg);
                }
            }
            RPE_WHEEL_UP => self.list_scroll(id, |l| l.wheel_up()),
            RPE_WHEEL_DOWN => self.list_scroll(id, |l| l.wheel_down()),
            RPE_PRESS_LEFT => {
                let focusable = matches!(self.regions.get(id).map(|r| &r.control), Some(Control::List(l)) if l.can_have_focus);
                if focusable
                    && !self
                        .regions
                        .get(id)
                        .is_some_and(|r| r.has(RSF_CURRENT_SELECTED))
                {
                    self.clear_current();
                    self.set_current(Some(id), false);
                }
            }
            _ => {}
        }
    }

    /// Minimal `uicTextEntryProcess`: click to focus, type, Return accepts, Escape rejects.
    fn text_event(&mut self, id: RegionId, event: u32, data: u32) {
        let aref = self.regions.get(id).and_then(|r| r.atom);
        let notify = |fe: &mut Self, msg: u32| {
            if let Some(Control::Text(t)) = fe.regions.get_mut(id).map(|r| &mut r.control) {
                t.message = msg;
            }
            if let Some(a) = aref {
                if let Some(n) = fe.atom(a).name.clone() {
                    fe.function_execute(&n, Some(a), false);
                }
            }
        };
        match event {
            RPE_PRESS_LEFT => {
                self.clear_current();
                self.set_current(Some(id), true);
                notify(self, CM_GAIN_FOCUS);
                self.ui_sound(sound::CLICK);
            }
            RPE_KEY_DOWN => {
                let k = (data & 0xffff) as u16;
                match k {
                    key::RETURN => {
                        self.set_status(id, RSF_KEY_CAPTURE, false);
                        self.keys_focussed = false;
                        notify(self, CM_ACCEPT_TEXT);
                    }
                    key::ESCAPE => {
                        self.set_status(id, RSF_KEY_CAPTURE, false);
                        self.keys_focussed = false;
                        notify(self, CM_REJECT_TEXT);
                    }
                    key::BACKSPACE => {
                        if let Some(Control::Text(t)) =
                            self.regions.get_mut(id).map(|r| &mut r.control)
                        {
                            if t.cursor > 0 {
                                t.cursor -= 1;
                                t.text.remove(t.cursor);
                            }
                        }
                    }
                    _ => {
                        let chars: Vec<char> = self.input.chars.drain(..).collect();
                        if let Some(Control::Text(t)) =
                            self.regions.get_mut(id).map(|r| &mut r.control)
                        {
                            for c in chars
                                .into_iter()
                                .filter(|c| !c.is_control() && (*c as u32) < 256)
                            {
                                if t.text.chars().count() < t.max_len {
                                    t.text.insert(t.cursor, c);
                                    t.cursor += 1;
                                }
                            }
                        }
                        notify(self, CM_KEY_PRESSED);
                    }
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------------------------------
    // Drawing
    // ---------------------------------------------------------------------------------------

    /// Draws the whole front end: regions are drawn in reverse processing order, so a parent
    /// is drawn before its children and the head of each list ends up on top.
    pub fn render(&mut self, canvas: &mut Canvas) {
        canvas.clear(crate::canvas::BLACK);
        self.render_over(canvas);
    }

    /// Draws the screens over what the canvas already holds (the in-game 3D view).
    pub fn render_over(&mut self, canvas: &mut Canvas) {
        let mut order = Vec::new();
        self.post_order(self.root, &mut order);
        let res = self.res.clone();
        let mut fer = Fer::new(canvas, &res);
        for &id in order.iter().rev() {
            self.draw_region(id, &mut fer);
        }
    }

    fn post_order(&self, id: RegionId, out: &mut Vec<RegionId>) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        for &c in &r.children {
            self.post_order(c, out);
        }
        out.push(id);
    }

    /// Whether a control belongs to a screen other than the one on top (drawn "off").
    fn on_other_screen(&self, r: &Region) -> bool {
        match (self.current_screen(), r.atom) {
            (Some(cur), Some(a)) => cur != a.screen,
            _ => false,
        }
    }

    fn draw_region(&mut self, id: RegionId, fer: &mut Fer) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        let rect = r.rect;
        let status = r.status;
        let aref = r.atom;
        let other = self.on_other_screen(r);
        let inside = status & RSF_MOUSE_INSIDE != 0;
        let pressed = status & RSF_LEFT_PRESSED != 0;
        let current = status & RSF_CURRENT_SELECTED != 0;
        let disabled = status & RSF_REGION_DISABLED != 0;
        match r.draw {
            Draw::None => {}
            Draw::Base => {
                let Some(a) = aref else { return };
                let atom = self.atom(a);
                if atom.has(FAF_CONTENTS_VISIBLE) {
                    beveled_rect(fer.canvas, rect, atom.content_color, 2, 2);
                }
                if atom.has(FAF_BORDER_VISIBLE) {
                    let cut = r.cutouts.clone();
                    let ds = r.drawstyle;
                    let alpha = atom.status & FAS_USE_ALPHA != 0;
                    fer.box_region(rect, ds[0] as i32, ds[1] as i32, &cut, alpha);
                }
            }
            Draw::StaticRect => {
                let Some(a) = aref else { return };
                let atom = self.atom(a);
                if atom.has(FAF_CONTENTS_VISIBLE) {
                    fer.canvas.fill_rect(rect, atom.content_color);
                }
                if atom.has(FAF_BORDER_VISIBLE) {
                    let cut = r.cutouts.clone();
                    let ds = r.drawstyle;
                    let alpha = atom.status & FAS_USE_ALPHA != 0;
                    fer.box_region(rect, ds[0] as i32, ds[1] as i32, &cut, alpha);
                }
            }
            Draw::StaticText => {
                let Some(a) = aref else { return };
                self.draw_static_text(fer, a, rect);
            }
            Draw::Decorative => {
                if let Some(path) = aref.and_then(|a| self.atom(a).name.clone()) {
                    fer.decorative(rect, &path);
                }
            }
            Draw::User => {
                let Some(a) = aref else { return };
                let Some(name) = self.atom(a).name.clone() else {
                    return;
                };
                if let Some(&f) = self.draw_callbacks.get(&name) {
                    f(self, fer, a, rect);
                }
            }
            Draw::Button => {
                let look = if other {
                    ButtonLook::Off
                } else if disabled {
                    ButtonLook::OffDisabled
                } else if pressed {
                    if inside {
                        ButtonLook::OnFocusMouse
                    } else {
                        ButtonLook::OffFocusMouse
                    }
                } else if current {
                    if inside {
                        ButtonLook::OffFocusMouse
                    } else {
                        ButtonLook::OffFocus
                    }
                } else if inside {
                    ButtonLook::OffMouse
                } else {
                    ButtonLook::Off
                };
                fer.button(rect, look);
            }
            Draw::Toggle => {
                let checked = aref.is_some_and(|a| self.atom(a).checked());
                use ButtonLook::*;
                let look = if other {
                    ToggleOff
                } else if disabled {
                    if checked {
                        ToggleOnDisabled
                    } else {
                        ToggleOffDisabled
                    }
                } else {
                    let (focus, focus_mouse, mouse, plain) = if checked {
                        (ToggleOnFocus, ToggleOnFocusMouse, ToggleOnMouse, ToggleOn)
                    } else {
                        (
                            ToggleOffFocus,
                            ToggleOffFocusMouse,
                            ToggleOffMouse,
                            ToggleOff,
                        )
                    };
                    if pressed || current {
                        if inside {
                            focus_mouse
                        } else {
                            focus
                        }
                    } else if inside {
                        mouse
                    } else {
                        plain
                    }
                };
                fer.button(rect, look);
            }
            Draw::CheckBox => {
                let checked = aref.is_some_and(|a| self.atom(a).checked());
                let look = if other {
                    if checked {
                        MarkLook::OnDisabled
                    } else {
                        MarkLook::OffDisabled
                    }
                } else {
                    mark_look(checked, disabled, pressed || current, inside)
                };
                fer.checkbox(rect, look);
            }
            Draw::Radio => {
                let checked = aref.is_some_and(|a| self.atom(a).checked());
                let look = if other {
                    if checked {
                        MarkLook::On
                    } else {
                        MarkLook::Off
                    }
                } else {
                    mark_look(checked, disabled, pressed || current, inside)
                };
                fer.radio(rect, look);
            }
            Draw::BitmapButton => {
                let Some(a) = aref else { return };
                let checked = self.atom(a).checked();
                if other {
                    fer.radio(rect, if checked { MarkLook::On } else { MarkLook::Off });
                    return;
                }
                let look = match (checked, inside) {
                    (true, true) => BitmapLook::OnMouse,
                    (true, false) => BitmapLook::On,
                    (false, true) => BitmapLook::OffMouse,
                    (false, false) => BitmapLook::Off,
                };
                if let Some(base) = self.atom(a).bitmap.clone() {
                    fer.bitmap_button(rect, &base, look);
                }
            }
            Draw::HorizSlider | Draw::VertSlider => {
                let (value, max) = match &r.control {
                    Control::Slider(s) => (s.value, s.max),
                    _ => (0, 100),
                };
                let look = if disabled && r.draw == Draw::HorizSlider {
                    SliderLook::Disabled
                } else if inside {
                    if pressed {
                        SliderLook::OnMouse
                    } else {
                        SliderLook::OffMouse
                    }
                } else {
                    SliderLook::Off
                };
                if r.draw == Draw::HorizSlider {
                    fer.horiz_slider(rect, value, max, look);
                } else {
                    fer.vert_slider(rect, value, max, look);
                }
            }
            Draw::ScrollBar => {
                let Control::ScrollBar(sb) = &r.control else {
                    return;
                };
                let look = if other {
                    SliderLook::Off
                } else if pressed {
                    if inside {
                        SliderLook::OnMouse
                    } else {
                        SliderLook::OffMouse
                    }
                } else if inside {
                    SliderLook::OffMouse
                } else {
                    SliderLook::Off
                };
                fer.scrollbar(rect, sb.thumb, look);
            }
            Draw::ScrollButton => {
                let Role::ScrollButton { up, .. } = r.role else {
                    return;
                };
                let look = if pressed {
                    if inside {
                        SliderLook::OnMouse
                    } else {
                        SliderLook::OffMouse
                    }
                } else if inside {
                    SliderLook::OffMouse
                } else {
                    SliderLook::Off
                };
                fer.scroll_button(rect, up, look);
            }
            Draw::ListWindow => self.draw_list(fer, id),
            Draw::TextEntry => {
                let look = if other {
                    ButtonLook::TextOff
                } else if disabled {
                    ButtonLook::TextOffDisabled
                } else if pressed {
                    if inside {
                        ButtonLook::TextOnFocusMouse
                    } else {
                        ButtonLook::TextOffFocusMouse
                    }
                } else if current {
                    if inside {
                        ButtonLook::TextOffFocusMouse
                    } else {
                        ButtonLook::TextOffFocus
                    }
                } else if inside {
                    ButtonLook::TextOffMouse
                } else {
                    ButtonLook::TextOff
                };
                fer.button(rect, look);
                if let (Control::Text(t), Some(a)) = (&r.control, aref) {
                    let colour = self.atom(a).border_color;
                    let font = self
                        .atom(a)
                        .font
                        .clone()
                        .unwrap_or_else(|| "HW_EuroseCond_11.hff".into());
                    if let Some(font) = fer.res.font(&font) {
                        let tr = Rect::new(rect.x0 + 6, rect.y0, rect.x1 - 6, rect.y1);
                        let y = (tr.height() - text::height(&font)) / 2 + tr.y0;
                        let bytes = text::to_latin1(&t.text);
                        text::print(
                            fer.canvas,
                            &font,
                            tr.x0,
                            y,
                            colour,
                            &bytes,
                            (Shadow::None, [0; 4]),
                        );
                        if status & RSF_KEY_CAPTURE != 0 {
                            let cx = tr.x0
                                + text::width(
                                    &font,
                                    &text::to_latin1(&t.text[..t.cursor.min(t.text.len())]),
                                );
                            fer.canvas
                                .line(cx - 1, y + 2, cx - 1, y + text::height(&font), colour);
                        }
                    }
                }
            }
        }
    }

    /// `feStaticTextDraw`: the label in the current language.
    fn draw_static_text(&self, fer: &mut Fer, a: AtomRef, rect: Rect) {
        let atom = self.atom(a);
        let lang = fer.res.language().index();
        let text = atom.texts.get(lang).or(atom.text.as_ref());
        let (Some(text), Some(font_name)) = (text, &atom.font) else {
            return;
        };
        let Some(font) = fer.res.font(font_name) else {
            return;
        };
        let x = match atom.flags & hwc_formats::fib::ATOM_JUSTIFY_MASK {
            0x10000 => rect.x0,
            0x20000 => rect.x1 - text::width(&font, text),
            0x30000 => (atom.width - text::width(&font, text)) / 2 + rect.x0,
            _ => rect.x0,
        };
        let shadow =
            Shadow::from_bits(((atom.flags & hwc_formats::fib::ATOM_DROP_SHADOW_MASK) >> 20) as u8);
        text::print(
            fer.canvas,
            &font,
            x,
            rect.y0,
            atom.content_color,
            text,
            (shadow, atom.border_color),
        );
    }

    /// `uicListWindowDraw`.
    fn draw_list(&self, fer: &mut Fer, id: RegionId) {
        let Some(r) = self.regions.get(id) else {
            return;
        };
        let Control::List(l) = &r.control else { return };
        let full = r.rect;
        let inner = Rect::new(
            full.x0 + LIST_MARGIN,
            full.y0 + LIST_MARGIN,
            full.x1 - LIST_MARGIN,
            full.y1 - LIST_MARGIN,
        );
        fer.canvas.fill_rect(full, colour::BACKGROUND);
        fer.focus_window(full, r.has(RSF_CURRENT_SELECTED));
        fer.canvas.fill_rect(inner, colour::BACKGROUND);
        let drawer = r
            .atom
            .and_then(|a| self.atom(a).name.clone())
            .and_then(|n| self.list_drawers.get(&n).copied());
        let mut row = Rect::new(
            inner.x0,
            full.y0 + l.title_height + LIST_MARGIN,
            inner.x1,
            0,
        );
        row.y1 = row.y0 + l.item_height;
        for item in l.items.iter().skip(l.upper).take(l.max_visible) {
            match drawer {
                Some(f) => f(self, fer, row, item),
                None => default_item_draw(fer, l.font.as_deref(), row, item),
            }
            if item.selected {
                let mut s = row;
                s.x0 -= 2;
                s.x1 += 1;
                s.y1 -= 1;
                fer.canvas.blend_rect(s, colour::LIST_ITEM_TRANSLUCENT);
                fer.canvas
                    .outline_rect(s, colour::LIST_ITEM_TRANSLUCENT_BORDER);
            }
            row.y0 += l.item_height;
            row.y1 = row.y0 + l.item_height;
        }
    }
}

/// Every language's strings of every text atom, per screen and atom (`None` for atoms without
/// text). Reads the `.fib` layout directly: the parser keeps only the first string.
fn label_strings(data: &[u8]) -> Vec<Vec<Option<Vec<Vec<u8>>>>> {
    let u16_at = |at: usize| {
        data.get(at..at + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |at: usize| {
        data.get(at..at + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let Some(count) = u16_at(10) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for s in 0..count {
        let sh = 12 + 20 * s;
        let (Some(n_atoms), Some(atoms)) = (u16_at(sh + 10), u32_at(sh + 16)) else {
            break;
        };
        let mut labels = Vec::new();
        for a in 0..n_atoms {
            let at = atoms + 76 * a;
            let kind = data.get(at + 12).copied().unwrap_or(0);
            let ptr = u32_at(at + 40).unwrap_or(0);
            let font = u32_at(at + 44).unwrap_or(0);
            if ptr == 0 || kind == FA_RADIO_BUTTON {
                labels.push(None);
                continue;
            }
            // Strings run from the data pointer up to the font name (or the end of the data).
            let end = if font > ptr {
                font.min(data.len())
            } else {
                data.len().min(ptr + 1024)
            };
            let block = data.get(ptr..end).unwrap_or_default();
            let mut texts: Vec<Vec<u8>> = Vec::new();
            for part in block.split(|&b| b == 0) {
                if part.is_empty() {
                    break;
                }
                texts.push(part.to_vec());
                if texts.len() == 5 {
                    break;
                }
            }
            labels.push(Some(texts));
        }
        out.push(labels);
    }
    out
}

/// Default list row: the item text in the list font, highlighted when selected.
fn default_item_draw(fer: &mut Fer, font: Option<&str>, row: Rect, item: &ListItem) {
    let Some(font) = font.and_then(|f| fer.res.font(f)) else {
        return;
    };
    // A row that cannot be picked is greyed (the game's lists all draw their own; only the
    // resolution list of a borderless window has one).
    let colour = if item.selected {
        colour::LIST_ITEM_SELECTED
    } else if !item.can_select {
        colour::LIST_ITEM_INACTIVE
    } else {
        colour::LIST_ITEM_STANDARD
    };
    text::print(
        fer.canvas,
        &font,
        row.x0,
        row.y0,
        colour,
        &text::to_latin1(&item.text),
        (Shadow::None, [0; 4]),
    );
}

/// Check box / radio look from its state (`uicCheckBoxDraw`, `uicRadioButtonDraw`).
fn mark_look(checked: bool, disabled: bool, focus: bool, inside: bool) -> MarkLook {
    use MarkLook::*;
    match (checked, disabled, focus, inside) {
        (true, true, ..) => OnDisabled,
        (false, true, ..) => OffDisabled,
        (true, false, true, true) => OnFocusMouse,
        (true, false, true, false) => OnFocus,
        (true, false, false, true) => OnMouse,
        (true, false, false, false) => On,
        (false, false, true, true) => OffFocusMouse,
        (false, false, true, false) => OffFocus,
        (false, false, false, true) => OffMouse,
        (false, false, false, false) => Off,
    }
}

/// `feResRescaleBackground`: centre, then stretch to every screen edge the atom touched on the
/// 640x480 layout.
fn rescale_background(atom: &mut FeAtom, size: (i32, i32)) {
    let (dx, dy) = ((size.0 - FE_WIDTH) / 2, (size.1 - FE_HEIGHT) / 2);
    let (x0, y0) = (dx, dy);
    let (x1, y1) = (dx + FE_WIDTH - 1, dy + FE_HEIGHT - 1);
    atom.x += dx;
    atom.y += dy;
    if atom.y <= y0 {
        atom.height += atom.y;
        atom.y = 0;
    }
    if atom.x <= x0 {
        atom.width += atom.x;
        atom.x = 0;
    }
    if atom.y + atom.height >= y1 {
        atom.height = size.1 - 1 - atom.y;
    }
    if atom.x + atom.width >= x1 {
        atom.width = size.0 - 1 - atom.x;
    }
}

/// `primBeveledRectSolid`: a filled octagon with `xb` x `yb` corner bevels.
fn beveled_rect(canvas: &mut Canvas, r: Rect, c: Color, xb: i32, yb: i32) {
    for y in r.y0..r.y1 {
        let d = if y < r.y0 + yb {
            (r.y0 + yb - y) * xb / yb.max(1)
        } else if y >= r.y1 - yb {
            (y - (r.y1 - yb) + 1) * xb / yb.max(1)
        } else {
            0
        };
        canvas.fill_rect(Rect::new(r.x0 + d, y, r.x1 - d, y + 1), c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_rescale_matches_engine() {
        let mut a = FeAtom {
            name: None,
            flags: FAF_BACKGROUND,
            status: 0,
            kind: 0,
            border_color: [0; 4],
            content_color: [0; 4],
            x: -1,
            y: -7,
            width: 644,
            height: 76,
            loaded: (-1, -7, 644, 76),
            text: None,
            texts: Vec::new(),
            font: None,
            radio_value: 0,
            bitmap: None,
            hotkey_modifiers: 0,
            hotkeys: [0; 5],
            tabstop: 0,
            drawstyle: [0; 2],
            region: None,
        };
        rescale_background(&mut a, (1024, 768));
        // Touches the top and both sides: stretched to the screen edges.
        assert_eq!((a.x, a.y), (0, 0));
        assert_eq!(a.width, 1023);
        assert_eq!(a.height, 144 - 7 + 76);
        let mut b = a.clone();
        b.x = 100;
        b.y = 100;
        b.width = 50;
        b.height = 50;
        rescale_background(&mut b, (640, 480));
        assert_eq!((b.x, b.y, b.width, b.height), (100, 100, 50, 50));
    }

    #[test]
    fn decorative_quirk_mask() {
        assert!(decorative_stackable(
            "Feman\\TexDecorative\\a_main_title_mid.lif"
        ));
        assert!(decorative_stackable("FeMan\\Textures\\x.lif"));
        // Character 0 and type-sized values index the real table.
        assert!(!decorative_stackable("short"));
    }
}
