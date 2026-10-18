"""Helper for naming cataclysm.exe functions against HW1 source (used by naming workers).

Commands (Ghidra MCP server on 127.0.0.1:8089 must be running):
  window <HW1 file.c>        candidate cataclysm functions whose link-order window allows this file,
                             in address order (addr, name, size, #strings, callee names), plus the
                             HW1 functions defined in the file in source order
  decomp <addr|name>         decompiled C of one cataclysm function
  hw1 <function>             the HW1 source of a function (from reference/hw1-2003)
  callers <addr|name>        functions calling it
  rename <addr> <name> <group> [note]
                             rename in Ghidra and append to recovered/naming/<group>.tsv
"""
import json, os, re, sys, urllib.parse, urllib.request

ROOT = r'D:\Git\HWC_Modern'
B, PROG = 'http://127.0.0.1:8089', 'cataclysm.exe'
SRC = os.path.join(ROOT, 'reference', 'hw1-2003', 'src')
FUNC = re.compile(r'^[A-Za-z_][\w \t\*]*?\b([A-Za-z_]\w*)\s*\([^;{}]*\)\s*\{', re.M)


def get(path, **q):
    return json.load(urllib.request.urlopen(B + path + '?' + urllib.parse.urlencode(dict(q, program=PROG)), timeout=600))


def post(path, body):
    req = urllib.request.Request(B + path + '?program=' + PROG, json.dumps(body).encode(), {'Content-Type': 'application/json'})
    return json.load(urllib.request.urlopen(req, timeout=600))


def link_order():
    order = []
    for sub in ('Game', 'SDL', 'Ships', 'Generated'):
        order += sorted((f for f in os.listdir(os.path.join(SRC, sub)) if f.lower().endswith(('.c', '.cpp'))), key=str.lower)
    return {f: i for i, f in enumerate(order)}


def hw1_functions(fname):
    for sub in ('Game', 'SDL', 'Ships', 'Generated'):
        p = os.path.join(SRC, sub, fname)
        if os.path.exists(p):
            txt = open(p, encoding='latin1').read()
            return p, txt, [(m.group(1), m.start()) for m in FUNC.finditer(txt) if m.group(1) not in ('if', 'while', 'for', 'switch')]
    raise SystemExit('no such HW1 file: ' + fname)


def cmd_window(fname):
    rank = link_order()
    r = rank[fname]
    feats = {}
    for l in open(os.path.join(ROOT, 'recovered', 'features', 'cataclysm.jsonl')):
        f = json.loads(l); feats[f['addr']] = f
    live = {f['address']: f['name'] for f in get('/find_functions', limit=100000)['functions']}
    print('# candidate cataclysm functions for', fname)
    for l in list(open(os.path.join(ROOT, 'recovered', 'module_map.tsv')))[1:]:
        a, _, f = l.rstrip('\n').split('\t')
        lo, hi = (f[4:].split('|') if f.startswith('gap:') else (f, f))
        if rank.get(lo, -1) <= r <= rank.get(hi, 10 ** 6):
            ft = feats.get(a, {})
            callees = [live.get(c, c) if c.startswith('FUN_') else c for c in ft.get('callees', [])]
            print(a, live.get(a, '?'), ft.get('size', '?'), 'strings=%d' % len(ft.get('strings', [])),
                  'calls=' + ','.join(dict.fromkeys(callees))[:200])
    _, _, fns = hw1_functions(fname)
    print('# HW1 functions in', fname, '(source order):', ' '.join(n for n, _ in fns))


def cmd_hw1(name):
    rank = link_order()
    for fname in rank:
        try:
            p, txt, fns = hw1_functions(fname)
        except SystemExit:
            continue
        for i, (n, start) in enumerate(fns):
            if n == name:
                j, d = txt.index('{', start), 0
                k = j
                while k < len(txt):
                    if txt[k] == '{': d += 1
                    elif txt[k] == '}':
                        d -= 1
                        if d == 0: break
                    k += 1
                print('//', fname); print(txt[start:k + 1]); return
    print('not found')


def cmd_rename(addr, name, group, note=''):
    old = get('/force_decompile', function=addr).get('name') or 'FUN_' + addr
    r = post('/rename_function', {'old_name': old, 'new_name': name, 'strict_mode': False})
    ok = r.get('status') == 'success' or r.get('success') is True
    print(json.dumps(r)[:300])
    if ok:
        d = os.path.join(ROOT, 'recovered', 'naming'); os.makedirs(d, exist_ok=True)
        with open(os.path.join(d, group + '.tsv'), 'a') as o:
            o.write('%s\t%s\t%s\t%s\n' % (addr, old, name, note))


if __name__ == '__main__':
    c, args = sys.argv[1], sys.argv[2:]
    if c == 'window': cmd_window(*args)
    elif c == 'decomp': print(get('/force_decompile', function=args[0]).get('decompiled', ''))
    elif c == 'hw1': cmd_hw1(args[0])
    elif c == 'callers':
        for e in get('/get_function_call_graph', function_name=args[0], depth=1).get('edges', []):
            if e['callee'].split('@')[0] == args[0] or e['callee'].endswith(args[0]): print(e['caller'])
    elif c == 'rename': cmd_rename(*args)
    else: print(__doc__)
