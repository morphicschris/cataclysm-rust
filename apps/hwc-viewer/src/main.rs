//! Interactive ship viewer for Homeworld: Cataclysm data.
//!
//! ```text
//! hwc-viewer <cataclysm.big> [ship-name] [--background btg] [--lighting hsf]
//!            [--screenshot out.png [--size WxH] [--angle deg] [--declination deg]
//!             [--distance d] [--time s] [--team n|off]]
//! ```

mod app;
mod viewer;

use std::path::Path;
use std::process::ExitCode;

use hwc_formats::big::BigArchive;
use hwc_render::{headless_device, OffscreenTarget, Renderer, OFFSCREEN_FORMAT};

use viewer::Viewer;

const USAGE: &str = "usage: hwc-viewer <cataclysm.big> [ship-name] [options]

options:
  --background <btg>      sky background, e.g. mission13 or btg\\m05.btg
  --lighting <hsf>        scene lighting, e.g. mission13 (default hsf\\default.hsf)
  --team <0-7|off>        team colour scheme (default 0)
  --no-navlights          hide nav lights
  --screenshot <out.png>  render one frame offscreen, save it and exit
    --size <WxH>          image size (default 1280x960)
    --angle <deg>         camera azimuth
    --declination <deg>   camera elevation (negative is above the ship)
    --distance <d>        camera distance
    --fov <deg>           vertical field of view (default from camera.script)
    --time <s>            seconds since the ship appeared (nav light phase, default 0.75)

controls:
  right-drag (or left-drag) rotate, both buttons drag or wheel zoom, +/- zoom
  Left/Right previous/next ship, PageUp/PageDown previous/next race
  B / Shift+B next/previous background, T cycle team colours, N nav lights,
  Home reset camera, Esc quit";

#[derive(Debug, Default)]
struct Options {
    big: String,
    ship: Option<String>,
    background: Option<String>,
    lighting: Option<String>,
    team: Option<Option<usize>>,
    nav_lights: bool,
    screenshot: Option<String>,
    size: (u32, u32),
    angle: Option<f32>,
    declination: Option<f32>,
    distance: Option<f32>,
    fov: Option<f32>,
    time: f32,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        nav_lights: true,
        size: (1280, 960),
        time: 0.75,
        ..Default::default()
    };
    let mut positional = Vec::new();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().ok_or(format!("{arg} needs a value"));
        let num = |s: String| {
            s.parse::<f32>()
                .map_err(|_| format!("{arg}: {s:?} is not a number"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Err(USAGE.into()),
            "--background" => o.background = Some(value()?),
            "--lighting" => o.lighting = Some(value()?),
            "--team" => {
                let v = value()?;
                o.team = Some(if v.eq_ignore_ascii_case("off") {
                    None
                } else {
                    Some(v.parse().map_err(|_| format!("--team: bad value {v:?}"))?)
                });
            }
            "--no-navlights" => o.nav_lights = false,
            "--screenshot" => o.screenshot = Some(value()?),
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once(['x', 'X']).ok_or(format!("bad size {v:?}"))?;
                let dim = |s: &str| {
                    s.parse::<u32>()
                        .ok()
                        .filter(|n| (16..=8192).contains(n))
                        .ok_or(format!("bad size {v:?}"))
                };
                o.size = (dim(w)?, dim(h)?);
            }
            "--angle" => o.angle = Some(num(value()?)?),
            "--declination" => o.declination = Some(num(value()?)?),
            "--distance" => o.distance = Some(num(value()?)?),
            "--time" => o.time = num(value()?)?,
            "--fov" => o.fov = Some(num(value()?)?),
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"))
            }
            p => positional.push(p.to_owned()),
        }
    }
    match positional.as_slice() {
        [big] => o.big = big.clone(),
        [big, ship] => {
            o.big = big.clone();
            o.ship = Some(ship.clone());
        }
        _ => return Err(USAGE.into()),
    }
    Ok(o)
}

fn setup(o: &Options) -> Result<Viewer, String> {
    let big = BigArchive::open(&o.big).map_err(|e| format!("{}: {e}", o.big))?;
    let mut v = Viewer::new(Box::new(big))?;
    if let Some(ship) = &o.ship {
        v.select_ship_by_name(ship)?;
    }
    if let Some(bg) = &o.background {
        v.select_background(bg)?;
    }
    if let Some(l) = &o.lighting {
        v.set_lighting(l)?;
    }
    if let Some(team) = o.team {
        if let Some(t) = team {
            if t >= v.team_colors.schemes.len() {
                return Err(format!(
                    "--team: only {} schemes",
                    v.team_colors.schemes.len()
                ));
            }
        }
        v.team = team;
    }
    v.nav_lights = o.nav_lights;
    v.ensure_model()?;
    if let Some(a) = o.angle {
        v.camera.angle = a.to_radians();
        v.camera.rotate_angle(0.0);
    }
    if let Some(d) = o.declination {
        v.camera.declination = d.to_radians();
        v.camera.rotate_declination(0.0);
    }
    if let Some(d) = o.distance {
        v.camera.distance = d;
    }
    if let Some(f) = o.fov {
        v.camera.tweaks.field_of_view = f;
    }
    Ok(v)
}

fn screenshot(mut v: Viewer, o: &Options, out: &str) -> Result<(), String> {
    let (device, queue) = headless_device()?;
    let mut renderer = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
    v.prepare(&renderer)?;
    let target = OffscreenTarget::new(&renderer, o.size.0, o.size.1);
    let instances = v.instances(Some(o.time));
    let rgba = target.render(&mut renderer, &v.scene(&instances))?;
    save_png(Path::new(out), o.size.0, o.size.1, &rgba)?;
    eprintln!("{}", v.title());
    eprintln!("wrote {out} ({}x{})", o.size.0, o.size.1);
    // Release GPU objects before the device, after the GPU has finished with them.
    drop(instances);
    drop(v);
    drop(target);
    drop(renderer);
    let _ = device.poll(hwc_render::wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    drop(queue);
    drop(device);
    eprintln!("teardown done");
    Ok(())
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let file = std::fs::File::create(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().map_err(|e| e.to_string())?;
    w.write_image_data(rgba).map_err(|e| e.to_string())?;
    w.finish().map_err(|e| e.to_string())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|o| {
        let v = setup(&o)?;
        match &o.screenshot {
            Some(out) => screenshot(v, &o, out),
            None => app::run(v),
        }
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn parses_arguments() {
        let o = parse_args(&args(&[
            "c.big",
            "smothership",
            "--background",
            "mission13",
            "--screenshot",
            "x.png",
            "--size",
            "640x480",
            "--team",
            "off",
            "--angle",
            "-30",
        ]))
        .unwrap();
        assert_eq!(o.big, "c.big");
        assert_eq!(o.ship.as_deref(), Some("smothership"));
        assert_eq!(o.background.as_deref(), Some("mission13"));
        assert_eq!(o.size, (640, 480));
        assert_eq!(o.team, Some(None));
        assert_eq!(o.angle, Some(-30.0));
        assert!(parse_args(&args(&["c.big", "--bogus"])).is_err());
        assert!(parse_args(&args(&[])).is_err());
    }
}
