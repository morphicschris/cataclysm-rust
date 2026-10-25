//! The game's mouse pointer (`mouse.c` as Cataclysm has it): the pointer bitmaps of
//! `CursorBitmaps\`, which one is shown for the held modifier keys and for what is under the
//! pointer (`mouseSetCursorSetting` over the action classifier `mouseCursorActionClassify`),
//! and the band-box colour of each modifier (`mrSelectingDraw`).
//!
//! The pointer is drawn with its top-left corner on the mouse position for every bitmap: the
//! original has no per-cursor hot spot.
//!
//! The classifier works on two command-availability tables (`selGetCommandAvailability`): one
//! for the selection and one for the object under the pointer. The game fills them from the
//! simulation; this module only holds the decisions.

use std::rc::Rc;

use crate::assets::Resources;
use crate::canvas::{BlitMode, Canvas, Color, Image};
use crate::ingame::keys::{self, Action};

/// Pointer shapes, numbered like the original's cursor types (`mouseCursorType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CursorKind {
    Normal = 0,
    RotateCamera = 1,
    ZoomCamera = 2,
    AddShips = 3,
    FocusNoSelect = 4,
    Movement = 5,
    Attack = 6,
    ForceAttack = 7,
    Guard = 8,
    SpecialAttack = 9,
    SpecialOpActivate = 10,
    SpecialOpDeactivate = 11,
    Gui = 12,
    Resource = 13,
    Derelict = 14,
    Docking = 15,
    SmallDocking = 16,
    Research = 17,
    Sensors = 18,
    Build = 19,
    Salvage = 20,
    Support = 21,
    ForceField = 22,
    Traders = 23,
    SlipgateSmall = 27,
    SlipgateMedium = 28,
    SlipgateLarge = 29,
    Leech = 30,
    Link = 31,
    Mimic = 32,
    Emp = 33,
    Swarm = 34,
    FireMissile = 35,
    Ramming = 36,
    BeastWeapon = 37,
    Objectives = 38,
}

/// The bitmap of each cursor type (`mouseStartup` / the `mouseDraw` switch). `None` for the
/// camera cursors (nothing is drawn) and the types without a bitmap (a triangle is drawn).
pub fn bitmap_of(kind: CursorKind) -> Option<&'static str> {
    use CursorKind as K;
    Some(match kind {
        K::RotateCamera | K::ZoomCamera | K::Derelict => return None,
        K::Normal | K::Movement => "CursorBitmaps\\cursor.LiF",
        K::AddShips => "CursorBitmaps\\add.LiF",
        K::FocusNoSelect => "CursorBitmaps\\eye.LiF",
        K::Attack => "CursorBitmaps\\attack.LiF",
        K::ForceAttack => "CursorBitmaps\\exclaim.LiF",
        K::Guard => "CursorBitmaps\\shield.LiF",
        K::SpecialAttack | K::SpecialOpActivate => "CursorBitmaps\\special_activate.LiF",
        K::SpecialOpDeactivate => "CursorBitmaps\\special_deactivate.LiF",
        K::Gui | K::Research | K::Sensors | K::Build => "CursorBitmaps\\dialog.LiF",
        K::Resource => "CursorBitmaps\\resource.LiF",
        K::Docking | K::SmallDocking => "CursorBitmaps\\dock.LiF",
        K::Salvage => "CursorBitmaps\\salvage.LiF",
        K::Support => "CursorBitmaps\\support.LiF",
        K::ForceField => "CursorBitmaps\\force_field.LiF",
        K::Traders => "CursorBitmaps\\trader.LiF",
        K::SlipgateSmall => "CursorBitmaps\\slipgate_small.LiF",
        K::SlipgateMedium => "CursorBitmaps\\slipgate_medium.LiF",
        K::SlipgateLarge => "CursorBitmaps\\slipgate_large.LiF",
        K::Leech => "CursorBitmaps\\leech.LiF",
        K::Link => "CursorBitmaps\\link.LiF",
        K::Mimic => "CursorBitmaps\\mimic.LiF",
        K::Emp => "CursorBitmaps\\emp.LiF",
        K::Swarm => "CursorBitmaps\\swarm.LiF",
        K::FireMissile => "CursorBitmaps\\fire_missile.LiF",
        K::Ramming => "CursorBitmaps\\ramming.LiF",
        K::BeastWeapon => "CursorBitmaps\\beast_weapon.LiF",
        K::Objectives => "CursorBitmaps\\objectives.LiF",
    })
}

/// What the classifier decided the right button would do on the object under the pointer
/// (`mouseCursorActionClassify` results 0..17).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PointerAction {
    None = 0,
    Harvest = 1,
    Salvage = 2,
    Attack = 3,
    Repair = 4,
    ForceField = 5,
    SlipgateSmall = 6,
    SlipgateMedium = 7,
    SlipgateLarge = 8,
    Dock = 9,
    Leech = 10,
    Link = 11,
    Mimic = 12,
    Emp = 13,
    Swarm = 14,
    Ramming = 15,
    FireMissile = 16,
    BeastWeapon = 17,
}

impl PointerAction {
    /// The pointer the action shows (`mouseSetCursorSetting`'s mapping).
    pub fn cursor(self) -> Option<CursorKind> {
        use CursorKind as K;
        use PointerAction as A;
        Some(match self {
            A::None => return None,
            A::Harvest => K::Resource,
            A::Salvage => K::Salvage,
            A::Attack => K::Attack,
            A::Repair => K::Support,
            A::ForceField => K::ForceField,
            A::SlipgateSmall => K::SlipgateSmall,
            A::SlipgateMedium => K::SlipgateMedium,
            A::SlipgateLarge => K::SlipgateLarge,
            A::Dock => K::SmallDocking,
            A::Leech => K::Leech,
            A::Link => K::Link,
            A::Mimic => K::Mimic,
            A::Emp => K::Emp,
            A::Swarm => K::Swarm,
            A::Ramming => K::Ramming,
            A::FireMissile => K::FireMissile,
            A::BeastWeapon => K::BeastWeapon,
        })
    }
}

/// Indices of the 41-entry command-availability table (`selGetCommandAvailability`). For a
/// selection an entry says some ship can do it; the `ALL_*` entries say every ship can. For
/// the object under the pointer the same function describes what it is.
pub mod avail {
    /// A harvester that may collect (workers).
    pub const HARVEST: usize = 0;
    /// Salvage capability (upgrade 0x80000).
    pub const SALVAGE: usize = 1;
    /// Repair capability (upgrade 0x40000, not the mobile refineries).
    pub const REPAIR: usize = 2;
    /// Armed ships able to attack.
    pub const ATTACK: usize = 3;
    /// A ship of a player not allied with the local one.
    pub const ENEMY: usize = 4;
    /// A ship of the local player or an ally.
    pub const ALLY: usize = 5;
    /// A resource (or a ship that looks like one).
    pub const RESOURCE: usize = 6;
    /// Something the selection can salvage.
    pub const SALVAGEABLE: usize = 7;
    pub const SLIPGATE_SMALL: usize = 8;
    pub const SLIPGATE_MEDIUM: usize = 9;
    pub const SLIPGATE_LARGE: usize = 10;
    pub const HIVE: usize = 0xb;
    pub const HIVE_LAUNCHING: usize = 0xc;
    pub const EMP: usize = 0xd;
    pub const LEECH: usize = 0xe;
    pub const LEECH_ACTIVE: usize = 0xf;
    pub const BIG_GUN: usize = 0x10;
    pub const BIG_GUN_CHARGING: usize = 0x11;
    pub const LINK: usize = 0x12;
    pub const LINK_HOST: usize = 0x13;
    pub const BEAST_CASCADE: usize = 0x14;
    pub const RAMMING: usize = 0x15;
    pub const MIMIC: usize = 0x16;
    /// The target is of the type Mimics copy from (0x17).
    pub const MIMIC_TYPE: usize = 0x17;
    pub const MIMIC_DISGUISED: usize = 0x18;
    pub const FORCE_FIELD: usize = 0x19;
    pub const MISSILES: usize = 0x1a;
    pub const MISSILES_ATTACKING: usize = 0x1b;
    pub const CLOAK: usize = 0x1c;
    pub const CLOAKED: usize = 0x1d;
    pub const LINK_MISSILES: usize = 0x1f;
    pub const ALL_LEECH: usize = 0x20;
    pub const ALL_EMP: usize = 0x21;
    pub const ALL_MIMIC: usize = 0x22;
    pub const ALL_HIVE: usize = 0x23;
    pub const ALL_RAMMING: usize = 0x24;
    pub const ALL_BEAST_CASCADE: usize = 0x25;
    pub const ALL_LINK_MISSILES: usize = 0x26;
    pub const ALL_MISSILES: usize = 0x27;
    pub const LEECH_STATE: usize = 0x28;
    pub const LEN: usize = 0x29;
}

/// One command-availability table.
pub type Availability = [bool; avail::LEN];

/// What the classifier needs to know about the object under the pointer beyond its table:
/// the questions the original asks the simulation directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TargetFacts {
    /// A ship of the local player (`objtype == ship && owner == curPlayer`).
    pub own_ship: bool,
    /// Every selected ship can dock with it, it accepts docking now.
    pub all_can_dock: bool,
    /// It carries the link upgrade and every selected ship does too, all of its type.
    pub link_with_all: bool,
    /// The Beast can infect it (`beastGetInfectionStats`).
    pub infectable: bool,
    /// The selection's Mimics can take its shape.
    pub mimic_shape: bool,
}

/// `mouseCursorActionClassify`: the right-button action on the object under the pointer.
/// `classic_csa` is the Homeworld context-sensitive-actions option (`Homeworld_CSA`), which
/// turns the special attacks into plain attacks.
pub fn classify(
    sel: &Availability,
    tgt: &Availability,
    selected: usize,
    facts: &TargetFacts,
    classic_csa: bool,
) -> PointerAction {
    use avail::*;
    use PointerAction as A;
    let special = |a: PointerAction| if classic_csa { A::Attack } else { a };
    if tgt[RESOURCE] {
        if sel[HARVEST] && !sel[ATTACK] {
            return A::Harvest;
        }
    } else if tgt[ENEMY] {
        if sel[ALL_LEECH] {
            return A::Leech;
        }
        if sel[ALL_EMP] {
            return special(A::Emp);
        }
        if sel[ALL_HIVE] {
            return special(A::Swarm);
        }
        if sel[ALL_RAMMING] {
            return special(A::Ramming);
        }
        if sel[ALL_LINK_MISSILES] {
            return special(A::FireMissile);
        }
        if sel[ALL_BEAST_CASCADE] && facts.infectable {
            return special(A::BeastWeapon);
        }
    } else if tgt[ALLY] {
        if sel[REPAIR] && !sel[ATTACK] {
            return A::Repair;
        }
        if sel[ALL_MISSILES] && !tgt[MISSILES] {
            return A::ForceField;
        }
    } else if tgt[SLIPGATE_SMALL] {
        if sel[ALLY] {
            return A::SlipgateSmall;
        }
    } else if tgt[SLIPGATE_MEDIUM] {
        if sel[ALLY] {
            return A::SlipgateMedium;
        }
    } else if tgt[SLIPGATE_LARGE] {
        if sel[ALLY] {
            return A::SlipgateLarge;
        }
    } else if tgt[SALVAGEABLE] && sel[SALVAGE] {
        return A::Salvage;
    }
    if tgt[ENEMY] && sel[ATTACK] {
        return A::Attack;
    }
    if tgt[SALVAGEABLE] && !sel[ATTACK] && sel[SALVAGE] {
        return A::Salvage;
    }
    let mut out = A::None;
    if facts.own_ship && selected > 0 {
        if facts.all_can_dock {
            out = A::Dock;
        }
        if facts.link_with_all {
            return A::Link;
        }
        if out != A::None {
            return out;
        }
    }
    if sel[ALL_MIMIC] && facts.mimic_shape {
        return A::Mimic;
    }
    out
}

/// What decides the pointer this frame (`mouseSetCursorSetting` inputs).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointerState {
    /// The player may not give orders now (`mrDisabled`).
    pub disabled: bool,
    /// The objectives button was clicked (`moGetAndClearClick`).
    pub objectives_click: bool,
    /// Modifier actions held.
    pub force_attack: bool,
    pub multiple_select: bool,
    pub guard: bool,
    pub ship_special: bool,
    pub focus: bool,
    pub attack: bool,
    /// The right-button action the classifier found for the object under the pointer, when
    /// it ran (a context-sensitive scheme is on, an object not in the selection is under the
    /// pointer, no band box is being dragged, the sensors manager is closed).
    pub action: Option<PointerAction>,
    /// A front-end screen is up, the pie plate is up, or the menu is open (`mouseDraw`
    /// forces the plain pointer, the focus eye excepted while the plate is up).
    pub screen_up: bool,
    pub menu_open: bool,
    pub move_plate: bool,
    /// The camera is being turned with the right button (the pointer is hidden).
    pub camera_motion: bool,
}

impl PointerState {
    /// Reads the modifier actions from the key bindings.
    pub fn with_keys(mut self, held: impl Fn(Action) -> bool) -> Self {
        self.force_attack = held(keys::BANDBOX_FORCE_ATTACK);
        self.multiple_select = held(keys::MULTIPLE_SELECT);
        self.guard = held(keys::BANDBOX_GUARD);
        self.ship_special = held(keys::SHIP_SPECIAL);
        self.focus = held(keys::BANDBOX_FOCUS);
        self.attack = held(keys::BANDBOX_ATTACK);
        self
    }
}

/// `mouseSetCursorSetting`: the pointer for the modifiers held and the object under it.
pub fn select(s: &PointerState) -> CursorKind {
    use CursorKind as K;
    if s.disabled {
        return K::Normal;
    }
    if s.objectives_click {
        return K::Objectives;
    }
    if s.force_attack {
        return K::ForceAttack;
    }
    if s.multiple_select {
        return K::AddShips;
    }
    if s.guard {
        return K::Guard;
    }
    if s.ship_special {
        return K::SpecialAttack;
    }
    if s.focus {
        return K::FocusNoSelect;
    }
    if s.attack {
        return K::Attack;
    }
    s.action
        .and_then(PointerAction::cursor)
        .unwrap_or(K::Normal)
}

/// Band-box colour for the modifiers held (`mrSelectingDraw`).
pub fn band_box_color(held: impl Fn(Action) -> bool) -> Color {
    if held(keys::BANDBOX_GUARD) {
        [0, 0, 250, 255]
    } else if held(keys::BANDBOX_FOCUS) {
        [100, 100, 100, 255]
    } else if held(keys::BANDBOX_ATTACK) {
        [255, 0, 0, 255]
    } else if held(keys::BANDBOX_FORCE_ATTACK) {
        [250, 100, 100, 255]
    } else if held(keys::SHIP_SPECIAL) {
        [75, 100, 100, 255]
    } else {
        [255, 255, 255, 255]
    }
}

/// How long the special-attack pointer stays after the key goes up (`MC_MOUSEHOLDTIME`).
const HOLD_TIME: f32 = 1.0;
/// The fallback pointer's colours (`mouseDrawTriangle`).
const TRIANGLE_FILL: Color = [0, 0, 0, 255];
const TRIANGLE_EDGE: Color = [255, 255, 255, 255];

/// The loaded pointer bitmaps and the drawing state (`mouseDraw`).
pub struct Pointer {
    res: Resources,
    images: Vec<(CursorKind, Option<Rc<Image>>)>,
    /// The special-attack pointer held on (`mousehold`), and until when.
    hold: Option<f32>,
    pub kind: CursorKind,
    pub visible: bool,
}

impl Pointer {
    pub fn new(res: Resources) -> Pointer {
        Pointer {
            res,
            images: Vec::new(),
            hold: None,
            kind: CursorKind::Normal,
            visible: true,
        }
    }

    fn image(&mut self, kind: CursorKind) -> Option<Rc<Image>> {
        if let Some((_, img)) = self.images.iter().find(|(k, _)| *k == kind) {
            return img.clone();
        }
        let img = bitmap_of(kind).and_then(|p| self.res.image(p));
        self.images.push((kind, img.clone()));
        img
    }

    /// Picks this frame's pointer: `select` with the `mouseDraw` overrides and the
    /// special-attack hold. `now` is the game time.
    pub fn update(&mut self, s: &PointerState, now: f32) -> CursorKind {
        let mut kind = select(s);
        if kind == CursorKind::Normal {
            if let Some(until) = self.hold {
                if until >= now {
                    kind = CursorKind::SpecialAttack;
                } else {
                    self.hold = None;
                }
            }
        }
        if s.screen_up || (s.move_plate && kind != CursorKind::FocusNoSelect) || s.menu_open {
            kind = CursorKind::Normal;
        }
        if kind == CursorKind::SpecialAttack && self.hold.is_none() {
            self.hold = Some(now + HOLD_TIME);
        }
        self.visible = !s.camera_motion;
        self.kind = kind;
        kind
    }

    /// Draws the pointer with its top-left corner at the mouse position.
    pub fn draw(&mut self, canvas: &mut Canvas, x: i32, y: i32) {
        if !self.visible || matches!(self.kind, CursorKind::RotateCamera | CursorKind::ZoomCamera) {
            return;
        }
        canvas.set_clip(None);
        match self.image(self.kind) {
            Some(img) => canvas.blit(&img, x, y, BlitMode::Blend),
            None => draw_triangle(canvas, x, y),
        }
    }
}

/// `mouseDrawTriangle`: the pointer drawn when a bitmap is missing.
fn draw_triangle(canvas: &mut Canvas, x: i32, y: i32) {
    for row in 0..16 {
        let w = (row * 7 / 16).max(0);
        for col in 0..=w {
            canvas.put(x + col, y + row, TRIANGLE_FILL);
        }
    }
    canvas.line(x, y, x + 7, y + 14, TRIANGLE_EDGE);
    canvas.line(x, y, x, y + 16, TRIANGLE_EDGE);
    canvas.line(x, y + 16, x + 7, y + 14, TRIANGLE_EDGE);
}

#[cfg(test)]
mod tests {
    use super::avail::*;
    use super::*;

    fn table(set: &[usize]) -> Availability {
        let mut t = [false; LEN];
        for &i in set {
            t[i] = true;
        }
        t
    }

    #[test]
    fn modifiers_win_in_the_original_order() {
        let s = PointerState {
            attack: true,
            focus: true,
            ..Default::default()
        };
        assert_eq!(select(&s), CursorKind::FocusNoSelect);
        let s = PointerState {
            attack: true,
            action: Some(PointerAction::Dock),
            ..Default::default()
        };
        assert_eq!(select(&s), CursorKind::Attack);
        let s = PointerState {
            force_attack: true,
            multiple_select: true,
            ..Default::default()
        };
        assert_eq!(select(&s), CursorKind::ForceAttack);
        let s = PointerState {
            action: Some(PointerAction::Harvest),
            ..Default::default()
        };
        assert_eq!(select(&s), CursorKind::Resource);
        assert_eq!(select(&PointerState::default()), CursorKind::Normal);
    }

    #[test]
    fn armed_ships_over_an_enemy_attack() {
        let sel = table(&[ATTACK, ALLY]);
        let enemy = table(&[ENEMY]);
        let f = TargetFacts::default();
        assert_eq!(classify(&sel, &enemy, 3, &f, false), PointerAction::Attack);
        // Unarmed ships over an enemy: nothing.
        let workers = table(&[HARVEST, ALLY]);
        assert_eq!(
            classify(&workers, &enemy, 3, &f, false),
            PointerAction::None
        );
        // Workers over a resource harvest; armed ships do not.
        let rock = table(&[RESOURCE]);
        assert_eq!(
            classify(&workers, &rock, 3, &f, false),
            PointerAction::Harvest
        );
        assert_eq!(classify(&sel, &rock, 3, &f, false), PointerAction::None);
    }

    #[test]
    fn docking_and_the_classic_scheme() {
        let sel = table(&[ATTACK, ALLY]);
        let own = table(&[ALLY]);
        let f = TargetFacts {
            own_ship: true,
            all_can_dock: true,
            ..Default::default()
        };
        assert_eq!(classify(&sel, &own, 2, &f, false), PointerAction::Dock);
        assert_eq!(PointerAction::Dock.cursor(), Some(CursorKind::SmallDocking));
        let rammers = table(&[ATTACK, ALL_RAMMING, RAMMING]);
        let enemy = table(&[ENEMY]);
        let none = TargetFacts::default();
        assert_eq!(
            classify(&rammers, &enemy, 1, &none, false),
            PointerAction::Ramming
        );
        assert_eq!(
            classify(&rammers, &enemy, 1, &none, true),
            PointerAction::Attack
        );
    }

    #[test]
    fn band_box_colours_follow_the_modifier() {
        assert_eq!(
            band_box_color(|a| a == keys::BANDBOX_ATTACK),
            [255, 0, 0, 255]
        );
        assert_eq!(
            band_box_color(|a| a == keys::BANDBOX_FOCUS),
            [100, 100, 100, 255]
        );
        assert_eq!(band_box_color(|_| false), [255, 255, 255, 255]);
        // Guard is checked first.
        assert_eq!(
            band_box_color(|a| a == keys::BANDBOX_GUARD || a == keys::BANDBOX_ATTACK),
            [0, 0, 250, 255]
        );
    }

    #[test]
    fn the_plate_and_screens_force_the_plain_pointer() {
        let res = Resources::new(Rc::new(NoFiles));
        let mut p = Pointer::new(res);
        let attack = PointerState {
            attack: true,
            move_plate: true,
            ..Default::default()
        };
        assert_eq!(p.update(&attack, 0.0), CursorKind::Normal);
        let focus = PointerState {
            focus: true,
            move_plate: true,
            ..Default::default()
        };
        assert_eq!(p.update(&focus, 0.0), CursorKind::FocusNoSelect);
        // The special-attack pointer stays a second after the key goes up.
        let special = PointerState {
            ship_special: true,
            ..Default::default()
        };
        assert_eq!(p.update(&special, 10.0), CursorKind::SpecialAttack);
        assert_eq!(
            p.update(&PointerState::default(), 10.5),
            CursorKind::SpecialAttack
        );
        assert_eq!(p.update(&PointerState::default(), 11.5), CursorKind::Normal);
        // Every cursor type that has a bitmap names one under CursorBitmaps.
        assert!(bitmap_of(CursorKind::Attack)
            .unwrap()
            .starts_with("CursorBitmaps\\"));
        assert_eq!(bitmap_of(CursorKind::RotateCamera), None);
    }

    struct NoFiles;
    impl crate::assets::AssetSource for NoFiles {
        fn read(&self, _name: &str) -> Option<Vec<u8>> {
            None
        }
    }
}
