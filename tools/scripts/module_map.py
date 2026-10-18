"""Assign every cataclysm.exe function to an original source file using linker order.

MSVC 6 laid .text out object by object in link order, which for this build is: src/game/*.c
alphabetically, then src/win32/*, then ships, then generated missions, then libraries. The string
anchors (names_string_anchors.tsv) give file labels at scattered addresses; the anchors consistent with
that order (longest non-decreasing subsequence by file rank) become boundaries, and every function between
two anchors of the same file belongs to it. Functions between anchors of different files are 'gap:A|B'.

Usage: module_map.py anchors.tsv hw1_src_dir out.tsv
"""
import bisect, collections, json, os, sys, urllib.request

anchors_tsv, srcdir, out = sys.argv[1:4]
B = 'http://127.0.0.1:8089'

# file rank = position in link order: Game, SDL(win32), Ships, Generated, then anything else
order = []
for sub in ('Game', 'SDL', 'Ships', 'Generated'):
    d = os.path.join(srcdir, sub)
    order += sorted((f for f in os.listdir(d) if f.lower().endswith(('.c', '.cpp'))), key=str.lower)
rank = {f: i for i, f in enumerate(order)}

pts = []
for l in list(open(anchors_tsv))[1:]:
    fn, _, f, n, conf = l.rstrip('\n').split('\t')
    if fn.startswith('FUN_') and f in rank:
        pts.append((int(fn[4:], 16), rank[f], f))
pts.sort()

# longest non-decreasing subsequence on rank (patience sorting with predecessor links)
tails, tails_idx, prev = [], [], [None] * len(pts)
for i, (_, r, _) in enumerate(pts):
    k = bisect.bisect_right(tails, r)
    if k == len(tails): tails.append(r); tails_idx.append(i)
    else: tails[k] = r; tails_idx[k] = i
    prev[i] = tails_idx[k - 1] if k else None
keep, i = [], tails_idx[-1]
while i is not None: keep.append(pts[i]); i = prev[i]
keep.reverse()
print('anchors', len(pts), 'consistent with link order', len(keep))

# all functions
funcs, off = [], 0
while True:
    with urllib.request.urlopen(f'{B}/find_functions?limit=5000&offset={off}', timeout=300) as r:
        page = json.load(r)
    items = page.get('functions', [])
    funcs += items
    if len(items) < 5000: break
    off += 5000
print('functions', len(funcs))

addrs = [a for a, _, _ in keep]
per_file = collections.Counter()
with open(out, 'w') as o:
    o.write('address\tname\tfile\n')
    for fobj in funcs:
        a = int(fobj['address'], 16)
        j = bisect.bisect_right(addrs, a)
        lo = keep[j - 1] if j else None
        hi = keep[j] if j < len(keep) else None
        if lo and hi and lo[2] == hi[2]: f = lo[2]
        elif lo and a == lo[0]: f = lo[2]
        else: f = 'gap:%s|%s' % (lo[2] if lo else '^', hi[2] if hi else '$')
        per_file[f if not f.startswith('gap') else 'gap'] += 1
        o.write('%08x\t%s\t%s\n' % (a, fobj['name'], f))
print('assigned', sum(v for k, v in per_file.items() if k != 'gap'), 'between-file gaps', per_file['gap'])
