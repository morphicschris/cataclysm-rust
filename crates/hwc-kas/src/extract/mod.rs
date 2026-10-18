//! Reading missions from the player's own `cataclysm.exe` (install-time extractor).
//!
//! The published game ships no mission logic: it must rebuild each mission's IR from the
//! compiled kas2c code inside the player's executable, then cache it (see [`crate::binary`]).
//! This module has the first stage (locating the missions in the exe bytes); the rest is the
//! design for porting `tools/scripts/kas_decompile.py` (plus `crates/hwc-kas/tools/kas_ir_export.py`)
//! to Rust.
//!
//! # What the extractor needs
//!
//! 1. **PE mapping** ([`PeImage`]): section table, virtual address to file offset, C strings.
//! 2. **Mission discovery** ([`find_missions`]): the level-start selector (0x005148f0 in the
//!    Aug 2000 build) and `tutInitTutorial` (0x0052a740) call
//!    `kasMissionStart("missionNN", Init_MissionNN, Watch_MissionNN)` as `push watch; push init;
//!    push name; call kasMissionStart`. Scanning for that push/push/push/call pattern gives the
//!    name and both entry points. To be robust against other builds, identify `kasMissionStart`
//!    itself by its body (memStrncpy of 0x2f bytes, store of the watch pointer, `hsStaticInit`,
//!    tail jump to the init function) rather than by address.
//! 3. **Runtime map**: the address of every `kasf*` function, `kasJump`, `kasFSMCreate`, the
//!    label lookups (`kasAITeamPtr`, `kasAITeamShipsPtr`, `kasGrowSelectionPtr`, `kasVolumePtr`,
//!    `kasPathPtr` and its no-check variant, `kasVectorPtr`, `kasShipsVectorPtr`,
//!    `kasTeamsVectorPtr`, `kasVolumeVectorPtr`, `kasThisTeamsVectorPtr`), `aitMsgGet`,
//!    `aitMsgSend`, and the globals `CurrentTeamP`, `CurrentMissionScope`,
//!    `CurrentMissionScopeName`, `strCurLanguage`. The exe embeds the kas2c function table for its
//!    debug console (KAS name, C name pointer, parameter descriptors) and
//!    `consoleCallKASFunction` dispatches on the hashed KAS name with direct calls, so the table
//!    and the dispatcher give the `FuncId -> address` map without any hard-coded addresses.
//!    The remaining runtime entry points are the callees of `kasMissionStart`'s callers and of the
//!    mission functions with fixed argument shapes (3 args = `kasJump`, 4 = `kasFSMCreate`, 1
//!    constant arg = label lookups) and can be confirmed by their short bodies.
//! 4. **Function decoding**: an x86-32 decoder for the subset MSVC emitted for kas2c output
//!    (mov/push/pop/call/ret/jmp/jcc/cmp/test/add/sub/imul/idiv/cdq/sar/shl/shr/and/or/xor/neg/not/
//!    inc/dec/lea/setcc/sbb, `[abs]`/`[reg+disp]`/`[reg*4+abs]` operands). A table-driven decoder
//!    of a few hundred lines covers it; no general-purpose disassembler is needed.
//! 5. **Symbolic execution per function** (kas_decompile `FuncDecomp`): track registers and the
//!    argument stack symbolically (constants, calls, `CurrentTeamP`, loaded globals), turn each
//!    `call` into a call node with its pushed arguments (arity from the function table, or from
//!    callee `ret n`/caller `add esp` for the runtime helpers), and conditional branches into
//!    conditions over the flags (`cmp`/`test` + `jcc`, `setcc`/`sbb` idioms for values). Fold
//!    MSVC's division-by-constant sequences (`imul` magic + `sar` + sign fix) back to `/`.
//!    Recognise the message loop: load of `team->msgQueue` (+0x50), count, the `aitMsgGet(team, i)`
//!    call, the compare tree on its result, the `aitMsgSend(team, team, msg)` re-queue, and the
//!    back edge.
//! 6. **Structuring** (kas_decompile `Structurer`): build the CFG, merge short-circuit
//!    condition chains into AND/OR, compute post-dominators and emit IF/ELSE regions; recover
//!    `switch` from compare trees on a single call value (the message switch drops case 0, the
//!    "empty slot" continue).
//! 7. **Lowering to IR** (kas_ir_export): `kasJump(h, init, watch)` → `Stmt::Jump`;
//!    `kasFSMCreate(h, init, watch, team)` → `Stmt::FsmCreate`; IFONCE = `if (!VarGet(h) &&
//!    cond) { VarCreateSet(h, 1); ... }`; label lookups → typed references; `CHARPTR` constants →
//!    the C string at that address; `table[strCurLanguage]` → LSTRING (read the three string
//!    pointers of the table).
//! 8. **Mission graph**: start from `Init_M`/`Watch_M`, follow `FSMCreate` to FSM INIT/WATCH and
//!    `kasJump` to state INIT/WATCH (per owning FSM, because MSVC folded identical functions
//!    across states and missions). kas2c also emitted states nothing jumps to; find them as
//!    unreferenced 16-byte-aligned functions between a mission's code and the next, paired as
//!    (INIT, WATCH) in address order and attached to the FSM whose code precedes them.
//! 9. **Validation**: every mission must lower without unknown calls or leftover symbolic values;
//!    compare against a known-good digest of the IR (a hash of [`crate::binary::encode`]) for the
//!    known executable versions, and refuse to run a mission that fails to lower.
//!
//! The Python pipeline lowers all 18 scripts of the Aug 2000 build without errors (2,083 kas2c
//! functions), so the approach is proven; porting it is a mechanical but sizeable task.

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
    pub fn parse(data: &'a [u8]) -> Result<Self, Error> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_pe() {
        assert!(PeImage::parse(b"hello").is_err());
    }
}
