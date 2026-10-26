//! The window: start-up movies, then the front end, then (after the loading screen) a
//! single-player mission; winit events in, frames out through wgpu, sound through hwc-audio.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::consts::key;
use hwc_ui::present::{Filter, Presenter};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{options_mode, Session};
use hwc_ui::Canvas;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::audio::{self, Audio};
use crate::game::{self, Game};
use crate::intro::{Cue, Intro, SEQUENCE};
use crate::movie_script::MovieScript;
use crate::play::{Loading, Progress};
use crate::preview::{Outcome, Preview};
use crate::Args;
use hwc_ui::ingame::InputEvent;
use hwc_ui::AssetSource;

/// The display mode the engine switches to for the start-up movies when full screen
/// (`hwSetRes(640, 480, 32)`, falling back to 16 bits).
const INTRO_MODE: (u32, u32) = (640, 480);

/// `Movies\animatics.lst`: the movie before mission `n` is `ANIMATICS[n - 1]`
/// (`animBinkPlay(m, m + 1)` after mission `m`; Animatic01 on a new game).
const ANIMATICS: [&str; 18] = [
    "Animatic01.bik",
    "Animatic02.bik",
    "Animatic03.bik",
    "Animatic04.bik",
    "Animatic05.bik",
    "Animatic06.bik",
    "Animatic07.bik",
    "Animatic08.bik",
    "Animatic09.bik",
    "Animatic10.bik",
    "Animatic11.bik",
    "Animatic12.bik",
    "Animatic13.bik",
    "Animatic14.bik",
    "Animatic15.bik",
    "Animatic16.bik",
    "Animatic17.bik",
    "Animatic18.bik",
];

struct Gpu {
    presenter: Presenter,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    window: Arc<Window>,
    /// MSAA sample counts the adapter supports for the 3D view.
    msaa_counts: Vec<u32>,
}

struct App {
    /// The player's saved games.
    saves: Rc<crate::saves::SaveDir>,
    session: Session,
    audio: Option<Audio>,
    intro: Option<Intro>,
    movie_canvas: Canvas,
    gpu: Option<Gpu>,
    error: Option<String>,
    last: Instant,
    /// Window size asked for when not full screen.
    size: (u32, u32),
    fullscreen: bool,
    linear: bool,
    started: Instant,
    exit_after: Option<f32>,
    frames: u64,
    /// `--ingame-preview`: starting a game opens the in-game interface on a stand-in fleet.
    ingame_preview: bool,
    source: Rc<dyn AssetSource>,
    preview: Option<Preview>,
    data: Rc<GameData>,
    /// The loading screen between the menu and a mission.
    loading: Option<Box<Loading>>,
    /// The mission being played.
    game: Option<Box<Game>>,
    /// The 3D renderer (created on the window's device when a mission first starts).
    renderer: Option<hwc_render::Renderer>,
    /// `--mission`: start this mission as soon as the front end is up.
    start_mission: Option<u32>,
    /// Mouse position in canvas pixels.
    mouse: (i32, i32),
    /// The movie playing is a mission's (the game waits behind it), not the start-up's.
    game_movie: bool,
    /// The levels `animBinkPlay` saved before the movie (`soundEventGetVolume`), put back
    /// when it ends: the movie's script sets levels of its own.
    movie_volumes: Option<hwc_audio::system::Volumes>,
    /// `--rollcall-below-subtitles`.
    rollcall_below_band: bool,
    /// How the movies (intro and animatics, with their subtitles) fill the window.
    movie_scale: ScaleMode,
    /// `--debug-keys`.
    debug_keys: bool,
    /// `--autoplay`: the simulation's test player plays each mission.
    autoplay: bool,
    /// `--lod` and `--right-click` (None: the options file decides).
    full_detail: Option<bool>,
    right_click_move: Option<bool>,
    /// `--render-scale`, `--msaa`, `--fxaa`, `--scene-res`, `--zoom` (None: the options file).
    render_scale: Option<f32>,
    msaa: Option<u32>,
    fxaa: Option<bool>,
    scene_at_window: Option<bool>,
    smooth_zoom: Option<bool>,
    /// `--focus-zoom`, `--zoom-limit`, `--draw-distance` (None: the options file).
    focus_zoom: Option<bool>,
    zoom_clear: Option<bool>,
    render_list: Option<bool>,
    /// `--letterbox` (None: the options file).
    tall_letterbox: Option<bool>,
    /// `--no-vsync`.
    no_vsync: bool,
    /// `--frame-times`: the 3D frame's draw times, printed every few seconds.
    frame_times: Option<FrameTimes>,
}

/// Draw times of the mission's frames (`--frame-times`): the time `Game::render` takes, which
/// waits for the GPU to finish the 3D view (it is read back), and the time between frames.
#[derive(Default)]
struct FrameTimes {
    render: Vec<f64>,
    interval: Vec<f64>,
}

impl FrameTimes {
    const EVERY: usize = 240;

    fn add(&mut self, render_ms: f64, interval_ms: f64) {
        self.render.push(render_ms);
        self.interval.push(interval_ms);
        if self.render.len() >= Self::EVERY {
            let stats = |v: &mut Vec<f64>| {
                v.sort_by(f64::total_cmp);
                let mean = v.iter().sum::<f64>() / v.len() as f64;
                let p95 = v[(v.len() * 95 / 100).min(v.len() - 1)];
                let max = v[v.len() - 1];
                v.clear();
                (mean, p95, max)
            };
            let r = stats(&mut self.render);
            let i = stats(&mut self.interval);
            eprintln!(
                "cataclysm: frame times over {} frames: render mean {:.1} ms, p95 {:.1}, max {:.1}; frame interval mean {:.1} ms, p95 {:.1}, max {:.1}",
                Self::EVERY, r.0, r.1, r.2, i.0, i.1, i.2
            );
        }
    }
}

pub fn run(session: Session, data: Rc<GameData>, args: &Args) -> Result<(), String> {
    let mode = options_mode(&session.fe.game.options);
    let size = args
        .size
        .unwrap_or((mode.0.max(320) as u32, mode.1.max(240) as u32));
    // The sound system starts before the movies (`soundEventInit`), with the saved volumes.
    let audio = match crate::open_game_data(args).and_then(|g| Audio::open(g, args.sound)) {
        Ok(mut a) => {
            a.apply_options(&session.fe.game.options);
            Some(a)
        }
        Err(e) => {
            eprintln!("cataclysm: no sound: {e}");
            None
        }
    };
    let source: Rc<dyn AssetSource> = data.clone();
    let intro = if args.intro {
        match Intro::new(data.clone(), &SEQUENCE) {
            Ok(i) => Some(i),
            Err(e) => {
                eprintln!("cataclysm: intro movies unavailable: {e}");
                None
            }
        }
    } else {
        None
    };
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let saves = Rc::new(crate::saves::SaveDir::locate(&data));
    let mut session = session;
    session.fe.game.saves = Some(saves.clone());
    let mut app = App {
        saves,
        session,
        audio,
        intro,
        movie_canvas: Canvas::new(1, 1),
        gpu: None,
        error: None,
        last: Instant::now(),
        size,
        fullscreen: args.fullscreen,
        linear: args.linear,
        started: Instant::now(),
        exit_after: args.exit_after,
        frames: 0,
        ingame_preview: args.ingame_preview,
        source,
        preview: None,
        data,
        loading: None,
        game: None,
        renderer: None,
        start_mission: args.mission,
        mouse: (0, 0),
        game_movie: false,
        movie_volumes: None,
        rollcall_below_band: args.rollcall_below_band,
        // User request: movies stretch to the window unless `--scale` says otherwise (the
        // original draws them unscaled).
        movie_scale: if args.scale_given {
            args.scale
        } else {
            ScaleMode::Stretch
        },
        debug_keys: args.debug_keys,
        autoplay: args.autoplay,
        full_detail: args.full_detail,
        right_click_move: args.right_click_move,
        render_scale: args.render_scale,
        msaa: args.msaa,
        fxaa: args.fxaa,
        scene_at_window: args.scene_at_window,
        smooth_zoom: args.smooth_zoom,
        focus_zoom: args.focus_zoom,
        zoom_clear: args.zoom_clear,
        render_list: args.render_list,
        tall_letterbox: args.tall_letterbox,
        no_vsync: args.no_vsync,
        frame_times: args.frame_times.then(FrameTimes::default),
    };
    if app.intro.is_none() {
        app.front_end_start();
    }
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

impl Gpu {
    fn new(window: Arc<Window>, linear: bool, no_vsync: bool) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let adapter =
            pollster::block_on(
                instance.request_adapter(&wgpu::RequestAdapterOptions {
                    // The game's 3D view wants the faster GPU when there are two (`WGPU_POWER_PREF`
                    // = low or high chooses); the integrated one also shares memory with the desktop.
                    power_preference: wgpu::PowerPreference::from_env()
                        .unwrap_or(wgpu::PowerPreference::HighPerformance),
                    compatible_surface: Some(&surface),
                    ..Default::default()
                }),
            )
            .map_err(|e| format!("no GPU adapter: {e}"))?;
        // With the adapter's own format features MSAA may use 2 and 8 samples as well as 4.
        let (device, queue) = hwc_render::request_device(&adapter)?;
        let msaa_counts = hwc_render::quality::supported_sample_counts(&adapter, device.features());
        let info = adapter.get_info();
        eprintln!(
            "cataclysm: GPU {} ({:?}, driver {} {})",
            info.name, info.backend, info.driver, info.driver_info
        );
        // GPU errors and a lost device go to the crash log instead of passing unseen.
        device.on_uncaptured_error(Arc::new(|e: wgpu::Error| {
            crate::crash::log(&format!("wgpu error: {e}"));
        }));
        device.set_device_lost_callback(|reason, message| {
            // Dropping the device at exit reports `Destroyed`.
            if reason != wgpu::DeviceLostReason::Destroyed {
                crate::crash::set_gpu_lost();
                crate::crash::log(&format!(
                    "wgpu device lost ({reason:?}): {message}

the last frames drawn:
{}",
                    crate::draw_check::history()
                ));
            }
        });
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by the adapter")?;
        // The frame holds final sRGB-encoded pixels: show them without conversion.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = if no_vsync {
            wgpu::PresentMode::AutoNoVsync
        } else {
            wgpu::PresentMode::AutoVsync
        };
        surface.configure(&device, &config);
        let filter = if linear {
            Filter::Linear
        } else {
            Filter::Nearest
        };
        let presenter = Presenter::new(&device, config.format, filter);
        Ok(Self {
            presenter,
            surface,
            config,
            device,
            queue,
            window,
            msaa_counts,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    fn present(&mut self, canvas: &Canvas, viewport: (f32, f32, f32, f32)) {
        // The canvas's pixels are already RGBA8 rows: uploaded as they are, without a copy.
        let span = crate::profile::span("upload");
        let size = (canvas.width.max(1) as u32, canvas.height.max(1) as u32);
        let rgba = canvas.pixels.as_flattened();
        crate::profile::count("upload_bytes", rgba.len() as u64);
        if rgba.len() == (size.0 * size.1 * 4) as usize {
            self.presenter
                .upload_rgba(&self.device, &self.queue, size, rgba);
        } else {
            self.presenter.upload(&self.device, &self.queue, canvas);
        }
        drop(span);
        self.present_uploaded(viewport, None);
    }

    /// The 3D view at window resolution under the canvas (`rgba`, its pixels nothing was drawn
    /// over transparent).
    fn present_layered(
        &mut self,
        rgba: &[u8],
        size: (u32, u32),
        viewport: (f32, f32, f32, f32),
        scene: &wgpu::TextureView,
    ) {
        let span = crate::profile::span("upload");
        crate::profile::count("upload_bytes", rgba.len() as u64);
        self.presenter
            .upload_rgba(&self.device, &self.queue, size, rgba);
        drop(span);
        self.present_uploaded(viewport, Some(scene));
    }

    fn present_uploaded(
        &mut self,
        viewport: (f32, f32, f32, f32),
        scene: Option<&wgpu::TextureView>,
    ) {
        // Includes the wait for a swap-chain image (vsync).
        let _p = crate::profile::span("present");
        let frame = match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                let size = self.window.inner_size();
                self.resize(size.width, size.height);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = self.device.create_command_encoder(&Default::default());
        match scene {
            Some(s) => self
                .presenter
                .draw_layered(&self.device, &mut encoder, &view, viewport, s),
            None => self.presenter.draw(&mut encoder, &view, viewport),
        }
        self.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        self.queue.present(frame);
    }
}

/// An exclusive full-screen mode of `size` on the window's monitor: 32 bits preferred (then
/// 16, like `hwSetRes`), at the monitor's current refresh rate when it offers it.
fn exclusive_mode(window: &Window, size: (u32, u32)) -> Option<Fullscreen> {
    let monitor = window.current_monitor()?;
    let refresh = monitor.refresh_rate_millihertz();
    monitor
        .video_modes()
        .filter(|m| (m.size().width, m.size().height) == size)
        .max_by_key(|m| {
            (
                m.bit_depth() == 32,
                m.bit_depth(),
                Some(m.refresh_rate_millihertz()) == refresh,
                m.refresh_rate_millihertz(),
            )
        })
        .map(Fullscreen::Exclusive)
}

/// Engine key code (a Windows virtual-key code, as the original reads) for a key event. The
/// digit keys come from their place on the keyboard, as virtual-key codes do: with Ctrl or
/// Shift held the character they type is no digit (Ctrl+1 types nothing, Shift+1 a `!`), which
/// lost the hot-key groups. Other keys are read without the modifiers' effect.
fn event_code(event: &winit::event::KeyEvent) -> Option<u16> {
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::platform::modifier_supplement::KeyEventExtModifierSupplement;
    if let PhysicalKey::Code(c) = event.physical_key {
        let digit = [
            KeyCode::Digit0,
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ]
        .iter()
        .position(|&d| d == c);
        if let Some(n) = digit {
            return Some(0x30 + n as u16);
        }
    }
    key_code(&event.key_without_modifiers())
}

/// Engine key code for a winit key.
fn key_code(k: &Key) -> Option<u16> {
    Some(match k {
        Key::Named(n) => match n {
            NamedKey::Escape => key::ESCAPE,
            NamedKey::Enter => key::RETURN,
            NamedKey::Tab => key::TAB,
            NamedKey::Space => key::SPACE,
            NamedKey::Backspace => key::BACKSPACE,
            NamedKey::Delete => key::DELETE,
            NamedKey::ArrowLeft => key::LEFT,
            NamedKey::ArrowRight => key::RIGHT,
            NamedKey::ArrowUp => key::UP,
            NamedKey::ArrowDown => key::DOWN,
            NamedKey::Home => key::HOME,
            NamedKey::End => key::END,
            NamedKey::PageUp => key::PAGE_UP,
            NamedKey::PageDown => key::PAGE_DOWN,
            NamedKey::Shift => key::SHIFT,
            NamedKey::Control => key::CONTROL,
            NamedKey::Alt => key::ALT,
            NamedKey::CapsLock => 0x14,
            NamedKey::Insert => 0x2d,
            NamedKey::F1 => 0x70,
            NamedKey::F2 => 0x71,
            NamedKey::F3 => 0x72,
            NamedKey::F4 => 0x73,
            NamedKey::F5 => 0x74,
            NamedKey::F6 => 0x75,
            NamedKey::F7 => 0x76,
            NamedKey::F8 => 0x77,
            NamedKey::F9 => 0x78,
            NamedKey::F12 => 0x7b,
            _ => return None,
        },
        Key::Character(s) => {
            let c = s.chars().next()?.to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                c as u16
            } else {
                // Windows virtual-key codes of the punctuation keys (US layout).
                match c {
                    ' ' => key::SPACE,
                    '`' | '~' => 0xc0,
                    '=' => 0xbb,
                    '+' => 0x6b,
                    '-' | '_' => 0xbd,
                    '[' | '{' => 0xdb,
                    ']' | '}' => 0xdd,
                    '\\' | '|' => 0xdc,
                    ';' | ':' => 0xba,
                    '\'' | '"' => 0xde,
                    ',' | '<' => 0xbc,
                    '.' | '>' => 0xbe,
                    '/' | '?' => 0xbf,
                    _ => return None,
                }
            }
        }
        _ => return None,
    })
}

/// The command line's modern options (`--lod`, `--msaa`, `--zoom`...): over the options
/// file's for this run.
#[derive(Debug, Clone, Copy)]
struct Overrides {
    full_detail: Option<bool>,
    right_click_move: Option<bool>,
    render_scale: Option<f32>,
    msaa: Option<u32>,
    fxaa: Option<bool>,
    scene_at_window: Option<bool>,
    smooth_zoom: Option<bool>,
    focus_zoom: Option<bool>,
    zoom_clear: Option<bool>,
    render_list: Option<bool>,
    tall_letterbox: Option<bool>,
}

impl Overrides {
    /// Sets them on the game (at its start, and after the options are applied).
    fn apply(&self, game: &mut Game) {
        if let Some(v) = self.full_detail {
            game.world.full_detail = v;
        }
        if let Some(v) = self.right_click_move {
            game.right_click_move = v;
        }
        if let Some(s) = self.render_scale {
            game.quality.scale = hwc_render::quality::clamp_scale(s);
        }
        if let Some(n) = self.msaa {
            game.quality.msaa = n;
        }
        if let Some(v) = self.fxaa {
            game.quality.fxaa = v;
        }
        if let Some(v) = self.scene_at_window {
            game.scene_at_window = v;
        }
        if let Some(v) = self.smooth_zoom {
            game.smooth_zoom = v;
        }
        if let Some(v) = self.focus_zoom {
            game.world.focus_zoom = v;
        }
        if let Some(v) = self.zoom_clear {
            game.world.zoom_clear = v;
        }
        if let Some(v) = self.render_list {
            game.world.render_list = v;
        }
        if let Some(v) = self.tall_letterbox {
            game.letterbox.tall = v;
        }
    }

    /// Tells the modern options screen which of its options the command line set (shown,
    /// not changed there).
    fn lock(&self, game: &mut Game) {
        let ui = &mut game.ui;
        let b = |v: bool| i32::from(v);
        let locks = [
            ("hwcFullDetailShips", self.full_detail.map(b)),
            ("hwcRightClickMove", self.right_click_move.map(b)),
            (
                "hwcRenderScale",
                self.render_scale.map(|s| (s * 100.0).round() as i32),
            ),
            ("hwcMsaa", self.msaa.map(|n| n as i32)),
            ("hwcFxaa", self.fxaa.map(b)),
            ("hwcSceneAtWindow", self.scene_at_window.map(b)),
            ("hwcSmoothZoom", self.smooth_zoom.map(b)),
            ("hwcFocusZoom", self.focus_zoom.map(b)),
            ("hwcZoomClear", self.zoom_clear.map(b)),
            ("hwcRenderList", self.render_list.map(b)),
            ("hwcOriginalLetterbox", self.tall_letterbox.map(|t| b(!t))),
        ];
        for (key, v) in locks {
            if let Some(v) = v {
                ui.lock_option(key, v);
            }
        }
    }
}

impl App {
    fn overrides(&self) -> Overrides {
        Overrides {
            full_detail: self.full_detail,
            right_click_move: self.right_click_move,
            render_scale: self.render_scale,
            msaa: self.msaa,
            fxaa: self.fxaa,
            scene_at_window: self.scene_at_window,
            smooth_zoom: self.smooth_zoom,
            focus_zoom: self.focus_zoom,
            zoom_clear: self.zoom_clear,
            render_list: self.render_list,
            tall_letterbox: self.tall_letterbox,
        }
    }

    /// After the movies (`mainCleanupAfterVideo`): back to the configured display mode, then
    /// the front end and its music.
    fn front_end_start(&mut self) {
        let (w, h) = options_mode(&self.session.fe.game.options);
        self.set_display_mode(w, h);
        if let Some(a) = &mut self.audio {
            a.play_music(audio::FRONT_END_TRACK);
        }
        if let Some(n) = self.start_mission.take() {
            let difficulty = self.session.fe.game.options.get("DifficultySetting");
            self.start_loading(n, difficulty);
        }
    }

    /// `singlePlayerStartGame`: the menus go, the loading screen comes up.
    fn start_loading(&mut self, mission: u32, difficulty: i32) {
        self.session.fe.all_screens_delete();
        let size = self.session.placement.canvas;
        match Loading::new(self.data.clone(), &self.session, mission, difficulty, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l))
            }
            Err(e) => {
                eprintln!("cataclysm: cannot start mission {mission}: {e}");
                self.back_to_menu();
            }
        }
    }

    /// `utyLoadMultiSinglePlayerGame` / `gpLoadGame`: a saved game loads behind its mission's
    /// loading screen (the game running, if any, ends first and the music stops).
    fn load_saved_game(&mut self, path: &std::path::Path) {
        self.game = None;
        self.loading = None;
        if let Some(a) = &mut self.audio {
            a.stop_music(0.0);
            a.stop_all_sfx();
        }
        self.session.fe.all_screens_delete();
        let size = self.session.placement.canvas;
        match Loading::from_save(self.data.clone(), &self.session, path, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l));
            }
            Err(e) => {
                eprintln!("cataclysm: cannot load {}: {e}", path.display());
                self.back_to_menu();
            }
        }
    }

    /// The fleet has jumped: the next mission loads with it (`singlePlayerLoadNewLevel`, the
    /// loading screen of the next mission), or after the last one the game ends.
    fn next_mission(&mut self) {
        let Some(g) = self.game.take() else { return };
        let run = g.world.run();
        let (mission, skill) = (run.game.config.mission, run.game.config.skill);
        let carry = run.carry();
        // The hot-key groups go with the ships that jumped.
        let groups = carry.as_ref().map(|c| {
            g.hotkeys.carry_out(
                &run.universe,
                &run.game.hyper.in_hyperspace,
                crate::host::PLAYER,
                c,
                mission + 1,
            )
        });
        // `singlePlayerStartNis` reads these from the mission that has just ended.
        let centre = run
            .var(None, "G_NisCentreDerelict")
            .map(|t| (t, run.var(None, "G_NisCentreInstance")));
        drop(g);
        let next = mission + 1;
        let carry = match carry {
            Some(c) if crate::mission::supported(next) => c,
            _ => {
                self.back_to_menu();
                return;
            }
        };
        if let Some(a) = &mut self.audio {
            a.stop_music(0.0);
        }
        let size = self.session.placement.canvas;
        match Loading::new(self.data.clone(), &self.session, next, skill, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l.with_carry(carry, centre).with_groups(groups)))
            }
            Err(e) => {
                eprintln!("cataclysm: cannot start mission {next}: {e}");
                self.back_to_menu();
            }
        }
    }

    /// The NIS level's NIS has stopped: the mission loads from it behind a loading screen
    /// of its own (`singlePlayerNisStoppedCB` -> `singlePlayerLoadNewLevel(0, 0)`).
    fn after_nis_level(&mut self) {
        let Some(mut g) = self.game.take() else {
            return;
        };
        let skill = g.world.skill();
        let groups = g.carried_groups.take();
        let crate::world::Sim::NisLevel(nis) = g.world.into_sim() else {
            self.back_to_menu();
            return;
        };
        let size = self.session.placement.canvas;
        let mission = nis.mission;
        match Loading::after_nis_level(self.data.clone(), &self.session, *nis, skill, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l.with_groups(groups)))
            }
            Err(e) => {
                eprintln!("cataclysm: cannot start mission {mission}: {e}");
                self.back_to_menu();
            }
        }
    }

    /// A mission's movie (`animBinkPlay`): played full screen while the game waits, with
    /// its soundtrack script `Movies\<name>.script` (music, levels, speech and subtitles).
    /// The music and effects stop and the levels are kept to restore afterwards.
    fn play_animatic(&mut self, n: u32) -> bool {
        let Some(&file) = ANIMATICS.get(n.wrapping_sub(1) as usize) else {
            return false;
        };
        let steps = [crate::intro::Step::Movie { file, music: -1 }];
        match Intro::new(self.data.clone(), &steps) {
            Ok(mut i) => {
                let name = script_name(file);
                match self.source.read(&name).map(|d| MovieScript::parse(&d)) {
                    Some(Ok(script)) => {
                        for (line, what) in &script.ignored {
                            eprintln!("note: {name}:{line}: {what} (ignored)");
                        }
                        i.set_script(script);
                    }
                    Some(Err(e)) => eprintln!("cataclysm: {name}: {e}"),
                    // `animLoadNISScript` gives an empty event list when the file is missing.
                    None => eprintln!("cataclysm: {name} not found; the movie plays silent"),
                }
                if let Some(a) = &mut self.audio {
                    a.stop_music(0.0);
                    a.stop_all_sfx();
                    self.movie_volumes = Some(a.volumes());
                }
                if let Some(s) = self.game.as_mut().and_then(|g| g.subtitles.as_mut()) {
                    s.reset();
                }
                self.intro = Some(i);
                self.game_movie = true;
                true
            }
            Err(e) => {
                eprintln!("cataclysm: {file}: {e}");
                false
            }
        }
    }

    /// Back to the main menu and its music (quitting a game, or a mission that cannot start).
    fn back_to_menu(&mut self) {
        self.loading = None;
        self.game = None;
        if let Some(a) = &mut self.audio {
            a.set_in_game(false);
            a.apply_options(&self.session.fe.game.options);
            a.play_music(audio::FRONT_END_TRACK);
        }
        self.session.fe.all_screens_delete();
        self.session.fe.screen_start(hwc_ui::session::MAIN_SCREEN);
    }

    /// One frame of the loading screen or the mission; true when one of them is up.
    fn game_frame(&mut self, dt: f32) -> Result<bool, String> {
        let size = self.session.placement.canvas;
        if let Some(l) = &mut self.loading {
            let viewport = self.session.placement.viewport;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.present(l.render(size), viewport);
                if self.renderer.is_none() {
                    let mut r = hwc_render::Renderer::new(
                        &gpu.device,
                        &gpu.queue,
                        hwc_render::OFFSCREEN_FORMAT,
                    );
                    r.set_msaa_support(&gpu.msaa_counts);
                    if crate::profile::enabled() && !r.set_gpu_timing(true) {
                        eprintln!(
                            "cataclysm: --profile: no GPU timestamp queries here, CPU times only"
                        );
                    }
                    self.renderer = Some(r);
                }
            }
            match l.advance(&self.session, self.renderer.as_mut(), dt) {
                Progress::Working => {}
                Progress::Ready(mut game) => {
                    self.loading = None;
                    game.debug_keys |= self.debug_keys;
                    if self.autoplay {
                        game.autoplay = Some(hwc_sim::autoplay::Autoplay::new(16 * 60 * 30));
                    }
                    game.rollcall_below_band = self.rollcall_below_band;
                    let ov = self.overrides();
                    ov.apply(&mut game);
                    ov.lock(&mut game);
                    if let Some(r) = &self.renderer {
                        let q = game.quality;
                        let n = hwc_render::quality::pick_sample_count(q.msaa, r.msaa_support());
                        if n != q.msaa {
                            eprintln!(
                                "cataclysm: {}x MSAA is not supported here (supported: {:?}); using {n}x",
                                q.msaa,
                                r.msaa_support()
                            );
                        }
                        if !q.is_original() {
                            eprintln!(
                                "cataclysm: 3D view: render scale {:.2}, {n}x MSAA, FXAA {}",
                                q.scale,
                                if q.fxaa { "on" } else { "off" }
                            );
                        }
                    }
                    if let Some(a) = &mut self.audio {
                        a.stop_music(0.0);
                        a.set_in_game(true);
                    }
                    // `singlePlayerPostInit`: a new game shows Animatic01 before the opening.
                    let new_game = game.world.mission() == 1 && game.has_queued_nis();
                    self.game = Some(game);
                    if new_game {
                        self.play_animatic(1);
                    }
                }
                Progress::Abandon(why) => {
                    eprintln!("cataclysm: {why}");
                    self.back_to_menu();
                }
            }
            return Ok(true);
        }
        let ov = self.overrides();
        let Some(g) = self.game.as_mut() else {
            return Ok(false);
        };
        let mut leave = false;
        let mut restart = false;
        let mut load: Option<std::path::PathBuf> = None;
        let mut animatic = None;
        let mut next = false;
        let mut nis_level_done = false;
        crate::profile::frame_start();
        let span = crate::profile::span("update");
        let outcomes = g.update(dt, &mut self.audio);
        drop(span);
        for o in outcomes {
            match o {
                game::Outcome::Leave => leave = true,
                game::Outcome::Restart => restart = true,
                game::Outcome::ApplyOptions => {
                    if let Some(a) = &mut self.audio {
                        a.apply_options(&g.ui.fe.game.options);
                    }
                    // The modern options apply at once (the command line's still win).
                    let options = g.ui.fe.game.options.clone();
                    g.apply_modern_options(&options);
                    ov.apply(g);
                }
                game::Outcome::Animatic(n) => animatic = Some(n),
                game::Outcome::NextMission => next = true,
                game::Outcome::NisLevelDone => nis_level_done = true,
                game::Outcome::LoadGame(p) => load = Some(p),
            }
        }
        if let Some(p) = load {
            self.load_saved_game(&p);
            return Ok(true);
        }
        if nis_level_done {
            self.after_nis_level();
            return Ok(true);
        }
        if let Some(n) = animatic {
            if !self.play_animatic(n) {
                if let Some(g) = self.game.as_mut() {
                    g.movie_finished();
                }
            }
            return Ok(true);
        }
        if next {
            self.next_mission();
            return Ok(true);
        }
        let Some(g) = self.game.as_mut() else {
            return Ok(true);
        };
        if leave || restart {
            let (mission, difficulty) = (g.world.mission(), g.world.skill());
            // `SP_Restart_Level` (`gpLoadGivenGame`): the latest mission autosave, when there
            // is one; the mission from its start otherwise.
            let level_save = g.level_save.clone().filter(|p| p.is_file());
            self.back_to_menu();
            if restart {
                match level_save {
                    Some(p) => self.load_saved_game(&p),
                    None => self.start_loading(mission, difficulty),
                }
            }
            return Ok(true);
        }
        let (Some(gpu), Some(r)) = (self.gpu.as_mut(), self.renderer.as_mut()) else {
            return Ok(true);
        };
        // In a scaled front-end mode the 3D view can be drawn at the window's pixels.
        let viewport = self.session.placement.viewport;
        g.present = (g.scene_at_window && self.session.scale != ScaleMode::Original).then(|| {
            (
                viewport.2.round().max(1.0) as u32,
                viewport.3.round().max(1.0) as u32,
            )
        });
        let started = Instant::now();
        let span = crate::profile::span("render");
        g.render(r, size)?;
        drop(span);
        if let Some(t) = &mut self.frame_times {
            t.add(started.elapsed().as_secs_f64() * 1e3, f64::from(dt) * 1e3);
        }
        // The move plate put the pointer somewhere (`mousePositionSet`): the system pointer
        // follows, so the next movement starts from there.
        if let Some((x, y)) = g.take_pointer_warp() {
            self.mouse = (x, y);
            let (wx, wy) = self.session.placement.to_window(x, y);
            let _ = gpu
                .window
                .set_cursor_position(winit::dpi::PhysicalPosition::new(wx, wy));
        }
        if self.session.cursor_visible {
            g.draw_cursor();
        }
        match g.scene_layer() {
            Some((rgba, scene)) => {
                let c = g.canvas();
                let size = (c.width.max(1) as u32, c.height.max(1) as u32);
                gpu.present_layered(&rgba, size, viewport, scene);
            }
            None => gpu.present(g.canvas(), viewport),
        }
        crate::profile::frame_end();
        Ok(true)
    }

    /// Full screen: the exclusive mode of that size if the monitor has it (else borderless);
    /// in a window: the window size.
    fn set_display_mode(&mut self, w: i32, h: i32) {
        let size = (w.max(320) as u32, h.max(240) as u32);
        let Some(gpu) = &self.gpu else {
            return;
        };
        if self.fullscreen {
            let fs = exclusive_mode(&gpu.window, size).or(Some(Fullscreen::Borderless(None)));
            gpu.window.set_fullscreen(fs);
        } else {
            self.size = size;
            if gpu.window.fullscreen().is_none() {
                let _ = gpu
                    .window
                    .request_inner_size(PhysicalSize::new(size.0, size.1));
            }
        }
    }

    fn cues(&mut self, cues: Vec<Cue>) {
        for c in cues {
            match c {
                Cue::PlayMusic(t) => {
                    if let Some(a) = &mut self.audio {
                        a.play_music(t);
                    }
                }
                Cue::StopMusic => {
                    if let Some(a) = &mut self.audio {
                        a.stop_music(0.0);
                    }
                }
                Cue::Script(action) => {
                    let (subs, mission) = match self.game.as_mut() {
                        Some(g) => (g.subtitles.as_mut(), g.world.mission()),
                        None => (None, 0),
                    };
                    crate::movie_script::present(action, &mut self.audio, subs, mission);
                }
            }
        }
    }

    /// After a mission's movie (`animBinkPlay` returning): music, effects, speech and
    /// subtitles stop and the saved levels come back.
    fn animatic_finished(&mut self) {
        if let Some(a) = &mut self.audio {
            a.stop_music(0.0);
            a.stop_all_sfx();
            a.stop_all_speech();
            if let Some(v) = self.movie_volumes.take() {
                a.set_volumes(v);
            }
        }
        if let Some(g) = self.game.as_mut() {
            if let Some(s) = g.subtitles.as_mut() {
                s.reset();
            }
            g.movie_finished();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        // A lost device (a driver reset) cannot draw again: the game stops with a message
        // rather than failing in the next GPU call.
        if crate::crash::gpu_lost() {
            return Err(format!(
                "the graphics device was lost (the driver reset the GPU); details are in {}",
                crate::crash::LOG_NAME
            ));
        }
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        if let Some(a) = &mut self.audio {
            a.update();
        }
        if let Some(intro) = &mut self.intro {
            let cues = intro.update(dt);
            let done = intro.finished();
            let clock = intro.script_clock();
            self.cues(cues);
            // `animBinkDecode`'s `subTitlesUpdate`, on the movie clock.
            if let (Some(t), true) = (clock, self.game_movie) {
                if let Some(s) = self.game.as_mut().and_then(|g| g.subtitles.as_mut()) {
                    s.update(t);
                }
            }
            if done {
                self.intro = None;
                if std::mem::take(&mut self.game_movie) {
                    self.animatic_finished();
                } else {
                    self.front_end_start();
                }
            }
        }
        if self.intro.is_none() && (self.loading.is_some() || self.game.is_some()) {
            self.game_frame(dt)?;
            self.frames += 1;
            // The draw counts of the last frames, on disk in case a GPU reset ends the process.
            if self.frames.is_multiple_of(8) {
                crate::draw_check::write_history();
            }
            // Development (`HWC_GPU_REPORT`): the GPU allocations every 600 frames.
            if self.frames.is_multiple_of(600) && std::env::var_os("HWC_GPU_REPORT").is_some() {
                if let Some(gpu) = &self.gpu {
                    crate::crash::gpu_report(&gpu.device);
                }
            }
            if self
                .exit_after
                .is_some_and(|t| self.started.elapsed().as_secs_f32() >= t)
            {
                eprintln!("cataclysm: {} frames presented", self.frames);
                event_loop.exit();
            }
            return Ok(());
        }
        if let Some(p) = &mut self.preview {
            for o in p.update(dt) {
                match o {
                    Outcome::Leave => {
                        self.preview = None;
                        self.session.fe.all_screens_delete();
                        self.session.fe.screen_start(hwc_ui::session::MAIN_SCREEN);
                        break;
                    }
                    Outcome::Sound(e) => {
                        if let Some(a) = &mut self.audio {
                            a.event(e);
                        }
                    }
                    Outcome::Speech(e) => {
                        if let Some(a) = &mut self.audio {
                            a.speech(e);
                        }
                    }
                    Outcome::ApplyOptions => {
                        if let (Some(a), Some(p)) = (&mut self.audio, &self.preview) {
                            a.apply_options(&p.ui.fe.game.options);
                        }
                    }
                }
            }
        } else if self.intro.is_none() {
            for r in self.session.update(dt) {
                self.request(event_loop, r);
            }
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        match &self.intro {
            Some(intro) => {
                let s = gpu.window.inner_size();
                let placement = intro.render(
                    &mut self.movie_canvas,
                    self.movie_scale,
                    (s.width, s.height),
                );
                if let (Some(t), true) = (intro.script_clock(), self.game_movie) {
                    if let Some(subs) = self.game.as_mut().and_then(|g| g.subtitles.as_mut()) {
                        draw_movie_subtitles(subs, &mut self.movie_canvas, t);
                    }
                }
                gpu.present(&self.movie_canvas, placement.viewport);
            }
            None => {
                let viewport = self.session.placement.viewport;
                let canvas = match &mut self.preview {
                    Some(p) => p.render(&self.session),
                    None => self.session.render(),
                };
                gpu.present(canvas, viewport);
            }
        }
        self.frames += 1;
        if self
            .exit_after
            .is_some_and(|t| self.started.elapsed().as_secs_f32() >= t)
        {
            eprintln!("cataclysm: {} frames presented", self.frames);
            event_loop.exit();
        }
        Ok(())
    }

    fn request(&mut self, event_loop: &ActiveEventLoop, r: Request) {
        match r {
            Request::Quit => {
                // `utyGameQuit`: fade the music, pause the sound, close.
                if let Some(a) = &mut self.audio {
                    a.stop_music(audio::QUIT_FADE);
                    a.pause();
                }
                event_loop.exit();
            }
            Request::SoundEvent(e) => {
                if let Some(a) = &mut self.audio {
                    a.event(e);
                }
            }
            Request::Speech(e) => {
                if let Some(a) = &mut self.audio {
                    a.speech(e);
                }
            }
            Request::AudioSettings | Request::ApplyOptions => {
                if let Some(a) = &mut self.audio {
                    a.apply_options(&self.session.fe.game.options);
                }
            }
            Request::DisplayMode { width, height } => self.set_display_mode(width, height),
            Request::StartGame {
                tutorial,
                difficulty,
            } => {
                if self.ingame_preview {
                    match Preview::new(self.source.clone(), &self.session) {
                        Ok(p) => self.preview = Some(p),
                        Err(e) => eprintln!("cataclysm: in-game preview: {e}"),
                    }
                    return;
                }
                // The training mission is not a campaign level the simulation can start yet:
                // mission 0 shows the "not playable" note. New Game starts mission 1
                // (`singlePlayerInit`).
                self.start_loading(if tutorial { 0 } else { 1 }, difficulty);
            }
            Request::LoadGame(key) => self.load_saved_game(std::path::Path::new(&key)),
            Request::Multiplayer => eprintln!("cataclysm: multiplayer is not available yet"),
            Request::OpenUrl(url) => eprintln!("cataclysm: troubleshooting page: {url}"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Homeworld: Cataclysm")
            .with_inner_size(PhysicalSize::new(self.size.0, self.size.1));
        let result = event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w), self.linear, self.no_vsync));
        match result {
            Ok(gpu) => {
                // The engine draws its own pointer.
                gpu.window.set_cursor_visible(false);
                if self.fullscreen {
                    // The intro runs in 640x480; without it, the configured mode right away.
                    let size = if self.intro.is_some() {
                        INTRO_MODE
                    } else {
                        let (w, h) = options_mode(&self.session.fe.game.options);
                        (w.max(320) as u32, h.max(240) as u32)
                    };
                    let fs =
                        exclusive_mode(&gpu.window, size).or(Some(Fullscreen::Borderless(None)));
                    gpu.window.set_fullscreen(fs);
                }
                let s = gpu.window.inner_size();
                self.session.resize(s.width, s.height);
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(gpu) = self.gpu.as_mut() {
                    gpu.resize(size.width, size.height);
                }
                self.session.resize(size.width, size.height);
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw(event_loop) {
                    self.error = Some(e);
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                let down = event.state == ElementState::Pressed;
                if down {
                    match &event.logical_key {
                        Key::Named(NamedKey::F11) => {
                            if let Some(gpu) = &self.gpu {
                                let fs = gpu
                                    .window
                                    .fullscreen()
                                    .is_none()
                                    .then_some(Fullscreen::Borderless(None));
                                self.fullscreen = fs.is_some();
                                gpu.window.set_fullscreen(fs);
                            }
                            return;
                        }
                        Key::Named(NamedKey::F10) => {
                            let next = match self.session.scale {
                                ScaleMode::Original => ScaleMode::Fit,
                                ScaleMode::Fit => ScaleMode::Integer,
                                ScaleMode::Integer => ScaleMode::Stretch,
                                ScaleMode::Stretch => ScaleMode::Original,
                            };
                            self.session.set_scale(next);
                            self.movie_scale = next;
                            eprintln!("cataclysm: scaling {next:?}");
                            return;
                        }
                        _ => {}
                    }
                }
                if let Some(intro) = &mut self.intro {
                    // Only releasing Escape does anything during the movies (`binkStop`).
                    if !down && event.logical_key == Key::Named(NamedKey::Escape) {
                        let mut cues = Vec::new();
                        intro.stop_movie(&mut cues);
                        self.cues(cues);
                    }
                    return;
                }
                if self.loading.is_some() {
                    return;
                }
                if let Some(g) = &mut self.game {
                    if let Some(code) = event_code(&event) {
                        g.input(
                            if down {
                                InputEvent::KeyDown(code)
                            } else {
                                InputEvent::KeyUp(code)
                            },
                            &mut self.audio,
                        );
                    }
                    if down {
                        if let Some(text) = &event.text {
                            for c in text.chars() {
                                g.input(InputEvent::Char(c), &mut self.audio);
                            }
                        }
                    }
                    return;
                }
                if let Some(p) = &mut self.preview {
                    if let Some(code) = event_code(&event) {
                        p.input(if down {
                            InputEvent::KeyDown(code)
                        } else {
                            InputEvent::KeyUp(code)
                        });
                    }
                    if down {
                        if let Some(text) = &event.text {
                            for c in text.chars() {
                                p.input(InputEvent::Char(c));
                            }
                        }
                    }
                    return;
                }
                if let Some(code) = event_code(&event) {
                    if down {
                        if !event.repeat {
                            self.session.key_down(code);
                        }
                    } else {
                        self.session.key_up(code);
                    }
                }
                if down {
                    if let Some(text) = &event.text {
                        for c in text.chars() {
                            self.session.fe.char_typed(c);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(g) = &mut self.game {
                    let (x, y) = self.session.placement.to_canvas(position.x, position.y);
                    self.mouse = (x, y);
                    g.input(InputEvent::MouseMove { x, y }, &mut self.audio);
                    return;
                }
                self.session.mouse_move(position.x, position.y);
                if let Some(p) = &mut self.preview {
                    let (x, y) = self.session.placement.to_canvas(position.x, position.y);
                    p.input(InputEvent::MouseMove { x, y });
                }
            }
            WindowEvent::CursorEntered { .. } => self.session.cursor_visible = true,
            WindowEvent::CursorLeft { .. } => self.session.cursor_visible = false,
            WindowEvent::MouseInput { state, button, .. } => {
                if self.intro.is_some() {
                    return;
                }
                let down = state == ElementState::Pressed;
                if self.loading.is_some() {
                    return;
                }
                if let Some(g) = &mut self.game {
                    let left = match button {
                        MouseButton::Left => true,
                        MouseButton::Right => false,
                        _ => return,
                    };
                    g.input(InputEvent::MouseButton { left, down }, &mut self.audio);
                    return;
                }
                if let Some(p) = &mut self.preview {
                    let left = match button {
                        MouseButton::Left => true,
                        MouseButton::Right => false,
                        _ => return,
                    };
                    p.input(InputEvent::MouseButton { left, down });
                    return;
                }
                match button {
                    MouseButton::Left => self.session.mouse_button(true, down),
                    MouseButton::Right => self.session.mouse_button(false, down),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                if self.intro.is_some() {
                    return;
                }
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                if y != 0.0 {
                    if let Some(g) = &mut self.game {
                        g.input(InputEvent::Wheel { up: y > 0.0 }, &mut self.audio);
                        return;
                    }
                    match &mut self.preview {
                        Some(p) => p.input(InputEvent::Wheel { up: y > 0.0 }),
                        None => self.session.fe.wheel(y > 0.0),
                    }
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(gpu) = &self.gpu {
            gpu.window.request_redraw();
        }
    }
}

/// `Movies\<name>.script` for `<name>.bik`.
pub fn script_name(movie: &str) -> String {
    let stem = movie.strip_suffix(".bik").unwrap_or(movie);
    format!("Movies\\{stem}.script")
}

/// `animBinkDisplay`'s subtitle pass over the movie frame, on the movie clock: every region with
/// text, without the band (rendering is off while `binkPlay` runs).
pub fn draw_movie_subtitles(
    subs: &mut hwc_ui::subtitle::Subtitles,
    canvas: &mut Canvas,
    clock: f32,
) {
    let state = hwc_ui::subtitle::DrawState {
        movie: true,
        ..Default::default()
    };
    subs.resize(canvas.width, canvas.height);
    subs.draw(canvas, clock, None, state);
}
