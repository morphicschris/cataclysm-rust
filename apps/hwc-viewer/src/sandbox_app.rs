//! The sandbox window: input handling and presentation for [`Sandbox`].

use std::sync::Arc;
use std::time::Instant;

use hwc_render::wgpu;
use hwc_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::overlay::OverlayRenderer;
use crate::sandbox::{Mode, Sandbox};

/// Pixels the cursor may move between press and release and still count as a click.
const CLICK_SLOP: f32 = 4.0;

struct Gpu {
    overlay: OverlayRenderer,
    depth: wgpu::TextureView,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    window: Arc<Window>,
}

struct App {
    sandbox: Sandbox,
    gpu: Option<Gpu>,
    error: Option<String>,
    left_down: Option<[f32; 2]>,
    right: bool,
    cursor: Option<[f32; 2]>,
    shift: bool,
    last_frame: Instant,
    started: Instant,
    exit_after: Option<f32>,
    title: String,
}

/// Runs the sandbox window. `exit_after` closes it after that many seconds (for automated
/// captures).
pub fn run(sandbox: Sandbox, exit_after: Option<f32>) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        sandbox,
        gpu: None,
        error: None,
        left_down: None,
        right: false,
        cursor: None,
        shift: false,
        last_frame: Instant::now(),
        started: Instant::now(),
        exit_after,
        title: String::new(),
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

impl Gpu {
    fn new(window: Arc<Window>) -> Result<Self, String> {
        let instance =
            wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let surface = instance
            .create_surface(window.clone())
            .map_err(|e| format!("surface: {e}"))?;
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
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
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, &queue, config.format);
        let depth = renderer.create_depth(config.width, config.height);
        let overlay = OverlayRenderer::new(renderer.device(), config.format);
        Ok(Self {
            overlay,
            depth,
            surface,
            config,
            renderer,
            window,
        })
    }

    fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(self.renderer.device(), &self.config);
        self.depth = self.renderer.create_depth(width, height);
    }
}

impl App {
    fn size(&self) -> (u32, u32) {
        self.gpu
            .as_ref()
            .map_or((1, 1), |g| (g.config.width, g.config.height))
    }

    fn redraw(&mut self) -> Result<(), String> {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f64();
        self.last_frame = now;
        self.sandbox.advance(dt);
        let size = self.size();
        let drag = match (self.left_down, self.cursor) {
            (Some(a), Some(b))
                if self.sandbox.mode == Mode::Select
                    && ((a[0] - b[0]).abs() > CLICK_SLOP || (a[1] - b[1]).abs() > CLICK_SLOP) =>
            {
                Some((a, b))
            }
            _ => None,
        };
        let overlay = self.sandbox.overlay(size, self.cursor, drag);
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        self.sandbox.prepare(&gpu.renderer);
        let title = self.sandbox.title();
        if title != self.title {
            gpu.window.set_title(&title);
            self.title = title;
        }
        let frame = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t)
            | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            _ => {
                let s = gpu.window.inner_size();
                gpu.resize(s.width, s.height);
                return Ok(());
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let device = gpu.renderer.device().clone();
        let queue = gpu.renderer.queue().clone();
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let instances = self.sandbox.instances();
            let scene = self.sandbox.scene(&instances);
            gpu.renderer
                .render(&mut encoder, &view, &gpu.depth, size, &scene);
        }
        gpu.overlay
            .draw(&device, &queue, &mut encoder, &view, size, &overlay);
        queue.submit([encoder.finish()]);
        gpu.window.pre_present_notify();
        queue.present(frame);
        Ok(())
    }

    fn key(&mut self, event_loop: &ActiveEventLoop, key: &Key, pressed: bool) {
        if let Key::Named(NamedKey::Shift) = key {
            self.shift = pressed;
        }
        if !pressed {
            return;
        }
        let sb = &mut self.sandbox;
        match key {
            Key::Named(NamedKey::Escape) => {
                if sb.mode != Mode::Select {
                    sb.mode = Mode::Select;
                } else {
                    event_loop.exit();
                }
            }
            Key::Named(NamedKey::Space) => sb.paused = !sb.paused,
            Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                "m" => {
                    sb.mode = if sb.mode == Mode::Move || sb.selection.is_empty() {
                        Mode::Select
                    } else {
                        Mode::Move
                    };
                }
                "a" => {
                    sb.mode = if sb.mode == Mode::Attack || sb.selection.is_empty() {
                        Mode::Select
                    } else {
                        Mode::Attack
                    };
                }
                "f" => sb.cycle_formation(),
                "h" => sb.halt(),
                "i" => {
                    sb.interpolate = !sb.interpolate;
                    sb.status = format!(
                        "interpolation {}",
                        if sb.interpolate { "on" } else { "off" }
                    );
                }
                "n" => sb.nav_lights = !sb.nav_lights,
                "c" => {
                    sb.focus = if sb.selection.is_empty() {
                        sb.run.universe.ship_list.clone()
                    } else {
                        sb.selection.clone()
                    };
                    sb.update_focus();
                }
                "." => {
                    if sb.paused {
                        sb.step();
                    }
                }
                "+" | "=" => sb.camera.key_zoom(true, sb.camera.closest_zoom),
                "-" => sb.camera.key_zoom(false, sb.camera.closest_zoom),
                _ => {}
            },
            _ => {}
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("hwc-viewer sandbox")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0));
        match event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w)))
        {
            Ok(gpu) => {
                gpu.window.request_redraw();
                self.gpu = Some(gpu);
                self.last_frame = Instant::now();
                self.started = Instant::now();
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
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.error = Some(e);
                    event_loop.exit();
                }
                if self
                    .exit_after
                    .is_some_and(|s| self.started.elapsed().as_secs_f32() > s)
                {
                    event_loop.exit();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                self.key(
                    event_loop,
                    &event.logical_key,
                    event.state == ElementState::Pressed,
                );
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let down = state == ElementState::Pressed;
                let size = self.size();
                match button {
                    MouseButton::Left if down => self.left_down = self.cursor,
                    MouseButton::Left => {
                        let (Some(a), Some(b)) = (self.left_down.take(), self.cursor) else {
                            return;
                        };
                        let sb = &mut self.sandbox;
                        if sb.mode == Mode::Move {
                            if sb.move_to_cursor(size, b) {
                                sb.mode = Mode::Select;
                            }
                        } else if sb.mode == Mode::Attack {
                            if sb.attack_clicked(size, b) {
                                sb.mode = Mode::Select;
                            }
                        } else if (a[0] - b[0]).abs() <= CLICK_SLOP
                            && (a[1] - b[1]).abs() <= CLICK_SLOP
                        {
                            sb.click_select(size, b, self.shift);
                        } else {
                            sb.box_select(size, a, b, self.shift);
                        }
                    }
                    MouseButton::Right => self.right = down,
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let p = [position.x as f32, position.y as f32];
                if let Some(c) = self.cursor {
                    if self.right {
                        let min = self.sandbox.camera.closest_zoom;
                        self.sandbox
                            .camera
                            .mouse_drag(p[0] - c[0], p[1] - c[1], false, min);
                    }
                }
                self.cursor = Some(p);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 120.0) as f32,
                };
                let min = self.sandbox.camera.closest_zoom;
                for _ in 0..(notches.abs().round().max(1.0) as u32) {
                    self.sandbox.camera.wheel(notches > 0.0, min);
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
