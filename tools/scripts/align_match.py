"""Match cataclysm.exe functions to HW1 functions by aligning them in link order, per source file.

Both binaries lay functions out object file by object file, and within an object in source order. For each
HW1 source file F we take
  - the HW1 reference build's functions defined in F (hw1_ref.dll, names from its PDB), in address order;
  - the cataclysm.exe functions whose link-order bounds (module_map.tsv) allow F, in address order;
and run a Needleman-Wunsch alignment scored on compiler-robust features (strings, large/float constants,
block count, callee count, and callees already matched). Aligned pairs scoring above THRESH are proposals.
Two passes: the second uses callee matches from the first.

Usage: align_match.py <hw1 src> <module_map.tsv> <cat.jsonl> <ref.jsonl> <out.tsv>
"""
import collections, json, os, re, sys

srcdir, mapfile, catf, reff, out = sys.argv[1:6]
THRESH, GAP = 2.2, -0.6

# HW1 function -> file (from source) and link rank of files
FUNC = re.compile(r'^[A-Za-z_][\w \t\*]*?\b([A-Za-z_]\w*)\s*\([^;{}]*\)\s*\{', re.M)
order, hw1_file = [], {}
for sub in ('Game', 'SDL', 'Ships', 'Generated'):
    fs = sorted((f for f in os.listdir(os.path.join(srcdir, sub)) if f.lower().endswith(('.c', '.cpp'))), key=str.lower)
    order += fs
    for f in fs:
        for m in FUNC.finditer(open(os.path.join(srcdir, sub, f), encoding='latin1').read()):
            hw1_file.setdefault(m.group(1), f)
rank = {f: i for i, f in enumerate(order)}

cat = [json.loads(l) for l in open(catf)]
ref = [json.loads(l) for l in open(reff)]
cat.sort(key=lambda r: int(r['addr'], 16))
ref.sort(key=lambda r: int(r['addr'], 16))
cat_by_name = {r['name']: r for r in cat}
ref_names = {r['name'] for r in ref}

bounds = {}
for l in list(open(mapfile))[1:]:
    a, _, f = l.rstrip('\n').split('\t')
    if f.startswith('gap:'):
        lo, hi = f[4:].split('|')
        bounds[a] = (rank.get(lo, -1), rank.get(hi, 10 ** 6))
    else:
        bounds[a] = (rank[f], rank[f]) if f in rank else (-1, 10 ** 6)


def feat(r):
    big = collections.Counter(c for c in r['consts'] if c > 0xffff)  # floats / magic numbers, not offsets
    return set(r['strings']), big, r['blocks'], len(r['callees'])


catF = {r['addr']: feat(r) for r in cat}
refF = {r['addr']: feat(r) for r in ref}
mapping = {}  # cat name -> ref name


def score(c, h):
    if c['name'] == h['name']: return 10.0
    sc, kc, bc, nc = catF[c['addr']]
    sh, kh, bh, nh = refF[h['addr']]
    s = 0.0
    if sc or sh:
        j = len(sc & sh) / len(sc | sh)
        s += 5 * j - (1.5 if not (sc & sh) else 0)
    if kc or kh:
        ov = sum((kc & kh).values())
        s += 3 * (2 * ov / (sum(kc.values()) + sum(kh.values()))) - (0.5 if not ov else 0)
    s += 1.5 * (1 - abs(bc - bh) / max(bc, bh, 1)) - 0.6
    s += 1.0 * (1 - abs(nc - nh) / max(nc, nh, 1)) - 0.4
    agree = sum(1 for x in c['callees'] if mapping.get(x) in set(h['callees']))
    if c['callees'] and h['callees']:
        s += 3 * agree / max(len(set(c['callees'])), len(set(h['callees'])))
    return s


def nw(A, B):
    n, m = len(A), len(B)
    S = [[0.0] * (m + 1) for _ in range(n + 1)]
    T = [[0] * (m + 1) for _ in range(n + 1)]
    for i in range(1, n + 1): S[i][0] = S[i - 1][0] + GAP; T[i][0] = 1
    for j in range(1, m + 1): S[0][j] = S[0][j - 1] + GAP; T[0][j] = 2
    sc = {}
    for i in range(1, n + 1):
        for j in range(1, m + 1):
            v = score(A[i - 1], B[j - 1]); sc[i, j] = v
            best, t = S[i - 1][j - 1] + v, 0
            if S[i - 1][j] + GAP > best: best, t = S[i - 1][j] + GAP, 1
            if S[i][j - 1] + GAP > best: best, t = S[i][j - 1] + GAP, 2
            S[i][j], T[i][j] = best, t
    i, j, pairs = n, m, []
    while i > 0 and j > 0:
        t = T[i][j]
        if t == 0: pairs.append((A[i - 1], B[j - 1], sc[i, j])); i -= 1; j -= 1
        elif t == 1: i -= 1
        else: j -= 1
    return pairs


ref_by_file = collections.defaultdict(list)
for r in ref:
    f = hw1_file.get(r['name'])
    if f: ref_by_file[f].append(r)

results = {}
for pas in range(2):
    results = {}
    for f, R in ref_by_file.items():
        rf = rank[f]
        C = [c for c in cat if bounds.get(c['addr'], (-1, 10 ** 6))[0] <= rf <= bounds.get(c['addr'], (-1, 10 ** 6))[1]]
        if not C or len(C) * len(R) > 4_000_000: continue
        for c, h, s in nw(C, R):
            if s >= THRESH and (c['addr'] not in results or results[c['addr']][2] < s):
                results[c['addr']] = (c['name'], h['name'], s, f)
    # one HW1 name per cataclysm function and vice versa: keep the best scoring
    best_for_ref = {}
    for a, (cn, hn, s, f) in results.items():
        if hn not in best_for_ref or best_for_ref[hn][1] < s: best_for_ref[hn] = (a, s)
    results = {a: v for a, v in results.items() if best_for_ref[v[1]][0] == a}
    mapping = {v[0]: v[1] for v in results.values()}
    print('pass', pas, 'matches', len(results), 'of which already named', sum(1 for v in results.values() if v[0] == v[1]))

with open(out, 'w') as o:
    o.write('address\tcurrent\tproposed\tscore\tfile\n')
    for a in sorted(results):
        cn, hn, s, f = results[a]
        o.write('%s\t%s\t%s\t%.2f\t%s\n' % (a, cn, hn, s, f))
