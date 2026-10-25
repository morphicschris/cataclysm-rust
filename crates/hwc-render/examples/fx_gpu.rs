//! Render ETG effects on the GPU with [`hwc_render::EffectRenderer`] to PNG frames.
//!
//! `cargo run -p hwc-render --example fx_gpu -- [effect.ebg ...]`
//!
//! Data comes from `HWC_EXTRACTED_DIR` (default: the repository's `extracted` folder) or, when
//! that does not exist, `cataclysm.big` in `HWC_GAME_DIR`. Writes
//! `screenshots/fx_gpu/<effect>_NNN.png` and a contact sheet `<effect>_sheet.png`, framed like
//! the CPU sheets of `hwc-fx`'s `render_effect` example (camera on -Y looking at the effect's
//! origin, +Z up the screen) so the two can be compared side by side.

use std::path::{Path, PathBuf};

use hwc_fx::compiler::{FileSource, Library};
use hwc_fx::{load_scripts, FxWorld, RenderList};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Vector};
use hwc_render::{
    headless_device, AssetSource, CameraTweaks, DirSource, EffectRenderer, FxView,
    OffscreenTarget, OrbitCamera, Renderer, Scene, SceneExtras, SceneLighting, OFFSCREEN_FORMAT,
};

const W: u32 = 320;
const H: u32 = 320;
const FRAMES: usize = 48;

enum Data {
    Dir(DirSource),
    Big(hwc_formats::big::BigArchive),
}

impl Data {
    fn src(&self) -> &dyn AssetSource {
        match self {
            Data::Dir(d) => d,
            Data::Big(b) => b,
        }
    }
}

impl FileSource for Data {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.src().read(path)
    }
}

fn open_data() -> Data {
    let dir = std::env::var_os("HWC_EXTRACTED_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../extracted"));
    if dir.join("etg").is_dir() {
        return Data::Dir(DirSource::new(dir));
    }
    let game = hwc_formats::game_dir().expect("set HWC_EXTRACTED_DIR or HWC_GAME_DIR");
    Data::Big(hwc_formats::big::BigArchive::open(game.join("cataclysm.big")).expect("cataclysm.big"))
}

fn save_png(path: &Path, w: u32, h: u32, rgb: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgb).unwrap();
}

/// Largest |x| or |z| reached (same framing rule as the CPU example).
fn extent(list: &RenderList) -> f32 {
    let mut m: f32 = 0.0;
    let mut acc = |p: Vector, r: f32| m = m.max(p.x.abs() + r).max(p.z.abs() + r);
    list.sprites.iter().for_each(|s| acc(s.position, s.size[0] * 0.5));
    list.lines.iter().for_each(|l| {
        acc(l.start, 0.0);
        acc(l.end, 0.0)
    });
    list.bolts.iter().flat_map(|b| &b.points).for_each(|p| acc(*p, 0.0));
    list.points.iter().for_each(|p| acc(p.position, 0.0));
    list.meshes.iter().for_each(|x| acc(x.position, 0.0));
    m
}

fn main() {
    let data = open_data();
    let mut lib = Library::new();
    let load = load_scripts(&mut lib, &data);
    for e in &load.errors {
        eprintln!("compile error: {e}");
    }
    let mut names: Vec<String> = std::env::args().skip(1).collect();
    if names.is_empty() {
        names = [
            "bigdeath.ebg",
            "FireEnergySmall01.ebg",
            "biggunmuzzleflash.ebg@12@0.6",
            "eboltCascade.ebg",
            "HyperWindow.ebg",
            "ElectroMagneticPulse.ebg",
        ]
        .map(String::from)
        .to_vec();
    }
    for n in &names {
        let n = n.split('@').next().unwrap_or(n);
        if lib.get(n).is_none() {
            if let Err(e) = lib.load_effect(&data, &format!("ETG\\{n}")) {
                eprintln!("{n}: {e}");
            }
        }
    }

    let (device, queue) = match headless_device() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("no GPU: {e}");
            return;
        }
    };
    let mut renderer = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
    let target = OffscreenTarget::new(&renderer, W, H);
    let mut fx = EffectRenderer::new(&renderer);
    if !fx.load_lens_flares(&renderer, data.src(), "ps-50-300mm-zoom") {
        eprintln!("lens flare script not found");
    }
    let lighting = SceneLighting::default();
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/fx_gpu");
    std::fs::create_dir_all(&out).unwrap();

    for arg in &names {
        // `effect.ebg@seconds` simulates that long and samples FRAMES frames evenly.
        // `effect.ebg@seconds@shift` also moves the effect off the screen centre by `shift`
        // (a fraction of the half screen), to show lens flares spread out.
        let mut parts = arg.split('@');
        let name = parts.next().unwrap_or(arg);
        let seconds = parts.next().and_then(|t| t.parse::<f32>().ok()).unwrap_or(3.0);
        let shift = parts.next().and_then(|t| t.parse::<f32>().ok()).unwrap_or(0.0);
        let steps = ((seconds / UNIVERSE_UPDATE_PERIOD).round() as usize).max(FRAMES);
        let stride = steps / FRAMES;
        // First pass: the extent, to frame the whole effect as the CPU sheets do.
        let mut world = FxWorld::new(lib.clone());
        if world.spawn_named(name, Vector::ZERO, Matrix::IDENTITY, &[]).is_none() {
            eprintln!("{name}: not found");
            continue;
        }
        let mut reach: f32 = 1.0;
        for _ in 0..stride * FRAMES {
            let mut list = RenderList::default();
            world.step(UNIVERSE_UPDATE_PERIOD, Some(&mut list));
            reach = reach.max(extent(&list));
            // Meshes: their transformed vertices.
            for m in &list.meshes {
                let Some(mesh) = m.mesh.as_deref().and_then(|n| fx.load_mesh(data.src(), n)) else {
                    continue;
                };
                let xf = glam::Mat4::from_cols_array(&m.model_matrix(Vector::new(0.0, -1e5, 0.0)).m);
                for p in &mesh.positions {
                    let w = xf.transform_point3(*p);
                    reach = reach.max(w.x.abs()).max(w.z.abs());
                }
            }
        }
        let eye_dist = reach * 2.4;
        // The CPU canvas uses a focal length of 0.9 x width.
        let fov = 2.0 * (0.5f32 / 0.9).atan();
        let view = glam::Mat4::look_at_rh(
            glam::Vec3::new(0.0, -eye_dist, 0.0),
            glam::Vec3::ZERO,
            glam::Vec3::Z,
        );
        let near = (eye_dist * 0.01).max(0.1);
        let proj = glam::Mat4::perspective_rh(fov, W as f32 / H as f32, near, eye_dist * 20.0);
        let fx_view = FxView::new(view, proj, W, H);
        let camera = OrbitCamera::new(CameraTweaks::default(), eye_dist);
        let scene = Scene {
            camera: &camera,
            lighting: &lighting,
            background: None,
            ships: &[],
            nav_lights: false,
        };

        let mut world = FxWorld::new(lib.clone());
        let half_view = eye_dist * 0.5 / 0.9;
        let origin = Vector::new(shift * half_view, 0.0, shift * half_view * 0.6);
        world.spawn_named(name, origin, Matrix::IDENTITY, &[]);
        let stem = name.trim_end_matches(".ebg").trim_end_matches(".EBG");
        let (cols, thumb) = (8usize, 2usize);
        let rows = FRAMES.div_ceil(cols);
        let (sw, sh) = (cols * W as usize / thumb, rows * H as usize / thumb);
        let mut sheet = vec![0u8; sw * sh * 3];
        let mut lit_pixels = 0usize;
        let mut flares_seen = 0usize;
        for step in 0..stride * FRAMES {
            let mut list = RenderList::default();
            world.step(UNIVERSE_UPDATE_PERIOD, Some(&mut list));
            // Every frame, as in the game (the lens flares fade per frame).
            fx.prepare(&renderer, data.src(), &list, &fx_view, &lighting);
            fx.prepare_lens_flares(&mut world.lens_flares, &fx_view, None);
            flares_seen += list.flares.len();
            if (step + 1) % stride != 0 {
                continue;
            }
            let i = step / stride;
            let rgba = target
                .render_ex_with(&mut renderer, &scene, &SceneExtras::default(), |r, enc, c, d| {
                    fx.draw(r, enc, c, d)
                })
                .expect("render");
            let rgb: Vec<u8> = rgba.chunks(4).flat_map(|p| [p[0], p[1], p[2]]).collect();
            lit_pixels += rgb.chunks(3).filter(|p| p.iter().any(|&c| c > 16)).count();
            save_png(&out.join(format!("{stem}_{i:03}.png")), W, H, &rgb);
            let (ox, oy) = ((i % cols) * W as usize / thumb, (i / cols) * H as usize / thumb);
            for y in 0..H as usize / thumb {
                for x in 0..W as usize / thumb {
                    let s = ((y * thumb) * W as usize + x * thumb) * 3;
                    let d = ((oy + y) * sw + ox + x) * 3;
                    sheet[d..d + 3].copy_from_slice(&rgb[s..s + 3]);
                }
            }
        }
        save_png(&out.join(format!("{stem}_sheet.png")), sw as u32, sh as u32, &sheet);
        eprintln!(
            "{name}: {FRAMES} frames over {seconds} s, view radius {reach:.0}, {lit_pixels} lit pixels, {flares_seen} flare-frames"
        );
    }
}
