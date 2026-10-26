//! A check of each frame's draw lists before they go to the GPU, and a record of the last
//! frames' counts for the crash log.
//!
//! A ship or effect whose numbers are not finite (a NaN or infinite position, size or
//! transform) would give the rasteriser triangles of unbounded size; such entries are made
//! empty (zero size, transparent) rather than removed, so the effect lists' draw-order spans
//! stay valid. Sizes and counts past what any of the original's effects produce are clamped.
//! The counts of the last [`HISTORY`] frames are kept; when the GPU device is lost they go into
//! `cataclysm-crash.log` with the report, so a hang can be traced to what was being drawn.

use std::collections::VecDeque;
use std::sync::Mutex;

use hwc_fx::RenderList;
use hwc_math::Vector;

/// Frames of counts kept for the crash log.
pub const HISTORY: usize = 120;
/// No effect sprite, point or cube of the original is anywhere near this size (world units).
const MAX_SIZE: f32 = 200_000.0;
/// Line and bolt widths (pixels) past any the effects use.
const MAX_WIDTH: f32 = 64.0;
/// Circle slices of a sprite (the effects use a few dozen at most).
const MAX_SLICES: u32 = 64;
/// Points of one bolt.
const MAX_BOLT_POINTS: usize = 256;

static FRAMES: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// What one frame drew, and what the check changed.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub ships: usize,
    pub sprites: usize,
    pub lines: usize,
    pub bolts: usize,
    pub bolt_points: usize,
    pub points: usize,
    pub cubes: usize,
    pub meshes: usize,
    pub flares: usize,
    pub spans: usize,
    pub largest_sprite: f32,
    /// The game's own effect geometry: sprites, lines, triangles, strips, glow meshes, flares.
    pub batch: [usize; 6],
    /// Entries emptied because a number was not finite.
    pub not_finite: usize,
    /// Entries whose size, width or count was clamped.
    pub clamped: usize,
}

fn finite(v: Vector) -> bool {
    v.x.is_finite() && v.y.is_finite() && v.z.is_finite()
}

fn finite_all(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// Clamps `x` to `max`, counting it.
fn clamp(x: &mut f32, max: f32, clamped: &mut usize) {
    if *x > max {
        *x = max;
        *clamped += 1;
    }
}

/// Checks and counts an effect list (see the module notes).
pub fn check_effects(list: &mut RenderList, stats: &mut Stats) {
    for s in &mut list.sprites {
        if !finite(s.position)
            || !finite_all(&s.size)
            || !s.rotation.is_finite()
            || !finite_all(&s.color)
        {
            s.size = [0.0; 2];
            s.color = [0.0; 4];
            s.position = Vector::ZERO;
            s.rotation = 0.0;
            stats.not_finite += 1;
            continue;
        }
        clamp(&mut s.size[0], MAX_SIZE, &mut stats.clamped);
        clamp(&mut s.size[1], MAX_SIZE, &mut stats.clamped);
        if s.slices > MAX_SLICES {
            s.slices = MAX_SLICES;
            stats.clamped += 1;
        }
        stats.largest_sprite = stats.largest_sprite.max(s.size[0].max(s.size[1]));
    }
    for l in &mut list.lines {
        if !finite(l.start) || !finite(l.end) || !l.width.is_finite() || !finite_all(&l.color) {
            l.start = Vector::ZERO;
            l.end = Vector::ZERO;
            l.width = 0.0;
            l.color = [0.0; 4];
            stats.not_finite += 1;
            continue;
        }
        clamp(&mut l.width, MAX_WIDTH, &mut stats.clamped);
    }
    for b in &mut list.bolts {
        if b.points.len() > MAX_BOLT_POINTS {
            b.points.truncate(MAX_BOLT_POINTS);
            stats.clamped += 1;
        }
        if !b.points.iter().all(|&p| finite(p)) || !b.width.is_finite() {
            b.points.clear();
            b.width = 0.0;
            stats.not_finite += 1;
            continue;
        }
        clamp(&mut b.width, MAX_WIDTH, &mut stats.clamped);
        stats.bolt_points += b.points.len();
    }
    for p in &mut list.points {
        if !finite(p.position) || !p.size.is_finite() {
            p.position = Vector::ZERO;
            p.size = 0.0;
            p.color = [0.0; 4];
            stats.not_finite += 1;
            continue;
        }
        clamp(&mut p.size, MAX_SIZE, &mut stats.clamped);
    }
    for c in &mut list.cubes {
        if !finite(c.position) || !c.size.is_finite() {
            c.position = Vector::ZERO;
            c.size = 0.0;
            c.color = [0.0; 4];
            stats.not_finite += 1;
            continue;
        }
        clamp(&mut c.size, MAX_SIZE, &mut stats.clamped);
    }
    for m in &mut list.meshes {
        if !finite_all(&m.transform.m) || !finite(m.position) || !finite_all(&m.color) {
            m.transform = hwc_math::HMatrix::default();
            m.transform.m = [0.0; 16];
            m.transform.m[15] = 1.0;
            m.color = [0.0; 4];
            stats.not_finite += 1;
        }
    }
    for f in &mut list.flares {
        if !finite(f.position) || !f.intensity.is_finite() {
            f.intensity = 0.0;
            f.position = Vector::ZERO;
            stats.not_finite += 1;
        }
    }
    stats.sprites = list.sprites.len();
    stats.lines = list.lines.len();
    stats.bolts = list.bolts.len();
    stats.points = list.points.len();
    stats.cubes = list.cubes.len();
    stats.meshes = list.meshes.len();
    stats.flares = list.flares.len();
    stats.spans = list.spans.len();
}

/// Checks the game's own effect geometry (trails, glows, the hyperspace window, the
/// stand-ins): entries with numbers that are not finite go, sizes past any the game makes are
/// clamped. (This list has no draw-order spans, so entries can be removed.)
pub fn check_batch(b: &mut hwc_render::EffectBatch, stats: &mut Stats) {
    use glam::{Vec2, Vec3};
    let v3 = |v: Vec3| v.is_finite();
    let c4 = |c: &[f32; 4]| finite_all(c);
    let n = b.sprites.len()
        + b.lines.len()
        + b.triangles.len()
        + b.strips.len()
        + b.glow_meshes.len()
        + b.flares.len();
    b.sprites.retain(|s| {
        v3(s.position) && s.half_size.is_finite() && s.rotation.is_finite() && c4(&s.color)
    });
    b.lines.retain(|l| {
        v3(l.from) && v3(l.to) && l.width.is_finite() && c4(&l.from_color) && c4(&l.to_color)
    });
    b.triangles
        .retain(|t| t.vertices.iter().all(|v| v3(v.position) && c4(&v.color)));
    b.strips
        .retain(|s| s.points.iter().all(|&p| v3(p)) && s.width.is_finite());
    b.glow_meshes
        .retain(|g| g.transform.is_finite() && c4(&g.color));
    b.flares
        .retain(|f| f.centre.is_finite() && f.half_size.is_finite() && c4(&f.color));
    stats.not_finite += n
        - (b.sprites.len()
            + b.lines.len()
            + b.triangles.len()
            + b.strips.len()
            + b.glow_meshes.len()
            + b.flares.len());
    for s in &mut b.sprites {
        if s.half_size.max_element() > MAX_SIZE {
            s.half_size = s.half_size.min(Vec2::splat(MAX_SIZE));
            stats.clamped += 1;
        }
    }
    for l in &mut b.lines {
        clamp(&mut l.width, MAX_WIDTH, &mut stats.clamped);
    }
    for s in &mut b.strips {
        if s.points.len() > MAX_BOLT_POINTS * 4 {
            s.points.truncate(MAX_BOLT_POINTS * 4);
            stats.clamped += 1;
        }
    }
    stats.batch = [
        b.sprites.len(),
        b.lines.len(),
        b.triangles.len(),
        b.strips.len(),
        b.glow_meshes.len(),
        b.flares.len(),
    ];
}

/// Records a frame's counts.
pub fn record(frame: u64, stats: &Stats) {
    let line = format!("frame {frame}: {stats:?}");
    if let Ok(mut f) = FRAMES.lock() {
        if f.len() >= HISTORY {
            f.pop_front();
        }
        f.push_back(line);
    }
}

/// Writes the recorded frames to `cataclysm-draws.log` (replacing it), so that a GPU reset
/// that takes the process down with it still leaves the last frames' counts behind.
pub fn write_history() {
    crate::crash::write_file("cataclysm-draws.log", &history());
}

/// The recorded frames, oldest first.
pub fn history() -> String {
    FRAMES
        .lock()
        .map(|f| f.iter().cloned().collect::<Vec<_>>().join("\n"))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bad_entries_are_emptied_and_huge_ones_clamped() {
        let mut list = RenderList::default();
        let mut s = hwc_fx::render::Sprite {
            position: Vector::ZERO,
            size: [f32::NAN, 1.0],
            rotation: 0.0,
            color: [1.0; 4],
            bias: [0.0; 3],
            texture: None,
            uv: [[0, 0], [1, 1]],
            slices: 0,
            blend: hwc_fx::render::Blend::Additive,
            stipple: false,
            additive: true,
            illum: 0.0,
        };
        list.sprites.push(s.clone());
        s.size = [1e9, 2.0];
        s.slices = 100_000;
        list.sprites.push(s);
        let mut stats = Stats::default();
        check_effects(&mut list, &mut stats);
        assert_eq!(stats.not_finite, 1);
        assert_eq!(list.sprites[0].size, [0.0; 2]);
        assert_eq!(list.sprites[1].size[0], MAX_SIZE);
        assert_eq!(list.sprites[1].slices, MAX_SLICES);
        assert_eq!(stats.sprites, 2);
        record(1, &stats);
        assert!(history().contains("frame 1"));
    }
}
