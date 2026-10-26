//! The compiled form of an effect: three byte-addressed code blocks with the exact layout the
//! original compiler emits (32-bit little-endian words, same structure sizes).
//!
//! Keeping the original layout matters because the scripts' control flow is expressed in byte
//! offsets: branch targets, `if`/`at`/`between` skip lengths, `alternate` jump tables and labels
//! (which are just code offsets that scripts can even pass around as numbers). Pointers the
//! original stored in the code (function addresses, texture handles, meshes, effect statics)
//! are replaced by indices into the tables of [`crate::compiler::Library`].
//!
//! Opcode layouts (all fields are dwords unless noted):
//! * `Nop`, `Delete`, `End`, `Yield`, `Return`: `op` (4 bytes)
//! * `Before/After/At/Every[Var]`: `op, skipBytes, time|varOffset` (12)
//! * `Between`: `op, start, end, startVar, endVar, skipBytes` (24)
//! * `Effector*` (time-index effectors): `op, end, varOffset, rateOffset, effectorID:u8` (20)
//! * `Goto`, `Call`: `op, codeBlock, target` (12)
//! * conditionals: `op, param0, param1, skipBytes` (16)
//! * `VariableCopy`/`VariableAssign`: `op, source, dest` (12)
//! * `Function`: `op, function, nParams, returnVar, passThis, {type, value} * nParams` (20 + 8n)
//! * `Alternate`: `op, criteria, tableLength, skipBytes, offsetTable` (20)

/// Opcode numbers (`etgopcode`).
pub mod op {
    pub const NOP: u32 = 0;
    pub const BEFORE: u32 = 1;
    pub const BEFORE_VAR: u32 = 2;
    pub const AFTER: u32 = 3;
    pub const AFTER_VAR: u32 = 4;
    pub const AT: u32 = 5;
    pub const AT_VAR: u32 = 6;
    pub const BETWEEN: u32 = 7;
    pub const EVERY: u32 = 8;
    pub const EVERY_VAR: u32 = 9;
    pub const EFFECTOR_VI: u32 = 26;
    pub const EFFECTOR_VF: u32 = 27;
    pub const EFFECTOR_VCO: u32 = 28;
    pub const EFFECTOR_VCA: u32 = 29;
    pub const EFFECTOR_CI: u32 = 30;
    pub const EFFECTOR_CF: u32 = 31;
    pub const EFFECTOR_CCO: u32 = 32;
    pub const EFFECTOR_CCA: u32 = 33;
    pub const DELETE: u32 = 34;
    pub const END: u32 = 35;
    pub const YIELD: u32 = 36;
    pub const GOTO: u32 = 37;
    pub const CALL: u32 = 38;
    pub const RETURN: u32 = 39;
    pub const EQUAL_VCI: u32 = 42;
    pub const NOT_EQUAL_VCI: u32 = 43;
    pub const GREATER_VCI: u32 = 44;
    pub const GREATER_EQUAL_VCI: u32 = 45;
    pub const LESS_VCI: u32 = 46;
    pub const LESS_EQUAL_VCI: u32 = 47;
    pub const EQUAL_VVI: u32 = 48;
    pub const LESS_EQUAL_VVI: u32 = 53;
    pub const EQUAL_VCF: u32 = 54;
    pub const LESS_EQUAL_VCF: u32 = 59;
    pub const EQUAL_VVF: u32 = 60;
    pub const LESS_EQUAL_VVF: u32 = 65;
    pub const ZERO_VI: u32 = 66;
    pub const NOT_ZERO_VF: u32 = 69;
    pub const VARIABLE_COPY: u32 = 70;
    pub const VARIABLE_ASSIGN: u32 = 71;
    pub const FUNCTION: u32 = 72;
    pub const ALTERNATE: u32 = 73;
    pub const CASE: u32 = 74;
    pub const LAST_OP: u32 = 75;
    /// Flag on `Goto`/`Call` whose label is resolved after the whole file is read.
    pub const RESOLVE: u32 = 0x4000;
}

/// Structure sizes.
pub const SIZE_PROCESS: u32 = 4;
pub const SIZE_AT: u32 = 12;
pub const SIZE_BETWEEN: u32 = 24;
pub const SIZE_SEFFECTOR: u32 = 20;
pub const SIZE_BRANCH: u32 = 12;
pub const SIZE_CONDITIONAL: u32 = 16;
pub const SIZE_VARCOPY: u32 = 12;
pub const SIZE_DECISION: u32 = 20;

/// `etgFunctionSize(n)`.
pub const fn function_size(n: u32) -> u32 {
    20 + 8 * n
}

/// The handler-table length of an opcode (`etgHandleTable[op].length`), used when walking
/// code. Function calls are variable-length and handled separately.
pub fn op_length(opcode: u32) -> u32 {
    match opcode {
        0 => 4,
        1..=6 | 8 | 9 => 12,
        7 => 24,
        10..=25 => 24,
        26..=33 => 20,
        34..=36 => 4,
        37 | 38 => 12,
        39..=41 => 4,
        42..=69 => 16,
        70 | 71 => 12,
        72 => 28,
        73 | 74 => 20,
        _ => 0,
    }
}

/// Parameter types (`EVT_*`).
pub mod evt {
    pub const VOID: u8 = 0;
    pub const INT: u8 = 1;
    pub const FLOAT: u8 = 2;
    pub const RGB: u8 = 3;
    pub const RGBA: u8 = 4;
    pub const POINTER: u8 = 5;
    pub const CONSTANT: u8 = 6;
    pub const LABEL: u8 = 7;
    pub const CONST_LABEL: u8 = 8;
    pub const VAR_LABEL: u8 = 9;
}

/// Code block indices (`EPM_*`).
pub const BLOCK_STARTUP: usize = 0;
pub const BLOCK_EACH_FRAME: usize = 1;
pub const BLOCK_TIME_INDEX: usize = 2;

/// Read a dword.
#[inline]
pub fn rd(code: &[u8], at: u32) -> u32 {
    let a = at as usize;
    u32::from_le_bytes([code[a], code[a + 1], code[a + 2], code[a + 3]])
}

/// Read a dword, or 0 past the end (the original would read whatever follows the block).
#[inline]
pub fn rd_or0(code: &[u8], at: u32) -> u32 {
    let a = at as usize;
    if a + 4 <= code.len() {
        rd(code, at)
    } else {
        0
    }
}

/// Write a dword, growing the buffer as needed.
#[inline]
pub fn wr(code: &mut Vec<u8>, at: u32, v: u32) {
    let a = at as usize;
    if code.len() < a + 4 {
        code.resize(a + 4, 0);
    }
    code[a..a + 4].copy_from_slice(&v.to_le_bytes());
}

/// One decoded instruction, for inspection and debugging.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Instr {
    Nop,
    TimeTest { opcode: u32, skip: u32, operand: u32 },
    Between { start: f32, end: f32, start_var: u32, end_var: u32, skip: u32 },
    Effector { opcode: u32, end: u32, var: u32, rate: u32, id: u8 },
    Delete,
    End,
    Yield,
    Branch { opcode: u32, block: u32, target: u32 },
    Return,
    Cond { opcode: u32, p0: u32, p1: u32, skip: u32 },
    VarCopy { source: u32, dest: u32 },
    VarAssign { value: u32, dest: u32 },
    Call { function: u32, ret: u32, pass_this: bool, params: Vec<(u32, u32)> },
    Alternate { criteria: u32, table_len: u32, skip: u32, table: u32 },
    Unknown(u32),
}

/// Decode the instruction at `at`; returns it with its length.
pub fn decode(code: &[u8], at: u32) -> (Instr, u32) {
    let opc = rd(code, at) & !op::RESOLVE;
    let w = |k: u32| rd_or0(code, at + 4 * k);
    match opc {
        op::NOP => (Instr::Nop, 4),
        1..=6 | 8 | 9 => (Instr::TimeTest { opcode: opc, skip: w(1), operand: w(2) }, 12),
        op::BETWEEN => (
            Instr::Between {
                start: f32::from_bits(w(1)),
                end: f32::from_bits(w(2)),
                start_var: w(3),
                end_var: w(4),
                skip: w(5),
            },
            24,
        ),
        26..=33 => (
            Instr::Effector { opcode: opc, end: w(1), var: w(2), rate: w(3), id: (w(4) & 0xff) as u8 },
            20,
        ),
        op::DELETE => (Instr::Delete, 4),
        op::END => (Instr::End, 4),
        op::YIELD => (Instr::Yield, 4),
        op::GOTO | op::CALL => (Instr::Branch { opcode: opc, block: w(1), target: w(2) }, 12),
        op::RETURN => (Instr::Return, 4),
        42..=69 => (Instr::Cond { opcode: opc, p0: w(1), p1: w(2), skip: w(3) }, 16),
        op::VARIABLE_COPY => (Instr::VarCopy { source: w(1), dest: w(2) }, 12),
        op::VARIABLE_ASSIGN => (Instr::VarAssign { value: w(1), dest: w(2) }, 12),
        op::FUNCTION => {
            let n = w(2);
            let params = (0..n).map(|i| (w(5 + 2 * i), w(6 + 2 * i))).collect();
            (Instr::Call { function: w(1), ret: w(3), pass_this: w(4) != 0, params }, function_size(n))
        }
        op::ALTERNATE => (Instr::Alternate { criteria: w(1), table_len: w(2), skip: w(3), table: w(4) }, 20),
        other => (Instr::Unknown(other), op_length(other).max(4)),
    }
}

/// Decode a whole code block into `(offset, instruction)` pairs.
pub fn disassemble(code: &[u8]) -> Vec<(u32, Instr)> {
    let mut out = Vec::new();
    let mut at = 0u32;
    while (at as usize) + 4 <= code.len() {
        let (i, len) = decode(code, at);
        out.push((at, i));
        at += len;
    }
    out
}
