//! Saved games resume bit-exactly (needs the game: `HWC_DATA_DIR` or `HWC_GAME_DIR`; skipped
//! when neither is set).
//!
//! Mission 1 is played by the scripted test player; at several moments (the fleet arriving
//! from hyperspace, an NIS waiting, fleet intelligence speaking, effects in flight, combat) the
//! game is saved, loaded and run on. Every update's world hash, and the whole state at the end,
//! must equal the uninterrupted run's.

use std::sync::{Arc, OnceLock};

use hwc_formats::gamedata::GameData;
use hwc_kas::Mission;
use hwc_sim::autoplay::Autoplay;
use hwc_sim::campaign::Carry;
use hwc_sim::effects::EffectData;
use hwc_sim::harness::world_hash;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, MissionConfig, MissionRun};
use hwc_sim::save::{self, Relink, SaveError, SaveHeader};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

struct Data {
    gd: GameData,
    statics: Arc<StaticWorld>,
    tweaks: SimTweaks,
    effects: EffectData,
}

fn data() -> Option<&'static Data> {
    static DATA: OnceLock<Option<Data>> = OnceLock::new();
    DATA.get_or_init(|| {
        let gd = match GameData::from_env() {
            Some(Ok(gd)) => gd,
            _ => {
                eprintln!("HWC_DATA_DIR / HWC_GAME_DIR not set; skipping");
                return None;
            }
        };
        let src = GameDataSource(&gd);
        let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
        let tweaks = SimTweaks::load(&src);
        let effects = EffectData::load_from(&src);
        Some(Data { gd, statics, tweaks, effects })
    })
    .as_ref()
}

fn script(d: &Data, m: u32) -> Mission {
    load_kas_mission(&d.gd, &mission_script_name(m)).expect("mission script")
}

fn config(m: u32) -> MissionConfig {
    let mut c = MissionConfig::new(m);
    c.log_calls = false;
    c.speech_seconds = Some(3.0);
    // The test plays the NIS requests itself (see `tick`), so that a save can be made while
    // one waits.
    c.hold_nis = true;
    c.effects = true;
    c
}

fn start(d: &Data, m: u32, carry: Option<&Carry>) -> MissionRun {
    let src = GameDataSource(&d.gd);
    match carry {
        None => MissionRun::start(d.statics.clone(), d.tweaks.clone(), &src, script(d, m), config(m)),
        Some(c) => MissionRun::start_campaign(d.statics.clone(), d.tweaks.clone(), &src, script(d, m), config(m), c),
    }
    .unwrap_or_else(|e| panic!("mission {m}: {e}"))
}

/// What a loader links to: the game data, and the mission script loaded afresh.
fn relink(d: &Data, m: u32) -> Relink {
    let mut r = Relink::new(d.statics.clone());
    r.effects = Some(d.effects.clone());
    r.mission = Some(Arc::new(script(d, m)));
    r
}

/// The player and the front end for one update: the test player's orders, the update, and an
/// NIS that has waited long enough "finishes playing".
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct Game {
    run: MissionRun,
    player: Autoplay,
    nis_since: Option<u32>,
}

impl Game {
    fn tick(&mut self) -> u64 {
        self.player.tick(&mut self.run);
        self.run.step();
        let now = self.run.universe.update_counter;
        if self.run.game.pending_nis.is_some() {
            let since = *self.nis_since.get_or_insert(now);
            if now - since >= 40 {
                self.run.nis_finished(false);
                self.nis_since = None;
            }
        }
        world_hash(&self.run.universe)
    }
}

fn header(g: &Game) -> SaveHeader {
    SaveHeader {
        description: "test".into(),
        mission: g.run.game.config.mission,
        game_time: g.run.universe.total_time_elapsed,
        frame: g.run.universe.update_counter,
        script_fingerprint: save::script_fingerprint(g.run.kas.mission()),
        ..SaveHeader::default()
    }
}

fn state_bytes(g: &Game) -> Vec<u8> {
    save::encode(&SaveHeader::default(), g).unwrap()
}

/// Names the parts of two games that encode differently.
fn differing_parts(a: &Game, b: &Game) -> String {
    fn enc<T: serde::Serialize>(v: &T) -> Vec<u8> {
        save::encode(&SaveHeader::default(), v).unwrap()
    }
    let mut out = Vec::new();
    let (ua, ub) = (&a.run.universe, &b.run.universe);
    let mut check = |name: &str, x: Vec<u8>, y: Vec<u8>| {
        if x != y {
            out.push(name.to_string());
        }
    };
    check("player", enc(&a.player), enc(&b.player));
    check("kas", enc(&a.run.kas), enc(&b.run.kas));
    check("game", enc(&a.run.game), enc(&b.run.game));
    check("ships", enc(&ua.ships), enc(&ub.ships));
    check("bullets", enc(&ua.bullets), enc(&ub.bullets));
    check("missiles", enc(&ua.missiles), enc(&ub.missiles));
    check("blobs", enc(&ua.blobs), enc(&ub.blobs));
    check("commands", enc(&ua.commands), enc(&ub.commands));
    check("base", enc(&ua.base), enc(&ub.base));
    check("randy", enc(&ua.randy), enc(&ub.randy));
    check("events", enc(&ua.events), enc(&ub.events));
    check("special_events", enc(&ua.special_events), enc(&ub.special_events));
    check("render_list", enc(&ua.render_list), enc(&ub.render_list));
    if let (Some(ea), Some(eb)) = (&ua.effects, &ub.effects) {
        check("fx.world.effects", enc(&ea.world.effects), enc(&eb.world.effects));
        check("fx.world.systems", enc(&ea.world.systems), enc(&eb.world.systems));
        check("fx.world.owners", enc(&ea.world.owners.iter().collect::<std::collections::BTreeMap<_, _>>()), enc(&eb.world.owners.iter().collect::<std::collections::BTreeMap<_, _>>()));
        check("fx.world.randy", enc(&ea.world.randy), enc(&eb.world.randy));
        check("fx.world.events", enc(&ea.world.events), enc(&eb.world.events));
        check("fx.world.warnings", enc(&ea.world.warnings), enc(&eb.world.warnings));
        check("fx.world", enc(&ea.world), enc(&eb.world));
        check("fx", enc(ea), enc(eb));
    }
    check("universe", enc(ua), enc(ub));
    out.join(", ")
}

struct Checkpoint {
    name: &'static str,
    /// First update at or after `after` where `when` holds.
    after: u32,
    when: fn(&MissionRun) -> bool,
    frame: Option<u32>,
    saved: Vec<u8>,
    /// The uninterrupted run's state `RUN_ON` updates after the save.
    expected: Vec<u8>,
}

const RUN_ON: u32 = 240;

fn cp(name: &'static str, after: u32, when: fn(&MissionRun) -> bool) -> Checkpoint {
    Checkpoint { name, after, when, frame: None, saved: Vec::new(), expected: Vec::new() }
}

fn effects_in_flight(r: &MissionRun) -> usize {
    r.universe.effects.as_ref().map_or(0, |e| e.world.effects.len())
}

#[test]
fn mission1_saves_resume_bit_exactly() {
    let Some(d) = data() else { return };
    let limit: u32 = std::env::var("HWC_SAVE_TEST_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(16 * 420);
    let mut checkpoints = vec![
        cp("first update", 0, |_| true),
        cp("hyperspace arrival", 1, |r| r.game.arriving.iter().any(|a| a.ship.is_some())),
        cp("NIS waiting", 1, |r| r.game.pending_nis.is_some()),
        cp("fleet intel speaking", 1, |r| r.game.speech_end.is_some()),
        cp("effects in flight", 200, |r| effects_in_flight(r) >= 12),
        cp("combat", 400, |r| r.universe.bullet_list.len() >= 4 || !r.universe.missile_list.is_empty()),
        cp("late", limit.saturating_sub(RUN_ON + 1), |_| true),
    ];
    let mut g = Game { run: start(d, 1, None), player: Autoplay::new(16 * 4500), nis_since: None };
    // `hashes[i]`: the world hash at update counter `c0 + i`.
    let c0 = g.run.universe.update_counter;
    let mut hashes = vec![world_hash(&g.run.universe)];
    for _ in 0..limit {
        let frame = g.run.universe.update_counter;
        for c in checkpoints.iter_mut() {
            if c.frame.is_none() && frame >= c.after && frame + RUN_ON < limit && (c.when)(&g.run) {
                c.frame = Some(frame);
                c.saved = save::encode(&header(&g), &g).unwrap();
            }
            if c.frame.is_some_and(|f| f + RUN_ON == frame) {
                c.expected = state_bytes(&g);
            }
        }
        hashes.push(g.tick());
        assert_eq!(hashes.len() as u32 - 1, g.run.universe.update_counter - c0);
    }
    let relink = relink(d, 1);
    let mut tested = 0;
    for c in &checkpoints {
        let Some(frame) = c.frame else {
            eprintln!("checkpoint {:?}: not reached in {limit} updates", c.name);
            continue;
        };
        let (h, mut loaded): (SaveHeader, Game) = save::decode(&c.saved, &relink).unwrap_or_else(|e| panic!("{}: {e}", c.name));
        assert_eq!(h.frame, frame);
        // Loading is lossless: the loaded game saves to the same bytes.
        assert_eq!(save::encode(&header(&loaded), &loaded).unwrap(), c.saved, "{}: re-encoding differs", c.name);
        let base = loaded.run.universe.update_counter;
        assert_eq!(world_hash(&loaded.run.universe), hashes[(base - c0) as usize], "{}", c.name);
        for i in 1..=RUN_ON {
            let got = loaded.tick();
            let want = hashes[(base + i - c0) as usize];
            assert_eq!(got, want, "{}: saved at update {frame}, diverged {i} updates after loading", c.name);
        }
        if state_bytes(&loaded) != c.expected {
            let (_, want): (SaveHeader, Game) = save::decode(&c.expected, &relink).unwrap();
            panic!("{}: the state differs {RUN_ON} updates after loading: {}", c.name, differing_parts(&loaded, &want));
        }
        eprintln!(
            "checkpoint {:?} at update {frame}: {} bytes saved, {} effects, {} bullets; resumed bit-exactly",
            c.name,
            c.saved.len(),
            effects_in_flight(&loaded.run),
            loaded.run.universe.bullet_list.len()
        );
        tested += 1;
    }
    assert!(tested >= 5, "only {tested} checkpoints were reached");
}

/// A game saved late in mission 1 and loaded carries the same fleet into mission 2, which then
/// runs exactly as after the uninterrupted mission.
#[test]
fn saves_carry_across_the_mission_boundary() {
    let Some(d) = data() else { return };
    let mut g = Game { run: start(d, 1, None), player: Autoplay::new(16 * 4500), nis_since: None };
    let limit = 16 * 6000;
    // Save every 2000 updates; keep the last save before the jump.
    let mut last_save: Option<(u32, Vec<u8>)> = None;
    for _ in 0..limit {
        if g.run.universe.update_counter % 2000 == 1000 {
            last_save = Some((g.run.universe.update_counter, save::encode(&header(&g), &g).unwrap()));
        }
        g.tick();
        if g.run.mission_complete() {
            break;
        }
    }
    if !g.run.mission_complete() {
        // Force the jump the way the campaign test does when the scripted orders run out.
        let _ = g.run.force_hyperspace();
        for _ in 0..16 * 120 {
            g.tick();
            if g.run.mission_complete() {
                break;
            }
            let _ = g.run.hyperspace_go_now();
        }
    }
    if !g.run.mission_complete() {
        eprintln!("mission 1 did not end within the test's updates; skipping the boundary check");
        return;
    }
    let carry = g.run.carry().expect("carry");
    let (frame, bytes) = last_save.expect("a save was made");
    let (_, mut loaded): (SaveHeader, Game) = save::decode(&bytes, &relink(d, 1)).unwrap();
    assert_eq!(loaded.run.universe.update_counter, frame);
    let forced = g.run.game.hyper.hyperspace_override;
    while !loaded.run.mission_complete() && loaded.run.universe.update_counter < g.run.universe.update_counter {
        loaded.tick();
        if forced && loaded.run.universe.update_counter >= 16 * 6000 {
            break;
        }
    }
    if !loaded.run.mission_complete() {
        // The uninterrupted run was forced: repeat the same steps.
        let _ = loaded.run.force_hyperspace();
        for _ in 0..16 * 120 {
            loaded.tick();
            if loaded.run.mission_complete() {
                break;
            }
            let _ = loaded.run.hyperspace_go_now();
        }
    }
    assert_eq!(loaded.run.universe.update_counter, g.run.universe.update_counter, "the loaded game jumps on the same update");
    let loaded_carry = loaded.run.carry().expect("carry from the loaded game");
    assert_eq!(loaded_carry.to_text(), carry.to_text(), "the same fleet reaches mission 2");

    let mut a = start(d, 2, Some(&carry));
    let mut b = start(d, 2, Some(&loaded_carry));
    for i in 0..16 * 60 {
        a.step();
        b.step();
        assert_eq!(world_hash(&a.universe), world_hash(&b.universe), "mission 2 diverged at update {i}");
    }
    // And mission 2 itself saves and resumes.
    let bytes = save::encode(&SaveHeader { mission: 2, script_fingerprint: save::script_fingerprint(a.kas.mission()), ..Default::default() }, &a).unwrap();
    let (_, mut c): (SaveHeader, MissionRun) = save::decode(&bytes, &relink(d, 2)).unwrap();
    for i in 0..RUN_ON {
        a.step();
        c.step();
        assert_eq!(world_hash(&a.universe), world_hash(&c.universe), "mission 2 save diverged {i} updates after loading");
    }
}

#[test]
fn loading_checks_the_version_and_the_script() {
    let Some(d) = data() else { return };
    let run = start(d, 1, None);
    let h = SaveHeader { mission: 1, script_fingerprint: save::script_fingerprint(run.kas.mission()), ..Default::default() };
    let mut bytes = save::encode(&h, &run).unwrap();
    // The mission 2 script is not the one the game was saved with.
    match save::decode::<MissionRun>(&bytes, &relink(d, 2)) {
        Err(SaveError::Script { mission: 1 }) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
    bytes[8] = bytes[8].wrapping_add(1);
    match save::decode::<MissionRun>(&bytes, &relink(d, 1)) {
        Err(e @ SaveError::Version { .. }) => assert!(e.to_string().contains("version"), "{e}"),
        other => panic!("{:?}", other.map(|_| ())),
    }
}

/// The mission's `SaveLevel`: handled at the end of the update that asked for it
/// (`kasfSaveLevelDeferred`), with the autosave's KAS globals; loading an autosave counts the
/// load (`kasAutoSaveOnLoad`).
#[test]
fn mission_autosave_requests() {
    let Some(d) = data() else { return };
    let mut g = Game { run: start(d, 1, None), player: Autoplay::new(16 * 4500), nis_since: None };
    let mut found = None;
    for _ in 0..16 * 120 {
        g.tick();
        if let Some(a) = g.run.game.autosave.clone() {
            found = Some(a);
            break;
        }
    }
    let mut run = g.run;
    let a = found.expect("mission 1 asks for a save");
    assert_eq!(a.file_name(), format!("{:02} - {}", a.level, a.name));
    assert!(run.game.pending_save.is_none(), "the request is taken in its own update");
    assert_eq!(run.var(None, hwc_sim::mission::AUTO_SAVE), Some(1));
    assert_eq!(run.var(None, hwc_sim::mission::TIMES_GAME_LOADED), Some(0));
    // The file holds G_AutoSave; the game goes on without it.
    let bytes = save::encode(&SaveHeader { autosave: true, ..header_of(&run) }, &run).unwrap();
    let mut on = run.clone();
    on.step();
    assert!(on.game.autosave.is_none());
    assert_eq!(on.var(None, hwc_sim::mission::AUTO_SAVE), None, "tidied up by the next update");
    run.autosave_written();
    assert_eq!(run.var(None, hwc_sim::mission::AUTO_SAVE), None);
    let (_, mut loaded): (SaveHeader, MissionRun) = save::decode(&bytes, &relink(d, 1)).unwrap();
    assert!(loaded.loaded(), "an autosave");
    assert_eq!(loaded.var(None, hwc_sim::mission::TIMES_GAME_LOADED), Some(1));
    loaded.autosave_written();
    assert!(!loaded.loaded(), "a save made after it is not one");
    // Both carry on the same.
    for i in 0..64 {
        run.step();
        loaded.step();
        assert_eq!(world_hash(&run.universe), world_hash(&loaded.universe), "update {i}");
    }
}

fn header_of(run: &MissionRun) -> SaveHeader {
    SaveHeader { mission: run.game.config.mission, script_fingerprint: save::script_fingerprint(run.kas.mission()), ..SaveHeader::default() }
}
