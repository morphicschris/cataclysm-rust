//! Headless single-player mission run.
//!
//! ```text
//! cargo run -p hwc-sim --example mission -- <data> <mission> [--ticks N] [--skill N]
//!     [--nislet SECONDS] [--speech SECONDS] [--no-calls] [--every N]
//! ```
//!
//! `<data>` is an imported data directory, a game install or `cataclysm.big` (with the
//! mission script then taken from `HWC_DATA_DIR`/`HWC_GAME_DIR`). Loads the mission level,
//! runs `N` universe updates (16 per second) with the KAS mission and prints the log: level
//! loading, single-player state changes, KAS state changes, objectives and KAS calls.

use std::sync::Arc;

use hwc_formats::gamedata::GameData;
use hwc_sim::mission::{load_kas_mission, mission_script_name, GameDataSource, LogKind, MissionConfig, MissionRun};
use hwc_sim::statics::{LoadOptions, StaticWorld};
use hwc_sim::SimTweaks;

fn usage() -> ! {
    eprintln!("usage: mission <data> <mission> [--ticks N] [--skill N] [--nislet S] [--speech S] [--no-calls] [--every N]");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        usage();
    }
    let data = &args[0];
    let mission: u32 = args[1].parse().unwrap_or_else(|_| usage());
    let mut ticks = 16 * 120;
    let mut config = MissionConfig::new(mission);
    let mut every = 0u32;
    let mut i = 2;
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).cloned().unwrap_or_else(|| usage());
        match args[i].as_str() {
            "--ticks" => ticks = val(i).parse().unwrap_or_else(|_| usage()),
            "--skill" => config.skill = val(i).parse().unwrap_or_else(|_| usage()),
            "--nislet" => config.nislet_seconds = val(i).parse().unwrap_or_else(|_| usage()),
            "--speech" => config.speech_seconds = Some(val(i).parse().unwrap_or_else(|_| usage())),
            "--every" => every = val(i).parse().unwrap_or_else(|_| usage()),
            "--no-calls" => {
                config.log_calls = false;
                i += 1;
                continue;
            }
            _ => usage(),
        }
        i += 2;
    }

    let gd = GameData::open(data).unwrap_or_else(|e| {
        eprintln!("{data}: {e}");
        std::process::exit(1);
    });
    let src = GameDataSource(&gd);
    let script_name = mission_script_name(mission);
    let script = load_kas_mission(&gd, &script_name)
        .or_else(|e| match GameData::from_env() {
            Some(Ok(other)) => load_kas_mission(&other, &script_name),
            _ => Err(e),
        })
        .unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(1);
        });
    let options = LoadOptions { single_player: true, ..LoadOptions::default() };
    let statics = Arc::new(StaticWorld::load_from(&src, options));
    let tweaks = SimTweaks::load(&src);
    let mut run = MissionRun::start(statics, tweaks, &src, script, config).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1);
    });

    let mut printed = 0;
    for t in 0..ticks {
        run.step();
        for e in &run.game.log[printed..] {
            println!("{e}");
        }
        printed = run.game.log.len();
        if every > 0 && (t + 1) % every == 0 {
            let u = &run.universe;
            println!(
                "-- frame {} t={:.2}s ships={} sp_state={} objectives={:?}",
                u.update_counter,
                u.total_time_elapsed,
                u.ship_list.len(),
                run.game.sp_state,
                run.game.objectives.iter().map(|o| o.label.as_str()).collect::<Vec<_>>()
            );
        }
    }
    let unimplemented: std::collections::BTreeMap<String, usize> =
        run.game.log.iter().filter(|e| e.kind == LogKind::Unimplemented).fold(Default::default(), |mut m, e| {
            *m.entry(e.text.split(' ').next().unwrap_or("").to_string()).or_default() += 1;
            m
        });
    println!("\n== after {ticks} updates ({:.1} s game time)", run.universe.total_time_elapsed);
    println!("ships: {}", run.universe.ship_list.len());
    for t in run.kas.teams() {
        let state = t.state.map(|s| run.game.name(s)).unwrap_or_else(|| "-".into());
        let n = run.game.level.team(t.label).map_or(0, |x| x.ships.len());
        println!("team {:32} ships {:3} state {}", run.game.name(t.label), n, state);
    }
    for o in &run.game.objectives {
        println!("objective {:?}: {}", o.label, o.brief);
    }
    if !unimplemented.is_empty() {
        println!("unimplemented runtime functions called: {unimplemented:?}");
    }
}
