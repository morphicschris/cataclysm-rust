//! Tests over the shipped movies. Needs `HWC_GAME_DIR` (the Cataclysm install); skips cleanly
//! when it is unset. Set `HWC_FFMPEG` to an ffmpeg executable to also compare decoded pictures
//! bit for bit against ffmpeg's Bink decoder.

use std::io::Read;
use std::path::PathBuf;

use hwc_video::{BinkFile, VideoDecoder};

const MOVIES: [(&str, u32); 20] = [
    ("Animatic01", 3689),
    ("Animatic02", 2040),
    ("Animatic03", 3060),
    ("Animatic04", 1896),
    ("Animatic05", 2564),
    ("Animatic06", 1710),
    ("Animatic07", 2490),
    ("Animatic08", 2010),
    ("Animatic09", 1586),
    ("Animatic10", 1861),
    ("Animatic11", 2100),
    ("Animatic12", 1215),
    ("Animatic13", 1260),
    ("Animatic14", 1470),
    ("Animatic15", 2790),
    ("Animatic16", 2616),
    ("Animatic17", 750),
    ("Animatic18", 2520),
    ("bdogintro", 807),
    ("sierra", 240),
];

fn movies_dir() -> Option<PathBuf> {
    let Some(game) = std::env::var_os("HWC_GAME_DIR") else {
        eprintln!("HWC_GAME_DIR not set; skipping");
        return None;
    };
    Some(PathBuf::from(game).join("Movies"))
}

#[test]
fn headers_match_the_shipped_movies() {
    let Some(dir) = movies_dir() else { return };
    for (name, frames) in MOVIES {
        let movie = BinkFile::open(dir.join(format!("{name}.bik"))).unwrap();
        let h = movie.header();
        assert_eq!(h.revision, b'f', "{name}");
        assert_eq!((h.width, h.height), (640, 480), "{name}");
        assert_eq!(h.frame_count, frames, "{name}");
        assert_eq!(h.video_flags, 0, "{name}");
        assert!(movie.audio_tracks().is_empty(), "{name}");
        if name == "sierra" {
            assert_eq!((h.fps_num, h.fps_den), (1_000_000, 66_667));
        } else {
            assert_eq!((h.fps_num, h.fps_den), (30, 1), "{name}");
        }
        assert!(movie.frame_entries()[0].keyframe, "{name}");
    }
}

#[test]
fn every_frame_of_every_movie_decodes() {
    let Some(dir) = movies_dir() else { return };
    let mut total = 0u64;
    for (name, frames) in MOVIES {
        let movie = BinkFile::open(dir.join(format!("{name}.bik"))).unwrap();
        let mut dec = movie.video_decoder().unwrap();
        for i in 0..movie.frame_count() {
            let packet = movie.packet(i).unwrap();
            dec.decode(packet.video)
                .unwrap_or_else(|e| panic!("{name} frame {i}: {e}"));
        }
        assert_eq!(dec.frames_decoded(), frames as u64);
        total += dec.frames_decoded();
    }
    assert_eq!(total, 38_674);
}

#[test]
fn rgba_iterator_yields_timestamps_and_sane_pictures() {
    let Some(dir) = movies_dir() else { return };
    let movie = BinkFile::open(dir.join("sierra.bik")).unwrap();
    let frames: Vec<_> = movie.frames().unwrap().map(|f| f.unwrap()).collect();
    assert_eq!(frames.len(), 240);
    let last = frames.last().unwrap();
    assert!((last.time - 239.0 * 0.066667).abs() < 1e-9);
    // The logo at 10 s: mostly black starfield with a bright gold title.
    let f = &frames[150];
    let px = |x: usize, y: usize| &f.rgba[(y * f.width + x) * 4..(y * f.width + x) * 4 + 4];
    assert!(px(10, 10)[..3].iter().all(|&c| c < 40));
    let gold = f
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 180 && p[1] > 120 && p[2] < 120)
        .count();
    assert!(gold > 2000, "gold pixels: {gold}");
}

/// Bit-exact comparison with ffmpeg's decoder over a whole movie (opt-in).
#[test]
fn matches_ffmpeg_bit_for_bit() {
    let Some(dir) = movies_dir() else { return };
    let Some(ffmpeg) = std::env::var_os("HWC_FFMPEG") else {
        eprintln!("HWC_FFMPEG not set; skipping");
        return;
    };
    for name in ["sierra", "Animatic17"] {
        let path = dir.join(format!("{name}.bik"));
        let mut child = std::process::Command::new(&ffmpeg)
            .args(["-v", "error", "-i"])
            .arg(&path)
            .args(["-f", "rawvideo", "-pix_fmt", "yuv420p", "-"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("run ffmpeg");
        let mut out = child.stdout.take().unwrap();
        let movie = BinkFile::open(&path).unwrap();
        let mut dec = VideoDecoder::new(movie.header()).unwrap();
        let (w, h) = (640usize, 480usize);
        let mut buf = vec![0u8; w * h * 3 / 2];
        for i in 0..movie.frame_count() {
            dec.decode(movie.packet(i).unwrap().video).unwrap();
            out.read_exact(&mut buf).unwrap();
            let f = dec.yuv();
            for r in 0..h {
                assert_eq!(
                    &f.y[r * f.y_stride..r * f.y_stride + w],
                    &buf[r * w..(r + 1) * w],
                    "{name} frame {i} Y row {r}"
                );
            }
            let (cw, ch) = (w / 2, h / 2);
            for (k, plane) in [f.u, f.v].into_iter().enumerate() {
                let base = w * h + k * cw * ch;
                for r in 0..ch {
                    assert_eq!(
                        &plane[r * f.uv_stride..r * f.uv_stride + cw],
                        &buf[base + r * cw..base + (r + 1) * cw],
                        "{name} frame {i} chroma {k} row {r}"
                    );
                }
            }
        }
        child.wait().unwrap();
    }
}
