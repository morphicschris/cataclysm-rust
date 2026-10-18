//! A small x86-32 decoder.
//!
//! It decodes the integer instructions MSVC emitted for kas2c output and the runtime code the
//! extractor inspects (moves, stack, ALU, shifts, multiply/divide, branches, `setcc`, `lea`) into
//! a structured form, and computes the length of everything else (FPU, string, SSE, system
//! instructions) so that linear scans stay in sync. Anything it cannot size is reported as
//! undecodable.

/// A 32-bit general purpose register (encoding order).
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Reg {
    Eax = 0,
    Ecx = 1,
    Edx = 2,
    Ebx = 3,
    Esp = 4,
    Ebp = 5,
    Esi = 6,
    Edi = 7,
}

impl Reg {
    /// The register with encoding `n` (0..8).
    pub fn from_index(n: u8) -> Reg {
        match n & 7 {
            0 => Reg::Eax,
            1 => Reg::Ecx,
            2 => Reg::Edx,
            3 => Reg::Ebx,
            4 => Reg::Esp,
            5 => Reg::Ebp,
            6 => Reg::Esi,
            _ => Reg::Edi,
        }
    }

    /// Encoding index.
    pub fn index(self) -> usize {
        self as usize
    }
}

/// A memory operand `[base + index*scale + disp]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mem {
    /// Base register.
    pub base: Option<Reg>,
    /// Index register.
    pub index: Option<Reg>,
    /// Scale of the index (1, 2, 4, 8).
    pub scale: u8,
    /// Displacement (sign-extended).
    pub disp: i32,
    /// Access size in bytes.
    pub size: u8,
}

/// An operand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operand {
    /// A register; `size` is 1, 2 or 4 bytes (8-bit registers name their 32-bit parent, `high`
    /// marks AH/CH/DH/BH).
    Reg {
        /// The (parent) register.
        reg: Reg,
        /// Width in bytes.
        size: u8,
        /// AH/CH/DH/BH.
        high: bool,
    },
    /// An immediate, masked to 32 bits (sign-extended first where the encoding says so).
    Imm(u32),
    /// A memory operand.
    Mem(Mem),
    /// A branch target.
    Rel(u32),
}

impl Operand {
    /// The register, if this is a register operand.
    pub fn reg(&self) -> Option<Reg> {
        match *self {
            Operand::Reg { reg, .. } => Some(reg),
            _ => None,
        }
    }

    /// The immediate or branch target.
    pub fn imm(&self) -> Option<u32> {
        match *self {
            Operand::Imm(v) | Operand::Rel(v) => Some(v),
            _ => None,
        }
    }

    /// The memory operand.
    pub fn mem(&self) -> Option<&Mem> {
        match self {
            Operand::Mem(m) => Some(m),
            _ => None,
        }
    }
}

/// Condition codes, in encoding order.
#[allow(missing_docs)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cc {
    O,
    No,
    B,
    Ae,
    E,
    Ne,
    Be,
    A,
    S,
    Ns,
    P,
    Np,
    L,
    Ge,
    Le,
    G,
}

impl Cc {
    fn from_index(n: u8) -> Cc {
        const ALL: [Cc; 16] =
            [Cc::O, Cc::No, Cc::B, Cc::Ae, Cc::E, Cc::Ne, Cc::Be, Cc::A, Cc::S, Cc::Ns, Cc::P, Cc::Np, Cc::L, Cc::Ge, Cc::Le, Cc::G];
        ALL[usize::from(n & 15)]
    }
}

/// Instruction kinds the extractor distinguishes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub enum Op {
    Mov,
    Movzx,
    Movsx,
    Lea,
    Push,
    Pop,
    Add,
    Or,
    Adc,
    Sbb,
    And,
    Sub,
    Xor,
    Cmp,
    Test,
    Inc,
    Dec,
    Neg,
    Not,
    Mul,
    /// One, two or three operand forms.
    Imul,
    Div,
    Idiv,
    Shl,
    Shr,
    Sar,
    /// Rotates and other shift-group members.
    Rotate,
    Xchg,
    Cdq,
    Setcc(Cc),
    Jcc(Cc),
    /// `loop`, `loope`, `loopne`, `jecxz`.
    Loop,
    Jmp,
    Call,
    Ret,
    Nop,
    Int3,
    Hlt,
    Ud2,
    /// A valid instruction the extractor has no semantics for.
    Other,
}

/// One decoded instruction.
#[derive(Clone, Copy, Debug)]
pub struct Insn {
    /// Address.
    pub addr: u32,
    /// Length in bytes.
    pub len: u8,
    /// Kind.
    pub op: Op,
    /// Operands (destination first).
    pub ops: [Option<Operand>; 3],
}

impl Insn {
    /// Address of the next instruction.
    pub fn next(&self) -> u32 {
        self.addr.wrapping_add(u32::from(self.len))
    }

    /// Operand `i`.
    pub fn op(&self, i: usize) -> Option<Operand> {
        self.ops[i]
    }

    /// Number of operands.
    pub fn nops(&self) -> usize {
        self.ops.iter().filter(|o| o.is_some()).count()
    }

    /// The direct branch/call target.
    pub fn target(&self) -> Option<u32> {
        match self.ops[0] {
            Some(Operand::Rel(t)) => Some(t),
            _ => None,
        }
    }

    /// True for `nop`, `int3` and the `lea r, [r+0]` / `mov edi, edi` filler MSVC pads with.
    pub fn is_padding(&self) -> bool {
        match self.op {
            Op::Nop | Op::Int3 => true,
            Op::Lea => matches!(
                (self.ops[0], self.ops[1]),
                (Some(Operand::Reg { reg, .. }), Some(Operand::Mem(Mem { base: Some(b), index: None, disp: 0, .. }))) if reg == b
            ),
            Op::Mov => matches!(
                (self.ops[0], self.ops[1]),
                (Some(Operand::Reg { reg: a, size: 4, .. }), Some(Operand::Reg { reg: b, size: 4, .. })) if a == b
            ),
            _ => false,
        }
    }
}

struct Cursor<'a> {
    b: &'a [u8],
    i: usize,
}

impl Cursor<'_> {
    fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.i)?;
        self.i += 1;
        Some(v)
    }
    fn i8(&mut self) -> Option<i32> {
        self.u8().map(|v| i32::from(v as i8))
    }
    fn u16(&mut self) -> Option<u16> {
        let v = self.b.get(self.i..self.i + 2)?;
        self.i += 2;
        Some(u16::from_le_bytes([v[0], v[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        let v = self.b.get(self.i..self.i + 4)?;
        self.i += 4;
        Some(u32::from_le_bytes([v[0], v[1], v[2], v[3]]))
    }
}

/// A decoded ModRM byte: `rm` operand and `reg` field.
struct ModRm {
    rm: Operand,
    reg: u8,
}

fn reg_operand(n: u8, size: u8) -> Operand {
    if size == 1 {
        Operand::Reg { reg: Reg::from_index(n & 3), size: 1, high: n & 4 != 0 }
    } else {
        Operand::Reg { reg: Reg::from_index(n), size, high: false }
    }
}

fn modrm(c: &mut Cursor<'_>, size: u8, addr16: bool) -> Option<ModRm> {
    let m = c.u8()?;
    let (md, reg, rm) = (m >> 6, (m >> 3) & 7, m & 7);
    if md == 3 {
        return Some(ModRm { rm: reg_operand(rm, size), reg });
    }
    if addr16 {
        // Sized only; 16-bit addressing never appears in the code the extractor reads.
        match (md, rm) {
            (0, 6) | (2, _) => {
                c.u16()?;
            }
            (1, _) => {
                c.u8()?;
            }
            _ => {}
        }
        return Some(ModRm { rm: Operand::Mem(Mem { base: None, index: None, scale: 1, disp: 0, size }), reg });
    }
    let mut mem = Mem { base: None, index: None, scale: 1, disp: 0, size };
    let mut no_base = false;
    if rm == 4 {
        let sib = c.u8()?;
        let (ss, idx, base) = (sib >> 6, (sib >> 3) & 7, sib & 7);
        if idx != 4 {
            mem.index = Some(Reg::from_index(idx));
            mem.scale = 1 << ss;
        }
        if base == 5 && md == 0 {
            no_base = true;
        } else {
            mem.base = Some(Reg::from_index(base));
        }
    } else if rm == 5 && md == 0 {
        no_base = true;
    } else {
        mem.base = Some(Reg::from_index(rm));
    }
    mem.disp = match md {
        0 if no_base => c.u32()? as i32,
        0 => 0,
        1 => c.i8()?,
        _ => c.u32()? as i32,
    };
    Some(ModRm { rm: Operand::Mem(mem), reg })
}

/// Decodes one instruction at `addr` from `bytes` (which start at `addr`).
pub fn decode(bytes: &[u8], addr: u32) -> Option<Insn> {
    let mut c = Cursor { b: bytes, i: 0 };
    let mut opsize16 = false;
    let mut addr16 = false;
    let mut b;
    loop {
        b = c.u8()?;
        match b {
            0x66 => opsize16 = true,
            0x67 => addr16 = true,
            0xf0 | 0xf2 | 0xf3 | 0x26 | 0x2e | 0x36 | 0x3e | 0x64 | 0x65 => {}
            _ => break,
        }
        if c.i > 8 {
            return None;
        }
    }
    let vsize: u8 = if opsize16 { 2 } else { 4 };
    let imm_v = |c: &mut Cursor<'_>| -> Option<u32> {
        if opsize16 {
            c.u16().map(|v| v as i16 as i32 as u32)
        } else {
            c.u32()
        }
    };
    let mut ops: [Option<Operand>; 3] = [None; 3];
    const ALU: [Op; 8] = [Op::Add, Op::Or, Op::Adc, Op::Sbb, Op::And, Op::Sub, Op::Xor, Op::Cmp];
    const SHIFT: [Op; 8] = [Op::Rotate, Op::Rotate, Op::Rotate, Op::Rotate, Op::Shl, Op::Shr, Op::Shl, Op::Sar];
    let op = match b {
        0x00..=0x3f if b & 7 < 6 => {
            let op = ALU[usize::from(b >> 3)];
            match b & 7 {
                0..=3 => {
                    let size = if b & 1 == 0 { 1 } else { vsize };
                    let m = modrm(&mut c, size, addr16)?;
                    let r = reg_operand(m.reg, size);
                    if b & 2 == 0 {
                        ops = [Some(m.rm), Some(r), None];
                    } else {
                        ops = [Some(r), Some(m.rm), None];
                    }
                }
                4 => ops = [Some(reg_operand(0, 1)), Some(Operand::Imm(c.i8()? as u32 & 0xff)), None],
                _ => ops = [Some(reg_operand(0, vsize)), Some(Operand::Imm(imm_v(&mut c)?)), None],
            }
            op
        }
        0x0f => return decode_0f(&mut c, addr, opsize16, addr16),
        0x00..=0x3f => Op::Other, // push/pop segment, daa/das/aaa/aas
        0x40..=0x47 => {
            ops[0] = Some(reg_operand(b & 7, vsize));
            Op::Inc
        }
        0x48..=0x4f => {
            ops[0] = Some(reg_operand(b & 7, vsize));
            Op::Dec
        }
        0x50..=0x57 => {
            ops[0] = Some(reg_operand(b & 7, vsize));
            Op::Push
        }
        0x58..=0x5f => {
            ops[0] = Some(reg_operand(b & 7, vsize));
            Op::Pop
        }
        0x60 | 0x61 => Op::Other,
        0x62 | 0x63 => {
            modrm(&mut c, vsize, addr16)?;
            Op::Other
        }
        0x68 => {
            ops[0] = Some(Operand::Imm(imm_v(&mut c)?));
            Op::Push
        }
        0x6a => {
            ops[0] = Some(Operand::Imm(c.i8()? as u32));
            Op::Push
        }
        0x69 | 0x6b => {
            let m = modrm(&mut c, vsize, addr16)?;
            let imm = if b == 0x69 { imm_v(&mut c)? } else { c.i8()? as u32 };
            ops = [Some(reg_operand(m.reg, vsize)), Some(m.rm), Some(Operand::Imm(imm))];
            Op::Imul
        }
        0x6c..=0x6f => Op::Other,
        0x70..=0x7f => {
            let d = c.i8()?;
            let next = addr.wrapping_add(c.i as u32);
            ops[0] = Some(Operand::Rel(next.wrapping_add(d as u32)));
            Op::Jcc(Cc::from_index(b))
        }
        0x80..=0x83 => {
            let size = if b & 1 == 0 { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            let imm = match b {
                0x81 => imm_v(&mut c)?,
                0x83 => c.i8()? as u32,
                _ => c.u8()? as u32,
            };
            ops = [Some(m.rm), Some(Operand::Imm(imm)), None];
            ALU[usize::from(m.reg)]
        }
        0x84..=0x8b => {
            let size = if b & 1 == 0 { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            let r = reg_operand(m.reg, size);
            ops = if b >= 0x8a { [Some(r), Some(m.rm), None] } else { [Some(m.rm), Some(r), None] };
            match b {
                0x84 | 0x85 => Op::Test,
                0x86 | 0x87 => Op::Xchg,
                _ => Op::Mov,
            }
        }
        0x8c | 0x8e => {
            modrm(&mut c, 2, addr16)?;
            Op::Other
        }
        0x8d => {
            let m = modrm(&mut c, vsize, addr16)?;
            ops = [Some(reg_operand(m.reg, vsize)), Some(m.rm), None];
            Op::Lea
        }
        0x8f => {
            let m = modrm(&mut c, vsize, addr16)?;
            ops[0] = Some(m.rm);
            Op::Pop
        }
        0x90 => Op::Nop,
        0x91..=0x97 => {
            ops = [Some(reg_operand(0, vsize)), Some(reg_operand(b & 7, vsize)), None];
            Op::Xchg
        }
        0x98 => Op::Other,
        0x99 => Op::Cdq,
        0x9a => {
            c.u32()?;
            c.u16()?;
            Op::Other
        }
        0x9b..=0x9f => Op::Other,
        0xa0..=0xa3 => {
            let size = if b & 1 == 0 { 1 } else { vsize };
            let a = if addr16 { u32::from(c.u16()?) } else { c.u32()? };
            let m = Operand::Mem(Mem { base: None, index: None, scale: 1, disp: a as i32, size });
            let r = reg_operand(0, size);
            ops = if b < 0xa2 { [Some(r), Some(m), None] } else { [Some(m), Some(r), None] };
            Op::Mov
        }
        0xa4..=0xa7 | 0xaa..=0xaf => Op::Other,
        0xa8 => {
            ops = [Some(reg_operand(0, 1)), Some(Operand::Imm(c.u8()? as u32)), None];
            Op::Test
        }
        0xa9 => {
            ops = [Some(reg_operand(0, vsize)), Some(Operand::Imm(imm_v(&mut c)?)), None];
            Op::Test
        }
        0xb0..=0xb7 => {
            ops = [Some(reg_operand(b & 7, 1)), Some(Operand::Imm(c.u8()? as u32)), None];
            Op::Mov
        }
        0xb8..=0xbf => {
            ops = [Some(reg_operand(b & 7, vsize)), Some(Operand::Imm(imm_v(&mut c)?)), None];
            Op::Mov
        }
        0xc0 | 0xc1 | 0xd0..=0xd3 => {
            let size = if b & 1 == 0 { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            let count = match b {
                0xc0 | 0xc1 => Operand::Imm(c.u8()? as u32),
                0xd0 | 0xd1 => Operand::Imm(1),
                _ => reg_operand(1, 1),
            };
            ops = [Some(m.rm), Some(count), None];
            SHIFT[usize::from(m.reg)]
        }
        0xc2 => {
            c.u16()?;
            Op::Ret
        }
        0xc3 => Op::Ret,
        0xc4 | 0xc5 => {
            modrm(&mut c, vsize, addr16)?;
            Op::Other
        }
        0xc6 | 0xc7 => {
            let size = if b == 0xc6 { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            let imm = if b == 0xc6 { c.u8()? as u32 } else { imm_v(&mut c)? };
            ops = [Some(m.rm), Some(Operand::Imm(imm)), None];
            Op::Mov
        }
        0xc8 => {
            c.u16()?;
            c.u8()?;
            Op::Other
        }
        0xc9 => Op::Other,
        0xca => {
            c.u16()?;
            Op::Other
        }
        0xcb => Op::Other,
        0xcc => Op::Int3,
        0xcd => {
            c.u8()?;
            Op::Other
        }
        0xce | 0xcf => Op::Other,
        0xd4 | 0xd5 => {
            c.u8()?;
            Op::Other
        }
        0xd6 | 0xd7 => Op::Other,
        0xd8..=0xdf => {
            modrm(&mut c, 4, addr16)?;
            Op::Other
        }
        0xe0..=0xe3 => {
            let d = c.i8()?;
            let next = addr.wrapping_add(c.i as u32);
            ops[0] = Some(Operand::Rel(next.wrapping_add(d as u32)));
            Op::Loop
        }
        0xe4..=0xe7 => {
            c.u8()?;
            Op::Other
        }
        0xe8 | 0xe9 => {
            let d = c.u32()?;
            let next = addr.wrapping_add(c.i as u32);
            ops[0] = Some(Operand::Rel(next.wrapping_add(d)));
            if b == 0xe8 {
                Op::Call
            } else {
                Op::Jmp
            }
        }
        0xea => {
            c.u32()?;
            c.u16()?;
            Op::Other
        }
        0xeb => {
            let d = c.i8()?;
            let next = addr.wrapping_add(c.i as u32);
            ops[0] = Some(Operand::Rel(next.wrapping_add(d as u32)));
            Op::Jmp
        }
        0xec..=0xef | 0xf1 | 0xf5 | 0xf8..=0xfd => Op::Other,
        0xf4 => Op::Hlt,
        0xf6 | 0xf7 => {
            let size = if b == 0xf6 { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            match m.reg {
                0 | 1 => {
                    let imm = if b == 0xf6 { c.u8()? as u32 } else { imm_v(&mut c)? };
                    ops = [Some(m.rm), Some(Operand::Imm(imm)), None];
                    Op::Test
                }
                r => {
                    ops[0] = Some(m.rm);
                    [Op::Test, Op::Test, Op::Not, Op::Neg, Op::Mul, Op::Imul, Op::Div, Op::Idiv][usize::from(r)]
                }
            }
        }
        0xfe | 0xff => {
            let size = if b == 0xfe { 1 } else { vsize };
            let m = modrm(&mut c, size, addr16)?;
            ops[0] = Some(m.rm);
            match (b, m.reg) {
                (_, 0) => Op::Inc,
                (_, 1) => Op::Dec,
                (0xff, 2) => Op::Call,
                (0xff, 4) => Op::Jmp,
                (0xff, 6) => Op::Push,
                (0xff, 3) | (0xff, 5) => Op::Other,
                _ => return None,
            }
        }
        _ => return None,
    };
    Some(Insn { addr, len: c.i as u8, op, ops })
}

fn decode_0f(c: &mut Cursor<'_>, addr: u32, opsize16: bool, addr16: bool) -> Option<Insn> {
    let vsize: u8 = if opsize16 { 2 } else { 4 };
    let b = c.u8()?;
    let mut ops: [Option<Operand>; 3] = [None; 3];
    let op = match b {
        0x80..=0x8f => {
            let d = if opsize16 { u32::from(c.u16()?) as i16 as i32 as u32 } else { c.u32()? };
            let next = addr.wrapping_add(c.i as u32);
            ops[0] = Some(Operand::Rel(next.wrapping_add(d)));
            Op::Jcc(Cc::from_index(b))
        }
        0x90..=0x9f => {
            let m = modrm(c, 1, addr16)?;
            ops[0] = Some(m.rm);
            Op::Setcc(Cc::from_index(b))
        }
        0xaf => {
            let m = modrm(c, vsize, addr16)?;
            ops = [Some(reg_operand(m.reg, vsize)), Some(m.rm), None];
            Op::Imul
        }
        0xb6 | 0xb7 | 0xbe | 0xbf => {
            let m = modrm(c, if b & 1 == 0 { 1 } else { 2 }, addr16)?;
            ops = [Some(reg_operand(m.reg, vsize)), Some(m.rm), None];
            if b < 0xb8 {
                Op::Movzx
            } else {
                Op::Movsx
            }
        }
        0x0b => Op::Ud2,
        0xc8..=0xcf | 0x05..=0x09 | 0x30..=0x35 | 0x77 | 0xa0..=0xa2 | 0xa8..=0xaa => Op::Other,
        0x70..=0x73 | 0xa4 | 0xac | 0xba | 0xc2 | 0xc4..=0xc6 => {
            modrm(c, vsize, addr16)?;
            c.u8()?;
            Op::Other
        }
        0x38 | 0x3a | 0x0f => return None,
        _ => {
            modrm(c, vsize, addr16)?;
            Op::Other
        }
    };
    Some(Insn { addr, len: c.i as u8, op, ops })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(bytes: &[u8]) -> Insn {
        decode(bytes, 0x1000).unwrap()
    }

    #[test]
    fn decodes_common_forms() {
        // push 0xa8a880c
        let i = d(&[0x68, 0x0c, 0x88, 0x8a, 0x0a]);
        assert_eq!((i.op, i.len, i.op(0)), (Op::Push, 5, Some(Operand::Imm(0x0a8a880c))));
        // mov dword ptr [0xadc7dc], 0
        let i = d(&[0xc7, 0x05, 0xdc, 0xc7, 0xad, 0x00, 0, 0, 0, 0]);
        assert_eq!(i.len, 10);
        assert_eq!(i.op(0).and_then(|o| o.mem().map(|m| (m.base, m.disp))), Some((None, 0xadc7dc)));
        // call rel32
        let i = d(&[0xe8, 0xfc, 0xff, 0xff, 0xff]);
        assert_eq!((i.op, i.target()), (Op::Call, Some(0x1001)));
        // add esp, 0x28
        let i = d(&[0x83, 0xc4, 0x28]);
        assert_eq!((i.op, i.op(0).and_then(|o| o.reg()), i.op(1)), (Op::Add, Some(Reg::Esp), Some(Operand::Imm(0x28))));
        // or eax, -1
        assert_eq!(d(&[0x83, 0xc8, 0xff]).op(1), Some(Operand::Imm(0xffff_ffff)));
        // jmp dword ptr [eax*4 + 0x5d5e68]
        let i = d(&[0xff, 0x24, 0x85, 0x68, 0x5e, 0x5d, 0x00]);
        let m = *i.op(0).unwrap().mem().unwrap();
        assert_eq!((i.op, m.base, m.index, m.scale, m.disp), (Op::Jmp, None, Some(Reg::Eax), 4, 0x5d5e68));
        // setle dl
        let i = d(&[0x0f, 0x9e, 0xc2]);
        assert_eq!((i.op, i.op(0)), (Op::Setcc(Cc::Le), Some(Operand::Reg { reg: Reg::Edx, size: 1, high: false })));
        // mov dl, byte ptr [ecx + 0x5e0ef4]
        let i = d(&[0x8a, 0x91, 0xf4, 0x0e, 0x5e, 0x00]);
        assert_eq!(i.op(1).unwrap().mem().map(|m| (m.base, m.size, m.disp)), Some((Some(Reg::Ecx), 1, 0x5e0ef4)));
        // imul ecx (one operand), idiv ecx, sar esi, 1
        assert_eq!(d(&[0xf7, 0xe9]).op, Op::Imul);
        assert_eq!(d(&[0xf7, 0xf9]).op, Op::Idiv);
        let i = d(&[0xd1, 0xfe]);
        assert_eq!((i.op, i.op(1)), (Op::Sar, Some(Operand::Imm(1))));
        // mov ecx, [esp + 8]
        let i = d(&[0x8b, 0x4c, 0x24, 0x08]);
        assert_eq!(i.op(1).unwrap().mem().map(|m| (m.base, m.disp)), Some((Some(Reg::Esp), 8)));
        // fld dword ptr [eax] is sized but opaque
        assert_eq!((d(&[0xd9, 0x00]).op, d(&[0xd9, 0x00]).len), (Op::Other, 2));
        // lea esp, [esp] padding
        assert!(d(&[0x8d, 0x64, 0x24, 0x00]).is_padding());
    }
}
