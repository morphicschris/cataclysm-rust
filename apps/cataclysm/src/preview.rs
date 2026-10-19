//! `--ingame-preview`: the in-game interface on a stand-in fleet (`hwc_ui::ingame::mock`), so
//! the managers, overlay, menus and keys can be tried before missions are playable.

use std::rc::Rc;

use hwc_ui::ingame::mock::MockHost;
use hwc_ui::ingame::{GameRequest, InGameUi, InputEvent, Setup};
use hwc_ui::session::Session;
use hwc_ui::{AssetSource, Canvas};

pub struct Preview {
    pub ui: InGameUi,
    pub host: MockHost,
    pub canvas: Canvas,
    mouse: (i32, i32),
    right_pressed: bool,
}

/// What the application must do after a preview frame.
pub enum Outcome {
    /// Quit to the main menu.
    Leave,
    Sound(u32),
    Speech(u32),
    ApplyOptions,
}

impl Preview {
    pub fn new(source: Rc<dyn AssetSource>, session: &Session) -> Result<Preview, String> {
        let ig = &session.fe.game.ig;
        let ui = InGameUi::new(
            source,
            Setup {
                options: session.fe.game.options.clone(),
                default_keys: Some(ig.default_keys.to_config()),
                keys: Some(ig.keys.to_config()),
                keys_path: ig.keys_path.clone(),
            },
        )?;
        Ok(Preview {
            ui,
            host: MockHost::somtaaw(),
            canvas: Canvas::new(640, 480),
            mouse: (0, 0),
            right_pressed: false,
        })
    }

    pub fn input(&mut self, ev: InputEvent) {
        if let InputEvent::MouseMove { x, y } = ev {
            self.mouse = (x, y);
        }
        let used = self.ui.handle_input(&self.host, ev);
        // A right click the game has no order for opens the menu (`mrRightClickSmartOrder`).
        match ev {
            InputEvent::MouseButton { left: false, down } if !used => {
                if down {
                    self.right_pressed = true;
                } else if self.right_pressed {
                    self.right_pressed = false;
                    self.ui.open_popup(&self.host, false);
                }
            }
            _ => {}
        }
    }

    pub fn update(&mut self, dt: f32) -> Vec<Outcome> {
        let mut out = Vec::new();
        for r in self.ui.update(&self.host, dt) {
            self.host.apply(&r);
            match r {
                GameRequest::QuitToMain => out.push(Outcome::Leave),
                GameRequest::Sound(e) => out.push(Outcome::Sound(e)),
                GameRequest::Speech { event, .. } => out.push(Outcome::Speech(event)),
                GameRequest::ApplyOptions | GameRequest::AudioSettings => {
                    out.push(Outcome::ApplyOptions)
                }
                _ => {}
            }
        }
        self.host.player.paused = self.host.paused;
        if !self.host.paused {
            self.host.tick(dt);
        }
        out
    }

    /// The frame: a starfield standing in for the 3D view, the interface, the pointer.
    pub fn render(&mut self, session: &Session) -> &Canvas {
        let (w, h) = session.placement.canvas;
        if (self.canvas.width, self.canvas.height) != (w, h) {
            self.canvas.resize(w, h);
            self.ui.resize(w, h);
        }
        self.canvas.clear([4, 6, 14, 255]);
        let mut seed = 0x2545_f491u32;
        for _ in 0..(w * h / 700) {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let x = (seed % w.max(1) as u32) as i32;
            let y = ((seed >> 12) % h.max(1) as u32) as i32;
            let b = 110 + (seed >> 24) as u8 % 140;
            self.canvas.put(x, y, [b, b, b, 255]);
        }
        self.ui.draw(&mut self.canvas);
        session.draw_pointer(&mut self.canvas, self.mouse.0, self.mouse.1);
        &self.canvas
    }
}
