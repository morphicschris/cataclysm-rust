//! Close-ups of the Acolyte in the original look and the modern ship look, written as PNGs for
//! comparing by eye. Needs a GPU and game data (`HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`):
//!
//! `cargo test --release -p hwc-render --test ship_look -- --ignored --nocapture`
//!
//! The pictures go to `HWC_SHOT_DIR` (default `target/ship_look`).

use std::path::PathBuf;

use hwc_render::{
    headless_device, mesh_to_world, AssetSource, CameraTweaks, DirSource, OffscreenTarget,
    OrbitCamera, Renderer, Scene, SceneLighting, ShipDef, ShipInstance, ShipLook, ShipModel,
    TeamColors, TeamPaint, OFFSCREEN_FORMAT,
};

fn data_source() -> Option<Box<dyn AssetSource>> {
    if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
        return Some(Box::new(DirSource::new(dir)));
    }
    let big = hwc_formats::big::BigArchive::open(hwc_formats::game_dir()?.join("cataclysm.big"));
    Some(Box::new(big.ok()?))
}

fn save(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// `lif` with its pixels taken from the PNG at `path`, keeping its alpha and team colour
/// masks (sampled from the original at each new pixel).
fn replaced(lif: &hwc_formats::lif::Lif, path: &std::path::Path) -> hwc_formats::lif::Lif {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width, info.height);
    let channels = info.color_type.samples();
    let old = lif.to_rgba();
    let at = |x: u32, y: u32| -> usize {
        let ox = (x * lif.width / w).min(lif.width - 1);
        let oy = (y * lif.height / h).min(lif.height - 1);
        (oy * lif.width + ox) as usize
    };
    let mut pixels = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) as usize) * channels;
            pixels.extend_from_slice(&buf[i..i + 3]);
            pixels.push(old[at(x, y) * 4 + 3]);
        }
    }
    let mask = |which| {
        lif.team_mask(which).map(|m| {
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| m[at(x, y)])
                .collect()
        })
    };
    hwc_formats::lif::Lif {
        width: w,
        height: h,
        format: hwc_formats::lif::PixelFormat::Rgba,
        palette: None,
        team_effect0: mask(0),
        team_effect1: mask(1),
        pixels,
        ..lif.clone()
    }
}

#[test]
#[ignore]
fn acolyte_close_ups() {
    let Some(src) = data_source() else {
        eprintln!("skipping: no game data");
        return;
    };
    let (device, queue) = match headless_device() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    let ship = std::env::var("HWC_SHIP").unwrap_or_else(|_| "sacolyte".into());
    let all = ShipDef::list_all(src.as_ref());
    let Some(def) = all.iter().find(|d| d.name == ship) else {
        panic!(
            "no {ship}; ships: {:?}",
            all.iter().map(|d| &d.name).collect::<Vec<_>>()
        );
    };
    let mut model = ShipModel::load(src.as_ref(), def).unwrap();
    // Experiments with replacement textures: HWC_TEX_DUMP writes every hull texture as
    // tex_<part>_<material>.png there, HWC_TEX_OVERRIDE takes any such file found there in
    // place of the texture (any size; alpha and team colour masks come from the original).
    if let Some(dir) = std::env::var_os("HWC_TEX_DUMP").map(PathBuf::from) {
        std::fs::create_dir_all(&dir).unwrap();
        for (p, part) in model.parts.iter().enumerate() {
            for (m, lif) in part.textures.iter().enumerate() {
                if let Some(lif) = lif {
                    let path = dir.join(format!("tex_{p}_{m}.png"));
                    save(&path, lif.width, lif.height, &lif.to_rgba());
                }
            }
        }
    }
    if let Some(dir) = std::env::var_os("HWC_TEX_OVERRIDE").map(PathBuf::from) {
        for (p, part) in model.parts.iter_mut().enumerate() {
            for (m, lif) in part.textures.iter_mut().enumerate() {
                let path = dir.join(format!("tex_{p}_{m}.png"));
                if let (Some(lif), true) = (lif.as_mut(), path.exists()) {
                    *lif = replaced(lif, &path);
                    eprintln!("replaced material {m} with {}", path.display());
                }
            }
        }
    }
    let colors = TeamColors::default();
    let dir = std::env::var_os("HWC_SHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ship_look")
        });
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (960, 600);
    let (centre, radius) = model.bounds();
    let lighting = SceneLighting::default();
    for (look_name, look) in [
        ("original", ShipLook::original()),
        ("modern", ShipLook::modern()),
    ] {
        let mut r = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
        r.set_ship_look(look);
        let gpu = r.upload_ship(&model, TeamPaint::Scheme(&colors, colors.schemes[0]));
        let instances = [ShipInstance {
            ship: &gpu,
            transform: mesh_to_world(),
            age: 0.5,
        }];
        // Close-ups from the front quarter, the rear and above, and one far enough out to show
        // the texture filtering at a distance.
        for (view, angle, decl, dist) in [
            ("quarter", 135.0f32, -22.0f32, 1.5f32),
            ("rear", 45.0, -28.0, 1.5),
            ("top", 300.0, -55.0, 1.5),
            ("close", 150.0, -35.0, 1.0),
            ("far", 135.0, -25.0, 6.0),
        ] {
            let mut camera = OrbitCamera::new(CameraTweaks::default(), radius * dist);
            camera.ignore_zoom = true;
            camera.distance = radius * dist;
            camera.look_at = mesh_to_world().transform_point3(centre);
            camera.angle = angle.to_radians();
            camera.declination = decl.to_radians();
            let target = OffscreenTarget::new(&r, w, h);
            let scene = Scene {
                camera: &camera,
                lighting: &lighting,
                background: None,
                ships: &instances,
                nav_lights: true,
            };
            let timed = r.set_gpu_timing(true);
            let rgba = target.render(&mut r, &scene).unwrap();
            if timed {
                // Warm, then the mean of a few frames.
                let mut total = 0.0;
                for _ in 0..10 {
                    target.render(&mut r, &scene).unwrap();
                    total += r.take_gpu_ms().unwrap_or(0.0);
                }
                eprintln!("{look_name} {view}: {:.3} ms on the GPU", total / 10.0);
            }
            let path = dir.join(format!("{ship}_{view}_{look_name}.png"));
            save(&path, w, h, &rgba);
            eprintln!("wrote {}", path.display());
        }
    }
}
