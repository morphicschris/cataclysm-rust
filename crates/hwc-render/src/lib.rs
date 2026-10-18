//! Real-time renderer for Homeworld: Cataclysm, reproducing the original OpenGL look: ship
//! meshes with team-coloured textures under fixed-function style lighting, `.btg` backgrounds,
//! nav lights, and the game's orbit camera.
//!
//! The renderer knows nothing about windows: give [`Renderer`] a wgpu device and queue and it
//! draws a [`Scene`] into any texture view (a swap-chain image or an [`OffscreenTarget`]).

pub mod assets;
pub mod background;
pub mod camera;
pub mod gpu;
pub mod lighting;
pub mod mesh;
pub mod tga;

pub use assets::{AssetSource, DirSource, ShipDef, ShipModel, TeamColors, TeamScheme};
pub use background::Background;
pub use camera::{CameraTweaks, OrbitCamera};
pub use gpu::{
    headless_device, GpuBackground, GpuShip, OffscreenTarget, Renderer, Scene, ShipInstance,
    TeamPaint, DEPTH_FORMAT, OFFSCREEN_FORMAT,
};
pub use lighting::SceneLighting;
pub use mesh::mesh_to_world;

pub use wgpu;
