//! The window: winit event handling and presenting frames through a wgpu surface.

use std::sync::Arc;
use std::time::Instant;

use hwc_render::wgpu;
use hwc_render::Renderer;
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::viewer::Viewer;

// Field order is drop order: GPU objects go before the device-owning renderer and the window.
struct Gpu {
    depth: wgpu::TextureView,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    window: Arc<Window>,
}

// The viewer's GPU resources are dropped before the GPU context.
struct App {
    viewer: Viewer,
    gpu: Option<Gpu>,
    error: Option<String>,
    left: bool,
    right: bool,
    cursor: Option<(f64, f64)>,
    zoom_in_held: bool,
    zoom_out_held: bool,
    shift: bool,
    last_tick: Instant,
    title: String,
}

pub fn run(viewer: Viewer) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| e.to_string())?;
    let mut app = App {
        viewer,
        gpu: None,
        error: None,
        left: false,
        right: false,
        cursor: None,
        zoom_in_held: false,
        zoom_out_held: false,
        shift: false,
        last_tick: Instant::now(),
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
        // The original blends in gamma space, so prefer a non-sRGB swap chain.
        let caps = surface.get_capabilities(&adapter);
        if let Some(f) = caps.formats.iter().find(|f| !f.is_srgb()) {
            config.format = *f;
        }
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        let renderer = Renderer::new(&device, &queue, config.format);
        let depth = renderer.create_depth(config.width, config.height);
        Ok(Self {
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
    fn redraw(&mut self) -> Result<(), String> {
        let Some(gpu) = self.gpu.as_mut() else {
            return Ok(());
        };
        // Held zoom keys act once per 1/60 s, like one game frame.
        let now = Instant::now();
        let ticks = ((now - self.last_tick).as_secs_f32() * 60.0).floor() as u32;
        if ticks > 0 {
            self.last_tick = now;
            for _ in 0..ticks.min(10) {
                if self.zoom_in_held {
                    self.viewer.camera.key_zoom(true, self.viewer.min_zoom);
                }
                if self.zoom_out_held {
                    self.viewer.camera.key_zoom(false, self.viewer.min_zoom);
                }
            }
        }
        if let Err(e) = self.viewer.prepare(&gpu.renderer) {
            eprintln!("{e}");
        }
        let title = self.viewer.title();
        if title != self.title {
            gpu.window.set_title(&title);
            self.title = title;
        }
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
        let mut encoder = gpu
            .renderer
            .device()
            .create_command_encoder(&Default::default());
        let size = (gpu.config.width, gpu.config.height);
        self.viewer
            .render(&mut gpu.renderer, &mut encoder, &view, &gpu.depth, size);
        gpu.renderer.queue().submit([encoder.finish()]);
        gpu.window.pre_present_notify();
        gpu.renderer.queue().present(frame);
        Ok(())
    }

    fn key(&mut self, event_loop: &ActiveEventLoop, key: &Key, pressed: bool) {
        match key {
            Key::Named(NamedKey::Shift) => self.shift = pressed,
            Key::Character(c) if c == "+" || c == "=" => self.zoom_in_held = pressed,
            Key::Character(c) if c == "-" => self.zoom_out_held = pressed,
            _ => {}
        }
        if !pressed {
            return;
        }
        let v = &mut self.viewer;
        match key {
            Key::Named(NamedKey::Escape) => event_loop.exit(),
            Key::Named(NamedKey::ArrowRight) => v.step_ship(1),
            Key::Named(NamedKey::ArrowLeft) => v.step_ship(-1),
            Key::Named(NamedKey::PageDown) => v.step_race(1),
            Key::Named(NamedKey::PageUp) => v.step_race(-1),
            Key::Named(NamedKey::Home) => v.reset_camera(),
            Key::Character(c) => match c.to_ascii_lowercase().as_str() {
                "b" => v.step_background(if self.shift { -1 } else { 1 }),
                "t" => v.step_team(),
                "n" => v.nav_lights = !v.nav_lights,
                "r" => v.reset_camera(),
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
            .with_title("hwc-viewer")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 960.0));
        let result = event_loop
            .create_window(attrs)
            .map_err(|e| e.to_string())
            .and_then(|w| Gpu::new(Arc::new(w)));
        match result {
            Ok(gpu) => {
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
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.error = Some(e);
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
                match button {
                    MouseButton::Left => self.left = down,
                    MouseButton::Right => self.right = down,
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some((x, y)) = self.cursor {
                    let (dx, dy) = ((position.x - x) as f32, (position.y - y) as f32);
                    if self.left || self.right {
                        let both = self.left && self.right;
                        let min = self.viewer.min_zoom;
                        self.viewer.camera.mouse_drag(dx, dy, both, min);
                    }
                }
                self.cursor = Some((position.x, position.y));
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let notches = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 120.0) as f32,
                };
                let min = self.viewer.min_zoom;
                let steps = notches.abs().round().max(1.0) as u32;
                for _ in 0..steps {
                    self.viewer.camera.wheel(notches > 0.0, min);
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
