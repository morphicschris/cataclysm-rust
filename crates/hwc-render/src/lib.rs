//! Real-time renderer for Homeworld: Cataclysm, reproducing the original OpenGL look: ship
//! meshes with team-coloured textures under fixed-function style lighting, `.btg` backgrounds,
//! nav lights, and the game's orbit camera.
//!
//! The renderer knows nothing about windows: give [`Renderer`] a wgpu device and queue and it
//! draws a [`Scene`] into any texture view (a swap-chain image or an [`OffscreenTarget`]).
//!
//! Beyond the basic scene, [`Renderer::render_ex`] takes per-instance [`ShipRenderState`]s
//! (level of detail, `.mad` animation pose, cloak/phase/infection/EMP/tint effects) and an
//! [`EffectBatch`] of sprites, lines, strips and flares; [`trails::ShipTrails`] produces engine
//! trails and glows into such a batch.

pub mod anim;
pub mod assets;
pub mod background;
pub mod camera;
pub mod effects;
pub mod fx;
pub mod gpu;
pub mod lighting;
pub mod lod;
pub mod mesh;
pub mod nebula;
pub mod nis_sun;
pub mod quality;
pub mod render_state;
pub mod tga;
pub mod trails;

pub use anim::{MadClip, ShipAnimator, ANIM_LOOP, ANIM_STOP_AT_END};
pub use assets::{AssetSource, DirSource, ShipDef, ShipModel, TeamColors, TeamScheme};
pub use background::Background;
pub use camera::{CameraTweaks, OrbitCamera};
pub use effects::{Blend, EffectBatch, EffectTexture};
pub use fx::{EffectRenderer, FxOptions, FxView};
pub use gpu::{
    headless_device, headless_device_with_msaa, request_device, EngineEffectAssets, GpuBackground, GpuShip, OffscreenTarget, Renderer, Scene,
    SceneExtras, ShipInstance, TeamPaint, DEPTH_FORMAT, OFFSCREEN_FORMAT,
};
pub use lighting::SceneLighting;
pub use lod::{LodSelector, LodTable};
pub use mesh::{mesh_to_world, Skeleton};
pub use nebula::NebulaRenderer;
pub use nis_sun::NisSunFlare;
pub use quality::{RenderPlan, RenderQuality};
pub use render_state::ShipRenderState;

pub use wgpu;
