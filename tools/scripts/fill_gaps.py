"""Create functions at likely starts inside Ghidra's code gaps: 16-byte-aligned addresses that follow
NOP/INT3 padding (MSVC pads between functions), or the gap start itself when it is aligned.
Repeats until no new functions appear. Usage: fill_gaps.py <exe> [program-name]"""
import json, sys, urllib.parse, urllib.request

exe = sys.argv[1]
prog = sys.argv[2] if len(sys.argv) > 2 else 'cataclysm.exe'
B = 'http://127.0.0.1:8089'
d = open(exe, 'rb').read()
IB = 0x400000
PAD = (0x90, 0xcc)


def get(path, **q):
    q['program'] = prog
    return json.load(urllib.request.urlopen(B + path + '?' + urllib.parse.urlencode(q), timeout=600))


def post(path, body):
    req = urllib.request.Request(B + path + '?program=' + prog, json.dumps(body).encode(), {'Content-Type': 'application/json'})
    return json.load(urllib.request.urlopen(req, timeout=600))


for rnd in range(10):
    gaps = [g for g in get('/find_code_gaps', min_size=16, limit=10000)['gaps'] if int(g['start'], 16) < 0x6f0000]
    total = sum(g['size'] for g in gaps)
    cands = []
    for g in gaps:
        s, e = int(g['start'], 16), int(g['end'], 16)
        for a in range((s + 15) & ~15, e, 16):
            o = a - IB
            if d[o] in PAD: continue
            if a == s or d[o - 1] in PAD or d[o - 1] == 0xc3:
                cands.append(a)
    ok = 0
    for a in cands:
        r = post('/create_function', {'address': '%08x' % a, 'disassemble_first': True})
        if r.get('success'): ok += 1
    print('round', rnd, 'gaps', len(gaps), 'gap bytes', total, 'candidates', len(cands), 'created', ok, flush=True)
    if not ok: break
print(get('/get_function_count'))
