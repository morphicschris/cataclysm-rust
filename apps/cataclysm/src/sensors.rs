//! The sensors manager (`Sensors.c` as Cataclysm has it): the map view of the whole level.
//!
//! Opening it pulls the camera back from the main view along its viewing direction to the
//! sensors distance (`smSensorsBegin`, `smZoomUpdate`): the first part of the zoom still shows
//! the main 3D view, the rest the sensors scene, while the backdrop dims and the sensors
//! colours fade in. Closing zooms back the same way, to the ships under the camera or to the
//! ships picked for focus (`smSensorsClose`, `smCullAndFocusSelecting`).
//!
//! The scene (`smSceneRender`) draws the universe's objects straight from the lists, each one
//! tested against the local player's fog of war: the sensor-range discs of the player's ships
//! (nested ones culled), the explored cells' outline, mines, resources, ship points in the
//! owner's colour, full meshes for the large ship types, class outline icons, the selection
//! blinking, the hot-key group numbers and the universe bounds. Selection, orders and the move
//! pie plate are the main view's (the game hands them the sensors camera); panning uses the
//! pan keys or the overlay's arrows with the original's cubic camera tracking.

use std::f32::consts::TAU;

use glam::{Vec2, Vec3};
use hwc_formats::script::ScriptFile;
use hwc_render::effects::PixelTriangle;
use hwc_render::OrbitCamera;
use hwc_sim::fow::{FowGrid, OUTSIDE};
use hwc_sim::harness::group_centre;
use hwc_sim::level::SmTweaks;
use hwc_sim::object::sof;
use hwc_sim::statics::ShipClass;
use hwc_sim::{ShipId, Universe};
use hwc_ui::canvas::{Canvas, Color};
use hwc_ui::options::Options;

use crate::host::player;
use crate::profile;
use crate::world::World;

/// UI sound events of the sensors manager.
pub const SOUND_INTRO: u32 = 0x12d;
pub const SOUND_EXIT: u32 = 0x12e;
pub const SOUND_MANAGER: u32 = 0x101;
pub const SOUND_OF_SPACE: u32 = 0x100;

/// Ticks of the task timer per second (`taskFrequency`).
const TICKS_PER_SECOND: f32 = 240.0;
/// How far from the universe edge panning stops (`pieMovePointClipToLimits`).
const PAN_EDGE_MARGIN: f32 = 8000.0;
/// The backdrop's brightness when fully zoomed out (`smBackgroundDim`).
const BACKGROUND_DIM: f32 = 0.5;
/// The sensors camera's starting distance (`cameraInit(&smCamera, ...)`).
const CAMERA_INIT_DISTANCE: f32 = 90_000.0;
/// Pie-plate destinations farther than this from the main camera cancel the plate on close
/// (`MAX_MOVE_DISTANCE`, squared).
pub const MAX_MOVE_DISTANCE_SQ: f32 = 196_000_000.0;

/// Point colours (`0x888f80..`).
const OWN_COLOR: Color = [0, 255, 0, 255];
const OWN_HARVESTER_COLOR: Color = [0, 255, 0, 255];
const ALLY_COLOR: Color = [0, 192, 255, 255];
const ENEMY_COLOR: Color = [255, 0, 0, 255];
const CRYSTAL_COLOR: Color = [255, 0, 255, 255];
const MINE_COLOR: Color = [255, 112, 32, 255];
const ASTEROID_COLOR: Color = [180, 120, 90, 255];
const DUST_COLOR: Color = [200, 140, 110, 255];
const METEOR_COLOR: Color = [200, 150, 150, 255];
/// Sensor-range disc colours: nearest and farthest (`0x888fb8`, `0x888fbc`).
const RANGE_NEAR: Color = [0, 0, 128, 255];
const RANGE_FAR: Color = [0, 0, 64, 255];
/// How far a range disc's fade reaches beyond the range (`0x888c40`).
const RANGE_FADE: f32 = 1.373_291;
/// Nested range discs are dropped when this close (`0x888fd0`, times the range squared).
const RANGE_NEST: f32 = 0.5;
/// The universe-bounds corner colour (`0x888bc8`).
const BOUNDS_COLOR: Color = [64, 107, 162, 255];
/// The sensors viewport's own colour (`SM_ViewPort` content colour default).
const BACKGROUND_COLOR: Color = [73, 98, 100, 255];
/// Hot-key number colour (`selHotKeyNumberColor`, 0x8889c0).
const HOTKEY_COLOR: Color = hwc_ui::ingame::overlay::HOTKEY_NUMBER_COLOR;
/// Explored-area outline gradients (near, far) for the explored and occupied layers, and how
/// much of a cell an outline corner cuts off (`0x875638..0x875680`).
const EXPLORED_NEAR: Color = [200, 200, 200, 255];
const EXPLORED_FAR: Color = [50, 50, 50, 255];
const OCCUPIED_NEAR: Color = [0, 0, 255, 255];
const OCCUPIED_FAR: Color = [0, 0, 50, 255];
const CORNER_CUT: f32 = 0.2;

/// Ship types drawn as full meshes when their detail level is a mesh (flag 2 of the type
/// table at 0x888ae0).
const MESH_TYPES: &[usize] = &[
    10, 12, 16, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 46, 47, 51, 53, 54, 55,
    56, 57, 62, 66, 76, 80, 96, 98, 100, 101, 104, 109, 110, 111, 115, 116, 117, 118, 119, 120,
    121, 128, 130, 131, 137,
];

/// `sensors.script` tweakables (with the executable's defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tweaks {
    pub zoom_length: f32,
    pub main_view_zoom_length: f32,
    pub focus_radius: f32,
    pub selected_flash_speed: f32,
    pub zoom_min_factor: f32,
    pub zoom_max_factor: f32,
    pub pan_speed_multiplier: f32,
    pub pan_track: f32,
    pub pan_eval_threshold: f32,
    pub cursor_pan_x: i32,
    pub cursor_pan_y: i32,
    pub max_frame_ticks: i32,
    /// `CAM_CUBIC_EVAL_TWEAK` (`cameracommand.script`).
    pub cubic_tweak: f32,
    /// `CAMERA_FOCUSFAR_DISTANCE` (`cameracommand.script`).
    pub focus_far_distance: f32,
}

impl Default for Tweaks {
    fn default() -> Self {
        Tweaks {
            zoom_length: 10.0,
            main_view_zoom_length: 5.0,
            focus_radius: 5000.0,
            selected_flash_speed: 0.8,
            zoom_min_factor: 8.0,
            zoom_max_factor: 4.0,
            pan_speed_multiplier: 0.001,
            pan_track: 0.05,
            pan_eval_threshold: 1.0,
            cursor_pan_x: 300,
            cursor_pan_y: 300,
            max_frame_ticks: 64,
            cubic_tweak: 1.6,
            focus_far_distance: 6000.0,
        }
    }
}

impl Tweaks {
    pub fn from_scripts(sensors: Option<&ScriptFile>, camera: Option<&ScriptFile>) -> Tweaks {
        let mut t = Tweaks::default();
        let f = |s: Option<&ScriptFile>, k: &str, d: f32| {
            s.and_then(|s| s.get(k)).and_then(|e| e.f32()).unwrap_or(d)
        };
        let i = |s: Option<&ScriptFile>, k: &str, d: i32| {
            s.and_then(|s| s.get(k)).and_then(|e| e.i32()).unwrap_or(d)
        };
        t.zoom_length = f(sensors, "smZoomLength", t.zoom_length);
        t.main_view_zoom_length = f(sensors, "smMainViewZoomLength", t.main_view_zoom_length);
        t.focus_radius = f(sensors, "smFocusRadius", t.focus_radius);
        t.selected_flash_speed = f(sensors, "smSelectedFlashSpeed", t.selected_flash_speed);
        t.zoom_min_factor = f(sensors, "smZoomMinFactor", t.zoom_min_factor);
        t.zoom_max_factor = f(sensors, "smZoomMaxFactor", t.zoom_max_factor);
        t.pan_speed_multiplier = f(sensors, "smPanSpeedMultiplier", t.pan_speed_multiplier);
        t.pan_track = f(sensors, "smPanTrack", t.pan_track);
        t.pan_eval_threshold = f(sensors, "smPanEvalThreshold", t.pan_eval_threshold);
        t.cursor_pan_x = i(sensors, "smCursorPanX", t.cursor_pan_x);
        t.cursor_pan_y = i(sensors, "smCursorPanY", t.cursor_pan_y);
        t.max_frame_ticks = i(sensors, "smMaxFrameTicks", t.max_frame_ticks);
        t.cubic_tweak = f(camera, "CAM_CUBIC_EVAL_TWEAK", t.cubic_tweak);
        t.focus_far_distance = f(camera, "CAMERA_FOCUSFAR_DISTANCE", t.focus_far_distance);
        t
    }
}

/// The sensors-view display toggles (`ToggleSV_*` actions, the gameplay options).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toggles {
    pub dust: bool,
    pub resources: bool,
    pub ranges: bool,
    pub background: bool,
    pub areas_visited: bool,
    pub areas_occupied: bool,
    pub icons: bool,
    pub meshes: bool,
    pub pieplate_lines: bool,
    pub meteors: bool,
    pub compass: bool,
    pub objective_pointers: bool,
    pub slipgate_paths: bool,
    pub universe_extents: bool,
}

impl Toggles {
    pub fn from_options(o: &Options) -> Toggles {
        let b = |k: &str| o.get(k) != 0;
        Toggles {
            dust: b("ShowAsteriod0"),
            resources: b("ShowResources"),
            ranges: b("ShowFOWBlobs"),
            background: b("ShowBackground"),
            areas_visited: b("ShowAreaVisited"),
            areas_occupied: b("ShowAreaOccupied"),
            icons: b("ShowIcons"),
            meshes: b("ShowEnemyShips"),
            pieplate_lines: b("ShowFriendlyShips"),
            meteors: b("ShowMeteors"),
            compass: b("ShowCompass"),
            objective_pointers: b("ShowObjectivesPointer"),
            slipgate_paths: b("ShowSlipgatePaths"),
            universe_extents: b("ShowUniverseExtents"),
        }
    }

    /// Flips the toggle of a `ToggleSV_*` action (136..147); false for other actions.
    pub fn toggle(&mut self, action: u8) -> bool {
        let t = match action {
            136 => &mut self.resources,
            137 => &mut self.ranges,
            138 => &mut self.areas_visited,
            139 => &mut self.areas_occupied,
            140 => &mut self.icons,
            141 => &mut self.meshes,
            142 => &mut self.pieplate_lines,
            143 => &mut self.background,
            144 => &mut self.dust,
            145 => &mut self.compass,
            146 => &mut self.objective_pointers,
            147 => &mut self.slipgate_paths,
            _ => return false,
        };
        *t = !*t;
        true
    }
}

/// Class outline icons from `Tactical.script` (`IconClass` / `vertex`), by ship class.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Icons {
    pub by_class: Vec<Vec<[f32; 2]>>,
}

impl Icons {
    pub fn from_script(s: &ScriptFile) -> Icons {
        let mut by_class: Vec<Vec<[f32; 2]>> = vec![Vec::new(); 13];
        let mut current: Option<usize> = None;
        for e in s.entries() {
            match e.key_base().to_ascii_lowercase().as_str() {
                "iconclass" => {
                    let name = e.string().unwrap_or("");
                    current = ShipClass::from_name(name)
                        .or_else(|| {
                            // The script spells it CLASS_Noncombat.
                            name.eq_ignore_ascii_case("CLASS_Noncombat")
                                .then_some(ShipClass::NonCombat)
                        })
                        .map(|c| c.index());
                    if let Some(c) = current {
                        by_class[c].clear();
                    }
                }
                "vertex" => {
                    if let Some(c) = current {
                        let v: Vec<f32> = e
                            .raw
                            .split([',', ' ', '\t'])
                            .filter(|p| !p.is_empty())
                            .filter_map(|p| p.parse().ok())
                            .collect();
                        if v.len() >= 2 {
                            by_class[c].push([v[0], v[1]]);
                        }
                    }
                }
                _ => {}
            }
        }
        Icons { by_class }
    }
}

/// `smShipIconIndexGet`: the outline icon of a ship (by class number), if it has one.
pub fn icon_index(u: &Universe, id: ShipId) -> Option<usize> {
    let s = u.ship(id)?;
    let class = u.statics.ship(s.ship_type).and_then(|st| st.ship_class);
    if class == Some(ShipClass::Mothership) {
        return Some(ShipClass::Mothership.index());
    }
    Some(match s.ship_type.index() {
        0x0c | 0x2f | 0x4c | 0x3e => ShipClass::Carrier.index(),
        0x2e | 0x42 | 0x50 | 0x0a => ShipClass::HeavyCruiser.index(),
        0x62 | 0x64 | 0x65 => ShipClass::NonCombat.index(),
        0x10 | 0x33 => ShipClass::Resource.index(),
        0x6d | 0x19 => ShipClass::Destroyer.index(),
        _ => return None,
    })
}

/// The fog-of-war mode (0x8ae4c8): 0 off, 1 the fog covers the grid only, 2 everything
/// outside the grid is hidden as well. The game ships with 2 and never changes it.
pub const FOG_MODE: i32 = 2;

/// The local player's fog-of-war tests as the sensors manager makes them (`fowPointInGrid`,
/// `fowPointGetViewerCount`, `fowPointIsExplored` in [`FOG_MODE`]).
pub struct Fog<'a> {
    pub grid: Option<&'a FowGrid>,
    /// The fog mode the tests use ([`FOG_MODE`]).
    pub mode: i32,
}

impl<'a> Fog<'a> {
    pub fn of(u: &'a Universe) -> Fog<'a> {
        Fog {
            grid: u.fog.grids.get(player()).and_then(Option::as_ref),
            mode: FOG_MODE,
        }
    }

    /// The point's cell, when it lies in the grid (`fowPointInGrid`: inside the grid's box,
    /// the lower faces included).
    fn cell(&self, p: hwc_math::Vector) -> Option<[i32; 3]> {
        let g = self.grid?;
        let inside = |v: f32, o: f32, n: i32| o <= v && v < n as f32 * g.cell_size + o;
        let in_grid = inside(p.x, g.origin.x, g.dims[0])
            && inside(p.y, g.origin.y, g.dims[1])
            && inside(p.z, g.origin.z, g.dims[2]);
        if !in_grid {
            return None;
        }
        let (c, _) = g.point_to_cell(p, 0.0);
        (g.viewer_count(c) != OUTSIDE).then_some(c)
    }

    /// The fog test of `smShipsDraw` / `smMinesDraw` / `smEnemyShipVisible`: seen now. With
    /// the fog off everything is seen; outside the grid only in mode 1.
    pub fn seen(&self, p: hwc_math::Vector) -> bool {
        if self.mode == 0 {
            return true;
        }
        let Some(g) = self.grid else { return false };
        match self.cell(p) {
            Some(c) => g.viewer_count(c) > 0,
            None => self.mode < 2,
        }
    }

    /// `selFowPointHidden(p, FALSE)`: not seen now by the local player (with the fog on, and
    /// no grid, everything is hidden).
    pub fn point_hidden(&self, p: hwc_math::Vector) -> bool {
        self.mode != 0 && !self.seen(p)
    }

    /// Ever seen (resources, slipgates; `selFowPointHidden(p, TRUE)` negated).
    pub fn explored(&self, p: hwc_math::Vector) -> bool {
        if self.mode == 0 {
            return true;
        }
        let Some(g) = self.grid else { return false };
        match self.cell(p) {
            Some(c) => g.is_explored(c),
            None => self.mode < 2,
        }
    }
}

/// Whether the local player sees a ship in the sensors view (`smShipsDraw`'s test, as
/// `smEnemyShipVisible`): its own and allied ships always; when the local player is dead or
/// has no fog grid, every ship; otherwise ships not cloaked and in a cell its ships see now.
/// Outside the single-player game a Mimic disguised as one of the player's ships, or a ship
/// hidden from the player (`univShipVisibleToPlayer`), is not seen either.
pub fn ship_visible(u: &Universe, fog: &Fog, id: ShipId) -> bool {
    let Some(s) = u.ship(id) else { return false };
    if s.flags & sof::HIDE != 0 {
        return false;
    }
    let dead = u.base.players.get(player()).is_some_and(|p| p.state == 1);
    if allied(u, s.player) || dead || fog.grid.is_none() {
        return true;
    }
    if !u.single_player
        && (hwc_sim::ships::mimic::is_disguised_as_player(u, id, player())
            || !hwc_sim::visibility::visible_to_player(u, id, player()))
    {
        return false;
    }
    !hwc_sim::cloak::is_cloaked(s) && fog.seen(s.posinfo.position)
}

fn allied(u: &Universe, player: usize) -> bool {
    player == crate::host::player() || u.players.get(crate::host::player()).is_some_and(|p| p.is_ally(player))
}

/// Whether another player's ship is coloured as an ally (`smShipsDraw`, `toAllShipsDraw`): an
/// allied player's, or a Mimic disguised as one of the local player's ships
/// (`MimicIsDisguisedAsPlayer`).
pub fn looks_allied(u: &Universe, id: ShipId) -> bool {
    let Some(s) = u.ship(id) else { return false };
    allied(u, s.player) || hwc_sim::ships::mimic::is_disguised_as_player(u, id, player())
}

/// The class a ship's overlay icon shows: a Mimic disguised as one of the local player's
/// ships shows its disguise's (`MimicGetApparentShipClass` in `toAllShipsDraw`).
pub fn apparent_class(u: &Universe, id: ShipId) -> Option<ShipClass> {
    let s = u.ship(id)?;
    let t = if s.player != player() && hwc_sim::ships::mimic::is_disguised_as_player(u, id, player()) {
        hwc_sim::ships::mimic::apparent_ship_type(u, id).unwrap_or(s.ship_type)
    } else {
        s.ship_type
    };
    u.statics.ship(t).and_then(|st| st.ship_class)
}

/// The zoom between the main view and the sensors view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Zoom {
    /// Pulling back into the sensors view (`smZoomingOut`).
    Out,
    /// Closing (`smZoomingIn`).
    In,
    Idle,
}

/// What the frame shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// The main 3D view (closed, or the main-view part of a zoom).
    Main,
    Sensors,
}

/// An open sensors manager.
#[derive(Debug, Clone)]
struct Open {
    /// The main camera as it was (limits and tweaks; its position follows the zoom).
    main: OrbitCamera,
    zoom: Zoom,
    time: f32,
    length: f32,
    main_length: f32,
    look_start: Vec3,
    look_end: Vec3,
    eye_start: Vec3,
    eye_end: Vec3,
    /// Where panning is taking the camera (`smCameraLookatPoint`) and the tracking speed.
    pan_target: Vec3,
    pan_velocity: Vec3,
    /// Ships to focus on once the zoom back reaches the main view.
    focus: Option<Vec<ShipId>>,
    /// The zoom back reached the main view (`smFocusTransition`).
    transition: bool,
    /// Ships the sensors camera keeps following (a user-requested addition; the original's
    /// sensors camera never follows anything), and whether it has caught up with them.
    follow: Option<Vec<ShipId>>,
    locked: bool,
}

/// The sensors manager.
pub struct Sensors {
    pub tweaks: Tweaks,
    pub toggles: Toggles,
    pub icons: Icons,
    /// The sensors camera's distance kept between openings (`smInitialDistance`).
    initial_distance: Option<f32>,
    /// `smInstantTransition`.
    pub instant: bool,
    /// Opening over a main view that follows its focus keeps following it in the sensors
    /// view (a user-requested addition, on by default; `hwcSensorsFollow = 0` leaves the
    /// original's camera that stays where it opened).
    pub follow_focus: bool,
    /// The focus key in the sensors view centres the sensors camera on the selection and
    /// follows it (a user-requested change, on by default; `hwcSensorsFocusStays = 0` gives
    /// back the original's `smCullAndFocusSelecting`, which closes onto them).
    pub focus_stays: bool,
    open: Option<Open>,
}

/// The sensors camera for a level (`smUpdateParameters`).
fn sensors_camera(main: &OrbitCamera, sm: &SmTweaks, t: &Tweaks, distance: f32) -> OrbitCamera {
    let mut c = OrbitCamera::new(main.tweaks, CAMERA_INIT_DISTANCE);
    c.tweaks.clip_near = sm.zoom_min / t.zoom_min_factor;
    c.tweaks.clip_far = sm.zoom_max * t.zoom_max_factor;
    c.closest_zoom = sm.zoom_min;
    c.farthest_zoom = sm.zoom_max;
    c.distance = distance;
    c.mouse_sens = main.mouse_sens;
    c.angle = main.angle;
    c.declination = main.declination;
    c.look_at = main.look_at;
    c
}

/// Places an orbit camera at `eye` looking at `look` (`GetDistanceAngleDeclination`).
pub fn place(c: &mut OrbitCamera, eye: Vec3, look: Vec3) {
    let v = eye - look;
    let d = v.length();
    c.look_at = look;
    c.zoom_target = None;
    if d > 1e-3 {
        c.distance = d;
        let mut a = v.y.atan2(v.x);
        if a < 0.0 {
            a += TAU;
        }
        c.angle = a;
        c.declination = (-v.z / d).clamp(-1.0, 1.0).asin();
    }
}

/// `EvalCubic`: one step of the camera's tracking spline.
fn eval_cubic(y0: &mut f32, m0: &mut f32, y1: f32, t: f32, tweak: f32) {
    let c = *m0;
    let e = y1 - *y0;
    let a = c - e - e;
    let b = e + e - tweak * c;
    *y0 += ((a * t + b) * t + c) * t;
    *m0 = ((3.0 * a * t) + (b + b)) * t + c;
}

/// `colBlend`: `a * f + b * (1 - f)` in 1/256 steps.
pub fn col_blend(a: Color, b: Color, f: f32) -> Color {
    let k = if f.is_nan() {
        0
    } else {
        ((f * 256.0) as i32).clamp(0, 255)
    };
    let mix = |x: u8, y: u8| ((i32::from(y) * (255 - k) + i32::from(x) * k) >> 8) as u8;
    [mix(a[0], b[0]), mix(a[1], b[1]), mix(a[2], b[2]), 255]
}

/// `colMultiplyClamped`.
fn col_scale(c: Color, f: f32) -> Color {
    let m = |x: u8| (f32::from(x) * f).clamp(0.0, 255.0) as u8;
    [m(c[0]), m(c[1]), m(c[2]), 255]
}

impl Sensors {
    pub fn new(tweaks: Tweaks, toggles: Toggles, icons: Icons) -> Sensors {
        Sensors {
            tweaks,
            toggles,
            icons,
            initial_distance: None,
            instant: false,
            follow_focus: true,
            focus_stays: true,
            open: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Zooming in or out: the viewport takes no input (`smViewportProcess`).
    pub fn zooming(&self) -> bool {
        self.open.as_ref().is_some_and(|o| o.zoom != Zoom::Idle)
    }

    /// The zoom fraction: 0 in the main view, 1 in the sensors view.
    pub fn factor(&self) -> f32 {
        match &self.open {
            None => 0.0,
            Some(o) => match o.zoom {
                Zoom::Idle => 1.0,
                _ => (o.time / o.length).clamp(0.0, 1.0),
            },
        }
    }

    /// What this frame draws.
    pub fn phase(&self) -> Phase {
        match &self.open {
            None => Phase::Main,
            Some(o) => match o.zoom {
                Zoom::Idle => Phase::Sensors,
                Zoom::Out => {
                    if o.time < o.main_length {
                        Phase::Main
                    } else {
                        Phase::Sensors
                    }
                }
                Zoom::In => {
                    if o.time >= o.main_length {
                        Phase::Sensors
                    } else {
                        Phase::Main
                    }
                }
            },
        }
    }

    /// The main camera's clip planes, for drawing the main-view part of a zoom.
    pub fn main_camera(&self) -> Option<&OrbitCamera> {
        self.open.as_ref().map(|o| &o.main)
    }

    /// The main camera kept aside while the manager is open, to change.
    pub fn main_camera_mut(&mut self) -> Option<&mut OrbitCamera> {
        self.open.as_mut().map(|o| &mut o.main)
    }

    /// `smSensorsBegin`: the camera starts pulling back from the main view. Returns the sound.
    pub fn begin(&mut self, world: &mut World) -> Option<u32> {
        if self.open.is_some() {
            return None;
        }
        let sm = world.level().sm.clone();
        let main = world.camera;
        let initial = self
            .initial_distance
            .unwrap_or(if sm.initial_distance > 0.0 {
                sm.initial_distance
            } else {
                (sm.zoom_min + sm.zoom_max) * 0.5
            });
        let (length, main_length) = if self.instant {
            (0.1, 0.05)
        } else {
            (self.tweaks.zoom_length, self.tweaks.main_view_zoom_length)
        };
        let look = main.look_at;
        let eye = main.eye();
        let dir = (eye - look).normalize_or(Vec3::X);
        let mut cam = sensors_camera(&main, &sm, &self.tweaks, initial);
        place(&mut cam, eye, look);
        let follow = (self.follow_focus && world.follow_focus && !world.focus.is_empty())
            .then(|| world.focus.clone());
        world.camera = cam;
        world.follow_focus = false;
        self.open = Some(Open {
            main,
            zoom: Zoom::Out,
            time: 0.0,
            length,
            main_length,
            look_start: look,
            look_end: look,
            eye_start: eye,
            eye_end: look + dir * initial,
            pan_target: look,
            pan_velocity: Vec3::ZERO,
            focus: None,
            transition: false,
            locked: follow.is_some(),
            follow,
        });
        world.run_mut().game.sensors_open = true;
        Some(SOUND_INTRO)
    }

    /// `smSensorsClose`: zoom back to the main view. `look` is where the main camera looks
    /// now (its focus). Returns the sound.
    pub fn close(&mut self, world: &World, look: Vec3) -> Option<u32> {
        let o = self.open.as_mut()?;
        if o.zoom != Zoom::Idle {
            return None;
        }
        let main_dist = (o.main.eye() - o.main.look_at).length();
        let cam = &world.camera;
        let (eye_end, look_end) = (cam.eye(), cam.look_at);
        let dir = (look_end - eye_end).normalize_or(Vec3::X);
        o.look_start = look;
        o.eye_start = look - dir * main_dist;
        o.look_end = look_end;
        o.eye_end = eye_end;
        o.zoom = Zoom::In;
        o.time = o.length;
        o.transition = false;
        o.focus = None;
        self.initial_distance = Some(cam.distance);
        Some(SOUND_EXIT)
    }

    /// `smSensorsCloseForGood`: closes at once, whatever it is doing (zooming out or in
    /// included), with the main camera as it was when the manager opened. An NIS starting
    /// does this (`spMainScreen`): the cutscene must not start, or end, with the sensors
    /// camera or a zoom still going.
    pub fn close_for_good(&mut self, world: &mut World) -> bool {
        let Some(o) = self.open.take() else {
            return false;
        };
        world.camera = o.main;
        world.follow_focus = true;
        world.update_focus();
        if let Some(run) = world.mission_run_mut() {
            run.game.sensors_open = false;
            run.game.sensors_kas_flag = 0;
        }
        true
    }

    /// `smCullAndFocusSelecting`: close onto these ships (those within the focus radius of
    /// their mean). Returns the sound, or None when nothing could be focused.
    pub fn close_on(&mut self, world: &World, ships: &[ShipId]) -> Option<u32> {
        if self.zooming() {
            return None;
        }
        let kept = self.cull(world, ships)?;
        let centre = kept.iter().map(|p| p.1).sum::<Vec3>() / kept.len() as f32;
        let far = self.tweaks.focus_far_distance;
        let o = self.open.as_mut()?;
        let cam = &world.camera;
        let (eye_end, look_end) = (cam.eye(), cam.look_at);
        let dir = (look_end - eye_end).normalize_or(Vec3::X);
        o.look_start = centre;
        o.eye_start = centre - dir * far;
        o.look_end = look_end;
        o.eye_end = eye_end;
        o.zoom = Zoom::In;
        o.time = o.length;
        o.transition = false;
        o.focus = Some(kept.into_iter().map(|p| p.0).collect());
        self.initial_distance = Some(cam.distance);
        Some(SOUND_EXIT)
    }

    /// The modern focus in the sensors view: the sensors camera glides onto these ships (those
    /// `smCullAndFocusSelecting` would keep) and follows them, and they become the main
    /// view's focus for when the manager closes. False when nothing could be focused.
    pub fn focus_here(&mut self, world: &mut World, ships: &[ShipId]) -> bool {
        if self.zooming() || self.open.is_none() {
            return false;
        }
        let Some(kept) = self.cull(world, ships) else {
            return false;
        };
        let ids: Vec<ShipId> = kept.into_iter().map(|p| p.0).collect();
        let o = self.open.as_mut().unwrap();
        o.follow = Some(ids.clone());
        o.locked = false;
        world.focus = ids;
        true
    }

    /// `mrWaypointAdd` with `AutoFocusOnWaypoints`: the sensors camera looks at the waypoint
    /// just placed (and stops following ships).
    pub fn look_at(&mut self, world: &mut World, p: Vec3) {
        let Some(o) = self.open.as_mut() else { return };
        o.follow = None;
        world.camera.look_at = p;
    }

    /// The ships within the focus radius of the mean of those that exist, with their
    /// positions (`ccFocusCullRadiusMean`).
    fn cull(&self, world: &World, ships: &[ShipId]) -> Option<Vec<(ShipId, Vec3)>> {
        let u = world.universe();
        let pos = |id: ShipId| u.ship(id).map(|s| v3(s.posinfo.position));
        let pts: Vec<(ShipId, Vec3)> = ships
            .iter()
            .filter_map(|&id| Some((id, pos(id)?)))
            .collect();
        if pts.is_empty() {
            return None;
        }
        let mean = pts.iter().map(|p| p.1).sum::<Vec3>() / pts.len() as f32;
        let r2 = self.tweaks.focus_radius * self.tweaks.focus_radius;
        let kept: Vec<(ShipId, Vec3)> = pts
            .into_iter()
            .filter(|p| (p.1 - mean).length_squared() <= r2)
            .collect();
        (!kept.is_empty()).then_some(kept)
    }

    /// One frame (`smViewportRender`'s update part): the zoom, then panning. `pan` is the
    /// held pan directions (left, right, up, down). Returns the sounds to play.
    pub fn update(
        &mut self,
        world: &mut World,
        dt: f32,
        pan: [bool; 4],
        menu_open: bool,
    ) -> Vec<u32> {
        let mut sounds = Vec::new();
        let Some(o) = self.open.as_mut() else {
            return sounds;
        };
        let ticks = ((dt * TICKS_PER_SECOND).round() as i32).clamp(0, self.tweaks.max_frame_ticks);
        let step = ticks as f32 / TICKS_PER_SECOND;
        // The ships followed: those still alive, and where their centre is.
        let followed = o.follow.as_mut().and_then(|f| {
            let u = world.universe();
            f.retain(|&id| u.ship(id).is_some());
            (!f.is_empty()).then(|| v3(group_centre(u, f)))
        });
        if followed.is_none() {
            o.follow = None;
        }
        match o.zoom {
            Zoom::Out => {
                // Pulling back over a moving focus: both ends of the zoom move with it.
                if let Some(c) = followed {
                    let d = c - o.look_end;
                    o.look_start += d;
                    o.look_end += d;
                    o.eye_start += d;
                    o.eye_end += d;
                }
                o.time += step;
                let f = (o.time / o.length).clamp(0.0, 1.0);
                blend_camera(&mut world.camera, o, f);
                if o.time >= o.length {
                    sounds.push(SOUND_MANAGER);
                    o.zoom = Zoom::Idle;
                    o.pan_target = world.camera.look_at;
                }
                return sounds;
            }
            Zoom::In => {
                o.time -= step;
                let f = (o.time / o.length).clamp(0.0, 1.0);
                blend_camera(&mut world.camera, o, f);
                if o.time < o.main_length && !o.transition {
                    o.transition = true;
                    if let Some(f) = o.focus.take() {
                        world.focus = f;
                        world.follow_focus = true;
                    }
                    sounds.push(SOUND_OF_SPACE);
                }
                if o.time <= 0.0 {
                    let mut main = o.main;
                    place(&mut main, world.camera.eye(), world.camera.look_at);
                    world.camera = main;
                    world.follow_focus = true;
                    world.update_focus();
                    world.run_mut().game.sensors_open = false;
                    world.run_mut().game.sensors_kas_flag = 0;
                    self.open = None;
                }
                return sounds;
            }
            Zoom::Idle => {}
        }
        if menu_open {
            return sounds;
        }
        // Pan keys and arrows: pixels of mouse scrolling this frame.
        let (mut dx, mut dy) = (0, 0);
        let px = |rate: i32| rate * ticks / TICKS_PER_SECOND as i32;
        if pan[0] {
            dx -= px(self.tweaks.cursor_pan_x);
        }
        if pan[1] {
            dx += px(self.tweaks.cursor_pan_x);
        }
        if pan[2] {
            dy -= px(self.tweaks.cursor_pan_y);
        }
        if pan[3] {
            dy += px(self.tweaks.cursor_pan_y);
        }
        let size = world.level().sm.universe_size;
        let cam = &mut world.camera;
        if dx != 0 || dy != 0 {
            let look = cam.look_at - cam.eye();
            let fwd = Vec3::new(look.x, look.y, 0.0).normalize_or(Vec3::X)
                * (self.tweaks.pan_speed_multiplier * cam.distance);
            let right = Vec3::new(fwd.y, -fwd.x, 0.0);
            o.pan_target += -(dy as f32) * fwd + dx as f32 * right;
            o.pan_target = clip_to_universe(o.pan_target, v3(size));
            // Panning away lets go of the ships followed.
            o.follow = None;
        } else if let (Some(c), Some(_)) = (followed, &o.follow) {
            o.pan_target = clip_to_universe(c, v3(size));
            if o.locked {
                // Caught up: stay on them (`ccLockOnTargetNow` for the sensors camera).
                cam.look_at = o.pan_target;
                o.pan_velocity = Vec3::ZERO;
            }
        }
        let d = (cam.look_at - o.pan_target).abs();
        if o.follow.is_some() && !o.locked && d.x + d.y + d.z <= self.tweaks.pan_eval_threshold {
            o.locked = true;
        }
        if d.x + d.y + d.z > self.tweaks.pan_eval_threshold {
            let t = self.tweaks.pan_track;
            let k = self.tweaks.cubic_tweak;
            for _ in 0..ticks {
                eval_cubic(
                    &mut cam.look_at.x,
                    &mut o.pan_velocity.x,
                    o.pan_target.x,
                    t,
                    k,
                );
                eval_cubic(
                    &mut cam.look_at.y,
                    &mut o.pan_velocity.y,
                    o.pan_target.y,
                    t,
                    k,
                );
                eval_cubic(
                    &mut cam.look_at.z,
                    &mut o.pan_velocity.z,
                    o.pan_target.z,
                    t,
                    k,
                );
            }
        }
        sounds
    }
}

/// `smZoomUpdate`'s camera part: look and eye blended between the zoom's ends.
fn blend_camera(c: &mut OrbitCamera, o: &Open, f: f32) {
    let look = o.look_start.lerp(o.look_end, f);
    let eye = o.eye_start.lerp(o.eye_end, f);
    place(c, eye, look);
}

/// `pieMovePointClipToLimits` from the universe centre: pulls a point back inside the
/// universe less the edge margin.
fn clip_to_universe(p: Vec3, size: Vec3) -> Vec3 {
    let lim = (size - Vec3::splat(PAN_EDGE_MARGIN)).max(Vec3::ZERO);
    let mut t: f32 = 1.0;
    for i in 0..3 {
        if p[i].abs() > lim[i] && p[i] != 0.0 {
            t = t.min(lim[i] / p[i].abs());
        }
    }
    p * t
}

fn v3(v: hwc_math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

// -------------------------------------------------------------------------------------------
// Drawing
// -------------------------------------------------------------------------------------------

/// A point on screen with its projected radius (`selCircleCompute`).
#[derive(Debug, Clone, Copy)]
pub struct Circle {
    pub x: f32,
    pub y: f32,
    pub r: f32,
}

/// Projects a sphere: centre and horizontal radius in pixels (None behind the camera).
pub fn project_sphere(world: &World, size: (u32, u32), p: Vec3, radius: f32) -> Option<Circle> {
    let c = world.project(size, p)?;
    let right = world.camera.view().row(0).truncate();
    let q = world.project(size, p + right * radius)?;
    Some(Circle {
        x: c[0],
        y: c[1],
        r: ((q[0] - c[0]).powi(2) + (q[1] - c[1]).powi(2)).sqrt(),
    })
}

/// `primRingFade2`: a filled regular polygon of radius `r0` whose edge fades out to `r1`,
/// with a vertex straight below the centre, alpha blended.
pub fn ring_fade(canvas: &mut Canvas, cx: f32, cy: f32, r0: f32, r1: f32, segments: i32, c: Color) {
    ring_fade_under(canvas, cx, cy, r0, r1, segments, c, None);
}

/// The radius inside which [`ring_fade`] fills every pixel whatever its angle (the inscribed
/// circle of the inner polygon, with a margin so that pixels near the edge take the exact path).
fn ring_inner_radius(r0: f32, segments: i32) -> f32 {
    let half = TAU / segments.max(3) as f32 * 0.5;
    r0 * half.cos() * (1.0 - 1e-4)
}

/// [`ring_fade`], leaving out the pixels a later ring fills anyway: `cover` holds, per pixel,
/// one more than the index of the last ring whose inner circle covers it, and this is ring
/// `index`.
#[allow(clippy::too_many_arguments)]
fn ring_fade_under(
    canvas: &mut Canvas,
    cx: f32,
    cy: f32,
    r0: f32,
    r1: f32,
    segments: i32,
    c: Color,
    cover: Option<(&[u16], u16)>,
) {
    let n = segments.max(3) as f32;
    let step = TAU / n;
    let half = step * 0.5;
    let (w, h) = (canvas.width, canvas.height);
    let x0 = ((cx - r1).floor() as i32).max(0);
    let x1 = ((cx + r1).ceil() as i32).min(w - 1);
    let y0 = ((cy - r1).floor() as i32).max(0);
    let y1 = ((cy + r1).ceil() as i32).min(h - 1);
    // Inside the polygon's inscribed circle every pixel is filled whatever its angle: those
    // skip the angle work (with a margin, so pixels near the edge take the exact path and the
    // picture is the same).
    let inner = ring_inner_radius(r0, segments);
    let inner2 = inner * inner;
    for y in y0..=y1 {
        let fy = y as f32 + 0.5 - cy;
        for x in x0..=x1 {
            if let Some((cover, index)) = cover {
                if cover[(y * w + x) as usize] > index + 1 {
                    continue;
                }
            }
            let fx = x as f32 + 0.5 - cx;
            let d2 = fx * fx + fy * fy;
            if d2 < inner2 {
                canvas.put(x, y, c);
                continue;
            }
            let d = d2.sqrt();
            if d > r1 {
                continue;
            }
            // Angle from straight down, then the polygon's radius in that direction.
            let a = fx.atan2(fy).rem_euclid(TAU);
            let local = a - ((a / step).floor() * step + half);
            let k = half.cos() / local.cos();
            let (e0, e1) = (r0 * k, r1 * k);
            if d <= e0 {
                canvas.put(x, y, c);
            } else if d < e1 {
                let alpha = ((e1 - d) / (e1 - e0).max(1e-3)).clamp(0.0, 1.0);
                canvas.blend(x, y, [c[0], c[1], c[2], (alpha * 255.0) as u8]);
            }
        }
    }
}

/// [`ring_fade`] as triangles for the GPU ([`PixelTriangle`]s, drawn in the background pass
/// right over the backdrop): the inner polygon opaque, and between it and the outer polygon
/// one trapezoid per segment fading from opaque to clear. The CPU's fade is linear along each
/// segment's apothem (`(e1 - d) / (e1 - e0)` with `e = r * cos(half) / cos(local)`), which the
/// trapezoid's interpolated alpha reproduces exactly; with the alpha cut to 8 bits as the
/// canvas does, the picture is the same to within one step of rounding.
pub fn ring_fade_triangles(
    c: &Circle,
    r1: f32,
    segments: i32,
    col: Color,
    out: &mut Vec<PixelTriangle>,
) {
    let n = segments.max(3);
    let step = TAU / n as f32;
    let centre = Vec2::new(c.x, c.y);
    let [r, g, b] = [col[0], col[1], col[2]].map(|v| f32::from(v) / 255.0);
    let solid = [r, g, b, 1.0];
    let clear = [r, g, b, 0.0];
    // Angles from straight down (x right, y down), a vertex at 0.
    let at = |j: i32, radius: f32| {
        let a = j as f32 * step;
        centre + Vec2::new(a.sin(), a.cos()) * radius
    };
    for j in 0..n {
        let (p0, p1) = (at(j, c.r), at(j + 1, c.r));
        let (q0, q1) = (at(j, r1), at(j + 1, r1));
        out.push(PixelTriangle {
            points: [centre, p0, p1],
            colors: [solid; 3],
        });
        out.push(PixelTriangle {
            points: [p0, q0, q1],
            colors: [solid, clear, clear],
        });
        out.push(PixelTriangle {
            points: [p0, q1, p1],
            colors: [solid, clear, solid],
        });
    }
}

/// Two triangles covering a `w` x `h` screen in one colour.
pub fn full_screen_triangles(w: f32, h: f32, color: [f32; 4], out: &mut Vec<PixelTriangle>) {
    let [a, b, c, d] = [
        Vec2::ZERO,
        Vec2::new(w, 0.0),
        Vec2::new(w, h),
        Vec2::new(0.0, h),
    ];
    out.push(PixelTriangle {
        points: [a, b, c],
        colors: [color; 3],
    });
    out.push(PixelTriangle {
        points: [a, c, d],
        colors: [color; 3],
    });
}

/// Rings (`primRingFade2`) drawn in order: centre and inner radius, outer radius, segments,
/// colour. A ring drawn later fills its inside opaquely, so what earlier rings would draw there
/// is never seen and is not drawn (the picture is the same as drawing every ring whole).
/// The disc count and the pixels of their bounding boxes, for the profile.
fn count_rings(rings: &[(Circle, f32, i32, Color)], size: (u32, u32)) {
    if !profile::enabled() {
        return;
    }
    profile::count("sm.discs", rings.len() as u64);
    let (w, h) = (size.0 as f32, size.1 as f32);
    let px: f32 = rings
        .iter()
        .map(|(c, outer, _, _)| {
            let x = ((c.x + outer).min(w) - (c.x - outer).max(0.0)).max(0.0);
            let y = ((c.y + outer).min(h) - (c.y - outer).max(0.0)).max(0.0);
            x * y
        })
        .sum();
    profile::count("sm.disc_box_px", px as u64);
}

fn draw_rings(canvas: &mut Canvas, rings: Vec<(Circle, f32, i32, Color)>) {
    if rings.len() >= usize::from(u16::MAX) {
        for (c, outer, segments, col) in rings {
            ring_fade(canvas, c.x, c.y, c.r, outer, segments, col);
        }
        return;
    }
    let (w, ht) = (canvas.width, canvas.height);
    let mut cover = vec![0u16; (w.max(0) * ht.max(0)) as usize];
    for (i, (c, _, segments, _)) in rings.iter().enumerate() {
        let r = ring_inner_radius(c.r, *segments);
        let r2 = r * r;
        let y0 = ((c.y - r).floor() as i32).max(0);
        let y1 = ((c.y + r).ceil() as i32).min(ht - 1);
        let x0 = ((c.x - r).floor() as i32).max(0);
        let x1 = ((c.x + r).ceil() as i32).min(w - 1);
        for y in y0..=y1 {
            let fy = y as f32 + 0.5 - c.y;
            for x in x0..=x1 {
                let fx = x as f32 + 0.5 - c.x;
                if fx * fx + fy * fy < r2 {
                    cover[(y * w + x) as usize] = i as u16 + 1;
                }
            }
        }
    }
    for (i, (c, outer, segments, col)) in rings.into_iter().enumerate() {
        ring_fade_under(
            canvas,
            c.x,
            c.y,
            c.r,
            outer,
            segments,
            col,
            Some((&cover, i as u16)),
        );
    }
}

/// Everything the sensors scene needs to draw.
pub struct SceneInput<'a> {
    pub world: &'a World,
    pub size: (u32, u32),
    pub selection: &'a [ShipId],
    pub groups: &'a [(u8, Vec<ShipId>)],
    pub time: f32,
    pub font: Option<&'a hwc_formats::font::Font>,
}

impl Sensors {
    /// The colour the sensors viewport clears to (`smIntBackgroundColor`), and the backdrop
    /// brightness (`btgSetColourMultiplier`).
    pub fn background(&self) -> (Color, f32) {
        let f = self.factor();
        (
            col_scale(BACKGROUND_COLOR, f),
            (1.0 - f) * (1.0 - BACKGROUND_DIM) + BACKGROUND_DIM,
        )
    }

    /// Ships drawn as meshes this frame: visible, of a mesh type, not blinking off.
    pub fn draws_mesh(&self, inp: &SceneInput, fog: &Fog, id: ShipId) -> bool {
        if !self.toggles.meshes {
            return false;
        }
        let u = inp.world.universe();
        let Some(s) = u.ship(id) else { return false };
        MESH_TYPES.contains(&s.ship_type.index()) && self.ship_drawn(inp, fog, id)
    }

    /// `smShipMeshDraw` draws each ship's clamped slaves (+0x1bc +0x34) with it: the
    /// Kuun-Lan's modules are components, which `smShipsDraw` itself skips.
    pub fn with_clamped(u: &Universe, hosts: &[ShipId]) -> Vec<ShipId> {
        let mut out = hosts.to_vec();
        for &id in hosts {
            let slaves = u
                .ship(id)
                .and_then(|s| s.base.clamp.as_ref()?.slaves.as_ref());
            for &slave in slaves.into_iter().flatten() {
                if u.ship(slave).is_some() && !out.contains(&slave) {
                    out.push(slave);
                }
            }
        }
        out
    }

    /// `smShipsDraw`'s per-ship visibility (fog, hidden types, selection blink).
    fn ship_drawn(&self, inp: &SceneInput, fog: &Fog, id: ShipId) -> bool {
        let u = inp.world.universe();
        let Some(s) = u.ship(id) else { return false };
        if !ship_visible(u, fog, id) {
            return false;
        }
        let st = u.statics.ship(s.ship_type);
        if st.and_then(|st| st.ship_class) == Some(ShipClass::Component) {
            return false;
        }
        // Swarmers are never drawn.
        if matches!(s.ship_type.index(), 9 | 0x2b) {
            return false;
        }
        if inp.selection.contains(&id) && self.blink_off(inp.time) {
            return false;
        }
        true
    }

    fn blink_off(&self, time: f32) -> bool {
        let f = self.tweaks.selected_flash_speed.max(1e-3);
        time.rem_euclid(f) <= f * 0.5
    }

    /// The layer the GPU draws over the backdrop in the background pass ([`PixelTriangle`]s):
    /// the backdrop's dimming (`btgSetColourMultiplier`) or, with the backdrop off, the clear
    /// colour, then the sensor range discs (`smSensorRangesDraw`, see
    /// [`ring_fade_triangles`]). [`draw_under`](Self::draw_under) with `discs_drawn` follows on
    /// the CPU.
    pub fn background_layer(&self, inp: &SceneInput) -> Vec<PixelTriangle> {
        let _p = profile::span("sm.range_discs");
        let (w, h) = (inp.size.0 as f32, inp.size.1 as f32);
        let mut out = Vec::new();
        let (clear, dim) = self.background();
        if self.toggles.background {
            if dim < 1.0 {
                // Black over the backdrop keeps `k = 1 - alpha` of it, rounded; the CPU kept
                // `floor(c * dim)`. With `k` the largest step of 1/255 below `dim` the two
                // agree for every 8-bit value at the manager's dim of one half.
                let k = ((dim * 255.0).ceil() - 1.0).max(0.0) / 255.0;
                full_screen_triangles(w, h, [0.0, 0.0, 0.0, 1.0 - k], &mut out);
            }
        } else {
            let c = [clear[0], clear[1], clear[2]].map(|v| f32::from(v) / 255.0);
            full_screen_triangles(w, h, [c[0], c[1], c[2], 1.0], &mut out);
        }
        if self.toggles.ranges {
            let rings = self.range_rings(inp);
            count_rings(&rings, inp.size);
            for (c, outer, segments, col) in &rings {
                ring_fade_triangles(c, *outer, *segments, *col, &mut out);
            }
        }
        profile::count("sm.layer_triangles", out.len() as u64);
        out
    }

    /// The 2D layer drawn before the meshes: range discs (unless `discs_drawn`: the GPU drew
    /// them, [`background_layer`](Self::background_layer)), universe bounds, explored cells,
    /// mines and resources.
    pub fn draw_under(&self, canvas: &mut Canvas, inp: &SceneInput, discs_drawn: bool) {
        let u = inp.world.universe();
        let fog = Fog::of(u);
        if self.toggles.ranges && !discs_drawn {
            let _p = profile::span("sm.range_discs");
            self.draw_ranges(canvas, inp);
        }
        if self.toggles.universe_extents {
            let _p = profile::span("sm.bounds");
            self.draw_bounds(canvas, inp);
        }
        if let Some(g) = fog.grid {
            self.draw_fog(canvas, inp, g);
        }
        let _p = profile::span("sm.mines_resources");
        // Mines (`smMinesDraw`): Mission 3 keeps them off the sensors until its script calls
        // `ShowMinesInSensors`; mines bigger than 45 m are drawn two pixels wide.
        let shown = inp
            .world
            .mission_run()
            .is_some_and(|r| r.game.show_mines_in_sensors);
        let hidden = u.single_player && u.base.options.mission == 3 && !shown;
        for &mid in &u.missile_list {
            if hidden {
                break;
            }
            let Some(m) = u.missiles.get(mid) else {
                continue;
            };
            if m.kind != hwc_sim::missile::kind::MINE || !fog.seen(m.posinfo.position) {
                continue;
            }
            let size = hwc_sim::missile::statics_of(u, m).map_or(1, |st| {
                if st.header.coll.coll_sphere_size > 45.0 {
                    2
                } else {
                    1
                }
            });
            if let Some(p) = inp.world.project(inp.size, v3(m.posinfo.position)) {
                point(canvas, p, size, MINE_COLOR);
            }
        }
        // Resources.
        for r in &u.base.resources.list {
            let meteor = r.attributes & 0x200 != 0;
            let color = if meteor {
                if !self.toggles.meteors {
                    continue;
                }
                METEOR_COLOR
            } else {
                if !self.toggles.resources {
                    continue;
                }
                match r.kind {
                    hwc_sim::resource::ResourceKind::Asteroid(_) => ASTEROID_COLOR,
                    hwc_sim::resource::ResourceKind::DustCloud(_)
                    | hwc_sim::resource::ResourceKind::Nebula(_) => DUST_COLOR,
                }
            };
            if !fog.explored(r.position) {
                continue;
            }
            if let Some(p) = inp.world.project(inp.size, v3(r.position)) {
                let big = inp.world.resource_radius(r) > 45.0;
                point(canvas, p, if big { 2 } else { 1 }, color);
            }
        }
    }

    /// `smSensorRangesDraw` on the CPU.
    fn draw_ranges(&self, canvas: &mut Canvas, inp: &SceneInput) {
        let rings = self.range_rings(inp);
        count_rings(&rings, inp.size);
        draw_rings(canvas, rings);
    }

    /// `smSensorRangesDraw`'s discs, farthest first: centre and inner radius, outer radius,
    /// segments, colour.
    fn range_rings(&self, inp: &SceneInput) -> Vec<(Circle, f32, i32, Color)> {
        let u = inp.world.universe();
        let eye = inp.world.camera.eye();
        struct Disc {
            on: bool,
            id: ShipId,
            dist: f32,
            range: f32,
            pos: Vec3,
            blob: Option<hwc_sim::object::BlobId>,
        }
        let mut discs: Vec<Disc> = u
            .ship_list
            .iter()
            .filter_map(|&id| {
                let s = u.ship(id)?;
                (s.player == player() && s.flags & sof::HIDE == 0).then(|| {
                    let pos = v3(s.posinfo.position);
                    Disc {
                        on: true,
                        id,
                        dist: (eye - pos).length(),
                        range: hwc_sim::visibility::vision_range(u, id) as f32,
                        pos,
                        blob: s.coll_my_blob,
                    }
                })
            })
            .collect();
        // Ranges inside a larger one of the same blob go.
        for i in 0..discs.len() {
            for j in 0..discs.len() {
                if !discs[i].on {
                    break;
                }
                if i == j || !discs[j].on || discs[i].blob != discs[j].blob {
                    continue;
                }
                if discs[i].range < discs[j].range {
                    let ri = discs[i].range;
                    if (discs[i].pos - discs[j].pos).length_squared() < RANGE_NEST * ri * ri {
                        discs[i].on = false;
                    }
                }
            }
        }
        discs.retain(|d| d.on);
        let near = discs.iter().map(|d| d.dist).fold(f32::INFINITY, f32::min);
        let far = discs.iter().map(|d| d.dist).fold(0.0, f32::max);
        discs.sort_by(|a, b| b.dist.total_cmp(&a.dist));
        let (bg, _) = self.background();
        let f = self.factor();
        let h = inp.size.1 as f32;
        let mut rings = Vec::new();
        for d in &discs {
            let _ = d.id;
            let Some(c) = project_sphere(inp.world, inp.size, d.pos, d.range) else {
                continue;
            };
            if c.r <= 0.0 {
                continue;
            }
            let outer = RANGE_FADE * c.r;
            let segments = ((outer as i32) * 30 / h.max(1.0) as i32 + 8).min(30);
            let t = ((d.dist - near) / (far - near)).clamp(0.0, 1.0);
            let col = col_blend(col_blend(RANGE_FAR, RANGE_NEAR, t), bg, f);
            rings.push((c, outer, segments, col));
        }
        rings
    }

    /// `smUniverseBoundsDraw`: brackets at the universe box's corners.
    fn draw_bounds(&self, canvas: &mut Canvas, inp: &SceneInput) {
        let f = self.factor();
        if f <= 0.0 {
            return;
        }
        let col = col_scale(BOUNDS_COLOR, f);
        let s = v3(inp.world.level().sm.universe_size);
        let corner = |i: u32| {
            Vec3::new(
                if i & 1 != 0 { s.x } else { -s.x },
                if i & 2 != 0 { s.y } else { -s.y },
                if i & 4 != 0 { s.z } else { -s.z },
            )
        };
        for a in 0..8u32 {
            for bit in [1u32, 2, 4] {
                if a & bit != 0 {
                    continue;
                }
                let (p, q) = (corner(a), corner(a | bit));
                let stub = (q - p) * 0.05;
                gradient_line(canvas, inp, p, p + stub, col);
                gradient_line(canvas, inp, q, q - stub, col);
            }
        }
    }

    /// `fowRender`: the outline of the explored (and, as an option, the occupied) cells.
    fn draw_fog(&self, canvas: &mut Canvas, inp: &SceneInput, g: &FowGrid) {
        let fow = profile::span("sm.fow_pass");
        let mut lines = Vec::new();
        if self.toggles.areas_occupied {
            fog_lines(g, Layer::Occupied, &mut lines);
            if self.toggles.areas_visited {
                fog_lines(g, Layer::ExploredOnly, &mut lines);
            }
        } else if self.toggles.areas_visited {
            fog_lines(g, Layer::Explored, &mut lines);
        }
        drop(fow);
        if lines.is_empty() {
            return;
        }
        let _p = profile::span("sm.explored_outlines");
        profile::count("sm.outline_lines", lines.len() as u64);
        let eye = inp.world.camera.eye();
        let mut dmin = f32::INFINITY;
        let mut dmax = 0.0f32;
        for l in &lines {
            for p in [l.0, l.1] {
                let d = (p - eye).length();
                dmin = dmin.min(d);
                dmax = dmax.max(d);
            }
        }
        for (a, b, layer) in lines {
            let d = ((a + b) * 0.5 - eye).length();
            let i = (((d - dmin) / (dmax - dmin).max(1.0)) * 16.0).clamp(0.0, 15.0) as i32;
            let (near, far) = match layer {
                Layer::Occupied => (OCCUPIED_NEAR, OCCUPIED_FAR),
                _ => (EXPLORED_NEAR, EXPLORED_FAR),
            };
            let col = col_blend(far, near, i as f32 / 16.0);
            if let (Some(p), Some(q)) = (
                inp.world.project(inp.size, a),
                inp.world.project(inp.size, b),
            ) {
                canvas.line(p[0] as i32, p[1] as i32, q[0] as i32, q[1] as i32, col);
            }
        }
    }

    /// The layer drawn after the meshes: ship points and icons (`smShipsDraw`). The pings and
    /// then the hot-key numbers ([`Sensors::draw_hotkey_numbers`]) follow.
    pub fn draw_over(&self, canvas: &mut Canvas, inp: &SceneInput, meshes_drawn: &[ShipId]) {
        let _p = profile::span("sm.ship_points");
        let u = inp.world.universe();
        let fog = Fog::of(u);
        let (w, h) = (inp.size.0 as f32, inp.size.1 as f32);
        for &id in &u.ship_list {
            let Some(s) = u.ship(id) else { continue };
            let crystal = matches!(
                s.ship_type,
                hwc_sim::statics::ShipType::dCrystal0 | hwc_sim::statics::ShipType::dCrystal1
            );
            // Crystals are drawn whatever hides other ships: `smShipsDraw` (0x505620) draws a
            // ship when `CrystalIsCrystal` (0x5aaa70) or its visibility test passes, so the
            // fog of war, cloaking, the hidden flag and the selection blink never hide them.
            if !crystal && !self.ship_drawn(inp, &fog, id) {
                continue;
            }
            // Slipgates (static capability 0x4000) get neither a point nor an icon.
            if u.statics
                .ship(s.ship_type)
                .is_some_and(|st| st.capability_flags & hwc_sim::dock::CAP_SLIPGATE != 0)
            {
                continue;
            }
            let own = s.player == player();
            // Own ships a script keeps out of the player's control are drawn as allies.
            let color = if crate::host::handed_over(s) {
                crate::host::HANDED_OVER_COLOR
            } else if own {
                let harvester = u.statics.ship(s.ship_type).and_then(|st| st.ship_class)
                    == Some(ShipClass::Resource);
                if harvester {
                    OWN_HARVESTER_COLOR
                } else {
                    OWN_COLOR
                }
            } else if looks_allied(u, id) {
                ALLY_COLOR
            } else if crystal {
                CRYSTAL_COLOR
            } else {
                ENEMY_COLOR
            };
            let pos = v3(s.posinfo.position);
            if !meshes_drawn.contains(&id) {
                if let Some(p) = inp.world.project(inp.size, pos) {
                    point(canvas, p, 2, color);
                }
            }
            // The class outline icon: allied ships always, others when the fog grid exists.
            let icon = if allied(u, s.player) || fog.grid.is_some() {
                icon_index(u, id)
            } else {
                None
            };
            let Some(icon) = icon.filter(|_| self.toggles.icons) else {
                continue;
            };
            let Some(verts) = self.icons.by_class.get(icon).filter(|v| !v.is_empty()) else {
                continue;
            };
            let st = u.statics.ship(s.ship_type);
            let radius = st.map_or(50.0, |st| st.header.coll.coll_sphere_size);
            let Some(c) = project_sphere(inp.world, inp.size, v3(s.coll.coll_position), radius)
            else {
                continue;
            };
            let scalar = st.map_or(1.0, |st| st.overlay_selection_scalar);
            let size = (scalar * c.r * 2.0 / w).max(8.0 / w);
            let (sx, sy) = (size * w * 0.5, size * h * 0.5);
            let pts: Vec<(i32, i32)> = verts
                .iter()
                .rev()
                .map(|v| ((c.x + v[0] * sx) as i32, (c.y - v[1] * sy) as i32))
                .collect();
            for k in 0..pts.len() {
                let (a, b) = (pts[k], pts[(k + 1) % pts.len()]);
                canvas.line(a.0, a.1, b.0, b.1, color);
            }
        }
    }

    /// `smShipHotkeyNumbersDraw`: drawn after the pings.
    pub fn draw_hotkey_numbers(&self, canvas: &mut Canvas, inp: &SceneInput) {
        let _p = profile::span("sm.hotkeys");
        self.draw_hotkeys(canvas, inp);
    }

    fn draw_hotkeys(&self, canvas: &mut Canvas, inp: &SceneInput) {
        let Some(font) = inp.font else { return };
        let u = inp.world.universe();
        let cw = hwc_ui::text::width(font, b"0").max(1);
        let fh = hwc_ui::text::height(font);
        for (n, ships) in inp.groups {
            let Some(label) = crate::hotkeys::LABELS.get(usize::from(*n)) else {
                continue;
            };
            let label = label.as_bytes();
            let mut placed: Vec<(i32, i32)> = Vec::new();
            for &id in ships {
                if !inp.selection.contains(&id) {
                    continue;
                }
                let Some(s) = u.ship(id) else { continue };
                let Some(p) = inp.world.project(inp.size, v3(s.posinfo.position)) else {
                    continue;
                };
                let (x, y) = (p[0] as i32, p[1] as i32);
                if placed
                    .iter()
                    .any(|q| (q.0 - x).abs() < cw * 5 && (q.1 - y).abs() < cw * 5)
                {
                    continue;
                }
                placed.push((x, y));
                hwc_ui::text::print(
                    canvas,
                    font,
                    x,
                    y - fh,
                    HOTKEY_COLOR,
                    label,
                    (hwc_ui::text::Shadow::from_bits(0), [0, 0, 0, 255]),
                );
            }
        }
    }
}

/// `primPoint3` with a point size of 1 or 2 pixels.
fn point(canvas: &mut Canvas, p: [f32; 2], size: i32, c: Color) {
    let (x, y) = (p[0].floor() as i32, p[1].floor() as i32);
    if size <= 1 {
        canvas.put(x, y, c);
    } else {
        for dy in -1..1 {
            for dx in -1..1 {
                canvas.put(x.saturating_add(dx), y.saturating_add(dy), c);
            }
        }
    }
}

/// `primLine3Gradient` from `c` at `a` to black at `b` (drawn in eight steps).
fn gradient_line(canvas: &mut Canvas, inp: &SceneInput, a: Vec3, b: Vec3, c: Color) {
    let (Some(p), Some(q)) = (
        inp.world.project(inp.size, a),
        inp.world.project(inp.size, b),
    ) else {
        return;
    };
    const STEPS: i32 = 8;
    for k in 0..STEPS {
        let t0 = k as f32 / STEPS as f32;
        let t1 = (k + 1) as f32 / STEPS as f32;
        let col = col_scale(c, 1.0 - (t0 + t1) * 0.5);
        canvas.line(
            (p[0] + (q[0] - p[0]) * t0) as i32,
            (p[1] + (q[1] - p[1]) * t0) as i32,
            (p[0] + (q[0] - p[0]) * t1) as i32,
            (p[1] + (q[1] - p[1]) * t1) as i32,
            col,
        );
    }
}

/// Which cells an outline surrounds (`fowRenderLayer` modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layer {
    /// Explored but not seen now (mode 0, with the occupied layer).
    ExploredOnly,
    /// Ever explored (mode 1).
    Explored,
    /// Seen now (mode 2).
    Occupied,
}

fn in_layer(g: &FowGrid, c: [i32; 3], layer: Layer) -> bool {
    let n = g.viewer_count(c);
    if n == OUTSIDE {
        return false;
    }
    match layer {
        Layer::ExploredOnly => g.is_explored(c) && n < 1,
        Layer::Explored => g.is_explored(c),
        Layer::Occupied => n >= 1,
    }
}

/// The outline edges of a layer: a cell's edge is drawn unless the cells around it make it
/// part of a flat face or the inside (the decision of `fowRenderLayer`), with the corners
/// where three outline edges meet cut off (`fowRenderCellFaces`).
fn fog_lines(g: &FowGrid, layer: Layer, out: &mut Vec<(Vec3, Vec3, Layer)>) {
    let cs = g.cell_size;
    let o = v3(g.origin);
    // Edge e: axis a, offsets (bu, bv) on the other two axes.
    let edges: Vec<(usize, usize, usize, i32, i32)> = (0..3)
        .flat_map(|a| {
            let (u, v) = ((a + 1) % 3, (a + 2) % 3);
            [(0, 0), (1, 0), (0, 1), (1, 1)]
                .into_iter()
                .map(move |(bu, bv)| (a, u, v, bu, bv))
        })
        .collect();
    let corner_of = |cell: [i32; 3], e: &(usize, usize, usize, i32, i32), end: i32| -> [i32; 3] {
        let mut k = [0; 3];
        k[e.0] = end;
        k[e.1] = e.3;
        k[e.2] = e.4;
        [cell[0] + k[0], cell[1] + k[1], cell[2] + k[2]]
    };
    let world = |c: [i32; 3]| o + Vec3::new(c[0] as f32, c[1] as f32, c[2] as f32) * cs;
    for z in 0..g.dims[2] {
        for y in 0..g.dims[1] {
            for x in 0..g.dims[0] {
                let cell = [x, y, z];
                if !in_layer(g, cell, layer) {
                    continue;
                }
                let drawn: Vec<bool> = edges
                    .iter()
                    .map(|&(_, u, v, bu, bv)| {
                        let off = |du: i32, dv: i32| {
                            let mut c = cell;
                            c[u] += du;
                            c[v] += dv;
                            in_layer(g, c, layer)
                        };
                        let su = if bu == 1 { 1 } else { -1 };
                        let sv = if bv == 1 { 1 } else { -1 };
                        let (b2, b3, b4) = (off(su, 0), off(su, sv), off(0, sv));
                        !matches!(
                            (b2, b3, b4),
                            (true, false, false) | (false, false, true) | (true, true, true)
                        )
                    })
                    .collect();
                // Corners where three drawn edges of this cell meet are cut.
                let cut = |corner: [i32; 3]| {
                    edges
                        .iter()
                        .enumerate()
                        .filter(|(i, e)| {
                            drawn[*i]
                                && (corner_of(cell, e, 0) == corner
                                    || corner_of(cell, e, 1) == corner)
                        })
                        .count()
                        == 3
                };
                for (i, e) in edges.iter().enumerate() {
                    if !drawn[i] {
                        continue;
                    }
                    let (ca, cb) = (corner_of(cell, e, 0), corner_of(cell, e, 1));
                    let (mut a, mut b) = (world(ca), world(cb));
                    let dir = (b - a).normalize_or_zero() * (CORNER_CUT * cs);
                    if cut(ca) {
                        a += dir;
                    }
                    if cut(cb) {
                        b -= dir;
                    }
                    out.push((a, b, layer));
                }
                // The cut corners' triangles.
                for k in 0..8 {
                    let corner = [
                        cell[0] + (k & 1),
                        cell[1] + ((k >> 1) & 1),
                        cell[2] + ((k >> 2) & 1),
                    ];
                    if !cut(corner) {
                        continue;
                    }
                    let p = world(corner);
                    let tips: Vec<Vec3> = (0..3)
                        .map(|axis| {
                            let mut d = Vec3::ZERO;
                            let inward = if corner[axis] > cell[axis] { -1.0 } else { 1.0 };
                            d[axis] = inward * CORNER_CUT * cs;
                            p + d
                        })
                        .collect();
                    for t in 0..3 {
                        out.push((tips[t], tips[(t + 1) % 3], layer));
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colour_blend_matches_col_blend() {
        assert_eq!(
            col_blend([0, 0, 64, 255], [0, 0, 128, 255], 0.0),
            [0, 0, 127, 255]
        );
        assert_eq!(
            col_blend([0, 0, 64, 255], [0, 0, 128, 255], 1.0),
            [0, 0, 63, 255]
        );
        // A NaN fraction (one disc: near == far) gives the second colour.
        assert_eq!(
            col_blend([0, 0, 64, 255], [0, 0, 128, 255], f32::NAN),
            [0, 0, 127, 255]
        );
    }

    #[test]
    fn placing_a_camera_round_trips() {
        let mut c = OrbitCamera::new(hwc_render::CameraTweaks::default(), 100.0);
        let look = Vec3::new(10.0, 20.0, 30.0);
        let eye = look + Vec3::new(-300.0, 400.0, 500.0);
        place(&mut c, eye, look);
        assert!((c.eye() - eye).length() < 0.05, "{:?}", c.eye());
        assert!((c.distance - eye.distance(look)).abs() < 1e-2);
    }

    #[test]
    fn cubic_tracking_converges_on_the_target() {
        let (mut y, mut m) = (0.0f32, 0.0f32);
        for _ in 0..2000 {
            eval_cubic(&mut y, &mut m, 1000.0, 0.05, 3.0);
        }
        assert!((y - 1000.0).abs() < 1.0, "{y}");
    }

    #[test]
    fn panning_stops_short_of_the_universe_edge() {
        let size = Vec3::new(100_000.0, 50_000.0, 20_000.0);
        let p = clip_to_universe(Vec3::new(200_000.0, 0.0, 0.0), size);
        assert!((p.x - 92_000.0).abs() < 1.0);
        let q = Vec3::new(1000.0, 2000.0, 0.0);
        assert_eq!(clip_to_universe(q, size), q);
    }

    #[test]
    fn toggles_flip_by_action() {
        let mut t = Toggles::from_options(&Options::default());
        assert!(t.ranges && t.icons && !t.compass && !t.areas_occupied);
        assert!(t.toggle(137));
        assert!(!t.ranges);
        assert!(!t.toggle(12));
    }

    #[test]
    fn icons_parse_from_the_tactical_script() {
        let s = ScriptFile::parse(
            "IconClass CLASS_Fighter\nnPoints 3\nvertex -0.3, -0.4\nvertex 0.5, 0\nvertex -0.3, 0.4\n\
             IconClass CLASS_Noncombat\nvertex 0, 1.2\nIconClass CLASS_Mine\nvertex 0,0\n",
        );
        let i = Icons::from_script(&s);
        assert_eq!(i.by_class[ShipClass::Fighter.index()].len(), 3);
        assert_eq!(i.by_class[ShipClass::NonCombat.index()], vec![[0.0, 1.2]]);
    }

    #[test]
    fn a_single_cell_outline_has_its_corners_cut() {
        let mut g = FowGrid::new(
            hwc_math::Vector::new(0.0, 0.0, 0.0),
            hwc_math::Vector::new(30.0, 30.0, 30.0),
            10.0,
        );
        let n = g.dims;
        assert_eq!(n, [3, 3, 3]);
        let idx = 13; // cell (1, 1, 1)
        g.cells[idx].explored = true;
        let mut lines = Vec::new();
        fog_lines(&g, Layer::Explored, &mut lines);
        // 12 shortened edges and 8 corner triangles.
        assert_eq!(lines.len(), 12 + 8 * 3);
        for (a, b, _) in &lines[..12] {
            assert!(((*b - *a).length() - 6.0).abs() < 1e-3);
        }
    }

    /// `selFowPointHidden` / `smEnemyShipVisible` in fog mode 2: a point is seen only in a
    /// cell one of the player's ships sees now; outside the grid, or with no grid, it is
    /// hidden.
    #[test]
    fn fog_mode_two_hides_all_but_the_cells_seen_now() {
        let mut g = FowGrid::new(
            hwc_math::Vector::new(0.0, 0.0, 0.0),
            hwc_math::Vector::new(30.0, 30.0, 30.0),
            10.0,
        );
        // A viewer in cell (0, 0, 0) with range 0 sees the cells next to it.
        g.add_viewer_cells([0, 0, 0], 1);
        let fog = Fog {
            grid: Some(&g),
            mode: FOG_MODE,
        };
        let p = |x: f32, y: f32, z: f32| hwc_math::Vector::new(x, y, z);
        assert!(fog.seen(p(5.0, 5.0, 5.0)));
        assert!(!fog.point_hidden(p(15.0, 15.0, 15.0)));
        assert!(fog.point_hidden(p(25.0, 25.0, 25.0)), "a cell no ship sees");
        assert!(fog.explored(p(15.0, 5.0, 5.0)));
        assert!(!fog.explored(p(25.0, 5.0, 5.0)));
        assert!(fog.point_hidden(p(-5.0, 5.0, 5.0)), "outside the grid");
        assert!(!fog.explored(p(-5.0, 5.0, 5.0)));
        let none = Fog {
            grid: None,
            mode: FOG_MODE,
        };
        assert!(none.point_hidden(p(5.0, 5.0, 5.0)));
        // Mode 1 leaves the outside of the grid open; mode 0 is no fog at all.
        let one = Fog {
            grid: Some(&g),
            mode: 1,
        };
        assert!(!one.point_hidden(p(-5.0, 5.0, 5.0)));
        assert!(one.point_hidden(p(25.0, 25.0, 25.0)));
        let off = Fog {
            grid: None,
            mode: 0,
        };
        assert!(!off.point_hidden(p(25.0, 25.0, 25.0)) && off.explored(p(25.0, 25.0, 25.0)));
    }

    #[test]
    fn rings_drawn_under_later_ones_are_the_same_picture() {
        let rings: Vec<(Circle, f32, i32, Color)> = (0..12)
            .map(|i| {
                let f = i as f32;
                (
                    Circle {
                        x: 30.0 + f * 7.3,
                        y: 40.0 + (f * 1.7).sin() * 20.0,
                        r: 10.0 + f * 2.1,
                    },
                    (10.0 + f * 2.1) * RANGE_FADE,
                    8 + i * 2,
                    [(20 * i) as u8, 100, 200 - (10 * i) as u8, 255],
                )
            })
            .collect();
        let mut a = Canvas::new(160, 100);
        a.clear([5, 6, 7, 255]);
        let mut b = a.clone();
        for (c, outer, seg, col) in &rings {
            ring_fade(&mut a, c.x, c.y, c.r, *outer, *seg, *col);
        }
        draw_rings(&mut b, rings);
        assert!(
            a.pixels == b.pixels,
            "the covered rings changed the picture"
        );
    }

    #[test]
    fn ring_fade_fills_the_inside() {
        let mut c = Canvas::new(40, 40);
        c.clear([0, 0, 0, 255]);
        ring_fade(&mut c, 20.0, 20.0, 8.0, 12.0, 16, [0, 0, 128, 255]);
        assert_eq!(c.get(20, 20), [0, 0, 128, 255]);
        assert_eq!(c.get(1, 1), [0, 0, 0, 255]);
        let edge = c.get(20 + 10, 20);
        assert!(edge[2] > 0 && edge[2] < 128, "{edge:?}");
    }
}
