//! What the sandbox window runs: a built-in harness scenario or a single-player mission.

use hwc_sim::harness::Run;
use hwc_sim::mission::MissionRun;
use hwc_sim::{ShipId, Universe};

pub enum Sim {
    Scenario(Box<Run>),
    Mission(Box<MissionRun>),
}

impl Sim {
    pub fn universe(&self) -> &Universe {
        match self {
            Sim::Scenario(r) => &r.universe,
            Sim::Mission(m) => &m.universe,
        }
    }

    pub fn universe_mut(&mut self) -> &mut Universe {
        match self {
            Sim::Scenario(r) => &mut r.universe,
            Sim::Mission(m) => &mut m.universe,
        }
    }

    /// One universe update (with the scenario's orders or the mission's KAS work).
    pub fn step(&mut self) {
        match self {
            Sim::Scenario(r) => {
                r.step();
            }
            Sim::Mission(m) => m.step(),
        }
    }

    /// Ships by creation index (the scenario's spawn order; the ship list for a mission).
    pub fn ids(&self) -> Vec<ShipId> {
        match self {
            Sim::Scenario(r) => r.ids.clone(),
            Sim::Mission(m) => m.universe.ship_list.clone(),
        }
    }

    pub fn mission(&self) -> Option<&MissionRun> {
        match self {
            Sim::Mission(m) => Some(m),
            Sim::Scenario(_) => None,
        }
    }

    pub fn mission_mut(&mut self) -> Option<&mut MissionRun> {
        match self {
            Sim::Mission(m) => Some(m),
            Sim::Scenario(_) => None,
        }
    }
}
