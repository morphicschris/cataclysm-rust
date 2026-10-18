"""Shared helpers for the Cataclysm KAS mission tools: PE access, capstone disassembly of the
compiled mission block, function splitting, runtime-function names.

Nothing here writes outside the caller-supplied output paths; the exe is opened read-only.
"""
import json
import os
import struct
import urllib.parse
import urllib.request

import capstone

EXE = r'D:\Games\Homeworld Cataclysm\Cataclysm\cataclysm.exe'
ROOT = r'D:\Git\HWC_Modern'
BLOCK_LO, BLOCK_HI = 0x5d1930, 0x66b330
GHIDRA = 'http://127.0.0.1:8089'

# well-known runtime globals
G_SCOPE = 0x00adc7dc          # CurrentMissionScope   (0 mission, 1 FSM, 2 state)
G_SCOPENAME = 0x00adc7e0      # CurrentMissionScopeName  (hash)
G_THISTEAM = 0x008e9d98       # CurrentTeamP
G_LANGUAGE = None             # strCurLanguage (filled in by discovery)


class PE:
    def __init__(self, path=EXE):
        self.d = open(path, 'rb').read()
        d = self.d
        pe = struct.unpack_from('<I', d, 0x3c)[0]
        nsec = struct.unpack_from('<H', d, pe + 6)[0]
        optsz = struct.unpack_from('<H', d, pe + 20)[0]
        self.base = struct.unpack_from('<I', d, pe + 24 + 28)[0]
        s = pe + 24 + optsz
        self.secs = []
        for i in range(nsec):
            n, vs, va, rs, rp = struct.unpack_from('<8sIIII', d, s + 40 * i)
            self.secs.append((n.rstrip(b'\0').decode(), self.base + va, vs, rp, rs))

    def off(self, va):
        for n, a, vs, rp, rs in self.secs:
            if a <= va < a + max(vs, rs):
                o = va - a
                return rp + o if o < rs else None
        return None

    def read(self, va, n):
        o = self.off(va)
        if o is None:
            return b'\0' * n
        return self.d[o:o + n]

    def u32(self, va):
        return struct.unpack('<I', self.read(va, 4))[0]

    def cstr(self, va, limit=4096):
        o = self.off(va)
        if o is None:
            return None
        e = self.d.find(b'\0', o, o + limit)
        if e < 0:
            return None
        return self.d[o:e].decode('latin1')

    def section(self, va):
        for n, a, vs, rp, rs in self.secs:
            if a <= va < a + max(vs, rs):
                return n
        return None


_md = None


def md():
    global _md
    if _md is None:
        _md = capstone.Cs(capstone.CS_ARCH_X86, capstone.CS_MODE_32)
        _md.detail = True
    return _md


def disasm(pe, lo, hi):
    code = pe.read(lo, hi - lo)
    return list(md().disasm(code, lo))


def split_functions(insns):
    """kas2c functions are emitted back to back, MSVC pads with NOP/INT3 to 16 bytes.
    A function starts at the first non-padding instruction after a RET/JMP (unconditional)."""
    funcs, cur, after_ret = [], [], True
    for ins in insns:
        if ins.mnemonic in ('nop', 'int3') or (ins.mnemonic == 'lea' and ins.op_str in ('esp, [esp]', 'ecx, [ecx]')):
            if after_ret:
                continue
        if after_ret and cur:
            # a new function only begins on a 16-byte boundary; otherwise it is a tail after an early ret
            if ins.address % 16 == 0:
                funcs.append(cur)
                cur = []
        after_ret = False
        cur.append(ins)
        if ins.mnemonic in ('ret', 'jmp') and not _is_internal_jmp(ins, cur):
            after_ret = True
    if cur:
        funcs.append(cur)
    return funcs


def _is_internal_jmp(ins, cur):
    if ins.mnemonic != 'jmp':
        return False
    try:
        t = int(ins.op_str, 16)
    except ValueError:
        return False
    # a jmp to a runtime function (tail call) ends the function; a jmp inside the block is a branch
    return BLOCK_LO <= t < BLOCK_HI and t >= cur[0].address - 0x20000 and not (0x48b000 <= t < 0x4a0000)


def ghidra_get(path, **q):
    q['program'] = 'cataclysm.exe'
    return json.load(urllib.request.urlopen(GHIDRA + path + '?' + urllib.parse.urlencode(q), timeout=600))


def load_function_names(cache=None):
    if cache and os.path.exists(cache):
        return {int(k, 16): v for k, v in json.load(open(cache)).items()}
    fs = ghidra_get('/find_functions', limit=100000)['functions']
    names = {int(f['address'], 16): f['name'] for f in fs}
    if cache:
        json.dump({'%08x' % k: v for k, v in names.items()}, open(cache, 'w'))
    return names


def callee_arity(pe, addr, max_insns=4000):
    """Number of 4-byte stack arguments a cdecl/stdcall function reads, by following its code
    (recursive descent) and tracking ESP. Returns (arity, stdcall_pop_bytes)."""
    m = md()
    seen = {}
    work = [(addr, 0, False)]   # (address, esp delta (bytes pushed), ebp_frame)
    maxarg = -1
    retpop = 0
    count = 0
    while work and count < max_insns:
        a, delta, frame = work.pop()
        while True:
            if a in seen or count >= max_insns:
                break
            seen[a] = delta
            code = pe.read(a, 16)
            try:
                ins = next(m.disasm(code, a))
            except StopIteration:
                break
            count += 1
            mn = ins.mnemonic
            for op in ins.operands:
                if op.type == capstone.x86.X86_OP_MEM:
                    base = ins.reg_name(op.mem.base) if op.mem.base else None
                    if base == 'esp' and op.mem.index == 0:
                        off = op.mem.disp - delta
                        if off >= 4:
                            maxarg = max(maxarg, (off - 4) // 4)
                    elif base == 'ebp' and frame and op.mem.index == 0 and op.mem.disp >= 8:
                        maxarg = max(maxarg, (op.mem.disp - 8) // 4)
            if mn == 'push':
                delta += 4
                if ins.op_str == 'ebp':
                    pass
            elif mn == 'pop':
                delta -= 4
            elif mn == 'pushal':
                delta += 32
            elif mn == 'popal':
                delta -= 32
            elif mn == 'mov' and ins.op_str == 'ebp, esp':
                frame = True
            elif mn in ('sub', 'add') and ins.op_str.startswith('esp, '):
                try:
                    v = int(ins.op_str[5:], 0)
                    delta += v if mn == 'sub' else -v
                except ValueError:
                    pass
            if mn == 'ret':
                if ins.op_str:
                    retpop = int(ins.op_str, 0)
                break
            if mn == 'jmp':
                try:
                    a = int(ins.op_str, 16)
                    continue
                except ValueError:
                    break
            if mn.startswith('j') or mn.startswith('loop'):
                try:
                    work.append((int(ins.op_str, 16), delta, frame))
                except ValueError:
                    pass
            if mn == 'call':
                # assume callee is cdecl: caller cleans up
                pass
            a = ins.address + ins.size
    return maxarg + 1, retpop
