//! Tests over the real game data. Needs `HWC_EXTRACTED_DIR` (the extracted archive) and/or
//! `HWC_GAME_DIR` (the install, for music and speech); skips cleanly when neither is set.
//! Set `HWC_AUDIO_DUMP_DIR` to also write the offline mixes as WAV files.

use std::sync::{Arc, Mutex, OnceLock};

use hwc_audio::codec::{BLOCK_FRAMES, BLOCK_SECONDS};
use hwc_audio::mixer::{Mixer, MixerOptions, PatchRef, NO_HANDLE};
use hwc_audio::offline::{self, levels, power_spectrum, Command, Cue};
use hwc_audio::system::{event, BankKind, Emitter, GameContext, ShipSounds, SoundSystem};
use hwc_audio::tables::Listener;
use hwc_audio::{DataSource, Race, SoundData};
use hwc_formats::audio::{decode_fquant_with, OutputRate, SynthesisWindow};

fn data() -> Option<Arc<SoundData>> {
    static DATA: OnceLock<Option<Arc<SoundData>>> = OnceLock::new();
    DATA.get_or_init(|| {
        let Some(src) = DataSource::from_env() else {
            eprintln!("HWC_EXTRACTED_DIR / HWC_GAME_DIR not set; skipping");
            return None;
        };
        Some(Arc::new(SoundData::load(&src).expect("loading sound data")))
    })
    .clone()
}

fn system(data: &Arc<SoundData>) -> SoundSystem {
    SoundSystem::new(data.clone(), Arc::new(Mutex::new(Mixer::default())))
}

fn dump(name: &str, r: &offline::Rendered) {
    if let Some(dir) = std::env::var_os("HWC_AUDIO_DUMP_DIR") {
        let path = std::path::PathBuf::from(dir).join(format!("{name}.wav"));
        std::fs::write(&path, r.to_wav()).unwrap();
        eprintln!("wrote {}", path.display());
    }
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// Mix one patch alone at full volume, centred, as a one-shot.
fn mix_alone(p: &PatchRef) -> Vec<f32> {
    let mut one_shot = p.clone();
    one_shot.flags = 0;
    let mut m = Mixer::new(MixerOptions::default());
    let h = m.play(&one_shot, None, -1.0, 0, 50, 255, false, false, false);
    assert!(h >= 0);
    let blocks = (p.end - p.start) / p.block_bytes();
    let mut l = Vec::with_capacity(blocks * BLOCK_FRAMES);
    for _ in 0..blocks {
        let mut f = [[0.0f32; BLOCK_FRAMES]; 2];
        m.process_block_f32(&mut f);
        assert_eq!(f[0], f[1]);
        l.extend_from_slice(&f[0]);
    }
    l
}

#[test]
fn every_patch_mixes_like_the_reference_decoder() {
    let Some(d) = data() else { return };
    let mut n = 0;
    let mut secs = 0.0;
    for bank in [&d.guns, &d.ships, &d.special, &d.ui] {
        for (i, p) in bank.patches.iter().enumerate() {
            let mixed = mix_alone(p);
            let r = decode_fquant_with(&p.data[p.start..p.end], p.bitrate, 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
            assert_eq!(mixed.len(), r.samples.len(), "{} {i}: length", bank.name);
            let pk = peak(&r.samples).max(1.0);
            let worst = mixed.iter().zip(&r.samples).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
            assert!(worst < 2e-3 * pk, "{} patch {i}: worst {worst}, peak {pk}", bank.name);
            n += 1;
            secs += p.duration_secs();
        }
    }
    assert_eq!(n, 391);
    eprintln!("{n} patches, {secs:.0} s of audio, identical to the reference decoder");
}

#[test]
fn event_tables_reference_valid_patches() {
    let Some(d) = data() else { return };
    let checks = [
        (&d.gun_events, &d.guns),
        (&d.gun_spec_events, &d.guns),
        (&d.ship_cmn_events, &d.ships),
        (&d.ship_events, &d.ships),
        (&d.derelict_events, &d.ships),
        (&d.spec_exp_events, &d.special),
        (&d.spec_hit_events, &d.special),
        (&d.ui_events, &d.ui),
    ];
    let mut refs = 0;
    for (lut, bank) in checks {
        for o in 0..usize::from(lut.num_objects) {
            for e in 0..usize::from(lut.num_events) {
                for &p in lut.variations(o, e) {
                    assert!(p < 0 || (p as usize) < bank.patches.len(), "{}: object {o} event {e} -> {p}", bank.name);
                    refs += usize::from(p >= 0);
                }
            }
        }
    }
    assert!(refs > 1000, "{refs}");
}

#[test]
fn ui_events_and_ambient() {
    let Some(d) = data() else { return };
    let mut s = system(&d);
    let mut ship = ShipSounds::default();
    // A click plays at full volume, centred, with priority 100 (events 0..7 are ambiences).
    let click = s.play_event(None, event::UI + 8, &mut ship);
    assert!(click >= 0);
    // The ambience returns no handle but keeps one internally; a second replaces the first.
    assert_eq!(s.play_event(None, event::UI + 1, &mut ship), NO_HANDLE);
    let first = s.ambient_handle();
    assert!(first >= 0);
    s.play_event(None, event::UI + 2, &mut ship);
    assert_ne!(s.ambient_handle(), first);
    let r = offline::render(&mut s, &[], 1.0, 3);
    assert!(levels(&r.left).rms > 10.0);
    assert!((levels(&r.left).rms / levels(&r.right).rms - 1.0).abs() < 1e-3);
}

fn ship_emitter(pos: [f32; 3]) -> Emitter {
    Emitter { position: pos, radius: 50.0, ship_class: 3, ship_type: 10, gun_sound_type: 2, coverage: 0.0, is_ship: true, visible: true }
}

#[test]
fn distance_attenuation_eq_and_pan() {
    let Some(d) = data() else { return };
    // The same explosion near and far: far is quieter and duller (distance EQ).
    let explosion = event::EXPLOSION + 2;
    let render_at = |x: f32, y: f32| {
        let mut s = system(&d);
        s.listener = Listener { eye: [0.0; 3], look_at: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0] };
        let e = Emitter { position: [x, y, 0.0], ..ship_emitter([0.0; 3]) };
        let cues = [Cue::new(0.0, Command::Event { event: explosion, emitter: Some(e) })];
        offline::render(&mut s, &cues, 2.5, 3)
    };
    let near = render_at(1000.0, 0.0);
    let far = render_at(9000.0, 0.0);
    let left = render_at(1000.0, 1000.0); // +y is the game's left here
    dump("explosion_near", &near);
    dump("explosion_far", &far);
    dump("explosion_left", &left);
    let (ln, lf) = (levels(&near.left).rms, levels(&far.left).rms);
    assert!(ln > 100.0, "near explosion too quiet: {ln}");
    // Row 70 of the tables: 239 at 1000 units, 128 at 9000 (both also trigger the secondary
    // explosion sound of row 81, with similar levels).
    assert!((0.4..0.65).contains(&(lf / ln)), "far {lf} vs near {ln}");
    let n = 1024;
    let (cn, cf) = (offline::centroid(&power_spectrum(&near.left, n), n), offline::centroid(&power_spectrum(&far.left, n), n));
    assert!(cf < cn, "distance EQ should darken: near {cn:.0} Hz, far {cf:.0} Hz");
    // Panned left (45 degrees): right channel attenuated to half once the 20-block sweep ends.
    let from = 25 * BLOCK_FRAMES;
    let (l, r) = (levels(&left.left[from..]).rms, levels(&left.right[from..]).rms);
    assert!((r / l - 0.5).abs() < 0.05, "pan -45: L {l} R {r}");
    eprintln!("explosion: near {ln:.0} far {lf:.0} rms; centroid near {cn:.0} Hz far {cf:.0} Hz; pan ratio {:.3}", r / l);
}

#[test]
fn gun_engine_and_hit_events_follow_their_rules() {
    let Some(d) = data() else { return };
    let mut s = system(&d);
    let e = ship_emitter([500.0, 0.0, 0.0]);
    let mut ship = ShipSounds::default();
    let shot = s.play_event(Some(&e), event::GUN_SHOT, &mut ship);
    assert!(shot >= 0);
    assert_eq!(ship.gun, shot);
    // While the previous shot is sounding the gun does not fire another.
    assert_eq!(s.play_event(Some(&e), event::GUN_SHOT, &mut ship), NO_HANDLE);
    let engine = s.play_event(Some(&e), event::SHIP_ENGINE, &mut ship);
    assert!(engine >= 0 && ship.engine == engine);
    assert_eq!(s.play_event(Some(&e), event::SHIP_ENGINE, &mut ship), NO_HANDLE);
    // Two concurrent hits of one kind, then the third is dropped.
    let hits: Vec<i32> = (0..3).map(|_| s.play_event(Some(&e), event::HIT + 1, &mut ship)).collect();
    assert!(hits[0] >= 0 && hits[1] >= 0 && hits[2] == NO_HANDLE, "{hits:?}");
    // Invisible objects and manager screens are silent.
    let hidden = Emitter { visible: false, ..e };
    assert_eq!(s.play_event(Some(&hidden), event::GUN_SHOT, &mut ShipSounds::default()), NO_HANDLE);
    s.context = GameContext { sensors: true, ..Default::default() };
    assert_eq!(s.play_event(Some(&e), event::HIT + 2, &mut ship), NO_HANDLE);
}

#[test]
fn voice_limit_and_priorities() {
    let Some(d) = data() else { return };
    let mut s = system(&d);
    let mut handles = Vec::new();
    for i in 0..30 {
        let h = s.play_patch(BankKind::Ships, i, None, 0, 50, 200, true);
        handles.push(h);
    }
    let m = s.mixer();
    assert!(m.voices_in_use() <= 16);
    let ok = handles.iter().filter(|&&h| h >= 0).count();
    // 14 start freely; the reserve lets each later one in only by fading out a lower-priority
    // voice, and played voices all sit at priority 50, so the rest fail.
    assert_eq!(ok, 14, "{handles:?}");
}

fn music_volume(s: &SoundSystem) -> Option<i16> {
    let m = s.mixer();
    m.stream_get_volume(m.stream_handle(3))
}

fn first_seconds_match(rendered: &[f32], reference: &[f32], gain: f32, frames: usize) -> f32 {
    let pk = peak(&reference[..frames]).max(1.0);
    rendered[..frames].iter().zip(&reference[..frames]).fold(0.0f32, |m, (a, b)| m.max((a - gain * b).abs())) / pk
}

#[test]
fn music_plays_fades_to_game_level_and_ducks() {
    let Some(d) = data() else { return };
    let Some(music) = &d.music else {
        eprintln!("no music (HWC_GAME_DIR not set); skipping");
        return;
    };
    let track = 3;
    let info = music.tracks[track];
    let mut s = system(&d);
    let cues = [Cue::new(0.0, Command::Music(track as i32))];
    let r = offline::render(&mut s, &cues, 6.0, 3);
    dump("music_track3", &r);
    let data = &music.bytes[info.data_offset..info.data_offset + info.len];
    let take = (6.5 / BLOCK_SECONDS) as usize * usize::from(info.bitrate / 8) * 2;
    let reference = decode_fquant_with(&data[..take.min(data.len())], info.bitrate, 2, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let ref_l: Vec<f32> = reference.samples.iter().step_by(2).copied().collect();
    let ref_r: Vec<f32> = reference.samples.iter().skip(1).step_by(2).copied().collect();
    // The stream starts on the first block at the track's level (255 * music volume 1.0).
    let first = 4 * BLOCK_FRAMES;
    assert!(first_seconds_match(&r.left, &ref_l, 1.0, first) < 2e-3);
    assert!(first_seconds_match(&r.right, &ref_r, 1.0, first) < 2e-3);
    // Then it fades (MUSIC_FADE_TIME = 2 s) to the in-game level: MUSIC_MAXGAME_VOL 0.9 with
    // nothing nearby, i.e. 230/255.
    let a = r.frame_at(4.0);
    let b = r.frame_at(6.0);
    let gain = 230.0 / 255.0;
    let err = r.left[a..b].iter().zip(&ref_l[a..b]).fold(0.0f32, |m, (x, y)| m.max((x - gain * y).abs()));
    assert!(err < 2e-3 * peak(&ref_l[a..b]), "level after fade: err {err}");
    assert_eq!(music_volume(&s), Some(230));

    // Letterbox ducks the in-game music to 70%: 230 -> 161.
    s.set_letterbox(true, false);
    s.music_level_now();
    let r2 = offline::render(&mut s, &[], 1.0, 3);
    assert!(r2.frames() > 0);
    assert_eq!(music_volume(&s), Some(161));
    eprintln!("music: starts at the reference level, fades to 230/255, letterbox 161/255");
}

#[test]
fn music_switch_fades_out_then_starts_next() {
    let Some(d) = data() else { return };
    if d.music.is_none() {
        return;
    }
    let mut s = system(&d);
    let mut out = offline::render(&mut s, &[Cue::new(0.0, Command::Music(3))], 1.0, 3);
    let before = music_volume(&s).unwrap();
    assert!(before > 230, "{before}");
    // Switching fades the playing track out over half a second (43 blocks)...
    let r = offline::render(&mut s, &[Cue::new(0.0, Command::Music(4))], 0.45, 3);
    out.left.extend_from_slice(&r.left);
    let v = music_volume(&s).unwrap();
    assert!(v < 30 && s.current_track() == 3, "{v}");
    // ...then starts the next one at its full level.
    let r = offline::render(&mut s, &[], 0.1, 3);
    out.left.extend_from_slice(&r.left);
    assert_eq!(s.current_track(), 4);
    assert_eq!(music_volume(&s), Some(255));
    // Stopping fades out over the given time and frees the stream.
    let r = offline::render(&mut s, &[Cue::new(0.0, Command::StopMusic(1.0))], 1.5, 3);
    assert_eq!(s.current_track(), -1);
    let m = s.mixer();
    assert!(m.stream_over(m.stream_handle(3)));
    drop(m);
    assert_eq!(levels(&r.left[r.frame_at(1.1)..]).peak, 0.0);
}

#[test]
fn opt_in_crossfade_overlaps_tracks() {
    let Some(d) = data() else { return };
    if d.music.is_none() {
        return;
    }
    let mut s = system(&d);
    let cues = [Cue::new(0.0, Command::Music(5)), Cue::new(1.0, Command::Crossfade(6, 1.0))];
    let r = offline::render(&mut s, &cues, 3.0, 3);
    dump("music_crossfade", &r);
    let at = |t: f64| levels(&r.left[r.frame_at(t)..r.frame_at(t + 0.1)]).rms;
    // No dip in the middle of the crossfade.
    assert!(at(1.45) > 0.3 * at(0.5).min(at(2.5)), "{} {} {}", at(0.5), at(1.45), at(2.5));
}

#[test]
fn looping_track_wraps_seamlessly() {
    let Some(d) = data() else { return };
    let Some(music) = &d.music else { return };
    // Shortest looping track with real content.
    let (index, info) = music
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.looping && t.duration_secs() > 5.0)
        .min_by(|a, b| a.1.duration_secs().total_cmp(&b.1.duration_secs()))
        .unwrap();
    let total = info.duration_secs();
    let frame_bytes = usize::from(info.bitrate / 8) * 2;
    let track_blocks = info.len / frame_bytes;
    let mut s = system(&d);
    s.context = GameContext { main_screen: false, ..Default::default() }; // managers: fixed level
    let r = offline::render(&mut s, &[Cue::new(0.0, Command::Music(index as i32))], total + 2.0, 3);
    // After the wrap the stream replays the track from its first block, gaplessly: the output
    // one block after the wrap equals the reference decode of block 1 onwards at the (by then
    // settled) manager-screen level, 0.2 * 255 truncated = 51.
    let bytes = &music.bytes[info.data_offset..info.data_offset + 120 * frame_bytes];
    let reference = decode_fquant_with(bytes, info.bitrate, 2, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
    let ref_l: Vec<f32> = reference.samples.iter().step_by(2).copied().collect();
    let gain = 51.0 / 255.0;
    let wrap = track_blocks * BLOCK_FRAMES;
    let pk = peak(&ref_l[BLOCK_FRAMES..]).max(1.0);
    let err = (BLOCK_FRAMES..ref_l.len()).fold(0.0f32, |m, i| m.max((r.left[wrap + i] - gain * ref_l[i]).abs()));
    assert!(err < 2e-3 * pk * gain, "loop restart differs: {err} (peak {pk})");
    let m = s.mixer();
    assert!(!m.stream_over(m.stream_handle(3)));
    drop(m);
    eprintln!("track {index} ({total:.1} s) loops back to its start without a gap");
}

#[test]
fn speech_sentences_queue_fragments_with_eq() {
    let Some(d) = data() else { return };
    for race in [Race::Somtaaw, Race::Beast] {
        let Some(speech) = d.speech(race) else {
            eprintln!("no speech for {race:?}; skipping");
            continue;
        };
        let mut s = system(&d);
        s.set_race(race);
        // Find the first sentence that has audio.
        let mut started = None;
        for ev in 0..usize::from(speech.sentences.num_events) {
            if let Some(st) = s.play_speech(ev as u32, 0, 0, 0) {
                started = Some((ev, st));
                break;
            }
        }
        let (ev, st) = started.expect("no playable sentence");
        let phrase = &speech.phrases.phrases[st.phrase_row];
        let blocks: usize = phrase.offsets.iter().map(|&o| speech.fragments[&(o as usize)].len / 64).sum();
        let r = offline::render(&mut s, &[], blocks as f64 * BLOCK_SECONDS + 1.0, 3);
        dump(&format!("speech_{race:?}_{ev}"), &r);
        // Reference: the fragments decoded back to back.
        let bytes: Vec<u8> = phrase
            .offsets
            .iter()
            .flat_map(|&o| {
                let f = &speech.fragments[&(o as usize)];
                speech.bytes[f.data_offset..f.data_offset + f.len].to_vec()
            })
            .collect();
        let reference = decode_fquant_with(&bytes, speech.bitrate(), 1, OutputRate::Hz22050, SynthesisWindow::Game).unwrap();
        let n = reference.samples.len();
        let c = offline::correlation(&r.left[..n], &reference.samples);
        let gain = levels(&r.left[..n]).rms / levels(&reference.samples).rms;
        // The radio EQ only trims the lowest and highest bands to 0.8.
        assert!(c > 0.97, "{race:?} event {ev}: correlation {c}");
        assert!((0.8..=1.0).contains(&gain), "{race:?} event {ev}: gain {gain}");
        // The radio EQ is on: the top band (bins 64..127, 5.5-11 kHz) is at 0.8 (power 0.64).
        let nfft = 512;
        let (so, sr) = (power_spectrum(&r.left[..n], nfft), power_spectrum(&reference.samples, nfft));
        let band = |sp: &[f64]| sp[(6000 * nfft / 22050)..(10000 * nfft / 22050)].iter().sum::<f64>();
        let top = band(&so) / band(&sr);
        assert!((top - 0.64).abs() < 0.05, "{race:?}: top band power ratio {top}");
        // Mono speech is centred, and the stream ends with the sentence.
        assert_eq!(r.left, r.right);
        assert!(s.speech_over(0));
        assert!(levels(&r.left[n + BLOCK_FRAMES..]).peak == 0.0);
        eprintln!("{race:?}: event {ev} row {} {} fragment(s), corr {c:.4} gain {gain:.3}, subtitles {:?}", st.phrase_row, st.fragments, st.subtitles);
    }
}

#[test]
fn scripted_scene_is_deterministic_and_balanced() {
    let Some(d) = data() else { return };
    let scene = || {
        let mut s = system(&d);
        s.listener = Listener { eye: [0.0; 3], look_at: [1.0, 0.0, 0.0], up: [0.0, 0.0, 1.0] };
        let ship = ship_emitter([800.0, -300.0, 0.0]);
        let mut cues = vec![
            Cue::new(0.0, Command::Event { event: event::UI, emitter: None }),
            Cue::new(0.1, Command::Event { event: event::SHIP_ENGINE, emitter: Some(ship) }),
            Cue::new(0.5, Command::Event { event: event::GUN_SHOT, emitter: Some(ship) }),
            Cue::new(0.9, Command::Event { event: event::EXPLOSION + 1, emitter: Some(Emitter { position: [2000.0, 900.0, 0.0], ..ship }) }),
            Cue::new(1.2, Command::Event { event: event::HIT + 1, emitter: Some(ship) }),
            Cue::new(1.5, Command::Speech { event: 3, variable: -1, variation: -1, stream: 0 }),
        ];
        if d.music.is_some() {
            cues.push(Cue::new(0.0, Command::Music(event::MUSIC_RANDOM)));
        }
        offline::render(&mut s, &cues, 4.0, 3)
    };
    let a = scene();
    let b = scene();
    assert_eq!(a.pcm, b.pcm, "offline rendering must be deterministic");
    dump("scene", &a);
    let la = levels(&a.left);
    assert!(la.rms > 200.0, "{la:?}");
    let clipped = a.pcm.iter().filter(|&&v| v == i16::MAX || v == i16::MIN).count();
    eprintln!(
        "scene: rms L {:.0} R {:.0} ({:.1} dBFS), peak {:.0}, clipped samples {clipped}",
        la.rms,
        levels(&a.right).rms,
        offline::dbfs(la.rms),
        la.peak
    );
}

