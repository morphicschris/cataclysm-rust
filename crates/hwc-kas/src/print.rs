//! Renders mission IR as KAS-like text.
//!
//! The output follows the layout of the decompiler's `.kas` files and adds the extensions listed
//! in [`crate::text`] where the IR holds more than plain KAS can say, so
//! `text::parse_mission(print_mission(m))` gives back the same code for any IR.

use std::fmt::Write;

use crate::functions::{Builtin, ParamType};
use crate::ir::*;
use crate::{kas_hash, var_hash, Hash, GLOBAL_FLAG};

/// Renders a whole mission.
pub fn print_mission(m: &Mission) -> String {
    let p = Printer { m, lnames: lstring_names(m) };
    let mut o = String::new();
    let _ = writeln!(o, "//\n//  {}  (hwc-kas IR, {:?} fidelity)\n//\n", m.name, m.fidelity);
    if !m.lstrings.is_empty() {
        o.push_str("LOCALIZATION\n");
        for (l, name) in m.lstrings.iter().zip(&p.lnames) {
            let _ = write!(o, "    LSTRING_{name}");
            if let Some(a) = l.addr {
                let _ = write!(o, "        // {a:08x}");
            }
            o.push('\n');
            for t in &l.texts {
                let _ = writeln!(o, "        \"{}\"", t.replace('\n', "\\n"));
            }
        }
        o.push_str("ENDL\n");
    }
    for f in &m.fsms {
        let name = p.label(f.hash);
        let _ = writeln!(o, "\n//{}\nFSM {name}", "-".repeat(70));
        if !f.states.is_empty() {
            let list: Vec<String> = f.states.iter().map(|s| p.label(s.hash)).collect();
            let _ = writeln!(o, "    States {};", list.join(", "));
        }
        p.block(&mut o, "INIT", "ENDINIT", &f.init, 1, Some(f));
        p.block(&mut o, "WATCH", "ENDWATCH", &f.watch, 1, Some(f));
        for s in &f.states {
            let sname = p.label(s.hash);
            let _ = writeln!(o, "\n    STATE {sname}");
            p.block(&mut o, "INIT", "ENDINIT", &s.init, 2, Some(f));
            p.block(&mut o, "WATCH", "ENDWATCH", &s.watch, 2, Some(f));
            let _ = writeln!(o, "    ENDSTATE {sname}");
        }
        let _ = writeln!(o, "ENDFSM {name}");
    }
    let _ = writeln!(o, "\n//{}\n// mission-level code", "-".repeat(70));
    p.block(&mut o, "INIT", "ENDINIT", &m.init, 0, None);
    p.block(&mut o, "WATCH", "ENDWATCH", &m.watch, 0, None);
    o
}

fn lstring_names(m: &Mission) -> Vec<String> {
    let mut used = std::collections::HashSet::new();
    m.lstrings
        .iter()
        .enumerate()
        .map(|(i, l)| {
            let base = l
                .name
                .as_deref()
                .filter(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'))
                .map_or_else(|| format!("S{i}"), str::to_string);
            let name = if used.contains(&base) { format!("{base}_{i}") } else { base };
            used.insert(name.clone());
            name
        })
        .collect()
}

struct Printer<'a> {
    m: &'a Mission,
    lnames: Vec<String>,
}

const IND: &str = "    ";

impl Printer<'_> {
    /// An FSM/state/team/selection label as an identifier.
    fn label(&self, h: Hash) -> String {
        if h & 0xF000_0000 == 0xF000_0000 {
            return format!("UnreferencedState_{:07x}", h & 0x0FFF_FFFF);
        }
        match self.m.names.get(&h) {
            Some(n) if kas_hash(n) == h && is_ident(n) && !looks_like_hash(n) => n.clone(),
            _ => format!("H{h:08x}"),
        }
    }

    /// A hashed name in a UDWORD position.
    fn hash_arg(&self, h: Hash, var: bool) -> String {
        if let Some(n) = self.m.names.get(&(h & !GLOBAL_FLAG)) {
            let ok = if var { var_hash(n) == h } else { kas_hash(n) == h };
            if ok && !n.contains('"') {
                return format!("\"{n}\"");
            }
        }
        format!("#{h:08x}")
    }

    fn block(&self, o: &mut String, kw: &str, end: &str, b: &Block, depth: usize, fsm: Option<&Fsm>) {
        let pad = IND.repeat(depth);
        let _ = write!(o, "{pad}{kw}");
        if let Some(a) = b.addr {
            let _ = write!(o, "    // {a:08x}");
        }
        o.push('\n');
        self.stmts(o, &b.stmts, depth + 1, fsm);
        let _ = writeln!(o, "{pad}{end}");
    }

    fn stmts(&self, o: &mut String, stmts: &[Stmt], depth: usize, fsm: Option<&Fsm>) {
        for (i, s) in stmts.iter().enumerate() {
            self.stmt(o, s, depth, fsm);
            if matches!(s, Stmt::MsgLoop { .. }) && matches!(stmts.get(i + 1), Some(Stmt::MsgLoop { .. })) {
                let _ = writeln!(o, "{}ENDMSGLOOP", IND.repeat(depth));
            }
        }
    }

    fn stmt(&self, o: &mut String, s: &Stmt, depth: usize, fsm: Option<&Fsm>) {
        let pad = IND.repeat(depth);
        match s {
            Stmt::Call(c) => {
                let _ = writeln!(o, "{pad}{};", self.call(c));
            }
            Stmt::If { cond, then, otherwise } => {
                let _ = writeln!(o, "{pad}IF ({})", self.cond(cond, 0));
                self.stmts(o, then, depth + 1, fsm);
                let mut rest = otherwise;
                loop {
                    match rest.as_slice() {
                        [] => break,
                        [Stmt::If { cond, then, otherwise }] => {
                            let _ = writeln!(o, "{pad}ELSEIF ({})", self.cond(cond, 0));
                            self.stmts(o, then, depth + 1, fsm);
                            rest = otherwise;
                        }
                        _ => {
                            let _ = writeln!(o, "{pad}ELSE");
                            self.stmts(o, rest, depth + 1, fsm);
                            break;
                        }
                    }
                }
                let _ = writeln!(o, "{pad}ENDIF");
            }
            Stmt::IfOnce { var, cond, body, otherwise } => {
                let c = cond.as_ref().map_or_else(|| "TRUE".to_string(), |c| self.cond(c, 0));
                let _ = writeln!(o, "{pad}IFONCE #{var:08x} ({c})");
                self.stmts(o, body, depth + 1, fsm);
                let _ = writeln!(o, "{pad}ENDIFONCE");
                if !otherwise.is_empty() {
                    let _ = writeln!(o, "{pad}ELSEONCE");
                    self.stmts(o, otherwise, depth + 1, fsm);
                    let _ = writeln!(o, "{pad}ENDELSEONCE");
                }
            }
            Stmt::Switch { value, arms, default } => {
                let _ = writeln!(o, "{pad}SWITCH ({})", self.expr(value, 0));
                for a in arms {
                    let vals: Vec<String> = a.values.iter().map(i32::to_string).collect();
                    let _ = writeln!(o, "{pad}{IND}CASE ({})", vals.join(", "));
                    self.stmts(o, &a.body, depth + 2, fsm);
                    let _ = writeln!(o, "{pad}{IND}ENDCASE");
                }
                if let Some(d) = default {
                    let _ = writeln!(o, "{pad}{IND}DEFAULT");
                    self.stmts(o, d, depth + 2, fsm);
                    let _ = writeln!(o, "{pad}{IND}ENDCASE");
                }
                let _ = writeln!(o, "{pad}ENDSWITCH");
            }
            Stmt::MsgLoop { handlers, default } => {
                let _ = writeln!(o, "{pad}// message handlers");
                for h in handlers {
                    let msgs: Vec<String> = h.msgs.iter().map(|&m| self.hash_arg(m, false)).collect();
                    let _ = writeln!(o, "{pad}ONMSG ({})", msgs.join(", "));
                    self.stmts(o, &h.body, depth + 1, fsm);
                    let _ = writeln!(o, "{pad}ENDONMSG");
                }
                if let Some(d) = default {
                    let _ = writeln!(o, "{pad}ONMSG DEFAULT");
                    self.stmts(o, d, depth + 1, fsm);
                    let _ = writeln!(o, "{pad}ENDONMSG");
                }
            }
            Stmt::Jump { hash, .. } => {
                let _ = writeln!(o, "{pad}JUMP {};", self.label(*hash));
            }
            Stmt::FsmCreate { fsm: f, team } => {
                let fh = self.m.fsms.get(*f).map_or(0, |f| f.hash);
                let _ = writeln!(o, "{pad}FSMCreate({}, {});", self.label(fh), self.team(*team));
            }
            Stmt::Return => {
                let _ = writeln!(o, "{pad}RETURN;");
            }
        }
    }

    fn call(&self, c: &Call) -> String {
        let info = c.func.info();
        let var = matches!(
            info.builtin,
            Some(
                Builtin::TimerCreate
                    | Builtin::TimerSet
                    | Builtin::TimerStart
                    | Builtin::TimerCreateSetStart
                    | Builtin::TimerStop
                    | Builtin::TimerRemaining
                    | Builtin::TimerExpired
                    | Builtin::TimerExpiredDestroy
                    | Builtin::TimerDestroy
                    | Builtin::VarCreate
                    | Builtin::VarSet
                    | Builtin::VarCreateSet
                    | Builtin::VarInc
                    | Builtin::VarDec
                    | Builtin::VarGet
                    | Builtin::VarDestroy
            )
        );
        let args: Vec<String> = c.args.iter().map(|a| self.arg(a, var)).collect();
        format!("{}({})", info.kas, args.join(", "))
    }

    fn team(&self, t: TeamRef) -> String {
        match t {
            TeamRef::Null => "NULL".into(),
            TeamRef::This => "THISTEAM".into(),
            TeamRef::Label(h) => format!("TEAM_{}", self.label(h)),
        }
    }

    fn arg(&self, a: &Arg, var: bool) -> String {
        match a {
            Arg::Int(e) => self.expr(e, 0),
            Arg::Hash(h) => self.hash_arg(*h, var),
            Arg::Str(s) => match s {
                StrRef::Null => "NULL".into(),
                StrRef::Int(v) => v.to_string(),
                StrRef::Text(t) => format!("\"{}\"", t.replace('"', "\\\"")),
                StrRef::Local(i) => format!("LSTRING_{}", self.lnames.get(*i).map_or("?", String::as_str)),
            },
            Arg::Ships(s) => match *s {
                ShipsRef::Null => "NULL".into(),
                ShipsRef::Selection(h) => format!("SHIPS_{}", self.label(h)),
                ShipsRef::Team(TeamRef::This) => "THISTEAMSHIPS".into(),
                ShipsRef::Team(TeamRef::Label(h)) => format!("TEAMSHIPS_{}", self.label(h)),
                ShipsRef::Team(TeamRef::Null) => "NULL".into(),
            },
            Arg::Point(p) => match *p {
                PointRef::Null | PointRef::Team(TeamRef::Null) => "NULL".into(),
                PointRef::Point(h) => format!("POINT_{}", self.label(h)),
                PointRef::Ships(h) => format!("SHIPSPOINT_{}", self.label(h)),
                PointRef::Team(TeamRef::This) => "THISTEAMSPOINT".into(),
                PointRef::Team(TeamRef::Label(h)) => format!("TEAMSPOINT_{}", self.label(h)),
                PointRef::Volume(h) => format!("VOLUMEPOINT_{}", self.label(h)),
            },
            Arg::Team(t) => self.team(*t),
            Arg::Volume(v) => v.map_or_else(|| "NULL".into(), |h| format!("VOLUME_{}", self.label(h))),
            Arg::Path(v) => v.map_or_else(|| "NULL".into(), |h| format!("PATH_{}", self.label(h))),
        }
    }

    fn cond(&self, c: &Cond, prec: u8) -> String {
        let (s, p) = match c {
            Cond::True => ("TRUE".to_string(), 9),
            Cond::False => ("FALSE".to_string(), 9),
            Cond::Or(a, b) => (format!("{} OR {}", self.cond(a, 1), self.cond(b, 2)), 1),
            Cond::And(a, b) => (format!("{} AND {}", self.cond(a, 2), self.cond(b, 3)), 2),
            Cond::Not(a) => (format!("NOT {}", self.cond(a, 3)), 3),
            Cond::Cmp(CmpOp::Ne, e, Expr::Int(0)) if !matches!(e, Expr::Cond(_)) => (self.expr(e, 9), 9),
            Cond::Cmp(op, a, b) => {
                let o = match op {
                    CmpOp::Eq => "=",
                    CmpOp::Ne => "!=",
                    CmpOp::Lt => "<",
                    CmpOp::Le => "<=",
                    CmpOp::Gt => ">",
                    CmpOp::Ge => ">=",
                    CmpOp::ULt => "U<",
                    CmpOp::ULe => "U<=",
                    CmpOp::UGt => "U>",
                    CmpOp::UGe => "U>=",
                };
                (format!("{} {o} {}", self.expr(a, 6), self.expr(b, 6)), 4)
            }
        };
        if p < prec {
            format!("({s})")
        } else {
            s
        }
    }

    fn expr(&self, e: &Expr, prec: u8) -> String {
        match e {
            Expr::Int(v) => v.to_string(),
            Expr::Call(c) => self.call(c),
            Expr::Bin(op, a, b) => {
                let p = op.precedence();
                let s = format!("{} {} {}", self.expr(a, p), op.symbol(), self.expr(b, p + 1));
                if p < prec {
                    format!("({s})")
                } else {
                    s
                }
            }
            Expr::Neg(a) => match **a {
                Expr::Int(_) => format!("-({})", self.expr(a, 0)),
                _ => format!("-{}", self.expr(a, 9)),
            },
            Expr::BitNot(a) => format!("~{}", self.expr(a, 9)),
            // spell out `!= 0` so the parenthesised value reads back as a condition
            Expr::Cond(c) => match &**c {
                Cond::Cmp(CmpOp::Ne, e, Expr::Int(0)) => format!("({} != 0)", self.expr(e, 6)),
                c => format!("({})", self.cond(c, 0)),
            },
        }
    }
}

fn is_ident(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && (b[0].is_ascii_alphabetic() || b[0] == b'_') && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'_')
}

fn looks_like_hash(s: &str) -> bool {
    (s.len() == 9 && s.starts_with('H') && s[1..].bytes().all(|b| b.is_ascii_hexdigit()))
        || s.starts_with("UnreferencedState_")
}

/// The type of each parameter, for callers that render arguments themselves.
pub fn param_types(c: &Call) -> Vec<ParamType> {
    c.func.info().params.iter().map(|p| p.1).collect()
}
