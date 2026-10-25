//! Headless campaign: missions played one after another with the fleet carried over.
//!
//! ```text
//! cargo run --release -p hwc-sim --example campaign -- <install> [--from N] [--to M]
//!     [--budget SECONDS] [--limit SECONDS] [--speech S] [--carry-dir DIR] [--log] [--effects]
//! ```
//!
//! `--from N` (N > 1) starts from the `warp.script` fleet of mission N, or from
//! `<carry-dir>/carry_m{N-1}.txt` when that exists. After each mission the carry-over is written
//! to `<carry-dir>/carry_m{N}.txt` (for `hwc-viewer --mission N+1 --carry FILE`).

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_sim::autoplay::Autoplay;
use hwc_sim::campaign::Carry;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, LogKind, MissionConfig, MissionRun};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = args.first().expect("usage: campaign <install> [--from N] [--to M] ...");
    let mut from = 1u32;
    let mut to = 6u32;
    let mut budget = 1200.0f32;
    let mut limit = 3600.0f32;
    let mut speech = Some(3.0f32);
    let mut carry_dir: Option<String> = None;
    let mut log = false;
    let mut effects = false;
    let mut i = 1;
    while i < args.len() {
        let v = || args.get(i + 1).cloned().unwrap_or_default();
        match args[i].as_str() {
            "--from" => from = v().parse().unwrap(),
            "--to" => to = v().parse().unwrap(),
            "--budget" => budget = v().parse().unwrap(),
            "--limit" => limit = v().parse().unwrap(),
            "--speech" => speech = v().parse().ok(),
            "--carry-dir" => carry_dir = Some(v()),
            "--log" => {
                log = true;
                i += 1;
                continue;
            }
            "--effects" => {
                effects = true;
                i += 1;
                continue;
            }
            a => panic!("unknown argument {a}"),
        }
        i += 2;
    }
    let gd = GameData::open(data).expect("game data");
    let src = GameDataSource(&gd);
    let statics = Arc::new(StaticWorld::load_from(&src, LoadOptions { single_player: true, ..LoadOptions::default() }));
    let tweaks = SimTweaks::load(&src);
    let mut carry: Option<Carry> = None;
    if from > 1 {
        let file = carry_dir.as_ref().map(|d| format!("{d}/carry_m{:02}.txt", from - 1));
        carry = match file.as_ref().and_then(|f| std::fs::read_to_string(f).ok()) {
            Some(t) => Some(Carry::from_text(&t, statics.clone(), tweaks.clone()).expect("carry file")),
            None => Carry::warp(statics.clone(), tweaks.clone(), &src, from),
        };
    }
    for m in from..=to {
        let script = load_kas_mission(&gd, &mission_script_name(m)).expect("mission script");
        let mut config = MissionConfig::new(m);
        config.log_calls = std::env::var_os("HWC_LOG_CALLS").is_some();
        config.speech_seconds = speech;
        let mut run = match &carry {
            None => MissionRun::start(statics.clone(), tweaks.clone(), &src, script, config),
            Some(c) => MissionRun::start_campaign(statics.clone(), tweaks.clone(), &src, script, config, c),
        }
        .unwrap_or_else(|e| panic!("mission {m}: {e}"));
        if effects {
            // As the viewer and the game run it: with the effects attached.
            hwc_sim::effects::attach(&mut run.universe, &hwc_sim::effects::EffectData::load_from(&src));
        }
        let mut ap = Autoplay::new((budget * 16.0) as u32);
        let ending = ap.play(&mut run, (limit * 16.0) as u32);
        let config_calls = std::env::var_os("HWC_LOG_CALLS").is_some();
        let mut unimpl = std::collections::BTreeMap::new();
        for e in &run.game.log {
            match e.kind {
                LogKind::Unimplemented => *unimpl.entry(e.text.split(" (").next().unwrap_or("").to_string()).or_insert(0) += 1,
                LogKind::Flow | LogKind::Objective | LogKind::Warning if log => println!("  {e}"),
                LogKind::Call if log && config_calls => println!("  {e}"),
                _ => {}
            }
        }
        if log {
            for (t, n) in &ap.notes {
                println!("  [{t:>6}] autoplay: {n}");
            }
        }
        let objectives: Vec<&str> = run.game.objectives.iter().map(|o| o.label.as_str()).collect();
        println!(
            "mission {m}: {ending:?} at update {} ({:.0} s), objectives {objectives:?}, RUs {}",
            run.universe.update_counter,
            run.universe.total_time_elapsed,
            run.universe.base.players[0].resource_units
        );
        for (k, n) in &unimpl {
            println!("  unimplemented: {k} x{n}");
        }
        let Some(c) = run.carry() else {
            println!("  no carry-over; stopping");
            break;
        };
        println!("  carried: {}", c.fleet.iter().map(|f| format!("{}{}", f.ship_type, if f.docked.is_empty() { String::new() } else { format!("(+{})", f.docked.len()) })).collect::<Vec<_>>().join(", "));
        if let Some(d) = &carry_dir {
            let _ = std::fs::create_dir_all(d);
            std::fs::write(format!("{d}/carry_m{m:02}.txt"), c.to_text()).expect("write carry");
        }
        carry = Some(c);
    }
}
