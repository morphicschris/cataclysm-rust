//! Interactive ship viewer for Homeworld: Cataclysm data.
//!
//! ```text
//! hwc-viewer <cataclysm.big> [ship-name] [--background btg] [--lighting hsf]
//!            [--screenshot out.png [--size WxH] [--angle deg] [--declination deg]
//!             [--distance d] [--time s] [--team n|off]]
//! hwc-viewer <cataclysm.big> --sandbox [scenario] [--interpolate] [--screenshot out.png
//!            --ticks 0,80,240 [--select 1,2,3] [--plate x,y]] [--exit-after s]
//! ```

mod app;
mod overlay;
mod sandbox;
mod sandbox_app;
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

sandbox mode (--sandbox [fleet|idle|patrol|dogfight|frigates]):
  --interpolate           blend rendering between 16 Hz updates (default: show the latest)
  --ticks <n,n,..>        with --screenshot: save one image per universe tick count
  --select <i,i,..>       pre-select ships by creation index
  --plate <x,y>           show the move plate as if the cursor were at pixel x,y
  --exit-after <s>        close the window after s seconds
  --follow                camera follows the selection (or the whole fleet)
  sandbox controls: left click / drag select (shift adds), M then click moves the selection
  on the plane through its centre, A then click attacks a ship, F cycles formations, H halts,
  C focuses the camera,
  Space pauses (. steps), I toggles interpolation, right-drag rotates, wheel zooms

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
    sandbox: Option<String>,
    interpolate: bool,
    ticks: Vec<u32>,
    select: Vec<usize>,
    plate: Option<[f32; 2]>,
    exit_after: Option<f32>,
    follow: bool,
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
            "--sandbox" => {
                let next = it
                    .clone()
                    .next()
                    .filter(|n| !n.starts_with("--") && hwc_sim::harness::scenario(n).is_some());
                o.sandbox = Some(match next {
                    Some(n) => {
                        it.next();
                        n.clone()
                    }
                    None => "fleet".into(),
                });
            }
            "--interpolate" => o.interpolate = true,
            "--follow" => o.follow = true,
            "--ticks" => {
                let v = value()?;
                o.ticks = v
                    .split(',')
                    .map(|t| {
                        t.trim()
                            .parse()
                            .map_err(|_| format!("--ticks: bad value {t:?}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--select" => {
                let v = value()?;
                o.select = v
                    .split(',')
                    .map(|t| {
                        t.trim()
                            .parse()
                            .map_err(|_| format!("--select: bad value {t:?}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--plate" => {
                let v = value()?;
                let (x, y) = v
                    .split_once(',')
                    .ok_or(format!("--plate: bad value {v:?}"))?;
                o.plate = Some([num(x.to_owned())?, num(y.to_owned())?]);
            }
            "--exit-after" => o.exit_after = Some(num(value()?)?),
            flag if flag.starts_with("--") => {
                return Err(format!("unknown option {flag}\n\n{USAGE}"))
            }
            p => positional.push(p.to_owned()),
        }
    }
    match positional.as_slice() {
        [big] => o.big = big.clone(),
        [big, scenario] if o.sandbox.is_some() => {
            o.big = big.clone();
            o.sandbox = Some(scenario.clone());
        }
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

fn setup_sandbox(o: &Options) -> Result<sandbox::Sandbox, String> {
    let name = o.sandbox.as_deref().unwrap_or("fleet");
    let scenario = hwc_sim::harness::scenario(name).ok_or_else(|| {
        format!(
            "unknown scenario {name:?} (try {:?})",
            hwc_sim::harness::SCENARIOS
        )
    })?;
    let big = BigArchive::open(&o.big).map_err(|e| format!("{}: {e}", o.big))?;
    let mut sb = sandbox::Sandbox::new(big, scenario, o.interpolate)?;
    if let Some(bg) = &o.background {
        sb.set_background(Some(viewer::normalize_asset(bg, "btg", "btg")));
    }
    for &i in &o.select {
        let id = *sb.run.ids.get(i).ok_or(format!("--select: no ship {i}"))?;
        sb.selection.push(id);
    }
    if let Some(a) = o.angle {
        sb.camera.angle = a.to_radians();
        sb.camera.rotate_angle(0.0);
    }
    if let Some(d) = o.declination {
        sb.camera.declination = d.to_radians();
        sb.camera.rotate_declination(0.0);
    }
    if let Some(d) = o.distance {
        sb.camera.distance = d;
    }
    if o.follow {
        sb.focus = if sb.selection.is_empty() {
            sb.run.universe.ship_list.clone()
        } else {
            sb.selection.clone()
        };
        sb.update_focus();
    }
    if o.plate.is_some() && !sb.selection.is_empty() {
        sb.mode = sandbox::Mode::Move;
    }
    Ok(sb)
}

/// Offscreen frames of the sandbox at the requested tick counts.
fn sandbox_screenshots(mut sb: sandbox::Sandbox, o: &Options, out: &str) -> Result<(), String> {
    let (device, queue) = headless_device()?;
    let mut renderer = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
    sb.prepare(&renderer);
    let target = OffscreenTarget::new(&renderer, o.size.0, o.size.1);
    let mut ticks = if o.ticks.is_empty() {
        vec![0]
    } else {
        o.ticks.clone()
    };
    ticks.sort_unstable();
    let path = Path::new(out);
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sandbox")
        .to_owned();
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("png")
        .to_owned();
    for t in ticks {
        while sb.run.universe.update_counter < t {
            sb.step();
        }
        let instances = sb.instances();
        let mut rgba = target.render(&mut renderer, &sb.scene(&instances))?;
        drop(instances);
        sb.overlay(o.size, o.plate, None)
            .rasterize(&mut rgba, o.size.0, o.size.1);
        let file = if o.ticks.len() > 1 {
            path.with_file_name(format!("{stem}_t{t:04}.{ext}"))
        } else {
            path.to_path_buf()
        };
        save_png(&file, o.size.0, o.size.1, &rgba)?;
        eprintln!("{} -> {}", sb.title(), file.display());
    }
    drop(sb);
    drop(target);
    drop(renderer);
    let _ = device.poll(hwc_render::wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|o| {
        if o.sandbox.is_some() {
            let sb = setup_sandbox(&o)?;
            return match &o.screenshot {
                Some(out) => sandbox_screenshots(sb, &o, out),
                None => sandbox_app::run(sb, o.exit_after),
            };
        }
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
