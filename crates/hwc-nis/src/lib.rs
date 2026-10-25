//! The NIS player of the Homeworld: Cataclysm recreation: the scripted in-engine cinematics
//! (`nis\nNN.nis` and the NISlets) as `NIS.c` in `cataclysm.exe` plays them.
//!
//! * [`data`]: `nisLoad`, the motion paths kept as the game holds them in memory;
//! * [`events`]: the `.script` event list;
//! * [`euler`]: object and camera orientation from heading, pitch and bank;
//! * [`player`]: `nisStart` / `nisUpdate` / `nisSeek` / `nisStop` and the NIS task (fade);
//! * [`stage`]: the NIS objects in an `hwc_sim` universe;
//! * [`tables`]: which NIS plays when in the campaign.
//!
//! The spline maths itself is `hwc_math::spline` (Cataclysm's `bsCurveUpdate`).

// The x87 comparisons the original makes treat NaN in specific ways; `!(a < b)` is written
// deliberately where the binary tests "not less" (true for NaN).
#![allow(clippy::neg_cmp_op_on_partial_ord)]

pub mod data;
pub mod euler;
pub mod events;
pub mod player;
pub mod stage;
pub mod tables;

pub use data::{Category, NisData};
pub use events::{Action, Event};
pub use player::{Fired, NisCamera, NisPlayer, Step, StopReport};
pub use stage::Stage;
