//! User-requested modern marks over the main view, each with an option to go back to the
//! original:
//!
//! * Order lines (`hwcOrderLines`, `--order-lines`): a dashed line from each selected ship to what it
//!   was ordered to (the move's destination, the ship it attacks, guards, docks with or works
//!   on, the resource it harvests) with a circle there, coloured by the order.
//! * Ship icons (`hwcShipIcons`, `--ship-icons`): the class outline icons of `Tactical.script`
//!   (triangle fighters, square corvettes, rhombus frigates...) over every ship, over the ships
//!   away from the camera (fading out as they come close), or none (the original). Tab cycles them
//!   (`hwcTabShipIcons`; Shift+Tab is then the original's Toggle_Ship_Info).

use glam::Vec3;
use hwc_sim::command::{attr, order};
use hwc_sim::object::sof;
use hwc_sim::statics::ShipClass;
use hwc_sim::{ShipId, Universe};
use hwc_ui::canvas::{Canvas, Color};

use crate::host::PLAYER;
use crate::sensors::{self, Fog, Icons};
use crate::world::World;

/// Which ships carry a class icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconMode {
    /// None: the original.
    Off,
    /// Ships away from the camera: none up to [`FAR_HIDDEN`] from the eye, fading in to
    /// whole at [`FAR_FULL`].
    Far,
    /// Every ship the view shows.
    All,
}

impl IconMode {
    /// The `hwcShipIcons` value: 0 off, 1 far, 2 all.
    pub fn from_option(v: i32) -> IconMode {
        match v {
            0 => IconMode::Off,
            1 => IconMode::Far,
            _ => IconMode::All,
        }
    }

    pub fn to_option(self) -> i32 {
        match self {
            IconMode::Off => 0,
            IconMode::Far => 1,
            IconMode::All => 2,
        }
    }

    /// The next mode Tab gives: all, far, off, all...
    pub fn next(self) -> IconMode {
        match self {
            IconMode::All => IconMode::Far,
            IconMode::Far => IconMode::Off,
            IconMode::Off => IconMode::All,
        }
    }

    /// The message shown when Tab changes it.
    pub fn message(self) -> &'static str {
        match self {
            IconMode::All => "Ship icons: all ships",
            IconMode::Far => "Ship icons: distant ships",
            IconMode::Off => "Ship icons: off",
        }
    }
}

/// Far icons are not drawn on ships closer to the eye than this...
pub const FAR_HIDDEN: f32 = 4000.0;
/// ...and are drawn whole from this distance on.
pub const FAR_FULL: f32 = 10000.0;

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
/// Ships in a group are this close (world units) to the group's first ship, and their targets
/// to its target, to share one order line.
pub const GROUP_REACH: f32 = 800.0;

/// One order line: its order, where it ends, and the ships' centre and the target's.
#[derive(Debug, Clone, PartialEq)]
pub struct OrderGroup {
    pub kind: OrderKind,
    pub target: OrderTarget,
    pub from: Vec3,
    pub to: Vec3,
    /// How far the targets spread from `to` (the ring grows to cover them).
    pub spread: f32,
    count: usize,
    first_from: Vec3,
    first_to: Vec3,
}

/// Puts the ships' order lines together: lines of the same order whose ships are near each
/// other and whose ends are near each other (the same ship, or points within
/// [`GROUP_REACH`]) become one, from the ships' centre to the targets' centre. Eight fighters
/// moving in formation give one line and one ring.
pub fn group_orders(lines: &[(OrderKind, OrderTarget, Vec3, Vec3)]) -> Vec<OrderGroup> {
    let mut groups: Vec<OrderGroup> = Vec::new();
    let mut members: Vec<Vec<Vec3>> = Vec::new();
    for &(kind, target, from, to) in lines {
        let joins = |g: &OrderGroup| {
            g.kind == kind
                && (from - g.first_from).length() <= GROUP_REACH
                && match (g.target, target) {
                    (OrderTarget::Ship(a), OrderTarget::Ship(b)) => a == b,
                    (OrderTarget::Ship(_), _) | (_, OrderTarget::Ship(_)) => false,
                    _ => (to - g.first_to).length() <= GROUP_REACH,
                }
        };
        match groups.iter().position(joins) {
            Some(i) => {
                let g = &mut groups[i];
                let n = g.count as f32;
                g.from = (g.from * n + from) / (n + 1.0);
                g.to = (g.to * n + to) / (n + 1.0);
                g.count += 1;
                members[i].push(to);
            }
            None => {
                groups.push(OrderGroup {
                    kind,
                    target,
                    from,
                    to,
                    spread: 0.0,
                    count: 1,
                    first_from: from,
                    first_to: to,
                });
                members.push(vec![to]);
            }
        }
    }
    for (g, ends) in groups.iter_mut().zip(&members) {
        g.spread = ends
            .iter()
            .map(|&e| (e - g.to).length())
            .fold(0.0, f32::max);
    }
    groups
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
    let fog = Fog::of(u);
    let mut lines = Vec::new();
    for &id in selection {
        let Some(s) = u.ship(id) else { continue };
        if s.player != PLAYER || s.flags & (sof::DEAD | sof::HIDE) != 0 {
            continue;
        }
        let Some((kind, target)) = order_of(u, id) else {
            continue;
        };
        // No line to a ship the player cannot see now (out of sensor range, cloaked).
        if let OrderTarget::Ship(t) = target {
            if !shown(u, &fog, t) {
                continue;
            }
        }
        let to = match target {
            OrderTarget::Point(p) | OrderTarget::Resource(p) => p,
            OrderTarget::Ship(t) => v3(u.ship(t).expect("alive").posinfo.position),
        };
        lines.push((kind, target, v3(s.posinfo.position), to));
    }
    for g in group_orders(&lines) {
        let (Some(a), Some(b)) = (world.project(size, g.from), world.project(size, g.to)) else {
            continue;
        };
        let mut c = g.kind.color();
        c[3] = 170;
        order_line(canvas, a, b, c);
        c[3] = 230;
        let radius = match g.target {
            OrderTarget::Point(_) => 120.0,
            OrderTarget::Resource(_) => 160.0,
            OrderTarget::Ship(t) => {
                let r = u
                    .ship(t)
                    .and_then(|s| u.statics.ship(s.ship_type))
                    .map_or(50.0, |st| st.header.coll.coll_sphere_size);
                r * 1.3
            }
        };
        ring(canvas, world, size, g.to, radius + g.spread, min_px, c);
    }
}

/// Whether the main view shows a ship to the player (an icon goes on it).
/// Others' ships follow the sensors' rules (`smEnemyShipVisible`): not cloaked and in a part
/// of the map the player's ships see now, so an icon never shows a ship the original hides.
fn shown(u: &Universe, fog: &Fog, id: ShipId) -> bool {
    let Some(s) = u.ship(id) else { return false };
    if s.flags & (sof::DEAD | sof::HIDE) != 0 {
        return false;
    }
    // Modules and other parts are drawn with the ship they belong to.
    let st = u.statics.ship(s.ship_type);
    if st.is_some_and(|st| st.ship_class == Some(ShipClass::Component)) {
        return false;
    }
    sensors::ship_visible(u, fog, id)
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

/// The opacity of a far icon at this distance from the eye (0 up to [`FAR_HIDDEN`], 1 from
/// [`FAR_FULL`]).
pub fn far_fade(distance: f32) -> f32 {
    ((distance - FAR_HIDDEN) / (FAR_FULL - FAR_HIDDEN)).clamp(0.0, 1.0)
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
    let fog = Fog::of(u);
    let eye = world.camera.eye();
    let half = (size.1 as f32 / 1080.0 * 9.0).max(6.0);
    for &id in &u.ship_list {
        if !shown(u, &fog, id) {
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
            IconMode::Far => far_fade((p - eye).length()),
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
        assert_eq!(seen, [IconMode::Far, IconMode::Off, IconMode::All]);
        for m in seen {
            assert_eq!(IconMode::from_option(m.to_option()), m);
        }
    }

    #[test]
    fn far_icons_fade_out_close_to_the_camera() {
        assert_eq!(far_fade(0.0), 0.0);
        assert_eq!(far_fade(FAR_HIDDEN), 0.0);
        assert!((far_fade((FAR_HIDDEN + FAR_FULL) * 0.5) - 0.5).abs() < 1e-6);
        assert_eq!(far_fade(FAR_FULL), 1.0);
        assert_eq!(far_fade(1e6), 1.0);
    }

    #[test]
    fn a_formation_moving_to_a_point_gives_one_line() {
        let to = Vec3::new(5000.0, 0.0, 0.0);
        let mut lines: Vec<_> = (0..8)
            .map(|i| {
                let off = Vec3::new(0.0, i as f32 * 60.0, 0.0);
                (OrderKind::Move, OrderTarget::Point(to + off), off, to + off)
            })
            .collect();
        // Far away, another group with the same order; and an attack from the formation.
        let far = Vec3::new(0.0, 20000.0, 0.0);
        lines.push((OrderKind::Move, OrderTarget::Point(to), far, to));
        lines.push((
            OrderKind::Attack,
            OrderTarget::Ship(ShipId(3)),
            Vec3::ZERO,
            to,
        ));
        let g = group_orders(&lines);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].count, 8);
        assert!((g[0].from - Vec3::new(0.0, 210.0, 0.0)).length() < 1e-3);
        assert!((g[0].spread - 210.0).abs() < 1e-3);
        assert_eq!(g[1].count, 1);
        assert_eq!(g[2].kind, OrderKind::Attack);
    }

    #[test]
    fn lines_are_clipped_to_the_canvas() {
        let (a, b) = clip([-100.0, 50.0], [1e7, 50.0], 640.0, 480.0).unwrap();
        assert_eq!(a, [0.0, 50.0]);
        assert_eq!(b, [639.0, 50.0]);
        assert!(clip([-10.0, -10.0], [-5.0, 500.0], 640.0, 480.0).is_none());
    }
}
