"""Emulate cataclysm.exe particle functions in Ghidra and emit Rust test rows.

1. The particle fill (partFillGenericParticles, 0x4dbae0) on a random `pat` image and random
   stream-25 state -> particle records + final stream state.
2. partUpdateSystem (0x4dae60) on random particle records (points/lines/mesh/ebolt systems,
   local and world space, with and without drag) -> updated records, header, return value.

Usage: python emu_part.py <out.rs> [seed]
"""
import base64, json, math, random, struct, sys, urllib.request

URL = "http://127.0.0.1:8089/emulate_function?program=cataclysm.exe"
PAT = 0x8f5868
STREAMS = 0xad50c0
CALLBACK = 0x8f5f74
PSYS = 0x10000000
MAT = 0x10050000
STACK = 0x10060000


def _table():
    ent = [0] * 16384
    for i in range(8192):
        for j, e in ((0, 1023), (8192, 1024)):
            x = struct.unpack('<d', struct.pack('<Q', (((i << 7) | (e << 20)) << 32)))[0]
            ent[i + j] = (struct.unpack('<Q', struct.pack('<d', math.sqrt(x)))[0] >> 32) & 0xFFFFF
    return b''.join(struct.pack('<I', v) for v in ent)


TABLE = _table()


def fb(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


def f32(x):
    return struct.unpack('<f', struct.pack('<f', x))[0]


def call(func, mem, stack, reads):
    body = {
        "function": func,
        "registers": {"ESP": hex(STACK), "EBP": hex(STACK + 0x800)},
        "memory": [{"address": hex(a), "data": base64.b64encode(bytes(d)).decode()} for a, d in mem + [(STACK, stack), (0x8d3ff4, TABLE)]],
        "max_steps": 3000000,
        "read_memory_after": [{"address": hex(a), "length": n} for a, n in reads],
    }
    req = urllib.request.Request(URL, data=json.dumps(body).encode(), headers={"Content-Type": "application/json"})
    o = json.load(urllib.request.urlopen(req, timeout=600))
    assert o.get('stop_reason') == 'return', {k: o[k] for k in o if k != 'memory'}
    return o, [bytes.fromhex(m['hex']) for m in o['memory']]


def rot(r):
    a, b = r.uniform(0, 6.28), r.uniform(-0.6, 0.6)
    ca, sa, cb, sb = math.cos(a), math.sin(a), math.cos(b), math.sin(b)
    h = [ca * cb, sa * cb, sb]
    up = [-ca * sb, -sa * sb, cb]
    right = [h[1] * up[2] - h[2] * up[1], h[2] * up[0] - h[0] * up[2], h[0] * up[1] - h[1] * up[0]]
    return [f32(x) for x in up + right + h]


def rnd_pat(r):
    w = [0] * 114
    F = lambda i, v: w.__setitem__(i, fb(v))
    for i in range(6):
        F(i, r.uniform(-300, 300))
    alpha = r.random() < 0.5
    world = r.random() < 0.5
    xyz = r.random() < 0.2
    w[6] = (2 if alpha else 0) | (4 if world else 0) | (0x40 if xyz else 0) | (8 if r.random() < 0.3 else 0)
    F(7, r.choice([1.0, 0.98, 0.9]))
    F(8, r.uniform(1, 200))
    for i in (9, 10, 11):
        F(i, r.uniform(0.5, 3))
    for i in range(12, 18):
        F(i, r.uniform(-50, 50))
    for i in range(19, 41):
        F(i, r.uniform(-100, 100) if r.random() < 0.8 else 0.0)
    F(23, r.uniform(-2, 2))
    # The emulator has no FSIN/FCOS: theta is forced to +0 and the two instructions are
    # patched (FCOS -> FLD1, FSIN -> FSTP ST0), which yields exactly cos(0) and sin(0).
    F(17, 0.0)
    F(22, 0.0)
    F(24, 0.0)
    w[41] = r.choice([0, 1])
    F(42, r.uniform(0, 2))
    w[43] = r.choice([4, 16, 30])
    for i in range(44, 60):
        F(i, r.uniform(-1, 1.2))
    w[60] = r.choice([0, 1])
    for i in range(61, 69):
        F(i, r.uniform(-2, 5))
    w[69] = r.choice([0, 0x12345678])
    F(70, r.uniform(-1, 20))
    F(71, r.uniform(0, 3))
    w[72] = r.choice([0, 3])
    w[73] = r.choice([0xffffffff, 2])
    w[74] = 0xffffffff
    for i in (75, 76, 77):
        F(i, r.uniform(-5000, 5000))
    w[78] = MAT
    w[79] = 8
    w[80:84] = [0, 0, 32, 32]
    F(86, 10.0)
    F(87, 5.0)
    F(88, r.uniform(0, 3))
    w[89] = 1 | (r.choice([0, 3]) << 16)
    for i in range(90, 108):
        F(i, r.uniform(-4, 4))
    for i in range(108, 114):
        w[i] = r.choice([0, 1])
    return w


def rnd_stream(r):
    return [r.getrandbits(32) for _ in range(4)] + [r.getrandbits(32)]


def fill_case(r):
    pat = rnd_pat(r)
    m = rot(r)
    st = rnd_stream(r)
    n = r.choice([1, 2, 3])
    world = 1 if pat[6] & 4 else 0
    psys = bytearray(0x1c + n * 0x100)
    patb = struct.pack('<114I', *pat)
    streams = bytearray(27 * 20)
    struct.pack_into('<5I', streams, 25 * 20, *st)
    mem = [(PAT, patb), (MAT, struct.pack('<9f', *m)), (PSYS, psys), (STREAMS, streams), (CALLBACK, bytes(12)),
           (0x4dba53, bytes([0xd9, 0xe8])), (0x4dba5d, bytes([0xdd, 0xd8]))]
    stack = struct.pack('<6I', 0xdeadbeef, n, PSYS, 0x1c, 1, world)
    _, out = call("0x004dbae0", mem, stack, [(PSYS + 0x1c, n * 0x100), (STREAMS + 25 * 20, 20)])
    parts = [list(struct.unpack('<64I', out[0][i * 0x100:(i + 1) * 0x100])) for i in range(n)]
    return pat, m, st, n, parts, list(struct.unpack('<5I', out[1]))


def rnd_particle(r, kind, world):
    w = [0] * 64
    F = lambda i, v: w.__setitem__(i, fb(v))
    F(0, r.choice([r.uniform(-0.2, 2.0), 0.03]))
    F(1, r.choice([0.0, 0.0, r.uniform(0, 0.2)]))
    for i in range(2, 6):
        F(i, r.uniform(-300, 300))
    v = [r.uniform(-1, 1) for _ in range(3)]
    n = math.sqrt(sum(x * x for x in v)) or 1
    for i in range(3):
        F(6 + i, v[i] / n)
        F(9 + i, r.uniform(-2000, 2000))
    for i in range(12, 20):
        # (+0.0 rather than -0.0: Ghidra's FCOM treats -0.0 and 0.0 as different, the x87 does not)
        F(i, r.uniform(-0.5, 1.2) if i < 16 else (r.uniform(-2, 2) if r.random() < 0.7 else 0.0))
    F(20, r.uniform(0, 100))
    F(21, r.uniform(0, 100))
    F(22, r.choice([0.0, r.uniform(-200, 200)]))
    w[23] = r.choice([0, 1])
    F(24, r.uniform(-0.5, 1.5))
    F(25, r.uniform(-3, 3))
    F(26, r.uniform(-50, 100))
    F(27, r.choice([0.0, r.uniform(-300, 300)]))
    w[28] = r.choice([0, 1])
    F(29, 0.8)
    w[30] = 16
    w[32] = 0xffffffff
    F(33, r.uniform(-3, 3))
    F(34, r.choice([0.0, r.uniform(-5, 5)]))
    for i in range(35, 44):
        F(i, r.uniform(-500, 500))
    for i in range(44, 47):
        F(i, r.uniform(0, 1))
    F(48, 10.0)
    for i in range(51, 57):
        F(i, r.uniform(-6, 6))
    F(61, r.uniform(-1, 10))
    w[63] = r.choice([0, 8, 0x40, 0x10 | 8])
    return w


def update_case(r):
    kind = r.choice([0, 2, 3, 5])
    world = r.random() < 0.5
    flags = (4 if world else 0) | r.choice([0, 2]) | r.choice([0, 8])
    hdr = {0: 0x1c, 2: 0x1c, 3: 0x30, 5: 0x24}[kind]
    n = r.choice([1, 2, 4])
    drag = r.choice([1.0, 0.97, 0.5])
    ang = f32(r.uniform(-3, 3))
    angd = r.choice([0.0, f32(r.uniform(-2, 2))])
    dt = f32(r.choice([0.0625, 0.03125, 0.0, 0.1]))
    parts = [rnd_particle(r, kind, world) for _ in range(n)]
    head = bytearray(hdr)
    struct.pack_into('<II', head, 0, kind, flags)
    head[8] = 1
    struct.pack_into('<H', head, 10, n)
    struct.pack_into('<ffff', head, 12, 0.0, drag, ang, angd)
    psys = bytes(head) + b''.join(struct.pack('<64I', *p) for p in parts)
    stack = struct.pack('<II', 0xdeadbeef, PSYS) + struct.pack('<f', dt)
    o, out = call("0x004dae60", [(PSYS, psys)], stack, [(PSYS, len(psys))])
    ret = int(o['registers']['EAX'], 16) & 0xff
    res = out[0]
    after = [list(struct.unpack('<64I', res[hdr + i * 0x100: hdr + (i + 1) * 0x100])) for i in range(n)]
    hflags, = struct.unpack_from('<I', res, 4)
    hang, = struct.unpack_from('<I', res, 0x14)
    return kind, flags, drag, fb(ang), fb(angd), fb(dt), parts, after, ret, hflags, hang


def hx(v):
    return '0x%08x' % v


def arr(v):
    return '[' + ', '.join(hx(x) for x in v) + ']'


def main():
    out = sys.argv[1]
    r = random.Random(int(sys.argv[2]) if len(sys.argv) > 2 else 7)
    lines = ['// Generated by emu_part.py from Ghidra emulation of cataclysm.exe. Do not edit.', '']
    lines.append('pub const FILL_CASES: &[FillCase] = &[')
    for i in range(24):
        pat, m, st, n, parts, st_after = fill_case(r)
        lines.append('    FillCase {')
        lines.append('        pat: &' + arr(pat) + ',')
        lines.append('        mat: ' + arr([fb(x) for x in m]) + ',')
        lines.append('        stream: ' + arr(st) + ',')
        lines.append('        particles: &[' + ', '.join(arr(p) for p in parts) + '],')
        lines.append('        stream_after: ' + arr(st_after) + ',')
        lines.append('    },')
        print('fill', i, n, file=sys.stderr)
    lines.append('];')
    lines.append('')
    lines.append('pub const UPDATE_CASES: &[UpdateCase] = &[')
    for i in range(40):
        kind, flags, drag, ang, angd, dt, parts, after, ret, hflags, hang = update_case(r)
        lines.append('    UpdateCase {')
        lines.append(f'        kind: {kind}, flags: {hx(flags)}, drag: {hx(fb(drag))}, ang: {hx(ang)}, ang_delta: {hx(angd)}, dt: {hx(dt)},')
        lines.append('        before: &[' + ', '.join(arr(p) for p in parts) + '],')
        lines.append('        after: &[' + ', '.join(arr(p) for p in after) + '],')
        lines.append(f'        dead: {str(ret != 0).lower()}, flags_after: {hx(hflags)}, ang_after: {hx(hang)},')
        lines.append('    },')
        print('update', i, kind, file=sys.stderr)
    lines.append('];')
    open(out, 'w').write('\n'.join(lines) + '\n')


main()
