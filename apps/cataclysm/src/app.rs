//! The window: winit events in, the front end's frame out through wgpu.

use std::sync::Arc;
use std::time::Instant;

use hwc_ui::cataclysm::Request;
use hwc_ui::consts::key;
use hwc_ui::present::{Filter, Presenter};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::Session;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Fullscreen, Window, WindowId};

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
    gpu: Option<Gpu>,
    error: Option<String>,
    last: Instant,
    size: (u32, u32),
    linear: bool,
    started: Instant,
    exit_after: Option<f32>,
    frames: u64,
}

pub fn run(
    session: Session,
    size: Option<(u32, u32)>,
    linear: bool,
    exit_after: Option<f32>,
) -> Result<(), String> {
    let o = &session.fe.game.options;
    let size = size.unwrap_or((
        o.get("screenWidth").max(320) as u32,
        o.get("screenHeight").max(240) as u32,
    ));
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        session,
        gpu: None,
        error: None,
        last: Instant::now(),
        size,
        linear,
        started: Instant::now(),
        exit_after,
        frames: 0,
    };
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
    fn redraw(&mut self, event_loop: &ActiveEventLoop) -> Result<(), String> {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.25);
        self.last = now;
        for r in self.session.update(dt) {
            self.request(event_loop, r);
        }
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        let canvas = self.session.render();
        gpu.presenter.upload(&gpu.device, &gpu.queue, canvas);
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                let size = gpu.window.inner_size();
                gpu.resize(size.width, size.height);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        gpu.presenter
            .draw(&mut encoder, &view, self.session.placement.viewport);
        gpu.queue.submit([encoder.finish()]);
        gpu.window.pre_present_notify();
        gpu.queue.present(frame);
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
            Request::Quit => event_loop.exit(),
            Request::ApplyOptions => {
                let o = &self.session.fe.game.options;
                let (w, h) = (
                    o.get("screenWidth").max(320) as u32,
                    o.get("screenHeight").max(240) as u32,
                );
                if let Some(gpu) = &self.gpu {
                    if gpu.window.fullscreen().is_none() {
                        let _ = gpu
                            .window
                            .request_inner_size(winit::dpi::PhysicalSize::new(w, h));
                    }
                }
            }
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
            Request::PlaySound(s) => eprintln!("cataclysm: ({s}: no audio yet)"),
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
            .with_inner_size(winit::dpi::PhysicalSize::new(self.size.0, self.size.1));
        let result = event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w), self.linear));
        match result {
            Ok(gpu) => {
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
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                match button {
                    MouseButton::Left => self.session.mouse_button(true, down),
                    MouseButton::Right => self.session.mouse_button(false, down),
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
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
