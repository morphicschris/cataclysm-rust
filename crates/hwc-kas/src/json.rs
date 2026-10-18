//! Loader for the structured JSON form of a mission (dev path).
//!
//! The JSON is produced from the machine code by `crates/hwc-kas/tools/kas_ir_export.py`, which
//! lowers the AST of `tools/scripts/kas_decompile.py` to the IR shape. It is exact (unlike the
//! `.kas` text): it carries IFONCE guard hashes, unsigned comparisons, switch statements and the
//! `G_` bit of every hash. Shape (all hashes and addresses are 8-digit hex strings):
//!
//! ```text
//! { "format": "hwc-kas-ir", "version": 1, "mission": "Mission01", "name_hash": "087e6ef1",
//!   "lstrings": [{"addr", "name", "texts": [en, fr, de]}], "names": {hash: name},
//!   "fsms": [{"hash", "init": block, "watch": block, "states": [{"hash", "init", "watch"}]}],
//!   "init": block, "watch": block }
//! block = {"addr", "scope", "stmts": [stmt]}
//! stmt  = ["call", index, [value]] | ["if", cond, [stmt], [stmt]]
//!       | ["ifonce", hash, cond|null, [stmt], [stmt]] | ["switch", value, [[[int], [stmt]]], [stmt]|null]
//!       | ["msgloop", [[[hash], [stmt]]], [stmt]|null] | ["jump", hash] | ["fsmcreate", hash, value]
//!       | ["return"]
//! cond  = ["and"|"or", cond, cond] | ["not", cond] | ["true"] | ["false"] | ["cmp", op, value, value]
//! value = ["int", n] | ["str", s] | ["lstring", addr] | ["call", index, [value]]
//!       | ["bin", op, value, value] | ["un", "-"|"~", value] | ["cond", cond]
//!       | ["label", KIND, hash] | ["this", "team"|"teamships"|"teamspoint"]
//! ```

use std::collections::{BTreeMap, HashMap};

use crate::functions::{FuncId, ParamType, FUNCTIONS};
use crate::ir::*;
use crate::{Error, Hash};

/// A parsed JSON value.
#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` / `false`.
    Bool(bool),
    /// A number (integers are exact up to 2^53).
    Num(f64),
    /// A string.
    Str(String),
    /// An array.
    Arr(Vec<Json>),
    /// An object (key order kept).
    Obj(Vec<(String, Json)>),
}

impl Json {
    /// Parses a JSON document.
    pub fn parse(text: &str) -> Result<Json, Error> {
        let mut p = JsonParser { s: text.as_bytes(), i: 0 };
        p.ws();
        let v = p.value(0)?;
        p.ws();
        if p.i != p.s.len() {
            return Err(p.err("trailing characters"));
        }
        Ok(v)
    }

    /// Object member.
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Json::Obj(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn arr(&self) -> Result<&[Json], Error> {
        match self {
            Json::Arr(a) => Ok(a),
            _ => Err(Error::new(format!("expected an array, got {}", self.kind()))),
        }
    }

    fn str(&self) -> Result<&str, Error> {
        match self {
            Json::Str(s) => Ok(s),
            _ => Err(Error::new(format!("expected a string, got {}", self.kind()))),
        }
    }

    fn num(&self) -> Result<f64, Error> {
        match self {
            Json::Num(n) => Ok(*n),
            _ => Err(Error::new(format!("expected a number, got {}", self.kind()))),
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            Json::Null => "null",
            Json::Bool(_) => "a boolean",
            Json::Num(_) => "a number",
            Json::Str(_) => "a string",
            Json::Arr(_) => "an array",
            Json::Obj(_) => "an object",
        }
    }
}

struct JsonParser<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonParser<'_> {
    fn err(&self, m: &str) -> Error {
        Error::new(format!("JSON: {m} at byte {}", self.i))
    }

    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\r' | b'\n') {
            self.i += 1;
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, Error> {
        if depth > 512 {
            return Err(self.err("nesting too deep"));
        }
        match self.s.get(self.i) {
            None => Err(self.err("unexpected end")),
            Some(b'{') => {
                self.i += 1;
                let mut m = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b'}') {
                    self.i += 1;
                    return Ok(Json::Obj(m));
                }
                loop {
                    self.ws();
                    let k = self.string()?;
                    self.ws();
                    if self.s.get(self.i) != Some(&b':') {
                        return Err(self.err("expected ':'"));
                    }
                    self.i += 1;
                    self.ws();
                    let v = self.value(depth + 1)?;
                    m.push((k, v));
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b'}') => {
                            self.i += 1;
                            return Ok(Json::Obj(m));
                        }
                        _ => return Err(self.err("expected ',' or '}'")),
                    }
                }
            }
            Some(b'[') => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s.get(self.i) == Some(&b']') {
                    self.i += 1;
                    return Ok(Json::Arr(a));
                }
                loop {
                    self.ws();
                    a.push(self.value(depth + 1)?);
                    self.ws();
                    match self.s.get(self.i) {
                        Some(b',') => self.i += 1,
                        Some(b']') => {
                            self.i += 1;
                            return Ok(Json::Arr(a));
                        }
                        _ => return Err(self.err("expected ',' or ']'")),
                    }
                }
            }
            Some(b'"') => Ok(Json::Str(self.string()?)),
            Some(b't') if self.s[self.i..].starts_with(b"true") => {
                self.i += 4;
                Ok(Json::Bool(true))
            }
            Some(b'f') if self.s[self.i..].starts_with(b"false") => {
                self.i += 5;
                Ok(Json::Bool(false))
            }
            Some(b'n') if self.s[self.i..].starts_with(b"null") => {
                self.i += 4;
                Ok(Json::Null)
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let st = self.i;
                self.i += 1;
                while self.i < self.s.len() && matches!(self.s[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.s[st..self.i]).map_err(|_| self.err("bad number"))?;
                t.parse::<f64>().map(Json::Num).map_err(|_| self.err("bad number"))
            }
            Some(_) => Err(self.err("unexpected character")),
        }
    }

    fn string(&mut self) -> Result<String, Error> {
        if self.s.get(self.i) != Some(&b'"') {
            return Err(self.err("expected a string"));
        }
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.s.get(self.i).ok_or_else(|| self.err("unterminated string"))?;
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or_else(|| self.err("unterminated escape"))?;
                    self.i += 1;
                    match e {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut cp = self.hex4()?;
                            if (0xD800..0xDC00).contains(&cp) && self.s[self.i..].starts_with(b"\\u") {
                                self.i += 2;
                                let lo = self.hex4()?;
                                cp = 0x10000 + ((cp - 0xD800) << 10) + (lo.wrapping_sub(0xDC00) & 0x3ff);
                            }
                            let ch = char::from_u32(cp).unwrap_or('\u{fffd}');
                            let mut buf = [0u8; 4];
                            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                        }
                        _ => return Err(self.err("bad escape")),
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8(out).map_err(|_| self.err("invalid UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32, Error> {
        let h = self.s.get(self.i..self.i + 4).ok_or_else(|| self.err("short \\u escape"))?;
        let t = std::str::from_utf8(h).map_err(|_| self.err("bad \\u escape"))?;
        self.i += 4;
        u32::from_str_radix(t, 16).map_err(|_| self.err("bad \\u escape"))
    }
}

/// Loads a mission from the exporter's JSON.
pub fn load_mission(text: &str) -> Result<Mission, Error> {
    let root = Json::parse(text)?;
    let field = |k: &str| root.get(k).ok_or_else(|| Error::new(format!("missing \"{k}\"")));
    if field("format")?.str()? != "hwc-kas-ir" {
        return Err(Error::new("not a hwc-kas-ir document"));
    }
    let version = field("version")?.num()?;
    if version != 1.0 {
        return Err(Error::new(format!("unsupported hwc-kas-ir version {version}")));
    }
    let name = field("mission")?.str()?.to_string();
    let name_hash = hex(field("name_hash")?)?;

    let mut lstrings = Vec::new();
    let mut lstring_by_addr = HashMap::new();
    for (i, l) in field("lstrings")?.arr()?.iter().enumerate() {
        let addr = hex(l.get("addr").ok_or_else(|| Error::new("lstring without addr"))?)?;
        let texts = l.get("texts").ok_or_else(|| Error::new("lstring without texts"))?.arr()?;
        let t = |k: usize| texts.get(k).map(|v| v.str().map(str::to_string)).unwrap_or(Ok(String::new()));
        lstrings.push(LString {
            name: l.get("name").and_then(|n| n.str().ok()).map(str::to_string),
            addr: Some(addr),
            texts: [t(0)?, t(1)?, t(2)?],
        });
        lstring_by_addr.insert(addr, i);
    }

    let mut names = BTreeMap::new();
    if let Some(Json::Obj(m)) = root.get("names") {
        for (k, v) in m {
            let h = u32::from_str_radix(k, 16).map_err(|_| Error::new(format!("bad hash key {k}")))?;
            names.insert(h, v.str()?.to_string());
        }
    }

    // FSM hashes first (FSMCreate can refer forward).
    let fsms_json = field("fsms")?.arr()?;
    let mut fsm_index = HashMap::new();
    for (i, f) in fsms_json.iter().enumerate() {
        fsm_index.insert(hex(f.get("hash").ok_or_else(|| Error::new("fsm without hash"))?)?, i);
    }
    let cx = Lower { lstrings: &lstring_by_addr, fsms: &fsm_index };

    let mut fsms = Vec::new();
    for f in fsms_json {
        let hash = hex(f.get("hash").unwrap_or(&Json::Null))?;
        let states_json = f.get("states").map(Json::arr).transpose()?.unwrap_or(&[]);
        let mut state_index = HashMap::new();
        for (i, s) in states_json.iter().enumerate() {
            state_index.insert(hex(s.get("hash").ok_or_else(|| Error::new("state without hash"))?)?, i);
        }
        let ctx = |kind: &str| format!("FSM {hash:08x} {kind}");
        let mut states = Vec::new();
        for s in states_json {
            let sh = hex(s.get("hash").unwrap_or(&Json::Null))?;
            states.push(State {
                hash: sh,
                init: cx.block(s.get("init"), Some(&state_index)).map_err(|e| e.context(&ctx(&format!("state {sh:08x} INIT"))))?,
                watch: cx.block(s.get("watch"), Some(&state_index)).map_err(|e| e.context(&ctx(&format!("state {sh:08x} WATCH"))))?,
            });
        }
        fsms.push(Fsm {
            hash,
            init: cx.block(f.get("init"), Some(&state_index)).map_err(|e| e.context(&ctx("INIT")))?,
            watch: cx.block(f.get("watch"), Some(&state_index)).map_err(|e| e.context(&ctx("WATCH")))?,
            states,
        });
    }
    let init = cx.block(root.get("init"), None).map_err(|e| e.context("mission INIT"))?;
    let watch = cx.block(root.get("watch"), None).map_err(|e| e.context("mission WATCH"))?;
    Ok(Mission { name, name_hash, lstrings, fsms, init, watch, names, fidelity: Fidelity::Exact })
}

fn hex(v: &Json) -> Result<u32, Error> {
    let s = v.str()?;
    u32::from_str_radix(s, 16).map_err(|_| Error::new(format!("bad hex value {s:?}")))
}

fn int(v: &Json) -> Result<i32, Error> {
    let n = v.num()?;
    if n.fract() != 0.0 || n < f64::from(i32::MIN) || n > f64::from(u32::MAX) {
        return Err(Error::new(format!("bad integer {n}")));
    }
    Ok(n as i64 as i32)
}

struct Lower<'a> {
    lstrings: &'a HashMap<u32, usize>,
    fsms: &'a HashMap<Hash, usize>,
}

type StateMap = HashMap<Hash, usize>;

impl Lower<'_> {
    fn block(&self, b: Option<&Json>, states: Option<&StateMap>) -> Result<Block, Error> {
        let b = b.ok_or_else(|| Error::new("missing block"))?;
        let addr = match b.get("addr") {
            Some(a @ Json::Str(_)) => Some(hex(a)?),
            _ => None,
        };
        let stmts = self.stmts(b.get("stmts").ok_or_else(|| Error::new("block without stmts"))?, states)?;
        Ok(Block { addr, stmts })
    }

    fn stmts(&self, v: &Json, states: Option<&StateMap>) -> Result<Vec<Stmt>, Error> {
        v.arr()?.iter().map(|s| self.stmt(s, states)).collect()
    }

    fn stmt(&self, s: &Json, states: Option<&StateMap>) -> Result<Stmt, Error> {
        let a = s.arr()?;
        let tag = a.first().ok_or_else(|| Error::new("empty statement"))?.str()?;
        let at = |i: usize| a.get(i).ok_or_else(|| Error::new(format!("short \"{tag}\" statement")));
        Ok(match tag {
            "call" => Stmt::Call(self.call(int(at(1)?)?, at(2)?)?),
            "if" => Stmt::If {
                cond: self.cond(at(1)?)?,
                then: self.stmts(at(2)?, states)?,
                otherwise: self.stmts(at(3)?, states)?,
            },
            "ifonce" => Stmt::IfOnce {
                var: hex(at(1)?)?,
                cond: match at(2)? {
                    Json::Null => None,
                    c => Some(self.cond(c)?),
                },
                body: self.stmts(at(3)?, states)?,
                otherwise: self.stmts(at(4)?, states)?,
            },
            "switch" => {
                let mut arms = Vec::new();
                for arm in at(2)?.arr()? {
                    let arm = arm.arr()?;
                    let vals = arm.first().ok_or_else(|| Error::new("bad switch arm"))?.arr()?;
                    arms.push(SwitchArm {
                        values: vals.iter().map(int).collect::<Result<_, _>>()?,
                        body: self.stmts(arm.get(1).ok_or_else(|| Error::new("bad switch arm"))?, states)?,
                    });
                }
                Stmt::Switch {
                    value: self.expr(at(1)?)?,
                    arms,
                    default: match at(3)? {
                        Json::Null => None,
                        d => Some(self.stmts(d, states)?),
                    },
                }
            }
            "msgloop" => {
                let mut handlers = Vec::new();
                for h in at(1)?.arr()? {
                    let h = h.arr()?;
                    let msgs = h.first().ok_or_else(|| Error::new("bad message handler"))?.arr()?;
                    handlers.push(MsgHandler {
                        msgs: msgs.iter().map(hex).collect::<Result<_, _>>()?,
                        body: self.stmts(h.get(1).ok_or_else(|| Error::new("bad message handler"))?, states)?,
                    });
                }
                Stmt::MsgLoop {
                    handlers,
                    default: match at(2)? {
                        Json::Null => None,
                        d => Some(self.stmts(d, states)?),
                    },
                }
            }
            "jump" => {
                let hash = hex(at(1)?)?;
                let states = states.ok_or_else(|| Error::new("JUMP outside an FSM"))?;
                let state = *states.get(&hash).ok_or_else(|| Error::new(format!("JUMP to unknown state {hash:08x}")))?;
                Stmt::Jump { state, hash }
            }
            "fsmcreate" => {
                let hash = hex(at(1)?)?;
                let fsm = *self.fsms.get(&hash).ok_or_else(|| Error::new(format!("FSMCreate of unknown FSM {hash:08x}")))?;
                Stmt::FsmCreate { fsm, team: self.team(at(2)?)? }
            }
            "return" => Stmt::Return,
            _ => return Err(Error::new(format!("unknown statement \"{tag}\""))),
        })
    }

    fn call(&self, index: i32, args: &Json) -> Result<Call, Error> {
        let info = usize::try_from(index)
            .ok()
            .and_then(|i| FUNCTIONS.get(i))
            .ok_or_else(|| Error::new(format!("bad function index {index}")))?;
        let args = args.arr()?;
        if args.len() != info.params.len() {
            return Err(Error::new(format!("{}: {} arguments, expected {}", info.kas, args.len(), info.params.len())));
        }
        let mut out = Vec::with_capacity(args.len());
        for (a, &(pname, pt)) in args.iter().zip(info.params) {
            out.push(self.arg(a, pt).map_err(|e| e.context(&format!("{}({pname})", info.kas)))?);
        }
        Ok(Call { func: FuncId(index as u16), args: out })
    }

    fn arg(&self, v: &Json, pt: ParamType) -> Result<Arg, Error> {
        let a = v.arr()?;
        let tag = a.first().ok_or_else(|| Error::new("empty value"))?.str()?;
        let int0 = || -> Result<Option<i32>, Error> { if tag == "int" { Ok(Some(int(&a[1])?)) } else { Ok(None) } };
        let null_or = |what: &str| -> Result<(), Error> {
            match int0()? {
                Some(0) => Ok(()),
                _ => Err(Error::new(format!("{what} argument expected, got {tag}"))),
            }
        };
        Ok(match pt {
            ParamType::Int | ParamType::Bool => Arg::Int(self.expr(v)?),
            ParamType::Hash => match int0()? {
                Some(n) => Arg::Hash(n as u32),
                None => return Err(Error::new(format!("hash argument must be a constant, got {tag}"))),
            },
            ParamType::Str => match tag {
                "str" => Arg::Str(StrRef::Text(a[1].str()?.to_string())),
                "lstring" => {
                    let addr = hex(&a[1])?;
                    Arg::Str(StrRef::Local(
                        *self.lstrings.get(&addr).ok_or_else(|| Error::new(format!("unknown LSTRING table {addr:08x}")))?,
                    ))
                }
                "int" => match int(&a[1])? {
                    0 => Arg::Str(StrRef::Null),
                    n => Arg::Str(StrRef::Int(n)),
                },
                _ => return Err(Error::new(format!("string argument expected, got {tag}"))),
            },
            ParamType::Team => Arg::Team(self.team(v)?),
            ParamType::Ships => Arg::Ships(match (tag, a.get(1).and_then(|x| x.str().ok())) {
                ("label", Some("SHIPS")) => ShipsRef::Selection(hex(&a[2])?),
                ("label", Some("TEAMSHIPS")) => ShipsRef::Team(TeamRef::Label(hex(&a[2])?)),
                ("this", Some("teamships")) => ShipsRef::Team(TeamRef::This),
                _ => {
                    null_or("selection")?;
                    ShipsRef::Null
                }
            }),
            ParamType::Point => Arg::Point(match (tag, a.get(1).and_then(|x| x.str().ok())) {
                ("label", Some("POINT")) => PointRef::Point(hex(&a[2])?),
                ("label", Some("SHIPSPOINT")) => PointRef::Ships(hex(&a[2])?),
                ("label", Some("TEAMSPOINT")) => PointRef::Team(TeamRef::Label(hex(&a[2])?)),
                ("label", Some("VOLUMEPOINT")) => PointRef::Volume(hex(&a[2])?),
                ("this", Some("teamspoint")) => PointRef::Team(TeamRef::This),
                _ => {
                    null_or("point")?;
                    PointRef::Null
                }
            }),
            ParamType::Volume => Arg::Volume(match (tag, a.get(1).and_then(|x| x.str().ok())) {
                ("label", Some("VOLUME")) => Some(hex(&a[2])?),
                _ => {
                    null_or("volume")?;
                    None
                }
            }),
            ParamType::Path => Arg::Path(match (tag, a.get(1).and_then(|x| x.str().ok())) {
                ("label", Some("PATH")) => Some(hex(&a[2])?),
                _ => {
                    null_or("path")?;
                    None
                }
            }),
        })
    }

    fn team(&self, v: &Json) -> Result<TeamRef, Error> {
        let a = v.arr()?;
        let tag = a.first().ok_or_else(|| Error::new("empty value"))?.str()?;
        match (tag, a.get(1).and_then(|x| x.str().ok())) {
            ("label", Some("TEAM")) => Ok(TeamRef::Label(hex(&a[2])?)),
            ("this", Some("team")) => Ok(TeamRef::This),
            ("int", _) if int(&a[1])? == 0 => Ok(TeamRef::Null),
            _ => Err(Error::new(format!("team argument expected, got {tag}"))),
        }
    }

    fn expr(&self, v: &Json) -> Result<Expr, Error> {
        let a = v.arr()?;
        let tag = a.first().ok_or_else(|| Error::new("empty value"))?.str()?;
        let at = |i: usize| a.get(i).ok_or_else(|| Error::new(format!("short \"{tag}\" value")));
        Ok(match tag {
            "int" => Expr::Int(int(at(1)?)?),
            "call" => {
                let c = self.call(int(at(1)?)?, at(2)?)?;
                if !c.func.info().returns {
                    return Err(Error::new(format!("{} returns no value", c.func)));
                }
                Expr::Call(Box::new(c))
            }
            "bin" => {
                let op = match at(1)?.str()? {
                    "+" => BinOp::Add,
                    "-" => BinOp::Sub,
                    "*" => BinOp::Mul,
                    "/" => BinOp::Div,
                    "%" => BinOp::Rem,
                    "&" => BinOp::And,
                    "|" => BinOp::Or,
                    "^" => BinOp::Xor,
                    "<<" => BinOp::Shl,
                    ">>" => BinOp::Shr,
                    o => return Err(Error::new(format!("unknown operator {o}"))),
                };
                Expr::Bin(op, Box::new(self.expr(at(2)?)?), Box::new(self.expr(at(3)?)?))
            }
            "un" => match at(1)?.str()? {
                "-" => Expr::Neg(Box::new(self.expr(at(2)?)?)),
                "~" => Expr::BitNot(Box::new(self.expr(at(2)?)?)),
                o => return Err(Error::new(format!("unknown unary operator {o}"))),
            },
            "cond" => Expr::Cond(Box::new(self.cond(at(1)?)?)),
            _ => return Err(Error::new(format!("integer expression expected, got {tag}"))),
        })
    }

    fn cond(&self, v: &Json) -> Result<Cond, Error> {
        let a = v.arr()?;
        let tag = a.first().ok_or_else(|| Error::new("empty condition"))?.str()?;
        let at = |i: usize| a.get(i).ok_or_else(|| Error::new(format!("short \"{tag}\" condition")));
        Ok(match tag {
            "true" => Cond::True,
            "false" => Cond::False,
            "not" => Cond::Not(Box::new(self.cond(at(1)?)?)),
            "and" => Cond::And(Box::new(self.cond(at(1)?)?), Box::new(self.cond(at(2)?)?)),
            "or" => Cond::Or(Box::new(self.cond(at(1)?)?), Box::new(self.cond(at(2)?)?)),
            "cmp" => {
                let op = match at(1)?.str()? {
                    "==" => CmpOp::Eq,
                    "!=" => CmpOp::Ne,
                    "<" => CmpOp::Lt,
                    "<=" => CmpOp::Le,
                    ">" => CmpOp::Gt,
                    ">=" => CmpOp::Ge,
                    "u<" => CmpOp::ULt,
                    "u<=" => CmpOp::ULe,
                    "u>" => CmpOp::UGt,
                    "u>=" => CmpOp::UGe,
                    o => return Err(Error::new(format!("unknown comparison {o}"))),
                };
                Cond::Cmp(op, self.expr(at(2)?)?, self.expr(at(3)?)?)
            }
            _ => return Err(Error::new(format!("condition expected, got {tag}"))),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_json() {
        let v = Json::parse(r#"{"a": [1, -2.5, "x\"é", null, true], "b": {}}"#).unwrap();
        assert_eq!(
            v.get("a"),
            Some(&Json::Arr(vec![
                Json::Num(1.0),
                Json::Num(-2.5),
                Json::Str("x\"é".into()),
                Json::Null,
                Json::Bool(true)
            ]))
        );
        assert!(Json::parse("[1,]").is_err());
    }

    #[test]
    fn loads_small_mission() {
        let doc = r#"{"format":"hwc-kas-ir","version":1,"mission":"Test","name_hash":"00005678",
          "lstrings":[{"addr":"00001000","name":"Hi","texts":["Hi","Salut","Hallo"]}],
          "names":{"0000000a":"Alpha"},
          "fsms":[{"hash":"0000000a","init":{"addr":"00000010","stmts":[["jump","0000000b"]]},
                   "watch":{"stmts":[]},
                   "states":[{"hash":"0000000b","init":{"stmts":[["call",11,[["int",1234],["int",5]]]]},
                              "watch":{"stmts":[["if",["cmp","==",["call",14,[["int",-2147483648]]],["int",1]],
                                                  [["jump","0000000b"]],[]]]}}]}],
          "init":{"stmts":[["fsmcreate","0000000a",["label","TEAM","0000000a"]]]},
          "watch":{"stmts":[]}}"#;
        let m = load_mission(doc).unwrap();
        assert_eq!(m.fsms.len(), 1);
        assert_eq!(m.fsms[0].init.stmts, vec![Stmt::Jump { state: 0, hash: 0xb }]);
        assert_eq!(m.init.stmts, vec![Stmt::FsmCreate { fsm: 0, team: TeamRef::Label(0xa) }]);
        assert_eq!(m.fsms[0].init.addr, Some(0x10));
        match &m.fsms[0].states[0].watch.stmts[0] {
            Stmt::If { cond: Cond::Cmp(CmpOp::Eq, Expr::Call(c), Expr::Int(1)), .. } => {
                assert_eq!(c.args, vec![Arg::Hash(0x8000_0000)]);
            }
            s => panic!("{s:?}"),
        }
    }
}
