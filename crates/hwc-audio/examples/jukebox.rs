//! Jukebox: list and play the game's music, sound events and speech.
//!
//! Data comes from `HWC_EXTRACTED_DIR` (extracted archive) and `HWC_GAME_DIR` (install, needed
//! for music and speech), or `--extracted <dir>` / `--game <dir>`.
//!
//! ```text
//! cargo run -p hwc-audio --example jukebox -- list
//! cargo run -p hwc-audio --example jukebox -- music 3            # play through the sound card
//! cargo run -p hwc-audio --example jukebox -- speech beast 26
//! cargo run -p hwc-audio --example jukebox -- event 0x1002 --wav boom.wav --secs 4
//! cargo run -p hwc-audio --example jukebox                        # interactive
//! ```
//!
//! Interactive commands: `m <track>`, `stop`, `x <track>` (crossfade), `e <event> [distance]
//! [pan-angle]`, `p <bank> <patch>`, `s <event> [variable] [variation]`, `race beast|somtaaw`,
//! `vol <sfx> <speech> <music>`, `list`, `q`.

use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use hwc_audio::mixer::Mixer;
use hwc_audio::offline::{self, levels};
use hwc_audio::output::{AudioOutput, OutputOptions};
use hwc_audio::system::{event, BankKind, Emitter, ShipSounds, SoundSystem};
use hwc_audio::tables::Listener;
use hwc_audio::{DataSource, Race, SoundData};

struct Args {
    extracted: Option<PathBuf>,
    game: Option<PathBuf>,
    wav: Option<PathBuf>,
    secs: f64,
    rest: Vec<String>,
}

fn parse_args() -> Args {
    let mut a = Args {
        extracted: std::env::var_os("HWC_EXTRACTED_DIR").map(PathBuf::from),
        game: std::env::var_os("HWC_GAME_DIR").map(PathBuf::from),
        wav: None,
        secs: 0.0,
        rest: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--extracted" => a.extracted = it.next().map(PathBuf::from),
            "--game" => a.game = it.next().map(PathBuf::from),
            "--wav" => a.wav = it.next().map(PathBuf::from),
            "--secs" => a.secs = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
            _ => a.rest.push(arg),
        }
    }
    a
}

fn num(s: Option<&String>) -> Option<i64> {
    let s = s?;
    match s.strip_prefix("0x") {
        Some(h) => i64::from_str_radix(h, 16).ok(),
        None => s.parse().ok(),
    }
}

fn bank_kind(s: &str) -> Option<BankKind> {
    match s {
        "guns" | "gun" => Some(BankKind::Guns),
        "ships" | "ship" => Some(BankKind::Ships),
        "special" | "specialeffects" | "fx" => Some(BankKind::Special),
        "ui" => Some(BankKind::Ui),
        _ => None,
    }
}

fn list(d: &SoundData) {
    println!("Effect banks:");
    for b in [&d.guns, &d.ships, &d.special, &d.ui] {
        let secs: f64 = b.patches.iter().map(|p| p.duration_secs()).sum();
        let looping = b.patches.iter().filter(|p| p.flags & 1 != 0).count();
        println!("  {:<15} {:>3} patches ({looping} looping), {secs:.0} s", b.name, b.patches.len());
    }
    println!("Event tables (objects x events):");
    for (name, l) in [
        ("ui     0x01xx", &d.ui_events),
        ("gun    0x02xx", &d.gun_events),
        ("shipc  0x04xx", &d.ship_cmn_events),
        ("ship   0x08xx", &d.ship_events),
        ("exp    0x10xx", &d.spec_exp_events),
        ("hit    0x40xx", &d.spec_hit_events),
    ] {
        let used = (0..usize::from(l.num_objects))
            .flat_map(|o| (0..usize::from(l.num_events)).map(move |e| (o, e)))
            .filter(|&(o, e)| l.variations(o, e).iter().any(|&p| p >= 0))
            .count();
        println!("  {name}: {} x {} ({used} with sound)", l.num_objects, l.num_events);
    }
    match &d.music {
        Some(m) => {
            println!("Music ({} tracks):", m.tracks.len());
            for (i, t) in m.tracks.iter().enumerate() {
                println!(
                    "  {i:>2}: {:>6.1} s {} vol {:>3} {}ch",
                    t.duration_secs(),
                    if t.looping { "loop" } else { "    " },
                    t.volume,
                    t.channels
                );
            }
        }
        None => println!("Music: not available (needs the install directory)"),
    }
    for race in [Race::Somtaaw, Race::Beast] {
        match d.speech(race) {
            Some(s) => {
                let playable = (0..usize::from(s.sentences.num_events))
                    .filter(|&e| s.sentences.sentence(0, e).is_some_and(|x| x.num_variations > 0))
                    .count();
                let subs = s.fragments.values().filter(|f| f.subtitle.is_some()).count();
                println!(
                    "Speech {race:?}: {} sentence events ({playable} with variations), {} fragments, {subs} subtitled",
                    s.sentences.num_events,
                    s.fragments.len()
                );
            }
            None => println!("Speech {race:?}: not available"),
        }
    }
}

/// Run one command. Returns false to quit.
fn run(sys: &mut SoundSystem, words: &[String]) -> bool {
    let Some(cmd) = words.first().map(String::as_str) else { return true };
    let mut ship = ShipSounds::default();
    match cmd {
        "q" | "quit" | "exit" => return false,
        "list" | "l" => list(sys.data()),
        "m" | "music" => {
            let t = num(words.get(1)).unwrap_or(i64::from(event::MUSIC_RANDOM)) as i32;
            println!("music {t}: {}", sys.play_music(t));
        }
        "x" | "crossfade" => {
            let t = num(words.get(1)).unwrap_or(0) as i32;
            let secs = words.get(2).and_then(|s| s.parse().ok()).unwrap_or(2.0);
            println!("crossfade to {t}: {}", sys.crossfade_music(t, secs));
        }
        "stop" => {
            sys.stop_music(-1, 1.0);
            sys.stop_all(0.5, false);
            for i in 0..3 {
                sys.stop_speech(i, 0.5);
            }
        }
        "e" | "event" => {
            let ev = num(words.get(1)).unwrap_or(0x108) as u32;
            let dist: f32 = words.get(2).and_then(|s| s.parse().ok()).unwrap_or(1000.0);
            let angle: f32 = words.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
            sys.listener = Listener::default();
            let a = angle.to_radians();
            // The listener looks along +x with +z up, so the game's "right" is -y.
            let pos = [dist * a.cos(), -dist * a.sin(), 0.0];
            let e = Emitter { position: pos, radius: 10.0, ship_class: 3, ship_type: 1, gun_sound_type: 1, ..Default::default() };
            let emitter = (ev & event::MASK != event::UI).then_some(&e);
            let h = sys.play_event(emitter, ev, &mut ship);
            println!("event {ev:#06x} at {dist} units, {angle} deg: handle {h}");
        }
        "p" | "patch" => {
            let Some(b) = words.get(1).and_then(|s| bank_kind(s)) else {
                println!("usage: p guns|ships|special|ui <patch>");
                return true;
            };
            let n = num(words.get(2)).unwrap_or(0) as i32;
            println!("patch {n}: handle {}", sys.play_patch(b, n, None, 0, 100, 255, true));
        }
        "s" | "speech" => {
            let (ev, var, variation) = (num(words.get(1)).unwrap_or(0), num(words.get(2)).unwrap_or(-1), num(words.get(3)).unwrap_or(-1));
            match sys.play_speech(ev as u32, var as i32, variation as i32, 0) {
                Some(s) => println!(
                    "speech {ev}: row {} ({} fragment(s), ~{} ms){}",
                    s.phrase_row,
                    s.fragments,
                    s.duration_ms,
                    if s.subtitles.is_empty() { String::new() } else { format!(": \"{}\"", s.subtitles.join(" ")) }
                ),
                None => println!("speech {ev}: nothing to say"),
            }
        }
        "race" => {
            let r = if words.get(1).is_some_and(|w| w.starts_with('b')) { Race::Beast } else { Race::Somtaaw };
            sys.set_race(r);
            println!("race {r:?}");
        }
        "vol" => {
            let f = |i: usize| words.get(i).and_then(|s| s.parse::<f32>().ok()).unwrap_or(1.0);
            sys.set_sfx_volume(f(1));
            sys.set_speech_volume(f(2));
            sys.set_music_volume(f(3));
        }
        _ => println!("unknown command {cmd:?}"),
    }
    true
}

fn main() {
    let args = parse_args();
    let src = match DataSource::new(args.extracted.clone(), args.game.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("{e}\nset HWC_EXTRACTED_DIR / HWC_GAME_DIR or pass --extracted / --game");
            std::process::exit(1);
        }
    };
    let data = match SoundData::load(&src) {
        Ok(d) => Arc::new(d),
        Err(e) => {
            eprintln!("loading sound data: {e}");
            std::process::exit(1);
        }
    };
    let mixer = Arc::new(Mutex::new(Mixer::default()));
    let mut sys = SoundSystem::new(data.clone(), mixer.clone());

    if args.rest.first().map(String::as_str) == Some("list") {
        list(&data);
        return;
    }

    // Offline: render the command to a WAV file and print its levels.
    if let Some(path) = &args.wav {
        let secs = if args.secs > 0.0 { args.secs } else { 10.0 };
        if !args.rest.is_empty() {
            // Apply the command at t = 0 through the same path as interactive use.
            run(&mut sys, &args.rest);
        }
        let r = offline::render(&mut sys, &[], secs, 3);
        let (l, rr) = (levels(&r.left), levels(&r.right));
        println!(
            "{:.1} s: rms L {:.0} ({:.1} dBFS) R {:.0}, peak {:.0}",
            secs,
            l.rms,
            offline::dbfs(l.rms),
            rr.rms,
            l.peak.max(rr.peak)
        );
        std::fs::write(path, r.to_wav()).expect("writing wav");
        println!("wrote {}", path.display());
        return;
    }

    let out = match AudioOutput::start(mixer, OutputOptions::default()) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("{e}; use --wav <file> to render offline instead");
            std::process::exit(1);
        }
    };
    println!("output: {} at {} Hz, {} channels (mixer runs at 22050 Hz)", out.device_name, out.device_rate, out.device_channels);

    if !args.rest.is_empty() {
        run(&mut sys, &args.rest);
        let secs = if args.secs > 0.0 { args.secs } else { 15.0 };
        let steps = (secs * 30.0) as usize;
        for _ in 0..steps {
            sys.update(true);
            std::thread::sleep(Duration::from_millis(33));
        }
        return;
    }

    println!("commands: m <track> | x <track> [secs] | stop | e <event> [dist] [angle] | p <bank> <n> | s <event> [var] [variation] | race b|s | vol s sp m | list | q");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    loop {
        match rx.recv_timeout(Duration::from_millis(33)) {
            Ok(line) => {
                let words: Vec<String> = line.split_whitespace().map(str::to_owned).collect();
                if !run(&mut sys, &words) {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        sys.update(true);
    }
}
