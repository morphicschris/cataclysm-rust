//! Scene lighting with the semantics of the engine's fixed-function OpenGL setup: a global
//! ambient term plus up to two lights, each with ambient and diffuse colours, evaluated per
//! vertex as `material.ambient × (global + Σ light.ambient) + Σ material.diffuse × light.diffuse
//! × max(N·L, 0)` and clamped to 0..1 (no specular: the engine never sets material specular).
//!
//! Lights normally come from a mission's `.hsf` scene file. Without one the engine uses the
//! `default` entry of `light.script`.

use glam::Vec3;
use hwc_formats::hsf::{Hsf, LightKind};

/// Ambient term the engine uses when a scene file has no ambient light.
pub const DEFAULT_GLOBAL_AMBIENT: f32 = 0.2;

/// One light. `direction` points from the scene towards the light, in world space (Z up).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Light {
    pub direction: Vec3,
    pub ambient: Vec3,
    pub diffuse: Vec3,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SceneLighting {
    pub global_ambient: Vec3,
    /// At most two lights are used, as in the engine.
    pub lights: Vec<Light>,
}

impl Default for SceneLighting {
    /// `light.script`'s default light: ambient 0.6, diffuse 0.8, placed at (1, -10000, 3500)
    /// with w = 0.1, i.e. far enough away to act as a directional light.
    fn default() -> Self {
        Self {
            global_ambient: Vec3::splat(DEFAULT_GLOBAL_AMBIENT),
            lights: vec![Light {
                direction: Vec3::new(1.0, -10000.0, 3500.0).normalize(),
                ambient: Vec3::splat(0.6),
                diffuse: Vec3::splat(0.8),
            }],
        }
    }
}

/// Converts a scene-file position from the exporter's axes to the engine's world axes:
/// (x, y, z) becomes (-y, -z, x).
pub fn exporter_to_world(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[1], -p[2], p[0])
}

impl SceneLighting {
    /// Builds lighting the way the engine parses an `.hsf`: the first non-ambient record fills
    /// light 0, the second light 1 (later ones overwrite light 0 again, and with more than two
    /// only light 0 stays enabled); each light's ambient is
    /// 0.3 × intensity × colour and its diffuse intensity × colour; the ambient record sets the
    /// global ambient to twice its intensity × colour, capped at 1.
    pub fn from_hsf(hsf: &Hsf) -> Self {
        let mut out = Self::default();
        let mut slots: [Option<Light>; 2] = [None, None];
        let mut count = 0usize;
        for l in &hsf.lights {
            let colour = Vec3::from_array(l.color.map(|c| f32::from(c) / 255.0)) * l.intensity;
            if l.kind == LightKind::Ambient {
                out.global_ambient = (colour * 2.0).min(Vec3::ONE);
                continue;
            }
            count += 1;
            let slot = if count == 2 { 1 } else { 0 };
            slots[slot] = Some(Light {
                direction: exporter_to_world(l.position).normalize_or(Vec3::Z),
                ambient: colour * 0.3,
                diffuse: colour,
            });
        }
        if let Some(first) = slots[0] {
            out.lights = vec![first];
            // The engine only enables the second light when there are exactly two.
            if count == 2 {
                out.lights.extend(slots[1]);
            }
        }
        out
    }

    /// Lit colour of a vertex for the given material colours and unit normal, exactly as the
    /// fixed-function pipeline computes it (used by tests and software paths).
    pub fn shade(&self, ambient: Vec3, diffuse: Vec3, normal: Vec3) -> Vec3 {
        let mut c = ambient * self.global_ambient;
        for l in self.lights.iter().take(2) {
            c += ambient * l.ambient + diffuse * l.diffuse * normal.dot(l.direction).max(0.0);
        }
        c.clamp(Vec3::ZERO, Vec3::ONE)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hwc_formats::hsf::Light as HsfLight;

    fn hsf_light(kind: LightKind, position: [f32; 3], intensity: f32) -> HsfLight {
        HsfLight {
            kind,
            position,
            orientation: [0.0; 3],
            cone_angle: None,
            edge_angle: None,
            color: [255, 255, 255],
            intensity,
        }
    }

    #[test]
    fn hsf_rules() {
        let hsf = Hsf {
            version: 0x100,
            lights: vec![
                hsf_light(LightKind::Ambient, [0.0; 3], 0.3),
                hsf_light(LightKind::Distant, [5.0, 0.0, 0.0], 1.0),
                hsf_light(LightKind::Distant, [0.0, -1.0, 0.0], 0.5),
            ],
        };
        let s = SceneLighting::from_hsf(&hsf);
        assert!((s.global_ambient - Vec3::splat(0.6)).length() < 1e-6);
        assert_eq!(s.lights.len(), 2);
        assert_eq!(s.lights[0].direction, Vec3::Z);
        assert!((s.lights[0].ambient - Vec3::splat(0.3)).length() < 1e-6);
        assert_eq!(s.lights[1].direction, Vec3::X);
        assert!((s.lights[1].diffuse - Vec3::splat(0.5)).length() < 1e-6);
    }

    #[test]
    fn shading_clamps_and_ignores_back_light() {
        let s = SceneLighting {
            global_ambient: Vec3::splat(0.2),
            lights: vec![Light {
                direction: Vec3::Z,
                ambient: Vec3::splat(0.5),
                diffuse: Vec3::ONE,
            }],
        };
        let lit = s.shade(Vec3::splat(0.5), Vec3::ONE, Vec3::Z);
        assert_eq!(lit, Vec3::ONE);
        let back = s.shade(Vec3::splat(0.5), Vec3::ONE, -Vec3::Z);
        assert!((back - Vec3::splat(0.35)).length() < 1e-6);
    }
}
