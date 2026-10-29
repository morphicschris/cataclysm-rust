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
//!
//! With `hwcModernFields` (a user-requested addition, on by default) the same triangles are
//! drawn as an energy bubble instead ([`Modern`]): a field sphere is rounded out between its
//! sentinels, brightest at its rim (a Fresnel falloff), with a drifting hexagon pattern and a
//! scan band sweeping over it; it fades in as the field rises, dims as its strength runs down,
//! flashes as the original does when raised or hit, and every hit sends out a ripple from
//! where the shot met the field ([`hwc_sim::Universe::field_hits`]).

use std::collections::BTreeSet;

use glam::{Mat4, Vec2, Vec3, Vec4};
use hwc_render::effects::{EffectBatch, EffectVertex, Line, Sprite, Triangle, FULL_UV};
use hwc_render::{Blend, EffectTexture, Renderer};
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

/// One queued field triangle (`SentinelQueueFieldHit`'s 0x38-byte entry), with its
/// formation's shape for the modern look.
struct Queued {
    spec: SentinelSpec,
    corners: [Vec3; 3],
    distance: f32,
    /// A field sphere (else a wall), the centre of its sentinels' field points and their mean
    /// distance from it.
    sphere: bool,
    centre: Vec3,
    radius: f32,
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
/// transform; `random` is the particle stream the flicker draws from. With `modern`, they
/// are drawn in the modern look at time `time` (the frame's, between updates).
#[allow(clippy::too_many_arguments)]
pub fn draw(
    u: &Universe,
    drawn: impl IntoIterator<Item = ShipId>,
    transform: impl Fn(ShipId) -> Option<Mat4>,
    eye: Vec3,
    glow: EffectTexture,
    random: &mut impl FnMut() -> u32,
    modern: Option<(&Modern, f32)>,
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
        let sphere = c.formation.formation_type == formation_kind::SPHERE;
        let points: Vec<Vec3> = c
            .selection
            .iter()
            .filter_map(|&s| field_origin(u, s, &transform))
            .collect();
        let count = points.len().max(1) as f32;
        let centre = points.iter().sum::<Vec3>() / count;
        let radius = points.iter().map(|p| (*p - centre).length()).sum::<f32>() / count;
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
                    sphere,
                    centre,
                    radius,
                });
            }
        }
    }
    // Far to near; equal distances keep their queueing order.
    queue.sort_by(|x, y| y.distance.total_cmp(&x.distance));
    let now = u.total_time_elapsed;
    if let Some((m, time)) = modern {
        for q in &queue {
            m.segment(q, now, time, eye, glow, random, out);
        }
        m.ripples(u, &transform, time, glow, out);
        return;
    }
    for q in &queue {
        segment(q, now, glow, random, out);
    }
}

/// How white the original's triangle colour is ([`segment`]): the flicker, hold and fade of a
/// freshly raised field, and the fade after a hit.
fn flash(f: &SentinelSpec, now: f32, random: &mut impl FnMut() -> u32) -> f32 {
    let g = 1.0 - f.glow;
    let mut w = if g < 0.5 {
        random() as f32 * 2.328_306_4e-10
    } else if g < 0.8 {
        1.0
    } else {
        (1.0 - g) * 5.0
    };
    let since_hit = now - f.last_hit;
    if since_hit <= HIT_FADE {
        let k = (HIT_FADE - since_hit) / HIT_FADE;
        if 0.0 < k {
            w = k;
        }
    }
    w.clamp(0.0, 1.0)
}

/// Field colours of the modern look by level (blue, green, red as the original's).
const HUES: [Vec3; 3] = [
    Vec3::new(0.25, 0.55, 1.0),
    Vec3::new(0.3, 1.0, 0.55),
    Vec3::new(1.0, 0.35, 0.25),
];
/// Subdivisions of each side of a field triangle.
const SUBDIVIDE: usize = 5;
/// World units per hexagon column of the pattern.
const PATTERN_SCALE: f32 = 420.0;
/// How long a hit's ripple lasts, and at most how many are kept.
const RIPPLE_TIME: f32 = 0.9;
const RIPPLES: usize = 96;

/// A hit's ripple: where on the field (from the sentinel hit) and when.
struct Ripple {
    sentinel: ShipId,
    offset: Vec3,
    start: f32,
}

/// The modern field look: its textures (generated, not game data) and the hits' ripples.
pub struct Modern {
    pattern: EffectTexture,
    ring: EffectTexture,
    ripples: Vec<Ripple>,
}

impl Modern {
    pub fn new(renderer: &mut Renderer) -> Self {
        let (w, h, hex) = hex_texture();
        let pattern = renderer.create_effect_texture(w, h, &hex);
        let (w, h, ring) = ring_texture();
        let ring = renderer.create_effect_texture(w, h, &ring);
        Self {
            pattern,
            ring,
            ripples: Vec::new(),
        }
    }

    /// Takes the field hits of the last update (call after each).
    pub fn note_hits(&mut self, u: &Universe) {
        let now = u.total_time_elapsed;
        self.ripples.retain(|r| now - r.start < RIPPLE_TIME);
        for &(sentinel, at) in &u.field_hits {
            let Some(s) = u.ships.get(sentinel) else {
                continue;
            };
            let p = s.posinfo.position;
            self.ripples.push(Ripple {
                sentinel,
                offset: Vec3::new(at.x - p.x, at.y - p.y, at.z - p.z),
                start: now,
            });
        }
        if self.ripples.len() > RIPPLES {
            let extra = self.ripples.len() - RIPPLES;
            self.ripples.drain(..extra);
        }
    }

    /// One field triangle as a piece of the bubble.
    #[allow(clippy::too_many_arguments)]
    fn segment(
        &self,
        q: &Queued,
        now: f32,
        time: f32,
        eye: Vec3,
        glow: EffectTexture,
        random: &mut impl FnMut() -> u32,
        out: &mut EffectBatch,
    ) {
        let f = &q.spec;
        let level = f.level.min(2);
        let hue = HUES[level];
        let rise = f.rise.clamp(0.0, 1.0);
        let fade = rise * rise * (3.0 - 2.0 * rise);
        let white = if f.rise == 1.0 {
            flash(f, now, random)
        } else {
            0.0
        };
        let strength = (f.strength / sentinel::FIELD_STRENGTH[level]).clamp(0.0, 1.0);
        let colour = hue.lerp(Vec3::ONE, white * 0.8);
        let [v0, v1, v2] = q.corners;
        let flat = (v1 - v0).cross(v2 - v0).normalize_or_zero();
        let radius = q.radius.max(1.0);
        let corner_r = [v0, v1, v2].map(|v| (v - q.centre).length());
        // A point of the triangle by barycentric weights (rounded onto the sphere) and the
        // surface's normal there.
        let place = |a: f32, b: f32| -> (Vec3, Vec3) {
            let p = v0 + (v1 - v0) * a + (v2 - v0) * b;
            if !q.sphere {
                return (p, flat);
            }
            let r = corner_r[0] * (1.0 - a - b) + corner_r[1] * a + corner_r[2] * b;
            let n = (p - q.centre).normalize_or_zero();
            (q.centre + n * r, n)
        };
        let phase = (time * 0.35).rem_euclid(2.6) - 1.3;
        let axes = pattern_axes(place(1.0 / 3.0, 1.0 / 3.0).1);
        // The plain glow (rim, scan band, flash) and the pattern's colours, and its uv.
        let surface = |p: Vec3, n: Vec3| -> (Vec4, Vec4, [f32; 2]) {
            let view = (eye - p).normalize_or_zero();
            let rim = 1.0 - n.dot(view).abs();
            let h = (p - q.centre).z / radius;
            let scan = (-(h - phase) * (h - phase) / 0.012).exp();
            let pulse = 0.8 + 0.2 * (time * 3.0 + h * 5.0).sin();
            let a = ((0.03 + 0.55 * rim.powi(3) + 0.25 * scan) * (0.5 + 0.5 * strength)
                + 0.3 * white)
                * fade;
            let b = (0.04 + 0.3 * rim * rim + 0.35 * scan) * pulse * (0.4 + 0.6 * strength) * fade;
            let uv = [
                p.dot(axes.0) / PATTERN_SCALE + time * 0.03,
                p.dot(axes.1) / (PATTERN_SCALE * 1.732) - time * 0.02,
            ];
            (colour.extend(a.min(1.0)), colour.extend(b.min(1.0)), uv)
        };
        let n = SUBDIVIDE;
        type Point = (Vec3, Vec4, Vec4, [f32; 2]);
        let grid: Vec<Vec<Point>> = (0..=n)
            .map(|i| {
                (0..=n - i)
                    .map(|j| {
                        let (p, nn) = place(i as f32 / n as f32, j as f32 / n as f32);
                        let (a, b, uv) = surface(p, nn);
                        (p, a, b, uv)
                    })
                    .collect()
            })
            .collect();
        let mut tri = |k: [(usize, usize); 3]| {
            let vs = k.map(|(i, j)| grid[i][j]);
            out.triangles.push(Triangle {
                vertices: vs.map(|(p, a, _, _)| EffectVertex {
                    position: p,
                    color: a.to_array(),
                    uv: [0.0, 0.0],
                }),
                texture: EffectTexture::WHITE,
                blend: Blend::Additive,
            });
            out.triangles.push(Triangle {
                vertices: vs.map(|(p, _, b, uv)| EffectVertex {
                    position: p,
                    color: b.to_array(),
                    uv,
                }),
                texture: self.pattern,
                blend: Blend::Additive,
            });
        };
        for i in 0..n {
            for j in 0..n - i {
                tri([(i, j), (i + 1, j), (i, j + 1)]);
                if i + j + 1 < n {
                    tri([(i + 1, j), (i + 1, j + 1), (i, j + 1)]);
                }
            }
        }
        // The owner's two edges, following the bubble.
        let edge = colour
            .extend((0.45 * (rise / RISE_CORNERS).min(1.0) + 0.4 * white).min(1.0))
            .to_array();
        for side in [(1.0, 0.0), (0.0, 1.0)] {
            let mut last = v0;
            for k in 1..=n {
                let t = k as f32 / n as f32;
                let p = place(side.0 * t, side.1 * t).0;
                out.lines.push(Line {
                    from: last,
                    to: p,
                    from_color: edge,
                    to_color: edge,
                    width: 1.5,
                    blend: Blend::Additive,
                });
                last = p;
            }
        }
        out.sprites.push(Sprite {
            position: v0,
            half_size: Vec2::splat(22.0 * fade),
            rotation: 0.0,
            color: hue.lerp(Vec3::ONE, 0.5).extend(0.7 * fade).to_array(),
            uv: FULL_UV,
            texture: glow,
            blend: Blend::Additive,
        });
    }

    /// The ripples of the hits, spreading from where each shot met the field.
    fn ripples(
        &self,
        u: &Universe,
        transform: &impl Fn(ShipId) -> Option<Mat4>,
        time: f32,
        glow: EffectTexture,
        out: &mut EffectBatch,
    ) {
        for r in &self.ripples {
            let t = (time - r.start) / RIPPLE_TIME;
            if !(0.0..1.0).contains(&t) {
                continue;
            }
            let Some(m) = transform(r.sentinel) else {
                continue;
            };
            let level = spec_of(u, r.sentinel).map_or(0, |f| f.level.min(2));
            let at = m.w_axis.truncate() + r.offset;
            let colour = HUES[level].lerp(Vec3::ONE, 0.6);
            let ease = 1.0 - (1.0 - t) * (1.0 - t);
            out.sprites.push(Sprite {
                position: at,
                half_size: Vec2::splat(40.0 + 260.0 * ease),
                rotation: 0.0,
                color: colour.extend((1.0 - t) * (1.0 - t)).to_array(),
                uv: FULL_UV,
                texture: self.ring,
                blend: Blend::Additive,
            });
            out.sprites.push(Sprite {
                position: at,
                half_size: Vec2::splat(90.0 * (1.0 - t)),
                rotation: 0.0,
                color: colour.extend(0.8 * (1.0 - t)).to_array(),
                uv: FULL_UV,
                texture: glow,
                blend: Blend::Additive,
            });
        }
    }
}

/// The two pattern axes for a surface facing `n`: the plane of its two smaller components.
fn pattern_axes(n: Vec3) -> (Vec3, Vec3) {
    let a = n.abs();
    if a.x >= a.y && a.x >= a.z {
        (Vec3::Y, Vec3::Z)
    } else if a.y >= a.z {
        (Vec3::X, Vec3::Z)
    } else {
        (Vec3::X, Vec3::Y)
    }
}

/// A tiling hexagon grid (one period: 1 by sqrt 3 cells): bright cell edges over a faint
/// fill, white with the pattern in alpha.
fn hex_texture() -> (u32, u32, Vec<u8>) {
    let (w, h) = (128u32, 222u32);
    let period = Vec2::new(1.0, 1.732_050_8);
    let half = period * 0.5;
    let side = period.normalize();
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let uv = Vec2::new(
                (x as f32 + 0.5) / w as f32,
                (y as f32 + 0.5) / h as f32 * period.y,
            );
            let a = uv.rem_euclid(period) - half;
            let b = (uv - half).rem_euclid(period) - half;
            let g = if a.length_squared() < b.length_squared() {
                a
            } else {
                b
            }
            .abs();
            let edge = 0.5 - g.dot(side).max(g.x);
            let line = (1.0 - edge / 0.035).clamp(0.0, 1.0);
            let inner = (1.0 - edge / 0.18).clamp(0.0, 1.0).powi(2) * 0.35;
            let alpha = (line + inner + 0.06).min(1.0);
            px.extend_from_slice(&[255, 255, 255, (alpha * 255.0) as u8]);
        }
    }
    (w, h, px)
}

/// A soft ring for the ripples, white with the ring in alpha.
fn ring_texture() -> (u32, u32, Vec<u8>) {
    let n = 64u32;
    let mut px = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let d = Vec2::new(x as f32 + 0.5, y as f32 + 0.5) / (n as f32 * 0.5) - 1.0;
            let r = d.length();
            let ring = (-((r - 0.8) / 0.09).powi(2)).exp();
            let inner = (1.0 - r).clamp(0.0, 1.0) * 0.15;
            let a = if r < 1.0 {
                (ring + inner).min(1.0)
            } else {
                0.0
            };
            px.extend_from_slice(&[255, 255, 255, (a * 255.0) as u8]);
        }
    }
    (n, n, px)
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
                None,
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
        // The modern look: each triangle as a rounded patch, plain and patterned, with the
        // owner's two edges and a glow; a hit adds a ripple and a flash.
        let mut look = Modern {
            pattern: EffectTexture(8),
            ring: EffectTexture(9),
            ripples: Vec::new(),
        };
        let time = run.universe.total_time_elapsed;
        let modern = |u: &Universe, look: &Modern, random: &mut dyn FnMut() -> u32| {
            let mut out = EffectBatch::default();
            draw(
                u,
                ids.iter().copied(),
                at(u),
                eye,
                EffectTexture(7),
                &mut || random(),
                Some((look, time)),
                &mut out,
            );
            out
        };
        let out = modern(&run.universe, &look, &mut random);
        assert_eq!(out.triangles.len(), n * SUBDIVIDE * SUBDIVIDE * 2);
        assert_eq!(out.lines.len(), n * 2 * SUBDIVIDE);
        assert_eq!(out.sprites.len(), n);
        assert!(out.triangles.iter().any(|t| t.texture == EffectTexture(8)));
        run.universe
            .field_hits
            .push((ids[3], hwc_math::Vector::new(0.0, 0.0, 500.0)));
        look.note_hits(&run.universe);
        let with_hit = modern(&run.universe, &look, &mut random);
        assert_eq!(with_hit.sprites.len(), n + 2);
        assert!(with_hit
            .sprites
            .iter()
            .any(|s| s.texture == EffectTexture(9)));
        // Only drawn sentinels queue their formation.
        let mut none = EffectBatch::default();
        draw(
            &run.universe,
            [run.ids[12]],
            at(&run.universe),
            eye,
            EffectTexture(7),
            &mut random,
            None,
            &mut none,
        );
        assert!(none.is_empty());
    }
}
