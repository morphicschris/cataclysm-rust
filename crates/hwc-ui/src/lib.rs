//! Front end (menu system) of Homeworld: Cataclysm, recreated from the engine's front-end
//! manager: `.fib` screen layouts, the region tree with its mouse/keyboard processing, every UI
//! control type, the textured frame/button drawing from `FeMan\Textures`, `.hff` text, named
//! callbacks, and the 640x480 layout placed on any window size.
//!
//! Rendering is done on the CPU into an RGBA framebuffer ([`canvas::Canvas`]), which is exact for
//! the engine's unblended, alpha-tested 2D drawing; [`present`] shows that frame in a window
//! with wgpu, either at 1:1 like the original or scaled.

pub mod assets;
pub mod canvas;
pub mod cataclysm;
pub mod colpick;
pub mod consts;
pub mod controls;
pub mod credits;
pub mod fe;
pub mod fer;
pub mod ingame;
pub mod known;
pub mod options;
pub mod present;
pub mod region;
pub mod scale;
pub mod session;
pub mod text;

pub use assets::{AssetSource, Language, Resources};
pub use canvas::{Canvas, Rect};
pub use fe::{Call, FrontEnd};
