//! Tests over the recovered Cataclysm missions (private data, not in a public checkout).
//!
//! Looks for `recovered/missions` (the `.kas` text and the `ir/*.json` export) under the workspace
//! root, or in `HWC_KAS_MISSIONS_DIR`; skips cleanly when absent.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use hwc_kas::interp::Interpreter;
use hwc_kas::ir::{Fidelity, Mission};
use hwc_kas::{binary, json, kas_hash, print, text, Cx, FuncId, KasHost, KasRuntime, Value};

fn missions_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HWC_KAS_MISSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../recovered/missions"));
    if dir.join("Mission01.kas").exists() {
        Some(dir)
    } else {
        eprintln!("recovered missions not found ({}); skipping", dir.display());
        None
    }
}

const NAMES: [&str; 18] = [
    "Mission01", "Mission02", "Mission03", "Mission04", "Mission05", "Mission06", "Mission07", "Mission08",
    "Mission09", "Mission10", "Mission11", "Mission12", "Mission13", "Mission14", "Mission15", "Mission16",
    "Mission17", "Tutorial1",
];

fn load_text(dir: &std::path::Path, name: &str) -> Mission {
    let src = std::fs::read_to_string(dir.join(format!("{name}.kas"))).unwrap();
    text::parse_mission(name, &src).unwrap_or_else(|e| panic!("{name}.kas: {e}"))
}

fn load_json(dir: &std::path::Path, name: &str) -> Option<Mission> {
    let p = dir.join("ir").join(format!("{name}.json"));
    let src = std::fs::read_to_string(&p).ok()?;
    Some(json::load_mission(&src).unwrap_or_else(|e| panic!("{}: {e}", p.display())))
}

/// Code only (names, LSTRING display names and fidelity can legitimately differ).
fn same_code(a: &Mission, b: &Mission) -> bool {
    a.fsms == b.fsms && a.init == b.init && a.watch == b.watch && a.lstrings.len() == b.lstrings.len()
        && a.lstrings.iter().zip(&b.lstrings).all(|(x, y)| x.texts == y.texts)
}

/// A headless game: records every call, advances time one universe frame (1/16 s) per frame and
/// runs KAS every 16 frames; runtime functions return a deterministic pseudo-random small value
/// so missions take varied paths.
#[derive(Default)]
struct Mock {
    now: f32,
    seed: u32,
    calls: Vec<(u64, FuncId, String, i32)>,
    counts: BTreeMap<FuncId, usize>,
    warnings: usize,
    states: Vec<(u32, u32)>,
    noisy: bool,
}

impl Mock {
    fn next(&mut self) -> u32 {
        self.seed = self.seed.wrapping_mul(1_103_515_245).wrapping_add(12345);
        self.seed >> 16
    }
}

impl KasRuntime for Mock {
    fn unhandled(&mut self, _cx: &Cx, _func: FuncId) -> i32 {
        if self.noisy {
            (self.next() % 4) as i32
        } else {
            0
        }
    }
}

impl KasHost for Mock {
    fn universe_time(&mut self) -> f32 {
        self.now
    }
    fn random(&mut self, _stream: u32) -> u32 {
        self.next()
    }
    fn trace_call(&mut self, cx: &Cx, func: FuncId, args: &[Value<'_>], result: i32) {
        *self.counts.entry(func).or_default() += 1;
        if self.calls.len() < 200_000 {
            let a: Vec<String> = args.iter().map(|v| v.to_string()).collect();
            self.calls.push((cx.tick, func, a.join(","), result));
        }
    }
    fn state_changed(&mut self, team: u32, state: u32) {
        self.states.push((team, state));
    }
    fn warning(&mut self, _cx: &Cx, _message: &str) {
        self.warnings += 1;
    }
}

fn run(m: Arc<Mission>, host: &mut Mock, ticks: u64) -> Interpreter {
    let mut it = Interpreter::new(m);
    it.start(host);
    for _ in 0..ticks {
        host.now += 1.0; // 16 frames at 16 Hz
        it.execute(host);
    }
    it
}

#[test]
fn text_and_json_load_and_agree_structurally() {
    let Some(dir) = missions_dir() else { return };
    let mut json_count = 0;
    for name in NAMES {
        let t = load_text(&dir, name);
        assert_eq!(t.fidelity, Fidelity::Text);
        assert!(!t.fsms.is_empty(), "{name}: no FSMs");
        let Some(j) = load_json(&dir, name) else { continue };
        json_count += 1;
        assert_eq!(j.name_hash, kas_hash(name));
        let fsm_states = |m: &Mission| -> Vec<(u32, Vec<u32>)> {
            m.fsms.iter().map(|f| (f.hash, f.states.iter().map(|s| s.hash).collect())).collect()
        };
        assert_eq!(fsm_states(&t), fsm_states(&j), "{name}: FSM/state structure differs");
        assert_eq!(t.lstrings.len(), j.lstrings.len(), "{name}");
        for (a, b) in t.lstrings.iter().zip(&j.lstrings) {
            assert_eq!(a.texts, b.texts, "{name}");
            assert_eq!(a.addr, b.addr, "{name}");
        }
        // block addresses agree
        let addrs = |m: &Mission| m.blocks().map(|(_, b)| b.addr).collect::<Vec<_>>();
        assert_eq!(addrs(&t), addrs(&j), "{name}: block addresses");
    }
    eprintln!("compared {json_count} JSON exports with the text form");
}

#[test]
fn print_parse_and_binary_round_trips() {
    let Some(dir) = missions_dir() else { return };
    for name in NAMES {
        let mut forms = vec![load_text(&dir, name)];
        forms.extend(load_json(&dir, name));
        for m in forms {
            let printed = print::print_mission(&m);
            let back = text::parse_mission(name, &printed).unwrap_or_else(|e| panic!("{name} reprint ({:?}): {e}", m.fidelity));
            if !same_code(&m, &back) {
                for ((o, a), (_, b)) in m.blocks().zip(back.blocks()) {
                    if a != b {
                        for (x, y) in a.stmts.iter().zip(&b.stmts) {
                            if x != y {
                                panic!("{name} ({:?}) {o:?}: print/parse changed
{x:?}
{y:?}", m.fidelity);
                            }
                        }
                        panic!("{name} ({:?}) {o:?}: statement count changed", m.fidelity);
                    }
                }
                panic!("{name} ({:?}): print/parse changed the mission", m.fidelity);
            }
            let bin = binary::encode(&m);
            let dec = binary::decode(&bin).unwrap();
            assert_eq!(dec, m, "{name}: binary round trip");
        }
    }
}

#[test]
fn every_mission_runs_headless() {
    let Some(dir) = missions_dir() else { return };
    for name in NAMES {
        let mut forms = vec![load_text(&dir, name)];
        forms.extend(load_json(&dir, name));
        for m in forms {
            let fid = m.fidelity;
            for noisy in [false, true] {
                let m = Arc::new(m.clone());
                let mut host = Mock { noisy, seed: 7, ..Default::default() };
                let it = run(m.clone(), &mut host, 3000);
                let script_teams = it.teams().filter(|t| t.script).count();
                assert!(script_teams > 0, "{name} {fid:?}: no FSM created");
                let calls: usize = host.counts.values().sum();
                eprintln!(
                    "{name:9} {fid:?} noisy={noisy}: {} teams ({script_teams} scripted), {calls} calls, {} jumps, {} vars, {} timers, {} warnings",
                    it.teams().count(),
                    host.states.len(),
                    it.vars().len(),
                    it.timers().len(),
                    host.warnings
                );
            }
        }
    }
}

#[test]
fn mission01_opening() {
    let Some(dir) = missions_dir() else { return };
    let mut forms = vec![load_text(&dir, "Mission01")];
    forms.extend(load_json(&dir, "Mission01"));
    for m in forms {
        let m = Arc::new(m);
        let mut host = Mock::default();
        let mut it = Interpreter::new(m.clone());
        it.start(&mut host);
        let fleet_intel = kas_hash("FleetIntel");
        let objectives = kas_hash("Objectives");
        // Init_Mission01 creates the two top-level FSMs; FleetIntel's INIT jumps to NISLET1.
        let fi = it.team(fleet_intel).expect("FleetIntel team");
        assert_eq!(fi.fsm, Some(fleet_intel));
        assert_eq!(fi.state, Some(kas_hash("NISLET1")));
        assert_eq!(it.team(objectives).and_then(|t| t.fsm), Some(objectives));
        assert_eq!(it.teams().filter(|t| t.script).count(), 2);
        // NISLET1's INIT asked for the nislet (a G_ variable at mission scope).
        assert_eq!(it.var(None, hwc_kas::var_hash("G_PlayNislet")), Some(1));
        // FleetIntel-scope variables are keyed by the team label.
        assert_eq!(it.var(Some(fleet_intel), kas_hash("Failed")), Some(0));

        // Nothing happens until the nislet reports back.
        for _ in 0..5 {
            host.now += 1.0;
            it.execute(&mut host);
        }
        assert_eq!(it.team(fleet_intel).unwrap().state, Some(kas_hash("NISLET1")));
        it.set_var(None, 0x8c64_1d25, 1);
        host.now += 1.0;
        it.execute(&mut host);
        let fi = it.team(fleet_intel).unwrap();
        assert_eq!(fi.state, Some(kas_hash("WAITMessage")), "{:?}", m.fidelity);
        // ... and the opening created the battle FSMs.
        assert!(it.team(kas_hash("KushanCarriers")).is_some_and(|t| t.fsm == Some(kas_hash("KushanCarriers"))));
        assert!(it.teams().filter(|t| t.script).count() > 30);
        // StartLevelByHyperspacingShips and MusicPlay(36) were called by FleetIntel.
        let music = FuncId::by_name("MusicPlay").unwrap();
        assert!(host.calls.iter().any(|(_, f, a, _)| *f == music && a == "36"));
        assert!(host.counts.contains_key(&FuncId::by_name("StartLevelByHyperspacingShips").unwrap()));
        // WAITMessage's INIT does VarSet("G_RecentlyLoaded", 0): VarSet never creates.
        assert_eq!(it.var(None, hwc_kas::var_hash("G_RecentlyLoaded")), None);
    }
}

/// The exact (JSON) and text forms produce the same call trace for the opening of every mission
/// as long as the text form's known losses (IFONCE guard hashes, Random switches) are not
/// involved; IFONCE guard hashes are matched up as they appear.
#[test]
fn text_and_json_traces_agree() {
    let Some(dir) = missions_dir() else { return };
    let mut checked = 0;
    let mut identical = 0;
    for name in NAMES {
        let Some(j) = load_json(&dir, name) else { continue };
        let t = load_text(&dir, name);
        let mut hj = Mock { seed: 1, ..Default::default() };
        let mut ht = Mock { seed: 1, ..Default::default() };
        run(Arc::new(j), &mut hj, 400);
        run(Arc::new(t), &mut ht, 400);
        let mut map: HashMap<String, String> = HashMap::new();
        let n = hj.calls.len().min(ht.calls.len());
        let mut diverged = None;
        for i in 0..n {
            let (a, b) = (&hj.calls[i], &ht.calls[i]);
            if a.0 != b.0 || a.1 != b.1 || a.3 != b.3 {
                diverged = Some(i);
                break;
            }
            if a.2 != b.2 {
                // synthetic IFONCE guard (#7xxxxxxx) in the text form
                let ok = b.2.starts_with("#7") && map.entry(b.2.split(',').next().unwrap().to_string()).or_insert_with(|| a.2.split(',').next().unwrap().to_string()) == a.2.split(',').next().unwrap();
                if !ok {
                    diverged = Some(i);
                    break;
                }
            }
        }
        match diverged {
            None => {
                assert_eq!(hj.calls.len(), ht.calls.len(), "{name}: trace lengths differ");
                eprintln!("{name}: {n} calls identical");
                identical += 1;
            }
            Some(i) => {
                let (a, b) = (&hj.calls[i], &ht.calls[i]);
                let random = FuncId::by_name("Random").unwrap();
                // the known losses of the text form
                let why = if a.1 == random || b.1 == random {
                    "switch on Random() rendered as an IF chain"
                } else if b.2.starts_with("#7") {
                    "hand-written once-guard folded into IFONCE"
                } else if a.1 == FuncId::by_name("dbgAssert").unwrap() {
                    "-(cond) value rendered as plain arithmetic"
                } else {
                    panic!("{name}: unexpected divergence at call {i}: json {a:?} vs text {b:?}");
                };
                eprintln!("{name}: diverges at call {i} ({why})");
            }
        }
        checked += 1;
    }
    assert!(identical >= 10, "only {identical} identical traces");
    eprintln!("checked {checked} missions");
}

/// Mission discovery straight from the player's executable (`HWC_GAME_DIR`; read only).
#[test]
fn finds_missions_in_the_executable() {
    let Some(game) = std::env::var_os("HWC_GAME_DIR") else {
        eprintln!("HWC_GAME_DIR not set; skipping");
        return;
    };
    let game = PathBuf::from(game);
    let exe = [game.join("cataclysm.exe"), game.join("Cataclysm").join("cataclysm.exe")].into_iter().find(|p| p.exists());
    let Some(exe) = exe else {
        eprintln!("cataclysm.exe not found under HWC_GAME_DIR; skipping");
        return;
    };
    let bytes = std::fs::read(exe).unwrap();
    let pe = hwc_kas::extract::PeImage::parse(&bytes).unwrap();
    let found = hwc_kas::extract::find_missions(&pe);
    assert_eq!(found.len(), 18, "{found:?}");
    if let Some(dir) = missions_dir() {
        for e in &found {
            let name = e.name.replace("mission", "Mission").replace("tutorial", "Tutorial");
            if let Some(j) = load_json(&dir, &name) {
                assert_eq!(Some(e.init), j.init.addr, "{name}");
                assert_eq!(Some(e.watch), j.watch.addr, "{name}");
            }
        }
    }
}
