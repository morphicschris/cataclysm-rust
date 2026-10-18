//! Compact binary form of a mission IR (what an install-time extractor would cache).
//!
//! Layout: magic `HKAS`, a little-endian `u16` version, then the mission as a tagged tree of
//! LEB128 varints (zigzag for signed values) and length-prefixed UTF-8 strings. The encoding is
//! lossless: `decode(&encode(m)) == m`.

use std::collections::BTreeMap;

use crate::functions::{FuncId, ParamType, FUNCTIONS};
use crate::ir::*;
use crate::Error;

const MAGIC: &[u8; 4] = b"HKAS";
const VERSION: u16 = 1;

/// Encodes a mission.
pub fn encode(m: &Mission) -> Vec<u8> {
    let mut w = W(Vec::with_capacity(64 * 1024));
    w.0.extend_from_slice(MAGIC);
    w.0.extend_from_slice(&VERSION.to_le_bytes());
    w.str(&m.name);
    w.u(m.name_hash.into());
    w.u(match m.fidelity {
        Fidelity::Exact => 0,
        Fidelity::Text => 1,
    });
    w.u(m.lstrings.len() as u64);
    for l in &m.lstrings {
        w.opt_str(l.name.as_deref());
        w.opt_u(l.addr.map(u64::from));
        for t in &l.texts {
            w.str(t);
        }
    }
    w.u(m.names.len() as u64);
    for (h, n) in &m.names {
        w.u((*h).into());
        w.str(n);
    }
    w.u(m.fsms.len() as u64);
    for f in &m.fsms {
        w.u(f.hash.into());
        w.block(&f.init);
        w.block(&f.watch);
        w.u(f.states.len() as u64);
        for s in &f.states {
            w.u(s.hash.into());
            w.block(&s.init);
            w.block(&s.watch);
        }
    }
    w.block(&m.init);
    w.block(&m.watch);
    w.0
}

/// Decodes a mission.
pub fn decode(data: &[u8]) -> Result<Mission, Error> {
    if data.len() < 6 || &data[..4] != MAGIC {
        return Err(Error::new("not a hwc-kas binary mission"));
    }
    let version = u16::from_le_bytes([data[4], data[5]]);
    if version != VERSION {
        return Err(Error::new(format!("unsupported hwc-kas binary version {version}")));
    }
    let mut r = R { d: data, i: 6 };
    let name = r.str()?;
    let name_hash = r.u32()?;
    let fidelity = match r.u()? {
        0 => Fidelity::Exact,
        1 => Fidelity::Text,
        v => return Err(r.err(&format!("bad fidelity {v}"))),
    };
    let n = r.len()?;
    let mut lstrings = Vec::with_capacity(n);
    for _ in 0..n {
        let name = r.opt_str()?;
        let addr = r.opt_u()?.map(|v| v as u32);
        lstrings.push(LString { name, addr, texts: [r.str()?, r.str()?, r.str()?] });
    }
    let n = r.len()?;
    let mut names = BTreeMap::new();
    for _ in 0..n {
        let h = r.u32()?;
        names.insert(h, r.str()?);
    }
    let n = r.len()?;
    let mut fsms = Vec::with_capacity(n);
    for _ in 0..n {
        let hash = r.u32()?;
        let init = r.block()?;
        let watch = r.block()?;
        let ns = r.len()?;
        let mut states = Vec::with_capacity(ns);
        for _ in 0..ns {
            states.push(State { hash: r.u32()?, init: r.block()?, watch: r.block()? });
        }
        fsms.push(Fsm { hash, init, watch, states });
    }
    let init = r.block()?;
    let watch = r.block()?;
    if r.i != data.len() {
        return Err(r.err("trailing bytes"));
    }
    let m = Mission { name, name_hash, lstrings, fsms, init, watch, names, fidelity };
    validate(&m)?;
    Ok(m)
}

/// Checks indices (FSMs, states, LSTRINGs) so the interpreter can index without failing.
pub fn validate(m: &Mission) -> Result<(), Error> {
    for (owner, b) in m.blocks() {
        let states = owner.fsm().map(|f| m.fsms[f].states.len());
        check(&b.stmts, m, states)?;
    }
    Ok(())
}

fn check(stmts: &[Stmt], m: &Mission, states: Option<usize>) -> Result<(), Error> {
    let mut err = None;
    let mut chk = |s: &Stmt| match s {
        Stmt::Jump { state, .. } if states.is_none_or(|n| *state >= n) => err = Some("JUMP target out of range"),
        Stmt::FsmCreate { fsm, .. } if *fsm >= m.fsms.len() => err = Some("FSMCreate target out of range"),
        _ => {}
    };
    walk(stmts, &mut chk);
    if let Some(e) = err {
        return Err(Error::new(e));
    }
    let mut bad = false;
    visit_calls(stmts, &mut |c| {
        if c.args.iter().any(|a| matches!(a, Arg::Str(StrRef::Local(i)) if *i >= m.lstrings.len())) {
            bad = true;
        }
    });
    if bad {
        return Err(Error::new("LSTRING index out of range"));
    }
    Ok(())
}

fn walk(stmts: &[Stmt], f: &mut dyn FnMut(&Stmt)) {
    for s in stmts {
        f(s);
        match s {
            Stmt::If { then, otherwise, .. } => {
                walk(then, f);
                walk(otherwise, f);
            }
            Stmt::IfOnce { body, otherwise, .. } => {
                walk(body, f);
                walk(otherwise, f);
            }
            Stmt::Switch { arms, default, .. } => {
                for a in arms {
                    walk(&a.body, f);
                }
                if let Some(d) = default {
                    walk(d, f);
                }
            }
            Stmt::MsgLoop { handlers, default } => {
                for h in handlers {
                    walk(&h.body, f);
                }
                if let Some(d) = default {
                    walk(d, f);
                }
            }
            _ => {}
        }
    }
}

struct W(Vec<u8>);

impl W {
    fn u(&mut self, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                self.0.push(b);
                return;
            }
            self.0.push(b | 0x80);
        }
    }
    fn s(&mut self, v: i32) {
        self.u(u64::from(((v << 1) ^ (v >> 31)) as u32));
    }
    fn str(&mut self, s: &str) {
        self.u(s.len() as u64);
        self.0.extend_from_slice(s.as_bytes());
    }
    fn opt_str(&mut self, s: Option<&str>) {
        match s {
            None => self.u(0),
            Some(s) => {
                self.u(1);
                self.str(s);
            }
        }
    }
    fn opt_u(&mut self, v: Option<u64>) {
        match v {
            None => self.u(0),
            Some(v) => {
                self.u(1);
                self.u(v);
            }
        }
    }
    fn block(&mut self, b: &Block) {
        self.opt_u(b.addr.map(u64::from));
        self.stmts(&b.stmts);
    }
    fn stmts(&mut self, s: &[Stmt]) {
        self.u(s.len() as u64);
        for x in s {
            self.stmt(x);
        }
    }
    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Call(c) => {
                self.u(0);
                self.call(c);
            }
            Stmt::If { cond, then, otherwise } => {
                self.u(1);
                self.cond(cond);
                self.stmts(then);
                self.stmts(otherwise);
            }
            Stmt::IfOnce { var, cond, body, otherwise } => {
                self.u(2);
                self.u((*var).into());
                match cond {
                    None => self.u(0),
                    Some(c) => {
                        self.u(1);
                        self.cond(c);
                    }
                }
                self.stmts(body);
                self.stmts(otherwise);
            }
            Stmt::Switch { value, arms, default } => {
                self.u(3);
                self.expr(value);
                self.u(arms.len() as u64);
                for a in arms {
                    self.u(a.values.len() as u64);
                    for v in &a.values {
                        self.s(*v);
                    }
                    self.stmts(&a.body);
                }
                self.opt_stmts(default.as_deref());
            }
            Stmt::MsgLoop { handlers, default } => {
                self.u(4);
                self.u(handlers.len() as u64);
                for h in handlers {
                    self.u(h.msgs.len() as u64);
                    for m in &h.msgs {
                        self.u((*m).into());
                    }
                    self.stmts(&h.body);
                }
                self.opt_stmts(default.as_deref());
            }
            Stmt::Jump { state, hash } => {
                self.u(5);
                self.u(*state as u64);
                self.u((*hash).into());
            }
            Stmt::FsmCreate { fsm, team } => {
                self.u(6);
                self.u(*fsm as u64);
                self.team(*team);
            }
            Stmt::Return => self.u(7),
        }
    }
    fn opt_stmts(&mut self, s: Option<&[Stmt]>) {
        match s {
            None => self.u(0),
            Some(s) => {
                self.u(1);
                self.stmts(s);
            }
        }
    }
    fn team(&mut self, t: TeamRef) {
        match t {
            TeamRef::Null => self.u(0),
            TeamRef::This => self.u(1),
            TeamRef::Label(h) => {
                self.u(2);
                self.u(h.into());
            }
        }
    }
    fn call(&mut self, c: &Call) {
        self.u(c.func.0.into());
        // the parameter types come from the table; only the payloads are stored
        for a in &c.args {
            match a {
                Arg::Int(e) => self.expr(e),
                Arg::Hash(h) => self.u((*h).into()),
                Arg::Str(s) => match s {
                    StrRef::Null => self.u(0),
                    StrRef::Int(v) => {
                        self.u(1);
                        self.s(*v);
                    }
                    StrRef::Text(t) => {
                        self.u(2);
                        self.str(t);
                    }
                    StrRef::Local(i) => {
                        self.u(3);
                        self.u(*i as u64);
                    }
                },
                Arg::Ships(s) => match *s {
                    ShipsRef::Null => self.u(0),
                    ShipsRef::Selection(h) => {
                        self.u(1);
                        self.u(h.into());
                    }
                    ShipsRef::Team(t) => {
                        self.u(2);
                        self.team(t);
                    }
                },
                Arg::Point(p) => match *p {
                    PointRef::Null => self.u(0),
                    PointRef::Point(h) => {
                        self.u(1);
                        self.u(h.into());
                    }
                    PointRef::Ships(h) => {
                        self.u(2);
                        self.u(h.into());
                    }
                    PointRef::Team(t) => {
                        self.u(3);
                        self.team(t);
                    }
                    PointRef::Volume(h) => {
                        self.u(4);
                        self.u(h.into());
                    }
                },
                Arg::Team(t) => self.team(*t),
                Arg::Volume(v) | Arg::Path(v) => self.opt_u(v.map(u64::from)),
            }
        }
    }
    fn cond(&mut self, c: &Cond) {
        match c {
            Cond::True => self.u(0),
            Cond::False => self.u(1),
            Cond::Not(a) => {
                self.u(2);
                self.cond(a);
            }
            Cond::And(a, b) => {
                self.u(3);
                self.cond(a);
                self.cond(b);
            }
            Cond::Or(a, b) => {
                self.u(4);
                self.cond(a);
                self.cond(b);
            }
            Cond::Cmp(op, a, b) => {
                self.u(5 + *op as u64);
                self.expr(a);
                self.expr(b);
            }
        }
    }
    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(v) => {
                self.u(0);
                self.s(*v);
            }
            Expr::Call(c) => {
                self.u(1);
                self.call(c);
            }
            Expr::Bin(op, a, b) => {
                self.u(2 + *op as u64);
                self.expr(a);
                self.expr(b);
            }
            Expr::Neg(a) => {
                self.u(12);
                self.expr(a);
            }
            Expr::BitNot(a) => {
                self.u(13);
                self.expr(a);
            }
            Expr::Cond(c) => {
                self.u(14);
                self.cond(c);
            }
        }
    }
}

const CMP_OPS: [CmpOp; 10] =
    [CmpOp::Eq, CmpOp::Ne, CmpOp::Lt, CmpOp::Le, CmpOp::Gt, CmpOp::Ge, CmpOp::ULt, CmpOp::ULe, CmpOp::UGt, CmpOp::UGe];
const BIN_OPS: [BinOp; 10] = [
    BinOp::Add,
    BinOp::Sub,
    BinOp::Mul,
    BinOp::Div,
    BinOp::Rem,
    BinOp::And,
    BinOp::Or,
    BinOp::Xor,
    BinOp::Shl,
    BinOp::Shr,
];

struct R<'a> {
    d: &'a [u8],
    i: usize,
}

impl R<'_> {
    fn err(&self, m: &str) -> Error {
        Error::new(format!("binary mission: {m} at byte {}", self.i))
    }
    fn u(&mut self) -> Result<u64, Error> {
        let mut v = 0u64;
        let mut shift = 0;
        loop {
            let b = *self.d.get(self.i).ok_or_else(|| self.err("truncated"))?;
            self.i += 1;
            if shift >= 64 {
                return Err(self.err("varint too long"));
            }
            v |= u64::from(b & 0x7f) << shift;
            if b & 0x80 == 0 {
                return Ok(v);
            }
            shift += 7;
        }
    }
    fn u32(&mut self) -> Result<u32, Error> {
        u32::try_from(self.u()?).map_err(|_| self.err("value out of range"))
    }
    fn len(&mut self) -> Result<usize, Error> {
        let n = self.u()?;
        if n > (self.d.len() - self.i) as u64 * 8 + 16 {
            return Err(self.err("implausible length"));
        }
        Ok(n as usize)
    }
    fn s(&mut self) -> Result<i32, Error> {
        let v = self.u32()?;
        Ok(((v >> 1) as i32) ^ -((v & 1) as i32))
    }
    fn str(&mut self) -> Result<String, Error> {
        let n = self.len()?;
        let b = self.d.get(self.i..self.i + n).ok_or_else(|| self.err("truncated string"))?;
        self.i += n;
        String::from_utf8(b.to_vec()).map_err(|_| self.err("invalid UTF-8"))
    }
    fn opt_str(&mut self) -> Result<Option<String>, Error> {
        Ok(match self.u()? {
            0 => None,
            _ => Some(self.str()?),
        })
    }
    fn opt_u(&mut self) -> Result<Option<u64>, Error> {
        Ok(match self.u()? {
            0 => None,
            _ => Some(self.u()?),
        })
    }
    fn block(&mut self) -> Result<Block, Error> {
        let addr = self.opt_u()?.map(|v| v as u32);
        Ok(Block { addr, stmts: self.stmts(0)? })
    }
    fn stmts(&mut self, depth: usize) -> Result<Vec<Stmt>, Error> {
        if depth > 256 {
            return Err(self.err("nesting too deep"));
        }
        let n = self.len()?;
        let mut v = Vec::with_capacity(n.min(1024));
        for _ in 0..n {
            v.push(self.stmt(depth)?);
        }
        Ok(v)
    }
    fn opt_stmts(&mut self, depth: usize) -> Result<Option<Vec<Stmt>>, Error> {
        Ok(match self.u()? {
            0 => None,
            _ => Some(self.stmts(depth + 1)?),
        })
    }
    fn stmt(&mut self, depth: usize) -> Result<Stmt, Error> {
        Ok(match self.u()? {
            0 => Stmt::Call(self.call(depth)?),
            1 => Stmt::If { cond: self.cond(depth)?, then: self.stmts(depth + 1)?, otherwise: self.stmts(depth + 1)? },
            2 => {
                let var = self.u32()?;
                let cond = match self.u()? {
                    0 => None,
                    _ => Some(self.cond(depth)?),
                };
                Stmt::IfOnce { var, cond, body: self.stmts(depth + 1)?, otherwise: self.stmts(depth + 1)? }
            }
            3 => {
                let value = self.expr(depth)?;
                let n = self.len()?;
                let mut arms = Vec::with_capacity(n);
                for _ in 0..n {
                    let nv = self.len()?;
                    let values = (0..nv).map(|_| self.s()).collect::<Result<_, _>>()?;
                    arms.push(SwitchArm { values, body: self.stmts(depth + 1)? });
                }
                Stmt::Switch { value, arms, default: self.opt_stmts(depth)? }
            }
            4 => {
                let n = self.len()?;
                let mut handlers = Vec::with_capacity(n);
                for _ in 0..n {
                    let nm = self.len()?;
                    let msgs = (0..nm).map(|_| self.u32()).collect::<Result<_, _>>()?;
                    handlers.push(MsgHandler { msgs, body: self.stmts(depth + 1)? });
                }
                Stmt::MsgLoop { handlers, default: self.opt_stmts(depth)? }
            }
            5 => Stmt::Jump { state: self.u()? as usize, hash: self.u32()? },
            6 => Stmt::FsmCreate { fsm: self.u()? as usize, team: self.team()? },
            7 => Stmt::Return,
            t => return Err(self.err(&format!("bad statement tag {t}"))),
        })
    }
    fn team(&mut self) -> Result<TeamRef, Error> {
        Ok(match self.u()? {
            0 => TeamRef::Null,
            1 => TeamRef::This,
            2 => TeamRef::Label(self.u32()?),
            t => return Err(self.err(&format!("bad team tag {t}"))),
        })
    }
    fn call(&mut self, depth: usize) -> Result<Call, Error> {
        let f = self.u()?;
        let info = FUNCTIONS.get(f as usize).ok_or_else(|| self.err("bad function index"))?;
        let mut args = Vec::with_capacity(info.params.len());
        for &(_, pt) in info.params {
            args.push(match pt {
                ParamType::Int | ParamType::Bool => Arg::Int(self.expr(depth + 1)?),
                ParamType::Hash => Arg::Hash(self.u32()?),
                ParamType::Str => Arg::Str(match self.u()? {
                    0 => StrRef::Null,
                    1 => StrRef::Int(self.s()?),
                    2 => StrRef::Text(self.str()?),
                    3 => StrRef::Local(self.u()? as usize),
                    t => return Err(self.err(&format!("bad string tag {t}"))),
                }),
                ParamType::Ships => Arg::Ships(match self.u()? {
                    0 => ShipsRef::Null,
                    1 => ShipsRef::Selection(self.u32()?),
                    2 => ShipsRef::Team(self.team()?),
                    t => return Err(self.err(&format!("bad selection tag {t}"))),
                }),
                ParamType::Point => Arg::Point(match self.u()? {
                    0 => PointRef::Null,
                    1 => PointRef::Point(self.u32()?),
                    2 => PointRef::Ships(self.u32()?),
                    3 => PointRef::Team(self.team()?),
                    4 => PointRef::Volume(self.u32()?),
                    t => return Err(self.err(&format!("bad point tag {t}"))),
                }),
                ParamType::Team => Arg::Team(self.team()?),
                ParamType::Volume => Arg::Volume(self.opt_u()?.map(|v| v as u32)),
                ParamType::Path => Arg::Path(self.opt_u()?.map(|v| v as u32)),
            });
        }
        Ok(Call { func: FuncId(f as u16), args })
    }
    fn cond(&mut self, depth: usize) -> Result<Cond, Error> {
        if depth > 512 {
            return Err(self.err("nesting too deep"));
        }
        let d = depth + 1;
        Ok(match self.u()? {
            0 => Cond::True,
            1 => Cond::False,
            2 => Cond::Not(Box::new(self.cond(d)?)),
            3 => Cond::And(Box::new(self.cond(d)?), Box::new(self.cond(d)?)),
            4 => Cond::Or(Box::new(self.cond(d)?), Box::new(self.cond(d)?)),
            t @ 5..=14 => Cond::Cmp(CMP_OPS[(t - 5) as usize], self.expr(d)?, self.expr(d)?),
            t => return Err(self.err(&format!("bad condition tag {t}"))),
        })
    }
    fn expr(&mut self, depth: usize) -> Result<Expr, Error> {
        if depth > 512 {
            return Err(self.err("nesting too deep"));
        }
        let d = depth + 1;
        Ok(match self.u()? {
            0 => Expr::Int(self.s()?),
            1 => Expr::Call(Box::new(self.call(d)?)),
            t @ 2..=11 => Expr::Bin(BIN_OPS[(t - 2) as usize], Box::new(self.expr(d)?), Box::new(self.expr(d)?)),
            12 => Expr::Neg(Box::new(self.expr(d)?)),
            13 => Expr::BitNot(Box::new(self.expr(d)?)),
            14 => Expr::Cond(Box::new(self.cond(d)?)),
            t => return Err(self.err(&format!("bad expression tag {t}"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_tables_match_discriminants() {
        for (i, op) in CMP_OPS.iter().enumerate() {
            assert_eq!(*op as usize, i);
        }
        for (i, op) in BIN_OPS.iter().enumerate() {
            assert_eq!(*op as usize, i);
        }
    }

    #[test]
    fn rejects_garbage() {
        assert!(decode(b"nope").is_err());
        assert!(decode(b"HKAS\x01\x00\x05ab").is_err());
    }
}
