//! The window: start-up movies, then the front end; winit events in, frames out through wgpu,
//! sound through hwc-audio.

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
use crate::intro::{Cue, Intro, SEQUENCE};
use crate::Args;

/// The display mode the engine switches to for the start-up movies when full screen
/// (`hwSetRes(640, 480, 32)`, falling back to 16 bits).
const INTRO_MODE: (u32, u32) = (640, 480);

struct Gpu {
    presenter: Presenter,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    device: wgpu::Device,
    queue: wgpu::Queue,
    window: Arc<Window>,
}

struct App {
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
    let intro = if args.intro {
        match Intro::new(data, &SEQUENCE) {
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
    let mut app = App {
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
    };
    if app.intro.is_none() {
        app.front_end_start();
    }
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

impl Gpu {
    fn new(window: Arc<Window>, linear: bool) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::LowPower,
            compatible_surface: Some(&surface),
            ..Default::default()
        }))
        .map_err(|e| format!("no GPU adapter: {e}"))?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default()))
                .map_err(|e| format!("no GPU device: {e}"))?;
        let size = window.inner_size();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("surface not supported by the adapter")?;
        // The frame holds final sRGB-encoded pixels: show them without conversion.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
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
        self.presenter.upload(&self.device, &self.queue, canvas);
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
        self.presenter.draw(&mut encoder, &view, viewport);
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
            _ => return None,
        },
        Key::Character(s) => {
            let c = s.chars().next()?.to_ascii_uppercase();
            if c.is_ascii_alphanumeric() {
                c as u16
            } else if c == ' ' {
                key::SPACE
            } else {
                return None;
            }
        }
        _ => return None,
    })
}

impl App {
    /// After the movies (`mainCleanupAfterVideo`): back to the configured display mode, then
    /// the front end and its music.
    fn front_end_start(&mut self) {
        let (w, h) = options_mode(&self.session.fe.game.options);
        self.set_display_mode(w, h);
        if let Some(a) = &mut self.audio {
            a.play_music(audio::FRONT_END_TRACK);
        }
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
        let Some(a) = &mut self.audio else { return };
        for c in cues {
            match c {
                Cue::PlayMusic(t) => a.play_music(t),
                Cue::StopMusic => a.stop_music(0.0),
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        if let Some(a) = &mut self.audio {
            a.update();
        }
        if let Some(intro) = &mut self.intro {
            let cues = intro.update(dt);
            let done = intro.finished();
            self.cues(cues);
            if done {
                self.intro = None;
                self.front_end_start();
            }
        }
        if self.intro.is_none() {
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
                    self.session.scale,
                    (s.width, s.height),
                );
                gpu.present(&self.movie_canvas, placement.viewport);
            }
            None => {
                let viewport = self.session.placement.viewport;
                let canvas = self.session.render();
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
                let what = if tutorial {
                    "training mission"
                } else {
                    "single-player campaign"
                };
                eprintln!("cataclysm: the {what} would start here (difficulty {difficulty}); missions are not playable yet");
            }
            Request::LoadGame(name) => eprintln!("cataclysm: would load saved game {name}"),
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
            .and_then(|w| Gpu::new(Arc::new(w), self.linear));
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
                if let Some(code) = key_code(&event.logical_key) {
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
                self.session.mouse_move(position.x, position.y)
            }
            WindowEvent::CursorEntered { .. } => self.session.cursor_visible = true,
            WindowEvent::CursorLeft { .. } => self.session.cursor_visible = false,
            WindowEvent::MouseInput { state, button, .. } => {
                if self.intro.is_some() {
                    return;
                }
                let down = state == ElementState::Pressed;
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
                    self.session.fe.wheel(y > 0.0);
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
