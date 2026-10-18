//! Parser for the KAS-like text form of a mission.
//!
//! Accepts the `.kas` files written by `tools/scripts/kas_decompile.py` and the output of
//! [`crate::print`](mod@crate::print). The decompiler's rendering is not exact; IR parsed from it is marked
//! [`Fidelity::Text`] and differs from the compiled code in these ways:
//!
//! * IFONCE guard variables are not shown. Each IFONCE gets a synthetic hash
//!   (`0x7000_0000 | n`, outside the range of real label hashes), so guards still work, but code
//!   that reads the guard variable elsewhere (rare hand-written once-patterns the decompiler
//!   folded into IFONCE) no longer sees it.
//! * Unsigned comparisons are rendered like signed ones.
//! * A `switch` on one value is rendered as an IF/ELSEIF chain that re-evaluates the value for
//!   every arm (this matters for `Random(...)` switches).
//! * A name shown for a `G_` variable/timer is assumed to carry the global bit.
//!
//! The printer's extensions make the text exact: `IFONCE #hash (cond)`, unsigned comparisons
//! `U<` `U<=` `U>` `U>=`, and `SWITCH (expr) CASE (v, ...) ... ENDCASE DEFAULT ... ENDCASE
//! ENDSWITCH`.

use std::collections::{BTreeMap, HashMap};

use crate::functions::{Builtin, FuncId, ParamType};
use crate::ir::*;
use crate::{kas_hash, var_hash, Error, Hash};

/// First synthetic IFONCE guard hash (real label hashes are below `0x1000_0000`).
pub const SYNTHETIC_IFONCE_BASE: Hash = 0x7000_0000;

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Ident(String),
    Int(i64),
    HashLit(u32),
    Str(String),
    Punct(&'static str),
    Comment(String),
    Eof,
}

#[derive(Clone, Debug)]
struct Token {
    tok: Tok,
    line: usize,
}

const PUNCTS: [&str; 22] = [
    "U<=", "U>=", "U<", "U>", "<<", ">>", "<=", ">=", "!=", "==", "(", ")", ",", ";", "=", "<", ">", "+", "-", "*",
    "/", "%",
];
const PUNCTS1: [&str; 4] = ["&", "|", "^", "~"];

fn lex(src: &str) -> Result<Vec<Token>, Error> {
    let s = src.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < s.len() {
        let c = s[i];
        if c == b'\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if s[i..].starts_with(b"//") {
            let st = i + 2;
            while i < s.len() && s[i] != b'\n' {
                i += 1;
            }
            out.push(Token { tok: Tok::Comment(String::from_utf8_lossy(&s[st..i]).trim().to_string()), line });
            continue;
        }
        if c == b'"' {
            let start_line = line;
            i += 1;
            let mut buf = Vec::new();
            loop {
                match s.get(i) {
                    None => return Err(Error::new(format!("line {start_line}: unterminated string"))),
                    Some(b'"') => {
                        i += 1;
                        break;
                    }
                    Some(b'\\') if s.get(i + 1) == Some(&b'"') => {
                        buf.push(b'"');
                        i += 2;
                    }
                    // CRLF checkouts: a line break inside a literal is a single '\n'
                    Some(b'\r') if s.get(i + 1) == Some(&b'\n') => i += 1,
                    Some(&b) => {
                        if b == b'\n' {
                            line += 1;
                        }
                        buf.push(b);
                        i += 1;
                    }
                }
            }
            out.push(Token { tok: Tok::Str(String::from_utf8_lossy(&buf).into_owned()), line: start_line });
            continue;
        }
        if c == b'#' {
            let st = i + 1;
            i += 1;
            while i < s.len() && s[i].is_ascii_hexdigit() {
                i += 1;
            }
            let t = std::str::from_utf8(&s[st..i]).unwrap_or("");
            let v = u32::from_str_radix(t, 16).map_err(|_| Error::new(format!("line {line}: bad hash literal")))?;
            out.push(Token { tok: Tok::HashLit(v), line });
            continue;
        }
        if c.is_ascii_digit() {
            let st = i;
            let v = if s[i..].starts_with(b"0x") || s[i..].starts_with(b"0X") {
                i += 2;
                while i < s.len() && s[i].is_ascii_hexdigit() {
                    i += 1;
                }
                i64::from_str_radix(std::str::from_utf8(&s[st + 2..i]).unwrap_or(""), 16)
            } else {
                while i < s.len() && s[i].is_ascii_digit() {
                    i += 1;
                }
                std::str::from_utf8(&s[st..i]).unwrap_or("").parse::<i64>()
            };
            let v = v.map_err(|_| Error::new(format!("line {line}: bad number")))?;
            out.push(Token { tok: Tok::Int(v), line });
            continue;
        }
        let unsigned_cmp = c == b'U' && matches!(s.get(i + 1), Some(b'<') | Some(b'>'));
        if !unsigned_cmp && (c.is_ascii_alphabetic() || c == b'_') {
            let st = i;
            while i < s.len() && (s[i].is_ascii_alphanumeric() || s[i] == b'_') {
                i += 1;
            }
            out.push(Token { tok: Tok::Ident(String::from_utf8_lossy(&s[st..i]).into_owned()), line });
            continue;
        }
        let p = PUNCTS
            .iter()
            .chain(PUNCTS1.iter())
            .find(|p| s[i..].starts_with(p.as_bytes()))
            .ok_or_else(|| Error::new(format!("line {line}: unexpected character {:?}", c as char)))?;
        i += p.len();
        out.push(Token { tok: Tok::Punct(p), line });
    }
    out.push(Token { tok: Tok::Eof, line });
    Ok(out)
}

/// Parses a mission from its text form. `name` is the script name (e.g. `Mission01`).
pub fn parse_mission(name: &str, src: &str) -> Result<Mission, Error> {
    let toks = lex(src)?;
    let mut p = Parser {
        toks,
        pos: 0,
        names: BTreeMap::new(),
        lstrings: Vec::new(),
        lstring_names: HashMap::new(),
        next_ifonce: 0,
    };
    p.mission(name)
}

/// A condition or an expression while parsing (KAS lets either appear in both places).
enum Node {
    C(Cond),
    E(Expr),
}

impl Node {
    fn cond(self) -> Cond {
        match self {
            Node::C(c) => c,
            Node::E(e) => Cond::truth(e),
        }
    }
    fn expr(self) -> Expr {
        match self {
            Node::E(e) => e,
            Node::C(c) => Expr::Cond(Box::new(c)),
        }
    }
}

const UNRESOLVED: usize = usize::MAX;

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    names: BTreeMap<Hash, String>,
    lstrings: Vec<LString>,
    lstring_names: HashMap<String, usize>,
    next_ifonce: u32,
}

impl Parser {
    // ---- token helpers --------------------------------------------------------------------
    fn skip_comments(&mut self) {
        while matches!(self.toks[self.pos].tok, Tok::Comment(_)) {
            self.pos += 1;
        }
    }

    fn peek(&mut self) -> &Tok {
        self.skip_comments();
        &self.toks[self.pos].tok
    }

    fn peek2(&mut self) -> &Tok {
        self.skip_comments();
        let mut j = self.pos + 1;
        while matches!(self.toks[j].tok, Tok::Comment(_)) {
            j += 1;
        }
        &self.toks[j].tok
    }

    fn line(&mut self) -> usize {
        self.skip_comments();
        self.toks[self.pos].line
    }

    fn next(&mut self) -> Tok {
        self.skip_comments();
        let t = self.toks[self.pos].tok.clone();
        if t != Tok::Eof {
            self.pos += 1;
        }
        t
    }

    fn err(&mut self, msg: impl std::fmt::Display) -> Error {
        let line = self.line();
        let t = self.toks[self.pos].tok.clone();
        Error::new(format!("line {line}: {msg} (at {t:?})"))
    }

    fn is_kw(&mut self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }

    fn eat_kw(&mut self, kw: &str) -> bool {
        if self.is_kw(kw) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_kw(&mut self, kw: &str) -> Result<(), Error> {
        if self.eat_kw(kw) {
            Ok(())
        } else {
            Err(self.err(format_args!("expected {kw}")))
        }
    }

    fn is_punct(&mut self, p: &str) -> bool {
        matches!(self.peek(), Tok::Punct(q) if *q == p)
    }

    fn eat_punct(&mut self, p: &str) -> bool {
        if self.is_punct(p) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, p: &str) -> Result<(), Error> {
        if self.eat_punct(p) {
            Ok(())
        } else {
            Err(self.err(format_args!("expected '{p}'")))
        }
    }

    fn ident(&mut self) -> Result<String, Error> {
        match self.next() {
            Tok::Ident(s) => Ok(s),
            t => {
                self.pos -= 1;
                Err(self.err(format_args!("expected a name, got {t:?}")))
            }
        }
    }

    /// An address comment (`// 005d4370`) on the same line as the previous token.
    fn addr_comment(&mut self) -> Option<u32> {
        let prev_line = self.toks[self.pos.checked_sub(1)?].line;
        if let Tok::Comment(c) = &self.toks[self.pos].tok {
            if self.toks[self.pos].line == prev_line {
                let v = (c.len() == 8).then(|| u32::from_str_radix(c, 16).ok()).flatten();
                self.pos += 1;
                return v;
            }
        }
        None
    }

    // ---- labels ---------------------------------------------------------------------------
    fn label(&mut self, name: &str) -> Hash {
        if name.len() == 9 && name.starts_with('H') {
            if let Ok(h) = u32::from_str_radix(&name[1..], 16) {
                return h;
            }
        }
        if let Some(rest) = name.strip_prefix("UnreferencedState_") {
            if let Ok(h) = u32::from_str_radix(rest, 16) {
                return 0xF000_0000 | (h & 0x0FFF_FFFF);
            }
        }
        let h = kas_hash(name);
        self.names.entry(h).or_insert_with(|| name.to_string());
        h
    }

    fn hash_name(&mut self, name: &str, var: bool) -> Hash {
        let h = if var { var_hash(name) } else { kas_hash(name) };
        self.names.entry(kas_hash(name)).or_insert_with(|| name.to_string());
        h
    }

    /// A hashed name in a `UDWORD` position: `"Name"`, `#xxxxxxxx` or a number.
    fn hash_arg(&mut self, var: bool) -> Result<Hash, Error> {
        match self.next() {
            Tok::Str(s) => Ok(self.hash_name(&s, var)),
            Tok::HashLit(h) => Ok(h),
            Tok::Int(v) => Ok(v as u32),
            Tok::Punct("-") => match self.next() {
                Tok::Int(v) => Ok((-v) as u32),
                _ => Err(self.err("expected a number")),
            },
            t => {
                self.pos -= 1;
                Err(self.err(format_args!("expected a label hash, got {t:?}")))
            }
        }
    }

    // ---- file -----------------------------------------------------------------------------
    fn mission(&mut self, name: &str) -> Result<Mission, Error> {
        let mut fsms = Vec::new();
        let mut init = None;
        let mut watch = None;
        loop {
            match self.peek().clone() {
                Tok::Eof => break,
                Tok::Ident(k) if k == "LOCALIZATION" => {
                    self.pos += 1;
                    self.localization()?;
                }
                Tok::Ident(k) if k == "FSM" => {
                    self.pos += 1;
                    fsms.push(self.fsm()?);
                }
                Tok::Ident(k) if k == "INIT" => {
                    self.pos += 1;
                    init = Some(self.block("ENDINIT")?);
                }
                Tok::Ident(k) if k == "WATCH" => {
                    self.pos += 1;
                    watch = Some(self.block("ENDWATCH")?);
                }
                _ => return Err(self.err("expected LOCALIZATION, FSM, INIT or WATCH")),
            }
        }
        let fsm_index: HashMap<Hash, usize> = fsms.iter().enumerate().map(|(i, f)| (f.hash, i)).collect();
        let mut m = Mission {
            name: name.to_string(),
            name_hash: kas_hash(name),
            lstrings: std::mem::take(&mut self.lstrings),
            fsms,
            init: init.ok_or_else(|| Error::new("no mission-level INIT"))?,
            watch: watch.ok_or_else(|| Error::new("no mission-level WATCH"))?,
            names: std::mem::take(&mut self.names),
            fidelity: Fidelity::Text,
        };
        let fix = |stmts: &mut Vec<Stmt>| fix_fsm_creates(stmts, &fsm_index);
        fix(&mut m.init.stmts)?;
        fix(&mut m.watch.stmts)?;
        for f in &mut m.fsms {
            fix(&mut f.init.stmts)?;
            fix(&mut f.watch.stmts)?;
            for s in &mut f.states {
                fix(&mut s.init.stmts)?;
                fix(&mut s.watch.stmts)?;
            }
        }
        Ok(m)
    }

    fn localization(&mut self) -> Result<(), Error> {
        while !self.eat_kw("ENDL") {
            let id = self.ident()?;
            let name = id.strip_prefix("LSTRING_").ok_or_else(|| self.err("expected LSTRING_<name>"))?.to_string();
            let addr = self.addr_comment();
            let mut texts: [String; 3] = Default::default();
            for t in &mut texts {
                match self.next() {
                    Tok::Str(s) => *t = s.replace("\\n", "\n"),
                    _ => return Err(self.err("expected three strings per LSTRING")),
                }
            }
            self.lstring_names.insert(name.clone(), self.lstrings.len());
            self.lstrings.push(LString { name: Some(name), addr, texts });
        }
        Ok(())
    }

    fn fsm(&mut self) -> Result<Fsm, Error> {
        let name = self.ident()?;
        let hash = self.label(&name);
        let mut declared = Vec::new();
        if self.eat_kw("States") {
            loop {
                let s = self.ident()?;
                declared.push(self.label(&s));
                if self.eat_punct(";") {
                    break;
                }
                self.expect_punct(",")?;
            }
        }
        self.expect_kw("INIT")?;
        let init = self.block("ENDINIT")?;
        self.expect_kw("WATCH")?;
        let watch = self.block("ENDWATCH")?;
        let mut states = Vec::new();
        while self.eat_kw("STATE") {
            let sname = self.ident()?;
            let sh = self.label(&sname);
            self.expect_kw("INIT")?;
            let sinit = self.block("ENDINIT")?;
            self.expect_kw("WATCH")?;
            let swatch = self.block("ENDWATCH")?;
            self.expect_kw("ENDSTATE")?;
            if !matches!(self.peek(), Tok::Ident(k) if k == "STATE" || k == "ENDFSM") {
                self.ident()?;
            }
            states.push(State { hash: sh, init: sinit, watch: swatch });
        }
        self.expect_kw("ENDFSM")?;
        if matches!(self.peek(), Tok::Ident(k) if k != "FSM" && k != "INIT" && k != "WATCH") {
            self.ident()?;
        }
        // declared-but-missing states become empty states (nothing JUMPs to them otherwise)
        for d in declared {
            if !states.iter().any(|s| s.hash == d) {
                states.push(State { hash: d, init: Block::default(), watch: Block::default() });
            }
        }
        let index: HashMap<Hash, usize> = states.iter().enumerate().map(|(i, s)| (s.hash, i)).collect();
        let mut f = Fsm { hash, init, watch, states };
        let ctx = |e: Error| e.context(&format!("FSM {name}"));
        fix_jumps(&mut f.init.stmts, &index).map_err(ctx)?;
        fix_jumps(&mut f.watch.stmts, &index).map_err(ctx)?;
        for s in &mut f.states {
            fix_jumps(&mut s.init.stmts, &index).map_err(ctx)?;
            fix_jumps(&mut s.watch.stmts, &index).map_err(ctx)?;
        }
        Ok(f)
    }

    fn block(&mut self, end: &str) -> Result<Block, Error> {
        let addr = self.addr_comment();
        let stmts = self.stmts(&[end])?;
        self.expect_kw(end)?;
        Ok(Block { addr, stmts })
    }

    // ---- statements -----------------------------------------------------------------------
    fn stmts(&mut self, ends: &[&str]) -> Result<Vec<Stmt>, Error> {
        let mut out = Vec::new();
        loop {
            match self.peek() {
                Tok::Ident(k) if ends.contains(&k.as_str()) => return Ok(out),
                Tok::Eof => return Err(self.err(format_args!("unexpected end of file, expected {}", ends.join("/")))),
                _ => {}
            }
            if self.is_kw("ONMSG") {
                out.push(self.msg_loop()?);
            } else {
                out.push(self.stmt()?);
            }
        }
    }

    fn stmt(&mut self) -> Result<Stmt, Error> {
        let line = self.line();
        let kw = self.ident()?;
        let r = match kw.as_str() {
            "IF" => self.if_rest(),
            "IFONCE" => {
                let var = match self.peek() {
                    Tok::HashLit(h) => {
                        let h = *h;
                        self.pos += 1;
                        h
                    }
                    _ => {
                        self.next_ifonce += 1;
                        SYNTHETIC_IFONCE_BASE | self.next_ifonce
                    }
                };
                self.expect_punct("(")?;
                let cond = match self.or()? {
                    Node::C(Cond::True) => None,
                    n => Some(n.cond()),
                };
                self.expect_punct(")")?;
                let body = self.stmts(&["ENDIFONCE"])?;
                self.expect_kw("ENDIFONCE")?;
                let otherwise = if self.eat_kw("ELSEONCE") {
                    let o = self.stmts(&["ENDELSEONCE"])?;
                    self.expect_kw("ENDELSEONCE")?;
                    o
                } else {
                    Vec::new()
                };
                Ok(Stmt::IfOnce { var, cond, body, otherwise })
            }
            "SWITCH" => self.switch_rest(),
            "JUMP" => {
                let s = self.ident()?;
                let hash = self.label(&s);
                self.expect_punct(";")?;
                Ok(Stmt::Jump { state: UNRESOLVED, hash })
            }
            "RETURN" => {
                self.expect_punct(";")?;
                Ok(Stmt::Return)
            }
            "FSMCreate" => {
                self.expect_punct("(")?;
                let f = self.ident()?;
                let hash = self.label(&f);
                self.expect_punct(",")?;
                let team = self.team_ref()?;
                self.expect_punct(")")?;
                self.expect_punct(";")?;
                // the FSM may be defined later: carry its hash until fix_fsm_creates resolves it
                Ok(Stmt::FsmCreate { fsm: UNRESOLVED - hash as usize, team })
            }
            _ => {
                self.pos -= 1;
                let c = self.call()?;
                self.expect_punct(";")?;
                Ok(Stmt::Call(c))
            }
        };
        r.map_err(|e| if e.message.starts_with("line ") { e } else { e.context(&format!("line {line}")) })
    }

    fn if_rest(&mut self) -> Result<Stmt, Error> {
        self.expect_punct("(")?;
        let cond = self.or()?.cond();
        self.expect_punct(")")?;
        let then = self.stmts(&["ELSEIF", "ELSE", "ENDIF"])?;
        let otherwise = if self.eat_kw("ELSEIF") {
            vec![self.if_rest()?]
        } else if self.eat_kw("ELSE") {
            let o = self.stmts(&["ENDIF"])?;
            self.expect_kw("ENDIF")?;
            o
        } else {
            self.expect_kw("ENDIF")?;
            Vec::new()
        };
        Ok(Stmt::If { cond, then, otherwise })
    }

    fn switch_rest(&mut self) -> Result<Stmt, Error> {
        self.expect_punct("(")?;
        let value = self.or()?.expr();
        self.expect_punct(")")?;
        let mut arms = Vec::new();
        let mut default = None;
        loop {
            if self.eat_kw("ENDSWITCH") {
                break;
            }
            if self.eat_kw("DEFAULT") {
                let b = self.stmts(&["ENDCASE"])?;
                self.expect_kw("ENDCASE")?;
                default = Some(b);
                continue;
            }
            self.expect_kw("CASE")?;
            self.expect_punct("(")?;
            let mut values = Vec::new();
            loop {
                values.push(self.const_int()?);
                if self.eat_punct(")") {
                    break;
                }
                self.expect_punct(",")?;
            }
            let body = self.stmts(&["ENDCASE"])?;
            self.expect_kw("ENDCASE")?;
            arms.push(SwitchArm { values, body });
        }
        Ok(Stmt::Switch { value, arms, default })
    }

    fn const_int(&mut self) -> Result<i32, Error> {
        let neg = self.eat_punct("-");
        match self.next() {
            Tok::Int(v) => Ok(if neg { (-v) as i32 } else { v as i32 }),
            _ => Err(self.err("expected a number")),
        }
    }

    fn msg_loop(&mut self) -> Result<Stmt, Error> {
        let mut handlers = Vec::new();
        let mut default = None;
        while self.eat_kw("ONMSG") {
            if self.eat_kw("DEFAULT") {
                let b = self.stmts(&["ENDONMSG"])?;
                self.expect_kw("ENDONMSG")?;
                default = Some(b);
                continue;
            }
            self.expect_punct("(")?;
            let mut msgs = Vec::new();
            loop {
                msgs.push(self.hash_arg(false)?);
                if self.eat_punct(")") {
                    break;
                }
                self.expect_punct(",")?;
            }
            let body = self.stmts(&["ENDONMSG"])?;
            self.expect_kw("ENDONMSG")?;
            handlers.push(MsgHandler { msgs, body });
        }
        // an explicit end marker separates adjacent loops (printer extension)
        self.eat_kw("ENDMSGLOOP");
        Ok(Stmt::MsgLoop { handlers, default })
    }

    // ---- calls and arguments --------------------------------------------------------------
    fn call(&mut self) -> Result<Call, Error> {
        let name = self.ident()?;
        let func = FuncId::by_name(&name).ok_or_else(|| self.err(format_args!("unknown function {name}")))?;
        let info = func.info();
        self.expect_punct("(")?;
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
        let mut args = Vec::with_capacity(info.params.len());
        for (k, &(_, pt)) in info.params.iter().enumerate() {
            if k > 0 {
                self.expect_punct(",")?;
            }
            args.push(self.arg(pt, var)?);
        }
        self.expect_punct(")").map_err(|e| e.context(&format!("{name} takes {} arguments", info.params.len())))?;
        Ok(Call { func, args })
    }

    fn is_null(&mut self) -> bool {
        if self.is_kw("NULL") || matches!(self.peek(), Tok::Int(0)) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn prefixed(&mut self, prefixes: &[&str]) -> Result<(usize, Hash), Error> {
        let id = self.ident()?;
        for (i, p) in prefixes.iter().enumerate() {
            if let Some(rest) = id.strip_prefix(p) {
                if !rest.is_empty() {
                    return Ok((i, self.label(rest)));
                }
            }
        }
        self.pos -= 1;
        Err(self.err(format_args!("expected one of {}", prefixes.join(", "))))
    }

    fn team_ref(&mut self) -> Result<TeamRef, Error> {
        if self.is_null() {
            return Ok(TeamRef::Null);
        }
        if self.eat_kw("THISTEAM") {
            return Ok(TeamRef::This);
        }
        Ok(TeamRef::Label(self.prefixed(&["TEAM_"])?.1))
    }

    fn arg(&mut self, pt: ParamType, var: bool) -> Result<Arg, Error> {
        Ok(match pt {
            ParamType::Int | ParamType::Bool => Arg::Int(self.or()?.expr()),
            ParamType::Hash => Arg::Hash(self.hash_arg(var)?),
            ParamType::Str => Arg::Str(match self.peek().clone() {
                Tok::Str(s) => {
                    self.pos += 1;
                    StrRef::Text(s)
                }
                Tok::Ident(id) if id.starts_with("LSTRING_") => {
                    self.pos += 1;
                    let found = self.lstring_names.get(&id["LSTRING_".len()..]).copied();
                    let idx = found.ok_or_else(|| self.err("unknown LSTRING"))?;
                    StrRef::Local(idx)
                }
                Tok::Ident(id) if id == "NULL" => {
                    self.pos += 1;
                    StrRef::Null
                }
                _ => match self.const_int()? {
                    0 => StrRef::Null,
                    v => StrRef::Int(v),
                },
            }),
            ParamType::Team => Arg::Team(self.team_ref()?),
            ParamType::Ships => Arg::Ships(if self.is_null() {
                ShipsRef::Null
            } else if self.eat_kw("THISTEAMSHIPS") {
                ShipsRef::Team(TeamRef::This)
            } else {
                match self.prefixed(&["SHIPS_", "TEAMSHIPS_"])? {
                    (0, h) => ShipsRef::Selection(h),
                    (_, h) => ShipsRef::Team(TeamRef::Label(h)),
                }
            }),
            ParamType::Point => Arg::Point(if self.is_null() {
                PointRef::Null
            } else if self.eat_kw("THISTEAMSPOINT") {
                PointRef::Team(TeamRef::This)
            } else {
                match self.prefixed(&["POINT_", "SHIPSPOINT_", "TEAMSPOINT_", "VOLUMEPOINT_"])? {
                    (0, h) => PointRef::Point(h),
                    (1, h) => PointRef::Ships(h),
                    (2, h) => PointRef::Team(TeamRef::Label(h)),
                    (_, h) => PointRef::Volume(h),
                }
            }),
            ParamType::Volume => Arg::Volume(if self.is_null() { None } else { Some(self.prefixed(&["VOLUME_"])?.1) }),
            ParamType::Path => Arg::Path(if self.is_null() { None } else { Some(self.prefixed(&["PATH_"])?.1) }),
        })
    }

    // ---- conditions and expressions -------------------------------------------------------
    fn or(&mut self) -> Result<Node, Error> {
        let mut l = self.and()?;
        while self.eat_kw("OR") {
            let r = self.and()?;
            l = Node::C(Cond::Or(Box::new(l.cond()), Box::new(r.cond())));
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Node, Error> {
        let mut l = self.not()?;
        while self.eat_kw("AND") {
            let r = self.not()?;
            l = Node::C(Cond::And(Box::new(l.cond()), Box::new(r.cond())));
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Node, Error> {
        if self.eat_kw("NOT") {
            return Ok(Node::C(Cond::Not(Box::new(self.not()?.cond()))));
        }
        self.cmp()
    }

    fn cmp(&mut self) -> Result<Node, Error> {
        let l = self.bin(0)?;
        let op = match self.peek() {
            Tok::Punct("=") | Tok::Punct("==") => CmpOp::Eq,
            Tok::Punct("!=") => CmpOp::Ne,
            Tok::Punct("<") => CmpOp::Lt,
            Tok::Punct("<=") => CmpOp::Le,
            Tok::Punct(">") => CmpOp::Gt,
            Tok::Punct(">=") => CmpOp::Ge,
            Tok::Punct("U<") => CmpOp::ULt,
            Tok::Punct("U<=") => CmpOp::ULe,
            Tok::Punct("U>") => CmpOp::UGt,
            Tok::Punct("U>=") => CmpOp::UGe,
            _ => return Ok(l),
        };
        self.pos += 1;
        let r = self.bin(0)?;
        Ok(Node::C(Cond::Cmp(op, l.expr(), r.expr())))
    }

    fn binop(&mut self) -> Option<BinOp> {
        Some(match self.peek() {
            Tok::Punct("*") => BinOp::Mul,
            Tok::Punct("/") => BinOp::Div,
            Tok::Punct("%") => BinOp::Rem,
            Tok::Punct("+") => BinOp::Add,
            Tok::Punct("-") => BinOp::Sub,
            Tok::Punct("<<") => BinOp::Shl,
            Tok::Punct(">>") => BinOp::Shr,
            Tok::Punct("&") => BinOp::And,
            Tok::Punct("|") => BinOp::Or,
            Tok::Punct("^") => BinOp::Xor,
            _ => return None,
        })
    }

    fn bin(&mut self, min: u8) -> Result<Node, Error> {
        let mut l = self.unary()?;
        while let Some(op) = self.binop() {
            let p = op.precedence();
            if p < min {
                break;
            }
            self.pos += 1;
            let r = self.bin(p + 1)?;
            l = Node::E(Expr::Bin(op, Box::new(l.expr()), Box::new(r.expr())));
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Node, Error> {
        if self.eat_punct("-") {
            if let Tok::Int(v) = *self.peek() {
                self.pos += 1;
                return Ok(Node::E(Expr::Int((-v) as i32)));
            }
            return Ok(Node::E(Expr::Neg(Box::new(self.unary()?.expr()))));
        }
        if self.eat_punct("~") {
            return Ok(Node::E(Expr::BitNot(Box::new(self.unary()?.expr()))));
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Node, Error> {
        match self.peek().clone() {
            Tok::Int(v) => {
                self.pos += 1;
                Ok(Node::E(Expr::Int(v as i32)))
            }
            Tok::Punct("(") => {
                self.pos += 1;
                let n = self.or()?;
                self.expect_punct(")")?;
                Ok(n)
            }
            Tok::Ident(k) if k == "TRUE" => {
                self.pos += 1;
                Ok(Node::C(Cond::True))
            }
            Tok::Ident(k) if k == "FALSE" => {
                self.pos += 1;
                Ok(Node::C(Cond::False))
            }
            Tok::Ident(_) if *self.peek2() == Tok::Punct("(") => {
                let c = self.call()?;
                if !c.func.info().returns {
                    return Err(self.err(format_args!("{} returns no value", c.func)));
                }
                Ok(Node::E(Expr::Call(Box::new(c))))
            }
            _ => Err(self.err("expected an expression")),
        }
    }
}

fn walk_stmts_mut(stmts: &mut [Stmt], f: &mut dyn FnMut(&mut Stmt) -> Result<(), Error>) -> Result<(), Error> {
    for s in stmts {
        f(s)?;
        match s {
            Stmt::If { then, otherwise, .. } => {
                walk_stmts_mut(then, f)?;
                walk_stmts_mut(otherwise, f)?;
            }
            Stmt::IfOnce { body, otherwise, .. } => {
                walk_stmts_mut(body, f)?;
                walk_stmts_mut(otherwise, f)?;
            }
            Stmt::Switch { arms, default, .. } => {
                for a in arms {
                    walk_stmts_mut(&mut a.body, f)?;
                }
                if let Some(d) = default {
                    walk_stmts_mut(d, f)?;
                }
            }
            Stmt::MsgLoop { handlers, default } => {
                for h in handlers {
                    walk_stmts_mut(&mut h.body, f)?;
                }
                if let Some(d) = default {
                    walk_stmts_mut(d, f)?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn fix_jumps(stmts: &mut [Stmt], index: &HashMap<Hash, usize>) -> Result<(), Error> {
    walk_stmts_mut(stmts, &mut |s| {
        if let Stmt::Jump { state, hash } = s {
            *state = *index.get(hash).ok_or_else(|| Error::new(format!("JUMP to unknown state {hash:08x}")))?;
        }
        Ok(())
    })
}

fn fix_fsm_creates(stmts: &mut [Stmt], index: &HashMap<Hash, usize>) -> Result<(), Error> {
    walk_stmts_mut(stmts, &mut |s| {
        if let Stmt::FsmCreate { fsm, .. } = s {
            let hash = (UNRESOLVED - *fsm) as Hash;
            *fsm = *index.get(&hash).ok_or_else(|| Error::new(format!("FSMCreate of unknown FSM {hash:08x}")))?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = r#"
LOCALIZATION
    LSTRING_Hello        // 00001000
        "Hello\nthere"
        "French version goes here"
        "German version goes here"
ENDL

FSM Alpha
    States One, Two;
    INIT    // 00000010
        VarCreateSet("G_Flag", 0);
        JUMP One;
    ENDINIT
    WATCH
    ENDWATCH

    STATE One
        INIT
        ENDINIT
        WATCH
            IF ((VarGet("x") + 2) * 3 >= 12 AND NOT TimerExpired(#80001234))
                MsgSend(TEAM_Beta, "Go");
                JUMP Two;
            ELSEIF (Random(0, 2) = 1)
                dbgAssert((VarGet("x") <= 12));
            ELSE
                RETURN;
            ENDIF
            IFONCE (TRUE)
                ObjectiveCreate("Obj", LSTRING_Hello, LSTRING_Hello);
            ENDIFONCE
        ENDWATCH
    ENDSTATE One

    STATE Two
        INIT
        ENDINIT
        WATCH
            ONMSG ("Go", #00001234)
                ShipsAdd(SHIPS_H09ad5044, THISTEAMSHIPS);
                RUsSet(1, RUsGet(1) - GetBuildCost("bWorker") / 2);
            ENDONMSG
        ENDWATCH
    ENDSTATE Two
ENDFSM Alpha

INIT    // 00000100
    FSMCreate(Alpha, TEAM_Alpha);
ENDINIT
WATCH
ENDWATCH
"#;

    #[test]
    fn parses_sample() {
        let m = parse_mission("Test", SRC).unwrap();
        assert_eq!(m.lstrings.len(), 1);
        assert_eq!(m.lstrings[0].texts[0], "Hello\nthere");
        assert_eq!(m.lstrings[0].addr, Some(0x1000));
        assert_eq!(m.fsms.len(), 1);
        let f = &m.fsms[0];
        assert_eq!(f.hash, kas_hash("Alpha"));
        assert_eq!(f.init.addr, Some(0x10));
        assert_eq!(f.states.len(), 2);
        match &f.init.stmts[0] {
            Stmt::Call(c) => assert_eq!(c.args[0], Arg::Hash(var_hash("G_Flag"))),
            s => panic!("{s:?}"),
        }
        assert_eq!(f.init.stmts[1], Stmt::Jump { state: 0, hash: kas_hash("One") });
        assert_eq!(m.init.stmts[0], Stmt::FsmCreate { fsm: 0, team: TeamRef::Label(kas_hash("Alpha")) });
        match &f.states[0].watch.stmts[1] {
            Stmt::IfOnce { var, cond: None, .. } => assert_eq!(*var, SYNTHETIC_IFONCE_BASE | 1),
            s => panic!("{s:?}"),
        }
        match &f.states[1].watch.stmts[0] {
            Stmt::MsgLoop { handlers, default: None } => {
                assert_eq!(handlers[0].msgs, vec![kas_hash("Go"), 0x1234]);
                match &handlers[0].body[0] {
                    Stmt::Call(c) => {
                        assert_eq!(c.args[0], Arg::Ships(ShipsRef::Selection(0x09ad5044)));
                        assert_eq!(c.args[1], Arg::Ships(ShipsRef::Team(TeamRef::This)));
                    }
                    s => panic!("{s:?}"),
                }
            }
            s => panic!("{s:?}"),
        }
    }

    #[test]
    fn reports_errors_with_lines() {
        let e = parse_mission("T", "INIT\n  Nonsense(1);\nENDINIT\nWATCH\nENDWATCH\n").unwrap_err();
        assert!(e.message.contains("line 2"), "{e}");
    }
}
