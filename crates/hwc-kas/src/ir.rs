//! The mission IR: a complete, executable description of one compiled KAS mission.
//!
//! The IR mirrors what kas2c generated: a mission has mission-level INIT/WATCH code and a list of
//! FSMs; each FSM has its own INIT/WATCH code and a list of states, each with INIT/WATCH code.
//! Code is a tree of statements; runtime calls refer to the kas2c function table by index
//! ([`FuncId`]) with arguments typed by the table. Labels are kept as hashes ([`Hash`](type@crate::Hash)(type@crate::Hash)); names,
//! when known, live in [`Mission::names`] and are only used for display.

use std::collections::BTreeMap;

use crate::functions::{FuncId, ParamType};
use crate::Hash;

/// How faithfully a mission IR represents the compiled code.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Fidelity {
    /// Built from the machine code (exact).
    Exact,
    /// Parsed from the `.kas` text rendering, which loses some detail (IFONCE variable hashes,
    /// signed/unsigned comparisons, single evaluation of switch values). See [`crate::text`].
    Text,
}

/// One mission (or the tutorial).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Mission {
    /// Script name as passed to `kasMissionStart`, e.g. `Mission01` (any case).
    pub name: String,
    /// Hash of the name: the scope name of mission-level code.
    pub name_hash: Hash,
    /// Localised string tables (`LSTRING_x`), English/French/German.
    pub lstrings: Vec<LString>,
    /// FSMs. [`Stmt::FsmCreate`] refers to them by index.
    pub fsms: Vec<Fsm>,
    /// Mission-level INIT (runs once from `kasMissionStart`).
    pub init: Block,
    /// Mission-level WATCH (runs first on every KAS tick).
    pub watch: Block,
    /// Known label names, by hash (display only; never used for execution).
    pub names: BTreeMap<Hash, String>,
    /// Where the IR came from.
    pub fidelity: Fidelity,
}

/// A localised string table: three `char*` (English, French, German).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LString {
    /// Display name (`LSTRING_<name>` in the text form), if any.
    pub name: Option<String>,
    /// Address of the table in the executable, if known.
    pub addr: Option<u32>,
    /// The texts, indexed by language.
    pub texts: [String; 3],
}

/// An FSM: code that controls one team.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Fsm {
    /// FSM label hash.
    pub hash: Hash,
    /// Runs when the FSM is created for a team (`FSMCreate`).
    pub init: Block,
    /// Runs every KAS tick before the current state's WATCH.
    pub watch: Block,
    /// States. [`Stmt::Jump`] refers to them by index.
    pub states: Vec<State>,
}

/// One FSM state.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct State {
    /// State label hash.
    pub hash: Hash,
    /// Runs on `JUMP` to this state.
    pub init: Block,
    /// Runs every KAS tick while the team is in this state.
    pub watch: Block,
}

/// An INIT or WATCH body (one kas2c-generated C function).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    /// Address of the compiled function, if known.
    pub addr: Option<u32>,
    /// The statements.
    pub stmts: Vec<Stmt>,
}

/// A statement.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Stmt {
    /// A runtime call whose result (if any) is discarded.
    Call(Call),
    /// `IF (cond) then ELSE otherwise ENDIF` (ELSEIF is a nested `If` alone in `otherwise`).
    If {
        /// Condition.
        cond: Cond,
        /// Taken when the condition holds.
        then: Vec<Stmt>,
        /// Taken otherwise (may be empty).
        otherwise: Vec<Stmt>,
    },
    /// `IFONCE (cond) body ENDIFONCE`, compiled as
    /// `if (!VarGet(var) && cond) { VarCreateSet(var, 1); body } else { otherwise }`.
    IfOnce {
        /// The guard variable (a scoped variable like any other).
        var: Hash,
        /// The condition; `None` for `IFONCE (TRUE)`.
        cond: Option<Cond>,
        /// Body, run at most once per guard variable.
        body: Vec<Stmt>,
        /// The else part of the compiled `if` (normally empty).
        otherwise: Vec<Stmt>,
    },
    /// A `switch` over a value evaluated once (kas2c/MSVC output of IF chains on one value).
    Switch {
        /// The value.
        value: Expr,
        /// Arms, tested in order.
        arms: Vec<SwitchArm>,
        /// Taken when no arm matches.
        default: Option<Vec<Stmt>>,
    },
    /// Cataclysm message handlers: a loop over the current team's message queue that dispatches
    /// on each message hash. Messages without a handler are re-queued (unless `default` is set).
    MsgLoop {
        /// Handlers, by message hash.
        handlers: Vec<MsgHandler>,
        /// Code for unhandled messages instead of re-queueing them.
        default: Option<Vec<Stmt>>,
    },
    /// `JUMP state` (changes the current team's state, runs its INIT, then returns).
    Jump {
        /// Index into the owning FSM's states.
        state: usize,
        /// The state's hash.
        hash: Hash,
    },
    /// `FSMCreate(fsm, team)`.
    FsmCreate {
        /// Index into [`Mission::fsms`].
        fsm: usize,
        /// The team that the FSM takes over.
        team: TeamRef,
    },
    /// `RETURN` (leaves the INIT/WATCH function).
    Return,
}

/// One arm of a [`Stmt::Switch`].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SwitchArm {
    /// Values that select this arm.
    pub values: Vec<i32>,
    /// Code.
    pub body: Vec<Stmt>,
}

/// One message handler of a [`Stmt::MsgLoop`].
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MsgHandler {
    /// Message hashes handled.
    pub msgs: Vec<Hash>,
    /// Code.
    pub body: Vec<Stmt>,
}

/// A boolean condition with C short-circuit semantics.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Cond {
    /// Always true.
    True,
    /// Always false.
    False,
    /// Logical not.
    Not(Box<Cond>),
    /// Short-circuit and (left first).
    And(Box<Cond>, Box<Cond>),
    /// Short-circuit or (left first).
    Or(Box<Cond>, Box<Cond>),
    /// Integer comparison.
    Cmp(CmpOp, Expr, Expr),
}

impl Cond {
    /// `e != 0`: the C truth value of an expression.
    pub fn truth(e: Expr) -> Cond {
        Cond::Cmp(CmpOp::Ne, e, Expr::Int(0))
    }
}

/// Comparison operators (signed unless prefixed with U).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[allow(missing_docs)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    ULt,
    ULe,
    UGt,
    UGe,
}

impl CmpOp {
    /// Evaluates the comparison.
    pub fn eval(self, a: i32, b: i32) -> bool {
        let (ua, ub) = (a as u32, b as u32);
        match self {
            CmpOp::Eq => a == b,
            CmpOp::Ne => a != b,
            CmpOp::Lt => a < b,
            CmpOp::Le => a <= b,
            CmpOp::Gt => a > b,
            CmpOp::Ge => a >= b,
            CmpOp::ULt => ua < ub,
            CmpOp::ULe => ua <= ub,
            CmpOp::UGt => ua > ub,
            CmpOp::UGe => ua >= ub,
        }
    }

    /// The comparison that holds exactly when this one does not.
    pub fn negate(self) -> CmpOp {
        match self {
            CmpOp::Eq => CmpOp::Ne,
            CmpOp::Ne => CmpOp::Eq,
            CmpOp::Lt => CmpOp::Ge,
            CmpOp::Le => CmpOp::Gt,
            CmpOp::Gt => CmpOp::Le,
            CmpOp::Ge => CmpOp::Lt,
            CmpOp::ULt => CmpOp::UGe,
            CmpOp::ULe => CmpOp::UGt,
            CmpOp::UGt => CmpOp::ULe,
            CmpOp::UGe => CmpOp::ULt,
        }
    }
}

/// A signed 32-bit integer expression (KAS has no floating point).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Expr {
    /// Constant.
    Int(i32),
    /// Runtime call (must return a value).
    Call(Box<Call>),
    /// Binary arithmetic, wrapping like the compiled 32-bit code.
    Bin(BinOp, Box<Expr>, Box<Expr>),
    /// Two's complement negation.
    Neg(Box<Expr>),
    /// Bitwise not.
    BitNot(Box<Expr>),
    /// A condition used as a value: 1 or 0.
    Cond(Box<Cond>),
}

/// Binary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BinOp {
    /// Wrapping add.
    Add,
    /// Wrapping subtract.
    Sub,
    /// Wrapping multiply.
    Mul,
    /// Signed division, truncating toward zero.
    Div,
    /// Signed remainder.
    Rem,
    /// Bitwise and.
    And,
    /// Bitwise or.
    Or,
    /// Bitwise xor.
    Xor,
    /// Shift left (count masked to 5 bits).
    Shl,
    /// Arithmetic shift right (count masked to 5 bits).
    Shr,
}

impl BinOp {
    /// Source spelling.
    pub fn symbol(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Rem => "%",
            BinOp::And => "&",
            BinOp::Or => "|",
            BinOp::Xor => "^",
            BinOp::Shl => "<<",
            BinOp::Shr => ">>",
        }
    }

    /// Precedence used by the text form (higher binds tighter); `&`, `^` and `|` share a level.
    pub fn precedence(self) -> u8 {
        match self {
            BinOp::Mul | BinOp::Div | BinOp::Rem => 5,
            BinOp::Add | BinOp::Sub => 4,
            BinOp::Shl | BinOp::Shr => 3,
            BinOp::And | BinOp::Or | BinOp::Xor => 2,
        }
    }

    /// Evaluates the operator; `None` on division by zero.
    pub fn eval(self, a: i32, b: i32) -> Option<i32> {
        Some(match self {
            BinOp::Add => a.wrapping_add(b),
            BinOp::Sub => a.wrapping_sub(b),
            BinOp::Mul => a.wrapping_mul(b),
            BinOp::Div => {
                if b == 0 {
                    return None;
                }
                a.wrapping_div(b)
            }
            BinOp::Rem => {
                if b == 0 {
                    return None;
                }
                a.wrapping_rem(b)
            }
            BinOp::And => a & b,
            BinOp::Or => a | b,
            BinOp::Xor => a ^ b,
            BinOp::Shl => a.wrapping_shl(b as u32 & 31),
            BinOp::Shr => a.wrapping_shr(b as u32 & 31),
        })
    }
}

/// A runtime function call.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Call {
    /// Table index.
    pub func: FuncId,
    /// Arguments, one per table parameter, typed by the parameter.
    pub args: Vec<Arg>,
}

/// A call argument, shaped by the parameter type.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Arg {
    /// `SDWORD` / `BOOL`.
    Int(Expr),
    /// `UDWORD`: a label hash (variable/timer names carry [`crate::GLOBAL_FLAG`] for `G_` names).
    Hash(Hash),
    /// `CHARPTR`.
    Str(StrRef),
    /// `GROWSELECTIONPTR`.
    Ships(ShipsRef),
    /// `VECTORPTR`.
    Point(PointRef),
    /// `AITEAMPTR`.
    Team(TeamRef),
    /// `VOLUMEPTR` (`None` = NULL).
    Volume(Option<Hash>),
    /// `PATHPTR` (`None` = NULL).
    Path(Option<Hash>),
}

impl Arg {
    /// The parameter type this argument fits.
    pub fn param_type(&self) -> ParamType {
        match self {
            Arg::Int(_) => ParamType::Int,
            Arg::Hash(_) => ParamType::Hash,
            Arg::Str(_) => ParamType::Str,
            Arg::Ships(_) => ParamType::Ships,
            Arg::Point(_) => ParamType::Point,
            Arg::Team(_) => ParamType::Team,
            Arg::Volume(_) => ParamType::Volume,
            Arg::Path(_) => ParamType::Path,
        }
    }
}

/// A team reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TeamRef {
    /// NULL.
    Null,
    /// `THISTEAM`: the team whose code is running.
    This,
    /// `TEAM_<label>`.
    Label(Hash),
}

/// A ship selection reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ShipsRef {
    /// NULL.
    Null,
    /// `SHIPS_<label>`: a named KAS selection.
    Selection(Hash),
    /// `TEAMSHIPS_<label>` or `THISTEAMSHIPS`: a team's ship list.
    Team(TeamRef),
}

/// A position reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum PointRef {
    /// NULL.
    Null,
    /// `POINT_<label>`: a labelled point.
    Point(Hash),
    /// `SHIPSPOINT_<label>`: centre of a named selection.
    Ships(Hash),
    /// `TEAMSPOINT_<label>` / `THISTEAMSPOINT`: centre of a team's ships.
    Team(TeamRef),
    /// `VOLUMEPOINT_<label>`: centre of a labelled volume.
    Volume(Hash),
}

/// A string argument.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum StrRef {
    /// NULL.
    Null,
    /// A non-pointer integer passed where a string is expected.
    Int(i32),
    /// A literal.
    Text(String),
    /// A localised string: index into [`Mission::lstrings`].
    Local(usize),
}

impl Mission {
    /// Display name of a hash (`None` when unknown).
    pub fn name_of(&self, h: Hash) -> Option<&str> {
        self.names.get(&(h & !crate::GLOBAL_FLAG)).map(String::as_str)
    }

    /// Finds an FSM by hash.
    pub fn fsm_index(&self, hash: Hash) -> Option<usize> {
        self.fsms.iter().position(|f| f.hash == hash)
    }

    /// Visits every block: mission INIT/WATCH, then each FSM's INIT/WATCH and its states'.
    pub fn blocks(&self) -> impl Iterator<Item = (BlockOwner, &Block)> {
        let mission = [(BlockOwner::Mission, &self.init), (BlockOwner::Mission, &self.watch)];
        let fsms = self.fsms.iter().enumerate().flat_map(|(fi, f)| {
            [(BlockOwner::Fsm(fi), &f.init), (BlockOwner::Fsm(fi), &f.watch)].into_iter().chain(
                f.states.iter().enumerate().flat_map(move |(si, s)| {
                    [(BlockOwner::State(fi, si), &s.init), (BlockOwner::State(fi, si), &s.watch)]
                }),
            )
        });
        mission.into_iter().chain(fsms)
    }

    /// Counts calls of each function over the whole mission.
    pub fn call_counts(&self) -> BTreeMap<FuncId, usize> {
        let mut out = BTreeMap::new();
        for (_, b) in self.blocks() {
            visit_calls(&b.stmts, &mut |c| *out.entry(c.func).or_insert(0) += 1);
        }
        out
    }
}

/// Which code a block belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BlockOwner {
    /// Mission-level code (scope 0).
    Mission,
    /// FSM-level code (scope 1) of FSM `.0`.
    Fsm(usize),
    /// State code (scope 2) of FSM `.0`, state `.1`.
    State(usize, usize),
}

impl BlockOwner {
    /// The FSM the code belongs to.
    pub fn fsm(self) -> Option<usize> {
        match self {
            BlockOwner::Mission => None,
            BlockOwner::Fsm(f) | BlockOwner::State(f, _) => Some(f),
        }
    }
}

/// Calls `f` for every call in a statement list (including nested expressions).
pub fn visit_calls<'a>(stmts: &'a [Stmt], f: &mut dyn FnMut(&'a Call)) {
    for s in stmts {
        match s {
            Stmt::Call(c) => visit_call(c, f),
            Stmt::If { cond, then, otherwise } => {
                visit_cond(cond, f);
                visit_calls(then, f);
                visit_calls(otherwise, f);
            }
            Stmt::IfOnce { cond, body, otherwise, .. } => {
                if let Some(c) = cond {
                    visit_cond(c, f);
                }
                visit_calls(body, f);
                visit_calls(otherwise, f);
            }
            Stmt::Switch { value, arms, default } => {
                visit_expr(value, f);
                for a in arms {
                    visit_calls(&a.body, f);
                }
                if let Some(d) = default {
                    visit_calls(d, f);
                }
            }
            Stmt::MsgLoop { handlers, default } => {
                for h in handlers {
                    visit_calls(&h.body, f);
                }
                if let Some(d) = default {
                    visit_calls(d, f);
                }
            }
            Stmt::Jump { .. } | Stmt::FsmCreate { .. } | Stmt::Return => {}
        }
    }
}

fn visit_call<'a>(c: &'a Call, f: &mut dyn FnMut(&'a Call)) {
    f(c);
    for a in &c.args {
        if let Arg::Int(e) = a {
            visit_expr(e, f);
        }
    }
}

fn visit_cond<'a>(c: &'a Cond, f: &mut dyn FnMut(&'a Call)) {
    match c {
        Cond::True | Cond::False => {}
        Cond::Not(a) => visit_cond(a, f),
        Cond::And(a, b) | Cond::Or(a, b) => {
            visit_cond(a, f);
            visit_cond(b, f);
        }
        Cond::Cmp(_, a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
    }
}

fn visit_expr<'a>(e: &'a Expr, f: &mut dyn FnMut(&'a Call)) {
    match e {
        Expr::Int(_) => {}
        Expr::Call(c) => visit_call(c, f),
        Expr::Bin(_, a, b) => {
            visit_expr(a, f);
            visit_expr(b, f);
        }
        Expr::Neg(a) | Expr::BitNot(a) => visit_expr(a, f),
        Expr::Cond(c) => visit_cond(c, f),
    }
}
