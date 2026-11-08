//! The window: start-up movies, then the front end, then (after the loading screen) a
//! single-player mission; winit events in, frames out through wgpu, sound through hwc-audio.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::consts::key;
use hwc_ui::display::{self, Mode, Monitor, Setting};
use hwc_ui::present::{Filter, Presenter};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::Session;
use hwc_ui::Canvas;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, ModifiersState, NamedKey};
use winit::monitor::MonitorHandle;
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
    /// The skirmish last started (the escape menu's restart starts it again).
    last_skirmish: Option<hwc_ui::skirmish::SkirmishRequest>,
    /// The LAN screens' network (the browser, the game being set up).
    lan: crate::lan::LanFront,
    session: Session,
    audio: Option<Audio>,
    intro: Option<Intro>,
    movie_canvas: Canvas,
    gpu: Option<Gpu>,
    error: Option<String>,
    last: Instant,
    /// The display asked for (a modern addition: window, borderless window or exclusive full
    /// screen); once applied, the one in use.
    display: Setting,
    /// The full-screen mode Alt+Enter goes back to from a window.
    last_full: Mode,
    /// The display the options asked for when last looked at (a change there, from the
    /// in-game options, is switched to).
    options_display: Setting,
    /// The monitor the window is on.
    monitor: Option<Monitor>,
    monitor_handle: Option<MonitorHandle>,
    /// Modifier keys held (Alt+Enter).
    modifiers: ModifiersState,
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
    /// `--order-lines`, `--ship-icons`, `--health-bars` (None: the options file decides).
    order_lines: Option<bool>,
    ship_icons: Option<i32>,
    health_brackets: Option<bool>,
    /// `--render-scale`, `--msaa`, `--fxaa`, `--scene-res`, `--zoom` (None: the options file).
    render_scale: Option<f32>,
    msaa: Option<u32>,
    fxaa: Option<bool>,
    scene_at_window: Option<bool>,
    smooth_zoom: Option<bool>,
    /// `--focus-zoom`, `--alt-focus`, `--zoom-limit`, `--draw-distance` (None: the options
    /// file).
    focus_zoom: Option<bool>,
    alt_focus_keeps_zoom: Option<bool>,
    zoom_clear: Option<bool>,
    render_list: Option<bool>,
    /// `--letterbox` (None: the options file).
    tall_letterbox: Option<bool>,
    /// `--sensors-follow`, `--sensors-focus` (None: the options file).
    sensors_follow: Option<bool>,
    sensors_focus_stays: Option<bool>,
    /// `--no-vsync`.
    no_vsync: bool,
    /// `--frame-times`: the 3D frame's draw times, printed every few seconds.
    frame_times: Option<FrameTimes>,
    /// The live scene behind the modern front end.
    backdrop: Backdrop,
}

/// The live scene behind the modern front end (a modern addition): loaded a step a frame
/// once the modern menus are up, faded in when ready, dropped while a game runs.
#[derive(Default)]
struct Backdrop {
    loader: Option<crate::menu_scene::MenuSceneLoader>,
    scene: Option<crate::menu_scene::MenuScene>,
    /// Seconds since the scene became ready (it fades in).
    age: f32,
    /// Loading failed: the plain backdrop stays.
    failed: bool,
}

impl Backdrop {
    /// How long the scene takes to fade in.
    const FADE: f32 = 1.2;

    fn drop_scene(&mut self) {
        self.loader = None;
        self.scene = None;
        self.age = 0.0;
    }
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
    // The options' display, with the command line's mode and size for this run.
    let options_display = Setting::from_options(&session.fe.game.options);
    let mut wanted = options_display;
    if let Some(m) = args.display_mode {
        wanted.mode = m;
    }
    if let Some((w, h)) = args.size {
        wanted.width = w as i32;
        wanted.height = h as i32;
    }
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
        last_skirmish: None,
        lan: Default::default(),
        saves,
        session,
        audio,
        intro,
        movie_canvas: Canvas::new(1, 1),
        gpu: None,
        error: None,
        last: Instant::now(),
        display: wanted,
        last_full: if wanted.mode.is_full() {
            wanted.mode
        } else {
            Mode::Borderless
        },
        options_display,
        monitor: None,
        monitor_handle: None,
        modifiers: ModifiersState::empty(),
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
        order_lines: args.order_lines,
        ship_icons: args.ship_icons,
        health_brackets: args.health_brackets,
        render_scale: args.render_scale,
        msaa: args.msaa,
        fxaa: args.fxaa,
        scene_at_window: args.scene_at_window,
        smooth_zoom: args.smooth_zoom,
        focus_zoom: args.focus_zoom,
        alt_focus_keeps_zoom: args.alt_focus_keeps_zoom,
        zoom_clear: args.zoom_clear,
        render_list: args.render_list,
        tall_letterbox: args.tall_letterbox,
        sensors_follow: args.sensors_follow,
        sensors_focus_stays: args.sensors_focus_stays,
        no_vsync: args.no_vsync,
        frame_times: args.frame_times.then(FrameTimes::default),
        backdrop: Backdrop::default(),
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

/// An exclusive full-screen mode of `size` on a monitor: at the refresh rate asked for
/// (millihertz, 0: any), 32 bits preferred (then 16, like `hwSetRes`), then the monitor's
/// current refresh rate, then the highest.
fn exclusive_mode(monitor: &MonitorHandle, size: (u32, u32), refresh: u32) -> Option<Fullscreen> {
    let current = monitor.refresh_rate_millihertz();
    monitor
        .video_modes()
        .filter(|m| (m.size().width, m.size().height) == size)
        .max_by_key(|m| {
            let r = m.refresh_rate_millihertz();
            (
                refresh > 0 && r.abs_diff(refresh) <= 500,
                m.bit_depth() == 32,
                m.bit_depth(),
                current.is_some_and(|c| r.abs_diff(c) <= 500),
                r,
            )
        })
        .map(Fullscreen::Exclusive)
}

/// What the menus need to know of a monitor: its desktop size and rate and its full-screen
/// modes.
fn monitor_info(m: &MonitorHandle) -> Monitor {
    let size = m.size();
    Monitor {
        size: (size.width as i32, size.height as i32),
        refresh_mhz: m.refresh_rate_millihertz().map_or(0, |r| r as i32),
        modes: m
            .video_modes()
            .map(|v| {
                display::VideoMode::new(
                    v.size().width as i32,
                    v.size().height as i32,
                    v.refresh_rate_millihertz() as i32,
                )
            })
            .collect(),
    }
}

/// The window's monitor (the primary one before it is known).
fn window_monitor(w: &Window) -> Option<MonitorHandle> {
    w.current_monitor().or_else(|| w.primary_monitor())
}

/// Where a window of `size` (its inner area) goes to be centred on a monitor, given the size
/// of its frame (outer less inner).
fn centred(monitor: &MonitorHandle, size: (u32, u32), frame: (u32, u32)) -> PhysicalPosition<i32> {
    let (mp, ms) = (monitor.position(), monitor.size());
    let outer = (size.0 + frame.0, size.1 + frame.1);
    let x = mp.x + (ms.width as i32 - outer.0 as i32).max(0) / 2;
    let y = mp.y + (ms.height as i32 - outer.1 as i32).max(0) / 2;
    PhysicalPosition::new(x, y)
}

/// The movies' place in the window. Stretching (the default, a user-requested change) widens a
/// 4:3 movie at most to 16:9: on wider screens (21:9, 32:9) it is pillarboxed there, not
/// stretched further.
fn movie_viewport(
    placement: hwc_ui::scale::Placement,
    mode: ScaleMode,
    window: (u32, u32),
) -> (f32, f32, f32, f32) {
    let (x, y, w, h) = placement.viewport;
    if mode != ScaleMode::Stretch {
        return (x, y, w, h);
    }
    let widest = h * 16.0 / 9.0;
    if w <= widest {
        return (x, y, w, h);
    }
    let x = ((window.0 as f32 - widest) / 2.0).floor();
    (x, y, widest, h)
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
        // The keypad's own codes (`VK_NUMPAD0`.., `VK_ADD`...). Its digits and point are
        // those only with Num Lock on (the logical key is then a character); read without
        // the modifiers they would be the cursor keys, and the 5 (`VK_CLEAR`) no key at all,
        // so a text entry never saw it.
        let typed = matches!(event.logical_key, Key::Character(_));
        let pad = [
            KeyCode::Numpad0,
            KeyCode::Numpad1,
            KeyCode::Numpad2,
            KeyCode::Numpad3,
            KeyCode::Numpad4,
            KeyCode::Numpad5,
            KeyCode::Numpad6,
            KeyCode::Numpad7,
            KeyCode::Numpad8,
            KeyCode::Numpad9,
        ]
        .iter()
        .position(|&d| d == c);
        match (c, pad) {
            (_, Some(n)) if typed => return Some(0x60 + n as u16),
            (KeyCode::NumpadDecimal, _) if typed => return Some(0x6e),
            (KeyCode::NumpadMultiply, _) => return Some(0x6a),
            (KeyCode::NumpadAdd, _) => return Some(0x6b),
            (KeyCode::NumpadSubtract, _) => return Some(0x6d),
            (KeyCode::NumpadDivide, _) => return Some(0x6f),
            _ => {}
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
            NamedKey::Clear => 0x0c,
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
    order_lines: Option<bool>,
    ship_icons: Option<i32>,
    health_brackets: Option<bool>,
    render_scale: Option<f32>,
    msaa: Option<u32>,
    fxaa: Option<bool>,
    scene_at_window: Option<bool>,
    smooth_zoom: Option<bool>,
    focus_zoom: Option<bool>,
    alt_focus_keeps_zoom: Option<bool>,
    zoom_clear: Option<bool>,
    render_list: Option<bool>,
    tall_letterbox: Option<bool>,
    sensors_follow: Option<bool>,
    sensors_focus_stays: Option<bool>,
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
        if let Some(v) = self.order_lines {
            game.order_lines = v;
        }
        if let Some(v) = self.ship_icons {
            game.ship_icons = crate::tactical::IconMode::from_option(v);
        }
        if let Some(v) = self.health_brackets {
            game.health_brackets = v;
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
        if let Some(v) = self.alt_focus_keeps_zoom {
            game.world.alt_focus_keeps_zoom = v;
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
        if let Some(v) = self.sensors_follow {
            game.sensors.follow_focus = v;
        }
        if let Some(v) = self.sensors_focus_stays {
            game.sensors.focus_stays = v;
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
            ("hwcOrderLines", self.order_lines.map(b)),
            ("hwcShipIcons", self.ship_icons),
            ("hwcHealthBrackets", self.health_brackets.map(b)),
            (
                "hwcRenderScale",
                self.render_scale.map(|s| (s * 100.0).round() as i32),
            ),
            ("hwcMsaa", self.msaa.map(|n| n as i32)),
            ("hwcFxaa", self.fxaa.map(b)),
            ("hwcSceneAtWindow", self.scene_at_window.map(b)),
            ("hwcSmoothZoom", self.smooth_zoom.map(b)),
            ("hwcFocusZoom", self.focus_zoom.map(b)),
            ("hwcAltFocusKeepsZoom", self.alt_focus_keeps_zoom.map(b)),
            ("hwcZoomClear", self.zoom_clear.map(b)),
            ("hwcRenderList", self.render_list.map(b)),
            ("hwcOriginalLetterbox", self.tall_letterbox.map(|t| b(!t))),
            ("hwcSensorsFollow", self.sensors_follow.map(b)),
            ("hwcSensorsFocusStays", self.sensors_focus_stays.map(b)),
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
            order_lines: self.order_lines,
            ship_icons: self.ship_icons,
            health_brackets: self.health_brackets,
            render_scale: self.render_scale,
            msaa: self.msaa,
            fxaa: self.fxaa,
            scene_at_window: self.scene_at_window,
            smooth_zoom: self.smooth_zoom,
            focus_zoom: self.focus_zoom,
            alt_focus_keeps_zoom: self.alt_focus_keeps_zoom,
            zoom_clear: self.zoom_clear,
            render_list: self.render_list,
            tall_letterbox: self.tall_letterbox,
            sensors_follow: self.sensors_follow,
            sensors_focus_stays: self.sensors_focus_stays,
        }
    }

    /// After the movies (`mainCleanupAfterVideo`): back to the configured display mode, then
    /// the front end and its music.
    fn front_end_start(&mut self) {
        self.apply_display(self.display);
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
        self.backdrop.drop_scene();
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

    /// `utyStartSkirmish`: the skirmish loads behind a loading screen.
    fn start_skirmish(&mut self, req: hwc_ui::skirmish::SkirmishRequest) {
        self.backdrop.drop_scene();
        self.session.fe.all_screens_delete();
        let size = self.session.placement.canvas;
        let map = req.map.clone();
        self.last_skirmish = Some(req.clone());
        match Loading::skirmish(self.data.clone(), &self.session, req, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l))
            }
            Err(e) => {
                eprintln!("cataclysm: cannot start a skirmish on {map}: {e}");
                self.back_to_menu();
            }
        }
    }

    /// A network game loads behind the loading screen (the same game on every machine).
    fn start_network_game(&mut self, start: crate::lan::NetStart) {
        self.backdrop.drop_scene();
        self.session.fe.all_screens_delete();
        let size = self.session.placement.canvas;
        let map = start.setup.map.clone();
        match Loading::network(self.data.clone(), &self.session, start, size) {
            Ok(mut l) => {
                l.saves = Some(self.saves.clone());
                self.loading = Some(Box::new(l))
            }
            Err(e) => {
                eprintln!("cataclysm: cannot start the network game on {map}: {e}");
                self.back_to_menu();
            }
        }
    }

    /// `utyLoadMultiSinglePlayerGame` / `gpLoadGame`: a saved game loads behind its mission's
    /// loading screen (the game running, if any, ends first and the music stops).
    fn load_saved_game(&mut self, path: &std::path::Path) {
        self.backdrop.drop_scene();
        self.end_game();
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
        let Some(g) = self.end_game() else { return };
        let run = g.world.run();
        let (mission, skill) = (run.game.config.mission, run.game.config.skill);
        let carry = run.carry();
        // The hot-key groups go with the ships that jumped.
        let groups = carry.as_ref().map(|c| {
            g.hotkeys.carry_out(
                &run.universe,
                &run.game.hyper.in_hyperspace,
                crate::host::player(),
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
        let Some(mut g) = self.end_game() else {
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
        self.end_game();
        if let Some(a) = &mut self.audio {
            a.set_in_game(false);
            a.apply_options(&self.session.fe.game.options);
            a.play_music(audio::FRONT_END_TRACK);
        }
        self.session.fe.all_screens_delete();
        self.session.fe.screen_start(hwc_ui::session::MAIN_SCREEN);
    }

    /// The 3D renderer on the window's device, made the first time it is needed (a mission,
    /// or the scene behind the modern front end).
    fn make_renderer(renderer: &mut Option<hwc_render::Renderer>, gpu: &Gpu) {
        if renderer.is_some() {
            return;
        }
        let mut r =
            hwc_render::Renderer::new(&gpu.device, &gpu.queue, hwc_render::OFFSCREEN_FORMAT);
        r.set_msaa_support(&gpu.msaa_counts);
        if crate::profile::enabled() && !r.set_gpu_timing(true) {
            eprintln!("cataclysm: --profile: no GPU timestamp queries here, CPU times only");
        }
        *renderer = Some(r);
    }

    /// The front end's picture: the modern menus over the live scene (loading it a step a
    /// frame, the plain backdrop until it is ready), else the original screens.
    fn front_end_frame(&mut self, dt: f32) {
        if !self.session.modern() {
            self.backdrop.drop_scene();
            self.session.render();
            return;
        }
        let b = &mut self.backdrop;
        if let (Some(gpu), false) = (self.gpu.as_ref(), b.failed) {
            Self::make_renderer(&mut self.renderer, gpu);
        }
        let Some(r) = self.renderer.as_mut().filter(|_| !b.failed) else {
            self.session.render();
            return;
        };
        if b.scene.is_none() {
            let colors = crate::play::player_colors(&self.session);
            let loader = b.loader.get_or_insert_with(|| {
                crate::menu_scene::MenuSceneLoader::new(self.data.clone(), colors)
            });
            crate::watchdog::checkpoint("front end: scene loading");
            match loader.advance(r) {
                Ok(Some(scene)) => {
                    b.scene = Some(scene);
                    b.loader = None;
                    b.age = 0.0;
                }
                Ok(None) => {}
                Err(e) => {
                    eprintln!("cataclysm: menu scene: {e}");
                    b.failed = true;
                    b.loader = None;
                }
            }
        }
        let Some(scene) = b.scene.as_mut() else {
            self.session.render();
            return;
        };
        // The camera keeps its framing whatever screen is up (the ship right of the main menu).
        scene.set_options(
            &self.session.fe.game.options,
            crate::menu_scene::QualityOverrides {
                scale: self.render_scale,
                msaa: self.msaa,
                fxaa: self.fxaa,
            },
        );
        crate::watchdog::checkpoint("front end: scene");
        scene.update(dt);
        if let Err(e) = scene.render(r, self.session.canvas_mut()) {
            eprintln!("cataclysm: menu scene: {e}");
            b.failed = true;
            b.drop_scene();
            self.session.render();
            return;
        }
        b.age += dt;
        if b.age < Backdrop::FADE {
            let a = (255.0 * (1.0 - b.age / Backdrop::FADE)) as u8;
            let c = self.session.canvas_mut();
            let all = hwc_ui::Rect::new(0, 0, c.width, c.height);
            c.blend_rect(all, [3, 4, 8, a]);
        }
        self.session.render_over_backdrop();
    }

    /// One frame of the loading screen or the mission; true when one of them is up.
    fn game_frame(&mut self, dt: f32) -> Result<bool, String> {
        let size = self.session.placement.canvas;
        if let Some(l) = &mut self.loading {
            let viewport = self.session.placement.viewport;
            if let Some(gpu) = self.gpu.as_mut() {
                gpu.present(l.render(size), viewport);
                Self::make_renderer(&mut self.renderer, gpu);
            }
            crate::watchdog::checkpoint("loading: advance");
            match l.advance(&self.session, self.renderer.as_mut(), dt) {
                Progress::Working => {}
                Progress::Ready(mut game) => {
                    self.loading = None;
                    game.debug_keys |= self.debug_keys;
                    if self.autoplay {
                        game.autoplay = Some(hwc_sim::autoplay::Autoplay::new(16 * 60 * 30));
                    }
                    game.rollcall_below_band = self.rollcall_below_band;
                    game.ui.set_display(self.monitor.clone(), self.display);
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
        let mut display_options = None;
        crate::profile::frame_start();
        let span = crate::profile::span("update");
        crate::watchdog::checkpoint("game: update");
        let outcomes = g.update(dt, &mut self.audio);
        crate::watchdog::checkpoint("game: outcomes");
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
                    display_options = Some(Setting::from_options(&options));
                }
                game::Outcome::Animatic(n) => animatic = Some(n),
                game::Outcome::NextMission => next = true,
                game::Outcome::NisLevelDone => nis_level_done = true,
                game::Outcome::LoadGame(p) => load = Some(p),
            }
        }
        // A screen mode or resolution accepted in the modern options (already in the options
        // file): switched to.
        if let Some(s) = display_options {
            if s != self.options_display {
                self.options_display = s;
                self.apply_display(s);
            }
        }
        if let Some(p) = load {
            self.load_saved_game(&p);
            return Ok(true);
        }
        if nis_level_done {
            crate::watchdog::checkpoint("game: after the NIS level");
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
                match (level_save, mission, self.last_skirmish.clone()) {
                    (_, 0, Some(req)) => self.start_skirmish(req),
                    (Some(p), _, _) => self.load_saved_game(&p),
                    _ => self.start_loading(mission, difficulty),
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
        crate::watchdog::checkpoint("game: render");
        g.render(r, size)?;
        crate::watchdog::checkpoint("game: present");
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

    /// Switches the window to a display (a modern addition): a window of its size, centred on
    /// the monitor, with its frame and resizable; a borderless window over the monitor at its
    /// own resolution; exclusive full screen at its size and refresh rate (a borderless window,
    /// with a note, when the monitor has no such mode). What cannot be used on this monitor
    /// falls back safely ([`Setting::validated`]). The canvas, the interface's layout and the
    /// camera's aspect follow with the resize that comes after.
    fn apply_display(&mut self, wanted: Setting) {
        let Some(gpu) = &self.gpu else {
            self.display = wanted;
            return;
        };
        let w = gpu.window.clone();
        let handle = window_monitor(&w);
        self.monitor = handle.as_ref().map(monitor_info);
        self.monitor_handle = handle.clone();
        let (mut s, note) = wanted.validated(self.monitor.as_ref());
        if let Some(n) = note {
            eprintln!("cataclysm: display: {n}");
        }
        let size = (s.width as u32, s.height as u32);
        match s.mode {
            Mode::Windowed => {
                // The frame's size, from the window as it is (a guess while full screen).
                let frame = if w.fullscreen().is_none() && w.is_decorated() {
                    let (o, i) = (w.outer_size(), w.inner_size());
                    (
                        o.width.saturating_sub(i.width),
                        o.height.saturating_sub(i.height),
                    )
                } else {
                    (16, 39)
                };
                w.set_fullscreen(None);
                w.set_decorations(true);
                w.set_resizable(true);
                let _ = w.request_inner_size(PhysicalSize::new(size.0, size.1));
                if let Some(m) = &handle {
                    w.set_outer_position(centred(m, size, frame));
                }
            }
            Mode::Borderless => w.set_fullscreen(Some(Fullscreen::Borderless(handle.clone()))),
            Mode::Fullscreen => {
                match handle
                    .as_ref()
                    .and_then(|m| exclusive_mode(m, size, s.refresh_mhz.max(0) as u32))
                {
                    Some(fs) => w.set_fullscreen(Some(fs)),
                    None => {
                        eprintln!(
                            "cataclysm: display: no exclusive {}x{} full-screen mode here; a borderless window instead",
                            s.width, s.height
                        );
                        s.mode = Mode::Borderless;
                        w.set_fullscreen(Some(Fullscreen::Borderless(handle.clone())));
                    }
                }
            }
        }
        if s.mode.is_full() {
            self.last_full = s.mode;
        }
        self.display = s;
        self.session.set_display(self.monitor.clone(), s);
        if let Some(g) = self.game.as_mut() {
            g.ui.set_display(self.monitor.clone(), s);
        }
        if let Some(p) = self.preview.as_mut() {
            p.ui.set_display(self.monitor.clone(), s);
        }
        let mode = match s.mode {
            Mode::Windowed => format!("a {}x{} window", s.width, s.height),
            Mode::Borderless => match &self.monitor {
                Some(m) => format!("a borderless {}x{} window", m.size.0, m.size.1),
                None => "a borderless window".into(),
            },
            Mode::Fullscreen => format!(
                "full screen {}x{}{}",
                s.width,
                s.height,
                if s.refresh_mhz > 0 {
                    format!(" at {} Hz", (s.refresh_mhz + 500) / 1000)
                } else {
                    String::new()
                }
            ),
        };
        eprintln!("cataclysm: display: {mode}");
        w.request_redraw();
    }

    /// The options as the player has them now: the mission's interface while a mission is
    /// up (its options screens change them), else the front end's. Everything that keeps an
    /// option writes the options file through this one.
    fn kept_options(&mut self) -> &mut hwc_ui::cataclysm::Cataclysm {
        match self.game.as_mut() {
            Some(g) => &mut g.ui.fe.game,
            None => &mut self.session.fe.game,
        }
    }

    /// Keeps a display in the options file.
    fn remember_display(&mut self, s: Setting) {
        self.options_display = s;
        let mut o = hwc_ui::options::Options::default();
        s.store(&mut o);
        let values = [
            display::MODE_KEY,
            "screenWidth",
            "screenHeight",
            display::REFRESH_KEY,
        ]
        .map(|key| (key, o.get(key)));
        self.kept_options().set_options_now(&values);
    }

    /// The mission ends (`gameEnd`): the options and key bindings its options screens kept
    /// go back to the front end (the next mission starts from them) and are saved
    /// (`opOptionsSaveSettings`).
    fn end_game(&mut self) -> Option<Box<Game>> {
        let mut g = self.game.take()?;
        // A recording ends with the game.
        g.finish_recording();
        if let Some(n) = g.net.as_mut() {
            n.session.leave();
        }
        crate::host::set_player(crate::host::PLAYER);
        hand_back_options(&mut self.session, &g);
        Some(g)
    }

    /// Alt+Enter (and F11): from a window to the last full-screen mode, from full screen to a
    /// window (a modern addition). The choice is kept in the options file.
    fn toggle_window(&mut self) {
        let mut s = self.display;
        if s.mode == Mode::Windowed {
            s.mode = self.last_full;
        } else {
            self.last_full = s.mode;
            s.mode = Mode::Windowed;
        }
        self.remember_display(s);
        self.apply_display(s);
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
        crate::watchdog::frame(self.frames);
        crate::watchdog::checkpoint("redraw: audio");
        if let Some(a) = &mut self.audio {
            a.update();
        }
        crate::watchdog::checkpoint("redraw: frame");
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
            crate::watchdog::checkpoint("front end: update");
            for r in self.session.update(dt) {
                self.request(event_loop, r);
            }
            if let Some(start) = self.lan.update(&mut self.session.fe) {
                self.start_network_game(start);
            }
        }
        if self.gpu.is_none() {
            return Ok(());
        }
        if self.intro.is_none() && self.preview.is_none() {
            self.front_end_frame(dt);
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
                let viewport = movie_viewport(placement, self.movie_scale, (s.width, s.height));
                gpu.present(&self.movie_canvas, viewport);
            }
            None => {
                let viewport = self.session.placement.viewport;
                let canvas = match &mut self.preview {
                    Some(p) => p.render(&self.session),
                    None => &self.session.canvas,
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
            Request::DisplayMode(s) => {
                self.options_display = Setting::from_options(&self.session.fe.game.options);
                self.apply_display(s);
            }
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
            Request::LoadRecordedGame(key) => {
                // `gpLoadTheRecordedGame`: the saved game it starts from, then its orders.
                self.load_saved_game(std::path::Path::new(&key));
                if let Some(l) = self.loading.as_mut() {
                    l.playback = true;
                }
            }
            Request::Multiplayer => eprintln!("cataclysm: multiplayer is not available yet"),
            Request::StartSkirmish(s) => self.start_skirmish(s),
            Request::Lan(r) => {
                if let Some(start) = self.lan.request(&mut self.session.fe, r) {
                    self.start_network_game(start);
                }
            }
            Request::OpenUrl(url) => eprintln!("cataclysm: troubleshooting page: {url}"),
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let _busy = crate::watchdog::enter("resumed");
        if self.gpu.is_some() {
            return;
        }
        // The window starts at its size, centred on the primary monitor; full screen comes
        // right after (a borderless one at once, so nothing shows in a window first).
        let primary = event_loop.primary_monitor();
        let size = (
            self.display.width.max(display::MIN_SIZE.0) as u32,
            self.display.height.max(display::MIN_SIZE.1) as u32,
        );
        let mut attrs = Window::default_attributes()
            .with_title("Homeworld: Cataclysm")
            .with_inner_size(PhysicalSize::new(size.0, size.1))
            .with_resizable(true);
        if let Some(m) = &primary {
            attrs = attrs.with_position(centred(m, size, (16, 39)));
            if self.display.mode == Mode::Borderless {
                attrs = attrs.with_fullscreen(Some(Fullscreen::Borderless(Some(m.clone()))));
            }
        }
        let result = event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w), self.linear, self.no_vsync));
        match result {
            Ok(gpu) => {
                // The engine draws its own pointer.
                gpu.window.set_cursor_visible(false);
                let w = gpu.window.clone();
                self.gpu = Some(gpu);
                let fullscreen_intro =
                    self.intro.is_some() && self.display.mode == Mode::Fullscreen;
                if fullscreen_intro {
                    // Full screen: the intro runs in 640x480 (`hwSetRes`), the configured mode
                    // comes after it.
                    let handle = window_monitor(&w);
                    self.monitor = handle.as_ref().map(monitor_info);
                    self.monitor_handle = handle.clone();
                    let fs = handle
                        .as_ref()
                        .and_then(|m| exclusive_mode(m, INTRO_MODE, 0))
                        .unwrap_or(Fullscreen::Borderless(handle.clone()));
                    w.set_fullscreen(Some(fs));
                    let (s, _) = self.display.validated(self.monitor.as_ref());
                    self.session.set_display(self.monitor.clone(), s);
                } else {
                    self.apply_display(self.display);
                }
                let s = w.inner_size();
                self.session.resize(s.width, s.height);
                w.request_redraw();
            }
            Err(e) => {
                self.error = Some(e);
                event_loop.exit();
            }
        }
    }

    /// Leaving: the options are saved, and out of exclusive full screen first, so the desktop
    /// gets its own mode back at once.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // The options as kept are saved on the way out, whatever the way out.
        self.kept_options().save_options();
        if let Some(gpu) = &self.gpu {
            if matches!(gpu.window.fullscreen(), Some(Fullscreen::Exclusive(_))) {
                gpu.window.set_fullscreen(None);
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let _busy = crate::watchdog::enter(match event {
            WindowEvent::RedrawRequested => "redraw",
            WindowEvent::KeyboardInput { .. } => "key event",
            WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => "mouse event",
            WindowEvent::CursorMoved { .. } => "pointer event",
            WindowEvent::Resized(_) => "resize",
            _ => "window event",
        });
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::ScaleFactorChanged { .. } | WindowEvent::Moved(_) => {
                // Another monitor: its modes for the menus.
                if let Some(gpu) = &self.gpu {
                    let handle = window_monitor(&gpu.window);
                    if handle != self.monitor_handle {
                        self.monitor = handle.as_ref().map(monitor_info);
                        self.monitor_handle = handle;
                        self.session.set_display(self.monitor.clone(), self.display);
                        if let Some(g) = self.game.as_mut() {
                            g.ui.set_display(self.monitor.clone(), self.display);
                        }
                    }
                }
            }
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
                let alt_enter =
                    event.logical_key == Key::Named(NamedKey::Enter) && self.modifiers.alt_key();
                if alt_enter || event.logical_key == Key::Named(NamedKey::F11) {
                    // Not passed on: the menus would take the Return.
                    if down && !event.repeat {
                        self.toggle_window();
                    }
                    return;
                }
                if down && event.logical_key == Key::Named(NamedKey::F10) {
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
                            self.session.char_typed(c);
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
                        None => self.session.wheel(y > 0.0),
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
/// `gameEnd`'s `opOptionsSaveSettings`: the options and key bindings a mission's interface
/// kept become the front end's (the next mission starts from them) and are saved.
pub fn hand_back_options(session: &mut Session, game: &Game) {
    session.fe.game.adopt_kept(&game.ui.fe.game);
    session.fe.game.save_options();
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use hwc_ui::scale::Placement;

    #[test]
    fn movies_stretch_at_most_to_16_9() {
        let at = |mode, w: u32, h: u32| movie_viewport(Placement::new(mode, (w, h)), mode, (w, h));
        // Up to 16:9 the movie fills the window (a user-requested change).
        assert_eq!(
            at(ScaleMode::Stretch, 1920, 1080),
            (0.0, 0.0, 1920.0, 1080.0)
        );
        assert_eq!(at(ScaleMode::Stretch, 1024, 768), (0.0, 0.0, 1024.0, 768.0));
        // Wider: pillarboxed at 16:9, centred.
        assert_eq!(
            at(ScaleMode::Stretch, 5120, 1440),
            (1280.0, 0.0, 2560.0, 1440.0)
        );
        let (x, _, w, h) = at(ScaleMode::Stretch, 3440, 1440);
        assert!((w / h - 16.0 / 9.0).abs() < 1e-3 && (x * 2.0 + w - 3440.0).abs() < 1.0);
        // The other modes are as they were.
        assert_eq!(
            at(ScaleMode::Fit, 5120, 1440),
            Placement::new(ScaleMode::Fit, (5120, 1440)).viewport
        );
    }
}
