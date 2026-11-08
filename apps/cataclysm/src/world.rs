//! The 3D view of a running mission: the simulation's universe drawn with the game camera
//! through `hwc-render` (adapted from the viewer's mission mode, `apps/hwc-viewer`).
//!
//! Every ship is drawn with the renderer's per-instance state: a level of detail chosen from its
//! screen size, engine trails and capital glows updated at each simulation tick, `.mad`
//! animations started by the simulation's state, the team-colour tint with engine glow and hit
//! flashes, and the hyperspace clip of ships arriving in a mission.
//!
//! The simulation's effects (`hwc_sim::effects`: the ETG effects the original creates for
//! muzzle flashes, bullets and beams, hits, damage smoke, deaths and hulks, hyperspace windows,
//! specials, harvest/salvage/repair beams) are drawn through [`hwc_render::EffectRenderer`],
//! with lens flares hidden behind ships by collision-box ray tests; bullets without an effect
//! are the original's one-pixel lines in the bullet colour. Without effect data (or with
//! `HWC_NO_EFFECTS` set) the older stand-ins are drawn instead: bullets and beams as lines,
//! hits as sparks, deaths as an expanding glow, the hyperspace window as a quad.
//!
//! The simulation runs at the original's 16 Hz through the task scheduler model of
//! `hwc_math::timing` (240 Hz ticks, the universe task every 15, at most 8 per frame);
//! rendering shows the latest update as the original does.

use std::collections::BTreeMap;

use glam::{Mat4, Vec2, Vec3, Vec4};
use hwc_formats::hsf::Hsf;
use hwc_math::timing::{scale_ticks, TaskTicker, TIMER_RESOLUTION};
use hwc_render::assets::{load_mad_clip, mothership_modules, read_script, AssetSource};
use hwc_render::effects::{EffectBatch, Line as FxLine, Sprite, FULL_UV};
use hwc_render::lod::{screen_radius, LodKind, LodSelector, LodTable};
use hwc_render::render_state::{
    infection_progress, normalized_team_color, CollisionBox, ConstructionState, ShipTint,
    TintParams,
};
use hwc_render::trails::{
    ShipTrailDef, ShipTrails, TeamTrailColors, TrailFrame, TrailGate, TrailTick, TrailTweaks,
};
use hwc_render::{
    Background, Blend, CameraTweaks, EffectTexture, EngineEffectAssets, FocusShip, GpuBackground,
    GpuShip, MadClip, OrbitCamera, Renderer, Scene, SceneExtras, SceneLighting, ShipAnimator,
    ShipDef, ShipInstance, ShipModel, ShipRenderState, TeamColors, TeamPaint,
};
use hwc_sim::harness::group_centre;
use hwc_sim::level::LevelState;
use hwc_sim::mission::{hs_effect, MissionRun};
use hwc_sim::nis_level::NisLevelRun;
use hwc_sim::resource::ResourceKind;
use hwc_sim::statics::{ShipClass, ShipType};
use hwc_sim::universe::SimEvent;
use hwc_sim::{ShipId, Universe};

use crate::data::{normalize_asset, Assets};

/// Bullet colours by weapon technology (projectile, energy, ion, plasma, laser, -, -, big gun,
/// beast infection). The original takes them from the gun effects; these are stand-ins until
/// the effect system drives them.
const WEAPON_COLORS: [[f32; 4]; 9] = [
    [1.0, 0.9, 0.4, 1.0],
    [0.4, 1.0, 0.5, 1.0],
    [0.5, 0.95, 1.0, 1.0],
    [1.0, 0.5, 0.2, 1.0],
    [1.0, 0.3, 0.3, 1.0],
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 1.0, 1.0, 1.0],
    [1.0, 0.8, 0.6, 1.0],
    [0.7, 1.0, 0.3, 0.9],
];
const HIT_COLOR: [f32; 4] = [1.0, 0.95, 0.7, 0.9];
const EXPLOSION_COLOR: [f32; 4] = [1.0, 0.6, 0.25, 1.0];
/// How long hit sparks and explosion flashes stay (seconds of game time).
const HIT_TIME: f32 = 0.25;
const EXPLOSION_TIME: f32 = 1.5;
const BEAM_COLOR: [f32; 4] = [0.6, 1.0, 0.9, 0.8];
/// One universe update in seconds.
const TICK: f32 = 1.0 / 16.0;
/// Pieces each recorded trail segment is curved into with smooth trails on.
const SMOOTH_TRAIL_PIECES: usize = 4;
/// The lens-flare set the game loads (`lensflare\\ps-50-300mm-zoom`).
const LENS_FLARE_SET: &str = "ps-50-300mm-zoom";
/// A short-lived effect drawn as sprites.
#[derive(Debug, Clone, Copy)]
struct Effect {
    position: Vec3,
    radius: f32,
    start: f32,
    explosion: bool,
}

/// A ship type ready to draw.
struct ShipArt {
    levels: Vec<ShipModel>,
    table: Option<LodTable>,
    /// Uploaded per colour scheme.
    gpu: BTreeMap<usize, GpuShip>,
    trail: ShipTrailDef,
    clip: Option<MadClip>,
    /// Per level: which `.mad` path drives each skeleton object.
    bindings: Vec<Vec<Option<usize>>>,
}

impl ShipArt {
    fn any_gpu(&self) -> Option<&GpuShip> {
        self.gpu.values().next()
    }
}

/// A race's missile or mine mesh (`RACE_X\Missile.lod` / `Mine.lod`) ready to draw.
struct MissileArt {
    levels: Vec<ShipModel>,
    table: Option<LodTable>,
    /// Uploaded per colour scheme.
    gpu: BTreeMap<usize, GpuShip>,
}

/// Which missile mesh: the race's index and whether it is the mine.
type MissileKey = (usize, bool);

/// The crates of a multiplayer game (`MG_CratesEnabled`) are drawn with the missile art under
/// this key: the derelict `R_Derelict\dCrate`, uncoloured.
const CRATE_KEY: MissileKey = (usize::MAX, false);

/// The type and colour scheme a ship is drawn with: a Mimic past half its morph shows its
/// disguise (`MimicGetApparentShipType`, with the scheme `MimicChooseColourScheme` picked).
fn drawn_look(u: &Universe, s: &hwc_sim::object::Ship) -> (ShipType, usize) {
    hwc_sim::ships::mimic::drawn_look(u, s.id).unwrap_or((s.ship_type, s.color_scheme))
}

/// `rndRenderObject`'s Beast infection state: while the infection timer runs (or is set at
/// all during an NIS) the type the ship becomes is blended in over it, through the infection
/// duration (`beastInfectDuration`; 4 s in an NIS).
fn infection_look(u: &Universe, s: &hwc_sim::object::Ship) -> Option<(ShipType, f32)> {
    let (timer, now) = (s.beast.timer, u.total_time_elapsed);
    if !(now < timer || (timer != 0.0 && u.nis_running)) {
        return None;
    }
    let stat = u.statics.ship(s.ship_type)?;
    let to = hwc_sim::ships::beast::infection_stats(stat)?.2?;
    let duration = if u.nis_running {
        4.0
    } else {
        hwc_sim::ships::beast::infect_duration(u, s.ship_type) as f32
    };
    Some((to, infection_progress(now, timer, duration)))
}

/// `rndRenderObject`'s cloak fade (`meshSetFade(5, ..)`): fading in and out by the cloak's level.
fn cloak_fade(s: &hwc_sim::object::Ship) -> f32 {
    hwc_sim::cloak::fade_level(s).map_or(1.0, |level| 1.0 - level)
}

/// `rndRenderObject` draws nothing of another player's cloaked ship the local player cannot see
/// (`cloakIsCloaked`, not `curPlayerPtr`'s, `!univShipVisibleToPlayer`), and its trails are left
/// out on the same test (HW1's `rndMainViewRenderFunction`).
fn cloak_hidden(u: &Universe, s: &hwc_sim::object::Ship) -> bool {
    let me = crate::host::player();
    hwc_sim::cloak::is_cloaked(s) && s.player != me && !hwc_sim::visibility::visible_to_player(u, s.id, me)
}

/// `rndRenderObject` sets `0x8f18b2` for the player's own fully cloaked ship: the materials drop
/// their textures.
fn cloak_untextured(s: &hwc_sim::object::Ship) -> bool {
    s.player == crate::host::player() && hwc_sim::cloak::fade_level(s).is_none() && hwc_sim::cloak::is_cloaked(s)
}

/// `rndRenderObject`'s lightning state: the ship's flicker is on and within its window (not
/// for a Mimic in disguise).
fn lightning_on(u: &Universe, s: &hwc_sim::object::Ship) -> bool {
    let (f, now) = (&s.flicker, u.total_time_elapsed);
    f.state != 0
        && f.next <= now
        && now < f.end
        && s.flags2 & hwc_sim::object::flags2::DISGUISED == 0
}

fn missile_key(m: &hwc_sim::missile::Missile) -> MissileKey {
    (m.race.index(), m.kind == hwc_sim::missile::kind::MINE)
}

/// An asteroid kind ready to draw (`None` art when its mesh could not be loaded).
struct ResourceArt {
    model: ShipModel,
    gpu: Option<GpuShip>,
}

/// Per-ship presentation state that lives across frames.
struct ShipFx {
    trails: ShipTrails,
    anim: ShipAnimator,
    lod: usize,
    tint: ShipTint,
    last_mad: i32,
    /// The simulation's animation start last seen.
    mad_started: u32,
}

impl Frame {
    /// Whether the ship is drawn at a mesh level of detail this frame.
    pub fn is_mesh(&self, id: ShipId) -> bool {
        self.ids.iter().any(|&(i, mesh)| i == Some(id) && mesh)
    }
}

#[cfg(test)]
impl Frame {
    /// Each drawn ship with the level it is drawn at.
    pub fn ship_levels(&self) -> Vec<(ShipId, usize)> {
        self.items
            .iter()
            .zip(&self.ids)
            .filter_map(|(item, (id, _))| id.map(|id| (id, item.lod)))
            .collect()
    }
}

/// What `Sandbox::frame` prepared for one drawn frame.
pub struct Frame {
    items: Vec<Item>,
    /// The ship each item draws (None for asteroids), and whether its detail level is a mesh.
    ids: Vec<(Option<ShipId>, bool)>,
    poses: Vec<Option<Vec<Option<Mat4>>>>,
    pub effects: EffectBatch,
    /// The simulation's effects to draw this frame.
    pub fx_list: hwc_fx::RenderList,
}

enum ItemKind {
    Ship(ShipType, usize),
    Asteroid(u8),
    /// A missile or mine, in a colour scheme.
    Missile(MissileKey, usize),
}

struct Item {
    kind: ItemKind,
    transform: Mat4,
    age: f32,
    lod: usize,
    clip: Option<Vec4>,
    tint: TintParams,
    time: f32,
    /// Under external construction: the build progress and the mesh-space collision box.
    construction: Option<ConstructionState>,
    /// Beast infection: the type it becomes (drawn over the hull in its colour scheme) and
    /// the progress, 0..1.
    infection: Option<(ShipType, usize, f32)>,
    /// Electric bolts crawl over the hull.
    lightning: bool,
    /// The Naggarok's EMP pulse ([`ShipRenderState::emp_ambient`]).
    emp_ambient: Option<(f32, bool)>,
    /// Opacity (a cloak fading).
    fade: f32,
    /// Drawn without textures.
    untextured: bool,
    /// A depot retrofitting it: the start and end, the type it becomes and that type's colours.
    retrofit: Option<(f32, f32, ShipType, usize)>,
}

/// The mission's universe and everything needed to draw it.
/// What the world's universe belongs to.
// The mission is the common case and is used in place; the NIS level is short-lived.
#[allow(clippy::large_enum_variant)]
pub enum Sim {
    /// A running mission.
    Mission(MissionRun),
    /// The NIS level before a mission (`singlePlayerStartNis` with a level of its own): the
    /// mission is not loaded until its NIS has stopped.
    NisLevel(Box<NisLevelRun>),
}

impl Sim {
    pub fn universe(&self) -> &Universe {
        match self {
            Sim::Mission(r) => &r.universe,
            Sim::NisLevel(n) => &n.universe,
        }
    }

    pub fn universe_mut(&mut self) -> &mut Universe {
        match self {
            Sim::Mission(r) => &mut r.universe,
            Sim::NisLevel(n) => &mut n.universe,
        }
    }

    pub fn level(&self) -> &LevelState {
        match self {
            Sim::Mission(r) => &r.game.level,
            Sim::NisLevel(n) => &n.level,
        }
    }

    /// The mission being played or led into.
    pub fn mission(&self) -> u32 {
        match self {
            Sim::Mission(r) => r.game.config.mission,
            Sim::NisLevel(n) => n.mission,
        }
    }
}

pub struct World {
    src: Assets,
    sim: Sim,
    previous: BTreeMap<ShipId, (Vec3, Mat4)>,
    art: BTreeMap<ShipType, ShipArt>,
    team_colors: TeamColors,
    team_trails: TeamTrailColors,
    trail_tweaks: TrailTweaks,
    engine_fx: Option<EngineEffectAssets>,
    pub lod: LodSelector,
    /// Ships are drawn at their most detailed level whatever their size on screen (a
    /// user-requested change, on by default; `--lod original` or `hwcFullDetailShips = 0`
    /// restore the original's screen-size selection). Only the drawing changes: the level
    /// the original would pick is still worked out and reported by [`ship_lod`](Self::ship_lod)
    /// (the hot-key numbers' font) and [`Frame::is_mesh`] (the sensors manager's meshes).
    pub full_detail: bool,
    fx: BTreeMap<ShipId, ShipFx>,
    pub lighting: SceneLighting,
    background_path: Option<String>,
    /// The level's background theta and phi in degrees.
    background_turn: [f32; 2],
    background: Option<GpuBackground>,
    pub camera: OrbitCamera,
    /// Blend between the last two updates (off: the original shows the latest update).
    pub interpolate: bool,
    /// The random stream of the sentinels' field flicker.
    field_random: u32,
    /// Sentinel fields drawn as energy bubbles (`hwcModernFields`) or as the original's
    /// triangles; the bubbles' textures and ripples.
    pub modern_fields: bool,
    fields_look: Option<crate::sentinel_field::Modern>,
    /// Game paused: no universe updates.
    pub paused: bool,
    ticker: TaskTicker,
    tick_carry: f64,
    /// The time-compression factor `taskExecuteAllPending` scales each frame's ticks by
    /// (`GAME_SPEED_FACTORS`; 1 is normal speed).
    pub speed_factor: f32,
    pub nav_lights: bool,
    /// The camera keeps looking at these ships (the game's camera focus).
    pub focus: Vec<ShipId>,
    effects: Vec<Effect>,
    resource_art: BTreeMap<u8, Option<ResourceArt>>,
    /// Missile and mine meshes (`None` when one could not be loaded).
    missile_art: BTreeMap<MissileKey, Option<MissileArt>>,
    /// The camera has not been put on the mothership yet.
    camera_pending: bool,
    /// The camera follows [`focus`](Self::focus) (off while the sensors manager owns it).
    pub follow_focus: bool,
    /// The simulation creates and runs the original's effects.
    pub effects_on: bool,
    fx_renderer: Option<hwc_render::EffectRenderer>,
    /// The NIS sun lens flare (listed while an NIS plays in this world).
    pub nis_sun: hwc_render::nis_sun::NisSunFlare,
    /// Lens flares are on (the `LensFlare` option).
    pub lens_flares: bool,
    neb_renderer: Option<hwc_render::NebulaRenderer>,
    /// The nebula NIS's fog (`plasmaCloud*`): the level's `Fog` line and the NIS `fog` events.
    pub plasma: hwc_render::PlasmaCloud,
    /// Its renderer, made once the cloud texture is wanted (`None` inside: it did not load).
    plasma_renderer: Option<Option<hwc_render::PlasmaCloudRenderer>>,
    /// The main view is not drawn (the sensors manager is open): the nebulae take no camera
    /// and are not drawn.
    pub sensors_open: bool,
    /// Frames built so far (the draw-check history's numbering).
    frames_drawn: u64,
    /// `Tweak.script`'s render list reach and edge fade.
    pub render_limits: RenderLimits,
    /// Ships, effects and asteroids are drawn by the original's render list: within a radial
    /// distance of the look-at point ([`RenderLimits`]). Off (`hwcRenderList = 0`): everything
    /// within the far clip distance of the eye, also measured radially.
    pub render_list: bool,
    /// `zoomInCloseAsPossible`: the camera is easing in to frame the focused ships, and keeps
    /// framing them until the player zooms.
    pub focus_close: bool,
    /// Focusing again on the ships already in focus zooms in on them (the original's
    /// "autozoom", on by default; `hwcFocusZoom = 0` turns it off).
    pub focus_zoom: bool,
    /// The Alt focus (Alt+click, Alt+band box) recentres on the ships at the current zoom
    /// distance instead of closing in on them (a user-requested change, on by default;
    /// `hwcAltFocusKeepsZoom = 0` gives back the original's `ccFocusClose`).
    pub alt_focus_keeps_zoom: bool,
    /// The closest zoom on focused ships also keeps the near clip plane outside their hulls (a
    /// user-requested addition, on by default; `hwcZoomClear = 0` leaves the original's limit).
    pub zoom_clear: bool,
    /// In a formation attack the wingmen always take the leader's target (a user-requested
    /// change, off by default; `hwcFormationFocusFire`), handed to the universe each update.
    pub formation_focus_leader: bool,
    /// The battle chatter option (`opBattleChatter` / 100, `battleChatterFrequencySet`),
    /// handed to the universe each update with the camera's eye.
    pub chatter_frequency: Option<f32>,
    /// What the speech queue refuses, from the audio and the hot-key groups (`None`: no
    /// audio); the render list is added before each update.
    pub speech_gate: Option<hwc_sim::speech::SpeechGate>,
    /// `hwcBattleReports`: fleet intelligence's battle reports in single-player games too.
    pub battle_reports: bool,
    /// The camera stack's earlier entries (`CameraStackEntry`: the focus and its remembered
    /// distance) that a mission's `TutCameraFocusCancel` (`ccCancelFocus`) goes back to; pushed
    /// by the mission's camera focus calls ([`script_focus`](Self::script_focus)). Not saved.
    pub focus_stack: Vec<(Vec<ShipId>, f32)>,
    /// `CameraSetChaseMode` (camera command +0x8c bit 8): with one ship in focus the camera
    /// rides along from its pilot's seat (`cameraSetEyePositionBasedOnShip`).
    pub chase: bool,
    /// The orbiting camera while [`chase`](Self::chase) has the view (put back at the start of
    /// each frame, so input and the mission see the remembered camera).
    chase_saved: Option<OrbitCamera>,
    /// The optional Homeworld Remastered deaths and engine trails (`hwcRemasteredFx`), when on
    /// and imported.
    hwr: Option<crate::hwr_fx::HwrFx>,
    /// How hulls are drawn: the modern ship look (filtered textures, per-pixel light, bump,
    /// smoothed facets; a user-requested change, on in the game by default, `hwcShipLook = 0`
    /// gives back the original's) or the original's. [`prepare`](Self::prepare) hands it to
    /// the renderer and uploads the ships again when it changes.
    pub ship_look: hwc_render::ShipLook,
}

/// The main camera and what it follows, as saved with a game ([`World::view_state`]).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ViewState {
    pub look_at: [f32; 3],
    pub angle: f32,
    pub declination: f32,
    pub distance: f32,
    pub closest_zoom: f32,
    pub farthest_zoom: f32,
    pub ignore_zoom: bool,
    pub zoom_target: Option<f32>,
    pub focus: Vec<ShipId>,
    pub follow_focus: bool,
    pub focus_close: bool,
    pub camera_pending: bool,
}

/// Engine vector to render vector.
fn v3(v: hwc_math::Vector) -> Vec3 {
    Vec3::new(v.x, v.y, v.z)
}

fn matrix(m: &hwc_math::Matrix, p: Vec3) -> Mat4 {
    let m = &m.m;
    Mat4::from_cols(
        Vec4::new(m[0], m[1], m[2], 0.0),
        Vec4::new(m[3], m[4], m[5], 0.0),
        Vec4::new(m[6], m[7], m[8], 0.0),
        p.extend(1.0),
    )
}
/// The level a ship is drawn at: the most detailed one with full detail on, else the level
/// the original's selection picked.
fn drawn_lod(full_detail: bool, nominal: usize) -> usize {
    if full_detail {
        0
    } else {
        nominal
    }
}

impl World {
    /// The mission's world: level background and lighting, the camera on the start point
    /// until the mothership comes out of hyperspace. `player_colors` replaces colour scheme 0
    /// (the player's) with the colours picked in the front end.
    pub fn new(
        src: Assets,
        sim: Sim,
        player_colors: Option<([u8; 3], [u8; 3])>,
    ) -> Result<Self, String> {
        let (bg, light) = crate::mission::level_presentation(sim.level());
        let background_turn = crate::mission::background_turn(sim.level());
        let start = sim
            .level()
            .point(hwc_kas::kas_hash("StartPointPlayer"))
            .map_or(Vec3::ZERO, |p| Vec3::new(p[0], p[1], p[2]));
        let ctweaks = read_script(&src, "camera.script")
            .map(|s| CameraTweaks::from_script(&s))
            .unwrap_or_default();
        let mut team_colors = read_script(&src, "teamcolors.script")
            .map(|s| TeamColors::from_script(&s))
            .unwrap_or_default();
        if let (Some((base, stripe)), Some(s)) = (player_colors, team_colors.schemes.first_mut()) {
            s.base = base;
            s.stripe = stripe;
        }
        let team_trails = read_script(&src, "teamcolors.script")
            .map(|s| TeamTrailColors::from_script(&s))
            .unwrap_or_default();
        let trail_tweaks = read_script(&src, "trail.script")
            .map(|s| TrailTweaks::from_script(&s))
            .unwrap_or_default();
        let mut camera = OrbitCamera::new(ctweaks, 9000.0);
        camera.angle = (-150.0f32).to_radians();
        camera.declination = (-35.0f32).to_radians();
        camera.rotate_angle(0.0);
        camera.rotate_declination(0.0);
        camera.look_at = start;
        camera.distance = 7000.0;
        let background_path = Some(bg.map_or_else(
            || "btg\\mission01.btg".into(),
            |b| normalize_asset(&b, "btg", "btg"),
        ));
        let lighting = light.map_or_else(
            || "hsf\\default.hsf".into(),
            |l| normalize_asset(&l, "hsf", "hsf"),
        );
        let mut w = Self {
            src,
            sim,
            previous: BTreeMap::new(),
            art: BTreeMap::new(),
            team_colors,
            team_trails,
            trail_tweaks,
            engine_fx: None,
            lod: LodSelector::default(),
            full_detail: true,
            fx: BTreeMap::new(),
            lighting: SceneLighting::default(),
            background_path,
            background_turn,
            background: None,
            camera,
            interpolate: false,
            field_random: 0x2545_f491,
            modern_fields: true,
            fields_look: None,
            paused: false,
            ticker: TaskTicker::new(15),
            tick_carry: 0.0,
            speed_factor: 1.0,
            nav_lights: true,
            focus: Vec::new(),
            effects: Vec::new(),
            resource_art: BTreeMap::new(),
            missile_art: BTreeMap::new(),
            ship_look: hwc_render::ShipLook::original(),
            camera_pending: true,
            follow_focus: true,
            effects_on: false,
            fx_renderer: None,
            nis_sun: hwc_render::nis_sun::NisSunFlare::default(),
            lens_flares: true,
            neb_renderer: None,
            plasma: hwc_render::PlasmaCloud::default(),
            plasma_renderer: None,
            sensors_open: false,
            frames_drawn: 0,
            render_limits: RenderLimits::default(),
            render_list: true,
            focus_close: false,
            focus_zoom: true,
            alt_focus_keeps_zoom: true,
            zoom_clear: true,
            formation_focus_leader: false,
            chatter_frequency: None,
            speech_gate: None,
            battle_reports: false,
            focus_stack: Vec::new(),
            chase: false,
            chase_saved: None,
            hwr: None,
        };
        if let Ok(t) = read_script(&w.src, "tweak.script") {
            w.render_limits = RenderLimits::from_script(&t);
        }
        // `levelPreInit` resets the cloud; the level's `Fog` line sets it.
        if let Some((_, fog)) = w
            .sim
            .level()
            .unmodelled
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Fog"))
        {
            w.plasma.set_from_script(fog);
        }
        w.attach_effects();
        w.set_lighting(&lighting);
        w.load_art()?;
        w.snapshot();
        Ok(w)
    }

    /// Compiles the ETG effects and attaches them to the universe (before its first update).
    fn attach_effects(&mut self) {
        if self.universe().effects.is_some() {
            self.effects_on = true;
            return;
        }
        if std::env::var_os("HWC_NO_EFFECTS").is_some() {
            return;
        }
        let data = hwc_sim::effects::EffectData::load_from(&self.src);
        if data.is_empty() {
            eprintln!("warning: no ETG effects found; drawing stand-in effects");
            return;
        }
        hwc_sim::effects::attach(self.universe_mut(), &data);
        self.effects_on = true;
    }

    /// Turns the Homeworld Remastered deaths and engine trails on or off. On needs the imported
    /// effects (`hwc-import --hwr`); returns whether they are now in use.
    pub fn set_remastered_fx(&mut self, on: bool) -> bool {
        if !on {
            self.hwr = None;
            return false;
        }
        if self.hwr.is_some() {
            return true;
        }
        let Some(dir) = crate::hwr_fx::find_dir(self.src.data().root()) else {
            eprintln!("note: Remastered effects are on but not imported (hwc-import --hwr <Homeworld Remastered folder> <data folder>)");
            return false;
        };
        match crate::hwr_fx::HwrFx::load(&dir) {
            Ok(h) => {
                self.hwr = Some(h);
                true
            }
            Err(e) => {
                eprintln!("warning: Remastered effects in {}: {e}", dir.display());
                false
            }
        }
    }

    /// The Remastered death sounds due since the last call.
    pub fn take_remastered_sounds(&mut self) -> Vec<crate::hwr_fx::SoundCue> {
        self.hwr.as_mut().map_or_else(Vec::new, |h| h.take_sounds())
    }

    /// Engine trails curve through their recorded points (`hwcSmoothTrails`) or keep the
    /// original's straight segments.
    pub fn set_smooth_trails(&mut self, on: bool) {
        self.trail_tweaks.smoothing = if on { SMOOTH_TRAIL_PIECES } else { 1 };
    }

    /// Capital glows keep their size relative to the hull at every zoom (`hwcGlowsInProportion`)
    /// or grow with distance as the original's do.
    pub fn set_glows_in_proportion(&mut self, on: bool) {
        self.trail_tweaks.glows_in_proportion = on;
    }

    /// Whether effect `id`'s output is replaced by the Remastered effects (its sounds too).
    pub fn remastered_hides_effect(&self, id: u32) -> bool {
        let (Some(h), Some(fx)) = (&self.hwr, self.universe().effects.as_ref()) else {
            return false;
        };
        fx.world
            .effects
            .iter()
            .find(|e| e.id == id)
            .is_some_and(|e| h.hides(e))
    }

    /// Team colour schemes (base, stripe); scheme 0 is the local player's.
    pub fn team_schemes(&self) -> Vec<([u8; 3], [u8; 3])> {
        self.team_colors
            .schemes
            .iter()
            .map(|s| (s.base, s.stripe))
            .collect()
    }

    /// The game files the world draws from.
    pub fn assets(&self) -> &Assets {
        &self.src
    }

    pub fn universe(&self) -> &Universe {
        self.sim.universe()
    }

    pub fn universe_mut(&mut self) -> &mut Universe {
        self.sim.universe_mut()
    }

    /// The level the universe was loaded from.
    pub fn level(&self) -> &LevelState {
        self.sim.level()
    }

    /// The mission being played, or the one the NIS level leads into.
    pub fn mission(&self) -> u32 {
        self.sim.mission()
    }

    /// The difficulty the mission is played at.
    pub fn skill(&self) -> i32 {
        match &self.sim {
            Sim::Mission(r) => r.game.config.skill,
            Sim::NisLevel(n) => n.universe.sp_difficulty as i32,
        }
    }

    /// The running mission (`None` while its NIS level plays).
    pub fn mission_run(&self) -> Option<&MissionRun> {
        match &self.sim {
            Sim::Mission(r) => Some(r),
            Sim::NisLevel(_) => None,
        }
    }

    /// The running mission, mutably (`None` while its NIS level plays).
    pub fn mission_run_mut(&mut self) -> Option<&mut MissionRun> {
        match &mut self.sim {
            Sim::Mission(r) => Some(r),
            Sim::NisLevel(_) => None,
        }
    }

    /// The running mission. Only for the mission's own paths: panics while an NIS level
    /// plays (callers that can run then use [`World::mission_run`]).
    pub fn run(&self) -> &MissionRun {
        self.mission_run()
            .expect("the mission is not loaded while its NIS level plays")
    }

    /// [`World::run`], mutably.
    pub fn run_mut(&mut self) -> &mut MissionRun {
        self.mission_run_mut()
            .expect("the mission is not loaded while its NIS level plays")
    }

    /// The NIS level playing in this world, if it is one.
    pub fn nis_level(&self) -> Option<&NisLevelRun> {
        match &self.sim {
            Sim::NisLevel(n) => Some(n),
            Sim::Mission(_) => None,
        }
    }

    /// The simulation, giving up the world (the NIS level's universe goes on into
    /// [`NisLevelRun::finish`]).
    pub fn into_sim(self) -> Sim {
        self.sim
    }

    fn set_lighting(&mut self, path: &str) {
        match AssetSource::read(&self.src, path).and_then(|d| Hsf::parse(&d).ok()) {
            Some(h) => self.lighting = SceneLighting::from_hsf(&h),
            None => eprintln!("warning: {path}: no lighting"),
        }
    }

    /// Crates drawn in the last frame (the crate art is loaded and uploaded, and crates lie in
    /// view).
    #[cfg(test)]
    pub fn drawn_crates(&self) -> usize {
        let loaded = self.missile_art.get(&CRATE_KEY).and_then(Option::as_ref).is_some_and(|a| a.gpu.contains_key(&0));
        if loaded {
            self.universe().mp.crates.list.len()
        } else {
            0
        }
    }

    fn load_art(&mut self) -> Result<(), String> {
        self.load_missile_art();
        let u = self.universe();
        let types: Vec<ShipType> = u
            .ships
            .iter()
            .flat_map(|s| {
                let infected = infection_look(u, s).map(|(t, _)| t);
                let retrofit = hwc_sim::salvage::retrofit_look(u, s.id).map(|r| r.2);
                [Some(s.ship_type), Some(drawn_look(u, s).0), infected, retrofit]
            })
            .flatten()
            .collect();
        if types.iter().all(|t| self.art.contains_key(t)) && self.resource_kinds_loaded() {
            return Ok(());
        }
        let all = ShipDef::list_all(&self.src);
        for t in types {
            if self.art.contains_key(&t) {
                continue;
            }
            let race = t.race().name().to_ascii_lowercase();
            let name = t.name().to_ascii_lowercase();
            let def = all
                .iter()
                .find(|d| d.race == race && d.name == name)
                .cloned()
                .ok_or_else(|| format!("no model for {race}\\{name}"))?;
            let (mut levels, table) =
                ShipModel::load_lods(&self.src, &def).map_err(|e| e.to_string())?;
            // Mothership modules are ships of their own in the simulation (clamped to the hull);
            // only other ships with module meshes get them added.
            if !matches!(t, ShipType::sMothership | ShipType::bMothership) {
                for m in mothership_modules(&def, &all) {
                    for (l, level) in levels.iter_mut().enumerate() {
                        let r = if l == 0 {
                            level.add_module(&self.src, m)
                        } else {
                            level.add_module_level(&self.src, m, l)
                        };
                        if let Err(e) = r {
                            eprintln!("warning: module {} level {l}: {e}", m.name);
                        }
                    }
                }
            }
            let mut trail = ShipTrailDef::load(&self.src, &def);
            if let Some(r) = &mut trail.ramp {
                r.recolorize_all(&self.team_trails);
            }
            let clip = load_mad_clip(&self.src, &def);
            self.art.insert(
                t,
                ShipArt {
                    levels,
                    table,
                    gpu: BTreeMap::new(),
                    trail,
                    clip,
                    bindings: Vec::new(),
                },
            );
        }
        let kinds: Vec<u8> = self
            .universe()
            .base
            .resources
            .list
            .iter()
            .filter_map(|r| match r.kind {
                ResourceKind::Asteroid(k) => Some(k),
                ResourceKind::DustCloud(_) | ResourceKind::Nebula(_) => None,
            })
            .collect();
        for k in kinds {
            if self.resource_art.contains_key(&k) {
                continue;
            }
            let name = format!("asteroid{k}");
            let art = ShipDef::load(&self.src, "resources\\asteroids", &name)
                .and_then(|d| ShipModel::load(&self.src, &d))
                .map_err(|e| eprintln!("warning: {name}: {e}"))
                .ok()
                .map(|model| ResourceArt { model, gpu: None });
            self.resource_art.insert(k, art);
        }
        Ok(())
    }

    /// Loads the meshes of missile and mine kinds that appeared since the last update.
    fn load_missile_art(&mut self) {
        let u = self.sim.universe();
        let mut keys: Vec<(MissileKey, hwc_sim::statics::ShipRace)> = u
            .missile_list
            .iter()
            .filter_map(|&mid| u.missiles.get(mid))
            .map(|m| (missile_key(m), m.race))
            .filter(|(k, _)| !self.missile_art.contains_key(k))
            .collect();
        keys.sort_by_key(|(k, _)| *k);
        keys.dedup_by_key(|(k, _)| *k);
        for ((race_index, mine), race) in keys {
            let dir = race.name().to_ascii_lowercase();
            let name = if mine { "mine" } else { "missile" };
            let art = ShipDef::load(&self.src, &dir, name)
                .and_then(|d| ShipModel::load_lods(&self.src, &d))
                .map_err(|e| eprintln!("warning: {dir}\\{name}: {e}"))
                .ok()
                .map(|(levels, table)| MissileArt {
                    levels,
                    table,
                    gpu: BTreeMap::new(),
                });
            self.missile_art.insert((race_index, mine), art);
        }
        // The crate (a derelict of type `Crate` in the original).
        let u = self.sim.universe();
        if !u.mp.crates.list.is_empty() && !self.missile_art.contains_key(&CRATE_KEY) {
            let art = ShipDef::load(&self.src, "race_derelict", "dcrate")
                .and_then(|d| ShipModel::load_lods(&self.src, &d))
                .map_err(|e| eprintln!("warning: race_derelict\\dcrate: {e}"))
                .ok()
                .map(|(levels, table)| MissileArt { levels, table, gpu: BTreeMap::new() });
            self.missile_art.insert(CRATE_KEY, art);
        }
    }

    /// Loads the art of ships and asteroids that appeared since the last update.
    pub fn load_new_art(&mut self) {
        if let Err(e) = self.load_art() {
            eprintln!("cataclysm: {e}");
        }
    }

    fn resource_kinds_loaded(&self) -> bool {
        self.universe()
            .base
            .resources
            .list
            .iter()
            .all(|r| match r.kind {
                ResourceKind::Asteroid(k) => self.resource_art.contains_key(&k),
                ResourceKind::DustCloud(_) | ResourceKind::Nebula(_) => true,
            })
    }

    /// Uploads models, engine effects and the background (call before drawing).
    pub fn prepare(&mut self, renderer: &mut Renderer) {
        if renderer.ship_look() != self.ship_look {
            renderer.set_ship_look(self.ship_look);
            for art in self.art.values_mut() {
                art.gpu.clear();
            }
            for art in self.missile_art.values_mut().flatten() {
                art.gpu.clear();
            }
            for art in self.resource_art.values_mut().flatten() {
                art.gpu = None;
            }
        }
        if self.engine_fx.is_none() {
            self.engine_fx = Some(renderer.load_engine_effects(&self.src));
        }
        if self.fields_look.is_none() {
            self.fields_look = Some(crate::sentinel_field::Modern::new(renderer));
        }
        if let Some(h) = &mut self.hwr {
            h.prepare(renderer);
        }
        let needed: Vec<(ShipType, usize)> = self
            .universe()
            .ships
            .iter()
            .flat_map(|s| {
                let u = self.universe();
                let infected = infection_look(u, s).map(|(t, _)| (t, s.color_scheme));
                let retrofit = hwc_sim::salvage::retrofit_look(u, s.id).map(|r| (r.2, r.3));
                [Some(drawn_look(u, s)), infected, retrofit]
            })
            .flatten()
            .collect();
        for (t, scheme) in needed {
            let Some(art) = self.art.get_mut(&t) else {
                continue;
            };
            if art.gpu.contains_key(&scheme) {
                continue;
            }
            let paint = match self
                .team_colors
                .schemes
                .get(scheme)
                .or(self.team_colors.schemes.first())
            {
                Some(s) => TeamPaint::Scheme(&self.team_colors, *s),
                None => TeamPaint::None,
            };
            let gpu = renderer.upload_ship_lods(&art.levels, art.table.clone(), paint);
            if art.bindings.is_empty() {
                art.bindings = (0..gpu.level_count())
                    .map(|l| {
                        art.clip
                            .as_ref()
                            .map_or_else(Vec::new, |c| c.bind(gpu.skeleton(l)))
                    })
                    .collect();
            }
            art.gpu.insert(scheme, gpu);
        }
        let u = self.sim.universe();
        let mut missiles: Vec<(MissileKey, usize)> = u
            .missile_list
            .iter()
            .filter_map(|&mid| u.missiles.get(mid))
            .map(|m| (missile_key(m), usize::from(m.color_scheme)))
            .collect();
        missiles.sort_unstable();
        missiles.dedup();
        for (key, scheme) in missiles {
            let Some(Some(art)) = self.missile_art.get_mut(&key) else {
                continue;
            };
            if art.gpu.contains_key(&scheme) {
                continue;
            }
            let paint = match self
                .team_colors
                .schemes
                .get(scheme)
                .or(self.team_colors.schemes.first())
            {
                Some(s) => TeamPaint::Scheme(&self.team_colors, *s),
                None => TeamPaint::None,
            };
            let gpu = renderer.upload_ship_lods(&art.levels, art.table.clone(), paint);
            art.gpu.insert(scheme, gpu);
        }
        if let Some(Some(art)) = self.missile_art.get_mut(&CRATE_KEY) {
            if !art.gpu.contains_key(&0) {
                let gpu = renderer.upload_ship_lods(&art.levels, art.table.clone(), TeamPaint::None);
                art.gpu.insert(0, gpu);
            }
        }
        for art in self.resource_art.values_mut().flatten() {
            if art.gpu.is_none() {
                art.gpu = Some(renderer.upload_ship(&art.model, TeamPaint::None));
            }
        }
        if self.background.is_none() {
            if let Some(p) = &self.background_path {
                match Background::load_turned(&self.src, p, self.background_turn) {
                    Ok(bg) => self.background = Some(renderer.upload_background(&bg)),
                    Err(e) => {
                        eprintln!("warning: {e}");
                        self.background_path = None;
                    }
                }
            }
        }
    }
    fn transform_of(s: &hwc_sim::Ship) -> (Vec3, Mat4) {
        let p = v3(s.posinfo.position);
        (p, matrix(&s.rotinfo.coordsys, p))
    }

    fn snapshot(&mut self) {
        self.previous = self
            .universe()
            .ships
            .iter()
            .map(|s| (s.id, Self::transform_of(s)))
            .collect();
    }

    /// One universe update (keeps the previous state for interpolation) and everything that
    /// One universe update (keeps the previous state for interpolation) and the presentation
    /// work that follows it at the simulation rate: effects, animations and trails.
    pub fn step(&mut self) {
        self.snapshot();
        self.set_nebula_camera();
        self.sim.universe_mut().formation_focus_leader = self.formation_focus_leader;
        if let Some(fx) = self.sim.universe_mut().effects.as_mut() {
            fx.field_bubble = self.modern_fields;
        }
        self.hand_chatter();
        match &mut self.sim {
            Sim::Mission(r) => r.step(),
            Sim::NisLevel(n) => n.universe.update(),
        }
        self.after_update();
    }

    /// One universe update without the mission (its KAS script and single-player state):
    /// the universe of the opening NIS's level, which runs before the mission exists.
    pub fn step_universe(&mut self) {
        self.snapshot();
        self.set_nebula_camera();
        self.sim.universe_mut().formation_focus_leader = self.formation_focus_leader;
        if let Some(fx) = self.sim.universe_mut().effects.as_mut() {
            fx.field_bubble = self.modern_fields;
        }
        self.hand_chatter();
        self.sim.universe_mut().update();
        self.after_update();
    }

    /// What battle chatter needs from the front end: the chatter option, the main camera's
    /// eye (`mrCamera->eyeposition`), the local player, and the speech queue's refusals with
    /// the local ships in the render list.
    fn hand_chatter(&mut self) {
        let eye = self.camera.eye();
        let gate = self.speech_gate.clone().map(|mut g| {
            let u = self.universe();
            g.in_view = u
                .ship_list
                .iter()
                .filter(|&&id| {
                    u.ship(id).is_some_and(|s| {
                        s.player == crate::host::player() && {
                            let p = s.posinfo.position;
                            self.ship_in_view(s, Vec3::new(p.x, p.y, p.z))
                        }
                    })
                })
                .copied()
                .collect();
            g
        });
        let u = self.sim.universe_mut();
        u.speech_gate = gate;
        let c = &mut u.chatter;
        c.frequency = self.chatter_frequency;
        c.camera_eye = Some(hwc_math::Vector::new(eye.x, eye.y, eye.z));
        c.local_player = crate::host::player();
        c.reports_in_campaign = self.battle_reports;
    }

    /// `mrCamera` for the next universe update: the nebulae's render list and the tendrils they
    /// build depend on the main camera.
    fn set_nebula_camera(&mut self) {
        if !self.sensors_open {
            crate::nebula_view::set_camera(self.sim.universe_mut(), &self.camera);
        }
    }

    /// The presentation work after a universe update.
    fn after_update(&mut self) {
        // Ships built or arriving since the last update need their art.
        if let Err(e) = self.load_art() {
            eprintln!("cataclysm: {e}");
        }
        if let Some(m) = &mut self.fields_look {
            m.note_hits(self.sim.universe());
        }
        let now = self.universe().total_time_elapsed;
        self.effects.retain(|e| {
            now - e.start
                < if e.explosion {
                    EXPLOSION_TIME
                } else {
                    HIT_TIME
                }
        });
        let events = self.universe().events.clone();
        let stand_ins = !self.effects_on;
        for e in &events {
            match e {
                SimEvent::BulletHit { ship, .. } if !stand_ins => {
                    if let Some(f) = self.fx.get_mut(ship) {
                        f.tint.flash(0.1);
                    }
                }
                SimEvent::BulletHit { position, ship, .. } => {
                    self.effects.push(Effect {
                        position: v3(*position),
                        radius: 0.0,
                        start: now,
                        explosion: false,
                    });
                    if let Some(f) = self.fx.get_mut(ship) {
                        f.tint.flash(0.1);
                    }
                }
                SimEvent::ShipDied {
                    position, radius, ..
                } if stand_ins => self.effects.push(Effect {
                    position: v3(*position),
                    radius: *radius,
                    start: now,
                    explosion: true,
                }),
                _ => {}
            }
        }
        self.update_ship_fx();
        if let Some(h) = &mut self.hwr {
            let art = &self.art;
            h.after_update(self.sim.universe(), |t| {
                art.get(&t).and_then(|a| a.any_gpu()).map(|g| g.bounds().1)
            });
        }
        let u = self.sim.universe();
        self.focus.retain(|id| u.ship(*id).is_some());
        self.camera_start();
        self.update_focus();
    }

    /// The camera goes to the player's mothership once it is out of hyperspace.
    fn camera_start(&mut self) {
        if !self.camera_pending || !self.follow_focus {
            return;
        }
        let u = self.sim.universe();
        let ms = u.base.players.first().and_then(|p| p.mothership);
        if let Some(s) = ms.and_then(|id| u.ship(id)) {
            if s.flags & hwc_sim::object::sof::HIDE == 0 {
                let r = u
                    .statics
                    .ship(s.ship_type)
                    .map_or(800.0, |st| st.header.coll.coll_sphere_size);
                self.camera.distance = (r * 5.0).max(3000.0);
                self.camera.zoom_target = None;
                self.focus = vec![s.id];
                self.camera_pending = false;
            }
        }
    }

    /// Cuts the ship's engine trails (`trailZeroLength`, an NIS object that jumped).
    pub fn cut_trails(&mut self, id: ShipId) {
        let Some(t) = self.universe().ship(id).map(|s| s.ship_type) else {
            return;
        };
        if let (Some(art), Some(fx)) = (self.art.get(&t), self.fx.get_mut(&id)) {
            fx.trails = ShipTrails::new(&art.trail, id.0 as u32 ^ 0x9e37_79b9);
        }
    }

    /// `madAnimationStart` by animation name (`meshAnimStart` in an NIS); false when the
    /// ship's `.mad` has no such animation.
    pub fn play_anim(&mut self, id: ShipId, name: &str) -> bool {
        let Some(t) = self.universe().ship(id).map(|s| s.ship_type) else {
            return false;
        };
        let Some(art) = self.art.get(&t) else {
            return false;
        };
        let Some(clip) = art.clip.as_ref() else {
            return false;
        };
        let Some(index) = clip.find(name) else {
            return false;
        };
        if let Some(fx) = self.fx.get_mut(&id) {
            fx.anim.play(clip, index, 0);
            fx.last_mad = index as i32;
        }
        if let Some(s) = self.sim.universe_mut().ships.get_mut(id) {
            s.base.mad_anim = index as i32;
        }
        true
    }

    /// Looks from `eye` at `look_at` with `up` as the view's up (the NIS camera, banked as the
    /// NIS says) and a vertical field of view in degrees.
    pub fn set_view(&mut self, eye: Vec3, look_at: Vec3, up: Vec3, fov: f32) {
        let dist = (eye - look_at).length();
        if !dist.is_finite() || dist <= 1e-3 {
            return;
        }
        self.camera.set_eye_look_up(eye, look_at, Some(up));
        self.camera.tweaks.field_of_view = fov;
    }

    /// Hyperspace window state of every ship arriving or leaving: `(state, clipt)`.
    fn hyperspace(&self) -> BTreeMap<ShipId, (u32, f32)> {
        // The NIS level's fleet waits in hyperspace with no window open.
        let Some(run) = self.mission_run() else {
            return BTreeMap::new();
        };
        run.hyperspace_windows()
            .into_iter()
            .map(|(id, state, clipt)| (id, (state, clipt)))
            .collect()
    }

    /// Animations triggered by the simulation's state, and the trail ring update, once per tick.
    fn update_ship_fx(&mut self) {
        let u = self.sim.universe();
        // Dead ships stay (as hulks) until their explosion deletes them.
        let live: Vec<ShipId> = u.ship_list.iter().chain(&u.hulks).copied().collect();
        self.fx.retain(|id, _| live.contains(id));
        for id in live {
            let Some(s) = u.ship(id) else { continue };
            let Some(art) = self.art.get(&s.ship_type) else {
                continue;
            };
            let fx = self.fx.entry(id).or_insert_with(|| ShipFx {
                trails: ShipTrails::new(&art.trail, id.0 as u32 ^ 0x9e37_79b9),
                anim: ShipAnimator::default(),
                lod: 0,
                tint: ShipTint::default(),
                last_mad: -1,
                mad_started: 0,
            });
            if let Some(clip) = &art.clip {
                // The simulation keeps the clock (`hwc_sim::mad`); missions and scenes without it
                // run their own.
                let sim = s.base.mad.filter(|m| m.started > 0);
                if let Some(m) = sim {
                    if m.started != fx.mad_started {
                        fx.mad_started = m.started;
                        if m.current >= 0 {
                            fx.anim.play(clip, m.current as usize, m.flags);
                        }
                    }
                    if m.current < 0 {
                        fx.anim.stop();
                    } else if fx.anim.current == Some(m.current as usize) {
                        fx.anim.time = m.time;
                        fx.anim.paused = m.paused;
                    }
                } else {
                    fx.anim.update(clip, TICK);
                }
            }
            // A ramming frigate charging or pushing stretches its first trail.
            if let Some(t) = fx.trails.trails.iter_mut().find(|t| t.slot == 0) {
                t.ribbon_scale = hwc_sim::ships::ramming::trail_scale(u, id);
            }
            let (_, transform) = Self::transform_of(s);
            // The nozzles go through the objects of the level drawn (`trailUpdate` reads the
            // ship's current level), posed or at rest.
            let objects = art.any_gpu().map(|gpu| {
                let lod =
                    drawn_lod(self.full_detail, fx.lod).min(gpu.level_count().saturating_sub(1));
                let pose = fx_pose(art, fx, lod.min(art.bindings.len().saturating_sub(1)));
                let sk = gpu.skeleton(lod);
                let mats = match &pose {
                    Some(p) => {
                        sk.resolve(|j| p.get(j).copied().flatten().unwrap_or(sk.objects[j].rest))
                    }
                    None => sk.rest_matrices(),
                };
                (sk, mats)
            });
            fx.trails.update(
                &art.trail,
                &self.trail_tweaks,
                &TrailTick {
                    transform,
                    objects: objects.as_ref().map(|(sk, m)| (*sk, m.as_slice())),
                },
            );
            fx.tint.update(u.total_time_elapsed);
        }
    }

    /// The focused ships as the camera's zoom limits see them: collision centre and radius,
    /// the minimum zoom distance (a disguised Mimic's apparent one), and the reach of the
    /// drawn model about the collision centre.
    pub fn focus_ships(&self) -> Vec<FocusShip> {
        let u = self.universe();
        self.focus
            .iter()
            .filter_map(|&id| {
                let s = u.ship(id)?;
                let st = u.statics.ship(s.ship_type)?;
                let c = &st.header.coll;
                let minimum_zoom = hwc_sim::ships::mimic::apparent_sizes(u, id)
                    .map_or(st.minimum_zoom_distance, |a| a.2);
                let offset = Vec3::new(
                    c.coll_sphere_offset.x,
                    c.coll_sphere_offset.y,
                    c.coll_sphere_offset.z,
                );
                let drawn_radius =
                    self.art
                        .get(&s.ship_type)
                        .and_then(|a| a.any_gpu())
                        .map_or(0.0, |g| {
                            let (centre, r) = g.bounds();
                            (centre - offset).length() + r
                        });
                Some(FocusShip {
                    centre: v3(s.coll.coll_position),
                    radius: c.coll_sphere_size,
                    minimum_zoom,
                    drawn_radius,
                })
            })
            .collect()
    }

    /// The closest the main camera may zoom: `NewSetFocusPoint`'s distance for the focus
    /// (the camera's closest zoom when nothing is in focus), and with
    /// [`zoom_clear`](Self::zoom_clear) far enough that no focused hull is cut by the near
    /// clip plane.
    pub fn focus_min_zoom(&self) -> f32 {
        let ships = self.focus_ships();
        let d = self.camera.focus_distance(&ships);
        if self.zoom_clear {
            d.max(self.camera.clear_distance(&ships))
        } else {
            d
        }
    }

    /// `ccFocus` (`close` false) and `ccFocusClose` (`close` true, the Alt focus) on the main
    /// camera. The same ships again ask for the close-up (`zoomInCloseAsPossible`); new ones
    /// become the focus, with the camera kept at least as far as frames them. With
    /// [`alt_focus_keeps_zoom`](Self::alt_focus_keeps_zoom) the Alt focus only recentres.
    pub fn focus_on(&mut self, ships: Vec<ShipId>, close: bool) {
        if ships.is_empty() {
            return;
        }
        let recentre_only = close && self.alt_focus_keeps_zoom;
        let close = close && !recentre_only;
        let same = !self.focus.is_empty()
            && ships.len() == self.focus.len()
            && ships.iter().all(|id| self.focus.contains(id));
        if same {
            if close || (self.focus_zoom && !recentre_only) {
                self.focus_close = true;
            }
            return;
        }
        self.focus = ships;
        self.focus_close = close;
        self.update_focus();
        let min = self.focus_min_zoom();
        if self.camera.distance < min {
            self.camera.zoom_target = Some(min);
        }
    }

    /// A mission's camera focus (`ccFocusGeneral` from `CameraFocus`, `CameraFocusClose` and
    /// `TutCameraFocus`). The same ships again only ask for the close-up. New ones go on top
    /// of the camera stack with the camera at least as far as frames them; when the look-at
    /// point moves 10000 or more, the camera turns to face it from where the eye was. A `cut`
    /// (`ccFocusScripted`, `ccFocusCloseScripted`: the next `CameraChase` jumps) is there at
    /// once, the close-up at its distance; otherwise the zoom eases as for the player's focus.
    pub fn script_focus(&mut self, ships: Vec<ShipId>, close: bool, cut: bool) {
        // `RENDER_VIEWABLE_DISTANCE_SQR` (0x8a215c) and the stack's size limit (0x747d38).
        const FAR_SQ: f32 = 1.0e8;
        const STACK: usize = 20;
        if ships.is_empty() {
            return;
        }
        if self.focuses_on(&ships) {
            self.focus_close = true;
            if cut {
                self.camera.distance = self.focus_min_zoom();
                self.camera.zoom_target = None;
            }
            return;
        }
        let eye = self.camera.eye();
        let old_look = self.camera.look_at;
        if !self.focus.is_empty() {
            self.focus_stack
                .push((self.focus.clone(), self.camera.distance));
            if self.focus_stack.len() >= STACK {
                self.focus_stack.remove(0);
            }
        }
        self.focus = ships;
        self.follow_focus = true;
        self.focus_close = close;
        self.update_focus();
        let look = self.camera.look_at;
        if (look - old_look).length_squared() >= FAR_SQ {
            let d = eye - look;
            let len = d.length();
            if len > 0.0 {
                let mut angle = d.y.atan2(d.x);
                if angle < 0.0 {
                    angle += std::f32::consts::TAU;
                }
                self.camera.angle = angle;
                self.camera.declination =
                    (d.z / len).clamp(-1.0, 1.0).acos() - std::f32::consts::FRAC_PI_2;
                self.camera.rotate_declination(0.0);
            }
        }
        let min = self.focus_min_zoom();
        let distance = self.camera.distance.max(min);
        if cut {
            self.camera.distance = if close { min } else { distance };
            self.camera.zoom_target = None;
        } else if distance != self.camera.distance {
            self.camera.zoom_target = Some(distance);
        }
    }

    /// The camera already focuses on these ships (`SelectionsAreEquivalent`).
    pub fn focuses_on(&self, ships: &[ShipId]) -> bool {
        !self.focus.is_empty()
            && ships.len() == self.focus.len()
            && ships.iter().all(|id| self.focus.contains(id))
    }

    /// `ccChangeAngleDeclination` + `cameraSetEyePosition` (radians; the angle wrapped, the
    /// declination clamped), at once.
    pub fn script_angles(&mut self, angle: f32, declination: f32) {
        self.camera.angle = angle;
        self.camera.rotate_angle(0.0);
        self.camera.declination = declination;
        self.camera.rotate_declination(0.0);
    }

    /// `ccSetDistance`, or with `scripted` `ccDistanceScripted` (which also lets go of the
    /// close-up zoom), at once.
    pub fn script_distance(&mut self, distance: f32, scripted: bool) {
        self.camera.distance = distance;
        self.camera.zoom_target = None;
        if scripted {
            self.focus_close = false;
        }
    }

    /// `ccCancelFocus`: back to the newest earlier focus with a ship left, at its remembered
    /// distance (the zoom eases there; angle and declination stay), the close-up let go.
    pub fn cancel_focus(&mut self) {
        while let Some((mut ships, distance)) = self.focus_stack.pop() {
            let u = self.universe();
            ships.retain(|id| u.ship(*id).is_some());
            if ships.is_empty() {
                continue;
            }
            self.focus = ships;
            self.follow_focus = true;
            self.focus_close = false;
            self.update_focus();
            self.camera.zoom_target = Some(distance.max(self.focus_min_zoom()));
            return;
        }
    }

    /// The start of a frame: the orbiting camera comes back from the chase view.
    pub fn chase_restore(&mut self) {
        if let Some(c) = self.chase_saved.take() {
            self.camera = c;
        }
    }

    /// The end of a frame: with [`chase`](Self::chase) on and one ship in focus, the view is
    /// from that ship (`CameraChase` -> `cameraSetEyePositionBasedOnShip`: the eye at the
    /// ship's `pilotViewOffset`, looking along its heading, its up the view's up). With more or
    /// fewer ships the chase mode switches itself off.
    pub fn chase_apply(&mut self) {
        if !self.chase || self.sensors_open {
            return;
        }
        let [id] = self.focus[..] else {
            self.chase = false;
            return;
        };
        let u = self.sim.universe();
        let Some(s) = u.ship(id) else {
            return;
        };
        let offset = u
            .statics
            .ship(s.ship_type)
            .map_or(hwc_math::Vector::ZERO, |st| st.pilot_view_offset);
        let m = &s.rotinfo.coordsys;
        let eye = v3(s.posinfo.position) + v3(m.mul_vec(offset));
        let (up, heading) = (v3(m.col(0)), v3(m.col(2)));
        let saved = self.camera;
        self.camera.set_eye_look_up(eye, eye + heading, Some(up));
        self.chase_saved = Some(saved);
    }

    /// The main camera and its focus, for a saved game (the camera is part of the game: the
    /// nebulae's update reads it).
    pub fn view_state(&self) -> ViewState {
        let c = self.chase_saved.as_ref().unwrap_or(&self.camera);
        ViewState {
            look_at: c.look_at.to_array(),
            angle: c.angle,
            declination: c.declination,
            distance: c.distance,
            closest_zoom: c.closest_zoom,
            farthest_zoom: c.farthest_zoom,
            ignore_zoom: c.ignore_zoom,
            zoom_target: c.zoom_target,
            focus: self.focus.clone(),
            follow_focus: self.follow_focus,
            focus_close: self.focus_close,
            camera_pending: self.camera_pending,
        }
    }

    /// Puts back a saved [`view_state`](Self::view_state).
    pub fn restore_view(&mut self, v: &ViewState) {
        let c = &mut self.camera;
        c.look_at = Vec3::from_array(v.look_at);
        c.angle = v.angle;
        c.declination = v.declination;
        c.distance = v.distance;
        c.closest_zoom = v.closest_zoom;
        c.farthest_zoom = v.farthest_zoom;
        c.ignore_zoom = v.ignore_zoom;
        c.zoom_target = v.zoom_target;
        c.up_vector = None;
        self.focus = v.focus.clone();
        self.follow_focus = v.follow_focus;
        self.focus_close = v.focus_close;
        self.camera_pending = v.camera_pending;
        // No blending from where the ships were before the game was loaded.
        self.snapshot();
    }

    /// The player zoomed: the close-up focus lets go (`CAM_USER_ZOOMED`).
    pub fn user_zoomed(&mut self) {
        self.focus_close = false;
    }

    /// One frame of the camera's zoom: the close-up focus keeps the target at the framing
    /// distance (when `main` says the camera is the main view's), and the smooth zoom eases
    /// towards its target within `min_zoom`.
    pub fn ease_camera(&mut self, dt: f32, min_zoom: f32, main: bool) {
        if self.focus.is_empty() {
            self.focus_close = false;
        }
        if self.focus_close && main {
            self.camera.zoom_target = Some(min_zoom);
        }
        self.camera.ease_zoom(dt, min_zoom);
    }

    /// Whether a ship is drawn: by the render list, or, with it off, also beyond it within
    /// the far clip distance of the eye when the local player's sensors see it now (the
    /// sensors manager's fog test). The original only draws its render list, around the
    /// look-at point the player's own ships see, so the far view keeps to what they see.
    fn ship_in_view(&self, s: &hwc_sim::object::Ship, position: Vec3) -> bool {
        // The sensors manager draws its ships its own way, from its own camera.
        if self.sensors_open {
            return true;
        }
        let eye = self.camera.eye();
        let u = self.universe();
        let listed = match u.statics.ship(s.ship_type) {
            Some(st) => {
                let to_look = (position - self.camera.look_at).length_squared();
                let to_eye = (position - eye).length_squared();
                self.render_limits.ship_drawn(
                    st.ship_class,
                    s.ship_type,
                    st.renderlist_fade,
                    st.renderlist_limit_sqr,
                    to_look,
                    to_look.min(to_eye),
                )
            }
            None => true,
        };
        if self.render_list || listed {
            return listed;
        }
        (position - eye).length() < self.camera.tweaks.clip_far
            && crate::sensors::ship_visible(u, &crate::sensors::Fog::of(u), s.id)
    }

    /// Whether an effect, bullet or asteroid at `position` is drawn (see
    /// [`ship_in_view`](Self::ship_in_view)): beyond the render list, the far view draws
    /// asteroids where the player has explored (as the sensors manager draws resources) and
    /// everything else where its ships see now.
    fn object_in_view(&self, position: Vec3, big: bool) -> bool {
        self.object_drawn(position, big, false)
    }

    fn object_drawn(&self, position: Vec3, big: bool, asteroid: bool) -> bool {
        if self.sensors_open {
            return true;
        }
        let listed = self
            .render_limits
            .object_listed((position - self.camera.look_at).length_squared(), big);
        if self.render_list || listed {
            return listed;
        }
        (position - self.camera.eye()).length() < self.camera.tweaks.clip_far
            && far_seen(
                &crate::sensors::Fog::of(self.universe()),
                position,
                asteroid,
            )
    }

    /// Keeps the camera on the focused ships.
    pub fn update_focus(&mut self) {
        if self.follow_focus && !self.focus.is_empty() {
            let c = group_centre(self.universe(), &self.focus);
            self.camera.look_at = v3(c);
        }
    }

    /// Advances real time: the task scheduler hands out 240 Hz ticks and runs the universe
    /// task once per 15 (at most 8 times per frame, like the original). The frame's ticks are
    /// scaled by the time compression first.
    pub fn due_steps(&mut self, seconds: f64) -> u32 {
        if self.paused {
            return 0;
        }
        self.tick_carry += seconds * f64::from(TIMER_RESOLUTION);
        let ticks = self.tick_carry.floor();
        self.tick_carry -= ticks;
        let scaled = scale_ticks(ticks as i32, self.speed_factor).max(0);
        self.ticker.advance(scaled as u32)
    }

    /// Blend factor between the previous and the current update.
    fn alpha(&self) -> f32 {
        if !self.interpolate || self.paused {
            return 1.0;
        }
        let carry = self.tick_carry * f64::from(self.speed_factor);
        ((f64::from(self.ticker.carry) + carry) / f64::from(self.ticker.ticks_per_call))
            .clamp(0.0, 1.0) as f32
    }

    /// Render transform of each visible ship.
    fn render_transforms(&self) -> Vec<(ShipId, ShipType, Mat4, f32)> {
        let a = self.alpha();
        let age = self.universe().total_time_elapsed;
        self.universe()
            .ships
            .iter()
            .filter(|s| hwc_sim::effects::ship_drawn(self.universe(), s.id))
            .map(|s| {
                let (p, m) = Self::transform_of(s);
                let m = match (a < 1.0, self.previous.get(&s.id)) {
                    (true, Some((pp, pm))) => {
                        let pos = pp.lerp(p, a);
                        let cols = |m: &Mat4, i: usize| m.col(i).truncate();
                        let blend = |i: usize| {
                            cols(pm, i)
                                .lerp(cols(&m, i), a)
                                .normalize_or_zero()
                                .extend(0.0)
                        };
                        Mat4::from_cols(blend(0), blend(1), blend(2), pos.extend(1.0))
                    }
                    _ => m,
                };
                (
                    s.id,
                    drawn_look(self.universe(), s).0,
                    m,
                    age - s.time_created + 0.75,
                )
            })
            .collect()
    }

    /// Prepares one frame: per-instance level of detail, pose, tint and clip, engine trails and
    /// glows, and the combat effects. Call after [`prepare`](Self::prepare).
    pub fn frame(&mut self, size: (u32, u32)) -> Frame {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        let view = self.camera.view();
        let proj = self.camera.projection(aspect);
        let eye = self.camera.eye();
        let glow_texture = self
            .engine_fx
            .map_or(EffectTexture::WHITE, |e| e.glow_texture);
        let hyper = self.hyperspace();
        let now = self.universe().total_time_elapsed;
        // Time since the last tick for the trails' tails (0 without interpolation: the frame
        // shows the last update).
        let tick_alpha = if self.interpolate { self.alpha() } else { 0.0 };
        let mut frame = Frame {
            items: Vec::new(),
            ids: Vec::new(),
            poses: Vec::new(),
            effects: EffectBatch::default(),
            fx_list: hwc_fx::RenderList::default(),
        };
        let mut transforms = self.render_transforms();
        if let Some(h) = &self.hwr {
            transforms.retain(|(id, ..)| !h.replaces_hulk(*id));
        }
        let drawn_at: std::collections::HashMap<ShipId, Mat4> =
            transforms.iter().map(|&(id, _, m, _)| (id, m)).collect();
        let u = self.sim.universe();
        for (id, t, transform, age) in transforms {
            let Some(s) = u.ship(id) else { continue };
            if !self.ship_in_view(s, transform.w_axis.truncate()) {
                continue;
            }
            if cloak_hidden(u, s) {
                continue;
            }
            let color_scheme = drawn_look(u, s).1;
            let Some(art) = self.art.get(&t) else {
                continue;
            };
            let Some(gpu) = art.gpu.get(&color_scheme) else {
                continue;
            };
            let Some(fx) = self.fx.get_mut(&id) else {
                continue;
            };
            let (centre, radius) = gpu.bounds();
            let r = screen_radius(view, proj, transform.transform_point3(centre), radius);
            let d2 = (transform.w_axis.truncate() - eye).length_squared();
            fx.lod = self
                .lod
                .select(gpu.level_count(), fx.lod, r, d2, gpu.lod_table());
            let nominal = fx.lod;
            let lod = drawn_lod(self.full_detail, nominal);
            let pose = fx_pose(art, fx, lod.min(art.bindings.len().saturating_sub(1)));
            let st = u.statics.ship(t);
            let scheme = self
                .team_colors
                .schemes
                .get(color_scheme)
                .or(self.team_colors.schemes.first());
            let team_color = scheme.map_or([1.0; 3], |c| normalized_team_color(c.base));
            // Hyperspace: the window cuts the hull along its heading (`hsGetEquation`). A ship
            // clamped to a host (the mothership's modules) has no window of its own
            // (`hsShouldDisplayEffect`) and is cut by the host's plane while the host is in
            // hyperspace; the host's window is sized to cover them (`hyperspaceBoxScale`).
            let host = clamp_host(self.sim.level(), s);
            let hs = hyper.get(&id).copied();
            let clip = match host {
                Some(h) => hyper.get(&h).and_then(|&(state, clipt)| {
                    let hs = u.ship(h)?;
                    let fl = u
                        .statics
                        .ship(hs.ship_type)
                        .map_or(radius, |st| st.header.coll.forward_length);
                    let plane = hs_plane(state, clipt, fl)?;
                    let (_, host_m) = Self::transform_of(hs);
                    Some((host_m.inverse() * transform).transpose() * plane)
                }),
                None => hs.and_then(|(state, clipt)| {
                    let fl = st.map_or(radius, |st| st.header.coll.forward_length);
                    hs_plane(state, clipt, fl)
                }),
            };
            // `hsEnd` draws the window on top of the HyperWindow effect, effects or not.
            if let (Some((state, clipt)), Some(st), None) = (hs, st, host) {
                hyperspace_window(&mut frame.effects, transform, st, state, clipt);
            }
            let max_velocity = if s.max_velocity_copy > 0.0 {
                s.max_velocity_copy
            } else {
                st.map_or(1.0, |st| st.header.max_velocity)
            };
            // The nozzles go through the drawn level's objects, posed or at rest
            // (`trailGetNozzleWorldPosition`).
            let objects = {
                let sk = gpu.skeleton(lod);
                Some(match &pose {
                    Some(p) => {
                        sk.resolve(|j| p.get(j).copied().flatten().unwrap_or(sk.objects[j].rest))
                    }
                    None => sk.rest_matrices(),
                })
            };
            // N-LIPS (`rndRenderObject`): a ship drawn as a mesh with a scaling cap grows with its
            // distance in front of the camera, and its engines with it.
            let eye_space_z = view.transform_point3(transform.w_axis.truncate()).z;
            let drawn_mesh = art
                .table
                .as_ref()
                .and_then(|t| t.levels.get(lod))
                .is_none_or(|l| l.kind == LodKind::Mesh);
            let nlips = match st {
                Some(st) if st.scale_cap != 0.0 && eye_space_z < 0.0 && drawn_mesh => {
                    1.0 - eye_space_z * st.scale_cap
                }
                _ => 1.0,
            };
            let gate = TrailGate {
                moving: s.posinfo.is_moving != 0 || v3(s.posinfo.velocity).length_squared() > 1.0,
                docked: s.base.dock_state == hwc_sim::dock::state::INSIDE,
                hyperspacing: hs.is_some(),
                cloaked: cloak_hidden(u, s),
                suppressed: s.base.clamp.is_some() || s.flags & hwc_sim::base::SOF_MODULE != 0,
                idle_dodge: s.is_dodging && s.ship_idle,
            };
            let ribbon = self
                .hwr
                .as_mut()
                .and_then(|h| h.ribbon(u, t, art.trail.collision_radius));
            fx.trails.draw_styled(
                &art.trail,
                &self.trail_tweaks,
                &TrailFrame {
                    transform,
                    objects: objects.as_ref().map(|m| (gpu.skeleton(lod), m.as_slice())),
                    velocity: v3(s.posinfo.velocity),
                    max_velocity,
                    gate,
                    lod,
                    team: color_scheme,
                    team_color,
                    eye,
                    eye_space_z,
                    scale: nlips,
                    fade: 1.0,
                    tick_alpha,
                    glow_texture,
                },
                ribbon.as_ref(),
                &mut frame.effects,
            );
            let tint = fx
                .tint
                .params(team_color, fx.trails.engine_glow_levels(&self.trail_tweaks));
            let construction = st.and_then(|st| construction_state(s, &st.header.coll));
            // A depot's beam eating the ship (`consumeGetClipPlane`).
            let clip = hwc_sim::consume::clip_plane(u, id)
                .map(|(n, d)| Vec4::new(n.x, n.y, n.z, d))
                .or(clip);
            frame.items.push(Item {
                kind: ItemKind::Ship(t, color_scheme),
                transform: transform * Mat4::from_scale(Vec3::splat(nlips)),
                age,
                lod,
                clip,
                tint,
                time: now,
                construction,
                infection: infection_look(u, s).map(|(t, p)| (t, s.color_scheme, p)),
                lightning: lightning_on(u, s),
                emp_ambient: self
                    .naggarok_emp(s)
                    .then(|| hwc_render::render_state::naggarok_base(u.update_counter)),
                fade: cloak_fade(s),
                untextured: cloak_untextured(s),
                retrofit: hwc_sim::salvage::retrofit_look(u, id),
            });
            frame.poses.push(pose);
            let mesh = art
                .table
                .as_ref()
                .and_then(|t| t.levels.get(nominal))
                .is_none_or(|l| l.kind == LodKind::Mesh);
            frame.ids.push((Some(id), mesh));
        }
        // Dead hulls the Remastered death sequences keep in view.
        if let Some(h) = &self.hwr {
            let now = f64::from(now) + f64::from(self.alpha()) * f64::from(TICK);
            for (g, transform) in h.ghosts(now) {
                let scheme = g.color_scheme;
                let uploaded = self
                    .art
                    .get(&g.ship_type)
                    .is_some_and(|a| a.gpu.contains_key(&scheme));
                if !uploaded || !self.object_in_view(transform.w_axis.truncate(), true) {
                    continue;
                }
                frame.items.push(Item {
                    kind: ItemKind::Ship(g.ship_type, scheme),
                    transform,
                    age: 10.0,
                    lod: 0,
                    clip: None,
                    tint: TintParams::default(),
                    time: now as f32,
                    construction: None,
                    infection: None,
                    lightning: false,
                    emp_ambient: None,
                    fade: 1.0,
                    untextured: false,
                    retrofit: None,
                });
                frame.poses.push(None);
                frame.ids.push((None, true));
            }
        }
        for r in &u.base.resources.list {
            let ResourceKind::Asteroid(k) = r.kind else {
                continue;
            };
            if r.flags & hwc_sim::object::sof::HIDE != 0
                || !self.object_drawn(v3(r.position), false, true)
            {
                continue;
            }
            if !self
                .resource_art
                .get(&k)
                .is_some_and(|a| a.as_ref().is_some_and(|a| a.gpu.is_some()))
            {
                continue;
            }
            frame.items.push(Item {
                kind: ItemKind::Asteroid(k),
                transform: matrix(&r.coordsys, v3(r.position)),
                age: 10.0,
                lod: 0,
                clip: None,
                tint: TintParams::default(),
                time: now,
                construction: None,
                infection: None,
                lightning: false,
                emp_ambient: None,
                fade: 1.0,
                untextured: false,
                retrofit: None,
            });
            frame.poses.push(None);
            frame.ids.push((None, false));
        }
        // Missiles and mines (`rndRenderTask`'s `OBJ_MissileType`: their LOD meshes, in the
        // owner's colours). The sensors manager draws mines as points of its own.
        if !self.sensors_open {
            let a = self.alpha();
            for &mid in &u.missile_list {
                let Some(m) = u.missiles.get(mid) else {
                    continue;
                };
                if m.flags & hwc_sim::object::sof::HIDE != 0 {
                    continue;
                }
                let key = missile_key(m);
                let scheme = usize::from(m.color_scheme);
                let Some(gpu) = self
                    .missile_art
                    .get(&key)
                    .and_then(Option::as_ref)
                    .and_then(|art| art.gpu.get(&scheme))
                else {
                    continue;
                };
                // Between updates a missile moves on along its velocity.
                let p = v3(m.posinfo.position) + v3(m.posinfo.velocity) * (TICK * (a - 1.0));
                if !self.object_in_view(p, false) {
                    continue;
                }
                let transform = matrix(&m.rotinfo.coordsys, p);
                let (centre, radius) = gpu.bounds();
                let r = screen_radius(view, proj, transform.transform_point3(centre), radius);
                let d2 = (p - eye).length_squared();
                let nominal = self
                    .lod
                    .select(gpu.level_count(), 0, r, d2, gpu.lod_table());
                frame.items.push(Item {
                    kind: ItemKind::Missile(key, scheme),
                    transform,
                    age: 10.0,
                    lod: drawn_lod(self.full_detail, nominal),
                    clip: None,
                    tint: TintParams::default(),
                    time: now,
                    construction: None,
                    infection: None,
                    lightning: false,
                    emp_ambient: None,
                    fade: 1.0,
                    untextured: false,
                    retrofit: None,
                });
                frame.poses.push(None);
                frame.ids.push((None, true));
            }
            // The crates, where they lie.
            if let Some(gpu) = self.missile_art.get(&CRATE_KEY).and_then(Option::as_ref).and_then(|art| art.gpu.get(&0)) {
                for c in &u.mp.crates.list {
                    let p = v3(c.position);
                    if !self.object_in_view(p, false) {
                        continue;
                    }
                    let transform = Mat4::from_translation(p);
                    let (centre, radius) = gpu.bounds();
                    let r = screen_radius(view, proj, transform.transform_point3(centre), radius);
                    let d2 = (p - eye).length_squared();
                    let nominal = self.lod.select(gpu.level_count(), 0, r, d2, gpu.lod_table());
                    frame.items.push(Item {
                        kind: ItemKind::Missile(CRATE_KEY, 0),
                        transform,
                        age: 10.0,
                        lod: drawn_lod(self.full_detail, nominal),
                        clip: None,
                        tint: TintParams::default(),
                        time: now,
                        construction: None,
                        infection: None,
                        lightning: false,
                        emp_ambient: None,
                        fade: 1.0,
                        untextured: false,
                        retrofit: None,
                    });
                    frame.poses.push(None);
                    frame.ids.push((None, true));
                }
            }
        }
        self.combat_effects(&mut frame.effects, glow_texture);
        let field_time = self.universe().total_time_elapsed + (self.alpha() - 1.0) * TICK;
        let u = self.sim.universe();
        let seed = &mut self.field_random;
        let modern = self
            .fields_look
            .as_ref()
            .filter(|_| self.modern_fields)
            .map(|m| (m, field_time));
        crate::sentinel_field::draw(
            u,
            frame.ids.iter().filter_map(|(id, _)| *id),
            |id| {
                drawn_at
                    .get(&id)
                    .copied()
                    .or_else(|| u.ship(id).map(|s| Self::transform_of(s).1))
            },
            eye,
            glow_texture,
            &mut || {
                // The particle stream (`ranRandomFn(0)`) is not the simulation's.
                *seed ^= *seed << 13;
                *seed ^= *seed >> 17;
                *seed ^= *seed << 5;
                *seed
            },
            modern,
            &mut frame.effects,
        );
        if let Some(mut h) = self.hwr.take() {
            let u = self.sim.universe();
            let now = f64::from(u.total_time_elapsed) + f64::from(self.alpha()) * f64::from(TICK);
            h.draw(u, now, &mut frame.effects);
            if self.effects_on {
                hwc_sim::effects::draw_except(self.sim.universe_mut(), &mut frame.fx_list, |e| {
                    h.hides(e)
                });
            }
            self.hwr = Some(h);
        } else if self.effects_on {
            hwc_sim::effects::draw(self.universe_mut(), &mut frame.fx_list);
        }
        if self.effects_on {
            // Effects are objects of the render list too.
            retain_fx_systems(&mut frame.fx_list, |p| self.object_in_view(p, false));
        }
        // Nothing unbounded goes to the GPU; the counts go to the crash log's history.
        let mut stats = crate::draw_check::Stats::default();
        let keep: Vec<bool> = frame
            .items
            .iter()
            .map(|i| i.transform.is_finite())
            .collect();
        if keep.iter().any(|k| !k) {
            stats.not_finite += keep.iter().filter(|k| !**k).count();
            let mut k = keep.iter();
            frame.items.retain(|_| *k.next().unwrap_or(&true));
            let mut k = keep.iter();
            frame.ids.retain(|_| *k.next().unwrap_or(&true));
            let mut k = keep.iter();
            frame.poses.retain(|_| *k.next().unwrap_or(&true));
        }
        stats.ships = frame.items.len();
        crate::draw_check::check_effects(&mut frame.fx_list, &mut stats);
        crate::draw_check::check_batch(&mut frame.effects, &mut stats);
        self.frames_drawn += 1;
        crate::draw_check::record(self.frames_drawn, &stats);
        frame
    }

    /// Builds the effect geometry of a frame for this camera, with the lens flares (each new
    /// flare hidden when a ship's collision box lies between it and the eye, the original's
    /// `collCheckRectLine` test). False when there is nothing to draw.
    fn prepare_effects(&mut self, renderer: &Renderer, size: (u32, u32), frame: &Frame) -> bool {
        if !self.effects_on {
            return false;
        }
        let src = &self.src;
        let fx = self.fx_renderer.get_or_insert_with(|| {
            let mut fx = hwc_render::EffectRenderer::new(renderer);
            if !fx.load_lens_flares(renderer, src, LENS_FLARE_SET) {
                eprintln!("warning: lens flare set {LENS_FLARE_SET} not found");
            }
            fx
        });
        let view = hwc_render::FxView::from_orbit(&self.camera, size.0, size.1);
        fx.prepare(renderer, &self.src, &frame.fx_list, &view, &self.lighting);
        let Some(world) = self.sim.universe_mut().effects.as_mut() else {
            return true;
        };
        let mut flares = std::mem::take(&mut world.world.lens_flares);
        let u = self.sim.universe();
        let occluded = |pos: Vec3, dir: Vec3, dist: f32| flare_occluded(u, pos, dir, dist);
        let sun = Some(&mut self.nis_sun).filter(|s| s.listed);
        fx.prepare_lens_flares_with_nis_sun(&mut flares, sun, &view, Some(&occluded));
        if let Some(world) = self.sim.universe_mut().effects.as_mut() {
            world.world.lens_flares = flares;
        }
        true
    }

    /// Bullets, beams, harvesting beams, hit sparks and explosion glows as effect primitives.
    fn combat_effects(&self, out: &mut EffectBatch, glow: EffectTexture) {
        let u = self.universe();
        if self.effects_on {
            original_bullets(u, out, |p| self.object_in_view(p, false));
            return;
        }
        for &bid in &u.bullet_list {
            let b = u.bullets.bullet(bid);
            let p = v3(b.posinfo.position);
            let beam = hwc_sim::bullet::is_beam(b.weapon);
            let tail = if beam {
                p + v3(b.heading) * b.beam_travel_dist
            } else {
                p - v3(b.heading) * b.length_mag.max(30.0)
            };
            let c = WEAPON_COLORS[(b.weapon as usize).min(WEAPON_COLORS.len() - 1)];
            let mut faded = c;
            faded[3] = 0.0;
            out.lines.push(FxLine {
                from: p,
                to: tail,
                from_color: c,
                to_color: if beam { c } else { faded },
                width: if beam { 3.0 } else { 1.5 },
                blend: Blend::Additive,
            });
        }
        for s in u
            .ships
            .iter()
            .filter(|s| s.flags & hwc_sim::object::sof::HIDE == 0 && s.base.harvest_effect)
        {
            let target = s
                .command
                .and_then(|c| u.commands.get(c))
                .and_then(|c| c.collect.resource)
                .and_then(|r| u.base.resources.get(r));
            if let Some(r) = target {
                out.lines.push(FxLine {
                    from: v3(s.posinfo.position),
                    to: v3(r.position),
                    from_color: BEAM_COLOR,
                    to_color: BEAM_COLOR,
                    width: 2.0,
                    blend: Blend::Additive,
                });
            }
        }
        let now = u.total_time_elapsed;
        for e in &self.effects {
            let (size, color) = if e.explosion {
                let k = ((now - e.start) / EXPLOSION_TIME).clamp(0.0, 1.0);
                let mut c = EXPLOSION_COLOR;
                c[3] = 1.0 - k;
                (e.radius.max(40.0) * (0.8 + 2.2 * k), c)
            } else {
                let k = ((now - e.start) / HIT_TIME).clamp(0.0, 1.0);
                let mut c = HIT_COLOR;
                c[3] = 1.0 - k;
                (12.0, c)
            };
            out.sprites.push(Sprite {
                position: e.position,
                half_size: Vec2::splat(size),
                rotation: e.start * 7.0,
                color,
                uv: FULL_UV,
                texture: glow,
                blend: Blend::Additive,
            });
        }
    }

    /// `rndSetupObjectRenderStates`' Naggarok state (`rndShipMovingUnderNaggarokEMP` 0x570480):
    /// the Naggarok (type 0x6d) under way once the script's `G_NaggarokEMP` counts more than the
    /// EMPs that have hit it (unsigned). `meshCurrentMaterialDefault` then pulses its materials'
    /// ambient ([`ShipRenderState::emp_ambient`]); it draws no bolts.
    fn naggarok_emp(&self, s: &hwc_sim::object::Ship) -> bool {
        s.ship_type == ShipType::dBeastMothership
            && s.posinfo.velocity.mag_sq() > 0.01
            && self
                .mission_run()
                .and_then(|r| r.var(None, "G_NaggarokEMP"))
                .is_some_and(|v| (s.disable.emp_count as u32) < v as u32)
    }

    /// The ship instances and their render states for a prepared frame.
    pub fn instances<'a>(
        &'a self,
        frame: &'a Frame,
    ) -> (Vec<ShipInstance<'a>>, Vec<ShipRenderState<'a>>) {
        let mut ships = Vec::with_capacity(frame.items.len());
        let mut states = Vec::with_capacity(frame.items.len());
        for (item, pose) in frame.items.iter().zip(&frame.poses) {
            let gpu = match item.kind {
                ItemKind::Ship(t, scheme) => self.art.get(&t).and_then(|a| a.gpu.get(&scheme)),
                ItemKind::Missile(key, scheme) => self
                    .missile_art
                    .get(&key)
                    .and_then(|a| a.as_ref())
                    .and_then(|a| a.gpu.get(&scheme)),
                ItemKind::Asteroid(k) => self
                    .resource_art
                    .get(&k)
                    .and_then(|a| a.as_ref())
                    .and_then(|a| a.gpu.as_ref()),
            };
            let Some(gpu) = gpu else { continue };
            ships.push(ShipInstance {
                ship: gpu,
                transform: item.transform,
                age: item.age,
            });
            let infection = item.infection.and_then(|(t, scheme, p)| {
                let infected = self.art.get(&t).and_then(|a| a.gpu.get(&scheme))?;
                Some((infected, p))
            });
            let phase = item.retrofit.and_then(|(start, end, t, scheme)| {
                let art = self.art.get(&t)?;
                let (stage, value) = hwc_render::render_state::phase_blend_state(item.time, start, end)?;
                let coll = &self.universe().statics.ship(t)?.header.coll;
                Some(hwc_render::render_state::PhaseState {
                    stage,
                    value,
                    converted: art.gpu.get(&scheme),
                    collision_box: CollisionBox {
                        corner: v3(coll.coll_rect_offset),
                        extent: Vec3::new(coll.up_length, coll.right_length, coll.forward_length),
                    },
                })
            });
            states.push(ShipRenderState {
                lod: item.lod,
                pose: pose.as_deref(),
                clip_plane: item.clip,
                tint: item.tint,
                time: item.time,
                construction: item.construction,
                infection,
                emp_lightning: item.lightning,
                emp_ambient: item.emp_ambient,
                fade: item.fade,
                untextured: item.untextured,
                phase,
                ..Default::default()
            });
        }
        (ships, states)
    }

    pub fn scene<'a>(&'a self, instances: &'a [ShipInstance<'a>]) -> Scene<'a> {
        Scene {
            camera: &self.camera,
            lighting: &self.lighting,
            background: self.background.as_ref(),
            ships: instances,
            nav_lights: self.nav_lights,
        }
    }
    /// Renders a prepared frame offscreen, the simulation's effects over the scene.
    pub fn render_offscreen(
        &mut self,
        renderer: &mut Renderer,
        target: &hwc_render::OffscreenTarget,
        frame: &Frame,
    ) -> Result<Vec<u8>, String> {
        let effects = self.prepare_effects(renderer, (target.width, target.height), frame);
        let nebulae = !self.sensors_open
            && crate::nebula_view::prepare(
                &mut self.neb_renderer,
                renderer,
                self.sim.universe(),
                &self.camera,
                (target.width, target.height),
            );
        // `plasmaCloudUpdate` before the scene, `plasmaCloudRender` after it.
        let now = self.sim.universe().total_time_elapsed;
        self.plasma.update(now);
        let cloud = if self.sensors_open {
            None
        } else {
            self.plasma.render(now)
        };
        renderer.set_sky_brightness(if self.sensors_open {
            1.0
        } else {
            self.plasma.sky
        });
        if self.plasma.texture && self.plasma_renderer.is_none() {
            self.plasma_renderer = Some(
                hwc_render::assets::load_lif(&self.src, hwc_render::plasma_cloud::TEXTURE)
                    .map_err(|e| eprintln!("warning: plasma cloud: {e}"))
                    .ok()
                    .map(|l| {
                        hwc_render::PlasmaCloudRenderer::new(
                            renderer,
                            l.width,
                            l.height,
                            &l.to_rgba(),
                        )
                    }),
            );
        }
        let mut plasma = self.plasma_renderer.take();
        if let Some(Some(p)) = plasma.as_mut() {
            p.prepare(cloud.as_ref(), target.width, target.height);
        }
        let mut neb = self.neb_renderer.take();
        let mut fx = self.fx_renderer.take();
        let out = {
            let (ships, states) = self.instances(frame);
            let scene = self.scene(&ships);
            let extras = SceneExtras {
                ship_states: &states,
                effects: Some(&frame.effects),
            };
            target.render_ex_with(renderer, &scene, &extras, |r, enc, c, d| {
                if let Some(fx) = fx.as_mut().filter(|_| effects) {
                    fx.draw(r, enc, c, d);
                }
                // `nebRender`, after the effects.
                if let Some(n) = neb.as_mut().filter(|_| nebulae) {
                    n.draw(r, enc, c, d);
                }
                if let Some(Some(p)) = plasma.as_mut() {
                    p.draw(r, enc, c);
                }
            })
        };
        renderer.set_sky_brightness(1.0);
        self.fx_renderer = fx;
        self.neb_renderer = neb;
        self.plasma_renderer = plasma;
        out
    }

    /// Renders only the background (the sensors manager's backdrop), or the clear colour
    /// when `background` is off, with `layer`'s pixel triangles over it, into `out` (RGBA
    /// rows of the whole target).
    pub fn render_background(
        &self,
        renderer: &mut Renderer,
        target: &hwc_render::OffscreenTarget,
        background: bool,
        layer: &EffectBatch,
        out: &mut [u8],
    ) -> Result<(), String> {
        let scene = Scene {
            camera: &self.camera,
            lighting: &self.lighting,
            background: if background {
                self.background.as_ref()
            } else {
                None
            },
            ships: &[],
            nav_lights: false,
        };
        let extras = SceneExtras {
            ship_states: &[],
            effects: Some(layer),
        };
        target.render_ex_into(renderer, &scene, &extras, out)
    }

    /// Renders the frame's ships for which `keep(id, mesh_level)` holds over a black clear,
    /// with no background and no effects (the sensors manager's ship meshes), reading back
    /// the rectangle `region` (`x, y, w, h`; all of the target for `None`). Returns the rows
    /// and the rectangle read.
    pub fn render_ships_only(
        &self,
        renderer: &mut Renderer,
        target: &hwc_render::OffscreenTarget,
        frame: &Frame,
        region: Option<[u32; 4]>,
        keep: impl Fn(ShipId, bool) -> bool,
    ) -> Result<Option<RegionPixels>, String> {
        let (all, all_states) = self.instances(frame);
        let mut ships = Vec::new();
        let mut states = Vec::new();
        for ((ship, state), &(id, mesh)) in all.into_iter().zip(all_states).zip(&frame.ids) {
            if id.is_some_and(|id| keep(id, mesh)) {
                ships.push(ship);
                states.push(state);
            }
        }
        if ships.is_empty() {
            return Ok(None);
        }
        let scene = Scene {
            camera: &self.camera,
            lighting: &self.lighting,
            background: None,
            ships: &ships,
            nav_lights: false,
        };
        let extras = SceneExtras {
            ship_states: &states,
            effects: None,
        };
        let region = target.clamp_region(region);
        target
            .render_ex_region(renderer, &scene, &extras, region)
            .map(|rgba| Some((rgba, region)))
    }

    /// Bounding radius of an asteroid kind's mesh (for picking).
    pub fn resource_radius(&self, r: &hwc_sim::resource::Resource) -> f32 {
        match r.kind {
            ResourceKind::Asteroid(k) => self
                .resource_art
                .get(&k)
                .and_then(|a| a.as_ref())
                .and_then(|a| a.gpu.as_ref())
                .map_or(50.0, |g| g.bounds().1),
            ResourceKind::DustCloud(_) | ResourceKind::Nebula(_) => 200.0,
        }
    }

    /// The nearest resource whose screen box contains the point.
    pub fn resource_at(
        &self,
        size: (u32, u32),
        px: [f32; 2],
    ) -> Option<hwc_sim::resource::ResourceId> {
        let right = self.camera.view().row(0).truncate();
        let mut best: Option<(f32, hwc_sim::resource::ResourceId)> = None;
        for r in &self.universe().base.resources.list {
            let c = v3(r.position);
            let Some(p) = self.project(size, c) else {
                continue;
            };
            let Some(q) = self.project(size, c + right * self.resource_radius(r)) else {
                continue;
            };
            let rad = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2))
                .sqrt()
                .max(6.0);
            if (px[0] - p[0]).abs() <= rad && (px[1] - p[1]).abs() <= rad {
                let d = (c - self.camera.eye()).length();
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, r.id));
                }
            }
        }
        best.map(|b| b.1)
    }

    pub fn view_proj(&self, size: (u32, u32)) -> Mat4 {
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        self.camera.projection(aspect) * self.camera.view()
    }

    /// World point to pixel (None behind the camera).
    pub fn project(&self, size: (u32, u32), p: Vec3) -> Option<[f32; 2]> {
        let c = self.view_proj(size) * p.extend(1.0);
        if c.w <= 1e-3 {
            return None;
        }
        let n = c.truncate() / c.w;
        Some([
            (n.x + 1.0) * 0.5 * size.0 as f32,
            (1.0 - n.y) * 0.5 * size.1 as f32,
        ])
    }

    /// The level of detail a ship was last drawn at (`ship->currentLOD`).
    pub fn ship_lod(&self, id: ShipId) -> Option<usize> {
        self.fx.get(&id).map(|f| f.lod)
    }

    /// Screen box of a ship (its collision sphere's projection).
    pub fn screen_box(&self, size: (u32, u32), s: &hwc_sim::Ship) -> Option<([f32; 2], [f32; 2])> {
        let r = self
            .universe()
            .statics
            .ship(s.ship_type)
            .map_or(50.0, |st| st.header.coll.coll_sphere_size);
        self.sphere_box(size, v3(s.coll.coll_position), r)
    }

    /// Screen box of a live mine (`id` is its selection id) that is drawn in the main view.
    pub fn mine_box(&self, size: (u32, u32), id: ShipId) -> Option<([f32; 2], [f32; 2])> {
        let u = self.universe();
        let m = u.missiles.get(hwc_sim::target::live_mine(u, id)?)?;
        if m.flags & hwc_sim::object::sof::HIDE != 0
            || !self.object_in_view(v3(m.posinfo.position), false)
        {
            return None;
        }
        let r =
            hwc_sim::missile::statics_of(u, m).map_or(50.0, |st| st.header.coll.coll_sphere_size);
        self.sphere_box(size, v3(m.coll.coll_position), r)
    }

    /// Screen box of a sphere of radius `r` (at least 20) about `c`.
    fn sphere_box(&self, size: (u32, u32), c: Vec3, r: f32) -> Option<([f32; 2], [f32; 2])> {
        let r = r.max(20.0);
        let p = self.project(size, c)?;
        let right = self.camera.view().row(0).truncate();
        let q = self.project(size, c + right * r)?;
        let rad = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2))
            .sqrt()
            .max(6.0);
        Some(([p[0] - rad, p[1] - rad], [p[0] + rad, p[1] + rad]))
    }

    /// The nearest object under the point among the ships `keep` lets through and the mines
    /// `keep_mine` does (a mine is given by its selection id).
    pub fn object_at_where(
        &self,
        size: (u32, u32),
        px: [f32; 2],
        keep: impl Fn(ShipId) -> bool,
        keep_mine: impl Fn(ShipId) -> bool,
    ) -> Option<ShipId> {
        let ship = self.ship_at_where(size, px, keep);
        let eye = self.camera.eye();
        let u = self.universe();
        let dist = |id: ShipId| {
            hwc_sim::target::get(u, id)
                .map_or(f32::MAX, |t| (v3(t.posinfo.position) - eye).length())
        };
        let mut best = ship.map(|id| (dist(id), id));
        for &mid in &u.missile_list {
            let Some(id) = ShipId::from_mine(mid) else {
                continue;
            };
            if !keep_mine(id) {
                continue;
            }
            let Some((lo, hi)) = self.mine_box(size, id) else {
                continue;
            };
            if px[0] >= lo[0] && px[0] <= hi[0] && px[1] >= lo[1] && px[1] <= hi[1] {
                let d = dist(id);
                if best.is_none_or(|b| d < b.0) {
                    best = Some((d, id));
                }
            }
        }
        best.map(|b| b.1)
    }

    /// The nearest visible ship whose screen box contains the point.
    pub fn ship_at(&self, size: (u32, u32), px: [f32; 2]) -> Option<ShipId> {
        self.ship_at_where(size, px, |_| true)
    }

    /// Like [`ship_at`](Self::ship_at) among the ships `keep` accepts.
    pub fn ship_at_where(
        &self,
        size: (u32, u32),
        px: [f32; 2],
        keep: impl Fn(ShipId) -> bool,
    ) -> Option<ShipId> {
        let mut best: Option<(f32, ShipId)> = None;
        for s in self.universe().ships.iter().filter(|s| {
            s.flags & hwc_sim::object::sof::HIDE == 0
                && keep(s.id)
                && self.ship_in_view(s, v3(s.posinfo.position))
        }) {
            if let Some((lo, hi)) = self.screen_box(size, s) {
                if px[0] >= lo[0] && px[0] <= hi[0] && px[1] >= lo[1] && px[1] <= hi[1] {
                    let d = (v3(s.posinfo.position) - self.camera.eye()).length();
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, s.id));
                    }
                }
            }
        }
        best.map(|b| b.1)
    }
}
/// The ship a ship is clamped to: its clamp host, or for a mission, the level's module clamps.
fn clamp_host(level: &LevelState, s: &hwc_sim::Ship) -> Option<ShipId> {
    s.base.clamp.as_ref().and_then(|c| c.host).or_else(|| {
        level
            .clamps
            .iter()
            .find(|(m, _)| *m == s.id)
            .map(|(_, h)| *h)
    })
}

/// The construction state of a ship flagged as under external construction
/// (`rndSetupShipFadeState`): its build progress and its collision box in mesh space.
fn construction_state(
    s: &hwc_sim::Ship,
    coll: &hwc_sim::statics::header::StaticCollInfo,
) -> Option<ConstructionState> {
    if s.special_flags2 & hwc_sim::base::special2::UNDER_CONSTRUCTION == 0 {
        return None;
    }
    Some(ConstructionState {
        progress: s.base.build_progress,
        collision_box: CollisionBox {
            corner: v3(coll.coll_rect_offset),
            extent: Vec3::new(coll.up_length, coll.right_length, coll.forward_length),
        },
    })
}

/// The hyperspace clip plane in ship space for effect state `state` (`hsGetEquation`): points
/// with `plane . (p, 1) >= 0` are drawn.
/// RGBA rows read back from part of a picture, and the rectangle (`x, y, w, h`) they cover.
pub type RegionPixels = (Vec<u8>, [u32; 4]);

fn hs_plane(state: u32, clipt: f32, forward_length: f32) -> Option<Vec4> {
    let fl = forward_length;
    Some(match state {
        hs_effect::POPUP_OUTOF => Vec4::new(0.0, 0.0, 1.0, -fl),
        hs_effect::SLICING_OUTOF => Vec4::new(0.0, 0.0, 1.0, -clipt * fl),
        hs_effect::COLLAPSE_OUTOF => Vec4::new(0.0, 0.0, 1.0, fl),
        hs_effect::SLICING_INTO => Vec4::new(0.0, 0.0, -1.0, clipt * fl),
        hs_effect::COLLAPSE_INTO => Vec4::new(0.0, 0.0, -1.0, -fl),
        _ => return None,
    })
}

/// The ship's current `.mad` pose for detail level `level` (None when nothing is playing).
fn fx_pose(art: &ShipArt, fx: &ShipFx, level: usize) -> Option<Vec<Option<Mat4>>> {
    let clip = art.clip.as_ref()?;
    fx.anim.current?;
    let binding = art.bindings.get(level)?;
    clip.pose(&fx.anim, binding)
}

/// The hyperspace window (`hsEnd`, drawn by [`hwc_render::effects::hyperspace_window`]) of a
/// ship in hyperspace state `state`; `transform` is its model matrix.
fn hyperspace_window(
    out: &mut EffectBatch,
    transform: Mat4,
    st: &hwc_sim::statics::ShipStaticInfo,
    state: u32,
    clipt: f32,
) {
    let c = &st.header.coll;
    let [r, g, b, _] = st.hyperspace_color;
    let shape = hwc_render::effects::HyperspaceWindowShape {
        forward_length: c.forward_length,
        up_length: c.up_length,
        right_length: c.right_length,
        coll_sphere_offset: Vec3::new(
            c.coll_sphere_offset.x,
            c.coll_sphere_offset.y,
            c.coll_sphere_offset.z,
        ),
        box_scale: st.hyperspace_box_scale,
        color: [r, g, b],
    };
    hwc_render::effects::hyperspace_window(out, transform, &shape, state, clipt);
}

/// Whether a ship's collision box hides a lens flare at `pos` (looking along `dir` towards the
/// eye, `dist` away).
fn flare_occluded(u: &Universe, pos: Vec3, dir: Vec3, dist: f32) -> bool {
    let p = hwc_math::Vector::new(pos.x, pos.y, pos.z);
    let d = hwc_math::Vector::new(dir.x, dir.y, dir.z);
    u.ships
        .iter()
        .filter(|s| hwc_sim::effects::ship_drawn(u, s.id))
        .any(|s| {
            u.statics.ship(s.ship_type).is_some_and(|st| {
                let (t, _) = hwc_sim::collision::rect_line(s, st, p, d, dist);
                t > 0.0
            })
        })
}

/// The bullets the original draws itself (`rndRenderObject`): those without an effect, as a
/// one-pixel line from the bullet along its length in the bullet colour. Ion beams are not
/// drawn in their first update and Beast infection beams never.
/// The render list's reach (`univUpdateRenderList`) and the edge fade (`rndFade`), from
/// `Tweak.script`: per ship class, the distance from the camera's look-at point within which
/// a ship is listed for drawing, and the band at the edge over which it fades out (measured
/// from the nearer of the look-at point and the eye). Both are radial distances, so what is
/// drawn does not depend on where in the picture a ship lies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderLimits {
    /// `RENDER_LIMIT_*` and `RENDER_FADE_*` by class: mothership, heavy cruiser, carrier,
    /// destroyer, frigate, corvette, fighter, resource, non-combat.
    pub limit: [f32; 9],
    pub fade: [f32; 9],
    /// `RENDER_VIEWABLE_DISTANCE_SQR` / `RENDER_MAXVIEWABLE_DISTANCE_SQR` (the script gives
    /// the distances, squared on load; kept unsquared here) and `RENDER_VIEWABLE_FADE` /
    /// `RENDER_MAXVIEWABLE_FADE`: the other objects (effects, bullets, asteroids), normal and
    /// big.
    pub viewable: f32,
    pub max_viewable: f32,
    pub viewable_fade: f32,
    pub max_viewable_fade: f32,
}

/// `rndFade` stretches the limits by this much.
const FADE_REACH: f32 = 1.3;

impl Default for RenderLimits {
    fn default() -> Self {
        let mut limit = [16000.0; 9];
        limit[0] = 18000.0;
        RenderLimits {
            limit,
            fade: [5000.0; 9],
            viewable: 16000.0,
            max_viewable: 20000.0,
            viewable_fade: 5000.0,
            max_viewable_fade: 3500.0,
        }
    }
}

impl RenderLimits {
    pub fn from_script(s: &hwc_formats::script::ScriptFile) -> Self {
        let mut t = Self::default();
        let names = [
            "MOTHERSHIP",
            "HEAVYCRUISER",
            "CARRIER",
            "DESTROYER",
            "FRIGATE",
            "CORVETTE",
            "FIGHTER",
            "RESOURCE",
            "NONCOMBAT",
        ];
        let get = |k: &str| s.get(k).and_then(|e| e.f32());
        for (i, n) in names.iter().enumerate() {
            if let Some(v) = get(&format!("RENDER_LIMIT_{n}")) {
                t.limit[i] = v;
            }
            if let Some(v) = get(&format!("RENDER_FADE_{n}")) {
                t.fade[i] = v;
            }
        }
        let slots: [(&str, &mut f32); 4] = [
            ("RENDER_VIEWABLE_DISTANCE_SQR", &mut t.viewable),
            ("RENDER_MAXVIEWABLE_DISTANCE_SQR", &mut t.max_viewable),
            ("RENDER_VIEWABLE_FADE", &mut t.viewable_fade),
            ("RENDER_MAXVIEWABLE_FADE", &mut t.max_viewable_fade),
        ];
        for (k, slot) in slots {
            if let Some(v) = get(k) {
                *slot = v;
            }
        }
        t
    }

    /// The render list's class slot: monster ships and components take the mothership's
    /// limit, micro ships the fighters'.
    fn list_slot(class: Option<ShipClass>) -> usize {
        match class {
            Some(ShipClass::Mothership | ShipClass::MonsterShip | ShipClass::Component) | None => 0,
            Some(ShipClass::HeavyCruiser) => 1,
            Some(ShipClass::Carrier) => 2,
            Some(ShipClass::Destroyer) => 3,
            Some(ShipClass::Frigate) => 4,
            Some(ShipClass::Corvette) => 5,
            Some(ShipClass::Fighter | ShipClass::Micro) => 6,
            Some(ShipClass::Resource) => 7,
            Some(ShipClass::NonCombat) => 8,
        }
    }

    /// The fade's class slot: components by their type (two of them take the carrier's and
    /// the destroyer's values, the rest the mothership's), an unknown class the frigate's.
    fn fade_slot(class: Option<ShipClass>, ship_type: ShipType) -> usize {
        match class {
            Some(ShipClass::Component) => match ship_type.index() {
                13 => 2,
                11 => 3,
                _ => 0,
            },
            None => 4,
            c => Self::list_slot(c),
        }
    }

    /// Whether a ship is drawn: it is in the render list (within its limit of the look-at
    /// point; `look_at_sq` is that distance squared) and not wholly faded out at the edge
    /// (`nearest_sq`: the squared distance from the nearer of the look-at point and the eye).
    /// `static_fade` / `static_limit_sq` are the ship's own `renderlistFade` /
    /// `renderlistLimit` (squared), 0 when it has none.
    pub fn ship_drawn(
        &self,
        class: Option<ShipClass>,
        ship_type: ShipType,
        static_fade: f32,
        static_limit_sq: f32,
        look_at_sq: f32,
        nearest_sq: f32,
    ) -> bool {
        let list = if static_limit_sq != 0.0 {
            static_limit_sq
        } else {
            let l = self.limit[Self::list_slot(class)];
            l * l
        };
        if look_at_sq >= list {
            return false;
        }
        let (fade, limit) = if static_fade != 0.0 {
            (static_fade, static_limit_sq.max(0.0).sqrt())
        } else {
            let i = Self::fade_slot(class, ship_type);
            (self.fade[i], self.limit[i])
        };
        let fade = fade * FADE_REACH;
        let start = limit * FADE_REACH - fade;
        nearest_sq < start * start + fade * fade
    }

    /// Whether another object (an effect, a bullet, an asteroid) at squared distance
    /// `look_at_sq` from the look-at point is in the render list: within the viewable
    /// distance, or within the longer one for a big object.
    pub fn object_listed(&self, look_at_sq: f32, big: bool) -> bool {
        let max = self.max_viewable * self.max_viewable;
        let normal = self.viewable * self.viewable;
        look_at_sq < max && (look_at_sq < normal || big)
    }
}

/// Keeps the particle systems of an effect list whose first primitive `keep` accepts (whole
/// systems, so a cut never splits one), in their draw order.
/// Whether the far view draws an object beyond the render list at `p`: where the local
/// player's ships see now, or, for an asteroid, where they have explored. With no fog grid
/// (fog off, or a cutscene level) nothing is hidden.
fn far_seen(fog: &crate::sensors::Fog, p: Vec3, asteroid: bool) -> bool {
    let p = hwc_math::Vector::new(p.x, p.y, p.z);
    fog.grid.is_none()
        || if asteroid {
            fog.explored(p)
        } else {
            fog.seen(p)
        }
}

fn retain_fx_systems(list: &mut hwc_fx::RenderList, keep: impl Fn(Vec3) -> bool) {
    use hwc_fx::render::PrimKind;
    let order = list.draw_order();
    let old = std::mem::take(list);
    let first = |k: PrimKind, i: usize| -> Option<Vec3> {
        let p = match k {
            PrimKind::Sprites => old.sprites.get(i).map(|x| x.position),
            PrimKind::Lines => old.lines.get(i).map(|x| x.start),
            PrimKind::Bolts => old.bolts.get(i).and_then(|x| x.points.first().copied()),
            PrimKind::Points => old.points.get(i).map(|x| x.position),
            PrimKind::Cubes => old.cubes.get(i).map(|x| x.position),
            PrimKind::Meshes => old.meshes.get(i).map(|x| x.position),
            PrimKind::Flares => old.flares.get(i).map(|x| x.position),
        };
        p.map(v3)
    };
    for span in order {
        if span.count == 0 || !first(span.kind, span.start).is_none_or(&keep) {
            continue;
        }
        let r = span.start..span.start + span.count;
        let before = list.kind_len(span.kind);
        match span.kind {
            PrimKind::Sprites => list.sprites.extend_from_slice(&old.sprites[r]),
            PrimKind::Lines => list.lines.extend_from_slice(&old.lines[r]),
            PrimKind::Bolts => list.bolts.extend_from_slice(&old.bolts[r]),
            PrimKind::Points => list.points.extend_from_slice(&old.points[r]),
            PrimKind::Cubes => list.cubes.extend_from_slice(&old.cubes[r]),
            PrimKind::Meshes => list.meshes.extend_from_slice(&old.meshes[r]),
            PrimKind::Flares => list.flares.extend_from_slice(&old.flares[r]),
        }
        list.push_span(span.kind, before, span.lit, span.alpha);
    }
}

fn original_bullets(u: &Universe, out: &mut EffectBatch, in_view: impl Fn(Vec3) -> bool) {
    use hwc_sim::bullet::weapon;
    let with_effect = u.effects.as_ref().map(|fx| &fx.bullet_effect);
    for &bid in &u.bullet_list {
        if with_effect.is_some_and(|m| m.contains_key(&bid)) {
            continue;
        }
        let b = u.bullets.bullet(bid);
        if b.weapon == weapon::BEASTINFECT || (b.weapon == weapon::ION && b.time_lived <= TICK) {
            continue;
        }
        let p = v3(b.posinfo.position);
        if !in_view(p) {
            continue;
        }
        let c = b.color;
        let color = [
            (c & 0xff) as f32 / 255.0,
            ((c >> 8) & 0xff) as f32 / 255.0,
            ((c >> 16) & 0xff) as f32 / 255.0,
            1.0,
        ];
        out.lines.push(FxLine {
            from: p,
            to: p + v3(b.length_vec),
            from_color: color,
            to_color: color,
            width: 1.0,
            blend: Blend::Alpha,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_list_reach_is_radial_and_follows_tweak_script() {
        let l = RenderLimits::default();
        let fighter = Some(ShipClass::Fighter);
        let t = ShipType::sMothership;
        let drawn = |class, d_look: f32, d_near: f32| {
            l.ship_drawn(class, t, 0.0, 0.0, d_look * d_look, d_near * d_near)
        };
        // Listed within 16000 of the look-at point; faded out from the nearer point at
        // sqrt((1.3 * 16000 - 1.3 * 5000)^2 + (1.3 * 5000)^2) = 15708.
        assert!(drawn(fighter, 15000.0, 15000.0));
        assert!(drawn(fighter, 15700.0, 15700.0));
        assert!(!drawn(fighter, 15720.0, 15720.0));
        assert!(
            drawn(fighter, 15900.0, 9000.0),
            "near the eye it is not faded"
        );
        assert!(!drawn(fighter, 16000.0, 100.0), "not in the render list");
        // The mothership reaches 18000; micro ships go with the fighters, components with the
        // mothership.
        assert!(drawn(Some(ShipClass::Mothership), 17500.0, 9000.0));
        assert!(!drawn(Some(ShipClass::Micro), 16500.0, 9000.0));
        assert!(drawn(Some(ShipClass::Component), 17500.0, 9000.0));
        // A ship's own renderlistLimit / renderlistFade replace the class's.
        let own = l.ship_drawn(
            fighter,
            t,
            1000.0,
            30000.0 * 30000.0,
            25000.0f32.powi(2),
            9000.0f32.powi(2),
        );
        assert!(own);
        // The script's values are read.
        let s = hwc_formats::script::ScriptFile::parse("RENDER_LIMIT_FIGHTER 9000.0\nRENDER_FADE_FIGHTER 1000.0\nRENDER_VIEWABLE_DISTANCE_SQR 12000.0\n");
        let r = RenderLimits::from_script(&s);
        assert_eq!(r.limit[6], 9000.0);
        assert_eq!(r.fade[6], 1000.0);
        assert_eq!(r.viewable, 12000.0);
        assert!(r.object_listed(11000.0f32.powi(2), false));
        assert!(!r.object_listed(13000.0f32.powi(2), false));
        assert!(r.object_listed(13000.0f32.powi(2), true));
        assert!(!r.object_listed(21000.0f32.powi(2), true));
    }

    #[test]
    fn effect_systems_are_kept_or_cut_whole() {
        use hwc_fx::render::{Point, PrimKind, Span};
        let p = |x: f32| Point {
            position: hwc_math::Vector::new(x, 0.0, 0.0),
            size: 1.0,
            color: [1.0; 4],
            blend: hwc_fx::render::Blend::Alpha,
        };
        let mut list = hwc_fx::RenderList {
            points: vec![p(0.0), p(1.0), p(5000.0), p(5001.0), p(2.0)],
            spans: vec![
                Span {
                    kind: PrimKind::Points,
                    start: 0,
                    count: 2,
                    lit: false,
                    alpha: true,
                },
                Span {
                    kind: PrimKind::Points,
                    start: 2,
                    count: 2,
                    lit: false,
                    alpha: false,
                },
                Span {
                    kind: PrimKind::Points,
                    start: 4,
                    count: 1,
                    lit: true,
                    alpha: false,
                },
            ],
            ..Default::default()
        };
        retain_fx_systems(&mut list, |v| v.x < 1000.0);
        let xs: Vec<f32> = list.points.iter().map(|q| q.position.x).collect();
        assert_eq!(xs, vec![0.0, 1.0, 2.0]);
        assert_eq!(list.spans.len(), 2);
        assert_eq!(
            (list.spans[1].start, list.spans[1].count, list.spans[1].lit),
            (2, 1, true)
        );
        assert_eq!(list.draw_order(), list.spans);
    }

    #[test]
    fn full_detail_draws_the_top_level() {
        assert_eq!(drawn_lod(true, 3), 0);
        assert_eq!(drawn_lod(false, 3), 3);
        assert_eq!(drawn_lod(false, 0), 0);
    }

    #[test]
    fn hyperspace_planes_follow_hs_get_equation() {
        let fl = 100.0;
        assert_eq!(
            hs_plane(hs_effect::POPUP_OUTOF, 1.0, fl),
            Some(Vec4::new(0.0, 0.0, 1.0, -100.0))
        );
        // Mid-slice the nose (+z) is through the window, the tail is not.
        let p = hs_plane(hs_effect::SLICING_OUTOF, 0.25, fl).unwrap();
        assert!(p.dot(Vec4::new(0.0, 0.0, 90.0, 1.0)) > 0.0);
        assert!(p.dot(Vec4::new(0.0, 0.0, 0.0, 1.0)) < 0.0);
        let p = hs_plane(hs_effect::COLLAPSE_OUTOF, 1.0, fl).unwrap();
        assert!(p.dot(Vec4::new(0.0, 0.0, -99.0, 1.0)) > 0.0);
        assert_eq!(hs_plane(hs_effect::INACTIVE, 0.0, fl), None);
    }
}
