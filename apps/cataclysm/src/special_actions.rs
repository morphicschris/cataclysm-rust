//! Game-data test of the special actions on the game path: in mission 1, after the linking
//! technology, two selected Acolytes, the Z key (`Ship_Special`) opens the special-action menu,
//! its "Link" item makes them an ACV, and the ACV's menu offers "Unlink". Needs the game
//! install (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Run with `--release`.
//! In mission 2, the selected workers repair the Bushan-Re from the menu's Repair, from a Z
//! band box around it and from a right click on it.

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
            g.input(
                InputEvent::MouseButton { left: true, down },
                &mut self.audio,
            );
            self.frame(g);
        }
    }
}

fn menu_labels(g: &Game) -> Vec<String> {
    g.ui.fe
        .game
        .ig
        .popup
        .items
        .iter()
        .map(|i| i.label.clone())
        .collect()
}

/// Opens the special-action menu with Z and clicks `label`.
fn z_menu_pick(rig: &mut Rig, g: &mut Game, label: &str) {
    rig.key(g, KEY_Z);
    assert_eq!(
        g.ui.fe.game.ig.popup.open,
        Some(MenuKind::Special),
        "Z opens the special-action menu"
    );
    let labels = menu_labels(g);
    let item =
        g.ui.fe
            .game
            .ig
            .popup
            .items
            .iter()
            .find(|i| i.label == label)
            .unwrap_or_else(|| panic!("no {label} in {labels:?}"));
    let r = item.rect;
    rig.click(g, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    assert!(g.ui.fe.game.ig.popup.open.is_none(), "the menu closes");
}

fn acolytes(g: &mut Game) -> [ShipId; 2] {
    let u = g.world.universe_mut();
    let ms = u.base.players[PLAYER].mothership.expect("mothership");
    let p = u.ship(ms).unwrap().posinfo.position;
    let mut make = |off: Vector| {
        let id =
            hwc_sim::base::create_ship(u, ShipType::sAcolyte, PLAYER, p + off, Default::default())
                .expect("acolyte");
        hwc_sim::construction::unit_cap_create_ship(u, id);
        id
    };
    [
        make(Vector::new(3000.0, 0.0, 0.0)),
        make(Vector::new(3400.0, 300.0, 200.0)),
    ]
}

fn acvs(g: &Game) -> Vec<ShipId> {
    let u = g.world.universe();
    u.ship_list
        .iter()
        .copied()
        .filter(|&id| {
            u.ship(id)
                .is_some_and(|s| s.ship_type == ShipType::sACV && s.player == PLAYER)
        })
        .collect()
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
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == hwc_sim::mission::sp_state::NONE)
    {
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
    assert!(
        labels.iter().any(|l| l == "Unlink") && !labels.iter().any(|l| l == "Link"),
        "{labels:?}"
    );
}

// ---- Mission 2: repairing the Bushan-Re -----------------------------------------------------

/// The Shift key (`Multiple_Select`, with which the user-requested right click opens the menu).
const KEY_SHIFT: u16 = 0x10;

impl Rig {
    fn mission2(&mut self) -> Box<Game> {
        let mut l = Loading::new(self.data.clone(), &self.session, 2, 2, SIZE).expect("loading");
        l.opening_nis = false;
        l.loader.play_nislets = false;
        for _ in 0..40 {
            match l.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("mission 2 did not start: {e}"),
            }
        }
        panic!("loading never finished");
    }

    fn right_click(&mut self, g: &mut Game, x: i32, y: i32) {
        g.input(InputEvent::MouseMove { x, y }, &mut self.audio);
        self.frame(g);
        for down in [true, false] {
            g.input(
                InputEvent::MouseButton { left: false, down },
                &mut self.audio,
            );
            self.frame(g);
        }
    }

    /// Clicks the open menu's item `label`.
    fn menu_pick(&mut self, g: &mut Game, label: &str) {
        let labels = menu_labels(g);
        let r =
            g.ui.fe
                .game
                .ig
                .popup
                .items
                .iter()
                .find(|i| i.label == label)
                .map(|i| i.rect);
        let r = r.unwrap_or_else(|| panic!("no {label} in {labels:?}"));
        self.click(g, (r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
    }
}

/// The Bushan-Re (`kDestroyer`'s ship in the universe).
fn bushan_re(g: &Game) -> Option<ShipId> {
    let run = g.world.run();
    let team = run.game.level.team(hwc_kas::kas_hash("kDestroyer"))?;
    team.ships
        .iter()
        .copied()
        .find(|&id| run.universe.ship_list.contains(&id) && run.universe.ship(id).is_some())
}

fn health_percent(g: &Game, id: ShipId) -> f32 {
    let u = g.world.universe();
    let s = u.ship(id).expect("ship");
    let max = u
        .statics
        .ship(s.ship_type)
        .expect("statics")
        .health
        .max_health;
    100.0 * s.health / max
}

/// Some of the workers repair `target` (a special order on it, the repair role).
fn repairing(g: &Game, workers: &[ShipId], target: ShipId) -> bool {
    let u = g.world.universe();
    workers.iter().any(|&w| {
        hwc_sim::ships::worker_repair::is_repairing(u, w)
            && u.ship(w)
                .and_then(|s| s.command)
                .and_then(|c| u.commands.get(c))
                .is_some_and(|c| {
                    c.order == hwc_sim::command::order::SPECIAL
                        && c.special_targets.contains(&target)
                })
    })
}

/// The middle of the ship on screen and its half width.
fn on_screen(g: &Game, id: ShipId) -> (i32, i32, i32) {
    let s = g.world.universe().ship(id).expect("ship");
    let (lo, hi) = g
        .world
        .screen_box((SIZE.0 as u32, SIZE.1 as u32), s)
        .expect("the ship is on screen");
    (
        ((lo[0] + hi[0]) * 0.5) as i32,
        ((lo[1] + hi[1]) * 0.5) as i32,
        ((hi[0] - lo[0]) * 0.5) as i32,
    )
}

fn stop(rig: &mut Rig, g: &mut Game, workers: &[ShipId], target: ShipId) {
    hwc_sim::command::cl_halt(g.world.universe_mut(), workers);
    rig.frame(g);
    assert!(!repairing(g, workers, target));
}

/// Mission 2 from the warp fleet with repair researched and the workers upgraded: once the
/// Bushan-Re is found, Shift + right click on it and the menu's Special Action..., Repair; the
/// Z band box around it and Repair; and a plain right click on it (over which the repair
/// pointer shows) each send the selected workers to repair it. The repairs then heal it to
/// 90 %, which ends the "Repair the BUSHAN-RE" objective.
#[test]
fn mission2_workers_repair_the_bushan_re() {
    let Some(mut rig) = rig() else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission2();
    let g = &mut *game;
    let mut n = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == hwc_sim::mission::sp_state::NONE
        && bushan_re(g).is_some())
    {
        rig.frame(g);
        n += 1;
        assert!(n < 16 * 90, "the fleet or the Bushan-Re never arrived");
    }
    let bushan = bushan_re(g).expect("Bushan-Re");
    // Repair researched; the workers (three more beside the warp fleet's) upgraded and moved
    // beside the Bushan-Re, which finds it.
    let u = g.world.universe_mut();
    let repair = Technology::from_name("TECH_REPAIR").expect("TECH_REPAIR");
    u.base.players[PLAYER].tech.add(repair.index());
    hwc_sim::upgrade::player_update_from_research(u, PLAYER);
    let target = u.ship(bushan).unwrap().posinfo.position;
    let ms = u.base.players[PLAYER].mothership.expect("mothership");
    let home = u.ship(ms).unwrap().posinfo.position;
    for k in 0..3 {
        let p = home + Vector::new(3000.0 + 400.0 * k as f32, 0.0, 0.0);
        let id = hwc_sim::base::create_ship(u, ShipType::sWorker, PLAYER, p, Default::default())
            .expect("worker");
        hwc_sim::construction::unit_cap_create_ship(u, id);
    }
    let workers: Vec<ShipId> = u
        .ship_list
        .iter()
        .copied()
        .filter(|&id| {
            u.ship(id)
                .is_some_and(|s| s.ship_type == ShipType::sWorker && s.player == PLAYER)
        })
        .collect();
    assert!(workers.len() >= 3, "{workers:?}");
    for (k, &w) in workers.iter().enumerate() {
        hwc_sim::upgrade::apply_upgrades(u, w);
        assert!(
            hwc_sim::upgrade::has_upgrade(u.ship(w).unwrap(), 0x4_0000),
            "the repair upgrade"
        );
        let p = target + Vector::new(-3000.0 - 400.0 * k as f32, 2500.0, 300.0 * k as f32);
        u.ships.ship_mut(w).posinfo.position = p;
    }
    let mut n = 0;
    while g.world.run().game.objective("RepairDestroyer").is_none()
        || g.world.run().game.sp_state != hwc_sim::mission::sp_state::NONE
    {
        rig.frame(g);
        n += 1;
        assert!(n < 16 * 600, "the repair objective never came");
    }
    for _ in 0..16 * 15 {
        rig.frame(g);
    }
    assert_eq!(
        g.world.universe().ship(bushan).unwrap().player,
        PLAYER,
        "the Bushan-Re is the player's"
    );
    let start = health_percent(g, bushan);
    assert!(start < 40.0, "{start}");
    if g.sensors.is_open() {
        g.toggle_sensors(&mut rig.audio);
        for _ in 0..16 * 4 {
            rig.frame(g);
        }
    }
    assert!(!g.sensors.is_open());
    assert!(
        g.right_click_move,
        "the user-requested right click is on by default"
    );

    let aim = |rig: &mut Rig, g: &mut Game| {
        g.world.focus_on(vec![bushan], false);
        for _ in 0..32 {
            rig.frame(g);
        }
        g.selection = workers.clone();
        rig.frame(g);
        // The view takes the window's size as it draws.
        g.render(&mut rig.renderer, SIZE).expect("render");
        on_screen(g, bushan)
    };

    // The original's default scheme (`RightClick_CSA`) does the action under the pointer on a
    // right click; without it the user-requested right click does, and Shift asks it for the
    // menu.
    assert!(g.ui.fe.game.options.get_bool("RightClick_CSA"));
    g.ui.fe.game.options.set_bool("RightClick_CSA", false);

    // 1. Shift + right click on it: the menu, its Special Action..., Repair.
    let (x, y, _) = aim(&mut rig, g);
    g.input(InputEvent::KeyDown(KEY_SHIFT), &mut rig.audio);
    rig.right_click(g, x, y);
    g.input(InputEvent::KeyUp(KEY_SHIFT), &mut rig.audio);
    assert_eq!(
        g.ui.fe.game.ig.popup.open,
        Some(MenuKind::Main),
        "Shift + right click opens the menu"
    );
    rig.menu_pick(g, "Special Action...");
    assert_eq!(g.ui.fe.game.ig.popup.open, Some(MenuKind::Special));
    rig.menu_pick(g, "Repair");
    rig.frame(g);
    assert!(g.ui.fe.game.ig.popup.open.is_none());
    assert!(
        repairing(g, &workers, bushan),
        "the menu's Repair repairs the ship it was opened over"
    );
    stop(&mut rig, g, &workers, bushan);

    // 2. The Z band box around it, Repair.
    let (x, y, r) = aim(&mut rig, g);
    let r = r.max(8) + 4;
    g.input(InputEvent::KeyDown(KEY_Z), &mut rig.audio);
    g.input(InputEvent::MouseMove { x: x - r, y: y - r }, &mut rig.audio);
    rig.frame(g);
    let button = |down| InputEvent::MouseButton { left: true, down };
    g.input(button(true), &mut rig.audio);
    rig.frame(g);
    g.input(InputEvent::MouseMove { x: x + r, y: y + r }, &mut rig.audio);
    rig.frame(g);
    g.input(button(false), &mut rig.audio);
    rig.frame(g);
    g.input(InputEvent::KeyUp(KEY_Z), &mut rig.audio);
    rig.frame(g);
    assert_eq!(
        g.ui.fe.game.ig.popup.open,
        Some(MenuKind::Special),
        "the Z band box opens the special menu"
    );
    assert_eq!(g.special_targets, vec![bushan]);
    rig.menu_pick(g, "Repair");
    rig.frame(g);
    assert!(
        repairing(g, &workers, bushan),
        "Repair from the Z band box repairs the boxed ship"
    );
    stop(&mut rig, g, &workers, bushan);

    // 3. A right click on it, with the user-requested right click and then with the original
    // scheme: the repair pointer, and the workers repair it.
    for csa in [false, true] {
        g.ui.fe.game.options.set_bool("RightClick_CSA", csa);
        let (x, y, _) = aim(&mut rig, g);
        g.input(InputEvent::MouseMove { x, y }, &mut rig.audio);
        rig.frame(g);
        assert_eq!(
            g.cursor(),
            hwc_ui::ingame::cursor::CursorKind::Support,
            "the repair pointer over it (RightClick_CSA {csa})"
        );
        rig.right_click(g, x, y);
        assert!(g.ui.fe.game.ig.popup.open.is_none(), "no menu");
        assert!(
            repairing(g, &workers, bushan),
            "the right click repairs it (RightClick_CSA {csa})"
        );
        if !csa {
            stop(&mut rig, g, &workers, bushan);
        }
    }

    // The repairs heal it; past the ambush (which lifts the 75 % cap, set here) they reach
    // 90 % and the objective goes.
    let mut n = 0;
    while health_percent(g, bushan) < start + 5.0 {
        rig.frame(g);
        n += 1;
        assert!(
            n < 16 * 300,
            "no repair in {n} frames: {} %",
            health_percent(g, bushan)
        );
    }
    g.world.run_mut().kas.set_var(None, 0x8361_7ea8, 1);
    let mut n = 0;
    while g.world.run().game.objective("RepairDestroyer").is_some() {
        rig.frame(g);
        n += 1;
        assert!(
            n < 16 * 60 * 30,
            "the objective is still there at {} %",
            health_percent(g, bushan)
        );
    }
    let end = health_percent(g, bushan);
    assert!(end >= 89.0, "{end}");
    eprintln!("Bushan-Re repaired from {start:.1} % to {end:.1} %");
}
