//! The `[expression]` evaluator used by the effect line reader (`Eval.c`), and the
//! number-to-text conversion that substitutes the result back into the line.
//!
//! Grammar (recursive descent, as in the original):
//! `logical := expr (('<'|'<='|'>'|'>='|'=='|'!=') expr)*`,
//! `expr := ['-'] mul (('+'|'-') mul)*`, `mul := factor (('*'|'/') factor)*`,
//! `factor := number | '(' expr ')' | PI | func '(' expr [',' expr] ')'`.
//! The input is upper-cased first, so function names are case-insensitive.

/// Errors the evaluator can report.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvalError {
    Syntax,
    DivZero,
    Range,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Tok {
    Plus,
    Minus,
    Mult,
    Div,
    Open,
    Close,
    Comma,
    Digit,
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Neq,
    Func(Func),
    Error,
    Eol,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Func {
    Acos,
    Asin,
    Atan,
    Atan2,
    Ceil,
    Cos,
    Cosh,
    Exp,
    Fabs,
    Fact,
    Floor,
    Fmod,
    Ldexp,
    Log,
    Log10,
    Pi,
    Pow,
    Rand,
    Rnd,
    Sin,
    Sinh,
    Sqr,
    Sqrt,
    Tan,
    Tanh,
}

fn lookup(name: &str) -> Option<Func> {
    use Func::*;
    Some(match name {
        "ACOS" => Acos,
        "ASIN" => Asin,
        "ATAN" => Atan,
        "ATAN2" => Atan2,
        "CEIL" => Ceil,
        "COS" => Cos,
        "COSH" => Cosh,
        "EXP" => Exp,
        "FABS" => Fabs,
        "FACT" => Fact,
        "FLOOR" => Floor,
        "FMOD" => Fmod,
        "LDEXP" => Ldexp,
        "LOG" => Log,
        "LOG10" => Log10,
        "PI" => Pi,
        "POW" => Pow,
        "RAND" => Rand,
        "RND" => Rnd,
        "SIN" => Sin,
        "SINH" => Sinh,
        "SQR" => Sqr,
        "SQRT" => Sqrt,
        "TAN" => Tan,
        "TANH" => Tanh,
        _ => return None,
    })
}

struct Parser<'a> {
    s: Vec<u8>,
    pc: usize,
    cur: Tok,
    val: f64,
    err: Option<EvalError>,
    random: &'a mut dyn FnMut() -> u32,
}

impl Parser<'_> {
    fn math_err(&mut self, e: EvalError) {
        self.cur = Tok::Error;
        self.err = Some(e);
    }

    fn at(&self, i: usize) -> u8 {
        self.s.get(i).copied().unwrap_or(0)
    }

    fn read_token(&mut self) {
        while self.at(self.pc) != 0 && crate::cfmt::is_c_space(self.at(self.pc)) {
            self.pc += 1;
        }
        let c = self.at(self.pc);
        if c.is_ascii_alphabetic() {
            let start = self.pc;
            while self.at(self.pc).is_ascii_alphanumeric() {
                self.pc += 1;
            }
            let name = String::from_utf8_lossy(&self.s[start..self.pc]).into_owned();
            match lookup(&name) {
                Some(f) => self.cur = Tok::Func(f),
                None => self.math_err(EvalError::Syntax),
            }
            return;
        }
        if c.is_ascii_digit() || c == b'.' {
            let start = self.pc;
            while self.at(self.pc).is_ascii_digit() || self.at(self.pc) == b'.' {
                self.pc += 1;
            }
            if self.at(self.pc) == b'E' {
                self.pc += 1;
                if matches!(self.at(self.pc), b'+' | b'-') {
                    self.pc += 1;
                }
                while self.at(self.pc).is_ascii_digit() {
                    self.pc += 1;
                }
            }
            let text = String::from_utf8_lossy(&self.s[start..self.pc]).into_owned();
            // strtod must consume the whole token.
            let fixed = if text.starts_with('.') { format!("0{text}") } else { text.clone() };
            let fixed = if fixed.ends_with('.') { format!("{fixed}0") } else { fixed };
            match fixed.parse::<f64>() {
                Ok(v) if text.matches('.').count() <= 1 => {
                    self.val = v;
                    self.cur = Tok::Digit;
                }
                _ => self.math_err(EvalError::Syntax),
            }
            return;
        }
        let next = self.at(self.pc + 1);
        self.cur = match c {
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Mult,
            b'/' => Tok::Div,
            b'(' => Tok::Open,
            b')' => Tok::Close,
            b',' => Tok::Comma,
            b'<' if next == b'=' => {
                self.pc += 1;
                Tok::Le
            }
            b'<' => Tok::Lt,
            b'>' if next == b'=' => {
                self.pc += 1;
                Tok::Ge
            }
            b'>' => Tok::Gt,
            b'=' if next == b'=' => {
                self.pc += 1;
                Tok::Eq
            }
            b'!' if next == b'=' => {
                self.pc += 1;
                Tok::Neq
            }
            b'=' | b'!' => {
                self.math_err(EvalError::Syntax);
                return;
            }
            0 => Tok::Eol,
            _ => {
                self.math_err(EvalError::Syntax);
                Tok::Error
            }
        };
        self.pc += 1;
    }

    fn logical(&mut self) -> f64 {
        let mut e1 = self.expression();
        if self.cur == Tok::Error {
            return 0.0;
        }
        while matches!(self.cur, Tok::Lt | Tok::Le | Tok::Gt | Tok::Ge | Tok::Eq | Tok::Neq) {
            let opr = self.cur;
            self.read_token();
            let e2 = self.expression();
            if self.cur == Tok::Error {
                return 0.0;
            }
            let r = match opr {
                Tok::Lt => e1 < e2,
                Tok::Le => e1 <= e2,
                Tok::Gt => e1 > e2,
                Tok::Ge => e1 >= e2,
                Tok::Eq => e1 == e2,
                _ => e1 != e2,
            };
            e1 = if r { 1.0 } else { 0.0 };
        }
        e1
    }

    fn expression(&mut self) -> f64 {
        let mut e = if self.cur == Tok::Minus {
            self.read_token();
            -self.multiply()
        } else {
            self.multiply()
        };
        if self.cur == Tok::Error {
            return 0.0;
        }
        while matches!(self.cur, Tok::Plus | Tok::Minus) {
            let opr = self.cur;
            self.read_token();
            let p = self.multiply();
            if self.cur == Tok::Error {
                return 0.0;
            }
            if opr == Tok::Plus {
                e += p;
            } else {
                e -= p;
            }
        }
        e
    }

    fn multiply(&mut self) -> f64 {
        let mut p1 = self.factor();
        if self.cur == Tok::Error {
            return 0.0;
        }
        while matches!(self.cur, Tok::Mult | Tok::Div) {
            let opr = self.cur;
            self.read_token();
            let p2 = self.factor();
            if self.cur == Tok::Error {
                return 0.0;
            }
            if opr == Tok::Mult {
                p1 *= p2;
            } else if p2 != 0.0 {
                p1 /= p2;
            } else {
                self.math_err(EvalError::DivZero);
                return 0.0;
            }
        }
        p1
    }

    fn expect(&mut self, t: Tok) -> bool {
        if self.cur != t {
            self.math_err(EvalError::Syntax);
            return false;
        }
        true
    }

    fn factor(&mut self) -> f64 {
        if matches!(self.cur, Tok::Eol | Tok::Error) {
            self.cur = Tok::Error;
            if self.err.is_none() {
                self.err = Some(EvalError::Syntax);
            }
            return 0.0;
        }
        let f = match self.cur {
            Tok::Digit => self.val,
            Tok::Open => {
                self.read_token();
                let f = self.expression();
                if self.cur == Tok::Error || !self.expect(Tok::Close) {
                    return 0.0;
                }
                f
            }
            Tok::Func(Func::Pi) => 3.141_592_653_597_89,
            Tok::Func(func @ (Func::Atan2 | Func::Fmod | Func::Ldexp | Func::Pow)) => {
                self.read_token();
                if !self.expect(Tok::Open) {
                    return 0.0;
                }
                self.read_token();
                let mut f = self.expression();
                if self.cur == Tok::Error || !self.expect(Tok::Comma) {
                    return 0.0;
                }
                self.read_token();
                let f2 = self.expression();
                if self.cur == Tok::Error {
                    return 0.0;
                }
                match func {
                    Func::Atan2 => f = f.atan2(f2),
                    Func::Fmod => f %= f2,
                    Func::Ldexp => f *= 2f64.powi(f2 as i32),
                    _ => {
                        if f == 0.0 && f2 <= 0.0 {
                            self.math_err(EvalError::Range);
                            return 0.0;
                        }
                        let ip = f2.trunc();
                        if f < 0.0 && ip != 0.0 {
                            self.math_err(EvalError::Range);
                            return 0.0;
                        }
                        f = f.powf(ip);
                    }
                }
                if !self.expect(Tok::Close) {
                    return 0.0;
                }
                f
            }
            Tok::Func(func) => {
                self.read_token();
                if !self.expect(Tok::Open) {
                    return 0.0;
                }
                self.read_token();
                let mut f = self.expression();
                if self.cur == Tok::Error {
                    return 0.0;
                }
                let range = |p: &mut Self| {
                    p.math_err(EvalError::Range);
                    0.0
                };
                f = match func {
                    Func::Acos if f.abs() <= 1.0 => f.acos(),
                    Func::Asin if f.abs() <= 1.0 => f.asin(),
                    Func::Acos | Func::Asin => return range(self),
                    Func::Atan => f.atan(),
                    Func::Ceil => f.ceil(),
                    Func::Cos => f.cos(),
                    Func::Cosh => f.cosh(),
                    Func::Exp => f.exp(),
                    Func::Fabs => f.abs(),
                    Func::Fact if f > 0.0 => (1..=(f as i32)).fold(1.0, |a, i| a * f64::from(i)),
                    Func::Floor => f.floor(),
                    Func::Log if f > 0.0 => f.ln(),
                    Func::Log10 if f > 0.0 => f.log10(),
                    Func::Fact | Func::Log | Func::Log10 => return range(self),
                    Func::Rand => 1.0 + ((f * f64::from((self.random)())) / f64::from(u32::MAX)).trunc(),
                    Func::Rnd => f * (f64::from((self.random)()) / f64::from(u32::MAX)),
                    Func::Sin => f.sin(),
                    Func::Sinh => f.sinh(),
                    Func::Sqr => f * f,
                    Func::Sqrt if f >= 0.0 => f.sqrt(),
                    Func::Sqrt => return range(self),
                    Func::Tan if f.cos() != 0.0 => f.tan(),
                    Func::Tan => return range(self),
                    Func::Tanh => f.tanh(),
                    _ => f,
                };
                if !self.expect(Tok::Close) {
                    return 0.0;
                }
                f
            }
            _ => {
                self.math_err(EvalError::Syntax);
                return 0.0;
            }
        };
        self.read_token();
        f
    }
}

/// `evalEvaluate`: evaluate an expression. `random` supplies `ranRandom` values for the
/// `RAND`/`RND` functions.
pub fn evaluate(expr: &str, random: &mut dyn FnMut() -> u32) -> (f64, Option<EvalError>) {
    let mut p = Parser {
        s: expr.to_ascii_uppercase().into_bytes(),
        pc: 0,
        cur: Tok::Eol,
        val: 0.0,
        err: None,
        random,
    };
    p.read_token();
    let e = p.logical();
    let err = if p.err.is_none() && p.cur != Tok::Eol { Some(EvalError::Syntax) } else { p.err };
    (e, err)
}

/// `evalNum2Str`: `%f` above 1e12, otherwise `%12.12f` with trailing zeros (and a trailing
/// point) removed.
pub fn num_to_str(n: f64) -> String {
    if n > 1e12 {
        return format!("{n:.6}");
    }
    let mut s = format!("{n:12.12}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(s: &str) -> f64 {
        let mut r = || 0u32;
        let (v, e) = evaluate(s, &mut r);
        assert_eq!(e, None, "{s}");
        v
    }

    #[test]
    fn arithmetic() {
        assert_eq!(ev("255 + 255 * 256 + 255 * 65536 + 255 * 65536 * 256"), 4_294_967_295.0);
        assert_eq!(ev("-1 * (-1.4105 *1000)"), 1410.5);
        assert_eq!(ev("(7 + 4096)"), 4103.0);
        assert_eq!(ev("(1 - 3) / 4"), -0.5);
        assert_eq!(ev("2 < 3"), 1.0);
        assert!((ev("sqrt(16) + pi") - 7.141_592_653_597_89).abs() < 1e-12);
    }

    #[test]
    fn number_text() {
        assert_eq!(num_to_str(4_294_967_295.0), "4294967295");
        assert_eq!(num_to_str(-380.0), "-380");
        assert_eq!(num_to_str(0.25), "0.25");
        assert_eq!(num_to_str(0.0), "0");
        assert_eq!(num_to_str(-0.3), "-0.3");
    }
}
