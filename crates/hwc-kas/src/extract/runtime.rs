//! The runtime map: which addresses in the executable are the KAS runtime functions the mission
//! code calls, found by the structure of the code rather than by address.
//!
//! * The kas2c function table is embedded (fixed-size records: KAS name, C name, return flag,
//!   parameter count and parameters) for the debug console. It is located by its first record
//!   and checked against [`FUNCTIONS`].
//! * The console's call dispatcher hashes the KAS name and walks a compare tree over the name
//!   hashes; each leaf calls the C function. Walking the tree gives `FuncId -> address`.
//! * The console's argument parser recognises the `SHIPS_`, `TEAM_`, `POINT_`, ... prefixes and
//!   calls the matching label lookup, which gives the label lookup functions. Variants the console
//!   does not use are matched by the label list they search.
//! * `kasfMsgSend` gives `CurrentTeamP` (the global it passes as the sender) and the team
//!   message-queue send function; `kasJump` and `kasFSMCreate` are recognised by their bodies
//!   (they transfer control through a code pointer argument).

use std::collections::{HashMap, HashSet};

use super::x86::{decode, Cc, Insn, Mem, Op, Operand, Reg};
use super::{ExtractError, PeImage};
use crate::functions::{FuncId, FUNCTIONS};
use crate::kas_hash;

/// Label lookup kinds (the kas2c parameter keywords).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum LabelKind {
    Ships,
    Team,
    TeamShips,
    Volume,
    Path,
    Point,
    ShipsPoint,
    TeamsPoint,
    VolumePoint,
}

/// What a called address is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Target {
    /// A kas2c table function.
    Func(FuncId),
    /// A label lookup taking the label hash.
    Label(LabelKind),
    /// The centre of the current team's ships (no arguments).
    ThisTeamsPoint,
    /// `kasJump(state, init, watch)`.
    Jump,
    /// `kasFSMCreate(fsm, init, watch, team)`.
    FsmCreate,
    /// The team message queue send `(sender, team, msg)`.
    MsgSend,
    /// The team message queue fetch `(team, index)`; recognised by its use in a message loop.
    MsgGet,
    /// Not a known runtime function.
    Unknown,
}

impl Target {
    /// Number of stack arguments.
    pub(crate) fn arity(self) -> Option<usize> {
        Some(match self {
            Target::Func(f) => f.info().params.len(),
            Target::Label(_) => 1,
            Target::ThisTeamsPoint => 0,
            Target::Jump => 3,
            Target::FsmCreate => 4,
            Target::MsgSend => 3,
            Target::MsgGet => 2,
            Target::Unknown => return None,
        })
    }
}

// Layout of one record of the embedded kas2c table.
const REC_SIZE: usize = 0x990;
const REC_CNAME: usize = 0x88;
const REC_NPARAMS: usize = 0x10c;
const NAME_FIELD: usize = 0x80;

const LABEL_PREFIXES: [(&str, LabelKind); 9] = [
    ("SHIPS_", LabelKind::Ships),
    ("TEAMSHIPS_", LabelKind::TeamShips),
    ("TEAM_", LabelKind::Team),
    ("PATH_", LabelKind::Path),
    ("TEAMSPOINT_", LabelKind::TeamsPoint),
    ("SHIPSPOINT_", LabelKind::ShipsPoint),
    ("VOLUMEPOINT_", LabelKind::VolumePoint),
    ("POINT_", LabelKind::Point),
    ("VOLUME_", LabelKind::Volume),
];

/// The runtime map of one executable.
#[derive(Debug)]
pub(crate) struct Runtime {
    /// Table functions by address.
    pub funcs: HashMap<u32, FuncId>,
    /// Label lookups by address (from the console argument parser).
    pub labels: HashMap<u32, LabelKind>,
    /// Label kinds by the global label list their lookup searches.
    list_kinds: HashMap<u32, LabelKind>,
    /// `CurrentTeamP`.
    pub team_ptr: u32,
    /// The team message queue send function.
    pub msg_send: u32,
    /// `CurrentMissionScope` and `CurrentMissionScopeName` (set from the mission code).
    pub scope: [u32; 2],
}

fn rd(pe: &PeImage<'_>, va: u32) -> Option<Insn> {
    decode(pe.code_at(va)?, va)
}

/// The global a memory operand reads, if it is a plain `[abs]`.
fn abs_of(m: &Mem) -> Option<u32> {
    (m.base.is_none() && m.index.is_none()).then_some(m.disp as u32)
}

impl Runtime {
    /// Builds the map from the executable.
    pub(crate) fn build(pe: &PeImage<'_>) -> Result<Runtime, ExtractError> {
        let table = parse_table(pe)?;
        let funcs = dispatch_map(pe, &table)?;
        let labels = label_map(pe);
        for kind in [LabelKind::Team, LabelKind::TeamShips, LabelKind::Volume, LabelKind::Point, LabelKind::Path] {
            if !labels.values().any(|&k| k == kind) {
                return Err(ExtractError::new(format!("label lookup for {kind:?} not found")));
            }
        }
        // Lists searched by the plain lookups (the point lookups search the same lists and then
        // compute a position): a list only one kind searches identifies variants of its lookup.
        let mut by_list: HashMap<u32, HashSet<LabelKind>> = HashMap::new();
        let plain = [LabelKind::Ships, LabelKind::Team, LabelKind::TeamShips, LabelKind::Volume, LabelKind::Path, LabelKind::Point];
        for (&a, &k) in labels.iter().filter(|(_, k)| plain.contains(k)) {
            if let Some(g) = first_global_read(pe, a) {
                by_list.entry(g).or_default().insert(k);
            }
        }
        let list_kinds = by_list
            .into_iter()
            .filter(|(_, ks)| ks.len() == 1)
            .map(|(g, ks)| (g, *ks.iter().next().unwrap()))
            .collect();

        let send = FuncId::by_name("MsgSend").ok_or_else(|| ExtractError::new("MsgSend missing from the function table"))?;
        let send_at = funcs
            .iter()
            .find(|(_, &f)| f == send)
            .map(|(&a, _)| a)
            .ok_or_else(|| ExtractError::new("kasfMsgSend not found"))?;
        let (mut team_ptr, mut msg_send) = (None, None);
        let mut a = send_at;
        for _ in 0..16 {
            let Some(i) = rd(pe, a) else { break };
            if team_ptr.is_none() {
                team_ptr = i.ops.iter().flatten().find_map(|o| o.mem().and_then(abs_of));
            }
            if i.op == Op::Call {
                msg_send = i.target();
                break;
            }
            if i.op == Op::Ret {
                break;
            }
            a = i.next();
        }
        let (Some(team_ptr), Some(msg_send)) = (team_ptr, msg_send) else {
            return Err(ExtractError::new("unexpected kasfMsgSend body"));
        };
        Ok(Runtime { funcs, labels, list_kinds, team_ptr, msg_send, scope: [0; 2] })
    }

    /// A map built by hand (tests).
    #[cfg(test)]
    pub(crate) fn for_tests(funcs: HashMap<u32, FuncId>, team_ptr: u32, scope: [u32; 2]) -> Runtime {
        Runtime { funcs, labels: HashMap::new(), list_kinds: HashMap::new(), team_ptr, msg_send: 0, scope }
    }

    /// Classifies a call target (without the usage-based message-get recognition).
    pub(crate) fn classify(&self, pe: &PeImage<'_>, a: u32) -> Target {
        if let Some(&f) = self.funcs.get(&a) {
            return Target::Func(f);
        }
        if let Some(&k) = self.labels.get(&a) {
            return Target::Label(k);
        }
        if a == self.msg_send {
            return Target::MsgSend;
        }
        if let Some(t) = self.control_transfer(pe, a) {
            return t;
        }
        let first = first_global_read(pe, a);
        if let Some(&k) = first.and_then(|g| self.list_kinds.get(&g)) {
            if callee_arity(pe, a) == 1 {
                return Target::Label(k);
            }
        }
        if first == Some(self.team_ptr) && callee_arity(pe, a) == 0 && returns_static(pe, a) {
            return Target::ThisTeamsPoint;
        }
        Target::Unknown
    }

    /// Classifies a `jmp` target: only exactly known runtime entry points end a function.
    pub(crate) fn is_tail_target(&self, pe: &PeImage<'_>, a: u32) -> bool {
        self.funcs.contains_key(&a)
            || self.labels.contains_key(&a)
            || a == self.msg_send
            || self.control_transfer(pe, a).is_some()
    }

    /// `kasJump` ends with a jump through its init-function argument; `kasFSMCreate` calls
    /// through one.
    fn control_transfer(&self, pe: &PeImage<'_>, a: u32) -> Option<Target> {
        let mut at = a;
        for _ in 0..48 {
            let i = rd(pe, at)?;
            let via_stack = matches!(i.op(0), Some(Operand::Mem(Mem { base: Some(Reg::Esp), index: None, .. })));
            match i.op {
                Op::Jmp if via_stack => return Some(Target::Jump),
                Op::Call if via_stack => return Some(Target::FsmCreate),
                Op::Ret | Op::Jmp => return None,
                _ => {}
            }
            at = i.next();
        }
        None
    }
}

/// The first absolute global a function reads (within its first instructions).
fn first_global_read(pe: &PeImage<'_>, a: u32) -> Option<u32> {
    let mut at = a;
    for _ in 0..6 {
        let i = rd(pe, at)?;
        if i.op == Op::Mov {
            if let Some(g) = i.op(1).as_ref().and_then(Operand::mem).and_then(abs_of) {
                return Some(g);
            }
        }
        if matches!(i.op, Op::Ret | Op::Jmp | Op::Call | Op::Jcc(_)) {
            return None;
        }
        at = i.next();
    }
    None
}

/// The function returns the address of a static (`mov eax, imm32` before its `ret`).
fn returns_static(pe: &PeImage<'_>, a: u32) -> bool {
    let mut at = a;
    let mut last_imm = false;
    for _ in 0..64 {
        let Some(i) = rd(pe, at) else { return false };
        match i.op {
            Op::Ret => return last_imm,
            Op::Mov if i.op(0).and_then(|o| o.reg()) == Some(Reg::Eax) && matches!(i.op(1), Some(Operand::Imm(_))) => {
                last_imm = true
            }
            Op::Mov if i.op(0).and_then(|o| o.reg()) == Some(Reg::Eax) => last_imm = false,
            Op::Jmp | Op::Jcc(_) => {}
            _ => {}
        }
        at = i.next();
    }
    false
}

/// Number of 4-byte stack arguments a function reads, by following its code and tracking ESP.
pub(crate) fn callee_arity(pe: &PeImage<'_>, addr: u32) -> usize {
    let mut seen: HashSet<u32> = HashSet::new();
    let mut work = vec![(addr, 0i32, false)];
    let mut maxarg: i32 = -1;
    let mut count = 0;
    while let Some((mut a, mut delta, mut frame)) = work.pop() {
        loop {
            if count >= 1500 || !seen.insert(a) {
                break;
            }
            let Some(i) = rd(pe, a) else { break };
            count += 1;
            for m in i.ops.iter().flatten().filter_map(Operand::mem) {
                if m.index.is_none() {
                    match m.base {
                        Some(Reg::Esp) => {
                            let off = m.disp - delta;
                            if off >= 4 {
                                maxarg = maxarg.max((off - 4) / 4);
                            }
                        }
                        Some(Reg::Ebp) if frame && m.disp >= 8 => maxarg = maxarg.max((m.disp - 8) / 4),
                        _ => {}
                    }
                }
            }
            let esp_imm = || match (i.op(0), i.op(1)) {
                (Some(Operand::Reg { reg: Reg::Esp, .. }), Some(Operand::Imm(v))) => Some(v as i32),
                _ => None,
            };
            match i.op {
                Op::Push => delta += 4,
                Op::Pop => delta -= 4,
                Op::Mov if matches!(
                    (i.op(0), i.op(1)),
                    (Some(Operand::Reg { reg: Reg::Ebp, size: 4, .. }), Some(Operand::Reg { reg: Reg::Esp, size: 4, .. }))
                ) =>
                {
                    frame = true
                }
                Op::Sub => delta += esp_imm().unwrap_or(0),
                Op::Add => delta -= esp_imm().unwrap_or(0),
                _ => {}
            }
            match i.op {
                Op::Ret => break,
                Op::Jmp => match i.target() {
                    Some(t) => {
                        a = t;
                        continue;
                    }
                    None => break,
                },
                Op::Jcc(_) | Op::Loop => {
                    if let Some(t) = i.target() {
                        work.push((t, delta, frame));
                    }
                }
                _ => {}
            }
            a = i.next();
        }
    }
    (maxarg + 1) as usize
}

/// One entry of the embedded kas2c table: (KAS name, C name, parameter count).
type TableEntry = (String, String, usize);

fn field(b: &[u8]) -> Option<String> {
    let end = b.iter().position(|&c| c == 0)?;
    let s = &b[..end];
    (!s.is_empty() && s.iter().all(|c| c.is_ascii_graphic())).then(|| s.iter().map(|&c| c as char).collect())
}

/// Locates the embedded table by its first record and reads it, checking every entry against
/// the generated table.
fn parse_table(pe: &PeImage<'_>) -> Result<Vec<(FuncId, TableEntry)>, ExtractError> {
    let first = &FUNCTIONS[0];
    let mut pat = first.kas.as_bytes().to_vec();
    pat.push(0);
    let data = pe.data();
    let mut start = None;
    let mut from = 0;
    while let Some(p) = find(&data[from..], &pat).map(|p| p + from) {
        if data.get(p + REC_CNAME..).and_then(field).as_deref() == Some(first.c) {
            start = Some(p);
            break;
        }
        from = p + 1;
    }
    let start = start.ok_or_else(|| ExtractError::new("kas2c function table not found"))?;
    let mut out = Vec::new();
    let mut at = start;
    while let Some(rec) = data.get(at..at + REC_SIZE) {
        let (Some(kas), Some(c)) = (field(&rec[..NAME_FIELD]), field(&rec[REC_CNAME..REC_CNAME + NAME_FIELD])) else { break };
        let n = u32::from_le_bytes(rec[REC_NPARAMS..REC_NPARAMS + 4].try_into().unwrap()) as usize;
        let id = FuncId::by_name(&kas).ok_or_else(|| ExtractError::new(format!("unknown kas2c function {kas}")))?;
        let info = id.info();
        if info.c != c || info.params.len() != n {
            return Err(ExtractError::new(format!("kas2c table entry {kas} ({c}, {n} parameters) differs from the known table")));
        }
        out.push((id, (kas, c, n)));
        at += REC_SIZE;
    }
    if out.len() < FUNCTIONS.len() / 2 {
        return Err(ExtractError::new(format!("kas2c function table too short ({} entries)", out.len())));
    }
    Ok(out)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Finds the console dispatcher (a compare tree over KAS name hashes right after the name is
/// hashed) and maps each table function to the address its leaf calls.
fn dispatch_map(pe: &PeImage<'_>, table: &[(FuncId, TableEntry)]) -> Result<HashMap<u32, FuncId>, ExtractError> {
    let hashes: HashMap<u32, Vec<FuncId>> = table.iter().fold(HashMap::new(), |mut m, (id, (kas, _, _))| {
        m.entry(kas_hash(kas)).or_default().push(*id);
        m
    });
    // `call hash; add esp, 4; cmp eax, imm32` with the immediate one of the name hashes.
    let mut best: Option<HashMap<u32, u32>> = None;
    for (base, code) in pe.code_sections() {
        for i in 0..code.len().saturating_sub(13) {
            if code[i] == 0xe8 && code[i + 5..i + 8] == [0x83, 0xc4, 0x04] && code[i + 8] == 0x3d {
                let imm = u32::from_le_bytes(code[i + 9..i + 13].try_into().unwrap());
                if hashes.contains_key(&imm) {
                    let leaves = walk_tree(pe, base + i as u32 + 8);
                    if best.as_ref().is_none_or(|b| leaves.len() > b.len()) {
                        best = Some(leaves);
                    }
                }
            }
        }
    }
    let leaves = best.ok_or_else(|| ExtractError::new("kas2c console dispatcher not found"))?;
    let mut by_addr: HashMap<u32, Vec<FuncId>> = HashMap::new();
    for (h, ids) in &hashes {
        let Some(&leaf) = leaves.get(h) else { continue };
        let Some(t) = leaf_target(pe, leaf) else { continue };
        by_addr.entry(t).or_default().extend(ids);
    }
    if by_addr.values().map(Vec::len).sum::<usize>() < table.len() * 9 / 10 {
        return Err(ExtractError::new("kas2c console dispatcher: too few functions resolved"));
    }
    // Entries sharing one address were folded by the linker into one body (identical code). The
    // lowest table index stands for the group, except for the empty debug stub, which the
    // scripts reach through dbgAssert.
    let assert = FuncId::by_name("dbgAssert");
    Ok(by_addr
        .into_iter()
        .map(|(a, mut ids)| {
            ids.sort();
            let pick = assert.filter(|x| ids.contains(x)).unwrap_or(ids[0]);
            (a, pick)
        })
        .collect())
}

/// Walks a compare tree on `eax` (the hash, possibly offset by `sub`/`dec`) and returns the
/// leaf address for each hash value.
fn walk_tree(pe: &PeImage<'_>, root: u32) -> HashMap<u32, u32> {
    let mut leaves = HashMap::new();
    let mut seen = HashSet::new();
    let mut work = vec![(root, 0u32)];
    let is_eax = |o: Option<Operand>| matches!(o, Some(Operand::Reg { reg: Reg::Eax, size: 4, .. }));
    while let Some((mut a, mut delta)) = work.pop() {
        let mut eq: Option<u32> = None;
        for _ in 0..4096 {
            if !seen.insert((a, delta)) {
                break;
            }
            let Some(i) = rd(pe, a) else { break };
            let imm = i.op(1).and_then(|o| o.imm());
            match i.op {
                Op::Cmp if is_eax(i.op(0)) && imm.is_some() => eq = Some(delta.wrapping_add(imm.unwrap())),
                Op::Sub if is_eax(i.op(0)) && imm.is_some() => {
                    delta = delta.wrapping_add(imm.unwrap());
                    eq = Some(delta);
                }
                Op::Add if is_eax(i.op(0)) && imm.is_some() => {
                    delta = delta.wrapping_sub(imm.unwrap());
                    eq = Some(delta);
                }
                Op::Dec if is_eax(i.op(0)) => {
                    delta = delta.wrapping_add(1);
                    eq = Some(delta);
                }
                Op::Inc if is_eax(i.op(0)) => {
                    delta = delta.wrapping_sub(1);
                    eq = Some(delta);
                }
                Op::Jcc(Cc::E) => {
                    if let (Some(h), Some(t)) = (eq, i.target()) {
                        leaves.entry(h).or_insert(t);
                    }
                }
                Op::Jcc(Cc::Ne) => {
                    if let Some(t) = i.target() {
                        work.push((t, delta));
                    }
                    if let Some(h) = eq {
                        leaves.entry(h).or_insert(i.next());
                    }
                    break;
                }
                Op::Jcc(_) => {
                    if let Some(t) = i.target() {
                        work.push((t, delta));
                    }
                }
                Op::Jmp if i.target().is_some() => {
                    a = i.target().unwrap();
                    continue;
                }
                _ => break,
            }
            a = i.next();
        }
    }
    leaves
}

/// The function a dispatcher leaf calls (or tail-jumps to).
fn leaf_target(pe: &PeImage<'_>, leaf: u32) -> Option<u32> {
    let mut a = leaf;
    for _ in 0..24 {
        let i = rd(pe, a)?;
        match i.op {
            Op::Call | Op::Jmp => return i.target(),
            Op::Ret | Op::Jcc(_) => return None,
            _ => {}
        }
        a = i.next();
    }
    None
}

/// The console argument parser compares an argument against each label prefix and, on a match,
/// hashes the rest and calls the lookup: `push len; push prefix; push s; call strncmp; ...;
/// call hash; push eax; call lookup`.
fn label_map(pe: &PeImage<'_>) -> HashMap<u32, LabelKind> {
    let mut out = HashMap::new();
    let mut bad = HashSet::new();
    for (base, code) in pe.code_sections() {
        for i in 0..code.len().saturating_sub(5) {
            if code[i] != 0x68 {
                continue;
            }
            let p = u32::from_le_bytes(code[i + 1..i + 5].try_into().unwrap());
            let Some(s) = pe.cstr(p, 16) else { continue };
            let Some(&(_, kind)) = LABEL_PREFIXES.iter().find(|(pfx, _)| *pfx == s) else { continue };
            let mut a = base + i as u32 + 5;
            let mut calls = 0;
            for _ in 0..16 {
                let Some(ins) = rd(pe, a) else { break };
                if ins.op == Op::Ret {
                    break;
                }
                if ins.op == Op::Call {
                    calls += 1;
                    if calls == 3 {
                        if let Some(t) = ins.target() {
                            if out.insert(t, kind).is_some_and(|k| k != kind) {
                                bad.insert(t);
                            }
                        }
                        break;
                    }
                }
                a = ins.next();
            }
        }
    }
    out.retain(|a, _| !bad.contains(a));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares the runtime map with the private address notes (when present).
    #[test]
    fn runtime_map_matches_notes() {
        let Some(dir) = std::env::var_os("HWC_GAME_DIR") else { return };
        let Ok(exe) = std::fs::read(std::path::Path::new(&dir).join("cataclysm.exe")) else { return };
        let pe = PeImage::parse(&exe).unwrap();
        let rt = Runtime::build(&pe).unwrap();
        eprintln!("{} table functions, labels {:x?}, lists {:x?}", rt.funcs.len(), rt.labels, rt.list_kinds);
        eprintln!("team {:08x} send {:08x}", rt.team_ptr, rt.msg_send);
        for a in [0x48c1d0u32, 0x48c0c0, 0x48c010, 0x48bc60, 0x48bc80, 0x417ed0] {
            eprintln!("{a:08x}: {:?} arity {} first {:x?}", rt.classify(&pe, a), callee_arity(&pe, a), first_global_read(&pe, a));
        }
        let notes = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../recovered/missions/data/runtime_names.tsv");
        let Ok(text) = std::fs::read_to_string(notes) else { return };
        let mut bad = 0;
        for line in text.lines().skip(1) {
            let p: Vec<&str> = line.split('\t').collect();
            let (Some(a), Some(names)) = (p.first().and_then(|a| u32::from_str_radix(a, 16).ok()), p.get(1)) else { continue };
            let first = names.split('|').next().unwrap();
            if !first.starts_with("kasf") || p.get(2).is_none_or(|k| k.is_empty()) {
                continue;
            }
            match rt.funcs.get(&a) {
                Some(f) if names.split('|').any(|n| n == f.info().c) => {}
                other => {
                    bad += 1;
                    eprintln!("{a:08x} {names}: {other:?}");
                }
            }
        }
        assert_eq!(bad, 0);
    }
}
