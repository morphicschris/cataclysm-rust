//! Tick-by-tick comparison of two state streams.
//!
//! Frames are paired in order (frame *i* of one stream with frame *i* of the other; the update
//! counter is one of the compared fields), ships are paired by ship number. Every field is
//! compared exactly: integers by value, floats by bit pattern (so `-0.0 != 0.0` and NaNs with
//! different payloads differ), unless [`CompareOptions::float_ulps`] allows some slack. The
//! report gives the first frame that differs with every difference in it (up to a limit),
//! each with the value it had one frame earlier for context.

use std::collections::HashMap;
use std::fmt;

use crate::state::{ShipState, StateFrame, StateStream};

/// What to compare.
#[derive(Debug, Clone)]
pub struct CompareOptions {
    /// Random streams left out. Default: stream 0, which the original seeds from the clock at
    /// start-up and which only feeds non-simulation code.
    pub ignore_streams: Vec<usize>,
    /// Compare `univ_time`.
    pub compare_univ_time: bool,
    /// Allowed distance in units in the last place for float fields; 0 = bit-exact.
    pub float_ulps: u32,
    /// Most differences listed for the diverging frame.
    pub max_diffs: usize,
}

impl Default for CompareOptions {
    fn default() -> Self {
        Self {
            ignore_streams: vec![0],
            compare_univ_time: true,
            float_ulps: 0,
            max_diffs: 64,
        }
    }
}

/// A compared value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(u64),
    Float(f32),
    Bytes(Vec<u8>),
    Ships(Vec<u16>),
    Absent,
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Int(v) => write!(f, "{v} ({v:#x})"),
            Value::Float(v) => write!(f, "{v:?} ({:#010x})", v.to_bits()),
            Value::Bytes(b) => write!(f, "{b:02x?}"),
            Value::Ships(s) => write!(f, "{s:?}"),
            Value::Absent => write!(f, "absent"),
        }
    }
}

/// One differing field.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldDiff {
    /// `ship 12.position.x`, `stream 10.n`, `ship list`, ...
    pub field: String,
    pub expected: Value,
    pub actual: Value,
    /// The expected stream's value one frame earlier, when there is one.
    pub previous: Option<Value>,
}

impl FieldDiff {
    /// Distance in units in the last place, for float fields.
    pub fn ulps(&self) -> Option<u64> {
        match (&self.expected, &self.actual) {
            (Value::Float(a), Value::Float(b)) => Some(ulp_distance(*a, *b)),
            _ => None,
        }
    }
}

/// The first frame where the streams disagree.
#[derive(Debug, Clone, PartialEq)]
pub struct Divergence {
    /// Frame index in both streams.
    pub frame: usize,
    /// Update counters on both sides.
    pub expected_counter: u32,
    pub actual_counter: u32,
    /// Differences found (at most `max_diffs`).
    pub diffs: Vec<FieldDiff>,
    /// Total differences in the frame.
    pub total_diffs: usize,
}

/// Result of [`compare`].
#[derive(Debug, Clone, PartialEq)]
pub struct Report {
    pub expected_frames: usize,
    pub actual_frames: usize,
    /// Leading frames that match.
    pub matching_frames: usize,
    pub divergence: Option<Divergence>,
}

impl Report {
    /// Both streams have the same length and no frame differs.
    pub fn identical(&self) -> bool {
        self.divergence.is_none() && self.expected_frames == self.actual_frames
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "frames: expected {}, actual {}; {} leading frames identical",
            self.expected_frames, self.actual_frames, self.matching_frames
        )?;
        match &self.divergence {
            None if self.expected_frames == self.actual_frames => writeln!(f, "no divergence"),
            None => writeln!(f, "no divergence in the common frames; the streams differ in length"),
            Some(d) => {
                writeln!(
                    f,
                    "first divergence at frame {} (update counter expected {}, actual {}): {} field(s) differ",
                    d.frame, d.expected_counter, d.actual_counter, d.total_diffs
                )?;
                for diff in &d.diffs {
                    write!(f, "  {}: expected {}, actual {}", diff.field, diff.expected, diff.actual)?;
                    if let Some(u) = diff.ulps() {
                        write!(f, " [{u} ulp]")?;
                    }
                    if let Some(p) = &diff.previous {
                        write!(f, "; previous frame {p}")?;
                    }
                    writeln!(f)?;
                }
                if d.total_diffs > d.diffs.len() {
                    writeln!(f, "  ... {} more", d.total_diffs - d.diffs.len())?;
                }
                Ok(())
            }
        }
    }
}

/// Distance between two floats in representable steps (sign-magnitude aware).
pub fn ulp_distance(a: f32, b: f32) -> u64 {
    fn key(v: f32) -> i64 {
        let bits = v.to_bits() as i32 as i64;
        if bits < 0 {
            i32::MIN as i64 - bits
        } else {
            bits
        }
    }
    (key(a) - key(b)).unsigned_abs()
}

/// Compares `actual` against `expected` (normally: the recreation against the original).
pub fn compare(expected: &StateStream, actual: &StateStream, opts: &CompareOptions) -> Report {
    let common = expected.frames.len().min(actual.frames.len());
    let mut divergence = None;
    let mut matching = 0;
    for i in 0..common {
        let prev = i.checked_sub(1).map(|p| &expected.frames[p]);
        let diffs = frame_diffs(&expected.frames[i], &actual.frames[i], prev, opts);
        if !diffs.is_empty() {
            let total = diffs.len();
            divergence = Some(Divergence {
                frame: i,
                expected_counter: expected.frames[i].update_counter,
                actual_counter: actual.frames[i].update_counter,
                diffs: diffs.into_iter().take(opts.max_diffs).collect(),
                total_diffs: total,
            });
            break;
        }
        matching += 1;
    }
    Report {
        expected_frames: expected.frames.len(),
        actual_frames: actual.frames.len(),
        matching_frames: matching,
        divergence,
    }
}

struct Diffs<'o> {
    out: Vec<FieldDiff>,
    opts: &'o CompareOptions,
}

impl Diffs<'_> {
    fn int(&mut self, field: impl FnOnce() -> String, a: u64, b: u64, prev: Option<u64>) {
        if a != b {
            self.out.push(FieldDiff {
                field: field(),
                expected: Value::Int(a),
                actual: Value::Int(b),
                previous: prev.map(Value::Int),
            });
        }
    }

    fn float(&mut self, field: impl FnOnce() -> String, a: f32, b: f32, prev: Option<f32>) {
        let same = if self.opts.float_ulps == 0 {
            a.to_bits() == b.to_bits()
        } else {
            a.to_bits() == b.to_bits()
                || (!a.is_nan() && !b.is_nan() && ulp_distance(a, b) <= self.opts.float_ulps as u64)
        };
        if !same {
            self.out.push(FieldDiff {
                field: field(),
                expected: Value::Float(a),
                actual: Value::Float(b),
                previous: prev.map(Value::Float),
            });
        }
    }
}

fn ship_index(f: &StateFrame) -> HashMap<u16, &ShipState> {
    f.ships.iter().map(|s| (s.ship_number, s)).collect()
}

/// Every difference between two frames.
pub fn frame_diffs(e: &StateFrame, a: &StateFrame, prev: Option<&StateFrame>, opts: &CompareOptions) -> Vec<FieldDiff> {
    let mut d = Diffs { out: Vec::new(), opts };
    d.int(
        || "update counter".into(),
        e.update_counter as u64,
        a.update_counter as u64,
        prev.map(|p| p.update_counter as u64),
    );
    if opts.compare_univ_time {
        d.float(|| "universe time".into(), e.univ_time, a.univ_time, prev.map(|p| p.univ_time));
    }
    let streams = e.streams.len().max(a.streams.len());
    for i in (0..streams).filter(|i| !opts.ignore_streams.contains(i)) {
        let (se, sa) = (e.streams.get(i), a.streams.get(i));
        let sp = prev.and_then(|p| p.streams.get(i));
        match (se, sa) {
            (Some(se), Some(sa)) => {
                let parts = |s: &hwc_math::random::RanStream| [s.x, s.y, s.z, s.c, s.n];
                for (k, name) in ["x", "y", "z", "c", "n"].iter().enumerate() {
                    d.int(
                        || format!("stream {i}.{name}"),
                        parts(se)[k] as u64,
                        parts(sa)[k] as u64,
                        sp.map(|s| parts(s)[k] as u64),
                    );
                }
            }
            (se, sa) => d.out.push(FieldDiff {
                field: format!("stream {i}"),
                expected: if se.is_some() { Value::Int(1) } else { Value::Absent },
                actual: if sa.is_some() { Value::Int(1) } else { Value::Absent },
                previous: None,
            }),
        }
    }

    let order = |f: &StateFrame| f.ships.iter().map(|s| s.ship_number).collect::<Vec<_>>();
    let (oe, oa) = (order(e), order(a));
    if oe != oa {
        d.out.push(FieldDiff {
            field: "ship list".into(),
            expected: Value::Ships(oe),
            actual: Value::Ships(oa),
            previous: prev.map(|p| Value::Ships(order(p))),
        });
    }
    let (ia, ip) = (ship_index(a), prev.map(ship_index));
    for se in &e.ships {
        let n = se.ship_number;
        let Some(sa) = ia.get(&n) else {
            d.out.push(FieldDiff {
                field: format!("ship {n}"),
                expected: Value::Int(n as u64),
                actual: Value::Absent,
                previous: None,
            });
            continue;
        };
        let sp = ip.as_ref().and_then(|m| m.get(&n).copied());
        ship_diffs(&mut d, se, sa, sp);
    }
    for sa in a.ships.iter().filter(|s| e.ship(s.ship_number).is_none()) {
        d.out.push(FieldDiff {
            field: format!("ship {}", sa.ship_number),
            expected: Value::Absent,
            actual: Value::Int(sa.ship_number as u64),
            previous: None,
        });
    }
    d.out
}

type IntField = (&'static str, fn(&ShipState) -> u64);
type FloatsField = (&'static str, fn(&ShipState) -> &[f32]);

fn ship_diffs(d: &mut Diffs<'_>, e: &ShipState, a: &ShipState, p: Option<&ShipState>) {
    let n = e.ship_number;
    let ints: [IntField; 5] = [
        ("player", |s| s.player as u64),
        ("race", |s| s.race as u64),
        ("type", |s| s.ship_type as u64),
        ("reserved", |s| s.reserved as u64),
        ("flags", |s| s.flags as u64),
    ];
    for (name, get) in ints {
        d.int(|| format!("ship {n}.{name}"), get(e), get(a), p.map(get));
    }
    const AXES: [&str; 3] = ["x", "y", "z"];
    let vecs: [FloatsField; 4] = [
        ("position", |s| &s.position),
        ("velocity", |s| &s.velocity),
        ("coordsys", |s| &s.coordsys),
        ("rotspeed", |s| &s.rotspeed),
    ];
    for (name, get) in vecs {
        for (k, (&ve, &va)) in get(e).iter().zip(get(a)).enumerate() {
            let label = if get(e).len() == 3 {
                AXES[k].to_string()
            } else {
                format!("[{k}]")
            };
            d.float(|| format!("ship {n}.{name}.{label}"), ve, va, p.map(|p| get(p)[k]));
        }
    }
    d.float(|| format!("ship {n}.health"), e.health, a.health, p.map(|p| p.health));
    d.float(|| format!("ship {n}.fuel"), e.fuel, a.fuel, p.map(|p| p.fuel));
    if e.extra != a.extra {
        d.out.push(FieldDiff {
            field: format!("ship {n}.extra"),
            expected: Value::Bytes(e.extra.clone()),
            actual: Value::Bytes(a.extra.clone()),
            previous: p.map(|p| Value::Bytes(p.extra.clone())),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::tests::frame;
    use crate::state::{Source, StateHeader};

    fn stream(frames: Vec<StateFrame>) -> StateStream {
        StateStream {
            header: StateHeader::new(Source::Original, ""),
            frames,
            truncated: false,
        }
    }

    #[test]
    fn identical_streams() {
        let a = stream(vec![frame(1, &[1.0, 2.0]), frame(2, &[1.5, 2.5])]);
        let r = compare(&a, &a.clone(), &CompareOptions::default());
        assert!(r.identical());
        assert_eq!(r.matching_frames, 2);
    }

    #[test]
    fn reports_first_divergence_with_context() {
        let a = stream(vec![frame(1, &[1.0, 2.0]), frame(2, &[1.5, 2.5]), frame(3, &[9.0, 9.0])]);
        let mut b = a.clone();
        b.frames[1].ships[1].position[0] = f32::from_bits(2.5f32.to_bits() + 1);
        b.frames[1].streams[10].n ^= 1;
        b.frames[2].ships[0].health = 1.0;
        let r = compare(&a, &b, &CompareOptions::default());
        let d = r.divergence.as_ref().unwrap();
        assert_eq!((r.matching_frames, d.frame, d.total_diffs), (1, 1, 2));
        let pos = d.diffs.iter().find(|f| f.field == "ship 2.position.x").unwrap();
        assert_eq!(pos.ulps(), Some(1));
        assert_eq!(pos.previous, Some(Value::Float(2.0)));
        assert!(d.diffs.iter().any(|f| f.field == "stream 10.n"));
        assert!(r.to_string().contains("ship 2.position.x"));

        let loose = CompareOptions {
            float_ulps: 1,
            ..CompareOptions::default()
        };
        let r = compare(&a, &b, &loose);
        assert_eq!(r.divergence.unwrap().total_diffs, 1);
    }

    #[test]
    fn zero_signs_and_ignored_streams() {
        let a = stream(vec![frame(1, &[0.0])]);
        let mut b = a.clone();
        b.frames[0].ships[0].position[0] = -0.0;
        b.frames[0].streams[0].x = 5;
        let d = compare(&a, &b, &CompareOptions::default()).divergence.unwrap();
        assert_eq!(d.total_diffs, 1);
        assert_eq!(d.diffs[0].field, "ship 1.position.x");
    }

    #[test]
    fn ship_membership_and_order() {
        let a = stream(vec![frame(1, &[1.0, 2.0, 3.0])]);
        let mut b = a.clone();
        b.frames[0].ships.swap(0, 1);
        let d = compare(&a, &b, &CompareOptions::default()).divergence.unwrap();
        assert_eq!(d.diffs.len(), 1);
        assert_eq!(d.diffs[0].field, "ship list");
        b.frames[0].ships.pop();
        let d = compare(&a, &b, &CompareOptions::default()).divergence.unwrap();
        assert!(d.diffs.iter().any(|f| f.field == "ship 3" && f.actual == Value::Absent));
    }

    #[test]
    fn ulp_distance_crosses_zero() {
        assert_eq!(ulp_distance(0.0, -0.0), 0);
        assert_eq!(ulp_distance(f32::from_bits(1), -f32::from_bits(1)), 2);
        assert_eq!(ulp_distance(1.0, 1.0), 0);
    }
}
