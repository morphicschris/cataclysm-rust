//! Extraction of the missions from the player's `cataclysm.exe`, compared with the reference IR
//! exported by the Python tools (private data). Needs `HWC_GAME_DIR` (the install) and
//! `recovered/missions/ir` (or `HWC_KAS_MISSIONS_DIR`); skips cleanly when either is absent.

use std::path::PathBuf;
use std::time::Instant;

use hwc_kas::extract;
use hwc_kas::ir::{Block, Mission, Stmt};
use hwc_kas::json;

fn exe() -> Option<Vec<u8>> {
    let dir = PathBuf::from(std::env::var_os("HWC_GAME_DIR")?);
    let entry = std::fs::read_dir(&dir).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case("cataclysm.exe"))?;
    std::fs::read(entry.path()).ok()
}

fn ir_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HWC_KAS_MISSIONS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../recovered/missions"))
        .join("ir");
    dir.join("Mission01.json").exists().then_some(dir)
}

fn short(s: String) -> String {
    if s.len() > 700 {
        format!("{}...", &s[..700])
    } else {
        s
    }
}

fn diff_stmts(path: &str, a: &[Stmt], b: &[Stmt], out: &mut Vec<String>) {
    if a == b {
        return;
    }
    for (i, (x, y)) in a.iter().zip(b).enumerate() {
        if x != y {
            out.push(format!("{path}[{i}]:\n  got  {}\n  want {}", short(format!("{x:?}")), short(format!("{y:?}"))));
            return;
        }
    }
    out.push(format!("{path}: {} statements, want {}", a.len(), b.len()));
}

fn diff_block(path: &str, a: &Block, b: &Block, out: &mut Vec<String>) {
    if a.addr != b.addr {
        out.push(format!("{path}: addr {:x?} want {:x?}", a.addr, b.addr));
    }
    diff_stmts(path, &a.stmts, &b.stmts, out);
}

fn diff(a: &Mission, b: &Mission) -> Vec<String> {
    let mut out = Vec::new();
    if a.name_hash != b.name_hash {
        out.push(format!("name hash {:08x} want {:08x}", a.name_hash, b.name_hash));
    }
    if a.fsms.len() != b.fsms.len() {
        out.push(format!("{} FSMs, want {}", a.fsms.len(), b.fsms.len()));
    }
    for (i, (x, y)) in a.fsms.iter().zip(&b.fsms).enumerate() {
        let p = format!("fsm[{i}] {:08x}", y.hash);
        if x.hash != y.hash {
            out.push(format!("{p}: hash {:08x}", x.hash));
        }
        diff_block(&format!("{p} INIT"), &x.init, &y.init, &mut out);
        diff_block(&format!("{p} WATCH"), &x.watch, &y.watch, &mut out);
        if x.states.len() != y.states.len() {
            out.push(format!("{p}: {} states, want {}", x.states.len(), y.states.len()));
        }
        for (j, (s, t)) in x.states.iter().zip(&y.states).enumerate() {
            let q = format!("{p} state[{j}] {:08x}", t.hash);
            if s.hash != t.hash {
                out.push(format!("{q}: hash {:08x}", s.hash));
            }
            diff_block(&format!("{q} INIT"), &s.init, &t.init, &mut out);
            diff_block(&format!("{q} WATCH"), &s.watch, &t.watch, &mut out);
        }
    }
    diff_block("INIT", &a.init, &b.init, &mut out);
    diff_block("WATCH", &a.watch, &b.watch, &mut out);
    if a.lstrings.len() != b.lstrings.len() {
        out.push(format!("{} LSTRINGs, want {}", a.lstrings.len(), b.lstrings.len()));
    }
    for (i, (x, y)) in a.lstrings.iter().zip(&b.lstrings).enumerate() {
        if x.texts != y.texts || x.addr != y.addr {
            out.push(format!("LSTRING[{i}]: {:?} {:x?}, want {:?} {:x?}", x.texts, x.addr, y.texts, y.addr));
        }
    }
    out
}

#[test]
fn extracts_missions_like_the_reference() {
    let Some(exe) = exe() else {
        eprintln!("HWC_GAME_DIR not set or cataclysm.exe missing; skipping");
        return;
    };
    let t = Instant::now();
    let results = extract::extract_each(&exe).expect("extraction");
    let took = t.elapsed();
    eprintln!("extracted {} missions in {:.1} ms", results.len(), took.as_secs_f64() * 1000.0);
    for r in &results {
        if let Err(e) = r {
            eprintln!("FAILED {e}");
        }
    }
    let missions: Vec<&Mission> = results.iter().flatten().collect();
    assert_eq!(missions.len(), results.len(), "every mission lowers");
    assert_eq!(missions.len(), 18);

    let Some(dir) = ir_dir() else {
        eprintln!("reference IR not found; skipping the comparison");
        return;
    };
    let mut bad = Vec::new();
    for m in &missions {
        let src = std::fs::read_to_string(dir.join(format!("{}.json", m.name))).unwrap_or_else(|_| panic!("{} reference", m.name));
        let want = json::load_mission(&src).unwrap();
        let d = diff(m, &want);
        let states: usize = m.fsms.iter().map(|f| f.states.len()).sum();
        eprintln!("{}: {} FSMs, {} states, {} LSTRINGs: {}", m.name, m.fsms.len(), states, m.lstrings.len(), if d.is_empty() { "identical".to_string() } else { format!("{} differences", d.len()) });
        for x in d.iter().take(6) {
            eprintln!("  {x}");
        }
        if !d.is_empty() {
            bad.push(m.name.clone());
        }
    }
    assert!(bad.is_empty(), "missions differing from the reference: {bad:?}");
}
