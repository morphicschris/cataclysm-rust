//! Homeworld: Cataclysm recreation — the game executable: the start-up movies with their
//! music, the front end (the menu system) with its music, sounds and bitmap pointer, and from
//! Single Player -> New Game the loading screen and the mission on the simulation, with the
//! 3D view, the in-game interface, the game's controls, music, speech and sound.

mod app;
mod audio;
#[cfg(test)]
mod campaign_shots;
mod data;
mod game;
mod host;
mod intro;
mod loading;
mod mission;
mod movie_script;
mod nebula_view;
mod nis;
#[cfg(test)]
mod nis_shots;
mod play;
mod preview;
mod sensors;
#[cfg(test)]
mod smoke;
#[cfg(test)]
mod speech;
mod tour;
mod world;

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
  --fullscreen          full screen in the configured display mode (default, like the
                        original's /fullscreen; the intro runs in 640x480)
  --window              in a window of the configured size (the original's /window)
  --skip-intro          start at the main menu (the original's hidden /disableAVI)
  --scale <mode>        original (default: 1:1 and centred, like the engine), fit
                        (aspect-correct scaling), integer (whole-number scaling), stretch;
                        also applies to the movies
  --filter <f>          nearest (default) or linear, when the picture is scaled
  --size <WxH>          window size (default: the configured resolution)
  --config <file>       options file (default: <data>/config/hwc-options.cfg, or
                        ./hwc-options.cfg when running from an install or archive)
  --no-sound            do not open an audio device
  --tour <outdir>       render a scripted walk through the intro and the menus to PNG
                        files and exit
  --exit-after <s>      close the window after s seconds (smoke tests)
  --mission <n>         go straight from the front end into mission n (development;
                        missions after the first start with the warp.script fleet)
  --debug-keys          the original's single-player debug keys (compiled out of the
                        retail game): C forces the jump to the next mission
  --ingame-preview      starting a game opens the in-game interface (managers, overlay,
                        menus, keys) on a stand-in fleet instead of returning to the menu
  --rollcall-below-subtitles
                        open the hyperspace roll call below the speech band (the original
                        keeps it in the top-left corner, under the band and portrait)
  -h, --help            this text

The original's switches /window, /fullscreen and /disableAVI are accepted too.
keys: Escape (released) stops the movie playing; F11 toggles borderless full screen;
      F10 cycles the scaling mode; in a mission the game's key bindings (keys.cfg)";

pub struct Args {
    pub data: Option<PathBuf>,
    pub scale: ScaleMode,
    pub linear: bool,
    pub size: Option<(u32, u32)>,
    pub config: Option<PathBuf>,
    pub tour: Option<PathBuf>,
    pub exit_after: Option<f32>,
    pub fullscreen: bool,
    pub intro: bool,
    pub sound: bool,
    pub ingame_preview: bool,
    pub mission: Option<u32>,
    pub debug_keys: bool,
    pub rollcall_below_band: bool,
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
        fullscreen: true,
        intro: true,
        sound: true,
        ingame_preview: false,
        mission: None,
        debug_keys: false,
        rollcall_below_band: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        // The original's command-line switches (matched without regard to case).
        match arg.to_ascii_lowercase().as_str() {
            "/window" => {
                a.fullscreen = false;
                continue;
            }
            "/fullscreen" => {
                a.fullscreen = true;
                continue;
            }
            "/disableavi" => {
                a.intro = false;
                continue;
            }
            _ => {}
        }
        match arg.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--window" => a.fullscreen = false,
            "--fullscreen" => a.fullscreen = true,
            "--skip-intro" => a.intro = false,
            "--no-sound" => a.sound = false,
            "--ingame-preview" => a.ingame_preview = true,
            "--debug-keys" => a.debug_keys = true,
            "--rollcall-below-subtitles" => a.rollcall_below_band = true,
            "--mission" => {
                a.mission = Some(value("--mission")?.parse().map_err(|_| "bad --mission")?)
            }
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

/// Opens the game files (the data directory, or for development an install or archive).
pub fn open_game_data(args: &Args) -> Result<GameData, String> {
    match &args.data {
        Some(p) => GameData::open(p).map_err(|e| e.to_string()),
        None => GameData::from_env()
            .ok_or(
                "no game data: pass the data directory made by hwc-import (or set HWC_DATA_DIR)",
            )?
            .map_err(|e| e.to_string()),
    }
}

/// Builds the front-end session on the game files.
fn open_session(args: &Args, data: Rc<GameData>) -> Result<Session, String> {
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
    // The player's key bindings live beside the options file.
    let keys_path = options_path.as_ref().map(|p| p.with_file_name("keys.cfg"));
    let keys = read(keys_path.clone());
    let source: Rc<dyn AssetSource> = data;
    Session::new(
        source,
        Setup {
            options_path,
            original_cfg,
            default_keys,
            keys,
            keys_path,
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
    let opened = open_game_data(&args).map(Rc::new).and_then(|data| {
        let session = open_session(&args, data.clone())?;
        Ok((data, session))
    });
    let (data, session) = match opened {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cataclysm: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = match &args.tour {
        Some(dir) => tour::run(session, data, &args, Path::new(dir)),
        None => app::run(session, data, &args),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cataclysm: {e}");
            ExitCode::FAILURE
        }
    }
}
