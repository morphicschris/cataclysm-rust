//! User-requested modern marks over the main view, each with an option to go back to the
//! original:
//!
//! * Order lines (`hwcOrderLines`, `--order-lines`): a dashed line from each selected ship to what it
//!   was ordered to (the move's destination, the ship it attacks, guards, docks with or works
//!   on, the resource it harvests) with a circle there, coloured by the order.
//! * Ship icons (`hwcShipIcons`, `--ship-icons`): the class outline icons of `Tactical.script`
//!   (triangle fighters, square corvettes, rhombus frigates...) over every ship, over the ships
//!   near the camera fading out with distance, or none (the original). Tab cycles them
//!   (`hwcTabShipIcons`; Shift+Tab is then the original's Toggle_Ship_Info).

use glam::Vec3;
use hwc_sim::command::{attr, order};
use hwc_sim::object::sof;
use hwc_sim::statics::ShipClass;
use hwc_sim::{ShipId, Universe};
use hwc_ui::canvas::{Canvas, Color};

use crate::host::PLAYER;
use crate::sensors::Icons;
use crate::world::World;

/// Which ships carry a class icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    /// None: the original.
    Off,
    /// Ships near the camera, fading out between [`NEAR_FULL`] and [`NEAR_GONE`].
    Near,
    /// Every ship the view shows.
    All,
}

impl IconMode {
    /// The `hwcShipIcons` value: 0 off, 1 near, 2 all.
    pub fn from_option(v: i32) -> IconMode {
        match v {
            0 => IconMode::Off,
            1 => IconMode::Near,
            _ => IconMode::All,
        }
    }

    pub fn to_option(self) -> i32 {
        match self {
            IconMode::Off => 0,
            IconMode::Near => 1,
            IconMode::All => 2,
        }
    }

    /// The next mode Tab gives: all, near, off, all...
    pub fn next(self) -> IconMode {
        match self {
            IconMode::All => IconMode::Near,
            IconMode::Near => IconMode::Off,
            IconMode::Off => IconMode::All,
        }
    }

    /// The message shown when Tab changes it.
    pub fn message(self) -> &'static str {
        match self {
            IconMode::All => "Ship icons: all ships",
            IconMode::Near => "Ship icons: nearby ships",
            IconMode::Off => "Ship icons: off",
        }
    }
}

/// Near icons are drawn whole up to this distance from the eye...
pub const NEAR_FULL: f32 = 8000.0;
/// ...and are gone at this one (the render list's reach for most classes, `tweak.script`).
pub const NEAR_GONE: f32 = 16000.0;

/// What an order line points at, and its colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderKind {
    Move,
    Attack,
    Harvest,
    Dock,
    Guard,
    /// Salvage, repair and the other targeted specials.
    Special,
}

impl OrderKind {
    pub fn color(self) -> Color {
        match self {
            OrderKind::Move => [40, 230, 40, 255],
            OrderKind::Attack => [255, 40, 40, 255],
            OrderKind::Harvest => [255, 220, 0, 255],
            OrderKind::Dock => [0, 150, 255, 255],
            OrderKind::Guard => [120, 220, 255, 255],
            OrderKind::Special => [200, 90, 255, 255],
        }
    }
}

/// Where an order line ends.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderTarget {
    Point(Vec3),
    Ship(ShipId),
    /// A resource at this point.
    Resource(Vec3),
}

fn v3(v: hwc_math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn alive(u: &Universe, id: ShipId) -> bool {
    u.ship(id).is_some_and(|s| s.flags & sof::DEAD == 0)
}

/// The order a ship is carrying out and what it points at, if it has a line.
pub fn order_of(u: &Universe, id: ShipId) -> Option<(OrderKind, OrderTarget)> {
    let s = u.ship(id)?;
    let c = u.commands.get(s.command?)?;
    if c.attributes & attr::PROTECTING != 0 {
        if let Some(&p) = c.protect.iter().find(|&&p| p != id && alive(u, p)) {
            return Some((OrderKind::Guard, OrderTarget::Ship(p)));
        }
    }
    let ship = |k, t: Option<ShipId>| {
        t.filter(|&t| alive(u, t))
            .map(|t| (k, OrderTarget::Ship(t)))
    };
    match c.order {
        order::MOVE => {
            let i = usize::try_from(c.waypoint_index).ok()?;
            let p = v3(*c.waypoints.get(i)?);
            // A ship that has arrived has nothing to show.
            let r = u
                .statics
                .ship(s.ship_type)
                .map_or(50.0, |st| st.header.coll.coll_sphere_size);
            ((p - v3(s.posinfo.position)).length() > r)
                .then_some((OrderKind::Move, OrderTarget::Point(p)))
        }
        order::ATTACK => ship(
            OrderKind::Attack,
            s.attack
                .target
                .filter(|&t| alive(u, t))
                .or_else(|| c.attack_targets.iter().copied().find(|&t| alive(u, t))),
        ),
        order::COLLECT_RESOURCE => {
            let r = u.base.resources.get(c.collect.resource?)?;
            Some((OrderKind::Harvest, OrderTarget::Resource(v3(r.position))))
        }
        order::DOCK => ship(OrderKind::Dock, s.base.dock_with),
        order::SPECIAL => ship(
            OrderKind::Special,
            c.special_targets.iter().copied().find(|&t| alive(u, t)),
        ),
        _ => None,
    }
}

/// Clips a segment to the rectangle `[0, w) x [0, h)` (Liang-Barsky).
fn clip(a: [f32; 2], b: [f32; 2], w: f32, h: f32) -> Option<([f32; 2], [f32; 2])> {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for (p, q) in [
        (-dx, a[0]),
        (dx, w - 1.0 - a[0]),
        (-dy, a[1]),
        (dy, h - 1.0 - a[1]),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return None;
            }
        } else {
            let t = q / p;
            if p < 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
        }
    }
    (t0 <= t1).then(|| {
        (
            [a[0] + t0 * dx, a[1] + t0 * dy],
            [a[0] + t1 * dx, a[1] + t1 * dy],
        )
    })
}

fn line(canvas: &mut Canvas, a: [f32; 2], b: [f32; 2], c: Color) {
    line_pattern(canvas, a, b, c, 0xffff);
}

fn line_pattern(canvas: &mut Canvas, a: [f32; 2], b: [f32; 2], c: Color, pattern: u16) {
    let (w, h) = (canvas.width as f32, canvas.height as f32);
    if let Some((a, b)) = clip(a, b, w, h) {
        let (x0, y0, x1, y1) = (a[0] as i32, a[1] as i32, b[0] as i32, b[1] as i32);
        canvas.line_blend_stippled(x0, y0, x1, y1, c, pattern);
    }
}

/// The order's colour with dashes of a paler tint over it, as HW2's order lines.
fn order_line(canvas: &mut Canvas, a: [f32; 2], b: [f32; 2], c: Color) {
    let pale = [c[0] / 2 + 128, c[1] / 2 + 128, c[2] / 2 + 128, c[3]];
    line(canvas, a, b, c);
    line_pattern(canvas, a, b, pale, 0x0f0f);
}

fn circle(canvas: &mut Canvas, centre: [f32; 2], r: f32, c: Color) {
    let n = ((r * 0.75) as usize).clamp(12, 48);
    let at = |k: usize| {
        let t = k as f32 / n as f32 * std::f32::consts::TAU;
        [centre[0] + r * t.cos(), centre[1] + r * t.sin()]
    };
    for k in 0..n {
        line(canvas, at(k), at(k + 1), c);
    }
}

/// A ring of this world radius lying flat (in the plane of the universe) around `p`, as a
/// doubled band; a circle of `min_px` on screen when the ring would be smaller.
fn ring(
    canvas: &mut Canvas,
    world: &World,
    size: (u32, u32),
    p: Vec3,
    radius: f32,
    min_px: f32,
    c: Color,
) {
    let Some(centre) = world.project(size, p) else {
        return;
    };
    const N: usize = 40;
    for band in [1.0f32, 0.9] {
        let pts: Option<Vec<[f32; 2]>> = (0..N)
            .map(|k| {
                let t = k as f32 / N as f32 * std::f32::consts::TAU;
                world.project(size, p + Vec3::new(t.cos(), t.sin(), 0.0) * radius * band)
            })
            .collect();
        let Some(pts) = pts else { return };
        let extent = pts
            .iter()
            .map(|q| (q[0] - centre[0]).abs().max((q[1] - centre[1]).abs()))
            .fold(0.0f32, f32::max);
        if extent < min_px {
            circle(canvas, centre, min_px * band, c);
            continue;
        }
        for k in 0..N {
            line(canvas, pts[k], pts[(k + 1) % N], c);
        }
    }
}

/// The order lines of the selected ships (main view), drawn under the health bars.
pub fn draw_order_lines(
    canvas: &mut Canvas,
    world: &World,
    size: (u32, u32),
    selection: &[ShipId],
) {
    let u = world.universe();
    let min_px = (size.1 as f32 / 1080.0 * 7.0).max(5.0);
    let mut ends: Vec<(OrderKind, OrderTarget)> = Vec::new();
    for &id in selection {
        let Some(s) = u.ship(id) else { continue };
        if s.player != PLAYER || s.flags & (sof::DEAD | sof::HIDE) != 0 {
            continue;
        }
        let Some((kind, target)) = order_of(u, id) else {
            continue;
        };
        let to = match target {
            OrderTarget::Point(p) | OrderTarget::Resource(p) => p,
            OrderTarget::Ship(t) => v3(u.ship(t).expect("alive").posinfo.position),
        };
        let from = v3(s.posinfo.position);
        let (Some(a), Some(b)) = (world.project(size, from), world.project(size, to)) else {
            continue;
        };
        let mut c = kind.color();
        c[3] = 170;
        order_line(canvas, a, b, c);
        if !ends.contains(&(kind, target)) {
            ends.push((kind, target));
        }
    }
    // One ring per target, however many ships go there.
    for (kind, target) in ends {
        let mut c = kind.color();
        c[3] = 230;
        let (p, radius) = match target {
            OrderTarget::Point(p) => (p, 120.0),
            OrderTarget::Resource(p) => (p, 160.0),
            OrderTarget::Ship(t) => {
                let Some(s) = u.ship(t) else { continue };
                let r = u
                    .statics
                    .ship(s.ship_type)
                    .map_or(50.0, |st| st.header.coll.coll_sphere_size);
                (v3(s.posinfo.position), r * 1.3)
            }
        };
        ring(canvas, world, size, p, radius, min_px, c);
    }
}

/// Whether the main view shows a ship to the player (an icon goes on it).
fn shown(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ship(id) else { return false };
    if s.flags & (sof::DEAD | sof::HIDE) != 0 {
        return false;
    }
    // Modules and other parts are drawn with the ship they belong to.
    let st = u.statics.ship(s.ship_type);
    if st.is_some_and(|st| st.ship_class == Some(ShipClass::Component)) {
        return false;
    }
    let ally = s.player == PLAYER || u.players.get(PLAYER).is_some_and(|p| p.is_ally(s.player));
    ally || !hwc_sim::cloak::is_cloaked(s)
}

fn icon_color(u: &Universe, player: usize) -> Color {
    if player == PLAYER {
        [0, 255, 0, 255]
    } else if u.players.get(PLAYER).is_some_and(|p| p.is_ally(player)) {
        [0, 192, 255, 255]
    } else {
        [255, 40, 40, 255]
    }
}

/// The opacity of a near icon at this distance from the eye (1 up to [`NEAR_FULL`], 0 from
/// [`NEAR_GONE`]).
pub fn near_fade(distance: f32) -> f32 {
    (1.0 - (distance - NEAR_FULL) / (NEAR_GONE - NEAR_FULL)).clamp(0.0, 1.0)
}

/// The class icons over the main view's ships.
pub fn draw_icons(
    canvas: &mut Canvas,
    world: &World,
    size: (u32, u32),
    icons: &Icons,
    mode: IconMode,
) {
    if mode == IconMode::Off {
        return;
    }
    let u = world.universe();
    let eye = world.camera.eye();
    let half = (size.1 as f32 / 1080.0 * 9.0).max(6.0);
    for &id in &u.ship_list {
        if !shown(u, id) {
            continue;
        }
        let s = u.ship(id).expect("shown");
        let Some(mut class) = u.statics.ship(s.ship_type).and_then(|st| st.ship_class) else {
            continue;
        };
        // The script has no icon for the monster ships; they are drawn as motherships.
        if class == ShipClass::MonsterShip {
            class = ShipClass::Mothership;
        }
        let p = v3(s.coll.coll_position);
        let alpha = match mode {
            IconMode::Near => near_fade((p - eye).length()),
            _ => 1.0,
        };
        if alpha <= 0.0 {
            continue;
        }
        let Some(c) = world.project(size, p) else {
            continue;
        };
        if c[0] < -half
            || c[1] < -half
            || c[0] > size.0 as f32 + half
            || c[1] > size.1 as f32 + half
        {
            continue;
        }
        // The script's non-combat icon is the corvette's square with a mast; one that did not
        // parse whole falls back to the corvette's.
        let verts = icons
            .by_class
            .get(class.index())
            .filter(|v| v.len() >= 3)
            .or_else(|| icons.by_class.get(ShipClass::Corvette.index()))
            .filter(|v| v.len() >= 2);
        let Some(verts) = verts else { continue };
        let mut col = icon_color(u, s.player);
        col[3] = (200.0 * alpha) as u8;
        // Capital ships get a larger icon.
        let big = matches!(
            class,
            ShipClass::Mothership
                | ShipClass::HeavyCruiser
                | ShipClass::Carrier
                | ShipClass::Destroyer
        );
        let k = if big { half * 2.6 } else { half * 2.0 };
        let pts: Vec<[f32; 2]> = verts
            .iter()
            .map(|v| [c[0] + v[0] * k, c[1] - v[1] * k])
            .collect();
        for i in 0..pts.len() {
            line(canvas, pts[i], pts[(i + 1) % pts.len()], col);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_cycles_all_near_off() {
        let mut m = IconMode::All;
        let mut seen = Vec::new();
        for _ in 0..3 {
            m = m.next();
            seen.push(m);
        }
        assert_eq!(seen, [IconMode::Near, IconMode::Off, IconMode::All]);
        for m in seen {
            assert_eq!(IconMode::from_option(m.to_option()), m);
        }
    }

    #[test]
    fn near_icons_fade_with_distance() {
        assert_eq!(near_fade(0.0), 1.0);
        assert_eq!(near_fade(NEAR_FULL), 1.0);
        assert!((near_fade((NEAR_FULL + NEAR_GONE) * 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(near_fade(NEAR_GONE), 0.0);
        assert_eq!(near_fade(1e6), 0.0);
    }

    #[test]
    fn lines_are_clipped_to_the_canvas() {
        let (a, b) = clip([-100.0, 50.0], [1e7, 50.0], 640.0, 480.0).unwrap();
        assert_eq!(a, [0.0, 50.0]);
        assert_eq!(b, [639.0, 50.0]);
        assert!(clip([-10.0, -10.0], [-5.0, 500.0], 640.0, 480.0).is_none());
    }
}
