//! Mission level: discovering a mission's FSMs and states from its INIT/WATCH roots, and
//! lowering the structured code of every function to the IR.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::decomp::{walk_calls, walk_cond_calls, CallV, DecompileError, FuncDecomp, Node, Structurer, Term, A, B, C, U, V};
use super::runtime::{LabelKind, Runtime, Target};
use super::x86::decode;
use super::{decomp::TargetCache, ExtractError, PeImage};
use crate::functions::{FuncId, ParamType};
use crate::ir::*;
use crate::{kas_hash, Hash};

/// One decompiled function.
pub(crate) struct Func {
    nodes: Vec<Node>,
    ast: Vec<A>,
    /// Instruction start addresses.
    addrs: Vec<u32>,
    /// Jump tables and byte maps inside the function.
    data: Vec<(u32, u32)>,
}

impl Func {
    /// Every call of the function: statement calls node by node, then the calls in the node's
    /// condition or switch value.
    fn calls(&self) -> Vec<Rc<CallV>> {
        let mut out = Vec::new();
        for n in &self.nodes {
            for s in &n.stmts {
                if let super::decomp::NStmt::Call(c) = s {
                    out.push(c.clone());
                }
            }
            match &n.term {
                Some(Term::If(c, ..)) => walk_cond_calls(c, &mut |x| out.push(x.clone())),
                Some(Term::Switch(v, _)) => walk_calls(v, &mut |x| out.push(x.clone())),
                _ => {}
            }
        }
        out
    }
}

/// Decompiles and structures one function.
pub(crate) fn decompile(pe: &PeImage<'_>, rt: &Runtime, cache: &mut TargetCache, a: u32) -> Result<Func, DecompileError> {
    let mut fd = FuncDecomp::new(pe, rt, cache, a)?;
    fd.run()?;
    let root = fd.root as isize;
    let addrs = std::mem::take(&mut fd.addrs);
    let data = std::mem::take(&mut fd.data);
    let mut nodes = std::mem::take(&mut fd.nodes);
    let ast = Structurer::new(&mut nodes, fd.root)?.emit(root, None)?;
    Ok(Func { nodes, ast, addrs, data })
}

/// End of the function at `a` (its code and the jump tables after it) rounded up to 16 bytes.
pub(crate) fn function_end(pe: &PeImage<'_>, rt: &Runtime, cache: &mut TargetCache, a: u32) -> Option<u32> {
    let fd = FuncDecomp::new(pe, rt, cache, a).ok()?;
    let end = fd.data.iter().map(|d| d.1).fold(fd.end, u32::max);
    Some((end + 15) & !15)
}

struct FsmInfo {
    hash: Hash,
    init: Option<u32>,
    watch: Option<u32>,
    states: Vec<(Hash, Option<u32>, Option<u32>)>,
}

/// Synthetic hash for a state no `JUMP` reaches (its label is not in the code).
const ORPHAN_FLAG: Hash = 0xF000_0000;

/// A mission being extracted.
pub(crate) struct MissionX<'a, 'p> {
    pe: &'a PeImage<'p>,
    rt: &'a Runtime,
    cache: TargetCache,
    pub name: String,
    pub init: u32,
    pub watch: u32,
    funcs: HashMap<u32, Result<Func, DecompileError>>,
    owner: HashMap<u32, Option<usize>>,
    fsms: Vec<FsmInfo>,
    seen: HashSet<(u32, usize)>,
}

impl<'a, 'p> MissionX<'a, 'p> {
    pub(crate) fn new(pe: &'a PeImage<'p>, rt: &'a Runtime, name: String, init: u32, watch: u32) -> Self {
        MissionX {
            pe,
            rt,
            cache: TargetCache::new(),
            name,
            init,
            watch,
            funcs: HashMap::new(),
            owner: HashMap::new(),
            fsms: Vec::new(),
            seen: HashSet::new(),
        }
    }

    fn func(&mut self, a: u32) -> &Result<Func, DecompileError> {
        if !self.funcs.contains_key(&a) {
            let f = decompile(self.pe, self.rt, &mut self.cache, a);
            self.funcs.insert(a, f);
        }
        &self.funcs[&a]
    }

    fn calls_of(&self, a: u32) -> Vec<Rc<CallV>> {
        match self.funcs.get(&a) {
            Some(Ok(f)) => f.calls(),
            _ => Vec::new(),
        }
    }

    fn const_args(c: &CallV, n: usize) -> Vec<Option<u32>> {
        c.args.iter().take(n).map(|v| if let V::Const(x) = v { Some(*x) } else { None }).collect()
    }

    fn fsm_create(&mut self, c: &CallV, work: &mut Vec<(Option<u32>, usize)>) {
        let a = Self::const_args(c, 3);
        let (Some(h), i, w) = (a[0], a[1], a[2]) else { return };
        if self.fsms.iter().all(|f| f.hash != h) {
            self.fsms.push(FsmInfo { hash: h, init: i, watch: w, states: Vec::new() });
            let fi = self.fsms.len() - 1;
            work.push((i, fi));
            work.push((w, fi));
        }
    }

    /// Follows `FSMCreate` from the mission roots and `JUMP` from FSM code.
    pub(crate) fn discover(&mut self) {
        self.func(self.init);
        self.func(self.watch);
        let mut work = Vec::new();
        for f in [self.init, self.watch] {
            for c in self.calls_of(f) {
                if c.target == Target::FsmCreate && c.args.len() == 4 {
                    self.fsm_create(&c, &mut work);
                }
            }
        }
        self.run(work);
    }

    fn run(&mut self, mut work: Vec<(Option<u32>, usize)>) {
        let mut k = 0;
        while k < work.len() {
            let (f, fsm) = work[k];
            k += 1;
            let Some(f) = f else { continue };
            if !self.pe.is_code(f) || !self.seen.insert((f, fsm)) {
                continue;
            }
            self.owner.entry(f).or_insert(Some(fsm));
            self.func(f);
            for c in self.calls_of(f) {
                if c.target == Target::Jump && c.args.len() == 3 {
                    let a = Self::const_args(&c, 3);
                    let Some(h) = a[0] else { continue };
                    let st = &mut self.fsms[fsm].states;
                    if st.iter().all(|s| s.0 != h) {
                        st.push((h, a[1], a[2]));
                        work.push((a[1], fsm));
                        work.push((a[2], fsm));
                    }
                } else if c.target == Target::FsmCreate && c.args.len() == 4 {
                    self.fsm_create(&c, &mut work);
                }
            }
        }
    }

    /// kas2c emits INIT/WATCH functions for every declared state, also ones no `JUMP` reaches.
    /// Unreferenced functions in `[lo, hi)` are attached, in (INIT, WATCH) pairs, to the FSM
    /// whose code precedes them. Returns how many states were added.
    pub(crate) fn orphans(&mut self, lo: u32, hi: u32) -> usize {
        if hi <= lo {
            return 0;
        }
        let mut covered = Coverage::new(lo, hi);
        for f in self.funcs.values().flatten() {
            covered.code(self.pe, &f.addrs);
            for &(s, e) in &f.data {
                covered.mark(s, e);
            }
        }
        let starts = gap_function_starts(self.pe, self.rt, &mut self.cache, &mut covered);
        let firsts = [0xa1u8, 0x8b, 0x68, 0xc7, 0x6a, 0x56, 0x53, 0x55, 0x57, 0x83, 0xc3];
        let cands: Vec<u32> = starts
            .into_iter()
            .filter(|a| !self.funcs.contains_key(a) && self.pe.code_at(*a).is_some_and(|b| firsts.contains(&b[0])))
            .collect();
        let mut known: Vec<(u32, Option<usize>)> =
            self.owner.iter().filter(|(a, _)| (lo..hi).contains(*a)).map(|(a, o)| (*a, *o)).collect();
        known.sort_unstable();
        let mut added = 0;
        let mut k = 0;
        while k < cands.len() {
            let a = cands[k];
            let Some(Some(fsm)) = known.iter().rev().find(|(x, _)| *x < a).map(|(_, o)| *o) else {
                k += 1;
                continue;
            };
            let w = cands.get(k + 1).copied();
            k += 2;
            let key = ORPHAN_FLAG | (a & 0x0fff_ffff);
            self.fsms[fsm].states.push((key, Some(a), w));
            self.run(vec![(Some(a), fsm), (w, fsm)]);
            added += 1;
        }
        added
    }

    /// Lowest and highest address of the mission's code.
    pub(crate) fn extent(&self) -> Option<(u32, u32)> {
        let all = self.funcs.values().flatten().flat_map(|f| f.addrs.iter().copied());
        let (mut lo, mut hi) = (u32::MAX, 0);
        for a in all {
            lo = lo.min(a);
            hi = hi.max(a);
        }
        (hi > 0).then_some((lo, hi))
    }

    /// Lowers everything to the IR. Also returns the global that indexes the localised string
    /// tables, if the mission uses any.
    pub(crate) fn lower(&self) -> Result<(Mission, Option<u32>), ExtractError> {
        let mut lw = Lowerer { pe: self.pe, x: self, lstrings: Vec::new(), lstring_at: HashMap::new(), lang: None };
        let fsm_index: HashMap<Hash, usize> = self.fsms.iter().enumerate().map(|(i, f)| (f.hash, i)).collect();
        let mut fsms = Vec::new();
        for f in &self.fsms {
            let states_map: HashMap<Hash, usize> = f.states.iter().enumerate().map(|(i, s)| (s.0, i)).collect();
            let cx = Ctx { states: Some(&states_map), fsms: &fsm_index };
            let what = |k: &str| format!("FSM {:08x} {k}", f.hash);
            let mut states = Vec::new();
            for &(sh, si, sw) in &f.states {
                let init = lw.block(si, &cx).map_err(|e| e.context(&what(&format!("state {sh:08x} INIT"))))?;
                let watch = lw.block(sw, &cx).map_err(|e| e.context(&what(&format!("state {sh:08x} WATCH"))))?;
                states.push(State { hash: sh, init, watch });
            }
            let init = lw.block(f.init, &cx).map_err(|e| e.context(&what("INIT")))?;
            let watch = lw.block(f.watch, &cx).map_err(|e| e.context(&what("WATCH")))?;
            fsms.push(Fsm { hash: f.hash, init, watch, states });
        }
        let cx = Ctx { states: None, fsms: &fsm_index };
        let init = lw.block(Some(self.init), &cx).map_err(|e| e.context("mission INIT"))?;
        let watch = lw.block(Some(self.watch), &cx).map_err(|e| e.context("mission WATCH"))?;
        let lstrings = lw
            .lstrings
            .iter()
            .map(|(addr, texts)| LString { name: None, addr: Some(*addr), texts: texts.clone() })
            .collect();
        let m = Mission {
            name: self.name.clone(),
            name_hash: kas_hash(&self.name),
            lstrings,
            fsms,
            init,
            watch,
            names: Default::default(),
            fidelity: Fidelity::Exact,
        };
        Ok((m, lw.lang))
    }

}

/// Function starts in the parts of `[lo, hi)` not covered by known code or data: the first
/// non-padding instruction at a 16-byte boundary, then after the end of each function found.
fn gap_function_starts(pe: &PeImage<'_>, rt: &Runtime, cache: &mut TargetCache, covered: &mut Coverage) -> Vec<u32> {
    let mut out = Vec::new();
    let (lo, hi) = (covered.lo, covered.hi);
    let mut a = lo;
    while a < hi {
        if covered.has(a) {
            a += 1;
            continue;
        }
        let Some(i) = pe.code_at(a).and_then(|b| decode(b, a)) else {
            a += 1;
            continue;
        };
        if i.is_padding() {
            a = i.next();
            continue;
        }
        if a % 16 != 0 {
            a = (a + 15) & !15;
            continue;
        }
        out.push(a);
        match FuncDecomp::new(pe, rt, cache, a) {
            Ok(fd) => {
                covered.code(pe, &fd.addrs);
                for &(s, e) in &fd.data {
                    covered.mark(s, e);
                }
                a = a.max(fd.end);
            }
            Err(_) => a += 16,
        }
    }
    out
}

/// Bytes of `[lo, hi)` known to be code or data of decoded functions.
struct Coverage {
    lo: u32,
    hi: u32,
    bits: Vec<bool>,
}

impl Coverage {
    fn new(lo: u32, hi: u32) -> Self {
        Coverage { lo, hi, bits: vec![false; (hi - lo) as usize] }
    }

    fn mark(&mut self, s: u32, e: u32) {
        let (s, e) = (s.max(self.lo), e.min(self.hi));
        for a in s..e {
            self.bits[(a - self.lo) as usize] = true;
        }
    }

    fn code(&mut self, pe: &PeImage<'_>, addrs: &[u32]) {
        for &a in addrs {
            if let Some(i) = pe.code_at(a).and_then(|b| decode(b, a)) {
                self.mark(a, i.next());
            }
        }
    }

    fn has(&self, a: u32) -> bool {
        self.bits[(a - self.lo) as usize]
    }
}

struct Ctx<'m> {
    states: Option<&'m HashMap<Hash, usize>>,
    fsms: &'m HashMap<Hash, usize>,
}

/// A lowered value before it is typed by the parameter it is passed to.
#[derive(Clone, Debug)]
enum Lv {
    Int(i32),
    Str(String),
    LStr(usize),
    Call(FuncId, Vec<Lv>),
    Bin(BinOp, Box<Lv>, Box<Lv>),
    Neg(Box<Lv>),
    BitNot(Box<Lv>),
    Cond(Cond),
    Label(LabelKind, Hash),
    ThisTeam,
    ThisTeamShips,
    ThisTeamsPoint,
}

/// A lowered statement (`Requeue` only survives as an unhandled-message default).
enum Ls {
    S(Stmt),
    Requeue,
}

struct Lowerer<'x, 'p> {
    pe: &'x PeImage<'p>,
    x: &'x MissionX<'x, 'p>,
    lstrings: Vec<(u32, [String; 3])>,
    lstring_at: HashMap<u32, usize>,
    lang: Option<u32>,
}

fn err(m: impl Into<String>) -> ExtractError {
    ExtractError::new(m)
}

type R<T> = Result<T, ExtractError>;

impl Lowerer<'_, '_> {
    fn block(&mut self, a: Option<u32>, cx: &Ctx<'_>) -> R<Block> {
        let a = a.ok_or_else(|| err("function address is not a constant"))?;
        let f = match self.x.funcs.get(&a) {
            Some(Ok(f)) => f,
            Some(Err(e)) => return Err(err(format!("function {a:08x} failed: {}", e.0))),
            None => return Err(err(format!("function {a:08x} not decompiled"))),
        };
        let stmts = self.stmts(&f.ast, true, false, cx)?;
        Ok(Block { addr: Some(a), stmts: finish(stmts)? })
    }

    fn stmts(&mut self, ast: &[A], top: bool, in_loop: bool, cx: &Ctx<'_>) -> R<Vec<Ls>> {
        let mut out = Vec::new();
        let mut k = 0;
        while k < ast.len() {
            let last = k + 1 == ast.len();
            match &ast[k] {
                A::Scope => {}
                A::Call(c) => match c.target {
                    Target::Jump if c.args.len() == 3 && matches!(c.args[0], V::Const(_)) => {
                        let V::Const(h) = c.args[0] else { unreachable!() };
                        let states = cx.states.ok_or_else(|| err("JUMP outside an FSM"))?;
                        let state = *states.get(&h).ok_or_else(|| err(format!("JUMP to unknown state {h:08x}")))?;
                        out.push(Ls::S(Stmt::Jump { state, hash: h }));
                        if matches!(ast.get(k + 1), Some(A::Return)) {
                            k += 1;
                        }
                    }
                    Target::FsmCreate if c.args.len() == 4 && matches!(c.args[0], V::Const(_)) => {
                        let V::Const(h) = c.args[0] else { unreachable!() };
                        let fsm = *cx.fsms.get(&h).ok_or_else(|| err(format!("FSMCreate of unknown FSM {h:08x}")))?;
                        let team = self.val(&c.args[3], None)?;
                        out.push(Ls::S(Stmt::FsmCreate { fsm, team: team_ref(team)? }));
                    }
                    Target::Label(_) | Target::ThisTeamsPoint => {}
                    Target::MsgSend if in_loop => out.push(Ls::Requeue),
                    _ => match self.call_value(c)? {
                        Lv::Call(f, args) => out.push(Ls::S(Stmt::Call(typed_call(f, args)?))),
                        _ => return Err(err("label lookup used as a statement")),
                    },
                },
                A::Return => {
                    if !(top && last) {
                        out.push(Ls::S(Stmt::Return));
                    }
                }
                A::If(c, then, els) => out.push(Ls::S(self.if_stmt(c, then, els, in_loop, cx)?)),
                A::Switch(var, arms, default) => {
                    if matches!(var, V::Call(cv) if cv.target == Target::MsgGet) {
                        return Err(err("message switch outside a message loop"));
                    }
                    let mut la = Vec::new();
                    for (vals, body) in arms {
                        let body = finish(self.stmts(body, false, in_loop, cx)?)?;
                        la.push(SwitchArm { values: vals.iter().map(|&v| v as i32).collect(), body });
                    }
                    let default = match default {
                        Some(d) => Some(finish(self.stmts(d, false, in_loop, cx)?)?),
                        None => None,
                    };
                    let value = expr(self.val(var, None)?)?;
                    out.push(Ls::S(Stmt::Switch { value, arms: la, default }));
                }
                A::MsgLoop(body) => out.push(Ls::S(self.msgloop(body, cx)?)),
                A::Store => return Err(err("store to memory")),
                A::Lost => return Err(err("pushed value never used")),
            }
            k += 1;
        }
        Ok(out)
    }

    fn if_stmt(&mut self, c: &C, then: &[A], els: &[A], in_loop: bool, cx: &Ctx<'_>) -> R<Stmt> {
        if let Some((var, cond2)) = match_ifonce(c, then) {
            let cond = match cond2 {
                Some(c2) => Some(self.cond(&c2)?),
                None => None,
            };
            let body = finish(self.stmts(&then[1..], false, in_loop, cx)?)?;
            let otherwise = finish(self.stmts(els, false, in_loop, cx)?)?;
            return Ok(Stmt::IfOnce { var, cond, body, otherwise });
        }
        let cond = self.cond(c)?;
        let then = finish(self.stmts(then, false, in_loop, cx)?)?;
        let otherwise = finish(self.stmts(els, false, in_loop, cx)?)?;
        Ok(Stmt::If { cond, then, otherwise })
    }

    fn msgloop(&mut self, body: &[A], cx: &Ctx<'_>) -> R<Stmt> {
        let body: Vec<&A> = body.iter().filter(|s| !matches!(s, A::Scope)).collect();
        let [A::Switch(var, arms, default)] = body.as_slice() else {
            return Err(err("unexpected message loop body"));
        };
        if !matches!(var, V::Call(cv) if cv.target == Target::MsgGet) {
            return Err(err("message loop does not switch on the message"));
        }
        let mut handlers = Vec::new();
        for (vals, arm) in arms {
            handlers.push(MsgHandler { msgs: vals.clone(), body: finish(self.stmts(arm, false, true, cx)?)? });
        }
        let default = match default {
            Some(d) => {
                let d = self.stmts(d, false, true, cx)?;
                if matches!(d.as_slice(), [Ls::Requeue]) {
                    None
                } else {
                    Some(finish(d)?)
                }
            }
            None => None,
        };
        Ok(Stmt::MsgLoop { handlers, default })
    }

    fn lstring(&mut self, table: u32) -> usize {
        if let Some(&i) = self.lstring_at.get(&table) {
            return i;
        }
        let texts: [String; 3] =
            std::array::from_fn(|k| self.pe.cstr(self.pe.u32_at(table + 4 * k as u32), 2048).unwrap_or_default());
        self.lstrings.push((table, texts));
        self.lstring_at.insert(table, self.lstrings.len() - 1);
        self.lstrings.len() - 1
    }

    fn val(&mut self, v: &V, pt: Option<ParamType>) -> R<Lv> {
        Ok(match v {
            V::Const(x) => {
                if pt == Some(ParamType::Str) && *x > 0x40_0000 {
                    if let Some(s) = self.pe.cstr(*x, 4096) {
                        return Ok(Lv::Str(s));
                    }
                }
                Lv::Int(*x as i32)
            }
            V::Team => Lv::ThisTeam,
            V::Call(c) => self.call_value(c)?,
            V::Index(table, idx, 4) => match **idx {
                V::Mem(g) if self.lang.is_none_or(|l| l == g) => {
                    self.lang = Some(g);
                    Lv::LStr(self.lstring(*table))
                }
                _ => return Err(err(format!("table access {v:?}"))),
            },
            V::Bin(B::Add, a, b) if **a == V::Team && **b == V::Const(0x1c) => Lv::ThisTeamShips,
            V::Bin(op, a, b) => {
                let op = match op {
                    B::Add => BinOp::Add,
                    B::Sub => BinOp::Sub,
                    B::Mul => BinOp::Mul,
                    B::Div => BinOp::Div,
                    B::Rem => BinOp::Rem,
                    B::And => BinOp::And,
                    B::Or => BinOp::Or,
                    B::Xor => BinOp::Xor,
                    B::Shl => BinOp::Shl,
                    B::Shr => BinOp::Shr,
                    B::MulHi => return Err(err("unfolded multiply-high")),
                };
                let a = self.val(a, None)?;
                let b = self.val(b, None)?;
                Lv::Bin(op, Box::new(a), Box::new(b))
            }
            V::Un(op, a) => match (&**a, op) {
                (V::Cond(c), U::Neg) => Lv::Neg(Box::new(Lv::Cond(self.cond(c)?))),
                (V::Cond(_), _) => return Err(err("unary operator on a condition")),
                (_, U::Neg) => Lv::Neg(Box::new(self.val(a, None)?)),
                (_, U::Not) => Lv::BitNot(Box::new(self.val(a, None)?)),
                (_, U::Sgn) => return Err(err("sign value")),
            },
            V::Cond(c) => Lv::Cond(self.cond(c)?),
            _ => return Err(err(format!("unsupported value {v:?}"))),
        })
    }

    fn call_value(&mut self, c: &CallV) -> R<Lv> {
        match c.target {
            Target::Label(k) if c.args.len() == 1 && matches!(c.args[0], V::Const(_)) => {
                let V::Const(h) = c.args[0] else { unreachable!() };
                Ok(Lv::Label(k, h))
            }
            Target::ThisTeamsPoint => Ok(Lv::ThisTeamsPoint),
            Target::Func(f) => {
                let params = f.info().params;
                if c.args.len() != params.len() {
                    return Err(err(format!("{}: {} args, table says {}", f.name(), c.args.len(), params.len())));
                }
                let mut args = Vec::with_capacity(params.len());
                for (a, &(_, pt)) in c.args.iter().zip(params) {
                    args.push(self.val(a, Some(pt))?);
                }
                Ok(Lv::Call(f, args))
            }
            t => Err(err(format!("runtime call {:08x} ({t:?}) used as a value", c.addr))),
        }
    }

    fn cond(&mut self, c: &C) -> R<Cond> {
        Ok(match c {
            C::And(a, b) => Cond::And(Box::new(self.cond(a)?), Box::new(self.cond(b)?)),
            C::Or(a, b) => Cond::Or(Box::new(self.cond(a)?), Box::new(self.cond(b)?)),
            C::Not(a) => Cond::Not(Box::new(self.cond(a)?)),
            C::True => Cond::True,
            C::False => Cond::False,
            C::Rel(op, a, b) => {
                let a = self.val(a, None)?;
                let b = self.val(b, None)?;
                Cond::Cmp(*op, expr(a)?, expr(b)?)
            }
            C::Unknown(v) => return Err(err(format!("uninterpreted condition {v:?}"))),
        })
    }
}

fn finish(v: Vec<Ls>) -> R<Vec<Stmt>> {
    v.into_iter()
        .map(|s| match s {
            Ls::S(s) => Ok(s),
            Ls::Requeue => Err(err("message re-queue outside a default handler")),
        })
        .collect()
}

/// IFONCE: `if (!VarGet(h) && cond) { VarCreateSet(h, 1); ... }`. Returns the guard variable and
/// the remaining condition.
fn match_ifonce(c: &C, then: &[A]) -> Option<(Hash, Option<C>)> {
    let A::Call(first) = then.first()? else { return None };
    let Target::Func(f) = first.target else { return None };
    if f.info().c != "kasfVarCreateSet" || first.args.len() != 2 {
        return None;
    }
    let (V::Const(h), V::Const(1)) = (&first.args[0], &first.args[1]) else { return None };
    let is_guard = |x: &C| {
        matches!(x, C::Rel(CmpOp::Eq, V::Call(g), V::Const(0))
            if matches!(g.target, Target::Func(gf) if gf.info().c == "kasfVarValueGet")
                && g.args.first() == Some(&V::Const(*h)))
    };
    if is_guard(c) {
        return Some((*h, None));
    }
    let mut conj = Vec::new();
    fn flat(x: &C, out: &mut Vec<C>) {
        if let C::And(a, b) = x {
            flat(a, out);
            flat(b, out);
        } else {
            out.push(x.clone());
        }
    }
    flat(c, &mut conj);
    if conj.len() >= 2 && is_guard(&conj[0]) {
        let mut rest = conj[1].clone();
        for x in &conj[2..] {
            rest = C::And(Rc::new(rest), Rc::new(x.clone()));
        }
        return Some((*h, Some(rest)));
    }
    None
}

fn expr(v: Lv) -> R<Expr> {
    Ok(match v {
        Lv::Int(n) => Expr::Int(n),
        Lv::Call(f, args) => {
            if !f.info().returns {
                return Err(err(format!("{} returns no value", f.name())));
            }
            Expr::Call(Box::new(typed_call(f, args)?))
        }
        Lv::Bin(op, a, b) => Expr::Bin(op, Box::new(expr(*a)?), Box::new(expr(*b)?)),
        Lv::Neg(a) => Expr::Neg(Box::new(expr(*a)?)),
        Lv::BitNot(a) => Expr::BitNot(Box::new(expr(*a)?)),
        Lv::Cond(c) => Expr::Cond(Box::new(c)),
        other => return Err(err(format!("integer expression expected, got {other:?}"))),
    })
}

fn typed_call(f: FuncId, args: Vec<Lv>) -> R<Call> {
    let params = f.info().params;
    let mut out = Vec::with_capacity(args.len());
    for (a, &(pname, pt)) in args.into_iter().zip(params) {
        out.push(arg(a, pt).map_err(|e| e.context(&format!("{}({pname})", f.name())))?);
    }
    Ok(Call { func: f, args: out })
}

fn arg(v: Lv, pt: ParamType) -> R<Arg> {
    use LabelKind as K;
    let bad = |what: &str, v: &Lv| err(format!("{what} argument expected, got {v:?}"));
    Ok(match pt {
        ParamType::Int | ParamType::Bool => Arg::Int(expr(v)?),
        ParamType::Hash => match v {
            Lv::Int(n) => Arg::Hash(n as u32),
            v => return Err(bad("hash", &v)),
        },
        ParamType::Str => Arg::Str(match v {
            Lv::Str(s) => StrRef::Text(s),
            Lv::LStr(i) => StrRef::Local(i),
            Lv::Int(0) => StrRef::Null,
            Lv::Int(n) => StrRef::Int(n),
            v => return Err(bad("string", &v)),
        }),
        ParamType::Team => Arg::Team(team_ref(v)?),
        ParamType::Ships => Arg::Ships(match v {
            Lv::Label(K::Ships, h) => ShipsRef::Selection(h),
            Lv::Label(K::TeamShips, h) => ShipsRef::Team(TeamRef::Label(h)),
            Lv::ThisTeamShips => ShipsRef::Team(TeamRef::This),
            Lv::Int(0) => ShipsRef::Null,
            v => return Err(bad("selection", &v)),
        }),
        ParamType::Point => Arg::Point(match v {
            Lv::Label(K::Point, h) => PointRef::Point(h),
            Lv::Label(K::ShipsPoint, h) => PointRef::Ships(h),
            Lv::Label(K::TeamsPoint, h) => PointRef::Team(TeamRef::Label(h)),
            Lv::Label(K::VolumePoint, h) => PointRef::Volume(h),
            Lv::ThisTeamsPoint => PointRef::Team(TeamRef::This),
            Lv::Int(0) => PointRef::Null,
            v => return Err(bad("point", &v)),
        }),
        ParamType::Volume => Arg::Volume(match v {
            Lv::Label(K::Volume, h) => Some(h),
            Lv::Int(0) => None,
            v => return Err(bad("volume", &v)),
        }),
        ParamType::Path => Arg::Path(match v {
            Lv::Label(K::Path, h) => Some(h),
            Lv::Int(0) => None,
            v => return Err(bad("path", &v)),
        }),
    })
}

fn team_ref(v: Lv) -> R<TeamRef> {
    Ok(match v {
        Lv::Label(LabelKind::Team, h) => TeamRef::Label(h),
        Lv::ThisTeam => TeamRef::This,
        Lv::Int(0) => TeamRef::Null,
        v => return Err(err(format!("team argument expected, got {v:?}"))),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::functions::ids;

    /// A one-section PE32 image whose code starts at 0x401000.
    fn image(code: &[(u32, &[u8])]) -> Vec<u8> {
        let mut f = vec![0u8; 0x400];
        f[..2].copy_from_slice(b"MZ");
        f[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
        f[0x40..0x44].copy_from_slice(b"PE\0\0");
        f[0x44..0x46].copy_from_slice(&0x14cu16.to_le_bytes());
        f[0x46..0x48].copy_from_slice(&1u16.to_le_bytes());
        f[0x54..0x56].copy_from_slice(&0xe0u16.to_le_bytes());
        f[0x58..0x5a].copy_from_slice(&0x10bu16.to_le_bytes());
        f[0x74..0x78].copy_from_slice(&0x40_0000u32.to_le_bytes());
        let s = 0x138;
        f[s..s + 5].copy_from_slice(b".text");
        for (o, v) in [(8, 0x200u32), (12, 0x1000), (16, 0x200), (20, 0x200), (36, 0x6000_0020)] {
            f[s + o..s + o + 4].copy_from_slice(&v.to_le_bytes());
        }
        for (va, bytes) in code {
            let o = (va - 0x40_1000 + 0x200) as usize;
            f[o..o + bytes.len()].copy_from_slice(bytes);
        }
        f
    }

    fn le(v: u32) -> [u8; 4] {
        v.to_le_bytes()
    }

    #[test]
    fn extracts_a_synthetic_mission() {
        let (fsm, state, var) = (0x0abc_def0u32, 0x0123_4567u32, 0x1234u32);
        let name = kas_hash("Mission99");
        let mut init_m = vec![0xc7, 0x05, 0x00, 0x00, 0x50, 0x00, 0, 0, 0, 0, 0xc7, 0x05, 0x04, 0x00, 0x50, 0x00];
        init_m.extend(le(name));
        init_m.extend([0x6a, 0x00, 0x68, 0x70, 0x10, 0x40, 0x00, 0x68, 0x50, 0x10, 0x40, 0x00, 0x68]);
        init_m.extend(le(fsm));
        init_m.extend([0xe8, 0xe6, 0, 0, 0, 0x83, 0xc4, 0x10, 0xc3]);
        let mut fsm_init = vec![0x68, 0xb0, 0x10, 0x40, 0x00, 0x68, 0x80, 0x10, 0x40, 0x00, 0x68];
        fsm_init.extend(le(state));
        fsm_init.extend([0xe8, 0x9c, 0, 0, 0, 0x83, 0xc4, 0x0c, 0xc3]);
        let mut state_init = vec![0x68];
        state_init.extend(le(var));
        state_init.extend([0xe8, 0x96, 0, 0, 0, 0x83, 0xc4, 0x04, 0x85, 0xc0, 0x75, 0x0f, 0x6a, 0x01, 0x68]);
        state_init.extend(le(var));
        state_init.extend([0xe8, 0x93, 0, 0, 0, 0x83, 0xc4, 0x08, 0xc3]);
        let exe = image(&[
            (0x40_1000, &init_m),
            (0x40_1040, &[0xc3]),
            (0x40_1050, &fsm_init),
            (0x40_1070, &[0xc3]),
            (0x40_1080, &state_init),
            (0x40_10b0, &[0xc3]),
            (0x40_1100, &[0xff, 0x64, 0x24, 0x08]),       // kasJump: jmp [esp+8]
            (0x40_1110, &[0xff, 0x54, 0x24, 0x10, 0xc3]), // kasFSMCreate: call [esp+0x10]
            (0x40_1120, &[0xc3]),
            (0x40_1130, &[0xc3]),
        ]);
        let pe = PeImage::parse(&exe).unwrap();
        let funcs = HashMap::from([(0x40_1120, ids::VAR_GET), (0x40_1130, ids::VAR_CREATE_SET)]);
        let rt = Runtime::for_tests(funcs, 0x50_0008, [0x50_0000, 0x50_0004]);
        let mut mx = MissionX::new(&pe, &rt, "Mission99".into(), 0x40_1000, 0x40_1040);
        mx.discover();
        let (m, lang) = mx.lower().unwrap();
        assert_eq!(lang, None);
        assert_eq!(m.init.stmts, vec![Stmt::FsmCreate { fsm: 0, team: TeamRef::Null }]);
        assert!(m.watch.stmts.is_empty());
        assert_eq!(m.fsms.len(), 1);
        let f = &m.fsms[0];
        assert_eq!((f.hash, f.init.addr, f.watch.addr), (fsm, Some(0x40_1050), Some(0x40_1070)));
        assert_eq!(f.init.stmts, vec![Stmt::Jump { state: 0, hash: state }]);
        assert_eq!(f.states.len(), 1);
        assert_eq!(f.states[0].init.stmts, vec![Stmt::IfOnce { var, cond: None, body: vec![], otherwise: vec![] }]);
        crate::binary::validate(&m).unwrap();
    }
}
