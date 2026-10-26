//! Sandbox and mission mode: a live `hwc-sim` universe rendered with the game camera. Ships can
//! be selected (click or drag a box), ordered to move on the movement plane (`M` then click; a
//! simple version of the game's move pie plate is drawn), put into formation (`F` cycles) and
//! ordered to attack (`A` then click an enemy). Every ship has a health bar.
//!
//! Effects are the original's: the simulation runs the ETG effects where `cataclysm.exe` creates
//! them (`hwc_sim::effects`: muzzle flashes, bullet and beam effects, hits, damage smoke, deaths,
//! hyperspace windows, special abilities) and each frame they are drawn with
//! [`hwc_render::EffectRenderer`], lens flares hidden behind ships by collision-box ray tests.
//! Bullets without an effect are drawn as the original's one-pixel lines in the bullet colour.
//! Effect sounds (`playSound`) play through [`Audio`]. Without the `ETG` data (or with
//! `HWC_VIEWER_NO_EFFECTS` set) the older stand-ins are drawn: weapon-coloured lines, hit
//! sparks and explosion glows.
//!
//! Base building: mothership modules are separate ships clamped to the hull, asteroids are
//! drawn with their meshes, harvesting beams and docking traffic are shown, `B` queues a ship at
//! the mothership, `L` launches the ships inside it, `U` toggles its auto-launch, `R` starts
//! research, `G` sends the selected workers harvesting; the title shows resource units, support
//! units, the build queue and research.
//!
//! Every ship is drawn with the renderer's per-instance state: a level of detail chosen from its
//! screen size, engine trails and capital glows updated at each simulation tick, `.mad`
//! animations started by the simulation's state (dock and harvest animations, combat-in/out,
//! linking, the command module's rotation), the team-colour tint with engine glow and hit
//! flashes, and the hyperspace clip of ships arriving in a mission.
//!
//! In mission mode ([`crate::mission`]) the universe is a `MissionRun`: the level's background
//! and lighting are used, the camera starts on the player's mothership, objectives, subtitles
//! and notes about skipped NIS sequences are drawn as overlay text, and the mission's music and
//! speech play through [`Audio`].
//!
//! The simulation runs at the original's 16 Hz through the task scheduler model of
//! `hwc_math::timing` (240 Hz ticks, the universe task every 15). Rendering shows the latest
//! update as the original does; `--interpolate` blends between the last two updates instead.

use std::collections::BTreeMap;
use std::sync::Arc;

use glam::{Mat4, Vec2, Vec3, Vec4};
use hwc_formats::hsf::Hsf;
use hwc_math::timing::{TaskTicker, TIMER_RESOLUTION};
use hwc_render::assets::{load_mad_clip, mothership_modules, read_script, AssetSource};
use hwc_render::effects::{EffectBatch, Line as FxLine, Sprite, FULL_UV};
use hwc_render::lod::{screen_radius, LodSelector, LodTable};
use hwc_render::render_state::{normalized_team_color, CollisionBox, ConstructionState, ShipTint, TintParams};
use hwc_render::trails::{
    ShipTrailDef, ShipTrails, TeamTrailColors, TrailFrame, TrailGate, TrailTick, TrailTweaks,
};
use hwc_render::{
    Background, Blend, CameraTweaks, EffectTexture, EngineEffectAssets, GpuBackground, GpuShip,
    MadClip, OrbitCamera, Renderer, Scene, SceneExtras, SceneLighting, ShipAnimator, ShipDef,
    ShipInstance, ShipModel, ShipRenderState, TeamColors, TeamPaint, ANIM_STOP_AT_END,
};
use hwc_sim::harness::{group_centre, Run, Scenario};
use hwc_sim::mission::{hs_effect, MissionRun};
use hwc_sim::resource::ResourceKind;
use hwc_sim::statics::{LoadOptions, ShipType, StaticWorld};
use hwc_sim::universe::SimEvent;
use hwc_sim::{command, formation, ShipId, SimTweaks, Universe};

use crate::audio::{self, Audio};
use crate::data::GameAssets;
use crate::mission::{Cue, CueReader};
use crate::overlay::Overlay;
use crate::sim::Sim;
use crate::text::TextFont;

mod nebula_view;

const SELECT_COLOR: [f32; 4] = [0.2, 1.0, 0.3, 0.9];
const BOX_COLOR: [f32; 4] = [0.4, 0.9, 1.0, 0.8];
const PLATE_COLOR: [f32; 4] = [0.3, 0.6, 1.0, 0.55];
const DEST_COLOR: [f32; 4] = [1.0, 0.85, 0.2, 0.9];
const ATTACK_COLOR: [f32; 4] = [1.0, 0.25, 0.2, 0.9];
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
const DOCK_COLOR: [f32; 4] = [0.5, 0.7, 1.0, 0.5];
const ASTEROID_COLOR: [f32; 4] = [0.7, 0.6, 0.5, 0.7];
/// How long a quick key's result stays on screen (seconds).
const ACTION_NOTE_SECONDS: f64 = 4.0;
/// How long the key hint shows after a mission starts (seconds of real time).
const KEY_HINT_SECONDS: f64 = 30.0;
/// The quick keys of mission mode (the real managers are in `hwc-ui` / the game).
pub const MISSION_KEY_HINT: &str = "keys: click/drag select, M move, A attack, G harvest, H halt, F formation, B queue a ship, R start research, L launch, U auto-launch, C focus, J hyperspace, Space pause";
/// One universe update in seconds.
const TICK: f32 = 1.0 / 16.0;
/// How long notes and location cards stay up (game seconds).
const NOTICE_TIME: f32 = 8.0;
/// Subtitles without a known length stay this long.
const SUBTITLE_TIME: f32 = 6.0;

/// What `B` builds: the first type the mothership can build now.
const BUILD_CHOICES: [ShipType; 4] = [
    ShipType::sAcolyte,
    ShipType::sWorker,
    ShipType::sRecon,
    ShipType::sMothershipDockingBay,
];

/// A short-lived effect drawn as sprites.
#[derive(Debug, Clone, Copy)]
struct Effect {
    position: Vec3,
    radius: f32,
    start: f32,
    explosion: bool,
}

/// Named animations of a ship type's `.mad` (ship animation numbers).
#[derive(Debug, Clone, Copy, Default)]
struct AnimIds {
    combat_in: Option<usize>,
    combat_out: Option<usize>,
    link: Option<usize>,
    /// Played continuously (the command module's gravity ring).
    idle_loop: Option<usize>,
    /// Played while harvesting (the worker's blades).
    harvest_loop: Option<usize>,
}

/// A ship type ready to draw.
struct ShipArt {
    levels: Vec<ShipModel>,
    table: Option<LodTable>,
    /// Uploaded per colour scheme.
    gpu: BTreeMap<usize, GpuShip>,
    trail: ShipTrailDef,
    clip: Option<MadClip>,
    anims: AnimIds,
    /// Per level: which `.mad` path drives each skeleton object.
    bindings: Vec<Vec<Option<usize>>>,
}

impl ShipArt {
    fn any_gpu(&self) -> Option<&GpuShip> {
        self.gpu.values().next()
    }
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
    attacking: bool,
    link_state: u32,
    harvesting: bool,
}

/// What `Sandbox::frame` prepared for one drawn frame.
pub struct Frame {
    items: Vec<Item>,
    poses: Vec<Option<Vec<Option<Mat4>>>>,
    pub effects: EffectBatch,
    /// The ETG effects to draw this frame.
    pub fx_list: hwc_fx::RenderList,
}

enum ItemKind {
    Ship(ShipType, usize),
    Asteroid(u8),
}

struct Item {
    kind: ItemKind,
    transform: Mat4,
    age: f32,
    lod: usize,
    clip: Option<Vec4>,
    tint: TintParams,
    time: f32,
    /// Under external construction: build progress and mesh-space collision box.
    construction: Option<ConstructionState>,
}

/// What the left mouse button does next.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Select,
    /// Choosing a destination on the movement plane.
    Move,
    /// Choosing a ship to attack.
    Attack,
}

/// Mission presentation state.
#[derive(Default)]
struct MissionHud {
    cues: CueReader,
    /// Subtitle lines and the game time they go away.
    subtitles: Vec<(String, f32)>,
    notices: Vec<(String, f32)>,
    location: Option<(String, f32)>,
    /// The camera has not been put on the mothership yet.
    camera_pending: bool,
    music: Option<i32>,
}

pub struct Sandbox {
    src: GameAssets,
    pub sim: Sim,
    previous: BTreeMap<ShipId, (Vec3, Mat4)>,
    art: BTreeMap<ShipType, ShipArt>,
    team_colors: TeamColors,
    team_trails: TeamTrailColors,
    trail_tweaks: TrailTweaks,
    engine_fx: Option<EngineEffectAssets>,
    pub lod: LodSelector,
    fx: BTreeMap<ShipId, ShipFx>,
    pub lighting: SceneLighting,
    background_path: Option<String>,
    background: Option<GpuBackground>,
    pub camera: OrbitCamera,
    pub selection: Vec<ShipId>,
    pub mode: Mode,
    pub interpolate: bool,
    pub paused: bool,
    ticker: TaskTicker,
    tick_carry: f64,
    /// Last move destination, shown until the ships arrive.
    pub destination: Option<Vec3>,
    pub formation_index: usize,
    pub nav_lights: bool,
    pub status: String,
    /// What the last quick key did (or why it was refused), on screen until the time given in
    /// [`Self::ui_clock`] seconds.
    action_note: Option<(String, f64)>,
    /// Real time the window has been running (seconds), for on-screen notes.
    ui_clock: f64,
    /// The camera keeps looking at these ships (the game's camera focus).
    pub focus: Vec<ShipId>,
    effects: Vec<Effect>,
    /// The simulation runs the original's effects (see the module notes).
    effects_on: bool,
    fx_renderer: Option<hwc_render::EffectRenderer>,
    neb_renderer: Option<hwc_render::NebulaRenderer>,
    resource_art: BTreeMap<u8, Option<ResourceArt>>,
    pub audio: Option<Audio>,
    /// Scripted test orders playing the mission (`--autoplay`).
    pub autoplay: Option<hwc_sim::autoplay::Autoplay>,
    /// Where to write the carry-over once the fleet has jumped (`--save-carry`), and whether
    /// it was written.
    pub save_carry: Option<String>,
    carry_saved: bool,
    hud: MissionHud,
    font: Option<TextFont>,
    small_font: Option<TextFont>,
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

impl Sandbox {
    /// A built-in harness scenario.
    pub fn new(src: GameAssets, scenario: Scenario, interpolate: bool) -> Result<Self, String> {
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = SimTweaks::load(&src);
        let run = Run::new(statics, tweaks, scenario).map_err(|e| e.to_string())?;
        let mut sb = Self::with_sim(src, Sim::Scenario(Box::new(run)), interpolate)?;
        sb.camera.look_at = sb.fleet_centre();
        Ok(sb)
    }

    /// A single-player mission.
    pub fn mission(src: GameAssets, run: MissionRun, interpolate: bool) -> Result<Self, String> {
        let (bg, light) = crate::mission::level_presentation(&run.game.level);
        let start = run
            .game
            .level
            .point(hwc_kas::kas_hash("StartPointPlayer"))
            .map_or(Vec3::ZERO, |p| Vec3::new(p[0], p[1], p[2]));
        let mut sb = Self::with_sim(src, Sim::Mission(Box::new(run)), interpolate)?;
        sb.background_path = bg.map(|b| crate::viewer::normalize_asset(&b, "btg", "btg"));
        if let Some(l) = light {
            sb.set_lighting(&crate::viewer::normalize_asset(&l, "hsf", "hsf"));
        }
        sb.camera.look_at = start;
        sb.camera.distance = 7000.0;
        sb.hud.camera_pending = true;
        sb.poll_mission();
        Ok(sb)
    }

    fn with_sim(src: GameAssets, sim: Sim, interpolate: bool) -> Result<Self, String> {
        let ctweaks = read_script(&src, "camera.script")
            .map(|s| CameraTweaks::from_script(&s))
            .unwrap_or_default();
        let team_colors = read_script(&src, "teamcolors.script")
            .map(|s| TeamColors::from_script(&s))
            .unwrap_or_default();
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
        let font = TextFont::load(&src, "arial_13.hff");
        let small_font = TextFont::load(&src, "arial_12.hff");
        let mut sb = Self {
            src,
            sim,
            previous: BTreeMap::new(),
            art: BTreeMap::new(),
            team_colors,
            team_trails,
            trail_tweaks,
            engine_fx: None,
            lod: LodSelector::default(),
            fx: BTreeMap::new(),
            lighting: SceneLighting::default(),
            background_path: Some("btg\\mission01.btg".into()),
            background: None,
            camera,
            selection: Vec::new(),
            mode: Mode::Select,
            interpolate,
            paused: false,
            ticker: TaskTicker::new(15),
            tick_carry: 0.0,
            destination: None,
            formation_index: 0,
            nav_lights: true,
            status: String::new(),
            action_note: None,
            ui_clock: 0.0,
            focus: Vec::new(),
            effects: Vec::new(),
            effects_on: false,
            fx_renderer: None,
            neb_renderer: None,
            resource_art: BTreeMap::new(),
            audio: None,
            autoplay: None,
            save_carry: None,
            carry_saved: false,
            hud: MissionHud::default(),
            font,
            small_font,
        };
        sb.set_lighting("hsf\\default.hsf");
        sb.attach_effects();
        sb.load_art()?;
        sb.snapshot();
        Ok(sb)
    }

    /// Compiles the ETG effects and attaches them to the universe (before its first update).
    fn attach_effects(&mut self) {
        if std::env::var_os("HWC_VIEWER_NO_EFFECTS").is_some() || self.sim.universe().effects.is_some() {
            self.effects_on = self.sim.universe().effects.is_some();
            return;
        }
        let data = hwc_sim::effects::EffectData::load_from(&self.src);
        if data.is_empty() {
            eprintln!("warning: no ETG effects found; drawing stand-in effects");
            return;
        }
        hwc_sim::effects::attach(self.sim.universe_mut(), &data);
        self.effects_on = true;
    }

    pub fn universe(&self) -> &Universe {
        self.sim.universe()
    }

    fn set_lighting(&mut self, path: &str) {
        match AssetSource::read(&self.src, path).and_then(|d| Hsf::parse(&d).ok()) {
            Some(h) => self.lighting = SceneLighting::from_hsf(&h),
            None => eprintln!("warning: {path}: no lighting"),
        }
    }

    pub fn set_background(&mut self, path: Option<String>) {
        self.background_path = path;
        self.background = None;
    }

    /// A mission subtitle is on screen.
    pub fn has_subtitle(&self) -> bool {
        !self.hud.subtitles.is_empty()
    }

    pub fn is_mission(&self) -> bool {
        self.sim.mission().is_some()
    }

    fn load_art(&mut self) -> Result<(), String> {
        let types: Vec<ShipType> = self.universe().ships.iter().map(|s| s.ship_type).collect();
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
            let anims = clip.as_ref().map_or(AnimIds::default(), |c| AnimIds {
                combat_in: c.find("CombatIn"),
                combat_out: c.find("CombatOut"),
                link: c.find("Link"),
                idle_loop: c.find("GravRotate"),
                harvest_loop: c.find("BladesSpin"),
            });
            self.art.insert(
                t,
                ShipArt {
                    levels,
                    table,
                    gpu: BTreeMap::new(),
                    trail,
                    clip,
                    anims,
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

    fn resource_kinds_loaded(&self) -> bool {
        self.universe().base.resources.list.iter().all(|r| match r.kind {
            ResourceKind::Asteroid(k) => self.resource_art.contains_key(&k),
            ResourceKind::DustCloud(_) | ResourceKind::Nebula(_) => true,
        })
    }

    /// Uploads models, engine effects and the background (call before drawing).
    pub fn prepare(&mut self, renderer: &mut Renderer) {
        if self.engine_fx.is_none() {
            self.engine_fx = Some(renderer.load_engine_effects(&self.src));
        }
        let needed: Vec<(ShipType, usize)> = self
            .universe()
            .ships
            .iter()
            .map(|s| (s.ship_type, s.color_scheme))
            .collect();
        for (t, scheme) in needed {
            let Some(art) = self.art.get_mut(&t) else { continue };
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
        for art in self.resource_art.values_mut().flatten() {
            if art.gpu.is_none() {
                art.gpu = Some(renderer.upload_ship(&art.model, TeamPaint::None));
            }
        }
        if self.background.is_none() {
            if let Some(p) = &self.background_path {
                match Background::load(&self.src, p) {
                    Ok(bg) => self.background = Some(renderer.upload_background(&bg)),
                    Err(e) => {
                        eprintln!("warning: {e}");
                        self.background_path = None;
                    }
                }
            }
        }
    }

    pub fn fleet_centre(&self) -> Vec3 {
        let ids: Vec<ShipId> = self.universe().ship_list.clone();
        v3(group_centre(self.universe(), &ids))
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
    /// follows it at the simulation rate: effects, sounds, mission cues, animations and trails.
    pub fn step(&mut self) {
        self.snapshot();
        nebula_view::set_camera(self.sim.universe_mut(), &self.camera);
        if let (Some(ap), Some(run)) = (self.autoplay.as_mut(), self.sim.mission_mut()) {
            ap.tick(run);
        }
        self.sim.step();
        self.write_carry_when_done();
        // Ships built or arriving since the last update need their art.
        if let Err(e) = self.load_art() {
            eprintln!("warning: {e}");
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
        if let Some(a) = &mut self.audio {
            a.sim_events(self.sim.universe(), !self.effects_on);
            a.effect_events(self.sim.universe());
        }
        self.poll_mission();
        self.update_ship_fx();
        let u = self.sim.universe();
        self.selection.retain(|id| u.ship(*id).is_some());
        self.focus.retain(|id| u.ship(*id).is_some());
        self.update_focus();
        let u = self.sim.universe();
        let busy = self.selection.iter().any(|id| {
            u.ship(*id)
                .and_then(|s| s.command)
                .and_then(|c| u.commands.get(c))
                .is_some_and(|c| c.order == command::order::MOVE)
        });
        if !busy {
            self.destination = None;
        }
    }

    /// Mission cues (music, speech, cards, notes) and the camera's start on the mothership.
    fn poll_mission(&mut self) {
        let Some(run) = self.sim.mission() else { return };
        let cues = self.hud.cues.poll(run);
        let now = run.universe.total_time_elapsed;
        let verbose = std::env::var_os("HWC_VIEWER_LOG").is_some();
        for cue in cues {
            if verbose {
                eprintln!("[{now:8.3}s] {cue:?}");
            }
            match cue {
                Cue::Music(track) => {
                    self.hud.music = Some(track);
                    if let Some(a) = &mut self.audio {
                        a.music(track);
                    }
                }
                Cue::Speech { event, variable } => {
                    let spoken = self.audio.as_mut().and_then(|a| a.speech(event, variable));
                    match spoken {
                        Some(s) => {
                            let secs = (s.duration_ms.max(0) as f32 / 1000.0).max(1.0);
                            // The subtitle's end is what ends the intel event in the original.
                            if let Some(m) = self.sim.mission_mut() {
                                m.game.speech_end = Some(now + secs);
                            }
                            let text = s.subtitles.join(" ");
                            if !text.trim().is_empty() {
                                self.hud.subtitles = vec![(text, now + secs)];
                            }
                        }
                        None => {
                            self.hud.subtitles = vec![(
                                format!("[speech event {:#x}]", event & 0x7ff),
                                now + SUBTITLE_TIME,
                            )]
                        }
                    }
                }
                Cue::LocationCard { milliseconds, text } => {
                    let secs = (milliseconds.max(1000) as f32 / 1000.0).min(30.0);
                    self.hud.location = Some((text, now + secs));
                }
                Cue::Notice(n) => self.hud.notices.push((n, now + NOTICE_TIME)),
            }
        }
        self.hud.subtitles.retain(|s| s.1 > now);
        self.hud.notices.retain(|s| s.1 > now);
        if self.hud.location.as_ref().is_some_and(|l| l.1 <= now) {
            self.hud.location = None;
        }
        if self.hud.camera_pending {
            let u = self.sim.universe();
            let ms = u.base.players.first().and_then(|p| p.mothership);
            if let Some(s) = ms.and_then(|id| u.ship(id)) {
                if s.flags & hwc_sim::object::sof::HIDE == 0 {
                    let r = u
                        .statics
                        .ship(s.ship_type)
                        .map_or(800.0, |st| st.header.coll.coll_sphere_size);
                    self.camera.distance = (r * 5.0).max(3000.0);
                    self.focus = vec![s.id];
                    self.hud.camera_pending = false;
                    self.update_focus();
                }
            }
        }
    }

    /// Hyperspace effect state of each ship arriving or leaving (mission only):
    /// `(state, clipt)`.
    fn hyperspace(&self) -> BTreeMap<ShipId, (u32, f32)> {
        self.sim
            .mission()
            .map(|m| {
                m.hyperspace_windows()
                    .into_iter()
                    .map(|(id, state, clipt)| (id, (state, clipt)))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The hyperspace button (J): the jump when the mission offers it, or the roll call's
    /// "go now" while it is up.
    pub fn hyperspace_button(&mut self) {
        let Some(run) = self.sim.mission_mut() else { return };
        let note = if run.game.sp_state == hwc_sim::mission::sp_state::WAITING_ROLLCALL {
            if run.hyperspace_go_now() {
                "roll call: go now".to_string()
            } else {
                "hyperspace under way".to_string()
            }
        } else {
            match run.hyperspace_button() {
                Ok(()) => "hyperspace".to_string(),
                Err(e) => format!("hyperspace refused: {e:?}"),
            }
        };
        self.status = note;
    }

    /// Writes the carry-over file once the mission's fleet is in hyperspace.
    fn write_carry_when_done(&mut self) {
        if self.carry_saved {
            return;
        }
        let Some(path) = self.save_carry.clone() else { return };
        let Some(c) = self.sim.mission().and_then(|m| m.carry()) else { return };
        match std::fs::write(&path, c.to_text()) {
            Ok(()) => println!("mission {} complete: carry-over written to {path}", c.mission),
            Err(e) => eprintln!("{path}: {e}"),
        }
        self.carry_saved = true;
    }

    /// Animations triggered by the simulation's state, and the trail ring update, once per tick.
    fn update_ship_fx(&mut self) {
        let u = self.sim.universe();
        // Dead ships stay (as hulks) until their explosion deletes them.
        let live: Vec<ShipId> = u.ship_list.iter().chain(&u.hulks).copied().collect();
        self.fx.retain(|id, _| live.contains(id));
        for id in live {
            let Some(s) = u.ship(id) else { continue };
            let Some(art) = self.art.get(&s.ship_type) else { continue };
            let fx = self.fx.entry(id).or_insert_with(|| ShipFx {
                trails: ShipTrails::new(&art.trail, id.0 as u32 ^ 0x9e37_79b9),
                anim: ShipAnimator::default(),
                lod: 0,
                tint: ShipTint::default(),
                last_mad: -1,
                attacking: false,
                link_state: 0,
                harvesting: false,
            });
            if let Some(clip) = &art.clip {
                // Dock and harvest animations the simulation asks for (`ship+0x4bc`).
                let mad = s.base.mad_anim;
                if mad != fx.last_mad {
                    fx.last_mad = mad;
                    if mad >= 0 {
                        fx.anim.play(clip, mad as usize, 0);
                    }
                }
                // Gun ports open when the ship goes into combat and close after.
                let attacking = s.attack.target.is_some();
                if attacking != fx.attacking {
                    fx.attacking = attacking;
                    let a = if attacking {
                        art.anims.combat_in
                    } else {
                        art.anims.combat_out
                    };
                    if let Some(a) = a {
                        let flags = if attacking { 0 } else { ANIM_STOP_AT_END };
                        fx.anim.play(clip, a, flags);
                    }
                }
                if s.base.link.state != fx.link_state {
                    fx.link_state = s.base.link.state;
                    if fx.link_state == 4 {
                        if let Some(a) = art.anims.link {
                            fx.anim.play(clip, a, 0);
                        }
                    }
                }
                let harvesting = s.base.harvest_effect;
                if harvesting != fx.harvesting {
                    fx.harvesting = harvesting;
                    match (harvesting, art.anims.harvest_loop) {
                        (true, Some(a)) => {
                            fx.anim.play(clip, a, hwc_render::ANIM_LOOP);
                        }
                        (false, Some(a)) if fx.anim.current == Some(a) => fx.anim.stop(),
                        _ => {}
                    }
                }
                if fx.anim.current.is_none() {
                    if let Some(a) = art.anims.idle_loop {
                        fx.anim.play(clip, a, hwc_render::ANIM_LOOP);
                    }
                }
                fx.anim.update(clip, TICK);
                // TODO(render states): carrier doors (DoorsPortOpen/Close) on launch and dock,
                // Mimic link, Sentinel and hive-frigate animations once the simulation exposes
                // the states that start them.
            }
            let (_, transform) = Self::transform_of(s);
            // The nozzles go through the objects of the level drawn (`trailUpdate` reads the
            // ship's current level), posed or at rest.
            let objects = art.any_gpu().map(|gpu| {
                let lod = fx.lod.min(gpu.level_count().saturating_sub(1));
                let pose = fx_pose(art, fx, lod.min(art.bindings.len().saturating_sub(1)));
                let sk = gpu.skeleton(lod);
                let mats = match &pose {
                    Some(p) => sk.resolve(|j| p.get(j).copied().flatten().unwrap_or(sk.objects[j].rest)),
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

    /// Keeps the camera on the focused ships.
    pub fn update_focus(&mut self) {
        if !self.focus.is_empty() {
            let c = group_centre(self.universe(), &self.focus);
            self.camera.look_at = v3(c);
        }
    }

    /// Advances real time: the task scheduler hands out 240 Hz ticks and runs the universe
    /// task once per 15 (at most 8 times per frame, like the original).
    pub fn advance(&mut self, seconds: f64) {
        self.ui_clock += seconds;
        if let Some(a) = &mut self.audio {
            a.set_listener(self.camera.eye(), self.camera.look_at);
            a.nearby_ships(self.sim.universe());
            a.frame(!self.paused);
        }
        if self.paused {
            return;
        }
        self.tick_carry += seconds * f64::from(TIMER_RESOLUTION);
        let ticks = self.tick_carry.floor();
        self.tick_carry -= ticks;
        let calls = self.ticker.advance(ticks as u32);
        for _ in 0..calls {
            self.step();
        }
    }

    /// Blend factor between the previous and the current update.
    fn alpha(&self) -> f32 {
        if !self.interpolate || self.paused {
            return 1.0;
        }
        ((f64::from(self.ticker.carry) + self.tick_carry) / f64::from(self.ticker.ticks_per_call))
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
                (s.id, s.ship_type, m, age - s.time_created + 0.75)
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
        let mut frame = Frame {
            items: Vec::new(),
            poses: Vec::new(),
            effects: EffectBatch::default(),
            fx_list: hwc_fx::RenderList::default(),
        };
        let transforms = self.render_transforms();
        let sim = &self.sim;
        let u = sim.universe();
        for (id, t, transform, age) in transforms {
            let Some(s) = u.ship(id) else { continue };
            let Some(art) = self.art.get(&t) else { continue };
            let Some(gpu) = art.gpu.get(&s.color_scheme) else { continue };
            let Some(fx) = self.fx.get_mut(&id) else { continue };
            let (centre, radius) = gpu.bounds();
            let r = screen_radius(view, proj, transform.transform_point3(centre), radius);
            let d2 = (transform.w_axis.truncate() - eye).length_squared();
            fx.lod = self
                .lod
                .select(gpu.level_count(), fx.lod, r, d2, gpu.lod_table());
            let lod = fx.lod;
            let pose = fx_pose(art, fx, lod.min(art.bindings.len().saturating_sub(1)));
            let st = u.statics.ship(t);
            let scheme = self
                .team_colors
                .schemes
                .get(s.color_scheme)
                .or(self.team_colors.schemes.first());
            let team_color = scheme.map_or([1.0; 3], |c| normalized_team_color(c.base));
            // Hyperspace: the window cuts the hull along its heading (`hsGetEquation`). A ship
            // clamped to a host (the mothership's modules) has no window of its own
            // (`hsShouldDisplayEffect`) and is cut by the host's plane while the host is in
            // hyperspace; the host's window is sized to cover them (`hyperspaceBoxScale`).
            let host = clamp_host(sim, s);
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
                    Some(p) => sk.resolve(|j| p.get(j).copied().flatten().unwrap_or(sk.objects[j].rest)),
                    None => sk.rest_matrices(),
                })
            };
            let gate = TrailGate {
                moving: s.posinfo.is_moving != 0 || v3(s.posinfo.velocity).length_squared() > 1.0,
                docked: s.base.dock_state == hwc_sim::dock::state::INSIDE,
                hyperspacing: hs.is_some(),
                cloaked: false,
                suppressed: s.base.clamp.is_some() || s.flags & hwc_sim::base::SOF_MODULE != 0,
                idle_dodge: s.is_dodging && s.ship_idle,
            };
            fx.trails.draw(
                &art.trail,
                &self.trail_tweaks,
                &TrailFrame {
                    transform,
                    objects: objects.as_ref().map(|m| (gpu.skeleton(lod), m.as_slice())),
                    velocity: v3(s.posinfo.velocity),
                    max_velocity,
                    gate,
                    lod,
                    team: s.color_scheme,
                    team_color,
                    eye,
                    eye_space_z: view.transform_point3(transform.w_axis.truncate()).z,
                    scale: 1.0,
                    fade: 1.0,
                    glow_texture,
                },
                &mut frame.effects,
            );
            let tint = fx
                .tint
                .params(team_color, fx.trails.engine_glow_levels(&self.trail_tweaks));
            let construction = st
                .filter(|_| s.special_flags2 & hwc_sim::base::special2::UNDER_CONSTRUCTION != 0)
                .map(|st| {
                    let c = &st.header.coll;
                    let o = c.coll_rect_offset;
                    ConstructionState {
                        progress: s.base.build_progress,
                        collision_box: CollisionBox {
                            corner: Vec3::new(o.x, o.y, o.z),
                            extent: Vec3::new(c.up_length, c.right_length, c.forward_length),
                        },
                    }
                });
            frame.items.push(Item {
                kind: ItemKind::Ship(t, s.color_scheme),
                transform,
                age,
                lod,
                clip,
                tint,
                time: now,
                construction,
            });
            frame.poses.push(pose);
            // TODO(render states): cloak fade/wipe, phase blend, Beast infection and EMP
            // lightning once the simulation exposes cloaking, phasing, infection and the
            // Naggarok's EMP.
        }
        for r in &u.base.resources.list {
            let ResourceKind::Asteroid(k) = r.kind else { continue };
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
            });
            frame.poses.push(None);
        }
        self.combat_effects(&mut frame.effects, glow_texture);
        if self.effects_on {
            hwc_sim::effects::draw(self.sim.universe_mut(), &mut frame.fx_list);
        }
        frame
    }

    /// Bullets, beams, harvesting beams, hit sparks and explosion glows as effect primitives.
    fn combat_effects(&self, out: &mut EffectBatch, glow: EffectTexture) {
        let u = self.universe();
        if self.effects_on {
            original_bullets(u, out);
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
            states.push(ShipRenderState {
                lod: item.lod,
                pose: pose.as_deref(),
                clip_plane: item.clip,
                tint: item.tint,
                time: item.time,
                construction: item.construction,
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

    /// Draws a prepared frame (window path).
    pub fn render(
        &mut self,
        renderer: &mut Renderer,
        encoder: &mut hwc_render::wgpu::CommandEncoder,
        color: &hwc_render::wgpu::TextureView,
        depth: &hwc_render::wgpu::TextureView,
        size: (u32, u32),
        frame: &Frame,
    ) {
        {
            let (ships, states) = self.instances(frame);
            let scene = self.scene(&ships);
            let extras = SceneExtras {
                ship_states: &states,
                effects: Some(&frame.effects),
            };
            renderer.render_ex(encoder, color, depth, size, &scene, &extras);
        }
        if self.prepare_effects(renderer, size, frame) {
            if let Some(fx) = self.fx_renderer.as_mut() {
                fx.draw(renderer, encoder, color, depth);
            }
        }
        if nebula_view::prepare(&mut self.neb_renderer, renderer, self.sim.universe(), &self.camera, size) {
            if let Some(n) = self.neb_renderer.as_mut() {
                n.draw(renderer, encoder, color, depth);
            }
        }
    }

    /// Renders a prepared frame offscreen.
    pub fn render_offscreen(
        &mut self,
        renderer: &mut Renderer,
        target: &hwc_render::OffscreenTarget,
        frame: &Frame,
    ) -> Result<Vec<u8>, String> {
        let effects = self.prepare_effects(renderer, (target.width, target.height), frame);
        let nebulae = nebula_view::prepare(&mut self.neb_renderer, renderer, self.sim.universe(), &self.camera, (target.width, target.height));
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
                if let Some(n) = neb.as_mut().filter(|_| nebulae) {
                    n.draw(r, enc, c, d);
                }
            })
        };
        self.fx_renderer = fx;
        self.neb_renderer = neb;
        out
    }

    /// Builds the effect geometry of a frame for this camera, with the lens flares (each new
    /// flare hidden when a ship's collision box lies between it and the eye, the original's
    /// `collCheckRectLine` test). False when there is nothing to draw.
    fn prepare_effects(&mut self, renderer: &Renderer, size: (u32, u32), frame: &Frame) -> bool {
        if !self.effects_on {
            return false;
        }
        let fx = self.fx_renderer.get_or_insert_with(|| {
            let mut fx = hwc_render::EffectRenderer::new(renderer);
            if !fx.load_lens_flares(renderer, &self.src, LENS_FLARE_SET) {
                eprintln!("warning: lens flare set {LENS_FLARE_SET} not found");
            }
            fx
        });
        let view = hwc_render::FxView::from_orbit(&self.camera, size.0, size.1);
        fx.prepare(renderer, &self.src, &frame.fx_list, &view, &self.lighting);
        let u = self.sim.universe_mut();
        let Some(world) = u.effects.as_mut() else { return true };
        let mut flares = std::mem::take(&mut world.world.lens_flares);
        let u = self.sim.universe();
        let occluded = |pos: Vec3, dir: Vec3, dist: f32| flare_occluded(u, pos, dir, dist);
        fx.prepare_lens_flares(&mut flares, &view, Some(&occluded));
        if let Some(world) = self.sim.universe_mut().effects.as_mut() {
            world.world.lens_flares = flares;
        }
        true
    }

    fn view_proj(&self, size: (u32, u32)) -> Mat4 {
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

    /// Ray through a pixel: (origin, direction).
    fn ray(&self, size: (u32, u32), px: [f32; 2]) -> (Vec3, Vec3) {
        let inv = self.view_proj(size).inverse();
        let nx = px[0] / size.0 as f32 * 2.0 - 1.0;
        let ny = 1.0 - px[1] / size.1 as f32 * 2.0;
        let a = inv * Vec4::new(nx, ny, 0.0, 1.0);
        let b = inv * Vec4::new(nx, ny, 1.0, 1.0);
        let (a, b) = (a.truncate() / a.w, b.truncate() / b.w);
        (a, (b - a).normalize())
    }

    /// The movement plane: horizontal through the selection's centre.
    fn plane_point(&self, size: (u32, u32), px: [f32; 2]) -> Option<Vec3> {
        if self.selection.is_empty() {
            return None;
        }
        let c = v3(group_centre(self.universe(), &self.selection));
        let (o, d) = self.ray(size, px);
        if d.z.abs() < 1e-5 {
            return None;
        }
        let t = (c.z - o.z) / d.z;
        (t > 0.0).then(|| o + d * t)
    }

    /// Screen box of a ship (its collision sphere's projection).
    fn screen_box(&self, size: (u32, u32), s: &hwc_sim::Ship) -> Option<([f32; 2], [f32; 2])> {
        let r = self
            .universe()
            .statics
            .ship(s.ship_type)
            .map_or(50.0, |st| st.header.coll.coll_sphere_size)
            .max(20.0);
        let c = v3(s.coll.coll_position);
        let p = self.project(size, c)?;
        let right = self.camera.view().row(0).truncate();
        let q = self.project(size, c + right * r)?;
        let rad = ((q[0] - p[0]).powi(2) + (q[1] - p[1]).powi(2))
            .sqrt()
            .max(6.0);
        Some(([p[0] - rad, p[1] - rad], [p[0] + rad, p[1] + rad]))
    }

    /// The nearest visible ship whose screen box contains the point.
    fn ship_at(&self, size: (u32, u32), px: [f32; 2]) -> Option<ShipId> {
        let mut best: Option<(f32, ShipId)> = None;
        for s in self
            .universe()
            .ships
            .iter()
            .filter(|s| s.flags & hwc_sim::object::sof::HIDE == 0)
        {
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

    /// The local player's ships only can be selected and given orders.
    fn selectable(&self, id: ShipId) -> bool {
        self.universe().ship(id).is_some_and(|s| {
            s.player == 0
                && s.flags & hwc_sim::object::sof::HIDE == 0
                && s.flags & hwc_sim::base::SOF_MODULE == 0
        }) || !self.is_mission()
    }

    fn ui_sound(&mut self, ev: u32) {
        if let Some(a) = &mut self.audio {
            a.ui(ev);
        }
    }

    /// Click selection: the nearest ship whose screen box contains the point.
    pub fn click_select(&mut self, size: (u32, u32), px: [f32; 2], add: bool) {
        let hit = self.ship_at(size, px).filter(|&id| self.selectable(id));
        if !add {
            self.selection.clear();
        }
        if let Some(id) = hit {
            if !self.selection.contains(&id) {
                self.selection.push(id);
            }
            self.ui_sound(audio::ui::CLICK);
        }
    }

    /// Box selection (the original selects the ships whose centres fall inside the band box).
    pub fn box_select(&mut self, size: (u32, u32), a: [f32; 2], b: [f32; 2], add: bool) {
        let (lo, hi) = (
            [a[0].min(b[0]), a[1].min(b[1])],
            [a[0].max(b[0]), a[1].max(b[1])],
        );
        if !add {
            self.selection.clear();
        }
        let ids: Vec<ShipId> = self.universe().ship_list.clone();
        for id in ids {
            if !self.selectable(id) {
                continue;
            }
            let s = self.universe().ship(id).expect("ship");
            if s.flags & hwc_sim::object::sof::HIDE != 0 {
                continue;
            }
            if let Some(p) = self.project(size, v3(s.posinfo.position)) {
                if p[0] >= lo[0]
                    && p[0] <= hi[0]
                    && p[1] >= lo[1]
                    && p[1] <= hi[1]
                    && !self.selection.contains(&id)
                {
                    self.selection.push(id);
                }
            }
        }
        if !self.selection.is_empty() {
            self.ui_sound(audio::ui::CLICK);
        }
    }

    /// `M`: toggle the move plate.
    pub fn toggle_move(&mut self) {
        self.mode = if self.mode == Mode::Move || self.selection.is_empty() {
            if self.mode == Mode::Move {
                self.ui_sound(audio::ui::MOVEMENT_GUI_OFF);
            }
            Mode::Select
        } else {
            self.ui_sound(audio::ui::MOVEMENT_GUI_ON);
            Mode::Move
        };
    }

    /// Moves the selection to the plane point under the cursor.
    pub fn move_to_cursor(&mut self, size: (u32, u32), px: [f32; 2]) -> bool {
        let Some(p) = self.plane_point(size, px) else {
            return false;
        };
        let from = group_centre(self.universe(), &self.selection);
        let dest = hwc_math::Vector::new(p.x, p.y, p.z);
        let sel = self.selection.clone();
        command::cl_move(
            self.sim.universe_mut(),
            &sel,
            command::waypoint_mode::ONCE,
            from,
            &[dest],
        );
        self.destination = Some(p);
        self.status = format!(
            "move {} ships to ({:.0}, {:.0}, {:.0})",
            sel.len(),
            p.x,
            p.y,
            p.z
        );
        self.ui_sound(audio::ui::MOVEMENT_GUI_ACCEPT);
        true
    }

    /// `F`: next formation for the selection.
    pub fn cycle_formation(&mut self) {
        if self.selection.len() < 2 {
            self.status = "formation needs at least two ships".into();
            return;
        }
        let t = formation::CYCLE[self.formation_index % formation::CYCLE.len()];
        self.formation_index += 1;
        let sel = self.selection.clone();
        command::cl_formation(self.sim.universe_mut(), &sel, t);
        self.status = format!("formation: {}", formation::name(t));
        self.ui_sound(audio::ui::CLICK);
    }

    /// `A` then click: the selection attacks the clicked ship.
    pub fn attack_clicked(&mut self, size: (u32, u32), px: [f32; 2]) -> bool {
        let Some(target) = self.ship_at(size, px) else {
            return false;
        };
        let sel: Vec<ShipId> = self
            .selection
            .iter()
            .copied()
            .filter(|&id| id != target)
            .collect();
        command::cl_attack(self.sim.universe_mut(), &sel, &[target]);
        let t = self.universe().ship(target).map(|s| s.ship_type);
        self.status = format!("{} ships attack {:?}", sel.len(), t);
        self.ui_sound(audio::ui::CLICK);
        true
    }

    /// The local player's mothership.
    fn mothership(&self) -> Option<ShipId> {
        self.universe().base.players.first().and_then(|p| p.mothership)
    }

    /// `B`: queue one ship at the mothership (the first of [`BUILD_CHOICES`] it can build). The
    /// build manager itself lives in `hwc-ui` (the game); this only says what happened.
    pub fn queue_build(&mut self) {
        let Some(ms) = self.mothership() else {
            self.note("Build refused: no mothership".into());
            return;
        };
        let factory = self.universe().ship(ms).map_or_else(|| "?".into(), |s| s.ship_type.to_string());
        let mission = self.sim.mission().is_some();
        for t in BUILD_CHOICES {
            // A mission's construction manager only offers what the level and KAS allow.
            if mission && !hwc_sim::level::construction_offers(self.universe(), ms, t) {
                continue;
            }
            if hwc_sim::construction::queue_build(self.sim.universe_mut(), ms, t, 1) > 0 {
                self.note(format!("Queued: {t} at {factory}"));
                self.ui_sound(audio::ui::CLICK);
                return;
            }
        }
        let why = build_refusal(self.universe(), ms, BUILD_CHOICES[0]);
        self.note(format!("Build refused at {factory}: {} {why}", BUILD_CHOICES[0]));
    }

    /// `L`: launch everything inside the mothership.
    pub fn launch(&mut self) {
        let Some(ms) = self.mothership() else { return };
        let inside = self
            .universe()
            .ship(ms)
            .and_then(|s| s.base.inside.as_ref())
            .map(|i| i.ships.clone())
            .unwrap_or_default();
        hwc_sim::dock::cl_launch_multiple_ships(self.sim.universe_mut(), &inside, ms);
        self.status = format!("launching {}", inside.len());
    }

    /// `U`: toggle the mothership's auto-launch.
    pub fn toggle_auto_launch(&mut self) {
        let bits = self
            .universe()
            .base
            .players
            .first()
            .map_or(0, |p| p.auto_launch)
            ^ 1;
        hwc_sim::dock::cl_auto_launch(self.sim.universe_mut(), bits, 0);
        self.status = format!("auto-launch {}", if bits & 1 != 0 { "on" } else { "off" });
    }

    /// `R`: research the first technology a lab is free for (the research manager lives in
    /// `hwc-ui`); says what started or why nothing did.
    pub fn research(&mut self) {
        for &t in hwc_sim::statics::Technology::ALL {
            if hwc_sim::research::research_tech_if_lab_free(self.sim.universe_mut(), 0, t.index())
            {
                self.note(format!("Researching: {t}"));
                self.ui_sound(audio::ui::CLICK);
                return;
            }
        }
        let u = self.universe();
        let why = match u.base.players.first() {
            None => "no player",
            Some(p) if !p.can_do_research => "research is not available (no research module, or the mission does not allow it yet)",
            Some(p) if hwc_sim::statics::Technology::ALL.iter().all(|t| p.tech.has.get(t.index()).copied() == Some(1)) => {
                "everything is researched"
            }
            Some(_) => "no free lab for any missing technology",
        };
        self.note(format!("Research refused: {why}"));
    }

    /// Shows `text` on screen for a few seconds (and in the title).
    fn note(&mut self, text: String) {
        self.action_note = Some((text.clone(), self.ui_clock + ACTION_NOTE_SECONDS));
        self.status = text;
    }

    /// `G`: the selected ships harvest.
    pub fn gather(&mut self) {
        let sel = self.selection.clone();
        hwc_sim::resource::cl_collect_resource(self.sim.universe_mut(), &sel, None);
        self.status = "harvesting".into();
    }

    pub fn halt(&mut self) {
        let sel = self.selection.clone();
        command::cl_halt(self.sim.universe_mut(), &sel);
        self.destination = None;
        self.status = "halt".into();
    }

    /// Selection markers, the drag box, the move plate, health bars and the mission text.
    pub fn overlay(
        &self,
        size: (u32, u32),
        cursor: Option<[f32; 2]>,
        drag: Option<([f32; 2], [f32; 2])>,
    ) -> Overlay {
        let mut o = Overlay::default();
        let transforms = self.render_transforms();
        self.combat_overlay(&mut o, size);
        for id in &self.selection {
            let Some(s) = self.universe().ship(*id) else {
                continue;
            };
            let mut s = s.clone();
            if let Some((_, _, m, _)) = transforms.iter().find(|t| t.0 == *id) {
                let off = s.coll.coll_offset;
                let p = m.col(3).truncate();
                s.coll.coll_position =
                    hwc_math::Vector::new(p.x + off.x, p.y + off.y, p.z + off.z);
            }
            if let Some((lo, hi)) = self.screen_box(size, &s) {
                o.brackets(lo, hi, SELECT_COLOR);
            }
        }
        if let Some((a, b)) = drag {
            o.rect(
                [a[0].min(b[0]), a[1].min(b[1])],
                [a[0].max(b[0]), a[1].max(b[1])],
                BOX_COLOR,
            );
        }
        if self.mode == Mode::Move && !self.selection.is_empty() {
            let c = v3(group_centre(self.universe(), &self.selection));
            let target = cursor.and_then(|px| self.plane_point(size, px));
            let radius = target.map_or(2000.0, |t| (t - c).truncate().length().max(200.0));
            self.plate(&mut o, size, c, radius);
            if let Some(t) = target {
                if let (Some(a), Some(b)) = (self.project(size, c), self.project(size, t)) {
                    o.line(a, b, DEST_COLOR);
                    o.rect(
                        [b[0] - 4.0, b[1] - 4.0],
                        [b[0] + 4.0, b[1] + 4.0],
                        DEST_COLOR,
                    );
                }
            }
        }
        if let Some(d) = self.destination {
            if let Some(p) = self.project(size, d) {
                o.line([p[0] - 8.0, p[1]], [p[0] + 8.0, p[1]], DEST_COLOR);
                o.line([p[0], p[1] - 8.0], [p[0], p[1] + 8.0], DEST_COLOR);
            }
        }
        self.text_overlay(&mut o, size);
        o
    }

    /// Objectives, subtitles, location card, notes and a status line.
    fn text_overlay(&self, o: &mut Overlay, size: (u32, u32)) {
        let Some(font) = &self.font else { return };
        let small = self.small_font.as_ref().unwrap_or(font);
        let (w, h) = (size.0 as f32, size.1 as f32);
        let lh = font.line_height() + 2.0;
        if let Some(m) = self.sim.mission() {
            // Objectives, top left.
            let mut y = 12.0;
            if !m.game.objectives.is_empty() {
                font.draw(o, 12.0, y, "OBJECTIVES", [1.0, 0.85, 0.3, 1.0]);
                y += lh;
                for ob in &m.game.objectives {
                    let (mark, color) = if ob.complete {
                        ("[x] ", [0.5, 0.9, 0.5, 1.0])
                    } else {
                        ("[ ] ", [0.9, 0.9, 0.9, 1.0])
                    };
                    font.draw(o, 12.0, y, &format!("{mark}{}", ob.brief), color);
                    y += lh;
                }
            }
            // Notes, top centre.
            let mut y = 12.0;
            for (n, _) in &self.hud.notices {
                let tw = font.width(n);
                font.draw(o, (w - tw) * 0.5, y, n, [0.6, 0.85, 1.0, 1.0]);
                y += lh;
            }
            if let Some((card, _)) = &self.hud.location {
                let tw = font.width(card);
                font.draw(o, (w - tw) * 0.5, h * 0.3, card, [1.0, 1.0, 1.0, 1.0]);
            }
            // Subtitles, bottom centre on a dark band.
            let lines: Vec<String> = self
                .hud
                .subtitles
                .iter()
                .flat_map(|(s, _)| font.wrap(s, w * 0.7))
                .collect();
            if !lines.is_empty() {
                let top = h - 60.0 - lines.len() as f32 * lh;
                let widest = lines.iter().map(|l| font.width(l)).fold(0.0, f32::max);
                o.fill(
                    [(w - widest) * 0.5 - 10.0, top - 6.0],
                    [(w + widest) * 0.5 + 10.0, top + lines.len() as f32 * lh + 4.0],
                    [0.0, 0.0, 0.0, 0.55],
                );
                for (i, l) in lines.iter().enumerate() {
                    let tw = font.width(l);
                    font.draw(o, (w - tw) * 0.5, top + i as f32 * lh, l, [0.75, 0.95, 1.0, 1.0]);
                }
            }
        }
        // The last quick key's result, above the status line.
        let status_y = h - small.line_height() - 8.0;
        if let Some((note, until)) = &self.action_note {
            if self.ui_clock < *until {
                let tw = font.width(note);
                let y = status_y - 2.0 * lh - 30.0;
                o.fill([(w - tw) * 0.5 - 8.0, y - 4.0], [(w + tw) * 0.5 + 8.0, y + lh], [0.0, 0.0, 0.0, 0.55]);
                font.draw(o, (w - tw) * 0.5, y, note, [1.0, 0.9, 0.5, 1.0]);
            }
        }
        if self.sim.mission().is_some() && self.ui_clock < KEY_HINT_SECONDS {
            small.draw(o, 10.0, status_y - small.line_height() - 4.0, MISSION_KEY_HINT, [0.7, 0.8, 0.9, 0.9]);
        }
        // Status, bottom left.
        let u = self.universe();
        let mut status = format!(
            "{:.1} s  ships {}  selected {}  {}",
            u.total_time_elapsed,
            u.ship_list.len(),
            self.selection.len(),
            self.base_status()
        );
        if let Some(m) = self.sim.mission() {
            if m.universe.universe_paused {
                status.push_str(" PAUSED BY MISSION");
            }
            if let Some(t) = self.hud.music {
                status.push_str(&format!("  music {t}"));
            }
        }
        if self.paused {
            status.push_str("  PAUSED");
        }
        small.draw(o, 10.0, h - small.line_height() - 8.0, &status, [0.8, 0.8, 0.8, 0.9]);
    }

    /// Docking lines and asteroids without a mesh.
    fn base_overlay(&self, o: &mut Overlay, size: (u32, u32)) {
        let u = self.universe();
        for s in u
            .ships
            .iter()
            .filter(|s| s.flags & hwc_sim::object::sof::HIDE == 0)
        {
            if let Some(d) = s.base.dock_with.and_then(|d| u.ship(d)) {
                if s.base.dock_state < hwc_sim::dock::state::FLY_OUT {
                    if let (Some(a), Some(b)) = (
                        self.project(size, v3(s.posinfo.position)),
                        self.project(size, v3(d.posinfo.position)),
                    ) {
                        o.line(a, b, DOCK_COLOR);
                    }
                }
            }
        }
        for r in &u.base.resources.list {
            let has_mesh = matches!(r.kind, ResourceKind::Asteroid(k) if self.resource_art.get(&k).is_some_and(|a| a.is_some()));
            if has_mesh {
                continue;
            }
            if let Some(c) = self.project(size, v3(r.position)) {
                let pts: Vec<[f32; 2]> = (0..12)
                    .map(|i| {
                        let a = i as f32 / 12.0 * std::f32::consts::TAU;
                        [c[0] + a.cos() * 6.0, c[1] + a.sin() * 6.0]
                    })
                    .collect();
                o.polygon(&pts, ASTEROID_COLOR);
            }
        }
    }

    /// Health bars and attack lines of the selection.
    fn combat_overlay(&self, o: &mut Overlay, size: (u32, u32)) {
        self.base_overlay(o, size);
        let u = self.universe();
        // The game draws health bars for the selection; the scenario sandbox shows all of them.
        let all = !self.is_mission();
        for s in u.ships.iter().filter(|s| {
            s.flags & (hwc_sim::object::sof::HIDE | hwc_sim::object::sof::DEAD) == 0
                && s.flags & hwc_sim::base::SOF_MODULE == 0
                && (all || self.selection.contains(&s.id))
        }) {
            let Some(max) = u.statics.ship(s.ship_type).map(|st| st.health.max_health) else {
                continue;
            };
            let Some((lo, hi)) = self.screen_box(size, s) else {
                continue;
            };
            let frac = (s.health / max).clamp(0.0, 1.0);
            let w = (hi[0] - lo[0]).max(16.0);
            let (x0, y) = (lo[0], lo[1] - 6.0);
            let col = if frac > 0.6 {
                [0.2, 1.0, 0.2, 0.9]
            } else if frac > 0.3 {
                [1.0, 0.9, 0.2, 0.9]
            } else {
                [1.0, 0.2, 0.2, 0.9]
            };
            o.line([x0, y], [x0 + w, y], [0.2, 0.2, 0.2, 0.8]);
            o.line([x0, y], [x0 + w * frac, y], col);
            o.line([x0, y + 1.0], [x0 + w * frac, y + 1.0], col);
            if let Some(t) = s.attack.target.and_then(|t| u.ship(t)) {
                if self.selection.contains(&s.id) {
                    if let (Some(a), Some(b)) = (
                        self.project(size, v3(s.posinfo.position)),
                        self.project(size, v3(t.posinfo.position)),
                    ) {
                        o.line(a, b, [ATTACK_COLOR[0], ATTACK_COLOR[1], ATTACK_COLOR[2], 0.35]);
                    }
                }
            }
        }
    }

    /// The move pie plate: concentric rings and spokes on the movement plane.
    fn plate(&self, o: &mut Overlay, size: (u32, u32), c: Vec3, radius: f32) {
        for k in 1..=3 {
            let r = radius * k as f32 / 3.0;
            let pts: Vec<[f32; 2]> = (0..48)
                .filter_map(|i| {
                    let a = i as f32 / 48.0 * std::f32::consts::TAU;
                    self.project(size, c + Vec3::new(a.cos() * r, a.sin() * r, 0.0))
                })
                .collect();
            if pts.len() == 48 {
                o.polygon(&pts, PLATE_COLOR);
            }
        }
        for i in 0..8 {
            let a = i as f32 / 8.0 * std::f32::consts::TAU;
            if let (Some(p), Some(q)) = (
                self.project(size, c),
                self.project(size, c + Vec3::new(a.cos() * radius, a.sin() * radius, 0.0)),
            ) {
                o.line(p, q, PLATE_COLOR);
            }
        }
    }

    /// Resource units, support units, the build queue and research of player 0.
    pub fn base_status(&self) -> String {
        let u = self.universe();
        let Some(p) = u.base.players.first() else {
            return String::new();
        };
        // The player's live RUs (the mission's `game.resource_units` only mirrors what the
        // script set with `RUsSet` and goes stale as soon as anything is built or harvested).
        let rus = p.resource_units;
        let mut queue = Vec::new();
        for f in u
            .base
            .factories
            .iter()
            .filter(|f| f.ship.and_then(|s| u.ship(s)).is_some_and(|s| s.player == 0))
        {
            for j in f.jobs.iter().filter(|j| j.n_jobs > 0) {
                if let Some(t) = j.info {
                    let done = if j.time_total > 0 {
                        100 - 100 * j.time_left / j.time_total
                    } else {
                        0
                    };
                    queue.push(format!("{t} x{} {done}%", j.n_jobs));
                }
            }
        }
        let research: Vec<String> = p
            .topics
            .iter()
            .map(|t| {
                format!(
                    "{} {:.0}s",
                    hwc_sim::statics::Technology::from_index(t.tech).map_or("?", |x| x.name()),
                    t.time_left
                )
            })
            .collect();
        format!(
            "RU {} SU {}/{} build [{}] research [{}] ",
            rus,
            p.su_used,
            p.su_capacity,
            queue.join(", "),
            research.join(", ")
        )
    }

    pub fn title(&self) -> String {
        let u = self.universe();
        let what = match self.sim.mission() {
            Some(m) => format!("Mission {:02}", m.game.config.mission),
            None => "sandbox".into(),
        };
        format!(
            "hwc-viewer {what} - tick {} ({:.1} s) - {} ships, {} selected - {}{}{}{}",
            u.update_counter,
            u.total_time_elapsed,
            u.ship_list.len(),
            self.selection.len(),
            self.base_status(),
            match self.mode {
                Mode::Move => "MOVE: click a point - ",
                Mode::Attack => "ATTACK: click a ship - ",
                Mode::Select => "",
            },
            if self.paused { "PAUSED - " } else { "" },
            self.status
        )
    }
}

/// Why `factory` cannot queue a `t` (the build manager's checks, first failing one).
/// The lens-flare set the game loads (`lensflare\\ps-50-300mm-zoom`).
const LENS_FLARE_SET: &str = "ps-50-300mm-zoom";

/// Whether a ship's collision box hides a lens flare at `pos` (looking along `dir` towards the
/// eye, `dist` away).
fn flare_occluded(u: &Universe, pos: Vec3, dir: Vec3, dist: f32) -> bool {
    let p = hwc_math::Vector::new(pos.x, pos.y, pos.z);
    let d = hwc_math::Vector::new(dir.x, dir.y, dir.z);
    u.ships.iter().filter(|s| hwc_sim::effects::ship_drawn(u, s.id)).any(|s| {
        u.statics.ship(s.ship_type).is_some_and(|st| {
            let (t, _) = hwc_sim::collision::rect_line(s, st, p, d, dist);
            t > 0.0
        })
    })
}

/// The bullets the original draws itself (`rndRenderObject`): those without an effect, as a
/// one-pixel line from the bullet along its length in the bullet colour. Beams are not drawn
/// in their first update and Beast infection beams never.
fn original_bullets(u: &Universe, out: &mut EffectBatch) {
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

fn build_refusal(u: &Universe, factory: ShipId, t: ShipType) -> &'static str {
    use hwc_sim::construction as cm;
    let Some(f) = u.ship(factory) else { return "(no factory)" };
    if !cm::can_build_ship_type(u, factory, t, false) {
        return "is not in its build list or has no free build point";
    }
    if !cm::can_build_ship_type(u, factory, t, true) {
        return "needs research (or a docking bay)";
    }
    if !cm::player_has_support_units_for_type(u, t, f.player, 0) {
        return "needs more support units";
    }
    if !cm::unit_cap_can_create_ship(u, t, factory, 0) {
        return "is at its unit cap";
    }
    "cannot be built"
}

/// The ship a ship is clamped to: its clamp host, or for a mission, the level's module clamps.
fn clamp_host(sim: &Sim, s: &hwc_sim::Ship) -> Option<ShipId> {
    s.base.clamp.as_ref().and_then(|c| c.host).or_else(|| {
        sim.mission()?
            .game
            .level
            .clamps
            .iter()
            .find(|(m, _)| *m == s.id)
            .map(|(_, h)| *h)
    })
}

/// The hyperspace clip plane in ship space for effect state `state` (`hsGetEquation`): points
/// with `plane . (p, 1) >= 0` are drawn.
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
        coll_sphere_offset: Vec3::new(c.coll_sphere_offset.x, c.coll_sphere_offset.y, c.coll_sphere_offset.z),
        box_scale: st.hyperspace_box_scale,
        color: [r, g, b],
    };
    hwc_render::effects::hyperspace_window(out, transform, &shape, state, clipt);
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Mission 1's arrival draws one hyperspace window, the mothership's, and it is wide enough
    /// for the clamped modules (the docking bay among them) to pass through it.
    #[test]
    fn mission_one_arrival_has_one_window_covering_the_modules() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR") else {
            return;
        };
        let assets = GameAssets::open(std::path::Path::new(&dir)).unwrap();
        let run = crate::mission::start(&assets, 1, 2, None).unwrap();
        let mut sb = Sandbox::mission(assets, run, false).unwrap();
        let mut checked = false;
        while sb.universe().update_counter < 400 && !checked {
            sb.step();
            let hyper = sb.hyperspace();
            let u = sb.universe();
            let Some((&ms, &(state, clipt))) = hyper.iter().find(|(id, _)| {
                u.ship(**id).is_some_and(|s| s.ship_type == ShipType::sMothership)
            }) else {
                continue;
            };
            if state != hs_effect::SLICING_OUTOF {
                continue;
            }
            let windows: Vec<ShipId> = hyper
                .keys()
                .copied()
                .filter(|&id| u.ship(id).is_some_and(|s| clamp_host(&sb.sim, s).is_none()))
                .collect();
            assert_eq!(windows, vec![ms], "only the mothership opens a window");
            let m = u.ship(ms).unwrap();
            let st = u.statics.ship(m.ship_type).unwrap();
            let mut batch = EffectBatch::default();
            hyperspace_window(&mut batch, Mat4::IDENTITY, st, state, clipt);
            let corners: Vec<Vec3> = batch.triangles.iter().flat_map(|t| t.vertices.iter().map(|v| v.position)).collect();
            let (_, host_m) = Sandbox::transform_of(m);
            let inv = host_m.inverse();
            for &id in hyper.keys().filter(|&&id| id != ms) {
                let p = inv.transform_point3(v3(u.ship(id).unwrap().posinfo.position));
                assert!(
                    corners.iter().any(|c| c.x <= p.x) && corners.iter().any(|c| c.x >= p.x)
                        && corners.iter().any(|c| c.y <= p.y) && corners.iter().any(|c| c.y >= p.y),
                    "module {id:?} at {p} outside the window {corners:?}"
                );
            }
            checked = true;
        }
        assert!(checked, "the mothership never sliced through");
        // B and R say what they did.
        while !sb.sim.mission().unwrap().game.ships_have_arrived {
            sb.step();
        }
        sb.queue_build();
        let note = sb.action_note.clone().unwrap().0;
        assert!(note.starts_with("Queued: ") || note.starts_with("Build refused"), "{note}");
        eprintln!("B: {note}");
        sb.research();
        let note = sb.action_note.clone().unwrap().0;
        assert!(note.starts_with("Researching: ") || note.starts_with("Research refused"), "{note}");
        eprintln!("R: {note}");
    }

    /// Mission 1 on the simulation, drawn offscreen: the fleet arrives through the hyperspace
    /// window, ships get trails, objectives and subtitles reach the overlay. Needs the game
    /// install (`HWC_GAME_DIR`) and a GPU; skips otherwise.
    #[test]
    fn mission_one_frames() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR") else {
            return;
        };
        let assets = GameAssets::open(std::path::Path::new(&dir)).unwrap();
        let Ok(audio) = assets.reopen().and_then(|gd| Audio::new(gd, true, false)) else {
            return;
        };
        let run = crate::mission::start(&assets, 1, 2, None).unwrap();
        let mut sb = Sandbox::mission(assets, run, false).unwrap();
        sb.audio = Some(audio);
        let Ok((device, queue)) = hwc_render::headless_device() else {
            return;
        };
        let mut renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
        let size = (640, 400);
        let mut clipped = 0;
        let mut trail_frames = 0;
        let mut subtitles = 0;
        while sb.universe().update_counter < 1300 {
            sb.step();
            if !sb.universe().update_counter.is_multiple_of(8) {
                continue;
            }
            sb.prepare(&mut renderer);
            let frame = sb.frame(size);
            clipped += frame.items.iter().filter(|i| i.clip.is_some()).count();
            if !frame.effects.sprites.is_empty() || !frame.effects.glow_meshes.is_empty() {
                trail_frames += 1;
            }
            subtitles += usize::from(!sb.hud.subtitles.is_empty());
        }
        assert!(clipped > 0, "no ship drawn through the hyperspace window");
        assert!(trail_frames > 0, "no engine trails or glows");
        assert!(subtitles > 0, "no subtitles from the mission's speech");
        let m = sb.sim.mission().unwrap();
        assert!(m.game.ships_have_arrived);
        assert!(!m.game.objectives.is_empty(), "the first objective is up by now");
        let o = sb.overlay(size, None, None);
        assert!(o.rects.len() > 100, "objective text drawn");
        assert!(sb.hud.music.is_some());
    }
}
