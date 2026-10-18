"""Send a C header to ghidra-mcp /import_data_types; on parse failure, report the failing line with context.
Usage: import_types.py header.h [prelude.h]"""
import json, re, sys, urllib.request

text = open(sys.argv[1], encoding='latin1').read()
if len(sys.argv) > 2:
    text = open(sys.argv[2]).read() + '\n' + text
req = urllib.request.Request('http://127.0.0.1:8089/import_data_types', json.dumps({'source': text}).encode(),
                             {'Content-Type': 'application/json'})
r = json.load(urllib.request.urlopen(req, timeout=900))
print({k: r.get(k) for k in ('status', 'parse_succeeded', 'types_added', 'types_after')}); print((r.get('error') or '')[:400]); print((str(r.get('messages') or r.get('parser_messages') or ''))[:800])
m = re.search(r'at line (\d+), column (\d+)', r.get('error', '') or '')
if m:
    ln = int(m.group(1)); lines = text.split('\n')
    for i in range(max(0, ln - 6), min(len(lines), ln + 2)):
        print('%6d%s %s' % (i + 1, '>' if i + 1 == ln else ' ', lines[i]))
