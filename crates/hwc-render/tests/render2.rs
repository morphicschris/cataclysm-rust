//! Offscreen renders of the second renderer stage: engine trails and glows behind moving ships,
//! level-of-detail selection, `.mad` animation mid-play and special render states. Images go to
//! `screenshots/render2/`. Needs a GPU and game data (`HWC_EXTRACTED_DIR` or `HWC_GAME_DIR`);
//! skips otherwise.

use std::path::{Path, PathBuf};

use glam::{Mat4, Vec3};
use hwc_render::assets::{load_mad_clip, read_script};
use hwc_render::effects::EffectBatch;
use hwc_render::lod::{screen_radius, LodSelector};
use hwc_render::render_state::{
    cloak_wipe_plane, normalized_team_color, phase_blend_state, CollisionBox, PhaseState, ShipTint,
};
use hwc_render::trails::{
    ShipTrailDef, ShipTrails, TeamTrailColors, TrailFrame, TrailGate, TrailTick, TrailTweaks,
};
use hwc_render::{
    headless_device, mesh_to_world, AssetSource, CameraTweaks, DirSource, GpuShip, OffscreenTarget,
    OrbitCamera, Renderer, Scene, SceneExtras, SceneLighting, ShipAnimator, ShipDef, ShipInstance,
    ShipModel, ShipRenderState, TeamColors, TeamPaint, OFFSCREEN_FORMAT,
};

const TILE: (u32, u32) = (480, 360);
/// Simulation rate of the universe update.
const TICK: f32 = 1.0 / 16.0;

struct Ctx {
    src: Box<dyn AssetSource>,
    renderer: Renderer,
    colors: TeamColors,
    lighting: SceneLighting,
}

fn setup() -> Option<Ctx> {
    let src: Box<dyn AssetSource> = if let Some(dir) = std::env::var_os("HWC_EXTRACTED_DIR") {
        Box::new(DirSource::new(dir))
    } else {
        let game = hwc_formats::game_dir()?;
        Box::new(hwc_formats::big::BigArchive::open(game.join("cataclysm.big")).ok()?)
    };
    let (device, queue) = match headless_device() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let colors = read_script(src.as_ref(), "teamcolors.script")
        .map(|s| TeamColors::from_script(&s))
        .unwrap_or_default();
    Some(Ctx {
        renderer: Renderer::new(&device, &queue, OFFSCREEN_FORMAT),
        src,
        colors,
        lighting: SceneLighting::default(),
    })
}

fn out_dir() -> PathBuf {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/render2");
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn save_png(path: &Path, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(rgba).unwrap();
}

/// Lays tiles out left to right.
fn montage(tiles: &[Vec<u8>]) -> (u32, u32, Vec<u8>) {
    let (tw, th) = TILE;
    let w = tw * tiles.len() as u32;
    let mut out = vec![0u8; (w * th * 4) as usize];
    for (i, t) in tiles.iter().enumerate() {
        for y in 0..th as usize {
            let src = &t[y * tw as usize * 4..(y + 1) * tw as usize * 4];
            let at = (y * w as usize + i * tw as usize) * 4;
            out[at..at + src.len()].copy_from_slice(src);
        }
    }
    (w, th, out)
}

fn ship(ctx: &Ctx, race: &str, name: &str) -> (ShipDef, Vec<ShipModel>, GpuShip) {
    let def = ShipDef::load(ctx.src.as_ref(), race, name).unwrap();
    let (levels, table) = ShipModel::load_lods(ctx.src.as_ref(), &def).unwrap();
    let paint = TeamPaint::Scheme(&ctx.colors, ctx.colors.schemes[0]);
    let gpu = ctx.renderer.upload_ship_lods(&levels, table, paint);
    (def, levels, gpu)
}

fn camera(look_at: Vec3, distance: f32, angle_deg: f32, declination_deg: f32) -> OrbitCamera {
    let mut c = OrbitCamera::new(CameraTweaks::default(), distance);
    c.ignore_zoom = true;
    c.look_at = look_at;
    c.angle = angle_deg.to_radians();
    c.declination = declination_deg.to_radians();
    c
}

fn render(
    ctx: &mut Ctx,
    cam: &OrbitCamera,
    ships: &[ShipInstance<'_>],
    extras: &SceneExtras<'_>,
) -> Vec<u8> {
    let target = OffscreenTarget::new(&ctx.renderer, TILE.0, TILE.1);
    let scene = Scene {
        camera: cam,
        lighting: &ctx.lighting,
        background: None,
        ships,
        nav_lights: true,
    };
    target.render_ex(&mut ctx.renderer, &scene, extras).unwrap()
}

fn lit_pixels(rgba: &[u8]) -> usize {
    rgba.chunks(4)
        .filter(|p| p[..3].iter().any(|&c| c > 8))
        .count()
}

/// A ship flying an arc: mesh-to-world matrix and velocity after `t` seconds.
fn flight(t: f32, speed: f32, turn_rate: f32) -> (Mat4, Vec3) {
    let yaw = turn_rate * t;
    // Heading is mesh +Z, which mesh_to_world turns into world -X; integrate the arc exactly.
    let heading = |yaw: f32| Vec3::new(-yaw.cos(), -yaw.sin(), 0.0);
    let pos = if turn_rate.abs() < 1e-6 {
        heading(0.0) * speed * t
    } else {
        Vec3::new(-yaw.sin(), yaw.cos() - 1.0, 0.0) * (speed / turn_rate)
    };
    let transform = Mat4::from_translation(pos) * Mat4::from_rotation_z(yaw) * mesh_to_world();
    (transform, heading(yaw) * speed)
}

#[test]
fn trails_and_glows_behind_moving_ships() {
    let Some(mut ctx) = setup() else { return };
    let assets = ctx.renderer.load_engine_effects(ctx.src.as_ref());
    assert_eq!(assets.glow_meshes, 21, "engine glow meshes");
    let tweaks = read_script(ctx.src.as_ref(), "trail.script")
        .map(|s| TrailTweaks::from_script(&s))
        .unwrap_or_default();
    let team_trails = read_script(ctx.src.as_ref(), "teamcolors.script")
        .map(|s| TeamTrailColors::from_script(&s))
        .unwrap_or_default();
    let team_color = normalized_team_color(ctx.colors.schemes[0].base);

    let mut tiles = Vec::new();
    for (race, name, speed, turn, dist, ang, decl) in [
        (
            "race_sect",
            "sacolyte",
            825.0 * 0.9,
            0.5f32,
            700.0,
            -70.0,
            -35.0,
        ),
        ("race_sect", "sworker", 425.0, -0.3, 600.0, -100.0, -30.0),
        ("race_sect", "sdestroyer", 325.0, 0.0, 1400.0, -35.0, -20.0),
    ] {
        let (def, _levels, gpu) = ship(&ctx, race, name);
        let mut tdef = ShipTrailDef::load(ctx.src.as_ref(), &def);
        if let Some(r) = &mut tdef.ramp {
            r.recolorize_all(&team_trails);
        }
        assert!(tdef.trail_count() > 0, "{name} has engines");
        let mut trails = ShipTrails::new(&tdef, 12345);
        let ticks = 64;
        let t_end = ticks as f32 * TICK;
        let (transform, velocity) = flight(t_end, speed, turn);
        let pos = transform.w_axis.truncate();
        // Look from the lit side, across the flight path, centred a little behind the ship.
        let yaw_deg = (turn * t_end).to_degrees();
        let cam = camera(
            pos - velocity.normalize() * dist * 0.3,
            dist,
            yaw_deg + ang,
            decl,
        );
        let eye_space_z = cam.view().transform_point3(pos).z;
        let mut effects = EffectBatch::default();
        // One update and one drawn frame per tick, as the game interleaves them.
        for k in 0..=ticks {
            let (transform, velocity) = flight(k as f32 * TICK, speed, turn);
            trails.update(
                &tdef,
                &tweaks,
                &TrailTick {
                    transform,
                    objects: None,
                },
            );
            let frame = TrailFrame {
                transform,
                objects: None,
                velocity,
                max_velocity: speed / 0.9,
                gate: TrailGate {
                    moving: true,
                    ..Default::default()
                },
                lod: 0,
                team: 0,
                team_color,
                eye: cam.eye(),
                eye_space_z,
                scale: 1.0,
                fade: 1.0,
                glow_texture: assets.glow_texture,
            };
            effects.clear();
            trails.draw(&tdef, &tweaks, &frame, &mut effects);
        }
        if tdef.ramp.is_some() {
            assert!(!effects.triangles.is_empty(), "{name}: ribbon geometry");
            assert!(!effects.sprites.is_empty(), "{name}: nozzle glow");
        } else {
            assert_eq!(
                effects.glow_meshes.len(),
                tdef.trail_count(),
                "{name}: glow meshes"
            );
        }
        let states = [ShipRenderState {
            tint: ShipTint::default().params(team_color, trails.engine_glow_levels(&tweaks)),
            ..Default::default()
        }];
        let instances = [ShipInstance {
            ship: &gpu,
            transform,
            age: 1.0,
        }];
        let extras = SceneExtras {
            ship_states: &states,
            effects: Some(&effects),
        };
        let with = render(&mut ctx, &cam, &instances, &extras);
        let without = render(&mut ctx, &cam, &instances, &SceneExtras::default());
        let diff = with
            .chunks(4)
            .zip(without.chunks(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 12))
            .count();
        assert!(diff > 100, "{name}: trails changed only {diff} pixels");
        tiles.push(with);
    }
    let (w, h, img) = montage(&tiles);
    save_png(&out_dir().join("trails.png"), w, h, &img);
}

#[test]
fn lod_levels_and_switching() {
    let Some(mut ctx) = setup() else { return };
    let (_def, levels, gpu) = ship(&ctx, "race_sect", "sacolyte");
    assert!(
        gpu.level_count() >= 4,
        "acolyte has {} levels",
        gpu.level_count()
    );
    let tris: Vec<usize> = (0..gpu.level_count())
        .map(|l| gpu.level_triangle_count(l))
        .collect();
    assert!(tris.windows(2).all(|w| w[0] >= w[1]), "{tris:?}");
    assert!(tris[0] > tris[tris.len() - 1]);

    // Screen-size selection as the camera pulls back.
    let (centre, radius) = levels[0].bounds();
    let selector = LodSelector::default();
    let mut picked = Vec::new();
    let mut current = 0;
    for dist in [100.0f32, 300.0, 800.0, 2000.0, 5000.0, 12000.0] {
        let cam = camera(Vec3::ZERO, dist, 30.0, -20.0);
        let proj = cam.projection(TILE.0 as f32 / TILE.1 as f32);
        let r = screen_radius(
            cam.view(),
            proj,
            mesh_to_world().transform_point3(centre),
            radius,
        );
        let d2 = cam.eye().length_squared();
        current = selector.select(gpu.level_count(), current, r, d2, gpu.lod_table());
        picked.push(current);
    }
    eprintln!("acolyte LOD by distance: {picked:?}, triangles per level {tris:?}");
    assert!(picked.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(picked[0], 0);
    assert!(*picked.last().unwrap() >= 3);

    // Each level at the same close-up framing.
    let mut tiles = Vec::new();
    let cam = camera(
        mesh_to_world().transform_point3(centre),
        radius * 2.6,
        35.0,
        -25.0,
    );
    for level in 0..gpu.level_count() {
        let states = [ShipRenderState {
            lod: level,
            ..Default::default()
        }];
        let instances = [ShipInstance {
            ship: &gpu,
            transform: mesh_to_world(),
            age: 1.0,
        }];
        let extras = SceneExtras {
            ship_states: &states,
            effects: None,
        };
        let img = render(&mut ctx, &cam, &instances, &extras);
        assert!(lit_pixels(&img) > 1000, "level {level} drew nothing");
        tiles.push(img);
    }
    assert_ne!(tiles[0], tiles[tiles.len() - 1]);
    let (w, h, img) = montage(&tiles);
    save_png(&out_dir().join("lod_levels.png"), w, h, &img);

    // The switch itself: the same ship at the distances just either side of a level change.
    let mut switch_tiles = Vec::new();
    let mut prev = None;
    let mut dist = 200.0f32;
    let mut found = None;
    while dist < 20000.0 {
        let cam = camera(Vec3::ZERO, dist, 30.0, -20.0);
        let proj = cam.projection(TILE.0 as f32 / TILE.1 as f32);
        let r = screen_radius(
            cam.view(),
            proj,
            mesh_to_world().transform_point3(centre),
            radius,
        );
        let l = selector.select(gpu.level_count(), 0, r, 0.0, None);
        if prev.is_some_and(|p| p != l) && l == 2 {
            found = Some((dist / 1.05, dist, prev.unwrap(), l));
            break;
        }
        prev = Some(l);
        dist *= 1.05;
    }
    let (d0, d1, l0, l1) = found.expect("a switch to level 2");
    for (d, l) in [(d0, l0), (d1, l1)] {
        // Zoomed in by narrowing the field of view so the mesh fills the tile at that distance.
        let mut cam = camera(Vec3::ZERO, d, 30.0, -20.0);
        cam.tweaks.field_of_view = (2.0 * (radius * 1.6 / d).atan()).to_degrees();
        cam.tweaks.clip_near = d * 0.5;
        let states = [ShipRenderState {
            lod: l,
            ..Default::default()
        }];
        let instances = [ShipInstance {
            ship: &gpu,
            transform: mesh_to_world(),
            age: 1.0,
        }];
        let extras = SceneExtras {
            ship_states: &states,
            effects: None,
        };
        switch_tiles.push(render(&mut ctx, &cam, &instances, &extras));
    }
    eprintln!("LOD switch {l0} -> {l1} between {d0:.0} and {d1:.0} units");
    let (w, h, img) = montage(&switch_tiles);
    save_png(&out_dir().join("lod_switch.png"), w, h, &img);
}

#[test]
fn mad_animation_mid_play() {
    let Some(mut ctx) = setup() else { return };
    let (def, levels, gpu) = ship(&ctx, "race_sect", "sacolyte");
    let clip = load_mad_clip(ctx.src.as_ref(), &def).expect("acolyte .mad");
    assert_eq!(clip.remap.len(), 4);
    let skeleton = gpu.skeleton(0);
    let binding = clip.bind(skeleton);
    assert!(
        binding.iter().all(Option::is_some),
        "every acolyte object is animated"
    );

    // At an animation's first frame the path reproduces the mesh's own local matrices.
    let mut probe = ShipAnimator::default();
    probe.play(&clip, clip.find("CombatIn").unwrap(), 0);
    let pose = clip.pose(&probe, &binding).unwrap();
    for (i, m) in pose.iter().enumerate() {
        let rest = skeleton.objects[i].rest;
        let m = m.unwrap();
        let err = (m.w_axis - rest.w_axis).length()
            + (m.x_axis - rest.x_axis).length()
            + (m.y_axis - rest.y_axis).length();
        assert!(
            err < 0.05,
            "object {} differs from rest by {err}: {m:?} vs {rest:?}",
            skeleton.objects[i].name
        );
    }

    let (centre, radius) = levels[0].bounds();
    let cam = camera(
        mesh_to_world().transform_point3(centre),
        radius * 2.4,
        40.0,
        -30.0,
    );
    let mut tiles = Vec::new();
    let mut moved = Vec::new();
    for (name, fraction) in [
        ("", 0.0f32),
        ("CombatIn", 1.0),
        ("Link", 0.5),
        ("Link", 1.0),
        ("Unlink", 0.5),
    ] {
        let mut anim = ShipAnimator::default();
        if !name.is_empty() {
            let n = clip.find(name).unwrap();
            anim.play(&clip, n, 0);
            let a = &clip.mad.animations[clip.file_animation(n).unwrap()];
            let steps = 40;
            let dt = (a.end_time - a.start_time) * fraction / steps as f32;
            for _ in 0..steps {
                anim.update(&clip, dt);
            }
        }
        let pose = clip.pose(&anim, &binding);
        if let Some(p) = &pose {
            let mats = skeleton.resolve(|j| p[j].unwrap_or(skeleton.objects[j].rest));
            let rest = skeleton.rest_matrices();
            moved.push(
                mats.iter()
                    .zip(&rest)
                    .map(|(a, b)| a.w_axis.distance(b.w_axis))
                    .fold(0.0f32, f32::max),
            );
        }
        let states = [ShipRenderState {
            pose: pose.as_deref(),
            ..Default::default()
        }];
        let instances = [ShipInstance {
            ship: &gpu,
            transform: mesh_to_world(),
            age: 1.0,
        }];
        let extras = SceneExtras {
            ship_states: &states,
            effects: None,
        };
        tiles.push(render(&mut ctx, &cam, &instances, &extras));
    }
    eprintln!("largest object displacement per pose: {moved:?}");
    assert!(
        moved.iter().any(|&d| d > 1.0),
        "animation moved nothing: {moved:?}"
    );
    assert_ne!(tiles[0], tiles[2], "Link mid-play looks like the rest pose");
    let (w, h, img) = montage(&tiles);
    save_png(&out_dir().join("mad_acolyte.png"), w, h, &img);
}

#[test]
fn special_render_states() {
    let Some(mut ctx) = setup() else { return };
    let (_d, levels, destroyer) = ship(&ctx, "race_sect", "sdestroyer");
    let (_c, _l, frigate) = ship(&ctx, "race_sect", "shivefrigate");
    let (centre, radius) = levels[0].bounds();
    let bx = CollisionBox {
        corner: centre - Vec3::splat(radius * 0.7),
        extent: Vec3::splat(radius * 1.4),
    };
    let cam = camera(
        mesh_to_world().transform_point3(centre),
        radius * 2.2,
        50.0,
        -25.0,
    );
    let team = normalized_team_color(ctx.colors.schemes[0].base);
    let phase = phase_blend_state(5.0, 0.0, 10.0).unwrap();
    let base = ShipRenderState {
        tint: ShipTint::default().params(team, [1.0, 0.0, 0.0, 0.0]),
        time: 3.0,
        ..Default::default()
    };
    let variants = [
        base,
        ShipRenderState { fade: 0.4, ..base },
        ShipRenderState {
            clip_plane: Some(cloak_wipe_plane(&bx, 0.5)),
            ..base
        },
        ShipRenderState {
            emp_lightning: true,
            ..base
        },
        ShipRenderState {
            phase: Some(PhaseState {
                stage: phase.0,
                value: phase.1,
                converted: Some(&frigate),
                collision_box: bx,
            }),
            ..base
        },
    ];
    let mut tiles = Vec::new();
    for state in variants {
        let states = [state];
        let instances = [ShipInstance {
            ship: &destroyer,
            transform: mesh_to_world(),
            age: 1.0,
        }];
        let extras = SceneExtras {
            ship_states: &states,
            effects: None,
        };
        tiles.push(render(&mut ctx, &cam, &instances, &extras));
    }
    let lit: Vec<usize> = tiles.iter().map(|t| lit_pixels(t)).collect();
    eprintln!("lit pixels per state: {lit:?}");
    assert!(lit[2] < lit[0], "the wipe should cut part of the hull away");
    assert_ne!(tiles[0], tiles[1]);
    assert_ne!(tiles[0], tiles[3]);
    let (w, h, img) = montage(&tiles);
    save_png(&out_dir().join("render_states.png"), w, h, &img);
}

/// Frigates seen from the side with their engine trails, the nozzles placed without the mesh
/// objects' matrices (`before`: how they were drawn when no animation played) and through them
/// (`after`, as `trailGetNozzleWorldPosition` does). The ramming frigate's first object sits
/// 162 units behind the mesh origin, so its trails started inside the hull. Images go to
/// `screenshots/aa/frigate-trails-{before,after}.png`.
#[test]
fn frigate_trails_follow_their_engines() {
    let Some(mut ctx) = setup() else { return };
    let assets = ctx.renderer.load_engine_effects(ctx.src.as_ref());
    let tweaks = read_script(ctx.src.as_ref(), "trail.script")
        .map(|s| TrailTweaks::from_script(&s))
        .unwrap_or_default();
    let team_trails = read_script(ctx.src.as_ref(), "teamcolors.script")
        .map(|s| TeamTrailColors::from_script(&s))
        .unwrap_or_default();
    let team_color = normalized_team_color(ctx.colors.schemes[0].base);
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../screenshots/aa");
    std::fs::create_dir_all(&dir).unwrap();
    let ships = [
        ("race_sect", "srammingfrigate"),
        ("race_sect", "smultibeamfrigate"),
        ("race_sect", "shivefrigate"),
    ];
    let mut before = Vec::new();
    let mut after = Vec::new();
    for (race, name) in ships {
        let (def, _levels, gpu) = ship(&ctx, race, name);
        let mut tdef = ShipTrailDef::load(ctx.src.as_ref(), &def);
        if let Some(r) = &mut tdef.ramp {
            r.recolorize_all(&team_trails);
        }
        assert!(tdef.trail_count() > 0, "{name} has engines");
        let sk = gpu.skeleton(0);
        let rest = sk.rest_matrices();
        let speed = def.stats.maxvelocity.unwrap_or(300.0);
        let (_, radius) = gpu.bounds();
        for (objects, out) in [(None, &mut before), (Some((sk, rest.as_slice())), &mut after)] {
            let mut trails = ShipTrails::new(&tdef, 99);
            let ticks = 48;
            let (transform, velocity) = flight(ticks as f32 * TICK, speed, 0.0);
            let pos = transform.w_axis.truncate();
            // From the side (the flight runs along world -X), level, a little behind the ship.
            let cam = camera(pos + Vec3::new(radius * 0.3, 0.0, 0.0), radius * 1.7, 90.0, -6.0);
            let eye_space_z = cam.view().transform_point3(pos).z;
            let mut effects = EffectBatch::default();
            for k in 0..=ticks {
                let (transform, velocity) = flight(k as f32 * TICK, speed, 0.0);
                trails.update(&tdef, &tweaks, &TrailTick { transform, objects });
                let frame = TrailFrame {
                    transform,
                    objects,
                    velocity,
                    max_velocity: speed,
                    gate: TrailGate {
                        moving: true,
                        ..Default::default()
                    },
                    lod: 0,
                    team: 0,
                    team_color,
                    eye: cam.eye(),
                    eye_space_z,
                    scale: 1.0,
                    fade: 1.0,
                    glow_texture: assets.glow_texture,
                };
                effects.clear();
                trails.draw(&tdef, &tweaks, &frame, &mut effects);
            }
            let _ = velocity;
            let states = [ShipRenderState {
                tint: ShipTint::default().params(team_color, trails.engine_glow_levels(&tweaks)),
                ..Default::default()
            }];
            let instances = [ShipInstance {
                ship: &gpu,
                transform,
                age: 1.0,
            }];
            let extras = SceneExtras {
                ship_states: &states,
                effects: Some(&effects),
            };
            out.push(render(&mut ctx, &cam, &instances, &extras));
        }
    }
    // The ramming frigate's picture changes; the others' first objects sit at the origin.
    let changed = |i: usize| {
        before[i]
            .chunks(4)
            .zip(after[i].chunks(4))
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 12))
            .count()
    };
    assert!(changed(0) > 50, "ramming frigate trails did not move ({})", changed(0));
    for (i, (_, name)) in ships.iter().enumerate().skip(1) {
        assert!(changed(i) < 20, "{name}: trails moved ({})", changed(i));
    }
    for (tiles, file) in [(&before, "frigate-trails-before.png"), (&after, "frigate-trails-after.png")] {
        let (w, h, img) = montage(tiles);
        save_png(&dir.join(file), w, h, &img);
    }
}
