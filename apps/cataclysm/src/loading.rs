//! The single-player loading screen (`HorseRace.c`: `horseRaceInit`, `hrInitBackground`,
//! `hrChooseSinglePlayerBitmap`, `hrDrawPlayersProgress`, `horseRaceRender`).
//!
//! Between the menu and a mission the engine shows the `Horse_Race_Single` screen of
//! `FEMAN\Horse_Race.fib` over the mission's `SinglePlayer\MissionNN\loading.jpg`, resampled
//! to 640x480 and drawn unscaled in the middle of the screen (black around it). The
//! progress bar is a solid 152x3 rectangle at (243, 234) of the 640x480 layout in the colour
//! of the mission's `loading.script` (`r g b`, default 255,63,63), filled to the loading
//! fraction; the skill level ("Skill Level NORMAL") is printed in white `Arial_12` at
//! (10, height - 40). The pointer is hidden while loading.

use std::rc::Rc;

use hwc_ui::assets::{AssetSource, Language, Resources};
use hwc_ui::canvas::{BlitMode, Canvas, Color, Image, Rect};
use hwc_ui::fe::FrontEnd;
use hwc_ui::fer::Fer;
use hwc_ui::region::AtomRef;
use hwc_ui::text::{self, Shadow};

/// Layout position and size of the progress bar (`feResRepositionX(0xf3)`, `Y(0xea)`, width
/// `0x98`, height 3).
const BAR: (i32, i32, i32, i32) = (243, 234, 152, 3);
/// Bar colour when the mission has no `loading.script`.
const DEFAULT_BAR: Color = [255, 63, 63, 255];
const SKILL_KEYS: [&str; 5] = [
    "strSkillVeryEasy",
    "strSkillEasy",
    "strSkillNormal",
    "strSkillHard",
    "strSkillVeryHard",
];

/// What the screen's draw callback reads.
pub struct LoadState {
    pub progress: f32,
    pub bar_color: Color,
    pub skill_text: String,
    font: Option<Rc<hwc_formats::font::Font>>,
}

pub struct LoadingScreen {
    fe: FrontEnd<LoadState>,
    background: Option<Image>,
    pub canvas: Canvas,
}

/// Decodes a JPEG into RGBA.
pub fn decode_jpeg(data: &[u8]) -> Option<Image> {
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

/// `hrInitBackground`'s resampling to 640x480: point sampling when the picture is at least
/// that big in either direction, bilinear interpolation (`hrGetInterpPixel`) otherwise.
pub fn resample_640x480(src: &Image) -> Image {
    let (dw, dh) = (640, 480);
    if (src.width, src.height) == (dw, dh) {
        return src.clone();
    }
    let sx = src.width as f32 / dw as f32;
    let sy = src.height as f32 / dh as f32;
    let mut out = Image::new(dw, dh, [0, 0, 0, 255]);
    let big = sx >= 1.0 || sy >= 1.0;
    for y in 0..dh {
        for x in 0..dw {
            let fx = x as f32 * sx;
            let fy = y as f32 * sy;
            let c = if big {
                let px = (fx as i32).min(src.width - 1);
                let py = (fy as i32).min(src.height - 1);
                src.get(px, py)
            } else {
                let x0 = (fx as i32).min(src.width - 1);
                let y0 = (fy as i32).min(src.height - 1);
                let x1 = (x0 + 1).min(src.width - 1);
                let y1 = (y0 + 1).min(src.height - 1);
                let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
                let mut c = [0u8; 4];
                for (i, ch) in c.iter_mut().enumerate() {
                    let a = f32::from(src.get(x0, y0)[i]);
                    let b = f32::from(src.get(x1, y0)[i]);
                    let d = f32::from(src.get(x0, y1)[i]);
                    let e = f32::from(src.get(x1, y1)[i]);
                    let top = a + (b - a) * tx;
                    let bottom = d + (e - d) * tx;
                    *ch = (top + (bottom - top) * ty) as u8;
                }
                c
            };
            out.pixels[(y * dw + x) as usize] = [c[0], c[1], c[2], 255];
        }
    }
    out
}

/// `loading.script`: `r g b` on the first line.
pub fn parse_bar_color(text: &str) -> Option<Color> {
    let mut it = text
        .lines()
        .next()?
        .split_whitespace()
        .map(|v| v.parse::<i32>().ok());
    let r = it.next()??;
    let g = it.next()??;
    let b = it.next()??;
    Some([r as u8, g as u8, b as u8, 255])
}

/// `hrDrawPlayersProgress` in single player.
fn draw_progress(fe: &mut FrontEnd<LoadState>, fer: &mut Fer, _atom: AtomRef, _rect: Rect) {
    let (w, h) = (fer.canvas.width, fer.canvas.height);
    let (ox, oy) = ((w - 640) / 2, (h - 480) / 2);
    let st = &fe.game;
    let f = st.progress.clamp(0.0, 1.0);
    let x0 = BAR.0 + ox;
    let y0 = BAR.1 + oy;
    let x1 = x0 + (BAR.2 as f32 * f) as i32;
    fer.canvas
        .fill_rect(Rect::new(x0, y0, x1, y0 + BAR.3), st.bar_color);
    if let Some(font) = &st.font {
        let bytes = text::to_latin1(&st.skill_text);
        text::print(
            fer.canvas,
            font,
            10,
            h - 40,
            [255, 255, 255, 255],
            &bytes,
            (Shadow::None, [0, 0, 0, 255]),
        );
    }
}

fn read_text(source: &dyn AssetSource, name: &str) -> Option<String> {
    source
        .read(name)
        .map(|d| String::from_utf8_lossy(&d).into_owned())
}

impl LoadingScreen {
    /// `horseRaceInit` + `hrInitBackground` for mission `mission` at `difficulty` (0..4).
    pub fn new(
        source: Rc<dyn AssetSource>,
        language: u32,
        mission: u32,
        difficulty: i32,
        size: (i32, i32),
    ) -> Result<Self, String> {
        Self::for_level(source, language, mission, None, difficulty, size)
    }

    /// The loading screen of mission `mission`'s level, or with `nis` of its NIS level
    /// (`hrChooseSinglePlayerBitmap`: `SinglePlayer\missionNN_NisKK\loading.jpg`, no picture
    /// when there is none; the bar colour is the mission's either way).
    pub fn for_level(
        source: Rc<dyn AssetSource>,
        language: u32,
        mission: u32,
        nis: Option<u32>,
        difficulty: i32,
        size: (i32, i32),
    ) -> Result<Self, String> {
        let mut res = Resources::new(source.clone());
        res.set_language(Language::from_index(language));
        let lang_file = format!("{:?}.script", res.language()).to_ascii_lowercase();
        let strings = read_text(&*source, &lang_file)
            .map(|t| hwc_ui::ingame::strings::Strings::parse(&t))
            .unwrap_or_default();
        let skill_text = SKILL_KEYS
            .get(difficulty.clamp(0, 4) as usize)
            .and_then(|k| strings.get(k))
            .unwrap_or("")
            .to_string();
        let font = res.font("Arial_12.hff");
        let dir = format!("SinglePlayer\\mission{mission:02}");
        let picture = match nis {
            Some(n) => format!("SinglePlayer\\mission{mission:02}_Nis{n:02}\\loading.jpg"),
            None => format!("{dir}\\loading.jpg"),
        };
        let background = source
            .read(&picture)
            .and_then(|d| decode_jpeg(&d))
            .map(|i| resample_640x480(&i));
        let bar_color = read_text(&*source, &format!("{dir}\\loading.script"))
            .and_then(|t| parse_bar_color(&t))
            .unwrap_or(DEFAULT_BAR);
        let state = LoadState {
            progress: 0.0,
            bar_color,
            skill_text,
            font,
        };
        let mut fe = FrontEnd::new(state, Rc::new(res));
        fe.draw_callback_add("HR_DrawPlayersProgress", draw_progress);
        let fib = source
            .read("FEMAN\\Horse_Race.fib")
            .ok_or("FEMAN\\Horse_Race.fib not found")?;
        fe.load_fib_data(&fib)?;
        fe.set_screen_size(size.0, size.1);
        fe.screen_start("Horse_Race_Single");
        Ok(Self {
            fe,
            background,
            canvas: Canvas::new(size.0, size.1),
        })
    }

    pub fn set_progress(&mut self, f: f32) {
        self.fe.game.progress = f;
    }

    /// A note in the middle of the screen (the mission cannot be started).
    pub fn note(&mut self, s: &str) {
        let Some(font) = self.fe.game.font.clone() else {
            return;
        };
        let bytes = text::to_latin1(s);
        let w = text::width(&font, &bytes);
        let (cw, ch) = (self.canvas.width, self.canvas.height);
        let r = Rect::new((cw - w) / 2 - 8, ch / 2 - 8, (cw + w) / 2 + 8, ch / 2 + 20);
        self.canvas.blend_rect(r, [0, 0, 0, 192]);
        text::print(
            &mut self.canvas,
            &font,
            (cw - w) / 2,
            ch / 2,
            [255, 255, 255, 255],
            &bytes,
            (Shadow::None, [0, 0, 0, 255]),
        );
    }

    /// `horseRaceRender`: black, the picture in the middle, the screen and the bar.
    pub fn render(&mut self, size: (i32, i32)) -> &Canvas {
        if (self.canvas.width, self.canvas.height) != size {
            self.canvas.resize(size.0, size.1);
            self.fe.set_screen_size(size.0, size.1);
        }
        self.canvas.clear([0, 0, 0, 255]);
        if let Some(bg) = &self.background {
            let x = (size.0 - bg.width) / 2;
            let y = (size.1 - bg.height) / 2;
            self.canvas.blit(bg, x, y, BlitMode::Opaque);
        }
        self.fe.render_over(&mut self.canvas);
        &self.canvas
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_colour_from_script() {
        assert_eq!(parse_bar_color("10 200 30\n"), Some([10, 200, 30, 255]));
        assert_eq!(parse_bar_color("junk"), None);
    }

    #[test]
    fn resample_keeps_640x480_and_scales_others() {
        let img = Image::new(640, 480, [1, 2, 3, 255]);
        assert_eq!(resample_640x480(&img), img);
        let small = Image::new(320, 240, [10, 20, 30, 255]);
        let r = resample_640x480(&small);
        assert_eq!((r.width, r.height), (640, 480));
        assert_eq!(r.get(100, 100), [10, 20, 30, 255]);
    }
}
