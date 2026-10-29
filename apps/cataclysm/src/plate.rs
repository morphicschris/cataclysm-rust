//! The move pie plate (`PiePlate.c` as Cataclysm has it: `piePointModeOnOff`,
//! `piePointModeToggle`, `piePointModePause`, `piePointSpecDraw`, `piePlaneDraw`,
//! `piePlaneDiscDraw`, `pieMovementCursorDraw`, `pieAllShipsToPiePlateDraw`,
//! `pieMovePointClipToLimits` and `pieDistanceReadoutDraw`).
//!
//! The plate lies on the horizontal plane through the middle of the selection. In the plane
//! mode the destination is where the pointer's ray meets that plane. Holding the height key
//! (Shift) switches to the height mode: the pointer is held on the screen image of the vertical
//! line through the plane point, and the destination is where the pointer's ray meets that
//! vertical, so the height follows the pointer exactly. Each switch puts the system pointer
//! back on the point being edited (the plane point, or the height point), and a height chosen
//! once is kept when the plane point moves again. While the camera turns (right button held)
//! the plate is paused: both points only follow the selection's drift.
//!
//! Drawn each frame: the disc (an outline through the plane point with 15° ticks, four spokes
//! with cross-hatches every 700 m), the plane point's circle, the stippled line from the middle,
//! with a height the vertical, the line back to the middle on the plane and the destination's
//! circle, the drop line of every visible ship to the plane with a circle where it meets it
//! (the closest one in its own colour, a chime when that changes), and the distance readout
//! beside the pointer.
//!
//! Placing waypoints (`piePointModeOnOff(4, mrWaypointAdd)`) uses the same plate, coloured by
//! the waypoint mode, with the path drawn over it (`pieWaypointPathDraw`): a white line from
//! the selection through the waypoints so far, and three yellow rings spinning round each.

use glam::{Mat4, Vec3, Vec4};
use hwc_formats::font::Font;
use hwc_formats::script::ScriptFile;
use hwc_sim::ShipId;
use hwc_ui::canvas::{Canvas, Color};
use hwc_ui::text::{self, Shadow};

/// UI sound events: the plate opens, closes (`mrMoveShipsExecute`, `mrMoveShips`), and the
/// pointer comes onto another ship's drop line.
pub const SOUND_ON: u32 = 0x116;
pub const SOUND_OFF: u32 = 0x117;
pub const SOUND_CHIMES: u32 = 0x118;

/// Below this height the destination counts as on the plane (`pieDottedDistance`).
const DOTTED_DISTANCE: f32 = 2.0;
/// The farthest the main view's plate reaches across and up (`pieMaxMoveHorizontal/Vertical`).
const MAX_MOVE_HORIZONTAL: f32 = 15_000.0;
const MAX_MOVE_VERTICAL: f32 = 15_000.0;
/// Times the camera's distance: how near a ship's foot must be to count as the closest one
/// (`pieShipLineTickSize`), never less than [`CLOSEST_DISTANCE`].
const SHIP_LINE_TICK_SIZE: f32 = 0.03;
const CLOSEST_DISTANCE: f32 = 200.0;
/// The moving stipple of the line from the middle (`PIE_LineStipple`, `pieLineStippleSpeed`).
const STIPPLE_PATTERN: u16 = 0xff00;
const STIPPLE_SPEED: f32 = 1.5;
/// Keeps a ray's slope pointing at the plane it is cut with (`PIE_PlanePointFudge`).
const PLANE_POINT_FUDGE: f32 = 1e-5;
/// Destinations stay this far inside the universe (`pieMovePointClipToLimits`).
const UNIVERSE_MARGIN: f32 = 8000.0;
/// At most this many drop-line circles a frame (`PIE_PlaneScreenPointIndex`).
const MAX_PLANE_POINTS: usize = 256;
/// The disc: outline segments, tick spacing and inner end, spokes and their hatches.
const DISC_SEGMENTS: i32 = 48;
const TICK_STEP: f32 = 0.261_799_4;
const TICK_INNER: f32 = 0.9;
const SPOKES: [(f32, f32); 4] = [(0.0, 1.0), (1.0, 0.0), (0.0, -1.0), (-1.0, 0.0)];
const HATCH_SPACING: f32 = 700.0;
const HATCH_HALF: f32 = 300.0;
/// The move line's colour for a plain move (`0x8836d4`).
const MOVE_LINE_COLOR: Color = [255, 255, 255, 255];
/// The move line's colour while placing waypoints, by mode: linear, circular, end to end
/// (`0x8836c4`, `0x8836c8`, `0x8836cc`).
pub const WAYPOINT_LINE_COLORS: [Color; 3] = [
    [0x91, 0xb5, 0xf7, 255],
    [0xac, 0x59, 0xcd, 255],
    [0x82, 0x79, 0xad, 255],
];
/// The waypoint path (`0x8836b8`) and its rings (`0x8836b4`), the rings' radius and slices
/// (`0x8836ac`, `0x8836b0`) and their spin in degrees a second (`0x8836bc`..`0x8836c0`).
const WAYPOINT_PATH_COLOR: Color = [255, 255, 255, 255];
const WAYPOINT_RING_COLOR: Color = [0xef, 0xec, 0x5d, 255];
const WAYPOINT_RING_RADIUS: f32 = 200.0;
const WAYPOINT_RING_SLICES: i32 = 32;
const WAYPOINT_SPIN: (f32, f32) = (10.0, 20.0);
/// Waypoints one order holds (`mrWaypointAdd`: 19, the rest overwrite a spare slot).
pub const MAX_WAYPOINTS: usize = 19;
/// The readout's offset from the pointer.
const READOUT_OFFSET: i32 = 10;

/// The plate's colours from `tweak.script`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Colors {
    /// The disc and the plane point's circle (`TW_MOVE_POINT_LINE_COLOR`).
    pub plane: Color,
    pub ship_line: Color,
    pub ship_line_closest: Color,
    pub readout: Color,
}

impl Default for Colors {
    fn default() -> Self {
        Colors {
            plane: [9, 143, 202, 255],
            ship_line: [233, 114, 41, 255],
            ship_line_closest: [255, 255, 22, 255],
            readout: [245, 58, 58, 255],
        }
    }
}

impl Colors {
    pub fn from_tweaks(t: Option<&ScriptFile>) -> Colors {
        let d = Colors::default();
        let rgb = |k: &str, d: Color| {
            t.and_then(|t| t.get(k))
                .and_then(|e| {
                    let c: Vec<i32> = e.values.iter().filter_map(|v| v.as_i32()).collect();
                    (c.len() >= 3).then(|| [c[0] as u8, c[1] as u8, c[2] as u8, 255])
                })
                .unwrap_or(d)
        };
        Colors {
            plane: rgb("TW_MOVE_POINT_LINE_COLOR", d.plane),
            ship_line: rgb("TW_SHIP_LINE_COLOR", d.ship_line),
            ship_line_closest: rgb("TW_SHIP_LINE_CLOSEST_COLOR", d.ship_line_closest),
            readout: rgb("TW_DISTANCE_READOUT_COLOR", d.readout),
        }
    }
}

/// The readout's font name from `tweak.script` (`TW_DISTANCE_READOUT_FONT`).
pub fn readout_font_name(t: Option<&ScriptFile>) -> String {
    t.and_then(|t| t.get("TW_DISTANCE_READOUT_FONT"))
        .and_then(|e| {
            e.values
                .first()
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| "HW_EuroseCond_11.hff".into())
}

/// The camera the plate is worked out and drawn with.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
    /// Viewport in pixels.
    pub size: (u32, u32),
}

impl View {
    fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }

    fn pixel(&self, c: Vec4) -> [f32; 2] {
        let n = c.truncate() / c.w;
        [
            (n.x + 1.0) * 0.5 * self.size.0 as f32,
            (1.0 - n.y) * 0.5 * self.size.1 as f32,
        ]
    }

    /// A world point in pixels (None behind the camera).
    pub fn project(&self, p: Vec3) -> Option<[f32; 2]> {
        let c = self.view_proj() * p.extend(1.0);
        (c.w > 1e-3).then(|| self.pixel(c))
    }

    /// The direction of the ray through a pixel (`cameraRayCast`).
    pub fn ray(&self, px: [f32; 2]) -> Vec3 {
        let inv = self.view_proj().inverse();
        let nx = px[0] / self.size.0 as f32 * 2.0 - 1.0;
        let ny = 1.0 - px[1] / self.size.1 as f32 * 2.0;
        let a = inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * Vec4::new(nx, ny, 1.0, 1.0);
        (b.truncate() / b.w - a.truncate() / a.w).normalize_or_zero()
    }

    /// The part of a segment inside the view, in pixels (`clipLineToScreen`).
    pub fn clip(&self, a: Vec3, b: Vec3) -> Option<([f32; 2], [f32; 2])> {
        let vp = self.view_proj();
        let (ca, cb) = (vp * a.extend(1.0), vp * b.extend(1.0));
        let d = cb - ca;
        let (mut t0, mut t1) = (0.0f32, 1.0f32);
        // Each plane as `dot(k, clip) >= 0`: in front, and the four sides.
        let planes = [
            (Vec4::new(0.0, 0.0, 0.0, 1.0), -1e-3),
            (Vec4::new(1.0, 0.0, 0.0, 1.0), 0.0),
            (Vec4::new(-1.0, 0.0, 0.0, 1.0), 0.0),
            (Vec4::new(0.0, 1.0, 0.0, 1.0), 0.0),
            (Vec4::new(0.0, -1.0, 0.0, 1.0), 0.0),
        ];
        for (k, off) in planes {
            let fa = k.dot(ca) + off;
            let fd = k.dot(d);
            if fd.abs() < 1e-12 {
                if fa < 0.0 {
                    return None;
                }
                continue;
            }
            let t = -fa / fd;
            if fd > 0.0 {
                t0 = t0.max(t);
            } else {
                t1 = t1.min(t);
            }
            if t0 > t1 {
                return None;
            }
        }
        Some((self.pixel(ca + d * t0), self.pixel(ca + d * t1)))
    }

    /// The radius in pixels of a sphere at `p` (`selCircleComputeGeneral`).
    pub fn screen_radius(&self, p: Vec3, r: f32) -> Option<f32> {
        let c = self.view * p.extend(1.0);
        let s = self.proj * c;
        let q = self.proj * (c + Vec4::new(r, 0.0, 0.0, 0.0));
        (s.w > 1e-3).then(|| (q.x - s.x) / s.w * 0.5 * self.size.0 as f32)
    }

    /// A 3D line, clipped to the view.
    fn line(&self, canvas: &mut Canvas, a: Vec3, b: Vec3, c: Color, stipple: u16) {
        if let Some((p, q)) = self.clip(a, b) {
            canvas.line_stippled(
                p[0].round() as i32,
                p[1].round() as i32,
                q[0].round() as i32,
                q[1].round() as i32,
                c,
                stipple,
            );
        }
    }

    /// `primCircleOutlineZ`: a circle on the horizontal plane through `centre`.
    fn circle(&self, canvas: &mut Canvas, centre: Vec3, r: f32, segments: i32, c: Color) {
        let n = segments.max(3);
        let at = |k: i32| {
            let a = std::f32::consts::TAU * k as f32 / n as f32;
            centre + Vec3::new(a.sin() * r, a.cos() * r, 0.0)
        };
        let mut last = at(0);
        for k in 1..=n {
            let p = at(k);
            self.line(canvas, last, p, c, 0xffff);
            last = p;
        }
    }

    /// `pieScreenSizeOfCircleCompute`: a world radius held between a sixth of the view's width
    /// and 8 pixels on screen, with its segment count.
    fn circle_size(&self, p: Vec3, r: f32) -> Option<(f32, i32)> {
        let mut px = self.screen_radius(p, r)?;
        let mut r = r;
        let max = self.size.0 as f32 / 6.0;
        let min = 8.0;
        if px > max {
            r *= max / px;
            px = max;
        }
        if px < min && px > 0.0 {
            r *= min / px;
            px = min;
        }
        Some((r, crate::pings::circle_segments(px)))
    }
}

/// The spinning rings of the waypoint path (`pieWaypointPathDraw`'s statics): each waypoint
/// slot turns about its own axis, picked once at random, by a random 10..20 degrees a second.
#[derive(Debug, Clone)]
pub struct WaypointRings {
    axes: [Vec3; MAX_WAYPOINTS + 1],
    angles: [f32; MAX_WAYPOINTS + 1],
    seed: u32,
}

impl Default for WaypointRings {
    fn default() -> Self {
        let mut r = WaypointRings {
            axes: [Vec3::ZERO; MAX_WAYPOINTS + 1],
            angles: [0.0; MAX_WAYPOINTS + 1],
            seed: 0x1234_5678,
        };
        for k in 0..r.axes.len() {
            r.axes[k] = Vec3::new(r.unit(), r.unit(), r.unit());
        }
        r
    }
}

impl WaypointRings {
    /// A random number in 0..1 (the drawing's own; nothing in the game depends on it).
    fn unit(&mut self) -> f32 {
        self.seed = self
            .seed
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        (self.seed >> 8) as f32 / (1u32 << 24) as f32
    }

    /// Draws the path from `centre` through `points` with three rings round each point, the
    /// rings turned on by `dt` seconds.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        view: &View,
        centre: Vec3,
        points: &[Vec3],
        dt: f32,
    ) {
        let mut from = centre;
        for (k, &p) in points.iter().enumerate().take(MAX_WAYPOINTS) {
            view.line(canvas, from, p, WAYPOINT_PATH_COLOR, 0xffff);
            from = p;
            let speed = WAYPOINT_SPIN.0 + (WAYPOINT_SPIN.1 - WAYPOINT_SPIN.0) * self.unit();
            let mut a = self.angles[k] + speed * dt;
            if a > 360.0 {
                a -= 360.0;
            }
            self.angles[k] = a;
            let turn =
                glam::Quat::from_axis_angle(self.axes[k].normalize_or_zero(), a.to_radians());
            for axis in 0..3 {
                let n = WAYPOINT_RING_SLICES;
                let at = |i: i32| {
                    let t = std::f32::consts::TAU * i as f32 / n as f32;
                    let (x, y) = (
                        t.cos() * WAYPOINT_RING_RADIUS,
                        t.sin() * WAYPOINT_RING_RADIUS,
                    );
                    let v = match axis {
                        0 => Vec3::new(0.0, x, y),
                        1 => Vec3::new(x, 0.0, y),
                        _ => Vec3::new(x, y, 0.0),
                    };
                    p + turn * v
                };
                let mut last = at(0);
                for i in 1..=n {
                    let q = at(i);
                    view.line(canvas, last, q, WAYPOINT_RING_COLOR, 0xffff);
                    last = q;
                }
            }
        }
    }
}

/// Which point the pointer moves (`PSM_XY`, `PSM_Z`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Spec {
    Plane,
    Height,
}

/// A ship the drop lines are drawn for.
#[derive(Debug, Clone, Copy)]
pub struct DropShip {
    pub id: ShipId,
    pub position: Vec3,
    pub radius: f32,
    pub selected: bool,
}

/// The plate's state while it is up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Plate {
    pub spec: Spec,
    /// The camera is turning (`PSM_Waiting`).
    pub paused: bool,
    /// The middle of the selection (`selCentrePoint`) and its ships' average size.
    pub centre: Vec3,
    pub average_size: f32,
    /// The destination's foot on the plate (`piePlanePoint`) and the destination
    /// (`pieHeightPoint`).
    pub plane: Vec3,
    pub height: Vec3,
    /// The height chosen above the plate (`piePointSpecZ`).
    pub spec_z: f32,
    /// Put the system pointer back on the point being edited next frame.
    pub mouse_reset: bool,
    /// The vertical through the plane point on screen: (top, bottom) in pixels.
    vertical: Option<([f32; 2], [f32; 2])>,
    /// The ship whose foot was closest to the plane point last frame.
    pub closest: Option<ShipId>,
    stipple_counter: f32,
    /// The move line's colour (`pieMovementCursorColor`, 0xad5478): white for a move, by
    /// waypoint mode while placing waypoints.
    pub line_color: Color,
}

impl Plate {
    /// `piePointModeOnOff` turning the plate on.
    pub fn new(centre: Vec3, average_size: f32) -> Plate {
        Plate {
            spec: Spec::Plane,
            paused: false,
            centre,
            average_size,
            plane: centre,
            height: centre,
            spec_z: 0.0,
            mouse_reset: false,
            vertical: None,
            closest: None,
            stipple_counter: 0.0,
            line_color: MOVE_LINE_COLOR,
        }
    }

    /// `piePointModeToggle`: the height key went down (`on`) or up.
    pub fn toggle(&mut self, on: bool) {
        if on {
            if self.spec == Spec::Plane {
                self.spec = Spec::Height;
                if self.spec_z == 0.0 {
                    self.height = self.plane;
                }
            }
        } else if self.spec == Spec::Height {
            self.spec = Spec::Plane;
        }
        self.mouse_reset = true;
    }

    /// `piePointModePause`: the camera starts or stops turning.
    pub fn pause(&mut self, on: bool) {
        if on {
            self.paused = true;
        } else if self.paused {
            self.paused = false;
            self.mouse_reset = true;
        }
    }

    /// Where the ships are sent (`piePointSpecExecute`): the plane point without a height.
    pub fn destination(&self) -> Vec3 {
        if self.spec_z == 0.0 {
            self.plane
        } else {
            self.height
        }
    }

    /// The stipple pattern this frame.
    fn stipple(&self) -> u16 {
        let shift = (self.stipple_counter as i32).rem_euclid(16) as u32;
        let p = u32::from(STIPPLE_PATTERN);
        (((p << 16) | p) >> shift) as u16
    }

    /// One frame of `piePointSpecDraw`'s point work. `pointer` is the game's pointer in pixels
    /// and is moved where the original moves the mouse; returns whether it was moved.
    /// `universe` is the universe's half size; `sensors` drops the main view's reach limits.
    pub fn update(
        &mut self,
        centre: Vec3,
        average_size: f32,
        view: &View,
        pointer: &mut (i32, i32),
        universe: Vec3,
        sensors: bool,
    ) -> bool {
        let mut moved = false;
        let drift = centre - self.centre;
        self.centre = centre;
        self.average_size = average_size;
        self.stipple_counter += STIPPLE_SPEED;
        let mut set = |pointer: &mut (i32, i32), p: [f32; 2]| {
            let q = (p[0].round() as i32, p[1].round() as i32);
            let inside =
                q.0 >= 0 && q.1 >= 0 && q.0 < view.size.0 as i32 && q.1 < view.size.1 as i32;
            if inside && q != *pointer {
                *pointer = q;
                moved = true;
            }
            inside
        };
        if self.mouse_reset && !self.paused {
            match self.spec {
                Spec::Plane => {
                    let (a, b) = (self.plane + drift, self.height + drift);
                    if let Some(p) = view.project(a) {
                        set(pointer, p);
                    }
                    let same = match (view.project(a), view.project(b)) {
                        (Some(p), Some(q)) => {
                            let r = |v: [f32; 2]| (v[0] as i32, v[1] as i32);
                            on_screen(view, p) && on_screen(view, q) && r(p) == r(q)
                        }
                        _ => false,
                    };
                    if same {
                        self.height = self.plane;
                        self.spec_z = 0.0;
                    }
                }
                Spec::Height => {
                    if let Some(p) = view.project(self.height + drift) {
                        set(pointer, p);
                    }
                }
            }
            self.mouse_reset = false;
        }
        if self.paused {
            self.plane += drift;
            self.height += drift;
        } else {
            match self.spec {
                Spec::Plane => {
                    let mut d = view.ray(px(*pointer));
                    if view.eye.z >= centre.z || d.z == 0.0 {
                        if d.z >= 0.0 {
                            d.z = -PLANE_POINT_FUDGE;
                        }
                    } else if d.z <= 0.0 {
                        d.z = PLANE_POINT_FUDGE;
                    }
                    if let Some(p) = cut(view.eye, d, 2, centre.z) {
                        self.plane = p;
                    }
                    self.height.z += drift.z;
                }
                Spec::Height => {
                    if let Some((top, bottom)) = self.vertical {
                        let y = (pointer.1 as f32).clamp(top[1], bottom[1]);
                        let dy = bottom[1] - top[1];
                        let x = if dy.abs() < 1e-3 {
                            top[0]
                        } else {
                            top[0] + (y - top[1]) * (bottom[0] - top[0]) / dy
                        };
                        set(pointer, [x, y]);
                    }
                    let mut d = view.ray(px(*pointer));
                    let foot = self.plane + drift;
                    // The vertical plane through the foot that faces the camera most.
                    let (axis, value) = if (view.eye.x - foot.x).abs() > (view.eye.y - foot.y).abs()
                    {
                        (0, foot.x)
                    } else {
                        (1, foot.y)
                    };
                    if view.eye[axis] >= value || d[axis] == 0.0 {
                        if d[axis] >= 0.0 {
                            d[axis] = -PLANE_POINT_FUDGE;
                        }
                    } else if d[axis] <= 0.0 {
                        d[axis] = PLANE_POINT_FUDGE;
                    }
                    if let Some(h) = cut(view.eye, d, axis, value) {
                        self.height.z = h.z;
                        self.spec_z = h.z - self.plane.z;
                    }
                    self.plane += drift;
                }
            }
        }
        self.height.x = self.plane.x;
        self.height.y = self.plane.y;
        let low = Vec3::new(self.plane.x, self.plane.y, -universe.z);
        let high = Vec3::new(self.plane.x, self.plane.y, universe.z);
        self.vertical = view
            .clip(low, high)
            .map(|(a, b)| if a[1] <= b[1] { (a, b) } else { (b, a) });
        if !sensors {
            let v = (self.height - centre).truncate();
            let len = v.length();
            if len > MAX_MOVE_HORIZONTAL {
                let p = centre.truncate() + v * (MAX_MOVE_HORIZONTAL / len);
                self.plane = p.extend(centre.z);
                self.height.x = p.x;
                self.height.y = p.y;
            }
            let dz = self.height.z - self.plane.z;
            if dz.abs() > MAX_MOVE_VERTICAL {
                self.height.z = self.plane.z + dz * MAX_MOVE_VERTICAL / dz.abs();
            }
        }
        clip_to_limits(universe, centre, &mut self.height);
        self.plane = Vec3::new(self.height.x, self.height.y, centre.z);
        moved
    }

    /// Draws the plate. `ships` are the drop lines' ships (None when the pieplate lines are
    /// off); `readout_at` is where the distance readout goes (the pointer), or None to put it
    /// by the destination (while the camera turns). Returns the chime to play.
    pub fn draw(
        &mut self,
        canvas: &mut Canvas,
        view: &View,
        colors: &Colors,
        ships: Option<&[DropShip]>,
        font: Option<&Font>,
        readout_at: Option<(i32, i32)>,
    ) -> Option<u32> {
        if let Some(font) = font {
            let at = readout_at.or_else(|| {
                view.project(self.height)
                    .map(|p| (p[0] as i32, p[1] as i32))
            });
            if let Some((x, y)) = at {
                let s = text::to_latin1(&distance_text(self.height.distance(self.centre)));
                text::print(
                    canvas,
                    font,
                    x + READOUT_OFFSET,
                    y + READOUT_OFFSET,
                    colors.readout,
                    &s,
                    (Shadow::None, [0, 0, 0, 255]),
                );
            }
        }
        // `piePlaneDraw`.
        let radius = self.plane.distance(self.centre);
        disc(canvas, view, self.centre, radius, colors.plane);
        if let Some((r, n)) = view.circle_size(self.plane, self.average_size) {
            view.circle(canvas, self.plane, r, n, colors.plane);
        }
        // `pieMovementCursorDraw`.
        let stipple = self.stipple();
        if self.spec_z.abs() >= DOTTED_DISTANCE {
            let c = self.line_color;
            view.line(canvas, self.height, self.centre, c, stipple);
            view.line(canvas, self.height, self.plane, c, 0xffff);
            view.line(canvas, self.plane, self.centre, c, 0xffff);
            if let Some((r, n)) = view.circle_size(self.height, self.average_size) {
                view.circle(canvas, self.height, r, n, c);
            }
        } else {
            view.line(canvas, self.plane, self.centre, self.line_color, stipple);
        }
        ships.and_then(|s| self.drop_lines(canvas, view, colors, s))
    }

    /// `pieAllShipsToPiePlateDraw`: each ship's line down (or up) to the plate, a circle where
    /// it meets the plate when the ship is off it, and the closest ship to the plane point.
    fn drop_lines(
        &mut self,
        canvas: &mut Canvas,
        view: &View,
        colors: &Colors,
        ships: &[DropShip],
    ) -> Option<u32> {
        let mut circles = 0;
        let mut best: Option<(f32, ShipId, bool, bool)> = None;
        for s in ships {
            let foot = Vec3::new(s.position.x, s.position.y, self.centre.z);
            let c = if Some(s.id) == self.closest {
                colors.ship_line_closest
            } else {
                colors.ship_line
            };
            view.line(canvas, foot, s.position, c, 0xffff);
            let mut circled = false;
            if (s.position.z - foot.z).abs() > s.radius && circles < MAX_PLANE_POINTS {
                if let Some(px) = view.screen_radius(foot, s.radius).filter(|&r| r > 0.0) {
                    circles += 1;
                    view.circle(canvas, foot, s.radius, crate::pings::circle_segments(px), c);
                    circled = true;
                }
            }
            let d = (self.plane.x - foot.x).abs() + (self.plane.y - foot.y).abs();
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, s.id, circled, s.selected));
            }
        }
        let near = (view.eye.distance(self.centre) * SHIP_LINE_TICK_SIZE).max(CLOSEST_DISTANCE);
        let best = best.filter(|b| b.0 <= near);
        let chime = best
            .filter(|&(_, id, circled, selected)| Some(id) != self.closest && !selected && circled)
            .map(|_| SOUND_CHIMES);
        self.closest = best.map(|b| b.1);
        chime
    }
}

fn px(p: (i32, i32)) -> [f32; 2] {
    [p.0 as f32, p.1 as f32]
}

fn on_screen(view: &View, p: [f32; 2]) -> bool {
    p[0] >= 0.0 && p[1] >= 0.0 && p[0] < view.size.0 as f32 && p[1] < view.size.1 as f32
}

/// Where a line from `o` along `d` meets the plane `axis = value` (`vecLineIntersectWith*`).
fn cut(o: Vec3, d: Vec3, axis: usize, value: f32) -> Option<Vec3> {
    let t = (value - o[axis]) / d[axis];
    let p = o + d * t;
    p.is_finite().then_some(p)
}

/// `piePlaneDiscDraw`: the outline, its ticks and the four spokes with their hatches.
fn disc(canvas: &mut Canvas, view: &View, c: Vec3, r: f32, color: Color) {
    view.circle(canvas, c, r, DISC_SEGMENTS, color);
    let mut a = TICK_STEP;
    while a < std::f32::consts::TAU {
        let (s, co) = a.sin_cos();
        if s.abs() > 1e-5 && co.abs() > 1e-5 {
            let inner = c + Vec3::new(s * r * TICK_INNER, co * r * TICK_INNER, 0.0);
            let outer = c + Vec3::new(s * r, co * r, 0.0);
            view.line(canvas, inner, outer, color, 0xffff);
        }
        a += TICK_STEP;
    }
    for (dx, dy) in SPOKES {
        view.line(canvas, c, c + Vec3::new(dx * r, dy * r, 0.0), color, 0xffff);
        let mut d = HATCH_SPACING;
        while d < r - HATCH_SPACING / 2.0 {
            let a = c + Vec3::new(d * dy + dx * HATCH_HALF, d * dx + dy * HATCH_HALF, 0.0);
            let b = c + Vec3::new(d * dy - dx * HATCH_HALF, d * dx - dy * HATCH_HALF, 0.0);
            view.line(canvas, a, b, color, 0xffff);
            d += HATCH_SPACING;
        }
    }
}

/// `pieMovePointClipToLimits`: keeps the destination inside the universe (less a margin),
/// cut back along the line from `from`, or at `from` when no such point is inside.
pub fn clip_to_limits(universe: Vec3, from: Vec3, to: &mut Vec3) {
    let lim = universe - Vec3::splat(UNIVERSE_MARGIN);
    let inside = |p: Vec3| (0..3).all(|i| p[i] >= -lim[i] && p[i] <= lim[i]);
    if !inside(from) {
        for i in 0..3 {
            to[i] = to[i].min(lim[i]).max(-lim[i]);
        }
    }
    if inside(*to) {
        return;
    }
    let along = |i: usize, p: Vec3| -> (f32, f32) {
        if p[i] < -lim[i] {
            ((-lim[i] - from[i]) / (p[i] - from[i]), -lim[i])
        } else if p[i] > lim[i] {
            ((lim[i] - from[i]) / (p[i] - from[i]), lim[i])
        } else {
            (1.0, p[i])
        }
    };
    let (tx, mut x) = along(0, *to);
    let (ty, mut y) = along(1, *to);
    if tx != 1.0 || ty != 1.0 {
        if ty <= tx {
            x = ty * (to.x - from.x) + from.x;
        } else {
            y = tx * (to.y - from.y) + from.y;
        }
    }
    let z = to.z.min(lim.z).max(-lim.z);
    let p = Vec3::new(x, y, z);
    *to = if inside(p) { p } else { from };
}

/// The readout's text (`pieDistanceReadoutDraw`): metres under 100, hundreds of metres under
/// 10 km, whole kilometres beyond.
pub fn distance_text(d: f32) -> String {
    let m = d.max(0.0) as u32;
    if m < 100 {
        format!("{m} m")
    } else if m < 10_000 {
        format!("{} m", m / 100 * 100)
    } else {
        format!("{} km", m / 1000)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: (u32, u32) = (800, 600);
    const UNIVERSE: Vec3 = Vec3::new(100_000.0, 100_000.0, 50_000.0);

    /// Looking down at the origin from the side and above.
    fn view(eye: Vec3) -> View {
        View {
            view: Mat4::look_at_rh(eye, Vec3::ZERO, Vec3::Z),
            proj: Mat4::perspective_rh(1.0, SIZE.0 as f32 / SIZE.1 as f32, 10.0, 1e6),
            eye,
            size: SIZE,
        }
    }

    fn pointer_at(v: &View, p: Vec3) -> (i32, i32) {
        let q = v.project(p).unwrap();
        (q[0].round() as i32, q[1].round() as i32)
    }

    #[test]
    fn the_plane_point_follows_the_pointer_on_the_plate() {
        let v = view(Vec3::new(-6000.0, -2000.0, 4000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        let target = Vec3::new(1000.0, 1500.0, 0.0);
        let mut ptr = pointer_at(&v, target);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!(plate.plane.distance(target) < 30.0, "{:?}", plate.plane);
        assert_eq!(plate.plane.z, 0.0);
        assert_eq!(plate.destination(), plate.plane);
    }

    #[test]
    fn shift_lifts_the_destination_under_the_pointer_and_keeps_it() {
        let v = view(Vec3::new(-6000.0, -2000.0, 4000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        let foot = Vec3::new(1000.0, 1500.0, 0.0);
        let mut ptr = pointer_at(&v, foot);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        let foot = plate.plane;
        // Shift down: the pointer goes back onto the plane point, nothing moves yet.
        plate.toggle(true);
        assert_eq!(plate.spec, Spec::Height);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!(plate.spec_z.abs() < 20.0);
        // Up the screen by the height of a point 2000 m above the foot, and a little to the
        // side: the pointer is held on the vertical and the height is 2000 m.
        let up = pointer_at(&v, foot + Vec3::new(0.0, 0.0, 2000.0));
        ptr = (up.0 + 25, up.1);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!(
            (ptr.0 - up.0).abs() <= 1,
            "pointer held on the vertical: {ptr:?} {up:?}"
        );
        assert!(
            (plate.spec_z - 2000.0).abs() < 40.0,
            "height {}",
            plate.spec_z
        );
        assert!(plate.height.truncate().distance(foot.truncate()) < 1e-3);
        assert_eq!(plate.destination(), plate.height);
        // Shift up: back to the plate; the pointer returns to the plane point and the height
        // stays while the foot moves.
        plate.toggle(false);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert_eq!(ptr, pointer_at(&v, foot));
        let h = plate.height.z;
        let new_foot = Vec3::new(-500.0, 800.0, 0.0);
        ptr = pointer_at(&v, new_foot);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!(plate.plane.distance(new_foot) < 30.0);
        assert!((plate.height.z - h).abs() < 1e-3, "the height is kept");
        // Shift again: the pointer goes to the raised point, the height is not reset.
        plate.toggle(true);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!((plate.height.z - h).abs() < 40.0);
        let at = pointer_at(&v, plate.height);
        assert!((ptr.0 - at.0).abs() <= 1 && (ptr.1 - at.1).abs() <= 1);
    }

    #[test]
    fn a_height_that_shows_as_the_same_pixel_is_dropped() {
        let v = view(Vec3::new(-6000.0, -2000.0, 4000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        let mut ptr = pointer_at(&v, Vec3::new(500.0, 0.0, 0.0));
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        plate.toggle(true);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        plate.height.z = 0.01;
        plate.spec_z = 0.01;
        plate.toggle(false);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert_eq!(plate.spec_z, 0.0);
        assert_eq!(plate.destination(), plate.plane);
    }

    #[test]
    fn a_turning_camera_pauses_the_plate_and_drift_carries_it() {
        let v = view(Vec3::new(-6000.0, -2000.0, 4000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        let mut ptr = pointer_at(&v, Vec3::new(1000.0, 0.0, 0.0));
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        let before = plate.plane;
        plate.pause(true);
        let drift = Vec3::new(100.0, 50.0, 0.0);
        let mut moved = (10, 10);
        plate.update(drift, 50.0, &v, &mut moved, UNIVERSE, false);
        assert!(plate.plane.distance(before + drift) < 1e-2);
        plate.pause(false);
        assert!(plate.mouse_reset);
        plate.update(drift, 50.0, &v, &mut moved, UNIVERSE, false);
        assert_eq!(moved, pointer_at(&v, before + drift));
    }

    #[test]
    fn the_main_view_limits_the_reach() {
        let v = view(Vec3::new(0.0, -2000.0, 60_000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        plate.plane = Vec3::new(30_000.0, 0.0, 0.0);
        plate.paused = true;
        let mut ptr = (0, 0);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!((plate.plane.length() - MAX_MOVE_HORIZONTAL).abs() < 1.0);
        plate.plane = Vec3::new(30_000.0, 0.0, 0.0);
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, true);
        assert!(
            (plate.plane.x - 30_000.0).abs() < 1.0,
            "no limit in the sensors manager"
        );
        plate.height.z = 40_000.0;
        plate.update(Vec3::ZERO, 50.0, &v, &mut ptr, UNIVERSE, false);
        assert!((plate.height.z - MAX_MOVE_VERTICAL).abs() < 1.0);
    }

    #[test]
    fn destinations_stay_inside_the_universe() {
        let u = Vec3::new(20_000.0, 20_000.0, 10_000.0);
        let mut to = Vec3::new(30_000.0, 0.0, 0.0);
        clip_to_limits(u, Vec3::ZERO, &mut to);
        assert_eq!(to, Vec3::new(12_000.0, 0.0, 0.0));
        let mut to = Vec3::new(24_000.0, 12_000.0, 5000.0);
        clip_to_limits(u, Vec3::ZERO, &mut to);
        assert!((to.x - 12_000.0).abs() < 1e-2 && (to.y - 6000.0).abs() < 1e-2);
        assert_eq!(to.z, 2000.0);
        let mut to = Vec3::new(1000.0, 1000.0, 1000.0);
        clip_to_limits(u, Vec3::ZERO, &mut to);
        assert_eq!(to, Vec3::new(1000.0, 1000.0, 1000.0));
    }

    #[test]
    fn readout_rounds_like_the_original() {
        assert_eq!(distance_text(57.9), "57 m");
        assert_eq!(distance_text(1234.0), "1200 m");
        assert_eq!(distance_text(25_600.0), "25 km");
    }

    #[test]
    fn stipple_pattern_walks() {
        let mut plate = Plate::new(Vec3::ZERO, 1.0);
        assert_eq!(plate.stipple(), 0xff00);
        plate.stipple_counter = 4.0;
        assert_eq!(plate.stipple(), 0x0ff0);
    }

    #[test]
    fn clipping_keeps_the_visible_part() {
        let v = view(Vec3::new(-6000.0, 0.0, 0.0));
        // A vertical through the origin, far longer than the view.
        let (a, b) = v
            .clip(Vec3::new(0.0, 0.0, -1e6), Vec3::new(0.0, 0.0, 1e6))
            .unwrap();
        for p in [a, b] {
            assert!(p[1] >= -0.5 && p[1] <= SIZE.1 as f32 + 0.5);
            assert!((p[0] - SIZE.0 as f32 / 2.0).abs() < 1.0);
        }
        // Behind the camera.
        assert!(v
            .clip(Vec3::new(-7000.0, 0.0, 0.0), Vec3::new(-8000.0, 0.0, 0.0))
            .is_none());
    }

    #[test]
    fn drop_lines_find_the_closest_ship_and_chime_once() {
        let v = view(Vec3::new(-6000.0, -2000.0, 4000.0));
        let mut plate = Plate::new(Vec3::ZERO, 50.0);
        plate.plane = Vec3::new(1000.0, 0.0, 0.0);
        let ships = [
            DropShip {
                id: ShipId(1),
                position: Vec3::new(1050.0, 20.0, 800.0),
                radius: 40.0,
                selected: false,
            },
            DropShip {
                id: ShipId(2),
                position: Vec3::new(-3000.0, 0.0, 0.0),
                radius: 40.0,
                selected: false,
            },
        ];
        let mut canvas = Canvas::new(SIZE.0 as i32, SIZE.1 as i32);
        let colors = Colors::default();
        let first = plate.draw(&mut canvas, &v, &colors, Some(&ships), None, None);
        assert_eq!(first, Some(SOUND_CHIMES));
        assert_eq!(plate.closest, Some(ShipId(1)));
        let again = plate.draw(&mut canvas, &v, &colors, Some(&ships), None, None);
        assert_eq!(again, None);
        // The closest ship's line is drawn in its colour.
        assert!(canvas.pixels.contains(&colors.ship_line_closest));
        plate.plane = Vec3::new(-20_000.0, 9000.0, 0.0);
        plate.draw(&mut canvas, &v, &colors, Some(&ships), None, None);
        assert_eq!(plate.closest, None, "nothing close enough");
    }
}
