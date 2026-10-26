//! What the presentation layer needs to draw a ship's special state (read-only views of the
//! simulation; nothing here changes it).

use crate::object::ShipId;
use crate::universe::Universe;

/// Per-ship render state of the special abilities.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct RenderFlags {
    /// Fully cloaked (`cloakIsCloaked`).
    pub cloaked: bool,
    /// Cloak fade while cloaking or decloaking (0 visible .. 1 cloaked), `None` otherwise.
    pub cloak_fade: Option<f32>,
    /// Cloak power (0..1).
    pub cloak_power: f32,
    /// Being infected by the Beast (infection timer running).
    pub beast_infected: bool,
    /// Disabled by an EMP (shocked and the disable timer running).
    pub emp_disabled: bool,
    /// Disabled for any reason (`SOF_Disabled`).
    pub disabled: bool,
    /// Slowed by a field (DFG, Nomad moon).
    pub field_slowed: bool,
    /// Visible to the given player (stealthy types: Mimics, Leech, cloakers).
    pub visible_to_viewer: bool,
    /// A Mimic's disguise: morph progress (1 own shape .. 0 the disguise) and the ship type
    /// shown (`None`: an asteroid or nothing yet).
    pub mimic: Option<(f32, Option<crate::statics::ShipType>)>,
}

/// The render state of `id` as seen by `viewer` (a player index).
pub fn render_flags(u: &Universe, id: ShipId, viewer: usize) -> RenderFlags {
    let Some(s) = u.ships.get(id) else { return RenderFlags::default() };
    RenderFlags {
        cloaked: crate::cloak::is_cloaked(s),
        cloak_fade: crate::cloak::fade_level(s),
        cloak_power: crate::cloak::power_fraction(s),
        beast_infected: s.beast.timer != 0.0,
        emp_disabled: s.disable.emp_shocked && s.disable.disabled,
        disabled: s.flags & crate::object::sof::DISABLED != 0,
        field_slowed: s.disable.slowed,
        visible_to_viewer: crate::visibility::visible_to_player(u, id, viewer),
        mimic: crate::ships::mimic::render_state(u, id),
    }
}
