"""Propagate HW1 function names through the call graph.

For every cataclysm.exe function that already has an HW1 name, compare the ordered list of calls in its
decompiled body with the ordered list of calls in the same HW1 function, align the two sequences
(difflib), and vote: an unnamed callee FUN_xxxxxxxx aligned to HW1 callee `foo` gets one vote for `foo`.
A proposal is accepted when it has >= MIN_VOTES votes and no competing name, and `foo` is not already
used. Repeat for ROUNDS rounds, since each round names new functions whose bodies seed the next.

A proposal must also be consistent with link order: the HW1 file defining `foo` must rank between the
files of the nearest anchors around the function's address (module_map.tsv).

Usage: callgraph_propagate.py <hw1 src dir> <module_map.tsv> <log.tsv> [--apply]
"""
import collections, difflib, json, os, re, sys, urllib.parse, urllib.request

B = 'http://127.0.0.1:8089'
srcdir, mapfile, logpath = sys.argv[1], sys.argv[2], sys.argv[3]
APPLY = '--apply' in sys.argv
ROUNDS, MIN_VOTES = 8, 1
SKIP = {'if', 'while', 'for', 'switch', 'return', 'sizeof', 'dbgAssert', 'dbgMessage', 'dbgMessagef',
        'dbgAssertOrIgnore', 'defined', 'min', 'max', 'abs', 'ABS', 'MIN', 'MAX', 'bitTest', 'bitSet',
        'bitClear', 'isnan', 'TypeOfIsObject'}


def call(path, body=None, **q):
    url = B + path + ('?' + urllib.parse.urlencode(q) if q else '')
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data, {'Content-Type': 'application/json'} if data else {})
    with urllib.request.urlopen(req, timeout=300) as r:
        return json.load(r)


# --- HW1 ordered call lists
FUNC = re.compile(r'^[A-Za-z_][\w \t\*]*?\b([A-Za-z_]\w*)\s*\(([^;{}]*)\)\s*\{', re.M)
CALL = re.compile(r'\b([A-Za-z_]\w*)\s*\(')
STRIP = re.compile(r'"(?:[^"\\\n]|\\.)*"|//[^\n]*|/\*.*?\*/', re.S)
hw1, hw1_file = {}, {}
order = []
for sub in ('Game', 'SDL', 'Ships', 'Generated'):
    order += sorted((f for f in os.listdir(os.path.join(srcdir, sub)) if f.lower().endswith(('.c', '.cpp'))), key=str.lower)
rank = {f: i for i, f in enumerate(order)}
for dp, _, fs in os.walk(srcdir):
    for f in fs:
        if not f.lower().endswith(('.c', '.cpp')): continue
        txt = STRIP.sub('""', open(os.path.join(dp, f), encoding='latin1').read())
        for m in FUNC.finditer(txt):
            name = m.group(1)
            if name in SKIP: continue
            i, d = m.end() - 1, 0
            while i < len(txt):
                if txt[i] == '{': d += 1
                elif txt[i] == '}':
                    d -= 1
                    if d == 0: break
                i += 1
            calls = [c for c in CALL.findall(txt[m.end():i]) if c not in SKIP and not c.isupper()]
            if name not in hw1 or len(calls) > len(hw1[name]): hw1[name] = calls; hw1_file[name] = f
print('HW1 functions parsed:', len(hw1))

# --- cataclysm functions
funcs, off = [], 0
while True:
    page = call('/find_functions', limit=5000, offset=off)['functions']
    funcs += page
    if len(page) < 5000: break
    off += 5000
names = {f['name'] for f in funcs}
bounds = {}
for l in list(open(mapfile))[1:]:
    a, _, f = l.rstrip('\n').split('\t')
    if f.startswith('gap:'):
        lo, hi = f[4:].split('|')
        bounds['FUN_' + a] = (rank.get(lo, -1), rank.get(hi, 10 ** 6))
    else:
        bounds['FUN_' + a] = (rank.get(f, -1), rank.get(f, 10 ** 6))


def placed_ok(fun, hw1name):
    r = rank.get(hw1_file.get(hw1name), None)
    lo, hi = bounds.get(fun, (-1, 10 ** 6))
    return r is not None and lo <= r <= hi
decomp_cache = {}
DCALL = re.compile(r'\b((?:FUN_[0-9a-f]{8})|[A-Za-z_]\w*)\s*\(')


def cat_calls(fname):
    if fname not in decomp_cache:
        try:
            code = call('/force_decompile', name=fname).get('decompiled', '')
        except Exception:
            code = ''
        body = code[code.find('{'):] if '{' in code else ''
        decomp_cache[fname] = [c for c in DCALL.findall(body) if c in names or c.startswith('FUN_')]
    return decomp_cache[fname]


log = open(logpath, 'a')
done = set()
for rnd in range(ROUNDS):
    named = [f['name'] for f in funcs if f['name'] in hw1 and f['name'] not in done]
    votes = collections.defaultdict(collections.Counter)
    for fn in named:
        done.add(fn)
        a, b = cat_calls(fn), hw1[fn]
        sm = difflib.SequenceMatcher(None, [x if not x.startswith('FUN_') else '?' for x in a], b, autojunk=False)
        for tag, i1, i2, j1, j2 in sm.get_opcodes():
            if tag == 'replace' and (i2 - i1) == (j2 - j1):
                for k in range(i2 - i1):
                    if a[i1 + k].startswith('FUN_') and b[j1 + k] in hw1:
                        votes[a[i1 + k]][b[j1 + k]] += 1
    taken = {f['name'] for f in funcs}
    accepted = 0
    for fun, c in votes.items():
        (best, n), = c.most_common(1)
        if n < MIN_VOTES or len(c) > 1 or best in taken or not placed_ok(fun, best): continue
        ok = True
        if APPLY:
            r = call('/rename_function', {'old_name': fun, 'new_name': best, 'strict_mode': False})
            ok = r.get('status') == 'success' or r.get('success') is True
        if ok:
            taken.add(best); accepted += 1
            for f in funcs:
                if f['name'] == fun: f['name'] = best
            names.add(best)
            log.write('%d\t%s\t%s\t%d\n' % (rnd, fun, best, n))
    log.flush()
    print('round', rnd, 'named sources', len(named), 'accepted', accepted)
    if not accepted: break
