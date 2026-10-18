//! Decodes a Bink movie, writes chosen frames as PNG and reports decoding speed.
//!
//! ```text
//! cargo run --release -p hwc-video --example bik2png -- <movie.bik> [options]
//!   --frames 0,120,500   frames to write (default: first, every 10 s, last)
//!   --every N            write every Nth frame instead
//!   --out DIR            output directory (default: screenshots/video/<movie name>)
//!   --strip A..B         also write frames A..B (exclusive) side by side in one contact sheet
//!   --tables FILE        codec tables: a binkw32.dll or a file written by import_tables
//!                        (default: binkw32.dll in the install the movie belongs to)
//! ```
//!
//! None of the Cataclysm movies has an audio track; when a movie does, its track layout is
//! printed (Bink audio decoding is not implemented because no shipped file needs it).

use std::path::{Path, PathBuf};
use std::time::Instant;

use hwc_video::{BinkFile, BinkTables, VideoDecoder};

fn write_png(path: &Path, width: usize, height: usize, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width as u32, height as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .expect("write png");
}

fn main() {
    let mut args = std::env::args().skip(1);
    let mut movie_path = None;
    let mut frames: Option<Vec<u32>> = None;
    let mut every: Option<u32> = None;
    let mut out: Option<PathBuf> = None;
    let mut strip: Option<(u32, u32)> = None;
    let mut tables_path: Option<PathBuf> = None;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--frames" => {
                frames = Some(
                    args.next()
                        .expect("--frames list")
                        .split(',')
                        .map(|s| s.trim().parse().expect("frame number"))
                        .collect(),
                )
            }
            "--every" => every = Some(args.next().expect("--every N").parse().expect("number")),
            "--tables" => tables_path = Some(args.next().expect("--tables FILE").into()),
            "--out" => out = Some(args.next().expect("--out DIR").into()),
            "--strip" => {
                let s = args.next().expect("--strip A..B");
                let (a, b) = s.split_once("..").expect("A..B");
                strip = Some((a.parse().unwrap(), b.parse().unwrap()));
            }
            _ => movie_path = Some(PathBuf::from(a)),
        }
    }
    let Some(movie_path) = movie_path else {
        eprintln!(
            "usage: bik2png <movie.bik> [--frames a,b,c | --every N] [--out DIR] [--strip A..B]"
        );
        std::process::exit(2);
    };

    let movie = BinkFile::open(&movie_path).expect("open movie");
    // Movies live in <install>\Movies; the DLL sits in <install>.
    let tables_path = tables_path.unwrap_or_else(|| {
        let movies = movie_path.parent().unwrap_or(Path::new("."));
        movies
            .parent()
            .unwrap_or(Path::new("."))
            .join("binkw32.dll")
    });
    let table_bytes = std::fs::read(&tables_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", tables_path.display()));
    let tables = BinkTables::from_serialized(&table_bytes)
        .or_else(|_| BinkTables::from_binkw32(&table_bytes))
        .expect("codec tables");
    let h = movie.header();
    let stem = movie_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let out = out.unwrap_or_else(|| {
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        repo.join("screenshots").join("video").join(&stem)
    });
    std::fs::create_dir_all(&out).expect("create output dir");

    println!(
        "{stem}: BIK{} {}x{} {} frames @ {}/{} fps ({:.2} s), flags {:#x}, keyframes {}",
        h.revision as char,
        h.width,
        h.height,
        h.frame_count,
        h.fps_num,
        h.fps_den,
        h.duration(),
        h.video_flags,
        movie.frame_entries().iter().filter(|e| e.keyframe).count()
    );
    if movie.audio_tracks().is_empty() {
        println!("no audio tracks");
    }
    for t in movie.audio_tracks() {
        println!(
            "audio track {}: {} Hz, {} ch, {} coding (decoding not implemented)",
            t.id,
            t.sample_rate,
            t.channels(),
            if t.uses_dct() { "DCT" } else { "RDFT" }
        );
    }

    let n = h.frame_count;
    let wanted: Vec<u32> = match (frames, every) {
        (Some(f), _) => f,
        (None, Some(e)) => (0..n).step_by(e.max(1) as usize).collect(),
        (None, None) => {
            let step = (10.0 * h.fps()).round() as u32;
            let mut v: Vec<u32> = (0..n).step_by(step.max(1) as usize).collect();
            v.push(n - 1);
            v
        }
    };

    let (w, hgt) = (h.width as usize, h.height as usize);
    let mut decoder = VideoDecoder::new(h, &tables).expect("decoder");
    let mut rgba = vec![0u8; w * hgt * 4];
    let mut sheet: Vec<Vec<u8>> = Vec::new();
    let mut decode_secs = 0.0f64;
    let mut convert_secs = 0.0f64;
    let mut written = 0;
    for i in 0..n {
        let packet = movie.packet(i).expect("packet");
        let t0 = Instant::now();
        decoder.decode(packet.video).expect("decode");
        decode_secs += t0.elapsed().as_secs_f64();
        let t1 = Instant::now();
        decoder.to_rgba(&mut rgba);
        convert_secs += t1.elapsed().as_secs_f64();
        if wanted.contains(&i) {
            let path = out.join(format!("{stem}_{i:05}.png"));
            write_png(&path, w, hgt, &rgba);
            written += 1;
        }
        if let Some((a, b)) = strip {
            if (a..b).contains(&i) {
                sheet.push(rgba.clone());
            }
        }
    }
    if !sheet.is_empty() {
        // Half-size thumbnails, four per row.
        let (tw, th) = (w / 2, hgt / 2);
        let cols = 4.min(sheet.len());
        let rows = sheet.len().div_ceil(cols);
        let sw = tw * cols;
        let mut img = vec![0u8; sw * th * rows * 4];
        for (k, f) in sheet.iter().enumerate() {
            let (ox, oy) = ((k % cols) * tw, (k / cols) * th);
            for y in 0..th {
                for x in 0..tw {
                    let s = ((2 * y) * w + 2 * x) * 4;
                    let d = ((oy + y) * sw + ox + x) * 4;
                    img[d..d + 4].copy_from_slice(&f[s..s + 4]);
                }
            }
        }
        let (a, b) = strip.unwrap();
        write_png(
            &out.join(format!("{stem}_strip_{a}-{b}.png")),
            sw,
            th * rows,
            &img,
        );
    }
    let movie_secs = h.duration();
    println!(
        "wrote {written} png to {}\ndecode: {:.3} s for {n} frames ({:.0} fps, {:.1}x real time); yuv->rgba: {:.3} s ({:.0} fps)",
        out.display(),
        decode_secs,
        n as f64 / decode_secs,
        movie_secs / decode_secs,
        convert_secs,
        n as f64 / convert_secs
    );
}
