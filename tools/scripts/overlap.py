"""Per-source-file overlap: how many of HW1's string literals appear verbatim in cataclysm.exe."""
import re, sys, os, json
exe, srcroot, out = sys.argv[1], sys.argv[2], sys.argv[3]
d = open(exe, 'rb').read()
exe_strs = set(m.group().decode('latin1') for m in re.finditer(rb'[\x20-\x7e]{6,}', d))
lit = re.compile(r'"((?:[^"\\\n]|\\.){6,})"')
rows = []
for dp, _, fs in os.walk(srcroot):
    for f in fs:
        if not f.lower().endswith(('.c', '.cpp')): continue
        p = os.path.join(dp, f)
        txt = open(p, encoding='latin1').read()
        lits = set()
        for m in lit.finditer(txt):
            s = m.group(1)
            s = s.replace('\\n', '\n').replace('\\t', '\t').replace('\\"', '"').replace('\\\\', '\\')
            if '%' in s or '\n' in s:  # format strings still match as prefix chunks; split on newlines
                pass
            parts = [x for x in re.split(r'[\n\t]', s) if len(x) >= 6]
            lits.update(parts)
        if not lits: continue
        hit = sum(1 for s in lits if s in exe_strs or any(s in e for e in ()))
        rows.append((os.path.relpath(p, srcroot).replace('\\', '/'), len(lits), hit, len(txt.splitlines())))
rows.sort(key=lambda r: -r[1])
tot = sum(r[1] for r in rows); th = sum(r[2] for r in rows)
print('HW1 literals', tot, 'found in Cataclysm', th, '(%.1f%%)' % (100 * th / tot))
json.dump(rows, open(out, 'w'), indent=0)
for r in rows[:40]: print('%-40s lits=%4d hit=%4d (%3d%%) lines=%d' % (r[0], r[1], r[2], 100 * r[2] // r[1], r[3]))
