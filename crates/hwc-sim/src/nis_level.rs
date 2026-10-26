//! The NIS levels of the campaign (`SinglePlayer\MissionNN_NISMM`): the cut scenes that play
//! in a level of their own before a mission (the opening of mission 1, and between missions
//! 3 and 4, 5 and 6, ...).
//!
//! The original (`singlePlayerGameUpdate` state 5, `singlePlayerStartNis` 0x516790,
//! `singlePlayerLoadNewLevel(1, ..)` 0x515da0, `singlePlayerNisStoppedCB` 0x516550): when the
//! jump out of mission N ends and mission N+1 has an NIS level (table 0x88a3f4, its NIS number
//! in 0x88a440), the universe is reset (the fleet stays in the hyperspace list), the NIS level
//! is loaded, the NIS is loaded and started centred on the level's `StartPointPlayer` (turned by
//! its angle), or on the ship of the KAS selection `NisCentreShip`, or on the derelict whose type
//! is `G_NisCentreDerelict` (instance `G_NisCentreInstance`); the single-player state is 8 while
//! it plays. When it stops, `G_Nis<M>Complete` is set and the mission's own level is loaded
//! (state 6, the mission's KAS starts on the next update).
//!
//! [`NisLevelRun`] is that phase for a front end: [`NisLevelRun::start`] loads the NIS level
//! (with the carried fleet in hyperspace, so the NIS can dress the mothership with the modules
//! the fleet has), the front end plays the NIS from [`NisLevelRun::nis_files`] at
//! [`NisLevelRun::placement`], steps the universe as it likes, and calls
//! [`NisLevelRun::finish`] when the NIS has stopped, which loads the mission.

use std::sync::Arc;

use hwc_kas::Mission as KasMission;
use hwc_math::fpu::{ld, st};
use hwc_math::Vector;

use crate::campaign::Carry;
use crate::level::{self, LevelContext, LevelState, ResourceTweaks};
use crate::mission::{FleetShip, Game, MissionConfig, MissionError, MissionRun};
use crate::object::ShipId;

use crate::statics::{DataSource, ShipRace, StaticWorld};
use crate::universe::Universe;
use crate::SimTweaks;

/// `0x88a3f4[mission]`: the mission starts with an NIS level.
const HAS_NIS_LEVEL: [u8; 21] = [0, 1, 0, 0, 1, 0, 1, 1, 0, 0, 1, 0, 1, 0, 1, 1, 1, 1, 0, 0, 1];
/// `0x88a440[mission]`: the NIS number of the mission's NIS (-1: none).
const NIS_NUMBER: [i8; 19] = [0, 1, -1, -1, 2, -1, 4, 5, -1, -1, 6, -1, 7, -1, 8, 9, 10, 11, 0];

/// The NIS level mission `mission` starts with, by NIS number (`Mission04_NIS02` for 4).
pub fn nis_level_before(mission: u32) -> Option<u32> {
    let m = mission as usize;
    if HAS_NIS_LEVEL.get(m).copied().unwrap_or(0) == 0 {
        return None;
    }
    NIS_NUMBER.get(m).copied().filter(|&n| n > 0).map(|n| n as u32)
}

/// The NIS a mission plays after its arrival when it has no NIS level (`0x88a440[mission]`
/// with `0x88a3f4[mission]` clear, `singlePlayerGameUpdate` state 9).
pub fn nis_after_arrival(mission: u32) -> Option<u32> {
    let m = mission as usize;
    if HAS_NIS_LEVEL.get(m).copied().unwrap_or(1) != 0 {
        return None;
    }
    NIS_NUMBER.get(m).copied().filter(|&n| n > 0).map(|n| n as u32)
}

/// Where `singlePlayerStartNis` centres the NIS.
#[derive(Debug, Clone, PartialEq)]
pub struct NisPlacement {
    /// `StartPointPlayer` of the NIS level (the origin when it has none).
    pub position: Vector,
    /// Its fourth component (degrees) in radians: the NIS is turned about z by it.
    pub angle: f32,
    /// The first ship of the KAS selection `NisCentreShip`, when the mission left one.
    pub centre_ship: Option<ShipId>,
    /// `G_NisCentreDerelict` / `G_NisCentreInstance` and the level derelict of that type
    /// (index into [`LevelState::derelicts`]), when the mission set them: the NIS's own object
    /// of that type and instance is moved onto the derelict (`nisShipStartPosition`).
    pub centre_derelict: Option<(i32, Option<i32>, Option<usize>)>,
}

/// The NIS level phase before a mission.
#[derive(Debug, Clone)]
pub struct NisLevelRun {
    /// The universe with the NIS level in it (and the carried fleet in hyperspace).
    pub universe: Universe,
    pub level: LevelState,
    /// The mission the NIS leads into.
    pub mission: u32,
    /// The NIS number (`nis\n<NN>.nis`).
    pub nis: u32,
    pub placement: NisPlacement,
    /// The carried fleet (`None` for the opening of mission 1).
    pub carry: Option<Carry>,
}

impl NisLevelRun {
    /// `singlePlayerStartNis(.., load level)`: when `mission` starts with an NIS level, loads
    /// it (`None` otherwise: the mission loads at once). `carry` is the fleet from the previous
    /// mission (`None` for mission 1). `centre` is the previous mission's `G_NisCentreDerelict`
    /// and `G_NisCentreInstance`, when set.
    pub fn start(
        statics: Arc<StaticWorld>,
        tweaks: SimTweaks,
        src: &dyn DataSource,
        mission: u32,
        carry: Option<&Carry>,
        centre: Option<(i32, Option<i32>)>,
    ) -> Result<Option<Self>, MissionError> {
        let Some(nis) = nis_level_before(mission) else { return Ok(None) };
        let mut universe = Universe::new(statics, tweaks, &[ShipRace::Somtaaw, ShipRace::Taiidan]);
        universe.single_player = true;
        universe.base.options.mission = mission as i32;
        crate::base::init(&mut universe);
        if let Some(c) = carry {
            crate::campaign::restore_player(&mut universe, c);
            universe.total_time_elapsed = c.total_time;
        }
        let mut level = LevelState::default();
        let ctx = LevelContext { single_player: true, mission, fleet_modifier: 0.0, resources: ResourceTweaks::load(src) };
        let (dir, file) = level::mission_paths(mission, Some(nis));
        level::load_level(&mut universe, src, &dir, &file, &ctx, &mut level)?;
        level::preset_ships_to_position(&mut universe);
        // The fleet waits in hyperspace while the NIS plays.
        if let Some(c) = carry {
            let mut game = Game::for_fleet(mission, crate::campaign::wipe_unavailable_modules(&c.fleet, mission));
            crate::campaign::create_fleet_in_hyperspace(&mut universe, &mut game);
        }
        // singlePlayerLoadNewLevel(1, ..) ends with levelFogOfWarCreate too.
        crate::fow::create_for_local_player(&mut universe, crate::mission::HUMAN_PLAYER);
        let sp = level.point(hwc_kas::kas_hash("StartPointPlayer")).unwrap_or([0.0; 4]);
        let centre_derelict = centre.map(|(t, inst)| {
            let found = level.derelicts.iter().position(|d| d.derelict_type.index() as i32 == t);
            (t, inst, found)
        });
        let placement = NisPlacement {
            position: Vector::new(sp[0], sp[1], sp[2]),
            angle: st(ld(sp[3]) * ld(level::DEG_TO_RAD)),
            centre_ship: None,
            centre_derelict,
        };
        Ok(Some(NisLevelRun { universe, level, mission, nis, placement, carry: carry.cloned() }))
    }

    /// The NIS to play: `nis\n<NN>.nis` and its script.
    pub fn nis_files(&self) -> (String, String) {
        (format!("nis\\n{:02}.nis", self.nis), format!("nis\\n{:02}.script", self.nis))
    }

    /// `singlePlayerNisStoppedCB` + `singlePlayerLoadNewLevel(0, 0)`: the NIS has stopped; the
    /// mission's level is loaded with the fleet (the game clock goes on from the NIS level) and
    /// `G_Nis<NN>Complete` is set for the mission's KAS.
    pub fn finish(self, statics: Arc<StaticWorld>, tweaks: SimTweaks, src: &dyn DataSource, script: KasMission, config: MissionConfig) -> Result<MissionRun, MissionError> {
        let mut run = match &self.carry {
            None => MissionRun::start(statics, tweaks, src, script, config)?,
            Some(c) => {
                let mut c = c.clone();
                c.total_time = self.universe.total_time_elapsed;
                MissionRun::start_campaign(statics, tweaks, src, script, config, &c)?
            }
        };
        run.kas.set_var(None, hwc_kas::var_hash(&format!("G_Nis{}Complete", self.nis)), 1);
        run.game.log(crate::mission::LogKind::Flow, format!("NIS level {} played; mission {} loads", self.nis, self.mission));
        Ok(run)
    }
}

impl Game {
    /// A bare game state holding a fleet for `mission` (used to put a fleet into hyperspace
    /// outside a mission run).
    pub(crate) fn for_fleet(mission: u32, fleet: Vec<FleetShip>) -> Game {
        let mut g = crate::mission::empty_game(MissionConfig::new(mission));
        g.fleet = fleet;
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nis_levels_follow_the_tables() {
        let with: Vec<(u32, u32)> = (1..=17).filter_map(|m| nis_level_before(m).map(|n| (m, n))).collect();
        assert_eq!(with, vec![(1, 1), (4, 2), (6, 4), (7, 5), (10, 6), (12, 7), (14, 8), (15, 9), (16, 10), (17, 11)]);
        assert_eq!(nis_after_arrival(2), None);
        assert_eq!(nis_after_arrival(5), None);
    }
}
