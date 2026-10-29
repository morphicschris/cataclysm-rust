//! Game-data checks of saving and loading through the game's own screens: mission 1 is saved
//! from the pause menu's save screen (with the overwrite question), loaded back behind the
//! loading screen, and run on next to the game that was saved, update for update; the load
//! screen lists the game with its mission and date and deletes it; the main menu's Load Game
//! list reads the same folder; quick save and load are off by default. Needs the game install
//! (`HWC_GAME_DIR`) and a GPU adapter; skips otherwise. Slow in a debug build: run with
//! `--release`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_render::Renderer;
use hwc_sim::harness::world_hash;
use hwc_sim::mission::sp_state;
use hwc_ui::canvas::Rect;
use hwc_ui::ingame::modern_menu::{self as menu, Kind};
use hwc_ui::ingame::{InputEvent, Manager};
use hwc_ui::saves::SaveFolder;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

use crate::game::{Game, Outcome};
use crate::play::{Loading, Progress};
use crate::saves::SaveDir;

const DT: f32 = 1.0 / 16.0;
const SIZE: (i32, i32) = (1024, 768);

struct Rig {
    data: Rc<GameData>,
    session: Session,
    renderer: Renderer,
    audio: Option<crate::audio::Audio>,
    saves: Rc<SaveDir>,
    /// Every outcome the frames reported.
    seen: Vec<Outcome>,
}

/// A fresh save folder under the build directory.
fn save_dir(name: &str) -> Rc<SaveDir> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/save-checks")
        .join(name);
    let _ = std::fs::remove_dir_all(&root);
    Rc::new(SaveDir::new(root))
}

fn rig(name: &str) -> Option<Rig> {
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
    let saves = save_dir(name);
    session.fe.game.saves = Some(saves.clone());
    Some(Rig {
        data,
        session,
        renderer,
        audio: None,
        saves,
        seen: Vec::new(),
    })
}

impl Rig {
    fn finish(&mut self, mut l: Loading) -> Box<Game> {
        l.saves = Some(self.saves.clone());
        for _ in 0..40 {
            match l.advance(&self.session, Some(&mut self.renderer), DT) {
                Progress::Working => {}
                Progress::Ready(g) => return g,
                Progress::Abandon(e) => panic!("the game did not start: {e}"),
            }
        }
        panic!("loading never finished");
    }

    fn mission_one(&mut self) -> Box<Game> {
        let mut l = Loading::new(self.data.clone(), &self.session, 1, 2, SIZE).expect("loading");
        l.opening_nis = false;
        l.loader.play_nislets = false;
        self.finish(l)
    }

    fn load(&mut self, path: &std::path::Path) -> Box<Game> {
        let l = Loading::from_save(self.data.clone(), &self.session, path, SIZE)
            .expect("loading the save");
        self.finish(l)
    }

    fn frame(&mut self, g: &mut Game) -> Vec<Outcome> {
        let out = g.update(DT, &mut self.audio);
        assert!(
            !out.contains(&Outcome::NextMission),
            "mission 1 ended: {out:?}"
        );
        self.seen.extend(out.iter().cloned());
        out
    }

    fn input(&mut self, g: &mut Game, ev: InputEvent) {
        g.input(ev, &mut self.audio);
    }

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
        let l = menu::layout(&m, SIZE.0, SIZE.1);
        let labels: Vec<String> = m.items.iter().map(|i| i.label.clone()).collect();
        let i = m
            .find(label)
            .unwrap_or_else(|| panic!("no {label} on {} ({labels:?})", m.name));
        let r = match m.items[i].kind {
            Kind::Toggle { .. } => l.placed[i].row,
            _ => l.placed[i].control,
        };
        let (x, y) = centre(r);
        self.click(g, x, y);
    }

    /// Clicks the save list's row named `name`.
    fn select_row(&mut self, g: &mut Game, name: &str) {
        let m = menu::model(&g.ui.fe).expect("a modern menu");
        let l = menu::layout(&m, SIZE.0, SIZE.1);
        let (i, k) = m
            .items
            .iter()
            .enumerate()
            .find_map(|(i, it)| match &it.kind {
                Kind::List { rows, .. } => rows.iter().position(|r| r.text == name).map(|k| (i, k)),
                _ => None,
            })
            .unwrap_or_else(|| panic!("no row {name}"));
        let (x, y) = centre(l.placed[i].parts[k]);
        self.click(g, x, y);
        assert!(
            list_rows_selected(g).as_deref() == Some(name),
            "{name} selected"
        );
    }

    fn key(&mut self, g: &mut Game, code: u16) {
        self.input(g, InputEvent::KeyDown(code));
        self.frame(g);
        self.input(g, InputEvent::KeyUp(code));
        self.frame(g);
    }

    fn type_text(&mut self, g: &mut Game, text: &str) {
        // Into the name box.
        let m = menu::model(&g.ui.fe).expect("a modern menu");

        let l = menu::layout(&m, SIZE.0, SIZE.1);
        if let Some(i) = m
            .items
            .iter()
            .position(|it| matches!(it.kind, Kind::Entry { .. }))
        {
            let (x, y) = centre(l.placed[i].control);
            self.click(g, x, y);
        }
        // A key press and its character, as the window sends them.
        for c in text.chars() {
            let vk = c.to_ascii_uppercase() as u16;
            self.input(g, InputEvent::KeyDown(vk));
            self.input(g, InputEvent::Char(c));
            self.frame(g);
            self.input(g, InputEvent::KeyUp(vk));
        }
        self.frame(g);
    }

    /// Escape until the pause menu is up.
    fn pause_menu(&mut self, g: &mut Game) {
        g.render(&mut self.renderer, SIZE).expect("render");
        for _ in 0..3 {
            if g.ui.manager() == Some(Manager::EscMenu) {
                return;
            }
            self.key(g, 27);
        }
        assert_eq!(g.ui.manager(), Some(Manager::EscMenu));
    }

    /// Pause menu, SAVE GAME, the name, and the save button.
    fn save_from_menu(&mut self, g: &mut Game, name: &str) {
        self.pause_menu(g);
        self.press(g, "SAVE GAME");
        assert_eq!(screen(g), "In_game_save");
        self.type_text(g, name);
        // The save screen's own button (the pause menu's label for it, too).
        self.press(g, "SAVE GAME");
    }
}

fn centre(r: Rect) -> (i32, i32) {
    ((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2)
}

fn screen(g: &Game) -> String {
    g.ui.fe.current_screen_name().unwrap_or("").to_string()
}

/// The rows of the save list on the screen.
fn list_rows(g: &Game) -> Vec<(String, Option<String>)> {
    let m = menu::model(&g.ui.fe).expect("a modern menu");
    m.items
        .iter()
        .find_map(|it| match &it.kind {
            Kind::List { rows, .. } => Some(
                rows.iter()
                    .map(|r| (r.text.clone(), r.detail.clone()))
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// The selected row of the save list on the screen.
fn list_rows_selected(g: &Game) -> Option<String> {
    let m = menu::model(&g.ui.fe)?;
    m.items.iter().find_map(|it| match &it.kind {
        Kind::List { rows, .. } => rows.iter().find(|r| r.selected).map(|r| r.text.clone()),
        _ => None,
    })
}

/// Mission 1 until the fleet is in and the intro is over.
fn into_mission(rig: &mut Rig, g: &mut Game) {
    let mut frames = 0;
    while !(g.world.run().game.ships_have_arrived
        && g.world.run().game.sp_state == sp_state::NONE
        && !g.letterbox.fully
        && g.nis.is_none())
    {
        rig.frame(g);
        frames += 1;
        assert!(frames < 16 * 600, "mission 1 never left its intro");
    }
    for _ in 0..32 {
        rig.frame(g);
    }
}

/// World hashes by update counter over `frames` frames.
fn run_hashes(rig: &mut Rig, g: &mut Game, frames: usize) -> BTreeMap<u32, u64> {
    let mut h = BTreeMap::new();
    for _ in 0..frames {
        rig.frame(g);
        let u = g.world.universe();
        h.insert(u.update_counter, world_hash(u));
    }
    h
}

#[test]
fn mission_one_saves_and_loads_through_the_menus() {
    let Some(mut rig) = rig("menus") else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    assert!(
        g.saves.is_some(),
        "the loading screen hands the save folder over"
    );
    into_mission(&mut rig, g);
    // A selection and a hot-key group to come back.
    let mine: Vec<hwc_sim::ShipId> = g
        .world
        .universe()
        .ship_list
        .iter()
        .copied()
        .filter(|&id| {
            crate::host::selectable(g.world.universe(), id)
                && g.world.universe().ships.ship(id).player == crate::host::PLAYER
        })
        .take(3)
        .collect();
    assert!(!mine.is_empty());
    // (Escape with a selection would clear it first.)
    rig.pause_menu(g);
    g.selection = mine.clone();
    g.hotkeys.assign(2, &mine);
    g.world.camera.distance = 5432.5;

    rig.save_from_menu(g, "Test Save");
    assert_eq!(g.ui.manager(), None, "saving closes the menu");
    assert!(!g.paused, "and the game goes on");
    let path = rig.saves.path_for(SaveFolder::SinglePlayer, "Test Save");
    assert!(path.is_file(), "{} written", path.display());
    let h = hwc_sim::save::read_header(&path).unwrap();
    assert_eq!(
        (h.description.as_str(), h.mission, h.autosave),
        ("Test Save", 1, false)
    );
    // The save was made from the menu's request, before this frame's update.
    let saved_counter = h.frame;
    assert!(g.world.universe().update_counter - saved_counter <= 1);
    let assets = crate::data::Assets(rig.data.clone());
    let relink = crate::saves::relink(&assets, g.world.universe().statics.clone(), 1).unwrap();
    let saved_ui = crate::saves::read(&path, &relink).unwrap().1.ui;
    let bytes = std::fs::metadata(&path).unwrap().len();
    eprintln!("saved at update {saved_counter}: {bytes} bytes");

    // The game that was saved runs on.
    let expected = run_hashes(&mut rig, g, 160);

    // Loading puts the game back as it was saved, and it runs on exactly the same.
    let mut loaded = rig.load(&path);
    let l = &mut *loaded;
    assert_eq!(l.world.universe().update_counter, saved_counter);
    assert_eq!(l.ui_state(), saved_ui, "the interface comes back");
    assert_eq!(l.selection, mine);
    assert_eq!(l.hotkeys.group(2), &mine[..]);
    assert_eq!(l.world.camera.distance, 5432.5);
    let got = run_hashes(&mut rig, l, 160);
    let mut compared = 0;
    for (n, h) in &got {
        if let Some(e) = expected.get(n) {
            assert_eq!(h, e, "update {n} differs after loading");
            compared += 1;
        }
    }
    assert!(compared > 100, "only {compared} updates compared");

    // Saving under the same name asks first: no keeps the file, yes overwrites it.
    rig.save_from_menu(l, "Test Save");
    assert_eq!(screen(l), hwc_ui::ingame::escmenu::OVERWRITE_POPUP);
    rig.press(l, "NO");
    assert_eq!(
        hwc_sim::save::read_header(&path).unwrap().frame,
        saved_counter,
        "kept"
    );
    rig.key(l, 27);
    rig.key(l, 27);
    rig.save_from_menu(l, "Test Save");
    assert_eq!(screen(l), hwc_ui::ingame::escmenu::OVERWRITE_POPUP);
    rig.press(l, "YES");
    let now = hwc_sim::save::read_header(&path).unwrap();
    assert!(now.frame > saved_counter, "overwritten");
    assert!(now.thumbnail.is_some(), "with a picture of the game");

    // The load screen lists it with its mission and date, and deletes it after asking.
    rig.save_from_menu(l, "Another One");
    rig.pause_menu(l);
    rig.press(l, "LOAD GAME");
    assert_eq!(screen(l), "In_game_load");
    let rows = list_rows(l);
    // Mission 1's own SaveLevel ("01 - Hiigara") is in the list too.
    let names: Vec<&str> = rows.iter().map(|r| r.0.as_str()).collect();
    assert!(names.ends_with(&["Another One", "Test Save"]), "{names:?}");
    assert!(
        rows.iter().all(|r| r
            .1
            .as_deref()
            .is_some_and(|d| d.starts_with("MISSION 01 - "))),
        "{rows:?}"
    );
    let before = rows.len();
    rig.select_row(l, "Another One");
    rig.press(l, "DELETE GAME");
    assert!(screen(l).starts_with("DeleteGamePopup"), "asks first");
    assert!(rig
        .saves
        .path_for(SaveFolder::SinglePlayer, "Another One")
        .exists());
    rig.press(l, "YES");
    assert_eq!(screen(l), "In_game_load");
    assert!(
        !rig.saves
            .path_for(SaveFolder::SinglePlayer, "Another One")
            .exists(),
        "deleted"
    );
    assert!(path.exists(), "the other one stays");
    // Loading from the load screen.
    if screen(l) != "In_game_load" {
        rig.pause_menu(l);
        rig.press(l, "LOAD GAME");
    }
    assert_eq!(list_rows(l).len(), before - 1);
    rig.select_row(l, "Test Save");
    rig.seen.clear();
    rig.press(l, "LOAD GAME");
    let out = &rig.seen;
    assert!(
        out.iter()
            .any(|o| matches!(o, Outcome::LoadGame(p) if *p == path)),
        "{out:?}"
    );

    // The main menu's Load Game screen reads the same folder.
    let fe = &mut rig.session.fe;
    let list = fe
        .game
        .saves
        .as_ref()
        .unwrap()
        .list(SaveFolder::SinglePlayer);
    let names: Vec<&str> = list.iter().map(|e| e.name.as_str()).collect();
    assert!(
        names.contains(&"Test Save") && !names.contains(&"Another One"),
        "{names:?}"
    );
    // Its screen: the list comes from the folder, and loading asks the game to load the file.
    fe.all_screens_delete();
    fe.screen_start("Load_game");
    let atom = fe.find_atom("FE_GameWindowInit").expect("the list");
    let rows: Vec<String> = fe
        .list(atom)
        .expect("a list")
        .items
        .iter()
        .map(|i| i.text.clone())
        .collect();
    assert_eq!(rows, names);
    let at = rows.iter().position(|r| r == "Test Save").unwrap();
    fe.list_mut(atom).unwrap().select(at);
    fe.function_execute("LoadMultiSinglePlayerGameFromMain", None, false);
    let key = path.to_string_lossy().into_owned();
    assert!(
        fe.game
            .requests
            .contains(&hwc_ui::cataclysm::Request::LoadGame(key)),
        "{:?}",
        fe.game.requests
    );
}

/// The save name's box has the keyboard (`regKeysFocussed`): a held `-` types and does not
/// zoom the camera, and the keypad's 5 types too.
#[test]
fn typing_a_save_name_keeps_the_keys() {
    let Some(mut rig) = rig("typing") else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    into_mission(&mut rig, g);
    rig.pause_menu(g);
    rig.press(g, "SAVE GAME");
    assert_eq!(screen(g), "In_game_save");
    rig.type_text(g, "ab");
    let distance = g.world.camera.distance;
    for (code, c) in [(0xbd, '-'), (0x65, '5')] {
        rig.input(g, InputEvent::KeyDown(code));
        rig.input(g, InputEvent::Char(c));
        for _ in 0..10 {
            rig.frame(g);
        }
        rig.input(g, InputEvent::KeyUp(code));
        rig.frame(g);
    }
    let fe = &g.ui.fe;
    let typed = fe
        .find_atom("IG_TextEntryWindowInit")
        .and_then(|a| fe.text_entry(a));
    assert_eq!(typed, Some("ab-5"));
    assert_eq!(g.world.camera.distance, distance, "the camera did not zoom");
}

#[test]
fn quick_save_and_autosave() {
    let Some(mut rig) = rig("quick") else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let mut game = rig.mission_one();
    let g = &mut *game;
    into_mission(&mut rig, g);
    let quick = rig
        .saves
        .path_for(SaveFolder::SinglePlayer, crate::saves::QUICK_SAVE);
    // Off by default: Shift+F5 is not a save.
    let shift = hwc_ui::consts::key::SHIFT;
    let quick_key = |rig: &mut Rig, g: &mut Game, code: u16| {
        rig.input(g, InputEvent::KeyDown(shift));
        rig.input(g, InputEvent::KeyDown(code));
        rig.input(g, InputEvent::KeyUp(code));
        rig.input(g, InputEvent::KeyUp(shift));
        rig.frame(g)
    };
    quick_key(&mut rig, g, crate::game::QUICK_SAVE_KEY);
    assert!(!quick.exists(), "quick save is opt-in");
    g.quick_save_keys = true;
    quick_key(&mut rig, g, crate::game::QUICK_SAVE_KEY);
    assert!(quick.is_file(), "Shift+F5 saves");
    rig.seen.clear();
    quick_key(&mut rig, g, crate::game::QUICK_LOAD_KEY);
    rig.frame(g);
    let out = &rig.seen;
    assert!(
        out.iter()
            .any(|o| matches!(o, Outcome::LoadGame(p) if *p == quick)),
        "Shift+F9 loads: {out:?}"
    );

    // A mission's SaveLevel: written at the end of its update, as `NN - name`, and what
    // restart loads; loading it counts the load (`G_TimesGameLoaded`) and rewrites it.
    // (Clear of mission 1's own SaveLevel, "01 - Hiigara" at update 256.)
    for _ in 0..32 {
        rig.frame(g);
    }
    assert!(
        rig.saves
            .path_for(SaveFolder::SinglePlayer, "01 - Hiigara")
            .is_file(),
        "mission 1 saves itself"
    );
    g.world.run_mut().game.pending_save = Some((2, "Test Autosave".into()));
    let at = g.world.universe().update_counter;
    while g.world.universe().update_counter == at {
        rig.frame(g);
    }

    let auto = rig
        .saves
        .path_for(SaveFolder::SinglePlayer, "02 - Test Autosave");

    assert!(auto.is_file(), "{} written", auto.display());
    assert_eq!(g.level_save.as_deref(), Some(auto.as_path()));
    assert!(hwc_sim::save::read_header(&auto).unwrap().autosave);
    assert_eq!(
        g.world.run().var(None, hwc_sim::mission::AUTO_SAVE),
        None,
        "G_AutoSave only in the file"
    );
    let mut l = rig.load(&auto);
    assert_eq!(
        l.world.run().var(None, hwc_sim::mission::TIMES_GAME_LOADED),
        Some(1)
    );
    assert_eq!(l.world.run().var(None, hwc_sim::mission::AUTO_SAVE), None);
    assert_eq!(l.level_save.as_deref(), Some(auto.as_path()));
    let again = rig.load(&auto);
    assert_eq!(
        again
            .world
            .run()
            .var(None, hwc_sim::mission::TIMES_GAME_LOADED),
        Some(2),
        "the file was rewritten"
    );
    rig.frame(&mut l);
}

/// Options kept in a mission are in the options file at once, go back to the front end when
/// the mission ends, and a restart (a new session from the file) starts the next mission with
/// them.
#[test]
fn options_kept_in_a_mission_start_the_next_one() {
    let Some(mut rig) = rig("options") else {
        eprintln!("skipped: HWC_GAME_DIR or a GPU adapter missing");
        return;
    };
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/save-checks/options-config/hwc-options.cfg");
    let _ = std::fs::remove_file(&path);
    rig.session.fe.game.config_path = Some(path.clone());
    let mut game = rig.mission_one();
    let ui = &mut game.ui.fe.game;
    assert!(!ui.options.get_bool("hwcFxaa"));
    ui.options.set("hwcFxaa", 1);
    ui.options.set("SpeechVolume", 33);
    ui.options_commit();
    let saved = hwc_ui::options::Options::load(&path).expect("the options file");
    assert_eq!(saved.get("hwcFxaa"), 1, "written on accept");
    assert_eq!(saved.get("SpeechVolume"), 33);
    // The mission ends: the front end has them (and writes them again).
    crate::app::hand_back_options(&mut rig.session, &game);
    drop(game);
    assert_eq!(rig.session.fe.game.options.get("hwcFxaa"), 1);
    // A restart: a new session from the file.
    let source: Rc<dyn AssetSource> = rig.data.clone();
    rig.session = Session::new(
        source,
        Setup {
            options_path: Some(path.clone()),
            original_cfg: None,
            default_keys: None,
            keys: None,
            keys_path: None,
            scale: ScaleMode::Original,
        },
    )
    .expect("session");
    rig.session.resize(SIZE.0 as u32, SIZE.1 as u32);
    rig.session.fe.game.saves = Some(rig.saves.clone());
    let game = rig.mission_one();
    assert!(
        game.quality.fxaa,
        "the next game starts with the kept option"
    );
    assert_eq!(game.ui.fe.game.options.get("SpeechVolume"), 33);
}
