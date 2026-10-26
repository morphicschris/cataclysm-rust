//! Game-data screenshots of the modern menus (`hwc_ui::ingame::modern_menu`, `--menus modern`,
//! the default) in mission 1: the pause menu, the options hub and each options screen (audio,
//! advanced speech, video, gameplay, key bindings, the modern options), save, the quit
//! confirmation, and the systems manager, over the running mission at 1920x1080, 1024x768
//! and 640x480. Each screen is reached by clicking the modern controls, as a player would.
//! Screenshots go to `screenshots/menus/m1_*.png`. Needs the game install (`HWC_GAME_DIR`)
//! and a GPU adapter; skips otherwise. Slow in a debug build: run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::mission::sp_state;
use hwc_ui::canvas::Rect;
use hwc_ui::ingame::modern_launch_manager as mlm;
use hwc_ui::ingame::modern_menu::{self as menu, Kind};
use hwc_ui::ingame::modern_options as mo;
use hwc_ui::ingame::{InputEvent, Manager};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};

const DT: f32 = 1.0 / 16.0;

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
    size: (i32, i32),
}

fn rig(size: (i32, i32)) -> Option<Rig> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let data = Rc::new(GameData::open(&dir).ok()?);
    let (device, queue) = crate::test_gpu::device()?;
    let renderer = Renderer::new(&device, &queue, hwc_render::OFFSCREEN_FORMAT);
    let read = |name: &str| {
        data.install_file(name)
            .and_then(|p| std::fs::read(p).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let source: Rc<dyn AssetSource> = data.clone();
    let mut session = Session::new(
        source,
        Setup {
            options_path: None,
            original_cfg: read("Cataclysm.cfg"),
            default_keys: read("default_keys.cfg"),
            keys: None,
            keys_path: None,
            scale: ScaleMode::Original,
        },
    )
    .ok()?;
    session.resize(size.0 as u32, size.1 as u32);
    Some(Rig {
        data,
        session,
        renderer,
        audio: None,
        size,
    })
}

impl Rig {
    fn mission_one(&mut self) -> Box<Game> {
        let mut l =
            Loading::new(self.data.clone(), &self.session, 1, 2, self.size).expect("loading");
        l.opening_nis = false;
        l.loader.play_nislets = false;
        for _ in 0..40 {
            match l.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission 1 did not start: {e}"),
            }
        }
        panic!("loading never finished");
    }

    fn frame(&mut self, g: &mut Game) {
        let out = g.update(DT, &mut self.audio);
        assert!(
            !out.contains(&Outcome::NextMission),
            "mission 1 ended: {out:?}"
        );
        if out.contains(&Outcome::ApplyOptions) {
            let o = g.ui.fe.game.options.clone();
            g.apply_modern_options(&o);
        }
    }

    fn input(&mut self, g: &mut Game, ev: InputEvent) {
        g.input(ev, &mut self.audio);
    }

    /// A click clear of any double-click time.
    fn click(&mut self, g: &mut Game, x: i32, y: i32) {
        for _ in 0..10 {
            self.frame(g);
        }
        self.input(g, InputEvent::MouseMove { x, y });
        self.frame(g);
        for down in [true, false] {
            self.input(g, InputEvent::MouseButton { left: true, down });
            self.frame(g);
        }
    }

    /// Clicks the modern control labelled `label`.
    fn press(&mut self, g: &mut Game, label: &str) {
        let m = menu::model(&g.ui.fe).expect("a modern menu");
        let l = menu::layout(&m, self.size.0, self.size.1);
        let i = m
            .find(label)
            .unwrap_or_else(|| panic!("no {label} on {}", m.name));
        let r = match m.items[i].kind {
            Kind::Toggle { .. } => l.placed[i].row,
            _ => l.placed[i].control,
        };
        let (x, y) = centre(r);
        self.click(g, x, y);
    }

    fn key(&mut self, g: &mut Game, code: u16) {
        self.input(g, InputEvent::KeyDown(code));
        self.frame(g);
        self.input(g, InputEvent::KeyUp(code));
        self.frame(g);
    }

    /// The pointer off every control, a few frames for the highlights to settle, a shot.
    fn shot(&mut self, g: &mut Game, name: &str) {
        self.input(
            g,
            InputEvent::MouseMove {
                x: 2,
                y: self.size.1 - 2,
            },
        );
        for _ in 0..8 {
            self.frame(g);
        }
        for _ in 0..2 {
            g.render(&mut self.renderer, self.size).expect("render");
        }
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/menus");
        std::fs::create_dir_all(&dir).expect("screenshot dir");
        g.canvas()
            .save_png(&dir.join(format!("{name}_{}x{}.png", self.size.0, self.size.1)))
            .expect("png");
    }
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn boxed(g: &Game) -> bool {
    g.letterbox.fully || g.nis.is_some()
}

fn screen(g: &Game) -> String {
    g.ui.fe.current_screen_name().unwrap_or("").to_string()
}

fn in_mission(size: (i32, i32)) {
    let Some(mut rig) = rig(size) else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    assert!(menu::enabled(&g.ui.fe.game.options), "on by default");
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !boxed(g))
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    for _ in 0..8 {
        rig.frame(g);
    }
    // One frame drawn: the interface takes the window's size.
    g.render(&mut rig.renderer, size).expect("render");
    // Escape with nothing to cancel opens the pause menu.
    for _ in 0..3 {
        if g.ui.manager() == Some(Manager::EscMenu) {
            break;
        }
        rig.key(g, 27);
    }
    assert_eq!(g.ui.manager(), Some(Manager::EscMenu));
    assert!(menu::active(&g.ui.fe));
    assert!(g.paused, "the pause menu pauses");
    rig.shot(g, "m1_pause");
    rig.press(g, "OPTIONS");
    assert_eq!(screen(g), "Game_options");
    rig.shot(g, "m1_options");
    for (button, name, tag) in [
        ("AUDIO", "Audio", "audio"),
        ("VIDEO", "Video", "video"),
        ("GAMEPLAY", "Gameplay", "gameplay"),
        ("KEY BINDINGS", "Keys", "keys"),
    ] {
        rig.press(g, button);
        assert_eq!(screen(g), name);
        rig.shot(g, &format!("m1_{tag}"));
        if name == "Audio" {
            rig.press(g, "ADVANCED SPEECH SETTINGS");
            assert_eq!(screen(g), "Advanced_speech");
            rig.shot(g, "m1_advanced_speech");
            rig.key(g, 27);
        }
        rig.key(g, 27);
        assert_eq!(screen(g), "Game_options");
    }
    rig.press(g, mo::HUB_LABEL);
    assert!(g.ui.state().mmenu.page.is_some());
    // MSAA 4x, accepted: applied to the 3D view at once.
    let m = menu::model(&g.ui.fe).unwrap();
    let l = menu::layout(&m, size.0, size.1);
    let i = m.find("ANTI-ALIASING (MSAA)").unwrap();
    let (x, y) = centre(l.placed[i].parts[2]);
    rig.click(g, x, y);
    rig.shot(g, "m1_enhancements");
    rig.press(g, "ACCEPT");
    assert_eq!(g.quality.msaa, 4, "applied at once");
    rig.key(g, 27);
    assert_eq!(screen(g), "In_game_esc_menu2");
    rig.press(g, "SAVE GAME");
    assert_eq!(screen(g), "In_game_save");
    rig.shot(g, "m1_save");
    rig.key(g, 27);
    rig.press(g, "QUIT");
    assert_eq!(screen(g), "Quit_game2");
    rig.shot(g, "m1_quit");
    rig.press(g, "NO");
    rig.press(g, "BACK TO GAME");
    assert_eq!(g.ui.manager(), None);
    assert!(!g.paused);
    // The systems manager.
    rig.key(g, u16::from(b'S'));
    assert_eq!(g.ui.manager(), Some(Manager::Launch));
    assert!(mlm::active(&g.ui.fe));
    rig.shot(g, "m1_systems");
    rig.key(g, 27);
    assert_eq!(g.ui.manager(), None);
}

#[test]
fn modern_menus_in_mission_one_1080p() {
    in_mission((1920, 1080));
}

#[test]
fn modern_menus_in_mission_one_768() {
    in_mission((1024, 768));
}

#[test]
fn modern_menus_in_mission_one_640() {
    in_mission((640, 480));
}
