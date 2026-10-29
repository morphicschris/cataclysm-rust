//! The sentinels' force fields as the original draws them (`SentinelUpdateFieldSegments`
//! 0x5c1130, `SentinelUpdateFieldSegment` 0x5c0840, `SentinelQueueFieldHit` 0x5c04c0 and
//! `SentinelFieldHitEffect` 0x5c0a20, which despite its name draws one field triangle).
//!
//! When a sentinel of a field wall or sphere is drawn, every field triangle of its formation
//! whose owner's field has started to rise is queued with its distance from the eye (the
//! centre of its three corners); the queue keeps the nearest 79 (34 with Direct3D) and they
//! are drawn far to near. A triangle's corners are the `DPT_ForceFieldOrigin` points of its
//! three sentinels. Each draws:
//!
//! - two additive lines from the owner to the other two corners, coloured by the field level
//!   (blue, green, red), fading in over the first three quarters of the rise at the corners
//!   and over the last quarter at the midpoints;
//! - once fully risen, the triangle itself, additive, in the level's colour at alpha 100: a
//!   freshly raised field first flickers white, stays white, then fades to the colour, and a
//!   field hit in the last 10 seconds is lightened towards white;
//! - and a red glow sprite (`etg\textures\glow32`) of up to 4 units at each corner, without
//!   depth test in the original (depth tested here, like the other effect sprites).

use std::collections::BTreeSet;

use glam::{Mat4, Vec2, Vec3};
use hwc_render::effects::{EffectBatch, EffectVertex, Line, Sprite, Triangle, FULL_UV};
use hwc_render::{Blend, EffectTexture};
use hwc_sim::object::ShipId;
use hwc_sim::ships::sentinel::{self, formation_kind, SentinelSpec};
use hwc_sim::ships::ShipSpec;
use hwc_sim::statics::enums::DockPointType;
use hwc_sim::Universe;

/// Queue length with OpenGL (`(-(RGLtype != 0) & 0x2d) + 0x22`; 34 with Direct3D).
const QUEUE: usize = 0x22 + 0x2d;
/// Rise fraction at which the corners are fully lit (0x8bc584) and the span of the midpoints'
/// fade-in after it (0x8bc588).
const RISE_CORNERS: f32 = 0.75;
const RISE_MIDDLE: f32 = 0.25;
/// Rise fraction at which the corner sprites reach full size (0x8bc58c) and that size
/// (0x8bc590).
const RISE_SPRITES: f32 = 0.65;
const SPRITE_SIZE: f32 = 4.0;
/// How long a hit lightens the field (0x8bc578).
const HIT_FADE: f32 = 10.0;
/// The corner sprites' colour (0x8bc594).
const SPRITE_COLOR: u32 = 0xff00_00ff;
/// White at alpha 150, the flash colour of the triangles (0x8bc5bc).
const FLASH: u32 = 0x96ff_ffff;
/// By field level: the lines' colours at the corners (0x8bc598, 0x8bc5a4, 0x8bc5b0) and the
/// midpoints (0x8bc59c, 0x8bc5a8, 0x8bc5b4), the triangle's (0x8bc5a0, 0x8bc5ac, 0x8bc5b8).
const CORNER: [u32; 3] = [0xffff_0000, 0xff00_ff00, 0xff00_00ff];
const MIDDLE: [u32; 3] = [0xffff_0000, 0xff00_ff00, 0xff00_00ff];
const FILL: [u32; 3] = [0x64c8_0000, 0x6400_c800, 0x6400_00c8];
/// The colours the lines fade from (0x91e0c8..0x91e0dc: never written, so transparent black).
const FROM: u32 = 0;

/// `colBlendAlpha` (0x54ff10): `a × f + b × (1 - f)` per byte (alpha too), `f` in 256ths
/// (at most 255), each channel divided by 256.
pub fn col_blend_alpha(a: u32, b: u32, f: f32) -> u32 {
    let i = (f * 256.0).round_ties_even() as i32;
    let i = if i > 0xff { 0xff } else { (i & 0xff) as u32 };
    let mut out = 0;
    for shift in [0, 8, 16, 24] {
        let ca = (a >> shift) & 0xff;
        let cb = (b >> shift) & 0xff;
        out |= (((ca * i + cb * (0xff - i)) >> 8) & 0xff) << shift;
    }
    out
}

/// A GL colour (red in the low byte) as linear 0..1 RGBA.
fn rgba(c: u32) -> [f32; 4] {
    [0, 8, 16, 24].map(|s| ((c >> s) & 0xff) as f32 / 255.0)
}

/// One queued field triangle (`SentinelQueueFieldHit`'s 0x38-byte entry).
struct Queued {
    spec: SentinelSpec,
    corners: [Vec3; 3],
    distance: f32,
}

fn spec_of(u: &Universe, id: ShipId) -> Option<&SentinelSpec> {
    match &u.ships.get(id)?.spec {
        ShipSpec::Sentinel(f) => Some(f),
        _ => None,
    }
}

/// The ship's `DPT_ForceFieldOrigin` point in the world, through its drawn transform.
fn field_origin(
    u: &Universe,
    id: ShipId,
    transform: &impl Fn(ShipId) -> Option<Mat4>,
) -> Option<Vec3> {
    let s = u.ships.get(id)?;
    let m = transform(id)?;
    let point = u
        .statics
        .ship(s.ship_type)
        .and_then(|st| st.dock.as_ref())
        .and_then(|d| {
            let k = d.first_of_type[DockPointType::ForceFieldOrigin.index()];
            d.points.get(usize::from(k))
        })
        .map_or(Vec3::ZERO, |p| {
            Vec3::new(p.position.x, p.position.y, p.position.z)
        });
    Some(m.transform_point3(point))
}

/// Queues the field triangles of the formations of the drawn sentinels (`rndRenderObject`:
/// each formation once a frame) and draws them far to near. `transform` gives a ship's drawn
/// transform; `random` is the particle stream the flicker draws from.
pub fn draw(
    u: &Universe,
    drawn: impl IntoIterator<Item = ShipId>,
    transform: impl Fn(ShipId) -> Option<Mat4>,
    eye: Vec3,
    glow: EffectTexture,
    random: &mut impl FnMut() -> u32,
    out: &mut EffectBatch,
) {
    let mut seen = BTreeSet::new();
    let mut queue: Vec<Queued> = Vec::new();
    for id in drawn {
        let Some(s) = u.ships.get(id) else { continue };
        if !sentinel::is_sentinel_type(s.ship_type) {
            continue;
        }
        let Some(c) = s.command.and_then(|c| u.commands.get(c)) else {
            continue;
        };
        if c.attributes & hwc_sim::command::attr::FORMATION == 0
            || !matches!(
                c.formation.formation_type,
                formation_kind::WALL | formation_kind::SPHERE
            )
            || !seen.insert(c.id)
        {
            continue;
        }
        for &owner in &c.selection {
            let Some(f) = spec_of(u, owner) else { continue };
            if f.rise <= 0.0 {
                continue;
            }
            for &(a, b) in &f.segments {
                let (Some(p0), Some(p1), Some(p2)) = (
                    field_origin(u, owner, &transform),
                    field_origin(u, a, &transform),
                    field_origin(u, b, &transform),
                ) else {
                    continue;
                };
                let distance = ((p0 + p1 + p2) * 0.333_333_34 - eye).length();
                // The nearest QUEUE stay; a full queue drops its farthest.
                if queue.len() >= QUEUE {
                    let far = queue.iter().map(|q| q.distance).fold(f32::MIN, f32::max);
                    if far <= distance {
                        continue;
                    }
                    let k = queue
                        .iter()
                        .position(|q| q.distance == far)
                        .expect("farthest");
                    queue.remove(k);
                }
                queue.push(Queued {
                    spec: f.clone(),
                    corners: [p0, p1, p2],
                    distance,
                });
            }
        }
    }
    // Far to near; equal distances keep their queueing order.
    queue.sort_by(|x, y| y.distance.total_cmp(&x.distance));
    let now = u.total_time_elapsed;
    for q in &queue {
        segment(q, now, glow, random, out);
    }
}

/// `SentinelFieldHitEffect`: one field triangle.
fn segment(
    q: &Queued,
    now: f32,
    glow: EffectTexture,
    random: &mut impl FnMut() -> u32,
    out: &mut EffectBatch,
) {
    let f = &q.spec;
    let level = f.level.min(2);
    let since_hit = now - f.last_hit;
    let t = (f.rise / RISE_CORNERS).min(1.0);
    let corner = col_blend_alpha(FROM, CORNER[level], 1.0 - t);
    let t = ((f.rise - RISE_CORNERS) / RISE_MIDDLE).clamp(0.0, 1.0);
    let middle = col_blend_alpha(FROM, MIDDLE[level], 1.0 - t);
    let [v0, v1, v2] = q.corners;
    for v in [v1, v2] {
        let mid = v0 + (v - v0) * 0.5;
        for (from, to) in [(v0, mid), (mid, v)] {
            let (fc, tc) = if from == v0 {
                (corner, middle)
            } else {
                (middle, corner)
            };
            out.lines.push(Line {
                from,
                to,
                from_color: rgba(fc),
                to_color: rgba(tc),
                width: 1.0,
                blend: Blend::Additive,
            });
        }
    }
    if f.rise != 1.0 {
        return;
    }
    let fill = FILL[level];
    let g = 1.0 - f.glow;
    let mut color = if g < 0.5 {
        col_blend_alpha(FLASH, fill, random() as f32 * 2.328_306_4e-10)
    } else if g < 0.8 {
        FLASH
    } else {
        col_blend_alpha(FLASH, fill, (1.0 - g) * 5.0)
    };
    if since_hit <= HIT_FADE {
        let k = (HIT_FADE - since_hit) / HIT_FADE;
        if 0.0 < k {
            color = col_blend_alpha(FLASH, fill, k);
        }
    }
    let c = rgba(color);
    let vertex = |position| EffectVertex {
        position,
        color: c,
        uv: [0.0, 0.0],
    };
    out.triangles.push(Triangle {
        vertices: [vertex(v0), vertex(v1), vertex(v2)],
        texture: EffectTexture::WHITE,
        blend: Blend::Additive,
    });
    let size = SPRITE_SIZE * (f.rise / RISE_SPRITES).min(1.0);
    for position in [v0, v1, v2] {
        out.sprites.push(Sprite {
            position,
            half_size: Vec2::splat(size),
            rotation: 0.0,
            color: rgba(SPRITE_COLOR),
            uv: FULL_UV,
            texture: glow,
            blend: Blend::Additive,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn col_blend_alpha_matches_the_original() {
        // Full weight is 255/256 of the first colour; none is 255/256 of the second.
        assert_eq!(col_blend_alpha(0, 0xffff_0000, 0.0), 0xfefe_0000);
        assert_eq!(col_blend_alpha(0, 0xffff_0000, 1.0), 0);
        assert_eq!(col_blend_alpha(0xffff_ffff, 0, 1.0), 0xfefe_fefe);
        assert_eq!(col_blend_alpha(FLASH, FILL[0], 0.5), 0x7ce2_7f7f);
    }

    /// Twelve upgraded sentinels guarding a processor raise a field sphere: once it is up, its
    /// twenty triangles are drawn with their edges and corner glows, in the level-0 blue.
    #[test]
    fn a_raised_field_sphere_is_drawn() {
        use hwc_formats::big::BigArchive;
        use hwc_math::Vector;
        use hwc_sim::harness::{Run, Scenario, Spawn};
        use hwc_sim::statics::{LoadOptions, ShipRace, ShipType, StaticWorld};
        let Some(dir) = std::env::var_os("HWC_GAME_DIR") else {
            eprintln!("skipped: HWC_GAME_DIR not set");
            return;
        };
        let src = BigArchive::open(std::path::Path::new(&dir).join("cataclysm.big")).unwrap();
        let statics = std::sync::Arc::new(StaticWorld::load_from(&src, LoadOptions::default()));
        let tweaks = hwc_sim::SimTweaks::load(&src);
        let mut spawns: Vec<Spawn> = (0..12)
            .map(|i| Spawn {
                ship_type: ShipType::sSentinel,
                player: 0,
                position: Vector::new(-800.0, (i as f32 - 5.5) * 150.0, 0.0),
            })
            .collect();
        spawns.push(Spawn {
            ship_type: ShipType::sProcessor,
            player: 0,
            position: Vector::ZERO,
        });
        let s = Scenario {
            name: "field drawing",
            players: vec![ShipRace::Somtaaw],
            spawns,
            orders: Vec::new(),
            resources: Vec::new(),
            resource_units: Vec::new(),
        };
        let mut run = Run::new(statics, tweaks, s).unwrap();
        let ids: Vec<ShipId> = run.ids[..12].to_vec();
        for &i in &ids {
            run.universe.ships.ship_mut(i).upgrade_flags |= 0x1000;
        }
        hwc_sim::player_orders::mr_protect_ships(&mut run.universe, &ids, Some(&[run.ids[12]]));
        fn at(u: &Universe) -> impl Fn(ShipId) -> Option<Mat4> + '_ {
            move |id| {
                u.ships.get(id).map(|s| {
                    let m = &s.rotinfo.coordsys.m;
                    let p = s.posinfo.position;
                    Mat4::from_cols(
                        glam::Vec4::new(m[0], m[1], m[2], 0.0),
                        glam::Vec4::new(m[3], m[4], m[5], 0.0),
                        glam::Vec4::new(m[6], m[7], m[8], 0.0),
                        glam::Vec4::new(p.x, p.y, p.z, 1.0),
                    )
                })
            }
        }
        let eye = Vec3::new(0.0, 0.0, 8000.0);
        let mut random = || 0x8000_0000u32;
        let draw_now = |u: &Universe, random: &mut dyn FnMut() -> u32| {
            let mut out = EffectBatch::default();
            draw(
                u,
                ids.iter().copied(),
                at(u),
                eye,
                EffectTexture(7),
                &mut || random(),
                &mut out,
            );
            out
        };
        assert!(
            draw_now(&run.universe, &mut random).is_empty(),
            "no field before it rises"
        );
        // Most of the sphere comes up (a sentinel can stay short of its slot, its triangles
        // then not drawn).
        let raised = |u: &Universe| -> usize {
            ids.iter()
                .filter_map(|&i| spec_of(u, i))
                .filter(|f| f.rise == 1.0 && f.glow == 0.0)
                .map(|f| f.segments.len())
                .sum()
        };
        for _ in 0..2000 {
            run.step();
            if raised(&run.universe) >= 12 {
                break;
            }
        }
        let n = raised(&run.universe);
        assert!(n >= 12, "{n} field triangles up");
        let out = draw_now(&run.universe, &mut random);
        assert_eq!(out.triangles.len(), n);
        assert_eq!(out.lines.len(), 4 * n);
        assert_eq!(out.sprites.len(), 3 * n);
        assert!(out
            .sprites
            .iter()
            .all(|s| s.texture == EffectTexture(7) && s.half_size.x == SPRITE_SIZE));
        let fill = rgba(col_blend_alpha(FLASH, FILL[0], 0.0));
        assert!(out
            .triangles
            .iter()
            .all(|t| t.vertices[0].color == fill && t.blend == Blend::Additive));
        assert_eq!(out.lines[0].from_color, rgba(0xfefe_0000));
        // Far to near.
        let d = |t: &Triangle| {
            (t.vertices.iter().map(|v| v.position).sum::<Vec3>() / 3.0 - eye).length()
        };
        assert!(out
            .triangles
            .windows(2)
            .all(|w| d(&w[0]) >= d(&w[1]) - 0.01));
        // Only drawn sentinels queue their formation.
        let mut none = EffectBatch::default();
        draw(
            &run.universe,
            [run.ids[12]],
            at(&run.universe),
            eye,
            EffectTexture(7),
            &mut random,
            &mut none,
        );
        assert!(none.is_empty());
    }
}
