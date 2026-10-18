"""Translate Homeworld: Cataclysm compiled KAS missions (kas2c output linked into cataclysm.exe)
back into KAS-like source.

Works directly on the machine code (capstone), not on the Ghidra decompiler output: the
kas2c/MSVC code is regular enough that a small symbolic executor + structurer recovers the
original IF/ELSE/IFONCE/JUMP structure, and it avoids Ghidra's bad no-return/function-boundary
guesses in this block.  Ghidra (MCP on 127.0.0.1:8089) is only used for function names of the
runtime (cached) and, optionally, for renaming the mission functions (--rename).

Usage:
    kas_decompile.py --list                      list missions found via the mission selector
    kas_decompile.py Mission01 [-o out.kas]      translate one mission
    kas_decompile.py --all                       translate every mission into recovered/missions/
    options: --rename (rename mission functions in Ghidra), --names FILE (extra name candidates)
"""
import argparse
import collections
import json
import os
import re
import struct
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import capstone                                               # noqa: E402
from capstone import x86                                      # noqa: E402
from kas_scan import PE, md, load_function_names, callee_arity, BLOCK_LO, BLOCK_HI, ROOT, ghidra_get  # noqa: E402
from kas_hash import kas_hash                                 # noqa: E402

OUTDIR = os.path.join(ROOT, 'recovered', 'missions')
DATADIR = os.path.join(OUTDIR, 'data')

A_TEAM = 0x008e9d98            # CurrentTeamP (kasThisTeamPtr)
A_SCOPE = 0x00adc7dc           # CurrentMissionScope
A_SCOPENAME = 0x00adc7e0       # CurrentMissionScopeName (hash)
A_LANG = 0x00910464            # strCurLanguage
N_LANG = 3                     # Cataclysm LSTRINGs: English, French, German
A_MISSIONSTART = 0x0048bd90
A_JUMP = 0x0048bc60
A_FSMCREATE = 0x0048bc80
A_MSGGET = 0x00417ed0          # aitMsgGet(team, index): remove+return queued message
A_MSGSEND_RAW = 0x00417e20     # aitMsgSend(sender, team, msg)
SELECTOR_FUNCS = (0x005148f0, 0x0052a740)   # singlePlayerKasMissionStart, tutInitTutorial

# name of the hashed-label accessors (kas2c "param" keywords)
LABEL_FUNCS = {
    0x0048c1d0: 'SHIPS', 0x0048bdc0: 'TEAM', 0x0048be00: 'TEAMSHIPS', 0x0048c110: 'VOLUME',
    0x0048c0c0: 'PATH', 0x0048c080: 'PATH', 0x0048c150: 'POINT', 0x0048be40: 'SHIPSPOINT',
    0x0048bee0: 'TEAMSPOINT', 0x0048bf80: 'VOLUMEPOINT',
}
A_THISTEAMSPOINT = 0x0048c010

REG32 = {}
for r in ('eax', 'ecx', 'edx', 'ebx', 'esi', 'edi', 'ebp'):
    REG32[r] = r
for r8, r32 in (('al', 'eax'), ('ah', 'eax'), ('ax', 'eax'), ('cl', 'ecx'), ('ch', 'ecx'), ('cx', 'ecx'),
                ('dl', 'edx'), ('dh', 'edx'), ('dx', 'edx'), ('bl', 'ebx'), ('bh', 'ebx'), ('bx', 'ebx'),
                ('si', 'esi'), ('di', 'edi'), ('bp', 'ebp')):
    REG32[r8] = r32
TAB, NL = chr(9), chr(10)
GPRS = ('eax', 'ecx', 'edx', 'ebx', 'esi', 'edi', 'ebp')


# ----------------------------------------------------------------------------------------------
# values
# ----------------------------------------------------------------------------------------------
class V:
    __slots__ = ()

    def key(self):
        return repr(self)


class Const(V):
    __slots__ = ('v',)

    def __init__(self, v):
        self.v = v & 0xffffffff

    @property
    def s(self):
        return self.v - (1 << 32) if self.v & 0x80000000 else self.v

    def __repr__(self):
        return 'C%x' % self.v


class Sym(V):
    __slots__ = ('name',)

    def __init__(self, name):
        self.name = name

    def __repr__(self):
        return 'S:' + self.name


_callid = [0]


class Call(V):
    __slots__ = ('addr', 'args', 'id', 'at')

    def __init__(self, addr, args, at):
        self.addr, self.args, self.at = addr, args, at
        _callid[0] += 1
        self.id = _callid[0]

    def __repr__(self):
        return 'F%x@%x(%s)' % (self.addr, self.at, ','.join(map(repr, self.args)))


class Mem(V):
    __slots__ = ('addr',)

    def __init__(self, addr):
        self.addr = addr

    def __repr__(self):
        return 'M%x' % self.addr


class Field(V):
    __slots__ = ('base', 'off')

    def __init__(self, base, off):
        self.base, self.off = base, off

    def __repr__(self):
        return '[%r+%x]' % (self.base, self.off)


class Index(V):
    __slots__ = ('table', 'idx', 'scale')

    def __init__(self, table, idx, scale):
        self.table, self.idx, self.scale = table, idx, scale

    def __repr__(self):
        return '%x[%r*%d]' % (self.table, self.idx, self.scale)


class Bin(V):
    __slots__ = ('op', 'a', 'b')

    def __init__(self, op, a, b):
        self.op, self.a, self.b = op, a, b

    def __repr__(self):
        return '(%r%s%r)' % (self.a, self.op, self.b)


class Un(V):
    __slots__ = ('op', 'a')

    def __init__(self, op, a):
        self.op, self.a = op, a

    def __repr__(self):
        return '%s(%r)' % (self.op, self.a)


class Unk(V):
    __slots__ = ('d',)

    def __init__(self, d):
        self.d = d

    def __repr__(self):
        return '?' + self.d


def _is_sgn_of(y, x):
    if isinstance(y, Un) and y.op == 'sgn':
        return y.a.key() == x.key()
    if isinstance(y, Bin) and y.op == '&' and isinstance(y.b, Const):
        return _is_sgn_of(y.a, x)
    return False


def _magic(v):
    # (mulhi(x, C) >> k)  ->  (x, C, k)
    k = 0
    if isinstance(v, Bin) and v.op == '>>' and isinstance(v.b, Const):
        k, v = v.b.v, v.a
    if isinstance(v, Bin) and v.op == 'mulhi' and isinstance(v.b, Const):
        return v.a, v.b.v, k
    return None


def mk_bin(op, a, b):
    # MSVC division by a constant: q = (mulhi(x,C) >> k); q += q >> 31   ->  x / round(2^(32+k) / C)
    if op == '+' and isinstance(b, Bin) and b.op == '>>' and isinstance(b.b, Const) and b.b.v == 31:
        ma = _magic(a)
        if ma is not None and b.a.key() == a.key():
            x, c, k = ma
            if c & 0x80000000:
                c -= 1 << 32
            if c:
                return Bin('/', x, Const(round((1 << (32 + k)) / c)))
    # MSVC signed division by 2^k:  (x - sgn(x)) >> k   /  (x + (sgn(x) & m)) >> k
    if op == '>>' and isinstance(b, Const) and isinstance(a, Bin) and a.op in ('+', '-') and _is_sgn_of(a.b, a.a):
        return Bin('/', a.a, Const(1 << b.v))
    if isinstance(a, Const) and isinstance(b, Const):
        x, y = a.s, b.s
        try:
            r = {'+': x + y, '-': x - y, '*': x * y, '&': x & y, '|': x | y, '^': x ^ y,
                 '<<': x << (y & 31), '>>': x >> (y & 31), '/': int(x / y) if y else None}[op]
            if r is not None:
                return Const(r)
        except KeyError:
            pass
    if op in ('+', '-') and isinstance(b, Const) and b.v == 0:
        return a
    if op == '+' and isinstance(a, Const) and a.v == 0:
        return b
    if op == '-' and isinstance(a, Bin) and a.op == '*' and isinstance(a.b, Const) and a.a.key() == b.key():
        return Bin('*', b, Const(a.b.s - 1))
    if op == '+' and isinstance(a, Bin) and a.op in ('+', '-') and isinstance(a.b, Const) and isinstance(b, Const):
        return mk_bin(a.op, a.a, Const(a.b.s + (b.s if a.op == '+' else -b.s)))
    if op == '-' and isinstance(a, Bin) and a.op in ('+', '-') and isinstance(a.b, Const) and isinstance(b, Const):
        return mk_bin(a.op, a.a, Const(a.b.s + (-b.s if a.op == '+' else b.s)))
    return Bin(op, a, b)


# boolean conditions
class Cond:
    __slots__ = ('op', 'a', 'b')

    def __init__(self, op, a, b=None):
        self.op, self.a, self.b = op, a, b

    def __repr__(self):
        return '<%s %r %r>' % (self.op, self.a, self.b)


NEG_REL = {'==': '!=', '!=': '==', '<': '>=', '>=': '<', '>': '<=', '<=': '>', 'u<': 'u>=', 'u>=': 'u<',
           'u>': 'u<=', 'u<=': 'u>'}


def rel(op, a, b):
    # normalise (x - c) op k  ->  x op (k + c);  (x + c) op k -> x op (k - c)  (exact for == / !=)
    while isinstance(a, Bin) and a.op in ('+', '-') and isinstance(a.b, Const) and isinstance(b, Const):
        b = Const(b.s + a.b.s if a.op == '-' else b.s - a.b.s)
        a = a.a
    return Cond(op, a, b)


def cnot(c):
    if c.op in NEG_REL:
        return Cond(NEG_REL[c.op], c.a, c.b)
    if c.op == 'not':
        return c.a
    if c.op == 'and':
        return Cond('or', cnot(c.a), cnot(c.b))
    if c.op == 'or':
        return Cond('and', cnot(c.a), cnot(c.b))
    if c.op == 'true':
        return Cond('false', None)
    if c.op == 'false':
        return Cond('true', None)
    return Cond('not', c)


def cand(a, b):
    if a.op == 'true':
        return b
    if b.op == 'true':
        return a
    return Cond('and', a, b)


def cor(a, b):
    if a.op == 'false':
        return b
    if b.op == 'false':
        return a
    return Cond('or', a, b)


def walk_values(v):
    yield v
    if isinstance(v, Call):
        for a in v.args:
            yield from walk_values(a)
    elif isinstance(v, (Bin,)):
        yield from walk_values(v.a)
        yield from walk_values(v.b)
    elif isinstance(v, Un):
        yield from walk_values(v.a)
    elif isinstance(v, Field):
        yield from walk_values(v.base)
    elif isinstance(v, Index):
        yield from walk_values(v.idx)
    elif isinstance(v, Cond):
        if isinstance(v.a, (V, Cond)):
            yield from walk_values(v.a)
        if isinstance(v.b, (V, Cond)):
            yield from walk_values(v.b)


# ----------------------------------------------------------------------------------------------
# semantic graph
# ----------------------------------------------------------------------------------------------
class Node:
    __slots__ = ('id', 'addr', 'stmts', 'term', 'preds')

    def __init__(self, nid, addr):
        self.id, self.addr, self.stmts, self.term, self.preds = nid, addr, [], None, []

    def succs(self):
        t = self.term
        if t is None:
            return []
        k = t[0]
        if k == 'goto':
            return [t[1]]
        if k == 'if':
            return [t[2], t[3]]
        if k == 'switch':
            return list(dict.fromkeys(list(t[2].values()) + ([t[3]] if t[3] is not None else [])))
        if k == 'msgloop':
            return [t[1], t[2]]
        return []


class State:
    __slots__ = ('regs', 'stack', 'flags', 'carry', 'loop')

    def __init__(self):
        self.regs = {r: Unk('in_' + r) for r in GPRS}
        self.stack = []          # list of [value, consumed]
        self.flags = None
        self.carry = None
        self.loop = None         # (header_addr) when inside a message loop

    def copy(self):
        s = State()
        s.regs = dict(self.regs)
        s.stack = [list(e) for e in self.stack]
        s.flags, s.carry, s.loop = self.flags, self.carry, self.loop
        return s


class DecompileError(Exception):
    pass


class Runtime:
    """runtime function names, KAS table, arities."""

    def __init__(self, pe, names):
        self.pe, self.names = pe, names
        self.table = {}
        tpath = os.path.join(DATADIR, 'kas_function_table.json')
        for e in json.load(open(tpath)):
            self.table.setdefault(e['c'], e)
        self.cname = {}
        rpath = os.path.join(DATADIR, 'runtime_names.tsv')
        if os.path.exists(rpath):
            for line in open(rpath, encoding='utf-8'):
                p = line.rstrip('\n').split('\t')
                if len(p) >= 2 and re.fullmatch(r'[0-9a-fA-F]{8}', p[0]) and p[1]:
                    self.cname[int(p[0], 16)] = p[1].split('|')[0]
        self.arity_cache_path = os.path.join(DATADIR, 'arity.json')
        self.arity = {}
        if os.path.exists(self.arity_cache_path):
            self.arity = {int(k, 16): v for k, v in json.load(open(self.arity_cache_path)).items()}
        self._dirty = False

    def name(self, a):
        return self.cname.get(a) or self.names.get(a) or 'FUN_%08x' % a

    def entry(self, a):
        return self.table.get(self.name(a))

    def nargs(self, a):
        e = self.entry(a)
        if e is not None:
            return e['n']
        if a not in self.arity:
            self.arity[a] = callee_arity(self.pe, a, 1500)[0]
            self._dirty = True
        ar = self.arity[a]
        if ar > 16:
            ar = e['n'] if e else 0
        return ar

    def save(self):
        if self._dirty:
            json.dump({'%08x' % k: v for k, v in sorted(self.arity.items())}, open(self.arity_cache_path, 'w'), indent=0)


# ----------------------------------------------------------------------------------------------
# function-level decompiler
# ----------------------------------------------------------------------------------------------
class FuncDecomp:
    MAX_NODES = 6000

    def __init__(self, pe, rt, entry):
        self.pe, self.rt, self.entry = pe, rt, entry
        self.insn = {}
        self.cfg_succ = {}
        self.leaders = set()
        self.warnings = []
        self._build_cfg()
        self._liveness()
        self.nodes = []
        self.memo = {}

    # ---- instruction CFG ----------------------------------------------------------------------
    def dec(self, a):
        i = self.insn.get(a)
        if i is None:
            code = self.pe.read(a, 16)
            try:
                i = next(md().disasm(code, a))
            except StopIteration:
                raise DecompileError('cannot decode at %x' % a)
            self.insn[a] = i
        return i

    def prev_insn(self, a):
        for back in range(1, 16):
            p = self.insn.get(a - back)
            if p is not None and p.address + p.size == a:
                return p
        return None

    def jumptable(self, i):
        """MSVC switch: 'cmp r, N; ja dflt; jmp [r*4+T]' or with a byte map
        'cmp r, N; ja dflt; xor edx,edx; mov dl,[r+B]; jmp [edx*4+T]'.
        Returns (index_reg, [target for r = 0..N])."""
        op = i.operands[0]
        if op.type != x86.X86_OP_MEM or op.mem.scale != 4 or op.mem.base != 0:
            return None
        table = op.mem.disp & 0xffffffff
        idxreg = REG32.get(i.reg_name(op.mem.index))
        bytemap = None
        p = self.prev_insn(i.address)
        if p is not None and p.mnemonic == 'mov' and p.operands[1].type == x86.X86_OP_MEM and p.operands[1].size == 1:
            bytemap = p.operands[1].mem.disp & 0xffffffff
            idxreg = REG32.get(i.reg_name(p.operands[1].mem.base))
        n = None
        a = i.address
        for _ in range(8):
            p = self.prev_insn(a)
            if p is None:
                break
            if p.mnemonic == 'cmp' and p.operands[1].type == x86.X86_OP_IMM and p.operands[0].type == x86.X86_OP_REG                     and REG32.get(p.reg_name(p.operands[0].reg)) == idxreg:
                n = p.operands[1].imm + 1
                break
            a = p.address
        if n is None or n > 1024:
            return None
        if bytemap is not None:
            raw = self.pe.read(bytemap, n)
            return idxreg, [self.pe.u32(table + 4 * b) for b in raw]
        return idxreg, [self.pe.u32(table + 4 * k) for k in range(n)]

    def _build_cfg(self):
        work = [self.entry]
        self.leaders.add(self.entry)
        seen = set()
        while work:
            a = work.pop()
            while a not in seen:
                if not (BLOCK_LO <= a < BLOCK_HI) or len(seen) > 40000:
                    raise DecompileError('code runs outside the mission block at %x' % a)
                seen.add(a)
                i = self.dec(a)
                nxt = a + i.size
                m = i.mnemonic
                if m == 'ret':
                    self.cfg_succ[a] = []
                    break
                if m == 'jmp':
                    if i.operands[0].type == x86.X86_OP_IMM:
                        t = i.operands[0].imm
                        if not (BLOCK_LO <= t < BLOCK_HI):      # tail call
                            self.cfg_succ[a] = []
                            break
                        self.cfg_succ[a] = [t]
                        self.leaders.add(t)
                        work.append(t)
                    else:
                        tab = self.jumptable(i)
                        if tab is None:
                            raise DecompileError('indirect jmp at %x' % a)
                        tab = tab[1]
                        self.cfg_succ[a] = list(dict.fromkeys(tab))
                        for t in tab:
                            self.leaders.add(t)
                            work.append(t)
                    break
                if m.startswith('j') or m.startswith('loop'):
                    t = i.operands[0].imm
                    self.cfg_succ[a] = [nxt, t]
                    self.leaders.update((nxt, t))
                    work.append(t)
                    a = nxt
                    continue
                if m in ('int3', 'hlt', 'ud2'):
                    self.cfg_succ[a] = []
                    break
                self.cfg_succ[a] = [nxt]
                a = nxt
        self.addrs = sorted(seen)
        self.end = max(self.addrs) + self.insn[max(self.addrs)].size

    def _rw(self, i):
        m = i.mnemonic
        try:
            rd, wr = i.regs_access()
        except capstone.CsError:
            rd, wr = [], []
        rd = {REG32.get(i.reg_name(r)) for r in rd} - {None}
        wr = {REG32.get(i.reg_name(r)) for r in wr} - {None}
        if m in ('xor', 'sub', 'sbb') and len(i.operands) == 2 and i.operands[0].type == x86.X86_OP_REG \
                and i.operands[1].type == x86.X86_OP_REG and i.operands[0].reg == i.operands[1].reg:
            r = REG32.get(i.reg_name(i.operands[0].reg))
            if m != 'sbb':
                rd.discard(r)
        if m == 'call':
            rd = set()
            wr = {'eax', 'ecx', 'edx'}
        if m == 'ret':
            rd = {'eax'} if False else set()
        if m.startswith('set'):
            # setcc writes the low byte only; we model it as a full write after 'xor r,r'
            pass
        return rd, wr

    def _liveness(self):
        self.live_in = {a: set() for a in self.addrs}
        rw = {a: self._rw(self.insn[a]) for a in self.addrs}
        changed = True
        order = sorted(self.addrs, reverse=True)
        while changed:
            changed = False
            for a in order:
                out = set()
                for s in self.cfg_succ.get(a, []):
                    out |= self.live_in.get(s, set())
                rd, wr = rw[a]
                li = rd | (out - wr)
                if li != self.live_in[a]:
                    self.live_in[a] = li
                    changed = True

    # ---- symbolic execution -------------------------------------------------------------------
    def operand(self, st, i, op):
        if op.type == x86.X86_OP_IMM:
            return Const(op.imm)
        if op.type == x86.X86_OP_REG:
            r = REG32.get(i.reg_name(op.reg))
            if r is None:
                return Unk(i.reg_name(op.reg))
            return st.regs[r]
        if op.type == x86.X86_OP_MEM:
            return self.memval(st, i, op)
        return Unk('op')

    def memval(self, st, i, op):
        base = i.reg_name(op.mem.base) if op.mem.base else None
        index = i.reg_name(op.mem.index) if op.mem.index else None
        disp = op.mem.disp & 0xffffffff
        if base == 'esp':
            k = (op.mem.disp // 4)
            if k < len(st.stack):
                return st.stack[-1 - k][0]
            return Unk('stk')
        if base is None and index is None:
            if disp == A_TEAM:
                return Sym('team')
            if disp == A_LANG:
                return Sym('lang')
            return Mem(disp)
        if base is None and index is not None:
            return Index(disp, st.regs[REG32[index]], op.mem.scale)
        b = st.regs[REG32[base]]
        if index is not None:
            return Unk('mem2')
        return Field(b, op.mem.disp)

    def write(self, st, i, op, val, node):
        if op.type == x86.X86_OP_REG:
            r = REG32.get(i.reg_name(op.reg))
            if r:
                st.regs[r] = val
            return
        if op.type == x86.X86_OP_MEM:
            base = i.reg_name(op.mem.base) if op.mem.base else None
            disp = op.mem.disp & 0xffffffff
            if base is None and not op.mem.index:
                if disp == A_SCOPE:
                    node.stmts.append(('scope', val))
                    return
                if disp == A_SCOPENAME:
                    node.stmts.append(('scopename', val))
                    return
            if base == 'esp':
                k = op.mem.disp // 4
                if k < len(st.stack):
                    st.stack[-1 - k][0] = val
                return
            self.use(node, val)
            node.stmts.append(('store', self.memval(st, i, op), val))

    def use(self, node, v):
        """v is consumed (argument / condition / store): its calls are sub-expressions, not statements."""
        if not isinstance(v, (V, Cond)):
            return
        ids = {x.id for x in walk_values(v) if isinstance(x, Call)}
        if ids:
            node.stmts[:] = [s for s in node.stmts if not (s[0] == 'call' and s[1].id in ids)]

    def new_node(self, addr):
        n = Node(len(self.nodes), addr)
        self.nodes.append(n)
        if len(self.nodes) > self.MAX_NODES:
            raise DecompileError('node explosion')
        return n

    def state_key(self, a, st):
        live = self.live_in.get(a, set())
        regs = tuple((r, st.regs[r].key()) for r in sorted(live))
        stk = tuple(e[0].key() if not e[1] else '.' for e in st.stack)
        return (a, regs, stk, st.loop)

    def run(self):
        sys.setrecursionlimit(100000)
        self.root = self.explore(self.entry, State())
        # a call whose result is consumed in a later node (pushed before a jmp into shared code, or kept in a
        # register) is an expression there, not a statement here
        used = set()
        for n in self.nodes:
            vals = [a for s in n.stmts if s[0] == 'call' for a in s[1].args]
            vals += [s[2] for s in n.stmts if s[0] == 'store']
            vals += list(walk_term(n.term))
            for v in vals:
                for x in walk_values(v):
                    if isinstance(x, Call):
                        used.add(x.id)
        for n in self.nodes:
            n.stmts = [s for s in n.stmts if not (s[0] == 'call' and s[1].id in used)]
        for n in self.nodes:
            for s in n.succs():
                self.nodes[s].preds.append(n.id)
        return self.root

    def explore(self, a, st):
        key = self.state_key(a, st)
        if key in self.memo:
            return self.memo[key]
        node = self.new_node(a)
        self.memo[key] = node.id
        self.step(node, a, st)
        return node.id

    def flags_cond(self, st, jcc):
        f = st.flags
        if f is None:
            return Cond('?', Unk('flags'))
        kind = f[0]
        cc = jcc[1:]
        if kind == 'test':                 # test a, a  -> compare a with 0
            a = f[1]
            m = {'e': '==', 'z': '==', 'ne': '!=', 'nz': '!=', 's': '<', 'ns': '>=', 'l': '<', 'ge': '>=',
                 'le': '<=', 'g': '>'}
            if cc in m:
                return rel(m[cc], a, Const(0))
            if cc in ('a',):
                return rel('!=', a, Const(0))
            if cc in ('be',):
                return rel('==', a, Const(0))
        elif kind == 'cmp':
            a, b = f[1], f[2]
            m = {'e': '==', 'z': '==', 'ne': '!=', 'nz': '!=', 'l': '<', 'ge': '>=', 'le': '<=', 'g': '>',
                 'b': 'u<', 'ae': 'u>=', 'a': 'u>', 'be': 'u<=', 'nb': 'u>=', 'c': 'u<', 'nc': 'u>='}
            if cc in m:
                return rel(m[cc], a, b)
            if cc == 's':
                return rel('<', mk_bin('-', a, b), Const(0))
            if cc == 'ns':
                return rel('>=', mk_bin('-', a, b), Const(0))
        elif kind == 'res':               # flags of an arithmetic result
            r = f[1]
            m = {'e': '==', 'z': '==', 'ne': '!=', 'nz': '!=', 's': '<', 'ns': '>=', 'l': '<', 'ge': '>=',
                 'le': '<=', 'g': '>'}
            if cc in m:
                return rel(m[cc], r, Const(0))
        return Cond('?', Unk('flags:%s:%s' % (kind, jcc)))

    def do_call(self, node, st, target, at):
        k = self.rt.nargs(target)
        if k > len(st.stack):
            self.warnings.append('call %x at %x wants %d args, stack has %d' % (target, at, k, len(st.stack)))
            k = len(st.stack)
        args = []
        for j in range(k):
            e = st.stack[-1 - j]
            if e[1]:
                self.warnings.append('arg %d of %x at %x consumed twice' % (j, target, at))
            e[1] = True
            args.append(e[0])
            self.use(node, e[0])
        c = Call(target, args, at)
        node.stmts.append(('call', c))
        st.regs['eax'] = c
        st.regs['ecx'] = Unk('clob')
        st.regs['edx'] = Unk('clob')
        return c

    def step(self, node, a, st):
        """execute from a until a terminator; fills node.stmts / node.term."""
        first = True
        while True:
            if not first and a in self.leaders:
                node.term = ('goto', self.explore(a, st))
                return
            first = False
            i = self.insn.get(a) or self.dec(a)
            m = i.mnemonic
            ops = i.operands
            nxt = a + i.size

            if m == 'push':
                st.stack.append([self.operand(st, i, ops[0]), False])
            elif m == 'pop':
                v = st.stack.pop()[0] if st.stack else Unk('pop')
                self.write(st, i, ops[0], v, node)
            elif m == 'mov':
                # message-loop prologue / continue detection: reading [team+0x50]
                if ops[1].type == x86.X86_OP_MEM and ops[1].mem.disp == 0x50 and ops[1].mem.base \
                        and isinstance(st.regs[REG32[i.reg_name(ops[1].mem.base)]], Sym) \
                        and st.regs[REG32[i.reg_name(ops[1].mem.base)]].name == 'team':
                    if st.loop is not None:
                        node.term = ('continue',)
                        return
                    if self.msgloop(node, a, st):
                        return
                v = self.operand(st, i, ops[1])
                self.write(st, i, ops[0], v, node)
            elif m == 'movzx' or m == 'movsx':
                self.write(st, i, ops[0], self.operand(st, i, ops[1]), node)
            elif m == 'lea':
                op = ops[1]
                base = i.reg_name(op.mem.base) if op.mem.base else None
                idx = i.reg_name(op.mem.index) if op.mem.index else None
                v = Const(op.mem.disp)
                if base:
                    v = mk_bin('+', st.regs[REG32[base]], v) if base != 'esp' else Unk('lea_esp')
                if idx:
                    v = mk_bin('+', v, mk_bin('*', st.regs[REG32[idx]], Const(op.mem.scale)))
                if not (base and idx is None and op.mem.disp == 0 and REG32.get(base) == REG32.get(i.reg_name(ops[0].reg))):
                    self.write(st, i, ops[0], v, node)
            elif m in ('add', 'sub', 'imul', 'and', 'or', 'xor', 'shl', 'sar', 'shr', 'adc', 'sbb') and len(ops) == 2:
                if ops[0].type == x86.X86_OP_REG and i.reg_name(ops[0].reg) == 'esp':
                    n = ops[1].imm // 4 if ops[1].type == x86.X86_OP_IMM else 0
                    if m == 'add':
                        for _ in range(n):
                            if st.stack:
                                e = st.stack.pop()
                                if not e[1] and not isinstance(e[0], Unk):
                                    self.warnings.append('unconsumed push %r dropped at %x' % (e[0], a))
                                    node.stmts.append(('lost', e[0]))
                    else:
                        for _ in range(n):
                            st.stack.append([Unk('local'), True])
                else:
                    x = self.operand(st, i, ops[0])
                    y = self.operand(st, i, ops[1])
                    same = ops[1].type == x86.X86_OP_REG and ops[0].type == x86.X86_OP_REG and ops[0].reg == ops[1].reg
                    if m == 'xor' and same:
                        r = Const(0)
                    elif m == 'sub' and same:
                        r = Const(0)
                    elif m == 'sbb' and same:
                        r = Un('-', st.carry) if st.carry is not None else Unk('sbb')
                    elif m == 'or' and isinstance(y, Const) and y.v == 0xffffffff:
                        r = Const(-1)
                    elif m == 'and' and isinstance(y, Const) and y.v == 0:
                        r = Const(0)
                    else:
                        r = mk_bin({'add': '+', 'sub': '-', 'imul': '*', 'and': '&', 'or': '|', 'xor': '^',
                                    'shl': '<<', 'sar': '>>', 'shr': '>>', 'adc': '+', 'sbb': '-'}[m], x, y)
                    self.write(st, i, ops[0], r, node)
                    st.flags = ('cmp', x, y) if m == 'sub' else ('res', r)
            elif m in ('imul', 'mul') and len(ops) == 1:
                x, y = st.regs['eax'], self.operand(st, i, ops[0])
                st.regs['edx'] = Bin('mulhi', y, x) if isinstance(x, Const) else Bin('mulhi', x, y)
                st.regs['eax'] = mk_bin('*', x, y)
            elif m == 'imul' and len(ops) == 3:
                self.write(st, i, ops[0], mk_bin('*', self.operand(st, i, ops[1]), self.operand(st, i, ops[2])), node)
            elif m in ('inc', 'dec', 'neg', 'not'):
                x = self.operand(st, i, ops[0])
                if m == 'inc':
                    r = mk_bin('+', x, Const(1))
                elif m == 'dec':
                    r = mk_bin('-', x, Const(1))
                elif m == 'neg':
                    r = Un('-', x) if not isinstance(x, Un) or x.op != '-' else x.a
                    st.carry = Cond('!=', x, Const(0)) if not isinstance(x, Cond) else x
                    if isinstance(x, Un) and x.op == '-' and isinstance(x.a, Cond):
                        r = x.a     # -(-(cond)) -> cond
                else:
                    r = Un('~', x)
                self.write(st, i, ops[0], r, node)
                st.flags = ('res', r)
            elif m == 'cdq':
                st.regs['edx'] = Un('sgn', st.regs['eax'])
            elif m == 'idiv' or m == 'div':
                d = self.operand(st, i, ops[0])
                st.regs['eax'] = mk_bin('/', st.regs['eax'], d)
                st.regs['edx'] = mk_bin('%', st.regs['eax'], d)
            elif m == 'test':
                x = self.operand(st, i, ops[0])
                y = self.operand(st, i, ops[1])
                if ops[0].type == ops[1].type == x86.X86_OP_REG and ops[0].reg == ops[1].reg:
                    st.flags = ('test', x)
                else:
                    st.flags = ('test', mk_bin('&', x, y))
            elif m == 'cmp':
                st.flags = ('cmp', self.operand(st, i, ops[0]), self.operand(st, i, ops[1]))
            elif m.startswith('set'):
                c = self.flags_cond(st, 'j' + m[3:])
                self.write(st, i, ops[0], c, node)
            elif m == 'call':
                if ops[0].type != x86.X86_OP_IMM:
                    raise DecompileError('indirect call at %x' % a)
                self.do_call(node, st, ops[0].imm, a)
            elif m == 'ret':
                node.term = ('ret',)
                return
            elif m == 'jmp':
                if ops[0].type == x86.X86_OP_IMM:
                    t = ops[0].imm
                    if not (BLOCK_LO <= t < BLOCK_HI):
                        self.do_call(node, st, t, a)
                        node.term = ('ret',)
                        return
                    node.term = ('goto', self.explore(t, st))
                    return
                idxreg, tab = self.jumptable(i)
                idx = st.regs[idxreg]
                off = 0
                while isinstance(idx, Bin) and idx.op in ('+', '-') and isinstance(idx.b, Const):
                    off += -idx.b.s if idx.op == '+' else idx.b.s
                    idx = idx.a
                cases = collections.OrderedDict()
                done = {}
                for k, t in enumerate(tab):
                    if t not in done:
                        done[t] = self.explore(t, st.copy())
                    cases[(k + off) & 0xffffffff] = done[t]
                node.term = ('switch', idx, cases, None)
                return
            elif m.startswith('j'):
                c = self.flags_cond(st, m)
                t = ops[0].imm
                self.use(node, c)
                if st.loop is None and self._is_queue_cond(c):
                    pass
                st_t = st.copy()
                self.branch_facts(st_t, c, True)
                self.branch_facts(st, c, False)
                tn = self.explore(t, st_t)
                fn = self.explore(nxt, st)
                node.term = ('if', c, tn, fn)
                return
            elif m in ('nop',):
                pass
            elif m == 'xchg':
                x, y = self.operand(st, i, ops[0]), self.operand(st, i, ops[1])
                self.write(st, i, ops[0], y, node)
                self.write(st, i, ops[1], x, node)
            else:
                raise DecompileError('unhandled insn %x: %s %s' % (a, m, i.op_str))
            a = nxt

    @staticmethod
    def branch_facts(st, c, taken):
        """on the path where 'x == K' holds, registers holding x are known to be K (MSVC reuses them)."""
        op = c.op if taken else NEG_REL.get(c.op)
        if op != '==' or not isinstance(c.b, Const) or not isinstance(c.a, V):
            return
        k = c.a.key()
        for r in GPRS:
            if st.regs[r].key() == k:
                st.regs[r] = Const(c.b.v)

    def _is_queue_cond(self, c):
        return False

    def msgloop(self, node, a, st):
        """kas2c (Cataclysm) message handler loop:
              q = team->msgQueue; n = q ? q->count : -1;  for (i = n; i >= 0 ...) { msg = aitMsgGet(team, i); switch(msg) ... }
           Recognised at the read of [team+0x50]. Returns True when a loop node was emitted."""
        seq = []
        b = a
        for _ in range(12):
            i = self.dec(b)
            seq.append(i)
            if i.mnemonic in ('jl', 'js'):
                break
            b += i.size
        else:
            return False
        jl = seq[-1]
        # find 'mov esi, eax' (loop register)
        loopreg = None
        for i in seq:
            if i.mnemonic == 'mov' and i.operands[0].type == x86.X86_OP_REG and i.operands[1].type == x86.X86_OP_REG:
                loopreg = REG32[i.reg_name(i.operands[0].reg)]
        if loopreg is None:
            return False
        # emulate pushes in the prologue (callee-saved register spill)
        for i in seq:
            if i.mnemonic == 'push':
                st.stack.append([self.operand(st, i, i.operands[0]), True])
        exit_st = st.copy()
        body_st = st.copy()
        body_st.regs[loopreg] = Sym('msgindex')
        body_st.loop = jl.address + jl.size
        body = self.explore(jl.address + jl.size, body_st)
        ex = self.explore(jl.operands[0].imm, exit_st)
        node.term = ('msgloop', body, ex)
        return True


# ----------------------------------------------------------------------------------------------
# structuring
# ----------------------------------------------------------------------------------------------
class Structurer:
    def __init__(self, fd):
        self.fd = fd
        self.N = fd.nodes
        self.EXIT = -1
        self._collapse()
        self._postdom()
        self.budget = 40 * len(self.N) + 2000

    def _recompute_preds(self):
        live = set()
        st = [self.fd.root]
        while st:
            u = st.pop()
            if u in live:
                continue
            live.add(u)
            st.extend(self.N[u].succs())
        for n in self.N:
            n.preds = []
        for u in sorted(live):
            for s in dict.fromkeys(self.N[u].succs()):
                self.N[s].preds.append(u)
        self.live = live

    def _swkey(self, c):
        v = self._swvar(c)
        return v.key() if v is not None else None

    def _collapse(self):
        """merge short-circuit condition chains: A ? (B ? x : y) : y  ->  (A AND B) ? x : y, etc."""
        self._recompute_preds()
        changed = True
        while changed:
            changed = False
            for A in sorted(self.live):
                n = self.N[A]
                if n.term is None or n.term[0] != 'if':
                    continue
                _, c, t, f = n.term
                for B in (t, f):
                    if B == A or not self.is_pure_cond(B) or self.N[B].preds != [A]:
                        continue
                    _, cb, tb, fb = self.N[B].term
                    k1, k2 = self._swkey(c), self._swkey(cb)
                    if k1 is not None and k1 == k2:
                        continue
                    new = None
                    if B == t:
                        if fb == f:
                            new = ('if', cand(c, cb), tb, f)
                        elif tb == f:
                            new = ('if', cand(c, cnot(cb)), fb, f)
                    else:
                        if tb == t:
                            new = ('if', cor(c, cb), t, fb)
                        elif fb == t:
                            new = ('if', cor(c, cnot(cb)), t, tb)
                    if new is not None:
                        n.term = new
                        self._recompute_preds()
                        changed = True
                        break
                if changed:
                    break

    def _postdom(self):
        # post-dominators on the (acyclic, loop bodies end in 'continue') semantic graph
        N = self.N
        succ = {n.id: [s for s in n.succs()] for n in N}
        # loops: the msgloop body is a separate region whose exits are 'continue' / 'ret'
        order = []
        seen = set()

        def dfs(u):
            stack = [(u, iter(succ[u]))]
            seen.add(u)
            while stack:
                v, it = stack[-1]
                for w in it:
                    if w not in seen:
                        seen.add(w)
                        stack.append((w, iter(succ[w])))
                        break
                else:
                    order.append(v)
                    stack.pop()
        for n in N:
            if n.id not in seen:
                dfs(n.id)
        # order is a post-order; process in post-order so successors come first
        ipdom = {}
        rank = {v: k for k, v in enumerate(order)}

        def rk(x):
            return -1 if x == self.EXIT else rank[x]

        def intersect(a, b):
            while a != b:
                while rk(a) > rk(b):
                    a = ipdom[a]
                while rk(b) > rk(a):
                    b = ipdom[b]
            return a
        for v in order:
            ss = succ[v]
            if not ss:
                ipdom[v] = self.EXIT
                continue
            cur = None
            for s in ss:
                if s not in ipdom:
                    continue
                cur = s if cur is None else intersect(cur, s)
            ipdom[v] = cur if cur is not None else self.EXIT
        self.ipdom = ipdom
        self.succ = succ

    def reaches(self, a, b, limit=None):
        if a == b:
            return True
        seen = {a}
        st = [a]
        while st:
            u = st.pop()
            for w in self.succ.get(u, []):
                if w == b:
                    return True
                if w not in seen and w != limit:
                    seen.add(w)
                    st.append(w)
        return False

    def is_pure_cond(self, nid):
        n = self.N[nid]
        return n.term is not None and n.term[0] == 'if' and not [s for s in n.stmts if s[0] not in ('scope', 'scopename')]

    def cond_region(self, head):
        """short-circuit condition: grow a set of pure-condition nodes whose preds are all in the set."""
        S = [head]
        Sset = {head}
        changed = True
        while changed:
            changed = False
            for u in list(S):
                t = self.N[u].term
                for w in (t[2], t[3]):
                    if w in Sset or not self.is_pure_cond(w):
                        continue
                    if all(p in Sset for p in self.N[w].preds):
                        S.append(w)
                        Sset.add(w)
                        changed = True
        exits = []
        for u in S:
            t = self.N[u].term
            for w in (t[2], t[3]):
                if w not in Sset and w not in exits:
                    exits.append(w)
        if len(exits) != 2:
            return None
        return S, Sset, exits

    def formula(self, u, Sset, X, Y, memo):
        if u == X:
            return Cond('true', None)
        if u == Y:
            return Cond('false', None)
        if u in memo:
            return memo[u]
        t = self.N[u].term
        c = t[1]
        ft = self.formula(t[2], Sset, X, Y, memo)
        ff = self.formula(t[3], Sset, X, Y, memo)
        if ft.op == 'true' and ff.op == 'false':
            r = c
        elif ft.op == 'false' and ff.op == 'true':
            r = cnot(c)
        elif ff.op == 'false':
            r = cand(c, ft)
        elif ft.op == 'true':
            r = cor(c, ff)
        elif ft.op == 'false':
            r = cand(cnot(c), ff)
        elif ff.op == 'true':
            r = cor(cnot(c), ft)
        else:
            r = cor(cand(c, ft), cand(cnot(c), ff))
        memo[u] = r
        return r

    def emit(self, nid, stop, depth=0):
        out = []
        guard = 0
        while nid is not None and nid != stop and nid != self.EXIT:
            self.budget -= 1
            if self.budget < 0:
                raise DecompileError('structuring blow-up (code duplication)')
            guard += 1
            if guard > 100000:
                raise DecompileError('structuring loop')
            n = self.N[nid]
            out.extend(n.stmts)
            t = n.term
            k = t[0]
            if k == 'goto':
                nid = t[1]
                continue
            if k == 'ret':
                out.append(('return',))
                break
            if k == 'continue':
                break
            if k == 'msgloop':
                body = self.switchify_loop(t[1])
                out.append(('msgloop', body))
                nid = t[2]
                continue
            if k == 'switch':
                join = self.ipdom[nid]
                cases = collections.OrderedDict()
                for v, tgt in t[2].items():
                    cases.setdefault(tgt, []).append(v)
                arms = [(vals, self.emit(tgt, join, depth + 1)) for tgt, vals in cases.items()]
                out.append(('switch', t[1], arms, None))
                nid = join
                continue
            if k == 'if':
                sw = self.try_switch(nid)
                if sw is not None:
                    var, arms, default, join = sw
                    out.append(('switch', var, [(vals, self.emit(tgt, join, depth + 1)) for tgt, vals in arms],
                                self.emit(default, join, depth + 1) if default is not None and default != join else None))
                    nid = join
                    continue
                join = self.ipdom[nid]
                cr = None
                if cr is None:
                    S, Sset, exits = [nid], {nid}, [t[2], t[3]]
                else:
                    S, Sset, exits = cr
                X, Y = exits
                # choose THEN / ELSE
                if Y == join:
                    pass
                elif X == join:
                    X, Y = Y, X
                else:
                    if self.N[X].addr > self.N[Y].addr:
                        X, Y = Y, X
                c = self.formula(nid, Sset, X, Y, {})
                then = self.emit(X, join, depth + 1)
                els = self.emit(Y, join, depth + 1) if Y != join else []
                if not then and els:
                    c, then, els = cnot(c), els, []
                out.append(('if', c, then, els))
                nid = join
                continue
            raise DecompileError('bad terminator %r' % (t,))
        return out

    # ---- switch recovery over compare trees -----------------------------------------------------
    def try_switch(self, head):
        t = self.N[head].term
        var = self._swvar(t[1])
        if var is None:
            return None
        leaves = []          # (target, intervals)
        self._swwalk(head, var.key(), [(0, 0xffffffff)], leaves, set(), head)
        if leaves is None or len(leaves) < (2 if var.addr == A_MSGGET else 3):
            return None
        per = collections.OrderedDict()
        for tgt, iv in leaves:
            if not iv:
                continue
            tgt = self.skip_trivial(tgt)
            vals = self._iv_values(iv)
            per.setdefault(tgt, []).append(vals)
        arms = collections.OrderedDict()
        default = None
        for tgt, lst in per.items():
            if all(v is not None for v in lst) and sum(len(v) for v in lst) <= 8:
                arms[tgt] = [x for v in lst for x in v]
            else:
                if default is not None and default != tgt:
                    return None
                default = tgt
        ismsg = isinstance(var, Call) and var.addr == A_MSGGET
        if ismsg:
            # message 0 = empty queue slot: kas2c just continues the loop
            arms = collections.OrderedDict((t, [v for v in vs if v != 0]) for t, vs in arms.items())
            arms = collections.OrderedDict((t, vs) for t, vs in arms.items() if vs)
        if len(arms) < (1 if ismsg else 2):
            return None
        join = self.ipdom[head]
        return var, list(arms.items()), default, join

    def skip_trivial(self, nid):
        seen = set()
        while nid not in seen:
            seen.add(nid)
            n = self.N[nid]
            if n.term is not None and n.term[0] == 'goto' and not [x for x in n.stmts if x[0] not in ('scope', 'scopename')]:
                nid = n.term[1]
            else:
                break
        return nid

    def _swvar(self, c):
        if isinstance(c, Cond) and c.op in NEG_REL and isinstance(c.b, Const) and isinstance(c.a, Call):
            return c.a
        return None

    def _swwalk(self, nid, vkey, iv, leaves, seen, head):
        n = self.N[nid]
        pure = self.is_pure_cond(nid) or (n.term is not None and n.term[0] == 'switch' and not [
            x for x in n.stmts if x[0] not in ('scope', 'scopename')])
        if nid != head and (not pure or len(n.preds) != 1):
            leaves.append((nid, iv))
            return
        c = n.term[1]
        v = self._swvar(c) if n.term[0] == 'if' else (n.term[1] if isinstance(n.term[1], Call) else None)
        if v is None or v.key() != vkey:
            leaves.append((nid, iv))
            return
        if n.term[0] == 'switch':
            for val, tgt in n.term[2].items():
                if any(lo <= val <= hi for lo, hi in iv):
                    leaves.append((tgt, [(val, val)]))
            return
        k = c.b.v
        tr, fl = self._split(iv, c.op, k)
        self._swwalk(n.term[2], vkey, tr, leaves, seen, head)
        self._swwalk(n.term[3], vkey, fl, leaves, seen, head)

    @staticmethod
    def _split(iv, op, k):
        def inter(iv, lo, hi):
            r = []
            for a, b in iv:
                a2, b2 = max(a, lo), min(b, hi)
                if a2 <= b2:
                    r.append((a2, b2))
            return r
        M = 0xffffffff
        sk = k if k < 0x80000000 else k - (1 << 32)
        ranges = {
            '==': [(k, k)], '!=': [(0, k - 1), (k + 1, M)],
            'u<': [(0, k - 1)], 'u<=': [(0, k)], 'u>': [(k + 1, M)], 'u>=': [(k, M)],
        }
        if op in ('<', '<=', '>', '>='):
            # signed: map onto unsigned ranges
            if op == '<':
                lo, hi = -(1 << 31), sk - 1
            elif op == '<=':
                lo, hi = -(1 << 31), sk
            elif op == '>':
                lo, hi = sk + 1, (1 << 31) - 1
            else:
                lo, hi = sk, (1 << 31) - 1
            rr = []
            if lo < 0:
                rr.append(((lo & M), min(M, (min(hi, -1)) & M)))
            if hi >= 0:
                rr.append((max(lo, 0), hi))
            ranges[op] = rr
        yes = sorted(ranges[op])
        no = []
        cur = 0
        for lo, hi in yes:
            if lo > cur:
                no.append((cur, lo - 1))
            cur = max(cur, hi + 1)
        if cur <= M:
            no.append((cur, M))
        t = []
        for lo, hi in yes:
            t += inter(iv, lo, hi)
        f = []
        for lo, hi in no:
            f += inter(iv, lo, hi)
        return t, f

    @staticmethod
    def _iv_values(iv):
        tot = sum(b - a + 1 for a, b in iv)
        if tot > 4:
            return None
        vals = []
        for a, b in iv:
            vals.extend(range(a, b + 1))
        return vals

    def switchify_loop(self, body):
        return self.emit(body, None)


# ----------------------------------------------------------------------------------------------
# rendering
# ----------------------------------------------------------------------------------------------
class Namer:
    """hash -> name resolution."""

    def __init__(self):
        self.names = {}
        self.guess = set()
        self.used_guess = set()
        self.used = collections.Counter()
        self.unresolved = collections.Counter()

    def add(self, name, source=''):
        h = kas_hash(name)
        if h not in self.names:
            self.names[h] = name

    def get(self, h):
        g = ''
        if h & 0x80000000:
            h &= 0x7fffffff
            g = ''
        n = self.names.get(h)
        if n:
            self.used[h] += 1
            if h in self.guess:
                self.used_guess.add(h)
            return g + n
        self.unresolved[h] += 1
        return None


class Renderer:
    def __init__(self, pe, rt, namer, mission):
        self.pe, self.rt, self.namer, self.mission = pe, rt, namer, mission
        self.lstrings = collections.OrderedDict()      # table address -> name
        self.lstring_text = {}
        self.unknown_funcs = collections.Counter()

    def hname(self, h, quote=True):
        n = self.namer.get(h)
        if n is None:
            s = '#%08x' % h
            return s
        return '"%s"' % n if quote else n

    def label(self, h):
        if h & 0xF0000000 == 0xF0000000:
            return 'UnreferencedState_%07x' % (h & 0x0fffffff)
        n = self.namer.get(h & 0x7fffffff)
        return n if n else 'H%08x' % h

    def cstr(self, a):
        s = self.pe.cstr(a, 2048)
        if s is None:
            return None
        return s

    def lstring(self, table):
        if table not in self.lstrings:
            texts = []
            for k in range(N_LANG):
                p = self.pe.u32(table + 4 * k)
                texts.append(self.cstr(p) or '')
            eng = texts[0]
            base = re.sub(r'[^A-Za-z0-9]+', '', eng.title())[:24] or 'Str'
            name = base
            n = 2
            while name in self.lstrings.values():
                name = '%s%d' % (base, n)
                n += 1
            self.lstrings[table] = name
            self.lstring_text[table] = texts
        return 'LSTRING_' + self.lstrings[table]

    def val(self, v, ptype=None, prec=0):
        if isinstance(v, Const):
            if ptype == 'UDWORD':
                return self.hname(v.v)
            if ptype == 'CHARPTR':
                s = self.cstr(v.v) if v.v else None
                if s is not None and v.v > 0x400000:
                    return '"%s"' % s.replace('"', '\\"')
                return '%d' % v.s
            if ptype in ('GROWSELECTIONPTR', 'AITEAMPTR', 'VECTORPTR', 'VOLUMEPTR', 'PATHPTR') and v.v == 0:
                return 'NULL'
            if v.v >= 0x10000000 and v.v < 0x80000000 or (ptype == 'SDWORD' and v.v > 0x100000 and v.v < 0x80000000):
                return '0x%x' % v.v
            return '%d' % v.s
        if isinstance(v, Sym):
            if v.name == 'team':
                return 'THISTEAM'
            if v.name == 'msgindex':
                return 'MSGINDEX'
            return v.name.upper()
        if isinstance(v, Call):
            return self.call(v)
        if isinstance(v, Index):
            if isinstance(v.idx, Sym) and v.idx.name == 'lang' and v.scale == 4:
                return self.lstring(v.table)
            return '%s[%s]' % (self.mem(v.table), self.val(v.idx))
        if isinstance(v, Field):
            if isinstance(v.base, Sym) and v.base.name == 'team' and v.off == 0x58:
                return 'THISTEAM_LABEL'
            return '[%s+0x%x]' % (self.val(v.base), v.off)
        if isinstance(v, Bin):
            if isinstance(v.base if False else v.a, Sym) and v.a.name == 'team' and v.op == '+' and isinstance(v.b, Const) and v.b.v == 0x1c:
                return 'THISTEAMSHIPS'
            p = {'*': 5, '/': 5, '%': 5, '+': 4, '-': 4, '<<': 3, '>>': 3, '&': 2, '^': 2, '|': 2}.get(v.op, 1)
            s = '%s %s %s' % (self.val(v.a, None, p), v.op, self.val(v.b, None, p + 1))
            return '(%s)' % s if p < prec else s
        if isinstance(v, Un):
            if v.op == '-' and isinstance(v.a, Cond):
                return self.cond(v.a)
            return '%s%s' % (v.op, self.val(v.a, None, 9))
        if isinstance(v, Mem):
            return self.mem(v.addr)
        if isinstance(v, Cond):
            return '(%s)' % self.cond(v)
        if isinstance(v, Unk):
            return '/*?%s*/' % v.d
        return '/*%r*/' % (v,)

    def mem(self, a):
        return 'MEM_%08x' % a

    def call(self, c):
        a = c.addr
        if a in LABEL_FUNCS and len(c.args) == 1 and isinstance(c.args[0], Const):
            return '%s_%s' % (LABEL_FUNCS[a], self.label(c.args[0].v))
        if a == A_THISTEAMSPOINT:
            return 'THISTEAMSPOINT'
        if a == A_MSGGET:
            return 'MsgGet(%s)' % ', '.join(self.val(x) for x in c.args)
        cname = self.rt.name(a)
        e = self.rt.entry(a)
        if e is None:
            self.unknown_funcs[cname] += 1
            kn = cname[4:] if cname.startswith('kasf') else cname
            return '%s(%s)' % (kn, ', '.join(self.val(x) for x in c.args))
        types = [p[1] for p in e['params']]
        args = []
        for k, x in enumerate(c.args):
            args.append(self.val(x, types[k] if k < len(types) else None))
        return '%s(%s)' % (e['kas'], ', '.join(args))

    def cond(self, c, prec=0):
        if not isinstance(c, Cond):
            return self.val(c)
        op = c.op
        if op == 'and' or op == 'or':
            p = 1 if op == 'or' else 2
            s = '%s %s %s' % (self.cond(c.a, p), op.upper(), self.cond(c.b, p))
            return '(%s)' % s if p < prec else s
        if op == 'not':
            return 'NOT %s' % self.cond(c.a, 3)
        if op == 'true':
            return 'TRUE'
        if op == 'false':
            return 'FALSE'
        if op == '?':
            return '/*?%r*/' % (c.a,)
        a, b = c.a, c.b
        # boolean-valued call compared with 0
        if isinstance(b, Const) and b.v == 0 and op in ('!=', '=='):
            sa = self.val(a, None, 9)
            if op == '!=':
                return sa
            if isinstance(a, Call) and re.search(r'Count|Get|Health|Fuel|RUs|Random|Distance|Remaining|Num',
                                                 self.call(a).split('(')[0]) and not re.match(
                                                     r'(MsgReceived|ObjectiveGet)', self.call(a)):
                return '%s = 0' % sa
            return 'NOT %s' % sa
        o = {'==': '=', '!=': '!=', '<': '<', '<=': '<=', '>': '>', '>=': '>=', 'u<': '<', 'u<=': '<=',
             'u>': '>', 'u>=': '>='}[op]
        return '%s %s %s' % (self.val(a, None, 6), o, self.val(b, None, 6))


# ----------------------------------------------------------------------------------------------
# mission level
# ----------------------------------------------------------------------------------------------
def find_missions(pe):
    """decode kasMissionStart(name, Init, Watch) call sites in the mission selector functions."""
    res = []
    for f in SELECTOR_FUNCS:
        code = pe.read(f, 0x1400)
        pushes = []
        for i in md().disasm(code, f):
            if i.mnemonic == 'push':
                pushes.append(i.operands[0].imm if i.operands[0].type == x86.X86_OP_IMM else None)
            elif i.mnemonic == 'call':
                if i.operands[0].type == x86.X86_OP_IMM and i.operands[0].imm == A_MISSIONSTART and len(pushes) >= 3:
                    w, ini, nm = pushes[-3], pushes[-2], pushes[-1]
                    name = pe.cstr(nm)
                    res.append((name, ini, w))
                pushes = []
            elif i.mnemonic == 'ret' and f == 0x0052a740:
                break
    seen = set()
    out = []
    for name, ini, w in res:
        if name in seen:
            continue
        seen.add(name)
        out.append((name, ini, w))
    return out


def canonical_mission_name(n):
    m = re.match(r'(mission|tutorial)(\d+)$', n)
    if m:
        return ('Mission%s' if m.group(1) == 'mission' else 'Tutorial%s') % m.group(2)
    return n


class MissionDecomp:
    def __init__(self, pe, rt, namer, name, init, watch):
        self.pe, self.rt, self.namer = pe, rt, namer
        self.name, self.init, self.watch = name, init, watch
        self.funcs = {}          # addr -> FuncDecomp
        self.role = {}           # addr -> list of (kind, fsm, state)  kind in init/watch
        self.fsms = collections.OrderedDict()     # fsm hash -> dict(init, watch, team, states: OrderedDict(hash -> (init, watch)))
        self.errors = {}
        namer.used_guess = set()
        self.render = Renderer(pe, rt, namer, name)

    def decompile_func(self, a):
        if a in self.funcs:
            return self.funcs[a]
        try:
            fd = FuncDecomp(self.pe, self.rt, a)
        except (DecompileError, RecursionError) as e:
            fd = FuncDecomp.__new__(FuncDecomp)
            fd.nodes, fd.warnings, fd.ast = [], [], None
            self.errors[a] = str(e)
            self.funcs[a] = fd
            return fd
        try:
            fd.run()
            fd.ast = Structurer(fd).emit(fd.root, None)
        except (DecompileError, RecursionError) as e:
            fd.ast = None
            self.errors[a] = str(e)
        self.funcs[a] = fd
        return fd

    @staticmethod
    def iter_calls(fd):
        for n in fd.nodes:
            for s in n.stmts:
                if s[0] == 'call':
                    yield s[1]
            for v in walk_term(n.term):
                for x in walk_values(v):
                    if isinstance(x, Call):
                        yield x

    def discover(self):
        self.decompile_func(self.init)
        self.decompile_func(self.watch)
        self.owner = {}
        self.seen = set()
        work = []
        for f in (self.init, self.watch):
            for c in self.calls_of(f):
                if c.addr == A_FSMCREATE and len(c.args) == 4:
                    h, i, w = (x.v if isinstance(x, Const) else None for x in c.args[:3])
                    if h not in self.fsms:
                        self.fsms[h] = {'init': i, 'watch': w, 'team': c.args[3], 'states': collections.OrderedDict()}
                        work += [(i, h), (w, h)]
        self._run(work)

    def _run(self, work):
        while work:
            f, fsm = work.pop(0)
            if f is None or not (BLOCK_LO <= f < BLOCK_HI) or (f, fsm) in self.seen:
                continue
            self.seen.add((f, fsm))
            self.owner.setdefault(f, fsm)
            self.decompile_func(f)
            for c in self.calls_of(f):
                if c.addr == A_JUMP and len(c.args) == 3:
                    h, i, w = (x.v if isinstance(x, Const) else None for x in c.args)
                    st = self.fsms[fsm]['states']
                    if h not in st:
                        st[h] = (i, w)
                        work += [(i, fsm), (w, fsm)]
                elif c.addr == A_FSMCREATE and len(c.args) == 4:
                    h, i, w = (x.v if isinstance(x, Const) else None for x in c.args[:3])
                    if h not in self.fsms:
                        self.fsms[h] = {'init': i, 'watch': w, 'team': c.args[3], 'states': collections.OrderedDict()}
                        work += [(i, h), (w, h)]

    def orphans(self, lo, hi, starts):
        """kas2c emits Init/Watch functions for every declared state, even ones no JUMP reaches; the linker
        keeps them.  Find such unreferenced functions in [lo, hi) and attach them, in (Init, Watch) pairs, to
        the FSM whose code precedes them."""
        self.lo, self.hi = lo, hi
        covered = set()
        for fd in self.funcs.values():
            covered.update(getattr(fd, 'addrs', []))
        cands = sorted(a for a in starts if lo <= a < hi and a % 16 == 0 and a not in covered and a not in self.funcs)
        cands = [a for a in cands if self.pe.read(a, 5)[0] in (0xa1, 0x8b, 0x68, 0xc7, 0x6a, 0x56, 0x53, 0x55, 0x57, 0x83, 0xc3)]
        known = sorted((a, self.owner[a]) for a in self.owner if lo <= a < hi)
        k = 0
        added = 0
        while k < len(cands):
            a = cands[k]
            below = [o for x, o in known if x < a]
            if not below:
                k += 1
                continue
            fsm = below[-1]
            if fsm is None or fsm not in self.fsms:
                k += 1
                continue
            i = a
            w = cands[k + 1] if k + 1 < len(cands) else None
            k += 2
            key = 0xF0000000 | (i & 0x0fffffff)    # synthetic state "hash"
            self.fsms[fsm]['states'][key] = (i, w)
            self.orphan_states = getattr(self, 'orphan_states', set()) | {key}
            self._run([(i, fsm), (w, fsm)])
            added += 1
        return added

    def calls_of(self, a):
        fd = self.funcs.get(a)
        if fd is None:
            return []
        return list(self.iter_calls(fd))

    # ---- output -----------------------------------------------------------------------------
    def body(self, a, indent, ctx):
        fd = self.funcs.get(a)
        pad = '    ' * indent
        if fd is None:
            return [pad + '// (not decompiled)']
        if fd.ast is None:
            return [pad + '// DECOMPILATION FAILED at %08x: %s' % (a, self.errors.get(a))]
        lines = []
        self.stmts(fd.ast, indent, lines, ctx, top=True)
        for w in fd.warnings[:5]:
            lines.append(pad + '// warning: ' + w)
        return lines

    def stmts(self, ast, indent, lines, ctx, top=False):
        pad = '    ' * indent
        R = self.render
        k = 0
        while k < len(ast):
            s = ast[k]
            kind = s[0]
            if kind in ('scope', 'scopename'):
                pass
            elif kind == 'call':
                c = s[1]
                if c.addr == A_JUMP and len(c.args) == 3 and isinstance(c.args[0], Const):
                    lines.append(pad + 'JUMP %s;' % R.label(c.args[0].v))
                    # the 'return' after a jump is implicit in KAS
                    if k + 1 < len(ast) and ast[k + 1][0] == 'return':
                        k += 1
                elif c.addr == A_FSMCREATE and len(c.args) == 4 and isinstance(c.args[0], Const):
                    lines.append(pad + 'FSMCreate(%s, %s);' % (R.label(c.args[0].v), R.val(c.args[3])))
                elif c.addr in LABEL_FUNCS or c.addr == A_THISTEAMSPOINT:
                    pass     # label lookup whose result MSVC kept in a register (CSE): not a statement
                elif c.addr == A_MSGSEND_RAW and ctx.get('inloop'):
                    lines.append(pad + '// (unhandled message is re-queued)')
                else:
                    lines.append(pad + R.val(c) + ';')
            elif kind == 'return':
                if not (top and k == len(ast) - 1):
                    lines.append(pad + 'RETURN;')
            elif kind == 'continue':
                pass
            elif kind == 'store':
                lines.append(pad + '// store %s = %s' % (R.val(s[1]), R.val(s[2])))
            elif kind == 'lost':
                lines.append(pad + '// (discarded value %s)' % R.val(s[1]))
            elif kind == 'if':
                self.emit_if(s, indent, lines, ctx, tail=top and k == len(ast) - 1)
            elif kind == 'switch':
                var = s[1]
                is_msg = isinstance(var, Call) and var.addr == A_MSGGET
                if is_msg:
                    for vals, arm in s[2]:
                        lines.append(pad + 'ONMSG (%s)' % ', '.join(R.hname(v) for v in vals))
                        self.stmts(arm, indent + 1, lines, ctx)
                        lines.append(pad + 'ENDONMSG')
                    if s[3]:
                        sub = []
                        self.stmts(s[3], indent + 1, sub, ctx)
                        sub = [x for x in sub if 're-queued' not in x]
                        if sub:
                            lines.append(pad + 'ONMSG DEFAULT')
                            lines.extend(sub)
                            lines.append(pad + 'ENDONMSG')
                else:
                    first = True
                    for vals, arm in s[2]:
                        c = ' OR '.join('%s = %d' % (R.val(var), v if v < 0x80000000 else v - (1 << 32)) for v in vals)
                        lines.append(pad + ('IF (%s)' if first else 'ELSEIF (%s)') % c)
                        first = False
                        self.stmts(arm, indent + 1, lines, ctx)
                    if s[3]:
                        lines.append(pad + 'ELSE')
                        self.stmts(s[3], indent + 1, lines, ctx)
                    lines.append(pad + 'ENDIF')
            elif kind == 'msgloop':
                lines.append(pad + '// message handlers (kas2c: loop over this team\'s message queue)')
                sub = dict(ctx)
                sub['inloop'] = True
                body = s[1]
                # the loop body is: msg = MsgGet(THISTEAM, MSGINDEX); switch(msg) {...}
                self.stmts(body, indent, lines, sub)
            else:
                lines.append(pad + '// ?? %r' % (s,))
            k += 1

    def ends(self, blk):
        """block always leaves the function (JUMP / RETURN)"""
        if not blk:
            return False
        last = blk[-1]
        if last[0] == 'return':
            return True
        if last[0] == 'call' and last[1].addr == A_JUMP:
            return True
        if last[0] == 'if':
            return self.ends(last[2]) and self.ends(last[3])
        return False

    def ends_jump(self, blk):
        """block always ends in a JUMP (ignoring a trailing return)"""
        b = [x for x in blk if x[0] != 'return']
        if not b:
            return False
        last = b[-1]
        if last[0] == 'call' and last[1].addr == A_JUMP:
            return True
        if last[0] == 'if':
            return self.ends_jump(last[2]) and self.ends_jump(last[3])
        return False

    def emit_if(self, s, indent, lines, ctx, tail=False):
        pad = '    ' * indent
        R = self.render
        c, then, els = s[1], s[2], s[3]
        if els == [('return',)] and (tail or self.ends(then)):
            els = []
        if then == [('return',)] and els and tail:
            # IF (c) RETURN ELSE body  ->  IF (NOT c) body
            c, then, els = cnot(c), els, []
        # IFONCE: NOT VarGet(H) AND cond  { VarCreateSet(H, 1); ... }
        once = self.match_ifonce(c, then)
        if once is not None:
            cond2, body = once
            lines.append(pad + 'IFONCE (%s)' % (R.cond(cond2) if cond2 is not None else 'TRUE'))
            self.stmts(body, indent + 1, lines, ctx, top=tail and not els)
            lines.append(pad + 'ENDIFONCE')
            if els:
                self.stmts(els, indent, lines, ctx, top=tail)
            return
        lines.append(pad + 'IF (%s)' % R.cond(c))
        flat = bool(els) and (self.ends_jump(then) or (self.ends(then) and not tail))             and not (len(els) == 1 and els[0][0] == 'if')
        # when the ELSE part is flattened after ENDIF, the THEN part must keep its RETURN
        self.stmts(then, indent + 1, lines, ctx, top=tail and not flat)
        while els:
            if flat:
                # the THEN part JUMPs/RETURNs: the rest is simply the code after ENDIF
                lines.append(pad + 'ENDIF')
                self.stmts(els, indent, lines, ctx, top=tail)
                return
            if len(els) == 1 and els[0][0] == 'if' and self.match_ifonce(els[0][1], els[0][2]) is None:
                e = els[0]
                lines.append(pad + 'ELSEIF (%s)' % R.cond(e[1]))
                then = e[2]
                els = e[3]
                flat = bool(els) and (self.ends_jump(then) or (self.ends(then) and not tail))                     and not (len(els) == 1 and els[0][0] == 'if')
                self.stmts(then, indent + 1, lines, ctx, top=tail and not flat)
                if els == [('return',)] and (tail or self.ends(then)):
                    els = []
                continue
            lines.append(pad + 'ELSE')
            self.stmts(els, indent + 1, lines, ctx, top=tail)
            break
        lines.append(pad + 'ENDIF')

    def match_ifonce(self, c, then):
        if not then or then[0][0] != 'call':
            return None
        first = then[0][1]
        if self.rt.name(first.addr) != 'kasfVarCreateSet' or len(first.args) != 2:
            return None
        h = first.args[0]
        if not (isinstance(h, Const) and isinstance(first.args[1], Const) and first.args[1].v == 1):
            return None

        def is_guard(x):
            return (isinstance(x, Cond) and x.op == '==' and isinstance(x.b, Const) and x.b.v == 0
                    and isinstance(x.a, Call) and self.rt.name(x.a.addr) == 'kasfVarValueGet'
                    and isinstance(x.a.args[0], Const) and x.a.args[0].v == h.v)
        if is_guard(c):
            return None, then[1:]
        conj = []

        def flat(x):
            if isinstance(x, Cond) and x.op == 'and':
                flat(x.a)
                flat(x.b)
            else:
                conj.append(x)
        flat(c)
        if len(conj) >= 2 and is_guard(conj[0]):
            rest = conj[1]
            for x in conj[2:]:
                rest = Cond('and', rest, x)
            return rest, then[1:]
        return None

    def fsm_name(self, h):
        return self.render.label(h)

    def output(self):
        R = self.render
        L = []
        hdr = []
        hdr.append('//')
        hdr.append('//  %s  --  recovered from cataclysm.exe by tools/scripts/kas_decompile.py' % self.name)
        hdr.append('//  Init_%s @ %08x, Watch_%s @ %08x' % (self.name, self.init, self.name, self.watch))
        hdr.append('//  Names in quotes / identifiers were recovered from their KAS hash; #xxxxxxxx and H_xxxxxxxx')
        hdr.append('//  are hashes with no known name.  ONMSG blocks are Cataclysm message handlers (syntax inferred).')
        hdr.append('//')
        body = []
        for fh, f in self.fsms.items():
            fname = self.fsm_name(fh)
            body.append('')
            body.append('//' + '-' * 70)
            body.append('FSM %s' % fname)
            if f['states']:
                body.append('    States %s;' % ', '.join(self.render.label(s) for s in f['states']))
            body.append('    INIT    // %08x' % (f['init'] or 0))
            body.extend(self.body(f['init'], 2, {}))
            body.append('    ENDINIT')
            body.append('    WATCH   // %08x' % (f['watch'] or 0))
            body.extend(self.body(f['watch'], 2, {}))
            body.append('    ENDWATCH')
            for sh, (i, w) in f['states'].items():
                body.append('')
                body.append('    STATE %s' % self.render.label(sh))
                body.append('        INIT    // %08x' % (i or 0))
                body.extend(self.body(i, 3, {}))
                body.append('        ENDINIT')
                body.append('        WATCH   // %08x' % (w or 0))
                body.extend(self.body(w, 3, {}))
                body.append('        ENDWATCH')
                body.append('    ENDSTATE %s' % self.render.label(sh))
            body.append('ENDFSM %s' % fname)
        body.append('')
        body.append('//' + '-' * 70)
        body.append('// mission-level code')
        body.append('INIT    // %08x' % self.init)
        body.extend(self.body(self.init, 1, {}))
        body.append('ENDINIT')
        body.append('WATCH   // %08x' % self.watch)
        body.extend(self.body(self.watch, 1, {}))
        body.append('ENDWATCH')
        loc = []
        if R.lstrings:
            loc.append('')
            loc.append('LOCALIZATION')
            for t, n in R.lstrings.items():
                loc.append('    LSTRING_%s        // %08x' % (n, t))
                for s in R.lstring_text[t]:
                    loc.append('        "%s"' % s.replace('\n', '\\n'))
            loc.append('ENDL')
        tail = []
        g = sorted(self.namer.used_guess)
        if g:
            tail.append('')
            tail.append('// These names were recovered by combining two vocabulary words (hash match, not certain):')
            for k in range(0, len(g), 4):
                tail.append('//   ' + '  '.join('%08x=%s' % (h, self.namer.names[h]) for h in g[k:k + 4]))
        return '\n'.join(hdr + loc + body + tail) + '\n'

    def collect_hashes(self):
        """(hash, kind) for every hashed label used by the mission."""
        out = collections.Counter()
        for fh, f in self.fsms.items():
            out[(fh, 'FSM')] += 1
            for sh in f['states']:
                out[(sh, 'STATE')] += 1
        for fd in self.funcs.values():
            for n in fd.nodes:
                vals = [s[1] for s in n.stmts if s[0] == 'call'] + list(walk_term(n.term))
                if n.term and n.term[0] == 'switch' and isinstance(n.term[1], Call) and n.term[1].addr == A_MSGGET:
                    for v in n.term[2]:
                        out[(v, 'MSG')] += 1
                for v in vals:
                    for c in walk_values(v):
                        if isinstance(c, Cond) and isinstance(c.a, Call) and c.a.addr == A_MSGGET and isinstance(c.b, Const):
                            out[(c.b.v, 'MSG')] += 1
                        if not isinstance(c, Call):
                            continue
                        if c.addr in LABEL_FUNCS and c.args and isinstance(c.args[0], Const):
                            out[(c.args[0].v, LABEL_FUNCS[c.addr])] += 1
                            continue
                        if c.addr in (A_JUMP, A_FSMCREATE):
                            continue
                        e = self.rt.entry(c.addr)
                        if e is None:
                            continue
                        for k, x in enumerate(c.args):
                            if k < len(e['params']) and e['params'][k][1] == 'UDWORD' and isinstance(x, Const):
                                out[(x.v, '%s.%s' % (e['kas'], e['params'][k][0]))] += 1
        return out

    def kas2c_names(self):
        """kas2c-style C function names for every function of this mission."""
        res = collections.OrderedDict()
        res[self.init] = 'Init_%s' % self.name
        res[self.watch] = 'Watch_%s' % self.name
        for fh, f in self.fsms.items():
            fn = re.sub(r'\W', '_', self.fsm_name(fh))
            for kind in ('init', 'watch'):
                if f[kind]:
                    res.setdefault(f[kind], '%s_%s_%s' % ('Init' if kind == 'init' else 'Watch', self.name, fn))
            for sh, (i, w) in f['states'].items():
                sn = re.sub(r'\W', '_', self.render.label(sh))
                if i:
                    res.setdefault(i, 'Init_%s_%s_%s' % (self.name, fn, sn))
                if w:
                    res.setdefault(w, 'Watch_%s_%s_%s' % (self.name, fn, sn))
        return res


def walk_term(t):
    if t is None:
        return
    if t[0] == 'if':
        yield t[1]
    elif t[0] == 'switch':
        yield t[1]


def load_namer(extra=None):
    nm = Namer()
    path = os.path.join(DATADIR, 'kas_names.tsv')
    if os.path.exists(path):
        for line in open(path, encoding='utf-8'):
            p = line.rstrip('\n').split('\t')
            if len(p) >= 2 and re.fullmatch(r'[0-9a-f]{8}', p[0]):
                nm.names.setdefault(int(p[0], 16), p[1])
                if len(p) >= 3 and p[2] == 'guess':
                    nm.guess.add(int(p[0], 16))
    if extra:
        for line in open(extra, encoding='utf-8'):
            w = line.strip()
            if w:
                nm.add(w)
    return nm


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('mission', nargs='?')
    ap.add_argument('--list', action='store_true')
    ap.add_argument('--all', action='store_true')
    ap.add_argument('-o', '--out')
    ap.add_argument('--rename', action='store_true')
    ap.add_argument('--names')
    ap.add_argument('--json', help='write function table json')
    ap.add_argument('--hashes', help='write all hashes used (tsv)')
    a = ap.parse_args()
    pe = PE()
    names = load_function_names(os.path.join(DATADIR, 'ghidra_names.json'))
    rt = Runtime(pe, names)
    missions = [(canonical_mission_name(n), i, w) for n, i, w in find_missions(pe)]
    if a.list:
        for n, i, w in missions:
            print(n, '%08x' % i, '%08x' % w)
        return
    todo = missions if a.all else [m for m in missions if m[0].lower() == (a.mission or '').lower()]
    if not todo:
        raise SystemExit('unknown mission; use --list')
    namer = load_namer(a.names)
    hashrows = []
    rename_plan = collections.OrderedDict()
    order = sorted(missions, key=lambda m: m[2])
    ends = {}
    for n, i, w in order:
        fdw = FuncDecomp(pe, rt, w)
        ends[n] = (fdw.end + 15) & ~15
    starts = set(a for a in names if BLOCK_LO <= a < BLOCK_HI)
    for n, i, w in todo:
        md_ = MissionDecomp(pe, rt, namer, n, i, w)
        md_.discover()
        k = [m[0] for m in order].index(n)
        lo = ends[order[k - 1][0]] if k else BLOCK_LO
        n_orph = md_.orphans(lo, ends[n], starts)
        text = md_.output()
        out = a.out if (a.out and not a.all) else os.path.join(OUTDIR, n + '.kas')
        open(out, 'w', encoding='utf-8').write(text)
        nf = len(md_.funcs)
        print('%s: %08x-%08x %d functions, %d FSMs, %d states (%d unreferenced), %d failed -> %s' % (
            n, lo, ends[n], nf, len(md_.fsms), sum(len(f['states']) for f in md_.fsms.values()), n_orph,
            len(md_.errors), out))
        for fa, e in list(md_.errors.items())[:10]:
            print('   fail %08x: %s' % (fa, e))
        if a.json:
            js = {'mission': n, 'init': i, 'watch': w, 'range': ['%08x' % lo, '%08x' % ends[n]],
                  'functions': {('%08x' % k): v for k, v in md_.kas2c_names().items()},
                  'errors': {('%08x' % k): v for k, v in md_.errors.items()},
                  'all_functions': sorted('%08x' % k for k in md_.funcs),
                  'lstrings': {('%08x' % k): v for k, v in md_.render.lstrings.items()},
                  'fsms': {('%08x' % k): {'name': md_.fsm_name(k), 'init': '%08x' % (f['init'] or 0),
                                          'watch': '%08x' % (f['watch'] or 0), 'states': len(f['states'])}
                           for k, f in md_.fsms.items()},
                  'unresolved_hashes': len(md_.namer.unresolved)}
            p = a.json if not a.all else os.path.join(DATADIR, n + '_functions.json')
            json.dump(js, open(p, 'w'), indent=1)
        if a.rename:
            for fa, fname in md_.kas2c_names().items():
                rename_plan.setdefault(fa, []).append(fname)
        if a.hashes:
            for (h, kind), cnt in sorted(md_.collect_hashes().items()):
                hashrows.append(TAB.join((n, '%08x' % h, kind, str(cnt))))
    if a.rename:
        rename_in_ghidra(rename_plan)
    if a.hashes:
        open(a.hashes, 'w').write(NL.join([TAB.join(('mission', 'hash', 'kind', 'count'))] + hashrows) + NL)
    rt.save()


def rename_in_ghidra(plan):
    """plan: addr -> [kas2c names]; an ICF-folded function shared by several states keeps the first
    name with an _ICF suffix.  Only addresses inside the mission block are touched."""
    import urllib.request
    ok = bad = 0
    log = []
    for addr, nms in plan.items():
        if not (BLOCK_LO <= addr < BLOCK_HI):
            continue
        uniq = list(dict.fromkeys(nms))
        name = uniq[0] if len(uniq) == 1 else uniq[0] + '_ICF'
        name = re.sub(r'[^A-Za-z0-9_]', '_', name)
        body = json.dumps({'old_name': '0x%08x' % addr, 'new_name': name, 'strict_mode': 'off'}).encode()
        req = urllib.request.Request('http://127.0.0.1:8089/rename_function?program=cataclysm.exe', body,
                                     {'Content-Type': 'application/json'})
        try:
            r = json.load(urllib.request.urlopen(req, timeout=120))
        except Exception as e:     # noqa: BLE001
            r = {'status': 'error', 'message': str(e)}
        if r.get('status') == 'success':
            ok += 1
            log.append(TAB.join(('%08x' % addr, name)))
        else:
            bad += 1
            log.append(TAB.join(('%08x' % addr, name, 'FAILED: ' + str(r.get('message') or r.get('error'))[:200])))
    open(os.path.join(DATADIR, 'ghidra_renames.tsv'), 'w').write(NL.join(log) + NL)
    print('ghidra renames: %d ok, %d failed (see data/ghidra_renames.tsv)' % (ok, bad))


if __name__ == '__main__':
    main()
