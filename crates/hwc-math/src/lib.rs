//! Deterministic math foundation for the Homeworld: Cataclysm recreation.
//!
//! Everything in this crate aims to reproduce what `cataclysm.exe` computes, bit for bit,
//! wherever the simulation depends on it. The original is MSVC6 code running on the x87
//! FPU, so the crate is organised around a model of that FPU (see [`fpu`]):
//!
//! * **Intermediates are `f64`.** The game runs with the x87 precision-control field set to
//!   53 bits (see [`fpu`] for the evidence), so every value that lives in an FPU register is
//!   exactly an IEEE double. MSVC6 does not honour `(float)` casts on register values; a
//!   value is rounded to `f32` only when it is stored to a `float` in memory.
//! * **Stores are `f32`.** Struct fields, globals and spilled locals are `real32`, so the
//!   functions here round to `f32` exactly where the original executes `FSTP dword`.
//! * **Evaluation order follows the binary**, not the C source: the compiler re-associates
//!   sums (e.g. `hmatMultiplyHMatByHVec`), and the hand-written x87 assembly in `Matrix.c`
//!   pairs terms its own way. Every order here was traced from the disassembly.
//!
//! Modules:
//! * [`fpu`] — control-word findings, `__ftol`, x87 compare semantics, the 1.5·2^52 rounding trick.
//! * [`fastmath`] — the table-driven `fmathSqrtDouble` (used for every vector length).
//! * [`trig`] — x87 `FSIN`/`FCOS` emulation (66-bit π reduction).
//! * [`vector`], [`matrix`] — `Vector.c`, `Matrix.c`, `Matrix-mult.c` and Cataclysm additions.
//! * [`random`] — `Randy.c` multi-stream generator (27 streams in Cataclysm).
//! * [`crt`] — MSVC6 CRT behaviour: `qsort`, `rand`.
//! * [`spline`] — `B-Spline.c` (Cataclysm's modified `bsCurveUpdate`).
//! * [`timing`] — task-scheduler and universe update-rate constants and tick accounting.

pub mod crt;
pub mod fastmath;
pub mod fpu;
pub mod matrix;
pub mod random;
pub mod serde_array;
pub mod spline;
pub mod timing;
pub mod trig;
pub mod vector;

pub use matrix::{HMatrix, Matrix};
pub use random::Randy;
pub use vector::{HVector, Vector};

/// `PI` as the game defines it (`3.14159265358979f`, rounded to `f32`).
pub const PI: f32 = std::f32::consts::PI;
/// `TWOPI` (`2.0f * PI`).
pub const TWOPI: f32 = 2.0 * PI;
/// Sentinel "really big" value (`(real32)10e20`).
pub const REALLY_BIG: f32 = 10e20;
/// Sentinel "really small" value (`(real32)10e-20`).
pub const REALLY_SMALL: f32 = 10e-20;
/// Sentinel "really negative" value (`(real32)-10e20`).
pub const REALLY_NEGATIVE: f32 = -10e20;
