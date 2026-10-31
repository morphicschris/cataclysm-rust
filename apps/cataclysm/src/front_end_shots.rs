//! The modern front end over its live scene, as the game draws it ([`crate::app`]'s
//! `Backdrop`): the scene loaded and run, the menus walked and drawn over it, with the time
//! each frame takes. Needs the game install (`HWC_GAME_DIR`) and a GPU adapter; skips
//! otherwise. Screenshots go to `screenshots/front_end/scene_*.png`. Run with `--release`:
//! `cargo test --release -p cataclysm front_end_shots -- --ignored --nocapture`.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_ui::ingame::modern_menu as menu;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::menu_scene::{MenuScene, MenuSceneLoader};

const DT: f32 = 1.0 / 60.0;

fn save(s: &Session, name: &str) {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/front_end");
    s.canvas
        .save_png(&dir.join(format!("scene_{name}.png")))
        .expect("png");
}

/// One frame as the game draws it: the scene behind, the menus over it.
fn frame(s: &mut Session, scene: &mut MenuScene, r: &mut Renderer) -> f64 {
    let t = Instant::now();
    s.update(DT);
    scene.set_focus_x(s.backdrop_focus());
    scene.update(DT);
    scene.render(r, s.canvas_mut()).expect("scene");
    s.render_over_backdrop();
    t.elapsed().as_secs_f64() * 1e3
}

fn click(s: &mut Session, scene: &mut MenuScene, r: &mut Renderer, label: &str) {
    for _ in 0..30 {
        frame(s, scene, r);
    }
    let m = menu::model(&s.fe).unwrap_or_else(|| panic!("no modern screen: {:?} modern {}", s.fe.stack_names(), s.modern()));
    let (w, h) = s.fe.screen_size();
    let l = menu::layout_for(&s.fe, &m, w, h);
    let i = m.find(label).unwrap_or_else(|| panic!("no {label}"));
    let c = l.placed[i].control;
    s.mouse_move(f64::from((c.x0 + c.x1) / 2), f64::from((c.y0 + c.y1) / 2));
    frame(s, scene, r);
    s.mouse_button(true, true);
    frame(s, scene, r);
    s.mouse_button(true, false);
    frame(s, scene, r);
    // The pointer off the controls, for the picture.
    s.mouse_move(f64::from(w - 8), f64::from(h - 8));
    for _ in 0..90 {
        frame(s, scene, r);
    }
}

#[test]
#[ignore]
fn front_end_shots() {
    let Some(dir) = std::env::var_os("HWC_GAME_DIR").map(PathBuf::from) else {
        return;
    };
    let Ok(data) = GameData::open(&dir) else {
        return;
    };
    let data = Rc::new(data);
    let Some((device, queue)) = crate::test_gpu::device() else {
        return;
    };
    let mut r = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
    for size in [(1920, 1080), (1280, 720)] {
        let source: Rc<dyn AssetSource> = data.clone();
        let mut s = Session::new(
            source,
            Setup {
                scale: ScaleMode::Original,
                ..Default::default()
            },
        )
        .expect("session");
        s.resize(size.0, size.1);
        let mut loader = MenuSceneLoader::new(data.clone(), None);
        let started = Instant::now();
        let mut steps = 0;
        let mut scene = loop {
            steps += 1;
            if let Some(sc) = loader.advance(&mut r).expect("scene loads") {
                break sc;
            }
            assert!(steps < 100, "the scene never loaded");
        };
        eprintln!(
            "front end {}x{}: scene loaded in {steps} steps, {:.0} ms",
            size.0,
            size.1,
            started.elapsed().as_secs_f64() * 1e3
        );
        // Let the workers get going.
        for _ in 0..(20.0 / DT) as usize {
            s.update(DT);
            scene.update(DT);
        }
        let tag = format!("{}x{}", size.0, size.1);
        let mut times = Vec::new();
        for _ in 0..120 {
            times.push(frame(&mut s, &mut scene, &mut r));
        }
        times.sort_by(f64::total_cmp);
        eprintln!(
            "front end {tag}: main menu frame (update + scene + menus) median {:.2} ms, p90 {:.2} ms",
            times[times.len() / 2],
            times[times.len() * 9 / 10]
        );
        save(&s, &format!("main_{tag}"));
        click(&mut s, &mut scene, &mut r, "SINGLE PLAYER");
        save(&s, &format!("single_player_{tag}"));
        click(&mut s, &mut scene, &mut r, "PLAYER SETUP");
        save(&s, &format!("player_setup_{tag}"));
        click(&mut s, &mut scene, &mut r, "BACK");
        click(&mut s, &mut scene, &mut r, "OPTIONS");
        save(&s, &format!("audio_{tag}"));
        click(&mut s, &mut scene, &mut r, "ENHANCEMENTS");
        save(&s, &format!("enhancements_{tag}"));
        click(&mut s, &mut scene, &mut r, "CANCEL");
        click(&mut s, &mut scene, &mut r, "CANCEL");
        click(&mut s, &mut scene, &mut r, "QUIT");
        save(&s, &format!("quit_{tag}"));
    }
}
