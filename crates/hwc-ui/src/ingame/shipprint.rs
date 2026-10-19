//! The ship info panel (`ShipPrint.c`, Toggle_Ship_Info): type and rank, upgrades coloured by
//! state, class, health, armour level and guns of the first selected ship. It can be dragged
//! while the overlay options key is held, and has its own option menu then.

use crate::canvas::{Canvas, Color, Rect, BLACK, WHITE};
use crate::ingame::draw;
use crate::ingame::host::*;
use crate::ingame::overlay::{health_colour, BACKDROP};
use crate::ingame::Fe;

pub const FONT: &str = "default.hff";
const GREEN: Color = [0, 255, 0, 255];
const YELLOW: Color = [255, 255, 0, 255];
const RED: Color = [255, 0, 0, 255];

/// Upgrade names in panel order with their bit (`0x889718`).
pub const UPGRADES: [(&str, u32); 24] = [
    ("Advanced Fighter Drive", 0x1),
    ("Advanced Frigate Drive", 0x2),
    ("Advanced Destroyer Drive", 0x4),
    ("Linking", 0x8),
    ("Mimic", 0x20),
    ("Leeching", 0x40),
    ("Explosive Charge", 0x80),
    ("Afterburners", 0x100),
    ("EMP", 0x200),
    ("Repulse", 0x400),
    ("Swarming", 0x800),
    ("Force Field 1", 0x1000),
    ("Force Field 2", 0x2000),
    ("Force Field 3", 0x4000),
    ("Deflection", 0x8000),
    ("Infecting", 0x10000),
    ("Cloaking", 0x20000),
    ("Repairing", 0x40000),
    ("Salvaging", 0x80000),
    ("Crystal Processing", 0x100000),
    ("Self Repairing", 0x200000),
    ("Infection Vaccine", 0x400000),
    ("Advanced Sensors", 0x800000),
    ("Missile Ordnance", 0x4000000),
];

/// Option menu entries (keys 1-8) and their defaults.
pub const OPTIONS: [&str; 8] = [
    "GUNS",
    "ARMOR",
    "TECHNOLOGY THE SHIP HAS",
    "ALL TECHNOLOGY",
    "MAX TECHNOLOGY",
    "TECHNOLOGY THE SHIP NEEDS",
    "CLASS",
    "HEALTH",
];

#[derive(Debug, Clone)]
pub struct State {
    /// Toggle_Ship_Info.
    pub on: bool,
    pub x: i32,
    pub y: i32,
    pub options: [bool; 8],
    /// Drag in progress: mouse position at the press.
    drag: Option<(i32, i32)>,
    mouse: (i32, i32),
    /// Panel extent from the last draw.
    extent: Rect,
}

impl Default for State {
    fn default() -> Self {
        Self {
            on: false,
            x: 30,
            y: 200,
            options: [true, true, true, false, true, true, true, true],
            drag: None,
            mouse: (0, 0),
            extent: Rect::default(),
        }
    }
}

pub fn toggle(fe: &mut Fe) {
    let s = &mut fe.game.ig.ship_print;
    s.on = !s.on;
}

/// `sprShouldDisplay`: on, and the first selected ship is the player's.
pub fn visible(fe: &Fe) -> bool {
    fe.game.ig.ship_print.on
        && fe
            .game
            .ig
            .view
            .selection
            .first()
            .is_some_and(|s| s.owner != Owner::Enemy && s.owner != Owner::Ally)
}

/// `sprToggleOption` (digit while the options key is held).
pub fn toggle_option(fe: &mut Fe, digit: u8) {
    if let Some(o) = fe
        .game
        .ig
        .ship_print
        .options
        .get_mut(usize::from(digit).wrapping_sub(1))
    {
        *o = !*o;
    }
}

pub fn mouse_move(fe: &mut Fe, x: i32, y: i32) {
    fe.game.ig.ship_print.mouse = (x, y);
}

/// Dragging: press inside the panel while the options key is held; release moves it.
pub fn mouse_button(fe: &mut Fe, left: bool, down: bool, x: i32, y: i32) -> bool {
    if !left || !visible(fe) {
        return false;
    }
    let held = fe.game.ig.overlay.options_held;
    let s = &mut fe.game.ig.ship_print;
    if down {
        let r = Rect::new(s.x - 10, s.y - 10, s.extent.x1, s.extent.y1);
        if held && r.contains(x, y) {
            s.drag = Some((x, y));
            return true;
        }
        false
    } else if let Some((px, py)) = s.drag.take() {
        s.x += x - px;
        s.y += y - py;
        true
    } else {
        false
    }
}

/// `sprDraw`.
pub fn draw(fe: &mut Fe, canvas: &mut Canvas) {
    if !visible(fe) {
        return;
    }
    let Some(f) = fe.resources().font(FONT) else {
        return;
    };
    let ship = fe.game.ig.view.selection[0].clone();
    let rows = layout(fe, &f, &ship);
    let s = &fe.game.ig.ship_print;
    let (mut right, mut bottom) = (s.x, s.y);
    for (x, y, _, t) in &rows {
        right = right.max(x + draw::width(&f, t) + 10);
        bottom = bottom.max(y + crate::text::height(&f) + 10);
    }
    let panel = Rect::new(s.x - 10, s.y - 10, right, bottom);
    canvas.blend_rect(panel, BACKDROP);
    canvas.outline_rect(panel, WHITE);
    canvas.outline_rect(
        Rect::new(panel.x0 + 1, panel.y0 + 1, panel.x1 - 1, panel.y1 - 1),
        WHITE,
    );
    for (x, y, c, t) in rows {
        draw::print_shadow(canvas, &f, x, y, c, [0, 0, 0, 0x40], &t);
    }
    if let Some((px, py)) = s.drag {
        let (mx, my) = s.mouse;
        canvas.outline_rect(panel.offset(mx - px, my - py), WHITE);
    }
    fe.game.ig.ship_print.extent = panel;
    if fe.game.ig.overlay.options_held {
        draw_menu(fe, canvas, &f);
    }
}

/// The text runs of the panel: (x, y, colour, text).
fn layout(fe: &Fe, f: &hwc_formats::font::Font, ship: &ShipInfo) -> Vec<(i32, i32, Color, String)> {
    let s = &fe.game.ig.ship_print;
    let o = s.options;
    let h = crate::text::height(f);
    let (x, y) = (s.x, s.y);
    let mut out = vec![(x, y, WHITE, format!("{}   ({})", ship.name, ship.rank_name))];
    // Upgrades, left column.
    let possible = ship.upgrades_possible;
    let mut ups = Vec::new();
    for (name, bit) in UPGRADES {
        let c = if ship.upgrades & bit != 0 {
            o[2].then_some(GREEN)
        } else if possible & ship.upgrades_researched & bit != 0 {
            o[5].then_some(YELLOW)
        } else if possible & bit != 0 {
            (o[4] && fe.game.ig.view.player.single_player).then_some(RED)
        } else {
            o[3].then_some(WHITE)
        };
        if let Some(c) = c {
            ups.push((name, c));
        }
    }
    let indent = ups
        .iter()
        .map(|(n, _)| draw::width(f, n))
        .max()
        .unwrap_or(0)
        + 10;
    if !ups.is_empty() {
        out.push((x, y + 20, WHITE, "Technology".into()));
        for (i, (name, c)) in ups.into_iter().enumerate() {
            out.push((x, y + 20 + 2 * h + i as i32 * h, c, name.into()));
        }
    }
    let rx = x + indent;
    if o[6] {
        out.push((rx, y + 20, WHITE, format!("Class: {}", ship.class_name)));
    }
    if o[7] {
        let label = format!("Health: {:.0} / {:.0}", ship.health, ship.max_health);
        let w = draw::width(f, &label);
        let frac = if ship.max_health > 0.0 {
            ship.health / ship.max_health
        } else {
            0.0
        };
        out.push((rx, y + 35, WHITE, label));
        out.push((
            rx + w,
            y + 35,
            health_colour(frac),
            format!("   {:.0} %", frac * 100.0),
        ));
    }
    if o[1] {
        let c = if ship.tech_level_raisable {
            YELLOW
        } else {
            GREEN
        };
        out.push((
            rx,
            y + 50,
            c,
            format!("Armor Level {}", ship.tech_level.max(1)),
        ));
    }
    if o[0] {
        let gy = y + 70;
        if ship.guns.is_empty() {
            out.push((rx, gy, WHITE, "No Guns Available...".into()));
        } else {
            let cols = [rx, rx + 70, rx + 150, rx + 240];
            for (c, t) in cols.iter().zip(["Weapons:", "Size", "Type", "Damage"]) {
                out.push((*c, gy, WHITE, t.into()));
            }
            for (i, g) in ship.guns.iter().enumerate() {
                let ly = gy + (i as i32 + 1) * h;
                out.push((cols[0], ly, WHITE, format!("Gun {}:", i + 1)));
                out.push((cols[1], ly, WHITE, g.size.clone()));
                let tc = if g.upgrade_pending { YELLOW } else { GREEN };
                out.push((cols[2], ly, tc, g.kind.clone()));
                out.push((cols[3], ly, WHITE, format!("{:.0}", g.damage)));
            }
        }
    }
    out
}

/// `sprDrawToggleMenu`.
fn draw_menu(fe: &Fe, canvas: &mut Canvas, f: &hwc_formats::font::Font) {
    let h = crate::text::height(f);
    let w = draw::width(f, "(1) SHOW MAX TECHNOLOGY THE SHIP CAN HAVE DISABLED") + 50;
    let r = Rect::new(10, 10, w, 7 * h + 40);
    canvas.fill_rect(r, BLACK);
    canvas.outline_rect(r, WHITE);
    for (i, name) in OPTIONS.iter().enumerate() {
        let on = fe.game.ig.ship_print.options[i];
        let c = if on { GREEN } else { RED };
        let state = if on { "ENABLED" } else { "DISABLED" };
        draw::print(
            canvas,
            f,
            20,
            20 + i as i32 * h,
            c,
            &format!("({}) SHOW {name} {state}", i + 1),
        );
    }
}
