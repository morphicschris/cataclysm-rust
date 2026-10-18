//! Flag words of the region and front-end managers. Values match the engine's so that `.fib`
//! data and recovered behaviour can be compared directly.

// --- Region status (`RSF_*`) ------------------------------------------------------------------
pub const RSF_REALLY_DIRTY: u32 = 0x0001;
pub const RSF_CANT_FOCUS_TO: u32 = 0x0002;
pub const RSF_REGION_DISABLED: u32 = 0x0004;
pub const RSF_DRAW_THIS_FRAME: u32 = 0x0008;
pub const RSF_KEY_CAPTURE: u32 = 0x0020;
pub const RSF_TO_BE_DELETED: u32 = 0x0040;
pub const RSF_PROCESSING: u32 = 0x0080;
pub const RSF_PRIORITY_REGION: u32 = 0x0100;
pub const RSF_CURRENT_SELECTED: u32 = 0x0200;
pub const RSF_MOUSE_INSIDE: u32 = 0x0800;
pub const RSF_LEFT_PRESSED: u32 = 0x1000;
pub const RSF_RIGHT_PRESSED: u32 = 0x2000;
pub const RSF_KEY_PRESSED: u32 = 0x8000;

// --- Region events / filter bits (`RPE_*`) -----------------------------------------------------
pub const RPE_ENTER: u32 = 0x0000_0001;
pub const RPE_EXIT: u32 = 0x0000_0002;
pub const RPE_ENTER_HOLD_LEFT: u32 = 0x0000_0004;
pub const RPE_ENTER_HOLD_RIGHT: u32 = 0x0000_0008;
pub const RPE_EXIT_HOLD_LEFT: u32 = 0x0000_0020;
pub const RPE_EXIT_HOLD_RIGHT: u32 = 0x0000_0040;
pub const RPE_WHEEL_UP: u32 = 0x0000_0100;
pub const RPE_WHEEL_DOWN: u32 = 0x0000_0200;
pub const RPE_HOLD_LEFT: u32 = 0x0000_0400;
pub const RPE_HOLD_RIGHT: u32 = 0x0000_0800;
pub const RPE_PRESS_LEFT: u32 = 0x0000_2000;
pub const RPE_PRESS_RIGHT: u32 = 0x0000_4000;
pub const RPE_RELEASE_LEFT: u32 = 0x0001_0000;
pub const RPE_RELEASE_RIGHT: u32 = 0x0002_0000;
pub const RPM_PRESS_RELEASE: u32 = 0x0007_e000;
pub const RPM_MOUSE_EVENTS: u32 = 0x0007_ffff;
pub const RPE_KEY_DOWN: u32 = 0x0008_0000;
pub const RPE_KEY_UP: u32 = 0x0010_0000;
pub const RPE_KEY_REPEAT: u32 = 0x0020_0000;
pub const RPE_KEY_HOLD: u32 = 0x0040_0000;
pub const RPM_KEY_EVENT: u32 = 0x0078_0000;
pub const RPE_DOUBLE_LEFT: u32 = 0x0080_0000;
pub const RPE_DOUBLE_RIGHT: u32 = 0x0100_0000;
pub const RPM_ALL_EVENTS: u32 = 0x01ff_ffff;
pub const RPE_MODAL_BREAK: u32 = 0x4000_0000;

/// Filter of every button-like control (`UIF_Button`).
pub const UIF_BUTTON: u32 = RPE_ENTER_HOLD_LEFT
    | RPE_EXIT_HOLD_LEFT
    | RPE_PRESS_LEFT
    | RPE_RELEASE_LEFT
    | RPE_ENTER
    | RPE_EXIT;
/// List windows and scroll bars also take the wheel and held clicks.
pub const UIF_SCROLLING: u32 =
    UIF_BUTTON | RPE_WHEEL_DOWN | RPE_WHEEL_UP | RPE_HOLD_LEFT | RPE_ENTER_HOLD_LEFT;
pub const UIF_TEXT_ENTRY: u32 = UIF_BUTTON | RPE_KEY_DOWN;
pub const UIF_DRAG_BUTTON: u32 = UIF_BUTTON | RPE_HOLD_LEFT;

// --- Atom flags (`FAF_*`, mirrored from hwc_formats::fib) ---------------------------------------
pub use hwc_formats::fib::{
    ATOM_ALWAYS_ON_TOP as FAF_ALWAYS_ON_TOP, ATOM_BACKGROUND as FAF_BACKGROUND,
    ATOM_BITMAP as FAF_BITMAP, ATOM_BORDER_VISIBLE as FAF_BORDER_VISIBLE,
    ATOM_CALL_ON_CREATE as FAF_CALL_ON_CREATE, ATOM_CALL_ON_DELETE as FAF_CALL_ON_DELETE,
    ATOM_CONTENTS_VISIBLE as FAF_CONTENTS_VISIBLE, ATOM_DEFAULT_BACK as FAF_DEFAULT_BACK,
    ATOM_DEFAULT_OK as FAF_DEFAULT_OK, ATOM_DISABLED as FAF_DISABLED,
    ATOM_DONT_CUTOUT_BASE as FAF_DONT_CUTOUT_BASE, ATOM_DRAGGABLE as FAF_DRAGGABLE,
    ATOM_FUNCTION as FAF_FUNCTION, ATOM_HIDDEN as FAF_HIDDEN, ATOM_LINK as FAF_LINK,
    ATOM_MODAL as FAF_MODAL, ATOM_POPUP as FAF_POPUP,
};

// --- Atom runtime status (`FAS_*`) -----------------------------------------------------------
pub const FAS_USE_ALPHA: u32 = 0x1;
pub const FAS_CHECKED: u32 = 0x2;
pub const FAS_ON_CREATE: u32 = 0x4;
pub const FAS_ON_DELETE: u32 = 0x8;

// --- Atom types (`FA_*`) -----------------------------------------------------------------------
pub const FA_USER_REGION: u8 = 1;
pub const FA_STATIC_TEXT: u8 = 2;
pub const FA_BUTTON: u8 = 3;
pub const FA_CHECK_BOX: u8 = 4;
pub const FA_TOGGLE_BUTTON: u8 = 5;
pub const FA_SCROLL_BAR: u8 = 6;
pub const FA_TEXT_ENTRY: u8 = 8;
pub const FA_MENU_ITEM: u8 = 11;
pub const FA_RADIO_BUTTON: u8 = 12;
pub const FA_CUTOUT_REGION: u8 = 13;
pub const FA_DECORATIVE_REGION: u8 = 14;
pub const FA_LIST_WINDOW: u8 = 16;
pub const FA_BITMAP_BUTTON: u8 = 17;
pub const FA_HORIZ_SLIDER: u8 = 18;
pub const FA_VERT_SLIDER: u8 = 19;
pub const FA_DRAG_BUTTON: u8 = 20;
pub const FA_OPAQUE_DECORATIVE_REGION: u8 = 21;

// --- Control messages passed to callbacks (`CM_*`) ----------------------------------------------
pub const CM_GAIN_FOCUS: u32 = 1;
pub const CM_LOSE_FOCUS: u32 = 2;
pub const CM_ACCEPT_TEXT: u32 = 3;
pub const CM_REJECT_TEXT: u32 = 4;
pub const CM_KEY_PRESSED: u32 = 5;
pub const CM_NEW_ITEM_SELECTED: u32 = 6;
pub const CM_DOUBLE_CLICK: u32 = 8;

// --- Hot-key modifiers (`HKM_*`) ---------------------------------------------------------------
pub const HKM_CONTROL: u8 = 0x01;
pub const HKM_ALT: u8 = 0x02;
pub const HKM_SHIFT: u8 = 0x04;

/// Key codes. Letters and digits are their upper-case ASCII codes, the rest follow the Windows
/// virtual-key numbering the original keyboard layer uses (and that `.fib` hot keys store).
pub mod key {
    pub const BACKSPACE: u16 = 0x08;
    pub const TAB: u16 = 0x09;
    pub const RETURN: u16 = 0x0d;
    pub const SHIFT: u16 = 0x10;
    pub const CONTROL: u16 = 0x11;
    pub const ALT: u16 = 0x12;
    pub const ESCAPE: u16 = 0x1b;
    pub const SPACE: u16 = 0x20;
    pub const PAGE_UP: u16 = 0x21;
    pub const PAGE_DOWN: u16 = 0x22;
    pub const END: u16 = 0x23;
    pub const HOME: u16 = 0x24;
    pub const LEFT: u16 = 0x25;
    pub const UP: u16 = 0x26;
    pub const RIGHT: u16 = 0x27;
    pub const DOWN: u16 = 0x28;
    pub const DELETE: u16 = 0x2e;
    /// Flag OR-ed into a buffered key when shift was held.
    pub const SHIFT_BIT: u32 = 0x1_0000;
}

/// Colours from `fecolour.script` (`FEC_*`).
pub mod colour {
    use crate::canvas::Color;
    pub const BACKGROUND: Color = [0, 35, 79, 255];
    pub const LIST_ITEM_TRANSLUCENT: Color = [192, 160, 100, 63];
    pub const LIST_ITEM_TRANSLUCENT_BORDER: Color = [120, 70, 0, 255];
    pub const LIST_ITEM_STANDARD: Color = [10, 150, 210, 255];
    pub const LIST_ITEM_SELECTED: Color = [70, 190, 220, 255];
    pub const LIST_ITEM_INACTIVE: Color = [50, 100, 140, 255];
}
