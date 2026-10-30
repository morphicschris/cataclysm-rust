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
use hwc_sim::object::{sof, CommandId};
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
/// A world segment on screen, cut at the camera's near side first: a line whose end is
/// behind the camera (its target out of view after the camera turned) still shows the part in
/// front. None when all of it is behind.
fn project_segment(
    world: &World,
    size: (u32, u32),
    a: Vec3,
    b: Vec3,
) -> Option<([f32; 2], [f32; 2])> {
    segment_on_screen(world.view_proj(size), size, a, b)
}

fn segment_on_screen(
    m: glam::Mat4,
    size: (u32, u32),
    a: Vec3,
    b: Vec3,
) -> Option<([f32; 2], [f32; 2])> {
    const NEAR_W: f32 = 1.0;
    let (ca, cb) = (m * a.extend(1.0), m * b.extend(1.0));
    if ca.w < NEAR_W && cb.w < NEAR_W {
        return None;
    }
    let cut = |p: glam::Vec4, q: glam::Vec4| p + (q - p) * ((NEAR_W - p.w) / (q.w - p.w));
    let (ca, cb) = match (ca.w < NEAR_W, cb.w < NEAR_W) {
        (true, _) => (cut(ca, cb), cb),
        (_, true) => (ca, cut(cb, ca)),
        _ => (ca, cb),
    };
    let px = |c: glam::Vec4| {
        let n = c.truncate() / c.w;
        [
            (n.x + 1.0) * 0.5 * size.0 as f32,
            (1.0 - n.y) * 0.5 * size.1 as f32,
        ]
    };
    Some((px(ca), px(cb)))
}

/// Ships of different orders are this close (world units) to a group's first ship, and their
/// targets to its target, to share its order line.
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
    command: Option<CommandId>,
    first_from: Vec3,
    first_to: Vec3,
}

/// One ship's order line before grouping: the order, its target, the command it belongs to,
/// the ship's position and the target's.
pub type OrderLine = (OrderKind, OrderTarget, Option<CommandId>, Vec3, Vec3);

/// Puts the ships' order lines together, from the ships' centre to the targets' centre: the
/// lines of one command (a formation, a group given one order, each ship on its own target)
/// always, and lines of the same order from ships near each other to ends near each other
/// (within [`GROUP_REACH`]). Eight fighters in formation give one line and one ring.
pub fn group_orders(lines: &[OrderLine]) -> Vec<OrderGroup> {
    let mut groups: Vec<OrderGroup> = Vec::new();
    let mut members: Vec<Vec<Vec3>> = Vec::new();
    for &(kind, target, command, from, to) in lines {
        let joins = |g: &OrderGroup| {
            let same_kind_of_end = matches!(
                (g.target, target),
                (OrderTarget::Ship(_), OrderTarget::Ship(_))
                    | (OrderTarget::Point(_), OrderTarget::Point(_))
                    | (OrderTarget::Resource(_), OrderTarget::Resource(_))
            );
            g.kind == kind
                && same_kind_of_end
                && ((command.is_some() && g.command == command)
                    || ((from - g.first_from).length() <= GROUP_REACH
                        && (to - g.first_to).length() <= GROUP_REACH))
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
                    command,
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
        lines.push((kind, target, s.command, v3(s.posinfo.position), to));
    }
    for g in group_orders(&lines) {
        let Some((a, b)) = project_segment(world, size, g.from, g.to) else {
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
    // Slipgates (static capability 0x4000) get no overlay mark (`toAllShipsDraw`).
    if st.is_some_and(|st| st.capability_flags & hwc_sim::dock::CAP_SLIPGATE != 0) {
        return false;
    }
    sensors::ship_visible(u, fog, id)
}

/// Whether a ship carries a class icon: a shown ship that is not a crystal. `toAllShipsDraw`
/// (0x523b50) gives crystals no overlay mark (`CrystalIsCrystal`, 0x5aaa70: ship types
/// dCrystal0/1); the sensors manager still draws them (`smShipsDraw`).
fn icon_shown(u: &Universe, fog: &Fog, id: ShipId) -> bool {
    shown(u, fog, id) && !hwc_sim::ships::crystal::is_crystal(u, id)
}

/// `toAllShipsDraw`'s colours: own ships green, allies light blue, others red. Own ships a
/// script keeps out of the player's control (specialFlags2 0x100000) are drawn as allies:
/// [`crate::host::HANDED_OVER_COLOR`].
fn icon_color(u: &Universe, id: ShipId) -> Color {
    let handed_over = u.ship(id).is_some_and(crate::host::handed_over);
    if handed_over {
        crate::host::HANDED_OVER_COLOR
    } else if u.ship(id).is_some_and(|s| s.player == PLAYER) {
        [0, 255, 0, 255]
    } else if sensors::looks_allied(u, id) {
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
        if !icon_shown(u, &fog, id) {
            continue;
        }
        let s = u.ship(id).expect("shown");
        let Some(mut class) = sensors::apparent_class(u, id) else {
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
        let mut col = icon_color(u, id);
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

    /// Over the real game data (`HWC_GAME_DIR`; skipped without it): crystals get no class
    /// icon (`toAllShipsDraw` leaves them out), another player's ship does.
    #[test]
    fn crystals_get_no_icon() {
        use hwc_formats::big::BigArchive;
        use hwc_math::Vector;
        use hwc_sim::statics::{LoadOptions, ShipRace, ShipType, StaticWorld};
        let Ok(dir) = std::env::var("HWC_GAME_DIR") else {
            eprintln!("skipped: HWC_GAME_DIR not set");
            return;
        };
        let src = BigArchive::open(std::path::Path::new(&dir).join("cataclysm.big"))
            .expect("cataclysm.big");
        let statics = std::sync::Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = hwc_sim::SimTweaks::load(&src);
        let mut u = Universe::new(statics, tweaks, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        let mut spawn = |t: ShipType, x: f32| {
            hwc_sim::base::spawn_ship(&mut u, t, 1, Vector::new(x, 0.0, 0.0), None).expect("ship")
        };
        let crystal = spawn(ShipType::dCrystal0, 0.0);
        let big_crystal = spawn(ShipType::dCrystal1, 2000.0);
        let enemy = spawn(ShipType::sAcolyte, 4000.0);
        let fog = Fog::of(&u);
        for id in [crystal, big_crystal] {
            assert!(shown(&u, &fog, id), "a crystal is in view");
            assert!(!icon_shown(&u, &fog, id), "a crystal has an icon");
        }
        assert!(
            icon_shown(&u, &fog, enemy),
            "another player's ship has no icon"
        );
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
    fn a_formation_gives_one_line() {
        let to = Vec3::new(5000.0, 0.0, 0.0);
        // Eight ships moving on their own commands, close together.
        let mut lines: Vec<OrderLine> = (0..8)
            .map(|i| {
                let off = Vec3::new(0.0, i as f32 * 60.0, 0.0);
                let cmd = Some(CommandId(i));
                (
                    OrderKind::Move,
                    OrderTarget::Point(to + off),
                    cmd,
                    off,
                    to + off,
                )
            })
            .collect();
        // Far away, another group with the same order.
        let far = Vec3::new(0.0, 20000.0, 0.0);
        lines.push((OrderKind::Move, OrderTarget::Point(to), None, far, to));
        // A formation attacking, each ship on its own target, spread wider than the reach.
        lines.extend((0..9u16).map(|i| {
            let at = Vec3::new(f32::from(i) * 400.0, 0.0, 0.0);
            let t = OrderTarget::Ship(ShipId(10 + i));
            (OrderKind::Attack, t, Some(CommandId(99)), at, to + at)
        }));
        let g = group_orders(&lines);
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].count, 8);
        assert!((g[0].from - Vec3::new(0.0, 210.0, 0.0)).length() < 1e-3);
        assert!((g[0].spread - 210.0).abs() < 1e-3);
        assert_eq!(g[1].count, 1);
        assert_eq!((g[2].kind, g[2].count), (OrderKind::Attack, 9));
    }

    #[test]
    fn a_line_to_a_point_behind_the_camera_still_shows() {
        // Camera at the origin looking down -z.
        let m = glam::Mat4::perspective_rh(1.0, 4.0 / 3.0, 10.0, 1e5)
            * glam::Mat4::look_at_rh(Vec3::ZERO, Vec3::NEG_Z, Vec3::Y);
        let size = (640, 480);
        let ship = Vec3::new(100.0, 0.0, -1000.0);
        let behind = Vec3::new(100.0, 0.0, 2000.0);
        let (a, b) = segment_on_screen(m, size, ship, behind).expect("drawn");
        assert!((a[0] - 320.0).abs() < 100.0 && (a[1] - 240.0).abs() < 1.0);
        // The cut end runs off the right of the screen, along the line.
        assert!(b[0] > 640.0 && (b[1] - 240.0).abs() < 1.0);
        assert!(segment_on_screen(m, size, behind, behind * 2.0).is_none());
    }

    #[test]
    fn lines_are_clipped_to_the_canvas() {
        let (a, b) = clip([-100.0, 50.0], [1e7, 50.0], 640.0, 480.0).unwrap();
        assert_eq!(a, [0.0, 50.0]);
        assert_eq!(b, [639.0, 50.0]);
        assert!(clip([-10.0, -10.0], [-5.0, 500.0], 640.0, 480.0).is_none());
    }
}
