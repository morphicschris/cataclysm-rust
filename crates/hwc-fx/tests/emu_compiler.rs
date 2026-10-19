//! Compiler equivalence with `cataclysm.exe`: the original `etgEffectCodeLoad` was run in
//! Ghidra's p-code emulator (script `EtgDump.java`; file I/O, the heap, `sscanf`/`sprintf`,
//! `fmod` and the texture/mesh registries replaced by stubs) on a selection of effect files,
//! and the resulting effect statics dumped in a normalised text form. Here the same files are
//! compiled in the same order and the results compared field by field.
//!
//! Needs the effect files (`HWC_GAME_DIR` or `HWC_EXTRACTED_DIR`); skipped otherwise.

mod common;

use hwc_fx::bytecode::{self, op};
use hwc_fx::compiler::{EffectStatic, Library};
use hwc_fx::functions::{Fun, FUNCTION_TABLE};

fn fun_name(id: u32) -> String {
    let f = Fun::from_u32(id).expect("function id");
    match f {
        Fun::SetTrHandle => "SetTrHandle".into(),
        Fun::ModifyTexture => "ModifyTexture".into(),
        Fun::SetMeshdata => "SetMeshdata".into(),
        Fun::SpawnNewEffect => "SpawnNewEffect".into(),
        Fun::CreateEffects => "CreateEffects".into(),
        Fun::CreateCallbackSetup => "CreateCallbackSetup".into(),
        _ => FUNCTION_TABLE.iter().find(|e| e.fun == f).unwrap().name.to_string(),
    }
}

fn hex(v: u32) -> String {
    format!("{v:08x}")
}

/// The same normalised form `EtgDump.java` prints.
fn dump(lib: &Library, s: &EffectStatic) -> Vec<String> {
    let mut out = vec![format!(
        "S {} npb={} vs={} rs={} es={} nhist={} sops={} ndec={} clen={}",
        s.name,
        hex(s.n_particle_blocks),
        hex(s.variable_size),
        hex(s.rate_size),
        hex(s.effect_size),
        hex(s.n_history as u32),
        hex(s.special_ops),
        hex(s.decisions.len() as u32),
        hex(s.const_data.len() as u32)
    )];
    let mut l = String::from("I");
    for (i, v) in &s.var_init {
        l += &format!(" {i}={}", hex(*v));
    }
    out.push(l);
    let mut l = String::from("C ");
    for i in (0..s.const_data.len()).step_by(4) {
        l += &format!("{} ", hex(bytecode::rd(&s.const_data, i as u32)));
    }
    out.push(l);
    let mut l = String::from("D");
    for d in &s.decisions {
        l += &format!(" {}:{}:{}", d.kind, d.low, d.high);
    }
    out.push(l);
    for (b, code) in s.code.iter().enumerate() {
        let mut l = format!("B{b}");
        let mut at = 0u32;
        while (at as usize) < code.len() {
            let opc = bytecode::rd(code, at);
            let n = if opc == op::FUNCTION { 5 + 2 * bytecode::rd(code, at + 8) } else { bytecode::op_length(opc) / 4 };
            for k in 0..n {
                let v = bytecode::rd(code, at + 4 * k);
                if (26..=33).contains(&opc) && k == 4 {
                    // Only the low byte (effectorID) is written; the rest is stale buffer data.
                    l += &format!(" ??????{:02x}", v & 0xff);
                } else if opc == op::FUNCTION && k == 1 {
                    l += &format!(" F:{}", fun_name(v));
                } else if opc == op::FUNCTION
                    && k == 6
                    && matches!(Fun::from_u32(bytecode::rd(code, at + 4)), Some(Fun::SpawnNewEffect | Fun::CreateEffects))
                {
                    l += &format!(" E:{}", lib.statics[v as usize].name);
                } else {
                    l += &format!(" {}", hex(v));
                }
            }
            at += 4 * n;
        }
        out.push(l);
    }
    let mut l = String::from("A");
    for v in &s.alternate_offsets {
        l += &format!(" {}", hex(*v));
    }
    out.push(l);
    out
}

fn check(dump_text: &str) {
    let Some(data) = common::data() else {
        eprintln!("skipped: set HWC_GAME_DIR or HWC_EXTRACTED_DIR");
        return;
    };
    let mut lib = Library::new();
    for line in dump_text.lines().filter(|l| l.starts_with("L ")) {
        lib.load_effect(&data, &format!("ETG\\{}", &line[2..])).expect("compile");
    }
    let lines: Vec<&str> = dump_text.lines().collect();
    let mut checked = 0;
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        if let Some(rest) = l.strip_prefix("S ") {
            let name = rest.split(' ').next().unwrap();
            let s = &lib.statics[lib.get(name).unwrap_or_else(|| panic!("{name} not registered"))];
            let mine = dump(&lib, s);
            if !s.is_loaded() {
                assert!(l.contains("npb=ffffffff"), "{name}: loaded only in the original");
                i += 1;
                continue;
            }
            for (k, m) in mine.iter().enumerate() {
                let want = lines[i + k].trim_end();
                let mt: Vec<&str> = m.trim_end().split(' ').collect();
                let wt: Vec<&str> = want.split(' ').collect();
                let same = mt.len() == wt.len()
                    && mt.iter().zip(&wt).all(|(a, b)| a == b || (a.starts_with("??????") && b.len() == 8 && a[6..] == b[6..]));
                assert!(same, "{name}: line {k} differs
 mine: {m}
 want: {want}");
            }
            checked += 1;
            i += mine.len();
        } else if let Some(rest) = l.strip_prefix("T ") {
            let (idx, name) = rest.split_once(' ').unwrap();
            assert_eq!(lib.textures[idx.parse::<usize>().unwrap()], name, "texture {idx}");
            i += 1;
        } else {
            i += 1;
        }
    }
    eprintln!("{checked} effect statics identical to the original compiler's output");
    assert!(checked > 0);
}

#[test]
fn compiler_matches_binary_small() {
    check(include_str!("data/etg_emu_small.txt"));
}

#[test]
fn compiler_matches_binary_batch() {
    check(include_str!("data/etg_emu_batch.txt"));
}
