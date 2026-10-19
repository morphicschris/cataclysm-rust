//! The particle system (`Particle.c`), Cataclysm version.
//!
//! Particle systems are created from the global attribute block ([`Pat`], the original's
//! `pat`) that ETG `set*` functions write into, then advanced by [`PartSystem::update`].
//! Cataclysm's changes over Homeworld 1 are reproduced: 256-byte particles with separate X/Y
//! scale, two new system types (5 = electric bolts, 6 = lens flares), the bolt `turbulance` and
//! `segments` attributes, a length clamp flag, a copy of the spawning effect's velocity per
//! particle, a random offset array, and the dedicated random stream 25.
//!
//! Arithmetic follows the binary's x87 code: values held in registers are `f64`, stores to
//! `float` fields round with [`st`]. The order and the rounding points of `partUpdateSystem`
//! and `partMakePosition` were taken from the disassembly.

use hwc_math::fpu::{ftol_i32, ld, st, Ext};
use hwc_math::random::{Randy, RAN_PARTICLES_CAT, RAN_PARTICLE_STREAM};
use hwc_math::trig::{fcos, fsin};
use hwc_math::{Matrix, Vector};

/// `particleType`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u32)]
pub enum PartType {
    Points = 0,
    Cubes = 1,
    Lines = 2,
    Mesh = 3,
    Billboard = 4,
    /// Cataclysm: jittered line strips ("electric bolts").
    EBolt = 5,
    /// Cataclysm: each particle drives a lens-flare object.
    LensFlare = 6,
}

impl PartType {
    /// `partHeaderSize` of a system of this type.
    pub fn header_size(self) -> u32 {
        match self {
            PartType::Points | PartType::Cubes | PartType::Lines | PartType::LensFlare => 0x1c,
            PartType::Mesh => 0x30,
            PartType::Billboard => 0x50,
            PartType::EBolt => 0x24,
        }
    }
}

/// System flags (`PART_MONOSCALE` ...).
pub mod sysflag {
    pub const MONOSCALE: u32 = 0x1;
    pub const ALPHA: u32 = 0x2;
    pub const WORLDSPACE: u32 = 0x4;
    pub const WORLDSPACE_VELOCITY: u32 = 0x8;
    pub const XYZSCALE: u32 = 0x40;
}

/// Particle flags (`PART_SPECULAR` ...).
pub mod pflag {
    pub const SPECULAR: u32 = 1;
    pub const STIPPLE: u32 = 2;
    pub const NO_DEPTH_WRITE: u32 = 4;
    pub const ADDITIVE: u32 = 8;
    pub const PSEUDO_BILLBOARD: u32 = 16;
    pub const TRUE_BILLBOARD: u32 = 32;
    pub const XYZSCALE: u32 = 64;
}

/// `TR_Invalid`.
pub const TR_INVALID: u32 = 0xffff_ffff;

/// Texture/mesh animation loop markers.
pub const PART_LOOPSTART: u32 = 1;
pub const PART_LOOP: u32 = 2;

/// A pointer the original stored in `pat` or a particle: a location in an effect's constant
/// data or variable block, or the static random offset array.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataRef {
    Const { stat: u32, offset: u32 },
    Var { effect: u32, offset: u32 },
    RandomOffsets,
}

/// Reads the data a [`DataRef`] points at.
pub trait DataSource {
    /// The dword at `byte_offset` past the reference (0 when unavailable).
    fn dword(&self, r: DataRef, byte_offset: u32) -> u32;
}

/// Number of entries in the static random offset array (0x8f5a40..0x8f5ef0).
pub const RANDOM_OFFSET_COUNT: usize = 100;

/// `particleAttribs` (`pat`): the attributes the next created system will use.
#[derive(Clone, Debug, PartialEq)]
pub struct Pat {
    pub world_vel: Vector,
    /// Cataclysm: second copy of the spawning effect's velocity (copied into each particle).
    pub effect_vel: Vector,
    pub flags: u32,
    pub drag: f32,
    pub scale: f32,
    pub xyz_scale: [f32; 3],
    pub scale_dist: f32,
    pub delta_scale: f32,
    pub delta_scale_dist: f32,
    pub offset_lof: f32,
    pub offset_r: f32,
    pub offset_theta: f32,
    pub offset_array: Option<DataRef>,
    pub delta_lof: f32,
    pub delta_lof_dist: f32,
    pub delta_r: [f32; 2],
    pub delta_r_dist: [f32; 2],
    pub vel_lof: f32,
    pub vel_lof_dist: f32,
    pub vel_r: f32,
    pub vel_r_dist: f32,
    pub delta_vel_lof: f32,
    pub delta_vel_lof_dist: f32,
    pub delta_vel_r: f32,
    pub delta_vel_r_dist: f32,
    pub ang: f32,
    pub ang_dist: f32,
    pub ang_delta: f32,
    pub ang_delta_dist: f32,
    pub length: f32,
    pub length_dist: f32,
    pub delta_length: f32,
    pub delta_length_dist: f32,
    /// Cataclysm field at 0x8f590c, copied to particles; cleared by `setDeltaLength`.
    pub clamp_length: u32,
    pub turbulance: f32,
    pub segments: u16,
    pub icolor: [f32; 4],
    pub color_dist: [f32; 4],
    pub delta_color: [f32; 4],
    pub delta_color_dist: [f32; 4],
    pub lit: u8,
    pub illum: f32,
    pub illum_dist: f32,
    pub delta_illum: f32,
    pub delta_illum_dist: f32,
    pub lifespan: f32,
    pub lifespan_dist: f32,
    pub waitspan: f32,
    pub waitspan_dist: f32,
    pub mesh: u32,
    pub exponent: f32,
    pub exponent_dist: f32,
    pub color_scheme: u8,
    pub loop_count: i32,
    pub tex: u32,
    pub position: Vector,
    /// The coordinate system of the effect that set it (the original keeps a pointer).
    pub mat: Matrix,
    pub slices: u32,
    pub uv: [[u32; 2]; 2],
    pub tstruct: Option<DataRef>,
    pub mstruct: Option<DataRef>,
    pub rate: f32,
    pub mesh_rate: f32,
    pub start_mesh_frame: f32,
    pub loop_flag: u8,
    pub start_frame: u16,
    pub tumble: [f32; 3],
    pub tumble_dist: [f32; 3],
    pub delta_tumble: [f32; 3],
    pub delta_tumble_dist: [f32; 3],
    pub bias: [f32; 3],
    pub bias_dist: [f32; 3],
    pub specular_only: u32,
    pub stipple_alpha: u32,
    pub no_depth_write: u32,
    pub additive_blends: u32,
    pub pseudo_billboard: u32,
    pub true_billboard: u32,
}

impl Pat {
    /// All zero: the state of the global before the first `setDefaults()`.
    pub fn zeroed() -> Self {
        Self {
            world_vel: Vector::ZERO,
            effect_vel: Vector::ZERO,
            flags: 0,
            drag: 0.0,
            scale: 0.0,
            xyz_scale: [0.0; 3],
            scale_dist: 0.0,
            delta_scale: 0.0,
            delta_scale_dist: 0.0,
            offset_lof: 0.0,
            offset_r: 0.0,
            offset_theta: 0.0,
            offset_array: None,
            delta_lof: 0.0,
            delta_lof_dist: 0.0,
            delta_r: [0.0; 2],
            delta_r_dist: [0.0; 2],
            vel_lof: 0.0,
            vel_lof_dist: 0.0,
            vel_r: 0.0,
            vel_r_dist: 0.0,
            delta_vel_lof: 0.0,
            delta_vel_lof_dist: 0.0,
            delta_vel_r: 0.0,
            delta_vel_r_dist: 0.0,
            ang: 0.0,
            ang_dist: 0.0,
            ang_delta: 0.0,
            ang_delta_dist: 0.0,
            length: 0.0,
            length_dist: 0.0,
            delta_length: 0.0,
            delta_length_dist: 0.0,
            clamp_length: 0,
            turbulance: 0.0,
            segments: 0,
            icolor: [0.0; 4],
            color_dist: [0.0; 4],
            delta_color: [0.0; 4],
            delta_color_dist: [0.0; 4],
            lit: 0,
            illum: 0.0,
            illum_dist: 0.0,
            delta_illum: 0.0,
            delta_illum_dist: 0.0,
            lifespan: 0.0,
            lifespan_dist: 0.0,
            waitspan: 0.0,
            waitspan_dist: 0.0,
            mesh: 0,
            exponent: 0.0,
            exponent_dist: 0.0,
            color_scheme: 0,
            loop_count: 0,
            tex: 0,
            position: Vector::ZERO,
            mat: Matrix { m: [0.0; 9] },
            slices: 0,
            uv: [[0; 2]; 2],
            tstruct: None,
            mstruct: None,
            rate: 0.0,
            mesh_rate: 0.0,
            start_mesh_frame: 0.0,
            loop_flag: 0,
            start_frame: 0,
            tumble: [0.0; 3],
            tumble_dist: [0.0; 3],
            delta_tumble: [0.0; 3],
            delta_tumble_dist: [0.0; 3],
            bias: [0.0; 3],
            bias_dist: [0.0; 3],
            specular_only: 0,
            stipple_alpha: 0,
            no_depth_write: 0,
            additive_blends: 0,
            pseudo_billboard: 0,
            true_billboard: 0,
        }
    }

    /// `partSetDefaults`: the default block at 0x883238 (0x72 dwords), which also resets the
    /// velocities, position and the coordinate-system pointer.
    pub fn defaults() -> Self {
        let mut p = Self::zeroed();
        p.drag = 1.0;
        p.scale = 5.0;
        p.xyz_scale = [1.0; 3];
        p.delta_r = [0.0, hwc_math::TWOPI];
        p.delta_r_dist = [0.0, hwc_math::TWOPI];
        p.length = 5.0;
        p.turbulance = 0.8;
        p.segments = 16;
        p.icolor = [1.0; 4];
        p.lifespan = 2.0;
        p.exponent = -1.0;
        p.loop_count = -1;
        p.tex = TR_INVALID;
        p.slices = 8;
        p.uv = [[0, 0], [1, 1]];
        p.rate = 10.0;
        p.mesh_rate = 5.0;
        p.loop_flag = 1;
        p.additive_blends = 1;
        p
    }
}

impl Pat {
    /// Build from the raw 0x72-dword image of the original's `pat` (0x8f5868). Pointer
    /// fields are not representable and are left empty; `mat` supplies the matrix the
    /// coordinate-system pointer refers to.
    pub fn from_raw(w: &[u32], mat: Matrix) -> Self {
        let fl = |i: usize| f32::from_bits(w[i]);
        let v3 = |i: usize| Vector::new(fl(i), fl(i + 1), fl(i + 2));
        let a3 = |i: usize| [fl(i), fl(i + 1), fl(i + 2)];
        let a4 = |i: usize| [fl(i), fl(i + 1), fl(i + 2), fl(i + 3)];
        Pat {
            world_vel: v3(0),
            effect_vel: v3(3),
            flags: w[6],
            drag: fl(7),
            scale: fl(8),
            xyz_scale: a3(9),
            scale_dist: fl(12),
            delta_scale: fl(13),
            delta_scale_dist: fl(14),
            offset_lof: fl(15),
            offset_r: fl(16),
            offset_theta: fl(17),
            offset_array: None,
            delta_lof: fl(19),
            delta_lof_dist: fl(20),
            delta_r: [fl(21), fl(22)],
            delta_r_dist: [fl(23), fl(24)],
            vel_lof: fl(25),
            vel_lof_dist: fl(26),
            vel_r: fl(27),
            vel_r_dist: fl(28),
            delta_vel_lof: fl(29),
            delta_vel_lof_dist: fl(30),
            delta_vel_r: fl(31),
            delta_vel_r_dist: fl(32),
            ang: fl(33),
            ang_dist: fl(34),
            ang_delta: fl(35),
            ang_delta_dist: fl(36),
            length: fl(37),
            length_dist: fl(38),
            delta_length: fl(39),
            delta_length_dist: fl(40),
            clamp_length: w[41],
            turbulance: fl(42),
            segments: (w[43] & 0xffff) as u16,
            icolor: a4(44),
            color_dist: a4(48),
            delta_color: a4(52),
            delta_color_dist: a4(56),
            lit: (w[60] & 0xff) as u8,
            illum: fl(61),
            illum_dist: fl(62),
            delta_illum: fl(63),
            delta_illum_dist: fl(64),
            lifespan: fl(65),
            lifespan_dist: fl(66),
            waitspan: fl(67),
            waitspan_dist: fl(68),
            mesh: w[69],
            exponent: fl(70),
            exponent_dist: fl(71),
            color_scheme: (w[72] & 0xff) as u8,
            loop_count: w[73] as i32,
            tex: w[74],
            position: v3(75),
            mat,
            slices: w[79],
            uv: [[w[80], w[81]], [w[82], w[83]]],
            tstruct: None,
            mstruct: None,
            rate: fl(86),
            mesh_rate: fl(87),
            start_mesh_frame: fl(88),
            loop_flag: (w[89] & 0xff) as u8,
            start_frame: (w[89] >> 16) as u16,
            tumble: a3(90),
            tumble_dist: a3(93),
            delta_tumble: a3(96),
            delta_tumble_dist: a3(99),
            bias: a3(102),
            bias_dist: a3(105),
            specular_only: w[108],
            stipple_alpha: w[109],
            no_depth_write: w[110],
            additive_blends: w[111],
            pseudo_billboard: w[112],
            true_billboard: w[113],
        }
    }
}

impl Particle {
    /// The raw 64-dword record (pointer fields as 0, flags/bytes packed as in the original).
    pub fn to_raw(&self) -> [u32; 64] {
        let mut w = [0u32; 64];
        let b = |v: f32| v.to_bits();
        let put3 = |w: &mut [u32; 64], i: usize, v: Vector| {
            w[i] = v.x.to_bits();
            w[i + 1] = v.y.to_bits();
            w[i + 2] = v.z.to_bits();
        };
        w[0] = b(self.lifespan);
        w[1] = b(self.waitspan);
        w[2] = b(self.vel_lof);
        w[3] = b(self.vel_r);
        w[4] = b(self.delta_vel_lof);
        w[5] = b(self.delta_vel_r);
        put3(&mut w, 6, self.rvec);
        put3(&mut w, 9, self.position);
        for i in 0..4 {
            w[12 + i] = b(self.icolor[i]);
            w[16 + i] = b(self.delta_color[i]);
        }
        w[20] = b(self.scale_x);
        w[21] = b(self.scale_y);
        w[22] = b(self.delta_scale);
        w[23] = u32::from(self.lit);
        w[24] = b(self.illum);
        w[25] = b(self.delta_illum);
        w[26] = b(self.length);
        w[27] = b(self.delta_length);
        w[28] = self.clamp_length;
        w[29] = b(self.turbulance);
        w[30] = u32::from(self.segments);
        w[32] = self.tex;
        w[33] = b(self.rot);
        w[34] = b(self.delta_rot);
        put3(&mut w, 35, self.w_vel);
        put3(&mut w, 38, self.effect_vel);
        put3(&mut w, 41, self.w_accel);
        for i in 0..3 {
            w[44 + i] = b(self.bias[i]);
            w[51 + i] = b(self.tumble[i]);
            w[54 + i] = b(self.delta_tumble[i]);
        }
        w[48] = b(self.rate);
        w[49] = u32::from(self.loop_flag);
        w[50] = b(self.current_frame);
        w[57] = self.mesh;
        w[58] = b(self.mesh_rate);
        w[60] = b(self.mesh_frame);
        w[61] = b(self.exponent);
        w[62] = u32::from(self.color_scheme) | (u32::from(self.loop_count as u8) << 8);
        w[63] = self.flags;
        w
    }

    /// Rebuild from a raw record (pointer fields become empty).
    pub fn from_raw(w: &[u32; 64]) -> Self {
        let fl = |i: usize| f32::from_bits(w[i]);
        let v3 = |i: usize| Vector::new(fl(i), fl(i + 1), fl(i + 2));
        Particle {
            lifespan: fl(0),
            waitspan: fl(1),
            vel_lof: fl(2),
            vel_r: fl(3),
            delta_vel_lof: fl(4),
            delta_vel_r: fl(5),
            rvec: v3(6),
            position: v3(9),
            icolor: [fl(12), fl(13), fl(14), fl(15)],
            delta_color: [fl(16), fl(17), fl(18), fl(19)],
            scale_x: fl(20),
            scale_y: fl(21),
            delta_scale: fl(22),
            lit: (w[23] & 0xff) as u8,
            illum: fl(24),
            delta_illum: fl(25),
            length: fl(26),
            delta_length: fl(27),
            clamp_length: w[28],
            turbulance: fl(29),
            segments: (w[30] & 0xffff) as u16,
            lens_flare: None,
            tex: w[32],
            rot: fl(33),
            delta_rot: fl(34),
            w_vel: v3(35),
            effect_vel: v3(38),
            w_accel: v3(41),
            bias: [fl(44), fl(45), fl(46)],
            tstruct: None,
            rate: fl(48),
            loop_flag: (w[49] & 0xff) as u8,
            current_frame: fl(50),
            tumble: [fl(51), fl(52), fl(53)],
            delta_tumble: [fl(54), fl(55), fl(56)],
            mesh: w[57],
            mesh_rate: fl(58),
            mstruct: None,
            mesh_frame: fl(60),
            exponent: fl(61),
            color_scheme: (w[62] & 0xff) as u8,
            loop_count: ((w[62] >> 8) & 0xff) as u8 as i8,
            flags: w[63],
        }
    }
}

/// One particle (the 0x100-byte record).
#[derive(Clone, Debug, PartialEq)]
pub struct Particle {
    pub lifespan: f32,
    pub waitspan: f32,
    pub vel_lof: f32,
    pub vel_r: f32,
    pub delta_vel_lof: f32,
    pub delta_vel_r: f32,
    pub rvec: Vector,
    pub position: Vector,
    pub icolor: [f32; 4],
    pub delta_color: [f32; 4],
    pub scale_x: f32,
    pub scale_y: f32,
    pub delta_scale: f32,
    pub lit: u8,
    pub illum: f32,
    pub delta_illum: f32,
    pub length: f32,
    pub delta_length: f32,
    /// Non-zero: `length` is clamped at zero while it changes.
    pub clamp_length: u32,
    pub turbulance: f32,
    pub segments: u16,
    /// Lens-flare object index for type-6 systems.
    pub lens_flare: Option<usize>,
    pub tex: u32,
    pub rot: f32,
    pub delta_rot: f32,
    pub w_vel: Vector,
    pub effect_vel: Vector,
    pub w_accel: Vector,
    pub bias: [f32; 3],
    pub tstruct: Option<DataRef>,
    pub rate: f32,
    pub loop_flag: u8,
    pub current_frame: f32,
    pub tumble: [f32; 3],
    pub delta_tumble: [f32; 3],
    pub mesh: u32,
    pub mesh_rate: f32,
    pub mstruct: Option<DataRef>,
    pub mesh_frame: f32,
    pub exponent: f32,
    pub color_scheme: u8,
    pub loop_count: i8,
    pub flags: u32,
}

/// A lens-flare object driven by a type-6 particle (the 0x40-byte record linked into the
/// lens-flare list).
#[derive(Clone, Debug, PartialEq)]
pub struct LensFlare {
    pub position: Vector,
    /// Field +0xc (1.0 at creation).
    pub intensity: f32,
    /// Field +0x10.
    pub enabled: u8,
    /// Field +0x11, cleared every time the owning system is "rendered".
    pub pending: u8,
    /// Field +0x12.
    pub active: u8,
}

/// A particle system.
#[derive(Clone, Debug, PartialEq)]
pub struct PartSystem {
    pub kind: PartType,
    pub flags: u32,
    pub is_alive: bool,
    pub last_updated: f32,
    pub drag: f32,
    pub ang: f32,
    pub ang_delta: f32,
    /// Billboard/mesh: system texture.
    pub tex: u32,
    /// Billboard: texture rectangle in texels `[[u0, v0], [u1, v1]]`.
    pub uv: [[u32; 2]; 2],
    /// Billboard: the position used for billboarding; mesh: `partPos`.
    pub position: Vector,
    pub slices: u16,
    /// The coordinate system pointer captured at creation (`partMat`).
    pub mat: Matrix,
    /// EBolt header copy of `turbulance` and `segments`.
    pub bolt_turbulance: f32,
    pub bolt_segments: u16,
    pub particles: Vec<Particle>,
}

/// `partRealDist` as a register value: `n ± r·d` with `r` a random fraction (stream 25).
pub fn real_dist(randy: &mut Randy, n: f32, d: f32) -> Ext {
    let r = st(f64::from(randy.random(RAN_PARTICLES_CAT) % 1_000_000) * 1e-6);
    let sign_rand = randy.random(RAN_PARTICLES_CAT);
    let mut sign: Ext = if sign_rand & 1 == 0 { -1.0 } else { 1.0 };
    if hwc_math::fpu::lt_or_unordered(ld(d), 0.0) {
        sign = -1.0;
    }
    sign * ld(r) * ld(d) + ld(n)
}

/// `partRealDist` stored to a float.
pub fn dist(randy: &mut Randy, n: f32, d: f32) -> f32 {
    st(real_dist(randy, n, d))
}

fn clamp_neg1(v: f32) -> f32 {
    if !(-1.0..=1.0).contains(&v) {
        if v > 1.0 {
            1.0
        } else {
            -1.0
        }
    } else {
        v
    }
}

/// `partColorDist`: three randomised channels clamped to `[-1, 1]`.
pub fn color_dist3(randy: &mut Randy, c: &[f32; 4], d: [f32; 3]) -> [f32; 3] {
    let r = dist(randy, c[0], d[0]);
    let g = dist(randy, c[1], d[1]);
    let b = dist(randy, c[2], d[2]);
    [clamp_neg1(r), clamp_neg1(g), clamp_neg1(b)]
}

/// `partMakePosition`: a cylindrical offset. Returns `(position, rvec)`.
#[allow(clippy::too_many_arguments)]
pub fn make_position(
    randy: &mut Randy,
    offset_lof: f32,
    offset_r: f32,
    offset_theta: f32,
    delta_lof: f32,
    delta_lof_dist: f32,
    delta_r: f32,
    delta_r_dist: f32,
    delta_theta: f32,
    delta_theta_dist: f32,
) -> (Vector, Vector) {
    let d_lof = dist(randy, delta_lof, delta_lof_dist);
    let d_r = dist(randy, delta_r, delta_r_dist);
    let theta = real_dist(randy, delta_theta, delta_theta_dist) + ld(offset_theta);
    let radius = st(ld(offset_r) + ld(d_r));
    let c = st(fcos(theta));
    let s = st(fsin(theta));
    // matMakeRotAboutZ(&m, sinTheta, cosTheta): the arguments are swapped in the original.
    let m = Matrix::rot_about_z(s, c);
    let xyz = m.mul_vec(Vector::new(radius, 0.0, 0.0));
    let rvec = m.mul_vec(Vector::new(1.0, 0.0, 0.0));
    let z = st((ld(offset_lof) + ld(d_lof)) + ld(xyz.z));
    (Vector::new(xyz.x, xyz.y, z), rvec)
}

fn read_vec(src: &dyn DataSource, r: DataRef, i: u32) -> Vector {
    let f = |k: u32| f32::from_bits(src.dword(r, i * 12 + k * 4));
    Vector::new(f(0), f(1), f(2))
}

/// Everything particle creation needs besides `pat`.
pub struct CreateEnv<'a> {
    pub randy: &'a mut Randy,
    pub data: &'a dyn DataSource,
    pub lens_flares: &'a mut Vec<LensFlare>,
}

impl PartSystem {
    /// The system header for a new system of `kind` with `n` particles (`partCreationHelper`
    /// before the fill).
    pub fn new_header(kind: PartType, pat: &Pat, now: f32) -> Self {
        let mut s = PartSystem {
            kind,
            flags: pat.flags,
            is_alive: true,
            last_updated: now,
            drag: pat.drag,
            ang: pat.ang,
            ang_delta: pat.ang_delta,
            tex: TR_INVALID,
            uv: [[0; 2]; 2],
            position: Vector::ZERO,
            slices: 0,
            mat: pat.mat,
            bolt_turbulance: 0.0,
            bolt_segments: 0,
            particles: Vec::new(),
        };
        match kind {
            PartType::Mesh => {
                s.tex = pat.tex;
                s.position = pat.position;
            }
            PartType::Billboard => {
                s.tex = pat.tex;
                s.slices = pat.slices as u16;
                s.uv = pat.uv;
                s.position = pat.position;
            }
            PartType::EBolt => {
                s.bolt_turbulance = pat.turbulance;
                s.bolt_segments = pat.segments;
            }
            _ => {}
        }
        s
    }

    /// Number of particles.
    pub fn len(&self) -> usize {
        self.particles.len()
    }

    /// Whether the system has no particles.
    pub fn is_empty(&self) -> bool {
        self.particles.is_empty()
    }
}

/// One particle from the current `pat` (the body of `partFillGenericParticles`).
pub fn make_particle(pat: &Pat, kind: PartType, index: u32, env: &mut CreateEnv) -> Particle {
    let alpha = pat.flags & sysflag::ALPHA != 0;
    let world = pat.flags & sysflag::WORLDSPACE != 0;
    let randy = &mut *env.randy;
    let mut flags = 0;
    if pat.specular_only != 0 {
        flags = pflag::SPECULAR;
    }
    if pat.stipple_alpha != 0 {
        flags |= pflag::STIPPLE;
    }
    if pat.additive_blends != 0 {
        flags |= pflag::ADDITIVE;
    }
    if pat.pseudo_billboard != 0 {
        flags |= pflag::PSEUDO_BILLBOARD;
    }
    if pat.true_billboard != 0 {
        flags |= pflag::TRUE_BILLBOARD;
    }
    if pat.no_depth_write != 0 {
        flags |= pflag::NO_DEPTH_WRITE;
    }
    let rot = dist(randy, pat.ang, pat.ang_dist);
    let delta_rot = dist(randy, pat.ang_delta, pat.ang_delta_dist);
    let (mut position, rvec0) = make_position(
        randy,
        pat.offset_lof,
        pat.offset_r,
        pat.offset_theta,
        pat.delta_lof,
        pat.delta_lof_dist,
        pat.delta_r[0],
        pat.delta_r_dist[0],
        pat.delta_r[1],
        pat.delta_r_dist[1],
    );
    if let Some(arr) = pat.offset_array {
        let o = read_vec(env.data, arr, index);
        position = Vector::new(st(ld(o.x) + ld(position.x)), st(ld(o.y) + ld(position.y)), st(ld(o.z) + ld(position.z)));
    }
    let mut rvec = rvec0;
    rvec.normalize();
    let delta_vel_lof = dist(randy, pat.delta_vel_lof, pat.delta_vel_lof_dist);
    let delta_vel_r = dist(randy, pat.delta_vel_r, pat.delta_vel_r_dist);
    let vel_lof = dist(randy, pat.vel_lof, pat.vel_lof_dist);
    let vel_r = dist(randy, pat.vel_r, pat.vel_r_dist);
    let scale = dist(randy, pat.scale, pat.scale_dist);
    let delta_scale = dist(randy, pat.delta_scale, pat.delta_scale_dist);
    let mut icolor = [0.0f32; 4];
    let mut delta_color = [0.0f32; 4];
    if alpha {
        let c = color_dist3(randy, &pat.icolor, [pat.color_dist[0], pat.color_dist[1], pat.color_dist[2]]);
        icolor[..3].copy_from_slice(&c);
        icolor[3] = clamp_neg1(dist(randy, pat.icolor[3], pat.color_dist[3]));
        let d = color_dist3(
            randy,
            &pat.delta_color,
            [pat.delta_color_dist[0], pat.delta_color_dist[1], pat.delta_color_dist[2]],
        );
        delta_color[..3].copy_from_slice(&d);
        delta_color[3] = clamp_neg1(dist(randy, pat.delta_color[3], pat.delta_color_dist[3]));
    } else {
        let c = color_dist3(randy, &pat.icolor, [pat.color_dist[0], pat.color_dist[1], pat.color_dist[2]]);
        icolor[..3].copy_from_slice(&c);
        icolor[3] = 1.0;
        let d = color_dist3(
            randy,
            &pat.delta_color,
            [pat.delta_color_dist[0], pat.delta_color_dist[1], pat.delta_color_dist[2]],
        );
        delta_color[..3].copy_from_slice(&d);
        // deltaColor[3] is left as whatever the allocation held; zero here.
    }
    let illum = dist(randy, pat.illum, pat.illum_dist);
    let delta_illum = dist(randy, pat.delta_illum, pat.delta_illum_dist);
    let exponent = dist(randy, pat.exponent, pat.exponent_dist);
    let lifespan = dist(randy, pat.lifespan, pat.lifespan_dist);
    let waitspan = dist(randy, pat.waitspan, pat.waitspan_dist);
    let length = dist(randy, pat.length, pat.length_dist);
    let delta_length = dist(randy, pat.delta_length, pat.delta_length_dist);
    let lens_flare = if kind == PartType::LensFlare {
        env.lens_flares.push(LensFlare { position: Vector::ZERO, intensity: 1.0, enabled: 1, pending: 1, active: 1 });
        Some(env.lens_flares.len() - 1)
    } else {
        None
    };
    let bias = [
        dist(randy, pat.bias[0], pat.bias_dist[0]),
        dist(randy, pat.bias[1], pat.bias_dist[1]),
        dist(randy, pat.bias[2], pat.bias_dist[2]),
    ];
    let mut tumble = [
        dist(randy, pat.tumble[0], pat.tumble_dist[0]),
        dist(randy, pat.tumble[1], pat.tumble_dist[1]),
        dist(randy, pat.tumble[2], pat.tumble_dist[2]),
    ];
    let delta_tumble = [
        dist(randy, pat.delta_tumble[0], pat.delta_tumble_dist[0]),
        dist(randy, pat.delta_tumble[1], pat.delta_tumble_dist[1]),
        dist(randy, pat.delta_tumble[2], pat.delta_tumble_dist[2]),
    ];
    if pat.flags & sysflag::XYZSCALE != 0 {
        flags |= pflag::XYZSCALE;
        tumble = pat.xyz_scale;
    } else {
        flags &= !pflag::XYZSCALE;
    }
    let mut p = Particle {
        lifespan,
        waitspan,
        vel_lof,
        vel_r,
        delta_vel_lof,
        delta_vel_r,
        rvec,
        position,
        icolor,
        delta_color,
        scale_x: scale,
        scale_y: scale,
        delta_scale,
        lit: pat.lit,
        illum,
        delta_illum,
        length,
        delta_length,
        clamp_length: pat.clamp_length,
        turbulance: pat.turbulance,
        segments: pat.segments,
        lens_flare,
        tex: TR_INVALID,
        rot,
        delta_rot,
        w_vel: Vector::ZERO,
        effect_vel: Vector::ZERO,
        w_accel: Vector::ZERO,
        bias,
        tstruct: pat.tstruct,
        rate: pat.rate,
        loop_flag: pat.loop_flag,
        current_frame: f32::from(pat.start_frame),
        tumble,
        delta_tumble,
        mesh: pat.mesh,
        mesh_rate: pat.mesh_rate,
        mstruct: pat.mstruct,
        mesh_frame: pat.start_mesh_frame,
        exponent,
        color_scheme: pat.color_scheme,
        loop_count: pat.loop_count as i8,
        flags,
    };
    if world {
        let m = pat.mat;
        let (rx, ry, rz) = (ld(p.rvec.x), ld(p.rvec.y), ld(p.rvec.z));
        let vel = Vector::new(st(ld(p.vel_r) * rx), st(ld(p.vel_r) * ry), st(ld(p.vel_r) * rz + ld(p.vel_lof)));
        let acc = Vector::new(
            st(ld(p.delta_vel_r) * rx),
            st(ld(p.delta_vel_r) * ry),
            st(ld(p.delta_vel_r) * rz + ld(p.delta_vel_lof)),
        );
        let wp = m.mul_vec(p.position);
        p.position = Vector::new(
            st(ld(wp.x) + ld(pat.position.x)),
            st(ld(wp.y) + ld(pat.position.y)),
            st(ld(wp.z) + ld(pat.position.z)),
        );
        let wv = m.mul_vec(vel);
        p.w_vel = Vector::new(
            st(ld(wv.x) + ld(pat.world_vel.x)),
            st(ld(wv.y) + ld(pat.world_vel.y)),
            st(ld(wv.z) + ld(pat.world_vel.z)),
        );
        p.effect_vel = pat.effect_vel;
        p.w_accel = m.mul_vec(acc);
        // The transformed rvec is computed and discarded; rvec is only re-normalised.
        p.rvec.normalize();
    }
    p
}

/// `partColorDelta`.
fn color_delta(c: &mut [f32; 4], d: &[f32; 4], sys_flags: u32, dt: f32) {
    for i in 0..3 {
        if d[i] != 0.0 {
            let v = ld(dt) * ld(d[i]) + ld(c[i]);
            c[i] = if v >= 0.0 { st(if v > 1.0 { 1.0 } else { v }) } else { 0.0 };
        }
    }
    if sys_flags & sysflag::ALPHA != 0 && d[3] != 0.0 {
        let v = ld(dt) * ld(d[3]) + ld(c[3]);
        c[3] = if v < 0.0 || v.is_nan() { 0.0 } else { st(if v > 1.0 { 1.0 } else { v }) };
    }
}

/// A texture animation frame: texture handle, UV bytes and loop flags.
fn tex_frame(src: &dyn DataSource, r: DataRef, frame: i32) -> (u32, [u32; 4], u32) {
    let base = (frame as u32).wrapping_mul(12);
    let tex = src.dword(r, base);
    let uv = src.dword(r, base + 4);
    let flags = src.dword(r, base + 8);
    (tex, [uv & 0xff, (uv >> 8) & 0xff, (uv >> 16) & 0xff, uv >> 24], flags)
}

/// `partAdvanceTexAnim`.
fn advance_tex_anim(src: &dyn DataSource, p: &Particle) -> i32 {
    let r = p.tstruct.expect("tex anim");
    let cur = ftol_i32(ld(p.current_frame));
    if p.loop_flag != 0 && tex_frame(src, r, cur).2 == PART_LOOP {
        let mut frame = 0;
        loop {
            let (tex, _, fl) = tex_frame(src, r, frame);
            if fl == PART_LOOPSTART {
                return frame;
            }
            if tex == 0xffff_ffff {
                return -1;
            }
            frame += 1;
            if frame > 4096 {
                return -1;
            }
        }
    }
    let next = cur + 1;
    if tex_frame(src, r, next).0 == 0xffff_ffff {
        if p.loop_flag == 0 {
            cur
        } else {
            0
        }
    } else {
        next
    }
}

/// `partAdvanceMeshMorph`.
fn advance_mesh_morph(src: &dyn DataSource, p: &mut Particle) -> i32 {
    let r = p.mstruct.expect("mesh anim");
    let cur = ftol_i32(ld(p.mesh_frame));
    let frame = |i: i32| (src.dword(r, (i as u32).wrapping_mul(8)), src.dword(r, (i as u32).wrapping_mul(8) + 4));
    if frame(cur).1 == PART_LOOP {
        let lc = p.loop_count;
        let mut seek = true;
        if lc > 0 {
            p.loop_count = lc - 1;
        } else if lc == 0 {
            seek = false;
        }
        if seek {
            if frame(0).1 == PART_LOOPSTART {
                return 0;
            }
            let mut i = 0;
            loop {
                if frame(i).0 == 0 {
                    return -1;
                }
                i += 1;
                if frame(i).1 == PART_LOOPSTART {
                    return i;
                }
                if i > 4096 {
                    return -1;
                }
            }
        }
    }
    let next = cur + 1;
    let m = frame(next).0;
    if m == 0 || m == 0xffff_ffff {
        if p.loop_count != 0 {
            0
        } else {
            cur
        }
    } else {
        next
    }
}

/// `partModifyTexture` on a whole system.
pub fn modify_texture(sys: &mut PartSystem, tex: u32, uv: [u32; 4]) {
    match sys.kind {
        PartType::Billboard => {
            sys.tex = tex;
            sys.uv = [[uv[0], uv[1]], [uv[2], uv[3]]];
            for p in &mut sys.particles {
                p.tex = tex;
            }
        }
        PartType::Mesh => {
            sys.tex = tex;
            for p in &mut sys.particles {
                p.tex = tex;
            }
        }
        _ => {}
    }
}

impl PartSystem {
    /// `partUpdateSystem`: advance by `dt`. Returns `true` when no particle is alive or waiting.
    pub fn update(&mut self, dt: f32, src: &dyn DataSource, flares: &mut [LensFlare]) -> bool {
        let dtx = ld(dt);
        if self.ang_delta != 0.0 {
            self.ang = st(dtx * ld(self.ang_delta) + ld(self.ang));
        }
        let draggin = self.drag.to_bits() != 1.0f32.to_bits();
        let drag = ld(self.drag);
        let mut hits = 0u32;
        let n = self.particles.len();
        for i in 0..n {
            if self.kind == PartType::Mesh {
                let p = &mut self.particles[i];
                for k in 0..3 {
                    p.tumble[k] = st(dtx * ld(p.delta_tumble[k]) + ld(p.tumble[k]));
                }
            }
            {
                let p = &mut self.particles[i];
                if p.waitspan > 0.0 {
                    let w = st(ld(p.waitspan) - dtx);
                    p.waitspan = w;
                    if w < 0.0 || w.is_nan() {
                        p.waitspan = 0.0;
                    }
                    hits += 1;
                    continue;
                }
            }
            // Texture animation (also re-applies the frame's texture to the whole system).
            if self.particles[i].tstruct.is_some() && self.particles[i].rate != 0.0 {
                let r = self.particles[i].tstruct.unwrap();
                let prev = self.particles[i].current_frame;
                let next = dtx * ld(self.particles[i].rate) + ld(prev);
                let next_st = st(next);
                let a = ftol_i32(next);
                let b = ftol_i32(ld(prev));
                let frame = if a == b {
                    self.particles[i].current_frame = next_st;
                    a
                } else {
                    let frac = st(ld(next_st) - ld(next_st).floor());
                    let mut k = a - b;
                    while k > 0 {
                        let f = advance_tex_anim(src, &self.particles[i]);
                        self.particles[i].current_frame = st(f64::from(f) + ld(frac));
                        k -= 1;
                    }
                    ftol_i32(ld(self.particles[i].current_frame))
                };
                let (tex, uv, _) = tex_frame(src, r, frame);
                modify_texture(self, tex, uv);
            }
            let p = &mut self.particles[i];
            // Mesh morph animation, with the frame clamped to the animation's range.
            if p.mstruct.is_some() && p.mesh_rate != 0.0 {
                let r = p.mstruct.unwrap();
                let prev = p.mesh_frame;
                let next = dtx * ld(p.mesh_rate) + ld(prev);
                let next_st = st(next);
                let a = ftol_i32(next);
                let b = ftol_i32(ld(prev));
                if a == b {
                    p.mesh_frame = next_st;
                } else {
                    let frac = st(ld(next_st) - ld(next_st).floor());
                    let mut k = a - b;
                    while k > 0 {
                        let f = advance_mesh_morph(src, p);
                        p.mesh_frame = st(f64::from(f) + ld(frac));
                        k -= 1;
                    }
                }
                let mut count = 0i32;
                loop {
                    let m = src.dword(r, (count as u32) * 8);
                    if m == 0xffff_ffff {
                        break;
                    }
                    if src.dword(r, (count as u32) * 8 + 4) == PART_LOOP {
                        count += 1;
                        break;
                    }
                    count += 1;
                    if count > 4096 {
                        break;
                    }
                }
                let f = ftol_i32(ld(p.mesh_frame));
                if f >= count {
                    p.mesh_frame = (count - 1) as f32;
                } else if f < 0 {
                    p.mesh_frame = 0.0;
                }
                let f = ftol_i32(ld(p.mesh_frame));
                p.mesh = src.dword(r, (f as u32).wrapping_mul(8));
            }
            if p.delta_rot != 0.0 {
                p.rot = st(dtx * ld(p.delta_rot) + ld(p.rot));
            }
            if p.delta_length != 0.0 {
                let l = st(dtx * ld(p.delta_length) + ld(p.length));
                p.length = l;
                if l.is_nan() {
                    p.length = 1.0;
                }
                if p.clamp_length != 0 && (p.length < 0.0 || p.length.is_nan()) {
                    p.length = 0.0;
                }
            }
            if self.flags & sysflag::WORLDSPACE != 0 {
                if p.flags & pflag::XYZSCALE == 0 {
                    let vx = ld(p.w_vel.x) * dtx;
                    let vy = st(ld(p.w_vel.y) * dtx);
                    let vz = st(ld(p.w_vel.z) * dtx);
                    let ax = st(ld(p.w_accel.x) * dtx);
                    let ay = st(ld(p.w_accel.y) * dtx);
                    let az = st(ld(p.w_accel.z) * dtx);
                    let vx2 = vx + ld(ax);
                    let vy2 = st(ld(ay) + ld(vy));
                    let vz2 = st(ld(az) + ld(vz));
                    p.position.x = st(vx2 + ld(p.position.x));
                    p.position.y = st(ld(vy2) + ld(p.position.y));
                    p.position.z = st(ld(vz2) + ld(p.position.z));
                    let wx = ld(ax) + ld(p.w_vel.x);
                    let wy = st(ld(ay) + ld(p.w_vel.y));
                    let wz = st(ld(az) + ld(p.w_vel.z));
                    p.w_vel.x = st(drag * wx);
                    p.w_vel.y = st(drag * ld(wy));
                    p.w_vel.z = st(drag * ld(wz));
                    p.w_accel.x = st(drag * ld(p.w_accel.x));
                    p.w_accel.y = st(drag * ld(p.w_accel.y));
                    p.w_accel.z = st(drag * ld(p.w_accel.z));
                }
            } else {
                let t = dtx * ld(p.vel_r);
                let rx = st(t * ld(p.rvec.x));
                let ry = st(t * ld(p.rvec.y));
                let rz = t * ld(p.rvec.z);
                p.position.x = st(ld(rx) + ld(p.position.x));
                p.position.y = st(ld(ry) + ld(p.position.y));
                p.position.z = st((dtx * ld(p.vel_lof) + ld(p.position.z)) + rz);
                p.vel_lof = st(dtx * ld(p.delta_vel_lof) + ld(p.vel_lof));
                p.vel_r = st(dtx * ld(p.delta_vel_r) + ld(p.vel_r));
                if draggin {
                    p.vel_lof = st(drag * ld(p.vel_lof));
                    p.vel_r = st(drag * ld(p.vel_r));
                }
            }
            if let Some(fi) = p.lens_flare {
                if let Some(f) = flares.get_mut(fi) {
                    f.position = p.position;
                }
            }
            if p.delta_scale != 0.0 {
                let sx = st(dtx * ld(p.delta_scale) + ld(p.scale_x));
                p.scale_x = if sx < 0.0 || sx.is_nan() { 0.0 } else { sx };
                let sy = st(dtx * ld(p.delta_scale) + ld(p.scale_y));
                p.scale_y = if sy < 0.0 || sy.is_nan() { 0.0 } else { sy };
            }
            let dc = p.delta_color;
            color_delta(&mut p.icolor, &dc, self.flags, dt);
            if p.icolor[3] < 1.0 || p.icolor[3].is_nan() {
                self.flags |= sysflag::ALPHA;
            }
            let p = &mut self.particles[i];
            if p.lit != 0 {
                let v = st(dtx * ld(p.delta_illum) + ld(p.illum));
                p.illum = v;
                if v < 0.0 || v.is_nan() {
                    p.illum = 0.0;
                } else if v > 1.0 {
                    p.illum = 1.0;
                }
            }
            let l = st(ld(p.lifespan) - dtx);
            p.lifespan = l;
            if l > 0.0 {
                hits += 1;
            }
        }
        hits == 0
    }
}

/// `partSetRandomOffsetArray`: fill the static offset array with points in a box (stream 0).
pub fn random_offset_array(randy: &mut Randy, a: [f32; 6]) -> Vec<Vector> {
    let (mut x0, mut y0, mut z0, mut x1, mut y1, mut z1) = (a[0], a[1], a[2], a[3], a[4], a[5]);
    if x1 < x0 {
        std::mem::swap(&mut x0, &mut x1);
    }
    if y1 < y0 {
        std::mem::swap(&mut y0, &mut y1);
    }
    if z1 < z0 {
        std::mem::swap(&mut z0, &mut z1);
    }
    let k = f64::from(2.328_306_4e-10f32);
    let one = |randy: &mut Randy, lo: f32, hi: f32| {
        let u = f64::from(randy.random(RAN_PARTICLE_STREAM));
        st(u * (ld(hi) - ld(lo)) * k + ld(lo))
    };
    (0..RANDOM_OFFSET_COUNT)
        .map(|_| {
            let x = one(randy, x0, x1);
            let y = one(randy, y0, y1);
            let z = one(randy, z0, z1);
            Vector::new(x, y, z)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoData;
    impl DataSource for NoData {
        fn dword(&self, _: DataRef, _: u32) -> u32 {
            0
        }
    }

    #[test]
    fn real_dist_uses_stream_25() {
        let mut a = Randy::default();
        let v = real_dist(&mut a, 5.0, 0.0);
        assert_eq!(v, 5.0);
        assert_ne!(a.streams[RAN_PARTICLES_CAT], Randy::default().streams[RAN_PARTICLES_CAT]);
        assert_eq!(a.streams[RAN_PARTICLE_STREAM], Randy::default().streams[RAN_PARTICLE_STREAM]);
    }

    #[test]
    fn local_kinematics_and_lifespan() {
        let mut pat = Pat::defaults();
        pat.vel_lof = 10.0;
        pat.lifespan = 1.0;
        let mut randy = Randy::default();
        let mut flares = Vec::new();
        let mut env = CreateEnv { randy: &mut randy, data: &NoData, lens_flares: &mut flares };
        let mut sys = PartSystem::new_header(PartType::Points, &pat, 0.0);
        sys.particles.push(make_particle(&pat, PartType::Points, 0, &mut env));
        assert!(!sys.update(0.5, &NoData, &mut []));
        assert_eq!(sys.particles[0].position.z, 5.0);
        assert!(sys.update(0.5, &NoData, &mut []));
    }
}
