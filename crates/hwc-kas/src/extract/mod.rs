//! Reading missions from the player's own `cataclysm.exe` (install-time extractor).
//!
//! The published game ships no mission logic: it rebuilds each mission's IR from the compiled
//! kas2c code inside the player's executable, then caches it (see [`crate::binary`]).
//! [`extract_missions`] does the whole job:
//!
//! 1. **PE mapping** ([`PeImage`]): sections, virtual address to file offset, C strings.
//! 2. **Mission discovery** ([`find_missions`]): `kasMissionStart("missionNN", Init, Watch)`
//!    call sites (`push watch; push init; push name; call`), the callee being the function most
//!    such sites call.
//! 3. **Runtime map** (`runtime`): the embedded kas2c function table and the debug console's
//!    hash dispatcher give every table function's address; the console's argument parser gives
//!    the label lookups; `kasfMsgSend` gives `CurrentTeamP` and the queue send function;
//!    `kasJump`/`kasFSMCreate` are recognised by their bodies. Nothing is located by address, so
//!    other builds with the same runtime structure work too.
//! 4. **Decoding** (`x86`): a small x86-32 decoder for the instructions MSVC emitted.
//! 5. **Symbolic execution** (`decomp`): registers and the argument stack as expressions, calls
//!    with their pushed arguments, branch conditions from the flags, MSVC division idioms folded,
//!    jump tables, and the message loop over the team's queue.
//! 6. **Structuring** (`decomp`): short-circuit conditions, post-dominator IF/ELSE regions,
//!    switches from jump tables and compare trees (the message switch drops case 0).
//! 7. **Lowering** (`lower`): `kasJump` -> `JUMP`, `kasFSMCreate` -> `FSMCreate`, IFONCE guards,
//!    label lookups -> typed references, `char*` constants -> strings, `table[strCurLanguage]` ->
//!    LSTRINGs; arguments typed by the function table.
//! 8. **Mission graph** (`lower`): from `Init_M`/`Watch_M` follow `FSMCreate` to FSMs and
//!    `kasJump` to states (per owning FSM: the linker folded identical functions across states
//!    and missions); unreferenced 16-byte-aligned functions between a mission's code and the next
//!    are attached as states of the FSM whose code precedes them.
//! 9. **Validation**: a mission that does not lower completely (unknown calls, leftover symbolic
//!    values) is an error; the result passes [`crate::binary::validate`].
//!
//! Label names are not in the executable (only their hashes), so [`Mission::names`] is empty.

mod decomp;
mod lower;
mod runtime;
mod x86;

use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use crate::ir::Mission;
use crate::kas_hash;
use crate::Error;

/// A minimal PE32 image over the executable's bytes.
#[derive(Clone, Copy, Debug)]
pub struct PeImage<'a> {
    data: &'a [u8],
    image_base: u32,
    sections_at: usize,
    nsections: usize,
}

impl<'a> PeImage<'a> {
    /// Parses the PE headers.
    pub fn parse(data: &'a [u8]) -> Result<Self, crate::Error> {
        let rd32 = |o: usize| data.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
        let rd16 = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
        if data.get(..2) != Some(b"MZ") {
            return Err(Error::new("not an MZ executable"));
        }
        let pe = rd32(0x3c).ok_or_else(|| Error::new("truncated MZ header"))? as usize;
        if data.get(pe..pe + 4) != Some(b"PE\0\0") {
            return Err(Error::new("not a PE executable"));
        }
        let nsections = rd16(pe + 6).ok_or_else(|| Error::new("truncated PE header"))? as usize;
        let opt_size = rd16(pe + 20).ok_or_else(|| Error::new("truncated PE header"))? as usize;
        if rd16(pe + 24) != Some(0x10b) {
            return Err(Error::new("not a PE32 image"));
        }
        let image_base = rd32(pe + 24 + 28).ok_or_else(|| Error::new("truncated optional header"))?;
        let sections_at = pe + 24 + opt_size;
        if data.len() < sections_at + nsections * 40 {
            return Err(Error::new("truncated section table"));
        }
        Ok(PeImage { data, image_base, sections_at, nsections })
    }

    /// File offset of a virtual address, if it lies in a section's raw data.
    pub fn offset(&self, va: u32) -> Option<usize> {
        let rva = va.checked_sub(self.image_base)?;
        (0..self.nsections).find_map(|i| {
            let s = &self.data[self.sections_at + i * 40..self.sections_at + i * 40 + 40];
            let g = |o: usize| u32::from_le_bytes([s[o], s[o + 1], s[o + 2], s[o + 3]]);
            let (vsize, vaddr, rsize, raw) = (g(8), g(12), g(16), g(20));
            let size = vsize.max(rsize);
            (rva >= vaddr && rva < vaddr + size && rva - vaddr < rsize).then(|| (raw + rva - vaddr) as usize)
        })
    }

    /// Bytes at a virtual address.
    pub fn bytes(&self, va: u32, len: usize) -> Option<&'a [u8]> {
        let o = self.offset(va)?;
        self.data.get(o..o + len)
    }

    /// A NUL-terminated string at a virtual address (Latin-1).
    pub fn cstr(&self, va: u32, max: usize) -> Option<String> {
        let o = self.offset(va)?;
        let tail = self.data.get(o..)?;
        let end = tail.iter().take(max).position(|&b| b == 0)?;
        Some(tail[..end].iter().map(|&b| b as char).collect())
    }

    /// The whole file.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    fn section(&self, va: u32) -> Option<(u32, usize, usize, u32)> {
        let rva = va.checked_sub(self.image_base)?;
        (0..self.nsections).find_map(|i| {
            let s = &self.data[self.sections_at + i * 40..self.sections_at + i * 40 + 40];
            let g = |o: usize| u32::from_le_bytes([s[o], s[o + 1], s[o + 2], s[o + 3]]);
            let (vaddr, rsize, raw, flags) = (g(12), g(16), g(20), g(36));
            (rva >= vaddr && rva - vaddr < rsize).then_some((vaddr, raw as usize, rsize as usize, flags))
        })
    }

    /// The bytes from `va` to the end of its section's raw data.
    pub fn code_at(&self, va: u32) -> Option<&'a [u8]> {
        let (vaddr, raw, rsize, _) = self.section(va)?;
        let o = raw + (va - self.image_base - vaddr) as usize;
        self.data.get(o..(raw + rsize).min(self.data.len()))
    }

    /// True when `va` lies in an executable section.
    pub fn is_code(&self, va: u32) -> bool {
        self.section(va).is_some_and(|s| s.3 & 0x2000_0000 != 0)
    }

    /// The little-endian `u32` at `va` (0 when unmapped).
    pub fn u32_at(&self, va: u32) -> u32 {
        self.bytes(va, 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// The byte at `va` (0 when unmapped).
    pub fn u8_at(&self, va: u32) -> u8 {
        self.bytes(va, 1).map_or(0, |b| b[0])
    }

    /// Executable sections as (virtual address, bytes).
    pub fn code_sections(&self) -> Vec<(u32, &'a [u8])> {
        (0..self.nsections)
            .filter_map(|i| {
                let s = &self.data[self.sections_at + i * 40..self.sections_at + i * 40 + 40];
                let g = |o: usize| u32::from_le_bytes([s[o], s[o + 1], s[o + 2], s[o + 3]]);
                let (vaddr, rsize, raw, flags) = (g(12), g(16), g(20) as usize, g(36));
                (flags & 0x2000_0000 != 0).then(|| (self.image_base + vaddr, self.data.get(raw..raw + rsize as usize)))
            })
            .filter_map(|(va, b)| b.map(|b| (va, b)))
            .collect()
    }
}

/// A mission found in the executable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionEntry {
    /// Script name as passed to `kasMissionStart` (e.g. `mission01`).
    pub name: String,
    /// `Init_<M>` address.
    pub init: u32,
    /// `Watch_<M>` address.
    pub watch: u32,
}

/// Finds `kasMissionStart(name, init, watch)` call sites: three `push imm32` (watch, init, name)
/// followed by a `call rel32`. The callee used by the most such sites with a printable
/// `mission`/`tutorial` name is taken to be `kasMissionStart`. Duplicate names are reported once.
pub fn find_missions(pe: &PeImage<'_>) -> Vec<MissionEntry> {
    let mut sites: Vec<(u32, MissionEntry)> = Vec::new();
    for (base, code) in pe.code_sections() {
        let mut i = 0;
        while i + 20 <= code.len() {
            // push imm32 (68 xx xx xx xx) x3, call rel32 (e8 xx xx xx xx)
            if code[i] == 0x68 && code[i + 5] == 0x68 && code[i + 10] == 0x68 && code[i + 15] == 0xe8 {
                let imm = |o: usize| u32::from_le_bytes([code[o], code[o + 1], code[o + 2], code[o + 3]]);
                let (watch, init, name) = (imm(i + 1), imm(i + 6), imm(i + 11));
                let next = base.wrapping_add((i + 20) as u32);
                let target = next.wrapping_add(imm(i + 16));
                if let Some(n) = pe.cstr(name, 64) {
                    let lower = n.to_ascii_lowercase();
                    let tail = lower.trim_start_matches("mission").trim_start_matches("tutorial");
                    if (lower.starts_with("mission") || lower.starts_with("tutorial"))
                        && !tail.is_empty()
                        && tail.bytes().all(|b| b.is_ascii_digit())
                        && pe.offset(init).is_some()
                        && pe.offset(watch).is_some()
                    {
                        sites.push((target, MissionEntry { name: n, init, watch }));
                    }
                }
            }
            i += 1;
        }
    }
    let mut counts = std::collections::HashMap::new();
    for (t, _) in &sites {
        *counts.entry(*t).or_insert(0usize) += 1;
    }
    let Some((&start, _)) = counts.iter().max_by_key(|(_, &c)| c) else { return Vec::new() };
    let mut out: Vec<MissionEntry> = Vec::new();
    for (t, m) in sites {
        if t == start && !out.iter().any(|o| o.name.eq_ignore_ascii_case(&m.name)) {
            out.push(m);
        }
    }
    out
}

/// An extraction failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractError {
    /// The mission it concerns, if any.
    pub mission: Option<String>,
    /// Description, innermost context last.
    pub message: String,
}

impl ExtractError {
    /// A new error.
    pub fn new(message: impl Into<String>) -> Self {
        ExtractError { mission: None, message: message.into() }
    }

    /// Prefixes the message with where it happened.
    pub fn context(mut self, at: &str) -> Self {
        self.message = format!("{at}: {}", self.message);
        self
    }

    fn in_mission(mut self, name: &str) -> Self {
        self.mission = Some(name.to_string());
        self
    }
}

impl fmt::Display for ExtractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.mission {
            Some(m) => write!(f, "{m}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for ExtractError {}

impl From<Error> for ExtractError {
    fn from(e: Error) -> Self {
        ExtractError::new(e.message)
    }
}

/// `mission01` -> `Mission01`, `tutorial1` -> `Tutorial1`.
pub fn canonical_name(name: &str) -> String {
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + &c.as_str().to_ascii_lowercase(),
        None => String::new(),
    }
}

/// Extracts every mission from the executable's bytes. Fails if any mission does not lower
/// completely.
pub fn extract_missions(exe_bytes: &[u8]) -> Result<Vec<Mission>, ExtractError> {
    extract_each(exe_bytes)?.into_iter().collect()
}

/// Extracts every mission, reporting failures per mission (in discovery order).
pub fn extract_each(exe_bytes: &[u8]) -> Result<Vec<Result<Mission, ExtractError>>, ExtractError> {
    let pe = PeImage::parse(exe_bytes)?;
    let entries = find_missions(&pe);
    if entries.is_empty() {
        return Err(ExtractError::new("no kasMissionStart call sites found"));
    }
    let mut rt = runtime::Runtime::build(&pe)?;
    rt.scope = scope_globals(&pe, &entries[0])?;
    let rt = rt;

    // Each mission's code ends with its Watch function; unreferenced states are looked for
    // between the previous mission's end and this one's.
    let mut by_watch: Vec<usize> = (0..entries.len()).collect();
    by_watch.sort_by_key(|&i| entries[i].watch);
    let mut cache = decomp::TargetCache::new();
    let mut ends = vec![0u32; entries.len()];
    for &i in &by_watch {
        ends[i] = lower::function_end(&pe, &rt, &mut cache, entries[i].watch).unwrap_or(entries[i].watch);
    }
    let prev_end = |i: usize| -> Option<u32> {
        let k = by_watch.iter().position(|&x| x == i)?;
        (k > 0).then(|| ends[by_watch[k - 1]])
    };

    type Lowered = Result<(Mission, Option<u32>), ExtractError>;
    let results: Mutex<Vec<Option<Lowered>>> = Mutex::new(vec![None; entries.len()]);
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(entries.len());
    std::thread::scope(|s| {
        for _ in 0..workers {
            let (pe, rt, entries, ends, results, next) = (&pe, &rt, &entries, &ends, &results, &next);
            let prev_end = &prev_end;
            std::thread::Builder::new()
                .stack_size(256 << 20)
                .spawn_scoped(s, move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(e) = entries.get(i) else { break };
                    let name = canonical_name(&e.name);
                    let mut mx = lower::MissionX::new(pe, rt, name.clone(), e.init, e.watch);
                    mx.discover();
                    let lo = prev_end(i).or_else(|| mx.extent().map(|x| x.0)).unwrap_or(ends[i]);
                    mx.orphans(lo, ends[i]);
                    let r = mx.lower().map_err(|err| err.in_mission(&name));
                    results.lock().unwrap()[i] = Some(r);
                })
                .expect("spawn extractor thread");
        }
    });
    let results = results.into_inner().unwrap();

    let mut lang: Option<u32> = None;
    let mut out = Vec::with_capacity(entries.len());
    for (i, r) in results.into_iter().enumerate() {
        let r = r.unwrap_or_else(|| Err(ExtractError::new("not extracted").in_mission(&entries[i].name)));
        out.push(r.and_then(|(m, l)| {
            if let Some(l) = l {
                if lang.is_some_and(|x| x != l) {
                    return Err(ExtractError::new("localised strings indexed by different globals").in_mission(&m.name));
                }
                lang = Some(l);
            }
            crate::binary::validate(&m).map_err(|e| ExtractError::from(e).in_mission(&m.name))?;
            Ok(m)
        }));
    }
    Ok(out)
}

/// `CurrentMissionScope` and `CurrentMissionScopeName`: the mission Init function starts by
/// storing 0 and the mission's name hash into them.
fn scope_globals(pe: &PeImage<'_>, m: &MissionEntry) -> Result<[u32; 2], ExtractError> {
    let want = kas_hash(&m.name);
    let (mut scope, mut name) = (None, None);
    let mut a = m.init;
    for _ in 0..8 {
        let Some(i) = pe.code_at(a).and_then(|b| x86::decode(b, a)) else { break };
        if i.op == x86::Op::Mov {
            if let (Some(x86::Operand::Mem(d)), Some(x86::Operand::Imm(v))) = (i.op(0), i.op(1)) {
                if d.base.is_none() && d.index.is_none() {
                    if v == want {
                        name = Some(d.disp as u32);
                    } else if v == 0 {
                        scope = Some(d.disp as u32);
                    }
                }
            }
        }
        a = i.next();
    }
    match (scope, name) {
        (Some(s), Some(n)) => Ok([s, n]),
        _ => Err(ExtractError::new(format!("unexpected prologue of the {} INIT function", m.name))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pe() {
        assert!(PeImage::parse(b"hello").is_err());
    }
}
