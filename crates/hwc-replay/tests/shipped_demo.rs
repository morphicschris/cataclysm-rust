//! The demo shipped in `cataclysm.big` (`demos\abc640x480.dem`). Needs HWC_DATA_DIR /
//! HWC_GAME_DIR (read through `GameData`) or HWC_EXTRACTED_DIR; skips otherwise.

use std::path::PathBuf;

use hwc_replay::demo::{Demo, DemoEvent, Flavour, OptionValue};

const NAME: &str = "demos\\abc640x480.dem";

fn load() -> Option<Vec<u8>> {
    if let Some(Ok(gd)) = hwc_formats::gamedata::GameData::from_env() {
        if let Ok(Some(b)) = gd.read(NAME) {
            return Some(b);
        }
    }
    let dir = PathBuf::from(std::env::var_os("HWC_EXTRACTED_DIR")?);
    std::fs::read(dir.join("demos").join("abc640x480.dem")).ok()
}

#[test]
fn shipped_demo() {
    let Some(bytes) = load() else {
        eprintln!("skipping: no game data (set HWC_GAME_DIR, HWC_DATA_DIR or HWC_EXTRACTED_DIR)");
        return;
    };
    let demo = Demo::parse(&bytes).expect("parse");
    // A Homeworld recording: 25 random streams, Homeworld's five-entry option table.
    assert_eq!(demo.flavour(), Flavour::Homeworld);
    assert_eq!(demo.version, 7);
    assert_eq!(demo.start_time, 0.0);
    assert_eq!(
        demo.options().unwrap(),
        vec![
            OptionValue::Bool(1),
            OptionValue::Real32(0.0),
            OptionValue::Bool(1),
            OptionValue::Real32(1.0),
            OptionValue::Dword(0x80),
        ]
    );
    assert_eq!(demo.inputs().count(), 2233);
    assert_eq!(demo.ticks().count(), 2232);
    assert_eq!(demo.total_ticks(), 44957);
    assert!(matches!(demo.events.last(), Some(DemoEvent::Input(_))));
    // Snapshot times follow the tick counts (single-precision scheduler clock).
    assert!(demo.timing_error() < 1e-3, "timing error {}", demo.timing_error());
    // Byte-exact round trip.
    assert_eq!(demo.to_bytes(), bytes);
    // Read as a Cataclysm file it does not make sense.
    let as_cat = Demo::parse_with_streams(&bytes, 27).map(|d| d.options().is_ok());
    assert!(!matches!(as_cat, Ok(true)));
}
