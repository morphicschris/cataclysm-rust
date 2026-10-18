//! Team colour picker (`ColPick.c`): base and stripe colours chosen on a hue/saturation field
//! and a value bar, previews, the five previous colour schemes and the race preview image
//! painted in the team colours.

use std::rc::Rc;

use hwc_formats::lif::Lif;

use crate::canvas::{BlitMode, Color, Image, Rect, BLACK, WHITE};
use crate::cataclysm::{pack_color, unpack_color, Cataclysm};
use crate::consts::*;
use crate::fe::{Call, FrontEnd};
use crate::fer::Fer;
use crate::options::Options;
use crate::region::{AtomRef, RegionId};

/// Size of the hue/saturation field and the value bar textures.
const HUE_SAT: i32 = 256;
const VALUE_HEIGHT: i32 = 256;
/// Marker circle and value arrow.
const CIRCLE_RADIUS: i32 = 4;
const ARROW_WIDTH: i32 = 16;
const ARROW_HEIGHT: i32 = 16;
/// Default team colours of player 0 (`teamcolors.script`), used when the script is missing.
const DEFAULT_BASE: Color = [66, 108, 202, 255];
const DEFAULT_STRIPE: Color = [255, 255, 255, 255];

/// One colour as the picker edits it: 0..255 HSV plus the RGB it gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PickColor {
    pub hue: i32,
    pub sat: i32,
    pub val: i32,
    pub rgb: Color,
}

impl PickColor {
    pub fn from_rgb(c: Color) -> Self {
        let (h, s, v) = rgb_to_hsv(c);
        Self {
            hue: to_byte(h),
            sat: to_byte(s),
            val: to_byte(v),
            rgb: [c[0], c[1], c[2], 255],
        }
    }

    fn update_rgb(&mut self) {
        self.rgb = hsv_to_rgb(
            self.hue as f32 / 255.0,
            self.sat as f32 / 255.0,
            self.val as f32 / 255.0,
        );
    }
}

fn to_byte(v: f32) -> i32 {
    (v * 255.0).round().clamp(0.0, 255.0) as i32
}

/// `colRGBToHSV` (0..1 components; hue 0..1, undefined hue reported as 0).
pub fn rgb_to_hsv(c: Color) -> (f32, f32, f32) {
    let (r, g, b) = (
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let delta = max - min;
    let s = if max > 0.0 { delta / max } else { 0.0 };
    if s == 0.0 {
        return (0.0, s, v);
    }
    let mut h = if r == max {
        (g - b) / delta
    } else if g == max {
        2.0 + (b - r) / delta
    } else {
        4.0 + (r - g) / delta
    };
    h /= 6.0;
    if h < 0.0 {
        h += 1.0;
    }
    (h, s, v)
}

/// `colHSVToRGB`.
pub fn hsv_to_rgb(h: f32, s: f32, v: f32) -> Color {
    let byte = |x: f32| (x * 255.0).round().clamp(0.0, 255.0) as u8;
    if s <= 0.0 {
        return [byte(v), byte(v), byte(v), 255];
    }
    let h = if h >= 1.0 { 0.0 } else { h * 6.0 };
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    let (r, g, b) = match i as i32 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [byte(r), byte(g), byte(b), 255]
}

/// Picker state.
#[derive(Debug, Clone)]
pub struct ColorPicker {
    pub base: PickColor,
    pub stripe: PickColor,
    /// Editing the stripe colour (`cpColorMode`).
    pub stripe_mode: bool,
    pub previous: [(Color, Color); 5],
    pub default: (Color, Color),
    pub picked: bool,
    hue_sat: Option<Rc<Image>>,
}

impl ColorPicker {
    pub fn from_options(o: &Options) -> Self {
        let mut previous = [(BLACK, BLACK); 5];
        for (i, p) in previous.iter_mut().enumerate() {
            *p = (
                unpack_color(o.get(&format!("PrevColor{i}.base"))),
                unpack_color(o.get(&format!("PrevColor{i}.detail"))),
            );
        }
        let picked = o.get_bool("ColorsPicked");
        let (base, stripe) = if picked {
            (
                unpack_color(o.get("PlayerBaseColor")),
                unpack_color(o.get("PlayerStripeColor")),
            )
        } else {
            (DEFAULT_BASE, DEFAULT_STRIPE)
        };
        Self {
            base: PickColor::from_rgb(base),
            stripe: PickColor::from_rgb(stripe),
            stripe_mode: false,
            previous,
            default: (DEFAULT_BASE, DEFAULT_STRIPE),
            picked,
            hue_sat: None,
        }
    }

    /// Reads player 0's default colours from `teamcolors.script`.
    pub fn load_defaults(&mut self, script: &str) {
        let find = |key: &str| {
            script.lines().find_map(|l| {
                let l = l.split("//").next()?.trim();
                let rest = l.strip_prefix(key)?.trim();
                let v: Vec<u8> = rest
                    .split(',')
                    .filter_map(|x| x.trim().parse().ok())
                    .collect();
                (v.len() == 4 && v[0] == 0).then(|| [v[1], v[2], v[3], 255])
            })
        };
        if let (Some(b), Some(s)) = (find("BaseColor"), find("StripeColor")) {
            self.default = (b, s);
            if !self.picked {
                self.set_colors(b, s);
            }
        }
    }

    pub fn set_colors(&mut self, base: Color, stripe: Color) {
        self.base = PickColor::from_rgb(base);
        self.stripe = PickColor::from_rgb(stripe);
    }

    fn current(&mut self) -> &mut PickColor {
        if self.stripe_mode {
            &mut self.stripe
        } else {
            &mut self.base
        }
    }

    /// The hue/saturation field (`cpHueSatImageCreate`): hue across, saturation up, value
    /// falling off slightly towards low saturation.
    pub fn hue_sat_image(&mut self) -> Rc<Image> {
        self.hue_sat
            .get_or_insert_with(|| {
                let mut img = Image::new(HUE_SAT, HUE_SAT, BLACK);
                for sat in 0..HUE_SAT {
                    for hue in 0..HUE_SAT {
                        let c = hsv_to_rgb(
                            hue as f32 / 255.0,
                            (HUE_SAT - 1 - sat) as f32 / 255.0,
                            (HUE_SAT - 1 - sat / 4) as f32 / 255.0,
                        );
                        img.pixels[(sat * HUE_SAT + hue) as usize] = c;
                    }
                }
                Rc::new(img)
            })
            .clone()
    }

    /// `cpAcceptColors` + `cpColorsAddToPreviousList`: the chosen colours become the player's.
    pub fn accept(&mut self, o: &mut Options) {
        let (b, s) = (self.base.rgb, self.stripe.rgb);
        if (b, s) != self.default
            || o.get("PlayerBaseColor") != pack_color(b)
            || o.get("PlayerStripeColor") != pack_color(s)
        {
            self.picked = true;
        }
        if let Some(i) = self.previous.iter().position(|&p| p == (b, s)) {
            self.previous.swap(i, 0);
        } else {
            self.previous.rotate_right(1);
            self.previous[0] = (b, s);
        }
        self.store(o);
    }

    /// Writes the colour options.
    pub fn store(&self, o: &mut Options) {
        o.set("PlayerBaseColor", pack_color(self.base.rgb));
        o.set("PlayerStripeColor", pack_color(self.stripe.rgb));
        o.set_bool("ColorsPicked", self.picked);
        for (i, (b, d)) in self.previous.iter().enumerate() {
            o.set(&format!("PrevColor{i}.base"), pack_color(*b));
            o.set(&format!("PrevColor{i}.detail"), pack_color(*d));
        }
    }
}

type Fe = FrontEnd<Cataclysm>;

pub fn register(fe: &mut Fe) {
    fe.callback_add("CP_BaseColor", base_color);
    fe.callback_add("CP_StripeColor", |fe, _| set_mode(fe, true));
    fe.callback_add("CP_Default", |fe, _| {
        let (b, s) = fe.game.colors.default;
        fe.game.colors.set_colors(b, s);
        fe.game.colors.picked = true;
    });
    for name in [
        "CP_Previous0",
        "CP_Previous1",
        "CP_Previous2",
        "CP_Previous3",
        "CP_Previous4",
    ] {
        fe.callback_add(name, |fe, call| {
            let i = call
                .name
                .as_bytes()
                .last()
                .map_or(0, |c| (c - b'0') as usize)
                .min(4);
            let (b, s) = fe.game.colors.previous[i];
            if b != BLACK {
                fe.game.colors.set_colors(b, s);
            }
            fe.game.colors.picked = true;
        });
    }
    fe.region_function_add("cpHueSaturation", hue_sat_process);
    fe.region_function_add("cpValue", value_process);
    fe.draw_callback_add("CP_BaseColorPreview", |fe, fer, a, r| {
        preview(fe, fer, a, r, false)
    });
    fe.draw_callback_add("CP_StripeColorPreview", |fe, fer, a, r| {
        preview(fe, fer, a, r, true)
    });
    fe.draw_callback_add("CP_PreviewImage", preview_image);
    fe.draw_callback_add("CP_HueSaturation", hue_sat_draw);
    fe.draw_callback_add("CP_Value", value_draw);
    for name in [
        "CP_PreviousDraw0",
        "CP_PreviousDraw1",
        "CP_PreviousDraw2",
        "CP_PreviousDraw3",
        "CP_PreviousDraw4",
    ] {
        fe.draw_callback_add(name, previous_draw);
    }
}

fn set_mode(fe: &mut Fe, stripe: bool) {
    fe.toggle_set("CP_BaseColor", !stripe);
    fe.toggle_set("CP_StripeColor", stripe);
    fe.game.colors.stripe_mode = stripe;
}

/// `cpBaseColor`: on creation wires the field and bar up as interactive regions; on deletion
/// keeps the colours.
fn base_color(fe: &mut Fe, call: &Call) {
    if fe.first_call(call) {
        let filter = RPE_PRESS_LEFT | RPE_HOLD_LEFT | RPE_RELEASE_LEFT | RPE_EXIT_HOLD_LEFT;
        fe.region_function_set("CP_HueSaturation", "cpHueSaturation", filter);
        fe.region_function_set("CP_Value", "cpValue", filter);
        set_mode(fe, false);
        return;
    }
    if fe.last_call(call) {
        let g = &mut fe.game;
        g.colors.accept(&mut g.options);
        return;
    }
    set_mode(fe, false);
}

/// `cpHueSaturationProcess`.
fn hue_sat_process(fe: &mut Fe, id: RegionId, event: u32) {
    let Some(rect) = fe.region(id).map(|r| r.rect) else {
        return;
    };
    if event == RPE_PRESS_LEFT {
        fe.mouse_clip(Some(rect));
    }
    if event == RPE_PRESS_LEFT || event == RPE_HOLD_LEFT {
        let (mx, my) = fe.mouse_pos();
        let c = fe.game.colors.current();
        c.hue = (mx - rect.x0) * HUE_SAT / rect.width().max(1);
        c.sat = (rect.y1 - 1 - my) * HUE_SAT / rect.height().max(1);
        c.update_rgb();
    } else {
        fe.mouse_clip(None);
    }
}

/// `cpValueProcess`.
fn value_process(fe: &mut Fe, id: RegionId, event: u32) {
    let Some(rect) = fe.region(id).map(|r| r.rect) else {
        return;
    };
    if event == RPE_PRESS_LEFT {
        fe.mouse_clip(Some(rect));
    }
    if event == RPE_PRESS_LEFT || event == RPE_HOLD_LEFT {
        let (_, my) = fe.mouse_pos();
        let c = fe.game.colors.current();
        c.val = ((rect.y1 - 1 - my) * VALUE_HEIGHT / rect.height().max(1)).max(0);
        c.update_rgb();
    } else {
        fe.mouse_clip(None);
    }
}

/// Base / stripe colour swatch.
fn preview(fe: &mut Fe, fer: &mut Fer, a: AtomRef, rect: Rect, stripe: bool) {
    let c = if stripe {
        fe.game.colors.stripe.rgb
    } else {
        fe.game.colors.base.rgb
    };
    fer.canvas
        .fill_rect(Rect::new(rect.x0, rect.y0, rect.x1, rect.y1 - 1), c);
    fer.canvas.outline_rect(rect, fe.atom(a).border_color);
}

/// The race picture painted in the team colours (`cpPreviewImageDraw`).
fn preview_image(fe: &mut Fe, fer: &mut Fer, a: AtomRef, rect: Rect) {
    let race = if fe.game.options.get("PlayerRace") == 0 {
        1
    } else {
        2
    };
    let path = format!("ColorPicker\\Race{race}\\Race{race}.lif");
    let (b, s) = (fe.game.colors.base.rgb, fe.game.colors.stripe.rgb);
    let Some(data) = fer.res.read(&path) else {
        fe.log_once(format!("colour picker: {path} unavailable"));
        return;
    };
    let Ok(lif) = Lif::parse(&data) else { return };
    let rgba = lif.to_rgba_team([b[0], b[1], b[2]], [s[0], s[1], s[2]]);
    let img = Image::from_rgba(lif.width as i32, lif.height as i32, &rgba);
    crate::cataclysm::draw_image_in(fer, &img, rect);
    fer.canvas.outline_rect(rect, fe.atom(a).border_color);
}

/// `cpHueSaturationDraw`: the field with a circle on the current hue and saturation.
fn hue_sat_draw(fe: &mut Fe, fer: &mut Fer, a: AtomRef, rect: Rect) {
    let border = fe.atom(a).border_color;
    fer.canvas.fill_rect(
        Rect::new(
            rect.x0 - CIRCLE_RADIUS,
            rect.y0 - CIRCLE_RADIUS - 1,
            rect.x1 + CIRCLE_RADIUS,
            rect.y1 + CIRCLE_RADIUS,
        ),
        border,
    );
    let img = fe.game.colors.hue_sat_image();
    fer.canvas.blit_scaled(&img, rect, 255);
    fer.canvas
        .line(rect.x0, rect.y1 - 1, rect.x1, rect.y1 - 1, border);
    fer.canvas.line(rect.x0, rect.y1, rect.x1, rect.y1, border);
    let c = if fe.game.colors.stripe_mode {
        fe.game.colors.stripe
    } else {
        fe.game.colors.base
    };
    let cx = rect.x0 + c.hue * rect.width() / HUE_SAT + 1;
    let cy = rect.y1 - c.sat * rect.height() / HUE_SAT;
    octagon(fer, cx, cy, BLACK);
    octagon(fer, cx - 1, cy - 1, WHITE);
}

/// `primOvalArcOutline2` with 8 segments.
fn octagon(fer: &mut Fer, cx: i32, cy: i32, c: Color) {
    let pts: Vec<(i32, i32)> = (0..=8)
        .map(|i| {
            let t = i as f32 * std::f32::consts::TAU / 8.0;
            (
                cx + (t.sin() * CIRCLE_RADIUS as f32).round() as i32,
                cy + (t.cos() * CIRCLE_RADIUS as f32).round() as i32,
            )
        })
        .collect();
    for w in pts.windows(2) {
        fer.canvas.line(w[0].0, w[0].1, w[1].0, w[1].1, c);
    }
}

/// `cpValueDraw`: the value gradient with an arrow at the current value.
fn value_draw(fe: &mut Fe, fer: &mut Fer, a: AtomRef, rect: Rect) {
    let c = if fe.game.colors.stripe_mode {
        fe.game.colors.stripe
    } else {
        fe.game.colors.base
    };
    let val = c.val * rect.height() / VALUE_HEIGHT;
    let tx0 = rect.x1 - ARROW_WIDTH;
    let ty0 = rect.y1 - val - 1;
    let tip = tx0 + ARROW_WIDTH;
    fer.canvas
        .line(tx0, ty0, tip, ty0 - ARROW_HEIGHT / 2, WHITE);
    fer.canvas.line(
        tip,
        ty0 - ARROW_HEIGHT / 2,
        tip,
        ty0 + ARROW_HEIGHT / 2,
        WHITE,
    );
    fer.canvas
        .line(tip, ty0 + ARROW_HEIGHT / 2, tx0, ty0, WHITE);
    let bar = Rect::new(rect.x0, rect.y0, tx0 - 1, rect.y1);
    for y in bar.y0..bar.y1 {
        let v = 1.0 - (y - bar.y0) as f32 / bar.height().max(1) as f32;
        let col = hsv_to_rgb(c.hue as f32 / 255.0, c.sat as f32 / 255.0, v);
        fer.canvas
            .fill_rect(Rect::new(bar.x0, y, bar.x1, y + 1), col);
    }
    fer.canvas.outline_rect(bar, fe.atom(a).border_color);
    let _ = BlitMode::Opaque;
}

/// `cpPreviousDraw`: a remembered scheme, stripe as a band across the base.
fn previous_draw(fe: &mut Fe, fer: &mut Fer, a: AtomRef, rect: Rect) {
    let i = fe
        .atom(a)
        .name
        .as_deref()
        .and_then(|n| n.as_bytes().get(15))
        .map_or(0, |c| c.saturating_sub(b'0') as usize)
        .min(4);
    let (b, s) = fe.game.colors.previous[i];
    fer.canvas.fill_rect(rect, b);
    let band = Rect::new(
        rect.x0,
        rect.y0 + rect.height() / 3,
        rect.x1,
        rect.y0 + rect.height() * 3 / 5,
    );
    fer.canvas.fill_rect(band, s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_round_trip() {
        for c in [
            [66, 108, 202, 255],
            [255, 255, 255, 255],
            [200, 0, 0, 255],
            [0, 0, 0, 255],
        ] {
            let (h, s, v) = rgb_to_hsv(c);
            let back = hsv_to_rgb(h, s, v);
            for k in 0..3 {
                assert!(
                    (back[k] as i32 - c[k] as i32).abs() <= 1,
                    "{c:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn previous_list() {
        let mut o = Options::default();
        let mut p = ColorPicker::from_options(&o);
        p.set_colors([1, 2, 3, 255], [4, 5, 6, 255]);
        p.accept(&mut o);
        p.set_colors([7, 8, 9, 255], [4, 5, 6, 255]);
        p.accept(&mut o);
        p.set_colors([1, 2, 3, 255], [4, 5, 6, 255]);
        p.accept(&mut o);
        assert_eq!(p.previous[0].0, [1, 2, 3, 255]);
        assert_eq!(p.previous[1].0, [7, 8, 9, 255]);
        assert!(p.picked);
        let q = ColorPicker::from_options(&o);
        assert_eq!(q.base.rgb, [1, 2, 3, 255]);
        assert_eq!(q.previous, p.previous);
    }

    #[test]
    fn team_defaults() {
        let mut p = ColorPicker::from_options(&Options::default());
        p.load_defaults(
            "BaseColor   0, 1, 2, 3\nBaseColor 1, 9,9,9\nStripeColor 0, 4, 5, 6 // x\n",
        );
        assert_eq!(p.default, ([1, 2, 3, 255], [4, 5, 6, 255]));
        assert_eq!(p.base.rgb, [1, 2, 3, 255]);
    }
}
