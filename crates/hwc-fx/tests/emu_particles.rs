//! Bit-exact comparison with `cataclysm.exe`: particle creation (the body of
//! `partFillGenericParticles`, 0x4dbae0) and `partUpdateSystem` (0x4dae60), run in Ghidra's
//! p-code emulator on random inputs (see `emu_part.py`; the emulator lacks FSIN/FCOS, so the
//! creation cases use a zero angle with those two instructions patched to constants).

use hwc_fx::particle::{make_particle, CreateEnv, DataRef, DataSource, PartSystem, PartType, Particle, Pat};
use hwc_math::random::{RanStream, Randy, RAN_PARTICLES_CAT};
use hwc_math::Matrix;

pub struct FillCase {
    pub pat: &'static [u32; 114],
    pub mat: [u32; 9],
    pub stream: [u32; 5],
    pub particles: &'static [[u32; 64]],
    pub stream_after: [u32; 5],
}

pub struct UpdateCase {
    pub kind: u32,
    pub flags: u32,
    pub drag: u32,
    pub ang: u32,
    pub ang_delta: u32,
    pub dt: u32,
    pub before: &'static [[u32; 64]],
    pub after: &'static [[u32; 64]],
    pub dead: bool,
    pub flags_after: u32,
    pub ang_after: u32,
}

#[allow(clippy::unreadable_literal)]
mod data {
    use super::{FillCase, UpdateCase};
    include!("data/emu_particles.rs");
}

struct NoData;
impl DataSource for NoData {
    fn dword(&self, _: DataRef, _: u32) -> u32 {
        0
    }
}

fn stream(v: [u32; 5]) -> RanStream {
    RanStream { x: v[0], y: v[1], z: v[2], c: v[3], n: v[4] }
}

fn diff(a: &[u32; 64], b: &[u32; 64]) -> Vec<String> {
    (0..64)
        .filter(|&i| a[i] != b[i] && i != 31)
        .map(|i| format!("dword {i:#x}: got {:#010x} ({}) want {:#010x} ({})", a[i], f32::from_bits(a[i]), b[i], f32::from_bits(b[i])))
        .collect()
}

#[test]
fn particle_creation_matches_binary() {
    let mut bad = Vec::new();
    for (ci, c) in data::FILL_CASES.iter().enumerate() {
        let mat = Matrix { m: c.mat.map(f32::from_bits) };
        let pat = Pat::from_raw(c.pat, mat);
        let mut randy = Randy::default();
        randy.streams[RAN_PARTICLES_CAT] = stream(c.stream);
        let mut flares = Vec::new();
        let mut env = CreateEnv { randy: &mut randy, data: &NoData, lens_flares: &mut flares };
        for (i, want) in c.particles.iter().enumerate() {
            let p = make_particle(&pat, PartType::Points, i as u32, &mut env);
            let mut got = p.to_raw();
            // The original leaves deltaColor[3] untouched without PART_ALPHA (memory is zeroed
            // in the emulation, as here).
            got[62] &= 0xffff;
            let mut w = *want;
            w[62] &= 0xffff;
            for d in diff(&got, &w) {
                bad.push(format!("fill case {ci} particle {i}: {d}"));
            }
        }
        assert_eq!(randy.streams[RAN_PARTICLES_CAT], stream(c.stream_after), "fill case {ci}: random stream");
    }
    assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn particle_update_matches_binary() {
    let mut bad = Vec::new();
    for (ci, c) in data::UPDATE_CASES.iter().enumerate() {
        let kind = match c.kind {
            0 => PartType::Points,
            2 => PartType::Lines,
            3 => PartType::Mesh,
            _ => PartType::EBolt,
        };
        let mut sys = PartSystem::new_header(kind, &Pat::defaults(), 0.0);
        sys.flags = c.flags;
        sys.drag = f32::from_bits(c.drag);
        sys.ang = f32::from_bits(c.ang);
        sys.ang_delta = f32::from_bits(c.ang_delta);
        sys.particles = c.before.iter().map(Particle::from_raw).collect();
        let dead = sys.update(f32::from_bits(c.dt), &NoData, &mut []);
        assert_eq!(dead, c.dead, "update case {ci}: return value");
        assert_eq!(sys.flags, c.flags_after, "update case {ci}: system flags");
        assert_eq!(sys.ang.to_bits(), c.ang_after, "update case {ci}: angle");
        for (i, (p, want)) in sys.particles.iter().zip(c.after).enumerate() {
            for d in diff(&p.to_raw(), want) {
                bad.push(format!("update case {ci} (kind {}) particle {i}: {d}", c.kind));
            }
        }
    }
    assert!(bad.is_empty(), "{} mismatches:\n{}", bad.len(), bad.join("\n"));
}
