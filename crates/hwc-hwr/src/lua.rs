//! A small reader for the Lua *data* Homeworld 2 keeps its effects and ship tuning in.
//!
//! Effect files (`art/fx/*.lua`), animation lists (`*.anim`) and ship events (`*.events`) are
//! assignments of table constructors; ship files (`*.ship`) are mostly calls such as
//! `setEngineTrail(NewShipType, 0, 5.5, "trail_ribbon.tga", ...)`. This reader evaluates
//! literals, tables, names and number arithmetic, and records top-level assignments and calls.
//! Nothing is executed: control flow, function definitions and anything else it does not
//! understand are skipped to the end of the statement.

use std::collections::BTreeMap;

/// A Lua value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Nil,
    Bool(bool),
    Num(f64),
    Str(String),
    /// An identifier or field path that was not resolved (`NewShipType`, `SOME_CONSTANT`).
    Name(String),
    Table(Table),
}

/// A table constructor: positional items and keyed fields, in source order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Table {
    pub items: Vec<Value>,
    pub fields: Vec<(String, Value)>,
}

impl Table {
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v)
    }
}

impl Value {
    pub fn as_num(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            Value::Bool(b) => Some(f64::from(u8::from(*b))),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_table(&self) -> Option<&Table> {
        match self {
            Value::Table(t) => Some(t),
            _ => None,
        }
    }

    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_table()?.get(key)
    }
}

/// A top-level call statement.
#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub name: String,
    pub args: Vec<Value>,
}

/// Everything a chunk assigned and called at top level.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Chunk {
    /// Assignments by target (`fx`, `NewShipType.mainEngineMaxSpeed`); later ones win.
    pub globals: BTreeMap<String, Value>,
    pub calls: Vec<Call>,
}

impl Chunk {
    pub fn global(&self, name: &str) -> Option<&Value> {
        self.globals
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn calls_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Call> + 'a {
        self.calls
            .iter()
            .filter(move |c| c.name.eq_ignore_ascii_case(name))
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Name(String),
    Num(f64),
    Str(String),
    Sym(&'static str),
}

const SYMS: [&str; 22] = [
    "==", "~=", "<=", ">=", "..", "{", "}", "(", ")", "[", "]", "=", ",", ";", ".", "+", "-", "*",
    "/", "<", ">", ":",
];

fn lex(src: &str) -> Vec<Tok> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if b[i..].starts_with(b"--") {
            i += 2;
            if b[i..].starts_with(b"[[") {
                match src[i..].find("]]") {
                    Some(e) => i += e + 2,
                    None => i = b.len(),
                }
            } else {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
        } else if c == b'"' || c == b'\'' {
            let mut s = Vec::new();
            i += 1;
            while i < b.len() && b[i] != c && b[i] != b'\n' {
                if b[i] == b'\\' && i + 1 < b.len() {
                    i += 1;
                    s.push(match b[i] {
                        b'n' => b'\n',
                        b't' => b'\t',
                        o => o,
                    });
                } else {
                    s.push(b[i]);
                }
                i += 1;
            }
            i += 1;
            out.push(Tok::Str(s.iter().map(|&c| c as char).collect()));
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                if (b[i] == b'e' || b[i] == b'E') && matches!(b.get(i + 1), Some(b'-' | b'+')) {
                    i += 1;
                }
                i += 1;
            }
            out.push(Tok::Num(src[start..i].parse().unwrap_or(0.0)));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push(Tok::Name(src[start..i].to_string()));
        } else if let Some(s) = SYMS.iter().find(|s| b[i..].starts_with(s.as_bytes())) {
            i += s.len();
            out.push(Tok::Sym(s));
        } else {
            i += 1;
        }
    }
    out
}

struct Parser {
    toks: Vec<Tok>,
    pos: usize,
}

const KEYWORDS: [&str; 12] = [
    "if", "for", "while", "function", "local", "return", "do", "repeat", "end", "else", "elseif",
    "until",
];

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.toks.get(self.pos)
    }

    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Some(Tok::Sym(x)) if *x == s)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expr(&mut self) -> Option<Value> {
        let mut v = self.term()?;
        loop {
            if self.eat("+") {
                v = arith(v, self.term()?, |a, b| a + b);
            } else if self.eat("-") {
                v = arith(v, self.term()?, |a, b| a - b);
            } else if self.eat("..") {
                let r = self.term()?;
                v = match (v, r) {
                    (Value::Str(a), Value::Str(b)) => Value::Str(a + &b),
                    (a, _) => a,
                };
            } else {
                return Some(v);
            }
        }
    }

    fn term(&mut self) -> Option<Value> {
        let mut v = self.unary()?;
        loop {
            if self.eat("*") {
                v = arith(v, self.unary()?, |a, b| a * b);
            } else if self.eat("/") {
                v = arith(v, self.unary()?, |a, b| a / b);
            } else {
                return Some(v);
            }
        }
    }

    fn unary(&mut self) -> Option<Value> {
        if self.eat("-") {
            return Some(match self.unary()? {
                Value::Num(n) => Value::Num(-n),
                v => v,
            });
        }
        self.primary()
    }

    fn primary(&mut self) -> Option<Value> {
        let t = self.peek()?.clone();
        self.pos += 1;
        match t {
            Tok::Num(n) => Some(Value::Num(n)),
            Tok::Str(s) => Some(Value::Str(s)),
            Tok::Sym("{") => self.table().map(Value::Table),
            Tok::Sym("(") => {
                let v = self.expr()?;
                self.eat(")").then_some(v)
            }
            Tok::Name(n) => match n.as_str() {
                "nil" => Some(Value::Nil),
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => {
                    let path = self.name_path(n);
                    // A call inside an expression: its value is unknown.
                    if self.is_sym("(") {
                        self.args()?;
                        return Some(Value::Nil);
                    }
                    Some(Value::Name(path))
                }
            },
            _ => None,
        }
    }

    fn name_path(&mut self, mut n: String) -> String {
        while self.is_sym(".") || self.is_sym(":") {
            if let Some(Tok::Name(f)) = self.toks.get(self.pos + 1).cloned() {
                self.pos += 2;
                n.push('.');
                n.push_str(&f);
            } else {
                break;
            }
        }
        n
    }

    fn args(&mut self) -> Option<Vec<Value>> {
        if !self.eat("(") {
            return None;
        }
        let mut args = Vec::new();
        if self.eat(")") {
            return Some(args);
        }
        loop {
            args.push(self.expr()?);
            if self.eat(")") {
                return Some(args);
            }
            if !self.eat(",") {
                return None;
            }
        }
    }

    fn table(&mut self) -> Option<Table> {
        let mut t = Table::default();
        loop {
            if self.eat("}") {
                return Some(t);
            }
            let keyed = match (self.peek(), self.toks.get(self.pos + 1)) {
                (Some(Tok::Name(k)), Some(Tok::Sym("="))) => Some(k.clone()),
                _ => None,
            };
            if let Some(k) = keyed {
                self.pos += 2;
                let v = self.expr()?;
                t.fields.push((k, v));
            } else if self.eat("[") {
                let k = self.expr()?;
                if !self.eat("]") || !self.eat("=") {
                    return None;
                }
                let v = self.expr()?;
                let key = match k {
                    Value::Str(s) => s,
                    Value::Num(n) => format!("{n}"),
                    _ => String::new(),
                };
                t.fields.push((key, v));
            } else {
                t.items.push(self.expr()?);
            }
            if !self.eat(",") && !self.eat(";") && !self.is_sym("}") {
                return None;
            }
        }
    }

    /// One statement at top level. Returns false at the end of input.
    fn statement(&mut self, chunk: &mut Chunk) -> bool {
        let Some(t) = self.peek().cloned() else {
            return false;
        };
        let start = self.pos;
        let ok = match t {
            Tok::Name(n) if !KEYWORDS.contains(&n.as_str()) => {
                self.pos += 1;
                let target = self.name_path(n);
                if self.eat("=") {
                    match self.expr() {
                        Some(v) => {
                            chunk.globals.insert(target, v);
                            true
                        }
                        None => false,
                    }
                } else if self.is_sym("(") {
                    match self.args() {
                        Some(args) => {
                            chunk.calls.push(Call { name: target, args });
                            true
                        }
                        None => false,
                    }
                } else {
                    false
                }
            }
            Tok::Sym(";") => {
                self.pos += 1;
                true
            }
            _ => false,
        };
        if !ok {
            // Skip the token the statement started at (and anything up to a separator), so a
            // construct this reader does not handle never stops the scan.
            self.pos = start + 1;
        }
        true
    }
}

fn arith(a: Value, b: Value, f: impl Fn(f64, f64) -> f64) -> Value {
    match (a.as_num(), b.as_num()) {
        (Some(x), Some(y)) => Value::Num(f(x, y)),
        _ => a,
    }
}

/// Parses a chunk of Lua data.
pub fn parse(src: &str) -> Chunk {
    let mut p = Parser {
        toks: lex(src),
        pos: 0,
    };
    let mut chunk = Chunk::default();
    while p.statement(&mut chunk) {}
    chunk
}

/// Parses bytes (Latin-1, as the files are stored).
pub fn parse_bytes(src: &[u8]) -> Chunk {
    parse(&src.iter().map(|&c| c as char).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_tables() {
        let c = parse(
            "fx = {\nproperties = {\nproperty_00 = {\nname = \"Radius\",\ntype = \"VARTYPE_ARRAY_TIMEFLOAT\",\nvalue = {\n0,\n2,\n1,\n-2.5e-1,\n},\n},\n},\nstyle = \"STYLE_RING\",\n}\n",
        );
        let fx = c.global("fx").unwrap();
        assert_eq!(fx.get("style").unwrap().as_str(), Some("STYLE_RING"));
        let p = fx.get("properties").unwrap().get("property_00").unwrap();
        let v = p.get("value").unwrap().as_table().unwrap();
        assert_eq!(
            v.items
                .iter()
                .map(|v| v.as_num().unwrap())
                .collect::<Vec<_>>(),
            vec![0.0, 2.0, 1.0, -0.25]
        );
    }

    #[test]
    fn anim_file() {
        let c = parse("fps = 20;\r\nstartFrame = 0;\r\nloopCount =01\r\n;\r\ntexture = \r\n{\r\n    \"data:/a.tga\",\r\n    \"data:/b.tga\",\r\n   \r\n}");
        assert_eq!(c.global("fps").unwrap().as_num(), Some(20.0));
        assert_eq!(c.global("loopCount").unwrap().as_num(), Some(1.0));
        assert_eq!(
            c.global("texture").unwrap().as_table().unwrap().items.len(),
            2
        );
    }

    #[test]
    fn ship_calls_and_skips() {
        let c = parse(
            "NewShipType = StartShipConfig()\n-- comment\nNewShipType.mainEngineMaxSpeed=350*1.4\nif x then y() end\nsetEngineTrail(NewShipType,0,3.86,\"trail_ribbon.tga\",0.001,0.5,0.025,2);\n--[[ block\n]]\nfunction f() return 1 end\nsetEngineBurn(NewShipType,24,0.5,1,15,0,0.7,0.1,22);",
        );
        assert!(
            (c.global("NewShipType.mainEngineMaxSpeed")
                .unwrap()
                .as_num()
                .unwrap()
                - 490.0)
                .abs()
                < 1e-9
        );
        let t: Vec<_> = c.calls_named("setEngineTrail").collect();
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].args[0], Value::Name("NewShipType".into()));
        assert_eq!(t[0].args[3].as_str(), Some("trail_ribbon.tga"));
        assert_eq!(c.calls_named("setEngineBurn").count(), 1);
    }
}
