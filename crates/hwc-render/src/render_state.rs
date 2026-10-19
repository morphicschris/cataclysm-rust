//! Per-instance render state: Cataclysm's special ship effects (cloak fade, phase blend, beast
//! infection, consume, Naggarok EMP lightning, colour tint) and the inputs that select a level of
//! detail and an animated pose.
//!
//! The helpers here compute the effect parameters from game times the same way the engine's
//! per-object render-state setup does; the sim (or a viewer) owns the times and flags.

use glam::{Mat4, Vec3, Vec4};

use crate::gpu::GpuShip;

/// Ship colour tint inputs (the engine's per-vertex tint): vertices flagged in the mesh glow
/// with the team colour while engine `n` is lit, or flash.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TintParams {
    /// The tint option; when off no vertex tint applies.
    pub enabled: bool,
    /// Team colour scaled so its largest component is 1.
    pub team_color: [f32; 3],
    /// Engine glow per trail slot, 0..1 (see [`crate::trails::ShipTrails::engine_glow_levels`]).
    pub engine_glow: [f32; 4],
    /// Glow level, 0..1; darkens tinted vertices by 0.15 × level.
    pub glow: f32,
    /// Flash level, 0..1; vertices with the flash flag add their own colour while it is up.
    pub flash: f32,
}

impl Default for TintParams {
    fn default() -> Self {
        Self {
            enabled: true,
            team_color: [1.0; 3],
            engine_glow: [0.0; 4],
            glow: 0.0,
            flash: 0.0,
        }
    }
}

/// The team colour normalised so its brightest channel is 1 (white for black).
pub fn normalized_team_color(c: [u8; 3]) -> [f32; 3] {
    let f = c.map(|v| f32::from(v) / 256.0);
    let m = f[0].max(f[1]).max(f[2]);
    if m <= 0.0 {
        return [1.0; 3];
    }
    f.map(|v| v / m)
}

/// Decaying flash and glow levels (the engine's tint effect update).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ShipTint {
    pub glow: f32,
    pub flash: f32,
    last_time: Option<f32>,
}

impl ShipTint {
    /// Starts a flash (e.g. when the ship is hit).
    pub fn flash(&mut self, amount: f32) {
        self.flash = (self.flash + amount).min(1.0);
    }

    /// Advances to game time `now`: the flash decays 0.2 per second and feeds a fifth of what it
    /// loses into the glow, which itself decays 0.1 per second.
    pub fn update(&mut self, now: f32) {
        let dt = self.last_time.map_or(0.0, |t| (now - t).max(0.0));
        self.last_time = Some(now);
        if self.flash > 0.0 {
            let before = self.flash.min(1.0);
            self.flash = (before - dt * 0.2).max(0.0);
            self.glow += (before - self.flash) * 0.2;
        }
        self.glow = (self.glow - dt * 0.1).clamp(0.0, 1.0);
    }

    pub fn params(&self, team_color: [f32; 3], engine_glow: [f32; 4]) -> TintParams {
        TintParams {
            team_color,
            engine_glow,
            glow: self.glow,
            flash: self.flash,
            ..TintParams::default()
        }
    }
}

/// Stage of the three-part phase blend (in, hold, out).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStage {
    In,
    Sweep,
    Out,
}

/// The phase-blend state at game time `now` for an effect running from `start` to `end`:
/// the first and last tenth ease with a cosine, the middle is linear. `None` once it is over.
pub fn phase_blend_state(now: f32, start: f32, end: f32) -> Option<(PhaseStage, f32)> {
    if now >= end || end <= start {
        return None;
    }
    let t = (now - start) / (end - start);
    let ease = |f: f32| -((f + 1.0) * std::f32::consts::FRAC_PI_2).cos();
    Some(if t < 0.1 {
        (PhaseStage::In, ease(t * 10.0))
    } else if t < 0.9 {
        (PhaseStage::Sweep, (t - 0.1) * 1.25)
    } else {
        (PhaseStage::Out, ease((t - 0.9) * 10.0))
    })
}

/// Beast infection progress, 0..1: how far through `duration` seconds the infection that ends
/// at `infect_end` is.
pub fn infection_progress(now: f32, infect_end: f32, duration: f32) -> f32 {
    if duration <= 0.0 {
        return 1.0;
    }
    (1.0 - (infect_end - now) / duration).min(1.0)
}

/// A mesh-space collision box (minimum corner and extent).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionBox {
    pub corner: Vec3,
    pub extent: Vec3,
}

/// The clip plane of a cloak "wipe": the hull is cut along the ship's heading axis at a
/// distance that grows with the cloak fraction. Keeps points with `plane · (p, 1) >= 0`.
pub fn cloak_wipe_plane(bx: &CollisionBox, cloak_fraction: f32) -> Vec4 {
    let d = bx.extent.z * cloak_fraction + bx.corner.z;
    Vec4::new(0.0, 0.0, -1.0, d)
}

/// The sweep plane of the phase blend's middle stage: across the ship's up axis, moving with
/// the stage value. The original ship keeps the side the plane faces; the converted ship is
/// drawn with the plane flipped.
pub fn phase_sweep_plane(bx: &CollisionBox, value: f32) -> Vec4 {
    let d = (1.0 - value) * bx.extent.x + bx.corner.x;
    Vec4::new(-1.0, 0.0, 0.0, d)
}

/// How cloaking shows: fading out, or a clip-plane wipe (the graphics option).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CloakStyle {
    Fade,
    Wipe(CollisionBox),
}

/// Extra per-instance state for [`crate::Renderer::render_ex`]. Index-matched with
/// [`crate::Scene::ships`]; ships without a state draw as before.
#[derive(Clone, Copy)]
pub struct ShipRenderState<'a> {
    /// Level of detail to draw (clamped to the levels uploaded).
    pub lod: usize,
    /// Per-object local matrices for the drawn level (`None` keeps the rest matrix), as from
    /// [`crate::anim::MadClip::pose`] with a binding for that level's skeleton.
    pub pose: Option<&'a [Option<Mat4>]>,
    /// Overall opacity: below 1 the ship is drawn blended in the sorted transparent pass.
    pub fade: f32,
    /// Mesh-space plane; parts of the hull with `plane · (p, 1) < 0` are cut away.
    pub clip_plane: Option<Vec4>,
    pub tint: TintParams,
    /// Beast infection: the infected variant drawn over the hull, scaled by 1.01 and blended
    /// in with the progress (0..1).
    pub infection: Option<(&'a GpuShip, f32)>,
    /// Phase blend stage and value, with the ship it becomes and its collision box.
    pub phase: Option<PhaseState<'a>>,
    /// Naggarok EMP: lightning crawls over the hull edges.
    pub emp_lightning: bool,
    /// Seconds, for time-varying effects (lightning).
    pub time: f32,
    /// Forces the sorted transparent pass.
    pub transparent: bool,
}

#[derive(Clone, Copy)]
pub struct PhaseState<'a> {
    pub stage: PhaseStage,
    pub value: f32,
    pub converted: Option<&'a GpuShip>,
    pub collision_box: CollisionBox,
}

impl Default for ShipRenderState<'_> {
    fn default() -> Self {
        Self {
            lod: 0,
            pose: None,
            fade: 1.0,
            clip_plane: None,
            tint: TintParams::default(),
            infection: None,
            phase: None,
            emp_lightning: false,
            time: 0.0,
            transparent: false,
        }
    }
}

impl ShipRenderState<'_> {
    /// Whether the engine would defer this ship to the depth-sorted transparent list.
    pub fn is_transparent(&self) -> bool {
        self.transparent || self.fade < 1.0 || self.infection.is_some() || self.phase.is_some()
    }
}

/// Transforms a mesh-space plane into world space for a mesh-to-world matrix.
pub fn plane_to_world(transform: Mat4, plane: Vec4) -> Vec4 {
    transform.inverse().transpose() * plane
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_stages() {
        let (s, v) = phase_blend_state(0.0, 0.0, 10.0).unwrap();
        assert_eq!(s, PhaseStage::In);
        assert!(v.abs() < 1e-6);
        let (s, v) = phase_blend_state(0.999, 0.0, 10.0).unwrap();
        assert_eq!(s, PhaseStage::In);
        assert!(v > 0.99);
        let (s, v) = phase_blend_state(5.0, 0.0, 10.0).unwrap();
        assert_eq!(s, PhaseStage::Sweep);
        assert!((v - 0.5).abs() < 1e-5);
        assert_eq!(
            phase_blend_state(9.5, 0.0, 10.0).unwrap().0,
            PhaseStage::Out
        );
        assert!(phase_blend_state(10.0, 0.0, 10.0).is_none());
    }

    #[test]
    fn team_colour_normalisation() {
        let c = normalized_team_color([66, 108, 202]);
        assert!((c[2] - 1.0).abs() < 1e-6);
        assert!((c[0] - 66.0 / 202.0).abs() < 1e-6);
        assert_eq!(normalized_team_color([0, 0, 0]), [1.0; 3]);
    }

    #[test]
    fn tint_decay() {
        let mut t = ShipTint::default();
        t.update(0.0);
        t.flash(1.0);
        t.update(1.0);
        assert!((t.flash - 0.8).abs() < 1e-6);
        assert!((t.glow - (0.04 - 0.1f32).max(0.0)).abs() < 1e-6);
    }

    #[test]
    fn planes() {
        let bx = CollisionBox {
            corner: Vec3::splat(-10.0),
            extent: Vec3::splat(20.0),
        };
        let p = cloak_wipe_plane(&bx, 0.5);
        assert!(p.dot(Vec3::new(0.0, 0.0, -5.0).extend(1.0)) > 0.0);
        assert!(p.dot(Vec3::new(0.0, 0.0, 5.0).extend(1.0)) < 0.0);
        let world = plane_to_world(Mat4::from_translation(Vec3::new(0.0, 0.0, 100.0)), p);
        assert!(world.dot(Vec3::new(0.0, 0.0, 95.0).extend(1.0)) > 0.0);
        assert!((infection_progress(5.0, 10.0, 10.0) - 0.5).abs() < 1e-6);
    }
}
