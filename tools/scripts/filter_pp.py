"""Keep only declarations that came from HW1 headers in an MSVC /E preprocessed file, drop function
bodies and MSVC-only keywords, so Ghidra's CParser can ingest the result as a type vocabulary.
Usage: filter_pp.py in.i out.h"""
import re, sys

src, dst = sys.argv[1], sys.argv[2]
keep = False
out = []
for line in open(src, encoding='latin1'):
    m = re.match(r'#line \d+ "(.*)"', line)
    if m:
        p = m.group(1).replace('\\\\', '/').lower()
        keep = 'hw1-2003' in p or 'sdlstub' in p
        continue
    if keep and line.strip():
        out.append(line)
text = ''.join(out)

# Drop function definition bodies (a top-level '{' that follows a ')'), leaving a ';'.
buf, depth, i, n = [], 0, 0, len(text)
while i < n:
    c = text[i]
    if depth == 0 and c == '{' and re.search(r'\)\s*$', ''.join(buf[-300:])):
        j, d = i, 0
        while j < n:
            if text[j] == '{': d += 1
            elif text[j] == '}':
                d -= 1
                if d == 0: break
            j += 1
        buf.append(';')
        i = j + 1
        continue
    if c == '{': depth += 1
    elif c == '}': depth -= 1
    buf.append(c)
    i += 1
text = ''.join(buf)

KEYWORDS = r'\b(__inline|__forceinline|__cdecl|__stdcall|__fastcall|static|extern)\b'
text = re.sub(KEYWORDS, '', text)
text = re.sub(r'__declspec\([^)]*\)', '', text)
text = re.sub(r'#pragma[^\n]*\n', '\n', text)
open(dst, 'w').write(text)
print('lines', text.count('\n'))
