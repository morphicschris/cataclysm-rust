"""Export the structured mission IR of every Cataclysm KAS mission as JSON (dev path for hwc-kas).

This is the "--json structured form" consumed by `hwc_kas::json::load_mission`. It reuses the
decompiler in tools/scripts/kas_decompile.py (machine code -> structured AST) and lowers its AST to
the hwc-kas IR shape, keeping everything the .kas text rendering loses:

  * IFONCE guard variable hashes (the text form drops them),
  * signed vs unsigned comparisons,
  * switch statements over a single evaluated value (the text form renders IF/ELSEIF chains),
  * the exact G_ bit (bit 31) of every variable / timer hash,
  * -(cond) values produced by sbb idioms,
  * block addresses, LSTRING table addresses, label names (when known).

The output is game-derived content and belongs in the private recovered/ tree; it is never
published. Run:  python crates/hwc-kas/tools/kas_ir_export.py [-o DIR] [Mission01 ...]
"""
import argparse
import collections
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.normpath(os.path.join(HERE, '..', '..', '..'))
sys.path.insert(0, os.path.join(ROOT, 'tools', 'scripts'))
import kas_decompile as K          # noqa: E402
from kas_hash import kas_hash      # noqa: E402

FORMAT = 'hwc-kas-ir'
VERSION = 1


class ExportError(Exception):
    pass


def hx(v):
    return '%08x' % (v & 0xffffffff)


class Exporter:
    def __init__(self, pe, rt, md, table_index):
        self.pe, self.rt, self.md = pe, rt, md
        self.R = md.render
        self.table_index = table_index          # C name -> first table index
        self.hashes = set()
        self.negcond = 0

    # ---- values ---------------------------------------------------------------------------
    def func_index(self, addr):
        name = self.rt.name(addr)
        idx = self.table_index.get(name)
        if idx is None:
            raise ExportError('call to non-KAS function %s (%08x)' % (name, addr))
        return idx

    def val(self, v, ptype=None):
        if isinstance(v, K.Const):
            if ptype == 'CHARPTR' and v.v > 0x400000:
                s = self.pe.cstr(v.v, 4096)
                if s is not None:
                    return ['str', s]
            if ptype == 'UDWORD':
                self.hashes.add(v.v)
            return ['int', v.s]
        if isinstance(v, K.Sym):
            if v.name == 'team':
                return ['this', 'team']
            raise ExportError('unexpected symbol %s' % v.name)
        if isinstance(v, K.Call):
            return self.call_value(v)
        if isinstance(v, K.Index):
            if isinstance(v.idx, K.Sym) and v.idx.name == 'lang' and v.scale == 4:
                self.R.lstring(v.table)
                return ['lstring', hx(v.table)]
            raise ExportError('table access %r' % (v,))
        if isinstance(v, K.Bin):
            if isinstance(v.a, K.Sym) and v.a.name == 'team' and v.op == '+' and isinstance(v.b, K.Const) \
                    and v.b.v == 0x1c:
                return ['this', 'teamships']
            if v.op not in ('+', '-', '*', '/', '%', '&', '|', '^', '<<', '>>'):
                raise ExportError('operator %s' % v.op)
            return ['bin', v.op, self.val(v.a), self.val(v.b)]
        if isinstance(v, K.Un):
            if isinstance(v.a, K.Cond):
                if v.op != '-':
                    raise ExportError('unary %s of condition' % v.op)
                self.negcond += 1
                return ['un', '-', ['cond', self.cond(v.a)]]
            if v.op not in ('-', '~'):
                raise ExportError('unary %s' % v.op)
            return ['un', v.op, self.val(v.a)]
        if isinstance(v, K.Cond):
            return ['cond', self.cond(v)]
        raise ExportError('unsupported value %r' % (v,))

    def call_value(self, c):
        a = c.addr
        if a in K.LABEL_FUNCS and len(c.args) == 1 and isinstance(c.args[0], K.Const):
            self.hashes.add(c.args[0].v)
            return ['label', K.LABEL_FUNCS[a], hx(c.args[0].v)]
        if a == K.A_THISTEAMSPOINT:
            return ['this', 'teamspoint']
        if a in (K.A_MSGGET, K.A_JUMP, K.A_FSMCREATE, K.A_MSGSEND_RAW):
            raise ExportError('runtime call %08x used as a value' % a)
        idx = self.func_index(a)
        e = self.rt.entry(a)
        types = [p[1] for p in e['params']]
        if len(c.args) != len(types):
            raise ExportError('%s: %d args, table says %d' % (e['kas'], len(c.args), len(types)))
        return ['call', idx, [self.val(x, types[k]) for k, x in enumerate(c.args)]]

    def cond(self, c):
        if not isinstance(c, K.Cond):
            # a bare value used as a condition: C truth
            return ['cmp', '!=', self.val(c), ['int', 0]]
        op = c.op
        if op in ('and', 'or'):
            return [op, self.cond(c.a), self.cond(c.b)]
        if op == 'not':
            return ['not', self.cond(c.a)]
        if op in ('true', 'false'):
            return [op]
        if op in K.NEG_REL:
            return ['cmp', op, self.val(c.a), self.val(c.b)]
        raise ExportError('condition %r' % (c,))

    # ---- statements -----------------------------------------------------------------------
    def block(self, addr):
        fd = self.md.funcs.get(addr)
        if fd is None:
            raise ExportError('function %08x not decompiled' % addr)
        if fd.ast is None:
            raise ExportError('function %08x failed: %s' % (addr, self.md.errors.get(addr)))
        scope = None
        stmts = list(fd.ast)
        for s in stmts:
            if s[0] == 'scope' and isinstance(s[1], K.Const):
                scope = s[1].v
        return {'addr': hx(addr), 'scope': scope, 'stmts': self.stmts(stmts, top=True)}

    def stmts(self, ast, top=False, inloop=False):
        out = []
        k = 0
        while k < len(ast):
            s = ast[k]
            kind = s[0]
            last = k == len(ast) - 1
            if kind in ('scope', 'scopename', 'continue'):
                pass
            elif kind == 'call':
                c = s[1]
                if c.addr == K.A_JUMP and len(c.args) == 3 and isinstance(c.args[0], K.Const):
                    self.hashes.add(c.args[0].v)
                    out.append(['jump', hx(c.args[0].v)])
                    if k + 1 < len(ast) and ast[k + 1][0] == 'return':
                        k += 1
                elif c.addr == K.A_FSMCREATE and len(c.args) == 4 and isinstance(c.args[0], K.Const):
                    self.hashes.add(c.args[0].v)
                    out.append(['fsmcreate', hx(c.args[0].v), self.val(c.args[3])])
                elif c.addr in K.LABEL_FUNCS or c.addr == K.A_THISTEAMSPOINT:
                    pass          # CSE'd label lookup, no effect
                elif c.addr == K.A_MSGSEND_RAW and inloop:
                    out.append(['requeue'])
                else:
                    v = self.call_value(c)
                    out.append(['call', v[1], v[2]])
            elif kind == 'return':
                if not (top and last):
                    out.append(['return'])
            elif kind == 'if':
                out.append(self.if_stmt(s, inloop))
            elif kind == 'switch':
                var = s[1]
                if isinstance(var, K.Call) and var.addr == K.A_MSGGET:
                    raise ExportError('message switch outside a message loop')
                arms = [[[int(x if x < 0x80000000 else x - (1 << 32)) for x in vals], self.stmts(arm, inloop=inloop)]
                        for vals, arm in s[2]]
                dflt = self.stmts(s[3], inloop=inloop) if s[3] is not None else None
                out.append(['switch', self.val(var), arms, dflt])
            elif kind == 'msgloop':
                out.append(self.msgloop(s[1]))
            else:
                raise ExportError('statement %r' % (s,))
            k += 1
        return out

    def if_stmt(self, s, inloop):
        c, then, els = s[1], s[2], s[3]
        once = self.md.match_ifonce(c, then)
        if once is not None:
            h = then[0][1].args[0].v
            self.hashes.add(h)
            cond2, body = once
            return ['ifonce', hx(h), self.cond(cond2) if cond2 is not None else None,
                    self.stmts(body, inloop=inloop), self.stmts(els, inloop=inloop)]
        return ['if', self.cond(c), self.stmts(then, inloop=inloop), self.stmts(els, inloop=inloop)]

    def msgloop(self, body):
        body = [x for x in body if x[0] not in ('scope', 'scopename', 'continue')]
        if len(body) != 1 or body[0][0] != 'switch':
            raise ExportError('unexpected message loop body %r' % (body,))
        sw = body[0]
        var = sw[1]
        if not (isinstance(var, K.Call) and var.addr == K.A_MSGGET):
            raise ExportError('message loop does not switch on the message')
        handlers = []
        for vals, arm in sw[2]:
            for v in vals:
                self.hashes.add(v)
            handlers.append([[hx(v) for v in vals], self.stmts(arm, inloop=True)])
        dflt = None
        if sw[3] is not None:
            d = self.stmts(sw[3], inloop=True)
            if d != [['requeue']]:
                dflt = d
        return ['msgloop', handlers, dflt]


def export_mission(pe, rt, namer, table_index, n, i, w, lo, hi, starts):
    md = K.MissionDecomp(pe, rt, namer, n, i, w)
    md.discover()
    md.orphans(lo, hi, starts)
    ex = Exporter(pe, rt, md, table_index)
    fsms = []
    for fh, f in md.fsms.items():
        ex.hashes.add(fh)
        states = []
        for sh, (si, sw) in f['states'].items():
            ex.hashes.add(sh)
            states.append({'hash': hx(sh), 'init': ex.block(si), 'watch': ex.block(sw)})
        fsms.append({'hash': hx(fh), 'init': ex.block(f['init']), 'watch': ex.block(f['watch']), 'states': states})
    init = ex.block(i)
    watch = ex.block(w)
    lstrings = [{'addr': hx(t), 'name': nm, 'texts': ex.R.lstring_text[t]} for t, nm in ex.R.lstrings.items()]
    names = {}
    for h in sorted(ex.hashes):
        nm = namer.names.get(h & 0x7fffffff)
        if nm:
            names[hx(h & 0x7fffffff)] = nm
    return {
        'format': FORMAT, 'version': VERSION,
        'mission': n, 'name_hash': hx(kas_hash(n)),
        'init_addr': hx(i), 'watch_addr': hx(w), 'range': [hx(lo), hx(hi)],
        'lstrings': lstrings, 'names': names, 'fsms': fsms, 'init': init, 'watch': watch,
    }, ex.negcond


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('missions', nargs='*')
    ap.add_argument('-o', '--out', default=os.path.join(ROOT, 'recovered', 'missions', 'ir'))
    a = ap.parse_args()
    pe = K.PE()
    names = K.load_function_names(os.path.join(K.DATADIR, 'ghidra_names.json'))
    rt = K.Runtime(pe, names)
    table = json.load(open(os.path.join(K.DATADIR, 'kas_function_table.json')))
    table_index = {}
    for k, e in enumerate(table):
        table_index.setdefault(e['c'], k)
    missions = [(K.canonical_mission_name(n), i, w) for n, i, w in K.find_missions(pe)]
    order = sorted(missions, key=lambda m: m[2])
    ends = {}
    for n, i, w in order:
        ends[n] = (K.FuncDecomp(pe, rt, w).end + 15) & ~15
    starts = set(x for x in names if K.BLOCK_LO <= x < K.BLOCK_HI)
    os.makedirs(a.out, exist_ok=True)
    namer = K.load_namer(None)
    todo = [m for m in missions if not a.missions or m[0].lower() in [x.lower() for x in a.missions]]
    stats = collections.Counter()
    for n, i, w in todo:
        k = [m[0] for m in order].index(n)
        lo = ends[order[k - 1][0]] if k else K.BLOCK_LO
        js, negc = export_mission(pe, rt, namer, table_index, n, i, w, lo, ends[n], starts)
        p = os.path.join(a.out, n + '.json')
        with open(p, 'w', encoding='utf-8') as f:
            json.dump(js, f, indent=None, separators=(',', ':'), ensure_ascii=False)
        stats['negcond'] += negc
        print('%s: %d FSMs, %d states -> %s' % (n, len(js['fsms']), sum(len(f['states']) for f in js['fsms']), p))
    print('negated-condition values:', stats['negcond'])
    rt.save()


if __name__ == '__main__':
    main()
