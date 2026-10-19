//! Camera-independent render data produced from live particle systems.
//!
//! The original drew particles immediately with OpenGL. Here each system becomes a list of
//! instances in world space carrying everything a renderer needs: colour, size, texture and
//! texel rectangle, blend mode and the per-particle flags. Billboards are emitted as
//! camera-facing quads to be expanded by the renderer; lines, bolts and points as geometry.

use hwc_math::{HMatrix, Vector};

/// How a primitive is blended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    /// No blending (alpha == 1 and the texture has no alpha).
    Opaque,
    /// Standard alpha blending.
    Alpha,
    /// Additive blending (`PART_ADDITIVE` on an alpha texture).
    Additive,
}

/// A camera-facing textured quad or untextured disc.
#[derive(Clone, Debug, PartialEq)]
pub struct Sprite {
    /// Centre in world space.
    pub position: Vector,
    /// Full width/height of the quad (the original draws `±scale/2`).
    pub size: [f32; 2],
    /// Rotation in the view plane (radians).
    pub rotation: f32,
    pub color: [f32; 4],
    /// Additive colour bias (`setAddColor`).
    pub bias: [f32; 3],
    /// Texture path (as registered), or `None` for a filled disc of radius `size[0]`.
    pub texture: Option<String>,
    /// Texel rectangle `[[u0, v0], [u1, v1]]` (divide by the texture's size on disk).
    pub uv: [[u32; 2]; 2],
    /// Disc subdivision for untextured sprites (`setCircleSlices`).
    pub slices: u32,
    pub blend: Blend,
    /// `PART_STIPPLE` (screen-door transparency in the original).
    pub stipple: bool,
}

/// A line segment (`createLines`).
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub start: Vector,
    pub end: Vector,
    /// Line width in pixels.
    pub width: f32,
    pub color: [f32; 4],
    pub blend: Blend,
}

/// A jittered polyline (`createEBolts`).
#[derive(Clone, Debug, PartialEq)]
pub struct Bolt {
    pub points: Vec<Vector>,
    pub width: f32,
    pub color: [f32; 4],
    pub blend: Blend,
}

/// A point (`createPoints`), at most 3 pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Point {
    pub position: Vector,
    pub size: f32,
    pub color: [f32; 4],
    pub blend: Blend,
}

/// A cube particle (`PART_CUBES`; not created by any ETG function).
#[derive(Clone, Debug, PartialEq)]
pub struct Cube {
    pub position: Vector,
    pub size: f32,
    pub color: [f32; 4],
}

/// A mesh particle.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshInstance {
    /// Mesh path (as registered).
    pub mesh: Option<String>,
    /// Next morph-target mesh and blend fraction, for morph animations.
    pub morph: Option<(Option<String>, f32)>,
    /// Object-to-world transform including tumble and scale (for non-billboarded meshes).
    pub transform: HMatrix,
    /// Particle position in world space.
    pub position: Vector,
    /// Normalised velocity used to orient (pseudo-)billboarded meshes.
    pub heading: Vector,
    pub color: [f32; 4],
    pub bias: [f32; 3],
    pub texture: Option<String>,
    pub blend: Blend,
    pub lit: bool,
    pub illum: f32,
    pub specular_exponent: Option<f32>,
    pub color_scheme: u8,
    pub depth_write: bool,
    pub pseudo_billboard: bool,
    pub true_billboard: bool,
}

/// A lens flare driven by a type-6 particle.
#[derive(Clone, Debug, PartialEq)]
pub struct Flare {
    pub position: Vector,
    pub intensity: f32,
}

/// Everything one draw pass produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RenderList {
    pub sprites: Vec<Sprite>,
    pub lines: Vec<Line>,
    pub bolts: Vec<Bolt>,
    pub points: Vec<Point>,
    pub cubes: Vec<Cube>,
    pub meshes: Vec<MeshInstance>,
    pub flares: Vec<Flare>,
}

impl RenderList {
    /// Total number of primitives.
    pub fn len(&self) -> usize {
        self.sprites.len()
            + self.lines.len()
            + self.bolts.len()
            + self.points.len()
            + self.cubes.len()
            + self.meshes.len()
            + self.flares.len()
    }

    /// Whether nothing was produced.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
