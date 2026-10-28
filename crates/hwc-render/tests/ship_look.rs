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

/// A destroyer with a carrier and Acolytes over a mission background, under that mission's
/// lights, built up one modern effect at a time: `fleet_<step>.png`.
#[test]
#[ignore]
fn fleet_effects() {
    use hwc_render::{Background, PostFx, RenderQuality};
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
    let mission = std::env::var("HWC_MISSION").unwrap_or_else(|_| "mission03".into());
    let all = ShipDef::list_all(src.as_ref());
    let load = |name: &str| {
        let def = all.iter().find(|d| d.name == name).unwrap();
        ShipModel::load(src.as_ref(), def).unwrap()
    };
    let models = [load("sdestroyer"), load("scarrier"), load("sacolyte")];
    let bg = Background::load(src.as_ref(), &format!("btg/{mission}.btg")).unwrap();
    let lighting = src
        .read(&format!("hsf\\{mission}.hsf"))
        .and_then(|d| hwc_formats::hsf::Hsf::parse(&d).ok())
        .map_or_else(SceneLighting::default, |h| SceneLighting::from_hsf(&h));
    let dir = std::env::var_os("HWC_SHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/ship_look")
        });
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h) = (1280, 720);
    let colors = TeamColors::default();
    let (_, rd) = models[0].bounds();
    let (_, rc) = models[1].bounds();
    // Mesh space to world: the ship's heading along world -X, then turned and placed.
    let place = |at: glam::Vec3, yaw: f32| {
        glam::Mat4::from_translation(at)
            * glam::Mat4::from_rotation_z(yaw.to_radians())
            * mesh_to_world()
    };
    let none = ShipLook {
        subdivide: false,
        occlusion: 0.0,
        sky_light: 0.0,
        tone_map: false,
        shadows: false,
        ..ShipLook::modern()
    };
    let steps: [(&str, ShipLook, PostFx); 7] = [
        ("1_original", ShipLook::original(), PostFx::default()),
        ("2_ship_look", none, PostFx::default()),
        (
            "3_sky_light",
            ShipLook {
                sky_light: 1.0,
                ..none
            },
            PostFx::default(),
        ),
        (
            "4_occlusion_subdivide",
            ShipLook {
                sky_light: 1.0,
                occlusion: 0.8,
                subdivide: true,
                ..none
            },
            PostFx::default(),
        ),
        (
            "5_shadows",
            ShipLook {
                shadows: true,
                tone_map: true,
                ..ShipLook::modern()
            },
            PostFx::default(),
        ),
        (
            "6_bloom_tone_dither",
            ShipLook::modern(),
            PostFx {
                heat: 0.0,
                ..PostFx::modern()
            },
        ),
        ("7_all_with_heat", ShipLook::modern(), PostFx::modern()),
    ];
    for (name, look, post) in steps {
        let mut r = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
        r.set_ship_look(look);
        let paint = TeamPaint::Scheme(&colors, colors.schemes[0]);
        let gpu: Vec<_> = models.iter().map(|m| r.upload_ship(m, paint)).collect();
        let gpu_bg = r.upload_background(&bg);
        // The Acolytes fly between the light and the destroyer, so their shadows fall on it;
        // the carrier sits behind; the camera looks from the lit side.
        let l = lighting
            .lights
            .first()
            .map_or(glam::Vec3::Z, |l| l.direction.normalize());
        let side = l.cross(glam::Vec3::Z).normalize_or(glam::Vec3::Y);
        let want = (l + side * 0.7).normalize();
        let mut instances = vec![
            ShipInstance {
                ship: &gpu[0],
                transform: place(glam::Vec3::ZERO, 20.0),
                age: 0.5,
            },
            ShipInstance {
                ship: &gpu[1],
                transform: place(-want * (rd + rc) * 1.6 + side * rd * 1.2, 20.0),
                age: 0.5,
            },
        ];
        for k in 0..5 {
            let k = k as f32 - 2.0;
            let at = l * rd * 0.55 + side * k * rd * 0.2 + glam::Vec3::Z * (k.abs() * rd * 0.05);
            instances.push(ShipInstance {
                ship: &gpu[2],
                transform: place(at, 25.0),
                age: 0.5,
            });
        }
        let mut camera = OrbitCamera::new(CameraTweaks::default(), rd * 2.4);
        camera.ignore_zoom = true;
        camera.distance = rd * 2.4;
        camera.look_at = side * rd * 0.3;
        camera.declination = (-20f32).to_radians();
        camera.angle = (0..72)
            .map(|i| (i as f32 * 5.0).to_radians())
            .max_by(|&a, &b| {
                let score = |ang: f32| {
                    let mut c = camera.clone();
                    c.angle = ang;
                    (c.eye() - c.look_at).normalize().dot(want)
                };
                score(a).total_cmp(&score(b))
            })
            .unwrap();
        let quality = RenderQuality {
            post,
            ..Default::default()
        };
        let target = OffscreenTarget::with_quality(&r, (w, h), (w, h), quality);
        let scene = Scene {
            camera: &camera,
            lighting: &lighting,
            background: Some(&gpu_bg),
            ships: &instances,
            nav_lights: true,
        };
        let timed = r.set_gpu_timing(true);
        let rgba = target.render(&mut r, &scene).unwrap();
        if timed {
            let mut total = 0.0;
            for _ in 0..10 {
                target.render(&mut r, &scene).unwrap();
                total += r.take_gpu_ms().unwrap_or(0.0);
            }
            eprintln!("{name}: {:.3} ms on the GPU", total / 10.0);
        }
        let path = dir.join(format!("fleet_{name}.png"));
        save(&path, w, h, &rgba);
        eprintln!("wrote {}", path.display());
        instances.clear();
    }
}
