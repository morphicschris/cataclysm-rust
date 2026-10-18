import re, sys, collections, math
d = open(sys.argv[1], 'rb').read()
ss = [m.group().decode('latin1') for m in re.finditer(rb'[\x20-\x7e]{5,}', d)]
print('total strings', len(ss))
BS = chr(92)
src = collections.Counter(s for s in ss if re.search(r'\.(c|cpp|h|asm)\b', s, re.I) and (BS in s or '/' in s or len(s) < 40))
print('source-file-like strings', len(src))
for s in sorted(src): print('  ', s)

def ent(b):
    c = collections.Counter(b); n = len(b)
    return -sum(v / n * math.log2(v / n) for v in c.values())

print('entropy text chunks:', [round(ent(d[o:o + 0x40000]), 2) for o in range(0x1000, 0x320000, 0x40000)])
print('entropy cms_t', round(ent(d[0x6fa000:0x70d000]), 2))
