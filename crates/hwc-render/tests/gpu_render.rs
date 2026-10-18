//! Offscreen rendering smoke tests. They skip when no GPU adapter is available; the ship test
//! also needs game data (`HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`).

use hwc_render::assets::mothership_modules;
use hwc_render::{
    headless_device, mesh_to_world, AssetSource, Background, CameraTweaks, DirSource,
    OffscreenTarget, OrbitCamera, Renderer, Scene, SceneLighting, ShipDef, ShipInstance, ShipModel,
    TeamColors, TeamPaint, OFFSCREEN_FORMAT,
};

fn renderer() -> Option<Renderer> {
    match headless_device() {
        Ok((device, queue)) => Some(Renderer::new(&device, &queue, OFFSCREEN_FORMAT)),
        Err(e) => {
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn data_source() -> Option<Box<dyn AssetSource>> {
    if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
        return Some(Box::new(DirSource::new(dir)));
    }
    let big = hwc_formats::big::BigArchive::open(hwc_formats::game_dir()?.join("cataclysm.big"));
    Some(Box::new(big.ok()?))
}

#[test]
fn empty_scene_clears_to_black() {
    let Some(mut r) = renderer() else { return };
    let target = OffscreenTarget::new(&r, 64, 48);
    let camera = OrbitCamera::new(CameraTweaks::default(), 500.0);
    let lighting = SceneLighting::default();
    let scene = Scene {
        camera: &camera,
        lighting: &lighting,
        background: None,
        ships: &[],
        nav_lights: true,
    };
    let rgba = target.render(&mut r, &scene).unwrap();
    assert_eq!(rgba.len(), 64 * 48 * 4);
    assert!(rgba.chunks(4).all(|p| p == [0, 0, 0, 255]));
}

#[test]
fn renders_mothership_over_background() {
    let Some(src) = data_source() else {
        eprintln!("skipping: no game data");
        return;
    };
    let Some(mut r) = renderer() else { return };
    let all = ShipDef::list_all(src.as_ref());
    let def = all.iter().find(|d| d.name == "smothership").unwrap();
    let mut model = ShipModel::load(src.as_ref(), def).unwrap();
    for m in mothership_modules(def, &all) {
        model.add_module(src.as_ref(), m).unwrap();
    }
    let colors = TeamColors::default();
    let ship = r.upload_ship(&model, TeamPaint::Scheme(&colors, colors.schemes[0]));
    let bg = Background::load(src.as_ref(), "btg/mission13.btg").unwrap();
    let gpu_bg = r.upload_background(&bg);

    let (centre, radius) = model.bounds();
    let mut camera = OrbitCamera::new(CameraTweaks::default(), radius * 1.4);
    camera.look_at = mesh_to_world().transform_point3(centre);
    camera.declination = (-20f32).to_radians();
    let lighting = SceneLighting::default();
    let instances = [ShipInstance {
        ship: &ship,
        transform: mesh_to_world(),
        age: 0.5,
    }];
    let render = |r: &mut Renderer, background| {
        let target = OffscreenTarget::new(r, 160, 120);
        let scene = Scene {
            camera: &camera,
            lighting: &lighting,
            background,
            ships: &instances,
            nav_lights: true,
        };
        target.render(r, &scene).unwrap()
    };
    let with_bg = render(&mut r, Some(&gpu_bg));
    let ship_only = render(&mut r, None);
    let lit = ship_only.chunks(4).filter(|p| p[..3] != [0, 0, 0]).count();
    // The ship covers a good part of the frame but not all of it.
    assert!(
        lit > 160 * 120 / 20 && lit < 160 * 120 * 9 / 10,
        "{lit} lit pixels"
    );
    // The background fills the rest.
    let black = with_bg.chunks(4).filter(|p| p[..3] == [0, 0, 0]).count();
    assert!(
        black < 160 * 120 / 20,
        "{black} black pixels with a background"
    );
}
