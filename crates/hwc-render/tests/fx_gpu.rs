//! Offscreen GPU tests of the effect renderer. They skip without a GPU adapter; the effect
//! tests also need game data (`HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`).

use glam::{Mat4, Vec3};
use hwc_fx::compiler::{FileSource, Library};
use hwc_fx::render::{Blend, Line, Point, RenderList, Sprite};
use hwc_fx::{load_scripts, FxWorld};
use hwc_math::timing::UNIVERSE_UPDATE_PERIOD;
use hwc_math::{Matrix, Vector};
use hwc_render::{
    headless_device, AssetSource, CameraTweaks, DirSource, EffectRenderer, FxView,
    OffscreenTarget, OrbitCamera, Renderer, Scene, SceneExtras, SceneLighting, OFFSCREEN_FORMAT,
};

const W: u32 = 64;
const H: u32 = 64;

fn renderer() -> Option<Renderer> {
    match headless_device() {
        Ok((device, queue)) => Some(Renderer::new(&device, &queue, OFFSCREEN_FORMAT)),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

struct Data(Box<dyn AssetSource>);

impl FileSource for Data {
    fn read_file(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path)
    }
}

fn data() -> Option<Data> {
    if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
        return Some(Data(Box::new(DirSource::new(dir))));
    }
    let big = hwc_formats::big::BigArchive::open(hwc_formats::game_dir()?.join("cataclysm.big"));
    Some(Data(Box::new(big.ok()?)))
}

struct Empty;

impl AssetSource for Empty {
    fn read(&self, _: &str) -> Option<Vec<u8>> {
        None
    }
    fn list(&self) -> Vec<String> {
        Vec::new()
    }
}

/// Camera on -Y looking at the origin, +Z up; `half` world units fill half the screen.
fn view(half: f32) -> FxView {
    let fov = 1.0f32;
    let dist = half / (fov * 0.5).tan();
    let v = Mat4::look_at_rh(Vec3::new(0.0, -dist, 0.0), Vec3::ZERO, Vec3::Z);
    let p = Mat4::perspective_rh(fov, 1.0, 1.0, dist * 4.0);
    FxView::new(v, p, W, H)
}

fn draw(r: &mut Renderer, fx: &mut EffectRenderer, list: &RenderList, v: &FxView) -> Vec<u8> {
    let target = OffscreenTarget::new(r, W, H);
    let lighting = SceneLighting::default();
    let camera = OrbitCamera::new(CameraTweaks::default(), 100.0);
    let scene = Scene {
        camera: &camera,
        lighting: &lighting,
        background: None,
        ships: &[],
        nav_lights: false,
    };
    fx.prepare(r, &Empty, list, v, &lighting);
    target
        .render_ex_with(r, &scene, &SceneExtras::default(), |r, e, c, d| fx.draw(r, e, c, d))
        .unwrap()
}

fn px(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let i = ((y * W + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
}

fn disc(color: [f32; 4], radius: f32) -> Sprite {
    Sprite {
        position: Vector::ZERO,
        size: [radius, radius],
        rotation: 0.0,
        color,
        bias: [0.0; 3],
        texture: None,
        uv: [[0, 0], [0, 0]],
        slices: 16,
        blend: Blend::Opaque,
        stipple: false,
        additive: false,
        illum: 0.0,
    }
}

#[test]
fn opaque_and_alpha_discs() {
    let Some(mut r) = renderer() else { return };
    let mut fx = EffectRenderer::new(&r);
    let mut list = RenderList::default();
    list.sprites.push(disc([0.5, 0.25, 1.0, 1.0], 5.0));
    let img = draw(&mut r, &mut fx, &list, &view(10.0));
    // Colour bytes as partCircleSolid3 rounds them: 0.5 -> 128, 0.25 -> 64.
    assert_eq!(&px(&img, 32, 32)[..3], &[128, 64, 255]);
    // Radius 5 of 10: a quarter of the width from the centre is inside, beyond the edge black.
    assert_eq!(&px(&img, 32, 22)[..3], &[128, 64, 255]);
    assert_eq!(&px(&img, 32, 10)[..3], &[0, 0, 0]);
    // A half-transparent white disc on top blends with alpha.
    list.sprites.push(disc([1.0, 1.0, 1.0, 0.5], 5.0));
    let img = draw(&mut r, &mut fx, &list, &view(10.0));
    let c = px(&img, 32, 32);
    assert!((i32::from(c[0]) - 192).abs() <= 1, "{c:?}");
    assert!((i32::from(c[1]) - 160).abs() <= 1, "{c:?}");
}

#[test]
fn points_and_lines_are_pixel_sized() {
    let Some(mut r) = renderer() else { return };
    let mut fx = EffectRenderer::new(&r);
    let mut list = RenderList::default();
    list.points.push(Point {
        position: Vector::ZERO,
        size: 3.0,
        color: [1.0, 0.0, 0.0, 1.0],
        blend: Blend::Opaque,
    });
    list.lines.push(Line {
        start: Vector::new(-8.0, 0.0, 5.0),
        end: Vector::new(8.0, 0.0, 5.0),
        width: 1.0,
        color: [0.0, 1.0, 0.0, 1.0],
        blend: Blend::Opaque,
    });
    let img = draw(&mut r, &mut fx, &list, &view(10.0));
    let red = (0..W * H)
        .filter(|i| px(&img, i % W, i / W)[..3] == [255, 0, 0])
        .count();
    assert_eq!(red, 9, "a 3-pixel point covers 3 x 3 pixels");
    let green_rows: std::collections::BTreeSet<u32> = (0..W * H)
        .filter(|i| px(&img, i % W, i / W)[..3] == [0, 255, 0])
        .map(|i| i / W)
        .collect();
    assert_eq!(green_rows.len(), 1, "a 1-pixel horizontal line is one row: {green_rows:?}");
}

#[test]
fn effects_render_with_game_textures() {
    let Some(d) = data() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Some(mut r) = renderer() else { return };
    let mut lib = Library::new();
    load_scripts(&mut lib, &d);
    for name in ["bigdeath.ebg", "FireEnergySmall01.ebg"] {
        if lib.get(name).is_none() {
            lib.load_effect(&d, &format!("ETG\\{name}")).unwrap();
        }
        let mut world = FxWorld::new(lib.clone());
        world.spawn_named(name, Vector::ZERO, Matrix::IDENTITY, &[]).unwrap();
        let mut fx = EffectRenderer::new(&r);
        let target = OffscreenTarget::new(&r, W, H);
        let lighting = SceneLighting::default();
        let camera = OrbitCamera::new(CameraTweaks::default(), 100.0);
        let scene = Scene {
            camera: &camera,
            lighting: &lighting,
            background: None,
            ships: &[],
            nav_lights: false,
        };
        let v = view(if name == "bigdeath.ebg" { 2000.0 } else { 40.0 });
        let (mut lit, mut textured) = (0, false);
        for _ in 0..12 {
            let mut list = RenderList::default();
            world.step(UNIVERSE_UPDATE_PERIOD, Some(&mut list));
            fx.prepare(&r, d.0.as_ref(), &list, &v, &lighting);
            textured |= fx.frame().draws.iter().any(|d| d.texture > 0);
            let img = target
                .render_ex_with(&mut r, &scene, &SceneExtras::default(), |r, e, c, dp| {
                    fx.draw(r, e, c, dp)
                })
                .unwrap();
            lit += img.chunks(4).filter(|p| p[0] > 32 || p[1] > 32).count();
        }
        assert!(lit > 50, "{name}: only {lit} lit pixels");
        // Textures came from the archive.
        assert!(textured, "{name}: untextured");
    }
}

#[test]
fn lens_flare_draws_off_centre() {
    let Some(d) = data() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Some(mut r) = renderer() else { return };
    let mut fx = EffectRenderer::new(&r);
    assert!(fx.load_lens_flares(&r, d.0.as_ref(), "ps-50-300mm-zoom"));
    let set = fx.lens_flares().unwrap();
    assert_eq!(set.fadeout_frame_size, 1.1);
    assert_eq!(set.rects.len(), 10);
    let mut flares = vec![hwc_fx::particle::LensFlare {
        position: Vector::new(5.0, 0.0, 5.0),
        intensity: 1.0,
        enabled: 1,
        pending: 0,
        active: 1,
    }];
    let v = view(10.0);
    let target = OffscreenTarget::new(&r, W, H);
    let lighting = SceneLighting::default();
    let camera = OrbitCamera::new(CameraTweaks::default(), 100.0);
    let scene = Scene {
        camera: &camera,
        lighting: &lighting,
        background: None,
        ships: &[],
        nav_lights: false,
    };
    let mut img = Vec::new();
    for _ in 0..10 {
        flares[0].pending = 0; // its particle system drew this frame
        fx.prepare(&r, d.0.as_ref(), &RenderList::default(), &v, &lighting);
        fx.prepare_lens_flares(&mut flares, &v, None);
        img = target
            .render_ex_with(&mut r, &scene, &SceneExtras::default(), |r, e, c, dp| {
                fx.draw(r, e, c, dp)
            })
            .unwrap();
    }
    assert!(fx.flare_states()[0].fade > 0.99);
    assert_eq!(flares[0].pending, 1);
    // The light sits up and to the right of the centre: brightest there.
    let bright = |x: u32, y: u32| px(&img, x, y)[..3].iter().map(|&c| u32::from(c)).sum::<u32>();
    assert!(bright(48, 16) > 300, "light source missing: {:?}", px(&img, 48, 16));
}
