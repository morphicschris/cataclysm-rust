//! The image-quality modes (render scale, MSAA, FXAA) drawn offscreen on a synthetic scene: a
//! white triangle with slanted edges and a one-pixel line over black. Skips without a GPU.

use glam::Vec3;
use hwc_render::effects::{EffectVertex, Line, Triangle};
use hwc_render::{
    headless_device_with_msaa, Blend, CameraTweaks, EffectBatch, EffectTexture, OffscreenTarget,
    OrbitCamera, RenderQuality, Renderer, Scene, SceneExtras, SceneLighting, OFFSCREEN_FORMAT,
};

const W: u32 = 160;
const H: u32 = 120;

fn renderer() -> Option<Renderer> {
    match headless_device_with_msaa() {
        Ok((device, queue, counts)) => {
            let mut r = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
            r.set_msaa_support(&counts);
            Some(r)
        }
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn batch() -> EffectBatch {
    let v = |y: f32, z: f32| EffectVertex {
        position: Vec3::new(0.0, y, z),
        color: [1.0; 4],
        uv: [0.0, 0.0],
    };
    let mut b = EffectBatch::default();
    b.triangles.push(Triangle {
        vertices: [v(-150.0, -90.0), v(130.0, -70.0), v(-40.0, 120.0)],
        texture: EffectTexture::WHITE,
        blend: Blend::Alpha,
    });
    b.lines.push(Line {
        from: Vec3::new(0.0, -250.0, 170.0),
        to: Vec3::new(0.0, 250.0, 140.0),
        from_color: [1.0; 4],
        to_color: [1.0; 4],
        width: 1.0,
        blend: Blend::Alpha,
    });
    b
}

/// Renders the scene in `quality` (MSAA from the renderer), returning the picture and the
/// target's plan.
fn draw(r: &mut Renderer, quality: RenderQuality, present: (u32, u32)) -> (Vec<u8>, hwc_render::RenderPlan) {
    r.set_sample_count(quality.msaa);
    let target = OffscreenTarget::with_quality(r, (W, H), present, quality);
    let camera = OrbitCamera::new(CameraTweaks::default(), 500.0);
    let lighting = SceneLighting::default();
    let scene = Scene {
        camera: &camera,
        lighting: &lighting,
        background: None,
        ships: &[],
        nav_lights: false,
    };
    let b = batch();
    let extras = SceneExtras {
        ship_states: &[],
        effects: Some(&b),
    };
    let rgba = target.render_ex(r, &scene, &extras).unwrap();
    (rgba, target.plan)
}

/// Pixels that are neither black nor white: the smoothed edges.
fn partial(rgba: &[u8]) -> usize {
    rgba.chunks(4).filter(|p| p[0] > 8 && p[0] < 247).count()
}

/// Total brightness (what a filter has to keep).
fn weight(rgba: &[u8]) -> f64 {
    rgba.chunks(4).map(|p| f64::from(p[0])).sum::<f64>() / 255.0
}

fn save(name: &str, rgba: &[u8]) {
    if let Some(dir) = std::env::var_os("HWC_SHOT_DIR") {
        let path = std::path::Path::new(&dir).join(name);
        let f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let mut enc = png::Encoder::new(f, W, H);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(rgba).unwrap();
    }
}

#[test]
fn every_mode_renders_and_smooths_edges() {
    let Some(mut r) = renderer() else { return };
    let (off, plan) = draw(&mut r, RenderQuality::default(), (W, H));
    assert_eq!(plan.render, (W, H));
    assert_eq!(off.len(), (W * H * 4) as usize);
    save("quality-off.png", &off);
    // The original picture has hard edges.
    let hard = partial(&off);
    let base = weight(&off);
    assert!(base > 300.0, "the triangle is drawn ({base})");
    eprintln!("off: {hard} partial pixels, weight {base:.0}; MSAA support {:?}", r.msaa_support());

    let mut modes = vec![
        ("scale-0.5", RenderQuality { scale: 0.5, ..Default::default() }),
        ("scale-1.5", RenderQuality { scale: 1.5, ..Default::default() }),
        ("scale-2", RenderQuality { scale: 2.0, ..Default::default() }),
        ("fxaa", RenderQuality { fxaa: true, ..Default::default() }),
        ("scale-2-fxaa", RenderQuality { scale: 2.0, fxaa: true, ..Default::default() }),
    ];
    for n in r.msaa_support().to_vec() {
        if n > 1 {
            modes.push((
                Box::leak(format!("msaa-{n}").into_boxed_str()),
                RenderQuality { msaa: n, ..Default::default() },
            ));
            modes.push((
                Box::leak(format!("scale-2-msaa-{n}").into_boxed_str()),
                RenderQuality { scale: 2.0, msaa: n, ..Default::default() },
            ));
        }
    }
    for (name, q) in modes {
        let (img, plan) = draw(&mut r, q, (W, H));
        save(&format!("quality-{name}.png"), &img);
        assert_eq!(img.len(), (W * H * 4) as usize, "{name}");
        assert_eq!(plan.samples, q.msaa.max(1), "{name}");
        let p = partial(&img);
        let w = weight(&img);
        eprintln!("{name}: render {:?}, {p} partial pixels, weight {w:.0}", plan.render);
        // Same picture, same weight: framing and line widths follow the logical size.
        assert!((w - base).abs() < base * 0.08, "{name}: weight {w} vs {base}");
        if q.scale > 1.0 || q.msaa > 1 || q.fxaa {
            assert!(p > hard + 40, "{name}: {p} partial pixels vs {hard}");
        }
    }
    r.set_sample_count(1);
}

#[test]
fn layered_present_keeps_the_logical_framing() {
    let Some(mut r) = renderer() else { return };
    let (small, _) = draw(&mut r, RenderQuality::default(), (W, H));
    // Shown at 3x: the scene is drawn at the display size and filtered down for the read-back.
    let (big, plan) = draw(&mut r, RenderQuality::default(), (W * 3, H * 3));
    assert_eq!(plan.render, (W * 3, H * 3));
    assert_eq!(big.len(), small.len());
    let (a, b) = (weight(&small), weight(&big));
    assert!((a - b).abs() < a * 0.08, "{a} vs {b}");
}

#[test]
fn target_and_renderer_samples_must_match() {
    let Some(mut r) = renderer() else { return };
    if !r.msaa_support().contains(&4) {
        return;
    }
    r.set_sample_count(1);
    let target = OffscreenTarget::new(&r, W, H);
    r.set_sample_count(4);
    let camera = OrbitCamera::new(CameraTweaks::default(), 500.0);
    let lighting = SceneLighting::default();
    let scene = Scene {
        camera: &camera,
        lighting: &lighting,
        background: None,
        ships: &[],
        nav_lights: false,
    };
    assert!(target.render(&mut r, &scene).is_err());
    r.set_sample_count(1);
}
