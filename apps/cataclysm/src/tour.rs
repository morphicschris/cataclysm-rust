//! Scripted walk through the start-up movies and the menus, rendered offscreen to PNG files
//! (`--tour <dir>`): the intro player, clicks and key presses are the same the window uses, so
//! the pictures show exactly what a player would see after those inputs. The sound events and
//! music cues the walk produces are printed (no audio device is opened).

use std::path::Path;
use std::rc::Rc;

use hwc_formats::GameData;
use hwc_ui::cataclysm::Request;
use hwc_ui::consts::key;
use hwc_ui::present::{render_offscreen, Filter};
use hwc_ui::scale::{Placement, ScaleMode};
use hwc_ui::session::Session;
use hwc_ui::Canvas;

use crate::intro::{Cue, Intro, SEQUENCE};
use crate::Args;

/// One step of the tour.
enum Step {
    /// Click the centre of the named atom on the current screen.
    Click(&'static str),
    /// Move the mouse onto the named atom (hover).
    Hover(&'static str),
    /// Press the left button on the named atom and keep it down.
    Press(&'static str),
    Release,
    /// Press and release a key.
    Key(u16),
    /// Press a slider (or picker) at this fraction of its width and height, drag a little,
    /// release.
    Drag(&'static str, f32),
    /// Click the radio button of a group that has this value.
    ClickValue(&'static str, u32),
    /// Click item `n` of the named list window.
    ClickItem(&'static str, usize),
    /// Run frames for this many seconds.
    Wait(f32),
    /// Save the canvas as `<name>.png`.
    Shot(&'static str),
    /// Resize the "window" with a scaling mode, present the frame through the GPU presenter
    /// offscreen and save the window image.
    Window(&'static str, u32, u32, ScaleMode),
}

use Step::*;

const STEPS: &[Step] = &[
    Wait(0.1),
    Shot("01_main_menu"),
    Hover("SinglePlayerOptions"),
    Shot("02_main_menu_hover"),
    Press("OP_Options_Start"),
    Shot("03_main_menu_pressed"),
    Release,
    Wait(0.05),
    Shot("04_audio_options"),
    Drag("OP_Music_Volume", 0.25),
    Click("OP_AutoChannels"),
    Shot("05_audio_changed"),
    Click("Audio_Options-TO-Advanced"),
    Shot("06_advanced_audio"),
    Click("Advanced-TO-Equalizer"),
    Drag("OP_Equalizer3", 0.6),
    Shot("07_equalizer"),
    Click("OP_Equalizer_To_Audio_Cancel"),
    Click("Audio_Options-TO-Video_Options"),
    Shot("08_video_options"),
    ClickValue("OP_Effects", 1),
    Shot("09_video_low_effects"),
    ClickItem("OP_Resolution", 1),
    Shot("09b_video_800x600_selected"),
    Click("OP_Options_Accept"),
    Wait(0.05),
    Shot("09c_countdown_box"),
    Click("OP_Countdown_No"),
    Wait(0.05),
    Shot("09d_countdown_reverted"),
    Click("OP_Options_Start"),
    Click("Audio_Options-TO-Video_Options"),
    Click("Video_Options-TO-Options_New"),
    Shot("10_custom_effects"),
    Click("OP_Effects_To_Video_Cancel"),
    Click("Video_Options-TO-Video_Explanation"),
    Shot("11_texture_info"),
    Click("Video_Explanation-TO-Video_Options"),
    Click("Video_Options-TO-Gameplay_Options"),
    Shot("12_gameplay_options"),
    Click("OP_HomeworldClassic"),
    Shot("13_gameplay_homeworld_classic"),
    Click("Gameplay_Options-TO-Keyboard"),
    Shot("14_key_bindings"),
    Click("OP_Options_Accept"),
    Wait(0.05),
    Shot("15_back_to_main"),
    Key(b'T' as u16),
    Wait(0.05),
    Shot("16_select_training"),
    Click("Select_tutorial-TO-Main_game_screen"),
    Click("SinglePlayerOptions"),
    Shot("17_single_player"),
    Click("Create_new_game-TO-Select_colour"),
    Drag("CP_HueSaturation", 0.1),
    Shot("18_select_colour"),
    Click("Select_colour-TO-Create_new_game"),
    Click("Create_new_game-TO-Load_game"),
    Shot("19_load_game"),
    Click("Load_game-TO-Create_new_game"),
    Click("StartSinglePlayerGame"),
    Shot("20_tutorial_alert"),
    Click("TutorialAlertCancel"),
    Key(key::ESCAPE),
    Wait(0.05),
    Shot("21_quit_game"),
    Key(b'N' as u16),
    Wait(0.05),
    Click("FE_SHOWCREDITS"),
    Wait(1.0),
    Shot("22_credits"),
    Wait(4.5),
    Shot("23_credits_crossfade"),
    Key(key::ESCAPE),
    Wait(0.05),
    Shot("24_after_credits"),
    Click("FE_Multiplayer_Game"),
    Shot("25_multiplayer_stub"),
    Click("MG_ConnectionBack"),
    Window(
        "26_window_original_1024x768",
        1024,
        768,
        ScaleMode::Original,
    ),
    Click("OP_Options_Start"),
    Window(
        "27_window_original_1280x720_audio",
        1280,
        720,
        ScaleMode::Original,
    ),
    Window("28_window_fit_1280x720_audio", 1280, 720, ScaleMode::Fit),
    Window(
        "29_window_integer_1920x1080_audio",
        1920,
        1080,
        ScaleMode::Integer,
    ),
];

fn frame(s: &mut Session, dt: f32) {
    let mut sounds = Vec::new();
    for r in s.update(dt) {
        match r {
            Request::SoundEvent(e) => sounds.push(format!("{e:#x}")),
            r => eprintln!("tour: request {r:?}"),
        }
    }
    if !sounds.is_empty() {
        eprintln!("tour: sound events {}", sounds.join(" "));
    }
}

/// Centre of a named atom of the current screen, in canvas pixels.
fn atom_centre(s: &Session, name: &str) -> Option<(i32, i32)> {
    let a = s.fe.find_atom(name)?;
    let r = s.fe.atom(a).rect();
    Some(((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2))
}

fn point_at(s: &mut Session, name: &str) -> Result<(), String> {
    let (x, y) = atom_centre(s, name)
        .ok_or_else(|| format!("no atom {name} on {:?}", s.fe.stack_names()))?;
    // The tour drives the canvas directly (no window): canvas = window pixels.
    s.fe.mouse_move(x, y);
    frame(s, 1.0 / 60.0);
    Ok(())
}

/// Saves `canvas` placed in a `size` window (through the GPU presenter when there is one).
fn save_window(
    canvas: &Canvas,
    size: (u32, u32),
    viewport: (f32, f32, f32, f32),
    path: &Path,
) -> Result<(), String> {
    let image = match render_offscreen(canvas, size, viewport, Filter::Nearest) {
        Ok(img) => img,
        Err(e) => {
            eprintln!("tour: GPU unavailable ({e}); composing on the CPU");
            let mut img = Canvas::new(size.0 as i32, size.1 as i32);
            let (x, y, vw, vh) = viewport;
            let src = hwc_ui::canvas::Image {
                width: canvas.width,
                height: canvas.height,
                pixels: canvas.pixels.clone(),
            };
            img.blit_scaled(
                &src,
                hwc_ui::Rect::from_xywh(x as i32, y as i32, vw as i32, vh as i32),
                255,
            );
            img
        }
    };
    image
        .save_png(path)
        .map_err(|e| format!("{}: {e}", path.display()))
}

/// One picture of the intro in a window of `size` with scaling `mode`.
fn intro_shot(
    intro: &Intro,
    dir: &Path,
    name: &str,
    size: (u32, u32),
    mode: ScaleMode,
) -> Result<(), String> {
    let mut canvas = Canvas::new(1, 1);
    let placement: Placement = intro.render(&mut canvas, mode, size);
    let path = dir.join(format!("{name}.png"));
    save_window(&canvas, size, placement.viewport, &path)?;
    let at = intro
        .position()
        .map_or("between movies".to_string(), |(f, n)| {
            format!("frame {f} of {n}")
        });
    println!(
        "{} (intro, {mode:?} {}x{}: {at})",
        path.display(),
        size.0,
        size.1
    );
    Ok(())
}

fn intro_advance(intro: &mut Intro, seconds: f32) {
    let mut left = seconds;
    while left > 0.0 {
        let dt = left.min(1.0 / 30.0);
        for c in intro.update(dt) {
            match c {
                Cue::PlayMusic(t) => eprintln!("tour: music track {t} starts"),
                Cue::StopMusic => eprintln!("tour: music stops"),
                Cue::Script(a) => eprintln!("tour: movie script {a:?}"),
            }
        }
        left -= dt;
    }
}

/// The start-up movies: a few frames of each, the pause between them, Escape, and the
/// placement at other window sizes.
fn intro_tour(data: Rc<GameData>, dir: &Path) -> Result<(), String> {
    let mut intro = match Intro::new(data, &SEQUENCE) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("tour: intro skipped: {e}");
            return Ok(());
        }
    };
    intro_advance(&mut intro, 0.2);
    intro_shot(
        &intro,
        dir,
        "00a_intro_sierra_start",
        (640, 480),
        ScaleMode::Original,
    )?;
    intro_advance(&mut intro, 6.0);
    intro_shot(
        &intro,
        dir,
        "00b_intro_sierra_6s",
        (640, 480),
        ScaleMode::Original,
    )?;
    intro_shot(
        &intro,
        dir,
        "00c_intro_sierra_window_1024x768",
        (1024, 768),
        ScaleMode::Original,
    )?;
    intro_advance(&mut intro, 6.0);
    intro_shot(
        &intro,
        dir,
        "00d_intro_sierra_12s",
        (640, 480),
        ScaleMode::Original,
    )?;
    eprintln!("tour: Escape released");
    let mut cues = Vec::new();
    intro.stop_movie(&mut cues);
    eprintln!("tour: cues {cues:?}");
    intro_advance(&mut intro, 1.0);
    intro_shot(
        &intro,
        dir,
        "00e_intro_pause_after_escape",
        (640, 480),
        ScaleMode::Original,
    )?;
    intro_advance(&mut intro, 1.2 + 4.0);
    intro_shot(
        &intro,
        dir,
        "00f_intro_bdogintro_4s",
        (640, 480),
        ScaleMode::Original,
    )?;
    intro_advance(&mut intro, 10.0);
    intro_shot(
        &intro,
        dir,
        "00g_intro_bdogintro_14s",
        (640, 480),
        ScaleMode::Original,
    )?;
    intro_shot(
        &intro,
        dir,
        "00h_intro_bdogintro_fit_1280x720",
        (1280, 720),
        ScaleMode::Fit,
    )?;
    intro_advance(&mut intro, 20.0);
    eprintln!("tour: intro finished: {}", intro.finished());
    Ok(())
}

pub fn run(mut s: Session, data: Rc<GameData>, args: &Args, dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    if args.intro {
        intro_tour(data, dir)?;
    }
    eprintln!(
        "tour: front end starts; music track {}",
        crate::audio::FRONT_END_TRACK
    );
    s.resize(640, 480);
    for step in STEPS {
        match step {
            Click(name) => {
                point_at(&mut s, name)?;
                s.mouse_button(true, true);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, false);
                frame(&mut s, 1.0 / 60.0);
                frame(&mut s, 1.0 / 60.0);
            }
            Hover(name) => {
                point_at(&mut s, name)?;
                frame(&mut s, 1.0 / 60.0);
            }
            Press(name) => {
                point_at(&mut s, name)?;
                s.mouse_button(true, true);
                frame(&mut s, 1.0 / 60.0);
                frame(&mut s, 1.0 / 60.0);
            }
            Release => {
                s.mouse_button(true, false);
                frame(&mut s, 1.0 / 60.0);
            }
            Key(k) => {
                s.key_down(*k);
                frame(&mut s, 1.0 / 60.0);
                s.key_up(*k);
                frame(&mut s, 1.0 / 60.0);
            }
            Drag(name, f) => {
                let a =
                    s.fe.find_atom(name)
                        .ok_or_else(|| format!("no atom {name}"))?;
                let r = s.fe.atom(a).rect();
                let x = r.x0 + (r.width() as f32 * f) as i32;
                let y = r.y0 + (r.height() as f32 * f) as i32;
                s.fe.mouse_move(r.x0 + r.width() / 2, r.y0 + r.height() / 2);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, true);
                frame(&mut s, 1.0 / 60.0);
                s.fe.mouse_move(x, y);
                frame(&mut s, 1.0 / 60.0);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, false);
                frame(&mut s, 1.0 / 60.0);
            }
            ClickValue(name, v) => {
                let screen = s.fe.current_screen().ok_or("no screen")?;
                let a =
                    s.fe.atoms_named(screen, name)
                        .into_iter()
                        .find(|&a| s.fe.atom(a).radio_value == *v)
                        .ok_or_else(|| format!("no {name} = {v}"))?;
                let r = s.fe.atom(a).rect();
                s.fe.mouse_move((r.x0 + r.x1) / 2, (r.y0 + r.y1) / 2);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, true);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, false);
                frame(&mut s, 1.0 / 60.0);
            }
            ClickItem(name, n) => {
                let a =
                    s.fe.find_atom(name)
                        .ok_or_else(|| format!("no atom {name}"))?;
                let rect = s.fe.atom(a).rect();
                let y = {
                    let l =
                        s.fe.list(a)
                            .ok_or_else(|| format!("{name} is not a list"))?;
                    (rect.y0..rect.y1)
                        .find(|&y| l.item_at(rect, y) == Some(*n))
                        .ok_or_else(|| format!("{name} has no visible item {n}"))?
                };
                s.fe.mouse_move((rect.x0 + rect.x1) / 2, y + 2);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, true);
                frame(&mut s, 1.0 / 60.0);
                s.mouse_button(true, false);
                frame(&mut s, 1.0 / 60.0);
            }
            Wait(t) => {
                let mut left = *t;
                while left > 0.0 {
                    frame(&mut s, 1.0 / 60.0);
                    left -= 1.0 / 60.0;
                }
            }
            Window(name, w, h, mode) => {
                s.set_scale(*mode);
                s.resize(*w, *h);
                frame(&mut s, 1.0 / 60.0);
                let viewport = s.placement.viewport;
                let canvas = s.render().clone();
                let path = dir.join(format!("{name}.png"));
                save_window(&canvas, (*w, *h), viewport, &path)?;
                println!(
                    "{} ({mode:?} {w}x{h}: {})",
                    path.display(),
                    s.fe.stack_names().join(" > ")
                );
            }
            Shot(name) => {
                let path = dir.join(format!("{name}.png"));
                s.render()
                    .save_png(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                println!("{} ({})", path.display(), s.fe.stack_names().join(" > "));
            }
        }
    }
    Ok(())
}
