//! The KAS interpreter: runs a [`Mission`] with the original runtime semantics.
//!
//! Behaviour follows Cataclysm's runtime (`kasMissionStart`, `kasExecute`, `kasJump`,
//! `kasFSMCreate`, the `kasfVar*` / `kasfTimer*` families, `aitMsgSend` / the kas2c message loop,
//! `kasfOtherKASPause`):
//!
//! * **Ticks.** [`Interpreter::execute`] is one `kasExecute`: unless the KAS pause is active it runs
//!   the mission WATCH, then for every team in team-list order that is under script control the
//!   FSM WATCH and then the current state's WATCH (read after the FSM WATCH, so a JUMP there takes
//!   effect in the same tick). While a team holds the KAS pause (`PauseOtherKAS`) only that team
//!   runs. How often to call it is the game's business; see [`kas_frame_due`].
//! * **Scopes.** Every INIT/WATCH sets `CurrentMissionScope` (mission 0, FSM 1, state 2) and
//!   `CurrentMissionScopeName` (the mission name hash, else the team label) on entry. Variables
//!   and timers are keyed by `(scope key, name hash)` where the scope key is 0 for mission-level
//!   code and the team label otherwise; names carrying [`GLOBAL_FLAG`] (kas2c's `G_` names) always
//!   use key 0. `kasFSMCreate` saves and restores the current team and scope around the FSM INIT;
//!   `kasJump` does not.
//! * **JUMP** sets the current team's state and runs the new state's INIT immediately; the
//!   function containing the JUMP then returns.
//! * **Messages.** Each team has the original 64-entry queue (which wraps to empty when it fills).
//!   A message loop walks the queue from index `count` down to 0, removing each message by
//!   swapping in the last one; unhandled messages are re-sent to the team itself.
//! * **Arguments** of a runtime call are evaluated right to left, as MSVC did; operands of
//!   arithmetic left to right; conditions short-circuit.
//!
//! Where the original would crash (NULL team for `FSMCreate`/`MsgSend`, division by zero,
//! endless JUMP recursion) the interpreter reports a warning and carries on.

use std::sync::Arc;

use crate::functions::{ids, Builtin, FuncId};
use crate::host::{dispatch, Cx, KasHost, Point, Scope, Ships, Str, Value};
use crate::ir::*;
use crate::{Hash, GLOBAL_FLAG};

/// Capacity of a team's message queue (`aitMsgSend` wraps the count to 0 when it reaches this).
pub const MSG_QUEUE_LEN: usize = 64;

/// Nesting limit for JUMP / FSMCreate chains (the original would overflow its stack).
const MAX_DEPTH: u32 = 64;

/// The ranRandomFn stream `Random` draws from.
pub const RANDOM_STREAM: u32 = 6;

/// Whether `kasExecute` runs on this universe update: the original tests
/// `(univUpdateCounter & updateRate) == (kasFrame & updateRate)`. Single player uses the
/// computer player's `aiplayerUpdateRate` (from `AIPLAYER_UPDATE_RATE[level]` in
/// aiplayer.script, e.g. 15) and `SP_KAS_UPDATE_FRAME` (tweak.script, 0); the tutorial uses
/// `TUTORIAL_KAS_UPDATE_MASK` (1) and `TUTORIAL_KAS_UPDATE_FRAME` (0). The game also skips KAS
/// while a hyperspace transition is in progress.
pub fn kas_frame_due(univ_update_counter: u32, update_rate: u32, kas_frame: u32) -> bool {
    univ_update_counter & update_rate == kas_frame & update_rate
}

/// A KAS variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Var {
    /// Scope key: 0 for mission scope, else the owning team's label.
    pub scope: Hash,
    /// Name hash (without [`GLOBAL_FLAG`]).
    pub name: Hash,
    /// Value.
    pub value: i32,
}

/// A KAS timer (counts down in game seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timer {
    /// Scope key, as for [`Var`].
    pub scope: Hash,
    /// Name hash (without [`GLOBAL_FLAG`]).
    pub name: Hash,
    /// Game time when started.
    pub start: f32,
    /// Duration in seconds (the remaining time while stopped).
    pub duration: f32,
    /// Game time when paused by `PauseOtherKAS`.
    pub pause_time: f32,
    /// Running.
    pub enabled: bool,
    /// Frozen by `PauseOtherKAS`.
    pub paused: bool,
}

/// A team's message queue (`team->msgQueue`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MsgQueue {
    count: usize,
    msgs: [Hash; MSG_QUEUE_LEN],
    senders: [Hash; MSG_QUEUE_LEN],
}

impl MsgQueue {
    fn new() -> Self {
        MsgQueue { count: 0, msgs: [0; MSG_QUEUE_LEN], senders: [0; MSG_QUEUE_LEN] }
    }

    /// Queued messages and their senders (0 = no team), oldest first.
    pub fn messages(&self) -> impl Iterator<Item = (Hash, Hash)> + '_ {
        (0..self.count).map(|i| (self.msgs[i], self.senders[i]))
    }

    /// Number of queued messages.
    pub fn len(&self) -> usize {
        self.count
    }

    /// No messages queued.
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Removes and returns entry `i` (the last entry takes its place); 0 when out of range.
    fn take(&mut self, i: usize) -> Hash {
        if i >= self.count {
            return 0;
        }
        let m = self.msgs[i];
        if self.count > 1 {
            self.msgs[i] = self.msgs[self.count - 1];
            self.senders[i] = self.senders[self.count - 1];
        }
        self.count -= 1;
        m
    }
}

#[derive(Clone, Debug)]
struct TeamSlot {
    label: Hash,
    script: bool,
    fsm: Option<usize>,
    state: Option<(usize, usize)>,
    queue: Option<Box<MsgQueue>>,
}

/// The KAS state of one team.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TeamView<'a> {
    /// Team label.
    pub label: Hash,
    /// Under script control (`teamType == ScriptTeam`).
    pub script: bool,
    /// Hash of the FSM controlling the team.
    pub fsm: Option<Hash>,
    /// Hash of the team's current state.
    pub state: Option<Hash>,
    /// The message queue, if one was ever allocated.
    pub queue: Option<&'a MsgQueue>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Flow {
    Next,
    Return,
}

/// Runs one mission.
#[derive(Clone, Debug)]
pub struct Interpreter {
    mission: Arc<Mission>,
    started: bool,
    vars: Vec<Var>,
    timers: Vec<Timer>,
    teams: Vec<TeamSlot>,
    current: Option<usize>,
    scope: Scope,
    scope_name: Hash,
    unpaused: Option<usize>,
    tick: u64,
    depth: u32,
    warnings: Vec<String>,
}

const MAX_WARNINGS: usize = 1000;

impl Interpreter {
    /// A new interpreter for `mission` (nothing runs until [`start`](Self::start)).
    pub fn new(mission: impl Into<Arc<Mission>>) -> Self {
        Interpreter {
            mission: mission.into(),
            started: false,
            vars: Vec::new(),
            timers: Vec::new(),
            teams: Vec::new(),
            current: None,
            scope: Scope::Mission,
            scope_name: 0,
            unpaused: None,
            tick: 0,
            depth: 0,
            warnings: Vec::new(),
        }
    }

    /// The mission.
    pub fn mission(&self) -> &Arc<Mission> {
        &self.mission
    }

    /// Declares a team in the AI player's team list. Teams are executed and receive `MsgSendAll`
    /// in registration order, so the game should register its teams in its own team-list order
    /// before [`start`](Self::start). Teams first referenced by mission code are appended.
    pub fn register_team(&mut self, label: Hash) -> usize {
        if let Some(i) = self.team_index(label) {
            return i;
        }
        self.teams.push(TeamSlot { label, script: false, fsm: None, state: None, queue: None });
        self.teams.len() - 1
    }

    fn team_index(&self, label: Hash) -> Option<usize> {
        self.teams.iter().position(|t| t.label == label)
    }

    /// `kasMissionStart`: runs the mission INIT.
    pub fn start<H: KasHost + ?Sized>(&mut self, host: &mut H) {
        self.started = true;
        let m = self.mission.clone();
        self.run_block(host, &m, BlockOwner::Mission, &m.init);
    }

    /// `kasExecute`: one KAS tick.
    pub fn execute<H: KasHost + ?Sized>(&mut self, host: &mut H) {
        self.tick += 1;
        if !self.started {
            return;
        }
        let m = self.mission.clone();
        match self.unpaused {
            None => {
                self.run_block(host, &m, BlockOwner::Mission, &m.watch);
                let mut i = 0;
                while i < self.teams.len() {
                    self.current = Some(i);
                    self.watch_team(host, &m, i);
                    i += 1;
                }
            }
            Some(t) => {
                self.current = Some(t);
                self.watch_team(host, &m, t);
            }
        }
    }

    fn watch_team<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, i: usize) {
        if !self.teams[i].script {
            return;
        }
        if let Some(f) = self.teams[i].fsm {
            self.run_block(host, m, BlockOwner::Fsm(f), &m.fsms[f].watch);
        }
        if let Some((f, s)) = self.teams.get(i).and_then(|t| t.state) {
            self.run_block(host, m, BlockOwner::State(f, s), &m.fsms[f].states[s].watch);
        }
    }

    /// Queues `msg` for team `to` from team `from` (`aitMsgSend`; the game sends messages too).
    pub fn post_message(&mut self, to: Hash, from: Option<Hash>, msg: Hash) {
        let t = self.register_team(to);
        self.msg_send(t, from.unwrap_or(0), msg);
    }

    /// `kasTeamDied`: forget the team as the current team.
    pub fn team_died(&mut self, label: Hash) {
        if let Some(i) = self.team_index(label) {
            if self.current == Some(i) {
                self.current = None;
            }
        }
    }

    /// Changes whether a team is under script control (`teamType == ScriptTeam`).
    pub fn set_team_script(&mut self, label: Hash, script: bool) {
        let i = self.register_team(label);
        self.teams[i].script = script;
    }

    /// The KAS state of a team.
    pub fn team(&self, label: Hash) -> Option<TeamView<'_>> {
        self.team_index(label).map(|i| self.view(i))
    }

    /// All known teams, in team-list order.
    pub fn teams(&self) -> impl Iterator<Item = TeamView<'_>> + '_ {
        (0..self.teams.len()).map(|i| self.view(i))
    }

    fn view(&self, i: usize) -> TeamView<'_> {
        let t = &self.teams[i];
        TeamView {
            label: t.label,
            script: t.script,
            fsm: t.fsm.map(|f| self.mission.fsms[f].hash),
            state: t.state.map(|(f, s)| self.mission.fsms[f].states[s].hash),
            queue: t.queue.as_deref(),
        }
    }

    /// Variables, in creation order.
    pub fn vars(&self) -> &[Var] {
        &self.vars
    }

    /// Looks a variable up: `team` is the owning team for FSM/state variables, `None` for
    /// mission-scope ones. `name` may carry [`GLOBAL_FLAG`].
    pub fn var(&self, team: Option<Hash>, name: Hash) -> Option<i32> {
        let (scope, name) = if name & GLOBAL_FLAG != 0 { (0, name & !GLOBAL_FLAG) } else { (team.unwrap_or(0), name) };
        self.vars.iter().find(|v| v.scope == scope && v.name == name).map(|v| v.value)
    }

    /// Creates or sets a variable from the game side (`VarCreateSet` semantics): `team` selects a
    /// team's FSM/state scope, `None` mission scope; [`GLOBAL_FLAG`] forces mission scope.
    pub fn set_var(&mut self, team: Option<Hash>, name: Hash, value: i32) {
        let key = if name & GLOBAL_FLAG != 0 { (0, name & !GLOBAL_FLAG) } else { (team.unwrap_or(0), name) };
        let i = self.var_create(key);
        self.vars[i].value = value;
    }

    /// Destroys a variable from the game side (`kasVarDestroyEntry`); `team` and `name` as for
    /// [`var`](Self::var). Returns its value if it existed.
    pub fn remove_var(&mut self, team: Option<Hash>, name: Hash) -> Option<i32> {
        let key = if name & GLOBAL_FLAG != 0 { (0, name & !GLOBAL_FLAG) } else { (team.unwrap_or(0), name) };
        let i = self.var_find(key)?;
        Some(self.vars.remove(i).value)
    }

    /// Timers, in creation order.
    pub fn timers(&self) -> &[Timer] {
        &self.timers
    }

    /// The team holding the KAS pause, if any.
    pub fn paused_by(&self) -> Option<Hash> {
        self.unpaused.map(|i| self.teams[i].label)
    }

    /// Number of [`execute`](Self::execute) calls so far.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Warnings so far (capped).
    pub fn warnings(&self) -> &[String] {
        &self.warnings
    }

    // ---- execution ----------------------------------------------------------------------

    fn cx(&self) -> Cx {
        Cx {
            team: self.current.map(|i| self.teams[i].label),
            scope: self.scope,
            scope_name: self.scope_name,
            tick: self.tick,
        }
    }

    fn warn<H: KasHost + ?Sized>(&mut self, host: &mut H, msg: String) {
        host.warning(&self.cx(), &msg);
        if self.warnings.len() < MAX_WARNINGS {
            self.warnings.push(format!("tick {}: {msg}", self.tick));
        }
    }

    fn run_block<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, owner: BlockOwner, block: &Block) {
        if self.depth >= MAX_DEPTH {
            self.warn(host, "JUMP/FSMCreate nesting too deep; code not run".into());
            return;
        }
        let team_label = self.current.map_or(0, |i| self.teams[i].label);
        match owner {
            BlockOwner::Mission => {
                self.scope = Scope::Mission;
                self.scope_name = m.name_hash;
            }
            BlockOwner::Fsm(_) => {
                self.scope = Scope::Fsm;
                self.scope_name = team_label;
            }
            BlockOwner::State(..) => {
                self.scope = Scope::State;
                self.scope_name = team_label;
            }
        }
        self.depth += 1;
        self.exec(host, m, owner.fsm(), &block.stmts);
        self.depth -= 1;
    }

    fn exec<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, fsm: Option<usize>, stmts: &[Stmt]) -> Flow {
        for s in stmts {
            if self.stmt(host, m, fsm, s) == Flow::Return {
                return Flow::Return;
            }
        }
        Flow::Next
    }

    fn stmt<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, fsm: Option<usize>, s: &Stmt) -> Flow {
        match s {
            Stmt::Call(c) => {
                self.call(host, m, c);
                Flow::Next
            }
            Stmt::If { cond, then, otherwise } => {
                if self.cond(host, m, cond) {
                    self.exec(host, m, fsm, then)
                } else {
                    self.exec(host, m, fsm, otherwise)
                }
            }
            Stmt::IfOnce { var, cond, body, otherwise } => {
                let done = self.builtin_call(host, ids::VAR_GET, Builtin::VarGet, &[Value::Hash(*var)]);
                let go = done == 0 && cond.as_ref().is_none_or(|c| self.cond(host, m, c));
                if go {
                    self.builtin_call(host, ids::VAR_CREATE_SET, Builtin::VarCreateSet, &[Value::Hash(*var), Value::Int(1)]);
                    self.exec(host, m, fsm, body)
                } else {
                    self.exec(host, m, fsm, otherwise)
                }
            }
            Stmt::Switch { value, arms, default } => {
                let v = self.expr(host, m, value);
                match arms.iter().find(|a| a.values.contains(&v)) {
                    Some(a) => self.exec(host, m, fsm, &a.body),
                    None => match default {
                        Some(d) => self.exec(host, m, fsm, d),
                        None => Flow::Next,
                    },
                }
            }
            Stmt::MsgLoop { handlers, default } => self.msg_loop(host, m, fsm, handlers, default.as_deref()),
            Stmt::Jump { state, hash } => {
                match (fsm, self.current) {
                    (Some(f), Some(t)) => {
                        self.teams[t].state = Some((f, *state));
                        host.state_changed(self.teams[t].label, *hash);
                        self.run_block(host, m, BlockOwner::State(f, *state), &m.fsms[f].states[*state].init);
                    }
                    _ => self.warn(host, format!("JUMP {hash:08x} without a current team")),
                }
                Flow::Return
            }
            Stmt::FsmCreate { fsm: f, team } => {
                self.fsm_create(host, m, *f, *team);
                Flow::Next
            }
            Stmt::Return => Flow::Return,
        }
    }

    fn fsm_create<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, f: usize, team: TeamRef) {
        let t = match team {
            TeamRef::Null => None,
            TeamRef::This => self.current,
            TeamRef::Label(h) => Some(self.register_team(h)),
        };
        let Some(t) = t else {
            self.warn(host, format!("FSMCreate({:08x}) on a NULL team", m.fsms[f].hash));
            return;
        };
        let saved = (self.current, self.scope, self.scope_name);
        self.current = Some(t);
        let slot = &mut self.teams[t];
        slot.script = true;
        slot.fsm = Some(f);
        slot.state = None;
        self.scope_name = slot.label;
        host.fsm_created(slot.label, m.fsms[f].hash);
        self.run_block(host, m, BlockOwner::Fsm(f), &m.fsms[f].init);
        (self.current, self.scope, self.scope_name) = saved;
    }

    fn msg_loop<H: KasHost + ?Sized>(
        &mut self,
        host: &mut H,
        m: &Mission,
        fsm: Option<usize>,
        handlers: &[MsgHandler],
        default: Option<&[Stmt]>,
    ) -> Flow {
        let Some(t) = self.current else { return Flow::Next };
        let mut n: i64 = self.teams[t].queue.as_ref().map_or(-1, |q| q.count as i64);
        if n < 0 {
            return Flow::Next;
        }
        loop {
            // the team cannot change inside a WATCH (FSMCreate restores it), but stay safe
            let t = self.current.unwrap_or(t);
            let msg = self.teams[t].queue.as_mut().map_or(0, |q| q.take(n as usize));
            if msg != 0 {
                match handlers.iter().find(|h| h.msgs.contains(&msg)) {
                    Some(h) => {
                        if self.exec(host, m, fsm, &h.body) == Flow::Return {
                            return Flow::Return;
                        }
                    }
                    None => match default {
                        Some(d) => {
                            if self.exec(host, m, fsm, d) == Flow::Return {
                                return Flow::Return;
                            }
                        }
                        None => {
                            let me = self.teams[t].label;
                            self.msg_send(t, me, msg);
                        }
                    },
                }
            }
            let t = self.current.unwrap_or(t);
            if self.teams[t].queue.as_ref().is_none_or(|q| q.count == 0) {
                return Flow::Next;
            }
            n -= 1;
            if n < 0 {
                return Flow::Next;
            }
        }
    }

    fn msg_send(&mut self, to: usize, from: Hash, msg: Hash) {
        let q = self.teams[to].queue.get_or_insert_with(|| Box::new(MsgQueue::new()));
        q.msgs[q.count] = msg;
        q.senders[q.count] = from;
        q.count += 1;
        if q.count == MSG_QUEUE_LEN {
            q.count = 0;
        }
    }

    // ---- expressions --------------------------------------------------------------------

    fn cond<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, c: &Cond) -> bool {
        match c {
            Cond::True => true,
            Cond::False => false,
            Cond::Not(a) => !self.cond(host, m, a),
            Cond::And(a, b) => self.cond(host, m, a) && self.cond(host, m, b),
            Cond::Or(a, b) => self.cond(host, m, a) || self.cond(host, m, b),
            Cond::Cmp(op, a, b) => {
                let x = self.expr(host, m, a);
                let y = self.expr(host, m, b);
                op.eval(x, y)
            }
        }
    }

    fn expr<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, e: &Expr) -> i32 {
        match e {
            Expr::Int(v) => *v,
            Expr::Call(c) => self.call(host, m, c),
            Expr::Bin(op, a, b) => {
                let x = self.expr(host, m, a);
                let y = self.expr(host, m, b);
                match op.eval(x, y) {
                    Some(v) => v,
                    None => {
                        self.warn(host, format!("division by zero ({x} {} 0)", op.symbol()));
                        0
                    }
                }
            }
            Expr::Neg(a) => self.expr(host, m, a).wrapping_neg(),
            Expr::BitNot(a) => !self.expr(host, m, a),
            Expr::Cond(c) => i32::from(self.cond(host, m, c)),
        }
    }

    fn team_label(&mut self, r: TeamRef) -> Option<Hash> {
        match r {
            TeamRef::Null => None,
            TeamRef::This => self.current.map(|i| self.teams[i].label),
            TeamRef::Label(h) => {
                self.register_team(h);
                Some(h)
            }
        }
    }

    fn call<H: KasHost + ?Sized>(&mut self, host: &mut H, m: &Mission, c: &Call) -> i32 {
        // MSVC pushes (and evaluates) arguments right to left.
        let mut args: Vec<Value<'_>> = vec![Value::Int(0); c.args.len()];
        for (i, a) in c.args.iter().enumerate().rev() {
            args[i] = self.arg(host, m, a);
        }
        let info = c.func.info();
        match info.builtin {
            Some(b) => self.builtin_call(host, c.func, b, &args),
            None => {
                let cx = self.cx();
                let r = dispatch(host, &cx, c.func, &args);
                let r = if info.returns { r } else { 0 };
                host.trace_call(&cx, c.func, &args, r);
                r
            }
        }
    }

    fn arg<'m, H: KasHost + ?Sized>(&mut self, host: &mut H, m: &'m Mission, a: &'m Arg) -> Value<'m> {
        match a {
            Arg::Int(e) => Value::Int(self.expr(host, m, e)),
            Arg::Hash(h) => Value::Hash(*h),
            Arg::Str(s) => Value::Str(match s {
                StrRef::Null => Str::Null,
                StrRef::Int(v) => Str::Int(*v),
                StrRef::Text(t) => Str::Text(t),
                StrRef::Local(i) => {
                    let lang = host.language().min(2);
                    Str::Local { index: *i, text: m.lstrings.get(*i).map_or("", |l| l.texts[lang].as_str()) }
                }
            }),
            Arg::Ships(s) => Value::Ships(match *s {
                ShipsRef::Null => Ships::Null,
                ShipsRef::Selection(h) => Ships::Selection(h),
                ShipsRef::Team(t) => self.team_label(t).map_or(Ships::Null, Ships::Team),
            }),
            Arg::Point(p) => Value::Point(match *p {
                PointRef::Null => Point::Null,
                PointRef::Point(h) => Point::Label(h),
                PointRef::Ships(h) => Point::Ships(h),
                PointRef::Volume(h) => Point::Volume(h),
                PointRef::Team(t) => self.team_label(t).map_or(Point::Null, Point::Team),
            }),
            Arg::Team(t) => Value::Team(self.team_label(*t)),
            Arg::Volume(v) => Value::Volume(*v),
            Arg::Path(v) => Value::Path(*v),
        }
    }

    // ---- builtins -----------------------------------------------------------------------

    fn key(&self, name: Hash) -> (Hash, Hash) {
        if name & GLOBAL_FLAG != 0 {
            (0, name & !GLOBAL_FLAG)
        } else if self.scope != Scope::Mission {
            (self.scope_name, name)
        } else {
            (0, name)
        }
    }

    fn var_find(&self, key: (Hash, Hash)) -> Option<usize> {
        self.vars.iter().position(|v| v.scope == key.0 && v.name == key.1)
    }

    fn var_create(&mut self, key: (Hash, Hash)) -> usize {
        match self.var_find(key) {
            Some(i) => i,
            None => {
                self.vars.push(Var { scope: key.0, name: key.1, value: 0 });
                self.vars.len() - 1
            }
        }
    }

    fn timer_find(&self, key: (Hash, Hash)) -> Option<usize> {
        self.timers.iter().position(|t| t.scope == key.0 && t.name == key.1)
    }

    fn builtin_call<H: KasHost + ?Sized>(&mut self, host: &mut H, id: FuncId, b: Builtin, args: &[Value<'_>]) -> i32 {
        let cx = self.cx();
        let r = self.builtin(host, b, args);
        let r = if id.info().returns { r } else { 0 };
        host.trace_call(&cx, id, args, r);
        r
    }

    fn builtin<H: KasHost + ?Sized>(&mut self, host: &mut H, b: Builtin, args: &[Value<'_>]) -> i32 {
        let arg = |i: usize| args.get(i).copied().unwrap_or(Value::Int(0));
        match b {
            Builtin::VarCreate => {
                let k = self.key(arg(0).hash());
                if k.1 != 0 {
                    self.var_create(k);
                }
                0
            }
            Builtin::VarCreateSet => {
                let i = self.var_create(self.key(arg(0).hash()));
                self.vars[i].value = arg(1).int();
                0
            }
            Builtin::VarSet | Builtin::VarInc | Builtin::VarDec => {
                if let Some(i) = self.var_find(self.key(arg(0).hash())) {
                    let v = &mut self.vars[i].value;
                    *v = match b {
                        Builtin::VarSet => arg(1).int(),
                        Builtin::VarInc => v.wrapping_add(1),
                        _ => v.wrapping_sub(1),
                    };
                }
                0
            }
            Builtin::VarGet => self.var_find(self.key(arg(0).hash())).map_or(0, |i| self.vars[i].value),
            Builtin::VarDestroy => {
                if let Some(i) = self.var_find(self.key(arg(0).hash())) {
                    self.vars.remove(i);
                }
                0
            }
            Builtin::TimerCreate => {
                self.timer_create(self.key(arg(0).hash()));
                0
            }
            Builtin::TimerSet => {
                if let Some(i) = self.timer_find(self.key(arg(0).hash())) {
                    self.timers[i].duration = arg(1).int() as f32;
                }
                0
            }
            Builtin::TimerStart => {
                if let Some(i) = self.timer_find(self.key(arg(0).hash())) {
                    let now = host.universe_time();
                    timer_start(&mut self.timers[i], now);
                }
                0
            }
            Builtin::TimerCreateSetStart => {
                let now = host.universe_time();
                let i = self.timer_create(self.key(arg(0).hash()));
                self.timers[i].duration = arg(1).int() as f32;
                timer_start(&mut self.timers[i], now);
                0
            }
            Builtin::TimerStop => {
                if let Some(i) = self.timer_find(self.key(arg(0).hash())) {
                    let now = host.universe_time();
                    let t = &mut self.timers[i];
                    if t.paused {
                        t.enabled = false;
                        t.duration = 2_147_483_648.0;
                    } else if t.enabled {
                        let r = timer_left(t, now);
                        let r = if r < 1 { 0 } else { r };
                        t.enabled = false;
                        t.duration = r as f32;
                    } else {
                        t.duration = fist(f64::from(t.duration)) as f32;
                    }
                }
                0
            }
            Builtin::TimerRemaining => match self.timer_find(self.key(arg(0).hash())) {
                None => 0,
                Some(i) => {
                    let now = host.universe_time();
                    let t = &mut self.timers[i];
                    if t.paused {
                        i32::MAX
                    } else if !t.enabled {
                        fist(f64::from(t.duration))
                    } else {
                        let r = timer_left(t, now);
                        if r > 0 {
                            r
                        } else {
                            t.enabled = false;
                            t.duration = 0.0;
                            0
                        }
                    }
                }
            },
            Builtin::TimerExpired | Builtin::TimerExpiredDestroy => match self.timer_find(self.key(arg(0).hash())) {
                None => 0,
                Some(i) => {
                    let now = host.universe_time();
                    let t = &mut self.timers[i];
                    let expired = if t.paused {
                        false
                    } else if !t.enabled {
                        fist(f64::from(t.duration)) == 0
                    } else if timer_left(t, now) > 0 {
                        false
                    } else {
                        t.enabled = false;
                        t.duration = 0.0;
                        true
                    };
                    if expired && b == Builtin::TimerExpiredDestroy {
                        self.timers.remove(i);
                    }
                    i32::from(expired)
                }
            },
            Builtin::TimerDestroy => {
                if let Some(i) = self.timer_find(self.key(arg(0).hash())) {
                    self.timers.remove(i);
                }
                0
            }
            Builtin::MsgSend => {
                let from = self.current.map_or(0, |i| self.teams[i].label);
                match arg(0).team() {
                    Some(to) => {
                        let t = self.register_team(to);
                        self.msg_send(t, from, arg(1).hash());
                    }
                    None => self.warn(host, format!("MsgSend(NULL, {:08x})", arg(1).hash())),
                }
                0
            }
            Builtin::MsgSendAll => {
                let from = self.current.map_or(0, |i| self.teams[i].label);
                for t in 0..self.teams.len() {
                    self.msg_send(t, from, arg(0).hash());
                }
                0
            }
            Builtin::MsgReceived => {
                let msg = arg(0).hash();
                let Some(t) = self.current else { return 0 };
                let Some(q) = self.teams[t].queue.as_mut() else { return 0 };
                if msg == 0 {
                    return 0;
                }
                match q.msgs[..q.count].iter().position(|&x| x == msg) {
                    Some(i) => {
                        q.take(i);
                        1
                    }
                    None => 0,
                }
            }
            Builtin::MsgQueueFlush => {
                if let Some(t) = arg(0).team().and_then(|l| self.team_index(l)) {
                    self.teams[t].queue = None;
                }
                0
            }
            Builtin::ThisTeamIs => i32::from(arg(0).team() == self.current.map(|i| self.teams[i].label)),
            Builtin::Random => {
                let (mut lo, mut hi) = (arg(0).int(), arg(1).int());
                if hi < lo {
                    std::mem::swap(&mut lo, &mut hi);
                }
                let r = host.random(RANDOM_STREAM);
                let span = (hi.wrapping_sub(lo) as u32).wrapping_add(1);
                if span == 0 {
                    return r as i32;
                }
                ((r % span) as i32).wrapping_add(lo)
            }
            Builtin::PauseOtherKAS => {
                self.unpaused = self.current;
                let now = host.universe_time();
                let keep = self.scope_name;
                for t in self.timers.iter_mut().filter(|t| t.scope != keep) {
                    t.pause_time = now;
                    t.paused = true;
                }
                0
            }
            Builtin::UnpauseOtherKAS => {
                self.unpaused = None;
                let now = host.universe_time();
                for t in self.timers.iter_mut().filter(|t| t.paused) {
                    t.paused = false;
                    t.duration += now - t.pause_time;
                }
                0
            }
        }
    }

    fn timer_create(&mut self, key: (Hash, Hash)) -> usize {
        match self.timer_find(key) {
            Some(i) => i,
            None => {
                self.timers.push(Timer {
                    scope: key.0,
                    name: key.1,
                    start: 0.0,
                    duration: 0.0,
                    pause_time: 0.0,
                    enabled: false,
                    paused: false,
                });
                self.timers.len() - 1
            }
        }
    }
}

fn timer_start(t: &mut Timer, now: f32) {
    if t.duration <= 0.0 {
        t.enabled = false;
    } else if !t.enabled {
        t.start = now;
        t.enabled = true;
        t.paused = false;
    }
}

/// Whole seconds left on a running timer, rounded to nearest (ties to even) like the original's
/// float-to-int conversion.
fn timer_left(t: &Timer, now: f32) -> i32 {
    fist(f64::from(t.duration) - (f64::from(now) - f64::from(t.start)))
}

/// x87 `fistp` with the default rounding mode: round half to even, wrapping to 32 bits.
fn fist(x: f64) -> i32 {
    x.round_ties_even() as i64 as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::KasRuntime;

    #[derive(Default)]
    struct Clock {
        now: f32,
    }
    impl KasRuntime for Clock {}
    impl KasHost for Clock {
        fn universe_time(&mut self) -> f32 {
            self.now
        }
    }

    #[test]
    fn rounding() {
        assert_eq!(fist(2.5), 2);
        assert_eq!(fist(3.5), 4);
        assert_eq!(fist(-0.5), 0);
        assert_eq!(fist(0.6), 1);
    }

    #[test]
    fn frame_due() {
        assert!(kas_frame_due(32, 15, 0));
        assert!(!kas_frame_due(33, 15, 0));
        assert!(kas_frame_due(2, 1, 0));
    }

    #[test]
    fn queue_take_swaps_last() {
        let mut q = MsgQueue::new();
        for m in [1, 2, 3] {
            q.msgs[q.count] = m;
            q.count += 1;
        }
        assert_eq!(q.take(0), 1);
        assert_eq!(q.messages().map(|x| x.0).collect::<Vec<_>>(), vec![3, 2]);
        assert_eq!(q.take(5), 0);
    }

    #[test]
    fn timers_follow_game_time() {
        let mission = Mission {
            name: "t".into(),
            name_hash: 1,
            lstrings: vec![],
            fsms: vec![],
            init: Block::default(),
            watch: Block::default(),
            names: Default::default(),
            fidelity: Fidelity::Exact,
        };
        let mut it = Interpreter::new(mission);
        let mut h = Clock::default();
        let name = [Value::Hash(5), Value::Int(3)];
        it.builtin(&mut h, Builtin::TimerCreateSetStart, &name);
        h.now = 2.4;
        assert_eq!(it.builtin(&mut h, Builtin::TimerRemaining, &name), 1);
        assert_eq!(it.builtin(&mut h, Builtin::TimerExpired, &name), 0);
        h.now = 2.6;
        assert_eq!(it.builtin(&mut h, Builtin::TimerExpired, &name), 1);
        assert!(!it.timers()[0].enabled);
        assert_eq!(it.builtin(&mut h, Builtin::TimerExpiredDestroy, &name), 1);
        assert!(it.timers().is_empty());
        // unknown timers: never expired, nothing remaining
        assert_eq!(it.builtin(&mut h, Builtin::TimerExpired, &name), 0);
        // KAS pause freezes other scopes' timers
        it.builtin(&mut h, Builtin::TimerCreateSetStart, &name);
        it.scope = Scope::State;
        it.scope_name = 77;
        it.builtin(&mut h, Builtin::PauseOtherKAS, &[]);
        h.now = 100.0;
        it.scope = Scope::Mission;
        assert_eq!(it.builtin(&mut h, Builtin::TimerRemaining, &name), i32::MAX);
        it.builtin(&mut h, Builtin::UnpauseOtherKAS, &[]);
        assert_eq!(it.builtin(&mut h, Builtin::TimerRemaining, &name), 3);
    }

    #[test]
    fn variable_scopes() {
        let mission = Mission {
            name: "t".into(),
            name_hash: 1,
            lstrings: vec![],
            fsms: vec![],
            init: Block::default(),
            watch: Block::default(),
            names: Default::default(),
            fidelity: Fidelity::Exact,
        };
        let mut it = Interpreter::new(mission);
        let mut h = Clock::default();
        it.scope = Scope::State;
        it.scope_name = 0x42;
        it.builtin(&mut h, Builtin::VarCreateSet, &[Value::Hash(9), Value::Int(4)]);
        it.builtin(&mut h, Builtin::VarCreateSet, &[Value::Hash(9 | GLOBAL_FLAG), Value::Int(7)]);
        assert_eq!(it.var(Some(0x42), 9), Some(4));
        assert_eq!(it.var(None, 9), Some(7));
        it.scope = Scope::Mission;
        assert_eq!(it.builtin(&mut h, Builtin::VarGet, &[Value::Hash(9)]), 7);
        it.builtin(&mut h, Builtin::VarInc, &[Value::Hash(9)]);
        assert_eq!(it.var(None, 9 | GLOBAL_FLAG), Some(8));
        // VarSet does not create
        it.builtin(&mut h, Builtin::VarSet, &[Value::Hash(10), Value::Int(1)]);
        assert_eq!(it.var(None, 10), None);
    }
}
