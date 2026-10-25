//! Interactive ship viewer for Homeworld: Cataclysm data.
//!
//! ```text
//! hwc-viewer <cataclysm.big> [ship-name] [--background btg] [--lighting hsf]
//!            [--screenshot out.png [--size WxH] [--angle deg] [--declination deg]
//!             [--distance d] [--time s] [--team n|off]]
//! hwc-viewer <cataclysm.big> --sandbox [scenario] [--interpolate] [--screenshot out.png
//!            --ticks 0,80,240 [--select 1,2,3] [--plate x,y]] [--exit-after s] [--mute]
//! hwc-viewer <install or cataclysm.big> --mission 1 [--skill n] [--mute] [--screenshot out.png
//!            (--ticks 0,300 | --shots arrival,launch,combat,objective)] [--exit-after s]
//! ```

mod app;
mod audio;
mod data;
mod mission;
mod overlay;
mod sandbox;
mod sandbox_app;
mod sim;
mod text;
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

sandbox mode (--sandbox [fleet|idle|patrol|dogfight|frigates|base]):
  --interpolate           blend rendering between 16 Hz updates (default: show the latest)
  --ticks <n,n,..>        with --screenshot: save one image per universe tick count
  --select <i,i,..>       pre-select ships by creation index
  --plate <x,y>           show the move plate as if the cursor were at pixel x,y
  --exit-after <s>        close the window after s seconds
  --follow                camera follows the selection (or the whole fleet)
  sandbox controls: left click / drag select (shift adds), M then click moves the selection
  on the plane through its centre, A then click attacks a ship, F cycles formations, H halts,
  C focuses the camera, B queues a ship at the mothership, L launches the ships inside it,
  U toggles its auto-launch, R starts research, G sends the selection harvesting,
  Space pauses (. steps), I toggles interpolation, right-drag rotates, wheel zooms
  --mute                  no sound (sound events, music and speech are played otherwise)
  --soft-limit            soft knee instead of the original's hard 16-bit clamp on loud mixes

mission mode (--mission <n>; the data is a game install, imported data or the cataclysm.big
of an install, because the mission script comes from cataclysm.exe):
  the mission runs live with the sandbox controls for the player's ships; objectives,
  subtitles and skipped NIS sequences are shown as overlay text
  --skill <0-2>           difficulty (default 2)
  --carry <file>          the fleet from the previous mission (written by --save-carry or the
                          hwc-sim campaign example); without it missions after 1 start with
                          the warp.script fleet
  --save-carry <file>     write the carry-over once the fleet has jumped (J = hyperspace)
  --autoplay              scripted test orders play the mission and press hyperspace
    --budget <s>          seconds after the fleet arrives before autoplay forces the jump
  --shots <names>         with --screenshot: images when these first happen: arrival,
                          launch, combat, subtitle, objective, asteroids (within --ticks max,
                          default 4000)

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
    mission: Option<u32>,
    skill: i32,
    /// `--carry FILE`: the fleet carried over from the previous mission.
    carry: Option<String>,
    /// `--save-carry FILE`: write the carry-over when the mission's fleet has jumped.
    save_carry: Option<String>,
    /// `--autoplay`: the scripted test orders play the mission (and press hyperspace).
    autoplay: bool,
    /// `--budget S`: seconds after the arrival before autoplay forces the jump.
    budget: Option<f32>,
    mute: bool,
    soft_limit: bool,
    shots: Vec<String>,
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut o = Options {
        nav_lights: true,
        size: (1280, 960),
        time: 0.75,
        skill: 2,
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
            "--mission" => {
                let v = value()?;
                o.mission = Some(v.parse().map_err(|_| format!("--mission: bad value {v:?}"))?);
            }
            "--skill" => {
                let v = value()?;
                o.skill = v.parse().map_err(|_| format!("--skill: bad value {v:?}"))?;
            }
            "--mute" => o.mute = true,
            "--carry" => o.carry = Some(value()?),
            "--save-carry" => o.save_carry = Some(value()?),
            "--autoplay" => o.autoplay = true,
            "--budget" => o.budget = Some(num(value()?)?),
            "--soft-limit" => o.soft_limit = true,
            "--shots" => {
                o.shots = value()?
                    .split(',')
                    .map(|s| s.trim().to_ascii_lowercase())
                    .collect()
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
        [big, scenario] if o.sandbox.is_some() && o.mission.is_none() => {
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
    let assets = data::GameAssets::open(Path::new(&o.big))?;
    let audio_data = assets.reopen();
    let mut sb = match o.mission {
        Some(n) => {
            let run = mission::start(&assets, n, o.skill, o.carry.as_deref())?;
            if o.screenshot.is_none() {
                println!("{}", sandbox::MISSION_KEY_HINT);
                println!("(B and R are quick keys; the build and research managers are in the game, cataclysm.exe)");
            }
            let mut sb = sandbox::Sandbox::mission(assets, run, o.interpolate)?;
            if o.autoplay {
                sb.autoplay = Some(hwc_sim::autoplay::Autoplay::new((o.budget.unwrap_or(900.0) * 16.0) as u32));
            }
            sb.save_carry = o.save_carry.clone();
            sb
        }
        None => {
            let name = o.sandbox.as_deref().unwrap_or("fleet");
            let scenario = hwc_sim::harness::scenario(name).ok_or_else(|| {
                format!(
                    "unknown scenario {name:?} (try {:?})",
                    hwc_sim::harness::SCENARIOS
                )
            })?;
            sandbox::Sandbox::new(assets, scenario, o.interpolate)?
        }
    };
    // Offscreen captures never play sound, but the engine still gives speech its subtitles.
    let mute = o.mute || o.screenshot.is_some();
    match audio_data.and_then(|gd| audio::Audio::new(gd, mute, o.soft_limit)) {
        Ok(a) => sb.audio = Some(a),
        Err(e) => eprintln!("warning: no sound: {e}"),
    }
    if let Some(bg) = &o.background {
        sb.set_background(Some(viewer::normalize_asset(bg, "btg", "btg")));
    }
    let ids = sb.sim.ids();
    for &i in &o.select {
        let id = *ids.get(i).ok_or(format!("--select: no ship {i}"))?;
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
            sb.universe().ship_list.clone()
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

/// Offscreen GPU context for sandbox and mission captures.
struct Capture {
    target: OffscreenTarget,
    renderer: Renderer,
    queue: hwc_render::wgpu::Queue,
    device: hwc_render::wgpu::Device,
    size: (u32, u32),
}

impl Capture {
    fn new(size: (u32, u32)) -> Result<Self, String> {
        let (device, queue) = headless_device()?;
        let renderer = Renderer::new(&device, &queue, OFFSCREEN_FORMAT);
        let target = OffscreenTarget::new(&renderer, size.0, size.1);
        Ok(Self {
            target,
            renderer,
            queue,
            device,
            size,
        })
    }

    fn shoot(
        &mut self,
        sb: &mut sandbox::Sandbox,
        plate: Option<[f32; 2]>,
        file: &Path,
    ) -> Result<(), String> {
        sb.prepare(&mut self.renderer);
        let frame = sb.frame(self.size);
        let mut rgba = sb.render_offscreen(&mut self.renderer, &self.target, &frame)?;
        sb.overlay(self.size, plate, None)
            .rasterize(&mut rgba, self.size.0, self.size.1);
        save_png(file, self.size.0, self.size.1, &rgba)?;
        eprintln!("{} -> {}", sb.title(), file.display());
        Ok(())
    }

    /// Releases GPU objects before the device, after the GPU has finished with them.
    fn finish(self) {
        let Self {
            target,
            renderer,
            queue,
            device,
            ..
        } = self;
        drop(target);
        drop(renderer);
        let _ = device.poll(hwc_render::wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        drop(queue);
        drop(device);
    }
}

fn shot_path(out: &str, tag: &str, many: bool) -> std::path::PathBuf {
    let path = Path::new(out);
    if !many {
        return path.to_path_buf();
    }
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("shot");
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("png");
    path.with_file_name(format!("{stem}_{tag}.{ext}"))
}

/// Offscreen frames at the requested tick counts.
fn sandbox_screenshots(mut sb: sandbox::Sandbox, o: &Options, out: &str) -> Result<(), String> {
    let mut cap = Capture::new(o.size)?;
    let mut ticks = if o.ticks.is_empty() {
        vec![0]
    } else {
        o.ticks.clone()
    };
    ticks.sort_unstable();
    for t in ticks {
        while sb.universe().update_counter < t {
            sb.step();
        }
        let file = shot_path(out, &format!("t{t:04}"), o.ticks.len() > 1);
        cap.shoot(&mut sb, o.plate, &file)?;
    }
    drop(sb);
    cap.finish();
    Ok(())
}

/// The points of the mothership's hyperspace arrival the `kuunlan` shots capture: (tag,
/// `hsState`, `clipt` at or below which it is taken).
const KUUNLAN_STAGES: [(&str, u32, f32); 7] = [
    ("1_popup", hwc_sim::mission::hs_effect::POPUP_OUTOF, 0.5),
    ("2_slice_start", hwc_sim::mission::hs_effect::SLICING_OUTOF, 0.8),
    ("3_slice_front", hwc_sim::mission::hs_effect::SLICING_OUTOF, 0.3),
    ("4_slice_mid", hwc_sim::mission::hs_effect::SLICING_OUTOF, -0.3),
    ("5_slice_end", hwc_sim::mission::hs_effect::SLICING_OUTOF, -0.85),
    ("6_collapse", hwc_sim::mission::hs_effect::COLLAPSE_OUTOF, 0.5),
    ("7_collapse_end", hwc_sim::mission::hs_effect::COLLAPSE_OUTOF, 0.1),
];

/// The next `kuunlan` stage the mothership has reached, if not taken yet.
fn kuunlan_stage(run: &hwc_sim::mission::MissionRun, done: &mut Vec<&'static str>) -> Option<String> {
    let a = run.game.arriving.iter().find(|a| a.mothership)?;
    for (tag, state, clipt) in KUUNLAN_STAGES {
        if done.contains(&tag) {
            continue;
        }
        if a.hs_state == state && a.clipt <= clipt {
            done.push(tag);
            return Some(format!("kuunlan_{tag}"));
        }
        return None;
    }
    None
}

/// Mission captures when things first happen: the fleet arriving from hyperspace (the
/// mothership mid-slice), the docked ships launching, the first hit anywhere (the camera moves
/// there), the first objective.
fn mission_shots(mut sb: sandbox::Sandbox, o: &Options, out: &str) -> Result<(), String> {
    use hwc_sim::mission::hs_effect;
    let mut cap = Capture::new(o.size)?;
    let max = o.ticks.iter().copied().max().unwrap_or(4000);
    let mut wanted: Vec<String> = o.shots.clone();
    let mut launched_at = None;
    let mut combat_at: Option<(u32, glam::Vec3)> = None;
    let mut kuunlan_done: Vec<&'static str> = Vec::new();
    while !wanted.is_empty() && sb.universe().update_counter < max {
        sb.step();
        let tick = sb.universe().update_counter;
        let Some(run) = sb.sim.mission() else { break };
        let want = |w: &str| wanted.iter().any(|x| x == w);
        let mut take = None;
        if want("arrival")
            && run.game.arriving.iter().any(|a| {
                a.mothership && a.hs_state == hs_effect::SLICING_OUTOF && a.clipt < 0.0
            })
        {
            take = Some("arrival");
        }
        if want("launch") {
            if launched_at.is_none() && run.game.ships_have_arrived {
                launched_at = Some(tick);
            }
            if take.is_none() && launched_at.is_some_and(|t| tick >= t + 24) {
                take = Some("launch");
            }
        }
        if take.is_none() && want("subtitle") && sb.has_subtitle() {
            take = Some("subtitle");
        }
        if take.is_none() && want("objective") && !run.game.objectives.is_empty() {
            take = Some("objective");
        }
        // The fleet leaving at the end of the mission: the mothership in its window.
        if take.is_none()
            && want("jump")
            && run.game.hyper.ships.iter().any(|(id, r)| {
                Some(*id) == run.universe.base.players[0].mothership
                    && r.hs_state == hs_effect::SLICING_INTO
                    && r.clipt < 0.3
            })
        {
            take = Some("jump");
        }
        // A side view of the mothership, square to its heading, so the window shows edge-on to
        // the camera as it opens, slices and collapses.
        let kuunlan = if want("kuunlan") {
            kuunlan_stage(run, &mut kuunlan_done).and_then(|tag| {
                let u = sb.universe();
                let m = run.game.arriving.iter().find(|a| a.mothership).and_then(|a| a.ship).and_then(|id| u.ship(id))?;
                let p = m.posinfo.position;
                let h = m.rotinfo.coordsys.col(2);
                Some((tag, glam::Vec3::new(p.x, p.y, p.z), h.y.atan2(h.x)))
            })
        } else {
            None
        };
        // The mission's nearest asteroid to the mothership, with the field around it.
        let asteroid_at = (want("asteroids") && run.game.ships_have_arrived)
            .then(|| {
                let u = sb.universe();
                let m = run.game.level.motherships.first().copied().flatten().and_then(|id| u.ship(id))?;
                let mp = m.posinfo.position;
                let d = |r: &hwc_sim::resource::Resource| (r.position - mp).mag_sq();
                let r = u.base.resources.list.iter().min_by(|a, b| d(a).total_cmp(&d(b)))?;
                Some(glam::Vec3::new(r.position.x, r.position.y, r.position.z))
            })
            .flatten();
        if want("combat") {
            if combat_at.is_none() {
                combat_at = sb.universe().events.iter().find_map(|e| match e {
                    hwc_sim::universe::SimEvent::BulletHit { position, .. } => {
                        Some((tick, glam::Vec3::new(position.x, position.y, position.z)))
                    }
                    _ => None,
                });
            }
            if let Some((t, p)) = combat_at {
                if tick >= t + 12 {
                    let saved = (sb.camera.look_at, sb.camera.distance, sb.focus.clone());
                    sb.focus.clear();
                    sb.camera.look_at = p;
                    sb.camera.distance = 2500.0;
                    cap.shoot(&mut sb, None, &shot_path(out, "combat", true))?;
                    (sb.camera.look_at, sb.camera.distance, sb.focus) = saved;
                    wanted.retain(|w| w != "combat");
                }
            }
        }
        if let Some(tag) = take {
            cap.shoot(&mut sb, None, &shot_path(out, tag, true))?;
            wanted.retain(|w| w != tag);
        }
        if let Some((tag, p, heading)) = kuunlan {
            // From the side and from ahead of the ship, a little off its axis.
            let saved = (sb.camera.look_at, sb.camera.distance, sb.camera.angle, sb.camera.declination, sb.focus.clone());
            sb.focus.clear();
            sb.camera.look_at = p;
            sb.camera.declination = -0.25;
            for (view, offset, distance) in [("side", 1.15, 9000.0), ("front", 0.4, 7000.0)] {
                sb.camera.distance = distance;
                sb.camera.angle = heading + offset;
                cap.shoot(&mut sb, None, &shot_path(out, &format!("{tag}_{view}"), true))?;
            }
            (sb.camera.look_at, sb.camera.distance, sb.camera.angle, sb.camera.declination, sb.focus) = saved;
        }
        if kuunlan_done.len() >= KUUNLAN_STAGES.len() {
            wanted.retain(|w| w != "kuunlan");
        }
        if let Some(p) = asteroid_at {
            let saved = (sb.camera.look_at, sb.camera.distance, sb.focus.clone());
            sb.focus.clear();
            sb.camera.look_at = p;
            sb.camera.distance = 12000.0;
            cap.shoot(&mut sb, None, &shot_path(out, "asteroids", true))?;
            (sb.camera.look_at, sb.camera.distance, sb.focus) = saved;
            wanted.retain(|w| w != "asteroids");
        }
    }
    if !wanted.is_empty() {
        eprintln!("not reached within {max} updates: {wanted:?}");
    }
    drop(sb);
    cap.finish();
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = parse_args(&args).and_then(|o| {
        if o.sandbox.is_some() || o.mission.is_some() {
            let sb = setup_sandbox(&o)?;
            return match &o.screenshot {
                Some(out) if !o.shots.is_empty() => mission_shots(sb, &o, out),
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
