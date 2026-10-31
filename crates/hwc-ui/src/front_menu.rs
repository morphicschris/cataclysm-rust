//! The modern front end: the main menu and every screen behind it (training, single player
//! with its team colours and saved games, the options and their pop-ups, the quit and delete
//! confirmations) in the modern HUD's look, over a live scene the application draws behind
//! them. A user-requested change, on by default; `--front-end original` or
//! `hwcModernFrontEnd = 0` bring back the original screens.
//!
//! The screens are the in-game modern menus' ([`crate::ingame::modern_menu`]), built every
//! frame from the original screen's atoms with the original screen open underneath, so every
//! callback and rule stays the original's. What is the front end's own: the panels stand in a
//! column on the left (the scene shows on the right), the main menu has the game's name over
//! it and the version under it, the column of buttons down the right of the options and
//! single-player screens becomes a row of tabs (the options get ENHANCEMENTS as one more), and
//! the pictures those screens draw themselves (the team colour preview, the colour picker,
//! the previous colours, the countdown) are shown from the original's own drawing, zoomed, with
//! the mouse handed on to them.

use std::sync::atomic::{AtomicU8, Ordering};

use crate::canvas::{Canvas, Color, Rect};
use crate::ingame::modern_hud::{Face, Pen};
use crate::ingame::modern_manager::{self as mm, write, GOLD};
use crate::ingame::modern_menu::{Layout, Model};
use crate::options::Options;

/// The options-file key: 1 (default) the modern front end, 0 the original's.
pub const OPTION_KEY: &str = "hwcModernFrontEnd";

/// The main menu.
pub const MAIN: &str = "Main_game_screen";

/// The option screens, joined by their tabs; ENHANCEMENTS is added to them.
pub const OPTION_TABS: [&str; 4] = [
    "Audio_Options",
    "Video_Options",
    "Gameplay_Options",
    "Keyboard",
];

/// User regions the modern screens show some other way (the version number goes under the
/// main menu).
pub const DRAWN_ELSEWHERE: [&str; 1] = ["FE_DrawVersionNum"];

/// 0: the options file decides; 1 modern; 2 original (`--front-end`).
static OVERRIDE: AtomicU8 = AtomicU8::new(0);

/// Sets the command line's choice; `None`: the options file decides.
pub fn set_override(modern: Option<bool>) {
    let v = match modern {
        None => 0,
        Some(true) => 1,
        Some(false) => 2,
    };
    OVERRIDE.store(v, Ordering::Relaxed);
}

/// The command line's choice, if it made one.
pub fn overridden() -> Option<bool> {
    match OVERRIDE.load(Ordering::Relaxed) {
        1 => Some(true),
        2 => Some(false),
        _ => None,
    }
}

/// Whether the front end is the modern one (whatever the HUD).
pub fn enabled(options: &Options) -> bool {
    overridden().unwrap_or_else(|| options.get_bool(OPTION_KEY))
}

/// Parses `--front-end`'s value.
pub fn parse_mode(s: &str) -> Option<bool> {
    mm::parse_mode(s)
}

/// The title of a screen that has none of its own.
pub fn title_for(screen: &str) -> &'static str {
    match screen {
        MAIN => "MAIN MENU",
        "Countdown_Message_Box" => "KEEP THIS DISPLAY?",
        "Press_Key" => "KEY BINDINGS",
        "List_Mappable_Keys" => "KEYS",
        "Video_Explanation" => "VIDEO OPTIONS",
        _ => "",
    }
}

/// The space over the main menu for the game's name, at 1080 lines.
const NAME_H: f32 = 190.0;

/// Puts a laid-out screen in the front end's place: its left edge a margin from the window's,
/// the main menu lower down under the game's name.
pub fn place(l: &mut Layout, m: &Model, width: i32, height: i32) {
    let p = l.frame.panel;
    let margin = ((width as f32 * 0.045).round() as i32).max(l.frame.px(24.0));
    let dx = margin.min(((width - p.width()) / 2).max(0)) - p.x0;
    let dy = if m.name == MAIN {
        let name = l.frame.px(NAME_H);
        let top = ((height - p.height() - name) / 2).max(0);
        (top + name).min((height - p.height()).max(0)) - p.y0
    } else {
        0
    };
    l.shift(dx, dy);
    l.frame.dim = false;
}

/// Centres a pop-up's panel over `over` (the panel of the screen under it), inside the window.
pub fn centre_on(l: &mut Layout, over: Rect, width: i32, height: i32) {
    let p = l.frame.panel;
    let x = ((over.x0 + over.x1 - p.width()) / 2).clamp(0, (width - p.width()).max(0));
    let y = ((over.y0 + over.y1 - p.height()) / 2).clamp(0, (height - p.height()).max(0));
    l.shift(x - p.x0, y - p.y0);
}

/// Under the panels: the scene darkened towards the left, where they stand.
pub fn draw_under(canvas: &mut Canvas, l: &Layout) {
    shade_left(canvas, l.frame.panel.x1);
}

/// Beside the panels: on the main menu, the game's name over it and the version under it.
pub fn draw_over(canvas: &mut Canvas, pen: &Pen, m: &Model, l: &Layout, version: &str) {
    if m.name != MAIN {
        return;
    }
    let f = &l.frame;
    let x = f.panel.x0;
    let base = f.panel.y0 - f.px(34.0);
    write(
        canvas,
        pen,
        Face::Bold,
        f.text(68.0),
        6.0 * f.scale,
        x - f.px(3.0),
        base,
        GOLD,
        Some(([255, 160, 0, 150], 9.0 * f.scale)),
        "CATACLYSM",
    );
    write(
        canvas,
        pen,
        Face::Semibold,
        f.text(24.0),
        14.0 * f.scale,
        x,
        base - f.px(78.0),
        [236, 239, 243, 230],
        Some(([255, 255, 255, 60], 4.0 * f.scale)),
        "HOMEWORLD",
    );
    mm::hline(
        canvas,
        x,
        f.panel.x1,
        base + f.px(18.0),
        mm::alpha(GOLD, 0.35),
    );
    if !version.is_empty() {
        let px = f.text(11.0);
        write(
            canvas,
            pen,
            Face::Semibold,
            px,
            2.0 * f.scale,
            x,
            (canvas.height - f.px(24.0)).max(f.panel.y1 + f.px(28.0)),
            mm::DIM,
            None,
            &version.to_uppercase(),
        );
    }
}

/// Darkens the picture from the left edge to a little past `x`, most at the left.
fn shade_left(canvas: &mut Canvas, x: i32) {
    let end = (x + (canvas.width as f32 * 0.18) as i32).min(canvas.width);
    if end <= 0 {
        return;
    }
    for col in 0..end {
        let t = col as f32 / end as f32;
        let a = (150.0 * (1.0 - t * t)) as u8;
        if a == 0 {
            continue;
        }
        let c: Color = [2, 3, 6, a];
        canvas.blend_rect(Rect::new(col, 0, col + 1, canvas.height), c);
    }
}

/// The backdrop before the scene is ready (or without one): deep space, darker at the edges.
pub fn draw_backdrop(canvas: &mut Canvas) {
    let (w, h) = (canvas.width.max(1), canvas.height.max(1));
    let (cx, cy) = (w as f32 * 0.62, h as f32 * 0.45);
    let r = (w.max(h) as f32) * 0.75;
    for y in 0..h {
        for x in 0..w {
            let d = (((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / r).min(1.0);
            let k = 1.0 - d;
            let c = [
                (4.0 + 18.0 * k * k) as u8,
                (5.0 + 14.0 * k * k) as u8,
                (9.0 + 22.0 * k * k) as u8,
                255,
            ];
            canvas.pixels[(y * w + x) as usize] = c;
        }
    }
}

/// Copies `src`'s rectangle `from` into `to` on `dst`, scaled with bilinear filtering (the
/// pictures the original screens draw, zoomed into the modern ones).
pub fn copy_scaled(dst: &mut Canvas, src: &Canvas, from: Rect, to: Rect) {
    let from = Rect::new(
        from.x0.max(0),
        from.y0.max(0),
        from.x1.min(src.width),
        from.y1.min(src.height),
    );
    if from.width() <= 0 || from.height() <= 0 || to.width() <= 0 || to.height() <= 0 {
        return;
    }
    let clip = dst.clip();
    let sx = from.width() as f32 / to.width() as f32;
    let sy = from.height() as f32 / to.height() as f32;
    let at = |x: i32, y: i32| -> [f32; 3] {
        let x = x.clamp(from.x0, from.x1 - 1);
        let y = y.clamp(from.y0, from.y1 - 1);
        let p = src.pixels[(y * src.width + x) as usize];
        [f32::from(p[0]), f32::from(p[1]), f32::from(p[2])]
    };
    for y in to.y0.max(clip.y0)..to.y1.min(clip.y1) {
        let fy = from.y0 as f32 + (y - to.y0) as f32 * sy + sy * 0.5 - 0.5;
        let y0 = fy.floor() as i32;
        let ty = fy - y0 as f32;
        for x in to.x0.max(clip.x0)..to.x1.min(clip.x1) {
            let fx = from.x0 as f32 + (x - to.x0) as f32 * sx + sx * 0.5 - 0.5;
            let x0 = fx.floor() as i32;
            let tx = fx - x0 as f32;
            let (a, b, c, d) = (
                at(x0, y0),
                at(x0 + 1, y0),
                at(x0, y0 + 1),
                at(x0 + 1, y0 + 1),
            );
            let mut out = [0u8, 0, 0, 255];
            for k in 0..3 {
                let top = a[k] + (b[k] - a[k]) * tx;
                let bottom = c[k] + (d[k] - c[k]) * tx;
                out[k] = (top + (bottom - top) * ty).round() as u8;
            }
            dst.pixels[(y * dst.width + x) as usize] = out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_scaled_doubles_a_picture() {
        let mut src = Canvas::new(4, 4);
        src.fill_rect(Rect::new(0, 0, 2, 4), [255, 0, 0, 255]);
        src.fill_rect(Rect::new(2, 0, 4, 4), [0, 0, 255, 255]);
        let mut dst = Canvas::new(8, 8);
        copy_scaled(&mut dst, &src, Rect::new(0, 0, 4, 4), Rect::new(0, 0, 8, 8));
        assert_eq!(dst.pixels[0], [255, 0, 0, 255]);
        assert_eq!(dst.pixels[7], [0, 0, 255, 255]);
        // Filtered across the edge in the middle.
        let mid = dst.pixels[4];
        assert!(mid[0] > 0 && mid[2] > 0, "{mid:?}");
    }

    #[test]
    fn the_switch_follows_the_command_line_then_the_options() {
        let mut o = Options::default();
        assert!(enabled(&o), "on by default");
        o.set(OPTION_KEY, 0);
        assert!(!enabled(&o));
        set_override(Some(true));
        assert!(enabled(&o));
        set_override(None);
        assert_eq!(parse_mode("original"), Some(false));
    }
}
