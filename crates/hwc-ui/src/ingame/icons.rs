//! Overlay icons (`OverlayIcons\overlayicons.script`): named rectangles of small `.lif` pages
//! (`name  file  x y w h  pageWidth pageHeight`), used by the popup menu, progress bars,
//! sensors scroll arrows, experience stars and the statistics screen.

use std::collections::HashMap;
use std::rc::Rc;

use crate::assets::Resources;
use crate::canvas::{BlitMode, Canvas, Image};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconDef {
    pub file: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

#[derive(Debug, Clone, Default)]
pub struct Icons {
    defs: HashMap<String, IconDef>,
}

impl Icons {
    pub fn parse(text: &str) -> Icons {
        let mut defs = HashMap::new();
        for line in text.lines() {
            let line = line.split(';').next().unwrap_or("");
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() < 6 {
                continue;
            }
            let num = |i: usize| f[i].parse::<i32>().ok();
            let (Some(x), Some(y), Some(w), Some(h)) = (num(2), num(3), num(4), num(5)) else {
                continue;
            };
            defs.insert(
                f[0].to_ascii_lowercase(),
                IconDef {
                    file: f[1].to_string(),
                    x,
                    y,
                    w,
                    h,
                },
            );
        }
        Icons { defs }
    }

    pub fn def(&self, name: &str) -> Option<&IconDef> {
        self.defs.get(&name.to_ascii_lowercase())
    }

    /// The icon's pixels, cut from its page.
    pub fn image(&self, res: &Resources, name: &str) -> Option<Rc<Image>> {
        let d = self.def(name)?.clone();
        let path = format!("OverlayIcons\\{}.lif", d.file);
        let page = res.image(&path)?;
        res.derived_image(
            &format!("{path}#{}", name.to_ascii_lowercase()),
            move || {
                let mut img = Image::new(d.w, d.h, [0, 0, 0, 0]);
                for y in 0..d.h.min(page.height - d.y) {
                    for x in 0..d.w.min(page.width - d.x) {
                        img.pixels[(y * d.w + x) as usize] = page.get(d.x + x, d.y + y);
                    }
                }
                Some(img)
            },
        )
    }

    /// Draws an icon with its top-left corner at (x, y), blended (`primRectTextured` with
    /// alpha blending on).
    pub fn draw(&self, res: &Resources, canvas: &mut Canvas, name: &str, x: i32, y: i32) {
        if let Some(img) = self.image(res, name) {
            canvas.blit(&img, x, y, BlitMode::Blend);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_icon_lines() {
        let i = Icons::parse(
            "PopupMenu_Background\t\tmenu_background\t0 0 4 4 4 4\n\
             SensorsScroll_Up  Sensors_UpDown  0 32 64 16 64 64\n",
        );
        assert_eq!(i.def("popupmenu_background").unwrap().w, 4);
        assert_eq!(i.def("SensorsScroll_Up").unwrap().y, 32);
    }
}
