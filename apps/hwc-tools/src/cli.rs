//! Argument handling for the `render-*` commands.

use std::path::Path;

use hwc_formats::big::BigArchive;
use hwc_formats::fib::{Fib, Screen};

use crate::render::background::{Background, SkyView};
use crate::render::canvas::Canvas;
use crate::render::screen::{self, ScreenRenderer};
use crate::render::ship::{ShipModel, ShipView, DEFAULT_TEAM};

type CmdResult = Result<(), Box<dyn std::error::Error>>;

/// Ships are rendered at this multiple of the output size and box-filtered down.
const SUPERSAMPLE: usize = 3;

/// Parsed `--flag value` options plus the remaining positional arguments.
#[derive(Debug)]
struct Args<'a> {
    positional: Vec<&'a str>,
    team: [[u8; 3]; 2],
    yaw: Option<f32>,
    pitch: Option<f32>,
    sky_yaw: f32,
    sky_pitch: f32,
    size: Option<(usize, usize)>,
}

impl<'a> Args<'a> {
    fn parse(args: &[&'a str]) -> Result<Self, String> {
        let mut out = Args {
            positional: Vec::new(),
            team: DEFAULT_TEAM,
            yaw: None,
            pitch: None,
            sky_yaw: 0.0,
            sky_pitch: 0.0,
            size: None,
        };
        let mut iter = args.iter().copied();
        while let Some(arg) = iter.next() {
            let mut value = || iter.next().ok_or(format!("{arg} needs a value"));
            match arg {
                "--team" => {
                    let base = parse_rgb(value()?)?;
                    let stripe = parse_rgb(value()?)?;
                    out.team = [base, stripe];
                }
                "--yaw" => out.yaw = Some(parse_num(arg, value()?)?),
                "--pitch" => out.pitch = Some(parse_num(arg, value()?)?),
                "--sky-yaw" => out.sky_yaw = parse_num(arg, value()?)?,
                "--sky-pitch" => out.sky_pitch = parse_num(arg, value()?)?,
                "--size" => out.size = Some(parse_size(value()?)?),
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                positional => out.positional.push(positional),
            }
        }
        Ok(out)
    }

    fn expect_positional(&self, n: usize, cmd: &str) -> Result<(), String> {
        if self.positional.len() == n {
            Ok(())
        } else {
            Err(format!(
                "{cmd} takes {n} positional arguments, got {} (see --help)",
                self.positional.len()
            ))
        }
    }
}

fn parse_num(flag: &str, s: &str) -> Result<f32, String> {
    s.parse()
        .map_err(|_| format!("{flag}: {s:?} is not a number"))
}

fn parse_rgb(s: &str) -> Result<[u8; 3], String> {
    let parts: Vec<u8> = s
        .split(',')
        .map(|p| p.trim().parse::<u8>())
        .collect::<Result<_, _>>()
        .map_err(|_| format!("bad colour {s:?}, expected R,G,B"))?;
    parts
        .try_into()
        .map_err(|_| format!("bad colour {s:?}, expected R,G,B"))
}

fn parse_size(s: &str) -> Result<(usize, usize), String> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .ok_or_else(|| format!("bad size {s:?}, expected WxH"))?;
    let dim = |v: &str| {
        v.parse::<usize>()
            .ok()
            .filter(|&n| (16..=8192).contains(&n))
            .ok_or_else(|| format!("bad size {s:?}"))
    };
    Ok((dim(w)?, dim(h)?))
}

pub fn run_render(cmd: &str, rest: &[&str]) -> CmdResult {
    let args = Args::parse(rest)?;
    match cmd {
        "render-ship" => {
            args.expect_positional(3, cmd)?;
            let [big, mesh, out] = [0, 1, 2].map(|i| args.positional[i]);
            let big = BigArchive::open(big)?;
            let (w, h) = args.size.unwrap_or((1280, 960));
            let ship = ship_layer(&big, mesh, &args, w, h, ShipView::default())?;
            let mut canvas = Canvas::new(w, h, [0.0, 0.0, 0.0, 1.0]);
            canvas.composite_over(&ship);
            save(&canvas, out)
        }
        "render-background" => {
            args.expect_positional(3, cmd)?;
            let [big, btg, out] = [0, 1, 2].map(|i| args.positional[i]);
            let big = BigArchive::open(big)?;
            let (w, h) = args.size.unwrap_or((1600, 900));
            let sky = Background::load(&big, btg)?;
            let mut canvas = Canvas::new(w, h, [0.0, 0.0, 0.0, 1.0]);
            let view = SkyView {
                yaw: args.yaw.unwrap_or(0.0),
                pitch: args.pitch.unwrap_or(0.0),
            };
            sky.render(&mut canvas, view);
            eprintln!(
                "{btg}: {} vertices, {} triangles, {} stars",
                sky.btg.vertices.len(),
                sky.btg.polygons.len(),
                sky.btg.stars.len()
            );
            save(&canvas, out)
        }
        "render-screen" => {
            args.expect_positional(4, cmd)?;
            let [big, fib_path, name, out] = [0, 1, 2, 3].map(|i| args.positional[i]);
            let big = BigArchive::open(big)?;
            let fib = load_fib(&big, fib_path)?;
            let screen = find_screen(&fib, name)?;
            let mut canvas = Canvas::new(screen::WIDTH, screen::HEIGHT, [0.0, 0.0, 0.0, 1.0]);
            ScreenRenderer::new(&big).render(screen, &mut canvas);
            save(&canvas, out)
        }
        "screen-info" => {
            args.expect_positional(3, cmd)?;
            let [big, fib_path, name] = [0, 1, 2].map(|i| args.positional[i]);
            let fib = load_fib(&BigArchive::open(big)?, fib_path)?;
            for (i, a) in find_screen(&fib, name)?.atoms.iter().enumerate() {
                let r = a.rect;
                println!(
                    "{i:3} {:?} flags {:#07x} ({},{})-({},{}) border {:?} content {:?} name {:?} text {:?} font {:?} bitmap {:?}",
                    a.kind, a.flags, r.left, r.top, r.right, r.bottom, a.border_color,
                    a.content_color, a.name, a.text(), a.font(), a.bitmap
                );
            }
            Ok(())
        }
        "render-hero" => {
            args.expect_positional(4, cmd)?;
            let [big, mesh, btg, out] = [0, 1, 2, 3].map(|i| args.positional[i]);
            let big = BigArchive::open(big)?;
            let (w, h) = args.size.unwrap_or((1600, 900));
            let mut canvas = Canvas::new(w, h, [0.0, 0.0, 0.0, 1.0]);
            let sky = Background::load(&big, btg)?;
            let sky_view = SkyView {
                yaw: args.sky_yaw,
                pitch: args.sky_pitch,
            };
            sky.render(&mut canvas, sky_view);
            let view = ShipView {
                fill: 0.8,
                offset: [0.08, 0.04],
                ..ShipView::default()
            };
            let ship = ship_layer(&big, mesh, &args, w, h, view)?;
            canvas.composite_over(&ship);
            save(&canvas, out)
        }
        "geo-info" => {
            args.expect_positional(2, cmd)?;
            let big = BigArchive::open(args.positional[0])?;
            let mesh = args.positional[1];
            let data = big
                .read_file(mesh)?
                .ok_or_else(|| format!("{mesh}: not in archive"))?;
            print_geo_info(&hwc_formats::geo::Geo::parse(&data)?);
            Ok(())
        }
        _ => unreachable!("dispatch only routes render commands here"),
    }
}

fn load_fib(big: &BigArchive, path: &str) -> Result<Fib, Box<dyn std::error::Error>> {
    let data = big
        .read_file(path)?
        .ok_or_else(|| format!("{path}: not in archive"))?;
    Ok(Fib::parse(&data)?)
}

fn find_screen<'f>(fib: &'f Fib, name: &str) -> Result<&'f Screen, String> {
    fib.screen(name).ok_or_else(|| {
        let names: Vec<&str> = fib.screens.iter().map(|s| s.name.as_str()).collect();
        format!("no screen {name:?}; screens: {}", names.join(", "))
    })
}

/// Renders a ship supersampled onto a transparent layer of the requested size.
fn ship_layer(
    big: &BigArchive,
    mesh: &str,
    args: &Args<'_>,
    w: usize,
    h: usize,
    mut view: ShipView,
) -> Result<Canvas, Box<dyn std::error::Error>> {
    let paths: Vec<&str> = mesh.split('+').collect();
    let model = ShipModel::load(big, &paths, args.team)?;
    view.yaw = args.yaw.unwrap_or(view.yaw);
    view.pitch = args.pitch.unwrap_or(view.pitch);
    let mut layer = Canvas::new(w * SUPERSAMPLE, h * SUPERSAMPLE, [0.0; 4]);
    model.render(&mut layer, &view);
    for part in &model.parts {
        eprintln!(
            "{}: {} objects, {} triangles, {} materials",
            part.path,
            part.geo.objects.len(),
            part.triangle_count(),
            part.geo.materials.len()
        );
    }
    Ok(layer.downsample(SUPERSAMPLE))
}

fn print_geo_info(geo: &hwc_formats::geo::Geo) {
    println!(
        "name {:?}, paged {}, {} materials",
        geo.name,
        geo.paged,
        geo.materials.len()
    );
    for (i, m) in geo.materials.iter().enumerate() {
        println!(
            "  material {i}: {:?} texture {:?} flags {:#x} ambient {:?} diffuse {:?}",
            m.name, m.texture, m.flags, m.ambient, m.diffuse
        );
    }
    for (i, o) in geo.objects.iter().enumerate() {
        println!(
            "  object {i} {:?}: flags {:#x}, {} vertices, {} polygons, parent {:?} child {:?} sibling {:?}",
            o.name,
            o.flags,
            o.vertices.len(),
            o.polygons.len(),
            o.parent,
            o.first_child,
            o.next_sibling
        );
        println!("    matrix {:?}", o.local_matrix);
    }
}

fn save(canvas: &Canvas, out: &str) -> CmdResult {
    canvas.save_png(Path::new(out))?;
    eprintln!("wrote {out} ({}x{})", canvas.width, canvas.height);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_options() {
        let a = Args::parse(&[
            "a", "--team", "1,2,3", "4,5,6", "--yaw", "-30", "b", "--size", "64x32",
        ])
        .unwrap();
        assert_eq!(a.positional, ["a", "b"]);
        assert_eq!(a.team, [[1, 2, 3], [4, 5, 6]]);
        assert_eq!(a.yaw, Some(-30.0));
        assert_eq!(a.size, Some((64, 32)));
        assert!(Args::parse(&["--bogus"]).is_err());
        assert!(Args::parse(&["--team", "1,2"]).is_err());
    }
}
