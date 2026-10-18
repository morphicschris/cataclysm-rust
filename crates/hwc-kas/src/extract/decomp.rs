//! Function-level decompilation: symbolic execution of one kas2c function into a graph of
//! nodes (statements + a terminator), then structuring of that graph into nested statements.
//!
//! Symbolic execution tracks the registers and the argument stack as expressions (constants,
//! call results, the current team, loaded globals), turns each `call` into a call node with its
//! pushed arguments, and each conditional branch into a condition over the last flag-setting
//! instruction. States reaching the same address with the same live registers and stack are
//! merged, so code shared between branches (MSVC tail merging) becomes shared nodes.
//!
//! Structuring merges short-circuit condition chains, computes post-dominators and emits
//! IF/ELSE regions, `switch` statements (jump tables and compare trees over one call value) and
//! the Cataclysm message loop.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use super::runtime::{Runtime, Target};
use super::x86::{decode, Cc, Insn, Mem, Op, Operand, Reg};
use super::PeImage;
use crate::ir::CmpOp;

/// A decompilation failure.
#[derive(Debug, Clone)]
pub(crate) struct DecompileError(pub String);

type Res<T> = Result<T, DecompileError>;

fn fail<T>(m: impl Into<String>) -> Res<T> {
    Err(DecompileError(m.into()))
}

// ---------------------------------------------------------------------------------------------
// values
// ---------------------------------------------------------------------------------------------

/// Binary operators of symbolic values.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum B {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    And,
    Or,
    Xor,
    Shl,
    Shr,
    /// High half of a 32x32 multiply.
    MulHi,
}

/// Unary operators.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum U {
    Neg,
    Not,
    /// `cdq`: 0 or -1 by the sign.
    Sgn,
}

/// A symbolic 32-bit value.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum V {
    Const(u32),
    /// `CurrentTeamP`.
    Team,
    /// The message loop index.
    MsgIndex,
    Call(Rc<CallV>),
    /// A loaded global.
    Mem(u32),
    /// `[base + off]`.
    Field(Rc<V>, i32),
    /// `[idx * scale + table]`.
    Index(u32, Rc<V>, u8),
    Bin(B, Rc<V>, Rc<V>),
    Un(U, Rc<V>),
    /// Unknown, with a description.
    Unk(Rc<str>),
    /// A condition used as a value.
    Cond(Rc<C>),
}

/// A call: target, call site and arguments. `id` distinguishes call instances and is not part
/// of the value's identity.
#[derive(Clone, Debug)]
pub(crate) struct CallV {
    pub addr: u32,
    pub at: u32,
    pub args: Vec<V>,
    pub target: Target,
    pub id: u32,
}

impl PartialEq for CallV {
    fn eq(&self, o: &Self) -> bool {
        self.addr == o.addr && self.at == o.at && self.args == o.args
    }
}
impl Eq for CallV {}
impl std::hash::Hash for CallV {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        self.addr.hash(h);
        self.at.hash(h);
        self.args.hash(h);
    }
}

/// A condition.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum C {
    Rel(CmpOp, V, V),
    And(Rc<C>, Rc<C>),
    Or(Rc<C>, Rc<C>),
    Not(Rc<C>),
    True,
    False,
    /// Flags the executor could not interpret.
    Unknown(V),
}

fn k(v: i64) -> V {
    V::Const(v as u32)
}

fn unk(s: &str) -> V {
    V::Unk(Rc::from(s))
}

fn signed(v: u32) -> i64 {
    i64::from(v as i32)
}

fn bin(op: B, a: V, b: V) -> V {
    V::Bin(op, Rc::new(a), Rc::new(b))
}

/// `mulhi(x, C) >> k` -> `(x, C, k)`.
fn magic(v: &V) -> Option<(&V, u32, u32)> {
    let (mut v, mut sh) = (v, 0);
    if let V::Bin(B::Shr, a, b) = v {
        if let V::Const(s) = **b {
            sh = s;
            v = a;
        }
    }
    if let V::Bin(B::MulHi, a, b) = v {
        if let V::Const(c) = **b {
            return Some((a, c, sh));
        }
    }
    None
}

fn is_sgn_of(y: &V, x: &V) -> bool {
    match y {
        V::Un(U::Sgn, a) => **a == *x,
        V::Bin(B::And, a, b) if matches!(**b, V::Const(_)) => is_sgn_of(a, x),
        _ => false,
    }
}

/// Builds a binary value with MSVC idiom folding (division by constants) and constant folding.
pub(crate) fn mk_bin(op: B, a: V, b: V) -> V {
    // q = (mulhi(x, C) >> k); q += q >> 31  ->  x / round(2^(32+k) / C)
    if op == B::Add {
        if let V::Bin(B::Shr, ba, bb) = &b {
            if **bb == V::Const(31) {
                if let Some((x, c, sh)) = magic(&a) {
                    if **ba == a {
                        let c = signed(c);
                        if c != 0 {
                            let q = ((2f64).powi(32 + sh as i32) / c as f64).round_ties_even() as i128;
                            return bin(B::Div, x.clone(), V::Const(q as u32));
                        }
                    }
                }
            }
        }
    }
    // (x - sgn(x)) >> k  /  (x + (sgn(x) & m)) >> k  ->  x / 2^k
    if op == B::Shr {
        if let (V::Const(s), V::Bin(B::Add | B::Sub, aa, ab)) = (&b, &a) {
            if is_sgn_of(ab, aa) {
                let d = if *s < 32 { 1u32 << s } else { 0 };
                return bin(B::Div, (**aa).clone(), V::Const(d));
            }
        }
    }
    if let (V::Const(x), V::Const(y)) = (&a, &b) {
        let (x, y) = (signed(*x), signed(*y));
        let r = match op {
            B::Add => Some(x + y),
            B::Sub => Some(x - y),
            B::Mul => Some(x * y),
            B::And => Some(x & y),
            B::Or => Some(x | y),
            B::Xor => Some(x ^ y),
            B::Shl => Some(x << (y & 31)),
            B::Shr => Some(x >> (y & 31)),
            B::Div => (y != 0).then(|| x / y),
            B::Rem | B::MulHi => None,
        };
        if let Some(r) = r {
            return k(r);
        }
    }
    if matches!(op, B::Add | B::Sub) && b == V::Const(0) {
        return a;
    }
    if op == B::Add && a == V::Const(0) {
        return b;
    }
    if op == B::Sub {
        if let V::Bin(B::Mul, aa, ab) = &a {
            if let V::Const(m) = **ab {
                if **aa == b {
                    return bin(B::Mul, b, k(signed(m) - 1));
                }
            }
        }
    }
    if let (V::Bin(aop @ (B::Add | B::Sub), aa, ab), V::Const(bv)) = (&a, &b) {
        if let V::Const(av) = **ab {
            let (av, bv) = (signed(av), signed(*bv));
            if op == B::Add {
                return mk_bin(*aop, (**aa).clone(), k(av + if *aop == B::Add { bv } else { -bv }));
            }
            if op == B::Sub {
                return mk_bin(*aop, (**aa).clone(), k(av + if *aop == B::Add { -bv } else { bv }));
            }
        }
    }
    bin(op, a, b)
}

/// A comparison, normalising `(x - c) op k` to `x op (k + c)`.
fn rel(op: CmpOp, mut a: V, mut b: V) -> C {
    while let (V::Bin(aop @ (B::Add | B::Sub), aa, ab), V::Const(bv)) = (&a, &b) {
        let V::Const(av) = **ab else { break };
        let nb = if *aop == B::Sub { signed(*bv) + signed(av) } else { signed(*bv) - signed(av) };
        let na = (**aa).clone();
        a = na;
        b = k(nb);
    }
    C::Rel(op, a, b)
}

pub(crate) fn cnot(c: &C) -> C {
    match c {
        C::Rel(op, a, b) => C::Rel(op.negate(), a.clone(), b.clone()),
        C::Not(a) => (**a).clone(),
        C::And(a, b) => C::Or(Rc::new(cnot(a)), Rc::new(cnot(b))),
        C::Or(a, b) => C::And(Rc::new(cnot(a)), Rc::new(cnot(b))),
        C::True => C::False,
        C::False => C::True,
        C::Unknown(_) => C::Not(Rc::new(c.clone())),
    }
}

fn cand(a: C, b: C) -> C {
    if a == C::True {
        return b;
    }
    if b == C::True {
        return a;
    }
    C::And(Rc::new(a), Rc::new(b))
}

fn cor(a: C, b: C) -> C {
    if a == C::False {
        return b;
    }
    if b == C::False {
        return a;
    }
    C::Or(Rc::new(a), Rc::new(b))
}

/// Calls the closure for every call inside a value (pre-order).
pub(crate) fn walk_calls<'a>(v: &'a V, f: &mut dyn FnMut(&'a Rc<CallV>)) {
    match v {
        V::Call(c) => {
            f(c);
            for a in &c.args {
                walk_calls(a, f);
            }
        }
        V::Bin(_, a, b) => {
            walk_calls(a, f);
            walk_calls(b, f);
        }
        V::Un(_, a) | V::Field(a, _) | V::Index(_, a, _) => walk_calls(a, f),
        V::Cond(c) => walk_cond_calls(c, f),
        _ => {}
    }
}

pub(crate) fn walk_cond_calls<'a>(c: &'a C, f: &mut dyn FnMut(&'a Rc<CallV>)) {
    match c {
        C::Rel(_, a, b) => {
            walk_calls(a, f);
            walk_calls(b, f);
        }
        C::And(a, b) | C::Or(a, b) => {
            walk_cond_calls(a, f);
            walk_cond_calls(b, f);
        }
        C::Not(a) => walk_cond_calls(a, f),
        C::Unknown(v) => walk_calls(v, f),
        C::True | C::False => {}
    }
}

// ---------------------------------------------------------------------------------------------
// graph
// ---------------------------------------------------------------------------------------------

/// A straight-line statement of a node.
#[derive(Clone, Debug)]
pub(crate) enum NStmt {
    Call(Rc<CallV>),
    /// A store to the KAS scope globals (bookkeeping, no effect on the IR).
    Scope,
    /// Any other store (the lowering rejects it).
    Store(V),
    /// A pushed value dropped without being consumed.
    Lost,
}

/// How a node ends.
#[derive(Clone, Debug)]
pub(crate) enum Term {
    Goto(usize),
    If(C, usize, usize),
    /// Jump table: value, (case value, target) in table order.
    Switch(V, Vec<(u32, usize)>),
    /// Message loop: body, exit.
    MsgLoop(usize, usize),
    Ret,
    /// Back to the message loop header.
    Continue,
}

#[derive(Clone, Debug)]
pub(crate) struct Node {
    pub addr: u32,
    pub stmts: Vec<NStmt>,
    pub term: Option<Term>,
    pub preds: Vec<usize>,
}

impl Node {
    fn succs(&self) -> Vec<usize> {
        match &self.term {
            Some(Term::Goto(t)) => vec![*t],
            Some(Term::If(_, t, f)) => vec![*t, *f],
            Some(Term::Switch(_, cases)) => {
                let mut v: Vec<usize> = Vec::new();
                for &(_, t) in cases {
                    if !v.contains(&t) {
                        v.push(t);
                    }
                }
                v
            }
            Some(Term::MsgLoop(b, e)) => vec![*b, *e],
            _ => Vec::new(),
        }
    }

    fn pure_stmts(&self) -> bool {
        self.stmts.iter().all(|s| matches!(s, NStmt::Scope))
    }
}

#[derive(Clone, Debug)]
enum Flags {
    Test(V),
    Cmp(V, V),
    Res(V),
}

#[derive(Clone, Debug)]
struct State {
    regs: [V; 8],
    stack: Vec<(V, bool)>,
    flags: Option<Flags>,
    carry: Option<C>,
    in_loop: Option<u32>,
}

const GPRS: [Reg; 7] = [Reg::Eax, Reg::Ecx, Reg::Edx, Reg::Ebx, Reg::Esi, Reg::Edi, Reg::Ebp];

impl State {
    fn new() -> State {
        let names = ["in_eax", "in_ecx", "in_edx", "in_ebx", "esp", "in_ebp", "in_esi", "in_edi"];
        State { regs: names.map(unk), stack: Vec::new(), flags: None, carry: None, in_loop: None }
    }

    fn reg(&self, r: Reg) -> V {
        if r == Reg::Esp {
            unk("esp")
        } else {
            self.regs[r.index()].clone()
        }
    }

    /// Python-style negative indexing of the stack from the top: slot `k` is `stack[-1-k]`.
    fn slot(&self, k: i64) -> Option<usize> {
        let n = self.stack.len() as i64;
        if k >= n {
            return None;
        }
        let i = -1 - k;
        let i = if i < 0 { n + i } else { i };
        (0..n).contains(&i).then_some(i as usize)
    }
}

#[derive(PartialEq, Eq, Hash)]
struct Key {
    addr: u32,
    regs: Vec<V>,
    stack: Vec<Option<V>>,
    in_loop: Option<u32>,
}

// ---------------------------------------------------------------------------------------------
// function decompiler
// ---------------------------------------------------------------------------------------------

const MAX_NODES: usize = 6000;

/// Per-thread cache of call-target classifications.
pub(crate) type TargetCache = HashMap<u32, Target>;

/// One function: its instruction CFG and (after [`FuncDecomp::run`]) its node graph.
pub(crate) struct FuncDecomp<'a, 'p> {
    pe: &'a PeImage<'p>,
    rt: &'a Runtime,
    cache: &'a mut TargetCache,
    pub entry: u32,
    insn: HashMap<u32, Insn>,
    cfg_succ: HashMap<u32, Vec<u32>>,
    leaders: HashSet<u32>,
    /// Instruction addresses, sorted.
    pub addrs: Vec<u32>,
    /// End of the highest instruction.
    pub end: u32,
    /// Jump table and byte map ranges (data inside the function).
    pub data: Vec<(u32, u32)>,
    live_in: HashMap<u32, u8>,
    pub nodes: Vec<Node>,
    memo: HashMap<Key, usize>,
    pub root: usize,
    next_id: u32,
}

fn reg_bit(r: Reg) -> u8 {
    match r {
        Reg::Esp => 0,
        r => 1 << r.index(),
    }
}

fn mem_regs(m: &Mem) -> u8 {
    m.base.map_or(0, reg_bit) | m.index.map_or(0, reg_bit)
}

fn opnd_read(o: Option<Operand>) -> u8 {
    match o {
        Some(Operand::Reg { reg, .. }) => reg_bit(reg),
        Some(Operand::Mem(m)) => mem_regs(&m),
        _ => 0,
    }
}

fn dst_write(o: Option<Operand>) -> u8 {
    match o {
        Some(Operand::Reg { reg, .. }) => reg_bit(reg),
        _ => 0,
    }
}

fn dst_read(o: Option<Operand>) -> u8 {
    // a memory destination reads its address registers
    match o {
        Some(Operand::Mem(m)) => mem_regs(&m),
        _ => 0,
    }
}

fn same_reg(i: &Insn) -> bool {
    matches!((i.op(0), i.op(1)), (Some(a @ Operand::Reg { .. }), Some(b @ Operand::Reg { .. })) if a == b)
}

/// Registers read and written (general purpose registers only, as bit masks).
fn reads_writes(i: &Insn) -> (u8, u8) {
    let (o0, o1, o2) = (i.op(0), i.op(1), i.op(2));
    let eax = reg_bit(Reg::Eax);
    let edx = reg_bit(Reg::Edx);
    match i.op {
        Op::Mov | Op::Movzx | Op::Movsx | Op::Lea => (dst_read(o0) | opnd_read(o1), dst_write(o0)),
        Op::Push => (opnd_read(o0), 0),
        Op::Pop => (dst_read(o0), dst_write(o0)),
        Op::Add | Op::Or | Op::Adc | Op::Sbb | Op::And | Op::Sub | Op::Xor => {
            let mut rd = opnd_read(o0) | opnd_read(o1);
            if matches!(i.op, Op::Xor | Op::Sub) && same_reg(i) {
                rd &= !opnd_read(o0);
            }
            (rd, dst_write(o0))
        }
        Op::Cmp | Op::Test => (opnd_read(o0) | opnd_read(o1), 0),
        Op::Inc | Op::Dec | Op::Neg | Op::Not => (opnd_read(o0), dst_write(o0)),
        Op::Shl | Op::Shr | Op::Sar | Op::Rotate => (opnd_read(o0) | opnd_read(o1), dst_write(o0)),
        Op::Imul if o2.is_some() => (opnd_read(o1), dst_write(o0)),
        Op::Imul if o1.is_some() => (opnd_read(o0) | opnd_read(o1), dst_write(o0)),
        Op::Mul | Op::Imul => (eax | opnd_read(o0), eax | edx),
        Op::Div | Op::Idiv => (eax | edx | opnd_read(o0), eax | edx),
        Op::Cdq => (eax, edx),
        Op::Setcc(_) => (dst_read(o0), dst_write(o0)),
        Op::Xchg => (opnd_read(o0) | opnd_read(o1), dst_write(o0) | dst_write(o1)),
        Op::Call => (0, eax | reg_bit(Reg::Ecx) | edx),
        Op::Ret => (0, 0),
        Op::Jmp => (opnd_read(o0) & if i.target().is_some() { 0 } else { 0xff }, 0),
        _ => (0, 0),
    }
}

fn cc_rel_test(cc: Cc) -> Option<CmpOp> {
    Some(match cc {
        Cc::E => CmpOp::Eq,
        Cc::Ne => CmpOp::Ne,
        Cc::S | Cc::L => CmpOp::Lt,
        Cc::Ns | Cc::Ge => CmpOp::Ge,
        Cc::Le => CmpOp::Le,
        Cc::G => CmpOp::Gt,
        _ => return None,
    })
}

fn cc_name(cc: Cc) -> &'static str {
    match cc {
        Cc::O => "o",
        Cc::No => "no",
        Cc::B => "b",
        Cc::Ae => "ae",
        Cc::E => "e",
        Cc::Ne => "ne",
        Cc::Be => "be",
        Cc::A => "a",
        Cc::S => "s",
        Cc::Ns => "ns",
        Cc::P => "p",
        Cc::Np => "np",
        Cc::L => "l",
        Cc::Ge => "ge",
        Cc::Le => "le",
        Cc::G => "g",
    }
}

impl<'a, 'p> FuncDecomp<'a, 'p> {
    /// Builds the instruction CFG and liveness of the function at `entry`.
    pub(crate) fn new(pe: &'a PeImage<'p>, rt: &'a Runtime, cache: &'a mut TargetCache, entry: u32) -> Res<Self> {
        let mut fd = FuncDecomp {
            pe,
            rt,
            cache,
            entry,
            insn: HashMap::new(),
            cfg_succ: HashMap::new(),
            leaders: HashSet::new(),
            addrs: Vec::new(),
            end: entry,
            data: Vec::new(),
            live_in: HashMap::new(),
            nodes: Vec::new(),
            memo: HashMap::new(),
            root: 0,
            next_id: 0,
        };
        fd.build_cfg()?;
        fd.liveness();
        Ok(fd)
    }

    fn dec(&mut self, a: u32) -> Res<Insn> {
        if let Some(i) = self.insn.get(&a) {
            return Ok(*i);
        }
        let i = self
            .pe
            .code_at(a)
            .and_then(|b| decode(b, a))
            .ok_or_else(|| DecompileError(format!("cannot decode at {a:08x}")))?;
        self.insn.insert(a, i);
        Ok(i)
    }

    fn prev_insn(&self, a: u32) -> Option<Insn> {
        (1..16).find_map(|back| self.insn.get(&a.wrapping_sub(back)).filter(|p| p.next() == a).copied())
    }

    /// MSVC switch: `cmp r, N; ja default; jmp [r*4 + T]`, or with a byte map
    /// `cmp r, N; ja default; xor edx, edx; mov dl, [r + M]; jmp [edx*4 + T]`.
    /// Returns the index register and the targets for index 0..=N.
    fn jumptable(&mut self, i: &Insn) -> Option<(Reg, Vec<u32>)> {
        let m = *i.op(0)?.mem()?;
        if m.scale != 4 || m.base.is_some() {
            return None;
        }
        let table = m.disp as u32;
        let mut idx = m.index?;
        let mut bytemap = None;
        if let Some(p) = self.prev_insn(i.addr) {
            if p.op == Op::Mov {
                if let Some(Operand::Mem(pm)) = p.op(1) {
                    if pm.size == 1 {
                        bytemap = Some(pm.disp as u32);
                        idx = pm.base?;
                    }
                }
            }
        }
        let mut n: Option<u64> = None;
        let mut a = i.addr;
        for _ in 0..8 {
            let Some(p) = self.prev_insn(a) else { break };
            if p.op == Op::Cmp {
                if let (Some(Operand::Reg { reg, .. }), Some(Operand::Imm(v))) = (p.op(0), p.op(1)) {
                    if reg == idx {
                        n = Some(u64::from(v) + 1);
                        break;
                    }
                }
            }
            a = p.addr;
        }
        let n = n.filter(|&n| n <= 1024)? as u32;
        let targets = match bytemap {
            Some(map) => {
                self.data.push((map, map + n));
                let max = (0..n).map(|k| self.pe.u8_at(map + k)).max().unwrap_or(0);
                self.data.push((table, table + 4 * (u32::from(max) + 1)));
                (0..n).map(|k| self.pe.u32_at(table + 4 * u32::from(self.pe.u8_at(map + k)))).collect()
            }
            None => {
                self.data.push((table, table + 4 * n));
                (0..n).map(|k| self.pe.u32_at(table + 4 * k)).collect()
            }
        };
        Some((idx, targets))
    }

    fn build_cfg(&mut self) -> Res<()> {
        let mut work = vec![self.entry];
        self.leaders.insert(self.entry);
        let mut seen: HashSet<u32> = HashSet::new();
        while let Some(mut a) = work.pop() {
            while !seen.contains(&a) {
                if !self.pe.is_code(a) || seen.len() > 40000 {
                    return fail(format!("code runs outside the code section at {a:08x}"));
                }
                seen.insert(a);
                let i = self.dec(a)?;
                let nxt = i.next();
                match i.op {
                    Op::Ret | Op::Int3 | Op::Hlt | Op::Ud2 => {
                        self.cfg_succ.insert(a, Vec::new());
                        break;
                    }
                    Op::Jmp => {
                        if let Some(t) = i.target() {
                            if self.rt.is_tail_target(self.pe, t) {
                                self.cfg_succ.insert(a, Vec::new());
                            } else {
                                self.cfg_succ.insert(a, vec![t]);
                                self.leaders.insert(t);
                                work.push(t);
                            }
                        } else {
                            let (_, tab) =
                                self.jumptable(&i).ok_or_else(|| DecompileError(format!("indirect jmp at {a:08x}")))?;
                            let mut uniq: Vec<u32> = Vec::new();
                            for &t in &tab {
                                if !uniq.contains(&t) {
                                    uniq.push(t);
                                }
                                self.leaders.insert(t);
                                work.push(t);
                            }
                            self.cfg_succ.insert(a, uniq);
                        }
                        break;
                    }
                    Op::Jcc(_) | Op::Loop => {
                        let t = i.target().unwrap_or(nxt);
                        self.cfg_succ.insert(a, vec![nxt, t]);
                        self.leaders.insert(nxt);
                        self.leaders.insert(t);
                        work.push(t);
                        a = nxt;
                        continue;
                    }
                    _ => {}
                }
                self.cfg_succ.insert(a, vec![nxt]);
                a = nxt;
            }
        }
        let mut addrs: Vec<u32> = seen.into_iter().collect();
        addrs.sort_unstable();
        let last = *addrs.last().unwrap();
        self.end = self.insn[&last].next();
        self.addrs = addrs;
        Ok(())
    }

    fn liveness(&mut self) {
        let rw: HashMap<u32, (u8, u8)> = self.addrs.iter().map(|a| (*a, reads_writes(&self.insn[a]))).collect();
        for a in &self.addrs {
            self.live_in.insert(*a, 0);
        }
        let order: Vec<u32> = self.addrs.iter().rev().copied().collect();
        loop {
            let mut changed = false;
            for a in &order {
                let mut out = 0u8;
                for s in self.cfg_succ.get(a).map(Vec::as_slice).unwrap_or(&[]) {
                    out |= self.live_in.get(s).copied().unwrap_or(0);
                }
                let (rd, wr) = rw[a];
                let li = rd | (out & !wr);
                if li != self.live_in[a] {
                    self.live_in.insert(*a, li);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
    }

    // ---- symbolic execution ------------------------------------------------------------------

    fn operand(&self, st: &State, o: Option<Operand>) -> V {
        match o {
            Some(Operand::Imm(v)) | Some(Operand::Rel(v)) => V::Const(v),
            Some(Operand::Reg { reg, .. }) => st.reg(reg),
            Some(Operand::Mem(m)) => self.memval(st, &m),
            None => unk("op"),
        }
    }

    fn memval(&self, st: &State, m: &Mem) -> V {
        let disp = m.disp as u32;
        if m.base == Some(Reg::Esp) {
            let k = i64::from(m.disp).div_euclid(4);
            return match st.slot(k) {
                Some(i) => st.stack[i].0.clone(),
                None => unk("stk"),
            };
        }
        match (m.base, m.index) {
            (None, None) if disp == self.rt.team_ptr => V::Team,
            (None, None) => V::Mem(disp),
            (None, Some(ix)) => V::Index(disp, Rc::new(st.reg(ix)), m.scale),
            (Some(_), Some(_)) => unk("mem2"),
            (Some(b), None) => V::Field(Rc::new(st.reg(b)), m.disp),
        }
    }

    fn write(&mut self, st: &mut State, nid: usize, o: Option<Operand>, val: V) {
        match o {
            Some(Operand::Reg { reg, .. }) => {
                if reg != Reg::Esp {
                    st.regs[reg.index()] = val;
                }
            }
            Some(Operand::Mem(m)) => {
                if m.base.is_none() && m.index.is_none() && self.rt_scope(m.disp as u32) {
                    self.nodes[nid].stmts.push(NStmt::Scope);
                    return;
                }
                if m.base == Some(Reg::Esp) {
                    let k = i64::from(m.disp).div_euclid(4);
                    if let Some(i) = st.slot(k) {
                        st.stack[i].0 = val;
                    }
                    return;
                }
                self.use_value(nid, &val);
                self.nodes[nid].stmts.push(NStmt::Store(val));
            }
            _ => {}
        }
    }

    fn rt_scope(&self, g: u32) -> bool {
        self.rt.scope.contains(&g)
    }

    /// The value is consumed (argument, condition, store): calls inside it are expressions, not
    /// statements of this node.
    fn use_value(&mut self, nid: usize, v: &V) {
        let mut ids = HashSet::new();
        walk_calls(v, &mut |c| {
            ids.insert(c.id);
        });
        if !ids.is_empty() {
            self.nodes[nid].stmts.retain(|s| !matches!(s, NStmt::Call(c) if ids.contains(&c.id)));
        }
    }

    fn use_cond(&mut self, nid: usize, c: &C) {
        let mut ids = HashSet::new();
        walk_cond_calls(c, &mut |x| {
            ids.insert(x.id);
        });
        if !ids.is_empty() {
            self.nodes[nid].stmts.retain(|s| !matches!(s, NStmt::Call(x) if ids.contains(&x.id)));
        }
    }

    fn new_node(&mut self, addr: u32) -> Res<usize> {
        self.nodes.push(Node { addr, stmts: Vec::new(), term: None, preds: Vec::new() });
        if self.nodes.len() > MAX_NODES {
            return fail("node explosion");
        }
        Ok(self.nodes.len() - 1)
    }

    fn state_key(&self, a: u32, st: &State) -> Key {
        let live = self.live_in.get(&a).copied().unwrap_or(0);
        let regs = GPRS.iter().filter(|r| live & reg_bit(**r) != 0).map(|r| st.reg(*r)).collect();
        let stack = st.stack.iter().map(|(v, used)| (!used).then(|| v.clone())).collect();
        Key { addr: a, regs, stack, in_loop: st.in_loop }
    }

    /// Runs the symbolic execution from the entry point.
    pub(crate) fn run(&mut self) -> Res<()> {
        self.root = self.explore(self.entry, State::new())?;
        // A call whose result is consumed in another node (pushed before a jump into shared
        // code, or kept in a register) is an expression there, not a statement here.
        let mut used = HashSet::new();
        for n in &self.nodes {
            let mut add = |c: &Rc<CallV>| {
                used.insert(c.id);
            };
            for s in &n.stmts {
                match s {
                    NStmt::Call(c) => c.args.iter().for_each(|a| walk_calls(a, &mut add)),
                    NStmt::Store(v) => walk_calls(v, &mut add),
                    _ => {}
                }
            }
            match &n.term {
                Some(Term::If(c, _, _)) => walk_cond_calls(c, &mut add),
                Some(Term::Switch(v, _)) => walk_calls(v, &mut add),
                _ => {}
            }
        }
        for n in &mut self.nodes {
            n.stmts.retain(|s| !matches!(s, NStmt::Call(c) if used.contains(&c.id)));
        }
        Ok(())
    }

    fn explore(&mut self, a: u32, st: State) -> Res<usize> {
        let key = self.state_key(a, &st);
        if let Some(&n) = self.memo.get(&key) {
            return Ok(n);
        }
        let nid = self.new_node(a)?;
        self.memo.insert(key, nid);
        self.step(nid, a, st)?;
        Ok(nid)
    }

    fn flags_cond(&self, st: &State, cc: Cc) -> C {
        let Some(f) = &st.flags else { return C::Unknown(unk("flags")) };
        let unknown = |kind: &str| C::Unknown(unk(&format!("flags:{kind}:j{}", cc_name(cc))));
        match f {
            Flags::Test(a) => match cc {
                Cc::A => rel(CmpOp::Ne, a.clone(), V::Const(0)),
                Cc::Be => rel(CmpOp::Eq, a.clone(), V::Const(0)),
                _ => match cc_rel_test(cc) {
                    Some(op) => rel(op, a.clone(), V::Const(0)),
                    None => unknown("test"),
                },
            },
            Flags::Cmp(a, b) => {
                let op = match cc {
                    Cc::E => CmpOp::Eq,
                    Cc::Ne => CmpOp::Ne,
                    Cc::L => CmpOp::Lt,
                    Cc::Ge => CmpOp::Ge,
                    Cc::Le => CmpOp::Le,
                    Cc::G => CmpOp::Gt,
                    Cc::B => CmpOp::ULt,
                    Cc::Ae => CmpOp::UGe,
                    Cc::A => CmpOp::UGt,
                    Cc::Be => CmpOp::ULe,
                    Cc::S => return rel(CmpOp::Lt, mk_bin(B::Sub, a.clone(), b.clone()), V::Const(0)),
                    Cc::Ns => return rel(CmpOp::Ge, mk_bin(B::Sub, a.clone(), b.clone()), V::Const(0)),
                    _ => return unknown("cmp"),
                };
                rel(op, a.clone(), b.clone())
            }
            Flags::Res(r) => match cc_rel_test(cc) {
                Some(op) => rel(op, r.clone(), V::Const(0)),
                None => unknown("res"),
            },
        }
    }

    fn do_call(&mut self, nid: usize, st: &mut State, target: u32, at: u32) -> Res<()> {
        let mut role = match self.cache.get(&target) {
            Some(t) => *t,
            None => {
                let t = self.rt.classify(self.pe, target);
                self.cache.insert(target, t);
                t
            }
        };
        if role == Target::Unknown && st.in_loop.is_some() && st.stack.len() >= 2 {
            let n = st.stack.len();
            if st.stack[n - 1].0 == V::Team && st.stack[n - 2].0 == V::MsgIndex {
                role = Target::MsgGet;
            }
        }
        let Some(mut nargs) = role.arity() else {
            return fail(format!("call to an unrecognised function {target:08x} at {at:08x}"));
        };
        nargs = nargs.min(st.stack.len());
        let mut args = Vec::with_capacity(nargs);
        for j in 0..nargs {
            let i = st.stack.len() - 1 - j;
            st.stack[i].1 = true;
            let v = st.stack[i].0.clone();
            self.use_value(nid, &v);
            args.push(v);
        }
        self.next_id += 1;
        let c = Rc::new(CallV { addr: target, at, args, target: role, id: self.next_id });
        self.nodes[nid].stmts.push(NStmt::Call(c.clone()));
        st.regs[Reg::Eax.index()] = V::Call(c);
        st.regs[Reg::Ecx.index()] = unk("clob");
        st.regs[Reg::Edx.index()] = unk("clob");
        Ok(())
    }

    fn step(&mut self, nid: usize, mut a: u32, mut st: State) -> Res<()> {
        let mut first = true;
        loop {
            if !first && self.leaders.contains(&a) {
                let t = self.explore(a, st)?;
                self.nodes[nid].term = Some(Term::Goto(t));
                return Ok(());
            }
            first = false;
            let i = self.dec(a)?;
            let (o0, o1, o2) = (i.op(0), i.op(1), i.op(2));
            let nxt = i.next();
            match i.op {
                Op::Push => {
                    let v = self.operand(&st, o0);
                    st.stack.push((v, false));
                }
                Op::Pop => {
                    let v = st.stack.pop().map(|e| e.0).unwrap_or_else(|| unk("pop"));
                    self.write(&mut st, nid, o0, v);
                }
                Op::Mov => {
                    // message loop header (or `continue`): a read of the team's queue pointer
                    if let Some(Operand::Mem(m)) = o1 {
                        if m.disp == 0x50 && m.base.is_some_and(|b| st.reg(b) == V::Team) {
                            if st.in_loop.is_some() {
                                self.nodes[nid].term = Some(Term::Continue);
                                return Ok(());
                            }
                            if self.msgloop(nid, a, &mut st)? {
                                return Ok(());
                            }
                        }
                    }
                    let v = self.operand(&st, o1);
                    self.write(&mut st, nid, o0, v);
                }
                Op::Movzx | Op::Movsx => {
                    let v = self.operand(&st, o1);
                    self.write(&mut st, nid, o0, v);
                }
                Op::Lea => {
                    let m = *o1.and_then(|o| o.mem().copied()).as_ref().ok_or_else(|| DecompileError("bad lea".into()))?;
                    let mut v = k(i64::from(m.disp));
                    if let Some(b) = m.base {
                        v = if b != Reg::Esp { mk_bin(B::Add, st.reg(b), v) } else { unk("lea_esp") };
                    }
                    if let Some(ix) = m.index {
                        v = mk_bin(B::Add, v, mk_bin(B::Mul, st.reg(ix), V::Const(u32::from(m.scale))));
                    }
                    let nop = m.base.is_some() && m.index.is_none() && m.disp == 0 && m.base == o0.and_then(|o| o.reg());
                    if !nop {
                        self.write(&mut st, nid, o0, v);
                    }
                }
                Op::Add | Op::Sub | Op::And | Op::Or | Op::Xor | Op::Shl | Op::Sar | Op::Shr | Op::Adc | Op::Sbb
                | Op::Imul
                    if i.nops() == 2 =>
                {
                    if o0.and_then(|o| o.reg()) == Some(Reg::Esp) && matches!(o0, Some(Operand::Reg { size: 4, .. })) {
                        let n = match o1 {
                            Some(Operand::Imm(v)) => v / 4,
                            _ => 0,
                        };
                        if i.op == Op::Add {
                            for _ in 0..n {
                                if let Some((v, used)) = st.stack.pop() {
                                    if !used && !matches!(v, V::Unk(_)) {
                                        self.nodes[nid].stmts.push(NStmt::Lost);
                                    }
                                }
                            }
                        } else if i.op == Op::Sub {
                            for _ in 0..n {
                                st.stack.push((unk("local"), true));
                            }
                        }
                    } else {
                        let x = self.operand(&st, o0);
                        let y = self.operand(&st, o1);
                        let same = same_reg(&i);
                        let r = match i.op {
                            Op::Xor | Op::Sub if same => V::Const(0),
                            Op::Sbb if same => match &st.carry {
                                Some(c) => V::Un(U::Neg, Rc::new(V::Cond(Rc::new(c.clone())))),
                                None => unk("sbb"),
                            },
                            Op::Or if y == V::Const(0xffff_ffff) => V::Const(0xffff_ffff),
                            Op::And if y == V::Const(0) => V::Const(0),
                            op => {
                                let b = match op {
                                    Op::Add | Op::Adc => B::Add,
                                    Op::Sub | Op::Sbb => B::Sub,
                                    Op::Imul => B::Mul,
                                    Op::And => B::And,
                                    Op::Or => B::Or,
                                    Op::Xor => B::Xor,
                                    Op::Shl => B::Shl,
                                    _ => B::Shr,
                                };
                                mk_bin(b, x.clone(), y.clone())
                            }
                        };
                        self.write(&mut st, nid, o0, r.clone());
                        st.flags = Some(if i.op == Op::Sub { Flags::Cmp(x, y) } else { Flags::Res(r) });
                    }
                }
                Op::Imul | Op::Mul if i.nops() == 1 => {
                    let x = st.reg(Reg::Eax);
                    let y = self.operand(&st, o0);
                    let hi = if matches!(x, V::Const(_)) { bin(B::MulHi, y.clone(), x.clone()) } else { bin(B::MulHi, x.clone(), y.clone()) };
                    st.regs[Reg::Edx.index()] = hi;
                    st.regs[Reg::Eax.index()] = mk_bin(B::Mul, x, y);
                }
                Op::Imul if i.nops() == 3 => {
                    let v = mk_bin(B::Mul, self.operand(&st, o1), self.operand(&st, o2));
                    self.write(&mut st, nid, o0, v);
                }
                Op::Inc | Op::Dec | Op::Neg | Op::Not => {
                    let x = self.operand(&st, o0);
                    let r = match i.op {
                        Op::Inc => mk_bin(B::Add, x, V::Const(1)),
                        Op::Dec => mk_bin(B::Sub, x, V::Const(1)),
                        Op::Neg => {
                            st.carry = Some(match &x {
                                V::Cond(c) => (**c).clone(),
                                _ => C::Rel(CmpOp::Ne, x.clone(), V::Const(0)),
                            });
                            match &x {
                                V::Un(U::Neg, inner) => (**inner).clone(),
                                _ => V::Un(U::Neg, Rc::new(x)),
                            }
                        }
                        _ => V::Un(U::Not, Rc::new(x)),
                    };
                    self.write(&mut st, nid, o0, r.clone());
                    st.flags = Some(Flags::Res(r));
                }
                Op::Cdq => {
                    st.regs[Reg::Edx.index()] = V::Un(U::Sgn, Rc::new(st.reg(Reg::Eax)));
                }
                Op::Idiv | Op::Div => {
                    let d = self.operand(&st, o0);
                    let q = mk_bin(B::Div, st.reg(Reg::Eax), d.clone());
                    st.regs[Reg::Eax.index()] = q.clone();
                    st.regs[Reg::Edx.index()] = mk_bin(B::Rem, q, d);
                }
                Op::Test => {
                    let x = self.operand(&st, o0);
                    let y = self.operand(&st, o1);
                    st.flags = Some(if same_reg(&i) { Flags::Test(x) } else { Flags::Test(mk_bin(B::And, x, y)) });
                }
                Op::Cmp => {
                    st.flags = Some(Flags::Cmp(self.operand(&st, o0), self.operand(&st, o1)));
                }
                Op::Setcc(cc) => {
                    let c = self.flags_cond(&st, cc);
                    self.write(&mut st, nid, o0, V::Cond(Rc::new(c)));
                }
                Op::Call => {
                    let Some(t) = i.target() else { return fail(format!("indirect call at {a:08x}")) };
                    self.do_call(nid, &mut st, t, a)?;
                }
                Op::Ret => {
                    self.nodes[nid].term = Some(Term::Ret);
                    return Ok(());
                }
                Op::Jmp => {
                    if let Some(t) = i.target() {
                        if self.rt.is_tail_target(self.pe, t) {
                            self.do_call(nid, &mut st, t, a)?;
                            self.nodes[nid].term = Some(Term::Ret);
                            return Ok(());
                        }
                        let n = self.explore(t, st)?;
                        self.nodes[nid].term = Some(Term::Goto(n));
                        return Ok(());
                    }
                    let (ix, tab) = self.jumptable(&i).ok_or_else(|| DecompileError(format!("indirect jmp at {a:08x}")))?;
                    let mut idx = st.reg(ix);
                    let mut off: i64 = 0;
                    while let V::Bin(op @ (B::Add | B::Sub), x, y) = &idx {
                        let V::Const(c) = **y else { break };
                        off += if *op == B::Add { -signed(c) } else { signed(c) };
                        idx = (**x).clone();
                    }
                    let mut done: HashMap<u32, usize> = HashMap::new();
                    let mut cases = Vec::with_capacity(tab.len());
                    for (kk, t) in tab.iter().enumerate() {
                        let n = match done.get(t) {
                            Some(&n) => n,
                            None => {
                                let n = self.explore(*t, st.clone())?;
                                done.insert(*t, n);
                                n
                            }
                        };
                        cases.push(((kk as i64 + off) as u32, n));
                    }
                    self.nodes[nid].term = Some(Term::Switch(idx, cases));
                    return Ok(());
                }
                Op::Jcc(cc) => {
                    let c = self.flags_cond(&st, cc);
                    let t = i.target().unwrap_or(nxt);
                    self.use_cond(nid, &c);
                    let mut st_t = st.clone();
                    branch_facts(&mut st_t, &c, true);
                    branch_facts(&mut st, &c, false);
                    let tn = self.explore(t, st_t)?;
                    let fnn = self.explore(nxt, st)?;
                    self.nodes[nid].term = Some(Term::If(c, tn, fnn));
                    return Ok(());
                }
                Op::Nop => {}
                Op::Xchg => {
                    let x = self.operand(&st, o0);
                    let y = self.operand(&st, o1);
                    self.write(&mut st, nid, o0, y);
                    self.write(&mut st, nid, o1, x);
                }
                _ => return fail(format!("unhandled instruction at {a:08x}: {:?}", i.op)),
            }
            a = nxt;
        }
    }

    /// The kas2c message handler loop, recognised at the read of the team's queue pointer:
    /// `q = team->msgQueue; for (i = count; i >= 0; ...) { msg = get(team, i); switch (msg) ... }`.
    fn msgloop(&mut self, nid: usize, a: u32, st: &mut State) -> Res<bool> {
        let mut seq = Vec::new();
        let mut b = a;
        let mut found = false;
        for _ in 0..12 {
            let i = self.dec(b)?;
            seq.push(i);
            if matches!(i.op, Op::Jcc(Cc::L | Cc::S)) {
                found = true;
                break;
            }
            b = i.next();
        }
        if !found {
            return Ok(false);
        }
        let jl = *seq.last().unwrap();
        let mut loopreg = None;
        for i in &seq {
            if i.op == Op::Mov {
                if let (Some(Operand::Reg { reg, .. }), Some(Operand::Reg { .. })) = (i.op(0), i.op(1)) {
                    loopreg = Some(reg);
                }
            }
        }
        let Some(loopreg) = loopreg else { return Ok(false) };
        for i in &seq {
            if i.op == Op::Push {
                let v = self.operand(st, i.op(0));
                st.stack.push((v, true));
            }
        }
        let exit_st = st.clone();
        let mut body_st = st.clone();
        if loopreg != Reg::Esp {
            body_st.regs[loopreg.index()] = V::MsgIndex;
        }
        body_st.in_loop = Some(jl.next());
        let body = self.explore(jl.next(), body_st)?;
        let ex = self.explore(jl.target().unwrap_or(jl.next()), exit_st)?;
        self.nodes[nid].term = Some(Term::MsgLoop(body, ex));
        Ok(true)
    }
}

/// On the path where `x == K` holds, registers holding `x` are known to be `K` (MSVC reuses
/// them).
fn branch_facts(st: &mut State, c: &C, taken: bool) {
    let C::Rel(op, a, b) = c else { return };
    let op = if taken { *op } else { op.negate() };
    let V::Const(kv) = b else { return };
    if op != CmpOp::Eq || matches!(a, V::Cond(_)) {
        return;
    }
    for r in GPRS {
        if st.regs[r.index()] == *a {
            st.regs[r.index()] = V::Const(*kv);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// structuring
// ---------------------------------------------------------------------------------------------

/// A structured statement.
#[derive(Clone, Debug)]
pub(crate) enum A {
    Call(Rc<CallV>),
    Scope,
    Store,
    Lost,
    Return,
    If(C, Vec<A>, Vec<A>),
    /// Value, arms (values, body), default.
    Switch(V, Vec<(Vec<u32>, Vec<A>)>, Option<Vec<A>>),
    MsgLoop(Vec<A>),
}

const EXIT: isize = -1;

pub(crate) struct Structurer<'n> {
    n: &'n mut [Node],
    root: usize,
    live: Vec<bool>,
    ipdom: HashMap<usize, isize>,
    budget: isize,
}

impl<'n> Structurer<'n> {
    pub(crate) fn new(nodes: &'n mut [Node], root: usize) -> Res<Self> {
        let budget = 40 * nodes.len() as isize + 2000;
        let live = vec![false; nodes.len()];
        let mut s = Structurer { n: nodes, root, live, ipdom: HashMap::new(), budget };
        s.collapse();
        s.postdom()?;
        Ok(s)
    }

    fn recompute_preds(&mut self) {
        self.live.iter_mut().for_each(|x| *x = false);
        let mut stack = vec![self.root];
        while let Some(u) = stack.pop() {
            if self.live[u] {
                continue;
            }
            self.live[u] = true;
            stack.extend(self.n[u].succs());
        }
        for n in self.n.iter_mut() {
            n.preds.clear();
        }
        for u in 0..self.n.len() {
            if !self.live[u] {
                continue;
            }
            for s in self.n[u].succs() {
                self.n[s].preds.push(u);
            }
        }
    }

    fn is_pure_cond(&self, id: usize) -> bool {
        matches!(self.n[id].term, Some(Term::If(..))) && self.n[id].pure_stmts()
    }

    fn swvar(c: &C) -> Option<&V> {
        match c {
            C::Rel(_, a @ V::Call(_), V::Const(_)) => Some(a),
            _ => None,
        }
    }

    /// Merges short-circuit chains: `A ? (B ? x : y) : y` -> `(A AND B) ? x : y`, etc.
    fn collapse(&mut self) {
        self.recompute_preds();
        'outer: loop {
            for a in 0..self.n.len() {
                if !self.live[a] {
                    continue;
                }
                let Some(Term::If(c, t, f)) = self.n[a].term.clone() else { continue };
                for b in [t, f] {
                    if b == a || !self.is_pure_cond(b) || self.n[b].preds != [a] {
                        continue;
                    }
                    let Some(Term::If(cb, tb, fb)) = self.n[b].term.clone() else { continue };
                    if let (Some(k1), Some(k2)) = (Self::swvar(&c), Self::swvar(&cb)) {
                        if k1 == k2 {
                            continue;
                        }
                    }
                    let new = if b == t {
                        if fb == f {
                            Some(Term::If(cand(c.clone(), cb.clone()), tb, f))
                        } else if tb == f {
                            Some(Term::If(cand(c.clone(), cnot(&cb)), fb, f))
                        } else {
                            None
                        }
                    } else if tb == t {
                        Some(Term::If(cor(c.clone(), cb.clone()), t, fb))
                    } else if fb == t {
                        Some(Term::If(cor(c.clone(), cnot(&cb)), t, tb))
                    } else {
                        None
                    };
                    if let Some(new) = new {
                        self.n[a].term = Some(new);
                        self.recompute_preds();
                        continue 'outer;
                    }
                }
            }
            break;
        }
    }

    /// Immediate post-dominators (Cooper-Harvey-Kennedy over a post-order of all nodes).
    fn postdom(&mut self) -> Res<()> {
        let succ: Vec<Vec<usize>> = self.n.iter().map(Node::succs).collect();
        let mut order = Vec::with_capacity(self.n.len());
        let mut seen = vec![false; self.n.len()];
        for start in 0..self.n.len() {
            if seen[start] {
                continue;
            }
            seen[start] = true;
            let mut stack: Vec<(usize, usize)> = vec![(start, 0)];
            while let Some(top) = stack.last_mut() {
                let (v, i) = *top;
                if i < succ[v].len() {
                    top.1 += 1;
                    let w = succ[v][i];
                    if !seen[w] {
                        seen[w] = true;
                        stack.push((w, 0));
                    }
                } else {
                    order.push(v);
                    stack.pop();
                }
            }
        }
        let mut rank = vec![0isize; self.n.len()];
        for (r, &v) in order.iter().enumerate() {
            rank[v] = r as isize;
        }
        let rk = |x: isize| if x == EXIT { -1 } else { rank[x as usize] };
        let mut ipdom: HashMap<usize, isize> = HashMap::new();
        for &v in &order {
            let ss = &succ[v];
            if ss.is_empty() {
                ipdom.insert(v, EXIT);
                continue;
            }
            let mut cur: Option<isize> = None;
            for &s in ss {
                if !ipdom.contains_key(&s) {
                    continue;
                }
                cur = Some(match cur {
                    None => s as isize,
                    Some(c) => {
                        let (mut a, mut b) = (c, s as isize);
                        let mut guard = 0;
                        while a != b {
                            while rk(a) > rk(b) {
                                a = *ipdom.get(&(a as usize)).ok_or_else(|| DecompileError("post-dominator walk".into()))?;
                            }
                            while rk(b) > rk(a) {
                                b = *ipdom.get(&(b as usize)).ok_or_else(|| DecompileError("post-dominator walk".into()))?;
                            }
                            guard += 1;
                            if guard > 1_000_000 {
                                return fail("post-dominator loop");
                            }
                        }
                        a
                    }
                });
            }
            ipdom.insert(v, cur.unwrap_or(EXIT));
        }
        self.ipdom = ipdom;
        Ok(())
    }

    fn formula(&self, u: usize, x: usize, y: usize) -> C {
        // only the head node is in the condition region, so this is a single test
        let _ = u;
        let Some(Term::If(c, t, f)) = &self.n[u].term else { return C::True };
        let leaf = |w: usize| {
            if w == x {
                Some(C::True)
            } else if w == y {
                Some(C::False)
            } else {
                None
            }
        };
        let ft = leaf(*t).unwrap_or(C::True);
        let ff = leaf(*f).unwrap_or(C::True);
        match (&ft, &ff) {
            (C::True, C::False) => c.clone(),
            (C::False, C::True) => cnot(c),
            (_, C::False) => cand(c.clone(), ft),
            (C::True, _) => cor(c.clone(), ff),
            (C::False, _) => cand(cnot(c), ff),
            (_, C::True) => cor(cnot(c), ft),
            _ => cor(cand(c.clone(), ft), cand(cnot(c), ff)),
        }
    }

    fn ipdom_of(&self, n: usize) -> isize {
        self.ipdom.get(&n).copied().unwrap_or(EXIT)
    }

    /// Emits the structured statements from node `nid` up to (not including) `stop`.
    pub(crate) fn emit(&mut self, start: isize, stop: Option<isize>) -> Res<Vec<A>> {
        let mut out = Vec::new();
        let mut nid = start;
        while nid != EXIT && Some(nid) != stop {
            self.budget -= 1;
            if self.budget < 0 {
                return fail("structuring blow-up (code duplication)");
            }
            let u = nid as usize;
            for s in &self.n[u].stmts {
                out.push(match s {
                    NStmt::Call(c) => A::Call(c.clone()),
                    NStmt::Scope => A::Scope,
                    NStmt::Store(_) => A::Store,
                    NStmt::Lost => A::Lost,
                });
            }
            let term = self.n[u].term.clone().ok_or_else(|| DecompileError("node without terminator".into()))?;
            match term {
                Term::Goto(t) => nid = t as isize,
                Term::Ret => {
                    out.push(A::Return);
                    break;
                }
                Term::Continue => break,
                Term::MsgLoop(body, exit) => {
                    let b = self.emit(body as isize, None)?;
                    out.push(A::MsgLoop(b));
                    nid = exit as isize;
                }
                Term::Switch(v, cases) => {
                    let join = self.ipdom_of(u);
                    let mut groups: Vec<(usize, Vec<u32>)> = Vec::new();
                    for (val, t) in cases {
                        match groups.iter_mut().find(|g| g.0 == t) {
                            Some(g) => g.1.push(val),
                            None => groups.push((t, vec![val])),
                        }
                    }
                    let mut arms = Vec::new();
                    for (t, vals) in groups {
                        arms.push((vals, self.emit(t as isize, Some(join))?));
                    }
                    out.push(A::Switch(v, arms, None));
                    nid = join;
                }
                Term::If(_, t, f) => {
                    if let Some((var, arms, default, join)) = self.try_switch(u) {
                        let mut ea = Vec::new();
                        for (tgt, vals) in arms {
                            ea.push((vals, self.emit(tgt as isize, Some(join))?));
                        }
                        let d = match default {
                            Some(d) if d as isize != join => Some(self.emit(d as isize, Some(join))?),
                            _ => None,
                        };
                        out.push(A::Switch(var, ea, d));
                        nid = join;
                        continue;
                    }
                    let join = self.ipdom_of(u);
                    let (mut x, mut y) = (t, f);
                    // THEN is the branch that is not the join point, else the lower address
                    if y as isize != join && (x as isize == join || self.n[x].addr > self.n[y].addr) {
                        std::mem::swap(&mut x, &mut y);
                    }
                    let mut c = self.formula(u, x, y);
                    let mut then = self.emit(x as isize, Some(join))?;
                    let mut els = if y as isize != join { self.emit(y as isize, Some(join))? } else { Vec::new() };
                    if then.is_empty() && !els.is_empty() {
                        c = cnot(&c);
                        then = std::mem::take(&mut els);
                    }
                    out.push(A::If(c, then, els));
                    nid = join;
                }
            }
        }
        Ok(out)
    }

    fn skip_trivial(&self, mut id: usize) -> usize {
        let mut seen = HashSet::new();
        while seen.insert(id) {
            match &self.n[id].term {
                Some(Term::Goto(t)) if self.n[id].pure_stmts() => id = *t,
                _ => break,
            }
        }
        id
    }

    /// Switch recovery over a compare tree on one call value.
    #[allow(clippy::type_complexity)]
    fn try_switch(&self, head: usize) -> Option<(V, Vec<(usize, Vec<u32>)>, Option<usize>, isize)> {
        let Some(Term::If(c, ..)) = &self.n[head].term else { return None };
        let var = Self::swvar(c)?.clone();
        let is_msg = matches!(&var, V::Call(cv) if cv.target == Target::MsgGet);
        let mut leaves = Vec::new();
        self.swwalk(head, &var, vec![(0, M)], &mut leaves, head, 0);
        if leaves.len() < if is_msg { 2 } else { 3 } {
            return None;
        }
        let mut per: Vec<(usize, Vec<Option<Vec<u32>>>)> = Vec::new();
        for (tgt, iv) in leaves {
            if iv.is_empty() {
                continue;
            }
            let tgt = self.skip_trivial(tgt);
            let vals = iv_values(&iv);
            match per.iter_mut().find(|p| p.0 == tgt) {
                Some(p) => p.1.push(vals),
                None => per.push((tgt, vec![vals])),
            }
        }
        let mut arms: Vec<(usize, Vec<u32>)> = Vec::new();
        let mut default = None;
        for (tgt, lst) in per {
            if lst.iter().all(Option::is_some) && lst.iter().map(|v| v.as_ref().unwrap().len()).sum::<usize>() <= 8 {
                arms.push((tgt, lst.into_iter().flatten().flatten().collect()));
            } else {
                if default.is_some_and(|d| d != tgt) {
                    return None;
                }
                default = Some(tgt);
            }
        }
        if is_msg {
            // message 0 is an empty queue slot: kas2c just continues the loop
            for a in &mut arms {
                a.1.retain(|&v| v != 0);
            }
            arms.retain(|a| !a.1.is_empty());
        }
        if arms.len() < if is_msg { 1 } else { 2 } {
            return None;
        }
        Some((var, arms, default, self.ipdom_of(head)))
    }

    fn swwalk(&self, id: usize, var: &V, iv: Vec<(i64, i64)>, leaves: &mut Vec<(usize, Vec<(i64, i64)>)>, head: usize, depth: usize) {
        let n = &self.n[id];
        let pure = self.is_pure_cond(id) || (matches!(n.term, Some(Term::Switch(..))) && n.pure_stmts());
        if depth > 10_000 || (id != head && (!pure || n.preds.len() != 1)) {
            leaves.push((id, iv));
            return;
        }
        match &n.term {
            Some(Term::If(c, t, f)) => {
                let Some(v) = Self::swvar(c) else {
                    leaves.push((id, iv));
                    return;
                };
                if v != var {
                    leaves.push((id, iv));
                    return;
                }
                let C::Rel(op, _, V::Const(kv)) = c else { unreachable!() };
                let (tr, fl) = split(&iv, *op, *kv);
                let (t, f) = (*t, *f);
                self.swwalk(t, var, tr, leaves, head, depth + 1);
                self.swwalk(f, var, fl, leaves, head, depth + 1);
            }
            Some(Term::Switch(v, cases)) => {
                if !matches!(v, V::Call(_)) || v != var {
                    leaves.push((id, iv));
                    return;
                }
                for &(val, tgt) in cases {
                    let val = i64::from(val);
                    if iv.iter().any(|&(lo, hi)| lo <= val && val <= hi) {
                        leaves.push((tgt, vec![(val, val)]));
                    }
                }
            }
            _ => leaves.push((id, iv)),
        }
    }
}

const M: i64 = 0xffff_ffff;

/// Sets of 32-bit values as sorted inclusive intervals.
type Intervals = Vec<(i64, i64)>;

/// Splits a set of unsigned intervals by `x op k` into (true, false) parts.
fn split(iv: &[(i64, i64)], op: CmpOp, k: u32) -> (Intervals, Intervals) {
    let inter = |iv: &[(i64, i64)], lo: i64, hi: i64| -> Vec<(i64, i64)> {
        iv.iter()
            .filter_map(|&(a, b)| {
                let (a2, b2) = (a.max(lo), b.min(hi));
                (a2 <= b2).then_some((a2, b2))
            })
            .collect()
    };
    let ku = i64::from(k);
    let sk = signed(k);
    let mut yes: Vec<(i64, i64)> = match op {
        CmpOp::Eq => vec![(ku, ku)],
        CmpOp::Ne => vec![(0, ku - 1), (ku + 1, M)],
        CmpOp::ULt => vec![(0, ku - 1)],
        CmpOp::ULe => vec![(0, ku)],
        CmpOp::UGt => vec![(ku + 1, M)],
        CmpOp::UGe => vec![(ku, M)],
        CmpOp::Lt | CmpOp::Le | CmpOp::Gt | CmpOp::Ge => {
            let (lo, hi) = match op {
                CmpOp::Lt => (-(1i64 << 31), sk - 1),
                CmpOp::Le => (-(1i64 << 31), sk),
                CmpOp::Gt => (sk + 1, (1i64 << 31) - 1),
                _ => (sk, (1i64 << 31) - 1),
            };
            let mut rr = Vec::new();
            if lo < 0 {
                rr.push((lo & M, M.min(hi.min(-1) & M)));
            }
            if hi >= 0 {
                rr.push((lo.max(0), hi));
            }
            rr
        }
    };
    yes.sort();
    let mut no = Vec::new();
    let mut cur = 0i64;
    for &(lo, hi) in &yes {
        if lo > cur {
            no.push((cur, lo - 1));
        }
        cur = cur.max(hi + 1);
    }
    if cur <= M {
        no.push((cur, M));
    }
    let t = yes.iter().flat_map(|&(lo, hi)| inter(iv, lo, hi)).collect();
    let f = no.iter().flat_map(|&(lo, hi)| inter(iv, lo, hi)).collect();
    (t, f)
}

fn iv_values(iv: &[(i64, i64)]) -> Option<Vec<u32>> {
    let tot: i64 = iv.iter().map(|(a, b)| b - a + 1).sum();
    if tot > 4 {
        return None;
    }
    Some(iv.iter().flat_map(|&(a, b)| (a..=b).map(|v| v as u32)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds_division_by_constant() {
        // x / 3 via mulhi(x, 0x55555556); q + (q >> 31)
        let x = unk("x");
        let q = bin(B::MulHi, x.clone(), V::Const(0x5555_5556));
        let r = mk_bin(B::Add, q.clone(), mk_bin(B::Shr, q, V::Const(31)));
        assert_eq!(r, bin(B::Div, x, V::Const(3)));
    }

    #[test]
    fn normalises_relations() {
        let x = unk("x");
        let c = rel(CmpOp::Eq, mk_bin(B::Sub, x.clone(), V::Const(5)), V::Const(2));
        assert_eq!(c, C::Rel(CmpOp::Eq, x, V::Const(7)));
    }

    #[test]
    fn splits_intervals() {
        let (t, f) = split(&[(0, M)], CmpOp::Eq, 5);
        assert_eq!(t, vec![(5, 5)]);
        assert_eq!(f, vec![(0, 4), (6, M)]);
        let (t, _) = split(&[(0, M)], CmpOp::Lt, 0);
        assert_eq!(t, vec![(0x8000_0000, M)]);
    }
}
