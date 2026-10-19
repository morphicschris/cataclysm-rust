//! Effects for the Homeworld: Cataclysm recreation.
//!
//! * [`compiler`]: the ETG ("Effects Tool of the Gods") compiler, a port of `etgEffectCodeLoad`
//!   that turns the preprocessed `.ebg` effect scripts into byte-code ([`bytecode`]).
//! * [`scripts`]: the `ETG\*.script` tables binding effects to deaths, guns, damage and so on.
//! * [`runtime`]: effect instances and the interpreter (`etgEffectUpdate`, `etgEffectDraw`),
//!   with the functions scripts call ([`functions`]).
//! * [`particle`]: the particle system (`Particle.c`), including Cataclysm's electric bolts and
//!   lens flares.
//! * [`render`]/[`emit`]: camera-independent render data for a renderer to draw.
//!
//! See `docs/formats/etg.md` for the script language and byte-code layout.

pub mod bytecode;
pub mod cfmt;
pub mod compiler;
pub mod emit;
pub mod eval;
pub mod functions;
pub mod particle;
pub mod render;
pub mod runtime;
pub mod scripts;

pub use compiler::{CompileError, EffectStatic, FileSource, Library};
pub use render::RenderList;
pub use runtime::{FxEvent, FxWorld, Owner};
pub use scripts::{load_scripts, EtgTables};
