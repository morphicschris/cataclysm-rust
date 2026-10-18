"""List a Homeworld RBF1.23 .big archive's TOC (read-only). Writes a TSV of name, sizes, compression."""
import struct, sys, collections
path, out = sys.argv[1], sys.argv[2]
f = open(path, 'rb')
hdr = f.read(7)
assert hdr == b'RBF1.23', hdr
n, flags = struct.unpack('<ii', f.read(8))
ents = [struct.unpack('<IIHxxIIIIB3x', f.read(32)) for _ in range(n)]
rows = []
for c1, c2, nl, stored, real, off, ts, comp in ents:
    f.seek(off)
    raw = bytearray(f.read(nl))
    m = 213
    for i in range(nl):
        b = raw[i]; raw[i] ^= m; m = raw[i]
    rows.append((raw.decode('latin1'), stored, real, comp, ts))
rows.sort()
with open(out, 'w', encoding='utf-8') as o:
    o.write('name\tstored\treal\tcompressed\ttimestamp\n')
    for r in rows: o.write('\t'.join(map(str, r)) + '\n')
print('files', n, 'flags', flags, 'compressed', sum(r[3] for r in rows), 'real bytes', sum(r[2] for r in rows))
top = collections.Counter(); ext = collections.Counter()
for r in rows:
    parts = r[0].replace('/', '\\').split('\\')
    top[parts[0].lower() if len(parts) > 1 else '(root)'] += 1
    ext[r[0].rsplit('.', 1)[-1].lower() if '.' in r[0] else '(none)'] += 1
print('top dirs:', top.most_common(40))
print('extensions:', ext.most_common(40))
