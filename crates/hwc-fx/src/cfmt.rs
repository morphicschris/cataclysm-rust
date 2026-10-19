//! Small re-implementations of the C library behaviour the effect compiler leans on.
//!
//! The original parser is built from `strtok`, `sscanf("%d")`, `sscanf("%f")`, `strchr` and a
//! handful of hand-written scanners. Their quirks (wrapping integer overflow, `strtok`
//! swallowing a leading minus sign when `-` is a delimiter, prefix parsing) are visible in the
//! compiled effects, so they are reproduced here rather than replaced with idiomatic parsing.

use hwc_math::fpu::{round_magic, Ext};

/// The characters `etgParseNamesMatch` treats as part of an identifier (sic: the original
/// table lists `6` twice and has no gap).
pub const LABEL_CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz12345667890-_{}#";

/// Characters accepted by `etgIsNumber`.
const NUMBER_CHARS: &[u8] = b"0123456789.-";

/// `isspace` in the C locale.
pub fn is_c_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// `etgIsNumber`: every character is a digit, `.` or `-` (so `""`, `"-"` and `"1-2"` count).
pub fn is_number(s: &str) -> bool {
    s.bytes().all(|c| NUMBER_CHARS.contains(&c))
}

/// `etgParseNamesMatch`: length of `name` when the identifier prefix of `s` equals the
/// identifier prefix of `name`, else 0.
pub fn names_match(s: &str, name: &str) -> usize {
    let count = |t: &str| t.bytes().take_while(|c| LABEL_CHARS.contains(c)).count();
    let (cs, cn) = (count(s), count(name));
    if cs != cn || s.as_bytes()[..cn] != name.as_bytes()[..cn] {
        return 0;
    }
    cn
}

/// `sscanf(s, "%d", &x)` as the MSVC6 runtime does it: leading white space, optional sign,
/// digits accumulated with silent 32-bit wrap-around. `None` when no digit was read.
pub fn scan_int(s: &str) -> Option<u32> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && is_c_space(b[i]) {
        i += 1;
    }
    let mut neg = false;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        neg = b[i] == b'-';
        i += 1;
    }
    let start = i;
    let mut v: u32 = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        v = v.wrapping_mul(10).wrapping_add(u32::from(b[i] - b'0'));
        i += 1;
    }
    if i == start {
        return None;
    }
    Some(if neg { v.wrapping_neg() } else { v })
}

/// The longest prefix of `s` (after white space) that `%f` would consume, converted to a
/// double. `None` when there is no mantissa digit.
pub fn scan_double(s: &str) -> Option<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && is_c_space(b[i]) {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let mut digits = 0;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
        digits += 1;
    }
    if i < b.len() && b[i] == b'.' {
        i += 1;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
            digits += 1;
        }
    }
    if digits == 0 {
        return None;
    }
    let mut end = i;
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let ds = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > ds {
            end = j;
        }
    }
    let mut text = s[start..end].to_string();
    if text.ends_with('.') {
        text.push('0');
    }
    if text.starts_with('.') || text.starts_with("-.") || text.starts_with("+.") {
        text = text.replacen('.', "0.", 1);
    }
    text.parse::<f64>().ok()
}

/// `sscanf(s, "%f", &f)`: the scanned double rounded to a `float`.
pub fn scan_float(s: &str) -> Option<f32> {
    scan_double(s).map(|d| d as f32)
}

/// `sscanf(s, "%f", ...)` into a dword: the float's bit pattern.
pub fn scan_float_bits(s: &str) -> Option<u32> {
    scan_float(s).map(f32::to_bits)
}

/// A `strtok` emulation over an owned string. Each call may use different delimiters, as
/// the C function allows.
pub struct StrTok {
    s: Vec<u8>,
    pos: usize,
}

impl StrTok {
    pub fn new(s: &str) -> Self {
        Self { s: s.as_bytes().to_vec(), pos: 0 }
    }

    /// Next token, or `None` at the end of the string.
    pub fn next_tok(&mut self, delims: &str) -> Option<String> {
        let d = delims.as_bytes();
        while self.pos < self.s.len() && d.contains(&self.s[self.pos]) {
            self.pos += 1;
        }
        if self.pos >= self.s.len() {
            return None;
        }
        let start = self.pos;
        while self.pos < self.s.len() && !d.contains(&self.s[self.pos]) {
            self.pos += 1;
        }
        let tok = String::from_utf8_lossy(&self.s[start..self.pos]).into_owned();
        if self.pos < self.s.len() {
            self.pos += 1; // the delimiter is overwritten with a NUL and skipped
        }
        Some(tok)
    }
}

/// The game's `colRealToUbyte`: `x * 256` rounded to nearest (the `1.5 * 2^52` trick), with
/// results of 256 or more clamped to 255. Negative inputs wrap through the low byte.
pub fn col_real_to_ubyte(x: Ext) -> u32 {
    let v = round_magic(x * 256.0);
    if v < 0x100 {
        (v as u32) & 0xff
    } else {
        0xff
    }
}

/// The single-precision constant `1/255` the game multiplies colour bytes by.
pub const ONE_OVER_255: f32 = 0.003_921_569;

/// `colUbyteToReal`: a colour byte as a register value.
pub fn col_ubyte_to_real(b: u32) -> Ext {
    f64::from(b & 0xff) * f64::from(ONE_OVER_255)
}

/// Red, green, blue and alpha bytes of a packed colour (`colRGBA` layout).
pub fn col_channels(c: u32) -> [u32; 4] {
    [c & 0xff, (c >> 8) & 0xff, (c >> 16) & 0xff, c >> 24]
}

/// `colRGBA`.
pub fn col_rgba(r: u32, g: u32, b: u32, a: u32) -> u32 {
    (r & 0xff) | ((g & 0xff) << 8) | ((b & 0xff) << 16) | ((a & 0xff) << 24)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int_scan_wraps_like_msvc() {
        assert_eq!(scan_int("4294967295"), Some(0xffff_ffff));
        assert_eq!(scan_int(" -3x"), Some((-3i32) as u32));
        assert_eq!(scan_int("0.7"), Some(0));
        assert_eq!(scan_int("x"), None);
        assert_eq!(scan_int("42798743551"), Some(4_144_037_887));
    }

    #[test]
    fn float_scan_takes_prefix() {
        assert_eq!(scan_float("3;"), Some(3.0));
        assert_eq!(scan_float("-1.5e1z"), Some(-15.0));
        assert_eq!(scan_float(".5"), Some(0.5));
        assert_eq!(scan_float("1e"), Some(1.0));
        assert_eq!(scan_float("-"), None);
    }

    #[test]
    fn strtok_drops_leading_minus_when_it_is_a_delimiter() {
        let mut t = StrTok::new("speed <- -200");
        assert_eq!(t.next_tok(" \t<-").as_deref(), Some("speed"));
        assert_eq!(t.next_tok(" \t<-").as_deref(), Some("200"));
        assert_eq!(t.next_tok(" \t<-"), None);
    }

    #[test]
    fn names_match_uses_identifier_prefix() {
        assert_eq!(names_match("if(length == 0", "if"), 2);
        assert_eq!(names_match("eachFrame", "eachFrame"), 9);
        assert_eq!(names_match("at(0.5", "alt"), 0);
        assert_eq!(names_match("}", "}"), 1);
    }

    #[test]
    fn colour_rounding() {
        assert_eq!(col_real_to_ubyte(1.0), 255);
        assert_eq!(col_real_to_ubyte(0.5), 128);
        assert_eq!(col_real_to_ubyte(0.0), 0);
    }
}
