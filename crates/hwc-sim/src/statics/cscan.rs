//! The value conversions the original keyword setters perform. They all go through the C runtime's
//! `sscanf`, so a malformed value converts as far as it can and leaves the rest of the target
//! untouched. This module reproduces exactly that behaviour for the `%d`, `%f` and `%s`
//! conversions and literal characters, which is all the setters use.

/// One converted field.
#[derive(Debug, Clone, PartialEq)]
pub enum Scanned {
    Int(i32),
    Float(f32),
    Str(String),
}

impl Scanned {
    pub fn int(&self) -> Option<i32> {
        match self {
            Scanned::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn float(&self) -> Option<f32> {
        match self {
            Scanned::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn str(&self) -> Option<&str> {
        match self {
            Scanned::Str(s) => Some(s),
            _ => None,
        }
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

/// Length of the decimal integer prefix (optional sign and at least one digit) of `s`.
fn int_prefix(s: &[u8]) -> usize {
    let sign = usize::from(matches!(s.first(), Some(b'+' | b'-')));
    let digits = s[sign..].iter().take_while(|b| b.is_ascii_digit()).count();
    if digits == 0 {
        0
    } else {
        sign + digits
    }
}

/// Length of the floating-point prefix of `s`: sign, digits, optional fraction, optional exponent
/// (only consumed when it has digits). Zero when there is no digit before the exponent.
fn float_prefix(s: &[u8]) -> usize {
    let mut i = usize::from(matches!(s.first(), Some(b'+' | b'-')));
    let int_digits = s[i..].iter().take_while(|b| b.is_ascii_digit()).count();
    i += int_digits;
    let mut frac_digits = 0;
    if s.get(i) == Some(&b'.') {
        frac_digits = s[i + 1..].iter().take_while(|b| b.is_ascii_digit()).count();
        if int_digits + frac_digits > 0 {
            i += 1 + frac_digits;
        }
    }
    if int_digits + frac_digits == 0 {
        return 0;
    }
    if matches!(s.get(i), Some(b'e' | b'E')) {
        let mut j = i + 1;
        if matches!(s.get(j), Some(b'+' | b'-')) {
            j += 1;
        }
        let exp_digits = s[j.min(s.len())..].iter().take_while(|b| b.is_ascii_digit()).count();
        if exp_digits > 0 {
            i = j + exp_digits;
        }
    }
    i
}

/// Runs a scanf-style format over `input` and returns the fields converted before the first
/// mismatch, like the C function does (its return value is the length of the result).
///
/// Supported directives: `%d`, `%f`, `%s`, `%%`, whitespace (matches any run of whitespace,
/// including none) and literal characters (must match exactly; no whitespace is skipped before
/// them).
pub fn scan(input: &str, format: &str) -> Vec<Scanned> {
    let s = input.as_bytes();
    let f = format.as_bytes();
    let mut out = Vec::new();
    let (mut si, mut fi) = (0usize, 0usize);
    while fi < f.len() {
        let fc = f[fi];
        if is_space(fc) {
            while fi < f.len() && is_space(f[fi]) {
                fi += 1;
            }
            while si < s.len() && is_space(s[si]) {
                si += 1;
            }
            continue;
        }
        if fc != b'%' || f.get(fi + 1) == Some(&b'%') {
            let lit = fc;
            fi += if fc == b'%' { 2 } else { 1 };
            if s.get(si) != Some(&lit) {
                return out;
            }
            si += 1;
            continue;
        }
        let conv = match f.get(fi + 1) {
            Some(&c) => c,
            None => return out,
        };
        fi += 2;
        while si < s.len() && is_space(s[si]) {
            si += 1;
        }
        if si >= s.len() {
            return out;
        }
        let rest = &s[si..];
        match conv {
            b'd' => {
                let n = int_prefix(rest);
                if n == 0 {
                    return out;
                }
                let text = std::str::from_utf8(&rest[..n]).unwrap_or("0");
                // The C runtime accumulates in 32 bits and wraps on overflow.
                let value = text.parse::<i64>().map(|v| v as i32).unwrap_or_else(|_| {
                    let neg = text.starts_with('-');
                    let mut acc: i32 = 0;
                    for b in text.bytes().filter(u8::is_ascii_digit) {
                        acc = acc.wrapping_mul(10).wrapping_add(i32::from(b - b'0'));
                    }
                    if neg {
                        acc.wrapping_neg()
                    } else {
                        acc
                    }
                });
                out.push(Scanned::Int(value));
                si += n;
            }
            b'f' => {
                let n = float_prefix(rest);
                if n == 0 {
                    return out;
                }
                let text = std::str::from_utf8(&rest[..n]).unwrap_or("0");
                // The runtime converts to double and then narrows to the float target.
                let value = text.parse::<f64>().unwrap_or(0.0) as f32;
                out.push(Scanned::Float(value));
                si += n;
            }
            b's' => {
                let n = rest.iter().take_while(|&&b| !is_space(b)).count();
                out.push(Scanned::Str(String::from_utf8_lossy(&rest[..n]).into_owned()));
                si += n;
            }
            _ => return out,
        }
    }
    out
}

/// `sscanf(value, "%f", &target)`: the converted value, or `None` (target unchanged).
pub fn scan_f32(value: &str) -> Option<f32> {
    scan(value, "%f").first().and_then(Scanned::float)
}

/// `sscanf(value, "%d", &target)`.
pub fn scan_i32(value: &str) -> Option<i32> {
    scan(value, "%d").first().and_then(Scanned::int)
}

/// The engine's boolean setter: skips leading blanks and commas; true when the next character is
/// a digit from 1 to 9, or when the remaining text is `TRUE` or `YES` ignoring case. Anything
/// else (including a missing value) is false.
pub fn parse_bool(value: &str) -> bool {
    let rest = value.trim_start_matches([' ', '\t', ',', '\n']);
    match rest.as_bytes().first() {
        Some(b'1'..=b'9') => true,
        _ => rest.eq_ignore_ascii_case("TRUE") || rest.eq_ignore_ascii_case("YES"),
    }
}

/// Replaces commas with blanks and `?` with `0`, as the list setters (`DockPoint`, `NavLight`,
/// `AsteroidBreakInfo`) do before scanning.
pub fn decomma(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            ',' => ' ',
            '?' => '0',
            c => c,
        })
        .collect()
}

/// `"%d,%d,%d"` colour setter: red, green, blue packed as RGBA bytes with alpha 255.
pub fn scan_rgb(value: &str) -> Option<[u8; 4]> {
    let v = scan(value, "%d,%d,%d");
    if v.len() < 3 {
        return None;
    }
    let c = |i: usize| v[i].int().unwrap_or(0) as u8;
    Some([c(0), c(1), c(2), 255])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_like_sscanf() {
        assert_eq!(scan_f32("240.0f"), Some(240.0));
        assert_eq!(scan_f32("  -1.5e2 rest"), Some(-150.0));
        assert_eq!(scan_f32(".5"), Some(0.5));
        assert_eq!(scan_f32("1e"), Some(1.0));
        assert_eq!(scan_f32("TRUE"), None);
        assert_eq!(scan_f32(""), None);
        assert_eq!(scan_i32("12.75"), Some(12));
        assert_eq!(scan_i32("-3"), Some(-3));
        assert_eq!(scan_i32("x3"), None);
    }

    #[test]
    fn formats() {
        let v = scan("Link DPT_Link 185 185 200 0 2", "%s%s%f%f%f%d%d");
        assert_eq!(v.len(), 7);
        assert_eq!(v[0], Scanned::Str("Link".into()));
        assert_eq!(v[6], Scanned::Int(2));
        // Literal commas must follow the number directly.
        assert_eq!(scan("1, 2, 3", "%d,%d,%d").len(), 3);
        assert_eq!(scan("1 ,2", "%d,%d").len(), 1);
        assert_eq!(scan_rgb("157,118,91"), Some([157, 118, 91, 255]));
    }

    #[test]
    fn booleans() {
        assert!(parse_bool("TRUE"));
        assert!(parse_bool("yes"));
        assert!(parse_bool("1"));
        assert!(parse_bool(" ,7"));
        assert!(!parse_bool("0"));
        assert!(!parse_bool("FALSE"));
        assert!(!parse_bool("TRUEISH"));
        assert!(!parse_bool(""));
    }
}
