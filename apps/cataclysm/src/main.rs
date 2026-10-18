//! Homeworld: Cataclysm recreation — the game executable. For now it runs the front end (the
//! menu system) on the player's imported data.

mod app;
mod tour;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use hwc_formats::gamedata::Layout;
use hwc_formats::GameData;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

const USAGE: &str = "usage: cataclysm [data] [options]

data: the data directory made by hwc-import (default: $HWC_DATA_DIR, then $HWC_GAME_DIR);
      for development also an original install directory or a cataclysm.big file

options:
  --scale <mode>        original (default: 1:1 and centred, like the engine), fit
                        (aspect-correct scaling), integer (whole-number scaling), stretch
  --filter <f>          nearest (default) or linear, when the front end is scaled
  --size <WxH>          initial window size (default: the configured resolution)
  --config <file>       options file (default: <data>/config/hwc-options.cfg, or
                        ./hwc-options.cfg when running from an install or archive)
  --tour <outdir>       render a scripted walk through the menus to PNG files and exit
  --exit-after <s>      close the window after s seconds (smoke tests)
  -h, --help            this text

keys: F11 toggles fullscreen; F10 cycles the scaling mode";

struct Args {
    data: Option<PathBuf>,
    scale: ScaleMode,
    linear: bool,
    size: Option<(u32, u32)>,
    config: Option<PathBuf>,
    tour: Option<PathBuf>,
    exit_after: Option<f32>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        data: None,
        scale: ScaleMode::Original,
        linear: false,
        size: None,
        config: None,
        tour: None,
        exit_after: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--scale" => {
                let v = value("--scale")?;
                a.scale = ScaleMode::parse(&v).ok_or_else(|| format!("unknown scale mode {v}"))?;
            }
            "--filter" => a.linear = value("--filter")?.eq_ignore_ascii_case("linear"),
            "--size" => {
                let v = value("--size")?;
                let (w, h) = v.split_once('x').ok_or("--size wants WxH")?;
                a.size = Some((
                    w.parse().map_err(|_| "bad width")?,
                    h.parse().map_err(|_| "bad height")?,
                ));
            }
            "--config" => a.config = Some(PathBuf::from(value("--config")?)),
            "--tour" => a.tour = Some(PathBuf::from(value("--tour")?)),
            "--exit-after" => {
                a.exit_after = Some(
                    value("--exit-after")?
                        .parse()
                        .map_err(|_| "bad --exit-after")?,
                )
            }
            s if !s.starts_with('-') && a.data.is_none() => a.data = Some(PathBuf::from(s)),
            s => return Err(format!("unknown argument {s}")),
        }
    }
    Ok(a)
}

/// Opens the game files and builds the front-end session.
fn open_session(args: &Args) -> Result<Session, String> {
    let data = match &args.data {
        Some(p) => GameData::open(p).map_err(|e| e.to_string())?,
        None => GameData::from_env()
            .ok_or(
                "no game data: pass the data directory made by hwc-import (or set HWC_DATA_DIR)",
            )?
            .map_err(|e| e.to_string())?,
    };
    let read = |p: Option<PathBuf>| {
        p.and_then(|p| std::fs::read(p).ok())
            .map(|b| String::from_utf8_lossy(&b).into_owned())
    };
    let original_cfg = read(data.install_file("Cataclysm.cfg"));
    let default_keys = read(data.install_file("default_keys.cfg"));
    let options_path = args.config.clone().or_else(|| match data.layout() {
        // Never write into an original install.
        Layout::Imported => data
            .root()
            .map(|r| r.join("config").join("hwc-options.cfg")),
        _ => Some(PathBuf::from("hwc-options.cfg")),
    });
    let source: Rc<dyn AssetSource> = Rc::new(data);
    Session::new(
        source,
        Setup {
            options_path,
            original_cfg,
            default_keys,
            scale: args.scale,
        },
    )
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            if !e.is_empty() {
                eprintln!("{e}\n");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let session = match open_session(&args) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cataclysm: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match &args.tour {
        Some(dir) => tour::run(session, Path::new(dir)),
        None => app::run(session, args.size, args.linear, args.exit_after),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cataclysm: {e}");
            ExitCode::FAILURE
        }
    }
}
