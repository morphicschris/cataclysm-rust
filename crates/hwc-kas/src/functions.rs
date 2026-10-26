//! The kas2c runtime function table.
//!
//! Cataclysm's kas2c compiles each KAS function call to a direct call of a `kasf*` C function.
//! The executable embeds the full table (for its debug console): KAS name, C name, whether the
//! function returns a value, and the parameter names and types. Mission IR refers to runtime
//! functions by their index in that table ([`FuncId`]).

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

#[path = "generated/table.rs"]
mod table;

pub use table::FUNCTIONS;

/// Index of a runtime function in the kas2c function table (0..486).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct FuncId(pub u16);

impl FuncId {
    /// The table entry. Panics on an id outside the table.
    pub fn info(self) -> &'static FuncInfo {
        &FUNCTIONS[usize::from(self.0)]
    }

    /// The KAS name, or `"?"` for an id outside the table.
    pub fn name(self) -> &'static str {
        FUNCTIONS.get(usize::from(self.0)).map_or("?", |f| f.kas)
    }

    /// Looks a function up by its KAS name (case-insensitive, like kas2c).
    pub fn by_name(name: &str) -> Option<FuncId> {
        static MAP: OnceLock<HashMap<String, u16>> = OnceLock::new();
        let map = MAP.get_or_init(|| {
            let mut m = HashMap::new();
            for (i, f) in FUNCTIONS.iter().enumerate() {
                m.entry(f.kas.to_ascii_lowercase()).or_insert(i as u16);
            }
            m
        });
        map.get(&name.to_ascii_lowercase()).map(|&i| FuncId(i))
    }
}

impl fmt::Display for FuncId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Parameter types of the kas2c table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamType {
    /// `GROWSELECTIONPTR`: a ship selection (`SHIPS_x`, `TEAMSHIPS_x`, `THISTEAMSHIPS`).
    Ships,
    /// `SDWORD`: a signed 32-bit integer expression.
    Int,
    /// `CHARPTR`: a string (literal or localised `LSTRING_x`).
    Str,
    /// `VECTORPTR`: a position (`POINT_x`, `SHIPSPOINT_x`, `TEAMSPOINT_x`, `VOLUMEPOINT_x`, `THISTEAMSPOINT`).
    Point,
    /// `UDWORD`: a hashed label (variable, timer, message or point name).
    Hash,
    /// `AITEAMPTR`: a team (`TEAM_x`, `THISTEAM`).
    Team,
    /// `VOLUMEPTR`: a labelled volume.
    Volume,
    /// `BOOL`: an integer used as a flag.
    Bool,
    /// `PATHPTR`: a labelled path.
    Path,
}

/// Runtime functions whose state lives in the KAS runtime itself; the interpreter implements them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[allow(missing_docs)]
pub enum Builtin {
    TimerCreate,
    TimerSet,
    TimerStart,
    TimerCreateSetStart,
    TimerStop,
    TimerRemaining,
    TimerExpired,
    TimerExpiredDestroy,
    TimerDestroy,
    VarCreate,
    VarSet,
    VarCreateSet,
    VarInc,
    VarDec,
    VarGet,
    VarDestroy,
    MsgSend,
    MsgSendAll,
    MsgReceived,
    MsgQueueFlush,
    ThisTeamIs,
    Random,
    PauseOtherKAS,
    UnpauseOtherKAS,
}

/// One entry of the kas2c function table.
#[derive(Debug)]
pub struct FuncInfo {
    /// Name in KAS source.
    pub kas: &'static str,
    /// Name of the C function kas2c calls.
    pub c: &'static str,
    /// Whether the function returns a (signed 32-bit) value.
    pub returns: bool,
    /// Parameter names and types.
    pub params: &'static [(&'static str, ParamType)],
    /// Set when the interpreter implements the function itself.
    pub builtin: Option<Builtin>,
}

/// Well-known table indices used by the interpreter and the loaders.
pub mod ids {
    use super::FuncId;
    /// `VarCreateSet(name, value)`.
    pub const VAR_CREATE_SET: FuncId = FuncId(11);
    /// `VarGet(name)`.
    pub const VAR_GET: FuncId = FuncId(14);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_shape() {
        assert_eq!(FUNCTIONS.len(), 486);
        assert_eq!(FUNCTIONS[ids::VAR_CREATE_SET.0 as usize].kas, "VarCreateSet");
        assert_eq!(FUNCTIONS[ids::VAR_GET.0 as usize].kas, "VarGet");
        assert_eq!(FuncId::by_name("varget"), Some(ids::VAR_GET));
        assert_eq!(FuncId::by_name("ShipsSetColour").map(|f| f.name()), Some("ShipsSetColour"));
        let builtins = FUNCTIONS.iter().filter(|f| f.builtin.is_some()).count();
        assert_eq!(builtins, 24);
        for f in FUNCTIONS.iter() {
            assert_eq!(FuncId::by_name(f.kas).map(|i| i.info().kas), Some(f.kas));
        }
    }
}
