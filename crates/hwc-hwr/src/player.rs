//! Plays Remastered effects: a tree of running effects per start, advanced by real time and
//! drawn as camera-facing sprites, oriented quads and ribbons.
//!
//! The Homeworld 2 effect runtime itself is not available, so the behaviour here is read from
//! the data: curves run over each effect's normalised life; a ring is a textured quad sized by
//! `Radius` and tinted by `Colour`, spinning by `SpinZ` (radians) and moved by `Offset`; a
//! spray emits `Emitter_Rate` particles a second for `Emitter_Duration` seconds into a cone of
//! `Emitter_Deviation` degrees at `Particle_Speed`, each particle being another effect that
//! moves with the particle, slowed by `Emitter_Drag` (per tenth of a second); a combo starts its
//! effects together; a trail is a ribbon through its parent's recent positions, `Length` seconds
//! long. Lights, beams and lens flares are not drawn.

use std::collections::VecDeque;

use glam::{Mat3, Vec3};

use crate::fx::{BlendMode, Effect, FULL_UV};
use crate::library::{FxId, Library, TexId, TexRef};
use crate::ships::DeathSequence;

/// Most particles alive in one spray.
pub const MAX_SPRAY_PARTICLES: usize = 160;
/// Most primitives drawn per frame (beyond this, effects keep running but are not drawn).
pub const MAX_PRIMITIVES: usize = 6000;

/// Where an effect is: position, velocity and axes (columns: right, up, forward).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub position: Vec3,
    pub velocity: Vec3,
    pub basis: Mat3,
}

impl Anchor {
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            basis: Mat3::IDENTITY,
        }
    }
}

/// A camera-facing textured square.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprite {
    pub position: Vec3,
    pub half_size: f32,
    pub rotation: f32,
    pub color: [f32; 4],
    pub texture: TexId,
    /// Texture rectangle `[u0, v0, u1, v1]`.
    pub uv: [f32; 4],
    pub blend: BlendMode,
}

/// A world-oriented textured quad (corners in order around the quad, uv 00, 10, 11, 01).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quad {
    pub corners: [Vec3; 4],
    pub color: [f32; 4],
    pub texture: TexId,
    pub uv: [f32; 4],
    pub blend: BlendMode,
}

/// A camera-facing ribbon, newest point first.
#[derive(Debug, Clone, PartialEq)]
pub struct Strip {
    pub points: Vec<Vec3>,
    pub width: f32,
    pub head: [f32; 4],
    pub tail: [f32; 4],
    pub texture: TexId,
    pub blend: BlendMode,
}

/// Everything one frame draws.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DrawList {
    pub sprites: Vec<Sprite>,
    pub quads: Vec<Quad>,
    pub strips: Vec<Strip>,
}

impl DrawList {
    pub fn len(&self) -> usize {
        self.sprites.len() + self.quads.len() + self.strips.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn clear(&mut self) {
        self.sprites.clear();
        self.quads.clear();
        self.strips.clear();
    }

    fn full(&self) -> bool {
        self.len() >= MAX_PRIMITIVES
    }
}

/// A small deterministic generator (presentation only).
#[derive(Debug, Clone)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed ^ 0x9e37_79b9_7f4a_7c15 | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// Uniform in 0..1.
    pub fn unit(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }

    /// Uniform in -1..1.
    pub fn sym(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }

    /// A direction within `half_angle` radians of `axis`.
    pub fn cone(&mut self, axis: Vec3, half_angle: f32) -> Vec3 {
        let cos_max = half_angle.min(std::f32::consts::PI).cos();
        let z = 1.0 - self.unit() * (1.0 - cos_max);
        let r = (1.0 - z * z).max(0.0).sqrt();
        let phi = self.unit() * std::f32::consts::TAU;
        let local = Vec3::new(r * phi.cos(), r * phi.sin(), z);
        let axis = axis.normalize_or(Vec3::Z);
        let helper = if axis.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
        let u = axis.cross(helper).normalize();
        let v = axis.cross(u);
        u * local.x + v * local.y + axis * local.z
    }
}

#[derive(Debug, Clone)]
struct Particle {
    position: Vec3,
    velocity: Vec3,
    node: Node,
}

#[derive(Debug, Clone)]
enum State {
    Ring {
        angle: f32,
        radius_k: f32,
        texture: Option<TexRef>,
        frame: u32,
        spawned: bool,
    },
    Spray {
        emitted: f32,
        started: bool,
        particles: Vec<Particle>,
    },
    Trail {
        points: VecDeque<(Vec3, f32)>,
        texture: Option<TexRef>,
    },
    Plain,
}

/// A running effect and the effects it started.
#[derive(Debug, Clone)]
struct Node {
    fx: FxId,
    scale: f32,
    age: f32,
    state: State,
    children: Vec<Node>,
    finished: bool,
}

fn spawn_child(
    lib: &mut Library,
    name: Option<&str>,
    scale: f32,
    rng: &mut Rng,
    depth: u32,
) -> Option<Node> {
    let id = lib.effect(name?)?;
    Node::new(lib, id, scale, rng, depth + 1)
}

impl Node {
    fn new(lib: &mut Library, fx: FxId, scale: f32, rng: &mut Rng, depth: u32) -> Option<Self> {
        if depth > 8 {
            return None;
        }
        let mut children = Vec::new();
        let state = match lib.effects.get(fx)?.clone() {
            Effect::Ring(r) => {
                let texture = r.texture.as_deref().and_then(|t| lib.texture(t));
                let frame = match &texture {
                    Some(TexRef::Anim { frames, .. }) if r.random_bitmap => {
                        (rng.unit() * frames.len() as f32) as u32
                    }
                    _ => 0,
                };
                children.extend(spawn_child(
                    lib,
                    r.birth_spawn.as_deref(),
                    scale,
                    rng,
                    depth,
                ));
                State::Ring {
                    angle: if r.spin_random {
                        rng.unit() * std::f32::consts::TAU
                    } else {
                        0.0
                    },
                    radius_k: 1.0 + rng.sym() * r.radius_epsilon,
                    texture,
                    frame,
                    spawned: false,
                }
            }
            Effect::Spray(_) => State::Spray {
                emitted: 0.0,
                started: false,
                particles: Vec::new(),
            },
            Effect::Combo(c) => {
                for name in &c.children {
                    children.extend(spawn_child(lib, Some(name), scale, rng, depth));
                }
                State::Plain
            }
            Effect::Trail(t) => State::Trail {
                points: VecDeque::new(),
                texture: t.texture.as_deref().and_then(|p| lib.texture(p)),
            },
            Effect::Light(_) | Effect::Unsupported => State::Plain,
        };
        Some(Self {
            fx,
            scale,
            age: 0.0,
            state,
            children,
            finished: false,
        })
    }

    fn done(&self) -> bool {
        self.finished && self.children.is_empty()
    }

    fn update(&mut self, lib: &mut Library, dt: f32, anchor: &Anchor, rng: &mut Rng, depth: u32) {
        self.age += dt;
        let effect = lib.effects[self.fx].clone();
        match (&effect, &mut self.state) {
            (Effect::Ring(r), State::Ring { spawned, .. }) => {
                if self.age >= r.duration {
                    if r.looping {
                        self.age %= r.duration;
                    } else if !self.finished {
                        self.finished = true;
                        if !*spawned {
                            *spawned = true;
                            self.children.extend(spawn_child(
                                lib,
                                r.spawn.as_deref(),
                                self.scale,
                                rng,
                                depth,
                            ));
                        }
                    }
                }
            }
            (
                Effect::Spray(s),
                State::Spray {
                    emitted,
                    started,
                    particles,
                },
            ) => {
                let t = if s.duration > 0.0 {
                    (self.age / s.duration).min(1.0)
                } else {
                    1.0
                };
                let emitting = s.looping || (s.duration > 0.0 && self.age - dt < s.duration);
                let mut count = 0usize;
                if !*started {
                    *started = true;
                    if s.force_first {
                        count += 1;
                    }
                    if s.duration <= 0.0 {
                        count += (s.rate.at(0.0) / 30.0).round().max(1.0) as usize;
                    }
                }
                if emitting {
                    *emitted += s.rate.at(t) * dt;
                    count += *emitted as usize;
                    *emitted = emitted.fract();
                }
                let room = MAX_SPRAY_PARTICLES.saturating_sub(particles.len());
                let axis = anchor.basis.y_axis;
                for _ in 0..count.min(room) {
                    let Some(p) = s.particle.as_deref().and_then(|n| lib.effect(n)) else {
                        break;
                    };
                    let off = Vec3::from(s.offset.at(t)) * self.scale;
                    let vol = Vec3::from(s.volume.at(t)) * self.scale;
                    let jitter = Vec3::new(rng.sym() * vol.x, rng.sym() * vol.y, rng.sym() * vol.z);
                    let position = anchor.position + anchor.basis * (off + jitter);
                    let dir = rng.cone(axis, (s.deviation.at(t) * 0.5).to_radians());
                    let speed = s.speed.at(t) * if s.scale_speed { self.scale } else { 1.0 };
                    let velocity = dir * speed + anchor.velocity * s.inherit_velocity;
                    let pscale = self.scale * s.particle_scale.at(t).max(0.0);
                    if let Some(node) = Node::new(lib, p, pscale, rng, depth + 1) {
                        particles.push(Particle {
                            position,
                            velocity,
                            node,
                        });
                    }
                }
                let drag = s.drag.clamp(0.0, 1.0).powf(dt * 10.0);
                for p in particles.iter_mut() {
                    p.position += p.velocity * dt;
                    p.velocity *= drag;
                    let basis = basis_along(p.velocity, anchor.basis);
                    let a = Anchor {
                        position: p.position,
                        velocity: p.velocity,
                        basis,
                    };
                    p.node.update(lib, dt, &a, rng, depth + 1);
                }
                particles.retain(|p| !p.node.done());
                if !emitting && particles.is_empty() {
                    self.finished = true;
                }
            }
            (Effect::Trail(tr), State::Trail { points, .. }) => {
                for p in points.iter_mut() {
                    p.1 += dt;
                }
                points.push_front((anchor.position, 0.0));
                while points.back().is_some_and(|p| p.1 > tr.length) {
                    points.pop_back();
                }
                points.truncate(64);
                if self.age >= tr.duration + tr.length {
                    self.finished = true;
                }
            }
            (Effect::Light(l), _) => self.finished = self.age >= l.duration,
            _ => self.finished = true,
        }
        for c in self.children.iter_mut() {
            c.update(lib, dt, anchor, rng, depth + 1);
        }
        self.children.retain(|c| !c.done());
    }

    fn draw(&self, lib: &Library, anchor: &Anchor, out: &mut DrawList) {
        if out.full() {
            return;
        }
        match (&lib.effects[self.fx], &self.state) {
            (
                Effect::Ring(r),
                State::Ring {
                    angle,
                    radius_k,
                    texture,
                    frame,
                    ..
                },
            ) if !self.finished => {
                let t = (self.age / r.duration).clamp(0.0, 1.0);
                let radius = r.radius.at(t) * radius_k * self.scale;
                let color = r.colour.at(t);
                // Rings whose art is missing are not drawn.
                if let (true, Some(tx)) = (radius > 0.0 && color[3] > 0.003, texture.as_ref()) {
                    let position =
                        anchor.position + anchor.basis * (Vec3::from(r.offset.at(t)) * self.scale);
                    let (texture, uv) = frame_of(tx, *frame, r.random_bitmap, self.age);
                    if r.billboard {
                        out.sprites.push(Sprite {
                            position,
                            half_size: radius,
                            rotation: angle + r.spin_z.at(t),
                            color,
                            texture,
                            uv,
                            blend: r.blend,
                        });
                    } else {
                        let rot = Mat3::from_axis_angle(
                            anchor.basis.y_axis.normalize_or(Vec3::Y),
                            angle + r.spin_z.at(t),
                        );
                        let a = rot * anchor.basis.x_axis.normalize_or(Vec3::X) * radius;
                        let b = rot * anchor.basis.z_axis.normalize_or(Vec3::Z) * radius;
                        out.quads.push(Quad {
                            corners: [
                                position - a - b,
                                position + a - b,
                                position + a + b,
                                position - a + b,
                            ],
                            color,
                            texture,
                            uv,
                            blend: r.blend,
                        });
                    }
                }
            }
            (Effect::Spray(_), State::Spray { particles, .. }) => {
                for p in particles {
                    let a = Anchor {
                        position: p.position,
                        velocity: p.velocity,
                        basis: basis_along(p.velocity, anchor.basis),
                    };
                    p.node.draw(lib, &a, out);
                }
            }
            (
                Effect::Trail(tr),
                State::Trail {
                    points,
                    texture: Some(tx),
                },
            ) if points.len() >= 2 => {
                let fade = if self.age > tr.duration {
                    1.0 - ((self.age - tr.duration) / tr.length).min(1.0)
                } else {
                    1.0
                };
                let mut head = tr.head;
                let mut tail = tr.tail;
                head[3] *= fade;
                tail[3] *= fade;
                out.strips.push(Strip {
                    points: points.iter().map(|p| p.0).collect(),
                    width: tr.width * self.scale,
                    head,
                    tail,
                    texture: frame_of(tx, 0, false, self.age).0,
                    blend: tr.blend,
                });
            }
            _ => {}
        }
        for c in &self.children {
            c.draw(lib, anchor, out);
        }
    }
}

fn frame_of(t: &TexRef, frame: u32, fixed: bool, age: f32) -> (TexId, [f32; 4]) {
    match t {
        TexRef::Still(id) => (*id, FULL_UV),
        TexRef::Anim { frames, fps, loops } => {
            let n = frames.len().max(1);
            let i = if fixed {
                frame as usize % n
            } else {
                let f = (age * fps) as usize;
                if *loops {
                    f % n
                } else {
                    f.min(n - 1)
                }
            };
            frames[i]
        }
    }
}

/// Axes whose up vector follows `dir` (falling back to `basis`).
fn basis_along(dir: Vec3, basis: Mat3) -> Mat3 {
    let Some(up) = dir.try_normalize() else {
        return basis;
    };
    let helper = if up.dot(basis.z_axis).abs() < 0.95 {
        basis.z_axis
    } else {
        basis.x_axis
    };
    let right = up.cross(helper).normalize_or(Vec3::X);
    let forward = right.cross(up);
    Mat3::from_cols(right, up, forward)
}

/// Where on a hull a death-event marker sits, as a fraction of the ship's radius along its axes
/// (`top_00`: up, `front_01`: forward, ...; `Root` and unknown names: the centre).
pub fn marker_offset(marker: &str, rng: &mut Rng) -> Vec3 {
    let m = marker.to_ascii_lowercase();
    let dir = if m.starts_with("top") {
        Vec3::Y
    } else if m.starts_with("bottom") {
        -Vec3::Y
    } else if m.starts_with("left") {
        -Vec3::X
    } else if m.starts_with("right") {
        Vec3::X
    } else if m.starts_with("front") {
        Vec3::Z
    } else if m.starts_with("back") {
        -Vec3::Z
    } else {
        return Vec3::ZERO;
    };
    dir * (0.35 + 0.3 * rng.unit()) + Vec3::new(rng.sym(), rng.sym(), rng.sym()) * 0.25
}

/// How fast a gone owner's last velocity dies away (per second).
const DRIFT_DECAY: f32 = 1.5;

/// Moves an anchor whose owner is gone on with its last velocity, slowing down.
fn drift(a: &mut Anchor, dt: f32) {
    a.position += a.velocity * dt;
    a.velocity *= (-DRIFT_DECAY * dt).exp();
}

#[derive(Debug, Clone)]
struct Root {
    owner: u64,
    node: Node,
    /// Offset from the owner in its axes.
    offset: Vec3,
    anchor: Anchor,
}

#[derive(Debug, Clone)]
struct Pending {
    owner: u64,
    at: f32,
    fx: Vec<String>,
    markers: Vec<String>,
    scale: f32,
    radius: f32,
    /// The owner when last seen.
    anchor: Anchor,
}

/// Running effects.
#[derive(Debug)]
pub struct Player {
    pub lib: Library,
    roots: Vec<Root>,
    pending: Vec<Pending>,
    time: f32,
    rng: Rng,
}

impl Player {
    pub fn new(lib: Library, seed: u64) -> Self {
        Self {
            lib,
            roots: Vec::new(),
            pending: Vec::new(),
            time: 0.0,
            rng: Rng::new(seed),
        }
    }

    /// Effects running or waiting to start.
    pub fn active(&self) -> usize {
        self.roots.len() + self.pending.len()
    }

    /// Starts effect `name` at `offset` (in the owner's axes) from `anchor`, following `owner`.
    pub fn spawn(
        &mut self,
        name: &str,
        owner: u64,
        anchor: Anchor,
        offset: Vec3,
        scale: f32,
    ) -> bool {
        let Some(id) = self.lib.effect(name) else {
            return false;
        };
        let Some(node) = Node::new(&mut self.lib, id, scale, &mut self.rng, 0) else {
            return false;
        };
        self.roots.push(Root {
            owner,
            node,
            offset,
            anchor,
        });
        true
    }

    /// Queues a ship's death sequence. `anchor` is the ship now; `radius` its size; `entity` the
    /// scale used for `fx_scale = "entity"` events and `size` a factor on numeric scales.
    pub fn start_death(
        &mut self,
        seq: &DeathSequence,
        owner: u64,
        anchor: Anchor,
        radius: f32,
        entity: f32,
        size: f32,
    ) {
        for e in &seq.events {
            self.pending.push(Pending {
                owner,
                at: self.time + e.time,
                fx: e.fx.clone(),
                markers: if e.markers.is_empty() {
                    vec!["Root".into()]
                } else {
                    e.markers.clone()
                },
                scale: e.scale.map_or(entity, |s| s * size),
                radius,
                anchor,
            });
        }
    }

    /// Advances everything by `dt` seconds. `owner` gives the current anchor of an owner while it
    /// exists; effects of a gone owner drift on with its last velocity, slowing down.
    pub fn update(&mut self, dt: f32, owner: impl Fn(u64) -> Option<Anchor>) {
        self.time += dt;
        for p in self.pending.iter_mut() {
            match owner(p.owner) {
                Some(a) => p.anchor = a,
                None => drift(&mut p.anchor, dt),
            }
        }
        let due: Vec<Pending> = {
            let (due, wait): (Vec<_>, Vec<_>) =
                self.pending.drain(..).partition(|p| p.at <= self.time);
            self.pending = wait;
            due
        };
        for p in due {
            let anchor = p.anchor;
            for m in &p.markers {
                let offset = marker_offset(m, &mut self.rng) * p.radius;
                for f in &p.fx {
                    self.spawn(f, p.owner, anchor, offset, p.scale);
                }
            }
        }
        let lib = &mut self.lib;
        let rng = &mut self.rng;
        for r in self.roots.iter_mut() {
            match owner(r.owner) {
                Some(a) => r.anchor = a,
                None => drift(&mut r.anchor, dt),
            }
            let mut a = r.anchor;
            a.position += a.basis * r.offset;
            r.node.update(lib, dt, &a, rng, 0);
        }
        self.roots.retain(|r| !r.node.done());
    }

    /// Drops everything (a level change).
    pub fn clear(&mut self) {
        self.roots.clear();
        self.pending.clear();
    }

    pub fn draw(&self, out: &mut DrawList) {
        for r in &self.roots {
            let mut a = r.anchor;
            a.position += a.basis * r.offset;
            r.node.draw(&self.lib, &a, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fx::FxDef;

    fn def(name: &str, style: &str, props: &str) -> FxDef {
        FxDef::parse(
            name,
            format!("fx = {{ style = \"{style}\", properties = {{ {props} }}, }}").as_bytes(),
        )
        .unwrap()
    }

    fn p(n: usize, name: &str, ty: &str, value: &str) -> String {
        format!("property_{n:02} = {{ name = \"{name}\", type = \"{ty}\", value = {value}, }},")
    }

    fn lib() -> Library {
        let ring = def(
            "flash",
            "STYLE_RING",
            &[
                p(0, "Duration", "VARTYPE_FLOAT", "1"),
                p(1, "Radius", "VARTYPE_ARRAY_TIMEFLOAT", "{ 0, 1, 1, 3 }"),
                p(
                    2,
                    "Colour",
                    "VARTYPE_ARRAY_TIMECOLOUR",
                    "{ entry_00 = { 0, 1, 1, 1, 1 }, entry_01 = { 1, 1, 1, 1, 0 } }",
                ),
                p(3, "Blending", "VARTYPE_INT", "2"),
                p(4, "Texture", "VARTYPE_STRING", "\"DATA:ART/FX/FLASH.TGA\""),
            ]
            .concat(),
        );
        let spray = def(
            "sparks",
            "STYLE_SPRAY",
            &[
                p(0, "Emitter_Duration", "VARTYPE_FLOAT", "0.5"),
                p(
                    1,
                    "Emitter_Rate",
                    "VARTYPE_ARRAY_TIMEFLOAT",
                    "{ 0, 20, 1, 20 }",
                ),
                p(
                    2,
                    "Particle_Speed",
                    "VARTYPE_ARRAY_TIMEFLOAT",
                    "{ 0, 10, 1, 10 }",
                ),
                p(
                    3,
                    "Emitter_Deviation",
                    "VARTYPE_ARRAY_TIMEFLOAT",
                    "{ 0, 360, 1, 360 }",
                ),
                p(4, "Particle_Fx", "VARTYPE_STRING", "\"flash\""),
                p(5, "Emitter_Drag", "VARTYPE_FLOAT", "1"),
            ]
            .concat(),
        );
        let combo = def(
            "boom",
            "STYLE_COMBO",
            &[
                p(0, "Fx1", "VARTYPE_STRING", "\"FLASH\""),
                p(1, "FX2", "VARTYPE_STRING", "\"sparks\""),
            ]
            .concat(),
        );
        Library::from_defs(&[ring, spray, combo])
    }

    #[test]
    fn ring_grows_and_fades() {
        let mut pl = Player::new(lib(), 1);
        assert!(pl.spawn("flash", 0, Anchor::at(Vec3::ZERO), Vec3::ZERO, 2.0));
        pl.update(0.5, |_| None);
        let mut d = DrawList::default();
        pl.draw(&mut d);
        assert_eq!(d.sprites.len(), 1);
        assert!((d.sprites[0].half_size - 4.0).abs() < 1e-4);
        assert!((d.sprites[0].color[3] - 0.5).abs() < 1e-4);
        assert_eq!(d.sprites[0].blend, BlendMode::Additive);
        pl.update(0.6, |_| None);
        assert_eq!(pl.active(), 0);
    }

    #[test]
    fn combo_spray_emits_and_ends() {
        let mut pl = Player::new(lib(), 7);
        assert!(pl.spawn(
            "boom",
            3,
            Anchor::at(Vec3::new(100.0, 0.0, 0.0)),
            Vec3::ZERO,
            1.0
        ));
        let mut most = 0;
        for _ in 0..30 {
            pl.update(1.0 / 30.0, |_| None);
            let mut d = DrawList::default();
            pl.draw(&mut d);
            most = most.max(d.sprites.len());
        }
        // 20 a second for half a second, plus the combo's own ring (gone after 1 s).
        assert!((8..=13).contains(&most), "{most}");
        let mut d = DrawList::default();
        pl.draw(&mut d);
        assert!(d
            .sprites
            .iter()
            .any(|s| (s.position - Vec3::new(100.0, 0.0, 0.0)).length() > 1.0));
        for _ in 0..60 {
            pl.update(1.0 / 30.0, |_| None);
        }
        assert_eq!(pl.active(), 0);
    }

    #[test]
    fn death_sequence_waits_for_its_time_and_follows_the_owner() {
        let seq = DeathSequence {
            length: 2.0,
            events: vec![crate::ships::DeathEvent {
                time: 1.0,
                fx: vec!["flash".into()],
                markers: vec!["Root".into()],
                scale: Some(1.0),
            }],
        };
        let mut pl = Player::new(lib(), 2);
        pl.start_death(&seq, 9, Anchor::at(Vec3::ZERO), 10.0, 1.0, 1.0);
        let pos = Vec3::new(5.0, 6.0, 7.0);
        pl.update(0.5, |_| Some(Anchor::at(pos)));
        let mut d = DrawList::default();
        pl.draw(&mut d);
        assert!(d.is_empty());
        pl.update(0.6, |_| Some(Anchor::at(pos)));
        pl.draw(&mut d);
        assert_eq!(d.sprites.len(), 1);
        assert_eq!(d.sprites[0].position, pos);
    }

    #[test]
    fn cone_stays_within_angle() {
        let mut r = Rng::new(5);
        for _ in 0..200 {
            let d = r.cone(Vec3::Y, 0.2);
            assert!(d.angle_between(Vec3::Y) <= 0.2001);
            assert!((d.length() - 1.0).abs() < 1e-4);
        }
    }
}
