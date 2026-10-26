//! The computer player's team moves (`AIMoves.c`, `AITeam.c`): the moves a KAS team is carrying
//! out, processed once per computer-player pass (`aitExecute`, from `aifFleetCommand`).
//!
//! KAS orders that the original routes through this layer (`MoveTo`, `Intercept`) create a move
//! on the team; the next pass (`univUpdateCounter & 15 == 1` for the single-player computer
//! player, one update after `kasExecute`) gives the order, and later passes watch it until it is
//! done. Direct KAS orders (`Attack`, `Guard`, `Stop`, docking, launching ...) first delete every
//! move of the team (`aitDeleteAllTeamMoves`). A team is idle (`aitTeamIsIdle`) when it has no
//! current move or the current move is a "done" or "get ships" move.

use hwc_math::Vector;

use crate::object::ShipId;

/// Move types (`AIMoveTypes`, Cataclysm numbering) of the moves modelled here.
pub mod move_type {
    pub const DONE: u32 = 0;
    pub const GET_SHIPS: u32 = 4;
    pub const MOVE_TEAM: u32 = 6;
    pub const INTERCEPT: u32 = 9;
    pub const FANCY_GET_SHIPS: u32 = 0x16;
    pub const DOCK: u32 = 0x17;
    pub const LAUNCH: u32 = 0x18;
}

/// `dockmoveFlags` of a dock move (`aimCreateDock`).
pub mod dock_flags {
    /// Dock with the given ship (otherwise each ship finds the nearest).
    pub const AT_SHIP: u32 = 1;
    /// Stay docked until launched explicitly.
    pub const STAY: u32 = 2;
    /// Dock at once (`DOCK_INSTANTANEOUSLY`).
    pub const INSTANT: u32 = 4;
}

/// The formation `aimCreateDock` gives its move (0x740aec).
pub const DOCK_MOVE_FORMATION: i32 = 5;

/// `SAME_FORMATION`: a move that keeps the previous move's formation.
pub const SAME_FORMATION: i32 = 8;
/// `NO_FORMATION`.
pub const NO_FORMATION: i32 = 7;

/// What a move does.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MoveKind {
    /// `MOVE_MOVETEAM` (`aimProcessMove`): the team flies to `destination`.
    MoveTeam { destination: Vector },
    /// `MOVE_INTERCEPT` (`aimProcessIntercept`): the team flies at `target`, re-aiming every
    /// `interval` seconds, until its first ship is within `AIM_INTERCEPT_FINISH_DISTSQ`.
    Intercept {
        target: ShipId,
        interval: f32,
        /// +0x1c: when the destination may be re-aimed next.
        next_time: f32,
        /// +0x20: where the team was sent.
        destination: Vector,
        /// +0x2c: the destination was the target's own destination (it was moving).
        target_moving: bool,
    },
    /// `MOVE_DOCK` (`aimProcessDock`): the team docks (`aiuWrapDock`) and the move is done once
    /// every ship that docked has come back out, or is staying at a carrier's or mothership's
    /// refuelling point.
    Dock {
        /// +0x1c: [`dock_flags`].
        flags: u32,
        /// +0x18: the ship to dock with (cleared when it dies).
        host: Option<ShipId>,
        /// +0x14: the team's ships still docking (`selectMemDupSelection` of the team).
        docking: Vec<ShipId>,
    },
    /// `MOVE_LAUNCH` (`aimProcessLaunch`): the team's ships inside the first ship's host are
    /// launched (`clWrapLaunchMultipleShips`).
    Launch,
}

/// `AITeamMove`, with the fields the modelled moves use.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TeamMove {
    pub kind: MoveKind,
    /// +0x164: the move has given its order (`processing`).
    pub processing: bool,
    /// +0x165: wait for the move to finish before going on.
    pub wait: bool,
    /// +0x166: delete the move once it is done.
    pub remove: bool,
    /// +0x168, +0x16c.
    pub formation: i32,
    pub tactics: i32,
}

impl TeamMove {
    /// The move's type number (`move->type`).
    pub fn move_type(&self) -> u32 {
        match self.kind {
            MoveKind::MoveTeam { .. } => move_type::MOVE_TEAM,
            MoveKind::Intercept { .. } => move_type::INTERCEPT,
            MoveKind::Dock { .. } => move_type::DOCK,
            MoveKind::Launch => move_type::LAUNCH,
        }
    }

    /// `aimCreateDock` (0x40c2f0): tactics 2, the dock move formation.
    pub fn dock(flags: u32, host: Option<ShipId>, wait: bool, remove: bool) -> Self {
        TeamMove { kind: MoveKind::Dock { flags, host, docking: Vec::new() }, processing: false, wait, remove, formation: DOCK_MOVE_FORMATION, tactics: 2 }
    }

    /// `aimCreateLaunch`: tactics 2, `SAME_FORMATION`.
    pub fn launch(wait: bool, remove: bool) -> Self {
        TeamMove { kind: MoveKind::Launch, processing: false, wait, remove, formation: SAME_FORMATION, tactics: 2 }
    }

    /// `aimCreateMoveTeam`: tactics 2 (aggressive).
    pub fn move_team(destination: Vector, formation: i32, wait: bool, remove: bool) -> Self {
        TeamMove { kind: MoveKind::MoveTeam { destination }, processing: false, wait, remove, formation, tactics: 2 }
    }

    /// `aimCreateIntercept`.
    pub fn intercept(target: ShipId, interval: f32, formation: i32, tactics: i32, wait: bool, remove: bool) -> Self {
        TeamMove {
            kind: MoveKind::Intercept { target, interval, next_time: 0.0, destination: Vector::ZERO, target_moving: false },
            processing: false,
            wait,
            remove,
            formation,
            tactics,
        }
    }
}

/// A team's move list (`team->moves`) and its current move (`team->curMove`).
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct TeamMoves {
    pub moves: Vec<TeamMove>,
    pub current: Option<usize>,
    /// `teamDelay`: passes to wait before the next move is processed.
    pub delay: u8,
    /// `TEAM_DOCKING` (`team->teamFlags & 0x4000`): set when a dock move gives its order,
    /// cleared when it is done (`kasfTeamDocking`).
    pub docking: bool,
}

impl TeamMoves {
    /// `aitDeleteAllTeamMoves`.
    pub fn delete_all(&mut self) {
        self.moves.clear();
        self.current = None;
    }

    /// `listAddNode` on the team's moves.
    pub fn add(&mut self, m: TeamMove) {
        self.moves.push(m);
    }

    /// `aitTeamIsIdle` (0x417270).
    pub fn is_idle(&self) -> bool {
        match self.current.and_then(|i| self.moves.get(i)) {
            None => true,
            Some(m) => matches!(m.move_type(), move_type::DONE | move_type::GET_SHIPS | move_type::FANCY_GET_SHIPS),
        }
    }

    /// `aitTeamIsDone` (0x417250).
    pub fn is_done(&self) -> bool {
        self.current.and_then(|i| self.moves.get(i)).is_none_or(|m| m.move_type() == move_type::DONE)
    }

    /// `aitAddmoveBeforeAndMakeCurrent`: `m` goes in front of move `before` and is current.
    pub fn add_before_and_make_current(&mut self, before: usize, m: TeamMove) {
        self.moves.insert(before, m);
        self.current = Some(before);
    }
}

/// `AIT_TEAM_MOVE_DELAY[TEAM_ADVANCED]` (`aiplayer.script`): the single-player computer player
/// plays at `AI_ADV`, so its teams are advanced teams and wait no passes between moves.
pub const KAS_TEAM_MOVE_DELAY: u8 = 0;

/// `AIM_INTERCEPT_FINISH_DISTSQ` (`aiplayer.script`; 4e6 compiled in).
pub const INTERCEPT_FINISH_DISTSQ: f32 = 16_000_000.0;
/// `AIM_INTERCEPT_RECALC_DISTSQ`.
pub const INTERCEPT_RECALC_DISTSQ: f32 = 2_000_000.0;
/// The range `aimProcessMove` passes to `aitTeamIsFinishedMoving` (0x740aa4).
pub const MOVE_FINISHED_RANGE: f32 = 1300.0;
