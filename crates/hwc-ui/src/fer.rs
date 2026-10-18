//! Textured front-end drawing ("fer" = front-end registry): framed boxes with cut-outs, buttons,
//! check boxes, radio buttons, sliders, scroll bars, list frames and decorative bitmaps, built
//! from the `.lif` pieces in `FeMan\Textures\`.
//!
//! Every routine keeps the engine's placement rules, including its habit of anchoring a bitmap
//! by its *bottom*-left corner (`draw(x, y, tex)` covers rows `y - height .. y`) and the tiling
//! quirks of its line routine.

use crate::assets::Resources;
use crate::canvas::{BlitMode, Canvas, Image, Rect};
use std::rc::Rc;

/// A frame-piece texture slot: an index into the engine's fixed table of front-end bitmaps
/// (`FeMan\Textures\*.lif`). Groups are contiguous, so states are reached by offsets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Tex(pub u16);

impl Tex {
    pub const BOX_OUTER_CORNER: Tex = Tex(0);
    pub const BOX_INNER_CORNER: Tex = Tex(1);
    pub const BOX_OUTGLOW_STRAIGHT: Tex = Tex(2);
    pub const BOX_INGLOW_STRAIGHT: Tex = Tex(3);
    pub const BOX_SMALL_OUTER_CORNER: Tex = Tex(4);
    pub const BOX_SMALL_INNER_CORNER: Tex = Tex(5);
    pub const BOX_SMALL_OUTGLOW_STRAIGHT: Tex = Tex(6);
    pub const BOX_SMALL_INGLOW_STRAIGHT: Tex = Tex(7);
    pub const BOX_LARGE_OUTER_CORNER: Tex = Tex(8);
    pub const BOX_LARGE_INNER_CORNER: Tex = Tex(9);
    pub const BOX_LARGE_OUTGLOW_STRAIGHT: Tex = Tex(10);
    pub const BOX_LARGE_INGLOW_STRAIGHT: Tex = Tex(11);
    pub const BOX_NOGLOW_CORNER: Tex = Tex(12);
    pub const BOX_NOGLOW_SMALL_CORNER: Tex = Tex(13);
    pub const BOX_NOGLOW_LARGE_CORNER: Tex = Tex(14);
    pub const BOX_NOGLOW_STRAIGHT: Tex = Tex(15);
    /// First of 6 button states x (left cap, middle, right cap).
    pub const BUTTON_OFF_LEFT: Tex = Tex(16);
    /// First of 6 text-box states x 3 pieces.
    pub const TEXT_OFF_LEFT: Tex = Tex(34);
    /// First of 10 toggle states x 3 pieces (5 "on", then 5 "off").
    pub const TOGGLE_ON_LEFT: Tex = Tex(52);
    /// 10 check box states (5 on, 5 off).
    pub const CHECK_ON: Tex = Tex(82);
    /// 10 radio button states.
    pub const RADIO_ON: Tex = Tex(92);
    pub const VERT_SB_TOP: Tex = Tex(102);
    pub const VERT_SB_MID: Tex = Tex(103);
    pub const VERT_SB_BOTTOM: Tex = Tex(104);
    /// Thumb pieces: off (top, mid, bottom), mouse-over, pressed.
    pub const VERT_ST_OFF_TOP: Tex = Tex(105);
    /// Arrow buttons: off up/down, mouse up/down, pressed up/down.
    pub const VERT_SA_OFF_UP: Tex = Tex(114);
    /// List frame: NW, NE, SE, SW corners, N, E, S, W sides, middle; then the focused set.
    pub const LIST_NW_CORNER: Tex = Tex(120);
    pub const LIST_FOCUS_NW_CORNER: Tex = Tex(129);
    pub const VOLUME_BUTTON: Tex = Tex(159);
    /// Slider bars: disabled, off, mouse, on, each (left cap, middle, right cap).
    pub const VOLUME_DISABLED_LEFTCAP: Tex = Tex(160);
    pub const EQ_BAR_BUTTON: Tex = Tex(172);
    /// Equaliser bar pieces: straight (mouse, on, off), middle, bottom cap, top cap.
    pub const EQ_BAR_MOUSE_STRAIGHT: Tex = Tex(173);

    pub fn path(self) -> String {
        format!("FeMan\\Textures\\{}.lif", TEX_FILES[self.0 as usize])
    }

    pub fn offset(self, n: u16) -> Tex {
        Tex(self.0 + n)
    }
}

/// File name (in `FeMan\Textures\`) of each slot, same order as [`Tex`].
const TEX_FILES: [&str; 185] = [
    "medium_convex",
    "medium_concave",
    "medium_convex_straight",
    "medium_concave_straight",
    "small_convex",
    "small_concave",
    "small_convex_straight",
    "small_concave_straight",
    "large_convex",
    "large_concave",
    "large_convex_straight",
    "large_concave_straight",
    "medium_corner",
    "small_corner",
    "large_corner",
    "thin_straight",
    "button_off_left_endcap",
    "button_off_mid",
    "button_off_right_endcap",
    "button_off_focus_left_endcap",
    "button_off_focus_mid",
    "button_off_focus_right_endcap",
    "button_off_focus_mouse_left_endcap",
    "button_off_focus_mouse_mid",
    "button_off_focus_mouse_right_endcap",
    "button_off_mouse_left_endcap",
    "button_off_mouse_mid",
    "button_off_mouse_right_endcap",
    "button_off_disabled_left_endcap",
    "button_off_disabled_mid",
    "button_off_disabled_right_endcap",
    "button_on_focus_mouse_left_endcap",
    "button_on_focus_mouse_mid",
    "button_on_focus_mouse_right_endcap",
    "textbox_off_left_endcap",
    "textbox_off_mid",
    "textbox_off_right_endcap",
    "textbox_off_focus_left_endcap",
    "textbox_off_focus_mid",
    "textbox_off_focus_right_endcap",
    "textbox_off_focus_mouse_left_endcap",
    "textbox_off_focus_mouse_mid",
    "textbox_off_focus_mouse_right_endcap",
    "textbox_off_mouse_left_endcap",
    "textbox_off_mouse_mid",
    "textbox_off_mouse_right_endcap",
    "textbox_disabled_left_endcap",
    "textbox_disabled_mid",
    "textbox_disabled_right_endcap",
    "textbox_on_focus_mouse_left_endcap",
    "textbox_on_focus_mouse_mid",
    "textbox_on_focus_mouse_right_endcap",
    "toggle_on_left_endcap",
    "toggle_on_mid",
    "toggle_on_right_endcap",
    "toggle_on_focus_left_endcap",
    "toggle_on_focus_mid",
    "toggle_on_focus_right_endcap",
    "toggle_on_mouse_left_endcap",
    "toggle_on_mouse_mid",
    "toggle_on_mouse_right_endcap",
    "toggle_on_focus_mouse_left_endcap",
    "toggle_on_focus_mouse_mid",
    "toggle_on_focus_mouse_right_endcap",
    "toggle_disabled_on_left_endcap",
    "toggle_disabled_on_mid",
    "toggle_disabled_on_right_endcap",
    "toggle_off_left_endcap",
    "toggle_off_mid",
    "toggle_off_right_endcap",
    "toggle_off_focus_left_endcap",
    "toggle_off_focus_mid",
    "toggle_off_focus_right_endcap",
    "toggle_off_mouse_left_endcap",
    "toggle_off_mouse_mid",
    "toggle_off_mouse_right_endcap",
    "toggle_off_focus_mouse_left_endcap",
    "toggle_off_focus_mouse_mid",
    "toggle_off_focus_mouse_right_endcap",
    "toggle_disabled_off_left_endcap",
    "toggle_disabled_off_mid",
    "toggle_disabled_off_right_endcap",
    "checkbox_on",
    "checkbox_on_focus",
    "checkbox_on_mouse",
    "checkbox_on_focus_mouse",
    "checkbox_disabled_on",
    "checkbox_off",
    "checkbox_off_focus",
    "checkbox_off_mouse",
    "checkbox_off_focus_mouse",
    "checkbox_disabled_off",
    "radio_on",
    "radio_on_focus",
    "radio_on_mouse",
    "radio_on_focus_mouse",
    "radio_disabled_on",
    "radio_off",
    "radio_off_focus",
    "radio_off_mouse",
    "radio_off_focus_mouse",
    "radio_disabled_off",
    "vscrollbar_topcap",
    "vscrollbar_mid",
    "vscrollbar_botcap",
    "vscrolltab_off_topcap",
    "vscrolltab_off_mid",
    "vscrolltab_off_botcap",
    "vscrolltab_mouse_topcap",
    "vscrolltab_mouse_mid",
    "vscrolltab_mouse_botcap",
    "vscrolltab_on_topcap",
    "vscrolltab_on_mid",
    "vscrolltab_on_botcap",
    "vscrollbar_off_up",
    "vscrollbar_off_down",
    "vscrollbar_mouse_up",
    "vscrollbar_mouse_down",
    "vscrollbar_on_up",
    "vscrollbar_on_down",
    "info_nw_corner",
    "info_ne_corner",
    "info_se_corner",
    "info_sw_corner",
    "info_n_straight",
    "info_e_straight",
    "info_s_straight",
    "info_w_straight",
    "info_mid",
    "info_focus_nw_corner",
    "info_focus_ne_corner",
    "info_focus_se_corner",
    "info_focus_sw_corner",
    "info_focus_n_straight",
    "info_focus_e_straight",
    "info_focus_s_straight",
    "info_focus_w_straight",
    "info_focus_mid",
    "menu_mouseover_mid",
    "menu_mouseover_leftcap",
    "menu_mouseover_rightcap",
    "menu_selected",
    "menu_arrow",
    "arrow_left_off",
    "arrow_left_mouse",
    "arrow_left_on",
    "arrow_right_off",
    "arrow_right_mouse",
    "arrow_right_on",
    "build_arrow_left_off",
    "build_arrow_left_mouse",
    "build_arrow_left_on",
    "build_arrow_right_off",
    "build_arrow_right_mouse",
    "build_arrow_right_on",
    "shipview_lights_off",
    "shipview_lights_top",
    "shipview_lights_bottom",
    "shipview_lights_both",
    "volume_button",
    "volume_disabled_leftcap",
    "volume_disabled_mid",
    "volume_disabled_rightcap",
    "volume_off_leftcap",
    "volume_off_mid",
    "volume_off_rightcap",
    "volume_mouse_leftcap",
    "volume_mouse_mid",
    "volume_mouse_rightcap",
    "volume_on_leftcap",
    "volume_on_mid",
    "volume_on_rightcap",
    "eq_bar_button",
    "eq_bar_mouse_straight",
    "eq_bar_on_straight",
    "eq_bar_off_straight",
    "eq_bar_mouse_middle",
    "eq_bar_on_middle",
    "eq_bar_off_middle",
    "eq_bar_mouse_botcap",
    "eq_bar_on_botcap",
    "eq_bar_off_botcap",
    "eq_bar_mouse_topcap",
    "eq_bar_on_topcap",
    "eq_bar_off_topcap",
];

/// Orientation a frame piece is registered with. Corner pieces are drawn for the top-left
/// corner, side pieces for the left side; the other orientations are quarter-turn rotations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Orient {
    None,
    Top,
    Right,
    Left,
    Bottom,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl Orient {
    fn quarters(self) -> u32 {
        match self {
            Orient::None | Orient::Left | Orient::TopLeft => 0,
            Orient::Top | Orient::TopRight => 1,
            Orient::Right | Orient::BottomRight => 2,
            Orient::Bottom | Orient::BottomLeft => 3,
        }
    }
}

/// Button / text box / toggle textures (`ferbuttonstate`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonLook {
    Off,
    OffFocus,
    OffFocusMouse,
    OffMouse,
    OffDisabled,
    OnFocusMouse,
    TextOff,
    TextOffFocus,
    TextOffFocusMouse,
    TextOffMouse,
    TextOffDisabled,
    TextOnFocusMouse,
    ToggleOn,
    ToggleOnFocus,
    ToggleOnMouse,
    ToggleOnFocusMouse,
    ToggleOnDisabled,
    ToggleOff,
    ToggleOffFocus,
    ToggleOffMouse,
    ToggleOffFocusMouse,
    ToggleOffDisabled,
}

impl ButtonLook {
    /// Left-cap slot; the middle and right cap follow it.
    fn left(self) -> Tex {
        let i = self as u16;
        if i < 12 {
            // Buttons then text boxes: 6 states each, 3 pieces per state.
            Tex::BUTTON_OFF_LEFT.offset(3 * i)
        } else {
            Tex::TOGGLE_ON_LEFT.offset(3 * (i - 12))
        }
    }
}

/// Two-state check box / radio look (`c_*` / `r_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkLook {
    On,
    OnFocus,
    OnMouse,
    OnFocusMouse,
    OnDisabled,
    Off,
    OffFocus,
    OffMouse,
    OffFocusMouse,
    OffDisabled,
}

/// Slider look (`s_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SliderLook {
    Off,
    OffMouse,
    OnMouse,
    Disabled,
}

/// Bitmap button look (`bb_*`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitmapLook {
    Off,
    OffMouse,
    On,
    OnMouse,
}

/// The cut-out corners a cutout region removed from a framed box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cutout {
    pub rect: Rect,
}

/// Concave corner offset for convex corners (`FER_CCBM`).
const CCBM: i32 = 8;

/// The drawing context: a canvas plus the bitmap caches.
pub struct Fer<'a> {
    pub canvas: &'a mut Canvas,
    pub res: &'a Resources,
}

impl<'a> Fer<'a> {
    pub fn new(canvas: &'a mut Canvas, res: &'a Resources) -> Self {
        Self { canvas, res }
    }

    /// A frame piece in the requested orientation.
    pub fn tex(&self, t: Tex, orient: Orient) -> Option<Rc<Image>> {
        let q = orient.quarters();
        if q == 0 {
            return self.res.image(&t.path());
        }
        let base = self.res.image(&t.path())?;
        self.res
            .derived_image(&format!("{}#{q}", t.path()), || Some(base.rotated(q)))
    }

    /// `ferDraw`: the bitmap occupies `x .. x+w`, `y-h .. y`.
    pub fn draw(&mut self, x: i32, y: i32, img: &Image, mode: BlitMode) {
        self.canvas.blit(img, x, y - img.height, mode);
    }

    fn corner(
        &mut self,
        x: i32,
        y: i32,
        img: &Image,
        convex: bool,
        corner: Orient,
        mode: BlitMode,
    ) {
        let (w, h) = (img.width, img.height);
        let (dx, dy) = if convex {
            match corner {
                Orient::BottomLeft => (x - CCBM, y + CCBM),
                Orient::TopLeft => (x - CCBM, y + h - CCBM),
                Orient::TopRight => (x - h + CCBM, y + h - CCBM),
                Orient::BottomRight => (x - h + CCBM, y + CCBM),
                _ => return,
            }
        } else {
            match corner {
                Orient::BottomLeft => (x, y),
                Orient::TopLeft => (x, y + h),
                Orient::TopRight => (x - w, y + h),
                Orient::BottomRight => (x - w, y),
                _ => return,
            }
        };
        self.draw(dx, dy, img, mode);
    }

    /// `ferDrawBoxLine`: one tiled side of a framed box. `kludge` shifts the tiles by one
    /// texture size as the engine's alignment tweak does.
    #[allow(clippy::too_many_arguments)]
    fn box_line(
        &mut self,
        mut x0: i32,
        mut y0: i32,
        x1: i32,
        y1: i32,
        corner_offset: i32,
        corner_width: i32,
        img: &Image,
        kludge: bool,
        mode: BlitMode,
    ) {
        let (w, h) = (img.width, img.height);
        let (mut dist_x, mut dist_y) = (x1 - x0, y1 - y0);
        let (mut x1, mut y1) = (x1, y1);
        let (mut tx, mut ty) = (0, 0);
        if y0 == y1 {
            x0 += corner_width - corner_offset;
            x1 -= corner_width - corner_offset;
            dist_x -= w + 2 * (corner_width - corner_offset);
        } else {
            y0 += corner_width + h - corner_offset;
            y1 -= corner_width + h - corner_offset;
            dist_y -= h + 2 * (corner_width - corner_offset);
        }
        while tx <= dist_x && ty <= dist_y {
            if kludge {
                if dist_x != 0 {
                    self.draw(x0, y0 + h, img, mode);
                } else {
                    self.draw(x0 - w, y0, img, mode);
                }
            } else {
                self.draw(x0, y0, img, mode);
            }
            if dist_x != 0 {
                x0 += w;
                tx += w;
            } else {
                y0 += h;
                ty += h;
            }
        }
        if kludge {
            self.draw(x1 - w, y1 + h, img, mode);
        } else if dist_x != 0 {
            self.draw(x1 - w, y1, img, mode);
        } else {
            self.draw(x1, y1 + h, img, mode);
        }
    }

    /// `ferDrawLine`: tiles `img` between two end caps of size `corner_width`.
    #[allow(clippy::too_many_arguments)]
    pub fn line(
        &mut self,
        mut x0: i32,
        mut y0: i32,
        x1: i32,
        y1: i32,
        corner_width: i32,
        img: &Image,
        mode: BlitMode,
    ) {
        let (w, h) = (img.width, img.height);
        let (mut dist_x, mut dist_y) = (x1 - x0, y1 - y0);
        let (mut x1, mut y1) = (x1, y1);
        let (mut tx, mut ty) = (0, 0);
        if y0 == y1 {
            x0 += corner_width;
            x1 -= corner_width;
            dist_x -= w + 2 * corner_width;
        } else {
            y0 += corner_width + h;
            y1 -= corner_width + h;
            dist_y -= h + 2 * corner_width;
        }
        while tx <= dist_x && ty <= dist_y {
            self.draw(x0, y0, img, mode);
            if dist_x != 0 {
                x0 += w;
                tx += w;
            } else {
                y0 += h;
                ty += h;
            }
        }
        if dist_x != 0 {
            self.draw(x1 - w, y1, img, mode);
        } else {
            self.draw(x1, y1 + h, img, mode);
        }
    }

    /// Framed box (`ferDrawBoxRegion`). `textures`/`glow` are the atom's two draw-style words;
    /// out-of-range values select medium textures with an external glow, as in the engine.
    pub fn box_region(
        &mut self,
        r: Rect,
        textures: i32,
        glow: i32,
        cutouts: &[Cutout],
        use_alpha: bool,
    ) {
        let textures = if !(-1..=5).contains(&textures) {
            1
        } else {
            textures
        };
        let glow = if !(-1..=5).contains(&glow) { 4 } else { glow };
        let (outer, inner, side, lineup) = match (textures, glow) {
            (0, 3) => (
                Tex::BOX_SMALL_INNER_CORNER,
                Tex::BOX_SMALL_OUTER_CORNER,
                Tex::BOX_SMALL_INGLOW_STRAIGHT,
                true,
            ),
            (0, 4) => (
                Tex::BOX_SMALL_OUTER_CORNER,
                Tex::BOX_SMALL_INNER_CORNER,
                Tex::BOX_SMALL_OUTGLOW_STRAIGHT,
                true,
            ),
            (0, _) => (
                Tex::BOX_NOGLOW_SMALL_CORNER,
                Tex::BOX_NOGLOW_SMALL_CORNER,
                Tex::BOX_NOGLOW_STRAIGHT,
                true,
            ),
            (1, 3) => (
                Tex::BOX_INNER_CORNER,
                Tex::BOX_OUTER_CORNER,
                Tex::BOX_INGLOW_STRAIGHT,
                true,
            ),
            (1, 4) => (
                Tex::BOX_OUTER_CORNER,
                Tex::BOX_INNER_CORNER,
                Tex::BOX_OUTGLOW_STRAIGHT,
                true,
            ),
            (1, _) => (
                Tex::BOX_NOGLOW_CORNER,
                Tex::BOX_NOGLOW_CORNER,
                Tex::BOX_NOGLOW_STRAIGHT,
                true,
            ),
            (2, 3) => (
                Tex::BOX_LARGE_INNER_CORNER,
                Tex::BOX_LARGE_OUTER_CORNER,
                Tex::BOX_LARGE_INGLOW_STRAIGHT,
                false,
            ),
            (2, 4) => (
                Tex::BOX_LARGE_OUTER_CORNER,
                Tex::BOX_LARGE_INNER_CORNER,
                Tex::BOX_LARGE_OUTGLOW_STRAIGHT,
                true,
            ),
            (2, _) => (
                Tex::BOX_NOGLOW_LARGE_CORNER,
                Tex::BOX_NOGLOW_LARGE_CORNER,
                Tex::BOX_NOGLOW_STRAIGHT,
                true,
            ),
            _ => (
                Tex::BOX_OUTER_CORNER,
                Tex::BOX_INNER_CORNER,
                Tex::BOX_OUTGLOW_STRAIGHT,
                true,
            ),
        };
        let mode = if use_alpha {
            BlitMode::Blend
        } else {
            BlitMode::AlphaTest(0)
        };
        self.draw_box(r, outer, inner, side, lineup, cutouts, mode);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_box(
        &mut self,
        r: Rect,
        outer: Tex,
        inner: Tex,
        side: Tex,
        kludge: bool,
        cutouts: &[Cutout],
        mode: BlitMode,
    ) {
        // Each cut-out returns which part of the box it removed; the engine keeps at most 8.
        let mut cuts: Vec<(Orient, Rect)> = Vec::new();
        for c in cutouts {
            let kind = self.cutout(r, c.rect, outer, inner, side, kludge, mode);
            if cuts.len() < 8 {
                cuts.push((kind, c.rect));
            }
        }
        let width = self.box_corners(r, outer, &cuts, mode);
        self.box_sides(r, side, width, &cuts, kludge, mode);
    }

    fn box_corners(
        &mut self,
        r: Rect,
        corner: Tex,
        cuts: &[(Orient, Rect)],
        mode: BlitMode,
    ) -> i32 {
        let has = |o: Orient| cuts.iter().any(|c| c.0 == o);
        let mut width = self.tex(corner, Orient::TopLeft).map_or(0, |t| t.width);
        for (o, x, y) in [
            (Orient::BottomLeft, r.x0, r.y1),
            (Orient::TopLeft, r.x0, r.y0),
            (Orient::TopRight, r.x1, r.y0),
            (Orient::BottomRight, r.x1, r.y1),
        ] {
            if !has(o) {
                if let Some(t) = self.tex(corner, o) {
                    width = t.width;
                    self.corner(x, y, &t, true, o, mode);
                }
            }
        }
        width
    }

    fn box_sides(
        &mut self,
        r: Rect,
        side: Tex,
        cw: i32,
        cuts: &[(Orient, Rect)],
        kludge: bool,
        mode: BlitMode,
    ) {
        let find = |o: Orient| cuts.iter().find(|c| c.0 == o).map(|c| c.1);
        // Left side.
        if let Some(t) = self.tex(side, Orient::Left) {
            let mut e = Rect::new(r.x0, r.y0, r.x0, r.y1);
            if let Some(c) = find(Orient::TopLeft) {
                e.y0 = c.y1;
            }
            if let Some(c) = find(Orient::Left) {
                e.y1 = c.y0;
                self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, kludge, mode);
                e.y0 = c.y1;
                e.y1 = r.y1;
            }
            if let Some(c) = find(Orient::BottomLeft) {
                e.y1 = c.y0;
            }
            self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, kludge, mode);
        }
        // Top side.
        if let Some(t) = self.tex(side, Orient::Top) {
            let mut e = Rect::new(r.x0, r.y0, r.x1, r.y0);
            if let Some(c) = find(Orient::TopLeft) {
                e.x0 = c.x1;
            }
            if let Some(c) = find(Orient::Top) {
                e.x1 = c.x0;
                self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, !kludge, mode);
                e.x0 = c.x1;
                e.x1 = r.x1;
            }
            if let Some(c) = find(Orient::TopRight) {
                e.x1 = c.x0;
            }
            self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, !kludge, mode);
        }
        // Right side.
        if let Some(t) = self.tex(side, Orient::Right) {
            let mut e = Rect::new(r.x1, r.y0, r.x1, r.y1);
            if let Some(c) = find(Orient::TopRight) {
                e.y0 = c.y1;
            }
            if let Some(c) = find(Orient::Right) {
                e.y1 = c.y0;
                self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, !kludge, mode);
                e.y0 = c.y1;
                e.y1 = r.y1;
            }
            if let Some(c) = find(Orient::BottomRight) {
                e.y1 = c.y0;
            }
            self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, !kludge, mode);
        }
        // Bottom side.
        if let Some(t) = self.tex(side, Orient::Bottom) {
            let mut e = Rect::new(r.x0, r.y1, r.x1, r.y1);
            if let Some(c) = find(Orient::BottomLeft) {
                e.x0 = c.x1;
            }
            if let Some(c) = find(Orient::Bottom) {
                e.x1 = c.x0;
                self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, kludge, mode);
                e.x0 = c.x1;
                e.x1 = r.x1;
            }
            if let Some(c) = find(Orient::BottomRight) {
                e.x1 = c.x0;
            }
            self.box_line(e.x0, e.y0, e.x1, e.y1, CCBM, cw, &t, kludge, mode);
        }
    }

    /// `ferDrawCutout`: extra corners and sides where a cutout region bites into the box.
    #[allow(clippy::too_many_arguments)]
    fn cutout(
        &mut self,
        d: Rect,
        c: Rect,
        outer: Tex,
        inner: Tex,
        side: Tex,
        k: bool,
        mode: BlitMode,
    ) -> Orient {
        let inside = |x: i32, y: i32, r: Rect| r.strictly_contains(x, y);
        let t = |s: &Self, tex: Tex, o: Orient| s.tex(tex, o);
        if inside(d.x0, d.y0, c) {
            if let Some(tx) = t(self, outer, Orient::TopLeft) {
                self.corner(d.x0, c.y1, &tx, true, Orient::TopLeft, mode);
                self.corner(c.x1, d.y0, &tx, true, Orient::TopLeft, mode);
            }
            let mut cw = 0;
            if let Some(tx) = t(self, inner, Orient::BottomRight) {
                self.corner(c.x1, c.y1, &tx, false, Orient::BottomRight, mode);
                cw = tx.width;
            }
            if let Some(tx) = t(self, side, Orient::Left) {
                self.box_line(c.x1, d.y0, c.x1, c.y1 - cw, 0, cw, &tx, k, mode);
            }
            if let Some(tx) = t(self, side, Orient::Top) {
                self.box_line(d.x0, c.y1, c.x1 - cw, c.y1, 0, cw, &tx, !k, mode);
            }
            return Orient::TopLeft;
        }
        if inside(d.x1, d.y0, c) {
            if let Some(tx) = t(self, outer, Orient::TopRight) {
                self.corner(c.x0, d.y0, &tx, true, Orient::TopRight, mode);
                self.corner(d.x1, c.y1, &tx, true, Orient::TopRight, mode);
            }
            let mut cw = 0;
            if let Some(tx) = t(self, inner, Orient::BottomLeft) {
                self.corner(c.x0, c.y1, &tx, false, Orient::BottomLeft, mode);
                cw = tx.width / 2;
            }
            if let Some(tx) = t(self, side, Orient::Right) {
                self.box_line(c.x0, d.y0, c.x0, c.y1 - cw, 0, cw, &tx, !k, mode);
            }
            if let Some(tx) = t(self, side, Orient::Top) {
                self.box_line(c.x0 + cw, c.y1, d.x1, c.y1, 0, cw, &tx, !k, mode);
            }
            return Orient::TopRight;
        }
        if inside(d.x1, d.y1, c) {
            if let Some(tx) = t(self, outer, Orient::BottomRight) {
                self.corner(c.x0, d.y1, &tx, true, Orient::BottomRight, mode);
                self.corner(d.x1, c.y0, &tx, true, Orient::BottomRight, mode);
            }
            let mut cw = 0;
            if let Some(tx) = t(self, inner, Orient::TopLeft) {
                self.corner(c.x0, c.y0, &tx, false, Orient::TopLeft, mode);
                cw = tx.width / 2;
            }
            if let Some(tx) = t(self, side, Orient::Right) {
                self.box_line(c.x0, c.y0 + cw, c.x0, d.y1, 0, cw, &tx, !k, mode);
            }
            if let Some(tx) = t(self, side, Orient::Bottom) {
                self.box_line(c.x0 + cw, c.y0, d.x1, c.y0, 0, cw, &tx, k, mode);
            }
            return Orient::BottomRight;
        }
        if inside(d.x0, d.y1, c) {
            if let Some(tx) = t(self, outer, Orient::BottomLeft) {
                self.corner(d.x0, c.y0, &tx, true, Orient::BottomLeft, mode);
                self.corner(c.x1, d.y1, &tx, true, Orient::BottomLeft, mode);
            }
            let mut cw = 0;
            if let Some(tx) = t(self, inner, Orient::TopRight) {
                self.corner(c.x1, c.y0, &tx, false, Orient::TopRight, mode);
                cw = tx.width / 2;
            }
            if let Some(tx) = t(self, side, Orient::Left) {
                self.box_line(c.x1, c.y0 + cw, c.x1, d.y1, 0, cw, &tx, k, mode);
            }
            if let Some(tx) = t(self, side, Orient::Bottom) {
                self.box_line(d.x0, c.y0, c.x1 - cw, c.y0, 0, cw, &tx, k, mode);
            }
            return Orient::BottomLeft;
        }
        // Whole bottom edge of the cut-out inside the box: a notch in the top side.
        if inside(c.x0, c.y1, d) && inside(c.x1, c.y1, d) {
            self.notch_corners(
                &[
                    (outer, Orient::TopRight, c.x0, d.y0, true),
                    (outer, Orient::TopLeft, c.x1, d.y0, true),
                    (inner, Orient::BottomLeft, c.x0, c.y1, false),
                    (inner, Orient::BottomRight, c.x1, c.y1, false),
                ],
                mode,
            );
            let cw = self
                .tex(inner, Orient::BottomRight)
                .map_or(0, |t| t.width / 2);
            self.notch_lines(
                side,
                &[
                    (Orient::Top, c.x0, c.y1, c.x1, c.y1, !k),
                    (Orient::Right, c.x0, d.y0, c.x0, c.y1, !k),
                    (Orient::Left, c.x1, d.y0, c.x1, c.y1, k),
                ],
                cw,
                mode,
            );
            return Orient::Top;
        }
        // Whole top edge inside: a notch in the bottom side.
        if inside(c.x0, c.y0, d) && inside(c.x1, c.y0, d) {
            self.notch_corners(
                &[
                    (inner, Orient::TopRight, c.x1, c.y0, false),
                    (inner, Orient::TopLeft, c.x0, c.y0, false),
                    (outer, Orient::BottomLeft, c.x1, d.y1, true),
                    (outer, Orient::BottomRight, c.x0, d.y1, true),
                ],
                mode,
            );
            let cw = self
                .tex(outer, Orient::BottomRight)
                .map_or(0, |t| t.width / 2);
            self.notch_lines(
                side,
                &[
                    (Orient::Bottom, c.x0, c.y0, c.x1, c.y0, k),
                    (Orient::Right, c.x0, c.y0, c.x0, d.y1, !k),
                    (Orient::Left, c.x1, c.y0, c.x1, d.y1, k),
                ],
                cw,
                mode,
            );
            return Orient::Bottom;
        }
        // Whole left edge inside: a notch in the right side.
        if inside(c.x0, c.y0, d) && inside(c.x0, c.y1, d) {
            self.notch_corners(
                &[
                    (outer, Orient::TopRight, d.x1, c.y1, true),
                    (inner, Orient::TopLeft, c.x0, c.y0, false),
                    (inner, Orient::BottomLeft, c.x0, c.y1, false),
                    (outer, Orient::BottomRight, d.x1, c.y0, true),
                ],
                mode,
            );
            let cw = self
                .tex(outer, Orient::BottomRight)
                .map_or(0, |t| t.width / 2);
            self.notch_lines(
                side,
                &[
                    (Orient::Right, c.x0, c.y0, c.x0, c.y1, !k),
                    (Orient::Bottom, c.x0, c.y0, d.x1, c.y0, k),
                    (Orient::Top, c.x0, c.y1, d.x1, c.y1, !k),
                ],
                cw,
                mode,
            );
            return Orient::Right;
        }
        // Whole right edge inside: a notch in the left side.
        if inside(c.x1, c.y0, d) && inside(c.x1, c.y1, d) {
            self.notch_corners(
                &[
                    (inner, Orient::TopRight, c.x1, c.y0, false),
                    (outer, Orient::TopLeft, d.x0, c.y1, true),
                    (outer, Orient::BottomLeft, d.x0, c.y0, true),
                    (inner, Orient::BottomRight, c.x1, c.y1, false),
                ],
                mode,
            );
            let cw = self
                .tex(inner, Orient::BottomRight)
                .map_or(0, |t| t.width / 2);
            self.notch_lines(
                side,
                &[
                    (Orient::Left, c.x1, c.y0, c.x1, c.y1, k),
                    (Orient::Bottom, d.x0, c.y0, c.x1, c.y0, k),
                    (Orient::Top, d.x0, c.y1, c.x1, c.y1, !k),
                ],
                cw,
                mode,
            );
            return Orient::Left;
        }
        // The engine returns an undefined value here; nothing was cut.
        Orient::None
    }

    fn notch_corners(&mut self, list: &[(Tex, Orient, i32, i32, bool)], mode: BlitMode) {
        for &(tex, o, x, y, convex) in list {
            if let Some(t) = self.tex(tex, o) {
                self.corner(x, y, &t, convex, o, mode);
            }
        }
    }

    fn notch_lines(
        &mut self,
        side: Tex,
        list: &[(Orient, i32, i32, i32, i32, bool)],
        cw: i32,
        mode: BlitMode,
    ) {
        for &(o, x0, y0, x1, y1, k) in list {
            if let Some(t) = self.tex(side, o) {
                self.box_line(x0, y0, x1, y1, 0, cw, &t, k, mode);
            }
        }
    }

    /// `ferDrawButton`: left cap, right cap and a tiled middle, bottom-aligned on the rect.
    pub fn button(&mut self, r: Rect, look: ButtonLook) {
        let left = look.left();
        let mode = BlitMode::AlphaTest(0);
        if let Some(t) = self.tex(left, Orient::None) {
            self.draw(r.x0, r.y1, &t, mode);
        }
        let mut end_width = 0;
        if let Some(t) = self.tex(left.offset(2), Orient::None) {
            self.draw(r.x1 - t.width, r.y1, &t, mode);
            end_width = t.width;
        }
        if let Some(t) = self.tex(left.offset(1), Orient::None) {
            self.line(r.x0, r.y1, r.x1, r.y1, end_width, &t, mode);
        }
    }

    pub fn checkbox(&mut self, r: Rect, look: MarkLook) {
        if let Some(t) = self.tex(Tex::CHECK_ON.offset(look as u16), Orient::None) {
            self.draw(r.x0, r.y1, &t, BlitMode::AlphaTest(0));
        }
    }

    pub fn radio(&mut self, r: Rect, look: MarkLook) {
        if let Some(t) = self.tex(Tex::RADIO_ON.offset(look as u16), Orient::None) {
            self.draw(r.x0, r.y1, &t, BlitMode::AlphaTest(0));
        }
    }

    /// `ferDrawHorizSlider`: volume-style bar with a marker at `value / max`.
    pub fn horiz_slider(&mut self, r: Rect, value: u32, max: u32, look: SliderLook) {
        let first = match look {
            SliderLook::Disabled => Tex::VOLUME_DISABLED_LEFTCAP,
            SliderLook::Off => Tex::VOLUME_DISABLED_LEFTCAP.offset(3),
            SliderLook::OffMouse => Tex::VOLUME_DISABLED_LEFTCAP.offset(6),
            SliderLook::OnMouse => Tex::VOLUME_DISABLED_LEFTCAP.offset(9),
        };
        let mode = BlitMode::AlphaTest(0);
        if let Some(mid) = self.tex(first.offset(1), Orient::None) {
            self.line(r.x0 - 4, r.y1, r.x1 + 4, r.y1, 8, &mid, mode);
        }
        if let Some(t) = self.tex(first, Orient::None) {
            self.draw(r.x0 - 12, r.y1, &t, mode);
        }
        if let Some(t) = self.tex(first.offset(2), Orient::None) {
            self.draw(r.x1 - 4, r.y1, &t, mode);
        }
        let spos = slider_pos(value, max, r.x0, r.x1);
        if let Some(t) = self.tex(Tex::VOLUME_BUTTON, Orient::None) {
            self.draw(spos - 4, r.y1, &t, mode);
        }
    }

    /// `ferDrawVertSlider`: equaliser bar.
    pub fn vert_slider(&mut self, r: Rect, value: u32, max: u32, look: SliderLook) {
        // Equaliser pieces come in (mouse, on, off) triples: straight, middle, bottom, top.
        let k = match look {
            SliderLook::OffMouse => 0,
            SliderLook::OnMouse => 1,
            _ => 2,
        };
        let s = Tex::EQ_BAR_MOUSE_STRAIGHT;
        let (mid, centre, bot, top) = (
            s.offset(k),
            s.offset(3 + k),
            s.offset(6 + k),
            s.offset(9 + k),
        );
        let mode = BlitMode::AlphaTest(0);
        if let Some(t) = self.tex(mid, Orient::None) {
            self.line(r.x0, r.y0 - 5, r.x0, r.y1 - 5, 0, &t, mode);
        }
        for (tex, y) in [(top, r.y0 - 5), (bot, r.y1 - 5), (centre, r.y1 - 5)] {
            if let Some(t) = self.tex(tex, Orient::None) {
                self.draw(r.x0, y, &t, mode);
            }
        }
        let spos = slider_pos(value, max, r.y0, r.y1);
        if let Some(t) = self.tex(Tex::EQ_BAR_BUTTON, Orient::None) {
            self.draw(r.x0 + 11, spos - 5, &t, mode);
        }
    }

    /// `ferDrawScrollbar` (vertical bars only, as in the engine). `thumb` is the thumb rect.
    pub fn scrollbar(&mut self, r: Rect, thumb: Rect, look: SliderLook) {
        let k = match look {
            SliderLook::OffMouse => 3,
            SliderLook::OnMouse => 6,
            _ => 0,
        };
        let (tt, tm, tb) = (
            Tex::VERT_ST_OFF_TOP.offset(k),
            Tex::VERT_ST_OFF_TOP.offset(k + 1),
            Tex::VERT_ST_OFF_TOP.offset(k + 2),
        );
        let texs: Vec<Option<Rc<Image>>> = [
            Tex::VERT_SB_TOP,
            Tex::VERT_SB_MID,
            Tex::VERT_SB_BOTTOM,
            tt,
            tm,
            tb,
        ]
        .iter()
        .map(|&t| self.tex(t, Orient::None))
        .collect();
        let Some(texs) = texs.into_iter().collect::<Option<Vec<_>>>() else {
            return;
        };
        let total: i32 = texs.iter().map(|t| t.height).sum();
        if total >= r.height() {
            return;
        }
        let mode = BlitMode::Blend;
        self.draw(r.x0, r.y0 + texs[0].height, &texs[0], mode);
        self.line(r.x0, r.y0, r.x0, r.y1, texs[0].height, &texs[1], mode);
        self.draw(r.x0, r.y1, &texs[2], mode);
        self.draw(thumb.x0, thumb.y0 + texs[3].height, &texs[3], mode);
        self.line(
            thumb.x0,
            thumb.y0,
            thumb.x0,
            thumb.y1,
            texs[3].height,
            &texs[4],
            mode,
        );
        self.draw(thumb.x0, thumb.y1, &texs[5], mode);
    }

    /// `ferDrawScrollbarButton`: `up` arrow or down arrow in off / mouse-over / pressed state.
    pub fn scroll_button(&mut self, r: Rect, up: bool, look: SliderLook) {
        let state = match look {
            SliderLook::OffMouse => 2,
            SliderLook::OnMouse => 4,
            _ => 0,
        };
        let tex = Tex::VERT_SA_OFF_UP.offset(state + u16::from(!up));
        if let Some(t) = self.tex(tex, Orient::None) {
            let y = if up { r.y0 + t.height } else { r.y1 };
            self.draw(r.x0, y, &t, BlitMode::Blend);
        }
    }

    /// `ferDrawFocusWindow`: the list-window frame, highlighted when it has focus.
    pub fn focus_window(&mut self, r: Rect, focus: bool) {
        let base = if focus {
            Tex::LIST_FOCUS_NW_CORNER
        } else {
            Tex::LIST_NW_CORNER
        };
        let mode = BlitMode::AlphaTest(0);
        let get = |s: &Self, i: u16| s.tex(base.offset(i), Orient::None);
        let (Some(nw), Some(ne), Some(se), Some(sw)) =
            (get(self, 0), get(self, 1), get(self, 2), get(self, 3))
        else {
            return;
        };
        self.draw(r.x0, r.y0 + nw.height, &nw, mode);
        self.draw(r.x1 - ne.width, r.y0 + ne.height, &ne, mode);
        self.draw(r.x1 - se.width, r.y1, &se, mode);
        self.draw(r.x0, r.y1, &sw, mode);
        let (cw, ch) = (sw.width, sw.height);
        if let Some(n) = get(self, 4) {
            self.line(r.x0, r.y0 + n.height, r.x1, r.y0 + n.height, cw, &n, mode);
        }
        if let Some(e) = get(self, 5) {
            self.line(r.x1 - e.width, r.y0, r.x1 - e.width, r.y1, ch, &e, mode);
        }
        if let Some(s) = get(self, 6) {
            self.line(r.x0, r.y1, r.x1, r.y1, cw, &s, mode);
        }
        if let Some(w) = get(self, 7) {
            self.line(r.x0, r.y0, r.x0, r.y1, ch, &w, mode);
        }
    }

    /// `ferDrawDecorative`: a bitmap anchored at the region's bottom-left. Bitmaps up to 256
    /// pixels on a side are alpha-tested; bigger ones go through the engine's pixel-copy path,
    /// which writes them whole.
    pub fn decorative(&mut self, r: Rect, path: &str) {
        if let Some(img) = self.res.image(path) {
            let mode = if img.width <= 256 && img.height <= 256 {
                BlitMode::AlphaTest(1)
            } else {
                BlitMode::Opaque
            };
            self.draw(r.x0, r.y1, &img, mode);
        }
    }

    /// `ferDrawBitmapButton`: `<base>_off.lif`, `_off_mouse`, `_on` or `_on_mouse`.
    pub fn bitmap_button(&mut self, r: Rect, base: &str, look: BitmapLook) {
        let suffix = match look {
            BitmapLook::Off => "_off.lif",
            BitmapLook::OffMouse => "_off_mouse.lif",
            BitmapLook::On => "_on.lif",
            BitmapLook::OnMouse => "_on_mouse.lif",
        };
        if let Some(img) = self.res.image(&format!("{base}{suffix}")) {
            self.draw(r.x0, r.y1, &img, BlitMode::Opaque);
        }
    }
}

/// Marker position of a slider: `value / max` along `lo .. hi`, truncated like the engine's
/// conversion to a 16-bit coordinate.
pub fn slider_pos(value: u32, max: u32, lo: i32, hi: i32) -> i32 {
    if max == 0 {
        return lo;
    }
    let f = value as f32 / max as f32 * (hi - lo) as f32 + lo as f32;
    f as i32 as u16 as i16 as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texture_table_matches_enum() {
        assert_eq!(TEX_FILES[Tex::CHECK_ON.0 as usize], "checkbox_on");
        assert_eq!(TEX_FILES[Tex::RADIO_ON.0 as usize], "radio_on");
        assert_eq!(TEX_FILES[Tex::VOLUME_BUTTON.0 as usize], "volume_button");
        assert_eq!(TEX_FILES[Tex::EQ_BAR_BUTTON.0 as usize], "eq_bar_button");
        assert_eq!(
            TEX_FILES[Tex::LIST_FOCUS_NW_CORNER.0 as usize],
            "info_focus_nw_corner"
        );
        assert_eq!(
            TEX_FILES[Tex::VERT_SA_OFF_UP.0 as usize],
            "vscrollbar_off_up"
        );
        assert_eq!(
            TEX_FILES[Tex::VERT_ST_OFF_TOP.0 as usize],
            "vscrolltab_off_topcap"
        );
        assert_eq!(
            TEX_FILES[Tex::VOLUME_DISABLED_LEFTCAP.0 as usize + 9],
            "volume_on_leftcap"
        );
        assert_eq!(
            TEX_FILES[Tex::EQ_BAR_MOUSE_STRAIGHT.0 as usize + 11],
            "eq_bar_off_topcap"
        );
        assert_eq!(
            TEX_FILES[ButtonLook::OnFocusMouse.left().0 as usize],
            "button_on_focus_mouse_left_endcap"
        );
        assert_eq!(
            TEX_FILES[ButtonLook::TextOff.left().0 as usize],
            "textbox_off_left_endcap"
        );
        assert_eq!(
            TEX_FILES[ButtonLook::ToggleOffDisabled.left().0 as usize + 2],
            "toggle_disabled_off_right_endcap"
        );
        assert_eq!(
            Tex(67).path(),
            "FeMan\\Textures\\toggle_off_left_endcap.lif"
        );
    }

    #[test]
    fn slider_marker() {
        assert_eq!(slider_pos(50, 100, 78, 230), 154);
        assert_eq!(slider_pos(0, 100, 78, 230), 78);
        assert_eq!(slider_pos(5, 0, 10, 20), 10);
    }
}
