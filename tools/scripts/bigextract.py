"""Extract every file from an RBF1.23 .big archive (read-only on the source) into an output directory.
Stopgap so format work can start before the Rust big/LZSS reader exists. Verifies each expanded size.
Usage: bigextract.py <cataclysm.big> <outdir> [name-substring-filter]"""
import os, struct, sys

path, outdir = sys.argv[1], sys.argv[2]
f = open(path, 'rb')
assert f.read(7) == b'RBF1.23'
n, flags = struct.unpack('<ii', f.read(8))
ents = [struct.unpack('<IIHxxIIIIB3x', f.read(32)) for _ in range(n)]


def lzss_expand(data, real):
    window = bytearray(4096)
    out = bytearray()
    pos, bitpos, nbits = 1, 0, len(data) * 8

    def bits(k):
        nonlocal bitpos
        v = 0
        for _ in range(k):
            byte = data[bitpos >> 3]
            v = (v << 1) | ((byte >> (7 - (bitpos & 7))) & 1)
            bitpos += 1
        return v

    while bitpos < nbits:
        if bits(1):
            c = bits(8); out.append(c); window[pos] = c; pos = (pos + 1) & 4095
        else:
            mp = bits(12)
            if mp == 0: break
            ml = bits(4) + 1  # BREAK_EVEN == 1
            for i in range(ml + 1):
                c = window[(mp + i) & 4095]; out.append(c); window[pos] = c; pos = (pos + 1) & 4095
        if len(out) >= real: break
    return bytes(out[:real])


bad = 0
for c1, c2, nl, stored, real, off, ts, comp in ents:
    f.seek(off)
    raw = bytearray(f.read(nl + 1))[:nl]
    m = 213
    for i in range(nl):
        raw[i] ^= m; m = raw[i]
    name = raw.decode('latin1').replace('\\', '/')
    if len(sys.argv) > 3 and sys.argv[3].lower() not in name.lower():
        continue
    data = f.read(stored)
    if comp:
        data = lzss_expand(data, real)
    if len(data) != real:
        bad += 1; print('size mismatch', name, len(data), real)
    dst = os.path.join(outdir, name)
    os.makedirs(os.path.dirname(dst) or outdir, exist_ok=True)
    open(dst, 'wb').write(data)
print('extracted', n, 'files,', bad, 'size mismatches')
