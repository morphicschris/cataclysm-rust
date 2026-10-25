//! Camera-independent render data produced from live particle systems.
//!
//! The original drew particles immediately with OpenGL. Here each system becomes a list of
//! instances in world space carrying everything a renderer needs: colour, size, texture and
//! texel rectangle, blend mode and the per-particle flags. Billboards are emitted as
//! camera-facing quads to be expanded by the renderer; lines, bolts and points as geometry.
//! [`RenderList::spans`] keeps the order the systems were drawn in.

use hwc_math::{HMatrix, Matrix, Vector};

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
    /// `PART_ADDITIVE`. The original only honours it when the system's texture has an alpha
    /// channel (otherwise the quad is opaque, or alpha blended when its alpha is below 1); a
    /// renderer that knows the texture resolves [`Sprite::blend`] again from this flag.
    pub additive: bool,
    /// The particle's ambient override (`setIllum`, 0 = none).
    pub illum: f32,
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
    /// The blend function the original selects. It never enables `GL_BLEND` for bolts, so a
    /// faithful renderer draws them opaque whatever this says.
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
    /// Edge length: `drawBox(scale / 2)` spans `±size / 2`.
    pub size: f32,
    pub color: [f32; 4],
    /// Alpha blended when the system has the alpha flag, otherwise opaque.
    pub blend: Blend,
}

/// A mesh particle.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshInstance {
    /// Mesh path (as registered).
    pub mesh: Option<String>,
    /// Next morph-target mesh and blend fraction, for morph animations.
    pub morph: Option<(Option<String>, f32)>,
    /// Object-to-world transform including tumble and scale (for non-billboarded meshes).
    /// Pseudo-billboards add a camera-dependent basis on top: see
    /// [`MeshInstance::model_matrix`].
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
    /// `PART_ADDITIVE` (honoured when the texture has alpha or the system is alpha blended).
    pub additive: bool,
    /// The system's `PART_ALPHA` flag (`glColor4f` and blending on).
    pub alpha_system: bool,
    /// `PART_XYZSCALE` (hyperspace-style meshes; their colour bias is not applied).
    pub xyz_scale: bool,
    /// The mesh of the morph animation's first frame; the original takes the polygon list and
    /// materials of a morphed mesh from it.
    pub morph_base: Option<String>,
    /// Pseudo-billboard input: the particle velocity the original orients by (effect
    /// velocity removed unless the system has world-space velocity), normalised, `+Z` if zero.
    pub billboard_velocity: Vector,
    /// The system's coordinate system and position (`partMat`, `partPos`).
    pub system_coordsys: Matrix,
    pub system_position: Vector,
    /// The particle position in the system's space (world space for world-space systems).
    pub local_position: Vector,
    /// World-space system.
    pub world_space: bool,
}

impl MeshInstance {
    /// The pseudo-billboard basis (`calcPseudoBillboardMatrixWorld`/`NotWorld`): its columns
    /// are up = eye ray × velocity, right = velocity × up, and the velocity. `eye` is the camera
    /// position in world space. For a local system the eye is brought into the system's space
    /// the way the original does it (relative to the particle's world position).
    pub fn pseudo_billboard_basis(&self, eye: Vector) -> HMatrix {
        let vel = self.billboard_velocity;
        let mut veye = if self.world_space {
            sub(self.local_position, eye)
        } else {
            let origin = add(self.system_coordsys.mul_vec(self.local_position), self.system_position);
            let cam = self.system_coordsys.vec_mul(sub(eye, origin));
            sub(self.local_position, cam)
        };
        veye.normalize();
        let mut up = veye.cross(vel);
        up.normalize();
        let mut right = vel.cross(up);
        right.normalize();
        HMatrix::from_matrix(&Matrix::from_cols(up, right, vel))
    }

    /// The full object-to-world matrix for a camera at `eye`: [`MeshInstance::transform`],
    /// followed by the pseudo-billboard basis when the particle has that flag.
    pub fn model_matrix(&self, eye: Vector) -> HMatrix {
        if self.pseudo_billboard {
            self.transform.mul_hmat(&self.pseudo_billboard_basis(eye))
        } else {
            self.transform
        }
    }
}

fn add(a: Vector, b: Vector) -> Vector {
    Vector::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

fn sub(a: Vector, b: Vector) -> Vector {
    Vector::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// A lens flare driven by a type-6 particle.
#[derive(Clone, Debug, PartialEq)]
pub struct Flare {
    pub position: Vector,
    pub intensity: f32,
    /// Index of the flare object in [`crate::FxWorld::lens_flares`].
    pub index: usize,
}

/// Which list of a [`RenderList`] a [`Span`] covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimKind {
    Sprites,
    Lines,
    Bolts,
    Points,
    Cubes,
    Meshes,
    Flares,
}

/// One particle system's primitives, in the order the original drew the systems.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Span {
    pub kind: PrimKind,
    /// Range in the list named by `kind`.
    pub start: usize,
    pub count: usize,
    /// The first particle's `lit` flag: the original enables lighting for the whole system.
    pub lit: bool,
    /// The system's `PART_ALPHA` flag.
    pub alpha: bool,
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
    /// Draw order: one span per emitted system. Lists built by hand may leave this empty, in
    /// which case renderers draw the lists one kind after another (see
    /// [`RenderList::draw_order`]).
    pub spans: Vec<Span>,
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

    /// Empties every list (keeping their capacity).
    pub fn clear(&mut self) {
        self.sprites.clear();
        self.lines.clear();
        self.bolts.clear();
        self.points.clear();
        self.cubes.clear();
        self.meshes.clear();
        self.flares.clear();
        self.spans.clear();
    }

    /// Number of primitives of one kind.
    pub fn kind_len(&self, kind: PrimKind) -> usize {
        match kind {
            PrimKind::Sprites => self.sprites.len(),
            PrimKind::Lines => self.lines.len(),
            PrimKind::Bolts => self.bolts.len(),
            PrimKind::Points => self.points.len(),
            PrimKind::Cubes => self.cubes.len(),
            PrimKind::Meshes => self.meshes.len(),
            PrimKind::Flares => self.flares.len(),
        }
    }

    /// The spans to draw, in order: [`RenderList::spans`] when every primitive is covered by
    /// them, otherwise one span per non-empty list.
    pub fn draw_order(&self) -> Vec<Span> {
        let covered: usize = self.spans.iter().map(|s| s.count).sum();
        if !self.spans.is_empty() && covered == self.len() {
            return self.spans.clone();
        }
        [
            PrimKind::Sprites,
            PrimKind::Meshes,
            PrimKind::Cubes,
            PrimKind::Lines,
            PrimKind::Bolts,
            PrimKind::Points,
            PrimKind::Flares,
        ]
        .into_iter()
        .filter(|k| self.kind_len(*k) > 0)
        .map(|kind| Span { kind, start: 0, count: self.kind_len(kind), lit: false, alpha: false })
        .collect()
    }

    /// Records a span for primitives pushed since `before` (a snapshot of
    /// [`RenderList::kind_len`]); empty spans are skipped.
    pub fn push_span(&mut self, kind: PrimKind, before: usize, lit: bool, alpha: bool) {
        let count = self.kind_len(kind).saturating_sub(before);
        if count > 0 {
            self.spans.push(Span { kind, start: before, count, lit, alpha });
        }
    }
}
