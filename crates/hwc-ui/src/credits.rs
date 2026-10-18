//! The credits slide show (plug screens, `PlugScreen.c`): each `.plug`/`.cred` script shows a
//! JPEG, waits for its time-out and then cross-fades to the next picture (or fades through
//! black) and continues with the next script, until the `gameon` link ends it.

use crate::assets::Resources;
use crate::canvas::{Canvas, Image, Rect, BLACK};

/// Directory of the credits scripts.
const DIR: &str = "Credits\\";
/// Fade length when a script gives none (`PS_FadeTime`).
const FADE_TIME: f32 = 0.5;

#[derive(Debug, Clone, PartialEq, Default)]
struct PlugScript {
    image: Option<String>,
    cross_fade: Option<(f32, String)>,
    timeout: Option<(f32, String)>,
}

fn parse_script(text: &str) -> PlugScript {
    let mut s = PlugScript::default();
    for line in text.lines() {
        let line = line.trim();
        let Some((key, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let rest = rest.trim();
        let pair = || {
            let (t, name) = rest.split_once(',')?;
            Some((t.trim().parse::<f32>().ok()?, name.trim().to_string()))
        };
        match key {
            "Image" => s.image = Some(rest.to_string()),
            "CrossFadeLink" => s.cross_fade = pair(),
            "TimeOut" => s.timeout = pair(),
            _ => {}
        }
    }
    s
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Showing,
    /// Fading to the next picture or to black.
    FadeOut,
    /// Fading in from black.
    FadeIn,
}

/// A running slide show.
pub struct Credits {
    script: PlugScript,
    image: Option<Image>,
    fade_image: Option<Image>,
    fade_time: f32,
    phase: Phase,
    clock: f32,
    pub finished: bool,
}

fn load_jpeg(res: &Resources, name: &str) -> Option<Image> {
    let lang = format!("{DIR}{:?}\\{name}", res.language());
    let data = res
        .read(&lang)
        .or_else(|| res.read(&format!("{DIR}{name}")))?;
    let mut dec = jpeg_decoder::Decoder::new(std::io::Cursor::new(data));
    let pixels = dec.decode().ok()?;
    let info = dec.info()?;
    let (w, h) = (i32::from(info.width), i32::from(info.height));
    let rgba: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => pixels
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        jpeg_decoder::PixelFormat::L8 => pixels.iter().flat_map(|&l| [l, l, l, 255]).collect(),
        _ => return None,
    };
    Some(Image::from_rgba(w, h, &rgba))
}

impl Credits {
    /// `psModeBegin("Credits\\") + psScreenStart(first)`.
    pub fn start(res: &Resources, first: &str) -> Option<Credits> {
        let text = res.read(&format!("{DIR}{first}"))?;
        let script = parse_script(&String::from_utf8_lossy(&text));
        let mut c = Credits {
            script: PlugScript::default(),
            image: None,
            fade_image: None,
            fade_time: FADE_TIME,
            phase: Phase::FadeIn,
            clock: 0.0,
            finished: false,
        };
        c.enter(res, script, true);
        Some(c)
    }

    fn enter(&mut self, res: &Resources, script: PlugScript, from_black: bool) {
        // After a cross-fade the faded-to picture is already loaded.
        self.image = match self.fade_image.take() {
            Some(img) => Some(img),
            None => script.image.as_deref().and_then(|n| load_jpeg(res, n)),
        };
        self.fade_time = FADE_TIME;
        if let Some((t, name)) = &script.cross_fade {
            self.fade_time = *t;
            self.fade_image = load_jpeg(res, name);
        }
        self.script = script;
        self.clock = 0.0;
        self.phase = if from_black {
            Phase::FadeIn
        } else {
            Phase::Showing
        };
    }

    /// Advances the show; `skip` ends it (Escape or a click).
    pub fn update(&mut self, res: &Resources, dt: f32, skip: bool) {
        if self.finished {
            return;
        }
        if skip {
            self.finished = true;
            return;
        }
        self.clock += dt;
        match self.phase {
            Phase::FadeIn => {
                if self.clock >= FADE_TIME {
                    self.phase = Phase::Showing;
                    self.clock = 0.0;
                }
            }
            Phase::Showing => {
                if let Some((t, _)) = &self.script.timeout {
                    if self.clock >= *t {
                        self.phase = Phase::FadeOut;
                        self.clock = 0.0;
                    }
                }
            }
            Phase::FadeOut => {
                if self.clock >= self.fade_time {
                    let next = self
                        .script
                        .timeout
                        .as_ref()
                        .map(|t| t.1.clone())
                        .unwrap_or_default();
                    if next.eq_ignore_ascii_case("gameon") {
                        self.finished = true;
                        return;
                    }
                    let crossed = self.fade_image.is_some();
                    match res.read(&format!("{DIR}{next}")) {
                        Some(t) => {
                            let script = parse_script(&String::from_utf8_lossy(&t));
                            self.enter(res, script, !crossed);
                        }
                        None => self.finished = true,
                    }
                }
            }
        }
    }

    /// Draws the current picture centred on the canvas, with any fade.
    pub fn render(&self, canvas: &mut Canvas) {
        canvas.clear(BLACK);
        let (cw, ch) = (canvas.width, canvas.height);
        let place = |img: &Image| {
            Rect::from_xywh(
                (cw - img.width) / 2,
                (ch - img.height) / 2,
                img.width,
                img.height,
            )
        };
        if let Some(img) = &self.image {
            let r = place(img);
            canvas.blit_scaled(img, r, 255);
        }
        let level = |t: f32, len: f32| ((t / len.max(1e-3)) * 255.0).clamp(0.0, 255.0) as u8;
        let full = Rect::new(0, 0, canvas.width, canvas.height);
        match self.phase {
            Phase::FadeOut => {
                let a = level(self.clock, self.fade_time);
                match &self.fade_image {
                    Some(img) => {
                        let r = place(img);
                        canvas.blit_scaled(img, r, a);
                    }
                    None => canvas.blend_rect(full, [0, 0, 0, a]),
                }
            }
            Phase::FadeIn => {
                let a = 255 - level(self.clock, FADE_TIME);
                canvas.blend_rect(full, [0, 0, 0, a]);
            }
            Phase::Showing => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plug_parsing() {
        let s = parse_script(
            "Image       01_title.jpg\nCrossFadeLink 3.00,02_intro_A.jpg\nGameLink    10,4016,a.lif,b.lif\nTimeOut     03.00,Credits01.plug\nMouse       FALSE\n",
        );
        assert_eq!(s.image.as_deref(), Some("01_title.jpg"));
        assert_eq!(s.cross_fade, Some((3.0, "02_intro_A.jpg".into())));
        assert_eq!(s.timeout, Some((3.0, "Credits01.plug".into())));
    }
}
