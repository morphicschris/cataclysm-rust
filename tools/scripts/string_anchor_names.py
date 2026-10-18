"""Propose names for cataclysm.exe functions by matching string literals against HW1 source.

For every string literal in reference/hw1-2003 that occurs in exactly one HW1 function, find the
Cataclysm string with the same text and the function(s) that reference it. When all anchors that land
on one Cataclysm function agree on one HW1 function name, that is a proposal.

Writes a TSV (address, current name, proposed name, source file, anchor count, conflicts).
Does not rename anything. Usage: string_anchor_names.py <hw1 src dir> <out.tsv>
"""
import collections, json, os, re, sys, urllib.parse, urllib.request

B = 'http://127.0.0.1:8089'
srcdir, out = sys.argv[1], sys.argv[2]


def get(path, **q):
    with urllib.request.urlopen(B + path + '?' + urllib.parse.urlencode(q), timeout=300) as r:
        return json.load(r)


# --- HW1: string literal -> set of (function, file)
FUNC = re.compile(r'^[A-Za-z_][\w \t\*]*?\b([A-Za-z_]\w*)\s*\(([^;{}]*)\)\s*\{', re.M)
LIT = re.compile(r'"((?:[^"\\\n]|\\.){5,})"')
lit_funcs = collections.defaultdict(set)
for dp, _, fs in os.walk(srcdir):
    for f in fs:
        if not f.lower().endswith(('.c', '.cpp')): continue
        txt = open(os.path.join(dp, f), encoding='latin1').read()
        for m in FUNC.finditer(txt):
            name = m.group(1)
            if name in ('if', 'while', 'for', 'switch', 'return', 'sizeof'): continue
            # body = balanced braces from the match end
            i, d = m.end() - 1, 0
            while i < len(txt):
                if txt[i] == '{': d += 1
                elif txt[i] == '}':
                    d -= 1
                    if d == 0: break
                i += 1
            for lm in LIT.finditer(txt[m.end():i]):
                s = bytes(lm.group(1), 'latin1').decode('unicode_escape', 'ignore')
                lit_funcs[s].add((name, f))
unique = {s: next(iter(v)) for s, v in lit_funcs.items() if len(v) == 1}
print('HW1 literals unique to one function:', len(unique))

# --- Cataclysm strings
strings, off = [], 0
while True:
    r = get('/list_strings', limit=5000, offset=off)
    strings += r['strings']
    if len(r['strings']) < 5000: break
    off += 5000
print('cataclysm strings:', len(strings))

votes = collections.defaultdict(collections.Counter)
cur = {}
hits = 0
for s in strings:
    v = s['value']
    if v not in unique: continue
    hits += 1
    refs = get('/get_xrefs_to', address=s['address']).get('references', [])
    for ref in refs:
        fn = ref.get('from_function')
        if not fn: continue
        votes[fn][unique[v]] += 1
print('matching anchor strings:', hits)

with open(out, 'w') as o:
    o.write('function\tproposed\tfile\tanchors\tconflicting\n')
    for fn, c in sorted(votes.items()):
        (name, f), n = c.most_common(1)[0]
        other = sum(c.values()) - n
        o.write('%s\t%s\t%s\t%d\t%d\n' % (fn, name, f, n, other))
print('functions with proposals:', len(votes))
