//! Tests against the user's real `cataclysm.big`. Skipped when `HWC_GAME_DIR` is unset.

use std::time::Instant;

use hwc_formats::big::{name_crcs, normalize_name, BigArchive};

fn open_archive() -> Option<BigArchive> {
    let Some(dir) = hwc_formats::game_dir() else {
        eprintln!("HWC_GAME_DIR not set; skipping real-archive test");
        return None;
    };
    let path = dir.join("cataclysm.big");
    Some(BigArchive::open(&path).unwrap_or_else(|e| panic!("opening {}: {e}", path.display())))
}

#[test]
fn toc_names_and_crcs() {
    let Some(big) = open_archive() else { return };
    let entries = big.entries();
    assert_eq!(entries.len(), 16_528);
    assert!(big.sorted_by_crc());

    for e in entries {
        assert!(!e.name.is_empty() && e.name.is_ascii(), "odd name {:?}", e.name);
        let key = normalize_name(&e.name);
        assert_eq!(name_crcs(key.as_bytes()), (e.name_crc1, e.name_crc2), "CRC mismatch for {}", e.name);
        let found = big.find(&e.name).expect("find by stored name");
        assert_eq!(found.data_offset, e.data_offset);
        let alt = e.name.replace('\\', "/").to_ascii_uppercase();
        assert_eq!(big.find(&alt).map(|f| f.data_offset), Some(e.data_offset));
    }

    // Header flag 1 promises ascending (crc1, crc2) order.
    assert!(entries.windows(2).all(|w| (w[0].name_crc1, w[0].name_crc2) < (w[1].name_crc1, w[1].name_crc2)));
    assert!(big.find("no\\such\\file.txt").is_none());
}

#[test]
fn decompress_every_entry() {
    let Some(big) = open_archive() else { return };
    let start = Instant::now();
    let (mut compressed, mut bytes) = (0usize, 0u64);
    for e in big.entries() {
        let data = big.read(e).unwrap_or_else(|err| panic!("{}: {err}", e.name));
        assert_eq!(data.len(), e.real_len as usize, "{}", e.name);
        compressed += e.compressed as usize;
        bytes += data.len() as u64;
    }
    eprintln!(
        "decoded {} entries ({compressed} compressed), {bytes} bytes in {:?}",
        big.entries().len(),
        start.elapsed()
    );

    let script = big.read_file("aiplayer.script").unwrap().expect("aiplayer.script present");
    let head = &script[..script.len().min(256)];
    assert!(
        head.iter().all(|&b| b.is_ascii_graphic() || b.is_ascii_whitespace()),
        "aiplayer.script does not start with text: {:?}",
        String::from_utf8_lossy(head)
    );
}
