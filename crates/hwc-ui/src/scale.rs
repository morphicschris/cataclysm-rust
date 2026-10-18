//! Placing the 640x480 front end in a window.
//!
//! The original never scales its menus: at higher resolutions every screen is drawn 1:1, centred,
//! and the atoms flagged "background" are stretched to the screen edges they touch
//! (`feResRepositionX/Y`, `feResRescaleBackground`). That is [`ScaleMode::Original`] and the
//! default. The modern options scale the 640x480 layout instead.

/// How the front end fills the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScaleMode {
    /// Engine behaviour: layout at native pixels, centred, backgrounds extended.
    #[default]
    Original,
    /// 640x480 scaled uniformly to fit the window, letter-/pillar-boxed.
    Fit,
    /// 640x480 scaled by the largest whole factor that fits (sharp pixels).
    Integer,
    /// 640x480 stretched to the whole window.
    Stretch,
}

impl ScaleMode {
    pub fn parse(s: &str) -> Option<ScaleMode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "original" => ScaleMode::Original,
            "fit" | "aspect" => ScaleMode::Fit,
            "integer" => ScaleMode::Integer,
            "stretch" => ScaleMode::Stretch,
            _ => return None,
        })
    }
}

/// Where the front-end canvas goes in the window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Size of the canvas the front end draws into.
    pub canvas: (i32, i32),
    /// Viewport in window pixels: x, y, width, height.
    pub viewport: (f32, f32, f32, f32),
}

impl Placement {
    pub fn new(mode: ScaleMode, window: (u32, u32)) -> Placement {
        let (ww, wh) = (window.0.max(1) as f32, window.1.max(1) as f32);
        let (cw, ch) = (crate::fe::FE_WIDTH as f32, crate::fe::FE_HEIGHT as f32);
        let fixed = (crate::fe::FE_WIDTH, crate::fe::FE_HEIGHT);
        match mode {
            ScaleMode::Original => Placement {
                canvas: (window.0.max(1) as i32, window.1.max(1) as i32),
                viewport: (0.0, 0.0, ww, wh),
            },
            ScaleMode::Stretch => Placement {
                canvas: fixed,
                viewport: (0.0, 0.0, ww, wh),
            },
            ScaleMode::Fit | ScaleMode::Integer => {
                let mut s = (ww / cw).min(wh / ch);
                if mode == ScaleMode::Integer {
                    s = s.floor().max(1.0);
                }
                let (w, h) = (cw * s, ch * s);
                Placement {
                    canvas: fixed,
                    viewport: (((ww - w) / 2.0).floor(), ((wh - h) / 2.0).floor(), w, h),
                }
            }
        }
    }

    /// Window pixel to canvas pixel.
    pub fn to_canvas(&self, x: f64, y: f64) -> (i32, i32) {
        let (vx, vy, vw, vh) = self.viewport;
        let cx = (x as f32 - vx) * self.canvas.0 as f32 / vw.max(1.0);
        let cy = (y as f32 - vy) * self.canvas.1 as f32 / vh.max(1.0);
        (cx.floor() as i32, cy.floor() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn original_is_one_to_one() {
        let p = Placement::new(ScaleMode::Original, (1024, 768));
        assert_eq!(p.canvas, (1024, 768));
        assert_eq!(p.to_canvas(300.5, 200.0), (300, 200));
    }

    #[test]
    fn fit_letterboxes() {
        let p = Placement::new(ScaleMode::Fit, (1920, 1080));
        assert_eq!(p.canvas, (640, 480));
        assert_eq!(p.viewport, (240.0, 0.0, 1440.0, 1080.0));
        assert_eq!(p.to_canvas(240.0, 0.0), (0, 0));
        assert_eq!(p.to_canvas(1679.0, 1079.0), (639, 479));
        assert!(p.to_canvas(100.0, 10.0).0 < 0);
    }

    #[test]
    fn integer_and_stretch() {
        let p = Placement::new(ScaleMode::Integer, (1920, 1080));
        assert_eq!(p.viewport, (320.0, 60.0, 1280.0, 960.0));
        let s = Placement::new(ScaleMode::Stretch, (800, 300));
        assert_eq!(s.to_canvas(400.0, 150.0), (320, 240));
        assert_eq!(ScaleMode::parse("FIT"), Some(ScaleMode::Fit));
    }
}
