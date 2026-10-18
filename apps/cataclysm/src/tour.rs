//! Scripted walk through the menus, rendered offscreen to PNG files (`--tour <dir>`): clicks and
//! key presses are fed to the same session the window uses, so the pictures show exactly what a
//! player would see after those inputs.

use std::path::Path;

use hwc_ui::consts::key;
use hwc_ui::present::{render_offscreen, Filter};
use hwc_ui::scale::ScaleMode;
use hwc_ui::session::Session;

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
    for r in s.update(dt) {
        eprintln!("tour: request {r:?}");
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

pub fn run(mut s: Session, dir: &Path) -> Result<(), String> {
    s.resize(640, 480);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
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
                let image = match render_offscreen(&canvas, (*w, *h), viewport, Filter::Nearest) {
                    Ok(img) => img,
                    Err(e) => {
                        eprintln!("tour: GPU unavailable ({e}); composing on the CPU");
                        let mut img = hwc_ui::Canvas::new(*w as i32, *h as i32);
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
                let path = dir.join(format!("{name}.png"));
                image
                    .save_png(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
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
