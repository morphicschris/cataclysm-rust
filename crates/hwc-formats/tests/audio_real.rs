//! Tests against the real sound data. `HWC_EXTRACTED_DIR` must point at the extracted archive
//! (containing `soundfx\`), `HWC_GAME_DIR` at the install holding the `.wxd`/`.vce` files. Each test
//! skips cleanly when its variable is unset. Set `HWC_AUDIO_DUMP_DIR` to also write sample WAVs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use hwc_formats::audio::{
    music_tracks, Bank, DecodedAudio, Encoding, FqDecoder, Lut, MusicIndex, OutputRate, Patch, SpeechBank,
    StreamRecord, SynthesisWindow, BLOCK_SECONDS,
};

fn soundfx_dir() -> Option<PathBuf> {
    match std::env::var_os("HWC_EXTRACTED_DIR") {
        Some(d) => Some(PathBuf::from(d).join("soundfx")),
        None => {
            eprintln!("HWC_EXTRACTED_DIR not set; skipping");
            None
        }
    }
}

fn game_dir() -> Option<PathBuf> {
    let d = hwc_formats::game_dir();
    if d.is_none() {
        eprintln!("HWC_GAME_DIR not set; skipping");
    }
    d
}

fn files_with_ext(dir: &Path, ext: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case(ext)))
        .collect();
    v.sort();
    v
}

fn read(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

fn stem(path: &Path) -> String {
    path.file_stem().unwrap().to_string_lossy().to_ascii_lowercase()
}

/// Sanity checks shared by every decode: finite samples, length matching the block count.
fn check_decoded(what: &str, audio: &DecodedAudio, blocks: usize, rate: OutputRate) -> f64 {
    assert_eq!(audio.frames(), blocks * rate.samples_per_block(), "{what}: length");
    assert!(audio.samples.iter().all(|s| s.is_finite()), "{what}: non-finite sample");
    let expected = blocks as f64 * BLOCK_SECONDS;
    assert!((audio.duration_secs() - expected).abs() < 1e-6, "{what}: duration");
    let energy: f64 = audio.samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (energy / audio.samples.len().max(1) as f64).sqrt();
    let peak = audio.samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak < 200_000.0, "{what}: implausible peak {peak}");
    rms
}

fn dump(name: &str, audio: &DecodedAudio) {
    if let Some(dir) = std::env::var_os("HWC_AUDIO_DUMP_DIR") {
        let path = PathBuf::from(dir).join(format!("{name}.wav"));
        std::fs::write(&path, audio.to_wav()).unwrap();
        eprintln!("wrote {}", path.display());
    }
}

#[test]
fn banks_parse_and_decode() {
    let Some(dir) = soundfx_dir() else { return };
    let banks = files_with_ext(&dir, "bnk");
    assert_eq!(banks.len(), 4, "expected guns/ships/specialeffects/ui");
    let expected: HashMap<&str, usize> = [("guns", 37), ("ships", 216), ("specialeffects", 77), ("ui", 61)].into();
    let t = Instant::now();
    let mut total_patches = 0;
    let mut total_secs = 0.0;
    let mut bins_by_rate = HashMap::new();
    for path in &banks {
        let bytes = read(path);
        let bank = Bank::parse(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let name = stem(path);
        assert_eq!(Some(&bank.patches.len()), expected.get(name.as_str()), "{name} patch count");
        let mut silent = 0;
        let mut end = 0;
        for p in &bank.patches {
            let what = format!("{name}#{}", p.index);
            assert_eq!(&p.tag, b"PCH0", "{what}");
            assert_eq!(p.format.format_tag, 1, "{what}");
            assert_eq!(p.format.channels, 1, "{what}");
            assert_eq!(p.format.bits_per_sample, 16, "{what}");
            assert!(matches!(p.format.sample_rate, 22_050 | 44_100), "{what}: rate {}", p.format.sample_rate);
            let Encoding::Fquant { bitrate } = p.encoding() else { panic!("{what}: PCM patch") };
            let block = u32::from(bitrate / 8);
            assert_eq!(p.data_size % block, 0, "{what}: partial block");
            assert_eq!(p.loop_start % block, 0, "{what}: loop start alignment");
            assert_eq!(p.loop_end % block, 0, "{what}: loop end alignment");
            assert!(p.data_offset as usize >= end, "{what}: payloads overlap or out of order");
            end = (p.data_offset + p.data_size) as usize;

            let audio = p.decode(OutputRate::Hz22050).unwrap();
            let rms = check_decoded(&what, &audio, p.num_blocks(), OutputRate::Hz22050);
            if rms < 1.0 {
                silent += 1;
            }
            total_secs += p.duration_secs();
            let top = highest_bin(p);
            let r = p.format.sample_rate;
            bins_by_rate.entry(r).and_modify(|m: &mut (usize, usize)| *m = (m.0.min(top), m.1.max(top))).or_insert((top, top));
            if r == 44_100 && rms > 100.0 {
                // No patch codes bins above 127, so the 44.1 kHz rendering is the same signal at
                // twice the sample rate and must carry the same energy.
                let wide = p.decode(OutputRate::Hz44100).unwrap();
                let wide_rms = check_decoded(&what, &wide, p.num_blocks(), OutputRate::Hz44100);
                let ratio = wide_rms / rms;
                assert!((0.98..1.02).contains(&ratio), "{what}: 44.1k/22.05k rms ratio {ratio}");
            }
            if p.index < 3 {
                dump(&format!("bank_{name}_{:03}", p.index), &audio);
            }
        }
        assert_eq!(end, bytes.len(), "{name}: last payload should end the file");
        assert!(silent * 10 < bank.patches.len(), "{name}: {silent} near-silent patches");
        total_patches += bank.patches.len();
    }
    eprintln!("{total_patches} patches, {total_secs:.1}s of audio decoded in {:?}", t.elapsed());
    eprintln!("highest coded bin by source rate (min, max over patches): {bins_by_rate:?}");
}

fn highest_bin(p: &Patch) -> usize {
    let Encoding::Fquant { bitrate } = p.encoding() else { return 0 };
    let mut dec = FqDecoder::new(OutputRate::Hz44100, 1, SynthesisWindow::default());
    let mut out = vec![0.0; OutputRate::Hz44100.samples_per_block()];
    for blk in p.data.chunks_exact(usize::from(bitrate / 8)) {
        dec.decode_block(0, blk, u32::from(bitrate), &mut out);
    }
    dec.highest_bin()
}

#[test]
fn luts_parse_and_cross_check() {
    let Some(dir) = soundfx_dir() else { return };
    let mut luts = HashMap::new();
    for path in files_with_ext(&dir, "lut") {
        let lut = Lut::parse(&read(&path)).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        luts.insert(stem(&path), lut);
    }
    assert_eq!(luts.len(), 15);

    // Each event table matches its bank by checksum, and only names patches that exist.
    for (lut_name, bank_name) in [
        ("gunevents", "guns"),
        ("gunspecevents", "guns"),
        ("shipcmnevents", "ships"),
        ("shipevents", "ships"),
        ("derelictevents", "ships"),
        ("specexpevents", "specialeffects"),
        ("spechitevents", "specialeffects"),
        ("uievents", "ui"),
    ] {
        let Lut::Sfx(lut) = &luts[lut_name] else { panic!("{lut_name} is not PKG0") };
        let bytes = read(&dir.join(format!("{bank_name}.bnk")));
        let bank = Bank::parse(&bytes).unwrap();
        assert_eq!(lut.checksum, bank.checksum, "{lut_name} vs {bank_name}");
        let mut used = 0;
        for o in 0..usize::from(lut.num_objects) {
            for e in 0..usize::from(lut.num_events) {
                let row = lut.row(o, e).unwrap();
                assert!(row[0] >= 0 && row[0] <= lut.num_variations as i16, "{lut_name}[{o},{e}] count");
                for &p in lut.variations(o, e) {
                    assert!(p >= 0 && (p as usize) < bank.patches.len(), "{lut_name}[{o},{e}] -> {p}");
                    used += 1;
                }
            }
        }
        assert!(used > 0, "{lut_name} references nothing");
    }

    for (sentence, phrase) in [("beastsentence", "beastphrase"), ("sectsentence", "sectphrase")] {
        let Lut::Sentence(s) = &luts[sentence] else { panic!("{sentence} is not LUT1") };
        let Lut::Phrase(p) = &luts[phrase] else { panic!("{phrase} is not LUT2") };
        assert_eq!(s.checksum, p.checksum, "{sentence} vs {phrase}");
        assert_eq!(s.comp_bitrates.len(), usize::from(s.num_actors));
        assert!(s.comp_bitrates.iter().all(|&b| b == 512), "{sentence}: {:?}", s.comp_bitrates);
        for a in 0..usize::from(s.num_actors) {
            for e in 0..usize::from(s.num_events) {
                let st = s.sentence(a, e).unwrap();
                let rows = i32::from(st.num_variations) * i32::from(st.max_variable);
                if rows > 0 {
                    assert!(st.phrase_index >= 0, "{sentence}[{e}]");
                    assert!((i32::from(st.phrase_index) + rows) as usize <= p.phrases.len(), "{sentence}[{e}] {st:?}");
                }
            }
        }
        assert!(p.phrases.iter().all(|ph| ph.probability <= 100));
    }

    let (Lut::Volume(v), Lut::Range(r), Lut::Frequency(f)) = (&luts["volume"], &luts["range"], &luts["frequency"])
    else {
        panic!("volume/range/frequency kinds")
    };
    assert_eq!((v.columns, v.rows), (r.columns, r.rows));
    for row in 0..usize::from(r.rows) {
        let ranges: Vec<i32> = (0..usize::from(r.columns)).map(|c| r.get(row, c).unwrap()).collect();
        assert!(ranges.windows(2).all(|w| w[0] <= w[1]), "range row {row} not ascending: {ranges:?}");
    }
    assert!(f.values.iter().all(|x| (0.0..=1.0).contains(x)));
}

#[test]
fn music_index_and_data() {
    let (Some(fx), Some(game)) = (soundfx_dir(), game_dir()) else { return };
    let index = MusicIndex::parse(&read(&fx.join("music4cat.wxh"))).unwrap();
    assert_eq!(index.streams.len(), 62);
    let t = Instant::now();
    let wxd = read(&game.join("Music4Cat.wxd"));
    let tracks = music_tracks(&index, &wxd).unwrap();
    eprintln!("read + indexed {} MB in {:?}", wxd.len() >> 20, t.elapsed());
    assert_eq!(tracks.last().unwrap().record.end(), wxd.len());
    let mut total = 0.0;
    for w in tracks.windows(2) {
        assert_eq!(w[0].record.end(), w[1].info.offset as usize, "tracks are contiguous");
    }
    for tr in &tracks {
        assert_eq!((tr.info.bitrate, tr.info.channels), (512, 2), "track {}", tr.index);
        assert!(tr.record.subtitle.is_none());
        assert_eq!(tr.record.data.len() % 128, 0);
        total += tr.duration_secs();
    }
    eprintln!("{} tracks, {:.1} min total", tracks.len(), total / 60.0);

    // Decode the opening of a few tracks (and all of the short stingers) and check them.
    let t = Instant::now();
    for &i in &[0usize, 20, 37, 40, 61] {
        let tr = &tracks[i];
        let blocks = tr.num_blocks().min(1300); // ~15 s
        let head = &tr.record.data[..blocks * 128];
        let audio = hwc_formats::audio::decode_fquant(head, 512, 2, OutputRate::Hz22050).unwrap();
        let rms = check_decoded(&format!("music {i}"), &audio, blocks, OutputRate::Hz22050);
        if (37..=39).contains(&i) {
            // Three 0.7 s tracks are deliberate silence: every block is an empty reset block.
            assert_eq!(rms, 0.0, "music {i} should be silent");
        } else {
            assert!(rms > 10.0, "music {i} is silent (rms {rms})");
        }
        dump(&format!("music_{i:02}"), &audio);
    }
    eprintln!("decoded music samples in {:?}", t.elapsed());
}

fn check_speech(file: &str, phrase_lut: &str, expected: usize) {
    let (Some(fx), Some(game)) = (soundfx_dir(), game_dir()) else { return };
    let t = Instant::now();
    let bytes = read(&game.join(file));
    let bank = SpeechBank::parse(&bytes).unwrap();
    eprintln!("{file}: {} phrases indexed in {:?}", bank.phrases.len(), t.elapsed());
    assert_eq!(bank.phrases.len(), expected);
    assert_eq!(bank.phrases.last().unwrap().end(), bytes.len());

    let Lut::Phrase(lut) = Lut::parse(&read(&fx.join(phrase_lut))).unwrap() else { panic!() };
    assert_eq!(lut.checksum, bank.checksum);
    let mut referenced = 0;
    for ph in lut.phrases.iter().filter(|p| !p.placeholder) {
        for &o in &ph.offsets {
            assert!(bank.find(o as usize).is_some(), "{phrase_lut}: offset {o} is not a phrase start");
            referenced += 1;
        }
    }
    assert!(referenced > 0);

    let with_text = bank.phrases.iter().filter(|r| r.subtitle.is_some_and(|s| !s.is_empty())).count();
    eprintln!("{file}: {with_text} phrases carry subtitles");

    let t = Instant::now();
    let step = (bank.phrases.len() / 40).max(1);
    let mut voiced = 0;
    let mut checked = 0;
    for (i, r) in bank.phrases.iter().enumerate().step_by(step) {
        let rms = decode_phrase(&format!("{file}#{i}"), r);
        if rms > 10.0 {
            voiced += 1;
        }
        checked += 1;
        if i < 3 * step {
            let audio = hwc_formats::audio::decode_fquant(r.data, 512, 1, OutputRate::Hz22050).unwrap();
            dump(&format!("{}_{i:04}", stem(Path::new(file))), &audio);
        }
    }
    assert!(voiced * 10 >= checked * 9, "{file}: only {voiced}/{checked} sampled phrases have signal");
    eprintln!("{file}: decoded {checked} sample phrases in {:?}", t.elapsed());
}

fn decode_phrase(what: &str, r: &StreamRecord) -> f64 {
    assert_eq!(r.data.len() % 64, 0, "{what}");
    let audio = hwc_formats::audio::decode_fquant(r.data, 512, 1, OutputRate::Hz22050).unwrap();
    check_decoded(what, &audio, r.data.len() / 64, OutputRate::Hz22050)
}

#[test]
fn beast_speech() {
    check_speech("BeastSpeech.vce", "beastphrase.lut", 1438);
}

#[test]
fn sect_speech() {
    check_speech("SectSpeech.vce", "sectphrase.lut", 2096);
}
