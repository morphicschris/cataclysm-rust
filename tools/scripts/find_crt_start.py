"""Find MSVC6 CRT startup candidates (WinMainCRTStartup / mainCRTStartup) in a PE by byte pattern.

The game's own CRT entry sits in .text, unreferenced, because the protection wrapper's entry replaced it
in the PE header. Pattern: SEH prologue (push ebp; mov ebp,esp; push -1; push scope; push handler;
mov eax,fs:[0]) followed shortly by `call [GetVersion]`.
"""
import re, struct, sys

d = open(sys.argv[1], 'rb').read()
pe = struct.unpack_from('<I', d, 0x3c)[0]
nsec, = struct.unpack_from('<H', d, pe + 6)
optsz, = struct.unpack_from('<H', d, pe + 20)
o = pe + 24
ib, = struct.unpack_from('<I', d, o + 28)
secs = []
s = o + optsz
for i in range(nsec):
    n, vs, va, rs, rp = struct.unpack_from('<8sIIII', d, s + 40 * i)
    secs.append((n.rstrip(b'\0').decode(), va, vs, rp, rs))

def off(rva):
    for n, va, vs, rp, rs in secs:
        if va <= rva < va + max(vs, rs): return rva - va + rp

def rva_of(fo):
    for n, va, vs, rp, rs in secs:
        if rp <= fo < rp + rs: return fo - rp + va

# locate IAT slot of kernel32!GetVersion
irva = struct.unpack_from('<I', d, o + 96 + 8)[0]
i = off(irva); getversion_slot = None
while True:
    ilt, _, _, name, iat = struct.unpack_from('<IIIII', d, i)
    if name == 0: break
    t = off(ilt or iat); k = 0
    while True:
        e, = struct.unpack_from('<I', d, t)
        if e == 0: break
        if not e & 0x80000000:
            q = off(e) + 2
            if d[q:d.index(b'\0', q)] == b'GetVersion': getversion_slot = ib + iat + 4 * k
        t += 4; k += 1
    i += 20
print('GetVersion IAT slot', hex(getversion_slot))
call = b'\xff\x15' + struct.pack('<I', getversion_slot)
pro = re.compile(rb'\x55\x8b\xec\x6a\xff\x68....\x68....\x64\xa1\x00\x00\x00\x00', re.S)
for m in pro.finditer(d):
    win = d[m.start():m.start() + 64]
    if call in win:
        va = ib + rva_of(m.start())
        sec = [n for n, va0, vs, rp, rs in secs if rp <= m.start() < rp + rs][0]
        print('candidate %08x in %s  bytes after prologue: %s' % (va, sec, win[20:40].hex(' ')))
