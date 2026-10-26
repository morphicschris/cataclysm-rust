//! Nebulae (`Nebulae.c`): the tendril clouds of `Resources Nebula,...` level lines.
//!
//! A level line with the `Nebula` layout calls [`neb_go`] (`nebGo`): it throws 10000 candidate
//! *chunks* into a box around the line's centre, threads the line's number of *tendrils*
//! through them (each tendril a chain of straight segments between chunks, kinking at most
//! `NEB_MAX_TENDRIL_ANGULAR_DEVIATION`), drops the chunks no tendril touches and turns every
//! remaining chunk into a harvestable resource object (`univAddNebula`, kind
//! [`ResourceKind::Nebula`]). Harvesting a chunk fades the tendrils attached to it
//! ([`fade_attached_tendrils`]); exhausting it switches its tendrils off ([`delete_chunk`]).
//!
//! Every update [`set_fog`] (`nebSetFog`, called from `univUpdate`) sums the nebula fog from
//! the chunks in the render list and lets those chunks wobble about. The render list depends
//! on the camera, which the frontend hands over in [`Nebulae::camera`]; without a camera (a
//! headless run) nothing is in it, exactly as when the original renders nothing.
//!
//! The tendril cylinders are made the first time a tendril is drawn (`nebRenderTendril`), with
//! random radii from the nebula stream; [`set_fog`] runs that part of `nebRenderNebula`'s
//! selection too, so the geometry and its random numbers stay in the simulation and the
//! renderer only draws ([`TendrilLod`]).
//!
//! Random numbers: chunk positions, velocities, tendril colours and radii come from the nebula
//! stream (`RAN_Nebulae`, 5) and the cloud stream (`RAN_Clouds`, 4, `cloudRandomSphericalPoint`);
//! only the chunk resources' orientations use the game stream (three numbers each, in chunk
//! order), as for every other resource.

use hwc_math::fastmath::fmath_sqrt_double;
use hwc_math::fpu::{eq_or_unordered, fild_u32, ftol_i32, ftol_u32, ld, lt_or_unordered, st, Ext};
use hwc_math::matrix::HMatrix;
use hwc_math::random::{Randy, RAN_CLOUDS, RAN_NEBULAE};
use hwc_math::trig::{fcos, fsin};
use hwc_math::vector::HVector;
use hwc_math::Vector;

use crate::level::read_lines;
use crate::resource::{self, ResourceId, ResourceKind};
use crate::statics::cscan::scan_f32;
use crate::statics::DataSource;
use crate::universe::Universe;

/// `NEB_MAX_TENDRILS`: tendril slots per chunk.
pub const MAX_TENDRILS_PER_CHUNK: usize = 3;
/// `NUM_NEBTENDRIL_LODS`.
pub const TENDRIL_LODS: usize = 2;
/// The chunk count every level line asks for (the binary passes 10000, not the line's field).
pub const LEVEL_CHUNKS: u32 = 10_000;
/// `MAXD` in `nebColourTendril`: distance at which the darkening is complete.
pub const COLOUR_MAX_DISTANCE: f32 = 40_000.0;
/// `nebradius` (0x87e51c): `NEB_CHUNK_RADIUS * 2`, the fog contribution scale.
pub const FOG_CHUNK_RADIUS: f32 = 1600.0;

/// Chunk flags (`NEB_CHUNK_*`).
pub mod chunk_flags {
    pub const INACTIVE: u32 = 1;
    pub const PICKED: u32 = 2;
    pub const DEAD: u32 = 4;
}

/// Tendril flags (`NEB_TENDRIL_*`).
pub mod tendril_flags {
    pub const INACTIVE: u32 = 1;
    /// The first segment of a chain: faded out at its start.
    pub const LEADING: u32 = 2;
    /// The last segment of a chain: faded out at its end.
    pub const TRAILING: u32 = 4;
    /// The cylinders exist.
    pub const GEOMETRY: u32 = 8;
}

/// How a tendril is attached to a chunk (`NEB_CHUNK_FORE` / `NEB_CHUNK_AFT`).
pub const CHUNK_FORE: u32 = 1;
pub const CHUNK_AFT: u32 = 2;

/// `Nebulae.script` (table at 0x87e3d8..0x87e420) plus the render-list distance it relies on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NebulaTweaks {
    /// `NEB_RADIUS` (spaceobj size; also in [`crate::statics::Tweaks`]).
    pub radius: f32,
    pub fog_red: f32,
    pub fog_green: f32,
    pub fog_blue: f32,
    pub bg_scalar: f32,
    pub tendril_red_dev: f32,
    pub tendril_green_dev: f32,
    pub tendril_blue_dev: f32,
    pub distance_desaturation: f32,
    pub max_tendril_angular_deviation: f32,
    pub max_tendril_distance: f32,
    pub pick_distance_threshold: f32,
    pub pick_distance_minthresh: f32,
    pub shuffle_distance: f32,
    pub vel_ran_multiplier: f32,
    pub ran_range: f32,
    pub vel_base: f32,
    pub tendril_radius_base: f32,
    pub tendril_radius_range: f32,
    /// `TENDRILALPHA` (0x87e3d4, not scripted).
    pub tendril_alpha: u8,
    /// `RENDER_VIEWABLE_DISTANCE_SQR` (`Tweak.script`, squared on load): render-list range of
    /// the chunks, measured from the camera's look-at point.
    pub render_viewable_distance_sqr: f32,
}

impl Default for NebulaTweaks {
    /// The binary's initial values (overridden by the scripts).
    fn default() -> Self {
        NebulaTweaks {
            radius: 100.0,
            fog_red: 0.1,
            fog_green: 0.3,
            fog_blue: 0.7,
            bg_scalar: 0.2,
            tendril_red_dev: 0.05,
            tendril_green_dev: 0.05,
            tendril_blue_dev: 0.1,
            distance_desaturation: 127.0,
            max_tendril_angular_deviation: 45.0,
            max_tendril_distance: 80_000.0,
            pick_distance_threshold: 12_000.0,
            pick_distance_minthresh: 500.0,
            shuffle_distance: 550.0,
            vel_ran_multiplier: 0.4,
            ran_range: 3.0,
            vel_base: 0.4,
            tendril_radius_base: 200.0,
            tendril_radius_range: 400.0,
            tendril_alpha: 163,
            render_viewable_distance_sqr: 16_000.0 * 16_000.0,
        }
    }
}

impl NebulaTweaks {
    /// `Nebulae.script` and `Tweak.script` over the defaults.
    pub fn load(src: &dyn DataSource) -> Self {
        let mut t = NebulaTweaks::default();
        if let Some(lines) = read_lines(src, "Nebulae.script") {
            for (k, v) in lines {
                if let Some(slot) = t.slot(&k) {
                    if let Some(x) = scan_f32(&v) {
                        *slot = x;
                    }
                }
            }
        }
        if let Some(lines) = read_lines(src, "Tweak.script") {
            for (k, v) in lines {
                if k.eq_ignore_ascii_case("RENDER_VIEWABLE_DISTANCE_SQR") {
                    if let Some(x) = scan_f32(&v) {
                        t.render_viewable_distance_sqr = st(ld(x) * ld(x));
                    }
                }
            }
        }
        t
    }

    fn slot(&mut self, key: &str) -> Option<&mut f32> {
        let k = key.to_ascii_uppercase();
        Some(match k.as_str() {
            "NEB_RADIUS" => &mut self.radius,
            "NEB_FOG_RED" => &mut self.fog_red,
            "NEB_FOG_GREEN" => &mut self.fog_green,
            "NEB_FOG_BLUE" => &mut self.fog_blue,
            "NEB_BG_SCALAR" => &mut self.bg_scalar,
            "NEB_TENDRIL_RED_DEV" => &mut self.tendril_red_dev,
            "NEB_TENDRIL_GREEN_DEV" => &mut self.tendril_green_dev,
            "NEB_TENDRIL_BLUE_DEV" => &mut self.tendril_blue_dev,
            "NEB_DISTANCE_DESATURATION" => &mut self.distance_desaturation,
            "NEB_MAX_TENDRIL_ANGULAR_DEVIATION" => &mut self.max_tendril_angular_deviation,
            "NEB_MAX_TENDRIL_DISTANCE" => &mut self.max_tendril_distance,
            "NEB_PICK_DISTANCE_THRESHOLD" => &mut self.pick_distance_threshold,
            "NEB_PICK_DISTANCE_MINTHRESH" => &mut self.pick_distance_minthresh,
            "NEB_SHUFFLE_DISTANCE" => &mut self.shuffle_distance,
            "NEB_VEL_RAN_MULTIPLIER" => &mut self.vel_ran_multiplier,
            "NEB_RAN_RANGE" => &mut self.ran_range,
            "NEB_VEL_BASE" => &mut self.vel_base,
            "NEB_TENDRIL_RADIUS_BASE" => &mut self.tendril_radius_base,
            "NEB_TENDRIL_RADIUS_RANGE" => &mut self.tendril_radius_range,
            _ => return None,
        })
    }

    /// The fog colour (`nebFogColor`, set once by `nebStartup` after the scripts are read).
    pub fn fog_colour(&self) -> [f32; 4] {
        [self.fog_red, self.fog_green, self.fog_blue, 1.0]
    }
}

/// A chunk (`nebChunk`, 0x58 bytes): a node tendrils meet at, and a resource object.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub flags: u32,
    pub num_tendrils: u32,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub position: Vector,
    /// Wobble velocity (per update).
    pub velocity: Vector,
    /// Wobble offset added to the tendril ends at this chunk.
    pub d_pos: Vector,
    /// Attached tendrils (indices into [`Nebula::tendrils`]), in attachment order.
    pub tendrils: [Option<u16>; MAX_TENDRILS_PER_CHUNK],
    pub t_flags: [u32; MAX_TENDRILS_PER_CHUNK],
    /// The resource object (`spaceobj`).
    pub resource: Option<ResourceId>,
}

impl Chunk {
    fn attach(&mut self, tendril: u16, how: u32) {
        let n = self.num_tendrils as usize;
        if n < MAX_TENDRILS_PER_CHUNK {
            self.t_flags[n] = how;
            self.tendrils[n] = Some(tendril);
        }
        self.num_tendrils += 1;
    }

    /// The tendrils actually attached.
    pub fn attached(&self) -> impl Iterator<Item = u16> + '_ {
        self.tendrils.iter().take(self.num_tendrils as usize).flatten().copied()
    }
}

/// One level of detail of a tendril's cylinder (`nebTendrilLOD`): `slices * (stacks + 1)`
/// world-space vertices and unit normals, ring by ring from the tendril's start.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct TendrilLod {
    pub slices: u32,
    pub stacks: u32,
    pub vertices: Vec<Vector>,
    pub normals: Vec<Vector>,
}

/// A tendril (`nebTendril`, 0xfc bytes): one straight segment of a chain between two chunks.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Tendril {
    pub flags: u32,
    /// Start and end chunk (indices into [`Nebula::chunks`]).
    pub a: Option<u16>,
    pub b: Option<u16>,
    pub direction: Vector,
    /// Bounding sphere of the cylinder (zero until it exists).
    pub radius: f32,
    pub midpoint: Vector,
    /// Neighbours in the chain.
    pub prev: Option<u16>,
    pub next: Option<u16>,
    /// The two cylinders (6 slices; 5 stacks with a bulge, and 1 stack).
    pub lods: [TendrilLod; TENDRIL_LODS],
    /// `realColour`: RGBA before distance darkening.
    pub real_colour: [u8; 4],
    /// `fadeFactor`: resource units left in a harvested chunk, as a fraction.
    pub fade_factor: f32,
    pub max_alpha: u8,
}

impl Tendril {
    fn blank() -> Self {
        Tendril {
            flags: 0,
            a: None,
            b: None,
            direction: Vector::ZERO,
            radius: 0.0,
            midpoint: Vector::ZERO,
            prev: None,
            next: None,
            lods: Default::default(),
            real_colour: [0; 4],
            fade_factor: 1.0,
            max_alpha: 0,
        }
    }

    /// Both ends are set.
    pub fn ends(&self) -> Option<(usize, usize)> {
        Some((usize::from(self.a?), usize::from(self.b?)))
    }

    pub fn has_geometry(&self) -> bool {
        self.flags & tendril_flags::GEOMETRY != 0
    }
}

/// One nebula (`nebulae_t`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Nebula {
    pub chunks: Vec<Chunk>,
    pub tendrils: Vec<Tendril>,
}

/// The camera the render list and the tendril selection use (`mrCamera`).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct NebulaCamera {
    pub eye: Vector,
    pub look_at: Vector,
    /// `clipPlaneFar`.
    pub clip_far: f32,
}

/// What `nebSetFog` left for the renderer.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct NebulaFog {
    /// `rndFogOn` (only the software renderers ever enable GL fog with it).
    pub on: bool,
    /// `GL_FOG_DENSITY`.
    pub density: f32,
    /// `nebFogSum`.
    pub sum: f32,
    /// Added to the background colour for the clear colour (`NEB_BG_SCALAR` is 0 in the shipped
    /// script, so this stays zero).
    pub clear_add: [i32; 3],
}

/// All nebulae of the level (`nebNebulae[]`, `numNebulae`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Nebulae {
    pub tweaks: NebulaTweaks,
    tweaks_loaded: bool,
    pub list: Vec<Nebula>,
    /// Set by the frontend before each update; `None` while nothing is rendered.
    pub camera: Option<NebulaCamera>,
    pub fog: NebulaFog,
}

impl Nebulae {
    /// `univSpaceObjInRenderList` for a chunk at `p`: within the viewable distance of the
    /// camera's look-at point.
    pub fn in_render_list(&self, p: Vector) -> bool {
        self.camera.is_some_and(|cam| in_render_list(&self.tweaks, &cam, p))
    }

    /// The nebula and chunk of a chunk resource.
    pub fn chunk_of(&self, r: ResourceId) -> Option<(usize, usize)> {
        self.list.iter().enumerate().find_map(|(n, neb)| neb.chunks.iter().position(|c| c.resource == Some(r)).map(|c| (n, c)))
    }
}

/// Whether a chunk at `p` is in the render list of camera `cam` (`univUpdateRenderList` for a
/// resource: within the viewable distance of the look-at point).
pub fn in_render_list(tw: &NebulaTweaks, cam: &NebulaCamera, p: Vector) -> bool {
    let d = Vector::new(st(ld(cam.look_at.x) - ld(p.x)), st(ld(cam.look_at.y) - ld(p.y)), st(ld(cam.look_at.z) - ld(p.z)));
    d.mag_sq() < tw.render_viewable_distance_sqr
}

// ------------------------------------------------------------------------------- random draws

/// `realRand(n)`: `(ranRandom % (int)(n * 10000)) * 0.0001f`, zero for `n == 0`; a register
/// value.
fn real_rand(randy: &mut Randy, n: f32) -> Ext {
    if eq_or_unordered(ld(n), 0.0) {
        return 0.0;
    }
    let r = randy.random(RAN_NEBULAE);
    let m = ftol_u32(ld(n) * 10_000.0);
    fild_u32(r.checked_rem(m).unwrap_or(0)) * ld(0.0001f32)
}

/// `cloudRandomSphericalPoint`: (1, 0, 0) turned about z, then twice about x, by whole-degree
/// angles from the cloud stream (the sine and cosine passed the other way round).
pub fn random_spherical_point(randy: &mut Randy) -> Vector {
    let angle = |randy: &mut Randy| fild_u32(randy.random(RAN_CLOUDS) % 360) * 0.017_453_293_005_625_408;
    let a = angle(randy);
    let (c, s) = (st(fcos(a)), st(fsin(a)));
    let rz = HMatrix::rot_about_z(s, c);
    let r = rz.mul_hvec(HVector::new(1.0, 0.0, 0.0, 1.0));
    let b = angle(randy);
    let (c, s) = (st(fcos(b)), st(fsin(b)));
    let rx = HMatrix::rot_about_x(s, c);
    let h = rx.mul_hvec(r);
    let r = rx.mul_hvec(h);
    let mut v = r.homogenize();
    v.normalize();
    v
}

/// `chunkRandomVelocity`.
fn random_velocity(randy: &mut Randy, tw: &NebulaTweaks) -> Vector {
    let r = randy.random(RAN_NEBULAE);
    let range = ftol_u32(ld(tw.ran_range));
    let k = st(fild_u32(r.checked_rem(range).unwrap_or(0)) * ld(tw.vel_ran_multiplier) + ld(tw.vel_base));
    let v = random_spherical_point(randy);
    Vector::new(st(ld(k) * ld(v.x)), st(ld(k) * ld(v.y)), st(ld(k) * ld(v.z)))
}

/// `nebInitTendril`: a blank tendril with its colour, the fog colour deviated per channel.
fn init_tendril(t: &mut Tendril, randy: &mut Randy, tw: &NebulaTweaks) {
    *t = Tendril::blank();
    let mut channel = |fog: f32, dev: f32| {
        let r = real_rand(randy, st(ld(dev) + ld(dev)));
        ftol_i32(((r - ld(dev)) + ld(fog)) * 255.0).clamp(0, 255) as u8
    };
    let r = channel(tw.fog_red, tw.tendril_red_dev);
    let g = channel(tw.fog_green, tw.tendril_green_dev);
    let b = channel(tw.fog_blue, tw.tendril_blue_dev);
    t.real_colour = [r, g, b, tw.tendril_alpha];
    t.max_alpha = tw.tendril_alpha;
    t.fade_factor = 1.0;
}

// ------------------------------------------------------------------------------- construction

/// `nebChunkDistance` as the callers inline it: the game's square root of the register sum.
fn chunk_distance(a: Vector, b: Vector) -> Ext {
    let dx = ld(a.x) - ld(b.x);
    let dy = ld(a.y) - ld(b.y);
    let dz = ld(a.z) - ld(b.z);
    fmath_sqrt_double(dx * dx + dy * dy + dz * dz)
}

/// `nebAngularDeviation` (degrees, a register value).
pub fn angular_deviation(new: Vector, a: Vector, b: Vector) -> Ext {
    let mut last = Vector::new(st(ld(b.x) - ld(a.x)), st(ld(b.y) - ld(a.y)), st(ld(b.z) - ld(a.z)));
    let mut next = Vector::new(st(ld(new.x) - ld(b.x)), st(ld(new.y) - ld(b.y)), st(ld(new.z) - ld(b.z)));
    last.normalize();
    next.normalize();
    let dot = (ld(next.z) * ld(last.z) + ld(next.y) * ld(last.y)) + ld(next.x) * ld(last.x);
    let deg = dot.acos() * ld(57.295_78f32);
    if deg < 0.0 {
        -deg
    } else {
        deg
    }
}

/// `nebPickChunk` modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Pick {
    /// `NEB_INITIAL_CHUNK`: the most tendrilled of the first 16 free chunks.
    Initial,
    /// `NEB_MEDIAN_CHUNK` with the previous chunk(s) as constraints.
    Median(Option<usize>, Option<usize>),
}

impl Nebula {
    fn free(&self, i: usize) -> bool {
        let c = &self.chunks[i];
        c.flags & chunk_flags::PICKED == 0 && c.num_tendrils + 1 < MAX_TENDRILS_PER_CHUNK as u32
    }

    /// `nebNumSimilarTendrils`.
    fn similar_tendrils(&self, x: usize, y: usize) -> usize {
        let (x, y) = (Some(x as u16), Some(y as u16));
        self.tendrils.iter().filter(|t| (t.a == x && t.b == y) || (t.a == y && t.b == x)).count()
    }

    fn pick(&self, mode: Pick, tw: &NebulaTweaks) -> Option<usize> {
        const CANDIDATES: usize = 16;
        match mode {
            Pick::Initial => {
                let cands: Vec<usize> = (0..self.chunks.len()).filter(|&i| self.free(i)).take(CANDIDATES).collect();
                let mut best = *cands.first()?;
                for &c in &cands {
                    if self.chunks[c].num_tendrils > self.chunks[best].num_tendrils {
                        best = c;
                    }
                }
                Some(best)
            }
            Pick::Median(None, _) => (0..self.chunks.len()).find(|&i| self.free(i)),
            Pick::Median(Some(a), None) => (0..self.chunks.len()).find(|&i| {
                self.free(i)
                    && lt_or_unordered(chunk_distance(self.chunks[a].position, self.chunks[i].position), ld(tw.pick_distance_threshold))
                    && self.similar_tendrils(i, a) == 0
            }),
            Pick::Median(Some(a), Some(b)) => {
                let (pa, pb) = (self.chunks[a].position, self.chunks[b].position);
                let mut cands = Vec::with_capacity(CANDIDATES);
                for i in 0..self.chunks.len() {
                    if !self.free(i) {
                        continue;
                    }
                    let p = self.chunks[i].position;
                    let d = st(chunk_distance(pb, p));
                    let dev = angular_deviation(p, pa, pb);
                    if lt_or_unordered(dev, ld(tw.max_tendril_angular_deviation))
                        && lt_or_unordered(ld(d), ld(tw.pick_distance_threshold))
                        && ld(d) > ld(tw.pick_distance_minthresh)
                        && self.similar_tendrils(i, b) == 0
                    {
                        cands.push(i);
                        if cands.len() == CANDIDATES {
                            break;
                        }
                    }
                }
                let mut best = None;
                let mut min = 1e20f32;
                for &c in &cands {
                    let d = chunk_distance(self.chunks[c].position, pb);
                    if !eq_or_unordered(d, 0.0) && lt_or_unordered(d, ld(min)) {
                        min = st(d);
                        best = Some(c);
                    }
                }
                best
            }
        }
    }

    fn unpick_all(&mut self) {
        for c in &mut self.chunks {
            c.flags &= !chunk_flags::PICKED;
        }
    }

    /// `nebGetFreshTendril`: the first tendril without ends, initialised.
    fn fresh_tendril(&mut self, randy: &mut Randy, tw: &NebulaTweaks) -> Option<usize> {
        let i = self.tendrils.iter().position(|t| t.a.is_none() && t.b.is_none())?;
        init_tendril(&mut self.tendrils[i], randy, tw);
        Some(i)
    }

    /// `nebTendrilDirection`.
    fn set_direction(&mut self, t: usize) {
        let Some((a, b)) = self.tendrils[t].ends() else { return };
        let (pa, pb) = (self.chunks[a].position, self.chunks[b].position);
        let mut d = Vector::new(st(ld(pb.x) - ld(pa.x)), st(ld(pb.y) - ld(pa.y)), st(ld(pb.z) - ld(pa.z)));
        d.normalize();
        self.tendrils[t].direction = d;
    }

    fn link(&mut self, t: usize, a: usize, b: usize) {
        self.tendrils[t].a = Some(a as u16);
        self.tendrils[t].b = Some(b as u16);
        self.chunks[a].attach(t as u16, CHUNK_FORE);
        self.chunks[b].attach(t as u16, CHUNK_AFT);
        self.set_direction(t);
    }

    /// `nebAttachTendrils`: chains of tendrils until none is left.
    fn attach_tendrils(&mut self, randy: &mut Randy, tw: &NebulaTweaks) {
        'chains: loop {
            self.unpick_all();
            let Some(t) = self.fresh_tendril(randy, tw) else { break };
            // The first two chunks; a start chunk without a partner stays picked.
            let (ca, cb) = loop {
                // (The original dereferences the missing chunk and crashes.)
                let Some(ca) = self.pick(Pick::Initial, tw) else { break 'chains };
                self.chunks[ca].flags |= chunk_flags::PICKED;
                if let Some(cb) = self.pick(Pick::Median(Some(ca), None), tw) {
                    break (ca, cb);
                }
            };
            self.chunks[cb].flags |= chunk_flags::PICKED;
            self.link(t, ca, cb);
            self.tendrils[t].flags |= tendril_flags::LEADING;
            let mut last = t;
            let left0 = ld(tw.max_tendril_distance) - chunk_distance(self.chunks[ca].position, self.chunks[cb].position);
            let mut left = st(left0);
            if 0.0 < left0 {
                let (mut prev, mut cur) = (ca, cb);
                while self.chunks[cur].num_tendrils + 1 < MAX_TENDRILS_PER_CHUNK as u32 {
                    let Some(nb) = self.pick(Pick::Median(Some(prev), Some(cur)), tw) else { break };
                    self.chunks[nb].flags |= chunk_flags::PICKED;
                    left = st(ld(left) - chunk_distance(self.chunks[cur].position, self.chunks[nb].position));
                    let Some(nt) = self.fresh_tendril(randy, tw) else { break };
                    self.tendrils[last].next = Some(nt as u16);
                    self.tendrils[nt].prev = Some(last as u16);
                    self.link(nt, cur, nb);
                    prev = cur;
                    cur = nb;
                    last = nt;
                    if left <= 0.0 {
                        break;
                    }
                }
            }
            self.tendrils[last].flags |= tendril_flags::TRAILING;
        }
        self.unpick_all();
    }

    /// `nebDeactivateEmptyChunksAndFixup` up to the resource creation: drops the chunks without
    /// tendrils (renumbering the tendril ends) and switches off chains of a single segment.
    fn compact(&mut self) {
        let mut map = vec![None; self.chunks.len()];
        let mut kept = Vec::new();
        for (i, c) in self.chunks.iter_mut().enumerate() {
            if c.num_tendrils == 0 {
                c.flags |= chunk_flags::INACTIVE;
            } else {
                map[i] = Some(kept.len() as u16);
                kept.push(c.clone());
            }
        }
        self.chunks = kept;
        for t in &mut self.tendrils {
            t.a = t.a.and_then(|a| map[usize::from(a)]);
            t.b = t.b.and_then(|b| map[usize::from(b)]);
            if t.flags & tendril_flags::LEADING != 0 && t.flags & tendril_flags::TRAILING != 0 {
                t.flags |= tendril_flags::INACTIVE;
            }
        }
    }
}

/// Builds one nebula without touching the universe (the chunk resources are not created yet):
/// `nebAllocateTendrils` (every tendril initialised), `nebDistributeChunks`,
/// `nebAttachTendrils` and the compaction.
pub fn build(randy: &mut Randy, tw: &NebulaTweaks, origin: Vector, dimension: Vector, num_chunks: u32, num_tendrils: u32) -> Nebula {
    let mut neb = Nebula { chunks: Vec::with_capacity(num_chunks as usize), tendrils: vec![Tendril::blank(); num_tendrils as usize] };
    for t in &mut neb.tendrils {
        init_tendril(t, randy, tw);
    }
    let coord = |randy: &mut Randy, d: f32| st(real_rand(randy, d) - ld(d) * 0.5);
    for _ in 0..num_chunks {
        let velocity = random_velocity(randy, tw);
        let x = coord(randy, dimension.x);
        let y = coord(randy, dimension.y);
        let z = coord(randy, dimension.z);
        neb.chunks.push(Chunk {
            flags: 0,
            num_tendrils: 0,
            inner_radius: 160.0,
            outer_radius: 245.0,
            position: Vector::new(st(ld(origin.x) + ld(x)), st(ld(origin.y) + ld(y)), st(ld(origin.z) + ld(z))),
            velocity,
            d_pos: Vector::ZERO,
            tendrils: [None; MAX_TENDRILS_PER_CHUNK],
            t_flags: [0; MAX_TENDRILS_PER_CHUNK],
            resource: None,
        });
    }
    neb.attach_tendrils(randy, tw);
    neb.compact();
    neb
}

/// `nebGo` for a level line: one more nebula over `dimension` (x: `radius`, y: `length`,
/// z: `roty` of the line) around `origin`, its chunks added as resources (`univAddNebula`, in
/// chunk order) with the line's attributes. The first nebula of a level resets the nebula
/// stream, as `nebReset` does when the mission starts.
pub fn neb_go(u: &mut Universe, src: &dyn DataSource, origin: Vector, dimension: Vector, num_chunks: u32, num_tendrils: u32, attributes: u32) {
    if !u.nebulae.tweaks_loaded {
        u.nebulae.tweaks = NebulaTweaks::load(src);
        u.nebulae.tweaks_loaded = true;
    }
    if u.nebulae.list.is_empty() {
        u.randy.parameters_reset(RAN_NEBULAE);
    }
    let tw = u.nebulae.tweaks.clone();
    let mut neb = build(&mut u.randy, &tw, origin, dimension, num_chunks, num_tendrils);
    for c in &mut neb.chunks {
        c.resource = add_chunk_resource(u, c.position, attributes);
    }
    u.nebulae.list.push(neb);
}

/// `univAddNebula` (0x53c770) for a chunk: a `Nebula0` resource with a random orientation.
fn add_chunk_resource(u: &mut Universe, p: Vector, attributes: u32) -> Option<ResourceId> {
    match resource::add_resource(u, ResourceKind::Nebula(0), p) {
        Some(id) => {
            if let Some(r) = u.base.resources.get_mut(id) {
                r.attributes = attributes;
            }
            Some(id)
        }
        None => {
            // No statics: the object number and the orientation are still taken.
            u.base.resources.next_id = u.base.resources.next_id.wrapping_add(1);
            resource::random_orientation(u);
            None
        }
    }
}

// ------------------------------------------------------------------------------- gameplay

/// `nebFadeAttachedTendrils`: harvesting a chunk dims the tendrils attached to it to `factor`
/// (resource units left over the full value).
pub fn fade_attached_tendrils(u: &mut Universe, r: ResourceId, factor: f32) {
    let Some((n, c)) = u.nebulae.chunk_of(r) else { return };
    let neb = &mut u.nebulae.list[n];
    let ts: Vec<u16> = neb.chunks[c].attached().collect();
    for t in ts {
        neb.tendrils[usize::from(t)].fade_factor = factor;
    }
}

/// `nebDeleteChunk`: an exhausted chunk is dead, its tendrils go out, and the neighbouring
/// segments of their chains become the new chain ends.
pub fn delete_chunk(u: &mut Universe, r: ResourceId) {
    let Some((n, c)) = u.nebulae.chunk_of(r) else { return };
    let neb = &mut u.nebulae.list[n];
    neb.chunks[c].flags |= chunk_flags::DEAD;
    let ts: Vec<u16> = neb.chunks[c].attached().collect();
    for t in ts {
        let t = &mut neb.tendrils[usize::from(t)];
        t.flags |= tendril_flags::INACTIVE;
        let (prev, next) = (t.prev, t.next);
        if let Some(p) = prev {
            neb.tendrils[usize::from(p)].flags |= tendril_flags::TRAILING;
        }
        if let Some(x) = next {
            neb.tendrils[usize::from(x)].flags |= tendril_flags::LEADING;
        }
    }
}

// ------------------------------------------------------------------------------- per update

/// `nebSetFog` (0x4c6670), run by `univUpdate`: fog density from the chunks in the render
/// list, which also wobble; then the tendrils the next frame draws get their cylinders.
pub fn set_fog(u: &mut Universe) {
    if u.nebulae.list.is_empty() {
        return;
    }
    let tw = u.nebulae.tweaks.clone();
    let cam = u.nebulae.camera;
    let mut sum = 0.0f32;
    // The render list holds the live chunks within range, in creation order.
    let mut members = Vec::new();
    for (n, neb) in u.nebulae.list.iter().enumerate() {
        for (c, ch) in neb.chunks.iter().enumerate() {
            if ch.flags & chunk_flags::DEAD == 0 && ch.resource.is_some() && u.nebulae.in_render_list(ch.position) {
                members.push((n, c));
            }
        }
    }
    if let Some(cam) = cam {
        for (n, c) in members {
            let ch = &mut u.nebulae.list[n].chunks[c];
            let e = Vector::new(st(ld(cam.eye.x) - ld(ch.position.x)), st(ld(cam.eye.y) - ld(ch.position.y)), st(ld(cam.eye.z) - ld(ch.position.z)));
            let dist = fmath_sqrt_double(ld(e.mag_sq()));
            if !eq_or_unordered(dist, 0.0) {
                sum = st(ld(FOG_CHUNK_RADIUS) / dist + ld(sum));
            }
            wobble(ch, &mut u.randy, &tw);
        }
    }
    let fog_sum = st(ld(sum) * 0.15);
    let mut f = NebulaFog { sum: fog_sum, ..Default::default() };
    let level = if fog_sum <= 0.86 { fog_sum } else { 0.86 };
    if level > 0.1 {
        f.on = true;
        f.density = level;
    }
    if 0.0 < level {
        let mut s = st(ld(level) * 1.5);
        s = if s <= 0.57 { s.max(0.1) } else { 0.57 };
        f.sum = s;
        let add = |c: f32| ftol_i32(ld(c) * ld(s) * ld(tw.bg_scalar) * 255.0);
        f.clear_add = [add(tw.fog_red), add(tw.fog_green), add(tw.fog_blue)];
    }
    u.nebulae.fog = f;
    if let Some(cam) = cam {
        let Universe { nebulae, randy, .. } = u;
        for neb in &mut nebulae.list {
            prepare_geometry(neb, randy, &tw, &cam);
        }
    }
}

/// `nebUpdateChunk`: drift by the velocity; past the slack between the radii, step back and
/// pick a new direction.
fn wobble(ch: &mut Chunk, randy: &mut Randy, tw: &NebulaTweaks) {
    let d = Vector::new(st(ld(ch.velocity.x) + ld(ch.d_pos.x)), st(ld(ch.velocity.y) + ld(ch.d_pos.y)), st(ld(ch.velocity.z) + ld(ch.d_pos.z)));
    ch.d_pos = d;
    let mag = fmath_sqrt_double(ld(d.x) * ld(d.x) + ld(d.y) * ld(d.y) + ld(d.z) * ld(d.z));
    if ld(ch.outer_radius) - ld(ch.inner_radius) < mag {
        ch.d_pos = Vector::new(st(ld(d.x) - ld(ch.velocity.x)), st(ld(d.y) - ld(ch.velocity.y)), st(ld(d.z) - ld(ch.velocity.z)));
        ch.velocity = random_velocity(randy, tw);
    }
}

/// `nebIsClipped` as Cataclysm inlines it: behind the eye relative to the view direction.
pub fn tendril_clipped(t: &Tendril, cam: &NebulaCamera) -> bool {
    let (e, m, l) = (cam.eye, t.midpoint, cam.look_at);
    let dot = (ld(e.x) - ld(m.x)) * (ld(e.x) - ld(l.x)) + (ld(e.y) - ld(m.y)) * (ld(e.y) - ld(l.y)) + (ld(e.z) - ld(m.z)) * (ld(e.z) - ld(l.z));
    !(0.0 <= dot)
}

/// Whether either end chunk is within `1.1 * clipPlaneFar` of the eye.
pub fn tendril_in_range(neb: &Nebula, t: &Tendril, cam: &NebulaCamera) -> bool {
    let Some((a, b)) = t.ends() else { return false };
    let far = st(ld(cam.clip_far) * ld(1.1f32));
    let max = st(ld(far) * ld(far));
    let d2 = |p: Vector| {
        let (x, y, z) = (st(ld(p.x) - ld(cam.eye.x)), st(ld(p.y) - ld(cam.eye.y)), st(ld(p.z) - ld(cam.eye.z)));
        ld(x) * ld(x) + ld(y) * ld(y) + ld(z) * ld(z)
    };
    d2(neb.chunks[a].position) <= ld(max) || d2(neb.chunks[b].position) <= ld(max)
}

/// The tendrils `nebRenderNebula` draws for this camera (active, in front, in range).
pub fn drawn_tendrils<'a>(neb: &'a Nebula, cam: &'a NebulaCamera) -> impl Iterator<Item = usize> + 'a {
    neb.tendrils
        .iter()
        .enumerate()
        .filter(move |(_, t)| t.flags & tendril_flags::INACTIVE == 0 && t.ends().is_some() && !tendril_clipped(t, cam) && tendril_in_range(neb, t, cam))
        .map(|(i, _)| i)
}

/// The geometry part of `nebRenderNebula` / `nebRenderTendril`.
fn prepare_geometry(neb: &mut Nebula, randy: &mut Randy, tw: &NebulaTweaks, cam: &NebulaCamera) {
    let todo: Vec<usize> = drawn_tendrils(neb, cam).filter(|&i| !neb.tendrils[i].has_geometry()).collect();
    for i in todo {
        make_geometry(neb, i, randy, tw);
    }
}

/// `nebRenderTendril`'s first-time set-up: the tendril's frame (forward from end to end,
/// shortened by the shuffle distance at both ends, up = +z), a random radius, and both
/// cylinders.
pub fn make_geometry(neb: &mut Nebula, i: usize, randy: &mut Randy, tw: &NebulaTweaks) {
    let Some((a, b)) = neb.tendrils[i].ends() else { return };
    let dir = neb.tendrils[i].direction;
    let (pa, pb) = (neb.chunks[a].position, neb.chunks[b].position);
    let sh = ld(tw.shuffle_distance);
    let a0 = Vector::new(st(ld(pa.x) + sh * ld(dir.x)), st(ld(pa.y) + sh * ld(dir.y)), st(ld(pa.z) + sh * ld(dir.z)));
    let b0 = Vector::new(st(ld(pb.x) - sh * ld(dir.x)), st(ld(pb.y) - sh * ld(dir.y)), st(ld(pb.z) - sh * ld(dir.z)));
    let mut fwd = Vector::new(st(ld(b0.x) - ld(a0.x)), st(ld(b0.y) - ld(a0.y)), st(ld(b0.z) - ld(a0.z)));
    let length = st(fmath_sqrt_double(ld(fwd.x) * ld(fwd.x) + ld(fwd.y) * ld(fwd.y) + ld(fwd.z) * ld(fwd.z)));
    fwd.normalize();
    let right = fwd.cross(Vector::new(0.0, 0.0, 1.0));
    let hmat = HMatrix {
        m: [right.x, right.y, right.z, 0.0, 0.0, 0.0, 1.0, 0.0, fwd.x, fwd.y, fwd.z, 0.0, a0.x, a0.y, a0.z, 1.0],
    };
    let mut rr = st(real_rand(randy, tw.tendril_radius_range));
    if randy.random(RAN_NEBULAE) & 1 == 0 {
        rr = -rr;
    }
    let radius = st(ld(rr) + ld(tw.tendril_radius_base));
    let t = &mut neb.tendrils[i];
    t.lods[0] = create_cylinder(&hmat, radius, radius, length, 6, 5, randy);
    t.lods[1] = create_cylinder(&hmat, radius, radius, length, 6, 1, randy);
    let (r, m) = bounds(&t.lods[1].vertices);
    t.radius = r;
    t.midpoint = m;
    t.flags |= tendril_flags::GEOMETRY;
}

/// `nebCreateCylinder`: `stacks + 1` rings of `slices` vertices along the frame's z axis; the
/// five-stack version bulges by 10%, 15%, 10% (in or out by a random sign) at rings 1..3.
pub fn create_cylinder(hmat: &HMatrix, base: f32, top: f32, height: f32, slices: u32, stacks: u32, randy: &mut Randy) -> TendrilLod {
    let da = st(ld(6.283_185_5f32) / f64::from(slices));
    let dr = st((ld(top) - ld(base)) / f64::from(slices));
    let dz = st(ld(height) / f64::from(stacks));
    let nz = st((ld(base) - ld(top)) / ld(height));
    let sign: f32 = if randy.random(RAN_NEBULAE) & 1 != 0 { -1.0 } else { 1.0 };
    let m = &hmat.m;
    let mut lod = TendrilLod { slices, stacks, vertices: Vec::new(), normals: Vec::new() };
    let mut z = 0.0f32;
    for j in 0..=stacks {
        let mut r = base;
        for i in 0..slices {
            let ang = f64::from(i) * ld(da);
            let s = st(-fsin(ang));
            let c = st(fcos(ang));
            let bulge = match (stacks, j) {
                (5, 2) => Some(0.15f32),
                (5, 1) | (5, 3) => Some(0.1f32),
                _ => None,
            };
            let (x, y) = match bulge {
                Some(k) => {
                    let rr = st((ld(sign) * ld(k) + 1.0) * ld(r));
                    (st(ld(rr) * ld(s)), st(ld(rr) * ld(c)))
                }
                None => (st(ld(s) * ld(r)), st(ld(c) * ld(r))),
            };
            let v = hmat.mul_hvec(HVector::new(x, y, z, 1.0)).homogenize();
            lod.vertices.push(v);
            let mut n = Vector::new(
                st((ld(nz) * ld(m[2]) + ld(c) * ld(m[1])) + ld(s) * ld(m[0])),
                st((ld(s) * ld(m[4]) + ld(c) * ld(m[5])) + ld(nz) * ld(m[6])),
                st((ld(s) * ld(m[8]) + ld(c) * ld(m[9])) + ld(nz) * ld(m[10])),
            );
            n.normalize();
            lod.normals.push(n);
            r = st(ld(r) + ld(dr));
        }
        z = st(ld(z) + ld(dz));
    }
    lod
}

/// `nebTendrilBounds`: Ritter's bounding sphere of the low-detail cylinder.
pub fn bounds(verts: &[Vector]) -> (f32, Vector) {
    if verts.is_empty() {
        return (0.0, Vector::ZERO);
    }
    let big = 1e9f32;
    let (mut xmin, mut xmax, mut ymin, mut ymax, mut zmin, mut zmax) = (
        Vector::new(big, 0.0, 0.0),
        Vector::new(-big, 0.0, 0.0),
        Vector::new(0.0, big, 0.0),
        Vector::new(0.0, -big, 0.0),
        Vector::new(0.0, 0.0, big),
        Vector::new(0.0, 0.0, -big),
    );
    for &p in verts {
        if p.x < xmin.x {
            xmin = p;
        }
        if p.x > xmax.x {
            xmax = p;
        }
        if p.y < ymin.y {
            ymin = p;
        }
        if p.y > ymax.y {
            ymax = p;
        }
        if p.z < zmin.z {
            zmin = p;
        }
        if p.z > zmax.z {
            zmax = p;
        }
    }
    let span = |a: Vector, b: Vector| {
        let (dx, dy, dz) = (st(ld(b.x) - ld(a.x)), st(ld(b.y) - ld(a.y)), st(ld(b.z) - ld(a.z)));
        st(ld(dx) * ld(dx) + ld(dy) * ld(dy) + ld(dz) * ld(dz))
    };
    let (mut d1, mut d2) = (xmin, xmax);
    let mut max = span(xmin, xmax);
    let ys = span(ymin, ymax);
    if max < ys {
        max = ys;
        d1 = ymin;
        d2 = ymax;
    }
    if max < span(zmin, zmax) {
        d1 = zmin;
        d2 = zmax;
    }
    let mut cen = Vector::new(st((ld(d1.x) + ld(d2.x)) * 0.5), st((ld(d1.y) + ld(d2.y)) * 0.5), st((ld(d1.z) + ld(d2.z)) * 0.5));
    let mut rad_sq = span(cen, d2);
    let mut rad = st(fmath_sqrt_double(ld(rad_sq)));
    for &p in verts {
        let d = span(cen, p);
        if d > rad_sq {
            let old_to_p = st(fmath_sqrt_double(ld(d)));
            rad = st((ld(rad) + ld(old_to_p)) * 0.5);
            rad_sq = st(ld(rad) * ld(rad));
            let old_to_new = st(ld(old_to_p) - ld(rad));
            cen = Vector::new(
                st((ld(rad) * ld(cen.x) + ld(old_to_new) * ld(p.x)) / ld(old_to_p)),
                st((ld(rad) * ld(cen.y) + ld(old_to_new) * ld(p.y)) / ld(old_to_p)),
                st((ld(rad) * ld(cen.z) + ld(old_to_new) * ld(p.z)) / ld(old_to_p)),
            );
        }
    }
    (rad, cen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped_tweaks() -> NebulaTweaks {
        NebulaTweaks {
            fog_red: 1.0,
            fog_green: 0.25,
            fog_blue: 0.4,
            bg_scalar: 0.0,
            tendril_red_dev: 0.1,
            tendril_green_dev: 0.15,
            tendril_blue_dev: 0.2,
            distance_desaturation: 150.0,
            max_tendril_distance: 60_000.0,
            pick_distance_minthresh: 2300.0,
            shuffle_distance: 500.0,
            vel_ran_multiplier: 0.45,
            ran_range: 7.2,
            vel_base: 1.2,
            tendril_radius_base: 900.0,
            tendril_radius_range: 350.0,
            radius: 400.0,
            ..NebulaTweaks::default()
        }
    }

    #[test]
    fn real_rand_matches_the_formula() {
        let mut a = Randy::default();
        let mut b = Randy::default();
        assert_eq!(real_rand(&mut a, 0.0), 0.0);
        assert_eq!(a, Randy::default(), "zero draws nothing");
        let v = real_rand(&mut a, 2.5);
        let r = b.random(RAN_NEBULAE);
        assert_eq!(v, f64::from(r % 25_000) * f64::from(0.0001f32));
        assert!((0.0..2.5).contains(&v));
    }

    #[test]
    fn spherical_points_are_unit_and_draw_two_cloud_numbers() {
        let mut r = Randy::default();
        for _ in 0..50 {
            let before = r.clone();
            let v = random_spherical_point(&mut r);
            // (vecNormalize uses the game's table square root.)
            assert!((v.mag_ext() - 1.0).abs() < 1e-3, "{v:?}");
            let mut b = before.clone();
            b.random(RAN_CLOUDS);
            b.random(RAN_CLOUDS);
            assert_eq!(b, r);
        }
    }

    #[test]
    fn tendril_colours_stay_near_the_fog_colour() {
        let tw = shipped_tweaks();
        let mut r = Randy::default();
        for _ in 0..100 {
            let mut t = Tendril::blank();
            init_tendril(&mut t, &mut r, &tw);
            assert!(t.real_colour[0] >= 229, "red 1.0 +- 0.1 (clamped): {:?}", t.real_colour);
            assert!((26..=103).contains(&t.real_colour[1]), "{:?}", t.real_colour);
            assert!((51..=153).contains(&t.real_colour[2]), "{:?}", t.real_colour);
            assert_eq!(t.real_colour[3], 163);
            assert_eq!(t.fade_factor, 1.0);
        }
    }

    #[test]
    fn angular_deviation_of_a_straight_line_and_a_right_angle() {
        let a = Vector::new(0.0, 0.0, 0.0);
        let b = Vector::new(1000.0, 0.0, 0.0);
        assert!(angular_deviation(Vector::new(2000.0, 0.0, 0.0), a, b) < 1e-3);
        let d = angular_deviation(Vector::new(1000.0, 1000.0, 0.0), a, b);
        assert!((d - 90.0).abs() < 1e-3, "{d}");
    }

    #[test]
    fn a_small_nebula_links_chains_within_the_constraints() {
        let tw = shipped_tweaks();
        let mut r = Randy::default();
        let neb = build(&mut r, &tw, Vector::new(1000.0, 2000.0, 3000.0), Vector::new(37971.9, 37971.9, 113915.7), 2000, 30);
        assert!(!neb.chunks.is_empty());
        for c in &neb.chunks {
            assert!(c.num_tendrils >= 1 && c.num_tendrils <= 2, "{c:?}");
            assert_eq!(c.flags & chunk_flags::PICKED, 0);
        }
        for (i, t) in neb.tendrils.iter().enumerate() {
            let (a, b) = t.ends().expect("every tendril is used");
            let d = chunk_distance(neb.chunks[a].position, neb.chunks[b].position);
            assert!(d < 12_000.0 * 1.01, "tendril {i}: {d}");
            assert!(neb.chunks[a].attached().any(|x| usize::from(x) == i));
            assert!(neb.chunks[b].attached().any(|x| usize::from(x) == i));
            if let Some(p) = t.prev {
                assert_eq!(neb.tendrils[usize::from(p)].next, Some(i as u16));
                assert_eq!(neb.tendrils[usize::from(p)].b, t.a, "chains are continuous");
            } else {
                assert!(t.flags & tendril_flags::LEADING != 0);
            }
            if t.next.is_none() {
                assert!(t.flags & tendril_flags::TRAILING != 0);
            }
            let single = t.prev.is_none() && t.next.is_none();
            assert_eq!(t.flags & tendril_flags::INACTIVE != 0, single);
        }
        // Deterministic.
        let mut r2 = Randy::default();
        let again = build(&mut r2, &tw, Vector::new(1000.0, 2000.0, 3000.0), Vector::new(37971.9, 37971.9, 113915.7), 2000, 30);
        assert_eq!(neb, again);
        assert_eq!(r, r2);
    }

    #[test]
    fn cylinders_have_rings_along_the_frame() {
        let mut r = Randy::default();
        let h = HMatrix { m: [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 10.0, 20.0, 30.0, 1.0] };
        let lod = create_cylinder(&h, 100.0, 100.0, 5000.0, 6, 1, &mut r);
        assert_eq!(lod.vertices.len(), 12);
        for (k, v) in lod.vertices.iter().enumerate() {
            let ring = (k / 6) as f32;
            assert!((v.z - (30.0 + ring * 5000.0)).abs() < 1e-3, "{v:?}");
            let rr = ((v.x - 10.0).powi(2) + (v.y - 20.0).powi(2)).sqrt();
            assert!((rr - 100.0).abs() < 1e-3);
            assert!((lod.normals[k].mag_ext() - 1.0).abs() < 1e-3);
        }
        assert_eq!(lod.vertices[0], Vector::new(10.0, 120.0, 30.0), "first vertex at -sin 0, cos 0");
        let bulgy = create_cylinder(&h, 100.0, 100.0, 5000.0, 6, 5, &mut r);
        assert_eq!(bulgy.vertices.len(), 36);
        let ring_r = |j: usize| (bulgy.vertices[j * 6].y - 20.0).abs();
        assert!((ring_r(0) - 100.0).abs() < 1e-3 && (ring_r(5) - 100.0).abs() < 1e-3);
        assert!((ring_r(2) - 100.0).abs() > 14.9 && (ring_r(1) - 100.0).abs() > 9.9);
        let (rad, mid) = bounds(&lod.vertices);
        assert!((mid.z - 2530.0).abs() < 30.0, "{mid:?}");
        assert!(rad >= 2500.0);
    }

    #[test]
    fn wobble_stays_within_the_slack() {
        let tw = shipped_tweaks();
        let mut r = Randy::default();
        let mut c = Chunk {
            flags: 0,
            num_tendrils: 1,
            inner_radius: 160.0,
            outer_radius: 245.0,
            position: Vector::ZERO,
            velocity: random_velocity(&mut r, &tw),
            d_pos: Vector::ZERO,
            tendrils: [None; 3],
            t_flags: [0; 3],
            resource: None,
        };
        for _ in 0..2000 {
            wobble(&mut c, &mut r, &tw);
            assert!(c.d_pos.mag_ext() <= 85.0 + 10.0);
        }
    }
}
