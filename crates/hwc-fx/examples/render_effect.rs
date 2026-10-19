//! Render effects headless to PNG frames with a tiny CPU splatter, for eyeballing.
//!
//! `cargo run -p hwc-fx --example render_effect -- [effect.ebg ...]`
//!
//! Reads the game from `HWC_GAME_DIR` (default: the usual install path) or
//! `HWC_EXTRACTED_DIR`, writes `screenshots/fx/<effect>_NNN.png` plus a contact sheet
//! `<effect>_sheet.png`. The camera looks along +Y at the effect's origin with +Z (the
//! effect's line of fire) pointing up the screen; sprites are textured and blended
//! (additive or alpha), lines/bolts/points are drawn as thin strokes, meshes as discs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use hwc_formats::gamedata::GameData;
use hwc_formats::lif::Lif;
use hwc_fx::compiler::{DirSource, FileSource, Library};
use hwc_fx::render::{Blend, RenderList};
use hwc_fx::{load_scripts, FxWorld};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Vector};

const W: usize = 320;
const H: usize = 320;

enum Data {
    Game(GameData),
    Dir(DirSource),
}

impl FileSource for Data {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        match self {
            Data::Game(g) => g.read_file(path),
            Data::Dir(d) => d.read_file(path),
        }
    }
}

struct Texture {
    w: usize,
    h: usize,
    rgba: Vec<u8>,
}

struct Canvas {
    px: Vec<[f32; 3]>,
    eye_dist: f32,
    focal: f32,
}

impl Canvas {
    fn new(eye_dist: f32) -> Self {
        Self { px: vec![[0.0; 3]; W * H], eye_dist, focal: W as f32 * 0.9 }
    }

    /// World -> screen (x right, y down) and the perspective scale at that depth.
    fn project(&self, p: Vector) -> Option<(f32, f32, f32)> {
        let depth = p.y + self.eye_dist;
        if depth <= 1.0 {
            return None;
        }
        let k = self.focal / depth;
        Some((W as f32 / 2.0 + p.x * k, H as f32 / 2.0 - p.z * k, k))
    }

    fn blend(&mut self, x: usize, y: usize, c: [f32; 4], mode: Blend) {
        let d = &mut self.px[y * W + x];
        for i in 0..3 {
            d[i] = match mode {
                Blend::Additive => d[i] + c[i] * c[3],
                Blend::Alpha => d[i] * (1.0 - c[3]) + c[i] * c[3],
                Blend::Opaque => c[i],
            };
        }
    }

    fn dot(&mut self, x: f32, y: f32, r: f32, c: [f32; 4], mode: Blend) {
        let r = r.max(0.5);
        let (x0, x1) = ((x - r).floor().max(0.0) as i32, (x + r).ceil().min(W as f32 - 1.0) as i32);
        let (y0, y1) = ((y - r).floor().max(0.0) as i32, (y + r).ceil().min(H as f32 - 1.0) as i32);
        for py in y0..=y1 {
            for px in x0..=x1 {
                let (dx, dy) = (px as f32 + 0.5 - x, py as f32 + 0.5 - y);
                if dx * dx + dy * dy <= r * r {
                    self.blend(px as usize, py as usize, c, mode);
                }
            }
        }
    }

    fn line(&mut self, a: (f32, f32), b: (f32, f32), c: [f32; 4], mode: Blend) {
        let n = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).clamp(1, 4000);
        for i in 0..=n {
            let t = i as f32 / n as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            if x >= 0.0 && y >= 0.0 && (x as usize) < W && (y as usize) < H {
                self.blend(x as usize, y as usize, c, mode);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sprite(&mut self, x: f32, y: f32, half: (f32, f32), rot: f32, tex: Option<&Texture>, uv: [[u32; 2]; 2], c: [f32; 4], mode: Blend) {
        let Some(t) = tex else {
            self.dot(x, y, half.0 * 2.0, c, mode);
            return;
        };
        let r = (half.0.max(half.1) * 1.5).max(0.5);
        let (cs, sn) = (rot.cos(), rot.sin());
        let (x0, x1) = ((x - r).floor().max(0.0) as i32, (x + r).ceil().min(W as f32 - 1.0) as i32);
        let (y0, y1) = ((y - r).floor().max(0.0) as i32, (y + r).ceil().min(H as f32 - 1.0) as i32);
        for py in y0..=y1 {
            for px in x0..=x1 {
                let (dx, dy) = (px as f32 + 0.5 - x, -(py as f32 + 0.5 - y));
                let (lx, ly) = (dx * cs + dy * sn, -dx * sn + dy * cs);
                let (u, v) = (lx / (2.0 * half.0) + 0.5, ly / (2.0 * half.1) + 0.5);
                if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
                    continue;
                }
                let tu = uv[0][0] as f32 + u * (uv[1][0] as f32 - uv[0][0] as f32);
                let tv = uv[0][1] as f32 + v * (uv[1][1] as f32 - uv[0][1] as f32);
                let (tx, ty) = ((tu as usize).min(t.w - 1), (tv as usize).min(t.h - 1));
                let s = &t.rgba[(ty * t.w + tx) * 4..(ty * t.w + tx) * 4 + 4];
                let k = |i: usize| f32::from(s[i]) / 255.0;
                let col = [c[0] * k(0), c[1] * k(1), c[2] * k(2), c[3] * k(3)];
                self.blend(px as usize, py as usize, col, if mode == Blend::Opaque { Blend::Alpha } else { mode });
            }
        }
    }

    fn draw(&mut self, list: &RenderList, textures: &mut HashMap<String, Option<Texture>>, data: &Data) {
        for s in &list.sprites {
            let Some((x, y, k)) = self.project(s.position) else { continue };
            let tex = s.texture.as_ref().and_then(|name| {
                textures
                    .entry(name.clone())
                    .or_insert_with(|| {
                        let lif = Lif::parse(&data.read_file(&format!("{name}.lif"))?).ok()?;
                        Some(Texture { w: lif.width as usize, h: lif.height as usize, rgba: lif.to_rgba() })
                    })
                    .as_ref()
            });
            let half = (s.size[0] * 0.5 * k, s.size[1] * 0.5 * k);
            self.sprite(x, y, half, s.rotation, tex, s.uv, s.color, s.blend);
        }
        for l in &list.lines {
            if let (Some(a), Some(b)) = (self.project(l.start), self.project(l.end)) {
                self.line((a.0, a.1), (b.0, b.1), l.color, Blend::Additive);
            }
        }
        for b in &list.bolts {
            let pts: Vec<_> = b.points.iter().filter_map(|p| self.project(*p)).collect();
            for w in pts.windows(2) {
                self.line((w[0].0, w[0].1), (w[1].0, w[1].1), b.color, Blend::Additive);
            }
        }
        for p in &list.points {
            if let Some((x, y, _)) = self.project(p.position) {
                self.dot(x, y, p.size * 0.5, p.color, Blend::Additive);
            }
        }
        for m in &list.meshes {
            if let Some((x, y, k)) = self.project(m.position) {
                let size = m.transform.m[0].abs().max(m.transform.m[5].abs()).max(1.0);
                self.dot(x, y, (size * k * 0.5).clamp(1.0, 40.0), [m.color[0], m.color[1], m.color[2], 0.6], Blend::Alpha);
            }
        }
        for f in &list.flares {
            if let Some((x, y, _)) = self.project(f.position) {
                self.dot(x, y, 6.0, [1.0, 1.0, 0.8, 0.8], Blend::Additive);
            }
        }
    }

    fn rgb(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0) as u8)).collect()
    }
}

fn save_png(path: &Path, w: usize, h: usize, rgb: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}

fn extent(list: &RenderList) -> f32 {
    let mut m: f32 = 0.0;
    let mut acc = |p: Vector, r: f32| m = m.max(p.x.abs() + r).max(p.z.abs() + r);
    for s in &list.sprites {
        acc(s.position, s.size[0] * 0.5);
    }
    for l in &list.lines {
        acc(l.start, 0.0);
        acc(l.end, 0.0);
    }
    for b in &list.bolts {
        for p in &b.points {
            acc(*p, 0.0);
        }
    }
    for p in &list.points {
        acc(p.position, 0.0);
    }
    for x in &list.meshes {
        acc(x.position, 0.0);
    }
    m
}

fn render(lib: &Library, data: &Data, name: &str, frames: usize, out_dir: &Path) {
    // First pass: find the extent so the camera frames the whole effect.
    let mut lists = Vec::new();
    let mut world = FxWorld::new(lib.clone());
    if world.spawn_named(name, Vector::ZERO, Matrix::IDENTITY, &[]).is_none() {
        eprintln!("{name}: not found");
        return;
    }
    let mut reach: f32 = 1.0;
    for _ in 0..frames {
        let mut list = RenderList::default();
        world.step(UNIVERSE_UPDATE_PERIOD, Some(&mut list));
        reach = reach.max(extent(&list));
        lists.push(list);
    }
    let eye = reach * 2.4;
    let mut textures = HashMap::new();
    let stem = name.trim_end_matches(".ebg").trim_end_matches(".EBG");
    let cols = 8;
    let thumb = 2; // downsample factor for the sheet
    let rows = frames.div_ceil(cols);
    let (sw, sh) = (cols * W / thumb, rows * H / thumb);
    let mut sheet = vec![0u8; sw * sh * 3];
    for (i, list) in lists.iter().enumerate() {
        let mut c = Canvas::new(eye);
        c.draw(list, &mut textures, data);
        let rgb = c.rgb();
        save_png(&out_dir.join(format!("{stem}_{i:03}.png")), W, H, &rgb);
        let (ox, oy) = ((i % cols) * W / thumb, (i / cols) * H / thumb);
        for y in 0..H / thumb {
            for x in 0..W / thumb {
                let s = ((y * thumb) * W + x * thumb) * 3;
                let d = ((oy + y) * sw + ox + x) * 3;
                sheet[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
            }
        }
    }
    save_png(&out_dir.join(format!("{stem}_sheet.png")), sw, sh, &sheet);
    eprintln!("{name}: {frames} frames, view radius {reach:.0}");
}

fn main() {
    let data = match std::env::var_os("HWC_EXTRACTED_DIR") {
        Some(d) => Data::Dir(DirSource { root: d.into() }),
        None => {
            let dir = std::env::var("HWC_GAME_DIR").unwrap_or_else(|_| r"D:\Games\Homeworld Cataclysm\Cataclysm".into());
            Data::Game(GameData::open(dir).expect("open game data"))
        }
    };
    let mut lib = Library::new();
    let load = load_scripts(&mut lib, &data);
    for e in &load.errors {
        eprintln!("compile error: {e}");
    }
    let mut names: Vec<String> = std::env::args().skip(1).collect();
    if names.is_empty() {
        names = ["DestDamageSmall.ebg", "bigdeath.ebg", "HitEnergy.ebg", "FireEnergySmall01.ebg", "eboltCascade.ebg", "afterburner.ebg"]
            .iter()
            .map(|s| s.to_string())
            .collect();
    }
    for n in &names {
        if lib.get(n).is_none() {
            if let Err(e) = lib.load_effect(&data, &format!("ETG\\{n}")) {
                eprintln!("{e}");
            }
        }
    }
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/fx");
    std::fs::create_dir_all(&out).unwrap();
    for n in &names {
        render(&lib, &data, n, 48, &out);
    }
}
