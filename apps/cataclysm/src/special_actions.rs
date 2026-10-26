//! Game-data test of the special actions on the game path: in mission 1, after the linking
//! technology, two selected Acolytes, the Z key (`Ship_Special`) opens the special-action menu,
//! its "Link" item makes them an ACV, and the ACV's menu offers "Unlink". Needs the game
//! install (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Run with `--release`.

use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_math::Vector;
use hwc_render::Renderer;
use hwc_sim::statics::{ShipType, Technology};
use hwc_sim::ShipId;
use hwc_ui::ingame::popup::MenuKind;
use hwc_ui::ingame::InputEvent;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::Game;
use crate::host::PLAYER;
use crate::play::{Loading, Progress};

const SIZE: (i32, i32) = (1024, 768);
const DT: f32 = 1.0 / 16.0;
/// The Z key.
const KEY_Z: u16 = 0x5a;

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
}

fn rig() -> Option<Rig> {
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
    session.resize(SIZE.0 as u32, SIZE.1 as u32);
    Some(Rig {
        data,
        session,
        renderer,
        audio: None,
    })
}

impl Rig {
    fn mission1(&mut self) -> Box<Game> {
        let mut l = Loading::new(self.data.clone(), &self.session, 1, 2, SIZE).expect("loading");
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
        g.update(DT, &mut self.audio);
    }

    fn key(&mut self, g: &mut Game, code: u16) {
        g.input(InputEvent::KeyDown(code), &mut self.audio);
        self.frame(g);
        g.input(InputEvent::KeyUp(code), &mut self.audio);
        self.frame(g);
    }

    fn click(&mut self, g: &mut Game, x: i32, y: i32) {
        g.input(InputEvent::MouseMove { x, y }, &mut self.audio);
        self.frame(g);
        for down in [true, false] {
            g.input(InputEvent::MouseButton { left: true, down }, &mut self.audio);
            self.frame(g);
        }
    }
}

fn menu_labels(g: &Game) -> Vec<String> {
    g.ui.fe.game.ig.popup.items.iter().map(|i| i.label.clone()).collect()
}

/// Opens the special-action menu with Z and clicks `label`.
fn z_menu_pick(rig: &mut Rig, g: &mut Game, label: &str) {
    rig.key(g, KEY_Z);
    assert_eq!(g.ui.fe.game.ig.popup.open, Some(MenuKind::Special), "Z opens the special-action menu");
    let labels = menu_labels(g);
    let item = g.ui.fe.game.ig.popup.items.iter().find(|i| i.label == label).unwrap_or_else(|| panic!("no {label} in {labels:?}"));
    let r = item.rect;
    rig.click(g, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    assert!(g.ui.fe.game.ig.popup.open.is_none(), "the menu closes");
}

fn acolytes(g: &mut Game) -> [ShipId; 2] {
    let u = g.world.universe_mut();
    let ms = u.base.players[PLAYER].mothership.expect("mothership");
    let p = u.ship(ms).unwrap().posinfo.position;
    let mut make = |off: Vector| {
        let id = hwc_sim::base::create_ship(u, ShipType::sAcolyte, PLAYER, p + off, Default::default()).expect("acolyte");
        hwc_sim::construction::unit_cap_create_ship(u, id);
        id
    };
    [make(Vector::new(3000.0, 0.0, 0.0)), make(Vector::new(3400.0, 300.0, 200.0))]
}

fn acvs(g: &Game) -> Vec<ShipId> {
    let u = g.world.universe();
    u.ship_list.iter().copied().filter(|&id| u.ship(id).is_some_and(|s| s.ship_type == ShipType::sACV && s.player == PLAYER)).collect()
}

#[test]
fn mission1_z_menu_links_acolytes_into_an_acv() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission1();
    let g = &mut *game;
    let mut n = 0;
    while !(g.world.run().game.ships_have_arrived && g.world.run().game.sp_state == hwc_sim::mission::sp_state::NONE) {
        rig.frame(g);
        n += 1;
        assert!(n < 16 * 60, "the fleet never arrived");
    }
    // Linking researched.
    let u = g.world.universe_mut();
    u.base.players[PLAYER].tech.add(Technology::Link.index());
    hwc_sim::upgrade::player_update_from_research(u, PLAYER);
    let pair = acolytes(g);
    g.selection = pair.to_vec();
    rig.frame(g);

    z_menu_pick(&mut rig, g, "Link");
    let mut n = 0;
    while acvs(g).is_empty() {
        rig.frame(g);
        n += 1;
        assert!(n < 16 * 120, "no ACV after {n} frames");
    }
    assert!(pair.iter().all(|&a| g.world.universe().ship(a).is_none()));

    // The ACV's menu splits it again.
    g.selection = acvs(g);
    rig.frame(g);
    rig.key(g, KEY_Z);
    let labels = menu_labels(g);
    assert!(labels.iter().any(|l| l == "Unlink") && !labels.iter().any(|l| l == "Link"), "{labels:?}");
}
