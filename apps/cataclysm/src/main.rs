//! Homeworld: Cataclysm recreation — the game executable: the start-up movies with their
//! music, the front end (the menu system) with its music, sounds and bitmap pointer, and from
//! Single Player -> New Game the loading screen and the mission on the simulation, with the
//! 3D view, the in-game interface, the game's controls, music, speech and sound.

mod app;
mod audio;
mod bandbox;
#[cfg(test)]
mod build_shots;
#[cfg(test)]
mod buildui_shots;
#[cfg(test)]
mod campaign_shots;
mod crash;
mod data;
mod draw_check;
mod game;
#[cfg(test)]
mod hang_checks;
mod host;
mod hotkeys;
#[cfg(test)]
mod hud_shots;
mod hwr_fx;
#[cfg(test)]
mod hwr_shots;
mod intel_trace;
mod intro;
mod loading;
#[cfg(test)]
mod m1_checks;
#[cfg(test)]
mod menus_shots;
mod mission;
mod movie_script;
mod nebula_view;
mod nis;
#[cfg(test)]
mod nis_shots;
mod pings;
mod plate;
mod play;
mod preview;
mod profile;
#[cfg(test)]
mod quality_shots;
#[cfg(test)]
mod resui_shots;
#[cfg(test)]
mod save_checks;
mod saves;
#[cfg(test)]
mod screen_shots;
mod sensors;
mod ship_preview;
#[cfg(test)]
mod smoke;
#[cfg(test)]
mod special_actions;
#[cfg(test)]
mod speech;
mod tactical;
#[cfg(test)]
mod test_gpu;
mod tour;
mod watchdog;
mod world;

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use hwc_formats::gamedata::Layout;
use hwc_formats::GameData;
use hwc_ui::display::Mode;
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::{Session, Setup};
use hwc_ui::AssetSource;

const USAGE: &str = "usage: cataclysm [data] [options]

data: the data directory made by hwc-import (default: $HWC_DATA_DIR, then $HWC_GAME_DIR);
      for development also an original install directory or a cataclysm.big file

options:
  --display-mode <m>    borderless (default, a user-requested change: a borderless window
                        covering the monitor at its own resolution), windowed (a resizable
                        window of the configured size, centred) or fullscreen (exclusive
                        full screen at the configured resolution and refresh rate; the
                        intro runs in 640x480, as the original's); for this run, the options
                        file's hwcDisplayMode = 0 window / 1 borderless / 2 full screen
                        otherwise. Alt+Enter (or F11) switches between the window and the
                        last full-screen mode
  --borderless          --display-mode borderless
  --window              --display-mode windowed (the original's /window)
  --fullscreen          --display-mode fullscreen (the original's /fullscreen)
  --skip-intro          start at the main menu (the original's hidden /disableAVI)
  --scale <mode>        original (default: 1:1 and centred, like the engine), fit
                        (aspect-correct scaling), integer (whole-number scaling), stretch;
                        also applies to the movies, which otherwise stretch to fill
                        the window (a user-requested change: the original shows them
                        unscaled)
  --filter <f>          nearest (default) or linear, when the picture is scaled
  --size <WxH>          window or full-screen size for this run (default: the configured
                        resolution, screenWidth x screenHeight; the menus list 640x480 to
                        5120x1440 and the monitor's own full-screen modes)
  --config <file>       options file (default: <data>/config/hwc-options.cfg, or
                        ./hwc-options.cfg when running from an install or archive)
  --no-sound            do not open an audio device
                        (the music slider starts at 45, a user-requested change: the
                        original's default is 60, this is 25% lower; a MusicVolume saved in the
                        options file is kept)
  --tour <outdir>       render a scripted walk through the intro and the menus to PNG
                        files and exit
  --exit-after <s>      close the window after s seconds (smoke tests)
  --mission <n>         go straight from the front end into mission n (development;
                        missions after the first start with the warp.script fleet)
  --debug-keys          the original's single-player debug keys (compiled out of the
                        retail game): C forces the jump to the next mission
  --ingame-preview      starting a game opens the in-game interface (managers, overlay,
                        menus, keys) on a stand-in fleet instead of returning to the menu
  --autoplay            (development) the simulation's test player plays the missions:
                        builds, harvests, fights and jumps when the mission allows
  --rollcall-below-subtitles
                        open the hyperspace roll call below the speech band (the original
                        keeps it in the top-left corner, under the band and portrait)
  --lod <mode>          full (default, a user-requested change: ships are drawn at their
                        most detailed level at any distance) or original (the original's
                        level-of-detail selection); options file: hwcFullDetailShips = 1/0
  --right-click <mode>  modern (default, a user-requested change: with ships selected a
                        right click on empty space opens the move plate, Shift+right click
                        the menu) or original (the menu, or the context-sensitive action);
                        options file: hwcRightClickMove = 1/0
                        (double-clicking one of your ships selects all your ships of that
                        type on screen, a user-requested change; options file:
                        hwcDoubleClickSelectType = 1/0)
  --order-lines <mode>  modern (default, a user-requested change: lines from the selected
                        ships to what they were ordered to, with a circle there, coloured by
                        the order: green move, red attack, yellow harvest, blue dock, light
                        blue guard, purple special) or original (none); options file:
                        hwcOrderLines = 1/0
  --ship-icons <mode>   all (default, a user-requested change: class icons over every ship),
                        near (over the ships near the camera, fading out with distance) or
                        none (the original); Tab cycles them in the game and Shift+Tab is
                        then Toggle Ship Info (options file: hwcShipIcons = 2/1/0,
                        hwcTabShipIcons = 1/0 to give Tab back to Toggle Ship Info)
  --health-bars <mode>  modern (default, a user-requested change: a selected ship's health
                        bar has a thin black outline and white [ ] brackets) or original;
                        options file: hwcHealthBrackets = 1/0
  --zoom <mode>         smooth (default, a user-requested change: each mouse-wheel notch
                        eases the camera to its distance) or original (the original's
                        immediate step); options file: hwcSmoothZoom = 1/0
  --focus-zoom <on|off> on (default, as the original): focusing again on the ships already
                        in focus (F twice) eases the camera in to frame them; off: a
                        repeated focus does nothing. Options file: hwcFocusZoom = 1/0
  --zoom-limit <mode>   hull (default, a user-requested addition: the closest zoom on the
                        focused ships also keeps the near clip plane outside their hulls) or
                        original (the original's limit from their collision size and minimum
                        zoom distance alone); options file: hwcZoomClear = 1/0
  --draw-distance <m>   original (default: ships, effects and asteroids are drawn within a
                        distance of the camera's look-at point by class, Tweak.script's
                        render list) or far (everything within the far clip distance of the
                        eye); both are radial, the same at any camera angle. Options file:
                        hwcRenderList = 1/0
  Remastered effects    hwcRemasteredFx = 1 in the options file (a user-requested addition,
                        off by default, also on the ENHANCEMENTS screen): ship deaths and
                        engine trails use Homeworld Remastered's effects, imported from your
                        own Remastered install with hwc-import --hwr (looked for in
                        $HWC_HWR_DIR, then hwr/ in the data folder, then next to the game)
  --hud <mode>          modern (default, a user-requested change: RU and SU bars in the
                        top-right corner, in the style of the later Homeworld games) or
                        original (the SU / RU / time box, bottom right); options file:
                        hwcModernHud = 1/0. The original HUD also brings back the
                        original build and research managers and menus (unless
                        --build-ui, --research-ui or --menus says otherwise)
  --build-ui <mode>     modern (default, a user-requested change: the build manager in the
                        modern HUD's look, same controls, keys and actions) or original
                        (the original screen); options file: hwcModernBuildManager = 1/0
  --research-ui <mode>  modern (default, a user-requested change: the research manager in
                        the modern HUD's look, same controls, keys and actions) or original
                        (the original screen); options file: hwcModernResearchManager = 1/0
  --menus <mode>        modern (default, a user-requested change: the in-game escape
                        menu, save / load, quit and the other confirmations, the options
                        and all their screens (audio, speech, equalizer, video, effects,
                        gameplay, key bindings), the objectives and the systems (launch)
                        manager in the modern HUD's look, same controls, keys and actions,
                        plus an ENHANCEMENTS screen in the options for the modern options
                        below, applied at once) or original (the original screens); options
                        file: hwcModernMenus = 1/0. The original HUD brings back the
                        original screens too
  --letterbox <mode>    tall (default, a user-requested change: the cutscene and fleet-intel
                        bars are never thinner than on a 4:3 screen of the same height, 14%
                        of it each) or original (bars around a 1.85:1 picture: on a 16:9
                        screen only 2% each); options file: hwcOriginalLetterbox = 0/1

3D view quality (modern options, all off by default: the original look; see
docs/render-quality.md for the measured costs):
  --render-scale <s>    internal render scale 0.5..2.0 (default 1). Above 1 the 3D view is
                        supersampled (drawn larger, filtered down); below 1 it is cheaper and
                        softer. The interface and text stay at native resolution. Cost grows
                        with the square: 1.5 = 2.25x the pixels, 2.0 = 4x. Options file:
                        hwcRenderScale = 50..200 (percent)
  --msaa <n>            multisample anti-aliasing: 1 (off), 2, 4 or 8, limited to what the
                        GPU supports; covers ships, effects, trails, lines and nebulae.
                        Options file: hwcMsaa = n
  --fxaa <on|off>       FXAA post filter over the 3D view (cheap; softens edges and fine
                        detail a little). Options file: hwcFxaa = 1/0
  --scene-res <mode>    window (default) or layout: with --scale fit/integer/stretch the 3D
                        view is drawn at the window's pixels under the scaled 640x480
                        interface (window), or at the layout's 640x480 pixels and scaled up
                        with it (layout). Options file: hwcSceneAtWindow = 1/0
  A frame never draws more than 16.6 M samples (3840x2160 at 2x MSAA): over that the
  render scale, then the MSAA samples, are cut (a note is printed), which keeps a frame on an
  integrated GPU well clear of a driver reset; the scale is also kept within the GPU's
  texture size limit.
  Measured on an Intel UHD 750, Mission 1 combat at 1600x900 (mean frame; off: 13-14 ms):
  FXAA 16-18 ms, MSAA 2 15-18, MSAA 4 16-20, MSAA 8 18-22, scale 1.5 18-20, scale 2 19-22,
  scale 2 + MSAA 4 22-26 (cut to scale 1.70)
  --no-vsync            present without waiting for the display (for measuring)
  --frame-times         print the 3D frames' draw times every 240 frames
  --profile             (or HWC_PROFILE=1) print a per-section breakdown of the frame once
                        a second: CPU time of each step of the sensors manager's view (fog,
                        range discs, explored outlines, points, meshes, pings, HUD) and of the
                        main view, GPU time of each pass (timestamp queries, when the GPU has
                        them), pixel and draw counts, canvas copies and the upload

  -h, --help            this text

The original's switches /window, /fullscreen and /disableAVI are accepted too.
keys: Escape (released) stops the movie playing; Alt+Enter or F11 switches between a
      window and the last full-screen mode; F10 cycles the scaling mode; in a mission the game's key bindings (keys.cfg)";

pub struct Args {
    pub data: Option<PathBuf>,
    pub scale: ScaleMode,
    /// `--scale` was given (it then applies to the movies too).
    pub scale_given: bool,
    pub linear: bool,
    pub size: Option<(u32, u32)>,
    pub config: Option<PathBuf>,
    pub tour: Option<PathBuf>,
    pub exit_after: Option<f32>,
    /// `--display-mode`, `--window`, `--borderless`, `--fullscreen` (None: the options file).
    pub display_mode: Option<hwc_ui::display::Mode>,
    pub intro: bool,
    pub sound: bool,
    pub ingame_preview: bool,
    pub mission: Option<u32>,
    pub debug_keys: bool,
    pub rollcall_below_band: bool,
    pub autoplay: bool,
    /// `--lod`: Some(true) full detail, Some(false) the original's selection, None the
    /// options file's `hwcFullDetailShips`.
    pub full_detail: Option<bool>,
    /// `--right-click`: Some(true) modern, Some(false) original, None the options file's
    /// `hwcRightClickMove`.
    pub right_click_move: Option<bool>,
    /// `--order-lines`, `--ship-icons` (the `hwcShipIcons` value), `--health-bars`; None: the
    /// options file.
    pub order_lines: Option<bool>,
    pub ship_icons: Option<i32>,
    pub health_brackets: Option<bool>,
    /// `--zoom`: Some(true) smooth, Some(false) the original's step, None the options file.
    pub smooth_zoom: Option<bool>,
    /// `--focus-zoom`, `--zoom-limit` (Some(true): hull), `--draw-distance` (Some(true):
    /// original); None: the options file.
    pub focus_zoom: Option<bool>,
    pub zoom_clear: Option<bool>,
    pub render_list: Option<bool>,
    /// `--letterbox` (Some(true): tall); None: the options file.
    pub tall_letterbox: Option<bool>,
    /// `--render-scale`, `--msaa`, `--fxaa`, `--scene-res` (None: the options file).
    pub render_scale: Option<f32>,
    pub msaa: Option<u32>,
    pub fxaa: Option<bool>,
    pub scene_at_window: Option<bool>,
    pub no_vsync: bool,
    pub frame_times: bool,
}

fn parse_args() -> Result<Args, String> {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut a = Args {
        data: None,
        scale: ScaleMode::Original,
        scale_given: false,
        linear: false,
        size: None,
        config: None,
        tour: None,
        exit_after: None,
        display_mode: None,
        intro: true,
        sound: true,
        ingame_preview: false,
        mission: None,
        debug_keys: false,
        rollcall_below_band: false,
        autoplay: false,
        full_detail: None,
        right_click_move: None,
        order_lines: None,
        ship_icons: None,
        health_brackets: None,
        smooth_zoom: None,
        focus_zoom: None,
        zoom_clear: None,
        render_list: None,
        tall_letterbox: None,
        render_scale: None,
        msaa: None,
        fxaa: None,
        scene_at_window: None,
        no_vsync: false,
        frame_times: false,
    };
    let mut it = args.into_iter();
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        // The original's command-line switches (matched without regard to case).
        match arg.to_ascii_lowercase().as_str() {
            "/window" => {
                a.display_mode = Some(Mode::Windowed);
                continue;
            }
            "/fullscreen" => {
                a.display_mode = Some(Mode::Fullscreen);
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
            "--window" | "--windowed" => a.display_mode = Some(Mode::Windowed),
            "--borderless" => a.display_mode = Some(Mode::Borderless),
            "--fullscreen" => a.display_mode = Some(Mode::Fullscreen),
            "--display-mode" => {
                let v = value("--display-mode")?;
                a.display_mode =
                    Some(Mode::parse(&v).ok_or_else(|| format!("unknown --display-mode {v}"))?);
            }
            "--skip-intro" => a.intro = false,
            "--no-sound" => a.sound = false,
            "--ingame-preview" => a.ingame_preview = true,
            "--debug-keys" => a.debug_keys = true,
            "--autoplay" => a.autoplay = true,
            "--rollcall-below-subtitles" => a.rollcall_below_band = true,
            "--mission" => {
                a.mission = Some(value("--mission")?.parse().map_err(|_| "bad --mission")?)
            }
            "--scale" => {
                let v = value("--scale")?;
                a.scale = ScaleMode::parse(&v).ok_or_else(|| format!("unknown scale mode {v}"))?;
                a.scale_given = true;
            }
            "--lod" => {
                a.full_detail = Some(match value("--lod")?.to_ascii_lowercase().as_str() {
                    "full" => true,
                    "original" => false,
                    v => return Err(format!("unknown --lod mode {v}")),
                })
            }
            "--right-click" => {
                a.right_click_move = Some(
                    match value("--right-click")?.to_ascii_lowercase().as_str() {
                        "modern" => true,
                        "original" => false,
                        v => return Err(format!("unknown --right-click mode {v}")),
                    },
                )
            }
            "--order-lines" => {
                a.order_lines = Some(
                    match value("--order-lines")?.to_ascii_lowercase().as_str() {
                        "modern" => true,
                        "original" => false,
                        v => return Err(format!("unknown --order-lines mode {v}")),
                    },
                )
            }
            "--ship-icons" => {
                a.ship_icons = Some(match value("--ship-icons")?.to_ascii_lowercase().as_str() {
                    "all" => 2,
                    "near" => 1,
                    "none" | "original" => 0,
                    v => return Err(format!("unknown --ship-icons mode {v}")),
                })
            }
            "--health-bars" => {
                a.health_brackets = Some(
                    match value("--health-bars")?.to_ascii_lowercase().as_str() {
                        "modern" => true,
                        "original" => false,
                        v => return Err(format!("unknown --health-bars mode {v}")),
                    },
                )
            }
            "--zoom" => {
                a.smooth_zoom = Some(match value("--zoom")?.to_ascii_lowercase().as_str() {
                    "smooth" => true,
                    "original" => false,
                    v => return Err(format!("unknown --zoom mode {v}")),
                })
            }
            "--focus-zoom" => {
                a.focus_zoom = Some(match value("--focus-zoom")?.to_ascii_lowercase().as_str() {
                    "on" => true,
                    "off" => false,
                    v => return Err(format!("unknown --focus-zoom value {v}")),
                })
            }
            "--zoom-limit" => {
                a.zoom_clear = Some(match value("--zoom-limit")?.to_ascii_lowercase().as_str() {
                    "hull" => true,
                    "original" => false,
                    v => return Err(format!("unknown --zoom-limit mode {v}")),
                })
            }
            "--draw-distance" => {
                a.render_list = Some(
                    match value("--draw-distance")?.to_ascii_lowercase().as_str() {
                        "original" => true,
                        "far" => false,
                        v => return Err(format!("unknown --draw-distance mode {v}")),
                    },
                )
            }
            "--letterbox" => {
                a.tall_letterbox = Some(match value("--letterbox")?.to_ascii_lowercase().as_str() {
                    "tall" => true,
                    "original" => false,
                    v => return Err(format!("unknown --letterbox mode {v}")),
                })
            }
            "--build-ui" => {
                let v = value("--build-ui")?;
                let modern = hwc_ui::ingame::modern_build_manager::parse_mode(&v)
                    .ok_or_else(|| format!("unknown --build-ui mode {v}"))?;
                hwc_ui::ingame::modern_build_manager::set_override(Some(modern));
            }
            "--menus" => {
                let v = value("--menus")?;
                let modern = hwc_ui::ingame::modern_menu::parse_mode(&v)
                    .ok_or_else(|| format!("unknown --menus mode {v}"))?;
                hwc_ui::ingame::modern_menu::set_override(Some(modern));
            }
            "--research-ui" => {
                let v = value("--research-ui")?;
                let modern = hwc_ui::ingame::modern_research_manager::parse_mode(&v)
                    .ok_or_else(|| format!("unknown --research-ui mode {v}"))?;
                hwc_ui::ingame::modern_research_manager::set_override(Some(modern));
            }
            "--profile" => crate::profile::enable(),
            "--hud" => {
                let v = value("--hud")?;
                let modern = hwc_ui::ingame::modern_hud::parse_mode(&v)
                    .ok_or_else(|| format!("unknown --hud mode {v}"))?;
                hwc_ui::ingame::modern_hud::set_override(Some(modern));
            }
            "--render-scale" => {
                let v: f32 = value("--render-scale")?
                    .parse()
                    .map_err(|_| "bad --render-scale")?;
                if !(hwc_render::quality::SCALE_MIN..=hwc_render::quality::SCALE_MAX).contains(&v) {
                    return Err(format!(
                        "--render-scale must be {}..{}",
                        hwc_render::quality::SCALE_MIN,
                        hwc_render::quality::SCALE_MAX
                    ));
                }
                a.render_scale = Some(v);
            }
            "--msaa" => {
                let n: u32 = value("--msaa")?.parse().map_err(|_| "bad --msaa")?;
                if ![1, 2, 4, 8].contains(&n) {
                    return Err("--msaa must be 1, 2, 4 or 8".into());
                }
                a.msaa = Some(n);
            }
            "--fxaa" => {
                a.fxaa = Some(match value("--fxaa")?.to_ascii_lowercase().as_str() {
                    "on" | "1" => true,
                    "off" | "0" => false,
                    v => return Err(format!("unknown --fxaa value {v}")),
                })
            }
            "--scene-res" => {
                a.scene_at_window =
                    Some(match value("--scene-res")?.to_ascii_lowercase().as_str() {
                        "window" => true,
                        "layout" => false,
                        v => return Err(format!("unknown --scene-res mode {v}")),
                    })
            }
            "--no-vsync" => a.no_vsync = true,
            "--frame-times" => a.frame_times = true,
            "--filter" => a.linear = value("--filter")?.eq_ignore_ascii_case("linear"),
            "--size" => {
                let v = value("--size")?;
                let (w, h) = hwc_ui::display::parse_size(&v).ok_or("--size wants WxH")?;
                a.size = Some((w as u32, h as u32));
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

/// Whether `path` (a file that may not exist yet) lies inside the directory `dir`.
fn is_inside(dir: &Path, path: &Path) -> bool {
    let Ok(dir) = dir.canonicalize() else {
        return false;
    };
    let Ok(abs) = std::path::absolute(path) else {
        return false;
    };
    // The nearest folder that exists, as the file system names it.
    abs.ancestors()
        .skip(1)
        .find_map(|a| a.canonicalize().ok())
        .is_some_and(|p| p.starts_with(&dir))
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
    // Never write into an original install, even when run from inside it: the options and
    // key bindings are then kept for this run only.
    let options_path = options_path.filter(|p| {
        let inside = data.layout() == Layout::Install
            && data.root().is_some_and(|root| is_inside(root, p));
        if inside {
            eprintln!(
                "cataclysm: {} is inside the original install; options will not be saved                  (pass --config <file> elsewhere)",
                p.display()
            );
        }
        !inside
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
    crash::install();
    // A main loop that stops responding writes where it was to the crash log.
    watchdog::install(watchdog::to_crash_log);
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
            // The game cannot start: the reason goes into the crash log too.
            crash::append(&format!("cataclysm: the game could not start: {e}\n"));
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
            // Every way out on an error goes into the crash log too.
            crash::log(&format!("the game stopped: {e}"));
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_inside_the_install_are_refused() {
        let root = std::env::temp_dir().join(format!("hwc-inside-{}", std::process::id()));
        let install = root.join("Cataclysm");
        std::fs::create_dir_all(&install).unwrap();
        assert!(is_inside(&install, &install.join("hwc-options.cfg")));
        assert!(is_inside(
            &install,
            &install.join("config").join("keys.cfg")
        ));
        assert!(!is_inside(&install, &root.join("hwc-options.cfg")));
        assert!(!is_inside(&install, &root.join("Cataclysm2").join("x.cfg")));
        let _ = std::fs::remove_dir_all(&root);
    }

    fn parse(args: &[&str]) -> Result<Args, String> {
        parse_args_from(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn screen_mode_switches() {
        // None: the options file decides (borderless by default).
        assert_eq!(parse(&[]).unwrap().display_mode, None);
        for (args, want) in [
            (&["--window"][..], Mode::Windowed),
            (&["--windowed"], Mode::Windowed),
            (&["/WINDOW"], Mode::Windowed),
            (&["--borderless"], Mode::Borderless),
            (&["--fullscreen"], Mode::Fullscreen),
            (&["/fullscreen"], Mode::Fullscreen),
            (&["--display-mode", "windowed"], Mode::Windowed),
            (&["--display-mode", "Borderless"], Mode::Borderless),
            (&["--display-mode", "fullscreen"], Mode::Fullscreen),
            // The last one wins.
            (&["--fullscreen", "--window"], Mode::Windowed),
        ] {
            assert_eq!(parse(args).unwrap().display_mode, Some(want), "{args:?}");
        }
        assert!(parse(&["--display-mode", "tv"]).is_err());
        assert!(parse(&["--display-mode"]).is_err());
    }

    #[test]
    fn size_switch() {
        assert_eq!(
            parse(&["--size", "5120x1440"]).unwrap().size,
            Some((5120, 1440))
        );
        assert_eq!(
            parse(&["--size", "3440X1440"]).unwrap().size,
            Some((3440, 1440))
        );
        assert!(parse(&["--size", "wide"]).is_err());
        assert!(parse(&["--size", "0x0"]).is_err());
    }
}
