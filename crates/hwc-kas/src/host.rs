//! The interface between the interpreter and the game.
//!
//! [`KasRuntime`] (generated from the kas2c function table) has one typed method per game-side
//! runtime function; [`KasHost`] adds the few services the KAS runtime itself needs (game time,
//! the random number stream, the language) plus optional hooks. Arguments reach the host
//! resolved: `THISTEAM`-style references are replaced by the current team's label, localised
//! strings by the text for the current language.

use std::fmt;

use crate::functions::FuncId;
use crate::Hash;

#[path = "generated/runtime.rs"]
mod runtime;

pub use runtime::KasRuntime;
pub(crate) use runtime::dispatch;

/// `CurrentMissionScope`: which level of code is running.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum Scope {
    /// Mission-level code (variables and timers live at mission scope).
    #[default]
    Mission,
    /// FSM-level code of the current team.
    Fsm,
    /// State code of the current team.
    State,
}

/// The calling context of a runtime function.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cx {
    /// Label of the current team (`CurrentTeamP`), if any. Mission-level code sees whatever team
    /// ran last, as in the original.
    pub team: Option<Hash>,
    /// The running code's scope.
    pub scope: Scope,
    /// `CurrentMissionScopeName`: the mission name hash at mission level, else the team label.
    pub scope_name: Hash,
    /// Number of `kasExecute` calls so far (0 during `kasMissionStart`).
    pub tick: u64,
}

/// A team argument: the team's label, or `None` for NULL.
pub type Team = Option<Hash>;

/// A resolved ship selection argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Ships {
    /// NULL.
    Null,
    /// A named KAS selection (`SHIPS_x`), created on first use by the game.
    Selection(Hash),
    /// A team's ship list (`TEAMSHIPS_x`, `THISTEAMSHIPS`).
    Team(Hash),
}

/// A resolved position argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Point {
    /// NULL.
    Null,
    /// A labelled point (`POINT_x`).
    Label(Hash),
    /// Centre of a named selection (`SHIPSPOINT_x`).
    Ships(Hash),
    /// Centre of a team's ships (`TEAMSPOINT_x`, `THISTEAMSPOINT`).
    Team(Hash),
    /// Centre of a labelled volume (`VOLUMEPOINT_x`).
    Volume(Hash),
}

/// A resolved string argument.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Str<'a> {
    /// NULL.
    Null,
    /// An integer passed where a string was expected.
    Int(i32),
    /// A literal.
    Text(&'a str),
    /// A localised string: the table index and the text for the current language.
    Local {
        /// Index into the mission's LSTRING tables.
        index: usize,
        /// Text in the current language.
        text: &'a str,
    },
}

impl<'a> Str<'a> {
    /// The text, if this is a string.
    pub fn text(self) -> Option<&'a str> {
        match self {
            Str::Text(t) | Str::Local { text: t, .. } => Some(t),
            Str::Null | Str::Int(_) => None,
        }
    }
}

/// An evaluated argument (used for dispatch and tracing).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Value<'a> {
    Int(i32),
    Hash(Hash),
    Str(Str<'a>),
    Ships(Ships),
    Point(Point),
    Team(Team),
    Volume(Option<Hash>),
    Path(Option<Hash>),
}

impl<'a> Value<'a> {
    /// As an integer (labels give their hash, NULL gives 0).
    pub fn int(self) -> i32 {
        match self {
            Value::Int(v) | Value::Str(Str::Int(v)) => v,
            Value::Hash(h) => h as i32,
            Value::Team(t) | Value::Volume(t) | Value::Path(t) => t.unwrap_or(0) as i32,
            _ => 0,
        }
    }

    /// As a hash.
    pub fn hash(self) -> Hash {
        self.int() as u32
    }

    /// As a string.
    pub fn str(self) -> Str<'a> {
        match self {
            Value::Str(s) => s,
            Value::Int(0) => Str::Null,
            Value::Int(v) => Str::Int(v),
            _ => Str::Null,
        }
    }

    /// As a ship selection.
    pub fn ships(self) -> Ships {
        match self {
            Value::Ships(s) => s,
            Value::Team(Some(t)) => Ships::Team(t),
            _ => Ships::Null,
        }
    }

    /// As a position.
    pub fn point(self) -> Point {
        match self {
            Value::Point(p) => p,
            _ => Point::Null,
        }
    }

    /// As a team.
    pub fn team(self) -> Team {
        match self {
            Value::Team(t) => t,
            _ => None,
        }
    }

    /// As a volume.
    pub fn volume(self) -> Option<Hash> {
        match self {
            Value::Volume(v) => v,
            _ => None,
        }
    }

    /// As a path.
    pub fn path(self) -> Option<Hash> {
        match self {
            Value::Path(v) => v,
            _ => None,
        }
    }
}

impl fmt::Display for Value<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Value::Int(v) => write!(f, "{v}"),
            Value::Hash(h) => write!(f, "#{h:08x}"),
            Value::Str(Str::Null) | Value::Ships(Ships::Null) | Value::Point(Point::Null) => f.write_str("NULL"),
            Value::Team(None) | Value::Volume(None) | Value::Path(None) => f.write_str("NULL"),
            Value::Str(Str::Int(v)) => write!(f, "{v}"),
            Value::Str(Str::Text(t)) => write!(f, "{t:?}"),
            Value::Str(Str::Local { index, .. }) => write!(f, "LSTRING[{index}]"),
            Value::Ships(Ships::Selection(h)) => write!(f, "SHIPS_{h:08x}"),
            Value::Ships(Ships::Team(h)) => write!(f, "TEAMSHIPS_{h:08x}"),
            Value::Point(Point::Label(h)) => write!(f, "POINT_{h:08x}"),
            Value::Point(Point::Ships(h)) => write!(f, "SHIPSPOINT_{h:08x}"),
            Value::Point(Point::Team(h)) => write!(f, "TEAMSPOINT_{h:08x}"),
            Value::Point(Point::Volume(h)) => write!(f, "VOLUMEPOINT_{h:08x}"),
            Value::Team(Some(h)) => write!(f, "TEAM_{h:08x}"),
            Value::Volume(Some(h)) => write!(f, "VOLUME_{h:08x}"),
            Value::Path(Some(h)) => write!(f, "PATH_{h:08x}"),
        }
    }
}

/// Services the KAS runtime needs from the game, plus optional observation hooks.
///
/// Everything has a default so a headless host can be `impl KasRuntime for H {}` plus
/// `impl KasHost for H {}`; time then stands still (timers never run down) and `Random` always
/// returns its lower bound.
#[allow(unused_variables)]
pub trait KasHost: KasRuntime {
    /// `universe.totaltimeelapsed`: game time in seconds, read by every timer operation.
    fn universe_time(&mut self) -> f32 {
        0.0
    }

    /// `ranRandomFn(stream)`: the game's random number generator (`Random` uses stream 6).
    fn random(&mut self, stream: u32) -> u32 {
        0
    }

    /// `strCurLanguage`: index into the LSTRING tables (0 English, 1 French, 2 German).
    fn language(&self) -> usize {
        0
    }

    /// Called after every runtime call (builtins included) with its arguments and result.
    fn trace_call(&mut self, cx: &Cx, func: FuncId, args: &[Value<'_>], result: i32) {}

    /// `kasFSMCreate` gave `team` to FSM `fsm` (the original also sets the team to script control,
    /// neutral tactics and no formation; the game side owns those fields).
    fn fsm_created(&mut self, team: Hash, fsm: Hash) {}

    /// `kasJump`: `team` entered state `state`.
    fn state_changed(&mut self, team: Hash, state: Hash) {}

    /// Something the original would have crashed on or reported (a NULL team, division by zero,
    /// runaway JUMP recursion). The interpreter carries on.
    fn warning(&mut self, cx: &Cx, message: &str) {}
}
