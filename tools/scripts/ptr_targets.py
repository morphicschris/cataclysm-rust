"""List code addresses referenced as absolute pointers from data sections or push-imm32 in code,
that fall in a given VA range. Used to seed function creation in code only reached via tables
(e.g. compiled KAS missions). Prints one hex address per line."""
import struct, sys

exe, lo, hi = sys.argv[1], int(sys.argv[2], 16), int(sys.argv[3], 16)
d = open(exe, 'rb').read()
IB = 0x400000
pe = struct.unpack_from('<I', d, 0x3c)[0]
nsec, = struct.unpack_from('<H', d, pe + 6)
optsz, = struct.unpack_from('<H', d, pe + 20)
s = pe + 24 + optsz
secs = [struct.unpack_from('<8sIIII', d, s + 40 * i) for i in range(nsec)]
targets = set()
for name, vs, va, rs, rp in secs:
    name = name.rstrip(b'\0')
    blob = d[rp:rp + rs]
    if name == b'.text':
        # push imm32 (0x68) and mov reg/mem, imm32 forms carrying code addresses
        for i in range(len(blob) - 5):
            if blob[i] in (0x68, 0xb8, 0xb9, 0xba, 0xbb, 0xbe, 0xbf):
                v, = struct.unpack_from('<I', blob, i + 1)
                if lo <= v < hi: targets.add(v)
    elif name in (b'.data', b'.rdata'):
        for i in range(0, len(blob) - 3, 4):
            v, = struct.unpack_from('<I', blob, i)
            if lo <= v < hi: targets.add(v)
# keep only targets whose byte looks like a plausible instruction start (not mid-padding)
out = sorted(t for t in targets if d[t - IB] not in (0x00, 0xcc))
print('\n'.join('%08x' % t for t in out))
